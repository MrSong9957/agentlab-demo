#!/usr/bin/env python3
"""PTY 验收口径包装：在 200x50 伪终端 + TERM=dumb 内串行执行给定命令。

口径依据：2026-10-27 b1b2 tty-investigation——PTY 存在性触发渲染着色路径，
TERM=dumb 抑制着色，与 docker exec -t 验收环境等价（tchat-tui 快照口径）。
清除 TCHAT_SANDBOX＝普通模式全量。转发 master 输出避免 PTY 缓冲阻塞。
"""
import fcntl
import os
import pty
import struct
import subprocess
import sys
import termios


def main() -> None:
    cmd = sys.argv[1:]
    env = dict(os.environ)
    env.pop("TCHAT_SANDBOX", None)
    env["TERM"] = "dumb"
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 200, 0, 0))
    proc = subprocess.Popen(
        cmd, stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True
    )
    os.close(slave)
    try:
        while True:
            try:
                data = os.read(master, 65536)
            except OSError:
                break  # slave 全部关闭＝子进程已退出
            if not data:
                break
            sys.stdout.buffer.write(data)
            sys.stdout.buffer.flush()
    finally:
        os.close(master)
    sys.exit(proc.wait())


if __name__ == "__main__":
    main()
