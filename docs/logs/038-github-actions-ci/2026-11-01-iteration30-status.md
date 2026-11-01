# 038 迭代轮 30 三平台状态（2026-11-01，commit 9a1e65c）

## ubuntu: 3852/4
4 失败＝D1-D3 runner 异常真实检查（c13/b2_mcp/attack06/sandbox_matrix）＋已登记存量。本地全量 3854/2/6。

## windows: 3110/219（编译清零＋SHELL cmd /C 修复）
SHELL 平台等价（cmd /C）修复后 3110 用例通过。剩余 219 失败＝测试夹具使用 sh 语法命令串
（echo >/printf >/mkfifo 等），cmd /C 无法执行。需逐目标适配 Windows 等价夹具（cmd 语法的
echo/重定向）或引入跨平台命令构造——为下一轮主工作（~8 个目标：exec_command/exec_sandbox/
apply_patch_sandbox/context_management/context_perf/js_orchestration/js_orchestration_perf/mcp_config 等）。

## macos: 仍运行中（PTY 卡死截断前）
读路径卡死诊断（D-mac）待下一轮。
