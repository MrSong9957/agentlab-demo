# 013-M2 候选选择实施记录（2026-10-18）

任务分支 `feat/013-memory-m2`（基线 main @ `99eaf81`）。上游锚：openai/codex @
`ce0323f2f2d3b25e82806cd555237da3db4e1897`——`state/src/runtime/memories.rs::
claim_stage1_jobs_for_startup`（编排循环＋`stage1_source_needs_update`）、
`memories/write/src/lib.rs mod stage_one`（常量）、`config/src/types.rs`
（MemoriesToml/MemoriesConfig 默认与钳制）、`memories/write/src/phase1.rs`
（填参点）。实施前已逐项核实（不得凭名称猜测）：默认 2/10/6，钳制 1–128/0–90/1–48，
`THREAD_SCAN_LIMIT=5_000` 与 `JOB_LEASE_SECONDS=3_600` 为常量非配置；窗口
`updated_at_ms >= now−年龄`（含）且 `<= now−闲置`（含），`ORDER BY updated_at_ms
DESC LIMIT scan_limit`（无并列决胜键，上游显式 `include_thread_id_tiebreaker=false`）。

## 1. 变更清单

- `src/app/src/memory/candidates.rs`（新）：`claim_stage1_jobs_for_startup`
  编排（0 值短路→threads 扫描→目录交集→needs_update→`try_claim_stage1_job`
  领取，领满即止，worker＝当前会话，`max_running_jobs=max_claimed`）；
  `Stage1StartupClaimParams`（无 `allowed_sources`——本地无来源概念，登记差异）；
  `Stage1JobClaim` 最小投影（含 `session_path` 供 M3）；常量
  `THREAD_SCAN_LIMIT`/`JOB_LEASE_SECONDS`；`params_from_config`（钳制在参数构造，
  ＝上游 `From<MemoriesToml>` 换算点）。
- `src/app/src/memory/store.rs`：`stage1_startup_scan`（过滤/排序/计数口径逐字：
  `memory_mode='enabled'`、排除当前会话、毫秒双窗含端点、`DESC LIMIT`；G6 最小表
  无来源/归档列＝登记差异）；`stage1_source_needs_update`（结果行或成功水位
  覆盖即跳过；与 `try_claim` 事务内检查的重叠系上游有意设计，注释注明）。
- `src/app/src/memory/config.rs`：`max_rollouts_per_startup`/`max_rollout_age_days`/
  `min_rollout_idle_hours` 三字段（serde 缺省＝上游默认）＋`clamped()`（上游钳制
  语义）＋手写 `Default` 与 serde 对齐（审查修复，见 §6）。
- `src/app/src/memory/mod.rs`：模块声明与再导出。
- `src/app/tests/memory_candidates.rs`（新，18 用例）：A04/A05（含 +1s/+1ms 边界
  两侧与恰等边界）/A06/A07/A08/A15、降序＋领取上限、扫描上限与领取上限区分、
  扫描预算计数口径、退避/耗尽跳过且循环继续、重复触发租期挡板、目录∩登记交集、
  缺目录、0 值短路、配置默认/旧配置兼容/越界钳制/未知键拒绝/Default 对齐。

职责边界（按 M2 契约确认）：本阶段只读 threads 登记、经 M1 原语领取任务（jobs
写入）；**不登记会话**（登记＝会话写入方职责，后续阶段接线——此前 threads 表
无生产写入方，首次接线随 D2/M3 落地）；不装载会话正文（M3）；无后台执行。
去重＝（会话 ID＋源更新时间）身份：threads 主键与目录 stem 结构性唯一，无重复源。

## 2. TDD 证据

- 基线（实施前）：`cargo check` 通过；`--test memory_store` 21/21；lib catalog 31
  过滤项全绿。
- RED：先写 `memory_candidates.rs` 全部用例（引用未实现符号），编译失败
  E0432/E0560/E0609 共 10 错——目标行为缺失证明。
- GREEN：实现上述四文件后 `--test memory_candidates` 14/14（补强前）；
  `--test memory_store` 21/21、lib 102/102 无回归。

## 3. comprehensive-testing（Skill 已调用，按其工作流）

审计缺口→补强 3 用例＋2 强化：扫描预算被后续过滤器剔除的行消耗不回补
（`m2_scan_budget_consumed_by_rows_failing_later_filters`，钉住「扫描上限约束
数据库行」的上游计数口径）；退避/重试耗尽候选跳过且循环继续（§7.3「失败状态」
行，`m2_backoff_and_exhausted_threads_skipped_but_loop_continues`，步进时钟布置
同版本三次失败耗尽＋未来 retry_at 退避）；同参数立即重跑被活动租期挡住
（`m2_immediate_rerun_blocked_by_active_leases`，对应 D2 每轮触发的重复检查）；
worker 身份断言加入降序用例；+1ms 闲置边界加入 A05。拒绝项：CT-MBT（单遍筛选
管道无多阶段状态机）、并列时间顺序断言（上游无决胜键＝不可指定行为）、
`max_running_jobs` 交互（M1 原语已测）、负数 usize JSON（serde 框架行为）。

## 4. Security Testing Gate（无已装 Skill，按语义执行——本轮判定：跳过详细门）

依据：① 新增文件访问仅 `session::list_sessions`（既有 readdir＋stat，无正文
读取）；② 路径构造仅 `sessions_dir.join(thread_id + ".jsonl")`，thread_id 必先
通过目录交集（真实 readdir stem，文件名不含路径分隔符，`..` 不可能是 readdir
条目）→ 结构性无穿越；③ 新增 SQL 均静态文本＋`?N` 参数绑定（无拼接），扫描为
只读；④ 状态写入全部经 M1 已门禁原语（`try_claim_stage1_job`）；⑤ 无新增
不可信输入边界（配置装载沿 catalog 既有 deny_unknown_fields 面）。M5 合并轮
`mem_*` 的路径围栏属后续阶段（013 登记）。

## 5. 变异证明（断言不变，临时破坏实现）

| 变异 | 预期失败用例 | 结果 |
|---|---|---|
| A 闲置上界 `<=`→`<` | a05（恰等边界入选） | 按预期失败 ✓ 恢复后绿 |
| B 排序 `DESC`→`ASC` | desc_order（降序＋限额） | 按预期失败 ✓ 恢复后绿 |
| C `stage1_source_needs_update` 恒真 | a06/a07 | **仍通过＝等价变异**：`try_claim` 事务内同两项检查（上游双重设计，预检查是纯优化），排除并登记 |
| C′ 源版本 ms→s 换算移除 | a07（水位值）/a08（截断） | 按预期失败 ✓ 恢复后绿 |
| D `clamped()` 恒等返回 | config_clamps | 按预期失败 ✓ 恢复后绿 |
| E 目录交集检查移除 | need_both_file_and_registration | 按预期失败 ✓ 恢复后绿 |

另登记排除：领满即止的循环 `break` 移除＝等价变异（SQL 运行数上限
`max_running_jobs=max_claimed` 产生相同可观测行为——无额外任务行、同领取列表）。
全部恢复后 17/17（补 Default 对齐用例后 18/18）＋memory_store 21/21 复绿。

## 6. code-review（Skill 双轴，并行子代理）发现与处置

- **Standards 轴**：无硬违规。判断题处置：① `derive(Default)` 与 serde 缺省分裂
  （`default()` 得 0/0/0 vs `{}` 得 2/10/6）——**已修**：手写 `Default` 对齐＋
  测试 `m2_config_default_matches_serde_defaults` 钉住；② needs_update 与
  try_claim 查询重复——上游逐字双重设计，**注释注明**非缺陷；③ 扫描内 `.max(0)`
  与 `clamped()` 双重防御——均为上游原形态（store 侧防御＋换算侧钳制），保留；
  ④ 测试直连 rusqlite 布置 memory_mode（无生产写入面，注释说明）——接受。
- **Spec 轴**：① **规格验收命令失配（实质发现）**：`cargo test --workspace memory`
  按测试名过滤，m1_/m2_ 用例 0 条执行（实测空转通过）——规格侧已修正为实测
  有效的 `cargo test --workspace --test memory_candidates --test memory_store`
  （18＋21 实跑验证）；M1 条目同款命令同病（M1 实际验证用的是 `--test
  memory_store`，记录在案，见 §7 勘误说明）；② §4「候选扫描上限…目录枚举上限
  同值」与落地形态不符——落地＝上游计数口径（LIMIT 约束 DB 行，目录枚举仅作
  存在性交集不受限），规格 M2 条目已按预测→实测回填；③ 钳制时机：本项目结构
  ＝上游 Toml 面（原值），`clamped()`＝effective 面，消费方必须经
  `clamped()/params_from_config`——config.rs 文档与规格均已写明；④ `enabled`
  总开关不在领取路径检查——与上游一致（门在启动入口），D2 接线时调用方先
  gate，已登记。
- 工作树观察：`db/wiki.db` 为 10-08 既有未跟踪文件，与本轮无关，不触碰。

## 7. verify:full 与最终验证

- `cargo check`：通过（exit 0；仅 tui 层既有警告，与本轮无关）。
- `cargo test --workspace --no-fail-fast`：**2817 通过 / 0 失败 / 0 忽略**（exit 0；
  M1 基线 2799＋本轮新增 18＝2817，逐条对账无缺口）。
- 修正后验收命令实测：`cargo test --workspace --test memory_candidates --test
  memory_store` → 18/18＋21/21（39 条实跑）。
- 验证后无影响结果的代码改动（后续仅规格/记录文档回填）。

## 8. 剩余风险与登记

- threads 登记无生产写入方（登记接线随 D2/M3 阶段）：在接线前，真实运行中候选
  恒为空（A15 语义）；测试经 `register_thread` 原语布置。
- 上游并列更新时间无决胜键：同毫秒更新的多会话间顺序未指定（上游行为，测试
  避免依赖并列顺序）。
- `list_sessions` 将目录项名（含极少见的目录名 `*.jsonl`）计入存在性——沿
  007 `/sessions` 既有语义，未改动。
