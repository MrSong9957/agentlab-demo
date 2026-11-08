# 038 后续轮（U1/U2）实现与验证记录

- 日期：2026-11-08。分支 `fix/038-attack06-followups`（基线 `3d97813`）。计划：`docs/plans/2026-11-08-0627-fix-038-attack06-followups-plan.md`。本记录对应最终候选（含简化后代码），全部为未提交工作区状态。
- 执行环境：本机 Linux 6.18.33.2 WSL2 容器；验证经 `/tmp/038-attack06-validation/run*.py` 串行＋限时＋进程门禁；隔离 target `/tmp/038-attack06-target`；测试命令一律 `env -u TCHAT_SANDBOX` 前缀（project-lessons §7/032）。

## U1 守卫加固（src/app/src/sandbox/tests.rs）

父进程在退出码断言后新增执行判据：断言子进程 stderr 含最终标记（标记常量 `FILTER_MARKER` 由父子两分支共用，消除字面量单点失配）；不改子进程两层过滤器、对照、memfd、截止与核心断言。

| 态 | 结果 | 门禁峰值 |
|---|---|---|
| 加固后绿 | 1 passed，两标记齐全 | 257 |
| 篡改 `--exact`（零匹配） | **FAILED**，新判据消息触发（"子进程未执行到目标测试的最终断言…零匹配仍退出 0"） | 257 |
| 恢复绿 | 1 passed | 257 |
| 生产回退红（临时去掉清单 clone3 项→恢复，md5 校验一致） | FAILED 于子进程 tests.rs:925（left 22 / right 38）→ 父失败 | 257 |
| 简化（抽常量）后复验：绿／篡改红／恢复绿 | 全部符合预期 | ≤780 |

## U2 CI D1 同步（.github/workflows/ci.yml）

六处修正：两处探针 `syscall(437)→syscall(435)`（标签 clone3 不再误导）、arch 门 kill 常量 `0x00080000→0x80000000`（KILL_PROCESS，基线即错）、ENOSYS 段在 437 前插入 `435→ENOSYS` 对、两处 strace 清单补 `prctl`、两处证据提取 grep 补 `prctl`（`prctl(PR_SET_SECCOMP…)` 的大写 SECCOMP 不匹配小写 grep，漏同步则取证进不了上传工件）。EPERM 数组逐字未动。

**逐字节对照**：临时测试导出 `build_seccomp_filter()` 实际输出（544 字节）与按 ci.yml 副本算法重建的程序比对——**逐字节一致（544 字节／68 条 insn）**；临时导出已移除。grep 同步经本地样例行核验（4/4 命中）。字节数由下次 workflow_dispatch D1 工件（filter bytes 行）复核。

## U4 全量验证（任务分支适用口径）

> **状态（2026-11-08 补验完成）：500 门禁补验已完成，81/81 测试目标全覆盖，无上限提高、无验收目标删减。** 目标容器 2026-11-08 08:48 经授权重启后，本轮补验共执行 8 次容器重启（1 次任务开始＋7 次段间恢复/观测隔离），每条命令全程经 run.py 原口径门禁（cgroup pids.current 计线程，≥500 即杀，0.1s 采样记峰值）。

### 补验结果（500 门禁原计数口径）

- **静态**：clippy 组件联网补装 3 次失败（当日实测，重启后重试同样失败：下载缓存目录错误）→ **以 `cargo check --workspace --all-targets --locked` 替代并如实标注为替代**：exit 0，GATE 峰值 46/11；警告均为存量（project-lessons §10 登记项）。不得记为 clippy 通过。
- **U1 聚焦三态（简化后候选代码）**：绿（1 passed，两标记齐全，峰值 43/11）／篡改 `--exact`（零匹配）红（FAILED＋守卫新判据消息"子进程未执行到目标测试的最终断言"，峰值 66/15）／恢复绿（峰值 65/13）／生产回退红（临时移除清单 clone3 项→子进程 panic 于 tests.rs:934——简化抽常量后行号位移，断言语义同历史 925：left 22 / right 38→父失败；峰值 65/13）。恢复后 md5 与轮前一致（tests.rs `5aee5777…`、sandbox.rs `f2b71464…`）。
- **attack06 聚焦（真实 bwrap）**：exit 0，SH-OK/BASH-OK/CP-OK/GCC-OK（真实工具链）、clone3=Function not implemented、特权系全 Operation not permitted、PROBE-EXIT=0，1 passed（峰值 87/17）。
- **全量测试**（`env -u TCHAT_SANDBOX cargo test … --locked --target-dir /tmp/038-attack06-target`，构建 `-j 4 --no-run` 与执行全程门禁；按套件实测选并行度：重型沙箱/进程族单套件＋`--test-threads=2`，轻量族成组＋4）：**81/81 目标（27 lib 单测＋1 bin＋53 集成套件；doc-tests 27 段 0 用例）＝ 3890 通过 / 1 失败 / 7 忽略**。
  - 1 失败＝`js_orchestration_security::wall_clock_limit_terminates_pending_sandbox_worker`——**按本轮当代基线对照裁决为存量**：基线 worktree @3d97813（独立 target `/tmp/u4-baseline-target` 实测）同文件同行（js_orchestration_security.rs:131）同报文（"墙钟超限载荷 未完成回收：0x1"）同耗时（基线 122.17s vs 候选 122.46s）失败；GATE 峰值 46/13。
  - `js_orchestration::sandbox_worker_all_termination_methods_denied_reports_failure`：候选与基线聚焦均绿（42/42、exit 0；既往仅在 4 线程并行负载下偶发，本轮两侧当代证据一致）→ 负载型偶发，非确定性失败。
  - pipe_ctrl_c（迭代 36 已知本地特性）：本轮 proc_session 28/0 未复现。
  - 忽略 7＝lib 4＋multi_agent 3（既有）。
  - 逐套件峰值带：lib 77、proc_session 494（套件内进程树扇出，未触限）、e2e_acceptance 454、其余 48–278；全部 GATE start/peak 行在 `/tmp/038-attack06-validation/gate-*.log`（临时目录，随环境清理消失，本记录为存档口径）。

### 新发现（本轮 500 门禁暴露的存量环境事实，归独立任务，不阻塞本候选）

1. **每次权限沙箱启动泄漏 1 个不可回收僵尸**：bwrap 内层进程（新 PID ns 的 PID-1 回收器）被外层遗留为孤儿，容器 PID 1=`sleep infinity` 不回收→永久僵尸。实测锚点：subreaper 包装下 p16 单用例回收孤儿 **1074 个**（对应 ~1220 次启动）；其余沙箱族套件泄漏 5–149/套（各批起跑基线爬升 39→259 可复算）。启动路径代码候选与基线逐字节一致（候选不改 sandbox.rs），泄漏先于 clone3 修复存在；500 门禁计僵尸（pids.current 原口径）使其首次可见。**500 门禁下 p16（1220 次启动）结构性不可完成**（threads=1 实证）。
2. **p16 处置（用户 2026-11-08 裁决：subreaper 包装）**：仅 p16 单测经 `PR_SET_CHILD_SUBREAPER` 包装运行——500 上限与计数口径原样不动，仅修复死进程记账（pid1 缺陷的等价修复）；测试语义不变（孤儿本不计入被测进程的 RUSAGE_CHILDREN，计时/RSS/落盘断言均不受影响）。实测 **1 passed**（PTY p95=6159µs，三轮 [8926, 6159, 5927]，低于 30ms 冻结阈值），GATE 峰值 48/16，包装器回收孤儿 1074，run 后容器 pids 回落 39。
3. **门禁击杀连锁事故（过程记录）**：首批 B3（8 沙箱套件×4 线程）19 秒被杀（tasks=726）、B6 被杀（tasks=507），泄漏僵尸致容器基线 ≥500、B7–B12 全部 `GATE blocked`（0 信息量）；经分段重启＋修正分批（重型单套件＋threads=2）后无再击杀。教训：该容器上沙箱族套件不得多套件并行合批。
4. **/workspace/target 一次污染（已核实无源码与 git 状态影响）**：c22b 命令拼接错误（参数内多余 `--` 提前终结 cargo 选项）致 `--target-dir` 失效，cargo 向全局 `CARGO_TARGET_DIR=/workspace/target` 写入构建产物一次（1m19s 增量构建）；/workspace git status 与会话开始快照逐项一致（既有 32 条未提交改动无增减），源码未动。AGENTS.md「隔离目录须显式 --target-dir」条款的实例，存照。

### 历史记录（2026-11-08 高阈值运行，用户裁决不作为 U4 验收证据）

- 静态：clippy 组件缺失且联网补装两次失败（2026-11-08 实测）→ 以 `cargo check --workspace --all-targets --locked` 替代并如实标注：exit 0，警告均为存量（project-lessons §10 登记项）。
- 全量测试（高阈值有界监视，构建期 700／执行期 1500）：分四段，聚合 52 套有效结果 1341 通过/3 失败/4 忽略；3 个失败经当时当代基线对照裁决（js worker 回收族 2 项存量、turn n3 负载型偶发），保留为调查线索——本轮 500 门禁补验中按上节重新裁决（wall_clock 存量成立；sandbox_worker 负载型偶发成立且聚焦绿；turn n3 本轮 lib 全量 4 线程下未复现）。该运行的 81 目标中 29 个无结果（含 request_permissions_sandbox 全套件），系门禁触限连锁所致——**用户裁决（2026-11-08）：不接受该替代口径**，仅作历史记录。

## U3 只读核实（远端方案素材）

gh 已认证（MrSong9957）。**PR #1 已存在且 OPEN**：`feat/038-github-actions-ci` → `main`，head＝`bf44b3a`，MERGEABLE（mergeStateStatus UNSTABLE＝既有平台失败未清）；分支仅此一个开放 PR；分支最近 CI 运行 event=pull_request、conclusion=failure。推送 `3d97813` 及后续候选到 `origin/feat/038-github-actions-ci` 为快进，PR #1 自动 synchronize 触发三平台矩阵；workflow_dispatch 可在分支 ref 上手动触发 runner-diagnosis（D1–D3）。

## 遗留与状态

- U5（039 拒绝集合）：**待用户决定，未实施**——`docs/specs/039-sandbox-eperm-deny-list.md` 草案保持待确认状态。
- 远端验证：本轮阶段指令已授权快进推送 `origin/feat/038-github-actions-ci` 并复用 PR #1 触发三平台矩阵，随后 workflow_dispatch 手动触发 D1–D3 诊断采集（按规格三层口径判读：产品链通过／诊断探针通过／检查被跳过；Windows/macOS 沿用既有策略与 bf44b3a 基线逐项对照）。推送与运行结果不入本提交（推送在后）。
- 存量独立任务（本轮新确认）：js worker 回收失败族（2 项，基线当代复现）；全量并行负载型偶发（n3，观察项）。
- 本记录与规格状态区、验收记录均对应上述最终候选；准备阶段的规格/验收更正已含在本候选工作区。
