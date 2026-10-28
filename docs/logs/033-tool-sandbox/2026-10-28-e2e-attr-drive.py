#!/usr/bin/env python3
"""033 部署态 e2e 三失败归因驱动：真实 CLI＋本地 mock，行为级对照。

场景 A（n2b 核心，TUI/TERM=xterm）：exec 写入循环＋批内第二调用 → ESC 取消 →
  取消反馈/写盘是否继续/写入进程与 bwrap 是否存活/下一轮/退出清理。
场景 B（s103 核心，行模式 TERM=dumb）：MCP 服务器启动记 pid → 停用＋/mcp reload →
  旧 pid 是否 5s 内回收/遗留扫描/停用后对话/退出。

矩阵：{新二进制(正式路径), 旧二进制(回滚点, 直接执行不替换)} × {SAND=1, env -u}。
只观察与记录，不改任何配置与断言。
"""
import fcntl
import hashlib
import json
import os
import re
import shutil
import subprocess
import struct
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

NEW_BIN = "/workspace/target/debug/terminal-chat"
OLD_BIN = "/workspace/.devcontainer/033-rollback/terminal-chat.pre-b1b2-20261028"
FIXTURE = "/workspace/src/app/tests/fixtures/mcp_stdio_server.py"
ANSI = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[()][0B]|\x1b[=>]")


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


class _Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        self.server.mock._handle(self)

    def log_message(self, *a):
        pass


def tool_call(cid, name, args):
    return {"id": cid, "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}


class Pty:
    def __init__(self, bin_path, env, cwd, rows=50, cols=200):
        import pty as pty_mod
        import termios
        master, slave = pty_mod.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

        def prep():  # slave 成为子进程控制终端（TUI 的 /dev/tty 打开落到本 pty）
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
            self.buf.extend(ANSI.sub(b"", d))
        try:
            os.close(self.master)
        except OSError:
            pass

    def text(self):
        s = self.buf.decode("utf-8", "replace")
        s = ANSI.sub("", s.encode("utf-8", "replace")).decode("utf-8", "replace")
        return re.sub(r"\s+", "", s)  # TUI 逐字符定位渲染：扁平化后做子串匹配

    def wait_text(self, pat, timeout):
        pat = re.sub(r"\s+", "", pat)
        dl = time.time() + timeout
        while time.time() < dl:
            if pat in self.text():
                return True
            time.sleep(0.03)
        return False

    def send(self, data: bytes):
        os.write(self.master, data)

    def send_line(self, text: str):
        # TUI 输入处理对同批到达的「文本+CR」会吞提交（实测）：分开发送。
        self.send(text.encode())
        time.sleep(0.3)
        self.send(b"\r")

    def finish(self, timeout=30):
        dl = time.time() + timeout
        while time.time() < dl and self.p.poll() is None:
            time.sleep(0.05)
        if self.p.poll() is None:
            self.p.kill()
        self.stop.set()
        time.sleep(0.1)
        return self.p.returncode


def scan_procs(*frags):
    out = []
    for e in os.listdir("/proc"):
        if not e.isdigit():
            continue
        try:
            cl = open(f"/proc/{e}/cmdline", "rb").read().decode("utf-8", "replace")
        except OSError:
            continue
        if all(f in cl for f in frags):
            out.append(int(e))
    return sorted(out)


def cli_env(sandbox, key_name, key, cfg, user):
    env = dict(os.environ)
    env.pop("TCHAT_PUBLIC_CATALOG_URL", None)
    env.pop("TCHAT_SANDBOX", None)
    if sandbox:
        env["TCHAT_SANDBOX"] = "1"
    home = os.path.join(user, "home")
    os.makedirs(home, exist_ok=True)
    env.update({"TERM": "xterm", key_name: key, "TZ": "UTC", "LC_ALL": "C.UTF-8",
                "SHELL": "/bin/sh", "TCHAT_SESSION_TS": "20261028T090000",
                "TCHAT_CONFIG_DIR": cfg, "TCHAT_USER_DIR": user,
                "HOME": home, "XDG_CONFIG_HOME": home, "APPDATA": home,
                "CODEX_HOME": home})
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


def report(tag, items):
    print(f"\n===== {tag} =====")
    for k, v in items:
        print(f"  {k}: {v}")


def fsize(p):
    return os.path.getsize(p) if os.path.exists(p) else 0


def scenario_a(bin_path, sandbox, tag):
    key = "attr-a-key"
    cfg = tempfile.mkdtemp(prefix="attrA-cfg-")
    user = tempfile.mkdtemp(prefix="attrA-user-")
    work = tempfile.mkdtemp(prefix="attrA-work-")
    log = os.path.join(work, "w.log")
    second = os.path.join(work, "second.txt")
    batch = json.dumps({"choices": [{"message": {"role": "assistant", "content": None,
        "tool_calls": [
            tool_call("c-w", "exec_command", {"cmd": f"while true; do echo x >> {log}; sleep 0.05; done", "timeout_ms": 30000}),
            tool_call("c-s", "exec_command", {"cmd": f"touch {second}"}),
        ]}, "finish_reason": "tool_calls"}]})
    mock = Mock([batch, json.dumps({"choices": [{"message": {"content": "ATTR-A-DONE", "finish_reason": "stop"}}]})])
    write_config(cfg, user, mock.url, key, "ATTR_A_KEY")
    items = []
    pty = Pty(bin_path, cli_env(sandbox, "ATTR_A_KEY", key, cfg, user), work)
    try:
        items.append(("TUI 就绪", pty.wait_text("? for shortcuts", 20)))
        pty.send_line("go")
        items.append(("运行态命令上屏", pty.wait_text("while true; do echo x >>", 15)))
        grew = 0
        for _ in range(100):
            if fsize(log) > 0:
                grew = 1
                break
            time.sleep(0.02)
        items.append(("写盘探针已运行", grew == 1))
        pty.send(b"\x1b")
        items.append(("取消反馈「错误：」≤3s", pty.wait_text("错误：", 3)))
        items.append(("「仍由会话管理」呈现", pty.wait_text("仍由会话管理", 3)))
        s1 = fsize(log)
        time.sleep(0.4)
        s2 = fsize(log)
        items.append(("取消后 0.4s 写盘继续", f"s1={s1} s2={s2} => {s2 > s1}"))
        items.append(("取消后写入进程存活", scan_procs("while true; do echo x >>", work)))
        items.append(("取消后 bwrap 存活", scan_procs("bwrap", work)))
        items.append(("批内第二调用未执行", not os.path.exists(second)))
        items.append(("取消后未自动续请求(mock=1)", len(mock.requests) == 1))
        pty.send_line("next")
        items.append(("下一轮回复(CLI 存活)", pty.wait_text("ATTR-A-DONE", 25)))
        items.append(("续请求数=2", len(mock.requests) == 2))
        pty.send_line("/exit")
        items.append(("/exit 退出", pty.wait_text("再见", 20)))
        rc = pty.finish()
        items.append(("退出码 0", rc == 0))
        time.sleep(0.4)
        s3 = fsize(log)
        time.sleep(0.4)
        s4 = fsize(log)
        items.append(("退出后写盘停止", f"s3={s3} s4={s4} => {s4 == s3}"))
        items.append(("退出后写入进程无遗留", scan_procs("while true; do echo x >>", work) == []))
        items.append(("退出后 bwrap 无遗留", scan_procs("bwrap", work) == []))
    finally:
        pty.stop.set()
        try:
            pty.p.kill()
        except Exception:
            pass
        mock.shutdown()
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)
        shutil.rmtree(work, ignore_errors=True)
    report(tag, items)


def scenario_b(bin_path, sandbox, tag):
    key = "attr-b-key"
    cfg = tempfile.mkdtemp(prefix="attrB-cfg-")
    user = tempfile.mkdtemp(prefix="attrB-user-")
    work = os.path.join(user, "work")
    os.makedirs(work)
    rec = os.path.join(user, "record.jsonl")
    with open(os.path.join(user, "mcp.json"), "w") as f:
        json.dump({"mcp_servers": {"fx": {"enabled": True, "command": "python3",
                   "args": [FIXTURE, "--scenario", "echo", "--record", rec],
                   "cwd": tempfile.gettempdir()}}}, f)
    mock = Mock([json.dumps({"choices": [{"message": {"content": "ATTR-B-就绪", "finish_reason": "stop"}}]}),
                 json.dumps({"choices": [{"message": {"content": "ATTR-B-停用后仍可对话", "finish_reason": "stop"}}]})])
    write_config(cfg, user, mock.url, key, "ATTR_B_KEY")
    items = []
    env = cli_env(sandbox, "ATTR_B_KEY", key, cfg, user)
    env["TERM"] = "dumb"
    pty = Pty(bin_path, env, work)
    try:
        items.append(("行模式就绪「你> 」", pty.wait_text("你>", 20)))
        pty.send("连接一次\r".encode())
        items.append(("首轮回复", pty.wait_text("ATTR-B-就绪", 25)))
        items.append(("回到提示符", pty.wait_text("你>", 10)))
        pid = None
        for _ in range(50):
            try:
                for line in open(rec):
                    e = json.loads(line)
                    if e.get("kind") == "startup":
                        pid = e.get("pid")
            except (OSError, json.JSONDecodeError):
                pass
            if pid:
                break
            time.sleep(0.05)
        items.append(("MCP 启动记录 pid", pid))
        items.append(("MCP 子进程存活", bool(pid) and os.path.exists(f"/proc/{pid}")))
        with open(os.path.join(user, "mcp.json"), "w") as f:
            json.dump({"mcp_servers": {"fx": {"enabled": False, "command": "python3",
                       "args": [FIXTURE, "--scenario", "echo", "--record", rec],
                       "cwd": tempfile.gettempdir()}}}, f)
        pty.send_line("/mcp reload")
        items.append(("reload 完成提示", pty.wait_text("已重新加载配置并重建连接", 15)))
        dl = time.time() + 5
        gone = False
        while time.time() < dl:
            if pid and not os.path.exists(f"/proc/{pid}"):
                gone = True
                break
            time.sleep(0.05)
        items.append(("停用后旧 MCP 进程 5s 内回收", gone))
        items.append(("MCP python 遗留扫描", scan_procs("mcp_stdio_server.py")))
        items.append(("停用后 /mcp 显示已停用", pty.wait_text("已停用", 8)))
        pty.send("还在吗\r".encode())
        items.append(("停用后对话照常", pty.wait_text("ATTR-B-停用后仍可对话", 25)))
        pty.send(b"\x04")
        rc = pty.finish()
        items.append(("Ctrl-D 退出成功", rc == 0))
    finally:
        pty.stop.set()
        try:
            pty.p.kill()
        except Exception:
            pass
        mock.shutdown()
        shutil.rmtree(cfg, ignore_errors=True)
        shutil.rmtree(user, ignore_errors=True)
    report(tag, items)


def main():
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    plan = []
    if which in ("all", "a"):
        plan += [("A/新/沙箱", scenario_a, NEW_BIN, True), ("A/新/普通", scenario_a, NEW_BIN, False),
                 ("A/旧/沙箱", scenario_a, OLD_BIN, True), ("A/旧/普通", scenario_a, OLD_BIN, False)]
    if which in ("all", "b"):
        plan += [("B/新/沙箱", scenario_b, NEW_BIN, True), ("B/新/普通", scenario_b, NEW_BIN, False),
                 ("B/旧/沙箱", scenario_b, OLD_BIN, True), ("B/旧/普通", scenario_b, OLD_BIN, False)]
    for tag, fn, binp, sb in plan:
        fn(binp, sb, tag)


if __name__ == "__main__":
    main()
