# 030 工具搜索与按需加载：规格依据

- 日期：2026-10-26；工作目录：`/workspace`。
- 授权范围：用户同意继续原表下一项。本轮仅建立「工具搜索与按需加载」的[规格与实施计划](../../specs/030-tool-search.md)，不实施代码，不处理下一项「权限申请」。
- 状态：待实现；来源为当前工作区源码静态核对与固定上游参考，本轮没有运行测试、性能原型或真实模型服务。

## 当前实现与约束

| 核对点 | 当前证据 | 对 030 的影响 |
|---|---|---|
| 原表项目 | [对照记录](../../records/2026-10-21-codex-tools-comparison.md)中独立一行「工具搜索与按需加载」 | 搜索与按需声明属于同一任务，共用规格和实施计划；不再按子能力拆项 |
| 模型协议 | [024](../../specs/024-native-model-apis.md)已具备五协议普通函数转换；[native.rs](../../../src/app/src/api/native.rs)、[Anthropic](../../../src/app/src/api/native/anthropic.rs)、[Gemini](../../../src/app/src/api/native/gemini.rs)逐次读取当前 request.tools | 以普通函数 route 共用现有转换，不新增原生搜索 item；不能沿用「仅 Chat Completions」的历史前提 |
| 目录快照 | [manager.rs](../../../src/app/src/mcp/manager.rs)的 McpSnapshot 保存动态 entries、全部 definitions 及独立 resources；entries 按 server_id/raw_tool 排序，ToolEntry 含完整 Schema 和连接身份 | 可以复用快照构建本地索引，不另建跨轮目录、不根据名字拆分服务器路由 |
| 当前声明 | [prompt.rs](../../../src/app/src/prompt.rs)的 build_turn_request_with_mcp 每次追加整个 definitions 切片 | 需要区分常驻资源与动态工具，并为每次请求提供累计已加载定义；关闭开关时仍走原路径 |
| 当前执行 | [turn.rs](../../../src/app/src/turn.rs)只要当前 MCP 快照 entry 命中就能调用，不检查该响应对应的 tools 数组 | 不能只减少发送定义；必须在动态分派前检查本次模型请求固定的执行集合，防止猜名和同批搜索后直接调用 |
| 数量限制 | [config.rs](../../../src/app/src/mcp/config.rs)128 工具；[connection.rs](../../../src/app/src/mcp/connection.rs)每服务器 100 页/2048 项/64 KiB 游标；024 实际请求 10 MiB | 128 继续约束实际声明；本项另定跨服务器索引总量，明确哪些数值是新增行为，不把 per-server 边界误写成原有全局边界 |
| 兼容开关 | [catalog.rs](../../../src/app/src/catalog.rs)已有 experimental_supported_tools，021 已以精确名称决定消息工具；[main.rs](../../../src/app/src/main.rs)与 [tui_host.rs](../../../src/app/src/tui_host.rs)每轮传 TurnExtras | 复用开关字段和宿主入口，旧入口默认关闭；不能按模型名字推测支持 |
| 目录安全 | [catalog.rs](../../../src/app/src/mcp/catalog.rs)按去重/Schema/污染/名单发布；[secrets.rs](../../../src/app/src/mcp/secrets.rs)提供每连接敏感值检查 | 新增 raw server_id 来源概览和检索文本同样先检查；不因为原函数定义已检查就漏掉新增字段 |
| 测试落点 | [mcp_turn.rs](../../../src/app/tests/mcp_turn.rs)是真实 turn＋stdio；[native_model_apis.rs](../../../src/app/tests/native_model_apis.rs)是真实协议＋本地 HTTP；[E2E](../../../src/app/tests/e2e_acceptance.rs)有真实 CLI/PTY；[会话](../../../src/app/tests/session.rs)验证工具历史 | 复用系统边界夹具，搜索/加载/授权逻辑使用真实实现；不通过 mock 核心状态证明自身正确 |

当前工作区已有多处未提交代码和配置改动，030 本轮只新增文档、更新原表本行及项目经验指针，保留其余改动。029 的代码尚未实施；用户同意推进文档任务，不等于该功能已经实现或验收通过。

## 固定上游与库依据

通过 GitHub 连接器按 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4` 实际读取，不以 HEAD 代替：

- [tool_search_spec.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/tool_search_spec.rs)：query、可选 limit、来源概览和 next model call 的接入语义。
- [tool_search.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/tool_search.rs)：BM25 Embedder/Scorer、English 词法、目录索引、空查询/limit 校验与命中才物化定义。
- [MCP 检索文本](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/mcp.rs#L593)：名称、服务器、说明和排序后的顶层参数名。030 不引入本项目尚无的连接器/插件字段。
- [工具定义保留](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tools/src/tool_search.rs)：宿主保留共享目录与命中后定义；上游原生加载包装不等于五协议都可直接使用。
- [Cargo.toml](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/Cargo.toml)明确 bm25 2.3.2；[该版本 Cargo 元数据](https://docs.rs/crate/bm25/2.3.2/source/Cargo.toml.orig)及[库文档](https://docs.rs/bm25/2.3.2/bm25/)已实际打开，核实 MIT、默认 tokenizer、Unicode 词边界/归一化和按语料拟合接口。当前项目未直接使用 bm25，本轮不安装依赖。
- [OpenAI Docs：Tool search](https://developers.openai.com/api/docs/guides/tools-tool-search)已实际打开；其托管/client-executed 区分和原生 item 仅用于路线对照。030 普通函数方案不借用其原生缓存收益或模型支持声明。

## 规格取舍及未测事项

具体冻结的要求统一见规格，不在本记录复制接口或验收表。关键设计是：仅延迟动态 MCP 工具、默认关闭、每用户轮重建加载集合、每次模型请求固定调用权限、Schema 全量保留、搜索不产生外部业务调用。

新边界是提案中的行为约束，不是测得的容量或性能：4096 字节 query、最多 32 项结果、1 MiB 结果及跨服务器目录总量均在规格中明确标为新增；8 项默认、128 声明和原有 10 MiB 请求分别有上游/本地依据。

速度与 RSS 尚无该功能基线，规格要求 M0 用固定版本和夹具先测最小原型，再冻结 2–3 倍余量阈值；当前没有擅填绝对数字。没有真实服务用量、token、价格或费用结果；不能把字节节省或无新增网络请求写成免费或缓存命中。

## 文档验证

本轮只检查范围、编号、规格五节、实施计划、未来用例保持待测、本地链接和空白，不运行 Rust 构建、代码检查或功能测试。实际命令、工作目录、退出码及原始输出见 2026-10-26-doc-check.log。
