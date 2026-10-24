//! 021：send_message_to_user_async 工具——即时消息的参数解析、accepted 结果、
//! 实时与恢复共用的成功消息提取，以及宿主失败提示行。函数级参照
//! openai/codex@d138398（Apache-2.0）
//! `codex-rs/core/src/tools/handlers/send_message_to_user_async.rs`：
//! 严格单 `message` 字段、`trim()` 后空白拒绝、发出消息后立即返回
//! `{"accepted":true}`；不等待回答、不结束 turn。

use serde::Deserialize;
use serde_json::{json, Value};

/// 上游固定工具名。
pub const TOOL_NAME: &str = "send_message_to_user_async";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageArgs {
    message: String,
}

/// Chat Completions 映射自上游 ResponsesApiTool（固定提交 d138398）：
/// required＋additionalProperties:false 同既有本地工具；参数只有文本，
/// 不增加标题、重要性等级、超时或收件人。
pub fn tool_definition() -> Value {
    json!({"type":"function","function":{
        "name": TOOL_NAME,
        "description":"Send a concise message that needs the user's attention during ongoing work. The tool returns immediately without ending the turn or waiting for a reply; any reply arrives asynchronously as a new user message. Use this tool to report a critical blocker or a finding that may change the task's direction, or to answer a user question or status request received while work is still in progress. Use this tool when a message needs the user's immediate attention; use commentary for routine progress and intermediate context. Use clear formatting, such as bolding questions, to make requests easy to notice and answer.",
        "parameters":{"type":"object","properties":{"message":{"type":"string",
            "description":"The concise question or update to send to the user."}},
            "required":["message"],"additionalProperties":false}
    }})
}

/// 参数解析（上游 SendMessageToUserAsyncArgs 语义）：非法 JSON、缺字段、null、
/// 错误类型、未知字段均报参数错误；`trim()` 后空白报上游固定文案；
/// 内部换行与原文（除首尾空白）保留。
pub fn parse_arguments(raw: &str) -> Result<String, String> {
    let args: MessageArgs =
        serde_json::from_str(raw).map_err(|e| format!("工具参数不是有效 JSON：{e}"))?;
    let message = args.message.trim();
    if message.is_empty() {
        return Err("message must not be empty".to_string());
    }
    Ok(message.to_string())
}

/// 成功结果（与上游逐字一致）：只确认参数有效并由本地发送路径受理，
/// 不代表用户已读、已答或任务完成。
pub fn accepted_result() -> String {
    json!({ "accepted": true }).to_string()
}

/// 实时成功结果与 /resume 共用的转换：结果明确 `accepted=true` 且参数可解析
/// 才重建消息文本；未知、缺失、取消、失败或损坏结果不能推定发送成功（None）。
pub fn sent_message_from_record(arguments: &str, result: &str) -> Option<String> {
    let v: Value = serde_json::from_str(result).ok()?;
    if v.get("accepted").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    parse_arguments(arguments).ok()
}

/// 失败/取消/未知结果的宿主提示行（调用方仍需终端过滤后输出）：
/// 取消跳过＝「已取消」，其余错误＝「失败」，非 JSON＝「结果未知」。
pub fn failure_line(result: &str) -> String {
    let reason = serde_json::from_str::<Value>(result)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
    match reason {
        Some(reason) if reason == crate::turn::SKIPPED_BY_CANCEL_MESSAGE => {
            format!("[工具] {TOOL_NAME} 已取消：{reason}")
        }
        Some(reason) => format!("[工具] {TOOL_NAME} 失败：{reason}"),
        None => format!("[工具] {TOOL_NAME} 结果未知"),
    }
}
