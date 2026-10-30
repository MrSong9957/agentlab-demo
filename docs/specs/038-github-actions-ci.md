# 规格：一键全量测试接入 GitHub Actions（Windows/Linux/macOS 三平台）

- 状态：实施中
- 更新时间：2026-10-30

## 1. 要做什么，有哪些要求

把仓库既有的一键全量验证入口（AGENTS.md §8：`cargo check && cargo test --workspace --no-fail-fast`）接入 GitHub Actions，在 Windows、Linux、macOS 三个托管平台运行，推送代码或提 PR 时自动触发。

- R1. 复用现有测试入口命令，不另造测试入口；按项目规则 §6 补 `--locked`（锁定 Cargo.lock，不改变测试集合，只保证三端依赖一致）。
- R2. 触发条件：push（任意分支）或 pull_request；同一引用的重复触发取消旧运行（concurrency）。
- R3. 三平台矩阵并行，一端失败不取消其余平台（`fail-fast: false`），三端结果完整可见。
- R4. 平台差异按最小修改补齐：只加编译/平台门控（cfg），不删减、不改写任何测试断言；Linux 上测试集合与门控前完全一致。
- R5. 无法在某平台运行的项，逐项说明原因并记录于本规格与验证日志；不静默跳过。
- R6. 工具链与缓存复用成熟开源 Action：`dtolnay/rust-toolchain@stable`（工具链）、`Swatinem/rust-cache@v2`（构建缓存）。
- R7. 不把任何真实凭证写入 workflow 或仓库；全部测试本就用假凭证＋本地 mock。

## 2. 哪些情况容易出错

- Windows 检出 CRLF 污染文本断言 → workflow 显式 `git config --global core.autocrlf false`（配合仓库 `.gitattributes` 的 `* text=auto`）。
- GitHub 托管 Ubuntu 镜像未预装 bubblewrap → Linux 任务显式 `apt-get install bubblewrap`，否则沙箱族测试（031/033）全部 fail-closed 失败。
- PTY 测试经 `libc::posix_openpt` 实现（POSIX 语义），Windows 无法编译 → 文件级 `#![cfg(unix)]` 门控，Windows 不编译这些测试目标（非删除）。
- bubblewrap 为 Linux 专属技术（user namespaces），macOS/Windows 无沙箱执行面 → 沙箱专属测试文件门控 `#![cfg(target_os = "linux")]`。
- Windows/macOS 上依赖 `/proc`、`python3`、`/bin/sh` 等平台设施的用例 → 以三端实跑结果为准逐项定位，不凭猜测预门控。
- 首次在 Windows/macOS 编译整个 workspace → 库代码已有完整 cfg 门控（`sandbox.rs`、`catalog.rs` 等），但测试面从未验证，实跑可能暴露新问题。

## 3. 重要做法，为什么这样做

- 门控加在**文件级**（`#![cfg(unix)]` / `#![cfg(target_os = "linux")]`）而非逐用例拆分：这些测试文件整体依赖 POSIX 进程/PTY/fd/文件权限语义，逐用例拆分改动大、易漏；文件级门控在 Linux/macOS 上条件恒真，测试集合零变化。
- `common/mod.rs` 的 `pub mod pty;` 加 `#[cfg(unix)]`：该声明被所有集成测试共享，不门控则 Windows 上全部测试目标编译失败。
- CI 不预置 `TCHAT_SANDBOX`：e2e_sandbox 等文件按「继承部署注入态」双分支设计，CI 环境走普通模式分支；沙箱专属测试自行按场景置 1。这保持 CI 与容器部署两种形态都被覆盖，且避免 GH runner userns 受限时全量误报。
- Linux 安装 bubblewrap 而非跳过沙箱测试：容器内基线（compose 注入 `TCHAT_SANDBOX=1`）已验证沙箱全链路，CI 保持同等覆盖。

## 4. 踩过的坑，怎样解决

- 本容器网络受限（static.rust-lang.org 不可达），无法 `rustup target add` 做 Windows/macOS 交叉编译检查 → 改用三层替代验证：①静态审查全部 `std::os::unix`/`PtyCli` 引用与 cfg 门控分布；②本地以「影子 PATH 去掉 bwrap＋清空 `TCHAT_SANDBOX`」实测模拟 macOS/未装 bwrap 环境行为；③以三端真实 CI 运行为最终依据。
- 多个测试文件仅凭「是否出现 `std::os::unix`」筛查会漏：`native_model_apis.rs`、`sandbox_lifecycle.rs`、`user_input_security.rs` 经共享模块间接依赖 `common::pty`，自身无 unix API 却无法在 Windows 编译 → 按 `PtyCli` 引用链补充门控。

## 5. 先预测，再实测，最后确认是否合格

| 场景 | 预计结果 | 实测 |
|---|---|---|
| Linux 基线不受门控影响 | 门控前后全量结果一致 | 通过：`cargo check`＋全量 3852 通过/4 失败/6 忽略，4 个失败均为存量（干净 HEAD 复现）或机器敏感项，与门控无关——见 `docs/logs/038-github-actions-ci/2026-10-30-local-verification.md` |
| 无 bwrap 环境（模拟 macOS/未装 bwrap 的 CI） | 沙箱专属用例失败、其余通过 | 通过：4 目标 12 用例失败，全部可归类为「需 bwrap 真实执行」或「机器口径冻结阈值」；详见 `docs/logs/038-github-actions-ci/2026-10-30-sim-nobwrap-failures.log` |
| Windows 编译 | 16 个 unix 门控文件＋pty 模块不编译，其余全部编译 | 待填（CI） |
| macOS 运行 | bwrap 用例门控后，普通模式用例通过 | 待填（CI） |
| push / PR 自动触发 | 三平台矩阵各一次运行 | 待填（CI 链接） |

证据存 `docs/logs/038-github-actions-ci/`；三端实跑结论引用 Actions 运行记录。
