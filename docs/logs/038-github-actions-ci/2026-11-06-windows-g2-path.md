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
