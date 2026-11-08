→ [返回调研汇总](../2026-11-06-038-sandbox-start-gate.md)

## 身份

对基线 b98d52a 和当前工作树的只读启动链审计。

## 事实

两处 argv builder 都带 --as-pid-1、--new-session、--die-with-parent，并启用 --unshare-pid 与 /proc。通用 confirm 只找 namespace 不同且 PPID 为外层 leader 的子进程，之后直接 SIGCONT，不核验 State T/PGID/SID。

exec 在 start_with_owner 同步确认；Session monitor 拥有 ChildHandle::wait。apply_patch 在写 stdin 前 confirm，调用者 wait_with_output。MCP 在 spawn 后 confirm，但 mcp_stdio_gate_wrap 当前只运行 exec "$0" "$@"，未发 STOP；StdioProcess::close 持 Tokio Child wait。JS 先标记 tchat-js-ready 并尝试 self-stop，host callback 已用 PIDFD、入口名/argv、State T 和独立 PGID/SID 检查；init 帧只在 Released 后发送，JS runtime 保持 payload PIDFD 观测并等待 bwrap。

apply_patch 有两种当前分支：policy Some 时设置 INTERNAL_GATE_ENV=stop 并 confirm，确认后才写 stdin；policy=None 且 TCHAT_SANDBOX=1 时 gate 值是 off、跳过 confirm，spawn 后直接写 stdin。该分支违反 038 §0-A，需纳入修复计划。

exec 的 gate 前缀在 policy Some 或 sandbox_requested 时插入；MCP 在 sandbox_requested 时包装并在 spawn 后确认；JS 仅在 sandbox_requested 时 spawn 与 confirm。JS callback 对未停止的已标记入口用 PIDFD 发 SIGSTOP，confirm T 后才释放，init 帧随后才发送。

## 推断

计划应让 bwrap 内建 PID 1 reaper 接管 namespace 子进程等待，payload 为 PID 2。四入口共用 topology、stop-state/identity 检查与 release；JS 保留更严格的 role 检查。受控信号目标为 payload group，D2 sweep 排除 reaper；原宿主 wait owner 不变。外部监督者死亡继续走已记录失败契约，不增加任意孤儿 wait 要求。

来源：src/app/src/sandbox.rs 337-382, 1355-1445; src/app/src/proc_session.rs 540-566, 2261-2345; src/app/src/apply_patch.rs 197-240, 255-270; src/app/src/mcp/transport.rs 211-325, 364-395; src/app/src/js_orchestration/runtime.rs 477-541, 858-903, 950-1055; src/app/src/js_orchestration/worker.rs 105-116。
