# 规格：TUI 备用屏文字选择与右键复制

> 状态：**已实现并本地验证（2026-10-15 实施轮＋补验轮，待审阅候选；宿主粘贴实测项保留——OSC 52 已正确发送可证，宿主实际粘贴仍需现场验证）**　分支 `feat/008-codex-tui-port`，未提交。全量 2741/0/3（补验轮＝2738＋3）；e2e 63/63；A7 11/11。实现记录：`docs/records/2026-10-15-010-implementation.md`。
> 需求来源：用户明确要求"备用屏内可以选择文字，并右键复制"。008 §1/§32 已将 `transcript_view/{selection,select_mode,copy_mode}`、`clipboard_copy`、`text_selection`、`rendered_selection` 列入移植范围并已落地（vendored 零缺口）；A3 滚动修复轮的"不启用选择/复制"是该轮范围边界，本规格立项后由本规格承接。
> 母本：[openai/codex](https://github.com/openai/codex) 固定提交 `97771b1d78cf6366bc3946fe9e4bbca4ebee31d1`（本地 `/tmp/codex-upstream`）。
> 本轮仅调查＋规格，不修改生产代码。

## 1. 目标 & I/O

- **目标**：Owned transcript（备用屏全屏）模式下，鼠标拖选历史区/输入区文字并以高亮反馈选区；**右键复制当前选区**到宿主剪贴板，复制结果有如实反馈（成功/已发送未确认/失败）。选择与复制不提交消息、不改动草稿、不中断进行中 turn 的既有语义。
- **输入**：crossterm 鼠标事件（Down/Drag/Up(Left)，Down(Right)；捕获模式 1000/1002/1003 已由 vendored `tui/alternate_screen.rs:53-65` 启用，A3 轮已实测事件到达）。
- **输出**：选区高亮（REVERSED 样式）；剪贴板写入（本环境＝OSC 52 转义序列，见 §3-K4）；composer gap 反馈行（"Copied N chars"/"Copy sent to terminal · paste to verify"/"Copy failed"，vendored `transcript_view/composer_gap.rs:118-153`）。
- **范围外（明确不做）**：Ctrl+O copy picker（语义目标导航，母本 `copy_mode.rs:22-60`，无用户需求）；`select_mode/` 键盘 caret 模式（母本自身标记 dead_code，`select_mode.rs:44-45`，生产不可达）；transcript 搜索；打开链接（`ViewAction::OpenLink` 的 Ctrl/Super+点击路由为随选区免费获得的最小项，不扩展浏览器逻辑）；X11 PRIMARY/中键粘贴（容器内 `primary::available()` 恒 false，`clipboard_copy/primary.rs:9-17`，不宣称支持）；右键粘贴（母本 `app/right_click_paste.rs` 需剪贴板读取，容器内 arboard 不可用、OSC 52 读不被终端普遍支持，不启用）。
- **交互面（对齐母本 input.rs:253-392）**：Left 拖选（单击/双击/三击＝字符/词/行，`text_selection.rs:51-66`）；Shift+点击扩展；Right Down 复制当前选区（`input.rs:334-339`）；滚轮保持选区；Esc/输入键/composer 点击清除选区；composer 区内的选择与右键复制同链路（`bottom_pane/mod.rs:1607/1619/1639` 包装已在）。

## 2. 边界 & 风险

- [ ] **剪贴板通路（容器→宿主）**：容器内无 `DISPLAY`/`WAYLAND_DISPLAY` → arboard 原生路径失败 → vendored 回退 OSC 52（`clipboard_copy.rs:237-245,294-312`：`ESC]52;c;<base64>` 写 `/dev/tty`，tmux 包裹按需）。OSC 52 生效条件＝最外层终端模拟器支持（以下为**上游文档载明，本环境未实测、不构成成功保证**——补验轮 §3-K4 措辞收敛）：Windows Terminal 自 2020 年 v1.x 支持写剪贴板（1.23 起提供禁用开关，[devblog](https://devblogs.microsoft.com/commandline/windows-terminal-preview-1-23-release)；conhost 不支持，[terminal#18943](https://github.com/microsoft/terminal/issues/18943)）；kitty/WezTerm/iTerm2/Ghostty 等支持。**不做"必成功"假设**：不支持/被禁用时反馈"Copy sent to terminal · paste to verify"或失败文案，如实呈现（母本 Unconfirmed 语义）。载荷上限 100,000 原始字节（`clipboard_copy.rs:34`，OSC52_MAX_RAW_BYTES），超限＝失败反馈。
- [ ] **终端截获右键 vs 应用收到右键**：鼠标捕获开启时右键作为事件到达应用（本路径）；个别终端可配置为拦截右键做粘贴/菜单——属宿主终端行为，不在应用侧补偿。**Shift+拖选（终端原生选择，不向应用报事件）作为兼容补充保留**，不替代本规格的默认拖选＋右键复制（需求红线）。
- [ ] **安全（新增剪贴板边界）**：选区文本来自不可信内容（模型回复/工具输出）。进入终端字节流的唯一新增通道＝OSC 52 序列，载荷经 base64 编码（编码即围栏，原始控制序列不可逸出）；高亮渲染走 ratatui cell 路径（控制字符过滤，A7 已证）。A7 扩展用例：选区/复制路径注入（含恶意 OSC 52 伪装文本）零动作。**不可信工具输出不得自行触发剪贴板写入**——写入仅由用户显式选择动作触发。
- [ ] **复制异步性与竞争**：ClipboardWorker 异步（5s 启动预算，`worker.rs:22-23`）；Pending 期间重复复制＝Busy 反馈；Pending 票据由 `Selection::pending_copy` 持有，完成经 poll 回收（`selection.rs:498-537`）。驱动每事件/每 tick 必须 poll（OSC 52 字节写终端发生在 poll 内，`worker.rs:504-509`——不 poll 则复制永远不落地）。
- [ ] **既有回归**：Ctrl+C 退出契约（J10）保持——选区不得接管（K5 已落实，见 §3-K5）；拖选与 011 滚动条的沟位互斥（011 承接）；PTY e2e 的 QueryResponder/读节奏（008 坑 16/17）沿用。

## 3. 关键判断（为什么）

**调查结论（2026-10-15，源码证据）**：vendored 实现链**完整且已编译进 crate**（单元测试在 2688 基线内运行），缺口**只在驱动层**：

- **本地缺口**：
  1. `chat_driver.rs:362-381`：transcript 区只路由 ScrollUp/ScrollDown，点击/拖动/右键丢弃；`handle_mouse` 返回的 `ViewAction`（含 `Copy(String)`）被丢弃（`:374`），`handle_key` 返回值同样未消费（`:292`）。
  2. 键路由谓词 `transcript_owns_scroll_key`（`chat_driver.rs:425-449`）显式排除 Ctrl+Space 等选区键（A3 范围边界注释 `:423-424`）。
  3. 驱动从不 poll `tui.clipboard`（`tui.rs:690` 字段已在，`worker.rs:448 poll`）、从不调 `finish_copy`（视图 `selection.rs:498`、composer `bottom_pane/mod.rs:1607`）、从不在 Draw 调 `tick_selection`（`selection.rs:616-640`，拖选期间视口边缘自动滚动）。
  4. 视图剪贴板策略旗标未设置（母本 `app/owned_transcript.rs:79-110` sync_owned_transcript 设 `copy_on_select/primary_selection/...`）。
- **母本入口（固定提交，file:line）**：分发序 `app/owned_transcript.rs:447-487`（composer prepare 先行→composer 复制键/右键（`chat_composer/mouse.rs:59-91`）→composer 鼠标→transcript `handle_mouse`）；选择状态机 `transcript_view/selection.rs:132/227/181`（begin/extend/end）、快照冻结（turn 期间选区不漂移）、渲染 `selection.rs:652-713`＋`text.rs:430-457`；复制执行 `selection.rs:433 copy_action`→`tui/selection_clipboard.rs:48 copy_selection`→`clipboard_copy.rs:96 copy_to_clipboard`（arboard→WSL powershell→tmux→OSC 52 回退链）；反馈 `composer_gap.rs:49/118`。
- **必要适配（全部在 `chat_driver.rs` 本地新写模块，vendored 文件零改动）**：
  1. 鼠标分发改为母本序：`pane.prepare_composer_mouse` 无条件先行（冲刷 paste burst）→composer 区（含其右键复制，经 `mod.rs:1619` 包装＋闭包 `|text| tui.copy_transcript_selection(text, PlainText)`）→transcript 区 `handle_mouse(mouse, &cells)`，返回 `Some(action)` 经 `transcript.copy_action(&mut tui, &cells, &action, CopyFormat::Markdown)` 消费（`OpenLink` 经既有 `webbrowser` 路径）。
  2. 键路由谓词扩展：`view.owns_interaction_key(key)`（`input.rs:155`，pub(crate) 可直达）为真时先归 transcript（选区/搜索/活动聚焦期间母本语义）。
  3. 主循环每事件＋每 tick：`tui.clipboard.poll()` → 完成票据分派（视图 `finish_copy` 与 composer `finish_copy` 择一，对照母本 `app/clipboard.rs:56-88` 分派逻辑）。
  4. Draw 路径调 `transcript.tick_selection(&cells)`（返回 true＝需要下一帧，驱动已有 100ms tick 兜底）。
  5. 启动时按母本 sync_owned_transcript 设策略旗标（`copy_on_select` 用 vendeted `local_settings.rs:222-259` Auto 策略原样，不新造开关）。
- **K4 剪贴板路径结论（环境实测面）**：本产品运行环境（容器/远端 pty）唯一可用通路＝OSC 52 经 pty 链到达宿主终端模拟器（容器内 `CopyEnvironment` 各检测位均为否）。e2e 断言以捕获流中的 `ESC]52;c;<base64>` 载荷解码比对为"宿主粘贴"代理证据＋用户真实终端粘贴为最终验收（AGENTS.md E2E 原则：模拟屏幕通过仅为部分证据）。
- **K5 契约点（2026-10-15 实施轮落实；同日按键单一职责轮重定，见 §7）**：**左键拖选历史文字，右键复制非空选区；历史选区不得接管 Ctrl+C**。Ctrl+C 一律退出 CLI（不再承担取消——取消归 Esc；§7）——母本 `owns_interaction_key` 的选区复制键分支（Ctrl+C/Cmd+C/Ctrl+Insert/Ctrl+Shift+C）在本构型不接通，键盘永不触发复制。选区作为鼠标覆盖层：**Esc 不清选区**（单一职责轮起驱动统一拦截，空闲无动作、运行中仅取消——选区保持到用户点击输入区或其他按键路由前恢复）；其余键先恢复无选区态再按无选区路由处理（Enter＝提交、字符＝composer、Ctrl+Space 不路由）。vendored footer 的「Ctrl+C copy」提示在本驱动不渲染（footer_with_navigation 无调用点），不新增任何「Ctrl+C 复制」提示。本轮不新增自动复制配置项（视图 copy_on_select/primary 维持默认 false）。不启用搜索、打开链接（`ViewAction::OpenLink` 丢弃）及其他母本高级交互。
  - **定点修复轮（2026-10-15，见 §6）**：用户现场「有选区时 Ctrl+C 无提示且不退出」——「turn 运行中＝取消任务」此前仅覆盖工具批间/执行中（006 N2b），**在途模型请求（阻塞 ureq，总超时 180s）不轮询取消标志**＝置标志后无可视变化、回复照常到达，与现场症状一致（任务状态未明前提下经验定论，非预设）。修复后 Ctrl+C 运行中分支额外 `jump_to_latest()`（中断＝清选区/搜索覆盖层＋回跟随态）——否则 S48 冻结视口会把取消反馈行挡在视口外（s69b 事前 RED）。K5「键盘永不复制」与空闲 Esc 两步序不变。

## 4. 坑记录

1. **poll 不做＝复制假死**：OSC 52 终端写在 `ClipboardWorker::poll` 内发生（`worker.rs:504-509`）；只调 `copy_selection` 不 poll，选区反馈停在"Copying…"且宿主永远收不到序列。
2. **选区几何依赖真实渲染**：`handle_mouse` 的 inside 判定用 `self.area`（render 时赋值）；首帧渲染前的事件被忽略——测试须先 draw 再喂鼠标（008 坑 17 同源）。
3. **拖选期间 turn 增量到达**：选区快照冻结 cell 修订（`selection.rs:1-2`），`hold_position` 暂停跟随——不得为"简化"绕过快照机制。
4. **paste burst 冲刷次序**：母本对一切鼠标事件先 `prepare_composer_mouse`（冲刷挂起首字符）；漏掉会让"输入后立即拖选"丢首字符渲染。
5. **PTY e2e 鼠标事件须带 SGR 编码喂入**（S20 先例）；右键＝`ESC[<2;x;yM`。捕获内 `Moved` 事件高频——分发序中 Moved 不触发 transcript 重渲染路径外的额外工作（母本 `owned_transcript.rs:449-452` 仅非 Moved 重渲染）。
6. **base64 载荷断言勿截断比对**：捕获流里 OSC 52 序列可能与其他输出交错，先按 `ESC]52;c;` 定界再取到 BEL/ST。

## 5. 预测 → 实测（验收清单）

实施按 tdd → comprehensive-testing → Security Testing Gate → 独立 code-review → verify:full（008 §3-J12 纪律；无独立安全 Skill，以 S46 攻防 e2e＋A7 回归 11/11 承担并如实记录）。实测（2026-10-15）：

- [x] **A1 拖选内容一致性（PTY e2e）**：S40（ASCII 载荷逐字节精确）＋S43（中文＋emoji 载荷逐字节精确，宽字符列换算）；选区不含沟位列（view.area 收窄结构性保证＋S44 沟位互操作）；控制字符剥离（S46 断言无 ESC/BEL）。多行/折行/组合字符：**S47（补验轮）**——真实换行/软折行/中文/emoji/e＋U+0301 载荷逐字节精确；hit_test 破坏对照维持 vendored 单测面（登记）。
- [x] **A2 右键复制全链（PTY e2e）**：S40（历史区：载荷解码＝选区、mock 零请求、无草稿变化）＋S42（无选区右键 no-op＋composer 区选区右键同链路 PlainText）。宿主粘贴＝代理证据（OSC 52 请求正确），实际粘贴另列 A7。
- [x] **A3 反馈与失败如实**：S40 断言「Copy sent to terminal · paste to verify」可见且屏幕无「Copied 」（Unconfirmed＝已发送未确认，不伪报成功）。超限态：**S49（补验轮）**——>100KB 选区右键＝「Copy failed」＋零 OSC 52＋恢复复制（驱动消费失败结果）。Busy 态无确定性触发窗，维持 vendored 单测覆盖（登记）。
- [x] **A4 无副作用**：S40/S41b（mock 计数断言、草稿进提交载荷、退出码 0 干净退出）。
- [x] **A5 交互矩阵**：S44（滚轮保留选区＝快照冻结；沟位互不干扰；resize 往返选区存活）、S42（Esc 清选区；composer 点击清选区）、S41b（Enter＝照常提交恰好一次）、S41（Ctrl+C 一致性）。turn 进行中快照冻结：**S48（补验轮）**——延迟回复到达期间选区不漂移（右键载荷＝原选区）＋视口保持双段证明＋turn 中 Ctrl+C＝取消不复制。
- [x] **A6 安全攻防（A7 扩展）**：S46——OSC 52 伪装/CSI/BEL 混入正文：渲染＋拖选阶段零自发写入（裸流零 OSC 52）；右键后唯一载荷＝净化文本（控制字符剥离，敌意原文不逸出终端字节流，编码即围栏）。
- [ ] **A7 用户实机验收步骤（报告附）**：真实终端拖选中文/emoji→右键→宿主应用粘贴比对；Shift+原生选择补充路径可用性确认。**步骤已附（实施记录 §6），宿主粘贴未实测——保留该项**。实际粘贴失败时按保全轮口径记录「该场景未通过，原因待定位」＋终端类型/连接链路/提示/实际结果，不预判归因（OSC 52 已发送≠宿主已写入）。
- [x] **A8 既有回归**：全量 `cargo check --workspace && cargo test --workspace --no-fail-fast`＝**2741/0/3**（补验轮＝实施轮 2738＋S47/S48/S49）；e2e 63/63；A7 11/11。

## 6. Ctrl+C 定点修复轮（2026-10-15：有选区时取消未生效）

**用户现场**：复验 01–06 通过；无选区 Ctrl+C 可退出；选中文字后 Ctrl+C 无明显提示、也不退出；当时任务是否运行/是否在详情页未确认——两轴均须经验定论，不得预设。契约固定为四条：①空闲（有无选区一致）＝退出；②运行中（有无选区一致）＝取消＋CLI 存活；③Ctrl+C 永不复制、不得被选区/工具焦点/详情页静默消费；④详情页 Esc 一次返回不变。

**定位（真实驱动 PTY）**：驱动两条路由 Ctrl+C 均先行且消费（主对话 `chat_driver.rs` Key 臂首检；详情页槽位 J10 先行），空闲组合全部实测通过（S41 主对话、S67 页内、S68 页内＋选区）——**选区并未接管路由**。缺口在取消执行侧：`turn.rs` 的 `sender()`（阻塞 ureq，agent 总超时 180s）不轮询 `HOST_CANCEL`——工具批间（turn.rs:281）与 exec 执行中（exec.rs）有轮询，纯回复在途时置标志后无可视变化、回复照常到达渲染。s69 RED 屏幕证据：Ctrl+C 后 `Working (2s …)` 计时持续走。与「无提示、不退出」现场症状吻合（运行中组合下用户对照的空闲退出组恰好掩盖差异）。

**修复（最小差异，两处）**：
1. `tui_host.rs` sender 闭包：请求移入辅助线程执行、等待侧 `recv_timeout(100ms)` 轮询取消标志；取消＝停止**本地等待**并返回 Err（文案收尾轮更正为「用户已取消本轮：已停止等待模型回复（迟到结果将被丢弃）」——底层请求不被中止，不得宣称）→Failed 反馈行；迟到结果发往已弃接收端丢弃。工具批间/exec 取消路径（006 N2b）零改动；无新增持久化字段、模型面 JSON/会话语义零改动。
2. `chat_driver.rs` Ctrl+C 运行中分支：`transcript.jump_to_latest()`——S48 冻结视口（选区快照＋hold_position）会把取消反馈行挡在视口外，中断＝清覆盖层（选区/键盘选区/复制态/搜索）＋回跟随态。空闲分支不跳（进程退出，选区无意义）。

**验证**：事前 RED s69（无选区）/s69b（有选区，屏幕证据含选区高亮 `[7m`）/s70（页内两段）；s68 修复前即绿（页×空闲×选组本就正确，钉契约）。修复后全绿＋变异双验证（A：sender 回退→s69/s70 红；B：仅去 jump_to_latest→s69b 红/s69 绿——精确区分两处修复的必要性）。邻域回归 S41/S48/N2b/S66/S67 全绿（s48 语义微调：turn 中 Ctrl+C 现清选区回底，其断言不受影响）。全量 **2766/0/3**（基线 2762＋S68–S70＋S69b）、e2e **85/85**、A7 11/11。

**残余（如实）**：①详情页内取消反馈在主对话——页全屏覆盖，须 Esc 返回后可见（s70 断言返回后可见；页不因取消关闭是 J10 消费语义）；②现场证据边界：已证明＝等待模型响应期间旧实现不能及时响应取消、现有 PTY 空闲选区组合可以退出；**未证明**＝用户现场那次一定处于运行中、宿主终端拦截了 Ctrl+C。现场原因以新版复验为准——若复验仍异常，记录终端类型、选择方式（普通拖选 vs Shift+原生选择）与任务状态再定位；③线程 `recv_timeout` 100ms 粒度＝取消延迟上界约 200ms。**资源边界（重叠取消轮补记）**：单请求存活上限＝agent 总超时 180s；总并发（被弃辅助线程并存数）**无上限**——随重叠取消次数增长、各自独立回收（S72 实测基线＋2 后双双回落）；不得表述为「总体有界/不会积累」。「阻塞 ureq 无从中断」收窄为「当前实现未中止底层 HTTP 请求」（等待侧不关 socket、不中断/不 join——能否中止非论据）。


## 7. 按键单一职责轮（2026-10-15，Esc 审计后实施）

K5 的 Ctrl+C 分工重定（详见 012 §9 与 `docs/records/2026-10-15-esc-audit.md`）：**Ctrl+C 恒退出**（不取消不复制，任何页面/选区/聚焦/任务态）；**Esc＝仅取消**（运行中一次一键一取消、空闲无动作、不清选区——选区清除入口＝点击输入区或其他键路由前的恢复守卫）；右键复制、键盘永不复制、净化与载荷边界全部不变。§6 的取消语义（停止本地等待、迟到结果弃置、不宣称中止底层请求）原样适用于 Esc。受影响用例（S41 语义不变；S42/S48 清选区步骤改点击输入区；S69–S72 取消键改 Esc）按语义改写，逐项登记于实施记录。
