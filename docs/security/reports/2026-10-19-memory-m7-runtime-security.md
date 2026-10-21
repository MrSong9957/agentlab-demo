# 013-M7 运行时接线与命令面安全测试报告（2026-10-19）

- 范围：M7 生产接线——两驱动 D2 触发位点、后台管线（`memory/runner.rs`）、threads 登记写入侧、M6 注入生产接线、D9 命令面（`/memory-status`、`/memory-reset confirm`）。
- 方法：手动按 `.agents/agents/Security-Testing-Gate.md` 执行（`security-review` skill 入口在本仓库无 origin/HEAD 可用——M4/M5/M6 同先例）；源码逐路径核对＋加强恶意输入的 M7 全目标实测（15/15＋e2e S81–S83）。
- 上游对照：`memories/write/src/{start.rs,control.rs,extensions/ad_hoc.rs}` @ `ce0323f2`。

## 1. 配置与路径

- `memories_root` 派生自 `catalog::user_dir()`（信任根控制变量已在 F1 名单——`TCHAT_USER_DIR` 不受 `.env` 注入；本轮零新增环境变量/信任根面）。
- `resolve_thread_meta` 的 `rollout_path` 由 `sessions/<thread_id>.jsonl` 拼接：`thread_id` 全链源自 `Session` 文件 stem（`file_stem()`，不含路径分隔符）→ 无遍历注入入口。恶意 `threads` 行需先获得 `memories.sqlite` 写权限（用户私有 0700/0600，M1 契约）——与上游 state 库同权限界，非本轮新增面；phase2 合并输入对该路径的读取受限于既定 mem_* 工具边界（M5 已审）。
- `load_memories_config` 装载失败按默认关闭（会话不因配置错误中断）；`deny_unknown_fields`（M1 面）继续约束 memories 段。

## 2. 后台生命周期

- `enabled=false` 零副作用（不 spawn、不建目录/库——变异证明 m1）；触发位点在 route/origin 门之后（门失败零触发——S83 实测）。
- 后台线程完全静默（无 stdout/stderr 面——上游 `tracing::warn` 在本地无日志面，登记差异）；失败按既有 error/retry/lease 契约入库（bg-fail 实测），不 panic、不上屏、不阻塞主循环（blocked 实测：后台慢响应期间主轮正常完成）。
- 退出不等待（句柄丢弃、不 join）——进程退出后台随进程终止，与手册 §7.1「不保证完成」一致。

## 3. 日志脱敏

- 后台失败原因入库前经 `redact_secrets`（M3 契约，phase1 内部承接）；
- `/memory-status` 仅输出开关状态、路径、存在性与**计数**——不显示 `last_error`/记录正文，无秘密泄露面；
- 命令输出路径文本来自本地用户目录（非远端内容），TUI 经结构化渲染、行式经 `display::sanitize_for_terminal`。

## 4. 重置边界（D9/A31）

- 缺 `confirm` 零删除（变异证明 m4）；确认参数大小写敏感、精确匹配。
- 符号链接根拒绝（变异证明 m5；`symlink_metadata` 判定，不触链接目标）；根内容删除用 `DirEntry::file_type()`（lstat 语义——条目为符号链接时 `remove_file` 只删链接本身）；现代 `remove_dir_all` 不跨符号链接。外部路径（sessions、记忆根外）零接触（A31 实测：原始会话保留）。
- 清理结果如实报告：清理后以真实目录状态复核，残留时报「部分清理」不虚报成功。
- 本地布局差异处置：`memories.sqlite` 在记忆根内——清根内容＝生成面＋状态面一次达成；在途连接写入落已删 inode、不污染新库（reset-inflight 实测：新库计数全零）。

## 5. 在途任务交互

- `/memory-status`（运行中可用）与后台写并发的 SQLite 冲突由 `busy_timeout(5s)`＋`journal_mode=DELETE`（M1 面）承接；
- `/memory-reset` 在 turn 运行中被 composer 门拦截（`available_during_task=false`，与 /save 同列）；
- 重复触发由 jobs 领取上限与 phase2 全局锁互斥（dedupe 实测：总请求数不随重复触发翻倍）。

## 6. 发现与处置

- 未发现新增可复现安全缺陷。
- 继承边界（非本轮新增，登记）：threads 行被恶意库写入后的路径读取面（同上游权限界）；后台管线对候选会话正文的全量读取再截断（M3 既有）；供应商角色语义未实测（待授权）。
- 测试修正两处前提缺陷（fd 目录删除的 inode 语义、`ensure_private_dir` 目录自愈）——均由变异轮暴露后按真实行为重设计，未放宽任何断言语义。

## 7. 定点补验轮补充（2026-10-19 第二轮：reset 与在途真实管线交互）

以慢合并响应（2s）受控暂停真实后台管线于 phase2 模型请求在途点（此时 claim/prepare/同步已完成），在途窗口内执行 `/memory-reset confirm`，释放后三面核查（`m7_reset_during_inflight_pipeline_full_interaction`）：

- **重置时刻**：记忆根内容全清（含 `.git` 基线、状态库、已同步文件），报告如实（「已清理」基于真实目录状态复核）。
- **文件面（在途写回，非原子边界如实登记）**：释放后管线继续执行 `mem_write` 工具批——按**路径**写入（不受已删 inode 影响）重建 `MEMORY.md`/`memory_summary.md`。重置不阻止、也不承诺阻止在途任务的路径写入（跨资源事务/回滚不擅自承诺）；用户可见语义＝「清理当时内容；在途任务可能随后写回部分产物」。
- **库面**：后台全部 SQL（含失败记账）落已打开的已删 inode——状态库文件不复活；下次管线/命令才重建全新干净库（既有 `m7_reset_with_inflight_connection_keeps_new_store_clean` 证新库零计数）。
- **Git 面**：基线 `.git` 随重置删除且不复活；后台后续 git 命令以显式 `--git-dir` 指向已删路径而失败（按 M5 失败契约记账，落已删 inode）；**无记忆根外泄漏**（sessions 目录与用户目录其余部分核查零 `.git` 残留）。
- **原始会话**：全程保留（A31）。

结论：实际行为与已批准重置语义（清理生成记忆与状态、保留原始会话、非原子边界）一致，无需修改用户可见契约、无新增决策项。退出语义同轮补证：TUI（S85）与行式（S86）在后台请求永悬（silent mock，真实 HTTP 180s 超时量级）时 `/exit`/`exit` 均在 15s 有界窗口内完成真实进程退出（码 0）；变异证明——退出路径插入 20s 等待时两用例均失败、恢复后通过。

## 8. 收口轮补充（2026-10-19 第三轮：重置失效边界实现＋触发计数）

**用户确认的新边界**（更正 §7 结论）：`/memory-reset confirm` 成功返回后，重置前启动的旧任务不得再回写生成记忆或重建基线；之后的新用户轮仍可按开关重新生成。实现（`runner.rs`/`mem_tools.rs`/`workspace.rs`/`phase2.rs`）：

- **代际凭据**：`.epoch`（reset 排他锁内清空＋自增）与 `.pipeline-epoch`（prepare 先于基线写入、随基线入库——不构成工作区差异）配对；写入有效性＝无 `.epoch`（从未重置）恒有效、有则须相等。
- **竞态排除（核心）**：reset 的「清空＋代数自增」在根目录 **flock 排他**临界区；`mem_write` 的「检查＋写入」在 **flock 共享**同一临界区（跨进程有效）——「检查通过→reset 返回→旧任务才写入」的交错被互斥排除（测试③以测试侧持共享锁证实 reset 阻塞、释放后完成、此后写入全拒）。
- **写入入口覆盖**：mem_write（工具批，锁内检查）；phase2 收尾在基线 reset/成功提交前核对凭据（失配按 `failed_reset_invalidated` 失败记账）；runner 各阶段边界（首次建库〔锁内检查＋建库〕/seed 前后/每 stage1 任务前/phase2 前）比对启动代数——旧任务不得重建库、布局或基线。SQL 记账落旧任务已打开连接（重置后＝已删 inode；实测写事务失败被既有 `let _` 语义吞没、行保持领取态——不污染新库）。
- **纵深防线说明**：runner 阶段检查与 SQLite 已删连接的写失败互为冗余（变异 m12 单独移除阶段检查无可观察行为差异——claim 写已删连接天然失败）；阶段检查保留以覆盖文件面先于连接面的窗口。
- **既有非原子失败边界不变**（§9.8 部分文件变化不回滚）；本轮边界是**重置完成保证**（返回后旧任务失效），两者分立。
- **布局差异如实登记**：本地库在记忆根内——reset 连 threads 登记一并清除（上游库在根外、登记保留）。后果：reset 后非活动旧会话不再进入候选池（原始会话文件保留，仅登记丢失）；当前活动会话经写入侧在新库重新登记、后续轮次照常生成（测试④）。属 M1 布局差异的固有代价，非新增决策面。
- **触发计数观察点**：`spawn_memory_pipeline` 生产壳（enabled gate 后）进程内原子计数，`/memory-status` 显示「本进程累计触发」——费用知情的可观察面；非日志系统/持久化审计/环境变量后门；跨进程各自计数（如实）。PTY 经屏幕断言（S87：入队不增/重放恰＋1；S88：关态与拒绝启动恒 0）。
- **变异**：m11（mem_write 守卫移除→②重建被抓）、m13（reset 排他锁移除→③互斥被抓）、m14（计数×2→S87 抓）全过；m12 如上说明排除。

## 9. 借用凭据交错轮补充（2026-10-19 第四轮）

**源码核实结论**：第三轮的 mem_write/收尾校验对象是共享配对文件（`.pipeline-epoch`×`.epoch`）——共享文件表达「记忆根当前属于哪一代」且被最新 prepare 改写，**不含任务自身凭据**；reset 后新代任务 prepare 会恢复配对 → 旧任务可借新代凭据恢复写入（缺陷成立，源码级）。任务不可变快照此前只存在于 runner 阶段边界。

**修复（任务快照贯穿）**：phase2 于全局任务**领取成功后**绑定 `task_epoch`（函数局部、不可变）；`ToolMode::MemoryMerge.expected_epoch` 传递至 `mem_tools::execute`；`mem_write` 在根目录共享锁内校验 `read_epoch(root) == expected_epoch`（与 reset 排他清空互斥不变、跨进程不退化）；phase2 prepare 前/同步前/收尾前同快照校验；共享凭据文件机制整体移除（共享面仅剩 `.epoch` 当前代数）。

**组合交错证明**（`m7_old_task_cannot_borrow_new_epoch_credentials`，闸门屏障＋全有效响应——无租期过期/违规/超时制造拒绝）：旧 phase2 挂起→reset→真实新代管线完整完成（新代产物/基线收尾/新库 job done）→释放旧任务（独特旧内容）→文件级断言：新代 MEMORY.md/摘要保持、无旧任务专属文件、**新代基线 HEAD 逐 sha 不变**、新库状态零污染、新代任务确实完成。变异：mem_write 快照校验移除被该测试抓出（RED✓）；收尾 9a 单独移除未检出——旧任务 heartbeat 写已删连接在所有权再确认步天然失败（§8 同因实证），9a 属纵深防线（行为等价，如实排除）。

结论：凭据绑定语义已按「共享文件＝当前代、任务快照＝任务代」分立；无新增配置面或通用任务框架。

## 10. 锁边界核查轮补充（2026-10-19 第五轮）

核查证实第四轮实现存在两个窗口（源码级）：①claim 与任务代数快照绑定不在同一锁域（reset 插入→旧任务把新代数读作自身凭据）；②prepare/同步/Git 收尾的检查与实际修改在锁外分离。修复＝锁域扩展：`runner::SharedWriteGuard`（根目录 SH flock）统一载体，phase2 三个临界区（claim＋绑定＋prepare／检查＋同步／9a＋heartbeat＋基线收尾）与 runner 的 seed 临界区——「检查＋修改」均为原子域，锁不越过模型请求（reset 不等待在途请求的保证不变，②新增 reset＜10s 时限断言）。修复过程实测并消除一个实现陷阱：`drop(&guard)` no-op（丢弃引用）曾使锁越界持有至 HTTP 超时（187s）——已修正并有变异检出（m17）。claim 绑定窗口（线程内语句间隙）无外部屏障可稳定复现——由「领取与绑定同一临界区」的锁域结构保证（m17 证明锁域行为可检）。跨进程能力不变（flock＋`.epoch` 均共享介质）。
