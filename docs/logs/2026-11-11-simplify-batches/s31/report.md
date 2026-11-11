# S31 prompt 参数链收敛（批 1，2026-11-11）

## 改动

- `prompt.rs`：删除 `build_turn_request_with_memory` / `_with_async_host` /
  `_with_tool_search` / `_with_agents` 四层包装（13 位置参数、逐层布尔转抄），
  收敛为 `TurnRequestOptions`（`#[derive(Default)]` 开关结构体，12 具名字段）＋
  单一入口 `build_turn_request_options(history, &options)`。工具追加顺序与原
  with_agents 体逐行一致（声明顺序＝既有序列化形态）。`build_turn_request`
  保留（委托 Default）；`append_memory_context`／`build_turn_request_with`
  原样保留（子轮与合并轮仍用）。`#[allow(too_many_arguments)]` 随链消除
  （原 S10 已登记的 turn.rs:2666 同族——本条目隐含目标）。
- `turn.rs`：两处 `build_turn_request_with_agents`（User/UserWithMemory 仅
  memory_context 不同）合一为单个 `TurnRequestOptions` 构造＋match。
- 测试机械同步 3 文件：memory_inject（with_memory→options×3）、
  native_model_apis（with_tool_search→options×5）、prompt_assembly
  （with_async_host×11／with_tool_search×3／with_agents×3 → options）。
  context_management/context_perf 用 `build_turn_request` 零改动。

## 验证

- `cargo check --tests`：0 错误；警告 **580＝基线 580 零新增**（HEAD stash 对照：
  prompt_assembly:479、native_model_apis:2416 两处警告在 S31 前已存在，非本批引入）。
- prompt_assembly 19/0、memory_inject 18/0、native_model_apis 33/0、llm_flow 51/0、
  context_management 43/0、context_perf 1/0、lib prompt 1/0。

## 备注

- prompt_assembly.rs:477 `text`、native_model_apis.rs:2420 `mut` 两个测试内
  警告为存量（HEAD 版 :479/:2416 同在），不在本批范围（S10 低价值清理未触测试文件）。
