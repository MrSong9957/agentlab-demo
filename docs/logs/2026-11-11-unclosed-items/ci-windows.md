# 038 Windows CI 腿验证记录（2026-11-12）

## 触发方式（步骤 1 结论）

`.github/workflows/ci.yml:13-17`：`push` 仅 main、`pull_request` 任意分支、
`workflow_dispatch` 手动。任务分支推送**不**触发 CI；分支触发＝开 PR（
pull_request/synchronize 事件）或 `gh workflow run CI --ref <分支>`。本轮采用
**workflow_dispatch**（授权仅含推送分支，未含开 PR；gh 已认证含 workflow
scope）。远端起始状态：仅 `refs/heads/main`，任务分支不存在（本次首推）。

## Run 1（43127642145，f17ff48，workflow_dispatch 00:29Z）

- ubuntu ✅ success；runner-diagnosis ✅；macOS 进行中（被 run 2 并发组取消）。
- **windows-latest ❌ failure（cargo test 编译期，5 错全在批 3 cfg(windows) 测试面）**：

| 错误 | 位置 | 根因（windows-sys 0.59 registry 源码核实） |
|---|---|---|
| E0432 | proc_session.rs:2431 | `SYNCHRONIZE` 在 `Win32::Storage::FileSystem`（非 Threading）；`FILE_ACCESS_RIGHTS=u32`，feature 已启用 |
| E0308 | proc_session.rs:2434 | `HPCON = isize`（Console/mod.rs:315），`hpc: null_mut()` 类型错 → `hpc: 0`（try_wait 不触 hpc、ChildHandle 无 Drop，安全） |
| E0425 ×3 | proc_session.rs:2477/2481/2485 | S37 锚 3 误挂 proc_session 测试面：`strip_verbatim_prefix` 实为 permissions 私有 fn（`#[cfg(windows)]`）→ 测试归位 permissions tests mod（`super::` 可达；Linux 下空 mod 零告警） |

- lib 生产代码（非测试）零错误——批 3 对 A10 生产面的修复（WAIT_FAILED
  导入等）编译通过。
- 修复提交：`14a32ea`（仍无 Windows 编译环境，API 签名逐一对照本地
  registry windows-sys-0.59.0 源码；`cargo check --tests` 0 错误，警告 580 基线）。

## Run 2（43130590026，14a32ea，workflow_dispatch 00:53Z）

目标：验证 `14a32ea` 修复后 Windows 腿编译＋测试结果。

（结果见本文件末尾追加段）

## 口径说明

- Windows 腿 `continue-on-error`（ci.yml:43）：run 级结论不反映 Windows，
  判定一律看 windows-latest job 步骤级结果。
- 038 规格既有口径：Windows 为产品级移植项（迭代 35 后余 ~21 个已知测试
  失败族，proc_session 容差族等）；若 run 2 出现**非本分支引入**的既有失败
  族，按任务约定归因为产品级存量并如实登记，不谎报为绿。
