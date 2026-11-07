# 038 进程回收定向诊断（2026-11-05）

## 范围与控制

- 工作目录 `/workspace`，分支 `feat/038-github-actions-ci`，HEAD `57d5408`；当前候选 A 未提交，B 保持备份，未合并。
- 使用上一轮 L3 输出中登记的测试二进制 `target/debug/deps/js_orchestration-af746bcd027a6714`；本轮没有重新编译。二进制 SHA-256：`a766846f217dce00995d54ea061c131d1319650f0f040388296bf068e8f509d4`。
- 诊断执行器：`target/038-validation/reap-probe.py`。容器工作目录 `/workspace`，通过 `docker exec` 前台运行，外层 `timeout -k 15 75`；监督器内部设置 45 秒上限。
- 监督器先设为 Linux child subreaper，让本轮孤儿被监督器接管，避免直接遗留给容器 PID 1。目标子进程先自停；监督器取得 pidfd 和进程组后才放行。它以约 5ms 周期记录 `/proc`，进程数或 cgroup PID 任务数达到 500 时停止目标进程组及已确认子进程；只向本轮已跟踪且 starttime 一致的 PID 发信号。子收割器改变了孤儿最终接管者，因此不能把本次观察直接说成未插桩环境中的 PID 1 实测。
- 宿主 Docker stats：两场景前、后均为 35 个 PID 任务；该口径含线程，不等于进程数。监督器内部两场景起始均为 6 个可见进程、0 个僵尸、36 个线程及 36 个 cgroup PID 任务，含监督器自身。

## 沙箱场景与普通对照

仅运行既有集成用例 `pure_js_text_output_completes --exact --nocapture`，不运行整个测试目标。先启用 `TCHAT_SANDBOX=1`，再运行未设置该变量的普通模式对照。两次均使用独立 HOME、配置及临时目录；没有读取用户凭据。

| 场景 | 用例退出码 | 关键观察 | 有界等待 2 秒后 |
|---|---:|---|---|
| 沙箱 | 0 | 测试进程 163 的子进程 167 为 `bwrap`；168 先为其子进程，随后变为 `terminal-chat`。167 消失后，168 被子收割器 157 接管，进入 Z；监督器 `waitpid` 得到 168 的状态 `-9`。 | 6 个可见进程、0 僵尸、36 线程、36 个 cgroup 任务；已跟踪进程无残留。 |
| 普通模式 | 0 | `terminal-chat` 178 为测试进程 176 的直接子进程，在测试进程退出前消失；未观察到被监督器接管的 worker。测试进程自身短暂 Z 后由监督器 wait。 | 6 个可见进程、0 僵尸、36 线程、36 个 cgroup 任务；已跟踪进程无残留。 |

原始日志：

- `target/038-validation/reap-probe-20261105.log`，SHA-256 `b4f97d0d04e7f1da9c0deab74836b777750e0464c2bed920e6997b3fc5814066`。
- `target/038-validation/reap-probe-20261105-plain.log`，SHA-256 `00ba192f5d2991f3a2096fe69441170bc247b6ec1b349d365f3ebe7fd7fd4d72`。
- 两个用例各自的 stdout 保存在同目录 `reap-probe-test-20261105.log` 和 `reap-probe-test-20261105-plain.log`。

## 归因与边界

- 已证实：本次沙箱短场景中直接子进程 `bwrap` 消失后，worker 被重新托管，退出后曾成为僵尸；监督器主动 wait 后才消失。测试返回 0 不足以证明后代已回收。普通对照未观察到该现象。
- 代码依据：`src/app/src/js_orchestration/runtime.rs` 的 `close` 发送 shutdown 后调用 `kill_worker`；`kill_worker` 对直接子进程组发 SIGKILL，并只等待 `Child`。`git blame` 显示该函数来自 `f4a370d feat(034)`，在 `57d5408` 基线已存在；A 的未提交差异修改了同文件启动确认部分，未修改该函数。
- 合理推断：组杀和 bwrap 的父死亡语义使直接监督者先于后代结束，形成被重新托管的 worker。`/proc` 采样没有记录信号发送系统调用，且容器未安装 strace；不能把具体信号发送链说成已被本轮直接跟踪。
- Linux 文档：[waitpid(2)](https://man7.org/linux/man-pages/man2/waitpid.2.html) 描述未被 wait 的退出子进程成为僵尸；[pid_namespaces(7)](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html) 描述孤儿被 PID 命名空间 init 或较近的 subreaper 接管。上游 [bubblewrap 选项说明](https://github.com/containers/bubblewrap/blob/main/bwrap.xml) 说明 `--die-with-parent` 会向沙箱子进程发送 SIGKILL；上游 main 文档不替代当前 0.9.0 的运行证据。
- 当前只复现一条 JS worker 正常完成路径，不能证明历史 1,327 个进程全部来自这一处；取消、超时及其他调用方仍未验证。目标生产函数属于既有 034 实现，修改它超出本轮仅修复已授权 038 代码或测试的边界；未改产品代码或测试，也未修改 PID 1、容器配置。
- 若后续授权修复，应先在受控子收割器下建立能检测孤儿被接管的回归，再分别验证正常完成和明确终止；修复必须保持 034 的终止与回收契约及现有沙箱安全边界。仅为正常完成增加等待不能证明强制终止分支也已解决。

## 强制终止回归与方案边界（后续授权）

用户已明确授权本轮修改源自 034 的 worker 终止和回收逻辑。新增 `src/app/tests/js_orchestration.rs::sandbox_worker_terminate_reaps_payload`：测试在独立辅助进程中启动真实沙箱 worker，记录载荷 PID 和 starttime；产品 `terminate` 返回后，先检查原载荷是否仍存在，随后才由辅助进程负责失败现场清理。通过条件不依赖辅助进程的 `waitpid`。

工作目录 `/workspace`。在 `target/038-validation/reap-probe.py` 的进程控制、子收割和 500 门禁下，离线执行 `cargo test --locked --offline --test js_orchestration --no-run` 编译单目标，退出 0；然后只执行新二进制的 `sandbox_worker_terminate_reaps_payload --exact --nocapture`，旧实现失败，测试退出 101：`产品终止后 worker 378 仍在进程表中`。监督器记录 bwrap 377 已退出、载荷 378 被接管并成为 Z；辅助进程随后清理该载荷。测试结束并等待 2 秒后，6 个可见进程、0 僵尸、36 线程、36 个 cgroup PID 任务，已跟踪进程无残留。原始记录：

- `target/038-validation/reap-probe-20261105-rust-regression-old.log`。
- `target/038-validation/reap-probe-test-20261105-rust-regression-old.log`。

复现后只调整了辅助进程超时清理：去掉无界等待，并将终止结果断言移到失败现场清理之后。该清理改动尚未重新编译或执行；旧实现红灯证据对应调整前的测试版本，测试的产品回收断言未改变。

已核对约束：034 要求关闭后终止并回收 worker、沿宿主进程组处理，并保护独立的 016 进程；033 与 038 要求保留 PID 命名空间隔离和失败不放行。当前本地内核的 `pidfd_open` 和对自身的零信号 `pidfd_send_signal` 可用；该结果不能代表所有既有支持环境。Linux 上 `pidfd_open` 自 5.3 起可用（[手册](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)）。

尚未实施产品修复：对载荷使用 pidfd 而保留 bwrap 等待有望避免 PID 复用，但尚需解决不支持或被 seccomp 拒绝 pidfd、监督进程异常以及 034 宿主进程组终止约束。直接退回旧组杀会保留僵尸缺陷；单独等待正常退出不能覆盖强制终止。尚未证实这些约束必然要求修改全局等待归属、现有契约或容器配置。产品代码未改，回归测试当前仍为预期失败状态，不应视为已修复。

补充：033 规格的 B2 部署前提为 Linux 内核 ≥5.11，覆盖 pidfd_open 的 5.3 版本下限；当前 033 seccomp 文件也列有 pidfd_open 和 pidfd_send_signal。因此单凭内核最低版本不能认定 pidfd 与现有部署前提冲突。其他部署的 seccomp 权限、pidfd 捕获与进程退出竞争、监督进程异常时的有界失败报告，以及宿主进程组契约如何保持，仍须验证。Linux 手册区分 pidfd 可读（进程已退出，可仍为僵尸）与挂断（已回收）：[pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)。

当前测试版本的有界清理调整已在同一受控监督器中重新编译，并重新执行一次单用例：旧实现仍按回收断言失败，测试进程退出 101，载荷 PID 553 仍在进程表中；辅助进程清理后，监督器等待 2 秒，6 个可见进程、0 僵尸、36 线程、36 个 cgroup PID 任务，已跟踪进程无残留。记录为 `target/038-validation/reap-probe-20261105-rust-regression-old2.log` 和 `target/038-validation/reap-probe-test-20261105-rust-regression-old2.log`。两次受控构建和单用例检查的最终日志未发现 EAGAIN 报文。

## worker 回收修复候选与定向结果（2026-11-05 后续）

本节更新上文「尚未实施产品修复」的状态；上文保留当时的事实。工作目录 `/workspace`，未重跑 L3。约束来源：034 规格 §1.4–1.7 要求关闭后终止并回收 worker、保留宿主进程组清理且不杀独立 016 会话；033 的 PID 命名空间隔离和 038 的确认闸仍生效。当前候选不改变全局子进程等待归属或容器配置。

候选实现：启动沙箱前检查 pidfd 支持；在载荷放行前取得 pidfd，核对其 PID 命名空间和命名空间内 PID 1，再通过该句柄放行。关闭时对载荷句柄发送 SIGKILL，等待 pidfd 的 POLLHUP（已被 bwrap 回收），之后终止原宿主进程组并有界等待直接子进程。回收未确认时返回 `WORKER_REAP_FAILED`，不提交成功的 `cell_closed` 事件。普通模式仍使用进程组终止。Linux 手册说明 [pidfd_open](https://man7.org/linux/man-pages/man2/pidfd_open.2.html) 的 POLLIN 与 POLLHUP 差异；[PID 命名空间](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html)规定 PID 1 退出时内核终止该命名空间的其余进程。具体历史信号先后仍未直接跟踪。

执行器与上文相同，启动前先停止子进程并取得 pidfd、进程组控制；每例有独立临时 HOME、45 秒内部上限及 500 门禁，构建上限 240 秒。命令入口分别为 `python3 target/038-validation/reap-probe.py build9`（内部执行 `cargo test --locked --offline --test js_orchestration --no-run`）和 `python3 target/038-validation/reap-probe.py <下表模式>`（单个 `--exact --nocapture` 用例），外层 `timeout -k 15 275` 或 `timeout -k 15 75`。各模式的监督日志和测试输出分别为 `target/038-validation/reap-probe-20261105-<模式>.log`、`target/038-validation/reap-probe-test-20261105-<模式>.log`。构建和下表用例退出均为 0；没有运行整个测试目标。

| 模式 | 检查 |
|---|---|
| `normal1`、`normal2`、`plain-final` | 真实沙箱普通结束，修复后重复一次；另以未设置沙箱开关的普通模式运行。 |
| `rust-regression-new`、`repeat2`、`repeat3` | 强制终止回归，产品返回后、辅助进程失败清理前确认载荷不存在。与上文旧实现 `rust-regression-old2` 退出 101 对照；`repeat3` 使用最后一次产品改动后的二进制。 |
| `cancel-inflight` | 已启动的叶子收到取消，cell 返回 terminated。 |
| `timeout-isolation` | 观察时限到期仍为 running；终止一个 cell 后另一个 cell 完成。此项不等于 034 的 CPU 或 120 秒墙钟超限验收。 |
| `pidfd-denied`、`capture-denied`、`kill-denied` | 进程级 seccomp 分别拒绝自检中的 pidfd_open/pidfd_send_signal、载荷身份捕获、关闭时 pidfd SIGKILL；实际产品路径拒绝创建或返回失败。 |
| `early1`、`supervisor-dies` | 放行后的载荷提前退出并被 bwrap 回收；外力杀死监督进程时产品返回失败。未覆盖身份捕获前载荷自然退出的全部时序。 |
| `terminate1`、`concurrent1`、`cancel1`、`c13`、`persist-failure` | 既有终止、并发叶子、未等待叶子失败、016 句柄存续及创建事件保存失败的定向检查。 |

受控用例的监督器快照起始及等待 2 秒后均为 7 个可见进程、0 个僵尸、37 个线程及 37 个 cgroup PID 任务；其中包含监督器自身。`rust-regression-new` 较早一轮为 6/0/36/36。已跟踪进程最终均无残留；测试进程自身在父进程 wait 前可能短暂显示 Z，不是载荷残留。进程、线程和 cgroup 任务口径不同。宿主 Docker stats 在主要用例结束后显示 36 个 PID 任务，初次检查为 35；两次快照相差一个任务，不能由此确定其归属或构成累积。

异常边界不能当作产品回收通过：`supervisor-dies`、`kill-denied`、`capture-denied` 的载荷分别被诊断子收割器接管并 wait，产品只证明未误报成功；直接监督者已死亡或无法取得安全句柄时，仍存在产品自身不能证明回收的缺口。`c13` 中独立 016 进程的 `sh` 也由诊断器接管并 wait；该发现未归因或在本轮修复。未观察到随本轮有限次数普通 worker 回归累积，但不能据此判定所有进程路径或历史 1,327 个进程的原因。

测试二进制的最终构建为 `build9`；`capture-denied`、`persist-failure`、`normal2`、`repeat3`、`plain-final` 在此构建后运行。其他用例使用此前构建，后续产品改动限于创建事件保存失败时的回收结果文字，不能将其当成最终二进制重跑的证据。本机未安装 `ce-simplify-code` 技能；参照其上游说明对本轮代码做了局部检查，不能记为已完成该技能流程。身份捕获时目标退出的竞态、拒绝后的产品自身清理，以及 CPU/墙钟超时等仍需补证；本节不宣称 038 或所有异常路径已完成。
