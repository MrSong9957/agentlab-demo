//! 018 阶段 B：异步问询的最小解析、未决状态与回执通道（§8.3–8.6）。
//!
//! 上游锚点：Codex d138398 `core/src/tools/handlers/request_user_input_async.rs`
//! （校验后发布、返回 `{"accepted":true}`，不在处理器内等待）与
//! `tui/src/bottom_pane/async_questions/state.rs`（题目 ID＝
//! `JSON(["request_user_input_async", message_id, index])`）。`wait_for_user_input`
//! 是本地新增适配，不冒称上游同名工具（§8.4）。

use crate::user_input::{InputError, Question, QuestionOption, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

pub use codex_protocol::request_user_input::AsyncUserInputQuestion;

/// 异步发布回调返回的一次性回执通道：UI 提交经此送回工作线程；
/// `live` 拒绝已取消/已结束问询的迟到显示事件（同阶段 A 机制）。
pub struct AsyncReplyChannel {
    pub receiver: mpsc::Receiver<Result<Response, InputError>>,
    pub live: Arc<AtomicBool>,
}

/// 018 阶段 B：异步问询宿主接口——校验通过后建立回执通道并交给 UI，
/// 随即返回；不等待答案。未提供接口的调用方不声明也不执行两个新工具。
pub type AsyncInputHost<'a> =
    dyn FnMut(&crate::turn::ParsedToolCall, &[Question]) -> Result<AsyncReplyChannel, String> + 'a;

/// 上游规则：题目 ID＝`JSON(["request_user_input_async", 原调用 ID, 0 起序号])`。
pub fn question_id(call_id: &str, index: usize) -> String {
    serde_json::to_string(&json!(["request_user_input_async", call_id, index]))
        .expect("题目 ID 可序列化")
}

// 工具面严格接收 Schema 字段（§8.3）：顶层与每题拒绝未知字段。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArgs {
    questions: Vec<ToolQuestion>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolQuestion {
    title: String,
    /// 省略/null＝纯文字题（上游 Option 语义）；给出时须非空且各字符串非空白。
    options: Option<Vec<String>>,
}

/// 发布参数解析＋归一化：`title` 仅 trim 判断非空、不改原文；字符串选项映射
/// label、description 留空，开启 Other/文字输入；header 用本地题号。
pub fn parse_arguments(raw: &str, call_id: &str) -> Result<Vec<Question>, String> {
    let args: ToolArgs = serde_json::from_str(raw).map_err(|e| format!("问询参数无效：{e}"))?;
    if args.questions.is_empty() {
        return Err("问询问题不能为空".into());
    }
    let mut questions = Vec::with_capacity(args.questions.len());
    for (index, q) in args.questions.into_iter().enumerate() {
        if q.title.trim().is_empty() {
            return Err("问询标题不能为空白".into());
        }
        if let Some(options) = &q.options {
            if options.is_empty() || options.iter().any(|o| o.trim().is_empty()) {
                return Err("问询选项数组须非空且各选项非空白（纯文字题请省略 options）".into());
            }
        }
        questions.push(Question {
            id: question_id(call_id, index),
            header: format!("问题{}", index + 1),
            question: q.title,
            is_other: true,
            is_secret: false,
            options: q.options.map(|options| {
                options
                    .into_iter()
                    .map(|label| QuestionOption {
                        label,
                        description: String::new(),
                    })
                    .collect()
            }),
        });
    }
    Ok(questions)
}

// 等待工具参数（§8.4）：仅一个必填非空 request_id，拒绝未知字段。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    request_id: String,
}

pub fn parse_wait_arguments(raw: &str) -> Result<String, String> {
    let args: WaitArgs = serde_json::from_str(raw).map_err(|e| format!("等待参数无效：{e}"))?;
    if args.request_id.trim().is_empty() {
        return Err("request_id 必须非空".into());
    }
    Ok(args.request_id)
}

/// 发布确认（§8.3）：`accepted` 沿用 Codex；`request_id`＝原始工具调用 ID，
/// 为显式等待/关联增加的最小本地字段。只确认宿主已接收发布，不保证已答。
pub fn accepted_result(call_id: &str) -> String {
    json!({ "accepted": true, "request_id": call_id }).to_string()
}

/// 等待就绪结果：`ready` 不是第二份回答——真正回答数据由批末入史的
/// 专用事件承载，随下一请求供模型使用。
pub fn wait_ready_result(request_id: &str) -> String {
    json!({ "ready": true, "request_id": request_id }).to_string()
}

/// 回答入史正文（§8.6）：完整保留 request_id 与答案原文。
pub fn answer_event_content(request_id: &str, response: &Response) -> String {
    serde_json::to_string(&json!({ "request_id": request_id, "answers": response.answers }))
        .expect("回答事件可序列化")
}

/// 关闭事件正文：同一 request_id＋具体原因。
pub fn close_event_content(request_id: &str, reason: &str) -> String {
    json!({ "request_id": request_id, "reason": reason }).to_string()
}

/// 关闭原因（§8.5 固定短串，展示层据此措辞）。
pub const CLOSE_TASK_COMPLETED: &str = "task_completed";
pub const CLOSE_CANCELLED: &str = "cancelled";
pub const CLOSE_REQUEST_LIMIT: &str = "request_limit";
pub const CLOSE_HOST_EXIT: &str = "host_exit";
pub const CLOSE_CHANNEL_ERROR: &str = "channel_error";
pub const CLOSE_MODEL_ERROR: &str = "model_error";

/// 通道终态 → 关闭原因短串。
pub fn close_reason_for(error: &InputError) -> &'static str {
    match error {
        InputError::Cancelled => CLOSE_CANCELLED,
        InputError::HostExited => CLOSE_HOST_EXIT,
        InputError::Closed | InputError::InvalidResponse => CLOSE_CHANNEL_ERROR,
    }
}

/// Chat Completions 映射自上游异步处理器参数（固定提交 d138398）；
/// Schema 允许省略 options（纯文字题）。
pub fn tool_definition() -> Value {
    json!({"type":"function","function":{
        "name":"request_user_input_async",
        "description":"Publishes questions to the user without waiting for answers and returns immediately with {\"accepted\":true}. The user answers at their own pace while you continue independent work; do not wait or poll. Each turn allows at most one pending request.",
        "parameters":{"type":"object","properties":{"questions":{"type":"array",
            "description":"Questions to publish. Prefer 1 and do not exceed 3",
            "items":{"type":"object","properties":{
                "title":{"type":"string","description":"Single-sentence question shown to the user."},
                "options":{"type":"array","description":"2-3 mutually exclusive choice labels. Omit for a free-form text question.",
                    "items":{"type":"string"}}
            },"required":["title"],"additionalProperties":false}}},"required":["questions"],"additionalProperties":false}
    }})
}

/// 本地等待工具（§8.4 明确的本地适配，非上游能力）：按 request_id 显式等待
/// 本轮发布的问询回答；必须位于批内最后一条调用。
pub fn wait_tool_definition() -> Value {
    json!({"type":"function","function":{
        "name":"wait_for_user_input",
        "description":"Waits until the user answers the pending request_user_input_async question identified by request_id, then returns {\"ready\":true}. Must be the last call in the batch: actions that depend on the answer must be generated in the next request after the answer becomes visible. Use the request_id from the publish confirmation.",
        "parameters":{"type":"object","properties":{
            "request_id":{"type":"string","description":"request_id returned by request_user_input_async."}
        },"required":["request_id"],"additionalProperties":false}
    }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_id_uses_upstream_rule() {
        assert_eq!(
            question_id("call_17", 0),
            "[\"request_user_input_async\",\"call_17\",0]"
        );
    }

    #[test]
    fn parse_normalizes_options_and_freeform() {
        let questions = parse_arguments(
            r#"{"questions":[{"title":"保留密码还是验证码？","options":["保留密码","验证码"]},{"title":"面向谁？"}]}"#,
            "call_9",
        )
        .unwrap();
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].id, question_id("call_9", 0));
        assert_eq!(questions[0].header, "问题1");
        assert_eq!(questions[0].question, "保留密码还是验证码？");
        assert!(questions[0].is_other);
        assert_eq!(questions[0].options.as_ref().unwrap()[1].label, "验证码");
        assert_eq!(questions[0].options.as_ref().unwrap()[1].description, "");
        // 省略 options＝纯文字题（上游 Option 语义）
        assert_eq!(questions[1].options, None);
        // null 同样视作纯文字题
        let questions =
            parse_arguments(r#"{"questions":[{"title":"t","options":null}]}"#, "c").unwrap();
        assert_eq!(questions[0].options, None);
    }

    #[test]
    fn parse_rejects_invalid_shapes() {
        for raw in [
            "not json",
            "{}",
            r#"{"questions":[]}"#,
            r#"{"questions":[{"title":"t"}],"extra":1}"#,
            r#"{"questions":[{"title":"  ","options":["a"]}]}"#,
            r#"{"questions":[{"title":"t","options":[]}]}"#,
            r#"{"questions":[{"title":"t","options":["  "]}]}"#,
            r#"{"questions":[{"title":"t","options":"a"}]}"#,
            r#"{"questions":[{"title":"t","options":[5]}]}"#,
            r#"{"questions":[{"title":"t","extra":true}]}"#,
            r#"{"questions":[{"options":["a"]}]}"#,
            r#"{"questions":"x"}"#,
        ] {
            assert!(parse_arguments(raw, "c").is_err(), "错误接收：{raw}");
        }
    }

    #[test]
    fn wait_args_strict_nonempty_id() {
        assert_eq!(
            parse_wait_arguments(r#"{"request_id":"pub"}"#).unwrap(),
            "pub"
        );
        for raw in [
            "not json",
            "{}",
            r#"{"request_id":"  "}"#,
            r#"{"request_id":"a","extra":1}"#,
            r#"{"request_id":5}"#,
        ] {
            assert!(parse_wait_arguments(raw).is_err(), "错误接收：{raw}");
        }
    }

    #[test]
    fn event_contents_preserve_request_id() {
        let response = Response {
            answers: [(
                question_id("call_17", 0).clone(),
                crate::user_input::Answer {
                    answers: vec!["保留密码".into()],
                },
            )]
            .into(),
        };
        let content = answer_event_content("call_17", &response);
        let v: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["request_id"], json!("call_17"));
        assert!(v["answers"].is_object());
        assert_eq!(
            close_event_content("call_17", CLOSE_TASK_COMPLETED),
            r#"{"request_id":"call_17","reason":"task_completed"}"#
        );
    }
}
