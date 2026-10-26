# 033 宿主真实沙箱验收记录（2026-10-27）

**结论：全部 12 项 PASS，脚本退出码 0——真实沙箱验收通过，可进入合并决策。**

原始证据（不重复抄写，以下两份为本记录的依据）：
- 运行日志（含逐步时间戳与汇总）：`docs/logs/033-tool-sandbox/2026-10-27-host-acceptance-run.log`
- 全量测试日志（第 8 步落盘）：`docs/logs/033-tool-sandbox/acceptance-full.log`

## 1. 执行环境

| 项 | 值（日志实证） |
|---|---|
| 执行位置 | Windows 宿主 Git Bash（Docker Desktop 客户端，未开放 TCP、未动共享容器） |
| Docker | Client 29.8.2 windows/amd64；Server Docker Desktop 4.95.0（Engine 29.8.2，linux/amd64，runc 1.5.1） |
| bwrap | bubblewrap 0.9.0（验收镜像内 apt 安装） |
| seccomp 基线 | **moby/profiles/seccomp v0.2.3**——按引擎 `docker-v29.8.2` 的 `vendor/modules.txt` 锁定版本取得（moby v28 起默认 profile 移入独立模块，主仓库 tag 布局变化，本轮适配） |
| 基线补丁 | `scripts/acceptance/generated-033-seccomp.json`：基线 33 条规则 → 前插 6 条（两种 clone 等值＋unshare(NEWUSER) 等值＋mount/umount2/pivot_root），默认条目不动 |
| 验收容器 | `tchat-033-accept`（dev 用户；仅 bind worktree；不挂凭据目录/会话目录/docker socket；容器私有 CARGO_HOME/TARGET_DIR；运行结束已删除） |
| 镜像 | `agentlab-033-accept:latest`（dev 镜像＋bubblewrap；构建上下文仅验收目录；结束已删除） |

宿主执行面说明：宿主无 Rust 工具链且未安装任何东西；编译/测试全部在容器内。宿主仅运行
docker CLI 与 curl（下载基线）；python3 补丁生成经临时 shim 委托 WSL Ubuntu 既有 python3
（shim 为临时文件，验收后已删除）。全局未导出 MSYS_NO_PATHCONV，docker 调用点经临时
包装脚本按调用禁用 MSYS 路径转换（原因见 §3 S3）。

## 2. 验收结果（对照目标逐项）

| 目标 | 结果 |
|---|---|
| bwrap 干跑成功，真实沙箱分支被执行 | PASS：`probe_available == true`（与产品 probe_bwrap_at 同参数）；第 3 步对照默认策略拒绝 userns（非孤证） |
| 假凭据沙箱外存在、沙箱内取不到正文 | PASS：外 19 字节含 `FAKE-SECRET-LINE`；沙箱内该路径为 /dev/null 空绑定（`character special file size=0`），`test ! -s` 通过、`cat` 取不到正文（nodev 强制下 open EACCES，见 §3 S5） |
| 普通工作区文件正常读写 | PASS：沙箱内 acc-write.txt 写读回显成功 |
| exec / MCP stdio / apply_patch 三型正常 | PASS：exec_sandbox 2/2、mcp_stdio_sandbox 1/1、apply_patch_sandbox 3/3（真实分支；Update＝/proc/self/fd 恢复实证） |
| PID 隔离、PTY、取消、超时、子进程回收 | PASS：7a 沙箱内 pid1=载荷进程、可见进程数 4（容器面进程不可见，G3 关闭）；跨 PID ns 的 016 PTY/超时组杀契约随 exec_sandbox 全绿 |
| 普通模式回归 | PASS：exec_command 18/18＋mcp_stdio 35/35 零变化 |
| 最终完整测试 | PASS：`cargo check --locked --offline && cargo test --workspace --no-fail-fast --locked --offline`（串行）——**68 目标 3506 passed / 0 failed / 3 ignored**（上一轮 3505＋本轮新增 1 用例） |

单元（lib sandbox::）16/16（原 15＋新增 `wsl_interop_mask_follows_dir_presence_flag`）。

## 3. 本轮发现与处置（首跑即暴露，均为此前从未真实执行的路径）

**产品缺陷（TDD 修复，先 RED 后 GREEN）：**
- P1 `--tmpfs /run/WSL` 无条件遮蔽：非 WSL 环境（Docker 容器）无该目录，bwrap 在
  ro-bind 根上 mkdir 失败（EROFS）→ 沙箱整体不可用。修复＝按上游
  `mask_wsl_interop` 的 `is_dir()` 判定条件化（codex linux_run_main.rs:278）。
  RED＝新测试对 3 参旧签名 13×E0061 编译失败；GREEN＝16/16。
  **RED 证据性质澄清（合并前收口纠正）**：E0061 是**编译期失败**（旧 3 参签名
  的编译错误），不是运行缺陷复现；EROFS 是首轮真实运行的现象描述，其原始输出
  **未留档**（`2026-10-27-host-acceptance-run.log` 为修复后成功轮日志，全文无
  EROFS/E0061 记录，已核实）——本记录不将 EROFS 引述为有日志佐证的运行复现，
  P1 的可复验证据＝E0061 编译期 RED＋修复后 16/16 GREEN＋条件化双分支测试
  （`wsl_interop_mask_follows_dir_presence_flag`）。
- P2 MCP stdio 环境注入顺序：平台缺省白名单 HOME（主进程值）在沙箱白名单之后落地，
  覆盖沙箱锚定 HOME。修复＝沙箱白名单最后落地（普通模式逐字节不变）。
  RED＝a033 集成测试真实分支首跑断言失败（HOME=/home/dev≠锚定根）；GREEN＝1/1。

**既有测试健壮性缺口（未改产品/测试代码，验收侧规避＋记录）：**
- T1 `tchat-tui --test security_untrusted_text` 的 `a7_byte_level_writer_passthrough_check`
  经 crossterm 查询终端尺寸，`docker exec` 无控制终端时失败（/dev/tty ENXIO → 查询
  EAGAIN）。A/B 实验证明与 seccomp/bwrap 无关（纯 dev 镜像默认策略同样失败），
  `exec -t` 下 12/12 全绿；dev 容器历史全绿亦运行于带 TTY 的交互会话。验收脚本第 8
  步以 `docker exec -t` 执行（命令本体逐字不变）。**后续 CI 化时需处理该 tty 依赖。**

**验收脚本/工具链修复（均在现成验收入口的授权范围内最小修复并复验）：**
- S1 seccomp 基线获取：moby v28 起布局变化（profile 移入 moby/profiles、引擎 tag 改
  `docker-v<版本>`）→ 重写为 vendor 锁定链条＋旧布局回退；api.github.com 优先
  （raw.githubusercontent 本网络有分钟级连接重置窗口）。
- S2 补丁规则勘误：探针 clone flags 数值勘误（0x18020011＝**402680849 错，
  402784273 对**）；新增 `unshare(CLONE_NEWUSER)` 等值规则——`--dev`（devpts）场景
  bwrap 建第二级 userns 恢复 uid（bubblewrap.c:3239，实测 EPERM「unshare user ns」）；
  初版「bwrap 不调用 unshare」结论只覆盖第一级 clone 路径。
- S3 全局 `MSYS_NO_PATHCONV=1` 使原生 curl 收到未转换 `/tmp` 输出路径而写失败
  （rc=23，两轮「基线无法取得」的真实根因，非网络）→ 改为 docker 调用点级禁转换。
- S4 `--nocapture` 位置（须 `-- --nocapture`）；7a 断言写反（`--as-pid-1` 实测
  pid1=载荷进程）；7b 补产品同构的遮蔽挂载＋断言改 stat 级；`grep -q` 大输出在
  pipefail 下被 SIGPIPE 误杀（普通回归步两次误杀根因）→ here-string；全量数字
  校验在单括号 `[ ]` 中误用通配语义 → 显式分段比较；MOBY_TAG 残留引用（set -u）。
- S5 语义修正记录：userns 非特权 bind 强制 nodev → /dev/null 遮蔽的 open 为 EACCES
  而非读到 0 字节（「取不到正文」才是目标语义，与产品 A2 断言一致；上游 perms 000
  方案同为不可 open）。

## 4. 本轮变更清单（工作树内，未提交）

产品代码：`src/app/src/sandbox.rs`（P1）、`src/app/src/mcp/transport.rs`（P2）、
`src/app/src/sandbox/tests.rs`（P1 测试）、`src/app/tests/mcp_stdio_sandbox.rs`（清理未用导入）。
验收配套：`scripts/acceptance/033-sandbox-accept.sh`、`033-seccomp-patch.py`、
`generated-033-seccomp.json`、`033-accept-bwrap.Dockerfile`（未改，仅引用）。
文档：本记录＋规格状态更新。

## 5. 遗留与边界

- **dev 容器部署变更仍未实施**（安装 bubblewrap＋compose `security_opt` seccomp 补丁，
  见 `2026-10-26-sandbox-env-plan.md` §2）——需管理员批准，本轮未动共享容器；本次
  验收在独立临时容器证明机制与策略基线可行，dev 容器接入按既有方案执行即可。
- tty 敏感测试（§3 T1）CI 化需注意。
- 真实 .env（主仓库根）原位未读未改；未输出任何真实凭据；未提交/合并/推送；
  分支 `feat/033-tool-sandbox` 保持待验收候选状态。
