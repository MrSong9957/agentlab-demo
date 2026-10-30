# 013-M7 运行时接入与命令面实施记录（2026-10-19）

> 2026-10-19 证据整理：当前核心附件见 [M7 证据说明](../testing/evidence/013-m7/README.md)，完整旧目录及当时源码见[原件索引](evidence-index.md#m7-originals)。下文执行命令、JSON 中的路径和历史 manifest 均对应当时快照；整理仅同步引用，原始文档与附件保留在 `9e32a63`，交付状态和各批判定不变。

- 状态：**M7 代码接线已交付（2026-10-19 用户批准后快进合并 main @ `67b9461`，见 §10）、离线验证完成**。真实供应商补验待授权、待验证。M8–M9 未实施，V1 整体未完成。
- 分支：`feat/013-memory-m7`；基线/合并目标：`main @ 80f777898cf24a74a9669c478da9e8d7250b7029`。
- 规格：`docs/specs/013-memory-system-v1.md` §6 M7（批准边界 2026-10-19 用户确认 D2/D9）；D4/D6 等既有决定有效。
- 固定上游：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`（本轮经 raw.githubusercontent 复核 `start.rs`/`control.rs`/`extensions/{mod,ad_hoc}.rs`；ad_hoc instructions 模板逐字节拷贝，sha256 `d36a36083d92f9d44efbd95e0e4b6e81d7d149812f2bca2009b6dd4b8aa93e7`）。
- 保护项：compose/AGENTS.md/.gitignore/两条安装记录（已入库随 main）、宗旨文档删除状态、`db/`（仅字节指纹，未打开）、`.agents/`（未触碰）；全程原样。临时证据：`/tmp/013-m7-impl-aa7b52aa/`（本轮）＋`/tmp/013-m7-precheck-a3150937/`（准备轮）。

## 1. 基线（实际基线，非旧树沿用）

按任务要求在实际基线 `80f7778` 上重跑（git archive 副本＋专用 `--target-dir`，不污染共享产物）：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` exit 0——**2944 通过/0 失败/3 既有忽略**（45 套件）。与 M6 交付树 `2aa0002` 的 2944/0/3 相同：其间 4 笔并发提交均为文档/配置/无引用垃圾文件清理，无生产代码变化——实测确认，非沿用推断（`baseline2.log`）。

## 2. 实现内容与来源复用

| 模块 | 内容 | 来源 |
|---|---|---|
| `memory/runner.rs`（新，~370 行） | D2 触发编排（生产壳＋可测核心 `spawn_memory_pipeline_in`）、`ThreadRegistrar`（threads 登记推进器）、`resolve_thread_meta`（phase2 真实水合）、`memory_status_text`/`memory_reset`（D9）、`load_memories_config`、`memories_root` | 编排序＝上游 `start.rs`（store→layout→seed→prune→配额→phase1→phase2；本地 prune 属 M9 不跑、配额 D8 跳过，登记）；重置＝`control.rs`（清内容/保留根/拒绝符号链接根）；全部执行体复用 M1/M2/M3/M5 已交付能力 |
| `src/app/src/session.rs` | `current_target_stem()`＋`durable_history_len()` 最小访问器（thread_id＝auto 文件 stem；变化检测用持久化事实） | 本地新增（上游 thread 元数据在 state 库，G6 最小表对等） |
| `src/app/src/main.rs` | cmd_chat 接线（每轮装载配置→D2 spawn→`UserWithMemory` mode→on_record 登记）；`handle_input` 加两命令 | D2/D9 批准边界 |
| `src/app/src/tui_host.rs` | `run_turn_inner` 同构接线＋`TuiHost.registrar` 字段；`run_command` 加两命令 | 同上 |
| `src/tui/src/slash_command.rs` | 枚举 `MemoryStatus`/`MemoryReset`＋description＋`supports_inline_args`（reset 带 confirm 内联参数）＋`available_during_task`（status true／reset false） | vendored 008-ADAPT 区最小追加，不整文件替换 |
| `memory/store.rs` | `count_stage1_outputs`/`count_jobs_by_status`/`thread_updated_at_ms` 只读观察 | `/memory-status` 支撑 |
| `memory/templates/extensions_ad_hoc_instructions.md` | 上游 ad_hoc 说明逐字节 | `extensions/ad_hoc.rs` create_new 播种 |

关键设计事实：
- **触发位点**＝route＋origin 门通过之后（门失败零触发）；「带输入」由驱动层空输入门保证；排队输入仅在重放（实际再进 `spawn_turn`）时触发——入队路径不经过 spawn 位点，结构上不可能双重触发（S11 既有排队回归＋位点分析，未加低价值重复断言）。
- **threads 登记**：`note_persisted` 仅在 `(stem, durable_len)` 变化时 upsert（毫秒墙钟）——未成功持久化的数据不登记为已更新版本；只读操作与自动注入块不产生新持久化记录、天然不推进；`list_sessions` 的字节数从不当地用作时间（毫秒登记→M2 `div_euclid(1000)` 秒级源版本，e2e-pipeline 用例断言两者）。
- **关闭态零回归**：`enabled=false` 不构造 `UserWithMemory`（默认 `User` 逐字节不变）、不 spawn、不建目录/库（变异证明）；既有 95 个 e2e（全部关态）原样通过。
- **phase2 首轮恒合并**（M5 P7）：开态首次管线即使候选为空也发起一次合并请求（S81 实测请求数＝用户轮 1＋phase2 1，非缺陷）。

## 3. 测试与关键错误检测

新增 `src/app/tests/memory_runner.rs` **15 用例**＋e2e **S81/S81b/S82/S83**（PTY 3＋管道 1）。全部经生产入口（`spawn_memory_pipeline_in`/`TuiHost.spawn_turn`/真实 CLI 进程）＋本地 mock HTTP＋临时目录；不访问真实用户目录/记忆根；等待全部有界（轮询 deadline＋慢响应 1.5s＋`WATCHDOG`）；后台阻塞用 `slow_json` 而非真实长等待。

**方法说明（如实）**：实现骨架与测试同轮推进（先搭接口后即写测试），非严格先 RED——关键行为以**独立副本 8 项「破坏→失败→恢复→通过」**证明检出能力（断言不变，`mutation/run_mut.py`，全部 Cargo 命令显式 `--target-dir` 副本专用）：

| # | 故意破坏 | 检测用例 | 红/绿 |
|---|---|---|---|
| 1 | 移除 enabled gate | `m7_disabled_config_is_zero_side_effect` | 101→0 |
| 2 | 登记恒推进（删变化检测） | `m7_registrar_advances_only_on_durable_change` | 101→0 |
| 3 | 无目标仍登记（ghost stem） | `m7_registrar_does_not_advance_on_failed_persist` | 101→0 |
| 4 | reset 缺 confirm 也执行 | `m7_reset_requires_confirm_and_preserves_sessions` | 101→0 |
| 5 | 符号链接根不拒绝 | 同上 | 101→0 |
| 6 | status 无库时建库 | `m7_status_reports_config_root_and_counts` | 101→0 |
| 7 | seed 覆盖（create_new→truncate） | `m7_enabled_pipeline_initializes_root_and_seeds` | 101→0 |
| 8 | TUI 注入漏接（mode 恒 User） | e2e `s81_m7_enabled_turn_injects_and_spawns_pipeline` | 101→0 |

变异轮暴露并修正两处**测试前提缺陷**（实现无缺陷）：fd 已打开时目录删除不构成写失败（Linux inode 语义——改用只读父目录制造目标创建失败）；`ensure_private_dir` 会自愈重建目录（直接删目录不触发失败）。修正后断言语义不变、更真实。

## 4. comprehensive-testing 对照（契约映射）

规格 M7 验收项逐项：触发条件矩阵（开/关/route 失败/命令零触发——S83＋m7_disabled＋S82 `mock.count()==0`）；排队重放（位点分析＋S11 回归）；后台阻塞时前台可响应（`m7_main_turn_completes_while_pipeline_blocked`：慢响应期间主轮完成＋后台未结束）；登记/追加/恢复切换/毫秒秒换算/写失败（reg 系列 5 用例）；M6 生产快照/下一轮重读/恢复/不入史（S81/S81b＋M6 既有 18）；真实两阶段链路与文件/库/Git 副作用（`m7_pipeline_processes_real_candidate_end_to_end`：stage1 严格 JSON→phase2 工具批→MEMORY.md/summary 落盘＋库断言＋请求体断言）；重复触发/并发/失败隔离（dedupe/main-fail/bg-fail）；两界面命令往返/缺确认零删除/A31（S82/S83＋库级）；reset 外部路径（符号链接＋sessions 保留＋在途连接）。测试规范：临时目录进程号＋受控计数、无真实长等待、无覆盖率凑数。

## 5. Security Testing Gate

手动按本地 Gate 文档执行（skill 入口不适用先例），报告落盘 `docs/security/reports/2026-10-19-memory-m7-runtime-security.md`：配置与路径（零新增信任根面；stem 无分隔符无遍历）、后台生命周期（静默/不 panic/不阻塞/退出不等待）、日志脱敏（失败原因入库前 redact_secrets；status 只显计数不显错误正文）、重置边界（confirm/符号链接/lstat 语义/A31/如实报告）、在途交互（busy_timeout/在途写已删 inode不污染新库）。未发现新增可复现缺陷；继承边界如实登记。

## 6. code-review（双轴自查）

- **Standards**：复用优先满足（零新调度框架；执行体全部既有）；无未使用代码（Default derive 化、mut 清理，clippy exit 0 仅既有告警——`slash_command.rs` 的 `available_when_thread_unavailable` 分析告警经基线副本对比确认为既有条件编译面，未动）；新文件 scoped rustfmt check 通过；注释密度/命名与项目一致。
- **Spec**：D2 六条＋D9 五条批准边界逐项落实（见 §2 表）；供应商实测未冒充（全 mock，状态如实拆分）；M8/M9 零提前（prune 不跑、引用反馈/便条未动）；vendored 修改限于 008-ADAPT 式追加。

## 7. verify:full（实际命令与退出码）

环境：`/workspace`，rustc/cargo 1.99.0，`TZ=UTC`。日志前缀 `/tmp/013-m7-impl-aa7b52aa/`：

| 命令 | 退出码 | 结果 | 日志 |
|---|---:|---|---|
| 基线 `cargo check`（副本 `80f7778`） | 0 | 0 错误 | `baseline2.log` |
| 基线 `cargo test --workspace --no-fail-fast`（副本） | 0 | **2944/0/3**（45 套件） | `baseline2.log` |
| `cargo test -p terminal-chat --test memory_runner -- --list` | 0 | 列出 15（仅存在性） | `final-list.log` |
| `cargo test -p terminal-chat --test memory_runner --test memory_inject --test e2e_acceptance` | 0 | **99/0/0＋18/0/0＋15/0/0** | `final-combos.log` |
| `cargo test -p terminal-chat --test session_cli --test prompt_assembly --test memory_phase1 --test memory_phase2` | 0 | **24＋6＋19＋40 全 0 失败** | 同上 |
| `cargo test -p terminal-chat --lib --test memory_candidates --test memory_store` | 0 | **129＋18＋21 全 0 失败** | 同上 |
| `cargo check`（最终） | 0 | 0 错误 | `final-full.log` |
| `cargo test --workspace --no-fail-fast`（实施轮） | 0 | **2963/0/3**（46 套件） | `final-full.log` |
| `cargo check`（定点补验轮后） | 0 | 0 错误 | `final-full2.log` |
| `cargo test --workspace --no-fail-fast`（定点补验轮后） | 0 | **2968/0/3**（46 套件；2963＋5：runner 2＋e2e 3） | `final-full2.log` |
| `cargo check`（收口轮后） | 0 | 0 错误 | `final-full3.log` |
| `cargo test --workspace --no-fail-fast`（收口轮后） | 0 | **2974/0/3**（46 套件；2968＋6：runner＋4、e2e＋2）。收口首轮全量曾现 S81 并发偶发（reqs[0] 顺序假设）——修复为内容定位后复跑全绿（如实登记） | `final-full3.log` |

对账：实际基线 2944＋M7 新增＝**2963**（memory_runner 15＋e2e 4：S81/S81b/S82/S83），无既有用例减少、无新增忽略；e2e 95→99、全量目标 45→46。静态：clippy `-p terminal-chat --all-targets` exit 0（新文件零告警；既有告警保留）；两个新 Rust 文件 scoped rustfmt check 通过。兼容性：无 Schema/持久化格式变更（threads 表/会话文件既有结构仅读）；`/memory-reset` 是新增命令名，不与既有命令冲突（含母本 `/memories`）。

## 7a. 定点补验轮（2026-10-19 第二轮：生命周期与重置）

按用户定点任务核实四项证据；已有覆盖如实引用、缺口补验：

**① 已有覆盖核实**：取消与退出无真实进程观察（既有 `m7_main_failure…`/`m7_main_turn_completes…` 均库级且 join 收尾）；S11 为关态排队重放（无触发计数断言）；reset 在途仅覆盖旧 SQLite 连接（`m7_reset_with_inflight_connection…`）——三项均补验。

**② 后台阻塞时的取消与退出**（新增 `m7_turn_cancel_does_not_undo_pipeline`＋e2e S85/S86）：
- 取消不解约：主轮经独立失败通道返回 Err（等价取消位）时后台在慢响应上继续并完成（断言取消时 `!handle.is_finished()`＋join 后库已建）。
- 真实进程退出：后台 phase2 请求悬于 **silent mock**（连接保持、永不响应——真实 HTTP 180s 超时量级）；TUI `/exit`（S85，PTY）与行式 `exit`（S86，管道子进程＋`try_wait` 有界循环）均在 **15s 窗口**内完成进程退出（再见＋码 0）——若实现等待后台则远超窗口必然失败。
- **变异证明 2 项**（独立副本）：退出路径插入 20s sleep→S85/S86 双失败（15.5s 超时）；恢复→通过。
- 两驱动退出入口各有专属用例（S85＝`run_tui` 返回→`set_cancel`→`flush_final_session`→exit；S86＝行式主循环 break→`flush_final`→进程结束）。

**③ 排队重放的准确触发次数**（新增 S84）：
- S11 未检查管线启动——补开态 PTY 用例：turn1（慢响应 1.5s）运行中入队「排队的输入」→turn1 完成后重放。断言**含该排队文本的用户轮请求恰 1 份**（0＝未重放／2＝双重放均可区分；用户轮请求不经 jobs 去重，非后台模型请求数）＋threads 登记存在。
- 「入队时不触发」：黑盒下后台管线启动次数无物理信号（jobs/全局锁去重掩盖——如实登记观察限制）；证据＝静态位点（vendored `chat_driver` 入队走 `queue_input`（648/706 行）与 `spawn_turn` 钩子（410/660 行）分离，重放唯一经 660 同一钩子＝「实际成功重放时触发」）＋S84 行为证据。工具续轮在 turn 内循环（不回驱动层）＝结构保证；拒绝启动不触发＝S83（route 失败零库重建）＋S82（命令零模型请求）既有证据。

**④ reset 与在途管线完整交互**（新增 `m7_reset_during_inflight_pipeline_full_interaction`）：
- 受控阶段暂停：真实管线（含候选）stage1 正常→**phase2 合并请求慢 2s**——以 `mock.count()>=2` 精确等到「claim/prepare（git 基线）/同步已完成、模型请求在途」的暂停点（并断言 `.git`/`rollout_summaries` 已存在）。
- 在途窗口执行 `/memory-reset confirm`（当时全清＋如实报告）→释放（慢响应到达）→管线走完。
- **三面文件级证据**（非「旧所有者无法 SQL 提交」推断）：文件面＝在途 `mem_write` 按路径**重建** `MEMORY.md`/`memory_summary.md`（非原子边界如实）；库面＝`memories.sqlite` 不复活（后台 SQL 落已删 inode）；Git 面＝`.git` 不复活、后台后续 git 操作（显式 `--git-dir` 指向已删路径）失败按 M5 契约记账、**记忆根外零 `.git` 泄漏**；sessions 原始会话保留。
- 契约对照：与已批准重置语义一致（清理当时内容＋保留原始会话＋非原子边界），无需改用户可见契约、无新增决策项；安全报告 §7 已登记。

**⑤ 验证**：`memory_runner` 15→**17**、e2e 99→**102**（S84/S85/S86）；受影响回归 102＋18＋17＋24 全绿；全量见 §7 表更新。变异累计 **10 项**（8＋退出等待 2）全部「破坏→失败→恢复→通过」。



## 7b. 收口轮（2026-10-19 第三轮：重置失效边界＋后台触发计数）

用户本轮确认重置边界（更正 §7a④「在途写回属已批准语义」的结论——观察属实、结论误判，历史保留；规格 §5 D9 与 §6 M7 批准边界已同步）：

**实现**（`runner.rs` 为主＋`mem_tools.rs`/`workspace.rs`/`phase2.rs` 最小侵入）：
- 代际凭据：`.epoch`（reset 排他锁内清空＋自增）×`.pipeline-epoch`（prepare 先于基线写入、随基线入库——不构成差异）；写入有效＝无 `.epoch` 恒真、有则相等。
- 竞态排除：reset（EX 锁：清空＋自增）与 mem_write（SH 锁：检查＋写入）同根目录 flock 互斥——排除「检查过→reset→才写」交错；跨进程有效。
- 入口覆盖：mem_write（锁内检查＋写）；phase2 收尾前凭据核对（`failed_reset_invalidated` 记账）；runner 阶段边界（首次建库〔锁内检查＋建库〕/seed 前后/每任务前/phase2 前）比对启动代数。
- SQL 实测：失效任务 job_failed 记账落已删 inode 连接——写事务失败被既有 `let _` 吞没、行保持 running（如实断言，非「文件未复活」推断）；新库零污染。

**测试（RED→GREEN）**：测试基建加 `MockResp::gated`（闸门屏障——测试信号精确释放，替代固定延时）。四场景：②改造（在途 phase2 闸门暂停→reset→释放**有效写入响应**→三面零重建＋记账如实）＋①新增（stage1 闸门暂停→reset→释放→不 prepare/不再请求）＋③新增（测试侧持 SH 锁→reset 阻塞→释放完成→写入全拒）＋④新增（reset 后新会话经登记写入侧重建＋管线 phase2 首轮合成产物——「不靠禁用过负向」）。正路径回归暴露并修复一处实现缺陷：凭据文件原在基线后写入构成未跟踪差异（M5 `git_diff…` 用例抓到）——调整为先写凭据再建基线。

**布局差异（如实登记）**：本地库在记忆根内→reset 连 threads 登记清除（上游库在根外保留）；reset 后非活动旧会话退出候选池（原始会话文件保留），当前活动会话照常重登记与生成——M1 布局固有代价，测试④头注释＋安全报告 §8 登记。

**触发计数**：生产壳 `spawn_memory_pipeline`（enabled gate 后）进程内原子计数；`/memory-status` 显示「本进程累计触发后台管线：N 次」（费用知情观察面，非日志/持久化审计/环境变量后门）。测试：库级（关态不增/每次启动各＋1/status 可见）＋S87（PTY：turn1 运行中入队后 status 计数 1＝入队不触发；重放完成 status 计数 2＝恰＋1，可区分 0/1/2；用户轮请求恰 1 互证）＋S88（关态轮后恒 0；开态 route 失败轮后恒 0）。

**变异（独立副本）**：m11 mem_write 守卫移除→②抓（RED✓）；m13 reset 排他锁移除→③抓（RED✓)；m14 计数×2→S87 抓（RED✓）；m12 阶段检查单独移除→无可观察差异（claim 写已删连接天然失败拦截）——**如实排除**（纵深防线、行为等价；阶段检查保留覆盖文件面窗口）。全部恢复后复绿。累计变异 **14 项**（10＋4）。

**计数更新**：`memory_runner` 20→**21**、e2e 102→**105**（S87/S88＋S8x 家族回归）；M5 目标 40/40 修复后全绿。全量见 §7 表。

## 7c. 借用凭据交错轮（2026-10-19 第四轮：旧任务不得借新代凭据恢复写入）

**源码核实（凭据绑定现状）**：第三轮实现中，任务自身不可变代数快照只在 runner 阶段边界（`run_pipeline_once` 启动捕获）；`mem_write` 与 phase2 收尾（9a）校验的是**共享配对文件**（`.pipeline-epoch`×`.epoch`，`pipeline_writes_valid`）——共享文件说明「记忆根当前属于哪一代」，会被**最新一次 prepare** 改写。**缺陷成立（源码级）**：reset（`.epoch=N`）后新代任务 prepare 写 `.pipeline-epoch=N`→配对恢复 true→旧任务（代 N-1）的 mem_write/收尾校验重新通过——旧任务借新代凭据恢复写入。

**RED 复现**：组合交错测试 `m7_old_task_cannot_borrow_new_epoch_credentials`——旧 phase2 请求挂闸门→reset 成功→**真实新代管线**完整跑完（新会话登记/prepare/新代产物/基线收尾/新库任务 done）→释放旧任务（携带独特旧内容 `OLD-CONTENT`/`OLD-ONLY.md` 的**有效**工具批，无租期过期/违规响应/HTTP 超时）→RED＝旧任务覆盖 `MEMORY.md` 为旧内容。

**最小修复（任务快照贯穿，无新配置/框架）**：
- `phase2` 在**全局任务领取成功后**绑定 `task_epoch = read_epoch(root)`（任务领取时点、函数局部＝不可变）；
- `ToolMode::MemoryMerge` 加 `expected_epoch` 字段（加法变更）→turn 分派传 `mem_tools::execute(…, expected_epoch)`；
- `mem_write` 锁内校验 `read_epoch(root) != expected_epoch → 拒`（不再读共享配对；flock 互斥保留——检查＋写入同临界区）；
- phase2 prepare 前/同步前/9a 收尾前以 `read_epoch != task_epoch` 校验（`failed_reset_invalidated` 记账）；
- 删除共享凭据机制（`begin_pipeline_write_epoch`/`pipeline_writes_valid`/`PIPELINE_EPOCH_FILE`/prepare 写入）——共享面只剩 `.epoch`（当前代数），任务凭据只在任务内。
- 跨进程不退化（`.epoch` 文件＋flock 均共享介质）；reset 后新任务照常（③测试补「新代快照放行」正断言；借用测试④段证明新代完成）。

**GREEN 断言**（文件级证据，非「返回 Failed」）：旧任务不覆盖新代 `MEMORY.md`（新内容保持）、不新建 `OLD-ONLY.md`、新代摘要保持、**新代 Git 基线 HEAD 逐 sha 不变**、新库 job 仍 done 且 stage1 零污染；新代任务确实完成（产物＋基线＋job done）。

**变异**：m15（mem_write 快照校验移除→借用测试 RED✓）；m16（9a 移除未检出——旧任务 heartbeat 写已删连接失败在所有权再确认步天然拦截〔②实证同因〕，9a 属纵深防线，行为等价如实排除）。累计变异 16 项。

**适配面**：`memory_phase2` 两处 `MemoryMerge` 构造＋`execute` 调用、`memory_inject` 一处构造补 `expected_epoch: 0`（无重置场景＝代数 0 恒过，断言语义不变）；③互斥测试改为任务快照形态（失效快照拒绝＋新代快照放行双断言）。

**计数**：`memory_runner` 21→**22**；全量 **2975/0/3**（2974＋1，对账一致）。候选文件见 §9（本轮新增触及 `turn.rs`＋两测试构造适配）。

## 7d. 锁边界核查轮（2026-10-19 第五轮：任务代数绑定与实际写入的锁域）

**核查结论（源码级）——两个窗口均存在，已 RED→结构修复→回归**：

- **窗口 1（任务凭据绑定）**：第四轮实现中 `claim`（SQL）与 `task_epoch = read_epoch(root)`（绑定）**不在同一锁域**——reset 插入两者之间时旧任务把重置后的新代数读作自身凭据（prepare/写入全放行）。
- **窗口 2（检查与修改分离）**：prepare 前检查、同步前检查、9a 检查与各自的实际修改（布局/git/Markdown/基线收尾）均在锁外连续语句——reset 可插在检查后、修改前（或修改中途）；runner 的 seed 同样。

**修复（锁域扩展，无新框架）**：`runner` 提供 RAII `SharedWriteGuard`/`lock_shared`（根目录 SH flock，跨进程）；phase2 四个临界区——①「claim＋快照绑定＋检查＋prepare」（领取与绑定同临界区＝任务凭据不可能取到重置后的代数）；②「检查＋Markdown 同步」；③「9a 检查＋所有权再确认＋Git 基线收尾」（上游序 9a→10→11 保持，heartbeat 毫秒级 SQL 入区无碍）；runner 的「复核＋seed」同入临界区；mem_write 换用统一 guard。**锁不越过模型请求**：turn（含 gated 悬挂）期间不持锁——reset 仍不等待模型请求。

**修复中发现的实现陷阱（如实）**：首轮改动误用 `drop(&claim_guard)`（**no-op**——丢弃引用）→guard 活到函数尾→turn 悬挂期间 SH 被持有→reset 阻塞至 HTTP 180s 超时（②实测 187s）。修正为 `drop(claim_guard)`（move）；②补**显式时限断言**（reset＜10s——「不等待在途模型请求」的直接表达）。

**核查表（修复后）**：

| 操作 | 任务凭据 | 锁的取得/释放 | 修改在保护范围内 |
|---|---|---|---|
| 首次建库（`open_store_if_valid`） | 启动 epoch | SH：检查＋建库同区 | ✓ |
| claim＋快照绑定＋prepare（phase2 §1-2） | 绑定动作本身 | SH：claim→read_epoch→检查→prepare（git） | ✓（领取与绑定不可分） |
| Markdown 同步/差异文件（phase2 §5-7 前） | task_epoch | SH：检查＋sync 同区 | ✓ |
| mem_write | expected_epoch（mode 传递） | SH：检查＋写同区 | ✓ |
| 9a＋heartbeat＋Git 基线收尾 | task_epoch | SH：检查＋git reset 同区 | ✓ |
| seed（runner） | 启动 epoch | SH：复核＋播种同区 | ✓ |
| 模型请求（turn/chat） | —（不持锁） | 无锁 | —（reset 不等待模型请求） |

**变异**：m17（主路径 drop 改为越界持有→②时限断言 191s 失败✓检出、恢复 0.17s 通过）；m18（claim 区间去锁→无稳定检出——窗口＝线程内纳秒级语句间隙、无外部可插屏障——**如实登记**：该保证由锁域结构（claim 与 read_epoch 同一临界区）承载，m17 证明锁域行为可检）。累计变异 18 项。

**验证**：受影响回归全绿（runner 22/phase2 40 等）；`cargo check` exit 0；全量 **2975/0/3**（46 套件；本轮无新增用例——②为既有用例强化断言，代码变化经全量实测）。候选文件数不变（21）。

## 8. 限制与待验证（如实）




- **真实供应商补验待授权、待验证**：phase1 结构化输出真实可解析性、phase2 合并请求、注入开态用户轮（唯一 system＋尾段记忆块）的供应商接受度——本轮全部 mock，不以结构断言宣称语义等价。
- 后台日志完全静默（无本地日志面）：失败仅可经库/`/memory-status` 观察——登记差异（上游 tracing warn）。
- 排队「入队时不触发管线」的黑盒观察限制：后台管线启动次数无物理信号（jobs/全局锁去重掩盖）——以 vendored 位点静态证据（`queue_input` 与 `spawn_turn` 分离、重放唯一经同一钩子）＋S84 重放恰一次行为证据支撑（定点补验轮 §7a③）。
- ~~reset 非原子边界：在途管线于重置后可能按路径写回部分 Markdown~~（§7b 收口轮已按用户确认边界实现失效——旧任务不得回写/重建；既有非原子**失败**边界〔部分文件变化不回滚〕不变）。
- Linux 语义依赖：reset 在途写落已删 inode、只读父目录制造创建失败（unix cfg）；Windows 未实测（与项目现状一致）。
- M8（引用反馈/便条）、M9（30 天 prune/整链）未实施；本轮不引用上游 prune/引用反馈代码。

## 9. Git 与候选

当前 `feat/013-memory-m7`，未暂存/未提交。M7 可审阅候选文件共 **21** 个（实施轮 13＋收口轮 5＋借用轮 3：`turn.rs`〔快照字段/分派〕与 M5/M6 测试构造适配 `memory_phase2.rs`/`memory_inject.rs`——文件数如实变化，不守原数）：

| 类别 | 文件 |
|---|---|
| 实现（5） | `src/app/src/memory/runner.rs`（新；含失效边界＋计数）、`src/app/src/memory/mod.rs`、`src/app/src/{session.rs,main.rs,tui_host.rs}` |
| 失效边界（4） | `src/app/src/memory/{mem_tools.rs,workspace.rs,phase2.rs}`（写入守卫/收尾核对）＋`src/app/src/turn.rs`（MemoryMerge 快照字段/分派传参——借用轮） |
| vendored（1） | `src/tui/src/slash_command.rs`（008-ADAPT 区追加） |
| 模板（1） | `src/app/src/memory/templates/extensions_ad_hoc_instructions.md`（上游逐字节） |
| store（1） | `src/app/src/memory/store.rs`（只读观察方法） |
| 测试（6） | `src/app/tests/memory_runner.rs`（新，22 用例）、`src/app/tests/e2e_acceptance.rs`（＋7 用例）、`src/app/tests/common/mod.rs`（＋Gate 闸门基建）、`src/app/tests/llm_flow.rs`（构造器适配 3 处）、`src/app/tests/{memory_phase2.rs,memory_inject.rs}`（MemoryMerge/execute 构造补 `expected_epoch`——借用轮适配） |
| 文档（3） | 规格 013、本记录、安全报告（`docs/security/reports/2026-10-19-memory-m7-runtime-security.md`） |

本轮不暂存/不提交/不合并/不推送/不删分支；未调用真实模型、未访问真实记忆目录。等待用户批准提交、合并与任务分支清理。

## 10. 批准交付与合并核验（2026-10-19）

用户批准范围：M7 代码接线及离线验证交付＋精确提交＋快进合并 main＋安全删除任务分支；不含真实供应商补验、推送或开始 M8–M9。

- **交付前核验**：分支 `feat/013-memory-m7`，HEAD＝main＝基线 `80f777898cf24a74a9669c478da9e8d7250b7029`（快进前提成立），stash 0、单工作树。21 候选路径逐项核对为**最后一轮锁边界修复版本**（任务代数贯穿 turn/mem_write/收尾、claim＋绑定＋prepare 同锁域、同步/写入/Git 收尾检查与修改同临界区、模型请求不持锁、计数与补验在位）。
- **验证对应（非沿用）**：因锁边界轮后对三文件执行过 rustfmt，交付前对当前精确代码**实跑** `cargo check` exit 0＋全量 **2975/0/3**（46 套件，`delivery-final.log`）——候选与最终验证直接对应。
- **保护项核对**：compose/AGENTS.md/.gitignore/两条安装记录（已入库随 main）与宗旨文档删除状态、`db/wiki.db`（`c28522c4…`，未打开）、`.agents/` 三文件指纹全程一致；并发 `CLAUDE.md`（单行 `@AGENTS.md` 转发，10-18 生成）保留未跟踪、不入库。
- **精确暂存**：21 路径逐项显式 `git add`（首批因一条笔误路径整体未生效，已发现并重新正确暂存——如实登记）；暂存恰 21 项、`git diff --cached --check` exit 0；暂存版规格仍为待批准态（交付标记留本节文档提交翻转）。
- **实施提交**：`67b9461c12101f2cb4242bce43ad1c400d1f907f`，`feat: 013-M7 记忆后台管线与生产接线`，21 文件（3714+/470-），树 `7d7b761f045896c92c7009077ca7b0e06f5fce6e`，父＝基线 `80f7778`；提交后剩余工作区差异仅并发 `CLAUDE.md`（未跟踪）。
- **快进合并**：`git checkout main`（并发文件随切换保留）→ `git merge --ff-only feat/013-memory-m7` 成功（Fast-forward，80f7778→67b9461）；`merge-base --is-ancestor` exit 0；`git diff --exit-code 67b9461… main --` 为空——快进未引入候选外变化，按授权引用交付前实测 2975/0/3，不机械重跑。
- 交付文档更新（本记录 §10＋规格状态翻转）单独提交；任务分支随后 `git branch -d`（包含关系前置核验已过、不强制）。**保留原文不改写**：真实供应商补验待授权／待验证；m18 未稳定检出由锁域结构承载；其余等价变异排除（m12/m16）与继承限制（后台静默、Windows 未实测、threads 随 reset 清除的布局差异等）如前；M7 整体与 V1 不标全部完成，M8–M9 未实施。

## 11. 真实供应商补验前置核查与待授权方案（基于 main @ b731542）

**状态：M7 代码接线已交付；真实供应商补验待授权／待验证。** 本节只登记本地只读核查和后续调用边界，未执行任何真实 HTTP 请求、连通性探测、`/models` 或开态应用；不重开实现审查，不重跑 §10 已交付的 **2975/0/3** 离线门禁，不开始 M8/M9。规格历史「各 1–2 次固定输入真实调用」不是 HTTP 预算；phase2 含工具续轮，准确授权预算以本节为准。

### 11.1 Git 与保护项

- 起点：`main`，HEAD **`b731542800a6057e43b4db23e930c3f3ddb20465`**，符合任务预期；暂存区及已跟踪工作区为空，stash **0**；唯一未跟踪项 **`CLAUDE.md`**（内容为 `@AGENTS.md` 转发）。已读取 `/workspace/AGENTS.md`；涉及目录无更近的 `AGENTS.md`。
- **收尾并发变化（非本 Agent 操作）**：HEAD 前进为 **`799fbb972c7c4b500cd01d3f20ee99055a01ac6b`**，其父为预期 `b731542…`；该提交仅新增入库 `CLAUDE.md`，转发内容字节摘要与起点一致，故它现在是已跟踪文件。最终 HEAD **不再等于预期**，如实保留、不回退；生产源码未随此提交变化，本节仍以已交付 `b731542` 源码为依据。后续授权执行前须再次核对实际 HEAD 与配置。
- 保护项原样保留：`CLAUDE.md`、`db/`、`.agents/`、已入库 `.devcontainer/compose.yaml`／`AGENTS.md`／`.gitignore`、两条安装记录（2026-10-02、2026-10-18）和其他并发内容。`docs/Agent开发核心宗旨.md` 在 HEAD 及磁盘均不存在，保持既有已入库删除状态。
- 核对方法：转发文件、已入库保护文件及 `.agents/` 文件作字节摘要前后比对；`db/` 及已知 `db/wiki.db`/sidecar **仅核对元数据，不打开数据库、不遍历内容**，不宣称已读取验证其内容。未访问真实 sessions 或 memories 目录。临时核查基线只写入 `/tmp/m7-preflight-{baseline,read-files}.json`，不含凭据值。
- 本 Agent 仅追加本记录及规格的一处引用；不建分支、不暂存、不提交、不推送，不执行 stash/clean/reset。文档 `git diff --check` 已通过，保护项摘要/元数据核对通过；最终暂存区为空、stash 0，工作区仅这两份文档修改，无未跟踪项。真实 HTTP 请求 **0**，本轮离线测试未运行（沿用 §10 交付证据，不算本轮重测）。

### 11.2 本地配置事实（不等于服务端可用性验证）

装载依据：`main.rs` 启动时补充 `.env`（既有环境优先、信任根变量跳过）；`catalog.rs::{config_dir,effective_connection,merge_final,route}` 按内置→自动目录→手动覆盖。为保持只读，直接解析既存文件，不调用缺文件时会创建默认配置的 `load_manual/load_auto/build_final`，不运行启动权限审计或应用。

| 项目 | 本轮本地确认 |
|---|---|
| 配置目录 | `/workspace/config`，`manual.json` 与 `auto_catalog.json` 均存在、可解析 |
| 选中连接 | `selected_provider=deepseek`，`selected_model=deepseek-flash`；协议 **`openai-chat`** |
| 连接齐备性 | HTTPS 地址、凭据引用齐备；选中模型在有效名单中、未禁用；自动查询名单身份仍匹配；**凭据：存在** |
| 允许申请授权的目标 | origin **`https://api.deepseek.com`**；唯一模型端点 **`POST /chat/completions`**；配置 URL 无 userinfo、查询串或 fragment |
| 当前有效请求参数 | **`request_params={}`**；无 `response_format`、`max_tokens`、`max_completion_tokens`、thinking 或其他可选参数；不从模型名推定这些能力 |
| 模型目录元数据 | `context_length=1,048,576`、`max_output=393,216`；仅为本地目录值，未向供应商复核。**`max_output` 不会自动写入请求，不能视作实际生成或费用硬上限** |
| 当前网络行为 | `api.rs::agent`：连接 **10s**、每请求总超时 **180s**；不跟随重定向；请求/响应体各有 **10 MiB** 既有硬上限 |
| 自动重试 | API/turn 不做错误自动重发。jobs 默认剩余次数 3、失败退避 3,600s，须以后再次领取才会重试，并非单次函数内部额外调用 |
| origin 授权 | 用户授权文件中**没有匹配当前连接及凭据的授权**；后续只可在临时用户目录授予本次范围，不改真实授权文件 |
| 当前记忆开关 | 无 `memories` 段，effective 默认 `enabled=false`、`use_memories=true`；本轮没有开启 |

三项可复用同一个 provider/model/协议/凭据快照；这是配置复用结论，供应商对 JSON 输出、工具和 system 尾段的实际接受度仍待验证。不切换 provider/model。价格、账户额度、远端默认输出 token 上限及实际计费均**未知**，本地没有足以支持费用估算的依据，不编造金额。

### 11.3 拟批准的三项补验

授权后用 `/tmp` 内一次性最薄宿主直接调用现有库；本轮不创建宿主、不修改生产代码、测试或配置。三项顺序执行，任何一项未满足通过标准即停止本批，后续项记未验证，不为获得通过追加请求。

| 项目 | 实际调用入口与合成输入 | 可观察通过标准 | 最大真实 HTTP 请求数 |
|---|---|---|---:|
| ① phase1 | 临时 sessions 中 1 个固定 V1 JSONL 合成会话，正文 ≤8 KiB：虚构项目 `cobalt-demo`，用户重复确认中文及验收清单偏好，含虚构故障 `SYNTH-E17` 的处理经验，明确全为合成证据。通过 `MemoryStore::register_thread`、`claim_stage1_jobs_for_startup(max_claimed=1)` 领取后，调用 **`phase1::run_stage1_job`**；chat 闭包调用 **`api::chat_with_body`** | 供应商返回有效 chat 响应；原始 content 经既有 `StageOneOutput::parse` 严格三键合同及三态提交，预期 `SucceededWithOutput`、对应 `stage1_outputs`、job `done`/源水位一致。合法空结果如实记 `SucceededNoOutput`：可解析/空态成功已观察，有内容落库未验证，本批不报全通过。解析/HTTP/存储错误为 `Failed`，记错误/退避且无违规结果入库。**一次真实样本不宣称覆盖三态全部分支** | **1**（首次 1＋内部重试 0＋工具续轮 0） |
| ② phase2 | 独立临时记忆根/库；通过既有 register→claim→`mark_stage1_job_succeeded` 路径离线种入 **1 条**合成来源（≤8 KiB），不额外调 phase1、不依赖①结果。合成 resolver 仅返回临时 sessions/cwd。调用 **`phase2::run_phase2_consolidation`**，复用 **`turn::run_tool_turn_with_mode(ToolMode::MemoryMerge)`** 和原合并模板；sender 调 `api::chat_with_body`，只开放既有 **`mem_list/mem_read/mem_write`** | 请求工具恰三项；实际观察三项工具的成功调用及结果进入后续请求（工具数和 HTTP 数分开记账）；仅临时根变化。`MEMORY.md` 为有效常规文件并包含合成来源经验；`memory_summary.md` 首行恰 `v1` 且有合成内容，既有 `validate_consolidation_artifacts` 通过、无残留符号链接；模型以非空终文自然结束，返回 **`Phase2Outcome::Succeeded { sql_committed: true }`**，job `done`、选中来源/水位与 Git 基线收尾一致。无模型请求短路、漏工具、空文件、校验失败、工具错误、达到上限或 SQL 未提交均不得算通过 | **8**（首次 1＋至多 7 次工具续轮＋内部重试 0；即现有 `MAX_MODEL_REQUESTS_PER_TURN`） |
| ③ 注入开态用户轮 | 第三个临时根预置合成摘要 `v1\n合成环境代号：COBALT-17。`（≤1 KiB）；新空 history，内存配置 `enabled=true/use_memories=true`；调用 **`turn::run_tool_turn_with_mode(ToolMode::UserWithMemory)`**，问题为「仅根据给定合成摘要回答环境代号，不调用工具」；sender 调 `api::chat_with_body` | 实际待发体恰一条首位 system，基础前缀保持、尾段恰一处 `<memory-context>`、含合成摘要；仍为既有两项用户工具定义、无 developer/第二条 system。供应商接受并返回含 `COBALT-17` 的非空正式回复，真实 turn 正常返回，自动记忆块不在 history/临时会话中。若返回任意非空 tool_calls，宿主在把响应交给 turn 前直接报错，**不执行用户工具、不续轮**。只证明此输入的接受度与实际响应，**不证明 system/developer 语义等价** | **1**（首次 1＋内部重试 0＋工具续轮 0） |

**三项合计至多 10 次真实模型 HTTP POST（1＋8＋1）**。预算不是「一次 phase2＝一次请求」，也不是目标必定用满；提前终文即结束。发送前同时检查单项及全局预算，失败、超时、断连的发送尝试也消耗名额；任何未用额度不得跨项挪用。记录发送尝试数、已收到响应数及可得的供应商响应 ID；无法确认服务端是否收到/计费的最后一次请求单独记未知，绝不自动补发。

### 11.4 限制与停止条件

- **保持协议/模板**：三项沿用当前空 `request_params`、既有模板、解析和角色适配，不加 `response_format`、`max_tokens` 或强制工具选择，不修复/剥围栏/补字段后冒充原输出可解析。输出 token 参数的 `supported_params` 当前未知，未经本地能力确认不擅自加入白名单；若审批要求**生成/计费 token 硬上限**，须先补足该能力依据和限定值，再修订授权范围，当前方案不具备此保证。
- **输入**：上述单来源/摘要大小；每次实际最终请求 JSON（含 model、模板、工具及续轮历史）在宿主侧预检 **≤256 KiB**，超限在调用 API 前拒绝发送，不删减模板绕过。phase1 仍走原窗口 70% 内容预算（本地元数据计算为 734,003 近似 tokens，8 KiB 合成源远小于它）；注入正文仍走原 2,500 近似 token 预算，不更改实现。
- **输出**：保留 API 原响应体 **10 MiB** 上限；每次返回 JSON 在宿主重序列化后另限 **64 KiB**，超限整次失败，不截断修复。phase2 单次写仍受既有 **1 MiB** 工具上限、单批 ≤32 调用约束。宿主大小门限只限制本地接受/后续处理，**不会约束服务端已经生成的 token 或费用**；当前默认生成上限仍未知。
- **HTTP 内部重连**：锁定 `ureq 2.12.1` 对已发 body 的 POST 不做响应失败重试，但复用池连接时存在请求头发送失败后重连一次的分支（registry `src/unit.rs::connect_inner`）。补验 sender **每次新建一个既有 `api::agent()`，完成本次后丢弃**，不跨请求复用 Agent/连接池，消除该分支，保持 10s/180s 与 redirects=0。因此上表内部 HTTP 重试为 0；复用的是连接配置，不要求复用 TCP socket。
- **时间**：单请求连接 10s/总计 180s；额外外层 watchdog：①240s、②1,800s、③240s，三项执行总计 ≤2,280s（38 分钟，不含离线准备）。阶段内每次发送前查单调 deadline；watchdog 到期终止宿主及其子进程组，不再发请求。Git 沿用单命令 60s 既有上限。终止在途等待不能保证服务端停止生成/计费，按未知记录，不重试。
- **失败停线**：origin/配置不符、401/403/429、其他非成功 HTTP、3xx、网络/超时、空/截断/违规响应、输出门限、phase1 非预期分支、phase2 工具结果错误或产物/所有权/SQL/Git 收尾失败均停止本批；工具错误最迟在下次 sender 检查既有 tool 结果时拦截进一步 HTTP，不允许通过后续模型请求修复。不得推进时钟越过失败退避、再次领取、自动换供应商、删输出上限重发或扩大来源/轮次。取消/失败的部分临时文件按既有非原子语义如实登记，不伪报成功。

### 11.5 隔离、授权及结果记录

- 授权后建立独占 `/tmp` 私有目录（0700），三项各用独立 `user/sessions/memories/cwd`；覆盖 `HOME`、`APPDATA`、`XDG_CONFIG_HOME`、`XDG_DATA_HOME`、`TCHAT_USER_DIR`、`TCHAT_CONFIG_DIR`、`TMPDIR`，固定 `TZ=UTC`、`LC_ALL=C`。配置快照仅在内存；不复制整个 `.env` 或真实用户目录。只给宿主本次选中凭据的必要环境/内存值，不放命令行、请求正文或记录。所有业务来源时间由既有 `Clock` 固定为 `T0=1,700,000,000`，合成源更新时间 `T0-7h`，不靠真实时间触发候选；HTTP/watchdog 使用单调计时。
- 仅在这个临时用户目录调用既有 `grant_origin/ensure_origin_authorized`，绑定上述 **单一 origin＋当前凭据**；本次明确批准必须包含这条临时外发授权。临时授权不改真实授权文件、不授权 `/models`、公共目录、备用 origin 或其他模型。授权后若 provider/model/协议/URL/参数/凭据发生变化，停在本地核查，不沿用本方案外发。
- **没有后台触发入口**：不启动 `cargo run`、真实 CLI/TUI、`main::cmd_chat`、`TuiHost::spawn_turn`、`spawn_memory_pipeline(_in)`；不依赖 `use_memories=false` 或候选为空来「关闭」后台（phase2 首轮即使空候选也会请求）。①直接单任务（候选扫描仅临时 sessions），②直接单次合并，③只有库级用户轮；不做 refresh、模型目录查询、真实会话扫描/恢复或 M8/M9。③的所有用户工具调用在分派前拒绝。由此本批后台管线启动数为 0，只有一个串行 sender 可外发。
- ③的 `on_record` 只接临时 `Session::on_history`，正常返回后 `flush_final`，不接 `ThreadRegistrar` 或后台入口；检查实际写入的用户/助手记录非空且不含自动记忆块，避免以空会话文件冒充「不入史」证据。②仍为既有纯内存内部合并会话。
- 若需编译一次性宿主，在 `/tmp` 独立目录、沿用仓库锁定依赖并离线构建，显式指定该目录专用 target，不能使用全局 `/workspace/target`；不改仓库清单/lockfile。宿主准备及预算/拒绝分派自检须无网络；不能满足隔离、锁版本或有界发送就零真实调用并报告缺口。本轮尚未创建/编译/自检该宿主，不把计划写成已通过。
- 后续结果**追加本节**：实际 HEAD/provider/model/协议及脱敏端点；各项首次/续轮/重试、发送尝试和收到响应的实数；实际请求参数、输入/输出大小、可得 usage、finish_reason/响应 ID；phase1 解析及三态/job/源水位；phase2 工具调用与结果、两产物和 v1 校验、SQL/选中来源/Git 基线；注入结构/实际回复/正常返回与不入史；耗时、退出状态、失败原因及所有未验证项。未返回的 usage/服务端接收或计费不能推算为零。
- 仅保存合成请求/响应正文和必要状态证据；写入记录前以当前凭据整值替换并复用既有秘密脱敏，**不保存凭据、认证头或敏感 URL**，不启用 HTTP header/debug trace。临时授权/凭据不并入证据；汇总脱敏结果后停止全部进程并清理临时运行目录。若有不敏感证据附件，只放本记录对应位置并引用，不另建重复计划。

**尚缺条件**：用户明确授权本节真实调用及临时 origin 外发；服务端支持/账户可用性待本次请求观察；若要求计费 token 硬预算，另缺参数能力确认与明确数值。连接配置与凭据在本地齐备，不要求用户在聊天中提供密钥。本轮止于计划，不开始上述执行。

### 11.6 已授权单批执行结果（2026-10-19）：本地 harness 失败，真实请求 0，待审阅

**最新状态：M7 代码接线已交付；本次真实供应商补验未完成，供应商场景仍未验证。** 用户已明确批准本节范围，不再请求同一授权；§11 开头及 11.5「待授权」是准备轮历史。本批已经启动并按首项失败规则封闭，**没有修正后续跑、重开批次或复用剩余额度**。不是供应商拒绝，不能标整批通过。M8/M9 未实施，V1 不标完成。

**交接与隔离事实**：起点 `main @ 799fbb972c7c4b500cd01d3f20ee99055a01ac6b`，M7 实施提交 `67b9461c…` 为祖先；既有两份计划文档修改完整保留，暂存区空、stash 0。确认分支名未占用后，从实际 main 建立普通分支 **`test/013-memory-m7-provider-check`**，未创建 worktree。临时私有根 **`/tmp/013-m7-provider-dxei6mir/`**，harness 与锁版本源码副本/专用 target 全在此处；不使用共享 `/workspace/target`。临时运行根及 `run/phase1` 启动目录已建，但进程在 `run_phase` 之前退出，**尚未创建阶段 user/sessions/记忆库/记忆根/临时 origin 授权，也未解析使用真实凭据**（前置仅对 `.env` 等作保护摘要）。没有访问真实 sessions、memories 或打开真实数据库。

**获准目标（实际没有发送）**：`deepseek / deepseek-flash`，`openai-chat`，唯一端点 `https://api.deepseek.com/chat/completions`，`request_params={}`；本批没有新增 `response_format`/输出参数、换供应商/模型、连通性探测、`/models` 或重定向跟随。

| 阶段 | 实际 HTTP 发送尝试 / 累计 | 请求大小、HTTP 状态、usage | 实际结果与未验证项 |
|---|---:|---|---|
| phase1 | **0 / 0** | 未生成最终请求；HTTP 状态不适用；usage 未知（无请求/响应） | 启动进程 **2 ms、退出码 2**，stderr=`batch scope rejected`，未超时。未进入 API、严格解析、三态分派和存储入口，故未验证有内容成功入库，也未观察合法空结果 |
| phase2 | **0 / 0** | 未发送；HTTP/usage 无观测 | **未执行**：首项本地失败后停批；工具执行、两产物、摘要首行 `v1`、正常终文及 SQL 提交均未验证 |
| 注入用户轮 | **0 / 0** | 未发送；HTTP/usage 无观测 | **未执行**：没有供应商接受度、`COBALT-17` 实际回复或正常返回证据；更不宣称 system/developer 语义等价 |

**总计真实 HTTP 尝试 0/10，未用额度 10，已封闭不可继续使用**；内部 HTTP 重试 0、额外后台触发 0。没有任何供应商返回的 usage 或计费数据，费用不作估算。原始 batch summary 中 `known_usage_subtotal=0` 是对空响应集合求和，**不是供应商返回了 usage=0**。原始[进程结果（原件快照）](<evidence-index.md#archive-86609132ce2d6e3b>)、[逐项汇总（原件快照）](<evidence-index.md#archive-4edf91a35bf4ee78>)、[计数账本（原件快照）](<evidence-index.md#archive-88e6e429e92362bc>)保留不改写；`closed=true`、`authorization_consumed=true`、`further_batch_permitted=false`，无 `http_attempt` 事件。

**本地失败原因与检验缺口**：本 Agent 编写的 [harness（原件快照）](<evidence-index.md#archive-695335d98badc476>) 第 395 行用 `Path::starts_with("/tmp/013-m7-provider-")` 判断临时目录前缀。Rust 该方法比较**完整路径组件**，`013-m7-provider-dxei6mir` 不等于组件 `013-m7-provider-`，因此合法目录被拒绝。独立[纯离线复现（原件快照）](<evidence-index.md#archive-458edbc40835ac12>)输出 `path_component_prefix=false string_prefix=true`，见[定位证据（原件快照）](<evidence-index.md#archive-1bf0109590553a79>)。未修改失败版本或再次启动真实入口。这是 harness 缺陷，不是供应商失败，也不改变 M7 已交付代码的离线结论。

**发送前已做的离线检查及其限度**：临时副本 `cargo test --offline --locked -p terminal-chat --bin m7-provider-harness --target-dir <本批专用 target> -- --test-threads=1` **6 通过/0 失败/0 忽略**：1/8/1 与总 10 的实际中间件预算函数、错误地址/方法/关闭/过期门、最终请求 256 KiB 边界与 model/参数/来源门、返回 JSON 64 KiB/异常结束门、注入工具响应在真实 turn 分派前拒绝，以及真实三阶段库＋固定离线传输演练；监督脚本 **4/0/0**：实际 0.15s watchdog 强停、失败跳过后续项、成功各执行一次、单批标记排他创建。另将临时 phase2 cap **8→9**，保持断言不变，预算测试 exit **101** 如期失败；无条件恢复后六项全绿，再离线编译正式二进制并锁定源/二进制摘要。默认启动 Rust/Python 程序均 exit 2、无网络。但这些演练直接调用 `run_phase`，**漏了真实 CLI 合法目录的正向入口**，没有发现本次拒绝缺陷；不得以六项全绿冒充真实补验通过。详见[离线检查证据（原件快照）](<evidence-index.md#archive-4c52c66e3cd0c280>)。

**限制落实情况**：预算扣减位于 `LiveWire::handle` 调用唯一网络 continuation 前，先持久化全局/阶段计数再发送；地址/method 与最终含 model JSON 预检、64 KiB 返回接受门、每次新 Agent 且连接池容量 0、redirects=0、连接 10s/请求 180s 均在失败版本源码中，API 沿用 10 MiB 读取上限。`run_batch.py` 的真实 `Popen.communicate(timeout=240/1800/240)` 与进程树/独立 Git 进程组清理承载阶段 watchdog，单批总时限 2,280s；首项退出 2 即封批并跳过后续项。**本次只实际观察到入口拒绝及封批，未观察真实 HTTP 路径、供应商响应和真实请求超时**；相关 guard 的离线证据与真实未验证项分别保留。

**差异与收尾**：生产代码、测试、配置及仓库清单/lockfile均未改；harness 只以源码快照留在 `docs/records/013-m7-provider-check/`，不属于 Cargo target，默认 `cargo test` 不会触发真实请求。离线构建通过，有源码副本继承的 tchat-tui 497 条警告，无新增 harness 编译告警；`rustfmt` 组件未安装，尝试调用失败，未安装组件、未声称格式检查通过。按批准范围不重跑完整 workspace 门禁，§10 **2975/0/3** 仍为既有交付证据。所有附件均为本地失败/合成内容、源码和计数，不含真实凭据、认证头或敏感 URL；[证据清单（原件快照）](<evidence-index.md#archive-e4c5e48f35f1d3ef>)记录摘要。临时运行目录及源码副本/target 在证据保存后清理，仅保留私有批次已启动/已封闭标记及零计数账本，防止误续跑。

最终保持测试分支及原 HEAD，不暂存、不提交、不合并、不推送、不删除分支。保护项核对：CLAUDE/AGENTS/compose/忽略规则/两条安装记录/`.agents` 内容摘要与起点一致，`db/` 及已知文件仅元数据核对一致（未打开），宗旨文档保持已删除；既有两份计划原文保留（规格仅同步本次状态）。`git diff --check` 检查文档变化，结果与最终 Git 状态另由本轮收尾输出核实。其他供应商、空结果专项、Windows 等边界继续未验证；本批到此结束，停在审阅阶段。

**收尾实证**：`git diff --check` exit **0**，新增源码/证据逐项 whitespace check 通过；暂存区空、stash 0、仍仅原工作树。工作区为两份文档修改＋本批证据目录 10 个未跟踪文件，除此无变化。保护项、旧记录全文前缀、临时清理与封批标记核对均通过，详见[收尾核对（原件快照）](<evidence-index.md#archive-2447d6bf9069f219>)。

### 11.7 harness 路径离线修复与新批次申请（2026-10-19）

**最新状态：M7 代码接线已交付；harness 路径离线修复已验证，新的真实供应商补验批次待授权／待验证。** 本轮仅离线修复与检查，真实供应商 HTTP 尝试 **0**，未加载真实发送凭据、未开启真实批次。§11.6 原文与原始证据完整保留；原批次 **0/10、未用 10 次已封存**，不能转入新批次。phase1、phase2、注入用户轮的供应商行为仍全部待验证，M8/M9 未实施，V1 未完成。

**冻结与根因**：原批次 10 个附件逐字节冻结到 [failed-batch-2026-10-19/（原件快照）](<evidence-index.md#archive-a52ac92df34cd324>)。原 harness SHA-256＝`7eaffdc44caf2e2aa6e29d073be5a8bb6c7c31f6e5286c9b69c9950670015933`，原控制脚本＝`8a62de1514e74b44ba79fa105d9d73ffabe062bb700483018d87abe7d48a57ab`；原 phase1 退出 **2**、`batch scope rejected`、2 ms、0 次 HTTP 的进程证据保持不变。根目录原为 `/tmp/013-m7-provider-dxei6mir`，控制脚本从自己的父目录取得 `BASE`，传给 harness 的也是该根；二进制为 `BASE/target/debug/m7-provider-harness`，cwd 为 `BASE/run/phase1`，**不存在脚本与参数根目录不一致**。原进程附件未记录 argv；[冻结命令说明（原件快照）](<evidence-index.md#archive-048a929324068762>)明确区分依据原脚本重建的命令向量与实际保存的退出证据，未重启旧入口。

Rust 官方 [`Path::starts_with`](https://doc.rust-lang.org/std/path/struct.Path.html#method.starts_with) 按完整路径组件比较，`013-m7-provider-dxei6mir` 不匹配组件 `013-m7-provider-`。原校验想限制的是 `/tmp` 的直接子目录、该目录的命名前缀及它的 `run/<phase>` 子目录；不是 `/tmp/013-m7-provider-/…`。本地实际 `tempfile.mkdtemp(prefix="013-m7-provider-")` 生成 8 位小写字母／数字／下划线后缀；本次合法根符合该结构，拒绝来自组件语义误用。

**最小修复及离线安全边界**：只改本证据目录的 [harness.rs（原件快照）](<evidence-index.md#archive-695335d98badc476>)、[run_batch.py（原件快照）](<evidence-index.md#archive-e9425e8a6f71a2c4>)，新增 [offline_cli_checks.py（原件快照）](<evidence-index.md#archive-f44a0dc95c784c8c>)，不改生产代码、仓库测试、配置、Cargo 清单或 lockfile。

- 路径按组件收紧：父目录恰为 `/tmp`；仅对 `file_name` 校验固定前缀及 8 位后缀；拒绝 `..`、非绝对／非规范路径、root 与关键子目录／控制文件符号链接；根目录须为私有常规目录。依据官方 [`canonicalize`](https://doc.rust-lang.org/std/path/struct.Path.html#method.canonicalize) 与 [`symlink_metadata`](https://doc.rust-lang.org/std/path/struct.Path.html#method.symlink_metadata) 语义分别检查规范路径与链接本体，没有把整串路径改成字符串前缀匹配。
- 绑定脚本实际布局：当前可执行文件恰位于该根的 `target/debug/`，cwd 恰为 `run/<phase>`；阶段目录为空、启动标记／账本为常规文件、无关闭标记、当前阶段未尝试、阶段及累计账本有效。其他同名规则的临时根也不能借用本进程进入。错误输入在 origin 授权、数据库创建、凭据解析和发送之前退出 2。
- 控制脚本仅提取共用 `phase_command`，实际发送 argv 保持原样。新增 `--dry-run` 复用真实参数解析与同一 `validate_scope`，通过后打印 `send-preparation-allowed` 即返回，不进入 `run_phase`。测试构建另外用 **`m7-offline-only` 编译掉真实凭据加载与 LiveWire 网络 continuation**；驱动先确认该构建标记才执行 CLI。即使误走 live 分支也只会拒绝，不能外发。仓库默认 Cargo target 不包含 harness，默认 `cargo test` 不执行真实请求。

**RED → GREEN 与边界证据**：先复跑冻结版本原有 6 项离线测试，**6/0/0、exit 0**，确认原测试缺口。随后仅加入离线检查点／拒绝发送构建，保留错误的路径条件；通过真实监督脚本的参数构造、cwd、环境与 `supervise` 调用实际 CLI，`legal_cli_phase1` 仍 **CLI exit 2／测试 exit 1**，stderr 与原失败一致、前后文件快照不变。RED 源码另存 [red-harness.rs（原件快照）](<evidence-index.md#archive-e4739075b6f520e3>)，指纹及命令见 red-result.json。修复路径条件后不改通过断言，三项合法 CLI 均 **exit 0** 到达离线检查点，明确 `credential_loaded=false`、`origin_granted=false`、`database_created=false`、`HTTP_attempts=0`。

统一离线复核命令（`/workspace`，非交互、固定锁版本、独立临时源码／target，运行后清理）：

```sh
python3 docs/records/013-m7-provider-check/offline_cli_checks.py --verify-all
```

本次实际运行另以 `--result /tmp/013-m7-harness-fix-799fbb9/evidence/verify-all.json` 保存结果，命令 **exit 0，42 通过／0 失败／0 跳过**：7 项 Rust、31 项实际 CLI、4 项监督脚本；编译及不带离线 feature 的 `cargo check` 均 **exit 0**。各条实际命令与退出码见 verification.json。**正常发送构建仅类型检查，未执行。** 未机械重跑 workspace 门禁；§10 的 2975/0/3 仍是既有交付证据，不冒充本轮结果；未安装／运行 rustfmt、clippy，不声称其通过。

31 项 CLI 覆盖：三项合法入口；相似前缀、空／错误后缀、任意 `/tmp` 根；父目录跳转、相对／嵌套根、另一新临时根与不存在根；根、run、阶段、target、debug、标记、账本链接；缺标记、关闭状态、错误活动阶段、已尝试阶段、已有内容／user 链接、错误 cwd、非私有根、错误阶段／令牌／多余参数。拒绝用例均检查 **exit 2＋原样 stderr＋完整夹具文件快照无变化**。另在独立临时源码中分别故意移除后缀检查及可执行文件根绑定，保持断言不变，两项均 **测试 exit 1** 检出非法 CLI 错误通过；正式候选指纹始终未变，见 boundary-mutations.json。

**原发送护栏复核**：与失败版本的[源码差异（原件快照）](<evidence-index.md#archive-3f1049d27ce43c67>)确认实际发送分支的接口／model／参数／来源检查、1＋8＋1／总 10 次发送前持久化扣减、失败停批、零重试／池容量 0／不跟随重定向、大小与时限均未放宽。7 项 Rust 中保留原六项断言，并增加离线构建拒绝凭据／网络 continuation 的检查；三阶段合成演练新增核对临时授权文件恰一条指定 origin＋合成凭据指纹。4 项监督脚本继续验证实际 watchdog 强停、失败停止后项、顺序执行一次与排他启动标记。离线模拟的计数不是供应商请求；**本轮真实 HTTP 尝试与后台额外触发均为 0**，无供应商 usage／费用观测。

修复候选 SHA-256：

| 文件 | SHA-256 |
|---|---|
| `harness.rs` | `a971b2faf1e9c838623e3a84513264da1d426c7e463af914f5b735a389d594d3` |
| `run_batch.py` | `f27ebcb6c09817afbe4110bae1a2a62e311309f2cd53586128aeea49747f44a3` |
| `offline_cli_checks.py` | `e5331123be08f319b75a734b786ce4a8fe4ce9a837d6789e65bbb454d789e14b` |

**新批次申请（尚未授权，原额度不转入）**：沿用 §11.2–11.5 的目标与限制，无供应商、模型、协议、参数、预算或通过标准变更。仅申请一批 **`deepseek / deepseek-flash`、`openai-chat`、`POST https://api.deepseek.com/chat/completions`、`request_params={}`**；不新增 `response_format`、输出 token 参数或修改模板。通过当前路径预检只说明本地准备可行，不表示收到真实调用授权。

| 顺序／库入口 | 独立合成输入及通过标准 | 最大 HTTP 发送尝试 |
|---|---|---:|
| ① `phase1::run_stage1_job` → `api::chat_with_body` | ≤8 KiB 固定合成会话；真实 content 走既有严格解析、三态分派及存储，预期 `SucceededWithOutput`、产物入库、job done／水位正确。合法空结果如实记录且停止，不改称有内容成功 | **1** |
| ② `phase2::run_phase2_consolidation` → `run_tool_turn_with_mode(MemoryMerge)` | 独立种入 ≤8 KiB 合成来源；实际成功调用 `mem_list`／`mem_read`／`mem_write`，有效 `MEMORY.md` 与首行恰 `v1` 的摘要；既有产物校验、非空正常终文、SQL 成功提交及收尾通过。不伪造调用或补产物 | **8**，含首次及最多 7 次工具续轮 |
| ③ `run_tool_turn_with_mode(UserWithMemory)` | 独立 ≤1 KiB 合成摘要；唯一 system 尾段、恰一记忆块，供应商接受并正式回复 `COBALT-17`、正常结束且自动块不入史；任何工具响应在执行前拒绝。不推断 system/developer 语义等价 | **1** |

- **总计至多 10 次发送尝试**，内部 HTTP 重试 0，阶段额度不挪用。任何本地／HTTP／工具／解析／产物／SQL／超时／超限失败立即关闭整批，后项不执行；不修正续跑、不另开批、不自动换模型或供应商，全部通过后也不使用剩余额度追加试验。
- 最终请求 **≤256 KiB**；API 响应读取 **10 MiB**，宿主接受 JSON **≤64 KiB**；连接 **10s**、单请求 **180s**，阶段 **240／1800／240s**、整批 **2280s**。保持默认输出协议；服务端输出 token 上限、usage 与价格／费用目前未知，**没有明确计费 token 或金额预算上限**，上述本地输出限制不承诺费用上限。
- 新授权后才新建 `/tmp/013-m7-provider-<8 位后缀>` 私有根及各阶段临时 user、sessions、数据库、记忆根与 cwd，全部只含合成内容。只在该范围对指定 origin＋现有选中凭据授权，不改全局 origin 策略；不访问真实历史／记忆、不启动交互应用／后台管线，额外后台触发须为 0。三项独立验证，不宣称同一数据链端到端通过。
- 启动前须重新核对实际 HEAD、配置／参数、凭据存在及离线候选／二进制指纹；有漂移在发送前停下。**当前尚缺新的单批真实调用授权**；服务端可用性／账户／三项行为仍待真实验证，本轮未重新解析凭据，不要求在聊天提供密钥。旧 `/tmp/013-m7-provider-dxei6mir` 只保留原封批标记与零账本，不复用。
- 若获新授权，结果仍追加本记录：各阶段实际次数／累计、请求大小／耗时／HTTP 状态／结束原因、工具名称、解析／存储／产物／SQL、供应商 usage（缺失写未知）、停止点／未执行／未验证项及后台计数；证据不保存凭据、认证头。其他供应商、空结果专项、Windows 边界继续未验证。

**Git、保护与候选**：保持 `test/013-memory-m7-provider-check @ 799fbb972c7c4b500cd01d3f20ee99055a01ac6b`；两份既有计划追加保留，§11.6 全文未覆盖。原失败快照内旧清单只描述冻结版本，修复候选另见 [新证据清单（原件快照）](<evidence-index.md#archive-cade8551ca20442a>)，不把旧源摘要冒充当前源摘要。实际候选清单、保护文件字节／数据库元数据／旧封批标记比对与最终 `git diff --check` 结果见 [本轮收尾核对（原件快照）](<evidence-index.md#archive-41662447871798ac>)。不暂存、不提交、不合并、不推送、不删除分支；到此等待新的真实调用授权。

### 11.8 新授权批次真实执行结果（2026-10-19）：phase1 合法空结果，停止整批

**最新状态：M7 代码接线已交付；本批真实供应商补验部分观察、未通过预定验收，已封闭，待审阅。** 用户已明确授权本次新批次，不再请求该授权。§11.7 的「待授权」保留为申请时历史；本次只执行该一个新批次，未修正续跑、未重开、未追加试验。M8/M9 未实施，V1 不标完成。

**批次身份与发送前复核**：`m7-deepseek-new-367qctgq`，全新私有根 `/tmp/013-m7-provider-367qctgq`，与旧 `/tmp/013-m7-provider-dxei6mir` 无关；身份、源码／二进制指纹、实际 argv／cwd 与独立 `ledger.json` 关联见 BATCH_IDENTITY.json。分支／HEAD 符合预期，暂存区空、stash 0、唯一工作树；修复 manifest 的三份源码、原失败快照、离线修复附件及两份既有文档摘要全部一致。未修改已批准 harness／启动脚本或断言，直接复制它们到新根，`git archive HEAD` 仅取锁版本源码／清单，独立 target 离线编译 **exit 0**（40,006 ms）。正常构建 `--offline-build-info` 及实际路径／参数绑定核对通过；沿用精确源码已有 **42/0/0** 离线证据，未机械重跑。监督脚本仍是 `run_batch.py --execute-authorized-batch M7-ONE-BATCH-2026-10-19`，**实际仅调用一次、最终 exit 1**，见 编译与准备证据、执行记录。

**实际目标与隔离**：请求及供应商响应的 model 均为 **`deepseek-flash`**，provider **`deepseek`**、协议 **`openai-chat`**，唯一模型端点 **`https://api.deepseek.com/chat/completions`**，`request_params={}`。最终请求仅 `messages`／`model` 两字段，未加 `response_format` 或输出 token 参数。只有 phase1 实际创建本批临时 user／sessions／数据库／记忆根／cwd，来源为 **1,062 字节 V1 合成会话**及既有内置模板；临时授权文件恰 **1** 条指定 origin，绑定本批所用凭据。phase2 与注入阶段只保留预建空目录，未初始化业务状态。未访问真实会话／记忆或打开真实数据库，不改真实配置或全局 origin 策略，不启动交互应用及后台管线；本批额外后台触发 **0**。

| 阶段 | 实际 HTTP 尝试／累计 | 实际结果与验收判定 |
|---|---:|---|
| phase1 | **1／1** | HTTP **200**、`finish_reason=stop`；请求 **32,718 字节**、返回 JSON 重序列化 **2,013 字节**，API 调用 **2,810 ms**，阶段进程 **2,889 ms**、未超时、退出 **1**。content 经既有严格三键解析及三态／存储路径得到 **`SucceededNoOutput`**；合法空态已观察，但未满足预定 **`SucceededWithOutput` 有内容入库**，故本项验收未通过并停止整批 |
| phase2 | **0／1** | **未执行**：三项 `mem_list`／`mem_read`／`mem_write` 的真实调用／成功、有效两产物、摘要首行 `v1`、正常终文、SQL 成功提交和 Git 收尾均未验证；不能据离线演练或手工产物宣称通过 |
| 注入用户轮 | **0／1** | **未执行**：供应商对唯一 system 尾段的接受度、`COBALT-17` 实际回复及正常结束仍待验证；无 system/developer 语义等价结论 |

**空态与存储实证**：供应商 content 原样为 `{"rollout_summary":"","rollout_slug":"","raw_memory":""}`，字段齐全、值全空；不是 JSON 解析失败、HTTP 失败或三态 `Failed`。阶段记录为 `passed=false/outcome=SucceededNoOutput/stage1_output=null`；job **done**、`last_error=null`、`retry_at=null`，输入及最后成功水位均 **1,699,974,800**。仅对本批临时 SQLite 以只读方式交叉核查：threads **1**、jobs **1**、stage1_outputs **0**，未生成 MEMORY／摘要文件，见 阶段结果、临时库与隔离核查。这证明此样本的合法空态处理，不证明有内容路径或完整三态全部分支。未因结果为空改模板、改合同或重发。

**计数、usage 与封闭状态**：真实 HTTP 发送尝试 **1/10**、收到响应 **1**、内部 HTTP 重试 **0**；phase1／phase2／注入分别 **1／0／0**，无 `/models`、探测或其他调用。供应商原始 usage：prompt **6,634**、completion **287**、total **6,921 tokens**；其中 reasoning **267**、缓存命中 **0**（按返回字段记录），完整 usage 见 事件与原始脱敏响应。费用未知，不由 tokens 推算金额。

未用 **9** 次已随本批封存，`closed=true`、`authorization_consumed=true`、`further_batch_permitted=false`、`all_passed=false`，不转移或追加使用，见新批次账本及批次汇总。阶段结果内 `attempts_at_end.closed=false` 是主程序根据 `passed=false` 执行关闭之前的快照；最终账本和 `BATCH_CLOSED` 才是封闭状态依据。旧失败批次仍为 **0/10、未用 10 次独立封存**，旧标记／账本／附件未改。

**限制实际生效情况与边界**：发送沿用同一 `LiveWire` 扣减后 continuation、固定 endpoint／model／参数／合成来源检查、每次新 Agent／池容量 0／redirects=0。请求 **256 KiB**、API 读取 **10 MiB**、宿主接受 JSON **64 KiB**；连接 **10s**、请求 **180s**；监督脚本阶段 **240／1800／240s** 与不重置的整批 **2280s** 机制均未改且本次已使用。实际仅首项请求返回，未触及大小／超时／预算极限，不把此前离线边界检查宣称为真实极限观测。服务端默认生成上限和金额仍未知，没有计费 token／金额硬预算。

**证据、保护与收尾**：保存本批合成请求／脱敏响应、事件、三态结果、计数、执行／编译与临时库核查；不保存真实凭据、认证头或临时授权文件。凭据整值及其 JSON 转义形式在留存证据中检查为不存在，结果汇总前只读复核原监督脚本的全部保护基线通过。现有实施记录到 §11.7 全文作为前缀保留，原失败／离线修复附件均不覆盖；规格只同步本次状态。新附件清单与摘要见 [manifest.json（原件快照）](<evidence-index.md#archive-faf4d9adc193cba7>)，实际候选、Git 与保护项前后核对／清理结果见 [preservation.json（原件快照）](<evidence-index.md#archive-b8f1312336bb57d6>)。临时凭据授权、会话、数据库、记忆根、源码与 target 在证据保存后清理；只保留新批身份、启动／封闭标记和最终计数账本，旧封闭目录不触碰。

其他供应商、phase1 有内容／失败分支、phase2 与注入真实行为、Windows 继续未验证；未追加独立空结果专项，不能由此次偶然空态推广全部空结果边界。三项按独立合成夹具设计，本批仅执行第一项，不宣称同一数据链端到端通过。生产源码、测试、配置、仓库清单／lockfile 未改；不重跑 workspace 全量门禁、不开始 M8/M9。保持原分支与 HEAD，不暂存、不提交、不合并、不推送、不删除分支，到此等待结果审阅。

### 11.9 合法空结果离线审阅轮（2026-10-19）：输入与判定链重放通过，根因＝夹具自述虚构触发证据门槛，方案待审

**范围与计数**：应审阅指令只做离线审阅——读取已保存的脱敏请求／响应／事件／结果与临时库核查证据，未发送任何真实请求（**HTTP 0**）、未触发后台管线（**0**）、未访问真实会话／数据库／记忆目录、未改动任何历史批次目标或结论（§11.8 原判保留：`all_passed=false` 不改判）。旧批 0/10 与本批 1/10 各自独立封存状态经 [preservation.json（原件快照）](<evidence-index.md#archive-b8f1312336bb57d6>) 复核未动；剩余额度不解封、不转移。

**事实登记（沿用原证据，不改判）**：已观察——HTTP 200、`finish_reason=stop`、严格三键解析成功、`SucceededNoOutput`、job done（`retry_remaining=3`、`last_error=null`）、输入与成功水位均 1,699,974,800；未达到——有内容成功入库（`stage1_outputs=0`）；未执行——phase2 与注入用户轮（独立夹具、0 请求）。单次合法空结果不推出供应商不兼容或生产缺陷。

**输入组装链离线重放（14/14 通过）**：新增 [offline_replay_review.py（原件快照）](<evidence-index.md#archive-2b1698c3cc7e45e8>)（只读证据与生产模板，无网络），结果 offline-replay-result.json。要点：①夹具四条（`harness.rs fixture()`）→ 会话文件字节复算 **1,062**＝`result.json source_bytes`，≤8 KiB 限额；②请求 system 与生产 `src/app/src/memory/templates/stage_one_system.md` **逐字节一致**；③请求 user 与 `stage_one_input.md` 占位符渲染＋夹具投影重放**逐字节一致**——角色序（user/assistant/user/assistant）、四条内容逐项相等，`sanitize_history_item_for_memories` 零剔除（夹具不匹配 AGENTS.md/skill 注入标记）、`redact_secrets` 零替换（无秘密模式）、无截断标记（远低于 150,000 token 回退预算）；④请求形状恰 `messages`＋`model` 两键、角色序 system→user。**结论：模型看到了完整夹具，无内容在装载、过滤、序列化、渲染或预算环节丢失。**

**空结果判定链离线重放**：实际响应 content `{"rollout_summary":"","rollout_slug":"","raw_memory":""}`——三合同字段全部为字符串、键集合恰为三键（`deny_unknown_fields` 通过）、**去空白长度均为 0**；生产分派条件（`phase1.rs:160-165`：`raw_memory` 为 `Some("")` 或 `rollout_summary` 为空）重放结果 `SucceededNoOutput`，与记录 outcome 一致。字段提取无错：`api::reply_text` 只取 `choices/0/message/content`，本响应 content 非空提取成功；deepseek 将 267 token 推理置于 `reasoning_content`、最终 JSON 置于 `content`，与生产提取路径精确匹配（若推理混入 content 导致 JSON 缺失，严格解析会失败——本次未发生）。存储副作用符合空结果契约：`mark_stage1_job_succeeded_no_output` 仅 UPDATE jobs（done／水位推进／清错），无 stage1_outputs INSERT——与临时库只读核查（threads 1、jobs 1、stage1_outputs 0）一致。

**根因定性（证据三层独立一致，非生产缺陷）**：①输入侧——上节重放证明组装无损；②判定侧——空分派与存储完全按合同执行；③模型侧——脱敏响应 `reasoning_content` 明确陈述：回放被"explicitly marked as fictional synthetic replay, not real project or real execution evidence"，按系统提示「Evidence-based only / No-op is allowed and preferred」规则保存自述虚构的偏好会违反证据门槛，故正确选择全空返回。即：**夹具为诚实标注"不能当作真实证据"，恰好触发生产提示的证据门槛，二者语义相互作用导致合理 no-op**。夹具内容本身按提炼提示标准属高信号形态（稳定操作偏好＋重复确认＋可复用故障处理），harness 离线 stub 亦预期非空输出——不能凭模型返回空认定夹具无价值。协议兼容性证据反而增强：openai-chat 全链路（请求形状、响应提取、严格解析、三态、存储）真实样本走通合法空路径。

**下一步最小补验方案（待批准，本轮未执行任何一项）**：

- **覆盖区分**：已有——协议与合法空路径的供应商兼容性（本批 1 样本）；仍缺——phase1 非空提炼入库、phase2 工具链、注入用户轮（后两项本就是独立夹具、独立目录，phase2 用独立合成 seed 不依赖 phase1 输出，注入用独立摘要文件）。
- **方案变化 A（建议，须批准）**：三项分开申请、分开执行、各自独立上限与停止条件；合法空结果不再阻断其他独立测试。变化 B（备选，须批准）：保留单批三阶段，但把「phase1 空结果立即封批」降级为「记录后继续」，等于放宽原停止条件——不默认采用。
- **phase1 非空补验**：输入＝夹具 v2——把首句"以下全部是虚构的合成回放，不能当作真实项目或真实执行证据"改为中性测试说明（如实标注合成来源但不否定证据资格，如"本回放为记忆提炼链路验证用的合成测试数据，请按正常提炼标准处理其中偏好与经验"），其余三条不变；原夹具与原响应保留不改。通过标准＝`SucceededWithOutput` 且 `stage1_outputs` 新增 1 行且三字段非空。请求上限 1 次发送尝试；任何失败立即封批。先补离线输入完整性检查（v2 渲染无内容丢失，同本轮脚本方法），新夹具标为待实测。
- **phase2**：输入＝既有独立合成 seed（不依赖 phase1）；通过标准＝`mem_list`/`mem_read`/`mem_write` 真实调用成功、两产物有效、摘要首行 `v1`、正常终文、SQL 提交；上限 8。
- **注入**：输入＝独立 `memory_summary.md`；通过标准＝供应商接受 system 尾段、回复含 `COBALT-17`、正常结束；上限 1。
- 不通过事后放宽通过标准解封既有额度；所有新批次须新授权后执行。

**本轮收尾**：新增离线检查实际运行（14/14 通过，exit 0）；按审阅指令免重跑 workspace 全量门禁（生产源码零改动，`git status` 仅两文档追加＋证据目录）。运行 `git diff --check` 通过（无空白错误）。不暂存、不提交、不合并、不推送、不删除分支、不开始 M8/M9；两份文档既有追加与 44 个既有证据文件未改动，本轮新增文件限于 `docs/records/013-m7-provider-check/offline-review-367qctgq/`（脚本＋结果两件）。到此停止，等待方案审阅与新的真实调用授权。

### 11.10 v2 夹具定稿与三批独立补验授权包（2026-10-19，离线准备轮，真实请求 0）

**范围与计数**：按审阅后指令只做离线准备——定稿 phase1 v2 夹具、完成最小离线检查、提交可一次批准的三批授权包。本轮真实 HTTP **0**、后台管线触发 **0**、未访问真实会话／数据库／记忆目录；原 0/10（`dxei6mir`）与 1/10（`367qctgq`）两批继续独立封存、不改判、不解封；§11.8／§11.9 结论与全部既有证据（含上轮审阅 14/14 重放结果）原样保留。

**v2 夹具定稿**：修改 `harness.rs fixture()` 第一条 user 消息，仅将自我否定证据资格的首句「以下全部是虚构的合成回放，不能当作真实项目或真实执行证据。」替换为指定文本「**以下为本次测试构造的合成会话。**」，其后 114 字符及第 2–4 条（含原有「合成」「虚构故障」标注）逐字不变；角色与顺序不变；未加入任何「必须非空」要求，未改生产提示／解析合同／空结果语义。v1 完整冻结于 [harness-v1-frozen.rs（原件快照）](<evidence-index.md#archive-64e3933085480a38>)（sha256 `a971b2fa…`＝367qctgq 批 manifest 记录的执行版指纹），精确差异见 harness-v1-to-v2.diff（仅一行）。v2 harness sha256 `14862629…`（完整指纹见 [manifest.json（原件快照）](<evidence-index.md#archive-720b5c4b318a4e29>)）。**v2 状态＝待实测：如实标注合成来源，但不保证模型一定选择非空**；本轮起 `approved_harness_unchanged` 保护断言不再适用于 `harness.rs`（该修改属本轮授权范围，此登记即为变更记录），`run_batch.py`／`offline_cli_checks.py` 未改。

**离线检查（全部实际运行，共 7＋3＋11＝21 项通过、0 失败）**：

- **离线排练 7/7**（offline-only 构建：`git archive HEAD` 源码＋`[[bin]]` 绑定 v2 harness＋`--features m7-offline-only`，独立 `--target-dir`，不污染共享产物）：含 `real_library_three_stage_rehearsal`——**v2 夹具沿真实库路径**（`write_session`→`read_recent_rollout_items`→`build_stage_one_request`→`validate_request` 与独立组装的 expected **逐字节相等**→离线 stub 有内容响应→`SucceededWithOutput` 入库→phase2→注入）全链通过；及 `offline_build_cannot_load_real_credentials_or_continue_live_transport`（offline-only 构建拒绝加载真实凭据、`LiveWire` 拒绝真实传输、账本零预留、无事件文件——**离线模式不能发送真实请求**的结构验证，未加载任何可实际发送的连接）。
- **CLI 合法入口 3/3**（`legal_cli_phase1/phase2/injection`，offline-only 二进制）：`--dry-run` 到达 `send-preparation-allowed` 检查点（`offline_only_build=true`、`credential_loaded=false`、`HTTP_attempts=0`、快照证明零副作用）。
- **v2 专项 11/11**（[verify_v2_fixture.py（原件快照）](<evidence-index.md#archive-72177654b2aa9af3>)→[v2-verification.json（原件快照）](<evidence-index.md#archive-e9459cb1948398db>)）：差异恰为首句替换（第 2–4 条逐字相同、角色序不变）；v2 会话源复算 **1,020 字节** ≤8 KiB；生产模板渲染的最终请求 **32,658 字节** ≤256 KiB；五项值得提炼的关键内容（中文交流／先失败后通过／SYNTH-E17／verify-cobalt 清单／不盲目重试）及 assistant 侧排查结论全部完整进入请求正文；无截断（远低于 150,000 token 回退预算）。

按指令只运行新增及受影响检查（未重复 32 个 scope 拒绝案例、监督自检与 workspace 全量门禁——`run_batch.py`／生产源码未改）。临时根 `/tmp/013-m7-v2prep-a1b2c3d4` 用后即弃。

**覆盖登记**：已有——openai-chat 协议与合法空路径的真实供应商证据（367qctgq，1 样本）＋两轮离线管线重放（367qctgq 实请求重放 14/14、v2 正向验证 11/11）；仍缺——**非空提炼入库、phase2 工具链、注入用户轮的真实行为**，正是下列三批目标。

---

**三批独立补验授权包（方案 A；待用户批准，本轮未执行任何批次）**

**共同配置（三批一致，沿用 367qctgq 批）**：provider `deepseek`／model `deepseek-flash`／协议 `openai-chat`／唯一端点 `https://api.deepseek.com/chat/completions`／`request_params={}`。大小限制：最终请求 ≤256 KiB、API 响应读取 ≤10 MiB、宿主接受 JSON ≤64 KiB、合成会话源 ≤8 KiB。时限：连接 10s、单请求 180s、阶段 240／1800／240s、单批总 2,280s。内部 HTTP 重试 0（每请求新 Agent＋池容量 0）；服务端输出上限与价格未知，**费用无硬预算，不由 tokens 推算金额**。每次发送尝试先经独立账本持久预留；发送前离线预检（scope／指纹／dry-run 检查点）通过方可执行；证据留存与脱敏、临时根清理、真实数据零访问等全部保护沿用既有基线。

| 项 | 批 1：phase1-v2 非空路径 | 批 2：phase2 合并工具链 | 批 3：注入用户轮 |
|---|---|---|---|
| 建议批次令牌 | `M7-P1V2-2026-10-19` | `M7-P2-2026-10-19` | `M7-INJ-2026-10-19` |
| 输入（独立夹具） | v2 夹具会话（1,020 字节，本节定稿） | 独立合成 seed（raw＋summary ≤8 KiB，**与 phase1 输出无关**） | 独立 `memory_summary.md`（`v1`＋`COBALT-17`，**与 phase1 输出无关**） |
| 发送尝试上限 | **1** | **8**（含工具续轮） | **1** |
| 通过标准 | `SucceededWithOutput` 且 `stage1_outputs` 恰 1 行且 `raw_memory`/`rollout_summary` 非空且 job done＋水位提交（harness 既有断言，不放宽） | 三工具（`mem_list`/`mem_read`/`mem_write`）真实调用成功、两产物有效、摘要首行 `v1`、SQL 提交、git 基线干净、job done（harness 既有断言全集） | system 唯一含 memory-context、回复含 `COBALT-17`、历史 2 条／会话 3 行、memory-context 不入史、正常结束 |
| 停止条件 | **再次合法返回空＝该批未达非空目标并封闭**（不改判、不重发、不据此自动改夹具）；任何失败立即封闭 | 任何工具错误／解析／产物／SQL 失败立即封闭 | 任何失败立即封闭 |

**跨批规则**：①每批独立身份（各自新 `/tmp/013-m7-provider-<8 位>` 私有根）、独立账本、独立预算与停止条件，**额度不互借**；②**全部发送尝试合计不超过 10 次**（1＋8＋1）；③phase2／注入不依赖 phase1 非空（独立夹具独立根），phase1 合法空只封闭批 1；④**一次授权覆盖三批时，单批未达标默认允许继续其余已授权独立批次**（方案 A 设计意图；批准时可改为「任一批未达标即停止全部」，以批准意见为准）——此为待批准的方案变化；⑤**共同前提不成立时停止所有尚未执行的请求**：HEAD／分支或 harness／脚本指纹与本授权包不符、配置／凭据／端点漂移、离线预检失败、保护基线破坏等；⑥不通过事后放宽通过标准解封任何已封批次（含 0/10、1/10 两旧批）。

**本轮收尾**：生产源码、`run_batch.py`、`offline_cli_checks.py`、用户配置零改动；新增文件限于 `docs/records/013-m7-provider-check/v2-fixture/`（v1 冻结副本、diff、验证脚本与结果、CLI 三案例、manifest 共 9 件）＋两文档追加。`git diff --check` 通过。不暂存、不提交、不合并、不推送、不删除分支、不开始 M8/M9。**到此停止，等待本授权包的批准与新的真实调用授权。**

### 11.11 方案 A 执行轮：共用预检本地错误，三批未执行并封闭（2026-10-19）

**结果：真实发送 0/10，三项均未执行；共用预检失败后全部停止，未修正后续跑。** 用户已明确批准 §11.10 的三个独立批次及方案 A，不再申请同一授权；上节「待批准」是准备轮历史。批次自身未达标允许继续其他独立批次，但本次失败发生在共用发送前复核，适用「停止所有尚未发送请求」。本轮不新增任何供应商行为证据，不改变非空提炼覆盖状态；M8/M9 未实施、V1 未完成。

**发送前已核实**：分支 `test/013-memory-m7-provider-check`、HEAD `799fbb972c7c4b500cd01d3f20ee99055a01ac6b` 符合预期；暂存区空、stash 0、单工作树。当前 v2 manifest 的八个文件及上节明确未改的两份脚本指纹 **10/10 匹配**；以 v2 harness 完整 SHA-256 `148626298729769ad4e0461503a03ec951d6dff66916533d5ac3af803d21f82b` 为依据，未拿 offline-fix 中的 v1 指纹作当前期望。配置仍为 `deepseek / deepseek-flash`、`openai-chat`、唯一端点 `https://api.deepseek.com/chat/completions`、`request_params={}`；当前凭据引用 `env:DEEPSEEK_API_KEY`，选中值来自项目 `.env`，仅在本地内存确认存在，未输出或留存明文。无 `/models`、连通性探测、真实 CLI/TUI 或后台入口。

**本地失败及责任范围**：为将已审宿主按三个私有根分别分发而新增的控制层 `/tmp/013-m7-plan-a-20261019.py`，在唯一一次 `prepare-offline` 调用中执行到 `baseline()`→`save(preflight.json)` 时退出 **1**，错误为 `TypeError: Path.open() got an unexpected keyword argument 'opener'`。这是本 Agent 新增控制层把内置 `open` 的参数误用于 `Path.open` 的实现错误；语法编译通过不能证明该调用可运行。已审 v2 harness、两份原脚本与生产代码均未改；没有把错误归为供应商拒绝。准确进程耗时未捕获，记未知，不从工具轮询时间推算。完整退出证据与原样失败源码见 共用预检失败、[失败控制层（原件快照）](<evidence-index.md#archive-76dcb5cf3d06c9df>)。

失败时临时准备目录只有两份既有文档的备份；尚未落盘 `preflight.json`，尚未归档源码、构建禁发或 live 二进制、调用任何 harness、创建三个运行根、授予临时 origin、建立阶段数据库或生成最终模型请求。计划的 **59 项禁发检查全部未运行**（Rust 7、CLI 31、监督 4、v2 专项 11、新根检查 6），不得借用 §11.10 的既有 21 项或更早 42 项结果冒充本轮通过。按用户边界未机械重跑 workspace 全量门禁、typecheck 或 lint；本轮不报告全量通过。

| 独立批次 | 实际发送 / 三批累计 | 结果与封闭状态 | 未用额度 |
|---|---:|---|---:|
| `M7-P1V2-2026-10-19` | **0 / 0** | **未执行、已封闭**；未观察解析、`SucceededWithOutput`、输出行或 job／水位提交 | **1，封存** |
| `M7-P2-2026-10-19` | **0 / 0** | **未执行、已封闭**；未观察三工具、产物、`v1`、正常终文、SQL 或 Git 基线收尾 | **8，封存** |
| `M7-INJ-2026-10-19` | **0 / 0** | **未执行、已封闭**；未观察供应商接受、`COBALT-17` 回复或不入史行为 | **1，封存** |

三项各自的 身份、账本与结果分别留存。失败后只在原计划的三个全新私有根 `/tmp/013-m7-provider-m7a1v201`、`m7a2v201`、`m7a3v201` 建立 **0700 已封闭标记目录**，各仅含身份、零账本与 `BATCH_CLOSED`；不是运行根，没有 `BATCH_STARTED`、`LIVE_STARTED`、阶段数据或可执行文件。额度不互借，不复用旧目录，不自动重新开启。

**计量与验证边界**：本轮合计 **0 次发送、0 个响应**；HTTP 状态及 `finish_reason` 不适用，各批耗时无观测；usage **未知（无供应商响应）**、费用未知，不写成服务端 usage=0。内部重试 0、后台额外触发 0；整体真实执行计时未启动。三项独立夹具均未实例化或发送，不能构成同一数据链端到端通过，更不证明 system/developer 语义等价。旧 `dxei6mir` 的 0/10 与 `367qctgq` 的 1/10 原判定及未用额度继续封存，历史请求不并入本轮计数。

**证据与 Git 边界**：本轮候选仅两份文档的必要状态同步及 `plan-a-2026-10-19-stopped/` 新附件。实施记录至 §11.10 全文保留为前缀；v2、v1 冻结副本与全部既有附件原样保护；配置、锁文件、CLAUDE／AGENTS／compose／忽略规则／两条安装记录／`.agents` 内容未改；`db/` 及已知数据库文件只核对元数据，没有打开真实数据库或访问真实会话／记忆目录。结果落盘前的[保护基线（原件快照）](<evidence-index.md#archive-05ddba711f6538f3>)及最终[收尾核对（原件快照）](<evidence-index.md#archive-de8e179eeedf26e0>)区分了失败发生前的观察与结果写入前后比对；附件摘要见 [manifest（原件快照）](<evidence-index.md#archive-98b4a355319598e5>)。不暂存、不提交、不合并、不推送、不删除分支。本轮到此停止，等待结果审阅。

### 11.12 分批控制层离线修复与完整入口排练（2026-10-19，真实请求 0）

**结果**：控制层已离线修复，实际 CLI 的完整预检、构建、三阶段启动、独立计数与封批路径均已排练。最终回归 **74 个具名验证项通过／0 失败／0 跳过**，原计划 59 项此次实际执行，不能回填成 §11.11 当时已运行。普通编译分支离线类型检查 exit 0；真实请求与后台额外触发均 **0**。只处理控制脚本、禁发适配、检查及文档，未重跑 workspace 全量门禁，未声称 clippy／rustfmt 通过。M8/M9 未实施、V1 未完成；非空提炼、phase2 工具链、注入用户轮的真实供应商证据仍待新授权。

**RED→GREEN 与范围**：原 [失败源码（原件快照）](<evidence-index.md#archive-76dcb5cf3d06c9df>)不改。用原 `python3 /tmp/013-m7-plan-a-20261019.py prepare-offline` 实际入口，在全新临时目录、固定合成凭据覆盖环境及隔离 Git 配置下重现同一 `Path.open(opener=...)` 异常，exit 1；RED 证据保留完整 stderr 与耗时。首次复现监督断言失败且未留存子进程输出，未算作成功复现；随后实证才计入。最小修复＝改用内置 `open(temp, ..., opener=...)`，保留 0600 opener、flush／fsync、临时文件＋原子替换及路径校验，见 单独修复差异。

完整入口首轮还发现原 `shutil.copyfile` 不保留执行位，阶段二进制为不可执行文件，实际启动报 `PermissionError`；当时已运行原计划中的 53 项及新增 1 项，六个新根检查未完成，不冒称 59 全过。该轮源码、退出日志、检查与封闭模拟账本保留，复制后显式设 **0700** 的修复差异另存。之后在新根从同一 `prepare-offline`／分批执行入口成功通过。

候选 [plan_a.py（原件快照）](<evidence-index.md#archive-3418de07b7baf583>)复用原 `run_batch.py` 的 `phase_command`、`supervise`、`verify_preflight`，不删除路径／指纹门；增加新私有根参数、明确 offline 模式、一次启动／封批及完整初始预检失败收口。原 v2 harness 全文、夹具与两份支持脚本保留；候选 [harness.rs（原件快照）](<evidence-index.md#archive-f20e28736a0010bb>)只增加编译期 `m7-offline-only` 才可用的执行适配、合成失败／耗尽响应及独立全局上限测试。`fixture()` 字节、供应商／端点／大小限制、生产模板和既定通过标准不变；差异见 [控制层完整 diff（原件快照）](<evidence-index.md#archive-203296b693c3f7f5>)、禁发适配 diff。生产源码、配置与 Cargo 清单／锁文件未改。

**一条离线命令（仓库根 `/workspace`，默认执行全部；可选留存附件）**：

```sh
python3 docs/records/013-m7-provider-check/controller-fix/verify_controller.py --artifacts /tmp/013-m7-cfix-final-artifacts
```

命令实际 exit **0**，每个子命令 argv、exit、耗时及完整入口 stdout／stderr 见 verification.json和同目录日志。再次执行应省略 `--artifacts` 或选择不存在的新附件目录；验证器自行创建新临时根，结束清理模拟运行数据，所有测试只用标准库和锁定 Rust 依赖。构建均 `--offline --locked --features m7-offline-only`、显式独立 `--target-dir`；后续故障场景仅复用校验过的同份禁发二进制，不重复编译或冒称重跑单元测试。普通分支仅执行 `cargo check --offline --locked ... --bin m7-provider-harness --target-dir /tmp/013-m7-cfix-normal-check-target`，exit 0，没有运行 live 宿主。编译有既有 TUI 等告警，完整输出留存，不据此声称 lint 通过。

**计数**：原 59＝Rust 原 7＋实际 CLI 31＋监督 4＋v2 专项 11＋三个新根的六个 dry-run；新增独立全局预留上限测试 1，合计基础检查 **60**。再加完整控制场景 **6**、默认／缺新授权／优化解释器拒绝入口 **3**、质量证明 **5**，共 **74**。完整命令共执行 48 次新根 dry-run（八次准备各六项）及 16 个离线阶段进程（六场景 13＋两项变异 3）；均有检查点与事件证据。辅助脚本随后仅校正子进程 HOME／TMPDIR 归属以统一清理，控制层及 harness 未变，五项质量证明定点复验 5/0，未机械重复其余已通过检查。

| 完整入口场景 | phase1／phase2／注入模拟尝试 | 实际检查结果 |
|---|---|---|
| 正常 | 1／3／1 | 三项通过、独立账本封闭；真实库 SQL、产物／v1、Git 基线合同、注入历史检查通过 |
| phase1 合法空 | 1／3／1 | 首批 `SucceededNoOutput` 未达非空目标并封闭，另两批继续通过 |
| 共用预检指纹失败 | 0／0／0 | 三批均未启动、全部封闭 |
| 首批后指纹漂移 | 1／0／0 | 只完成首批，未启动后两批且全部封闭 |
| phase2 用尽 | 1／8／1 | phase2 第八次后失败封闭，无第九次；注入继续，总数恰 10 |
| 首批零尝试且 phase2 用尽 | 0／8／1 | 首批剩余 1 次封存；phase2 仍最多 8 次，不能借用，累计 9 |

每次实际中间件预留同时检查本批 cap 与累计 10，下一独立账本只携带已封闭前批的计数快照，不携带其数据或额度。正常及故障流程均从真实控制 CLI 启动编译期禁发二进制，三个合法阶段的 `send-preparation-allowed`／`offline-phase-start` 检查点、临时授权合成指纹、模拟请求／响应、工具、SQL、Git 结果及各自封批账本均在 verification/ 分目录留存；`ledger_kind=offline_simulation`，不与任何历史真实批次混用。原接口日志中的 `HTTP_attempts` 是模拟预留数，须结合 `offline_simulation`／`offline_stub` 及 `real_HTTP_attempts=0` 读取；`live-build-info` 等兼容字段在本轮记录 `offline_only_build=true`，不是运行了 live 构建。模拟 usage 也不当作供应商计费数据。

**错误检测证明**：除原接口异常 RED→GREEN、执行位失败→成功外，保持场景断言不变，在独立临时源码分别故意把“单批失败”改为“全停”、去掉累计 seed，两个错误均被实际控制入口的原断言检出；正确候选六场景全绿。另验证优化 Python 不可绕过 assert 门、0600 打开在 umask=0 时仍成立且原子替换、不允许复开已封闭模拟批、v2 夹具及参数不变。变异源码、差异、命令、退出及检测结果见 质量证明。

**隔离与历史保护**：选中凭据仅为固定合成值；禁发构建编译掉真实凭据加载与网络 continuation，guard 测试实际拒绝两条路径。原 `.env` 只参与既有保护哈希，不按真实凭据解析／使用；不访问真实会话、数据库内容或记忆目录。保留 §11.11 的原 79 项文件／6 项元数据证据，并在本轮重新核对，旧 0/10、1/10 及三个 0/1、0/8、0/1 批的封存状态与字节均不变。收尾保护、清理、文档前缀与 Git 差异见 [preservation.json（原件快照）](<evidence-index.md#archive-bb4b1bee4382eb65>)，当前附件指纹见 [manifest（原件快照）](<evidence-index.md#archive-50cd14d331bfa370>)。本轮没有真实 HTTP 状态、finish_reason 或供应商 usage，费用未知；后台额外触发 0，不把独立夹具写成同一数据链真实端到端通过，不证明 system/developer 语义等价。

**新的三批授权包（仅待批准，未执行）**：机器可读的 authorization-package.json绑定本轮实测版本：

| 当前执行候选 | 完整 SHA-256 |
|---|---|
| `controller-fix/plan_a.py` | `e5f0d968fa18b59623d8a7bb556f361a3151ce2b60b0419bf2f8b09f9c4b3f89` |
| `controller-fix/harness.rs` | `932e8ed182ce1b047dcabaffbc149e61962ae34dda89191d6493deb527a2b41c` |
| 原 `run_batch.py` | `f27ebcb6c09817afbe4110bae1a2a62e311309f2cd53586128aeea49747f44a3` |
| 原 `offline_cli_checks.py` | `e5331123be08f319b75a734b786ce4a8fe4ce9a837d6789e65bbb454d789e14b` |

冻结 v2 `148626298729769ad4e0461503a03ec951d6dff66916533d5ac3af803d21f82b` 是夹具／旧证据依据，不能混作新增适配 harness 的期望指纹。后续须复核全部当前指纹、配置／凭据来源、分支／HEAD、保护及新临时范围，离线预检后按同份源码生成 live 构建并另记二进制指纹；当前普通类型检查不代替该复核。

| 新独立批次（与所有旧批无关） | 新上限 | 输入与通过条件 |
|---|---:|---|
| `M7-P1V2-CFIX-2026-10-19` | 1 | 原 v2 合成会话；`SucceededWithOutput`、恰一行非空合同字段、job done／水位提交；合法空只封闭本批 |
| `M7-P2-CFIX-2026-10-19` | 8，含续轮 | 原独立 seed；三工具成功、两产物／v1、正常终文、SQL 提交及既有 Git 基线合同 |
| `M7-INJ-CFIX-2026-10-19` | 1 | 独立摘要＋唯一 system 尾段；COBALT-17、正常结束、自动记忆不入史 |

共同配置仍 `deepseek / deepseek-flash`、`openai-chat`、唯一 `https://api.deepseek.com/chat/completions`、`request_params={}`；源≤8 KiB、请求≤256 KiB、响应读取≤10 MiB、接受 JSON≤64 KiB；连接10秒／单请求180秒，三批240／1800／240秒，**整体2280秒从首次执行统一计时不重置**；内部重试0、后台0，合计最多10次发送尝试，额度不互借。只允许三个全新0700临时范围的合成数据；方案 A 区分单批失败与共同失败，不修正续跑、不自动重开。费用及服务端输出上限未知，无计费 token／金额硬预算，不按 usage 估价。建议一次批准新令牌 `M7-CFIX-THREE-BATCHES-2026-10-19`，随后才可调用 `prepare-authorized`／`execute-authorized-three-batches --authorization <新令牌> --work /tmp/013-m7-plan-a-<全新名>`；旧授权及其剩余额度一律不可复用。

本轮不暂存、不提交、不合并、不推送、不删除分支；未开始 M8/M9。离线修复报告与新授权包已准备，到此停止，等待新的真实调用授权。

### 11.13 CFIX 三批独立真实补验结果

**2026-10-19 结果：三个独立样本均达到各自目标，证据候选待审阅。** 用户已明确授权 CFIX 授权包，§11.12 及该包中的「待授权」保留为准备轮历史。本轮严格使用已排练的 `prepare-authorized`／`execute-authorized-three-batches` 入口各一次，没有修改控制层、harness、夹具或生产逻辑。真实发送 **1＋6＋1＝8/10**，控制入口 exit **0**，共同停止原因为空；三个账本均已封闭，phase2 未用 **2** 次封存，不追加探索。旧批次的判定、账本及封存额度保持原样。本轮总览见 summary.json。

**发送前复核与构建**：分支 `test/013-memory-m7-provider-check`、HEAD `799fbb972c7c4b500cd01d3f20ee99055a01ac6b` 符合预期，暂存区空、stash 0、单工作树。授权包四项源码、两份支持脚本、冻结 v2 完整指纹及原 475 项候选 manifest 均匹配；与离线保护起点比对的 106 项文件／6 项元数据也匹配。当前控制层／适配 harness 分别沿用 §11.12 的 `e5f0d968…c4b3f89`／`932e8ed1…a2b41c` 完整指纹；没有把冻结 v2 或历史二进制指纹作为新构建的期望值。完整 SHA、三个新根与绑定见 [发送前审计（原件快照）](<evidence-index.md#archive-f91f83ba9d45722e>)、READY 复核。

本轮实际禁发预检 **60 通过／0 失败／0 跳过**：Rust 8、CLI 31、监督 4、v2 专项 11、三个新根 dry-run 6。禁发二进制 `offline_only_build=true`，同份已审源码的 live 构建 `offline_only_build=false`；均从 HEAD 的隔离副本 `--offline --locked` 构建，使用本轮独立 target，阶段执行文件 0700，argv／cwd／新账本绑定检查通过。构建指纹如下；控制步骤、退出码与完整日志分别保留，不把 §11.12 的 74 项历史结果算成本轮重跑。

| 本轮二进制 | 完整 SHA-256 |
|---|---|
| 禁发 | `147b870b8fc2894c6b61fa3965b0e72bbb6d540d3f93959405581ac45d0bbe75` |
| live | `ead5505912adb85c7a7871a9cd4a28c80c334c5cd93fe1e75cc5bee74ff4e3db` |

辅助只读摘要查看曾将授权包的 `plan_a.py` 键误写成带目录的键，产生一次 `KeyError`、exit 1；该观察错误已留在 READY 复核附件。随后直接读取原授权包与 READY，按实际键结构核对匹配；已审准备入口 exit 0、60 项预检结果和源码均未变，没有修改或重跑控制入口，也未观察到共同前提失效。

**调用与隔离**：配置始终为 `deepseek / deepseek-flash`、`openai-chat`、`request_params={}`，唯一接口 `https://api.deepseek.com/chat/completions`。选中凭据引用 `env:DEEPSEEK_API_KEY`，本次来源为项目 `.env` 的该条目，只在内存使用，未输出值。三个 0700 新私有根各自使用合成会话／seed／摘要及临时 origin 授权；跨批只传计数快照。未使用真实 CLI/TUI 或后台管线入口，无额外接口探测，后台额外触发 **0**。没有访问真实会话、数据库内容或记忆目录；真实 `db/` 及已知库文件仅比对元数据。

| 独立批次／新根后缀 | 实际发送／累计 | 协议与目标 | 进程耗时／退出 | 封闭与未用额度 |
|---|---:|---|---|---|
| `M7-P1V2-CFIX-2026-10-19`／`749273bb` | **1／1** | HTTP 200、`stop`；**SucceededWithOutput，通过** | 11,708 ms／0 | 已封闭，0 |
| `M7-P2-CFIX-2026-10-19`／`92b37a5e` | **6／7** | 六次 HTTP 200，五次 `tool_calls` 后 `stop`；**通过** | 20,459 ms／0 | 已封闭，**2 次封存** |
| `M7-INJ-CFIX-2026-10-19`／`e9f9808b` | **1／8** | HTTP 200、`stop`；**通过** | 1,145 ms／0 | 已封闭，0 |

- **phase1-v2**：源会话 **1,020 bytes**，首句为「以下为本次测试构造的合成会话。」，其余内容与角色顺序沿用定稿 v2。既有 `StageOneOutput::parse` 严格解析为 `SucceededWithOutput`；临时 SQL 的 `stage1_outputs` **恰 1 行**，`raw_memory`／`rollout_summary`／`rollout_slug` 非空，stage1 job `done`，输入与成功水位均为 **1699974800**，`quick_check=ok`。据此新增**该合成样本的非空提炼覆盖**，不改判旧合法空样本。见 实际结果、SQL 证据。
- **phase2**：使用既有独立 seed，没有消费本轮 phase1 输出。模型实际发起且工具层成功执行 **7 次**：`mem_list` **2**、`mem_read` **3**、`mem_write` **2**；未手工补调用或产物。`MEMORY.md` **2,591 bytes**、`memory_summary.md` **2,288 bytes**，两产物校验通过，摘要首行恰为 **`v1`**；正常终文，`Succeeded { sql_committed: true }`，全局 job `done`、成功水位 **1699974800**、选中来源快照已提交。既有 `memory_workspace_diff` 的 Git 基线合同通过，四个被跟踪文件逐字节等于临时 Git HEAD，临时 diff 文件已删除；原始 `git status` 为 `?? memories.sqlite`，该运行库按既定合同排除，未把原始 porcelain 说成全空。见 实际结果、工具事件、产物、SQL／Git 证据。
- **注入用户轮**：独立合成摘要进入唯一 system 消息的尾段，`<memory-context>` 恰一次；正常回复为 **「环境代号：COBALT-17」**。内存历史 **2 条**，实际自动会话 **3 行**（含 meta），flush 成功、持久化错误为空，自动 memory-context 未入历史或落盘会话。仅证明这个固定请求样本被该供应商接受，**不证明 system/developer 语义等价**。见 实际结果、实际会话。

**逐请求真实响应**（耗时为发送入口至响应 JSON 接受；不是控制进程总耗时）：

| 请求 | HTTP | 耗时 ms | finish_reason | 输入 tokens | 输出 tokens | total_tokens |
|---|---:|---:|---|---:|---:|---:|
| phase1 #1 | 200 | 11,622 | stop | 6,621 | 2,585 | 9,206 |
| phase2 #1 | 200 | 1,086 | tool_calls | 11,876 | 83 | 11,959 |
| phase2 #2 | 200 | 1,192 | tool_calls | 12,738 | 150 | 12,888 |
| phase2 #3 | 200 | 9,310 | tool_calls | 13,358 | 1,840 | 15,198 |
| phase2 #4 | 200 | 3,638 | tool_calls | 15,225 | 625 | 15,850 |
| phase2 #5 | 200 | 948 | tool_calls | 15,878 | 37 | 15,915 |
| phase2 #6 | 200 | 3,629 | stop | 16,015 | 685 | 16,700 |
| injection #1 | 200 | 1,105 | stop | 5,949 | 115 | 6,064 |

phase1 usage 合计 **6,621＋2,585＝9,206**；phase2 **85,090＋3,420＝88,510**；注入 **5,949＋115＝6,064**。八个响应均提供 usage，本轮合计 **97,660 输入＋6,120 输出＝103,780 tokens**；原始字段还报告 cache hit **77,440**、cache miss **20,220**、输出 reasoning **2,238**，这些是 usage 子项，不再额外相加。完整原始 usage、response ID、请求／响应及事件按批保存，见 计量汇总。**费用未知**，无计费 token／金额硬预算，未据 usage 估价；远端输出 token 上限仍未知。

**限制与停止**：实际发送入口同时检查各批 1／8／1 与累计 10，内部 HTTP 重试 **0**；最终累计账本 **1／7／8**，各批最终封闭账本分别保留。源≤8 KiB、请求≤256 KiB、响应读取≤10 MiB、接受 JSON≤64 KiB；观测最大请求 **65,708 bytes**，最大接受响应 JSON **12,298 bytes**。连接10秒／单请求180秒、阶段240／1800／240秒；统一执行计时 **67,487 ms**，整体上限2280秒，切批未重置。三项均正常结束，无超时或失败发送；单批失败与共同失败的停止分支仅引用既有离线排练，不冒充本轮真实触发。三份夹具分别独立，**不能写成同一数据链端到端通过**；其他供应商、模型、参数、平台及未执行分支仍未验证。

**证据、清理与 Git**：本轮先校验并归档 100 份必要附件，再清理临时运行数据；另新增汇总、核对、清理与 manifest 等审阅附件。请求响应、工具结果、产物、SQL 查询结果、Git 快照、真实退出码、构建版本及独立身份／封闭账本均留存；没有保存认证头、凭据、临时授权文件或原始 SQLite 库。对保留附件检查当前凭据及其 JSON 转义形式均不存在。三个临时 origin 授权随各自运行目录删除；临时会话／数据库／记忆、可执行文件、构建副本与 target 已清理，仅保留封批身份、账本、摘要及不可续跑标记。清理前本轮可执行进程存活数为0，见 cleanup.json。

结果落盘前 **582 项既有保护文件／6 项元数据全部匹配**，既有两文档及完整 Git status 与发送前一致，见 执行后保护复核。本轮新增候选仅 `cfix-live-2026-10-19/` 与本节／规格必要状态；实施记录原文至 §11.12 完整保留为前缀，历史证据与并发保护项未覆盖。最终摘要指纹、文档差异与保护复核见 [manifest（原件快照）](<evidence-index.md#archive-a50e34a735b2ce5f>)、preservation。没有暂存、提交、合并、推送或删除分支，HEAD／main／stash／工作树保持起点；工作区仍保留待审文档及附件差异。生产代码未改，按本次明确边界**未重跑 workspace 全量、独立 typecheck 或 lint**，不声称这些门禁本轮通过。**M8/M9 未实施、V1 未完成；全部授权项目已结束，到此停止，等待结果审阅与证据交付批准。**

**2026-10-19 证据交付（用户批准后执行）**：上段「等待结果审阅与证据交付批准」为归档前状态；用户已批准提交、快进合并与删除验证分支。交付核验：分支 `test/013-memory-m7-provider-check`、基线 HEAD `799fbb972c7c4b500cd01d3f20ee99055a01ac6b` 与预期一致，暂存区空、stash 0；656 个批准路径（本文件＋规格＋`013-m7-provider-check/` 内 654 份附件）自 /tmp 生成清单逐文件显式暂存，未用目录级或全量暂存；暂存集合与批准清单精确一致（654 新增＋2 修改），[cfix-live manifest（原件快照）](<evidence-index.md#archive-a50e34a735b2ce5f>) 105 项与两文档 `document_sha256` 暂存 blob 全部匹配，保护项零进入。`git diff --cached --check` 仅在归档原始附件（捕获 diff／stdout／批次标记）内报 27 行尾随空白／EOF 空行——属已审阅原样字节，为保持 manifest 指纹不作改写；两份文档零警告。归档提交 **`9e32a6347ae96d9ac2ff14d2655bdaf784d92fd9`**（`test(013): 归档 M7 供应商补验证据`，656 files，+71096/−3）在验证分支创建后 `git merge --ff-only` 快进合并 main，合并后 main 文件树与该提交逐字节一致（`git diff` 为空）、包含关系经 `merge-base --is-ancestor` 证实。本段与规格状态翻转构成第二笔文档提交（`docs(013): M7 供应商补验证据标记为已交付`），其后以 `git branch -d` 安全删除验证分支（其提交已包含于 main）。历史 manifest 及其中 `document_sha256` 保持归档提交时的原值，对应 `9e32a63` 中的文档快照，不因交付状态更新而倒改。本轮结论维持 §11.13 边界：仅覆盖既定三个独立合成样本（三批实际 8/10 次、剩余 2 次封存、内部重试 0、后台额外触发 0），不推广为同链端到端、角色语义等价、其他供应商或 Windows；未推送、未发布、未操作远程、未追加真实请求。**M8/M9 未实施、V1 未完成。**
