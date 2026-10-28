# Codex 可执行工具与本项目对比

- 调研日期：2026-10-21。
- 状态：初次调研与讨论；2026-10-26 复核下表实现状态，2026-10-27 同步 033 沙箱前置与待实施计划。本轮仅修订文档。
- 上游基线：openai/codex `d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`。
- 统计口径：模型可调用的工具，不将 Rust crate、本机命令或仅有展示类型的能力计入。

建议优先补充 **进程会话、计划管理、结构化问询和 MCP 接入**。文件读取、搜索、Git、构建与测试已有 shell 入口，无须逐项包装成新工具。

本次核对的是 Codex 开源仓库 [`d138398cef7d` 提交](https://github.com/openai/codex/commit/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4)（2026-10-21）。工具是否开放取决于模型、配置、运行模式和扩展，无法给出适用于所有会话的固定数量。

初次调研时（2026-10-21），本项目普通用户轮只有 `exec_command`、`apply_patch` 两项；`mem_read / mem_write / mem_list` 仅供内部记忆合并使用。[工具定义](../../src/app/src/prompt.rs)与[执行分派](../../src/app/src/turn.rs)均确认了这一边界。

> 2026-10-26 复核：下表按当前源码及已有交付记录更新；后文早期优先级建议和图片说明保留其历史语境，不作为当前实现清单。本轮未重新运行功能测试。剩余缺口与首项选择见[核对记录](../logs/029-file-patch/2026-10-26-spec-research.md)。

> 2026-10-27 同步：033 已交付且共享 dev 已部署、默认启用，替代旧双账号执行链路；029/031/034/035 的规格与计划已据此修订。六项功能仍待实现，032 仍为 034/035 的前置；原表行名、顺序和任务粒度不变。变更范围与待执行清单见[同步记录](2026-10-27-sandbox-spec-sync.md)。

根据 Codex 的[核心注册表](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/spec_plan.rs#L1234)，主要差异如下：

| 能力 | Codex 工具 | 本项目与建议 |
|---|---|---|
| 命令执行、进程交互 | `exec_command`、`write_stdin` | [016](../specs/016-process-sessions-plan.md) 已实施并批准交付（2026-10-22）：可续接进程会话、后续输出与输入、PTY、取消及回收；2026-10-26 复核代码入口仍存在 |
| 文件补丁 | `apply_patch` | 基础功能已有；JSON 参数为主动适配。按本行作为一项任务，[029 规格与实施计划](../specs/029-file-patch.md)统一覆盖剩余 `Move to` 与 Unicode 匹配容差；两者不另拆任务（[差异依据](2026-10-22-codex-apply-patch-comparison.md)）。2026-10-28 已按 029 M1–M6 实现完毕（Move 语法/文件操作与部分成功/第四级匹配/声明/保存展示与沙箱隔离；红绿＋变异＋攻防证据见 [docs/logs/029-file-patch/](../logs/029-file-patch/)），**待验收**；004 契约相应条款已同步修订 |
| 任务计划 | `update_plan` | [017](../specs/017-update-plan.md) 已实施并批准交付（2026-10-22），保留 8 次请求额度；2026-10-26 复核工具定义、执行与计划状态模块均已存在（[实施证据](2026-10-22-017-implementation.md)） |
| 用户问询 | `request_user_input`、`request_user_input_async` | [018](../specs/018-user-input.md) 阶段 A 同步问答已本地交付（2026-10-23），任务分支已清理；独立回答通道、关闭自动空答。阶段 B 异步问询已实施并获批准本地交付（2026-10-23）：TUI 七工具（异步发布＋本地等待）、单项未决、F6 回答窗口（[证据](2026-10-23-018b-implementation.md)） |
| 执行中发消息 | `send_message_to_user_async` | [021](../specs/021-send-message-to-user-async.md) 已实施（2026-10-23）：模型目录 opt-in、accepted 即时确认、消息不结束轮、无额外注入、/resume 只展示不重发；运行中 Enter 接续当前轮＋显式排队保留（[证据](2026-10-23-021-implementation.md)）。任务完成后的响铃、桌面提醒由应用另行实现 |
| 查看图片 | `view_image` | [019](../specs/019-view-image.md) 已实施（2026-10-23）：按路径加载真实图片并传给模型（Chat Completions 批末 user 附件）、能力开关、保存恢复与 TUI 卡片（[证据](2026-10-23-view-image-impl.md)） |
| MCP 外部工具 | 按服务器动态注册工具 | [022](../specs/022-mcp-tools.md) 已实施并批准本地交付（2026-10-24）：stdio／Streamable HTTP、每轮快照、原始身份路由、真实调用与凭证污染拒绝（[证据](2026-10-24-022-mcp-tools-implementation.md)）；资源接口、工具搜索另定范围 |
| MCP 资源 | `list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource` | [023](../specs/023-mcp-resources.md) 已交付本地 main（2026-10-25）：三接口、分页、读取及宿主接入；2026-10-26 复核实现与回归用例均存在（[实施证据](2026-10-24-023-mcp-resources-implementation.md)） |
| 工具搜索与按需加载 | `tool_search` | 已建立 [030 规格与实施计划](../specs/030-tool-search.md)（待实现，待用户审阅）：本地 BM25 搜索、动态 MCP 工具按需声明、五协议普通函数适配及请求级执行集合；当前代码仍全量发送定义，尚未实施。固定上游机制见本文末尾 |
| 权限申请 | `request_permissions` | 已建立 [031 规格与实施计划](../specs/031-request-permissions.md)（待实现，待用户审阅）：扩展 033 统一沙箱入口，补齐受限读/网络/系统调用策略、可信基线/上限、专用确认、本轮授权与到期收尾；同 UID 下的永久拒绝及别名边界须先验证，当前尚无实际权限申请入口 |
| 上下文管理 | `get_context_remaining`、`new_context` | [032 规格](../specs/032-context-management.md) 已实现（2026-10-28，待验收）：离线预算估算/未知、显式交接摘要、批末活动视图切换、V3 完整历史检查点与提交回执；不重置逻辑轮/环境；五协议计量与发送共用纯定型（出站字节不变） |
| JavaScript 工具编排 | `exec`、`wait` | 已建立 [034 规格与实施计划](../specs/034-javascript-tool-orchestration.md)（待实现，待用户审阅）：本地 QuickJS 子进程、五协议普通函数适配、受限工具代理、增量 wait 与 V3 调用证据；032 会话基建为实施前置，当前尚未实施 |
| 多 Agent | `spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`interrupt_agent`、`list_agents` | 已建立 [035 规格与实施计划](../specs/035-multi-agent.md)（待实现，待用户审阅）：六工具统一覆盖一层子会话、本地3子并发、消息/续派/等待、中断收尾、额度及 V3 来源记录；032 会话基建为实施前置，当前尚未实施 |

Codex 还包含 `clock.curr_time`、`clock.sleep`、`wait_for_environment`、`list_available_plugins_to_install`、`request_plugin_install` 等条件启用的工具。旧版多 Agent 接口另有 `send_input / resume_agent / close_agent`，属于版本差异，不需要同时照搬。[多 Agent 定义](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_spec.rs)

网页搜索和图像生成需要单独看待：

| 能力 | Codex 接入方式 | 对本项目的意义 |
|---|---|---|
| 网页搜索 | 托管 `web_search` 或外部扩展 `web.run` | 具有独立的服务依赖；增加工具名称并不能获得搜索能力 |
| 图像生成 | 扩展 `image_gen.imagegen` | 依赖提供方能力和授权，适合作为可选扩展 |

上述区别可见[网页搜索定义](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/hosted_spec.rs)与[扩展注册及图像生成条件](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/spec_plan.rs#L782)。

按本项目当前阶段，建议这样排序：

1. **先补进程会话与 `write_stdin`。** 当前执行默认超时 10 秒、最多 60 秒，stdin 关闭，没有 PTY。长测试、开发服务器和交互程序都会受限。需要支持“返回会话 ID → 继续读取输出或输入 → 明确结束状态”，同时保留现有超时、取消和进程回收语义。[当前限制](../../src/app/src/exec.rs)、[Codex 接口](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/shell_spec.rs)

2. **再补 `update_plan`。** 它不增加系统执行权限，接入成本相对较低，能直接改善多步骤任务的进度展示。项目目前还会主动移除基础提示词中的计划工具指引，接入时需同步调整。[现有适配](../../src/app/src/prompt.rs)

3. **补结构化问询。** 先复用 Codex 同步工具的选项和回答结构，接通界面及专用回答回送，等待用户提交后继续原任务；需明确适配上游普通模式的自动空答行为。异步问询后置，不能把当前普通输入排队当作已具备晚到回答能力。详见[问询复核](2026-10-22-codex-user-input-comparison.md)。

4. **接入 MCP，再按实际用途增加搜索、网页读取等工具。** 本项目当前采用 Chat Completions；通过 MCP 或普通函数工具接入外部服务，更容易保留现有多提供方架构。工具定义占用可接受时，继续全量提供；需要减少这部分上下文时，再单独设计 `tool_search` 的搜索和按需声明机制。

Codex CLI 已支持 [`-i 图片路径`](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/utils/cli/src/shared_options.rs#L10)、[粘贴图片](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/chatwidget/clipboard.rs#L14)、[拖入或粘贴图片路径](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/bottom_pane/chat_composer.rs#L1260)、[`@` 选图](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/bottom_pane/chat_composer.rs#L2515)；模型也能用 [`view_image`](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/view_image.rs#L95) 按路径看图。本项目目前[只传文本](../../src/app/src/prompt.rs)，还需接通图片内容和会话保存，并使用能看图的模型。

暂时不建议新增独立的 Git、测试、目录和文本搜索工具，也不建议扩大用户侧记忆工具范围。若以后需要跨平台文件接口或受限的只读模式，再考虑 `read_file / search_files / list_dir`。多 Agent、代码编排和插件安装则可以后置。

这些建议会扩展现有 [003 执行契约](../specs/003-exec-command.md)与 [006 调用循环契约](../specs/006-tool-call-loop.md)，后续应先明确新契约。原调研仅做只读检查，未修改代码、安装依赖或运行测试；本次仅保存报告。

## 工具搜索与按需加载（2026-10-24 源码复核）

沿用本文固定的 Codex 提交。以下讲普通工具调用模式：模型支持工具搜索、允许使用 `tool_search`，且存在可搜索的工具时，才开放这个入口。[注册条件](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/spec_plan.rs#L392)

核心理解如下：

- **直接可用的工具，包括已开放的 `tool_search`：**先把完整定义交给模型，即名称、说明和参数格式；函数工具的参数格式就是 Schema。[初始工具声明](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/spec_plan.rs#L628)、[搜索入口定义](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/tool_search_spec.rs#L16)
- **按需加载的工具：**完整目录留在 Codex 宿主。模型可以先看到来源的名称和简述，例如“mail：提供邮件收发能力”，不必先逐个看到所有工具的名字和用法。来源名、简述允许缺省，来源列表也允许省略，所以这不是每个工具都有一份简介的保证。[目录与来源信息](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tools/src/tool_search.rs#L50)、[来源概览的生成](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/tool_search_spec.rs#L34)
- **搜索命中后：**Codex 把匹配工具的名称、说明和参数格式放进搜索结果，再交给下一次模型请求，模型才据此调用具体工具。[搜索处理](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/tool_search.rs#L275)、[结果进入模型上下文](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/context.rs#L218)

这样省下的是尚未使用的工具定义占用的模型上下文，宿主仍保留工具目录。这里的“完整定义”指调用说明，不包含工具实现代码或资源正文。

“来源”是提供这一组工具的服务、应用或命名空间。在已核对的 MCP 路径中，优先用应用连接器名，否则用配置的 MCP 服务名。它与本地／云端的部署位置是两个概念；Codex 内置工具也不因此属于某个 MCP 服务器。[MCP 来源名](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/mcp.rs#L186)

模型可以用“发送邮件”这类用途词搜索，不需要先知道准确工具名。Codex 用 BM25 对目录文本排序，搜索字段包括工具名、服务器名、说明和参数字段名等；具体工具目录由运行时提供，无须预先把用户安装的工具训练进模型。[搜索排序](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/tool_search.rs#L153)、[MCP 搜索字段](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/mcp.rs#L593)

本项目目前没有这套按需加载流程：[定义构造](../../src/app/src/mcp/catalog.rs)保留名称、说明和参数 Schema，[请求组装](../../src/app/src/prompt.rs)每次加入本轮全部可用工具定义。这与 MCP 的 `tools/list` 和 023 的资源接口是独立能力。

以上结论来自固定源码，只修订文档；未实施工具搜索。官方 API 的逐函数延迟、服务端搜索及 Codex 代码编排属于其他路径，不能把它们的初始可见信息直接套到本文流程。
