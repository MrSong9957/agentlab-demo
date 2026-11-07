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

## 监督者退出判定复核（2026-11-07，未提交候选）

候选工作区：`/tmp/038-start-gate-YcOITR/repo`；分支 `debug/038-ubuntu-start-gate-ci`；HEAD 与适用基线均为 `63ca1d2cfa0485b665907c492e8d41d9396bee9e`。候选仅修改 `src/app/src/sandbox.rs`、`src/app/src/proc_session.rs`、`src/app/src/sandbox/tests.rs` 和本验收记录；未提交。

### 根因与修复

基线 `sandbox.rs` 的确认循环以 `/proc/<leader_pid>` 是否存在判断监督者是否退出（基线第 1416–1425 行）。回调杀死 bwrap 并通过 `child.try_wait()` 确认回收后，若内核将相同数字 PID 分配给新进程，`/proc/<leader_pid>` 仍存在。回调耗尽确认期限并返回 `Pending` 后，旧逻辑将替代进程当作原监督者，进入超时 abort 路径。

这一因果链已在基线复现，不再只是 PID 重用假设：隔离 user/PID namespace 中，回归确认原 bwrap 已收到 SIGKILL 并被 `try_wait()` 回收，随后确定性重用 PID 5；回调等到确认期限后返回 `Pending`。基线结果为确认超时、`abort_calls=1`，替代进程仍存活。精确输出见本节证据目录中的 `baseline-red-pid-reuse-deadline.log`。CI run [#79](https://github.com/MrSong9957/agentlab-demo/actions/runs/42300882693) 的监督者退出确认失败与该结果一致。

候选在确认开始时固定原监督者 PIDFD，并在确认循环入口及 `validate` 回调返回后检查该 PIDFD（当前 `sandbox.rs` 第 1406、1425、1489 行）。确认原监督者退出后立即返回 `LeaderGone`，不处理回调结果、不进入超时 abort，也不向复用旧数字 PID 的进程发送信号。存活监督者的回调若耗尽期限，仍按既有超时路径失败。PIDFD 退出轮询不消费原 `Child` 的等待状态，原调用者仍负责 wait。

回归曾短暂使用 `start_time` 必须变化来证明新旧进程身份；一次快速 PID 重用在相同内核时钟 tick 内完成，该断言失败。`start_time` 以 tick 计数，不能单独作为身份唯一性证明。最终回归以原 `Child` 已回收、替代 `Child` 获得同一 PID 且仍存活来固定身份关系；不依赖 tick 差值。

### 本地验证

下列命令均在隔离候选工作区运行，`CARGO_TARGET_DIR=/tmp/038-start-gate-final-target`、`CARGO_BUILD_JOBS=1`。每项重进程验证启动前检查 `/sys/fs/cgroup/pids.current < 500`；实际值为 240–248。命令均设置硬超时。

| 检查 | 实际结果 | 日志 |
|---|---|---|
| `cargo check --locked --offline -p terminal-chat`（90 秒上限） | 通过，1.05 秒 | `candidate-cargo-check.log` |
| `cargo test --locked --offline -p terminal-chat --lib --no-run`（60 秒上限） | 通过，4.12 秒；测试二进制已生成 | `candidate-test-compile.log` |
| `cargo test --locked --offline -p terminal-chat --lib proc_session::tests::gate_confirm_ -- --test-threads=1 --nocapture`（70 秒上限） | 7/7 通过，4.31 秒。覆盖回调耗尽期限、取消、监督者退出的 Pending/Ready、PID 重用及既有超时 | `candidate-confirm-regressions.log` |
| `gate_release_observes_stopped_payload_before_marker`（35 秒上限） | 1/1 通过；放行前 payload 状态 T，PGID=SID=23，PIDFD 已固定 | `candidate-release-gate.log` |
| `sandbox::tests::leader_pidfd_exit_check_preserves_wait_owner`（35 秒上限） | 1/1 通过；PIDFD 显示退出后，原 `Child::try_wait()` 仍取得状态 | `candidate-wait-owner.log` |
| `git diff --check` | 通过 | 命令输出无差异错误 |

最终 PID 重用实测：监督者与替代进程均使用 host PID 5；结果 `LeaderGone`、`abort_calls=0`、`replacement_alive=true`。确认和放行前身份日志、构建日志、修复前红测及当前基线补丁均保存在 `/workspace/target/038-validation/2026-11-07-start-gate-supervisor-exit/`，目录清单和 SHA-256 见其中 `README.md`。

### 简化、审查与交付状态

- 简化检查没有发现可合并的生产逻辑、代码质量或效率项；删除四条重复测试名称或断言的新增说明注释，保留解释 namespace 局部 PID 分配与 `start_time` tick 精度的注释。测试价值审查没有建议删除或合并本轮回归。
- Standards 审查未发现规范违规；Spec 审查未发现 §0-A 缺项、范围扩张或错误实现。审查仅覆盖本候选差异。
- 交付候选范围为三项代码文件和本验收记录：生产修复 `sandbox.rs`；直接回归 `proc_session.rs`、`sandbox/tests.rs`；配套验收记录为本文件。主工作区 `/workspace` 及其其他任务差异不属于候选范围。
- 提交前状态记录：本地确认和真实 bwrap 验证通过；当时 GitHub Actions 尚未验证该修改，run #79 为失败。本次新 SHA 的远端结果见下方；本地结果不代表 038 整体完成。

## 新 SHA 三平台 CI 验收（2026-11-07）

- 提交：`40a6d98288261721e501f86481ac6635ef1adb1f`，父提交 `63ca1d2cfa0485b665907c492e8d41d9396bee9e`；仅含本节所述四文件候选。PR #1 head 与 Actions run head 均为该 SHA。
- 运行：[CI run #42338650474](https://github.com/MrSong9957/agentlab-demo/actions/runs/42338650474)，事件 `pull_request`，终态 `failure`。PR 检查使用的合并快照为 `0d73ff3e7b124a3da8ce1805928fd069f0995078`。
- 三个平台的 `cargo check` 均通过；三平台的全量 `cargo test` 均实际失败。Windows/macOS job 标记了 `continue-on-error`，但各自 job 结论仍为 `failure`，不能视作测试通过。各 job 日志：[Ubuntu](https://github.com/MrSong9957/agentlab-demo/actions/runs/42338650474/job/128301210963)、[Windows](https://github.com/MrSong9957/agentlab-demo/actions/runs/42338650474/job/128301210887)、[macOS](https://github.com/MrSong9957/agentlab-demo/actions/runs/42338650474/job/128301210684)。

### 平台结果

| 平台 | 编译 | 全量测试 | 日志结论 |
|---|---|---|---|
| Ubuntu | `cargo check` 通过 | 失败；5 个 target 失败：`--lib`、`--test js_orchestration`、`--test js_orchestration_security`、`--test sandbox_matrix_probes`、`--test security_attack_031` | exit 101 |
| Windows | `cargo check` 通过 | 失败；10 个 target 失败：`--lib`、`--test apply_patch_sandbox`、`--test exec_command`、`--test mcp_tls`、`--test mcp_turn`、`--test memory_retention`、`--test memory_storage_files`、`--test multi_agent`、`--test proc_session`、`--test tool_loop`。其中 31 个测试报告失败 | exit 1；`normalize_rejects_missing_and_accepts_dotdot` 在 38-test target 中实际执行并通过 |
| macOS | `cargo check` 通过 | 失败；58 个测试报告失败，12 个 target 失败：`--lib`、`--test e2e_acceptance`、`--test e2e_sandbox`、`--test exec_command`、`--test js_orchestration_security`、`--test llm_flow`、`--test mcp_tls`、`--test memory_retention`、`--test proc_session`、`--test security_attack_031`、`--test session_cli`、`--test session_perf` | exit 101 |

Windows 日志中的 31 个失败用例为：`sandbox::tests::non_canonical_writable_root_fails_closed`、`internal_apply_entry_reads_stdin_and_writes_result`、`cancel_keeps_process_then_terminate_stops_writes`、`exec_p01_huge_timeout_accepted_then_terminate_reaps_group`、`exec_b07_concurrent_groups_isolated`、`min_timeout_accepted_and_expires_as_one_millisecond`、`timeout_kills_process_group_and_stops_writes`、`tls_https_round_trip_with_injected_root`、`a10_mixed_native_and_mcp_batch_runs_serially_and_pairs_in_order`、`a17_mcp_with_view_image_batch_pairs_then_image_projection`、`resources_filename_boundary_instructions_and_notes`、`m4_a13_m3_parse_store_export_redacts_body_and_exact_slug_projection`、`c15_persist_failure_spawn_rejected`、`blocked_input_is_cancellable_and_session_kept`、`cancel_wait_keeps_process_alive_and_queryable`、`capacity_64_rejects_65th_without_eviction`、`close_kills_all_and_rejects_new_starts`、`deadline_survives_cancel`、`explicit_timeout_kills_without_any_query`、`handles_scoped_to_manager_instance`、`input_limit_exact_passes_over_limit_rejects_with_zero_write`、`pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process`、`big_dual_stream_completes_and_truncates`、`pty_cancel_stops_further_input_submission`、`pty_child_sees_terminal`、`pipe_rejects_text_input_session_still_queryable`、`oversized_pending_stays_bounded`、`pty_interactive_read_answer`、`pty_write_window_expiry_returns_running_for_nonreading_program`、`read_does_not_reset_deadline`、`t18_cancelled_batch_skips_plan_as_two_field_failure`。

### 监督者退出与指定回归

| 用例 | 远端实际结果 | 证据与判定 |
|---|---|---|
| `gate_confirm_supervisor_dies_before_release_does_not_report_released` | 失败 | CI 断言收到 `TimedOut`，错误为“沙箱隔离/载荷停止状态确认超时……监督进程已回收”；原日志没有回调计数或 PIDFD 状态，不能证明确认回调执行，也不能证明监督进程已被等待回收。该远端验收未通过；时序复核见下节。 |
| `gate_confirm_supervisor_pid_reuse_does_not_abort_or_signal_replacement` | 环境阻断，未验证产品断言 | 子进程中的 `unshare --user --map-root-user --pid --fork` 因 `write failed /proc/self/uid_map: Operation not permitted` 退出。不能计为通过或产品行为失败。 |
| `gate_confirm_callback_exhausting_deadline_does_not_release` | 通过 | Ubuntu 测试日志显示该用例行以 `... ok` 结束。 |
| `leader_pidfd_exit_check_preserves_wait_owner` | 通过 | Ubuntu 测试日志显示该用例行以 `... ok` 结束。 |
| `gate_confirm_supervisor_dies_before_release_ready_does_not_release`、`gate_release_observes_stopped_payload_before_marker` | 未得到独立结果 | 两者均在单元测试锁已 poisoned 后因 `PoisonError` 失败，未进入各自行为断言。 |

原 `TEST_ENV_LOCK` 锁中毒级联未消失：核心退出测试先失败，随后 Ready、停止载荷放行和 sandbox 环境单测报告 `PoisonError`。同一 `--lib` 目标还出现两条已有进程终止 seam 失败：`seam_killpg_injection_deadline_not_fake_timeout` 得到 `Timeout`（断言要求不是伪造 timeout），`seam_killpg_injection_terminate_reports_failure_keeps_session` 得到 `Terminated`（断言要求 `Error`）。

其他 Ubuntu 失败：

- JS worker 的取消、payload 退出、监督者退出、会话隔离和墙钟终止测试在 `assert!(sandbox_requested())` 失败。Actions job 环境未设置 `TCHAT_SANDBOX`，故这些用例没有进入沙箱行为验证。
- `b2_mcp_face_fd_closed` 失败：断言期望 `tools=1`，实际 `tools=0`；记录 `FDSCAN=cat: /proc/self/fd/10: No such`。这是实际失败，不能据此宣称 FD 验收通过。
- `attack06_seccomp_allows_toolchain_blocks_privileged_syscalls` 失败：其他被测 syscall 返回 `Operation not permitted`，但 `clone3` 返回 `Invalid argument`；断言要求 `ENOSYS`。

### 结论

- 新 SHA 的 CI 已完成，但监督者退出判定没有通过远端验收；PID 重用回归受 runner user namespace 权限阻断。停止载荷放行回归因锁中毒没有独立验证。
- Windows G2 用例 `normalize_rejects_missing_and_accepts_dotdot` 在此提交快照上实际执行并通过；Windows 其他全量测试仍失败。
- macOS 和 Windows 的失败结果是其 `continue-on-error` job 中的真实测试结果。此 CI 不证明 038 整体完成，也不能关闭失败或环境阻断的验收项。本节只记录旧 SHA 的结果。

## 监督者退出 CI 时序复核与本地候选（2026-11-07）

### CI 日志能确认的事实

- 仍是 CI run [#42338650474](https://github.com/MrSong9957/agentlab-demo/actions/runs/42338650474)，Ubuntu job [日志](https://github.com/MrSong9957/agentlab-demo/actions/runs/42338650474/job/128301210963)，合并检出 SHA `0d73ff3e7b124a3da8ce1805928fd069f0995078`，PR head 为 `40a6d98288261721e501f86481ac6635ef1adb1f`。
- 目标测试在 `src/app/src/proc_session.rs:2804` 因 `expect("bwrap 消失应返回 LeaderGone")` 收到确认超时错误而 panic。原测试在此 `expect` 后才检查 `release_calls` 和 `abort_calls`，所以 CI 日志没有回调计数。
- 文案“监督进程已回收”由 `abort_sandbox_confirmation` 在传入的 abort 回调返回 `Ok(())` 时生成。目标测试的 abort 回调只增加 `abort_calls` 并返回 `Ok(())`；它不杀死、不等待 Child。该文案不是 wait 证据，也不能证明验证回调已运行。
- 成功执行验证回调时，回调会杀死 bwrap 并用 `child.try_wait()` 取得退出状态。确认函数随后立即轮询固定的原监督者 PIDFD；若原 Child 已由回调 wait，PIDFD 应报告已退出并返回 `LeaderGone`，再到超时分支的路径不成立。CI 结果表明验证回调未成功返回；旧 CI 日志没有事件跟踪，不能判断是首次扫描前超时还是另一个阶段。
- CI 日志将 `gate_confirm_timeout_does_not_release` 记为通过。该测试在同一单元测试进程内把 `TCHAT_SANDBOX_CONFIRM_TIMEOUT_MS` 设为 `1`，但未取得 `TEST_ENV_LOCK`；目标测试持有该锁，却不能阻止未持锁测试改写进程环境。确认函数读取的是该全局环境变量。这是与错误时序相符的竞态来源，但旧 CI 日志未记录目标用例实际读到的期限，故不能声称已从远端日志直接观测到该值。

### 单调时钟复现

改动前以 1 ms 注入值串行运行目标用例，并临时加测试诊断。原监督者 PIDFD 由目标测试另开一个观察句柄；该观察不调用 `Child::wait`：

| 相对确认开始时间 | 事件 | 观察 |
|---:|---|---|
| 0.000 ms | 测试启动 | leader PIDFD 未退出；注入期限 1 ms |
| 0.020 ms | 确认开始 | 同一期限 1 ms |
| 0.028 ms | 首次 leader PIDFD 轮询 | live |
| 0.187 ms | 首次 payload 扫描 | 未发现载荷 |
| 2.339 ms | 确认循环再次检查期限 | `before_discovery` 超时；尚未调用 validate |
| 153.181 ms | 测试 abort 回调 | leader PIDFD 仍未退出；这是 abort 路径，不是 Child wait |
| 153.260 ms | 确认返回 | `release_calls=0`、`abort_calls=1`、leader PIDFD 未退出；错误带有“监督进程已回收”后缀 |

临时诊断已从候选代码移除，原始本地输出保存在 `target/038-validation/2026-11-07-supervisor-timeout-race/forced-one-ms-before-fix.log`。默认环境下同一目标在基线代码上串行通过，耗时 0.01 秒。受控复现证明未同步的 1 ms 全局期限可产生与 CI 相同错误；它不伪称为 CI 运行本身的内部事件日志。

### 最小测试修复与验证边界

- `gate_confirm_timeout_does_not_release` 现在与其他沙箱环境测试共用 `TEST_ENV_LOCK`，并用同一个 `TestEnvRestore` 在持锁期间恢复原期限。产品的 2000 ms 默认值、确认截止逻辑和错误契约未变。候选只修复测试环境共享竞态，没有产品逻辑修改。
- 真实 PID 重用测试仍保留原 `unshare`/`ns_last_pid` 场景，但标记为显式 ignored，避免无权限 runner 把环境错误报成产品失败。Cargo 测试清单显示 ignored 原因。具备所需 namespace 权限时，运行 `cargo test --locked --offline -p terminal-chat --lib proc_session::tests::gate_confirm_supervisor_pid_reuse_does_not_abort_or_signal_replacement -- --exact --ignored --test-threads=1` 执行原真实 PID 重用断言。它不计通过。基础 CI 回归仍执行确认回调杀死并 wait 原 bwrap 的测试、停止态放行观察和 PIDFD wait-owner 测试。
- 当前远端 CI 没有诊断事件。只有新提交 SHA 的 Ubuntu 回归结果可以确认本地锁修复消除了该远端失败；本节不关闭该 CI 阻断。PID 重用覆盖在有权限环境下仍须实际执行并通过。
- 本轮候选范围：`src/app/src/proc_session.rs` 中全局期限测试隔离和显式标注受权限限制的真实 PID 重用测试；本验收记录中的 CI 证据修正和时序记录。`src/app/src/sandbox.rs` 没有最终差异。所有改动均未提交或推送。

### 本地候选验证（基线仍为 40a6d98）

所有测试分别以 `--test-threads=1` 执行；启动前 `pids.current` 分别为 242、241、241，低于 500。Cargo 使用 `CARGO_TARGET_DIR=/tmp/038-start-gate-final-target` 和 `CARGO_BUILD_JOBS=1`，每项有 `timeout` 硬上限。输出副本保存在 `/workspace/target/038-validation/2026-11-07-supervisor-timeout-race/`。

| 检查 | 实际结果 | 证据 |
|---|---|---|
| `cargo check --locked --offline -p terminal-chat` | 通过，退出码 0；1.56 秒 | `cargo-check.log` |
| `proc_session::tests::gate_confirm_` | 6 通过，0 失败，1 ignored，297 filtered；2.24 秒 | `gate-confirm.log`。目标 supervisor-death 回归、Ready 变体、超时和取消均实际执行并通过；真实 PID 重用未执行，ignored 原因和精确命令出现在测试清单。 |
| `proc_session::tests::gate_release_observes_stopped_payload_before_marker` | 1/1 通过；放行前观察 payload host PID 23、reaper PID 22、NSpid 2、State T、PGID=SID=23、namespace `pid:[4026532486]`、start time 30451117、固定 PIDFD | `stopped-release.log` |
| `sandbox::tests::leader_pidfd_exit_check_preserves_wait_owner` | 1/1 通过；PIDFD 检查退出后原 `Child::try_wait()` 仍能取得退出状态 | `wait-owner.log` |

旧 CI 失败时的“回调是否进入”没有原始远端事件记录。当前候选本地测试证明已修正环境变量争用；它尚未在 GitHub Actions 新 SHA 上验证。CI run #42338650474 的其他失败没有纳入本轮修改。

## timeout 测试隔离与失败诊断候选（2026-11-07）

### 已确认缺陷与证据边界

- 源码核对确认：旧版 `gate_confirm_timeout_does_not_release` 在单元测试进程中写入全局 `TCHAT_SANDBOX_CONFIRM_TIMEOUT_MS=1`，没有取得与其他 gate 测试共用的 `TEST_ENV_LOCK`，并在测试结束时直接删除变量。另一个目标用例持有锁并不能阻止该无锁写入。这是已证实的测试隔离缺陷。
- 改动前的本地受控复现使用 1 ms 注入值：确认循环在载荷发现前耗尽期限，validate 未进入，监督者 PIDFD 仍 live，随后进入 abort。输出保存在 `/workspace/target/038-validation/2026-11-07-timeout-isolation-final/pre-fix-forced-one-ms.log`。它证明该环境值可产生超时路径，不证明旧 CI 使用了同一读值或阶段顺序。
- CI run [#42338650474](https://github.com/MrSong9957/agentlab-demo/actions/runs/42338650474) 没有记录目标测试读到的期限、payload discovery、validate 回调或 PIDFD 状态。因此旧 CI 的实际时序仍未证实；新候选尚未推送，本地证据不能代替新 SHA 的远端验收。

### 最小差异

- `proc_session.rs`：所有直接写入确认期限的 lib 测试持有 `TEST_ENV_LOCK`，并通过 Drop 恢复原始 `OsString`。监督者退出 Pending/Ready 回归显式使用 2000 ms，避免继承外部期限。既有“回调耗尽期限”回归启用测试 trace，用于实际观察 `after_validate` 期限和 abort 顺序。
- `tests/common/mod.rs`：新增仅保存/恢复确认期限的窄型 RAII guard；调用者必须先取得本测试二进制既有互斥锁。
- `apply_patch_sandbox.rs`、`mcp_stdio_sandbox.rs`、`js_orchestration_security.rs`：timeout 写入均在各自已有锁内执行并恢复旧值。JS worker 测试的普通启动统一经窄 helper 持 `ENV_LOCK` 至 `spawn_cell` 完成；墙钟回归把沙箱前置检查、期限固定和启动放入同一锁区，避免并行 timeout 测试改变其读取与启动视图。子进程 `.env(...)`/`.env_remove(...)` 仅设置子进程环境，不修改测试进程环境。
- `sandbox.rs`：新增仅在 `cfg(test, target_os = "linux")` 生效的线程局部 trace；仅显式启用该 trace 的测试会输出。事件使用 `Instant` 单调时间，覆盖读到的期限、leader PIDFD、载荷发现和 State T、validate 进入与结果、期限耗尽阶段、abort 起止及回调结果。目标监督者回归另记录回调前 PIDFD、kill 结果、`Child::try_wait()`、等待后的 PIDFD、最终结果和回调计数。未修改产品确认、等待、终止或失败逻辑。
- 唯一 ignored 用例为真实数字 PID 重用回归 `gate_confirm_supervisor_pid_reuse_does_not_abort_or_signal_replacement`，其 `unshare --user --map-root-user --pid --fork` 需要 runner 未提供的可写 user/PID namespace。原用例和断言保留，普通 CI 不计此项通过；测试注解中的完整命令已用 `--exact --ignored --list` 核实，只列出这一用例，日志保存在 `/workspace/target/038-validation/2026-11-07-timeout-isolation-final/ignored-list.log`。
- 普通 runner 仍实际运行稳定 PIDFD 身份观察、Supervisor `LeaderGone` Pending/Ready、确认期限顺序以及原 `Child` wait owner 回归。真实 PID 数值重用是单独补充覆盖，不从普通 runner 结果推断。

### 最终候选本地验证（适用基线 `40a6d98288261721e501f86481ac6635ef1adb1f`）

命令按序单线程运行，每个重进程启动前读取 `/sys/fs/cgroup/pids.current`，记录值均低于 500；使用 `CARGO_TARGET_DIR=/tmp/038-start-gate-final-target`、`CARGO_BUILD_JOBS=1` 和硬超时。完整输出与补丁保存在 `/workspace/target/038-validation/2026-11-07-timeout-isolation-final/`。

| 检查 | 实际结果 |
|---|---|
| `cargo check --locked --offline -p terminal-chat` | 通过；测试编译另由各直接测试命令完成。保留仓库既有编译 warning。 |
| `proc_session::tests::gate_confirm_` | 6 通过、0 失败、1 ignored、297 filtered；2.23 秒。实际目标 trace 观察到期限 2000 ms、payload 从未发现转为 PID 312、reaper PID 311、State T、回调进入；回调 SIGKILL 原 bwrap 并 `Child::try_wait()` 得到 SIGKILL，原 PIDFD 随后为 exited，Pending/Ready 均返回 `LeaderGone`、`abort_calls=0`。 |
| `gate_confirm_callback_exhausting_deadline_does_not_release`（包含于上项） | 实际执行并通过；单调时间日志记录 `validate.return=ready`、leader PIDFD live、`deadline.expired stage=after_validate`、`abort.begin` 及 abort callback `Ok`。这是当前诊断代码对期限和 abort 路径的受控验证。 |
| `gate_release_observes_stopped_payload_before_marker` | 1/1 通过；实际执行。 |
| `leader_pidfd_exit_check_preserves_wait_owner` | 1/1 通过；实际执行，PIDFD 检查没有消费原 `Child` 等待状态。 |
| `a038_mcp_gate_timeout_does_not_start_server` | 1/1 通过；实际执行。 |
| `gate_policy_none_timeout_does_not_write_patch` | 1/1 通过；实际执行，补丁目标文件未被写入。 |
| `gate_timeout_worker_startup_fails_closed` | 1/1 通过；实际执行。 |
| `wall_clock_limit_terminates_pending_sandbox_worker` | 1/1 通过，120.16 秒；沙箱前置、固定期限与 worker 启动使用同一测试锁。 |
| `git diff --check` | 通过。 |

简化检查复用了 `tests/common` 的期限恢复 guard，移除了 apply_patch 测试中的重复实现；JS 启动互斥仅覆盖 worker 启动读取环境的临界区。独立规格审查未发现阻断项；标准审查找到并促成修复 JS worker 环境读取者未参与互斥的问题，复核后无剩余锁重入或覆盖缺口。没有改变生产契约或代码路径。

最终本地候选改动 7 个文件：`src/app/src/sandbox.rs`、`src/app/src/proc_session.rs`、`src/app/tests/common/mod.rs`、`src/app/tests/apply_patch_sandbox.rs`、`src/app/tests/mcp_stdio_sandbox.rs`、`src/app/tests/js_orchestration_security.rs` 和本验收记录。候选补丁适用上述 `40a6d98` 基线。新候选远端 CI 未验证；监督者退出 CI 阻断仍未关闭。没有提交、推送或触发 CI。

## timeout 隔离候选远端 CI（2026-11-07，未提交证据更新）

- 已发布提交：`f6e803ee5a81145e8a6890406eed7759ee1d4c62`，父提交 `40a6d98288261721e501f86481ac6635ef1adb1f`；提交范围为上方列出的七文件。
- 推送分支、PR #1 head 与 Actions run head 均为 `f6e803ee5a81145e8a6890406eed7759ee1d4c62`。PR #1 仍开放，base 为 `main`。
- [CI run #81](https://github.com/MrSong9957/agentlab-demo/actions/runs/42356537594)（`pull_request`）已完成，整体结论 `failure`。三个平台的 `cargo check` 均通过；三个全量测试均实际执行并失败。Windows/macOS 的 `continue-on-error` 不改变各自的真实测试失败结论。D1–D3 诊断 job 按工作流设置跳过。
- Actions 使用 `cargo test --workspace --no-fail-fast --locked`，没有 `--nocapture`。因此通过测试捕获的 `eprintln!` 时序诊断没有出现在 CI 日志；CI 可确认对应测试实际通过，详细单调时钟事件顺序仍引用前述本地 `gate-confirm.log`，不把它描述为远端日志观察。

| 平台 | 编译 | 全量测试 | 实际失败 target |
|---|---|---|---|
| Ubuntu | `cargo check --locked` 通过 | 失败，exit 101 | `js_orchestration`、`sandbox_matrix_probes`、`security_attack_031`（3 个 target） |
| Windows | `cargo check --locked` 通过 | 失败，exit 1；31 个测试失败 | `--lib`、`apply_patch_sandbox`、`exec_command`、`mcp_tls`、`mcp_turn`、`memory_retention`、`memory_storage_files`、`multi_agent`、`proc_session`、`tool_loop`（10 个 target） |
| macOS | `cargo check --locked` 通过 | 失败，exit 101 | `--lib`、`e2e_acceptance`、`e2e_sandbox`、`exec_command`、`js_orchestration_security`、`llm_flow`、`mcp_stdio`、`mcp_tls`、`memory_retention`、`proc_session`、`security_attack_031`、`session_cli`、`session_perf`（13 个 target） |

作业日志：[Ubuntu](https://github.com/MrSong9957/agentlab-demo/actions/runs/42356537594/job/128355745442)、[Windows](https://github.com/MrSong9957/agentlab-demo/actions/runs/42356537594/job/128355745297)、[macOS](https://github.com/MrSong9957/agentlab-demo/actions/runs/42356537594/job/128355745523)。

### 本轮关键验收

| 用例或检查 | 远端结果 | 证据与限制 |
|---|---|---|
| `gate_confirm_supervisor_dies_before_release_does_not_report_released` | Ubuntu 实际执行并通过 | 同一个 Ubuntu `--lib` 测试目标还实际执行并通过 `gate_confirm_supervisor_dies_before_release_ready_does_not_release`。通过用例的 trace 被 Cargo 捕获，CI 日志没有逐事件 trace 行。 |
| `gate_confirm_callback_exhausting_deadline_does_not_release` | Ubuntu 实际执行并通过 | 本地 `gate-confirm.log` 保留 deadline、validate、PIDFD、abort 的单调时钟顺序；远端绿色用例只证明断言通过。 |
| `gate_release_observes_stopped_payload_before_marker` | Ubuntu 实际执行并通过 | 通过。 |
| `leader_pidfd_exit_check_preserves_wait_owner` | Ubuntu 实际执行并通过 | 通过；没有消费原 `Child` 的 wait 状态。 |
| 真实 PID 数值重用回归 | Ubuntu ignored，未验证 | 日志说明 writable user/PID namespace 不可用；忽略状态不计为通过。 |
| `attack07_setsid_daemon_reaped_at_turn_end` | Ubuntu 实际执行并通过 | 在 `security_attack_031` 同一目标中通过；该目标因 attack06 失败而整体失败。 |
| Windows `normalize_rejects_missing_and_accepts_dotdot` | 实际执行并通过 | Windows `proc_session` 测试日志明确记录 `... ok`；不能据此推断完整 Windows 测试通过。 |

### CI 失败事实

- Ubuntu 的 `js_orchestration` 有 4 个失败：`sandbox_worker_cancel_inflight_leaf`、`sandbox_worker_payload_exits_before_terminate`、`sandbox_worker_supervisor_dies_reports_failure`、`sandbox_worker_timeout_and_session_isolation`。四者都在测试文件内的 `assert!(terminal_chat::sandbox::sandbox_requested())` 失败；这次日志没有给出该断言失败的更深原因。
- Ubuntu 的 `b2_mcp_face_fd_closed` 失败于工具发现断言：期望 `tools=1`，实际 `tools=0`；探针记录 `FDSCAN=cat: /proc/self/fd/10: No such`。这是实际失败；该输出不足以单独证明存在 FD 泄漏或确定根因。
- Ubuntu 的 `attack06_seccomp_allows_toolchain_blocks_privileged_syscalls` 失败：断言期望 `clone3=ENOSYS`，实际为 `Invalid argument`。同一目标中的 attack07 通过。
- Windows 有 31 个失败用例分布于上表 10 个 target。`exec_command` 有 5 个、`proc_session` 有 17 个失败；另有 `sandbox::tests::non_canonical_writable_root_fails_closed`、`internal_apply_entry_reads_stdin_and_writes_result`、两项 MCP turn、memory、multi-agent 和 tool-loop 用例失败。完整用例与断言见 Windows 作业日志。
- macOS 的失败跨越上表 13 个 target，包括 `proc_session` 的超时/终止回收断言和 `e2e_acceptance` 的进程清理断言；`session_perf::perf_tiers_baseline_driven` 也因实测峰值离散 119.4 MiB 超限而失败。完整用例与断言见 macOS 作业日志。
- 检查三平台作业日志，没有发现 `TEST_ENV_LOCK`、`PoisonError` 或由该锁中毒引发的级联。上述其余失败仍按各自日志事实记录，不归并为锁中毒问题。

### 结论和边界

- 监督者退出判定及普通 runner 可执行的期限、停止态放行和 Child wait-owner 回归在 Ubuntu 新 SHA 上通过；真实 PID 重用仍未验证。通过测试的远端时序 trace 不可见，已由本地证据补充记录。
- 该结果不能证明旧 CI 曾读取 1 ms，也不能关闭真实 PID 重用覆盖、B2 MCP 失败或本轮发现的其他三平台 CI 失败。
- 当前 CI 仅验证提交 `f6e803e` 的代码快照。038 整体未完成。
- 本节是运行后验收证据，仅追加于隔离候选工作区；未追加提交或推送。

## JS worker 沙箱回归环境前提（2026-11-07，本地未提交候选）

- CI #81 的 Ubuntu `js_orchestration` 日志中，以下四个用例都在启动 worker 前的 `sandbox_requested()` 断言失败：`sandbox_worker_payload_exits_before_terminate`（该 CI 版本第 434 行）、`sandbox_worker_supervisor_dies_reports_failure`（第 462 行）、`sandbox_worker_timeout_and_session_isolation`（第 793 行）、`sandbox_worker_cancel_inflight_leaf`（第 828 行）。失败发生在 `spawn_cell` 之前，原有退出、取消、超时及会话断言没有执行；本记录不沿用其他失败数量。
- 在本地显式移除 `TCHAT_SANDBOX` 与 `TCHAT_SANDBOX_CONFIRM_TIMEOUT_MS` 后，`sandbox_worker_payload_exits_before_terminate` 按原用例前提失败于 `sandbox_requested()`，尚未启动 worker。完整红测日志：`/workspace/target/038-validation/2026-11-07-js-worker-sandbox-env/baseline-unset.log`。这确认 runner 默认未设置沙箱变量会使回归前提缺失。
- 修复只改测试：父测试进程通过 `current_exe()` 重启当前精确用例；子进程在测试框架和库初始化前得到 `TCHAT_SANDBOX=1`，再运行原测试体。测试名取自当前测试线程，避免过滤名拼错时子进程零用例运行造成假通过。父测试进程环境不被修改，因此不需要跨线程环境锁或恢复 guard；生产环境和整个 CI job 都没有被改动。
- 四个用例均调用 `sandbox_payload_pid` 检查 bwrap 的 PID namespace、reaper 与实际载荷关系；会话隔离用例检查两个 cell 的载荷。原有行为断言保留。测试启动路径仍经 `spawn_cell` 和生产沙箱启动实现，不回退普通模式。

| 精确用例 | 结果 | 测试体耗时 | `pids.current` 启动门禁 | 日志 |
|---|---|---:|---:|---|
| `sandbox_worker_payload_exits_before_terminate` | 1 passed；实际载荷检查及退出后终止断言执行 | 0.18 s | 245 | `sandbox_worker_payload_exits_before_terminate.log` |
| `sandbox_worker_supervisor_dies_reports_failure` | 1 passed；实际载荷检查、监督者死亡及失败结果断言执行 | 0.18 s | 245 | `sandbox_worker_supervisor_dies_reports_failure.log` |
| `sandbox_worker_timeout_and_session_isolation` | 1 passed；两个载荷均检查，超时、终止和会话隔离断言执行 | 0.38 s | 245 | `sandbox_worker_timeout_and_session_isolation.log` |
| `sandbox_worker_cancel_inflight_leaf` | 1 passed；实际载荷检查、取消及终止断言执行 | 0.20 s | 246 | `sandbox_worker_cancel_inflight_leaf.log` |

四条最终命令均在外层环境移除上述两个变量后运行，使用 `cargo test --locked --offline -p terminal-chat --test js_orchestration <用例> -- --exact --nocapture --test-threads=1`，`CARGO_BUILD_JOBS=1`、每条 75 秒上限，并在运行前检查 `pids.current < 500`。Cargo 输出同时显示隔离子进程中的测试通过和父包装测试通过。四个日志均在上表所列独立文件中。

### 本地候选边界

- 最终本地直接回归通过，真实 bwrap 和载荷身份检查均实际执行；原行为断言没有放宽。简化审阅确认没有可替换的现成隔离辅助，也未发现资源效率问题；审阅发现并修正了测试名漂移可导致零用例子进程假通过的风险。测试名现在来自当前测试线程。
- 新差异对应 `src/app/tests/js_orchestration.rs` 与本验收记录。此记录还保留本候选开始前已有的 CI #81 验收证据更新，须按差异块选择。
- 本地结果不能代替新 SHA 的远端 CI。CI #81 原始失败仍是提交 `f6e803e` 的事实；该环境前提修复尚未提交、推送或由 GitHub Actions 验证。其他平台和独立失败不在本次处理范围。

## JS worker 沙箱前提修复远端 CI（2026-11-07，运行后未提交记录）

- 提交 `544a8e7db262fad50ddef2b2ffc5faa3c0ef1fb1` 基于 `f6e803ee5a81145e8a6890406eed7759ee1d4c62`，只包含 `src/app/tests/js_orchestration.rs` 与本验收记录。PR #1 仍开放，head SHA 为 `544a8e7db262fad50ddef2b2ffc5faa3c0ef1fb1`。
- [CI run #82](https://github.com/MrSong9957/agentlab-demo/actions/runs/42364810143) 的提交关联与 PR head 均指向该 SHA。事件为 `pull_request`，整体终态为 `failure`。三个平台的 `cargo check` 都通过；三个全量 `cargo test` 作业都实际执行并失败。Windows/macOS 的 `continue-on-error` 没有改变各自真实失败结果。D1–D3 诊断 job 按工作流设置跳过。
- 作业日志：[Ubuntu](https://github.com/MrSong9957/agentlab-demo/actions/runs/42364810143/job/128380519893)、[Windows](https://github.com/MrSong9957/agentlab-demo/actions/runs/42364810143/job/128380519915)、[macOS](https://github.com/MrSong9957/agentlab-demo/actions/runs/42364810143/job/128380519786)。

### 四个 JS worker 用例

四个用例只在 Linux 编译和执行（源码有 `#[cfg(target_os = "linux")]`）。Ubuntu 日志显示每个用例的隔离子进程均出现 `running 1 test`，精确筛选结果为 `1 passed; 0 failed; 0 ignored; 0 measured; 41 filtered out`；父测试也以 `... ok` 结束。整体 `js_orchestration` 目标为 42 通过、0 失败。用例进入原测试体：测试进程要求 `TCHAT_SANDBOX=1`，启动真实 worker，并调用 `sandbox_payload_pid` 验证实际载荷身份；各自原有行为断言均通过。Windows 与 macOS 因平台门控未运行这四项，不计为通过。

| 精确用例 | Ubuntu | 环境前提失败 | 行为断言失败 |
|---|---|---|---|
| `sandbox_worker_payload_exits_before_terminate` | 隔离子进程 1/1 通过，父测试通过 | 否 | 否 |
| `sandbox_worker_supervisor_dies_reports_failure` | 隔离子进程 1/1 通过，父测试通过 | 否 | 否 |
| `sandbox_worker_timeout_and_session_isolation` | 隔离子进程 1/1 通过，父测试通过 | 否 | 否 |
| `sandbox_worker_cancel_inflight_leaf` | 隔离子进程 1/1 通过，父测试通过 | 否 | 否 |

这些日志证明 CI #82 在该提交快照上实际进入真实沙箱并通过四个既有行为回归；它们不代表 Windows/macOS 执行了 Linux 专用用例，也不代表 038 整体验收完成。

### 三平台其余结果

| 平台 | cargo check | 全量测试 | 实际失败 target 与失败数 |
|---|---|---|---|
| Ubuntu | 通过 | 失败，exit 101 | `sandbox_matrix_probes` 1 项；`security_attack_031` 1 项。`js_orchestration` 42/42 通过。 |
| Windows | 通过 | 失败，exit 1 | 10 个 target、30 项：`--lib` 1、`apply_patch_sandbox` 1、`exec_command` 5、`mcp_tls` 1、`mcp_turn` 2、`memory_retention` 1、`memory_storage_files` 1、`multi_agent` 1、`proc_session` 16、`tool_loop` 1；`multi_agent` 另有 3 项 ignored。 |
| macOS | 通过 | 失败，exit 101 | 13 个 target、60 项：`--lib` 6、`e2e_acceptance` 27、`e2e_sandbox` 1、`exec_command` 5、`js_orchestration_security` 1、`llm_flow` 1、`mcp_stdio` 1、`mcp_tls` 1、`memory_retention` 1、`proc_session` 12、`security_attack_031` 3、`session_cli` 1、`session_perf` 1。 |

Ubuntu 的两个独立失败为：`b2_mcp_face_fd_closed` 期望 `tools=1`、实际 `tools=0`，探针含 `cat: /proc/self/fd/10: No such`；`attack06_seccomp_allows_toolchain_blocks_privileged_syscalls` 期望 `clone3=ENOSYS`、实际为 `Invalid argument`。同一安全目标中的 `attack07_setsid_daemon_reaped_at_turn_end` 通过。

Windows 与 macOS 的完整失败用例和断言见各自 Actions 日志。本次没有重试、修复这些独立失败，也没有把 `continue-on-error` 当作平台测试通过。

### 验收结论与工作区状态

- 本次 JS worker 测试前提修复已在新 SHA 的 Ubuntu Actions 上通过四个目标用例；隔离子进程不是零用例运行，环境前提断言和原行为断言均已实际覆盖。
- 038 的三平台全量 CI 仍失败：Ubuntu 有 MCP FD 探针和 attack06 失败；Windows 与 macOS 有各自多项失败。故本结果只关闭该提交快照上的四个 JS worker 用例，不关闭 038 整体或其他平台/功能验收项。
- 此节仅为 Actions 运行后的证据更新；提交仍为 `544a8e7db262fad50ddef2b2ffc5faa3c0ef1fb1`，本节保持未提交。没有修改生产代码、测试代码或工作流，也没有再次运行测试。

## B2 MCP 工具发现失败定位（2026-11-07，待一次远端取证）

### 规格与现有 CI 证据

- B2 契约见 [`033-tool-sandbox.md` 的 B1/B2 加固记录](../../specs/033-tool-sandbox.md)：沙箱启动子进程时，用 `close_range(CLOSE_RANGE_CLOEXEC)` 标记继承的 fd ≥3；stdio 0/1/2 保留；普通模式的继承行为不变。现有目标 `sandbox_matrix_probes.rs::b2_mcp_face_fd_closed` 仍断言沙箱分支发现 1 个工具、成功往返、假凭据不可读，普通对照发现 1 个工具且假凭据可读。
- CI [run #82](https://github.com/MrSong9957/agentlab-demo/actions/runs/42364810143) 的 Ubuntu 日志显示目标只失败在 `tools=0`；`started=true`，包装脚本已把 `FDSCAN=cat: /proc/self/fd/10: No such` 写入探针记录。工具数量断言在假凭据内容断言之前失败，所以该次日志没有给出后续 FD 内容判断。
- CI 没有保存 fixture 的 `tools/list` 输入／输出、McpManager 的连接错误、MCP server stderr 或 server 退出状态。现有日志只能确认分支进入包装脚本和目录快照为空；不能定位进程启动、启动闸、握手、工具发现中的具体阶段。
- CI runner 安装的是 bubblewrap `0.11.1-1ubuntu0.3`；当前容器为 bubblewrap `0.9.0`。这是已观察到的环境差异，不足以单独证明它导致失败。

### 本地复现

在当前基线 `544a8e7db262fad50ddef2b2ffc5faa3c0ef1fb1`，以 `CARGO_TARGET_DIR=/tmp/038-b2-mcp-target`、`CARGO_BUILD_JOBS=1` 和 `--test-threads=1` 执行精确目标。第一次离线冷构建在 180 秒上限内未完成且未进入测试体；续编后测试实际执行并通过，耗时 0.18 秒。沙箱和普通对照均启动 MCP、发现 1 个工具并完成调用；假凭据仅在普通对照可读，沙箱断言保持通过。日志分别是 `/workspace/target/038-validation/2026-11-07-b2-mcp-fd/initial-compile-timeout.log` 与 `local-exact-pass.log`。

本地两种模式的扫描记录都出现了 `cat: /proc/self/fd/N: No such`，但普通对照仍读出假凭据，沙箱分支没有读出。原脚本枚举的是扫描 shell 的 `/proc/self/fd/*`，随后由 `cat` 子进程读取 `/proc/self/fd/N`；路径中的 `self` 指向 `cat` 自身。因而该错误本身既不证明宿主 fd 泄漏，也不证明枚举竞态。CI 的 `fd=10` 是否为假凭据、shell 扫描时该 fd 指向什么，现有输出没有记录。

结论：本地未复现工具发现失败；本地通过不推翻 runner 上的失败。没有证据支持修改生产 MCP 启动、握手或 FD 收口代码。

### 一次性测试诊断候选

仅修改 B2 测试与其 Python 测试夹具；没有生产逻辑变化：

- 探针记录注入的假凭据 fd 编号，以及 `McpManager` prepare 前后状态、工具数和关闭状态。
- 包装脚本记录启动阶段，并对每个枚举 fd 记录扫描 shell 的 PID 与 `/proc/<shell-pid>/fd/N` 链接目标；保留原有 `cat /proc/self/fd/N` 内容扫描及宿主 FD 标记断言。
- Python 夹具把收到的协议 stdin 和发出的协议 stdout 原样复制到独立文件；服务 stderr 与退出码也写入独立文件。诊断数据不写入 MCP stdout，协议 stdout 仍只由夹具发送的 JSON-RPC 帧组成。
- 即使沙箱场景的 `tools=1` 断言失败，也先执行普通模式正向对照，再报告两边结果。`tools=1`、调用成功、沙箱无标记、普通模式可读四项断言均保留。
- 简化审阅未发现可删除、合并或削弱的 B2 断言。审阅发现缺失的记录文件会被 `unwrap_or_default()` 混同为空文件；现已让每个记录读取明确返回内容或 `read_error`，并为主探针记录保留单独读取错误。只改测试诊断，不改产品逻辑或原断言。

定向检查：Python fixture 语法解析通过；诊断记录读取调整后，经 `cargo test --locked --offline -p terminal-chat --test sandbox_matrix_probes --no-run` 再次编译通过；编译前核实 `pids.current=244`（低于 500），日志为 `/workspace/target/038-validation/2026-11-07-b2-mcp-fd/diagnostic-compile-final.log`。随后以 `--test-threads=1`、60 秒上限运行诊断候选的精确用例；启动前 `pids.current=258`，用例实际执行并通过（1 passed，0.11 秒），日志为 `/workspace/target/038-validation/2026-11-07-b2-mcp-fd/diagnostic-exact-pass.log`。本地仍未复现工具发现失败；沙箱扫描观察到 FD 3 是管道且未读到假标记，普通对照 FD 3 指向假凭据并读出标记，夹具启动和退出状态为 0。通过路径没有打印完整 MCP 帧诊断对象；该对象会在目标断言失败时随 outcome 一并输出。此结果验证诊断候选没有破坏本地正常路径，不能代表 runner 根因已知或 CI 已关闭。

完整候选补丁（含本节验收记录更新）与仅测试代码的诊断补丁分别保存在 `/workspace/target/038-validation/2026-11-07-b2-mcp-fd/candidate.patch` 和 `diagnostic-code.patch`。一次远端取证需要将该测试诊断候选更新到现有 PR 分支；现有 `pull_request` 工作流会启动 Ubuntu、Windows、macOS 全量 job，不更改 workflow，也不需要手动诊断 job。只用 Ubuntu B2 日志判断根因和 FD 断言；其余平台结果不在本问题范围。当前未提交、未推送、未触发 CI；等待用户授权该一次诊断发布与 CI 运行。
