# 规格：031 沙箱 EPERM 拒绝清单纠偏与编号回归锚

- 状态：**已实施待验收（2026-11-08）**。用户已确认按草案执行；B 类定案（依据草案内推荐与最小不变更原则）：328 移除、442 mount_setattr 加入、433 fspick 未纳入（草案未点名其注释意图，不自动扩大拒绝集合；如需纳入为 1 行清单＋对照表增量）。实现与验证证据见 [`2026-11-08-verification.md`](../logs/039-sandbox-eperm-deny-list/2026-11-08-verification.md)。
- 创建：2026-11-08。来源：038 attack06 后续轮独立审查（三审查者交叉命中，见 `/tmp/compound-engineering-1000/ce-code-review/20261108-024223-061dbcf6/report.md` #3 与 clone3 根因同类的编号漂移）。
- 关联：`docs/specs/031-request-permissions.md`（§1.6 要求限制关键系统调用；规格不逐项枚举清单，清单为 M0 冻结的实现选择，注释声称意图）；`docs/specs/038-github-actions-ci.md`（本轮范围登记）。

## 1. 要做什么

以权威系统调用表为基准，把 `src/app/src/sandbox.rs` 的 `SECCOMP_DENY_EPERM`（当前 27 条）修正到"注释＝实际拒绝＝官方名"三方一致，并加一个防同类漂移的编号回归锚。**分两类改动：**

- **A 类（纯注释修正，拒绝集合不变）：** 428-432 注释左移纠正（实际拒绝本就是 open_tree/move_mount/fsopen/fsconfig/fsmount，正确面，只是注释错位一格）。
- **B 类（拒绝集合变更，逐项待用户确认）：** 见 §4 待决清单。实施前不动任何编号。
- **C 类（结构防护，拒绝集合不变）：** 新增"清单条目 ↔ libc `SYS_*` 符号对照"回归锚，使编号漂移在构建期失败。

## 2. 权威依据（2026-11-08 核实）

- 官方 x86-64 UAPI：`/usr/include/x86_64-linux-gnu/asm/unistd_64.h`（本机内核头文件）；上游同构表 [torvalds/linux v6.18 arch/x86/entry/syscalls/syscall_64.tbl](https://github.com/torvalds/linux/blob/v6.18/arch/x86/entry/syscalls/syscall_64.tbl)。
- 锁定依赖：libc 0.2.189（`~/.cargo/registry/src/*/libc-0.2.189/.../x86_64/not_x32.rs`）：`SYS_pwritev2=328`、`SYS_open_tree=428`、`SYS_clone3=435`、`SYS_openat2=437`、`SYS_mount_setattr=442`。
- 核对方法：脚本化抽取清单（编号＋注释）逐条对三方表；全程可复现。

## 3. 事实：当前实际拒绝集合 vs 注释

| 判定 | 条目 | 说明 |
|---|---|---|
| 一致（21 条） | 101 ptrace、155 pivot_root、165 mount、166 umount2、167 swapon、168 swapoff、169 reboot、175 init_module、176 delete_module、246 kexec_load、272 unshare（附 L2 意图注记）、300 fanotify_init、303 name_to_handle_at、304 open_by_handle_at、308 setns、310/311 process_vm_readv/writev、313 finit_module、320 kexec_file_load、321 bpf、323 userfaultfd | 编号＝注释＝官方名 |
| **注释错＋疑似误拒（328）** | 注释 open_tree，官方 **pwritev2** | 实际拒绝了普通写调用 pwritev2；注释点名的 open_tree 已由 428 承担。328 为什么在清单里没有记录的意图 |
| **纯注释错位（428-432）** | 实拒 open_tree/move_mount/fsopen/fsconfig/fsmount；注释整体右移一格（428 标 fsopen … 432 标 mount_setattr） | A 类修正即可 |
| **缺失** | mount_setattr（442）、fspick（433） | 同为 mount 族 API（fsopen/fsconfig/fsmount/move_mount/open_tree 均已在清单）；442 为原注释意图点名（432 注释），433 是否纳入待定 |

## 4. B 类定案（2026-11-08 用户确认按草案执行后落地）

1. **328**：(a) 移除——不再误拒 pwritev2（推荐：注释意图 open_tree 已由 428 覆盖，保留即无理由误拒普通写路径）；或 (b) 保留编号、注释改 pwritev2（拒绝集合不变，但需给出拒绝 pwritev2 的理由）。
2. **442 mount_setattr**：是否加入（补齐 mount 族最后一块；注释意图已点名，`CLONE_NS_MASK`/cap-drop ALL 之下的纵深防御面）。
3. **433 fspick**：是否加入（同族可选）。
4. 回归锚形态：单测内把清单条目逐条与 `libc::SYS_*` 常量对照（任一不符即失败）——无争议部分，随 B 类或单独先行。

> 上述任一 B 类改动都会改变实际拒绝集合＝安全清单行为变更：实施后须重跑 seccomp 单测、U1 守卫与 attack06，并在 039 记录变更前后集合。（已执行：见验证记录——守卫子进程双层过滤器 errno 实测、attack06 全绿、回归锚变异证明。）

## 5. 边界

- 只动 `SECCOMP_DENY_EPERM` 与其注释＋新增对照测试；不动 `SECCOMP_DENY_ENOSYS`、clone 掩码块、清单生成算法。
- 相邻问题不顺手修：发现的其他清单/漂移一律登记。

## 6. 可观察验收

- 清单三方一致（注释＝官方＝libc），`grep` 可复核；A 类修正后 diff 只含注释行。
- 回归锚：人为制造一处编号/符号错配时测试失败（变异证明），恢复后通过。
- 若 B 类获批：变更条目在受控两层过滤器（外层 EINVAL 对照＋真实产品 memfd）下实测新 errno；attack06 全绿（工具链正向＋特权全拒）。
- 全量验证分层记录（同 038 本轮口径）。

## 7. 预测 → 实测（2026-11-08 已回填）

- 428-432 注释修正 → 预期过滤器字节不变（仅注释）→ 实测：**符合**——清单保持 27 条（-328 +442），seccomp_filter_shape_valid／seccomp_fd_roundtrip 通过；注释行不进过滤器。
- 328 处置（移除）→ 预期过滤器减少 2 条 insn、pwritev2 不再被拒 → 实测：**符合**——守卫子进程（外层 EINVAL 对照＋真实产品 memfd 过滤器）探针 pwritev2 实测非 EPERM；条目数 27→27（-1 +1，净 insn 数不变）。
- 新增 442 → 预期过滤器增加 2 条 insn、mount_setattr 实测 EPERM → 实测：**符合**——同一探针 mount_setattr=EPERM、open_tree（428 纠偏后）=EPERM。
- 回归锚变异 → 预期清单错配即失败 → 实测：**符合**——注入 428→429 编号漂移（clone3 类错误）后 `seccomp_deny_lists_match_libc_symbols` FAILED（报文「EPERM 清单缺少 open_tree（SYS_open_tree=428）——编号漂移或漏登记」）；还原后 ok。
