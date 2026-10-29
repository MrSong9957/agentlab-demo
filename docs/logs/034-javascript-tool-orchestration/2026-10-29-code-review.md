# 034 独立代码审查闭环（2026-10-29）

审查者：独立只读子代理（Explore 型，按 rules/REVIEW.md；不修改文件）。
范围：feat/034-javascript-tool-orchestration 分支相对 main 的全部未提交改动。

## 发现与处置（12 条全部闭环）

| 编号 | 严重度 | 摘要 | 处置 |
|---|---|---|---|
| P1-1 | P1 | wait 在终态已交付但输出未取尽时过早丢弃句柄（增量丢失、cell_closed 缺失） | 已修：仅 cell.is_closed() 才释放；回归 wait_keeps_handle_until_output_drained（增量恰补全 64000 字节） |
| P1-2 | P1 | 轮 128 叶子额度被每次 exec 重置（单轮可 8×31=248 次） | 已修：预算提升至 run_tool_turn_body/TUI 轮线程/CLI cmd_chat 入口，经 OrchestrationExtras.turn_budget 共享；回归 turn_budget_shared_same_instance |
| P1-3 | P1 | persist 失败路径写出 seq 空洞（1,3），整档会话装载被自研校验拒绝 | 已修：event_seq 仅 commit 成功后递增（失败序号由后续事件复用） |
| P1-4 | P1 | cell_id 进程内自增，/resume 后与旧事件碰撞致会话不可装载 | 已修：id 加进程启动纳秒前缀（cell-{nanos:x}-{seq}） |
| P2-5 | P2 | 额度扣减在参数/资格检查之后（与 §1.5「先原子扣额」相反） | 已修：帧解析成功即先扣轮额度，拒绝路径不回补 |
| P2-6 | P2 | wait terminate=true 不校验 cell_id 精确匹配 | 已修：terminate 同样过 id 匹配；回归 e2e_terminate_requires_exact_cell_id |
| P2-7 | P2 | cell_started 保存失败被标 WORKER_SPAWN_FAILED（错误码混淆） | 已修：区分 PERSIST_FAILED / WORKER_SPAWN_FAILED |
| P2-8 | P2 | output_truncated 置位后永不清零（粘性误报） | 已修：交付即复位（本次观察语义） |
| P2-9 | P2 | 桥接内部函数残留全局、ALL_TOOLS 仅浅冻结 | 已修：prelude IIFE 封装内部状态；__settle/tools/ALL_TOOLS/text 以 defineProperty 不可写安装；目录深冻结（含嵌套 Schema） |
| P2-10 | P2 | V3 容量校验绕过面（truncated 不限片段长；leaf 序号无界） | 已修：validator 补片段 ≤64KiB、未截断不得带 original_bytes、leaf_id ∈1..=31 |
| P2-11 | P2 | 单次 text()>512KiB 分块后以多条 output 交付（调用边界破坏） | 已修：cont 延续帧＋宿主 merge_last 按调用边界合并 |
| P2-12 | P2 | CellHandle 无 Drop 兜底（遗漏出口时 worker 泄漏） | 已修：Drop 未 closed 时组杀回收（事件链以孤儿 start 呈现「结果未知」） |

## 审查确认无问题的关键面
越权/凭据/IPC（冻结目录精确名、env_clear、坏帧断开、worker_exe 不经 env）、沙箱与进程（033 包装 fail-closed、killpg、RLIMIT 三层）、独占批与全部轮出口收尾（28 处 close_orch 无遗漏）、观察循环单点驱动与串行通道、V3 事件投影全过滤（含五协议 native 适配与记忆提炼）、参数契约与错误码语义、双 opt-in 装配（关闭路径逐字节不变）。

## 复验
修复后 js_orchestration 28/28 通过（含 3 项新增回归）；全量回归见 2026-10-29-final-hashes.md 更新。
