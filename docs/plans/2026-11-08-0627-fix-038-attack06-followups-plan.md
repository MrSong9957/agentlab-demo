---
title: 038 attack06 后续五项加固与验证 - Plan
type: fix
date: 2026-11-08
artifact_contract: ce-unified-plan/v1
product_contract_source: ce-plan-bootstrap
execution: code
---

# 038 attack06 后续五项加固与验证 - Plan

## Goal Capsule

- **Objective：** clone3 修复交付（`8ba97d2`＋`3d97813`）之后，038 的守卫、诊断与验证口径恢复可信：回归守卫不再可能静默空转，CI 诊断副本与产品过滤器一致，规格与验收记录不含不存在的跳过声明，全量验证有如实的分层结果，EPERM 清单修正有可裁决的规格。
- **Means：** 五项独立最小改动（U1–U5，关键决策 KTD1–KTD4），全部在任务分支 `fix/038-attack06-followups` 上本地实施；推送与远端验证仅按用户选择的目标执行（R3、Open Questions）。
- **Authority hierarchy：** 用户阶段指令（本轮处理范围 1–5）＞ AGENTS.md 流程＞ 本计划 KTD。拒绝集合变更与推送目标为用户保留决策（Open Questions）。
- **Execution profile：** 单分支、本地提交 2–4 笔（docs / fix / ci），不合并、不推送，除非用户随后授权。谁收尾：实现 Agent 完成验证与提交清单，用户批准推送与 EPERM 拒绝集合。
- **Stop conditions：** U5（EPERM 实施）在用户确认拒绝集合前不启动；任何需要改动的相邻问题（攻击面外清单条目、其他平台）一律登记不顺手修。

## Product Contract

### Summary

上轮独立审查确认 clone3 修复本身正确（Ready with fixes），并留下四项确认发现与一项 runner 验证缺口。本轮把四项可行动发现转成实施，把验证口径改到可区分、可核查：测试守卫加固（零匹配必须失败）、CI D1 诊断副本与探针同步、runner 产品链证据分层、全量验证如实分层、EPERM 清单规格化（拒绝集合待批）。

### Problem Frame

- 守卫空转面：`seccomp_filter_blocks_clone3_and_preserves_openat2` 的父进程只断言子进程退出码（`src/app/src/sandbox/tests.rs:885`），而 `--exact` 过滤器是硬编码字符串（tests.rs:867）。libtest 对零匹配仍以退出码 0 结束——测试一旦改名，守卫绿灯而证明断言从未执行。两个独立审查者（adversarial P1、testing P2）+验证者确认。
- 诊断漂移面：`.github/workflows/ci.yml` 的 D1 手工诊断副本声称与 `build_seccomp_filter` "字节一致"（ci.yml:127），但其 ENOSYS 段只有 437（ci.yml:134），两处探针调用 `syscall(437, NULL, 0)` 却注释为 clone3（ci.yml:120、178）。产品过滤器加入 435 后为 544 字节，副本仍 528 字节——435 规则若回退，D1 会给出误导性的"clone3 正常"取证。D1 strace 清单缺 `prctl`（ci.yml:146、185），而 findings.md 已证 bwrap 0.11.1 用 `prctl(PR_SET_SECCOMP)` 装载过滤器，漏跟踪即不能取证加载。
- 证据口径面：规格 §5 第 6/7/8 条与验收记录声称 CI 以 `TCHAT_SKIP_SECCOMP_ENFORCEMENT_PROBE=1`／`TCHAT_SKIP_RUNNER_TIMING_PROBES=1` 跳过检查——该机制已于 `c50ff40`（Linux 真实检查恢复）移除，当前 workflow 与代码均不引用（`git log -S` 双向核实）。现状是：attack06 在主 test job 真实运行、runner 复现即如实失败；文档若继续声称"被跳过"，会把"检查在跑"误报为"检查未跑"。
- 验证口径面：AGENTS.md 全量验证入口为 `cargo clippy --all-targets -- -D warnings && cargo test --workspace --no-fail-fast`（clippy 不可用兜底 `cargo check`）。本环境 clippy 组件缺失且联网补装失败（2026-11-08 实测），必须以"替代检查"如实标注，不得写成 clippy 通过。
- 清单漂移面：`SECCOMP_DENY_EPERM`（sandbox.rs:1181-1209）27 条中 21 条编号/注释/UAPI 三方一致；328 注释 open_tree 实为 pwritev2（误拒普通写调用），428-432 实拒 open_tree/move_mount/fsopen/fsconfig/fsmount 但注释整体右移一格，mount_setattr(442) 与 fspick(433) 缺失。权威对照（`/usr/include/x86_64-linux-gnu/asm/unistd_64.h`＋锁定的 libc 0.2.189 `SYS_*`）已脚本化核实。

### Requirements

- R1. 子进程未实际执行目标测试（含 `--exact` 零匹配、改名失配）时，父测试必须失败；子进程两层过滤器、独立 EINVAL 对照、真实产品 memfd、10 秒截止机制与两条核心证明断言保持不变。
- R2. CI D1 副本的 ENOSYS 段含 435 规则、两处探针调用真实 clone3（435）且标签正确，"字节一致"注释恢复真实（副本与产品逐字节一致：arch 门 KILL_PROCESS 常量、ENOSYS 次序 [435,437]、68 条 insn／544 字节）；EPERM 段本轮原样保留（跟随 039 裁决）。
- R3. 规格与验收记录更正为可核查事实：当前树无跳过机制，attack06 真实运行；runner 产品链证据必须能区分诊断探针通过（D1 工件）、产品链通过（主 test job attack06）、检查被跳过（早退标记＋前置安装步骤结果）。
- R4. 全量验证按 AGENTS.md 入口执行并如实分层报告 clippy／cargo check／测试结果；替代检查不得写成原检查通过；已知本地失败单列。出处注记：clippy-first 入口表述来自用户 2026-11-08 在 /workspace 主工作区的 AGENTS.md 未提交更新；任务分支已提交版 AGENTS.md 仍为 `cargo check && cargo test --workspace --no-fail-fast`（clippy 标注未安装）——U4 按 KTD4 口径执行，不依赖分支上已提交的 AGENTS.md 文本，本轮不提交 AGENTS.md。
- R5. EPERM 修正分离：纯注释修正与拒绝集合变更分开陈述；拒绝集合变更仅在用户批准后实施；实施后清单条目有与 libc `SYS_*` 符号对照的回归锚（拒绝集合不变）。

### Scope Boundaries

- 不修改：`SECCOMP_DENY_ENOSYS` 行为、attack06 载荷与断言、其余平台（Windows/macOS）、容器环境、`SECCOMP_DENY_EPERM` 的编号集合（除 039 获批后）。
- 考虑过不建：给 D2/D3 诊断 job 加 lib 单测调用（新单测已随主 test job 的 `cargo test --workspace` 覆盖，重复入诊断 job 无增量证据）；把跳过路径改成显式测试失败（会破坏"环境缺依赖≠产品缺陷"的既有分层，跳过可见性由安装步骤失败＝job 失败保证）。
- Deferred to Follow-Up Work：039 获批后的 EPERM 实施（U5 门控）；runner 产品链实测（需推送授权）。

### Open Questions

- **阻塞 U5（用户已保留）：** EPERM 拒绝集合——328 移除（不再误拒 pwritev2）还是保留＋注释改 pwritev2？是否新增 442 mount_setattr？是否新增 433 fspick？见 `docs/specs/039-sandbox-eperm-deny-list.md`。
- **阻塞远端验证（用户已保留）：** 推送目标——(a) 从 `fix/038-attack06-followups` 开 PR 触发 pull_request CI；(b) 快进推送 `origin/feat/038-github-actions-ci`（现位于 bf44b3a，`3d97813` 为其后代，FF 可行；push 仅 main 触发，FF 后无 CI）＋随后 PR；workflow_dispatch 手动 D1 需分支已在远端。

## Planning Contract

- KTD1. 父进程判据＝在现有退出码断言之后，断言子进程 stderr 含最终标记 `产品过滤器：clone3=ENOSYS openat2=ENOSYS`（session-settled: user-approved — 用户授权调整父进程判据并保留子进程设计；标记位于两条核心断言之后，存在即证明断言真实执行且通过，强于 stdout `1 passed`（只证明有一个测试运行）与证据文件方案（仓库惯例但新增 I/O 与清理面））。
- KTD2. D1 副本同步面＝三处内容修正＋探针与注释：①arch 门 kill 动作 `0x00080000`（KILL_THREAD，ci.yml:131）改为 `0x80000000`（产品 `SECCOMP_RET_KILL_PROCESS`，sandbox.rs:1226——基线即不一致，"字节一致"注释在基线即为假）；②ENOSYS 段在既有 437 对**之前**插入 `435→ENOSYS` 规则对（产品清单次序 [SYS_clone3, 437]，sandbox.rs:1215）；③两处探针改 `syscall(435)`＋标签更正。EPERM 数组逐字保留；同步后副本与产品过滤器逐字节一致（68 条 insn／544 字节）。
- KTD3. D1 strace 跟踪清单补 `prctl`（findings.md 实证 bwrap 0.11.1 经 `prctl(PR_SET_SECCOMP)` 装载；现清单只跟踪 seccomp/memfd_create/close_range，取证不了加载路径）。
- KTD4. 全量验证口径＝`env -u TCHAT_SANDBOX` 前缀（032 教训：环境泄漏扭曲基线）；隔离 `--target-dir`（现成 `/tmp/038-attack06-target` 已预热，020 教训：冷 target 全量编译超单命令时限）；clippy 失败即以 `cargo check` 替代并在报告标注"替代"（入口出处注记见 R4——分支已提交 AGENTS.md 为 check-first，本轮按本 KTD 执行）；已知本地失败（proc_session 管道 Ctrl-C，迭代 36 记录 3856/1/6）单列不计为回归。
- 假设：runner 镜像预装 gcc（ubuntu-latest 含 build-essential）、bubblewrap 由安装步骤显式安装且该步骤失败即 job 失败——attack06 早退路径在 runner 上概率低，"被跳过"由早退标记＋安装步骤结果判定。

## Implementation Units

### U1. 守卫加固：父进程断言子进程真实执行

- **Goal:** 满足 R1——零匹配时父测试失败。
- **Files:** `src/app/src/sandbox/tests.rs`（父分支，tests.rs:885 附近）。
- **Approach:** 按 KTD1 在 `assert!(out.status.success(), ...)` 后追加一条对 `out.stderr` 的标记断言；失败消息写明"`--exact` 名称可能失配（libtest 零匹配仍退出 0）"。不改子分支、超时路径与 `eprint!` 转发。
- **Patterns to follow:** 仓库自孵化测试证明执行的既有惯例（`src/app/tests/sandbox_matrix_probes.rs` run_probe 结果文件断言、`src/app/tests/multi_agent.rs` c13 证据文件断言）——本单元以进程内标记达成同一保证，不新增文件。
- **Test scenarios:**
  - 绿：完整跑该单测，通过且输出含两条既有标记（行为与加固前一致）。
  - 变异（守卫有效性证明）：临时篡改 `--exact` 字符串（或临时改测试名模拟失配）→ 父测试必须失败且消息指向新断言；恢复后复绿。
  - 红（既有证据链复验）：临时回退生产清单一行 → 子进程仍在 tests.rs:925 失败（22≠38）→ 父进程失败（退出码断言或新断言任一触发均算失败）；恢复后复绿。
  - 超时路径：不触发（代码路径未动，审查确认即可）。
- **Verification:** 聚焦单测三态（绿／变异红／生产回退红）在隔离 target-dir 下按串行＋500 门禁执行，全部符合预期。

### U2. CI D1 副本与探针同步

- **Goal:** 满足 R2——诊断副本恢复字节一致，探针取证真实 clone3。
- **Files:** `.github/workflows/ci.yml`（D1 最小复现段 120/134 行、产品启动链段 178 行、strace 跟踪清单 146/185 行、"字节一致"注释 127 行）。
- **Approach:** 按 KTD2 补 `prog += insn(0x15,0,1,435) + insn(0x06,0,0,0x00050000|38)`、探针改 `syscall(435, NULL, 0) /* clone3 */`；按 KTD3 strace 清单加 `prctl`；EPERM 数组逐字不动。
- **Test expectation: none --** workflow 文件无本地可执行测试；验证走下方模拟＋未来 D1 实跑工件。
- **Verification:** 本地 python 按副本算法重建程序 → 与 `build_seccomp_filter()` 实际输出（经临时测试导出字节，验证后移除临时导出）做**逐 insn 字节比对**，覆盖 arch 门 KILL_PROCESS 常量与 ENOSYS 次序 [435,437]；总计 68 条 insn／544 字节；已知产品侧行为锚（红绿日志 435→38）交叉印证。字节数 544 由下次 workflow_dispatch 的 D1 工件（filter bytes 行）复核；逐字节同一性由本地对照保证。
- **Dependencies:** 无（与 U1 并行）。

### U3. 规格与验收记录更正＋039 草案入库

- **Goal:** 满足 R3、R5 的文档面——证据口径可核查。
- **Files:** `docs/specs/038-github-actions-ci.md`（§5 第 6/7/8 条更正、状态区本轮条目）、`docs/logs/038-github-actions-ci/2026-11-08-attack06/acceptance.md`（跳过声明更正）、`docs/specs/039-sandbox-eperm-deny-list.md`（草案，准备阶段已建立）。
- **Approach:** 更正以日期化补注方式落在原条目内（保留历史记录），写明 `b3d5700` 引入／`c50ff40` 移除的证据链；039 草案承载权威对照表与待决事项。
- **Verification:** 文档中现存跳过陈述均附 2026-11-08 更正标注（历史原文按补注保留），不存在未标注的当前时态跳过声明；`grep -rn TCHAT_SKIP_ --include=*.yml --include=*.rs` 为空与文档一致。
- **Dependencies:** 无；准备阶段已完成主体，本单元=入库提交。

### U4. 全量验证执行

- **Goal:** 满足 R4——分支候选的全量分层结果。
- **Files:** 无代码改动；产出验证记录（docs/logs/038-github-actions-ci/ 下日期化记录）。
- **Approach:** 按 KTD4 在任务分支执行 AGENTS.md 全量入口：clippy（失败则 check 替代并标注）＋`cargo test --workspace --no-fail-fast`；串行、500 任务门禁（`/tmp/038-attack06-validation/run.py`）。
- **Test scenarios:**
  - 静态：clippy（或替代 check）退出码与警告归属如实记录（存量告警见 project-lessons §10，不计入本轮）。
  - 全量：通过/失败/忽略计数；已知本地失败（pipe_ctrl_c）单列并与迭代 36 基线（3856/1/6）对照。
  - 聚焦复核：U1 三态与 U2 模拟结果并入记录。
- **Verification:** 记录含命令、环境（env -u 前缀、target-dir）、退出码、分层结论；不出现"替代检查写成原检查通过"。
- **Dependencies:** U1、U2 完成后执行。

### U5. EPERM 清单实施（门控：用户确认拒绝集合后）

- **Goal:** 满足 R5 的实施面——仅在 Open Questions 裁决后启动。
- **Files:** `src/app/src/sandbox.rs`（清单与注释）、`src/app/src/sandbox/tests.rs`（编号回归锚）、`docs/specs/039-sandbox-eperm-deny-list.md`（状态流转）。
- **Approach:** 注释修正与拒绝集合变更按 039 草案分类执行；新增"清单条目 ↔ libc `SYS_*` 符号对照"单测（编译期/运行期一致性，拒绝集合不变）；改后以受控 memfd 装载法逐项探测变更条目，并重跑 seccomp 单测与 attack06。
- **Test scenarios:**
  - 回归锚：任一条目编号与符号名不符时测试失败（可用临时错配验证）。
  - 拒绝集合变更（若获批）：变更条目在受控两层过滤器下实测新 errno；未变更条目行为不变。
  - attack06 与 U1 守卫全绿。
- **Verification:** 清单与权威表三方一致（注释/官方/libc）；变更面仅限批准范围。
- **Dependencies:** 用户对 039 的拒绝集合裁决；U1 的受控装载模式可复用。

## Verification Contract

| 检查 | 命令（任务分支，隔离环境） | 覆盖与口径 |
|---|---|---|
| 静态 | `cargo clippy --all-targets -- -D warnings`；组件缺失时 `cargo check --workspace --all-targets --locked`（标注为替代）；均带 `--target-dir /tmp/038-attack06-target` | 替代不得记为 clippy 通过 |
| 全量测试 | `env -u TCHAT_SANDBOX cargo test --workspace --no-fail-fast --locked --target-dir /tmp/038-attack06-target` | 已知本地失败单列；报告通过/失败/忽略计数 |
| 聚焦回归 | `env -u TCHAT_SANDBOX cargo test -p terminal-chat --lib sandbox::tests::seccomp_filter_blocks_clone3_and_preserves_openat2 --target-dir /tmp/038-attack06-target -- --exact --nocapture` | 绿/变异红/生产回退红三态 |
| attack06 | `env -u TCHAT_SANDBOX cargo test -p terminal-chat --test security_attack_031 attack06_seccomp_allows_toolchain_blocks_privileged_syscalls --target-dir /tmp/038-attack06-target -- --exact --nocapture` | 工具链正向＋clone3=ENOSYS＋特权全拒 |
| D1 模拟 | python 按 ci.yml 副本算法重建 → 544 字节＋逐段结构对照 | 覆盖 U2；残余在下次 D1 实跑复核 |
| 执行条件 | 全部经 `/tmp/038-attack06-validation/run.py` 串行＋限时＋500 任务门禁 | 038 既有验证纪律 |

远端（需用户授权推送后）：PR 触发三平台矩阵；workflow_dispatch 手动 D1–D3（30 分钟）采集 prctl/strace/过滤器字节工件——按 R3 三层口径判读。

## Definition of Done

- 全局：R1–R4 全部有可核查证据；U5 仅在用户裁决后判定；工作区只含本计划与已批准清单内改动；无废弃尝试代码残留。
- U1：守卫三态符合预期，子进程设计逐项未动。
- U2：逐字节对照通过；`git diff` 仅触及 KTD2/KTD3 声明的行（含 arch 门 kill 常量行）。
- U3：现存跳过陈述均附更正标注、无未标注的当前时态跳过声明；039 草案与 sandbox.rs 当前事实一致。
- U4：验证记录完整且分层如实；替代口径显式标注。
- 提交边界：docs（U1 前置的准备产物与更正）、fix（U2 后的 tests.rs）、ci（U2）各自成笔；推送与合并不在本轮授权内。

## Sources

- 守卫发现：`/tmp/compound-engineering-1000/ce-code-review/20261108-024223-061dbcf6/report.md`（#1 P1、#2 P3、#3 存量；验证者裁决）。
- 跳过机制演变：`git log -S TCHAT_SKIP_SECCOMP_ENFORCEMENT_PROBE` → 引入 `b3d5700`（CI 第 3 轮）、移除 `c50ff40`（Linux 真实检查恢复）；attack06 注释"2026-10-31 恢复真实检查"（`src/app/tests/security_attack_031.rs:805-812`）。
- workflow 事实：`.github/workflows/ci.yml`（触发 13-17 行：push 仅 main、pull_request 全部、workflow_dispatch；test job env 44-55 行无跳过变量；D1 111-149、产品链对证 166-191）。
- EPERM 权威对照：`/usr/include/x86_64-linux-gnu/asm/unistd_64.h`、`~/.cargo/registry/src/*/libc-0.2.189/src/unix/linux_like/linux/gnu/b64/x86_64/not_x32.rs`（SYS_pwritev2=328、SYS_open_tree=428、SYS_clone3=435、SYS_openat2=437、SYS_mount_setattr=442）；脚本化核对记录见 039 草案。
- 推送拓扑：`origin/feat/038-github-actions-ci`＝bf44b3a；`b98d52a` 为其祖先；任务分支 `3d97813`＝bf44b3a＋2（FF 可行）；main＝`4b85ce8`。
- 验证纪律与已知基线：AGENTS.md §4/§5；`rules/project-lessons.md` §7（032 环境泄漏、029 多目标过滤器）、§9；038 迭代 36 L3 全量 3856/1/6（提交 57d5408 记录）。
