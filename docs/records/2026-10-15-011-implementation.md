# 011 历史区右侧滚动条——实现记录（2026-10-15 实施轮＋收尾轮）

规格：`docs/specs/011-tui-transcript-scrollbar.md`。分支 `feat/008-codex-tui-port` @ b0e8d48，未提交。
收尾轮（§8）：端点映射修正（RED×3→GREEN＋e2e 二次 RED 折叠回归＋变异验证）、RowCache 定点核查（无缺口）、stash 事故对象库基线核对、fork 竞态结论收敛；终验 **2730/0/3**（scrollbar 单测 11/11、e2e 52/52、A7 11/11）。

## 1. 复用入口与必要适配

**复用（零新增依赖、零第二滚动模型）**：
- ratatui facade 0.30.2 既有 re-export `Scrollbar`/`ScrollbarState`（widgets.rs:684；ratatui-widgets 0.3.2 scrollbar.rs）——锁定版本，无 Cargo 变更。
- `TranscriptView` 既有滚动语义：`scroll()`（钳制＋越过底部恢复跟随）、`start()`/`bottom_start()`（视口锚）、`LayoutCache`、`entry_key` 指针身份惯例（transcript_view.rs:61-63 同款 `Arc::as_ptr`）。
- 鼠标捕获 1000/1002/1003 已启用（`tui/alternate_screen.rs` EnablePointerCapture）——SGR 拖动事件无需新增终端配置。

**必要适配（落点）**：
- **新写 `src/tui/src/transcript_view/scrollbar.rs`**（本地新增，母本无滚动条组件）：`RowCache`（行数持久缓存）＋`scrollbar_geometry`（总行数/顶行/视口）＋`thumb_layout`（拇指区间，与 ratatui `part_lengths` 同公式）＋`render_gutter`/`gutter_press`/`drag_to`/`scroll_to_absolute_row`＋7 单测。
- **vendored `transcript_view.rs` 仅 1 行**：`pub(crate) mod scrollbar;`（§2 最小化纪律；子模块经 Rust 权限规则访问父模块私有项，未提任何字段可见性）。
- **本地 `chat_driver.rs`**（搜 `011` 注释）：draw 闭包内 `split_transcript_gutter` 沟位切分（真实帧缓冲尺寸，坑 1/坑 16 纪律）＋`render_gutter`（render 之后同帧取几何，坑 3）；鼠标路由（沟位 Down/Drag/Up 状态机——按下归属、指针越出沟位仍归拖动、Up 收口；与 draw 共用同一沟位推导）；RowCache 失效钩子（Ctrl+T、视图消费键）。
- **测试设施**：`common/pty.rs` PTY_COLS/PTY_ROWS 提 `pub(crate)`（单一事实源，e2e 内字面量同步改常量）；e2e 新增 S35–S39＋S36b 与 `gutter_marks_at`/`sgr_down|drag|up` 辅助。
- **登记**：`src/tui/README.md` 011 追加条目。

## 2. 关键设计决策

- **滚动不另写算法**：`scroll_to_absolute_row` 只算 delta 后调既有 `scroll()`——钳制/跟随/J10 语义全复用；轨道点击步长＝视口−1，与 PageUp handler（input.rs:436-438）逐字一致。
- **拇指三处同一公式**：渲染（ScrollbarState）、命中（`thumb_layout`）、拖动反演共用 ratatui `part_lengths` 整数舍入（无箭头符号，轨道长＝沟位全高）；单测 `thumb_layout_matches_rendered_scrollbar` 以同 buffer 渲染比对钉住。**收尾轮修正（§8）**：position 范围＝最大可滚动顶行（总行数−视口），非正文总行数。
- **拖动映射**：抓取偏移（滑块内）＋指针反推滑块区间＋渲染公式逆映射——端点精确（指针到轨道顶＝顶行 0、拇指贴轨道最底位＝最大可滚动顶行，量化折叠恢复跟随）；正文区行基准（prompt header 预留行不属轨道，区外按下无操作）。
- ~~跟随态滑块贴近底部而非精确贴底＝ratatui position 语义原生缺口~~ **收尾轮订正（§8）**：该「缺口」实为 content_length 传参误用（传了正文总行数），修正后跟随态滑块精确贴底；S35 断言与注释已同步。

## 3. 性能（坑 2 实测）

`LayoutCache` 仅 64 条近期条目——全量 Σ 会把可见窗口挤出并每帧重折行。`RowCache`（Weak 指针身份＋宽度键＋显式失效）后，2000 cell／3999 行极端会话实测（perf 探针，已移除）：

| 阶段 | release | debug |
|---|---|---|
| 首帧渲染＋首计量 | 12.4ms＋5.0ms | 40.7ms＋42.5ms |
| 稳定帧几何（缓存命中） | **70µs** | **718µs** |

缓存命中路径完全不触 `layout()`（`scrollbar.rs` rows() 命中直返计数）；live 尾部逐帧现算（1 条）；失效面＝宽度变化（内置）＋内容变更（指针身份）＋呈现态（Ctrl+T/展开折叠钩子）＋死键粗粒度收缩（len>4×cells，/resume 整体换 vec 场景）。

## 4. TDD 证据

- **事前 RED（实现前运行，日志 `/tmp/011-RED-final.log`）**：S35/S36/S37/S38 四条真实 PTY 行为用例 4/4 失败（无滑块/无拖动/无轨道翻页）；S39 为守护用例（实现前后均绿——短历史沟位留空）。
- **变异 RED×3**（改坏→对应用例红→回退复绿 3/3）：渲染禁用→S35 红；拖动映射 no-op→S36 红；轨道不翻页→S37 红。
- **comprehensive-testing 审计补强**：S36b（拖动版「新输出不抢位置」——S22 只盖 PageUp 路径；双段证明：保持断言＋Esc 回底后输出可见）＋单测 degenerate_sizes（退化尺寸无 panic）＋S35 增强（滑块占比断言、滚轮↔滑块同步）＋S36 增强（中点比例断言）。
- e2e 调试过程中的测试设计修正（如实登记）：S37 翻页锚点前缀误匹配（L10x 底部即有）、第 1 行属 prompt header（正文区外）、下方轨道点击误击 composer 区、S38 vt 解析器保持 200 宽（resize 后按列断言不可靠→任意列扫描＋行为断言）、S39 多余 Esc 触发 composer 编辑上一条模式与不读睡眠致终端查询停等（S23/S25 同法轮询修正）。

## 5. 验证

- `cargo check --workspace` 通过。
- 实施轮 `cargo test --workspace --no-fail-fast`＝**2726 通过 / 0 失败 / 3 忽略**；**收尾轮终验（§8 后最终候选）＝2730 通过 / 0 失败 / 3 忽略**（＝实施轮 2726＋收尾轮 4 新单测，最后一轮生产代码变更后复跑）。e2e 52/52（S19–S26 既有滚动集全绿；收尾轮 S19–S26＋S35–S39＋S36b 14/14 复跑）；scrollbar 单测 11/11（实施轮 7＋收尾轮 4）；A7 回归 11/11。
- 已知抖动（既有，非本轮路径）：`session_perf` 50ms 预算全量并行偶发超限（012 记录 §7 已登记）；本轮两轮终跑均通过。

## 6. 独立双轴审查与处置

Standards 轴：无 P1；P2×3 已修（mod.rs 注释错挂；PTY 尺寸双事实源→常量化；README 登记）；P3 清理已做（死 derive、可见性收窄、Default derive、裁剪几何简化、重复断言删除；`rows()` 内快照重解析与 layout() 同款风格保留）。Spec 轴：无 P1/P2；P3×4 处置——登记缺失（本记录＋README 已补）；K4 字面偏离（鼠标分发改共用 `split_transcript_gutter` 推导）；**RowCache 失效缺口登记**：搜索激活时拖到底经 `jump_to_latest→cancel_search` 行数变化无失效钩子——当前构型搜索键不可达（010 未实施），登记为 010 落地时的接线项；验收子项缺口（A1 占比/A4 滚轮同步/A2 中点已补强；A5 resize 显式不越界由 S23/S38 间接覆盖，登记）。

## 7. 遗留与后续

- 010（选择/复制）未实施；沟位判定已插在 transcript 区判定之前（§2 分发序），010 落地时按其 §3 衔接（含 A6「选区不含沟位列」与上述 RowCache 失效钩子）。
- Windows 真机（A6/008）仍待验证；本轮滚动条为纯 ANSI 字符渲染，无平台特定路径。
- 拖动映射在 live 尾部快速增长期间的行程精度为逐事件近似（端点精确）；未观察到可感知漂移。

## 8. 收尾轮（2026-10-15 第二轮）：端点映射修正＋RowCache 定点核查＋stash 事故核对

### 8.1 滑块端点映射修正（生产代码变更）

**根因**：`render_gutter` 把**正文总行数**直接当成组件 `ScrollbarState::content_length`（即 position 范围 0..=总行数−1），而 `position` 传的是视口顶行——其真实范围是 0..=总行数−视口（最大可滚动顶行）。ratatui-widgets 0.3.2 `part_lengths` 按-content_length 计算比例（thumb = round(viewport×track / (max_position+viewport))，源码 `~/.cargo/registry/.../ratatui-widgets-0.3.2/src/scrollbar.rs`），position 范围被撑大 (viewport−1) 行后：滑块比例偏短（100/20 例：3 行而非 4 行）、跟随态（顶行=80）滑块悬在轨道 13..16 够不到底（轨道 20）、拖动逆映射底部出现死区。命中与渲染共用同一（错误）公式，故交互自洽但整体几何压缩——上一轮将其误诊为「ratatui position 语义原生缺口」（§2 已订正）。

**修正（仅适配参数，`scrollbar.rs` 三处统一）**：`ScrollbarGeometry::max_top_row()`（=总行数−视口）为 position 上限；`thumb_layout` 的 `max_position`、`render_gutter` 的 `ScrollbarState::new(max_top_row()+1)`、`drag_to` 的逆映射分母全部改用同一值。不修改依赖、不另写滚动模型；`thumb_layout_matches_rendered_scrollbar`（同 buffer 比对）继续钉住渲染＝命中＝拖动同一几何。

**事前 RED（修正前运行，非 /tmp 依赖，可直接复跑）**：
```
cargo test -p tchat-tui --lib scrollbar
# 3 条新增端点用例失败：
# thumb_endpoints_touch_track_ends_at_extremes：顶行 0 时 (start,len)=(0,3)，期望 (0,4)（长度不反映可见占比）
# rendered_thumb_reaches_both_track_ends：真实 Buffer 贴顶滑块 [0,1,2]，期望 [0..4)
# drag_endpoints_exact_and_middle_proportional：中点映射失真（指针中点→顶行 48 而非 50/比例值）
```
夹具＝`cells_100_rows()`（cell0 单行 1 行＋cell1 两行 3 行＋48 单行 cell 各 2 行＝精确 100 行，视口 20）。

**二次 RED（端点折叠回归，真实 e2e 发现）**：首版修正后 e2e `s36` 失败（`wait_screen("seedscroll")` 超时）——旧公式的超大 max_position 恰好造成过冲、经 `scroll()` 越底恢复跟随；精确范围内非整除几何（239 行/视口 20）逆映射原始值 215 < 最大顶行 219，拇指视觉贴底但内容差 2 行、跟随未恢复。修正：`drag_to` 中拇指贴到轨道最底位（量化后多个顶行映射同一拇指位）直接折叠到 `max_top_row`。单元回归 `drag_to_bottom_folds_quantization_to_max_top_and_follows` 钉住该机制；变异验证（去掉折叠改回原始逆映射）该用例红、恢复后绿。

**修正确认**：scrollbar 单测 11/11（新增 4：三端点例＋折叠回归）；e2e S19–S26＋S35–S39＋S36b 14/14；S35 断言「滑块位于轨道下部」在精确贴底后更强（下方轨道 0 行）。

### 8.2 RowCache 失效定点核查（无缺口，证据引用）

同 cell 身份下内容/高度变化的全部路径核查（结论：**无实际缺口，无需修复**）：

| 面 | 核查结论 | 证据 |
|---|---|---|
| 活动内容（流式） | 每 delta `emit()` 新建 cell（controller.rs:722 `Box::new`）→ 新 Arc 身份→缓存未命中现算；live 尾部在缓存外逐帧现算（scrollbar_geometry 直调 layout）；turn 事件（ToolBegin/End/Delta/Final/Info）全部 push/remove/splice 新 Arc | `streaming/controller.rs`、`chat_driver.rs:198-265` |
| 同身份原地变更 | 全库无 transcript cell 的 `Arc::get_mut`/`make_mut`（命中仅为 keymap/markdown_copy，非 cell）；cell 内唯一可变态 MarkdownRenderCache 的输出是其 key 的纯函数，key 各维度（主题修订/调色板）生产会话内恒定（`set_syntax_theme` 生产零调用，仅测试），宽度已入 RowCache 键 | `history_cell/markdown_render_cache.rs`、`render/highlight.rs:271` |
| 详情展开/折叠 | 键盘路径（input.rs:209 `handle_disclosure_key`）经 `handle_key` 返回 Some→`chat_driver.rs:383` 清缓存钩子；鼠标点击展开（input.rs:472 `pointer_down`）当前驱动不可达（内容区点击不路由，仅滚轮） | `transcript_view/input.rs`、`chat_driver.rs:377-384` |
| 呈现切换 | Ctrl+T→`chat_driver.rs:371` 清缓存钩子；`set_presentation` 其余调用方（pager overlay/resume preview/测试）作用于独立 TranscriptView 实例，不触主视图 RowCache | `pager_overlay/transcript.rs:50`（`new()` 自建 view） |
| 宽度变化 | RowCache 内置宽度键（宽度变→全清） | `scrollbar.rs` `rows()` |
| 死键/地址复用 | Weak 升级校验＋粗粒度收缩（len>max(4×cells,64)） | `scrollbar.rs` `rows()` |
| 搜索展开 | 不可达（010 未实施）——维持 010 接线项登记（§6 既有处置） | 012 记录同结论 |

性能成果保持：本轮未触 RowCache 本体，稳定帧缓存命中路径不变（§3 的 70µs 结论不受影响）。

### 8.3 stash 事故恢复核对（对象库基线，非「测试通过」证明）

事故：本轮会话曾发生 stash 引发索引冲突后恢复。基线＝从对象库找回的 stash 双提交（`git fsck --unreachable`）：`c7272f3`（工作区快照，"On feat/008-codex-tui-port: audit-session-fix"，2026-10-15 14:04:54）＋ `d337e67`（索引快照）；2 父结构＝**未含未跟踪文件**（stash 机制面不触及未跟踪）。核对结果：

- **索引**：事故前索引＝纯 009 迁移暂存（`git diff b0e8d48 d337e67`＝118 文件、0 内容变更）；当前索引＝同 118 项重命名逐项一致＋session.rs 以**事故前工作区内容**入暂存（`git diff c7272f3:…session.rs :…session.rs`＝0 行差异）。内容零丢失；唯一边界变化＝session.rs 的未暂存修改变为已暂存（内容不变）。
- **工作区（tracked）**：`git diff c7272f3` 无删除、无类型变更（`--diff-filter=D/T` 空）；19 文件差异全部归因于快照后有记录的工作——session.rs +11＝PROBE_FORK_GUARD（012 §7）、session_perf/session_cli＝008-ADAPT（TERM=dumb 行式退化＋ToolResult 模式）、e2e_acceptance +2460＝S19–S39（快照时 e2e 仍为旧 s1–s6 集，008-a3/012/011 e2e 均为快照后）、Cargo.*／AGENTS.md／README／config／src/app/src/*＝008/009/003-006/012 有记录变更。
- **未跟踪**：无事故前基线（2 父 stash 不含未跟踪），**不能从 git 证据排除未跟踪文件损失**；间接确认＝008–012 规格/记录/AGENTS.md 引用的全部交付物（src/tui 树、src/shims、src/vendor、specs、records）均在场且全量测试通过。此限制如实登记，不声称完全核验。
- 未重演 stash、未用 reset 覆盖；两个 stash 提交保持不可达留存（未 gc），可继续复核。

### 8.4 fork 竞态结论收敛

012 记录 §7-2 与 `session.rs` 探针闸注释中「生产面无对等问题」改为已验证范围表述：**测试互扰已复现并修复；生产侧对等发生未证实**（原分析仅为机理不支持，非无对等证明），本轮不扩展生产修复范围（两处措辞已同步修订；008 交接档 §15-6 同步）。

### 8.5 独立双轴审查（收尾轮）与处置

code-review 双子代理（Standards/Spec）独立复核本轮映射与缓存：

- **Standards 轴**：无硬违规；判断性意见 3 条已处置——①`thumb_layout` 死防零分支（前置保证 total>viewport≥1，分母恒非零）删除；②`drag_to` `.max(1)` 死防御删除；③折叠回归测试硬编码 20 字面量收敛为 `geo.viewport_rows`。`plain2` 命名意见不改（调用处注释达意）。
- **Spec 轴**：无 P1/P2。数学自验算与 ratatui-widgets 0.3.2 `part_lengths` 逐式一致（100/20 例 (0,4)/16..20/贴底）；折叠语义正确；RowCache「无缺口」结论的代码证据独立复核成立（无 cell 原地变更、钩子覆盖、鼠标展开不可达、独立视图不触主缓存）；既有修复（header 排除/真实帧缓冲/越出沟位归属）确认保留。登记项：S35 e2e 层仍为相对断言（`above > below*3`），精确端点由单测逐值承担（分层可接受，与本记录 §8.1 表述一致）。

### 8.6 终验

最后一轮生产代码变更（§8.5 清理）后：`cargo check --workspace` 通过；`cargo test --workspace --no-fail-fast`＝**2730 通过 / 0 失败 / 3 忽略**（退出码 0）；scrollbar 单测 11/11；e2e 滚动集 14/14 复跑；A7 11/11。

## 9. 帧调度修复轮（2026-10-16）：无条件绘制→按状态变化绘制（规格 §7）

### 9.1 事前 RED（旧驱动，b0e8d48＋§8 候选工作区）

新增 e2e S50–S53（`ESC[?2026l` 同步更新收尾标记＝应用帧边界计数，空帧不漏计）：

| 用例 | 旧驱动实测（RED） |
|---|---|
| S50 空闲 1.2s（turn 完成后静置） | **13 帧**（100ms tick 无条件 draw 滴流，≈50B/帧） |
| S51 50 个 SGR Moved（35 编码、8ms 间隔） | **59 帧**（50 事件＋注入窗口内 tick） |
| S52 位移断言通过、排空 500ms 窗口 | **258 字节**持续滴流（位移守恒在旧驱动本就成立＝F2，滴流为 RED） |
| S53 粘贴冲刷免按键 | 通过（守护用例，防修复引入滞留） |

命令：`cargo test --test e2e_acceptance s5`（修复前运行，3 FAILED 1 passed）。

### 9.2 实现（最小差异）

- `chat_driver.rs`（本地模块）：①select 增 `draw_rx.recv()` 臂→`Ev::DrawRequested` 置脏（Lagged/Closed 亦唤醒，坑 4）；②循环尾无条件 draw→`if needs_draw { draw(); }`；③置脏规则——Key：Ctrl+C/选区守卫（坑 2）/owned_by_view/Submitted·Command·Queued（坑 1：`set_pending_input_preview` 不自请求）置脏，composer 其余结果不自置脏（其 `request_redraw`/冲刷定时自请求，突发缓冲遵循母本「honor paste batching」不逐键上屏）；Mouse：非 Moved 置脏（母本 owned_transcript.rs:449 对齐）、Moved 不置脏（全链路无状态变化：`prepare_mouse` 仅 Down、视图 `_ => None`、textarea 显式忽略）；Paste/Resize/App 保守置脏；Tick：剪贴板完成/turn 事件/冲刷产出/排队重放逐项 `changed` 追踪；④Key 分支 3 处 `continue` 移除（continue 会跳过尾绘制致脏标记滞留到下一事件，违反末帧保证）改 if/else 落穿；⑤`draw()` 内 `tick_selection` 返回 true→`schedule_frame_in(FRAME_TICK)` 自续帧（K-C：维持本地 100ms 节奏＝既有边缘自动滚动速率）＋`render_composer_gap` 返回的到期延迟穿出闭包（`gap_expiry_ref`）→`schedule_frame_in(delay)`（本地原丢弃该返回值）。
- `tui.rs`（vendored，登记）：`pub(crate) fn subscribe_draw_notifications()` 4 行——驱动订阅 `draw_tx`；母本部件侧（composer 键/粘贴/状态行 32ms 动画/剪贴板 worker/screen_size 重试）本就全部经 FrameRequester 自请求，唯一缺的接线即此订阅；FrameScheduler 合并＋120fps 限频零改动复用。
- 不新增：倍率配置、加速曲线、第二套滚动状态、绘制线程/调度框架（§7.1 边界）。步长恒 1.0 行/事件不变。

### 9.3 验证

- 修复后：S50/S51＝**0 帧 0 字节**；S52 慢速/突发各恰 +10 行、排空窗口 0 字节；帧对照——空闲 13→0、Moved50 59→0、滚轮 10 事件 10 帧≈7.0KB（逐事件即时渲染＝母本直接渲染路径，无积压）。
- comprehensive-testing 补强：S54（反馈 5s 到期免按键消失）、S55（turn 运行期动画帧 ≥5 帧/1s，标称 ≈31fps）。
- 变异 RED×2（已还原）：断开自续帧（`if false && autoscroll_advanced`）→ **S45 红**；断开 Draw 通知臂（`Ev::DrawRequested => {}`）→ **S55 红（实测 0 帧）**。S54 在断臂下仍绿＝迟到剪贴板读超时完成经 Tick 置脏兜底（合同仍受保护；该用例不单独隔离到期帧路径，如实登记）。
- 独立双轴审查：Spec 轴无缺失无越界（§7.1–7.5 逐项核对置脏落位与既有测试映射）；Standards 轴 3 项已修（README 复用登记、Key 分支 else 块缩进重排、s52 `format!` nit）＋s55 下界 10→5 负载加固；残留提醒：`tick_selection` 每次 draw 推进＝与母本 Draw-tick 模型同性质（叠加事件可瞬时加速，稳态守 K-C），登记不改。
- 终验（审查修复后复跑）：`cargo check --workspace` 0 错误；`cargo test --workspace --no-fail-fast`＝**2747 通过 / 0 失败 / 3 忽略**（基线 2741＋本轮 6）；e2e **69/69**；A7 **11/11**。临时探针/变异全部清除（grep 零残留）。

### 9.4 剩余现场验证项（不因本轮关闭）

- 滚轮「速度固定 1.0 行/事件」属协议粒度（F5：终端滚轮不携带手势幅度）——速度诉求与慢链路（SSH/WSL）体感仍待真实终端复验；方案 B（母本 `mouse_scroll_speed` 设置接线）未实施，待用户决策。
- 用户可重复复验步骤：`cargo run` 后①静置任意时长观察终端无持续输出（旧版每 100ms 有同步对闪动）；②鼠标在历史区悬停滑动（不按键不滚轮）无重绘；③滚轮滚动位移与事件数一致（10 次＝10 行）、突发快速滚动无「追赶感」；④turn 运行中 spinner 动画流畅；⑤右键复制后反馈约 5s 自动消失。
