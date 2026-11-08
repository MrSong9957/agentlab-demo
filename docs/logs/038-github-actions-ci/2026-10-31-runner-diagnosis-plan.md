# 038 runner 沙箱异常诊断方案（待授权的 CI 诊断轮执行）

三个 runner 实测异常在同版本 bwrap（0.9.0 与 0.11.1）的本地环境均**不复现**，
差异收敛到 runner 内核（6.14.x 裸机 VM）与本地（6.18.33 WSL2＋docker 嵌套）之间。
以下诊断按最小成本排序，全部为只读采集，不改产品代码。

## D1 seccomp 生效性（attack06：clone3 应 ENOSYS 实测 EINVAL）

已排除（本地实证）：bwrap 版本（0.9.0/0.11.1 均过）、过滤器程序结构（arch 门→
deny 清单→clone 掩码→ALLOW，无环境敏感分支）、fd 传递链（fail-closed 无静默
None/Err 路径）。

runner 侧步骤（在跑 attack06 的 job 内、同环境执行）：

1. `bwrap --version`＋`uname -r`（确认 0.11.1 与内核版本）。
2. 最小复现（不经产品代码）：以产品同款参数手工起 bwrap＋seccomp memfd——
   `python3 -c` 构造与 build_seccomp_filter() 字节一致的 cBPF（arch 门→437→ENOSYS→
   ALLOW），`memfd_create` 写入后：
   `bwrap --ro-bind / / --unshare-user --unshare-ipc --cap-drop ALL --seccomp 3 \
    --dev-bind /dev /dev -- /bin/sh -c 'printf 0 | gcc -x c - -o /tmp/p; /tmp/p'`
   （/tmp/p = `syscall(437,NULL,0)` 打印 errno）。
3. 对照：去掉 `--seccomp` 跑同一探针（区分"过滤器加载失败"与"加载未生效"）。
4. `strace -f -e trace=seccomp,memfd_create,close_range bwrap ...`（确认 bwrap 是否
   调用 seccomp(SECCOMP_SET_MODE_FILTER)、返回值、及 close_range 是否误伤 memfd）。
5. 若加载成功但仍 EINVAL：对比 6.14 内核 `SECCOMP_RET_ERRNO` 语义（检查
   `dmesg | grep -i seccomp`、`sysctl kernel.unprivileged_bpf_disabled` 等相关项）。

判读：步骤 3 与 2 结果相同 ⇒ bwrap 未安装过滤器（bwrap 侧）；步骤 2 ENOSYS 而产品
路径 EINVAL ⇒ 产品 fd 传递/参数差异（回对 SandboxLaunch argv）。

## D2 沙箱进程树回收（c13/e_reaping：组杀后成员存活超 poll 窗口）

1. 复现用例（已恢复真实检查，runner 上直接跑）＋并行采集：
   `watch -n0.2 'ps -eo pid,ppid,pgid,sid,stat,cmd | grep -E "bwrap|sleep"'`
   —— 记录 close() 后 bwrap 与 sleep 的 pgid/ stat（Z? D? 存活？）。
2. 关键判读：bwrap 自身是否死亡。
   - bwrap 存活 ⇒ killpg 目标组与 bwrap 实际 pgid 不符（查 spawn 的 setsid 是否
     成功——在 pre_exec 内失败会走 spawn 错误，不会静默）；
   - bwrap 死而 ns 内 sleep 存活 ⇒ PID ns 级联销毁未发生（内核行为，附内核版本）。
3. `cat /proc/<sleep-pid>/status | grep NSpid`（确认 ns 归属）。

## D3 沙箱内 MCP 服务器（b2_mcp：tools=0）

1. 用例失败时的 outcome JSON 已含 started/tools/probe_rec——补采：
   服务器进程 stderr（fixture 的 --record 侧信道）与 `ps` 快照，
   区分「进程死」/「进程活但未列工具」。
2. 探针侧加一次性诊断（如获授权改测试）：list_tools 返回前的原始 stderr
   转储（诊断后再移除）。
3. 对照本地：同一 fixture 在本地 bwrap 0.11.1 下 tools=1——差异只可能来自
   服务器进程在 runner 沙箱内的启动/依赖行为。

## 执行载体

上述步骤写成一个独立 job（不进门禁）：`workflow_dispatch` 触发、只跑诊断脚本并
上传日志工件。待用户批准推送/运行后执行；脚本可由本方案直接机械翻译。
