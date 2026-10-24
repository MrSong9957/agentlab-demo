# 执行中消息接入计划：源码核对记录

- 日期：2026-10-23。
- 范围：[工具对比记录](2026-10-21-codex-tools-comparison.md)中的 `send_message_to_user_async`。
- 用户要求：编写执行计划；边界问题参考 Codex 既有做法。本轮不实施程序行为。
- 产物：[021 接入计划](../specs/021-send-message-to-user-async.md)。

## 1. 来源与基线

继续采用原对比的 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`。本地缓存 `/tmp/openai-codex-investigate` 的 HEAD 实际为 `014489466a1739c2c563cdfe57880c2c7c8680e3`；核对结论来自显式 `git show <固定提交>:<路径>`／限定路径的 `git grep`，没有切换或修改上游工作区。

另通过 GitHub 连接器读取同一固定提交的处理器、注册条件与关键测试断言，确认公开源文件与缓存对象一致。文件 blob SHA：

| 路径（均在 `codex-rs/` 下） | GitHub 返回的 blob SHA |
|---|---|
| `core/src/tools/handlers/send_message_to_user_async.rs` | `4d390028f15b81e600fd1d5a04858460f46b01ca` |
| `core/src/tools/spec_plan.rs` | `24b60f3ff2231a2e4a5e27dc7a93331a9aacd436` |
| `core/tests/suite/request_user_input_async.rs` | `48fa1668a013b79b089f26aa10e03b4637ea6d47` |

处理器严格解析唯一 `message`，trim 后拒绝空白；发出带调用 ID 的 Async AgentMessage，再返回 `{"accepted":true}`。工具注册只允许根线程，feature 或模型目录任一显式 opt-in；feature 默认关闭。上游测试另外断言通知不结束 turn、空白失败且无额外消息，以及下一请求不额外插入通知正文。具体固定链接集中于[计划 §3.1](../specs/021-send-message-to-user-async.md#31-codex-复用来源)，不把阅读上游测试记为本轮执行通过。

补读 `protocol/items.rs`、`session/mod.rs`、`app-server-protocol/protocol/thread_history.rs` 与 `app-server/in_process.rs`，确认消息元数据、事件投递和界面历史不同于 turn 完成；补读 `session/turn_input.rs`、`session/turn.rs`、`session/input_queue.rs` 及 TUI 的 `input_submission/input_queue/input_flow`，确认中途普通输入由独立接续机制处理，不由通知处理器等待或关联答案。

## 2. 本地核实与适配依据

调研开始时 `/workspace` 仍有其他任务的 019／020 文档改动；核对期间其他任务将 main 推进至 `6b8f3a310c0a0748239641b127c6fdb46ec8d2db`（`feat: 020 CLI 用户附图（五入口＋完整附件链路）`）。本轮重新读取实际源码后固定该计划基线，不执行该提交／合并，不调整其他任务状态。

| 已读源码／契约 | 对计划的直接影响 |
|---|---|
| `prompt.rs`、`turn.rs` 的声明与分派 | 没有消息工具；继续使用每轮能力快照和模式隔离。工具调用原文与结果已经足以保存通知，不需新增会话类型 |
| `turn.rs` 的 ToolEnd、批末与纯文本结束 | 当前通知先于结果入史，不可宣称崩溃后恰一次；普通输入须在完整工具批之后接入，并参与结束判定 |
| `tui_host.rs`、`main.rs`、`chat_driver.rs` 的事件与输入 | 当前 Final 会结束任务，不能用来显示通知；Enter 运行中仍排队，须接通普通输入通道才能回应中途追问 |
| 020 `SubmittedTurn`、图片准备和回填 | 已有文字＋路径＋TextElement 结构，接续必须复用；不重建图片功能，不退化为文本 |
| `catalog.rs` 的 ModelInfo／overlay_model | 采用上游模型目录 opt-in 分支，复用可选字段覆盖和刷新规则，不引入全量 feature 系统 |
| 007、018、020 与既有测试 | 保留调用配对、专用回答、未知结果和草稿恢复；明确修订 Enter 的普通输入意图，显式排队仍保持 |

本地没有 Codex 的完整 App Server／异步会话框架，因此计划将消息投影放在原 ToolEnd 路径；普通输入采用每轮独立接续通道与同步批次边界。行式 CLI／管道提供单向消息输出，不伪称有运行中键盘输入能力。边界取舍已写入计划，没有另留需用户逐项确认的选项。

## 3. 本轮修改与验证

只新增本记录与 021 计划，更新工具对比中的该行并在 `project-lessons.md` 追加一条来源经验；没有修改其他规格、历史证据或真实配置。原有删除项和无关未跟踪文件保留。

本轮文档检查通过：4 份文档共 39 条本地链接（其中 6 条含锚点）可解析，2 段 JSON 示例有效，14 个唯一 Codex 源码路径均存在于固定提交；UTF-8、末尾换行、代码围栏与行尾空白检查错误 0。`git diff --check` 通过，工具对比只改本条，项目经验只追加本条。

`git diff --exit-code -- src Cargo.toml Cargo.lock config` 通过，检查时 HEAD 仍为 `6b8f3a3`；本轮未改程序、测试、依赖或真实配置。原有删除项不是本轮修改，未计入本功能差异。本功能的 A01–A15 均为待验证，不计入任何既有通过数量。未运行 Cargo、代码 lint、功能测试、门禁、提交、推送或发布。

## 4. 实施前复核与分支准备（2026-10-23 第二轮）

范围：仅文档与 Git 准备；未改程序、测试、依赖，未提交、合并、推送或删除分支，未启动 `/goal`。

复核结论：

- 固定提交 `d138398` 在本地缓存 `/tmp/openai-codex-investigate` 可用；计划 §3.1 的 14 个源码路径在该提交全部存在（`git cat-file -e` 逐一路径核实）。
- 抽查三处固定源码，与计划逐条一致：处理器 `send_message_to_user_async.rs` 为 `deny_unknown_fields`＋唯一 `message`、`trim()` 后空白报 `message must not be empty`、发出 `id=call_id`／`phase=FinalAnswer`／`delivery=Async`／`questions=None` 的 AgentMessage 并返回 `{"accepted":true}`；注册条件 `spec_plan.rs` 为根线程且 feature 或模型目录任一 opt-in；测试 `request_user_input_async.rs` 断言空白失败回给模型、下一请求含调用与 accepted 且**无**同文 assistant message、消息发出后任务继续到真正 TurnComplete。
- 计划符合固定五节；任务要求的五项重点（参数/返回值/开关/轮次、显示与保存恢复且恢复不重发、运行中接续与排队及晚到归属、与问询/批次/终答顺序、A01–A15 可观察结果＋状态＋副作用）均有对应条款，源码依据与待验证结论分开。

真实缺项与补齐：计划缺少 020 §5.1 同款的显式 L1/L2/L3 定义表（Supervisor 复核后作为 TDD 依据的公开边界）。已在规格新增「5.1 测试边界与等级」，原 5.1–5.3 顺延为 5.2–5.4；无其他重复计划或缺项，调研记录 §3 的既有检查未改写。

020 交付核验：`feat: 020 CLI 用户附图（五入口＋完整附件链路）` 已提交并快进合入本地 main（`6b8f3a310c0a0748239641b127c6fdb46ec8d2db`，领先 origin/main 11 个提交，未推送）；本地 020 特性分支已不存在。残留登记 `/tmp/terminal-chat-020-cli-image-input`（detached 于同一提交、工作区干净）属未完成的清理，本轮在既有授权内移除该 worktree 登记，未删除任何分支。

任务工作区与分支：`/workspace`（git 根），普通分支 `feat/021-send-message-to-user-async` 自 `main@6b8f3a3` 建立，未使用 worktree。工作区既有他任务改动原样保留：6 份 `docs/records/*.txt` 的未暂存删除、对照表该行与 `project-lessons.md` 追加、4 份无关未跟踪记录；本轮新增改动仅规格的 L1/L2/L3 小节与本记录本节。

本轮文档检查：`git diff --check` 通过；4 份文档本地链接（含锚点）全部可解析，规格 2 段 JSON 示例有效，代码围栏配对、行尾空白、CR、末尾换行 0 错误；`#31-codex-复用来源` 锚点未受小节顺延影响。未运行 Cargo、代码 lint 或功能门禁。
