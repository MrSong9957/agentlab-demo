# 035 独立代码审查报告与修复闭环（2026-10-29）

- 审查者：独立只读子代理（新上下文，未参与实现；按 `rules/REVIEW.md`）。
- 材料：`2026-10-29-review-diff.txt`（基线 main=4766d61）＋规格＋仓库源码。
- 审查范围：锁序/竞态、容量预算原子性、V3 持久化一致性、安全边界（参数
  严格性/子目录隔离/owner 归属/凭证）、根轮集成（批规则/交付/封口门/封组）、
  恢复与投影。

## 审查结论（原文摘要）

2 项阻塞（B1/B2）＋3 项非阻塞（N1–N3）＋4 项低于门槛小项；锁序纪律、参数
严格性、owner 归属、投影过滤、恢复不重放等重点面未发现违例；全局子槽的
预留/释放路径经逐出口核实无泄漏。

## 阻塞问题与处置

### B1：组关闭后子线程在途事件仍落盘 → 装载链校验整文件拒绝（/resume 不可用）

- 证据：根轮任意出口写 group_closed 后，阻塞在模型请求/工具执行中的子线程
  返回并提交 child_record（`commit_child_event` 无 closed 门），事件落在
  group_closed 之后；`validate_agent_chain`「group_closed 后仍有该组事件」
  → 整文件 Err。确定性路径（子阻塞在 send）与竞态路径（exec 结果提交）均
  可触发，破坏 007 会话契约与 C14/C15。
- **修复**（三层闭合，`agents/runtime.rs`＋`turn.rs`）：
  1. **迟到返回门**：`child_loop` 在 `sender.send` 返回后立即检查取消/组
     关闭——迟到响应不解析执行、不入子上下文（§1.5「迟到返回按关闭门丢弃」
     的子侧落地）；
  2. **group_closed 延迟提交**：`live_children` 计数＋`closed_event_written`
     CAS——轮出口仍有存活子线程时不写 group_closed（根轮出口不被传输排空
     阻塞），由**最后退出的子线程**经泵路径补写；在途事件必然先于 group_closed
     落盘，装载链校验恒成立；无子线程时根线程照常在轮出口提交；
  3. 终态摘要/补写失败时事实仍在（closed_event CAS 恰一次；写失败＝文件无
     group_closed，/resume 呈现「已失联/结果未知」的规格内降级形态）。
- 回归：`b1_group_closed_after_inflight_events_file_still_loads`（慢 sender
  阻塞→根终文遇活动子任务报错→封组→子 1.5s 后迟到终文被门丢弃→最后退出者
  补写 group_closed→会话文件装载成功且 group_closed 为该组最后事件）。

### B2：子工具调用预算旁路——额度耗尽仍真实执行

- 证据：`let _ = runtime.child_admit_call(agent);` 丢弃 Err 后无条件进入
  `dispatch_child_tool`——64 次/assignment 或组 256 次耗尽后的调用照常执行
  外部动作（exec/patch/MCP），且不计数；`event_room` 不足分支的错误消息
  自称「工具调用不开始」与行为相悖。
- **修复**：准入失败＝`BUDGET_LIMIT` 失败结果占位（配对完整）＋child_record
  记录占位结果，**不执行**（§1.6「调用前额度不足不能触发外部动作」）。
- 回归：`b2_call_budget_exhausted_no_external_action`（两批 32 次 update_plan
  ＝64 次上限→第 65 次写标记 exec：结果为 BUDGET_LIMIT、标记文件不存在）。

## 非阻塞问题与处置

| # | 问题 | 处置 |
|---|---|---|
| N1 | 终文封口门两次独立加锁的微窗口内已发布终态丢失 | `turn.rs` 接受终文前复查一次 `pending_updates`（先入正文再交付继续） |
| N2 | child_record 保存失败未释放 10 MiB/2 MiB 预留（累计后假性「预留不足」）；assistant 记录以 0 冲销未按真实大小计费；超大事件后 `context_continuable` 未置 false | 失败路径 `child_release_reserve(reserved, None)`；两条路径按 `event_bytes` 真实计费；新增 `mark_context_broken`（followup 返回 CONTEXT_UNAVAILABLE） |
| N3 | followup 启动失败回滚错置 Failed 并覆盖任务文案 | `rollback_followup` 还原前一终态与原任务文案（Plan 捕获 prev） |

低于门槛小项处置：TUI 补部分配置一次性 Info 提示（§1.2 提示半边原本只在
CLI）；`account_event` 注释与实现对齐；`same_reap_scope` 死代码已删；组消息
预算跨锁段 ±4 条过收窗口如实登记（见实施记录 §4）。

## 复验

- `env -u TCHAT_SANDBOX cargo test --locked --test multi_agent`：**20 通过/
  0 失败**（含 B1/B2 回归；单线程/并行模式均绿）。
- 修复触及面回归：`--lib`（289/0）、`--test tool_loop`（43/0）、
  `--test context_management`（19/0）、`--test session`（40/0）、
  `--test prompt_assembly`（20/0）。
- 审查者附带的正面核实（锁序/参数/owner/投影/隔离/fork）在修复轮未涉及，
  维持其结论。
