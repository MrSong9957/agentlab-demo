# 规格：命令执行（exec_command，默认可续接）

> 状态：**已实现（016 修订，待批准）**　更新：2026-10-22
> 定位：exec_command 工具契约（参数、结果、状态语义）；进程会话生命周期、取消、容量与宿主边界见 [016](016-process-sessions-plan.md)。016 修订前为同步执行规格（2026-10-08 首版、2026-10-10 N2/N4 固定边界）。
> 来源：[openai/codex](https://github.com/openai/codex) 对照固定提交 `d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`；本项目取舍在 016 §3/§7 定案。

## 1. 目标 & I/O

- 目标：执行一条命令；**默认可续接**——本次调用等待 `yield_time_ms` 后返回，进程未结束则返回 `running` 与 `session_id`，同一宿主、同一对话内可凭句柄经 `write_stdin` 查询新输出、发送输入（PTY）或明确终止。短命令在等待窗口内直接完成。
- 工具描述：启动命令并等待一段时间；运行中进程继续执行，凭 `session_id` 续接。

| 输入 | 约束 / 默认值 |
|---|---|
| `cmd: string` | 必填；不能全为空白；按执行环境的 Shell 语法解释（`/bin/sh -c`，运行时固定） |
| `workdir?: string` | 默认会话当前目录；相对路径基于该目录解析；必须是已存在的目录 |
| `yield_time_ms?: integer` | 本次调用等待毫秒数，`250..=30000`，默认 `10000`；越界拒绝。到期只返回（`running`＋句柄），不终止进程 |
| `timeout_ms?: integer` | 可选总运行时限（毫秒）；正整数，**无上限**（016 取消旧 60000 上限）。从成功启动按单调时钟独立计时，到期终止并回收进程组、报告 `timeout`；省略＝不设固定总时限。执行前校验数值→时长→截止时间的可表示性，无法安全表示的值拒绝执行（零命令副作用） |
| `max_output_chars?: integer` | 正整数 `1..=100000`（Unicode 码点），默认 `10000` |
| `tty?: boolean` | 默认 `false`（管道，stdin 关闭，仅支持 Ctrl+C 中断字符）；`true` 分配真实 PTY，允许后续发送文字 |

- 输出（与 `write_stdin` 统一）：`{status, exit_code, output, truncated, error, session_id}`，字段始终存在。
  - `status`：`running / completed / terminated / timeout / cancelled / error`（016 六状态）。
  - `running`：进程仍在运行，`session_id` 为可续接句柄（宿主分配的查找编号，不是操作系统 PID），`exit_code=null`；不等于任务成功。
  - `completed`：自然退出（含被信号终止，`exit_code=null` 不编造 0）；非零退出如实返回，成功须同时满足 `status=completed` 且 `exit_code=0`。
  - `terminated`／`timeout`：明确终止／显式时限到期并已终止回收，与自然完成分开。
  - `cancelled`：本次调用或等待被取消，**不表示进程停止**；仍可续接时保留 `session_id`。取消发生在启动前：不执行命令、空句柄。
  - `error`：参数、启动或交互失败；错误本身不表示进程退出，不因此删除仍有效的会话（`session_id` 保留有效句柄，否则 null）。
  - `output`：本次取走的新输出（stdout/stderr 按到达顺序合并；PTY 为合并通道）；UTF-8 解码，非法字节替换为 `�`；`truncated` 标记缓存或单次上限截断，被省略部分不承诺补发。
- 范围：首版 Linux 本地执行；同一对话跨工具调用、跨用户轮保留进程；不跨宿主重启恢复句柄。命令内 `cd` 不改变下一次调用的默认目录。工作目录不构成权限隔离（坑001，权限由执行环境控制）。

## 2. 边界 & 风险

- [ ] 参数非法（零/负/小数含 1.0、非整数类型）、目录不存在、进程启动失败——error 且无命令副作用。
- [ ] 等待到期进程仍在运行；误把等待当寿命会杀掉长任务/服务。
- [ ] 总时限的数值/时长/截止时间可表示性（本平台单调时钟秒域 i64 下 u64 毫秒全域可表示，全部接受；防御分支存在）。
- [ ] stdout/stderr 大量输出导致内存增长或管道阻塞（1 MiB 头尾缓存＋超限丢弃继续排空）。
- [ ] 命令失败或输出被截断，Agent 却判断任务成功（坑002）。

## 3. 关键判断（为什么）

- 默认可续接（016 定案）：长测试与本地服务需要“先返回、后续查询”；省略 `yield_time_ms` 仍按 10 秒等待返回，旧同步语义被替代，不保留显式开启开关。
- 等待与寿命分开：`yield_time_ms` 只控制返回时机；`timeout_ms` 是独立计时的总运行上限，由后台监控执行，不依赖查询在场，读取输出/发送输入/跨轮/取消均不重置。
- 双流并发读取、按到达顺序合并（坑003）；超限后继续读取并丢弃，内存有界；待取缓存单份 1 MiB，超限保留头尾并标记（016 §4）。
- Shell 固定 `/bin/sh -c`；PTY 模式经 posix_openpt 分配真实控制终端（会话组长自成一组的 setsid 方案，见 016）。

## 4. 坑记录 → 实测后同步到个人知识库

- 坑001：工作目录决定从哪里执行，不构成权限隔离。
- 坑002：有输出不代表成功；`running` 更不代表成功。
- 坑003：等子进程退出后才读管道，可能互相等待，永远退不出来。
- 坑004（016）：`process_group(0)` 后 `setsid` 会失败（已是组长）——PTY 模式只用 pre_exec setsid＋TIOCSCTTY，管道模式只用 process_group(0)。
- 坑005（016）：PTY 行规范有单行缓冲上限，大块输入依赖程序自身切 raw 模式；dd 一类单次短读工具会把短读当整块（验收用 `head -c` 计数）。
- 坑006（016）：等待到期与取消都不等于进程死亡——三者在结果状态上分开呈现。

## 5. 预测 → 实测（016 修订后）

| 场景 | 预测 / 验收 | 实测 |
|---|---|---|
| 非法输入（零/负/小数/非整数超时、等待越界、tty 非布尔、额外参数、缺失参数、坏目录） | `error`；无进程执行、无命令副作用、空句柄 | ✅ `invalid_inputs_error_without_side_effects`：13 组均 error，探针未出现 |
| workdir 解析（绝对/相对/缺省） | 相对基于会话当前目录；缺省即会话当前目录 | ✅ `workdir_resolves_relative_paths`（含子进程隔离场景） |
| 短命令窗口内完成 | `completed`＋真实退出码，不留会话记录 | ✅ `short_command_completes_inline_without_session` 等 |
| 双流合并＋退出码 7 | 两行都在输出中；`completed`、`exit_code=7` | ✅ `stdout_stderr_merged_exit_code_preserved` |
| 长命令返回句柄续接 | running＋句柄；查询取新输出至终态；终态取走后句柄失效 | ✅ `running_returns_session_process_continues_query_completes` |
| 显式总时限 | 无查询也到期终止；日志停止增长；`timeout` 无退出码 | ✅ `explicit_timeout_kills_without_any_query`、`timeout_kills_process_group_and_stops_writes` |
| 读取不重置时限 | 查询后死亡时刻仍＝启动＋时限 | ✅ `read_does_not_reset_deadline`（计时区间） |
| 大于旧上限的时限 | 60001、u64::MAX 接受（本平台可安全表示） | ✅ `n2a_timeout_upper_bound_removed_by_016`、`timeout_bounds_accept_large_values` |
| 取消 | cancelled＋保留句柄；进程继续；terminate 后停止 | ✅ `cancel_keeps_process_then_terminate_stops_writes`、`cancel_wait_keeps_process_alive_and_queryable` |
| 输出截断（码点/缓存） | ≤上限、truncated、确定性前缀、超限后继续排空 | ✅ `oversized_output_truncated_to_char_cap`、`discard_after_cap_keeps_child_running_to_completion`、`oversized_pending_stays_bounded` |
| UTF-8 跨块/非法字节 | 跨边界无损重组；非法替换 U+FFFD | ✅ `exec_b05_utf8_multibyte_across_read_boundary`、`invalid_utf8_bytes_replaced_end_to_end` |
| 并发进程组隔离 | A 超时终止不影响 B | ✅ `exec_b07_concurrent_groups_isolated` |
| cmd 含 NUL | 启动失败 error，无副作用 | ✅ `exec_e03_nul_in_command_fails_to_start` |
| cd 不粘滞 | 后续调用仍在会话当前目录 | ✅ `cd_inside_command_does_not_stick` |

验收：先写无效输入的失败测试再实现（016 实施轮按 TDD＋可逆变异证明检出力，见 016 §8）；全部自动化，不依赖人工手测。

### 测试加固与局部精简补充（2026-10-22，已验证）

仅调整 [exec_command.rs 集成测试](../../src/app/tests/exec_command.rs)，不改变 §1 的工作目录、输出和进程契约；上表既有运行记录不作为下列新增条件已通过的证据。

| 落点 | 验收与保持断言不变的反证预测 | 状态 |
|---|---|---|
| `cd_inside_command_does_not_stick` | 在独立临时会话目录、同一 `ProcessManager` 内连续执行 `cd 目标目录 && pwd` 与缺省 workdir 的 `pwd`；两次均自然完成、退出码 0，输出分别精确等于目标目录与原会话目录。使用现有子进程隔离 cwd，不能靠 `Cargo.toml` 存在性推断目录。临时令缺省目录错误沿用上次目标目录，第二次目录等值断言必须失败；恢复后通过 | ✅ 已验证（反证 H6 检出，见 [加固记录](../records/2026-10-22-test-hardening-round.md)） |
| 删除 `discard_after_cap_keeps_child_running_to_completion` 的前置证明 | 保留 `min_output_cap_truncates_at_codepoint_boundaries` 的 Unicode 1/2/3 边界及 100 KB 超限仍完成、退出码 0、精确输出、截断标志；保留 `exec_b04_dual_stream_mb_drain_after_cap` 的双流 MB 级排空、精确码点数及合法内容。两者均经真实 `run` → `ProcessManager::start`。先证实“超限后停止读取”会使保留用例在完成／退出码／有界收尾检查失败，恢复后通过，才能删除原用例 | ✅ 已验证（反证 D3：停止排空→退出码 141／断言 RED；原用例已删） |

P01 的极大时限合并条件见 [016 §6.1](016-process-sessions-plan.md#61-测试加固与局部精简执行准备2026-10-22)；普通大值、取消保活、整组终止均保留。短小 ASCII 截断用例不纳入本次删除。

## 固定边界（2026-10-22，随 016 修订）

- `timeout_ms`：正整数，**无固定上限**；旧 60000 毫秒上限与“默认 10 秒执行超时”语义取消——默认即不设固定总时限，等待默认 10000 毫秒只控制返回。
- `max_output_chars`：`1..=100000`（Unicode 码点）不变；缓存上限单份 1 MiB（头尾保留，016 §4）。
- 取消语义：REPL 执行期间 Ctrl+C（SIGINT 只置取消标志）**不再终止已启动进程**——停止本次等待与同批剩余调用（未执行调用按 cancelled 结构记录、空句柄），已启动进程仍由会话管理（016）；退出宿主时清理全部。
- 结果统一六字段；旧五字段同步结果不再保留特殊分支，旧调用方按新契约适配。
