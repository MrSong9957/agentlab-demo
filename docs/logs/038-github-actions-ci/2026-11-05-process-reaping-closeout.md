# 038 worker 回收修复最终验证

日期：2026-11-05
工作区：`/workspace`
分支：`feat/038-github-actions-ci`
基线：`57d5408`
状态：未提交候选。保留候选 A、已有回归和其他任务差异；未合并 B。
范围：JS worker 回收、身份捕获与启动放行竞态，以及直接受影响的 034、016、033、038 回归。

## 验收结论

### 产品自身回收通过

| 路径 | 证据与结果 |
|---|---|
| 正常终止 | `sandbox_worker_terminate_reaps_payload`，mode `reap-normal-final4` 通过。测试持有的 payload pidfd 到达退出态；bwrap 由产品等待后退出。探针没有等待 worker。 |
| pidfd SIGKILL、SIGTERM 均被拒 | `sandbox_worker_kill_denied_reaps_payload_group` 连续三次通过：`reap-denied-final5`、`final6`、`final7`。产品只向已验证的 worker 进程组发送回退信号，返回 `Terminated` 并保存 `CellClosed`。worker、同组后代和 bwrap 均在产品返回前退出。探针未执行 `ADOPTED_WAIT`，`REMAINING_TRACKED=[]`。 |
| pidfd SIGKILL 被拒、SIGTERM 可用 | `sandbox_worker_kill_denied_uses_term_fallback`，mode `reap-term-final2` 通过。payload pidfd 确认退出，产品完成关闭。 |
| pidfd 身份打开被拒 | `sandbox_worker_capture_denied_fails_closed`，mode `reap-capture-final3` 通过。产品不创建 cell、不写入 `CellStarted`，并收尾 bwrap。 |
| pidfd SIGCONT 被拒 | `sandbox_worker_release_signal_denied_reaps_unreleased_payload`，mode `reap-release-final2` 通过。产品未发送 init 帧，不创建 cell，并确认监督进程退出。 |
| 身份快照期间 worker 退出 | `payload_identity_capture_rejects_exit_during_snapshot`，mode `reap-lib-identity-final4` 通过。屏障在 `/proc` 快照后终止目标；捕获返回 `NotFound`，已登记的同一 pidfd 保留并显示退出。 |
| worker 入口尚未就绪 | `worker_entry_is_not_ready_before_sigstop`，mode `reap-lib-ready-final4` 通过；`sandbox_worker_entry_self_stops_before_init`，mode `reap-entry-gate-final2` 通过。前者验证非停止态不能作为就绪态；后者验证真实 worker 在读取 init 帧前到达 T 状态。 |
| 取消在途叶子 | `sandbox_worker_cancel_inflight_leaf`，mode `reap-cancel-final3` 通过。worker/bwrap 终止，叶子收到取消。 |
| 034 CPU 限额 | `cpu_limit_kills_spin_worker_host_continues`，mode `reap-cpu-final2` 通过。约 5.37 秒触发限制；测试确认 worker pidfd 退出，后续 cell 完成。 |
| 034 墙钟限额 | `wall_clock_limit_terminates_pending_sandbox_worker`，mode `reap-wall-final2` 通过，耗时 120.15 秒。测试确认 `WALL_TIME_LIMIT`、payload pidfd 和 supervisor pidfd 退出；探针没有等待 worker。 |

双重 pidfd 拒绝回归同时记录具体进程组：第一次运行的 worker 是 PID 12、start time `18942021`、PGID/SID 12；同组后代是 PID 13、start time `18942022`、PGID/SID 12；bwrap 是 PID 11、PGID 11；主进程 PGID/SID 4；独立会话哨兵是 PID 8、PGID/SID 8。用例在发信号前逐项断言 worker 组不包含 bwrap、主进程或哨兵，并断言后代属于 worker 组。三次运行都按 PID＋start time 确认 worker 和后代消失，哨兵存活。

### 产品报错，诊断监督者随后清理

| 异常 | 产品断言 | 诊断记录 |
|---|---|---|
| pidfd 与进程组信号都被拒 | `sandbox_worker_all_termination_methods_denied_reports_failure`，mode `reap-all-denied-final3` 通过其失败契约：有界返回 `WORKER_REAP_FAILED`，不保存 `CellClosed`。 | 产品断言后探针记录 `DIAGNOSTIC_CLEANUP_AFTER_PRODUCT_ASSERTION`，再对 payload PID 11 和 bwrap PID 10 执行 `ADOPTED_WAIT`。这不是产品回收通过。 |
| bwrap 在放行后被外力杀死 | `sandbox_worker_supervisor_dies_reports_failure`，mode `reap-supervisor-final2` 通过：返回 `WORKER_REAP_FAILED`，不保存 `CellClosed`。 | payload PID 9、start time `18961890` 在测试进程退出后由外层 subreaper wait。该 wait 不计为产品回收。 |
| bwrap 在放行前、载荷已定位后被外力杀死 | 新增 `gate_confirm_supervisor_dies_before_release_does_not_report_released`，mode `reap-lib-gate-supervisor-final7` 通过。屏障内杀死 bwrap；确认函数在有界时间内返回 `LeaderGone`，不返回 `Released`。`spawn_cell` 只在收到 `Released` 后才写 init 帧；`LeaderGone` 分支返回启动错误。 | bwrap 由测试中的外力模拟器 wait。若 payload 仍活，测试只在确认结果断言后才用已固定 pidfd 清理；最终探针记录 payload PID 302 的 `ADOPTED_WAIT`。这不是产品回收通过，也没有新增产品等待归属要求。 |

失败路径按产品错误和清理结果分开验收。产品错误不保存成功关闭事件。外部 subreaper 清理只用于测试收尾，不改变产品断言。

### 尚未验证

- 未运行 038 全量测试、L3、clippy 或 rustfmt。用户要求跳过 L3；本轮使用指定的最小回归。容器未安装 clippy/rustfmt。
- 外力终止 bwrap 或所有信号被拒时，产品不能保证自己 wait 孤儿载荷。本轮按原契约验证有界报错、不误报成功、不误放行；没有增加全局 subreaper、helper、PID 1 或容器配置，也没有改变等待归属。
- 038 整体仍未完成；本记录只覆盖上述回收路径和直接受影响的验收。

## 实现与因果闭合

- `WorkerCleanup::capture_and_verify_payload_identity` 先打开并保存 payload pidfd，再读取 `/proc` 身份快照。快照后复查同一 pidfd。身份检查期间退出时，失败清理仍使用已登记的句柄。
- 启动确认回调检查 PID namespace init、父 PID、start time、入口标记、`--internal-js` 参数、进程组和 session。入口标记出现但进程尚未停止时，产品通过固定 pidfd 发送 SIGSTOP 并继续检查。产品只在 T 状态和身份检查通过后记录进程组并发送 SIGCONT。
- `spawn_cell` 在确认返回 `Released` 后才写 init 帧。用户 JS 不能在身份检查、入口停止或监督进程异常期间收到源码。
- 正常终止使用固定 payload pidfd。pidfd SIGKILL 被拒时，产品先尝试 SIGTERM。若回退到组信号，目标是已验证且独立于 bwrap、主进程和其他会话的 worker PGID。
- 产品不杀 bwrap 来回退 worker 信号。worker 退出后，产品继续等待 bwrap 自然退出。只有 payload pidfd 和 bwrap 正常退出都得到确认，产品才保存 `CellClosed` 并返回 `Terminated`。
- `kill_worker_group_and_confirm` 对组信号失败、payload 仍活、bwrap 被信号终止或等待超时均返回错误。错误路径不保存成功关闭事件。
- 新的 pre-release supervisor test 使用真实 bwrap 和 payload 屏障，验证 `confirm_sandbox_with_release` 返回 `LeaderGone` 而不是 `Released`。调用方的 `LeaderGone` 分支不写 init 帧，并返回启动失败。

## 016 `sh` 接管现象

`c13_terminate_keeps_016_process_handle_alive` 在 `reap-c13-final3` 通过，016 句柄仍可续接。探针随后记录测试创建的 `sh` PID 17、start time `18946274` 被外层 subreaper wait。`gate03_released_runs_and_terminate_satisfies_016` 在 `reap-gate03-final2` 通过；探针也记录 016 测试 shell PID 13、start time `18963823` 被外层 subreaper wait。确认取消用例 `reap-lib-gate-cancel-final4` 另记录一个嵌套 bwrap 子进程 PID 21 的诊断 wait。

这些 shell 和嵌套 bwrap 来自 016 `ProcessManager`／通用启动闸测试，不是 JS worker。C13、gate03 的 016 句柄断言均通过。本轮没有改变这些进程的 wait owner；将 shell 接管现象留作 016 后续调查，不纳入 JS worker 回收结论。

## 最终串行简化检查

检查范围：本轮 `runtime.rs`、`worker.rs`、`sandbox.rs`、`proc_session.rs` 与 JS orchestration、启动闸回归。复用、质量、效率、测试价值和注释五组检查均已覆盖；等待逻辑修正后，复用、质量和效率检查再次检查了最终 helper。

- 采纳：将 pidfd 拒绝后的 SIGTERM 和进程组回退提取为 `recover_after_pidfd_kill_denied`，减少主终止路径的嵌套。
- 采纳：身份捕获通过 `WorkerCleanup` 方法先登记句柄；同一 pidfd 用于快照复核与失败收尾。
- 采纳：把放行状态改为 `SandboxRelease::{Pending, Released}`，避免 bool 含义依赖注释。
- 采纳：用 `rsplit` 读取 argv 最后一个非空参数，去掉一次性临时 `Vec`。
- 采纳：修正 timeout 测试 fixture，在 spawn 时创建独立进程组，消除 exec 后再 setpgid 的竞态。
- 保留：双重 pidfd 拒绝、正常终止、受控后代、独立会话哨兵、身份快照和入口状态回归。它们直接证明本轮契约。
- 跳过：未将 bwrap 2ms 有界轮询改成额外 pidfd。该优化只减少低频等待开销，会增加句柄和状态管理。
- 跳过：未改 016 `sh` 接管问题；它不属于 JS worker 回收路径。
- 最终加入的 pre-release supervisor regression 用真实 bwrap、payload pidfd 和 `LeaderGone` 结果证明边界。若 payload 仍活，测试仅在产品结果断言后发诊断信号；随后由外层探针 wait。该收尾不计作产品回收。早期 test-only 屏障失败日志保留，不作为通过证据。

### 最终差异对应性收口（2026-11-05）

范围只含本轮 worker 回收相关代码和回归：`runtime.rs`、`worker.rs`、`sandbox.rs`、`proc_session.rs`、`tests/js_orchestration.rs`，以及本记录列出的对应规格。`proc_session.rs` 与 `sandbox.rs` 还包含既有方案 A 和其他任务差异；本次检查没有把这些差异纳入 worker 回收简化范围，也没有覆盖或回退它们。

结论：本轮最终差异无需进一步简化。上文记录的五组检查、等待 helper 修正后的复查、已采纳修改和跳过项仍适用于最终代码。没有因本次收口修改生产逻辑或测试逻辑。

对应依据：最终 worker 回收实现位于 `runtime.rs` 的 `capture_and_verify_payload_identity`、`inspect_worker_entry`、`recover_after_pidfd_kill_denied` 和 `wait_for_payload_exit_and_bwrap_wait`；其最终双重 pidfd 拒绝三连是 `reap-denied-final5/6/7`。pre-release 回归的最终测试源码是 `proc_session.rs` 中的 `gate_confirm_supervisor_dies_before_release_does_not_report_released`，最终 mode `reap-lib-gate-supervisor-final7` 编译并通过。该 mode 之后只更新了规格和本记录；生产差异没有变化。CPU、墙钟、正常终止、取消、C13、身份快照和入口状态证据见下节索引。没有重跑这些检查。

本轮候选文件索引：

- 生产与入口：`src/app/src/js_orchestration/runtime.rs`、`src/app/src/js_orchestration/worker.rs`、`src/app/src/sandbox.rs`。
- 相关测试：`src/app/src/proc_session.rs`、`src/app/tests/js_orchestration.rs`。`src/app/tests/exec_sandbox.rs` 的 gate03 是复用的既有回归，不作为本轮新增修改。
- 规格与收口记录：`docs/specs/034-javascript-tool-orchestration.md`、`docs/specs/038-github-actions-ci.md`、`docs/logs/038-github-actions-ci/2026-11-05-process-reaping-closeout.md`。
- 原始串行验证输出：`target/038-validation/reap-probe-20261105-<mode>.log` 与 `target/038-validation/reap-probe-test-20261105-<mode>.log`。本轮没有生成新的测试输出。

## 验证命令与证据

工作目录均为 `/workspace`。每个 mode 单独串行运行；探针先停住受控子进程并取得 pidfd，再记录 `CONTROL_READY` 后放行。门禁阈值为 500；所列运行的起始 cgroup task 数为 221–226，均低于门槛。探针逐个记录 PID、start time、PGID、SID 和 adopted wait，不以全局线程数相同作为判据。

- JS／security／exec 集成和指定 lib 用例使用 `target/038-validation/reap-probe.py`，外层命令为 `timeout -k 15 60 python3 target/038-validation/reap-probe.py <mode>`；需要编译的 lib/build mode 使用 `timeout -k 15 240 ...`。
- 120 秒墙钟用例使用 `timeout -k 15 160 python3 target/038-validation/reap-probe.py reap-wall-final2`。用例自身运行 120.15 秒，探针期限为 140 秒。
- JS orchestration 编译：`reap-build-final11` 运行 `cargo test --locked --offline --test js_orchestration --no-run`，退出码 0。
- 双重 pidfd 拒绝三连：`reap-denied-final5`、`reap-denied-final6`、`reap-denied-final7`。每次测试通过，产品返回前完成 worker 和后代清理；每次 `REMAINING_TRACKED=[]`，没有 `ADOPTED_WAIT`。
- 取消、C13、CPU、墙钟：`reap-cancel-final3`、`reap-c13-final3`、`reap-cpu-final2`、`reap-wall-final2` 均通过其单项断言。
- 启动闸取消、超时、放行后终止和 pre-release supervisor death：`reap-lib-gate-cancel-final4`、`reap-lib-gate-timeout-final4`、`reap-gate03-final2`、`reap-lib-gate-supervisor-final7` 均通过。
- 身份、就绪、入口自停、信号回退及失败闭合：`reap-lib-identity-final4`、`reap-lib-ready-final4`、`reap-entry-gate-final2`、`reap-term-final2`、`reap-release-final2`、`reap-capture-final3`、`reap-all-denied-final3`、`reap-supervisor-final2` 均通过各自预期断言。
- 关闭后探针确认每个最终 run 的受跟踪进程均消失。双重拒绝三连、CPU 和墙钟路径不需要诊断器 wait。失败路径的诊断 wait 已按上表分类。

raw stdout/stderr 与进程快照保留于 `target/038-validation/reap-probe-20261105-<mode>.log`、`target/038-validation/reap-probe-test-20261105-<mode>.log`。最后一次 pre-release 通过记录为 `reap-probe-20261105-reap-lib-gate-supervisor-final7.log` 与 `reap-probe-test-20261105-reap-lib-gate-supervisor-final7.log`。

早期 `reap-denied-red1` 证明旧回退错误地终止 bwrap，导致产品无法 wait payload；`reap-denied-final4` 暴露产品把 bwrap 正常退出竞态误判为监督进程过早退出。当前 helper 等待 payload pidfd 与 bwrap 自然退出后，最终三连通过。pre-release 测试的 `final1` 至 `final6` 是夹具迭代，不作通过证据；`final6` 证实外力杀死 bwrap 后 payload 可能仍活，探针当时记录 PID 302、start time `19140113`。该 PID 在下一次检查时已不存在。`final7` 在产品结果断言后按 pidfd 条件清理，并通过且 `REMAINING_TRACKED=[]`。所有过渡日志保留于 target。

此次验证没有运行 L3、全量测试或其他失败用例。没有提交、合并、推送、触发 CI、部署、重启或重建容器，也没有操作凭据。
