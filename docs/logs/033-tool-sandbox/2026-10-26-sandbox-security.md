# 033 工具执行面沙箱安全攻防测试报告（2026-10-26）

范围：工作树 `/workspace/.worktrees/033-tool-sandbox`（分支 feat/033-tool-sandbox，033 未提交改动）。
威胁模型：027 R1 延续——驾驶开发 Agent 的模型/工具链经工具执行面（exec_command／MCP stdio／apply_patch）读取 `/workspace/.env`（真实凭据唯一值存储）。
约束遵守：全部验证使用临时目录与 FAKE 假凭据；未读取真实 `/workspace/.env` 正文；未安装组件、未改容器/账号/sudoers/ACL、未调用真实模型服务；未修改 `src/` 任何产品代码；argv 层临时测试用后已删除，`tests.rs` 已按字节还原（`cmp` 一致）并复验基线 13 用例全绿。

## 0. 结论速览

| 编号 | 发现 | 严重度 | 判定 |
|---|---|---|---|
| G1 | exec `workdir` 模型可控且无约束，沙箱 deny/可写根锚定其上——换 workdir 即可让真实 `.env` 完全脱离遮蔽 | **【阻断】** | 缺口（argv 层实测复现） |
| G2 | 非 canonical 可写根（含 `..`）与 deny 词法失配：遮蔽先于重叠 bind 排布，bwrap 挂载序后写者胜，遮蔽被重开 | **【阻断】**（G1 的第二跳板，单独可复现） | 缺口（argv 层实测复现） |
| G3 | 同 uid＋不 unshare-pid：工具可 kill 主进程；`/proc/<主进程>/environ` 可读（yama=1 只拦 ATTACH 不拦 READ，实测 rc=0；本环境父进程 environ 含 `ANTHROPIC_AUTH_TOKEN`） | 高危 | 缺口（同 uid 边界弱化中「本项目特有」部分；027 uid 边界移除后的回归面） |
| G4 | 上游 WSL interop 遮蔽被裁剪：MZ 头文件 execve 实测触发宿主 WSL `/init` 处理器（窗口侧执行面可达，可绕过全部 Linux 挂载策略读 `\\wsl.localhost` 面） | 高危（条件：WSL interop 启用——本部署实测活跃） | 缺口（与上游偏差项升级） |
| G5 | bwrap 经主进程 `PATH` 解析且结果跨会话复用：PATH 含工作区内可写目录时可被持久化栽赃假 bwrap（假沙箱＝无沙箱） | 中（条件：PATH 组含可写目录；当前容器 PATH 纯系统目录） | 部署条件性缺口 |
| G6 | MCP 接入 `envs(child_env)` 后接 `env_clear()`：白名单环境被 std 语义整体清除（注释声称"随后覆盖"与行为不符）；净效果由 `DEFAULT_ENV_VARS` 兜底，安全无回归、MCP 功能等价 | 低 | 实现瑕疵（机制注释错误） |
| G7 | `.env` 理论上可注入 `PATH`/`TERM`（dotenv 不覆盖已存在变量 → 正常部署不可行；仅 env 缺失的边缘环境可成立） | 低 | 加固建议 |
| D1 | 规格写「网络面用 `--unshare-net` 一档替代」，实现实为网络保留（代码注释"可加"），措辞不一致 | 低 | 文档偏差 |

failIfUnavailable（要求 2）：三面接入**未发现任何「不可用却无沙箱执行」路径**——见第 2 节。
环境面（要求 3）：`TCHAT_SANDBOX` 已收录 `TRUST_CONTROL_VARS` 且大小写不敏感＋dotenv 不覆盖，`.env` 无法开启沙箱——见第 3 节。
argv 组装（要求 1）：符号链接／`..`／`.`／重复组件／deny＝可写根／deny 为祖先等构造全部 fail-closed 或正确遮蔽，**唯 G1/G2 两条经模型可控 workdir 的组合路径成立**——见第 4 节。

## 1. 测试环境

| 探针 | 命令 | 结果 |
|---|---|---|
| bwrap | `which bwrap` | 不存在（argv 层验证；端到端沙箱行为按上游挂载语义推证，已在各项标注） |
| yama | `cat /proc/sys/kernel/yama/ptrace_scope` | `1` |
| 内核/能力 | `grep -E 'Seccomp|CapBnd' /proc/self/status` | `Seccomp: 2`、`CapBnd: 00000000a80425fb`（与 033 可行性探针一致） |
| WSL interop | `printf 'MZ' > fake.exe && ./fake.exe` | 触发宿主 WSL `/init`（`WSL ERROR: UtilGetPpid... Failed to parse /proc/.../stat`）——interop 处理器活跃 |
| /mnt/c | `ls /mnt` | 空（无直接 Windows 盘挂载；窗口侧唯一通道＝interop） |

测试期间源码被并行实现任务修改过一次（sandbox.rs 537→542 行，mtime 14:51）：「deny 为可写根祖先」由静默追加改为显式拒绝（fail-closed）。本报告 S2 结果即硬化后行为；硬化方向与本次攻防结论一致。

## 2. failIfUnavailable 攻防（要求 2）——通过

全库进程生成点核查（`grep -rn 'Command::new' src/app/src`，排除测试）：仅 proc_session(build_tool_command)、apply_patch(沙箱子进程)、sandbox(探测干跑)、view_image(mkfifo 固定)、memory/workspace(git 固定 argv)。无旁路 spawn。

| 接入面 | 不可用路径的行为 | 证据 |
|---|---|---|
| exec_command | `tool_launch_argv` Err 经 `?` 上抛 → `ExecResult::error("进程启动失败：{e}")`，不回退 | proc_session.rs:1054、:238-240 |
| MCP stdio | `tool_launch_argv(...)?` Err → `McpConnectError("启动服务器进程失败")`，服务器不启动 | transport.rs:209、connection.rs:263-269 |
| apply_patch | `Ok(None)`＝显式错误「沙箱模式未启用」；`Err(e)`＝「沙箱不可用：{e}」；无进程内回退分支 | apply_patch.rs:136-144 |
| 探测层 | PATH 查找＋真实干跑（ro-bind＋userns＋cap-drop 执行 `/bin/true`），`Err` 文案同样 OnceLock 缓存；`Ok(None)` 仅在未启用时返回 | sandbox.rs:108-143 |

集成测试（本环境无 bwrap＝failIfUnavailable 分支实跑）：exec_sandbox 2、mcp_stdio_sandbox 1、apply_patch_sandbox 3，全部通过。
**未发现 unwrap/默认值/错误分支导致的静默回退。通过。**

## 3. 环境面攻防（要求 3）——通过（附 G6/G7）

| 攻击尝试 | 实测 | 判定 |
|---|---|---|
| `.env` 写 `TCHAT_SANDBOX=1` 开启沙箱 | `TRUST_CONTROL_VARS` 收录（catalog.rs:242）＋`eq_ignore_ascii_case`（:267）＋已存在不覆盖（:281）；`sandbox_requested()` 只读真实 env | 阻断 ✓ |
| `.env` 注入凭据进工具子进程 | exec/apply：先 `env_clear` 再 `envs(child_env)`（顺序正确）；白名单仅 PATH/HOME/TERM（sandbox.rs:76-88）；单测 `child_env_whitelist_excludes_credentials` 通过 | 阻断 ✓ |
| `.env` 注入 `PATH`（毒化 bwrap 解析/工具解析） | dotenv「已存在变量不覆盖」→ 正常部署 PATH 恒已存在，注入不生效；仅 `env -i` 边缘环境成立（G7） | 边缘 ✓ |
| MCP 面白名单 | `envs(child_env)` 后接 `env_clear()`——实证 std 语义为整体清除（`/tmp/wt033-sectest/envtest.rs`：HOME/TERM 变空、先前 PATH 丢失），child_env 实际未进入子进程；`build_child_env` 的 DEFAULT_ENV_VARS（HOME/PATH/TERM 等 11 键）＋mcp.json env_vars 兜底 → 净效果安全且功能等价（G6，建议调整顺序消除注释与行为背离） | 瑕疵 |

## 4. argv 组装攻防（要求 1）——两项【阻断】缺口

方法：向 `src/app/src/sandbox/tests.rs` 追加临时攻击测试（S1–S7，用后已删除），`CARGO_TARGET_DIR=/tmp/wt033-target cargo test --lib sandbox::`。最终一次运行：20 通过＋1 失败（失败即 G2 缺口复现，断言保持不放宽）。

### 4.1 通过项

| 用例 | 构造 | 结果 |
|---|---|---|
| S1 | deny＝可写根本身 | 整根 `--perms 000 --tmpfs --remount-ro` 遮蔽位于最后一次 bind 之后（过保护方向，fail-safe）✓ |
| S2 | deny 为可写根之外的可写祖先 | 显式拒绝组装：「deny 路径 … 是可写根的祖先…拒绝组装」（fail-closed）✓ |
| S3 | deny 含 `./`、`a/b/../../` 组件 | `absolute_lexical` 规范化后遮蔽到真实 `ws/.env` ✓ |
| S4b | workspace 含 `..` 且中间组件不存在 | 内核逐段解析 ENOENT → `exists()=false` → 可写根整体丢弃（写面 fail-closed），deny 仍按词法路径遮蔽 ✓ |
| S6 | 空命令 | argv 以 `--` 结尾且其后为空，bwrap exec 阶段自然失败 ✓ |
| 既有 T1–T8 | 符号链接可写根/deny 目标/cwd 别名、deny 穿可写符号链接、rootless deny、missing no-op | 13 项基线全绿 ✓ |
| 命令注入 | 模型命令置于 `--` 之后 | bwrap 旗标不可注入 ✓ |

### 4.2 G1【阻断】：deny 锚定模型可控 workdir，真实 `.env` 可被完全移出遮蔽

根因链：`exec_command` 的 `workdir` 是模型入参（exec.rs:44）→ `resolve_workdir` 仅 `cwd.join(w)`＋`is_dir()`（exec.rs:289-300；绝对路径整体替换、`..` 不规范化、不限于会话工作区）→ `policy_for(workdir)` 把**可写根与 deny 全部锚定**在该目录（sandbox.rs:178-186）→ 全盘只读基线（`--ro-bind / /`）**不阻止读**，deny 遮蔽是 `.env` 唯一读防线。

实测（S5，单元层）：

```
真实工作区 ws=…/s5-ws（含 .env），模型传 workdir=…/s5-other
policy_for(other).unreadable_roots 不含 ws/.env
生成 argv 中不存在任何针对 ws/.env 的遮蔽项
=> 沙箱内 `cat <真实工作区>/.env` 走全盘 ro-bind 面＝可读
```

可达性（S7）：`resolve_workdir(Some("../../../.."))` 返回含 `..` 的存在目录；`resolve_workdir(Some("/etc"))`＝`/etc`——任意存在目录可选中。更短链：`workdir=".."`（父目录）即令 deny 落在 `父/.env`，会话工作区 `.env` 脱防。
修复建议（主 Agent 处置）：沙箱策略锚定**启动期固定并 canonical 化的会话工作区根**（贯穿 ProcessManager／MCP／apply_patch），`workdir` 只影响 `--chdir`/`current_dir`；或 `resolve_workdir` 强制 canonicalize 后必须位于会话工作区内。现有 exec_sandbox 集成测试以 workdir 为断言基准，故测不出本缺口——修复后应补「workdir 指向工作区外→显式拒绝或策略仍锚定真实工作区」回归用例。

### 4.3 G2【阻断】：非 canonical 可写根与 deny 词法失配 → 遮蔽被后续 bind 重开

可写根不经 `absolute_lexical`（仅 deny 规范化），嵌套/祖先/rootless 分类全靠词法 `starts_with`（sandbox.rs:283-296、306-317）。当模型令 workdir 词法形如 `<ws>/sub/..`（先建 `sub`——工作区写面本就开放——再传 `workdir:"sub/.."`）时，内核把该根解析回 `<ws>`，而词法分类失配导致遮蔽被排进 temp 根的嵌套段、**先于**该根 bind。实测（S4a，断言保持失败＝缺口回归证据）：

```
--new-session --die-with-parent
--ro-bind / /
--dev /dev
--bind /tmp /tmp
--ro-bind /dev/null …/s4a-ws/.env                     ← 遮蔽真实 .env（词法规范化生效）
--bind …/s4a-ws/sub/..  …/s4a-ws/sub/..               ← 随后 bind，内核解析回 s4a-ws
--unshare-user --unshare-ipc --chdir …/s4a-ws --cap-drop ALL
-- sh -c "cat .env"
```

挂载序后写者胜：目录 bind 覆盖先前的 `/dev/null` 文件挂载 → 沙箱内 `cat .env` 读到真实内容。安全不变式应为「遮蔽必须位于一切会重开该子树的 bind 之后」。
修复建议：`writable_roots`（含 workspace）在分类与 bind 前一律 canonicalize；或检测 raw≠canonical 时沿用祖先分支先例显式拒绝组装（fail-closed）。

## 5. 进程面余留（要求 4）——G3 高危，含项目特有部分

| 探针（同 uid 子进程→父进程） | 命令 | 结果 |
|---|---|---|
| P1 kill 面 | `sh -c 'kill -0 $PPID'` | rc=0——**可向主进程发信号（SIGKILL＝DoS）** |
| P2 environ 读 | `sh -c 'cat /proc/$PPID/environ'` | rc=0——**yama=1 不拦 READ 模式（只拦 ATTACH）** |
| P3 对照（父读子） | 同向子进程 | rc=0 |

- 主进程原始 environ 在本类部署中含真实凭据：实测本环境父进程 environ 54 个变量、含 `ANTHROPIC_AUTH_TOKEN`（仅验证键名存在）。沙箱内工具扫描 `/proc`（不 unshare-pid → 主进程可见）即可读取，**绕过全部 bwrap 文件策略**。`.env` 经 dotenv `set_var` 载入的值通常落在 glibc 重分配区、不回写 `/proc/pid/environ` 原始块——该通道暴露的是 wrapper 导出的原始凭据；`/proc/pid/mem`（堆）需 ATTACH、被 yama=1 挡住（部署若 yama=0 则连 `.env` 值一并暴露）。
- 与上游对比：codex linux-sandbox `--unshare-pid`＋`--proc`——子命名空间内主进程不可见，kill/environ 双面关闭；Claude Code 沙箱同为同 uid，但同样配合 pid 面隔离。**同 uid 本身＝上游一致的边界弱化；「主进程在 /proc 可见可达」＝本项目不 unshare-pid 的特有缺口**（027 双账号下该面被 uid 边界挡住，033 移除后回归）。
- 关于「016 killpg 需要不 unshare-pid」：父命名空间向子命名空间发信号本就允许（killpg 以主进程视角的 pgid 定位），该取舍依据存疑，建议主 Agent 重估；至少应将「工具可 kill 主进程、可读其原始 environ」记入已知边界。
- 缓解建议：①评估恢复 `--unshare-pid`（重验 016 PTY/回收契约）；②启动后尽早从自身 environ 清除凭据（wrapper 导出面），或文档强制「凭据只经 .env/受保护配置，不进 wrapper environ」。

## 6. 与上游 Codex 实现偏差逐项结论（要求 5）

| 裁剪项 | 本项目威胁模型下结论 |
|---|---|
| seccomp | 无 R1 实质缺口，纵深减弱：cap-drop ALL＋userns 已移除 mount 类能力；保留完整 syscall 面（含 binfmt_misc 触发，见 G4）意味着内核攻击面与 G4 无第二道闸。网络保留是显式决策（与 D1 措辞矛盾），R1 只关心读面，不构成缺口 |
| WSL interop 遮蔽 | **实质缺口（G4）**：上游遮蔽正是防「MZ execve → 宿主 interop → 绕开 Linux 沙箱」；本部署 WSL2＋interop 活跃（实测触发 `/init`），沙箱内工具可自写 MZ 文件（/tmp 可写）或读任意挂载 PE 执行 → Windows 侧代码执行，经 `\\wsl.localhost` 可读真实 `.env`。建议恢复上游遮蔽或在部署要求中强制禁用 interop（`wslconfig` interop=false / 容器不透传 binfmt） |
| `--proc` | 单独看非关键：不 unshare-pid 时新 procfs 仍显示同命名空间的全部进程。实质面来自 unshare-pid 缺席（G3）——即 G3 的修复会连带需要 `--proc` |
| `--perms 000`＋`--ro-bind-data` → `--ro-bind /dev/null` | 读面等价（读取恒空）；差异仅在写行为（写入被静默丢弃 vs EACCES）与探测面（fstat 显 char 设备，工具可察觉被遮蔽——不泄露内容）。接受 |
| helper 进程裁剪（主进程直组 argv） | 未发现缺口；探测 OnceLock 缓存与 spawn 使用同一解析结果，无 probe/exec TOCTTOU 新增面（PATH 栽赃风险独立成立为 G5） |
| glob 展开/ProjectRoots 元数据裁剪 | 本项目 deny 恒为单文件，无需 glob；但 G1/G2 表明唯一 deny 的锚定可被转移——修复 G1 后此项无增量风险 |

G5（补充，中危·条件性）：`find_bwrap_in_path` 按主进程 `PATH` 解析且结果跨工具调用/跨会话复用。若部署 PATH 含工作区内目录（如 `<ws>/bin`），上一会话植入的假 `bwrap`（自有干跑必然"通过"）会在本会话首次工具调用时被执行——无沙箱、主 uid，直接读 `.env`。当前容器 PATH 纯系统目录，不成立；建议探测结果校验（root 属主且不在可写根下）或支持管理员经专用变量固定绝对路径。

## 7. 交付物与验证缺口

- 产品代码零改动；`src/app/src/sandbox/tests.rs` 已按字节还原（`cmp` 一致）；临时测试脚本存于 `/tmp/wt033-sectest/`（envtest.rs 等），不入库。
- 测试终态：`cargo test --lib sandbox::` 13/13 通过；集成 exec_sandbox 2＋mcp_stdio_sandbox 1＋apply_patch_sandbox 3 全通过。
- 验证缺口：无 bwrap——G1/G2 的 argv 层证据确凿，但「挂载序后写者胜→遮蔽重开」的最终确认依赖 bwrap 环境端到端复跑（建议修复后补：沙箱内 `cat <真实工作区>/.env` 必须为空/报错的双分支断言）。G4 的窗口侧实际执行以宿主 `/init` 被触发为界，未（也不应）投放真实 PE。

## 8. 修复复验（2026-10-26，第二轮）

对主 Agent 六项处置逐项复验。方法＝只读核查＋定向测试（`CARGO_TARGET_DIR=/tmp/wt033-target`）；产品代码零改动。实测：`cargo test --lib sandbox::` **14/14 通过**；exec_sandbox 2、mcp_stdio_sandbox 1、apply_patch_sandbox 3 **全绿**（与主 Agent 声明一致；exec_command 18／mcp_stdio 35 全量由主 Agent 已跑，未重复）。

| 项 | 复验结论 |
|---|---|
| G1 锚定 | **通过**。接口改为 `tool_launch_argv(command)`（sandbox.rs:64，无目录参数），锚定根＝主进程 cwd 的 canonical 形态（:72-73）；三调用点（proc_session.rs:1055、apply_patch.rs:133、transport.rs:210）均只能传命令、无法传入目录，exec `workdir` 仅余 spawn `current_dir` 语义；全库无 `set_current_dir`（锚定稳定）。S5 场景由 exec_sandbox 锚定根断言覆盖（workdir 故意指向别处，断言锚定根 `.env` 遮蔽，有/无 bwrap 双分支）；S7 的「workdir 影响策略」攻击面随接口删除整体消失。残余说明：模型仍可将子进程 cwd 设到任意存在目录，其下 `.env` 在只读基线内可读——与规格「全盘只读＋仅工作区 .env deny」一致，非 R1 缺口 |
| G2 词法清洁 | **通过**。`create_filesystem_args` 拒绝 `absolute_lexical(root)≠root` 的可写根（sandbox.rs:275-289），`non_canonical_writable_root_fails_closed` 精确复现本报告 S4a 形态（`sub/..` → Err「词法非清洁」）；anchor（canonicalize）与 temp（canonicalize，sandbox.rs:199-206）均无失配输入源；符号链接可写根保留上游 canonical bind＋deny remap（T1e 绿）。残余组合核查：Rust `components`/`PartialEq` 将 `.`、`//`、尾斜杠规范化——这些形态与 canonical 相等，内核解析与 starts_with 分类一致、无失配；含 `..` 一律拒绝；不存在根走 `exists()` 丢弃（S4b，fail-closed 方向）。未发现残余失配组合 |
| G4 遮蔽 | 遮蔽对位置正确：`--tmpfs /run/WSL`＋`--tmpfs /proc` 位于 filesystem args 之后、unshare 之前（sandbox.rs:236-244），T1 期望已同步（tests.rs:54-58）；两根均非可写根，「最后遮蔽覆盖先 bind」不变式成立；`--tmpfs` 目标缺失时 bwrap 自建挂载点，非 WSL 部署不受影响。**但发现新的交叉回归，见下 G4-R1** |
| G5 PATH | **部分覆盖**。跳过「canonical 化后位于主 cwd 之下」的候选（sandbox.rs:152-165），工作区植入面已关；但 temp 可写根未排除——PATH 含 `/tmp`（或相对 PATH 项解析到 /tmp）的部署中，模型仍可在 /tmp 植入假 bwrap（自有干跑必过）。维持建议：候选 canonical 路径同时排除 temp 可写根＋属主/权限校验，或支持管理员固定绝对路径。严重度维持「中（条件性部署失误面）」 |
| G3 边界 | **读面已缓解（更正）**：`--tmpfs /proc` 落地后，本报告第 5 节 P2 通道（`/proc/<主进程>/environ` 读取）在沙箱内**不再可达**——该节描述为修复前状态。余留面：kill（无 /proc 时需 `kill -0` pid 扫描定位主进程，成本高但可行），维持「同 uid 已知边界」记录；恢复 `--unshare-pid` 仍为上游一致的完整解。「wrapper 最小化 environ」部署要求可保留为纵深卫生项，不再是沙箱内必要条件 |
| G6 顺序 | **通过**。transport 现为 `env_clear()` 在前、白名单 `envs(sandbox_env)` 与 mcp.json `envs(env)` 依序注入（mcp env 可覆盖白名单＝既有显式 env 契约），注释已更正（transport.rs:210-232）；mcp_stdio_sandbox 新增 `HOME=锚定根` 与污染键拒绝断言 |

### G4-R1【高危·功能回归，非安全缺口】：`--tmpfs /proc` 使沙箱模式 apply_patch 的 Update 全部失败

`--tmpfs /proc` 清空沙箱内 procfs，而 apply_patch Update 的 F2 写绑定经 `/proc/self/fd/<fd>` 重开已判型 inode（apply_patch.rs:889-898，`write_via_validated_fd`，Update 唯一写入路径，调用点 :1163）：procfs 为空 → open 必然 ENOENT → 走显式错误分支（「需要 /proc 已挂载」，fail-closed，无泄露、无静默回退——故不构成【阻断】）。Add（`create_new` 直开）与 Delete 不经该路径、不受影响。

复现链（本环境无 bwrap，代码路径推证＋本地逻辑唯一性核查）：沙箱模式 → apply_patch 任意 Update 补丁 → 内部子进程内 `open("/proc/self/fd/N")` ENOENT → 该文件报错、`state_known=false`。本地集成测试不可见的原因：apply_patch_sandbox 三测直跑 `--internal-apply`（无 bwrap 包装），/proc 完好。

根因＝上游成对语义是 `--unshare-pid`＋`--proc`（新 procfs 实例保留 `/proc/self`），本适配单取 `--tmpfs /proc` 牺牲了 `/proc/self/fd` 消费者。修复建议（主 Agent 处置）：
1. 上游一致方案：恢复 `--unshare-pid`＋`--proc`——连带完整关闭 G3 的 kill/environ 面；需 bwrap 环境重验 016 PTY/超时组杀/回收契约（与主 Agent「无环境不盲改」的暂缓判断一致，本条为环境就绪后的首选）。
2. 若暂不恢复：保留 `--tmpfs /proc` 并为 Update 提供不经 `/proc/self/fd` 的写入路径（如 `O_NOFOLLOW`＋`O_NONBLOCK` 路径直开＋事后 dev/ino 复核——FIFO「不打开路径」性质弱化，需重新评估 004 F2 契约），并在 bwrap 环境补「沙箱内 Update 成功＋FIFO 替换仍被拒」回归。

### 复验结论

修复复验通过（2026-10-26）：G1/G2/G6 关闭，G5 部分覆盖（建议保留），G3 读面已缓解、kill 面为已知边界；**新发现 G4-R1（沙箱模式 apply_patch Update 不可用，功能高危、fail-closed）需主 Agent 处置**，非安全缺口、不构成【阻断】。
