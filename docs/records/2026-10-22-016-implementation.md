---
title: 016 命令执行与进程交互——实施与验证记录
date: 2026-10-22
status: 二轮验收＋测试加固已合并 main（5f6a860）；证据补齐轮（§11）已批准交付（2026-10-22）
tags:
  - Agent
  - 命令执行
  - 进程会话
  - 016
upstream_commit: d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4
branch: feat/016-process-sessions
base: 9345b5299b204335f5c6dbfe3909cedecd2bf333
---

# 016 实施与验证记录

> 按[016 计划](../specs/016-process-sessions-plan.md)（已批准）实施。计划 §1–§7 为契约；本文记录实施事实、验证证据与剩余风险。调研背景见[命令执行与进程交互调研](2026-10-22-codex-command-execution-and-process-interaction.md)。
> **二轮（同日）**：按用户验收指令补齐三项验收（§9）——可控时间验证、PTY 阻塞写取消确定性验证、终止与回收失败注入；并修正过程记录（§10 过程偏差）与交付范围（§8）。首轮全量 3078/0/3 保留为历史证据，二轮后重新完整验证（§5）。

## 1. 改动清单

| 文件 | 改动 |
|---|---|
| `src/app/src/proc_session.rs`（新增，~730 行含测试） | 进程会话管理器：管道/PTY 启动、监控线程（退出回收＋独立时限）、读取线程（poll＋收尾放行）、`PendingBuffer`（1 MiB 头尾缓存＋增量 UTF-8）、终止/关闭/重置、`interact`（查询/输入/中断/terminate） |
| `src/app/src/exec.rs`（重写参数/结果层） | `ExecArgs`（cmd/workdir/yield_time_ms/timeout_ms/max_output_chars/tty）、`WriteStdinArgs`（session_id/chars/yield_time_ms/max_output_chars/terminate＋互斥与 64 KiB）、六状态 `ExecStatus`、六字段 `ExecResult`（含 session_id）、`cap_output` |
| `src/app/src/prompt.rs` | 三工具定义（exec_command 可续接语义＋write_stdin）；`ToolDisplay::Stdin{session_id, command, duration_ms}` |
| `src/app/src/turn.rs` | `procs: &ProcessManager` 参数（三个 run 函数）；exec/write_stdin 分派经管理器；`skipped_by_cancel` write_stdin 形态（cancelled＋空句柄）；未知工具错误列三项 |
| `src/app/src/main.rs` | 行式宿主持有管理器；/resume 成功 `reset`；退出先 `close` 再排空；运行中工具行＋取消清单展示 |
| `src/app/src/tui_host.rs` | `procs` 字段＋`close_process_sessions`；/resume 重置；Esc 取消的 Failed 消息附会话清单；`stdin_begin_event`（关联原命令）；`tool_end_item` 支持 write_stdin/running |
| `src/tui/src/chat_driver.rs` | `HostHistoryItem::Exec.running: Option<String>`＋运行中 cell 渲染（“仍在运行（会话 sN）”＋write_stdin 提示） |
| `src/app/src/memory/phase2.rs` | MemoryMerge 调用点补 `procs` 参数（内部轮不使用） |
| 测试 | `tests/proc_session.rs`（29 用例）、`tests/exec_command.rs`（按新契约重写 19 用例）、`tests/session_cli.rs`（e18/e19）、`tests/prompt_assembly.rs`／`tool_loop.rs`／`llm_flow.rs`／`memory_inject.rs`／`context_injection.rs` 等（契约同步：三工具、六字段、取消语义）、`src/tui/tests/security_untrusted_text.rs`（运行中 cell 注入）、tui/chat_driver 内嵌（running 字段） |
| 文档 | 003/005/006/007/012 规格修订注记、016 §8 实测、project-lessons §3/§4/§7/§8、AGENTS.md §7 两行 |

无新增依赖（PTY 经既有 libc；`posix_openpt/grantpt/unlockpt/ptsname_r/TIOCSCTTY/TIOCSWINSZ`）。未搬入 Codex 执行框架。

## 2. 事前预测 → 实测（关键行为）

| # | 事前预测（计划 §6） | 实测 |
|---|---|---|
| 1 | 工具先返回（running＋s1），进程继续执行；查询不重新启动（marker 恰写一次）；终态真实退出码；取走后句柄失效 | ✅ `running_returns_session_process_continues_query_completes` |
| 2 | 短命令窗口内完成，不留会话记录 | ✅ `short_command_completes_inline_without_session` |
| 3 | 显式时限无查询也到期终止；日志停止 | ✅ `explicit_timeout_kills_without_any_query` |
| 4 | 读取输出不重置时限（死亡时刻＝启动＋时限±300ms） | ✅ `read_does_not_reset_deadline` |
| 5 | 取消等待不杀进程；标志复位后可查询；随后 terminate 生效 | ✅ `cancel_wait_keeps_process_alive_and_queryable`、`cancel_keeps_process_then_terminate_stops_writes` |
| 6 | 取消不暂停时限 | ✅ `deadline_survives_cancel` |
| 7 | 取消发生在启动前：零副作用＋空句柄 | ✅ `cancel_before_start_skips_execution` |
| 8 | PTY 问答（read+换行）；isatty 检测 | ✅ `pty_interactive_read_answer`、`pty_child_sees_terminal` |
| 9 | 管道拒文字输入但会话可查；Ctrl+C 可中断退出（信号退出无退出码）；忽略中断后 terminate 组终止 | ✅ `pipe_rejects_text_input_session_still_queryable`、`pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process` |
| 10 | 64 KiB 恰过（head -c 收满 65536）；超限整次拒绝、零写入 | ✅ `input_limit_exact_passes_over_limit_rejects_with_zero_write` |
| 11 | 阻塞输入可取消（<3s），进程保留 | ✅ `blocked_input_is_cancellable_and_session_kept` |
| 12 | 64 条满额拒绝不淘汰；终止/查询收尾释放；已退出未取走占容量 | ✅ `capacity_64_rejects_65th_without_eviction`、`exited_undrained_session_still_occupies_capacity` |
| 13 | 增量无重放；结束只返回最后待取部分 | ✅ `incremental_output_without_replay`、`terminal_result_carries_last_output` |
| 14 | 1 MiB 头尾缓存：保留头尾＋省略标记＋取走清空＋跨 take UTF-8 | ✅ `proc_session` 单测 7 例（`pending_over_cap_keeps_head_and_tail_with_marker` 等）＋集成 `oversized_pending_stays_bounded` |
| 15 | 大于旧 60000 上限的时限接受；u64::MAX 在本平台可表示（接受） | ✅ `timeout_bounds_accept_large_values`、`n2a_timeout_upper_bound_removed_by_016` |
| 16 | 句柄不跨实例/重启；管理器 close/reset | ✅ `handles_scoped_to_manager_instance`、`close_kills_all_and_rejects_new_starts`、`reset_closes_old_and_allows_fresh_start`、e19 |
| 17 | 同轮跨请求续接（exec→write_stdin）；记忆轮拒绝；8 次额度耗尽下轮续接 | ✅ `turn_exec_running_then_write_stdin_across_requests`、`turn_memory_merge_rejects_write_stdin`、`turn_cap_exhaustion_keeps_sessions_for_next_turn` |
| 18 | 行式跨轮续接＋退出清理；TUI Esc 保活＋退出清理 | ✅ e18、N2b（`n2b_esc_cancel_keeps_process_cli_alive_cleans_on_exit`）、N2b2 |
| 19 | 运行中 cell 注入零动作（PTY 输出过滤） | ✅ `a7_running_session_cell_payload_is_neutralized` |

**过程偏差声明（2026-10-22 二轮如实更正）**：首轮实施中 proc_session/exec/turn 等核心实现**先行于**对应行为测试编写（非严格 test-first）；测试随后编写并运行，红灯未在实现前逐一取得。首轮以**事后可逆变异**（§3，9/9）证明检出力，该证明不能追认为事前 TDD——计划 §5“按 TDD 先写失败用例”的要求在本轮执行顺序上存在偏差。二轮新增验收按“先保留能复现的失败测试、再修复”（RED→GREEN，§9.2/§9.3）执行；对实现已正确的接缝类补测（§9.1）以定向变异证明检出力，不编造失败历史。

**过程实测修正（如实记录，非事后伪装预测）**：
- 初版测试期望“空查询返回 running”在 `sleep 5` 会话上 5 秒内自然到终态（查询阻塞到终态的语义）——测试改用 `sleep 30`；
- `dd bs=65536 count=1` 在 PTY 上把单次短读当整块（4095≠65536）——坑005，改用 `head -c` 计数验证；
- `seq 1 400000` 在本机 <250ms 完成，首取即取空——测试加 `sleep` 前置保证运行中窗口；
- 本工具链（rustc 1.99）Linux `Instant` 为秒域 i64：`checked_add(u64::MAX ms)` 返回 Some，**“无法安全表示→拒绝”分支不可经 u64 毫秒输入触达**——按平台能力如实验收（u64::MAX 接受），防御分支保留（代码审查＋变异存在性确认）；
- dash `printf` 不支持 `\x` 转义——改八进制。

## 3. 变异证明（可逆破坏，9/9 检出）

方法：`/tmp/mut/work` 隔离副本（tar 复制，排除 .git/target/docs）＋独立 `--target-dir /tmp/mut/target`；每项打补丁→定向测试→期望失败（RED）→还原→复跑 GREEN。原始输出 `/tmp/mut/mutations.log`（复刻于下表）。

| 变异 | 对应用例 | 结果 |
|---|---|---|
| M1 等待到期误写成杀进程（running 返回前 request_kill） | running_returns_session…（查询得 timeout≠completed） | RED exit=101 |
| M2 monitor 不再独立执行时限（deadline=None） | explicit_timeout_kills_without_any_query（得 running≠timeout） | RED |
| M3 读取输出重置计时（cv 提前唤醒顺延 2s） | read_does_not_reset_deadline（死亡时刻越界） | RED |
| M4 取消误杀（cancel 分支 request_kill） | cancel_wait_keeps_process_alive…（“取消后继续写日志”失败） | RED |
| M5 满额淘汰最旧（而非拒绝） | capacity_64…（65th 得 running≠error；s1 被杀） | RED |
| M6 运行中即丢记录（remove 后返回 running） | running_returns_session…（查询得未知句柄） | RED |
| M7 take 不清空缓存（重复返回已取输出） | incremental_output_without_replay（查询重放 ONE） | RED |
| M8 killpg→kill（只杀组长） | timeout_kills_process_group_and_stops_writes（后台孙进程继续写） | RED |
| M9 行式退出漏 close | e18（退出后日志继续增长） | RED |

还原健全性：副本与工作区源文件 diff 一致、定向用例复跑通过。首个判定脚本曾把 cargo 的 "error: test failed" 提示行误判为编译失败，修正分类器后复核——四项重试均为真实测试失败（panic 于目标断言），非编译失败。

## 4. 安全门禁（Security Testing Gate）

| 检查项 | 结论与证据 |
|---|---|
| 句柄归属 | 句柄＝管理器内 HashMap 查找键，非 PID；跨管理器/重启拒绝（`handles_scoped_to_manager_instance`、e19“未知或已失效”）；不回显敏感信息 |
| 资源上限 | 64 条会话满额拒绝（capacity 用例）；输入 ≤64 KiB 整次拒绝零写入（input_limit 用例：超限后 payload 0 字节）；待取缓存单份 1 MiB（单测＋oversized 集成）；读取/监控线程数固定（1–2＋1），查询不留线程（无 per-query spawn；PTY 写入线程随会话生命周期，取消即放弃等待） |
| 内部工具隔离 | MemoryMerge 分派层拒绝 write_stdin（`turn_memory_merge_rejects_write_stdin`——mem 注册表外按未知工具回传） |
| PTY 输出过滤 | 运行中 cell 载荷（CSI/OSC52/nF）零动作（`a7_running_session_cell_payload_is_neutralized`）；行式全部输出经 `display::sanitize_for_terminal` |
| 输入取消 | 阻塞写入手工线程＋取消轮询（`blocked_input_is_cancellable_and_session_kept`，<3s 返回 cancelled＋保留句柄）；部分写入如实说明无法撤回 |
| 进程组终止 | terminate/超时/退出清理一律 `killpg(pgid, SIGKILL)`＋组长 wait 回收（M8 变异证明只杀组长会被检出）；忽略 SIGINT 的程序仍可终止 |
| 退出清理 | 行式/TUI 退出先 close 全部会话再排空（e18、N2b/N2b2 退出后日志停止）；close 逐会话 3 秒有界，宽限后 `readers_may_stop` 放行读取线程（后代持管道不无限等待） |
| 参数校验先于副作用 | 全部非法输入（类型/越界/互斥/超限）在解析层拒绝；截止时间安全性在 spawn 前校验；取消先于启动则不 spawn（探针零副作用用例） |

### 4.1 安全门禁（二轮受影响面增补）

| 检查项 | 结论与证据 |
|---|---|
| 输入取消（强化） | 取消不只“停止等待”，还**停止提交尚未发送的输入分块**（T5：进度稳定且未收满；旧实现 8190→65536 被检出）；已发送部分保留且文案如实说明“无法撤回”；≤3 秒返回（实测 <0.1s） |
| 终止失败（新增） | killpg 注入 EPERM：未确认清理成功不返回 terminated、不伪装“已终止回收”的 timeout；error 与实际状态一致（进程仍在运行、句柄保留可查询可重试）；不误清理其他会话（第二组继续运行）；解除注入后真实终止、日志停止（T3/T4；M12 复核检出） |
| 退出清理 | close() 逐会话终止路径不变（n2b/e18 复跑通过）；kill 失败时 close 按既有“有界收尾＋readers_may_stop 放行”兜底（合成注入场景，计划无“关闭期失败报告”要求，不额外编码行为——登记为考虑后拒绝项） |
| 资源状态 | 每进程线程数不变（监控 1＋读取 1–2＋至多 1 写入线程）；写窗口到期有界返回（`pty_write_window_expiry_returns_running_for_nonreading_program`）；取消后句柄/会话保留（T5） |
| 时间接缝 | 全部 `cfg(test)`（线程局部/静态原子），生产构建零足迹；`cargo check`（非 test）不编译接缝 |

## 5. 验证命令与计数

- 基线（实施前，本工作区）：`cargo check` exit 0；`cargo test --workspace --no-fail-fast`＝50 目标 **3045 通过/0 失败/3 忽略**。
- 候选（实施后，含审查修复）：`cargo check` exit 0（terminal-chat 0 错误；vendored tui 警告为既有）；`cargo test --workspace --no-fail-fast`＝50 目标 **3078 通过/0 失败/3 既有忽略**（新增 33：proc_session 29＋tui security 1＋tui_host 恢复投影 1＋审查前后净增；另 exec_command 重写、session_cli +2、turn/prompt 等为既有用例契约适配）。新增 CLI/PTY 验收（e18/e19/N2b 改写、proc_session PTY 组）全部纳入默认回归。
- 定向复跑：`cargo test -p terminal-chat --test proc_session`（29/29）、`--test exec_command`（19/19）、`--test session_cli -- e18 e19`、`--test e2e_acceptance -- n2b`、`-p tchat-tui --test security_untrusted_text`（12/12）。
- **最终门禁（2026-10-22，/workspace）**：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` 首跑 3077 通过/1 失败——唯一失败 `s44_tui_wheel_keeps_selection_gutter_unchallenged` 为 project-lessons §8 已登记的既有 TUI 滚轮时序抖动（本轮零触碰该路径），隔离复跑立即通过；全量复跑 **51 目标 3078 通过/0 失败/3 既有忽略**（目标数 50→51＝新增 proc_session 测试目标），exit 0。
- 已知抖动：`session_perf::perf_tiers_baseline_driven`（负载敏感，012 §5-A8 既有登记，隔离复跑通过）与 `s44`（上述）。
- 变异：§3（9/9）。

## 6. code-review（两轴并行子代理，2026-10-22）

### 6.1 Standards 轴发现与处理

| 发现 | 处理 |
|---|---|
| 取消判定用 `e.contains("取消")` 反解析消息文本（012 教训重蹈） | **已修**：`turn::CANCELLED_TURN_ERROR_PREFIX` 常量＋`starts_with`（main/tui_host） |
| “已停止当前工作…{id}（{cmd}）”清单拼装在两宿主重复 | **已修**：提取 `ProcessManager::live_sessions_note()`，两宿主共用 |
| `execute_tool` 两个 `Err(e)` 臂手写六字段 Error 结构（同形不一致） | **已修**：改用 `ExecResult::error` |
| `.or(Some(..))` 绕写；`MAX_INPUT_WAIT_MS` 孤悬 impl 后 | **已修**：`unwrap_or_else`；常量移至顶部常量组并补文档注释 |
| main.rs 误改无关注释（P0-2→P2-4）；main.rs:23 过时取消注释 | **已修**：回滚 P0-2；注释按 016 更新 |
| 无注入时钟（时限用例真实睡眠±300ms 容差） | **接受并记录**：到期行为本身是真实进程时序（计划 §6 允许“保留必要的真实进程验收”）；检出力由 M2/M3 变异证明；后续若时限用例出现环境性抖动，再引入可注入时钟 |
| `ToolOutcome.session_cmd`／`live_sessions()` 裸元组（Data Clumps，判断性） | 接受：两处局部使用，收敛类型收益小于改动面（极简主义） |

### 6.2 Spec 轴发现与处理

| 发现 | 处理 |
|---|---|
| 行式未显示 terminated/timeout 实际结束状态 | **已修**：ToolEnd 增打“已明确终止并回收”／“总运行时限到期，已终止回收”（自然完成含非零退出仍由模型回复承载——006 行式纪律不变） |
| **恢复投影把历史 running 渲染为当前活会话 cell**（违 §3.3“恢复历史中的 running 只表示当时状态”） | **已修**：恢复路径 running 记录改按事实文本呈现（命令关联＋已收集输出＋“恢复后进程不再受本会话管理”）；新增回归 `project_history_items_renders_running_as_past_state` |
| **PTY 写入取消分支返回 running＋取消文案**（应为 cancelled） | **已修**：改组 `ExecStatus::Cancelled`＋句柄＋如实说明（原测试经输出等待取消分支通过，未暴露此分支——修复后两分支语义一致） |
| timeout 截止时间在 spawn 前计算（略早于“从成功启动开始”） | **已修**：可表示性仍在启动前校验（零副作用），实际截止时间移至启动成功后计算 |
| `MAX_INPUT_WAIT_MS=10s` 规格未定义 | **二轮已移除**（审查认定其静默截短参数层已接受的等待属“悄悄调整”越界）：写入阶段与输出等待共用同一 yield 窗口，由 yield 本身（≤30000ms）定界 |
| 无其他缺失/越界：范围外项（Windows/远程/重连/缩放/推送/沙箱/审批）确认未引入 | —— |

审查后全量复跑：`cargo test --workspace --no-fail-fast`＝**3078 通过／0 失败／3 既有忽略**（新增恢复投影回归 1 例）。

## 7. 剩余风险与未证明范围

- **跨重启重连／Windows／远程执行／终端缩放／后台推送／沙箱／审批**：范围外（计划 §1），未实现未验证。
- **deadline 安全校验分支**：本平台 u64 毫秒全域可表示，拒绝分支不可达（防御保留）；若未来 Instant 实现变化，行为按“实际可表示范围”如实变化（计划 §3.1 措辞）。
- **后代脱离进程组／宿主被 SIGKILL**：不保证清理（计划 §4 首版边界）；后代继承管道时读取线程宽限后放行，不无限等待。
- **PTY 大块输入**：行规范缓冲上限依赖程序自身 raw 模式；主机侧 64 KiB 上限与阻塞可取消已验证，程序侧行为不代管。
- **满 64 会话并行压测**：单测试并行启动 64 进程（~1s）通过；未做长时间高并发 soak。
- **（二轮登记）写线程生命周期**：取消后未被进程终止收尾的写线程存续至其阻塞写入返回（进程终止→EIO）；数量以输入调用数为上界（每次 ≤64 KiB 待发）。plan §4“每进程读取和等待任务数量固定”针对读取/等待任务，写线程属调用作用域——如实登记张力，不 silently 忽略。
- **（二轮登记）kill 持续失败的监控重试**：注入/真实持续 EPERM 下 monitor 以 10ms 间隔重试至进程死亡；错误已如实外露（error＋句柄保留），不无限阻塞任何调用方（终止 3 秒有界返回）。
- **（二轮登记）`clear_kill_failed` 瞬态窗口**：新终止尝试清旧记录到监控回填之间（≤10ms），并发查询可能短暂看不到失败说明——事实性不受影响。
- **`session_perf` 负载敏感**：既有登记（§5），未新增证据。

## 8. 待批准交付动作（仅暂存本任务文件及对应改动块）

分支 `feat/016-process-sessions`（基于 `9345b52`），合并目标 `main`。用户批准后**仅暂存本任务文件及对应改动块**（逐路径 `git add <file>`，禁止 `git add -A`／“暂存全部”）→ 提交 → 合并 → 删除任务分支。此前不暂存、不提交。

**任务改动清单（拟暂存——本任务产生的文件与改动块）**：

| 类别 | 路径 |
|---|---|
| 新增源码/测试 | `src/app/src/proc_session.rs`、`src/app/tests/proc_session.rs` |
| 任务母版与记录 | `docs/specs/016-process-sessions-plan.md`、`docs/records/2026-10-22-016-implementation.md` |
| 修改（改动块与本任务直接相关，整文件属本任务改动） | `src/app/src/exec.rs`、`lib.rs`、`main.rs`、`prompt.rs`、`turn.rs`、`tui_host.rs`、`memory/phase2.rs`；`src/app/tests/exec_command.rs`、`context_injection.rs`、`e2e_acceptance.rs`、`llm_flow.rs`、`memory_feedback.rs`、`memory_inject.rs`、`memory_lifecycle.rs`、`memory_phase2.rs`、`prompt_assembly.rs`、`session_cli.rs`、`tool_loop.rs`；`src/tui/src/chat_driver.rs`、`src/tui/tests/security_untrusted_text.rs` |
| 规格/文档同步 | `docs/specs/003/005/006/007/012-*.md`、`project-lessons.md`、`AGENTS.md`（仅 §7 两处 016 相关行——原有两行“文档不触发全量测试”修改保留在先，见下） |

**原有修改（非本任务产生，交付时保持原样、不并入本任务提交）**：
- `AGENTS.md` 原有两行修改（§3“仅修改文档不执行代码检查”规则）——本任务在其后追加了 §7 两行 016 相关更新；提交时以 `git add -p` 仅纳入 §7 改动块，或经用户确认一并纳入。
- 未跟踪文件（不属于本任务、不暂存）：`docs/records/2026-10-21-codex-tools-comparison.md`、`2026-10-21-opencode-global-agents-skills-install.md`、`2026-10-21-vault-credential-recovery.md`、`2026-10-22-codex-command-execution-and-process-interaction.md`（调研记录——016 引用其结论；是否随任务提交由用户决定，默认不暂存）。

**测试加固轮追加（2026-10-22 三轮，同一候选）**：七处测试加固＋四项条件删除＋两组合并＋15 项反证＋安全门＋审查，零生产行为改动；全量 `cargo check && cargo test --workspace --no-fail-fast`＝51 目标 **3079 通过/0 失败/3 既有忽略**。新增/修改文件随本候选一并待批：`docs/records/2026-10-22-test-hardening-round.md`（新）、`docs/security/reports/2026-10-22-test-hardening-injection-fingerprint-security.md`（新）、六份规格的“已验证”状态回填（002/003/008/012/013/016——002/008/013 工作区改动含本轮增量，暂存归属与 016 前既有修改一并由用户裁决）、七个测试文件（catalog.rs/proc_session.rs 单测部分、tests/proc_session.rs、exec_command.rs、memory_store.rs、memory_candidates.rs、memory_storage_files.rs、security_untrusted_text.rs、e2e_acceptance.rs）。详见[加固记录](2026-10-22-test-hardening-round.md)。

## 9. 二轮验收（2026-10-22，用户指令三项）

### 9.1 可控时间验证（最小内部接缝，不新增公共配置/时钟框架）

接缝（全部 `cfg(test)`、位于 `proc_session.rs`，生产代码零足迹）：
- `compute_deadline(ms)`——截止时间计算单点。**类型与计算依据（如实说明）**：std 的 Linux `Instant` 可表示范围（秒域 i64，约 ±9.2e18 s）远大于 u64 毫秒域（`u64::MAX` ms ≈ 1.8e16 s），`checked_add` 对全部 u64 毫秒值返回 `Some`——数值防御分支在本平台**不可自然触达**，仅可注入；
- 线程局部 `TEST_DEADLINE_FAILS`——置位后 `compute_deadline` 强制 `None`，验证“拒绝执行”真实路径（非伪造结果）；
- 线程局部 `TEST_DEADLINE_JOURNAL`——记录本线程每次成功启动的截止时间。

稳定验证落点（`proc_session::tests`）：
- `seam_deadline_journal_omission_and_acceptance`：省略 `timeout_ms`＝journal 记 `None`（无固定总时限）；180000（>旧 60000 上限）与 `u64::MAX` 均记 `Some`（接受）；随后查询**不新增、不改变**已记录的截止值。
- `seam_deadline_injection_rejects_execution`：注入计算失败→`error`＋“拒绝执行”、空句柄、探针零副作用。
- 真实进程验收保留：`explicit_timeout_kills_without_any_query`（无查询到期）、`deadline_survives_cancel`（取消后时限仍生效）、`read_does_not_reset_deadline`（查询不重置，计时区间）——后两者即“不暂停/不重置/不延长”的行为级证据；变异 M2/M3（首轮）为其检出力证明。
- 补测检出力变异（本轮，隔离副本＋独立 `--target-dir`）：**M10** `compute_deadline` 忽略注入标志（恒真实计算）→ 注入拒绝用例失败；**M11** journal 恒记 `Some(now)` → 省略＝无时限断言失败。

### 9.2 PTY 阻塞写入取消（确定性，先 RED 后修复）

**缺陷复现（RED，保留于测试历史）**：`tests/proc_session.rs::pty_cancel_stops_further_input_submission`。受控真实 PTY 场景：python3 慢速读端（每 4 KiB 落盘计数＋50ms 小睡，全程约 0.9 s），输入侧就绪信号＝`cnt` 文件 ≥4096 字节（实测写入确在进行且远未完成，不靠固定睡眠猜阻塞位置）；此刻取消。旧实现（整体 `write_all`）实测 RED：读端进度 8190 → **65536**（取消后仍持续提交全部剩余输入，违反计划 §3.2“停止后续写入”）。
**修复（GREEN）**：`InputWriter::write_chunked`——4096 字节分块提交，块间检查中止标志；取消时置位（单块内 syscall 不可移植中断，至多再送达一块）。三连跑通过；稳定值断言改为契约级确定性（取消后进度**趋于稳定且未收满 64 KiB**——旧实现恒收满 65536，可稳定区分）。
核对项落点：3 秒内返回（实测 <0.1 s）＋`cancelled`＋保留 `session_id`；原进程存活可查询；另一会话不受影响；取消后不再提交未发送输入（进度稳定<收满）；已发送部分保留（`data.bin` 长度＝读端进度、非零）且错误文案如实说明“无法撤回”；随后 `terminate` 成功。

### 9.3 终止与回收失败（killpg 边界最小注入，先 RED 后修复）

接缝：`kill_group_checked`——`killpg` 受检入口，`#[cfg(test)]` 静态 `TEST_KILLPG_FAIL_PGD` 按 pgid 精确注入 `EPERM`（不影响并行其他测试的真实终止）。
**缺陷复现（RED）**：注入后 terminate 返回 `Terminated`（“已明确终止并回收进程组”）——清理未确认成功却宣称已终止；时限到期路径同理伪装 `Timeout`（“已终止并回收”）。
**修复（GREEN）**：monitor 使用受检入口；`Failed(errno)` → 如实记录 `kill_failed`、保持 Running（不漏管）、保留请求重试；terminate 快路径读到本次尝试的失败立即返回 `error`（未确认清理成功、句柄保留、可重试）；运行中查询附带“终止失败、进程仍在运行”说明；成功终止/自然退出清除记录；每次 terminate 尝试先清上次记录（陈旧失败不阻塞重试）。
核对项落点（`seam_killpg_injection_terminate_reports_failure_keeps_session`／`..._deadline_not_fake_timeout`）：不返回“已终止”/伪装 timeout（error 状态）；目标进程仍存活且继续写、句柄保留可查询（错误与实际状态一致）；其他会话不受误清理（第二组继续运行可查询）；解除注入后重试真实终止、日志停止。真实进程组终止测试全部保留（`timeout_kills_process_group…`、`pipe_ctrl_c…`、M8 变异）。
说明：解除注入后由仍挂起的**时限**请求完成终止时，终态如实为 `timeout`（事由＝时限到期被终止回收）——测试按此断言。

## 10. 二轮流程与验证

- 顺序：接缝＋复现测试（RED：终止伪装×2、PTY 持续提交）→ 修复（killpg 诚实处理＋分块中止）→ GREEN → 补测变异（M10/M11）→ 受影响面回归 → comprehensive/安全门禁复核（§4 增补）→ code-review → verify:full（见 §5 增补）。
- AGENTS.md/project-lessons 本轮新增内容核对：均为 016 必要相关（模块计数、契约行、lessons §3/§4/§7/§8、记录索引）；未发现无关内容需撤销。

### 10.1 二轮 code-review（两轴并行子代理）与处理

**Standards 轴**：
| 发现 | 处理 |
|---|---|
| `TEST_KILLPG_FAIL_PGD` 全局注入无 Drop 守卫——断言 panic 会把注入泄漏给后续用例（AGENTS §3 测试独立性） | **已修**：`arm_killpg_fail` 返回 RAII 守卫，Drop 自动解除 |
| `interact` 内 4 处 5 字段错误字面量重复（新字段漂移风险） | **已修**：新增 `ExecResult::error_with_session(msg, sid)`，4 处收敛 |
| monitor 终止失败后按 10ms 无限重试 | 接受并记录（真实持续 EPERM 对自有子进程为合成场景；进程死亡即停止；见 §7 增补） |
| seam 测试样板重复／`mod tests` 内冗余 `#[cfg(test)]` | 冗余标注**已清理**；样板重复接受（4 用例、最小） |

**Spec 轴**：
| 发现 | 处理 |
|---|---|
| `MAX_INPUT_WAIT_MS` 静默截短已接受的写入等待（内部新增上限） | **已修**：删除该上限——写等待与输出等待共用 yield 窗口（≤30000ms 由参数层定界） |
| 时限请求挂起后用户 terminate 被静默丢弃（单槽请求）；终态归属未定案 | **已修**：显式 terminate 覆盖挂起的 deadline 请求——用户显式动作优先，终止成功后事由归 terminate（T4 断言随之更新） |
| ESRCH 视同成功终止——发信号前进程已自然死亡时误记 TimedOut/Terminated | **已修**：ESRCH→按自然退出如实归属（`Exited`，回收真实状态） |
| 取消后写线程存留至进程死亡，与“每进程任务数固定”有张力 | 接受并记录（§7）：读/监控任务固定；写线程随输入调用（每次 ≤64 KiB、有界），plan §4 固定任务约束针对读取/等待 |
| `clear_kill_failed` 先清后请求的瞬态窗口（并发查询短暂无注） | 接受（瞬态，下一轮询即回填；不改变事实性） |
| 测试鲁棒性：cnt 非原子读可 panic；`settled > at_cancel` 严格不等可假失败 | **已修**：读取失败沿用上次观测；断言放宽为 `>=`（真实校验是“未收满”与“≥4096 已送达”） |

审查后复跑：lib seam 10/10、集成 30/30（×2）、exec_command 19、session_cli 26、e2e n2b 2——全部通过。

### 10.2 过程坑（二轮，如实记录）

- **陈旧二进制伪故障**：隔离副本中变异后以 `mv .bak` 还原源文件，其 mtime 早于上次变异构建 → cargo 判定无需重编，跑的是**变异后的陈旧二进制**，表现为“还原后仍失败”。`touch` 强制重编后即恢复。教训：变异框架还原后必须 touch（或以内容哈希重编），否则“还原绿”可能失真。本轮 M10–M12 的红灯均基于打补丁后的新鲜构建，有效；该坑只影响还原后的复核跑，已用强制重编复核排除。
- 变异脚本首轮曾把 cargo 的 "error: test failed" 提示行误判为编译失败（首轮 §3 已记），二轮分类器沿用修正版。


## 11. 三轮：最后两项证据核对与补齐（2026-10-22）

**核对结论**：①时间控制——原接缝（compute_deadline 注入/记录）只覆盖参数接受、计算失败拒绝与记录，**未**控制监控线程的实际到期判定（行为用例依赖真实时间±300ms 容差）→ 本轮补齐；②PTY 写阻塞取消——原就绪证据 cnt≥4096 仅证明“输入已开始”，无发送侧背压证据，取消后断言混入内核缓冲排空 → 本轮补齐。

### 11.1 时间控制补齐（monotonic_now 时钟前移，仍无通用框架/公共配置）

- 接缝：`monotonic_now()`（proc_session.rs）——监控线程**到期判定**唯一入口；`#[cfg(test)] static TEST_CLOCK_SHIFT_MS`（RAII 守卫自动复位）可前移读数，稳定驱动真实终止；截止时间**计算**（compute_deadline）仍真实。生产构建零足迹（cfg(test) 不编译）。
- 四例（lib 单测，真实进程＋副作用断言，无时间容差）：`seam_clock_shift_expiry_without_query`（无任何查询，前移即终止——日志停止增长＝进程组死亡；事后查询 Timeout＋句柄失效）、`seam_clock_shift_deadline_survives_cancel`（取消后前移恰过**启动时**截止仍到期——取消不重置/暂停）、`seam_clock_shift_query_and_input_do_not_extend_deadline`（先查询[管道]＋输入[PTY]再前移恰过启动截止——重置缺陷将漏杀，断言失败）、`seam_clock_shift_omitted_timeout_has_no_fixed_limit`（省略 timeout，前移 1 小时后日志仍增长＋查询 Running——旧 10 秒式固定上限会杀，随后 terminate 收尾）。
- 检出力变异（隔离副本＋独立 target-dir，touch 还原纪律）：**M13** 前移被忽略→到期用例 RED；**M14** 为省略会话伪造 10 秒固定时限→省略用例 RED。真实时间验收（explicit_timeout/deadline_survives_cancel/read_does_not_reset 及 M2/M3）全部保留。
- 依据说明（保留）：Linux Instant 秒域 i64 ＞ u64 毫秒域，数值防御分支不可自然触达（compute_deadline 文档注释）。

### 11.2 PTY 写阻塞背压补齐（发送侧真实进度）

- 接缝：`SessionState.input_written`——**真实写线程**每块成功送出后累计（发送侧进度）；`#[doc(hidden)] ProcessManager::session_input_written` 只读观测（非承诺接口）。内核实测（本轮探测）：PTY 主设备无读端仅接受约 11.5–16 KiB 即阻塞——64 KiB 写入配“读端收满 1024 字节后停等释放信号”可确定性制造真实背压。
- `pty_cancel_stops_further_input_submission`（集成，强化重写）：就绪与**阻塞证明**＝读端 cnt 稳定于 LIMIT **且发送侧 input_written 两次采样相等且远小于 64 KiB**（写线程正在等待底层通道接受数据——真实状态，非测试假状态）；此刻取消。取消后**释放读端**恢复消费：若取消未停止提交（缺陷），写线程会随读端继续发送至 64 KiB——正确实现新发送≤一个在途分块（4096）。发送侧与接收侧分别断言：接收侧 cnt 的后续增长只是内核缓冲排空（≥LIMIT、≤发送侧、<64 KiB），不计为新发送。其余核对项（≤3 秒 cancelled＋句柄、进程存活、他话可操作、已发送部分保留＋如实说明、随后查询与终止）全部保留。三连跑通过。
- 检出力变异：**M15**（取消不置中止标志）对旧版就绪设计 UNDETECTED（读端停止时无从提交——设计盲区，如实登记），补“读端释放”阶段后重试 **RED 检出**；还原后 GREEN（副本 touch 重编复核）。

### 11.3 与“测试加固轮”（另一任务，同工作树）的交接事实

- 加固轮在二轮候选上完成七处加固/四项条件删除（含 nonzero_exit_reported_verbatim、terminal_result_carries_last_output、discard_after_cap——**替代覆盖反证 D1–D3 成立**）并经用户批准提交合并：main＝`5f6a860`（016 一二轮＋加固轮全部内容），任务分支已删。本轮核实时发现三个 016 用例“缺失”拟恢复，读取加固轮记录后确认属**有意删除且有替代证明**——按“保护其他任务改动”不恢复，未产生任何覆盖（恢复脚本断言失败原子未写入，已验证计数未变）。
- 当前工作树未提交改动＝本轮（三轮）两文件增量＋AGENTS.md 两处**原有非 016 修改**（项目原则复用句、文档流程句）＋四份非 016 未跟踪记录——均保持原样、不并入本轮交付。
- 加固轮全量基线 3079；本轮在其上净增 4（时钟接缝单测）。

### 11.4 三轮审查与验证

- 受影响 code-review（子代理）：无硬违规；已处理——`session_input_written` 加 `#[doc(hidden)]`＋非承诺注释、删除恒真伪断言改为有界“写入循环确实运行”证明、SEAM_LOCK 处补“带 timeout 会话的单测必须持锁”约束注释；接受并登记——两会话启动时差余量（无实际风险）、`blocked_at>=LIMIT` 冗余断言（保留作意图说明）。
- 受影响安全检查：时钟前移/写入进度均为 cfg(test)/doc(hidden) 观测面，生产行为零变化（cargo check 非测试构建不含接缝）；终止/输入取消安全面不变（11.1/11.2 用例本身即含终止与取消边界）。
- **最终完整验证（三轮后，/workspace，覆盖当前候选）**：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` 单次干净通过——**51 目标 3083 通过／0 失败／3 既有忽略**（加固轮基线 3079＋时钟接缝 4）。日志：/tmp/final3-check.log、/tmp/final3-test.log。clippy/rustfmt 未安装未运行（不计通过）。
