# 014 记忆系统四项最小修订实施记录

状态：**已交付（2026-10-20 用户批准后实施提交、快进合并并删除任务分支，见 §9）**。实施轮完成规格四项修订与全部规定门禁（TDD→测试完整性→安全门→双轴审查→全量验证）；实施轮自身未暂存/未提交（交付动作见 §9）。规格见 [014](../specs/014-memory-minimal-revisions.md)；上游语义依据见 [013 对照报告 §8/§9](./2026-10-20-013-memory-codex-comparison.md)。

## 1. 起点、分支与基线

- 实际 HEAD＝main＝`cab85bcf383498ae18b890b05f087fc2a8b558fe`（与参照一致）。工作区＝AGENTS.md 并发修改（保留，未纳入）＋未跟踪对照报告与 014 规格；暂存区空、stash 0、单工作树。
- 从 main 建普通分支 `feat/014-memory-minimal-revisions`（建前确认未占用）。
- 改前受影响基线实跑：**7 套件 256/0/0，exit 0**（lib 142、candidates 18、feedback 4、lifecycle 6、phase2 43、runner 22、store 21）。历史 3010/0/3 仅作参照。

## 2. 实现与精确边界

| 项 | 实现 |
|---|---|
| §1 反馈不建库 | `MemoryStore::open_existing`（store.rs）：`SQLITE_OPEN_READ_WRITE \| SQLITE_OPEN_NO_MUTEX`（**无 CREATE**）、不建根不 chmod（不调 `init_memories_root`）但**保留其符号链接根拒绝**（自带 `symlink_metadata` 检查——M1 契约 `m1_open_rejects_symlink_memory_root` 对齐，收口轮补齐，RED 先证绕过）、不跑迁移；`is_file` 预检仅区分 None/Err，禁创建由打开标志承担（预检与打开之间被 reset 删文件→打开失败而非重建）。`after_user_turn_with_clock`（feedback.rs）重排：enabled 门→**先** `cited_thread_ids` 判空早退（无引用零 IO）→`open_existing` 失败/缺失一律静默返回，**无任何回退**。计数/去重/缺行忽略/best-effort 原样 |
| §2 状态失败展示 | `LatestErrorJob{kind, last_error: Option, retry_at: Option, retry_remaining: i64(非空列)}`＋`latest_error_job()`（只读，`ORDER BY finished_at DESC, kind DESC, job_key DESC` 并列次序键）＋`phase2_success_cooldown_remaining()`；冷却判定抽出 `phase2_in_success_cooldown()`，**领取原语与展示共用同一函数/同一 6h 常量**（展示另要求 `status='done'`——可达状态下与领取条件等价，注释明示）。runner 展示：阶段中文映射／原因**先 `redact_secrets` 后 `display::truncate_chars(…,200)`＋`…` 标注**（内容 ≤200 字符口径，注释明示）／剩余重试次数／**`jobs.retry_at` 本值**＋剩余分钟或「现在可重试」。无库不建库守卫沿用；零新 schema/日志/遥测 |
| §3 generate 开关 | config.rs 增 `generate_memories: bool`（serde 默认 true；Default/clamped 同步；不钳制）。`register_thread(thread_id, memory_mode, updated_at_ms)` 参数化；`ON CONFLICT DO UPDATE SET updated_at_ms` **不变**＝既有行资格不被覆盖。`note_persisted` 传 `generate_memories ? enabled : disabled`——**全库唯一使用点**（结构上不可能误接管线/注入/清理/反馈门）。两处筛选与清理 SQL 零改动（仅补验证）。全部 26 处调用点更新（生产 1＋测试 25） |
| §4 注释勘误 | runner.rs 重置边界头注释（`.pipeline-epoch`→`.epoch`＋flock＋内存任务快照，四个行号锚点经审查核实）；store.rs uuid 句（依赖事实相符）。仅此两处 |

不重开 D2/D3/D4/D12-A/G8；未扩展格式约束/专用模型/全局后台暂停/V2。

## 3. RED/GREEN（新用例 9 例，均先写断言实跑）

- **RED（改前实跑，真实断言失败）**：§1 三例（无引用建库/有引用建库/reset 后重建——与对照报告探针 S1/S3 一致）＋§2 一例（无失败/冷却展示）＋§3 一例（登记恒 enabled）＝5 例 FAILED。配置字段为纯数据脚手架先行，行为 RED 由上述用例承载。
- **守卫/验证型（改前即绿，价值在变异与防回归）**：管线正向场景（enabled 来源＋generate=false→照常提炼合并，断言 3 次请求与产物）、两阶段筛选尊重资格＋清理无 mode 过滤（验证既有实现，未重写）。
- **审查补强（Spec 轴 P2 后新增）**：状态用例扩展**真实 phase2 失败路径**（领取→失败记账→「全局合并」阶段映射＋有错不显示冷却；同秒并列以 1.1s 间隔消除不确定性）；新用例 `m14_generate_false_leaves_use_paths_untouched`（验收⑥：generate=false 下注入/引用计数/状态/重置四使用面原样）。
- **收口轮（交付前用户指令）**：`m14_feedback_rejects_symlink_memory_root`（unix）——符号链接根＋合成真实库：直接入口须 Err（含拒绝文案）、反馈静默跳过且目标库零写入。**RED 先证绕过**（guard 缺失时该用例真实失败），补 5 行检查后 GREEN。
- **GREEN**：m14 **9/9**。测试侧构造修正两处（Drop 结构部分移动改独立目录；密钥置于截断点内——「先脱敏后截断」实现正确，密钥落截断点外时不泄漏无标记属预期边界）。

## 4. 错误检测（变异证明，隔离副本 `/tmp/014-mut`＋专属 target dir，逐项恢复，零残留）

| 变异 | 结果 |
|---|---|
| M1 反馈失败/缺失**回退建库**（None→`open`） | no-db 与 reset 两例 FAILED ✓（首版变异 Arc 非 Copy 编译失败，加 clone 后为行为变异——编译失败不计检出，如实登记） |
| M2 展示**误用 retry_remaining 替代 retry_at 本值** | 状态用例 FAILED ✓ |
| M3 **generate 误接管线总门** | 管线正向场景 FAILED ✓ |
| M4 **后续登记覆盖既有资格**（ON CONFLICT 追加 SET memory_mode） | 登记路径用例＋m1 冲突保持断言双 FAILED ✓ |
| M5 **绕过展示侧脱敏/截断**（直显原始 last_error） | 状态用例 FAILED（密钥可见）✓ |
| M6 **只读查询引入改值写**（latest_error_job 附带 UPDATE retry_at+1） | 状态用例 FAILED（库文件字节断言）✓；附注：**同值无操作 UPDATE 未被检出**——SQLite 未产生可观察文件变化，该形态无实际副作用，不虚报检出 |
| M7 **generate 误接注入门** | 验收⑥用例 FAILED ✓ |
| （规格列「恢复先 open 后解析顺序」） | **不可观察**：open_existing 无创建能力，重排仅增加无引用轮的一次开库 IO，验收①④对其天然不敏感；该风险的可创建形态即 M1。登记为等价（除 IO）而非检出 |

M1–M7 全检出（M6 为改值形态）；§4 注释勘误按收敛轮决定不做行为变异（人工核对＋全量回归）。

## 5. 门禁与实测

- **受影响套件**（收口后最终态：lib＋candidates＋feedback＋lifecycle＋phase1＋phase2＋runner＋store，`--no-fail-fast`）＝**8 套件 284/0/0，exit 0**（m14 9 例在内；inject 无本轮改动路径未列；初稿曾误记 323/331——以本行为准勘误）。
- **e2e（真实 CLI/PTY，含默认用户路径与恢复路径）**：单独首轮 **105/1**（s44 TUI 滚轮时序用例失败——与本轮记忆改动无交集；单独运行通过）；全量复跑与最终 workspace 内运行均 **106/0/0**。判定既有并行 PTY 抖动，双跑如实登记。
- **cargo check**（workspace）：exit 0。
- **cargo test --workspace --no-fail-fast**（收口后终验）：**49 套件，3019 通过/0 失败/3 既有忽略/0 过滤，exit 0**（基线 3010＋m14 9 例，对账吻合；审查修复后中间值为 3018 亦实测 exit 0）。
- **Clippy**：系统组件安装失败（下载缓存错误，M8 同况）；复用 M6 轮 1.99 配套组件（同版本族 b940084d7e），隔离副本 `cargo-clippy clippy --locked --offline -p terminal-chat --all-targets` **exit 0**；警告均为既有（runner.rs unneeded return／store.rs 复杂元组类型/测试 mod clone 三处——逐条核对在本轮未触碰代码），审查修复与收口轮后复跑仍零新增。
- **rustfmt（定点）**：8 个触碰文件的格式差异块与基线**逐块内容一致**（仅行号偏移）；我方代码经「标记选择性＋追加区整体」格式化达到与基线同等干净度。未顺带重排既有代码；不宣称全库通过。
- **git diff --check**：exit 0。
- **安全门**：[安全报告](../security/reports/2026-10-20-014-memory-minimal-revisions-security.md)——本轮范围无新增可复现安全缺陷；不扩大为全系统结论。

## 6. 双轴审查与处置

两轴均**无 P1**。发现与处置：

| 发现 | 处置 |
|---|---|
| Std P2-1 本地 `truncate_chars` 与 `display::truncate_chars` 重名异义、未复用 | ✅已修：复用 `display::truncate_chars`，超限时调用点追加 `…`（内容 200＋标记 1 的口径写入注释） |
| Std P2-2 `retry_remaining` 建模 Option 产生死分支（列 NOT NULL） | ✅已修：改 `i64`，runner 死分支删除 |
| Spec P2-1 phase2 失败展示无测试（「全局合并」映射未覆盖） | ✅已修：状态用例扩展真实 phase2 失败路径 |
| Spec P2-2 验收⑥（使用面不受 generate 影响）无测试 | ✅已修：新增 `m14_generate_false_leaves_use_paths_untouched` |
| Spec P2-3 变异清单不全 | ✅已补：M5/M6/M7 实跑检出；重排形态证不可观察（§4 附注）；门禁声明出入随本轮补齐消除 |
| Std P3-1 `unwrap_or(0)` 假默认 | ✅已修（let-else 重构） |
| Std P3-2 SQL 反斜杠续行风格独此两处 | ✅已修（`r#""#`） |
| Std P3-4 冗余 clone/多余分配 | ✅已修（解构 move） |
| Std P3-5 冷却「同一条件」注释过头 | ✅已修（补 done 守卫说明） |
| Std P3-6 排序无并列打破 | ✅已修（`kind DESC, job_key DESC` 次序键） |
| Std P3-7 测试裸 SQL 播种新行 | ✅已修（改生产 API）；candidates 旧注释过时＝既有内容、超出 §4 两处勘误范围，仅登记 |
| Std P3-8 断言失败泄漏临时目录 | ✅已修（CleanDir Drop 守卫） |
| Std P3-9 测试真实时钟/不必要 sleep | 部分：不必要 sleep 已删；状态用例真实时钟属既有接口限制（`memory_status_text_in` 无注入口，m7 同风格），登记不改接口 |
| Std P3-10 截断缺独立单测 | 登记取舍：M5 变异＋集成断言（密钥可见性/长尾/映射）已覆盖该逻辑；不为 3 行包装另立测试面 |
| Std P3-11 记录候选清单缺 4 文件 | ✅已修（§7 补全） |
| Std P3-3 open_existing 不设防符号链接根 | **✅收口轮已修（用户指令复核后升级）**：核实确认新入口确曾绕过 M1 交付的根拒绝（反馈路径旧经 open 被拒、新经 open_existing 被接受——RED 用例实证）；最小补齐＝入口自带 `symlink_metadata` 检查（不建根不 chmod，与「不调 init_memories_root」条款不冲突——该条款指不建目录/不设权限，非豁免拒绝）；规格 §1 边界已改写为准确条款；临时符号链接＋合成库验证（Err＋目标库零写入） |
| Spec P3-2 规格外的伴随微调 | 登记披露：三个 JOB 常量改 pub（展示映射必需）＋try_claim 内一处 params! 换行（选择性格式化触及我改动的 cooldown_cutoff 邻域，纯格式零行为） |
| Spec P3-3 缺省值无显式断言 | 登记不另改既有 m1_config 用例（解析成功＋Default 路径覆盖 true 语义；m14 注册路径断言 enabled 映射） |

## 7. 候选文件（逐文件清单，共 15 项）

生产 4（git 已修改）：
1. `src/app/src/memory/store.rs`（open_existing/状态查询/冷却同源/register_thread 参数化/uuid 注释）
2. `src/app/src/memory/runner.rs`（状态展示/登记传 mode/.pipeline-epoch 注释）
3. `src/app/src/memory/feedback.rs`（先解析后开库＋open_existing）
4. `src/app/src/memory/config.rs`（generate_memories 字段）

测试 8（git 已修改）：
5. `src/app/tests/memory_feedback.rs`（m14 用例 4 例：三建库场景＋符号链接拒绝）
6. `src/app/tests/memory_runner.rs`（m14 用例 4 例：状态展示/登记路径/管线正向/使用面）
7. `src/app/tests/memory_store.rs`（m14 筛选清理验证＋m1 登记测试扩展）
8. `src/app/tests/memory_candidates.rs`（register_thread 调用点＋clamped 字面量补字段）
9. `src/app/tests/memory_phase1.rs`（register_thread 调用点，无断言变化）
10. `src/app/tests/memory_phase2.rs`（register_thread 调用点，无断言变化）
11. `src/app/tests/common/memory_v1.rs`（register_thread 调用点，无断言变化）
12. `src/app/tests/e2e_acceptance.rs`（register_thread 调用点，无断言变化）

文档 3（git 未跟踪）：
13. `docs/specs/014-memory-minimal-revisions.md`（状态翻转＋收口条款改写）
14. `docs/records/2026-10-20-014-implementation.md`（本记录）
15. `docs/security/reports/2026-10-20-014-memory-minimal-revisions-security.md`

**保护内容（非候选）**：`AGENTS.md`（并发修改，保留未暂存）、`docs/records/2026-10-20-013-memory-codex-comparison.md`（既有未跟踪调查资料）。Git 实况＝13 修改＋4 未跟踪＝17 项，减 2 保护项＝**15 候选**（收口核对：曾按 12 修改口径误算为 14，实为 13 修改）。

## 8. 最终工作区检查

`git diff --check` exit 0；分支 `feat/014-memory-minimal-revisions`（基线 `main @ cab85bcf383498ae18b890b05f087fc2a8b558fe`，`git rev-parse` 实测）；暂存区空、stash 0、单工作树；候选 15 项全部未暂存。变异副本文件与工作区逐字节一致（零残留复核）。收口轮（符号链接补齐）后重跑：m14 9/9、受影响 284/0/0、workspace 3019/0/3、clippy exit 0、定点 rustfmt 与基线一致。停在「014 已实现并验证，待批准交付」。

## 9. 交付记录（2026-10-20 用户批准后执行）

批准范围＝§7 逐文件清单的 **15 个路径**。交付核验：分支 `feat/014-memory-minimal-revisions`、HEAD＝main＝merge-base＝`cab85bcf383498ae18b890b05f087fc2a8b558fe`、暂存区空、stash 0、单工作树；工作区恰为 15 候选＋两项保护（AGENTS.md 并发修改、013 对照报告），无候选外内容；`open_existing` 符号链接根拒绝在位（store.rs:201-206）；变异仅存在于 /tmp 副本、工作仓库零残留、无备份或临时文件混入。因 3019 运行后有过一次纯格式化编辑，对应关系按「无法严格证明」处理——**在精确候选字节上重跑最终门禁**：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` exit 0，**49 套件 3019/0/3、0 过滤**（与实施记录 §5 一致）。

- **实施提交** `463ed8ffcc4d904d8052a732537012c4fcd8ac59`（`feat: 014 记忆系统最小修订`，父＝`cab85bc`）：15 路径自 /tmp 清单逐文件显式暂存（无 `git add .`/`-A`/目录级）；暂存集合与清单 diff 为空；保护项零进入（AGENTS.md 与 013 对照报告均未暂存）；`git diff --cached --check` 通过；提交实际范围恰 15 文件（1021 插入/59 删除）。
- **快进合并**：`git merge --ff-only` 入 main（cab85bc→463ed8f）；`merge-base --is-ancestor` 证实包含；合并后 main 提交树与实施提交一致（均 `8dbe1c684e04793cab1f2c2006651c2d01ce22e7`）；快进零引入候选外变化，未重跑门禁（引用上述精确字节实测）。
- **交付文档提交**（本节＋规格状态翻转，`docs(014): 最小修订标记为已交付`）相对实施提交仅修改规格与本文档两份；RED/GREEN、符号链接漏检与修复经过、统计勘误、审查处置、变异及登记限制、reset 资格适配与其他已批准边界全部保留原文；013 已交付状态、安全报告与对照报告未改；历史「待批准」字样按各自轮次原文保留。
- **分支清理**：确认实施提交已包含于 main 后 `git branch -d feat/014-memory-minimal-revisions`（非强制）。
- 不推送、不发布、不操作远程、不追加真实模型调用；AGENTS.md 并发修改与 013 对照报告保持原样（起点 SHA `a753ea2e…`/`ba2ad1e8…` 复核见最终报告）。
