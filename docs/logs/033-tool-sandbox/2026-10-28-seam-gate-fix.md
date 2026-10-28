# 033 seam 监控轮询竞态确定性修复（2026-10-28，方案 a）——测试缝，独立于 B1/B2 产品改动

**范围**：仅 `proc_session::tests::seam_killpg_injection_deadline_not_fake_timeout`。
分支 `feat/033-b1-b2-hardening`（工作树 `.worktrees/033-b1-b2`，未提交）。
**被验证的产品终止语义零改动**——全部缝代码 `cfg(test)` 门控，生产构建无变化。

## 1. RED 证据（保留原日期与日志）

- 2026-10-27 run3 全量失败：`2026-10-27-b1b2-full-run3-final.log`（Timeout ≠
  Terminated，"总运行时限到期，已终止并回收进程组"）。
- 2026-10-27 负载定向复现：`2026-10-27-closeout-seam-attribution.log`
  （空闲 4/4 通过；16 忙环负载第 8/15 轮复现同款）。
- 机理：killpg 注入失败时监控线程**保留 `kill=Deadline` 请求**按
  `MONITOR_POLL`（10ms）轮询重试（proc_session.rs 监控循环"保留请求在后续
  轮询重试"分支）；原测试解除注入后与重试 tick 竞速——负载下调度翻转即误期。

## 2. 测试意图区分（本轮 /goal 第一节）

- **覆盖规则（被测语义）**：Deadline 请求**尚未消费**时，显式 terminate 经
  `request_kill` 的替换分支（`kill==Some(Deadline) && req==Terminate` → 替换）
  覆盖挂起请求，终止后事由归 terminate。→ 门控缝保证确定性验证（§3）。
- **Deadline 已消费 → Timeout 合法**：到期终止如实归 Timeout 的契约面由**既有
  确定性覆盖**承担（`seam_clock_shift_expiry_without_query` :1758「查询不延长
  截止」、:1669「到期终止如实报告」等 clock-shift 缝用例）——无需新测，本轮
  不重复（原测试首段"注入期内到期 → Running＋终止失败如实报告"断言原样保留）。

## 3. 同步时序说明（最小缝）

**门控位置＝监控循环顶部、读 `st.kill` 之前**：停靠时 Deadline 请求必然未被
消费。**自动放行条件＝`kill_snapshot()==Some(Terminate)`**：即
`request_kill` 的覆盖替换已提交——「terminate 提交」被状态机编排到「Deadline
重试消费」之前，不再依赖抢在 10ms tick 前。

```
测试:  布防(Paused) → [等 Parked ≤5s] → drop 注入 → interact(terminate)
                                                      │ request_kill 替换
监控:  … → 循环顶 test_gate_wait: Paused→Parked(不持会话锁) ─┘
       → 自动放行 → 读 kill=Terminate → killpg(注入已解除) Ok → Terminated
```

约束逐条：仅 `cfg(test)`（类型 3＋`test_gate_wait`＋`Session` 3 方法＋
`SessionState` 字段/init＋监控循环 1 处调用，共约 90 行）；**会话级定位**
（`SessionState.test_gate`，未布防即时返回，并行用例零影响）；停靠**不持会话锁**
（gate 锁 5ms 有界轮询＋瞬时 `kill_snapshot`，不阻塞 `request_kill`；锁序单向
gate→session，无死锁）；**有界等待**（停靠确认 5s＋门控自放行 10s）＋
**守卫 Drop 自动放行**（panic 展开亦经过，监控可推进、进程可回收）；**无新全局
覆盖**（对照 `TEST_KILLPG_FAIL_PGD`/`TEST_CLOCK_SHIFT_MS` 静态缝，本缝在会话内）。
测试内新增编排 11 行（布防＋等停靠），原关键断言（Running/终止失败如实报告/
Terminated/停止写入）一字未动。

## 4. 验证（2026-10-28）

| 项 | 结果 |
|---|---|
| 确定时序下原测试 | ✅ 空闲 3/3（7.26s 恒定）＋轻量受控负载（8 忙环＋taskset 0-7）8/8（7.27s 恒定）——`seam-gate-stable.log` |
| 检出力（变异） | ✅ 临时移除 `request_kill` 覆盖分支 → 测试**确定性失败**（门控不放行→interact 3s 有界回收→Error ≠ Terminated，:1708 断言捕获；`seam-gate-mutation.log`）→ 恢复通过。语义破坏不再依赖竞速即可检出 |
| Timeout 合法面 | ✅ 既有 clock-shift 缝用例覆盖（§2），未改动 |
| 相邻污染 | ✅ `proc_session::`＋`memory::workspace::tests::` 同进程 21/21（`seam-gate-adjacent.log`）；无遗留进程（pgrep 复核） |
| 编译 | ✅ E0597 一处（尾部表达式临时值）按编译器建议改局部绑定后通过；本文件无新增 clippy 告警 |

## 5. 最终全量（冻结后，2026-10-28，验收等价口径）

口径：独立 target（`/tmp/tchat-b1b2-target`）＋PTY 200x50＋`TERM=dumb`＋
`env -u TCHAT_SANDBOX`（部署注入态实测为 1）＋check/test 串行＋`--locked
--offline`（包装器 `/tmp/tchat-closeout/pty_run.py`）。

```
cargo check --locked --offline && cargo test --workspace --no-fail-fast --locked --offline
final_exit=0
70 目标：3529 passed / 0 failed / 3 ignored
```

日志：`2026-10-28-b1b2-final-full-pty.log`。不新增忽略项；不重复 A–G 对抗
矩阵与性能测试（按 /goal 复用 2026-10-27 证据）。

## 6. 验收结论

**待交付候选验证通过（2026-10-28）**：B1/B2 产品加固（此前轮）＋B1 元数据三分类
（收口轮）＋memory 测试隔离修复（收口轮）＋本缝（时序确定性）全部就位，最终全量
零失败。**边界**：候选全部改动未提交、未合并、未推送；当前部署二进制
（`/workspace/target/debug/terminal-chat`，mtime 1793056909）未替换。

## 7. 最终差异（跟踪文件 7 个，+432/−8）

| 文件 | 内容 | 归属 |
|---|---|---|
| `src/app/src/sandbox.rs` | B1 nlink fail-closed＋B2 close_range 标记助手＋元数据三分类 | B1/B2 产品 |
| `src/app/src/proc_session.rs` | B2 pre_exec 注入（沙箱分支 1 hunk）＋本缝（cfg(test) 全部） | B2 产品＋本缝 |
| `src/app/src/mcp/transport.rs` | B2 pre_exec 注入（tokio） | B2 产品 |
| `src/app/src/apply_patch.rs` | B2 pre_exec 注入 | B2 产品 |
| `src/app/src/memory/workspace.rs` | 覆盖缝读者串行锁 | 测试隔离修复 |
| `src/app/src/sandbox/tests.rs` | 单元 6 例（B1 3＋收口 3） | 测试 |
| `docs/specs/033-tool-sandbox.md` | B1/B2 加固轮＋收口段 | 文档 |

另有未跟踪新测试文件 2（e2e_sandbox.rs 7 例、sandbox_matrix_probes.rs 10 例）与
`docs/logs/033-tool-sandbox/` 证据 15 份（2026-10-27/28，日期均为真实执行日）。
