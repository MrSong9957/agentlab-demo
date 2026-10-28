#!/usr/bin/env python3
"""033 B1/B2 部署后验证：直接驱动正式路径二进制（本地 mock 全链路，假凭据）。

会话 A（TCHAT_SANDBOX=1，正常操作＋B2＋spawn 失败回传＋MCP 面＋apply_patch）：
  mock 第一轮返回四工具调用；第二轮收尾。断言来自 mock 捕获的工具结果与文件副作用。
会话 B（TCHAT_SANDBOX=1，B1）：deny 目标预建硬链接 → exec 工具启动前显式拒绝。
"""
import fcntl
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN = "/workspace/target/debug/terminal-chat"
KEY = "fake-deploy-033-key"
KEY_ENV = "TCHAT_DEPLOY_033_KEY"
VERDICT = []


def check(name, cond, detail=""):
    VERDICT.append((name, bool(cond)))
    print(("PASS " if cond else "FAIL ") + name + ((" | " + str(detail)[:300]) if detail else ""))


class _Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        self.server.mock._handle(self)

    def log_message(self, *a):
        pass


class Mock:
    """chat-completions 本地 mock：按序取响应，用尽后重复最后一条；记录请求。"""

    def __init__(self, responses):
        self.responses = list(responses)
        self._last = responses[-1] if responses else "{}"
        self.requests = []
        self.lock = threading.Lock()
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        self.httpd.mock = self
        self.port = self.httpd.server_address[1]
        self.url = f"http://127.0.0.1:{self.port}"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def _handle(self, h):
        n = int(h.headers.get("Content-Length", 0))
        body = h.rfile.read(n).decode()
        with self.lock:
            self.requests.append((h.headers.get("Authorization"), body))
            resp = self.responses.pop(0) if self.responses else self._last
        data = resp.encode()
        h.send_response(200)
        h.send_header("Content-Type", "application/json")
        h.send_header("Content-Length", str(len(data)))
        h.end_headers()
        h.wfile.write(data)

    def shutdown(self):
        self.httpd.shutdown()
        self.httpd.server_close()


def tool_call(cid, name, args):
    return {"id": cid, "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}


def round_msg(calls):
    return json.dumps({"choices": [{"message": {"tool_calls": calls}}]})


def done_msg(marker):
    return json.dumps({"choices": [{"message": {"content": marker, "finish_reason": "stop"}}]})


def write_config(cfg_dir, user_dir, url):
    with open(os.path.join(cfg_dir, "manual.json"), "w") as f:
        json.dump({"selected_provider": "deepseek", "selected_model": "deepseek-flash",
                   "providers": {"deepseek": {"base_url": url, "key_ref": f"env:{KEY_ENV}"}}}, f)
    fp = hashlib.sha256(KEY.encode()).hexdigest()[:16]
    origin = f"http://127.0.0.1:{url.rsplit(':', 1)[1]}"
    with open(os.path.join(user_dir, "authorized-origins.json"), "w") as f:
        json.dump({"grants": [{"origin": origin, "cred_fp": fp, "added_at": 0}]}, f)


def run_cli(work, cfg, user, extra_env=None, pass_fds=()):
    env = dict(os.environ)
    env.pop("TCHAT_PUBLIC_CATALOG_URL", None)
    env.update({"TERM": "dumb", "TCHAT_CONFIG_DIR": cfg, "TCHAT_USER_DIR": user,
                KEY_ENV: KEY, "TZ": "UTC", "LC_ALL": "C.UTF-8", "SHELL": "/bin/sh",
                "TCHAT_SESSION_TS": "20261028T080000", "TCHAT_SANDBOX": "1"})
    home = os.path.join(user, "home")
    os.makedirs(home, exist_ok=True)
    env.update({"HOME": home, "XDG_CONFIG_HOME": home, "APPDATA": home, "CODEX_HOME": home})
    return subprocess.run([BIN], cwd=work, env=env, input="部署全链路验证开始\n",
                          capture_output=True, text=True, timeout=180, pass_fds=pass_fds)


def tool_result(req_body, cid):
    data = json.loads(req_body)
    for m in data["messages"]:
        if m.get("tool_call_id") == cid:
            return json.loads(m["content"])
    raise AssertionError(f"缺少工具结果 {cid}")


def session_a():
    cfg = tempfile.mkdtemp(prefix="033dep-cfg-")
    user = tempfile.mkdtemp(prefix="033dep-user-")
    work = os.path.join(user, "work")
    os.makedirs(work)
    marker = "deploy-033-secret"
    with open(os.path.join(work, ".env"), "w") as f:
        f.write(f"TCHAT_DEPLOY_FAKE={marker}\n")
    assert marker in open(os.path.join(work, ".env")).read(), "前置：假凭据沙箱外可读"

    # B2 夹具：launcher（驱动进程）持有无 CLOEXEC 的假凭据 fd，跨 exec 传入 CLI。
    fd_path = os.path.join(work, "fd-secret.txt")
    with open(fd_path, "w") as f:
        f.write("FD-CHAIN-FAKE-033")
    fd = os.open(fd_path, os.O_RDONLY)
    fcntl.fcntl(fd, fcntl.F_SETFD, 0)  # 清除 CLOEXEC
    main_ns = os.readlink("/proc/self/ns/pid")

    # MCP 面：fx 服务器（bwrap 内启动；记录启动/遮蔽/pid）
    rec = os.path.join(user, "srv-rec")
    wrapper = (f"echo started >> '{rec}'; (cat .env) > '{rec}.env' 2>&1; "
               f"echo \"mypid=$$\" >> '{rec}'; exec sleep 60")
    with open(os.path.join(user, "mcp.json"), "w") as f:
        json.dump({"mcp_servers": {"fx": {"enabled": True, "command": "sh",
                                          "args": ["-c", wrapper], "cwd": work}}}, f)

    calls = [
        tool_call("d-exec1", "exec_command", {
            "cmd": ('echo "PID=$$"; echo "NS=$(readlink /proc/self/ns/pid)"; '
                    'echo "HOME=$HOME"; '
                    'echo "ENVERR=$( (cat .env) 2>&1 | head -c 60 )"; '
                    f'echo "FD=$(cat /proc/self/fd/{fd} 2>&1 | head -c 40)"'),
            "workdir": work, "timeout_ms": 15000, "yield_time_ms": 15000}),
        tool_call("d-exec2", "exec_command", {
            "cmd": "/nonexistent-033-deploy-probe", "workdir": work,
            "timeout_ms": 5000, "yield_time_ms": 5000}),
        tool_call("d-mcp", "mcp__fx__echo", {"text": "deploy-033"}),
        tool_call("d-patch", "apply_patch", {"patch":
                    "*** Begin Patch\n*** Add File: deploy-033-patched.txt\n+033-deploy-patch\n*** End Patch\n"}),
    ]
    mock = Mock([round_msg(calls), done_msg("DEPLOY-033-DONE-A")])
    write_config(cfg, user, mock.url)
    try:
        r = run_cli(work, cfg, user, pass_fds=(fd,))
        out = r.stdout + r.stderr
        check("A: CLI 退出码 0", r.returncode == 0, f"rc={r.returncode} tail={out[-300:]}")
        check("A: 全链路完成标记", "DEPLOY-033-DONE-A" in out)
        check("A: 恰好两轮请求", len(mock.requests) == 2, len(mock.requests))
        check("A: 假密钥生效于 mock 端点", all(KEY in (a or "") for a, _ in mock.requests))
        check("A: 沙箱默认启用（载荷 PID=1）",
              "PID=1" in tool_result(mock.requests[1][1], "d-exec1").get("output", ""))
        exec1 = tool_result(mock.requests[1][1], "d-exec1")
        check("A: 新 PID ns（与驱动进程不同）", main_ns not in exec1.get("output", ""), exec1.get("output", "")[:120])
        check("A: deny 遮蔽生效（.env 读取 EACCES）", "Permission denied" in exec1.get("output", ""), exec1.get("output", "")[:200])
        check("A/B2: launcher 遗留 fd 不进入载荷（No such file）",
              "No such file" in exec1.get("output", "") and "FD-CHAIN-FAKE-033" not in exec1.get("output", ""),
              [l for l in exec1.get("output", "").splitlines() if l.startswith("FD=")])
        r2 = tool_result(mock.requests[1][1], "d-exec2")
        check("A: 命令不存在如实报告（shell 127，无误报成功/挂起）",
              r2.get("status") == "completed" and r2.get("exit_code") == 127
              and "not found" in str(r2.get("output")), r2)
        mc = tool_result(mock.requests[1][1], "d-mcp")
        # 当前产品用户轮四工具（030 未实现），MCP 工具不可被模型按名调用＝预期错误；
        # MCP 面运行证据＝服务器经生产 transport 于 bwrap 内启动（下方 rec 断言）。
        check("A: MCP 工具按名调用被明确拒绝（用户轮四工具边界）",
              "未知工具 mcp__fx__echo" in str(mc) and "exec_command" in str(mc), mc)
        pt = tool_result(mock.requests[1][1], "d-patch")
        check("A: apply_patch 沙箱内落盘", pt.get("ok") is True, pt)
        check("A: apply_patch 文件真实产生",
              os.path.exists(os.path.join(work, "deploy-033-patched.txt")))
        rec_txt = open(rec).read() if os.path.exists(rec) else ""
        check("A: MCP 服务器经沙箱启动（mypid=1）", "mypid=1" in rec_txt, rec_txt[:80])
        env_txt = open(rec + ".env").read() if os.path.exists(rec + ".env") else ""
        check("A: MCP 面 .env 遮蔽（服务器读被拒）",
              "Permission denied" in env_txt and marker not in env_txt, env_txt[:100])
        check("A: 外部假凭据字节不变", marker in open(os.path.join(work, ".env")).read())
    finally:
        mock.shutdown()
        os.close(fd)
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)


def session_b():
    cfg = tempfile.mkdtemp(prefix="033dep-b1cfg-")
    user = tempfile.mkdtemp(prefix="033dep-b1user-")
    work = os.path.join(user, "work")
    os.makedirs(work)
    env_file = os.path.join(work, ".env")
    with open(env_file, "w") as f:
        f.write("TCHAT_DEPLOY_FAKE=b1-secret\n")
    os.link(env_file, os.path.join(work, "env-alias"))  # 预建硬链接
    marker2 = os.path.join(work, "b1-marker")
    calls = [tool_call("b1-exec", "exec_command", {
        "cmd": f"cat .env; echo ran > {marker2}", "workdir": work,
        "timeout_ms": 10000, "yield_time_ms": 10000})]
    mock = Mock([round_msg(calls), done_msg("DEPLOY-033-DONE-B")])
    write_config(cfg, user, mock.url)
    try:
        r = run_cli(work, cfg, user)
        out = r.stdout + r.stderr
        check("B1: CLI 退出码 0", r.returncode == 0, f"rc={r.returncode} tail={out[-200:]}")
        check("B1: 全链路完成标记", "DEPLOY-033-DONE-B" in out)
        res = tool_result(mock.requests[1][1], "b1-exec")
        check("B1: 硬链接 deny 目标＝启动前显式拒绝（不回退普通模式）",
              res.get("status") == "error" and "硬链接" in str(res.get("error")), res)
        check("B1: 无执行副作用（标记未出现）", not os.path.exists(marker2))
        check("B1: 假凭据未被改动", "b1-secret" in open(env_file).read())
        check("B1: 别名仍在（夹具未被动用）", os.path.exists(os.path.join(work, "env-alias")))
    finally:
        mock.shutdown()
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)


def main():
    sha = subprocess.run(["sha256sum", BIN], capture_output=True, text=True).stdout.split()[0]
    print(f"verified-binary sha256={sha}")
    check("正式二进制哈希＝部署构建产物", sha == "4ab2dcc16e142de18b2dbb643c3276fdd23534dcb0b316e7b04eaa41a3f1521a", sha)
    session_a()
    session_b()
    failed = [n for n, ok in VERDICT if not ok]
    print(f"\n== 部署验证：{len(VERDICT) - len(failed)} PASS / {len(failed)} FAIL ==")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
