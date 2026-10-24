# 021 实施记录：send_message_to_user_async 与运行中接续

- 日期：2026-10-23。
- 交付：用户已批准（2026-10-24）——实现、测试、规格、证据及相关段落随任务提交快进合入本地 main；未推送、未发布。基线 `main@6b8f3a310c0a0748239641b127c6fdb46ec8d2db`。
- 规格：[021 接入计划](../specs/021-send-message-to-user-async.md)；计划依据与源码核对见[调研记录](2026-10-23-send-message-to-user-async-plan-research.md)。
- 范围：实现消息工具＋运行中普通输入接续＋显示/恢复链路＋测试与文档；未修改无关模块。

## 1. 实现落点（§3.2 对照）

| 模块 | 内容 |
|---|---|
| 新增 `src/app/src/message_to_user.rs` | 工具定义（上游 d138398 描述/Schema 映射，`TOOL_NAME` 常量）、严格单 `message` 解析（deny_unknown_fields＋trim＋空白拒绝）、`{"accepted":true}`、实时/恢复共用 `sent_message_from_record`、`failure_line` |
| `catalog.rs` | `ModelInfo.experimental_supported_tools` 可选数组（逐字段覆盖）；`supports_send_message_to_user` 精确名匹配 |
| `prompt.rs` | `build_turn_request_with_async_host` 增加 `send_message` 快照参数；声明与问询开关独立 |
| `turn.rs` | `SteerInput`/`SteerSeal`/`SteerChannel`/`TurnExtras`；`TurnNotify::SteerIngested/SteerRestored`；`run_tool_turn_with_hosts_extras` 核心循环（send_message 分派、批末接入、seal 终文竞态、出口 close＋交还）；新入口 `run_user_turn_with_context_and_caps`（旧入口签名不变零改动） |
| `tui_host.rs` | `SteerSlot`（短锁受理资格）＋`TurnSteerChannel`（seal/close 同临界区原子）；`submit_steer`（宿主生成 id）；spawn 同步建通道；notify 映射 Steer*；ToolBegin 抑制＋ToolEnd 失败 Info；恢复投影（成功→`AssistantMessage`、失败→failure_line 文本） |
| `main.rs` | 行式：能力快照接 caps 入口；消息即时输出（终文前）；失败原因；Steer* 不可达 no-op；steer=None |
| `src/tui/src/chat_driver.rs` | `DriverHooks.submit_steer`；`DriverEvent::SteerIngested/SteerRestored`；`HostHistoryItem::AssistantMessage`（AgentMarkdownCell 同管线）；Enter 运行中＝尝试接续（Err 回执转排队）；接入预览；SteerRestored 入队＋暂停 |
| 会话/记忆 | 零持久化扩展（调用＋结果即 V1 记录）；恢复投影在 tui_host 层完成 |

## 2. TDD 与验证

- RED：`src/app/tests/message_to_user.rs` 先行（未实现 API 编译失败为 RED 证据）；随后 18/18 GREEN，最终 21/21（含混合批与安全用例）。tdd 技能流程核对：测试全部落在公共接缝（turn 完整循环/宿主 API/真实进程），断言以独立事实来源为准（上游固定文案 `{"accepted":true}`、`message must not be empty`、请求字节），无实现耦合/同义反复；L1 参数层为批量先行（登记取舍），循环行为层按失败逐个驱动修复。
- 单测：catalog `experimental_supported_tools_overlay_and_capability`（35/35 文件内）、tui_host `steer_channel_seal_close_atomicity`/`tool_end_item_send_message_shapes`/`project_history_items_restores_send_message`（12/12）、prompt_assembly `s17`（16/16）。
- L3：e2e_acceptance 新增 s11b（接续进当前轮）、s11c（取消恢复→暂停→恰一次重放）、s21（管道顺序＋trim＋无注入）、s21b（关闭态拒绝）、s21c（/resume 零重发）、s21d（TUI 显示＋任务仍运行＋草稿保留）、s21e（终端过滤）；既有 s11/s84/s87/s96/s110 按规格 §2.3「有意修订」改用显式排队键（Tab），全部 138/138。
- 全量（最终候选，含 A07 补证用例）：`cargo check` 0 错误；`cargo test --workspace --no-fail-fast`＝**56 目标 3310 通过 / 0 失败 / 3 忽略**（日志 `/tmp/021-verify-final.log`，SHA-256 `7027794d50ac634763270df93b17dc43a17d8b9c1ab6c7c06a2876fa7f394dc5`；基线日志 `/tmp/021-baseline.log`，SHA-256 `c42402ccd1b1160a023f8cc86e259a334a8bbd5e05a56d6128d5b13c6c3526d0`）。clippy/rustfmt 工具链下载失败（离线），两项未运行、不报通过。

### 统计核对轮增量变更（2026-10-24）

本轮仅新增 1 个测试（A07 直接用例，`tests/message_to_user.rs`）并修订文档；生产代码零变化（两轮变异实验均逐字节恢复，`turn.rs` SHA-256 `c9c3e24d…` 与候选一致）。增量审查：A07 用例沿用既有 `run_turn_with` 接缝、断言独立事实来源（accepted 逐字字面量、上限固定文案、请求计数）、无实现耦合；安全复验不适用（无生产代码变化），全量复验已执行（上方最终统计）。

### 统一口径（2026-10-24 核对，隔离副本实测）

- **基线必须对应 `6b8f3a3`**：`git worktree add /tmp/021-baseline 6b8f3a3`（不触碰脏工作区）后以显式 `--target-dir /tmp/021-baseline-target` 全量实测＝**55 目标 3276 通过 / 0 失败 / 3 忽略**（日志 `/tmp/021-baseline.log`）。此前记录的「基线 3171」实为 018-B 交付提交 `49060bf` 的状态，不含 020 交付的 105 项，不作为本基线；020 记录「3276/0/0」的通过数正确但忽略数应为 3（该记录的统计误差，不属于 021 修复范围，仅在此勘误说明）。
- **净增 34**＝`tests/message_to_user.rs` 22＋catalog 内嵌 1＋tui_host 内嵌 3＋prompt_assembly 1＋e2e_acceptance 7（s11b/s11c/s21/s21b/s21c/s21d/s21e）；e2e 子集 132→139。与逐目标统计（55→56 目标，tchat_tui lib 恒为 2163 通过＋3 忽略）完全自洽。
- **3 项忽略**（全部位于 tchat_tui lib，基线 6b8f3a3 已有，非 021 引入，对验收无影响）：
  1. `bottom_pane::textarea::tests::altgr_ctrl_alt_char_inserts_literal`——代码标注「AltGr modifier only applies on Windows」，Linux 无法行使该行为；
  2. `tui::owned_screen_tests::owned_screen_lifecycle_child`——子进程 helper（`#[ignore = "spawned by the owned-screen lifecycle test"]`），由非忽略父测试以环境变量守卫真实派生并驱动；
  3. `tui::panic_tests::caught_panic_pty_child`——同类子进程 helper，由父测试派生。
- 测试目标数口径：`cargo test --workspace` 输出的 Running 段数（55/56），通过/失败/忽略为各目标 `test result:` 小计之和，e2e 子集单列 `tests/e2e_acceptance.rs` 目标，无重复计数。

## 3. 变异验证（隔离副本可逆，逐项恢复并 `sha256sum -c` 校验）

| # | 变异 | 检出测试 |
|---|---|---|
| M1 | 通知被当 Final（tui_host 改发 Final） | s21d（消息后任务必须仍在运行——首版未检出，加受控闸门断言后检出） |
| M2 | 移除 trim | parse_arguments 测试＋s21 |
| M3 | 移除空白拒绝 | parse_arguments 测试 |
| M4 | 删除实际消息输出（CLI） | s21 |
| M5 | 额外注入 assistant 正文 | real_loop_message_does_not_end_turn |
| M6 | Enter 仍排下一轮（强制回执失败） | s11b（恰两次请求） |
| M7 | 批中插 user（批执行前接入） | steer_enters_history（双调用批顺序） |
| M8 | 接续漏附件 | steer_with_image_enters_history_with_payload |
| M9 | 终文忽略已接收输入 | final_text_race_continues_turn |
| M10 | 能力隔离失效（分派忽略开关） | closed_state_forged_call |

10/10 检出；恢复后哈希逐文件 OK，全量复跑 3310/0/3（含 A07 补证用例）。

### A07 直接用例与检出力（2026-10-24 补证）

- 新增 `message_tool_executes_on_eighth_request_then_cap_without_ninth`（tests/message_to_user.rs）：响应 1..7 为 exec_command、第 8 响应调用 `send_message_to_user_async`——断言 ToolEnd 通知携带 accepted、调用与结果按契约入史、`bodies.len()==8`（零第 9 次请求）并以「已达本轮模型请求上限（8）」收尾。实现即时满足（验收补证，非缺陷修复）。
- 检出力证明（可逆变异）：上限由**两处闸门**共同保障（循环顶 `requests >= MAX` 与批末 `requests == MAX`）——单点变异（`>=`→`>` 或批末 `==`→`usize::MAX`）均被另一处兜住而未被检出（登记为纵深防御事实）；同时禁用两处后本用例立即失败（第 9 次请求触发响应耗尽）。恢复后逐字节哈希核验一致。

### A06 窄屏覆盖（复用覆盖，引用既有用例）

消息 cell 与最终回复构造于**同一生产构造器**：`AgentMarkdownCell::new`（`#[cfg(test)]`）委托 `new_with_inline_visualizations(source, cwd, None)`（`history_cell/messages.rs`），与 `chat_driver.rs` 的 `project_tool_item(AssistantMessage)` 及 `finish_stream`（最终回复）逐参一致；`display_lines → render_lines → markdown_render` 为同一渲染路径。窄屏行为由既有用例覆盖并复跑通过：

| 用例 | 断言 |
|---|---|
| `history_cell::tests::agent_markdown_cell_narrow_width_shows_prefix_only` | 宽 2：仅渲染 `• ` 前缀（不溢出、不 panic） |
| `history_cell::tests::wrapped_and_prefixed_cells_handle_tiny_widths` | 宽 1..=4：`display_lines` 恒非空（agent markdown/user/reasoning 同批） |
| `markdown_render::markdown_render_tests::wrapped_list_item_is_separated_from_next_sibling` | 宽 24：折行与列表间距契约 |

不新增重复用例；PTY 实显（s21d，宽 200）与 s21 管道输出为 L3 佐证。

## 4. 安全门禁

- 输入来源：模型可控消息——展示副本过滤（s21e：stdout 零 ESC/CR/BEL；tui_host 转换层过滤单测），模型面参数原文逐字保留（security_raw：006 原样契约，`\u001b` 转义形态进请求）。
- 跨轮回执：宿主生成提交 id；SteerIngested/SteerRestored 按 id 恰一次；封口后提交得"当前轮已结束"回执（steer_channel 原子性单测）；取消/额度/合并轮交还恢复不丢失。
- 工具隔离：MemoryMerge 声明＋分派双层拒绝（真实合并轮测试）；关闭态伪造调用拒绝（L2＋s21b）；合并轮误传通道防御性封口＋交还（审查整改）。
- 终端过滤：行式 say 统一过滤；TUI AssistantMessage 过滤后进 Markdown 管线（与最终回复同一不可信文本路径）；失败行过滤。
- 既有权限：通知与接续不触碰 origin/凭证门；不授予执行权限（s21e 零副作用断言：恰两次请求、无文件写入）。

## 5. 审查处置（双轴）

- Standards：无硬违规。整改：`tool_definition()` 改用 `TOOL_NAME` 常量；Final/Failed 防呆清空块提取 `clear_steer_preview`。保留（判断级）：Steer 三结构跨 crate 各一份（边界转换）、长参数链（既有惯例延续）、`SKIPPED_BY_CANCEL_MESSAGE` 跨模块常量（既有共享常量模式）。
- Spec 整改：①TUI 成功消息展示副本补 `sanitize_for_terminal`（§2.1，实时/恢复同一转换）；②驱动 SteerRestored 恢复行为补 e2e s11c；③合并轮通道防御封口＋`merge_turn` 原因标签。保留决策：SteerRestored 恢复为待处理队列＋暂停（§2.3-6「恢复为待处理完整消息并暂停自动发送」的字面实现；020 退草稿语义仅适用于入史前拒绝的当轮提交）。
- 保留说明：A06 窄屏 Markdown 复用最终回复同一 Markdown 管线（既有快照覆盖），未新增窄屏用例；A07 消息工具无第 8 请求特例（与一般工具同路，通用上限测试覆盖）。

## 6. 坑记录（新增）

- 020 教训再现：PTY 测试中「文本＋按键」单次写入会落入粘贴突发窗把 Tab/Enter 吞掉——必须 sleep＋分离发送（s11/s84/s87/s96 排队键改造时复现，按 020 记录处置）。
- `Option<&mut dyn Trait>` 不能用 `as_deref_mut` 传入「按值收 Option」的辅助函数（对象生命周期把重借孔回升为调用者生命周期）——改为传 `&mut Option<…>`。
- `load_manual` 缺省创建副作用＋ view_image 11b 未隔离环境：全量运行会在 `src/app/config/` 留下缺省 manual.json（019/020 既有问题，非 021 引入；按「只提醒不修他人测试」处理，本轮已清理产物）。
- serde_json 对 `\u001b` 的双重转义链：mock 响应里 arguments 是「含转义形态的 JSON 字符串」，断言须匹配转义形态而非原始控制字符；且请求投影的 arguments 位于 `tool_calls[].function.arguments`。
