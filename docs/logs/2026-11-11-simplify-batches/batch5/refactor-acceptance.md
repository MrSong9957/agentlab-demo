# 批 5.1：重构组（S11–S30、S32）验收记录（2026-11-11）

对象：提交 `a6568c4`（30 文件，+1012/−1182）。

## 逐条核对（清单处理记录 ↔ 实际 diff）

20 条全部在 diff 中对应到各自机制，无漏项；改动文件集（session/agents×4/
proc_session/sandbox/view_image/apply_patch/api×2/mcp×7/memory×2/prompt/turn/
tui_host/main/catalog/display/exec/permissions/js_orchestration×2/hex 新建）与
各条声明范围一致，无越界文件。S27（前批 [x]）不涉及。

## S17 顺序专项（发现并修复 1 处误替换）

- 改前（a6568c4^）「先 close_orch」站点实数＝**4 处**（:1811/:1827/:1837 三×
  MODEL_ERROR＋:1874 TASK_COMPLETED「无 wait 终文」）——与清单 4 处吻合。
- a6568c4 的 diff 含 **5 个** orch_first——第 5 个（root_final_with_active_children，
  agents 侧收尾）为**误替换**：该站原为 `runtime.close_for_turn_exit`（agents 的
  非 close_orch!）＋steer→pending 且**不动 orch_cell**；误用宏会在 orch_cell
  有值时额外关闭编排 cell（行为变化）。**已修复**：还原显式两行（本文件同
  提交）。修复后 orch_first 调用恰 4 处，逐站点与改前顺序一致。
- 其余三×MODEL_ERROR 站点（wait_agent 批尾/exec_wait 独占批/活动 cell 普通批）
  逐站点与改前顺序一致。
- 修复后重验：turn lib 18/0、llm_flow 51/0、e2e_acceptance 150/0。

## S28 行为不可观察核对

- S28.1 选择性克隆：非对象 request 沿原 clone 路径；messages 覆盖语义等价
  （llm_flow 51/0）。
- S28.2 PendingBuffer：take() 头＋省略标记＋tail[offset..] 与原串接逐字节一致
  （proc_session 24/0 含截断断言）。
- S28.3 快照缓存：Arc 身份不可观察；断连原子量每轮仍经 §5.1 块重查（mcp 四
  套件全绿）。

## 警告基线

580（604−25＝close_orch! 宏体 never-read 假阳性统一抑制）——本批 S17 修复后
仍 580，零新增。
