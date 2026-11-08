---
title: "038 Worker Reaping Root Cause and Fix Plan - Plan"
date: 2026-11-05
type: plan
---

# 038 worker 回收根因与修复计划

## Goal Capsule

**目标：**修复产品可控制的 worker 终止和身份捕获路径，并明确产品失去 wait 归属后的契约选择。

**基线：**分支 feat/038-github-actions-ci，HEAD 57d5408；本工作区已有候选 A、回归、日志和其他任务差异。

**范围：**本文件只记录根因、成熟机制对照和实现计划；本轮不修改生产代码、测试或容器配置。

**成功条件：**每个返回结果都区分“产品已回收”“产品报告失败、外部诊断器随后清理”和“尚未验证”；最终验收能在候选差异上复现产品自己的等待证据。

**保留边界：**保留候选 A 和所有现存差异；候选 B 不进入合并或比较实施范围。

## Product Contract

### 原规格、现有实现和新增验收

| 来源 | 已确认要求 | 本轮含义 |
|---|---|---|
| [016 进程会话规格](../specs/016-process-sessions-plan.md) | terminate=true 和时限结束要终止目标进程组，并有界等待、回收由 ProcessManager 管理的子进程；取消等待不等于杀死已启动进程。 | 016 句柄及其 Child::wait 所有权不能被 JS worker 清理抢走。 |
| [034 JS 编排规格](../specs/034-javascript-tool-orchestration.md) §1.4、§1.6、§1.7、C11–C13 | 终止关闭调用门、结束并回收 JS worker；清理不得杀死独立 016 命令；沙箱 worker 使用独立进程组和 PID 命名空间；CPU 限额为 5 秒、墙钟限额为 120 秒，超限须实际终止并等待。 | 当前 034 契约要求产品完成其管理 worker 的有界终止和回收。现有文字没有明确规定主进程必须 wait 所有在监督者异常退出后被重新托管的任意后代。 |
| [038 规格](../specs/038-github-actions-ci.md) §0-A 及 2026-11-02 用户确认的候选 A | 用户代码只能在隔离确认及清理身份登记后放行；确认失败、取消和超时不得放行；清理失败必须报告，不能保存成功关闭事件。 | pidfd 和确认闸是候选 A 的既有方向；不能把“请求发出”当作“载荷已回收”。 |
| 本轮用户指令与 2026-11-05 收尾要求 | 外层诊断器或测试子收割器的 wait 不得记为产品通过；异常回收要分别报告。 | 这是比“测试结束后没有残留”更严格的验收口径。它约束证据归属，但没有单独决定是否要改变全局 wait owner。 |

因此，产品必须等待自己创建的直接 bwrap 子进程。bwrap 存活时，它也能作为 PID 命名空间载荷的直接父进程并 wait 载荷。现有契约没有确认：bwrap 被外力杀死后，主应用还必须 wait 被重新托管的载荷。若仍要求该路径由产品自身 wait，必须先决定新的等待归属。

### 当前进程关系与等待责任

| 进程 | 宿主可见父进程 | 进程组和会话 | PID 命名空间 | 信号接收者 | 当前 wait owner |
|---|---|---|---|---|---|
| terminal-chat 主进程或测试进程 | 容器进程树中的调用者 | JS worker 的外层组不与主进程组共用；具体 SID 由启动环境提供。 | 容器 PID 命名空间。 | 收到 API 取消、时限和外部终止事件。 | 由运行时和对应 std::process::Child 管理其直接子进程。 |
| bwrap 监督进程 | terminal-chat 的直接子进程；Command::process_group(0) 使其 PGID 等于自身 PID。 | bwrap 有独立 PGID，但沿用调用者 SID。 | 容器 PID 命名空间。 | 当前 pidfd 回退 killpg(child.id()) 会命中这个组。 | terminal-chat 的 Child::wait。 |
| bwrap PID 命名空间子进程 | bwrap 的直接子进程；宿主 /proc 可见 NSpid 末项为 1。 | 启动期间先继承外层组和 SID；bwrap 的 --new-session 在 setup 后调用 setsid()，之后该进程成为自己的 PGID/SID leader。 | 新 PID 命名空间的 PID 1；--as-pid-1 使它承载 worker，而非 bwrap 内置 reaper。 | 正常关闭时由 payload pidfd 收信号；当前双重 pidfd 拒绝后 bwrap 死亡会触发 --die-with-parent。 | bwrap 存活时由 bwrap wait；bwrap 死亡后不再是 terminal-chat 的子进程。 |
| worker 入口及其 JS 用户代码 | 与上行进程为同一进程，入口 exec 后成为 terminal-chat --internal-js。 | 自停标记生效后仍处于当前会话；现有确认回调没有证明它已经进入 T 停止态。 | 新 PID 命名空间 PID 1。 | worker 关闭、取消、CPU 限额或墙钟超限触发清理。 | bwrap monitor 存活时 bwrap wait；外部监督者死亡后由最近的 subreaper 或 PID 命名空间 init 接管。 |
| 诊断 harness 的 subreaper | 测试容器外层执行器的子进程。 | 不属于产品 worker 组。 | 诊断所在容器的 PID 命名空间。 | 通常不负责产品信号；只观察、限制任务数并在测试结果记录后清理。 | 它只能证明诊断器自己执行了 wait，不能证明产品通过。 |
| 容器 PID 1 / init | 容器根进程。 | 不属于 worker 专属组。 | 容器 PID 命名空间的 PID 1。 | 由 Docker 生命周期信号驱动；正常运行期不一定收到 worker 的直接信号。 | 只会 wait 被重新托管给它的进程。实际部署 PID 1 及 init 设置仍需对部署容器核实。 |

启动参数来自 [sandbox.rs](../../src/app/src/sandbox.rs)：--as-pid-1 --new-session --die-with-parent --unshare-user --unshare-pid --unshare-ipc。worker 外层由 [runtime.rs](../../src/app/src/js_orchestration/runtime.rs) 建立独立进程组。/proc 拓扑样本见 [拓扑记录](../logs/038-github-actions-ci/2026-11-05-process-reaping-debug.md) 和 target/038-validation/reap-probe-20261105-topology.log、target/038-validation/reap-probe-20261105-kill-denied-topology.log。

### 因果链和路径归因

| 路径 | 触发到残留的因果链 | 证据状态与归因 |
|---|---|---|
| 正常关闭、取消、CPU 或墙钟超限 | 关闭门或限额触发 → 产品对 payload pidfd 发 SIGKILL；若该信号被拒而 SIGTERM 可用，则对同一 pidfd 发 SIGTERM → bwrap monitor 保持存活并 wait PID 1 → pidfd 出现 HUP → 产品清理 bwrap 外层组并 wait 自己的 Child。 | 已有正常、取消及 CPU／墙钟记录支持这条路径。pidfd HUP 表示进程已被回收，但单凭 HUP 不标明回收者；产品通过还需结合 bwrap 仍是父进程、其 wait 代码及测试期间没有 harness adoption。 |
| SIGKILL 与 SIGTERM 都被 pidfd 策略拒绝 | 两个 pidfd 信号均失败 → 当前回退对 bwrap 的 PGID 发 SIGKILL → bwrap 退出 → --die-with-parent 使沙箱载荷收到父死亡信号 → 载荷成为外部孤儿 → 最近的 harness subreaper 接管并 wait → 产品没有 payload wait 归属，返回 WORKER_REAP_FAILED。 | “产品报错、诊断器随后 wait”已由 closeout-kill-denied-final 和 topology 日志证实。当前回退命中 bwrap 组；steady-state worker 因 --new-session 已离开该组。这是产品回退路径缺陷，不是 pidfd 失去 PID 身份固定能力。测试过滤器只拒绝 pidfd SIGKILL/SIGTERM；没有证据表明它也拒绝 killpg。 |
| bwrap 在放行前死亡 | bwrap 消失 → 当前 confirm_sandbox_with_release 返回 LeaderGone → 无 payload pidfd 时产品只对 bwrap PGID 发信号并 wait bwrap → 若 namespace 子进程已执行 setsid()，外层 killpg 不再命中它 → --die-with-parent 是否已安装取决于死亡时序 → 可能由较高层接管。 | 具体 pre-release 分支尚未被可靠屏障复现。上游 bwrap 0.9.0 源码把 PR_SET_PDEATHSIG 安装放在 exec 前；Linux 语义不补发已错过的父死亡事件。因此“早期 setup 子进程可逃过该信号”是有代码依据的推断，不是本机运行事实。 |
| bwrap 在放行后被外力杀死 | 外力杀 bwrap → 产品仍持有 payload pidfd，可向固定身份发信号 → payload 父进程已消失，payload 不再是 terminal-chat 的子进程 → waitid(P_PIDFD) 仍受子进程归属限制 → payload 被最近 subreaper 或 PID namespace init 接管 → 产品返回 WORKER_REAP_FAILED，之后诊断器 wait。 | 已由 closeout-supervisor-dies-final 和 topology 日志证实。pidfd 可以固定身份和发信号，但不会转移 wait 权限。这是产品内现有等待者已经消失后的架构边界。 |
| 身份捕获期间目标退出 | confirm 读到 namespace 子进程 → 产品打开 pidfd并先 poll 一次 → 随后逐项读取 namespace、PPID、start time、NSpid → 目标可能在第一次 poll 后退出 → 当前代码尚未在快照后再 poll，且完成全部校验前没有把 fd 存入 WorkerCleanup → 错误回滚可能走无 pidfd 分支。 | 现有 payload_identity_capture_rejects_exit_after_pidfd_open 在调用校验前已终止目标，只覆盖“打开时已退出”，没有覆盖检查与快照之间的退出。竞态窗口由当前代码确定存在；它是否造成无法回收的具体残留尚未验证。 |
| 放行阶段目标尚未进入 worker 入口 | confirm 发现不同 PID namespace 的子进程 → 回调核对 NSpid 末项为 1，但不核对进程是否是 terminal-chat --internal-js、是否已 setsid()、是否处于 T 停止态 → SIGCONT 可能早于 worker 入口自停 → 初始化帧已写入后 worker 仍可能自停 → 调用进入挂起或超时清理。 | 代码顺序和 bwrap 源码支持该窗口；现有拓扑样本曾观察 setup 阶段 comm=bwrap、状态 D，但没有把样本时点和真实 release 回调做成同一确定性断言。属待补实测的启动竞态。 |

正常路径的现有 pidfd 回收和 034 限额结果可复用，不能拿来替代两个 pidfd 信号都失败、监督进程死亡或身份捕获窗口的验收。POLLIN 只说明目标已退出；POLLHUP 表示它已被 wait 回收，但等待责任要从进程父子关系和实际 wait 调用判断。

### 产品、bwrap、PID 1 和插桩的边界

- **产品缺陷，已证实：**双重 pidfd 信号拒绝时，回退杀死 bwrap 等待者；产品返回失败而不报成功，但丢失了可继续 wait payload 的当前所有者。该分支可通过向已验证的 worker 进程组发信号，同时保留 bwrap 修复。
- **产品缺陷，代码已证实、后果未完全验证：**身份快照只在读取 /proc 前检查 pidfd 一次；验证中途未再检查，且 pidfd 在验证结束前未交给清理对象。实际异常是否导致残留仍待竞态测试。
- **bwrap 行为，版本证据明确：**本机报告 bwrap 0.9.0-1ubuntu0.3；上游源码用 setsid() 建立 sandbox session、--as-pid-1 跳过内置 reaper 分叉，并在 exec 前设置 PDEATHSIG。尚未比较 Ubuntu 补丁与上游 v0.9.0 的完整源码差异；源代码结论按“上游同版本”引用，运行事实按本机日志引用。
- **内核行为，已由权威接口文档确认：**waitid(P_PIDFD) 仍要求目标是调用者的子进程；最近活着的 subreaper 接管孤儿；PID namespace PID 1 退出时内核终止该 namespace 中其余进程。pidfd 不改变这些等待归属规则。
- **容器 PID 1 的责任：**容器 init 可回收最终被重新托管给它的孤儿；它不能让应用进程重新获得 waitid 归属。当前 harness 主动设为 subreaper，因而改变了孤儿最终接管者；它的 wait 仅用于有界清理测试现场。
- **插桩限制，已知：**诊断器在产品结果采集后才 wait；其观察可证明“此前出现孤儿并由诊断器清理”，但不能证明无插桩部署中由容器 PID 1 处理，也不能证明产品已回收。所有后续检查继续串行并先过 500 任务门禁。
- **部署事实，未验证：**.devcontainer/compose.yaml 当前没有声明 init: true；本次没有查询运行中目标容器的实际 PID 1 或宿主创建参数。计划不把工具容器 PID 1 快照外推为产品部署事实。

### 016 sh 接管现象

ProcessManager::monitor_thread 的终止和 Child::wait 逻辑没有被候选 A 改写；但候选 A 修改了 proc_session.rs 的共享沙箱启动路径，在 monitor 启动前加入同步确认闸，也改了 sandbox 启动行为。因此，不能仅凭“终止函数未改”就断言该现象完全不受本轮影响。

已有 C13 检查通过，证明 JS terminate 后 016 句柄仍可续接；记录中的 sh 是真实 exec_command 进程，在测试进程退出后才被外层 subreaper 接管。这个时间点与“016 句柄仍活着”的契约一致，不能单独证明产品回收缺陷。最小判断是：worker 的 JS kill_worker 修复没有直接改 016 wait 逻辑；共享 launch gate 仍是直接受影响面；保持 C13 作为回归，若最终候选在句柄存续或续接上失败，再按该共享路径追因，不顺手扩展本 worker 回收修复。

## Planning Contract

本计划只推荐保留现有 bwrap monitor 作为可控路径的 wait owner。它不预选主进程 subreaper、per-worker helper 或容器 init。

在实现前，用户需要决定：是否要求“bwrap 被外力杀死后，产品仍必须 wait 被收养的 payload”。若要求，用户还需选择全局或 per-worker 等待归属；若不要求，规格需明确产品错误返回与容器级最终回收的边界。容器 init 不能满足“产品自身 wait”的新口径。

## Research Comparison

| 来源及版本 | 适用条件 | 能解决的路径 | 不能解决的路径 | 可借鉴机制、所需改动和主要代价 |
|---|---|---|---|---|
| Linux waitid(2) 与 [pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)、[pidfd_send_signal(2)](https://www.man7.org/linux/man-pages/man2/pidfd_send_signal.2.html)；Linux man-pages | pidfd 固定一个进程身份并可对其发信号；waitid(P_PIDFD) 目标仍须是调用者的子进程。pidfd POLLIN 与 HUP 区分退出和已回收。 | 避免对已复用 PID 发错信号；bwrap 仍存活时确认 payload wait。 | 不能赋予调用者新的 wait 权限；不能让死亡的 bwrap 继续 wait；不能消除 pidfd 信号被 seccomp 拒绝。 | 继续复用候选 A 的 pidfd 身份句柄；捕获时立即交给清理对象，并在 /proc 快照后再检查就绪状态。无需新依赖；需严密处理退出竞态。 |
| Linux [PR_SET_CHILD_SUBREAPER](https://www.man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html)、[pid_namespaces(7)](https://www.man7.org/linux/man-pages/man7/pid_namespaces.7.html)、[PR_SET_PDEATHSIG](https://www.man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)、[setsid(2)](https://man7.org/linux/man-pages/man2/setsid.2.html) | subreaper 必须仍活着且位于孤儿祖先链；wait 仍须由被重新托管的父进程执行；PDEATHSIG 只关联直接父进程。 | 解释 bwrap 死亡后的接管者和 PID namespace 内 PID 1 死亡时的级联信号；可让选择的上层进程成为孤儿 wait owner。 | subreaper 不自动杀活进程；PDEATHSIG 不继承 fork，不能补发已经错过的父死亡；pidfd 不绕过 wait 归属。 | 可作为架构决策的语义依据。主进程开启 subreaper 会改变整个进程的孤儿归属和 wait 范围，并要求协调所有现有 Child::wait。 |
| containers/bubblewrap 上游 v0.9.0 [bwrap.xml](https://github.com/containers/bubblewrap/blob/v0.9.0/bwrap.xml) 与 [bubblewrap.c](https://github.com/containers/bubblewrap/blob/v0.9.0/bubblewrap.c)；本机 Debian/Ubuntu 包 0.9.0-1ubuntu0.3 | 当前沙箱确实使用 --as-pid-1 --new-session --die-with-parent。上游源码确认 PID namespace clone、setsid、reaper 选择、PDEATHSIG 和 exec 顺序。 | 保留 bwrap 时，可由 bwrap monitor wait sandbox PID 1；其 --die-with-parent 会在 monitor 死亡后给直接 sandbox 子进程发信号。 | --as-pid-1 关闭 bwrap 内置 reaper；bwrap 自身死亡后不能由它 wait；PDEATHSIG 安装前的死亡竞态未由现有规格证明已封闭。 | 首选复用现有 monitor wait；在 monitor 活着时把拒绝信号回退改为 worker 的已验证 PGID。可测试去掉 --as-pid-1 的内置 reaper，但这会改变 namespace PID 布局且仍解决不了 monitor 死亡。Ubuntu 补丁源码对照仍需在实施前补齐。 |
| bubblewrap issue [#529](https://github.com/containers/bubblewrap/issues/529) | 维护者说明 PDEATHSIG 只作用于直接子进程，且不会从父进程 fork 继承；PID namespace 中 PID 1 退出会终止 namespace 内其余进程。 | 支持核对 PDEATHSIG、fork 和 PID 1 级联的语义假设。 | 不是本项目当前二进制、启动阶段或 PID 树的证据；不能证明本地载荷一定已被 wait。 | 用作假设核验，不复制 issue 现场结论。无需产品改动；本地仍须用拓扑记录和同版本源码定位。 |
| bubblewrap issue [#697](https://github.com/containers/bubblewrap/issues/697)，报告版本 bwrap 0.11.0，Alpine 3.22，Docker on arm64-darwin | 报告者观察到没有 Docker init 时 bwrap 自身成为 zombie，并提出 docker run --init；issue 仍是报告和讨论。 | 提醒容器 PID 1 可能不回收其收养的孤儿或退出后的直接进程。 | 现象对象是 bwrap zombie；本项目问题对象是 bwrap 死亡后 namespace payload 的 wait owner。本机 bwrap 为 0.9.0 Ubuntu 包。 | 作为容器 PID 1 风险线索，不作为本项目根因。启用 init 需部署改动，不能替代产品 wait。 |
| Docker 官方 [docker run --init](https://docs.docker.com/reference/cli/docker/container/run/) 与 [Compose init](https://docs.docker.com/reference/compose-file/services/#init) | Docker 可为容器 PID 1 放置小型 init；它转发信号并回收容器中的孤儿。 | 当孤儿最终被重新托管给容器 PID 1 时做容器级 wait；覆盖应用主进程不负责的容器生命周期残留。 | 不让 terminal-chat 对容器 init 已收养的进程调用 waitid；不自动保证 worker 组终止后产品确认 payload 已回收。 | 借鉴部署的最终兜底。需要改 compose 或运行参数并调整验收契约；扩大所有容器进程的信号和回收行为。当前不预选。 |
| krallin/tini v0.19.0 [README](https://github.com/krallin/tini/blob/v0.19.0/README.md) 与 [tini.c](https://raw.githubusercontent.com/krallin/tini/v0.19.0/src/tini.c) | Tini 可作为容器 PID 1 或由 -s 设为 subreaper；可向直接子进程或进程组转发信号，并循环 waitpid(-1, WNOHANG) 回收可 wait 子进程。 | 借鉴信号转发和集中 wait 循环；当 Tini 活着且孤儿已被托管给它时，可 wait 该孤儿。 | 若 Tini 在外层且产品仍是 worker wait owner，它不会修复 app 的归属丢失；Tini 退出边界也不保证等待仍存活的脱离进程组后代。 | 可借鉴 reaper 循环和进程组信号概念。per-worker 采用时仍要设计 helper 生命周期、停止条件和结果 IPC；不能直接把 Tini 配成全局 init 当成产品通过。 |
| Yelp/dumb-init v1.2.5 [README](https://github.com/Yelp/dumb-init/blob/v1.2.5/README.md) 与 [dumb-init.c](https://raw.githubusercontent.com/Yelp/dumb-init/v1.2.5/dumb-init.c) | 默认将主子进程放入独立 session，向进程组转发信号；SIGCHLD 时用 waitpid(-1, WNOHANG) 收割退出子进程。 | 借鉴隔离进程组、转发和非阻塞 wait 循环。 | 主 child 结束时会对剩余组发信号并结束；不保证等待脱离组的存活后代，也不自动报告产品层 payload wait 完成。 | 可借鉴组信号模式，不直接复用其退出策略。把它加为 helper 要承担额外进程、IPC、生命周期和失败兜底。 |
| agent-substrate/env issue [#105](https://github.com/agent-substrate/env/issues/105)，2026-10 社区报告 | 社区容器环境报告 PID 1 不收孤儿，维护者讨论 reaper 与 supervisor 分工。 | 提出“wait 与主 child 生命周期竞争”以及 init/reaper 分层的验证问题。 | 不对应本项目 bwrap 版本、候选 A 或本机 PID 树；不能作为根因证据。 | 仅用于补充架构风险清单；实现前仍以 Linux、Docker、bwrap 官方语义和本仓库实测为准。 |

结论是先正确使用现有机制：保持 bwrap 存活、使用已验证 worker 身份和独立 PGID 终止、再由现有 bwrap 与 Rust Child wait。新增全局或 per-worker reaper 只有在用户要求覆盖外部 supervisor 死亡时才进入方案评估。

## Implementation Units

下列步骤只在用户审阅本计划并决定 wait owner 契约后执行。每一步保持单项串行运行、可中断、使用 target/038-validation/reap-probe.py 的 500 门禁；不重跑 L3。

| 步骤 | 修改位置和动作 | 验收方法与通过标准 |
|---|---|---|
| U0 关闭契约决策 | 更新 docs/specs/034-javascript-tool-orchestration.md 与 docs/specs/038-github-actions-ci.md，记录 bwrap 外力死亡后的产品 wait 要求；未获决定前不改变 wait owner。 | 规格区分产品管理的直接子进程、bwrap 活着时的 namespace payload，以及 supervisor 死亡后被重新托管的 payload。 |
| U1 身份捕获和放行 | 优先只改 src/app/src/js_orchestration/runtime.rs；若通用确认接口无法表达受控检查，才最小改 src/app/src/sandbox.rs。打开 pidfd 后立即放入回滚对象；核对 pidfd、PID namespace、NSpid、PPID、start time、PGID、SID；读取快照后再 poll pidfd；有界等待真实 worker 入口达到自停态 T 后才发送 SIGCONT。 | 在首轮 poll 后、身份读取期间用确定性屏障终止目标；断言不发送 init 帧、不保存 CellStarted、错误有界返回、pidfd 始终用于精确身份清理、bwrap 保持 wait 责任并由产品 wait。另证明启动阶段的 comm=bwrap 子进程不会被误当作已自停 worker 放行。 |
| U2 pidfd 拒绝回退 | 改 runtime.rs，使用 U1 验证得到的 worker PGID，而非 bwrap PGID；双重 pidfd 信号拒绝时向 worker 组发 SIGKILL，保留 bwrap 存活并 wait payload 和 bwrap。若 group signal 也失败，返回清理失败，不写成功关闭事件。 | 用现有 sandbox_worker_kill_denied_reports_failure 改成证明 pidfd 信号拒绝但进程组信号可用时产品自身完成回收；在断言前禁止诊断器 wait。另单独覆盖产品无法发出组信号时返回明确错误，不把外部收尸计为通过。 |
| U3 pre-release supervisor 退出 | 增加一个确定性测试屏障，分别在 bwrap namespace 子进程执行 setsid() 前后、PDEATHSIG 安装前后杀监督者；优先沿用 runtime.rs 清理和现有 bwrap 生命周期，不默认引入 helper。 | 每个时点检查 worker 是否收到用户帧、具体 PID/starttime/NSpid/PGID、信号结果、是否仍为产品子进程、wait 状态和最终归属。产品仍拥有的路径必须由产品 wait；无法归属的路径保持失败分类并交 U0 的契约决定。 |
| U4 post-release supervisor 退出 | 仅在 U0 要求产品继续 wait 后实施。评估主进程 subreaper、per-worker helper、容器 init 和契约调整的最小方案；如需代码，涉及 runtime.rs、proc_session.rs 的中央 wait 协调、必要的 helper 模块或 .devcontainer/compose.yaml。 | 若选择产品 wait，杀 bwrap 后必须在产品结果返回前证明产品 owner 对精确 payload PID 执行 wait；诊断 subreaper 的后续 wait 不计通过。若选择容器 init，规格必须将验收改为“容器级最终回收”，不能称产品自身 wait。 |
| U5 034 终止回归 | 复用 src/app/tests/js_orchestration_security.rs 的 cpu_limit_kills_spin_worker_host_continues 和 wall_clock_limit_terminates_pending_sandbox_worker，在最终改动后各串行执行一次。 | CPU 5 秒限额触发实际 SIGXCPU/终止并确认 payload 与 supervisor 回收；120 秒墙钟 deadline 到达后返回 WALL_TIME_LIMIT，实际终止 pending worker，并确认 payload 与 supervisor 回收。两例均检查后续 cell 或宿主继续工作。既有候选记录中的通过结果是修改前证据；本轮清理路径改动后必须更新。 |
| U6 016 和现有回归 | 保留 C13 c13_terminate_keeps_016_process_handle_alive；若 U1/U4 改动共享 proc_session.rs 或 sandbox 启动，则执行该单项。只在 C13 证明句柄丢失或错误终止时调查 sh 接管，不顺带重写 016 monitor。 | 016 进程在 JS 终止后仍可从同一 ProcessManager handle 续接；独立 016 组未被 worker PGID 信号击中。记录 harness 在测试进程退出后的接管时间，不把保留中的活进程计为回收成功或失败。 |
| U7 最终简化 | 串行检查本轮最终触及的 runtime.rs、必要的 sandbox.rs、proc_session.rs 和相关回归。保留证明身份竞态、产品自 wait、双重信号拒绝和真实限额终止的用例；不清理其他任务差异。 | 记录检查范围、采纳和跳过的简化。任何改变回收时序、状态判断、信号目标、wait owner 或回归断言的简化都使受影响检查失效，须先重跑这些检查。 |

### 等待归属选项和代价

| 选项 | 责任分配 | 能覆盖 | 主要代价和边界 |
|---|---|---|---|
| A. 保持当前 bwrap wait owner | 产品发终止信号并等待；活着的 bwrap wait PID 1；产品再 Child::wait bwrap。 | 正常关闭、取消、CPU／墙钟超限、pidfd 信号拒绝但组信号可用，以及其他 bwrap 存活路径。 | 不能承诺 bwrap 外力死亡后由产品 wait payload。适合当前已确认契约的最小修复。 |
| B. 主进程成为 child subreaper | 主进程接收 bwrap 孤儿并统一 wait，调用方必须把退出状态交给唯一 owner。 | bwrap 死亡后重新托管给主进程的 payload。 | 影响进程内所有孙进程；必须协调 std::process::Child、ProcessManager、后台线程的 wait，避免竞争和误收 unrelated child。 |
| C. 每个 worker 增加 helper subreaper | helper 启动并拥有 bwrap，收到有界请求后终止和 wait payload，再报告结果。 | 主进程退出前 helper 仍活着时的 worker 及 bwrap 异常退出。 | 增加进程、协议、超时和 helper 退出恢复；helper 自身死亡后仍落到更高层 PID 1。需要明确 IPC 和会话隔离。 |
| D. 启用容器 init | Docker init/Tini 成为容器 PID 1，接管被托管的孤儿。 | 容器级最终回收，避免孤儿留给无回收能力的 PID 1。 | 不满足产品自身 wait；改部署配置并改变验收口径；影响同容器所有服务进程。 |
| E. 明确缩窄异常契约 | 产品继续返回 WORKER_REAP_FAILED；说明外部 supervisor/PID 1 负责最终接管，不将其记作产品回收通过。 | 保持当前 wait owner 和局部产品范围。 | 不能满足本轮“所有异常由产品 wait”的新增要求；用户需明确接受失败返回后的容器级清理边界。 |

当前推荐只落地 A 能覆盖的路径。若用户要求覆盖 bwrap 外力死亡，则停止实施 U4，等待用户从 B、C、D 或 E 中选择，并据此更新规格后再继续。

## Verification Contract

### 验收矩阵

| 场景 | 预期产品行为 | 产品通过证据 | 失败或未验证分类 |
|---|---|---|---|
| 正常完成后关闭 | 关闭门后终止 worker 和其受控叶子，再 wait。 | 被跟踪的 PID/starttime 消失；payload pidfd HUP；bwrap Child wait 完成；无诊断器 adoption。 | 任一等待超时或产品只报告信号成功时记失败。 |
| 016 叶子在途时 JS cancel/terminate | 只清理 worker 组；按 034/C13 保留独立 016 句柄。 | C13 可从原 handle 续接；独立组存活。 | 诊断器事后杀掉叶子不能算产品符合 016。 |
| CPU 5 秒限额 | 实际终止 CPU-bound worker，宿主继续服务。 | 复用 CPU 用例的限额错误、payload/bwrap wait 和后续 cell 结果。 | 只让观察器返回、未触发限额或只看到诊断 cleanup 都失败。 |
| 120 秒墙钟限额 | deadline 到达后实际终止 pending worker。 | 复用墙钟用例的 WALL_TIME_LIMIT、payload 与 supervisor 回收证据。 | 短时观察 timeout 不替代该用例。 |
| pidfd SIGKILL 被拒、SIGTERM 可用 | SIGTERM 作用于同一 pidfd 身份；bwrap 保持活着并 wait。 | 现有 sandbox_worker_kill_denied_uses_term_fallback 通过且产品观察到 payload HUP 和 Child wait。 | 诊断器 later wait 不算产品通过。 |
| pidfd SIGKILL 与 SIGTERM 都被拒、worker PGID 信号可用 | 只杀已验证 worker PGID；保持 bwrap wait owner；wait payload 后 wait bwrap。 | 精确 PID/starttime 的产品 wait 证据；产品结果记录前无 harness wait；不写错误的成功关闭事件。 | 仍返回 WORKER_REAP_FAILED 表示未达当前可控路径目标；诊断 cleanup 另列。 |
| pidfd 与 worker PGID 信号都失败 | 不误报清理成功；保留失败结果。 | 明确报告失败及已尝试操作；任务始终有界；记录外部最终接管者。 | 外部 wait 只能证明环境清理，不能把本场景改为产品通过。 |
| 身份捕获期间目标退出 | 不放行用户帧；使用同一个 pidfd 做精确清理；bwrap 存活时由 bwrap wait。 | 屏障准确落在首 poll 后、快照期间；pidfd 身份和进程组断言；结果在期限内返回；产品 wait 先于诊断器。 | 当前单测“调用 verify 前已退出”不覆盖该竞态。 |
| 放行前 bwrap 死亡 | 不发送用户帧；报告实际 payload 身份和 wait 状态。 | 可重复的 setup barrier 分别覆盖 setsid 与 PDEATHSIG 时序。 | 无确定时序证据的旧 watcher 结果不得记通过；无 wait owner 时留待 U0 决定。 |
| 放行后 bwrap 被外力杀死 | 按用户决定的 wait owner 执行。 | 若要求产品 wait，必须证明产品对被收养 payload 实际 wait；否则只证明产品如实报错及容器最终清理。 | 默认分类为“产品报错但依赖诊断／容器清理”，不得标产品通过。 |

### 执行约束和证据

实施验证只用现有 target/038-validation/reap-probe.py，各运行串行执行。每次运行先通过 500 任务门禁，使用独立临时目录、内部超时和可中断的外层 timeout -k 15。先由产品断言并写入结果，再允许诊断器执行 wait 和失败现场清理。根据 PID、starttime、父 PID、PGID、SID、PID namespace、pidfd poll 与 wait 返回判断残留；不要求总线程数每次完全相同。

CPU 和墙钟使用现有 closeout-cpu3、closeout-wall3 模式，不增加等价观察测试。现有候选证据：CPU cpu_limit_kills_spin_worker_host_continues 5.30 秒通过；墙钟 wall_clock_limit_terminates_pending_sandbox_worker 120.06 秒通过。它们只代表当前记录中的候选差异；清理逻辑改动后，各重跑一次并把原始日志及时写入宿主绑定的 target/038-validation/ 路径，再更新正式日志。

必须保留并复用以下已有效检查：正常回收、捕获拒绝、放行 SIGCONT 被拒、SIGTERM 回退、双重 pidfd 信号拒绝、监督者死亡、C13、CPU 和墙钟测试。新增身份竞态检查必须证明旧顺序能失败、修复顺序通过；不重复运行 L3，不处理其他失败，不执行 CI。

## Definition of Done

- 产品可控制路径按精确身份、已验证进程组和现有 bwrap wait owner 完成有界终止与等待。
- 返回“成功关闭”前，产品已拿到约定的 payload 与 supervisor 回收证据；返回失败时不写成功关闭事件。
- 身份捕获中途退出测试命中竞态，并证明不会放行用户代码或依赖诊断器收尸。
- 用户已决定 bwrap 异常死亡后的等待契约；若选择改变 wait owner，规格和实现已反映该决定。未决定时，U4 不实施，文档保持为等待审阅的计划。
- CPU、120 秒墙钟和 C13 结果对应最终代码；串行简化记录列出范围、采纳／跳过项及因简化重跑的检查。
- 最终证据分类明确列出“产品自身回收通过”“产品报错但依赖诊断清理”“尚未验证”。
- 没有重跑 L3、没有处理其他任务失败、没有提交或发布。

## Appendix

### 本次调查依据

- [038 定向诊断与修复记录](../logs/038-github-actions-ci/2026-11-05-process-reaping-debug.md)：历史复现、旧实现红灯、候选 A 定向结果和测试插桩边界。
- [038 收尾回归记录](../logs/038-github-actions-ci/2026-11-05-process-reaping-closeout.md)：正常和异常回收分类、CPU／墙钟用例结果、C13 与简化检查记录。
- 原始 PID 拓扑：target/038-validation/reap-probe-20261105-topology.log 与 target/038-validation/reap-probe-20261105-kill-denied-topology.log。
- [Linux waitid(2)](https://man7.org/linux/man-pages/man2/waitid.2.html) 与 [PID namespace 语义](https://www.man7.org/linux/man-pages/man7/pid_namespaces.7.html)。
- 本机 bwrap：/usr/bin/bwrap --version 为 0.9.0；包版本为 0.9.0-1ubuntu0.3。Ubuntu 补丁与上游 v0.9.0 源码差异尚未逐块核对。
- 执行器位置：/workspace；测试和原始证据不搬离 target/038-validation/。运行时仍须先检查当前任务状态和 500 门禁。
