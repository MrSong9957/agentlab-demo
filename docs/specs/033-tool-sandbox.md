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
  027 B 双账号机制已由本规格替代并从 main 删除。
- 更新时间：2026-10-27
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
