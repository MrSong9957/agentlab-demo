# 规格：最小模型请求组装（Codex 复用版）

> 状态：已实现（组装与序列化已按 §5 验证；发送层接入与工具调用循环未做；**016 修订：工具定义增至三项**）　更新：2026-10-22（016）
> 母本：[openai/codex](https://github.com/openai/codex) 固定提交 `2fdf047c9631c9ed01a31b62efb7891718a931a8`（下表链接均指向该提交；[function-calling 格式](https://developers.openai.com/api/docs/guides/function-calling)）
> 最小组装＝基础提示词（Codex 原文＋明确列出的必要适配）＋工具定义（016 起三项：exec_command、apply_patch、write_stdin）＋用户消息与历史（文本结构投影）。

## 1. 目标 & I/O

- **目标**：组装基础提示词、工具定义、既有文本历史和当前用户消息，交给现有 Chat Completions 发送层（`src/api.rs::chat`）；不迁移 Responses。
- **输入**（三种，不可混配）：①**原始 prompt.md**——固定内置资源（`assets/codex/prompt.md`，构建期 `include_str!` 嵌入，与 Codex `model_info.rs` 加载方式相同，逐字节不改）；②**发送版提示词**——由母本经第 3 节四项适配得到，不可独立配置；③**调用输入**——按时间排列的 `user / assistant` 文本历史（不含系统消息与本轮输入）＋当前用户消息。不提供两份提示词可独立配置的接口。
- **输出**：`{messages, tools}`。`messages` 顺序固定 `system(发送版提示词) → 历史 → user(当前输入)`；`tools` 恰为 `exec_command、apply_patch、write_stdin` 三个 function 定义（016：write_stdin 为进程会话交互入口）。
- **约束**：本轮输入不得全为空白；历史可为空；角色、顺序、原文逐字保留；同一输入产出相同结果。
- **范围**：仅组装并序列化；发送、解析、工具执行、结果回传另立规格。不引入记忆、检索、压缩、多媒体、模板选择系统、AGENTS.md 自动注入或其他工具。

## 2. 边界 & 风险

- [ ] 嵌入母本与固定提交漂移；把"母本一致"与"发送文本＝母本＋适配"混为同一文本。
- [ ] `update_plan`、AGENTS.md 注入等未兑现声明残留，与实际工具白名单冲突。
- [ ] 工具说明只有文字未进 `tools`；声明未实现参数（`tty`、会话等）或自由文本与 JSON 接口混用。
- [ ] Codex 结构到 messages/tools 映射走样：顺序、role、原文被改写；中文、引号、换行损坏；空白输入被误接受。

## 3. 关键判断（为什么）

**复用映射**（方式三档：原文直接复用 / 局部代码复用 / 母本＋必要接口适配；类型写法为**文本投影**，省略未用字段，非完整定义）：

| 复用项 | 固定提交来源 | 类型 / 函数 | 方式 |
|---|---|---|---|
| 基础提示词 | [models-manager/prompt.md](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/models-manager/prompt.md)，加载确认 [model_info.rs#L16](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/models-manager/src/model_info.rs#L16) | `BASE_INSTRUCTIONS = include_str!("../prompt.md")` | 原文直接复用（嵌入母本，逐字节不改） |
| 未开放工具提示处理 | [update_plan_instructions.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/prompts/src/update_plan_instructions.rs) | `without_update_plan_instructions(&str) -> String` | 局部代码复用（纯文本过滤；**017 起停用**——update_plan 工具已接入，指引按母本原文保留，过滤器及其锚定测试移除，见 017 修订注记） |
| exec_command 定义 | [shell_spec.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/tools/handlers/shell_spec.rs) | `create_exec_command_tool*`（`required:["cmd"]`、`additionalProperties:false`） | 母本＋必要适配 |
| apply_patch 定义 | [apply_patch_spec.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/tools/handlers/apply_patch_spec.rs) | `create_apply_patch_freeform_tool` | 母本＋必要适配（适配为 function，见下） |
| 补丁语法 | [apply_patch.lark](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/assets/tools/apply_patch.lark) | `start: begin_patch hunk+ end_patch`（含 `change_move`） | 语法母本；本地为无 Move 子集（004） |
| 本轮用户输入 | [user_input.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/protocol/src/user_input.rs) | `UserInput::Text { text }`（文本投影） | 局部复用（仅文本结构） |
| 消息与历史条目 | [models.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/protocol/src/models.rs) | `ResponseItem::Message { role, content }`、`ContentItem::InputText/OutputText`（文本投影） | 局部复用（文本结构与有序保留语义） |
| 历史语义 | [history.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/context_manager/history.rs) | 有序条目列表 | 语义复用（有序、原样保留），不引入历史管理运行时 |
| 组装结构与函数 | [client_common.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/client_common.rs) `Prompt { input, tools, base_instructions }`；[turn.rs#L1563](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/session/turn.rs#L1563) `build_prompt` | — | 结构母本＋同构最小函数 |
| 请求构造对照 | [client.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/client.rs) `build_responses_request`；[context/base_instructions.rs](https://github.com/openai/codex/blob/2fdf047c9631c9ed01a31b62efb7891718a931a8/codex-rs/core/src/context/base_instructions.rs) | `BaseInstructionsFragment`（`role() = "developer"`） | 仅对照：Codex 把基础提示词作为 developer 消息拼入 `input` 前部（`input.splice(0..0, …)`） |

**基础提示词的必要适配（仅以下四项，逐项说明，不重写整篇；发送文本＝母本＋且仅＋这些适配）**：

1. 剔除 `## Planning` 与 `## update_plan` 节——本地未提供该工具，用 Codex 自带 `without_update_plan_instructions`（它即为此场景设计）。（**修订注记 2026-10-22，017 已实施**：`update_plan` 工具接入（规格 [017-update-plan](017-update-plan.md)），本适配停用——发送版保留母本两节原文，适配仅剩②③④三项；`without_update_plan_instructions` 与其上游锚定测试随之移除，发送文本差分对照仍由 `s2` 承担（独立重建不再剔除两节）。）
2. `prompt.md` 中 apply_patch 旧示例 `{"command":["apply_patch","*** Begin Patch…"]}` 为 shell 包装形态，与本地 JSON 契约冲突；该行最小改写为 `{"patch":"*** Begin Patch…"}`（Codex 无现成处理器，属必要改写）。
3. AGENTS.md 注入声明最小修正：原文称根 AGENTS.md 内容"已随 developer 消息注入、无需重读"——本地不做任何注入，不得声称内容已注入；将该句最小修正为"需要时通过现有 `exec_command` 读取适用的 AGENTS.md 文件"。不新增自动发现或注入模块。（**修订注记 2026-10-20，已实施**：015 规格 [docs/specs/015-turn-context-injection.md](015-turn-context-injection.md) 已实现用户轮上下文注入，本适配随之反转——声明现为与其 §3-5 一致的新句 "The applicable AGENTS.md instructions and environment context are provided in a user message before the current input and don't need to be re-read."，嵌入母本与其余三项适配不变；005 其余契约不因 015 改变。）
4. 其余提及 sandbox/审批升级等保留原文（措辞已限定"视运行配置而定"，不与白名单冲突，列为已知差异）。

**协议映射（Codex 结构 → Chat Completions，保持现有发送层；本地 `system` 首位属接口适配）**：Codex 将基础提示词作为 developer 消息加入 `input` 前部（见对照行）；本地适配为 `messages[0] = {role:"system", content:发送版提示词}`。`ResponseItem::Message` 文本项 → `{role, content:string}`（历史逐条、本轮置末位）。`ToolSpec::Function` → `{type:"function", function:{name, description, parameters}}`。`ToolSpec::Freeform`（apply_patch）→ 适配为 Function（唯一必填字符串 `patch`，004 契约）：为保持本地 JSON 工具契约，不增加 custom 工具支持。`Prompt` 其余字段（`parallel_tool_calls`、`output_schema` 等）不映射、不声明。

**工具定义（016 修订：以本地 003/004/016 契约为准）**：`exec_command` 参数恰为 `cmd`（必填）、`workdir`、`yield_time_ms`、`timeout_ms`、`max_output_chars`、`tty`——默认可续接语义（等待只控制返回；运行中返回 `session_id`）；Codex 的 `max_output_tokens/shell/login` 未实现不声明。`write_stdin`（016 新增）参数恰为 `session_id`（必填）、`chars`、`yield_time_ms`、`max_output_chars`、`terminate`。`apply_patch` 描述复用 Codex 首句"The `apply_patch` tool can be used to edit files."，删除"FREEFORM… do not wrap the patch in JSON"（与 JSON 契约相反）；语法说明不含 Move to。四工具（017 起增 `update_plan`——参数恰 `plan`（必填，元素 `{step,status}`，状态枚举 `pending/in_progress/completed`）与可选 `explanation`，Schema 对齐上游 `plan_spec.rs`）仅出现在用户轮请求面（含带记忆摘要的用户轮）；内部记忆合并轮仍为受限 mem 工具（013，不声明不执行 `update_plan`）。

## 4. 坑记录 → 实测后同步到个人知识库

- 坑001：告诉模型"有这个工具"≠提供可调用的结构化接口；以组装后的 `tools` 字段为准。
- 坑002：复用成熟方案必须裁掉未实现字段；自由文本与 JSON 接口不可混用。
- 坑003：固定复用要在构建期锚定（`include_str!`＋来源一致性校验），运行时拼接易漂移。
- 坑004：提示词里的"宿主已做某事"声明（注入、审批）是契约——宿主没做就必须改掉声明，不能只当文案。

## 5. 预测 → 实测

| 场景 | 预测 / 验收 | 实测 |
|---|---|---|
| 来源一致性（嵌入母本） | 嵌入的原始 `prompt.md` 与固定提交逐字节一致，不含任何剔除/改写 | 通过（`s1`：20903 字节、SHA-256 与锚点一致） |
| 提示词适配（发送文本） | 实际发送的提示词＝母本仅经第 3 节四项适配，无其他改动；与嵌入母本是两个不同文本且可分别校验 | 通过（`s2`：与测试侧独立重建逐字节一致，17068 字节；无 `update_plan`/旧示例/注入声明残留） |
| 空历史 | 恰 `system、user` 两条消息；恰 `system、user` 消息与三工具定义（016） | 通过（`s3`：2 消息＋3 工具＋顶层恰 `{messages, tools}` 且均为 function（原 `s7` 断言并入），序列化 JSON 复核；016 适配后含 write_stdin） |
| 两轮既有历史 | 角色、顺序、原文逐字保留；发送版提示词与本轮输入各恰出现一次 | 通过（`s4`：`system→user→assistant→user`；与历史同文的本轮输入不去重，恰 2 次；历史快照在组装与重复组装后均不变） |
| 中文、引号、换行 | 序列化再解析后逐字一致 | 通过（`s4`（原 `s5` 场景并入）：引号/换行/制表符/emoji/反斜杠在历史与本轮输入逐字投影，序列化再解析逐字往返一致，换行以 `\n` 转义） |
| 工具白名单（016：三项） | 名称恰为 `exec_command/apply_patch/write_stdin`；参数与 003/004/016 JSON 接口一致（`cmd`/`patch`/`session_id` 必填），无未声明字段、无 Move to、无 freeform 措辞 | 通过（`s6`：schema 与 `ExecArgs::from_value`/`apply_patch`/`WriteStdinArgs::from_value` 差分锚定——按 schema 声明键构造的最小实参被接受、未声明字段被拒） |
| 协议映射 | 请求仍发 `POST {base}/chat/completions`；`tools` 为 function 数组；`system` 在首位、本轮 `user` 在末位 | 通过（`s3`（原 `s7` 断言并入）：顶层恰 `{messages, tools}`、function 数组；`s4`：system 首位/user 末位、角色仅 system/user/assistant；端点行为未改，由既有 `llm_flow` 用例保证） |
| 重复组装 | 同一输入组装两次，输出完全相同 | 通过（`s4`（原 `s8` 场景并入）：同一输入两次组装，对每次结果分别核对消息序列、逐字内容与两工具定义——分别核对通过即蕴含输出相同，不以“两份相等”单独作证；输入历史不变） |
| 空白本轮输入 | 返回明确错误，不产生请求片段；历史不变 | 通过（`s9`：空串/空白/全角空格三种形态明确报错，历史原样） |

**验收**：先写失败测试再实现；上述自动化测试全部通过，并检查序列化后的真实 JSON 结构。组装过程不联网、不执行命令、不修改业务文件；不以模型是否调用工具作为本规格的通过标准。

**实测记录（2026-10-09）**：实现位于 `src/prompt.rs`（母本 `include_str!` 内置于 `assets/codex/prompt.md`，20903 字节、SHA-256 `ac8ae1…59807` 与固定提交一致）；九类验收在 `tests/prompt_assembly.rs`（RED：骨架行为实现下 6/9 断言失败；GREEN：全部通过），过滤器移植保真由 `src/prompt.rs` 内嵌 4 个用例锚定（含 Codex 原测试三例）。全量回归 `cargo test --workspace --no-fail-fast`：170 通过 / 0 失败 / 0 跳过（基线 157＋新增 13）。两轮历史样例序列化：消息序 `system(17068B) → user"你好" → assistant"Agent：收到" → user"请继续"`，工具数组含两项 function 定义（`cmd`/`patch` 必填、`additionalProperties:false`）。
**接口同步（2026-10-09，随 006 实施）**：历史条目扩展为 `HistoryItem`（`Text(HistoryMessage)`/`AssistantToolCalls`/`ToolResult`），`build_request` 改收 `&[HistoryItem]` 并新增 `build_turn_request(&[HistoryItem])`（循环续请求，不追加当前输入）；`Text` 变体与原 `HistoryMessage` 行为等价，上述九类验收同日复跑全部通过。
**测试精简（2026-10-10）**：九类验收要求不变；`s5`/`s7`/`s8` 三个测试函数分别并入 `s4`/`s3`（特殊字符样本、顶层键集与 function 类型、重复组装分别核对），`tests/prompt_assembly.rs` 由 9 个函数承载同口径断言。

**固定边界同步（2026-10-10，N4）**：工具定义中 `timeout_ms` 描述为 1..=60000（默认 10000）、`max_output_chars` 为 1..=100000 码点（默认 10000）；最终模型请求 JSON（含 model、messages、tools、request_params 序列化后）≤10 MiB（UTF-8 字节），发送前经限计量序列化检查（不先生成无界缓冲），超限零发送并明确报错、本轮结束保留历史。

**016 修订注记（2026-10-22，待批准）**：用户轮工具面由两项增至三项（`exec_command` 可续接语义＋新增 `write_stdin`）；`exec_command` 描述改为可续接语义（等待只控制返回、运行中返回句柄），参数表加入 `yield_time_ms`/`tty`，`timeout_ms` 描述取消 1..=60000 上限。测试同步：`s3`/`s4`/`s6` 按三工具契约更新（`prompt_assembly` 6/6 通过）；`tools` 数量断言在 tool_loop/llm_flow/e2e 同步为 3。本规格其余适配与母本锚定不变。

**017 修订注记（2026-10-22，已批准交付）**：用户轮工具面由三项增至四项（新增 `update_plan`，定义与解析契约见 [017-update-plan](017-update-plan.md)）；适配①（剔除计划指引节）停用，母本 `## Planning`/`## `update_plan`` 两节按原文保留，嵌入母本锚点（字节数＋SHA-256）不变。测试同步：`s2`（指引保留＋独立重建不再剔节）、`s3`/`s4`（四工具）、`s6`（update_plan Schema 与解析差分锚定）；`tools` 数量断言在 tool_loop/llm_flow/e2e/memory_inject 同步为 4。

**018 修订注记（2026-10-22）**：018 同步问询补充：用户轮工具数按宿主回答能力区分。有交互回答接口的普通轮/带记忆摘要轮为五项（增加 `request_user_input`）；无接口调用方及非交互管道仍四项，执行层拒绝伪造问询。内部记忆轮保持受限 mem 工具。Schema 与成功 `answers`/失败 `error` 结构见 [018](018-user-input.md)。
