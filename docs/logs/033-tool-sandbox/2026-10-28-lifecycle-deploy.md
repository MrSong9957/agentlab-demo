# 033 生命周期修复候选本地提交、合并、部署与部署后验证（2026-10-28）

授权：本轮 /goal 明确批准提交、合并、更新正式二进制，无需重复请求。全部操作在
Docker 容器内；真实 .env 原位未动未读正文；未改账号/sudoers/ACL/seccomp/部署开关；
未推送远程；未整体 reset/checkout/stash；未停止任何运行实例。

## 1. 提交（任务分支 fix/033-proc-lifecycle，工作树 .worktrees/033-lifecycle）

- **任务提交 `a3aef1c`**：15 文件，+29387/−16（proc_session.rs 长寿创建线程
  ＋69、mcp/connection.rs worker 移交＋31、mcp/transport.rs kill_on_drop＋6、
  e2e_acceptance.rs s103 外层回收判定＋61、sandbox_lifecycle.rs 回归四用例＋419、
  规格追加一节＋归因记录及 9 份验证证据日志）。
- 归因文档引用闭包核查后，round 8 的三份归因证据（e2e-attr-a/b.log、
  e2e-attr-drive.py，源自主工作树未跟踪文件，指纹逐一比对一致）随分支入库，
  避免悬空引用。
- 暂存：逐文件显式 `git add`（无 `git add .`）；工作树内无并行任务/个人配置混入。
- 门禁：gitleaks protect --staged 手动扫描 **exit 0**（no leaks found）；pre-commit
  钩子提交时真实执行 **exit 0**（fail-closed 门禁未绕过）。

## 2. 合并（main）

- main 基线 `ca769d3`（期间无并行新提交；未提交工作 42 项与本分支零交集，原样保留）。
- 碰撞处理：主工作树存在 7 份与分支同名的未跟踪文件（4 份与分支内容 sha256 一致；
  3 份 attr 源文件指纹一致）——合并前移至 /tmp 备份并记录指纹，合并后比对（6 份
  sha256 一致；lifecycle-final-full-pty.log 经 git CRLF→LF 规范化，`tr -d '\r'` 后
  逐字节一致），确认无内容差异后清理备份。
- **合并提交 `dc9ef7b`**（no-ff）：15 文件 +29387/−16 与候选统计一致，零冲突。
- **候选↔合并后一致性**：`git diff a3aef1c dc9ef7b` 为空（内容完全一致）——
  候选验证证据全部有效，无需补跑全量。
- 合并后 `cargo check --locked --offline` **exit 0**（2 warning 为存量 thread_local
  文档注释 lint，行号平移，非本轮引入）。

## 3. 构建与正式二进制替换

- 构建源码纯净性：main HEAD `dc9ef7b`；`git status -- src/ Cargo.toml Cargo.lock`
  除 `?? src/app/config/manual.json`（2026-10-24 运行时副产物遗留，不影响编译，
  原样保留）外无未提交改动。
- `cargo build --locked --offline --target-dir /tmp/tchat-lc-deploy-target`
  **exit 0**；产物 sha256 **`713e4c28a431f40134e1f0bd9d9170e14266046176d4501a739f7e34eff41cd6`**。
- 备份：替换前官方二进制（`4ab2dcc1…`）→
  `.devcontainer/033-rollback/terminal-chat.pre-lifecycle-20261028`（指纹核验一致；
  既有回滚点 `terminal-chat.pre-b1b2-20261028` 未覆盖）。
- 原子替换：同目录临时文件 `cp` → `chmod --reference` 保留执行权限 → `mv -f`；
  替换后指纹核验＝`713e4c28…`。

## 4. 部署后真实验证（直接驱动正式路径二进制＋本地 mock 全链路，假凭据）

三份驱动器全部显式使用正式路径 `/workspace/target/debug/terminal-chat` 并断言
sha256＝`713e4c28…`；部署态口径 `TCHAT_SANDBOX=1` 注入。日志见本目录
`2026-10-28-lifecycle-deploy-verify-{b1b2,lc,cancel}.log`。

1. **B1/B2/三工具面（`deploy-verify-driver.py`，指纹断言同步本轮产物）**：
   **22 PASS / 0 FAIL，exit 0**——默认注入＋启动横幅（「工具沙箱模式…bubblewrap
   隔离」）＋真实沙箱成功分支（载荷 PID=1、新 PID ns）；B1 硬链接拒绝经**新创建
   线程链路**显式回传（不回退普通模式）；B2 launcher 遗留 fd 不进载荷；命令不存在
   shell 127 如实回传；MCP 服务器经生产 transport 沙箱启动（mypid=1、.env 遮蔽）；
   apply_patch 沙箱内落盘；外部假凭据字节不变。
2. **生命周期四场景（`2026-10-28-lifecycle-deploy-driver.py`，本轮新增，复刻
   sandbox_lifecycle 回归）**：**32 PASS / 0 FAIL，exit 0**——
   C 管道跨轮：yield 长跑轮结束后写盘继续、write_stdin 查询 running＋tick、
   退出后活进程与 bwrap 双清零；
   D PTY 跨轮输入：write_stdin chars 送达保留会话（GOT:hello 落盘）；
   E MCP 跨轮＋回收：外层 argv 身份证跨轮存活、下一轮连接复用、停用 reload 后
   5s 内外层回收（唯一 record 标记；僵尸天然排除）；
   F SIGKILL：先证跨轮存活 → SIGKILL 自建 CLI → 三分类（alive=[]、zombies 2 个
   已死未收割不计泄漏）——die-with-parent 经长寿创建线程仍成立。
3. **取消契约对照矩阵（`e2e-attr-drive.py` 场景 A）**：新二进制沙箱态/普通态
   17/17 项全过（取消后写盘 12→26 继续、写入进程与 bwrap 存活、批内跳过、下一轮、
   退出清理）；**旧二进制（pre-b1b2 回滚点）同条件复现缺陷**（取消后写盘 8→8 停止、
   进程消失）——部署态差异即本轮修复，对照有效。

**结论：部署后验证全部通过，无回滚。**

## 5. 复用与补跑的覆盖范围

- 复用候选冻结态证据（代码内容 `git diff a3aef1c dc9ef7b` 为空）：标准口径全量
  71 目标 3533/0/3 exit 0（`2026-10-28-lifecycle-final-full-pty.log`）；双模式
  e2e_acceptance 143/143（`-lifecycle-e2e-sandbox-full.log`／`-lifecycle-e2e-env-u.log`）；
  沙箱专项、审查与变异敏感性记录见
  [生命周期修复主记录](2026-10-28-lifecycle-fix.md)。
- 补跑（部署验证必需）：合并后 `cargo check`；上述三份部署驱动器。
- 未重复：全量 test、性能矩阵（代码与候选一致，无新验证覆盖需求）。

## 6. 状态与剩余边界

- **main HEAD：dc9ef7b**（合并后文档收口提交见 git log）。正式二进制＝`713e4c28…`
  ← 构建 `dc9ef7b` ＝ 候选 `a3aef1c` 内容。
- 实例状态：部署时无活用户实例（仅 4 个先前遗留 defunct 僵壳，未动）；本轮全部
  验证实例由新二进制启动并已退出清理。既有实例若仍持有旧版（本次不存在），继续
  运行至自然重启。
- 回滚：`cp -p .devcontainer/033-rollback/terminal-chat.pre-lifecycle-20261028
  target/debug/terminal-chat`（恢复 `4ab2dcc1…`＝ca769d3 构建）；更早回滚点
  pre-b1b2 原样保留。
- 剩余边界：载荷运行期自开进程/线程仍不受生命周期管理（沙箱 ns 内语义不变）；
  apply_patch 面未接入创建线程（同步语义无需，代码注释已注明）；任务分支
  fix/033-proc-lifecycle 与工作树 .worktrees/033-lifecycle 暂时保留。
- 遗留提醒（与任务无关，仅报告）：`src/app/config/manual.json`（2026-10-24
  运行时副产物，未跟踪，未动）。
