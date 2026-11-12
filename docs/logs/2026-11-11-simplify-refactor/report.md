# S11–S30、S32 重构组处理记录（2026-11-11）

性质：代码结构收敛，行为逐字节不变。20 条（S27 前批已完成、S31 按指示跳过）。
分 7 批推进，每条改完跑受影响最窄过滤器，收尾全量验证。

## 分批处置摘要

| 批 | 条目 | 要点 |
|---|---|---|
| A | S30 裸串/同体 helper | 6 处 `"send_message_to_user_async"` → `message_to_user::TOOL_NAME`；catalog 两个 serde 空判 helper → `BTreeMap::is_empty`；main 三处 y/yes 谓词 → `is_affirmative`；未知命令提示串 → `tui_host::SLASH_COMMAND_HINT`（pub，bin 侧经 `terminal_chat::` 引用） |
| A | S18 char boundary | rustc 1.99.1 探针确认 std `floor/ceil_char_boundary` 可用；6 处手写回退循环＋proc_session 私有 fn×2 删除，调用点与 5 处断言改 std（语义等价：len 内截断／超限取 len） |
| A | S19 hex | 新建 `hex.rs`（`hex_encode(&[u8])`）＋lib 注册；6 站点替换（catalog/naming 取前 16 字符、session/runner 取前 16 字节——差异保留在调用方） |
| A | S24 view_image | `to_outcome(Result<(PathBuf, EncodedImage), String>)` 提取，execute/execute_sandboxed 两站点委托 |
| A | S25 utc_basic_now | chrono `from_timestamp(secs,0).format("%Y%m%dT%H%M%S")` 替换手写 civil_from_days（≈22 行）；拆 `utc_basic_from_secs(u64)`＋epoch 0 固定断言（`19700101T000000`）；「无 chrono 依赖」注释删除 |
| A | S20 sent_instructions | `static SENT: OnceLock<String>` 首算缓存＋clone；两条 assert 保留在首算闭包内；输出逐字节相同（确定性纯函数） |
| B | S14 catalog 谓词 | `declares_tool(model, name)` 单点；9 处单名谓词/缺失名查询薄委托；js_orchestration 双 opt-in 与 multi_agent 整组「部分缺≠全缺」语义保留不合并 |
| B | S22 permissions | 确认前一次构造 trial 复用（确认仅只读，两次构造恒等）；`parse_existing_abs_path(field_ctx, raw)` 提取——path_spec/deny_paths 两站共用，错误文案逐字保留 |
| B | S23 sandbox | `whitelist_env(home)` 单点（tool_child_env/permission_child_env 仅 HOME 实参不同）；`apply_spawn_fd_hygiene(cmd, seccomp)` 单点＋3 处 std 站点委托（mcp/transport 的 tokio Command 站点类型不同，保留原接线——清单「可选」项）；收口语义未削弱 |
| B | S32 跨模块符号 | agents/runtime `with_native` 逐字副本删除 → 复用 `turn::with_native`（提 pub(crate)）；events `valid_state_name` 词表 → `AgentState::from_display` 单源（as_str 互逆）；api 四协议裸串 → `PROTOCOL_ANTHROPIC/GOOGLE_INTERACTIONS/GOOGLE_GENERATE` 常量（api.rs 9 处＋native.rs 14 处；openai-chat 沿用 `SUPPORTED_PROTOCOL`）；`ParsedToolCall` → `pub use ToolCallRecord` 别名（恒等映射搬运×2 消除：parse_batch 直返、入史 `calls.clone()` 直用——外部 main/tui_host/user_input 的 `turn::ParsedToolCall` 引用零改动） |
| C | S15 mcp | `CallFailure::to_client_error(sensitive)` 单点（connection 已依赖 secrets 无环；resources 本地 fn 删、manager 内联 match 改委托）；`fresh_server_state` 单点（装载/兜底装载两站点）；reload 断连标记复用 `note_connection_down`；`upgraded_manager` 弱引用解析单点（call/资源桥两站点） |
| C | S16 api | `with_auth_headers` 单点（正式发送＋list_models 两站；anthropic-version 字面量收敛）；`unsupported_protocol` 单点（5 处文案逐字一致）；native.rs `textualized_calls`/`textualized_result` 单点（跨来源续接与不可表达降级共用；标题与 pending 语义差异留站点） |
| D | S21 上下文重复构建 | tui_host：`agents_context_message` 移入 `agents_ready` 分支并复用本轮 `context_sources`；main：同款移入分支（产物仅 AgentExtras 消费；纯函数无副作用） |
| D | S28.1 api 深拷贝 | `_history` 投影下 messages 被整体覆盖——深拷贝跳过该字段（非对象 request 沿原 clone 路径）；`json!(messages)` → `Value::Array(messages)` 直移 |
| D | S28.2 PendingBuffer | 超限省略改 `tail_offset` 前端偏移推进＋偏移过半（≥PENDING_CAP）批量物理收缩（摊还 O(n)）；take() 输出逐字节不变（头＋省略标记＋tail[offset..]） |
| D | S28.3 McpSnapshot | `Inner.snapshot_cache: Option<Arc<McpSnapshot>>`；publish 先查缓存、构建后写回；可变取锁（9 处）统一置 None 失效（涵盖 servers/listing/connection 全部变更点＋note_connection_down）；快照不可变、Arc 身份不可观察；断连原子量每轮仍经 prepare_for_turn §5.1 同步块重查 |
| D | S29.1-4 | tool_search：definition() 本就是 Value——序列化仅计量、不再回析；check_snapshot_capacity 搜索文本每条目构建一次（过滤与容量共用）；agents 子轮上下文锁内借用构建（不再整体深克隆＋二次序列化）；js init 帧预检与发送共用同一次序列化（probe 字节直接 write_all）。S29.5（事件字节口径）随 S11 线：session commit 返回 usize＝历史游标非字节，收敛点在 S11 原语——本轮未改字节口径（见「未排期」） |
| E | S13 proc 终止骨架 | `Self::terminate_sessions_bounded(&[Arc<Session>]) -> Vec<String>` 单点：温和终止（一次发齐）→3s 共享窗口（20ms）→升级强杀→500ms 短窗（20ms）。**漂移统一**：reap_where 短窗原 10ms→20ms（记录在案）；reap_extra 早退成功不清 reap_failed、后处理（记录移除/闩锁/文案）留调用方 |
| E | S26 memory | store：`Self::configure_conn` pragma 单点（open/open_existing 共用；CREATE 差异在打开标志不在此层）；thread_updated_at_ms 改 `query_row().optional()`（16 处先例；行取值错误文案「读取登记时间失败」随行映射保真，查询错误文案单列）；workspace：`isolate_env` env 隔离单点（run_git_env/git init 两站）、`diff_args_base` 开关向量单点（状态列表/统一 diff 各自追加专属开关）、`name_status_error` 错误臂单点 |
| F | S12 agents 记账 | `launch_child(agent_name, spec, thread_failed_error)` 单点（计数→起线程→失败回滚三段；错误文案参量保留 spawn/followup 措辞差异）；`commit_with(kind, agent, assignment, payload, persist)` 单点（序号＋构建＋2MiB 校验＋落库通道参量化：root/pump 各自委托）；`enqueue_message` 单点（计账＋入队＋组预算；followup 的 assignment_id 读取移出原混合临界区，读序与入队次序无观察差异——注释记录）；槽位释放顺序（live_children→GLOBAL_CHILD_SLOTS→core）不变 |
| G | S11 session 孪生管线 | `commit_event_line(history, line, subject, advance_over_line, untracked_kind)` 提交原语单点：预检文案（subject 参量：检查点/编排事件/Agent 事件）→V3 迁移（create_v3_target_with_tail）→滞留段＋事件行按序写入；advance_over_line 区分检查点（游标+1）与事件（untracked 吸收："orch"/"agent"）；失败截回文案逐字保留（检查点额外「活动视图不变」经 map_err 追加保真）。三 pub 方法薄委托。create 三胞胎：`create_target(meta_version, tail_lines, failure_prefix)` 单点，create_auto_file（meta=self.version、无尾行、空前缀）与 create_v3_target_with_tail（meta=3＋尾行＋「提交检查点失败：」前缀）薄委托；create_v3_target_with_checkpoint 别名删除。净删约 220 行 |
| G | S17 turn 出口/入史 | `close_exit!` 统一出口宏（5 变体：pending/model_error/orch_first model_error/orch_first task_completed/pending_dyn）——**4 处先 close_orch 站点用 orch_first 变体逐站点保留原顺序**（入史顺序可观察）；动态 reason（close_reason_for）经 pending_dyn 传参。三连 26 处收敛（宏体 4＋调用 22）；close_orch! 宏体 orch_cell=None 加 `#[allow(unused_assignments)]`（轮出口后无读取，直接/间接展开共 25 处 never-read 假阳性统一抑制）。`push_assistant_text` 单点（5 处 assistant 终文入史逐字同构块收敛） |

## 关键保真点（逐条）

- 错误文案：S22/S11/S13/S26 全部逐字保留（S11 检查点「活动视图不变」后缀经 map_err 条件追加——仅非停用路径，与原逻辑等价）。
- S17 顺序：4 处 orch_first 站点（:1802/:1818/:1828/:1865 一带）逐站点核对原顺序后选变体；主序站点（steer→pending→orch）同理。
- S13：10ms/20ms 漂移统一为 20ms（清单已预告）；早退不清 reap_failed 语义保留。
- S28.3：快照复用不改变断连检查时序（§5.1 块每轮先于 publish 执行）。
- S32.4：ParsedToolCall 为纯别名，外部 5 文件引用零改动。

## 验证日志（关键过滤轮摘录，全部 env -u TCHAT_SANDBOX）

- 批次 A：proc_session::tests 23/0（含边界断言）、prompt_assembly 19/0、tui_host 14/0、catalog 37/0、view_image 22/0。
- 批次 B：turn 18/0、request_permissions 38/0、llm_flow 51/0、mcp_config 17/0、multi_agent 24/0。
- 批次 C：mcp_l1 14/0、mcp_resources 37/0、mcp_turn 26/0。
- 批次 D：tool_search 1/0、agents 14/0、js_orchestration lib 13/0＋集成 42/0、multi_agent 24/0。
- 批次 E：memory lib 49/0、proc_session 23/0、memory_phase2 45/0、memory_lifecycle 6/0、exec_command 18/0。
- 批次 F：agents 14/0、multi_agent 24/0（首跑一次瞬态失败，复跑两次均 24/0——spawn 密集套件既有并行敏感，与 S33 计费断言 1/0 同过）。
- 批次 G：session lib 27/0＋集成 40/0、memory_lifecycle 6/0、e2e_acceptance 150/0、turn 18/0、llm_flow 51/0、context_management 43/0。
- 收尾：`cargo check --tests` 0 错误；警告 **580＝604 基线−25**（−25＝close_orch! 宏体 orch_cell=None 的 never-read 假阳性统一抑制：S17 宏间接展开使其 24→25，加 allow 后全部消除——零新增，对应关系如上）；**lib 单线程 310 passed / 0 failed / 1 ignored**；e2e_acceptance 150/0。

## 环境备注（排查记录）

- 本轮一次 `TCHAT_SANDBOX=1` 会话环境泄漏导致 turn::tests 一用例（apply_patch 真实执行）假红——`env -u` 后通过，干净树对照同失败，已定位为环境变量非代码问题；此后全部测试命令带 `env -u TCHAT_SANDBOX`（与 AGENTS.md 全量验证惯例一致）。
- multi_agent 套件两次瞬态失败均复跑即过（spawn 密集＋并行敏感既有形态，S12 改动前后行为一致）。

## 未排期项（登记，本轮不动）

1. S29.5 事件字节口径：`commit_agent_event_root` 的 usize 返回值是历史游标非字节数；字节口径单次序列化收敛需动 session 提交签名，与 S11 原语同域——留待单独确认后实施（account_event 现仍独立序列化，行为不变）。
2. S23 tokio 站点（mcp/transport :269 附近）fd 收口未合并（Command 类型不同，清单标记可选）。
3. mcp/secrets.rs `sort_by_key` 与 proc_session spawner_tx `Mutex`（S10 轮已登记）。

## 清单标 [x]

S11、S12、S13、S14、S15、S16、S17、S18、S19、S20、S21、S22、S23、S24、S25、S26、S28、S29、S30、S32（S27 前批已 [x]；S31 跳过）。
