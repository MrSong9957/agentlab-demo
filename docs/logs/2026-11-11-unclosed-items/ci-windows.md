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

- ubuntu ✅ success（三连绿）；runner-diagnosis ✅；macOS ❌ 59 失败（038 §5
  已登记的 macOS 产品级族：attack031/e2e s 系/seam 注入/PTY 等）。
- **windows-latest ❌ failure：编译全过，测试期 29 失败/10 套件**。
- **归因＝与 main 基线逐项对照**（merge-base 7c65624 的 run 42725951053，
  2026-11-10，Windows 腿 30 失败）：
  - **27/29 与 main 基线逐项一致**（同测试同文件，仅行号漂移）＝038 迭代
    32/35 已登记的存量产品级族：proc_session 容差族 15（回收超窗 3s/超时
    不生效/ConPTY 0xC0000142/输出空）、exec_command 超时组杀族 5、c15/t18
    （迭代 35 明确「取证中」）、apply_patch internal-apply、mcp_tls、
    mcp_turn a17、memory 文件名/slug 族 2；
  - **main 独有 2 项（a10_mixed、pipe_ctrl_c）本分支通过**（改善，非劣化）；
  - **本分支独有仅 1 项**：`sandbox::tests::absolute_lexical_unix_dedot_
    keeps_root_and_relative_shape`——A4/S27 本分支新增锚无门控，POSIX 断言
    在 Windows 路径语义不成立（`/`＝当前盘根）。macOS 同锚 ok（POSIX 一致）。
  - **S37 三锚＋A10 reap 锚全部 `ok`**（run 2 日志实证）——批 3 暴露面
    Windows 验证通过。
- 修复提交：`503507e`（unix 形态锚补 `#[cfg(unix)]` 门控，038 用例级门控
  惯例；Windows 形态由既有 cfg(windows) cross_drive 锚覆盖；Linux 门控恒
  真，lib 单测复跑 ok）。

## Run 3（43134862237，503507e，workflow_dispatch 01:31Z，最终裁决）

- ubuntu ✅ success（四连绿）——**A7 写线程锚（404198b）CI 首验通过**。
- **windows-latest：编译全过；测试期 28 失败；branch-only 回归＝0**——
  与 main 基线（run 42725951053，30 项）逐项对照：本分支**改善 2 项**
  （a10_mixed、pipe_ctrl_c 在 main 失败、本分支通过），其余 28 项与 main
  逐项一致（038 迭代 32/35 已登记存量产品级族：proc_session 容差族 15、
  exec_command 超时组杀族 5、c15/t18、apply_patch internal-apply、
  mcp_tls、mcp_turn a17、memory 文件名族 2、sandbox G2 verbatim 族 2）。
  run 2 的唯一本分支失败（unix_dedot 锚）已消失＝`503507e` 修复生效。
- **S37 三锚＋A10 reap 锚第二轮连绿**（try_wait_timeout/reader_eof/
  unc_verbatim/reap_extra_clears 全部 ok）。
- macOS ❌：job 提前终止于 e2e_acceptance PTY 卡点（038 已登记「macOS
  e2e PTY 卡死」产品级缺口同族）；记录到的 11 失败（seam 注入族×6、
  e2e PTY 族×5）全部为 038 §5 已登记 macOS 产品级族，与本分支改动无关。

## 最终结论

| 验收口径 | 结果 |
|---|---|
| 本分支引入面（A10 五处/S37 三锚/A4S27 锚）Windows 编译＋运行 | **全部通过**（编译 0 错；4 个 Windows 锚＋reap 锚二连绿） |
| branch-only 回归（对照 main 基线逐项） | **0**（且改善 2 项） |
| ubuntu 腿 | 四连绿（含 A7 锚 CI 首验） |
| Windows 腿绝对全绿 | **未达成**——28 项存量＝038 规格登记的 Windows 产品级移植项（016 组杀/ConPTY/容差族），超出 CI 配置范畴，continue-on-error 的设计原因；非本分支可修 |

阻断条件复核：「修复后 CI 连续两次红在同一非本分支原因」未触发——run 2→3
之间失败集合单调收敛（29→28，本分支失败归零），红的成分全部为在案存量族。

## 口径说明

- Windows 腿 `continue-on-error`（ci.yml:43）：run 级结论不反映 Windows，
  判定一律看 windows-latest job 步骤级结果。
- 038 规格既有口径：Windows/macOS 为产品级移植项（迭代 35 后余 21 个已知
  失败族；本分支对照实测 main 基线 30 族）；存量失败按规格如实登记，
  不谎报为绿——「Windows CI 腿绿」在本轮语境的可达口径＝**本分支引入面
  （A10 五处/S37 三锚/A4S27 锚）全部编译＋运行通过，失败集合⊆main 基线**。
