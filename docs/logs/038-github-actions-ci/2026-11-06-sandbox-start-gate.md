# 038 §0-A 启动闸确定性复现

日期：2026-11-06
项目：/workspace
分支：feat/038-github-actions-ci
基线及 HEAD：b98d52a682a0ba84080d0043c7bc88f697460473
状态：只读诊断；没有修改生产逻辑或测试源文件。

## 环境与边界

- 本机 bubblewrap：0.9.0；发行版包 0.9.0-1ubuntu0.3。
- cgroup task 门禁：两次启动前分别为 238、237；均低于 500。
- 两项探针单独串行运行，每项有 5 秒事件 deadline。探针通过 stdout 和 /proc 事件判定，不用固定睡眠推断停止状态。
- 当前工作区既有差异保持原样。探针脚本与原始输出位于 target/038-validation/038-gate-pid1-repro.py、038-gate-pid1-repro.log、038-gate-reaper-probe.py、038-gate-reaper-probe.log。

## 基线到当前工作树的差异核对

- HEAD 仍为基线 `b98d52a682a0ba84080d0043c7bc88f697460473`；本轮没有改变源代码或测试源文件。现有工作区已有 `sandbox.rs`、`proc_session.rs`、MCP、apply_patch、JS、测试和其他记录差异，均保留。
- 本轮相关的 `sandbox.rs` 差异把确认结果扩展为 namespace inode＋payload PID，供 016 管道 SIGINT 选择目标；`proc_session.rs` 使用该 PID 作为载荷组目标，并按 namespace 限定信号。两处 bwrap argv builder 在基线和当前工作树中仍带 `--as-pid-1`；通用确认在两者中仍只核验直接异 namespace 子进程与父 PID，然后 SIGCONT，不检查 State T、PIDFD 或载荷 PGID/SID。因此这些已有会话中断差异没有修复 §0-A。
- `proc_session.rs` 当前候选在 policy 存在或 sandbox requested 时确认；gate 仍由 shell 自发 SIGSTOP。MCP 当前差异主要将入口限定为 Linux；`mcp_stdio_gate_wrap` 的实际包装仍只 `exec "$0" "$@"`，没有 STOP。apply_patch 当前 Linux gate 标记在 `policy=None` 时设为 `off`，确认仍只在 `policy.is_some()` 执行，所以 033 `TCHAT_SANDBOX=1` 路径仍跳过确认并直接写 stdin。
- JS 当前候选保留 worker 自停、宿主按 PIDFD 补发 SIGSTOP 的核验和 Released 后才写 init 帧的顺序。该路径已有输入屏障，但载荷仍是 namespace PID 1；计划需把它迁移到同一 PID 1 reaper / PID 2 payload 拓扑，保留现有帧屏障。
- 结论：此前工作树差异没有改变本记录的 §0-A 失败证据，也没有消除计划列出的 MCP/apply_patch 缺口。历史验收与新增 G2、038 其他差异均不纳入本次生产修改。

## 当前四入口链路审计

| 入口 | 当前启动与 PID / PGID / SID | 确认点和阻止执行方式 | wait owner |
|---|---|---|---|
| exec | 沙箱 argv 带 --as-pid-1；受闸 shell 自发 SIGSTOP，载荷是 namespace PID 1。确认扫描 bwrap 的直接异 namespace 子进程。本次 real-bwrap 复现 host PID=6、NSpid=6:1，PGID=SID=6（实测）。 | start_with_owner 在发布 Session 前确认；通用路径不核验 T。PID 1 自停不可靠，shell 后续命令可能先执行。 | ProcessManager monitor_thread wait Rust ChildHandle；工作树差异另存 payload PID/namespace 用于中断。 |
| MCP stdio | 沙箱请求时先调用 mcp_stdio_gate_wrap；实际只返回 /bin/sh -c 'exec "$0" "$@"'，然后启动 bwrap。目标 server 是 PID 1 或其原位 exec 后进程。PGID/SID 的入口数值未单独观测；不得借用 exec 复现数值。 | spawn 后才调用通用确认；包装没有 STOP，server 可在确认前启动。 | StdioProcess::close 等 Tokio Child；Drop 和 close 超时按进程组终止。 |
| apply_patch | 031 policy 分支设置 INTERNAL_GATE_ENV=stop、spawn 后 confirm，成功后才写 patch stdin；033 TCHAT_SANDBOX 且 policy=None 分支把 gate 设为 off、跳过 confirm，spawn 后直接写 stdin。载荷 namespace PID 为 1；本轮未单独采集其 PGID/SID。 | 031 分支的内部 PID 1 自停不可靠，但 stdin 屏障仍挡住 patch 数据；033 分支没有启动确认，是已确认的直接缺口。 | 调用方 Child::wait_with_output。 |
| JS worker | bwrap --as-pid-1；worker 标记 tchat-js-ready、自停，然后尝试读 init 帧。载荷 namespace PID 为 1；回调源码要求 PGID=SID=worker PID，但本轮未单独采集其具体进程值。 | runtime callback 捕获 PIDFD，核对入口名/argv；观察到非 T 时由宿主对固定 PIDFD 发 SIGSTOP，再确认 T。spawn_cell 只在 Released 后写 init 帧，因此 JS 源码由帧屏障保护。 | JS runtime 用 PIDFD 观察 worker，并 wait/try_wait 自己的 bwrap Child。 |

共同事实：当前 bwrap namespace 载荷为 PID 1，不是 bwrap 内建 reaper。除 exec 的失败复现外，本轮没有把 MCP、apply_patch、JS 的进程组数值冒充成实测；相同 argv 只说明可能共享机制，未来接线验收仍逐入口读取 /proc 核对。候选拓扑 probe 的 PID 1 reaper PGID=SID=6，setsid 后 PID 2 payload PGID=SID=7。不得把不同入口或不同运行的 PID 当作同一身份。

## 当前启动链复现

调用 /usr/bin/bwrap 时保留当前关键参数：--as-pid-1、--new-session、--die-with-parent、--unshare-user、--unshare-pid、--unshare-ipc、--proc /proc；载荷执行当前 gate 形态：

~~~sh
kill -s STOP $$ 2>/dev/null || kill -s KILL $$;
printf 'USER_CODE_BEFORE_CONFIRM pid=%s\n' "$$";
exec /bin/sleep 30
~~~

诊断 harness 在看见用户 marker 之前没有调用产品确认函数。事件顺序和实际观察：

1. +0.000791s：spawn 返回，host bwrap PID 5；确认被 harness 扣住。
2. +0.009369s：stdout 已出现 USER_CODE_BEFORE_CONFIRM pid=1。
3. +0.018333s：/proc 快照：namespace payload host PID 6，NSpid 6:1，PPID 5，PID namespace pid:[4026532464]，PGID=SID=6，状态 S。
4. 产品确认函数尚未调用。State 不是 T；用户命令已开始。
5. 当前产品确认函数仅要求异 namespace、PPID=leader，不检查 T。故若在 marker 后调用，代码会识别 PID 6 并尝试 SIGCONT；这是源码推断，不是 harness 声称运行了产品确认函数。

结论：§0-A “确认完成前不执行用户代码”复现失败。SIGSTOP 系统调用返回或确认代码稍后发送 SIGCONT 均不能证明此前停住。

## 默认 reaper 拓扑探针

第二个真实 bwrap 探针移除 --as-pid-1，以 /usr/bin/setsid 启动同 PID gate shell；shell 在 STOP 后才输出 marker。宿主观察到：

- 外层 bwrap host PID 5。
- 内建 reaper namespace PID 1：host PID 6，PPID 5，NSpid 6:1，PID namespace inode pid:[4026532464]。
- gate/payload namespace PID 2：host PID 7，PPID 6，NSpid 7:2，同 namespace inode，启动时间 23726978，comm=sh，PGID=SID=7，状态 T。
- host 在看到 T 并核对 namespace、父子关系、start time、PGID/SID 后才发送 harness SIGCONT。
- +0.036568s 才出现 USER_CODE_AFTER_CONFIRM pid=2；随后 bwrap 返回 0。

这证明本机发行版 bwrap 默认拓扑与上游 v0.9.0 资料一致，也验证 setsid 命令可让 gate payload 成为独立 session/process group。SIGCONT 和 bwrap wait 都由诊断 harness 执行，仅作为拓扑探针，不是产品行为或产品回收验收。

## 清理归属

两次 probe 结束时，diagnostic harness 发信号并 wait 自己的 bwrap 子进程。当前失败 repro 用 harness SIGKILL bwrap，观察到 payload 不再存在；这不能计作产品回收。第二探针的 wait 只证明 harness 观察到 bwrap 完成，不计作应用的 Child wait。产品 reaping 责任需在实现后按原调用者 wait owner 和 bwrap PID 1 wait 另行验收。

## 依据与状态

- bubblewrap 官方 v0.9.0 源码与 man page、Linux PID namespace/signal/wait/setsid 语义列于 [项目计划](../../plans/2026-11-06-1106-code-038-sandbox-start-gate-plan.md#appendix)。
- 当前验收结论：038 §0-A 未满足。MCP 和 apply_patch policy=None 路径另有源码可确认的确认缺口；见上表。推荐复用 bubblewrap 内建 reaper；下一步按项目计划实现并定向验收。
- 未运行全量、L3、CI；未提交、推送、部署或操作凭据。

## 实现候选验收（2026-11-06，覆盖上方修复前诊断结论）

上方“当前四入口链路审计”和“当前验收结论”是实现前的历史快照。实现候选现已采用 bubblewrap 内建 PID 1 回收器。产品不依赖 namespace PID 2：从 bwrap 直接子进程按 `NSpid` 末项为 1 定位 reaper，再从 reaper 直接子进程动态定位唯一 payload。payload 以 setsid 建立独立 PGID/SID 后停止；宿主核验 namespace inode、父子关系、start time/PIDFD、State T、PGID/SID，SIGCONT 前再复核同一身份。

### 实测进程身份与事件顺序

真实 bwrap 产品 gate 回归：启动前 task 数 234（<500），串行执行，硬超时 40 秒。观察到 payload host PID=20、reaper host PID=19、payload `NSpid` 末项=2、State=T、PGID=SID=20、namespace=`pid:[4026532464]`，`pidfd_fixed=true`。gate callback 观察 marker 不存在；callback 返回 Ready 后 SIGCONT，用户 marker 才出现，bwrap 退出且其调用方 Child wait 成功。该运行中的 PID=20/19/2 是实测值，不是实现假设。

### 启动闸及入口结果

| 入口/范围 | 定向证据 | 结果 |
|---|---|---|
| 共用 gate | `gate_release_observes_stopped_payload_before_marker`；真实 bwrap，检查 State T、父子、namespace、PGID/SID、PIDFD 与 marker 顺序 | 通过 |
| exec | `gate03_released_runs_and_terminate_satisfies_016` | 通过；放行后命令执行，终止保留 016 契约 |
| MCP | `a038_mcp_gate_timeout_does_not_start_server`、`a033_sandboxed_stdio_keeps_explicit_env_or_fails_closed` | 超时无 server 启动标记；正常沙箱 server 启动并发现工具 |
| apply_patch | `gate_applies_patch_after_release`、`gate_policy_none_timeout_does_not_write_patch`、`gate_leader_gone_rejects_and_does_not_execute` | 正常放行后写入；policy=None 超时前文件不变；监督者提前退出时拒绝 |
| JS worker | `sandbox_worker_confirmation_timeout_does_not_dispatch_init_code`、`sandbox_worker_kill_denied_reaps_payload_group`、`sandbox_worker_release_signal_denied_reaps_unreleased_payload`、`sandbox_worker_capture_denied_fails_closed` | init 屏障和身份失败关闭通过；产品终止只命中核验载荷组，独立 session 哨兵存活且 worker PIDFD 回收 |
| 取消、超时及监督者异常 | `gate_confirm_cancel_does_not_execute`、`gate_confirm_timeout_does_not_release`、`gate_confirm_supervisor_dies_before_release_does_not_report_released` | 不放行；各自按既有失败结果清理 |

### 终止、D2 和回收回归

- `pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process`：真实沙箱 Ctrl+C 只影响目标 payload group；忽略 SIGINT 的载荷仍可由 terminate 结束，独立会话持续运行，日志停止写入。通过，18.31 秒。
- `c13_bwrap_owner_reap_matrix`：真实 bwrap owner/child 回收矩阵通过；动态 marker PID > 1，owner 正常回收。产品等待者保留自己的 bwrap Child。
- `sandbox_worker_terminate_reaps_payload`、`cpu_limit_kills_spin_worker_host_continues`、`wall_clock_limit_terminates_pending_sandbox_worker`：payload PIDFD 和 bwrap wait 均确认回收；CPU 限制后 host 可继续启动 worker；120.37 秒墙钟终止通过。
- 031 network-enabled D2 诊断变体通过（1/1，1.81 秒）：在原 `attack07_setsid_daemon_reaped_at_turn_end` 用例中临时授权网络但保留同一 031 受限挂载和 D2 载荷，证明 gate 与回收路径正常；临时 test source 改动已恢复。
- 原 network-disabled 031 用例在当前容器中环境阻断：bwrap stderr=`loopback: Failed to create NETLINK_ROUTE socket: Operation not permitted`，宿主 PID namespace 内未出现 init child，载荷未执行，确认闸未放行。此项不计通过；不要求修改 031 网络策略、workflow 或部署配置。

### 其他检查与状态

- bwrap 0.9.0 在本机完整与受限沙箱挂载视图中均可访问 `/usr/bin/setsid`；Ubuntu runner 目标镜像（24.04/26.04）由 util-linux 提供该命令，无新增部署依赖或 workflow 改动。
- 最小检查此前 `cargo check --locked --offline` 和相关集成测试 `--no-run` 通过。最终候选上的复跑结果将在本节末尾补记；代码变更后仍会重跑受影响检查。
- 未运行全量、L3、Windows 编译、G2、CI；未提交、推送、触发 CI、部署、重启容器或操作凭据。Windows 编译候选和 G2 仍待验证；038 整体未完成。

## 最终复核（2026-11-06）

最终源代码仍以 `b98d52a682a0ba84080d0043c7bc88f697460473` 为 HEAD；所有既有工作区差异均保留。本轮不改 workflow、容器配置、全局 wait owner 或 Windows 候选，也没有提交或推送。

最终 real-bwrap gate 事件记录：host bwrap 等待者确认 payload host PID=34、reaper host PID=33；payload 的 `NSpid` 末项为 2、State=T、PGID=SID=34、namespace=`pid:[4026532466]`、start time=25169694，`pidfd_fixed=true`。确认回调内 marker 不存在；回调返回 Ready、同一 PIDFD 身份复核通过并 SIGCONT 后 marker 才出现；调用方 Child wait 成功。此处 PID 2 是本次运行观察，不是实现假设。D2 sweep 排除 PID 1；确认取消/失败时仍由原入口等待者等待 bwrap。

| 最终代码路径 | 验证 | 结果 |
|---|---|---|
| 共用确认、取消、超时、监督者异常与 MCP 031 挂载 | `cargo test -p terminal-chat --locked --offline --lib gate_ -- --test-threads=1 --nocapture` | 7/7 通过；含确认回调中置取消 marker 不执行、真实 bwrap 身份和 T 状态检查、MCP 原程序单文件挂载断言 |
| exec 放行与 016 terminate | `gate03_released_runs_and_terminate_satisfies_016` | 1/1 通过，8.84 秒；marker 在释放后出现，等待者回收 bwrap |
| apply_patch 输入屏障 | `cargo test ... --test apply_patch_sandbox gate_ -- --test-threads=1 --nocapture` | 3/3 通过；覆盖正常写入、policy=None 超时文件不变、监督者先退出拒绝 |
| MCP stdio | `cargo test ... --test mcp_stdio_sandbox -- --test-threads=1` | 2/2 通过；server 启动记录、env 保留、gate 超时无启动标记 |
| JS worker | `cargo test ... --test js_orchestration sandbox_worker_ -- --test-threads=1` | 13/13 通过；init 帧屏障、启动超时、取消、信号拒绝回退与 worker 回收 |
| C13、管道 Ctrl+C 和自然退出竞态 | `c13_bwrap_owner_reap_matrix`、`pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process`、`proc_session::tests::sandbox_interrupt_after_payload_exit_is_natural_completion` | 分别 1/1、1/1、1/1 通过；独立会话保留；自然退出时 SIGINT 的 ESRCH/NotFound 继续由 monitor 收集终态 |
| CPU 和墙钟终止 | `cpu_limit_kills_spin_worker_host_continues`、`wall_clock_limit_terminates_pending_sandbox_worker` | 分别 1/1、1/1 通过；5.69 秒与 120.35 秒；worker 与 bwrap 均已回收 |
| 启动关闭协调 | `close_kills_all_and_rejects_new_starts` | 1/1 通过；start 专用锁协调 manager close，确认期间释放全局状态锁 |
| 构建和测试编译 | `cargo check --locked --offline`；相关 lib、proc_session、mcp_stdio_sandbox、sandbox_matrix_probes 的 `--no-run` | 均通过；保留项目现有 warning |
| 差异格式检查 | `git diff --check` | 通过。`cargo fmt --all -- --check` 未能运行：当前 stable toolchain 未安装 cargo-fmt；未改动全局 toolchain。 |

最小执行命令在每项前读取 `/sys/fs/cgroup/pids.current` 并确认 `<500`，单项串行运行；墙钟用例使用既定 120 秒限时。结束检查未发现残留 bwrap、sleep、tchat-js-worker 或 terminal-chat 进程。

独立审查提出的事项已逐项复核并处理：SIGCONT 前最终检查取消；读取 `/proc/self/ns/pid` 失败也进入中止清理；确认等待不再持有管理器状态锁；close 在途启动时按已保存身份定向杀 payload、D2 sweep 后再给 PID 1 回收机会；031 MCP gate 包装保留原服务器单文件挂载；普通与沙箱 Ctrl+C 在 payload 已自然退出时继续收集终态；MCP close 对 sandbox kill 后等待有界并保留监督进程后备。MCP close 仍沿用原有 `()` 返回接口；若 payload signal、组后备和直接 child kill 全部被宿主拒绝，最终有限等待不能证明回收，未新增不同于既有接口的失败结果。

原 network-disabled 031 D2 测试仍受当前容器网络命名空间限制：bwrap 返回 `loopback: Failed to create NETLINK_ROUTE socket: Operation not permitted`，没有载荷或用户代码；该项为环境阻断，不计通过。对应 network-enabled 变体已通过。Windows 编译、Windows G2、macOS、全量与 L3 仍未验证；038 整体仍未完成。
