# attack06：clone3 过滤器编号调查

- 调查基线：`bf44b3a92196cedaa341a9e30c79b220991d032a`；隔离分支 `debug/038-attack06-investigation`。
- CI：2026-11-08 的 [Ubuntu 作业](https://github.com/MrSong9957/agentlab-demo/actions/runs/42433446020/job/128587131094)，Ubuntu 26.04.1。CI 日志显示 `SH-OK`、`BASH-OK`、`CP-OK`、`GCC-OK`、`PROBE-EXIT=0`，以及 `clone3=Invalid argument`。attack06 断言 ENOSYS，结果失败。
- 源码 `security_attack_031.rs:126-138,845-859,880`：测试先通过 `permission_launch_argv` 构造 bwrap 启动，再在其载荷中运行探针。调用为 `syscall(SYS_clone3, 0, 0, 0)`；clone3 的有效参数是前两个，即 `(NULL, 0)`。打印错误文本的分支要求 libc 返回 `-1`；`Invalid argument` 对应 EINVAL（22）。CI 未输出数值 errno 或内核原始返回寄存器，不能把它们写成直接测得的值。

## 根因与实际过滤

`src/app/src/sandbox.rs:1211-1214` 声称 clone3 应返回 ENOSYS，但 ENOSYS 清单写的是 `437`。Linux x86-64 官方系统调用表明确规定 [435 是 clone3，437 是 openat2](https://github.com/torvalds/linux/blob/v6.18/arch/x86/entry/syscalls/syscall_64.tbl)。`build_seccomp_filter` 对清单编号生成 ERRNO/ENOSYS，其余未命中且不是 clone(2) 的调用走 ALLOW。因此该程序没有针对 435 的 ENOSYS 规则，却对 437 应用了 ENOSYS；这与 attack06 的既定目标直接冲突。

同一文件的 EPERM 清单还有其他注释与编号不一致的事实，例如 428 标为 fsopen，而官方表中 428 为 open_tree。本轮没有审计或修改该清单；它不作为 attack06 修复范围。

`permission_launch_argv` 构造 memfd 并将其作为 `--seccomp` 参数交给 bwrap（`sandbox.rs:797-802,1283-1308`）。本轮基于源码重建该过滤器并实测其行为；本次 CI 没有记录完整过滤器字节、prctl 调用或原始返回值，因此不能单凭 EINVAL 认定该 runner 是否完成加载，也不能排除同一 runner 还有其他启动问题。无论加载与否，现有字节都不包含针对 435 的预期拒绝规则。

内核 [seccomp 文档](https://www.kernel.org/doc/html/latest/userspace-api/seccomp_filter.html)规定 ERRNO 动作不执行系统调用；多层过滤器按动作优先级和安装次序决定结果。`clone3(NULL, 0)` 的参数无效，返回错误不能单独证明产品规则拦截。Linux [clone3 自测](https://github.com/torvalds/linux/blob/v6.18/tools/testing/selftests/clone3/clone3.c)也检查了小于最小结构大小的参数失败。历史 runner 诊断直接调用 `syscall(437, NULL, 0)`，验证的是 437 而非 clone3：其 ENOSYS 结果不能证明 clone3 规则正确。历史诊断的 strace 未跟踪 prctl，而 [bubblewrap 0.11.1 使用 `prctl(PR_SET_SECCOMP)`](https://github.com/containers/bubblewrap/blob/v0.11.1/bubblewrap.c)，因此“未调用 seccomp 系统调用”也不能证明未加载过滤器。

## 规格口径

- 031 §1.6 要求动态权限沙箱限制关键系统调用，且隔离创建失败不得静默降级；033 不提供 031 的应用层 seccomp。
- 038 当前范围要求 attack06 保持真实检查，不得通过跳过或削弱断言代替修复。031 正文未逐项规定 clone3 的 errno；现有实现注释和 attack06 明确将 clone3 的 ENOSYS 作为既定动作与回归断言。本轮不改变它。
- runner 上工具链的四项正向信号实际通过。其余 EPERM 不能仅凭 errno 证明来自产品过滤器。

## 受控对照

执行于 Linux 6.18.33.2 WSL2、x86-64、bubblewrap 0.9.0。测试进程原有外层 seccomp 下，直接对照中 clone3 本来就得到 ENOSYS；此环境可掩盖产品过滤器的缺口。为区分各层，只在临时测试进程叠加对 435 和 437 返回 EINVAL 的合成外层过滤器，再启动带独立 user/pid/ipc/net 命名空间的 bwrap。探针只使用无效参数，不执行宿主级特权操作。测试进程及其后代的限制不传播到父进程。

| 临时 bwrap 对照 | clone3：原始内核返回 / libc 返回 / errno | openat2：libc 返回 / errno |
|---|---|---|
| 仅合成外层过滤器 | -22 / -1 / 22 | -1 / 22 |
| 再加入按基线源码重建的 528 字节过滤器 | -22 / -1 / 22 | -1 / 38 |
| 在重建过滤器中另加入仅供对照的 435→ENOSYS，保留 437 | -38 / -1 / 38 | -1 / 38 |

两次对照均退出 0，逐项串行，每次启动前及运行中检查 cgroup tasks 和可见进程均低于 500；单项超时 20 秒，中断时杀死测试进程组。最终记录的峰值为 263 tasks、51 个可见进程。可复现材料：[探针](probe.c)、[控制器](control.py)、[基线输出](baseline-control.log)、[435 对照输出](clone3-control.log)。控制器的临时工作目录为 `/tmp/attack06-probe-20261108`：将两个源码复制到该目录，运行 `python3 /tmp/attack06-probe-20261108/control.py compile`，再串行运行 `layered` 和 `layered_with_clone3`；每次保留门禁和超时。CI 平台和本机不同，本地对照不等于在目标 runner 的实际产品链上取证。

## 最小修复准备

唯一目标：使 031 过滤器对实际的 x86-64 clone3 编号执行既定 ENOSYS 动作。生产落点 `src/app/src/sandbox.rs` 的 ENOSYS 清单：增加由 `libc::SYS_clone3` 得到的编号，保留现有 437 对 openat2 的实际限制；不要静默移除该限制或改变 errno 断言。建议在 `src/app/src/sandbox/tests.rs` 增加针对真实生成过滤器的编号和动作回归，避免仅检查形状；保留 `src/app/tests/security_attack_031.rs` 中现有 attack06 真实沙箱及工具链断言。已有 `seccomp_filter_shape_valid` 只检查形状，不能发现编号错配。

后续验证应在隔离环境用受控外层对照证明原始过滤器对 clone3 不拒绝、修复后的真实产品过滤器返回 ENOSYS，且 437 现有行为不变；再运行原 attack06，确认工具链正向证据和拒绝断言均通过。目标 runner 的产品链还应捕获完整 argv、过滤器 fd 内容、`prctl(PR_SET_SECCOMP)` 返回值和 clone3 的实际返回，以区分额外加载问题。此类远端验证需后续授权的运行；本轮不触发 CI。

本轮未修改生产代码、仓库过滤规则或测试断言，未运行全量/L3，未提交或推送。
