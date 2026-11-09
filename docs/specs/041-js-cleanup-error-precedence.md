# 规格：JS cell 清理错误不得覆盖既有终态错误

- 状态：**已实施待验收（2026-11-09）**
- 创建：2026-11-09。来源：038/040 轮登记的存量失败 `js_orchestration::worker_killed_ipc_dead_fails_cell`（在基线 `b85c4ee` 与 040 分支均确定性失败；同代码在更早 U4 记录中多次全绿——时序敏感面被 040 验证环境固化暴露，用户立项修复）。
- 关联：`docs/specs/034-javascript-tool-orchestration.md`（§1.5 IPC 断开语义）、040 验证记录「范围外发现」登记。

## 1. 根因（代码走查＋确定性复现）

worker 被外力 SIGKILL 时，事件序为：读线程观察到 ReaderDead → cell 终态置为「worker IPC 断开…」（Failed）→ observe 收尾调用 `close()` → `kill_worker` 对已被外力击杀的监督进程执行 payload-wait 确认，返回 Err「bwrap 被外力终止，payload wait 责任未确认」（该分支的存在是对的——监督进程死亡方式确实未核实）→ `cleanup_error = Some(...)` → `add_cleanup_failure(&mut observation)` **无条件覆盖** `observation.error`（原终态错误「IPC 断开」被洗成 `WORKER_REAP_FAILED`）。

测试断言两个可接受报文（「IPC 断开」或「读线程退出」），故失败。危害不只测试：真实场景中，模型/用户可见的终态原因（IPC 断开＝worker 崩溃）被内部清理细节（回收失败）替换，诊断信息失真；且 `WORKER_REAP_FAILED` 语义（清理未确认）比真实根因更晚、更间接。

## 2. 需求与验收

- R1（优先级规则）：`add_cleanup_failure` 在 observation 已携带终态错误时**不得覆盖**；cleanup 失败信息在无既有错误时照常呈现（正常完成＋清理失败、终止＋清理失败两条既有语义不变）。
- R2（不削弱断言）：`worker_killed_ipc_dead_fails_cell` 断言不变（仍要求 Failed＋「IPC 断开」/「读线程退出」二选一）。
- R3（确定性）：该测试连续 20 次运行全绿；`js_orchestration` 全套全绿（存量 `worker_killed_ipc_dead_fails_cell` 转绿，其余无回归）。
- R4（无扩散）：不改 kill_worker/wait 路径、不改收割调用（040 面）、不改会话拆除；仅观察层错误优先级。

## 3. 边界

- cleanup 失败仍有观测面：无既有错误时照常覆盖为 WORKER_REAP_FAILED（既有行为）。
- `cleanup_failed_observation`（closed 句柄复查路径）不变——该路径本无终态错误可保护。

## 4. 预测 → 实测（实现后回填）

- R1 修复 → 预期测试报文回归「IPC 断开」→ 实测：待测。
- 20 连绿 → 实测：待测。
- js 全套 → 实测：待测。
- 回归风险：既有测试断言 cleanup 失败应覆盖的场景（若有）→ 实测：待测。
