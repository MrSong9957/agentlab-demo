#!/usr/bin/env python3
"""嵌套 claude CLI 验收的安全入口（025 批次防回归）。

背景（2026-10-26 事故）：文档加载验收中，嵌套 `claude -p` 未隔离配置目录，
~/.claude/settings.json 的 env 块（ANTHROPIC_BASE_URL 指向真实服务）覆盖了
外部注入的 mock 地址，完成一次真实付费调用。settings env 优先级高于进程
环境变量——只靠「启动前导出 mock 变量」不能保证目标，必须隔离配置并核查
各层 settings 合并后的最终生效值。

子命令（全部只用假凭据与本地监听器，零真实服务、零真实密钥）：
  run <prompt>     安全运行一次 claude -p：隔离 CLAUDE_CONFIG_DIR＋预检＋行为断言
  demo-red         复现旧流程缺陷：假 HOME 的 settings env 指向错误本地监听器
                   （canary），ambient 仍是 mock——证明旧流程被全局 settings 劫持、
                   错误端点收到请求且回复被当作模型输出
  demo-green       新流程：mock 收到请求、canary 零请求、输出含 mock 标记
  demo-negative    生效 BASE_URL 非批准 mock 时：预检在子进程启动（请求发出）前
                   失败，mock 与 canary 均零请求

防护设计（对应事故根因）：
- 隔离入口＝CLAUDE_CONFIG_DIR（工具支持的独立配置目录，settings 与状态全部
  重定向，不再读取真实 ~/.claude）；子进程环境从零构造（仅 PATH 继承），
  不携带任何真实凭据或生产端点变量。
- 预检＝按 user→project→local→managed 优先级合并各层 settings.json 的 env 块：
  BASE_URL 覆盖值 ≠ 本次 mock 地址即中止；settings 内出现任何凭据键即中止。
- 行为断言（不只看启动参数）：mock 必须实际收到请求；canary 必须零请求；
  输出标记必须来自 mock。
- 兜底遏制：HTTP(S)_PROXY 指向本机关闭端口（NO_PROXY 放行 localhost），
  意外指向真实端点的请求在本机即失败，不出网。

退出码：0＝通过；1＝断言失败/异常；2＝预检拦截（run 与 demo-green 路径；
demo-negative 的「按预期拦截」计为通过，exit 0）。
"""
import argparse
import atexit
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

FAKE_TOKEN = "FAKE-TOKEN-LOCAL-MOCK-ONLY"
FAKE_MODEL = "mock-load-test"
MOCK_MARKER = "MOCK-REPLY-MARKER"
CANARY_MARKER = "CANARY-REPLY-MARKER"
DENY_PREFIXES = ("ANTHROPIC_", "CLAUDE_CODE_", "AWS_", "BEDROCK_")
DEAD_PROXY = "http://127.0.0.1:9"
CLAUDE_TIMEOUT = 240


def make_listener(marker):
    """本地 HTTP 监听器：兼容 stream/非 stream 的最小 Anthropic /v1/messages 应答。

    只记录方法、路径与计数，不记录请求头值；/v1/messages 请求体仅保留最后一份于
    内存（供 run 模式 --capture 落盘做上下文注入检查），不写入任何日志。
    """
    state = {"count": 0, "model_calls": 0, "paths": [], "last_body": b""}

    class Handler(BaseHTTPRequestHandler):
        def _send(self, code, ctype, payload):
            self.send_response(code)
            self.send_header("content-type", ctype)
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def _serve(self):
            n = int(self.headers.get("content-length") or 0)
            body = self.rfile.read(n) if n else b""
            try:
                parsed = json.loads(body) if body else {}
            except ValueError:
                parsed = {}
            state["count"] += 1
            state["paths"].append(f"{self.command} {self.path}")
            if self.path.startswith("/v1/messages"):
                state["last_body"] = body
                if "count_tokens" not in self.path:
                    state["model_calls"] += 1
            if self.path.startswith("/v1/messages/count_tokens"):
                self._send(200, "application/json", b'{"input_tokens":1}')
            elif self.path.startswith("/v1/messages"):
                model = parsed.get("model", "mock")
                if parsed.get("stream"):
                    events = []

                    def ev(kind, data):
                        events.append(f"event: {kind}\ndata: {json.dumps(data)}\n\n")

                    ev("message_start", {"type": "message_start", "message": {
                        "id": "msg_mock", "type": "message", "role": "assistant",
                        "model": model, "content": [], "stop_reason": None,
                        "usage": {"input_tokens": 1, "output_tokens": 1}}})
                    ev("content_block_start", {"type": "content_block_start", "index": 0,
                                               "content_block": {"type": "text", "text": ""}})
                    ev("content_block_delta", {"type": "content_block_delta", "index": 0,
                                               "delta": {"type": "text_delta", "text": marker}})
                    ev("content_block_stop", {"type": "content_block_stop", "index": 0})
                    ev("message_delta", {"type": "message_delta",
                                         "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                                         "usage": {"output_tokens": 1}})
                    ev("message_stop", {"type": "message_stop"})
                    self._send(200, "text/event-stream", "".join(events).encode())
                else:
                    msg = {"id": "msg_mock", "type": "message", "role": "assistant",
                           "model": model, "content": [{"type": "text", "text": marker}],
                           "stop_reason": "end_turn", "stop_sequence": None,
                           "usage": {"input_tokens": 1, "output_tokens": 1}}
                    self._send(200, "application/json", json.dumps(msg).encode())
            else:
                self._send(404, "application/json", b'{"error":"not found"}')

        do_POST = _serve
        do_GET = _serve

        def log_message(self, *args):
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd, state


def write_settings(directory, env_obj):
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, "settings.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump({"env": env_obj}, f, indent=2)
    return path


def preflight(cfg_dir, cwd, mock_url):
    """合并各层 settings 的 env，返回问题清单（空列表＝通过）。

    覆盖顺序 user→project→local→managed（后写覆盖前写，与工具生效优先级一致）；
    只检查与模型端点/凭据相关的键，不读取其他配置。
    """
    layers = [
        ("user(CLAUDE_CONFIG_DIR)", os.path.join(cfg_dir, "settings.json")),
        ("project", os.path.join(cwd, ".claude", "settings.json")),
        ("local", os.path.join(cwd, ".claude", "settings.local.json")),
        ("managed", "/etc/claude-code/managed-settings.json"),
    ]
    merged, sources = {}, {}
    for name, path in layers:
        if not os.path.isfile(path):
            continue
        try:
            with open(path, encoding="utf-8") as f:
                doc = json.load(f)
        except ValueError as e:
            return [f"预检失败：{path} 不是合法 JSON（{e}）"]
        if not isinstance(doc, dict):
            return [f"预检失败：{path} 顶层不是 JSON 对象"]
        env = doc.get("env")
        if env is None:
            env = {}
        if not isinstance(env, dict):
            return [f"预检失败：{path} 的 env 块不是对象"]
        for key, value in env.items():
            merged[key] = value
            sources[key] = f"{name} {path}"
    problems = []
    url = merged.get("ANTHROPIC_BASE_URL")
    if url is not None and url != mock_url:
        problems.append(f"ANTHROPIC_BASE_URL 被 [{sources['ANTHROPIC_BASE_URL']}] 覆盖为 {url}"
                        f"（≠ 批准 mock {mock_url}），拒绝运行")
    for key in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"):
        if key in merged:
            problems.append(f"{key} 出现在 [{sources[key]}]（验收配置不得存放任何凭据），拒绝运行")
    # 拒绝清单语义：其余任何模型相关覆盖键（Bedrock 等新端点形态、模型名、代理指向）
    # 默认拦截，防止预检键盲区（安全攻防 F-1）；只报键名与来源，不报值。
    for key in sorted(merged):
        if key != "ANTHROPIC_BASE_URL" and key.startswith(DENY_PREFIXES):
            problems.append(f"{key} 被 [{sources[key]}] 覆盖（非本次批准的模型相关键，默认拦截）")
    return problems


def claude_env(home, tmp, base_url, cfg_dir=None):
    """子进程环境从零构造：不继承真实凭据与生产端点变量（PATH 除外）。"""
    env = {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "HOME": home,
        "TMPDIR": tmp,
        "ANTHROPIC_BASE_URL": base_url,
        "ANTHROPIC_AUTH_TOKEN": FAKE_TOKEN,
        "ANTHROPIC_MODEL": FAKE_MODEL,
        "TERM": "dumb",
        "LANG": "C.UTF-8",
        "DISABLE_TELEMETRY": "1",
        "DISABLE_ERROR_REPORTING": "1",
        "DISABLE_BUG_COMMAND": "1",
        "DISABLE_AUTOUPDATER": "1",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        "HTTP_PROXY": DEAD_PROXY,
        "HTTPS_PROXY": DEAD_PROXY,
        "NO_PROXY": "localhost,127.0.0.1",
    }
    if cfg_dir is not None:
        env["CLAUDE_CONFIG_DIR"] = cfg_dir
    return env


TIMEOUT = object()  # 与「未启动」「返回码」可区分的超时哨兵（安全攻防 F-3）


def proc_ok(proc):
    """子进程正常返回且退出码 0（区分未启动与超时）。"""
    return proc is not None and proc is not TIMEOUT and proc.returncode == 0


def run_claude(prompt, cwd, env):
    try:
        return subprocess.run(
            ["claude", "-p", prompt],
            cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            capture_output=True, text=True, timeout=CLAUDE_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        return TIMEOUT


def finish(mode, problems_ok, proc, mock, canary, root, keep, expect_marker, exit_code=None):
    mock_n = mock["state"]["model_calls"]
    canary_n = canary["state"]["count"]
    marker_seen = ""
    if proc is not None and proc is not TIMEOUT and proc.stdout:
        marker_seen = expect_marker if expect_marker in proc.stdout else "(未命中)"
    if proc is None:
        claude_exit = "n/a(未启动)"
    elif proc is TIMEOUT:
        claude_exit = f"timeout({CLAUDE_TIMEOUT}s)"
    else:
        claude_exit = str(proc.returncode)
    print(f"[summary] mode={mode} claude_exit={claude_exit} "
          f"mock_model_calls={mock_n} canary_requests={canary_n} marker={marker_seen or '-'}")
    if proc is not None and proc is not TIMEOUT and proc.stdout:
        print("[claude stdout]", proc.stdout.strip()[:500])
    if proc is not None and proc is not TIMEOUT and proc.stderr:
        print("[claude stderr tail]", proc.stderr.strip()[-500:])
    ok = problems_ok and (not expect_marker or marker_seen == expect_marker)
    for httpd in (mock["httpd"], canary["httpd"]):
        httpd.shutdown()
        httpd.server_close()
    if not keep:
        shutil.rmtree(root, ignore_errors=True)
    sys.exit(exit_code if exit_code is not None else (0 if ok else 1))


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("mode", choices=["run", "demo-red", "demo-green", "demo-negative"])
    ap.add_argument("prompt", nargs="?", default="回复 ok",
                    help="run 模式传给 claude -p 的提示词")
    ap.add_argument("--cwd", default="/workspace", help="claude 子进程工作目录（默认 /workspace）")
    ap.add_argument("--capture", default=None,
                    help="run 模式：把 mock 收到的首个 /v1/messages 请求体保存到该路径"
                         "（供文档加载类验收检查上下文注入；请求体只含仓库上下文与假凭据无关字段）")
    ap.add_argument("--keep", action="store_true", help="保留临时目录便于排查")
    args = ap.parse_args()

    root = tempfile.mkdtemp(prefix="claude-mock-")
    if not args.keep:
        atexit.register(shutil.rmtree, root, ignore_errors=True)  # 崩溃路径也清理（F-6）
    tmp = os.path.join(root, "tmp")
    os.makedirs(tmp)
    home = os.path.join(root, "home")
    os.makedirs(home)
    demo_cwd = os.path.join(root, "cwd")
    os.makedirs(demo_cwd)
    cwd = args.cwd if args.mode == "run" else demo_cwd

    mock_httpd, mock_state = make_listener(MOCK_MARKER)
    canary_httpd, canary_state = make_listener(CANARY_MARKER)
    mock = {"httpd": mock_httpd, "state": mock_state, "url": f"http://127.0.0.1:{mock_httpd.server_address[1]}"}
    canary = {"httpd": canary_httpd, "state": canary_state, "url": f"http://127.0.0.1:{canary_httpd.server_address[1]}"}
    print(f"[listeners] mock={mock['url']} canary={canary['url']}")
    print(f"[claude] bin={shutil.which('claude') or 'claude(未解析)'}")  # PATH 审计（F-2）

    if args.mode == "demo-red":
        # 旧流程复现：不隔离配置（无 CLAUDE_CONFIG_DIR），假 HOME 的
        # ~/.claude/settings.json env 指向 canary；ambient 仍指向 mock。
        # 若 settings env 覆盖 ambient（事故根因），canary 将收到请求。
        write_settings(os.path.join(home, ".claude"),
                       {"ANTHROPIC_BASE_URL": canary["url"], "ANTHROPIC_AUTH_TOKEN": FAKE_TOKEN})
        proc = run_claude("回复 ok", cwd, claude_env(home, tmp, mock["url"]))
        ok = (proc_ok(proc)
              and canary["state"]["model_calls"] >= 1 and mock["state"]["count"] == 0)
        print(f"[expect] canary 模型调用>=1 且 mock==0（复现旧流程误选）；"
              f"实际 canary={canary['state']['model_calls']} mock={mock['state']['count']}")
        finish("demo-red", ok, proc, mock, canary, root, args.keep, CANARY_MARKER)

    if args.mode in ("run", "demo-green"):
        cfg = os.path.join(root, "cfg")
        write_settings(cfg, {})  # 显式空 env：隔离目录无任何覆盖
        problems = preflight(cfg, cwd, mock["url"])
        if problems:
            for p in problems:
                print("[preflight-blocked]", p)
            finish(args.mode, False, None, mock, canary, root, args.keep, "", exit_code=2)
        proc = run_claude(args.prompt, cwd, claude_env(home, tmp, mock["url"], cfg_dir=cfg))
        ok = (proc_ok(proc)
              and mock["state"]["model_calls"] >= 1 and canary["state"]["count"] == 0)
        if args.capture and mock["state"]["last_body"]:
            fd = os.open(args.capture, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(mock["state"]["last_body"])
            os.chmod(args.capture, 0o600)  # 已存在的旧文件也收敛到 0600（F-5）
            print(f"[capture] mock 请求体已保存（0600）：{args.capture}")
        print(f"[expect] mock 模型调用>=1 且 canary==0；"
              f"实际 mock={mock['state']['model_calls']} canary={canary['state']['count']}")
        finish(args.mode, ok, proc, mock, canary, root, args.keep, MOCK_MARKER)

    # demo-negative：隔离目录被写入指向 canary 的 env 覆盖——预检必须在
    # 子进程启动前拦截，两个监听器都收不到任何请求。
    cfg = os.path.join(root, "cfg")
    write_settings(cfg, {"ANTHROPIC_BASE_URL": canary["url"]})
    problems = preflight(cfg, cwd, mock["url"])
    if problems:
        for p in problems:
            print("[preflight-blocked]", p)
    ok = bool(problems) and mock["state"]["count"] == 0 and canary["state"]["count"] == 0
    print(f"[expect] 预检拦截且 mock==canary==0；实际 mock={mock['state']['count']} canary={canary['state']['count']}")
    finish("demo-negative", ok, None, mock, canary, root, args.keep, "")


if __name__ == "__main__":
    main()
