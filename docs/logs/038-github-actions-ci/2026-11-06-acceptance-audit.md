# 038 剩余验收核对（2026-11-06）

## 范围与候选

- 仓库：`/workspace`；分支：`feat/038-github-actions-ci`。
- 用户给定基线：`57d5408`；本地 HEAD：`0299f53`，共同祖先为 `57d5408`。本地远端跟踪状态显示分支 ahead 9；保留现有工作区差异；B 不合并。
- `.github/workflows/ci.yml` 自 `57d5408` 起没有差异，工作区也没有该文件改动。本次复用既有记录，并运行了 G2 精确用例的 Linux 定向验证；没有运行全量测试、推送或触发 CI。
- 本地候选有后续代码和文档差异。因此 11 月 4 日完整 L3 与更早三平台 Actions 结果不能作为当前候选的全量通过证据。

## 逐条剩余验收

| 规格来源 | 状态 | 现有证据 | 当前缺口 |
|---|---|---|---|
| §1 R1、R2、R3、R6；`.github/workflows/ci.yml` | 工作流配置符合；当前候选结果未执行 | 工作流运行 `cargo check --locked` 和 `cargo test --workspace --no-fail-fast --locked`；触发为 main push、pull_request、workflow_dispatch；同引用取消旧运行；三平台矩阵 `fail-fast: false`；使用指定 toolchain/cache actions。已有运行历史证明旧候选能触发这些 job。 | 当前候选没有新的完整三平台结果。 |
| §1 R3、§5 三平台验收；工作流 `continue-on-error` | **未满足**；当前候选平台结果也未执行 | 当前工作流仍对 Windows/macOS 设置 `continue-on-error`。最近规格记录的 Windows 21 项失败和 macOS 42 项失败/卡住来自旧候选，不能沿用为当前失败数，但表明最近一次已记录的平台验收未通过。 | 需取得当前候选 Windows、macOS 全量结果，修完确认的失败，并在两端通过后移除 `continue-on-error`。当前工作区改动涉及进程会话和沙箱路径，旧平台结果不能证明这些改动可编译或通过。 |
| §1 R4、R5；§7 覆盖审计 | 旧记录相互矛盾；当前源码/工作流检查未见五个提前返回开关 | 后续状态记录报告五个 `TCHAT_SKIP_*` 提前 return 已移除。当前工作流和源码中未检出 `TCHAT_SKIP_*` / `SKIP_RUNNER`，工作流也未设置这些变量。三个性能采样开关仍按 R4 运行场景并跳过冻结阈值断言，规格已说明此口径。 | §1 R4、R5 与 §7 中“CI 置位并计为 passed”的文字是旧状态，不能描述当前工作流。它不构成新的实现项；这次核对在附加记录中注明了现状，未重写历史说明。 |
| §1 R3/R4；迭代 35 剩余 Windows 项 | **夹具修复已本地验证；Windows 验收尚未执行** | `2026-11-02-iteration32-status.md` 记录 Windows G2：带 verbatim 前缀的路径在 `PathBuf::join` 中会折叠 `sub\\..`。当前测试已对 canonical 路径以字符串追加原始分量，并断言 Windows verbatim 前缀及 `parse_request` 保留原始输入。Linux 精确用例通过；见 [`2026-11-06-windows-g2-path.md`](2026-11-06-windows-g2-path.md)。 | 需要 Windows 精确用例结果确认该平台上的前置路径和规范化断言成立。Linux 结果不能替代 Windows 验收。 |
| §1 R1、R3；项目完整验证入口；11 月 4 日 L3 输出 | **当前候选全量未执行**；上次全量有失败 | `target/038-validation/l3-20261104.log` 完成了全量运行，记录 5 个失败目标：`js_orchestration`、`multi_agent`（`c09_fourth_child_rejected_and_group_assignment_cap`）、`proc_session`、`sandbox_matrix_probes`（MCP 环境断言）和 `session_perf`。之后已有 worker 回收、MCP 截断、pipe Ctrl+C 的定向记录：`2026-11-05-process-reaping-closeout.md`、`2026-11-05-mcp-env-read-truncation.md`、`2026-11-06-pipe-ctrl-c.md`；pipe 普通和真实沙箱精确用例均通过。 | 定向结果关闭各自覆盖的路径，但没有一份当前候选全量结果。`multi_agent::c09_fourth_child_rejected_and_group_assignment_cap` 和 `session_perf::perf_tiers_baseline_driven` 没有后续闭合证据；后者在既有记录中属于机器/磁盘敏感项。不能把 11 月 4 日失败数当成当前结果，也不能宣称全量通过。 |
| §1 R4/R5；runner D1–D3 诊断记录 | **当前 runner 结果未执行** | 工作流未设置 `TCHAT_SANDBOX`。它包含仅在 workflow_dispatch 运行的 `runner-diagnosis`，该 job 明确非门禁。旧 CI 记录曾有 runner 沙箱异常；本地候选的近期定向测试不等于 hosted runner 复测。 | 当前候选的完整 Ubuntu job 尚无结果。诊断 job 只能提供证据，不替代全量验收，也不能单独证明 D1–D3 已闭合。 |
| 038 §0-B 与审查 P2 | **规格要求已由本地证据满足；CI 自动覆盖建议，不是交付阻断项** | §0-B 要求同一精确用例在普通和真实 `TCHAT_SANDBOX=1` 两种模式验收。`2026-11-06-pipe-ctrl-c.md` 记录两条模式命令都通过。当前 workflow 不全局设置 `TCHAT_SANDBOX`，所以完整 CI 对这个用例只覆盖普通模式；测试本身没有自动启动第二次沙箱运行。 | 在 Linux job 自动增加真实沙箱精确用例是审查 P2 建议。现有规格要求两种模式“验收”，但未要求两种模式都必须由 CI 自动执行；本地证据已覆盖两种模式。因此该 P2 不阻止 038，也不应自动扩大为 workflow 需求。 |
| 038 §0-A 沙箱启动确认闸 | **未满足；本地真实 bwrap 证据否定 PID 1 自停前提** | 现有 bwrap argv 使用 `--as-pid-1`。本地读取目标进程 `/proc/<pid>/status` 得到 `NSpid: <host-pid> 1`；按当前 bwrap 参数运行 `kill -STOP $$; echo RESUMED`，输出 `RESUMED` 并以 0 退出。[Linux `pid_namespaces(7)`](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html) 说明，PID namespace init 只接受同 namespace 进程发送的、已安装 handler 的信号；SIGSTOP 不能安装 handler，只有来自祖先 namespace 的 SIGSTOP 会被强制递送。结合实跑可确认：PID 1 对自身 SIGSTOP 不会停下，`kill` 成功返回也不会触发当前 `|| kill -KILL $$` 备用分支。 | 用户命令可能在宿主完成隔离/清理身份确认前运行；当前确认函数发现异 namespace 子进程后立即发送一次 SIGCONT，未证明载荷处于 `T`。修复需要调整 PID 1/载荷启动结构，并重新验收 exec、MCP、apply_patch 和 JS worker 的隔离、进程组与回收语义。本轮 Windows 编译候选未改这项架构。此项是已有 §0-A 要求的实现缺口，不是新增审查建议；阻止 038 整体完成。 |

### 当前候选证据的适用范围

- worker 回收、MCP 截断和 pipe Ctrl+C 的记录对应当前工作区中的各自差异，可复用为定向验收证据。
- G2 测试夹具的 Linux 精确用例通过，证明 JSON 输入保留 `/sub/..` 且解析器保留原始字符串；它不证明 Windows 行为。简化审查与本次差异审查均未发现问题。
- 这些定向结果没有替代完整 L3，也没有产生新的 Windows/macOS hosted runner 结果。
- 11 月 3 日记录的容器 PID/fork 耗尽曾阻断 L3；11 月 4 日已有完整 L3 实际运行。因此本次不把那条旧环境阻断标成当前阻断。本轮没有运行测试是用户明确限定，不是环境失败。
- workflow 无工作区改动；本次检查没有发现工作流内写入真实凭证。

## 推荐的下一项目标

**完成 Windows G2 精确用例验收。**本地夹具修复和 Linux 精确验证已完成；下一步只需在 Windows 执行该精确用例并核对原始 `sub\\..` 输入。

- **已完成范围：**只改 `src/app/tests/request_permissions.rs` 中 `normalize_rejects_missing_and_accepts_dotdot` 的测试输入构造和必要诊断；没有改生产路径归一化逻辑或 workflow。
- **剩余最小验证：**在 Windows 运行同一精确测试，检查提交给解析器的输入仍包含 `sub\\..`，且规范化结果符合断言。
- **完成条件：**Windows 上 canonical 基路径保留 verbatim 前缀；测试输入经解析后仍包含原始 `sub\\..`；精确测试通过。Linux 精确结果已记录。
- **远端动作：**此 Windows 结果需要由 Windows runner 执行。当前 workflow 的 `pull_request` 事件会运行三平台全量 job，而不是单个精确用例；要取得 hosted CI 证据，须提交并推送候选到带有该工作流 PR 的分支，触发 CI。当前分支本地领先 9 个提交并有未提交差异；推送会发布这些提交，未提交差异需先另行纳入提交。当前没有验证 PR 是否存在，也没有提交、推送或触发 CI。本轮禁止这些动作；远端验证前需先确认提交范围并取得用户授权。

## 用户决策

- G2 测试夹具的本地改动无需额外产品取舍；Windows 验收仍待执行。提交、推送并触发 hosted CI 超出本轮授权，执行前需确认候选提交范围并取得用户授权。
- 旧规格状态行还记录了 `origin/main` 误推处置待决定。该历史运维事项不影响本分支本轮验收；本次未检查或操作远端。按本轮指示，B 不合并。若要回退 `origin/main` 上的历史提交，仍需单独决定该远端变更。
