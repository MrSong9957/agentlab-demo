# 安全修复 N1–N5（2026-10-10）

> 对象：docs/security/reports/2026-10-09-backend-security-2.md 发现 N1–N5。方法：每项真实业务
> RED→最小修改→GREEN（编译/夹具错误不计 RED，如实记录两处夹具修正）。基线（本轮实测）：
> `cargo check` 0 警告（退出码 0）；`cargo test --workspace --no-fail-fast` **214 通过/0 失败**（0）。
> 修复轮终态：223/0/0。**验收轮终态：`cargo check` 0 警告（0）；全量见文末**（含验收轮补强）。
> 数量核对修正（验收轮）：214→223 曾报"新增10＋改名1"，与 +9 矛盾——经 `-- --list` 前后集合程序比对，
> 根因是 **n2a 测试在 p01 重写的替换区间中被意外删除**（枚举差异无 n2a、exec_command 套件 15≠16），
> 且此前重放脚本对 n2a 的 grep 匹配了"0 passed 的 ok 行"属**假阳性**。验收轮已恢复 n2a（断言与原版一致）
> 并把重放判定改为"1 passed 且非 0 filtered"。最终口径：**新增 11（原 10 项修复测试（含恢复的
> n2a）＋验收轮新增 n2b2）＋改名 1（p01，业务断言保留于改名后测试，净增 0）＋删除/合并 0**，
> 214＋11＝225。
> 全程假 Key、本地 mock、隔离目录；未提交未 push。N6/N9 未在本轮范围。

## N1 CR 过滤【低→已修】
- **改动**：`src/display.rs::is_dangerous_control` 补 `'\r'`（模块白名单本就仅 \n/\t）。
- **原因**：CR 可做终端行首覆盖伪造（"正常文字\r伪造文字"），三条输出路径全中。
- **验证**：RED＝`display::tests::carriage_return_stripped_all_paths`（修复前 `left` 含原始 CR）；
  GREEN＝该单测＋真实 CLI 三路径 `llm_flow::n1_real_cli_three_paths_no_raw_cr`（回复/工具通知 id 含
  CR/401 错误消息——stdout 零 0x0D 字节、正常文字与 Tab 保留、**下一请求中 tool_call_id 仍为原始含
  CR 字符串**＝显示过滤未改写协议数据）。命令：`cargo test --lib display::tests`、
  `cargo test --test llm_flow n1_real`。
- 夹具修正（如实记录）：通知行可见前缀断言初次把 take(8)（含被剥 \r）误算为 8 字符，修正为 7。

## N2 超时上限与 REPL 取消【中→已修】
- **改动**：`src/exec.rs` 新增 `MAX_TIMEOUT_MS=60000`＋`bounded_int`（timeout_ms/max_output_chars
  共用，超限拒绝不调参）；`src/turn.rs::run_tool_turn` 增取消参数（轮首复位、批内未执行调用
  按工具返回对应结果结构记录"未执行"——exec＝cancelled 形态、apply_patch＝ok=false＋
  state_known=true＋未执行原因（补强节 n2b2 定型）、取消即结束本轮不重发）；`src/main.rs` 仅工具轮期间
  安装 SIGINT handler（`TOOL_CANCEL` 原子标志，只置位），轮毕恢复 SIG_DFL——001 的动画/提示符
  Ctrl+C 语义不变。kill_group/取消标志均复用既有机制。
- **验证**：RED(a)＝`exec_command::n2a_timeout_upper_bound_rejected_before_start`（修复前 60001
  竟 Completed）；RED(b)（n2b）原始 PTY 证据不足——取证时夹具尚无控制终端，\x03 不产生信号，
  属夹具缺陷非业务失败（教训保留：**先修夹具再取证**）；合格 RED 来自修正夹具后的定向变异：
  隔离副本（setsid+TIOCSCTTY）仅断开 SIGINT 接线，^C 以默认 SIGINT 杀死 CLI、n2b 因"取消后
  应报错并返回聊天入口"失败＝业务失败（见补强节第 2 条）。GREEN：60001/u64::MAX 拒绝＋探针
  零副作用；PTY 真实进程＋写盘探针：Ctrl+C 后 3s 内报错并回提示符、CLI 存活（最终 exit 0）、日志
  停止增长、同批第二调用未执行、mock 请求数不自动增长、**下一请求完整携带 assistant 调用＋
  cancelled（执行中）＋cancelled（未执行）两条匹配 tool 结果**、下一轮正常回复。
- `exec_p01_huge_timeout_investigation` 改名 `exec_p01_huge_timeout_rejected_and_cancel_path`
  （新契约：超限拒绝＋合法超时下取消回收，关键断言保留）。PTY 夹具补 `setsid+TIOCSCTTY`（否则
  \x03 无信号——夹具修正，如实记录）。

## N3 批大小上限【中→已修】
- **改动**：`turn.rs` 常量 `MAX_TOOL_CALLS_PER_BATCH=32`；在 assistant 入史与任何执行之前检查，
  超限整批拒绝（历史不污染）。8 次请求上限语义不变。
- **验证**：`turn::tests::n3_batch_size_limit`——33/300 整批拒绝（历史仅原记录＋本轮 user）；
  32 条恰上限全部真实执行（32 个落盘文件、恰 2 次模型请求）。**证据性质说明（如实）：N3 的 RED 并非
  修复前缺陷复现，而是定向变异证明**——该上限检查与 N2b 实现同批落地，故在隔离副本移除上限后取得
  该测试失败（"33 条应整批拒绝：已达本轮模型请求上限（8）"），副本已清理。

## N4 输出与出站体上限【中→已修】
- **改动**：`src/exec.rs` `MAX_OUTPUT_CHARS_LIMIT=100000`（bounded_int）；`src/api.rs`
  `MAX_REQUEST_BODY_BYTES=10 MiB`＋`LimitCountingWriter`（限计量序列化：写满即失败，不物化
  超限缓冲）置于 `post_chat_text`（chat/chat_with_body 共用路径）。
- **验证**：RED(a)＝`exec_command::n4a_max_output_chars_upper_bound`（修复前 100001 竟执行）；
  GREEN＝100001/1e6/2e8 拒绝零启动＋恰 100000 合法（300000 码点截为精确 100000 前缀、truncated、
  排空后 exit 0）。RED(b)＝`llm_flow::n4b_request_body_limit_exact_three_points`（修复前 L+1 被
  真实发送并 Ok）；GREEN＝两遍收敛精确构造 L−1/L/L+1（字节独立核对）：前两者恰一次请求且内容
  完整无损，L+1 零发送＋错误含 "10 MiB"。请求体超限经既有 Err 路径结束本轮（t8 语义：保留历史、
  不自动重发、不截断工具结果）。
- 工具定义同步：`prompt.rs` timeout_ms/max_output_chars 描述标注范围。

## N5 有限读入与生成内容限制【中→已修】
- **改动**：`src/apply_patch.rs` 常量 `MAX_PATCH_BYTES=1 MiB`（参数层、解析前）、
  `MAX_FILE_BYTES=4 MiB`＋`MAX_FILE_LINES=100000`（`logical_lines`＝lines() 语义：CRLF 一行、
  非空无末尾换行尾段一行）；Update 分支 fd 级 fstat 预检＋`take(4MiB+1)` 有限读取＋读后行数
  检查＋生成内容写前字节/行数检查；Add 生成行数检查于创建文件前。O_NONBLOCK/fd 判型/inode
  绑定/写入复核（F2）与 EFBIG 语义不变。
- **验证**：RED＝三个 `n5_*` 测试（修复前全部以"超限竟成功/未拒绝"失败）。GREEN：
  patch 恰 1 MiB 成功（内容逐字节）／+1 拒绝且不创建目标；读入恰 4 MiB（同长度替换不增长）成功／
  +1 字节拒绝且目标字节不变／100000 行成功、100001 行拒绝／尾段计行边界（99999\n＋尾 l＝100000
  成功）；生成内容 100001 行 Add 拒绝不创建、恰 100000 行成功、Update 前项成功＋后项超行拒绝
  （无回滚、清单只含前项、state_known 如实）。回归：FIFO/链接/inode 屏障/EFBIG（d6、
  second_file）全绿；f2 屏障测试用 5 行小文件不受影响。夹具修正（如实记录）：补丁尾标记缺换行、
  4MiB 边界用同长度替换避免自然增长，均为测试构造错误。
- 工具定义同步：apply_patch 描述标注 1 MiB／4 MiB／100000 行。

## 攻击重放（口径拆分，验收轮修正）
- **报告原始 payload 直接重放（通过）**：\r 注入三路径（`n1_real`，含 id/错误路径）；`sleep`＋
  `timeout_ms=u64::MAX`（`n2a` 拒绝＋`n2b` 真实 Ctrl+C 语义）；单批 300 条（`n3` 用例含 300）；
  `max_output_chars=2e8`（`n4a` 用例含 200_000_000）。
- **等价边界验证（非字面 payload，通过）**：请求体 L−1/L/L+1（`n4b`，对应 60MB 出站体场景）；
  4 MiB/100001 行读入与生成边界（`n5_*`，对应 95MB 文件场景）。
- **教训（如实记录）**：验收前的重放脚本曾以"存在 ok 行"判定通过，掩盖了 n2a 被误删的事实
  （空跑也打印 ok. 0 passed）；已改为"1 passed 且非 0 filtered"并全部复跑通过。

## 测试增量（审计记录同步，验收轮定稿）
新增 11（原 10 项修复测试＋验收轮 n2b2；n2a 曾被 p01 重写区间误删、验收轮恢复，断言与原版一致）：
`display::tests::carriage_return_stripped_all_paths`、
`llm_flow::n1_real_cli_three_paths_no_raw_cr`、`exec_command::n2a_timeout_upper_bound_rejected_before_start`、
`e2e_acceptance::n2b_repl_ctrl_c_cancels_tool_group_cli_alive`、`turn::tests::n3_batch_size_limit`、
`exec_command::n4a_max_output_chars_upper_bound`、`llm_flow::n4b_request_body_limit_exact_three_points`、
`apply_patch::n5_patch_text_byte_limit`／`n5_update_read_byte_and_line_limits`／`n5_generated_content_limits`、
`e2e_acceptance::n2b2_mixed_batch_cancel_per_tool_result_shapes`。
改名 1：`exec_p01_huge_timeout_investigation`→`exec_p01_huge_timeout_rejected_and_cancel_path`（新契约，净增 0）。
删除/合并 0。夹具：PTY 会话补 setsid+TIOCSCTTY 与 send_raw。枚举经 `-- --list` 与
logs/2026-10-10-test-inventory.txt 程序比对：**214→225**。修复轮旧结论"214→223、无遗漏"
（曾误报"新增10＋改名1"）为**已纠正的历史错误**——n2a 误删未被当时比对发现、重放 grep 假阳性，
验收轮以枚举差异发现并恢复，详见文首数量核对修正。

## 修复后剩余边界（如实）
- N6 并发授权写竞争（丢一条 grant，fail-safe 方向）与 N9 部分 Key 片段回显——**不在本轮范围**。
- 模型请求阶段（非工具执行期）Ctrl+C 仍为默认 SIGINT（handler 仅工具轮安装）——按 001/006 契约保留。
- 4 MiB/100000 行上限内的内存占用为有界线性（本轮未做倍率测量，不引用未测数字）。
- 未验证项：真实 TLS（环境受限）不变。

## 验收轮补强（2026-10-10 第二批）
1. **混合工具批取消（n2b2）**：`turn.rs::skipped_by_cancel` 改为按工具分形——exec 用 cancelled
   形态、apply_patch 用 ok=false＋state_known=true＋未执行原因。RED：混合测试修复前对 apply_patch
   断言 `ok=false` 得 Null（拿到 exec 形状）→ 修复 GREEN。`e2e_acceptance::n2b2_mixed_batch_cancel_
   per_tool_result_shapes`（PTY）：exec 进程组停止写盘、apply_patch 目标未创建、无自动续请求、
   下一请求完整携带 assistant＋两条匹配 tool 结果（各自契约结构）、下一轮正常、CLI 存活 exit 0。
2. **N2b 合格 RED（补证）**：原 RED 在修正控制终端夹具之前取得（\x03 无信号属夹具缺陷）。补做：
   隔离副本（含修正后 setsid+TIOCSCTTY 夹具）仅断开 SIGINT 接线（不装 handler）→ n2b 因
   "取消后应报错并返回聊天入口"超时失败、输出止于 ^C（SIGINT 默认杀死 CLI＝业务失败非夹具）。
3. **生成内容字节边界（n5 扩展）**：合法读入＋<1MiB 补丁，独立核对生成恰 4 MiB 成功、4 MiB+1
   写前拒绝（目标原始字节不变、changed_files 空、state_known=true）。定向变异：副本仅移除生成
   字节检查（行数保留）→ 测试失败于"生成 4 MiB+1 必须拒绝：ok=true 且已写入"＝业务检出。
4. 数量核对、口径修正见文首数量核对修正与"测试增量"、"攻击重放"节；全部变异副本与进程已清理，主树无故意错误。

## 最终验证（验收轮实测，2026-10-10）

- `cargo check`（容器内 /workspace）：**0 警告，退出码 0**。
- `cargo test --workspace --no-fail-fast`（容器内 /workspace）：**225 通过 / 0 失败 / 0 跳过，
  退出码 0**。套件分布：lib 87＋apply_patch 35＋e2e_acceptance 11＋exec_command 16＋
  llm_flow 53＋prompt_assembly 9＋tool_loop 14（合计 225；定稿时静态 `#[test]` 计数复核一致）。
- 变异副本（/tmp/mut-n2bred、/tmp/mut-n5b）与临时进程已清理；未提交、未 push。
