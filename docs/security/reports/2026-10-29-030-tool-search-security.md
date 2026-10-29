# 安全攻防测试报告：030 工具搜索与按需加载（tool_search）

- 日期：2026-10-29；被测对象：`src/app/src/tool_search.rs`、`turn.rs`（开关/装配/分派门）、`prompt.rs`（声明组装）、`mcp/manager.rs`（只读拆分访问）、宿主接线（`main.rs`/`tui_host.rs` 展示行）
- 规格：`docs/specs/030-tool-search.md`（§1.2/§1.3/§1.4/§1.5/§1.6）；关联 022 目录发布、023 资源、024 五协议、025 脱敏
- 方法：先代码审查定位攻击面，再以真实代码路径实测（`tests/tool_search.rs` 的 `sec_*` 3 项＋边界用例、`tests/mcp_turn.rs` ts30 系列 13 项真实 turn＋stdio MCP、`tests/mcp_http.rs` HTTP 腿、e2e s30 真实 CLI/PTY、变异反证 4/4）；`env -u TCHAT_SANDBOX`；独立临时目录与假密钥 `SECRET-KEY-TS30`
- 执行身份：uid 1000(dev)，容器 Linux

## 结论概要

8 类攻击矩阵（按门禁清单对本地 CLI 形态适配）全部实测。**核心防线全部有效**：参数严格白名单（手写 visitor——重复键/数组形式/未知字段/null/浮点全拒；serde 派生会接受数组位置形式、Map 会静默合并重复键，两者均不可用）、执行资格只来自本次请求声明集合（同批猜名/未搜索猜名/其他轮名称零业务调用，变异反证 1）、搜索零网络零外部调用、被排除与敏感条目不进概览/索引/结果、失败文案不回显输入、容量四边界（2048 项/10 MiB 目录/1 MiB 结果/128 声明）全部生效、8 请求/32 调用/批上限维持。**未发现高危或中危问题**；2 项低危观察（见 O-1/O-2）。

## 攻击矩阵与实测

| # | 类别（门禁） | 攻击手法（030 适配） | 实测防线 | 结果 |
|---|---|---|---|---|
| 1 | 数据越权 | 查询被禁用/被排除/被污染工具的准确名以图取回定义；命中已排除条目 | 快照层 022 排除（mcp_l1 既有）＋030 层 server_id/检索文本敏感值整项排除（`sensitive_sources_excluded_from_everything`）；概览计数只含干净项 | 拦截 ✓ |
| 2 | 功能越权 | 未声明模型的伪造 tool_search；同批先搜索后猜名；未搜索直接猜名；内部记忆轮伪造搜索 | `ts30_fake_search_unavailable`、`ts30_same_batch_guess_zero_calls`、`ts30_unsearched_guess_zero_calls`（请求集合门，变异反证 1）、memory_phase2 扩展（未知工具通用错误） | 拦截 ✓ |
| 3 | 身份认证（适配：声明资格） | 无开关/开关名不完全匹配/空目录仍想搜索 | `supports_tool_search_exact_name_only`（精确名）、`ts30_empty_dynamic_catalog_no_declaration`（空目录不声明＋伪造不可用） | 拦截 ✓ |
| 4 | 偷改字段提权 | query/limit 重复键、显式 null、浮点/布尔/字符串 limit、未知字段（含 server/auth/path/role 注入字段）、数组位置形式 | `sec_arguments_reject_duplicate_and_malformed_keys`＋`arguments_shape_and_limits`（手写 visitor：仅 map、逐键、重复即拒） | 拦截 ✓ |
| 5 | 恶意输入注入 | 指令样查询文本、JSON 伪装、标记注入；4096 字节边界与多字节截断错位 | `sec_query_injection_is_lexically_inert`（词法惰性、不得放大命中）＋字节边界两口径（恰好/超过）；展示行经宿主终端过滤（TUI `sanitize_for_terminal`/CLI say，代码路径核对） | 拦截 ✓ |
| 6 | 绕过协议层 | 目录命中即调用（绕过声明集合）；恢复历史激活旧加载；同批新命中立即执行 | 请求集合门＋变异反证 1；e2e s30 恢复后新轮 `names3` 断言不继承；`catalog_reorder` 顺序稳定 | 拦截 ✓ |
| 7 | 敏感信息泄露 | 密钥进概览/检索文本/结果/错误文案；来源概览伪造指令样说明 | 敏感值整项排除（概览/索引/结果三不入）＋`sec_failure_errors_never_echo_sensitive_input`（固定文案族）；概览为宿主序列化 JSON 数据块，服务器无法注入结构 | 拦截 ✓ |
| 8 | 资源滥用 | 搜索风暴（单批 32 调用）、目录 2048+1 项、10 MiB+1 字节、结果 1 MiB+1、声明 128+1、第 9 次模型请求 | 批 ≤32 既有；四容量边界精确二分实测；`ts30_eighth_response_search_no_ninth_request`；搜索零额外模型请求（e2e/mcp_turn 计数断言）；预算超限整次失败不淘汰 | 拦截 ✓ |

## 变异反证（S19，与 comprehensive-testing 共用证据）

4 项定向突变全部被关键用例捕获后还原复绿：执行集合门（同批/未搜索猜名双红）、同分先截断（冻结名单红）、128 预算（预算用例红）、目录容量短路（两项边界双红）。见[变异记录](../logs/030-tool-search/2026-10-29-mutation-evidence.md)。

## 低危观察（不构成漏洞，登记备查）

- **O-1【低】搜索结果把服务器描述完整返回给模型属预期数据面**：命中定义含服务器提供的 description/Schema（与 022 全量声明同一暴露面——按需加载不减少该面，仅减少数量）。恶意服务器描述中的指令样文本对模型的潜在影响与既有 MCP 面一致，不新增提升路径；宿主侧展示行只含命中名，不含描述。
- **O-2【低】bm25 词法为 English 固定管线（deunicode 音译）**：中文关键词经音译后同词可命中（夹具冻结），但跨语种同义/翻译不承诺；不构成安全问题，属能力边界（规格 §1.3-3 明示）。

## 报告与证据

- 本报告：`docs/security/reports/2026-10-29-030-tool-search-security.md`
- 攻防与边界用例：`src/app/tests/tool_search.rs`（`sec_*`、边界与冻结夹具）、`src/app/tests/mcp_turn.rs`（ts30 系列）、`src/app/tests/mcp_http.rs`（HTTP JSON 腿）、`src/app/tests/e2e_acceptance.rs`（s30）
- 变异反证记录：`docs/logs/030-tool-search/2026-10-29-mutation-evidence.md`
