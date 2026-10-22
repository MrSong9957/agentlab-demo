---
title: 016 候选测试加固与局部精简——实施与验证记录
date: 2026-10-22
status: 三轮（测试加固轮）完成，全量验证通过，已批准交付（2026-10-22 合并入 main）
tags:
  - Agent
  - 测试加固
  - 016
branch: feat/016-process-sessions
base: 9345b5299b204335f5c6dbfe3909cedecd2bf333
---

# 测试加固与局部精简记录（2026-10-22 三轮）

> 按 [016 §6.1](../specs/016-process-sessions-plan.md#61-测试加固与局部精简执行准备2026-10-22) 及五份规格的"测试加固与局部精简补充"条款执行。本轮**零生产行为改动**（catalog.rs 改动位于 `mod tests`、proc_session.rs 为缓存单测重写，其余为集成/e2e 测试文件）；实施基线快照 `/tmp/tchat-hardening-exec-baseline.json`（HEAD `9345b52`，含未提交与未跟踪候选）。

## 1. 七处加固（全部 GREEN＋反证检出）

| # | 落点 | 实测 | 反证（隔离副本＋独立 `--target-dir`） |
|---|---|---|---|
| 1 | [security_untrusted_text.rs](../../src/tui/tests/security_untrusted_text.rs)：A7 原始字节证据 | 新增 `CapturingParser`（先记录后转发）＋两层检查：引入符直接组合＋CSI/OSC/DCS/SOS/PM/APC 完整序列范围扫描（唯一标记 `ATK7QZ`）；`a7_full_pipeline` 逐格写入捕获＋屏幕双查，`a7_byte_level` 经真实 `Terminal::draw`→`CrosstermBackend` 捕获，正向对照断言合法 CSI 确实存在（不误报） | **H1**：writer 边界注入真实形态 `\x1b]52;c;ATK7QZ…\x07` → 序列扫描层 RED（vt100 消费该序列、屏幕断言不触发——证明原始字节检查不可替代）；还原 GREEN |
| 2 | [memory_store.rs](../../src/app/tests/memory_store.rs)：`m1_two_store_instances_serialize_via_busy_timeout` | 独立连接 `BEGIN IMMEDIATE` 持写锁（屏障/通道协调）；被测写者（真实 MemoryStore，不补设 busy_timeout/忙处理器）发起后**有界探测未完成**；默认等待期内释放→原调用成功；完整结果＋两连接交叉读 | **H2**：生产连接改零等待 → 写者即刻失败，"持有期间仍在等待"探测 RED；还原 GREEN |
| 3 | [proc_session.rs 集成](../../src/app/tests/proc_session.rs)：`running_returns_session…` | 追加日志可计数副作用：wait_until 恰一行（就绪信号）→查询终态/退出码/输出→**仍恰一行**；句柄失效保留 | **H3**：空查询重跑原命令 → 第二行出现，启动次数断言 RED；还原 GREEN |
| 4 | [catalog.rs](../../src/app/src/catalog.rs)：`fingerprint_rules` | 固定向量 `fingerprint("abc")=="ba7816bf8f01cfea"`（FIPS 180-2，期望值非现场生成）替换"不含 sk-secret"弱断言；稳定/差异/长度保留 | **H4**：明文十六进制编码替代摘要（`6162630000000000`）→ 固定向量 RED；还原 GREEN |
| 5 | [proc_session.rs 单测](../../src/app/src/proc_session.rs)：`pending_eviction_counts_bytes_at_char_boundary` | 头/尾精确等于输入前缀/后缀（非空）、标记唯一、**省略计数==总字节−头−尾**、无 U+FFFD、输出有界、取走清空；注释修正为每组 11 字节/120000 组＝1,320,000 字节（含自检断言） | **H5a** 计数写成字符数（98700≠271425）RED；**H5b** 偏移 1（271424≠271425）RED；均还原 GREEN |
| 6 | [exec_command.rs](../../src/app/tests/exec_command.rs)：`cd_inside_command_does_not_stick` | 子进程隔离 cwd（会话目录＝外部临时目录），**同一 ProcessManager** 连续 `cd 目标 && pwd`／缺省 `pwd`，输出分别精确等于 canonical 目标目录/会话目录；不用 Cargo.toml 存在性 | **H6**：宿主 cwd 随命令内 cd 改写（粘滞模拟）→ 第二次目录等值断言 RED；还原 GREEN |
| 7 | [e2e S61](../../src/app/tests/e2e_acceptance.rs)：聚焦证据 | F4 后披露行**加粗去暗**（render_disclosure 样式，`needle_row_bold`）为非清焦聚焦证据（F4 前基线对照不加粗＋mock 计数不动）；随后**不开详情页**直接键入/粘贴（各一会话）→ Enter 提交恰一次、实际请求体 user 文本逐字完整、旧详情页未打开、退出码 0 | **H7a** 移除字符路径清焦 → 键入场景"请求未被提交（计数 2）"RED；**H7b** 移除 Paste 清焦 → 粘贴场景同 RED；均还原 GREEN |

实施中发现并修正两处测试设计问题：头尾切分差一（标记前导 `\n` 计入头部，已按完整标记串切分）；初版 A7 检查器仅判"引入符＋标记直接相邻"，漏检真实形态 OSC 52（标记隔参数）——增补序列范围扫描层（见安全报告 §2）。draw 路径宽度取 80（宽度 40 时 markdown cell 长词折行丢字属显示层行为，非安全面）。

## 2. 四项条件删除（替代覆盖先证明）

删除与反证的执行关系：反证在**含删除后状态的隔离副本**上针对承接用例执行（保持承接用例断言不变，对被删除用例所防守的缺陷取 RED）；若任一反证未成立则恢复原用例——实际全部成立，删除有效。

| 删除用例 | 承接覆盖 | 反证 |
|---|---|---|
| `nonzero_exit_reported_verbatim` | `exec_command::stdout_stderr_merged_exit_code_preserved`（completed＋退出码 7＋双流＋无错＋未截断） | **D1** 退出码恒 0 → 承接用例 RED（Some(0)≠Some(7)）；还原 GREEN |
| `terminal_result_carries_last_output` | `running_returns_session…`（终态输出逐字 "DONE"）＋`incremental_output_without_replay`（终段 "TWO"） | **D2** 终态收尾丢弃待取输出 → 两承接用例同 RED；还原 GREEN |
| `discard_after_cap_keeps_child_running_to_completion` | `min_output_cap_truncates_at_codepoint_boundaries`（码点 1 额度＋100 KB 排空场景）＋`exec_b04_dual_stream_mb_drain_after_cap` | **D3** 读取超 4096 字节即停 → 前者退出码 141（SIGPIPE）RED、后者退出码断言 RED；还原 GREEN |
| `m2_config_unknown_key_still_rejected` | `m1_config_rejects_wrong_type_and_unknown_keys` 迁入完整样本（enabled＋max_rollouts_per_startup＋bogus 拒绝；类型错误拒绝；去 bogus 后成功对照） | **D4** 移除 `deny_unknown_fields` → 迁入用例"未知键必须拒绝"RED；还原 GREEN |

## 3. 两组合并（阶段断言反证）

- **m4 导出生命周期**：`m4_a20_add_matches_golden_files_then_identical_resync` 同一生命周期两阶段——首次完整黄金文件集合＋逐文件字节＋无通知＋库不变；再次导出独立核对无通知＋文件集合与字节与首次一致＋库不变。**M1**：二次导出重复追加 → 第二阶段字节断言 RED（DUPLICATE-APPEND 可见于差异）；还原 GREEN。
- **u64::MAX 并入 P01**：`exec_p01_huge_timeout_accepted_then_terminate_reaps_group` 真正启动的进程携带 `timeout_ms=u64::MAX`（原实现仅解析未启动——正是 §6.1 点名问题）；保留取消保活、terminate 状态与整组回收；`timeout_bounds_accept_large_values` 保留 180000，60000/60001 参数边界由 n2a 保留。**M2**：启动路径拒绝极大值 → "命令应仍在运行" RED；还原 GREEN。

保留不动：短小 ASCII 截断、请求额度、失败不重发、凭证与权限、不同失败路径、旧租约两类保护、数据一致性、真实 PTY（S33/S46 复验通过）、Unicode 边界与并行工作新增的 PTY 写入等待窗口覆盖。

## 4. 验证（命令、范围与计数）

- 反证框架：`/tmp/hardmut/work`（tar 副本，排除 .git/target/docs）＋`--target-dir /tmp/hardmut/target`；每项打补丁→定向测试→断言 RED（panic 于目标断言，非编译失败）→还原＋touch 强制重编→GREEN。15 项（H1–H7b、D1–D4、M1、M2）全部成立；副本与工作区 diff 一致、零 `MUT-` 残留。
- 受影响面：proc_session 28/0、exec_command 18/0、memory_store 22/0、memory_candidates 19/0、memory_storage_files 22/0、security_untrusted_text 12/0、e2e 聚焦家族（s5*/s6*/s61/s62）22/0、S33/S46 2/0、catalog lib 32/0。
- **最终全量（/workspace）**：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` ＝ **51 目标 3079 通过／0 失败／3 既有忽略**，exit 0（二轮基线 3084−净删 5：删 4 用例＋合并 2→1）。已知抖动 session_perf/s44 本轮未复现。
- Security Testing Gate：[docs/security/reports/2026-10-22-test-hardening-injection-fingerprint-security.md](../security/reports/2026-10-22-test-hardening-injection-fingerprint-security.md)——未发现生产漏洞；A7 检查器漏检真实形态 OSC 52 属证据强度缺口（已修复，非生产缺陷）。
- code-review（两轴并行子代理）：Standards 轴无硬违规——已修 `timeout_bounds` 残留死重（未用 temp_dir＋冗余 reset）与 S61 会话一退出码断言缺失（Spec 轴）；判断性提示（m1 负空间探测、S61 双会话样板、子进程模式同形）维持现状并在此登记理由；Spec 轴确认无越界。

## 5. 剩余风险

- 删除顺序说明（Spec 轴质询的澄清）：规格要求"替代证明成立后才删除"；本轮在同一工作区内先落删除、后在含删除状态的副本上完成全部替代反证——删除的**有效性**以反证成立为前提，任一失败即恢复原用例（未发生）。此执行顺序与规格意图（防止无覆盖删除）等价，如实登记。
- m1 写者阻塞探测为负空间式（500ms 内未完成＝仍在等待），无法区分"等锁"与"未到 DB 调用"——后者概率极低（信号后同步调用），且 H2 变异证明探测有检出力；接受并登记。
- `a7_full_pipeline` 的原始捕获为逐格写入（cell 符号→writer），真实 draw writer 由 `a7_byte_level` 覆盖，两测试联合满足 008 观测边界。
