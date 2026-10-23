//! 018 阶段 B：异步问询工具循环与契约测试（验收行：发布参数/确认、发布后继续、
//! 回答及时接入、依赖等待、结束交界、单项/重复/迟到、额度、能力与权限）。
mod common;

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use terminal_chat::proc_session::ProcessManager;
use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, UserInputEvent};
use terminal_chat::turn::{self, ToolMode};
use terminal_chat::user_input::{Answer, InputError, Question, Response};
use terminal_chat::user_input_async;

fn publish_args() -> String {
    json!({"questions":[{"title":"报告面向谁？","options":["内部评审","外部客户"]}]}).to_string()
}
fn wait_args(id: &str) -> String {
    json!({"request_id": id}).to_string()
}
fn call(id: &str, name: &str, arguments: &str) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}})
}
fn tool_response(calls: Vec<Value>) -> Value {
    json!({"choices":[{"message":{"tool_calls":calls}}]})
}
fn final_response(text: &str) -> Value {
    json!({"choices":[{"message":{"content":text}}]})
}
fn answered(qid: &str, label: &str) -> Response {
    Response {
        answers: [(
            qid.to_string(),
            Answer {
                answers: vec![label.to_string()],
            },
        )]
        .into(),
    }
}
fn q_id(call_id: &str) -> String {
    user_input_async::question_id(call_id, 0)
}

fn tool_results(history: &[HistoryItem]) -> Vec<(String, Value)> {
    history
        .iter()
        .filter_map(|h| {
            if let HistoryItem::ToolResult {
                tool_call_id,
                content,
                ..
            } = h
            {
                Some((tool_call_id.clone(), serde_json::from_str(content).unwrap()))
            } else {
                None
            }
        })
        .collect()
}

fn answer_event_of<'a>(history: &'a [HistoryItem], request_id: &str) -> Option<&'a HistoryMessage> {
    history.iter().find_map(|h| match h {
        HistoryItem::Text(
            m @ HistoryMessage {
                user_input_event: Some(UserInputEvent::Answer { request_id: rid }),
                ..
            },
        ) if rid == request_id => Some(m),
        _ => None,
    })
}

fn close_event_of<'a>(
    history: &'a [HistoryItem],
    request_id: &str,
) -> Option<(&'a HistoryMessage, Value)> {
    history.iter().find_map(|h| match h {
        HistoryItem::Text(
            m @ HistoryMessage {
                user_input_event: Some(UserInputEvent::Closed { request_id: rid }),
                content,
                ..
            },
        ) if rid == request_id => Some((m, serde_json::from_str(content).unwrap())),
        _ => None,
    })
}

/// 共享态异步宿主：发布回调登记一次性通道；测试线程按序号提交回答/失败，
/// 就绪用发布计数与可控事件，不依赖长时间睡眠猜时序。
#[derive(Clone, Default)]
struct AsyncHost {
    senders: Arc<Mutex<Vec<mpsc::Sender<Result<Response, InputError>>>>>,
    publishes: Arc<AtomicUsize>,
    questions: Arc<Mutex<Vec<Vec<Question>>>>,
}

impl AsyncHost {
    fn hook(
        &self,
    ) -> impl FnMut(
        &turn::ParsedToolCall,
        &[Question],
    ) -> Result<user_input_async::AsyncReplyChannel, String>
           + '_ {
        let senders = self.senders.clone();
        let publishes = self.publishes.clone();
        let questions = self.questions.clone();
        move |_call: &turn::ParsedToolCall, qs: &[Question]| {
            publishes.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = mpsc::channel();
            let live = Arc::new(AtomicBool::new(true));
            senders.lock().unwrap().push(tx);
            questions.lock().unwrap().push(qs.to_vec());
            Ok(user_input_async::AsyncReplyChannel { receiver: rx, live })
        }
    }
    fn wait_publish(&self, count: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.publishes.load(Ordering::SeqCst) < count {
            assert!(
                std::time::Instant::now() < deadline,
                "发布未发生（等待超时）"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    fn answer(&self, index: usize, response: Response) {
        self.senders.lock().unwrap()[index]
            .send(Ok(response))
            .expect("回答通道仍有效");
    }
    /// 迟到/重复提交探测：通道已失效时返回 Err。
    fn try_answer(
        &self,
        index: usize,
        response: Response,
    ) -> Result<(), mpsc::SendError<Result<Response, InputError>>> {
        self.senders.lock().unwrap()[index].send(Ok(response))
    }
    fn fail(&self, index: usize, error: InputError) {
        self.senders.lock().unwrap()[index]
            .send(Err(error))
            .expect("回执通道仍有效");
    }
    fn publish_count(&self) -> usize {
        self.publishes.load(Ordering::SeqCst)
    }
    fn questions(&self, index: usize) -> Vec<Question> {
        self.questions.lock().unwrap()[index].clone()
    }
}

fn sync_answer_host(
) -> impl FnMut(&turn::ParsedToolCall, &[Question]) -> Result<Response, InputError> {
    move |_, questions: &[Question]| {
        let mut answers = std::collections::HashMap::new();
        for q in questions {
            answers.insert(
                q.id.clone(),
                Answer {
                    answers: vec!["A".into()],
                },
            );
        }
        Ok(Response { answers })
    }
}

fn sync_args() -> String {
    json!({"questions":[{"id":"q","header":"方向","question":"选择？","options":[{"label":"A","description":"说明"}]}]}).to_string()
}

/// 验收行 1：发布参数——上游形状与纯文字题可用；非法形状失败且不调用宿主。
#[test]
fn publish_args_validate_upstream_shape_without_host_calls() {
    let host = AsyncHost::default();
    for raw in [
        "not json",
        "{}",
        r#"{"questions":[]}"#,
        r#"{"questions":[{"title":"t"}],"extra":1}"#,
        r#"{"questions":[{"title":"  ","options":["a"]}]}"#,
        r#"{"questions":[{"title":"t","options":[]}]}"#,
        r#"{"questions":[{"title":"t","options":["  "]}]}"#,
        r#"{"questions":[{"title":"t","options":"x"}]}"#,
        r#"{"questions":[{"title":"t","extra":true}]}"#,
        r#"{"questions":[{"options":["a"]}]}"#,
    ] {
        let mut history = Vec::new();
        let mut sent = 0;
        let mut async_host = host.hook();
        turn::run_tool_turn_with_hosts(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                Ok(if sent == 1 {
                    tool_response(vec![call("bad", "request_user_input_async", raw)])
                } else {
                    final_response("完成")
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            None,
            Some(&mut async_host),
        false
        )
        .unwrap();
        assert_eq!(host.publish_count(), 0, "非法参数不得调用宿主：{raw}");
        assert!(
            matches!(&history[2], HistoryItem::ToolResult { content, .. }
                if serde_json::from_str::<Value>(content).unwrap()["error"].is_string()),
            "失败结果：{raw}"
        );
    }
    // 合法：含选项与纯文字题（options 省略）都能发布并立即确认
    let mut history = Vec::new();
    let mut sent = 0;
    let mut async_host = host.hook();
    turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(if sent == 1 {
                tool_response(vec![call(
                    "pub",
                    "request_user_input_async",
                    r#"{"questions":[{"title":"题A","options":["x"]},{"title":"题B"}]}"#,
                )])
            } else {
                final_response("完成")
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host),
        false
    )
    .unwrap();
    assert_eq!(host.publish_count(), 1);
    assert_eq!(host.questions(0).len(), 2);
    assert_eq!(host.questions(0)[1].options, None, "纯文字题归一化");
    let HistoryItem::ToolResult { content, .. } = &history[2] else {
        panic!("缺发布结果");
    };
    assert_eq!(
        serde_json::from_str::<Value>(content).unwrap(),
        json!({"accepted":true,"request_id":"pub"}),
        "确认含关联 ID，未提交时无 answers"
    );
}

/// 验收行 2：发布后继续——同批独立命令真实执行；不自动空答、不空转查询模型；
/// 正常完成关闭未答问题为 task_completed。
#[test]
fn publish_then_independent_work_continues_without_polling() {
    let temp = common::TempDir::new("018b-continue");
    let probe = temp.path().join("side.txt");
    let host = AsyncHost::default();
    let mut history = Vec::new();
    let mut sent = 0;
    let mut async_host = host.hook();
    let reply = turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(if sent == 1 {
                tool_response(vec![
                    call("pub", "request_user_input_async", &publish_args()),
                    call(
                        "work",
                        "exec_command",
                        &json!({"cmd": format!("echo side > {}", probe.display())}).to_string(),
                    ),
                ])
            } else {
                final_response("S104-完成")
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host),
        false
    );
    assert_eq!(reply.unwrap(), "S104-完成");
    assert_eq!(sent, 2, "发布不等待答案：批后恰续一次请求");
    assert!(probe.exists(), "发布后独立命令已执行");
    assert_eq!(host.publish_count(), 1);
    let (_, closed) = close_event_of(&history, "pub").expect("正常完成关闭未答问题");
    assert_eq!(closed["request_id"], json!("pub"));
    assert_eq!(closed["reason"], json!("task_completed"));
}

/// 验收行 3：回答在工具执行中提交——原文只在完整批之后入史（批中不插入文本），
/// 恰一次进入下一请求，不丢后续结果。
#[test]
fn answer_submitted_mid_batch_enters_history_after_full_batch_only() {
    let temp = common::TempDir::new("018b-midbatch");
    let gate = temp.path().join("gate");
    let host = AsyncHost::default();
    // 批内第二条命令阻塞至 gate 出现：保证「回答先提交、批未收尾」确定成立
    let block_cmd = format!("while [ ! -e {} ]; do sleep 0.05; done", gate.display());
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let mut history = Vec::new();
    let mut next_body = Value::Null;
    let worker = {
        let host = host.clone();
        let ready_tx = ready_tx.clone();
        std::thread::spawn(move || {
            let mut async_host = host.hook();
            let mut sent = 0;
            let result = turn::run_tool_turn_with_hosts(
                &mut history,
                "go",
                &mut |body| {
                    sent += 1;
                    if sent == 2 {
                        next_body = body.clone();
                    }
                    Ok(if sent == 1 {
                        tool_response(vec![
                            call("pub", "request_user_input_async", &publish_args()),
                            call(
                                "work",
                                "exec_command",
                                &json!({"cmd": block_cmd, "yield_time_ms": 15000}).to_string(),
                            ),
                            call(
                                "after",
                                "exec_command",
                                &json!({"cmd": "echo after > /dev/null"}).to_string(),
                            ),
                        ])
                    } else {
                        final_response("S104-续算完成")
                    })
                },
                &mut |n| {
                    if let turn::TurnNotify::ToolBegin { id, .. } = &n {
                        if id == "work" {
                            ready_tx.send(()).unwrap();
                        }
                    }
                },
                None,
                &mut |_| {},
                ToolMode::User,
                &ProcessManager::new(),
                None,
                Some(&mut async_host),
        false
            );
            (result, history, sent, next_body)
        })
    };
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    // 工具执行中提交回答（exec 尚未返回——gate 未创建）
    host.answer(0, answered(&q_id("pub"), "内部评审"));
    std::fs::write(&gate, b"1").unwrap();
    let (result, history, sent, next_body) = worker.join().unwrap();
    assert_eq!(result.unwrap(), "S104-续算完成");
    assert_eq!(sent, 2);
    // 入史顺序：发布确认 → exec 结果 → 回答文本（完整批之后）
    let pos = |history: &[HistoryItem], pred: &dyn Fn(&HistoryItem) -> bool| {
        history.iter().position(|h| pred(h)).unwrap()
    };
    let pub_pos = pos(
        &history,
        &|h| matches!(h, HistoryItem::ToolResult { tool_call_id, .. } if tool_call_id == "pub"),
    );
    let exec_pos = pos(
        &history,
        &|h| matches!(h, HistoryItem::ToolResult { tool_call_id, .. } if tool_call_id == "work"),
    );
    let last_tool_pos = pos(
        &history,
        &|h| matches!(h, HistoryItem::ToolResult { tool_call_id, .. } if tool_call_id == "after"),
    );
    let answer_pos = pos(
        &history,
        &|h| matches!(h, HistoryItem::Text(HistoryMessage { user_input_event: Some(UserInputEvent::Answer { request_id }), .. }) if request_id == "pub"),
    );
    assert!(
        pub_pos < exec_pos && exec_pos < last_tool_pos && last_tool_pos < answer_pos,
        "回答只在完整批（含最后一条调用结果）后入史：pub@{pub_pos} exec@{exec_pos} after@{last_tool_pos} answer@{answer_pos}"
    );
    // 回答恰一次进入下一请求：tool 消息在前、user 回答在后
    let msgs = next_body["messages"].as_array().unwrap();
    let work_tool_pos = msgs
        .iter()
        .position(|m| m["tool_call_id"] == json!("work"))
        .unwrap();
    let answer_msg_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == json!("user")
                && m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("内部评审"))
        })
        .expect("回答进入下一请求");
    assert!(work_tool_pos < answer_msg_pos);
    let answer_events = history
        .iter()
        .filter(|h| {
            matches!(h, HistoryItem::Text(HistoryMessage { user_input_event: Some(UserInputEvent::Answer { request_id }), .. }) if request_id == "pub")
        })
        .count();
    assert_eq!(answer_events, 1, "恰一次入史");
}

/// 验收行 4：依赖等待——批末等待期间请求计数不增加；提交后下一请求才生成。
#[test]
fn wait_blocks_until_answer_then_next_request_carries_dependent_action() {
    let host = AsyncHost::default();
    let (wait_started_tx, wait_started_rx) = mpsc::channel::<String>();
    let mut history = Vec::new();
    let mut next_body = Value::Null;
    let worker = {
        let host = host.clone();
        let wait_started_tx = wait_started_tx.clone();
        std::thread::spawn(move || {
            let mut async_host = host.hook();
            let mut sent = 0;
            let result = turn::run_tool_turn_with_hosts(
                &mut history,
                "go",
                &mut |body| {
                    sent += 1;
                    if sent == 3 {
                        next_body = body.clone();
                    }
                    Ok(match sent {
                        1 => tool_response(vec![call(
                            "pub",
                            "request_user_input_async",
                            &publish_args(),
                        )]),
                        2 => {
                            tool_response(vec![call("w", "wait_for_user_input", &wait_args("pub"))])
                        }
                        _ => final_response("S104-依赖动作完成"),
                    })
                },
                &mut |n| {
                    if let turn::TurnNotify::UserInputAsyncWait { request_id } = &n {
                        wait_started_tx.send(request_id.clone()).unwrap();
                    }
                },
                None,
                &mut |_| {},
                ToolMode::User,
                &ProcessManager::new(),
                None,
                Some(&mut async_host),
        false
            );
            (result, history, sent, next_body)
        })
    };
    // 等待开始（可控事件），期间不提交答案——请求计数不增加
    assert_eq!(
        wait_started_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        "pub"
    );
    assert_eq!(host.publish_count(), 1);
    // 提交回答：等待解除，批末入史，下一请求（第 3 次）携带依赖动作
    host.answer(0, answered(&q_id("pub"), "外部客户"));
    let (result, history, sent, next_body) = worker.join().unwrap();
    assert_eq!(result.unwrap(), "S104-依赖动作完成");
    assert_eq!(sent, 3, "等待期间不新增请求");
    let results = tool_results(&history);
    assert_eq!(
        results.iter().find(|(id, _)| id == "w").unwrap().1,
        json!({"ready":true,"request_id":"pub"}),
        "wait 结果＝ready，不是第二份回答"
    );
    // 第 3 请求：wait 结果在前、回答文本随后；依赖动作由该请求生成
    let msgs = next_body["messages"].as_array().unwrap();
    let wait_pos = msgs
        .iter()
        .position(|m| m["tool_call_id"] == json!("w"))
        .unwrap();
    let answer_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == json!("user")
                && m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("外部客户"))
        })
        .unwrap();
    assert!(wait_pos < answer_pos, "答案数据随等待结果之后进入请求");
}

/// 验收行 4（结构）：带后续调用的等待批在入史/执行前整批拒绝。
#[test]
fn wait_with_trailing_call_rejects_whole_batch_before_history() {
    let host = AsyncHost::default();
    let temp = common::TempDir::new("018b-waitorder");
    let probe = temp.path().join("probe.txt");
    let mut history = Vec::new();
    let mut sent = 0;
    let mut async_host = host.hook();
    let error = turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            assert_eq!(sent, 1, "整批拒绝后不得再请求");
            Ok(tool_response(vec![
                call("w", "wait_for_user_input", &wait_args("pub")),
                call(
                    "after",
                    "exec_command",
                    &json!({"cmd": format!("echo x > {}", probe.display())}).to_string(),
                ),
            ]))
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host),
        false
    )
    .unwrap_err();
    assert!(error.contains("最后一条"), "{error}");
    assert_eq!(history.len(), 1, "批不入史：{history:?}");
    assert!(!probe.exists(), "未知答案的动作不预先执行");
}

/// 审计修复 1（§8.4）：任何非批末等待都整批拒绝——[wait, exec, wait] 与
/// [wait, wait] 不入史、中间命令不执行（其参数来自尚未看过答案的请求）。
#[test]
fn wait_before_batch_end_rejects_whole_batch_before_dependent_execution() {
    let temp = common::TempDir::new("018b-audit-waitorder");
    let probe = temp.path().join("dependent.txt");
    for batch in [
        vec![
            call("w1", "wait_for_user_input", &wait_args("pub")),
            call(
                "mid",
                "exec_command",
                &json!({"cmd": format!("echo audit-dependent-executed > {}", probe.display())})
                    .to_string(),
            ),
            call("w2", "wait_for_user_input", &wait_args("pub")),
        ],
        vec![
            call("w1", "wait_for_user_input", &wait_args("pub")),
            call("w2", "wait_for_user_input", &wait_args("pub")),
        ],
    ] {
        let host = AsyncHost::default();
        let mut history = Vec::new();
        let mut sent = 0;
        let inject_host = host.clone();
        let mut async_host = host.hook();
        let error = turn::run_tool_turn_with_hosts(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                if sent == 2 {
                    // 第二次请求发出后、响应返回前提交答案：即便答案已受理，
                    // 非批末等待批仍须整批拒绝
                    inject_host.answer(0, answered(&q_id("pub"), "内部评审"));
                }
                Ok(if sent == 1 {
                    tool_response(vec![call(
                        "pub",
                        "request_user_input_async",
                        &publish_args(),
                    )])
                } else {
                    tool_response(batch.clone())
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            None,
            Some(&mut async_host),
        false
        )
        .unwrap_err();
        assert!(
            error.contains("最后一条") && error.contains("整批拒绝"),
            "{error}"
        );
        assert_eq!(sent, 2, "整批拒绝后不得再请求");
        // 该批不入史：历史仅剩本轮输入、发布调用与发布确认，外加协议错误
        // 收尾保存的已受理答案（§8.5）；批内调用与中间命令结果均不出现
        assert_eq!(history.len(), 4, "非法批不入史：{history:?}");
        assert!(
            !history.iter().any(|h| matches!(h,
                HistoryItem::ToolResult { tool_call_id, .. } if tool_call_id != "pub")),
            "非法批的任何调用不得产生结果：{history:?}"
        );
        assert!(!probe.exists(), "未知答案的中间命令不得执行");
        // 已受理答案按 §8.5 保留（协议错误收尾先保存已答）
        assert!(answer_event_of(&history, "pub").is_some(), "已受理答案保留");
    }
}

/// 审计修复 2（§8.5）：协议解析失败不丢已受理答案——答案恰一次入史，
/// 已答问题不重记为未答关闭；未提交回答时仍按真实原因关闭。
#[test]
fn protocol_error_preserves_already_accepted_answer() {
    for with_answer in [true, false] {
        let host = AsyncHost::default();
        let mut history = Vec::new();
        let mut sent = 0;
        let inject = host.clone();
        let mut async_host = host.hook();
        let error = turn::run_tool_turn_with_hosts(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                if sent == 2 && with_answer {
                    inject.answer(0, answered(&q_id("pub"), "内部评审"));
                }
                Ok(if sent == 1 {
                    tool_response(vec![call(
                        "pub",
                        "request_user_input_async",
                        &publish_args(),
                    )])
                } else {
                    // tool_calls 非法形状 → 协议错误
                    json!({"choices":[{"message":{"tool_calls":42}}]})
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            None,
            Some(&mut async_host),
        false
        )
        .unwrap_err();
        assert!(error.contains("协议错误"), "{error}");
        assert_eq!(sent, 2);
        if with_answer {
            assert!(
                answer_event_of(&history, "pub").is_some(),
                "已受理答案在协议错误路径仍保留"
            );
            assert!(
                close_event_of(&history, "pub").is_none(),
                "已答问题不记为未答关闭"
            );
        } else {
            assert!(
                answer_event_of(&history, "pub").is_none(),
                "未提交回答不得凭空出现"
            );
            let (_, closed) = close_event_of(&history, "pub").expect("协议错误关闭未决问题");
            assert_eq!(closed["reason"], json!("model_error"));
        }
    }
}

/// 审计修复 3（§8.4）：答案批末入史后，同轮同 ID 再等待只确认就绪，
/// 不重问、不重复记录；未知 ID 仍拒绝。
#[test]
fn later_batch_wait_for_recorded_answer_returns_ready() {
    let host = AsyncHost::default();
    let mut history = Vec::new();
    let mut sent = 0;
    // 第 2 次请求发出前注入回答：该批 wait 就绪、批末入史；第 3 批再等待
    // 已入史的同轮 ID 应返回 ready；第 4 批未知 ID 仍拒绝
    let inject_host = host.clone();
    let mut async_host = host.hook();
    let result = turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            if sent == 2 {
                inject_host.answer(0, answered(&q_id("pub"), "内部评审"));
            }
            Ok(match sent {
                1 => tool_response(vec![call(
                    "pub",
                    "request_user_input_async",
                    &publish_args(),
                )]),
                2 => tool_response(vec![call("w", "wait_for_user_input", &wait_args("pub"))]),
                3 => tool_response(vec![call("w2", "wait_for_user_input", &wait_args("pub"))]),
                4 => tool_response(vec![call(
                    "bad",
                    "wait_for_user_input",
                    &wait_args("unknown-id"),
                )]),
                _ => final_response("完成"),
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host),
        false
    )
    .unwrap();
    assert_eq!(result, "完成");
    assert_eq!(sent, 5);
    let results = tool_results(&history);
    assert_eq!(
        results.iter().find(|(id, _)| id == "w").unwrap().1,
        json!({"ready":true,"request_id":"pub"})
    );
    assert_eq!(
        results.iter().find(|(id, _)| id == "w2").unwrap().1,
        json!({"ready":true,"request_id":"pub"}),
        "同轮已入史答案的 ID 再等待只确认就绪，不重问"
    );
    let bad = results
        .iter()
        .find(|(id, _)| id == "bad")
        .unwrap()
        .1
        .clone();
    assert!(
        bad["error"].as_str().unwrap().contains("没有未决问询"),
        "未知 ID 不得确认就绪：{bad}"
    );
    assert_eq!(
        history
            .iter()
            .filter(|h| matches!(
                h,
                HistoryItem::Text(HistoryMessage {
                    user_input_event: Some(UserInputEvent::Answer { .. }),
                    ..
                })
            ))
            .count(),
        1,
        "再等待不重复记录答案"
    );
    // 跨轮：已入史答案的 ID 只在本轮有效——新用户轮等待旧 ID 仍拒绝，
    // 不得命中上一轮的就绪记录（recorded_answer_ids 不跨轮泄漏）
    let mut sent2 = 0;
    let mut async_host2 = host.hook();
    turn::run_tool_turn_with_hosts(
        &mut history,
        "下一轮",
        &mut |_| {
            sent2 += 1;
            Ok(if sent2 == 1 {
                tool_response(vec![call("w3", "wait_for_user_input", &wait_args("pub"))])
            } else {
                final_response("完成")
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host2),
        false
    )
    .unwrap();
    let cross_turn = tool_results(&history)
        .into_iter()
        .find(|(id, _)| id == "w3")
        .unwrap()
        .1;
    assert!(
        cross_turn["error"]
            .as_str()
            .unwrap()
            .contains("没有未决问询"),
        "其他轮 ID 不得确认就绪：{cross_turn}"
    );
}

/// 审计收尾（§8.4）：同轮 Q1 已答入史、Q2 尚未回答——wait Q1 只确认就绪
/// 且不消费 Q2 通道；wait Q2 必须等待 Q2 自己的回答；两份回答各恰一次保存。
#[test]
fn wait_answered_id_keeps_second_pending_independent() {
    let host = AsyncHost::default();
    let (wait_tx, wait_rx) = mpsc::channel::<String>();
    let (done_tx, done_rx) =
        mpsc::channel::<(Result<String, String>, Vec<HistoryItem>, usize, Value)>();
    let mut history = Vec::new();
    let worker = {
        let host = host.clone();
        let wait_tx = wait_tx.clone();
        std::thread::spawn(move || {
            let mut async_host = host.hook();
            let mut sent = 0;
            let mut last_body = Value::Null;
            let result = turn::run_tool_turn_with_hosts(
                &mut history,
                "go",
                &mut |body| {
                    sent += 1;
                    if sent == 6 {
                        last_body = body.clone();
                    }
                    Ok(match sent {
                        1 => tool_response(vec![call(
                            "pub1",
                            "request_user_input_async",
                            &publish_args(),
                        )]),
                        2 => tool_response(vec![call(
                            "w1",
                            "wait_for_user_input",
                            &wait_args("pub1"),
                        )]),
                        3 => tool_response(vec![call(
                            "pub2",
                            "request_user_input_async",
                            &publish_args(),
                        )]),
                        4 => tool_response(vec![call(
                            "w1b",
                            "wait_for_user_input",
                            &wait_args("pub1"),
                        )]),
                        5 => tool_response(vec![call(
                            "w2",
                            "wait_for_user_input",
                            &wait_args("pub2"),
                        )]),
                        _ => final_response("S104-双问完成"),
                    })
                },
                &mut |n| {
                    if let turn::TurnNotify::UserInputAsyncWait { request_id } = &n {
                        wait_tx.send(request_id.clone()).unwrap();
                    }
                },
                None,
                &mut |_| {},
                ToolMode::User,
                &ProcessManager::new(),
                None,
                Some(&mut async_host),
        false
            );
            let _ = done_tx.send((result, history, sent, last_body));
        })
    };
    // 批 2：wait Q1 阻塞中提交 Q1 答案——就绪、批末入史
    assert_eq!(
        wait_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        "pub1"
    );
    host.answer(0, answered(&q_id("pub1"), "内部评审"));
    // 批 4 已就绪返回（不发等待通知）：下一通知必须是 Q2 的等待本身。
    // 若 wait Q1 误阻塞或吞 Q2 通道，这里收到的是第二个 "pub1" 或超时
    assert_eq!(
        wait_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        "pub2",
        "wait Q1 就绪确认后，Q2 仍保持未决并进入真实等待"
    );
    assert_eq!(host.publish_count(), 2);
    host.answer(1, answered(&q_id("pub2"), "外部客户"));
    let (result, history, sent, last_body) = done_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("看门狗：双问流程未在时限内完成");
    assert_eq!(result.unwrap(), "S104-双问完成");
    assert_eq!(sent, 6);
    let results = tool_results(&history);
    assert_eq!(
        results.iter().find(|(id, _)| id == "w1").unwrap().1,
        json!({"ready":true,"request_id":"pub1"})
    );
    assert_eq!(
        results.iter().find(|(id, _)| id == "w1b").unwrap().1,
        json!({"ready":true,"request_id":"pub1"}),
        "已入史 Q1 的再等待＝就绪确认，不重问"
    );
    assert_eq!(
        results.iter().find(|(id, _)| id == "w2").unwrap().1,
        json!({"ready":true,"request_id":"pub2"}),
        "wait Q2 等到的是 Q2 自己的回答"
    );
    // 两份回答各恰一次入史，无关闭事件（两题都已回答）
    assert!(answer_event_of(&history, "pub1").is_some());
    assert!(answer_event_of(&history, "pub2").is_some());
    assert_eq!(
        history
            .iter()
            .filter(|h| matches!(
                h,
                HistoryItem::Text(HistoryMessage {
                    user_input_event: Some(_),
                    ..
                })
            ))
            .count(),
        2,
        "恰两份答案事件，无关闭"
    );
    // 最终请求同时携带两份答案，且 Q1 先于 Q2
    let msgs = last_body["messages"].as_array().unwrap();
    let q1_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == json!("user")
                && m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("内部评审"))
        })
        .expect("Q1 答案进入最终请求");
    let q2_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == json!("user")
                && m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("外部客户"))
        })
        .expect("Q2 答案进入最终请求");
    assert!(q1_pos < q2_pos);
    let _ = worker.join();
}

/// 验收行 4（就绪）：已答 ID 再等待立即返回 ready，不重复入史、不重写回答。
#[test]
fn wait_already_answered_returns_ready_immediately() {
    let host = AsyncHost::default();
    let mut history = Vec::new();
    let mut sent = 0;
    // 在第 2 次请求发出前注入回答（sender 闭包内＝响应返回前），post-response
    // 检查点先暂存，wait 执行时已答——立即就绪、不进入阻塞
    let inject_host = host.clone();
    let mut async_host = host.hook();
    let result = turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            if sent == 2 {
                inject_host.answer(0, answered(&q_id("pub"), "内部评审"));
            }
            Ok(match sent {
                1 => tool_response(vec![call(
                    "pub",
                    "request_user_input_async",
                    &publish_args(),
                )]),
                2 => tool_response(vec![call("w", "wait_for_user_input", &wait_args("pub"))]),
                _ => final_response("完成"),
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host),
        false
    );
    assert_eq!(result.unwrap(), "完成");
    let results = tool_results(&history);
    assert_eq!(
        results.iter().find(|(id, _)| id == "w").unwrap().1,
        json!({"ready":true,"request_id":"pub"})
    );
    assert_eq!(
        history
            .iter()
            .filter(|h| matches!(
                h,
                HistoryItem::Text(HistoryMessage {
                    user_input_event: Some(UserInputEvent::Answer { .. }),
                    ..
                })
            ))
            .count(),
        1,
        "回答恰一次入史，ready 不是第二份回答"
    );
}

/// 验收行 6（单项/归属）：等待未知/不匹配 ID 报错；重复发布与异步未决时的
/// 同步问询均返回占用错误，不替换旧题、不暗中等待。
#[test]
fn occupancy_and_mismatch_errors_keep_single_pending_item() {
    let host = AsyncHost::default();
    let sync_calls = Arc::new(AtomicUsize::new(0));
    let mut history = Vec::new();
    let mut sent = 0;
    let calls = sync_calls.clone();
    let mut sync_host = move |call: &turn::ParsedToolCall, q: &[Question]| {
        calls.fetch_add(1, Ordering::SeqCst);
        sync_answer_host()(call, q)
    };
    let inject = host.clone();
    let mut async_host = host.hook();
    turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(match sent {
                1 => tool_response(vec![
                    call("pub", "request_user_input_async", &publish_args()),
                    call("pub2", "request_user_input_async", &publish_args()),
                    call("sync", "request_user_input", &sync_args()),
                    call("wait", "wait_for_user_input", &wait_args("other-id")),
                ]),
                _ => final_response("完成"),
            })
        },
        &mut |n| {
            // 等待调用开始前注入「pub」的回答：错 ID 的等待不得吞下它
            if matches!(n, turn::TurnNotify::ToolBegin { ref name, .. } if name == "wait_for_user_input")
            {
                inject.answer(0, answered(&q_id("pub"), "内部评审"));
            }
        },
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut sync_host),
        Some(&mut async_host),
        false
    )
    .unwrap();
    assert_eq!(host.publish_count(), 1, "第二项发布被占用错误拒绝");
    assert_eq!(
        sync_calls.load(Ordering::SeqCst),
        0,
        "异步未决时同步问询不进入宿主"
    );
    let results = tool_results(&history);
    let by_id = |id: &str| results.iter().find(|(rid, _)| rid == id).unwrap().1.clone();
    assert_eq!(by_id("pub"), json!({"accepted":true,"request_id":"pub"}));
    let pub2 = by_id("pub2");
    assert!(
        pub2["error"].as_str().unwrap().contains("已有未决问询"),
        "{pub2}"
    );
    let sync = by_id("sync");
    assert!(
        sync["error"].as_str().unwrap().contains("未决异步问询"),
        "{sync}"
    );
    let wait = by_id("wait");
    assert!(
        wait["error"].as_str().unwrap().contains("不匹配"),
        "其他轮/未知 ID 不得夺取未决问题（即使其答案已到）：{wait}"
    );
}

/// 验收行 5/8：等待中取消（Esc 语义）与通道失败——只记录一次正确终态，
/// 未决问题按真实原因关闭，不遗留等待。
#[test]
fn cancel_and_channel_failure_during_wait_close_with_real_reason() {
    // 场景一：取消标志（Esc 置位）
    {
        let host = AsyncHost::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let (wait_started_tx, wait_started_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) =
            mpsc::channel::<(Result<String, String>, Vec<HistoryItem>, usize)>();
        let mut history = Vec::new();
        {
            let host = host.clone();
            let cancel = cancel.clone();
            let wait_started_tx = wait_started_tx.clone();
            std::thread::spawn(move || {
                let mut async_host = host.hook();
                let mut sent = 0;
                let result = turn::run_tool_turn_with_hosts(
                    &mut history,
                    "go",
                    &mut |_| {
                        sent += 1;
                        Ok(match sent {
                            1 => tool_response(vec![call(
                                "pub",
                                "request_user_input_async",
                                &publish_args(),
                            )]),
                            _ => tool_response(vec![call(
                                "w",
                                "wait_for_user_input",
                                &wait_args("pub"),
                            )]),
                        })
                    },
                    &mut |n| {
                        if matches!(n, turn::TurnNotify::UserInputAsyncWait { .. }) {
                            wait_started_tx.send(()).unwrap();
                        }
                    },
                    Some(&cancel),
                    &mut |_| {},
                    ToolMode::User,
                    &ProcessManager::new(),
                    None,
                    Some(&mut async_host),
        false
                );
                let _ = done_tx.send((result, history, sent));
            });
        }
        wait_started_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        cancel.store(true, Ordering::SeqCst);
        // 看门狗：取消必须解除等待（防「取消不醒」类缺陷挂死测试）
        let (result, history, sent) = done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("取消未解除问询等待（看门狗超时）");
        let error = result.unwrap_err();
        assert!(error.contains("用户已取消"), "{error}");
        assert_eq!(sent, 2, "取消后不再请求模型");
        let results = tool_results(&history);
        assert_eq!(
            results.iter().find(|(id, _)| id == "w").unwrap().1,
            json!({"error":"用户已取消本轮：问询未完成"}),
            "等待调用记录一次真实终态"
        );
        let (_, closed) = close_event_of(&history, "pub").expect("取消关闭未决问题");
        assert_eq!(closed["reason"], json!("cancelled"));
        assert_eq!(
            history
                .iter()
                .filter(|h| matches!(
                    h,
                    HistoryItem::Text(HistoryMessage {
                        user_input_event: Some(_),
                        ..
                    })
                ))
                .count(),
            1,
            "关闭事件恰一条"
        );
    }
    // 场景二：通道失败（宿主退出语义）经回执通道送达
    {
        let host = AsyncHost::default();
        let mut history = Vec::new();
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let worker = {
            let host = host.clone();
            let ready_tx = ready_tx.clone();
            std::thread::spawn(move || {
                let mut async_host = host.hook();
                let mut sent = 0;
                let result = turn::run_tool_turn_with_hosts(
                    &mut history,
                    "go",
                    &mut |_| {
                        sent += 1;
                        Ok(if sent == 1 {
                            tool_response(vec![call(
                                "pub",
                                "request_user_input_async",
                                &publish_args(),
                            )])
                        } else {
                            ready_tx.send(()).unwrap();
                            tool_response(vec![call("w", "wait_for_user_input", &wait_args("pub"))])
                        })
                    },
                    &mut |_| {},
                    None,
                    &mut |_| {},
                    ToolMode::User,
                    &ProcessManager::new(),
                    None,
                    Some(&mut async_host),
        false
                );
                (result, history, sent)
            })
        };
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        host.wait_publish(1);
        host.fail(0, InputError::HostExited);
        let (result, history, sent) = worker.join().unwrap();
        assert_eq!(result.unwrap_err(), "宿主退出：问询未完成");
        assert_eq!(sent, 2);
        let (_, closed) = close_event_of(&history, "pub").expect("通道失败关闭未决问题");
        assert_eq!(closed["reason"], json!("host_exit"));
    }
}

/// 验收行 5：结束检查前已受理回答——终文不入史为任务完成，原轮继续让模型处理。
#[test]
fn terminal_text_with_received_answer_continues_original_turn() {
    let host = AsyncHost::default();
    let mut history = Vec::new();
    let mut sent = 0;
    let mut third_body = Value::Null;
    // 第 2 次请求发出前注入回答：响应为无工具终文，结束检查先处理已受理回答
    let inject_host = host.clone();
    let mut async_host = host.hook();
    let reply = turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |body| {
            sent += 1;
            if sent == 2 {
                inject_host.answer(0, answered(&q_id("pub"), "内部评审"));
            }
            if sent == 3 {
                third_body = body.clone();
            }
            Ok(match sent {
                1 => tool_response(vec![call(
                    "pub",
                    "request_user_input_async",
                    &publish_args(),
                )]),
                2 => final_response("S104-草稿终文"),
                _ => final_response("S104-最终完成"),
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host),
        false
    )
    .unwrap();
    assert_eq!(reply, "S104-最终完成", "草稿终文不提前交付");
    assert_eq!(sent, 3);
    // 草稿终文如实入史（assistant），随后回答事件（user）
    let draft_pos = history
        .iter()
        .position(|h| {
            matches!(h, HistoryItem::Text(HistoryMessage { role: HistoryRole::Assistant, content, .. }) if content == "S104-草稿终文")
        })
        .expect("草稿终文如实保留");
    let answer_pos = history
        .iter()
        .position(|h| {
            matches!(h, HistoryItem::Text(HistoryMessage { user_input_event: Some(UserInputEvent::Answer { request_id }), .. }) if request_id == "pub")
        })
        .unwrap();
    assert!(draft_pos < answer_pos);
    let msgs = third_body["messages"].as_array().unwrap();
    assert!(msgs
        .iter()
        .any(|m| m["role"] == json!("assistant") && m["content"] == json!("S104-草稿终文")));
    assert!(msgs.iter().any(|m| m["role"] == json!("user")
        && m["content"]
            .as_str()
            .is_some_and(|c| c.contains("内部评审"))));
}

/// 验收行 12：额度——第 8 次请求不再发布新异步问题/不开始阻塞等待；
/// 已到回答仍保存；未决项关闭为 request_limit；无第 9 次请求。
#[test]
fn eighth_request_rejects_publish_and_wait_and_saves_arrived_answer() {
    // 场景一：第 8 批含新发布与等待——各自返回额度错误，整轮按上限收尾
    {
        let host = AsyncHost::default();
        let mut history = Vec::new();
        let mut sent = 0;
        let mut async_host = host.hook();
        let error = turn::run_tool_turn_with_hosts(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                Ok(match sent {
                    1 => tool_response(vec![call(
                        "pub",
                        "request_user_input_async",
                        &publish_args(),
                    )]),
                    8 => tool_response(vec![
                        call("late", "request_user_input_async", &publish_args()),
                        call("w", "wait_for_user_input", &wait_args("pub")),
                    ]),
                    _ => tool_response(vec![call(
                        &format!("p{sent}"),
                        "update_plan",
                        r#"{"plan":[]}"#,
                    )]),
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            None,
            Some(&mut async_host),
        false
        )
        .unwrap_err();
        assert!(error.contains("上限（8）"), "{error}");
        assert_eq!(sent, 8, "无第 9 次请求");
        assert_eq!(host.publish_count(), 1, "第 8 批不发布新问题");
        let results = tool_results(&history);
        let late = results
            .iter()
            .find(|(id, _)| id == "late")
            .unwrap()
            .1
            .clone();
        assert!(
            late["error"]
                .as_str()
                .unwrap()
                .contains("不再发布新的异步问询"),
            "{late}"
        );
        let wait = results.iter().find(|(id, _)| id == "w").unwrap().1.clone();
        assert!(
            wait["error"]
                .as_str()
                .unwrap()
                .contains("不再为未答问题开始阻塞等待"),
            "第 8 批不阻塞等待：{wait}"
        );
        let (_, closed) = close_event_of(&history, "pub").expect("额度耗尽关闭未决问题");
        assert_eq!(closed["reason"], json!("request_limit"));
    }
    // 场景二：第 8 批期间回答到达——已受理答案保存，不发第 9 次、不关闭为未答
    {
        let host = AsyncHost::default();
        let mut history = Vec::new();
        let mut sent = 0;
        let inject_host = host.clone();
        let mut async_host = host.hook();
        let error = turn::run_tool_turn_with_hosts(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                if sent == 8 {
                    inject_host.answer(0, answered(&q_id("pub"), "外部客户"));
                }
                Ok(match sent {
                    1 => tool_response(vec![call(
                        "pub",
                        "request_user_input_async",
                        &publish_args(),
                    )]),
                    _ => tool_response(vec![call(
                        &format!("p{sent}"),
                        "update_plan",
                        r#"{"plan":[]}"#,
                    )]),
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            None,
            Some(&mut async_host),
        false
        )
        .unwrap_err();
        assert!(error.contains("上限（8）"), "{error}");
        assert_eq!(sent, 8);
        assert!(answer_event_of(&history, "pub").is_some(), "已到回答仍保存");
        assert!(
            close_event_of(&history, "pub").is_none(),
            "已答问题不再记为未答关闭"
        );
    }
}

/// 验收行 13：能力与权限——TUI 七工具/同步五工具/无宿主四工具/内部记忆轮三项；
/// 伪造工具名不能绕过能力检查；MemoryMerge 即使误传宿主也不声明不执行。
#[test]
fn tool_counts_and_forged_calls_respect_capability_gates() {
    let temp = common::TempDir::new("018b-caps");
    let host = AsyncHost::default();
    for (sync_on, async_on, expected) in [(true, true, 7usize), (true, false, 5), (false, false, 4)]
    {
        let mut history = Vec::new();
        let mut sent = 0;
        let mut async_host = host.hook();
        let mut sync_host = sync_answer_host();
        let sync_opt: Option<&mut terminal_chat::user_input::InputHost> =
            if sync_on { Some(&mut sync_host) } else { None };
        turn::run_tool_turn_with_hosts(
            &mut history,
            "go",
            &mut |body| {
                let tools = body["tools"].as_array().unwrap();
                assert_eq!(tools.len(), expected);
                let names: Vec<&str> = tools
                    .iter()
                    .map(|t| t["function"]["name"].as_str().unwrap())
                    .collect();
                assert_eq!(names.contains(&"request_user_input"), sync_on);
                assert_eq!(
                    names.contains(&"request_user_input_async")
                        && names.contains(&"wait_for_user_input"),
                    async_on
                );
                sent += 1;
                Ok(final_response("完成"))
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            sync_opt,
            if async_on {
                Some(&mut async_host)
            } else {
                None
            },
        false
        )
        .unwrap();
        let _ = sent;
    }
    // 无异步宿主：伪造发布/等待调用（等待在批末）——执行层拒绝，不进入宿主，
    // 按工具错误结果回传后正常继续，不升级为协议失败
    let mut history = Vec::new();
    let mut sent = 0;
    let result = turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(if sent == 1 {
                tool_response(vec![
                    call("forged", "request_user_input_async", &publish_args()),
                    call("forged-w", "wait_for_user_input", &wait_args("x")),
                ])
            } else {
                final_response("完成")
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        None,
        false
    )
    .unwrap();
    assert_eq!(result, "完成");
    assert_eq!(host.publish_count(), 0, "伪造名称不得触达宿主");
    let results = tool_results(&history);
    let forged = results
        .iter()
        .find(|(id, _)| id == "forged")
        .unwrap()
        .1
        .clone();
    assert!(
        forged["error"].as_str().unwrap().contains("不可用"),
        "{forged}"
    );
    let forged_w = results
        .iter()
        .find(|(id, _)| id == "forged-w")
        .unwrap()
        .1
        .clone();
    assert!(
        forged_w["error"].as_str().unwrap().contains("不可用"),
        "{forged_w}"
    );
    // MemoryMerge：误传两个宿主——不声明、不执行，仅 mem 三工具
    let mut history = Vec::new();
    let mut sent = 0;
    let mut async_host = host.hook();
    let mut sync_host = sync_answer_host();
    turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |body| {
            assert_eq!(body["tools"].as_array().unwrap().len(), 3);
            sent += 1;
            Ok(if sent == 1 {
                tool_response(vec![
                    call("f1", "request_user_input_async", &publish_args()),
                    call("f2", "request_user_input", &sync_args()),
                    call("f3", "wait_for_user_input", &wait_args("pub")),
                ])
            } else {
                final_response("完成")
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::MemoryMerge {
            root: temp.path(),
            system: "internal",
            expected_epoch: 0,
        },
        &ProcessManager::new(),
        Some(&mut sync_host),
        Some(&mut async_host),
        false
    )
    .unwrap();
    assert_eq!(host.publish_count(), 0, "合并轮不执行问询发布");
    let results = tool_results(&history);
    for (id, v) in &results {
        assert!(
            v["error"].as_str().unwrap().contains("未知工具"),
            "合并轮伪造问询调用按未知工具拒绝：{id} {v}"
        );
    }
    let _ = temp;
}

/// 验收行 6（迟到/重复）：回答入史并释放后，同通道迟到提交必须失效。
#[test]
fn late_answer_after_release_is_rejected_by_one_shot_channel() {
    let host = AsyncHost::default();
    let mut history = Vec::new();
    let mut sent = 0;
    let inject_host = host.clone();
    let mut async_host = host.hook();
    turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            if sent == 2 {
                inject_host.answer(0, answered(&q_id("pub"), "内部评审"));
            }
            Ok(match sent {
                1 => tool_response(vec![call(
                    "pub",
                    "request_user_input_async",
                    &publish_args(),
                )]),
                _ => final_response("完成"),
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        None,
        Some(&mut async_host),
        false
    )
    .unwrap();
    // 回答已入史、单项已释放：同通道重复提交送不进旧请求
    assert!(
        host.try_answer(0, answered(&q_id("pub"), "重复回答"))
            .is_err(),
        "迟到/重复回答必须失效"
    );
    assert_eq!(
        history
            .iter()
            .filter(|h| matches!(
                h,
                HistoryItem::Text(HistoryMessage {
                    user_input_event: Some(UserInputEvent::Answer { .. }),
                    ..
                })
            ))
            .count(),
        1
    );
}

/// 验收行 6：同步＋异步同轮并存——同步问答完成、异步发布正常；已受理回答后
/// 取消仍保留答案（不把已答问题重记为未答）。
#[test]
fn sync_and_async_coexist_and_cancel_preserves_accepted_answer() {
    let host = AsyncHost::default();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut history = Vec::new();
    let mut sent = 0;
    let inject_host = host.clone();
    let mut async_host = host.hook();
    let mut sync_host = sync_answer_host();
    let error = turn::run_tool_turn_with_hosts(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            if sent == 2 {
                inject_host.answer(0, answered(&q_id("pub"), "内部评审"));
            }
            Ok(if sent == 1 {
                tool_response(vec![
                    call("sync", "request_user_input", &sync_args()),
                    call("pub", "request_user_input_async", &publish_args()),
                ])
            } else {
                // 第 2 批执行前置取消：回答已受理、批结果保存后停止续请求
                cancel.store(true, Ordering::SeqCst);
                tool_response(vec![call("p2", "update_plan", r#"{"plan":[]}"#)])
            })
        },
        &mut |_| {},
        Some(&cancel),
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut sync_host),
        Some(&mut async_host),
        false
    )
    .unwrap_err();
    assert!(
        error.starts_with(turn::CANCELLED_TURN_ERROR_PREFIX),
        "{error}"
    );
    assert_eq!(sent, 2, "已受理回答后取消不再续请求");
    let results = tool_results(&history);
    assert_eq!(
        results.iter().find(|(id, _)| id == "sync").unwrap().1,
        json!({"answers":{"q":{"answers":["A"]}}}),
        "同步问答正常完成"
    );
    assert_eq!(
        results.iter().find(|(id, _)| id == "pub").unwrap().1,
        json!({"accepted":true,"request_id":"pub"})
    );
    assert!(
        answer_event_of(&history, "pub").is_some(),
        "已受理答案在取消后保留"
    );
    assert!(
        close_event_of(&history, "pub").is_none(),
        "已答问题不重记为未答关闭"
    );
}

/// 验收行 11（兼容）：来源标记经 V1 保存/装载保真；旧程序重存丢字段降级为
/// 普通历史文本（正文完整保留），旧记录缺字段可读。
#[test]
fn session_roundtrip_preserves_marker_and_old_resave_degrades() {
    let temp = common::TempDir::new("018b-session");
    let dir = temp.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let history = vec![
        HistoryItem::Text(HistoryMessage {
            role: HistoryRole::User,
            content: "go".into(),
            user_input_event: None,
        }),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![terminal_chat::prompt::ToolCallRecord {
                id: "pub".into(),
                name: "request_user_input_async".into(),
                arguments: publish_args(),
            }],
            content: None,
        },
        HistoryItem::ToolResult {
            image: None,
            tool_call_id: "pub".into(),
            content: user_input_async::accepted_result("pub"),
            display: None,
        },
        HistoryItem::Text(HistoryMessage {
            role: HistoryRole::User,
            content: user_input_async::answer_event_content(
                "pub",
                &answered(&q_id("pub"), "内部评审"),
            ),
            user_input_event: Some(UserInputEvent::Answer {
                request_id: "pub".into(),
            }),
        }),
    ];
    terminal_chat::session::save_snapshot(&dir, "snap018b", &history).unwrap();
    let loaded = terminal_chat::session::load_session_file(&dir.join("snap018b.jsonl")).unwrap();
    assert_eq!(loaded.history, history, "新保存→装载：标记保真");
    // 旧程序重存模拟：序列化行剥离 user_input_event 字段后可读，正文保留、标记丢失
    let line = serde_json::to_value(&history[3]).unwrap();
    let stripped = {
        let mut v = line.clone();
        v["user_input_event"] = json!("__STRIP__"); // 占位模拟旧程序丢字段
        v.as_object_mut().unwrap().remove("user_input_event");
        serde_json::from_value::<HistoryItem>(v).unwrap()
    };
    match stripped {
        HistoryItem::Text(HistoryMessage {
            content,
            user_input_event: None,
            ..
        }) => {
            assert!(content.contains("内部评审"), "答案正文保留：{content}");
        }
        other => panic!("旧程序重存后应为普通文本：{other:?}"),
    }
    // 旧 V1 记录（无字段）装载为 None
    let old_line = serde_json::to_string(&HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: "旧记录".into(),
        user_input_event: None,
    }))
    .unwrap();
    let manual = format!("{}\n{}\n", terminal_chat::session::META_LINE, old_line);
    std::fs::write(dir.join("oldv1.jsonl"), manual).unwrap();
    let loaded_old = terminal_chat::session::load_session_file(&dir.join("oldv1.jsonl")).unwrap();
    assert_eq!(loaded_old.history.len(), 1);
}

/// 审查修复回归（§8.5）：模型请求失败也关闭未决项并保留已受理回答。
#[test]
fn sender_failure_closes_pending_and_preserves_accepted_answer() {
    for with_answer in [true, false] {
        let host = AsyncHost::default();
        let mut history = Vec::new();
        let mut sent = 0;
        let inject = host.clone();
        let mut async_host = host.hook();
        let error = turn::run_tool_turn_with_hosts(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                if sent == 1 {
                    Ok(tool_response(vec![call(
                        "pub",
                        "request_user_input_async",
                        &publish_args(),
                    )]))
                } else {
                    if with_answer {
                        inject.answer(0, answered(&q_id("pub"), "内部评审"));
                    }
                    Err("模型请求失败".to_string())
                }
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            None,
            Some(&mut async_host),
        false
        )
        .unwrap_err();
        assert_eq!(error, "模型请求失败");
        assert_eq!(sent, 2);
        if with_answer {
            assert!(
                answer_event_of(&history, "pub").is_some(),
                "已受理回答在模型失败路径仍保留"
            );
            assert!(
                close_event_of(&history, "pub").is_none(),
                "已答问题不记为未答关闭"
            );
        } else {
            assert!(
                answer_event_of(&history, "pub").is_none(),
                "未提交回答不得凭空出现"
            );
            let (_, closed) = close_event_of(&history, "pub").expect("模型失败关闭未决问题");
            assert_eq!(closed["reason"], json!("model_error"));
        }
    }
}
