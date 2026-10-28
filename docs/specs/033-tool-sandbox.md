# 规格：工具执行面迁移系统级沙箱（027 B 替代评估）

- 状态：**已交付本地 main（2026-10-27）**——任务提交 `9d26236`、合并提交 `53ebf05`
  （no-ff；main 自基线 a14a57c 无新提交，合并零内容冲突）。合并结果与已验收候选的
  内容一致性由 git 树级比对独立证明（`git diff feat/033-tool-sandbox main` 为空）；
  候选验收时未提交、无独立源码快照存档，一致性另依据执行记录确认（宿主验收基于
  该工作树内容运行），mtime 类比对仅作辅助证据。合并后 `cargo check --locked
  --offline` 通过（0 error；warning 均为 shims 存量，非本次引入）。验收证据复用
  宿主真实沙箱验收（Docker Desktop 29.8.2 独立验收容器）：12 项全 PASS（probe 干跑、
  .env 遮蔽、三型工具真实分支、PID 隔离、普通回归零变化、全量 68 目标 3506/0/3）；
  验收中发现并修复两处产品缺陷（/run/WSL 遮蔽条件化、MCP 环境注入顺序），证据与
  处置见 `docs/logs/033-tool-sandbox/2026-10-27-host-acceptance.md`。**共享 dev 容器
  已部署并默认启用沙箱**（2026-10-27，部署退出码 0、验证 8 PASS/0 FAIL、退出码 0），
  依据[部署记录 §9](../logs/033-tool-sandbox/2026-10-27-dev-deploy.md#9-实际执行记录2026-10-27宿主侧接续)；
  生产及其他环境不在该部署结论范围内。本次仅同步文档，未重复部署或运行验收。
  **2026-10-28：B1/B2 加固合并（任务 `03078d1`、并行状态行 `876ba2f`、合并
  `f7c4abe`）并更新正式二进制（部署后验证 22 PASS/0 FAIL，本地 mock 全链路直接
  驱动正式路径）**，见
  [部署收口记录](../logs/033-tool-sandbox/2026-10-28-b1b2-deploy.md)。
  **2026-10-28：沙箱进程生命周期修复合并（任务 `a3aef1c`、合并 `dc9ef7b`）并
  更新正式二进制（`713e4c28…`；部署后验证 22+32 PASS/0 FAIL＋取消契约新旧对照
  矩阵——新二进制沙箱态 17/17，旧二进制同条件复现缺陷即修复对照）**，见
  [部署收口记录](../logs/033-tool-sandbox/2026-10-28-lifecycle-deploy.md)。
  027 B 双账号机制已由本规格替代并从 main 删除。
- 更新时间：2026-10-28
- 证据：`docs/logs/033-tool-sandbox/2026-10-26-feasibility-probe.md`（评估轮）；实现记录见文末「实现轮收口」

## 1. 要做什么，有哪些要求

把工具执行面（exec_command、MCP stdio、apply_patch）的隔离机制从 027 B 的
「sudo 双账号降权」（`TCHAT_ISOLATED_TOOLS=<用户名>`＋sudoers 两条＋tchat-app 账号）
迁移为系统级沙箱，消除管理员建账号/改 sudoers 的部署前提。要求：

- 优先复用现成隔离工具（bubblewrap，与 Claude Code sandbox-runtime / codex-linux-sandbox
  同型），不搬入整套 Agent 框架、不自造沙箱原语。
- 覆盖面按「命令执行」界定：exec_command 与 MCP stdio 进沙箱；apply_patch 复用
  internal-apply 子进程模式（补丁 stdin→结果 JSON），仅替换外层包装。
- 沙箱不可用必须显式失败（failIfUnavailable 语义），禁止静默回退到无隔离执行。
- 凭据环境清理（Command env_clear）、上下文安全打开（open_nofollow_chain）、私有记忆
  仓库禁钩子（hooksPath=/dev/null）、fd 防继承四处独立修复**正交保留**，不属于替换面。
- 历史规格与证据（027）保留，后续注明被本规格替代的条件与范围；不整批 revert、不改写历史。

范围确定结论（2026-10-26 核查，行号为 8df9278 树）：

| 类别 | 位置 | 处置 |
|---|---|---|
| 替换 | proc_session.rs:1086 `build_tool_command` 隔离分支（sudo 包装） | 换 bwrap（或 self 包装器）argv；env_clear 保留 |
| 替换 | mcp/transport.rs:204 spawn 隔离分支（sudo＋env K=V） | 同上；mcp.json 显式 env 注入契约不变 |
| 替换 | turn.rs:348 分派＋catalog.rs:889 `isolated_tool_user()` | 判定改沙箱开关（如 `TCHAT_SANDBOX=1`）；变量名替换 |
| 替换 | apply_patch.rs:101-200 `apply_patch_isolated*`＋main.rs:61 `--internal-apply` | **保留子进程模式**，外层 sudo→沙箱包装；uid 返回字段（双账号身份直证）失去用途，改造为沙箱生效标记后移除 |
| 删除 | proc_session.rs:528 `kill_group_cross_uid`（sudo /bin/kill 跨 uid 补杀） | 沙箱内进程同 uid，killpg 直接有效，补杀整体不需要；连带 catalog.rs:880-882 `SUDO_BIN/ENV_BIN/KILL_BIN` |
| 删除 | exec/PTY 取消/超时路径的补杀调用点（735/1221） | 随上条回归普通 killpg 语义 |
| 保留 | context.rs:111/180 `open_nofollow_chain` | 独立 TOCTOU 修复，主进程不进沙箱 |
| 保留 | memory/workspace.rs:168 `hooksPath=/dev/null` | 记忆 git 不进工具沙箱，防线仍需要 |
| 保留 | 三处 env_clear、fd CLOEXEC 对称关闭 | 凭据双防线（路径面＋环境面）正交 |
| 保留 | context 竞态测试、记忆钩子测试 | 对应保留修复的回归 |

## 2. 哪些情况容易出错

- 把 Landlock 当读防线 → 规则是白名单并集无 deny，「工作区可读但 .env 不可读」无法表达
  （探针判定场泄露=True）；Landlock 只能做写面/网络面纵深。
- bwrap 缺依赖时静默降级 → 参照 Claude Code `sandbox.failIfUnavailable`：启动时探测，
  不可用即拒绝启用隔离模式并报错，绝不静默回退。
- 误删 internal-apply 子进程 → 沙箱方案仍需要补丁子进程（stdin→JSON 契约复用）。
- 执行面枚举不全 → 动态链接器/libc 需 EXECUTE、`>/dev/null` 需 /dev WRITE、TMPDIR、
  9p 挂载点 bind 方向——探针已各踩一次。
- 隔离测试并发互斥（027 F3 教训）：沙箱开关同为进程级环境，用例须进程级锁。
- 补杀删除后取消/超时语义回归：同 uid 下 killpg 直接有效，但 PTY×沙箱×setsid 组合
  仍需真实验收（027 遗留验收项平移）。

## 3. 重要做法，为什么这样做

- 包装器自再执行（current_exe 隐藏子命令）：Codex（arg0=codex-linux-sandbox）与本项目
  internal-apply 同型——沙箱 argv 组装集中一处，主进程保持无沙箱状态（上下文/记忆/会话
  面零影响）。
- 三面统一走同一包装入口，避免 exec 与 MCP 规则漂移；mcp.json 显式 env 经 Command::env
  注入（bwrap 不清环境，sudo env_reset 不再存在，注入语义反而简化）。
- 探测前置＋显式失败：沙箱可用性在启动时测定（bwrap 存在＋userns 可用＋试运行），
  与 027「不以部分结果宣称完成」同一原则。

## 4. 踩过的坑，怎样解决

（2026-10-26 探针，详见证据日志）
- `landlock_add_rule` 是 4 参 syscall（fd, rule_type, attr, flags=0）——漏参时 attr 指针被
  当 rule_type，EINVAL 且报错误导（两处 TypeError/errno=22 均因此）。
- 全拒绝场景后进程连自建临时目录都删不掉（REMOVE 未授）——Landlock 限制的进程内
  清理逻辑必须走授权面或退出后清理。
- 目录级 READ_FILE 授予连带全部子文件（并集无 deny）+ 全功能授予泄露=True——两条
  实测共同构成「Landlock 不能作 .env 读防线」的判定依据。

## 5. 先预测，再实测，最后确认是否合格

本轮（评估）实测汇总：bwrap/unshare/mount 能力均不可用；Landlock ABI 7 可用、对 9p
生效、继承成立，但模型不匹配。**结论：当前环境阻断，不进入实现**。

真实验收前提（管理员操作，本仓库不做；实现已先行，见状态行）：
1. 容器运行参数允许沙箱：userns 路线（`sysctl kernel.apparmor_restrict_unprivileged_userns=0`
   或 seccomp 放行 unshare(CLONE_NEWUSER)）或 CAP_SYS_ADMIN 加入 bounding set＋setuid
   bubblewrap；2. `apt-get install bubblewrap`。

上游复用与适配声明（commit d8acdf151577132d4ee1de47a4d1822ae5acb762，Apache-2.0，
源码 https://github.com/openai/codex；实现为单文件 `src/app/src/sandbox.rs`，
下方「sandbox.rs 头注」指其模块头注）：
- 直接来自上游（函数级保留）：mount 分层算法 `create_filesystem_args`、遮蔽系列
  `append_unreadable_root_args`/`append_existing_unreadable_path_args`、可写符号
  链接 fail-closed 检查 `first_writable_symlink_component_in_path`、`--dev` 最小
  设备树注释、`canonical_target_if_symlinked_path`/`remap_paths_for_symlink_target`/
  `normalize_command_cwd_for_bwrap` 逐字保留。
- 本项目适配：①策略类型本地化（上游 `FileSystemSandboxPolicy` 耦合 codex-protocol
  不搬；本项目固定极小策略：全盘只读＋**工作区与系统临时目录可写**［temp 对齐
  Claude Code 沙箱默认写面］＋`.env` 遮蔽）；②主进程直组 argv（不引入 helper
  进程）；③裁剪 proxy/WSLg/daemon/glob 展开（rg 依赖）/seccomp（seccompiler 为新
  依赖，离线不可装；**网络面当前默认保留**，如需断网可加 `--unshare-net` 一档，
  未在本轮实现）；④`--ro-bind /dev/null` 代替上游 `--perms 000`＋
  `--ro-bind-data <fd>`（std Command 跨 spawn 的 fd 生命周期；/dev/null 读取恒空
  安全等价）；⑤deny 路径不存在时 no-op（上游预创建空文件防沙箱内重建——本项目
  deny 目标是已存在的凭据文件，missing＝无凭据可保护，安全后果为零，声明为对
  上游的已审差异）；⑥deny 为可写根祖先的分支生产策略不可达，实现为显式失败
  （fail closed，需先补上游的遮蔽后重建后代挂载点逻辑）；⑦PID/IPC/用户命名
  空间＋`--as-pid-1`＋新 procfs **成对恢复**（上游 launcher 的 `--as-pid-1`
  插入与 mount_proc 语义；G4-R1/G3 修复——`/proc/self/fd` 消费者恢复、主进程
  退出沙箱 /proc 视图；跨 ns 的 016 契约重验列入 bwrap 环境验收）；⑧bwrap 探测简化
  （system 探测＋真实干跑，bundled 裁剪）；⑨「启动时探测，不可用即拒绝启用」
  实现为启动提示预告＋调用时逐面显式拒绝（语义等效）。

实现轮验收（预测→实测）：
- 预测：沙箱内 `id -un`＝主进程 uid；哨兵 env 不出现；工作区读写正常；`cat .env`（假值
  场景）被 bwrap 路径规则拒绝；apply_patch Add/Update/Delete 正常；MCP 显式 env 保留；
  取消/超时/PTY 全部回收；普通模式（开关未设）零变化。
- 合格：以上全部实测通过＋第 2 节各坑点回归＋全量验证绿；沙箱不可用时启动显式报错
  （构造缺 bwrap 环境验证 failIfUnavailable）。
- 分支规划：实现自 main 新建 `feat/033-tool-sandbox`（025 任务分支已删，不复用）；
  规格与本日志先行落盘，未提交（工作区由并行任务共用，HEAD 切换留待实现轮）。

### 实现轮收口（2026-10-26，本节按实际结果记录）

**TDD/测试记录**（全部假凭据/临时目录，普通模式回归零变化）：
- 单元（`cargo test --lib sandbox::`，13 passed）：argv 层级双测、deny 目录 tmpfs
  遮蔽、rootless deny 遮蔽、可写根 symlink→canonical bind＋deny remap、missing
  no-op、cwd 别名 `--chdir`、`TCHAT_SANDBOX` 三态判定、统一入口 fail-closed、
  白名单环境排除凭据键、探测假路径显式报错。
- 集成（环境双分支断言，无 bwrap 分支在本环境实跑）：exec_sandbox 2（管道
  failIfUnavailable／PTY 超时组杀）、mcp_stdio_sandbox 1（显式 env 保留＋污染
  env 拒绝＋服务器不启动证据）、apply_patch_sandbox 3（internal 入口＋Add／
  Update/Delete 双分支）。
- 敏感性证明（变异）：禁用 `first_writable_symlink_component_in_path` 调用后
  T2/T3（deny 穿可写 symlink fail-closed）2 failed；恢复后 13/13 绿，变异无残留。
- 普通模式回归：exec_command 18、mcp_stdio 35、apply_patch 既有用例随全量。

**事件：进程级 env 并发污染（发现→修复）**：沙箱用例最初与普通用例同文件，
`TCHAT_SANDBOX` 的 set 窗口污染并发运行的普通用例启动路径（exec_command 10/20、
mcp_stdio 31/36 失败）。修复＝沙箱用例拆入独立测试文件（exec_sandbox.rs、
mcp_stdio_sandbox.rs、apply_patch_sandbox.rs）——cargo 各测试文件为独立进程，
进程边界天然消除 env 污染；027 的 ISOLATION_LOCK 只锁隔离用例自身、未覆盖
同进程读者，是同类潜在缺陷（027 时未触发属侥幸），随本次拆分一并消除。

**已验证项／阻断项**（分列，不以部分结果宣称完整验收）：
- 已验证：普通模式零变化（定向 53 用例＋全量 3502）、failIfUnavailable 三面显式
  失败（真实错误路径实跑）、argv 组装与 fail-closed 全分支、env 白名单、
  internal-apply 入口（无沙箱直跑实跑）、安全攻防 G1/G2/G4/G5/G6 修复。
- 已知边界（安全攻防 G3 余留）：同 uid——沙箱内进程对主进程的**信号面**经
  `--unshare-pid` 关闭（主进程不在沙箱 PID ns 视图内）；跨 ns 的 016 PTY/取消/
  超时契约重验需真实 bwrap 环境（列入阻断清单）。`--tmpfs /proc` 的已知功能
  限制（G4-R1 旧态：沙箱模式 Update 不可用）随成对机制恢复**已关闭**。
- 阻断（需真实 bwrap 环境；**2026-10-27 已全部由宿主真实验收关闭**，逐项结果见
  `docs/logs/033-tool-sandbox/2026-10-27-host-acceptance.md`）：.env 遮蔽的实际读取拒绝（含 G1/G2 修复后的
  workdir 转移与 `sub/..` 场景端到端复跑）、同 uid 沙箱内 id -un／PTY/取消/
  超时回收（**跨 PID ns 的 killpg 契约重验**——016 全套隔离变体）、MCP 沙箱
  往返、apply_patch 三型沙箱内落盘（含 Update 恢复验证）、探测 dry-run 成功
  分支。复跑命令与前置条件见第 5 节验收清单。

### B1/B2 加固轮（2026-10-27 实施，2026-10-28 候选验证通过、合并 main `f7c4abe` 并随部署更新正式二进制——分支 feat/033-b1-b2-hardening 已合并保留）

背景与风险核实：`docs/logs/033-tool-sandbox/2026-10-27-b1-b2-risk-verification.md`
——B1（deny 目标被沙箱外**预建硬链接**别名读出正文）与 B2（launcher 遗留非
CLOEXEC fd 经 `/proc/self/fd` 读出）为与冻结上游 d8acdf1 共有的路径级遮蔽/fd
面边界（上游 bwrap.rs/linux_run_main.rs/launcher.rs 均无对应处理）；矩阵实测
与上游核对后用户批准两项加固。实施记录：
`docs/logs/033-tool-sandbox/2026-10-27-b1-b2-hardening.md`。

- **B1**：`sandbox.rs::append_unreadable_root_args` 组装时（每次工具启动复查，
  不缓存安全结论）对 deny 目标 `fs::metadata()`（跟随符号链接取**实际常规
  文件**，不查链接本身）读 nlink，>1 即 Err（fail closed，三面共用的单一
  入口，与既有可写符号链接 fail-closed 同构）。目录不适用（nlink＝子目录
  计数）。**保证范围**：仅阻断「检查时已存在」的硬链接；检查到挂载之间及
  沙箱运行期的并发变化仍是边界——非完整竞态防护。真实 `.env` 元数据实测
  nlink=1，当前部署零触发。
- **B2**：新增 `sandbox::close_inherited_fds_mark_cloexec()`＝
  `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)`（**标记** CLOEXEC 而非关闭——
  在 pre_exec 里真 close 会切断 std 的 spawn 失败信道［Linux 为 SOCK_SEQPACKET
  socketpair］，execve 失败会被误报为 spawn 成功），经 pre_exec 注入三个工具
  spawn 点的**沙箱分支**（proc_session `build_tool_command`［管道＋PTY 共用］/
  mcp transport［tokio pre_exec］/ apply_patch internal-apply）。stdio 0/1/2、
  主进程 fd、载荷运行时自开 fd 均不受影响；普通模式零变化；close_range 失败
  （旧内核 ENOSYS）＝显式失败不静默放行；部署前提＝内核 ≥5.11（CLOSE_RANGE_
  CLOEXEC 引入）＋glibc ≥2.34（close_range 封装；libc 0.2.189 为 gnu 目标
  普通符号绑定，musl 无绑定——旧 glibc 链接期即失败，非静默；审查轮补记）。
- **修订总评口径**：路径级「指定路径不可读」承诺不变且全部兑现；加固后
  「检查时已存在的同 inode 别名」与「launcher 遗留非 CLOEXEC fd」两条条件性
  读取绕过通道关闭；**不承诺**「内容经任何别名/fd 都不可得」——TOCTOU 窗口
  与运行期新建链接/新开 fd 仍是边界（如实声明）。B3 维持 022 容错契约不动。
- **测试**：单元 3 例（多链接拒绝／单链接控制／rootless 符号链接跟随实际
  文件 nlink）＋集成 b1 三面拒绝（无执行副作用）＋b2 三探针（launcher→真实
  CLI 全链路／库级管道面／MCP 面 fd 扫描）＋spawn 失败信道探针（execve 失败
  仍返回 Err，防「真 close」型回归）。RED（未修复代码上 5 例真实失败记录在
  案）→GREEN→变异证明（移除 transport 注入→MCP 面测试失败）。
- **收口轮（2026-10-27）**：①B1 分类收口——`exists()` 对任何 stat 错误都返回
  false，会把非 NotFound 错误静默当 missing 跳过检查（fail-open），改为单次
  `fs::metadata` 三分类：NotFound/NotADirectory → missing no-op（既有差异
  不变）、其余错误 → 显式拒绝（fail closed，启动前、三面共用入口）；单元 3 例
  稳定夹具（ELOOP／父目录 EACCES／ENOTDIR 控制组）＋端到端探针证明拒绝先于
  工具启动。②全量 flake 根因修复——`commit_does_not_execute_planted_hooks`
  的 300ms 超时＝并发超时测试全局覆盖缝（`GIT_TIMEOUT_MS_OVERRIDE`）泄漏进
  读者测试，读者持有 `OVERRIDE_GUARD` 串行修复（断言零变化）。③最终全量
  3528 passed / 1 failed / 3 ignored：唯一失败 `seam_killpg_injection_deadline_
  not_fake_timeout` 为既有监控轮询竞态测试例外（负载下概率翻转、与候选改动
  零交集、两种交错均契约内行为），处置建议供用户决定。2026-10-28 按方案 a 完成
  确定性修复（会话级 `cfg(test)` 监控门控缝：停靠于监控循环顶部、terminate 覆盖
  提交后自动放行；断言零变化，变异检出力验证；与 B1/B2 产品改动分开记录于
  `2026-10-28-seam-gate-fix.md`）。**最终验收全量（PTY＋TERM=dumb 验收等价口径，
  `--locked --offline` 串行）＝70 目标 3529 passed / 0 failed / 3 ignored
  （exit=0，2026-10-28）——待交付候选验证通过**；仍未提交/合并/推送，部署版本
  未替换。证据：`docs/logs/033-tool-sandbox/2026-10-27-b1b2-closeout.md`、
  `2026-10-28-seam-gate-fix.md`。

### 沙箱进程生命周期修复（2026-10-28，候选待验收——分支 fix/033-proc-lifecycle，未合并）

- **缺陷**：`--die-with-parent` 的 PDEATHSIG 锚定「创建者内核线程」——工具 spawn
  此前在随轮退出的 turn 线程内执行，轮结束即整树 SIGKILL：016 的跨调用/跨轮保留、
  取消不终止在沙箱模式全部失效（n2b/n2b2 部署态失败根因；MCP stdio 服务器同病）。
  归因与四变体对照（新旧二进制×沙箱/普通）见
  `docs/logs/033-tool-sandbox/2026-10-28-e2e-failure-attribution.md`。
- **修复**：①exec 面——ProcessManager 专属长寿创建线程执行 fork/exec（PDEATHSIG
  锚点＝管理器生命周期；仅串行化创建，执行/监控照旧并发；创建线程失败＝显式报错
  fail closed；close() 先回收、线程随通道关闭后结束）；②MCP stdio 面——spawn 经
  manager runtime 的 `tokio::spawn` 移交核心 worker（常驻线程，非会退休的
  blocking 池）；③apply_patch 面——子进程同步完成于调用内，无需接入；④s103 测试
  沙箱分支改外层 argv 扫描判定回收（ns 内 pid 不可跨边界使用，僵尸天然排除）。
  `--die-with-parent` 与全部隔离/安全配置保留（宿主异常退出仍保证载荷停止执行）。
- **上游参考**：openai/codex issue #46702（同机制：PDEATHSIG 锚定创建线程、Tokio
  blocking 线程退休触发；**未合入上游**，0.155–0.162 均核查存在；issue 内候选方案
  为持久 spawn 线程——与本地同向；未复用上游代码）。
- **验证**：新增回归 `sandbox_lifecycle.rs` 四用例（管道/PTY 跨轮保留＋write_stdin
  管理/输入、MCP 跨轮存活＋reload 外层回收、宿主 SIGKILL 后载荷停止——活进程与
  僵尸区分）；RED（修复前沙箱注入态 3 FAILED）→GREEN（4/4）→env -u 对照 4/4；
  n2b/n2b2 沙箱注入态通过（修复前 FAILED）；完整 e2e_acceptance 双口径 143/143
  （修复前沙箱态 140/3）；标准验收口径最终全量 **71 目标 3533/0/3（exit 0）**。
  记录：`docs/logs/033-tool-sandbox/2026-10-28-lifecycle-fix.md`。
