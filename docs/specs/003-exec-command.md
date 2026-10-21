# 规格：同步命令执行（exec_command）

> 状态：已实现（2026-10-08）　更新：2026-10-08
> 定位：参考 Codex 编写的 MVP 规格；接口取舍见第 3 节。
> 来源：[openai/codex](https://github.com/openai/codex) · [初始工具定义](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/tools/handlers/shell_spec.rs#L24-L122) · [同步版本](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/tools/handlers/unified_exec/exec_command.rs#L478-L506)。源码固定在提交 `2fdf047c9631`。

## 1. 目标 & I/O

- 目标：执行一条命令，返回输出和退出状态；用于读文件、搜索代码、列目录、跑测试。
- 工具描述：在指定目录同步执行命令；完成、超时或取消后返回结果。

| 输入 | 约束 / 默认值 |
|---|---|
| `cmd: string` | 必填；不能全为空白；按执行环境的 Shell 语法解释 |
| `workdir?: string` | 默认会话当前目录；相对路径基于该目录解析 |
| `timeout_ms?: integer` | 正整数；默认 `10000` 毫秒 |
| `max_output_chars?: integer` | 正整数；默认 `10000` 个 Unicode 码点，即字符计数单位 |

- 输出：`{status, exit_code, output, truncated, error}`，字段始终存在。`status` 取 `completed / timeout / cancelled / error`；`exit_code` 为实际整数退出码，无法取得时为 `null`；`output` 合并 stdout/stderr，按 UTF-8 解码，非法字节替换为 `�`；`truncated` 为是否省略输出的布尔值；`error` 为错误说明字符串，正常完成时为 `null`。
- 范围：单次同步执行；Shell 由运行时固定；关闭 stdin；不用 PTY、`session_id` 或 `write_stdin`。只接受表中参数；命令内的 `cd` 不改变下一次调用的默认目录。

## 2. 边界 & 风险

- [ ] 参数非法、目录不存在、进程启动失败。
- [ ] 命令超时或被取消，子进程仍在运行。
- [ ] stdout/stderr 大量输出，导致内存增长或管道阻塞。
- [ ] 命令失败或输出被截断，Agent 却判断任务成功。

## 3. 关键判断（为什么）

- 同步执行：沿用 Codex 的同步版本思路，先完成一次调用，再继续推理，省去进程会话管理。
- 超时/取消：终止并回收受管理的进程及子进程；仅停止等待会留下后台任务。终止命令不会回滚已发生的文件修改。
- 输出：同时读取两条输出流；只保留合并输出的前 N 个字符，超限后继续读取并丢弃，设置 `truncated=true`，不保证两条流之间的精确顺序。MVP 用字符上限替代 Codex 的 `max_output_tokens`，省去分词器依赖。
- 状态：自然结束均为 `completed`，包括非零退出；成功须同时满足 `status=completed` 和 `exit_code=0`。参数/启动失败为 `error`，且不执行命令。

## 4. 坑记录 → 实测后同步到个人知识库

以下是设计提醒，尚未形成实测经验。

- 坑001：工作目录决定从哪里执行，不构成权限隔离；权限由执行环境控制。
- 坑002：有输出不代表成功；输出截断也不代表后面没有错误。
- 坑003：等子进程退出后才读管道，可能互相等待，永远退不出来。

## 5. 预测 → 实测

| 场景 | 预测 / 验收 | 实测 |
|---|---|---|
| 空命令、零超时、额外参数、不存在的目录 | `error`；无进程执行、无命令副作用 | ✅ `invalid_inputs_error_without_side_effects`：9 组非法输入均 `error`，退出码/输出为空，`touch` 探针文件未出现 |
| 指定目录读取相对路径文件 | 读到该目录中的目标内容 | ✅ `workdir_resolves_relative_paths`：绝对、相对（基于会话当前目录）、缺省 workdir 三种均正确 |
| stdout/stderr 各输出一行，退出码 7 | 两行都返回；`completed`、`exit_code=7` | ✅ `stdout_stderr_merged_exit_code_preserved`：两行均在输出中，`completed`、`exit_code=7` |
| 超时 / 取消带子进程的命令 | 返回对应状态；受管理进程停止，后续不再写文件 | ✅ `timeout_kills_process_group_and_stops_writes`、`cancel_flag_stops_managed_processes`：状态正确且被信号终止无退出码；日志文件 400ms 后不再增长（变异只杀组长不杀进程组时该断言失败，孙进程继续写） |
| 连续输出（含中文/emoji）超过上限 | 返回字符数 ≤ 上限；`truncated=true`；进程正常收尾，缓存有界 | ✅ `oversized_output_truncated_to_char_cap`：恰 200 码点、输出为完整输出的确定性前缀、`truncated=true`、`exit_code=0`；`discard_after_cap_keeps_child_running_to_completion`：200KB 输出超限丢弃后子进程仍正常收尾（证明超限后继续读取）；非法 UTF-8 字节端到端替换为 U+FFFD |

验收：先写无效输入的失败测试，再实现；以上场景全部通过，同时检查真实输出、进程状态和文件副作用。

## 固定边界（2026-10-10 安全修复 N2/N4）

- `timeout_ms`：1..=60000，默认 10000 不变；超限（如 60001、u64::MAX）在参数校验层直接拒绝，不悄悄调整、零进程启动。
- `max_output_chars`：1..=100000（Unicode 码点）；超限拒绝执行。合法上限下输出超过捕获上限仍截断并持续排空管道（原语义不变）。
- REPL 取消：exec_command 执行期间 Ctrl+C（SIGINT 只置取消标志）终止命令进程组并回收；3 秒内返回聊天入口、CLI 存活；已发生结果入史，同批剩余调用不再执行并按 ExecResult 的 cancelled 结构记录未执行原因；每个已记录调用均有匹配 tool 结果；取消后不自动请求模型、不重复已执行工具；取消状态不跨用户轮。
