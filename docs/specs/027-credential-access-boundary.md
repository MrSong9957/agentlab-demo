# 规格：凭据访问边界（.env 原位）

- 状态：实现中（受阻）（2026-10-26 修订三：方案 B 产品部分已按 TDD 实现——隔离模式经 `TCHAT_ISOLATED_TOOLS` 启用，五项 RED/GREEN 与定向回归全绿；**tchat-app 全链路为管理员部署前提，产品实现不解除"受阻"状态**——产品已交付本地 main（8df9278），管理员部署及强制隔离验收未完成（部署/回滚清单见附录）；`.env` 保留原位、内容不变）
- **替代注记（2026-10-26，033）**：本规格方案 B 的双账号产品机制（`TCHAT_ISOLATED_TOOLS`、sudo 包装、跨 uid 补杀、uid 身份直证字段）已由 033 删除并以系统级沙箱（bubblewrap，`TCHAT_SANDBOX`）替代——见 `docs/specs/033-tool-sandbox.md`；**保留项**：上下文安全打开（B⑥）、凭据环境清理、文件句柄防继承、记忆仓库禁钩子（B⑤）、internal-apply 子进程模式（033 复用）。管理员部署前提随之改变（不再需要建账号/sudoers，改为沙箱运行环境，见 033 第 5 节）。
- 更新时间：2026-10-26

## 1. 要做什么，有哪些要求

在**不移动、不改写 `/workspace/.env`** 的前提下，评估并尽量收窄开发 Agent（ZCode、Agent shell 等）对真实凭据的读取面，同时保持应用（terminal-chat）正常读取。若当前权限模型下无法实现强制限制，如实返回阻断与所需管理员操作。

背景事实（2026-10-25/26 核实）：

- 运行账号 `dev`（uid 1000）；`/workspace` 为 Windows 宿主 `D:\` 的 9p（drvfs）挂载——容器内权限位只是其中一层，宿主 Windows ACL 是另一层（宿主层管理超出本仓库范围）。
- `/workspace/.env`（0600，dev）为真实密钥唯一值存储；`main.rs:60` 启动时从 cwd 加载；shell 未导出、compose 不传递。
- 同 UID 进程对同用户文件无文件级强制边界：`0700/0600`、F7 元数据审计与 AGENTS.md 提醒都**不能单独作为"强制禁止读取"的完成证据**。

要求：

- 有效提权能力核实：不能仅凭"无 sudo/wheel 组"下结论——逐一实测 `sudo`/`su`/`doas` 可用性、枚举 setuid 位二进制、核对 `/etc/sudoers` 可读性与内容；结论基于实测。
- 假文件边界验证：以临时目录中 0600 假凭据文件实测 Agent 自身 Shell 与既有工具（ZCode 快照机制）的读取/留存边界——预期可读，作为"无强制边界"的直接证据。
- 成熟机制优先：只评估既有系统机制（文件属主/ACL、seccomp、独立 UID 运行、容器隔离、docker secrets 等成熟方案）的适用性，不自造沙箱。
- 应用可读性保持：任何方案不得破坏 `terminal-chat` 从 cwd `.env` 的正常加载（现有行为回归测试为准）。
- 范围外：不移动/不改写 `.env`；不新增 `TCHAT_DOTENV_PATH`；不修改 ZCode 二进制；不调用真实付费服务；真实凭据不进入聊天、测试、日志或备份。

## 2. 哪些情况容易出错

- 把"无提权组"当成"无提权能力" → 必须实测 sudo/su/setuid 清单。
- 把 0600/AGENTS.md 提醒当强制边界 → 假文件实测推翻；如实分层：权限位挡其他用户，不挡同 UID。
- 为达成"强制"引入自造沙箱/setuid 助手 → 新攻击面大于收益；只评估成熟机制。
- 评估结论替代执行 → 本规格允许的终态之一是"当前权限模型下不可实现＋阻断清单"，不得为完成而虚报。

## 3. 重要做法，为什么这样做

- 实测优先于推断：提权能力与读取边界都以实际命令/假文件验证，结论可复跑。
- 阻断即结论：同 UID 无内核级边界时，强制隔离的可行路径（独立 UID 运行应用、容器拆分、宿主层 ACL）都需要管理员级环境变更——列为待决定项而非本批实现。

## 4. 踩过的坑，怎样解决

- 2026-10-04 ZCode 工作区快照事故（见 025）：防复发的可达性收窄在本规格修订（`.env` 原位）下退化为"流程约定＋提交扫描（026）＋脱敏（025）"，强制读取限制待管理员级决策。

## 5. 先预测，再实测，最后确认是否合格

- `sudo -n true` / `su -` / `doas` → 预测均不可用（以实际退出码为准）→ 实测：**预测被推翻**——sudo 可用（`sudo -l`＝`(ALL) NOPASSWD: ALL`）；su 拒绝（无 root 口令）；doas 不存在（2026-10-26-boundary.md）。
- setuid 二进制清单 → 预测仅系统标准项（passwd/mount/su 等），无异常自装项 → 实测：11 个标准系统工具，无异常 ✓。
- `/etc/sudoers*` 可读性与内容 → 预测 dev 无任何规则 → 实测：**预测被推翻**——root 0400，dev 的 NOPASSWD: ALL 来自容器基础配置（boundary.md）。
- 0600 假凭据文件：Agent Shell `cat` → 预测可读 → 实测：可读（同 UID 无强制边界）✓；ZCode 快照范围 → 预测仅工作区 → 实测：**预测被推翻**——2025 个 artifact 跨 /workspace、/tmp、/vault、/home 四根（boundary.md）。
- 应用读取回归：CLI 从 cwd `.env`（假值副本场景）加载 → 预测行为不变 → 实测：沿用既有 dotenv 测试，全量绿 ✓。
- 真实 `.env` 原位、内容不变 → 预测布尔＝true → 实测：true（仅输出布尔；2026-10-26 复验 dev 仍可读＝边界未实施）。
- 9p 属主/权限位/sticky/写位 → 预测挂载生效则强制 → 实测：均强制（chown+0400 拒非属主读、sticky 拒非属主删除、写位拒他人追加；测试陷阱：sticky 须用他人属主目录验证——目录属主本有权删除）（2026-10-26-wrapper-revision.md）。
- 证据：命令与布尔/计数输出存 `docs/logs/027-credential-access-boundary/`。
- 验收：验收人按本节复跑；若终态为阻断，本节如实标注"阻断＋待管理员操作清单"，不得标"通过"。

## 附录：管理员操作方案（2026-10-26 修订二，待决定执行；本批未实施）

前置事实（全部实测，见 docs/logs/027-credential-access-boundary/2026-10-26-wrapper-revision.md）：
dev 拥有 `(ALL) NOPASSWD: ALL`；应用与 Agent 同以 dev 运行；/workspace 为 Windows `D:\`
的 9p 挂载（metadata 生效：属主/权限位/sticky/写位均实测强制）；/workspace 顶层 **777
root:root 无 sticky**；`.env` 无 /vault 别名路径（同源挂载但内容不同，`/vault/.env`
ENOENT）；无 Docker socket/CLI（docker 仅经 sudo）；`/usr/local/bin`、`/opt` root 属主
0755，dev 不可写；`sudo -u` env_reset 生效（HOME/PATH/USER 重置，PATH→secure_path）。

### 已核查并需关闭的旁路（方案 A/B 共同前提）

| 旁路 | 证据（file:line／实测） | 关闭方式 |
|---|---|---|
| sudo 通用提权 | `sudo -l`＝NOPASSWD: ALL | 删除该授权，仅留白名单 |
| 二进制被 dev 替换 | /workspace/target 为 dev 可写；/opt 不可写 | root 属主副本部署（/opt/tchat/bin），部署时校验哈希 |
| sudo 参数注入（`-i`＝任意文件读原语） | main.rs:31-62 | sudoers 无参数形式 `/usr/local/bin/tchat ""` |
| PATH/git 劫持 | memory/workspace.rs:169 经 PATH 找 git | env_reset→secure_path（实测） |
| mcp.json 命令以应用 uid 注入 | manager.rs:495（user_dir＝HOME 系路径） | HOME=tchat-app（env_reset 实测），路径归管理员 |
| 模型端点导向（真实 Key 发往 dev 自选地址） | catalog.rs:874-878（cwd `config/` dev 属主）＋base_url 可配（catalog.rs:399） | wrapper 固定 `TCHAT_CONFIG_DIR`→root 属主配置（既有 env 支持，无需产品改动） |
| `.env` 被删换（DoS，非泄露） | /workspace 777 无 sticky | `chmod +t /workspace`（sticky 实测强制） |
| Docker 控制权 | 无 socket/CLI（实测） | 仅经 sudo，随收紧移除 |
| **R1 驾驶者经工具读凭据** | exec_command/proc_session.rs:1050、MCP/transport.rs:204 等全部子进程继承应用 uid | **方案 A 关闭不了**（见下）；方案 B 关闭 |

### 方案 A：管理员层身份隔离（不改产品；威胁模型＝阻断"顺带读取"）

1. `useradd --system --shell /usr/sbin/nologin tchat-app`；HOME=/home/tchat-app（root
   属主，内含 `.config/terminal-chat/mcp.json` 等应用私有配置，管理员维护）。
2. `chown tchat-app:dev /workspace/.env && chmod 0400`（9p 实测：dev 读被拒、属主可读）。
3. `chmod +t /workspace`（root 执行；防 `.env` 被删换）。
4. sudoers：删除 dev 的 `(ALL) NOPASSWD: ALL`；新增
   `dev ALL=(tchat-app) NOPASSWD: /usr/local/bin/tchat ""`（仅此一条、无参数）。
5. wrapper `/usr/local/bin/tchat`（root 属主 0755 脚本）：`cd /workspace`；
   `export TCHAT_CONFIG_DIR=/etc/tchat/config`（root 属主 0644，内容＝现 /workspace/config
   副本）；`exec /opt/tchat/bin/terminal-chat`（root 属主二进制副本）。
6. 应用启动方式：`sudo -u tchat-app /usr/local/bin/tchat`（无参数；图片等经 TUI 内操作）。

验证（全部不输出内容）：dev `cat .env`→拒绝；`sudo -u tchat-app test -r .env`→可读；
dev 直跑 `cargo run`→应用无法读 .env（预期失败面）；wrapper＋本地 mock 一轮对话→正常；
`sudo cat .env`→sudoers 拒绝；dev 删除/替换 `.env`→sticky 拒绝；Agent 工具链（ZCode/Claude
Code 快照、grep、误 cat）读 `.env`→拒绝（**2026-10-04 快照事故类复发路径就此关闭**）。

回滚：完整清单见下方「部署回滚」（B 节，含备份前提与独立恢复入口）——本方案适用
其中 sudoers/.env/目录三项与账号步骤；二进制/wrapper 为 A 不涉及。

**功能代价（如实）**：应用全部工具子进程以 tchat-app 运行——改写 dev 属主 0644 文件被
权限位拒绝（9p 写位实测），apply_patch/exec 对既有仓库文件的写入将失败；新建文件可行但
属主为 tchat-app，dev 侧后续协作受阻。⇒ 方案 A 适合"terminal-chat 只当持凭据的模型会话、
不在其中改仓库"的用法；若要在应用内编码，需方案 B。

### 方案 B（修订版 2026-10-26）：A ＋ 受控面降权——完整可决定方案（仅评估，不实施）

**权限边界总表**（主进程＝tchat-app；证据与假凭据实验见
docs/logs/027-credential-access-boundary/2026-10-26-plan-b-boundary.md）：

| 面 | 代码入口 | 现身份（＝A） | B 后 |
|---|---|---|---|
| exec_command | proc_session.rs:1050/1072（**无 env_clear，子进程继承含凭据的父环境**） | 应用 | dev |
| MCP stdio | transport.rs:204（env_clear＋配置 env） | 应用 | dev |
| apply_patch 读+写（进程内原语） | apply_patch.rs:804 | 应用 | dev（自再执行 helper） |
| 记忆 git（argv 固定） | workspace.rs:169/394 | 应用 | 保持 tchat-app |
| 记忆工具读写（根域限制） | mem_tools.rs:218-295 | 应用 | 保持 |
| 图片读取（格式门：guess_format 失败即弃内容、错误仅含路径） | view_image.rs:142/153/249-250 | 应用 | 保持 |
| 上下文注入（文件名固定＋符号链接拒绝） | context.rs:65-79/94/234 | 应用 | 保持 |
| 会话读写（私有目录） | session.rs:39-41 | 应用 | 保持 |

主进程内部文件操作已全部枚举（无其他生产落点）；须降权的只有 exec_command、MCP、
apply_patch 三类。

**最小产品改动**：①`spawn_as_dev(argv, env, cwd)` helper（内部
`sudo -n -u dev /usr/bin/env K=V … argv`，数组直传不经 shell）；②接入 proc_session
两处、mcp/transport 一处；③apply_patch「读+判型+写」整体改 dev 侧自再执行
（`/proc/self/exe --internal-apply`，补丁经 stdin；主进程不再直接写模型指定路径）。
④sudoers 增加 `tchat-app ALL=(dev) NOPASSWD: ALL`（只降权，不提权）。
⑤（可选纵深）run_git 固定 argv 增加 `-c core.hooksPath=/dev/null`——E6 证实固定
argv 不防已植入钩子，现有防线是 .git 不可达三层（见下）；此行消除记忆根属主误配置
时的钩子执行面。

**管理员操作草案**：方案 A 全部步骤＋上述 sudoers 一条＋部署含 B 改动的二进制＋
记忆根属主核对＋wrapper 追加 `ulimit -c 0`。
**启动方式**：与 A 相同（`sudo -u tchat-app /usr/local/bin/tchat`，无参数）。

**已验证边界（假凭据实测/复用＋定向测试）**：dev 读写开发文件正常；dev 读他人属主
0400 假凭据被拒；sudo 子进程环境零继承（父 env 内凭据不进工具——同时关闭现状
exec_command 的环境泄露原语）；父持有 fd 不透传；主进程 `/proc/environ` 跨 uid 拒绝；
dev 除固定 wrapper 外无再获 tchat-app 身份路径（再启实例的工具同为 dev）。
记忆 git 钩子面（E6）：固定 argv 不防已植入钩子（假 pre-commit 以 git 进程 uid 执行，
标记落盘属主＝该 uid）——实际防线＝.git 不可达三层：resolve_scoped_path 拒
`.git`/符号链接/越界（mem_tools.rs:218-258；`hidden_and_excluded_component_checks`
定向 1 passed）＋记忆根属主隔离（dev 写他人 `.git/hooks` 被拒）＋run_git env_clear
无 HOME 无 ~/.gitconfig。上下文读取（E5）：terminal-chat 对符号链接 AGENTS.md
lstat 拒绝跳过（context.rs:65-79/234；`t04…` 定向 1 passed，提示不含正文）；嵌套
claude CLI 端到端则**跟随符号链接**（假凭据标记进入模型请求）——属 dev 侧工具链，
A/B 后读不到真实凭据，已记录为使用注意（--capture 可能含跟随目标正文）。

**影响**：exec——子进程环境被 sudo 重置（PATH=secure_path，dev 自装路径工具不可见，
可用 env_keep 或装标准路径缓解，管理员取舍）；PTY 交互在 NOPASSWD 下无需口令
（requiretty 未启用，本轮旁证），但 pre_exec setsid＋sudo 组合与取消/超时进程组信号
须实测（验收项）；每次执行多一层 fork/exec（毫秒级）。MCP——配置 env 经
/usr/bin/env 注入，stdio/pgroup 语义不变。apply_patch——改子进程执行，原子写与 F2
inode 语义同二进制内保持。记忆/会话/图片/上下文——零改动。

**部署前备份（管理员执行，回滚的全部前提）**：
- sudoers：`cp -a /etc/sudoers /root/sudoers.bak-$(date +%F)`＋`ls /etc/sudoers.d/`
  逐份备份至 /root；visudo -c 确认备份可用。
- 二进制：记录当前 `terminal-chat` 产物路径与哈希（`sha256sum` 存 /root）；部署新
  二进制前保留旧产物目录（如 `mv /opt/tchat /opt/tchat.prev`）。
- `.env` 与目录元数据：`stat /workspace/.env /workspace` 输出（属主/权限）存
  /root；记忆根位置与属主记录存 /root。

**部署回滚（顺序执行；入口＝宿主机容器恢复通道，不依赖 dev 账号内任何状态）**：
1. **独立管理员恢复入口**：宿主机 `docker compose -f .devcontainer/compose.yaml exec
   <service> sh`（容器 root）或 `docker exec -it <容器> sh`——dev 失去通用 sudo 后，
   容器 root 仍是完整管理通道；宿主重启容器亦可复位运行态（文件位变更持久，需下列
   步骤还原）。
2. 恢复 sudoers：以备份覆盖 `/etc/sudoers` 与 `/etc/sudoers.d/*`（`visudo -f <备份>`
   校验后 `install -m 0440` 放回）；`visudo -c` 通过后生效。删除新增的
   `dev ALL=(tchat-app) …` 与 `tchat-app ALL=(dev) …` 两条。
3. 恢复 `.env` 原属主与权限：`chown dev:dev /workspace/.env && chmod 0600
   /workspace/.env`（值不变；与备份的 stat 记录核对）。
4. 恢复目录位：`chmod -t /workspace`（还原 777 无 sticky 原状）；删除 wrapper
   `/usr/local/bin/tchat`；二进制回滚＝恢复 `/opt/tchat.prev`（或删除 `/opt/tchat`
   后按记录路径还原旧产物）；`/etc/tchat`（wrapper 固定配置）删除或归档；记忆根
   （tchat-app HOME 下）按记录属主恢复或整体归档后由应用重建。
5. 账号（可选最后）：确认无残留进程后 `userdel tchat-app`。
6. 回滚验证：dev 直跑 `cargo run` 正常读取 `.env`（假值副本场景）；`sudo -l` 恢复
   NOPASSWD: ALL；gitleaks 提交扫描不受影响；项目提交扫描 hook 仍在。

**未关闭路径（如实，不称「隔离完成」）**：
1. 主进程自身持凭据（内存/env）：仅同 uid/root 可见（已证跨 uid 拒绝）；core dump 经
   wrapper ulimit 收敛；宿主/容器 root 仍在信任边界。
2. dev 可经 wrapper 启动并驱动新实例：其工具同为 dev，无凭据读取面，仅配额消耗（属
   「dev 可用应用」既定授权）。
3. 图片面依赖格式门：若未来放宽格式校验、或新增任意文本文件读取面（如 @file 引用），
   必须改走 dev helper 或同级控制——列为架构守则。
4. 记忆 git 保持 tchat-app：安全依据是 .git/配置/钩子面三层不可达（作用域拒绝＋属主
   隔离＋env_clear），**不是**固定 argv（E6 证实已植入钩子会以 git 进程 uid 执行）；
   若记忆根属主被误配置为 dev 可写，植入钩子将以 tchat-app 执行——可选纵深⑤
   （hooksPath=/dev/null）消除该误配置面。

### B 产品实现（2026-10-26 本批；启用条件、改动点与验收）

**隔离模式启用条件**：环境变量 `TCHAT_ISOLATED_TOOLS=<用户名>`（如 `dev`）非空即启用；
未设/空＝普通模式，既有行为与其他平台零变化。由管理员 wrapper 固定导出（环境不属
模型或 cwd 配置可篡改面）；非 unix 平台该变量被忽略。启动审计输出一行
「工具隔离模式：工具以 <用户名> 执行」使用户可见。

**产品改动（最小集）**：
1. `catalog::isolated_tool_user()`：唯一判定入口（读上述变量）。
2. exec_command（proc_session.rs spawn_piped/spawn_pty）：隔离时命令包装为
   `sudo -n -u <user> -- /usr/bin/env TERM=<主进程TERM> /bin/sh -c <cmd>`；
   子进程 Command `env_clear`（不继承主进程凭据环境），sudo `env_reset`（默认
   HOME/PATH/USER 重置为目标用户）——E1 已证零继承；显式透传 TERM 保交互行为。
   `/bin/sh` 与 sudo 用绝对路径（部署前提：sudo 位于 /usr/bin/sudo）。
3. MCP stdio（mcp/transport.rs StdioProcess::spawn）：隔离时
   `sudo -n -u <user> -- /usr/bin/env K=V… -- <command> <args…>`——mcp.json 显式
   env（build_child_env 契约）经 /usr/bin/env 在 sudo env_reset 之后注入保留；
   不经 shell（execve 数组直传）。
4. apply_patch：隔离时 turn 层改调 `apply_patch_isolated`——补丁文本经 stdin 传给
   `sudo -n -u <user> -- <current_exe> --internal-apply`，读、判型、写全部在 dev 侧
   子进程完成（复用既有 /proc/self/fd 同 inode 语义）；结果 JSON 经 stdout 返回解析。
   main.rs 增加隐藏入口 `--internal-apply`（读 stdin→run()→结果 JSON→stdout）；
   该入口仅接受 stdin 补丁、不触达模型工具契约。
5. 记忆 git（workspace.rs run_git_env 固定 argv）：增加 `-c core.hooksPath=/dev/null`
   ——E6 证实固定 argv 不防已植入钩子，此行使私有仓库钩子一律不执行；只作用于
   私有记忆仓库，不改全局 git 配置、不影响项目提交扫描 hook。
6. 上下文读取 TOCTOU 修复（E5 延伸＋安全攻防 F1＋收口三）：context.rs
   `load_source_text` 的打开本体为逐组件 openat 目录句柄链（每级 O_NOFOLLOW，
   目录组件另加 O_DIRECTORY）——任何组件（含可写父目录中的目录组件）在打开
   瞬间为符号链接即该级 ELOOP，检查-打开窗口不存在；ELOOP 按「符号链接」跳过，
   正文不被读取（逐前缀元数据检查保留仅作提示分类）。非 unix 维持 std 打开。
7. 测试接缝（默认零影响）：`--internal-apply` 结果附子进程真实 uid（执行身份
   直证，无需额外接缝）；context 接缝 `TCHAT_CTX_RACE_SEAM`（仅 `#[cfg(test)]`
   构建，probe 与 open 间可控暂停，供 TOCTOU 竞态回归）。

隔离判定变量 `TCHAT_ISOLATED_TOOLS` 属信任根控制变量（TRUST_CONTROL_VARS）——
.env 不得启用或改换隔离目标（安全攻防 F2）。隔离测试用例以进程级互斥锁串行（F3）。

**验收（假凭据/假钩子，普通模式回归零变化）**：
- 隔离 exec `id -un`＝目标用户；主进程哨兵 env 不出现在子进程环境；
  dev 正常读写开发文件、PTY 交互/取消/超时/进程组回收不回归（016 关键用例隔离变体）。
- MCP：隔离连接＋工具往返成功；显式 env 注入在隔离下仍有效；隐式默认 env 语义按
  build_child_env 契约不变。
- apply_patch：隔离下 Add/Update/Delete 正确；`--internal-apply` 标记接缝证明执行在
  dev 侧子进程；普通模式路径不变。
- 记忆仓库植入假 pre-commit 钩子→提交不执行钩子（标记文件不出现）；项目 hook 不受影响。
- context：probe→open 间隙替换为符号链接（可控接缝）→正文（假凭据标记）不进入上下文。
- 部署阻断（本环境无法验收，管理员执行）：tchat-app 身份下的全链路、sudoers 白名单
  收紧后 `-u dev` 仍放行、PTY×sudo×setsid 组合、真实凭据读取拒绝——列为部署前提，
  不以本环境结果宣称完成。

### 仍需用户决定的范围

1. 方案 A 的功能代价是否可接受（A＝应用不再当编码工具用）；
2. 是否立项方案 B（产品变更，另行规格与门禁；修订版见上节）；
3. dev 失去通用 sudo 后日常 root 操作（装包、端口绑定等）的替代方式；
4. 宿主 Windows ACL（9p 之外的独立层）是否加做。

密钥轮换不列入本方案决策项：按用户指令全面停止（2026-10-26，记录保留），由用户
另行安排。

### 信任边界声明

即便 A/B 全做：宿主 Windows 管理员、容器 root、9p 服务端仍在信任边界内；本方案消除的是
"dev uid 的开发 Agent 及其工具链默认可读 `.env`"这一已证实的暴露面。025 脱敏、026 提交
扫描、流程约定（AGENTS.md F7 审计）与本方案互补，不因方案实施而移除。
