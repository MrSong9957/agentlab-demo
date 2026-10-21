# 规格：TUI 工具调用详情展示（命令与文件差异）

> 状态：**已实现并本地验证（2026-10-15 实施轮，待审阅候选）**　分支 `feat/008-codex-tui-port`，未提交。全量 `cargo check --workspace && cargo test --workspace --no-fail-fast`＝**2712 通过 / 0 失败 / 3 忽略**（基线 2688＋24 新增；e2e 46/46 含 S27–S34）。
> **2026-10-15 审计修订**（详见 `docs/records/2026-10-15-012-implementation.md` §7）：终验数字经日志逐行复核无误；分项修正为 e2e 9＋A7 3＋apply_patch 4＋turn 1＋tui_host 2＋chat_driver 5＋session 0（原稿 chat_driver 6/session 1 与总数 24 不闭合，session 1 实为从未运行的嵌套死测试，审计已激活并按真实装载契约修正夹具）；另修复既有测试竞态（探针 fork 的 fd 继承窗口，测试设施级，生产无对等问题）。
> 需求来源：用户明确要求"工具调用显示实际内容：终端命令至少显示命令；文件修改显示修改前后的差异"。
> **契约归属**：008 §1"输入"原契约已列"工具活动（exec_command 运行中/输出/结束、apply_patch 结果）"，且 §1 移植范围已含 `history_cell/dynamic（exec 输出）、patches（apply_patch diff）`——本能力＝**008 既有契约的实现缺口补回**（当前实现只回推单行 Info），本规格细化其行为与验收，008 头部登记引用。
> 母本：[openai/codex](https://github.com/openai/codex) 固定提交 `97771b1d78cf6366bc3946fe9e4bbca4ebee31d1`（本地 `/tmp/codex-upstream`）。
> **需求基线（2026-10-15 用户实施指令，逐条落实）**：①命令展示实际 cmd、执行状态及已有执行结果；②文件展示实际路径和真实新增、删除内容；③diff 来自执行时的**真实前后内容**，不用原始补丁猜实际行号或修改结果；④**取消显示为取消，不得机械映射为"拒绝"**；⑤实时终态与恢复历史复用同一投影路径；⑥旧会话已有 `arguments.cmd` 时应恢复命令展示，缺少真实 diff 时**明确降级**（不伪造、不重新执行工具）；⑦长内容可展开，提示中的按键必须实际可用（仅接通本功能需要的 F4/Ctrl+T 等母本操作）；⑧工具结束及最终回复出现后详情仍保留。另：不从"[工具] name"文本反解析参数；沿用现有工具执行、输出到达时机与调度契约；**新增展示数据不得无必要改变发送给模型的工具结果契约**——展示数据与模型面结果 JSON 分离（见 §2 修正）。
> 本轮（实施轮）修订：调查轮两处设计按用户指令修正——(a) 取消原"cancelled→Declined（未执行）"机械映射，改为独立的"已取消"呈现；(b) 调查轮 §2"apply_patch 结果 JSON 新增逐文件 diff 字段"会把 diff 混入发给模型的工具结果（token 膨胀且无必要），改为历史记录上的**独立可选展示字段**（模型面 content 原样）。
> 实施轮审查整改（独立双轴审查发现，均已修复＋回归测试）：①批内跳过的 apply_patch 曾呈现 "Failed to apply patch"（违 ④）——增 `TurnNotify::ToolEnd.cancelled` 事件旗标（实时）＋`turn::SKIPPED_BY_CANCEL_MESSAGE` 常量精确匹配（恢复路径），现渲染"apply_patch 已取消"；②exec 缺 `arguments.cmd` 曾丢弃输出/状态（违 §3-K4）——降级行保留退出码/错误/输出；③`state_known=false` 补丁失败现在失败 cell 上如实标注"变更状态未知…须重读"；④S28 增 `(+3 -3)` 计数锚点、S16 增 Running 断言、N2b2 增跳过补丁"已取消"断言。
> 本文件 §1–§4 为行为与验收契约（调查轮），§5 实施轮回填。

## 1. 目标 & I/O

- **目标**：TUI 会话中，exec_command 调用显示**实际命令**（运行中＝ spinner＋命令；结束＝命令＋输出预览＋状态/退出码），apply_patch 调用显示**逐文件真实差异**（路径＋新增/删除行着色渲染）；长内容按母本方式折叠，**完整详情可访问**（点击/F4 展开、Ctrl+T detailed 呈现）；成功/失败/取消准确区分；最终回复出现后工具详情仍在；`/resume` 恢复的工具调用渲染为**同一套 cell**且绝不重新执行（007）。
- **输入（数据源，全部现场已有）**：`turn.rs:227-231` 执行前有 `call.id/name/arguments`（exec 的 `cmd` 字符串、apply_patch 的 `patch` 全文）；`execute_tool` 结果 JSON（exec：status/exit_code/output/truncated；patch：ok/changed_files/state_known）；`HistoryItem::AssistantToolCalls.arguments` 已持久化（007 会话文件内）。
- **输出**：`ExecCell`（vendored `exec_cell/`）、`PatchHistoryCell`（vendored `history_cell/patches.rs`）、`PatchFailureCell` 渲染；折叠/展开经 vendored `transcript_view/disclosure.rs`。
- **范围外（明确不做）**：MCP/搜索/图片等无事件源 cell（008 §32 原裁剪不变）；exec 输出的流式逐字节推送（本地 exec_command 同步整段返回——运行态无输出、结束一次性给全量，不人工伪流式）；工具调用的重试/编辑交互。

## 2. 边界 & 风险

- [ ] **差异必须来自实际修改数据**：统一 diff 由 apply 执行时的**真实前后缓冲**生成（匹配器已读旧文、重写产生新文），不得由补丁文本"事后猜测"行号（本地三级容差匹配下补丁文本行号不可靠）；Add/Delete 文件携带真实全文。
- [ ] **展示数据与模型契约分离（实施轮修正）**：发给模型的工具结果 JSON（`HistoryItem::ToolResult.content`）**原样不变**；patch 逐文件真实 diff 与 exec 时长作为历史记录上的**独立可选展示字段**持久（serde 缺省兼容）——旧会话（无该字段）反序列化为 None，按 §3-K4 明确降级渲染，不迁移不重写；007 会话契约（meta v1/追加式/修复语义）零改动。exec 的命令/输出/退出码/取消态均由既有持久面推导（`AssistantToolCalls.arguments` 的 `cmd`＋结果 JSON），无需新字段。
- [ ] **安全（新增展示路径）**：命令/输出/diff 全部为不可信文本，渲染走 vendored cell 管线（ratatui 控制字符过滤＋syntect 高亮入 cell）；A7 攻防扩展：伪造 CSI/OSC/OSC52/DCS/nF 混入命令字符串、输出、diff 文本，经 ExecCell/PatchHistoryCell/detailed 呈现零动作、无伪造行、无自发剪贴板/终端写入（与 010 A6 同批）。
- [ ] **锁与线程**：工具事件由 turn 线程产生（`DriverEvent` 通道既有）；结构化事件沿用同通道，不新增锁；`history` 锁契约（007 §16 锁序）不变。
- [ ] **turn 生命周期**：工具 cell 不入 `stream_indices`（`chat_driver.rs:135-138` 纪律）——Final/Failed 只移除流式 cell，工具详情天然保留；取消批的未执行调用（`turn.rs:217-226` skipped_by_cancel）也发 ToolEnd 并呈现"已取消"（未运行，无输出节），不得显示为失败。
- [ ] **既有回归**：行式路径（非 TUI）工具行为零改动；`/resume` 不重放工具（007 契约）继续由投影保证。

## 3. 关键判断（为什么）

**调查结论（2026-10-15 源码证据）**：

- **本地缺口**：
  1. 活动行仅 `[工具] {name}（{id 前 8 位}）`（`turn.rs:227-231`）→ `DriverEvent::Info(String)` → `PlainHistoryCell`（`chat_driver.rs:197-201`）——命令、输出、差异全不上屏。
  2. `/resume` 投影：`Exec.command` 恒空串（`tui_host.rs:424`——`project_history_items` 只记 `call.name` 不记 `arguments`，:407-411）；`Patch` 只带文件名、`chat_driver.rs:751` `diff: String::new()`——恢复视图无命令无差异。
  3. 驱动键路由无 F4（活动聚焦）/Ctrl+T（detailed 呈现）——折叠提示文案（vendored `activity_preview.rs` "+ N lines (ctrl+t to view transcript)"）当前会撒谎。
- **母本入口（固定提交 file:line）**：`ExecCell::new_active_exec_command(call_id, command, parsed, source, interaction_input, animations)`（`exec_cell/render.rs:59`）＋`append_output`/`complete_call(exit_code, aggregated_output, duration)`（`exec_cell/model.rs:142-161`）＋`mark_failed`（:168-182）；`PatchHistoryCell::new_patch_event(changes: HashMap<PathBuf, FileChange>, cwd)`（`history_cell/patches.rs:67`）；`FileChange::{Add{content}, Delete{content}, Update{unified_diff, move_path}}`（`diff_model.rs:10-21`，diffy 解析 `diff_render.rs:612`）；折叠/展开 `transcript_view/disclosure.rs:17/51/136`（activity id `exec:<call_id>`/`patch:<id>`）；Ctrl+T＝`open_transcript` 键（`keymap.rs:1658`）→ `set_presentation(detailed)` 同视图切换（上游 `app/input.rs:363`）；恢复投影 `thread_transcript/tools.rs:120-185 CommandHistory::from_item → into_cell()`。
- **必要适配（分三个落点）**：
  1. **app 层（`turn.rs`/`tui_host.rs`/`apply_patch.rs`）**：notify 回调由 `&str` 扩为结构化枚举（`Info`/`ToolBegin`/`ToolEnd`）——签名局部改动，五个入史点（007）与工具执行/输出到达时机不变；执行计时 `Instant` 真实值。`apply_patch.rs`：执行时对**真实前后缓冲**生成逐文件展示 diff（Update＝`diffy::create_patch(旧, 新)`，Add/Delete＝真实全文；diffy 已在 workspace 锁内，app crate 增同名依赖不引新版本），作为**展示数据**随历史记录持久（不入模型面结果 JSON）。`project_history_items` 保留 `arguments` 配对：exec 命令取 `arguments.cmd`，patch 差异取展示字段。
  2. **驱动层（`chat_driver.rs`，本地模块）**：`DriverEvent` 增工具变体；工具 cell 记账 `HashMap<id, cell 下标>`：Begin→推入（exec＝`ExecCell` 运行态，`parsed=[]` 安全——`is_exploring_call` 要求非空 `exec_cell/model.rs:235`，不会被误分组 Exploring；command＝单元素 argv 携完整 shell 命令串）；patch＝轻量运行中标记 cell；End→按下标**替换**（exec 终态/patch 完整差异 cell；失败 patch 追加 `PatchFailureCell`）。**终态构造走与 `/resume` 同一投影路径**（构造 `HostHistoryItem` → `project_history` 同一实现）：实时结束态与恢复态同码同渲染；**取消为独立"已取消"呈现（实施轮修正：不映射 Declined/拒绝）**，与失败（红/`Failed`）区分；turn 终结（Final/Failed）清空记账（流式 cell 倒序移除引起的下标漂移不再被使用）。
  3. **键路由（`chat_driver.rs`）**：路由谓词追加 F4（`focus_activity`）与 Ctrl+T（`set_presentation(!is_detailed(), HistoryRenderMode::Rich)`）＋010 的 `owns_interaction_key`（聚焦后方向键/Enter/Esc 归视图）。
- **K1 为什么替换而非原位变更**：已提交 cells 为 `Arc<dyn HistoryCell>` 不可变（母本同型约束，`transcript_view` 以 Arc 指针为键）；我们的工具事件是粗粒度（Begin/End 两态），按下标替换新 Arc 即可，展开状态按 activity id（`exec:<id>`）保持——不引入母本 active-cell 原位变更机制（其存在是为流式 delta，本地无此需求，极简主义）。
- **K2 运行中补丁的显示（自行收敛）**：真实差异只在结束时可得（匹配器产物）——Begin 显示暗色"正在应用补丁…"标记行（诚实、不虚构部分差异），End 替换为完整 `PatchHistoryCell`。
- **K3 折叠与详情访问（复用母本）**：折叠预览＝紧凑行＋输出 3 行预览（`tool_output.rs:19-36`）＋"+ N lines"控制行；点击控制行/F4 聚焦展开（disclosure）；Ctrl+T detailed 全文。不新写折叠逻辑。
- **K4 旧会话明确降级（需求⑥）**：缺真实 diff 的 patch 记录（旧会话/展示字段缺失）渲染为**明示降级**的文件清单 cell（含"早期会话未保存差异详情"类文案），**不得**以空 diff 构造 `PatchHistoryCell`（`+0 −0` 计数＝伪造）；exec 缺 `arguments.cmd` 时命令行留空降级，输出/状态仍按结果 JSON 呈现。降级只影响展示，不触发任何工具重执行。

## 4. 坑记录

1. **`ParsedCommand` 留空防误判**：`is_exploring_call` 对空 parsed 的 `all()` 空真已被 `!call.parsed.is_empty()` 守卫（`exec_cell/model.rs:235`）——勿"补全" parsed 为 Read/Search 形状，否则任意命令被误组为 Exploring 组。
2. **事件顺序与配对**：`complete_call` 按 `call_id` 匹配（mismatch 返回 false＝路由信号）；取消批的 skipped 结果必须也发 ToolEnd（否则运行态 cell 永久 spinner）。
3. **展示数据单源**：实时展示与恢复渲染共用同一投影实现与同一持久字段（展示字段＋arguments＋结果 JSON）——新增展示数据只入持久展示字段，不建两套（否则实时与恢复渲染分叉）；模型面 content 与展示字段互不混装。
4. **`stream_indices` 纪律**：工具 cell 绝不混入流式下标集合（Final 清除时误删工具详情＝回归）。
5. **duration 用真实计时**：`Instant` 差值注入，勿用估算；测试经注入时钟固定（AGENTS.md 测试规范）。
6. **旧会话渲染退化不报错**：缺新字段的 patch 结果按文件名列表渲染（现状），不得因解析失败丢整条历史（`unwrap_or_default` 先例 `tui_host.rs:419`）。
7. **diffy 输入须为文本**：apply_patch 已限常规文件＋文本补丁（004），Update 的非 UTF-8 目标在读入即被拒（不可达 diffy）；Delete 的旧文展示读入遇非 UTF-8/超限＝**明确降级 `None`**（实施轮定稿：不做 lossy 展示——替换符内容比明示缺失更误导；不 panic）。

## 5. 预测 → 实测（验收清单）

实施按 tdd → comprehensive-testing → Security Testing Gate → 独立 code-review → verify:full（2026-10-15 实施轮全部执行；安全门禁无独立 Skill，以 A7 扩展攻防＋新路径渲染管线复核承担并如实记录）。实测结果：

- [x] **A1 命令展示（PTY e2e）**：S14（升级）＋S16 Running 断言＋S27——运行态 Running＋实际命令、终态命令＋输出预览＋成功/失败状态（Failed (exit N)）；展开（F4+Enter）/detailed（Ctrl+T）全文可见（S30/S30b）；变异 A（取消文案改 Declined）红灯证实。触发 exec_command 的 turn——运行中屏幕含 spinner/Running＋实际命令字符串；结束后同一 cell 位置显示命令＋输出预览（≤3 行＋"+ N lines"）＋状态（成功绿/Ran、非零红/Failed (exit N)）；输出全文经展开可见（点击/F4），Ctrl+T detailed 全量。
- [x] **A2 差异展示（PTY e2e）**：S28——增/改/删三文件路径＋(+3 -3) 计数锚点＋增删行着色渲染，磁盘事实交叉验证；失败＝PatchFailureCell（S34）；state_known=false 标注"变更状态未知"（单测）。diff 全部来自执行时真实前后缓冲（apply_patch 单测：Add/Delete 全文、Update diffy 统一 diff 可再解析、部分前缀、超限降级 None、模型面 JSON 不变）。apply_patch 增/改/删文件——cell 显示路径＋`(+a −d)` 计数＋着色 diff（增行/删行背景色）；计数与 diff 行数一致（由真实前后缓冲生成的断言锚点）；失败＝`PatchFailureCell` 错误行；`state_known=false` 场景如实呈现。
- [x] **A3 状态区分**：成功（S14）/失败非零退出（S27）/取消（S29 独立"已取消"，exec 被杀＋批内跳过两种——N2b2 断言跳过补丁不呈现 Failed）三态可辨；单测钉住"不得出现 Declined/拒绝"。成功/失败（非零退出）/取消（独立"已取消"呈现，含被杀有部分输出与批内未执行两种）三态屏幕可辨；取消不显示为"拒绝"或失败。
- [x] **A4 详情持久可浏览**：S14/S28/S34 最终回复出现后命令/差异/输出仍在屏（stream_indices 纪律）；e2e 全程无重放副作用（S34 磁盘断言＋S31 计数器）。最终回复（AgentMarkdown cell）出现后，工具 cell 仍在且可滚动回看、可展开（A3 修复后的滚动链路）；无重放（文件系统副作用仅在执行时发生一次，e2e 断言）。
- [x] **A5 恢复一致（PTY e2e）**：S31 双会话——/resume 后命令/输出同型渲染、mock 计数保持 2（零请求）、副作用文件不变（零执行）；S32 旧格式（无 display）装载不报错。`/resume` 装载含工具调用的会话——命令/输出/差异与实时终态同型渲染（同投影路径断言）；零模型请求（S24 先例）；旧格式会话（无 diff 字段）按现状渲染不报错。
- [x] **A6 持久化往返**：session 单测——display 序列化/往返保真、旧记录（无字段）→None；007 全部既有用例（追加/锁/修复）不回退。**审计注（2026-10-15）**：该单测实施轮因嵌套事故从未运行（A6 当时无执行证据）；审计已激活、修正夹具（配 tool_calls 批次＋user 开篇，符合 007 装载契约）并真实通过。
- [x] **A7 安全攻防**：单测 3 条（命令/diff/取消 cell 载体 CSI/OSC52/nF 注入零动作）＋S33 e2e（命令含转义＋输出伪造 OSC52/清屏——屏幕文本可见、捕获流无 OSC 52 外发、无清屏）。§2 所列注入经命令/输出/diff 三载体零动作、无伪造行、无自发剪贴板写入。
- [x] **A8 既有回归**：行式路径 e2e（tool_loop 14/14 逐字节活动行不变）；全量 `cargo check --workspace && cargo test --workspace --no-fail-fast`＝**2712/0/3**（基线 2688＋24 新增——审计修正分项：e2e 9、A7 3、apply_patch 4、turn 1、tui_host 2、chat_driver 5、session 0〔原计 1 为从未运行的嵌套死测试〕＝24 ✓）。已知抖动：`session_perf::perf_tiers_baseline_driven`（/sessions 50ms 预算）在全量并行下偶发超限（本轮两次 63ms、终跑通过、隔离复跑稳定通过）——该路径本轮零改动，登记为既有负载敏感项。行式路径 e2e 全绿；全量 `cargo check --workspace && cargo test --workspace --no-fail-fast` 通过并报告计数（基线 2688/0/3）。

## 6. 展开态修复轮（2026-10-16，诊断 §8 方案 ①② 实施）

### 6.1 目标 & I/O

- **目标**：exec 工具**展开态**（单 cell 展开（010 披露行点击/F4+Enter）与 Ctrl+T detailed 同一形态）修复三缺陷——①展开态首行缺 Ran/Failed 标题（状态归属不明，长输出下「✓ • 时长」凭空单现）；②命令/状态/时长拼接重复（尾部独立状态行与头部语义重叠）；③命令整条被单引号包裹（`$ 'echo … && …'`——单元素 argv 经 `escape_command` shlex 整体加引号）。
- **输入**：`project_tool_item`/ToolBegin 构造的 exec cell（命令串、输出、退出码、时长）。
- **输出**：展开态每 call＝状态头（marker＋Ran/Failed(exit N)/Running＋`• 时长`）**并入 `$ <cmd>` 首行**（与紧凑头同款单行构造；剥壳后原文）＋全量输出；紧凑形态、diff、Ctrl+C、工具执行契约**零变化**。（实施轮修订：原措辞「状态标题行＋`$ <cmd>`」为独立两行，实施中收敛为并入首行——复制文本经 source 前缀记账仍＝命令＋输出，理由见 §6.3 K-1/记录 §9。）
- **范围外**：「继续内容」可发现性（诊断 §8.2-3，滚动条已在位）；提示政策（§8.3 两则，留待用户裁决）。

### 6.2 边界 & 风险

- [ ] **argv 必须与 003 真实执行一致**：已核对 `src/app/src/exec.rs`＝`Command::new("/bin/sh").arg("-c").arg(cmd)`——真实 argv＝`["/bin/sh","-c",<cmd>]`（cmd 为单个参数）。投影侧按此存储；母本剥壳 `extract_bash_command`（认 `-lc|-c`＋Zsh/Bash/**Sh**）恰好识别该形态→`strip_bash_lc_and_escape` 剥出原文，**零伪造 bash/sh、零猜测拆词、零引号增删**。不改变实际执行（exec.rs 零改动）、模型面结果 JSON（`ToolDisplay` 序列化不变）与会话恢复语义（display-only cell 构造，无新增持久化字段）。
- [x] **vendored 文件偏离逐处登记**：`exec_cell/transcript.rs::detailed_hyperlink_lines`（状态头**并入 `$ cmd` 首行**＋移除尾部状态行——母本此形态无头部、状态在尾部独立行；本地改由头部承载，行为差异登记）；`exec_cell/compact.rs`（提取共享 `status_marker_span`/`status_title` 供两形态复用——纯提取，紧凑形态字节不变，快照佐证）。
- [ ] **紧凑形态字节不变**（目标红线）：共享 helper 提取后 compact 输出与提取前逐字节一致（既有 compact 路径快照/断言保证）。
- [ ] **净化边界**：新展示路径沿用 `highlight_bash_to_lines`/`ansi_escape_line`/Paragraph 渲染管线——原始命令字节不直接写终端（A7 exec 用例＋S33 e2e 经新路径复验）。
- [ ] **快照变更逐项说明**：exec_cell 两份 Insta 快照（transcript/render tests）因头部行变化更新——逐份登记差异原因，不批量接受。
- [ ] 已复现相同展开形态≠证明用户当时主动展开（文档不得反推用户操作——用户所见形态与单 cell 展开态逐元素一致仅说明形态来源）。

### 6.3 关键判断（为什么）

- **K-1 头部复用紧凑构造**：紧凑头（marker 颜色语义：绿=成/红=败/spinner=运行＋标题文案）已是用户验证过的状态语言；展开态复用同一构造（提取共享函数）保证两形态语义一致，时长远移头部（dim `• <时长>`），尾部独立状态行移除——「不因拼接重复展示」。
- **K-2 argv 对齐而非渲染特判**：在渲染层对单元素 argv 特判「不加引号」会给真实多 argv（未来 unified exec）引入歧义；按真实执行 argv 存储让母本剥壳链路自然工作（识别 shell 形态→剥出 script 原文；非 shell argv 仍走 `escape_command` 正常 join）。
- **K-3 尾部行移除而非保留**：保留尾部 `✓ • 时长` 会与头部时长重复（目标明令禁止）；头部 marker＋标题已承载成功/失败/退出码，无信息损失。

### 6.4 坑记录

1. `extract_bash_command` 只认**恰好 3 元素** `[shell, -lc|-c, script]`——多元素（附加参数）不剥壳、退回 escape join（真实执行亦不产生该形态，无矛盾）。
2. `detect_shell_type("/bin/sh")` 经 file_stem 递归＝`ShellType::Sh` ✓（勿凭字面以为绝对路径不识别）。
3. 紧凑头提取共享函数时须保 `is_unified_exec_interaction` 空输出特判仍在 compact 侧（detailed 不适用该折叠）。
4. `duration` 显示条件：`call.duration.is_some()` 才附加（运行态无时长、恢复旧会话无时长不显示 0）。

### 6.5 预测 → 实测（验收清单）

- [x] **C1 批次归属**：S56——一批 3 调用（150 行长输出/含中文与引号短命令/失败退出 4）默认全紧凑（尾部预览＋披露标记＋Failed (exit 4)）；仅点披露行展开长输出项：视口重锚至 cell 顶、头部 `• Ran • 143ms $ for i in $(seq…)`（实测）、其余两项保持紧凑可见（Esc 回底后断言）；长命令在屏恰出现一次（无拼接重复）。
- [x] **C2 展开收起**：S56——展开后头部 L001 直接可达；Esc 回底后 `− Show less` 在位；点击收起→`to expand` 标记＋S56-L150 预览恢复、展开头消失、命令仍恰一次；单测钉住成功/失败/运行三态头（detailed_form_leads…＋detailed_form_header_states_failed_and_running）。
- [x] **C3 命令保真**：单测 exec_command_displays_verbatim_without_shell_wrapping（空格/嵌套引号/中文/`&&`/`||`/`|` 四类命令逐字可见、无 `Ran '<cmd>'` 整条包裹）＋S56 中文引号命令紧凑/展开双断言（`Ran echo "中文 引号 测试"`／`$ for i in $(seq` 且无 `$ '`）。
- [x] **C4 四态准确**：成功（S56＋单测）／失败（S56 紧凑＋单测 `Failed (exit 9) • 7ms $ …`）／运行态（单测 `Running $ …`＋S16 既有）／取消（S29 独立「已取消」路径零改动复跑）。
- [x] **C5 恢复一致**：实时与 /resume 同一 `project_tool_item`（unit A 即走该路径——恢复投影同函数）；S31 复跑绿（零请求零重放）；argv 仅投影期构造，`HostHistoryItem` 仍存原串、序列化未动（无新增持久化字段）。
- [x] **C6 交互不退化**：S56——展开→收起→resize(120×30) 后草稿保留、布局正常、重新展开后新几何沟位列 119 滚动条在位、拖选可用；S30/S35–S39 复跑全绿；RowCache/阅读位置语义零改动。
- [x] **C7 注入零动作**：A7 `a7_exec_cell_command_payload_is_neutralized` 经新 argv 路径 11/11；S33 e2e 复跑（命令＋输出注入经新展示路径）；头部构造仅受控文案（marker/标题/时长），无原始命令字节。
- [x] **C8 全量**：`cargo check --workspace` 0 错误；`cargo test --workspace --no-fail-fast`＝**2751/0/3**（基线 2747＋本轮 4：单测 3＋e2e S56）；e2e **70/70**；快照 5 份逐项说明见记录 §9.3。

### 6.6 快照变更逐项说明（5 份）

1. `exec_cell/transcript_tests__raw_grouped_history…snap`：每 call `• Ran • 5ms` 并入 `$ cmd` 首行（Rich/Raw 一致）、尾部 `✓ • 5ms` 移除——§6 设计变更本体。
2. `exec_cell/render_tests__truncated_live_output_preview_and_transcript…snap`：transcript 段头部 `• Failed (exit 1) • 0ms` 并入、尾部 `✗ (1) • 0ms` 移除；preview/compact 段不变。
3. `transcript_view/text_tests__detailed_command_copy.snap`：首行 `• Running $ printf`（前缀并入后 24 列重排）；输出行与复制文本不变（`layout.text()==command\noutput` 断言原样通过——复制契约保持）。
4. `tool_output_tests__command_preview_matches…snap`：transcript 段 `$ echo output`→`• Ran • 0ms $ echo output`、尾部 `✓ • 0ms` 移除；history/compact 段不变。
5. `transcript_view/search_tests__find_selective_expansion.snap`：展开 cell 首行 `$ 'echo visible`→`• Running $ 'echo visible`（搜索选择性展开/手动披露语义不变；该夹具为单元素 argv，其引号属非 shell argv 的正确 join）。

## 7. 展开后 Enter 无法提交修复轮（2026-10-16 第二轮）

### 7.1 目标 & I/O（用户复现）

- **复现（用户实测报告，原样记录）**：①执行长输出工具命令，默认展示正常；②点击该工具的展开、收起入口；③在输入框输入下一条请求，然后按 Enter；④**实际：旧工具展开/收起，新请求未发送**；⑤预期：新请求正常提交，旧工具不响应该 Enter。
- **输入方式未明**：用户未说明③是逐字键入还是粘贴（含中文输入法路径）——**不得预设粘贴为根因**，两路径须分别经验验证。
- **用户复验状态**（本轮同步）：01/02 通过记录保留；**03 被本问题阻断**；04–09 未执行。
- **目标**：输入归属与工具聚焦的交接对齐母本——明确重新编辑输入（键入/粘贴/点击输入区）后，Enter 提交当前草稿；显式 F4 聚焦且**尚未重新编辑**时，Enter 仍按既有契约控制工具详情（S30b）。

### 7.2 边界 & 风险

- [ ] **禁止简单修补**：「草稿非空就提交」不可用——已有草稿后主动 F4 聚焦再 Enter 应继续操作工具（不误发草稿、不触发裸 exit）。正确语义＝**输入动作交还归属**（键入/粘贴/点击输入区），非草稿状态判断。
- [ ] **复用母本焦点交接，不新建第二套焦点状态机**：母本 Paste 分支（owned_transcript.rs:640-649）＝清选区＋搜索编辑态外 `clear_activity_focus()` 后落 composer；母本 composer 处理鼠标后（:475-483，非滚轮）＝`end_selection`＋`cancel_search`＋`clear_activity_focus()`。本地按同法接线。
- [ ] 既有语义零回归：选区（010）、回底/Esc、Ctrl+C 优先级（K5）、条件绘制（011 §7）、排队/粘贴冲刷、F4 聚焦导航（S30/S30b）、工具执行契约。
- [ ] 诊断不污染 TUI（探针走测试侧计数/断言，非生产打印）。

### 7.3 关键判断（为什么）

- **K-1 展开点击置焦是母本固有行为**：`toggle_activity`（disclosure.rs）末尾 `disclosure.focused = Some(ids)`——点击展开/收起即进入活动聚焦（`has_active_interaction` 吞键）。逐字键入的交还已有母本路径：`handle_disclosure_key` 对普通文本＝清焦返回 None → 键落 composer（"Ordinary text resumes composing"）。
- **K-2 疑点＝Paste 绕过键路径**（待经验证实，非预设）：本地驱动 `CrosstermEvent::Paste` 只 `pane.handle_paste`——若粘贴不清焦，Enter 仍归视图＝Accept＝切换旧工具，与复现④一致；逐字路径预期已通（K-1）。本轮以双路径 RED/GREEN 定论。
- **K-3 点击输入区交还**：母本在 composer 处理鼠标（非滚轮）后清三态（选区/搜索/活动焦点）；本地缺活动焦点清理——点击输入框后（即便未键入新字符）Enter 应提交而非操作工具。

### 7.4 坑记录

1. 点击披露行后首个 Esc 只解除交互（S56 双 Esc 现象）——焦点在身时 Esc 优先清焦/回底次序与无焦时不同，回归测试须按此设定期望。
2. 粘贴冲刷（burst）定时与「粘贴清焦」相互独立：清焦在 Paste 事件即时生效，草稿上屏仍在冲刷延迟后——断言用提交副作用而非草稿即时可见。
3. `paste_search` 本地不可达（010 未接线搜索），调用仅为母本逐字对齐（无活动搜索时恒 false→清焦）。

### 7.5 预测 → 实测（验收清单）

- [x] **D1 鼠标展开/收起 → 粘贴新请求 → Enter**：S57——事前 RED（修复前 mock 计数停在 2，Enter 把旧工具收起、屏幕证据与用户复现④一致）；修复后提交恰好一次（计数 3＋回底后回复可见）、旧工具仍展开（− Show less 可达）。
- [x] **D2 鼠标展开/收起 → 逐字输入（含中文）→ Enter**：S58——**修复前即绿**（经验证实 K-1：vendored「Ordinary text resumes composing」逐字清焦在位；逐字路径非根因——独立审查者以移除粘贴清焦变异复证：s57 红、s58 仍绿）。首版测试曾假红（展开锚定使视口保持阅读态、回复在视口外）——更正为 mock 计数先导＋Esc 回底后断言，登记为坑 2。
- [x] **D3 F4 聚焦 → 点击输入区 → 编辑 → Enter**：S59——airtight 结构（F4→Enter＝展开证明聚焦在位→点击输入区→同一键 Enter 必须提交预存草稿）；变异验证（移除点击清焦→「点击输入区后 Enter 应提交」红）。
- [x] **D4 先有草稿 → F4 → Enter**：S60——仍控制工具（展开）、含「exit」字样草稿零误发、无裸 exit、CLI 存活、草稿保留；钉死「草稿非空就提交」禁令。
- [x] **D5 F4 后重新键入/粘贴 → Enter**：S61 两会话（键入＝vendored 清焦；粘贴＝本轮修复路径），均提交＋回复可见。
- [x] **D6 回归**：全 e2e **76/76**（含 S30/S30b F4 契约、S40–S49 选区/Ctrl+C、S48 保持/取消、s50–s55 条件绘制）；粘贴清选区＝母本行为（owned_transcript.rs:641），全库无测试依赖「粘贴保选区」。
- [x] **D7 用户 03 场景**：S62——`for i in $(seq 1 30); do echo E$i; done; exit 4` 退出码 4 不重试（计数恰 2→提交后 3）、`Failed (exit 4)`＋E30 可见；展开旧工具＋粘贴新请求＋Enter 提交、回复可见。
- [x] **D8 全量**：`cargo check --workspace` 0 错误；`cargo test --workspace --no-fail-fast`＝**2757/0/3**（基线 2751＋本轮 6 e2e）；e2e **76/76**；A7 **11/11**。

**实测更正（诊断过程留档）**：①K-2「粘贴绕过键路径」证实为根因（s57 RED）；②K-1 逐字路径修复前即通（s58 首跑假红系视口保持语义的测试设计错误，非产品缺陷——修正断言后绿，如实登记）；③横向滚轮（ScrollLeft/Right）在 composer 区会清焦（母本本地不可核对、终端罕见发射）——登记观察项不处理。

## 8. 独立详情页轮（2026-10-16 第三轮，用户已选方案 B——替代原地展开/Show less）

### 8.1 目标 & I/O（目标契约 1–9）

- **替代决定**：主对话工具 cell 一律**紧凑预览**；点击披露入口（原 `+ N lines` 行，文案改 details 语义）或 F4 聚焦→Enter 或 Ctrl+T 打开该工具的**应用内独立详情页**（Overlay::Transcript 单 cell 页）；**移除原地展开/收起（Show less）交互**。
- **页内容**：该工具既有完整展示（命令/状态/时长/输出或文件差异，即 §6 展开态渲染）——单 cell 列表，不混入其他工具。
- **页提示**：固定返回对话提示；不再出现 Show less。（2026-10-15 单一职责轮：提示键改为 **q**「q back to conversation」——Esc 专取消，见 §9）
- **一次按键关闭**（含滚动后/有选区时）；关闭事件不外溢主对话。（2026-10-15 单一职责轮：关闭键由 Esc 改为 **q**——复用母本 PagerKeymap.close 的 q；Esc 在页内＝运行中取消（页保持打开、页内 notice 呈现反馈）、空闲无动作，见 §9）
- **关闭恢复**：进入前主对话阅读位置/跟随状态（`TranscriptBookmark` 内容锚点，resize 后不依赖旧屏幕坐标）、草稿与光标不变；输入交还 composer。
- **返回后**：键入/粘贴＋Enter 正常提交，不再控制旧工具（打开页时即清主视图活动聚焦）。
- **键盘入口**：F4 聚焦→Enter＝打开详情页；预存草稿时 F4→Enter **不得误提交**。Ctrl+T＝打开（聚焦工具，否则最近工具 cell）——不再切换全局 detailed 原地呈现；披露文案同步（不遗留 "^t to expand" 失实提示）。
- **Ctrl+C 恒 J10**（永不复制）——驱动在 overlay 路由**之前**处理，**不继承**母本 pager `close: q/ctrl+c` 默认。（2026-10-15 单一职责轮：Ctrl+C 一律退出——运行中不再「取消后留存」，退出走宿主完整收尾；取消归 Esc，见 §9）
- 范围外：新增快捷键/配置项/全历史浏览；主对话内搜索。

### 8.2 边界 & 风险

- [ ] **复用面**：`pager_overlay::Overlay::Transcript`（TranscriptOverlay——事件/剪贴板 poll/notice/draw 骨架）＋其内 `TranscriptView`（滚动/选区/右键复制/折行——与主对话同一视图类型，非第二套滚动模型）＋`TranscriptBookmark`（进入前快照/关闭恢复）＋既有富 cell（§6 展开态渲染即页内容）。不新建通用页面框架/并行事件循环；主界面已在备用屏，不重复进出终端备用缓冲。
- [x] **vendored 偏离逐处登记（012-ADAPT）**：`pager_overlay/transcript.rs`（页模式：单 cell 标题、~~Esc 一次关闭~~（§9 轮移除，改 q 关闭）、右列 011 沟位滚动条渲染＋按下归属拖动、固定提示行（§9 轮键改 q）、绕过 footer_with_navigation 保提示区、开页置顶、§9 轮新增 `set_notice` 访问器（页内取消反馈））；`pager_overlay.rs`（as_tool_page 透传）；`transcript_view/disclosure.rs`（`disclosure_hit` 纯命中提取＋`focused_activity_index` 访问器）；`transcript_view/layout.rs`（披露文案 to expand→for details）；`transcript_view_tests.rs`（vendored 测试 hint 字面量同步）。
- [x] **主事件循环**：详情页打开期间 Key/Paste/Mouse/Draw/Resize 全路由 overlay（母本 app.rs overlay 槽位同构）；Ctrl+C/Ctrl+D 在路由前按 J10 处理；Tick 继续 turn 入账（工具终态/新消息不丢失），但**剪贴板 poll 让位页内**（overlay.handle_event 自带）；ToolEnd 替换同 id 页内 cell（浏览中完成不滞留运行态）。
- [x] **既有回归面**：010 选区/Ctrl+C；011 主视图滚动条/resize/条件绘制/空闲零滴流（s50–s55）；012 命令保真/失败/取消/差异/复制文本/实时恢复一致；A7 注入。**旧契约用例改写**（S30/S30b/S56/s57–s62 的原地展开断言→新契约），逐项说明。
- [x] 打开/滚动/复制/关闭详情**零模型请求、零工具重执行**（计数核算：s63 恒 2、s65 浏览期间恒 2、s66 打开恒 2）。

### 8.3 关键判断（为什么）

- **K-A 单 cell TranscriptOverlay 而非 StaticOverlay**：后者无选区/右键复制/滚轮（需求 F 不可达）；前者内嵌完整 TranscriptView（与主对话同源滚动/选区/复制），富交互零新写。
- **K-B bookmark 恢复而非坐标回放**：`TranscriptBookmark`＝内容锚点（Position/EntryKey/快照），resize 后重定位不依赖旧行号——需求 5 的既有解。
- **K-C Esc 一次关闭是本地决定**：母本 transcript overlay 的 Esc＝页内回底/清选区、q/Ctrl+C 关闭；用户契约要求 Esc 直关——页模式在 `has_active_interaction` 分支前拦截 Esc（顺带清选区），Ctrl+C 不进页（K-J10）。（2026-10-15 单一职责轮**反转**：回到母本原生 close 能力——q 关页；Esc 交驱动统一取消；本地 Esc-直关分支移除）
- **K-D 打开即清主视图聚焦**：旧根因（点击置焦→Enter 吞）从结构上消除——入口打开页而非原地展开，焦点不再残留主视图。

### 8.4 坑记录

1. `PagerKeymap.close` 默认含 **ctrl+c**——驱动必须在 overlay 路由前处理 Ctrl+C，否则 J10 被页吞（需求 9 红线）。
2. 页打开期间驱动 Tick 的剪贴板 poll 会**抢走**页内 finish_copy 的完成分派——须让位（detail.is_none() 才 poll）。
3. 主视图披露点击须先 `disclosure_hit` 判定再开页——直接 handle_mouse 会先走 pointer_down 原地切换。
4. TranscriptOverlay 页内滚轮事件列坐标被重映射到内容区左缘——沟位拖动须在重映射前拦截。
5. 旧契约 e2e（S30/S30b/S56/s57–s62）断言原地展开——新契约下按新行为改写并逐项登记，不得静默放宽。

### 8.5 预测 → 实测（验收清单 A–G）

- [x] **E1（A）**：S63——60 行工具→Ctrl+T 打开→页顶部起（`• Ran • 142ms $ …`＋L01）、翻页两屏滚动只影响页、沟位滚动条在位（█/║）；一次 Esc→紧凑预览（`+ 57 lines`）恢复、跟随态回底（L60 预览）、页标记消失；S56 点击入口同链路＋页内不混入其他工具内容＋命令恰一次。
- [x] **E2（B）**：S64——中文/emoji 草稿＋光标左移两簇→点击开页→resize(120×30)→Esc→草稿逐字保留、插入点保留（X 落草稿中部`中文草X稿🎉`）、输入交还 composer。**残余（如实）**：保持阅读态＋resize 的组合恢复未做 e2e（内容长度边界下 PTY 场景不稳定）——由 bookmark 单测（transcript_view/bookmark）＋S63 跟随态恢复承担机制证明，登记残余验证项。
- [x] **E3（C/D）**：S65——浏览期间延迟回复入账（Esc 后可见）、粘贴新请求 Enter 恰好提交（计数 4）、exit 4 呈现 `Failed (exit 4)` 不重试、旧工具保持紧凑、CLI 存活；键入路径＝S58（页开→Esc→逐字→Enter 提交）；S57/S62 同契约。
- [x] **E4（E）**：S60（预存含 exit 草稿→F4→Enter＝开页、零提交、草稿保留）＋S66（Esc 后追加并提交）。
- [x] **E5（F）**：S66——页内拖选（S66-L10）＋右键复制＝OSC 52 载荷逐字节恰为所选输出（无 Ran/$ 头部污染）＋页内反馈；S63 沟位滚动条＋翻页＝滚动只影响页；一次 Esc（含选区）关闭。
- [x] **E6（G）**：S65——页开期间 mock 计数恒 2（浏览零请求零重放）、回复入账 Esc 后可见；ToolEnd 页内刷新＝`replace_cells`（exec 路径接线；patch 运行 cell 无 activity id＝按构造不可开页，登记）。S34 多 cell 终态回归绿。
- [x] **E7**：S63（Ctrl+T 开页）＋S67（页内 Ctrl+C＝J10 空闲退出，不继承 pager close）；快照×3 文案同步（to expand→for details 逐项核对）；旧原地展开入口全数移除（toggle_disclosure_at 成不可达死路径、Ctrl+T set_presentation 移除）。
- [x] **E8**：`cargo check --workspace` 0 错误；`cargo test --workspace --no-fail-fast`＝**2762/0/3**（现场基线 2757/0/3＋本轮 5：s63–s67）；e2e **81/81**（含 s56–s62 新契约改写 7 例）；A7 **11/11**；变异 RED×2（页内 J10 先行移除→S67 红；页模式 Esc 直关移除→S63 红）均还原。抖动如实：一次全量曾现 1 例未名失败（未捕获用例名，三次复跑 2762/0——模式与已登记 session_perf 负载敏感一致，未证实不断言）。


## 9. 按键单一职责轮（2026-10-15，替代旧 Esc/Ctrl+C 分工——用户已选方案 1）

**新契约（替代 §8.1 需求③/④/⑨中与下列不一致的表述；历史验收事实保留）**：
1. **Ctrl+C＝一律退出 CLI**：主对话/详情页、有无选区/聚焦、任务运行或空闲——统一走 Quit 路径（宿主收尾，实际顺序＝终端恢复→置取消→SIGKILL 工具进程组并回收→会话排空→`再见!` 码 0——`restore_after_exit` 在 `run_tui` 返回后先行，`flush_final_session` 阻塞等待 turn 线程释放 history 锁故进程组回收先于排空完成）。不复制、不「取消后留存」。
2. **Esc＝仅取消**：任务运行中一次按键一次取消（停止本地等待、迟到结果弃置、不宣称中止底层 HTTP 请求——010 §6/§7 语义不变）；**空闲无动作**。不再关页/清焦/清选区/回底/收 ? 提示；这些状态不得提前消费它。驱动统一拦截（主视图 Key 臂与详情页槽位均在任何组件路由之前；**历史搜索态同样不放行**（收尾轮定稿，替代同日早先的「放行」整改——见下）：Esc 在该态运行中＝取消任务、空闲＝无动作，均不退出搜索。**历史搜索的退出＝Enter**（复用其既有出口并扩展：匹配态＝接受历史项（母本行为）；无匹配/空闲/查找中态＝退出并恢复原草稿——该态母本行为为吞键无动作，无既有功能被覆盖、零新增按键；vendored `history_search.rs` ADAPT：Enter-else 分支＋footer 提示「enter accept」/「no match · enter restore」（去「esc cancel」））。
3. **q＝仅详情页内关闭**：复用母本 `PagerKeymap.close`（q；其 ctrl+c 成员被驱动先行拦截恒退出，实际 q 关页）；恢复阅读位置/草稿/光标/composer 焦点（bookmark 机制不变）；主对话 q＝普通文字。
4. **替代入口（均既有可达）**：清选区＝点击输入区；回底＝Ctrl+End（JumpTarget::Latest，`transcript_view/input.rs:64`，Alt 备选）；聚焦解除＝F4；? 提示收起＝再按 ?。
5. **详情页运行中 Esc**：取消任务＋**页保持打开**＋取消反馈经页内 notice 呈现（不得改正在查看的旧工具 cell）；仍用 q 返回。
6. **历史搜索＋工具聚焦复合态（2026-10-17 复合态收尾轮）**：搜索激活时 Enter 一律按搜索规则处理（匹配＝接受历史项／无匹配＝恢复原稿），**不得被工具聚焦的详情页入口抢占**；该 Enter 同时解除工具聚焦——退出搜索后再次 Enter 按正常输入规则（不被旧焦点抢走）；**裸 exit 检查对搜索态不适用**（composer 文本＝预览稿而非待提交输入——恰为 "exit" 时 Enter 仍是搜索语义，不触发退出，复审 P2-1）。复合态经「搜索中按 F4」形成（s80 实证可达）；「聚焦后 Ctrl+R」不形成复合（Ctrl+R 经 vendored「普通文本恢复输入」自清焦）；「聚焦后输入 query」同样自解。非搜索态 F4→Enter 开页行为不变。

实现与验证详见 `docs/records/2026-10-15-single-responsibility-keys.md`；受影响规格：001/006/008/010/011 同步修订。
