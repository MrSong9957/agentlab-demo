# 规格：记忆系统四项最小修订（013 交付后修订）

> 状态：**已交付（2026-10-20 用户批准后实施提交 `463ed8ffcc4d904d8052a732537012c4fcd8ac59` 快进合并 main，任务分支合并后删除）**。实施基线 `main @ cab85bcf383498ae18b890b05f087fc2a8b558fe`。RED→GREEN＋四类变异检出＋双轴审查；全量 `cargo check` exit 0、`cargo test --workspace --no-fail-fast` **49 套件 3019/0/3、0 过滤**（基线 3010＋m14 新增 9，含审查补强与收口轮符号链接拒绝对齐）；变异 M1–M7 检出（审查补齐）；e2e 106/0/0（首轮 105/1 为 s44 既有 PTY 抖动，单独与复跑均过）。详见 [实施记录](../records/2026-10-20-014-implementation.md) 与 [安全报告](../security/reports/2026-10-20-014-memory-minimal-revisions-security.md)。013 V1 已交付状态不变。
> **2026-10-20 范围收敛轮（用户确认三点后补清，仅文字）**：①§2 状态展示必须含 `retry_at` 本值（下次可重试时间），「剩余重试次数」不能替代；冷却按 phase2 领取条件同源推导（store.rs:644-649）。②§1 `open_existing` 失败后**不得回退**允许创建的 `open`，验收覆盖「有效引用＋库已被 reset 删除→跳过、不重建、用户轮正常结束」。③§5 明确 §4 注释勘误不做行为测试或变异证明，仅核对描述与实现一致。§3「资格随 reset 清除」保留为本地既有布局适配并醒目标注**与 Codex 跨重置资格保留不同**；本轮不增加 SessionMeta 持久化或逐会话命令。
> 需求来源：2026-10-20 用户指令——依据 [013 对照报告](../records/2026-10-20-013-memory-codex-comparison.md) §8/§9（复核与开关收口轮，勘误 E1–E8）完成四项最小修订的规格与实施准备。
> 上游锚点：openai/codex 固定提交 `ce0323f2f2d3b25e82806cd555237da3db4e1897`（快照 `/tmp/013-cmp-up/tree/`）。开关语义采用结论见对照报告 §9；本规格不重开 D2（轮级触发）、D3（合并轮受限工具）、D4（system 尾段注入）、D12-A（差异层便条过滤）、G8（外部 git）等已批准适配。
> 范围：严格限于 §1–§4 四项；每项含问题／输入／输出／边界／最小改动落点／验收场景。§5 为统一实施与验证要求。拟新增测试**均尚不存在、尚未运行**；013 收口时的 workspace 3010/0/3 仅为既有验证基线引用，不作为本轮结果。

## 1. 引用反馈不隐式建库

- **问题**：`after_user_turn_with_clock`（feedback.rs:208-226）在 `enabled` 后**先** `MemoryStore::open`（建根＋建库＋跑迁移）**再**解析引用——每个完成轮无条件开库。已复现（对照报告 §8.2-1 探针 S1/S3）：无引用回复即创建根与库；`/memory-reset confirm` 后带引用回复会重建库。数据无错误（缺行忽略、代数不受影响），但违反 `/memory-status` 已体现的「读路径不建库」原则，且重置后文件自行重现。
- **输入**：用户轮最终回复文本（可能含/不含 `<oai-mem-citation>` 块）；记忆根当前状态（不存在／存在无库／正常库／刚被 reset 清空）。
- **输出**：无有效引用→零 IO 直接返回；有引用且库存在→既有计数行为不变（`record_stage1_output_usage`，缺行忽略、调用方去重）；有引用但库不存在→**静默跳过**（不建目录、不建库、不迁移、不报错，用户轮不受影响）。反馈恒为 best-effort。
- **边界**：必须落实底层「**打开已有数据库、禁止创建**」语义——`MemoryStore` 新增只开既有库的构造（rusqlite `OpenFlags` 不含 `SQLITE_OPEN_CREATE`，文件缺失即失败；不调 `init_memories_root`——指不建根、不 chmod，**非豁免其符号链接拒绝**：入口自带 `symlink_metadata` 根检查，保留 M1 交付契约 `m1_open_rejects_symlink_memory_root` 的拒绝语义；不跑迁移——未迁移旧库按现状失败，如实跳过）。**`open_existing` 失败后不得退回允许创建的 `open`**——任何回退都重新引入隐式建库，禁止。**「先 exists() 再调允许创建的 open」同样不合格**：检查与打开之间 reset 可删除文件，交错窗口重新引入隐式建库。交错语义：open 成功后 reset 删文件→写入落已删 inode（runner.rs:46-47 既有论证同款），不污染新库；open 时文件已被删→失败→跳过。无需新增锁。不新增通用存储框架。
- **最小改动落点**：`store.rs` 新增 `open_existing`（约十余行＋单测）；`feedback.rs` 调整 `after_user_turn_with_clock`——先 `cited_thread_ids(reply)` 判空早退，再 `open_existing` 失败即返回。
- **验收场景**（拟新增用例，尚不存在）：①无引用且根不存在→根/库均不创建（探针 S1 为 RED 素材）；②有合法引用但库不存在→不创建库、调用正常返回；③既有用例保持——已有结果被有效引用计数与 last_usage 正确、未知 ID 忽略、重复 ID 去重恰 +1；④**存在有效引用、但数据库已被 reset 删除**：反馈跳过、数据库不重建、用户轮正常结束（探针 S3 为 RED 素材；另须证明无「失败后退回允许创建的 open」的回退路径——变异：加入回退→本用例必须失败）；⑤副作用断言用文件系统实际状态（目录/文件存在性），不只断言返回值。

## 2. /memory-status 最小失败展示

- **问题**：`memory_status_text_in`（runner.rs:429-479）只展示任务三态计数；`jobs.last_error`/`retry_at`/阶段已持久化（store.rs:489-514；phase2 失败原因为阶段名 phase2.rs:166-299，stage1 为脱敏后真实错误 phase1.rs:149）但不可见；phase2 成功后 6h 冷却与「无事可做」无法区分。后台失败对用户完全不可见。
- **输入**：既有 jobs 表行（无 schema 变更）；`PHASE2_SUCCESS_COOLDOWN_SECONDS` 既有常量。
- **输出**：在「后台任务」行后追加——**最近一条 error 任务**：阶段（stage1＝第一阶段／memory_consolidate_global＝全局合并）｜原因（**经 `redact_secrets` 再过滤一次**并截断 ≤200 字符）｜**下次可重试时间＝`jobs.retry_at`**（未到→显示绝对时间或剩余分钟；已到→「现在可重试」）——**必须展示 `retry_at` 本值，「剩余重试次数」（`retry_remaining`）不能替代下次可重试时间**；**合并冷却**：按**既有 phase2 领取条件同一公式**推导——`try_claim_global_phase2_job` 判 `SkippedCooldown` 的条件为 `last_error IS NULL && finished_at > now − PHASE2_SUCCESS_COOLDOWN_SECONDS`（store.rs:644-649，cutoff 见 :558，常量 6h 见 :27），状态展示以同一条件同一常量计算「合并冷却剩余 Y 分钟」，条件不满足不显示——**展示规则与实际领取行为必须同源，不得另立一套冷却规则**。
- **边界**：命令保持只读——沿用既有「先查库文件存在再打开」守卫（不建库、不建目录、不触发后台）；错误文本双重脱敏（写入侧已 `redact_secrets`＋api 层 `redact_credential`，展示侧再过一次＋截断）；输出有界。**不新增** schema、日志、遥测、历史统计；当前任务状态不得称为历史失败率。
- **最小改动落点**：`store.rs` 新增两个只读查询（`latest_error_job`；全局合并行读取——可复用 try_claim 的 SELECT 形态，store.rs:570）；`runner.rs` 组装展示文本。
- **验收场景**（拟新增，尚不存在）：①无库→既有「未初始化」路径不变且零创建；②库存在无 error→不出现失败行；③构造 `mark_stage1_job_failed`/phase2 失败后→阶段/原因/**下次可重试时间（`retry_at` 本值）**正确显示；④以含密钥样式超长文本作 failure_reason→展示为脱敏＋截断；⑤前后库文件字节/mtime 不变（无写入副作用）；⑥冷却行仅在按领取条件（store.rs:644-649 同式）可推导时出现（done＋无错＋6h 内），error 行与冷却行互斥呈现正确——展示的冷却判定与 `SkippedCooldown` 实际行为一致。

## 3. generate_memories 来源资格开关

- **问题**：本地无法「继续使用已有记忆但暂停新会话成为提炼来源」。上游四开关职责已经固定提交核定（对照报告 §9.2）：`Feature`＝总闸；`generate_memories`＝**仅**新登记会话的来源资格；`use_memories`＝仅注入；线程 `memory_mode`＝资格持久化状态。本地 `threads.memory_mode` 列已在 migrations/0003 预留且两处筛选（store.rs:966 候选、:1240 phase2 启用门）已按 `'enabled'` 过滤，仅缺写入面。
- **采用规则（与固定 Codex 一致）**：
  - `enabled`：保持现有总开关语义不变（管线/登记/注入/反馈全线门控）。
  - `generate_memories`（新增配置字段，serde 默认 `true`，缺省与原行为完全一致，不钳制）：仅决定**新登记**会话的 `memory_mode` 初值。`false` 时：原始会话照常保存、登记照常进行（新来源标 `disabled`）；**不**停止后台触发、既有合格来源的提炼、phase2 合并、30 天/14 天清理（上游调用链证据：generate 不在管线门控，E6）；**不**改变注入、引用反馈、状态、重置的既有职责。
  - `use_memories`：仅注入门（现状不变）。
  - `memory_mode`：资格权威。**既有行不因后续登记被全局配置覆盖**（对应上游 keeps-creation-mode：本地 `register_thread` 的 `ON CONFLICT DO UPDATE SET updated_at_ms` 现状已不触碰 memory_mode，天然满足）。
- **四条登记路径（按本地代码实况，不套用上游根外库结论）**：①新建会话（行不存在）→ INSERT 按当前 `generate_memories`；②正常恢复/继续写入（行存在）→ 冲突路径仅刷新 `updated_at_ms`，资格保持原值；③登记行缺失后的首次登记（行被外部删除/新库）→ 同①按当前配置；④reset 后重新登记 → 本地库在记忆根内、reset 整库删除 → 同①按当前配置重建。**本地适配（醒目标注）**：本地「资格随 reset 清除」是记忆库位于记忆根内这一既有布局下的明确适配，**与 Codex 的跨重置资格保留不同**——上游 threads 行存于共享 state 库（记忆重置不删 threads 行），且资格随会话文件 SessionMeta 持久、恢复时重放（threads.rs:1100-1102），故 reset 后资格存续；本地 threads 行即唯一权威且随库删除、会话文件不含 memory_mode。该差异是行为不同而非缺陷，与「reset＝清空生成记忆与状态」语义自洽。若未来要求资格跨 reset 存续，需在会话 meta 增可选字段（旧文件缺省视为 enabled 兼容）——**本轮不做**，仅此登记；本轮亦不增加 SessionMeta 持久化或逐会话切换命令。
- **两处筛选核实（已存在，验收覆盖即可，无改动）**：stage1 候选 `stage1_startup_scan`（store.rs:966）与 phase2 来源选择 `thread_enabled`（store.rs:1240）均 `memory_mode='enabled'`。**保留清理不新增 memory_mode 过滤**（store.rs:1099-1129 与上游 SQL 逐字一致，维持）。
- **本轮明确不做**：逐会话切换命令（上游 per-thread setter 不引入）；「保留来源资格但暂停全部后台处理」开关（上游无此组合，对照报告 §9.3，不得称上游同构）。
- **最小改动落点**：`config.rs` 增 `generate_memories: bool`（默认 true）；`store.rs::register_thread` 增 `memory_mode: &str` 参数（INSERT 值参数化，冲突路径不变）；`runner.rs::note_persisted` 传 `config.generate_memories` 映射 'enabled'/'disabled'。
- **验收场景**（拟新增，尚不存在；另以既有套件回归）：①缺省配置（无 memories 段/无该字段）与原行为逐字节一致（登记行 enabled）；②`generate_memories=false` 后新登记来源 disabled，**同一管线运行内既有 enabled 来源仍被领取并提炼**；③同一会话继续写入/再次登记→资格保持（冲突路径不改写）；④`generate_memories` 改回 true→旧 disabled 行**不被批量修改**；⑤两阶段筛选各设一条直接断言（disabled 行不入候选、其产物不入 phase2 选择；清理对 disabled 行照常按 30 天规则）；⑥职责互不混淆——`generate_memories=false` 时注入（use_memories=true）照常、反馈照常、`/memory-status`/`/memory-reset` 行为不变。

## 4. 两处注释勘误

- **问题**：两处现行注释与实现不符，误导维护者（对照报告 §8.2-6；本轮仅登记位置与拟改文字，不实施）。
- **位置 1**：`src/app/src/memory/runner.rs:36-49`。现文声称「承载＝记忆根内两个代际文件……`.pipeline-epoch`：管线 prepare 时刻的代数快照（写入者配对凭据）」——`.pipeline-epoch` 不存在（全库唯一提及即此注释）。**拟改为**：「承载＝`.epoch` 代数文件＋根目录 flock＋各任务领取时捕获的内存代数快照（task_epoch：stage1 每任务前、phase2 领取同临界区、mem_write 持任务快照在共享锁内校验）；写入入口在共享锁内『检查＋写入』同一临界区，与 reset 的排他清空互斥；阶段边界比对快照失配即中止（phase2.rs:163、mem_tools.rs:378-398、workspace.rs:365-367）」。
- **位置 2**：`src/app/src/memory/store.rs:10-11`。现文「`Uuid::new_v4()`→`new_token()`……避免新增 uuid 依赖」——uuid 已是依赖（src/app/Cargo.toml；storage_files.rs:17 用于文件名解析）。**拟改为**：「生成侧无需随机 UUID：纳秒-pid-计数器即满足 SQL 相等比较的唯一性目标（uuid 依赖仅用于既有文件名解析，不用于令牌生成）；令牌格式不参与语义」。
- **边界**：仅改这两处注释文字；不顺带清理其他注释、代码或格式。

## 5. 实施与验证要求（统一）

- **RED 先行**：§1 以对照报告探针 S1/S3 复现为问题基线，先落能够复现「无引用建库」「reset 后重建库」的失败用例再修复；§2/§3 新用例先写断言、在实现前实跑确认失败（列举不算通过）。**§4 注释勘误不需要行为测试或变异证明**——RED/GREEN 与错误检测仅针对 §1–§3 行为修订；两处注释只核对描述与当前实现一致（人工比对拟改文字与 phase2.rs:163/mem_tools.rs:378-398/workspace.rs:365-367 及 Cargo.toml 依赖事实），随实施轮全量回归一并通过即可。
- **副作用检查**：断言实际数据与副作用（目录/库文件存在性、jobs/stage1_outputs/threads 行值、文件字节/mtime），不能只断言函数返回值。
- **错误检测证明（有区分度的变异，恢复后复绿）**：§1——移除 NO_CREATE（恢复允许创建的 open）或恢复「先 open 后解析」顺序→验收①/④必须失败；§2——绕过展示侧脱敏/截断→验收④失败；查询改为写入→验收⑤失败；§3——`generate_memories` 误接成 spawn 管线门→验收②失败；误接注入门→验收⑥失败；登记冲突路径误改 memory_mode→验收③失败。
- **复跑范围**：受影响套件（memory_feedback、memory_store、memory_runner、memory_candidates、memory_phase2、memory_lifecycle）＋默认用户路径与 CLI/PTY 回归（行式管道＋TUI 冒烟：`/memory-status`、`/memory-reset`、含引用回复的用户轮不回归；e2e S89/S90 涉及反馈接线）。
- **终验**：`cargo check && cargo test --workspace --no-fail-fast`（013 收口的 49 套件 3010/0/3 仅为基线引用；实施轮以实际通过/失败/忽略/过滤数量报告）。
- **设施**：沿用现有测试设施（feedback_tests、集成测试、临时目录、env 隔离、可注入 Clock）；不新增抽象、框架或依赖。
- 拟新增测试当前**尚不存在、尚未运行**；本规格仅为契约，实施轮按 AGENTS.md 门禁（tdd → comprehensive-testing → Security Testing Gate 适用性 → code-review → 全量验证）另行执行。

## 6. 边界与不做清单

- 不改 013 已交付状态与既有规格结论；不重开 D2/D3/D4/D12-A/G8。
- 不新增：逐会话切换命令、「保留资格停全部后台」开关、per-thread API、会话文件 memory_mode 元数据、schema 变更、日志/遥测/统计、通用存储框架、新依赖。
- 反馈建库修复（§1）、失败展示（§2）、注释勘误（§4）与开关语义（§3）相互独立，可分别验收；§3 依赖 §4 无、依赖 §1 无（登记与反馈路径不同）。
- 与固定 Codex 一致必须保留的本地适配：轮级触发（D2，经 E5 确认与上游结构同构）；合并轮受限 mem 工具（D3）；system 尾段注入（D4）；D12-A 差异层过滤（mem_list/mem_read 旁路照常，E2）；外部 git 基线（G8）；数据库位于记忆根内（pathspec 排除适配）。
