# tchat-tui —— openai/codex TUI 渲染层移植（008）

来源：[openai/codex](https://github.com/openai/codex) 固定提交 `97771b1d78cf6366bc3946fe9e4bbca4ebee31d1`（Apache-2.0，含 NOTICE；`custom_terminal.rs` 另含派生自 ratatui 的 MIT 声明，逐字保留）。规格：`../../docs/specs/008-tui-rendering-port.md`。

## 结构

- `src/`：母本 `codex-rs/tui/src/` 的移植闭包（原样拷贝＋登记适配）＋4 个本地新写适配模块（`app_event.rs`/`app_event_sender.rs`/`chatwidget.rs`/`lib.rs`）。
- `assets/`：母本资产原样。
- `../shims/`：本地最小 shim crate（包名/库名沿用母本 crate 名，使 vendored 文件 import 逐字节不变；仅含保留文件消费的类型）＋真拷贝件（tui_keymap/key_aliases/system_commands 等）。
- `../vendor/`：整 crate 原样 vendor（codex-ansi-escape、codex-terminal-detection、codex-mermaid、codex-utils-fuzzy-match）。

## 复用率（A9 终版，脚本按字节比对，2026-10-14）

- 原样 321 / 必要适配 79 / 本地新写 5（app_event、app_event_sender、chatwidget、lib、chat_driver）＝ **80.25%**（不含新写；2026-10-14 最终收敛轮：history_cell/base.rs 因 A7 错误消息入口门禁用例将 PlainHistoryCell 提 pub，由原样转必要适配）
- **010 追加（2026-10-15）**：选择/复制全部复用 vendored 链（selection.rs 状态机、input.rs handle_mouse、copy_action→clipboard_copy OSC 52 回退、composer_gap 反馈、prepare/copy_composer_selection、tick_selection）——零 vendored 改动；本地 `chat_driver.rs` 接线（键守卫「键盘永不复制」、鼠标母本分发序、clipboard poll、提示沟行=驱动侧保留行替代 pane ComposerRenderOptions.composer_gap——单一几何源纪律）、`split_screen_areas` 增 gap 行。
- **011 追加（2026-10-15）**：新写 `transcript_view/scrollbar.rs`（滚动条几何/命中/拖动/行数缓存——母本无滚动条组件，ratatui facade 自带 `Scrollbar`；本地新增非母本移植）；vendored `transcript_view.rs` 仅追加 `pub(crate) mod scrollbar;` 声明（1 行，011 §2 最小化纪律）；本地 `chat_driver.rs` 沟位切分/渲染与鼠标路由（搜 `011` 注释）。
- **012 §9 复合态收尾轮（2026-10-17）**：本地 `chat_driver.rs` Enter 聚焦分支对 `pane.history_search_active()`（`bottom_pane/mod.rs` ADAPT 访问器）让路——搜索态清焦落穿 composer（Enter 按搜索规则），非搜索态开页不变。
- **012 §9 收尾轮（2026-10-17 历史搜索例外消除）**：vendored `chat_composer/history_search.rs` 两处 012-ADAPT——Enter 分支扩展（无匹配/空闲/查找中态＝退出并恢复原草稿，复用既有出口替代被驱动拦截的 Esc；匹配态接受不变）＋footer 提示（Match「enter accept」去 esc 段、NoMatch 增「enter restore」；样式单测与 3 快照同步）；本地 `chat_driver.rs` 删历史搜索态 Esc 放行（严格拦截）。
- **012 §9 追加（2026-10-15 按键单一职责轮）**：vendored `pager_overlay/transcript.rs` 三处 012-ADAPT 更新——页模式 Esc 直关分支移除（Esc 由驱动统一拦截＝取消/无动作）、页模式启用 `keymap.close`（q 关页，母本原生；close 内 ctrl+c 成员被驱动先行拦截不可达）、页提示键 Esc→q（键标取 close 首绑定）＋新增 `set_notice` 访问器（驱动页内取消反馈，不改页内容）；`pager_overlay.rs` `Overlay::set_notice` 透传。本地 `chat_driver.rs`：主视图 Key 臂与详情页槽位统一拦截 Ctrl+C（恒退出）/Esc（仅取消/空闲无动作）先于一切组件路由（结构性防止 vendored 选区/聚焦/Esc-to-latest/composer interrupt 分支接管）；`transcript_owns_key` Esc 子句删除；模块头失实 hooks.on_interrupt 注释修正。
- **012 §8 追加（2026-10-16 独立详情页）**：vendored `pager_overlay/transcript.rs` 页模式（012-ADAPT：标题/Esc 一次关闭/沟位滚动条/固定提示/禁入搜索/置顶）＋`pager_overlay.rs` as_tool_page 透传＋`transcript_view/disclosure.rs` disclosure_hit/focused_activity_index 提取＋`layout.rs`/vendored 测试文案（to expand→for details）；本地 `chat_driver.rs` ToolDetail 槽位与路由（复用 TranscriptOverlay＋TranscriptBookmark，零新框架）。
- **012 §6 追加（2026-10-16 展开态修复）**：vendored `exec_cell/compact.rs` 提取 `status_marker_span`/`status_title` 共享状态头构造（标记 012-ADAPT，紧凑形态字节不变）；vendored `exec_cell/transcript.rs` 展开态状态头并入 `$ cmd` 首行（`source.prefix_bytes` 复制记账）＋尾部状态行移除（标记 012-ADAPT）；本地 `chat_driver.rs` `shell_argv` 按 003 真实执行 argv（`/bin/sh -c`）存储（零 vendored）。
- **011 §7 追加（2026-10-16 帧调度）**：vendored `tui.rs` 追加 `pub(crate) fn subscribe_draw_notifications()`（4 行，标记 `011-ADAPT`——驱动订阅 `draw_tx` 帧通知；母本经 TuiEventStream 消费，本地驱动直连订阅）；复用既有 vendored `tui/frame_requester.rs`（FrameScheduler 合并＋120fps 限频，零改动）；本地 `chat_driver.rs` 循环尾无条件 draw 改脏标记条件绘制（搜 `011 §7` 注释）。
- 明细归档：`../../docs/records/2026-10-14-008-reuse-audit.txt`
- 低于 90% 预测的原因：规格 §1 裁剪清单在无事件源功能密集文件上的计划内深度裁剪（chat_composer.rs 12,633→9,295 行、bottom_pane/mod.rs 4,155→2,918 行、status/card.rs 978→393 行、history_cell/tests.rs 等，均带 `// 008-TRIM:` 登记）＋A7 需要的 pub 提升＋驱动集成改动。属计划内必要适配而非为凑比例的删减；未为提高比例引入任何无用代码。

## 登记标记约定

- `// 008-TRIM: <内容>（008 §1 裁剪：<原因>）`——按规格裁剪清单移除的声明/函数/分支/字段。
- `// 008-REVIVE: <原因>`——初判裁剪后经消费路径证实必须保留而恢复的模块。
- `// 008-ADAPT: <说明>`——本地最小适配（类型映射/克隆/构造差异），不改语义。
- `//! 008-ADAPT` 文件头——本地新写/提取模块（含母本行号出处）。

## 逐文件清单

`原样`（与母本逐字节一致，不含快照）：已随证据归档至 `../../docs/records/2026-10-14-008-reuse-audit.txt`（009 目录整理前位于 `/tmp/008-build/reuse_audit.txt` 与 `logs/`）。`必要适配`与`新写`文件均带上述标记，可 `grep -rn '008-TRIM\|008-REVIVE\|008-ADAPT' src/` 审计。

## 快照

`src/**/snapshots/*.snap` 共 519 个，内容与母本逐字节一致；仅文件名前缀 `codex_tui__`→`tchat_tui__`（insta 按包名寻址，登记为必要寻址适配）。
