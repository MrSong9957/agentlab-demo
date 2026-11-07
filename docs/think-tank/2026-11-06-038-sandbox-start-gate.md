# 038 Linux 沙箱启动闸计划前调研

日期：2026-11-06

## 目标

确认一个不改变 §0-A 外部契约的 Linux 沙箱启动拓扑，能在用户代码执行前验证载荷确实停止，并保留各现有 wait owner。

## 专家结论

- bubblewrap：上游 v0.9.0 文档说明 PID namespace 默认由 bubblewrap 内建 PID 1 回收器管理，初始应用为 PID 2；--as-pid-1 禁止创建该 reaper。源码在 --new-session 后 fork reaper，再 exec 初始应用。真实本机 probe 与此一致。见 [raw bwrap 研究](raw/2026-11-06-038-sandbox-start-gate-bwrap.md)。
- Linux 进程语义：namespace PID 1 的自发 SIGSTOP 不足以证明其处于停止态；wait 只允许等待调用者的子进程；setsid 使进程建立独立 PGID/SID。应直接核验 State T、PIDFD 和组身份。见 [raw Linux 研究](raw/2026-11-06-038-sandbox-start-gate-linux.md)。
- 仓库链路：通用确认不检查停止态；MCP 当前包装函数没有 STOP；apply_patch 的 033 sandbox_requested + policy=None 分支跳过确认且 gate=off。031 apply_patch 分支确认后才写 stdin；JS 另有 PIDFD、入口名、宿主 SIGSTOP 和 init 帧屏障。现有宿主 wait owner 可保留。见 [raw 仓库审计](raw/2026-11-06-038-sandbox-start-gate-repo.md)。

## 组合方案

1. 移除两处 argv builder 的 --as-pid-1，让 bubblewrap 内建 reaper 留作 namespace PID 1。
2. 让 gate 成为 PID 2，并在 SIGSTOP 前执行 setsid。exec/MCP 使用固定 shell wrapper；apply_patch/JS 在可信入口调用 setsid(2)。
3. shared confirm 走 bwrap 子树找到 PID 1 及 payload PID 2，捕获 PIDFD 后核验 namespace、PPID、start time、State T、PGID/SID，核验通过才 SIGCONT。
4. 受控终止信号发给 payload group，PID 1 reaper wait 载荷；外层当前调用者继续 wait bwrap。现有 D2 sweep 如补杀逃逸进程，必须跳过 reaper PID 1。监督者意外死亡的失败契约不扩大。
5. 修正 apply_patch 033 沙箱分支：policy=None 但 TCHAT_SANDBOX=1 时同样 gate 并在写补丁 stdin 前确认。

## 冲突与裁决

PID 2 默认继承 PID 1 的 group/session，因此单独移除 --as-pid-1 会使宿主 killpg 命中 reaper 和载荷。上游源码与本机拓扑探针都证明这一点。组合方案增加 setsid，并把确认后的身份检查与 group 目标一并保存。PID 1 的 wait 责任来自 bubblewrap 已有实现，不引入全局 subreaper 或自建 helper。

## 社区校准

侦察兵尝试使用 last30days skill。首次运行门要求用户决定是否允许读取浏览器 Cookie，且本环境没有已配置 backend 凭据。侦察兵未启动外部研究，未使用 WebSearch 冒充社区结果，也未改配置。该缺口不影响官方源码、man-pages 和真实 bwrap probe 支持的拓扑选择。见 [raw 侦察兵记录](raw/2026-11-06-038-sandbox-start-gate-scout.md)。

## 背景知识

- SIGSTOP 成功返回不等价于进程已经停止；确认应读实际状态并绑定 PIDFD。
- bwrap 的 --new-session 在内建 init fork 前执行，所以 PID 2 继承 PID 1 的 group/session，需另建独立 payload 组。
- PIDFD 固定身份但不能代替 wait ownership；应用只能 wait 自己的 child，bwrap 内建 PID 1 则能 wait namespace 子进程。

## 原始输出索引

- [bubblewrap 专家](raw/2026-11-06-038-sandbox-start-gate-bwrap.md)
- [Linux 语义专家](raw/2026-11-06-038-sandbox-start-gate-linux.md)
- [仓库审计专家](raw/2026-11-06-038-sandbox-start-gate-repo.md)
- [侦察兵](raw/2026-11-06-038-sandbox-start-gate-scout.md)
