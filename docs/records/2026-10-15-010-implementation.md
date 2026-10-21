# 010 备用屏文字选择与右键复制——实现记录（2026-10-15）

规格：`docs/specs/010-tui-selection-copy.md`（§3-K5 待决已按 /goal 指令落实收敛）。分支 `feat/008-codex-tui-port` @ b0e8d48，未提交。

## 1. K5 决定（先行落实）

**左键拖选历史文字，右键复制非空选区；历史选区不得接管 Ctrl+C**——无论有无选区，Ctrl+C 一律按现行优先级（turn 运行中＝取消任务、空闲＝`再见!`＋退出码 0，J10 原样）。母本 `owns_interaction_key`/`handle_selection_key` 的键盘复制分支（Ctrl+C/Cmd+C/Ctrl+Insert/Ctrl+Shift+C/Enter＝copy&follow）在本构型不接通：选区是鼠标覆盖层，除空闲 Esc（清选区，母本「先清再回底」）外任何键先恢复无选区态再按无选区路由（Enter＝提交、字符＝composer、Ctrl+Space 不路由）。vendored footer 的「Ctrl+C copy」提示在本驱动本就不渲染（`footer_with_navigation` 无调用点），不新增复制提示。不新增自动复制配置（视图 `copy_on_select`/`primary_selection`/`copy_on_select_clear_selection` 维持默认 false）。不启用搜索与打开链接（`ViewAction::OpenLink` 到达即丢弃）。

## 2. 复用与必要适配（母本固定提交 97771b1）

**复用（零 vendored 改动，全部经既有 pub(crate) 面）**：
- 视图选区状态机：`transcript_view/selection.rs`（begin/extend/end_selection、快照冻结、`has_selection_range`）、`input.rs::handle_mouse`（Down/Drag/Up/滚轮/右键→`ViewAction`）、`hit_test`（列↔偏移，选区几何与渲染同源）。
- 复制管线：`copy_action`→`copy_selected_text_with`（Markdown 序列化＋净化）→`Tui::copy_selection`（`SelectionCopy::EXPLICIT`）→`clipboard_copy` 回退链（容器内 arboard 不可用→OSC 52 `ESC]52;c;<base64>\x07` 写 /dev/tty，`Requested`→`CopyStatus::Unconfirmed`）。
- 反馈：`show_copy_feedback`/`render_composer_gap`（五态文案：Confirmed「Copied N chars」/Pending「Copying…」/Busy/Unconfirmed「Copy sent to terminal · paste to verify」/Err——**OSC 52 无宿主写入回执，不显示为成功**，Unconfirmed 文案如实区分）；完成回收 `finish_copy`（票据配对）。
- composer 侧：`prepare_composer_mouse`（母本无条件先行，冲刷粘贴突发——实测仅 Down 冲刷）、`copy_composer_selection`（composer 右键复制，PlainText 闭包）、`handle_composer_mouse`、`finish_composer_copy`。
- 边缘自动滚动：`tick_selection`（拖选悬停视口边缘每帧滚一行）。

**必要适配（全部在本地模块 `chat_driver.rs`，搜 `010` 注释）**：
1. **键守卫**（`:357-372`）：`idle_esc` 之外的任何键，若 `has_selection_range()` 先 `end_selection` 再走无选区路由——防 `has_active_interaction` 门把 Enter/母本复制键送入选区分支。Ctrl+C/Ctrl+D 分支在最前（J10 位置未动）。
2. **鼠标分发重构**（`:484-565`，母本 owned_transcript.rs:447-487 序）：`prepare_composer_mouse` 无条件先行→沟位（011）三臂保持最前→transcript 正文区 `handle_mouse`（含滚轮，返回 action 经 `copy_action(…, CopyFormat::Markdown)` 消费；OpenLink 丢弃）→composer 区 `copy_composer_selection`＋`handle_composer_mouse`（处理后非滚轮清 transcript 选区）。**正文区 Left Down 追加 `scrollbar_rows.clear()`**——鼠标接线使详情展开（`pointer_down→toggle_disclosure_at`）首次可达，补 011 §8.2 登记的失效面。
3. **剪贴板 poll**（Tick 分支 `:191-206`，坑 1）：`tui.clipboard.poll()`（OSC 52 终端写发生在 poll 内）→composer 票据 `finish_composer_copy`（回显落视图 gap 行）→视图 `finish_copy`（follow 旗标→`jump_to_latest`）。100ms tick 即 poll 节奏。
4. **提示沟行**（`split_screen_areas` `:833-855`＋draw）：transcript 与 pane 之间恒保留 1 行（极矮屏放弃），`render_composer_gap` 渲染复制反馈/回底控制——母本由 pane `ComposerRenderOptions.composer_gap` 内含，本地以驱动侧保留行实现（单一几何源纪律：draw 与鼠标分发共用 `split_screen_areas`）；`tick_selection` 移入 draw（100ms tick 兜底帧）。

## 3. TDD 证据

- **事前 RED**（实现前运行）：`cargo test -p terminal-chat --test e2e_acceptance -- s40_tui_drag_select_right_click_copies_via_osc52` → FAILED（`wait_screen("Copy sent")` 30s 超时；屏幕夹具正常渲染、无 OSC 52——选择/复制未接线）。
- **变异 RED**（改坏→红→恢复→绿）：
  1. 移除键守卫（`end_selection` 段改 no-op）→ **S41b 红**（选区激活时首键 Enter 被 vendored `handle_selection_key` 变为 copy&follow，不提交、30s 超时）；恢复→绿。S41 本身不红的原因：Ctrl+C 分支在守卫之前（is_ctrl_c 最先），守卫防的是 Enter/Ctrl+Insert 等——S41b 正是为此补的用例。
  2. （011 轮先例）端点/命中几何变异由既有单测钉住，本轮未重复。
- **审查驱动修正**（comprehensive-testing 轮）：S45 断言曾含恒真分支（`|| contains("S45-L10")`——底部视口 L10x 前缀恒在，S37 同坑）——改为 pre/post 顶条目号对照＋墙钟停驻（read_once 在持续重绘下即时返回，轮数≠时长，2.5s 墙钟 ≥25 tick）。
- 测试设计实证：avt 屏 `›` 渲染 2 列而布局列映射计 1 列（歧义宽字符）——逐列针定位对该字符差 1；S40/S43 的精确断言经验证成立，S46 改整行选择＋containment 断言（不依赖歧义列对齐）。

## 4. 验证

- e2e 新增 8 例（S40–S46＋S41b），全套 **60/60**；A7 回归 11/11；全量 `cargo check --workspace && cargo test --workspace --no-fail-fast`＝**2738 通过 / 0 失败 / 3 忽略**（011 收尾基线 2730＋本轮 8）。
- 用例映射（规格 §5）：
  - A1 内容一致性：S40（ASCII 逐字节精确）＋S43（中文＋emoji 逐字节精确；`sel_b64_decode(载荷)==needle`）。多行/折行/组合字符与「改坏 hit_test 须红」破坏对照：未单列用例——由 vendored `selection_tests`/`right_click_copy_tests` 单测面＋S43 宽字符列换算覆盖，如实登记。
  - A2 右键全链：S40（历史区，载荷＝选区、mock 零请求、退出干净）＋S42 末段（composer 区选区右键同链路，PlainText）。
  - A3 反馈如实：S40 断言「Copy sent」可见且屏幕无「Copied 」（Unconfirmed≠成功）。Busy/Err/100KB 超限态：环境不可达态（无 Pending 竞争窗口/无法制造超限会话——超限走 Err 文案同渲染路径），vendored `composer_gap_tests` 覆盖文案分支，登记。
  - A4 无副作用：S40/S41b（mock 计数、草稿进提交载荷、退出码 0）。
  - A5 交互矩阵：S42（无选区右键 no-op；Esc 清选区；composer 点击清选区）、S41b（Enter＝提交）、S44（滚轮保留选区·快照冻结；沟位按下不开选区；resize 往返后选区存活＝第三条相同载荷）。turn 进行中快照冻结无用例（需流式 turn 夹具），vendored 快照单测覆盖机理，登记。
  - A6 安全：S46（OSC 52 伪装/CSI/BEL 混入正文：渲染＋拖选阶段零自发写入；右键后唯一载荷＝净化文本，控制字符剥离、敌意原文不逸出终端字节流）。
  - A7 用户实机验收：见 §6 复验步骤（宿主粘贴为最终验收，**未实测、保留该项**）。
  - A8 回归：如上（60/60＋全量 2738/0/3）。

## 5. 独立双轴审查与处置

- **Standards 轴**：硬违规 1（S45 恒真断言，上述已修）；判断性意见已处置——`composer_ready` 嵌套合并、`drag_select_needle` 返回 `()`、`sel_b64_decode` 收紧 `unwrap`；`gutter_column` 更名意见保留（011 期命名，语义可读）。测试 `msg` 闭包三处近似重复（测试局部，不改）。
- **Spec 轴**：无 P1/P2。核查通过项：Ctrl+C 分支先于一切路由（`:356`）、Enter 首键即按＝提交（S41b）、Unconfirmed 不显成功、沟位先于选区、prepare 无条件先行、Down 清 RowCache、`split_screen_areas` 单一几何源、vendored 零改动、011 端点修复在位。登记缺口：A1 多行/组合字符单列用例、A5 turn 中快照、A3 Busy/超限态（如上）。

## 6. 用户实机复验步骤（A7——宿主粘贴最终验收，未实测保留）

1. 容器内 `/workspace` 运行 `cargo run`，`/resume seedsel`（或任意超一屏会话）。
2. 鼠标左键在历史文字上拖选（高亮反显）；右键复制。
3. 切到任意宿主应用粘贴：内容＝选区文本（判据：**能粘贴出选区文本＝通过**）。
   **实际粘贴失败时（保全轮口径）**：记录「该场景未通过，原因待定位」，并保留证据——
   终端类型与版本、连接链路（本地/SSH/tmux 等）、界面提示文案、期望与实际结果；
   **不直接判为终端限制，也不排除生产缺陷**（OSC 52 已正确发送 ≠ 宿主已写入剪贴板，
   两者的区分是事实陈述而非归因）。常见终端对 OSC 52 的支持名单仅系上游文档载明，
   本环境未实测、不构成成功保证。
4. 中文/emoji 各选一次（S43 对应）；Shift+原生终端选择作为补充路径确认可用性。
5. 有选区时按 Ctrl+C：行为与无选区时一致（turn 中＝取消；空闲＝退出 `再见!`），无剪贴板写入。

## 7. 遗留与后续

- 宿主粘贴实测（A7）保留；Windows 真机（008 A6）继续暂停。
- 键盘选择模式（Ctrl+Space/select_mode）与搜索维持范围外；010 落地后 `011` 登记的「搜索激活时 RowCache 失效」接线项仍待搜索启用时处理（搜索依旧不可达）。
- composer 区选区交互面（拖选/右键复制）已接通并有 e2e；composer 复制反馈回显同走视图 gap 行（母本同序）。


## 8. 补验轮（2026-10-15 第二轮）：缺口关闭＋坐标定论

### 8.1 坐标依据定论（任务二，逐单元格实证）

以临时探针逐格转储 avt 屏幕：用户行 `[0]="›" [1]=" " [2]="S" [3]="p" [4]="r" [5]="" [6]="W"`——
**avt 对 U+203A「›」按宽度 1 计（单格）**，与 TextLayout（unicode-width）一致；空白格
contents 为空串（拼接即丢——此前「S40 恰好对、S46 差一」实为夹具宽度启发（› 计 2）与
空格丢失的叠加假象）。**结论：测试设施差异，无生产选区映射缺陷**（生产端歧义宽字符
在按 2 格渲染的真实终端上的错位属终端类限制，母本同架构同限制，非本轮缺陷）。
处置：`screen_needle_span` 改**单元格锚定**（逐格拼接＋每字符记录所属格列，删除宽度
启发 `sel_char_width`）；S46 从 containment 升级为**逐字节精确断言**
（`载荷 == "EVILHEAD]52;c;bnVsbA==TAILEND"`）；注入防护断言保留（渲染/拖选零自发
写入＋右键后合法载荷与选区一致）。

### 8.2 缺口关闭（补验＝以现有实现直接跑，首轮结果如实记录）

| 缺口 | 用例 | 首轮结果 | 说明 |
|---|---|---|---|
| A1 多行/软折行/组合字符 | **S47** | 多行段**首跑即过**（载荷逐字节＝消息全文，真实 `\n` 保留）；折行段夹具两次修正（30×重复＝195 列未过 200 宽度阈、后 60×＝360 列成折行）后过 | 组合字符 e＋U+0301、中文、emoji 均入载荷；无沟位/相邻字符混入；mock 零请求；草稿未动＝结构性（全程仅鼠标事件） |
| A5 回复到达期间选区 | **S48** | 夹具修正三轮后过：漏提交 turn（粘贴突发吞 Enter→改 send_line 分步节奏）、一页 PageUp 不足（90 行会话改三次）、弱 OR 断言重写为双段证明 | 溢出会话双段证明：保持期旧历史在屏且新回复不上屏，Esc 回底后回复可见；选区快照不漂移（右键载荷＝原选区）；turn 中＋选区 Ctrl+C＝取消（CLI 存活、零新增复制、请求计数＝2 不撤回） |
| A3 失败结果分派 | **S49** | **首跑即过** | >100KB（100_104 > OSC52_MAX_RAW_BYTES=100_000）三击整行选择→右键＝「Copy failed」反馈＋裸流零 OSC 52＋不显示成功文案；Esc 后小选区复制成功（逐字节）——驱动消费失败结果、如实反馈、继续交互。Busy 态无确定性触发窗（worker 完成即出窗），维持 vendored 单测覆盖登记 |

三处首轮失败均为**夹具问题**（补验如实记录），**未发现生产缺陷**——事后无生产代码
变更，无 TDD 声明（不以变异补称事前 RED）。

### 8.3 独立定点审查与处置

空跑审计：S46/S49 可证伪；处置 4 项——S47 第二段复制改墙钟等待（轮数≠时长）、S47
弱 OR 断言删除（结构性注释）、S49 恢复载荷精确化（assert_eq）、S48 回复等待余量
2.5s 延迟＋4s 墙钟（确定性余量）＋Ctrl+C 段改墙钟。夹具边界推导（`after.max(last_cell+1)
.min(PTY_COLS)`，行末落沟位列经超内容列钳制不多选）审查通过；生产代码零改动声明
核实成立。

### 8.4 终验

`cargo check --workspace` 通过；`cargo test --workspace --no-fail-fast`＝**2741/0/3**
（上轮 2738＋S47/S48/S49）；e2e **63/63**；A7 11/11（全量内含）。

### 8.5 措辞收敛（终端支持表述）

人工复验与文档不再以「XX 终端 ≥N 支持 OSC 52」为保证——该名单未在本环境实测，
仅作「常见文档载明支持（未实测）」参考。**判据区分**：OSC 52 已正确发送（本仓 e2e
可证：裸流序列＋载荷逐字节）；宿主实际能够粘贴（**仍需现场验证，未实测**）。

**实际粘贴失败协议（保全轮补记）**：记录「该场景未通过，原因待定位」，保留终端类型、
连接链路、提示与实际结果四项证据；在定位完成前不得归因为终端限制，亦不得据此排除
生产缺陷——以具体证据诊断后再结论。