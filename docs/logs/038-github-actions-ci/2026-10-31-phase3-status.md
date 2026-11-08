# 038 第三阶段收口状态（2026-10-31，commit 5f98508）

## 本轮已验证
- Linux L3：3854 通过/2 失败/6 忽略（proc_session 管道 Ctrl-C、session_perf 预算＝已登记存量）
- sec7 GateChild 确定性重做三连稳定；attack06/c13/b2_mcp/e_reaping 真实检查恢复且本地通过（含 bwrap 0.11.1）
- macOS 平台化（/dev/fd、前缀重解、夹具、断言对齐）与 Windows 平台层（Job/锁族/ReOpenFile）实现并经 Linux 全量回归

## Windows 剩余 6 处编译错误（run 41214870300 实测，下一轮按此修）

## 根因注记
- 多轮相对路径 python 写入受 shell cwd 漂移影响，部分修复未落盘/被覆盖——后续一律绝对路径＋断言
- SeccompFd 全平台化等部分修复在合并中被 main 侧旧版覆盖，需逐项核对合并结果

## 剩余 6 处错误（CI run 41214870300 实测定位）

1. proc_session.rs:1590 — spawn_pty 的 seccomp match 第二处 pre_exec（第一处 1565 已门控）：needs #[cfg(unix)] on arms + windows unreachable arm
2. apply_patch.rs:187 — spawn 沙箱路径的 pre_exec close_inherited 门控未生效（cfg 位置错误），同款修复
3. apply_patch.rs:1245 — open_path windows 版签名（_extra_flags: ()）与调用点 E0308：统一调用参数
4. apply_patch.rs:1541 — lexical_normalize 在 windows 不可见：定义被 cfg 门控或丢失，需解除/补齐
5. workspace.rs:199 — process_group 双残留（cfg(unix) + 裸调用）：删除裸调用
6. （第 6 处与 4 同源重复计数）
