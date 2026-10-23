//! 018：Codex d138398 的同步问询参数归一化与本地宿主边界。
pub use codex_protocol::request_user_input::{
    RequestUserInputAnswer as Answer, RequestUserInputError as InputError,
    RequestUserInputQuestion as Question, RequestUserInputQuestionOption as QuestionOption,
    RequestUserInputResponse as Response,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

pub type InputHost<'a> =
    dyn FnMut(&crate::turn::ParsedToolCall, &[Question]) -> Result<Response, InputError> + 'a;

// 工具面严格接收 Schema 字段；内部 isOther/isSecret 不开放给模型。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArgs {
    questions: Vec<ToolQuestion>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolQuestion {
    id: String,
    header: String,
    question: String,
    options: Vec<ToolOption>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolOption {
    label: String,
    description: String,
}

pub fn parse_arguments(raw: &str) -> Result<Vec<Question>, String> {
    let args: ToolArgs = serde_json::from_str(raw).map_err(|e| format!("问询参数无效：{e}"))?;
    if args.questions.is_empty() {
        return Err("问询问题不能为空".into());
    }
    let mut ids = HashSet::new();
    let mut questions = Vec::with_capacity(args.questions.len());
    for q in args.questions {
        if q.id.trim().is_empty() || !ids.insert(q.id.clone()) {
            return Err("问询问题 ID 必须非空且批内唯一".into());
        }
        // Codex request_user_input_spec.rs normalize：每题非空选项，启用 Other。
        if q.options.is_empty() {
            return Err("request_user_input requires non-empty options for every question".into());
        }
        questions.push(Question {
            id: q.id,
            header: q.header,
            question: q.question,
            is_other: true,
            is_secret: false,
            options: Some(
                q.options
                    .into_iter()
                    .map(|o| QuestionOption {
                        label: o.label,
                        description: o.description,
                    })
                    .collect(),
            ),
        });
    }
    Ok(questions)
}

pub fn validate_response(questions: &[Question], response: &Response) -> Result<(), InputError> {
    if response
        .answers
        .keys()
        .all(|id| questions.iter().any(|q| &q.id == id))
    {
        Ok(())
    } else {
        Err(InputError::InvalidResponse)
    }
}

/// 018：标准库一次性回送通道；取消轮询只负责解除等待，不自动产生答案。
pub fn wait_for_response(
    receiver: std::sync::mpsc::Receiver<Result<Response, InputError>>,
    cancel: &std::sync::atomic::AtomicBool,
    live: &std::sync::atomic::AtomicBool,
) -> Result<Response, InputError> {
    use std::sync::{atomic::Ordering, mpsc::RecvTimeoutError};
    let result = loop {
        match receiver.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(result) => break result,
            Err(RecvTimeoutError::Disconnected) => break Err(InputError::Closed),
            Err(RecvTimeoutError::Timeout) if cancel.load(Ordering::SeqCst) => {
                // UI 先送真实终态再置取消标志；超时与退出相邻时优先已送达的终态。
                break match receiver.try_recv() {
                    Ok(result) => result,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(InputError::Closed),
                    Err(std::sync::mpsc::TryRecvError::Empty) => Err(InputError::Cancelled),
                };
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    };
    live.store(false, Ordering::SeqCst);
    result
}

/// 仅生成展示副本。ID 是关联键，绝不改写；选择仍从原始问题取 label。
pub fn sanitized_questions(questions: &[Question]) -> Vec<Question> {
    let mut copy = questions.to_vec();
    for q in &mut copy {
        q.header = crate::display::sanitize_for_terminal(&q.header);
        q.question = crate::display::sanitize_for_terminal(&q.question);
        if let Some(options) = &mut q.options {
            for o in options {
                o.label = crate::display::sanitize_for_terminal(&o.label);
                o.description = crate::display::sanitize_for_terminal(&o.description);
            }
        }
    }
    copy
}

pub fn sanitized_response(response: &Response) -> Response {
    Response {
        answers: response
            .answers
            .iter()
            .map(|(id, answer)| {
                (
                    id.clone(),
                    Answer {
                        answers: answer
                            .answers
                            .iter()
                            .map(|s| crate::display::sanitize_for_terminal(s))
                            .collect(),
                    },
                )
            })
            .collect(),
    }
}

/// Chat Completions 映射自 Codex request_user_input_spec.rs；数量仅为使用指引。
pub fn tool_definition() -> Value {
    json!({"type":"function","function":{
        "name":"request_user_input",
        "description":"Request user input for one to three short questions and wait for the response. Available when the interactive host can collect answers.",
        "parameters":{"type":"object","properties":{"questions":{"type":"array",
            "description":"Questions to show the user. Prefer 1 and do not exceed 3",
            "items":{"type":"object","properties":{
                "id":{"type":"string","description":"Stable identifier for mapping answers (snake_case)."},
                "header":{"type":"string","description":"Short header label shown in the UI (12 or fewer chars)."},
                "question":{"type":"string","description":"Single-sentence prompt shown to the user."},
                "options":{"type":"array","description":"Provide 2-3 mutually exclusive choices. Put the recommended option first and suffix its label with \"(Recommended)\". Do not include an \"Other\" option in this list; the client will add a free-form \"Other\" option automatically.",
                    "items":{"type":"object","properties":{"label":{"type":"string","description":"User-facing label (1-5 words)."},"description":{"type":"string","description":"One short sentence explaining impact/tradeoff if selected."}},"required":["label","description"],"additionalProperties":false}}
            },"required":["id","header","question","options"],"additionalProperties":false}}},"required":["questions"],"additionalProperties":false}
    }})
}
