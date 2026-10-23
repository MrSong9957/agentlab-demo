# 规格：终端 UI 渲染层移植（Codex TUI 同源）

> 状态：**实现中（A6 Windows ConPTY 未闭合）**　主体已并入 main @ `59c7e18`（2026-10-18 合并，原分支 `feat/008-codex-tui-port` 合并后删除）；维护任务分支 `fix/008-snapshot-hygiene`（快照副产物清理，见 §5 维护验收与 §4 坑 19）。
> **验收规则**：验收未闭合保持"实现中"；A1–A5、A7–A10 已实测通过，A6 无已授权 Windows 环境未实测——全部通过后才标"待批准"。
> 验收状态：A1–A5、A7–A10 全部通过实测（§5 各项＋证据 logs/2026-10-14-008-spec-revision.md §11-16）；**A6 Windows ConPTY 未闭合**（无已授权 Windows 环境——以真实验收阻断返回，不宣称完整验收通过）。
> 已知缺口（登记，2026-10-14 最终收敛轮更新）：~~Ctrl+D 退出~~（已闭合：驱动空输入 Ctrl+D＝Quit，非空交 composer 编辑域——rustyline 语义；S12 e2e）；~~Queued 输入／turn 期间提交~~（已闭合：turn 运行中 Enter（母本 steer 位点）与 queue_keys 均入驱动队列，turn 结束按空闲语义逐条重放（裸 exit/斜杠解析/turn），排队项经 pending_input_preview 显示；S11 e2e）；~~chat_composer_history 上/下键检索回填通道~~（已闭合：会话内回溯由 composer local_history 即时承接（S13 e2e，光标行断言）——001 rustyline 内存历史等价（均无跨会话持久化）；持久查找 `LookupMessageHistoryEntry` 不可达（驱动未调 `set_history_metadata`→persistent_entry_count 恒 0），非交互源按范围裁剪；驱动 draw 补光标定位（A1，S13 前红灯））。~~/resume 会话中投影不可达~~（已闭合：CommandOutcome::ReplaceCells 通道＋S10 e2e 验证）。
> **A3 重开（2026-10-14 用户实测反馈）**：完整 CLI 排版正常但**无法滚动聊天历史**——滚轮与键盘分页的具体失败范围当时未分别核实（不预设结论）。复现条件：TUI 交互模式、历史超过一屏；预期＝A3 原契约（滚轮/分页/跳转/限位/跟随，§3-J2 Owned 模式）。首轮代码证据（2026-10-14 诊断轮）：`chat_driver.rs` 事件分发不完整——Key 一律只交 `pane.handle_key_event`，从不调用 `transcript_view.handle_key`；Mouse 事件落入 `_ => {}` 丢弃（鼠标捕获实际已启用：`tui.rs set_owned_screen→configure_input(capture_mouse=true)`）。母本正确链路＝`app.rs:989 handle_owned_transcript_event` 先于 composer 分发：键最终 `transcript_view.handle_key`（`handle_scroll_key`：PageUp/PageDown(NONE)、JumpTarget、Esc-to-latest），鼠标先判 composer 区域再 `transcript_view.handle_mouse`（滚轮分数行累积→`scroll`）。修复契约＝接通既有实现，不另写滚动算法；不启用选择/复制/搜索/链接（范围边界；2026-10-15 注：选择/复制已由规格 010 立项解除该边界，搜索/链接维持范围外）。
> **A3 重开实测（2026-10-14 修复轮）**：根因确认＝`chat_driver.rs` 键事件只交 composer、鼠标事件落入 `_ => {}`（鼠标捕获本已启用）；修复＝滚动键（PageUp/PageDown 无修饰、JumpTarget、空闲浏览态 Esc-to-latest）先经 `transcript.handle_key`（母本 `handle_owned_transcript_event` 分发序），滚轮经区域判定进 `transcript.handle_mouse`（composer 区→`prepare/handle_composer_mouse`，母本同链路）；另修复驱动丢弃 `Resize` 事件导致布局滞留旧尺寸（现经 `screen_size_for_event`）。验收 S19–S24 e2e（滚轮/分页/限位/视口保持/草稿/恢复会话）＋路由谓词单测×2 全绿；全套 e2e 27/27；全量 **2678/0/3**（基线 2670＋8 新增）。审查轮补：弹窗门（`has_active_view`，母本 no_modal_or_popup_active）、布局推导共享（split_screen_areas）。vendored 文件零改动（custom_terminal.rs sha256 与迁移态一致，独立复核）。
> **2026-10-15 更新（2026-10-16 §6 展开态修复轮追加：展开态状态头并入 `$ cmd` 首行（Ran/Failed＋时长，复制文本仍＝命令＋输出）＋命令按 003 真实 argv（/bin/sh -c）剥壳展示无整条引号，见 012 §6/记录 §9；全量 2751/0/3、e2e 70/70；**2026-10-16 第二轮：展开后 Enter 无法提交修复（粘贴/点击输入区焦点交还，母本同法，见 012 §7/记录 §10；终验 2757/0/3、e2e 76/76；**2026-10-16 第三轮：独立工具详情页（点击/F4+Enter/⌃T 打开、Esc 一次返回、bookmark 锚点恢复、替代原地展开——见 012 §8/记录 §11；终验 2762/0/3、e2e 81/81）**）：规格 012（工具详情）已实施完成并本地验证（待审阅候选，见 012 §5–§8 实测）**——结构化工具事件（TurnNotify）→ToolDisplay 展示字段（模型面结果 JSON 原样）→ExecCell/PatchHistoryCell（实时终态与 /resume 同一投影）＋F4/Ctrl+T 路由接通；全量 2712/0/3。**010（选择/复制）已实施完成并本地验证（2026-10-15 实施轮，待审阅候选，宿主粘贴实测项保留，见 010 §5 实测与 `docs/records/2026-10-15-010-implementation.md`）**——K5 落实：左键拖选＋右键复制、Ctrl+C 恒按 J10（选区不得接管）、键盘永不复制、不启用搜索/链接/自动复制；全量 2741/0/3（补验轮＋S47–S49）；**011（滚动条）已实施完成并本地验证（2026-10-15 实施轮＋收尾轮，2026-10-16 §7 帧调度修复轮，待审阅候选，见 011 §5/§7 实测与 011 记录 §8/§9）**——沟位常驻＋ratatui Scrollbar（零新依赖）＋拖动/轨道点击复用既有 scroll() 语义＋RowCache 行数缓存（稳定帧 70µs@2000 cell）；收尾轮修正端点映射（content_length＝最大可滚动顶行+1，贴底精确触轨底）＋RowCache 失效定点核查无缺口＋stash 事故对象库基线核对；§7 轮修复驱动循环尾无条件 draw（复用母本 FrameScheduler 帧调度：空闲/50 Moved 零帧、滚轮位移守恒、反馈到期与边缘自动滚动自续帧），全量 **2747/0/3**（e2e 69/69）。012 数字经 2026-10-15 审计复核无误（分项修正见 012 规格头部与 `docs/records/2026-10-15-012-implementation.md` §7；fork 竞态结论已收敛为「测试互扰已复现并修复，生产侧对等发生未证实」）。
> **2026-10-15 按键单一职责轮（Esc 审计后实施，`docs/records/2026-10-15-esc-audit.md`）**：Ctrl+C＝一律退出（含运行中——宿主完整收尾，实际顺序＝终端恢复→置取消→SIGKILL 工具进程组并回收→会话排空→`再见!` 码 0）；Esc＝仅取消（运行中一次一键一取消，空闲无动作——不清选区/不清焦/不回底/不关页/不收 ? 提示；回底＝Ctrl+End）；q＝详情页关闭（母本 PagerKeymap.close 复用）。驱动统一拦截两键（主视图 Key 臂与详情页槽位先于一切组件路由），防止 vendored 选区/聚焦/阅读器/composer 分支接管；模块头「hooks.on_interrupt」失实注释修正（实际经 set_cancel_requested）。替代旧「Ctrl+C 运行中取消」分工（010 定点/收尾/重叠轮建立的取消机制本身不变，触发键换 Esc）。详见 012 §9 与实施记录。

> **2026-10-15 立项轮（三项交互改进，规格 010–012，原均"已立项未实现"）**：用户确认三项能力——①备用屏选择文字＋右键复制（**解除 A3 轮"不启用选择/复制"的范围边界**，规格 010：vendored 选择/复制链完整且在编译面内，缺口仅驱动层路由＋复制执行＋poll）；②历史区右侧滚动条（母本无组件（固定提交全树 grep 零命中），按已锁 ratatui 0.30.2 组件最小适配，规格 011，属本地新增 UI 并登记与母本的偏离）；③工具调用详情（**本规格 §1"输入"工具活动契约的实现缺口补回**——现状仅单行 Info（`turn.rs:227-231`），resume 投影 `Exec.command` 空、Patch 无 diff（`tui_host.rs:424`/`chat_driver.rs:751`），规格 012）。实施验证纪律按各规格 §5（tdd → comprehensive-testing → Security Gate → 独立 code-review → verify:full）。
> 启动期授权问句期间 Ctrl+C＝拒绝路径（错误＋退出码 1，与答 n 同路径；S2c 钉住＋A10 变异证据）。**已裁决（2026-10-14 用户确认保留现状，待决标记关闭）**。阶段归属：问句期间＝问句交互语义，不属"启动早期 SIGINT 130"段（该段仅指问句前后的裸启动窗口，见 §3-J10 表）；也不属 001 三段契约任一段。
> 其余登记差异（2026-10-14 审查轮更新）：turn 期间斜杠命令立即派发（母本同路径，不排队）；非空草稿 Ctrl+D 为无操作（textarea 无删字符映射；rustyline 为前向删字符——编辑域细节差异）。已知风险（独立审查 P2，登记不修，证据 §20）：turn 线程异常死亡→turn_running 卡死（Ctrl+D/exit 可逃生）；取消标志 spawn 窗口双重复位（006 既有）；turn 期间退出不取消宿主线程（exec 孤儿/apply_patch 半途——004 state_known、007 跳行修复兜底，退出排空已覆盖待写段）；crossterm 事件流错误按正常退出（码 0，与 001 行式 `Err(_) => break` 同语义）；取消标志 spawn 窗口双重复位（006 既有语义）。其余原登记项（F7 审计、/save 与 /resume 锁等待——含 Resume 的 available_during_task 登记补齐与 cmd_resume 解嵌套、turn 期间退出、线程死亡及毒锁善后）已于 2026-10-14 整改轮＋独立复审修复（证据 §21）。
> 修订记录：v1（调查轮）→ v2（模式/依赖/纪律修正＋D1 决策）→ v3（实现轮：开工 4 修正、测试结果回填 §5、审查修复 §16）→ 2026-10-15 立项轮（登记三项交互改进缺口，引用规格 010–012；本轮未改生产代码）→ 2026-10-18 维护轮（快照副产物清理实施：坑 19＋§5 维护验收实测回填；未改生产代码）。
> 目录整理注记（2026-10-14，规格 009）：仓库布局已按 `docs/specs/009-project-layout.md` 迁移——`src/`→`src/app/src/`、`tests/`→`src/app/tests/`、`assets/`→`src/app/assets/`、`tui/`→`src/tui/`、`shims/`→`src/shims/`、`vendor/`→`src/vendor/`、`logs/`→`docs/records/`；本文旧路径按此映射理解，验收契约与状态（实现中，A6 未闭合）不变。
> 母本：[openai/codex](https://github.com/openai/codex) 固定提交 `97771b1d78cf6366bc3946fe9e4bbca4ebee31d1`；调查证据见 `logs/2026-10-14-008-tui-investigation.md`、`logs/2026-10-14-008-spec-revision.md`。
> 本文件是行为与验收契约（v3 修订轮未改生产代码；2026-10-14 A3 重开修复轮改动＝`src/tui/src/chat_driver.rs` 本地新写模块的事件路由，vendored 文件零改动，见头部"A3 重开"）。

## 1. 目标 & I/O

- **目标**：交互模式下，终端 UI 渲染层与母本同源——同终端尺寸、同输入、同事件序列下，布局、样式（颜色/修饰/主题）、光标位置、滚动与历史渲染行为、终端模式恢复与母本一致；实现方式为**母本源码原样 vendor 优先**，每处适配必须说明必要性，不以"相似外观"重写代替源码复用。非交互路径（stdin/stdout 管道）行为保持现状不变。
- **模式定稿（对齐母本默认全屏，v2 修正）**：采用母本默认——alternate screen 全屏＋**Owned transcript**。源码依据：`config/src/types.rs:886` `fullscreen_transcript` 默认 `true`（"Own the fullscreen transcript, including scrolling, selection, and search"）；`AltScreenMode` 默认 `Auto`（非 over-SSH 即备用屏）；`tui/src/transcript_mode.rs` `TranscriptMode::resolve(owned, alt_screen)` 两者皆真 → `Owned`。
  - **Owned 路径（选定）**：历史为应用自持的 retained cells，由 `TranscriptView`（分页视口＋锚点）渲染，滚动/选择/搜索由应用侧承担（`pager_overlay/`、`transcript_view/selection|search`、`vim_search`）。v1"历史统一写入终端原生 scrollback"表述**错误**，仅适用于下述 inline 路径。
  - **Terminal/inline 路径（非默认，不启用）**：`insert_history.rs` 以转义序列把已完成行写入终端原生 scrollback。该文件因 `tui.rs` 生产代码引用而原样带入，但驱动不接此路径；**不得因任何困难默默降级为 inline**——降级属规格变更，须修订本规格后再做。
  - A1/A3 按选定模式（Owned）验证。
- **落点形态**：仓库根 Cargo.toml 转为 workspace；新增成员 crate `tchat-tui`（目录 `tui/`，承载母本 `codex-rs/tui/src/` 被移植文件树与快照，逐文件保留许可头——含 `custom_terminal.rs` 的 **MIT 声明（派生自 ratatui::Terminal，须整段保留）**与 Apache-2.0/NOTICE——目录级 README 登记来源提交与逐文件"原样/必要适配"清单）；`terminal-chat` 增加事件适配层与最小驱动，依赖 `tchat-tui`。
- **移植范围（映射总表见 §3）**：
  - **原样整组**：`custom_terminal.rs`（+cursor）、`tui.rs`+`tui/`、`insert_history.rs`（随 tui.rs）、`render/`、`markdown_render.rs`、`markdown_stream.rs`、`streaming/`，及核心闭包叶模块（45 模块 / 53,662 行，v1 实测）：wrapping、width、terminal_hyperlinks、style、ui_consts、text_formatting、line_truncation、diff_model、diff_render、exec_cell、exec_command、live_wrap、markdown、markdown_copy、markdown_text_merge、motion、notifications、table_detect、terminal_palette、terminal_probe、clock_format、color、key_hint、keymap、tooltips、tool_output、transcript_reflow、session_state、update_action、version、assistant_directives、clipboard_copy、daybreak、inline_visualization、legacy_core、test_backend、test_support、text_selection、rendered_selection 等。
  - **Owned transcript 子树（v2 增补，合计约 20,680 行含测试）**：`transcript_view.rs`+`transcript_view/`（15,343 行：分页视口、selection/search/footer/bookmark/snapshot/text 系、select_mode、copy_mode、follow_control、layout、mutations、prompt_header、input、activity、composer_gap、disclosure、search_presentation）、`thread_transcript.rs`+`thread_transcript/`（1,690 行：持久化条目→cell 投影）、`pager_overlay.rs`+`pager_overlay/`（2,112 行）、`transcript_reflow.rs`（331）、`vim_search.rs`（44）、`transcript_mode.rs`（24）、`local_settings.rs`（277，设置载体适配字段子集）、`status_indicator_widget.rs`（588）。
  - **部分移植（按事件源裁剪）**：`history_cell/` 首批保留 trait/mod、base、messages、dynamic（exec 输出）、patches（apply_patch diff）、notices、warnings、separators、session、markdown_render_cache、startup_warnings、streaming tail cells；裁剪 mcp*.rs、computer_activity、plans（**017 恢复**：`plans.rs` 计划更新 cell 随 update_plan 工具接入重新启用，仅移植 `PlanUpdateCell`/`new_plan_update` 最小子集，活动 ID 取宿主传入 ID，见 [017-update-plan](017-update-plan.md)）、approvals、request_user_input、search、hook_cell、activity_*.rs、spoken_artifacts。`bottom_pane/` 首批保留 mod、textarea+textarea/{editing,wrapping,vim,vim_search,mouse,hyperlinks}、chat_composer 核心+chat_composer/{slash_input,paste_input,footer_state,completion_target,chat_composer_history}、bottom_pane_view、view_stack、footer（含 `TranscriptFooter`——transcript_view 复用）、composer_gap、paste_burst、pending_input_preview、scroll_state、empty_state_policy、startup；裁剪 skills/memories/config_panel/mentions_v2/approval_overlay/async_questions/file_search_popup/effort_*/voice_strip/hooks_browser/mcp_server_elicitation/pending_thread_approvals/feedback_*/experimental_features_view/command_popup 等。
  - **整体 vendor 小叶 crate**：`codex-rs/ansi-escape`（142 行）、`codex-rs/terminal-detection`（416 行）。
  - **不移植**：`chatwidget/`（33,820 行，经 app_event 直连后端且含大量本地不存在功能；调用序列作驱动层参照）、`app.rs`/`app_*`/`app_server_*`（后端连接层）、`analytics/` 等其余模块。
- **输入**：crossterm 终端事件（按键、bracketed-paste、focus、resize、鼠标滚轮/选择，经母本 `tui/` 事件流）；本地 turn 事件（经 §3-J5 适配层）：user 提交、assistant 回复增量（合成 delta，§3-J6）、工具活动（exec_command 运行中/输出/结束、apply_patch 结果）、轮完成/错误/取消、会话恢复历史投影（§3-J7）。
- **输出**：终端字节流（ratatui/ratatui-crossterm）；测试观测面＝VT100Backend 屏幕状态、insta 快照、PTY e2e。
- **范围外（明确不做）**：inline 模式启用、真 SSE 流式（api.rs 保持非流式整段响应；流式渲染由确定性合成 delta 驱动，不宣称已实现 SSE）、母本专属功能 cell（MCP/审批/计划/**搜索工具 cell**（`history_cell/search.rs`——母本 web 搜索工具的展示，与 Owned transcript 内建的滚动/选择/搜索**无关**）/请求用户输入/hook/实时语音/pets 等）、analytics、非交互路径任何改动。**Owned transcript 的滚动、选择、搜索（`transcript_view/selection|search|select_mode|copy_mode`、`pager_overlay/`、`vim_search`）属移植范围**，不列范围外。

## 2. 边界 & 风险

- [x] **D1 框架替换（已决策，2026-10-14 用户裁决；Ctrl+C 契约于同日实现轮按 /goal 指令细化）**：①采用 Codex TUI 替换 rustyline 输入与渲染框架；②对齐母本默认全屏（Owned transcript，§1）；③01 仅修订被替换的显示层验收——空输入处理、命令语义、正常退出码 0、**Ctrl+C 分阶段契约（启动早期默认 SIGINT 130；就绪后 Interrupted 退出：恢复终端后打印 `再见!`、退出码 0。2026-10-15 单一职责轮重定：TUI 内 Ctrl+C 一律退出——运行中也退出（宿主收尾置取消＋终止进程组），不再「取消后留存」；取消＝Esc、详情页关闭＝q，见 012 §9）**与非交互路径继续保留（映射表见 §3-J10）；空闲态 Ctrl+C＝退出与母本键位（Ctrl+C＝取消）不同，属**驱动层登记适配**，其余键位按母本；④API 保持非流式，同步 turn 逻辑保持既有执行契约（003–007 行为不变，跑 blocking 线程经 channel 桥接事件，不改写为 async）。
- [ ] **D2 依赖底座（v2 已实测修正）**：ratatui 0.30.2 为 facade，`crossterm`/`scrolling-regions`/`underline-color`/`unstable-backend-writer`/`unstable-rendered-line-info`/`unstable-widget-ref`/`layout-cache` 特性**全部存在**并经 facade 转发至 ratatui-crossterm/ratatui-core/ratatui-widgets（sparse index v=2 条目在 features2 字段，单行解析会漏报；以 `cargo metadata`/`cargo tree -e features` 实测为准）。实验证据（`/tmp/ratatui-verify`，独立 target-dir）：解析为 ratatui 0.30.2＋ratatui-core 0.1.2＋ratatui-crossterm 0.1.2＋ratatui-widgets 0.3.2（registry）＋crossterm 0.29.0（**git fork rev `ed1cdab`**，MSRV 1.85）；ratatui 家族 MSRV 1.88 ≤ 本地 rustc 1.99.0；`cargo check` 通过。遗留风险：syntect `regex-onig`（C 库 onig）构建面扩大，默认照搬母本；**构建困难不构成关闭高亮、缩减验收或降级 inline 的授权**——受阻时排查并在报告回报，此类变化须修订本规格并获批准后才可做。
- [ ] **D3 安全回归（003 显示边界等价强度）**：TUI 渲染路径对不可信文本（模型回复、工具输出、远端错误）的过滤依赖母本机制（ratatui 控制字符过滤＋hyperlink 白名单通道）。**不得假设等价**：§5-A7 攻防用例必须证明 CSI/OSC/DCS/nF/CR 注入经 cell/streaming/composer 渲染零动作；A7 属**安全门禁必过项**（见 §3-J12 流程），未过不得交付。`display::sanitize_for_terminal` 在非交互路径继续独立生效。
- [ ] **D4 依赖扩张**：新增 ratatui（含 unstable 特性）、ratatui-crossterm/core/widgets、crossterm（fork patch，rev 固定入 Cargo.lock）、pulldown-cmark 0.10、textwrap、unicode-width 0.2/unicode-segmentation、syntect 5＋two-face 0.5、tokio（事件循环；turn 保持同步）、tokio-stream、insta/vt100（dev）。全部锁版本。
- [ ] **D5 vendor 规模与漂移**：核心闭包 53,662 行＋owned 子树约 20,680 行＋bottom_pane 首批子集（裁剪前约 34,471 行含测试）；被裁功能的快照/测试不得带入；保留文件中指向被裁模块的代码按"最小必要删除"处理并逐处登记，不顺手重构。
- [ ] **D6 `crate::` 路径兼容**：闭包模块必须落在同一 crate（`tchat-tui`）内；母本 lib.rs（4,125 行）只取闭包所需 mod 声明、re-export 与伴随实现，不整文件拷贝。
- [ ] **D7 许可**：`custom_terminal.rs` 的 **MIT 许可头逐字保留**（派生自 ratatui::Terminal：Florian Dehau 2016-2022＋Ratatati Developers 2023-2025 声明）；仓库级 Apache-2.0＋NOTICE 保留；`tui/README.md` 登记来源提交与逐文件"原样/必要适配"清单。
- [ ] **D8 平台验证缺口**：本环境无 Windows；ConPTY 实测标注"待验证"（计划：ConPTY＋屏幕状态断言或 CI Windows runner），Linux PTY 通过不能替代。
- [ ] **D9 既有回归**：非交互路径与 catalog/api/exec/apply_patch/session 测试面不受影响；每阶段跑全量 `cargo check && cargo test --workspace --no-fail-fast` 并报告计数（基线 282/0/0，2026-10-14，main @ `b0e8d48`）。
- [ ] **D10 隔离构建**：全局 `CARGO_TARGET_DIR=/workspace/target`；任何副本/实验必须显式 `--target-dir`（本轮依赖实验已遵守）。
- [ ] **D11 owned 模式专属耦合（v2 新增）**：`thread_transcript.rs`/`other_items.rs` 直接 `use crate::app_server_session::{AppServerSession, HistoryHydrationScope}` 且消费 `codex_app_server_protocol::{ThreadItem, Turn, TurnItemsView, TurnStatus…}`——历史水合入口须改本地投影源（J5/J7），cell 投影逻辑本体复用；`transcript_view/{search,search_footer,footer,disclosure}.rs` 与 `pager_overlay/transcript.rs` 复用 `bottom_pane::{TextArea, TextAreaState, TranscriptFooter}`——**bottom_pane 首批子集必须与 transcript_view 同批落地**（顺序依赖）。

## 3. 关键判断（为什么）

**复用决策规则与统计纪律（v2 定稿）**：本项目已有实现 → 母本现成代码 → 最小适配 → 新写。**"原样"文件＝与固定母本内容逐字节一致**；凡修改 import、删除代码或修改类型的文件一律归"必要适配"并逐处登记原因。≥90% 原样保留是**待验证预测**（§5-A9），不为达成比例引入无用模块。

**映射总表**（行数为固定提交实测；方式见统计纪律）：

| 母本路径（codex-rs/tui/src/） | 本项目落点 | 方式 | 必要性/原因 |
|---|---|---|---|
| custom_terminal.rs（1,324 行，**含 MIT 头**）+ custom_terminal/ | `tchat-tui/src/custom_terminal*` | 原样 | 母本自研 Terminal（派生自 ratatui::Terminal）：insert-history、diff 增量渲染、viewport 定制——重绘行为源头 |
| tui.rs（1,583 行）+ tui/（31 文件 6,116 行） | `tchat-tui/src/tui*` | 原样（通知类型 2 处经 J5 改写） | 终端控制边界：raw mode/恢复、备用屏、事件流、尺寸监控、Windows console/按键序列 |
| insert_history.rs（1,184 行） | `tchat-tui/src/insert_history.rs` | 原样 | tui.rs 生产代码引用；inline 路径本体，默认不启用（§1 模式定稿） |
| transcript_mode.rs（24 行） | `tchat-tui/src/transcript_mode.rs` | 原样 | Owned/Terminal 判定语义本体；驱动固定传 owned=true＋备用屏判定 |
| transcript_view.rs+transcript_view/（15,343 行含测试） | `tchat-tui/src/transcript_view*` | 原样（D11 所列 bottom_pane 复用处随子集落地；快照同批） | **Owned 模式历史视口本体**：分页、锚点、滚动、选择、搜索、布局缓存 |
| thread_transcript.rs+dir（1,690 行） | `tchat-tui/src/thread_transcript*` | 部分原样：历史水合入口（AppServerSession/HistoryHydrationScope）改本地 HistoryItem 投影源（J5/J7） | 持久化条目→与实时输出同源的 rich cell 投影——`/resume` 历史渲染直接复用 |
| pager_overlay.rs+dir（2,112 行） | `tchat-tui/src/pager_overlay*` | 原样 | Owned 模式滚动/分页 overlay |
| transcript_reflow.rs（331）/ vim_search.rs（44）/ local_settings.rs（277）/ status_indicator_widget.rs（588） | `tchat-tui/src/…` | 原样（local_settings 字段取本地所需子集，登记） | resize 重排、transcript 搜索、设置载体、状态指示 |
| render/（9 文件 2,700 行） | `tchat-tui/src/render/` | 原样 | HistoryCell→行渲染模型、syntect 高亮、主题 |
| markdown_render.rs（3,243）/ markdown_stream.rs（903） | 原样 | Markdown→ratatui 富文本样式源头；流式换行门控 |
| streaming/（16 文件 5,970 行） | 原样（plan 流代码保留但无事件触发） | 两区流式：稳定区＋活动区 tail cell（A2 本体） |
| history_cell/（38 文件 12,523 行） | 首批子集（§1 清单） | 部分原样 | HistoryCell 为 trait、cell 独立成文件；首批映射本地事件源；裁剪＝无事件源死代码 |
| bottom_pane/（156 文件 55,608 行） | 首批子集（§1 清单，含 TranscriptFooter） | 部分原样 | 输入编辑器（textarea：光标/换行/kill ring/vim/鼠标）与 composer 状态机；transcript_view 复用其部件（D11） |
| 核心闭包叶模块（45 模块清单见 §1） | `tchat-tui/src/…` | 原样 | 被上述模块 `crate::x` 直接引用，渲染行为一部分 |
| codex-rs/ansi-escape（142 行）/ terminal-detection（416 行） | `tui/` 内整体 vendor（登记改名） | 原样 | ANSI 解析；终端能力检测 |
| chatwidget/（33,820 行） | 不移植；`terminal-chat` 新写最小驱动（ChatDriver：Terminal+Tui+streaming controller+bottom_pane+TranscriptView 组装；参照母本 chatwidget/{rendering,streaming,turn_lifecycle}.rs 调用序列） | 新写（参照实现） | 整体移植＝拖入 app/app-server 层与 realtime/pets/plugins 等本地不存在功能（闭包污染实证：bottom_pane→app_event→chatwidget） |
| app.rs / app_* / app_server_* | 不移植 | — | 后端连接层（任务红线：不引入 Codex 后端） |
| codex_protocol / codex_app_server_protocol / codex_config / codex_utils_* | 不 vendor 整 crate；本地最小类型经 J5 替换 | 适配 | 渲染层入参类型（use 位点：protocol 328、app_server_protocol 960、config 108——含 owned 子树后的闭包计数）；整 crate 拖入远超 UI 所需 |

**关键裁决**：

- **J1 workspace 化＋新 crate**：闭包必须同 crate 才能保 `crate::` 引用原样（D6）；独立 crate 隔离 vendor 面、集中承载许可与来源登记。
- **J2 阶段划分（v2 修订：快照随模块，不集中 P4）**：P0 依赖底座（workspace 化、ratatui 特性＋crossterm fork patch、锁版本）→ P1 渲染核心 vendor（custom_terminal/tui/render/markdown*/streaming/history_cell 子集＋**对应上游测试与黄金快照同批移植**，`tchat-tui` 单测/快照过）→ P2 owned transcript 子树＋bottom_pane 子集 vendor（＋各自测试/快照；D11 顺序：bottom_pane 部件先行或同批）→ P3 事件适配层（J5）＋ChatDriver＋main.rs 切换（交互进 TUI，非交互路径不动）→ P4 本地 PTY e2e 扩展＋安全攻防（A7）→ P5 平台补测（Windows ConPTY，D8）。每阶段全量回归（D9）。
- **J5 事件边界适配（最大适配面）**：新建本地模型模块（`tui_model`），字段语义取母本对应定义最小子集；owned 子树内 `ThreadItem/Turn/TurnStatus/TurnItemsView`、`AppServerSession/HistoryHydrationScope` 等按本地 HistoryItem/会话投影源改写。不为省改写而 vendor 整协议 crate。
  **适配方式定稿（实现轮）**：为使保留文件 import 逐字节不变（最大化"原样"），`use codex_*` 路径由**本地最小 shim crate** 承载（包名/库名沿用 `codex-protocol`/`codex-config` 等，内容仅含保留文件实际消费的类型，逐类型取母本定义最小子集并登记）；shim 不是母本协议 crate 的 vendor，不含后端与无事件源功能。原样文件判定以源文件内容为准，import 不受影响。
- **J6 流式合成**：非流式整段回复按确定性规则（换行边界）合成 delta 喂 streaming 层；完成/中断以事件显式驱动。
- **J7 会话恢复渲染**：`/resume` 装载的 HistoryItem 经 thread_transcript 投影为静态历史 cell（不经 streaming、不重放工具，与 007 语义一致）。
- **J8 快照纪律**：母本 .snap 原样拷贝为黄金主文件，**随所属模块同批移植**（不推迟）；失败按期望/实际差异定位，不得为通过而重接受快照。
- **J9 复用统计**：P1/P2 完成后按"原样/必要适配"逐文件统计并公开清单（`tui/README.md`），对照 A9 预测；不为比例引入无用模块。
- **J10 001 验收映射（D1-③ 落实，001 不整体废止）**：

| 001 验收项 | 处置 | 说明 |
|---|---|---|
| 空输入不回显、循环继续 | **保留** | composer 空提交无效（母本行为），A4 验证 |
| `exit` 命令语义；Ctrl+D 退出 | **保留** | TUI 内 /exit、exit、Ctrl+D（**空输入行**，与 001 行式实测一致——rustyline 仅空行返回 Eof→`再见!`＋0）均恢复终端后退出；非空草稿 Ctrl+D 不退出＝母本 composer 键位（无删字符映射→无操作；rustyline 非空行为前向删字符，编辑域差异，头部登记） |
| 正常退出码 0；`再见!` 提示 | **保留** | TUI 退出、恢复终端模式后打印 `再见!`，退出码 0 |
| 启动/动画期间 Ctrl+C＝默认 SIGINT 130 | **保留** | TUI 初始化早期（未接管按键前）同语义——适用**非交互提问的裸启动窗口**（问句前计算段与问句后至 `tui::init()` 段，cooked mode＋默认 SIGINT）；启动期授权问句期间属问句交互（rustyline raw mode 读键），Ctrl+C＝拒绝路径（S2c，头部待决项），两类窗口相邻不重叠（与 001 中 `你> ` 提示符期 Ctrl+C≠130 同理） |
| 就绪后 Ctrl+C＝Interrupted 退出 0 | **保留** | TUI Ctrl+C＝一律退出（2026-10-15 单一职责轮重定：含运行中——退出走宿主完整收尾，不再「取消后留存」）；与母本键位（Ctrl+C＝取消）不同，属驱动层登记适配；取消＝Esc（驱动统一拦截，母本 interrupt_turn 同键）；详情页关闭＝q（母本 PagerKeymap.close） |
| 工具执行期间 Ctrl+C（006 N2b） | **保留** | 置取消标志＋SIGKILL 终止进程组＋CLI 存活，既有契约不变；通道断开或显示停止≠执行取消，不得把仍有后台副作用的任务报为"已取消" |
| `你> ` 提示符、`Agent：` 回显、开场动画与横幅 | **显示层替代** | composer、assistant markdown cell、母本 startup/empty state 渲染接管；非交互路径下 001 原行为继续有效 |
| 上下键输入历史（rustyline） | **输入层替代** | chat_composer_history 承接同等目标（回溯已提交输入） |
| 多行粘贴不丢输入（buffer-redux 教训） | **目标保留，机制替代** | bracketed-paste＋composer 承接（A4） |
| 非 UTF-8 不 panic、无 U+FFFD 回显 | **保留（目标）** | crossterm 事件层＋渲染路径过滤；TUI 路径待验证（A4） |
| 管道等非终端输出跳过动画/正常回显退出 | **保留** | 非交互路径整体不进 TUI，现状不变 |
| TERM=dumb 退化已知限制 | **保留** | `TERM=dumb` 沿用既有退化路径（不进入 TUI，维持 001 已知限制语义）；TERM **未设置**不得直接判为不支持，按母本能力探测（`terminal-detection`/`terminal_probe`）裁决 |

- **J11 同步桥**：turn 逻辑保持同步（blocking 线程）经 channel 喂驱动；003–007 已验收契约不改写。
- **J12 实现流程纪律（v2 新增）**：正式实现按 **tdd → comprehensive-testing → Security Testing Gate → code-review → verify:full** 顺序执行；A7（不可信终端文本）属安全门禁必过项；Windows ConPTY 未实测前保持"待验证"标注，不得虚报。

## 4. 坑记录

1. **crossterm 必须 patch 到 fork rev ed1cdab**（Windows Escape 输入映射修复＋丢弃次要设备属性回复）；crates.io 0.29.0 行为不等价；tokio-tungstenite/tungstenite fork 与渲染无关不带入。
2. **ratatui 0.30 facade＋sparse index v=2**：特性经 `features2` 字段发布，按旧格式单行解析会误报"缺特性"（v1 调查即因此误判）；验证一律以 `cargo metadata`/`cargo tree -e features` 实测为准（D2 实验记录）。
3. **ratatui 家族 MSRV 1.88**（core/crossterm/widgets 同）、crossterm fork MSRV 1.85；本地 rustc 1.99.0 满足。
4. **bottom_pane 是闭包毒点**（bottom_pane→app 直连、→app_event→app_server_session/chatwidget）；不裁剪＝整体移植。
5. **owned 子树自身耦合**：thread_transcript→app_server_session；transcript_view/pager_overlay 复用 bottom_pane 部件——bottom_pane 子集与 owned 子树同批落地（D11）。
6. **`crate::` 兼容**：闭包同 crate；母本 lib.rs 4,125 行只取闭包所需。
7. **codex_* use 散布在 cell 文件内**：裁文件即裁依赖；保留文件改写逐处登记，禁止批量替换不核对。
8. **两套过滤机制不同层**：TUI 路径＝ratatui 控制字符过滤＋hyperlink 白名单；非交互路径＝display::sanitize_for_terminal。等价性靠 A7 攻防证明。
9. **两种 transcript 路径不可混淆**：Owned（默认，应用自持＋TranscriptView）与 Terminal/inline（insert_history 写原生 scrollback）是不同渲染管线；A1/A3 按 Owned 断言；降级 inline 属规格变更。
10. **同步→异步桥**：不得把 003–007 已验收逻辑改写为 async（J11）。
11. **全局 CARGO_TARGET_DIR**：副本/实验显式 `--target-dir`（D10；本轮依赖实验与调查均遵守，未污染 /workspace/target）。
12. **网络抖动**：github/crates.io 偶发 TLS 中断（本轮 codex fetch 一次、依赖实验 metadata 一次，重试均成功）；克隆/下载脚本需重试；固定 rev 不受影响。
13. **HistoryCell 是 trait 非 enum**：裁剪按文件/类型；保留 cell 引用被裁类型时才需处理。
14. **insta 快照名带 crate 前缀**（`codex_tui__…`）：vendor 改 crate 名会改快照寻址——保持 lib 名 `tchat_tui` 或同步改名快照文件（登记）。
15. **unicode-segmentation 实际解析 1.13.3**（母本声明 1.12.0，semver 兼容上浮）：锁入 Cargo.lock 以实测为准。
16. **PTY 测试宿主必须应答终端转义查询（2026-10-14 A3 修复轮；2026-10-15 收尾修订）**：应用（custom_terminal/terminal_probe）会发 `ESC[6n`（光标位置）与 OSC 10/11（颜色）查询并读等应答；宿主不应答时 resize 后的 `get_cursor_position` 等待会吞掉后续输入，表现为偶发输入死亡。e2e `PtySession::read_once` 内置应答器（模拟真实终端）。**边界（修订，原「真实终端必应答，用户路径不受影响」无现场依据）**：常见 xterm 系终端通常应答这些查询，但不做绝对结论——不应答/应答异常的终端仍会走该等待路径，用户侧是否受影响取决于所用终端的应答行为（未实测终端矩阵）。收尾实测：应答器升级为跨读取缓冲的 `QueryResponder`（拆分查询不漏答、恰答一次、DSR 回报解析器真实光标；单测 8 条含 RED 变异证明）；resize 后立即逐字输入的渲染延迟实测收敛（S25 时延 14–16ms，含提交与后续输入）；深挖另发现**真实生产竞态**：`draw()` 预取尺寸与帧缓冲不一致时 composer 按旧行号写新缓冲越界 panic（全套 e2e 约 1/5 复现）——已修复（区域推导基于真实帧缓冲，S26 压力用例钉住）。2026-10-14 的延迟观察＝该竞态 panic 与宿主未应答查询的双重伪影，证据 docs/records/2026-10-15-008-a3-followup.md §2。
17. **e2e 捕获只在读取时增长**：测试睡眠期间不 `read_once` 会取到陈旧屏幕/原始字节，产生"应用冻结"伪影（曾据此误判死锁/futex）；凡 sleep 后断言屏幕，必须先轮询读取。
18. **驱动丢弃 crossterm `Resize` 事件＝布局冻结**：尺寸必须经 `tui.screen_size_for_event(&TuiEvent::Resize(..))` 喂入（母本同路径）；仅靠 draw 的缓存尺寸不会更新。
19. **insta 副产物不得入库；盘点勿按文件名关键词（2026-10-18 维护轮，fix/008-snapshot-hygiene）**：`.pending-snap`（inline 断言记录，源文件旁点文件）与 `.snap.new`（文件快照 pending）是 insta **只写副产物**，不参与比对；入库后每次测试运行都会向已存在者追加 PASS 标记弄脏工作区（insta `cleanup_passing` 仅当文件已存在时追加），`cargo insta review` 还可按陈旧行号改写源码/金快照。入库根因＝开发期副产物滞留工作区＋合并时批量暂存（`git add -A`）扫入（`59c7e18` 实例，判定证据链见 `docs/records/2026-10-18-008-snapshot-hygiene.md` §2）。防御＝`.gitignore` 两规则（`*.pending-snap`/`*.snap.new`）＋提交前 `git ls-files | grep -E '\.(pending-snap|snap\.new)$'` 须零命中。盘点教训：按 `-i pending` 关键词盘点会漏掉名字不含 pending 的 `.snap.new`——本轮初版清单因此漏 3 个（sparkle×2＋status_surface×1，属主测试均 008-TRIM 裁剪，残留登记见记录 §5，后续删除属新决定）。

## 5. 预测 → 实测（验收清单）

已实测（2026-10-14，两轮）：

- 本地基线（main @ `b0e8d48`）：`cargo check` 通过；`cargo test --workspace --no-fail-fast` **282 通过 / 0 失败 / 0 忽略**。
- 依赖底座（`/tmp/ratatui-verify`，独立 target-dir，未动项目 Cargo.toml/lock）：`cargo metadata` 解析 ratatui 0.30.2（registry，MSRV 1.88）＋ratatui-core 0.1.2＋ratatui-crossterm 0.1.2＋ratatui-widgets 0.3.2＋crossterm 0.29.0（git fork rev `ed1cdab335221515706178d68495bba2aed1924f`，MSRV 1.85）；`cargo tree -e features` 证实全部 7 项特性激活并转发；`cargo check` 通过（rustc 1.99.0）。
- 模式源码事实：`fullscreen_transcript` 默认 true、`AltScreenMode` 默认 Auto、`TranscriptMode::resolve` 两者皆真→Owned；`insert_history` 仅 inline 路径。

待验证（实现阶段逐项给出实测结果；当前均"未运行"）：

- **A1 布局/样式/光标对齐（Owned 模式）**：VT100Backend 驱动 `tchat_tui::Terminal`＋TranscriptView，同尺寸/同输入/同事件序列断言屏幕（布局、光标、样式 span）与母本一致；首批断言集自母本 vt100/transcript_view 用例移植。通过标准：移植用例全绿＋关键场景与母本 .snap 逐字节一致。
- **A2 流式分块**：合成 delta（分块到达、无末尾换行、代码块/表格跨块、完成、中断）喂 streaming controller；稳定区/活动区断言；复用母本 streaming 快照（representative_stream、completed_table 等）。
- **A3 历史渲染/滚动/resize/输入区高度（Owned 模式）**：retained cells 分页与 prepend（恢复历史不重排锚点）、应用侧滚动（pager_overlay）、resize 重排（transcript_reflow/size_monitor 母本单测原样通过）＋PTY 实测 SIGWINCH；composer 高度变化重排。**（2026-10-14 重开：用户实测无法滚动——滚轮/分页链路修复验收，见头部"A3 重开"与 S19–S24 e2e）**
- **A4 宽字符与编辑**：中文/宽字符/组合字符/emoji/长行（textarea wrapping、markdown 宽度）母本用例＋本地中文样例；多行粘贴经 bracketed-paste；空输入无效提交；TUI 路径非 UTF-8 不 panic 无 U+FFFD；Ctrl+C 三阶段（启动早期 130／空闲态退出 `再见!`＋0／工具执行期取消且 CLI 存活）按 §3-J10 契约验证。
- **A5 终端模式恢复**：正常退出（恢复后打印 `再见!`、退出码 0）、panic hook、异常路径后终端模式与屏幕干净（母本 alternate_screen/恢复逻辑原样）。
- **A6 平台**：Linux PTY 全量；**Windows ConPTY 实测待验证**（D8 计划）。
- **A7 安全攻防（003 等价，安全门禁必过）**：CSI/OSC/DCS/nF/CR 伪造序列经模型回复、工具输出、错误消息注入 cell/streaming/composer/transcript 渲染，屏幕断言零动作、无伪造行；未过不得交付。
- **A8 既有回归**：全量 `cargo check && cargo test --workspace --no-fail-fast` 各阶段通过并报告计数；非交互路径现有 e2e 不改动仍绿。
- **A9 复用率实测**：P1/P2 后按 J9 统计"原样（逐字节一致）/必要适配"逐文件清单，对照预测（核心闭包＋owned 子树 ≥90% 文件原样落地可编译，改写集中于 J5 类型位点与 D11 水合入口）；如实报告偏差，不为比例引入无用模块。
- **A10 关键用例有效性**：按仓库规范对 A1/A2/A7 抽样"临时破坏被测行为须失败"对照，证明用例非空跑。

**A7 原始输出证据补充（2026-10-22，执行准备；同日测试加固轮已验证）**：原 A7 历史记录保留，本项不改变过滤或合法终端控制契约。

- 观测边界：加固 [security_untrusted_text.rs](../../src/tui/tests/security_untrusted_text.rs) 的 `a7_full_pipeline_writes_no_injected_control_sequences_to_terminal`、`a7_byte_level_writer_passthrough_check`，捕获真实渲染 writer 在进入 VT100 解析器前的原始输出，并分别检查正常文字与解析后的屏幕结果；检查 `draw`／写入成功，不能把屏幕不含 ESC 当作原始字节安全证据。
- 复用现有 Terminal、VT100Backend 及真实 PTY S33/S46；仅允许必要的测试侧捕获 writer 辅助，不改生产过滤算法或引入新框架。以固定唯一攻击标记区分外来 CSI/OSC/DCS/nF/CR 载荷与渲染器合法的光标、样式、超链接控制；不得对全部 ESC 一律拒绝。S46 的显式用户复制可产生合法 OSC 52，渲染／拖选本身不得自发产生该载荷。
- 反证预测：在隔离副本中临时让一份外来、带唯一标记的完整 OSC 52 载荷直通同一 writer；保持断言不变，即使 VT100 消费该序列且正常文字仍可见，原始字节检查也必须失败。恢复后通过，并以无攻击输入证明合法渲染控制未被误报；不能只在屏幕文本后附加假载荷来代替渲染边界验证。
- 状态：**已验证（2026-10-22 测试加固轮）**——CapturingParser 先记录后转发；两层标记检查（引入符直接组合＋CSI/OSC/DCS/SOS/PM/APC 完整序列范围扫描，后者为实施中发现的补强：真实形态 OSC 52 的标记与引入符隔着参数，仅直接组合会漏检）；draw 路径正向对照断言合法 CSI 存在（不误报）。反证 H1 检出（writer 边界注入 `\x1b]52;c;…\x07` → 原始字节 RED，屏幕断言不触发），还原 GREEN；S33/S46 真实 PTY 复验通过。证据见 [加固记录](../records/2026-10-22-test-hardening-round.md)与[安全报告](../security/reports/2026-10-22-test-hardening-injection-fingerprint-security.md)。

**维护验收：快照副产物清理（fix/008-snapshot-hygiene，2026-10-18 立项＋实施，实测回填；根因与判定证据见 §4 坑 19 与 `docs/records/2026-10-18-008-snapshot-hygiene.md` §2/§3）**：

- 范围：`git rm`（精确路径，非 `git add -A`）5 个副产物——4 个 `.pending-snap`（markdown_render_tests/file_citations/followups/copy_mode）＋1 个 `.snap.new`（chat_composer__sparkle__…a_fresh_astra…）；`.gitignore` 增 `*.pending-snap` 与 `*.snap.new`；**保留全部已接受 `.snap`（含孤儿 `…a_fresh_astra….snap`，登记不删）**。
- 门禁适用性（本轮裁决）：无产品行为修改→**tdd 不适用**（不新增单测，以跟踪状态/忽略匹配/既有测试结果验证）；comprehensive-testing 覆盖见下实测项；仅删非测试文件＋忽略规则＋文档、无新增安全敏感面→**Security Testing Gate 跳过**（该裁决不补齐 010 剪贴板安全证据缺口）；Windows A6 暂缓不变。
- 实测（2026-10-18 实施轮，全部通过；命令与原始输出见记录 §3）：
  - [x] 5 路径 `git ls-files` 零命中，工作区文件已删（staged D×5）。
  - [x] `git check-ignore --no-index`：5 副产物路径全部命中；抽验 6 个正常 `.snap`（含孤儿 `…a_fresh_astra….snap`）均不命中。
  - [x] 已接受 `.snap` 与基线一致：`59c7e18` 基线 724 个＝当前 724 个，`git diff 59c7e18 -- '*.snap'` 为空。
  - [x] 严格模式重跑：3 个现存差异测试精确命中 **3/0**；受影响子集 **233/0** 与清理前基线一致；已裁剪测试（`navigate_copy_resize_and_exit`、`a_fresh_astra…`）零命中——单独注明，不计为通过项（命令、逐条 `test` 行与明细见记录 §3）。
  - [x] 两次测试运行后文件系统扫描（`find src/tui src/app -name '*.pending-snap' -o -name '*.snap.new'`）**零新增**（仅余 3 个登记残留，见坑 19）；不以 git status 代替（忽略规则会隐藏此类文件）。
  - [x] `git diff --check`（暂存＋未暂存）通过；最终差异仅授权范围（5 删除＋.gitignore＋规格/证据记录）。
  - [x] code-review（Skill 双轴）完成，发现与处置见记录 §4。
  - [x] verify:full＝L3 合理范围：`cargo check` 通过＋上述受影响测试＋一致性/忽略检查；**未重跑整个 workspace**（无代码路径变化，2026-10-18 合并门禁全量退出码 0 仍为有效全量基线，不重复运行未变化候选的已通过测试）。
- 残留登记（本轮范围外，不扩权处理）：3 个同类 `.snap.new` 仍被跟踪（`sparkle__shortcut_help_hides…`/`sparkle__shortcut_help_preserves…`/`status_surface__activity_shortcut_help…`，属主测试均 008-TRIM 裁剪，路径见记录 §5）；新忽略规则已覆盖其模式，删除属后续新决定。

**018 修订注记（2026-10-22）**：018 同步问询补充：恢复固定 Codex 基线的 `bottom_pane/request_user_input/{mod,layout,render}.rs` 必要子集及 `history_cell/request_user_input.rs`；用本地独立通道替代 App Server，保留选项/Other/notes/切题/未答确认，去掉异步队列、倒计时、自动空答和图片。主 composer 草稿与普通排队输入独立保留；实时/恢复同源。详见 [018](018-user-input.md) 和[实施证据](../records/2026-10-22-018-user-input-implementation.md)。
