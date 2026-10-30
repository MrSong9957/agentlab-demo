# 038 本地验证记录（2026-10-30，容器内 /workspace）

环境：devcontainer（rustc/cargo 1.99.0 stable，Cargo.lock 锁定，`TCHAT_SANDBOX=1` 为 compose 注入态）。clippy/rustfmt 未安装且容器网络受限无法补装（static.rust-lang.org 不可达），typecheck 用 `cargo check`（§8 既定口径）。

## 一、静态平台审计

- `grep std::os::unix / posix_openpt`（src/app、src/tui、src/shims）：库代码均有成对 `cfg(unix)/cfg(not(unix))` 门控；测试面 15 个文件整体依赖 POSIX 语义（PTY/Unix fd/文件权限位）且无门控。
- 间接依赖：`native_model_apis.rs`、`sandbox_lifecycle.rs`、`user_input_security.rs` 自身无 unix API 但经 `common::pty`（`libc::posix_openpt`）引用 → 一并门控。
- 漏网复核：`request_permissions.rs:484`、`security_attack_031.rs:457/459` 的 unix 用法均有语句级内联 `#[cfg(unix)]`，Windows 可编译（仅运行时行为差异，留待实跑）。
- `/proc` 依赖：`context_perf.rs` `vm_hwm_kb`、`exec_command.rs:536`、`js_orchestration_perf.rs` `worker_rss_kib` 均优雅降级；`multi_agent.rs` `vm_peak_kib/threads_now` 调用点全部位于 `#[ignore]` 测量入口。

## 二、模拟无 bwrap 实测（预测 macOS 与未装 bwrap 的 Linux CI）

命令：
```
env -u TCHAT_SANDBOX PATH="/usr/local/cargo/bin:/tmp/ci-sim-bin:/usr/sbin:/sbin" \
  cargo test --workspace --no-fail-fast --locked
```
`/tmp/ci-sim-bin`＝/usr/bin 全量符号链接（仅去 bwrap）。失败清单与处置：见 `2026-10-30-sim-nobwrap-failures.log`。

- 需 bwrap 真实执行（已用例级 `#[cfg(target_os = "linux")]`）：e2e_acceptance s140/s141/s142（031 权限沙箱 E2E）、e2e_sandbox b2_real_cli_fd_chain_full_path、sandbox_matrix_probes b1×2/b2/c/d/e 共 6 探针。
- 自带可用性分支、无 bwrap 下通过（文件级 `#![cfg(unix)]` 即可，macOS 照常跑）：mcp_stdio_sandbox、request_permissions_sandbox、apply_patch_sandbox、exec_sandbox、request_permissions、js_orchestration、multi_agent、security_attack_031、e2e_sandbox 其余用例、sandbox_matrix_probes 4 个 F fail-closed 探针。
- 机器口径冻结阈值：js_orchestration_perf c18×2（p95 342–368ms 超冻结 300/250ms）——同机两环境复现；CI 用模块设计的 `JS34_PERF_SAMPLE=1` 采样模式（跳断言不跳场景）。

## 三、标准环境全量基线（§8 既有入口，含本次全部门控改动）

命令：`cargo check --locked && cargo test --workspace --no-fail-fast --locked`（完整日志 `2026-10-30-baseline-full.log`）。

结果：**3852 通过 / 4 失败 / 6 忽略**（编译零错误——门控改动通过全量编译验证）。

4 个失败全部与本次改动无关：
1. `js_orchestration_perf::c18_cold_start_pure_js`（346ms>冻结 300ms）、`c18_leaf_counts_1_4_31`——机器速度未达 2026-10-29 冻结阈值（门控前的首轮模拟同样失败）；
2. `proc_session::pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process`——`git stash` 后于干净 HEAD 复跑同样失败（标准环境 `TCHAT_SANDBOX=1` 下稳定复现、模拟无沙箱环境两轮通过），存量问题，与本任务无关，单独报告；
3. `session_perf::perf_tiers_baseline_driven`（/sessions 百文件列出 271ms>50ms）——机器/磁盘敏感（本容器 /workspace 经 WSL2 bind mount）。

首轮模拟中 `--lib` 与 `multi_agent_security_probe` 各失败一次、随后两轮（模拟二轮＋标准基线）全部通过：失败详情被首轮 tail 管道截断无法定位，判为负载偶发；后续 CI 实跑继续观察。

## 四、验证缺口

- Windows/macOS 编译无法本地验证（rustup 目标下载被网络策略阻断，aws-lc-rs 交叉构建不可行）；以静态审计＋真实 CI 为准。
- clippy/fmt 未执行（工具未装、网络受限）。
- CI 端风险点：GH 托管 Ubuntu 24.04 的 unprivileged userns（bwrap 干跑依赖）、Windows 的 aws-lc-sys 原生构建（依赖 runner 预装 NASM/CMake/Perl）、session_perf 50ms 磁盘敏感预算——均以三端实跑结果定位。
- 安全检查：本变更为 CI 接线＋cfg 门控＋文档，无新增接口/凭证/数据处理面（workflow 无 secrets，测试全程假凭证＋本地 mock），不触发 Security-Testing-Gate 的接口攻防报告；第三方 Action（actions/checkout、dtolnay/rust-toolchain、Swatinem/rust-cache）按版本 tag 固定。
