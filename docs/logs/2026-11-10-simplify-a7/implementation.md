# A7 方案 a 实施记录（2026-11-11）

定案：`docs/logs/2026-11-10-simplify-a7-research/report.md` §定案（方案 a、N=16、
接受让出期停等的可观察变化）。

## 改动（全部在 `src/app/src/js_orchestration/runtime.rs`）

1. 容量常量 `EVENT_CHANNEL_CAPACITY: usize = 16`（模块常量区，注释写明口径：
   16 帧×≤1 MiB＝积压上界 16 MiB；与 `PENDING_OUTPUT_LIMIT`（观察合并侧 64 KiB）
   是两层不同上限）。
2. 通道构造改经单点 `event_channel()` → `mpsc::sync_channel(EVENT_CHANNEL_CAPACITY)`；
   `spawn_cell` 调用点由 `mpsc::channel::<Msg>()` 换为该单点。
3. `CellHandle.event_tx` 类型 `mpsc::Sender<Msg>` → `mpsc::SyncSender<Msg>`
   （读线程/叶子线程 send 与 clone 用法不变；叶子线程满时停等由观察/收割消费
   解除，消费侧 `recv_timeout` 恒有界，无循环等待）。
4. 读线程 send 处注释、模块头注释（`runtime.rs:7-14`，S8 暂缓项）按有界事实改写。
5. 新增有界性锚测试 `event_channel_is_bounded_at_declared_capacity`（见下）。

行为变化仅一处：让出期产出超 16 帧后 worker 停等（034 §1.4 修订段已登记）。

## send 阻塞缓解策略：选「接受滞留至 runtime drop」（调研报告两案之二）

理由：

- 生命周期证据：`orch_cell` 是轮函数局部 `Option<CellHandle>`（`turn.rs:1506`），
  `close_orch!` 宏在 `close_for_turn_exit` 后相邻两行即 `orch_cell = None`
  （`turn.rs:1517`）；其余出口（early return、unwind）由函数返回兜底 drop。
  句柄无逃逸路径，滞留窗口＝close→drop 之间，实际为零。
- `send_timeout`＋kill 感知的增益（提前退出）在该前提下不可观察：close 与置
  None 相邻，读线程无额外滞留期；为不可观察收益引入 200ms 轮询状态机违反
  极简原则，且轮询周期本身就是新的时序参数。
- 调研报告同案注记成立：「turn 结束必 drop（reader_rx 存于 runtime），无永久泄漏」。

## 三终止路径无悬挂论证

读线程唯一阻塞点是 `tx.send`（满时）；退出机制＝接收端 drop 后 send 报 Err
（`runtime.rs:550-558`，原语义保留）或消费腾容量后送达、读 EOF 报 `ReaderDead`。

1. **runtime drop**：`CellHandle` drop → `reader_rx` 弃置 → 滞留中的 send 立即
   返回 Err → 线程 return。无 join 依赖。
2. **worker 死亡**（被杀/自行退出）：让出期读线程可能滞留 send（EOF 读不到，
   `ReaderDead` 暂丢）。恢复途径：(a) 下次观察 `recv_timeout` 消费（观察循环
   恒有界：`wait_until ≤ min(yield deadline, wall_deadline)`）腾容量 → send
   送达 → 下一轮 `read_frame` 读到 EOF/坏帧 → `ReaderDead` 入队 → terminal；
   (b) 直到轮结束无观察 → close 收割消费或句柄弃置退出。终态兜底：观察循环
   顶部无条件检查 `wall_deadline`（`runtime.rs:1217` 附近），WALL_TIME_LIMIT
   终态不依赖 `ReaderDead`。
3. **轮终止**：`close_for_turn_exit` → `close`（cancel 置位；5 秒有界收割消费
   rx——`Ok(_) => continue` 对 Worker 帧同样消费腾容量；进程组杀）→ `orch_cell
   = None` → drop → 路径 1。叶子线程滞留 send（`LeafDone` 排在满通道后）同由
   消费腾容量解除；5 秒未及收割者沿既有孤儿 start 语义（§1.6，结果未知）。

无死锁：读线程/叶子线程只等消费者；消费侧所有等待均有界（`recv_timeout` 带
deadline、close 收割 5 秒上限），不存在「消费者无限等待生产者」的环。

## 有界性锚（无时序依赖，不硬凑）

`event_channel_is_bounded_at_declared_capacity`：经构造单点取通道，断言
(a) 容量内 N 次 send 全成、(b) 第 N+1 帧 `try_send` 被拒（有界性＝常量）、
(c) 接收端 drop 后 send 报 Err（三路径退出的机制前提）。全部为确定性断言，
零时序依赖。

未写的测试：「让出期超 N 帧后 worker 停等」的真实 worker 行为测试需对 stdout
管道填满时机做时序断言（OS 管道缓冲 ~64 KiB 与帧排空的竞争），稳定观测不可得，
按任务约定不硬凑；容量与断连机制已由锚测试锁定，行为面由调研报告机制论证＋
034 修订段登记。

## 验证（容器内 /workspace，串行）

| 项 | 结果 |
|---|---|
| `cargo check --tests` | 0 错误；警告 605＝基线零新增 |
| `cargo test --lib js_orchestration` | 13 passed / 0 failed（含新锚测试） |
| `cargo test --test js_orchestration` | 42 passed / 0 failed |
| `cargo test --test js_orchestration_security` | 20 passed / 0 failed（120s，真实沙箱） |
| `cargo test --test js_orchestration_perf` | 4 passed / **3 failed**（见下） |

perf 3 失败（`c18_cold_start_pure_js`、`c18_leaf_counts_1_4_31`、
`c18_yield_then_wait_and_terminate`，均在 `assert_p95` :214 超冻结阈值）为
**环境漂移存量，与 A7 无关**，证据：

- 干净树对照：`git stash` 后在 HEAD（改动前）跑同 3 用例，失败集合与断言
  位置完全一致（0 passed / 3 failed，362.7s vs 改动树 361.0s）。
- 采样（`JS34_PERF_SAMPLE=1`，断言旁路）：冷启动 p95 三轮中位 332,527µs
  vs 阈值 300,000µs，分布紧密（327/332/353ms）——系统性放慢（冻结基线
  2026-10-29 轻载实测 13.4ms，约 24 倍漂移），非离群抖动；期间 toolchain
  已由 1.99.0（冻结时记录）变为 1.99.1，宿主负载状态亦不同。
- 机制排除：三用例单轮仅数帧，通道从不接近满容量；`sync_channel` 未满时
  send 与无界通道同量级，无可观测时延差异路径。
- 残留进程检查：无泄漏 worker/busy 进程（仅既有「bwrap 留作等待者」的 0 成本
  僵尸形态）。
- 断言自带说明「机器/构建变化须重新冻结」；重新冻结基线属 C18 验收决策，
  本任务不擅自改动，留待用户处理。

js_orchestration 相关套件按可归责范围全绿：改动引入的失败为零。
