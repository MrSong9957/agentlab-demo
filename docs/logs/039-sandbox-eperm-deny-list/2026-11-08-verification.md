# 039 验证记录（2026-11-08）

环境：devcontainer（bwrap 0.9.0、内核 6.18 WSL2、rustc stable、libc 0.2.189 锁定）；仓库分支 `feat/039-sandbox-eperm-deny-list`（基线＝合并后 main `b85c4ee`，内容与候选 `467c9aa` 逐字节一致）。构建与测试均在隔离 target `/tmp/038-attack06-target`（显式 `CARGO_TARGET_DIR`，不污染共享产物）。

## 变更

- `src/app/src/sandbox.rs` `SECCOMP_DENY_EPERM`（27→27 条）：
  - B 类：移除 328（误拒 pwritev2；其注释意图 open_tree 已由 428 承担）；加入 442 mount_setattr（mount 族补齐，原 432 注释意图点名）。
  - A 类：428-432 注释左移纠偏（open_tree/move_mount/fsopen/fsconfig/fsmount）。
- `src/app/src/sandbox/tests.rs`：
  - 新增 `seccomp_deny_lists_match_libc_symbols`（C 类回归锚）：EPERM/ENOSYS 两清单逐条与 `libc::SYS_*` 对照＋双向集合一致＋升序。
  - 扩展 `seccomp_filter_blocks_clone3_and_preserves_openat2`：探针 6 参化；产品过滤器安装后实测 pwritev2≠EPERM、open_tree=EPERM、mount_setattr=EPERM（EPERM 只能来自过滤器——探针参数全零仅产生内核侧错误，无真实副作用）；父进程裁决标记同步扩展。
- 未动（§5 边界合规）：`SECCOMP_DENY_ENOSYS` 数值、clone 掩码块、清单生成算法（ENOSYS 符号对照由锚测试覆盖）。

## 命令与结果（串行）

| 检查 | 命令 | 结果 |
|---|---|---|
| C 类锚＋双层守卫 | `cargo test --lib seccomp -- --nocapture` | 4 passed（含锚 ok；标记行「外层对照：clone3=EINVAL openat2=EINVAL」＋「产品过滤器：clone3=ENOSYS openat2=ENOSYS pwritev2!=EPERM open_tree=EPERM mount_setattr=EPERM」） |
| 锚变异证明 | 注入清单 428→429 漂移后同命令 | FAILED：「EPERM 清单缺少 open_tree（SYS_open_tree=428）——编号漂移或漏登记」；还原后 ok。（另注：仅交换对照表行序属集合等价、不失败——锚保护对象是清单编号漂移，与设计一致。） |
| attack06 实弹 | `cargo test --test security_attack_031 attack06 -- --nocapture` | ok（SH/BASH/CP/GCC 工具链正向；unshare/mount/keyctl/perf_event_open 等特权面 EPERM；PROBE-EXIT=0） |
| lib 全量（分支内最小全量） | `cargo test --lib` | 305 passed / 0 failed / 1 ignored（既有 ignore） |
| 静态检查 | `cargo clippy --all-targets -- -D warnings` | 组件下载失败（rustup 缓存错误）→ 按 AGENTS.md 兜底 `cargo check --all-targets`：Finished，无新增错误；警告为 038 存量基线 |

## 集合变更记录（变更前后）

- 移除：328（实拒 pwritev2——普通写路径误拒，无登记意图）。
- 加入：442 mount_setattr（mount 族最后一块；CLONE_NS_MASK/cap-drop ALL 之下的纵深面）。
- 保持：其余 26 条编号与语义不变；437 openat2 ENOSYS、435 clone3 ENOSYS 行为不变（守卫测试断言）。
