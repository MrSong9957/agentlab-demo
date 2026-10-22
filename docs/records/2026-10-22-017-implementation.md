# 017 update_plan 最小接入——实施记录

- 日期：2026-10-22。
- 分支：`feat/017-update-plan`（基线 `main`@895cd60，变更未提交待批准）。
- 规格：[017-update-plan](../specs/017-update-plan.md)。Codex 复用基线：`d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`（本地缓存 `/tmp/openai-codex-investigate` `git show` 读取）。
- 状态：已批准交付（2026-10-22 用户批准提交、合并至本地 main、删除任务分支；不推送）。

## 1. 复用与适配登记

| 复用部分 | 上游源码（固定提交） | 实际落点与适配 |
|---|---|---|
| 参数三类型 | `protocol/src/plan_tool.rs` | `src/shims/codex-protocol/src/plan_tool.rs`（原 008 桩替换为真实类型；保留字段/枚举/Serde 属性，省 JsonSchema/TS 派生） |
| 工具描述与 Schema | `core/src/tools/handlers/plan_spec.rs` | `prompt.rs::update_plan_tool()`——ResponsesApiTool 映射为 Chat Completions function 形态（required＋additionalProperties:false，同既有工具） |
| 参数解析与成功确认 | `core/src/tools/handlers/plan.rs` | `src/app/src/plan.rs`——Serde 解析复用；上游文本 `Plan updated` 适配为本地两字段 JSON 结果（经 ToolEnd 展示） |
| 计划清单渲染 | `tui/src/history_cell/plans.rs` | `src/tui/src/history_cell/plans.rs`——仅 `PlanUpdateCell`/`new_plan_update`（Proposed Plan 流式与 Plan Mode cell 不移植）；适配：活动 ID 取宿主传入 ID（tool_call_id/投影合成 id），不生成随机 UUID |
| 模型使用指引 | 嵌入母本 `assets/codex/prompt.md` | `sent_instructions()` 停止剔除 `## Planning`/`## \`update_plan\`` 两节（母本原文保留；`without_update_plan_instructions` 过滤器及其锚定测试随之移除，差分对照由 prompt_assembly s2 承担） |

依赖变更：`src/app/Cargo.toml` 增加对已有本地 `codex-protocol` shim 的 path 依赖（无新第三方依赖、无版本升级）。

## 2. 行为实现要点

- **分派**：`turn.rs::execute_tool` 增 `update_plan` 臂（纯解析，无进程/文件/命令副作用，display 恒 None）；`skipped_by_cancel` 增两字段失败形状；未知工具提示列入 update_plan。内部记忆合并轮不注册（分派层仍按 mem 注册表拒绝，单测证明）。
- **取消/额度**：沿用现有循环——取消跳过＝`{"ok":false,"error":SKIPPED_BY_CANCEL_MESSAGE}`（不产生成功快照）；每轮 8 次模型请求按请求计数，第 8 批计划结果保存后报上限、无第 9 次（t17/t18）。
- **行式 CLI**：`main.rs` ToolEnd 分支——成功输出 `计划已更新（c/n 步完成）`＋`说明：`＋`[已完成/进行中/待办] 步骤`（空数组＝「暂无步骤」），失败/取消输出 `计划更新未生效：<原因>`；经 `say`（sanitize）打印。
- **TUI**：`tui_host::tool_end_item` 增 `update_plan` 臂——成功快照（结果明确成功且参数可解析）经 `sanitized_for_display` 过滤后交 `HostHistoryItem::Plan`；失败/取消只带原因；结果未知/参数损坏返回 None（文本降级）。`chat_driver::project_tool_item` Plan 臂：成功→`PlanUpdateCell`（实时 ToolEnd 与 /resume 同一投影）；失败/取消→原因 cell。ToolBegin 走既有活动行回退。
- **恢复**：`project_history_items` 按现有 id→(name,arguments) 配对复用同一转换；恢复零请求零执行（s91/e21）。

## 3. 测试证据（本轮新增/明确变化）

新增：`plan.rs` 单测 7（接受面/拒绝面/结果形状/成功判定/快照提取/过滤副本/行式输出）；`turn.rs` 2（分派形状＋合并轮隔离、取消两字段）；`tui_host.rs` 2（终态转换、恢复配对）；`chat_driver.rs` 2（成功/失败投影、多条快照顺序）；`plans_tests.rs` 4（上游两例快照＋URL 不拆分＋空计划/中文/宿主 ID）；`tool_loop` t15–t18（完整流程/参数错误/额度/CLI SIGINT 取消）；`session_cli` e21（V1 形状/原始参数/display 不落盘/恢复零请求/输出过滤）；`e2e_acceptance` s91（PTY 真实屏幕：紧凑视图排序清单、失败原因、Ctrl+T 详情页说明、/resume 一致、零请求）。

明确变化的既有断言：prompt_assembly s2/s3/s4/s6（四工具＋指引恢复＋Schema 差分锚定）、llm_flow ×2、tool_loop t1、e2e s1、memory_inject m6_d4（带记忆摘要轮四工具）。

**快照保真**：新生成 `plans_tests__plan_update_{with_note_and_wrapping,without_note}_snapshot.snap` 与 008-TRIM 遗留的同名上游快照正文**逐字节一致**（仅元数据行不同）——渲染移植保真由此锚定；孤儿旧快照（tests__ 前缀，2 个）删除。

## 4. 关键用例有效性证明（可逆变异，全部检出后还原复绿）

| 变异 | 模拟缺陷 | 检出于 |
|---|---|---|
| M1 | 用户轮工具面移除 update_plan（漏注册） | prompt_assembly s3 失败（工具数断言） |
| M2 | Plan 失败分支渲染成功清单（失败误画清单） | chat_driver plan_projection 失败 |
| M3 | success_snapshot 去掉结果成功门控 | plan snapshot 单测失败（失败结果产出快照） |
| M4 | sanitized_for_display 恒等（终端过滤失效） | plan＋tui_host 两单测失败 |
| M5 | 取消跳过改为单 error 通用结构 | turn 取消形状单测失败 |

「恢复重放」未做变异：恢复投影为纯函数路径（不可达 sender/执行器），架构上无重放通道；s91/e21 的零请求断言锁定该行为，构造重放需重新架构而非现实回归，故以断言＋说明代替。

## 5. 安全校验（聚焦项）

- **计划输入不执行**：t15（计划调用无文件副作用，目录仅自动会话文件）；plan.rs 无 fs/exec/net 调用（代码审查）。
- **内部工具隔离**：合并轮分派层拒绝（turn 单测）；合并轮请求面仍恰三项 mem 工具（既有 memory_phase2/runner 断言不变，全量通过）。
- **终端控制序列过滤**：单测（plan/tui_host，展示副本过滤、原件保留）＋e2e（e21 行式 ESC/BEL 不外泄且可读文字保留；TUI 侧过滤发生在 tui_host 转换层，经 s91 PTY 验证渲染）。
- **恢复不重放**：s91/e21 零模型请求断言。
- 已知接受面（同上游）：计划数组无数量上限——受既有 10 MiB 响应体上限约束，超大清单渲染代价与上游一致，未加严（§3.1 不缩小输入范围）。

## 5.1 安全门禁证据汇总（补验收轮复核引用）

聚焦三项的门禁证据（详见上文 §3/§4 与 §5）：
- **计划输入不执行**：`plan.rs` 无 fs/exec/net 调用（代码审查）；t15 计划调用零文件副作用（目录仅自动会话文件＋真实 exec 产物）；t17 连续 8 次计划调用无任何副作用。
- **内部工具隔离**：合并轮分派层拒绝 `update_plan`（`turn::tests::update_plan_dispatch_shapes_and_memory_merge_isolation`）；合并轮请求面仍恰三项 mem 工具（既有 memory_phase2/runner 断言，全量通过）。
- **终端控制序列过滤**：单测三处（plan 展示副本、tui_host 快照副本、审查补齐的 TUI 错误文本过滤）；e2e e21（行式 ESC/BEL 不外泄且可读文字保留）；s91/s92（TUI 渲染链路）。

## 6. 审查（code-review 双轴）与处理

**Standards 轴**：
- 硬违规 1（已修）：`prompt.rs` 被删函数的历史说明以 `///` 遗留，会错挂到下一函数 rustdoc——改为 `//` 普通注释。
- 判断性提示（接受并登记）：plans_tests `render_lines` 与 tests.rs 同形（兄弟模块私有，已在文件头声明）；plan_tool 三类型保留上游 `Serialize` 派生（保真取舍）；CLI 中文状态标签与 TUI 上游英文标题各通道各保真。
- 预存项（只提醒不动）：`history_cell/mod.rs` 的 `use crate::style::proposed_plan_style;` 在 HEAD 已 unused（008 裁剪残留，ProposedPlanCell 不在本轮最小移植面）。

**Spec 轴**：
- 缺口 1（已补）：005/006/007/008/012/016 规格修订注记与 AGENTS.md／project-lessons 索引同步（见 §7）。
- 缺口 2（属规划阶段既有产物，保护不动）：AGENTS.md §1 复用原则措辞与 §3 文档豁免段为 017 规划轮已有未提交改动，非本轮引入，保留原样。
- 可疑实现 1（已修）：TUI 计划失败原因文本可内嵌模型可控变体名——展示副本补 `sanitize_for_terminal`（CLI 侧经 say 本已过滤）；测试扩展覆盖。
- 可疑实现 2（登记不改）：TUI 紧凑视图按状态排序（上游母本原行为，§2 允许复用）；完整视图与行式 CLI 均按原文顺序，s91 断言按排序语义。

## 7. 规格与索引同步

005（适配①停用＋四工具＋017 修订注记）、006（头部状态＋范围＋017 修订注记）、007（017 补充：V1 形状不变）、008（plans 裁剪项标注 017 恢复）、012（017 补充：Plan 变体同一投影通道）、016（实现位置注记四工具）、017 规格（头部状态改已实施待批准＋§7 实施轮链接）、AGENTS.md §7（十二模块＋四工具）、project-lessons（§3 状态、§7 经验 4 条、§8 基线）。

## 8. 最终验证

容器内 `/workspace`：`cargo check && cargo test --workspace --no-fail-fast` exit 0——
**51 个测试目标全部 ok：3103 通过 / 0 失败 / 3 既有忽略 / 0 过滤**（含补验收轮 s92；3 忽略＝Windows AltGr 专项与两个子进程 helper，与基线一致；`git diff --check` 通过——曾修一处 006 注记 EOF 空行）。

用例数账目（首轮）：新增 23（plan 7＋turn 2＋tui_host 2＋chat_driver 2＋plans_tests 4＋tool_loop t15–t18 共 4＋session_cli e21＋e2e s91）、移除 4（`without_update_plan_instructions` 上游锚定测试随过滤器移除），净 +19；补验收轮再增 s92（净 +20，总计见 §8 与 project-lessons §8）；相对 HEAD（895cd60）实测基线见 project-lessons §8。lint：`cargo clippy`/`cargo fmt` 工具链未安装（AGENTS.md §7 既有登记），未执行；`cargo check` 通过，改动文件零新增 warning。

## 10. 补验收轮（2026-10-22，第二轮）

### 10.1 真实缩放验收（s92）

新增 e2e `s92_tui_update_plan_resize_reflow_integrity`：同一 PTY 会话内实际改变窗口尺寸（宽 200 → 窄 40 → 宽 200，master TIOCSWINSZ→SIGWINCH→驱动 Resize 重绘）。断言（全部有界等待＋VT100 解析后真实屏幕，无固定延时）：
- 宽屏：紧凑视图三种状态齐全、长中文步骤连续成行；
- 窄屏：紧凑行重排（长步骤裁剪出 `…`、连续长行消失）、状态标记保持；Ctrl+T 详情页完整视图**折行且文本完整**（首行前缀＋次行尾段「乙…癸」各自可见、整串不连续、说明可见）；
- 回宽：完整步骤重新连续成行、`…` 消失；
- resize 全程零额外模型请求（恰 2 次）。

测试基建：`VT100Backend::set_size`（vt100 `screen_mut().set_size`）＋ `PtySession::resize_and_sync`（既有 `resize` 保持解析器原尺寸服务行号对应断言，新方法同步尺寸服务布局断言，语义互补）＋ `wait_screen_pred`（谓词有界等待）。有效性证明（第 6 项可逆变异）：`resize_and_sync` 不发 TIOCSWINSZ（仅改测试侧屏幕）→ 窄屏重排断言超时红；还原后复绿——用例确证依赖真实 SIGWINCH 重绘。

### 10.2 显示契约核对（017 §3.1/§3.3）

| 契约点 | 结论 | 证据 |
|---|---|---|
| 原始参数保序保原文 | 符合 | t15（请求面 arguments 逐字）、e21（会话文件＋恢复请求逐字，含转义序列）、plan 单测（原件未被过滤改写） |
| 完整快照保序保原文 | 符合 | 完整视图按 `plan.iter()` 原序渲染（plans.rs）；CLI `cli_lines` 原序（plan 单测锁定顺序）；行式/TUI 均无改写步骤文本 |
| 默认紧凑视图 | 符合（上游复用条款） | 紧凑视图按状态排序（进行中→待办→已完成，母本 `sort_by_key` 原行为）＋计数头＋至多 3 行预览；**说明不在紧凑视图**（母本语义），经 Ctrl+T 详情页/展开呈现——s91/s92 按此断言 |
| 详情视图完整计划＋实时/恢复一致 | 符合 | 详情页＝完整视图（说明＋全部步骤原序＋空计划提示）；实时 ToolEnd 与 /resume 共用 `tool_end_item`＋`project_tool_item` 同一投影（tui_host/chat_driver 单测），s91 双会话同串断言、s92 缩放后仍一致 |

未发现违反现有契约的行为；紧凑视图排序系 §2/§3.3「复用 Codex 清单渲染」的组成部分（快照与上游逐字节一致佐证），未作改动、未改契约。

### 10.3 已有修改归属与拟提交范围

开工时（本轮会话分支创建前）git 状态快照：`M AGENTS.md`、`M project-lessons.md` 及 7 份未跟踪记录/规格已存在——属 017 规划轮产物（含 AGENTS.md §1 复用原则改写、§3 文档豁免段、project-lessons §3 017 行/§7 两条复核经验）。实施轮在其上追加：AGENTS.md §7 两行（模块数/工具数）、project-lessons §3 017 行改写＋§7 017 经验＋§8 基线。两轮同属 017 任务流。

未跟踪记录归属（引用关系核查）：
- **017 轮产物（拟随候选提交）**：`docs/specs/017-update-plan.md`、`records/2026-10-22-codex-update-plan-comparison.md`、`records/2026-10-22-017-implementation.md`。
- **随引用完整性纳入**：`records/2026-10-21-codex-tools-comparison.md`（017 规格→update-plan 复核→本记录链式引用）、`records/2026-10-22-codex-apply-patch-comparison.md`（project-lessons §7 规划轮条目引用）。
- **016 轮遗留缺口（既有悬空引用，非本轮产生，归属 016，待用户决定）**：`opencode-global-agents-skills-install`、`vault-credential-recovery`、`codex-command-execution-and-process-interaction` 三份被**已提交的** 016 记录/规格引用但未随 016 提交（开工前即未跟踪）——可随本次一并提交修复悬空链接，或单独处理。

## 11. 剩余风险与未运行检查（补验收轮更新）

- 未实测真实模型的计划质量（范围外，017 §7 同判）。
- ~~TUI「缩放后重排」未单独 e2e~~ → 已由 s92 补齐（真实 TIOCSWINSZ 宽→窄→宽、完整性与状态断言、有效性变异证明）。
- Windows ConPTY 仍为 008-A6 既有缺口，未变。
- `cargo clippy`/`cargo fmt` 未安装未运行（如实登记）。
- 兼容性检查工具链（api-extractor 等）不适用：纯 Rust 内部变更，无对外 API/Schema；会话 V1 往返由 e21/s91 验证。
- 016 轮三份未跟踪分析记录对已提交 016 文档构成悬空引用（见 §10.3，待用户决定纳入方式）。
