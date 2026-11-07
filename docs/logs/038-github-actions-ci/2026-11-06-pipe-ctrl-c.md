# 038 沙箱管道 Ctrl+C 验证记录（2026-11-06）

## 结果

普通模式、真实 `TCHAT_SANDBOX=1` 模式的精确回归用例均通过。沙箱模式下，SIGINT 忽略载荷在 Ctrl+C 后保持运行并继续写日志；同一会话的 `terminate=true` 在 3 秒内完成回收并停止写入。独立会话在目标中断和终止期间保持运行。修复未改变 wait/reap 所有权或容器配置。

## 根因与修复

沙箱确认闸放行前记录的宿主进程组不是最终载荷组。bwrap 使用 `--new-session` 后，载荷 PID 成为它的 PGID；旧逻辑仍向此前记录的 PGID 发信号，可能命中 bwrap 监督进程，或错过载荷。定向跟踪观察到记录目标 PGID 为 509、载荷实际 PGID 为 510，扫描后信号数为 0；失败复现见 `target/038-validation/pipe-c13-sandbox-trace-final-20261106.log`。

沙箱放行确认现在同时返回 PID 命名空间 inode 和载荷 PID。会话单独保存中断目标 PGID：沙箱使用载荷 PID，普通模式继续使用子进程组。沙箱 Ctrl+C 按命名空间 inode 与 PGID 双重筛选 `/proc` 进程后发送 SIGINT，排除可能共享初始 PGID 的 bwrap 监督进程。终止和回收路径仍使用既有会话进程组与 namespace sweep。

信号系统调用失败不会再被当作成功：普通模式保留 `killpg` 错误，沙箱扫描保留非 `ESRCH` 错误；没有任何载荷进程收到信号时返回错误。若请求期间会话已自然退出，则继续读取实际终态；若会话仍运行，则把错误和原句柄返回。信号发送时持有会话锁，保留既有中断与同句柄终止请求的串行顺序。

回归用例以 READY 文件确认信号处理器已生效。Unix 分支确认可响应载荷实际退出码为 130，并验证忽略 SIGINT 的会话在中断后仍写入、终止后停止写入；同时用独立会话哨兵确认信号和清理没有影响其他会话。Windows 保留原有整树终止分支。

## 实测

以下命令均在最终候选上串行执行，使用 `CARGO_TARGET_DIR=/workspace/target`、`CARGO_BUILD_JOBS=1`、Cargo 离线模式和 180 秒外层超时。每 5ms 监视一次 `/proc` 进程数与 cgroup 任务数；未使用 subreaper，也不替产品清理进程。

| 模式 | 命令 | 结果 | 日志 |
|---|---|---|---|
| 真实沙箱 | `TCHAT_SANDBOX=1 cargo test --locked --offline --test proc_session pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process -- --exact --nocapture` | 1 通过，0 失败，27 过滤；17.95 秒。门禁峰值 11 个可见进程、259 个 cgroup 任务 | `target/038-validation/pipe-c13-sandbox-final-verified-20261106.log` |
| 普通模式 | `env -u TCHAT_SANDBOX cargo test --locked --offline --test proc_session pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process -- --exact --nocapture` | 1 通过，0 失败，27 过滤；17.79 秒。门禁峰值 9 个可见进程、246 个 cgroup 任务 | `target/038-validation/pipe-c13-ordinary-final-verified-20261106.log` |
| C13 进程句柄回归，真实沙箱 | `TCHAT_SANDBOX=1 cargo test --locked --offline --test js_orchestration c13_terminate_keeps_016_process_handle_alive -- --exact --nocapture` | 1 通过，0 失败，40 过滤；5.62 秒。门禁峰值 10 个可见进程、258 个 cgroup 任务 | `target/038-validation/c13-016-handle-sandbox-final-verified-20261106.log` |

当前精确用例核实响应型载荷实际退出码为 130；忽略 SIGINT 的目标收到有效信号请求后仍报告 Running，日志在 Ctrl+C 后继续增长；同一句柄可再交互并以 `terminate=true` 在 3 秒内返回 Terminated，之后日志停止增长。独立会话在目标中断和终止后仍运行并继续写入，随后由测试通过同一管理器句柄明确终止。源码中的 `monitor_thread` 在设置 `Phase::Terminated` 前先调用产品拥有的 `child.wait()`；该运行没有外部 wait/reaper 清理，因此验收来自产品管理器本身。

第一次带跟踪的沙箱复现仍失败，并发现确认闸前后 PGID 不同；修正为使用确认返回的载荷 PID 后，当前候选上的普通、真实沙箱精确用例及 C13 回归均通过。简化检查移除了同文件中无关的 `explicit_timeout_kills_without_any_query` 错误消息调整；保留用于分开中断目标 PGID 与终止组、核对 sandbox namespace 的最小差异。`git diff --check` 通过。

## 覆盖范围与限制

038 当前 CI 不预设 `TCHAT_SANDBOX`，因此 CI 全量入口对此用例只覆盖普通模式；真实沙箱模式本轮由本地精确命令验证。代码审查也将此记为 P2 覆盖缺口。本轮按既定范围不修改 CI 工作流；若需要让未来 CI 自动阻止沙箱路径回归，应另行把真实沙箱精确用例接入适用的 Linux job。

本轮未运行 `cargo check`、全量测试或 L3，也未执行 CI。C13 是因本次共享 `proc_session.rs` 和沙箱启动确认路径改动而补跑的直接回归。
