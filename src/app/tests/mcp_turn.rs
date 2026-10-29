//! 022 L2：真实工具循环（turn）＋MCP stdio 夹具＋mock 模型。
//!
//! 覆盖 A07（声明与执行一致、伪造名拒绝、MemoryMerge 隔离）、A08（参数
//! 错误网络发送前返回）、A09（结果投影入下一请求）、A10（混合批串行）、
//! A11（批内取消）、A17（与 018 问询共存——批内配对顺序）。
//! 模型边界为替身（预设响应）；MCP 边界为真实 stdio 子进程。

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::mcp_fixtures::{read_record, setup, setup_with, EnvGuard, CANARY, CANARY_VALUE};
use common::TempDir;
use terminal_chat::context::ContextSources;
use terminal_chat::proc_session::ProcessManager;
use terminal_chat::prompt::{HistoryItem, HistoryRole};
use terminal_chat::turn::{run_user_turn_with_context_and_caps, ToolMode, TurnExtras, TurnNotify};

fn fin(content: &str) -> String {
    serde_json::json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

fn tc_response(calls: &[serde_json::Value]) -> String {
    serde_json::json!({ "choices": [{ "message": { "role": "assistant", "content": null, "tool_calls": calls }, "finish_reason": "tool_calls" }] })
        .to_string()
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "id": id, "type": "function", "function": { "name": name, "arguments": arguments.to_string() } })
}

struct TurnRun {
    reply: Result<String, String>,
    history: Vec<HistoryItem>,
    bodies: Vec<serde_json::Value>,
    events: Vec<TurnNotify>,
}

/// 运行一个用户轮：mock 模型按序响应；MCP 快照已由 stdio 夹具准备。
fn run_turn_with(
    ctx: &common::mcp_fixtures::TestCtx,
    responses: Vec<String>,
    cancel: Option<Arc<AtomicBool>>,
    mut on_notify: impl FnMut(&TurnNotify),
) -> TurnRun {
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut iter = responses.into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        let resp = iter.next().expect("预设响应耗尽");
        Ok(serde_json::from_str(&resp).unwrap())
    };
    let mut notify = |n: TurnNotify| {
        on_notify(&n);
        events.push(n);
    };
    let default_cancel = AtomicBool::new(false);
    let cancel_ref = cancel.as_deref().unwrap_or(&default_cancel);
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "执行任务",
        &mut sender,
        &mut notify,
        Some(cancel_ref),
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        TurnExtras {
            tool_search: false,
            mcp: Some(&snapshot),
            send_message: false,
            steer: None, context: None,
            permissions: None,
            orchestration: None,
        },
    );
    TurnRun {
        reply,
        history,
        bodies,
        events,
    }
}

fn tool_results(history: &[HistoryItem]) -> Vec<(String, String)> {
    history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                ..
            } => Some((tool_call_id.clone(), content.clone())),
            _ => None,
        })
        .collect()
}

// ---------- A07：声明与执行一致 ----------

#[test]
fn a07_declaration_and_dispatch_use_same_snapshot() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("echo", "");
    let run = run_turn_with(
        &ctx,
        vec![
            tc_response(&[call("c1", "mcp__fx__echo", serde_json::json!({"text": "hi"}))]),
            fin("完成"),
        ],
        None,
        |_| {},
    );
    // 声明：MCP 定义追加在原生工具之后，名字与快照一致。
    let tools = run.bodies[0]["tools"].as_array().unwrap();
    assert!(tools.len() >= 5, "原生四工具＋MCP：{}", tools.len());
    let last = &tools[tools.len() - 1];
    assert_eq!(last["function"]["name"], "mcp__fx__echo");
    assert_eq!(
        last["function"]["parameters"]["type"], "object",
        "直接暴露参数对象"
    );
    for (i, t) in tools.iter().enumerate().take(tools.len() - 1) {
        assert_ne!(
            t["function"]["name"], "mcp__fx__echo",
            "MCP 定义不插入原生工具之间（位置 {i}）"
        );
    }
    // 分派：服务器收到原始名与参数。
    ctx.close();
    let record = read_record(&ctx.record_path);
    let call_entry = record.iter().find(|e| e["kind"] == "call").unwrap();
    assert_eq!(call_entry["name"], "echo", "原始名送达");
    assert_eq!(call_entry["arguments"]["text"], "hi");
    // 下一请求包含配对结果（CC call ID 原样）。
    let second = &run.bodies[1];
    let msgs = second["messages"].as_array().unwrap();
    let assistant = msgs
        .iter()
        .find(|m| m["role"] == "assistant" && m["tool_calls"].is_array())
        .unwrap();
    assert_eq!(assistant["tool_calls"][0]["id"], "c1");
    let tool_msg = msgs
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == "c1")
        .expect("下一请求含配对 tool 消息");
    let content: serde_json::Value = serde_json::from_str(tool_msg["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["content"][0]["text"], "echo:hi");
    assert!(
        content.get("isError").is_none(),
        "缺省 isError 不擅自补 false：{content}"
    );
}

#[test]
fn a07_forged_names_fail_without_external_calls() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("echo", "");
    let run = run_turn_with(
        &ctx,
        vec![
            tc_response(&[
                call("f1", "mcp__fx__ghost", serde_json::json!({})),
                call("f2", " mcp__fx__echo", serde_json::json!({"text": "x"})),
                call("f3", "mcp__fx", serde_json::json!({})),
                call("f4", "MCP__FX__ECHO", serde_json::json!({"text": "x"})),
                call("f5", "mcp__fx__echo ", serde_json::json!({"text": "x"})),
            ]),
            fin("之后"),
        ],
        None,
        |_| {},
    );
    assert!(run.reply.is_ok(), "伪名按工具失败回传，轮继续：{:?}", run.reply);
    let results = tool_results(&run.history);
    assert_eq!(results.len(), 5, "全部伪名有配对结果");
    for (id, content) in &results {
        let v: serde_json::Value = serde_json::from_str(content).unwrap();
        // 伪名不在快照映射中 → 既有 006 未知工具通道（单 error 字段），零外部调用。
        assert!(
            v["error"].as_str().unwrap_or_default().contains("未知工具"),
            "{id} 伪名走未知工具通道：{v}"
        );
    }
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record.iter().all(|e| e["kind"] != "call"),
        "伪名零外部调用：{record:?}"
    );
}

#[test]
fn a07_memory_merge_turn_never_declares_or_executes_mcp() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("echo", "");
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    // 合并轮请求组装不含 MCP（mem_tools 单独入口）——即使误传快照，
    // turn 层 merge_mode 强制忽略。
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut iter = vec![
        tc_response(&[call("m1", "mcp__fx__echo", serde_json::json!({"text": "x"}))]),
        fin("合并完成"),
    ]
    .into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&iter.next().unwrap()).unwrap())
    };
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut notify = |n: TurnNotify| events.push(n);
    let root = TempDir::new("mcp-merge-root");
    let mem_config = terminal_chat::memory::MemoriesConfig::default();
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "合并输入",
        &mut sender,
        &mut notify,
        None,
        &mut |_| {},
        ToolMode::MemoryMerge {
            root: root.path(),
            system: "合并系统提示",
            expected_epoch: 0,
        },
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        TurnExtras {
            tool_search: false,
            mcp: Some(&snapshot),
            send_message: true,
            steer: None, context: None,
            permissions: None,
            orchestration: None,
        },
    );
    assert!(reply.is_ok());
    // 请求 tools 只含 mem 三工具。
    let tools = bodies[0]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["mem_read", "mem_write", "mem_list"]);
    // 伪造 MCP 名按未知工具失败，零外部调用。
    let mcp_result = tool_results(&history)
        .into_iter()
        .find(|(id, _)| id == "m1")
        .expect("伪造调用有配对结果");
    let v: serde_json::Value = serde_json::from_str(&mcp_result.1).unwrap();
    assert!(
        v.get("isError") == Some(&serde_json::json!(true))
            || v["error"].as_str().is_some(),
        "合并轮伪造 MCP 名失败（mem 工具错误通道）：{v}"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(record.iter().all(|e| e["kind"] != "call"), "合并轮零外部调用");
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

// ---------- A08：参数错误网络发送前返回 ----------

#[test]
fn a08_invalid_arguments_fail_before_sending() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("echo", "");
    let run = run_turn_with(
        &ctx,
        vec![
            tc_response(&[
                call("p1", "mcp__fx__echo", serde_json::json!("not-an-object")),
                call("p2", "mcp__fx__echo", serde_json::Value::String("{bad json".into())),
            ]),
            fin("继续"),
        ],
        None,
        |_| {},
    );
    let results = tool_results(&run.history);
    assert_eq!(results.len(), 2);
    for (id, content) in &results {
        let v: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(v["isError"], true, "{id}");
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("MCP 客户端错误（参数）"),
            "参数错误统一形态：{v}"
        );
    }
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record.iter().all(|e| e["kind"] != "call"),
        "坏参数零发送：{record:?}"
    );
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

// ---------- A09：结果投影（五类内容／isError／不支持响应） ----------

#[test]
fn a09_content_types_projected_in_order() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("content_types", "");
    let run = run_turn_with(
        &ctx,
        vec![
            tc_response(&[call("c9", "mcp__fx__echo", serde_json::json!({"text": "x"}))]),
            fin("看到结果"),
        ],
        None,
        |_| {},
    );
    let (_, content) = tool_results(&run.history).remove(0);
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    let blocks = v["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 5, "五类内容全部保留：{v}");
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[2]["type"], "audio");
    assert_eq!(blocks[3]["type"], "resource_link");
    assert_eq!(blocks[4]["type"], "resource");
    assert_eq!(blocks[0]["text"], "第一段");
    // 下一请求原样携带（JSON 字符串）。
    let tool_msg = run.bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    assert_eq!(tool_msg["content"].as_str().unwrap(), content);
    ctx.close();
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

#[test]
fn a09_is_error_true_stays_failure() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("is_error", "");
    let run = run_turn_with(
        &ctx,
        vec![
            tc_response(&[call("e1", "mcp__fx__echo", serde_json::json!({"text": "x"}))]),
            fin("失败可见"),
        ],
        None,
        |_| {},
    );
    let (_, content) = tool_results(&run.history).remove(0);
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(v["isError"], true, "服务端业务失败原样回传");
    assert_eq!(v["content"][0]["text"], "服务端业务失败");
    ctx.close();
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

#[test]
fn a09_structured_null_distinct_from_absent() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("structured_null", "");
    let run = run_turn_with(
        &ctx,
        vec![
            tc_response(&[call("n1", "mcp__fx__echo", serde_json::json!({"text": "x"}))]),
            fin("ok"),
        ],
        None,
        |_| {},
    );
    let (_, content) = tool_results(&run.history).remove(0);
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(v.get("structuredContent").is_some_and(serde_json::Value::is_null));
    ctx.close();
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

#[test]
fn a09_unsupported_responses_fail_as_client_protocol_error() {
    for (scenario, marker) in [
        ("unknown_content", "协议"),
        ("input_required", "协议"),
        ("task_result", "协议"),
    ] {
        std::env::set_var(CANARY, CANARY_VALUE);
        let ctx = setup(scenario, "");
        let run = run_turn_with(
            &ctx,
            vec![
                tc_response(&[call("u1", "mcp__fx__echo", serde_json::json!({"text": "x"}))]),
                fin("继续"),
            ],
            None,
            |_| {},
        );
        let (_, content) = tool_results(&run.history).remove(0);
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["isError"], true, "{scenario}");
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains(&format!("MCP 客户端错误（{marker}）")),
            "{scenario} 统一失败形态：{v}"
        );
        ctx.close();
        std::env::remove_var(CANARY);
        std::env::remove_var("TCHAT_USER_DIR");
    }
}

// ---------- A10：混合批串行 ----------

#[test]
fn a10_mixed_native_and_mcp_batch_runs_serially_and_pairs_in_order() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("echo", "");
    let run = run_turn_with(
        &ctx,
        vec![
            tc_response(&[
                call("x1", "mcp__fx__echo", serde_json::json!({"text": "mcp部分"})),
                call("x2", "exec_command", serde_json::json!({"cmd": "printf native-part > /dev/stdout", "yield_time_ms": 250})),
                call("x3", "mcp__fx__echo", serde_json::json!({"text": "second"})),
            ]),
            fin("混批完成"),
        ],
        None,
        |_| {},
    );
    let results = tool_results(&run.history);
    assert_eq!(results.len(), 3, "全部配对");
    assert_eq!(results[0].0, "x1");
    assert_eq!(results[2].0, "x3");
    // 串行：Begin/End 交替。
    let mut seen_begin = false;
    let mut ordered = true;
    for e in &run.events {
        match e {
            TurnNotify::ToolBegin { .. } => {
                if seen_begin {
                    ordered = false;
                }
                seen_begin = true;
            }
            TurnNotify::ToolEnd { .. } => seen_begin = false,
            _ => {}
        }
    }
    assert!(ordered, "工具严格串行（无并发 Begin）");
    let mcp_content: serde_json::Value =
        serde_json::from_str(&results[0].1).unwrap();
    assert_eq!(mcp_content["content"][0]["text"], "echo:mcp部分");
    let native: serde_json::Value = serde_json::from_str(&results[1].1).unwrap();
    assert_eq!(native["status"], "completed", "原生工具照常执行");
    ctx.close();
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

// ---------- A11：批内取消 ----------

#[test]
fn a11_cancel_during_mcp_call_completes_batch_and_stops() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup_with("echo", "", 2500);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_hook = cancel.clone();
    let run = run_turn_with(
        &ctx,
        vec![tc_response(&[
            call("k1", "mcp__fx__echo", serde_json::json!({"text": "slow"})),
            call("k2", "mcp__fx__echo", serde_json::json!({"text": "never"})),
        ])],
        Some(cancel.clone()),
        move |n: &TurnNotify| {
            if let TurnNotify::ToolBegin { id, .. } = n {
                if id == "k1" {
                    let c = cancel_for_hook.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(400));
                        c.store(true, Ordering::SeqCst);
                    });
                }
            }
        },
    );
    // 取消＝轮终止错误；k1 已发＝状态未知；k2 未执行＝取消跳过。
    assert!(run.reply.is_err());
    let results = tool_results(&run.history);
    assert_eq!(results.len(), 2, "同批剩余结果补齐");
    let k1: serde_json::Value = serde_json::from_str(&results[0].1).unwrap();
    assert_eq!(k1["isError"], true);
    assert!(
        k1["content"][0]["text"].as_str().unwrap().contains("状态未知"),
        "已发调用状态未知：{k1}"
    );
    let k2: serde_json::Value = serde_json::from_str(&results[1].1).unwrap();
    assert!(
        k2["error"].as_str().unwrap().contains("用户取消，本调用未执行"),
        "未执行调用按既有取消跳过形态补齐：{k2}"
    );
    // 只有一次模型请求（不续请求模型）。
    assert_eq!(run.bodies.len(), 1);
    ctx.close();
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

// ---------- A17：与 018 问询共存（批内配对顺序） ----------

#[test]
fn a17_mcp_call_coexists_with_user_input_in_batch() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("echo", "");
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut iter = vec![
        tc_response(&[
            call("q1", "request_user_input", serde_json::json!({
                "questions": [{
                    "id": "q-022",
                    "header": "确认",
                    "question": "继续吗？",
                    "options": [{"label": "是", "description": "继续"}],
                    "multiple": false
                }]
            })),
            call("m2", "mcp__fx__echo", serde_json::json!({"text": "after-answer"})),
        ]),
        fin("问询与结果都在"),
    ]
    .into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&iter.next().unwrap()).unwrap())
    };
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut notify = |n: TurnNotify| events.push(n);
    let mut input_host =
        |_: &terminal_chat::turn::ParsedToolCall,
          _: &[terminal_chat::user_input::Question]| {
            Ok(terminal_chat::user_input::Response {
                answers: [(
                    "q-022".to_string(),
                    terminal_chat::user_input::Answer {
                        answers: vec!["是".to_string()],
                    },
                )]
                .into_iter()
                .collect(),
            })
        };
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "执行任务",
        &mut sender,
        &mut notify,
        None,
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        Some(&mut input_host),
        None,
        false,
        None,
        TurnExtras {
            tool_search: false,
            mcp: Some(&snapshot),
            send_message: false,
            steer: None, context: None,
            permissions: None,
            orchestration: None,
        },
    );
    assert!(reply.is_ok());
    // 批内：调用与结果按原始顺序配对（问询结果在前、MCP 在后，无插入消息）。
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let tool_ids: Vec<String> = msgs
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["tool_call_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tool_ids, vec!["q1", "m2"], "按调用顺序完整配对");
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert_eq!(record.iter().filter(|e| e["kind"] == "call").count(), 1);
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}

// ---------- A10/§2.3：128 总量上限在模型请求前拒绝 ----------

#[test]
fn a10_total_tool_cap_rejects_before_model_request() {
    std::env::set_var(CANARY, CANARY_VALUE);
    let ctx = setup("tools_129", "");
    assert_eq!(ctx.snapshot.len(), 129, "129 项 MCP 工具全部发布");
    let run = run_turn_with(
        &ctx,
        vec![fin("不应到达")],
        None,
        |_| {},
    );
    assert!(run.reply.is_err(), "超限明确报错");
    let err = run.reply.unwrap_err();
    assert!(err.contains("128") && err.contains("129"), "错误含总量与 MCP 数：{err}");
    // 零模型请求（bodies 为空＝请求未发送）。
    assert!(run.bodies.is_empty(), "超限时零模型请求");
    ctx.close();
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
}


// ---------- A17 补充：MCP 与 019/021 共用处理路径的最小混合批 ----------

/// MCP 调用与 view_image 同批：先完整工具配对，view_image 成功结果的图片
/// 投影经共用 `project_history` 的批末 pending_attachments 路径追加，不插在
/// assistant 调用与配对结果之间（019 §4 适配与 022 §2.5 共用同一投影）。
#[test]
fn a17_mcp_with_view_image_batch_pairs_then_image_projection() {
    use std::sync::mpsc as std_mpsc;
    // 受控接续通道（021 SteerChannel 的测试实现，与 message_to_user.rs 同构）。
    struct QueuedChannel {
        tx: std_mpsc::Sender<terminal_chat::turn::SteerInput>,
        rx: std_mpsc::Receiver<terminal_chat::turn::SteerInput>,
    }
    impl terminal_chat::turn::SteerChannel for QueuedChannel {
        fn drain(&mut self) -> Vec<terminal_chat::turn::SteerInput> {
            let mut out = Vec::new();
            while let Ok(input) = self.rx.try_recv() {
                out.push(input);
            }
            out
        }
        fn seal(&mut self) -> terminal_chat::turn::SteerSeal {
            let inputs = self.drain();
            if inputs.is_empty() {
                terminal_chat::turn::SteerSeal::Closed
            } else {
                terminal_chat::turn::SteerSeal::Inputs(inputs)
            }
        }
        fn close(&mut self) -> Vec<terminal_chat::turn::SteerInput> {
            self.drain()
        }
    }
    let env_guard = EnvGuard::acquire(common::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
    std::env::set_var(CANARY, CANARY_VALUE);
    let dir = common::TempDir::new("a17-mcp-image");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    let record_dir = common::TempDir::new("a17-record");
    let record_path = record_dir.path().join("record.jsonl");
    let fixture = common::mcp_fixtures::FIXTURE.replace('\\', "/");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "echo", "--record", "{}"], "cwd": "{}"}}}}}}"#,
        record_path.display().to_string().replace('\\', "/"),
        std::env::temp_dir().display()
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = terminal_chat::mcp::manager::McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");

    // 1×1 PNG 夹具图片（view_image 成功后产生 image 载荷）。
    let img = dir.path().join("a17.png");
    {
        let img_buf = image::ImageBuffer::from_pixel(2, 2, image::Rgba([9u8, 8, 7, 255]));
        let mut cursor = std::io::Cursor::new(Vec::new());
        img_buf
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&img, cursor.into_inner()).unwrap();
    }

    // 运行中接续：工具批执行期间提交（批末接入——021 §2.3-4 共用边界）。
    let (steer_tx, steer_rx) = std_mpsc::channel();
    let mut channel = QueuedChannel {
        tx: steer_tx.clone(),
        rx: steer_rx,
    };

    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut steer_seen = false;
    let steer_tx_for_notify = steer_tx.clone();
    let mut iter = vec![
        tc_response(&[
            call("i1", "mcp__fx__echo", serde_json::json!({"text": "先 MCP"})),
            call(
                "i2",
                "view_image",
                serde_json::json!({"path": img.to_string_lossy()}),
            ),
        ]),
        tc_response(&[call(
            "i3",
            "mcp__fx__echo",
            serde_json::json!({"text": "接续后"}),
        )]),
        fin("A17 混合完成"),
    ]
    .into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&iter.next().unwrap()).unwrap())
    };
    let mut notify = |n: TurnNotify| {
        // 首个工具开始执行时提交接续输入（批内受理、批末接入）。
        if !steer_seen {
            if let TurnNotify::ToolBegin { .. } = &n {
                steer_seen = true;
                let _ = steer_tx_for_notify.send(terminal_chat::turn::SteerInput {
                    id: 1,
                    text: "批期间的接续输入".to_string(),
                    image_paths: Vec::new(),
                    text_elements: Vec::new(),
                });
            }
        }
        events.push(n);
    };
    let reply = run_user_turn_with_context_and_caps(
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
        /*image_input*/ true,
        None,
        TurnExtras {
            tool_search: false,
            mcp: Some(&snapshot),
            send_message: false,
            steer: Some(&mut channel), context: None,
            permissions: None,
            orchestration: None,
        },
    );
    assert!(reply.is_ok(), "{reply:?}");

    // 下一请求（第 2 次模型请求）：同批配对完整＋图片附件在配对之后、
    // 接续输入按来源顺序在批末（021 §2.3-4），且不插在调用与结果之间。
    let msgs = bodies[1]["messages"].as_array().unwrap();
    let pos = |pred: &dyn Fn(&serde_json::Value) -> bool| {
        msgs.iter().position(pred).expect("消息存在")
    };
    let assistant_i = pos(&|m| m["role"] == "assistant" && m["tool_calls"].is_array());
    let tool_i1 = pos(&|m| m["role"] == "tool" && m["tool_call_id"] == "i1");
    let tool_i2 = pos(&|m| m["role"] == "tool" && m["tool_call_id"] == "i2");
    let image_part = pos(&|m| {
        m["content"].is_array()
            && m["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["type"] == "image_url")
    });
    let steer_user = pos(&|m| {
        m["role"] == "user" && m["content"].as_str() == Some("批期间的接续输入")
    });
    assert!(assistant_i < tool_i1 && tool_i1 < tool_i2, "配对紧跟调用");
    assert!(
        tool_i2 < image_part,
        "图片附件在完整配对之后（批末投影）"
    );
    assert!(
        image_part < steer_user,
        "接续输入在批末图片投影之后按来源顺序入史"
    );
    // 无重复接入：接续恰一条。
    let steer_count = msgs
        .iter()
        .filter(|m| m["content"].as_str() == Some("批期间的接续输入"))
        .count();
    assert_eq!(steer_count, 1, "零重复接入");
    // MCP 原始调用真实发生（服务器账本）。
    manager.close();
    let record = read_record(&record_path);
    let mcp_calls = record
        .iter()
        .filter(|e| e["kind"] == "call")
        .collect::<Vec<_>>();
    assert_eq!(mcp_calls.len(), 2, "两轮 MCP 调用：{mcp_calls:?}");
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_USER_DIR");
    drop(env_guard);
}

// ---------- 030：工具搜索与按需加载（真实 turn＋stdio MCP＋mock 模型） ----------

/// 带 tool_search 开关的轮运行（复用 run_turn_with 全套真实边界）。
fn run_turn_ex(
    ctx: &common::mcp_fixtures::TestCtx,
    responses: Vec<String>,
    cancel: Option<Arc<AtomicBool>>,
    mut on_notify: impl FnMut(&TurnNotify),
    tool_search: bool,
) -> TurnRun {
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut events: Vec<TurnNotify> = Vec::new();
    let mut iter = responses.into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        let resp = iter.next().expect("预设响应耗尽");
        Ok(serde_json::from_str(&resp).unwrap())
    };
    let mut notify = |n: TurnNotify| {
        on_notify(&n);
        events.push(n);
    };
    let default_cancel = AtomicBool::new(false);
    let cancel_ref = cancel.as_deref().unwrap_or(&default_cancel);
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "执行任务",
        &mut sender,
        &mut notify,
        Some(cancel_ref),
        &mut |_| {},
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        None,
        TurnExtras {
            mcp: Some(&snapshot),
            send_message: false,
            tool_search,
            steer: None,
            context: None,
            permissions: None,
            orchestration: None,
        },
    );
    TurnRun { reply, history, bodies, events }
}

fn tool_names(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .expect("tools 数组")
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_string())
        .collect()
}

fn result_of(history: &[HistoryItem], id: &str) -> serde_json::Value {
    let (_, content) = tool_results(history)
        .into_iter()
        .find(|(cid, _)| cid == id)
        .unwrap_or_else(|| panic!("缺少 {id} 的结果"));
    serde_json::from_str(&content).expect("工具结果为 JSON")
}

fn recorded_calls(record_path: &std::path::Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(record_path).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == "call")
        .collect()
}

#[test]
fn ts30_search_then_load_then_call() {
    let ctx = setup("searchable", "");
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("c1", "tool_search", serde_json::json!({"query": "email", "limit": 2}))]),
            tc_response(&[call("c2", "mcp__fx__send_mail", serde_json::json!({"to": "a@b.c"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    // 请求 1：仅常驻＋tool_search；未加载动态定义不发送
    let names1 = tool_names(&run.bodies[0]);
    assert!(names1.contains(&"tool_search".to_string()), "须声明搜索工具");
    assert!(!names1.iter().any(|n| n.starts_with("mcp__fx__")), "动态定义须延迟：{names1:?}");
    // c1：三字段成功＋完整定义（Schema 原样）
    let r1 = result_of(&run.history, "c1");
    assert_eq!(r1["ok"], serde_json::json!(true));
    assert_eq!(r1["error"], serde_json::Value::Null);
    assert_eq!(r1["tools"].as_array().unwrap().len(), 1);
    assert_eq!(r1["tools"][0]["function"]["name"], serde_json::json!("mcp__fx__send_mail"));
    assert!(r1["tools"][0]["function"]["parameters"]["properties"]["to"].is_object());
    // 请求 2：声明已加载 send_mail（仍无 weather_lookup）
    let names2 = tool_names(&run.bodies[1]);
    assert!(names2.contains(&"mcp__fx__send_mail".to_string()), "下一请求须带命中定义");
    assert!(!names2.contains(&"mcp__fx__weather_lookup".to_string()));
    // 真实调用恰一次：服务器只收到原始名与原参数
    let calls = recorded_calls(&ctx.record_path);
    assert_eq!(calls.len(), 1, "搜索零业务调用，业务调用恰一次");
    assert_eq!(calls[0]["name"], serde_json::json!("send_mail"));
    assert_eq!(calls[0]["arguments"]["to"], serde_json::json!("a@b.c"));
    assert_eq!(run.reply, Ok("done".to_string()));
}

#[test]
fn ts30_same_batch_guess_zero_calls() {
    let ctx = setup("searchable", "");
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[
                call("c1", "tool_search", serde_json::json!({"query": "email"})),
                call("c2", "mcp__fx__send_mail", serde_json::json!({"to": "x"})),
            ]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    // 同批先搜索后猜名：目录命中也不可执行（请求集合门）
    let r2 = result_of(&run.history, "c2");
    assert!(r2.get("error").is_some(), "未声明调用走未知工具通用错误：{r2}");
    assert!(recorded_calls(&ctx.record_path).is_empty(), "零业务调用");
    // 下一请求已声明 send_mail（批末搜索已入加载集合）
    let names2 = tool_names(&run.bodies[1]);
    assert!(names2.contains(&"mcp__fx__send_mail".to_string()));
    assert_eq!(run.reply, Ok("done".to_string()));
}

#[test]
fn ts30_unsearched_guess_zero_calls() {
    let ctx = setup("searchable", "");
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("c1", "mcp__fx__weather_lookup", serde_json::json!({"city": "x"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    let r1 = result_of(&run.history, "c1");
    assert!(r1.get("error").is_some(), "未搜索直接猜名须失败：{r1}");
    assert!(recorded_calls(&ctx.record_path).is_empty(), "零业务调用");
}

#[test]
fn ts30_off_mode_full_declaration_and_call() {
    let ctx = setup("searchable", "");
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("c1", "mcp__fx__send_mail", serde_json::json!({"to": "y"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        false,
    );
    let names1 = tool_names(&run.bodies[0]);
    assert!(names1.contains(&"mcp__fx__send_mail".to_string()), "关闭模式全量声明不变");
    assert!(!names1.contains(&"tool_search".to_string()), "关闭模式不声明搜索工具");
    assert_eq!(recorded_calls(&ctx.record_path).len(), 1);
}

#[test]
fn ts30_fake_search_unavailable() {
    let ctx = setup("searchable", "");
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("c1", "tool_search", serde_json::json!({"query": "email"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        false,
    );
    let r1 = result_of(&run.history, "c1");
    assert_eq!(r1["ok"], serde_json::json!(false), "未声明模型的伪造搜索须失败");
    assert_eq!(r1["tools"], serde_json::json!([]));
    assert!(r1["error"].as_str().unwrap().contains("不可用"));
    assert!(recorded_calls(&ctx.record_path).is_empty());
}

#[test]
fn ts30_multi_search_accumulate_and_dedupe() {
    let ctx = setup("searchable", "");
    // 审查 #2 回归：先搜快照靠后的 weather（下标 2）再搜靠前的 email
    //（下标 0）——下一请求声明必须按快照身份序（send_mail 在前）。
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[
                call("c1", "tool_search", serde_json::json!({"query": "weather"})),
                call("c2", "tool_search", serde_json::json!({"query": "email"})),
            ]),
            tc_response(&[call("c3", "tool_search", serde_json::json!({"query": "email"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    // 请求 2：两批搜索累计，两个定义各一次，且按快照身份序（§1.4-2）
    let names2 = tool_names(&run.bodies[1]);
    assert_eq!(names2.iter().filter(|n| n == &"mcp__fx__send_mail").count(), 1);
    assert_eq!(names2.iter().filter(|n| n == &"mcp__fx__weather_lookup").count(), 1);
    let pos_mail = names2.iter().position(|n| n == "mcp__fx__send_mail").unwrap();
    let pos_weather = names2.iter().position(|n| n == "mcp__fx__weather_lookup").unwrap();
    assert!(pos_mail < pos_weather, "声明序＝快照身份序：{names2:?}");
    // 请求 3：重复命中不复制声明
    let names3 = tool_names(&run.bodies[2]);
    assert_eq!(names3.iter().filter(|n| n == &"mcp__fx__send_mail").count(), 1);
    // c3 结果仍返回定义（重复命中可再次返回；加载集合去重）
    let r3 = result_of(&run.history, "c3");
    assert_eq!(r3["ok"], serde_json::json!(true));
    assert_eq!(r3["tools"][0]["function"]["name"], serde_json::json!("mcp__fx__send_mail"));
}

#[test]
fn ts30_eighth_response_search_no_ninth_request() {
    let ctx = setup("searchable", "");
    let mut responses: Vec<String> = (1..=8)
        .map(|i| tc_response(&[call(&format!("c{i}"), "tool_search", serde_json::json!({"query": "email"}))]))
        .collect();
    responses.push(fin("unused"));
    let run = run_turn_ex(&ctx, responses, None, |_| {}, true);
    assert_eq!(run.bodies.len(), 8, "不发第 9 次请求");
    assert!(run.reply.is_err(), "额度耗尽须明确失败");
    // 第 8 次响应中的搜索照常执行并入史
    assert_eq!(
        run.history.iter().filter(|h| matches!(h, HistoryItem::ToolResult { tool_call_id, .. } if tool_call_id == "c8")).count(),
        1
    );
}

#[test]
fn ts30_cancel_backfill_three_field() {
    let ctx = setup("searchable", "");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[
                call("c1", "update_plan", serde_json::json!({"plan": {"step": "s", "status": "completed"}})),
                call("c2", "tool_search", serde_json::json!({"query": "email"})),
            ]),
            fin("done"),
        ],
        Some(cancel),
        move |n| {
            if let TurnNotify::ToolEnd { name, .. } = n {
                if name == "update_plan" {
                    flag.store(true, Ordering::SeqCst);
                }
            }
        },
        true,
    );
    let r2 = result_of(&run.history, "c2");
    assert_eq!(r2["ok"], serde_json::json!(false), "取消未执行搜索按三字段回填：{r2}");
    assert_eq!(r2["tools"], serde_json::json!([]));
    assert!(r2["error"].as_str().unwrap().contains("取消"));
}

#[test]
fn ts30_catalog_over_limit_fails_before_first_request() {
    let ctx = common::mcp_fixtures::setup_two("searchable_1300", "searchable_1300", "", "");
    let run = run_turn_ex(
        &ctx,
        vec![fin("unused")],
        None,
        |_| {},
        true,
    );
    let err = run.reply.expect_err("目录超限须明确失败");
    assert!(err.contains("2048"), "错误须说明条目上限：{err}");
    assert!(run.bodies.is_empty(), "超限不发半份目录、不发任何模型请求");
}

#[test]
fn ts30_new_turn_resets_loaded() {
    let ctx = setup("searchable", "");
    let first = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("c1", "tool_search", serde_json::json!({"query": "email"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    assert!(tool_names(&first.bodies[1]).contains(&"mcp__fx__send_mail".to_string()));
    // 下一用户轮：加载集合从空开始（不跨轮恢复）
    let second = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("d1", "tool_search", serde_json::json!({"query": "weather"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    let names = tool_names(&second.bodies[0]);
    assert!(!names.contains(&"mcp__fx__send_mail".to_string()), "新轮不得继承旧加载集合");
    assert!(!names.iter().any(|n| n.starts_with("mcp__fx__")));
}

#[test]
fn ts30_declared_budget_over_128_fails_whole_search() {
    // S07/S09：常驻＋tool_search＋累计加载 ≤128；超限本次搜索整体失败，
    // 此前成功加载不回滚、不被淘汰；成功定义恰一次进入下一请求。
    let ctx = setup("searchable_budget", "");
    let groups = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let searches: Vec<serde_json::Value> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| call(&format!("c{}", i + 1), "tool_search", serde_json::json!({"query": g, "limit": 26})))
        .collect();
    let run = run_turn_ex(
        &ctx,
        vec![tc_response(&searches), fin("done")],
        None,
        |_| {},
        true,
    );
    // 前 4 组加载 104 项（5 常驻＋104＝109 ≤128）；第 5 组 109+26＝135＞128 → 失败
    for i in 1..=4 {
        let r = result_of(&run.history, &format!("c{i}"));
        assert_eq!(r["ok"], serde_json::json!(true), "第 {i} 组应成功：{r}");
        assert_eq!(r["tools"].as_array().unwrap().len(), 26);
    }
    let r5 = result_of(&run.history, "c5");
    assert_eq!(r5["ok"], serde_json::json!(false), "第 5 组须整体失败：{r5}");
    assert_eq!(r5["tools"], serde_json::json!([]));
    let err = r5["error"].as_str().unwrap();
    assert!(err.contains("128"), "错误须说明声明上限：{err}");
    assert!(err.contains("limit"), "错误须提示缩小 limit：{err}");
    // 下一请求：恰 104 项已加载定义（无淘汰、无重复）
    let names2 = tool_names(&run.bodies[1]);
    let loaded = names2.iter().filter(|n| n.starts_with("mcp__fx__")).count();
    assert_eq!(loaded, 104, "此前成功加载全部保留");
    assert_eq!(names2.len(), 5 + 104, "4 内置＋tool_search＋104 动态");
    assert_eq!(run.reply, Ok("done".to_string()));
}

#[test]
fn ts30_resource_tools_cocxist_with_search() {
    // S02：仅资源能力的服务器仍提供固定资源工具；动态工具（echo）延迟。
    let ctx = setup("tools_and_resources", "");
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("c1", "tool_search", serde_json::json!({"query": "echo"}))]),
            tc_response(&[call("c2", "mcp__fx__echo", serde_json::json!({"text": "hi"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    let names1 = tool_names(&run.bodies[0]);
    assert!(names1.contains(&"tool_search".to_string()));
    assert!(names1.contains(&"list_mcp_resources".to_string()), "固定资源工具保持直接可用");
    assert!(names1.contains(&"read_mcp_resource".to_string()));
    assert!(!names1.contains(&"mcp__fx__echo".to_string()), "动态 echo 延迟");
    assert!(tool_names(&run.bodies[1]).contains(&"mcp__fx__echo".to_string()));
    assert_eq!(recorded_calls(&ctx.record_path).len(), 1);
    assert_eq!(run.reply, Ok("done".to_string()));
}

#[test]
fn ts30_empty_dynamic_catalog_no_declaration() {
    // S01：开启搜索但无可搜索动态工具——不声明 tool_search（原路径不变），
    // 伪造搜索调用按不可用失败；空目录轮不产生任何业务调用。
    let ctx = setup("empty_catalog", "");
    let run = run_turn_ex(
        &ctx,
        vec![
            tc_response(&[call("c1", "tool_search", serde_json::json!({"query": "x"}))]),
            fin("done"),
        ],
        None,
        |_| {},
        true,
    );
    let names1 = tool_names(&run.bodies[0]);
    assert!(!names1.contains(&"tool_search".to_string()), "无可搜索目录不声明搜索工具");
    let r1 = result_of(&run.history, "c1");
    assert_eq!(r1["ok"], serde_json::json!(false), "伪造搜索须失败：{r1}");
    assert!(r1["error"].as_str().unwrap().contains("不可用"));
    assert_eq!(run.reply, Ok("done".to_string()));
}
