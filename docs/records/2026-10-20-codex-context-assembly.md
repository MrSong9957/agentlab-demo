# Codex（openai/codex）上下文组装机制调查报告（2026-10-20）

> 纯调查轮：只阅读源码、测试与快照并做离线验证，未实现本项目任何上下文功能，未调用真实模型接口。
> 本报告所有结论均基于固定 commit；行号与本地路径对应本地 checkout，GitHub permalink 指向同一 commit。
> **修订版（2026-10-20 第二轮）**：修正 WebSocket 增量发送、两种窗口模式下基础指令/工具声明的位置、记忆注入内容、压缩三分支与重建顺序、续接与重试分界五处核心问题；收紧多项表述；新增分支对照表与完整实际组装样本（附件）。原实验事实保留，证据不足处降级标注。

## 1. 一句话结论与调查版本

**一句话结论（修订）**：Codex 的每个采样步骤在**逻辑上**都构造一份全量请求（`build_responses_request` 产出完整 `input`），但内容的「常驻位置」分两种窗口模式——普通窗口把基础指令作为请求前缀 developer 消息、工具声明放请求顶层 `tools` 字段（均不入持久历史）；IncrementalTools 窗口（Responses Lite 模型，feature 默认开）把基础指令与工具声明作为世界状态 Prefix 项**写入历史**、请求字段留空。历史本身按「首轮全量注入 → 之后每步世界状态差量追加」维护；超限时走三条压缩分支（本地摘要 / 远程 V2 / TokenBudget 窗口重置）替换整窗。传输层 WebSocket 在「所比较请求字段不变且当前 input 完整匹配上次基线前缀（可相等或追加）」时发送**增量载荷**（`previous_response_id` + 新增 items），否则全量；`WebsocketSession`（连接＋增量基线）跨 turn 缓存移交，续接不限于同一 turn；HTTP 恒全量。

**调查版本**：

| 项 | 值 |
|---|---|
| 获取日期 | 2026-10-20 |
| 固定 commit | `014489466a1739c2c563cdfe57880c2c7c8680e3`（官方 main） |
| 版本定位 | `git describe`: `voice-cygwin-108b38cf67cbb731-1807-g014489466a`；最近发布 tag `rust-v0.164.0-alpha.6`（本 commit 在其后 1807 个提交）；workspace 版本号 `0.0.0`（dev main） |
| 本地源码路径 | `/tmp/openai-codex-investigate`（Rust 实现在 `codex-rs/` 子目录） |
| permalink 前缀 | `https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/` |
| 实际组装样本附件 | `docs/records/2026-10-20-codex-context-assembly-prompt-sample.json`（本轮运行捕获） |

**覆盖的入口**：CLI TUI / `codex exec` / app-server 共用的 core 会话循环（`session/turn.rs::run_turn` → `run_sampling_request` → `client.rs::ModelClientSession::stream` → `build_responses_request`）；HTTP 与 WebSocket 两种传输（含 WS 增量路径）；本地/远程 V2/TokenBudget 三条压缩分支；resume/fork；Skills、MCP、插件、记忆注入路径。

**与我们 vendored 版本的关系**：本项目 `src/vendor/` 只 vendored 了 `codex-ansi-escape`、`codex-mermaid`、`codex-terminal-detection`、`codex-utils-fuzzy-match` 四个渲染/工具 crate（008 渲染层移植），**不含任何上下文组装代码**，与本次调查对象无版本混用问题。

**证据等级标记**：〔源码〕直接读源码确认；〔快照/测试〕仓库测试或快照文件确认；〔运行〕本轮实际运行确认；〔推断〕由多处源码推论；〔未验证〕未能确认。

---

## 2. 完整上下文清单表

模型最终收到的东西分三个通道：①请求顶层字段（`tools`/`reasoning`/`text` 等）；②**持久历史** `input` 数组（首轮全量注入后按差量追加，含 IncrementalTools 窗口的声明/指令 Prefix 项）；③请求构造时才附加的**请求级前缀项**（仅普通/非增量窗口，不入历史）。下表「位置」列标明通道与窗口条件。

| # | 内容 | 来源 | 加载条件 | 位置/角色 | 顺序 | 更新方式 | 限制 | 证据 |
|---|---|---|---|---|---|---|---|---|
| 1 | 基础模型指令（"You are Codex…"） | 模型目录 `models.json` 每模型自带字面量 `model_messages.instructions_template`（`codex-rs/models-manager/src/lib.rs:13-16` include_str! 嵌入；未知模型兜底 `model_info.rs:16`）；会话解析优先级 config 覆盖 > 会话历史 > 当前模型模板（`core/src/session/mod.rs:801-836`） | 每个采样步骤 | **普通窗口**：请求前缀 developer 消息（kind `model.base_instructions`，`client.rs:1008-1017`），不入历史。**IncrementalTools 窗口**：世界状态 Prefix 项写入历史（`context/world_state/model.rs:92-99`，"reassert full setup when the comparison state is missing"） | input 最前（普通窗口在请求拼接时前置；增量窗口在历史头部） | 普通窗口每请求重建（稳定 UUID v5 id）；增量窗口历史常驻，指令/模型变化时追加 `ModelInstructions` 替换声明片段（`model.rs:120-124`） | 无显式上限；provenance=Model 且关 update_plan 时请求副本剥掉 Planning 段（`session/mod.rs:1603-1619`；增量窗口在 `session/world_state.rs:75-89` 同样剥离） | 〔源码〕〔运行〕样本 item1 metadata `content_item_kinds:["model.base_instructions"]` |
| 2 | 工具声明（内置 + MCP + 扩展） | 内置 spec 硬编码（`core/src/tools/spec_plan.rs`：`exec_command`、`apply_patch`(Freeform+lark 语法)、`update_plan`、`view_image`、web_search 等）；MCP 工具由 `ToolInfo`→`ResponsesApiTool` 转换（`codex-rs/tools/src/mcp_tool.rs:38-78`）；每步重建 ToolRouter（`spec_plan.rs:121-186`） | 每个采样步骤重新捕获（MCP 后连上的 server 下一步生效） | **普通 Responses**：请求顶层 `tools` 字段（`client.rs:1006`）。**RLite 非增量**：请求前缀 `ResponseItem::AdditionalTools{role:"developer"}`（`client.rs:992-1004`，稳定 id）。**IncrementalTools**：历史 Prefix 项——首窗全量 `AdditionalTools`，之后只发变更工具的 `AdditionalTools` ＋ `RemovedTools` standalone 片段（`world_state/top_level_tools.rs:202-260`） | 增量窗口首窗与压缩重建时，Prefix 项位于该段历史最前（`split_prefix_updates`/`assemble_compaction_history`）；**后续工具变更的声明项按差量发生顺序追加到已有历史末尾**（`record_step_world_state_if_changed`→`record_conversation_items`，session/mod.rs:3782-3801），不重排既有历史——`Placement::Prefix` 只影响建窗时的位置，不是对整段历史的排序指令 | 每步 diff；`inference_tools_changed` 检测变化（`client.rs:1444-1452`） | agent-plugin MCP 单工具 8KB/总量 64KB 超限转 Hidden（`mcp_tool_exposure.rs:18-19,149-153`）；曝光六态（`spec_plan.rs:197-289`） | 〔源码〕〔快照〕〔运行〕样本 item0 |
| 3 | 自定义 developer 指令 | config.toml `developer_instructions`（`config/src/config_toml.rs:253-254`） | 非空且非 guardian 基础会话 | 历史：初始上下文 developer 包第一段（`session/mod.rs:4534-4540`）；guardian 会话独立成 GuardianPolicy developer 消息（`mod.rs:4748-4752`） | developer 包内最前（model_switch 之前则 model_switch 更前） | 只在初始上下文注入一次 | 无本地限制；企业 `ManagedDeveloperInstructions` 上限 10,000 tokens（`world_state/managed_developer_instructions.rs:18-31`） | 〔源码〕 |
| 4 | 权限说明（沙箱/审批） | 模板 `codex-rs/prompts/templates/permissions/`：`approval_policy/*.md`、`sandbox_mode/*.md`、`approved_prefixes.md`（`{{ network_access }}`/`{{ approved_prefixes }}` 运行时渲染） | PermissionsState world-state section；config 开关 `include_permissions_instructions` | 历史：developer 片段 `<permissions instructions>` … `</permissions instructions>`（标记定义 `prompts/src/permissions_instructions.rs:271`） | developer 包内（与 Skills 同一条消息的不同 content 项） | world-state 差量：权限变化时发新片段；新审批前缀走紧凑差量（`world_state/compact_permissions.rs`） | 模板固定 | 〔源码〕〔快照〕〔运行〕 |
| 5 | AGENTS.md（全局+线程+项目） | 全局 `$CODEX_HOME/AGENTS.md`/`AGENTS.override.md`（`codex-home/src/instructions/mod.rs:12-13,43-44`）；线程指令（10k token 上限，超限拒绝，`agents_md_manager.rs:190-200`）；项目：仓库根（marker 默认 `.git`）→cwd 每目录一文件，候选序 `AGENTS.override.md`>`AGENTS.md`>config `project_doc_fallback_filenames`（**默认空列表**，`agents_md.rs:189-296`） | 根→cwd 拼接（不是覆盖）；非项目→项目内容间插 `\n\n--- project-doc ---\n\n`（`agents_md.rs:49,401-417`） | 历史：**user 角色**消息，标记 `# AGENTS.md instructions` … `</INSTRUCTIONS>`（`context/user_instructions.rs:26-41`），与 `<environment_context>` 合并进同一条 user 消息 | 首轮初始上下文 user 包；后续变化时差量追加 | 首轮全量；变化时追加「These AGENTS.md instructions replace all previously provided…」/移除通知（`world_state/agents_md.rs:11-13,59-74`）；每步 `agents_md_manager.refresh`（环境 selection/trust 变化才重读盘，`session/mod.rs:3945-3953`）；cwd 变化=缓存键失效→重新发现 | 项目总量 `project_doc_max_bytes` 默认 32KiB 逐文件硬截断、跨环境共享（`agents_md.rs:142-166`）；空/纯空白跳过；不可信项目跳过项目层；**运行时默认不发现 CLAUDE.md/.cursorrules**——仅当用户显式配置 `project_doc_fallback_filenames` 才作为候选（测试 `uses_configured_fallback_when_agents_missing`），另有迁移工具一次性读取（`external-agent-migration/source/cla.rs:21`） | 〔源码〕〔快照〕〔运行〕 |
| 6 | 环境上下文 | `world_state/environment.rs:263-370` 渲染：`<environment_context>` 包 `<cwd>/<shell>/<shell_version>/<current_date>/<timezone>/<network>/<filesystem>/<subagents>`（多环境时 `<environments>` 结构） | EnvironmentsState section；config 开关 `include_environment_context(_time)` | 历史：user 角色片段，与 AGENTS_MD 同一条 user 消息 | 首轮 user 包；变化时差量 | diff 重渲染；XML 转义 | 模板固定；无 platform 字段 | 〔源码〕〔运行〕 |
| 7 | Skills 目录（名称+描述） | `ext/skills` 扩展扫描 `SKILL.md` frontmatter；渲染 `## Skills`+`### Available skills`（`catalog_prompt.rs:84-112`） | 线程级 `contribute_thread_context`（DeveloperCapabilities 槽）→ 初始 developer 包；RLite world-state 差量目录；每轮 TurnInput 目录（`extension.rs:320-421`） | 历史：developer 片段 `<skills_instructions>` | developer 包内 | 目录差量/每轮 | `skill_max_context_tokens` 配置 | 〔源码〕〔快照〕〔运行〕 |
| 8 | 模型切换声明 | `context/model_switch_instructions.rs`（`<model_switch>`，普通窗口）与 `context/model_instructions.rs`（`<model_instructions>`，增量窗口，「The following instructions replace the previous base instructions:」） | 检测到模型变化（`world_state/model.rs:126-135`） | 历史：developer 片段；注入完整上下文时插 developer 区块最前（`session/mod.rs:4684-4688`） | 切换发生的那一步 | 追加声明（不编辑历史旧指令）；回滚时从首 turn developer 消息剔除（`history.rs:827-843`） | — | 〔源码〕 |
| 9 | SKILL.md 全文 | 显式 `$skill` 提及时宿主读取全文（`ext/skills/src/host_prompt.rs:69-109`） | 仅用户输入含 `$mention`；否则模型按目录说明调 `skills.read` 工具读取 | 历史：**user 角色**消息 `<skill><name>…</name><path>…</path>…全文…</skill>`；注入点 `session/turn.rs:1105-1190` | 紧跟当轮用户输入之后 | 一次性 | agent-plugin skill 超限截断 | 〔源码〕〔快照〕`astra_kickoff…snap:134-175` |
| 10 | 插件说明 | `context/available_plugins_instructions.rs`（`<plugins_instructions>`）；显式提及插件时 `plugins/render.rs:19-85`（"Capabilities from the `X` plugin:"） | 插件能力从无到有发一次；提及即时 | 历史：developer 消息 | 紧随 skill 注入 | 差量 | 4KB | 〔源码〕〔快照〕 |
| 11 | 记忆（memories） | `$CODEX_HOME/memories/`（V1）或 `memories_v2/`（V2）下 `memory_summary.md` 的**正文**经模板渲染注入：`build_memory_tool_developer_instructions` 读文件→trim→**摘要正文先按 `truncate_text(Tokens(2500))` 截断**→再填入模板变量 `{{base_path}}`/`{{memory_summary}}`（`ext/memories/src/prompts.rs:31-62`；模板文本不占 2500 token 预算）；V1 模板把摘要正文嵌在 `MEMORY_SUMMARY BEGINS/ENDS` 段内并附使用决策规则、目录布局、引用格式（`templates/memories/read_path.md`） | **默认关**：feature `memories` `default_enabled:false` 且 config `use_memories` | 历史：developer 片段，kind `memories.instructions`，无文本标记（`core/src/context/memory.rs:23-45`）；V1=单个 developer_policy fragment；V2=已截断的渲染结果切成多个 ≤8900 bytes 片（`extension.rs:70-101`，片体渲染时再按 Bytes(8900) 截一次） | 初始上下文 developer 包 | 自动注入的是「指引＋摘要正文」；详细记忆（MEMORY.md、rollout_summaries/、skills/）靠模型按模板指引**用工具按需读取** | 摘要正文 2500 tokens（`ext/memories/src/lib.rs:16`）；V2 每片 ≤8900 bytes | 〔源码〕〔测试〕`prompt_contribution_uses_memory_summary_when_enabled`（`ext/memories/src/tests.rs:175`，断言 1 个 fragment＋DeveloperPolicy 槽） |
| 12 | 用户输入 | `UserInput::{Text,LocalImage,…}` → `ResponseItem::Message{role:"user"}` 单条消息，content_item_kinds `user.text/user.image/user.audio`（`session/mod.rs:3588-3626`） | 每轮 | 历史：user 消息（无标记＝真实输入） | 上下文注入之后 | 追加；steering 输入入 input_queue 下一 step 排入 | 图片先经准备（缩放/上传/内联） | 〔源码〕〔运行〕 |
| 13 | 工具调用与结果 | 模型流式输出 `FunctionCall/CustomToolCall/ToolSearchCall` → `handle_output_item_done` 派发执行（并行 FuturesOrdered）→ 输出 `FunctionCallOutput` 落历史（`turn.rs:2575-2603` drain_in_flight） | 模型发起调用即执行 | 历史：FunctionCall（assistant 侧）+ FunctionCallOutput（host 侧）配对 | 在产生它们的 assistant 输出之后 | 追加；`normalize_history` 保证配对、孤儿输出剔除（`history.rs:968-986`） | 活跃历史工具输出按截断策略（模型 `truncation_policy`，如 astra=tokens:10000，＋1.2 序列化余量）；rollout 的 ResponseItem 记录保留**进入该截断之前**的形态（`session/mod.rs:3722-3760`：先截断进内存历史、后以未截断 envelope 落盘；工具构造结果时自身的截断——exec 10k token/1MiB 字节帽——已更早发生） | 〔源码〕 |
| 14 | 推理项（Reasoning） | 模型输出；`include:["reasoning.encrypted_content"]` 请求带回密文 | 模型输出即记录 | 历史：`ResponseItem::Reasoning{encrypted_content}` 原样重放 | 跟随 assistant 输出 | 追加 | token 估算按 base64 长度×3/4−650（`history.rs:1058-1064`） | 〔源码〕 |
| 15 | 当前时间提醒 | `context/current_time_reminder.rs`（developer，`<current_time_reminder>`，"It is YYYY-MM-DD HH:MM:SS UTC."） | feature `CurrentTimeReminder`＋config `current_time_reminder.reminder_interval_seconds` 到期（`session/time_reminder.rs:156-199`；时钟失败另发 CurrentTimeUnavailable） | 历史：developer 片段 | step 边界 | 追加 | — | 〔源码〕 |
| 16 | token 预算/上下文窗口提示 | `context/token_budget_context.rs`（developer，Feature::TokenBudget 时独立 developer 消息，含窗口 id 与 hints）；`context_window_guidance`；rollout 预算提醒 | feature/预算条件 | 历史：developer | 独立消息 | 每窗更新 | — | 〔源码〕 |
| 17 | 压缩摘要 | 本地：`SUMMARY_PREFIX`＋最后一条 assistant 消息 | 压缩完成后 | 历史：**user 角色**消息（`core/src/context/compaction_summary.rs:26-28`）；远程 V2：`ResponseItem::Compaction{encrypted_content}` 在新历史末尾 | 新窗口内、初始上下文之后、精选用户消息之后 | 整窗替换时写入 | 本地保留用户消息预算 20,000 tokens（`compact.rs:63`）；远程保留消息总预算 64,000 tokens（`compact_remote_v2.rs:87`） | 〔源码〕〔快照〕 |
| 18 | 会话启动前缀（session prefix） | `core/src/session_prefix.rs` | 特定会话来源 | 历史：首批 | 最前 | 一次性 | — | 〔源码〕 |
| 19 | 扩展贡献片段 | `ExtensionRegistry` 的 context contributors：`contribute_thread_context`（DeveloperPolicy/DeveloperCapabilities/ContextWindow 槽）与 `contribute_turn_context`（`session/mod.rs:4567-4604`） | 扩展注册即生效（skills、memories、git attribution 等） | developer 包或独立消息 | 初始上下文 | turn 级每步可贡献 | 生产者自限（trait 约定，`context-fragments/src/fragment.rs:46`） | 〔源码〕 |
| 20 | hooks 注入 | `hook_additional_context.rs`、Stop hook continuation prompt（`turn.rs:743-760`） | hook 事件 | 历史：user/developer 消息 | 事件驱动 | 追加 | — | 〔源码〕 |
| 21 | 多 Agent 协作 | `<multi_agent_mode>`、`<multi_agent_role>`、AgentMessage 项、InterAgentCommunication | 模式开启/消息到达 | 历史：developer 片段＋AgentMessage ResponseItem | 注入完整上下文时 active mode 在 usage hint 之后单独消息（`session/mod.rs:4689,4729-4736`） | 差量/事件 | 保留 agent 消息 ≤10,000 tokens（远程压缩） | 〔源码〕〔运行〕样本 item3/4 |

---

## 3. 最终请求结构（示意与实际分离）

### 3.0 分支对照表（修订新增）

| 维度 | 普通 Responses 窗口 | Responses Lite（非增量） | RLite＋IncrementalTools（feature 默认开） |
|---|---|---|---|
| 基础指令位置 | 请求前缀 developer 消息（每请求重建，稳定 v5 id，`client.rs:1008-1017`） | 同左 | **历史 Prefix 项**（首窗全量，`world_state/model.rs:92-99`；请求不加前缀，`turn.rs:1688-1694`） |
| 工具声明位置 | 请求顶层 `tools` 字段 | 请求前缀 `AdditionalTools` item（`client.rs:992-1004`） | **历史 Prefix 项**：首窗全量，后续仅变更项＋`RemovedTools` 片段（`top_level_tools.rs:202-260`） |
| build_prompt 的 tools/base_instructions | 真实值 | 真实值 | 均置空（`turn.rs:1682-1694`） |
| 模型切换表达 | `<model_switch>` 片段 | 同左 | `<model_instructions>` 替换声明（`model.rs:120-124`） |
| 窗口判定 | `!use_responses_lite` | `use_responses_lite && !Feature::IncrementalTools` | `use_responses_lite && Feature::IncrementalTools`（`step_context.rs:52-58`；`session/world_state.rs:44-52`：已有历史时按「历史是否含工具声明」判定窗口延续） |

| 维度 | HTTP | WebSocket |
|---|---|---|
| 载荷 | 恒全量（`ResponsesApiRequest` 原样） | **增量/全量双模式**（§3.1b） |
| 会话生命周期 | 无状态 | `ModelClientSession` 每 turn 新建（turn.rs:194-195），但其内部的 `WebsocketSession`（连接、`last_request`、`last_response_rx`）**跨 turn 缓存移交**：`Drop` 时整体存回 thread 级 `ModelClient.cached_websocket_session`（client.rs:1435-1441），下个 turn `new_session` 取出复用（client.rs:674-687）。每 turn 全新的只是 `turn_state` 路由令牌（`Arc<OnceLock>`，client.rs:685；owner 变化时也重建，client.rs:1707-1709）——注释明言跨 turn 重放旧令牌违反契约（client.rs:291-297） |
| 缓存重置条件 | — | ① auth owner 变化（new_session 内 `reset("other")`，client.rs:677-681）；② WS 回退 HTTP（`force_http_fallback` 存入空会话，client.rs:736）；③ `websocket_connection` 里 headers 变化 / connection_key 变化 / 连接已关闭（`reset("other"/"connection_closed")` 后重连，client.rs:1686-1704）。`reset` 清连接与增量基线，但保留 `continuation_reset_reason` 供下次发送的遥测（client.rs:479-490） |
| 增量条件 | — | 缓存未重置且：本会话有上次请求与响应；**`responses_request_properties_mismatch` 所比较的字段**（model/tools/tool_choice/parallel_tool_calls/reasoning/store/stream/include/service_tier/prompt_cache_key/text；不含 input/stream_options/client_metadata/access_programs，client.rs:389-435）不变；当前 input **完整匹配**「上次 input＋服务端新增 items」基线前缀（逐项相等、忽略内部元数据；可完全相等〔此时新增列表为空〕或在其后追加，client.rs:1512-1543）；上次 response_id 非空 |
| 增量载荷 | — | `previous_response_id=Some(id)`、`input=新增尾部 items`（相等时为空列表；`client.rs:2140-2167`） |
| 全量时机 | 每次 | 无上次请求/响应/响应 id、所比较字段变化、input 缩短（InputShortened）或前缀不匹配（InputMismatch）——六类 `NonIncrementalReason`（client.rs:356-378）。恢复会话通常因新 thread 级 `ModelClient` 无缓存状态而全量，此时遥测标签取 `restored_history`（诊断标签来源，非主动阻断条件、非枚举变体，client.rs:2104-2114） |
| 预热与首次生成 | — | 两条不同路径：`preconnect_websocket` **只建立连接、从不发送提示载荷**（client.rs:1578-1590 文档注释 "This performs only connection setup; it never sends prompt payloads"）；`prewarm_websocket` 则携带 Prompt **发送一个 `generate:false` 的真实请求**（含 input 载荷但不做生成，等待 Completed 后复用连接，client.rs:2305-2353、2161），未跟踪的预热响应 id 可被续接复用（`last_response_from_untraced_warmup`，此时推理痕迹仍记录逻辑全量请求，client.rs:2123-2138）。首次生成请求 `generate=None` |
| 回退 | —（起点） | WS 建连 426/重试耗尽 → 本会话永久回退 HTTP（`try_switch_fallback_transport`） |

三种「差量」不要混淆：**状态差量**（世界状态 section 的 render_diff，产出历史新片段）、**工具声明差量**（IncrementalTools 的 AdditionalTools/RemovedTools 历史项）、**网络载荷差量**（WS previous_response_id + 尾部 items，只影响线上字节，不改逻辑请求）。逻辑请求（`build_responses_request` 产物与 rollout 推理痕迹）始终全量——源码注释明言 WS 复用未跟踪 warmup 响应时仍向推理痕迹记录**逻辑全量**请求（`client.rs:2133-2138`）。〔源码〕

### 3.1 序列化后的请求骨架（示意）

```jsonc
// 示意（字段顺序＝serde 声明序：路由字段在前，codex-rs/codex-api/src/common.rs:278-304 注释）
{
  "model": "gpt-6-astra",
  "stream": true,                       // 恒为流式
  "service_tier": "…",                  // 可选
  "input": [ /* 逻辑全量：请求级前缀(普通/非增量窗口) ＋ 持久历史 */ ],
  "tools": [ /* 普通窗口的 ToolSpec JSON；RLite 两种窗口均省略 */ ],
  "tool_choice": "auto",
  "parallel_tool_calls": true,
  "reasoning": { "effort": "…", "summary": "…" },
  "store": false,
  "stream_options": { … },
  "include": ["reasoning.encrypted_content"],
  "prompt_cache_key": "…",              // = session_id（内部会话为合成键，client.rs:646-658）
  "text": { "verbosity": "…", "format": … },
  "client_metadata": { … },
  "access_programs": null
}
// 没有 "instructions" 字段。HTTP 以此骨架原样发送；
// WebSocket 在增量条件下改写为 previous_response_id=<上次id>、input=<仅新增>（§3.0）。
```

**3.1b WebSocket 增量路径（修订）**：`stream_responses_websocket` 构造载荷前先 `prepare_websocket_request(&request)`（`client.rs:2094-2122`）→ `get_incremental_items`（`client.rs:1512-1543`）把「上一次请求 input ＋ 上一次响应服务端新增 items」当基线，逐项校验当前 input **完整匹配该基线前缀**（`response_items_equal_ignoring_internal_metadata`；当前更短即 `InputShortened` 失败；完全相等合法、返回空新增列表；尾部多出的部分即新增），并要求 `responses_request_properties_mismatch`（`client.rs:389-435`）所比较的字段不变；成功则 `ResponseCreateWsRequest{ previous_response_id: Some(last_response.response_id), input: 新增 items }`（`client.rs:2140-2167`），失败（六类 `NonIncrementalReason`，`client.rs:356-378`）则 `previous_response_id: None`、input 全量。`From<&ResponsesApiRequest>` 里的 `None` 只是默认值，实际由上述分支覆盖。**跨 turn 续接**：`WebsocketSession`（连接＋增量基线）经 `Drop`→thread 级缓存→下个 turn `new_session` 移交（§3.0 生命周期行），因此续接是否成立取决于缓存是否被重置（auth/回退/headers/key/连接关闭）与请求兼容性判定，**不是**「只能在同一 turn 内」；每 turn 重建的只有 `turn_state` 路由令牌。〔源码〕

### 3.2 场景一：新会话首轮请求（示意；实际样本见 §7.2 与附件）

```jsonc
// 证据：快照 all__suite__model_visible_layout__model_visible_layout_cwd_change_refreshes_agents.snap
// ＋ 本轮运行样本（增量窗口实测布局，附件 items 0-6）
{
  "model": "…",
  "input": [
    // —— 普通窗口：以下两项由 client.rs 在请求拼接时前置（不入历史）——
    //   [前缀] developer 基础指令消息（client.rs:1008-1017）
    //   [顶层] tools 字段承载工具声明
    // —— IncrementalTools 窗口（实测样本）：以下两项是历史 Prefix 项 ——
    { "type": "additional_tools", "id": "at_…", "role": "developer", "tools": [ /* 3 个 namespace: functions/clock/collaboration */ ] },
    { "type": "message", "role": "developer", "content": [ { "type": "input_text", "text": "You are Codex, an agent based on GPT-6. …（gpt-6.1-sol 模板全文 21,769 字符/21,779 UTF-8 字节）" } ] },
    // —— 两种窗口共有的历史·初始上下文 developer 包 ——
    { "type": "message", "role": "developer", "content": [
        { "type": "input_text", "text": "<skills_instructions>## Skills…</skills_instructions>" },
        { "type": "input_text", "text": "<permissions instructions>…审批/沙箱说明…</permissions instructions>" },
        { "type": "input_text", "text": "<collaboration_mode>…</collaboration_mode>" }
    ] },
    { "type": "message", "role": "developer", "content": [ { "type": "input_text", "text": "<multi_agent_role>…" } ] },
    { "type": "message", "role": "developer", "content": [ { "type": "input_text", "text": "<multi_agent_mode>…" } ] },
    // —— 历史·初始上下文 user 包 ——
    { "type": "message", "role": "user", "content": [
        { "type": "input_text", "text": "# AGENTS.md instructions for /repo\n\n<INSTRUCTIONS>\n全局\n\n--- project-doc ---\n\n项目层…\n</INSTRUCTIONS>" },
        { "type": "input_text", "text": "<environment_context>\n  <cwd>/repo</cwd>\n  <shell>bash</shell>\n  <current_date>2026-10-20</current_date>\n  <timezone>Asia/Shanghai</timezone>\n  <filesystem>…</filesystem>\n</environment_context>" }
    ] },
    // —— 历史·真实用户输入 ——
    { "type": "message", "role": "user", "content": [ { "type": "input_text", "text": "帮我看下这个仓库" } ] }
  ]
}
```

### 3.3 场景二：工具结果返回后的下一请求（同轮第二个采样步骤，示意）

```jsonc
// 外层 run_turn 循环再入（§4.3-5）：input 重新取 clone_history().for_prompt()（turn.rs:560-566）
{
  "input": [
    // …基础指令/声明（位置同 §3.2，按窗口模式）＋ 全部既有历史（原样重发）…，
    { "type": "reasoning", "summary": […], "encrypted_content": "…" },
    { "type": "message", "role": "assistant", "content": [ { "type": "output_text", "text": "我先看看目录" } ] },
    { "type": "function_call", "call_id": "call_x", "name": "exec_command", "arguments": "{\"cmd\":\"ls\"}" },
    { "type": "function_call_output", "call_id": "call_x", "output": "src/ docs/ …" }  // 活跃历史中已按策略截断
  ]
}
// WS 增量条件下，线上实际只发：previous_response_id=<上次id> + input=[function_call_output]（服务端新增的
// reasoning/assistant/function_call 属于上次响应的 items_added 基线，不重发）。
```

要点：同轮内「模型请求 → 工具执行 → 再请求」不新建 turn；call/output 配对由 `normalize_history` 在历史出口统一保证（`ensure_call_outputs_present` 给缺输出的调用合成占位、`remove_orphan_outputs` 剔除孤儿输出，`history.rs:968-986`）。

### 3.4 场景三：下一轮用户输入（第二轮 turn，示意）

```jsonc
{
  "input": [
    // …基础指令/声明＋既有历史原样保留…,
    { "type": "message", "role": "user", "content": [ { "type": "input_text", "text": "继续" } ] }
    // reference_context_item 已存在 → 本轮不再全量注入初始上下文，
    // 只追加世界状态差量片段（AGENTS/环境/权限/模型任一变化时才有；无变化则零新增）
  ]
}
// 新一轮 turn 新建 ModelClientSession（turn_state 路由令牌全新），但 WebsocketSession 从
// thread 级缓存移交（§3.0）：若缓存未被重置且本请求与上次兼容，新 turn 首请求也可增量续接；
// 是否续接由缓存状态＋兼容性判定决定，不存在「新 turn 必定全量」的规则。
```

### 3.5 场景四：压缩后的请求（示意）

```jsonc
// 证据：快照 all__suite__compact__mid_turn_compaction_shapes.snap ＋ compact_tests.rs:698-813（插入顺序实证）
// 压缩请求本身（本地分支）：input = 全量历史 + 末尾 user 消息 <SUMMARIZATION_PROMPT>
// （SUMMARIZATION_PROMPT 作为 UserInput::Text 走普通 turn，compact.rs:117-133；
//   远程 V2 分支则追加 ResponseItem::CompactionTrigger 请求控制项，compact_remote_v2_attempt.rs:172）
// 压缩完成 → 整窗替换 → 后续请求（以保留两条用户消息为例，§6.4）：
{
  "input": [
    // [prefix]（增量窗口）基础指令/工具声明 Prefix 项 —— split_prefix_updates 分离、恒在最前
    { "type": "message", "role": "user", "content": [ { "type": "input_text", "text": "第一条用户消息(保留,≤20k tokens)" } ] },
    { "type": "message", "role": "developer", "content": [ /* 重建的初始上下文（插在最后一条真实用户消息之前） */ ] },
    { "type": "message", "role": "user", "content": [ /* AGENTS+ENV 等重建 user 包 */ ] },
    { "type": "message", "role": "user", "content": [ { "type": "input_text", "text": "最后一条用户消息(保留)" } ] },
    { "type": "message", "role": "user", "content": [ { "type": "input_text", "text": "<SUMMARY_PREFIX>…摘要正文" } ] }
    // 远程 V2 分支：末项为 ResponseItem::Compaction{encrypted_content}（服务端密文摘要），无明文摘要消息
  ]
}
```

---

## 4. 实现逻辑与步骤

### 4.1 三层数据结构与术语

大白话：**thread（会话）**＝一个 rollout 文件、一个 Session 对象；**turn（轮）**＝用户一次输入引发的完整处理（`run_turn`，可能含多个采样步骤）；**step/采样请求**＝一次发往模型的请求（`StepContext` 是请求级快照）；**context window（窗口）**＝两次压缩之间的历史段（压缩换窗口号）；**逻辑请求**＝`build_responses_request` 构造的全量载荷（线上可能被 WS 压成增量）。

| 层 | 结构 | 位置 |
|---|---|---|
| 会话状态 | `Session`（含 `state`，历史在其 `history: ContextManager`） | `core/src/session/session.rs:60`、`session/mod.rs` |
| 内存历史 | `ContextManager { items: Arc<Vec<ResponseItemEnvelope>>, history_version, reference_context_item, world_state_baseline, token_info, … }`（旧→新；Arc 写时复制） | `core/src/context_manager/history.rs:92-127` |
| 请求载荷 | `Prompt { input, tools, base_instructions, output_schema, … }` | `core/src/client_common.rs:23-42` |
| 线上请求 | `ResponsesApiRequest`（HTTP）/`ResponseCreateWsRequest`（WS，增量时改写 previous_response_id/input） | `codex-rs/codex-api/src/common.rs:279-360` |
| 持久化 | `RolloutItem` JSONL（SessionMeta/ResponseItem/Compacted/TurnContext/TokenUsageRecord/WorldState/RetainedContext/EventMsg…） | `codex-rs/history/src/lib.rs:215-231` |

### 4.2 请求构造主链（一个采样的完整调用链）

```
run_turn                                    session/turn.rs:173
├─ run_pre_sampling_compact                 turn.rs:1379（达阈值→先压缩再采样）
├─ required_mcp_servers_for_input           turn.rs:1003（按输入提及启动 MCP/plugin）
├─ capture_step_context*                    session/mod.rs:3853→3893
│   └─ agents_md_manager.refresh / build_tool_router（每步刷新指令缓存与 ToolRouter）
├─ record_context_updates_and_set_reference_context_item   session/mod.rs:4931
│   ├─ reference_context_item=None → build_initial_context_with_world_state  mod.rs:4513（全量）
│   │   ├─ build_world_state_for_step 判定窗口模式（session/world_state.rs:53-115：
│   │   │   增量→ModelInstructionsState::for_incremental_tools（指令入 Prefix）；普通→ModelOnly）
│   │   ├─ developer_sections：DeveloperInstructions、扩展 contribute_thread_context
│   │   │   （skills 目录/memories）、ModelSwitchInstructions（插最前）、推荐插件、…
│   │   ├─ separate_developer_messages：TokenBudgetContext、多 Agent 角色、guardian policy…
│   │   ├─ world_state.render_full()：各 section 差量（增量窗口含 BaseInstructions/
│   │   │   AdditionalTools 的 Placement::Prefix 项）
│   │   └─ merge_world_state_updates：同角色相邻片段合并为一条消息（context_manager/updates.rs:21-49）
│   │       ＋ split_prefix_updates：Prefix 项独立成项（world_state/mod.rs:125-140）
│   └─ 否则 → ContextManager::update_world_state（对 world_state_baseline 逐 section render_diff）
├─ build_skills_and_plugins → injection_items → record_conversation_items    turn.rs:328-423
├─ run_hooks_and_record_inputs（用户输入落历史）  turn.rs:386
└─ loop（每采样步骤）
    ├─ maybe_record_current_time_reminder / record_step_world_state_if_changed /
    │   record_reasoning_effort_override   turn.rs:535-556
    ├─ sampling_request_input = sess.clone_history().for_prompt(input_modalities)   turn.rs:560-566
    │   └─ normalize_history（call/output 配对修复、孤儿剔除、按模态剥离图片/音频）
    └─ run_sampling_request               turn.rs:1714（内层 loop：失败重试，§4.3-5）
        ├─ 首迭代用传入 input；重试迭代 clone_history().for_prompt() 重建   turn.rs:1753-1759
        ├─ executed_tool_calls.attach_to_prompt（重试幂等元数据）
        ├─ build_prompt（incremental_tools 窗口→tools/base_instructions 置空） turn.rs:1673-1702
        ├─ try_run_sampling_request       turn.rs:2628
        │   └─ client_session.stream(...)  client.rs:2366
        │       ├─ WireApi::Responses（唯一 wire API）
        │       ├─ build_responses_request client.rs:956（逻辑全量请求）
        │       │   ├─ 请求级前缀：普通窗口基础指令 developer 消息；RLite 非增量 AdditionalTools
        │       │   ├─ 非 RLite：清 content_item_metadata（所有 provider）
        │       │   ├─ 非 OpenAI：清 passthrough 元数据；除 Reasoning/Compaction/ContextCompaction
        │       │   │   外清 encrypted_content；FunctionCall.encrypted_function_args=None（client.rs:1025-1043）
        │       │   └─ 组装 ResponsesApiRequest（store:false, stream:true, include:[encrypted_content]）
        │       ├─ WebSocket 优先：prepare_websocket_request → 增量(previous_response_id+尾部)或全量
        │       │   （client.rs:2094-2167；§3.1b）；WS 失败/426 → 永久回退 HTTP
        │       └─ HTTP：ResponsesHttpExecutor 恒全量
        ├─ 流事件循环：OutputItemDone → handle_output_item_done
        │   ├─ 非工具项：record_completed_response_item*（assistant 消息/Reasoning/FunctionCall 落历史）
        │   └─ 工具项：tool_future 入 FuturesOrdered（并行执行）
        ├─ drain_in_flight → 工具输出 envelope → record_annotated_conversation_items  turn.rs:2575-2603
        └─ 返回 Ok{needs_follow_up}（工具续接判定）或 Err（可重试错误交内层 loop）
```

**「逻辑全量」与「线上载荷」分离**：`store:false` 下 HTTP 每次全量；WS 增量依赖 `WebsocketSession` 缓存的 `last_request` 与 `last_response`（该缓存经 `Drop`→thread 级缓存跨 turn 移交，client.rs:287-288 注释明言缓存目的就是增量复用、1435-1441 存回路径；重置条件见 §3.0）。恢复会话时新的 thread 级 `ModelClient` 通常没有可复用的缓存状态，因此首个请求因 `NoPreviousRequest` 走全量；`restored_history` 标记只是此时非增量原因遥测标签的来源（client.rs:2104-2114），**不是主动拒绝增量的条件、不属于 `NonIncrementalReason` 枚举**。rollout 推理痕迹与快照测试断言的都是**逻辑请求**。〔源码〕

### 4.3 六个场景的执行顺序

1. **新会话第一条输入**：`record_initial_history(InitialHistory::New)` **不**立即注入上下文（`session/mod.rs:1668-1675`：推迟到首个真实 turn，让 turn/start 覆盖先合并）→ `run_turn` → 首个 step `reference_context_item=None` → 全量注入（§4.2）。
2. **工具调用后继续**：见 §4.5「正常续接与失败重试」——外层 `run_turn` 循环再入，全量历史重发（WS 可增量）。
3. **第二条用户输入**：新 `run_turn`（新 `ModelClientSession`＋全新 `turn_state` 令牌；`WebsocketSession` 从 thread 级缓存移交，WS 续接按 §3.0 条件判定）；世界状态走差量；Skills/插件按新输入提及重建注入项；steering 输入在采样后排空。
4. **cwd/模型/权限变化**：cwd → 环境缓存键失效 → AGENTS_MD 重新发现＋替换声明片段（快照 `cwd_change_refreshes_agents`）；模型 → 普通窗口 `<model_switch>`／增量窗口 `<model_instructions>` 替换声明，且切换后采样前的压缩用**旧模型**跑、失败按条件用当前模型重试一次（`compact_model_fallback.rs:10-33`）；权限 → PermissionsState 差量片段。配置变化另有 `ResponseItem::ConfigurationUpdate`（请求级去重/保留，`client.rs:967-983`）。
5. **上下文压缩**：触发（§6.3）→ `run_auto_compact`（turn.rs:1514）三分支（§6.4）→ 替换历史 → `recompute_token_usage` → continuation 以新窗口全量重发。
6. **恢复会话 / fork**：resume＝读 rollout JSONL → `reconstruct_history_from_rollout`（定位最后一个 `Compacted` 检查点，丢弃其前 items，用 replacement_history 重建，`session/rollout_reconstruction.rs:12-96`）→ 逐条 `replay_annotated_item` 装回（保留原 id 与 retained_source，`history.rs:534-553`）→ `replace_annotated_history(Reset)`。WS 首请求走全量的原因是**缓存状态缺失**（新 thread 级 `ModelClient` 无可移交的 `WebsocketSession`，`NoPreviousRequest`），`restored_history` 标记只用于此时非增量原因的诊断标签，不参与判定（§3.0）。fork＝基于**已持久化** items：`TruncateBeforeNthUserMessage(n)` 或 `Interrupted` 语义（`thread_manager.rs:203-227`）；引用式 fork 子 rollout 只写边界后记录。子代理 spawn 可选全历史 fork 或全新（`thread_spawn.rs:58-167`）。

### 4.4 流程图（主要分支）

```
                    ┌────────────────────────────────────────────────┐
   用户输入/op       │ Session 状态机（thread_manager / handlers）        │
   ───────────────▶ │  new turn / resume / fork / compact / inject     │
                    └───────────────┬────────────────────────────────┘
                                    ▼
                          run_turn（每轮一个，新 ModelClientSession＋新 turn_state 令牌；
                                   WebsocketSession 经 thread 级缓存移交，可跨 turn 续接）
                                    │
              ┌─────────────────────┼──────────────────────┐
              ▼                     ▼                      ▼
     pre-sampling compact   capture_step_context    首轮？──是──▶ 全量初始上下文
     （达阈值先压缩）        （AGENTS.md 刷新、        │            │（含增量窗口的
              │             ToolRouter 重建、          │            │  指令/声明 Prefix 项）
              │             窗口模式判定）             否            │
              │                     │                  ▼            ▼
              │                     │           world-state 差量   初始上下文入历史
              └─────────┬───────────┴──────────────────┴───────────┘
                        ▼
            ┌──── 采样步骤 loop（每 step）─────────────┐
            │ 时间提醒/世界状态差量/effort 覆盖           │
            │ history.for_prompt()（配对修复）          │
            │ build_prompt（按窗口模式填 tools/指令）     │
            │ build_responses_request（逻辑全量）        │
            │ client.stream → WS（增量或全量）/HTTP 全量 │
            │ 流事件：工具→并行执行→输出落历史           │
            └───────┬────────────────┬─────────────────┘
                    │ Ok{needs_follow_up=true}         │ Err(可重试)
                    ▼（外层 run_turn 循环）              ▼（run_sampling_request 内层 loop：
        全量历史再发（下一采样步骤）                      退避重试≤max_retries，重建 input）
                    │needs_follow_up=false
                    ▼
        token 检查 →（65% 后台投机压缩 / 90%+buffer 强制换窗 / PostTurn 百分比）
                    ▼
        run_auto_compact 三分支（TokenBudget 窗口重置 / 远程 V2 / 本地摘要）
                    ▼
        整窗替换：prefix 恒最前 ＋ 初始上下文插在最后一条真实用户消息前
        ＋ 精选用户消息 ＋ 摘要（user 消息）或 Compaction 项（V2）
```

### 4.5 正常续接与失败重试（修订）

**正常工具续接（外层循环）**：`try_run_sampling_request` 流结束 → `drain_in_flight` 把工具输出落历史（`turn.rs:3344-3351`）→ 返回 `Ok(SamplingRequestResult{needs_follow_up:true, …})`（needs_follow_up 由工具 future 累积，`turn.rs:2973`）→ `run_sampling_request` **立即** `return Ok`（`turn.rs:1796-1798`）→ 回到 `run_turn` 外层 loop：`needs_follow_up` 判定（`turn.rs:610`）→ `continue` → 下一迭代重取 step_context、重算 `sampling_request_input = clone_history().for_prompt()` → **新调用** `run_sampling_request`。一次工具续接＝外层循环一次迭代＝一次新的（逻辑全量）请求。

**失败重试（内层循环）**：`run_sampling_request` 自身的 `loop`（`turn.rs:1750-1843`）仅在 `try_run_sampling_request` 返回 **Err** 时介入：`handle_response_stream_error`（`ResponsesStreamRetryState`、退避、上限 provider `stream_max_retries()`）决定重试 → `retry??` 后继续 loop，此时 `initial_input` 已消费，`prompt_input` 改由 `clone_history().for_prompt()` 重建 → 再次 `try_run_sampling_request`。两类错误不重试直接上抛：`ContextWindowExceeded`（置满 token 后返回，交 `run_turn` 的 Err 分支压缩后 continue，`turn.rs:1800-1803`）与 `UsageLimitReached`。preempt（steering）取消 → 返回 `Ok{needs_follow_up:true}` 交外层排空输入。〔源码〕

---

## 5. 真正生效的提示词内容

### 5.1 静态模板（构建期 include_str! 嵌入）

| 模板 | 内容职责 | 注入条件/变量 |
|---|---|---|
| `codex-rs/protocol/src/prompts/base_instructions/default.md`（275 行） | 兜底基础指令（"You are a coding agent running in the Codex CLI…"）；**含完整 AGENTS.md 语义规则**（17-24 行：作用域＝所在目录整棵子树、触碰的每个文件必须遵守作用域内 AGENTS.md、代码风格规则只作用于作用域内代码）；与 `models-manager/prompt.md` 逐字节相同（双份维护） | 仅 `BaseInstructions::default()` 与未知模型兜底；正常路径用 models.json 每模型字面量模板 |
| models.json `model_messages.instructions_template` | **实际生效的基础指令**：每模型完整字面文本（如 gpt-6-astra 21,420 字符/21,428 字节、gpt-6.1-sol 21,769 字符/21,779 字节，开头均为 "You are Codex, an agent based on GPT-6. You and the user share one workspace…"）；较新模型（gpt-6-astra/6.1-sol/6-sol/6-luna）提及 AGENTS.md 但**无** default.md 那段 spec（仅在审批解释处顺带出现）；gpt-5.5/5.6 系不提及 | 模型目录（远端 `/models` 或打包内置，缓存 300s）；字段注释"Formerly a personality template, now literal text"（`openai_models.rs:545-546`）。实测样本（§7.2）的 item1 与打包目录 **gpt-6.1-sol** 模板逐字节一致〔运行〕 |
| `prompts/templates/permissions/*.md` | 审批策略说明（Escalation Requests 规则、命令分段求值）与沙箱模式说明 | `{{ network_access }}`、`{{ approved_prefixes }}` 运行时渲染 |
| `prompts/templates/compact/prompt.md` | 压缩摘要指令："You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM…"（要求含进展/决策/剩余工作/关键数据） | config `compact_prompt` 可替换；作为 user 消息附加 |
| `prompts/templates/compact/summary_prefix.md` | 摘要前缀："Another language model started to solve this problem and produced a summary of its thinking process…" | 拼接在摘要正文前 |
| `prompts/templates/review/rubric.md`、`guardian/*.md`、`realtime/*.md`、`persistent_mode.md` | review 评分、guardian 策略、realtime 会话、持久模式 | 各自功能路径 |
| memories `read_path.md`（V1）/`read_path_v2.md`（V2） | **记忆注入本体（修订）**：V1 模板含记忆使用决策规则（何时跳过/默认使用）、目录布局（memory_summary.md/MEMORY.md/skills/rollout_summaries/）、快速检索流程（≤4-6 步预算）、引用格式（`<oai-mem-citation>` 块）与更新规则（仅显式要求时写 ad_hoc notes）；**摘要正文经 `{{ memory_summary }}` 嵌入 `MEMORY_SUMMARY BEGINS/ENDS` 段**。V2 模板更精简（摘要即历史上下文、按需读 rollout_summaries、不做投机检索） | feature `memories`＋config `use_memories`；**`memory_summary.md` 正文先按 2500 tokens 截断（`truncate_text(Tokens(2500))` 作用于摘要文本本身，prompts.rs:40-45），再填入模板**——不是「模板＋摘要」合计限 2500 tokens，模板文本不占该预算；V2 分片另有单片段 ≤8900 bytes 独立限制（extension.rs:78-92） |

**存在但未被使用的提示词文件**〔源码，全仓 grep 无引用〕：`codex-rs/core/gpt_5_codex_prompt.md`、`gpt_5_1_prompt.md`、`gpt_5_2_prompt.md`、`gpt-5.1-codex-max_prompt.md`、`gpt-5.2-codex_prompt.md`、`codex-rs/core/templates/collab/experimental_prompt.md`——均为 model_family→models-manager 迁移（commit `cee37a32b2`）后的遗留物。本 commit 已无 `ModelFamily` 结构。

### 5.2 运行时拼装内容（模板＋代码生成）

- **基础指令请求副本**：`get_prompt_base_instructions` 在「update_plan 关闭＋无自定义 catalog＋provenance=Model」时剥掉 Planning/update_plan 段（`session/mod.rs:1603-1619`）——只影响发送副本，不改持久化；增量窗口在 `build_world_state_for_step` 内同样条件剥离（`session/world_state.rs:75-89`）。
- **AGENTS.md 包装**：`# AGENTS.md instructions for {cwd}\n\n<INSTRUCTIONS>\n{全局}\n\n--- project-doc ---\n\n{项目}\n</INSTRUCTIONS>`（user 角色；仅全局时无 ` for {cwd}` 后缀）——实测与源码逐字符一致（附件 item5）。
- **环境上下文**：`<environment_context>` XML（字段见清单 #6；实测含 `<permission_profile type="managed">`）。
- **Skills 目录**：`<skills_instructions>## Skills / ### Skill roots / ### Available skills（- name: description (locator)）</skills_instructions>`＋"使用前必须完整读取 SKILL.md"的 progressive disclosure 规则（`catalog_prompt.rs:7-40`）。
- **模型切换**：`\nThe user was previously using a different model. Please continue the conversation according to the following instructions:\n\n{新指令}`（`<model_switch>`）；增量窗口用 `<model_instructions>`＋"The following instructions replace the previous base instructions:"。
- **AGENTS.md 差量声明**："These AGENTS.md instructions replace all previously provided AGENTS.md instructions." / "The previously provided AGENTS.md instructions no longer apply."（程序行为）。
- **压缩前兜底改写**：超窗时最旧工具输出体替换为 "Output exceeded the available model context and was truncated"（`compact_remote_history.rs:18-19`）。

### 5.3 「程序处理」与「提示词要求」的区分（修订）

- 文件优先级（AGENTS.override.md > AGENTS.md > fallback、根→cwd 拼接）是**程序结构性处理**（`agents_md.rs` 发现与拼接逻辑）。
- 「AGENTS.md 的语义优先级」（作用域、必须遵守）出现在**兜底基础指令 default.md:17-24**（AGENTS.md spec 段）；较新的 per-model 模板（gpt-6-astra 等）不再含该 spec 段、仅顺带提及 AGENTS.md；review rubric（`prompts/templates/review/rubric.md:48`）另有一句 "More-specific guidance wins on conflict"。这些是**提示词要求**，配套的程序机制是发现/拼接/差量替换。
- Skills 的 "must read SKILL.md completely" 是**提示词要求**，配套 `skills.read` 工具与分页才是程序机制。

---

## 6. 跨轮次与压缩的状态变化（具体小例子）

### 6.1 术语速释

- **历史（items）**：内存里旧→新的消息数组；**逻辑请求**每步全量携带它（线上可能 WS 增量）。
- **history_version**：历史被整体改写的代号（压缩/回滚 +1）；**reset_version**：破坏性替换专用。
- **context window（窗口）**：两次压缩之间的历史段；压缩＝换窗口。
- **reference_context_item / world_state_baseline**：两个「上次快照」，分别用于 turn 级设置 diff 和世界状态 section diff。

### 6.2 历史列表演化（小例子）

```
首轮请求:        [D初始包][U:AGENTS+ENV][U:"修一下登录bug"]
模型回复+工具:    追加 [Reasoning][A:"我先复现"][FunctionCall ls][FunctionCallOutput "src/…"]
同轮再请求:      逻辑全量＝以上全部（WS 增量时线上仅发新增尾部）
第二轮输入:      追加 [U:"还是不行"]（新 turn、新 client session＋新 turn_state 令牌；WebsocketSession
                 从 thread 级缓存移交，WS 续接按缓存状态＋兼容性判定，非必定全量）
cwd 改变:        追加 [U[2]: 替换声明<AGENTS_MD>+<ENVIRONMENT_CONTEXT>]（旧消息仍在历史，靠声明覆盖语义）
压缩:            整窗替换（§6.4），history_version+1、窗口号+1、recompute_token_usage
```

### 6.3 预算与触发（数值与适用分支，修订）

| 参数 | 值 | 适用分支/开关 | 来源 |
|---|---|---|---|
| 窗口大小 | 模型目录 `context_window`（缺省回退 `max_context_window`） | 所有分支 | `openai_models.rs:518-520` |
| 可用窗口 | 窗口 × `effective_context_window_percent`（per-model 字段，默认 95%） | 硬上限计算 | `openai_models.rs:393,523-527` |
| 自动压缩默认阈值 | 窗口 × 90%（目录可配 `auto_compact_token_limit`，clamp 到 90% 内；config `model_auto_compact_token_limit`＋`scope` 亦可覆盖） | 前台强制换窗判定 | `openai_models.rs:529-537`、`config_types.rs` |
| 后台投机压缩启动阈值 | 65%（`DEFAULT_ASYNC_COMPACTION_THRESHOLD_PERCENT`） | **仅** feature `AsyncCompaction` 开启、且**非** TokenBudget、且 provider `remote_compaction=V2`（`session/async_compaction.rs:40-48`） | `core/src/config/mod.rs:226` |
| PostTurn 压缩阈值 | `model_post_turn_compact_threshold_percent`，默认 0＝关闭 | turn 结束时判定 | `session/context_window.rs:111-117` |
| token 计数 | 服务端最近 total ＋ 末次模型项之后新增的本地估算（4 字节/token，"coarse lower bound, not tokenizer-accurate"） | 所有判定共用 | `history.rs:648-650,939-956` |
| 工具输出截断 | 入历史时按策略（gpt-6.1-sol/gpt-6-astra 目录均为 tokens:10000；unified exec 默认 10k tokens、×1.2 序列化余量）；**与压缩是两回事：截断改单个 item，压缩换整窗** | 活跃历史 | `history.rs:555-577` |

### 6.4 压缩三分支与重建顺序（修订，测试实证）

**触发分发**（`run_auto_compact`，`turn.rs:1514-1614`）：
1. **TokenBudget 分支**（feature `TokenBudget`）：**无模型调用**。pre-compact hooks → 发 `ContextCompaction` turn item → `start_new_context_window`（`session/mod.rs:4854-4910`：新窗口号＋`build_initial_context_with_world_state` 全量重建初始上下文＋可选保留客户端项〔retained tool outputs/developer messages，受 64k token 预算截断〕）→ `replace_compacted_history`。**不保留用户消息、不生成摘要**（token-budget reset 不做总结，`turn.rs:784-786` 注释）。
2. **远程 V2 分支**（provider `remote_compaction=V2`）：请求＝全量历史＋末尾 `ResponseItem::CompactionTrigger` 请求控制项（`compact_remote_v2_attempt.rs:172`）；超窗先改写最旧工具输出；服务端必须返回**恰好一个** `ResponseItem::Compaction{encrypted_content}`（密文摘要，`compact_remote_v2.rs:418-477`）；保留＝user/HookPrompt 消息＋agent 间消息（≤10k tokens）＋retained tool outputs（feature 门控），按 64,000 token 总预算从新到旧截断；`Compaction` 项为 新历史最后一项。
3. **本地摘要分支**（provider 不支持 V2）：请求＝全量历史＋`<SUMMARIZATION_PROMPT>` user 消息；产物＝`SUMMARY_PREFIX`＋最后一条 assistant 消息；新历史＝精选用户消息（≤20,000 tokens，`compact.rs:63`）＋摘要 user 消息。

**重建插入顺序**（本地/远程共用 `build_compaction_replacement_history` → `assemble_compaction_history`，`compact.rs:84-110`）：
- `split_prefix_updates`（`world_state/mod.rs:125-140`）把 `Placement::Prefix` 项（增量窗口的基础指令/工具声明）与普通初始上下文分离：**prefix 恒在新历史最前**，不参与插入位置计算。
- 普通初始上下文经 `insert_initial_context_before_last_real_user_or_summary`（`compact.rs:646-695`）插入：**后向扫描**找最后一条「真实用户/agent 消息或 retained 独立输出」（FINAL_ANSWER 型 AgentMessage 不算），插在它**之前**；无真实用户消息→插在摘要前；只有 Compaction 项→插在其前；都没有→追加。

**两条用户消息的真实顺序**〔测试〕`compact_tests.rs:698-813`（`assemble_compaction_history_keeps_prefix_first_and_summary_last`）：

```
输入：compacted=[U1"older user", U2"latest user", AgentMsg(FINAL_ANSWER), Summary]
      prefix=[BaseInstructions]，initial_context=[D"fresh permissions"]
输出：[prefix(BaseInstructions), U1, D"fresh permissions", U2, AgentMsg, Summary]
      └──────────────┬────────────┘
   初始上下文插在最后一条真实用户消息 U2 之前（FINAL_ANSWER 型 AgentMessage 不构成边界）
```

**失败处理**：本地 `ContextWindowExceeded` → 删最旧 item（保持配对）重试；预算/不完整响应 → 直接失败；其他错误退避至 provider 上限（默认 5）。远程 v2 重试上限 2；输出非恰含 1 个 Compaction 项 → Fatal；旧模型失败满足条件用当前模型重试一次。PostTurn 压缩失败保留已完成 turn 仅告警（`turn.rs:806-837`）。压缩结果持久化：`RolloutItem::Compacted(CompactedItem)`（含 replacement_history、窗口号、usage、comp_hash）＋WorldState＋TurnContext（`session/mod.rs:4394-4404`）；resume 从最后一个 Compacted 重建。

**rollout 持久化的截断边界（修订）**：64KiB 常量（`PERSISTED_MCP_RESULT_MAX_BYTES`/`PERSISTED_COMMAND_OUTPUT_MAX_BYTES`，`rollout/src/policy.rs:19-22`）**只作用于** `RolloutItem::EventMsg(ItemCompleted)` 里的 `TurnItem::CommandExecution.aggregated_output` 与 `TurnItem::McpToolCall.result` 字段（UI 事件镜像记录，`policy.rs:256-280`）；`RolloutItem::ResponseItem`（历史记录）经 `should_persist_response_item` 原样保留**进入活跃历史截断之前**的 ResponseItem（`session/mod.rs:3722-3760`：`record_annotated_items` 截断的是内存副本，rollout 写入用未截断 envelope）。注意这不还原工具自身的更早截断（exec 结果构造时的 10k token/1MiB 字节帽已在更早发生）。〔源码〕

---

## 7. 验证证据与未知项

### 7.1 实际运行（本轮，两轮共通）

| 命令 | 结果 |
|---|---|
| `git clone https://github.com/openai/codex.git`（blob:none 过滤） | 成功，固定于 `014489466a` |
| `RUSTUP_TOOLCHAIN=stable CARGO_TARGET_DIR=/tmp/codex-build-target cargo build -p codex-cli --locked`（`codex-rs/` 下；仓库 pin Rust 1.95.0 本机 rustup 无权限安装，改用系统 stable 1.99.0；补装 pkg-config/libssl-dev 后成功，5m37s） | ✅ 退出码 0，`codex-cli 0.0.0` |
| `CODEX_HOME=<tmp> codex debug prompt-input '帮我看下这个仓库'`（临时 HOME 写全局 AGENTS.md、临时 git 仓库 cwd 写项目 AGENTS.md；离线，无密钥） | ✅ 退出码 0，7 个 input item，完整输出存附件 `docs/records/2026-10-20-codex-context-assembly-prompt-sample.json`（63,820 字节原始输出＋捕获元数据） |

### 7.2 实际组装样本（本轮运行确认；完整件见附件）

**捕获阶段**：`codex debug prompt-input` → `codex_core::build_prompt_input`（`core/src/prompt_debug.rs:26-120`）返回的 **`Prompt.input`＝`sess.clone_history().for_prompt()`，即持久历史项**。配置：无 config.toml；CODEX_HOME 含全局 AGENTS.md；cwd 为含项目 AGENTS.md 的临时 git 仓库。模型：**gpt-6.1-sol**（第四轮核验更正：item1 指令文本与固定 commit 打包目录 **gpt-6.1-sol** 模板逐字节一致——21,769 字符/21,779 UTF-8 字节；`use_responses_lite=true`、`context_window=272000`、`truncation_policy=tokens:10000`。此前误归因 gpt-6-astra 系按模板开头前缀取首个命中：多模型共享相近开头。运行时默认模型解析＝可用列表中首个 picker 可见项，附件 item0 的 spawn_agent 模型列表以 gpt-6.1-sol 开头可证）。功能开关：`incremental_tools` **默认开**（Stage::Stable, `default_enabled:true`，`features/src/lib.rs:1631-1636`）→ 本次为 **IncrementalTools 增量窗口**。

**基础指令来源核验（第四轮，〔运行〕）**：① 二进制内嵌目录（`codex debug models --bundled`）与源码 `models-manager/models.json` 逐字节一致（构建忠实于固定 commit）；② 阻断网络（假代理）重跑 `debug prompt-input`，item1 与附件逐字节一致且 CODEX_HOME 未产生任何模型缓存文件——**无远端目录参与，来源即固定 commit 的打包模板（gpt-6.1-sol 条目）**；③ 与打包 gpt-6-astra 模板（21,420 字符）的 349 字符差异＝两模型模板本身的新增段落与措辞差（如 "Avoid unnecessary apologies and self-blame…" 段），非运行时改写。

```
0 additional_tools/developer id=at_…  3 个 namespace（functions[含 custom "exec" 编排工具]、clock、collaboration）
   ← TopLevelToolsState 首窗全量声明（历史 Prefix 项；at_ id 由历史边界 assign_missing_response_item_id 分配）
1 message/developer id=msg_…  "You are Codex, an agent based on GPT-6.…"（21,769 字符＝gpt-6.1-sol 模板全文，与打包目录逐字节一致）
   ← ModelInstructionsState::for_incremental_tools 首窗 Prefix 项；metadata 实证 content_item_kinds=["model.base_instructions"]，
     provenance={type:"harness"}（正是 BaseInstructionsFragment::text_from_item 的识别形态）
2 message/developer[3]  [01]<skills_instructions> [02]<permissions instructions> [03]<collaboration_mode>
3 message/developer     <multi_agent_role>You are `/root`, the primary agent…
4 message/developer     <multi_agent_mode>Any earlier instruction enabling proactive multi-agent delegation…
5 message/user[2]       [01]"# AGENTS.md instructions for /tmp/…/cwd\n\n<INSTRUCTIONS>\n全局测试指令：优先中文回复\n\n--- project-doc ---\n\n项目规则：测试目录\n</INSTRUCTIONS>"
                       [02]"<environment_context>…cwd/shell/current_date/timezone/filesystem…</environment_context>"
6 message/user          "帮我看下这个仓库"
```

**该入口未覆盖的后续处理**（修订，读 `prompt_debug.rs` 与 `client.rs` 对照得出）：返回值**未经过** `build_responses_request` 的请求级步骤——①普通窗口的基础指令前置／RLite 非增量的 AdditionalTools 前置（本样本窗口本就不加）；②`ConfigurationUpdate` 过滤/去重；③图片 detail 规整（`normalize_image_details`）；④非 RLite 的 `clear_content_item_metadata`；⑤非 OpenAI 的加密内容清理；⑥`include_internal=false` 时的 `tool_result_metadata` 剥离；⑦guardian prompt 处理；⑧`executed_tool_calls.attach_to_prompt`。此外 WS 增量改写也在此之后。调试入口覆盖不到的阶段（工具输出落历史后的再请求、压缩后请求）由 §7.3 快照证据补足。〔源码〕

### 7.3 快照与测试证据（原有覆盖，非本轮运行）

| 证据 | 证明内容 |
|---|---|
| `core/tests/suite/snapshots/all__suite__model_visible_layout__model_visible_layout_cwd_change_refreshes_agents.snap` | 请求级消息布局与 cwd 差量刷新（普通窗口形态：`00:developer <MODEL_INSTRUCTIONS>` 为请求前缀抽象） |
| `core/tests/suite/snapshots/all__suite__compact__mid_turn_compaction_shapes.snap`（及 pre_turn/post_turn/manual 系列） | 压缩三段式：压缩请求含全历史+SUMMARIZATION_PROMPT；续请求含重建初始上下文+COMPACTION_SUMMARY |
| `core/src/compact_tests.rs:698-813` | **两条用户消息时初始上下文插在最后一条真实用户消息之前、prefix 恒最前、summary 恒最后**（§6.4） |
| `core/tests/suite/snapshots/all__suite__scenarios__astra_kickoff_remote_compaction_windows.snap:134-175` | `$skill` 提及→`<skill>` 全文 user 消息、插件 developer 消息 |
| `core/src/context/world_state/snapshots/…top_level_tools…snap` | 增量工具声明/移除差量项的精确 JSON |
| `core/src/context/world_state/snapshots/…agents_md…snap` | AGENTS.md 新增/替换/移除三态文本 |
| `core/tests/suite/agents_md*.rs`、`agents_md_tests.rs`（50+ 用例，含 `uses_configured_fallback_when_agents_missing`） | 拼接顺序、32KiB 截断、UTF-8 容错、override 优先、**显式 fallback 配置**、不可信跳过 |
| `core/src/compact_tests.rs`、`compact_remote_v2.rs` 内嵌 tests、`core/tests/suite/compact_remote.rs` | 20k/64k 预算、保留形状、恰好 1 个 Compaction 项 |
| `ext/memories/src/tests.rs:175`（`prompt_contribution_uses_memory_summary_when_enabled`） | 记忆注入=1 个 developer_policy fragment（含 memory_summary 内容） |
| `core/src/client_tests.rs`（WS 增量相关） | `get_incremental_items` 判定与回退行为（未逐条展开，标〔未验证〕细目） |
| `core/src/thread_manager_tests.rs:1187,2161,2630` | fork 只用已选历史、resume/fork 不恢复环境 |

### 7.4 未知项 / 未验证

1. WS 增量的服务端行为（`previous_response_id`＋`store:false` 下服务端如何重建输入）：客户端只管发送，服务端实现不公开，未调查。
2. `ModelClient::summarize_memories`（`client.rs:798`，POST `/v1/memories/trace_summarize`）：仓库内仅单测调用，无生产接线点——用途未确认〔源码确认存在，生产用途未验证〕。
3. 记忆注入的逐窗口细节：`contribute_thread_context` 在压缩后的初始上下文重建时重注入；是否每个新窗口都重注入未动态验证〔推断〕。
4. 全局（CODEX_HOME）AGENTS.md 是否有独立字节上限：provider 内无长度检查，未见上游校验〔未确认〕。
5. `project_doc_max_bytes` 是否计入全局指令：代码上看预算只约束项目条目〔源码倾向，未见显式测试〕。
6. 稳定 item id 的**缓存效果**：源码注释只说明「重试与恢复会话保持 item 身份」（`client.rs:985-986`）；「利于服务端前缀缓存命中」是推断——显式缓存机制是独立的 `prompt_cache_key` 字段（=session_id，`client.rs:646-658`）。
7. 未公开的 ChatGPT 服务端行为（远程 v2 压缩服务端如何生成密文摘要等）：不适用客户端结论。
8. Windows 平台路径、`client_tests.rs` WS 增量用例逐条核对未展开。
9. 本机无法安装仓库 pin 的 Rust 1.95.0（rustup 目录权限），以 stable 1.99.0 构建；未发现版本相关差异，但存在理论偏差面。

---

## 8. 对我们项目的参考（仅提炼已被证据支持的机制）

> 本节只列机制与适用条件，不构成迁移方案，不改动 013/014 等现有规格。

1. **「常驻内容」与「请求级内容」分离，且分窗口模式**：普通窗口把基础指令/工具声明留在请求级（历史不含这两类内容，但仍可能有其他 host 注入片段）；增量窗口把二者写入历史（建窗时位于段首、后续变更追加到末尾）、请求字段留空。内容位置策略与网络流量是两个独立维度——网络字节由 WS 增量机制决定（与窗口模式无关），增量窗口的实际收益是「声明随历史一次写入、仅变更时追加」，代价是历史的自包含性依赖 Prefix 项、压缩重建需重放它们。我们 007 的会话恢复若引入系统级内容，需先定位置策略。〔源码〕
2. **世界状态 section 化＋基线 diff**：AGENTS.md、环境、权限、模型各自带快照，首轮 render_full、之后 render_diff 只发变化片段，配合「replace all previously provided…」覆盖声明语义（旧消息不删）。适用于 cwd/配置变化场景。〔源码+快照〕
3. **逻辑全量与线上增量分离**：WS 增量（previous_response_id＋新增 items）判定＝所比较字段不变＋当前 input 完整匹配基线前缀（可相等或追加）＋`WebsocketSession` 缓存可用（跨 turn 移交，重置条件明确）；恢复会话通常因无缓存状态而全量，是状态缺失的自然结果而非主动阻断；推理痕迹/测试断言全部基于逻辑全量。若我们做增量优化，同样需要「逻辑视图」与「线上视图」两层。〔源码〕
4. **工具输出截断的双层设计**：工具构造时自身截断（10k token）→ 入活跃历史时按模型策略截断（保留配对）→ rollout 保存进入历史截断前的 ResponseItem。三层各有明确边界，恢复时重截断。〔源码〕
5. **压缩三分支同构生命周期**：TokenBudget（无模型调用的窗口重置）/远程 V2（密文摘要＋保留预算）/本地（明文摘要＋20k 用户消息预算）共享 hooks、turn item、`replace_compacted_history` 持久化路径；重建顺序有确定规则（prefix 最前、初始上下文插最后一条真实用户消息前、摘要/Compaction 恒最后）。〔源码+测试〕
6. **call/output 配对修复在历史出口统一做**（合成缺失输出、剔孤儿），单一出口不变量比分散校验更可依赖，与我们 007 配对修复可互相印证。〔源码〕
7. **Skills 渐进披露**：目录常驻，正文仅显式提及时全文注入，否则靠 `skills.read` 工具读取。〔源码+快照〕
8. **AGENTS.md 的工程化细节**：根→cwd 拼接、目录内 override 优先、共享字节预算硬截断、不可信项目跳过、失败容忍；其他规则文件名**只能**显式配置 fallback（默认不发现 CLAUDE.md）——我们若做多规则文件兼容需自证需求。语义优先级在提示词层（default.md spec 段），程序层只做结构拼接。〔源码+测试〕
9. **token 计数混合口径**：服务端 last usage＋本地字节粗估（4B/token，自认 coarse lower bound）；阈值用百分比并区分适用分支与开关（65% 仅后台投机压缩且需 AsyncCompaction+V2）。〔源码〕

---

## 附：本轮修订说明（2026-10-20 第二轮）

**五处核心纠正**：
1. **WS 增量发送**：原「WS 显式 previous_response_id:None 全量发送」错误——`From` 转换中的 None 只是默认值；实际由 `prepare_websocket_request`/`get_incremental_items` 双模式决定（增量=previous_response_id+新增 items；全量=六类 NonIncrementalReason 时）。HTTP 恒全量。（第三轮再修正：续接可跨 turn，见下方追加记录。）
2. **基础指令/工具声明的两种路径**：原「基础指令每请求前置、不入历史」只对普通窗口成立；IncrementalTools 窗口二者均为历史 Prefix 项（世界状态写入），请求字段留空。原七项 debug 输出的 item0/1 即历史 Prefix 项（metadata `model.base_instructions` 实证）；prompt_debug 返回 Prompt.input，未经过 §7.2 列出的八类请求级处理。
3. **记忆注入**：原「只注入何时查记忆的指引、不含记忆正文」错误——V1/V2 模板均把 `memory_summary.md` 正文（2500 tokens 截断）嵌入 `{{ memory_summary }}` 段；V1 单片段、V2 按 ≤8900 bytes 分片；按需读取的是 MEMORY.md/rollout_summaries/skills 等详细记忆。
4. **压缩分支与重建顺序**：补全 TokenBudget（无摘要无用户消息的窗口重置）/远程 V2（CompactionTrigger→密文 Compaction 项）/本地（SUMMARIZATION_PROMPT→明文摘要）三分支；用测试实证「两条用户消息时初始上下文插在最后一条真实用户消息之前、prefix 恒最前」；区分窗口 Prefix 与普通初始上下文。
5. **续接与重试分界**：正常工具续接=外层 run_turn 循环再入（needs_follow_up）；失败重试=run_sampling_request 内层 loop（退避≤max_retries）；ContextWindowExceeded 不重试、上抛交压缩路径。

**收紧项**：CLAUDE.md/.cursorrules（默认不发现，仅显式 fallback 配置）；AGENTS.md 语义优先级（在兜底 default.md:17-24，非只在 review rubric）；rollout 64KiB（只作用于 EventMsg 镜像的 CommandExecution.aggregated_output/McpToolCall.result，ResponseItem 历史记录不截断）；「完整输出」（=进入历史截断前的 ResponseItem，不含工具自身更早截断的还原）；非 OpenAI 清理（passthrough 全清、encrypted 除 Reasoning/Compaction/ContextCompaction 全清、FunctionCall.encrypted_function_args 置 None）；稳定 id（身份保持=源码注释，缓存效果=推断，显式缓存=prompt_cache_key）；65% 等数值（补适用分支/开关/配置覆盖）。

**第三轮定点修正（2026-10-20）**：
1. **WS 状态生命周期**：`ModelClientSession` 每 turn 新建，但 `WebsocketSession`（连接、`last_request`、`last_response_rx`）经 `Drop`→thread 级 `cached_websocket_session` 缓存跨 turn 移交（client.rs:674-687 取出、1435-1441 存回）；每 turn 全新的只有 `turn_state` 路由令牌。重置条件＝auth owner 变化（client.rs:677-681）/WS 回退 HTTP（client.rs:736）/headers·connection_key 变化·连接关闭（client.rs:1686-1704）。删除「新 turn 首请求必定全量」「新 session 无 last_request」「增量只能同一 turn 内」等绝对结论——跨 turn 是否续接取决于缓存、连接状态与请求兼容性。
2. **WS 判定条件**：`restored_history` 是无上次请求时非增量遥测标签的来源（client.rs:2104-2114），不是主动阻断条件、不属于 `NonIncrementalReason` 枚举；恢复会话全量通常是新 thread 级 ModelClient 无缓存状态的自然结果。「严格扩展」修正为「完整匹配基线前缀，可完全相等（空新增列表）或追加」；「非 input 字段不变」收紧为「`responses_request_properties_mismatch` 所比较的字段不变」；补预热请求（`generate:false`，client.rs:2126-2131、2161）与首次生成请求的区分，预热响应 id 可作续接基线。
3. **工具声明位置**：§2#2 分组改正（顶层 tools＝普通 Responses；请求前缀 AdditionalTools＝RLite 非增量）；删除「声明永远在对话内容之前」——首窗/压缩重建时 Prefix 项在段首，后续变更声明经 `record_step_world_state_if_changed`→`record_conversation_items` **追加到历史末尾**（session/mod.rs:3782-3801），`Placement::Prefix` 不重排既有历史；§8 删除「历史纯对话」，内容位置策略与 WS 网络增量分别说明。
4. **记忆预算口径**：`memory_summary.md` 正文**先**按 2500 tokens 截断、**再**填入模板（prompts.rs:40-45）；不是模板＋摘要合计限 2500 tokens；V2 单片段 ≤8900 bytes 独立限制保留。

**第四轮核验与修正（2026-10-20）**：
1. **交付一致性**：上一轮在回复中手工粘贴的 JSON 正文与仓库文件存在 16 字节偏差（粘贴件 LF 归一后 66,052 字节/SHA `7ffae355…`，文件为 66,068 字节/SHA `381a6e1c…`）——差异源于**人工转写**而非文件变动；此后以磁盘文件为唯一权威附件，不再以手工复制正文交付。
2. **基础指令来源更正**：样本 item1 与打包目录 **gpt-6.1-sol** 模板逐字节一致（21,769 字符/21,779 字节）；此前「默认模型 gpt-6-astra／21,420 字节模板」归因错误（按开头前缀取首个命中）。证据链：二进制 `debug models --bundled` 内嵌目录＝源码文件；假代理阻断网络重跑结果与附件逐字节一致且无模型缓存写入（无远端目录参与）；与 astra 模板的差异是两模型模板本身的版本措辞差。字符数与 UTF-8 字节数已分别标注。
3. **两处残留**：§4.3 第 6 项删除「restored_history 标记使 WS 首请求走全量」，统一为缓存状态缺失（`NoPreviousRequest`）导致无法续接、标记仅用于诊断；§3.0 区分 `preconnect_websocket`（只建连、从不发送提示载荷，client.rs:1578-1590）与 `prewarm_websocket`（携带 Prompt 发送 `generate:false` 真实请求并等待 Completed，client.rs:2305-2353、2161）。

**第三轮纠正项的固定 SHA 源码链接**（前缀见 §1，路径相对仓库根）：
- 会话取出/移交：[`codex-rs/core/src/client.rs#L674-L687`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L674-L687)、Drop 回存 [`#L1435-L1441`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L1435-L1441)、HTTP 回退清缓存 [`#L719-L738`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L719-L738)、连接重置条件 [`#L1686-L1704`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L1686-L1704)、`WebsocketSession::reset` [`#L479-L490`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L479-L490)
- 判定与诊断标签：`get_incremental_items` [`#L1512-L1543`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L1512-L1543)、`NonIncrementalReason` [`#L356-L378`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L356-L378)、`responses_request_properties_mismatch` [`#L389-L435`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L389-L435)、`restored_history` 标签 [`#L2104-L2122`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L2104-L2122)、载荷构造与 warmup `generate` [`#L2123-L2167`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/client.rs#L2123-L2167)
- 差量追加路径：[`codex-rs/core/src/session/mod.rs#L3782-L3801`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/core/src/session/mod.rs#L3782-L3801)
- 记忆截断顺序：[`codex-rs/ext/memories/src/prompts.rs#L31-L62`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/ext/memories/src/prompts.rs#L31-L62)、V2 分片 [`codex-rs/ext/memories/src/extension.rs#L70-L101`](https://github.com/openai/codex/blob/014489466a1739c2c563cdfe57880c2c7c8680e3/codex-rs/ext/memories/src/extension.rs#L70-L101)

**证据方法**：五路并行源码调查＋主链人工精读＋本轮定点复核（WS 增量链、窗口模式判定、记忆模板、压缩插入顺序测试、续接/重试路径逐行核对）；实际组装样本完整保存为附件。工作区零改动（本报告与附件为仅有的新增/修订文件；AGENTS.md 等既有未提交改动未触碰）。
