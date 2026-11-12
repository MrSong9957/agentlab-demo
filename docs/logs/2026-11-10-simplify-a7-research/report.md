# A7 调研：JS 编排事件通道无界（方案 a 有界化 vs 方案 b 维持无界）

日期：2026-11-10。对象：`src/app/src/js_orchestration/runtime.rs`。性质：调研记录，不改代码。

## 结论（先行）

1. 事件通道现状**无界**。模块头注释与规格 034 都写「有界」，代码与两处文档相反。这是清单 A7 的来源。
2. 推荐方案 a：改 `sync_channel(N)` 有界化。034 规格三处明文要求有界；无界积压是内存安全面，不是风格问题；实现面集中在一处读线程。
3. 方案 a 的唯一可观察差异：「让出期长输出 worker 提前停等」。这正是规格 §1.7 声明的背压语义，属回归设计意图。
4. 需用户拍板两点：是否接受上述行为变化；容量 N 取值（建议 16）。

## 现状（已核实事实）

- **通道无界**。`runtime.rs:527`：`let (tx, rx) = mpsc::channel::<Msg>()`。`mpsc::channel` 是 std 无界通道。
- **Msg 三变体**（`runtime.rs:231-236`）：`Worker(Value)`（worker stdout 帧）、`LeafDone`（叶子结果回执）、`ReaderDead(String)`（读线程退出原因）。
- **单帧上限 1 MiB**：`protocol.rs:13`：`MAX_FRAME_BYTES: u64 = 1024 * 1024`。帧数不限，所以通道总积压无上界。
- **读线程只管回送**（`runtime.rs:529-550`）：循环 `read_frame → tx.send(Msg::Worker(frame))`；`send` 失败（观察方 drop）即 return（:536-538）；worker 关闭 stdout 或坏帧时 send `ReaderDead` 后 return（:540-547）。send 无超时、无取消检查。
- **rx 只在观察路径被消费**：读线程端存为 `reader_rx`（`runtime.rs:589`）。`observe` 家族用 `recv_timeout` 消费（`runtime.rs:1234`），受 wall_deadline 与轮取消双重约束；close 时在途收割另有 5 秒上限的有界等待（`runtime.rs:1796-1800`）。
- **积压面**：让出（无人观察）期间，通道无界积压 raw 帧。`OutputBuffer` 的 64 KiB 上限（`PENDING_OUTPUT_LIMIT`，`runtime.rs:38`）只在观察合并时生效，不约束通道本身。
- **注释与代码相反**：`runtime.rs:7-11` 声称「有界事件通道」（:7）、「worker 写 stdout 阻塞形成天然背压」（:11）。实际通道无界，背压不存在。该注释是清单 S8 暂缓项，待定案后一并处理。
- **规格三处要求有界**：`docs/specs/034-javascript-tool-orchestration.md:70`（「新叶子请求仅留在有界队列」）、`:92`（「工作线程经有界事件队列回送」）、`:112`（「不留无限积压」）。
- **终止路径无读线程 join 点**：`kill_worker`（`runtime.rs:951`）与 `kill_worker_group_and_confirm`（`runtime.rs:1126`）按进程组杀进程；全文件唯一的线程 join 是叶子线程 `leaf.handle.join()`（`runtime.rs:1619`、`runtime.rs:1810`）。读线程的退出靠 rx drop：runtime drop 后 `send` 报 Err，线程 return（:536-538）。
- **并发规模**：规格限定同轮最多 1 个未消费 cell（034 `:50`、`:113`），所以暴露是「单 cell 内无界」，不是多 cell 叠加。
- **观察期语义（两方案均不变）**：观察时逐帧 `handle_worker_frame`（`runtime.rs:1236-1243`）；输出合并受 64 KiB 待交付上限（`runtime.rs:38`）；叶子结果 `LeafDone` 是独立变体，不受输出背压影响。

## 方案 a：有界化（`sync_channel(N)`）——风险与实现面

**机制**：读线程 `tx.send` 在通道满时阻塞 → stdout 管道（OS 缓冲约 64 KiB）填满 → worker JS 的写阻塞 → worker 停等。恢复 `runtime.rs:11` 注释声称、034 §1.7 规定的「天然背压」。

风险逐条，附缓解：

1. **读线程阻塞在 send 期间对 worker 死亡无感知**。线程卡在 `send`（`runtime.rs:536`），worker 被杀后的 stdout EOF 读不到，`ReaderDead` 通知丢失。
   缓解二选一：`send_timeout` 轮询＋kill/watch 标志；或接受「滞留至 runtime drop」——turn 结束必 drop（`reader_rx` 存于 runtime，`runtime.rs:589`），无永久泄漏。现状 close 路径已有 5 秒有界收割＋进程组杀（`runtime.rs:1796-1800`、`:1126`），可兜底，但需用测试钉住「ReaderDead 丢失时 terminal 仍由 wall_deadline/kill 产生」。
2. **worker 让出期提前停等（唯一可观察差异）**。无人观察且产出超过 N 帧时，worker 阻塞在 stdout 写（内存有界），而现状是无界积压继续计算（内存无界）。长输出 worker 的 wall-clock 可能提前触及 300s 墙钟被杀。规格 §1.7 的背压表述即此语义，属回归设计意图；需在 034 写明「让出期最多 N 帧后 worker 停等」。
3. **容量选择即内存上界**。N×1 MiB 为通道内存上界。建议常量化 N（如 16＝16 MiB 上界），在模块头写明口径；并与 `PENDING_OUTPUT_LIMIT`（64 KiB，观察侧，`runtime.rs:38`）区分——两层是不同的上限，注释不得混写。
4. **观察路径零改动**。`recv_timeout` 消费（`runtime.rs:1234`）、64 KiB 合并上限、叶子收割（`runtime.rs:1619`、`1810`）均不动。
5. **测试面**。新增「让出期 N 帧后 worker 停等、RSS 有界」行为测试（真实 worker 夹具）。既有 42 用例为即时观察，不受影响。回归口径：js_orchestration 全套＋security 套件。

## 方案 b：维持无界＋改注释＋登记 034 偏差——偏差后果

1. **内存暴露**。恶意或劣化脚本在无人观察期可使宿主内存无上限增长：每帧 ≤1 MiB（`protocol.rs:13`）、帧数不限。单 cell 内即无界（同轮最多 1 个未消费 cell，034 `:50`）。
2. **规格偏差要登记**。034 `:70`/`:92`/`:112` 三处「有界队列/不留无限积压」需登记为偏差（规格修订或已知边界），写明「宿主内存无界为既定姿态」。模块头注释改为如实描述：删 `runtime.rs:7`「有界事件通道」与 `:11`「天然背压」两处断言；清单 S8 暂缓项随之销账。
3. **终止/安全语义零变化，实现零风险**。但 A8（`output_frame_cont` 合并死路径）是独立缺口，方案 b 不解决。
4. **偏差登记不是终局**。未来若改有界，「worker 让出期停等」的行为差异会在那时出现，只是推迟。

## 推荐

**方案 a**（规格本意）。理由：

- 034 三处明文要求有界（`:70`/`:92`/`:112`），无界实现是规格偏差，维持它要反向改规格。
- 无界积压是内存安全面：worker 输出量由不可信 JS 决定，宿主侧无任何闸门。
- 实现面集中：spawn_cell 读线程一处（`runtime.rs:527` 起）＋容量常量＋send 失败/超时策略两个小决策。
- 可观察差异仅一处，且恰为规格声明的背压语义，不是新增行为。

建议实现顺序：

1. 容量常量定值（建议 16）。
2. `sync_channel`＋kill 感知 send。
3. 行为测试：停等＋RSS 有界。
4. 模块头注释按新事实改写（`runtime.rs:7-11`，S8 暂缓项销账）。
5. 034 规格回填「让出期最多 N 帧后 worker 停等」。

## 需要用户拍板的决策点

1. **是否接受「worker 让出期提前停等」**这一可观察行为变化。这是方案 a 的核心取舍：内存有界换长输出 worker 在让出期可能更早触墙钟。
2. **N 的取值**。建议 16（通道内存上界 16 MiB）；如对内存另有预算，给出目标值即可反推 N。

## 附：核实命令（供复现）

```bash
sed -n '525,556p' src/app/src/js_orchestration/runtime.rs   # 通道创建与读线程
sed -n '1225,1268p' src/app/src/js_orchestration/runtime.rs # 观察消费
grep -n 'PENDING_OUTPUT_LIMIT\|MAX_FRAME_BYTES' src/app/src/js_orchestration/*.rs  # 两层级上限
grep -n '有界' docs/specs/034-javascript-tool-orchestration.md                     # 规格三处要求
```

## 定案（2026-11-11）

用户已选**方案 a**（`sync_channel(N)` 有界化），**N=16**（通道内存上界 16 MiB），
并接受「让出期长输出 worker 提前停等」这一可观察行为变化（规格 §1.7 背压语义
回归设计意图）。后续实施另立记录；034 规格修订随实施落地。
