# 批 3：A10 五处 Windows 静态疑点＋S37 三锚（2026-11-11）

**验证边界声明：本容器无 Windows 工具链——五处修复与三个 cfg(windows) 测试锚
均经静态核实＋本机（linux target）编译通过；Windows 行为与锚的实跑待
038 Windows CI 验证（该腿 continue-on-error），不谎报已验证。**

## 逐处核实与修复

1. **UNC 三反斜杠（permissions.rs）**：证实——`format!("\\\\\\{rest}")` 为
   3 字面反斜杠，`\\srv\share` 原生形态需 2 个（path_covers 的 starts_with
   对齐失效）。修：抽纯函数 `strip_verbatim_prefix`（verbatim 剥离单点），
   UNC 臂 2 反斜杠；canonical_long_checked 委托之。
2. **WAIT_FAILED（proc_session.rs）**：清单所述「catch-all 吞掉超时分支」在
   当前代码已非原形态——`wait()`（阻塞版）直接使用 `WAIT_FAILED` 但 :35
   import 未含之＝**Windows 编译错误**（本机 cfg 不编译不可见）；
   `try_wait` 的 catch-all `_ => Err(last_os_error())` 语义正确（WAIT_FAILED
   归 Err 而非误报超时）。修：import 补 `WAIT_FAILED`。
3. **reader_eof 不累加（proc_session.rs）**：证实——Windows `spawn_reader`
   退出路径无 `reader_eof += 1`（unix 版 :2387 有），collect_done 每会话
   多等满 COLLECT_GRACE(2s)。修：Windows 线程尾累加＋notify（与 unix 同构）。
4. **context.rs raw_errno 同体矛盾**：证实——Windows 版函数体与 unix 全同
   （`raw_os_error().unwrap_or(-1)`）而注释声称「恒返回不可能匹配的值」；
   调用方 :148 比对 `libc::ELOOP`，Win32 错误码域理论可撞 POSIX 值。修：
   Windows 版恒返回 -1（与注释一致；ELOOP 提示分支确定不触发，统一「读取
   失败」提示——034「非 unix 平台维持 std File::open 行为不变」不受影响）。
5. **session 截断片段无上限**：证实——`validate_orchestration_chain` 仅拒
   「>64 KiB 且未标 truncated」；超长片段携 truncated=true 可通过。写入侧
   SNIPPET（orchestration）保证正常路径恒 ≤64 KiB，校验加严不破坏现有数据。
   修：truncated 片段同样须 ≤64 KiB（错误文案「截断片段超过 64 KiB」）。

## S37 三锚（全部 `#[cfg(windows)]`，proc_session.rs tests）

1. `try_wait_wait_timeout_branch_reachable`：OpenProcess 自身 SYNCHRONIZE
   句柄（未终止）→try_wait 0ms 落 WAIT_TIMEOUT 臂返回 Ok(None)；锚定分支
   可达＋WAIT_FAILED 导入由目标编译兜底。
2. `windows_reader_eof_accumulates_on_eof`：spawn_reader 读空游标→轮询
   reader_eof==1。
3. `unc_verbatim_restores_two_backslashes`：strip_verbatim_prefix 纯函数锚
   （UNC 2 反斜杠／盘符剥离／普通路径原样）。

## 验证

- `cargo check --tests`：0 错误；警告 **580＝基线零新增**。
- 本机（linux）：proc_session 24/0、context 15/0、session lib 29/0＋集成 40/0、
  request_permissions 38/0、security_attack_031 7/0、exec_command 18/0、
  memory_lifecycle 6/0。
- Windows 锚：**待 038 Windows CI**（continue-on-error 腿）；如实标注未验证。
