mod common;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc,
};
use terminal_chat::user_input::parse_arguments;
use terminal_chat::{
    proc_session::ProcessManager,
    prompt::HistoryItem,
    turn::{self, ToolMode},
    user_input::{Answer, Response},
};

fn question_args() -> String {
    json!({"questions":[{"id":"q","header":"方向","question":"选择？","options":[{"label":"A","description":"说明"}]}]}).to_string()
}
fn call(id: &str, name: &str, arguments: &str) -> serde_json::Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}})
}
fn tool_response(calls: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"choices":[{"message":{"tool_calls":calls}}]})
}
fn final_response() -> serde_json::Value {
    json!({"choices":[{"message":{"content":"done"}}]})
}
fn answered() -> Response {
    Response {
        answers: [(
            "q".into(),
            Answer {
                answers: vec!["A".into()],
            },
        )]
        .into(),
    }
}

#[test]
fn model_questions_normalize_other_and_preserve_raw_ids() {
    let raw = json!({"questions":[{"id":" 原始ID ","header":"方式","question":"怎么做？",
        "options":[{"label":"沿用","description":"保持兼容"}]}]})
    .to_string();
    let questions = parse_arguments(&raw).unwrap();
    assert_eq!(questions[0].id, " 原始ID ");
    assert!(questions[0].is_other);
    assert!(!questions[0].is_secret);
    assert_eq!(questions[0].options.as_ref().unwrap()[0].label, "沿用");
}

#[test]
fn schema_rejects_unknown_missing_and_invalid_values_without_quantity_caps() {
    let q =
        json!({"id":"q","header":"h","question":"?","options":[{"label":"a","description":"d"}]});
    let mut cases = vec![
        "not json".to_string(),
        "{}".into(),
        json!({"questions":[]}).to_string(),
    ];
    for key in ["id", "header", "question", "options"] {
        let mut bad = q.clone();
        bad.as_object_mut().unwrap().remove(key);
        cases.push(json!({"questions":[bad]}).to_string());
        let mut bad = q.clone();
        bad[key] = json!(5);
        cases.push(json!({"questions":[bad]}).to_string());
    }
    for field in [
        "isOther",
        "isSecret",
        "isBlocking",
        "autoResolutionMs",
        "extra",
    ] {
        let mut bad = q.clone();
        bad[field] = json!(true);
        cases.push(json!({"questions":[bad]}).to_string());
        let mut bad = json!({"questions":[q.clone()]});
        bad[field] = json!(true);
        cases.push(bad.to_string());
    }
    let mut blank_id = q.clone();
    blank_id["id"] = json!(" \t ");
    let mut empty_opts = q.clone();
    empty_opts["options"] = json!([]);
    let mut nested_extra = q.clone();
    nested_extra["options"][0]["extra"] = json!(true);
    cases.extend(
        [
            json!({"questions":[blank_id]}),
            json!({"questions":[q.clone(),q.clone()]}),
            json!({"questions":[empty_opts]}),
            json!({"questions":[nested_extra]}),
        ]
        .map(|v| v.to_string()),
    );
    for raw in cases {
        assert!(parse_arguments(&raw).is_err(), "错误接收：{raw}");
    }
    // 上游的 1-3/2-3 为提示，不额外设运行时硬限制。
    let many: Vec<_> = (0..5)
        .map(|i| {
            let mut v = q.clone();
            v["id"] = json!(format!("q{i}"));
            v["options"] = json!((0..5)
                .map(|_| json!({"label":"a","description":"d"}))
                .collect::<Vec<_>>());
            v
        })
        .collect();
    assert_eq!(
        parse_arguments(&json!({"questions":many}).to_string())
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn interactive_turn_waits_for_explicit_answer_before_next_tool_or_request() {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (answer_tx, answer_rx) = mpsc::channel();
    let (record_tx, record_rx) = mpsc::channel();
    let requests = Arc::new(AtomicUsize::new(0));
    let sent = requests.clone();
    let worker = std::thread::spawn(move || {
        let mut history = Vec::new();
        let mut sender = |body: &serde_json::Value| {
            assert_eq!(body["tools"].as_array().unwrap().len(), 5);
            if sent.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(json!({"choices":[{"message":{"tool_calls":[
                    {"id":"ask","type":"function","function":{"name":"request_user_input","arguments":json!({"questions":[{"id":" q ","header":"方向","question":"选择？","options":[{"label":"保留","description":"兼容"}]}]}).to_string()}},
                    {"id":"after","type":"function","function":{"name":"update_plan","arguments":"{\"plan\":[]}"}}
                ]}}]}))
            } else {
                let tool = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["tool_call_id"] == "ask")
                    .unwrap();
                let result: serde_json::Value =
                    serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
                assert_eq!(
                    result,
                    json!({"answers":{" q ":{"answers":["保留","user_note: 中文补充"]}}})
                );
                Ok(json!({"choices":[{"message":{"content":"继续完成"}}]}))
            }
        };
        let mut host = |_: &turn::ParsedToolCall,
                        questions: &[terminal_chat::user_input::Question]| {
            assert_eq!(questions[0].id, " q ");
            ready_tx.send(()).unwrap();
            Ok(answer_rx.recv().unwrap())
        };
        let result = turn::run_tool_turn_with_host(
            &mut history,
            "执行",
            &mut sender,
            &mut |_| {},
            None,
            &mut |h| {
                if let Some(HistoryItem::ToolResult { tool_call_id, .. }) = h.last() {
                    record_tx.send(tool_call_id.clone()).unwrap();
                }
            },
            ToolMode::User,
            &ProcessManager::new(),
            Some(&mut host),
        );
        (result, history)
    });
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(matches!(
        record_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    answer_tx
        .send(Response {
            answers: [(
                " q ".into(),
                Answer {
                    answers: vec!["保留".into(), "user_note: 中文补充".into()],
                },
            )]
            .into(),
        })
        .unwrap();
    let (result, history) = worker.join().unwrap();
    assert_eq!(result.unwrap(), "继续完成");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(record_rx.into_iter().collect::<Vec<_>>(), ["ask", "after"]);
    assert_eq!(history.iter().filter(|h| matches!(h, HistoryItem::Text(m) if m.role == terminal_chat::prompt::HistoryRole::User)).count(), 1);
}

#[test]
fn absent_host_and_memory_merge_deny_forged_calls_in_execution_layer() {
    let temp = common::TempDir::new("018-deny-root");
    let root = temp.path().to_path_buf();
    for (mode, host_enabled, expected_tools) in [
        (ToolMode::User, false, 4),
        (
            ToolMode::MemoryMerge {
                root: &root,
                system: "internal",
                expected_epoch: 0,
            },
            true,
            3,
        ),
    ] {
        let mut history = Vec::new();
        let mut sent = 0;
        let mut asks = 0;
        let mut host = |_: &turn::ParsedToolCall, _: &[terminal_chat::user_input::Question]| {
            asks += 1;
            Ok(answered())
        };
        turn::run_tool_turn_with_host(
            &mut history,
            "go",
            &mut |body| {
                assert_eq!(body["tools"].as_array().unwrap().len(), expected_tools);
                assert!(!body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["function"]["name"] == "request_user_input"));
                sent += 1;
                Ok(if sent == 1 {
                    tool_response(vec![call("forged", "request_user_input", &question_args())])
                } else {
                    final_response()
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            mode,
            &ProcessManager::new(),
            if host_enabled { Some(&mut host) } else { None },
        )
        .unwrap();
        assert_eq!(asks, 0);
        let HistoryItem::ToolResult {
            content, display, ..
        } = &history[2]
        else {
            panic!("缺工具结果")
        };
        assert!(serde_json::from_str::<serde_json::Value>(content).unwrap()["error"].is_string());
        assert!(display.is_none());
    }
}

#[test]
fn unfinished_question_pairs_remaining_calls_and_never_requests_again() {
    use terminal_chat::user_input::InputError;
    for reason in [
        InputError::Cancelled,
        InputError::HostExited,
        InputError::Closed,
    ] {
        let mut history = Vec::new();
        let mut sent = 0;
        let mut asks = 0;
        let mut host = |_: &turn::ParsedToolCall, _: &[terminal_chat::user_input::Question]| {
            asks += 1;
            Err(reason.clone())
        };
        let error = turn::run_tool_turn_with_host(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                assert_eq!(sent, 1, "终态后不得再请求");
                Ok(tool_response(vec![
                    call("current", "request_user_input", &question_args()),
                    call("next", "request_user_input", &question_args()),
                ]))
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            Some(&mut host),
        )
        .unwrap_err();
        assert_eq!(error, reason.message());
        assert_eq!(asks, 1);
        let results: Vec<_> = history
            .iter()
            .filter_map(|h| {
                if let HistoryItem::ToolResult {
                    tool_call_id,
                    content,
                    display,
                } = h
                {
                    assert!(display.is_none());
                    Some((
                        tool_call_id,
                        serde_json::from_str::<serde_json::Value>(content).unwrap(),
                    ))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "current");
        assert_eq!(results[1].0, "next");
        assert_eq!(results[0].1, json!({"error":reason.message()}));
        let next_error = results[1].1["error"].as_str().unwrap();
        assert!(next_error.contains("本调用未执行"));
        if reason != InputError::Cancelled {
            assert!(next_error.contains(reason.message()));
        }
    }
}

#[test]
fn channel_cancel_disconnect_and_late_answers_do_not_cross_requests() {
    use std::sync::atomic::AtomicBool;
    use terminal_chat::user_input::{wait_for_response, InputError};
    let (reply, rx) = mpsc::channel();
    let (ready, ready_rx) = mpsc::channel();
    let (done, done_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let live = Arc::new(AtomicBool::new(true));
    let c = cancel.clone();
    let l = live.clone();
    let worker = std::thread::spawn(move || {
        ready.send(()).unwrap();
        let result = wait_for_response(rx, &c, &l);
        done.send(result.clone()).unwrap();
        result
    });
    ready_rx.recv().unwrap();
    cancel.store(true, Ordering::SeqCst);
    if done_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .is_err()
    {
        drop(reply);
        let _ = worker.join();
        panic!("取消未解除问询等待");
    }
    assert_eq!(worker.join().unwrap(), Err(InputError::Cancelled));
    assert!(!live.load(Ordering::SeqCst));
    assert!(
        reply.send(Ok(answered())).is_err(),
        "旧请求的迟到答案必须失效"
    );
    let (new_reply, new_rx) = mpsc::channel();
    new_reply.send(Ok(answered())).unwrap();
    assert_eq!(
        wait_for_response(new_rx, &AtomicBool::new(false), &AtomicBool::new(true)).unwrap(),
        answered()
    );
    let (closed, rx) = mpsc::channel();
    drop(closed);
    assert_eq!(
        wait_for_response(rx, &AtomicBool::new(false), &AtomicBool::new(true)),
        Err(InputError::Closed)
    );
}

#[test]
fn eighth_question_result_saved_and_oversized_batch_never_opened() {
    let mut history = Vec::new();
    let mut sent = 0;
    let mut asks = 0;
    let mut host = |_: &turn::ParsedToolCall, _: &[terminal_chat::user_input::Question]| {
        asks += 1;
        Ok(answered())
    };
    let error = turn::run_tool_turn_with_host(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(tool_response(vec![if sent == 8 {
                call("eighth", "request_user_input", &question_args())
            } else {
                call(&format!("plan{sent}"), "update_plan", "{\"plan\":[]}")
            }]))
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut host),
    )
    .unwrap_err();
    assert!(error.contains("上限（8）"));
    assert_eq!(sent, 8);
    assert_eq!(asks, 1);
    assert!(
        matches!(history.last(),Some(HistoryItem::ToolResult {tool_call_id,content,display:None}) if tool_call_id=="eighth" && content.contains("answers"))
    );
    let mut history = Vec::new();
    let mut asks = 0;
    let error = turn::run_tool_turn_with_host(
        &mut history,
        "go",
        &mut |_| {
            Ok(tool_response(
                (0..33)
                    .map(|i| call(&format!("q{i}"), "request_user_input", &question_args()))
                    .collect(),
            ))
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut |_, _| {
            asks += 1;
            Ok(answered())
        }),
    )
    .unwrap_err();
    assert!(error.contains("32"));
    assert_eq!(asks, 0);
    assert_eq!(history.len(), 1);
}

#[test]
fn accepted_answer_stays_saved_but_cancel_prevents_followup_request() {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut history = Vec::new();
    let mut sent = 0;
    let result = turn::run_tool_turn_with_host(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            assert_eq!(sent, 1, "取消后不能再请求模型");
            Ok(tool_response(vec![call(
                "ask",
                "request_user_input",
                &question_args(),
            )]))
        },
        &mut |_| {},
        Some(&cancel),
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut |_, _| {
            cancel.store(true, Ordering::SeqCst);
            Ok(answered())
        }),
    );
    assert!(result
        .unwrap_err()
        .starts_with(turn::CANCELLED_TURN_ERROR_PREFIX));
    let Some(HistoryItem::ToolResult { content, .. }) = history.last() else {
        panic!("已受理回答丢失")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(content).unwrap(),
        json!({"answers":{"q":{"answers":["A"]}}})
    );
}

#[test]
fn memory_summary_user_turn_has_five_tools_only_with_answer_host() {
    let root = common::TempDir::new("018-summary");
    std::fs::write(root.path().join("memory_summary.md"), "018-summary-content").unwrap();
    let config = terminal_chat::memory::MemoriesConfig {
        enabled: true,
        use_memories: true,
        ..Default::default()
    };
    for has_host in [false, true] {
        let mut history = Vec::new();
        let mut sent = 0;
        let mut asks = 0;
        let mut host = |_: &turn::ParsedToolCall, _: &[terminal_chat::user_input::Question]| {
            asks += 1;
            Ok(answered())
        };
        turn::run_tool_turn_with_host(
            &mut history,
            "go",
            &mut |body| {
                assert!(body["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("018-summary-content"));
                assert_eq!(
                    body["tools"].as_array().unwrap().len(),
                    if has_host { 5 } else { 4 }
                );
                sent += 1;
                Ok(if has_host && sent == 1 {
                    tool_response(vec![call("ask", "request_user_input", &question_args())])
                } else {
                    final_response()
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::UserWithMemory {
                root: root.path(),
                config: &config,
            },
            &ProcessManager::new(),
            if has_host { Some(&mut host) } else { None },
        )
        .unwrap();
        assert_eq!(asks, usize::from(has_host));
    }
}

#[test]
fn invalid_arguments_never_open_host_and_unknown_answer_ids_end_turn() {
    for raw in [
        "{}",
        "not json",
        "{\"questions\":[]}",
        "{\"questions\":[],\"isBlocking\":true}",
    ] {
        let mut history = Vec::new();
        let mut sent = 0;
        let mut asks = 0;
        turn::run_tool_turn_with_host(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                Ok(if sent == 1 {
                    tool_response(vec![call("bad", "request_user_input", raw)])
                } else {
                    final_response()
                })
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &ProcessManager::new(),
            Some(&mut |_, _| {
                asks += 1;
                Ok(answered())
            }),
        )
        .unwrap();
        assert_eq!(asks, 0);
        assert!(
            matches!(&history[2],HistoryItem::ToolResult {content,display:None,..} if serde_json::from_str::<serde_json::Value>(content).unwrap()["error"].is_string())
        );
    }
    let mut history = Vec::new();
    let mut sent = 0;
    let error = turn::run_tool_turn_with_host(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(tool_response(vec![call(
                "bad-reply",
                "request_user_input",
                &question_args(),
            )]))
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut |_, _| {
            Ok(Response {
                answers: [(
                    "old-question".into(),
                    Answer {
                        answers: vec!["wrong".into()],
                    },
                )]
                .into(),
            })
        }),
    )
    .unwrap_err();
    assert!(error.contains("回答无效"));
    assert_eq!(sent, 1);
    assert!(
        matches!(history.last(),Some(HistoryItem::ToolResult {content,..}) if !content.contains("old-question"))
    );
}

#[test]
fn thirty_two_question_calls_execute_serially_and_keep_all_results() {
    let mut history = Vec::new();
    let mut sent = 0;
    let mut asks = 0;
    turn::run_tool_turn_with_host(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(if sent == 1 {
                tool_response(
                    (0..32)
                        .map(|i| call(&format!("q{i}"), "request_user_input", &question_args()))
                        .collect(),
                )
            } else {
                final_response()
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut |call, _| {
            assert_eq!(call.id, format!("q{asks}"));
            asks += 1;
            Ok(answered())
        }),
    )
    .unwrap();
    assert_eq!(asks, 32);
    assert_eq!(sent, 2);
    assert_eq!(
        history
            .iter()
            .filter(|h| matches!(h, HistoryItem::ToolResult { .. }))
            .count(),
        32
    );
}

#[test]
fn already_delivered_terminal_reason_is_not_replaced_by_cancel_flag() {
    use std::sync::atomic::AtomicBool;
    use terminal_chat::user_input::{wait_for_response, InputError};
    for result in [
        Ok(answered()),
        Err(InputError::HostExited),
        Err(InputError::Closed),
    ] {
        let (reply, rx) = mpsc::channel();
        reply.send(result.clone()).unwrap();
        assert_eq!(
            wait_for_response(rx, &AtomicBool::new(true), &AtomicBool::new(true)),
            result
        );
    }
}

#[test]
fn unfinished_question_keeps_each_skipped_tools_existing_result_shape() {
    use terminal_chat::user_input::InputError;
    let mut history = Vec::new();
    let mut sent = 0;
    let result = turn::run_tool_turn_with_host(
        &mut history,
        "go",
        &mut |_| {
            sent += 1;
            Ok(tool_response(vec![
                call("ask", "request_user_input", &question_args()),
                call("exec", "exec_command", "{}"),
                call("patch", "apply_patch", "{}"),
                call("plan", "update_plan", "{}"),
            ]))
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &ProcessManager::new(),
        Some(&mut |_, _| Err(InputError::HostExited)),
    );
    assert!(result.is_err());
    assert_eq!(sent, 1);
    let results: Vec<serde_json::Value> = history
        .iter()
        .filter_map(|h| {
            if let HistoryItem::ToolResult { content, .. } = h {
                Some(serde_json::from_str(content).unwrap())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results[1]["status"], "error");
    assert_eq!(results[1]["session_id"], serde_json::Value::Null);
    assert_eq!(results[2]["ok"], false);
    assert_eq!(results[2]["state_known"], true);
    assert_eq!(results[2]["changed_files"], json!([]));
    assert_eq!(results[3]["ok"], false);
    for result in &results[1..] {
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("宿主退出：问询未完成；本调用未执行"));
    }
}
