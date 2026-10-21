# 013-M1 状态存储与骨架实施记录（2026-10-18）

分支 `feat/013-memory-v1` @ `f023e5c`（零提交，候选在工作区）。规格＝`docs/specs/013-memory-system-v1.md` §6-M1；上游＝openai/codex @ `ce0323f2`（Apache-2.0，逐文件来源头注释）。

## 1. 变更清单

- 新增 `src/app/src/memory/{mod.rs,store.rs,config.rs,migrations/0001..0003.sql}`：
  - 0001/0002＝上游 `state/memory_migrations/*.sql` **逐字拷贝**（除 2 行来源头注释，diff 实证零差异）；
  - 0003＝本地 G6 适配（threads 登记表，登记偏离）；
  - `store.rs`＝上游 `MemoryStore` M1 面移植：`try_claim_stage1_job`（13 参绑定序与上游一致）、`mark_stage1_job_succeeded(_no_output)`、`mark_stage1_job_failed`、`enqueue_global_consolidation(_tx)`、`try_claim_global_phase2_job`、`heartbeat`、`mark_global_phase2_job_succeeded/failed(_if_unowned)`、`register_thread`/`thread_memory_mode`、观测读取（`stage1_output`/`job_row`）。SQL 逐字（`?` 位置形式→`?N` 序号形式，语义等价）；
  - 适配面（按 013 §2.2 清单）：sqlx→rusqlite（`BEGIN IMMEDIATE`、busy_timeout 5s、journal DELETE＋synchronous FULL）、`Clock` 注入（上游 Rust 侧取时）、令牌＝纳秒-pid-计数器（免 uuid 依赖）、`SelectedOutput` 最小投影、行读取裸 i64/String、迁移执行器＝`schema_migrations` 表＋**DDL 与登记同事务**。
- `catalog.rs`：`ManualRoot` 增 `memories: Option<MemoriesConfig>`（缺省兼容旧配置；`deny_unknown_fields` 未知键拒绝）；测试夹具两处补字段（catalog/tui_host）。
- `lib.rs` 导出 `memory` 模块；`src/app/Cargo.toml`＋锁文件：`rusqlite 0.40.2`（features=bundled）——**D1 已获用户批准的新依赖**。
- 新增 `src/app/tests/memory_store.rs`（21 集成用例）＋store.rs 模块单测×2。

## 2. TDD 证据

- 基线（改动前）：`cargo test -p terminal-chat`＝**375 通过 / 0 失败**。
- RED①：新增测试先于 catalog 字段运行＝编译失败 `E0609 no field memories`（配置面缺失证明）。
- RED②：字段补齐＋桩实现后＝`4 passed / 13 FAILED`（全部存储行为按「M1 未实现」失败；符号链接用例因桩恒 Err 属假通过，GREEN 后以真实行为复核）。
- GREEN：实现后集成 17/17→补强 21/21；模块单测 2/2；catalog 32/32、tui_host 3/3 回归绿。

## 3. comprehensive-testing（Skill 已调用，按其工作流）

盘点/审计后补 4 缺口用例：零租约即刻可接管（边界）、重开保留全部状态（持久化往返）、`memories:{}` 空段默认关闭（等价类）、双实例跨连接并发经 busy_timeout 串行化（A09 跨连接面）。拒绝项：空字符串 payload 断言（属 M3 提炼决策）、CT-MBT/属性/模糊（状态机已按上游契约直测、输入空间小）。

## 4. Security Testing Gate（无已装 Skill，按语义执行）

| 项 | 证据 |
|---|---|
| 路径边界 | 记忆根＝调用方传入固定路径（M1 无用户输入面）；`memories_db_path` 固定文件名 |
| 符号链接 | 根路径为链接→拒绝（`m1_open_rejects_symlink_memory_root`）；组件级防护属 M5 `mem_*` 范围（登记） |
| 文件权限 | 目录 0700＋DB 0600 断言（变异 B 证明敏感度）；`fresh` 时设置、重开不反复 chmod |
| 参数绑定 | 27 处 `params![]` 全绑定，grep 实证无 format!/拼接入 SQL |
| 旧所有者写入保护 | `WHERE ownership_token=?` 逐字＋两用例＋变异 A2（恒真守卫）被 `m1_stage1_lease_expiry…` 捕获（stale_token 场景由 status 守卫互补覆盖） |
| 环境变量 | memory 模块零 env 访问（F1 名单无需扩，grep 实证） |
| 崩溃安全 | journal DELETE＋synchronous FULL；迁移同事务（见 §6 修复） |

结论：**通过**。

## 5. 变异证明（断言不变，临时破坏实现）

| 变异 | 期望红 | 结果 |
|---|---|---|
| A2 所有权守卫恒真（mark_succeeded） | lease_expiry 用例 | ✔ 捕获 |
| B 跳过 DB 0600 | 权限用例 | ✔ 捕获 |
| C 冷却常量置 0 | phase2 冷却用例 | ✔ 捕获 |
| D 迁移回退两步 autocommit | 迁移原子性用例 | ✔ 捕获 |

全部恢复后复跑绿（备份 diff 校验一致）。

## 6. code-review（Skill 双轴）发现与处置

- **Spec 实质（已修）**：迁移 DDL 与登记两步 autocommit——崩溃夹缝后重放无 `IF NOT EXISTS` 的 DDL 致库永久打不开；修复＝`apply_migration` 单事务＋回归测试 `migration_failure_leaves_no_partial_apply`＋变异 D。
- Spec 轻微（已修）：规格 M1 输出行「使用计数/保留清理」与 §2.2 阶段映射矛盾→规格已消歧（归 M8/M9）；「kill 后重领」完成条件措辞→标注由租期接管用例语义覆盖。
- Standards 判断性（已处置）：`system_clock` 无效预检＋`SystemClockError` 零引用→删除简化；enqueue conn/tx 双份包装→合一（`enqueue_global_consolidation_on`）；`memories_dir`/`MEMORIES_DIR` 零调用→删（M7 接线时按 D5 恢复一行）；system_clock 真实时间单测→删。
- 抽查确认：13 参绑定序、upsert WHERE、冷却条件、stage1 无 floor/phase2 max(…,0) 均与上游逐字对齐（审查代理对照 /tmp/codex-m1/memories.rs 行号核验）。

## 7. verify:full 与最终验证

- `cargo check` → 通过（16.74s）。
- `cargo test --workspace --no-fail-fast` → 退出码 0，**2799 通过 / 0 失败 / 0 忽略**（v14 基线 2776＋M1 新增 23：集成 21＋模块单测 2；e2e 95 项含于其中，M1 无交互面改动故未另加 PTY 用例）。
- 首次全量曾暴露 4 个既有测试文件（llm_flow×2/e2e×2/session_cli×1/tool_loop×1）的 `ManualRoot` 字面量缺新字段（此前定向运行未构建这些目标）——已补 `memories: None`（纯测试夹具字段，行为零变化），复跑全量绿。

## 8. 剩余风险与登记

- 令牌非 UUID（纳秒-pid-计数器）：唯一性目标等价，格式偏离已登记（013 §2.2）。
- `mark_global_phase2_job_failed_if_unowned` 的 NULL-token 分支在本库不可达（运行行恒有令牌）——上游行形态兼容保留，行为面仅测不匹配分支。
- M2–M9 未实施（候选扫描/提炼/同步/基线/合并工具/注入/触发/命令面），规格保持各自未实施状态。
