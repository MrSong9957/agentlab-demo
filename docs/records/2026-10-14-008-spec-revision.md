# 008 规格 v2 修订与依赖底座验证（2026-10-14 第二轮）

上游母本不变：openai/codex 固定提交 `97771b1d78cf6366bc3946fe9e4bbca4ebee31d1`（`/tmp/codex-upstream`）。本轮未修改生产代码、未提交；项目 Cargo.toml/Cargo.lock 未动（实验在 `/tmp/ratatui-verify`，独立 target-dir）。

## 1. Git 与基线复核

- `main @ b0e8d48c1f879ae6ea37f0fdd11fe5440f9f9633`；既有差异仍为 `M config/auto_catalog.json`（refresh 时间戳）＋`?? db/`（wiki.db），均运行时产物，未动。
- 工具链：rustc/cargo 1.99.0（≥ratatui 家族 MSRV 1.88、crossterm fork MSRV 1.85）。
- 基线沿用 2026-10-14 上午实测：cargo check 通过；全量测试 282/0/0。

## 2. 全屏模式源码事实（规格 §1 修正依据）

- `config/src/types.rs:886` `fullscreen_transcript: bool`，serde `default = "default_true"`，注释"Own the fullscreen transcript, including scrolling, selection, and search. Defaults to `true`"。
- `AltScreenMode`（protocol/src/config_types.rs:659，经 config re-export）：`#[default] Auto` / Always / Never；`tui/src/lib.rs:2199 determine_alt_screen_mode`：Auto→`!terminal_app_over_ssh`。
- `tui/src/transcript_mode.rs`：`TranscriptMode::resolve(owned_enabled, alternate_screen_enabled)`——两者皆真→`Owned`，否则 `Terminal`。**母本默认＝Owned 全屏**。
- v1"历史统一写入终端原生 scrollback"表述错误：`insert_history.rs`（"Inserts finalized history rows into terminal scrollback"）是 Terminal/inline 路径；Owned 路径历史为应用 retained cells，由 `transcript_view.rs`（"The app owns history and pagination. This view owns only reading/selection state"）渲染，滚动/选择/搜索在应用侧（pager_overlay、transcript_view/selection|search、vim_search）。
- `streaming` 控制器在两模式下的差异实例：`chatwidget/streaming.rs:230,599` `ListSpacing::Compact`（owned）vs `AfterMultiline`（terminal）。

## 3. Owned 子树规模与耦合点（实测）

- 规模（含测试）：transcript_view 15,343；thread_transcript 1,690；pager_overlay 2,112；transcript_reflow 331；vim_search 44；transcript_mode 24；local_settings 277；status_indicator_widget 588。合计约 20,680 行。
- 耦合点（文件级，归 J5/D11 适配）：
  - `thread_transcript.rs`、`thread_transcript/other_items.rs` → `use crate::app_server_session::{AppServerSession, HistoryHydrationScope}`；消费 `codex_app_server_protocol::{ThreadItem, Turn, TurnItemsView, TurnStatus, CommandAction, FileUpdateChange, McpToolCallResult…}`——历史水合入口改本地 HistoryItem 投影源。
  - `transcript_view/{search.rs, search_footer.rs, footer.rs, disclosure.rs}`、`pager_overlay/transcript.rs` → `use crate::bottom_pane::{TextArea, TextAreaState, TranscriptFooter}`——bottom_pane 首批子集与 owned 子树同批落地。
- 含 owned 种子后闭包对 codex_* 的 use 计数（含 chatwidget/app 污染，仅作规模感）：app_server_protocol 960、protocol 328、config 108——不 vendor 整 crate 的判断不变。

## 4. 依赖底座验证（D2 实测）

实验工程 `/tmp/ratatui-verify`（Cargo.toml：ratatui 0.30.2 default-features=false，features＝母本 workspace（crossterm/layout-cache/underline-color）∪ tui manifest（scrolling-regions/unstable-backend-writer/unstable-rendered-line-info/unstable-widget-ref）；`[patch.crates-io] crossterm = git openai-oss-forks/crossterm rev ed1cdab…`；src/main.rs 仅引用 `ratatui::layout::Rect`/`text::Span`）。构建/检查均显式 `--target-dir /tmp/ratatui-verify/target`（D10）。

命令与结果：

| 命令 | 结果 |
|---|---|
| `cargo metadata --format-version 1` | 首跑网络抖动失败（stderr 被吞，重跑带 stderr 后 exit 0） |
| 解析版本 | ratatui 0.30.2（registry，rust_version 1.88.0）、ratatui-core 0.1.2、ratatui-crossterm 0.1.2、ratatui-widgets 0.3.2（registry）、crossterm 0.29.0（**git fork rev ed1cdab…**，rust_version 1.85.0）、unicode-segmentation 1.13.3、unicode-width 0.2.2 |
| `cargo tree -e features` | 7 项特性全部激活并经 facade 转发（如 scrolling-regions→ratatui-core＋ratatui-crossterm；unstable-backend-writer→ratatui-crossterm；underline-color→core/crossterm） |
| `cargo check --target-dir …` | **通过**（Finished dev profile；唯一一次失败是占位 main.rs 引用不存在的 API，改真实 API 后绿） |

结论：v1"crates.io 0.30.2 特性表无 crossterm/scrolling-regions/unstable-backend-writer"为 sparse index 旧格式解析误报（v=2 条目特性在 `features2` 字段）；母本特性集合在 crates.io 完整可用，fork patch 可解析可编译，MSRV 满足。

## 5. 许可与测试纪律补充

- `custom_terminal.rs` 头部为完整 **MIT 许可声明**（派生自 ratatui::Terminal；Copyright 2016-2022 Florian Dehau＋2023-2025 The Ratatui Developers）——逐字保留（D7）。
- 快照与上游测试随所属模块同批移植（J8）；实现流程 tdd→comprehensive-testing→Security Testing Gate→code-review→verify:full（J12）；A7 属安全门禁；Windows ConPTY 保持待验证。

## 6. 分支

- 修订后无重大待决（D1 已由用户裁决并入规格 §2/§3-J10），按授权从核实后的 main 创建实现分支 `feat/008-codex-tui-port`（创建动作见本轮报告；分支不含任何提交，工作区差异保持原样）。

## 7. 实现轮进度（2026-10-14 /goal，同日追加）

本节为阶段性记录（实现未完成，规格状态仍为"实现中"，不标"待批准"）：

- 规格开工 4 修正已写入（Ctrl+C 三阶段契约、Owned 范围澄清、TERM=dumb、构建困难非降级授权）。
- P0 完成：workspace（根 Cargo.toml）＋crossterm fork patch＋tchat-tui crate（edition 2024）。
- vendor：tui/ 计划文件集（约 12.8 万行＋495 快照）＋assets；vendor 整库 codex-ansi-escape/codex-terminal-detection/codex-mermaid。
- shims：20+ 个本地最小 crate（保 vendored import 逐字节不变）；其中 tui_keymap/key_aliases、shell-command 五文件为真拷贝。
- 本地适配模块：app_event/app_event_sender（子集枚举＋HistoryLookupResponse）、chatwidget（ActiveCellTranscriptKey 提取）。
- 裁剪手术：bottom_pane/history_cell/transcript_view 系按 §1 清单登记式 TRIM；ActivePopup 退化为 None（首批无弹窗）。
- 编译循环：tchat-tui 从 499 错收敛至 **260 错**（进行中）；terminal-chat 依赖已按 D9 回退，**既有基线 282/0 保持通过**。
- 跨会话续作文档：/tmp/008-tui-handoff.md（手术模式、剩余波次、坑记录）。

## 8. 续作轮（/goal 第二轮，2026-10-14 追加）

- 交接文档归档至 `logs/2026-10-14-008-handoff.md`。
- **适配方法纠正**（按 /goal）：固定返回值桩全部替换——elapsed/string 整 crate 原样拷贝（含上游单测，4/4 与 24/24 通过）；num_format 登记为 en-US 确定性本地映射（母本 icu_locale）＋行为测试；ThreadItem/Turn/Thread 本地真实映射（四变体：UserMessage/AgentMessage/CommandExecution/FileChange，字段取母本 item.rs/thread_data.rs；缺席功能变体不定义）；otel RuntimeMetricsSummary 真实字段与方法；AskForApproval/PermissionProfile/NetworkSandboxPolicy/ReasoningEffort/NotificationMethod/ParsedCommand/TextElement(protocol)/ByteRange 真实定义；system_commands.rs 原样整文件（受信目录防 PATH 劫持语义保留）。
- **根因修复**：孤儿属性 bug（`#[cfg(test)]` 跨越 TRIM 注释误套 `mod bottom_pane_view`，致其仅测试期编译）——全局修复含变体形式。
- **裁剪登记**：thread_transcript 后端水合（load_session_transcript→驱动直调投影）、MCP/Collab/computer-use/Plan/Reasoning/WebSearch/ImageGeneration/Review/HookPrompt/DynamicToolCall 投影臂、tooltips 远端公告与实验功能 tips、terminal_size_telemetry（遥测上报）、daybreak（访问计划横幅）、pets、DynamicToolCallCell（规格 v2 曾误标为 exec 预览——实在 exec_cell/，已更正登记）。
- **REVIVE 登记**（消费路径证实的管线内部件）：mention_codec/task_mentions（codec 纯函数保留、spawn_search 裁）、slash_commands/prompt_args（斜杠解析）、exploration_groups（连续命令执行分组，本地存在）、activity_group/activity_preview（exec 分组显示）、git_action_directives、copy_input_guard、model_catalog/update_versions（status 代理）。
- 并行子代理：status/ 裁剪与 composer/mod.rs 字段方法裁剪（进行中）。
- **A9 复用率实测（进行时点）**：原样 340 / 必要适配 26 / 本地新写 4 ＝ **92.9%**（≥90% 预测达成；脚本 /tmp/008-build/reuse_audit.py，终版在收尾重跑）。
- terminal-chat 基线保持 **282/0**。

## 9. 编译里程碑（/goal 第二轮收尾段）

- **tchat-tui lib 编译 0 错误**（`cargo check -p tchat-tui` 通过；此前峰值 644 错）。达成路径：两轮子代理并行裁剪（composer/mod.rs 与 status/）＋本人车道（thread_transcript/history_cell/tooltips/screen_reader/exec_cell/杂项）收敛。
- 28 个缺失的 `#[path]` 伴随测试文件补拷；tests 构建错误降至 410（全部为测试代码引用被裁功能），已分派两测试清理子代理（进行中）。
- **terminal-chat 基线复核：282/0（--no-fail-fast）**；一次 `new_auto_writer_lock_excludes_second_writer` 失败经单跑与全量复跑排除（疑为并发构建高负载下 flock 时序抖动，记入风险观察，007 测试不改）。
- A9 复用率：**88.8%**（355/45/4）——低于 90% 预测，归因于无事件源功能密集文件的计划内深度裁剪（登记于 tui/README.md）。
- tui/README.md 已建（结构/标记约定/复用率/快照说明）。

## 10. 007 锁测试偶发失败——有界诊断记录（2026-10-14 /goal 第三轮）

- 首次失败：`cargo test -p terminal-chat --lib --tests --target-dir /tmp/008-build/target`（无 --no-fail-fast，当日高负载），`tests/session.rs:1208` 断言 `!info_c.degraded`（"A 释放后新会话必须能续写 X"）。
- 复现（均为工作区、重度并发构建负载期间——两个子代理同时跑 cargo）：32 线程全套件循环 2/34 失败，同一测试同一断言。
- 不复现：单测试 60 轮；隔离副本（/tmp/007diag，含 [DEBUG-007] 失败现场转储插桩：/proc/locks、/proc/self/fd、inode）55+ 轮（含 threads=32＋中等构建负载）零失败，转储未触发；负载回落后工作区 12/12 通过。
- 静态核查：`Session` 无 Drop impl/无后台线程/无 Arc 共享 fd——`drop(a)` 同步关闭数据文件 fd，内核同步释放 flock；测试目录 `tchat-test-{pid}-{tag}-{nanos}` 唯一，无跨测试路径冲突可能；`try_flock` 仅 EWOULDBLOCK/EAGAIN→Busy。未找到任何在进程内可持有 X 锁的第三方。
- **结论：原因待确认**（不写"已排除"）。强相关证据：三次失败全部发生在多进程并发构建（共享 /tmp/008-build/target）期间；负载消失后不可复现。008 未改动 src/session.rs 与 tests/session.rs（git 状态核对），失败路径全在 007 已验收代码内——**记录为基线已有且与 008 无关的偶发风险**，不扩大任务处理；后续若在高负载下复现，可沿用 /tmp/007diag 的 [DEBUG-007] 转储定位持锁者。未延长 sleep、未改测试、未挑选重跑掩盖。

## 11. vendored 测试全绿（2026-10-14 /goal 第三轮）

- tchat-tui 测试：**2104 通过 / 0 失败 / 3 忽略**（`cargo test -p tchat-tui`）。
- 关键修复：顶层 `src/snapshots/`（198 个）漏拷导致 60+ 快照失败——补拷＋前缀改名；PathUri shim 语义补齐（file:// 含 scheme 切片、UNC、percent 解码、盘符去前导斜杠）修复 markdown_render 8 败；版本耦合快照 4 个经逐行 diff 验证"仅 0.0.0→0.1.0＋insta 元数据行"后按登记接受（J8 例外：差异全部来自登记适配量而非行为漂移）；status token 断言恢复母本原文（子代理曾误设 1.2K，正确值 1.9K＝非缓存输入＋输出）＋正向存在断言；footer_snapshots 混合测试裁 ShortcutOverlay 3 块保留 ComposerEmpty；spoken_artifacts 2 测试随裁剪功能整删登记。
- 测试保留/裁剪计数（两子代理报告汇总）：history_cell 124 上游测试＝87 留/37 裁；tools 5=2/3；other_items 3=2/1；source_cell 7=5/2；chat_composer 域 290/290 通过（弹窗/overlay/agents 23 个运行期必败测试整删登记）；pager_overlay 17/18 留；copy_mode 17/18（1 个快照与裁剪 plan cell 绑定）；status 保留 3＋恢复母本断言。
- 007 诊断结论见 §10（原因待确认，基线无关风险记录）。

## 12. A7 安全门禁全绿＋关键发现（2026-10-14 /goal 第三轮）

- **A7 测试 6/6 通过**（`tui/tests/security_untrusted_text.rs`）：CSI 清屏/光标移、OSC 剪贴板写入、nF 字符集、DCS、C1 单字节 CSI、CR 行首覆盖——经**真实渲染管线**（Paragraph→Buffer→vt100）注入与断言。
- **机制确认**：过滤边界在 ratatui-core `span.rs:314`（grapheme 迭代处 `filter(|g| !g.contains(char::is_control))`）——产品路径（Owned transcript 的 Paragraph 渲染）天然安全；display_lines 返回的行文本保留原始 ESC，但渲染时被剥离（残余为可见文本如 "[2J"）。
- **A10 有效性证据（同轮实证）**：早期测试版本绕过 Paragraph 直接 `set_symbol` 写 cell 时，A7 全部失败（屏幕空白/OSC 直通）——即"绕过过滤边界→目标测试失败"，证明 A7 用例非空跑。恢复经 Paragraph 的真实路径后全绿。
- **vt100 怪癖记录（008-ADAPT）**：vt100 crate 逐格流写（满列+CRLF 双换行滚屏；宽字符列计数）不适用于 CJK 屏幕断言——CJK 用例取 Buffer 级文本（Buffer 即 backend 逐 cell 写出内容），ASCII 用例经 vt100 证明字节级→屏幕级等价。
- 伴随适配（登记）：`Terminal::with_options` 的视口来自真实终端尺寸（测试无 tty 时为 0）——测试须显式 `set_viewport_area`/直接 Buffer 路径；`test_backend`/`streaming::controller`/`AgentMarkdownCell`/`HistoryRenderMode`/`InlineVisualizationContext`/`HyperlinkLine::line_text()` 按 A7 需要提 pub（逐处标注）；vt100 由 dev-dep 提为正式依赖。

## 13. A10 变异记录（部分完成，n2b 待 PTY 代理）

- **A7 变异（已完成，同轮实证见 §12）**：绕过 Paragraph 过滤边界（直接 set_symbol）→ 6 个 A7 测试全部失败；恢复真实管线 → 全绿。
- **n2b 取消变异（已执行、暂无法判定）**：将 `tui_host.rs` `on_host_sigint` 置位反转（A10-MUTATION）后运行 n2b——测试在首个 `你> ` 等待即超时失败（PTY 测试尚未适配 TUI 界面，属子代理在途工作），无法归因于变异；已恢复变异（grep A10-MUTATION=0，编译 0 错）。**待 PTY 代理完成适配后重跑该变异作为 A10 的 n2b 证据。**
- exec_command 取消用例（cancel_flag_stops_managed_processes 等）不受 tui_host 变异影响（独立标志），行为正常。

## 14. A10 完成＋TUI 命令面缺口修复＋全工作区全绿（2026-10-14 /goal 第三轮收尾）

- **A10 变异证据闭环**：`tui_host::set_cancel` 临时吞掉取消请求（A10-MUTATION）→ `n2b_repl_ctrl_c_cancels_tool_group_cli_alive` **失败**（"取消后应报错并返回聊天入口"超时，工具持续运行）；恢复后 n2b 2/2 通过，`grep A10-MUTATION src/`=0。加上 §12 的 A7 变异证据，A10 完成。
- **TUI 命令面生产缺口修复**（PTY 代理如实报告）：母本命令注册表不含本地命令——补 `SlashCommand::{Models,Refresh,Save,Sessions}` 变体（strum 自动解析/命令名，description 与 available_during_task 臂登记为 008-ADAPT；`/save` 入 supports_inline_args）；`tui_host::run_command` 命令名规范化（母本通道回传不带斜杠）。
- **PTY e2e 适配完成**（子代理）：13 个失败用例全部转绿——s1/s2a/s2b/e26（TUI 屏幕级等待，新增 VT100 屏幕等待设施）、auth_n04/e6/e16/perf（TERM=dumb 行式退化路径，语义断言原样）、e24/e25（启动期授权门→失败退出码 1）。实测关键发现已吸收：raw 子串等待对差分渲染不可靠；Enter 须为 CR 且与文本分开发送（paste-burst 启发式）。
- **全工作区**：`cargo test --workspace --no-fail-fast` ＝ **2652 通过 / 0 失败 / 3 忽略**（38 个测试目标）；`cargo check --workspace` 0 错。

## 15. comprehensive-testing 审计发现并修复真 Bug（2026-10-14）

- **审计发现**：s1 退出断言（本审计新增的 A5 硬化：退出后 `\x1b[?1049l` 必须出现）揭示了**两个真缺陷**：
  1. 驱动未调用 `set_owned_screen(true)`——TUI 实际以 inline 视口运行而非规格定稿的 Owned 全屏（违反 §1 模式定稿与"不默默降级 inline"红线）。
  2. 退出路径未调用 `tui::restore_after_exit()`——备用屏/raw mode/键盘增强/焦点事件全部未恢复（违反 A5 与 001 退出契约）。
- **修复**：`chat_driver::run` 首行 `set_owned_screen(true)?`；`main::run_tui` 退出后调用 `tchat_tui::tui::restore_after_exit()`（失败仅警告）。PTY 原始字节级验证：现在输出含 `\x1b[?1049h`（进入）与 `\x1b[?1049l`（离开）。
- **审计新增测试**：s1 追加退出后备用屏禁用断言（字节级）。
- 全工作区复跑：2651/1/3——唯一失败为 §10 已记录的 007 偶发锁测试（单跑与套件复跑均绿，与 008 无关，维持"原因待确认"）。
- 覆盖映射（A1-A10 现有保护）：A1＝e2e 18 处 VT100 屏幕等待；A2＝vendored streaming 113 用例；A3＝resize/size/transcript 251 用例；A4＝wrapping 87 用例；A5＝vendored 4 恢复用例＋本审计 s1 字节级断言；A6＝待 Windows（见 §16）；A7＝6 攻防用例；A8＝全工作区回归；A10＝A7＋n2b 双变异证据（§12/§14）。

## 16. 独立 code-review 双轴审查与修复（2026-10-14 /goal 第三轮收尾）

### Standards 轴（已修复全部硬违规）
- **ABBA 死锁**（tui_host cmd_resume 与 turn 线程 on_record 锁序相反）→ 修复为无嵌套快照判空后独占 autosave。
- 死代码×3（main.rs sigint_cancel_handler、DriverEvent::Interrupt、CommandOutcome::ShouldExit 消费端）→ 删除。ShouldExit 变体保留（驱动 handle_builtin_exit 已内建返回）。
- 误导注释×3（Lookup 空操作注释、流关闭 Resize(0,0)、Queued"宿主重发"）→ 修正为真实行为描述＋已知缺口登记。
- 纯转发（app_event_sender_clone）与空占位（self_clear_draft_placeholder）→ 删除。
- 未修复的判定性项（Duplicated Code 三处 cancel 守卫、DriverExit 单变体枚举、Middle Man spawn_turn）：保留——驱动代码量小、守卫差异各有语义（tick/app/key 三路径），提取反而模糊。

### Spec 轴（修复按优先级）
1. **P0 授权门 origin 比对缺失**（003 违背：同凭证切 provider 可外发未授权 origin）→ 修复为 (origin, fingerprint) 对比对。
2. **P0 流式收尾破碎**（稳定区行被弃、跨行 markdown 拆碎）→ Final 携带完整原文，驱动 truncate 本轮流式 cell 后建单一 Markdown cell。
3. **P1 裸 exit 不退出**（J10）→ 驱动在 Enter 分发前拦截 `exit` 裸词。
4. **P1 TUI 内横幅未跳过**（J10 显示层替代）→ TUI 判定提前至 banner() 之前。
5. 未修（如实登记）：/resume 会话中投影不可达（resumed_cells 仅启动时取——需驱动侧 cell 注入通道，登记为后续增强）；Ctrl+D 退出（crossterm 的 KeyCode::Null/EOE 处理需上游键位研究）；Queued 输入静默丢弃（母本 turn 排队语义，登记为已知缺口）；授权问句期 Ctrl+C 退出码 1（001 灰区，母本 rustyline 提问路径同行为——保留）。

### verify:full 终验
- `cargo check --workspace`：通过。
- `cargo test --workspace --no-fail-fast`：**2652 通过 / 0 失败 / 3 忽略**（38 测试目标全绿）。
- A6 Windows ConPTY：未执行（无已授权 Windows 环境可用）——**A6 未闭合**，最终报告以真实验收阻断返回。

## 17. 最终交付摘要（2026-10-14 /goal 全部轮次完成）

### 分支与提交状态
- 分支 `feat/008-codex-tui-port` @ b0e8d48（无新提交——按指令全程不提交）；19 个未跟踪路径＋2 个已跟踪修改（Cargo.toml/Cargo.lock），`config/auto_catalog.json`、`db/` 未触碰。
- 合并目标：main（无远端）。

### 真实验证命令与计数（最终候选）
- `cargo check --workspace`：通过（0 错）。
- `cargo test --workspace --no-fail-fast`：**2652 通过 / 0 失败 / 3 忽略**（38 个测试目标）。
  - 既有回归（008 前 282 个 terminal-chat 测试）：282 全部保留通过（部分经 008-ADAPT 适配 TUI 界面/TERM=dumb 路径，语义断言未放宽）。
  - 新增 TUI 测试：tchat-tui 2104＋A7 安全 6＋shims/vendor 上游单测（elapsed 4、string 24、config keymap 16、shell-command 等）＝约 2370。
- A9 复用率终版：**原样 322 / 必要适配 78 / 本地新写 5＝80.5%**（tui/src 与母本逐字节比对；shims 20 crate＋vendor 4 crate 单列）。

### 门禁执行记录
1. tdd：实现过程行为测试先行（§8/§11/§14 各轮 RED→GREEN 记录）；A10 双变异证据（A7 绕过 Paragraph→6 用例全红；set_cancel 吞取消→n2b 失败）。
2. comprehensive-testing：审计发现并修复 2 个真 Bug（Owned 备用屏从未进入、退出后终端模式未恢复——§15）。
3. Security Testing Gate（A7）：6/6 通过（字节级＋屏幕级双查；机制＝ratatui-core span.rs:314 控制字符过滤）。
4. code-review（双轴）：Standards 3 硬违规全修＋死锁修复；Spec P0×2（授权门 origin、流式收尾）＋P1×2（裸 exit、横幅跳过）修复（§16）。
5. verify:full：见上（最终候选全量覆盖）。

### 007 偶发失败结论
§10：**原因待确认**（不写"已排除"）——强相关于多进程并发构建负载；008 未改动 007 代码（git 核实）；单跑与负载回落后均不可复现（隔离副本 55+ 轮＋工作区 12/12）。记录为基线已有、与 008 无关的偶发风险。

### 剩余风险
- **A6 Windows ConPTY 未实测**（无已授权环境）——真实验收阻断，不宣称完整验收通过。
- 已知缺口 4 项（规格头部登记）：/resume 会话中投影、Ctrl+D 退出、Queued 排队、历史检索回填通道。
- 授权问句期 Ctrl+C＝退出码 1（001 灰区，母本同路径同行为）。
- 007 偶发（§10/§17）。

## 18. /resume 会话中投影缺口闭合＋规格状态修正（2026-10-14 完成审查器轮）

- **缺口闭合**：`CommandOutcome::ReplaceCells { output, cells }` 新变体——驱动命令分发通道支持 `/resume` 成功后把投影 cells 立即渲染进 transcript（`cells = new_cells; transcript.jump_to_latest()`）；宿主 `cmd_resume` 返回 ReplaceCells。**S10 e2e 用例**（`s10_tui_resume_projects_history_without_replay`）验证真实调用链：PTY→TUI→`/resume seed10`→历史 cell 渲染（用户/助手消息均出现在屏幕）→`/exit`→`再见!`→退出码 0→mock 零请求（不重放工具）→快照文件字节不变。
- **伴随修复（变异测试发现的真缺陷）**：命令派发后草稿未清空（母本经 popup 派发清空，本地直发路径缺失）——`/exit` 追加到未清的 `/resume seed10` 草稿变成 `CommandWithArgs(Resume, "seed10/exit")`。修复：`BottomPane::clear_composer_for_command()`（`ChatComposer::clear_for_command_dispatch`）在驱动 Command/CommandWithArgs 分支调用。
- **规格状态修正**：从"待批准"改回"**实现中（A6 Windows ConPTY 未闭合）**"——A6 未通过不能标"待批准"（验收规则：验收未闭合保持"实现中"）。已知缺口列表更新（/resume 投影划线闭合）。
- 全工作区终验：**2653 通过 / 0 失败 / 3 忽略**（38 测试目标；2652＋S10 新用例）。

## 19. 最终收敛轮：剩余交互契约核实与闭合（2026-10-14 /goal 第五轮）

### 核实结论（四项逐一）
1. **Ctrl+D 退出（J10 承诺，原缺口）**：修复——驱动在 is_ctrl_c 后拦截 Ctrl+D：空输入
   （`composer_text_with_pending().trim()` 为空）→ `DriverExit::Quit`（恢复终端＋`再见!`＋码 0）；
   非空交 composer 编辑域（母本键位；textarea 无删字符映射则为无操作）。证据：**S12 e2e**
   （修复前红灯：子进程不退出；修复后绿：码 0＋`\x1b[?1049l`＋零请求）。
2. **turn 期间提交不丢输入（原"Queued"缺口，核实后范围修正）**：
   - 母本语义核实（chat_composer.rs:2733-2741）：turn 运行中 **Enter（submit_keys）返回
     `Submitted`**（母本由 chatwidget 转 steer 注入当前 turn）；`Queued` 仅 queue_keys
     （Tab，页脚提示 "tab to queue message"）触发。本地同步 turn 不支持 steer。
   - 修复：Submitted 且 turn_running → 入驱动队列（`queue_input`＋pending_input_preview
     排队预览＝母本 UI）；queue_keys 的 Queued 同队列。turn 结束（Final/Failed）后 Tick 按
     **空闲提交同语义**逐条重放（`classify_idle_submission`：裸 exit→Quit；`/已知命令`→
     run_command；未知斜杠/其余→turn；前导空格斜杠＝普通文本，对齐 composer 空闲解析）。
     至多一个在飞 turn（J11）。证据：**S11 e2e**（延迟 mock 使第一轮在飞时提交第二条→
     第二请求携带排队原文、两轮回复均渲染；修复前红灯 30s：第二轮请求从未发出＝输入丢失）。
   - **伴随发现并修复两个真缺陷**（驱动探针定位）：
     a. **并发双 turn＋turn_cell_start 覆盖**：修复前 turn 期间 Submitted 被当新 turn 直接
        spawn——两个宿主线程并发、后轮 Final 的 truncate 用前轮 start 把第一轮 cell 删掉
        （S11 红灯直接原因）；
     b. **启动占位 cell 污染**：main.rs 启动注入 `resumed_projection()`（空历史）→
        thread_to_transcript_cells 塞入 "No transcript content available" 占位 cell，
        有内容后残留为永久 cell。修复：启动 cells 恒空（DriverHooks 删除 resumed_cells
        字段；占位文案仅 /resume 空会话投影给出）。e2e 就绪标记随之从该占位文案改为
        footer "? for shortcuts"（e2e_acceptance 9 处＋session_cli e26 2 处）。
3. **输入历史检索回填（001 rustyline 目标保留）**：会话内上/下键回溯由 composer
   local_history **母本原样路径**承接（提交时 record_local_submission 内建记录；
   persistent_entry_count=0 时 navigate 即时返回）。证据：**S13 e2e**（提交后按 Up，
   光标行（composer 草稿区）出现已提交原文；Enter 可重发＝mock 恰 1 请求的对照断言）。
   - **S13 首跑红灯暴露 A1 缺口并修复**：驱动 draw 从未 `set_cursor_position`（母本
     chatwidget 在 composer 聚焦时设置）——终端光标停在上次写入位。修复：draw 闭包经
     `Renderable::cursor_pos`＋`Frame::set_cursor_position` 定位（A1 光标保真）。
   - **LookupMessageHistoryEntry 不可达证明**：驱动链无 `set_history_metadata` 调用
     （chat_driver/tui_host/main 均无；BottomPane::set_history_metadata 仅存 pub(crate)
     无生产调用方）→ persistent_entry_count 恒 0 → 查找仅在 offset < 该值时发出。
     001 rustyline 亦无跨会话历史文件（main.rs 无 load_history），等价成立。
     驱动 no-op 处注释已改为该精确表述。
4. **授权问句期间 Ctrl+C（001 灰区）**：现状＝rustyline Interrupted → prompt_y_n 视为
   拒绝（与答 n 同路径）→ 错误＋**退出码 1**、零请求、无 `再见!`、不落盘授权。证据：
   **S2c e2e** 钉住现状＋**A10 变异**（prompt_y_n 读取错误分支临时改为 true→S2c 红；
   恢复→绿，S2b 不受影响）。处置：安全侧（拒绝不外发），**登记为待决项**——改为优雅
   退出（0＋再见）属公共行为变更，交用户裁决（规格头部"待决项"）。

### 其他登记差异（非缺口）
- turn 期间斜杠命令（/models 等）立即派发不排队（母本同路径；/resume 会短暂等 history
  锁至本轮 turn 结束——既有行为，未改）。
- 非空草稿 Ctrl+D＝无操作（rustyline 为前向删字符；编辑域细节差异）。

### 忽略测试清单（终验 3 个，核实验收关联）
- `bottom_pane::textarea::tests::altgr_ctrl_alt_char_inserts_literal`——vendored
  `#[cfg_attr(not(windows), ignore)]`（AltGr 仅 Windows 键盘行为）；关联 A6（Windows
  未闭合即不执行，与阻断一致）。
- `tui::owned_screen_tests::owned_screen_lifecycle_child`——子进程助手，由绿色父测试
  `owned_screen_preserves_inline_viewport_across_overlay_handoff_and_resume` 经
  CHILD_ENV 派生执行（行为已验，standalone --ignored 运行失败属预期）。
- `tui::panic_tests::caught_panic_pty_child`——同上，由绿色父测试
  `caught_panic_rehides_cursor_before_repainting_and_restores_its_shape` 派生执行。

### 本轮改动面（全部为 008 本地文件，vendored 文件零改动）
`tui/src/chat_driver.rs`（队列/Ctrl+D/光标/分类＋4 单测）、`src/main.rs`（hooks 删
resumed_cells）、`tests/e2e_acceptance.rs`（S2c/S11/S12/S13＋就绪标记 9 处）、
`tests/session_cli.rs`（e26 就绪标记 2 处）；A7 补错误消息入口用例时将
`tui/src/history_cell/base.rs` 的 PlainHistoryCell/::new 提 pub（含 mod.rs re-export）。
复用统计随动：**原样 321 / 必要适配 79 / 本地新写 5＝80.25%**（base.rs 由原样转必要适配；
README 与 audit 清单同步）。

### 终验（当前候选）
- `cargo check --workspace`：通过（0 错）。
- `cargo test --workspace --no-fail-fast`：**2662 通过 / 0 失败 / 3 忽略**（38 测试目标；
  上一候选 2653＋本轮 9：S2c/S11/S12/S13 e2e＋classify 4 单测＋A7 错误消息入口 1）。
- A7 门禁补强核对（指令第 3 节）：真实 Owned 输出链证据＝
  `a7_byte_level_writer_passthrough_check`（custom_terminal::Terminal.draw→backend 字节
  →vt100 屏幕）；入口覆盖齐备＝模型（markdown cell）、流式（StreamController）、工具输出
  （resume 投影）、**错误消息（本轮新增 PlainHistoryCell 用例）**、超链接（全链 ESC 零直通
  ⇒OSC-8 注入不可能，白名单通道为可信路径）。
- 007 偶发失败：维持 §10「**原因待确认**」（本轮全量一次通过，无新增证据，不扩大实验）。
- **A10 补 A1/A2 抽样变异**（指令核对发现 §12/§14 仅覆盖 A7＋set_cancel，规格 §5-A10
  要求 A1/A2/A7 三抽样）：
  - A1：markdown 渲染 bullet 前缀临时加一空格 → markdown_render 快照多例**失败**
    （blockquote/list 系）；恢复 → 216/216 绿。
  - A2：`on_commit_tick` 临时吞掉稳定区 cell（返回 None）→ streaming 全套**失败**
    （113 例含 representative/table 系）；恢复 → 113/113 绿。
  - 残留核查：`grep A1-MUTATION|A2-MUTATION|A10-MUTATION tui/ src/`＝0。
  - 另有真实缺陷对照（非人工变异，同样证明检出力）：S13 红灯检出 A1 光标缺失、
    S11 红灯检出 A2 turn cell 记账覆盖（本轮修复记录见上）。
- A1–A5、A7–A10 维持通过（A1 因光标定位修复增强）；**A6 Windows ConPTY 仍未闭合**
  （无已授权环境）——规格保持「实现中」。

## 20. 最终审查轮：独立 code-review 与修复（2026-10-14 /goal 第六轮）

### 契约核对（指令第 1 节）
- 非空 Ctrl+D 与 J10/A4：**无契约冲突**——J10"Ctrl+D 退出"承诺与 001 行式实测一致
  （rustyline 仅空行返回 Eof→`再见!`＋0）；非空＝母本 composer 键位（无删字符映射→
  无操作；rustyline 非空行为前向删字符，属编辑域差异）。已修正不准确表述：驱动注释、
  S12 注释与 J10 表行（"对齐 rustyline 语义"→精确退出条件＋差异登记）。
- 授权问句 Ctrl+C vs 启动早期 SIGINT 130：**阶段不冲突**——问句前/后的裸启动窗口＝
  cooked mode＋默认 SIGINT（130）；问句期间＝rustyline raw mode 读键（提问语义＝拒绝，
  S2c）；与 001 中 `你> ` 提示符期 Ctrl+C≠130 同理。J10"启动早期"行已补适用边界说明。

> **TDD 记录更正（2026-10-14 整改轮，按指令补记）**：本节各"红灯证据"的真实顺序为
> **修复先行、临时破坏对照补证**——§19 各缺陷在修复实现后才补测试，红证据经"临时恢复
> 缺陷行为→测试失败→恢复修复→通过"的对照取得（仓库规范允许的两种检证方式之一），
> 非严格的测试先行 TDD。变异对照保留为检出力证据；正确实现不回滚补造流程。
> §21 起恢复测试先行（红→绿）。

### 独立审查（指令第 2 节；实现 Agent 自查不可替代——独立 Agent 只读审查）
上轮 §16 覆盖旧候选；本轮收敛改动（§19）由独立审查覆盖。结果：**P0×2、P1×3、P2×10**，
全部处置：

**P0-1 turn 期 Info cell 被 Final 截断（已修）**：原 `cells.truncate(turn_cell_start)`
无条件截到提交时下标，把 turn 期间推入的 Info/错误 cell（工具活动行 `[工具] …`＝006
通知、autosave 写失败提示＝007 告警）一并删除（行式路径经 say() 持久保留）。修复：
改为 `stream_indices` 集合记账——Delta 记录流式 cell 下标，Final/Failed 倒序
`cells.remove` 仅移除流式 cell，Info/错误 cell 保留。**兼修 P1-3**（不再保留跨事件
窗口的绝对下标，/resume ReplaceCells 后的陈旧 start 交错不复存在）。
红灯证据：s14 e2e（工具轮＋含换行的最终回复→`[工具] exec_command` 在完成后存留）；
临时恢复旧截断语义（截到提交时下标）→s14 红，恢复→绿。

**P0-2 TUI 退出缺 flush_final（已修）**：行式退出前执行会话排空（007），TUI 分支缺失
——on_history 写失败的滞留待写段在退出时全部丢失。修复：`TuiHost::flush_final_session`
（先无嵌套快照 history 再独占 autosave——避免与 turn 线程 history→autosave 锁序构成
ABBA）；main 在 `再见!` 前调用，失败 `say` 报错（与行式同文案）、不改变退出码。
红灯证据：s15 e2e（子进程 RLIMIT_FSIZE 1024B＋SIGXFSZ 忽略——对已打开 fd 的 write
同样生效，chmod 无效；累计超限→追加失败提示可见＋退出排空失败明确报错）；临时移除
排空调用→s15 红，恢复→绿。

**P1-1 /resume summary 被 ReplaceCells 覆盖（已修）**：先 push 后整体覆盖＝summary 与
degraded 只读告警（007 安全提示）从不渲染。修复：先替换投影、后 push summary。
红灯证据：S10 增强（wait_screen("恢复 2 条")）；临时回退顺序→S10 红，恢复→绿。

**P1-2 未知斜杠重放误发模型（已修）**：classify 曾把未知斜杠当普通文本 spawn_turn，
而空闲按键路径拒绝（composer UnknownCommand，不发请求）。修复：斜杠前缀（非空名）
一律派发命令通道——宿主 run_command 对未知命令输出"未知命令"提示（与空闲拒绝语义
一致：不发给模型）。单测更新（classify_unknown_slash_routes_to_command_not_model）。

**P2 修复（小而实）**：
- P2-6 修饰键守卫：裸 exit 判定要求 `modifiers.is_empty()`（Shift+Enter 是换行键）；
  is_ctrl_d 增排除 ALT（AltGr）；新增修饰键矩阵单测。
- P2-1 live tail 清理：draw 无活动流时 `sync_live_tail(None)`（流结束后旧尾区行不残留）。
- P2-8 用户消息 cell：提交（含重放）推 `UserHistoryCell`（与 /resume 投影同类型——
  实时视图与恢复视图一致，母本 transcript 含用户消息）。S1 增强：回复后用户原文
  持久在屏（提交后草稿已清空，断言只可能由 transcript cell 满足）。
- P2-9 S11 路径锚定：排队后等待预览独有文案 "Queued follow-up inputs"（在第一轮
  完成前出现）——若退化成直接提交路径则断言失败，防回归被负载掩盖。

**P2 登记不修（已知风险，规格头部同步）**：P2-2 turn 线程 panic→turn_running 卡死
（毒锁级异常路径，无恢复机制；Ctrl+D/exit 可逃生）；P2-3 取消标志 spawn 窗口双重复位
（006 既有，调度延迟级窗口）；P2-4 turn 期间退出路径不取消宿主线程（exec 孤儿进程/
apply_patch 半途——004 state_known、007 跳行修复兜底；排空已由 P0-2 覆盖待写段）；
P2-5 crossterm 事件流 Err 当正常退出（码 0）；P2-7 TUI 跳过 F7 权限审计输出（行式打印
告警；进 TUI 为 info cell 属设计决策，待裁决）；P2-10 turn 期间 /save 阻塞 UI（与
/resume 锁等待同类）。

**审查核实无问题清单**（要点）：两条输入路径入队/单 turn 不变量、多轮重放记账、
Ctrl+D 与 composer 分支关系、draw 光标安全、main 退出映射、ABBA 锁序复核、
PlainHistoryCell pub 越权面、A7 用例非空跑、e2e 等待逻辑。

### Windows A6 补测指令（指令第 3 节）
已写入 handoff §11（自包含）：平台事实（full CLI 因 003/007 Unix-only 契约不可 Windows
编译——`killpg`/`/bin/sh`、`flock`、libc PTY 测试；渲染层 tchat-tui/shims/vendor 上游式
跨平台门控）；候选打包（排除 target/db/auto_catalog.json）；构建依赖（rustup≥1.88、
MSVC onig、crossterm fork 网络）；Tier 1 scratch 驱动（真实 chat_driver::run_interactive
＋stub hooks）；ConPTY（node-pty＋@xterm/headless）8 场景断言表（启动/中文输入与光标/
回复渲染/滚动 resize/粘贴/Ctrl+C 空闲/工具期取消/退出恢复）；AltGr 用例＝Windows 上
自动执行（vendored cfg 属性）；缺执行条件 6 项；交叉编译或 Linux PTY 不能关闭 A6。

### 本轮改动面
`tui/src/chat_driver.rs`（stream_indices/用户消息 cell/classify/修饰键/live tail＋2 单测）、
`src/tui_host.rs`（flush_final_session）、`src/main.rs`（退出排空）、
`tests/e2e_acceptance.rs`（s14/s15＋S1/S10/S11 强化）。复用统计不变（80.25%；全部
本地文件）。变异残留 grep＝0。

### 终验（当前候选，本轮修复后）
- `cargo check --workspace`：0 错。
- `cargo test --workspace --no-fail-fast`：**2665 通过 / 0 失败 / 3 忽略**（38 目标；
  2662＋s14/s15/修饰键单测）。e2e 18/18。
- 门禁链（本轮有修复）：tdd（s14/s15/S10/S1/S11 红绿对照）→ comprehensive-testing
  （独立审查充当系统回归审计）→ Security Gate（A7 面无改动，A7 7/7 维持）→
  code-review（独立审查，本轮）→ verify:full（全量 2665/0/3）。
- **A6 仍未闭合**（无 Windows 执行入口）——规格保持「实现中」。

## 21. 整改收敛轮：六项剩余 P2 逐项处置（2026-10-14 /goal 第七轮）

### 待决项关闭
授权问句 Ctrl+C＝拒绝＋码 1：**2026-10-14 用户裁决保留现状**，规格头部待决标记关闭
（S2c＋A10 变异证据维持有效）。Ctrl+D 阶段边界表述维持上轮精确化结果。

### 六项 P2 处置（每项先红后绿——本轮恢复测试先行）

**P2-2 turn 线程死亡卡死 → 已修复**。缺陷：run_turn_inner 线程 panic（如毒锁）静默
死亡，Final/Failed 永不到达，turn_running 永久卡死（Ctrl+C 只反复置标志）。
修复：线程体包 `catch_unwind(AssertUnwindSafe)`，panic 经独立 tx_panic 回推
`Failed("内部错误：会话线程异常终止，本轮已中止")`。红→绿：单测
`tui_host::tests::turn_thread_panic_surfaces_as_failed_event`（隔离 env＋预授权越过
003 门＋毒化 history；修复前 recv 超时 Disconnected 红，修复后 Failed 绿）。
注：毒锁后下一次提交仍会 panic→再次 Failed（可用的降级循环，不悬挂）。

**P2-3 取消标志 spawn 窗口双重复位 → 登记不修（可延期，非违约）**。依据：
tui_host 线程首行复位＋turn.rs 工具轮前复位（006 既有已验收实现，行式路径同窗口）；
窗口为线程调度延迟级且位于任何工具执行之前——此阶段取消的对象（工具）尚不存在，
006 验收语义即"工具执行期间"可取消。TUI 与行式奇偶一致，无契约违反。

**P2-4 退出不取消宿主线程 → 已修复**。缺陷：TUI 退出（/exit、裸 exit、Ctrl+D）直接
process::exit：工具子进程组孤儿化、退出排空（P0-2 引入）阻塞至工具超时。
修复：main 退出路径在 flush_final_session 前 `host.set_cancel(true)`——006 N2b 语义
（标志→exec 轮询终止进程组→turn 迅速返回释放 history）。红→绿：s16 e2e
（sleep 20 工具执行中 /exit，8s 上限等 `再见!`＋码 0；修复前阻塞至工具结束→红）。
残留上界：turn 停在 HTTP 请求中时取消标志不被轮询，退出等待以 api 总超时（180s，
003 已验收）为界；exec 孤儿由其 timeout_ms 上限自行终止（006"通道断开≠取消"语义）。

**P2-5 事件流错误当正常退出 → 登记不修（可延期，非违约）**。依据：001 已验收行式
路径对读取错误的处理即 `Err(_) => break`→`再见!`＋码 0（main.rs:97"其他读取错误…按
退出处理"）。TUI 对 crossterm 事件流 Err 的处理与该已验收语义奇偶一致，无契约违反。

**P2-7 F7 审计输出遗漏 → 已修复**。缺陷：行式启动打印审计（内容/脱敏/收紧行为），
TUI 分支跳过。修复：授权后运行同一 `catalog::audit_file_permissions(Some(".env"),
&config_dir())`（同参＝内容、脱敏（只读元数据不含秘密值）、自动收紧行为全保留），
结果经 `DriverHooks.startup_notices` 以**既有通知 cell（PlainHistoryCell）**渲染
（复用而非新 UI）。红→绿：s17 e2e（manual.json 0644→"权限收紧为 0600" 通知渲染＋
实际 0600 副作用；修复前无通知→红）。副作用说明：审计保持自动收紧（既有行为），
现所有 TUI e2e 首屏多出审计通知行（全量 21/21 无断言冲突）。A7 补例：
`a7_startup_audit_notice_with_hostile_path_is_neutralized`（敌意 TCHAT_CONFIG_DIR
路径注入通知行，ESC/C1/CR 零直通——A7 8/8）。

**P2-10 /save（与 /resume 同类）turn 期间锁等待 → 已修复（含审查前提修正）**。
核实修正：审查者"UI 冻结"前提对键盘路径不成立——composer 的
`available_during_task` 门（母本行为；本地命令登记"工具运行中禁用"）在派发前即拒绝
（屏幕提示 "'/save' is disabled while a task is in progress."＋恢复草稿）。
保留修复为纵深防御：cmd_save/cmd_resume 的 history **try_lock** 让位（"对话进行中…"
提示）——任何绕过 composer 的直接派发（如未来通道）也不会等锁冻结；cmd_resume
锁序与 §16/§20 ABBA 结论保持一致（判空 try_lock 成功＝无在飞 turn→autosave→history
嵌套安全）。红→绿：s18 e2e（turn 期间 /save 立即被拒——拒绝提示先于本轮回复出现＝
未阻塞；恢复草稿在轮后直接回车重试成功落盘；修复前红于"对话进行中"等待——
测试按实际分层行为改写并保留"不阻塞"检证）。

### 本轮改动面
`src/tui_host.rs`（catch_unwind＋try_lock×2＋单测）、`src/main.rs`（退出取消＋审计
startup_notices＋run_tui 签名）、`src/lib.rs`＋`src/catalog.rs`（crate 级测试 env 锁，
catalog 委托——多模块 env 测试互斥）、`tui/src/chat_driver.rs`（startup_notices 字段
＋通知 cell 渲染）、`tests/e2e_acceptance.rs`（s16/s17/s18）、
`tui/tests/security_untrusted_text.rs`（审计入口 A7 用例）。复用统计不变（80.25%，
全部本地文件）。

### 验证（当前候选）
- 单测红→绿 4 组：P2-2 单测＋s16/s17/s18 e2e（各先红后绿，记录如上）。
- `cargo test --test e2e_acceptance`：**21/21**；A7 安全面 **8/8**（含审计入口新例）。
- `cargo check --workspace`：0 错；`cargo test --workspace --no-fail-fast`：
  **2670 通过 / 0 失败 / 3 忽略**（38 目标；2665＋本轮 5）。
- 独立复审（本轮差异）：见 §21 末尾补记。

### 独立复审（本轮差异）与整改（§21 补记）
独立 Agent 只读复审本轮六项整改：**无 P0；P1×1；P2×6**，全部处置：

- **P1-1 `/resume` 遗漏 available_during_task=false 登记（已修）**：Resume 留在母本
  true 臂（本地实现持 history/autosave 锁，与母本后端语义不同）——turn 期间
  /resume 绕过 composer 门直达宿主：既造成 UX 不一致（草稿被吞 vs /save 保留），
  又使"判空 try_lock 成功＝无在飞 turn"不变式不成立（前置窗口内 autosave→history
  嵌套＝ABBA 残留窗口）。修复：①Resume 移入 false 臂（slash_command.rs，与
  Models/Refresh/Save/Sessions 对齐）；②cmd_resume 解嵌套——autosave 锁内完成
  resume 取结果并释放，再写 history（反向嵌套不复存在）。红→绿：s18 增强
  （turn 期间 /save 与 /resume 均被 composer 门拒——红灯时 /resume 屏显
  "对话进行中，/resume 已跳过"＝直达宿主；修复后屏显 disabled 提示）。
- **P2-1 毒锁善后（已修）**：flush_final_session 与 cmd_save/cmd_resume 的锁操作
  毒锁感知——`unwrap_or_else(into_inner)`/`TryLockError::Poisoned` 分支恢复数据
  （退出不再 panic 码 101；Poisoned 不再误报"对话进行中"）。单测补断言：毒锁下
  flush_final_session 必须完成。
- **P2-2 panic 区间 SIGINT 处理器残留（已修）**：catch_unwind 的 Err 分支先复位
  `SIGINT → SIG_DFL` 再回推 Failed。
- **P2-3 触及面 lint（已清）**：删除未用 `cancel` 绑定/`DriverHooks` 导入/`_m`；
  `run_tui` 移除未用 `agent` 参数；信号处理器改两步指针转换（与 main.rs 同式）。
- **P2-4 审计时序（已修）**：审计移至授权之前＝行式"审计→读配置"顺序（0000 权限
  角落行式先自愈再读，TUI 曾先读失败）。
- **P2-5 单测环境残留（已修）**：EnvRestore Drop 守卫清理三个 TCHAT_* 变量。
- **P2-6 测试加固（已修）**：s17 先等 TUI 哨兵（防行式退化假通过）；s18 否定断言
  改屏幕级。
- 复审判定"核实无问题"：catch_unwind 机制（捕获/移动/Ok-Err 区分/无双事件）、
  /save 分层、退出取消上界（api 180s/exec 10ms 轮询，s16 实测 0.81s）、TEST_ENV_LOCK
  覆盖、startup_notices 渲染、三 e2e 判别力、A7 新例。

### 终验（整改＋复审修复后，当前候选）
- `cargo check --workspace`：0 错（触及面无新增 warning）。
- `cargo test --test e2e_acceptance`：**21/21**；A7：**8/8**；
  `cargo test --workspace --no-fail-fast`：**2670 通过 / 0 失败 / 3 忽略**。
- 红绿记录：P2-2 单测、s16、s17、s18（含 /resume 门）均先红后绿；
  §20 已按真实顺序更正（修复先行＋变异对照，§21 起恢复测试先行）。
- **A6 Windows ConPTY 仍未闭合**（无执行入口；补测指令＝handoff §11）——规格保持
  「实现中」，不称可批准交付候选。

## 22. 定点复核与 A6 交接出包（2026-10-14 /goal 第八轮）

### 定点独立复核（原独立审查者，上下文延续）
对第七轮六项修复逐项复核——**全部关闭**（证据 file:line 由复核者亲自读出并定向实测：
tui_host 单测 1/1、slash_command 30/30、s16/s17/s18 3/3）：
1. P1-1 /resume 门（false 臂）＋cmd_resume 解嵌套（autosave 块内取结果→释放→独立写
   history；全程无持 A 取 B）——关闭；
2. P2-1 毒锁善后三处＋毒锁 flush 断言——关闭（连带发现 P2-新1，见下）；
3. P2-2 SIG_DFL 先复位再回推——关闭；
4. P2-4 审计先于授权（＝行式顺序）——关闭；
5. P2-5 EnvRestore 清理——关闭（附守卫上移建议，已采纳）；
6. P2-6 s17 哨兵＋s18 屏幕级否定断言＋/resume 门覆盖——关闭；
另确认 catch_unwind 机制与退出取消/排空路径无回归；触及面 lint 全清（src/ 警告 0）。

### 复核引出的修复（TDD：红→绿）
- **P2-新1 毒锁下 /resume 主线程 panic**：`resumed_projection` 裸 unwrap——run_command
  在驱动主线程同步执行，panic 以 101 中止且跳过终端恢复。红灯：单测扩展（毒化后
  `run_command("/resume poison-snap")`——修复前主线程 panic 于该行；实测取得）。
  修复：`unwrap_or_else(into_inner)`＋登记注释（含 P2-新2：假想直发通道的最坏情形
  已降级为阻塞至轮结束＋一致性瑕疵，纵深防御姿态）。绿：单测断言 ReplaceCells 返回。
- **P2-低采纳**：EnvRestore 守卫上移至 set_var 后（中途失败也清理）。

### 最终完整验证（本轮修复后）
`cargo check --workspace`＝0 错；e2e **21/21**；A7 **8/8**；
`cargo test --workspace --no-fail-fast`＝**2670 通过 / 0 失败 / 3 忽略**。
两项已说明不违约的延期风险（取消窗口／事件流错误按正常退出）维持登记。

### A6 补测包（实际出包，2026-10-14）
- `/tmp/008-a6/008-windows-a6.tgz`（1279 文件/约 1.6MB）；清单 `MANIFEST.txt`、
  身份 `CANDIDATE-ID.txt`、staging `pkg/008-windows-a6/`；SHA-256 定版值记录于
  本节末行与包外 `008-a6-package.sha256`（校验值不内嵌包内避免自引用失效）。
- 含候选全部未跟踪文件（tui/ shims/ vendor/ src/tui_host.rs docs/specs/008-* logs/）；
  排除 target/、db/、`.env`（运行凭据）、config 用户本地修改（auto_catalog.json 取基线）。
- 包内新增（候选外语义）：`conpty_probe/`（真实 `chat_driver::run_interactive`＋
  stub hooks：canned 回复 2/40 行、命令回显、取消标志 stderr 标记）、
  `scripts/conpty_run.mjs`（8 场景断言，node-pty＋@xterm/headless）、README-A6.md。
- 本地验证边界：probe 经 `cargo check`（隔离 target-dir）**通过**＝Windows 将运行的
  确切驱动代码对候选接口编译成立；Node 脚本未经本地执行（环境无 Node）——Windows 侧
  先 `node --check`。**Windows 专属结果全部待验证；A6 仍未闭合。**

**A6 补测包定版**（此后未再改动包内容）：SHA-256 `5363e32de0196879305d559cb65443894d49fd2c15987892b279b8a890f3aeae`（1279 文件；清单
`/tmp/008-a6/MANIFEST.txt`）。包内 handoff/证据为定版前一拷（其校验值表述均为
"以包外为准"），工作区本行为权威记录。

**定点复核闭环记录（原审查者最终确认）**：六项修复＋复核引出项全部关闭——P1-1
门＋解嵌套、P2-1 三处毒锁善后、P2-2 SIG_DFL、P2-4 审计时序、P2-5 守卫（含上移）、
P2-6 测试加固、P2-新1（resumed_projection 毒锁感知＋红灯可复现断言）、P2-新2 登记；
catch_unwind/退出路径无回归；确认者独立复测 tui_host 1/1、src 零警告，结论"全部收敛，
无新增发现"。两项不违约延期风险维持登记。

## 23. A6 Windows 验收轮：执行环境核验与包就位（2026-10-14 /goal 第九轮）

**执行环境核验（本会话＝开发容器，结论如实）**：
- 容器内核 WSL2 但为 Docker 容器：无 `/mnt/c`、无 WSL 互操作（`/proc/sys/fs/binfmt_misc`
  空、无 `/init`、PATH 无 powershell.exe/cmd.exe）——**无法执行任何 Windows 二进制**；
- 唯一宿主可见挂载＝D: 共享（容器内 `/vault`，Windows 侧 `D:\AgentLab\shared\Obsidian`）；
  该挂载内无 `D:\AgentLab` 根视图，无法在库外建目录；
- 宿主远程入口核验：host.docker.internal 的 22/3389/5985 全部关闭；无 docker.sock；
- 结论：**Windows 侧验证（构建、cargo test、AltGr、ConPTY 八场景）无法由本会话执行**——
  A6 保持未闭合，规格维持「实现中」。这是真实阻断，非推测（以上均为实测）。

**已完成的可达部分**：
1. 包完整性复核：`/tmp/008-a6/008-windows-a6.tgz` SHA-256＝
   `5363e32de0196879305d559cb65443894d49fd2c15987892b279b8a890f3aeae`（与交接值一致）。
2. 包就位（Windows 侧无需容器访问）：完整包＋校验/清单/身份＋`WINDOWS-RUN.md`
   （取包核验、前置、执行命令、结果回写要求）已放置于
   `/vault/008-windows-a6/`＝Windows `D:\AgentLab\shared\Obsidian\008-windows-a6\`；
   落位后复验 SHA 一致；未覆盖任何已有内容（新建目录）。
3. Windows 执行入口就绪后的路径：按包内 README-A6.md/WINDOWS-RUN.md 执行；
   失败时保留现场带回（勿放宽断言）；产品修复须回 /workspace 原分支走门禁并重建包。
