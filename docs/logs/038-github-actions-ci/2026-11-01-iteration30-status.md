# 038 迭代轮 30 三平台状态（2026-11-01，commit 9a1e65c，run 41255654319）

数字对应 run 41255654319（ubuntu/windows job 已完成，macos job 卡死截断未跑完）；**非最终 HEAD e6a62e3 的重新验证**——其后仅 2 个 [skip ci] 文档提交，无产品代码变更。

## ubuntu: 3852/4
4 失败＝D1-D3 runner 异常真实检查（c13/b2_mcp/attack06/sandbox_matrix）＋已登记存量。本地全量 3854/2/6（commit 5f98508 phase3 全量；2 失败＝proc_session 管道 Ctrl-C、session_perf 预算，存量复现证据见 `2026-10-31-phase3-status.md` 与 `2026-10-30-local-verification.md`）。

## windows: 3110/219（编译清零＋SHELL cmd /C 修复）
SHELL 平台等价（cmd /C）修复后 3110 用例通过。剩余 219 失败＝测试夹具使用 sh 语法命令串
（echo >/printf >/mkfifo 等），cmd /C 无法执行。需逐目标适配 Windows 等价夹具（cmd 语法的
echo/重定向）或引入跨平台命令构造——为下一轮主工作（~8 个目标：exec_command/exec_sandbox/
apply_patch_sandbox/context_management/context_perf/js_orchestration/js_orchestration_perf/mcp_config 等）。

## macos: 2812 通过/28 失败后卡死，测试未跑完（收口更新 2026-11-01）
run 41255654319 macos job：2812 通过、28 失败后卡死；截至 2026-11-01T01:06Z 收口检查该 job 仍 in_progress，其后测试目标未在 macOS 执行（此前同入口运行曾以 150 分钟上限截断，见规格 §6）。读路径卡死诊断（D-mac）待下一轮。

## 下一轮实施蓝图：测试夹具跨平台命令助手

### 设计
新建 `tests/common/cmd_builder.rs`（unix/windows cfg 双实现）：
- `write_file(path, content)` → unix: `printf '%s' 'content' > path`；windows: `(echo content) > path` 或 PowerShell `Set-Content`
- `append_loop(path, count, interval_ms)` → unix: `while` 循环；windows: `for /l` 循环
- `sleep(ms)` → unix: `sleep N`；windows: `ping -n N+1 127.0.0.1 >nul` 或 `timeout /t`
- `cat(path)` → unix: `cat path`；windows: `type path`
- `kill_int(pid)` → unix: `kill -INT pid`；windows: 无同构（由产品 terminate 覆盖）

### 影响范围（219 失败 → 按目标分解）
| 目标 | 失败数 | 主要模式 |
|---|---|---|
| exec_command | ~40 | write_file, sleep, cat |
| exec_sandbox | ~35 | write_file, append_loop, sleep |
| apply_patch_sandbox | ~30 | write_file, sleep |
| context_management | ~25 | write_file |
| context_perf | ~20 | write_file, cat |
| js_orchestration | ~20 | write_file, sleep |
| js_orchestration_perf | ~20 | write_file |
| mcp_config | ~15 | write_file, cat |
| 其他 | ~14 | sleep, cat |

### macOS PTY 读路径卡死 D-mac 诊断方案
在 `common/pty.rs` 的 `poll_read` 中加 bounded read：poll(50ms) 后 read 返回
`EAGAIN` 时记一次 miss，连续 miss 超 watchdog 触发 panic 并转储 `child.try_wait()`
＋captured 尾部。Windows/macOS 的 `read` 调用也套 poll 门控（当前直接 read 可能
阻塞——macOS fdescfs/BSD PTY 的 poll 行为差异待实证）。同方案写入
`e2e_acceptance.rs` 的 `read_once`。
