# 018 用户问询：交付后复核

- 日期：2026-10-23。
- 范围：同步、异步问询；`git diff 6c91818e28ee4af19729197fa730e14565444399...8c60b1042eb86e6f3783d69242f588614bb4e44a`。
- 依据：[018 用户问询契约](../specs/018-user-input.md)，以 §8.10 已确认取舍为准；不把异步仅 TUI、单项未决、旧版重存降级当作缺陷。
- 结论：规格审查发现 3 项行为缺口，规范审查发现 2 项低优先级问题。三项行为缺口均已用真实工具循环复现，**尚未修复**。按用户要求，本轮停在问题报告，不制定或实施“执行中发消息”计划。

## Spec

1. **[P1] 多个等待调用可绕过批末限制。** §8.4 要求每个 `wait_for_user_input` 都必须位于批末，否则入史和执行前整批拒绝。`src/app/src/turn.rs:755` 只检查“存在等待且最后一条不是等待”，因此 `[wait, exec_command, wait]` 被放行。探针先发布问题，在第二次模型请求发出后提交答案，再返回这个批次；中间命令实际执行并输出 `audit-dependent-executed`。此时命令参数来自尚未看过答案的模型请求。现有回归只覆盖 `[wait, exec]`。

2. **[P2] 协议错误会丢掉已受理答案。** §8.5 要求保留已受理答案。`turn.rs:720` 收取答案后，协议解析失败路径调用 `close_pending_async`；该函数在 `turn.rs:608` 取走状态，已有答案时既不保存答案，也不记录关闭。探针在第二次响应返回前提交答案，再返回 `tool_calls:42`；实际报协议错误，但答案事件与关闭事件均为 0。sender 返回错误的路径已单独保存答案，成功收到响应后的协议错误路径仍遗漏。

3. **[P2] 已接入答案后，再等待同轮 ID 会报错。** §8.4 明确要求此时只确认就绪，不重复记录答案。`turn.rs:1013` 在答案批末入史后清空唯一状态；后续批次等待原 ID，在 `turn.rs:925` 返回“当前没有未决问询可等待”。探针确认答案已进入下一次模型请求，再调用等待，实际仍返回错误。现有测试只覆盖答案暂存于同一批的情况。

## Standards

1. **测试未使用独立临时目录。** `src/app/tests/user_input_async.rs:515` 使用 `/tmp/018b-waitorder-<pid>` 并直接删除该路径，不符合 AGENTS.md §3 对独立临时目录与清理的要求。应复用已有临时目录夹具，避免路径碰撞和断言失败后的残留；本轮未修改。

2. **变异证据的恢复哈希不自洽。** [018b-mutations.json](../testing/evidence/018b-mutations.json) 的 M3、M6 中，`original_sha256` 与 `restored_sha256` 不同，却标记 `restored_ok:true`；与其“按原始字节恢复并核对 SHA-256”的方法说明不符。记录注明审查修复后重录，应明确区分候选或补充当次真实原始哈希，不能猜测后改写历史证据。当前源码哈希与恢复哈希一致，不能据此认定源码仍被破坏。

规范轴共 2 项，最严重的是测试隔离不符合项目要求；规格轴共 3 项，最高优先级为等待批次校验绕过。未发现值得新增重构的明确代码味道。

## 验证与证据

在 `/workspace` 执行 `cargo check && cargo test --workspace --no-fail-fast`，退出 0：54 套件汇总 **3171 通过、0 失败、3 原有忽略、0 过滤**。这证明既有回归通过，不能证明上述漏覆盖场景正确。

按 `diagnosing-bugs` 构造三个临时 Rust 探针：使用真实 `run_tool_turn_with_hosts`、固定模型响应和专用回答通道；命令只做 `printf`，不使用真实模型、密钥或用户配置。通过 Cargo 指纹匹配本次 `user_input_async` 测试使用的同一份 `terminal_chat` 与 `serde_json` 库，不修改被测代码。

运行命令：

```sh
/tmp/user-input-audit-8c60b10-probes/probes --test-threads=1 --nocapture
```

| 探针 | 契约断言 | 实际结果 |
|---|---|---|
| `rejects_wait_exec_wait_before_any_dependent_execution` | 整批拒绝，中间命令没有结果 | 回合成功结束，中间命令完成并输出 `audit-dependent-executed` |
| `protocol_error_preserves_already_accepted_answer` | 协议错误仍保留 1 条答案事件 | 协议错误，答案事件 0、关闭事件 0 |
| `later_batch_wait_for_recorded_answer_returns_ready` | 已入史答案对应的等待返回 `ready:true`，答案只保存一次 | 答案已保存一次，但等待返回“当前没有未决问询可等待” |

探针编译成功；两次运行均为 **0 通过、3 断言失败、退出 101**，三个症状输出一致。断言按已确认契约编写，失败即对应待修缺陷，未放宽断言或修改源码。首次临时夹具编译的括号笔误与错误 `serde_json` 构建变体已纠正，不计为产品问题。用户只授权检查后报告，因此未进入修复阶段。

临时源码、完整编译/运行参数及日志位于 `/tmp/user-input-audit-8c60b10-probes/`；上表保留关键输入、断言与结果，不依赖临时日志才能理解结论。

| 证据 | SHA-256 |
|---|---|
| `/tmp/user-input-audit-8c60b10.log` | `a81697bc8c6e7e7dd2df885e2705efb3c04b105c3fd47bf4fadc9b5ce909ab11` |
| `/tmp/user-input-audit-8c60b10-probes/probes.rs` | `3b7ca971fcc0c01e2c901696db88ba0688b69e72ec6aeff3ffe449a0717c6d89` |
| `/tmp/user-input-audit-8c60b10-probes/result.log` | `7cf8325becc40ac516db0af4c1bb0dfe8fb66a2831fecac1d2a72b783316616d` |
| `/tmp/user-input-audit-8c60b10-probes/repeat.log` | `07017d1be033a59563cb8d5b2083e74b6f25cdf10ef4008d2fb33c46c69b3ee3` |

审查结束时 HEAD 仍为 `8c60b10`；`git diff --exit-code 8c60b10 -- src Cargo.toml Cargo.lock` 通过，确认生产代码、仓库测试和依赖未改。本轮未重新运行 clippy、Windows ConPTY 或真实提供方验证，不将它们列为通过。

## 修复轮（2026-10-23，分支 fix/018-user-input-audit）

上表五项问题在同日修复轮闭环，按 TDD 执行：三条探针先移入 `tests/user_input_async.rs` 默认回归（保持输入、事件时机与契约断言），在 `main@8c60b10` 基线确认 **RED（0 通过、3 断言失败，症状与探针一致）**，再修复至 GREEN。未修改契约（§8.4/§8.5 原文即要求这些行为，原实现是缺口）。

| 项 | 修复 | 落点 |
|---|---|---|
| Spec 1 批次绕过 | 批末校验改为「任何非批末位置的等待都整批拒绝」：检查 `calls[..len-1]` 前缀，覆盖 `[wait, exec, wait]`、`[wait, wait]`；`[wait]`、`[exec, wait]` 合法批次保持 | `turn.rs` 批结构校验 |
| Spec 2 答案丢失 | 新增 `close_pending_on_model_error` 统一收尾：已受理答案先恰一次入史，未答项才按 `model_error` 关闭；sender 失败路径与全部协议错误路径（message 缺失/解析失败/content 非法/批超限/wait 非批末/空回复）共用，无双重保存 | `turn.rs` |
| Spec 3 已答再等待 | 任务内 `recorded_answer_ids` 记录本轮已入史答案 ID：再等待命中即返回 `ready:true`，不重问、不重复记录；未知/异轮 ID 仍拒绝，变量每轮新建不跨轮泄漏 | `turn.rs` wait 分支 |
| Standards 1 测试隔离 | `wait_with_trailing_call…` 改用 `common::TempDir` 独立临时目录（pid＋纳秒防碰撞，Drop 自动清理），删除固定 `/tmp` 路径与手动删除 | `tests/user_input_async.rs` |
| Standards 2 变异证据 | git 核实：M3/M6 重录时恢复哈希（`ea5003f0`/`3def01dc`）＝任务提交 `49060bf` 起的真实源码，恢复动作本身正确；声称的 `original_sha256`（`97f70ab8`/`046d5a46`）为审查修复前未提交中间态，git 不可取证。处置：该字段修正为重录时真实原始哈希并加 note 注明历史，原始摘录一字未改；另在修复候选上重放全部受影响变异（见下） | `docs/testing/evidence/018b-mutations.json` |

**新增回归**（默认全集）：`wait_before_batch_end_rejects_whole_batch_before_dependent_execution`（两批次变体＋probe 副作用断言）、`protocol_error_preserves_already_accepted_answer`（有/无答案两分支）、`later_batch_wait_for_recorded_answer_returns_ready`（含跨轮拒绝断言）。变异有效性以可逆回退证明（A1-A3＝逐项回退修复，M1-M6＝原六项在候选上复验），**九项全部断言失败（101，非编译失败）检出、恢复后哈希与候选一致（turn.rs `924d86d2…`、tui_host.rs `3def01dc…`）、复绿**；明细与哈希见 [018b-mutations.json](../testing/evidence/018b-mutations.json) `audit_fix_replay` 节。

审查双轴复核后三处调整：wait 就绪判断改用 `Vec::contains`；删除新测试中不可达的探针清理行；补跨轮 ID 拒绝断言（守护 `recorded_answer_ids` 不跨轮泄漏）。历史证据摘录恢复原文，仅 M3/M6 的 `original_sha256` 与 note 按核实数据修正。

安全面复核：批次执行绕过即「依赖未确认答案的命令预先执行」，已由 probe 断言堵住；答案丢失与错误归属分别由回归覆盖；既有安全门禁 `user_input_security` 9 项与 `e2e_acceptance` 122 项（含 S103-S106）修复后通过。原交付与误记事实保留不改；先前 3171/0/3 全绿结论不因本次修复伪造成包含新回归。

最终验证（分支 `fix/018-user-input-audit` 候选，2026-10-23）：容器 `/workspace` `cargo check && cargo test --workspace --no-fail-fast` 退出 0，54 套件 **3174 通过、0 失败、3 原有忽略**（相对基线 3171 净增 3＝三条审计回归；日志 `/tmp/018-audit-fix-final-verify.log`）；`git diff --check` 通过。lint（固定 1.99.0 组件直调，同阶段 B 方法）：clippy `-p terminal-chat --all-targets --locked` 通过（警告均为既有行，本轮新增代码无新警告）；`tests/user_input_async.rs` rustfmt `--check` 通过；`src/app/src/turn.rs` 未全量格式化为阶段 B 交付既有状态（本轮新增行未出现在 rustfmt 差异中），保持不重排既有代码。未运行：Windows ConPTY、真实提供方（边界与 018-B 相同）。

## 收尾轮（2026-10-23，同分支）

1. **历史证据处置修正**：上轮曾把 M3/M6 的 `original_sha256` 改写为重录时的恢复哈希——该推断不成立（git 提交哈希只能证明对应版本内容，不能替代变异前工作区快照）。已从基线恢复 M3/M6 原始哈希与失败摘录（M1-M6 全部证据字段与基线逐字段一致），并以 note 明确标注：original 与 restored 不自洽、原始中间态无法核验、**不计为有效恢复证明**，仅按原样保留为历史事实；有效性证明以 `audit_fix_replay` 节为准（九项真实重放，原始/恢复哈希逐项一致）。不猜改历史数据。
2. **双问并存场景回归**：新增 `wait_answered_id_keeps_second_pending_independent`——同轮 Q1 已答入史、Q2 未决时：wait Q1 返回 `ready:true` 且不消费 Q2 通道（下一等待通知必为 `pub2`）、Q2 保持未决；wait Q2 阻塞等待 Q2 自己的回答；两份回答各恰一次入史、无关闭事件；最终请求按序携带两份答案。当前实现直接通过（就绪 guard 先于未决匹配，无缺陷暴露，实现未改、候选哈希 `924d86d2…` 不变）；以删就绪 guard 的可逆变异验证测试敏感性：断言失败（101）检出、恢复后复绿。
3. **规格引用**：[018 规格 §9](../specs/018-user-input.md) 追加审计修复轮段（复现、修复、实测与证据链接）；契约与原交付历史未改。

收尾轮验证：`user_input_async` 19/19（含双问新回归）。最终全量（收尾候选，`cargo check && cargo test --workspace --no-fail-fast`）退出 0，54 套件 **3175 通过、0 失败、3 原有忽略**（相对基线净增 4＝四条审计回归；日志 `/tmp/018-audit-fix-wrapup-verify2.log`）；`git diff --check` 通过；rustfmt 对 `tests/user_input_async.rs` 通过。首次全量运行中既有性能用例 `session_perf::perf_tiers_baseline_driven` 因并行 I/O 抖动超阈值一次（56ms>50ms，该路径本轮未触碰），定向复跑与重跑全量均通过，单列为环境敏感项而非本轮问题。

## 已批准的本地交付（2026-10-23）

用户批准后执行。任务提交：`1d63698`（`fix: 018 修复异步问询审计缺陷（批末绕过/协议错误丢答案/已答再等待）`），父提交 `8c60b10`，范围恰为核定 7 文件（651 插入／31 删除）。`project-lessons.md` 与外部 019 规划存在混合改动：按 hunk 区分，仅 018 两行入提交，019 段留在工作区。合并采用引用快进（`main` 自 `8c60b10` 前进至 `1d63698`，先确认祖先关系，等价 ff-only 且不触碰工作区——直接 `git switch` 会因该工作区修改被拒）；核验：`1d63698 ∈ main`、main 树与任务提交逐字节一致、`git diff --check 8c60b10 main` 通过、`src` 与已验证候选逐字节一致（`turn.rs` SHA-256 前缀 `924d86d2`）且依赖未动，故沿用终验 **3175/0/3** 不重复全量。核验后删除本地任务分支（`git branch -d`，实际指向 `1d63698`）。外部修改完整保留于工作区：`docs/records/2026-10-21-codex-tools-comparison.md`（019 引用）、`project-lessons.md` 019 段、`docs/specs/019-view-image.md`、`docs/records/2026-10-23-view-image-plan-research.md` 及三份既有无关未跟踪记录。未推送、未发布、未改写既有提交。
