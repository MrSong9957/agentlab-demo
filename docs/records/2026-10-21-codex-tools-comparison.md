# Codex 可执行工具与本项目对比

- 调研日期：2026-10-21。
- 状态：调研与讨论；未实施工具能力变更。
- 上游基线：openai/codex `d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`。
- 统计口径：模型可调用的工具，不将 Rust crate、本机命令或仅有展示类型的能力计入。

建议优先补充 **进程会话、计划管理、结构化问询和 MCP 接入**。文件读取、搜索、Git、构建与测试已有 shell 入口，无须逐项包装成新工具。

本次核对的是 Codex 开源仓库 [`d138398cef7d` 提交](https://github.com/openai/codex/commit/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4)（2026-10-21）。工具是否开放取决于模型、配置、运行模式和扩展，无法给出适用于所有会话的固定数量。

本项目普通用户轮目前只有 `exec_command`、`apply_patch` 两项；`mem_read / mem_write / mem_list` 仅供内部记忆合并使用。[工具定义](../../src/app/src/prompt.rs)与[执行分派](../../src/app/src/turn.rs)均确认了这一边界。

根据 Codex 的[核心注册表](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/spec_plan.rs#L1234)，主要差异如下：

| 能力 | Codex 工具 | 本项目与建议 |
|---|---|---|
| 命令执行、进程交互 | `exec_command`、`write_stdin` | 已有同步执行；缺少进程会话、后续取输出和输入能力。**优先补齐** |
| 文件补丁 | `apply_patch` | 已有，继续复用；JSON 参数为主动适配，`Move to` 与 Unicode 匹配容差暂缺，无需紧急改造（[2026-10-22 复核](2026-10-22-codex-apply-patch-comparison.md)） |
| 任务计划 | `update_plan` | 确实缺少；已制定仅覆盖该功能的 [017 接入方案](../specs/017-update-plan.md)，尚未实施，保留 8 次请求额度（[2026-10-22 复核](2026-10-22-codex-update-plan-comparison.md)） |
| 用户问询 | `request_user_input`、`request_user_input_async` | [018](../specs/018-user-input.md) 阶段 A 同步问答已本地交付（2026-10-23），任务分支已清理；独立回答通道、关闭自动空答。阶段 B 异步问询已实施并获批准本地交付（2026-10-23）：TUI 七工具（异步发布＋本地等待）、单项未决、F6 回答窗口（[证据](2026-10-23-018b-implementation.md)） |
| 执行中发消息 | `send_message_to_user_async` | 缺少，暂缓接入。用于及时告知重要情况或回应中途追问，发完继续做，不调用提问工具、不等回复。任务完成后的响铃、桌面提醒由应用另行实现 |
| 查看图片 | `view_image` | [019](../specs/019-view-image.md) 已实施（2026-10-23）：按路径加载真实图片并传给模型（Chat Completions 批末 user 附件）、能力开关、保存恢复与 TUI 卡片（[证据](2026-10-23-view-image-impl.md)） |
| MCP 外部工具 | 按服务器动态注册工具 | 未接入用户轮。适合成为外部能力的统一入口 |
| MCP 资源 | `list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource` | 缺少；与 MCP 工具调用属于不同接口 |
| 工具发现 | `tool_search` | 缺少。工具数量较多、需要按需加载时再补 |
| 权限申请 | `request_permissions` | 缺少。必须与执行环境的权限控制配套 |
| 上下文管理 | `get_context_remaining`、`new_context` | 缺少。应与上下文预算、压缩及历史管理一起设计 |
| JavaScript 工具编排 | `exec`、`wait` | 缺少。用于组合工具调用，与 shell 的 `exec_command` 不同；暂缓 |
| 多 Agent | `spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`interrupt_agent`、`list_agents` | 缺少。涉及独立会话、调度、取消和资源管理，适合后续阶段 |

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

4. **接入 MCP，再按实际用途增加搜索、网页读取等工具。** 本项目当前采用 Chat Completions；通过 MCP 或普通函数工具接入外部服务，更容易保留现有多提供方架构。初期工具少时，可直接提供完整定义，暂不引入 `tool_search`。

Codex CLI 已支持 [`-i 图片路径`](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/utils/cli/src/shared_options.rs#L10)、[粘贴图片](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/chatwidget/clipboard.rs#L14)、[拖入或粘贴图片路径](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/bottom_pane/chat_composer.rs#L1260)、[`@` 选图](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/bottom_pane/chat_composer.rs#L2515)；模型也能用 [`view_image`](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/view_image.rs#L95) 按路径看图。本项目目前[只传文本](../../src/app/src/prompt.rs)，还需接通图片内容和会话保存，并使用能看图的模型。

暂时不建议新增独立的 Git、测试、目录和文本搜索工具，也不建议扩大用户侧记忆工具范围。若以后需要跨平台文件接口或受限的只读模式，再考虑 `read_file / search_files / list_dir`。多 Agent、代码编排和插件安装则可以后置。

这些建议会扩展现有 [003 执行契约](../specs/003-exec-command.md)与 [006 调用循环契约](../specs/006-tool-call-loop.md)，后续应先明确新契约。原调研仅做只读检查，未修改代码、安装依赖或运行测试；本次仅保存报告。
