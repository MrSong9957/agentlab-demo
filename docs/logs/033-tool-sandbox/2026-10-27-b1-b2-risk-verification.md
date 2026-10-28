# 033 B1/B2 风险核实与最小修复方案（2026-10-27 核实轮）

**性质：代码核查＋假凭据探针＋文档，未修改产品逻辑。** A–G 矩阵证据保留不变
（`2026-10-27-matrix-live-verification.md`）；本轮仅补 B2 真实 CLI 链路探针与
修复方案可行性探针（新增 2 用例，14/14 全绿，注入态与 `env -u` 两口径）。

**结论先行：**
- **B1（硬链接别名）**：前提＝沙箱外**预先存在**的同 inode 硬链接；三工具面产品链路
  真实可达；与冻结上游 codex `d8acdf1` **共有**（上游无任何 nlink 处理）；按当前
  规格字面（路径级遮蔽）**不违约**，但与 027/033 凭据保护意图存在承诺差距——建议
  规格补条目＋fail-closed 加固（方案 §2.3）。
- **B2（非 CLOEXEC fd 继承）**：前提＝launcher 预开无 CLOEXEC 的 fd；**本轮补齐
  真实 CLI 完整链路证据**（launcher→真实二进制→生产分派→bwrap→正文可读，沙箱/
  普通一致）；与冻结上游**共有**（上游无继承 fd 清理）；**非 033 规格缺口**（fd 面
  属 027 R1 正交防线＋部署信任边界）；建议沙箱分支 spawn 点 pre_exec 关闭 fd ≥3
  作为纵深（方案 §3.3）。

## 1. 修订总评（矩阵证据不变，承诺口径修正）

- **功能通过项**（A–G，证据同矩阵记录）：默认启用＋全链路多工具；exec/apply_patch/
  MCP 正常功能；指定路径（锚定根 `.env`）的相对/绝对/改 cwd/`sub/..`/符号链接读取
  全部拒绝；env 白名单（哨兵 env/HOME/PATH/TERM）；PID ns 隔离（environ/cwd/root/
  信号面）；取消/超时/正常完成回收；fail-closed 四场景三面；普通模式对照与网络边界
  如实记录。
- **条件性读取绕过**（本轮明确口径）：**B1** 沙箱外预建硬链接可读出 `.env` 正文；
  **B2** launcher 遗留非 CLOEXEC fd 可经 `/proc/self/fd` 读出。两者均**不违背**
  「指定路径不可读」承诺（遮蔽挂载在该路径上确实生效），但证明更强的承诺——
  「文件内容经任何别名都不可得」——**不成立**。硬链接读取成功不得记作正文保护通过。
- **现有容错行为**（契约，非缺陷）：取消后进程由可续接会话持有（016）、MCP 沙箱
  不可用时按 022 容错返回空快照（B3，保持观察项）、apply_patch Delete 触碰遮蔽
  路径报 EBUSY。
- **承诺区分**：033 规格/实现承诺＝**路径级**「`<锚定根>/.env` 不可读」；不承诺
  **内容级**「该 inode 经任何路径/fd 都不可得」。路径级承诺已在 A–G 全部兑现；
  内容级的两条残余通道即 B1/B2。

## 2. B1 核实（硬链接别名）

### 2.1 复现与前提（引用既有证据，不重跑矩阵）

- 全链路产品实证：`2026-10-27-matrixA-sandbox.log` `ENV_HARDLINK=1`（沙箱外预建
  `ln work/.env work/env-hardlink` 后，沙箱内 `cat env-hardlink` 读出唯一标记）。
- 沙箱内**自建**链接被拒（手工补充探针，产品同参 bwrap 子集）：
  `ln /tmp/…/.env self` → `Invalid cross-device link`（遮蔽路径已绑定 /dev/null，
  link(2) 跨设备失败）——沙箱内进程**无法**自行制造别名。
- 文件系统能力：`/workspace`（v9fs）与 `/tmp`（overlayfs）均支持硬链接
  （自有临时文件实测）——**攻击面在真实部署工作区实际存在**。

前提归纳：需要沙箱启动**之前**、在沙箱外（主进程不进沙箱，故指其他进程/用户操作）
对真实 `.env` 建立硬链接，且链接路径无需位于可写根内（读取走全盘只读基线）。

### 2.2 上游对照（冻结版本 codex `d8acdf151577132d4ee1de47a4d1822ae5acb762`）

已核对 `codex-rs/linux-sandbox` 三文件（bwrap.rs / linux_run_main.rs / launcher.rs）：

- **无任何 nlink/硬链接检查**（无 `st_nlink`、无 statx 链接计数读取；仅有的
  `FileIdentity{dev,ino}` 用于清理临时挂载点防误删，与读保护无关）。
- 上游 deny 实现（`append_unreadable_root_args`/`append_existing_unreadable_path_args`）：
  目录＝`--perms 000`＋`--tmpfs`＋可写后代重建＋`--remount-ro`；文件＝`--perms 000`＋
  `--ro-bind-data <nullfd> <path>`——**全部为路径级挂载**，与本项目适配差异
  （规格适配声明④：`--ro-bind /dev/null <path>` 替代 `--perms 000`＋`--ro-bind-data`）
  **不改变硬链接暴露类别**：挂载遮蔽的是路径，不是 inode。
- 结论：B1 为**与冻结上游共有的边界**，非本项目适配引入的回归；本项目的加固
  （若采纳 §2.3）将是超出上游的强化。

### 2.3 最小修复方案（nlink>1 拒绝启动；本轮未实施）

- **检查对象**：deny 目标（`锚定根/.env`）的 `symlink_metadata()`（lstat，不打开
  正文）→ `st_nlink`。
- **检查时机**：`sandbox.rs::append_unreadable_root_args` 的 `exists()` 分支内
  （即每次工具启动组装 argv 时，非仅进程启动时）——每次调用都复查，窗口最小化。
- **行为**：`st_nlink > 1` → 返回 `Err`（fail closed，错误指明路径与 nlink 值，
  与既有「词法非清洁拒绝组装」同型），调用侧三面显式报错、不执行。
- **并发变化边界（如实声明，不得描述为完整竞态防护）**：
  ① 检查通过后、bwrap 挂载完成前，沙箱外进程新建链接仍可绕过（TOCTOU 窗口）；
  ② 沙箱运行期间沙箱外进程新建链接同样绕过（挂载已定型）；③ 别名不可枚举
  （link(2) 无反向查找），无法逐路径遮蔽。检查的实际价值＝阻断**启动前已存在**
  的别名（备份工具/`cp -l`/用户手工遗留等现实形态）并收窄窗口，属**纵深收窄**，
  不是竞态防护。完整的 inode 级保护需 deny 面｢路径集外一律不可读｣的可表达性
  （Landlock 之外的 MAC/文件系统级机制），超出本项目范围。
- **兼容性影响**：真实 `.env` 本轮元数据实测 **nlink=1**（`stat -c %h`＝1，
  size/mtime 不变，未建链接、未开正文、未修改）——当前部署零触发；一般仓库的
  `.env` 被硬链接属罕见形态，误拒成本可接受且错误信息可指导排查（何路径、
  nlink 值多少）。
- **修复位置**：`src/app/src/sandbox.rs::append_unreadable_root_args`（单一函数，
  三面共用入口；约 +6 行＋错误文案）。
- **必要回归**：① 单元（`sandbox::tests`）：假 `.env`＋预建硬链接 → 组装 Err；
  nlink=1 既有用例不变（变异证明：去掉检查后新用例须红）；② 三型集成各补一例
  （预建硬链接场景工具显式拒绝、目标命令标记不出现）；③ `e2e_sandbox::matrix_a`
  的 `ENV_HARDLINK` 断言由「可读出（边界记录）」翻转为「exec 显式拒绝」。

## 3. B2 核实（非 CLOEXEC fd 继承）

### 3.1 证据链评估与补齐

| 链路环节 | 上轮（矩阵轮） | 本轮补齐 |
|---|---|---|
| launcher 预开非 CLOEXEC fd | —（缺失） | ✅ 测试进程（launcher 角色）开本轮假文件 fd（动态编号，`fcntl F_SETFD=0` 清 CLOEXEC，跨 spawn 保持） |
| 真实 terminal-chat 二进制 | 缺失（fd 持有者是测试二进制） | ✅ 真实 CLI（CARGO_BIN_EXE＝部署产物）管道模式，完整轮次（`mock.count()==2`、最终回复落 stdout、退出码 0） |
| 生产工具入口 | ✅ ProcessManager（turn.rs 同款） | ✅ mock 返回 exec_command → 生产分派 |
| bwrap | ✅ | ✅ 沙箱跑 `PID=1`（`--as-pid-1` 链路证据） |
| 正文读取 | ✅ `/proc/self/fd` 可读 | ✅ `FD=FD-CHAIN-FAKE-033`（假凭据） |
| 普通模式对照 | ✅ | ✅ `PID≠1`、同样可读（fd 面非沙箱差异面） |

- 上轮已有（引用）：`sandbox_matrix_probes.rs::d_fd_inheritance_boundary_recorded`
  （库级：测试进程→ProcessManager→bwrap→可读，沙箱/普通一致）＋手工 bwrap 探针
  （bash `exec 9<file` → 沙箱内 `cat /proc/self/fd/9` 读出）。
- 本轮新增：`e2e_sandbox.rs::b2_real_cli_fd_chain_full_path`（真实 CLI 全链路，两
  模式）＋ `sandbox_matrix_probes.rs::b2_fix_feasibility_preexec_close_fds_keeps_stdio`
  （修复方案可行性）。原始输出：`2026-10-27-b2-probes.log`。
- 实施注意（本轮实测）：测试内固定 fd 编号（如 9）会与并发用例的 PTY/套接字 fd
  冲突（dup2 覆盖他人或他人 Drop 关闭我方）——须动态编号并格式进探针命令。

**仅测试本轮假文件的额外 FD；未读取真实会话中的任何敏感 FD。**

### 3.2 上游对照（冻结版本 d8acdf1）

- `launcher.rs::exec_bwrap` 为**原地 `execv`**（非 spawn 子进程）；执行前唯一 fd
  处理＝对**自需**的 bind-data/proxy fd 调 `make_files_inheritable`（清 CLOEXEC，
  供 `--ro-bind-fd` 使用）。
- 全 crate（bwrap.rs/linux_run_main.rs/launcher.rs）**无** close_range/closefrom/
  逐个 close/仅保留 0-2 的继承面清理；`linux_run_main.rs` 的 close 仅针对 preflight
  stderr 管道自产 fd。
- 结论：非 CLOEXEC 继承 fd 同样透传到上游 bwrap 载荷——B2 与上游共有；上游的
  fd 卫生同样依赖「自开 fd 一律 CLOEXEC」（与本项目 027 R1 同型）。

### 3.3 最小修复方案（沙箱分支子进程 fd 清理；本轮未实施）

- **原则**：不关闭主进程 fd（PTY master、MCP 管道、错误上报通道、会话/记忆等
  全在主进程，直接关闭会破坏产品自身）；只在**工具子进程 spawn 边界**清理。
- **方案**：新增共享 helper（如 `sandbox::close_inherited_fds_preexec()`），在
  **沙箱分支**的三个工具 spawn 点经 `Command::pre_exec` 注入：
  ① `proc_session.rs::build_tool_command`（exec 面）；② `mcp/transport.rs::spawn`
  （MCP 面）；③ `apply_patch.rs` internal-apply 子进程（补丁面）。实现优先
  `close_range(3, ~0, 0)`（内核 6.18 支持；**须核实 033-seccomp 基线放行该
  syscall，未放行则回退**），回退＝循环 `close(3..=RLIMIT_NOFILE)`（close(2)
  异步信号安全，pre_exec 约束满足；1024 档循环约千次 syscall，工具启动频度下
  可忽略）。
- **保留清单（代码核查实证，三 spawn 点子进程合法 fd 仅 0/1/2）**：
  - exec 管道模式：stdin null／stdout、stderr piped（0/1/2）；
  - exec PTY 模式：三路＝PTY 从设备，经 std dup2 到 0/1/2（`proc_session.rs:1107`
    注释「std 先 dup2 再跑 pre_exec」——本轮可行性探针独立实证：pre_exec 关
    3..=1024 后 stdout 管道仍可用、fd 消失、0/1/2 保留）；
  - MCP stdio：stdin/stdout/stderr 管道（0/1/2，stderr 排空通道＝fd 2）；
  - apply_patch：补丁 stdin（0）／结果 JSON stdout（1）；
  - 载荷运行时自开的 fd（如 apply_patch Update 的 `/proc/self/fd` 同 inode 写回）
    在 exec **之后**打开，不受 pre_exec 清理影响。
- **范围**：仅沙箱分支（普通模式既有行为零变化）；三个 spawn 点共用一处 helper。
- **必要回归**：① 三型沙箱集成（PTY/管道/MCP/apply_patch 全量既有用例——验证
  0/1/2 保留与回收不回归）；② 本轮 `b2_real_cli_fd_chain_full_path` 断言翻转
  （沙箱跑 fd 不可读、普通对照仍可读）；③ `d_fd_inheritance_boundary_recorded`
  沙箱分支同步翻转；④ E 回收用例（PTY/管道）确认 stdio 完好。
- **部署面补充**：文档（README/部署记录）注明「启动 terminal-chat 的会话不得遗留
  可继承 fd」——该责任在 launcher，产品侧清理为纵深而非替代。

## 4. 规格缺口评估

| 项 | 是否当前规格缺口 | 依据与建议 |
|---|---|---|
| B1 | **承诺差距（建议补规格条目）** | 033 规格写明「`<工作区>/.env` 不可读**遮蔽**」＝路径级，按字面不违约；但 027→033 的安全意图是凭据内容保护。建议规格 §1 补一句承诺边界（「路径级遮蔽；同 inode 别名（预建硬链接）不在保证内」）＋可选 fail-closed 条目（§2.3 方案），或明确记录风险接受及理由 |
| B2 | **非 033 缺口（部署信任边界）** | fd 面不在 033 替换面——027 R1 正交防线（env_clear/CLOEXEC）针对**产品自开** fd；launcher 遗留 fd 属部署侧。建议部署文档注明；沙箱分支 pre_exec 清理为可选纵深（§3.3，超出上游） |
| B3 | 观察项（保持） | 022 容错语义，无回退证据；如需提升错误可见性属 022 范畴的独立决策 |

## 5. 修订后的矩阵总评

**A–G 必测项全部实际执行并通过（证据与结论同矩阵记录，本轮未重跑、未改动）。**
总评口径修正为：功能面（指定路径遮蔽、env 白名单、PID ns 隔离、回收、fail-closed、
网络边界如实）兑现路径级承诺；**存在两条条件性读取绕过通道**（B1 预建硬链接、
B2 launcher 遗留非 CLOEXEC fd），均与冻结上游 codex `d8acdf1` 共有、非本项目适配
引入——**不得据此宣称「.env 内容经任何别名都不可得」**；现有容错行为（取消会话
持有、MCP 空快照、EBUSY 拒删）为契约内行为。修复建议见 §2.3/§3.3（均未实施，
待决策）。

**结论适用范围（收窄声明）**：以上「与上游共有」「路径级挂载不保护 inode」
「bwrap 不关闭继承 fd」的判断，仅基于**本项目实测**（bubblewrap 0.9.0、内核
6.18.33、v9fs/overlayfs）与**已核对的具体上游版本**（openai/codex
`codex-rs/linux-sandbox` @ `d8acdf151577132d4ee1de47a4d1822ae5acb762` 的
bwrap.rs/linux_run_main.rs/launcher.rs 三文件）。未核对 Claude Code
sandbox-runtime 及其他 bwrap 封装实现的对应行为，不作外推。

## 6. 本轮执行记录

| 命令（`/workspace`） | 结果 |
|---|---|
| 上游核对：WebFetch × 3（d8acdf1 三文件） | 完成（§2.2/§3.2 引用） |
| `stat -c 'nlink=%h …' /workspace/.env` | nlink=1（仅元数据；size/mtime 不变） |
| `cargo test --test e2e_sandbox b2_real_cli -- --nocapture` | 1 passed RC=0（动态 fd 修正后；固定 fd 9 版本在合跑下暴露并发冲突并修正——测试自身问题，非产品） |
| `cargo test --test sandbox_matrix_probes b2_fix -- --nocapture` | 1 passed RC=0 |
| `cargo test --test e2e_sandbox --test sandbox_matrix_probes`（注入态 / `env -u`） | 7+7 passed / 7+7 passed，RC=0×2 |

新增测试 2 个（`b2_real_cli_fd_chain_full_path`、`b2_fix_feasibility_preexec_close_fds_keeps_stdio`）；
未改产品代码与既有测试；未重跑 A–G 全矩阵、全量与性能；未动部署开关/seccomp/
账号权限；未提交/合并/推送。日志：`2026-10-27-b2-probes.log`（同目录）。
