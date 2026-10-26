# 027 方案 B 产品实现（2026-10-26）：TDD 与验证记录

分支 fix/025-security-hardening（基线 62543a6）。隔离模式启用＝环境变量
`TCHAT_ISOLATED_TOOLS=<用户名>` 非空；未设＝普通模式零变化。规格依据：027 规格
附录「B 产品实现」章。

## RED/GREEN（每项先测后码）

| # | 项 | RED 证据 | GREEN |
|---|---|---|---|
| T1 | 记忆 git hooksPath（B⑤） | `commit_does_not_execute_planted_hooks`：假 pre-commit（仅 touch 标记）在提交时被执行→断言「标记不得出现」panicked（根缺陷可见） | 1 passed；workspace 既有 7 用例回归通过。改动＝run_git_env 固定 argv 加 `-c core.hooksPath=/dev/null`（workspace.rs） |
| T2 | context TOCTOU（B⑥，上轮 E5 延伸） | `race_symlink_swap_between_probe_and_open_is_not_read`：接缝（TCHAT_CTX_RACE_SEAM，仅 cfg(test)）在 probe→open 间隙把 AGENTS.md 换为符号链接→**替换目标正文被静默读入**（notices=[]，连提示都无） | 1 passed；context_injection 15 项集成回归通过。改动＝load_source_text 改 O_NOFOLLOW 打开（libc；非 unix 行为不变） |
| T3 | apply_patch 隔离（B③＋main `--internal-apply`） | 编译期缺失（E0425 `apply_patch_isolated` 不存在；022 同款 RED 形态）。实现中发现并修正：集成测试进程 current_exe 是 harness→改 `apply_patch_isolated_with_exe` 显式传 CARGO_BIN_EXE；sudo env_reset 删除接缝 env→标记接缝不可达→**改用更强证明**：`--internal-apply` 结果附子进程真实 uid（隔离目标 nobody＝uid 直证），标记接缝随之删除（两轴复查确认无消费者后清理） | 3 passed：internal 入口（stdin→结果 JSON＋uid）＋隔离 Add（uid＝目标用户、内容正确）＋隔离 Update/Delete（三类操作，身份直证） |
| T4 | exec_command 隔离（B①） | `isolated_mode_runs_as_target_user_without_inherited_env`：`id -un` 返回 dev（主进程身份）、主进程哨兵 env 可见于子进程→双断言失败 | 1 passed：身份＝nobody＋哨兵 env 零可见。改动＝proc_session `build_tool_command`：sudo -n -u <user> -- /usr/bin/env [TERM=…] /bin/sh -c <cmd>；Command env_clear；spawn_piped/spawn_pty 共用 |
| T5 | MCP stdio 隔离（B②） | `a027_isolated_stdio_runs_as_target_user_with_explicit_env`：record startup uid=1000（dev）→断言失败。过程修正：测试先锁 ENV_LOCK 再调 setup→自锁死（改为 setup_no_prepare）；`env K=V -- cmd` 被本机 coreutils 拒绝（rc=127，`--` 被当程序名；`env -- K=V cmd` 可用）→采用无 env `--` 形态（K=V 自带边界；sudo 的 `--` 保留） | 1 passed：服务器 uid=65534（nobody）＋显式 env（TCHAT_MCP_TOKEN_A）经 /usr/bin/env 保留＋污染 env（TCHAT_POISON_KEY）零进入。改动＝mcp/transport.rs StdioProcess::spawn 隔离包装 |

## 过程中发现并修复的缺陷（非预设）

1. **killpg 跨 uid 失效（真缺陷，实测发现）**：隔离超时变体测试暴露——超时后写循环
   存活。根因：killpg 的信号发送受 uid 检查，主进程（dev）对组内 nobody 成员
   SIGKILL 返回 EPERM；sudo/sh 同组实测（pgid 相同）但降权成员收不到信号。
   修复＝`kill_group_cross_uid`：killpg 后由 sudo 以目标用户身份对同组补发同一信号
   （幂等；SIGKILL 与 SIGINT 两路径：monitor 终止＋管道 Ctrl+C）。修复后
   `isolated_mode_pty_timeout_cancel_semantics_preserved` 全过（PTY 回显＋超时组杀
   写循环停止）。**部署含义**：该机制对称成立（tchat-app 主进程＋dev 工具），但须
   在管理员部署验收中真实复核。
2. MCP env 注入边界：env 值经 execve argv 直传（不经 shell 解析）。
3. 常量统一：SUDO_BIN/ENV_BIN 收敛到 catalog（三处引用），无重复定义。

## 启动可见性

`catalog::isolated_mode_notice()`：main 行式（audit 循环 chain）与 TUI（startup_notices
push）两路输出「工具隔离模式：exec/MCP/apply_patch 以 <用户> 身份执行」；未启用零输出。

## 定向回归（全绿；时点＝两轴复查处置完成后、安全攻防 F1-F3 处置前的快照与处置后复跑）

- exec_command 20（含隔离 3：身份/env/PTY+超时）
- apply_patch 36＋apply_patch_isolated 2
- mcp_stdio 36（含 a027）＋mcp_config 17＋mcp_turn 37＋mcp_http 13＋mcp_resources 19＋mcp_l1 14＋tool_loop 13
- context_injection 15＋lib（workspace 7＋TOCTOU race）
- e2e_acceptance 143（12 目标连跑时曾 1 失败＝s103 负载时序；单独重跑全绿复证）
- Clippy（--lib --tests）：本批涉及文件零警告

## 验证边界（如实）

- 本环境隔离目标测试用 nobody/dev（NOPASSWD: ALL 下真实 sudo 链路），**非 tchat-app
  身份全链路**；sudoers 白名单收紧后的放行性、PTY×sudo×setsid 组合的部署语义、
  wrapper 启动方式——仍属部署前提（管理员验收），不以本环境结果宣称完成。
- kill_group_cross_uid 在本环境以 nobody 目标验证；部署时目标=dev，机制对称，
  语义须管理员复核一次。

## 安全攻防处置（2026-10-26 深夜；报告 2026-10-26-plan-b-security.md，结论＝有条件通过）

三条隔离主链路（exec/MCP sudo 降权＋env、apply_patch sudo＋internal-apply、跨 uid 补杀）
与 hooksPath 防线在全部攻击构造下未被突破。处置：

- **F1（中，已修复）**：目录组件符号链接残余面——原 TOCTOU 修复只护末组件，
  global_dir/项目链目录本身为 symlink 时正文静默读入（standalone 实证穿透）。
  修复＝context.rs 逐前缀 symlink_metadata 检查（静态链拒绝）＋末组件 O_NOFOLLOW
  （竞态兜底）；RED `symlinked_directory_component_source_is_rejected`（notices=[]
  穿透）→GREEN，context_injection 15 项回归过。E5 表述已在 boundary 日志收窄更正。
- **F2（低，已修复）**：TCHAT_ISOLATED_TOOLS 加入 TRUST_CONTROL_VARS（.env 不得
  启用/改换隔离目标——「.env 不是信任根」语义对齐，catalog.rs）。
- **F3（低，已修复）**：隔离用例进程级 env 竞态——exec_command.rs 与
  apply_patch_isolated.rs 各加 ISOLATION_LOCK 互斥；并行（4 线程）下新用例全过。
  exec_b07_concurrent_groups_isolated 在非约定并行度下失败系 016 时代既有属性
  （与隔离 env 无关，最后改动 5f6a860），约定串行模式全过，不在本批范围。
- **F4/F5/F6/F7/F8/F9（通过/信息）**：--internal-apply 本地面定性（无提权）、sudo
  失败面零补丁泄漏、env 注入面字面直传、跨 uid 补杀产品形态有效（注意 procps 版本
  敏感与 dash 内建不可用——/bin/kill 绝对路径必要）、hooksPath 对 git 2.43 有效
  （极老 git <2.9 忽略未知 -c，部署锁版本）、TOCTOU 末组件双层有效。
- **F10（文档，已更正）**：`env K=V -- cmd` 才是真实失败形态（`env -- K=V cmd` 可用）
  ——transport.rs 注释与本文件 T5 归因更正；「定向回归全绿」加时点限定。

## 收口三：父目录竞态（2026-10-26 晨；应用户指令）

上轮 F1 修复为「逐前缀检查＋末组件 O_NOFOLLOW」——属"先检查、后打开"，检查到打开
之间可写父目录中的组件仍可被 rename-swap 成符号链接（检查已过、O_NOFOLLOW 不护
目录组件）。本轮按指令收口：

- RED：接缝移至全部检查之后、打开之前；新测试
  `parent_directory_swap_between_checks_and_open_is_not_followed`——接缝间隙把
  global_dir rename swap→symlink 指向含 PARENT-RACE-MARKER 的另一目录→**标记被
  静默读入（notices=[]）**。
- GREEN：打开本体改 `open_nofollow_chain`——逐组件 openat，目录组件
  O_DIRECTORY|O_NOFOLLOW、末组件 O_NOFOLLOW，每级相对上一级 fd 解析（无检查-打开
  窗口）；任何组件被替换即该级 ELOOP。fd 链失败全关。any_component_symlink 保留
  仅作提示分类（ELOOP→「符号链接」，其余→「读取失败」），安全职责不再依赖检查。
  非 unix 维持 std File::open。
- 验证：lib 3 个 TOCTOU 用例（父目录/末组件/静态目录链）全过＋context_injection
  15 项＋e2e 143 项＋最终全量见下。gitleaks 零泄漏。
- 聚焦安全审查与全量结果：见本文件追加段与本目录 2026-10-26-plan-b-security.md。

## 收口三验证与审查处置（2026-10-26 晨定稿）

- 聚焦安全审查（有条件通过→处置后通过）：链接/竞态拒绝边界成立（Rust 逐字复刻＋
  Python 双实现攻防，父目录/中间目录/末组件/连环替换全部拒绝、evil 零读取；提示
  分类正确；O_CLOEXEC 已置；还原后读原文无陈旧句柄）。处置：①竞态测试互斥锁
  （并行假红，4 线程复验 3/3）；②失败路径补 close(cur)（100 次 ELOOP 拒绝 fd 零
  泄漏回归断言入 dirsym 测试）；③空路径防御早返回；④docstring/注释同步。
- 独立 code-review（按 REVIEW.md）：open_nofollow_chain 本体通过（窗口消除确认）；
  fd 泄漏发现与安全审查同根因（已修）；规格第 6 条措辞滞后→已同步收口三机制。
- 自查发现（修复前）：成功路径中间目录 fd 泄漏——先行修复（成功/失败对称关闭）。
- 最终全量（parent-race-full.log）：exit 0，65 目标 **3490/0/3**（＋1＝父目录竞态
  回归）。Clippy（--lib）零警告；gitleaks 零泄漏。过程中全量两次被宿主侧强杀
  （e2e 后无退出码；疑似 OOM/回收）——第三次以脱离进程＋活性监控完成。
