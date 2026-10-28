# 033 沙箱多场景实测记录（2026-10-27，已启用沙箱的 dev 容器内）

**结论：矩阵 A–G 必测项全部实际执行并通过（PASS），无 BLOCKED；发现 0 个产品缺陷、
3 项已知边界（硬链接别名／非 CLOEXEC fd 继承／MCP 容错可见性，见 §5）。**
**承诺口径（2026-10-27 核实轮修正，详见
[2026-10-27-b1-b2-risk-verification.md](2026-10-27-b1-b2-risk-verification.md)）**：
本轮通过的是**路径级承诺**——「指定路径（锚定根 `.env`）不可读」；B1（沙箱外预建
硬链接）与 B2（launcher 遗留非 CLOEXEC fd）是两条**条件性读取绕过**，证明更强的
「内容经任何别名都不可得」承诺**不成立**——硬链接读取成功不得记作正文保护通过。
两条边界经核对与冻结上游 codex `d8acdf1` 共有，非适配引入。
全程在当前容器内完成：无宿主操作、未重建容器、未改 seccomp／持久开关／产品逻辑；
Windows 原生沙箱不在本轮范围（未实现，`#[cfg(not(unix))]` 恒 false）。

## 0. 被测对象（实测确认）

| 项 | 值 |
|---|---|
| 源码提交 | main `28b7999`（033 交付收口）；`git status -- src/ Cargo.toml Cargo.lock` 仅未跟踪运行时副产物 `src/app/config/`——**产品代码与 HEAD 一致，本轮零产品改动** |
| 被测二进制 | `/workspace/target/debug/terminal-chat`（364586272B，mtime 1793056909）；验证前后 mtime 不变（测试新增不触发 bin 重建） |
| bwrap | `/usr/bin/bwrap`，bubblewrap 0.9.0 |
| 开关 | `TCHAT_SANDBOX=1` 来自 compose environment（本会话及新测试会话均实证，无手工 export）；普通模式对照仅在隔离子进程 `env -u` |
| 实际策略（读码 `src/app/src/sandbox.rs`） | 全盘只读 ＋ 可写根＝[锚定根（主进程 cwd canonical）/tmp] ＋ 锚定根 `.env` 遮蔽（`--ro-bind /dev/null`，userns nodev 强制下 open=EACCES）；子进程 env 白名单 PATH/HOME=锚定根/TERM；PID/IPC/USER ns＋`--as-pid-1`＋新 procfs＋cap-drop ALL；**网络默认保留（无 --unshare-net）**；`/run/WSL` 条件遮蔽——本容器无该目录，跳过（符合预期） |
| 本轮新增 | 测试 2 个（`src/app/tests/e2e_sandbox.rs` 全链路 E2E、`src/app/tests/sandbox_matrix_probes.rs` 库入口探针，均子进程隔离环境、不污染全量回归）＋本记录；未改任何产品/既有测试代码 |

## 1. 场景矩阵结果

| 场景 | 结果 | 证据要点（原始输出见 §6 日志） |
|---|---|---|
| **A 默认启用与完整调用链** | **PASS** | ① 新会话 `TCHAT_SANDBOX=1`（compose 注入）＋干跑横幅「工具沙箱模式：exec/MCP/apply_patch 经 bubblewrap 隔离（/usr/bin/bwrap）」；② 真实 CLI＋PTY＋本地 mock（127.0.0.1 随机端口）一轮连续调用三工具（exec_command/apply_patch/update_plan）；③ `mock.count()==2` 恰好两轮（工具轮＋收尾轮），全部请求 Authorization 含本轮假密钥 `fake-033-mx-key`（生效端点＝本轮 mock 配置）；④ 第二请求体含三工具结果（exec 探针 21 项证据行、apply_patch ok+changed_files、update_plan ok/error=null）；⑤ 文件副作用外部核对（补丁文件内容、中文路径文件、删除生效）；⑥ 沙箱成功分支证据判定器（PROBE_PID=1＋PROBE_NS≠主进程＋哨兵不可见＋HOME=锚定根，四项全过） |
| **B 正常功能** | **PASS** | exec：workdir 显式、创建/追加（`中文 文件.txt` 内容逐字节核对）、删除（DEL_GONE=yes）、空格＋中文路径；apply_patch：Add（E2E 落盘）＋Update/Delete（三型集成 3/3，真实分支）；MCP stdio：真实启动 python 测试服务器、echo 往返（`echo:iso`）、显式 env 保留（`TOKEN=[canary-033mx]`）；正常退出后无遗留：E2E 侧 scan_leftovers 空、MCP close 后服务器进程扫描空、E 探针各场景父侧扫描空 |
| **C 文件与路径边界** | **PASS**（1 项已知边界，见 §5-B1） | 假凭据前置证明（沙箱外存在/可读/含唯一标记）；相对路径/绝对路径/改 cwd/`sub/..`/符号链接别名读取全部＝0（内容级 grep 计数），相对路径原始错误＝`cannot open .env: Permission denied`；三工具面：exec（E2E）／MCP 服务器（`ENV_READ=…Permission denied`）／apply_patch（Update=读取失败 EACCES、Delete=EBUSY 挂载点，外部字节不变）；只读区写入＝DENIED（`/var/tmp` 沙箱外预先证明可写）；硬链接别名＝**可读出正文（预建链接）**，沙箱内自建链接＝EXDEV 被拒（手工探针）；9p 工作区实测支持硬链接（攻击面实际存在）；拒绝操作后外部假凭据全部未变 |
| **D 环境、FD 与进程可见性** | **PASS**（1 项已知边界，见 §5-B2） | 父进程哨兵 env 三变量（dotenv 假凭据/派生哨兵/哨兵进程变量）沙箱内全部为空 `[][][]`（普通对照可见）；MCP 显式 env 保留＋污染 env 排除（env_clear）；HOME=锚定根、PATH 有效、TERM 白名单传递（`xterm-033mx`）；沙箱外哨兵进程：`/proc/<pid>` 不可见、environ/cwd/root 全 DENIED、`kill -0` 失败（PID ns 隔离，未以可见进程数为唯一断言）；FD：无 CLOEXEC 假凭据 fd 经 `/proc/self/fd` 可读（沙箱/普通一致——bwrap 不关闭继承 fd，产品自身 fd 均 CLOEXEC） |
| **E PTY、取消、超时与回收** | **PASS** | 管道/PTY × 取消/超时 各 3 次重复＋正常完成×2＋短命令×2（沙箱模式）；后台子进程＋孙进程；双侧扫描（子进程 close 前后）＋父进程退出后扫描全空；**取消＝016 既有契约**（`session_id=s1`、error「进程仍在运行，由会话 s1 管理」，close() 终止整组回收）；**超时＝契约到期组杀**（close 前即无遗留）；沙箱内正常完成的后台子/孙进程随 PID ns 回收（pid 1 退出→内核 SIGKILL，033 结构性改进）；既有 exec_sandbox PTY 超时组杀复用（三型集成 2/2） |
| **F fail-closed** | **PASS** | 隔离子进程（仅替换 PATH，未触碰 `/usr/bin/bwrap`）四场景 × 三工具面：bwrap 缺失／不可执行（0644）→ exec 显式错误含 bwrap、零输出；探测干跑失败（exit 1）→「干跑失败…可能禁止 unshare 用户命名空间」；探测成功但启动失败（仅放行探测参数）→ bwrap exit 1 非零退出；**全部场景执行标记文件未出现**（无静默回退）；apply_patch 面被拒＋文件不产生；MCP 面服务器不启动（marker 不存在、tools=0） |
| **G 普通模式与网络边界对照** | **PASS** | 同一探针 `env -u TCHAT_SANDBOX` 子进程对照：假凭据可读（ENV_*=1、48 字节无错误）、哨兵 env 可见、哨兵进程可见可控、只读区可写——与沙箱分支逐项相反，**差异证明来自沙箱**；网络：本地监听器两模式均 `NET_LOCAL=200`（恰一次访问）——**当前策略保留网络，未宣称网络隔离**；共享容器持久开关未动（无 compose/配置变更） |

矩阵外补充：`sandbox_evidence` 判定器正反例 5 项（有效通过/普通模式拒绝/空输出拒绝/异常字段拒绝/HOME 正确但 ns 相同拒绝）——开发记录非产品 RED。

## 2. 实际执行的命令与退出码

| # | 命令（工作目录 `/workspace`） | 结果 |
|---|---|---|
| 1 | `bash .devcontainer/verify-033-deploy.sh` | **8 PASS / 0 FAIL，RC=0**（gitleaks、bwrap/开关、横幅、三型集成、遮蔽实验、e2e 普通回归、.env 元数据前后一致） |
| 2 | `cargo test --test e2e_sandbox evidence` | RED：5 failed（todo!()）RC=101 → 实现后 **5 passed RC=0** |
| 3 | `cargo test --test e2e_sandbox matrix_a`（部署注入态） | **1 passed RC=0**（第 3 次达成：前两次为测试自身缺陷——授权指纹不匹配、changed_files 断言形状，均为测试修正非产品问题） |
| 4 | `env -u TCHAT_SANDBOX cargo test --test e2e_sandbox matrix_a -- --nocapture` | **1 passed RC=0**（普通对照分支） |
| 5 | `cargo test --test sandbox_matrix_probes -- --nocapture` | **6 passed RC=0**（迭代中 2 次失败均为断言口径错误：TOKEN 方括号字面值、取消回收时序误解 016 契约——修正后全绿；最终版含 MCP 服务器回收扫描） |
| 6 | `env -u TCHAT_SANDBOX cargo test --test e2e_sandbox --test sandbox_matrix_probes -- --nocapture` | **12 passed RC=0**（套件集成验证：新增文件在 env -u 全量回归口径下无污染） |
| 7 | 手工补充探针（产品同参 bwrap 子集，假凭据） | 继承 fd 透传（exit 0）、沙箱内自建硬链接 EXDEV（exit 0）、9p/overlayfs 硬链接能力（均按预期） |

测试数量合计：新增用例 12（E2E 1＋判定器 5＋探针 6），全为真实分支（无跳过/空跑）；既有三型集成 6＋e2e_acceptance 143 复跑全绿。**产品代码未变，按目标执行要求未重复全量与性能测试**（3506/0/3 基线复用部署记录 §4）。

## 3. 默认启动与生产链路生效证据

- 默认启动：verify 脚本第 2/3 步（新会话 `TCHAT_SANDBOX=1` 无手工 export＋横幅报告 bubblewrap）；E2E 沙箱分支启动横幅 `工具沙箱模式` 屏幕等待命中（继承部署注入态，测试未补设开关）。
- 生产链路：E2E 全链路为**真实 CLI 二进制**（CARGO_BIN_EXE＝部署产物）经 mock 返回 tool_calls → 生产分派（turn.rs）→ exec（ProcessManager→bwrap）/apply_patch（`--internal-apply` 子进程→bwrap）/update_plan → 结果回传 mock。探针文件走与 turn.rs 相同的库入口。工具真实进入沙箱的判定不依赖标记位：四项独立证据（§1-A⑥）。
- 手工 bwrap 探针仅作边界补充（fd/硬链接/自建链接），未替代产品链路实测。

## 4. 正常功能、越权拒绝、进程回收实际结果

- 正常功能（沙箱内）：exec 21 项探针全对（含中文/空格路径、追加、删除）；apply_patch 三操作落盘正确；MCP echo 往返成功。
- 越权拒绝（区分两层）：全部拒绝均发生在**操作系统沙箱层**（EACCES/EBUSY/EXDEV/不可见），无一例由工具权限门产生（本轮探针未触发权限门路径）；拒绝后外部假凭据逐字节未变；错误信息不回显 .env 正文。
- 进程回收：取消（会话持有→close 整组回收）／超时（到期组杀）／正常完成（PID ns 随 pid 1 退出全灭）／MCP close——父侧 `/proc` 扫描全部为空；仅对本轮创建并记录身份的进程发信号。

## 5. 缺陷、已知边界与最小修复建议

**产品缺陷：0。**（过程失败共 5 次，全部为测试自身问题：授权指纹、断言形状×2、断言字面值、契约时序误解——均已修正并复验，非产品缺陷，按开发记录处理。）

已知边界（按实际策略如实记录）：

- **B1 硬链接别名（预建）**：路径级遮蔽（`--ro-bind /dev/null <path>`）不覆盖同 inode 的其他路径——沙箱外**预先创建**的硬链接可读出 .env 正文（`ENV_HARDLINK=1` 实证）；沙箱内进程自建链接被 EXDEV 拒绝（无法在沙箱内制造别名）；9p 工作区支持硬链接，攻击面实际存在。**最小修复建议**：组装 deny 前对目标 `st_nlink > 1` 即 fail closed（真实 .env 元数据实测 nlink=1，当前部署零触发）。核实轮已确认与冻结上游 codex `d8acdf1` 共有（上游无 nlink 处理）——完整方案、竞态边界与必要回归见 [B1/B2 核实记录 §2](2026-10-27-b1-b2-risk-verification.md)。
- **B2 非 CLOEXEC fd 继承**：bwrap 不关闭继承的 fd ≥3——launcher 遗留的无 CLOEXEC fd 可被沙箱内工具经 `/proc/self/fd` 读出（库级＋真实 CLI 全链路＋手工探针三重实证，沙箱/普通无差异；上游 d8acdf1 同样不清理）。产品自身打开的 fd 均 CLOEXEC，风险面在部署侧。**最小修复建议（核实轮更新）**：沙箱分支三个工具 spawn 点 `pre_exec` 关闭 fd ≥3（可行性已探针实证：stdio 先于 pre_exec 就位、0/1/2 保留），保留清单与回归见 [B1/B2 核实记录 §3](2026-10-27-b1-b2-risk-verification.md)；部署文档同步注明 launcher 责任。
- **B3 MCP 面沙箱不可用的可见性**：fail-closed 时 McpManager 按 022 容错语义返回空快照（`started=true tools=0`），无显式错误文案——无静默回退（服务器 marker 不存在）但错误可见性弱于 exec/apply_patch 面。与 022 既有契约一致，记录为观察项，非缺陷。

另记录两项契约行为（非缺陷）：取消后进程由可续接会话持有（016）；apply_patch Delete 触碰遮蔽路径报 EBUSY（挂载点）而非 EACCES——拒绝语义等价。

## 6. 日志路径与清理结果

证据（本目录 `docs/logs/033-tool-sandbox/`，脱敏：全程假凭据/本地 mock/仅 .env 元数据）：

- `2026-10-27-matrix-verify-deploy.log`——部署验证重跑（8 PASS）
- `2026-10-27-matrix-sandbox-integration.log`——三型沙箱集成（6 用例全绿）
- `2026-10-27-matrix-e2e-normal.log`——普通模式回归 e2e_acceptance（143 passed）
- `2026-10-27-matrixA-sandbox.log`／`2026-10-27-matrixA-normal.log`——全链路 E2E 双分支探针原始输出（21 项证据行逐行可查）
- `2026-10-27-matrix-probes.log`——C/D/E/F 探针（最终版，含各场景结果 JSON）
- `2026-10-27-matrix-suite-env-u.log`——env -u 套件集成验证（12 passed）

清理：`/tmp` 本轮文件（探针夹具、日志、基线快照）已全部清除；无遗留进程/监听器（PID 1 基线 sleep 未触碰）；真实 `.env` 元数据前后一致（64B 1792585545，未读正文）；被测二进制 mtime 未变；未提交/合并/推送，未改部署配置。
