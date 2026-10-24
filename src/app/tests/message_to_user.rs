//! 021 send_message_to_user_async 验收（L1 组件＋L2 工具循环／接续共存）：
//! 参数解析与 accepted 结果、能力开关双层隔离、消息不结束轮、无额外同文
//! assistant 注入、运行中接续入史边界、终文竞态原子判定、取消/额度/准备失败
//! 的交还路径。全部使用真实循环与固定响应，无真实网络与用户环境。

mod common;

use common::TempDir;
use serde_json::{json, Value};
use std::sync::mpsc as std_mpsc;
use terminal_chat::message_to_user;
use terminal_chat::proc_session::ProcessManager;
use terminal_chat::prompt::HistoryItem;
use terminal_chat::turn::{self, SteerInput, SteerSeal, ToolMode, TurnExtras, TurnNotify};

// ---------- L1：参数解析与结果形状 ----------

#[test]
fn parse_arguments_trims_and_preserves_inner_text() {
    let parsed = message_to_user::parse_arguments(r#"{"message":"  发现配置不一致，\n继续核对。  "}"#)
        .expect("合法参数");
    assert_eq!(parsed, "发现配置不一致，\n继续核对。", "首尾空白删除，内部换行与原文保留");
    assert_eq!(
        message_to_user::parse_arguments(r#"{"message":"ok"}"#).unwrap(),
        "ok"
    );
}

#[test]
fn parse_arguments_rejects_invalid_shapes() {
    for raw in [
        r#"{"message":"   "}"#,
        r#"{"message":""}"#,
        r#"{}"#,
        r#"{"message":null}"#,
        r#"{"message":5}"#,
        r#"{"message":"x","extra":1}"#,
        r#"not-json{"#,
    ] {
        assert!(
            message_to_user::parse_arguments(raw).is_err(),
            "应拒绝：{raw}"
        );
    }
    // 空白拒绝使用上游固定文案
    assert_eq!(
        message_to_user::parse_arguments(r#"{"message":"  "}"#).unwrap_err(),
        "message must not be empty"
    );
}

#[test]
fn accepted_result_is_exact_and_record_conversion_paths() {
    assert_eq!(message_to_user::accepted_result(), r#"{"accepted":true}"#);
    // 成功：调用＋accepted 结果 → 重建消息（trim 后）
    let message = message_to_user::sent_message_from_record(
        r#"{"message":"  已完成核对。  "}"#,
        r#"{"accepted":true}"#,
    )
    .expect("成功记录应重建消息");
    assert_eq!(message, "已完成核对。");
    // 取消、失败、缺失、损坏结果不能推定发送成功
    for result in [
        r#"{"error":"用户取消，本调用未执行"}"#,
        r#"{"error":"message must not be empty"}"#,
        r#"{"accepted":false}"#,
        r#"{}"#,
        r#"not-json"#,
    ] {
        assert!(
            message_to_user::sent_message_from_record(r#"{"message":"m"}"#, result).is_none(),
            "{result} 不得推定发送成功"
        );
    }
    // accepted=true 即成功（多余字段不参与判定，与上游 accepted 精确布尔一致）
    assert_eq!(
        message_to_user::sent_message_from_record(r#"{"message":"m"}"#, r#"{"accepted":true,"x":1}"#)
            .expect("accepted=true 即成功"),
        "m"
    );
    // 结果成功但参数损坏：不推定成功
    assert!(message_to_user::sent_message_from_record(r#"corrupt"#, r#"{"accepted":true}"#).is_none());
}

#[test]
fn host_failure_line_variants() {
    assert_eq!(
        message_to_user::failure_line(r#"{"error":"用户取消，本调用未执行"}"#),
        "[工具] send_message_to_user_async 已取消：用户取消，本调用未执行"
    );
    assert_eq!(
        message_to_user::failure_line(r#"{"error":"message must not be empty"}"#),
        "[工具] send_message_to_user_async 失败：message must not be empty"
    );
    assert_eq!(
        message_to_user::failure_line(r#"not-json"#),
        "[工具] send_message_to_user_async 结果未知"
    );
}

// ---------- 测试替身：接续通道（每轮独立；与宿主实现同构语义） ----------

struct ChannelMock {
    tx: std_mpsc::Sender<SteerInput>,
    rx: std_mpsc::Receiver<SteerInput>,
    close_calls: std::rc::Rc<std::cell::Cell<usize>>,
}

impl ChannelMock {
    fn new() -> Self {
        let (tx, rx) = std_mpsc::channel();
        Self { tx, rx, close_calls: std::rc::Rc::new(std::cell::Cell::new(0)) }
    }
}

impl turn::SteerChannel for ChannelMock {
    fn drain(&mut self) -> Vec<SteerInput> {
        let mut out = Vec::new();
        while let Ok(input) = self.rx.try_recv() {
            out.push(input);
        }
        out
    }
    fn seal(&mut self) -> SteerSeal {
        let inputs = self.drain();
        if inputs.is_empty() {
            SteerSeal::Closed
        } else {
            SteerSeal::Inputs(inputs)
        }
    }
    fn close(&mut self) -> Vec<SteerInput> {
        self.close_calls.set(self.close_calls.get() + 1);
        self.drain()
    }
}

fn steer_input(id: u64, text: &str) -> SteerInput {
    SteerInput {
        id,
        text: text.to_string(),
        image_paths: Vec::new(),
        text_elements: Vec::new(),
    }
}

// ---------- 循环驱动 ----------

fn fin(content: &str) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

fn tc_response(calls: &[Value]) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": null, "tool_calls": calls }, "finish_reason": "tool_calls" }] })
        .to_string()
}

fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({ "id": id, "type": "function", "function": { "name": name, "arguments": arguments.to_string() } })
}

fn send_call(id: &str, message: &str) -> Value {
    call(id, "send_message_to_user_async", json!({ "message": message }))
}

struct TurnRun {
    reply: Result<String, String>,
    history: Vec<HistoryItem>,
    bodies: Vec<Value>,
    events: Vec<TurnNotify>,
    close_calls: std::rc::Rc<std::cell::Cell<usize>>,
}

/// 运行一个 send_message 能力可配置的用户轮；`presteer` 预载接续输入
///（受理先于批末接入——与工具执行期间提交经通道解耦后语义一致）；
/// `on_notify` 供用例在事件点注入副作用（确定性，不依赖真实时间）。
fn run_turn_with(
    responses: Vec<String>,
    send_message: bool,
    presteer: Vec<SteerInput>,
    mut on_notify: impl FnMut(&TurnNotify, &std_mpsc::Sender<SteerInput>),
) -> TurnRun {
    let mut channel = ChannelMock::new();
    for input in presteer {
        channel.tx.send(input).unwrap();
    }
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut iter = responses.into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        let resp = iter.next().expect("预设响应耗尽");
        Ok(serde_json::from_str(&resp).unwrap())
    };
    let submit_tx = channel.tx.clone();
    let mut notify = |n: TurnNotify| {
        on_notify(&n, &submit_tx);
        events.push(n);
    };
    let extras = TurnExtras { send_message, steer: Some(&mut channel) };
    let reply = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "执行任务",
        &mut sender,
        &mut notify,
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        extras,
    );
    TurnRun { reply, history, bodies, events, close_calls: channel.close_calls }
}

fn user_texts(history: &[HistoryItem]) -> Vec<String> {
    history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m)
                if m.role == terminal_chat::prompt::HistoryRole::User
                    && terminal_chat::context::context_body(&m.content).is_none() =>
            {
                Some(m.content.clone())
            }
            _ => None,
        })
        .collect()
}

fn assistant_texts(history: &[HistoryItem]) -> Vec<String> {
    history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m)
                if m.role == terminal_chat::prompt::HistoryRole::Assistant =>
            {
                Some(m.content.clone())
            }
            _ => None,
        })
        .collect()
}

// ---------- L2：真实循环——消息语义 ----------

/// A04 核心：消息出现后任务继续——下一请求含调用与 accepted、无同文 assistant、
/// 最终回复才结束；用户全程不回答。
#[test]
fn real_loop_message_does_not_end_turn_and_no_extra_assistant_injection() {
    let run = run_turn_with(
        vec![
            tc_response(&[send_call("m1", "  发现现有配置与规格不一致，先核对再改。  ")]),
            fin("已完成"),
        ],
        true,
        Vec::new(),
        |_, _| {},
    );
    assert_eq!(run.reply.unwrap(), "已完成", "通知不结束轮，最终回复才结束");
    assert_eq!(run.bodies.len(), 2, "恰两次模型请求");
    // ToolEnd 通知携带 accepted 结果（宿主据此投影消息）
    let end = run
        .events
        .iter()
        .find(|e| matches!(e, TurnNotify::ToolEnd { name, .. } if name == "send_message_to_user_async"))
        .expect("必有 send_message ToolEnd");
    match end {
        TurnNotify::ToolEnd { id, result, cancelled, .. } => {
            assert_eq!(id, "m1");
            assert_eq!(result, r#"{"accepted":true}"#);
            assert!(!cancelled);
        }
        other => panic!("{other:?}"),
    }
    // 历史：调用与结果配对；无额外同文 assistant 消息
    assert!(
        !assistant_texts(&run.history)
            .iter()
            .any(|t| t.contains("发现现有配置与规格不一致")),
        "不得注入同文 assistant 消息：{:?}",
        assistant_texts(&run.history)
    );
    let results: Vec<&str> = run
        .history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "m1" => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![r#"{"accepted":true}"#],
        "恰一条 accepted 结果"
    );
    // 第二请求：工具调用＋tool 结果原样；无同文 assistant；用户输入恰一次
    let msgs = run.bodies[1]["messages"].as_array().unwrap();
    let has_accepted_tool = msgs.iter().any(|m| {
        m["role"] == json!("tool")
            && m["tool_call_id"] == json!("m1")
            && m["content"] == json!(r#"{"accepted":true}"#)
    });
    assert!(has_accepted_tool, "下一请求含调用结果：{msgs:?}");
    let injected = msgs.iter().any(|m| {
        m["role"] == json!("assistant")
            && m.get("tool_calls").is_none()
            && m["content"].as_str().is_some_and(|c| c.contains("发现现有配置"))
    });
    assert!(!injected, "请求面不得额外注入同文 assistant：{msgs:?}");
    assert_eq!(user_texts(&run.history).len(), 1, "用户输入恰一次");
}

/// A03：空白消息失败——错误结果回传模型、无成功消息投影、任务继续。
#[test]
fn real_loop_blank_message_fails_and_turn_continues() {
    let run = run_turn_with(
        vec![tc_response(&[send_call("m-blank", "   ")]), fin("已跳过消息")],
        true,
        Vec::new(),
        |_, _| {},
    );
    assert_eq!(run.reply.unwrap(), "已跳过消息");
    let end = run
        .events
        .iter()
        .find(|e| matches!(e, TurnNotify::ToolEnd { name, .. } if name == "send_message_to_user_async"))
        .expect("失败也有 ToolEnd 终态");
    match end {
        TurnNotify::ToolEnd { result, .. } => {
            assert_eq!(result, r#"{"error":"message must not be empty"}"#);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(assistant_texts(&run.history).len(), 1, "失败无成功消息投影，仅最终回复");
    let results: Vec<&str> = run
        .history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "m-blank" => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(results, vec![r#"{"error":"message must not be empty"}"#]);
}

/// A01/A02：关闭态（未声明）伪造调用——执行层拒绝，无成功结果。
#[test]
fn closed_state_forged_call_is_rejected() {
    let run = run_turn_with(
        vec![tc_response(&[send_call("m-off", "偷跑")]), fin("收到拒绝")],
        false,
        Vec::new(),
        |_, _| {},
    );
    assert_eq!(run.reply.unwrap(), "收到拒绝");
    let results: Vec<Value> = run
        .history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "m-off" => {
                Some(serde_json::from_str(content).unwrap())
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["error"].as_str().unwrap().contains("send_message_to_user_async"),
        "关闭态拒绝并说明：{results:?}"
    );
    assert!(results[0].get("accepted").is_none(), "关闭态不得回 accepted");
}

/// A02：内部 MemoryMerge 不声明也不执行——按未知工具拒绝（真实合并轮）。
#[test]
fn memory_merge_never_executes_message_tool() {
    let mut history: Vec<HistoryItem> = Vec::new();
    let mut first = true;
    let err = turn::run_tool_turn_with_mode(
        &mut history,
        "合并",
        &mut |_| {
            let resp = if first {
                first = false;
                tc_response(&[send_call("mm1", "越权")])
            } else {
                fin("merge-done")
            };
            Ok(serde_json::from_str(&resp).unwrap())
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::MemoryMerge {
            root: std::path::Path::new("/nonexistent-021"),
            system: "merge",
            expected_epoch: 0,
        },
        &ProcessManager::new(),
    );
    let _ = err; // 合并轮收尾形态不属本测试关注点
    let results: Vec<Value> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "mm1" => {
                Some(serde_json::from_str(content).unwrap())
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1, "越权调用有匹配结果：{history:?}");
    assert!(
        results[0]["error"].as_str().unwrap().contains("未知工具"),
        "合并轮分派层拒绝：{results:?}"
    );
    // 声明层：合并轮请求组装不含该工具
    let body = terminal_chat::prompt::build_turn_request_with(
        &[],
        &terminal_chat::memory::mem_tools::tool_definitions(),
        "merge",
    );
    assert!(!body["tools"].as_array().unwrap().iter().any(|t| {
        t["function"]["name"] == json!("send_message_to_user_async")
    }), "合并轮不声明消息工具");
}

/// 取消批内未执行的消息调用：单 error 通用结构（不伪装 exec 结果）。
#[test]
fn cancelled_message_call_keeps_generic_error_shape() {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let cancel_ref = &cancel;
    let run = {
        let mut channel = ChannelMock::new();
        let mut history: Vec<HistoryItem> = Vec::new();
        let mgr = ProcessManager::new();
        let sources = terminal_chat::context::ContextSources::from_env();
        let mut notify = |n: TurnNotify| {
            if matches!(n, TurnNotify::ToolBegin { .. }) {
                cancel_ref.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            let _ = n;
        };
        let extras = TurnExtras { send_message: true, steer: Some(&mut channel) };
        let reply = turn::run_user_turn_with_context_and_caps(
            &mut history,
            "开始",
            &mut |_| {
                Ok(serde_json::from_str(&tc_response(&[
                    call("e1", "exec_command", json!({ "cmd": "true" })),
                    send_call("m-skip", "不会发送"),
                ]))
                .unwrap())
            },
            &mut notify,
            Some(&cancel),
            &mut |_| {},
            ToolMode::User,
            &sources,
            &mgr,
            None,
            None,
            false,
            None,
            extras,
        );
        (reply, history)
    };
    assert!(run.0.is_err(), "取消＝Err 结束");
    let results: Vec<&str> = run
        .1
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "m-skip" => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    let v: Value = serde_json::from_str(results[0]).unwrap();
    assert_eq!(
        v,
        json!({ "error": "用户取消，本调用未执行" }),
        "未执行消息调用＝单 error 通用结构：{v}"
    );
}

// ---------- L2：运行中接续（§2.3） ----------

/// A10：工具执行期间受理的追问在完整批配对之后、下一请求之前入史；
/// 不重新执行原工具，不把新文字发给在途请求。
#[test]
fn steer_enters_history_after_full_batch_before_next_request() {
    let mut channel = ChannelMock::new();
    channel.tx.send(steer_input(1, "顺便确认下日志路径")).unwrap();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut responses = vec![
        tc_response(&[
            call("e1", "exec_command", json!({ "cmd": "true" })),
            call("e2", "exec_command", json!({ "cmd": "true" })),
        ]),
        fin("已确认"),
    ]
    .into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    let reply = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "跑任务",
        &mut sender,
        &mut |n| events.push(n),
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        extras,
    )
    .unwrap();
    assert_eq!(reply, "已确认");
    assert_eq!(bodies.len(), 2);
    // 第二请求消息序：… assistant(tc) → tool(result) → user(追问)；追问在工具结果之后
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let tool_pos = msgs
        .iter()
        .position(|m| m["role"] == json!("tool") && m["tool_call_id"] == json!("e1"))
        .expect("工具结果在请求中");
    // 多调用批：两条结果都先于追问（不能在批中间插 user）
    let tool2_pos = msgs
        .iter()
        .position(|m| m["role"] == json!("tool") && m["tool_call_id"] == json!("e2"))
        .expect("第二条结果在请求中");
    let steer_pos = msgs
        .iter()
        .position(|m| m["role"] == json!("user") && m["content"] == json!("顺便确认下日志路径"))
        .expect("追问入请求");
    assert!(
        tool_pos < tool2_pos && tool2_pos < steer_pos,
        "追问在完整批（含第二条结果）之后：{msgs:?}"
    );
    // 入史恰一次：历史 user 文本恰两条（原输入＋追问）
    assert_eq!(
        user_texts(&history),
        vec!["跑任务".to_string(), "顺便确认下日志路径".to_string()],
        "追问按受理顺序恰一次入史"
    );
    // 宿主收到恰一次入史确认（id 关联）
    let ingested: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            TurnNotify::SteerIngested { input } => Some(input.id),
            _ => None,
        })
        .collect();
    assert_eq!(ingested, vec![1], "入史确认恰一次");
}

/// A11：终文竞态——模型返回纯文本时已有受理输入未处理，保留正文继续，
/// 不提前宣告完成；处理追问后才结束。
#[test]
fn final_text_race_continues_turn_with_kept_reply() {
    let mut channel = ChannelMock::new();
    channel.tx.send(steer_input(7, "等等，先改用方案 B")).unwrap();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut responses = vec![fin("第一段结论"), fin("方案 B 已处理完成")].into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    let extras = TurnExtras { send_message: true, steer: Some(&mut channel) };
    let reply = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "开始",
        &mut sender,
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        extras,
    )
    .unwrap();
    // 第一段不是最终交付；最终回复来自处理追问之后
    assert_eq!(reply, "方案 B 已处理完成");
    assert_eq!(bodies.len(), 2);
    // 第二请求：保留的第一段正文（assistant）＋追问（user），顺序固定
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let first_pos = msgs
        .iter()
        .position(|m| m["role"] == json!("assistant") && m["content"] == json!("第一段结论"))
        .expect("第一段正文保留入史");
    let steer_pos = msgs
        .iter()
        .position(|m| m["role"] == json!("user") && m["content"] == json!("等等，先改用方案 B"))
        .expect("追问入请求");
    assert!(first_pos < steer_pos, "正文在前、追问在后：{msgs:?}");
    assert!(
        msgs[steer_pos + 1..].iter().all(|m| m["role"] != json!("user")),
        "追问后无其他 user 插入"
    );
    // 历史保留两段 assistant 正文
    assert_eq!(
        assistant_texts(&history),
        vec!["第一段结论".to_string(), "方案 B 已处理完成".to_string()]
    );
}

/// A11：正常结束（无接续输入）在终文判定点原子封口恰一次；成功出口不经
/// close（seal 即封口），通道以 Closed 终态结束。
#[test]
fn turn_exit_seals_channel_exactly_once() {
    let seal_closed = std::rc::Rc::new(std::cell::Cell::new(0));
    let close_calls = std::rc::Rc::new(std::cell::Cell::new(0));
    struct Counting {
        seal_closed: std::rc::Rc<std::cell::Cell<usize>>,
        close_calls: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl turn::SteerChannel for Counting {
        fn drain(&mut self) -> Vec<SteerInput> {
            Vec::new()
        }
        fn seal(&mut self) -> SteerSeal {
            self.seal_closed.set(self.seal_closed.get() + 1);
            SteerSeal::Closed
        }
        fn close(&mut self) -> Vec<SteerInput> {
            self.close_calls.set(self.close_calls.get() + 1);
            Vec::new()
        }
    }
    let mut channel = Counting {
        seal_closed: seal_closed.clone(),
        close_calls: close_calls.clone(),
    };
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    turn::run_user_turn_with_context_and_caps(
        &mut history,
        "收尾",
        &mut |_| Ok(serde_json::from_str(&fin("done")).unwrap()),
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        extras,
    )
    .unwrap();
    assert_eq!(seal_closed.get(), 1, "终文判定使用 seal 原子封口恰一次");
    assert_eq!(close_calls.get(), 0, "成功出口由 seal 封口，不重复 close");
}

/// A07/§2.3-6：取消时未入史的已受理输入交还宿主恢复（不入史、不丢弃）。
#[test]
fn cancel_restores_uningested_steer() {
    let dir = TempDir::new("m21-cancel");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut channel = ChannelMock::new();
    channel.tx.send(steer_input(3, "取消前追问")).unwrap();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut events: Vec<TurnNotify> = Vec::new();
    let probe = dir.path().join("after-cancel.txt");
    let mut responses = vec![tc_response(&[
        call("e1", "exec_command", json!({ "cmd": "true" })),
        call("e2", "exec_command", json!({ "cmd": format!("echo x > {}", probe.display()) })),
    ])]
    .into_iter();
    let mut sender = move |_: &Value| -> Result<Value, String> {
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    // 第一条工具开始即置取消（确定性）
    let cancel_ref = &cancel;
    let mut notify = |n: TurnNotify| {
        if matches!(n, TurnNotify::ToolBegin { .. }) {
            cancel_ref.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        events.push(n);
    };
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    let err = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "会被取消",
        &mut sender,
        &mut notify,
        Some(&cancel),
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        extras,
    )
    .unwrap_err();
    assert!(err.contains("用户已取消"), "{err}");
    assert!(!probe.exists(), "取消后剩余调用未执行");
    // 输入未入史，经 SteerRestored 完整交还
    assert!(!user_texts(&history).iter().any(|t| t == "取消前追问"), "取消路径不入史");
    let restored: Vec<&SteerInput> = events
        .iter()
        .filter_map(|e| match e {
            TurnNotify::SteerRestored { inputs, .. } => Some(inputs.iter().collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(restored.len(), 1, "恰一次交还：{events:?}");
    assert_eq!(restored[0].id, 3);
    assert_eq!(restored[0].text, "取消前追问");
}

/// A12：接续图文准备失败（坏图）——明确交还恢复，不静默只发文本；任务继续。
#[test]
fn steer_image_prepare_failure_restores_and_continues() {
    let bad = std::path::PathBuf::from("/nonexistent-021/bad.png");
    let mut channel = ChannelMock::new();
    channel
        .tx
        .send(SteerInput {
            id: 9,
            text: "看这张图".into(),
            image_paths: vec![bad],
            text_elements: Vec::new(),
        })
        .unwrap();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut responses = vec![fin("第一段正文"), fin("继续完成")].into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    let mut events: Vec<TurnNotify> = Vec::new();
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    let reply = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "开始",
        &mut sender,
        &mut |n| events.push(n),
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        /*image_input*/ true,
        None,
        extras,
    )
    .unwrap();
    assert_eq!(reply, "继续完成", "第一段正文保留后继续，第二段才是终文");
    let restored: Option<&String> = events.iter().find_map(|e| match e {
        TurnNotify::SteerRestored { reason, .. } => Some(reason),
        _ => None,
    });
    let reason = restored.expect("准备失败必须交还恢复");
    assert!(!reason.is_empty(), "恢复原因明确：{reason}");
    // 不入史、不降级为纯文本
    assert!(!user_texts(&history).iter().any(|t| t == "看这张图"));
    assert_eq!(user_texts(&history).len(), 1, "仅原输入入史");
    assert!(
        assistant_texts(&history).contains(&"第一段正文".to_string()),
        "终文竞态保留的正文不因准备失败丢失"
    );
}

/// A12（成功路径）：接续图文保留图片与标记——入史带 images，下一请求同一条
/// user 消息携带 image_url 部件；不退化为纯文本。
#[test]
fn steer_with_image_enters_history_with_payload() {
    let dir = TempDir::new("m21-steer-img");
    // 真实 PNG（image crate 构造，与 020 测试同源）
    let px: [u8; 4] = [9, 8, 7, 255];
    let img_buf = image::ImageBuffer::from_pixel(2, 2, image::Rgba(px));
    let mut cursor = std::io::Cursor::new(Vec::new());
    img_buf.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
    let img = dir.path().join("steer.png");
    std::fs::write(&img, cursor.into_inner()).unwrap();
    let mut channel = ChannelMock::new();
    channel
        .tx
        .send(SteerInput {
            id: 11,
            text: String::new(),
            image_paths: vec![img.clone()],
            text_elements: vec![codex_protocol::user_input::TextElement::new(
            codex_protocol::user_input::ByteRange { start: 0, end: 1 },
            Some("[Image #1]".into()),
        )],
        })
        .unwrap();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut responses = vec![fin("第一段"), fin("图收到")].into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    turn::run_user_turn_with_context_and_caps(
        &mut history,
        "开始",
        &mut sender,
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        /*image_input*/ true,
        None,
        extras,
    )
    .unwrap();
    // 入史：纯图接续消息（content 为空文本＋images＋text_elements）
    let msg = history
        .iter()
        .find_map(|h| match h {
            HistoryItem::Text(m) if m.images.is_some() => Some(m),
            _ => None,
        })
        .expect("接续图片消息入史");
    assert_eq!(msg.images.as_ref().unwrap().len(), 1);
    assert_eq!(msg.text_elements.len(), 1, "结构化标记随消息入史");
    // 请求：同一条 user 消息携带 image_url 部件（终文竞态保留正文后继续）
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let with_image = msgs
        .iter()
        .find(|m| m["content"].is_array() && m["content"].as_array().unwrap().iter().any(|p| p["type"] == json!("image_url")))
        .expect("请求携带图片部件");
    assert!(
        with_image["content"].as_array().unwrap().iter().any(|p| p["type"] == json!("image_url")),
        "接续图片进入请求：{with_image}"
    );
}

/// A09：消息工具开关与接续相互独立——开关关闭时接续仍是普通输入能力。
#[test]
fn steer_works_without_message_capability() {
    let mut channel = ChannelMock::new();
    channel.tx.send(steer_input(2, "与通知工具无关的追问")).unwrap();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut responses = vec![tc_response(&[send_call("s-off", "不该成功")]), fin("完成")].into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    turn::run_user_turn_with_context_and_caps(
        &mut history,
        "开始",
        &mut sender,
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        extras,
    )
    .unwrap();
    // 开关关闭：工具被拒；追问仍正常入史并进入请求
    assert!(
        bodies[1]["messages"].as_array().unwrap().iter().any(|m| {
            m["role"] == json!("user") && m["content"] == json!("与通知工具无关的追问")
        }),
        "开关关闭不接续能力不受影响：{:?}",
        bodies[1]
    );
}

/// §2.4/A13：结构化答案与普通接续同批到达——两者均在完整工具结果之后入史，
/// 答案在前（既有 018 路径），接续随后；恰一次投影。
#[test]
fn steer_and_async_answer_both_after_full_batch() {
    use terminal_chat::user_input_async::AsyncReplyChannel;
    let mut channel = ChannelMock::new();
    channel.tx.send(steer_input(5, "普通追问")).unwrap();
    // 异步问询答案预载入回执通道
    let (answer_tx, answer_rx) = std_mpsc::channel();
    let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let answer = json!({
        "answers": {
            "[\"request_user_input_async\",\"q1\",0]": { "answers": ["选项一"] }
        }
    });
    answer_tx.send(Ok(serde_json::from_value(answer).unwrap())).unwrap();
    let answer_rx_cell = std::cell::RefCell::new(Some(answer_rx));
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut responses = vec![
        tc_response(&[call(
            "q1",
            "request_user_input_async",
            json!({ "questions": [{ "title": "方向？", "options": ["选项一"] }] }),
        )]),
        fin("两者都已看到"),
    ]
    .into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    let mut async_host = |_: &turn::ParsedToolCall, _: &[terminal_chat::user_input::Question]| {
        Ok(AsyncReplyChannel {
            receiver: answer_rx_cell.borrow_mut().take().expect("回执通道只取一次"),
            live: live.clone(),
        })
    };
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    turn::run_user_turn_with_context_and_caps(
        &mut history,
        "开始",
        &mut sender,
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        Some(&mut async_host),
        false,
        None,
        extras,
    )
    .unwrap();
    // 第二请求消息序：tool(accepted) → user(答案事件) → user(普通追问)
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let accepted_pos = msgs
        .iter()
        .position(|m| m["role"] == json!("tool") && m["tool_call_id"] == json!("q1"))
        .expect("发布结果在请求中");
    let answer_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == json!("user")
                && m["content"].as_str().is_some_and(|c| c.contains("request_id"))
        })
        .expect("答案事件入请求");
    let steer_pos = msgs
        .iter()
        .position(|m| m["role"] == json!("user") && m["content"] == json!("普通追问"))
        .expect("追问入请求");
    assert!(
        accepted_pos < answer_pos && answer_pos < steer_pos,
        "顺序：答案在前、追问随后：{msgs:?}"
    );
    // 历史各恰一次
    let users = user_texts(&history);
    assert_eq!(users.iter().filter(|t| t.as_str() == "普通追问").count(), 1);
}

/// 接续不重置请求预算：8 次请求后报上限；未入史输入交还恢复（不发第 9 次）。
/// 输入在第 8 批执行期间受理（预载会在更早批末被合法消化）。
#[test]
fn steer_does_not_reset_request_budget() {
    let mut channel = ChannelMock::new();
    let submit_tx = channel.tx.clone();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut sent = 0usize;
    let mut begins = 0usize;
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut notify = |n: TurnNotify| {
        if matches!(n, TurnNotify::ToolBegin { .. }) {
            begins += 1;
            if begins == 8 {
                // 第 8 批执行期间受理（确定性，不依赖真实时间）
                submit_tx.send(steer_input(4, "预算内追问")).unwrap();
            }
        }
        events.push(n);
    };
    let extras = TurnExtras { send_message: false, steer: Some(&mut channel) };
    let err = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "开始",
        &mut |_| {
            sent += 1;
            Ok(serde_json::from_str(&tc_response(&[call(
                &format!("c{sent}"),
                "exec_command",
                json!({ "cmd": "true" }),
            )]))
            .unwrap())
        },
        &mut notify,
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        extras,
    )
    .unwrap_err();
    assert_eq!(sent, 8, "接续不重置额度");
    assert!(err.contains("已达本轮模型请求上限（8）"), "{err}");
    assert!(!user_texts(&history).iter().any(|t| t == "预算内追问"), "额度路径未入史");
    assert!(
        events.iter().any(|e| matches!(e,
            TurnNotify::SteerRestored { inputs, .. } if inputs.iter().any(|i| i.id == 4))),
        "额度路径交还恢复：{events:?}"
    );
}

/// A05：多条消息按调用顺序各投影一次；同文不同 ID 分别保留（历史层面）。
#[test]
fn multiple_message_calls_kept_separately_in_order() {
    let run = run_turn_with(
        vec![
            tc_response(&[send_call("a1", "第一条"), send_call("a2", "第一条")]),
            fin("两条都已发出"),
        ],
        true,
        Vec::new(),
        |_, _| {},
    );
    assert_eq!(run.reply.unwrap(), "两条都已发出");
    let ends: Vec<&TurnNotify> = run
        .events
        .iter()
        .filter(|e| {
            matches!(e, TurnNotify::ToolEnd { name, .. } if name == "send_message_to_user_async")
        })
        .collect();
    assert_eq!(ends.len(), 2, "每条调用各一次 ToolEnd（投影各一条）");
    let ids: Vec<&String> = ends
        .iter()
        .map(|e| match e {
            TurnNotify::ToolEnd { id, .. } => id,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(ids, vec![&"a1".to_string(), &"a2".to_string()], "按调用顺序");
    // 结果各恰一条且同文（历史不按正文合并）
    let results = run
        .history
        .iter()
        .filter(|h| matches!(h, HistoryItem::ToolResult { content, .. } if content == r#"{"accepted":true}"#))
        .count();
    assert_eq!(results, 2);
}

// ---------- 安全门禁（输入来源／终端过滤／工具隔离） ----------

/// A14/安全门禁（L2）：模型可控消息——调用参数原文保留进历史（模型面不变，
/// 过滤只发生在展示副本层）；成功判定不受控制字符影响；关闭态伪造调用
/// （含控制字符参数）仍被双层隔离拒绝。
#[test]
fn security_raw_arguments_kept_for_model_and_isolation_holds() {
    let evil = "\u{1b}[2J清除\u{7}屏\rPWNED";
    let run = run_turn_with(
        vec![
            tc_response(&[send_call("s-evil", evil)]),
            fin("已展示"),
        ],
        true,
        Vec::new(),
        |_, _| {},
    );
    assert_eq!(run.reply.unwrap(), "已展示");
    // 模型面：调用参数原文（含全部控制字符）原样入史、下一请求原样投影
    let HistoryItem::AssistantToolCalls { tool_calls, .. } = &run.history[2] else {
        panic!("历史应含 assistant 调用记录：{:?}", run.history)
    };
    assert_eq!(tool_calls[0].arguments, json!({ "message": evil }).to_string());
    let msgs = run.bodies[1]["messages"].as_array().unwrap();
    // 006 契约：arguments 字符串原样保留（模型侧 JSON 转义形态 \u001b 逐字保留，
    // 不过滤、不重写）——过滤只发生在展示副本层
    assert!(
        msgs.iter().any(|m| {
            m["role"] == json!("assistant") && m.get("tool_calls").is_some()
                && m["tool_calls"][0]["function"]["arguments"].as_str().is_some_and(|a| a.contains("\\u001b[2J"))
        }),
        "下一请求保留参数原文（过滤只在展示层）：{msgs:?}"
    );
    // 结果判定不受控制字符影响：仍恰一条 accepted
    assert!(run.history.iter().any(|h| matches!(
        h,
        HistoryItem::ToolResult { tool_call_id, content, .. }
            if tool_call_id == "s-evil" && content == r#"{"accepted":true}"#
    )));
}

/// A05（019 共存）：同批 view_image＋send_message——按调用顺序执行与配对，
/// 各自投影（图片批末附件＋消息通知互不干扰），无额外注入。
#[test]
fn mixed_batch_view_image_and_message_kept_in_order() {
    let dir = TempDir::new("m21-mixed");
    let px: [u8; 4] = [1, 2, 3, 255];
    let img_buf = image::ImageBuffer::from_pixel(2, 2, image::Rgba(px));
    let mut cursor = std::io::Cursor::new(Vec::new());
    img_buf.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
    let img = dir.path().join("m.png");
    std::fs::write(&img, cursor.into_inner()).unwrap();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = terminal_chat::context::ContextSources::from_env();
    let mut bodies: Vec<Value> = Vec::new();
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut responses = vec![
        tc_response(&[
            call("v1", "view_image", json!({ "path": img.display().to_string() })),
            send_call("m1", "图片已查看，继续"),
        ]),
        fin("混合批完成"),
    ]
    .into_iter();
    let mut sender = |body: &Value| -> Result<Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().unwrap()).unwrap())
    };
    let extras = TurnExtras { send_message: true, steer: None };
    let reply = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "看图并通报",
        &mut sender,
        &mut |n| events.push(n),
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        /*image_input*/ true,
        None,
        extras,
    )
    .unwrap();
    assert_eq!(reply, "混合批完成");
    // 两个 ToolEnd 按调用顺序（view_image 在前）
    let end_names: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            TurnNotify::ToolEnd { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(end_names, vec!["view_image", "send_message_to_user_async"]);
    // 请求面：view_image 结果后附图片附件消息；消息调用恰 accepted；无同文注入
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let image_attachment = msgs
        .iter()
        .position(|m| m["content"].is_array() && m["content"].as_array().unwrap().iter().any(|p| p["type"] == json!("image_url")))
        .expect("view_image 图片附件在请求中");
    let accepted_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == json!("tool")
                && m["tool_call_id"] == json!("m1")
                && m["content"] == json!(r#"{"accepted":true}"#)
        })
        .expect("消息 accepted 结果配对");
    assert!(
        !msgs.iter().any(|m| {
            m["role"] == json!("assistant")
                && m.get("tool_calls").is_none()
                && m["content"].as_str().is_some_and(|c| c.contains("图片已查看"))
        }),
        "无同文 assistant 注入"
    );
    let _ = (image_attachment, accepted_pos);
}

/// A07 直接用例：第 8 次模型请求的响应调用 send_message_to_user_async——
/// 与其他合法工具一样执行并记录（ToolEnd 通知＋accepted 入史），随后报原
/// 请求上限，零第 9 次请求。
#[test]
fn message_tool_executes_on_eighth_request_then_cap_without_ninth() {
    let responses: Vec<String> = (0..7)
        .map(|i| tc_response(&[call(&format!("c{i}"), "exec_command", json!({ "cmd": "true" }))]))
        .chain(std::iter::once(tc_response(&[send_call("m8", "第 8 次的通知")])))
        .collect();
    let run = run_turn_with(responses, true, Vec::new(), |_, _| {});
    assert!(
        run.reply.unwrap_err().contains("已达本轮模型请求上限（8）"),
        "第 8 批后报上限"
    );
    assert_eq!(run.bodies.len(), 8, "零第 9 次请求");
    // 通知已发生且携带 accepted（宿主据此投影消息）
    let end = run.events.iter().find(|e| matches!(e,
        TurnNotify::ToolEnd { id, name, result, .. }
            if id == "m8" && name == "send_message_to_user_async"));
    match end {
        Some(TurnNotify::ToolEnd { result, .. }) => {
            assert_eq!(result, r#"{"accepted":true}"#);
        }
        other => panic!("第 8 批消息调用必有 ToolEnd 终态：{other:?}"),
    }
    // 调用与结果按契约入史
    assert!(run.history.iter().any(|h| matches!(h,
        HistoryItem::ToolResult { tool_call_id, content, .. }
            if tool_call_id == "m8" && content == r#"{"accepted":true}"#)));
}
