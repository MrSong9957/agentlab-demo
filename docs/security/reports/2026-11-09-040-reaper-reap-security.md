# 安全攻防测试报告：040 沙箱 bwrap ns-PID-1 reaper 收割

- 日期：2026-11-09
- 范围：分支 `feat/040-sandbox-bwrap-reaper-reap`（HEAD `4994684`，基线 `b85c4ee`＝合并后 main），diff `b85c4ee..4994684`
- 被测改动：`src/app/src/sandbox.rs` 新增 `ensure_child_subreaper`（PR_SET_CHILD_SUBREAPER 进程级一次）与 `reap_adopted_bwrap_reapers`（签名定向收割：PPid=本进程＋comm=bwrap＋NSpid 末级=1），收割点插入 probe/016 monitor/MCP/apply_patch/view_image/JS runtime 五处 try_wait 后
- 结论：**通过（可合并）。无阻塞级问题；1 项建议级加固，3 项记录级观察。**

---

## 1. 范围与方法

- 静态：`git diff b85c4ee..HEAD` 全量走查＋收割代码逐行审查（解析、状态机、syscall 面、panic 面、调用点语义）。
- 动态：既有安全回归（任务指定两组）＋自建攻防 harness（`/tmp/attack040`，**不入仓库**）以 path 依赖直连真实 `terminal_chat::sandbox::reap_adopted_bwrap_reapers()`，对**真实产品代码**发起 9 组攻击场景（a1/a2/a3/c1/d1/e1/f1/f2/g1/i1），全部在本容器实测（bwrap 0.9.0、内核 6.18 WSL2、pid_max=4194304、uid 1000）。
- harness 技术：`prctl(PR_SET_CHILD_SUBREAPER)`（与产品同 syscall）＋ `unshare(CLONE_NEWUSER|CLONE_NEWPID)`（单次组合调用；实测两步分开调用会被内核拒绝）伪造/构造各类签名候选进程，再由收割函数实弹处置，宿主侧轮询 /proc 取证。
- 构建：全部 cargo 命令串行执行，`CARGO_TARGET_DIR=/tmp/038-attack06-target`，工作目录 `src/app`。生产代码零修改（`git status` 干净，仅新增本报告文档）。

## 2. 不变式逐条结论

### 不变式 1：收割签名三重精确，绝不误收 — **通过**

证据（均为实弹 harness 结果，目标＝真实产品函数）：

| 场景 | 攻击构造 | 结果 |
|---|---|---|
| a1 | 子进程 `prctl(PR_SET_NAME,"bwrap")` 后退出成 Z（宿主 ns，NSpid 单级≠1） | 收割未触碰——harness 随后 `waitpid` 成功（rc=pid），wait 责任未被偷走 |
| a2 | 同上但 S 态长眠、无子进程 | 未被 SIGKILL（事后仍存活）；收割走快速路径返回（101ms，未进 1s 等待窗口） |
| a3 | comm 括号混淆 5 种（`b)wrap`／`bwrap) (x`／`bwrap(`／`)bwrap`／`bwrap )`）全部成 Z | 5/5 未被触碰，全部由 harness 亲自回收；收割耗时 1ms（签名零误匹配）——`rsplit_once(") ")`＋首 `(`-尾 `)` 提取＋全等比较的解析对括号注入免疫 |
| c1 | **全签名伪造**：真 ns-PID-1＋comm=bwrap＋被收养（Z 态） | 被产品收割正确回收（事后 harness `waitpid`＝ECHILD），符合设计——伪造者只能让自己的进程被回收 |
| f2 | 非 bwrap 收养孙辈孤儿（Z 态，comm 甚至与宿主同名） | 未被触碰，harness `waitpid` rc=pid、退出码 42 完整 |
| g1 | 100 轮压力：每轮 1 个全签名 Z＋1 个伪 comm Z | 100/100 全签名被收割（事后全 ECHILD）；100/100 伪 comm 原样保留由 harness 回收 |
| 仓库测试 | `reap_adopted_bwrap_reapers_leaves_non_bwrap_children_alone` | ok（非 bwrap Z 收养子进程 wait 责任不被偷走） |

结构性论证：签名三要素中仅 comm 可被攻击者自由伪造；PPid=本进程是内核维护的收养事实（无法伪造），NSpid 末级=1 是内核维护的 namespace 身份（无法伪造）。因此「仅伪造 comm」（bwrap 监督进程、JS worker、016 会话子进程均可走此路径）被 NSpid 一票否决；「全签名」要求同时是真 ns-PID-1 且被本进程收养——见不变式 5 分析，其后果被严格限定在攻击者自身进程上。

### 不变式 2：subreaper 只改收养归属，沙箱隔离语义全部未动 — **通过**

- diff 走查：`git diff b85c4ee..HEAD --stat` 显示生产代码变更仅为 `sandbox.rs` 新增三函数＋`probe_bwrap_at` 两行插入，及 6 个收割点单行调用＋测试文件。`src/app/src/sandbox/` 下仅 `tests.rs`（+41，纯测试）。seccomp 清单（BPF 构造）、clone 掩码（clone3 双层守卫）、bwrap argv（`--die-with-parent`/unshare 旗标组）、namespace 拓扑、D2 sweep（仍跳过 reaper PID 1）**逐字节未动**。
- 行为实测 f1：subreaper 登记后，在世子进程存活期间收割调用不产生任何影响（S 态不变），`waitpid` 语义原样（退出码 7 完整回传）。
- 实弹回归：`cargo test --test security_attack_031` 7/7 通过（含 attack06 seccomp 实弹：clone3 拦截＋gcc 工具链可用）、`cargo test --lib seccomp` 3 用例通过（清单锚＋fd roundtrip＋clone3/openat2 双层守卫）——隔离面无回归。
- 真实启动路径回归：`sandbox_launch_leaks_no_bwrap_reaper_zombie` ok，`REAPER-BASELINE-CLEAN 16->16`（3 次真实 bwrap 启动零僵尸累积，且日志 `DRAIN-TLM ensure_subreaper pid=17369 rc=0` 证明产品自己的 subreaper 登记在真实路径生效）。

### 不变式 3：无子进程孤儿的 SIGKILL 只及于空 PID namespace — **通过**

| 场景 | 构造 | 结果 |
|---|---|---|
| d1 | 全签名 S 态、**无子进程** ns-PID-1（空 ns） | 有界窗口 ~1.25s 耗尽后 SIGKILL 发出、目标消失（若 kill 后即时 WNOHANG 错过死亡过渡态，Z 残留至下一收割点自愈回收）——仅销毁一个空命名空间 |
| e1 | 全签名 S 态、**有子进程** ns-PID-1（ns 内还有存活孙进程） | **未被 SIGKILL**（1.16s 窗口后仍 S 态），登记 STUCK；第二次收割调用 0ms（巡检不重复耗窗）；随后外力 SIGKILL 该 reaper：内核 zap 语义亲测成立——垂死 ns-PID-1 亲自 SIGKILL 并 wait 收割全部 ns 成员（孙进程直接消失，**不会以 Z 态落到本进程**），Z 态 reaper 自身由下一次收割点 0ms 回收，容器侧零残留 |
| 攻击者可控性分析 | 见 §3 场景 5 | 匹配「S 态＋无子＋PPid=本进程＋NSpid 末级=1」的进程必然是某个空 PID namespace 的 1 号进程且为本进程直系收养子——SIGKILL 它只销毁空 ns，不可能波及宿主其他进程（宿主进程 NSpid 单级≠1，永不匹配） |

### 不变式 4：收割不引入新失败面 — **通过**

代码走查＋定向实测：

- /proc 扫描失败：`read_dir(...).ok()?` → `None` → 收割函数直接返回（调用方无感）；单条目 stat/status 读取失败逐条 `continue`。实测 i1：零匹配时单次收割 1ms（纯扫描开销，对每次收割点的常驻成本可忽略）。
- waitpid 竞态：`rc==pid`／`rc==0`／`rc==-1` 三态均有明确语义，ECHILD（并行收割点抢先）按已处理——g1 中 8 线程并发收割 8 个同签名目标，2ms 全部回收、零重复、零死锁、零 panic（STUCK Mutex 中毒经 `unwrap_or_else(|e| e.into_inner())` 兜底）。
- panic 面：无 `unwrap` 于外部数据（状态字段 `unwrap_or("?")`、集合操作无越权索引）；g1 的 100 轮伪造＋并发段全程无 panic。
- 调用方返回值：6 个收割点全部为语句级 fire-and-forget 调用（置于调用方已取得等待结果之后/错误路径内），不改变任何返回值与错误语义。
- 已知微小间隙（非漏洞）：SIGKILL 后即时 `waitpid(WNOHANG)` 可能返回 0（目标处于 R 态死亡过渡中，d1/d3 实测复现），Z 残留至下一收割点巡检自愈——无泄漏，仅一个收割点间隔的自愈延迟。

### 不变式 5：攻击者面（提权/逃逸/误杀宿主进程）— **通过**

攻击者（恶意工作区驱动沙箱内载荷代码）的三条可能路径，全部实测或论证封死：

1. **伪造 comm=bwrap 诱使误杀**（实测 a1/a2/a3/f2/g1）：必须同时满足 PPid=本进程与 NSpid 末级=1。在世子进程（正在被 016/JS 组件 wait 的 bwrap 监督进程等）PPid≠本进程（内核事实）；宿主 ns 普通进程 NSpid 单级≠1。仅 comm 伪造在全部 100＋5＋2 个样本中零命中。
2. **全签名伪造**（实测 c1/d1/e1）：载荷若能在沙箱内 `unshare(userns+pidns)`＋fork（本容器可行）并让中间父链死亡，可造出「被产品收养的真 ns-PID-1＋comm=bwrap」。后果实测为：Z 态→被产品收割（攻击者自己的进程被回收）；S 态无子→被 SIGKILL（攻击者自己的空 ns 被销毁）；S 态有子→仅登记。**无提权、无逃逸、无误杀**；唯一代价是每个新伪造 pid 在首个收割点消耗一次 ~1.2s 有界窗口（持续存在者被 STUCK 登记后降为 0ms 巡检）——延迟放大被有界窗口＋STUCK 登记双重封顶（记录级，见 §4-F2）。
3. **借 SIGKILL 分支击杀宿主其他进程**（论证＋d1/e1 实测）：SIGKILL 目标必须满足收养关系（内核事实）。目标为 unreaped 僵尸期间其 PID 被内核保留不可复用；跨 uid `kill` 受 EPERM 限权；本机 pid_max=4194304 循环分配使「并行收割点刚释放 pid＋立即复用＋命中」的 TOCTOU 窗口可忽略（见 §4-F1 建议）。另实测确认：容器存量 PPid=1 的 bwrap 僵尸/卡死（dbg 扫描可见 10+ 个）对本进程收割永久不可见，不受任何影响。

## 3. 攻防实测命令与结果（harness：/tmp/attack040，不入仓库）

构建：`cd /tmp/attack040 && CARGO_TARGET_DIR=/tmp/038-attack06-target cargo build --offline`（path 依赖直连 `/tmp/038-start-gate-YcOITR/repo/src/app` 的 `terminal-chat`，与仓库同源 Cargo.lock＋crossterm patch）。

运行：`/tmp/038-attack06-target/debug/attack040 <scenario>`，每场景独立进程。

| 场景 | 不变式 | 关键断言 | 实测指标 | 结果 |
|---|---|---|---|---|
| a1 | 1/5 | 伪 comm Z 不被收、harness 能收 | `harness_waitpid_rc=<pid>` | PASS |
| a2 | 1/5 | 伪 comm S 不被杀、快速返回 | `post_state=S reap_ms=101` | PASS |
| a3 | 1/5 | 5 种 comm 括号混淆零误匹配 | 5/5 回收权保留，`reap_ms=1` | PASS |
| c1 | 1/5 | 全签名 Z 被正确收割 | `adopted_matched_z=[pid]`，事后 ECHILD | PASS |
| d1 | 3 | 全签名 S 无子被 SIGKILL 且消失 | `reap_ms=1249`，轮询消失 | PASS |
| e1 | 3 | 有子 reaper 不被杀＋STUCK 快速巡检＋内核 zap 无残留 | `after1=S/S`，`reap2_ms=0`，`E=G(内核收走)`，`leftover=0` | PASS |
| f1 | 2 | 在世子进程 wait 语义不变 | `mid_state=S`，`exit_code=7` | PASS |
| f2 | 1/2 | 非 bwrap 收养孤儿不被触碰 | `rc=<pid>`，`exit_code_raw=42` | PASS |
| g1 | 1/4 | 100 轮压力＋8 线程并发无 panic/死锁 | `matched 100/100`，`mismatched 100/100 保留`，`concurrent_ms=2 leftover=0` | PASS |
| i1 | 4 | 零匹配常驻开销 | `worst_reap_ms=1` | PASS |

## 4. 发现的问题（分级）

### 阻塞

无。

### 建议

- **F1（建议，加固项）SIGKILL 分支按 pid 发信号存在理论 TOCTOU 窗口**：`libc::kill(pid, SIGKILL)` 与并行收割点释放同一 pid 之间存在极窄竞态——pid 被释放并复用后信号可能落到同 uid 的无关进程。现有缓解：目标为未收割子进程期间内核保留其 pid（僵尸不可复用）；跨 uid kill 受 EPERM；pid_max=4194304 循环分配使即时复用概率可忽略。加固方向：`pidfd_open(pid)`＋`pidfd_send_signal`（信号跟随进程而非 pid 号），或 kill 前后以 /proc 复核收养关系（不彻底）。不阻塞合并。

### 记录

- **F2（记录）伪造签名的一次性延迟放大**：可在沙箱内 unshare+fork 的载荷每制造一个新收养 S 态全签名候选，可令首个收割点消耗 ~1.2s 有界窗口；持续存在者被 STUCK 登记后降为 0ms。窗口受「候选必须在收割点之前被收养」与单次 ~1.2s 上限双重约束，无资源泄漏后果。
- **F3（记录）kill 后自愈间隙**：SIGKILL 后即时 WNOHANG 可能错过死亡过渡（实测复现），Z 残留一个收割点间隔后由巡检自愈，无泄漏。
- **F4（记录）遥测行**：`DRAIN-TLM ensure_subreaper pid=… rc=…` 含本进程 pid，无敏感性；与其他 DRAIN-TLM 遥测同风格。

### 正面安全观察（供评审参考）

- NSpid 末级=1 与 PPid=本进程均为内核维护事实，签名中唯一可伪造字段 comm 已被双重 kernel-truth 闸门隔离（防御深度经 a3/g1 与仓库负向测试实证）。
- 内核 `zap_pid_ns_processes` 语义（e1 实测）：外力击杀 ns-PID-1 reaper 时其 ns 成员被垂死者亲自收割，**不会**以 Z 态收养到产品进程——「多级 NSpid Z 僵尸被收割函数跳过」不构成新泄漏形态。
- 容器存量 PPid=1 僵尸/卡死对收割永久不可见（PPid 闸门），实测零触碰。

## 5. 既有安全回归（任务指定，全部通过）

| 命令（在 src/app，串行，CARGO_TARGET_DIR=/tmp/038-attack06-target） | 结果 |
|---|---|
| `cargo test --test security_attack_031` | 7/7 passed（attack01 参数伪造 / attack02 路径攻击 / attack03 symlink 逃逸 / attack04 快照投毒 / attack05 授权原子性 / attack06 seccomp 实弹 / attack07 setsid 守护回收），1.54s |
| `cargo test --lib seccomp` | 3/3 passed（seccomp_filter_shape_valid / seccomp_fd_roundtrip / seccomp_filter_blocks_clone3_and_preserves_openat2）；加上 attack06 实弹即任务口径的 4 个 seccomp 用例，全通过 |

补充回归（本改动直接相关面）：`cargo test --lib reap_adopted`（负向签名保护测试）ok；`cargo test --test request_permissions_sandbox sandbox_launch_leaks_no_bwrap_reaper_zombie -- --nocapture` ok（3 次真实 bwrap 启动，`REAPER-BASELINE-CLEAN 16->16` 零累积）。

## 6. 未覆盖范围与理由

- **跨 uid pid 复用误杀**（F1）未做并发实测：需要精确编排「并行收割点释放 pid→内核立即复用同一 pid」的微秒级竞态，harness 无法确定性构造；以内核 pid 分配机制（循环位图、pid_max=4M）与 EPERM 限权做论证封顶，并给出 pidfd 加固建议。
- **Windows/macOS 路径**：收割代码整体 `cfg(target_os = "linux")`，其他平台无此泄漏形态也无此代码，不在攻击面内。
- **载荷在沙箱内 unshare+fork 的可行性受 seccomp 清单实际约束程度**：harness 伪造是在宿主侧完成的（模拟已被收养后的状态）；沙箱内 seccomp 是否放行 unshare/clone 由 attack06 实弹与 031 套件独立把守，本次不重复验证（即便放行，后果已被 c1/d1/e1 实测限定为自伤＋有界延迟）。
- **500 门禁全量复跑**：验证记录（2026-11-08）已有完整门禁证据（16/16、peak 223、零累积、负向对照 exit=101），本轮以单测试真实启动回归（REAPER-BASELINE-CLEAN 16->16）确认修复态仍成立，未重复全量门禁。

## 7. 结论

**通过（可合并）。** 五条安全不变式全部成立：签名三重精确（含括号混淆与 105 个伪造样本零误触）、subreaper 语义与沙箱隔离零改动（静态 diff＋行为实测）、SIGKILL 严格限定于空 ns 的收养 ns-PID-1（有子者绝不杀，内核 zap 语义消除残僵尸）、收割函数无新失败面（100 轮压力＋8 线程并发零 panic/死锁，1ms 常驻开销）、攻击者面（伪造 comm／伪造全签名／借刀杀宿主进程）全部封死且后果上界为攻击者自身进程＋一次性 ~1.2s 有界延迟。遗留 1 项建议级加固（pidfd 收敛 SIGKILL TOCTOU）与 3 项记录级观察，均不阻塞。
