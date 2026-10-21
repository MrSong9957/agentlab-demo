# 规格：V1 记忆系统分阶段实施计划（013）

> 状态：**M1、M2、M3 已交付（2026-10-18 用户批准后合并，main @ `cf64a80`/`3d4e80e`/`63bff52`，原任务分支合并后删除）；M4 已交付（2026-10-18 用户批准后快进合并 main @ `4cd9f19`）；M5 已交付（2026-10-19 用户批准后快进合并 main @ `96e5908`，原任务分支合并后删除）；M6 已交付（2026-10-19 用户批准后快进合并 main @ `2aa0002`，原任务分支合并后删除）；M7 代码接线已交付（2026-10-19 用户批准后快进合并 main @ `67b9461`，原任务分支合并后删除；离线验证完成；CFIX 三项独立真实补验各自通过，补验证据已交付——归档提交 `9e32a6347ae96d9ac2ff14d2655bdaf784d92fd9` 经用户批准于 2026-10-19 快进合并 main 并删除验证分支，历史失败判定保留）；M8 已交付（2026-10-19 用户批准后快进合并 main @ `71e370e6`，任务分支 `feat/013-memory-m8` 合并后删除；D12-A 批准边界内：引用反馈＋便条差异层过滤，离线验证 2997/0/3）；M9 已交付（2026-10-19 用户批准后快进合并 main @ `58f6020`，任务分支 `feat/013-memory-m9` 合并后删除；离线完整回归 3010/0/3）；V1 于 2026-10-19 经用户最终批准，按既定范围已完成并交付（范围外/暂缓/未验证项按 §7/§8 原登记状态保留）**。
> 参考：`/vault/sources/agent/Codex记忆系统-框架实现与Agent执行手册.md`——移植规格 §18.1–18.6、验收矩阵 §19；机制依据 §4（目录/源码职责）、§6（SQLite 与 Markdown 分工）、§7（触发与候选）、§8（第一阶段）、§9（第二阶段）、§10（注入）、§11（搜索）、§12（修改/引用/遗忘/重置）、§13（配置与常量）。
> 范围：**V1**（§18.1 七项：单会话提炼、结果与任务状态、Markdown 同步、全局合并、摘要注入、通用文件检索、引用反馈与用户修改便条）。**V2、双写、专用工具（`memories.list/read/search/add_ad_hoc_note`）不纳入**——按参考文档 §18.1，源码中存在不构成扩展理由。
> 当前任务分支：无（M9 已交付：任务分支 `feat/013-memory-m9`、基线 `main @ ddeea8552ed8ef2b64585dbc22e481a82dc62d7d`、快进合并提交 `58f6020b3a21fbdd88cb56f193290045df6b4187`，合并后删除；准备历史及实施证据见 §6 M9）。历史交付记录：M8（任务分支 `feat/013-memory-m8`、基线 `main @ 1ea1a3b`、快进合并提交 `71e370e61fdb26c012a7c3370f27682ea0a99cbf`，合并后删除；M7 供应商补验证据已交付：验证分支归档提交 `9e32a63` 快进合并后删除；M7 代码接线已交付，原实施分支合并后删除；历史交付——M7：任务分支 `feat/013-memory-m7`，基线 `main @ 80f777898cf24a74a9669c478da9e8d7250b7029`，快进合并提交 `67b9461c12101f2cb4242bce43ad1c400d1f907f`；M6：任务分支 `feat/013-memory-m6`，基线 `main @ 1762958fb01b60673e681f3d42006b8df8b19d5e`，快进合并提交 `2aa000233536a2b9ceab2858344374207d6a94f9`；M5：任务分支 `feat/013-memory-m5`，基线 `main @ b43b2e79bc79ce3e38639ad29668ea768b7302fb`，快进合并提交 `96e590856b5efa0698e98b0ada385d8f37d950e0`；M4：任务分支 `feat/013-m4`，基线 `main @ 0b429aefa19772bd00fe251a51e6c74c4671b3f0`，快进合并提交 `4cd9f19e10393efa97c0fc8d9cb826fd9bdac611`）。每阶段按 AGENTS.md 门禁（tdd → comprehensive-testing → Security Testing Gate 适用性 → code-review → verify:full）。

> 修订：2026-10-21 并发迁移复核（后端回归候选，待批准）——M1 迁移在并发打开下必须幂等：两个连接都可能先在事务外读到「0001 未应用」，后到者重放不含 `IF NOT EXISTS` 的 DDL 会报 `table stage1_outputs already exists`。`apply_migration` 改为在 `BEGIN IMMEDIATE` 事务内复核 `schema_migrations`，已应用则提交跳过；新增 `memory::store::tests::migration_rechecks_after_concurrent_apply`（旧实现 RED、修复后 GREEN；移除复核的 M14 与把复核移到事务外的 M15 变异均可检出）。对应 2026-10-21 七项后端回归最终候选，证据 `docs/records/2026-10-21-backend-regression.md`；最终全量 exit 0、3045/0/3。
> 修订：2026-10-19 V1 最终收口轮——用户最终批准 V1 交付；既定范围内已完成并交付（M1–M9 全部已交付，矩阵实证与裁决登记闭合），范围外/暂缓/未验证项按原状态保留，本轮仅文档收口。
> 修订：2026-10-19 M9 实施轮——精确保留清理与同链验收已实现并验证（同日经用户批准交付，见 §6 M9 交付记录）；新增 13 用例、17 个非等价错误检出及 1 个等价排除，双轴审查修复闭合；最终 cargo check 与完整回归 3010/0/3 exit 0，矩阵回填，详见 §6 M9 和实施记录。首轮 S89/S90 夹具失败及修复保留历史；V1 未提前标记完成；实施轮无真实供应商调用，Git 交付动作仅发生在用户批准后的交付记录所述范围。

> 调查基线：main @ `f023e5c`（2026-10-18，工作区仅未跟踪 `db/`，本轮未触碰）。
> 修订：2026-10-18 补充调查轮——上下文组装全链证据（§2.1）＋记忆注入接入契约（§5.1）；D4/D11/G7/M6 据证据修订。仍未改生产代码；**D1–D11 均未获用户批准**，推荐项不是决定。
> 修订：2026-10-18 上游对齐轮——固定上游版本并建立精简复用清单（§2.2）；按「直接复用优先」修订 D1/D2/D3/D4/D7/D11/G8 与 §5.1、M1/M3/M5/M6/M7/M8；修正 §2.1 两处事实（本轮亦未改生产代码）。
> 修订：2026-10-18 定点修正轮——按完整 SHA 重核版本证据并勘误上轮两处错误（§2.2 版本事实）；G8 收敛为保留上游 git 基线机制（git 进程载体，快照降为替代实现并列差异）；复用清单补 SQL 事务/行数/回滚/所有权保留、完整适配清单（取代「仅四处」）、三类工具区分、合并轮具体权限边界与验收、三模板假设逐条核对（read_path JSONL 错述最小改写）；新增 M1 最小决策包（§6）。D1–D11 仍未获批准。
> 修订：2026-10-18 分支准备轮——任务分支 `feat/013-memory-v1`（基线 main @ `f023e5c`）建立；**用户已确认 D1（rusqlite(bundled)＋上游 SQL 移植，保留事务边界/回滚/行数判断/所有权校验）与 D5（记忆根 `<user_dir>/memories/`＋manual.json memories 段）**；M1 保持待实施，其余待决项维持原状、不视为随本次通过。
> 修订：2026-10-18 M1 实施轮——M1 实现并验证（**待批准**）：rusqlite 0.40.2(bundled)、`src/app/src/memory/`（DDL 逐字＋MemoryStore SQL 逐字移植＋threads 登记表）、catalog memories 段兼容；门禁全过（tdd RED→GREEN、comprehensive-testing 补强、Security Gate、code-review 双轴及修复、verify:full，证据 `docs/records/2026-10-18-013-m1-implementation.md`）；M2–M9 未实施。
> 修订：2026-10-18 M2 分支准备轮——确认 M2 无阻塞性待决项（M1 交付物就位；配置参数按 §4 上游默认在实施时扩展 MemoriesConfig，载体 D5 已批；D2/D3/D4/D6/D9/G8 均属后续阶段）；建分支 `feat/013-memory-m2`（基线 main @ `99eaf81`）；M2 标记待实施。
> 修订：2026-10-18 M2 实施轮——M2 实现并验证（**待批准**）：`memory/candidates.rs`（上游 `claim_stage1_jobs_for_startup` 编排移植）＋store 扫描/needs_update 两方法＋MemoriesConfig 三参数（默认/钳制/兼容）；上游常量与边界实施前逐项核实（§4 全对上）；门禁全过（tdd RED→GREEN、comprehensive-testing 补强至 18 用例、Security Gate 判定跳过详细门＋依据、code-review 双轴及发现处置、verify:full，证据 `docs/records/2026-10-18-013-m2-implementation.md`）；验收命令勘误＋§4 扫描上限落地口径回填（见 M2 条目）；M3–M9 未实施。
> 修订：2026-10-18 M3 分支准备轮——确认 M3 无阻塞性待决项（D6 默认模型/D7 证据条件/D11 换算均已嵌入契约；threads 登记接线收敛为 D2/M7、不属 M3）；M3 验收命令勘误（同 M2 病，M4–M6 同病待各轮修正）＋涉及模块补 rollout_input 输入组装与 HistoryItem 映射；建分支 `feat/013-memory-m3`（基线 main @ `651bbdf`）；M3 标记待实施。
> 修订：2026-10-18 M3 实施轮——M3 实现并验证（**待批准**）：`memory/{secrets,truncate,rollout_input,phase1_output,phase1}.rs`＋模板资产（逐字节）＋regex-lite 依赖（无新锁条目）；**D6 用户已确认（2026-10-18）**并登记；门禁全过（tdd RED→GREEN、comprehensive-testing 补强至集成 17＋单测 22、Security Gate 进入并通过、code-review 双轴——truncate 重复硬违规整改为复用 008 shim、verify:full，证据 `docs/records/2026-10-18-013-m3-implementation.md`）；M4 条目补 A13 导出面脱敏闭环注记；M4–M9 未实施。
> 修订：2026-10-18 M3 定点补验轮——①存储 I/O 故障两用例（目录改名注入，与失权拒提交分立有据；实现无缺陷，记录 §6a）；②结构化输出适配核准（官方资料＋请求体断言：不发送 `response_format`，模板承载请求侧契约＋严格解析承载响应侧保证，用户可经 `request_params` 自行透传；供应商实测待 M7）——M3 条目④重写、⑨补登记；全量门禁复跑见记录。M4–M9 未实施。
> 修订：2026-10-18 M3 文档勘误轮——字段层级表述更正（Chat Completions：`name/schema/strict` 位于 `response_format.json_schema` 内；Responses：HTTP 字段为 `text.format`，`output_schema` 系 Codex 内部参数）——仅注释与文档，生产逻辑零改动，复用 2856/0 验证。M4–M9 未实施。
> 修订：2026-10-18 M4 实施前确认与分支准备——核实 `main` 完整 HEAD 与报告一致，普通分支 `feat/013-memory-m4` 已从上述基线建立；M4 标记待实施。核对固定上游的文件管理、空集合、部分失败、布局符号链接清理及 A13 导出边界，勘误 M4 验收命令（详见 M4 条目）。无阻塞性待决项；G8/D3 属 M5、D4 属 M6，供应商兼容性/threads 登记/后台触发属 M7，其他决定未随本轮获批。本轮仅核对与规格登记，未运行测试、未实现 M4；M3 的 2856/0 为既有交接证据。

> 修订：2026-10-18 M4 实施轮——全量只读快照＋Markdown 同步已实现并验证（**待批准**）；L1 23/23、错误检测证明 11 项、安全检查与手工审查完成，最终 `cargo check` 通过、全量 **2879 通过/0 失败/3 既有忽略**（M3 2856＋新增 23）。指定门禁 Skill 本地缺失，按 AGENTS.md 手动执行，未冒称加载；证据 `docs/records/2026-10-18-013-m4-implementation.md`。本轮无真实模型调用，M5–M9 未实施，其他未确认决定保持原状。

> 修订：2026-10-18 M4 批准交付轮——用户明确批准提交、合并、核验及安全删除本次本地任务分支。实现提交 `4cd9f19e10393efa97c0fc8d9cb826fd9bdac611` 已快进合入 main，提交包含关系、合并树与 15 项候选源码/依赖/测试/黄金文件指纹核验通过；未引入代码变化，复用既有 `cargo check` 与完整回归 2879/0/3，M4 标记**已交付**。本次交付文档单独提交；Windows 权限未实测、clippy/rustfmt 未运行等说明保持，M5–M9 未实施，其他待决项未随交付获批。证据见 M4 实施记录 §9。

> 修订：2026-10-18 M5 实施前决策收敛轮——核实 main @ `b43b2e7`（分支/HEAD/暂存区与 M4 交接一致，既有 compose 修改与未跟踪 `db/` 未触碰）。代码事实核对：`turn.rs` `execute_tool`/`skipped_by_cancel` 硬绑两工具名、未知工具文案含「可用：exec_command、apply_patch」；`prompt.rs:290` 工具数组硬编码（D3 参数化注入点确认）；`exec.rs` 无内部非 shell 子进程原语（git 原语须 memory 模块私有新写，见 G8 确定性契约）；`memory/templates/` 缺 `consolidation.md`（M5 从固定上游拷贝）；phase2/workspace 骨架不存在（M5 新建，符合计划）。**D3/G8 推荐细化登记于 §2.2 两节，均保持待用户批准、不因写入规格视为已批**：mem_* 读写拒绝隐藏路径（`.git/`、`.gitignore` 不可触达，构成模型写入与 Git 元数据保护边界）；git 内部调用确定性契约（不经 shell、配置隔离、env 清除、固定 identity、`add -A -f`、差异计算前置 `add -A`——`git diff HEAD` 不含 untracked）。勘误 M5 验收命令（空转病，同 M2–M4 先例）。本轮仅核对与规格登记：未建分支、未实现 M5、未改生产代码或测试、未运行测试；M4 的 2879/0/3 为既有交接证据。

> 修订：2026-10-18 M5 决策包定点修正轮（G8 收口）——核实 main @ `b43b2e7`、上轮规格修改保留、compose 修改与未跟踪 `db/` 未触碰。固定上游源码核对（@ce0323f 网页拉取 baseline.rs/workspace.rs/phase2.rs/start.rs）：①上游 `memories_1.sqlite`/`state_5.sqlite` 位于 CODEX_HOME 根、**在记忆根外**（手册 §4.1），基线全量遍历仅跳 `.git`，库文件天然不参与比较；②`phase2_workspace_diff.md` 在 prepare/diff 计算/reset 三处前置删除（NotFound 忽略），从不入基线；③phase2 编排＝**领取全局锁在前**、prepare/ensure 在后，无差异短路**不 reset 基线**、reset 在 SQL 成功提交**之前**——上一轮报告「ensure 失败→本轮不领取」表述有误，以源码为准更正（ensure/diff/reset 失败均属领取后失败、按 retry 记账）。本地差异核实：`memories.sqlite` 在记忆根内（M1 落地，store.rs:1040）且 `journal_mode=DELETE`（store.rs:154）→ 三原语统一排除 `memories.sqlite`＋`memories.sqlite-journal`（否则任务记账写入使 A17 失效）。一次性临时仓库探测（mktemp 目录，结束清理，未触碰工作仓库/真实记忆根）：`add -A`（无 `-f`）在 `.gitignore=*.md` 下新摘要不可见、`add -A -f` 可见；排除 pathspec 后库写入不产生差异、完整重跑差异为空；`commit --amend` 连续 reset 后 `rev-list --count`＝1（单 commit 基线）、unborn 上 amend 必败；`GIT_CONFIG_COUNT` 注入 `diff.noprefix` 去前缀/`core.excludesfile` 改变未跟踪可见集（未隔离有影响、`COUNT=0`＋`GLOBAL/SYSTEM=/dev/null` 恢复）；无全局 identity commit exit 128、`-c` 固定值恢复 0；`GIT_DIR` 泄漏 fatal；`rev-parse --verify HEAD` unborn/损坏≠0、健康 0、损坏后 `git init` 幂等 reinit 可走初建。G8 契约据此修订（§3 G8 行「内部调用确定性与调用序契约」全量替换＋§2.2 baseline 行＋M5 涉及模块/先失败测试补基线与配置隔离契约）。**D3、G8 保持待用户批准**；本轮未建分支、未实现 M5、未改生产代码或测试、未在工作仓库提交。

> 修订：2026-10-18 M5 规格确认登记与分支准备轮——核实 main @ `b43b2e79bc79ce3e38639ad29668ea768b7302fb`（分支/HEAD/暂存区与 M4 交接一致；既有 compose 修改、records 修改与未跟踪 `db/` 未触碰）。普通分支 `feat/013-memory-m5` 已从该基线建立，M5 标记待实施。**D3、G8 用户已确认（2026-10-18）**并翻转各待批标记（§5 D3 行、§5.1 待用户决定与决策状态汇总、§2.2 两节、§3 G8 行、M5 条目）；批准的保护边界与相应验收要求全文登记于 §6 M5「实施前确认」与「D3/G8 批准边界与验收」两节——要点：D3 注册表恰为 `mem_read`/`mem_write`/`mem_list`、不提供 exec_command、限记忆根（拒绝绝对路径/父目录越界/符号链接）、隐藏路径与 `memories.sqlite`/`memories.sqlite-journal` 不向模型暴露（不可读/写/列表展示）、保留文件类型/参数/输出预算限制；G8 内部固定外部 git 进程、仅操作记忆根自有专用仓库（父项目仓库不得改变）、三原语一致文件集合与忽略处理、验收补「曾被跟踪的排除文件」与「reset 单提交语义适用前提实测」两项、先领取全局锁再 prepare/ensure、无差异短路不 reset、reset 在 SQL 成功提交之前、保留所有权/重试/非原子失败契约。**其他待决项（D2/D4/D9/D11 等）不视为随本次批准通过**。本轮仅分支准备与规格登记：未实现 M5、未改生产代码或测试、未运行测试（M4 的 2879/0/3 为既有交接证据）、未提交；未执行的验证保持待验证。

> 修订：2026-10-18 M5 实施轮——第二阶段合并已实现并验证（**待批准**）：`memory/{phase2,workspace,mem_tools}.rs`＋`templates/consolidation.md`（上游逐字节，51,237 字节 sha256 核对）＋turn/prompt 参数化（`ToolMode`——用户轮契约逐字节不变，合并轮分派层拒绝未注册工具）＋store 来源选择 SQL＋config 两参数（256/30，钳 1–4096/0–365）。门禁全过：tdd RED 4/25→GREEN 33→补强 38/38（错误检测证明 9 项变异「破坏→失败→恢复→通过」，符号链接三层冗余不可破坏如实登记）；Security Gate 手动（`security-review` skill 因无 origin/HEAD 入口失败，M4 先例）；code-review skill 双轴——标准轴 1 硬违规（未用心跳常量→改 90s 节流真实承载）＋5 判断题全处置（env 竞争改子进程承载、读线程宽限、SQL 提取、API 合一）；规格轴 3 项测试缺口全补（§9.8 不回滚、failed_workspace_status＋重试记账、containment 全树＋合并进度）。**本轮实测登记的契约细化（均在批准边界内，机制留白回填）**：①G8 ensure 不可用分支＝删除记忆根自有 `.git` 后全新初建（上游破坏性替换同义；规格原「init 幂等修复」对损坏 HEAD 不成立——reinit 不重写 HEAD，实测）；②「曾被跟踪的排除文件」机制＝reset 前置 `git rm --cached -q --ignore-unmatch`（仅两固定名、只动索引不删真实文件；仅排除 pathspec 时旧基线仍含该文件，amend 会保留——故必须 rm）；③基线提交时间确定性＝初建固定纪元、amend 取 HEAD 时间＋1s（真实时钟同秒同树 amend 产生相同 SHA 的坑，实测修复）；④git 仓库锚定＝每条命令显式 `--git-dir/--work-tree`（批准边界「不得发现父仓库」的机制承载，变异证明）；⑤D3 从严边界：排除文件名**任意层级**同名即拒（保守扩展，无害）。全量 **2923/0/3**（M4 2879＋新增 44），`cargo check` 0 错误。证据 `docs/records/2026-10-18-013-m5-implementation.md`。本轮无真实模型调用、未操作真实记忆目录、未提交；M6–M9 未实施，其他未确认决定保持原状。

> 修订：2026-10-18 M5 心跳退出边界定点收口轮——核实并更正上轮登记：`HEARTBEAT_THREAD_BELT`（10s）位于**生产**心跳线程循环内（`Instant` 墙钟计时、无测试门控）——非测试看门狗，而是生产生命周期的墙钟提前退出（模型请求在途超 10s 即停跳），**全程续租契约缺口成立**，已在既有授权内修复：删除墙钟带，线程唯一退出条件＝停止位；panic 展开由 `StopHeartbeatOnDrop`（chat 前绑定、Drop 置位）收口，`thread::scope` 展开路径 join ≤一个轮询周期不挂死。tdd：RED＝「请求在途、真实墙钟跨 10.5s（跨过旧停止条件）、业务时钟再进 90s——心跳须仍续租」（修复前失败复现：租期停在跨前值）；GREEN 含 panic 路径用例（catch_unwind 收口＋panic 后线程不再续租＋任务行保持 running 由租期到期接管承接）。code-review 双轴：行为符合契约（阻塞期全程覆盖、退出仅由请求生命周期驱动、续租不被最终所有权检查替代、M1 所有权语义零改动）；文本漂移已更正；判断题处置＝`stop_flag` 改名、删死 allow、测试批提取；`last` 时钟初始化因竞态回退（记录 §8-P8）。套件 40/40（＋2 用例）、全量复跑见记录 §7。本轮未提交、无真实模型调用。

> 修订：2026-10-18 M5 心跳契约修复与超时补验轮——**纠正上轮「心跳＝请求前 90s 节流」的登记（不满足批准契约）**，落实真实生命周期：①每请求前确定性续租；②**请求阻塞期间**由 `thread::scope` 心跳线程按受控时钟每 90s 续租（上游事件循环周期心跳的同步阻塞等价承载；真实 5ms 轮询注入时钟——退出条件唯一为停止位、无墙钟自限时，panic 由 Drop 保护收口，见同日「心跳退出边界定点收口轮」更正）；③请求返回即停。RED 复现：mock 请求未完成＋受控时钟跨 3 个 90s 点，修复前租期停在请求前值（`Some(T0+3600)`）→ 新用例失败；修复后通过且 4 次复跑稳定，断言含「请求未完成时租期已续期」与「有效续租期间第二领取者恒 SkippedRunning」。a22 相应重写：租期驱动接管已被心跳阻断（契约生效证据），失权终态改经直接改行模拟「已被他人令牌占据」——再确认 `Ok(false)`→不 reset/不成功/不替接读者写失败（旧所有者不能越权提交；M1 所有权语义零改动）。超时补验（此前「未单独行为测试」限制作废）：crate 私有可测缝＋假 git（后台 sleep＋双 pid 记录）＋300ms 生效超时，M5 公共入口直达超时路径——超时触发（非取消/路径/权限）、进程组含后代全灭（/proc State≠Z 实证；本容器 PID1 不收割孤儿、僵尸条目残留，终止即清理完成）、Err 报生效毫秒、基线未建立不伪报成功；过程修复两处真实缺陷（报文误报常量 60,000ms、init 分支未经统一程序缝）。受影响回归＋全量复跑通过（记录 §0/§7）。本轮仍未提交、无真实模型调用。
> 修订：2026-10-19 M5 批准交付轮——用户明确批准提交、快进合并 main 及核验后安全删除任务分支。实现提交 `96e590856b5efa0698e98b0ada385d8f37d950e0`（14 文件 4639+/37-，树 `a7355825`）经 `--ff-only` 合入 main（b43b2e79→96e5908），包含关系（`merge-base --is-ancestor`）与树一致（main 树＝提交树、`git diff` 为空）核验通过，未引入候选外代码变化——复用既有最终验证 `cargo check` 0 错误＋全量 **2926/0/3**（提交时代码与 §7 验证时点同态：最终源码改动均早于最终测试构建、其后仅 .md 更新）。交付前基线与保护项核对（compose +10 行、2026-10-02 记录 +33 行、`db/`、`.agents/` 四项原样未入提交、无 stash）见记录 §10。M5 标记**已交付**；M6–M9 未实施、V1 整体未完成、其他决策状态不变。本轮不推送、不操作远程、无真实模型调用、未触碰真实记忆目录。

> 修订：2026-10-18 M6 实施前确认与决策收敛轮——核实 `main @ 1762958fb01b60673e681f3d42006b8df8b19d5e` 与 M5 交接一致；暂存区/stash 为空，`feat/013-memory-m6` 名称可用，四项保护内容原样保留。按当前本地调用链与固定上游源码核实：M5 已提供通用请求组装参数化入口，M3 截断实现可直接复用；M6 唯一阻断为 **D4 注入角色适配待用户确认**（最小决策包见 §6 M6），预算/按轮读取沿已有契约，未将 M5 批准扩大解释为其他决定获批。勘误 M6 验收命令并区分本阶段能力验证与 M7 驱动接线。**M6 保持未实施、功能验证均待验证**；本轮只改本规格，未建分支、未改代码或测试、未运行 Cargo 检查/测试、未提交/合并/推送、未调用真实模型或触碰真实记忆目录；历史修订保留。

> 修订：2026-10-19 D4 批准登记与 M6 分支准备轮——用户已确认 D4，批准边界全文见 §6 M6；其他决策不随本轮批准。核实 `main @ 1762958fb01b60673e681f3d42006b8df8b19d5e`、暂存区/stash 为空、四项保护内容及 013 既有准备登记保留后，新建普通分支 `feat/013-memory-m6`（合并目标 `main`）；HEAD＝main＝merge-base＝上述完整 SHA，领先/落后均 0，未创建 worktree。D4 当前状态、§5.1 契约/待决清单、决策汇总与 M6 准备状态同步更新，历史记录保留。**M6 待实施、功能验收均待验证**；本轮仅分支准备及规格登记，未运行 Cargo 检查/测试，未改生产代码/测试/配置/会话数据，未访问真实记忆目录或调用真实模型，未暂存/提交/合并/推送/删除分支。

> 修订：2026-10-19 M6 实施轮——摘要读取/模板、`use_memories` 缺省兼容、D4 唯一 system 尾段及显式用户轮快照接口已实现并验证（**待批准，未暂存/提交**）；直接复用 M3 截断与 M5 请求/工具循环，模板仅作已登记的 Markdown/原始 JSONL 说明适配。18 项新增集成用例及 8 项「破坏→失败→恢复→通过」证明；安全门与双轴 code-review 无未处置发现。最终 `cargo check` exit 0，全量 **2944 通过/0 失败/3 既有忽略、0 过滤**（M5 2926＋新增 18），定向组合 107/0/0、配置/工具循环补充 168/0/0、CLI/PTY 95/0/0；Clippy exit 0（保留既有告警）、新文件 scoped rustfmt check 通过。完整来源、命令、计数和方法见 [M6 实施记录](../records/2026-10-19-013-m6-implementation.md)。已有准备/批准历史保留；M7–M9 未实施、V1 未完成，其他决定不随本轮获批，无真实模型调用或真实记忆目录操作。

> 修订：2026-10-19 M6 批准交付轮——用户明确批准提交、快进合并 main 及核验后安全删除任务分支。实现提交 `2aa000233536a2b9ceab2858344374207d6a94f9`（12 文件 1434+/22-，树 `7b26aeca030c81cf351a29b6d22d09bf0298c172`）经 `--ff-only` 合入 main（1762958→2aa0002），包含关系（`merge-base --is-ancestor`）与树一致（`git diff` 为空）核验通过，未引入候选外代码变化——复用既有最终验证 `cargo check` 0 错误＋全量 **2944/0/3**（候选与验证同态：12 文件 SHA-256 与最终验证指纹逐一相符、工作区经候选补丁逆向应用核验；合并零代码变化）。交付前基线与保护项核对（compose、AGENTS.md、宗旨删除、2026-10-02 记录、`db/`、`.agents/`、2026-10-18 记录均原样保留、未入提交，无 stash）见记录 §9。M6 标记**已交付**；M7–M9 未实施、V1 整体未完成、D4 及其他决策状态不变。本轮不推送、不操作远程、无真实模型调用、未触碰真实记忆目录。

> 修订：2026-10-19 M7 实施前确认与决策收敛轮——核实 main @ `80f777898cf24a74a9669c478da9e8d7250b7029`（M6 交付文档提交 `0fdd2a1` 在祖先链，其后 4 笔为用户确认的并发整理提交：宗旨并入 AGENTS.md、容器代理＋忽略规则、技能安装记录补录、历史误提交垃圾文件删除）；工作区干净、无 stash、单工作树、无远程，`feat/013-memory-m7` 名称合法且未占用。保护项指纹逐一核对一致（本地忽略项 `db/wiki.db`、`.agents/` 三文件；已入库 compose/AGENTS.md/两记录/.gitignore；宗旨文档保持删除），未打开数据库、未触碰真实记忆目录。按本地源码＋固定上游 `@ce0323f2`＋参考手册核对 M7 真实范围与代码事实（逐项见 §6 M7「实施前确认」）；**M7 真实阻断＝D2 触发与 D9 命令面两项待用户批准**（最小决策包见 M7 条目），其余为实现细节不上升决策；M7 验收命令勘误。**M7 保持未实施、功能验收保持待验证**，M8–M9 状态不变、其他决策不随本轮获批；本轮仅改本规格（`git diff --check` 通过），未建分支、未改生产代码/测试/配置、未运行 Cargo 检查或测试、未暂存/提交/合并/推送、未调用真实模型；历史修订保留。

> 修订：2026-10-19 D2/D9 批准登记与 M7 分支准备轮——**用户已确认 D2 与 D9（2026-10-19）并批准后续代码实施**，批准边界全文见 §6 M7「批准边界」；其他决策不随本轮获批、D4 及既有批准不变。核实实际 main @ `80f777898cf24a74a9669c478da9e8d7250b7029`（M6 交付参照点 `0fdd2a1` 包含于 main，其后 4 笔并发提交——`f1c2887` 宗旨并入 AGENTS.md、`a3b6623` 容器代理＋忽略规则、`0b7feba` 技能记录补录、`80f7778` 历史垃圾文件删除——全部为文档/配置/无引用占位文件清理，**无生产代码或运行资产变化**；原保护项 compose/AGENTS.md/两记录/.gitignore 分别随 `a3b6623`/`f1c2887`/`0b7feba` 入库，`db/`、`.agents/` 仍为本地忽略项、指纹一致；既有 2944/0/3 属 `2aa0002` 树验证，不自动算作新基线结果，实际基线全量留待实施阶段执行）。暂存区/stash 为空、单工作树、无远程，全部保护项指纹核对一致（未打开数据库、未访问真实记忆目录）后，从实际 main 同点新建普通分支 `feat/013-memory-m7`（HEAD＝main＝merge-base，领先/落后 0/0，未创建 worktree）。D2/D9 状态、决策汇总、M7 条目标注与准备状态同步更新，历史记录保留。**M7 待实施、功能验收保持待验证；真实供应商补验尚未授权，继续标待验证（mock 推进代码实施与离线验收，不得冒充真实供应商验证）**；M8–M9 不提前实施。本轮仅规格登记（`git diff --check`），未改生产代码/测试/配置、未运行真实模型、未暂存/提交/合并/推送。

> 修订：2026-10-19 M7 实施轮——两驱动 D2 触发、threads 登记写入侧、M6 注入生产接线（`UserWithMemory`）、`memory/runner.rs` 后台管线与 D9 命令面（`/memory-status`、`/memory-reset confirm`）已实现并离线验证（**待批准，未暂存/提交**）。实际基线全量在 `80f7778` 副本实测 **2944/0/3**（非沿用）；本轮新增 `memory_runner` 15 用例＋e2e S81/S81b/S82/S83（PTY 3＋管道 1），全部经生产入口＋mock HTTP＋临时目录；8 项「破坏→失败→恢复→通过」错误检测证明（enabled gate/登记检测/写失败/缺确认/符号链接/status 建库/seed 覆盖/TUI 注入漏接）；安全门手动（报告 `docs/security/reports/2026-10-19-memory-m7-runtime-security.md`）与双轴审查无未处置发现。最终 `cargo check` exit 0，全量 **2963 通过/0 失败/3 既有忽略**（2944＋19），定向组合 99＋18＋15（e2e/inject/runner）、24＋6＋19＋40（session/prompt/phase1/phase2）、129＋18＋21（lib/candidates/store）全绿；clippy exit 0（新文件零告警）、scoped rustfmt 通过。vendored `slash_command.rs` 仅 008-ADAPT 式追加。**真实供应商补验待授权、待验证（全 mock，不冒充）**；M8–M9 未实施、V1 未完成。完整来源、变异表、限制（后台静默/排队重放位点依据/Windows 未实测）见 [M7 实施记录](../records/2026-10-19-013-m7-implementation.md)。本轮无真实模型调用、未操作真实记忆目录、未暂存/提交/合并/推送/删分支。

> 修订：2026-10-19 M7 定点补验轮（生命周期与重置）——核实四项证据：①取消与退出补齐真实进程观察（`m7_turn_cancel_does_not_undo_pipeline`＋e2e **S85**（TUI：后台 phase2 悬于 silent mock＝180s 超时量级，`/exit` 15s 窗口内真实退出码 0）与 **S86**（行式管道子进程同构）；变异证明——退出路径插入 20s 等待→双用例失败→恢复通过）；②排队重放（**S84**：turn1 运行中入队→重放，含排队文本的用户轮请求恰 1 份＝重放恰一次；「入队不触发」以 vendored `chat_driver` 位点静态证据（`queue_input` 与 `spawn_turn` 钩子分离、重放唯一经同一钩子）支撑——后台管线启动次数无黑盒物理信号，如实登记观察限制）；③reset 与在途真实管线完整交互（`m7_reset_during_inflight_pipeline_full_interaction`：慢合并响应受控暂停于 claim/prepare/同步完成点→在途窗口 reset→释放后**三面文件级证据**——在途 `mem_write` 按路径重建 Markdown（非原子边界如实）、状态库/Git 基线不复活、记忆根外零 `.git` 泄漏、原始会话保留；与已批准语义一致，无契约修改、无新增决策项，安全报告 §7 登记）；④既有缺口如实引用（S83 route 失败零触发、S82 命令零请求）。`memory_runner` 15→**17**、e2e 99→**102**；变异累计 **10 项**全过；补验后 `cargo check` exit 0、全量 **2968/0/3**（2963＋5，对账一致）。**M7 代码接线已实现并离线验证，待批准；真实供应商补验待授权、待验证**；M8–M9 未实施。本轮未暂存/提交/合并/推送/删分支，无真实模型调用、未触碰真实记忆目录。

> 修订：2026-10-19 M7 收口轮（重置失效边界＋后台触发计数）——用户确认重置边界并更正前轮结论（§5 D9 已同步：confirm 成功返回后重置前旧任务不得再回写生成记忆或重建基线；新用户轮仍可按开关重新生成；此前「在途写回属已批准语义」系误判、历史保留；非原子失败边界与重置完成保证分立）。实现＝记忆根内代际凭据（`.epoch`×`.pipeline-epoch`）＋根目录 flock 互斥（reset 排他清空＋自增／mem_write 共享锁内检查＋写入——交错竞态被互斥排除，跨进程有效）＋phase2 收尾前凭据核对（`failed_reset_invalidated` 记账）＋runner 各阶段边界代数比对；SQL 实测＝失效任务记账落已删连接、写事务失败被既有 `let _` 吞没（行保持领取态，如实断言）。**布局差异如实登记**：本地库在记忆根内→reset 连 threads 登记清除（上游根外保留）——非活动旧会话退出候选池、活动会话照常重登记生成（M1 布局固有代价，非新决策面）。触发计数＝生产壳 `spawn_memory_pipeline`（enabled gate 后）进程内原子计数＋`/memory-status` 显示（费用知情观察面，非日志/持久化审计/环境变量后门）；S87（入队不增/重放恰＋1）/S88（关态与拒绝启动恒 0）经真实入口断言。测试基建加 `MockResp::gated` 闸门（屏障同步，替代固定延时）；四场景失效测试＋库级计数全过；修复两处实现/测试缺陷（凭据先于基线写入〔M5 git_diff 用例抓到〕、S81 并发顺序假设）。变异 4 项（mem_write 守卫/reset 排他锁/计数×2 全检出；阶段检查单独缺失＝行为等价纵深防线，如实排除）；累计 14 项。最终 `cargo check` exit 0、全量 **2974/0/3**（2968＋6，对账一致）。**M7 代码接线已实现并离线验证，待批准；真实供应商补验待授权、待验证**；M8–M9 未实施。本轮未暂存/提交/合并/推送/删分支，无真实模型调用、未触碰真实记忆目录。

> 修订：2026-10-19 M7 借用凭据交错轮——源码核实：第三轮 mem_write/收尾校验对象为共享配对文件（被最新 prepare 改写），**旧任务可借新代 prepare 恢复写入**（缺陷成立，RED 复现＝旧任务覆盖新代 MEMORY.md）。最小修复＝**任务快照贯穿**：phase2 领取成功时绑定 `task_epoch`（不可变）→`MemoryMerge.expected_epoch`→mem_write 锁内 `read_epoch==快照` 校验＋prepare 前/同步前/收尾前同校验；共享凭据文件机制移除（`.epoch` 仅表当前代）；flock 互斥与跨进程能力不变、reset 后新任务照常。组合交错测试（旧挂起→reset→真实新代完整完成→释放旧任务有效旧内容批）文件级断言全过（新代产物/基线 HEAD/新库零污染＋新代确实完成）；变异 16 项累计（mem_write 快照检出✓；收尾 9a 单独移除＝heartbeat 写已删连接天然兜底，纵深防线如实排除）。适配 M5/M6 测试构造（`expected_epoch: 0` 无重置场景恒过，断言语义不变）；`memory_runner` 22、全量 **2975/0/3**（2974＋1）。候选 19 文件（＋`turn.rs`）。**M7 代码接线已实现并离线验证，待批准；真实供应商补验待授权**；M8–M9 未实施。本轮未暂存/提交/合并/推送/删分支，无真实模型调用、未触碰真实记忆目录。

> 修订：2026-10-19 M7 锁边界核查轮——源码核查证实第四轮实现两个窗口：①`claim` 与任务代数快照绑定不在同一锁域（reset 插入→旧任务把新代数读作自身凭据）；②prepare/同步/Git 收尾的「检查与修改」锁外分离。修复＝锁域扩展（`SharedWriteGuard` SH flock 统一载体；phase2「claim＋绑定＋prepare」「检查＋同步」「9a＋heartbeat＋基线收尾」三临界区＋runner seed 临界区＋mem_write 换统一 guard）——检查与修改均为原子域、锁不越过模型请求（reset 不等待在途请求不变，②补 reset＜10s 时限断言）。修复中实测并消除 `drop(&guard)` no-op 陷阱（锁越界→reset 阻塞 187s）。变异 m17（锁越界→②191s 失败✓检出）；m18（claim 去锁）无稳定检出（线程内语句间隙、无外部屏障）——由锁域结构承载、如实登记。累计变异 18 项；全量 **2975/0/3**（无新增用例、代码变化实测）。**M7 代码接线已实现并离线验证，待批准；真实供应商补验待授权**；M8–M9 未实施。本轮未暂存/提交/合并/推送/删分支，无真实模型调用、未触碰真实记忆目录。

> 修订：2026-10-19 M8 实施轮——D12-A 用户批准（便条差异层过滤契约调整，批准登记见 §5 D12）；`memory/feedback.rs`（citations/usage 移植＋本地形状检查）、`store.record_stage1_output_usage`（上游 SQL 逐字移植）、两驱动完成位点接线、G8 差异处便条过滤（状态列表与正文同源）落地；新增 22 用例（单测 13＋集成 7＋e2e S89/S90）全部实跑，RED→GREEN 与变异 m1–m5 错误检测证明完成；全量 **2996/0/3**（基线 2975＋21）、`cargo check` exit 0。clippy/rustfmt 未运行（组件下载失败，如实登记）。M8 **已实现并离线验证、待批准**；真实供应商行为不重测（本轮零真实请求）；M9 未实施、V1 未完成。
> 修订：2026-10-19 M7 代码接线交付轮——用户明确批准提交、快进合并 main 与核验后删除任务分支（批准范围不含真实供应商补验/推送/M8–M9）。交付前对**当前精确代码**（含锁边界轮全部修复与格式化）实跑 `cargo check` exit 0＋全量 **2975/0/3**（46 套件——非沿用）；21 候选路径逐项核对（任务代数贯穿 turn/mem_write/收尾、claim＋绑定＋prepare 同锁域、同步/写入/Git 收尾检查与修改同临界区、模型请求期间不持锁、后台启动计数与 S87/S88 补验在位）。实现提交 `67b9461c12101f2cb4242bce43ad1c400d1f907f`（21 文件 3714+/470-，树 `7d7b761f`，父＝基线 `80f7778`）经 `--ff-only` 合入 main（80f7778→67b9461），包含关系与文件树一致（`git diff` 为空）核验通过——引用交付前实测 2975/0/3，不机械重跑。保护项全程核对一致（`db/`、`.agents/` 本地忽略项指纹不变、未打开数据库；并发 `CLAUDE.md`（单行 @AGENTS.md 转发）保留未跟踪不入库）。M7 标记**代码接线已交付、离线验证完成**；**真实供应商补验待授权／待验证（保留——不以 mock 冒充）；m18 未稳定检出由锁域结构承载、其余等价变异排除与继承限制均保留原文**；M7 整体与 V1 不标全部完成，M8–M9 未实施，D2/D4/D9 等已确认边界不变。本轮不推送、不操作远程、无真实模型调用、未触碰真实记忆目录。

## 1. V1 闭环目标（本项目语义）

在现有 CLI（TUI/REPL）上实现记忆闭环：

1. **触发**：一轮带输入的会话成功开始后，后台检查并运行两阶段任务（非向运行中轮次追加输入；进程退出不保证继续）。
2. **候选**：从 `sessions/` 目录选择其他会话（闲置/年龄/已处理版本过滤，领取上限）。
3. **提炼（第一阶段）**：按预算读取会话记录 → 秘密处理 → 调用提炼模型 → 严格 JSON（`raw_memory`/`rollout_summary`/`rollout_slug`）→ 三态结果入库（有内容/空结果/失败＋重试）。
4. **同步**：选中记录同步为 `memories/` 下 Markdown（会话摘要文件、初步经验合集），稳定排序、增删可观察。
5. **合并（第二阶段）**：全局锁＋冷却 → 选来源 → 文件差异 → 合并代理更新 `MEMORY.md`/`memory_summary.md` → 写后校验 → 基线重置。
6. **注入**：新用户轮请求携带总摘要＋查找指引（上游 developer 片段在本项目按 §5-D4 适配，不宣称角色语义等价）；缺失/空白跳过。
7. **检索**：模型经通用工具（本项目 `exec_command`，如 `rg`）搜索记忆文件——与 Codex V1 默认 `dedicated_tools=false` 一致，**检索面零新工具**（合并轮受限工具 `mem_read/mem_write/mem_list` 属 D3 已确认事项——2026-10-18 用户批准，非检索面）。
8. **反馈与便条**：解析回复中的记忆引用更新使用计数；用户明确要求的记忆修改写入便条文件，由合并阶段应用。

## 2. 现有能力盘点（复用优先）

| 需要的能力 | 本项目现有实现 | 复用方式 |
|---|---|---|
| 会话源数据（提炼输入） | `session.rs`：`HistoryItem`（user/assistant 文本、tool_calls、tool_result）serde tagged JSONL；`auto-<ts>-<pid>.jsonl`；`list_sessions`；流式装载 | 直接作为提炼输入域；`source_updated_at` 取文件 mtime 或记录内时间戳（`TCHAT_SESSION_TS` 注入先例保证测试确定性） |
| 「去掉注入说明」（§8.2-3） | 系统提示词**不入会话记录**：`prompt.rs::build_turn_request` 每请求临时注入 `system` 消息，session 只存 `HistoryItem` | 结构性免剥离（已核实）；提炼请求自行组装、不调 `sent_instructions` 即可 |
| 模型调用 | `api.rs::chat_with_body`（任意 body）＋`chat`（单输入）；非流式、响应上限、凭证脱敏、origin 授权 | 提炼/合并模型调用复用 `chat_with_body`（自定义 system 提示词＋投影历史）；凭证与授权边界自动继承 |
| 多消息请求先例 | `prompt.rs::build_request/build_turn_request`（`system → 历史 → user`，`tools` 恰两个） | 记忆相关请求（提炼/合并/注入）在同层扩展 |
| 任务锁/原子写/权限 | `session.rs`：flock 独占锁（打开即锁）、tmp＋rename 原子替换、0700/0600、路径逃逸拒绝 | 记忆任务所有权（jobs）与状态文件写入门禁直接套用同一套模式 |
| 秘密脱敏 | `api.rs::redact_credential`（错误消息中的凭证值脱敏） | 部分复用；提炼**输入侧** scrub 需新增规则（§5-D7） |
| 通用文件检索 | `exec.rs::exec_command`（003：`/bin/sh -c`、进程组终止、输出有界） | 即 V1 默认检索路径（参考 §11.1 示例即 `rg`）；注入指引告知记忆目录 |
| 文件差异 | 外部 `git` 进程基线（G8 收敛：`init/add -A/commit`＋`diff HEAD`，经 003 同源子进程机制）；`diffy` 为替代实现组件 | 第二阶段「相对上次成功基线的差异」计算 |
| 后台线程先例 | `tui_host.rs` sender 辅助线程（010 取消轮）＋ turn blocking 线程＋channel（J11 同步桥） | 触发后台任务不阻塞交互 |
| 测试底座 | mock HTTP（`tests/llm_flow.rs` 模式）、PTY e2e、`TCHAT_*` 环境注入、临时目录隔离（AGENTS.md §3） | 提炼/合并模型输出全部 mock；时钟/时间戳注入 |

## 2.1 上下文组装调查证据（2026-10-18 补充轮，源码＋既有测试核实）

**调用链（两入口汇合，无其他生产路径）**：

- TUI 路径：`tui_host.rs:229` `run_tool_turn(&mut history.lock(), &input, sender, …)`——sender 把 `api::chat_with_body` 放入辅助线程＋100ms 轮询取消（010 契约，不中止底层请求）；`on_record`→`autosave.on_history`（每入史点同步落会话）。
- 行式（非 TUI）路径：`main.rs:343` 同一 `run_tool_turn`，sender 直连 `chat_with_body`。
- 循环内（`turn.rs:207-335`）：user 输入**先入 history**（`on_record`）→ 每次迭代 `build_turn_request(history)` → sender → 响应校验（`choices[0].message`、批 ≤32 先于入史）→ 工具串行执行 → 每条 `ToolResult` 入史（`on_record`）→ 循环；纯文本回复入史返回。「普通聊天」＝循环首轮即无 tool_calls 的特例，**无独立无工具路径**。
- 发送层（`api.rs:282-311`）：`chat_with_body` 对 `messages` **逐字透传**（无角色改写/合并/丢弃）；仅注入 `model`、并入经 `check_request_param` 白名单校验的 `request_params`（保留字段 model/messages/tools 硬拒）；序列化经 10 MiB 计量写（`MAX_REQUEST_BODY_BYTES`，超限零发送，`api.rs:169-209`）。

**各部分来源与角色**（`prompt.rs` 全读核实）：

| 部分 | 来源 | 添加层 | 角色/顺序 |
|---|---|---|---|
| 系统提示 | `RAW_BASE_INSTRUCTIONS`（内置资产 `include_str!`）→ `sent_instructions()` 三步确定性适配（剔除 update_plan 节、apply_patch 示例改写、AGENTS.md 声明修正） | `build_turn_request`（prompt.rs:284-289） | `system`，**每请求恰一条、恒在首位**，内容纯静态 |
| 项目规则（AGENTS.md 等） | **不注入**——适配③明确改写母本声明为「需要时用 exec_command 自行读取」（prompt.rs:206-213） | — | 无 |
| 时间戳 | **请求内不存在**（时间戳仅用于会话文件名 `auto-<ts>-<pid>`） | — | 无 |
| 历史 | 内存 `Vec<HistoryItem>`（TUI 下 `Arc<Mutex>`；/resume 自会话文件装载） | `project_history`（prompt.rs:108-129） | `user`/`assistant`/`tool`，按存储顺序 |
| 当前输入 | 调用方推入 history 尾部后随历史投影（非独立追加） | `run_tool_turn`（turn.rs:221-226） | `user`；**仅本轮首个请求位于消息尾部，工具循环后续请求尾部为 assistant/tool（user 在历史中段）**（2026-10-18 上游对齐轮修正） |
| 工具结果 | `execute_tool` 有界输出（003/004 契约） | 入史后投影 | `role:"tool"`＋`tool_call_id` 与 assistant `tool_calls[].id` 配对；**`display` 展示字段明确不进请求**（prompt.rs:68-69） |

- **上轮「只有 role:"user"」的澄清**：该观察出自 `api.rs::chat()`（单条便捷函数）——经查**仅测试使用**（`tests/llm_flow.rs`），生产链路全部经 `build_turn_request` 组装，最终请求含 system/user/assistant/tool 四角色。
- **请求结构已被测试钉住**：`prompt_assembly.rs::s3`（空历史＝恰 system＋user 两条、恰两工具、顶层恰 `{messages,tools}`、真实 JSON 往返）、`s4`（历史逐字投影不去重）、`s6`（工具白名单）；本轮实跑 6/6 绿。**恢复等价已被钉住**：`session_cli.rs:321` 断言 `/resume` 后请求＝`build_request(loaded.history, 新输入)`；恢复用例 e1/e4/e16 实跑绿。合成示例（s3 实测结构）：`{"messages":[{"role":"system","content":sent_instructions()},{"role":"user","content":"你好"}],"tools":[exec_command_def, apply_patch_def]}`＋发送层附加 `model`（及白名单参数）。
- **大小控制现状**：10 MiB 请求预检（超限整请求拒绝）＋响应 10 MiB 上限＋每轮模型请求 ≤8＋单批 tool_calls ≤32＋工具输出各自有界。**10 MiB 是 HTTP 请求体序列化上限，不是模型上下文窗口**（2026-10-18 修正标注）；**无历史裁剪/压缩/摘要**——上下文单调增长直至 10 MiB 硬拒（既有边界，非记忆系统引入）。
- **重试**：不存在（002 登记「重试机制尚未实现」；sender Err 直接结束本轮、历史保留已发生结果——turn.rs 语义核实）。
- **恢复后的工具配对**：`session.rs` 流式装载做跳行＋计数＋配对修复（007），装载后 history 与实时同构，投影即正确配对。
- **宿主注入差异（2026-10-18 上游对齐轮登记）**：Codex 宿主把项目规则（AGENTS.md 等）作为 developer 消息注入会话上下文（母本提示词原文声称），故其提炼输入需「去掉注入说明」；本项目**不注入任何宿主内容**（005 适配③：模型自行 exec_command 读取）→ 会话文件天然只含用户/助手/工具记录，提炼输入免剥离的结构性结论不变，且无需为记忆接入做任何上下文改造（不扩大范围）。

## 2.2 上游版本与精简复用清单（2026-10-18 上游对齐轮，网页核对）

**版本事实（2026-10-18 定点修正轮，按完整 SHA 经 GitHub API 重新核对）**：

- **固定实施依据提交：`ce0323f2f2d3b25e82806cd555237da3db4e1897`**——author 日期 2026-10-17T03:54:31Z，message "Support typed content in contextual prompt fragments (#54776)"；手册全部 [Sxx] 引用与 §13 参数以此为准。
- **辅助核对提交：`8c076942cb70fba8e5c4732c23f1faabc4aed484`**——author 日期 2026-10-18T01:52:25Z，message "Allow agent messages to wake idle root sessions behind a feature flag (#54961)"，更改 9 个文件、无记忆模块路径。
- compare（ce0323f…8c07694）：**ahead 5 / behind 0**；5 个提交依次为 WebSocket zstd 压缩、tool declaration 共享、模型目录缓存保留、压缩特性门、空闲会话唤醒特性门——**files 列表对 memories/state/src/ext/memories/git-utils 零命中**（即两提交下记忆模块完全一致）。
- **勘误（撤销上一轮两处错误结论）**：①上轮把 ce0323f 的日期/message 误记到 8c07694 名下（当时一次 fetch 超时后重试返回了相同内容，未识别为数据错误）；②「`schema.rs`、`tools/`、`phase1_output.rs`、`phase2_sandbox_tests.rs`、V2 上下文切片为这 5 个提交新增」不成立——5 提交未触碰记忆模块，这些内容**在固定提交 ce0323f 已存在**；它们是否属 V1 按固定提交的实际版本分支判断：`tools/`＋`schema.rs`＝专用记忆工具注册面（V1 排除）；`phase1_output.rs`＝V1 输出解析合同（纳入复用）；V2 模板与上下文切片＝V2 分支（排除）；`phase2_sandbox_tests.rs`＝合并隔离测试（机制参照）。
- 许可：openai/codex 为 Apache-2.0——移植文件逐文件保留来源与 commit 登记（沿 008 vendor 惯例）。§2.2 清单内引用的常量（6 小时、3、8 MiB、2,500 tokens 等）已与手册 §13（ce0323f）逐项核对一致，无跨版本混用。

**复用清单（上游 @ce0323f（固定实施依据；两提交记忆模块一致，§版本事实）→ 本项目）**：

| 上游资产 | 内容（已核对） | 接入位置 | 复用方式 | 必要适配 | 保留的行为 |
|---|---|---|---|---|---|
| `state/memory_migrations/0001_memories.sql`、`0002_consolidation_progress.sql` | stage1_outputs/jobs/consolidation_progress DDL（sqlx migrate 嵌入） | M1 | **SQL 逐字拷贝** | 换执行器（见 D1） | 表/列/索引/约束全保 |
| `state/src/runtime/memories.rs`（MemoryStore，17 个 pub fn） | 领取/所有权令牌条件提交/租期/心跳/重试/水位/使用计数/保留清理/污染标记——全部语义在 SQL 文本内（ON CONFLICT 认领、`WHERE ownership_token=?`、lease/retry/cooldown、`DEFAULT_RETRY_REMAINING=3`、`PHASE2_SUCCESS_COOLDOWN_SECONDS=6h`） | M1/M2/M5/M8 | **SQL 语句逐条移植＋函数签名保留** | 去 async；sqlx→rusqlite；ThreadId/Uuid→本地类型 | **事务边界（`BEGIN IMMEDIATE` 开启/显式提交或 drop 回滚）、行数判断（受影响行数≠预期即视为失败，不盲信执行成功）、失败回滚、所有权校验（每个 mark_* 以 token 为 WHERE 条件）全部保留** |
| `memories/write/templates/memories/{stage_one_system,stage_one_input,consolidation}.md` | V1 提炼 system/user 模板（含严格 JSON 输出合同）＋合并代理提示词 | M3/M5 | **资产直接拷贝**（含来源登记） | 模板变量渲染器极简化（占位替换） | 提示词原文＝行为本体 |
| `memories/write/src/prompts.rs`＋`phase1_output.rs` | 模板加载渲染；V1 输出解析合同 | M3 | 小范围移植 | 渲染机制按本地最小实现 | 输出合同解析与三态判定 |
| `memories/write/src/{storage.rs,workspace.rs,phase1.rs,phase2.rs,start.rs,guard.rs,rollout_input.rs}` | Markdown 同步/基线差异与写后校验（`prepare/diff/write/reset/validate`、symlink 拒绝、V1 校验＝MEMORY.md 存在＋摘要首行 `v1`）/两阶段协调/开关与配额/输入组装（8 MiB、1 MiB/条、70%/150k 回退） | M3/M4/M5 | 函数级移植 | `tokio::fs`→`std::fs`；模型调用接 `api.rs`；Git 基线见 G8 | 同步/校验/管线语义 |
| `memories/write/src/*_tests.rs`（startup/guard/rollout_input/storage/workspace/prompts）＋`state/src/runtime/memories.rs` 内联测试＋`state/src/migrations_tests.rs` | 上游测试 | 对应阶段 | **同批移植为本地测试**（语义锚） | mock 边界换本地 mock LLM/时钟 | 断言语义 |
| `git-utils/src/baseline.rs`（ensure/reset/diff 三原语；上游＝gix＋similar＋一处外部 `git read-tree`） | 基线机制 | M5 | **保留 git 基线机制、载体改外部 `git` 进程**（G8 收敛：`init`＋`add -A`＋`commit`＋`diff HEAD`，经本地子进程机制运行，无新 crate；上游测试本就用 git CLI） | 三原语调用序与差异文件格式（状态列表＋diff 块＋4 MiB 截断）逐字保留；gix 内部实现替换为等价 git 命令；**本地排除集差异（2026-10-18 定点修正轮）**：上游全量遍历仅跳 `.git`（sqlite 在记忆根外天然不含），本地须三原语统一排除 `memories.sqlite`＋`memories.sqlite-journal`（M1 落地在根内、journal DELETE），暂存恒 `add -A -f`＋排除集、reset 用 `commit --amend` 保单 commit——准确调用序与环境隔离契约见 §3 G8 行 |
| `secrets/src/sanitizer.rs` | `redact_secrets`：4 条正则（sk-/AKIA/Bearer/键值赋值）→`[REDACTED_SECRET]`，ASCII 故意排除 Unicode 变体（测试钉住） | M3（输入＋生成结果两处调用） | **直接移植**（单文件） | `regex`→`regex-lite`（四模式语法兼容性作为移植测试的一部分验证；不兼容则加 `regex`） | 规则与测试原样 |
| `utils/string/src/truncate.rs` | `APPROX_BYTES_PER_TOKEN=4`；`ceil(bytes/4)`、`tokens*4`，无外部依赖 | M3/M6 | 直接移植（数行） | 无 | 估算公式原样 |
| `utils/output-truncation` | `TruncationPolicy::{Bytes,Tokens}`＋`truncate_text` 等 | M3/M6 | 小范围移植 | 去 MCP/audio 分支；`codex_protocol` 类型→本地枚举 | Tokens 语义与 1.2 序列化余量 |
| `memories/read/src/{citations.rs,usage.rs}`（＋各自测试） | `oai-mem-citation` 解析＋使用分类 | M8 | 直接移植 | 类型映射 | 引用解析语义（A27/A28 锚） |
| `ext/memories/src/prompts.rs`＋`templates/memories/read_path.md` | 读路径指引：读 `memory_summary.md`→trim→`TruncationPolicy::Tokens` 截断→空则 None→模板渲染（`base_path`＋`memory_summary`） | M6 | 函数级移植＋模板拷贝 | developer→system 协议适配（见 D4） | 读取-截断-跳过语义与指引原文 |
| `ext/memories/src/extension.rs` | `ContextContributor`→`PromptFragment::developer_policy` 贡献；工具注册（`enabled && dedicated_tools`） | M6（机制参照） | 不移植框架 | 贡献点改在 `build_turn_request` 组装层 | 「程序工作说明类消息」承载语义 |

**评估结论（直接引入模块 vs 小范围移植）**：不整 crate 引入（state/ext/memories 耦合 codex_core/extension_api/tokio 闭包，拖入面远超 V1 所需）；按上表以「SQL/模板/单文件工具函数」为单元直接复用，管线文件函数级移植——这是「直接复用→小范围适配→才自写」优先级下的最短路径。

**完整适配清单（2026-10-18 定点修正轮，取代此前「仅四处」的不完整表述）**：

1. 执行器与并发模型：sqlx→rusqlite（含 `BEGIN IMMEDIATE`/busy 重试对应）；全部 `async fn`→同步（`tokio::fs`→`std::fs`、`spawn_blocking`→直接调用）。
2. 类型映射：ThreadId/Uuid/anyhow→本地类型与错误；`MemoryVersion` 等协议枚举本地化。
3. 脱敏载体：`regex`→`regex-lite`（兼容性入移植测试；不兼容则改用 `regex`）。
4. 注入角色：developer 片段→system 尾段协议适配（D4）。
5. Git 基线载体：gix＋similar→外部 `git` 进程（G8 收敛；调用序与输出格式保留）。
6. **新增合并专用文件工具** `mem_read/mem_write/mem_list`（D3 已确认，2026-10-18，见下「三类工具区分」）。
7. **会话格式映射**：本项目 `HistoryItem`（text/tool_calls/tool_result）→上游提炼输入格式（`rollout_input.rs` 的记录组装语义），含「宿主注入不落会话」差异（免剥离）。
8. 模型调用对接：上游供应商接口→`api.rs::chat_with_body`（非流式、10 MiB 上限、凭证脱敏继承）。
9. 触发位点：`turn_processor`→本项目两入口 `run_tool_turn` 轮启动位（D2）。
10. 模板假设改写（仅确实不成立处，逐条登记）：见下「模板假设核对」。

**三类工具区分（D3 澄清）**：①**主聊天工具**＝`exec_command`/`apply_patch`（用户会话，003/004 契约，不因记忆系统改变）；②**合并专用文件工具**＝`mem_read/mem_write/mem_list`（**仅内部合并轮注册**，用户会话与检索均不可见，工具注册表由记忆运行时注入）；③**专用记忆检索工具**＝上游 `memories.list/read/search/add_ad_hoc_note`（V1 排除，不实现）。

**合并轮权限边界与验收（取代此前「无 shell 即无逃逸面」的笼统表述；D3 用户已确认，2026-10-18）**：合并轮工具注册表**恰为** `mem_read/mem_write/mem_list` 三项（断言不含 exec_command/apply_patch/其他）；`mem_*` 边界＝路径规范化后必须位于记忆根内（拒绝绝对路径与 `..` 跳转）、组件级拒绝符号链接、写入仅限常规文件、**读写均拒绝隐藏路径**（路径任一组件以 `.` 开头即拒——`.git/`、`.gitignore` 对模型不可见不可写，同时构成模型文件写入与 Git 基线元数据的保护边界；有界输出参数沿上游 `local.rs` 语义：读取行号 1 起/`max_lines`≥1/默认 20,000 tokens 预算、列目录默认上限 2,000 条，2026-10-18 M5 决策收敛轮补）；**`memories.sqlite`/`memories.sqlite-journal` 与隐藏路径一并不向模型暴露——不允许读取、写入或列表展示（2026-10-18 批准轮补：mem_list 输出同边界过滤，两库文件与隐藏条目不得出现在列示结果中）**；合并轮会话为临时内存态（不落 `sessions/`、不注入记忆、不写用户会话文件）。验收＝M5 越界用例逐条（绝对路径/父目录/符号链接/目录写入/隐藏路径与 `.git` 前缀拒绝）＋工具注册表断言＋mem_list 输出不含隐藏路径与两库文件的断言＋合并产物全部落在记忆根内的文件断言。

**模板假设核对（2026-10-18 定点修正轮，逐条对照 ce0323f 模板原文）**：

- `stage_one_system.md`（提炼）：无工具/命令/git 声明（示例中的命令仅是记忆内容样例）；JSON 合同（`rollout_summary`/`rollout_slug`/`raw_memory`、无额外键、无 JSON 外文本、空串 no-op）与 §8.5 一致；secrets 指示（`[REDACTED_SECRET]`）与 sanitizer 占位符一致——**原样拷贝，零改写**。
- `consolidation.md`（合并）：明文「`{{ memory_root }}/` is a git repository managed by Codex」——**与 G8 推荐一致，成立不改**；`rg --files`/`wc -l` 均带 "or equivalent"（`mem_list`/`mem_read` 可充当）——不改；skills 段鼓励 Python/shell helper 脚本——**与合并轮无执行能力冲突**，处置＝模板原文不改，在本项目合并 system 包装层追加一句注记「本环境无脚本执行能力；skills 可含说明/模板文件，不含可执行脚本」（登记为包装层适配）；输出结构要求（`v1` 首行、Task Group、rollout 注记三元组）纯文本输出，无工具依赖。
- `read_path.md`（读取指引）：占位符 `{{ base_path }}`/`{{ memory_summary }}` 与 ext/prompts.rs 一致；策略性指引（先摘要关键词→检索 MEMORY.md→按需开 1–2 个文件→4–6 步预算）与 `exec_command`＋`rg` 兼容——不改；引用格式 `<oai-mem-citation>` 原文格式照录 M8；**一处确证错误**：模板把 `rollout_summaries/` 描述为 jsonl（含 `session_meta.payload.id` 等）——手册 §21.2 已判定为上游已知错述（实际是 `.md`，`rollout_path` 才指向原始会话），**属「确实不成立」→ 最小改写该句并登记**（其余原文保留）。

**G8 替代实现（自写快照＋diffy）差异登记（非推荐，仅当用户否决 git 依赖时）**：①记忆根非 git 仓库——consolidation.md 的 git 句失真，须改写模板；②diff 由 `diffy` 生成，与 `git diff` 输出不保证逐字节一致（header/上下文行）；③无 git 历史（人工检视/恢复能力少于上游）；④与上游 baseline 测试不同路（上游测试直接用 git CLI）。

## 3. 缺失能力与与参考文档的差异

缺失（V1 全部记忆专有逻辑）：

- 状态存储：单会话结果（§6.2 `stage1_outputs` 字段族）、任务状态（§6.3 `jobs`：所有权令牌/有效期/重试/watermark）、合并进度、使用计数（§18.3 最少状态清单）。
- 候选选择（§7.3 全部规则）、两阶段协调（§9.2–9.4）、提炼提示词与严格 JSON 合同（§8.5）、三态语义（§8.6）、Markdown 同步（§9.4 步骤 3–4）、写后校验（§9.7）、注入（§10）、引用解析（§12.3）、保留清理（§12.4）、便条规则（§12.2）、重置（§12.7）、配置开关（§13.1）。

与 Codex 的结构性差异（移植时必须显式决策，不得默默照搬）：

| # | Codex | 本项目现状 | 处置 |
|---|---|---|---|
| G1 | SQLite（`memories_1.sqlite`＋`state_5.sqlite`，sqlx） | 无任何数据库依赖（纯文件式） | 复用上游 SQL（DDL＋DML 逐字移植），执行器适配 `rusqlite`（见 D1 修订） |
| G2 | 触发点 `app-server turn_processor`（进程内异步） | 单进程 CLI（REPL/TUI 驱动层） | §5-D2 |
| G3 | 合并代理＝隔离配置的完整 Agent（临时会话、禁 MCP、沙箱只写记忆根目录） | 唯一 agent 循环＝`run_tool_turn`（exec＋apply_patch 两工具，无沙箱层） | §5-D3 受限模式 |
| G4 | 配额检查（后端限额接口，无法判断时放行） | 无配额接口 | 按「无法判断→放行」语义跳过并登记（§5-D8） |
| G5 | developer 级注入消息 | 协议已用 `system`（每请求恰一条，`build_turn_request`） | §5-D4 |
| G6 | 会话含 `memory_mode`/来源类型/归档状态（state 库登记） | 会话文件无此元数据 | 随 D1 落 SQLite：本地 `threads` 登记表（memory_mode 等），默认 enabled |
| G7 | 模型窗口信息（70% 预算比例） | `ModelInfo.context_length` 有映射（`list_models` 取响应 `context_window`，api.rs:359-361），但供应商未必返回、手动目录无此字段 | 有值用真实窗口×70%；无值回退 150,000 tokens（上游值，字节按 D11 公式换算；登记差异） |
| G8 | Git 基线（`memories/.git`，上游＝gix＋similar＋一处外部 `git read-tree`；合并模板明文「memory_root is a git repository managed by Codex」） | 引入 gix 依赖树大；本目录尚非 git 仓库 | **保留上游 Git 基线机制，载体＝外部 `git` 进程**（见 G8 收敛）：`git init`＋`add -A`＋`commit`＝基线建立/重置、`git diff HEAD`＝差异；经本地既有子进程机制运行（003 同源），无新 crate；**内部调用确定性与调用序契约（2026-10-18 M5 决策收敛轮补、同日定点修正轮修订；G8 用户已确认，2026-10-18，批准保护边界见 §6 M5「G8 批准边界与验收」；探测证据见当轮记录）**：①**环境隔离**——`Command::new("git")` 不经 shell 直启；`GIT_CONFIG_COUNT=0` 显式覆盖（挡 `GIT_CONFIG_KEY_n/GIT_CONFIG_VALUE_n` env 注入——探测实证注入 `diff.noprefix` 确会去掉 `a//b/` 前缀、注入 `core.excludesfile` 确会改变未跟踪可见集）＋`GIT_CONFIG_GLOBAL/GIT_CONFIG_SYSTEM` 指向 `/dev/null`＋清除 `GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE`（泄漏实证 fatal）＋`-c user.name/user.email` 固定值（探测实证无全局 identity 时 commit exit 128、带 `-c` 恢复 0——外部 git 进程相对 gix 的新增差异点）；②**文件集合与暂存规则（三原语统一）**——排除集恰为 `:(exclude)memories.sqlite`＋`:(exclude)memories.sqlite-journal`（**本地布局差异**：上游 `memories_1.sqlite` 位于 CODEX_HOME 根、在记忆根外，基线全量遍历（仅跳 `.git`）天然不含库文件；本地 M1 落地单库在记忆根内且 `journal_mode=DELETE`，不排除则任务记账写入使 A17 无差异捷径失效、冷却期满即空转合并），暂存恒为 `git add -A -f -- . <排除集>`（`-f`＝被忽略文件强制入基线——探测实证 `.gitignore=*.md` 存在时无 `-f` 新摘要不可见；mem_write 禁隐藏路径使 `.gitignore` 正常不可达，双保险）；差异计算前先跑**同一条** add（`git diff HEAD` 不含 untracked，缺此步新摘要不可见）；`phase2_workspace_diff.md` 在 prepare/diff 计算/reset 三处前置删除（NotFound 忽略），从不入基线；③**调用序**——ensure＝`git rev-parse --verify HEAD` 判可用（探测：unborn/损坏 exit≠0、健康 0），不可用→删除记忆根自有 `.git` 后 `git init` 全新初建（2026-10-18 实施轮修正登记：reinit 不重写损坏 HEAD 文件、修复不成立，故取上游「不可用元数据以全新单提交基线替换」同义机制；普通 commit 初建——unborn 上 `--amend` 必败；**仓库锚定**：每条命令显式 `--git-dir=<root>/.git --work-tree=<root>`，git 永不向上发现父仓库）；reset＝删临时差异文件→`git rm --cached -q --ignore-unmatch -- memories.sqlite memories.sqlite-journal`（实施轮落实「曾被跟踪的排除文件」机制：仅两固定名、只动索引不删真实文件）→add→`git commit --amend --allow-empty`（单 commit 基线恒成立＝上游「破坏性替换 `.git` 为单 commit」等价，不删 `.git` 目录）→`git read-tree --reset HEAD` 对齐 index（上游 `write_index_from_head` 同款，初建后同跑）；**提交时间确定性**（实施轮补登记）：初建固定纪元、amend 取 HEAD 提交时间＋1s——真实时钟下同秒同树 amend 会产生相同 SHA，确定性派生使 reset 恒产生新提交且与真实时钟解耦；④**编排位（上游 phase2.rs 源码核实）**——领取全局锁**在前**、prepare/ensure 在后（「领取锁后才触碰工作区」）；无差异短路＝写成功＋新水位＋selected、**不 reset 基线**；reset 在 SQL 成功提交**之前**——ensure/diff/reset 失败均属领取后失败，按 `job::failed`＋retry 记账，reset 失败不得写 SQL 成功；进程组＋超时＋输出上限沿 003 模式要点；上游 baseline 测试本就直接使用 git CLI——「不好测」不成立（已撤销该理由）。次选＝引入 gix＋similar 源码级复用（依赖树大）。自写快照降为**替代实现**（差异见 §2.2 G8 注） |

## 4. Codex 默认参数（原值列出，供项目选择）

参考 §13.1（可配置）与 §13.4（常量）。**下表为 Codex 固定提交的值，不是本项目已采用值**；「建议」列仅为计划推荐，逐项待批准（§5）。

| 参数/常量 | Codex 值 | 含义 | 本项目建议 |
|---|---:|---|---|
| `features.memories` | `false` | 总功能开关 | 同（opt-in；关闭＝不触发、不注入） |
| `memories.version` | `"v1"` | 版本 | 固定 v1（V2 不纳入） |
| `memories.generate_memories` | `true` | 新会话可作生成输入 | 同 |
| `memories.use_memories` | `true` | 注入读取上下文 | 同 |
| `memories.max_rollouts_per_startup` | `2`（限 1–128） | 每轮领取上限 | 同（首版 2 足够） |
| `memories.max_rollout_age_days` | `10`（限 0–90） | 源会话年龄窗口 | 同 |
| `memories.min_rollout_idle_hours` | `6`（限 1–48） | 源会话闲置门槛 | 同 |
| `memories.max_raw_memories_for_consolidation` | `256`（限 1–4096） | 合并最大来源数 | 同 |
| `memories.max_unused_days` | `30`（限 0–365） | 保留窗口 | 同 |
| `memories.min_rate_limit_remaining_percent` | `25`（限 0–100） | 剩余配额门槛 | 无配额接口→跳过（D8） |
| `extract_model`/`consolidation_model` | 未指定（供应商默认） | 提炼/合并模型 | 默认＝当前会话所选模型（D6） |
| 第一阶段并发上限 | 8 | 代码并发 | 简化为顺序处理领取的 2 个（D10） |
| 候选扫描上限 | 5,000 | 数据库扫描 | 同值常量（非配置）；**落地口径（M2 实测）：LIMIT 约束 threads 扫描行，目录枚举仅作存在性交集** |
| 任务有效期（两阶段） | 3,600s | 所有权租期 | 同 |
| 失败重试延迟（两阶段） | 3,600s | `retry_at` | 同 |
| 默认重试计数 | 3（两阶段领取判断不同） | 重试 | 同语义（阶段一判耗尽；阶段二判等待/冷却） |
| 第二阶段成功冷却 | 6 小时 | 全局合并冷却 | 同（与闲置 6h 分开计时，§13.5） |
| 第二阶段心跳 | 90s | 运行期续期 | 同（简化：完成前周期续期） |
| 提炼推理强度 Low／合并 Medium | — | 模型参数 | 无推理强度配置面→不传（登记差异） |
| 近期输入读取量 | 8 MiB／单记录 1 MiB | 输入边界 | 同（字节级，免 token 换算） |
| 会话内容预算 | 有效窗口 70%／回退 150,000 tokens | 输入预算 | 沿用上游值与比例；无窗口回退 150,000 tokens（字节换算按 D11 公式） |
| 注入摘要预算 | 2,500 tokens | 注入上限 | 沿用上游常量（D11 公式换算，不自定字符数） |
| 旧初步结果清理批量 | 200 | 保留清理 | 同 |
| 扩展资源保留 | 14 天 | `resources/` 清理 | V1 仅 `ad_hoc` 扩展→实现为「无 resources 目录时 no-op」＋规则测试（A32） |
| 差异文件上限 | 4 MiB | `phase2_workspace_diff.md` | 同 |

## 5. 待决问题（实施前需批准；附推荐）

- **D1 状态存储选型**（2026-10-18 上游对齐轮修订；**用户已确认，2026-10-18**）：推荐 **`rusqlite`（bundled 特性）＋上游 SQL 逐字移植**——`state/memory_migrations/*.sql` DDL 拷贝、`MemoryStore` 17 个函数的 SQL 语句逐条移植（函数签名保留、去 async）。理由（按直接复用优先）：上游任务生命周期的全部语义（认领、`WHERE ownership_token=?` 条件提交、租期抢占、重试/冷却、水位、使用计数、保留清理）**写在 SQL 文本里**，这是最值得复用的资产；JSON 方案＝把这些 SQL 语义全部重写一遍，属「无法复用时才自写」，必要性不成立。三点质询的答复：**跨状态提交**——上游本就声明「文件写入与数据库状态不是一个跨存储原子事务」（手册 §9.8），同库内 stage1_outputs＋jobs 经 `BEGIN IMMEDIATE` 事务提交（rusqlite 等价 `begin_immediate`）；**崩溃恢复**——`lease_until` 过期可抢占＋`retry_at`（SQL 内置），无需自建；**旧所有者拒提交**——`WHERE ownership_token=?` 逐字保留（A10 语义）。执行器不用 sqlx 的原因：app 全同步（ureq/std 线程），sqlx 需 tokio 渗入全部调用链，适配面远超收益；rusqlite 保 SQL 逐字级复用。「零新增依赖」不作为重写理由（本轮指令明示）；bundled 免系统依赖，构建面扩大登记。**备选**（JSON 状态文件）保留但降级，仅当用户否决 SQLite 依赖时采用，且须接受 SQL 语义重写成本。G6 的 `threads` 登记表随本决策同库落建。
- **D2 触发点**（修订，对齐手册 §7.1；**用户已确认，2026-10-19，批准边界见 §6 M7**）：**每个「带输入的新用户轮成功启动后」触发一次后台检查**——本项目语义＝`run_tool_turn` 以非空输入进入（输入入史后）即 spawn；进程内以 jobs 互斥去重（同一时刻至多一条记忆管线运行，与上游全局任务锁一致）。与追加输入/失败/取消的关系：本项目**无「向运行中轮次追加输入」形态**（010 的排队输入在轮结束后按新轮重放＝新一轮启动，照常触发；登记为形态差异）；轮失败或取消**不回收**已触发的检查（候选只读其他会话，与当前轮成败无关）；进程退出后任务不保证完成（与上游一致）。备选「仅启动时运行」维持不推荐（错过长会话期间闲置积累）。
- **D3 合并执行模式**（2026-10-18 上游对齐轮修订；**用户已确认，2026-10-18**）：**任意 shell 不得以路径白名单包装宣称受限**——原「受限 run_tool_turn＋路径校验」方案撤销。修订推荐：合并阶段使用**专用受限工具集的真隔离**——为合并轮提供 `mem_read`/`mem_write`/`mem_list` 三个工具（仅记忆根目录内，路径规范化＋拒绝越界/符号链接，边界实现参照上游 `ext/memories/src/local.rs`），**不提供 exec_command**（合并轮权限边界与验收见 §2.2 专节）；合并代理其余配置对齐上游 §9.6 可达项：临时内存会话（不落 `sessions/`，不会成为提炼候选）、不注入记忆、不写会话文件。上游的 OS 级沙箱（landlock/seatbelt，`phase2_sandbox_tests.rs`）不可移植——本项目无沙箱层，以具体权限边界实现（见 §2.2「合并轮权限边界与验收」），**这是工具能力变化（新增受限工具）而非权限系统新建**；turn.rs 工具集需参数化（现硬绑 exec/apply_patch 两工具，注入点已存在于 `build_turn_request`）。原「待用户决定」两项已随 2026-10-18 批准落定：①新增受限工具获接受（注册表恰为 `mem_read`/`mem_write`/`mem_list` 三项）；②合并轮不提供 exec_command 亦不注册其他搜索工具——按推荐不允许 `rg`（合并输入已齐：差异＋手册＋摘要）。批准边界全文与相应验收见 §6 M5「D3 批准边界与验收」。
- **D4 注入角色**（2026-10-18 上游对齐轮再修订；**用户已确认，2026-10-19**）：上游证据＝`PromptFragment::developer_policy`（developer 角色片段，ext/memories/src/extension.rs）。本项目当前在用 user/system/assistant/tool。**已确认方案**：基础指令与记忆上下文保持逻辑分离，记忆块使用固定 `<memory-context>…</memory-context>` 边界追加到现有唯一 `system` 消息尾段，并明确属于历史参考；不新增第二条 system 或 developer 消息，消息条数/顺序及两项工具不变。未注入时请求原样，既定预算、轮内复用、下一轮重读、自动注入不入历史的契约不变。**developer→system 登记为角色适配，不宣称语义等价**；旧结构断言不构成语义等价证据。供应商实测仍归 M7，原第二条 system／原生 developer 备选未随本次批准采用。批准边界全文见 §6 M6；其他决策不随本轮批准。
- **D5 目录与配置载体**（**用户已确认，2026-10-18**）：记忆根＝`<user_dir>/memories/`（`TCHAT_USER_DIR` 体系，0700）；配置＝`config/manual.json` 新增 `memories` 段（经 `catalog.rs` 既有装载/原子写/权限路径）。新增任何 `TCHAT_MEMOR*` 环境变量必须并入 F1 信任根跳过名单（沿 007 先例）。
- **D6 模型选择**（**用户已确认，2026-10-18**）：提炼/合并默认用当前 `selected_model`，不新增 `extract_model` 配置（M3 已按此实施：模型经调用方闭包携带，全 mock 验证）。
- **D7 秘密 scrub**（修订，直接复用优先）：**移植上游 `secrets/src/sanitizer.rs::redact_secrets`**（4 条正则：`sk-`/AKIA/Bearer/键值赋值→`[REDACTED_SECRET]`，ASCII、Unicode 变体故意排除且测试钉住），作用于提炼**输入与生成结果**两处（上游 §8.2-4/8.5 同位）；叠加既有 `redact_credential`（API key 值本身，错误消息面）。适配点唯一：`regex`→`regex-lite`（本地已用）的语法兼容性须以移植测试证明（四模式含内联 `(?i)`、`\b`、量词），不兼容则直接采用 `regex` crate（登记新依赖）。不再自定规则。
- **D8 配额检查**：无后端配额接口→按参考 §7.6「无法判断时放行」语义**跳过该项**并登记；后续可读响应头剩余额度再启用。（未修订，维持。）
- **D9 命令面**（**用户已确认，2026-10-19，批准边界见 §6 M7**；**重置失效边界同日追加确认，见下**）：`/memory-status`（查看状态：开关、记忆根、摘要存在性、库内计数——状态观察）＋`/memory-reset confirm`（显式确认参数后清理生成记忆，保留原始会话；缺少 `confirm` 不执行清理；保留根目录，符号链接及清理边界按既定契约处理）。**重置失效边界（2026-10-19 用户明确确认）**：`/memory-reset confirm` 成功返回后，重置前启动的旧任务不得再回写生成记忆或重建基线；之后的新用户轮仍可按开关重新生成。此前定点补验轮登记的「在途任务按路径写回 Markdown＝非原子边界如实」观察属实，但**「该行为属已批准语义」的结论系误判**——按本轮明确边界更正（历史记录保留）；既有非原子**失败**边界（部分文件变化不回滚）与本轮明确的重置**完成**保证（返回后旧任务失效）为两件事，分别成立。
- **D10 并发简化**（2026-10-18 修订理由表述）：收回「并发 8 仅在多进程有意义」——单进程同样可以多线程并发等待模型请求。V1 首版**顺序处理**每轮领取的 ≤2 个候选，定位为**保守起步选择**而非上游语义不适用：上游并发上限 8 的语义保留在 SQL 层 `max_running_jobs`（移植后自动生效），代码层顺序不违反；后续如需并发，改为线程池即可（无架构变更）。
- **D11 token→字符换算**（修订，直接复用优先）：**采用上游估算公式与常量**——`utils/string/src/truncate.rs`：`APPROX_BYTES_PER_TOKEN=4`，token 数＝`ceil(bytes/4)`、字节＝`tokens*4`（无 CJK 区分、无外部依赖），连同 `TruncationPolicy::{Bytes,Tokens}` 语义移植；注入预算直接用上游常量 **2,500 tokens**（换算 10,000 字节截断点由同一公式得出，不自定字符数）；会话内容预算＝有效窗口 70%、无窗口回退 150,000 tokens（`ModelInfo.context_length` 有值时用真实窗口，G7）。不再「自行假定固定字符换算」。
- **D12 便条写入门禁落点**（2026-10-19 M8 实施前确认轮提出、同日离线边界核查轮修正；**用户已批准方案 A〔D12-A〕，2026-10-19**）：**代码依据**——便条进入合并模型的现有路径恰三条：①phase2 显式 extensions 输入（`phase2.rs::build_consolidation_prompt` 两个 extensions 占位符在本地 V1 **恒替换为空串**，今日不经此路径）；②G8 差异文本（`workspace.rs::diff_pathspec` 仅排除 `memories.sqlite*`，探针实证合法与非法名便条**原文逐字进入** `phase2_workspace_diff.md` 统一差异）；③mem_list 发现＋mem_read 读取（`mem_tools.rs` 仅排除隐藏点文件与 `memories.sqlite*`，notes 目录可列举可读取——**任何差异层过滤都拦不住此旁路**）。覆盖路径：apply_patch **Add**＝`create_new` 原子不覆盖既有文件（004 规格，唯一的不覆盖保障）；apply_patch **Update/Delete**＝可改写/删除既有便条（通用语义，无记忆特判）；exec_command＝任意写/覆盖/删除（A23 不虚称受限）。**V1 无专用工具（§1/A29–A30）＋D3/D4 冻结工具面 ⇒ 「写入时拒绝」在通用路径上不存在现成机制，全量不覆盖对全部写入路径不可保证（如实登记，上游 V1 同一现实）。**
  **（A，推荐＝满足当前需求的最小方案）差异级消费过滤**：在 `memory_workspace_diff` 产出处校验 `extensions/ad_hoc/notes/` 条目——文件名按上游 V2 正则 `^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-[a-z0-9][a-z0-9-]{0,79}\.md$`（总长 24–128 字节）、正文非全空白（差异已含内容或按路径读入）；不合格便条**不进入差异文本**（状态列表与统一差异同源过滤，本轮唯一变化仅非法便条时同样落入「无差异短路」不触发合并）。**此过滤为拟议行为，今日不存在**（探针实证现状＝原文入差异）。**保证范围**：「合并时排除」＝成立（机制＝差异单点过滤＋无差异短路）；「已有便条不被覆盖」＝仅在 apply_patch Add 路径成立（create_new 固有），apply_patch Update/Delete 与 exec 改写**不保证**；「写入时拒绝」＝不提供（非法便条文件留在盘上、仅不入合并模型）。**剩余绕过面**：合并代理可经 mem_list/mem_read 直读非法便条（靠合并指引「Use the already provided diff」的行为性信任，与上游同级——不扩展 mem 工具合同去拦截，若要拦须另批 D3 面扩展，本轮不提议）；用户侧任意文件系统操作天然不在保障内。**改动范围**＝`workspace.rs`（差异过滤）＋测试。**验收**：RED——非法名/全空白正文便条不出现在差异文本且单独存在时不触发合并（现状探针行为相反）；合法便条在差异中且合并输入含变化（A25/A26 联动）；重名 Add 被拒（既有 004 行为，仅回归引用）；mem_read 仍可直读（旁路如实断言存在而非拦截）。
  **（B，不推荐）apply_patch 定向门禁**：对 Add/Update/Delete 目标落在便条目录内的补丁写时应用同一校验。**属 004 通用工具合同变化**（apply_patch 须感知记忆根与开关，通用工具耦合记忆配置——非等价适配，须明示），且 exec 绕过依旧、仍不能全量保证不覆盖；拒绝面更早（写入时报错）但为局部拦截扩展通用工具，与本轮「不追求全面拦截」的取舍相悖，仅当用户明确要求写时反馈时采用。
  **D12-A 批准登记（2026-10-19 用户批准方案 A）**：①非法便条**不进入自动提供的差异状态列表和正文**（合并代理正常经差异所见即过滤后集合；盘上文件保留）；②mem_list/mem_read 仍可发现、读取非法便条——**提示词指引不构成技术隔离**，旁路如实保留；③**不提供通用写入时拒绝**（exec/apply_patch 合同不动）；④**不覆盖保证仅适用于 apply_patch Add**（create_new 固有；Update/Delete/exec 改写不保证）；⑤**这是 A25 验收语义的契约调整**——从「写入门禁层校验」调整为「差异层过滤」，M8 的 A25 测试按新边界实现（非法＝差异排除＋盘上保留，非写入报错）。上文「拟议行为」表述为批准前历史，保留不改；差异过滤范围不扩大到其他目录，不新增状态面计数（未在批准范围）。

## 5.1 记忆注入接入契约（依据 §2.1 证据；D4 用户已确认，2026-10-19；M6 已交付，2026-10-19）

**现有行为（已证实）**：注入位点的全部前提已满足——system 段每请求合成且不落会话（`sent_instructions` 纯函数）；`chat_with_body` 透传 messages；恢复后请求由 history 重建（session_cli:321 钉住）。

**历史缺口（2026-10-18 M6 实施前按当时源码勘误）**：M5 已落 `prompt.rs::build_turn_request_with(history, tools, system)` 与 `turn.rs::run_tool_turn_with_mode`（用户轮仍走默认 `build_turn_request`，内部合并轮自带 system/三项 mem 工具），**不再缺通用参数化入口**；M3 已落 `memory/truncate.rs` 并复用 008 `codex-utils-string`，不重复移植。M6 当时尚缺摘要读取/模板/注入块、读取开关与按轮缓存、注入契约测试；当时 `MemoriesConfig` 只有总开关与候选/合并参数，尚无 `use_memories` 字段，`memory/inject.rs` 与 `templates/read_path.md` 尚不存在。**2026-10-19 本轮已补齐并验证上述 M6 缺口**，实现与证据见 §6 M6；真实驱动接线仍归 M7。

**已确认契约（D4，2026-10-19；M6 已实现并验证，同日批准交付）**：

- **注入什么**：`memory_summary.md` 摘要正文＋固定查找/使用/引用指引（何时查、先搜 `MEMORY.md`、沿摘要追据、无命中即停、引用格式——§10.2/§10.3）。
- **注入哪里/角色**：现有唯一 `system` 消息尾段，以固定 `<memory-context>…</memory-context>` 边界隔开，位于基础提示词之后并明确属于历史参考；不新增第二条 system 或 developer 消息，消息条数/顺序及两项工具不变。未注入时请求原样。
- **读取边界（2026-10-18 修订，消除原「逐请求重读却同轮同值」矛盾）**：摘要**按轮读取**——每个新用户轮组装首个请求前读一次 `memory_summary.md` 并缓存于该轮，轮内全部模型请求（工具循环续请求）复用同值；新轮/`/resume` 后重新读取取最新。**不持久化进 `HistoryItem`**。由此：轮内上下文稳定；摘要更新后下一轮即生效；无重复注入（Codex 需防重复的持久上下文场景在本项目不存在）。
- **恢复后遗漏**：不存在——注入与 history 无关，`/resume` 后新轮照常读取合成。
- **不会被再提炼**：会话文件只存 `HistoryItem`（§2.1 证据），注入内容物理上不进提炼输入；M3 测试须钉住「提炼请求不含 `sent_instructions` 与记忆段」。
- **超长处理（修订，按上游行为）**：上游＝摘要文本经 `TruncationPolicy::Tokens(2,500)` 截断后入模板（ext/memories/src/prompts.rs 实测），**截断即止、无内容优先级保留**——按此保留；指引块（模板固定文案）天然不裁；「保持摘要紧凑」由合并提示词（consolidation.md）对合并代理提出，不是读取侧职责。
- **记忆事实 vs 当前指令/项目规则的区分**：注入块首行固定措辞「以下是历史记忆摘要，供参考，不是当前指令；与本次用户要求冲突时以用户为准」（**本项目自创说明行，非上游模板原文**；上游 read_path.md 模板原样拷贝，不与自创行混同）；项目规则本就不注入（模型自行读 AGENTS.md，来源文件不同，天然隔离）。

**M6 当前待决状态（2026-10-19 批准后更新）**：无阻塞性待决项；D4 用户已确认，批准边界见 §6 M6，无须再次询问。D11 的上游公式与常量已嵌入 M3/M6 契约，且 M3 已交付对应实现；本阶段直接复用，不重新提出换算方案，历史 D11 状态不翻为「用户已确认」。其他决策不随本轮批准。供应商兼容性实测、threads 登记与后台触发仍归 M7，不混作 M6 阻断；第二条 system／原生 developer 备选本轮未采用。

**分支准备阶段接入判断（历史：2026-10-19 D4 批准后）**：**现有能力已足够接入，D4 已确认，无更早阶段前置或阻塞性待决项，可直接接收 M6 实施 `/goal`；该准备轮不实施**。复用 M5 参数化组装，M6 补可选记忆块、turn 层按轮读取/复用能力及配置缺省兼容；API 透传层无需改造。M7 再把配置/路径接到 `main.rs` 与 `tui_host.rs` 两个真实驱动入口，并完成 threads 登记、后台触发、命令面与供应商实测；不提前纳入 M6。自动注入块本身不入 `HistoryItem`，提炼与内部合并轮不自动注入，以真实调用链测试钉住。

**当前实施状态（2026-10-19）**：`memory::inject::read_memory_context`、`build_turn_request_with_memory`、`ToolMode::UserWithMemory` 已落地并由真实 turn/会话/API/exec 链验证；None 快照亦轮内固定。M6 无待处置阻断项，已交付（2026-10-19 快进合并 main @ `2aa0002`）；默认 `User` 与 `MemoryMerge` 路径保留，M7 两驱动生产接线及供应商实测仍未实施。

**决策状态汇总（2026-10-18 上游对齐轮建立、定点修正轮及 M5 规格确认登记轮更新；2026-10-19 D4 批准登记轮更新）**：

| 决策 | 状态 | 说明 |
|---|---|---|
| D1 存储 | **用户已确认（2026-10-18）**：rusqlite(bundled)＋上游 SQL 逐字移植（事务/回滚/行数/所有权保留） | 三项质询均有 SQL 级答案 |
| D2 触发 | **用户已确认（2026-10-19）**：每带输入新轮成功启动触发后台管线；enabled 先 gate；jobs/全局锁互斥；主轮失败/取消不撤销；退出不等待；费用已知情 | 批准边界见 §6 M7 |
| D3 合并隔离 | **用户已确认（2026-10-18）**：注册表恰为 mem_read/mem_write/mem_list、不提供 exec_command、限记忆根、隐藏路径与库文件不暴露 | shell 白名单包装方案已撤销；批准边界见 §6 M5 |
| D4 注入角色 | **用户已确认（2026-10-19）**：现有唯一 system 尾段＋固定 memory-context 边界，明确历史参考；不新增消息，消息顺序及两工具不变 | 批准边界见 §6 M6；未注入原样，既定预算/轮缓存/重读/不入史不变；角色适配不宣称语义等价；供应商实测归 M7，其他决定未随本轮获批 |
| D5 目录/配置 | **用户已确认（2026-10-18）** | 无上游冲突 |
| D6 模型 | **用户已确认（2026-10-18）**：默认当前 selected_model（M3 实施轮获批） | 无上游冲突 |
| D9 命令面 | **用户已确认（2026-10-19）**：/memory-status＋/memory-reset confirm | 批准边界见 §6 M7 |
| D7 脱敏 | 已可推荐（移植 `redact_secrets`）；regex-lite 兼容性作为移植测试一部分 | 不自定规则 |
| D8 配额 | 已可推荐（跳过＋登记） | 无接口 |
| D10 并发 | 已可推荐（顺序起步，SQL 层 `max_running_jobs` 保上游语义） | 原理由已修正 |
| D11 换算 | 已可推荐（上游公式＋常量） | 无自定换算 |
| D12 便条门禁 | **用户已确认 D12-A（2026-10-19）**：差异层过滤（契约调整）——非法便条不入差异状态列表与正文；mem_list/mem_read 旁路保留、指引不构成隔离；无写入时拒绝；不覆盖仅 apply_patch Add；方案 B 未采用 | 详见 §5 D12 及批准登记；A25 验收语义随之调整 |
| G8 基线 | **用户已确认（2026-10-18）**：保留上游 git 基线机制、内部固定外部 git 进程载体，仅操作记忆根自有专用仓库 | 差异文件格式保留；模板 git 假设成立无需改写；批准边界见 §6 M5 |

## 6. 分阶段实施计划（依赖序）

**M1 最小决策包（2026-10-18 定点修正轮提取；只有以下两项真正阻塞 M1）**：

- **阻塞 M1 的决定（两项均已于 2026-10-18 获用户确认，M1 阻塞解除、保持待实施）**：① **D1 存储**＝`rusqlite`（bundled）＋上游 SQL 逐字移植（保留事务边界、回滚、行数判断、所有权校验）；② **D5 载体**＝记忆根 `<user_dir>/memories/`（0700）＋ `config/manual.json` 新增 `memories` 段（经 catalog 既有装载/原子写/权限路径；新增 `TCHAT_MEMOR*` 环境变量须并入 F1 信任根跳过名单）。
- **最少状态字段**（随 D1 上游 DDL 定，不另设计）：`stage1_outputs`（thread_id 主键、source_updated_at、raw_memory、rollout_summary、rollout_slug、generated_at、usage_count、last_usage、selected_for_phase2、selected_for_phase2_source_updated_at）；`jobs`（kind、job_key、status、worker_id、ownership_token、started_at、finished_at、lease_until、retry_at、retry_remaining、last_error、input_watermark、last_success_watermark）；`consolidation_progress`（max_thread_count）；`threads.memory_mode`（G6 登记）。
- **事务与任务所有权契约**：`BEGIN IMMEDIATE` 开事务；stage1_outputs 与 jobs 状态同事务提交；每个 `mark_*` 以 ownership_token 为 WHERE 条件并以受影响行数判断成败；失败＝事务回滚＋retry_at/retry_remaining 记账；租期 3,600s、过期可抢占。
- **RED 场景、验证范围与完成条件**＝M1 节所列（A10、租期抢占、重试记账、时钟注入、崩溃恢复、上游测试锚移植、全量门禁）。
- **不构成 M1 前置**（后续阶段再决，不得提前置入 M1）：D3 合并工具集（M5 前）；D4 注入角色与 developer 供应商实测（M6 前——推荐方案为 system 适配，不依赖实测）；D6 提炼/合并模型（M3 实施时默认＝当前会话模型）；D9 命令命名（M7 前）；D10 并发（顺序起步）；G8 已收敛（git 进程，M5 落地）。

> 通用约定：每阶段先写**能失败的测试**（RED）再实现（GREEN）；时间全部经可注入时钟（测试规范）；模型调用全 mock（离线、可复现）；每阶段完成跑全量门禁 `cargo check && cargo test --workspace --no-fail-fast`（AGENTS.md §7）并报告计数；涉及交互行为另补 PTY e2e。测试文件：`src/app/tests/memory_*.rs`＋模块内单测；全部落 `src/app/src/memory/`（或 `memories.rs`，随 D1 定）。

### M1 状态存储与骨架（对应参考 §18.6-步骤 2；**已交付**——2026-10-18 实测回填、用户批准后合并 main @ `cf64a80`）

- **目标**：落地 D1 修订——`rusqlite`（bundled）引入、上游 DDL（`state/memory_migrations/*.sql`）逐字建库、`MemoryStore` 函数签名与 SQL 逐条移植（去 async）、`threads` 登记表（G6）、目录初始化与配置段。
- **输入**：`TCHAT_USER_DIR`、`config/manual.json`、上游 SQL 文本、可注入时钟。
- **输出**：`memories/` 骨架（0700）＋ `memories.sqlite`（0600）、MemoryStore API（领取/所有权/租期/心跳/重试/水位——**使用计数与保留清理按 §2.2 归 M8/M9**，本阶段仅 DDL 列就位）、配置结构体。
- **涉及模块**：新 `src/app/src/memory/{mod,store,migrations.sql,config}.rs`；`catalog.rs`（manual 段装载）；`src/app/Cargo.toml`（rusqlite bundled，登记）。
- **先失败的测试**：所有权令牌过期/错误后提交被拒（A10，`WHERE ownership_token=?` 语义）；租期到期可被抢占；重试计数与 `retry_at`；同库事务（stage1_outputs＋jobs 同提交）；时钟注入下的 6h 冷却/3,600s 租期；SQL 行为锚＝上游 `migrations_tests.rs` 对应断言移植。
- **验收命令**：`cargo test --workspace memory`（单元＋集成）→ 全量门禁。
- **完成条件**：上游 SQL 全部语义有测试钉住；busy 重试/`BEGIN IMMEDIATE` 行为验证；崩溃恢复（kill 后 lease 过期重领）用例通过。**实测达成（2026-10-18）**：崩溃恢复语义＝租期到期接管＋失权拒提交用例覆盖（`m1_stage1_lease_expiry…`/`m1_stage1_stale_token…`）；集成 21/21＋模块单测 2/2＋catalog 32/32；变异证明×4（所有权守卫恒真/0600 缺失/冷却置零/迁移两步化均被捕获）；全量门禁见 `docs/records/2026-10-18-013-m1-implementation.md`。
- **实施坑记录（2026-10-18 M1 轮）**：①迁移 DDL 与登记必须同事务——两步 autocommit 在崩溃夹缝后重放无 `IF NOT EXISTS` 的 DDL 会永久打不开库（code-review 发现，修复＋回归测试＋变异证明）；并发打开还要求写事务内复核登记，避免后到连接重放已应用 DDL（2026-10-21 修复，见文首修订）；②上游冷却边界＝`finished_at > now−6h`，恰满 6 小时即可领取（时间用例须边界两侧留余量）；③更新输入水位会**恢复**重试计数（上游 §8.7）——耗尽用例必须同版本重试；④stage1 失败重试无下限、phase2 用 `max(…,0)` 不减破 0（上游差异，逐字保留）。

### M2 候选选择（§18.6-3；**已交付**——2026-10-18 实测回填、用户批准后合并 main @ `3d4e80e`，证据 `docs/records/2026-10-18-013-m2-implementation.md`）

- **目标**：实现 §7.3 全部筛选规则。
- **输入**：sessions 目录、当前会话 ID、时钟、配置（闲置/年龄/领取/扫描上限）、`threads` 登记表（SQLite，随 D1）与 stage1 成功版本。
- **输出**：有序候选列表（≤领取上限），按会话 ID＋源更新时间去重。
- **涉及模块**：`memory/candidates.rs`；复用 `session.rs` 枚举/装载。
- **先失败的测试**：排除当前会话（A04）；闲置不足/年龄过旧/两者满足（A05）；同版本已成功跳过、空结果版本仍跳过（A06/A07）；更新后重新可提炼（A08）；首次使用空候选（A15）；扫描/领取上限；时间边界两侧余量。
- **验收命令**：`cargo test --workspace --test memory_candidates --test memory_store`（**勘误，2026-10-18 实施轮**：原写 `cargo test --workspace memory` 按测试名过滤，m1_/m2_ 用例 0 条执行＝空转通过；M1 条目同款命令同病——M1 实际验证用的是 `--test memory_store`，以记录为准）。
- **完成条件**：矩阵 A04–A08、A15 全绿；时间用例不依赖真实墙钟。**实测达成（2026-10-18）**：`memory_candidates` 18/18（A04–A08/A15＋扫描预算计数口径＋退避/耗尽跳过＋重复触发租期挡板＋目录∩登记交集＋配置默认/兼容/钳制）；变异证明 5 项按预期失败＋2 项等价变异排除登记（needs_update 预检查、领满 break——均由 `try_claim` 事务内语义等价兜底）；code-review 双轴发现全处置（含 `Default`/serde 分裂修复、验收命令勘误）。
- **实施要点与坑记录（2026-10-18 M2 轮）**：① **扫描上限计数口径**（预测→实测）：§4 行「目录枚举上限同值」的落地形态＝上游口径——`LIMIT 5000` 约束 threads 扫描行，目录枚举（`list_sessions`）仅作存在性交集不受限；预算被随后过滤掉的行（无文件/已最新）消耗不回补，有测试钉住。② 窗口边界（上游 SQL 逐字）：`updated_at_ms >= now−10d` 与 `<= now−6h` **均含端点**（恰满 6h 闲置/恰满 10 天入选），毫秒精度；`ORDER BY updated_at_ms DESC` 无并列决胜键（上游显式 false），测试避免依赖并列顺序。③ 源版本＝`updated_at_ms.div_euclid(1000)`（上游 `timestamp()` 截断语义），亚秒余量有测试钉住。④ **钳制面**：本结构＝上游 Toml 面（原值装载），`clamped()`/`params_from_config`＝effective 面——消费方必须经钳制面取值（上游 `From<MemoriesToml>` 同构）；`Default` 手写与 serde 缺省对齐（审查修复）。⑤ **职责确认**：M2 只读 threads 登记、经 M1 原语领取；**登记写入方＝会话写入侧，随 D2 触发接线（M7 落地；2026-10-18 M3 分支准备轮由「D2/M3」收敛，M3 不含登记）**（接线前真实运行候选恒空＝A15 语义）；`enabled` 总开关门在调用方（上游门在启动入口），D2 接线时先 gate。⑥ `allowed_sources`/归档过滤无本地概念（会话全部产自本 CLI；threads 最小表无归档列）——登记差异沿 §2.2。

### M3 第一阶段提炼（§18.6-4；**已交付**——2026-10-18 实测回填、用户批准后合并 main @ `63bff52`，证据 `docs/records/2026-10-18-013-m3-implementation.md`）

- **目标**：单会话 → 严格 JSON 结果 → 三态入库。
- **输入**：候选会话记录（预算内：8 MiB 总量/1 MiB 单记录/回退字符预算）、提炼提示词（V1 优先保留/舍弃清单，§8.4）、mock 模型响应。
- **输出**：`stage1_outputs` 写入/替换、空结果删除旧记录＋传递变化、失败记错误＋`retry_at`。
- **涉及模块**：`memory/phase1.rs`＋`memory/phase1_output.rs`（上游同名文件移植：模板加载、输出合同解析）；输入组装＝上游 `rollout_input.rs` 函数级移植（§2.2 行 89——8 MiB/1 MiB/预算裁剪）＋HistoryItem→提炼输入映射（§2.2-7，宿主注入不落会话＝免剥离差异）；模板资产拷贝 `memory/templates/{stage_one_system,stage_one_input}.md`（上游原文，Apache-2.0 来源登记）；`redact_secrets` 移植（D7）；`utils/string` 估算公式与 `TruncationPolicy` 移植（D11）；模型调用复用 `api.rs::chat_with_body`。
- **先失败的测试**：输出不符 schema→失败＋重试（A11）；空结果三态（A12）；秘密替换作用于输入/存储/导出三处（A13）；单记录超限跳过、总预算裁剪从最近向前；所有权失效时不提交（A09/A10 完整）。
- **验收命令**：`cargo test --workspace --test memory_phase1 --test memory_candidates --test memory_store`＋`cargo test --lib memory::`（**勘误，2026-10-18 分支准备轮**：原写 `cargo test --workspace memory` 按测试名过滤＝空转通过，同 M2 实施轮勘误；M4–M6 条目同款命令同病，各阶段实施轮修正）。
- **完成条件**：三态语义与母本一致；「时间资格」与「内容价值」分开测试（§18.4 末条）。**实测达成（2026-10-18，含补验轮 19/19）**：集成 17/17（补验轮＋2 存储故障用例）（A11 四类违规/A12 三态含删旧传递/A13 三位置脱敏＋失败信息面/A09+10 失权三路拒提交/超限与预算裁剪/selected_model 实体入请求体/注入剥离/失败面/全链 M2→M3）＋模块单测 22＋变异证明 6 项；门禁全过（tdd、comprehensive-testing、Security Gate 进入并通过、code-review 双轴——硬违规 truncate 重复已整改为复用 008 shim、verify:full 见记录）。
- **实施要点与坑记录（2026-10-18 M3 轮）**：① **truncate 复用 008 shim**（code-review 硬违规整改）：`codex-utils-string` 已含上游 `utils/string/truncate.rs` 逐字节实现，app 以 path 依赖复用，本地仅承载 `TruncationPolicy`/`truncate_text`；② regex-lite 兼容性经移植测试证明（D7 条件满足，未引入 `regex`，无新锁条目）；③ 模板两文件逐字节拷贝（sha256 一致），渲染＝单遍占位符替换（元数据先行、内容最后——内容中模板样式文本不二次替换，有测试钉住）；④ **结构化输出分层（2026-10-18 补验轮核准；同日字段层级勘误）**：上游 Codex 以内部参数 `Prompt.output_schema`＋`output_schema_strict` 声明合同（映射 Responses API 的 HTTP 字段 `text.format`——`output_schema` 非 HTTP 字段）；Chat Completions 对应机制＝`response_format:{type:"json_schema", json_schema:{name, schema, strict}}`（OpenAI 官方文档；仅较新模型支持）——M3 不自行发送（不做未实测的供应商支持面假定，D6 零新增配置面），请求侧由上游模板原文承载字段契约，机器级保证在响应侧严格解析（违规→失败＋重试，进不了存储）；差别＝违规输出成本一次调用而非被端点阻止（登记差异）；用户可经既有 `request_params`（`supported_params` 门）自行透传；供应商实测待 M7；⑤ `rollout_cwd` 由调用方传入（会话工作目录未持久化，登记差异）；⑥ 失败原因入库前脱敏（上游不脱敏，本地加严，登记偏离）；⑦ 过滤时机＝序列化侧（上游读取侧剔除不占 8 MiB 预算；本地会话结构性无注入块，影响为零，登记）；⑧ **A13「导出」面**在 M3 无载体，本地以解析面替代——**M4 Markdown 同步须补脱敏断言闭环**（见 M4 条目注记）。⑨ **存储 I/O 故障（2026-10-18 补验轮）**：目录改名注入（journal 创建 ENOENT，root 亦确定生效）两用例——成功提交路径存储错误＝传播 Failed＋无部分提交（行/水位/计数/租期/结果/全局信号全未动）；失败记账路径存储错误＝按上游 `let _` 语义吞没、行原状不 panic；与失权拒提交（SQL 条件拒）分立有据，实现无缺陷。
- **实施前确认（2026-10-18 分支准备轮）**：无阻塞性待决项——① D6（模型）：**用户已确认（2026-10-18）**——默认＝当前 `selected_model`，零新增配置面（`extract_model` 不提前建），本轮全 mock 未调用真实模型；② D7（秘密）：regex-lite 兼容性以移植测试判定（实测满足）——证据条件分支，非用户决策；③ D11（换算）：上游公式与常量移植（经 008 shim＋收窄层），同嵌入契约；④ **threads 登记接线不属 M3**——M3 输入＝M2 候选（`Stage1JobClaim`），登记写入方＝会话写入侧、随 D2 触发落地（**M7**，`memory/runner.rs`；M2 条目⑤原写「D2/M3」已按本规格收敛为 D2/M7）；⑤ 配额检查跳过（D8，无接口）。

### M4 Markdown 同步（§18.6-5；**已交付**——2026-10-18 用户批准后快进合并 main @ `4cd9f19`，任务分支 `feat/013-memory-m4`，完整基线见文首，证据 `docs/records/2026-10-18-013-m4-implementation.md` §9）

- **目标**：选中记录 → `rollout_summaries/*.md`（前部 thread_id/updated_at/rollout_path/cwd）＋`raw_memories.md` 稳定序重建；不再选中的摘要删除。
- **输入**：stage1 结果的当前全量快照（非增量事件），按 thread_id 升序交给文件同步；本阶段不引入 M5 的使用次数/保留窗口/数量上限选择规则。现有 `Stage1OutputRow` 提供正文与源版本，`SelectedOutput` 仅含 ID/版本，不能替代正文输入；`rollout_path`/`cwd`/可选 `git_branch` 由调用方补齐。当前行与 007 会话 meta 未保存原会话 cwd，沿 M3「调用方提供 cwd」差异，不伪称已恢复原会话工作目录，不以 threads 登记接线为前置。
- **输出**：记忆根目录文件变化（新增/修改/删除、稳定排序）。
- **涉及模块（实测回填）**：`memory/storage_files.rs`（`Stage1FileInput::from_row`＋`sync_stage1_snapshot`）、`memory/store.rs::list_stage1_outputs`（单 SELECT 全量升序，与单行读取共用行映射）、`memory/mod.rs` 导出；`memory_storage_files` 集成目标与 9 份预写黄金主文件。不改 Schema、配置或生产触发点。
- **复用与适配（实施回填）**：上游正文/命名/清理与布局函数同步移植，完整切片取代上游 cap 参数（M5 上游调用本也传切片长度）；诊断改可注入出口并复用 `display::sanitize_for_terminal`；写入复用 `catalog::write_private_file`（根/写入父目录 0700、产物 0600，既有权限亦收紧，仍直接覆盖不回滚）。chrono/uuid 使用工作区既有锁定版本，未新增包版本；不生成 UUID、不调用模型。
- **固定上游依据（2026-10-18 核对）**：[storage.rs](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/storage.rs) 的同步、重建、清理和文件名函数＋[storage_tests.rs](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/storage_tests.rs)；布局依赖见 [lib.rs::ensure_layout](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/lib.rs) 与 [workspace.rs::remove_memory_symlinks](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/workspace.rs)。函数级移植，异步文件 API 换为同步 API（§2.2）。
- **覆盖与清理范围**：`raw_memories.md` 全量覆盖；`rollout_summaries/` 为程序生成目录，本轮 canonical 文件名组成 keep 集合，同名文件覆盖，直属目录项中所有小写 `.md` 后缀、且 stem 不在 keep 的项均尝试 `remove_file`（含旧 ID 命名与旧 slug 文件，**不靠生成清单或特定命名正则认领**）。不递归清理摘要子目录，不将非 `.md` 文件纳入旧摘要清理。普通 `MEMORY.md`/`memory_summary.md`/`skills/` 内容由 M5 合并维护；`phase2_workspace_diff.md` 与 `.git/` 基线由 M5 生成/清理/重置；便条与扩展资源按 M8/M9，均不纳入 M4 的正文重建或过期清理。数据库与原始会话不由文件同步修改。
- **布局清理的独立边界**：上游两个文件入口都先 `ensure_layout`——拒绝记忆根本身为符号链接，递归遍历根内目录，移除遇到的符号链接而不跟随其目标，再建立 `rollout_summaries/`。此链接清理不限于摘要目录（亦可涉及上述后续维护目录中的链接），不能把「普通文件不重建」误写成「这些目录完全不触碰」；本地根初始化复用 M1 `init_memories_root`（0700），保留该布局语义，错误返回调用方。仅在隔离临时目录验证，禁止用真实记忆目录验收。
- **格式与幂等边界**：摘要文件名前缀取 UUID 时间（不可用时取源更新时间）＋上游四位短 hash；本地非 UUID 会话 ID 走上游已有 fallback；可选 slug 按上游 ASCII 规则规范化、最多 60 字符。正文前部保留 thread_id/UTC RFC3339 源更新时间/rollout_path/cwd，可选 git_branch；raw 合集包含 thread_id/源更新时间/cwd/rollout_path 及实际摘要文件名，正文取 `raw_memory.trim()`。上游存储函数保留入参顺序、不自行排序；调用侧保证 thread_id 升序。同输入重跑要求文件名/内容字节一致，不承诺 mtime 不变，不需要 Git 基线验证内容幂等。
- **空集合与旧文件**：M3 空结果成功已删除旧结果行并推进成功水位，M4 消费其当前快照，不重复判定提炼三态。快照归零时清理摘要目录直属旧 `.md`，保留并重写 `raw_memories.md` 为 `# Raw Memories\n\nNo raw memories yet.\n`；不直接删除共享经验或总摘要，来源删除如何影响共享知识留 M5/M9。
- **失败契约（保留上游，不增加原子性承诺）**：布局、目录读取/迭代及产物写入错误返回；同步次序为清理旧摘要→写各摘要→重建 raw 合集。文件直接写入，可能截断旧文件；失败前已发生的删除/覆盖不回滚，文件与数据库之间也无原子事务。单个旧摘要 `remove_file` 失败除 NotFound 外仅告警并继续，可能残留旧件，不将其误报为清理已完成；该 best-effort 分支与致命写入失败分开验证。同步本身不提交任务成功、水位或 selected 状态，M5 后续负责记账与重试编排。
- **A13 导出面脱敏闭环（M3 交接，2026-10-18）**：位置＝`StageOneOutput::parse` 对 `raw_memory`/`rollout_summary`/`rollout_slug` 逐字段调用既有 `redact_secrets`→入库→M4 文件投影；不新定秘密识别规则。验收用真实 M3 解析/存储→M4 导出链（模型边界 mock），在三字段都放入固定秘密样例，断言产物正文及文件名不含原始样例、正文含 `[REDACTED_SECRET]`，文件名对应**脱敏后的 slug 再规范化**的期望值；仅断言找不到原始密钥不足以证明 slug 路径正确。
- **先失败的测试**：A20 来源增/改/减/归零的文件与目录黄金结果；canonical 改名清旧件、全量稳定排序与字节幂等；上述 A13 三字段贯通；非管理普通文件保留与布局链接清理不写链接目标；可复现的写入失败及旧件清理告警，分别检查已发生文件副作用与数据库未变。关键用例保持断言，通过修复前后或隔离变异证明会发现错误（AGENTS.md §4）。
- **验收命令（2026-10-18 实测，已勘误空转问题）**：在 `/workspace`，L1 `cargo test --workspace --test memory_storage_files`＝**23/23，0 忽略、0 过滤**（目标已实际建立并运行）；L2 `cargo test --workspace --test memory_storage_files --test memory_phase1 --test memory_candidates --test memory_store`＋`cargo test -p terminal-chat --lib memory::` 全绿，各轮计数见证据；最终完整入口 `cargo check`、`cargo test --workspace --no-fail-fast` 均 exit 0，**2879 通过/0 失败/3 既有忽略，0 过滤，其余测试未运行 0**。旧 `cargo test --workspace memory` 按测试名过滤，禁止以 0 条执行作为通过。clippy/rustfmt 仍未安装，两项工具检查未运行，不报通过。
- **完成条件（实测达成）**：文件副作用与数据库状态分别断言；A20 增/改/减/归零、幂等、管理边界、真实失败副作用、告警继续、链接与权限及 A13 真实链全部通过。新增 23 条集成用例，11 项隔离变异均被断言发现，恢复后最终 M4 23/23；安全检查与手工两遍审查完成，最终完整验证覆盖当前候选（含 CLI/PTY e2e 95/95）。详细来源、失败根因、命令和忽略项见 [M4 实施证据](../records/2026-10-18-013-m4-implementation.md)，M3 交接 2856/0 保持历史依据，不冒称本轮基线全量实跑。
- **实施前确认**：无阻塞性待决项——D1/D5 已确认且 M1–M3 接口就位，D7 的 regex-lite 兼容性证据条件已由 M3 满足；本阶段文件同步不调用模型。G8 的基线机制及 D3 的合并工具能力属 M5，D4 注入属 M6；供应商兼容性、threads 登记与 D2 后台触发属 M7，D9 命令命名亦为后续项。**这些及其他未确认决定均未随本轮获批**；用户的实施与本次批准交付均仅覆盖 M4。

### M5 第二阶段合并（§18.6-6；**已交付**——2026-10-19 用户批准后快进合并 main @ `96e5908`，任务分支 `feat/013-memory-m5`，基线见文首；最终全量 2926/0/3（实施轮 2923＋心跳契约修复与超时补验＋心跳退出边界定点收口两轮，见修订行），证据 `docs/records/2026-10-18-013-m5-implementation.md` §0/§0a/§7/§10）

- **目标**：全局协调（锁/冷却/重试）＋来源选择（§9.3 排序）＋差异驱动合并＋写后校验＋基线重置。
- **输入**：stage1 全量结果、git 基线（记忆根仓库）、mock 合并代理行为。
- **输出**：`MEMORY.md`/`memory_summary.md` 更新、差异文件生成与清理、本轮实际使用的来源版本记录（selected_outputs）、合并进度。
- **涉及模块**：`memory/phase2.rs`（上游 phase2.rs 函数级移植：全局领取/心跳/写后确认；编排序＝领取→prepare→输入选择→同步→diff→短路或合并→校验→所有权再确认→reset→SQL 成功，源码核实 2026-10-18 定点修正轮）、`memory/workspace.rs`（基线三原语＝外部 `git` 进程：ensure＝`rev-parse --verify HEAD` 判可用＋不可用时 `git init` 幂等 reinit＋普通 commit 初建、diff＝前置同一条 `add -A -f`＋排除集后 `diff HEAD`＋状态列表、reset＝删临时差异文件→add→`commit --amend --allow-empty`→`read-tree --reset HEAD`——**保留上游 git 基线机制**，G8 收敛，文件集合/暂存/环境隔离契约见 §3 G8 行；写后校验逐项移植自上游 `workspace.rs`：symlink 拒绝、MEMORY.md 存在、摘要首行 `v1`；`phase2_workspace_diff.md` 在 prepare/diff/reset 三处前置删除、NotFound 忽略，从不入基线）；合并轮＝参数化工具集的 `run_tool_turn`＋`mem_read/mem_write/mem_list`（D3 已确认，2026-10-18）。
- **先失败的测试**：冷却期内不领取（A16）；无差异且输出有效→成功不调模型（A17）；无差异但输出缺失→不走捷径（A18）；来源选择排序规则（A19）；代理自称完成但输出无效→失败不重置基线（A21）；合并期所有权丢失不提交成功（A22）；校验项＝MEMORY.md 存在/摘要首行 `v1`/符号链接拒绝（§9.7-V1）；**受限工具边界（D3，2026-10-18 已确认）：绝对路径/父目录跳转/符号链接/目录写入全部拒绝、仅记忆根内可读写（越界用例逐条）；`memories.sqlite`/`memories.sqlite-journal` 与隐藏路径读取、写入、列表展示全部拒绝——mem_list 输出不含两库文件与隐藏条目（批准轮补）；工具注册表恰为三项（不含 exec_command/apply_patch）断言；合并产物全部落在记忆根内的文件断言（§2.2 验收）**；**基线契约（G8，2026-10-18 定点修正轮补、同日批准轮补两项验收）**：被忽略的新摘要仍被差异检测（记忆根存在 `.gitignore` 忽略 `*.md` 时强制暂存仍可见）；**曾被跟踪后落入排除集的文件经处理后不再产生差异（批准轮新增——处理机制以实施轮实现为准、须确定并测试固化；探测注记：仅靠排除 pathspec 不够，`diff HEAD` 对 HEAD 中已跟踪文件的改动仍可见）**；无业务内容变化时完整流程重跑差异为空（库记账写入被排除集挡住＋临时差异文件三处前置删除）；A17 短路成功**不 reset 基线**、照写成功/新水位/selected 记账；ensure/diff/reset 失败均属**领取后**失败、按 retry 记账（失败原因枚举对齐上游 `failed_prepare_workspace`/`failed_workspace_status`/`failed_workspace_commit` 等），reset 失败不得写 SQL 成功；部分文件变化不回滚（§9.8）；**reset 单提交语义适用前提实测（批准轮新增）**：适用前提＝本契约自建且自管理的记忆根专用仓库（基线由 ensure/reset 自身建立的单 commit 历史），连续 reset 后 `rev-list --count`＝1 以测试固化（探测证据已有），**不预先将 `commit --amend` 泛称为任意仓库重建的等价实现**；**父项目仓库不变断言（批准轮新增）**：记忆根 git 操作全部落记忆根自有仓库，父项目仓库（含工作仓库）状态不变；**配置隔离（G8）**：env 注入 `diff.noprefix`/`core.excludesfile` 下输出格式与差异范围不变（隔离前后对照断言）；无全局 identity 时 commit 仍成功（固定 `-c` 值）；`GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE` 泄漏不干扰（探测实证各注入面均有可见影响，隔离后恢复）。
- **验收命令（2026-10-18 M5 决策收敛轮勘误，空转病同 M2–M4 先例）**：`cargo test --workspace --test memory_phase2 --test memory_store --test memory_candidates --test memory_phase1 --test memory_storage_files`＋`cargo test -p terminal-chat --lib memory::`（合并代理 mock：受限 turn 的模型响应＋工具调用序列 scripted；测试目标名以实施轮实际建立为准，最终以全量门禁覆盖）→ 全量门禁。原 `cargo test --workspace memory` 按测试名过滤 0 条执行＝空转通过，禁止作为验证依据。
- **完成条件**：「输入标记≠文件差异≠需要模型」三件事分立且有测试（§9.4）；部分失败不虚报成功。
- **实施前确认（2026-10-18 规格确认登记与分支准备轮）**：**D3、G8 用户已确认（2026-10-18）**——批准内容为下列两条边界及相应验收要求；任务分支 `feat/013-memory-m5`（基线 main @ `b43b2e79bc79ce3e38639ad29668ea768b7302fb`、合并目标 `main`）已建立，本轮仅登记未提交，M5 保持待实施。**其他待决项（D2/D4/D9/D11 及其余）不视为随本次批准通过**；本轮未实现 M5、未改生产代码或测试、未运行测试（M4 交付的全量 2879/0/3 为既有交接证据），未执行的验证保持待验证、不新建重复计划。
- **D3 批准边界与验收（2026-10-18）**：①仅内部合并轮注册 `mem_read`、`mem_write`、`mem_list`（注册表恰为三项，断言不含 exec_command/apply_patch/其他）；②合并轮不提供 `exec_command`，主聊天工具（003/004 契约）保持既有行为不变；③工具访问限记忆根——拒绝绝对路径、父目录越界（`..` 跳转）与符号链接（组件级）；④隐藏路径及 `memories.sqlite`、`memories.sqlite-journal` 不向模型暴露，不允许读取、写入或列表展示（mem_list 输出过滤同边界）；⑤保留规格规定的文件类型限制（写入仅限常规文件）、参数（读取行号 1 起/`max_lines`≥1）与输出预算限制（读取默认 20,000 tokens 预算、列目录默认上限 2,000 条）。验收＝上述各边界的越界用例逐条＋工具注册表断言＋mem_list 不展示断言＋合并产物全部落在记忆根内的文件断言（§2.2 验收节同步更新）。
- **G8 批准边界与验收（2026-10-18）**：①保留上游 Git 基线机制，以内部固定的外部 Git 进程承载（不经 shell、环境隔离、固定 identity——确定性契约全文见 §3 G8 行）；②仅操作记忆根自有的专用仓库，**父项目仓库不得改变**（验收含父仓库/工作仓库状态不变断言）；③ensure、diff、reset 使用一致的文件集合及忽略处理——排除数据库（`memories.sqlite`）、journal（`memories.sqlite-journal`）与临时差异文件（`phase2_workspace_diff.md` 三处前置删除）；④验收覆盖：新增被忽略文件（`.gitignore` 忽略 `*.md` 时强制暂存仍可见）、**曾被跟踪的排除文件**（旧基线曾提交后落入排除集的文件经处理后不再产生差异，机制以实施轮实现为准、须确定并测试固化）、无业务变化重跑（完整流程重跑差异为空）、配置隔离（env 注入对照断言）及固定 identity（无全局 identity 时 commit 仍成功）；⑤**reset 的单提交语义须明确适用前提并通过实测**——适用前提＝本契约自建且自管理的记忆根专用仓库（单 commit 基线由 ensure/reset 自身建立），连续 reset 后 `rev-list --count`＝1 以测试固化（探测证据已有），**不预先将 `commit --amend` 泛称为任意仓库重建的等价实现**；⑥先领取全局锁，再 prepare/ensure；⑦无差异短路不 reset 基线（照写成功/新水位/selected 记账）；⑧reset 在 SQL 成功提交之前（reset 失败不得写 SQL 成功）；⑨保留已收敛的所有权（`WHERE ownership_token=?` 条件提交）、重试（领取后失败按 `job::failed`＋retry 记账）及非原子失败契约（§9.8 部分文件变化不回滚）。
- **实测回填（2026-10-18 实施轮；心跳与超时两处由同日修复补验轮更新，见修订行与记录 §0）**：全部批准边界已实现并验证——分派层白名单（合并轮拒执行 exec_command/apply_patch/一切未注册名，请求体工具列表不构成保护）、越界/隐藏/库文件/符号链接/目录逐条拒绝＋mem_list 输出过滤（含**任意层级**同名排除的从严实现）、三原语排除集一致（曾被跟踪文件经 `rm --cached` 退出基线且真实库文件不删不改、内容不变）、`.git` 缺失与父仓库嵌套场景锚定有效（父仓库 HEAD/index/工作区不变断言）、env 注入面（`GIT_CONFIG_COUNT`/`GIT_CONFIG_GLOBAL/SYSTEM`/`GIT_DIR` 等）隔离前后行为不变、固定 identity（author/committer 逐字断言）、连续 reset `rev-list --count`＝1、A17 短路不 reset 且照写水位/selected（enqueue 旧版本新输入的水位 `+1` 语义实测：防旧水位短路）、§9.8 校验失败部分产物保留不回滚、全部领取后失败按 retry 记账（3→2/退避 3,600s，含 failed_workspace_status 实测＝.git 不可写）、合并产物全树 containment、**心跳＝请求前续租＋请求阻塞期间 90s 受控时钟续租＋请求返回即停（RED 复现缺口后修复，第二领取者在有效续租期间恒被挡）**、**git 子进程超时行为证据（M5 入口直达：超时触发/含后代进程组全灭/Err 报生效毫秒/基线未建立）**。测试 38/38＋模块单测 7；错误检测证明 9 项变异。剩余说明：心跳存储故障容忍未注入实例（后果同失权路径，登记）；TOCTOU 窗口与上游 local.rs 同界（登记）。

### M6 读取注入与检索指引（§18.6-7＋8 的 V1 能力适配；**已交付，2026-10-19**）

- **目标**：总摘要＋查找/使用规则组成注入块；检索经现有 `exec_command`。
- **输入**：V1 记忆根与 `memory_summary.md`、开关（总开关默认 false；`use_memories` 默认 true，§4）、预算（上游常量 2,500 tokens，D11）；不需要 threads 登记或运行后台管线才能读取已有摘要。
- **输出**：可选注入块及 `prompt.rs` 请求组装扩展；指引文本＝拷贝上游 `ext/memories/templates/memories/read_path.md` 模板（渲染 `base_path`＋`memory_summary` 两占位符，原文含查找指引与引用格式——§10.2/§10.3；仅按 §2.2 已登记的 Markdown/原始 JSONL 路径错述作必要适配）。
- **涉及模块**：`memory/inject.rs`、`memory/templates/read_path.md`、`memory/mod.rs`；复用/扩展 `prompt.rs`、`turn.rs` 的现有参数化入口以支持可选块与轮内复用；`memory/config.rs` 补规格既定 `use_memories` 缺省兼容。预算直接复用 M3 `memory/truncate.rs` 与 008 shim；主驱动生产接线归 M7。
- **先失败的测试**：总开关关/摘要缺失或全空白→不注入（A01/A02）；存在→注入块含摘要与指引、预算生效（A03）；检索指引引导 `rg` 于记忆目录（用 e2e 验证模型工具调用可用性，mock 响应含工具调用）。**结构契约测试（2026-10-18 补充轮增补，§5.1）**：注入后仍恰一条 `system`、恒首位、消息顺序与条数不变（扩展 `prompt_assembly::s3/s4` 形态断言）；`tools` 恰两定义不变；无记忆段时组装产物与现状逐字节一致（零回归锚）；摘要超 2,500 tokens→按上游 `TruncationPolicy::Tokens` 截断（行为与上游一致）、指引模板文案不裁。
- **补充验收边界（实施前制定，本轮实测见下）**：读取失败（含非法 UTF-8）与空白跳过；总开关/use_memories 任一关闭均不读摘要、不创建记忆目录或数据库；旧配置与默认用户轮保持既有请求、工具、历史/恢复、取消与显示行为。轮内修改摘要后续请求仍用同值；下一新轮及恢复后新轮读最新；自动注入块不落会话、不进入提炼或内部合并轮。模板渲染用固定路径归一化黄金文件，摘要内模板样式文字不二次展开。检索以真实 turn→本地 mock API→`exec_command` 执行 `rg`→结果进入下一请求，覆盖命中与未命中；不以检查指引字符串代替可用性证据，也不宣称通用 exec 受记忆根路径限制。M6 可用最薄宿主验证该能力；真实两驱动开态接入及开/关 PTY 闭环归 M7，本阶段 PTY 回归默认行为。
- **验收命令（勘误，2026-10-18；工作目录 `/workspace`）**：原 `cargo test --workspace memory` 是**用例全名子串过滤**，不是按测试文件选目标——`memory_candidates`/`memory_phase1`/`memory_storage_files` 的相关用例都会被过滤；即便其他目标执行了匹配用例，也不能证明 M6 执行。M6 已建立明确目标 `src/app/tests/memory_inject.rs`，先用 `cargo test -p terminal-chat --test memory_inject -- --list` 核对非零目标清单（仅列举不算通过），再运行 `cargo test -p terminal-chat --test memory_inject --test prompt_assembly --test session_cli --test memory_phase1 --test memory_phase2`；PTY 回归＝`cargo test -p terminal-chat --test e2e_acceptance`；另补 `cargo test -p terminal-chat --lib --test memory_candidates --test memory_store`；最终完整门禁仍为 `cargo check`、`cargo test --workspace --no-fail-fast`。**准备阶段历史说明保留：当时拟新增 memory_inject 目标尚不存在，上述命令仅为实施后规划，未列举或运行，不算通过**；本轮实际结果见下及实施记录，验收命令未带用例名过滤。
- **完成条件（本轮达成；2026-10-19 批准交付）**：落实已确认的 §5.1 契约与 A01–A03；上述能力/结构/轮缓存/隔离/检索用例非零执行；既有 prompt_assembly 6 用例及 M1–M5 回归语义保持，默认用户轮与 PTY 回归通过；关键新增用例有「破坏→失败→恢复→通过」证据；完整门禁通过。供应商角色语义等价不由结构测试声称，M7 项保持待验证。

**实施与验证结果（2026-10-19 实施轮；同日批准交付，见修订记录）**：新增 `memory_inject` **18/18**；无首行版本拒绝策略（读取侧按固定上游视为文本，M5 写后 `v1` 校验边界不变）。摘要保留正文预算 2,500 近似 tokens，UTF-8 首尾保留/中段截断，标记与固定指引另计；有/无摘要均按轮缓存；真实保存恢复及提炼/合并隔离、真实 `rg` 两条退出状态路径均实测。8 项关键错误检测全部失败如期并逐项恢复复绿；安全门、双轴审查及完整门禁完成：`--list` 列出 18（不计通过），组合 107/0/0、补充 168/0/0、PTY 95/0/0、check exit 0、workspace **2944/0/3**（0 过滤，其余未运行 0）。源码最终验证后未再改动；来源、RED/GREEN、方法/告警/限制与保护项见 [实施记录](../records/2026-10-19-013-m6-implementation.md)及[安全报告](../security/reports/2026-10-19-memory-inject-security.md)。M7–M9 保持未实施；无真实模型调用、未操作真实记忆目录（实施轮边界——未暂存/提交/合并/推送/删分支；提交、快进合并与删分支由批准交付轮完成，见修订记录）。

**以下为实施前确认、决策与分支准备的历史登记；其中“本轮”指各准备轮，不代表当前实施状态。**

**实施前确认（2026-10-18，只读源码核实）**：

- **Git 交接**：`main @ 1762958fb01b60673e681f3d42006b8df8b19d5e`（M5 交付文档提交；实现 `96e5908` 已在祖先链），暂存区/stash 为空、仅 `/workspace` 一个工作树，无任务分支；`git check-ref-format --branch feat/013-memory-m6` 合法，现有本地分支/工作树无同名或路径前缀占用，仓库无远程配置。保护项＝compose +10 行、2026-10-02 记录 +33 行、未跟踪 `db/wiki.db` 与 `.agents/agents/Security-Testing-Gate.md`，与 M5 记录 §10 一致；本轮只对保护项作只读内容/模式指纹核对，不打开数据库、不修改或暂存保护项。
- **前置已就绪**：M1 目录/配置约定；M3 `TruncationPolicy::Tokens`＋估算/截断实现；M5 通用 system/tools 组装入口与隔离的合并模式均已落地。M2 候选、M4 Markdown 同步与 M5 合并产物可供后续闭环，均已交付，但 M6 本身只依赖既定目录与可读摘要，不以 M7 来源登记/触发接线为前置。M5 最终 `cargo check` 与 2926/0/3 是既有交付证据，本轮未重跑。
- **源码锚与既定事项**：本地 `main.rs:343`／`tui_host.rs:229`→`run_tool_turn`→`run_tool_turn_with_mode`（User 分支）→`build_turn_request`→`build_turn_request_with`→sender→`api::chat_with_body`；发送层克隆并透传 messages，仅加 model/受限参数，仍有 10 MiB 总请求上限。固定上游 [`prompts.rs`](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/ext/memories/src/prompts.rs)＝read_to_string 失败跳过→trim→Tokens(2,500)→空则 None→渲染模板；[`lib.rs`](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/ext/memories/src/lib.rs) 常量为 2,500；本地复用实现保留 UTF-8 首尾、中段替换。**10,000 字节是保留摘要正文的近似预算，截断提示标记在此预算外；模板/边界/说明行也不计入摘要预算，不把整块硬裁到 10,000 字节，不乘 1.2 序列化余量**。按轮读取/轮内缓存/新轮重读/不入史已由 §5.1 规定，不是新增用户选择。上游源码已阅读，未编译或运行上游测试。

**历史决策包——D4（2026-10-18 当时待确认；2026-10-19 已按推荐批准，边界见下）**：

- **具体问题与推荐**：是否接受将上游 developer 记忆片段适配到**现有唯一 system 的尾段**，以固定 `<memory-context>…</memory-context>` 分隔，并保留 §5.1 的历史参考说明行？推荐接受：复用现有组装与在用角色，不改用户/助手/工具消息，不新增消息条目。依据为固定上游 [`extension.rs`](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/ext/memories/src/extension.rs) 的 `developer_policy` 和当前本地组装事实；这是显式协议适配，**不声称 system/developer 权限或供应商行为完全等价**。M5 批准仅覆盖 D3/G8，不覆盖 D4。
- **可选方案的实际差别**：①推荐＝仍恰一条 system，基础指令原文为前缀，仅追加记忆段，历史消息条数/顺序不变；②第二条 system＝逻辑块独立成消息，但消息数增加、恰一条 system 的启用态契约需改，且无 developer 原生语义保证；③原生 developer＝角色与上游一致，但引入本项目从未发送的角色，需修改启用态结构契约并在 M7 实测供应商支持，不凭透传能力推定支持。后两项若被选择，先修订对应契约；本轮不试发真实请求。
- **修改范围与验收边界**：按本条模块范围实现摘要/模板/可选组装/轮缓存与确定性测试；推荐方案按 A01–A03、唯一首位 system、基础提示词前缀保真、记忆段恰一次、工具恰两项、无段逐字节一致、工具配对与恢复、预算和隔离用例验收。D4 的确认只落定角色承载及对应包装，不提前批准 D2/D9 或扩展到 M7；供应商实测、threads 登记与后台触发继续归 M7。

**D4 批准边界与分支准备（用户已确认，2026-10-19）**：

- **任务分支与基线**：普通分支 `feat/013-memory-m6` 已从 `main @ 1762958fb01b60673e681f3d42006b8df8b19d5e` 同点建立，合并目标 `main`；分支 HEAD＝main＝merge-base＝上述完整 SHA，领先/落后均为 0，仍仅 `/workspace` 一个工作树。
- **① 承载位置**：记忆片段追加到现有唯一 system 消息尾段。
- **② 边界与性质**：使用固定 `<memory-context>…</memory-context>` 边界，明确内容属于历史参考，保留 §5.1 的历史参考说明行。
- **③ 请求结构**：不新增第二条 system 或 developer 消息；消息条数、顺序及现有 `exec_command`／`apply_patch` 两项工具不变。
- **④ 既定契约**：未注入时请求保持原样；既定摘要预算、轮内复用、下一轮重读及自动注入不入历史的契约不变。
- **⑤ 适配与后续**：登记为角色适配，不宣称 system 与 developer 语义等价；供应商实测仍归 M7。
- **批准范围**：其他决策不随本轮批准；M6 功能待实施、验收待验证。拟新增 `memory_inject` 目标尚不存在，未运行测试列举、Cargo 检查或测试，不把规划写成通过。四项保护内容及暂存区保持原样，本轮仅建立任务分支并更新本规格。

**准备状态（2026-10-19）**：Git/代码前置及 D4 批准均已就绪，任务分支已建立，**具备直接接收 M6 实施 `/goal` 的条件**。本轮停在分支准备阶段，不自动开始 M6，不再请求 D4 批准。

### M7 运行时接入与命令面（**代码接线已交付（2026-10-19）、离线验证完成；CFIX 非空提炼、合并工具链与注入用户轮三个独立真实样本分别通过，补验证据已交付（2026-10-19 归档 `9e32a63` 快进合并）；历史批次保持原判**；D2/D9 已确认）

**真实供应商补验前置核查与具体授权范围**：见 [M7 实施记录 §11](../records/2026-10-19-013-m7-implementation.md#11-真实供应商补验前置核查与待授权方案基于-main--b731542)。该节澄清 phase2 工具续轮与 HTTP 总预算；下文历史「各 1–2 次」不是 HTTP 请求上限。本轮不执行真实调用，保持「M7 代码接线已交付；真实供应商补验待授权／待验证」。

**2026-10-19 已授权执行轮状态**：上述「待授权／本轮不执行」为准备轮历史；本次唯一批次在 phase1 HTTP 发送前因 harness 合法目录校验缺陷退出，真实请求 **0/10**，phase2/注入均未执行；已封批，不修正续跑或重开，真实供应商补验仍未完成，见 [M7 实施记录 §11.6](../records/2026-10-19-013-m7-implementation.md#116-已授权单批执行结果2026-10-19本地-harness-失败真实请求-0待审阅)。M8/M9 未实施、V1 未完成。

**2026-10-19 离线修复轮状态**：原失败证据冻结保留；通过实际 CLI 复现 RED（退出 2）后，按路径组件及实际临时布局最小修复，合法 CLI 到达拒绝真实发送的离线检查点（退出 0）；相关离线检查 **42/0/0**，本轮真实 HTTP **0**。原 10 次未用额度封存，新批次仍须新授权；申请沿用 `deepseek / deepseek-flash`、`openai-chat`、原唯一接口／空参数、1＋8＋1／总 10 次与原停止条件，见 [M7 实施记录 §11.7](../records/2026-10-19-013-m7-implementation.md#117-harness-路径离线修复与新批次申请2026-10-19)。三项供应商行为、其他供应商、空结果专项、Windows 仍未验证，不开始 M8/M9、不标 V1 完成。

**2026-10-19 新授权批次结果**：用户另行明确授权后执行 `m7-deepseek-new-367qctgq`，真实 HTTP **1/10**；phase1 HTTP 200／正常结束，严格解析及存储路径得到合法 **`SucceededNoOutput`**（job done、水位提交、stage1_outputs=0），未达预定有内容成功入库目标，故立即封批；phase2 与注入 **未执行**。实际 usage 为 **6,634 输入＋287 输出＝6,921 tokens**，费用未知；本批未用 9 次、原批次未用 10 次各自封存，不续跑。仅此样本空态处理已观察，整批不标通过，详见 [M7 实施记录 §11.8](../records/2026-10-19-013-m7-implementation.md#118-新授权批次真实执行结果2026-10-19phase1-合法空结果停止整批)。其他供应商、未执行分支及 Windows 保持未验证，M8/M9 未实施、V1 未完成。

**2026-10-19 空结果离线审阅轮**：零真实请求。输入组装链（夹具→会话装载→过滤→投影→模板渲染→实际请求）与空结果判定链（三键合同→三态分派→存储副作用）离线重放 **14/14 通过**——模型输入完整无损，`SucceededNoOutput` 分派与存储符合合同；根因＝夹具首句自述"不能当作真实证据"触发生产提示证据门槛，模型合理 no-op，**非生产缺陷、非供应商协议不兼容**，原批判定不改。下一步最小补验方案（phase1 夹具 v2 非空补验＋phase2＋注入，建议分开授权执行）待用户批准，详见 [M7 实施记录 §11.9](../records/2026-10-19-013-m7-implementation.md#119-合法空结果离线审阅轮2026-10-19输入与判定链重放通过根因夹具自述虚构触发证据门槛方案待审)。

**2026-10-19 v2 夹具定稿与三批授权包（离线准备轮，真实请求 0）**：phase1 v2 夹具定稿——仅将首句替换为「以下为本次测试构造的合成会话。」，其余逐字不变，如实标注合成来源、**待实测、不保证非空**；v1 冻结保留。离线检查 **7＋3＋11 全部通过**（真实库三阶段排练含逐字节请求比对与禁发结构验证、CLI 合法入口 dry-run 检查点、v2 专项差异／内容完整性／预算）；生产源码零改动。三批独立补验授权包（phase1-v2 ≤1／phase2 ≤8／注入 ≤1，合计 ≤10，额度不互借，共同前提不成立即全部停止）已提交，**待用户批准后执行**，详见 [M7 实施记录 §11.10](../records/2026-10-19-013-m7-implementation.md#1110-v2-夹具定稿与三批独立补验授权包2026-10-19离线准备轮真实请求-0)。M8/M9 未实施、V1 未完成。

**2026-10-19 方案 A 执行轮**：用户已明确授权三个独立批次，上段「待批准」为历史状态。v2 与两份未改脚本的指纹、分支／HEAD 及脱敏配置复核符合预期，但新增分批控制层在共用禁发预检保存基线时因 `Path.open(opener=...)` 用法错误退出，尚未构建或启动任何 harness。按共同前提失败规则停止全部：`M7-P1V2-2026-10-19` **0/1、未执行并封闭**；`M7-P2-2026-10-19` **0/8、未执行并封闭**；`M7-INJ-2026-10-19` **0/1、未执行并封闭**。本轮合计发送 **0**，未用 **10** 次分别封存；usage／费用未知，后台额外触发 0。未修正续跑、不改判旧 0/10 与 1/10 批次；非空提炼、phase2 工具链及注入用户轮仍缺真实证据，M8/M9 未实施、V1 未完成。详情见 [M7 实施记录 §11.11](../records/2026-10-19-013-m7-implementation.md#1111-方案-a-执行轮共用预检本地错误三批未执行并封闭2026-10-19)。

**2026-10-19 控制层离线修复轮**：原 CLI 的 `Path.open(opener=...)` 异常已复现并修复，完整入口还检出并修复复制二进制丢失执行位的问题。最终离线回归 **74/0/0**（原计划59项此次实跑、额外上限1、完整控制场景6、拒绝入口3、质量证明5），普通分支离线类型检查通过；共同失败全停、单批失败后续独立批继续、1/8/1与总10及不借额度均有完整入口证据。生产代码／v2夹具／供应商参数／通过标准未改，真实调用0；旧批封存不变，三项真实供应商行为仍待新的CFIX三批授权，非空覆盖未更新，M8/M9未实施、V1未完成。最终版本指纹、报告及新授权包见 [M7 实施记录 §11.12](../records/2026-10-19-013-m7-implementation.md#1112-分批控制层离线修复与完整入口排练2026-10-19真实请求-0)。

**2026-10-19 CFIX 三批真实补验**：用户另行明确授权后，沿用已排练入口，禁发预检 **60/0/0**、同源码 live 构建通过；固定 `deepseek / deepseek-flash`、`openai-chat`、原唯一接口与空参数。phase1-v2 **1/1**：`SucceededWithOutput`、恰一行非空合同字段、job done／水位提交；phase2 **6/8**：三工具共7次实际成功、两产物／`v1`、正常终文、SQL提交及既定Git基线合同通过；注入 **1/1**：唯一system尾段、正常COBALT-17回复、自动记忆不入史。三项分别达到目标并封闭，合计 **8/10**，phase2未用2次封存；usage **97,660输入＋6,120输出＝103,780 tokens**，费用未知、内部重试0、后台额外触发0。新增该合成样本的非空提炼覆盖，三个独立夹具不构成同一数据链端到端通过，不证明角色语义等价；旧批原判及封存状态保留。临时授权与运行资源已清理，生产代码未改、未重跑workspace全量；证据经用户批准交付——归档提交 `9e32a6347ae96d9ac2ff14d2655bdaf784d92fd9`（656 路径逐文件暂存，指纹与脱敏核验通过）快进合并 main，验证分支随后安全删除，见 [M7 实施记录 §11.13](../records/2026-10-19-013-m7-implementation.md#1113-cfix-三批独立真实补验结果)。其他供应商／模型／平台及未执行分支仍未验证；M8/M9未实施、V1未完成。

- **目标**：D2 触发落地——**带输入的新用户轮成功启动时**（`run_tool_turn` 进入、输入入史后）spawn 后台两阶段（jobs 互斥去重）；`/memory-status`、`/memory-reset`（D9）；退出语义（不等待后台）。
- **输入**：驱动层（`tui_host.rs`/`main.rs`）轮启动位点。
- **输出**：接线＋命令；后台任务日志不污染 TUI。
- **涉及模块**：`tui_host.rs`、`main.rs`、`memory/runner.rs`。
- **先失败的测试**：触发条件（带输入新轮触发；本项目**无追加输入形态**——排队输入按新轮重放应照常触发，D2 形态差异登记）；后台失败不影响主循环；重置清生成记忆保会话（A31）；PTY e2e：开/关开关两态、命令往返。
- **验收命令**：`cargo test --workspace`＋`cargo test --test e2e_acceptance`。
- **完成条件**：默认关闭下行为与现状完全一致（零回归）；开启后闭环可观察。

**实施前确认（2026-10-19 准备轮；本段「本轮」＝该准备轮，不代表实施状态）**：

- **代码事实（逐项核实，2026-10-19 源码）**：
  - 两驱动轮入口：行式 `main.rs::cmd_chat`→`turn::run_tool_turn`（默认 `ToolMode::User`，M6 的 `UserWithMemory` 尚未接）；TUI `tui_host.rs::run_turn_inner`（独立线程）同位点。「轮成功启动」判定位点＝route＋origin 门通过之后（门失败轮未启动，不触发）。
  - 命令分发三处接线：行式 `handle_input`＋TUI `run_command`（现均 /models、/refresh、/save、/resume、/sessions 五命令）＋vendored `src/tui/src/slash_command.rs` 枚举 008-ADAPT 本地命令区（加变体有先例）。
  - 退出路径：行式主循环 break→`flush_final`→进程结束；TUI `run_tui` 返回→`set_cancel`→`flush_final_session`→`exit`。后台线程不被 join＝「进程退出后任务不保证完成」的天然承载（手册 §7.1 同义）。
  - M6 注入生产参数：记忆根＝`catalog::user_dir().join("memories")`；`MemoriesConfig` 生产来源＝manual.json `memories` 段（catalog `Manual` 结构字段；`FinalDict` 不含该段，取用走 manual 装载面）。
  - threads 登记写入侧缺位：`MemoryStore::register_thread(thread_id, updated_at_ms)`（M1 落地）无调用方。thread_id＝当前 auto 会话文件 stem；`Session::State::Writing{_path}` 持有路径但**无公开 stem 访问器**（M7 需补最小访问器）。`session::list_sessions` 只返回名称＋字节数（无 mtime）——`updated_at_ms` 必须由会话写入侧推进（入史点回调后 upsert，毫秒、随每次落盘推进，手册 §7.4），不能从目录枚举误取。
  - phase2 `resolve_thread`（M5 注入合成解析器）待换真实水合：`rollout_path`＝`sessions/<thread_id>.jsonl`、`cwd`＝当前进程目录、`git_branch`＝None（M3 登记差异延续；M5 测试合成解析器同值先例）。
  - 上游对照（`@ce0323f2`）：`memories/write/src/start.rs`＝后台门（临时会话/feature/非根 Agent/store 可用/layout/配额）→ensure_layout→seed extension instructions→prune→phase1→phase2（tokio::spawn，不阻塞轮）；`control.rs::clear_memory_roots_contents`＝清记忆根内容、保留根目录、拒绝符号链接根；手册 §7.1 触发三条件（有输入/实际启动一轮非追加/主环境就绪）、§7.3 候选表、§7.4 单位区分、§12.7 重置两面（库＋生成目录）且**原始会话保留**。本地 `memories.sqlite` 在记忆根内（M1），清根内容＝文件面＋状态面一次达成、sessions 目录不动。
- **批准边界（D2/D9 用户已确认，2026-10-19；准备轮决策包原文保留于下，状态由「待批准」更新为已确认）**：
  - **D2 触发**：每个**带输入、通过 route/origin 门并成功启动**的新用户轮触发一次后台管线（stage1→phase2 顺序）；**先检查 `enabled`**（关闭态零 spawn、零目录/库创建），复用 jobs 领取与 phase2 全局锁协调互斥（重复 spawn 无害）；**主轮失败或取消不撤销已启动的后台管线**；后台自身按既有失败、重试、租约与所有权契约处理；退出不等待。**费用知情**：`enabled=true` 且存在满足窗口的候选时，后台管线将发起真实模型调用（stage1 提炼＋phase2 合并，D6 默认当前 selected_model）；本地无配额接口（D8 跳过＋登记），节流仅由 6h 闲置/10 天窗口/冷却/重试承载。依据：手册 §7.1＋上游 `start.rs` 原序＋M1/M2/M5 已交付领取原语。修改范围＝两驱动触发位点＋新 `memory/runner.rs`；验收边界＝触发条件矩阵、后台失败不影响主循环、默认关闭零回归。
  - **D9 命令面**：`/memory-status`（状态观察：开关状态、记忆根路径、`memory_summary.md` 存在性、库内 stage1 产物/jobs 计数）＋`/memory-reset confirm`（**明确确认参数后**清理生成记忆、保留原始会话；**缺少 `confirm` 不执行清理**；保留根目录，符号链接及清理边界按既定契约——清记忆根内容含库文件、拒绝符号链接根、不动 sessions 目录，A31）。**重置失效边界（2026-10-19 追加确认）**：confirm 成功返回后，重置前启动的旧任务不得再回写生成记忆或重建基线（Markdown/状态库/Git 基线三面）；之后的新用户轮仍可按开关重新生成（不得靠永久禁用或要求重启满足负向约束）。依据：手册 §12.7＋上游 `control.rs`。修改范围＝三处命令接线（行式 `handle_input`、TUI `run_command`、vendored `slash_command.rs` 枚举）＋重置实现；验收边界＝A31、命令往返、两驱动一致。
  - **真实供应商补验不在本次批准内**：M3/D4 登记的供应商实测继续标待验证；代码实施与离线验收以 mock 推进，不得用 mock 冒充真实供应商验证。
- **实现细节（自行核实，不上升用户决策）**：runner 编排序与后台静默形态（日志不污染 TUI）；`memory_mode` 固定 'enabled'（G6 最小表默认，本地无 `generate_memories` 配置面，差异已登记）；seed extension instructions（`extensions/ad_hoc/instructions.md`，create_new 已存在跳过）推荐随 M7 管线保真落地、模板从固定上游拷贝；prune（30 天保留）不跑——M9 未实施，如实登记；D8 配额跳过、D10 顺序处理按既有登记执行。
- **验证范围（计划，不虚报）**：mock 可完成——触发条件矩阵（含排队输入重放）、threads 登记写入/推进/恢复切换、两驱动 M6 接线（开态请求含记忆段/关态逐字节同现状/轮内快照/恢复重读/不入史）、runner 编排与互斥、后台失败不影响主循环、命令面与 A31、PTY e2e 开关两态＋命令往返。**按契约须真实调用供应商**（M3/D4 登记的「供应商实测归 M7」）：phase1 结构化输出真实返回可解析性、phase2 合并请求、注入开态用户轮（唯一 system＋尾段记忆块）的供应商接受度——最小范围各 1–2 次固定输入真实调用、留请求/响应证据；离线 mock 回归门禁不变，真实调用为补充验证。尚缺条件：用户对真实调用的授权（成本）与已配置可用的 provider/凭证（不输出凭据）；该环境依赖不构成代码实施阻断。
- **验收命令（勘误，2026-10-19；工作目录 `/workspace`）**：原 `cargo test --workspace`＋`cargo test --test e2e_acceptance`——前者缺 `--no-fail-fast`（AGENTS.md §7 门禁为 `cargo check && cargo test --workspace --no-fail-fast`），后者在 workspace 根无 `-p` 虽可经默认成员解析、按 M4–M6 勘误先例显式化。拟新增目标 `src/app/tests/memory_runner.rs` **尚不存在**：先 `cargo test -p terminal-chat --test memory_runner -- --list` 核对非零清单（仅列举不算通过），再组合 `cargo test -p terminal-chat --test memory_runner --test memory_inject --test e2e_acceptance`；最终完整门禁仍为 `cargo check`、`cargo test --workspace --no-fail-fast`。
- **准备状态（2026-10-19 分支准备轮更新）**：D2/D9 已确认，普通分支 `feat/013-memory-m7` 已从实际 main @ `80f777898cf24a74a9669c478da9e8d7250b7029` 同点建立（HEAD＝main＝merge-base、领先/落后 0/0、单工作树、未创建 worktree）；**具备直接接收 M7 实施 `/goal` 的条件**。实际基线与 M6 交付树的差异仅为 4 笔文档/配置/垃圾文件清理提交（无生产代码变化），既有 2944/0/3 不自动沿用——实施阶段以实际基线跑基线全量后再进开发。本轮停在准备阶段，不自动开始接线，不再请求 D2/D9 批准。

### M8 引用反馈与用户便条（§18.6-9；**已交付（2026-10-19 用户批准后快进合并 main @ `71e370e6`，D12-A 批准边界内）**）

- **目标**：解析回复中 `<oai-mem-citation>` 的 `rollout_ids`→`usage_count`/`last_usage` 更新；便条＝模型按指引经通用工具写入 `extensions/ad_hoc/notes/`（文件名/不覆盖规则由指引＋写入门禁约束）。
- **涉及模块**：`memory/feedback.rs`（上游 `memories/read/src/{citations.rs,usage.rs}` 直接移植：`oai-mem-citation` 解析＋使用分类，A27/A28 锚＝上游同文件测试移植）、注入指引（M6 模板已含引用格式）；便条写入＝主会话现有工具（exec/apply_patch，§12.1 读指引引导），文件名/不覆盖规则在写入门禁层校验。
- **先失败的测试**：有效引用计数＋缺失 ID 忽略（A27）；仅工具打开不计数（A28）；便条空正文/非法名/重名拒绝且不覆盖（A25）；便条存在时合并输入含变化。
- **验收命令**：`cargo test --workspace memory`。
- **完成条件**：读取指标与来源使用计数严格分离（§12.3）。

**准备状态（2026-10-19 M8 实施前确认轮）**：实际基线 main @ `1ea1a3bf78c7a54527bf2cbdfe0af44ae13bd3f3`（＝预期 `1a45d5d`＋1 笔并发提交 `1ea1a3b` 证据精简归类——仅 docs/ 证据与引用同步、零生产代码，对 M8 无影响）；工作区干净、暂存区/stash 空、单工作树，`feat/013-memory-m8` 未占用（**本轮存在真实待决项 D12，分支暂不创建**）。本地源码事实：尚无 `memory/feedback.rs`；`store.rs` 的 `usage_count`/`last_usage` 列已在 `get_phase2_input_selection` 被读取（COALESCE 语义与上游一致）但**无写入路径**；`read_path.md`（M6）已含引用格式与 §12.1 便条指引，`extensions_ad_hoc_instructions.md` 已由 runner 播种（M7），`consolidation.md`（M5）已含 extensions 消费占位符；便条经 G8 git 基线 diff（含未跟踪）进入合并输入的通路已就绪。固定上游 `@ce0323f2` 网页核对：`memories/read/src/citations.rs`（85 行）＝解析 `<citation_entries>`＋`<rollout_ids>`（兼容旧 `<thread_ids>`）、去重、双空返 None、非法 ID 静默丢弃（A27 锚＝同目录测试移植）；`memories/read/src/usage.rs`（75 行）＝shell 读取分类（5 类＋版本），仅经 `core/src/memory_usage.rs` 发遥测计数、**从不写 usage_count**（A28 锚）；`state` 运行时 `record_stage1_output_usage`＝逐 ID 单立即事务 `UPDATE stage1_outputs SET usage_count = COALESCE(usage_count, 0) + 1, last_usage = ? WHERE thread_id = ?`，缺行忽略并返回受影响行数；A25 校验原文＝V2 专用工具 `ext/memories/src/tools/ad_hoc_note.rs`（正则＋长度 24–128＋正文 ≥1 字符＋后端新建）。分类：D1/D3/D4/D5/G8 及既有 M5–M7 接口＝已批准直接复用；feedback.rs 与测试移植、store 计数函数、turn 完成位点 best-effort 接线（沿上游缺行忽略语义、Clock 注入可控时间）＝按既有契约的实现细节；**真实待决＝仅 D12 便条写入门禁落点（§5 决策包，2026-10-19 边界核查轮已按代码依据修正：便条入合并模型今日唯一实际路径＝G8 差异文本〔探针实证原文进入，含非法名〕，extensions 占位符 V1 恒空，mem_read 为差异过滤拦不住的旁路；「不覆盖」仅 apply_patch Add 固有，Update/Delete/exec 不保证；推荐 A＝差异级消费过滤〔拟议行为〕，不扩展通用工具）**。验收命令勘误：`cargo test --workspace memory` 实测 `--list` 命中 33 个既有用例（非空跑；本轮实列，非测试通过），但 memory_phase2/memory_runner/e2e 中名字不含 `memory` 的用例会被漏掉——M8 定向检查须另含当轮新增模块所在套件，最终验收按 AGENTS.md §7 全量 `cargo check && cargo test --workspace --no-fail-fast`。拟新增 RED 用例（A27 有效计数＋缺 ID 忽略、A28 工具打开不计数、A25 门禁四规则、便条存在→合并输入含变化、turn 接线真实记录）**尚不存在、待实施后实跑**。本轮未实施功能、未调用真实模型、未运行 Cargo 全量门禁（仅 `--list` 核对过滤器）。

**实施状态（2026-10-19 M8 实施轮，D12-A 批准边界内）**：上段为准备轮历史（「本轮未实施」以本段为准更新）。落地＝`src/app/src/memory/feedback.rs`（上游 `citations.rs` 解析/去重/非法 ID 语义＋`usage.rs` 路径分类观测面＋本地 `auto-` 命名空间形状检查＋`after_user_turn` best-effort 接线）＋`store.rs::record_stage1_output_usage`（上游 SQL 逐字移植：单立即事务逐 ID `usage_count+1`/`last_usage=now`〔秒〕、缺行忽略、返回受影响行数）＋`main.rs`/`tui_host.rs` 两驱动用户轮完成位点接线（行式先回显后计数；TUI 先流式渲染后计数，失败静默）＋`workspace.rs` 差异处便条过滤（D12-A：非法名/超长/子目录/空白正文不入状态列表与正文，两清单同源排除集重算；仅非法便条被过滤时落入既有 A17 无差异短路；盘上文件保留；exec/apply_patch/mem_tools 合同零改动；既有排除集/截断上限/基线/所有权语义不变）。上游 shell 脚本读取分类器未移植（无遥测消费方）——A28 语义由路径级分类＋引擎级「工具真实打开不计数」测试承载。测试 21 例全部实跑：单测 13（feedback 8＋store 2＋workspace 3）＋集成 6（memory_feedback 3〔真实 turn＋真实 exec 读文件＋SQLite 副作用〕＋memory_phase2 3〔差异过滤/混合/非法单独短路〕）＋e2e S89/S90（TUI PTY 与行式管道两真实驱动入口→引用→SQLite 计数恰 +1）。RED：feedback 全 8 与 e2e 双例在桩实现下断言失败（实现前实跑）；store/workspace 过滤实现先于首跑落地（首跑失败为测试侧基线时序/冷却笔误，修正后绿）——错误检测以变异证明补强：m1 过滤禁用→2 用例失败、m2 去 last_usage→2 失败、m3 TUI 接线移除→S89 失败、m4 去重移除→2 失败，全部恢复后复绿；`m8_tool_read_without_citation_does_not_count` 为守卫型用例，补验轮以变异 m5（工具执行路径错误接入计数）证明其可检出、恢复后复绿；同轮新增 `m8_mem_tools_bypass_still_discovers_and_reads_invalid_notes`（D12-A 批准②显式断言：同一临时根内差异面过滤与 mem_list/mem_read 旁路两面互证）。门禁：`cargo check` exit 0；全量 `cargo test --workspace --no-fail-fast` **2997/0/3**（基线 2975＋22 含补验轮旁路用例，exit 0）；clippy/rustfmt 未运行（工具链组件下载失败，非跳过声明的既有安装态限制）。零真实供应商请求、零真实用户数据、全部临时目录＋mock。**M8 已实现并离线验证；真实供应商上的引用行为未实测（回复内容取决于模型，属既有 M7 供应商边界，不因本轮冒充）；M9 未实施、V1 未完成。**实施记录见 [M8 实施记录](../records/2026-10-19-013-m8-implementation.md)。

**交付记录（2026-10-19 用户批准后执行）**：上段「待批准」为交付前状态。12 个批准路径（8 修改＋4 新增）自 /tmp 清单逐文件显式暂存（无 `git add .`/`-A`/目录级），暂存集合与批准清单精确一致（4A＋8M）、`git diff --cached --check` 通过、保护项零进入、变异零残留与临时备份零纳入核验后，实施提交 **`71e370e61fdb26c012a7c3370f27682ea0a99cbf`**（父＝基线 `1ea1a3b`）在 `feat/013-memory-m8` 创建并 `git merge --ff-only` 快进合并 main——包含关系经 `merge-base --is-ancestor` 证实、合并后文件树与该提交逐字节一致（`git diff` 为空）；快进未引入变化，**引用交付前实测验证（`cargo check` exit 0；全量 2997/0/3 exit 0），不机械重跑、不追加真实调用**。D12-A 边界原文保留（自动差异过滤；mem_list/mem_read 旁路仍存在；通用写入不受此门禁约束；不覆盖仅 apply_patch Add）；历史记录、测试限制与变异说明（含守卫型用例与未单独变异项）全部保留；历史 manifest 与封批账本未改写。M9 未实施、V1 未完成。

### M9 保留清理与整链验收（§18.6-10；**已交付（2026-10-19 用户批准后快进合并 main @ `58f6020`）**）

- **目标**：30 天保留（已用按 last_usage/未用按 source_updated_at，批 200）＋扩展资源 14 天（无 resources 目录时 no-op）；全链 e2e；验收矩阵回填。
- **先失败的测试**：清理批上限与选择依据；资源 14 天边界（A32）；不把资源规则套到便条目录；整链：多会话→提炼→同步→合并→注入→引用→再合并的文件/状态/注入三面断言（A20 等）。
- **验收命令**：全量门禁＋e2e；矩阵映射表（§7）逐项回填实测。
- **原完成条件（保留原文，以下两项澄清其口径）**：V1 相关矩阵项全绿或如实标注缺口；与 Codex 行为偏离处全部登记（G1–G8＋D1–D11 裁决记录）。
- **M9 功能完成条件**：两类清理按下述精确边界接入生产管线；同一数据链的多会话→提炼→同步→合并→注入/检索→引用→过期再合并验收通过；关键用例取得 RED/恢复后 GREEN 证明；完整回归及适用静态检查有实际结果；§7 每项关联实测证据，跳过/范围外/暂缓/未验证分别登记。2026-10-19 实施轮已满足：新增两目标 13/13、同链与错误检测、完整回归 3010/0/3、适用静态检查及下方矩阵均有实证，登记为已实现并验证（2026-10-19 经用户批准交付）；详见 M9 实施记录。
- **V1 最终完成条件**：M1–M9 的必需功能与验收闭合、交付获准；适用矩阵项有可追溯实证，偏离上游与已接受边界完整登记（G1–G8＋D1–D12）。如实登记缺口本身不等于该项通过；既有范围排除和暂缓项不自动升级为 M9 阻塞，也不改写为已验证。M9 功能完成、用户批准交付和 V1 最终完成分别判断，当前 M9 功能条件已满足并经用户批准交付；V1 最终批准已于 2026-10-19 由用户给出，V1 按既定范围标记完成并交付——收口停点＝本规格与 M9 实施记录的文档收口提交，此后无既定实施动作；范围外/暂缓/未验证项不因收口改称通过。

**实施前确认（2026-10-19；本轮仅准备，以下验收均为待实施要求）**：

- **起点与保护**：已读取 `/workspace/AGENTS.md`，涉及目录无更近规则。实际分支/HEAD＝预期 `main @ ddeea8552ed8ef2b64585dbc22e481a82dc62d7d`；工作区/暂存区干净、stash 0、仅 `/workspace` 一棵工作树，无新增并发提交；M8 实施 `71e370e61fdb26c012a7c3370f27682ea0a99cbf` 与交付文档 `ddeea855…` 已在此基线。确认分支名未占用后建立文首普通分支；未实施、未暂存/提交/合并/推送/删除分支。沿用 M7/M8 保护清单：AGENTS.md、CLAUDE.md、.gitignore、compose、两条安装记录、宗旨文档既有删除状态、`.agents/`、`.env`、config、db；真实 db 及已知 sidecar 仅 lstat 元数据，不打开/遍历数据库，不访问真实 sessions/memories。已归档证据（含 `9e32a63` 原件、现行留存清单/封批账本）保留，不执行历史发送入口。本轮只改本规格。
- **输入、输出与最小复用面**：输入＝现有 `MemoriesConfig`（`max_unused_days` 默认 30、消费时钳制 0–365）、秒级 Clock、stage1 结果/任务/来源登记、记忆根中的扩展文件；输出＝有限批结果行删除与资源删除尝试，后续同步/差异/合并使过期来源退出生成文件。模块＝`memory/store.rs` 补上游清理方法、`runner.rs` 接数据库清理、`phase2.rs` 接资源清理，资源扫描函数按上游小范围移植（当前不存在）；复用 `storage_files.rs`/`workspace.rs`/`inject.rs`/`feedback.rs`/真实 turn 与 Session，零新增工具、配置或原始 sessions 清理。

**清理契约与源码依据（固定 `openai/codex@ce0323f2f2d3b25e82806cd555237da3db4e1897`）**：手册 §7.4、§12.4–12.6、§18.4–18.6、§19.2 已逐项对照；网页读取 cache miss 后通过固定 SHA 的 raw URL 取得源码，只读核对，未运行上游测试。

| 核对项 | 精确规则与接入约束 | 关键依据 |
|---|---|---|
| 结果行与批量 | `cutoff = now_seconds - max(max_unused_days, 0) × 86400`；仅 `selected_for_phase2 = 0 AND COALESCE(last_usage, source_updated_at) < cutoff` 删除。恰等于 cutoff 保留，phase2 选择相应使用 `>= cutoff`。是否用过由 last_usage 的 NULL 与否判定，不按 usage_count，也不取两时间的最大值。按上述 COALESCE、source_updated_at、thread_id 三键升序删最旧；每次后台启动只跑一批，调用上限 200，不循环清空；方法 limit=0 返回 0 | [上游 MemoryStore:397–438](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/state/src/runtime/memories.rs#L397)；本地 `store.rs:1106` 已有来源选择但无 prune 方法；`config.rs:59/92` 已有配置及钳制 |
| 保护与运行中任务 | 清理只执行单条原子 DELETE 并返回受影响行数；保留 threads、jobs/成功水位、consolidation_progress，不直接改 Markdown/Git（库及 DELETE journal 沿 G8 排除）、不另行 enqueue。selected 保护的是**上次成功合并来源**，并非所有在途快照；上游 SQL 无 jobs.status 过滤、无先领取 phase2 锁的要求，不自行追加隐含运行中保护。沿用现有租约/令牌/心跳；新增文件删除同样纳入既有根共享锁＋任务 epoch 检查，reset 成功后旧任务不能删新代资源 | 同上 SQL；本地 `store.rs:723` 成功事务按精确源版本更新 selected；`runner.rs::lock_shared/read_epoch`、`phase2.rs::run_phase2_consolidation` 既有重置代际临界区 |
| 数据库清理触发 | D2 既有 enabled/route/origin 门后启动；store/layout/seed 后、候选领取及 phase1 前执行 prune；清理失败 best-effort，不因失败中止后续两阶段，不消费模型额度；退出不等待等既有语义不变 | [上游 start:59–91](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/start.rs#L59)＋[phase1:95–117](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/phase1.rs#L95)；本地 `runner.rs:255` 当前明确跳过 prune，须接入 |
| 14 天资源 | 仅 `extensions/` 直接子目录、且 `instructions.md` 存在时，扫描其 `resources/` 直属常规文件；小写 `.md`、文件名前 19 字节按 `%Y-%m-%dT%H-%M-%S` 解析为 UTC，时间 `<= now - 14×86400` 尝试删除，等号在删除侧；不按 mtime、不递归、不套用便条命名正则。缺目录 no-op；无说明、不可解析、非 `.md`、子目录/非普通文件不由资源规则删除（既有 layout 仍会移除符号链接，外部目标不跟随）；I/O 失败跳过或继续，不虚称全清成功；不扫描 `ad_hoc/notes/` | [上游 extensions/prune:9–104](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/extensions/prune.rs#L9)＋[lib:43/87](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/lib.rs#L43)，`prune_tests.rs` 含恰等于边界删除断言 |
| 同步、基线与过期传播 | phase2 领取全局任务→prepare/ensure 原基线→选择→同步会话摘要/重建 raw→资源清理→diff；领取失败/运行中/冷却时资源清理不执行。过期但 selected 的行先保留在库；phase2 窗口选择排除它→M4 删除不再选择的摘要→diff 保留删除信号→合并只移除失去唯一支持的经验（A26）；成功后 selected 解除，下一次启动才可物理删行。无差异捷径不 reset；有变化则写后校验/确认所有权→reset→SQL 成功（既有非跨存储原子边界），不能先 reset 吃掉删除差异 | [上游 phase2:75–155/194–208](https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/codex-rs/memories/write/src/phase2.rs#L75)；本地 `phase2.rs:141–235`、`storage_files.rs:59/125`、`workspace.rs:357/678`；A26 内容判断仍由合并模型承担 |

**Clock 复用与测试隔离**：`store.rs:40/196` 的 `Clock = Arc<dyn Fn() -> i64 + Send + Sync>` 与 `MemoryStore::now()` 已可固定/步进；候选登记 `threads.updated_at_ms` 为毫秒，版本/last_usage/租期/清理为秒，沿用 `div_euclid(1000)`，资源解析显式 UTC。当前 runner 建库（`runner.rs:109`）、ThreadRegistrar（`:353`）与 `after_user_turn`（`feedback.rs:198`）生产壳直接用系统时钟，**尚无贯穿同链的注入入口**；实施时沿现有可测核心扩展最小时钟传递，生产默认仍 system_clock，不新增环境后门/命令/配置。复用 M5 SteppedClock 模式与闸门 mock，业务时间不依赖真实当前时间、sleep 或 mtime。所有测试使用独立临时 user/config/HOME/sessions/记忆根、固定 HTTP 响应、锁定依赖，结束清理；变异副本显式独享 `--target-dir`，不污染全局 target。

**同一数据链的验收方法与观察点（拟新增，当前尚不存在）**：只在链起点创建两份真实持久化的合成会话 A/B，各含唯一标记及共同经验标记；本轮活动会话 C 被排除。后续沿同一组来源 ID、源版本、路径和产物传递，禁止中途重播种 stage1 行、替换摘要或换独立根。mock 仅替代模型/HTTP 边界；每次响应前核对实际请求/真实工具结果中的前序标记，不能无条件返回一套与输入无关的成功产物。链内显式配置 `max_rollout_age_days=90`，保证末段被清理来源的同版本仍通过年龄/闲置门，按 jobs 成功水位跳过的断言不能靠年龄排除空跑。

| 环节 | 必须观察的同源证据 |
|---|---|
| 会话登记→候选 | 实际 Session 落盘后由 ThreadRegistrar 推进 `(stem, durable_len)` 与毫秒更新时间；再推进同一时钟至闲置边界，A/B 文件与登记交集被领取、C 排除；源版本恰为登记毫秒整除 1000。记录候选 ID/版本及源文件摘要 |
| 提炼→入库 | 实际请求包含 A/B 各自原始标记；严格解析后的 raw/summary 行对应相同 ID/版本，job done/成功水位一致；不能用直接造 stage1 行代替提炼 |
| 文件同步→首次合并 | 从这些行生成的 rollout_summaries/raw 含相同来源头/标记，稳定文本经路径归一后的黄金文件比对；真实 mem_read 返回这些文件/差异并进入工具续请求，再经 mem_write 写 MEMORY.md/summary；确认 SQL selected 精确版本、有效产物、临时差异移除及 Git 成功基线 |
| 注入→检索 | 下一真实用户轮经 UserWithMemory 读取**该次合并产物**，实际发送唯一 system 尾段含同一摘要；真实 exec/rg 查中同根来源，工具结果进入续请求；轮内快照/不入史和下一轮重读可观测，不手工替换为注入夹具 |
| 引用反馈 | 用户回复引用原始来源 B 的 ID；走实际完成位点/共享反馈核心，usage 恰 +1、last_usage 为同一受控时钟；仅打开文件不计数（A27/A28）；两驱动接线仍由真实 CLI/PTY e2e 检查 |
| 过期→再合并→后续批次 | 例：A/B 源更新时间 T0−7 天，T0 提炼并首合并；T0＋22 天通过上一步引用 B；T0＋24 天运行：A 已超过 30 天但 selected 行受保护，B 因 last_usage 保留；phase2 排除 A、文件/差异体现删除，经真实工具合并保留 B 支持的共同经验、删除 A 独有结论；后续新轮注入新版摘要。再触发时 A 已解除 selected 才删行，job 成功水位仍在，同版本不得重新提炼；原始 A/B sessions 字节不变。减少/归零等 A20 既有低层用例同时回归，不能替代此链 |

当前 `memory_runner::m7_pipeline_processes_real_candidate_end_to_end` 只覆盖单候选→两阶段→文件/部分请求断言，且手工登记、取系统时间；`memory_inject`、`memory_feedback` 各自夹具已验证局部机制，**它们的成功不能拼成上述同链通过**。确定性同链集成与真实 CLI/PTY 驱动覆盖分别报告，均不等于真实供应商同链；A26 的 mock 证明给定模型响应下的输入/文件/状态传播，不证明模型真实判断能力。

**RED 场景与有效命令（工作目录 `/workspace`；本轮未执行 Cargo）**：

- 拟新增 `src/app/tests/memory_retention.rs`、`src/app/tests/memory_lifecycle.rs` 两个集成目标及 M9 驱动用例，**当前均不存在**。前者覆盖 cutoff−1/恰等于/+1 秒、last_usage 优先于源时间（包括用过但最近使用仍旧）、selected 保护、201 条仅删 200 与三键排序、limit=0/配置 0、清理不删 jobs 水位与同版不再提炼；资源精确 14 天/前后 1 秒、无说明/无目录/非法前缀/非 md 保留、子目录不递归、链接不跟随且外部目标不变（管线仍沿用 M4 清链接）、便条保留、删除失败 best-effort。
- 后者覆盖上表同源链及编排守卫：清理调用缺失、资源删除晚于 diff、过期删除被提前 reset 隐藏、断开同步/注入/反馈、越过冷却/运行中、reset 闸门后旧任务删新代资源，均须被行为断言检出。保持断言不变，通过修复前对照或隔离副本暂时破坏 `<`/`<=`、selected 条件、COALESCE、批上限和上述接线取得 RED，恢复后 GREEN；不能把天然通过的守卫用例或编译失败当作行为错误检测证明。
- 当前已存在、可用于基线/关联回归的命令：`cargo test --locked --offline -p terminal-chat --test memory_store --test memory_candidates --test memory_phase1 --test memory_storage_files --test memory_phase2 --test memory_runner --test memory_inject --test memory_feedback`；真实进程/PTY 套件 `cargo test --locked --offline -p terminal-chat --test e2e_acceptance`。拟新增目标创建后才可运行 `cargo test --locked --offline -p terminal-chat --test memory_retention --test memory_lifecycle`；`-- --list` 只核对非零清单，不算执行通过。
- 最终完整门禁仍为 AGENTS.md §7 的 `cargo check && cargo test --workspace --no-fail-fast`（可分别加 `--locked --offline` 保持锁版本/离线）；定向过滤不能代替全量。实施前确认原有状态，完成后报告通过/失败/忽略及未运行数量；lint/格式/兼容性按实际适用与工具可用性报告，不沿用 M8 结果冒充本轮通过。本轮仅文档，`git diff --check` 已实跑 **exit 0**；保护清单摘要/元数据前后核对一致，已归档证据 229 个当前文件逐字节摘要一致。Cargo 测试、typecheck/lint、拟新增测试与上游测试本轮均未运行，不报测试通过；未运行用例数未重新枚举，M8 的 2997/0/3 仅为历史证据。

**真实证据、决定与停点**：

- M7 CFIX 的三项真实样本仍为三个独立根（非空提炼、合并工具链、注入用户轮），各自通过，实际请求 1/6/1、总 8，未用 2 次封存；[已交付摘要](../testing/evidence/013-m7/README.md)及原件/账本不改。它们不证明同一数据链端到端或 system/developer 语义等价。真实引用产生/反馈整链、真实保留传播、其他供应商/模型与 Windows 等保持未验证；新真实调用仍需另行授权，不沿用封存额度，本轮真实模型调用 0。
- **无阻塞**：保留/资源规则、触发和边界均有既有契约及固定源码，不新增用户可见取舍，无需新决策包，不重开 D1/D2/D3/D4/D5/D6/D9/G8 或 D12-A。D7/D8/D10/D11 保留现有登记和已交付落地，不冒称重新批准。D12-A 继续为差异层过滤、盘上保留、mem_list/mem_read 旁路仍可达、无通用写入拒绝、不覆盖仅 apply_patch Add；资源规则不扩大到便条。
- **必需与暂缓分离**：M9 必需为两类清理、上述确定性同链验收、真实 CLI/PTY 关联回归、矩阵实证回填与规定门禁；既有无配额接口的 A14 跳过、A29/A30 与专用工具范围排除、Windows/宿主专项等既有暂缓、真实供应商同链及角色语义未验证不自动升级为 M9 实施阻塞，亦不得改写为通过。M8 最终 2997/0/3 只作历史交接，非本轮重测。**普通分支就位，具备接收 M9 实施/goal 的条件；本轮不创建 goal、不开始实施，M9 未实施、V1 未完成。**

**实施状态（2026-10-19 M9 实施轮）**：以上“仅准备／尚不存在／未运行”为准备历史，以下为本轮实际结果；原要求及历史均保留。

- **已实现并验证**：`store.prune_stage1_outputs_for_retention` 逐字移植固定上游 SQL；runner seed 后/候选前、共享锁＋epoch 内每次只清一批 200；phase2 同步后/diff 前在同一锁域资源清理。默认 30 天 `< cutoff` 与资源 14 天 `<= cutoff` 两种等号保持。任务/水位、原始 sessions、便条、G8 基线与 D12-A 边界不变；不增加遗忘规则、配置或依赖。
- **同链已实跑**：新 `memory_retention` 7 例＋`memory_lifecycle` 6 例。其中同一临时根 A/B 真实 Session→登记/候选→严格提炼→Markdown 黄金→真实 mem_read/mem_write 合并→UserWithMemory 注入/真实 exec-rg→引用计数→selected 保护/真实选择淘汰→删除差异再合并→下一轮注入→下次物理删除→同源新版本再提炼。标记、ID/版本、请求、SQL、完整文件集合/Git 一路关联；不是拼接 M6/M7/M8 独立成功结果。
- **RED/GREEN 与补强**：新目标先 list 非零再实跑；缺实现的 DB/资源及缺 runner 接线均有断言失败，修复后通过。18 项变异中 17 个非等价错误实测检出，COALESCE→IFNULL 一项等价通过；编译失败的无效初尝试另记、不算检出。两轴审查的 rg 宿主配置隔离/完整投影黄金缺口已修复、错误检测及复核闭合。清理失败继续、单批推进、reset 两闸门和模型期间不持文件锁有真实副作用断言。
- **实际门禁**：改前受影响基线 416/0/0；首轮完整回归 3008/2/3（S89/S90 旧日期未选中夹具被新清理删除）如实保留。经真实成功事务把独立驱动夹具设为上次合并已选中来源、保留引用恰 +1 断言后，完整 E2E 106/0/0；最终 `cargo check` exit 0、`cargo test --workspace --no-fail-fast` exit 0，**49 套件 3010通过/0失败/3既有忽略、0过滤、必需检查未运行 0**。复用临时配套工具 Clippy exit 0（既有告警保留）、新文件 scoped rustfmt check exit 0；未宣称全库格式或上游测试通过。
- **保护与停点**：HEAD/main/merge-base 仍为文首完整基线，暂存区/stash 空，单工作树；15 项保护中 14 项相对准备快照一致，AGENTS.md 的非本轮写入本地说明修改已重读保留，229 个既有归档摘要一致。M9 功能条件满足并经用户批准交付（见下方交付记录）；V1 最终交付批准尚缺，不标已完成。真实模型调用 0；M7 三项独立真实样本、真实同链未验证、Windows/宿主暂缓及 A14/A29/A30 分类保持。无新增决定或阻塞。

完整来源、文件清单、逐环节观察、失败/修复和命令日志索引见 [M9 实施记录](../records/2026-10-19-013-m9-implementation.md)；安全适用性与实测见 [M9 安全报告](../security/reports/2026-10-19-memory-m9-security.md)。实施轮停在此处（未暂存、提交、合并、推送或删除分支）；2026-10-19 用户批准后交付，见下方交付记录与实施记录 §9。

**交付记录（2026-10-19 用户批准后执行）**：上段「停在候选审批」为交付前状态。25 个批准路径（7 修改＋18 新增，逐项与实施记录 §7 清单一致）自 /tmp 清单逐文件显式暂存（无 `git add .`/`-A`/目录级暂存），暂存集合与批准清单精确一致（18A＋7M）、25 个暂存内容指纹与 `candidate-files.json` 登记逐一相符、保护项零进入、`git diff --cached --check` 通过；18 项变异均在独立源码副本运行、工作仓库零残留，临时备份未纳入；22 份源码/测试/黄金与终验（3010/0/3）态逐字节一致。实施提交 **`58f6020b3a21fbdd88cb56f193290045df6b4187`**（`feat: 013-M9 保留清理与整链验收`，父＝基线 `ddeea855`）在 `feat/013-memory-m9` 创建并 `git merge --ff-only` 快进合并 main（ddeea85→58f6020）——包含关系经 `merge-base --is-ancestor` 证实、合并后提交树与该提交一致（`24e4d43`）；快进未引入变化，**引用交付前实测验证（`cargo check` exit 0；全量 `cargo test --workspace --no-fail-fast` exit 0：3010 通过/0 失败/3 既有忽略/0 过滤），不机械重跑、不追加真实调用**。精确清理边界（30 天 `< cutoff` 与资源 14 天 `<= cutoff` 两种等号、批 200、selected 保护）、变异排除（17 检出＋1 等价如实登记）、安全结论与未验证项（真实供应商同链、Windows/宿主等）全部保留；D12-A 及既有供应商证据（M7 三项独立样本）范围不变；历史 manifest、封批账本与已归档证据未改写；并发 AGENTS.md 本地修改原样保留、排除在两笔提交之外。V1 最终完成标记暂不翻转，留待最终收口确认（同日经用户最终批准后收口，见下段）。

**V1 最终收口（2026-10-19 用户最终批准）**：用户于 2026-10-19 批准 V1 最终交付。对照本节「V1 最终完成条件」逐项核验：M1–M9 必需功能与验收闭合（§6 各阶段标题均「已交付」，实施提交链 `cf64a80`/`3d4e80e`/`63bff52`/`4cd9f19`/`96e5908`/`2aa0002`/`67b9461`/`71e370e`/`58f6020`）；适用矩阵项有可追溯实证（§7 逐项回填，A14 按 D8 跳过、A29–A30 范围外按登记态保留）；偏离上游与已接受边界完整登记（G1–G8＋D1–D12，§5/§5.1 及各阶段记录）。最终批准缺口闭合，**V1 按既定范围标记为已完成并交付**。边界保持原状态、均不改称通过：A14 无配额接口按既有决定跳过；A29–A30（双写/V2）范围外；真实供应商同链、system/developer 角色语义等价、其他供应商/模型仍未验证；Windows 及宿主专项按既有暂缓；D12-A 仅自动差异过滤，mem_list/mem_read 旁路及通用写入能力保留。最终验证引用 M9 交付实测（`cargo check` exit 0；`cargo test --workspace --no-fail-fast` exit 0：3010 通过/0 失败/3 既有忽略/0 过滤），本次仅文档收口、不追加测试或真实调用；历史 manifest、封批账本与已归档证据未改写；并发 AGENTS.md 修改继续保留在提交之外。后续新范围（V2、专用工具、真实供应商验证等）需另行立项与授权。

**阶段依赖**：M1 → M2 → M3 → M4 → M5 →（M6 可与 M4/M5 并行开发，仅依赖 M1 目录约定）→ M7（需 M2–M6）→ M8（需 M1/M7）→ M9（需全部）。检索（参考 §18.6-8）无新代码，验证并入 M6/M9。

## 7. 验收矩阵映射（参考 §19.2 → 本计划）

| 矩阵项 | 归属 | 说明 |
|---|---|---|
| A01–A03（开关/空摘要/注入） | M6 | M6 已交付；M9 实跑 `memory_inject` 18/18（开关、空摘要、预算、轮缓存、恢复、不入史）；`memory_lifecycle::same_chain_registration_to_retention_remerge_and_injection` 补同一合并产物→唯一 system/轮内同快照→下一轮新摘要。见 [M9 记录 §3/§6](../records/2026-10-19-013-m9-implementation.md) |
| A04–A08（候选筛选/已处理/重提炼） | M2/M3 | M9 实跑 `memory_candidates` 18/18（a04/a05/a06/a07/a08）；同链真实登记 A/B、排除 C、清理后同版本不提炼、Session 追加的新版本重新提炼；`memory_retention::retention_batch_order_watermarks_and_new_version` 单独核实水位。见 M9 记录 §3/§4/§6 |
| A09–A10（并发所有权） | M1/M3 | M9 实跑 `memory_store` 21/21 的双连接/租约/旧令牌用例及 `memory_phase1::m3_a09_stale_owner_cannot_commit_after_takeover`；单进程内以两线程/两锁持有者模拟，不改称多机验证 |
| A11–A13（schema/空结果/秘密） | M3/M4 | M9 实跑 `memory_phase1` 19/19（schema、空输出、脱敏、真实 I/O 故障）＋`memory_storage_files` 23/23；A13 导出链为 `m4_a13_m3_parse_store_export_redacts_body_and_exact_slug_projection`。同链仅补实际严格解析→存储→投影，不替代秘密边界用例 |
| A14（配额） | — | 无配额接口→跳过并登记（D8） |
| A15（首次使用） | M2 | M9 实跑 `memory_candidates::m2_a15_first_use_yields_no_candidates` 及 `memory_runner::m7_enabled_pipeline_initializes_root_and_seeds`，首次空集合行为保留 |
| A16–A18（冷却/无变化捷径） | M5 | M9 实跑 `memory_phase2` 的 a16 两例/a17/a18；`memory_lifecycle::pipeline_prunes_one_batch_even_while_phase2_is_running` 证明 DB 清理仍执行、资源清理不越过全局运行锁；同链后续空 diff 不再模型调用 |
| A19–A20（来源选择/文件同步） | M5/M4 | M9 实跑 `memory_phase2::a19_selection_ordering_window_threads_and_paging`、`memory_storage_files` 新增/替换/减少/归零黄金；同链首次/过期/新版本三次 raw＋完整 rollout 文件集合逐字黄金，selected 精确源版本及 Git 单提交/clean 互证 |
| A21–A22（输出校验/所有权） | M5 | M9 实跑 `memory_phase2::a21_invalid_output_fails_without_reset_and_retries`、`a22_lost_ownership_after_validation_does_nothing` 与失败原因/部分修改不回滚用例；`memory_lifecycle` 两个 reset 闸门补代际失效与模型等待不持文件锁 |
| A23–A24（专用工具搜索/边界） | M6（检索）＋M5（受限工具边界） | 专用工具仍排除。M9 实跑 `memory_inject` 的真实 rg 命中/未命中续请求；同链以隔离 HOME/XDG、`env -i`/`--no-config` 的真实 exec/rg 检索该次合并产物。exec 无路径约束，不虚称受限；A24 由 `memory_phase2` 的 mem_read/write/list 越界/符号链接用例承载，D3 不变 |
| A25（便条规则） | M8 | D12-A 已批准的差异层过滤保持：M9 实跑 `memory_phase2` 三个 m8 过滤用例及 `memory_feedback::m8_mem_tools_bypass_still_discovers_and_reads_invalid_notes`；盘上保留、旁路仍可读、不覆盖仅 apply_patch Add。资源清理不扫描便条，见 `memory_retention::resources_filename_boundary_instructions_and_notes` |
| A26（来源删除保留他源经验） | M5/M9 | M9 同链中 A 过期→真实选择只留 B→删除差异进实际 mem_read 续请求→mem_write 删除 A 独有结论/保留 B 支持的共同经验→下一轮读取新摘要。证明确定性 mock 下的传播，不证明真实模型语义判断；真实同链仍未验证。见 M9 记录 §3 |
| A27–A28（引用反馈/指标分离） | M8 | M9 实跑 `memory_feedback` 4/4、feedback/store 单测、S89/S90 两真实驱动；同链重复引用 B 恰 +1、last_usage 同 Clock、实际工具打开前不计数。两驱动夹具按上次合并已选中来源修正，原计数断言保留（M9 记录 §5）；真实供应商引用行为仍未验证 |
| A29–A30（双写/V2） | **排除** | 范围外 |
| A31（重置） | M7 | M9 实跑 `memory_runner` 22/22 的重置/交错写/旧任务/新代凭证用例、e2e S82；新增 `memory_lifecycle` 的 selection→sync 闸门及模型等待闸门，断言新代资源保留、旧任务不重建库/Git/产物、sessions 不变。Windows 无 flock 实证仍暂缓 |
| A32（资源 14 天） | M9 | `memory_retention` 7/7：UTC 前后 1 秒/等号、说明存在、非法名/非 md/直属范围、链接根外、便条、真实删除失败继续；`memory_lifecycle::resource_deletion_reaches_merge_before_baseline_reset` 与同链证明资源删除进 diff→合并→Git。见 M9 记录 §2–§6 |

## 8. 边界与风险

- 时间可控（注入时钟/固定时间戳）、离线 mock、不虚报未运行项——沿 AGENTS.md §3–§5。
- 每阶段全量门禁；交互改动补 PTY e2e；默认关闭零回归是 M7 硬条件。
- 与参考文档不一致处记录差异不自行编造兼容行为（§18.7 范围条款）。
- 本计划本身不改动生产行为；实施前须先批准 §5 决策项（至少 D1–D5）。
