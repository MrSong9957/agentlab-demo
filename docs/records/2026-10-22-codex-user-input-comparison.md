# 用户问询与 Codex 的差异复核

- 日期：2026-10-22。
- 范围：[工具对比记录](2026-10-21-codex-tools-comparison.md)中的「用户问询」。
- 上游基线：`openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`，通过本地缓存的 `git show <提交>:<路径>` 读取。
- 本项目现状：017 已交付，普通用户轮为 `exec_command / apply_patch / write_stdin / update_plan` 四工具，两个问询工具均未接入。
- 状态：源码、规格与已有测试源码复核；本次仅修改文档。
- 后续实施：[018 用户问询分阶段接入](../specs/018-user-input.md)直接作为契约；阶段 A 同步问答已本地交付（2026-10-23），任务分支已清理；阶段 B 未实施。[实际来源与验证](2026-10-22-018-user-input-implementation.md)。下文保留接入前的差异复核。

## 结论

原表「缺少，可先做同步问询，异步版本后置」的判断成立。建议先复用 Codex 的 `request_user_input` 参数、回答结构和问询界面，补齐“提问 → 等待 → 用户回答 → 回传模型 → 继续原任务”这一条链路。

这里的同步是模型等待回答，界面仍须能输入、取消和退出。它比单向发布计划多了回答回送与等待生命周期，不能仅增加工具定义或打印问题。

## 两种工具实际做什么

| 项目 | `request_user_input` | `request_user_input_async` |
|---|---|---|
| 输入 | `questions`，每题含 `id / header / question / options`；选项含 `label / description` | `questions`，每题含 `title`，可选字符串数组 `options` |
| 回答方式 | 选项问答，归一化时要求每题有非空选项，并启用“其他”文字回答入口 | 可提供建议选项，也可省略选项成为纯文字问题 |
| 工具何时返回 | 通过宿主发布问题，等待回答通道收到结果；取消则回传错误 | 发布带 `Async` 投递标记的问题消息后立即返回 `{"accepted":true}` |
| 返回内容 | `{"answers":{"问题id":{"answers":["回答"]}}}`，由问题 ID 关联 | 只确认问题已接受发布，不包含用户回答；后续答复仍需宿主投递给对话 |
| 模型是否继续 | 工具结果到达前不进入依赖该结果的后续请求 | 可以继续独立工作，需另处理晚到回答与当前工作的关系 |
| 开放条件 | 有实验配置和可用模式检查；Default 模式可由功能开关开放，并非必须先实现 Plan Mode | 还检查模型是否声明支持该工具，以及是否为根线程；不是对所有模型无条件提供 |

上游同步接口的“1–3 个问题、2–3 个选项”等写在工具说明中，不能全部当成处理器已经强制校验的约束；实际强制项、归一化和本地边界应在实施规格中逐项确认。

## 复用时必须区分的行为

1. **同步接口与弹窗能力不同。** 底层弹窗支持纯文字问题，但模型侧同步工具的归一化要求每题提供非空选项。不能因弹窗支持文字输入，就宣称同步接口可以直接省略 `options`。
2. **普通模式也能开放。** `request_user_input_available_modes` 可通过 `DefaultModeRequestUserInput` 开放 Default；本项目无需为这个单一功能引入 Plan Mode，但需要适配工具描述和宿主开放方式。
3. **普通模式不等于无限等待。** 固定版本处理器在 Plan 模式设 `isBlocking=true`，其他模式设为 `false`，处理器本身仍等待回答通道。TUI 对非阻塞请求在无交互时先有 60 秒宽限、再有 60 秒倒计时，到期提交空回答；交互可停用该自动处理。若本项目首版目标是“收到用户回答再继续”，建议只复用阻塞问答所需部分，关闭自动空答；这是需要明示的本地适配，不能说成原样照搬普通模式。
4. **Codex 源码与 API 示例不是同一契约。** OpenAI 官方文档将 `request_user_input_async` 作为应用自定义工具示例，并说明宿主负责显示问题、收集与返回回答；其示例使用 `async:true` 和 `task_handle`，不同于本次固定 Codex 处理器的即时接受确认。应选定一种契约，不能混用，也无需为接入同步问答迁移 API。[官方异步工具说明](https://developers.openai.com/api/docs/guides/async-tool-calling)

## 本项目实际缺什么

- [prompt.rs](../../src/app/src/prompt.rs) 和 [turn.rs](../../src/app/src/turn.rs) 没有两个问询工具的定义或分派；现有通知只有 Info、ToolBegin、ToolEnd，尚无需要回答的请求通道。
- [bottom_pane/mod.rs](../../src/tui/src/bottom_pane/mod.rs) 中问询弹窗模块标为 `008-TRIM`；[history_cell/mod.rs](../../src/tui/src/history_cell/mod.rs) 的问答结果 cell 也被裁剪。协议 shim 中仍有部分问询名称的空壳类型，不代表功能已具备。
- [chat_driver.rs](../../src/tui/src/chat_driver.rs) 的 `DriverEvent / DriverHooks` 没有结构化问题及回答回送接口。当前任务运行期间输入进入 `queued_inputs`，等本轮结束后才处理。若等待问询结果时仍把回答放入这个队列，会形成“任务等回答、回答等任务结束”的互相等待；必须给当前问询单独回送回答。
- [main.rs](../../src/app/src/main.rs) 已有 `prompt_y_n`，但它只处理宿主的 yes/no 确认，不能承载模型的多题选项、文字回答与工具结果配对。

## 最小接入建议

1. **首版只增加同步工具。** 复用 Codex 的问题、选项、回答结构及必要解析／归一化；通过现有 Chat Completions function 工具面提供，不同时加入异步版或 Plan Mode。内部记忆合并轮仍保持原受限工具面。
2. **恢复必要界面并补回答通道。** TUI 复用上游选项、文字补充和结果显示的必要部分，适配本地事件与回送通道；行式 CLI 用现有输入框收集选择和文字。等待仅阻塞当前工具执行，不冻结界面，也不把回答作为下一轮普通聊天排队。
3. **先明确等待结束条件。** 推荐实际提交后才回传回答，取消／EOF／退出明确结束等待，不凭预选项或时间流逝推断用户已回答。没有可用交互通道时明确返回错误，不无限等待或误读后续聊天输入。TUI 保持 Esc 取消当前轮、Ctrl+C 退出的现有按键职责，不能直接搬入上游不同的按键行为。
4. **沿用历史与循环。** 回答通过配对的工具结果回传并保存；恢复历史只展示，不重新提问。待回答状态随当前请求结束而清理，迟到回答不得命中新轮的问题；继续保留 8 次模型请求、32 条单批调用及现有取消契约。

同步问询接通后，异步版仍需单独定义“问题发布后模型继续、回答晚到时何时送入上下文、取消与任务结束后如何处理”等行为。当前排队输入机制尚不具备这些语义，后置合理。

## 后续实施应证明的行为

- 本地 mock 驱动真实工具循环：问题确实出现，提交前不继续执行依赖回答的工作；提交后回答按问题 ID 回传，模型继续原任务。
- 真实 CLI／PTY 覆盖选择、其他文字、多题、取消、EOF、退出及无交互通道；等待时界面可响应，推荐项未提交不能变成回答。
- 回答与会话、当前轮和工具调用正确关联；取消、退出或切换后不会留下等待线程、错误消费排队输入或接受迟到回答。
- 保存恢复不重新提问，问答展示与实时一致；非法参数、内部记忆轮拒绝和终端控制字符过滤按现有边界验证。

## 源码依据与本轮验证范围

- [同步工具定义和归一化](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/request_user_input_spec.rs)、[同步处理器](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/request_user_input.rs)、[问题与回答协议](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/protocol/src/request_user_input.rs)。
- [模式开放](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tools/src/tool_config.rs)、[工具注册条件](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/spec_plan.rs)、[等待与回答回送](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/session/mod.rs)。
- [异步处理器](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/request_user_input_async.rs)、[问询弹窗及其测试](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tui/src/bottom_pane/request_user_input/mod.rs)。已读上游普通模式非阻塞测试、自动处理逻辑和本地输入队列源码，不将源码中的测试视为本次执行结果。
- 前轮复核的文档验证：当时涉及的 3 份文档共 23 条本地链接均可解析，8 处源码引用均确认存在于固定提交；命名、UTF-8、末尾换行、空白和 `git diff --check` 通过。未运行代码检查或测试，未实施问询工具；本轮计划验证另记于 018。
