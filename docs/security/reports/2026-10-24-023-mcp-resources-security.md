# 023 MCP 资源面安全测试报告

- 日期：2026-10-24。
- 范围：023 MCP Resources（`src/app/src/mcp/resources.rs`＋connection 资源请求＋manager 快照/分派＋turn 资源工具接入；测试 `src/app/tests/mcp_resources.rs`、`mcp_http.rs` 资源用例、`e2e_acceptance.rs` s105）。
- 方法：[Security-Testing-Gate](../../../.agents/agents/Security-Testing-Gate.md) 两步法——先以攻击者视角审查实现，再用受控本地夹具实测；不访问真实账号或第三方端点。凭证为测试 canary（`tchat-mcp-canary-022-secret`）。
- 规格：[023-mcp-resources](../../specs/023-mcp-resources.md) §5.4-2 攻防范围：服务器身份伪造、停用绕过、URI 本地打开、认证／blob 污染、畸形分页、超大输入、取消与请求重放。

## 1. 代码审查结论（攻击者视角）

| # | 攻击面 | 审查结论 | 风险 | 证据 |
|---|---|---|---|---|
| S1 | 服务器身份伪造／跨台串路由 | read/list 按快照 `(server)` 精确命中（`resources::find_server` 按 ID 精确匹配）；连接句柄、凭证、敏感值集合来自连接准备时冻结的快照，模型 arguments 只含业务字段，不能改变路由或身份 | 低 | 实测 T2 |
| S2 | 停用／未配置服务器绕过 | 停用条目不连接、不进资源快照；对它的调用在分派前按未知拒绝，零进程启动零请求 | 低 | 实测 T1 |
| S3 | URI 被本地打开（file://／远程 URL） | `resources.rs` 无任何文件／网络 API（结构性审查：无 `std::fs`／`reqwest`／`Command`）；URI 是 opaque 字符串，仅原样送达指定服务器解释 | 低 | 实测 T3＋结构审查 |
| S4 | 凭证污染（正文／blob 解码／元数据） | 读取结果逐内容项检查：text／blob 原文、base64 解码后字节（`contains_secret_bytes`）、`_meta` 逐值，再检查投影 JSON——仅扫序列化字符串会漏掉的 base64 编码凭证已覆盖；命中整份替换标准客户端错误，不入史／不入下一请求／不展示。定向与汇总路径都检查 | 低 | 实测 T4／T5＋M2-3 RED（blob 缺口曾真实存在并被检出后修复） |
| S5 | 畸形分页／响应 | 非数组 resources、非法结构由 SDK 反序列化失败 → 客户端协议错误（不崩溃、不伪造成功）；汇总中该台记 `errors`（R3），定向返回协议错误 | 低 | 实测 T6 |
| S6 | 超大输入（内存放大） | 单条传输消息 8 MiB（022 有界接收沿用）；工具结果 JSON 8 MiB（R2=B 三工具统一）；单页 2048 项、汇总每台 100 页／2048 项／64 KiB 游标 | 低 | 实测 T7＋`m3_page_limits`／`m3_aggregate_budget` |
| S7 | 取消与请求重放 | 发送前取消零请求；执行中取消立即停等＋有界取消通知（≤1s）＋已收集数据不发布；超时／断连后不自动重试（record 计数恰一次）；迟到结果随请求句柄丢弃，无第二条 tool 结果 | 低 | 实测 T8／T9／T10 |
| S8 | 游标注入（opaque 篡改） | 游标原样传递（含空字符串、空白、控制字符），客户端不解析、不 trim、不当结束；超 64 KiB／重复游标拒绝；`cursor` 无 `server` 是发送前参数错误 | 低 | `m3_cursor_with_whitespace`／`m1`（空游标）／`m3_page_limits` |
| S9 | 错误信息泄漏 | 部分失败 `errors[].error` 与客户端错误先经敏感值检查，命中替换通用诊断；不回显凭证、认证头或指纹 | 低 | `resources.rs::sanitize_detail`＋T4/T5 断言 canary 不出现 |
| S10 | 内部记忆轮提权 | 合并轮 `mcp_snapshot` 强制置 None（022 双层隔离沿用），资源工具不声明不执行；伪造调用走未知工具通道 | 低 | `m2_memory_merge_turn_never_executes_resource_tools`＋变异验证 |
| S11 | 恢复重放 | `/resume` 只展示历史；恢复不触达 MCP 管理器（022 结构性保证）；资源读取／目录零重放、零预读 | 低 | 实测 T11（e2e s105） |
| S12 | 反向请求提权（sampling／roots／elicitation） | 客户端能力与 022 相同（不声明三项）；资源请求不引入新的反向请求面 | 低 | 022 T14 沿用（connection handler 未变） |

审查中未发现高风险漏洞。

## 2. 实测记录（受控本地夹具）

全部为自动化回归用例，随 `cargo test --workspace` 默认运行：

| # | 攻击动作 | 实测结果 | 用例 |
|---|---|---|---|
| T1 | 对 `enabled:false` 服务器调 `read_mcp_resource` | 未知拒绝；record 文件不存在（零进程零请求） | `sec_disabled_server_resource_calls_fail_with_zero_requests` |
| T2 | 两台同 URI，指定 `fx` 读取 | 恰一次请求且只抵达 fx；结果归属 fx | `m6_same_uri_routes_only_to_specified_server` |
| T3 | `file:///etc/passwd` URI 读取 | URI 原样抵达服务器；返回夹具文本而非本地文件内容（宿主未读文件） | `sec_uri_is_never_opened_locally_only_forwarded` |
| T4 | 定向列举资源描述回显 canary | 整份拒绝；canary 不入历史／下一请求 | `sec_directed_polluted_list_is_rejected` |
| T5 | 读取正文／base64 blob／`_meta` 回显 canary（含汇总任一台污染） | 全部整份拒绝；干净台数据也不发布；canary 不出现在任何出口 | `m2_polluted_resource_results_are_rejected_wholesale`／`m6_polluted_aggregate_fails_wholesale` |
| T6 | `resources` 返回非数组 | 协议错误按部分失败记 errors，本轮继续；不崩溃不伪造成功 | `sec_malformed_list_response_fails_as_protocol_error` |
| T7 | 单条读取结果 9 MiB | 有界接收拒绝并关闭连接；标准客户端错误；不截断成成功 | `sec_oversize_read_is_rejected_by_transport_bound` |
| T8 | 汇总进行中取消（含已成功台数据） | 整次失败、已收集数据不发布、补齐配对、串行后续台零请求 | `m4_cancel_during_aggregate_fails_wholesale_and_keeps_batch_pairing`／`m6_cancel_outranks_partial_failure_with_collected_data` |
| T9 | 定向列举超时（服务器挂起） | 状态未知错误；请求恰一次（不自动重试） | `m4_directed_list_timeout_reports_unknown_state` |
| T10 | 超时后服务器迟到回复 | 无第二条 tool 结果；连接保留，下一轮正常；总请求数恰两次（无重放） | `m4_late_result_after_timeout_is_discarded_and_not_replayed` |
| T11 | 含资源调用的 V1 会话 `/resume`（配置仍在） | 恢复零 MCP 启动、零资源请求、零目录预读；新进程新提交才取当前能力 | `e2e_acceptance::s105_mcp_resources_resume_replays_nothing` |

## 3. 变异敏感性证明（安全相关）

| 变异 | 检出用例 |
|---|---|
| 移除 blob 解码后的字节级污染检查 | `m2_polluted_resource_results_are_rejected_wholesale`（c2，RED 阶段即真实缺口） |
| 移除汇总污染检查 | `m6_polluted_aggregate_fails_wholesale` |
| 取消降级为单台部分失败 | `m6_cancel_outranks_partial_failure_with_collected_data` |
| 分页预算超限时输出半份目录 | `m3_page_limits_reject_the_server_not_the_whole_call` |
| 绕过未知服务器检查／绕过记忆轮隔离 | `m2_read_argument_errors_fail_before_sending`／`m2_memory_merge_turn_never_executes_resource_tools` |

全部变异验证后恢复原实现并复验通过。

## 4. 结论

023 资源面在规格定义的攻防范围内未发现高风险漏洞：路由与身份由冻结快照决定、URI 不被本地解释、凭证污染在所有出口前拒绝、容量与超时有界、取消与失败不重放。所有实测均为可重复的自动化回归用例。
