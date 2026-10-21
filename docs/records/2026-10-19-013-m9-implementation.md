# 013-M9 保留清理与整链验收

状态：**已交付（2026-10-19 用户批准后实施提交、快进合并并删除任务分支，见 §9）**。M9 功能完成条件已满足；V1 已于 2026-10-19 经用户最终批准，按既定范围完成并交付（见 §10）。实施轮没有暂存、提交、合并、推送或删除分支。

## 1. 起点、范围与来源

- 普通分支 `feat/013-memory-m9`；HEAD＝main＝merge-base＝`ddeea8552ed8ef2b64585dbc22e481a82dc62d7d`，合并目标 `main`。单工作树 `/workspace`，暂存区空，stash 0；无新增并发提交。准备轮的 013 修改保留，在原规格内续写，没有另建计划或 worktree。
- 已读适用 AGENTS.md、013 完整 M9/矩阵/决定、M8 交接及关联实现和测试。已沿 TDD、comprehensive-testing、Security Testing Gate、code-review 执行门禁。TDD/code-review 为既有全局 Skill；安全门按仓库代理说明由主代理执行，双轴审查按 code-review Skill 使用两个只读子代理。
- 固定上游仍为 `openai/codex@ce0323f2f2d3b25e82806cd555237da3db4e1897`；复用准备轮 `/tmp/013-m9-precheck.Hfq3hT0D/upstream-*.rs`，未重新抓取移动分支、未运行上游测试。参考手册仅用已指定的资料文件，不访问真实 sessions/memories。
- 本轮原始日志及保护/源码摘要：`/tmp/013-m9-implementation-htz3y2bg/`（下文日志名均相对此目录）。历史 2997/0/3 只作 M8 参照；本轮先实际跑受影响基线 **416/0/0，exit 0**（`baseline.log`），再开始实现。

保护核对保留原快照，不重新定义起点掩盖差异：15 项中，14 项摘要/元数据一致；AGENTS.md 相对准备快照出现非本轮工具写入的本地修改，仅将资料末行补为“缺失 SKILL/Agent 时按需下载到全局目录”。旧 SHA `5114d8ea…`、当前 SHA `a753ea2e…`，未提交内容的操作者不能由 Git 确定，已重读且未覆盖。229 个既有归档证据逐文件 SHA 一致，包括 M7 原件、封批账本与历史失败。`.env`、config、db 及已知 SQLite sidecar 仅 lstat，不读取内容或遍历真实数据库；未打开真实会话/记忆。末次结果见 `protection-git-final.json`。

## 2. 实现与精确边界

| 位点 | 实现与复用 |
|---|---|
| `memory/store.rs::prune_stage1_outputs_for_retention` | 复用既有 Clock/连接互斥/错误映射；上游单条 DELETE SQL 逐字移植（去 Rust 字符串外围空白后 SHA-256 `6a8089b2a4458aa20dda35a242c9abf923113d27af58f8bda891581f302789b0`）。不改 DDL、jobs、threads、成功水位或 consolidation_progress，不 enqueue |
| `memory/retention.rs` | 上游 `extensions/prune.rs` 的直接遍历及 UTC 前缀判断；适配 tokio fs→std fs、Utc 当前时间→已有 Clock 秒值、tracing→已净化告警回调。目录另以 symlink_metadata 检查保持既有不跟随链接边界；常量批量 200，不新增配置/依赖 |
| `memory/runner.rs` | seed 后、候选领取/提炼前，在根共享锁＋同 epoch 校验内仅调用一批 DB 清理；SQL 失败 best-effort，继续后续阶段。生产后台复用 `run_memory_pipeline_once` 核心，只替换模型边界/Clock；路由与 origin 门仍在既有驱动入口 |
| `memory/phase2.rs` | 领取全局任务→prepare/ensure→实际选择→共享锁内校验 epoch/同步 Markdown/资源清理→diff→模型→校验/所有权→reset→SQL 成功。共享锁获取失败按既有 failed_sync_workspace_inputs 记账，不能无锁删除；模型等待不持文件修改锁 |
| `ThreadRegistrar::with_clock` / `feedback::after_user_turn_with_clock` | 将同一可步进 Clock 贯穿验收；生产默认壳继续 system_clock，登记默认保留毫秒精度。不新增环境后门或用户命令；自动上下文仍不入史 |

数据库规则：配置默认 30 天，沿原消费处钳制 0–365；`cutoff = now_seconds - max(days,0)×86400`（饱和运算）。仅删 `selected_for_phase2=0 AND COALESCE(last_usage,source_updated_at)<cutoff`，**等号保留**；last_usage 非 NULL 即优先，不按 usage_count、不取最大值。三键 COALESCE/source_updated_at/thread_id 升序，单次最多 200，后续启动推进下一批；limit 0 返回 0。SQL 不额外加运行中任务过滤，保留既有任务/租约语义。

资源规则：只扫描 extensions 直属扩展目录，要求 `instructions.md` 存在（目录也满足上游存在性判断）；只处理其 resources 直属常规小写 `.md`，前 19 字节按 `%Y-%m-%dT%H-%M-%S` 解析 UTC。时间 **`<= now−14×86400` 尝试删除**，等号在删除侧；不看 mtime、不递归、不扫描便条或原始 sessions。非法前缀/缺目录/非 md/非普通文件跳过；遍历 next 返回错误时按上游结束该层循环，删除 NotFound 忽略，其他删除失败告警后继续。根、扩展目录、resources 和资源文件链接均不跟随；管线原 M4 layout 移除链接的边界保持。

传播不是立即抹去所有最终文字：上次合并选中的旧 A 启动时仍受保护；真实 phase2 窗口选择淘汰 A，M4 同步删对应摘要并重建 raw，删除信号进入 diff，合并后成功事务解除 A 的 selected；下一次启动才物理删 A 结果。Git 库/journal 排除、无差异不 reset、成功前 reset 的既有非跨存储原子边界均不变，没有另加遗忘规则。A26 的内容选择仍由模型承担。

## 3. 同一数据链观察证据

用例：`memory_lifecycle::same_chain_registration_to_retention_remerge_and_injection`。单个 Env 临时根、真实 Session 文件、真实 SQLite/Git/工具执行；只 mock 模型响应，响应前核对实际请求或 tool 消息中的上游标记，脚本耗尽/意外模型调用均断言。没有在链中重播种 stage1 或手改 selected。

固定 T0＝1700000000；A＝`auto-20231107T221320-4101`，源版本 1699395199；B＝`auto-20231107T221320-4102`，源版本 1699395200；C 为活动会话排除。来源标记分别 SESSION-A/B-M9，独有 A/B-ONLY-M9，共享 SHARED-M9。`max_rollout_age_days=90` 使后段同版本仍在年龄/闲置候选窗，不能靠年龄过滤伪证水位有效。

| 环节 | 本用例实际动作与断言 |
|---|---|
| 登记→候选 | Session.on_history 真实落盘，ThreadRegistrar 记录 durable_len/毫秒时间；runner 实际候选领取 A/B、排除 C，提炼顺序 B→A；源版本等于登记毫秒整除 1000 |
| 提炼→入库 | 两次请求分别必须含 SESSION-B/A 标记及对应 ID；返回严格 JSON 经真实解析/脱敏/存储；SQL raw、源版本、job done、last_success_watermark 对应同一来源 |
| 同步→首次合并 | 实际生成 raw 与两份 rollout；路径归一后对完整文件名集合/全文做黄金比对。真实 mem_read 读取 raw/diff，其 tool 结果含 A/B 标记和 ID 后才允许 mock 返回 mem_write。检查 MEMORY/summary 黄金、selected 精确源版本、Git 一提交且后续 diff clean |
| 注入/真实检索 | T0＋22 天在 UserWithMemory 用户轮读取刚合并的摘要；两次实际请求均首位且唯一 system、memory-context 恰一次、同轮 system 完全相同。真实 exec 执行绝对 rg：env -i、临时 HOME/XDG、LC_ALL=C/TZ=UTC、--no-config；真实命中 SHARED-M9 和 B ID 的 tool 消息进入续请求。临时恶意 RIPGREP_CONFIG_PATH 不生效 |
| 引用反馈 | 实际回复重复引用同一 B ID；工具读取后 usage/last_usage 仍 NULL。共享完成核心计数后恰为 `(1,1701900800)`，A 不变；实际会话文件没有自动 memory-context。两生产驱动完成位点另由 S89/S90 承载，不能将这两独立夹具计入本链 |
| 过期→真实再选择/合并 | T0＋24 天 A 源已超过 30 天，仍 selected 因而启动保护；B 因近期引用保留。真实 get_phase2_input_selection 仅 B；runner→phase2 的 mem_read tool 结果必须含 `-RAW-A-M9`、`-RESOURCE-A-M9`、A ID。实际 mem_write 后去 A 独有、留 B 支持的共享经验；A 行仍在但 selected 已由真实成功事务解除，B 仍选中 |
| 文件/Git→下一轮 | 过期后的 raw、完整 rollout 集合、MEMORY/summary 黄金一致，A 摘要和过期资源确已消失；Git HEAD 变化但仍只有一提交，diff clean；下一个用户轮实际 system 读取 retained summary 且无 A-ONLY-M9 |
| 下一批物理删除/同版本跳过 | T0＋24 天＋1 秒再次真实 runner：A 行消失，job/成功水位逐值不变，脚本零请求；A/B 原始 Session 逐字节不变。扫描仍能看到 A，排除“因年龄不候选”空跑 |
| 清理后新版本 | T0＋24 天＋2 秒向同一个 A Session 真实追加 SESSION-A-V2-M9，经 registrar 更新毫秒版本；推进 6 小时闲置后真实候选→提炼→同步→合并成功。SQL 新源版本/成功水位/selected 精确版本更新，三类文件新黄金一致，B 原始 Session 不变 |

黄金共 12 份，按固定输出契约编写：三阶段 MEMORY/summary 六份，A/B/新 A rollout 三份，三阶段 raw 三份。raw 以 JSON 字符串容器保存原样文本，解码后全文比较，保留真实尾部两个 LF；没有 trim 放宽或从失败输出直接接受快照。此链证明确定性 mock 下的控制/数据传播，不等于真实供应商同链，也不证明真实模型的遗忘判断质量。

## 4. RED/GREEN 与错误检测

两个新目标先 `-- --list` 确认非零（最初各 1 例，最终 7＋6），列举不计通过。`red-initial.log`：缺失清理实现时 DB 0≠2、精确 14 天资源仍存在，两例真实断言失败，exit 101；最小实现后 `green-initial.log` 两例通过。`red-runner-behavior.log`：同链前段执行后在末段物理删除断言失败；`red-startup-batch.log`：缺 runner prune 时 201 行未减，exit 101；接线后通过。`red-runner.log` 的首轮回调类型笔误为编译失败，明确不算行为 RED。

comprehensive-testing 以状态转移核查：未选中→删除、选中→本次保护→真实选择淘汰→下次删除、成功水位阻同版/允许新版、全局 running/cooldown 与 startup 清理分离、reset 新旧代隔离、模型等待期间可 reset。补强至 retention 7 例、lifecycle 6 例，含三键顺序、失败继续/下次恢复、201 单批、说明存在/UTC/非递归/链接/便条、真实权限失败、两个 reset 闸门。断言检查 SQL 状态、文件全文、工具请求、Git、副作用及目标路径执行；mock 不替代核心业务判断。

所有变异在 `mutation-src` 独立源码副本中运行，专属 `--target-dir mutation-target`，工作仓库源码未破坏。`mutations.json`/`mutation-summary.json`：**18 项，17 项非等价行为错误全部被不变断言检出（exit 101），1 项等价通过（exit 0）**：

| 变异组 | 实际错误检测 |
|---|---|
| 清理条件/顺序 | 去 selected 保护、DB `<` 改 `<=`、usage 优先错误、200 改 201、源时间排序反向，分别失败 |
| 传播接线 | 断 runner DB 清理、断 phase2 资源清理、断 Markdown 同步、断引用反馈、断注入，分别失败 |
| 差异/代际/锁 | 过早 reset 吃删除差异、资源等号改为保留、移除 phase2 epoch 核对、模型等待持文件锁，分别失败 |
| 审查补强 | 破坏 rollout 来源头、破坏 raw 版本字段、移除 rg 环境及配置隔离，分别失败 |
| 等价 | 两参数 COALESCE→IFNULL，SQLite 此处等价，retention 7 例通过，按等价排除，不冒称检出 |

注入断线首轮 bare None 触发 E0282，不计检测；改为 `None::<String>` 后真实执行并断言失败（`injection-disconnect-typed.log`）。每次变异恢复原文件，初组恢复后 13/13（`mutation-restored-green-final.log`）；审查补三项恢复后 lifecycle 6/6（`mutation-restored-after-review.log`）。最终完整 3010/0/3 在候选源码恢复后执行。

## 5. 审查与完整回归发现

### Standards

初审 P2：同链真实 rg 会继承宿主 RIPGREP_CONFIG_PATH。复用 M6 的隔离执行方式，临时 HOME/XDG、清空进程环境、显式 locale/TZ、--no-config；增加恶意临时配置并用去隔离变异证明失败。只读复核无未闭合硬问题或 smell。

### Spec

初审 P2：raw/rollout 只看标记不足以满足完整黄金契约；另要求同链观测唯一 system/同轮快照。已补 6 份投影黄金及实际请求断言，来源头/源版本变异均被检出。复核无未闭合缺失、错误实现或超范围行为。

完整回归首轮 **3008通过/2失败/3忽略，exit 101**（`cargo-workspace-first.log`，`final-results-first.json`），失败仅 S89/S90 QueryReturnedNoRows：M8 独立驱动夹具的 2023 年结果从未 selected，也未被使用，按新契约被启动清理。没有停掉清理或放宽 usage==Some(1)；改由真实全局任务领取/成功 API 把夹具设为上次成功合并选中来源，并断言 prune 返回 0。完整 E2E 复绿 **106/0/0**（`e2e-fixture-fix-green.log`），随后重新跑完整门禁。双轴定点复核认可前提修复，指出成功记录触发冷却使原“后台悬于 silent”注释失实，已修正；S85/S86 独立悬置退出验证保持。S89/S90 仍只证明各自驱动反馈，不改称同链。

双轴最终未闭合发现：Standards 0、Spec 0。安全门见 [安全报告](../security/reports/2026-10-19-memory-m9-security.md)，无本范围新增可复现安全缺陷；没有将此结论扩大为全系统渗透通过。

## 6. 实际命令与结果

工作目录 `/workspace`；最终环境 `CARGO_NET_OFFLINE=true TZ=UTC`。依赖由原 Cargo.lock 锁定，零真实模型调用；既有本地 HTTP mock/真实 CLI/PTY 照常运行。下表都是本轮实跑，未借用历史结果。

| 检查 | 实际结果 / 日志 |
|---|---|
| `cargo test --locked --offline -p terminal-chat --lib --test memory_store --test memory_candidates --test memory_phase1 --test memory_storage_files --test memory_phase2 --test memory_runner --test memory_inject --test memory_feedback --test e2e_acceptance`（改前） | 10 套件 416/0/0、0过滤，exit 0；baseline.log |
| `cargo test --locked --offline -p terminal-chat --test memory_retention --test memory_lifecycle -- --list` | 最终列表 7＋6，exit 0，仅存在性；final-new-targets-list.log |
| 同命令去掉 `-- --list` | 13/0/0，exit 0；green-hardened.log；审查修复后的同链单例 review-fixes-green.log 亦 exit 0，最终全量包含全部 13 例 |
| `cargo test --locked --offline -p terminal-chat --test e2e_acceptance`（夹具修复后） | 106/0/0，exit 0；e2e-fixture-fix-green.log |
| `cargo check`（最终） | exit 0；cargo-check.log |
| `cargo test --workspace --no-fail-fast`（最终） | **49 套件，3010通过/0失败/3既有忽略，0过滤，必需检查未运行 0**；exit 0，cargo-workspace.log、verification-counts.json |
| Clippy | 复用 M6 临时 1.99 配套工具，`cargo-clippy clippy --locked --offline -p terminal-chat --all-targets` exit 0（clippy.log）；最终新目标＋e2e 再跑 exit 0（clippy-final.log）。保留 vendored TUI/既有 app/test 告警，非零告警数不改称零告警 |
| rustfmt | 同一临时工具 `rustfmt --edition 2021 --config skip_children=true --check`，只检查新 retention.rs、common/memory_v1.rs、memory_retention.rs、memory_lifecycle.rs，exit 0（rustfmt.log）；未宣称全库格式检查通过 |
| 兼容性 | 无 DDL/Schema、持久化格式、依赖、模型工具注册面变更；保留原公共生产入口，仅加 Clock/模型边界可测核心。项目无适用的外部 API breaking 工具，本轮未执行此类工具 |

最终全量关联套件：app lib 142；memory_candidates 18、feedback 4、inject 18、lifecycle 6、phase1 19、phase2 43、retention 7、runner 22、storage_files 23、store 21；E2E 106，全均 0 失败/0 忽略。3 项忽略为既有 TUI Windows AltGr、owned-screen 子入口、caught-panic PTY 子入口（后两者由相应父用例启动，不私自取消 ignore）；没有把 Windows ConPTY 标成已验。验证后 22 个候选源码/测试/黄金字节摘要与验证开始时一致（verification-source-hashes.json）。最终 diff/保护检查见 §8。

## 7. 候选文件与完成判断

候选共 25 个路径（不含并发 AGENTS.md）：

- 生产 6：`src/app/src/memory/{store.rs,retention.rs,mod.rs,runner.rs,phase2.rs,feedback.rs}`。
- 测试 4：`src/app/tests/{memory_retention.rs,memory_lifecycle.rs,e2e_acceptance.rs}`、`src/app/tests/common/memory_v1.rs`。
- 黄金 12：`src/app/tests/fixtures/memory_lifecycle/` 中 initial/retained/renewed 的 MEMORY/summary 六份、initial_a.md/b.md/renewed_a.md 三份、initial_raw.json/retained_raw.json/renewed_raw.json 三份。
- 文档 3：现有 013 规格、本记录、M9 安全报告。没有更改 AGENTS/CLAUDE/compose/配置/真实数据/既有归档。

M9 功能条件：精确清理、生产接线/同步传播、同链、错误检测、规定门禁、矩阵回填均满足，登记为**已实现并验证**（后经用户批准交付，见 §9）。无新增用户可见决定或真实阻塞。V1 最终条件单独核对：M1–M8 已交付，M9 本地候选已完成；最终交付批准尚缺，**不能将 V1 标为最终完成**。

D12-A、G1–G8、D1–D12 沿用原裁决，不重开、不扩展资源规则至便条。A14 无配额接口仍跳过，A29/A30 与专用工具仍范围外。M7 三项独立真实样本（非空提炼、合并工具链、注入用户轮）及原始失败账本保持原登记，它们不证明真实同链；本轮无真实供应商调用。真实同链引用/保留语义、system/developer 角色语义等价、其他供应商及 Windows/宿主专项仍未验证或既有暂缓，不能改称通过，也不自动升级为本次 M9 必需阻塞。新真实调用另需授权。

## 8. 最终工作区检查

最终 `git diff --check` **exit 0、无输出**；18 个未跟踪候选逐文件执行 `git diff --no-index --check -- /dev/null <file>`，均 exit 1（文件相对空文件有新增内容），无空白错误输出，不将该差异退出码冒充 exit 0。结果见 `diff-check.json`。25 个候选路径摘要见 `candidate-files.json`，22 个经完整门禁的源码/测试/黄金字节未变。

保护复核：229 份旧档案不变；15 个保护项中仅前述非本轮写入的 AGENTS.md 与准备快照不同，当前已读版本 SHA `a753ea2e…` 未再次变化，其余 14 项一致。分支、完整 HEAD/main、空暂存区、stash 0、单工作树均复核通过（`protection-git-final.json`）。原始证据和失败日志保留，不执行真实模型历史发送入口。实施轮停在交付候选审批阶段，尚未发生任何 Git 交付动作；用户批准后的交付动作见 §9。

## 9. 交付记录（2026-10-19 用户批准后执行）

批准范围＝实施轮 §7 登记的 25 个候选路径（7 修改＋18 新增，不含并发 AGENTS.md）。交付核验：分支 `feat/013-memory-m9`、HEAD＝main＝merge-base＝`ddeea8552ed8ef2b64585dbc22e481a82dc62d7d`、暂存区空、stash 0、单工作树；工作区恰为 25 路径＋AGENTS.md 并发修改，无候选外内容；25 个文件 SHA-256 与 §8 `candidate-files.json` 登记逐一相符，22 个终验后源码/测试/黄金摘要与 `verification-source-hashes.json` 一致——变异已恢复、临时备份未纳入，源码与 3010/0/3 验证态一致；229 份既有归档与保护项零变化。

25 个批准路径：`docs/specs/013-memory-system-v1.md`、`docs/records/2026-10-19-013-m9-implementation.md`、`docs/security/reports/2026-10-19-memory-m9-security.md`、`src/app/src/memory/{store.rs,retention.rs,mod.rs,runner.rs,phase2.rs,feedback.rs}`、`src/app/tests/{memory_retention.rs,memory_lifecycle.rs,e2e_acceptance.rs}`、`src/app/tests/common/memory_v1.rs`、`src/app/tests/fixtures/memory_lifecycle/` 下 12 份黄金（initial/retained/renewed 的 MEMORY/summary 六份、initial_a.md/b.md/renewed_a.md 三份、initial/retained/renewed_raw.json 三份）。

- **实施提交** `58f6020b3a21fbdd88cb56f193290045df6b4187`（`feat: 013-M9 保留清理与整链验收`，父＝`ddeea855`）：25 路径写入 /tmp 清单后逐文件显式暂存（无 `git add .`/`-A`/目录级暂存）；暂存集合与清单 diff 为空（18A＋7M）；25 个暂存 blob 指纹与 `candidate-files.json` 相符；`git diff --cached --check` 通过；保护项零进入。
- **快进合并**：`git merge --ff-only` 入 main（ddeea85→58f6020）；`merge-base --is-ancestor` 证实包含；合并后 main 提交树与实施提交一致（均 `24e4d4345f458be4cf069322ab96114de18d8d2b`）。快进未引入变化，**引用交付前实测验证**：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` exit 0，**3010 通过/0 失败/3 既有忽略/0 过滤**——不机械重跑、不追加真实调用。
- **交付文档提交**（本节＋规格状态翻转，`docs(013): M9 标记为已交付`）相对实施提交仅修改规格与本文档两份；精确清理边界、变异排除（含 1 个等价项如实登记）、安全结论与未验证项、D12-A 及 M7 既有供应商证据范围全部保留；历史 manifest、封批账本与已归档证据未改写；V1 最终完成标记未翻转，留待最终收口确认。
- **分支清理**：确认实施提交已包含于 main 后 `git branch -d feat/013-memory-m9`（非强制）。
- 不推送、不发布、不操作远程、不调用真实供应商；AGENTS.md 并发修改保持未暂存原样保留。

## 10. V1 最终收口（2026-10-19 用户最终批准）

用户于 2026-10-19 批准 V1 最终交付。条件对照按规格 §6「V1 最终完成条件」逐项核验并全部闭合：M1–M9 必需功能与验收闭合（九阶段实施提交 `cf64a80`/`3d4e80e`/`63bff52`/`4cd9f19`/`96e5908`/`2aa0002`/`67b9461`/`71e370e`/`58f6020` 均已合并 main）；适用矩阵项有可追溯实证（规格 §7）；偏离上游与已接受边界完整登记（G1–G8＋D1–D12）。M9 侧证据＝本记录 §1–§8（实现、同链、错误检测、门禁）＋§9 交付记录（实施提交 `58f6020b3a21fbdd88cb56f193290045df6b4187`、交付文档提交 `2746d4013ec0d9eda90714c879679849025f45bc`）。**V1 按既定范围标记为已完成并交付。**

边界保持原状态、不改称通过：A14 无配额接口按既有决定跳过；A29–A30（双写/V2）范围外；真实供应商同链、system/developer 角色语义等价、其他供应商/模型仍未验证；Windows 及宿主专项按既有暂缓；D12-A 仅自动差异过滤，mem_list/mem_read 旁路及通用写入能力保留。M7 三项独立真实样本仍不证明真实同链；新真实调用需另行授权。

本次收口仅更新两份文档（规格与本记录），引用既有最终验证：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` exit 0，3010 通过/0 失败/3 既有忽略/0 过滤（§6）。不追加测试或真实调用，不改写历史 manifest、封批账本、已归档证据或其他决策状态；历史「待批准」字样按各自轮次原文保留。收口提交完整 SHA 见本轮最终报告，不写入本记录。
