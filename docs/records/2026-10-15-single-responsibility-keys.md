# 按键单一职责轮（2026-10-15）：Ctrl+C 恒退出 / Esc 专取消 / q 关详情页

规格：012 §9（新契约主文）＋001/006/008/010/011 同步修订；前置调查＝`docs/records/2026-10-15-esc-audit.md`（用户在其 A/B 选项外选定方案 1——全部旧 Esc 分工撤销、按键各司其职）。基线：分支 feat/008-codex-tui-port、HEAD b0e8d48、149 项未提交（含审计记录）、v11 冻结；历史验证基线 2768/0/3、e2e 87/87（核对一致）。

## 1. 实现（最小差异：驱动统一分发＋vendored 三处 ADAPT）

- **`src/tui/src/chat_driver.rs`（本地）**：
  - 主视图 Key 臂：`is_ctrl_c` → 恒 `return Quit`（删运行中取消＋jump_to_latest）；新增 Esc 拦截臂（在 ctrl_d、选区守卫、Ctrl+T、视图/composer 路由之前）——运行中＝`set_cancel_requested(true)`＋needs_draw，空闲＝无动作，两态均消费不外溢；选区守卫删 `idle_esc` 豁免（Esc 已在前拦截，其余键守卫不变）。
  - 详情页槽位：Ctrl+C 恒 Quit（删运行中取消分支）；Esc 拦截——运行中＝取消＋`overlay.set_notice("已取消当前任务：已停止等待模型回复（迟到结果将被丢弃）")`＋schedule_frame（页保持打开、反馈在页状态行、不改正在查看的旧工具 cell），空闲无动作，消费后 continue。
  - `transcript_owns_key`：Esc 归属子句删除（一律不归视图；母本 Esc-to-latest 分支保留在 vendored 但本地无路由入口）＋注释更新。
  - 模块头：修正失实的「Esc＝中断（经 hooks.on_interrupt）」（无此 hook；实际经 composer→AppEvent::Interrupt，本轮改为驱动直拦 set_cancel_requested）＋新契约全文。
- **`src/tui/src/pager_overlay/transcript.rs`（vendored，012-ADAPT）**：页模式 Esc 直关分支移除（不可达，防回归）；页模式启用 `keymap.close`（q 关页——母本原生能力；close 绑定含 ctrl+c 但被驱动先行拦截恒退出，实际可达成员仅 q）；页提示键 Esc→q（键标取 keymap.close 首绑定）；新增 `set_notice` 访问器。`pager_overlay.rs`：`Overlay::set_notice` 透传。
- **防接管（结构性）**：两键均在任何组件路由前消费——vendored 选区清 Esc（input.rs:414）、聚焦 Cancel（disclosure ListAction）、Esc-to-latest（input.rs:452）、composer `should_interrupt_running_task`、页 Esc 关闭全部不可达；不清理无关母本代码。
- 保留不动：条件绘制/帧调度（011 §7）、右键复制与净化（010）、工具结果/会话恢复/排队输入（012/007/008）、取消真实边界文案（010 §6/§7——停止本地等待、迟到结果弃置、不宣称中止底层请求）。「esc to interrupt」提示现与实际一致（保留）；页提示改「q back to conversation」。

## 2. TDD 与验证

- 事前 RED×8（对旧实现）：s73（Ctrl+C 运行中退出收尾）/s73b（页内同）/s19（Esc 空闲无动作）/s74（聚焦态 Esc 取消）/s75（? 展开态 Esc 取消不收提示）/s76（空闲矩阵无动作）/s63（q 关页）/s70（页内 Esc 取消＋页保持＋notice）；s69b 红＝选区保持断言。**预绿如实登记**：s69/s71/s72（Esc 裸态取消链经 composer interrupt 路径在审计轮已证实可用，本轮改键后语义不变）。
- 原用例按语义改写（非批量替换，逐处登记）：n2b/n3 系列 3 处取消键改 Esc（含改名 n2b_esc_interrupt…）、s48（取消键＋清选区改点击输入区＋回底改 Ctrl+End）、s42/s47/s49/s56（清选区改点击）、s22/s24/s36b/s38（回底改 Ctrl+End `ESC[1;5F`）、s57–s62/s63–s66（页关闭改 q；s63 补主对话 q＝普通文字断言）、s30/s30b（页语义下 Esc 步骤改「无动作＋页保持」＋q）、wait_submitted_then_show_reply（双 Esc 改 Ctrl+End）。Ctrl+C 退出既有用例（s12/s41/s68/s2c）保留不动并复跑。
- 新增：s73（进程组回收＝pgrep 唯一标记 bounded 轮询归零＋正向锚点先证在运行；会话排空＝auto 文件含本轮记录；终端恢复＝?1049l/鼠标捕获复位）＋s73b/s74/s75/s76。
- 单测：`esc_never_routes_to_transcript` 重写（Esc 一律不归视图；补 Ctrl+End 归属断言）；`scroll_key_routing_contract` 注释同步。
- 变异×4（全部捕获，恢复零残留）：①主视图 Esc 拦截臂禁用→s76/s74 红；②Ctrl+C 运行中不退出→s73 红；③页模式 close 禁用→s63 红；④页 notice 文案替换→s70 红。
- 终验：`cargo check --workspace` 0 错误；全量 **2773/0/3**（基线 2768＋5）；e2e **92/92**。

## 3. 安全检查

改动仅键路由与页内固定文案 notice（无外部输入进入）；无新增序列化/配置/持久化字段；取消链路与退出收尾复用既有路径（006 N2b/007 flush/008-A5 restore）。`set_notice` 字符串为编译期字面量。

## 4. 残余（如实）

- 选区冻结视口（S48）下，主视图 Esc 取消的反馈行暂不可见（Esc 不得清选区/回底的直接代价）——点击输入区或 Ctrl+End 后可见；s69b 按此断言序（取消生效→选区未变→回底后反馈可见）。登记为设计取舍。
- mut4 首次构造括号失衡（编译失败未取得运行证据），改文案替换变异后取得（§2）。
- vendored Esc 消费分支（选区/聚焦/Esc-to-latest/composer interrupt）仍在源内但经驱动拦截不可达——按「不清理无关母本代码」保留，结构性防接管由拦截顺序保证＋变异①证明。

## 5. 人工复验（新版，区分三键）

- **Ctrl+C**：任务运行中（含工具执行中/详情页内）按 Ctrl+C→CLI 退出（`再见!`、码 0、终端恢复），工具进程不残留；空闲（任意选区/聚焦/页面态）同。
- **Esc**：任务运行中（裸/有选区/F4 聚焦/详情页内/? 提示展开）按一次→当前任务取消、CLI 存活；详情页不关闭（页内出现「已取消当前任务…」反馈行）；随后可正常提交新请求。空闲按 Esc→无任何变化（选区/焦点/页面/阅读位置/提示原样；清选区＝点击输入区、回底＝Ctrl+End、解焦＝F4、收提示＝再按 ?）。
- **q**：详情页内按 q→一次返回对话（阅读位置/草稿/光标/输入归属恢复）；主对话输入 q＝普通文字。
- 旧规则（Ctrl+C 取消/Esc 关页/Esc 清选区回底）下的历史实测通过记录保留，不视为新规则已通过现场验收。

## 6. 独立审查处置（2026-10-15，双轴）

结论：**需整改**（1×P1、3×P2、5×P3）——全部处置如下：

- **F1（P1，采纳·修复）**：Ctrl+R 历史搜索态被困（Esc 被拦、Ctrl+C 恒退出→无出口）。修复＝驱动主视图 Esc 拦截对 `pane.history_search_active()` 放行（composer 输入子模式归母本语义；转录层状态不放行）＋pane 访问器（vendored ADAPT 登记）＋TDD：s77 事前 RED（无匹配 query 下 Esc 后 reverse-i-search 提示仍在）→GREEN。运行中若处搜索态，Esc 先退搜索（一次按键一次动作）。规格 §9 登记偏离。
- **F2（P2，采纳·修复）**：详情页 Esc 拦截仅滤 Press——Repeat 穿透可复活 vendored 旧分支。拦截条件改 `Press | Repeat`。
- **F3（P2，采纳·修复）**：主视图非跟随态「↓ Back to bottom · esc」提示与实际相反。vendored `follow_control.rs` 三处去「· esc」段（012-ADAPT 登记）。
- **F4（P2，采纳·修复）**：退出收尾顺序文档失实——实际＝终端恢复（run_tui 返回后先行）→置取消→进程组终止回收→会话排空（flush 阻塞等 history 锁）→`再见!`。三处（chat_driver 模块头/012 §9/008 头部）措辞修正。
- **F6（P3，采纳·硬化）**：`transcript_owns_key` 交互激活门对 Esc 兜底返回 false（驱动前置拦截为主防线，防拦截重构后落入 vendored 分支）。
- **F7（P3，采纳）**：s73b 补「Working 可见」运行态前置锚点（原夹具意图未固化）。
- **F5/F8（P3，采纳·清理/登记）**：残留旧「Esc 关闭/回底」注释与测试名 6 处更正；vendored `footer.rs` 「esc clear」不可达文案并入 010 既有「vendored footer 提示不渲染」登记（本地无调用点）。
- 备忘不改：带修饰 Esc（Alt+Esc 等）落 vendored 选区清为母本行为（契约仅约裸 Esc）；「双击 Esc 编辑上一条」EscHint 提示随 Esc 拦截不可达（vendored 死分支，同 F8 类登记）。

## 7. 收尾轮（2026-10-17）：消除历史搜索 Esc 例外（F1 整改被替代）

用户定稿契约：Esc 绝不退出历史搜索（运行中＝取消任务、空闲＝无动作）——§6 的 F1「放行」整改**已被替代**（历史整改过程保留于 §6，以本节为准）。

- **出口核查（读实现，不预设）**：`history_search.rs` 既有退出路径＝①Enter（仅 `status==Match`：接受历史项、光标移尾）②Esc/Ctrl+C/`\x03`→`cancel_history_search`（恢复原草稿＋vim 状态）③Up/Down/Ctrl+R/Ctrl+S＝遍历不退出。「唯一出口是 Esc」仅在**无匹配/空闲态**成立（匹配态 Enter 即出口）。
- **方案（复用既有非 Esc 出口，零新增按键）**：扩展 Enter 既有分支——匹配态＝接受（原行为）；无匹配/空闲/查找中态＝`cancel_history_search()`（退出＋恢复原草稿）。冲突检查：该态 Enter 今日＝吞键无动作（`(None,true)`），扩展不覆盖任何既有键位/功能、不删除历史搜索；运行中 Enter 仍被搜索态消费（不会误提交/排队）。
- **实现**：驱动删 `history_search_active` 放行与 pane 访问器（严格拦截）；vendored `history_search.rs` 两处 ADAPT（Enter-else 分支＋footer 提示：Match「enter accept」（去「esc cancel」）、NoMatch「no match · enter restore」）；样式单测与 3 份快照同步（逐项核对）。
- **TDD（s77 重写为 s78＋新增 s79）**：s78＝空闲矩阵（无匹配 query 下 Esc 搜索态保持→Enter 退出恢复原草稿「草稿XY」＋光标拼接验证→匹配态 Enter 接受历史项→搜索态 Ctrl+C 退出码 0）；s79＝运行中（Esc＝取消反馈出现＋`reverse-i-search` 保持＋迟到回复不上屏→Enter 退出搜索→下一请求正常）。事前 RED×2（旧行为＝Esc 退搜索不取消）。变异＝去 Enter-else→s78 红（恢复零残留）。
- 复验指引更新：搜索态退出＝Enter（无匹配恢复原稿/匹配接受）；Esc 在搜索态与全局一致。

## 8. 定点复审处置（2026-10-17）

结论：**可交付**。处置：
- 采纳 F4（P2）：s78 ③ 补强——Enter 前正置 Match 态（footer「enter accept」可见），接受后追加输入验证「hist-target-msg尾2」拼接（区分 transcript 回显，防「误走 cancel」假绿）；复跑绿。
- 采纳 F5（P3）：AGENTS.md 收尾轮句补全量数字（2775/0/3、94/94）＋日志落盘指向。
- 登记不改（P3）：F1 带修饰 Esc（如 Alt+Esc）落 vendored 搜索取消分支——母本行为，契约指裸 Esc，常规终端罕见；F2 Ctrl+C 原始 \x03 变体在 crossterm 归一化下不可达（e2e \x03 实测走 Char('c')+CONTROL）；F3 搜索态＋F4 活动聚焦复合态下 Enter 被详情页入口抢占（012 §8 既有路由，契约文本未覆盖该复合态——已知取舍）；F6 footer_mode 两快照 source 元数据路径风格不一（不影响比对）。


## 9. 复合态收尾轮（2026-10-17）：历史搜索＋工具聚焦的 Enter 抢占消除

§8 登记的 F3（P3「复合态 Enter 被页入口抢占＝已知取舍」**已被本轮替代**——用户立项核清并消除）。

- **可达性实证（s80，真实工具夹具＋正置控制 F4→Enter＝开页防空跑）**：「Ctrl+R→query→F4→Enter」裸复合态在旧代码下打开 `/ TOOL DETAILS` 页（RED 屏幕证据）——F4 在搜索中经 `transcript_owns_key` 设焦、搜索仍在＝复合态真实形成，Enter 命中驱动 `Enter+is_activity_focused` 分支。「F4→Ctrl+R」**不形成**复合（Ctrl+R 经 handle_disclosure_key「普通文本恢复输入」清焦后落 composer 开搜索）；「F4 后输入 query」同样被首字符清焦自解（s80 场景 3 钉住）。
- **修复（最小，零新框架）**：驱动 Enter 聚焦分支内 `pane.history_search_active()` 让路——搜索态下 `clear_activity_focus()` ＋ 落穿 composer 路由（Enter 按搜索规则：接受/恢复）；退出搜索后再次 Enter＝正常输入规则（无旧焦点）。非搜索态分支原样。
- **TDD**：s80（控制＋场景 1 自解路径＋场景 2 裸复合无匹配恢复独立草稿「草稿2」＋场景 2b 裸复合匹配接受＋场景 3 消解机制；后续提交恰好一次、页未开、草稿/光标断言）事前 RED（场景 2 开页）→修复 GREEN；变异（让路门禁用）→s80 红，恢复零残留。邻域 s78/s79/s74/s63/s30b/s59/s66 复跑全绿。
- **终验**：check 退出码 0；全量 **2776/0/3**（2775＋s80）退出码 0；e2e **95/95** 退出码 0（[日志摘要及原件索引](../testing/verification-history.md#v14)）。范围外登记：复合态下 Ctrl+T 仍可开页（§8 既有路由，本轮不扩展）；复合态下其余键（方向键等）归视图导航（母本语义）。


## 10. 复合态轮定点复审处置（2026-10-17）

结论：**可交付**。处置：
- 采纳 P2-1（真实契约缺陷）：搜索态 composer 文本＝预览稿（原稿/匹配项）——恰为 "exit" 时裸 exit 检查误触发「一次 Enter＝退搜索＋退程序」。修复＝`is_bare_exit` 追加 `!pane.history_search_active()`（J10「composer 将处理该键时判定」语义对齐）；s80 场景 4 覆盖（变异移除门控→红，恢复零残留）。该缺陷在 v13 基线的无聚焦搜索态即存在，本轮一并闭合。
- 采纳 P2-2：s80 场景 2b 补「尾3」追加＋提交恰一次（count==5）＋「复合草稿尾1尾3」拼接断言（区分 transcript 回显）。
- P3-3（日期）说明：2026-10-17 为环境系统日期（复审会话未含该通知），非笔误，不改。
- 范围外登记（不改）：复合态下 Ctrl+T 仍可开页（§8 既有路由）。
- 复审确认：让路分支落穿后 Enter 必达搜索处理（两出口均非 Submitted——运行中不误排队/提交）；清焦时机充分；非搜索态零变化；无第三动作/双绘。
