# clone3 ENOSYS 修复——简化、独立审查与最终验收记录

- 日期：2026-11-08。分支 `debug/038-attack06-investigation`，基线 `bf44b3a92196cedaa341a9e30c79b220991d032a`（工作区未提交差异）。
- 前置记录：根因与受控对照见同目录 `findings.md`；安全测试报告见 `docs/security/reports/2026-11-08-038-clone3-security.md`。本文件记录其后的简化检查、独立代码审查、最终聚焦验证与交付清单，不修改上述既有文件。

## 变更范围

- `src/app/src/sandbox.rs:1215`：`SECCOMP_DENY_ENOSYS` 由 `&[437]` 改为 `&[libc::SYS_clone3 as u32, 437]`，并补注释说明保留 437（openat2）既有行为。
- `src/app/src/sandbox/tests.rs:860-927`：新增受控回归 `seccomp_filter_blocks_clone3_and_preserves_openat2`——父进程自孵化子进程（10 秒截止），子进程先装载仅含 clone3/openat2→EINVAL 的对照过滤器（验证外层生效），再从 `build_seccomp_fd()` 的产品 memfd 读取真实过滤器字节装载，断言两项均为 ENOSYS。
- `docs/specs/038-github-actions-ci.md` §5 第 6 条：追加日期化的根因确证与当前 runner 状态（原记录保留）。
- 本目录新增 4 份验证日志（见下）与安全报告（既有，本轮未改）。

## 简化检查（ce-simplify-code）

5 位审查视角（复用、质量、效率、测试价值、注释）并行只读审查新增代码，**未改动任何代码**。裁决摘要：

- 测试内裸十六进制字面量换用产品常量：跳过——外层对照须是独立预言机，用产品常量构造对照会在产品常量出错时自证；且该测试为本轮证明测试。
- 生产清单 `437` 换 `libc::SYS_openat2 as u32`＋删注释：跳过——437 是既有内容非本轮改动（精准修改原则），EPERM 清单既有惯例即裸数字＋注释。
- `build_seccomp_filter()` 重复调用绑定一次：跳过——可忽略开销，与相邻 `seccomp_fd_roundtrip` 既有惯用法一致。
- 子进程 `--exact` 名称失配空通过隐患：记为审查发现（见下），本轮不改。
- 删除测试内行内注释：跳过——均在证明测试内部；其中两条通过注释保留清单。

## 独立代码审查（ce-code-review，rules/REVIEW.md）

5 名独立审查者（correctness、security、testing、project-standards、adversarial；不继承实现上下文、只读、不运行构建）＋独立验证者。完整报告与工件：`/tmp/compound-engineering-1000/ce-code-review/20261108-024223-061dbcf6/report.md`（临时目录，报告全文亦见本轮交付汇报）。判定：**Ready with fixes**——阻塞定义（规格验收失败或破坏数据、安全、兼容性）无候选命中，无 P0。

已确认的非阻塞发现（本轮按授权范围未自动修复，逐条列给用户决定）：

1. **P1（tests.rs:885）**：父进程仅以退出码裁决子进程；若测试改名致 `--exact` 字符串失配，libtest 零匹配仍退出 0，守卫静默空转。修复一行：父进程补断言子进程 stdout 含 `1 passed`（或 stderr 含最终标记）。守卫当前与名称一致、工作正常，属潜在维护隐患。
2. **P3（ci.yml:134）**：CI D1 手工诊断副本的 ENOSYS 段缺 435 规则、探针把 `syscall(437)` 误标为 clone3——产品过滤器加 435 后副本（528 字节）与产品（544 字节）不再"字节一致"，今后若 435 规则回退，D1 会给出误导性"clone3 正常"取证。ci.yml 属本轮禁改 workflow，须用户处理。
3. **存量独立任务（sandbox.rs:1203-1208，三审查者交叉命中）**：`SECCOMP_DENY_EPERM` 清单编号/注释错位——328 实为 pwritev2（被误拒）、428-432 注释整体偏移一格、mount_setattr(442) 无条目拒绝。与 clone3 错配同类同源，按 rules/REVIEW.md 记独立任务，不阻塞本轮；拒绝集合变化须用户先确认。

## 最终聚焦验证（串行、500 任务门禁、隔离 target-dir `/tmp/038-attack06-target`）

| # | 检查 | 命令要点 | 结果 | 门禁峰值 |
|---|---|---|---|---|
| 1 | 编译检查 | `cargo check --workspace --all-targets --locked`（clippy 组件不可用且补装失败，按 AGENTS.md 兜底 check） | exit 0（警告均为 tchat-tui 等存量） | 317 |
| 2 | 最终树红 | 临时将清单回退为 `&[437]` → 跑聚焦测试 | exit 101：子进程 panic 于 `tests.rs:925`（`产品过滤器应拒绝 clone3: left: 22, right: 38`），父进程 panic 于 `tests.rs:885` | 254 |
| 3 | 恢复校验 | 从备份恢复 `sandbox.rs` | md5 `f2b714646bd52084b9de7ef2fb42a583` 与轮前一致；`git diff` 与轮前快照字节级一致 | — |
| 4 | 最终树绿 | 同命令 | exit 0，`外层对照：clone3=EINVAL openat2=EINVAL`＋`产品过滤器：clone3=ENOSYS openat2=ENOSYS`，1 passed | 256 |
| 5 | attack06（真实 bwrap） | `cargo test -p terminal-chat --test security_attack_031 attack06_seccomp_allows_toolchain_blocks_privileged_syscalls -- --exact --nocapture` | exit 0：SH-OK、BASH-OK、CP-OK、GCC-OK（真实工具链输出，非 bwrap/gcc 缺失的提前返回路径）、`clone3=Function not implemented`、特权系全 `Operation not permitted`、PROBE-EXIT=0，1 passed | 277 |

入库证据（本目录，`*.trim.log` 为从完整 cargo 输出中截取的测试相关段；完整日志在 `/tmp/038-attack06-validation/`，为临时文件）：`check-final.trim.log`、`red-final-tree.trim.log`、`green-final-tree.trim.log`、`attack06-final-tree.trim.log`。红绿重跑（#2–#4）在最终测试代码上补齐了先行会话红日志（其 panic 行号对应测试较早版本）的溯源缺口。

## 已知边界与未验证项

- **目标 runner 未验证**：所有验证在本地 Linux 6.18.33.2 WSL2、x86-64、bwrap 0.9.0。目标 GitHub runner（Ubuntu 26.04.1）产品链未复验；CI 探针仍以 `TCHAT_SKIP_SECCOMP_ENFORCEMENT_PROBE=1` 跳过（本轮范围禁改 workflow/环境）。不能宣称远端 CI 通过。关闭规格 §5 第 6 条需按安全报告在 runner 捕获 `prctl` 返回与过滤器 fd 内容。
- **未跑全量/L3**：按用户范围约定，本轮仅聚焦验证；全量验证（clippy＋全测试集）留待完整路径。
- **共享构建目录污染（一次性）**：红跑首次命令把 `--target-dir` 误置于 `--` 之后，cargo 回退全局 `CARGO_TARGET_DIR=/workspace/target` 构建了一次（仅构建缓存，主工作区源码与 git 状态不受影响）；已纠正并重跑。
- 既有日志中红/绿/attack06 的 `/tmp` 原始日志可能随环境清理消失，以上入库裁剪件为存档版本。

## 交付清单（可批准提交建议，均未提交、未推送）

1. `fix(038): deny x86-64 clone3 via ENOSYS in 031 sandbox filter`
   - `src/app/src/sandbox.rs`（ENOSYS 清单＋注释）、`src/app/src/sandbox/tests.rs`（受控回归）。
2. `docs(038): attack06 clone3 investigation, security report and acceptance`
   - `docs/logs/038-github-actions-ci/2026-11-08-attack06/`（findings.md、control.py、probe.c、baseline-control.log、clone3-control.log＋本轮 4 份日志与本记录）、`docs/security/reports/2026-11-08-038-clone3-security.md`、`docs/specs/038-github-actions-ci.md`（§5 第 6 条根因确证补注）。

补丁快照：`/tmp/038-attack06-validation/diff-after-red.patch`（= 最终 tracked diff；untracked 文件以上列为准）。
