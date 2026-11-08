# 038 clone3 seccomp 安全测试报告

日期：2026-11-08。审查基线：`bf44b3a92196cedaa341a9e30c79b220991d032a`。范围：Linux x86-64 的 031 动态权限沙箱中，clone3 的 ENOSYS 拒绝、既有 437（openat2）行为以及原 attack06。审查对象是本次工作区差异；本报告不评估整个后端或全部沙箱规则。

## 结论

基线存在一项已证实的纵深防御缺口：产品过滤器没有按既定要求拒绝 clone3。本次修复后的真实产品过滤器在受控进程中返回 ENOSYS；437 的 ENOSYS 行为保持不变。原 attack06 在本地真实 bwrap 沙箱中通过，正常工具链及其断言均有输出。**本范围内本地检查通过；目标 GitHub runner 的产品链尚未复验，不能据此宣布该环境已通过。** 没有证实实际沙箱逃逸。

## 问题清单

### 基线：clone3 未命中预期的 ENOSYS 规则

- **风险：中。** 产品 seccomp 未对 clone3 执行既定拒绝，攻击者可尝试利用该系统调用绕过仅针对 clone(2) 参数的命名空间限制。这是纵深防御缺口；能否建立新的命名空间或逃逸还取决于外层 seccomp、capability 和其他隔离，现有证据没有证明实际逃逸。
- **原因：** 基线的 `SECCOMP_DENY_ENOSYS` 只有 437，而 Linux x86-64 的 clone3 是 435，437 是 openat2；未命中清单的 clone3 最终进入 ALLOW 路径。见 [过滤器实现](../../../src/app/src/sandbox.rs) 和 [调查记录](../../logs/038-github-actions-ci/2026-11-08-attack06/findings.md)。
- **复现：** 在受控子进程先为 435、437 添加返回 EINVAL 的外层过滤器，再通过无效参数调用 `syscall(SYS_clone3, 0, 0, 0, 0)` 和 `syscall(SYS_openat2, 0, 0, 0, 0)`。安装基线产品过滤器后，clone3 的 libc 返回 `-1`、errno 为 22（EINVAL），没有变为预期的 38（ENOSYS）；回归测试因此失败，退出码 101。openat2 的 ENOSYS 断言先于 clone3 断言，未触发失败。另一次 bwrap 受控对照使用基线源码重建的过滤器，clone3 得到原始返回 `-22`、libc `-1`、errno 22，openat2 的 libc 返回 `-1`、errno 38。对照源码、日志和重现方法在 [调查记录](../../logs/038-github-actions-ci/2026-11-08-attack06/findings.md) 中。
- **状态：本次本地修复并验证。** 产品清单添加 `libc::SYS_clone3`，保留 437；新测试从产品 memfd 读取并安装真实生成的过滤器。先只安装对照过滤器，两项均为 EINVAL；再安装产品过滤器，两项均为 ENOSYS。该单项测试 1 通过、0 失败、0 忽略，退出码 0。外层对照避免将本机已有的 clone3 ENOSYS 错认成产品效果。复现测试为 `cargo test -p terminal-chat --lib sandbox::tests::seccomp_filter_blocks_clone3_and_preserves_openat2 -- --exact --nocapture`；运行时应沿用本轮串行、限时和 500 任务门禁，并为隔离工作区指定独立 `--target-dir`。红、绿记录分别为 `/tmp/038-attack06-validation/red.log` 和 `/tmp/038-attack06-validation/green-final.log`，为临时证据，可能随环境清理消失。

## 原 attack06 实测

[原测试](../../../src/app/tests/security_attack_031.rs) 未修改。它通过 `permission_launch_argv` 启动真实 bwrap，在载荷内编译并执行 C 探针，调用 clone3 和多个特权系统调用。本地输出 `SH-OK`、`BASH-OK`、`CP-OK`、`GCC-OK`、`clone3=Function not implemented` 和 `PROBE-EXIT=0`；mount、umount2、pivot_root、bpf、ptrace、setns、open_by_handle_at、process_vm_readv、unshare 等显示 `Operation not permitted`。测试结果为 1 通过、0 失败、0 忽略，6 项过滤，退出码 0；日志位于 `/tmp/038-attack06-validation/attack06.log`。复现入口为 `cargo test -p terminal-chat --test security_attack_031 attack06_seccomp_allows_toolchain_blocks_privileged_syscalls -- --exact --nocapture`，仍需应用上述串行和资源门禁。

本机原有外层 seccomp 已会令 clone3 返回 ENOSYS，所以 attack06 的输出单独不能证明该 errno 来自产品过滤器；产品过滤器的归因由上面的受控外层对照提供。其他系统调用的 EPERM 同样不能单凭 errno 归因于产品过滤器。attack06 中 bwrap 或 gcc 不可用时会提前返回；本次实际出现工具链与探针输出，故不是该提前返回路径。

## 测试限制和剩余风险

- 本地为 Linux 6.18.33.2 WSL2、x86-64、bubblewrap 0.9.0。原目标 CI 为 Ubuntu 26.04.1；未在该 runner 复验修复，也未捕获目标 runner 的实际过滤器 fd、`prctl(PR_SET_SECCOMP)` 返回和 clone3 返回值。CI 历史的 `clone3=Invalid argument` 无法单独证明过滤器是否加载。
- 新单项测试在受控子进程直接装载真实产品 memfd；原 attack06 覆盖本地 bwrap 启动链。它们不能替代目标 runner 上的完整链路验证，也不证明所有输入和所有沙箱逃逸路径均安全。
- 相邻旧有 EPERM 清单存在编号和注释不一致，例如 428 的注释与系统调用名称不符；该清单不在本轮审计或修改范围，不能视为已修复或已全面验证。
- 此次不涉及订单、账户、管理员接口、表单和认证服务；数据越权、功能越权、身份伪造、字段提权、SQL／脚本注入、前端校验绕过、敏感信息泄露及接口防刷不在本次检查范围，未计为通过。没有执行超大并发、全量测试或 L3。
- 既有受控对照和本轮单项记录均为串行、带超时及 500 任务门禁；受控测试使用无效参数，未进行宿主级特权操作。受控过滤器仅约束测试进程及后代。
