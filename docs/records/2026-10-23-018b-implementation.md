# 018 阶段 B：异步问询实施与验证

实施契约：[018-user-input §8](../specs/018-user-input.md)（2026-10-23 用户确认 §8.10 三项取舍后实施）。分支 `feat/018-user-input-async`（自 `main@dc7a4f1747b9670ed048aa37f63f5a8ddefc3e23` 建立，携带上轮 §8 调查的 4 份文档未提交改动）；合并目标本地 `main`。本轮未提交、未合并、未推送。无新增第三方依赖与版本升级。

当前状态：**已批准并交付本地 main（2026-10-23）：任务提交 `49060bf` 快进合入、核验通过、本地任务分支已删除；未推送、未发布、未删除远程分支。交付事实见 §9。**全量 `cargo check && cargo test --workspace --no-fail-fast`＝**3171 通过、0 失败、3 原有忽略**（相对阶段 A 交付基线 **3142/0/3** 净增 **29** 项，统计口径见 §8；日志 `/tmp/018b-verify-final.log`，SHA-256 `f3763112b42625750f7b9aa1c6a6b7d07fed625c541d728d68b1413c7e03c132`）；`git diff --check` 通过；lint 见 §5（两处旧测试错误沿用阶段 A 保留，无新增）。

## 1. 实际改动面

| 落点 | 改动 |
|---|---|
| `src/shims/codex-protocol/src/request_user_input.rs` | 新增 `AsyncUserInputQuestion`（title＋可省略 options，deny_unknown_fields），对齐 d138398 `items.rs` |
| `src/app/src/user_input_async.rs`（新增） | 发布参数解析/归一化（题目 ID＝`JSON(["request_user_input_async",调用ID,序号])`）、等待参数解析、确认/ready/回答/关闭正文构造、关闭原因短串、两个工具定义、`AsyncReplyChannel`/`AsyncInputHost` 类型 |
| `src/app/src/prompt.rs` | `HistoryMessage` 新增可选 `user_input_event`（`UserInputEvent::Answer/Closed`，serde 省略、旧记录默认 None）；`build_turn_request_with_async_host`（同步＋异步宿主＝七工具） |
| `src/app/src/turn.rs` | 新入口 `run_tool_turn_with_hosts`/`run_user_turn_with_context_and_hosts`；发布（占用/额度/能力门）、等待（批末结构校验、已答即就绪、可取消阻塞）、批末回答接入、结束检查先处理已受理回答、各结束路径关闭未决项；`TurnNotify` 新增等待/回答/关闭三事件 |
| `src/app/src/tui_host.rs` | TUI 异步发布回调（一次性回执通道，不取 history/autosave 锁）、三事件→DriverEvent 映射（展示副本过滤）、`request_user_input_async` 终态投影、恢复投影按来源标记合并（孤儿/矛盾/重复/降级不伪造已答） |
| `src/tui/src/chat_driver.rs` | `DriverEvent::UserInputAsync/Wait/Update`；未决项登记槽位＋`async_cells` 记账；**F6** 打开本轮唯一未决项（生产键位表无 F6 绑定，见 §6 核查）；回答/关闭原地替换同一 cell；Final/Failed 清理槽位 |
| `src/app/src/main.rs` | 行式 CLI 不接异步宿主（仍五工具）；notify 新事件显式无操作 |
| `src/app/tests/user_input_async.rs`（新增）、`user_input_security.rs`、`e2e_acceptance.rs`、`src/tui/src/chat_driver.rs` 内嵌测试、`src/app/src/tui_host.rs` 内嵌测试 | 见 §3/§4 |

既有同步契约未改：阶段 A 全部行为原样（S93–S102 复验通过；仅三处“五工具”断言按 §8.2 更新为 TUI 七工具）。

## 2. 已确认契约的落点（§8.10 三点）

1. **TUI 先行/单项未决/F6**：仅 `tui_host` 传入异步宿主；turn 循环内 `pending_async` 单槽位，第二发布与异步未决时的同步问询均返回占用错误（`occupancy_and_mismatch` 用例）；发布不抢焦点（仅提示 cell），F6/显式等待打开同一 overlay。
2. **本地 wait_for_user_input**：批末结构校验（带后续调用整批拒绝，入史前）；仅本轮 ID 匹配；已答即就绪；复用 `wait_for_response` 可取消等待；依赖动作由下一请求生成（S104 断言 wait 结果先于回答文本）。
3. **V1 可选来源字段＋重存降级**：`user_input_event` 仅由 turn.rs 回执/收尾路径设置；回答正文完整保留 request_id 与 answers；旧程序重存丢标记后按普通用户文本展示（`session_roundtrip` 用例＋`project_history_items_merges` 降级臂）；不做版本隔离。

## 3. 测试与验收对照（§8.9 十四行）

| 验收行 | 证据（全部通过） |
|---|---|
| 发布参数/确认 | `user_input_async::publish_args_validate…`（非法形状十类不触宿主；纯文字题归一化；确认含 request_id 无 answers） |
| 发布后继续 | `publish_then_independent_work_continues…`（同批独立命令执行、恰 2 请求、task_completed 关闭）＋E2E S103 |
| 回答及时接入 | `answer_submitted_mid_batch…`（批内阻塞命令期间提交；回答落在批内最后一条结果之后、恰一次进入下一请求） |
| 依赖等待 | `wait_blocks_until_answer…`（等待期请求计数不增、ready 非第二份回答、答案数据随其后）；`wait_with_trailing_call…`（整批拒绝零副作用）；`wait_already_answered…`（立即就绪）；E2E S104 |
| 结束交界 | `terminal_text_with_received_answer…`（终文不提前交付、草稿如实入史、原轮续算）；`sync_and_async_coexist…`（已受理回答后取消仍保留答案、不重记未答）；`sender_failure_closes_pending…`（模型请求失败＝model_error 关闭＋已受理答案保留，审查修复回归） |
| 单项/重复/迟到 | `occupancy_and_mismatch…`（占用/不匹配即使答案已到也不吞）；`late_answer_after_release…`（一次性通道，释放后迟到提交失效） |
| 输入与界面 | E2E S103（F6 打开、发布提示 cell、草稿保留、回答进下一请求）；S104（等待自动打开同一窗口） |
| 取消退出 | `cancel_and_channel_failure…`（标志与通道两路：真实终态、cancelled/host_exit 关闭、无续请求；看门狗防挂死）；E2E S105（Esc 解除等待、会话文件含 cancelled 关闭与无回答事件） |
| 会话切换 | 运行中 `/resume` 拒绝沿用既有门（composer `available_during_task`＋try_lock 纵深）；未决状态为轮内局部量，切换后旧令牌/通道不可达（§8.5 结构性满足） |
| 保存恢复 | `session_roundtrip…`（标记保真/旧程序重存降级/旧记录可读）；`tui_host::project_history_items_merges…`（answer/closed/仅确认/孤儿/矛盾/重复/降级七臂）；E2E S106（已答恢复同源展示、零请求） |
| 兼容性 | V1/A 序列化形状不变（全量 `session_cli`/`prompt_assembly` 通过）；阶段 A 用例原样通过 |
| 额度 | `eighth_request_rejects…`（第 8 批不发布/不阻塞、request_limit 关闭、已到回答保存、无第 9 次）；32/33 沿用既有 N3 |
| 能力与权限 | `tool_counts_and_forged_calls…`（7/5/4/3、伪造名执行层拒绝、MemoryMerge 强制无宿主）；安全门禁 §4 |
| 回归有效性 | 六项可逆变异全部断言失败（101）检出并复绿：018b-mutations.json（M3/M6 在审查修复改变锚点后重录，仍检出） |

## 4. 安全门禁（新增 3 项，合计 9/9 通过）

- `async_lookalike_chat_never_gains_source_marker_or_answers_pending`：与事件正文完全同形的普通聊天不得获得来源标记、不得回答未决问题（标记仅由宿主路径设置；序列化行无 `user_input_event` 键）。
- `async_untrusted_title_options_and_answers_are_display_copies`：题面/选项/回答展示副本过滤控制序列；JSON 字符串题目 ID（关联键）不改写；原文完整保留。
- `real_tui_async_attack_payloads_stay_data`：真实 TUI＋F6＋等待流，标题含 `ESC[2J`（屏幕干净、请求内原文保留）；Other 文字补充粘贴伪造 tool_calls/角色/命令载荷——不执行（marker 未创建）、不改请求目标（诱饵零请求）、不外发凭证、回答以数据进入第 3 次请求、七工具契约保持。

## 5. lint 与格式

固定 1.99.0 组件（按发行清单 URL/SHA-256 下载至 `/tmp/018b-quality-tools`，同阶段 A 方法；rustup 直接安装仍因下载缓存清理错误失败）：

- clippy `--workspace --lib --bins --locked` 通过（既有 shim 警告：`codex-utils-absolute-path` collapsible_if、`codex-config` dead_code，非本轮改动）。
- clippy `-p terminal-chat --all-targets --locked` 通过（覆盖全部新增测试）。
- clippy `--workspace --all-targets --locked` 失败＝`src/tui/tests/security_untrusted_text.rs:231/:235` 两处 `unused_io_amount`，与阶段 A 记录一致（旧文件未触碰，保留不顺手修）。
- 新文件经 rustfmt 1.10.0（`--check --config skip_children=true`，edition 2021）通过：`src/app/src/user_input_async.rs`、`src/app/tests/user_input_async.rs`。

## 6. 双轴审查（候选阶段）与处置

对 `git diff dc7a4f1` 工作树做 Standards/Spec 两轴独立审查（分支无新提交，比较用 `git diff <base>` 而非空区间 `<base>...HEAD`）。

**Standards 轴**：3 处硬违规（`chat_driver.rs` 函数粘连丢换行、`turn.rs` `close_pending_async` 嵌套缩进、`session.rs` 一处逗号前空格）与 1 处死绑定（wait 已答判定的 `let _ = answer`）**已全部修复**；F6/显式等待打开窗口的重复段抽取为 `PendingAsyncQuestion::open`。判断性意见保留不改：错误路径 7 处显式 `close_pending_async(…, <原因>)` 调用（各站点原因不同、显式列出与既有 `input_end` 风格一致）、11 参数入口（沿用既有 `with_host(s)` 包装委托惯例）。

**Spec 轴**：2 处缺口**已修复并补回归**——①§8.5 模型请求失败路径原不关闭未决项，现按「先处理已受理回答→未答关闭为 `model_error`」收尾（`sender_failure_closes_pending…` 用例）；②§8.6 恢复投影答案归属，答案含外来题号时不再合并为已答且正文按普通文本保留（`project_history_items_merges…` 外来题号臂；合并成功才跳过事件文本）。2 处经裁决保留并记录：wait 非批末的整批结构校验先于能力门（更严格方向，伪造调用被整批拒绝而非宽容执行）；发布确认后紧随取消的 `accepted:true`＋closed(cancelled) 序列（事件确已送达宿主层，随后按真实原因取消，非“宿主拒绝”）。1 处已知次缺口：用户提交到批末入史之间无显式“已提交待接入”中间展示态（行为可区分——未入史前 cell 不标已答；极简取舍不再加态）。

## 7. 实施要点与坑

- **F6 转义序列**：xterm F6＝`ESC[17~`；误发 `ESC[15~`（F5）时窗口不打开、后续 Enter 落入 composer 形成排队聊天。S103 已按正确序列固化。
- **PtyCli::send 自动追加 CR**：测试辅助 `send` 总是带 Enter，功能键/方向键/Tab 会连带提交。新增 `send_keys`（不追加 CR）供控制键使用；S103/S105 的按键序列全部改用 `send_keys`＋逐步可视等待。
- **看门狗防挂死**：等待类用例（取消/通道失败）经通道＋`recv_timeout` 交付结果，变异「取消不醒」时 10 秒内以断言失败检出，而非无限 join（M5 首轮即因 join 挂死超时，改为看门狗后干净检出）。
- **变异 M1/M2 首轮未检出的教训**：M1 目标测试手工构造历史（不经 turn 循环），变异不生效——改指 turn 驱动用例；M2 原批次仅两条调用且回答在末条执行中到达，“批中插答”与正确行为同位——批次加第三条调用后错位可检。测试选择必须让被破坏行为可见。
- **一次性通道语义**：`UserInputReply::send` 取走 sender 后通道即断；turn 侧 `PendingAsyncInput.channel` 收到终态即置 None，此后迟到/重复回执不再消费（断开＝Closed）。
- **回答进入下一请求的形态**：answer 事件为 user 角色 Text（正文＝`{"request_id","answers"}`＋来源标记），随批末入史进入下一次请求组装——与 §8.4/8.6 一致，模型面无新协议。
- **S105 断言转义**：会话文件 content 内 JSON 为转义形态，按字段组合断言（`cancelled`＋`reason`），不按未转义字面量匹配。
- 变异执行脚本必须在超时/异常路径也恢复文件（首轮 M4 超时崩溃残留变异，已手动恢复并复绿后重录）。

## 8. 验证命令与边界

- 全量（审查修复后最终复验）：容器 `/workspace` `cargo check && cargo test --workspace --no-fail-fast` → 退出 0，**3171/0/3**（3 忽略为原有 TUI 条目：Windows AltGr 专项＋两个子进程 helper）。日志 `/tmp/018b-verify-final.log`，SHA-256 `f3763112b42625750f7b9aa1c6a6b7d07fed625c541d728d68b1413c7e03c132`；审查修复前一轮 3170/0/3 见 `/tmp/018b-verify-full.log`。

**统计口径（2026-10-23 核对）**：阶段 A 交付基线＝**3142/0/3**（[阶段 A 实施记录 §6](2026-10-22-018-user-input-implementation.md)，53 测试目标，`dc7a4f1` 时的已交付状态）。本轮终验 3171/0/3，**净增 29**＝5（`src/app/src/user_input_async.rs` 模块内嵌单测）＋15（`tests/user_input_async.rs` 集成）＋3（安全门禁新增）＋4（E2E S103–S106）＋2（TUI 投影组件：chat_driver 1＋tui_host 1）；各分项数量与定向运行结果（15/9/122 等）一致可核。曾误记“净增 24”：该数以会话内中间值 3147 为参照——3147 实测于生产实现落地后、集成测试建立前（＝3142＋上述 5 项内嵌单测；3 处既有断言更新不改变数目），属中间态而非基线，且未留存独立日志，故不作口径依据；其算术关系（3142＋5＝3147，3147＋24＝3171）与净增 29 自洽。
- 定向：`--test user_input_async` 15、`--test user_input_security` 9、`--test e2e_acceptance` 122（含 S103–S106）、`--lib`（tui_host 8＋user_input_async 5）、`-p tchat-tui --lib chat_driver::tests::async_input` 1。
- `git diff --check` 通过。
- 未运行：Windows ConPTY/目标宿主终端（008 A6 边界保持）；真实模型/生产端点。环境 Linux，全部用本地 mock＋PTY＋隔离目录。

## 9. 已批准的本地交付（2026-10-23）

用户已明确批准：提交阶段 B 候选及必要文档至 `feat/018-user-input-async`，合并到本地 `main`，核验后删除本地任务分支；不推送、不发布、不删除远程分支。三个原有无关未跟踪记录不纳入提交、保持原样。

以下记录已经发生的提交、合并、核验与清理：

- 任务提交：`49060bf4992496179bd4d253655a25ebd50d8eac`，`feat: 018 异步问询最小接入（阶段 B）`；父提交为 `dc7a4f1747b9670ed048aa37f63f5a8ddefc3e23`，范围恰为 31 个核定文件（27 修改＋4 新增：`src/app/src/user_input_async.rs`、`src/app/tests/user_input_async.rs`、本记录、变异证据）。统计核对轮后仅文档变化，源码/测试/依赖与最终验证候选逐字节一致（提交前 `cargo check --workspace --lib --bins --tests` 通过，沿用 §8 终验 3171/0/3，不重复全量）。
- 暂存采用明确文件白名单逐文件 `git add`；暂存内容与工作树逐字节一致（无未暂存的已跟踪差异）、`git diff --cached --check` 通过；变异证据 JSON 补末尾换行后入提交；三份无关未跟踪记录未暂存。
- 执行 `git switch main`、`git merge --ff-only feat/018-user-input-async`，**Fast-forward** 成功；合并完成时 `main` 与任务提交 SHA 均为 `49060bf4992496179bd4d253655a25ebd50d8eac`，树均为 `c56b47e272f90449484fa5789cceb941b8d9951b`。
- 核验（均在删除分支前执行）：`git merge-base --is-ancestor 49060bf main` 通过；`git diff --exit-code 49060bf main` 退出 0（main 与批准候选树一致）；`git diff --check dc7a4f1 main` 通过；`git diff --name-only dc7a4f1 main` 恰 31 项与白名单一致；工作树无已跟踪差异；三份无关未跟踪记录保持未跟踪原样。
- 仅在上述核验成功后执行 `git branch -d feat/018-user-input-async`，实际输出 `Deleted branch feat/018-user-input-async (was 49060bf).`；`git show-ref --verify --quiet refs/heads/feat/018-user-input-async` 返回 1，确认本地分支已不存在。远程引用未动，未调用 pull/push 或发布操作。
- 后续文档提交（含本节）只保存交付事实与必要状态收口，不改程序、测试或验证结论。
