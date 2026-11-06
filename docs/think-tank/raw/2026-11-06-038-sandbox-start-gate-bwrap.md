→ [返回调研汇总](../2026-11-06-038-sandbox-start-gate.md)

## 身份

bubblewrap 0.9.0 上游实现研究。

## 事实

上游 v0.9.0 的 bwrap.xml 说明新 PID namespace 默认有最小 PID 1 reaper，负责收尸；初始应用为 PID 2；无子进程后 PID 1 退出。--as-pid-1 明确跳过该 reaper。源码在新 namespace 中处理 --new-session 后，根据 opt_as_pid_1 决定是否 fork；父支路进入 do_init 的 wait loop，子支路 execvp 初始命令。--die-with-parent 也施加于 init 和初始 command。

本机版本为 Ubuntu bubblewrap 0.9.0-1ubuntu0.3。真实探针观察到 host PID 6/NSpid 6:1 的 reaper，及 host PID 7/NSpid 7:2 的 payload；具体进程态与事件见 ../../logs/038-github-actions-ci/2026-11-06-sandbox-start-gate.md。

## 方案与前提

复用已有 reaper，而非自建。因 --new-session 在 fork 前调用，PID 2 默认继承 PID 1 的 PGID/SID；若宿主必须按载荷组发信号，受闸程序需在用户代码前建立独立 session/group。setsid(1) 探针无 fork，并在 PID 2 原位 exec shell。

依据：[bubblewrap.c fork/init 路径](https://github.com/containers/bubblewrap/blob/v0.9.0/bubblewrap.c#L3072-L3161)、[bubblewrap.c wait loop](https://github.com/containers/bubblewrap/blob/v0.9.0/bubblewrap.c#L3150-L3260)、[手册默认 PID 1 说明](https://github.com/containers/bubblewrap/blob/v0.9.0/bwrap.xml#L1368-L1378)、[--as-pid-1 说明](https://github.com/containers/bubblewrap/blob/v0.9.0/bwrap.xml#L2184-L2190)。
