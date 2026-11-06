---
title: "038 Linux 沙箱启动闸修复计划"
date: 2026-11-06
type: implementation-plan
artifact_contract: ce-unified-plan/v1
product_contract_source: ce-plan-bootstrap
execution: code
---

# 038 Linux 沙箱启动闸修复计划

## Goal Capsule

- **Objective:** Linux 沙箱中的用户命令只能在宿主确认隔离、载荷身份和可回收目标后开始执行。
- **Means:** 复用 bubblewrap 内建 PID 1 回收器，让 gate payload 建立独立 session/进程组后自停，并按父子关系、namespace inode 与 `NSpid` 动态发现；不依赖固定 PID（KTD1–KTD3）。
- **Authority:** 038 §0-A、016 既有进程会话契约、当前验收记录及本计划的 KTD。
- **Execution profile:** 最小路径；串行运行定向检查，先确认 cgroup task 数低于 500。
- **Stop conditions:** 若实现需要改变 §0-A/016 契约、普通模式、Windows 路径、全局 wait owner、容器 PID 1/配置或 workflow，停止并请用户决定。
- **Completion:** 实现者完成本计划的改动、定向验证、简化和代码审查；本计划不授权提交或推送。

## Product Contract

### Summary

修复 Linux bwrap 启动闸：宿主尚未确认前，四个沙箱入口都不得运行用户代码。受控正常结束时，bubblewrap 内建回收器回收载荷；现有宿主等待者仍等待自己直接拥有的 bwrap 子进程。

### Problem Frame

当前命令以 PID 1 身份运行在新 PID namespace。Linux 不会因 namespace init 自己发出的 SIGSTOP 而停止它。当前确认代码未核验载荷处于 T 状态，所以 shell 和 MCP 载荷可能在确认前执行。真实 bwrap 事件顺序和状态见 [启动闸复现记录](../logs/038-github-actions-ci/2026-11-06-sandbox-start-gate.md)。

### Requirements

- R1. 每个沙箱入口都在确认完成前阻止用户命令和输入执行，包括 apply_patch 的 policy=None + TCHAT_SANDBOX 路径。
- R2. 只有宿主核实同一 PIDFD 所指进程的 PID namespace、父进程、启动时间、停止状态 T、独立 PGID/SID 后才放行。缺失、不匹配或身份变化均 fail closed。
- R3. 正常放行对已核验载荷发送 SIGCONT；用户命令、MCP 可执行文件、补丁解析和 JS 源码执行都发生在该事件之后。
- R4. 确认取消、超时、启动错误和身份错误均不放行，按既有有界错误契约清理。能核验载荷时先向其 PIDFD/PGID 发终止信号，D2 sweep 排除 PID 1，并给回收器有界等待机会；尚未核验时沿用现有监督进程清理和错误结果。
- R5. 终止或中断只能命中已核验载荷组；调用者、bwrap 外层组、PID 1 回收器和其他会话哨兵不在目标组。
- R6. 产品仍由现有调用方等待直接拥有的 bwrap 子进程。PID 1 回收器等待载荷及 namespace 内后代。任意监督者外力死亡时不新增“产品必须 wait 所有孤儿”的保证。
- R7. 修改仅限 Linux 沙箱路径。普通模式和 Windows 行为不变；不改容器 PID 1、容器配置、workflow 或 Windows 编译候选。

## Planning Contract

### Key Technical Decisions

- KTD1. **使用 bubblewrap 内建 PID 1 reaper。** 不传 `--as-pid-1` 时，v0.9.0 将 PID 1 留给回收器；具体载荷 PID 不作假设。官方实现 wait 子进程；本机 Ubuntu 0.9.0 包的拓扑探针曾观察到载荷为 PID 2，但产品通过 `NSpid` 最后一项和父子关系动态定位。
- KTD2. **先建立载荷自己的 session/group，再停止。** shell 路径用已有 setsid 命令启动固定 gate shell；apply_patch 与 JS 内部载荷在可信入口调用 setsid(2)。setsid 失败即在读取输入或运行用户代码前退出。必须实测 State T，不能用信号返回码代替。
- KTD3. **共用身份捕获、放行和清理机制。** 从 bwrap 直接子进程中按 namespace 与 `NSpid` 末项 `1` 找回收器，再从其直接子进程中找末项不是 `1` 的唯一 gate payload。用 PID namespace、PPID、start time/PIDFD、State T、PGID、SID 验证。SIGCONT 与已登记 PIDFD 绑定，防止 PID 重用；不依赖 PID 2。
- KTD4. **保留原等待所有权。** exec monitor、MCP StdioProcess、apply_patch 调用者和 JS runtime 仍各自 wait/try_wait 自己的 bwrap Child。受控终止针对载荷组；D2 namespace sweep 若补杀逃逸进程，必须跳过 PID 1。监督者意外死亡继续按现有失败契约处理。

### High-Level Technical Design

~~~mermaid
sequenceDiagram
    participant H as 宿主调用方
    participant B as bwrap 外层监督进程
    participant I as namespace PID 1 回收器
    participant P as 动态发现的 gate/payload
    H->>B: spawn，不传 --as-pid-1
    B->>I: 建立 PID namespace，PID 1
    I->>P: 启动初始应用（PID 数值不固定）
    P->>P: setsid，发送 SIGSTOP
    H->>H: 核验 namespace/父子/starttime/PIDFD/State T/PGID/SID
    H->>P: SIGCONT（核验通过）
    P->>P: exec 用户命令或读取获准输入
    P-->>I: 退出
    I->>I: wait 载荷及剩余子进程
    I-->>B: 无子进程后退出
    B-->>H: bwrap 结束；既有调用方 wait
~~~

四个入口共用同一拓扑和确认契约，但保留最薄入口适配：exec 与 MCP 使用固定 shell gate 后 exec 原命令；apply_patch 与 JS 在读取 stdin/init 前调用内部 gate。JS 仍在确认成功后才写 init 帧。MCP 参数继续按原 argv 传递。

### Scope Boundaries

- **包括：** Linux bwrap 启动参数、gate、载荷身份/进程组验证、受控取消和终止目标、四入口回归与 §0-A 记录。
- **不包括：** 全局 subreaper、额外 supervisor/helper 二进制、容器 PID 1/配置、普通模式、Windows 实现、候选 B、无关工作区差异、workflow 修改、CI 触发、提交或推送。
- `/usr/bin/setsid` 已在本机完整、受限沙箱挂载视图中核实可用；Ubuntu `ubuntu-latest` 的 24.04/26.04 镜像均由 util-linux 提供该命令。目标 CI 镜像无需新增部署依赖或 workflow 修改。具体探针记录见启动闸验收日志。

### System-Wide Impact

- exec Session 保留原 monitor wait owner；只把 Linux 沙箱信号目标记录为载荷 PGID，并让 PID 1 回收器活到载荷退出。确认轮询期间释放管理器状态锁；启动专用锁串行化容量检查、确认和登记，close 标记关闭后等待在途启动清理完成。
- MCP 在确认时保留 namespace 和载荷身份；031 wrapper 虽以 setsid 启动 gate，挂载仍使用原服务器可执行文件。close 超时后先终止 payload group、执行跳过 PID 1 的 D2 sweep，再等待自己的 bwrap Child；超时后保留监督进程组后备和有界 wait。Drop/监督者死亡仍按既有兜底和失败契约。
- apply_patch 只在确认后写 patch stdin；调用者继续 wait_with_output。
- JS worker 从 namespace PID 1 调整为回收器的直接载荷子进程。保留 PIDFD、入口名和 argv 核验，且仅在放行后发送 init 帧；不依赖具体 PID 数值。
- D2 sweep 不得杀内建 reaper；受控路径结束后由其 wait 子进程。任何 wait owner 都不移交给全局进程或容器。

### Risks and Dependencies

- 当前 bubblewrap 为 Ubuntu 0.9.0-1ubuntu0.3。上游 v0.9.0 资料与本机探针相符；发行版补丁未逐项审阅。
- setsid 在本机完整与受限沙箱视图可用；Ubuntu `ubuntu-latest` 24.04/26.04 镜像含 util-linux。无需增加部署依赖或修改 workflow。
- 当前容器以 network-disabled 031 策略启动时，bwrap 因 `NETLINK_ROUTE socket: Operation not permitted` 无法完成网络隔离；该测试在本机受环境阻断。network-enabled 031 启动/回收变体通过。
- 现有 Linux session 终止逻辑和 D2 sweep 已按已核验载荷 PGID/SID 或 namespace 定向；PID 1 回收器被显式排除，并在 bwrap 仍存活时获得 wait 子进程的机会。

## Implementation Units

### U1. 建立可核验的 PID namespace 启动闸

- **Goal:** bwrap 使用内建 PID 1 reaper；载荷在用户代码前建立独立 session 并达到 T 状态。
- **Requirements:** R1–R4。
- **Files:** src/app/src/sandbox.rs、src/app/src/sandbox/tests.rs、src/app/src/proc_session.rs 的 gate 单测。复用现有测试文件，不新增独立测试文件。
- **Approach:** 两个 argv builder 都去掉 `--as-pid-1`；共享确认逻辑从 bwrap 子树动态找到 namespace PID 1 回收器及其唯一直接载荷子进程。捕获 PIDFD、start time、namespace 和父进程身份；核验 T 与 PGID=SID=payload PID；SIGCONT 前复核身份和取消标志。未停止、重用、身份不匹配或已取消均不返回 Released。确认前 namespace 读取错误也进入清理。
- **Test scenarios:** 真实 bwrap 下未确认时不产生用户 marker；按 `/proc` 观察并核对 namespace、父子、State T、PGID/SID；确认后才出现 marker；setsid 或 bwrap 设置失败时没有 marker；取消在确认回调期间到达时仍不得放行；PIDFD 信号失败不返回 Released。
- **Verification:** focused real-bwrap 测试通过事件同步和硬 deadline；直接检查 /proc 状态，不用固定 sleep 或 SIGSTOP 返回码判停止。运行前通过 500 task 门禁。

### U2. 接通四入口并保留回收责任

- **Goal:** 四入口使用同一 gate 身份/放行/清理机制，结束行为与现有契约一致。
- **Requirements:** R1–R7。
- **Files:** src/app/src/proc_session.rs、src/app/src/mcp/transport.rs、src/app/src/apply_patch.rs、src/app/src/js_orchestration/runtime.rs、src/app/src/js_orchestration/worker.rs；对应测试 src/app/tests/proc_session.rs、src/app/tests/exec_sandbox.rs、src/app/tests/mcp_stdio_sandbox.rs、src/app/tests/apply_patch_sandbox.rs、src/app/tests/js_orchestration.rs；更新 docs/specs/038-github-actions-ci.md 和 038 日志。
- **Approach:** exec/MCP shell gate 在 STOP 前建立 payload session；内部 apply_patch/JS gate 调用 setsid 后 STOP。apply_patch 必须在 policy Some 和 033 sandbox_requested 两条路径都设置 gate，并在写 stdin 前确认。确认前不发布 exec Session、不写 patch stdin、不发送 JS init 帧。确认失败时，已捕获载荷由 PIDFD/PGID 定向终止，reaper wait 后既有调用者 wait bwrap；超时/身份未建立时保留原有有界错误和监督进程清理。D2 sweep 排除 PID 1。exec 确认期间不持 manager 状态锁，start/close 用独立启动锁协调。031 MCP 仍挂载原服务器程序。自然退出导致 Ctrl+C 返回 ESRCH/NotFound 时继续收集原退出结果。Windows 路径不改变。
- **Test scenarios:** 四入口分别验证 marker/input/init 只在确认后出现；apply_patch 覆盖 031 policy 与 033 policy=None 两条沙箱路径；取消和确认超时不放行且等待关系正确；正常放行后结果保持；exec SIGINT 只发载荷组、terminate/timeout 结束载荷并 wait bwrap；MCP close 有界并保留监督进程后备；独立 session sentinel 存活；监督者放行前死亡返回既有错误且不误报 payload 已 wait。
- **Verification:** 定向运行 `gate_` 单测、gate03、apply_patch 三项 gate 用例、MCP stdio 两项用例、`sandbox_worker_` worker 回收用例、C13、普通 Ctrl+C/terminate、CPU/墙钟 worker 限制及 manager close/reset 用例。逐项串行并先核对 pids.current < 500；用 PID/starttime/namespace/PGID/SID 和 marker 顺序核验。最终命令和输出以启动闸验收记录为准。
- **Execution note:** 保留所有现有工作区差异，包括 Windows 编译候选及已有回归；逐文件审阅 diff，不用整文件暂存。B 不合并。

## Verification Contract

- 从 /workspace 运行；当前共享 target=/workspace/target。只运行 Linux 定向检查。
- 每项重进程测试单独串行运行，启动前读取 `/sys/fs/cgroup/pids.current`；值达到 500 或无法读取时不启动该项。
- 最小验证：focused real-bwrap gate test；U2 所列四入口用例；cargo check --locked --offline。
- 运行时设硬超时，并保留 stdout/stderr、退出码、PID、start time、NSpid、状态、PGID/SID 和事件顺序。以 marker/barrier 和 State T 判定用户代码的先后。
- 最小路径仍执行 ce-simplify-code、ce-code-review（按 rules/REVIEW.md）；不运行全量、L3、Windows CI，不推送或触发 CI。

## Definition of Done

- bwrap namespace PID 1 是内建 reaper；gate payload 按实际 `/proc` 父子关系与 `NSpid` 动态发现，并拥有独立进程组，不对载荷 PID 数值作假设。
- 四个入口确认完成前都没有可观察的用户代码/输入处理；成功后只放行已验证 PIDFD。
- 取消、超时、启动异常、状态错误及隔离哨兵满足 R4–R6，产品 wait owner 未改变。
- 普通模式和 Windows 分支没有运行路径变化；Windows 编译候选及其他工作区差异仍完整保留。
- 定向验证、代码简化和审查完成；038 §0-A 记录更新为实现候选对应的实际结果。038 整体状态不因本项单独变为完成。
- 诊断/试验代码未混入生产文件；测试后不遗留沙箱进程。

## Implementation and Verification (2026-11-06)

- Implemented the shared Linux gate and connected exec, MCP, apply_patch, and JS worker. Release rechecks the same PIDFD-backed identity immediately before SIGCONT. PID namespace members are signaled through PIDFDs after namespace, start time, PGID and SID checks; namespace D2 sweep skips PID 1 and runs twice before the existing owner waits for bwrap.
- Real bwrap gate observation: payload host PID 20, reaper host PID 19, payload `NSpid` ended in 2, state T, PGID=SID=20, namespace `pid:[4026532464]`; the marker appeared only after the host release callback. This observed PID 2 is evidence for that run, not an implementation assumption.
- Passed targeted confirmation, release, cancellation, timeout, startup-error, MCP, apply_patch policy=None, JS init-frame, exec SIGINT/terminate, worker group reaping, C13, CPU-limit and wall-clock-limit checks. Exact commands and outputs are in [`sandbox-start-gate.md`](../logs/038-github-actions-ci/2026-11-06-sandbox-start-gate.md).
- The existing 031 no-network D2 test is blocked in this container before payload creation: bwrap stderr is `loopback: Failed to create NETLINK_ROUTE socket: Operation not permitted`. A temporary network-enabled variation of the same 031 D2 test passed (1/1); the repository test source was restored to its original network-disabled contract. Windows compile and G2 remain unverified; 038 as a whole remains incomplete.

### Final Review Fixes and Verification (2026-11-06)

- A final check after the validation callback reads cancellation again immediately before `pidfd_send_signal(SIGCONT)`. A real-bwrap regression sets cancellation inside the callback and confirms the user marker remains absent.
- The manager state lock is released while the common gate polls `/proc`. A separate start lock preserves serialized capacity/start registration. `close` marks the manager closed and waits for in-flight startup; if confirmation already released the payload, the close path first signals the recorded payload group, sweeps that namespace while skipping PID 1, and only then gives the reaper a bounded wait window before supervisor fallback.
- Failure to read `/proc/self/ns/pid` now enters the same abort cleanup path. An independent review also found and resolved: the 031 MCP gate must mount the original server executable, MCP sandbox close must have bounded post-signal waits, and Ctrl+C after natural exit must continue collecting the natural result for both ordinary and sandboxed sessions.
- Final checks passed: `cargo check --locked --offline`; test compilation for lib, proc_session, mcp_stdio_sandbox and sandbox_matrix_probes; seven `gate_` unit tests; the exited-payload Ctrl+C unit regression; manager close/reject regression; pipe Ctrl+C/terminate; and the exec real-bwrap release/terminate test. The broader targeted runs before the final lock-order adjustment also passed: apply_patch gate tests (3/3), MCP stdio tests (2/2), JS worker tests (13/13), C13 (1/1), CPU limit (1/1) and 120-second wall-clock limit (1/1). See the final section of the linked verification log for timing and identity evidence.
- `cargo fmt --all -- --check` could not run because the active stable toolchain has no `cargo-fmt` component. No toolchain component was installed. Windows compile, Windows G2, macOS, full suite and L3 remain unverified by this candidate.

## Appendix

### Source and Research

- 038 §0-A：docs/specs/038-github-actions-ci.md。
- 基线和 HEAD：b98d52a682a0ba84080d0043c7bc88f697460473；既有工作区差异见本轮状态。
- bubblewrap v0.9.0：[bubblewrap.c namespace fork/init path](https://github.com/containers/bubblewrap/blob/v0.9.0/bubblewrap.c#L3072-L3161)、[bubblewrap.c init wait loop](https://github.com/containers/bubblewrap/blob/v0.9.0/bubblewrap.c#L3150-L3260)。
- bubblewrap 手册：[PID namespace 默认 init](https://github.com/containers/bubblewrap/blob/v0.9.0/bwrap.xml#L1368-L1378)、[--as-pid-1](https://github.com/containers/bubblewrap/blob/v0.9.0/bwrap.xml#L2184-L2190)。
- Linux man-pages：[pid_namespaces(7)](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html)、[wait(2)](https://man7.org/linux/man-pages/man2/waitpid.2.html)、[setsid(2)](https://man7.org/linux/man-pages/man2/setsid.2.html)、[signal(7)](https://man7.org/linux/man-pages/man7/signal.7.html)。
- 调研汇总和原始输出：docs/think-tank/2026-11-06-038-sandbox-start-gate.md。
