# view_image 最小接入计划：源码核对记录

- 日期：2026-10-23。
- 状态：只读调研及文档规划，未实施功能。
- 本地 HEAD：`8c60b10`；工作区已有 018 相关未提交改动，按当前工作区核对，不改写这些程序和测试。
- 需求来源：[Codex 工具对比](2026-10-21-codex-tools-comparison.md)中的“查看图片”项。
- 产物：[019 实现计划](../specs/019-view-image.md)。

## 1. 固定上游与关键事实

源码通过本地 `/tmp/openai-codex-investigate` 的 Git 对象库读取，对每个文件显式使用 `git show d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4:<path>`。该目录 HEAD 为另一个提交，未将其工作树或 HEAD 当作本次基线，也未切换或修改该仓库。

下列路径均位于固定提交的 `codex-rs/` 下：

| 文件 | 实际核对结果 |
|---|---|
| `core/src/tools/handlers/view_image_spec.rs` | 基础面必填 `path`；`detail`、`environment_id` 按选项开放；普通函数输出与 code mode 输出是不同封装 |
| `core/src/tools/handlers/view_image.rs` | 图片模态检查先于文件读取；相对环境 cwd 解析；普通文件、读取和解码检查；成功返回结构化 `input_image`；`log_output` 只记载荷长度 |
| `core/src/image_preparation.rs` | 图片准备集中执行；普通 `high` 使用 `PromptImageMode::HIGH_DETAIL`，不是旧用户附件入口的单纯 `ResizeToFit` |
| `utils/image/src/lib.rs` | 内容识别与解码；HIGH_DETAIL 为 2048／32 像素 patch／2500 patches；保留可用 PNG/JPEG/WebP 源字节、RGB ICC 和 EXIF 处理，GIF 转 PNG；有不含缓存的处理函数 |
| `protocol/src/openai_models.rs` | `InputModality` 包含 text/image/audio；上游缺省为 text＋image，本地多提供方能力未知时不能据此推定支持图片 |
| `core/tests/suite/view_image.rs` | 工具成功结果内含图片，明确断言不另插图片消息；解码真实请求图片验证尺寸，2304×864 缩为 2048×768；另有无效图片、目录、缺失文件和 text-only 模型用例 |
| `utils/image/src/image_tests.rs` | 小图字节保留、横／竖图缩放、元数据、GIF 转换、错误输入与尺寸预算用例可函数级移植 |
| `tui/src/history_cell/patches.rs` | 查看图片卡片和构造函数；只需适配本地活动 ID、路径与宿主投影 |

文件的固定提交链接集中列在[计划复用表](../specs/019-view-image.md#2-codex-复用清单)，不以仓库最新版本替代。

## 2. 本地必须适配的约束

| 当前实现 | 对计划的约束 |
|---|---|
| [prompt.rs](../../src/app/src/prompt.rs) `HistoryMessage`、`ToolResult.content` 为字符串，`project_history` 逐条投影；`display` 明确禁止进入模型请求 | 需要独立可选图片载荷；不能把图片塞进展示字段或字符串摘要 |
| [turn.rs](../../src/app/src/turn.rs) 按批记录调用与串行结果；018-B 的真实回答在批末进入历史 | 请求投影必须在完整配对结果之后附图，不在批中插入 user 消息；保留晚到回答次序 |
| [api.rs](../../src/app/src/api.rs:171) 最终出站 JSON 上限 10 MiB | 包括 base64 膨胀与全部历史，不能只限制原图片文件，也不能用 64 MiB 会话行上限当请求上限 |
| [catalog.rs](../../src/app/src/catalog.rs:26) 当前没有输入模态，模型字段按来源逐项覆盖 | 最小增加可选模态与两层能力判断；不根据名称推断，不新增网络探测 |
| [session.rs](../../src/app/src/session.rs:18) V1 JSONL、64 MiB 行上限、未知字段容差及批配对修复 | 在原结果行内存实际准备后的图片；读取旧会话兼容，旧程序重存会丢新增字段须说明 |
| [memory/rollout_input.rs](../../src/app/src/memory/rollout_input.rs:148) 共用聊天投影，单条 1 MiB、总量 8 MiB | 记忆投影前清图片；不扩大预算，超单条上限的图片记录仍跳过 |
| [history_cell/patches.rs](../../src/tui/src/history_cell/patches.rs:161) 已保留 `ViewImageHistoryCell`，仅构造函数和事件入口被裁；[chat_driver.rs](../../src/tui/src/chat_driver.rs) 尚无图片宿主项 | 恢复现有卡片入口即可，无需移植整个上游图片输入系统 |
| [src/tui/Cargo.toml](../../src/tui/Cargo.toml)、[Cargo.lock](../../Cargo.lock) 已含 image 0.25.10、base64 0.22.1 和四种相关解码特性 | app 仅增加已有锁版本的直接依赖，优先函数级移植，不引入上游缓存和附件服务 |

## 3. 官方协议依据与取舍

已实际打开 [OpenAI Codex Prompting Guide 的 view_image 示例](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide#view_image)，核对最小路径参数面；上游新提交中的可选字段仍以源码为准。

已实际打开 [OpenAI Chat Completions create 接口](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)，其 tool 内容部件只支持文本，user 消息支持 image_url。因本项目仍用此协议，计划选择完整 tool 批次后追加宿主标注的 user 图片附件，仅在请求投影中生成。**这是协议适配决定，不是 Codex 原有工具输出行为，也不代表两种角色语义等价。**

其余必要取舍：模型能力未知时关闭；首版只开放路径与 high；图片内联原结果以避免附件管理；20 MiB 输入／64 MiB 解码器分配预算／4 MiB 准备后字节为拟定本地限制；历史图片累计仍受原 10 MiB 请求上限。以上均为计划，尚无运行结果。

## 4. 本轮验证

- 4 份相关文档的 41 条本地链接（含锚点）可解析，15 个唯一 Codex 固定提交源码路径存在；检查错误 0。
- `git diff --check` 通过；文档内容复核确认区分上游行为、本地协议适配和后续待验收项。
- 对开始时已有未提交改动的 5 份已跟踪文件记录 SHA-256：`turn.rs`、`user_input_async.rs`、018-B 变异证据和 evidence-index 保持原字节；`project-lessons.md` 的原始前缀保持原字节，仅追加本次规划依据。
- 本轮新增计划与本记录，原对比表只更新 view_image 一行，项目经验只追加一条来源说明；未切分支、未安装依赖或修改 lockfile。
- 本轮未执行 TDD、功能测试、加固、安全测试、代码审查或 Cargo 全量验证；按 AGENTS.md，纯文档规划只检查内容、命名与链接。

## 5. CLI 图片输入讨论落盘（2026-10-23）

用户要求将六种入口及待补范围保存到文档，并澄清 `@` 选图是否等于搜索后调用 `view_image`。内容集中于 [019 §9](../specs/019-view-image.md#9-cli-图片输入补充讨论2026-10-23)；本轮只修改文档，不执行入口实施或代码验证。前文 §1–§4 是最初规划阶段的历史记录；后续工具实施状态见[实施记录](2026-10-23-view-image-impl.md)。

本轮沿用固定提交，通过 `git show`／`git grep` 追加核对：

- `tui/src/app/clipboard.rs` 的粘贴动作实际调用 `chat_widget.paste_image`；`clipboard_paste.rs` 读取图片数据／文件，生成 PNG。
- `tui/src/bottom_pane/chat_composer.rs::insert_selected_file_path` 对有效图片调用 `attach_image`；`chatwidget/input_submission.rs` 将本地附件加入 `UserInput::LocalImage` 或准备后的图片项，未走 `view_image` 工具分派。
- `bottom_pane/chat_composer/paste_input.rs` 调用 `handle_paste_image_path`；拖入产生路径时与普通路径粘贴共用此实现，不是独立的拖拽协议。
- `utils/cli/src/shared_options.rs` 定义 `--image/-i`；`exec/src/lib.rs` 将图片路径映射为 `UserInput::LocalImage`，不是仅有未接线的参数声明。
- `core/src/tools/handlers/view_image.rs::to_response_item` 返回关联 `call_id` 的 `FunctionCallOutput`，图片位于其 `InputImage` 内容项。
- 本地 `chat_driver.rs` 仍以 `InputResult::Submitted { text, .. }` 提取文本，并调用文本提交接口；保留附件辅助函数不等于用户附图链路已接通。既有 `view_image` 工具处理与用户直接附图可以共享底层图片准备逻辑，消息来源及记录仍需分别保留。

本轮文档验证：

- 3 份修改文档的 33 条本地链接（含锚点）可解析，15 个唯一 Codex 固定提交源码路径存在，检查错误 0；新增六种入口的计数与两条提交链路已按源码复核。
- 三份文档均为末尾追加，原有内容前缀的 SHA-256 不变；`git diff --check` 和文档空白检查通过。
- 对开始时记录的 36 份文件核对，35 份文件或文档原始前缀保持原字节。`src/tui/src/chat_driver.rs` 在本轮文档编辑期间有并发变化，本轮未写入或回滚该文件；复核其提交分支仍只传文本，文档中的待补判断仍成立。
- 本轮只写入 019 §9、本记录 §5 及项目经验中的一条链接说明；没有运行 Cargo、功能测试或改写既有实施证据。

## 6. 共用核心结构与附件细节复核（2026-10-23）

用户要求检查共用核心结构是否明确写入 019，并参考 Codex 源码补齐功能与细节。原 §9.2 已说明函数复用和两个提交路径，本轮补为 [§9.4 结构图](../specs/019-view-image.md#94-多个入口共用核心与两种出口)及 [§9.5 细节清单](../specs/019-view-image.md#95-参考-codex-仍需补齐的功能与细节)。只更新文档，不实施用户图片入口。

继续使用 `d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`，本轮实际阅读的新增依据：

| 固定提交中的 `codex-rs/` 路径 | 已核实事实及适配边界 |
|---|---|
| `core/src/image_preparation.rs`、`utils/image/src/lib.rs` | 图片准备同时覆盖 Message 与工具结果；入口采集和消息封装仍独立，不能称六个入口从头到尾只有同一个函数 |
| `protocol/src/user_input.rs`、`tui/src/chatwidget/user_messages.rs` | 用户输入带结构化图片／本地图片；纯图片也构成有效内容；完整消息保留文本、图片、来源及编号，合并时重映射附件标记 |
| `tui/src/bottom_pane/chat_composer/attachment_state.rs` | 附件按加入顺序保存；删除标记时移除附件并重排编号；提交前过滤没有对应结构化标记的本地附件；同路径可以被再次加入 |
| `tui/src/chatwidget/input_queue.rs`、`tui/src/bottom_pane/chat_composer_history.rs` | 队列保存完整消息；会话内历史可保存图片信息，跨会话文本输入历史不能自动等同完整会话图片恢复 |
| `tui/src/chatwidget/input_submission.rs` | 支持无文字但有图片的提交；模型不支持图片时恢复文字和附件并提示，不静默扔图继续发文本 |
| `tui/src/chatwidget/image_submission.rs`、`tui/src/app/startup.rs` | 远程工作区才启用 `snapshot_local_images`，并非所有本地提交默认快照；准备过程串行，失败／取消恢复原消息，按提交 ID 丢弃迟到通知；32 MiB 图片预算针对上游传输，不是本项目的限额 |
| `tui/src/clipboard_paste.rs` | 路径规范化支持单个带引号／转义路径、file URL 和 Windows／UNC；临时 PNG 通过 keep 保留，WSL fallback 不代表 Docker 剪贴板已接通 |
| `tui/src/bottom_pane/chat_composer.rs` | 外部编辑后同步筛除、重编号附件；plain_text 配置关闭图片与文件弹窗，可复用该边界保护问询回答 |

本地核对：共用核心已有 `view_image.rs::prepare_image` 与普通文件／有界读取路径；驱动排队仍为 `VecDeque<String>`，用户附件尚需贯通。冻结提交字节、队列图片预算、持久化可选字段及临时文件清理列为本地适配细节，不冒称上游所有模式已有相同契约；不改写工具首版的实现与测试结论。

本轮文档验证：

- 3 份修改文档的 39 条本地链接（含章节锚点）可解析，22 个唯一 Codex 固定提交源码路径存在，检查错误 0；结构图与新增清单已作内容复核。
- `git diff --check` 和文档空白检查通过；三份文档修改前的原始内容前缀保持原字节。
- 对开始时记录的 36 份文件核对，35 份文件或文档前缀保持原字节；原工具对比记录在文档编辑期间发生并发变化，本轮未写入或回滚该文件。
- 本轮只追加 019 §9.4／§9.5、本记录 §6 及项目经验中的一条来源说明；没有修改程序、测试或锁文件，也没有运行 Cargo 或程序测试。

## 7. 四项细节按 Codex 方案收敛（2026-10-23）

用户要求优先采用已有 Codex 方案，不把冻结时机、队列预算、持久化字段和清理规则留作额外审核。本轮更新 019 §9.5 并补 [§9.6 明确规则](../specs/019-view-image.md#96-四项具体规则优先复用-codex2026-10-23)；这是文档方案收敛，未实施新的图片入口。

本轮继续逐文件核对固定提交 `d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`：

- `chatwidget/input_flow.rs`、`input_submission.rs`、`user_messages.rs`、`input_queue.rs`：队列保存完整消息及 `LocalImageAttachment` 路径；已核对实现无全队列图片字节计数或对应拒绝分支。
- `protocol/src/models.rs::from_user_input`、`core/src/session/mod.rs::response_item_from_user_input_with_image_positions`／`record_user_prompt_and_emit_turn_item`：本地路径在构造正式提交内容时读取，随后准备图片，再将准备后的内容入历史及持久化。该路径明确替代先前“入队即冻结”的本地建议。
- `tui/src/chatwidget/image_submission.rs`：32 MiB 为单次远程图片准备的剩余传输预算，不是全队列预算；当前本地流程不移植此传输栈，也不新增全队列字节上限，继续使用项目现有单图限制及 10 MiB 请求上限。
- `protocol/src/protocol.rs::UserMessageEvent`：提供 `images`、`image_details`、`local_images` 和 `text_elements` 等字段；`rollout/src/policy.rs` 保留消息项，core 先准备后入史。按本项目范围只映射 `images: Option<Vec<String>>` 与已有 `TextElement`；固定 high 无需另加 detail 数组，内存中的读取路径不冒充可恢复的图片内容。
- `tui/src/clipboard_paste.rs::paste_image_to_temp_png`：写入 PNG 后 keep；`attachment_state.rs` 删除图片仅移除引用，已核对的取消／提交／退出相关路径未为 keep 后的文件增加自动删除。因此先复用保留行为，不自行增加 GC 或退出删除，明确系统临时目录可留存文件。

调查中一次全仓 `git grep` 触发了部分克隆的惰性取对象及后台维护，搜索范围过大，已停止本轮该只读搜索；清理行为的结论限定为实际读取的图片粘贴、附件删除与取消提交路径，不用未完成的全仓搜索证明所有上游模块均无其他机制。

本轮文档验证：

- 3 份修改文档的 41 条本地链接（含锚点）可解析，27 个唯一 Codex 固定提交源码路径存在；2 段 JSON 示例语法有效，检查错误 0。
- `git diff --check`、文档空白和代码围栏检查通过；§9.5 已同步替换四项待定措辞与对应验收，§9.6 明确列出已采用规则及必要的本地字段映射。
- 32 份其他已记录文件保持原字节，本轮只修订 019、本记录及对应项目经验；未运行 Cargo、功能测试或修改程序、测试与锁文件。
