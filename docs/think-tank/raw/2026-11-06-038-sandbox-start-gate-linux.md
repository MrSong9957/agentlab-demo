→ [返回调研汇总](../2026-11-06-038-sandbox-start-gate.md)

## 身份

Linux man-pages 进程、信号和 wait 语义研究。

## 事实与结论

PID namespace 的首个进程是 PID 1；其退出会触发内核向 namespace 其余进程发 SIGKILL。namespace init 对来自同 namespace 的信号有 handler 限制，但祖先 namespace 发出的 SIGKILL/SIGSTOP 强制递送。故 PID 1 内 kill(SIGSTOP, self) 的返回值不能证明其停止，状态应核验为 T。

waitpid/waitid 等待调用者的子进程；PIDFD 可稳定指向一个进程，但不转移 wait ownership。setsid 让非进程组组长创建新 session，且 PID=PGID=SID；execve 保留 PID/session。负 PGID 信号针对该进程组，成功只表示至少一个信号送达。

推断：将 bwrap 内建 PID 1 reaper 留在原位，让独立 PID 2 gate payload 真正停住。宿主捕获 PIDFD、核验 State T 和 namespace/组身份，放行后仍只 wait 自己拥有的 bwrap child；PID 1 wait payload。

来源：[pid_namespaces(7)](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html)、[wait(2)](https://man7.org/linux/man-pages/man2/waitpid.2.html)、[setsid(2)](https://man7.org/linux/man-pages/man2/setsid.2.html)、[signal(7)](https://man7.org/linux/man-pages/man7/signal.7.html)。
