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

## 4. 预测 → 实测（2026-11-09 已回填）

- R1 修复 → 预期测试报文回归「IPC 断开」→ 实测：**符合**——worker_killed_ipc_dead_fails_cell 断言通过。
- 20 连绿 → 实测：**符合**——20/20 全绿（b85c4ee+041 与 rebase 后 main+041 两形态）。
- R4 无扩散 → 实测：**符合**——仅 add_cleanup_failure 观察层 guard（+6 行）；kill_worker/wait 路径、040 收割调用、会话拆除均未触碰；cargo check --all-targets 通过。
- js 全套全绿 → **未达成，被独立的存量确定性失败阻断**：\`sandbox_worker_all_termination_methods_denied_reports_failure\` 在 rebase 后（ccfba40+041）单跑与并行均确定性失败。插桩实证：terminate → close 先写 shutdown 帧 → worker 收帧自退（await Promise 不影响引擎控制帧）→ kill_worker 的信号调用全部 ESRCH → wait 成功 → **Ok** → cleanup_error=None → Terminated；测试断言「信号全拒 ⇒ Failed＋WORKER_REAP_FAILED」。矛盾＝测试设计（把「信号被拒」当充分条件）与产品优雅关闭语义（shutdown 帧优先、信号为兜底）的冲突；11-08 记录「聚焦均绿」与 04-batchB「FAILED」并存，说明该测试结果随环境负载在两路径间翻转（帧处理 vs 信号 EPERM 的竞速）。**两个修法需用户裁决**：(a) 修测试——注入环境同时使 shutdown 帧失效（如 spawn 后即刻关闭协议 stdin 写端），保持产品语义与既有断言不变；(b) 修产品语义——信号被拒时即使 shutdown 帧成功也上报 WORKER_REAP_FAILED（改 terminate 成功语义，影响真实优雅关闭路径，不推荐）。此为阶段 4 授权范围（worker_killed_ipc_dead 修复）外的独立存量缺陷，按「重大取舍先请示」停下。
