# 批 5.2 验收审查 findings 处置（2026-11-11）

审查输出：correctness 6 findings＋adversarial 6 findings＋project-standards 1
finding——去重后 11 项，全部处置如下。peer（codex 路由）拒绝 unknown host
family 未启动——对抗位由本地 adversarial reviewer 承担（按技能 fallback 规则）。

## 修复（代码）

1. **P1（adversarial，high）**：S10 提交 `c5072d1` 误删
   `measure_request_body_bytes_bounded` 的 `return Ok(None)`（带尾注释的功能行
   被清理脚本误判；「非注释行零变化」断言对该形态失效）。已恢复＋注释记录＋
   context_management 43/0。**S10 报告的行为不变声明就此修正**。
2. **P2（correctness，high）**：A8 跨调用误合并——首片 160 KiB 超 64 KiB 待交付
   上限被整片丢弃后，cont 片经 merge_last 并入上一次调用遗留末条（内容损坏）。
   修：OutputBuffer 增 `dropping_call` 状态——首片被丢置位，同调用 cont 片随之
   丢弃（034「单次超 64 KiB 的 text 整条丢弃」语义），新调用首片成功复位。
   锚 `cont_frames_never_merge_across_calls_after_drop`。
3. **P2（correctness，medium）**：A7 有界通道三方死锁——根线程 reply_leaf 同步
   写 stdin 大结果帧（≤256 KiB＞管道 64 KiB）与「读线程停等→worker 不读 stdin」
   互锁，observe 循环不可达＝墙钟兜底失效。修：worker stdin 专用写线程
   （CellHandle.writer_tx；根线程只投递，写线程阻塞不牵连 observe；close/
   Drop 弃置接收端随写失败退出）。**首轮实施漏掉线格式长度前缀（裸 JSON）致
   帧流错位——js_orchestration/multi_agent 假红抓出，补 u64 LE 前缀后全绿**
   （45/0＋25/0）。
4. **P3（双 reviewer 发现）**：commit_checkpoint map_err 对预检分支误追加
   「，活动视图不变」、对截回失败分支漏还原——改三分支精准映射（contains/
   starts_with/replacen）。
5. **P3（双）**：create_target 统一后迁移路径终局文案误为「自动保存已停用」
   （事实错误）——增 `conflict_err` 参量，迁移路径恢复「检查点未提交（自动
   保存不受影响）」。
6. **P3（双）**：reap_where pending 判定与移除失去同锁一致性（伪失败 Err）——
   helper 只终止，最终 pending/移除判定收回锁内单遍。
7. **P3（correctness，high）**：S28.3 snapshot_cache 永不命中（ensure_loaded
   失效语句置于 loaded 检查前＝复用死路径）——失效移入实际装载分支。

## 登记（不改行为）

8. **low**：thread_updated_at_ms 行取值错误文案并入单一通道（query_row 限制）
   ——注释如实化（原注释「单列保真」失实）。
9. **low**：A5 序列化失败分支 is_error false→true 且不过污染检查（理论不可达）
   ——代码注释登记差异。
10. **low（standards）**：batch3 报告把 context.rs 修复依据误引 034——修正为
    027/context 注释（034 全文无该句）。

## 验证

- `cargo check --tests`：0 错误；警告 **580＝基线零新增**。
- 受影响套件：context_management 43/0、js_orchestration 45/0、multi_agent
  25/0、session lib 30/0、proc_session lib 24/0、mcp_l1 14/0、lib js_orchestration
  14/0（含 A8 跨调用锚）。
- 全量套件（修复前基线轮）：55/55 PASS（/tmp/full_suite_out.txt；修复后受影响
  套件逐一重跑如上）。
