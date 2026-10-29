# 030 变异反证记录（comprehensive-testing / S19）

- 日期：2026-10-29；工作目录 `/workspace`；分支 `feat/030-tool-search`。
- 方法：保持断言不变，对关键防线做定向可逆突变，确认对应用例红、还原后绿（每轮单测定向运行）。

| # | 突变 | 应红用例 | 实测 |
|---|---|---|---|
| 1 | 移除请求级执行集合门（分派仅按快照命中，`declared_dynamic` 过滤删除） | `ts30_same_batch_guess_zero_calls`、`ts30_unsearched_guess_zero_calls` | 双双红（同批猜名/未搜索猜名获得执行）→ 还原绿 |
| 2 | 同分排序顺序颠倒为先截断后排序（跨 limit 边界候选丢失） | `frozen_lexicon_ordering`（同分 10 项取 3 的身份序断言） | 红 → 还原绿 |
| 3 | 移除 128 声明预算检查 | `ts30_declared_budget_over_128_fails_whole_search` | 红（第 5 组搜索未整体失败）→ 还原绿 |
| 4 | 目录容量检查短路（条目数与 10 MiB 字节检查均放行） | `catalog_entry_count_boundary`、`catalog_json_size_boundary` | 双双红 → 还原绿 |

还原后 `tool_search` 与 `mcp_turn` 全绿（记录时 15/26；修复轮补测后 tool_search 为 20，计数漂移系后续增例，变异结论不受影响）。变异脚本与备份在 `/tmp/ts30-mutations/`（临时，不入仓库）。
