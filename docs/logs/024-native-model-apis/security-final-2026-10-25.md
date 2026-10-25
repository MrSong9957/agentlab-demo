# 安全测试收口（2026-10-25）

针对本轮四项修复的安全面执行攻防复验，全部由 `cargo test --test native_model_apis` 覆盖（30 项）：

1. 能力检查绕过（修复）：`stream_requires_declared_capability_before_any_request_on_every_protocol`
   —— 五协议 ×（空列表／不含 stream／能力未知）全部零 HTTP 本地报错；openai-chat 对照一致。
2. 外部 SSE 异常输入：`hostile_sse_shapes_and_arg_types_are_rejected_without_panic`
   —— created.interaction 为字符串／数字／数组／null、completed／status_update／step.stop 乱序，
      全部走 Err 协议错误路径，无 panic、单次请求零重试；空响应变体由 normalized 拒绝。
   —— CLI 级 `malformed_interactions_sse_is_a_protocol_error_not_a_crash`：真实进程不崩溃（修复前
      子进程 panic "cannot access key \"status\" in JSON string"，见 diagnosis-gemini-args-and-interactions-sse-red.log）。
3. Gemini args：缺省／null → `{}`（`generate_content_missing_args…`、`generate_content_null_args…`）；
   非对象（字符串／数字／数组／布尔）→ 错误指明 args 且零执行；原生 Part 逐字回传不虚构。
4. 保存状态校验：`damaged_native_payload_or_mismatched_saved_call_is_rejected_before_http`
   —— 四协议 × 三类损坏（ID 类型／ID 改动／参数改动）全部零发送（12 组合，沿用 2026-10-24 红例证明）。
5. 凭证与载荷保护：沿用 `native_errors_identify_source_and_request_without_credentials_or_retry`
   （错误不含 Key／指纹）、10 MiB 双上限、data URL 遮蔽（019）；本轮改动未新增凭证面。
6. 重定向拒绝、连接／总时限沿用 002 已验收路径（llm_flow B 系列），共享 post_chat_json。

结论：本轮修复未引入新攻击面；原审查发现的三个 P2 安全／契约缺口全部关闭。
