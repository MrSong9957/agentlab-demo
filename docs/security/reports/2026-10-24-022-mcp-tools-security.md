# 022 MCP 外部工具安全测试报告

- 日期：2026-10-24。
- 范围：022 MCP 外部工具接入（`src/app/src/mcp/`＋turn/prompt/main/tui_host 集成）。
- 方法：[Security-Testing-Gate](../../../.agents/agents/Security-Testing-Gate.md) 两步法——先以攻击者视角审查实现，再用受控本地服务实测；不访问真实账号或第三方端点。凭证全部为测试 canary（`tchat-*-canary-022-secret`）。
- 规格：[022-mcp-tools](../../specs/022-mcp-tools.md) §5.2 定义的本地边界攻防范围：越权分派、认证／凭证污染、参数与终端注入、重定向、超限、取消、跨轮快照、进程回收。

## 1. 代码审查结论（攻击者视角）

| # | 攻击面 | 审查结论 | 风险 | 证据 |
|---|---|---|---|---|
| R1 | 越权分派（模型诱导宿主改路由） | 连接 URL／凭证／服务器身份全部来自用户 mcp.json 与每轮冻结快照；分派只按快照精确命中（`McpSnapshot::entry`），不从模型 arguments 解释任何连接配置。模型无法让宿主换服务器、换凭证、改端点 | 低 | `manager.rs`（`ToolEntry` 只携带连接句柄）；`turn.rs` MCP 分派分支 |
| R2 | 越权分派（伪造名／大小写变体／空白名） | 快照映射精确匹配；`MCP__FX__ECHO`、` mcp__fx__echo`、`mcp__fx__echo `、`mcp__fx`、`mcp__fx__ghost` 全部落入既有未知工具通道，零外部调用 | 低 | 实测 T1 |
| R3 | 绕过名单／停用项执行 | 发布前按原始名过滤（允许名单缺省＝全部、显式空＝零、禁用优先）；未发布名称不可能命中快照。变异验证：忽略名单的变体被 A07 用例检出 | 低 | 实测 T1＋变异 M3 |
| R4 | MemoryMerge 内部轮提权 | 合并轮强制忽略快照与消息能力（`merge_mode` 双层隔离），即使误传也不声明不执行。变异验证检出 | 低 | 实测 T2＋变异 M4 |
| R5 | 凭证泄漏（日志／会话／展示） | 凭证只在连接准备时从环境变量解析；错误文本、状态行、投影结果先做敏感值包含检查，命中即替换通用诊断。变异验证：跳过污染检查被 A16 检出 | 低 | 实测 T3／T4＋变异 M10 |
| R6 | 凭证泄漏（未引用变量） | 环境白名单＝平台缺省＋显式引用；未引用的宿主变量不转交子进程。实测断言 POISON 键不在子进程环境 | 低 | 实测 T5 |
| R7 | 认证降级 | 配置了凭证但缺失／空值 → 该服务器准备失败，不降为匿名；零网络请求 | 低 | 实测 T6 |
| R8 | HTTP 重定向泄漏认证头 | reqwest `redirect::Policy::none()`（移植 SDK 默认配置）；302 原样返回失败，目标监听器零访问 | 低 | 实测 T7 |
| R9 | 404 会话失效隐式重放（多发写操作） | `reinit_on_expired_session = false` 显式关闭；404 → 调用失败，服务器端调用账本恰一次、无重初始化。变异验证：打开重放被 A12 检出 | 低 | 实测 T8＋变异 M8 |
| R10 | 参数注入（协议字段伪造） | `CallToolRequestParams` 由宿主构造（name／arguments 来自映射），`_meta` 不从模型 arguments 解释；业务字段原样透传（规格允许） | 低 | `connection.rs::call_tool_once` |
| R11 | shell／命令注入（配置 args） | stdio 直接按程序＋参数数组启动（`process_group(0)`），不经 shell；`;`／反引号／`$()` 按字面到达服务器 argv | 低 | 实测 T9 |
| R12 | 终端注入（结果／stderr 转义序列） | CLI 一切输出经 `display::sanitize_for_terminal`（say 统一入口）；TUI 走既有过滤展示层；stderr 尾部展示前脱敏＋控制字符过滤 | 低 | `main.rs` say；`manager.rs::sanitize_diagnostic` |
| R13 | 接收超限（内存放大） | stdio 单条 JSON、HTTP JSON 体、单 SSE 事件均 8 MiB 上限，在完整分配／解析前中止并关闭连接；结果不截断成成功。变异验证：放大限额被 A15 检出 | 低 | 实测 T10／T11＋变异 M11、M15 |
| R14 | 发送超限（10 MiB） | 模型请求完整序列化后由 api 层 10 MiB 计量拒绝（既有契约）；MCP 定义 128 项上限在请求前检查 | 低 | `api.rs`（既有）＋`turn.rs` 总量检查 |
| R15 | 取消语义伪造（伪称未执行／重复入史） | 取消＝本地等待立即结束＋有界取消通知（≤1s）；已发调用一律「状态未知」；迟到结果随 oneshot 句柄丢弃，无第二条 tool 结果。变异验证：取消后等待迟到结果的变体被 A11 检出 | 低 | 实测 T12＋变异 M9b |
| R16 | 断连重放 | 断连后连接标记关闭，下一轮不发布工具；副作用保留（写文件实测），无自动重试 | 低 | 实测 T13 |
| R17 | 反向请求提权（sampling／roots／elicitation） | 客户端不声明三项能力；roots／elicitation 覆盖 SDK 默认值统一 method-not-found；不递归调模型、不读工作区、不开问询 | 低 | 实测 T14 |
| R18 | 跨轮快照错绑 | 快照不可变（定义＋映射＋连接句柄冻结）；断连连接不改绑、不重连（仅显式 reload）；重列不重建连接 | 低 | 实测 T15 |
| R19 | 进程回收遗漏（孤儿／孙进程） | stdio 独立进程组（`process_group(0)`）；有界关闭（stdin EOF → 3 秒等待 → SIGKILL 进程组）；Drop 后备按组终止；实测 `/mcp reload` 停用后 PID 消失 | 低 | 实测 T16（e2e s103） |
| R20 | 恢复重放副作用 | `/resume` 只展示历史；恢复路径不触达 MCP 管理器（结构性保证）；无配置时旧工具记录照读、新提交才取当前能力 | 低 | 实测 T17（e2e s104） |
| R21 | 配置注入（服务器返回内容改配置） | 配置只在加载／reload 时从用户文件解析；服务器返回的目录／结果／instructions 不回写配置，也不提升到 system 提示 | 低 | 结构性（无回写路径） |

审查中未发现高风险漏洞。

## 2. 实测记录（受控本地服务）

全部为自动化回归用例，随 `cargo test --workspace` 默认运行；以下为安全角度的汇总索引：

| # | 攻击动作 | 实测结果 | 用例 |
|---|---|---|---|
| T1 | 伪造名（含大小写／空白／`__` 变体）5 连发 | 全部未知工具错误，record 零外部调用 | `mcp_turn::a07_forged_names_fail_without_external_calls` |
| T2 | MemoryMerge 轮误传 MCP 快照＋伪造调用 | 请求只含 mem 三工具；伪造调用失败；零外部调用 | `mcp_turn::a07_memory_merge_turn_never_declares_or_executes_mcp` |
| T3 | 服务器在工具描述回显 Bearer canary | 污染定义不发布；状态显示通用原因，不回显值 | `mcp_stdio::a16_polluted_tool_definition_is_not_published` |
| T4 | 服务器在结果文本回显 Bearer canary | 投影替换为标准客户端错误；canary 不出现在结果、历史、展示 | `mcp_stdio::a16_polluted_result_replaced_with_client_error` |
| T5 | 宿主环境含未引用 POISON 键 | 子进程环境无该键；仅白名单＋显式引用转交 | `mcp_stdio::a03_env_whitelist_explicit_vars_pass_others_blocked` |
| T6 | bearer_token_env_var 指向缺失变量 | 服务器不可用、报「凭证缺失」、零网络请求（监听器零访问） | `mcp_http::a04_missing_credential_fails_without_anonymous_fallback` |
| T7 | 端点返回 302 到第二监听器 | 连接失败；目标监听器零请求；认证头未重放 | `mcp_http::a04_redirects_are_not_followed` |
| T8 | tools/call 一律 404（会话失效） | 客户端失败；initialize 恰 1 次、call 恰 1 次（无重放） | `mcp_http::a12_session_expired_404_fails_without_reinit_replay` |
| T9 | 配置 args 含 `;`／反引号／`$()` | 服务器 argv 收到字面参数；无 `/bin/sh` | `mcp_stdio::a03_shell_metacharacters_passed_verbatim_no_shell` |
| T10 | stdio 返回 9 MiB 单行结果 | 连接关闭、调用失败、不截断成成功 | `mcp_stdio::a15_oversize_line_closes_connection_without_success` |
| T11 | HTTP 9 MiB JSON 体／9 MiB SSE 事件 | 有界读取／事件限额拒绝，不完整分配 | `mcp_http::a15_oversize_json_body_rejected`、`a15_oversize_sse_event_rejected` |
| T12 | 调用中用户取消（服务器 3 秒后仍回包） | 立即返回「状态未知」；迟到结果不入史；record 恰一次调用 | `mcp_stdio::a11_cancel_during_call_marks_unknown_and_drops_late_result` |
| T13 | 服务器写文件后退出再调用 | 首副作用保留、第二次调用失败（状态未知）、无重放、下轮不发布 | `mcp_stdio::a12_server_death_after_side_effect_marks_unknown_once` |
| T14 | 服务器反向 sampling／roots／elicitation | 统一 method-not-found；调用正常完成；模型不被调用、工作区不被读 | `mcp_stdio::a08_reverse_requests_get_method_not_found` |
| T15 | 目录变化通知后跨轮 | 本轮快照不变；下一轮完整重列且连接复用 | `mcp_stdio::a13_list_changed_relists_on_next_turn` |
| T16 | reload 停用后旧进程回收 | PID 消失（/proc 探测）；停用后原生对话照常 | `e2e_acceptance::s103_mcp_reload_disables_and_exit_reaps_process` |
| T17 | 恢复会话（历史含 MCP 调用、无配置） | 恢复零启动／零重放；旧记录照读；新请求不声明 MCP | `e2e_acceptance::s104_mcp_resume_replays_nothing_and_new_turn_uses_current_caps` |

## 3. 变体验证（关键用例能检出对应错误，规格 §5.2）

隔离副本可逆变异（应用变体 → 目标用例失败 → 还原字节一致 → 用例恢复通过）：

| 变异 | 检出用例 | 结果 |
|---|---|---|
| 遗漏实际调用（分派返回假成功） | mcp_turn a07_declaration | 检出 |
| 错误身份映射（模型名替代原始名调用） | mcp_stdio a03 | 检出 |
| 绕过名单（忽略 enabled_tools） | mcp_stdio a07_allowlist | 检出 |
| 绕过 MemoryMerge 隔离 | mcp_turn a07_memory_merge | 检出 |
| isError 画成功（投影丢弃 isError） | mcp_turn a09_is_error | 检出 |
| 只读第一页（忽略 next_cursor） | mcp_stdio a05_multi_page | 检出 |
| 漏判目录过期（永不重列） | mcp_stdio a13_list_changed | 检出 |
| 404 隐式多发（reinit 打开） | mcp_http a12_session_expired | 检出 |
| 取消后等待迟到结果并入史 | mcp_stdio a11_cancel | 检出 |
| 凭证污染结果透传（跳过检查） | mcp_stdio a16_polluted_result | 检出 |
| stdio 接收限额失效 | mcp_stdio a15_oversize_line | 检出 |
| HTTP JSON 体限额失效 | mcp_http a15_oversize_json | 检出 |
| 非 complete 响应当成功（实际路径） | mcp_turn a09_unsupported | 检出 |
| Schema 校验跳过 | mcp_stdio a08_bad_schema | 检出 |
| 重复原名覆盖注册表（去重跳过） | mcp_l1 naming_duplicate | 检出 |

恢复原始字节后核对一致；恢复后受影响用例全部通过。

## 4. 已知限制与剩余风险

1. **本地信任边界**：stdio 服务器以宿主用户身份运行，环境白名单只控制自动转交的数据，不构成进程隔离（规格 §2.1 既定边界）。启用即授权该服务器的筛选后工具调用。
2. **内存放大（低）**：单 Schema 可达 8 MiB（消息上限），128 项理论上限约 1 GiB 目录内存；完整请求仍受 10 MiB 拦截。首版接受（本地受信服务器）；后续可加单项 Schema 尺寸上限。
3. **Windows**：ConPTY／进程对象句柄验证缺平台条件，列为未运行（与规格一致）；Unix 进程组回收已实测。
4. **HTTP 并发上限**：SDK 默认 16 并发 POST；本项目分派为串行（服务锁），未暴露并发面。
5. **凭证指纹**：不输出指纹（规格 §2.2 身份比较仅在内存）；报告与日志均不含 canary 原文。

## 5. 结论

代码审查未发现高风险问题；17 项攻击面实测与 15 项变体验证全部通过。安全门禁通过，无范围内未修复项。

## 6. 补证轮复验（2026-10-24 第二轮，验收补证）

- A18 HTTP 在途退出实测（`a18_http_inflight_close_bounded_late_result_discarded`）通过；过程中发现并修复 close() 后自动重连缺陷（R19「进程回收遗漏」的连接维度延伸——关停后不得复活连接）。修复后复跑：mcp_http 11、mcp_stdio 31、mcp_turn 13、e2e 142 全绿。
- close() 增加 operate 锁（增量审查风险 1）：关停与 prepare/reload 互斥，消除「建立中的连接被漏关」竞态。无新攻击面（关停路径不含不可信输入）。
