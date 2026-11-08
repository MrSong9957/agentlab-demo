# Windows G2 路径夹具修复（2026-11-06）

## 范围

- 仓库：`/workspace`；分支：`feat/038-github-actions-ci`；修复时 HEAD：`0299f5314cd509e8aeb299550918e88968cca2e4`。
- 用户给定基线：`57d5408`；修复时 HEAD：`0299f53`。
- 只改 `src/app/tests/request_permissions.rs::normalize_rejects_missing_and_accepts_dotdot` 的测试输入构造和诊断。生产代码及 workflow 未改动；保留其余已有差异。
- 本记录描述 G2 本地验证完成时、提交前的候选状态；实际提交、推送和 CI 结果需后续补记。

## 缺陷和改动

原测试以 `f.requestable.join("sub").join("..")` 构造路径。Rust 在 Windows 上对带 verbatim 前缀的基路径调用 `PathBuf::join` 并追加非空路径时，会规范化并移除 `.` 和 `..` 分量。因此测试构造阶段可能先删除 `sub\\..`，没有把 G2 要验证的原始字符串交给权限解析器。

测试现在先 canonicalize 已存在的 fixture 目录，再用平台分隔符通过字符串追加 `sub` 和 `..`。Windows 上额外断言 canonical 路径具有 `\\?\` verbatim 前缀；请求序列化后打印诊断，并断言 `parse_request` 返回的路径字符串与输入完全相同。原有 `normalize_request` 结果断言保留。

运行工具链为 `rustc 1.99.1 (58cf8f9c3 2026-10-13)`。官方标准库文档规定：Windows 下，只有当 `self` 带 verbatim 前缀且追加路径非空时，`PathBuf::join` 才会移除新路径中的 `.` 和 `..`；Windows `canonicalize` 会返回 extended-length 路径。因此测试先 canonicalize 以明确触发该条件，再以字符串追加分量。文档：[PathBuf::join](https://doc.rust-lang.org/stable/std/path/struct.PathBuf.html#method.join)、[std::fs::canonicalize](https://doc.rust-lang.org/stable/std/fs/fn.canonicalize.html)。

最终差异只在 `normalize_rejects_missing_and_accepts_dotdot`：canonicalize fixture、Windows 前缀断言、字符串构造原始路径、JSON 序列化诊断及解析后原样断言。原有 `normalize_request` 结果断言保留。

## 验证

- 命令：
  `cargo test --locked --offline --test request_permissions normalize_rejects_missing_and_accepts_dotdot -- --exact --nocapture --test-threads=1`
- 环境：Linux；`CARGO_TARGET_DIR=/workspace/target`、`CARGO_BUILD_JOBS=1`、`CARGO_NET_OFFLINE=true`；限时 180 秒；监控容器 PID 上限 500。
- 结果：退出码 0；`1 passed, 0 failed, 37 filtered out`；耗时 33.70 秒；进程峰值 260（低于 500）；未超时。
- 实际序列化输入：
  ```json
  {"permissions":{"file_system":{"entries":[{"path":{"type":"path","path":"/tmp/tchat-031-m1-91-fixtures-0/export/sub/.."},"access":"read"}]}}}
  ```
- 解析结果与 `raw_path` 完全相等，因此在 Linux 上证明 `/sub/..` 未在 JSON 构造阶段丢失，也通过了解析器。测试日志：`/workspace/target/038-validation/g2-path-linux-20261106.log`。
- `git diff --check -- src/app/tests/request_permissions.rs` 通过。
- 简化审查和按 `rules/REVIEW.md` 对此测试差异的审查均无发现。

## 未完成验收

本环境是 Linux，未安装 Windows Rust target，也没有 PowerShell 或 Windows runner。因此 Windows 精确用例尚未运行。Linux 证据不能替代 Windows 验收，G2 仍须等 Windows 上确认 verbatim 前缀、原始 `sub\\..` 保留和规范化结果。

取得 hosted Windows 结果需要推送候选并触发现有 PR workflow。当前 workflow 的 `pull_request` job 会运行三平台全量检查，不提供仅运行此单项测试的触发方式。记录生成时，本地分支领先远端 9 个提交且工作区有其他未提交差异；推送会发布这 9 个提交，其他差异须另行提交。该状态为提交前快照；实际提交和 CI 结果另行补记。

## Windows 编译阻断修复准备（2026-11-06；本地未提交）

- 修复基线仍为 HEAD `b98d52a682a0ba84080d0043c7bc88f697460473`；远端未更新。本节记录该 HEAD 上的工作区候选，不包含新的提交或 CI 结果。
- 上述 Windows `cargo check` 的 11 个错误属于同一平台边界问题：`apply_patch.rs` 有 3 处直接引用 Linux 确认结果/函数；`sandbox.rs` 中未门控的确认包装函数又引用了 `AtomicBool`、`SandboxConfirm`、`libc::kill`/`SIGCONT`、`SandboxRelease`、清理函数和确认 helper，共 7 处；`js_orchestration/worker.rs` 另有 1 处调用 Linux 自停入口。相关 `/proc` PID 命名空间与 STOP/CONT 确认机制不是 Windows API。
- 本地差异把确认闸、`/proc` 身份读取、MCP 包装、自停入口、namespace 信号/清理及其调用点和专用测试限定为 `target_os = "linux"`。普通 Unix 的 `killpg(SIGINT)` 路径仍保留，Windows 的整树中断路径未改。Linux 的 worker 身份登记、放行、失败回收和 namespace 清理逻辑没有改动；没有增加空实现或跳过确认的成功分支。工作流未改。

本地验证（工作区包含已有未提交差异；没有运行全量/L3）：

| 检查 | 结果 |
|---|---|
| `cargo check --locked --offline --target-dir /workspace/target` | Linux 编译通过，退出码 0；输出含已有警告。 |
| `TCHAT_SANDBOX=1 cargo test --locked --offline --target-dir /workspace/target --test js_orchestration sandbox_worker_terminate_reaps_payload -- --exact --nocapture` | 通过：1 passed，0 failed，40 filtered；测试内隔离子进程同样通过。外层限时 60 秒。 |
| `TCHAT_SANDBOX=1 cargo test --locked --offline --target-dir /workspace/target --test proc_session pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process -- --exact --nocapture` | 真实沙箱通过：1 passed，0 failed，27 filtered；18.00 秒。外层限时 60 秒。 |
| Windows 目标编译 | 未验证：rustup 只安装 `x86_64-unknown-linux-gnu`，本机没有 Windows runner。不能据 Linux 编译宣称 Windows 修复通过。 |

结论：此候选在 Linux 编译及两个直接相关的真实沙箱回归上通过；Windows G2 和 Windows 编译仍待 Windows runner 对新提交实际执行。macOS `pidfd` 编译错误及 Ubuntu 既有测试失败不在本次修复范围，未修改或重跑。所有工作区差异保持未提交；本轮未推送、触发 CI 或合并。

## 11 个 Windows 编译错误逐项映射及当前核对（2026-11-06）

Windows job `127759627392`（run `42177622923`，head `b98d52a682a0ba84080d0043c7bc88f697460473`）的 `cargo check` 报告 11 个错误；测试步骤因编译失败而跳过：

| 序号 | 编译位置 | 缺少的名称 | 候选中的平台边界 |
|---|---|---|---|
| 1–2 | `src/app/src/apply_patch.rs`，两处 `SandboxConfirm` 分支 | `SandboxConfirm` | 将 Linux 确认结果匹配限定为 `target_os = "linux"`。 |
| 3 | `src/app/src/apply_patch.rs`，内部载荷入口 | `internal_payload_self_stop_if_gated` | 仅 Linux 调用自停入口。 |
| 4–10 | `src/app/src/sandbox.rs`，确认包装器和放行闭包 | `AtomicBool`、`SandboxConfirm`、`libc::kill`、`libc::SIGCONT`、`kill_sandbox_group_and_wait`、`SandboxRelease`、`confirm_sandbox_with_release` | `/proc`、PID namespace 与 SIGSTOP/SIGCONT 确认机制整体限定为 Linux；不添加 Windows 空实现。 |
| 11 | `src/app/src/js_orchestration/worker.rs` | `internal_payload_self_stop_if_gated` | 仅 Linux 调用。 |

候选同时将 `js_orchestration/runtime.rs`、`mcp/transport.rs`、`proc_session.rs` 中依赖 Linux 确认 API 的调用点限定为 Linux，并将 Linux 专用确认/namespace 测试限制到 Linux。MacOS 通用 Unix 的 PTY、信号与 fd 路径仍保留在 `cfg(unix)` 下。没有新增依赖；这些名称原本就属于 Linux 专用 API。当前工作区 `cargo check --locked --offline --target-dir /workspace/target` 通过，`git diff --check` 通过；本机只安装 `x86_64-unknown-linux-gnu` target，所以不能把 Linux 检查表述为 Windows 编译通过。

## 提交候选边界

- G2 测试修改与本记录已在远端分支的 `b98d52a682a0ba84080d0043c7bc88f697460473` 中；该 SHA 的 Windows job 没有进入测试步骤，因此 G2 仍未验证。
- 要让新的 Windows job 越过这 11 个编译错误，提交必须包含上表对应的 `target_os = "linux"` 条件及 `SandboxConfirm` 结构改动的兼容调用点。`proc_session.rs`、`sandbox.rs`、`mcp/transport.rs` 等文件还含 038 其他未提交修改，不能按整文件默认纳入；只暂存本节对应的调用点、类型/平台门控及 Linux 编译所需 API 配套差异。
- Windows CI 会先运行 `cargo check --locked`，通过后再运行 `cargo test --workspace --no-fail-fast --locked`。只有读取 Windows 测试日志并确认 `normalize_rejects_missing_and_accepts_dotdot` 实际执行、通过，才能关闭该提交快照的 G2。
- 本次没有暂存、提交、推送或触发 CI；不纳入其他工作区差异。
