# A7 写线程端到端行为锚：红→绿实证（2026-11-12）

testing_gaps 登记项（批 5 审查 R2 登记「端到端大结果用例重——testing_gaps
登记」）落地。测试：`tests/js_orchestration.rs::
large_leaf_result_through_writer_thread_arrives_intact`（提交 `404198b`）。

## 设计

- 真实 `spawn_cell`＋真 worker 二进制（`CARGO_BIN_EXE_terminal-chat`）＋真实
  管道；叶子结果 250KiB（≈3.9× 管道容量 64KiB；`MAX_BRIDGE_RESULT_BYTES`
  262144 字节内的最大量级）经 `reply_leaf` → stdin 写线程（u64 LE 长度前缀
  ＋JSON）→ worker。
- worker 侧 JS 对收到的字符串做长度＋FNV-1a 32 位摘要，经 `text()` 回传；
  宿主以同源摘要逐字节对照。帧长前缀错位、截断、损坏任一发生即摘要失配或
  ReaderDead（cell Failed）。
- **上限实测校准**：300KiB 首版快速失败——`handle_leaf_outcome` 在写线程
  之前即以桥接上限拒绝（SCRIPT_FAILED「结果 JSON 300008 字节超过 262144
  桥接上限，拒绝交付」），不达写线程；250KiB 为契约内可走写线程的最大量级。

## 红→绿序列（AGENTS.md §4「临时破坏被测行为」法）

死锁敏感性说明：修复前的根线程同步写在「worker 阻塞写 stdout 且不读 stdin」
时才死锁；真实 worker await 后即读 stdin，确定性构造与 1MiB 输出上限、
16 槽通道互斥（时序竞态非确定性）——红采用本条方法的临时破坏替代。

1. **开发基线绿**（干净 HEAD `f17ff48`＋本测试）：
   `test result: ok. 1 passed`（0.45s）——测试自身正确、两侧 FNV 一致。
2. **红**：临时改 `reply_leaf` 发裸 JSON（复现写线程初版真实缺陷——缺
   u64 LE 长度前缀）：
   ```
   thread '…arrives_intact' panicked: assertion `left == right` failed:
   Observation { ok: false, status: Failed, … "message": String("坏帧断开：
   IPC 帧长度 7308253928396956283 超过上限 1048576：坏帧断开") …}
   test result: FAILED. 0 passed; 1 failed
   ```
   帧长 7308253928396956283＝`{"t":"le` 八字节 ASCII 的 u64 LE 解释——
   与审查发现的历史缺陷机制逐字对应。
3. **绿**（还原后）：
   `test result: ok. 1 passed; 0 failed`（0.38s）。

## 命令

```
env -u TCHAT_SANDBOX cargo test --test js_orchestration \
  large_leaf_result_through_writer_thread_arrives_intact
```
（`TCHAT_SANDBOX` 既有约定：compose 注入 =1，测试一律 `env -u`。）

## CI 首验

随提交 404198b 进入分支，run 3（43134862237）ubuntu 腿全量 ✅ success
（第 4 连绿）——锚在 CI 环境真实执行并通过。
