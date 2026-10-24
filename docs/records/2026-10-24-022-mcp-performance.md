# 022 MCP 性能基线、瓶颈诊断与优化准备

- 日期：2026-10-24。
- 阶段：方案 A 已实施、验证并获批准交付本地 main（2026-10-24；第三轮最终语义核验后交付）。基线与测法同前文本节（未改动）。
- 分支：`perf/022-mcp-performance`（HEAD＝`4fbf052bdabbb54a3e637906ce32ac48c56b8e72`，即优化前基线）；合并目标本地 main。工作区仅含交付时既有 12 项无关遗留（8 删除＋4 未跟踪，未触碰）。
- 依据：[022 规格](../specs/022-mcp-tools.md)（含本轮新增 §6 性能任务节）；[实施记录](2026-10-24-022-mcp-tools-implementation.md)。

## 1. 测量方法（看结果前固定）

| 项 | 约定 |
|---|---|
| 环境 | 同机同会话：13th Gen Intel i5-13500H（16 逻辑核）、15.5 GiB 内存、Linux 容器；测量期背景负载 1m avg ≈ 5（全档一致，A/B 交替抵偿漂移） |
| 构建 | 两版本各在隔离 git worktree 以 `cargo build --release --locked --offline --target-dir <专用目录>` 构建；二进制 SHA-256：old(f662cf6)=`473613efb1934fd28f66ff2e0038333a74a69de275be9543ecfd3a52ca14b380`，new(4fbf052)=`398e670c54a69356f525290a227c335e5808192bf477ac0cdb26f5506899b261`，MCP 基线驱动 example=`7b3190d9efc1216b950fde9cd55133743f332b87249c95e1b53847c29a94ea2b` |
| 计时起止 | 接入开销：CLI 进程启动→进程退出（`time.monotonic`）；MCP 场景：管理器 API 调用前→返回（进程内 `Instant`） |
| 预热/样本/轮次 | 每场景预热 2（MCP 单次性场景预热 1）；样本 N=30（接入开销）/N=20（MCP 场景；单次性场景 N=1~5 如表注）；A/B 交替（old,new,old,new…） |
| 失败与负载干扰 | 失败＝非零退出/30s 超时→样本作废并补测；同侧失败 >3 中止场景（本轮全部 0 失败）；不挑轮——全部原始样本落盘，统计含极值 |
| 内存 | 主进程与 MCP 子进程分别取 `/proc/<pid>/status` `VmHWM`（**峰值** RSS，10~20ms 轮询）；「当前内存」未单独记录（VmHWM 为上界）——本轮所有内存数字均为峰值口径 |
| 统计 | p50/p95/min/max/n；噪声参照＝startup 两侧 p50 差 0.15ms、chat(new) p95−p50=0.61ms（目标须显著超过该量级） |
| 夹具 | 本地 stdio `mcp_server.py`（--tools/--schema-bytes/--scenario/--delay-ms）与本地 mock 模型 `mock_model.py`（固定终文响应）；凭证为假值 `perf-fake-key`（env 注入＋预置 origin 授权） |
| 停止条件 | 目录梯度上限 tools=512、schema=4 KiB（128 上限内 ×8 MiB 单项内，避免触顶非目标行为）；单场景墙钟 120s；均未触达（wall 见原始 meta） |

原始结果（全部样本，不挑轮）：`/tmp/perf-harness/results-overhead/*.txt`、`/tmp/perf-harness/results-mcp/*.txt`（每场景 times＋rss＋meta；临时目录，关键数字已摘录入本表；复跑脚本 `/tmp/perf-harness/bench_overhead.py`、`run_mcp_bench.py`，驱动源 `/tmp/perf-4fbf052/src/app/examples/mcp_bench.rs`——隔离 worktree 内临时 example，未入库）。

## 2. 接入开销基线（f662cf6 vs 4fbf052，未配置 MCP）

| 场景 | old p50/p95 | new p50/p95 | Δp50 | old/new RSS 中位（峰值） |
|---|---|---|---|---|
| startup（EOF 即退） | 11.0／11.5 ms | 11.1／11.4 ms | +0.1 ms | 1.3／1.2 MiB |
| chat_turn（一个普通用户轮，mock 模型） | 11.0／11.6 ms | 64.5／65.1 ms | **+53.5 ms** | 1.3／7.4 MiB |

- 逐对差值（chat−startup）：old p50=0.0ms；new p50=53.5ms（p95 54.0、min 52.7、max 54.1，n=30）——差值全部落在用户轮内，非启动路径。
- 内存 +6.1 MiB 为 rmcp 及传递依赖静态链接的进程尺寸增量（无 MCP 配置时不创建 runtime 任务；startup RSS 两版本相当）。
- **结论：接入对无 MCP 用户的主要影响是每个用户轮固定 +53.5ms 与二进制尺寸增大；来源见 §4 瓶颈①。**

## 3. MCP 优化前基线（4fbf052，目录梯度）

稳态快照（每轮 prepare）：

| 梯度 | p50／p95 (ms) | 主进程峰值 RSS | 子进程峰值 RSS |
|---|---|---|---|
| t8-s256 | 51.3／51.5 | 6.5 MiB | 11.9 MiB |
| t64-s256 | 51.8／52.3 | 7.3 MiB | 11.9 MiB |
| t512-s256 | 54.4／55.2 | 14.1 MiB | 12.8 MiB |
| t8-s4096 | 51.4／51.4 | 6.6 MiB | 11.9 MiB |
| t64-s4096 | 51.8／52.2 | 8.6 MiB | 12.4 MiB |
| t512-s4096 | 55.6／56.9 | 23.5 MiB | 16.5 MiB |

其余场景（n=1~10 如注）：

| 场景 | 梯度 | p50 (ms) | 备注 |
|---|---|---|---|
| discover_first（首次 prepare＝加载＋连接＋完整发现） | t8-s256 | 76.4 | n=1；含子进程 spawn |
| | t64-s256 | 70.1 | n=1 |
| | t512-s256 | 84.2 | n=1 |
| | t8-s4096 | 72.4 | n=1 |
| | t64-s4096 | 78.7 | n=1 |
| | t512-s4096 | 98.1 | n=1 |
| relist_after_notify（通知后下一轮重列） | t64-s256 | 52.9 | n=5 |
| | t512-s4096 | 72.1 | n=5 |
| call_rtt（单次调用往返，快响应） | 全部 6 梯度 | 0.1~0.2 | n=20/梯度 |
| cancel_inflight（调用中取消返回耗时；服务器延迟 200ms、30ms 处取消） | t64-s256 | 51.3 | n=10 |
| close（manager.close 有界关闭） | t64-s256 | 3.0 | n=1 |
| | t512-s4096 | 4.2 | n=1 |

- 子进程内存为 python 夹具自身（12~16.5 MiB，随目录规模缓增）；测量失败数全部为 0；无外推估算。
- **测法限制**：①discover 为单样本（连接生命周期限制，n=1 属方法边界，已如实标注）；②`child_peak_kib=0` 的行＝子进程在首次采样前退出（close/call 场景进程已结束），非零值仅见于长生命周期场景；③内存为 VmHWM 峰值口径，未单独记录稳态当前值；④MCP 场景经管理器 API 测量（不含 CLI 进程整体启动），与 §2 口径不同，不可跨表相减。

## 4. 瓶颈诊断（证据）与优化方案（候选，待确认后实施）

### 瓶颈①（主要）：`prepare_for_turn` 无条件 50ms settle

- 证据链：`manager.rs:184-187` 每轮 `sleep(50ms)`（注释：留出在途通知处理间隙）；§2 中无 MCP 配置的 chat 轮 +53.5ms 与之精确吻合；§3 全部稳态快照 p50 ≈ 51~56ms（减去 50ms 后实际工作仅 1.3~5.6ms，随目录规模缓增）；两个宿主入口（`main.rs:808`、`tui_host.rs:519`）都经过该路径——**影响所有用户每轮固定延迟**。
- 该 settle 是重列通知计数（stale_epoch）竞态的保守修补（实施记录 §3.2），现行正确性依赖「列出前记录基准」，settle 只兜底调用期间在途通知的可见性。

### 瓶颈②（次要）：取消等待的 50ms 轮询粒度

- 证据：`connection.rs:452` `wait_cancel_flag` 以 50ms 轮询同步 AtomicBool；cancel_inflight p50=51.3ms（取消点 30ms 后→平均多等 ~21ms），与轮询粒度吻合。

### 方案 A（建议实施）：消除无条件 settle——只对「存在活动连接且上轮有调用」时等待

- 做法：`prepare_for_turn` 中当 `inner.servers` 为空（无 MCP 配置）或全部连接无近期调用时跳过 settle；或把 settle 移到「确认本轮回列需要读取 epoch」的分支内（仅 needs_relist 判定前、且存在活动连接时）。
- 目标：无 MCP 配置 chat 轮 Δ 从 +53.5ms 降到 ≤ +1ms（噪声参照 0.15~0.61ms，目标超噪声 80×）；稳态快照 p50 从 ~51ms 降到 ~2ms（t512-s4096 ~6ms）。
- 允许回归范围：有活动连接且上轮发生调用的场景仍可保留 ≤50ms settle（语义不变）；不得改变 stale_epoch 基准时序（列出前记录）——`a13_notification_during_relist_keeps_stale_marker` 必须保持通过。
- 代价/风险：通知可见性从「每轮固定兜底」变为「条件兜底」，需以现有 a13 两用例＋变异 M7b 复验；实现预计 <20 行。
- 验收：§2/§3 同口径复测——chat_turn(new) p50 回到 ≤12ms；steady p50 ≤8ms（t512-s4096）；discover/relist 相应各 −50ms；全量回归＋安全相关用例（取消/断连/重列）不回归。

### 方案 B（可选，独立小改）：取消等待改 10ms 粒度（或事件化）

- 做法：`wait_cancel_flag` 轮询 50ms→10ms（保持 AtomicBool 桥接不变）。
- 目标：cancel_inflight p50 从 ~51ms 降到 ~12ms。代价：空转轮询频率×5（单请求生命周期内，可忽略）。验收：a11 两用例＋A18 在途退出用例不变红。
- 若只允许 1 项，选方案 A（影响面与收益均大一个数量级）。

### 明确不做（本轮边界）

- 新增目录资源上限（行为变化，需单独确认——实施记录 §5 已记录目录累计内存无上限的现状与量级：512×4KiB 目录实测主进程峰值 23.5MiB）。
- 任何可能触碰 8／32／128 预算、取消状态未知、恢复不重放、凭证边界的改动。

## 5. 复跑指南

```bash
# 接入开销（A/B）
python3 /tmp/perf-harness/bench_overhead.py
# MCP 梯度基线
python3 /tmp/perf-harness/run_mcp_bench.py
# 成分参照：python spawn+init ≈ 18~19ms（夹具固有，非本项目开销）
```

临时件说明：脚本与原始结果位于 `/tmp/perf-harness`、隔离 worktree `/tmp/perf-f662cf6`、`/tmp/perf-4fbf052`（构建目录 `/tmp/perf-target-old|new`）；`mcp_bench.rs` example 位于隔离 worktree，**未加入仓库**。本轮未改生产实现、未提交。

## 6. 方案 A 实施与同口径复测（2026-10-24 第二轮）

### 实现

- `connection.rs`：`ConnectionState.notify_grace: AtomicBool`——`call_tool_once` 发起时置位（调用后可能有在途目录变化通知；通知计数本身原子可见，无需等待）。
- `manager.rs::prepare_for_turn`：原无条件 50ms settle 改条件化——仅当存在活动连接且其 `notify_grace` 置位时等待 50ms（计入轮准备耗时），随后清除该位。无 MCP 配置、空闲连接、纯主动通知均零等待。`stale_epoch` 基准时序（列出前取得）不变；关闭后不自动复活不变。
- TDD：`mcp_stdio::p2a`（无配置稳态 prepare <25ms debug 阈值）、`p2b`（空闲连接同限）先 RED（实测 51.5/51.2ms 失败）后 GREEN；`p2_active_notification_without_recent_call_still_relists`（夹具 `notify_on_list` 场景：无近期调用时服务器主动通知→链式重列）全程保持绿（语义保护）。
- 审查修正：`a12` 断言原「进程死亡边界第二次调用必为状态未知」过强（时序取决于 send 与服务循环退出先后；§2.4 只要求**区分**两类）——改为显式二选一断言＋新增「SECOND 无副作用（无重放）」断言，强度不降反升。变异 M7b（漏判目录过期）复验仍被 a13 两用例检出。

### 复测（同口径：同机、release、--locked --offline、预热2＋N=30/20、A/B 交替、全样本落盘）

候选二进制：terminal-chat `a7ec5d650cdba188707d890ee1a185e3a14abf263c657fe11afc58077b7677e1`、mcp_bench `2d2c02037f0aeb025b28140bcb78064e7b04e1cbf9206f32a448169f4ecaeacc`（old 二进制哈希与基线一致，见 §1）。原始数据：`docs/testing/perf/022/candidate-{overhead,mcp}/`（SHA256SUMS 覆盖基线＋候选全量 109 文件）。

**P1 接入开销（f662cf6 → 候选）**：

| 场景 | old p50/p95 | 候选 p50/p95 | Δp50 | RSS（old/候选） |
|---|---|---|---|---|
| startup | 10.8／11.0 ms | 10.8／11.0 ms | 0.0 ms | 1.2／1.2 MiB |
| chat_turn | 10.7／10.8 ms | 10.7／10.8 ms | **0.0 ms ≤1ms ✓** | 1.2／1.2 MiB |

失败数 0/0；波动（候选 max 15.9ms 单轮，p95 10.8ms）在噪声带内。

**P2 稳态快照六档（4fbf052 → 候选）**：

| 梯度 | 基线 p50 | 候选 p50/p95 | 判定（≤8ms） |
|---|---|---|---|
| t8-s256 | 51.3 ms | 0.0／0.0 ms | ✓ |
| t64-s256 | 51.8 ms | 0.3／0.3 ms | ✓ |
| t512-s256 | 54.4 ms | 2.4／4.3 ms | ✓ |
| t8-s4096 | 51.4 ms | 0.0／0.1 ms | ✓ |
| t64-s4096 | 51.8 ms | 0.3／0.4 ms | ✓ |
| t512-s4096 | 55.6 ms | 3.4／5.7 ms | ✓ |

失败数 0；内存仅观察：候选六档 mainRss 0.4~2.9 MiB（基线 6.5~23.5 MiB——本轮未承诺下降，快照不再逐轮全量重建后峰值显著低于基线属附带观察）；子进程 RSS 在短命场景未采到（不可测，非零占用）。

**P3 保留 settle 的场景（单列条件与耗时）**：

| 场景 | 条件 | 基线 p50 | 候选 p50 |
|---|---|---|---|
| relist_after_notify t64-s256 | 上轮有调用→grace 置位→settle | 52.9 ms | 52.5 ms |
| relist_after_notify t512-s4096 | 同上 | 72.1 ms | 69.0 ms |
| cancel_inflight t64-s256 | 取消轮询 50ms 粒度（方案 B 未实施） | 51.3 ms | 51.3 ms |
| discover_first 六档 | 无（首轮无 grace；仅连接+发现） | 70~98 ms | 19.6~43.2 ms（n=1，不宣称稳定提升） |
| close | 无 | 3.0/4.2 ms | 4.3/3.9 ms |

### 功能验证

- 全量：`cargo check --locked --offline` 通过；`cargo test --workspace --locked --offline --no-fail-fast`＝62 目标 3405 通过／0 失败／3 既有 ignored。
- 复验：MCP 全套（stdio 34＋turn 13＋http 11＋config 17＋l1 14）、e2e 142（含 s102/s103/s104）、A13 两用例、A11 取消、A18 HTTP 在途退出、变异 M7b 检出——全部通过。安全复验：取消/断连/重列边界行为未变（a11/a12/a13/a18 全绿；无放松安全校验）。

### 残留缝隙（审查记录）

- 无近期调用且主动通知恰在 prepare 进行中在途（无 grace 不 settle）→ 该通知的重列顺延至下下轮（不丢、最终一致）；窗口为单次 prepare 的毫秒级，实测 p2_active 链式重列不受影响。
- 调用被取消后 grace 已置位（通知可能不发生）→ 下次 prepare 白等一次 50ms；多次调用间通知已到达再调用 → 多余等待。均为方案 A 已知误报（≤1 次 50ms、计入耗时），符合修订后 §2.2-6。

## 7. 最终语义核验轮（2026-10-24 第三轮）

### 7.1 通知时点与生效轮次（受控闸门对照实验）

三个时点定义：T1＝服务器发出通知（字节写入管道）；T2＝宿主服务任务受理（`stale_epoch` `fetch_add`，原子即时可见）；T3＝下一次 `prepare` 读取 epoch。契约（4fbf052 §2.2-6 原文）：通知在**某次 prepare 的 epoch 读取之前**完成 T2 → 该次 prepare 生效重列；settle 是实现兜底而非契约。

**修正版探针**（`scripts/perf/notify_probe_run.py`＋`mcp_bench::probe_notify_turn`＋夹具 `notify_after_ms`；NOTIFY_DELAY_MS 经 mcp.json `env_vars` 显式转交，缺失即报错——首轮探针因 `env_clear` 未转交而 delay 失效，数据作废并在归档 JSON 中标注）：gap=200ms、prepare×6、delay ∈ {210,230,250,270,300,350,410,470,530}ms、每格 3 次，基线与候选同夹具二进制对照。逐格计算「通知到达（list1+delay）后第 N 次 prepare 生效」：

- 基线 4fbf052：**27/27 格全部 N=1**（到达后第一次 prepare 即重列）。
- 候选方案 A：**27/27 格全部 N=1**。

结论：「已受理通知比原契约多延迟一轮（下下轮）」**未在任何格出现，判定无回归**。此前记录 §6 所称「顺延至下下轮」缝隙经修正探针证伪为不可触发；残留的仅为理论 µs 级窗口（T2 恰落在 prepare 进入与 epoch 读取之间）——该窗口**两侧同样存在**（基线 settle 是否覆盖取决于通知与 prepare 的相位，非候选特有），属契约内边界，非实测回归，也未在 27 格扫描中出现。原始数据：`docs/testing/perf/022/notify-probe/gap200-sweep.json`。

### 7.2 a12 两类状态的确定性依据

| 类别 | 确定性证据 | 竞态补充 |
|---|---|---|
| 发送前确定断连 → 未发出、服务器计数 0 | **新增** `mcp_stdio::a12_call_after_close_is_not_sent_with_zero_server_calls`：生产入口 `manager.close()`（同步回收 service）后经旧快照 entry 调用 → 断言 NotSent 分类＋isError＋服务器该次调用（`arguments.text="after-close"`）计数＝0。变异检出已验证（把该路径伪装为 SentUnknown 时用例失败） | — |
| 服务器确认接收后断连 → 状态未知、无重放 | 既有 `a11_cancel_during_call`（stdio：调用发出→取消→SentUnknown＋record 恰 1 次）、`a18_http_inflight`（HTTP 闸门确认在途→取消→SentUnknown＋账本恰 1 次） | a12 原用例（进程死亡边界）：二选一断言＋分类必居其一＋「SECOND 无副作用」独立断言——**分类证据**由左列确定性用例承担，「无重放副作用」由该用例的字节级断言承担，二者分别说明 |

### 7.3 p2_active 受控同步修复（本轮真实缺陷修复）

审查发现 `p2_active_notification_without_recent_call_still_relists` 单独运行 7/8 假红：夹具 `notify_on_list` 在 list **响应前**发通知，通知到达可能在「基准读取之前」（轮 1 即消化，契约内）——测试把「通知必晚于基准」当隐含前提，属测试同步缺陷。修复：夹具改为 list 响应后 30ms 发通知（确保晚于基准且在轮窗口内受理），测试轮间加 100ms 受控停顿（区分「网络在途晚到」与「宿主已受理」）。修复后 10/10 稳定通过；变异 M7b（漏判目录过期）在该用例上复验仍检出。a13 两用例、a11/a12/a15/a16、全套件与 e2e 复验全绿。

### 7.4 验证与性能判定

- `cargo check --locked --offline` 通过；`cargo test --workspace --locked --offline --no-fail-fast`＝62 目标 **3406 通过／0 失败／3 既有 ignored**（+1：a12 确定性用例）。
- **性能不重测依据**：本轮生产实现零变化（connection.rs/manager.rs 与方案 A 候选逐字节一致，diff 校验），候选二进制同源——P1（Δ=0.0ms）／P2（六档 0.0~3.4ms）结论沿用 §6 复测。测试与归档材料变更不影响二进制。
