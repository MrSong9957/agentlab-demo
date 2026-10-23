# 规格：工具调用循环（REPL 最小闭环）

> 状态：已实现（REPL 已接入并按 §5 验证；审批/并行/流式/持久化不在范围；**016 修订：用户轮工具三项、取消不再终止已启动进程；017 修订：用户轮工具四项（＋update_plan）；019 修订：模型图片能力确认（`input_modalities` 含 image）的用户轮条件性加第五项 `view_image`——声明与执行分派共用每轮固定能力快照，能力未知/text-only 时伪造调用不读文件只回能力错误；成功结果携带可选图片载荷入史（`ToolEnd` 通知仍只传文本摘要）；MemoryMerge 合并轮仍不开放**）　更新：2026-10-23（019）
> 母本：[openai/codex](https://github.com/openai/codex) 固定提交 `2fdf047c9631c9ed01a31b62efb7891718a931a8`；[Chat Completions function-calling](https://developers.openai.com/api/docs/guides/function-calling)（不迁移 Responses）

## 1. 目标 & I/O

- **最小闭环**：REPL 输入 → 模型请求（005 组装产物＋`model`）→ 判断 `tool_calls` → 按返回顺序**串行**执行 → 每个调用追加一条 `role=tool` 结果 → 再请求模型 → 无 `tool_calls` 时经 `display` 显示最终回复并结束本轮。
- **输入**：会话历史（内存态，`exit` 即丢弃，不落盘）＋当前用户输入。历史含四类记录：`user`/`assistant` 纯文本、`assistant` 带 `tool_calls`（`content` 保留原始字符串或 null，不统一改写）、`role=tool` 结果；后续用户轮次原样携带，不改写成聊天文本。
- **当前 user 只追加一次**：本轮开始时入历史；后续工具循环请求直接使用已含本轮 user、调用与结果的完整历史（`build_turn_request`），**不再次追加当前输入**；`system` 每个请求恰一条。
- **输出**：终端显示最终回复（或错误）；执行工具前打印单行 `[工具] 名称（id 按字符取前 8 位）`（经现有终端输出过滤）；带调用的 `assistant.content` 暂不显示但原样保留在历史与后续请求中。
- **消息格式（Chat Completions）**：assistant 消息原样保留 `tool_calls:[{id,type:"function",function:{name,arguments}}]`；`arguments` 为 JSON 字符串，解析为 `Value` 后交给工具入口；每个调用对应一条 `{role:"tool",tool_call_id:id,content:结果 JSON 字符串}`，`tool_call_id` 必须匹配该调用 `id`；`content` 为现有 `ExecResult`/`ApplyPatchResult` 序列化 JSON。
- **范围**：仅循环与消息拼装。不新增并行、流式工具调用、审批、自动重试、记忆、压缩或其他工具。016 修订：用户轮白名单为 `exec_command`/`apply_patch`/`write_stdin` 三项（内部记忆合并轮仍为受限 mem 工具，分派层拒绝跨模式工具名）；exec_command 默认可续接，取消当前轮不再自动终止已启动进程。017 修订：白名单增至四项（＋`update_plan`，纯解析无副作用；参数错误按两字段失败结果回传、取消跳过同形，见文末 017 修订注记）。

## 2. 边界 & 风险

- [ ] `tool_call_id` 与调用错位、多条结果顺序颠倒，模型侧关联失败。
- [ ] 工具失败被改写成成功文案；参数错误被当成协议错误整轮中断。
- [ ] 模型请求失败后自动重发，导致已执行工具重复副作用。
- [ ] 无 `tool_calls` 判定失误（如空数组）；上限缺失导致请求-调用乒乓失控。

## 3. 关键判断（为什么）

**复用映射**（母本固定提交；方式：直接复用本地 / 母本适配 / 本地新增）：

| 环节 | 来源 | 函数 / 结构 | 方式 |
|---|---|---|---|
| 请求组装 | 本地 005 | `prompt::build_request`＋两工具定义＋`sent_instructions` | 直接复用 |
| 工具执行 | 本地 003/004 | `exec::exec_command(&Value, None)`、`apply_patch::apply_patch(&Value)`（含各自结果结构与失败字段） | 直接复用 |
| 发送层 | 本地 `api.rs` | URL/鉴权头/重定向拒绝/10 MiB 上限/错误映射（`chat` 现有逻辑抽出共用） | 直接复用；新增接收完整 `{messages,tools}` 并返回**完整响应 JSON** 的入口（`chat_with_body`；`choices[0].message` 由 `turn` 提取），`chat` 原行为不变。该入口在发送前拒绝 `request_params` 携带 `messages/tools/model`（核心字段由程序组装，实测复现过 `tools` 可被覆盖后已封堵） |
| 循环骨架 | [turn.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/session/turn.rs) | `run_turn`／`drain_in_flight`：结果写回对话条目后继续请求 | 母本适配：去 guardian/compaction/MCP/`FuturesOrdered` 并行，单线程串行化 |
| 调用识别与继续标记 | [stream_events_utils.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/stream_events_utils.rs) | `handle_output_item_done`：识别→记录→`needs_follow_up` | 母本适配：改为解析 CC `message.tool_calls`（非空即继续） |
| 调用提取与分派 | [router.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/tools/router.rs)／[registry.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/tools/registry.rs) | `ToolRouter::build_tool_call`、按名分派 | 母本适配：白名单二选一分派，无注册表 |
| 错误回传语义 | [function_call_error.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/tools/src/function_call_error.rs) | `FunctionCallError::RespondToModel`（错误作为结果回传模型） | 母本适配：`role=tool` 消息承载 |
| 历史条目语义 | [models.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/protocol/src/models.rs)／[history.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/context_manager/history.rs) | 调用与结果条目、有序保留 | 语义复用；不引入压缩/快照 |
| 验收方式 | [responses.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/tests/common/responses.rs)／[tool_harness.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/tests/suite/tool_harness.rs)／[apply_patch_serialization.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/tests/suite/apply_patch_serialization.rs) | 捕获后续请求、按 `call_id` 核对回传、真实工具副作用 | 方式复用：本地 `tests/common` mock＋`CARGO_BIN_EXE` 管道驱动（同 e2e_acceptance 模式） |

**历史类型最小调整**：`prompt::HistoryMessage` 扩展为枚举 `HistoryItem`——`Text(HistoryMessage)`（与现文本行为等价，005 验收不回归）、`AssistantToolCalls{tool_calls, content:Option<String>}`（原始字符串或 null 原样保留）、`ToolResult{tool_call_id, content}`；`build_request` 改收 `&[HistoryItem]` 并新增 `build_turn_request(&[HistoryItem])`（循环续请求：只投影历史，不追加当前输入），两者 `system` 均恰一条。**协议校验顺序（先整条后执行）**：先校验整条 assistant 消息与整批调用结构——每个调用须有非空（trim 仅用于全空白判断）的 `id`/`name`、`type=="function"`、字符串 `arguments`，批内 `id` 不重复（按原始值判断）；`id`/`name` **原样保留**（存储、唯一性判断与分派均用原始值，含前后空白的名字按未知工具处理，2026-10-10 审查整改）；任何一项无效→整批不执行、该 assistant 消息不写入历史、本轮按协议错误报错。`arguments` 字符串内的 JSON 无效属**参数错误**：按目标工具失败结构回传（exec：`status="error"` 形态；apply_patch：`ok=false` 形态），循环继续，不与协议错误混淆；未知但非空的工具名按未知工具错误处理。**分派与回传**：名称∉{exec_command,apply_patch,write_stdin} → 不执行，回传 `{"error":"未知工具 …，可用：exec_command、apply_patch、write_stdin"}`；保留既有失败/状态字段，不伪造成功。**请求上限（每用户轮 8 次，常量，无配置）——第 8 次请求的两种结果**：返回最终回复→正常结束；返回合法工具调用→执行并保存全部对应结果后**报上限错误，不发第 9 次请求**，不生成假的 assistant 回复。最后一次请求只包含其发出前已有的结果；第 8 批结果保存在历史，下一用户轮计数重置并原样携带。**失败语义**：模型请求失败（网络/HTTP/解析）→ 本轮报错结束，**不自动重发**、不重复已执行工具，历史保留既有记录，下一用户轮可继续。

## 4. 坑记录 → 实测后同步到个人知识库

- 坑001：`tool_calls` 存在但 `content` 非空——以 `tool_calls` 为准继续循环，`content` 不显示（半截文案误导）。
- 坑002：错误必须走 `role=tool` 回传而非整轮中断——模型可自行纠正参数；仅协议/请求级错误才中断。
- 坑003：请求失败后历史里已有调用/结果——重试入口若重发请求会重复副作用，宁可不重试。

## 5. 预测 → 实测

全部自动化：管道驱动真实 CLI（`CARGO_BIN_EXE`）＋预授权＋本地 mock 返回预设响应序列并捕获每个请求体；真实执行两工具于隔离临时目录。

| 场景 | 预测 / 验收 | 实测 |
|---|---|---|
| 普通回复 | 无 `tool_calls` → 直接显示回复结束；恰 1 请求，含 system/user 与两项 tools | 通过（`t1`：roles=[system,user]，tools=2，model=deepseek-flash） |
| 创建文件 | apply_patch 调用真实落盘，字节正确；第 2 请求含原样 assistant(tool_calls)＋`role=tool`（id 匹配、content=ApplyPatchResult JSON）；共 2 请求后显示回复 | 通过（`t2`：字节 `T2-CONTENT\n`；`[工具] apply_patch（call-cre` 单行） |
| 执行命令 | exec_command 真实输出/退出码 7 出现在回传 content（ExecResult JSON） | 通过（`t3`：status=completed、exit_code=7、output 含 out-T3） |
| 多调用顺序 | 一次返回两个调用按序串行执行（后者可依赖前者产物）；第 2 请求两条 `role=tool` 顺序与 id 一一对应 | 通过（`t4`：第二个调用 cat 到第一个调用创建的文件，输出含 SERIAL-X） |
| 错误回传 | 参数错误/未知工具 → `role=tool` 错误内容、无文件副作用、不伪造成功；循环继续至最终回复 | 通过（`t5`＋单测：status=error/未知工具 error；同批合法调用照常执行） |
| 跨轮历史 | 下一用户轮次请求原样携带此前调用/结果/最终回复＋新 user | 通过（`t6`：roles=[system,user,assistant,tool,assistant,user]） |
| 循环上限 | mock 恒返 `tool_calls` → 恰 8 请求后停止并报上限错误；最后请求含全部已执行结果 | 通过（`t7`：恰 8 请求、副作用恰 8 次、第 8 请求含 7 批结果） |
| 请求失败 | 第 2 请求返回 500 → 本轮报错；请求总数不再增长（不重发）；历史保留已执行记录 | 通过（`t8`：两轮共 3 请求、`[工具]` 恰 1 次、下轮请求含已执行记录） |
| 协议错误 | 响应缺 `choices` → 明确报错结束，不执行任何工具 | 通过（`t9`：报错且无 `[工具]`、恰 1 请求） |
| 非空 content 随调用 | 带 `tool_calls` 且 `content` 非空：content 不在终端提前显示，后续请求原样携带该字符串（不改成 null） | 通过（`t10`：终端无“思考片段-T10”，第 2 请求 assistant.content 原样字符串） |
| 当前 user 不重复 | 连续工具循环的每个后续请求中，本轮 user 消息恰 1 条、system 恰 1 条 | 通过（`t11`：第 3 请求 user=1、system=1、两批结果各就位） |
| 批内后项无效 | 首项合法、后项缺 `id` → 整批无副作用、无效 assistant 不入历史，下一用户轮请求仍合法（system＋user 序列；本轮回复在其后产生） | 通过（`t12`：无副作用、无 `[工具]`，第 2 请求 roles=[system,user,user]） |
| 第 8 批与计数重置 | 恒返调用时恰 8 请求后报上限、无第 9 次；第 8 批结果入历史，下一用户轮重新计数（再 8 次）且首个请求携带此前全部结果 | 通过（`t13`：两轮共 16 请求、两次上限错误、第 9 请求含 8 批结果） |

**验收**：先写失败测试再实现；上述全部通过且检查捕获请求的真实 JSON。不连真实模型、不安排手测；不以模型"聪明地"纠正错误为通过标准，以消息结构与副作用为准。

**实测记录（2026-10-09）**：实现位于 `src/turn.rs`（`run_tool_turn` 循环＋`parse_tool_calls` 整批校验＋`execute_tool` 白名单分派）、`src/api.rs::chat_with_body`（与 `chat` 共用 `post_chat_text` 发送路径，`chat` 原行为不变）、`src/prompt.rs`（`HistoryItem` 三变体＋`build_turn_request` 续请求）、`src/main.rs`（内存会话历史＋REPL 接入，授权门与显示过滤不变）。RED：13 类 E2E（`tests/tool_loop.rs`，管道驱动真实 CLI）对旧实现 12 项行为断言失败（t9 因旧行为恰好报错而通过）；GREEN：13/13 通过＋`turn` 内嵌 3 单测。既有 `e2e_acceptance` s1/s2a 与 `llm_flow` C1 三处断言按 005/006 新契约升级（system 首位＋两项 tools＋跨轮历史原样携带），属契约同步而非放宽。

**加固记录（2026-10-09，回归加固轮）**：补 `t14_eighth_request_final_reply_ends_normally`（第 8 次请求返回最终回复→正常结束，恰 8 请求、不被上限错误吞掉）与 `turn::tests::non_string_content_is_protocol_error_and_history_clean`（非字符串 `assistant.content`→协议错误，历史仅本轮 user 不污染）。敏感性经两个定向变异证明（隔离副本、专用 `--target-dir`）：变异"回复分支内提前报上限"仅 t14 因预期原因失败（t7/t13/t1 对照通过）；变异"非字符串 content 静默字符串化"仅新单测失败。收尾核验 `chat_with_body` 后全量回归 `cargo test --workspace --no-fail-fast`：**190 通过 / 0 失败 / 0 跳过**（78＋31＋7＋9＋42＋9＋14），`cargo check` 0 警告；变异副本已清理、工作区无残留。

**固定边界同步（2026-10-10，N3/N2/N4；016 修订取消语义）**：单批 `tool_calls` ≤32 条——在任何工具执行与 assistant 调用记录入史之前检查，超限（33、300…）整批拒绝、零副作用、历史不污染；每用户轮 8 次请求上限语义不变。REPL 在工具执行期间 Ctrl+C（行式）／TUI Esc：SIGINT／取消键只置取消标志，**停止本次工具等待与同批剩余调用，不自动终止已启动进程**（016：进程仍由会话管理并继续收集输出）；执行中被取消的调用按 cancelled 形态记录且**保留可续接句柄**（`session_id` 为字符串）；未执行调用按各自工具的既定结果结构记录未执行原因（exec_command/write_stdin：cancelled 形态＋空句柄；apply_patch：ok=false＋state_known=true＋未执行原因；未知工具：单 `error` 字段通用结构 `{"error":"用户取消，本调用未执行"}`，不伪装成 exec 结果——2026-10-10 审查整改），每个已记录调用均有匹配 tool 结果；取消后不自动请求模型，历史保留，下一轮取消状态复位。取消状态不跨用户轮；进程会话的生命周期状态独立于该标志（016 §4）。最终模型请求 JSON 超 10 MiB 时本轮结束、零发送、不自动重发、不静默截断历史或工具结果。

**在途请求取消扩展（2026-10-15，010 定点轮）**：TUI 内 turn 运行中 Ctrl+C 的取消此前仅覆盖工具批间与 exec 执行中——**在途模型请求（阻塞 ureq，总超时 180s）不轮询取消标志**，置标志后无可视变化、回复照常到达（010 §6 现场症状根因）。修复位于 TUI 宿主 sender（`src/app/src/tui_host.rs`）：请求移辅助线程执行、等待侧 100ms 粒度轮询 `HOST_CANCEL`；取消＝**停止本地等待**并返回 `Err("用户已取消本轮：已停止等待模型回复（迟到结果将被丢弃）")` 结束本轮（历史保留已发生记录，与既有错误语义一致；文案如实——当前实现未中止底层 HTTP 请求（等待侧不关 socket、不中断/不 join 辅助线程）——请求继续至响应到达或 agent 总超时后辅助线程自然结束，迟到结果经已弃 channel 丢弃，不影响后续 turn/会话/界面（S71/S72 /proc 线程数定点核查）；被弃辅助线程单请求存活 ≤180s，**并存数量无上限**（随重叠取消次数增长、各自独立回收））。`turn.rs`/`api.rs` 零改动；行式路径与 006 既有取消契约不变。e2e S69/S69b/S70/S71（PTY 真实驱动），变异双验证见 010 §6。（2026-10-15 单一职责轮：TUI 内取消**触发键**改为 Esc——驱动统一拦截后仍走同一 `set_cancel_requested`/HOST_CANCEL 链路，本节全部取消边界与语义不变；行式 REPL 的 Ctrl+C 语义不变。Ctrl+C 在 TUI 内改恒退出（退出收尾含置取消＋进程组终止）。见 012 §9）

**016 修订注记（2026-10-22，待批准）**：①用户轮工具三项（新增 `write_stdin` 分派——经进程会话管理器执行，取消／上限语义与 exec_command 一致）；②取消不再终止已启动进程——`exec_command` 执行中被取消返回 `cancelled`＋`session_id`（进程存活），取消发生在启动前则不执行（cancelled＋空句柄＋零副作用）；③循环骨架、整批校验、配对、8 次请求／32 条上限、失败不重放不变。签名新增 `procs: &ProcessManager` 参数（跨轮进程会话管理器由宿主持有）。循环级验证：`tests/proc_session.rs` 工具循环集成三例（跨请求续接、MemoryMerge 拒绝 write_stdin、额度耗尽后下一轮续接）＋ `turn` 内嵌用例适配六字段结果。

**017 修订注记（2026-10-22，已批准交付）**：①用户轮工具四项——新增 `update_plan` 分派（`turn.rs` 纯解析执行，结果固定两字段 `{"ok":bool,"error":string|null}`；参数错误按工具结果回传不升级协议错误，与既有失败语义一致）；②取消后跳过的 `update_plan` 调用返回同形两字段失败（错误沿用 `SKIPPED_BY_CANCEL_MESSAGE`），不产生成功计划快照；③未知工具提示与 `skipped_by_cancel` 形状同步；④每轮 8 次模型请求、单批 32 条调用上限与循环骨架不变（计划调用占用请求额度的语义见 017 §3.2，tool_loop t17 验证）。契约明细以 [017-update-plan](017-update-plan.md) 为准。

**018 修订注记（2026-10-22）**：018 同步问询补充：交互宿主的 `request_user_input` 经可选回调等待明确提交；同批后续调用串行等待，不额外请求模型。取消、退出、通道关闭先保存当前错误并补齐剩余结果，说明真实结束原因。8 次请求/32 条调用及第 8 批保存规则不变，无接口与内部记忆轮不可执行问询。详见 [018](018-user-input.md)。
