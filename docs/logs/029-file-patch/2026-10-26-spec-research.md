# 029 文件补丁：状态核对与规格依据

- 日期：2026-10-26。
- 工作目录：`/workspace`。
- 任务边界：核对指定工具表的真实缺口；以原表「能力」列的一行为一项任务，本轮只完成「文件补丁」的规格与实施计划，等待用户同意后才处理下一项。未实现功能、未运行代码检查或测试、未提交。
- 产物：[029 规格及实施计划](../../specs/029-file-patch.md)。

## 对照表核对

下表是源码与既有规格/交付记录的静态核对，不代表本轮运行验收。待处理内容沿原表顺序；不把仅有系统提示、展示类型或 shell 可执行命令计作模型工具。

| 原表能力 | 当前证据 | 本轮判定 |
|---|---|---|
| 命令执行、进程交互 | [016](../../specs/016-process-sessions-plan.md)已批准交付；`proc_session.rs` 维护会话，`prompt.rs` 声明且 `turn.rs` 分派 write_stdin | 已实现；原表「缺少」过时 |
| 文件补丁 | [004](../../specs/004-apply-patch.md)及 `apply_patch.rs` 明确拒绝 Move to；`seek_sequence` 只有三级容差 | 基础功能已有；本项规格与实施计划同时覆盖剩余 Move to 和 Unicode 匹配容差，不拆成不同任务 |
| 任务计划 | [017](../../specs/017-update-plan.md)已批准交付；`plan.rs`、工具声明和分派均存在 | 已实现；原表「尚未实施」过时 |
| 用户问询 | [018](../../specs/018-user-input.md)、`user_input.rs` 与 `user_input_async.rs` | 同步与异步已实现；保留宿主/模式条件 |
| 执行中发消息 | [021](../../specs/021-send-message-to-user-async.md)、`message_to_user.rs` | 工具已实现；完成后的响铃/桌面通知属于另行应用能力，不计入本轮工具缺口 |
| 查看图片 | [019](../../specs/019-view-image.md)、`view_image.rs` | 已实现；受模型能力开关控制 |
| MCP 外部工具 | [022](../../specs/022-mcp-tools.md)、`mcp/catalog.rs`、`mcp/manager.rs` | 已实现；资源和按需搜索单独核对 |
| MCP 资源 | [023](../../specs/023-mcp-resources.md)已交付；`mcp/resources.rs` 定义三接口，管理器与宿主已有接入；有 `tests/mcp_resources.rs` | 已实现；原表「缺少」过时 |
| 工具搜索与按需加载 | `prompt.rs` 与 `mcp/catalog.rs` 全量构造本轮工具；用户轮注册和分派中无 tool_search | 未实现，留待后续逐项规划；[024](../../specs/024-native-model-apis.md)已有多协议实现，不能继续以「只支持 Chat Completions」作为路线前提 |
| 权限申请 | 用户轮工具声明和分派无 request_permissions；[027](../../specs/027-credential-access-boundary.md)提供执行身份隔离，不是模型权限申请工具 | 未实现，后续须配合执行环境权限机制制定规格 |
| 上下文管理 | 已有 `context.rs` 处理注入/预算相关逻辑，但没有 get_context_remaining / new_context 工具入口 | 表中两项工具未实现；不能将此写成项目完全没有上下文处理 |
| JavaScript 工具编排 | 用户轮无 JavaScript 组合工具的 exec / wait；shell exec_command 和 write_stdin 是不同能力 | 未实现；保留原表暂缓建议 |
| 多 Agent | 用户轮无独立 Agent 会话及 spawn_agent 等六工具入口 | 未实现，后续独立规划 |

## 首项选择与范围

用户明确要求按原表项目分任务，一项任务包含规格文档和实施计划。分项单位固定为「能力」列每一行，保留原名称，不再按内部差异另立任务。上一轮按可独立验收的缺口拆分，是错误的任务划分，已纠正。

按原表顺序核实并跳过已实现项后，本轮处理「文件补丁」；其规格与计划覆盖 Move to 和 Unicode 匹配容差两部分，统一收录于 `029-file-patch.md`。原先移动专用的文件名和日志目录已同步更名，编号仍为 029。

剩余任务按原表名称及顺序为：文件补丁 → 工具搜索与按需加载 → 权限申请 → 上下文管理 → JavaScript 工具编排 → 多 Agent。本轮不建立其他项目的规格或实施计划；只有用户同意后，才处理下一项「工具搜索与按需加载」。

既有工作区含多处未提交代码和文档，尤其是 027 隔离入口相关改动。本轮不改这些文件，也不把它们当作缺失能力重新实现。029 计划必须兼容读取时已存在的隔离子进程链路。

## 已读取的关键依据

- [规格规则](../../../rules/规格文档实战法.md)：采用固定五节、待实现状态、预测与实测分开；实施计划放在规格 §3.2。
- [差异复核](../../records/2026-10-22-codex-apply-patch-comparison.md)：JSON 封装是主动适配；禁止覆盖与四字段结果须保留；Move 需要明确冲突、部分成功、展示和保存语义。
- [当前解析/执行](../../../src/app/src/apply_patch.rs)：`Hunk::Update` 无目标路径；`update_lines` 拒绝 Move；`run/apply_hunk` 每块只有一条成功结果；读入、资源边界和 fd 判型已有实现。
- Unicode 依据：通过 GitHub 连接器实际读取固定提交的 [seek_sequence.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/apply-patch/src/seek_sequence.rs) 第四级比较。确认 28 码点映射、双侧 trim 后比较、严格级别全范围优先；本地尚无该级。规格已写明字符范围、原文保留、负例、优先级及 Move 组合验收，未把它提升为新的表格项目。
- [工具与展示数据](../../../src/app/src/prompt.rs)：只有 patch 参数；已有 Add/Delete 展示类型。复用两条记录可避免改动会话枚举及引入新显示组件。
- [已有回归](../../../src/app/tests/apply_patch.rs)：`invalid_patches_error_without_touching_files` 中有合法 Move 被当作不支持操作的旧用例；实施时须将这项预期改为正例，并保留重复、错位和空目标的非法用例，不能删去整组验证。
- [隔离回归](../../../src/app/tests/apply_patch_isolated.rs)：真实 `--internal-apply`、sudo 子进程与 uid 证据链已经存在；Move 应走相同执行身份，不能在主进程先读源。
- [会话](../../../src/app/tests/session.rs)与 [TUI 投影](../../../src/tui/src/chat_driver.rs)：已有 display 往返、旧记录降级及 Add/Delete/Update 渲染，可扩充行为用例；[PTY 验收](../../../src/app/tests/e2e_acceptance.rs)已有 S28 补丁和 S34 部分成功流程。
- 上游使用 GitHub 连接器按明确提交读取：`openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4` 的 [parser.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/apply-patch/src/parser.rs)与 [lib.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/apply-patch/src/lib.rs#L550)。确认可选 move_path、先写目标再删源及块内部分成功记录；没有用当前上游 HEAD 代替固定提交。
- [OpenAI Docs](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)已实际打开并核对 Move 语法；不将 Responses 内建 apply_patch 的操作对象混成本项目 JSON 函数工具契约。

## 文档检查

当前版本的原始终端日志：[2026-10-26-doc-check-row-scope.log](2026-10-26-doc-check-row-scope.log)。检查原表项目映射、两处缺口是否都进入同一规格和计划、编号、五节结构、UTF-8、链接、末尾换行和空白，以及指定记录的 diff；具体命令、工作目录、退出码与原始输出以该日志为准。

保留的 [2026-10-26-doc-check.log](2026-10-26-doc-check.log) 是更名前、仅覆盖移动的旧版文档检查原始输出，其中旧路径和旧范围仅代表当时状态，不能作为修订版验证结果。

当前没有本项两部分的运行结果、性能基线或安全测试结论；规格中的功能验收全部保持「待测」。
