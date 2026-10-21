# 规格：TUI 历史区右侧滚动条

> 状态：**已实现并本地验证（2026-10-15 实施轮＋收尾轮，2026-10-16 §7 帧调度修复轮，待审阅候选）**　分支 `feat/008-codex-tui-port`，未提交。收尾轮修正端点映射（K2：content_length＝最大可滚动顶行+1，非正文总行数；拖动端点量化折叠恢复跟随）＋RowCache 定点核查（无缺口）＋stash 事故核对，见实施记录 §8。§7 轮修复无条件绘制（复用母本 FrameRequester/FrameScheduler 帧调度，空闲/悬停零帧），见实施记录 §9。全量 `cargo check --workspace && cargo test --workspace --no-fail-fast`＝**2747 通过 / 0 失败 / 3 忽略**（2741 基线＋§7 轮 6 新 e2e）；e2e **69/69**；A7 11/11。实施轮事前 RED 4/5（S39 为守护用例）＋变异 RED×3，收尾轮端点 RED×3＋折叠变异 RED，§7 轮事前 RED×3（s50/s51/s52）＋变异 RED×2（S45 自续帧、s55 Draw 通知臂）。实现记录：`docs/records/2026-10-15-011-implementation.md`。
> 需求来源：用户明确要求"历史区右侧增加滚动条，可以拖动浏览历史"。
> 母本：[openai/codex](https://github.com/openai/codex) 固定提交 `97771b1d78cf6366bc3946fe9e4bbca4ebee31d1`（本地 `/tmp/codex-upstream`）。
> 本轮仅调查＋规格，不修改生产代码。

## 1. 目标 & I/O

- **目标**：Owned transcript 模式下，历史超过一屏时历史区右侧显示**滚动条**（轨道＋滑块，反映内容总量与当前可见范围），支持**拖动滑块**与**点击轨道**浏览历史；滚动条与滚轮/分页/跳转共用同一视口状态（`TranscriptView` 的 `Position`/跟随控制），不另建滚动模型。
- **输入**：沟位（历史区最右 1 列）上的鼠标 Down/Drag/Up(Left)。
- **输出**：沟位内渲染 ratatui `Scrollbar`（VerticalRight）；`Position` 变化（既有渲染管线随之刷新）。
- **组件来源（调查结论）**：母本 codex-rs/tui **没有滚动条组件**（固定提交全树 grep `Scrollbar` 零命中——母本交互模型为滚轮/分页/跳转，无滚动条先例）。按任务指令评估已锁定 ratatui 组件：**ratatui facade 0.30.2 已 re-export `ratatui_widgets::scrollbar`（`ratatui-0.30.2/src/widgets.rs:684`；`Scrollbar` scrollbar.rs:83、`ScrollbarOrientation`:106、`ScrollbarState`:145）**——零新增依赖、零版本漂移（Cargo.lock 既有），**新写代码属"本地新增"而非母本移植**，逐文件登记。
- **范围外（明确不做）**：滚动速度/外观配置项；水平滚动条；composer 区滚动条；inline 模式滚动条（原生 scrollback 由终端自管）。

## 2. 边界 & 风险

- [ ] **与母本行为偏离的登记**：滚动条是本地新增 UI（母本无），008"与母本一致"的验收面不适用于沟位本身；其余渲染/交互行为不得因沟位改变（选区、搜索、跟随、分页语义不变，仅宽度让出 1 列）。
- [ ] **vendored 文件改动纪律（最小化）**：新代码落在 `transcript_view/scrollbar.rs`（新文件）＋`chat_driver.rs`（本地模块）；vendored `transcript_view.rs` 仅允许追加 `mod scrollbar;` 声明与必要 `pub(crate)` 访问器（子模块可访问父模块私有字段，勿把字段提 pub）；改动登记入 `src/tui/README.md` 原样/适配清单（008 D5/J9 纪律）。custom_terminal/tui.rs 零改动。
- [ ] **宽度收缩的正确性**：cells 渲染宽度＝历史区宽−1，`TranscriptView.area` 同步收窄（render 传入收窄 rect 即可）——选区 hit-test、搜索高亮、行折行全部基于同一宽度，一致性由既有机制保证；LayoutCache 按宽度键缓存，宽度变化触发一次性重算（正确性无虞，长历史有一次 O(n) 重排成本）。
- [ ] **几何精度**：本地驱动持有**全量** cells（无母本的远端分页未装载态，`has_unloaded_history()` 恒 false），总行数＝逐 cell 布局高度求和（缓存摊销）——滚动条位置是**精确值**而非估计；仍须防 O(n²)（拖动时每帧求和改增量维护或以帧内单次求和为限）。
- [ ] **竞态保持**：沟位切分必须基于渲染闭包内真实帧缓冲尺寸（008 §4 坑 16/竞态修复纪律：`chat_driver.rs:592` 注释先例）——预取尺寸只作提示。
- [ ] **输入歧义**：沟位鼠标事件先于选区消费（沟位不可选文本）；拖动期间 crossterm Drag 事件持续到达（1002/1003 捕获已启用）；沟位外拖入/拖出按"按下时归属"（拖动状态机持有点击来源）。
- [ ] **既有回归**：A3 滚动验收集（S19–S26）不回退；010 选择规格与本规格共享鼠标分发序——实施时按 010 §3 分发序统一扩展（沟位判定插在 transcript 区判定之前）。

## 3. 关键判断（为什么）

- **K1 沟位常驻（自行收敛）**：历史区最右 1 列**恒预留**（内容不足一屏时留空/仅淡轨道），而非溢出时才出现——避免"历史首次超过一屏时全体重折行"的中间跳变与宽度抖动。代价＝编辑/渲染宽度恒 −1 列。备选（溢出才预留、接受一次性重折行）记录不采纳。
- **K2 几何模型**：`ScrollbarState { content_length: 最大可滚动顶行+1（=总行数−视口行数+1）, position: 视口顶行绝对行号, viewport_content_length: 视口行数 }`；总行数＝Σ 各 cell 布局高度（`LayoutCache`/`TextLayout` 既有缓存）；视口顶行＝当前 `Position::{Latest(=底部), Reading(Anchor)}` 锚的绝对行（自底向上累计，与 `start()`/`bottom_start()`（`transcript_view.rs` scroll 实现）同源）。跟随态＝滑块精确贴底（收尾轮修正：ratatui position 语义的可滚动位置范围是 content_length−1，直接传总行数会把范围撑大 (viewport−1) 行致滑块偏短、贴底够不到轨道底、拖动底部死区——实施记录 §8.1）；`follow_control` 的"N 行未读"提示语义不变（不因滚动条出现而清除）。
- **K3 交互映射（与既有视口状态同一套）**：
  - 拖滑块：Down 于滑块上→进入拖动（记录抓取偏移）；Drag→目标绝对行＝按比例换算→`scroll_to_absolute_row`（新助手：自底锚经 `move_rows` 等价行走，越界钳制——复用既有钳制语义，不新写滚动算法）；Up→结束。
  - 点轨道：滑块上方/下方＝PageUp/PageDown 等价（同一 `scroll` 路径），重复点击连续翻页。
  - 滚轮/键盘分页/跳转（Ctrl+Home/End 等）不变，滚动条仅随之刷新（只读跟随，除用户拖动/点轨道外不改 `Position`——**上翻后新回复不抢位置**由既有 hold/follow 机制保证，滚动条不得破坏）。
- **K4 本地缺口与落点**：无任何本地代码可复用（母本无、本地无）——全部新写：① `transcript_view/scrollbar.rs`：几何计算（总行数/顶行）、`scroll_to_absolute_row`、拖动状态；② `chat_driver.rs`：沟位区域推导（`split_screen_areas` 扩展返回沟位 rect，draw 与鼠标分发共用）、沟位鼠标路由、渲染闭包内画 `Scrollbar`；③ vendored `transcript_view.rs`：`mod` 声明＋最小访问器（登记）。
- **K5 待决（默认按推荐收敛）**：轨道点击步长＝一页（视口高度−1）vs 半页。推荐一页（与 PageUp/PageDown 语义一致，可预期）。

## 4. 坑记录

1. **勿用预取尺寸切沟位**（008 坑 16/竞态修复同源）：resize 窗口内预取 screen 与帧缓冲不一致，沟位/内容区错列会把鼠标事件路由到错误列。
2. **总行数每帧重算是性能陷阱**：拖动期间每 Drag 一帧；求和必须走 `LayoutCache`（宽度键缓存）并避免在几何函数内触发布局失效回环。
3. **Latest 锚的顶行计算**：跟随态下 live tail 增长会改变总行数与顶行——几何读取须与渲染同帧（在 render 闭包/同一 draw 内取，不缓存跨帧值）。
4. **选区进行中的拖动**：011 拖动与 010 选区都以 Left Down 起始——沟位判定必须先于选区判定，且沟位 Down 不得开启选区（`begin_selection` 不被调用）。
5. **PTY e2e 断言滑块字符**：ratatui Scrollbar 渲染为轨道/滑块字符单元——屏幕断言取沟位列字符；拖动后断言视口内容变化（非仅 `ScrollbarState` 调用），符合 AGENTS.md"验证真实行为"。

## 5. 预测 → 实测（验收清单）

实施按 tdd → comprehensive-testing → Security Testing Gate → 独立 code-review → verify:full（2026-10-15 实施轮全部执行；安全门禁无独立 Skill，以 A7 回归 11/11＋几何只消费布局元数据（不触内容文本）＋退化尺寸无 panic 单测承担并如实记录）。实测结果：

- [x] **A1 溢出显示**：S35——溢出时轨道（║）＋滑块（█）在沟位列、跟随态滑块**精确贴底**（收尾轮修正 content_length 传参后达成；初版「贴近底部而非精确贴底」系把正文总行数误作 position 范围的误诊，实施记录 §8.1）、长内容滑块＜轨道半长（占比断言）；S39＋单测 short_content——不足一屏沟位留空（K1 常驻）。
- [x] **A2 拖动浏览（PTY e2e）**：S36——Down→Drag 顶（L000 可见＋滑块贴顶）→Drag 中点（早晚条目均不可见，中段可见）→Drag 底（L119 可见）→Up（出沟位释放）；拖到底恢复跟随（后续 /sessions 输出自动贴底）；零模型请求。单测 drag_mapping_is_monotonic_and_clamped 钉住单调与端点。
- [x] **A3 轨道点击**：S37——上方轨道（第 2 行；第 1 行是 prompt header 属正文区外，按下无操作）单击＝整页（S37-L09 锚点＋最新条目移出）；连续 4 页达 L0xx 区；下方轨道连点 6 页容差内回最新。
- [x] **A4 同一视口状态**：S35——PageUp 后滑块同步上移、12 次滚轮续滚滑块继续上移（同一 Position 状态，无第二套偏移）；S36b（审计补强）——拖动上翻后 /sessions 新输出不抢位置（保持断言＋Esc 回底后输出可见双段证明，S22 同法）；S35 Esc/S36 拖底恢复跟随。（2026-10-15 单一职责轮：Esc-to-latest 移除——Esc 专取消、空闲无动作；S35/S36b 的回底断言改 **Ctrl+End**（JumpTarget::Latest 既有键）；Esc 空闲改断言「视口不变」，见 012 §9）
- [x] **A5 resize 与草稿**：S38——拖动中途 resize（200×50→120×30）无 panic、中文/emoji 草稿存活、释放后新几何重抓取拖到顶有效；S39 极窄 40×12 resize 存活；S23/S26 既有 resize 集全绿（现带沟位列运行）。resize 后显式不越界断言由 S23 短历史限位＋S38 重抓取间接覆盖（登记）。
- [x] **A6 与选择互斥**：结构性满足——选择未实施（010 未启用，无 begin_selection 调用路径），沟位 Down 命中专用分支不触视图；「选区文本不含沟位列」待 010 落地验证（登记为 010 验收项）。
- [x] **A7 安全**：几何只消费 row_count()（布局元数据），不触内容文本；A7 回归 11/11 全绿；单测 degenerate_sizes 覆盖 (1,10)/(2,1)/(1,1)/(0,5)/(5,0) 退化尺寸渲染/按下/拖动无 panic。
- [x] **A8 既有回归**：S19–S26 全绿（52/52 全套含 011 新增 6 例）；全量 `cargo check --workspace && cargo test --workspace --no-fail-fast`＝**2726/0/3**（2729 函数＝012 终验 2715＋审计激活 1＋本轮 13：scrollbar 单测 7＋e2e 6）。


## 6. 实测反馈诊断（2026-10-16 诊断轮，无生产改动）

用户反馈「滚动条可用，但不跟手；速度似固定、不随操作幅度增加」。PTY/单元探针证据：

| # | 事实 | 证据 |
|---|---|---|
| F1 | 步长恒 1.0 行/事件（本地＝母本默认；母本另有 `local_settings.tui.mouse_scroll_speed` 用户设置项，移植未接线） | `transcript_view.rs:132` / 母本 `resume_picker.rs:457` |
| F2 | 视图层事件保真完美：单元级 30 事件＝30 行、再 10＝10（无丢失/合并） | 临时单测（已清理） |
| F3 | 端到端单事件→内容帧 3-12ms（debug 构建），帧 ~800B（diffed） | PTY 探针 10 样本 |
| F4 | **驱动每轮循环末无条件 draw**：100ms tick 持续空帧滴流（~50B/帧，hex 定性＝`?2026l/h` 同步对＋光标定位；空闲也重绘）；Moved 悬停事件同走无条件 draw（终端鼠标上报率可达 60-125Hz → 慢链路洪泛）。母本仅对**非 Moved** 鼠标事件渲染＋帧率限制器（`MIN_FRAME_INTERVAL`=8.33ms）调度 Draw | `chat_driver.rs` 循环尾 draw／母本 `owned_transcript.rs:449`、`frame_rate_limiter.rs:13` |
| F5 | 终端滚轮协议不携带手势幅度（crossterm ScrollUp/Down 为单位槽粒）——「速度随幅度」只能经事件频率（现状每槽 1 行）或倍率设置表达 | crossterm 事件结构 |

未证实假设：用户终端/触控板的实际事件发送率与合并策略；慢链路（SSH/WSL）下的帧积压体感（本地 PTY 无链路时延，无法复现）。原生手感属现场验证边界。

**最小修复方案（候选，待批准后实施）**：
- **A（主）**：驱动 draw 条件化（脏标记）——Moved 事件不重绘；tick 仅在有待显变化（流式活动/动画 tick/反馈过期/选区边缘自动滚动）时重绘。对齐母本「非 Moved 才渲染」；消除滴流与悬停洪泛，慢链路收益最大。不引入加速曲线/自动倍率。
- **B（复用）**：接通母本 `mouse_scroll_speed` 设置通路（local_settings 已 vendored）——用户可调步长，属复用母本既有配置而非新增配置项。
- 滑块拖动已是坐标映射（011 端点修正＋单调/折叠回归在位），与滚轮步进分路径，不混用 ✓。

**验收场景（修复轮）**：①PTY 注入 N=10 滚轮事件→内容帧数恰 10、位移恰 10 行；②空闲 ≥2s→新增输出字节≈0（无空帧滴流）；③注入 50 个 Moved 悬停事件→零内容帧；④mouse_scroll_speed=3.0（如实施 B）→ N 事件位移 3N 行；⑤副作用：选区/滚动条/复制行为不回退（011/010 既有 e2e 全绿）。

## 7. 无条件绘制修复（2026-10-16 实施轮，方案 A）

### 7.1 目标 & I/O

- **目标**：驱动循环尾改为**按状态变化绘制**——复用母本既有 `FrameRequester`/`FrameScheduler`（帧请求合并＋120fps 限频）与 `draw_tx` Draw 通知，取消循环尾无条件 draw；消除空闲滴流与 Moved 悬停洪泛（§6 F4），输入与定时任务完整保留。
- **输入**：驱动既有事件多路（crossterm/tick/app）＋新增 draw 通知臂。
- **输出**：`tui.draw()` 仅在（a）驱动侧状态变化置脏、（b）部件经 FrameRequester 请求帧时执行；空闲/纯悬停零输出字节。
- **步长不变**：滚轮恒 1.0 行/事件（§6 F1）；不新增倍率配置、加速曲线或第二套滚动状态。方案 B（`mouse_scroll_speed` 接线）本轮不做；滚动速度诉求与慢链路体感仍属现场验证项，**本轮不声称解决全部滚动体验问题**。

### 7.2 边界 & 风险

- [ ] **定时任务不得停**：Tick 保留全部推进——剪贴板 poll 分派、turn 回排空、粘贴突发冲刷（`flush_paste_burst_if_due` 返回值入脏标记）、排队输入重放；选区边缘自动滚动与复制反馈到期改为**绘制时自调度**（`tick_selection` 返回 true→`schedule_frame_in(FRAME_TICK)` 维持链；`render_composer_gap` 返回到期延迟→`schedule_frame_in(delay)`，本地 draw 原丢弃该返回值，接线即可）。
- [ ] **部件侧已自请求**（核对结论）：composer 键路径 `request_redraw()`/冲刷定时、`handle_paste`、`set_task_running`（状态行 render 内 32ms 自调度——turn 期间 ExecCell spinner 动画经此保持）、剪贴板 worker（OSC 52 写完成）、screen_size 重试、sparkle（模型门控，本地会话不激活）。驱动侧只补 `draw_tx` 订阅臂。
- [ ] **Moved 零状态变化已逐层核实**：`prepare_composer_mouse` 仅 Down 动作、`handle_mouse_with_mode` Moved 落 `_ => None`、textarea `handle_mouse` 显式非 Down/Drag/Up 即 false——与母本「Moved 不渲染」（owned_transcript.rs:449）对齐；Moved 事件处理本身保留。
- [ ] **粘贴突发不逐键绘制**：composer 键路径返回 needs_redraw=false（突发缓冲字符）时无请求——即母本「honor paste batching」语义，由冲刷定时/Touch 冲刷触发显示；驱动不为 composer 路径强行置脏。
- [ ] **vendored 文件纪律**：`tui.rs` 仅追加 draw 通知订阅访问器（登记）；`chat_driver.rs` 为本地模块自由改动；不新建绘制线程/调度框架。
- [ ] **末帧保证**：最后一次状态变化必须显示——驱动侧置脏路径在循环尾立即绘制（母本对非 Moved 鼠标/键同样即时渲染，不经限频器）。
- [ ] **既有契约不回退**：真实帧缓冲尺寸、滚动条端点、选区快照、Ctrl+C 优先级、终端恢复（A3/S19–S26/S35–S49 全绿为门槛）。

### 7.3 关键判断（为什么）

- **K-A 脏标记＋通知双入口，而非全量走通知**：母本即时渲染用户输入（非 Moved 鼠标、非突发键）、通知渲染动画（限频 8.33ms 合并）。本地对齐：驱动侧分支（cells/滚动/选区/沟位/resize/命令结果）置脏后循环尾立即绘制；部件动画/冲刷/剪贴板经 FrameScheduler 合并限频后唤醒。不引入第二套限频。
- **K-B 计帧手段**：`tui.draw()` 每次经 crossterm `SynchronizedUpdate` 包裹（`ESC[?2026h … ESC[?2026l`）——PTY 捕获输出中计数 `ESC[?2026l` 即应用帧边界，空帧不漏计（不能只比对屏幕内容）。
- **K-C 自动滚动节奏保持本地现状**：母本 Draw 事件内 `tick_selection`+`schedule_frame_in(TARGET_FRAME_INTERVAL)`（≈120 行/s）；本地维持 `FRAME_TICK`（100ms，10 行/s）节奏链，避免改变既有边缘自动滚动速率引入回归。

### 7.4 坑记录

1. **`set_pending_input_preview` 不自请求**：排队预览变更只发生在驱动置脏路径（键 Queued 分支/Tick 重放）——须显式置脏，否则预览滞留旧屏。
2. **选区键守卫的隐性可见变化**：有选区时按键先 `end_selection`（视觉变化）——该键即便落 composer 无重绘请求，也须因守卫触发置脏。
3. **Ctrl+C 运行中取消不置脏也能活**：本地取消无可视变化（终态经 turn 回排事件驱动）；按任意键路径统一置脏保持与现状一致即可。
4. **broadcast recv Lagged/Closed**：唤醒即视为一次绘制请求（通知已发生），不区分错误。

### 7.5 预测 → 实测（验收清单）

- [x] **B1 空闲零帧**：S50——turn 完成后静置 1.2s＝**0 帧/0 字节**（旧驱动实测 **13 帧**/1.2s 滴流，事前 RED 证据）；空闲后键入仍即时上屏。
- [x] **B2 悬停零帧**：S51——50 个 SGR Moved（35 编码、8ms 间隔）＝**0 帧/0 字节**（旧驱动实测 **59 帧**，事前 RED）；随后滚轮仍产生绘制（事件处理保留）。
- [x] **B3 位移守恒**：S52——确定性溢出历史（S52-L000..L119），钉住条目屏幕行号：慢速 10 个滚轮（逐个 30ms）恰 **+10 行**；单次写入 10 事件再 **+10 行**（最终位置与慢速一致）；不要求 10 事件 10 帧（实测 10 帧逐事件即时渲染＝母本直接渲染路径行为）。
- [x] **B4 合并无积压**：S52——慢速与突发后各 500ms 排空窗口新增输出＝**0 字节**（无持续追赶；旧驱动同窗口实测 258 字节滴流，事前 RED）；Draw 通知路径合并由 vendored FrameScheduler 既有单测＋S55 运行期帧流覆盖。
- [x] **B5 定时功能免按键显示**：S53（粘贴冲刷草稿免按键上屏，守护）、S54（复制反馈 5s 到期免按键消失——draw 捕获 `render_composer_gap` 到期延迟→`schedule_frame_in` 接线）、S55（turn 运行期动画帧持续流动 ≥5 帧/1s，标称 ≈31fps；Draw 通知臂变异断裂＝恰 0 帧，变异 RED 证实）＋既有用例维持：流式回复（S20 等）、边缘自动滚动（S45——变异断开自续帧即红，变异 RED 证实）、剪贴板完成（S40–S44/S49）。
- [x] **B6 不退化**：S19–S26、S35–S49 全绿（e2e **69/69**＝既有 63＋本轮 6）；A7 **11/11**；全量 `cargo check --workspace` 0 错误＋`cargo test --workspace --no-fail-fast`＝**2747/0/3**（基线 2741＋本轮 6）。
- 独立 code-review（Standards＋Spec 双轴）：Spec 轴无缺失无越界；Standards 轴 3 项修复（`src/tui/README.md` 复用登记、Key 分支 else 块缩进重排、s52 `format!` 无占位符 nit）＋s55 下界 10→5 负载加固；残留提醒（tick_selection 每次 draw 推进＝与母本 Draw-tick 模型同性质，稳态节奏守 K-C）如实登记不修改。
- 前后对照（PTY 实测，`ESC[?2026l` 帧边界计数）：空闲 1.2s＝13 帧→**0 帧**；50 Moved＝59 帧→**0 帧**；滚轮 10 事件（慢速/突发）修复后均 10 帧≈7.0KB（逐事件即时渲染，无积压）。