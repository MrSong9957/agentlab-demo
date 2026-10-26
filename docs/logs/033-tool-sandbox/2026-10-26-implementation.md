# 033 实现轮验证记录（2026-10-26）

工作树：/workspace/.worktrees/033-tool-sandbox（分支 feat/033-tool-sandbox，
基线 a14a57c/main；全部改动未提交——按任务约束不提交/不合并/不推送）。
Cargo 目标目录：/tmp/wt033-target（显式指定，隔离全局共享构建产物）。

## 安全攻防发现与处置（2026-10-26-sandbox-security.md）

| 编号 | 发现 | 处置 |
|---|---|---|
| G1【阻断】 | exec `workdir` 模型可控，策略随其锚定→换 workdir 即让真实 .env 脱离遮蔽 | **已修**：tool_launch_argv 内部锚定主进程 cwd（canonical），接口删除 workspace 参数——调用方无法转移锚定；workdir 仅保留 current_dir 语义；新增锚定回归断言（exec_sandbox 锚定根 .env＋接口无参） |
| G2【阻断】 | 非 canonical 可写根（`sub/..`）与 deny 词法失配→遮蔽先于 bind 被重开 | **已修**：可写根词法清洁校验（absolute_lexical≠原值→显式拒绝）；symlink 可写根不受限（上游 canonical bind＋deny remap 语义，T1e 保留）；新增 non_canonical_writable_root_fails_closed 回归 |
| G3 高危 | 同 uid＋不 unshare-pid：工具可 kill 主进程／读其 /proc 原始 environ（yama=1 不拦 READ） | **记录为已知边界**（与上游 codex 差异项）：修复需恢复 --unshare-pid＋--proc 并在 bwrap 环境实测 016 PTY/取消/超时契约——本环境无 bwrap，不盲改；部署要求 wrapper 最小化 environ（凭据不经启动环境）。列入真实验收前提 |
| G4 高危 | WSL interop 遮蔽裁剪：MZ execve 经宿主 /init 在 Windows 侧执行（本部署 interop 活跃实测），绕开 Linux 挂载策略 | **已修**：argv 恢复上游成对遮蔽 `--tmpfs /run/WSL`＋`--tmpfs /proc`（T1 期望更新） |
| G5 中 | bwrap 经主进程 PATH 解析可被工作区内栽赃（条件性） | **已修**：find_bwrap_in_path 跳过位于主 cwd 下的候选（宁可显式不可用） |
| G6 低 | MCP 面 child_env 先注后被链式 env_clear 清空 | **已修**（审查轮同步发现）：env_clear 在前、child_env 后注入；mcp_stdio_sandbox 补 HOME=锚定根断言 |
| G7/D1 低 | .env 注入 PATH 边缘／规格措辞失实 | G7：dotenv 不覆盖已存在变量，正常部署不成立，已知边缘；D1 及声明失实：规格适配声明已重写 |

审查后定向复验：sandbox 单测 14/14（新增 G2 回归）、exec_sandbox 2、
mcp_stdio_sandbox 1、apply_patch_sandbox 3、exec_command 18（普通回归）、
mcp_stdio 35（普通回归）全绿；修复项由安全审查代理二次复验——**通过**
（G1/G2/G6 关闭；G5 部分覆盖建议保留；G3 读面因 `--tmpfs /proc` 落地已缓解、
kill 面为已知边界），并新发现 **G4-R1**（`--tmpfs /proc` 使沙箱模式 apply_patch
Update 经 /proc/self/fd 必败——fail-closed 功能限制，非安全缺口）。处置：
安全优先保留遮蔽（撤掉会重开 /proc/<宿主>/root 穿透逃逸面）——最终修复
（2026-10-26 收口轮，先 RED 后 GREEN）：恢复上游成对机制 `--unshare-pid`＋
`--as-pid-1`＋`--proc /proc`（create_bwrap_flags；上游 launcher argv.insert(1)
与 mount_proc 语义），撤 `--tmpfs /proc`——`/proc/self/fd` 消费者恢复（Update
修复）、主进程退出沙箱 /proc 视图（G3 关闭）；回归测试
pid_namespace_and_proc_paired（T9，RED→GREEN）＋T1 期望同步，单测 15/15、
沙箱集成 6/6 绿。跨 PID ns 的 016 killpg/PTY/取消/超时契约重验列入 bwrap
环境验收清单。详见安全报告 §8。

## 验证命令与结果（真实退出码直取）

| 检查 | 命令（worktree 内） | 结果 |
|---|---|---|
| 类型检查 | `cargo check --locked --offline` | 通过（0 error；既有 TUI 层 warning 非本次引入） |
| sandbox 单元 | `cargo test --lib sandbox::` | **14 passed / 0 failed** |
| exec 沙箱集成 | `cargo test --test exec_sandbox` | **2 passed / 0 failed** |
| MCP 沙箱集成 | `cargo test --test mcp_stdio_sandbox` | **1 passed / 0 failed** |
| apply_patch 沙箱 | `cargo test --test apply_patch_sandbox` | **3 passed / 0 failed** |
| exec 普通模式回归 | `cargo test --test exec_command` | **18 passed / 0 failed** |
| MCP 普通模式回归 | `cargo test --test mcp_stdio` | **35 passed / 0 failed** |
| 敏感性（变异） | 禁用 symlink fail-closed 检查 → T2/T3 | **2 failed（变异生效）**；恢复后全绿，无残留 |
| 泄密扫描（026 等价） | `gitleaks detect --no-git --source src/app`（worktree 配置） | **exit=0** |
| 全量 | `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 cargo check --locked --offline && cargo test --workspace --no-fail-fast --locked --offline` | 见下「全量」节 |

## 全量（收口轮终验，2026-10-26 冻结代码后重跑；日志 /tmp/033-full2.log）

命令：`CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 cargo check --locked --offline &&
cargo test --workspace --no-fail-fast --locked --offline`（worktree 内，
CARGO_TARGET_DIR=/tmp/wt033-target；**代码冻结后运行，运行期间零改动**）。

结果：`exit=0`——**68 个测试目标、3505 passed / 0 failed / 3 ignored**。

**与前次全量（3502/1）差异的准确解释**：
- 失败的 1 例＝`sandbox::tests::writable_root_symlink_binds_canonical_target`：
  前次全量启动于该用例补建 `.env` 夹具**之前**，编译的是旧版测试二进制（用例
  断言 deny 路径 remap 遮蔽，旧版未创建夹具文件→no-op→断言失败）。修复（夹具
  补建）后单跑、串行、本轮全量三度全绿——非顺序依赖、非产品缺陷。
- passed 3502→3505：+3＝收口轮新增（T9 `pid_namespace_and_proc_paired` 成对
  机制回归＋夹具修复使 T1e 转绿＋用例数微差），全部为 sandbox 单测。
- 3 ignored＝历史基线既有（非本轮引入）。

## 前次全量（修复前代码；日志 /tmp/033-full.log，已被本轮取代）

68 目标 3502/1/3——沙箱面结果为修复前二进制，已被本轮取代；普通模式面与本轮
一致（None 路径未变）。

## 历史进程泄漏（非本轮产生，如实记录）

宿主存在两条此前批次的泄漏测试循环（本轮 exec_sandbox 的 PTY 组杀验证正常，
未新增泄漏）：
- pid 474850：`/bin/sh -c while true; do echo x >> /tmp/tchat-exec-474826-iso-timeout/w.log ...`——**nobody 属主、PPID=1（孤儿）、14h39m**；w.log 已 2MB。定性：**027 B 批次** exec_command 隔离 PTY 超时用例（TCHAT_ISOLATED_TOOLS=nobody）的组杀失效实证残留——killpg 对降权成员 EPERM、补杀未覆盖后台写孙进程；该用例代码在本分支已被同 uid 沙箱版替换。
- pid 935465：`/bin/sh -c while [ ! -e /tmp/tchat-test-934929-s103-user-357117770/work/gate ]; do sleep 0.05; done`——**dev 属主、PPID=1、8h45m**；gate 永不出现。定性：**024 批次** s103 用例残留（与 024 记录「s103 连跑 1 失败后单独复证」吻合）。

两条均为已完成交付批次的孤儿残留（无害：/tmp 写与空转），本轮按指令未终止，
交由用户决定清理。

## gitleaks 口径说明

026 门禁＝`gitleaks protect --staged`（只扫暂存内容）。本轮验收用
`detect --no-git`（工作树快照）：全仓口径下 16 命中全部为存量内容
（025 值锚定豁免覆盖的三个源码假值指纹＋024 时代 performance-*.json 旧文件，
后者不在 025 豁免路径内属历史遗留），**本次新增/修改文件零命中**；
worktree 下 `--source src/app` exit=0（主区同口径 exit=1 系 025 豁免的
`performance-final-025.json` 与三个源码指纹在主区命中、worktree 因
024 旧文件不在豁免面……两区差异均为存量口径问题，与本次改动无关）。

## 测试副产物说明

worktree 根出现 `src/app/config/manual.json`（0600，测试运行副产物：provider
选择＋`env:DEEPSEEK_API_KEY` 引用，无密钥值）——untracked，不纳入提交。
