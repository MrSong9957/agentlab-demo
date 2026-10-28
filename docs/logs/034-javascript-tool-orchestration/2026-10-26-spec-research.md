# 034 JavaScript 工具编排：规格依据

- 日期：2026-10-26；工作目录：`/workspace`。
- 授权范围：原表「JavaScript 工具编排」一项，`exec`、`wait`共同形成[规格与实施计划](../../specs/034-javascript-tool-orchestration.md)。不推进下一项「多 Agent」，不实施前面各项。
- 状态：待实现；本轮只读本地/上游源码与官方资料、编写文档，没有安装依赖、运行JS/模型/MCP、构建或执行功能测试。
- 033已存在独立工具沙箱规格/证据，本项采用034，没有覆盖033及其他已有工作区改动。

## 当前实现与前置核对

| 核对点 | 依据 | 对本项的约束 |
|---|---|---|
| 工具循环 | [turn.rs](../../../src/app/src/turn.rs)固定8请求/32批次调用，整批校验后串行执行；现有宿主分支处理问询、图片、消息和MCP | 不能用shell冒充JS；先抽最小叶子分派，控制工具仍保留宿主批末边界 |
| 当前依赖 | [Cargo.toml](../../../src/app/Cargo.toml)没有JS引擎 | 当前无编排功能，新增引擎/传递依赖在实施轮锁定，本轮不安装 |
| 调用与历史 | [prompt.rs](../../../src/app/src/prompt.rs)用AssistantToolCalls/ToolResult配对，NativeAssistant携带原生状态 | 内层动作不是提供方产生的tool_calls，须另存宿主事件、过滤后才做原生批次配对 |
| 工具身份 | [mcp/naming.rs](../../../src/app/src/mcp/naming.rs)保存净化后的模型名；[turn.rs](../../../src/app/src/turn.rs)通过快照条目原始身份调用 | tools使用精确模型名作键，不做第二次JS净化或拆分路由；复用原污染/取消/参数检查 |
| 进程语义 | [016](../../specs/016-process-sessions-plan.md)与[proc_session.rs](../../../src/app/src/proc_session.rs)已支持跨调用进程，取消等待保留句柄 | 终止JS不是终止所有业务进程，保留已发生事实和真实句柄 |
| 隔离入口 | [proc_session.rs](../../../src/app/src/proc_session.rs)的build_tool_command有027隔离分支；[033](../../specs/033-tool-sandbox.md)评估尚未实现且当前环境受阻 | 新worker始终清环境/关fd，已配置身份隔离不可绕过；不在本项改环境或迁移沙箱 |
| 会话与下游 | 当前[session.rs](../../../src/app/src/session.rs)只接V1/V2、行上限64 MiB；[记忆读取器](../../../src/app/src/memory/rollout_input.rs)另有版本门与8 MiB/1 MiB预算 | 不能只加内存事件就宣称可恢复，须使用可报告失败的保存入口并更新所有读取器 |
| 既有规划 | [030](../../specs/030-tool-search.md)为请求级执行集合；[032](../../specs/032-context-management.md)已规划V3与提交回执，仍待实现 | 034明确以032基建为实施前置，关闭上下文工具仍可复用基建；新cell只能见创建时请求已开放目录，wait不扩权 |

工作区已有AGENTS、文档迁移、029–033与配置相关改动。已先查看git status，本项不暂存、提交、回滚或改写这些工作。读取与检查的实际命令/退出码见[终端记录](2026-10-26-doc-check.log)。

## 实际读取的固定上游

GitHub连接器读取 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`：

- [execute_spec.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/execute_spec.rs)为freeform源码＋pragma；[wait_spec.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/wait_spec.rs)为JSON控制参数。[description.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode-protocol/src/description.rs)说明新V8 isolate、无Node/直接I/O、辅助函数、增量wait及让出后持续运行；[runtime.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode-protocol/src/runtime.rs)的exec/wait缺省等待均10000ms。本项数值/输入/辅助函数范围按本地规格，不等同当前对话工具说明或上游全部特性。
- [execute_handler.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/execute_handler.rs)、[wait_handler.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/wait_handler.rs)和[delegate.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/delegate.rs)将cell与宿主step/分派关联；delegate保留创建时step，即让出后也不随wait获得另一次请求的全部能力。
- [code-mode/lib.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode/src/lib.rs)、[Cargo.toml](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode/Cargo.toml)与[remote_session.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode/src/remote_session.rs)显示该crate是host会话客户端，provider惰性启动配置的host程序；仅引入crate或handler不等于引入V8引擎。

已实际打开[OpenAI Docs编程式工具调用](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)：托管V8运行代码，客户端仍执行其工具；Responses原生program链和Agents托管harness是独立接入路径。此处仅据此确认编排/执行职责，不把托管能力、费用和模型支持承诺给本地五协议适配。

## 成熟引擎选择与未测事项

GitHub读取[rquickjs v0.14.0清单](https://github.com/DelSkayn/rquickjs/blob/d7ef5eeae702fea24c03643064de454f1c1dd4b0/Cargo.toml)，tag对象解引用提交为 `d7ef5eeae702fea24c03643064de454f1c1dd4b0`，版本0.14.0、MIT、最低Rust1.87；features支持只开std/futures。实际打开的[固定版本AsyncRuntime文档](https://docs.rs/rquickjs/0.14.0/rquickjs/struct.AsyncRuntime.html)说明中断/内存/栈控制，特别标注自定义allocator令set_memory_limit失效。docs.rs的Cargo.toml.orig访问失败，改用上述固定GitHub源码核实，没有据失败链接猜版本。另读[QuickJS原作者文档](https://bellard.org/quickjs/quickjs.html)确认嵌入API与std/os模块应区分，不把原作者当前版本与rquickjs实际绑定版本混为一谈。

关键本地适配是JSON exec、QuickJS、文本输出、单未消费cell、让出后暂停新业务受理、直接控制工具和V3宿主证据。完整接口、限额、前置和验收只在规格定义，不复制第二份清单。运行时的编译可用性、内存限制实际生效、IPC/取消/隔离与性能均未测，M0/M1须用真实引擎和子进程核实。

## 文档验证

只检查本项/对照表的内容、五节、编号、JSON、实施计划、验收待测、链接与空白。首次[终端记录](2026-10-26-doc-check.log)保留源码文本核对及失败输出：链接检查器误把行内代码 `tools[entry.name](args)` 当成Markdown链接；已修正检查器为先排除代码片段，再检查实际链接，没有删除或放宽真实链接断言。复查的实际命令、工作目录、退出码与原始输出见[2026-10-26-doc-check-2.log](2026-10-26-doc-check-2.log)。文档检查不代表032前置已完成、JS功能已实现或运行验收通过。
