# 033 B1/B2 加固实施记录（2026-10-27，候选待验收）

**分支**：`feat/033-b1-b2-hardening`（工作树 `.worktrees/033-b1-b2`，基线＝main
`28b7999`；全部改动未提交）。**候选已修复验证；当前部署版本（/workspace/target/
debug/terminal-chat 与共享 target）未替换**——工作树使用独立构建目录
`/tmp/tchat-b1b2-target`，主工作树与部署产物零触碰。

依据：用户批准的两项加固（风险核实见
[2026-10-27-b1-b2-risk-verification.md](2026-10-27-b1-b2-risk-verification.md)；
矩阵证据见 [2026-10-27-matrix-live-verification.md](2026-10-27-matrix-live-verification.md)）。

## 1. 改动清单（产品代码 4 文件，均最小范围）

| 文件 | 改动 |
|---|---|
| `src/app/src/sandbox.rs` | ① 模块头注补 B1/B2 说明；② 新增 `close_inherited_fds_mark_cloexec()`：Linux＝`close_range(3, ~0, CLOSE_RANGE_CLOEXEC)`（标记非关闭），非 Linux unix＝ENOSYS 显式失败（能力不足不静默放行）；③ `append_unreadable_root_args` 在 missing no-op 分支后插入 B1 检查：`fs::metadata()`（跟随符号链接）取实际常规文件 nlink>1 → Err（fail closed）；目录跳过（nlink＝子目录计数）；每次组装复查不缓存 |
| `src/app/src/proc_session.rs` | `build_tool_command` 沙箱分支注入 `pre_exec(close_inherited_fds_mark_cloexec)`——一处覆盖管道＋PTY 两模式（该函数为 exec 面唯一包装构造点）；普通分支零变化 |
| `src/app/src/mcp/transport.rs` | `StdioProcess::spawn` 沙箱分支经 tokio 自带 `pre_exec` 注入同一助手（unix 门控）；普通分支零变化 |
| `src/app/src/apply_patch.rs` | `apply_patch_sandboxed_with_exe` 的 spawn 链重构为可注入形式＋同一 pre_exec（unix 门控）；`apply_patch_with_display`（普通模式）零变化 |

**B2 关键机制依据**（子代理 A 调查＋实证，见 §3）：std 在 Linux 的 spawn 失败信道
为 SOCK_SEQPACKET socketpair（fd≥3、CLOEXEC）；pre_exec 里**真 close** fd≥3 会使
execve 失败被误报为 spawn 成功（子进程静默 exit 1）——故必须 `CLOSE_RANGE_CLOEXEC`
标记（execve 成功时内核统一关闭，失败时信道保留、errno 正常回传）。另：std 先
dup2 stdio（0/1/2）再跑 pre_exec（proc_session.rs:1107 既有注释＋本轮探针双实证）；
设置 pre_exec 后 std 必然走 fork+exec（posix_spawn 排除）。

## 2. RED → GREEN（真实运行缺陷，非编译失败）

RED（未修复产品代码上，工作树独立构建）：

| 用例 | RED 证据 |
|---|---|
| 单元 `deny_target_with_hardlinks_fails_closed` | `unwrap_err()` on Ok——argv 照常组装（泄漏场景实锤，完整 argv 已留档） |
| 单元 `rootless_symlink_deny_follows_to_actual_file_nlink` | 同上（rootless 符号链接 deny 的实际文件多链接未拒绝） |
| 集成 `b1_hardlinked_deny_refused_on_all_three_faces` | exec 面 `status=completed`（工具照常执行、marker 产生）——应为 error |
| 集成 `d_fd_inheritance_sandbox_closed_normal_unchanged` | 沙箱分支 `PID=1` 且 fd 正文可读（`FAKE-FD-SECRET-033`）——应不可读 |
| E2E `b2_real_cli_fd_chain_full_path` | 沙箱分支 launcher fd 正文可读——应不可读 |

GREEN（实现后）：上述全部通过；`matrix_a`（移除硬链接夹具后）两分支通过；
单元 19/19（16 存量＋3 新增）；探针文件 8→10 例全绿（新增 b1 三面、b2 MCP 面）；
三型沙箱集成 6/6（PTY/管道/MCP/apply_patch 与 pre_exec 共存无回归）；
e2e_acceptance 143/143（普通模式零变化）；两测试文件注入态与 `env -u` 双口径全绿。

**变异证明（敏感性）**：临时移除 mcp/transport.rs 的 pre_exec 注入 →
`b2_mcp_face_fd_closed` FAILED（RC=101）→ 恢复 → 通过——MCP 面测试确能发现注入
点丢失；exec 面与 B1 的检出力由上述 RED 直接证明（未修复态＝注入/检查缺失态）。

**spawn 失败信道探针**（b2_fix_feasibility，机制层）：pre_exec 内 CLOSE_RANGE_
CLOEXEC 后——execve 成功路径 stdio 0/1/2 保留、被标记 fd 不进载荷；**execve 失败
路径（程序不可执行）spawn() 仍返回 Err（OS 错误码原样回传）**——「真 close」型
回归（误报成功）的常驻防线。

## 3. 并行调查（两子代理，只读，实际启动）

- **子代理 A（本项目）**：全链行号级核查（启动→策略→三面分派→spawn 设置→错误
  回传→回收）；tokio 1.53.3 `as_std_mut`/自带 `pre_exec` 可用性；libc 0.2.189
  `close_range`/`CLOSE_RANGE_CLOEXEC` 绑定；std master 源码（Linux spawn 失败
  信道＝SOCK_SEQPACKET socketpair fd≥3 带 CLOEXEC、pre_exec 在 stdio dup2 之后
  按注册序执行、设置 pre_exec 后必然 fork+exec、AS-safe 要求与 Os-repr 错误
  范式）。未证实项：std 1.99 本机源码（rust-src 未装，以 master＋机制稳定性
  推断）；close_range 无 POSIX AS-safe 正式背书（单 syscall 无分配锁，低风险）。
- **子代理 B（Codex 官方源码）**：冻结点 `d8acdf1`（2026-10-23T12:00Z）＋当前
  main `b89952d582be`（2026-10-27）分开核对，全部引用记录实际提交。要点：
  ①外层 shell 工具路径（含沙箱化 exec）在 Linux **不做任何 fd 清理**（pre_exec
  仅 PDEATHSIG＋tty 脱离；`close_inherited_fds_except` 为 macOS-only）；Linux fd
  清理仅在 pty crate 的 setup-helper 路径（PTY/pipe 交互 spawn），实现＝
  `close_range`+`CLOSE_RANGE_CLOEXEC` **标记**＋getdents64 枚举补关，注释原文
  "Mark rather than close: std::process still needs its CLOEXEC error pipe if
  exec fails."——与本项目 B2 机制选择独立同型；②硬链接（nlink）全链路（sandboxing/
  linux-sandbox/protocol/core/utils-pty 20+ 文件，冻结点与 main）**零命中**；
  ③冻结点→main 唯一沙箱链行为变化＝`af2e52c858cf`（deny-glob 搜索根 metadata
  错误由静默跳过改 fail-closed）——本项目单路径 deny 无 glob 面，不适用；
  ④错误回传＝stderr＋退出码＋拒绝启发式关键词扫描（`is_likely_sandbox_denied`），
  preflight 管道仅 helper 自用。子代理 B 报告中「本项目未做 writable-symlink
  fail-closed」为**误判**——主代理核查：`first_writable_symlink_component_in_path`
  逐字保留于本项目 sandbox.rs 且有 T2/T3 单测覆盖（见差异表第 3 行）。

## 4. 差异表与取舍（本项目 vs 上游 d8acdf1；main 差异单列）

| # | 维度 | 本项目（033＋B1/B2 候选） | 上游（d8acdf1；main 除非注明） | 机制差异与实测影响 | 处置 |
|---|---|---|---|---|---|
| 1 | 工具子进程 fd 清理（B2） | **本轮新增**：三工具 spawn 点沙箱分支 pre_exec `close_range(CLOSE_RANGE_CLOEXEC)` 标记 | 外层 shell 工具路径**无清理**（Linux）；仅 PTY/pipe 交互路径经 setup-helper 清理（同型 close_range 标记＋getdents64 补关） | 本项目 shell 工具捕获路径与上游同漏（B2 实测链路可读）；上游机制印证「标记非关闭」的必要性 | **已实施**（未搬 setup-helper 架构——本项目无 helper 进程层，直接 pre_exec 达成同等收口；getdents64 补关为上游 helper 形态所需，本项目 execve 原子关闭已覆盖） |
| 2 | 受保护文件硬链接（B1） | **本轮新增**：deny 目标 nlink>1 fail-closed（每次启动复查） | 冻结点与 main **均零处理**（路径遮蔽不感知 inode 别名；seccomp 亦不限 link 类 syscall） | 上游同漏——「上游也有」不作为免修依据（用户已批准加固） | **已实施**（超出上游的加固；TOCTOU 余留如实声明） |
| 3 | writable-symlink fail-closed | **已有**（`first_writable_symlink_component_in_path` 逐字保留＋T2/T3 单测） | 同款（deny-read/read-only 两条报错） | 无差异 | 无需动作（子代理 B 的反向误判已纠正） |
| 4 | bwrap 可信性 | G5：拒绝**锚定工作区内**的 bwrap 候选 | 更广：拒绝策略可写/可替换路径的全部候选（`7aa8f5104963`/`b8f880bc38b8`，早于冻结点） | 上游覆盖「owner 可 chmod 后改写的路径」；本项目覆盖工作区面 | **值得另行评估**（超出本轮范围，只列建议） |
| 5 | deny 表达与遮蔽 | 单一具体路径（锚定根 .env）＋`--ro-bind /dev/null` | glob 预展开（rg/内建回退）＋perms/ro-bind-data＋canonical symlink 双记录＋运行时 ReadDenyMatcher＋上限/限深 | 本项目无 glob 面；上游 `af2e52c858cf`（main）的搜索根 fail-closed 修正亦属 glob 面 | 当前功能不需要 |
| 6 | fd 挂载认证 | 无 fd 传递挂载（/dev/null bind 替代，适配声明④） | `--ro-bind-fd`＋`--verify-fd-mount`＋daemon_mounts 校验三件套 | 仅当引入 fd 传递挂载时需要 | 不适用；未来引入 fd 传递时须整体评估 |
| 7 | 错误回传 | 三面显式 Err（ExecResult::error／connect_err／apply_patch err），022 容错隔离 | stderr＋退出码＋拒绝启发式关键词扫描（`is_likely_sandbox_denied` 明示为启发式） | 产品形态差异（本项目面向单工具显式契约） | 无需对齐 |
| 8 | 内层 seccomp/capget 自检 | 无内层阶段（既有裁剪：seccompiler 离线不可装） | `--apply-seccomp-then-exec` 内层＋capget 能力自检 panic | 既有已审差异（规格适配声明③） | 维持既有决策 |
| 9 | synthetic mount registry | deny missing＝no-op（已审差异：目标是已存在凭据文件） | per-uid registry＋活跃 owner 检测（解决并发沙箱合成挂载竞态） | 本项目不创建合成挂载 | 不适用 |

分类归档：**上游共有边界**＝B1、B2（#1/#2，均已超出上游加固）；**本项目适配引入
的缺口**＝无；**当前功能不需要的上游机制**＝#5/#6/#8/#9；**值得另行评估的高价值
项**＝#4（bwrap 可信性检查扩展）、sandbox integrity 检查（`cba716c0f1ca` 系）。

上游许可证与来源：Apache-2.0，`d8acdf1`（本项目 sandbox.rs 既有来源标注）＋
本轮核查引用的各提交哈希（记录于 §3）。

## 5. 验证与审查状态

| 项 | 状态 |
|---|---|
| RED/GREEN＋变异 | ✅ 见 §2 |
| 定向安全检查 | ✅ 沙箱对抗全集（A–G＋B1/B2）双口径全绿（e2e_sandbox 7＋sandbox_matrix_probes 9，注入态＋env -u） |
| spawn 失败路径 | ✅ pre_exec 标记在场时：程序不可执行（EACCES=13）、程序不存在（ENOENT=2）均正确返回 spawn Err——无误报成功/panic/挂起（`b2_fix_feasibility` 三段探针，真实错误码留档） |
| Clippy | ✅ 改动区零新增告警（存量 4 条系 clippy 首装暴露：sandbox.rs:402 unused-mut、:343-345 文档列表缩进、:76 复杂类型、:163 测试缝 dead_code——均为 033 既有代码，记独立任务不阻塞） |
| 跨平台可编译 | ⚠️ 验证缺口：Windows/macOS 交叉 check 在本容器于**存量 C 依赖层**失败（tree-sitter-bash/onig_sys/tree-sitter-powershell 需交叉 C 工具链，与本次改动无关）；本次 cfg 门控（unix / all(unix, not(linux))）沿用同文件既有范式（transport.rs 既有 `#[cfg(unix)] process_group`、proc_session 既有 pre_exec），静态语义三分支完备 |
| 代码审查 | ✅ code-review 技能双轴完成（审查者独立复跑 cargo check＋workspace tests＋19/19 单元＋bwrap 标志实测）。4 项发现全部核实并处置：①【阻塞·已修】B1 检查 `if let Ok(md)` 对非 NotFound 的 stat 错误 fail-open（v9fs 瞬时 EIO/ESTALE 场景静默跳过）→ 改为 match：NotFound/NotADirectory 落 bind（bwrap 显式失败），其余 Err 显式拒绝；②【文档·已修】部署前提补 glibc ≥2.34（libc 0.2.189 为 gnu 目标普通符号绑定、musl 无绑定——旧 glibc 链接期失败非静默；本容器 glibc 2.39）→ sandbox.rs 注释＋规格同步；③【测试健壮性·已修】两个 B1 单测夹具预清理（残留 EEXIST 会掩盖真实信号）；④【文档口径·已修】记录 §6「/dev/null 绑定读恒空」改为实测语义（本部署 open=EACCES，userns nodev 强制）。存量项（sandbox.rs 既有 `读取恒空` 注释措辞、clippy 存量 4 条）记独立任务不阻塞。修复后复验：单元 19/19＋b1 集成绿 |
| 全量验证 | ✅ 冻结后两轮：`env -u TCHAT_SANDBOX cargo check --locked && cargo test --workspace --no-fail-fast --locked`（独立构建目录）＝**69 目标 3524 passed / 1 failed / 3 ignored**（两轮一致）。当时唯一失败 `memory::workspace::tests::commit_does_not_execute_planted_hooks` 经收口轮根因分析**改判**：非「环境波动」而是测试隔离缺陷——并发超时测试的全局覆盖缝（`GIT_TIMEOUT_MS_OVERRIDE=300ms`）泄漏进读者测试（panic 报文中的「300ms 上限」即覆盖值指纹，默认预算为 60s），已修复并经负载压测 6/6＋run3 全量验证。**收口轮最终全量（run3）＝3528 passed / 1 failed / 3 ignored**：唯一失败 `seam_killpg_injection_deadline_not_fake_timeout` 为既有监控轮询竞态测试例外（10ms 窗口、负载下概率翻转、空闲 4/4 通过、与候选改动零交集、两种交错均契约内行为），例外声明与处置建议见收口记录 §3。日志：`2026-10-27-b1b2-full-run1/2.log`＋`-run3-final.log`；根因与例外归因：`2026-10-27-b1b2-closeout.md` |
| 沙箱显式复跑 | ✅ 冻结后 ambient `TCHAT_SANDBOX=1`（部署注入态，未手工设置）：三型集成 3+2+1 全绿（真实 bwrap 分支）＋e2e_sandbox 7/7（沙箱分支）＋sandbox_matrix_probes 9/9 |
| 全量环境口径（新发现，供后续复用） | 本容器无控制终端时：`script` 包装的 PTY（含 stty 设尺寸）会使 11 个 tchat-tui 渲染快照用例失真——根因＝PTY 存在性触发 `effective_stdout_color_level` 着色路径，快照按无色生成；`docker exec -t`（验收口径）之所以通过＝未转发 TERM。**验收等价口径＝PTY 存在＋`TERM=dumb`**（python pty 预设 200x50＋TERM=dumb：tchat-tui lib 2163/0＋security_untrusted_text 12/12 全绿，T1 既有结论的成因补全）。调查过程日志：`2026-10-27-b1b2-full-tty-investigation.log` |
| 性能 | B1/B2 使每次工具启动**增加固定数量系统调用**（1×stat＋1×close_range，O(1)，无随规模增长项）；此前记录的三型集成用例时长对比（基线 0.40/1.02/0.22s → 加固后 0.05/0.84/0.07s）仅为观察值，**不等同正式性能基准通过**；全量性能基线不重复（目标 §7：只按实际受影响范围） |

## 6. 剩余边界（如实声明，加固后仍成立）

- B1：检查到 bwrap 挂载之间的 TOCTOU 窗口、沙箱运行期沙箱外新建硬链接——仍是
  边界（别名不可枚举，路径级遮蔽无法表达 inode 级保护）；本检查只阻断启动前已
  存在的别名。
- B2：沙箱运行期载荷**自身**打开的 fd 不属继承面（含打开遮蔽路径——本部署
  实测 open 为 EACCES：userns 内非特权 /dev/null bind 被内核强制 nodev，
  与矩阵证据一致；无论 EACCES 还是读到空设备，均取不到正文——审查轮口径
  修正）；launcher 在工具运行中新开 fd 不影响已启动载荷。
- 「内容经任何别名/fd 都不可得」仍非承诺（见规格修订总评口径）。

## 7. 交付边界

- 全部开发与实测在 Docker 容器内（独立构建目录）；未读真实 .env 正文（仅元数据
  nlink=1 核对）；未改 seccomp/账号/sudoers/持久开关；未替换部署二进制；未停止
  用户实例；未提交/合并/推送（候选＝工作树未提交改动）。
- 合并注意：主工作树存在并行 spec-sync 任务对 `docs/specs/033-tool-sandbox.md`
  状态行的未提交修改（「尚未部署→已部署」），本工作树基于提交版追加 §B1/B2
  加固轮——合并时需两者叠加；`rules/project-lessons.md`（主工作树未跟踪）与
  本工作树无交集，经验沉淀在合并时统一归位。

## 8. 收口轮（2026-10-27，随后）

两项验收收口（全量失败归因处置＋B1 元数据错误证据）完成：①memory 全量失败真根因
＝测试覆盖缝泄漏（非环境波动，§5 全量行已改判）并修复；②发现并修复 B1 链路级
fail-open（`exists()` 吞 stat 错误 → 单次 metadata 三分类），新增 3 单元＋1 端到端
探针；③最终全量 run3＝3528/1/3，唯一失败为 seam 既有竞态例外（供用户决定处置）。
增量改动、RED/GREEN、归因链与例外声明全部见
[2026-10-27-b1b2-closeout.md](2026-10-27-b1b2-closeout.md)（原始日志
`2026-10-27-closeout-*.log` 六份）。

**2026-10-28 收口完成**：③的 seam 例外已按方案 a 确定性修复（会话级 `cfg(test)`
监控门控缝，断言零变化，变异检出力验证；与 B1/B2 产品改动分开记录于
[2026-10-28-seam-gate-fix.md](2026-10-28-seam-gate-fix.md)），最终验收全量
（PTY＋TERM=dumb 验收等价口径，`--locked --offline` 串行）＝70 目标
**3529 passed / 0 failed / 3 ignored**（exit=0，
`2026-10-28-b1b2-final-full-pty.log`）——**待交付候选验证通过**；候选未提交/
合并/推送，部署版本仍未替换。
