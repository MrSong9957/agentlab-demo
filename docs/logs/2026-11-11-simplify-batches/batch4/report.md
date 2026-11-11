# 批 4：S36 审查标签规则补锚（2026-11-11）

五条锚（第 5 条经核实已存在）；「今天该红的先红」核查：锚 1 首跑红，
**查明为锚预期算错而非规则未落实**——实际序列
`[(CellStarted,1),(CallStarted,2),(CallFinished,3 被拒),(CellClosed,3 复用)]`
＝无空洞（规则「成功才占用序号」已正确落实；若失败也递增则 closed=4＝空洞）。
断言修正为 closed==3 后绿；未改任何规则代码。

## 逐条

1. **P1-3 seq 连续无空洞**：新锚 `persist_failure_does_not_leave_seq_hole`
   （js_orchestration.rs）——RecordingSink 扩展 (kind,seq) 记录；call_finished
   保存失败→CellClosed 复用同号。标签注释「——审查 P1-3」已删（事实保留）。
2. **P2-8 二次 observe 不再截断**：新锚 `second_observe_does_not_repeat_truncation`
   ——100 KiB 条首次 observe output_truncated=true、二次 false（非粘性）。
   标签注释已删。
3. **N3 followup 满槽保状态文案**：新锚
   `n3_followup_full_slots_keeps_terminal_state_and_message`（multi_agent.rs）
   ——3 个屏障挂起 Agent 占满全局 3 槽→对 completed 目标 followup→
   CAPACITY_LIMIT「全应用子执行槽已满（3）」→list_agents 断言 state 保持
   completed＋last_task_message 原样。标签注释已删。
4. **reap_failed 清除时机**：新锚 `reap_extra_clears_failed_latch_even_without_extras`
   （proc_session.rs）——置位后再次 reap（无 extras，早退路径）成功即清除。
5. **sandbox 成对断言在组装层强制**：核实**已存在**——
   `pid_namespace_and_proc_paired`（sandbox/tests.rs，--unshare-pid＋--proc
   成对＋--as-pid-1 反证）；无标签注释可删（生产侧「成对」注释为事实文档）。

## 验证

- js_orchestration 45/0、multi_agent 25/0、proc_session lib 24/0；
  `cargo check --tests` 0 错误、警告 **580＝基线零新增**。
