#!/usr/bin/env python3
"""033 生命周期修复部署后验证：直接驱动正式路径二进制（本地 mock 全链路，假凭据）。

复刻回归 sandbox_lifecycle.rs 四场景（TUI/TERM=xterm 三场＋行模式 MCP 一场），
统一部署态口径 TCHAT_SANDBOX=1（与 deploy-verify-driver.py 相同的注入方式）：
  C（管道跨轮）：yield 长跑 → 轮结束写盘继续 → write_stdin 查询 running+tick →
    启动横幅提及 bubblewrap → /exit 清理。
  D（PTY 跨轮输入）：tty:true read 循环 → 轮结束后 write_stdin chars → GOT:hello 落盘。
  E（MCP 跨轮＋reload 回收）：唯一 record 路径 → 外层 argv 扫描证跨轮存活 →
    停用 reload → 5s 内外层回收（僵尸 cmdline 为空天然排除）。
  F（SIGKILL 载荷停止）：先证跨轮存活 → SIGKILL 自建 CLI → 活/僵尸/消失三分类，
    沙箱态断言零活进程。
三工具面、B1/B2、stdio、错误回传、fail-closed 由 deploy-verify-driver.py 会话 A/B
覆盖（同一正式路径二进制），本脚本不重复。
"""
import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN = "/workspace/target/debug/terminal-chat"
EXPECT_SHA = "713e4c28a431f40134e1f0bd9d9170e14266046176d4501a739f7e34eff41cd6"
FIXTURE = "/workspace/src/app/tests/fixtures/mcp_stdio_server.py"
ANSI = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[()][0B]|\x1b[=>]")
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
            self.requests.append(body)
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


def round_tools(calls):
    return json.dumps({"choices": [{"message": {"role": "assistant", "content": None,
                    "tool_calls": calls}, "finish_reason": "tool_calls"}]})


def round_done(marker):
    return json.dumps({"choices": [{"message": {"content": marker, "finish_reason": "stop"}}]})


def scan_procs(*frags):
    out = []
    for e in os.listdir("/proc"):
        if not e.isdigit():
            continue
        try:
            cl = open(f"/proc/{e}/cmdline", "rb").read().decode("utf-8", "replace")
        except OSError:
            continue
        if cl and all(f in cl for f in frags):
            out.append(int(e))
    return sorted(out)


def proc_state(pid):
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith("State:"):
                return line.strip()
    except OSError:
        pass
    return None


class Pty:
    def __init__(self, bin_path, env, cwd, rows=50, cols=200):
        import pty as pty_mod
        import termios
        master, slave = pty_mod.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

        def prep():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        fcntl.fcntl(master, fcntl.F_SETFL, os.O_NONBLOCK)
        self.master = master
        self.buf = bytearray()
        self.stop = threading.Event()
        self.p = subprocess.Popen([bin_path], cwd=cwd, stdin=slave, stdout=slave,
                                  stderr=slave, env=env, close_fds=True,
                                  preexec_fn=prep)
        os.close(slave)
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        while not self.stop.is_set():
            try:
                d = os.read(self.master, 65536)
            except (BlockingIOError, InterruptedError):
                time.sleep(0.01)
                continue
            except OSError:
                break
            if not d:
                break
            self.buf.extend(d)
        try:
            os.close(self.master)
        except OSError:
            pass

    def text(self):
        s = self.buf.decode("utf-8", "replace")
        s = ANSI.sub(b"", s.encode("utf-8", "replace")).decode("utf-8", "replace")
        return re.sub(r"\s+", "", s)

    def wait_text(self, pat, timeout):
        pat = re.sub(r"\s+", "", pat)
        dl = time.time() + timeout
        while time.time() < dl:
            if pat in self.text():
                return True
            time.sleep(0.03)
        return False

    def send_line(self, text: str):
        # 同批「文本+CR」会被 TUI 吞提交（实测）：分开发送。
        self.send(text.encode())
        time.sleep(0.3)
        self.send(b"\r")

    def send(self, data: bytes):
        os.write(self.master, data)

    def finish(self, timeout=30):
        dl = time.time() + timeout
        while time.time() < dl and self.p.poll() is None:
            time.sleep(0.05)
        if self.p.poll() is None:
            self.p.kill()
        self.stop.set()
        time.sleep(0.1)
        return self.p.returncode


def cli_env(key_name, key, cfg, user, term="xterm"):
    env = dict(os.environ)
    env.pop("TCHAT_PUBLIC_CATALOG_URL", None)
    env.update({"TERM": term, "TCHAT_CONFIG_DIR": cfg, "TCHAT_USER_DIR": user,
                key_name: key, "TZ": "UTC", "LC_ALL": "C.UTF-8", "SHELL": "/bin/sh",
                "TCHAT_SESSION_TS": "20261028T120000", "TCHAT_SANDBOX": "1"})
    home = os.path.join(user, "home")
    os.makedirs(home, exist_ok=True)
    env.update({"HOME": home, "XDG_CONFIG_HOME": home, "APPDATA": home, "CODEX_HOME": home})
    return env


def write_config(cfg, user, url, key, key_env):
    with open(os.path.join(cfg, "manual.json"), "w") as f:
        json.dump({"selected_provider": "deepseek", "selected_model": "deepseek-flash",
                   "providers": {"deepseek": {"base_url": url, "key_ref": f"env:{key_env}"}}}, f)
    fp = hashlib.sha256(key.encode()).hexdigest()[:16]
    port = url.rsplit(":", 1)[1]
    with open(os.path.join(user, "authorized-origins.json"), "w") as f:
        json.dump({"grants": [{"origin": f"http://127.0.0.1:{port}",
                               "cred_fp": fp, "added_at": 0}]}, f)


def tool_result(req_body, cid):
    data = json.loads(req_body)
    for m in data["messages"]:
        if m.get("tool_call_id") == cid:
            return json.loads(m["content"])
    raise AssertionError(f"缺少工具结果 {cid}")


def fsize(p):
    return os.path.getsize(p) if os.path.exists(p) else 0


def wait_growth(path, base, sec):
    dl = time.time() + sec
    while time.time() < dl:
        if fsize(path) > base:
            return True
        time.sleep(0.02)
    return False


def scenario_c():
    """管道跨轮：横幅＋yield 长跑跨轮写盘＋write_stdin 管理＋退出清理。"""
    key = "lcd-c-key"
    cfg = tempfile.mkdtemp(prefix="lcdC-cfg-")
    user = tempfile.mkdtemp(prefix="lcdC-user-")
    work = os.path.join(user, "work")
    os.makedirs(work)
    log = os.path.join(work, "w.log")
    marker = work  # 扫描标记＝work 路径（cmdline 必含日志路径；每场景唯一目录）
    mock = Mock([round_tools([tool_call("lc-w", "exec_command", {
                    "cmd": f"while true; do echo tick; echo x >> {log}; sleep 0.05; done",
                    "workdir": work, "timeout_ms": 30000, "yield_time_ms": 1500})]),
                 round_tools([tool_call("lc-q", "write_stdin",
                    {"session_id": "s1", "yield_time_ms": 5000})]),
                 round_done("LCD-C-DONE")])
    write_config(cfg, user, mock.url, key, "LCD_C_KEY")
    pty = None
    try:
        pty = Pty(BIN, cli_env("LCD_C_KEY", key, cfg, user), work)
        check("C: 启动横幅提及 bubblewrap（部署态沙箱可用）", pty.wait_text("bubblewrap", 20))
        pty.send_line("go")
        check("C: 命令上屏", pty.wait_text("while true; do echo tick", 15))
        check("C: 写盘探针已运行", wait_growth(log, 0, 5))
        check("C: 回合收尾（LC-DONE 上屏＝轮线程已退出）", pty.wait_text("LCD-C-DONE", 30))
        s1 = fsize(log)
        check("C: 轮结束后写盘继续（016 跨轮保留）", wait_growth(log, s1, 1.2),
              f"s1={s1}")
        check("C: 第二轮为 write_stdin 查询（请求数=3）", len(mock.requests) == 3,
              len(mock.requests))
        q = tool_result(mock.requests[2], "lc-q")
        check("C: 查询仍运行会话＝running", q.get("status") == "running", q)
        check("C: 查询取到跨轮新增输出（tick）", "tick" in str(q.get("output", "")), q)
        pty.send_line("/exit")
        check("C: /exit 退出（再见!）", pty.wait_text("再见", 20))
        rc = pty.finish()
        check("C: 退出码 0", rc == 0, rc)
        dl = time.time() + 3
        while time.time() < dl and scan_procs(marker):
            time.sleep(0.05)
        check("C: 退出后写入进程无遗留（活进程口径）", scan_procs(marker) == [],
              scan_procs(marker))
        check("C: 退出后无沙箱包装遗留", scan_procs("bwrap", marker) == [])
    finally:
        if pty:
            pty.stop.set()
            try:
                pty.p.kill()
            except Exception:
                pass
        for p in scan_procs(marker):
            try:
                os.kill(p, signal.SIGKILL)
            except OSError:
                pass
        mock.shutdown()
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)


def scenario_d():
    """PTY 跨轮输入：write_stdin chars 送达保留会话（GOT:hello 落盘）。"""
    key = "lcd-d-key"
    cfg = tempfile.mkdtemp(prefix="lcdD-cfg-")
    user = tempfile.mkdtemp(prefix="lcdD-user-")
    work = os.path.join(user, "work")
    os.makedirs(work)
    out = os.path.join(work, "out.log")
    mock = Mock([round_tools([tool_call("lc-r", "exec_command", {
                    "cmd": f'while IFS= read -r l; do echo "GOT:$l" >> {out}; done',
                    "workdir": work, "timeout_ms": 30000, "yield_time_ms": 1500,
                    "tty": True})]),
                 round_tools([tool_call("lc-i", "write_stdin",
                    {"session_id": "s1", "chars": "hello\n", "yield_time_ms": 800})]),
                 round_done("LCD-D-DONE")])
    write_config(cfg, user, mock.url, key, "LCD_D_KEY")
    pty = None
    try:
        pty = Pty(BIN, cli_env("LCD_D_KEY", key, cfg, user), work)
        check("D: TUI 就绪", pty.wait_text("? for shortcuts", 20))
        pty.send_line("go")
        check("D: PTY 命令上屏", pty.wait_text("whileIFS=read-rl", 15) or
              pty.wait_text("read-r", 15))
        check("D: 回合收尾（yield 会话保留）", pty.wait_text("LCD-D-DONE", 30))
        got = False
        dl = time.time() + 5
        while time.time() < dl:
            if os.path.exists(out) and "GOT:hello" in open(out, errors="replace").read():
                got = True
                break
            time.sleep(0.05)
        ws = tool_result(mock.requests[2], "lc-i")
        check("D: 轮结束后 write_stdin 输入送达（GOT:hello 落盘）", got,
              f"out={open(out, errors='replace').read()[:120] if os.path.exists(out) else '(无)'} ws={ws}")
        pty.send_line("/exit")
        check("D: /exit 退出码 0", pty.wait_text("再见", 20) and pty.finish() == 0)
    finally:
        if pty:
            pty.stop.set()
            try:
                pty.p.kill()
            except Exception:
                pass
        mock.shutdown()
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)


def scenario_e():
    """MCP 跨轮存活＋停用 reload 外层回收（唯一 record 标记＋外层 argv 扫描）。"""
    key = "lcd-e-key"
    cfg = tempfile.mkdtemp(prefix="lcdE-cfg-")
    user = tempfile.mkdtemp(prefix="lcdE-user-")
    work = os.path.join(user, "work")
    os.makedirs(work)
    rec_dir = tempfile.mkdtemp(prefix="lcdE-rec-")  # 每场景唯一 record 路径＝扫描标记
    rec = os.path.join(rec_dir, "record.jsonl")

    def write_mcp(enabled):
        with open(os.path.join(user, "mcp.json"), "w") as f:
            json.dump({"mcp_servers": {"fx": {"enabled": enabled, "command": "python3",
                       "args": [FIXTURE, "--scenario", "echo", "--record", rec],
                       "cwd": tempfile.gettempdir()}}}, f)

    write_mcp(True)
    mock = Mock([round_done("LCD-E-一轮"), round_done("LCD-E-二轮")])
    write_config(cfg, user, mock.url, key, "LCD_E_KEY")
    env = cli_env("LCD_E_KEY", key, cfg, user, term="dumb")  # 行模式驱动 /mcp reload
    pty = None
    try:
        pty = Pty(BIN, env, work)
        check("E: 行模式就绪「你> 」", pty.wait_text("你>", 20))
        pty.send_line("连接一次")
        check("E: 首轮回复", pty.wait_text("LCD-E-一轮", 25))
        dl = time.time() + 10
        while time.time() < dl and not scan_procs(FIXTURE):
            time.sleep(0.05)
        started = scan_procs(FIXTURE)
        check("E: MCP 服务器实际启动（外层 argv 身份）", bool(started), started)
        time.sleep(1.2)
        after = scan_procs(FIXTURE)
        check("E: 轮结束后 MCP 服务器跨轮存活", bool(after), after)
        pty.send_line("还在吗")
        check("E: 下一轮健康对话（连接复用）", pty.wait_text("LCD-E-二轮", 25))
        write_mcp(False)
        pty.send_line("/mcp reload")
        check("E: reload 完成提示", pty.wait_text("已重新加载配置并重建连接", 15))
        dl = time.time() + 5
        while time.time() < dl and scan_procs(FIXTURE):
            time.sleep(0.05)
        check("E: 停用后旧 MCP 进程外层回收", scan_procs(FIXTURE) == [],
              scan_procs(FIXTURE))
        pty.send(b"\x04")  # 行模式退出＝Ctrl-D（/exit 为 TUI 命令；attr-drive 同款实证配方）
        rc = pty.finish()
        check("E: Ctrl-D 退出码 0", rc == 0, rc)
    finally:
        if pty:
            pty.stop.set()
            try:
                pty.p.kill()
            except Exception:
                pass
        for p in scan_procs(FIXTURE):
            try:
                os.kill(p, signal.SIGKILL)
            except OSError:
                pass
        mock.shutdown()
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)
        shutil.rmtree(rec_dir, ignore_errors=True)


def scenario_f():
    """宿主 SIGKILL 后载荷停止（die-with-parent 经长寿创建线程仍成立）；三分类。"""
    key = "lcd-f-key"
    cfg = tempfile.mkdtemp(prefix="lcdF-cfg-")
    user = tempfile.mkdtemp(prefix="lcdF-user-")
    work = os.path.join(user, "work")
    os.makedirs(work)
    log = os.path.join(work, "w.log")
    marker = work
    mock = Mock([round_tools([tool_call("lc-k", "exec_command", {
                    "cmd": f"while true; do echo x >> {log}; sleep 0.05; done",
                    "workdir": work, "timeout_ms": 30000, "yield_time_ms": 1500})]),
                 round_done("LCD-F-DONE")])
    write_config(cfg, user, mock.url, key, "LCD_F_KEY")
    pty = None
    try:
        pty = Pty(BIN, cli_env("LCD_F_KEY", key, cfg, user), work)
        check("F: TUI 就绪", pty.wait_text("? for shortcuts", 20))
        pty.send_line("go")
        check("F: 命令上屏", pty.wait_text("while true; do echo x", 15))
        check("F: 回合收尾（会话跨轮保留）", pty.wait_text("LCD-F-DONE", 30))
        pre = scan_procs(marker)
        check("F: 先证存活（轮结束后载荷在册）", bool(pre), pre)
        cli_pid = pty.p.pid
        os.kill(cli_pid, signal.SIGKILL)
        dl = time.time() + 3
        live = None
        while time.time() < dl:
            live = scan_procs(marker)
            if not live:
                break
            time.sleep(0.05)
        alive, zombies, gone = [], [], []
        for pid in pre:
            st = proc_state(pid)
            if st is None:
                gone.append(pid)
            elif "Z" in st:
                zombies.append(pid)
            else:
                alive.append(f"{pid}:{st}")
        check("F: SIGKILL 后沙箱载荷停止执行（零活进程）", not alive,
              f"alive={alive} zombies={zombies} gone={gone} live_scan={live}")
        cli_st = proc_state(cli_pid)
        check("F: CLI 自身已死（僵尸或消失，不计泄漏）",
              cli_st is None or "Z" in cli_st, f"CLI {cli_pid}: {cli_st}")
        print(f"  [三分类明细] alive={alive} zombies={zombies}（已死未收割） gone={gone} CLI={cli_st}")
    finally:
        if pty:
            pty.stop.set()
            try:
                pty.p.kill()
            except Exception:
                pass
        for p in scan_procs(marker):
            try:
                os.kill(p, signal.SIGKILL)
            except OSError:
                pass
        mock.shutdown()
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)


def main():
    sha = subprocess.run(["sha256sum", BIN], capture_output=True, text=True).stdout.split()[0]
    print(f"verified-binary sha256={sha}")
    check("正式二进制哈希＝本轮部署构建产物", sha == EXPECT_SHA, sha)
    scenario_c()
    scenario_d()
    scenario_e()
    scenario_f()
    failed = [n for n, ok in VERDICT if not ok]
    print(f"\n== 生命周期部署验证：{len(VERDICT) - len(failed)} PASS / {len(failed)} FAIL ==")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
