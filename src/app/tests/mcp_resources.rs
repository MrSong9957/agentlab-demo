//! 023 L2：MCP 资源面——真实 stdio 夹具＋mock 模型。
//!
//! M1：资源专用服务器连接（不发 tools/list）、`list_mcp_resources` 按资源
//! 能力声明、定向列举真实调用与结果入下一模型请求。
//! 模型边界为替身（预设响应）；MCP 边界为真实 stdio 子进程。

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::mcp_fixtures::{definition_names, read_record, setup, setup_with, TestCtx};
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

fn run_resource_turn(
    ctx: &TestCtx,
    responses: Vec<String>,
) -> (Result<String, String>, Vec<HistoryItem>, Vec<serde_json::Value>) {
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut iter = responses.into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        let resp = iter.next().expect("预设响应耗尽");
        Ok(serde_json::from_str(&resp).unwrap())
    };
    let mut notify = |_n: TurnNotify| {};
    let cancel = AtomicBool::new(false);
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "列出资源",
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
        TurnExtras {
            tool_search: false,
            mcp: Some(&snapshot),
            send_message: false,
            steer: None, context: None,
            permissions: None,
            orchestration: None,
        },
    );
    (reply, history, bodies)
}

fn tool_result(history: &[HistoryItem], id: &str) -> serde_json::Value {
    history
        .iter()
        .find_map(|h| match h {
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                ..
            } if tool_call_id == id => serde_json::from_str(content).ok(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("历史中缺少 {id} 的工具结果"))
}

#[test]
fn m1_resources_only_server_declares_tool_without_tools_list() {
    let ctx = setup("resources_only", "");
    // 声明：仅资源服务器 → 已实现的资源工具各声明恰一次（M1 列举、
    // M2 模板；读取在 M2-2 实现前不声明——声明与执行一致）。
    let names = definition_names(&ctx.snapshot);
    for expected in ["list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource"] {
        let count = names.iter().filter(|n| n.as_str() == expected).count();
        assert_eq!(count, 1, "{expected} 声明恰一次：{names:?}");
    }
    ctx.close();
    let record = read_record(&ctx.record_path);
    // 仅资源服务器不收到 tools/list（能力独立判断）。
    assert!(
        record.iter().all(|e| e["kind"] != "list"),
        "仅资源服务器不发 tools/list：{record:?}"
    );
    // 准备阶段不预读资源目录（023 §3.2：实际调用时才获取）。
    assert!(
        record.iter().all(|e| e["kind"] != "list_resources"),
        "准备阶段不预读资源目录：{record:?}"
    );
}

#[test]
fn m1_no_capability_server_still_rejected() {
    // 保留 022 拒绝语义：没有 tools 也没有 resources 能力仍拒绝连接。
    let ctx = setup("no_tools_capability", "");
    let names = definition_names(&ctx.snapshot);
    assert!(
        names.iter().all(|n| n != "list_mcp_resources"),
        "无能力服务器不产生资源声明：{names:?}"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record.iter().all(|e| e["kind"] != "list_resources"),
        "无能力服务器零资源请求"
    );
    // 静默检查：快照状态行应显示不可用（连接被拒）。
    let lines = ctx.manager.status_lines().join("\n");
    assert!(
        lines.contains("不可用"),
        "无能力服务器在 /mcp 中不可用：{lines}"
    );
}

#[test]
fn m1_directed_list_dispatches_real_request_and_feeds_next_model_call() {
    let ctx = setup("resources_only", "");
    let (reply, history, bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "list_mcp_resources",
                serde_json::json!({"server": "fx"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "轮应成功：{reply:?}");
    // 声明：首个请求含资源工具定义。
    let tools = bodies[0]["tools"].as_array().unwrap();
    assert!(
        tools
            .iter()
            .any(|t| t["function"]["name"] == "list_mcp_resources"),
        "首个模型请求声明资源工具"
    );
    // 真实结果＋原调用 ID 进入下一模型请求。
    let result = tool_result(&history, "c1");
    assert_eq!(result["server"], "fx", "结果携带服务器归属：{result}");
    let resources = result["resources"].as_array().expect("资源数组");
    assert_eq!(resources.len(), 2, "夹具返回两项资源：{result}");
    assert_eq!(resources[0]["uri"], "tchat://guide/hello");
    assert_eq!(resources[0]["name"], "hello");
    assert_eq!(resources[0]["mimeType"], "text/plain");
    assert!(
        result.get("nextCursor").is_none(),
        "单页无游标时不伪造字段：{result}"
    );
    assert!(
        result.get("isError").is_none(),
        "成功结果不带 isError：{result}"
    );
    let second = &bodies[1];
    let msgs = second["messages"].as_array().unwrap();
    let tool_msg = msgs
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == "c1")
        .expect("下一请求含配对 tool 消息");
    let content: serde_json::Value =
        serde_json::from_str(tool_msg["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["resources"][0]["uri"], "tchat://guide/hello");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let list_entry = record
        .iter()
        .find(|e| e["kind"] == "list_resources")
        .expect("服务器收到 resources/list");
    assert_eq!(
        list_entry["params"]["cursor"], serde_json::Value::Null,
        "无游标时请求不带 cursor（SDK 自动注入的 _meta.progressToken 除外）：{list_entry}"
    );
}

#[test]
fn m2_templates_directed_list_returns_uri_templates() {
    let ctx = setup("resources_only", "");
    // 声明：模板工具随实现声明（M2-1 前不声明）。
    let names = definition_names(&ctx.snapshot);
    assert!(
        names.iter().any(|n| n == "list_mcp_resource_templates"),
        "模板工具应声明：{names:?}"
    );
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "list_mcp_resource_templates",
                serde_json::json!({"server": "fx"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["server"], "fx", "{result}");
    let templates = result["resourceTemplates"].as_array().expect("模板数组");
    assert_eq!(templates.len(), 2);
    assert_eq!(templates[0]["uriTemplate"], "tchat://guide/notes/{id}");
    assert_eq!(templates[0]["name"], "note");
    assert_eq!(templates[1]["mimeType"], "image/png");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let entry = record
        .iter()
        .find(|e| e["kind"] == "list_templates")
        .expect("服务器收到 resources/templates/list");
    assert_eq!(
        entry["params"]["cursor"], serde_json::Value::Null,
        "无游标不带 cursor"
    );
}

#[test]
fn m2_read_preserves_contents_blob_meta_and_routes_template_uri() {
    let ctx = setup("resources_only", "");
    let (reply, history, bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[
                // 模板展开后的 URI 原样抵达指定服务器（宿主不解释 URI）。
                call(
                    "c1",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/notes/42"}),
                ),
                call(
                    "c2",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/multi"}),
                ),
                call(
                    "c3",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/logo"}),
                ),
                call(
                    "c4",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/config"}),
                ),
            ]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    // 声明：读取工具随实现声明。
    let tools = bodies[0]["tools"].as_array().unwrap();
    assert!(
        tools.iter().any(|t| t["function"]["name"] == "read_mcp_resource"),
        "读取工具声明"
    );
    // c1：模板展开 URI 原样送达＋默认文本内容。
    let r1 = tool_result(&history, "c1");
    assert_eq!(r1["server"], "fx", "{r1}");
    assert_eq!(r1["uri"], "tchat://guide/notes/42", "{r1}");
    assert_eq!(r1["contents"][0]["text"], "content of tchat://guide/notes/42");
    // c2：多段 contents 顺序保留。
    let r2 = tool_result(&history, "c2");
    let contents = r2["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 2, "{r2}");
    assert_eq!(contents[0]["text"], "第一段");
    assert_eq!(contents[1]["text"], "第二段");
    assert_eq!(contents[1]["uri"], "tchat://guide/multi#part2");
    // c3：blob 保持编码字符串＋mimeType（base64(b"\x89PNG-fake-bytes")）。
    let r3 = tool_result(&history, "c3");
    assert_eq!(r3["contents"][0]["blob"], "iVBORy1mYWtlLWJ5dGVz", "{r3}");
    assert_eq!(r3["contents"][0]["mimeType"], "image/png");
    // c4：服务端 _meta 保留。
    let r4 = tool_result(&history, "c4");
    assert_eq!(r4["contents"][0]["text"], "{\"theme\": \"dark\"}", "{r4}");
    assert_eq!(r4["_meta"]["origin"], "fixture", "{r4}");
    assert_eq!(r4["contents"][0]["mimeType"], "application/json");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let reads: Vec<_> = record.iter().filter(|e| e["kind"] == "read_resource").collect();
    assert_eq!(reads.len(), 4, "四次读取各一次：{record:?}");
    assert_eq!(reads[0]["params"]["uri"], "tchat://guide/notes/42");
    assert_eq!(reads[1]["params"]["uri"], "tchat://guide/multi");
    assert_eq!(reads[2]["params"]["uri"], "tchat://guide/logo");
    assert_eq!(reads[3]["params"]["uri"], "tchat://guide/config");
}

#[test]
fn m2_polluted_resource_results_are_rejected_wholesale() {
    std::env::set_var(common::mcp_fixtures::CANARY, common::mcp_fixtures::CANARY_VALUE);
    let ctx = setup("resources_only", r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#);
    let (reply, history, bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[
                // 正文明文污染。
                call(
                    "c1",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/polluted"}),
                ),
                // blob 为 base64(凭证)：仅扫序列化字符串会漏掉。
                call(
                    "c2",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/polluted_blob"}),
                ),
                // 服务端 _meta 污染。
                call(
                    "c3",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/polluted_meta"}),
                ),
                // 列举结果污染（资源描述含凭证）也整份拒绝。
                call(
                    "c4",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": "tchat://guide/hello"}),
                ),
            ]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    for id in ["c1", "c2", "c3"] {
        let result = tool_result(&history, id);
        assert_eq!(result["isError"], true, "{id} 应整份拒绝：{result}");
        let text = serde_json::to_string(&result).unwrap();
        assert!(
            !text.contains(common::mcp_fixtures::CANARY_VALUE),
            "{id} 错误形态不得回显凭证"
        );
        assert!(
            text.contains("敏感数据"),
            "{id} 应为标准客户端错误：{text}"
        );
    }
    // c4 干净结果正常通过（同批污染拒绝不影响后续）。
    let clean = tool_result(&history, "c4");
    assert!(clean.get("isError").is_none(), "{clean}");
    // 历史／下一模型请求均不包含凭证原文。
    let history_text = serde_json::to_string(&history).unwrap();
    assert!(!history_text.contains(common::mcp_fixtures::CANARY_VALUE));
    let bodies_text = serde_json::to_string(&bodies).unwrap();
    assert!(!bodies_text.contains(common::mcp_fixtures::CANARY_VALUE));
    ctx.close();
}

#[test]
fn m2_read_argument_errors_fail_before_sending() {
    let ctx = setup("resources_only", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[
                call("c1", "read_mcp_resource", serde_json::json!({"uri": "u"})),
                call(
                    "c2",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx"}),
                ),
                call(
                    "c3",
                    "read_mcp_resource",
                    serde_json::json!({"server": "fx", "uri": 42}),
                ),
                call(
                    "c4",
                    "read_mcp_resource",
                    serde_json::json!({"server": ["fx"], "uri": "u"}),
                ),
                call(
                    "c5",
                    "read_mcp_resource",
                    serde_json::json!({"server": "ghost", "uri": "u"}),
                ),
                call("c6", "read_mcp_resource", serde_json::json!({})),
            ]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let expected_notes = [
        "缺少必填参数 `server`",
        "缺少必填参数 `uri`",
        "`uri` 必须是字符串",
        "`server` 必须是字符串",
        "未知或未启用的资源服务器：ghost",
        "缺少必填参数 `server`",
    ];
    for (id, note) in ["c1", "c2", "c3", "c4", "c5", "c6"].iter().zip(expected_notes) {
        let result = tool_result(&history, id);
        assert_eq!(result["isError"], true, "{id}：{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(note), "{id} 应含「{note}」：{text}");
    }
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record.iter().all(|e| e["kind"] != "read_resource"),
        "参数错误零外部读取：{record:?}"
    );
}

#[test]
fn m2_memory_merge_turn_never_executes_resource_tools() {
    // 内部记忆轮不声明、不执行固定资源工具：伪造调用走未知工具通道，
    // 服务器零请求。
    let ctx = setup("resources_only", "");
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut iter = vec![
        tc_response(&[call(
            "m1",
            "read_mcp_resource",
            serde_json::json!({"server": "fx", "uri": "tchat://guide/hello"}),
        )]),
        fin("合并完成"),
    ]
    .into_iter();
    let mut sender = |_body: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::from_str(&iter.next().unwrap()).unwrap())
    };
    let mut notify = |_n: TurnNotify| {};
    let root = TempDir::new("mcp-res-merge-root");
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
            send_message: false,
            steer: None, context: None,
            permissions: None,
            orchestration: None,
        },
    );
    assert!(reply.is_ok(), "{reply:?}");
    let _ = mem_config;
    let forged = tool_result(&history, "m1");
    assert!(
        forged["error"].as_str().unwrap_or_default().contains("未知工具"),
        "伪造资源调用走未知工具通道：{forged}"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record.iter().all(|e| e["kind"] != "read_resource"),
        "记忆轮零资源请求：{record:?}"
    );
}

#[test]
fn m3_directed_pagination_roundtrips_cursor_including_whitespace() {
    let ctx = setup("resources_paged", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[
                call(
                    "c1",
                    "list_mcp_resources",
                    serde_json::json!({"server": "fx"}),
                ),
                call(
                    "c2",
                    "list_mcp_resources",
                    serde_json::json!({"server": "fx", "cursor": "page-2"}),
                ),
            ]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    // 第一页：nextCursor 原样回传给模型。
    let page1 = tool_result(&history, "c1");
    assert_eq!(page1["nextCursor"], "page-2", "{page1}");
    assert_eq!(page1["resources"][0]["uri"], "tchat://p/one");
    // 第二页：带 cursor 定向取，结束无 nextCursor。
    let page2 = tool_result(&history, "c2");
    assert_eq!(page2["resources"][0]["uri"], "tchat://p/two");
    assert!(page2.get("nextCursor").is_none(), "{page2}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record.iter().filter(|e| e["kind"] == "list_resources").collect();
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[1]["params"]["cursor"], "page-2", "游标原样送达");
}

#[test]
fn m3_cursor_with_whitespace_is_passed_verbatim() {
    let ctx = setup("resources_cursor_space", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[
                call(
                    "c1",
                    "list_mcp_resources",
                    serde_json::json!({"server": "fx"}),
                ),
                call(
                    "c2",
                    "list_mcp_resources",
                    serde_json::json!({"server": "fx", "cursor": " spaced "}),
                ),
            ]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let page1 = tool_result(&history, "c1");
    assert_eq!(page1["nextCursor"], " spaced ", "不 trim：{page1}");
    let page2 = tool_result(&history, "c2");
    assert_eq!(page2["resources"][0]["uri"], "tchat://s/second", "{page2}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let second = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .nth(1)
        .unwrap();
    assert_eq!(second["params"]["cursor"], " spaced ", "空白游标原样送达");
}

#[test]
fn m3_aggregate_pages_all_servers_in_stable_order_with_server_ownership() {
    let ctx = common::mcp_fixtures::setup_two("resources_paged", "resources_only", "", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "list_mcp_resources",
                serde_json::json!({}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert!(result.get("isError").is_none(), "汇总成功不是错误：{result}");
    let resources = result["resources"].as_array().expect("资源数组");
    // fx（两页完整分页 2 项）在前、fy（单页 2 项）在后：按服务器 ID 稳定排序。
    assert_eq!(resources.len(), 4, "{result}");
    assert_eq!(resources[0]["server"], "fx");
    assert_eq!(resources[0]["uri"], "tchat://p/one");
    assert_eq!(resources[1]["server"], "fx");
    assert_eq!(resources[1]["uri"], "tchat://p/two");
    assert_eq!(resources[2]["server"], "fy");
    assert_eq!(resources[2]["uri"], "tchat://guide/hello");
    assert_eq!(resources[3]["server"], "fy");
    assert!(
        result.get("errors").is_none(),
        "全部成功时省略 errors：{result}"
    );
    assert!(
        result.get("nextCursor").is_none(),
        "汇总完整分页后无游标：{result}"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(lists.len(), 3, "fx 两页＋fy 一页：{record:?}");
}

#[test]
fn m3_aggregate_partial_failure_keeps_healthy_servers_with_errors() {
    let ctx = common::mcp_fixtures::setup_two("resources_paged", "resources_error", "", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert!(
        result.get("isError").is_none(),
        "部分失败是数据不是工具失败：{result}"
    );
    let resources = result["resources"].as_array().unwrap();
    assert!(
        resources.iter().all(|r| r["server"] == "fx"),
        "仅保留成功台完整数据：{result}"
    );
    assert_eq!(resources.len(), 2, "{result}");
    let errors = result["errors"].as_array().expect("errors 数组");
    assert_eq!(errors.len(), 1, "{result}");
    assert_eq!(errors[0]["server"], "fy");
    let reason = errors[0]["error"].as_str().unwrap();
    assert!(!reason.is_empty(), "脱敏原因非空");
}

#[test]
fn m3_aggregate_all_servers_failed_returns_empty_with_errors() {
    // R3=A：全部参与台可归属单台失败 → 空集合＋完整 errors（非整次失败）。
    let ctx = common::mcp_fixtures::setup_two("resources_error", "resources_error", "", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert!(result.get("isError").is_none(), "R3：不是整次失败：{result}");
    assert_eq!(result["resources"].as_array().unwrap().len(), 0);
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "每台一条：{result}");
    assert_eq!(errors[0]["server"], "fx");
    assert_eq!(errors[1]["server"], "fy");
}

#[test]
fn m3_aggregate_paginates_through_empty_string_cursor() {
    let ctx = setup("resources_empty_cursor", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    let resources = result["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 2, "空字符串游标翻到第二页：{result}");
    assert_eq!(resources[0]["uri"], "tchat://e/first");
    assert_eq!(resources[1]["uri"], "tchat://e/second");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(lists.len(), 2);
    assert_eq!(lists[1]["params"]["cursor"], "", "空游标原样传回翻页");
}

#[test]
fn m3_aggregate_respects_single_overall_deadline() {
    // R1=A：单一整体时限＝参与台 tool_timeout_sec 最大值；两台均 1 秒，
    // fx 挂起 → 整次失败，fy（串行遍历未到达）零请求，已收集数据不发布。
    let ctx = common::mcp_fixtures::setup_two(
        "resources_hang_list",
        "resources_only",
        r#", "tool_timeout_sec": 1"#,
        r#", "tool_timeout_sec": 1"#,
    );
    let started = std::time::Instant::now();
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 30,
        "整次失败受整体时限约束：{elapsed:?}"
    );
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "整次失败：{result}");
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("整体时限"), "应报告整体时限：{text}");
    assert!(
        result["resources"].is_null(),
        "整次失败不发布已收集数据"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(lists.len(), 1, "仅挂起台收到一次请求：{record:?}");
}

#[test]
fn m3_aggregate_deadline_window_is_shared_max_not_per_server() {
    // R1=A 组合精度补证：跨台窗口＝max(1s,4s)=4s。fx（自身 1s）挂起时，
    // 其请求被共享整体窗口覆盖到 4s 才整次失败——文案数值区分 max 与
    // 求和（5s），经时下界（≥2s）与 fy 零请求区分 max 与每台重新计时
    //（后者会在 fx 自身 1s 失败后继续查询 fy 并返回部分结果）。
    let ctx = common::mcp_fixtures::setup_two(
        "resources_hang_list",
        "resources_only",
        r#", "tool_timeout_sec": 1"#,
        r#", "tool_timeout_sec": 4"#,
    );
    let started = std::time::Instant::now();
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let elapsed = started.elapsed();
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "整次失败：{result}");
    let text = serde_json::to_string(&result).unwrap();
    assert!(
        text.contains("整体时限 4s"),
        "窗口取参与台最大值 max(1s,4s)，非求和/单台值：{text}"
    );
    assert!(
        elapsed >= std::time::Duration::from_secs(2),
        "fx 自身 1s 超时被共享窗口覆盖（每台重计时约 1s 即结束）：{elapsed:?}"
    );
    assert!(
        result["resources"].is_null(),
        "整次失败不发布已收集数据"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(lists.len(), 1, "仅挂起台收到一次请求，fy 零请求：{record:?}");
}

#[test]
fn m3_page_limits_reject_the_server_not_the_whole_call() {
    // 单台分页预算超限 → 该台记 errors、丢弃半份目录，不是整次失败。
    for (scenario, note) in [
        ("resources_101", "100 页"),
        ("resources_loop", "重复游标"),
        ("resources_cursor_64k", "64 KiB"),
        ("resources_items_paged_over", "2048 项"),
    ] {
        let ctx = setup(scenario, "");
        let (reply, history, _bodies) = run_resource_turn(
            &ctx,
            vec![
                tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
                fin("完成"),
            ],
        );
        assert!(reply.is_ok(), "{scenario}：{reply:?}");
        let result = tool_result(&history, "c1");
        assert!(
            result.get("isError").is_none(),
            "{scenario}：单台预算超限不是整次失败：{result}"
        );
        let resources = result["resources"].as_array().unwrap();
        assert!(
            resources.is_empty(),
            "{scenario}：失败台不输出半份目录：{result}"
        );
        let errors = result["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{scenario}");
        assert_eq!(errors[0]["server"], "fx");
        let reason = errors[0]["error"].as_str().unwrap();
        assert!(reason.contains(note), "{scenario} 应含「{note}」：{reason}");
        ctx.close();
    }
}

#[test]
fn m3_aggregate_budget_over_8mib_fails_wholesale() {
    // R2=B：单台 3 页 ×3MiB＝9MiB 超过 8 MiB 结果预算 → 整次失败。
    let ctx = setup("resources_big", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "预算超限整次失败：{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("预算"), "应报告预算超限：{text}");
    assert!(
        result["resources"].is_null(),
        "不发布已收集数据：{result}"
    );
}

#[test]
fn m3_directed_single_page_over_2048_items_fails() {
    let ctx = setup("resources_items_over", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "list_mcp_resources",
                serde_json::json!({"server": "fx"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("2048"), "单页上限：{text}");
}

#[test]
fn m4_cancel_before_dispatch_skips_request() {
    // 发送前取消＝该工具零外部请求（006 语义，资源工具同路径）。取消在
    // 模型返回工具批之后、工具执行之前置位（轮开始会复位取消标志）。
    let ctx = setup("resources_only", "");
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_flag = cancel.clone();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut iter = vec![
        tc_response(&[call(
            "c1",
            "read_mcp_resource",
            serde_json::json!({"server": "fx", "uri": "tchat://guide/hello"}),
        )]),
        fin("完成"),
    ]
    .into_iter();
    let mut sender = |_body: &serde_json::Value| -> Result<serde_json::Value, String> {
        // 第一个模型响应（工具批）返回后立即取消。
        let resp = iter.next().unwrap();
        cancel_flag.store(true, Ordering::SeqCst);
        Ok(serde_json::from_str(&resp).unwrap())
    };
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "取消轮",
        &mut sender,
        &mut |_| {},
        Some(cancel.as_ref()),
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
    // 006 取消语义：本轮以取消 Err 结束（已执行结果保留、剩余调用未执行）。
    assert!(
        reply.is_err(),
        "取消轮应返回取消错误：{reply:?}"
    );
    assert!(reply
        .as_ref()
        .err()
        .is_some_and(|e| e.contains("取消")));
    let skipped = tool_result(&history, "c1");
    assert!(
        skipped.get("error").is_some() || skipped.get("isError") == Some(&serde_json::json!(true)),
        "取消跳过按失败补齐：{skipped}"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record.iter().all(|e| e["kind"] != "read_resource"),
        "取消后零资源请求：{record:?}"
    );
}

#[test]
fn m4_cancel_during_aggregate_fails_wholesale_and_keeps_batch_pairing() {
    // 汇总进行中取消：整次失败（不发布半份成功集），串行遍历未到达的
    // fy 零请求；补齐配对后本轮结束。
    let ctx = common::mcp_fixtures::setup_two(
        "resources_hang_list",
        "resources_only",
        r#", "tool_timeout_sec": 30"#,
        "",
    );
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_wait = cancel.clone();
    let record_path = ctx.record_path.clone();
    // 屏障线程：等 fx 的 resources/list 到达后置取消。
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            let text = std::fs::read_to_string(&record_path).unwrap_or_default();
            if text.contains("list_resources") {
                cancel_for_wait.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut iter = vec![
        tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
        fin("完成"),
    ]
    .into_iter();
    let mut sender = |_body: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::from_str(&iter.next().unwrap()).unwrap())
    };
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "汇总取消",
        &mut sender,
        &mut |_| {},
        Some(cancel.as_ref()),
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
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "取消整次失败：{result}");
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("取消"), "应报告取消：{text}");
    assert!(result["resources"].is_null(), "不发布已收集数据");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(lists.len(), 1, "仅挂起台一次请求，fy 未被请求：{record:?}");
}

#[test]
fn m4_directed_list_timeout_reports_unknown_state() {
    // 定向单次请求超时：报「远端执行状态未知」，不自动重试（恰一次请求）。
    let ctx = setup_with(
        "resources_hang_list",
        r#", "tool_timeout_sec": 1"#,
        0,
    );
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "list_mcp_resources",
                serde_json::json!({"server": "fx"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("超时"), "应报告超时：{text}");
    assert!(text.contains("未知"), "远端状态未知：{text}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(lists.len(), 1, "不自动重试：{record:?}");
}

#[test]
fn m4_disconnect_after_read_removes_capability_until_reload() {
    // 读取成功后服务器进程退出：该台不可用，下一轮不再声明资源工具；
    // 后续调用返回不可用错误且零外部请求，仅显式 reload 可恢复。
    let ctx = setup("resources_die_after_read", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "read_mcp_resource",
                serde_json::json!({"server": "fx", "uri": "tchat://guide/hello"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let ok = tool_result(&history, "c1");
    assert!(ok.get("isError").is_none(), "首次读取成功：{ok}");
    // 等服务器进程退出传播到连接状态。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let snapshot2 = loop {
        let snapshot = ctx
            .manager
            .prepare_for_turn(&AtomicBool::new(false))
            .expect("准备不取消");
        if !snapshot.has_resources() || std::time::Instant::now() > deadline {
            break snapshot;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(
        !snapshot2.has_resources(),
        "断连后不再声明资源工具"
    );
    let lines = ctx.manager.status_lines().join("\n");
    assert!(lines.contains("不可用"), "状态显示不可用：{lines}");
    // 断连台上的调用返回不可用错误，零外部请求。
    let (content, failed) = snapshot2.call_resource_tool(
        "read_mcp_resource",
        r#"{"server":"fx","uri":"tchat://guide/hello"}"#,
        &AtomicBool::new(false),
    );
    assert!(failed, "断连台调用失败：{content}");
    let record = read_record(&ctx.record_path);
    let reads: Vec<_> = record.iter().filter(|e| e["kind"] == "read_resource").collect();
    assert_eq!(reads.len(), 1, "断连后零新增请求：{record:?}");
}

#[test]
fn m4_late_result_after_timeout_is_discarded_and_not_replayed() {
    // 1s 超时＋1.5s 迟到回复：恰一条超时结果；迟到结果不追加第二条、
    // 不入下一请求；连接未被超时关闭，下一轮可正常使用。
    let ctx = setup_with(
        "resources_slow_read",
        r#", "tool_timeout_sec": 1"#,
        0,
    );
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "read_mcp_resource",
                serde_json::json!({"server": "fx", "uri": "tchat://guide/slow"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "超时失败：{result}");
    // 等迟到回复写完（服务器 1.5s 后发送）。
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let record = read_record(&ctx.record_path);
        if record.iter().any(|e| e["kind"] == "read_resource")
            || std::time::Instant::now() > deadline
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // 历史中 c1 仍只有一条结果（超时），没有第二条迟到结果。
    let count = history
        .iter()
        .filter(|h| matches!(h, HistoryItem::ToolResult { tool_call_id, .. } if tool_call_id == "c1"))
        .count();
    assert_eq!(count, 1, "迟到结果不追加第二条");
    // 连接保留：下一轮再读同 URI 正常返回（不重放旧请求）。
    let (reply2, history2, _b2) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c2",
                "read_mcp_resource",
                serde_json::json!({"server": "fx", "uri": "tchat://guide/config"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply2.is_ok(), "超时不关连接：{reply2:?}");
    let again = tool_result(&history2, "c2");
    assert!(again.get("isError").is_none(), "下一轮正常：{again}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let reads: Vec<_> = record.iter().filter(|e| e["kind"] == "read_resource").collect();
    assert_eq!(reads.len(), 2, "两次读取各恰一次（无重放）：{record:?}");
}

#[test]
fn m5_mcp_status_distinguishes_tools_and_resources_capability() {
    // 仅资源服务器：状态行区分能力。
    // 各段用独立作用域：EnvGuard 随 TestCtx drop 释放，串行 setup。
    {
        let ctx = setup("resources_only", "");
        let lines = ctx.manager.status_lines().join("\n");
        assert!(
            lines.contains("资源 可用"),
            "仅资源服务器显示资源能力：{lines}"
        );
        assert!(
            !lines.contains("工具 1 个"),
            "仅资源服务器不显示工具计数为 1：{lines}"
        );
        ctx.close();
    }
    // 两者兼有：工具与资源都显示。
    {
        let both = setup("tools_and_resources", "");
        let lines = both.manager.status_lines().join("\n");
        assert!(
            lines.contains("资源 可用"),
            "兼有服务器显示资源能力：{lines}"
        );
        assert!(
            lines.contains("工具 1 个"),
            "兼有服务器显示工具计数：{lines}"
        );
        both.close();
    }
    // 仅工具（022 既有）：不显示资源。
    {
        let tools_only = setup("echo", "");
        let lines = tools_only.manager.status_lines().join("\n");
        assert!(
            !lines.contains("资源 可用"),
            "仅工具服务器不显示资源：{lines}"
        );
        tools_only.close();
    }
}

#[test]
fn m6_template_aggregate_collects_with_ownership() {
    // 模板汇总（省略 server）与资源汇总同构：完整收集＋服务器归属＋
    // 稳定顺序，覆盖 collect 的 templates 分支。
    let ctx = common::mcp_fixtures::setup_two("resources_only", "resources_only", "", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "list_mcp_resource_templates",
                serde_json::json!({}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    let templates = result["resourceTemplates"].as_array().expect("模板数组");
    assert_eq!(templates.len(), 4, "两台各 2 模板：{result}");
    assert!(
        templates.iter().take(2).all(|t| t["server"] == "fx"),
        "fx 在前：{result}"
    );
    assert!(
        templates.iter().skip(2).all(|t| t["server"] == "fy"),
        "fy 在后：{result}"
    );
    assert_eq!(templates[0]["uriTemplate"], "tchat://guide/notes/{id}");
}

#[test]
fn m6_same_uri_routes_only_to_specified_server() {
    // 同 URI 在不同服务器不串路由：(fx, uri) 只抵达 fx。
    let ctx = common::mcp_fixtures::setup_two("resources_only", "resources_only", "", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "read_mcp_resource",
                serde_json::json!({"server": "fx", "uri": "tchat://guide/hello"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["server"], "fx", "{result}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let reads: Vec<_> = record.iter().filter(|e| e["kind"] == "read_resource").collect();
    assert_eq!(reads.len(), 1, "恰一次读取：{record:?}");
}

#[test]
fn m6_polluted_aggregate_fails_wholesale() {
    // 汇总中任一台污染 → 整次失败（不发布任何台数据，包括干净台）。
    std::env::set_var(common::mcp_fixtures::CANARY, common::mcp_fixtures::CANARY_VALUE);
    let ctx = common::mcp_fixtures::setup_two(
        "resources_only",
        "resources_polluted",
        "",
        r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#,
    );
    let (reply, history, bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "污染汇总整次失败：{result}");
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("敏感数据"), "{text}");
    assert!(!text.contains(common::mcp_fixtures::CANARY_VALUE));
    assert!(
        result["resources"].is_null(),
        "干净台数据也不发布：{result}"
    );
    let all = format!(
        "{}{}",
        serde_json::to_string(&history).unwrap(),
        serde_json::to_string(&bodies).unwrap()
    );
    assert!(!all.contains(common::mcp_fixtures::CANARY_VALUE), "凭证不入历史/请求");
}

#[test]
fn m6_cancel_outranks_partial_failure_with_collected_data() {
    // 取消优先于部分失败：已成功收集 fx 数据后，fy 期间取消 → 整次失败，
    // fx 已收集数据不发布；fy 恰一次请求。
    let ctx = common::mcp_fixtures::setup_two(
        "resources_only",
        "resources_hang_list",
        "",
        r#", "tool_timeout_sec": 30"#,
    );
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_flag = cancel.clone();
    let record_path = ctx.record_path.clone();
    std::thread::spawn(move || {
        // 等 fy 的请求到达（fx 数据已成功收集后再取消）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            let text = std::fs::read_to_string(&record_path).unwrap_or_default();
            if text.matches("list_resources").count() >= 2 {
                cancel_flag.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut iter = vec![
        tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
        fin("完成"),
    ]
    .into_iter();
    let mut sender = |_body: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::from_str(&iter.next().unwrap()).unwrap())
    };
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "取消优先",
        &mut sender,
        &mut |_| {},
        Some(cancel.as_ref()),
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
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "取消整次失败：{result}");
    assert!(
        result["resources"].is_null(),
        "已收集的 fx 数据不发布：{result}"
    );
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("取消"), "{text}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(lists.len(), 2, "fx 一次＋fy 一次（挂起后取消）：{record:?}");
}

#[test]
fn m1_directed_list_passes_cursor_verbatim_and_reports_unknown_server() {
    let ctx = setup("resources_only", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[
                call(
                    "c1",
                    "list_mcp_resources",
                    serde_json::json!({"server": "fx", "cursor": ""}),
                ),
                call(
                    "c2",
                    "list_mcp_resources",
                    serde_json::json!({"server": "ghost"}),
                ),
                call(
                    "c3",
                    "list_mcp_resources",
                    serde_json::json!({"cursor": "x"}),
                ),
            ]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "失败按工具失败回传，轮继续：{reply:?}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    // 空字符串游标原样传递（opaque；不 trim、不当结束）。
    let list_entries: Vec<_> = record
        .iter()
        .filter(|e| e["kind"] == "list_resources")
        .collect();
    assert_eq!(list_entries.len(), 1, "仅 c1 真实发出：{record:?}");
    assert_eq!(
        list_entries[0]["params"]["cursor"], "",
        "空字符串游标原样送达：{list_entry:?}",
        list_entry = list_entries[0]
    );
    // 未知服务器与裸 cursor 是参数错误，零外部请求。
    let unknown = tool_result(&history, "c2");
    assert_eq!(unknown["isError"], true, "{unknown}");
    assert!(unknown["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("未知或未启用的资源服务器"));
    let bare_cursor = tool_result(&history, "c3");
    assert_eq!(bare_cursor["isError"], true, "{bare_cursor}");
    assert!(bare_cursor["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("cursor` 必须与 `server` 同时使用"));
}

// ---------- 023 M6-2：资源面安全实测（受控本地夹具） ----------

#[test]
fn sec_disabled_server_resource_calls_fail_with_zero_requests() {
    // 停用绕过：enabled:false 的服务器不连接、不进资源快照；对它的资源
    // 调用按未知服务器失败，零外部请求。
    let guard = common::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-sec-disabled");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    let fixture = common::mcp_fixtures::FIXTURE.replace('\\', "/");
    let record = dir.path().join("record.jsonl");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": false, "command": "python3", "args": ["{fixture}", "--scenario", "resources_only", "--record", "{}"], "cwd": "{}"}}}}}}"#,
        record.display(),
        std::env::temp_dir().display()
    );
    std::fs::write(dir.path().join("mcp.json"), json).unwrap();
    let manager = terminal_chat::mcp::manager::McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    assert!(!snapshot.has_resources(), "停用服务器不声明资源工具");
    let (content, failed) = snapshot.call_resource_tool(
        "read_mcp_resource",
        r#"{"server":"fx","uri":"tchat://guide/hello"}"#,
        &AtomicBool::new(false),
    );
    assert!(failed, "停用台调用失败：{content}");
    // 无资源服务器时工具不在声明面 → 防御分支「未知资源工具」；同样零请求。
    assert!(content.contains("未知"), "按未知拒绝：{content}");
    manager.close();
    drop(guard);
    assert!(
        !record.exists(),
        "停用服务器零进程启动零请求（无 record 文件）"
    );
}

#[test]
fn sec_uri_is_never_opened_locally_only_forwarded() {
    // URI 被本地打开：file:///etc/passwd 与远程 URL 均原样交服务器解释，
    // 宿主不读文件、不发 HTTP——夹具回什么就是什么。
    let ctx = setup("resources_only", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "read_mcp_resource",
                serde_json::json!({"server": "fx", "uri": "file:///etc/passwd"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    // 夹具按默认分支返回文本（而非本地文件内容）：证明 URI 只到达服务器。
    assert_eq!(
        result["contents"][0]["text"],
        "content of file:///etc/passwd",
        "URI 由服务器解释：{result}"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    let entry = record
        .iter()
        .find(|e| e["kind"] == "read_resource")
        .expect("读取请求抵达服务器");
    assert_eq!(entry["params"]["uri"], "file:///etc/passwd", "URI 原样转发");
}

#[test]
fn sec_directed_polluted_list_is_rejected() {
    // 定向列举污染：资源描述回显 canary → 整份拒绝，canary 不入历史。
    std::env::set_var(common::mcp_fixtures::CANARY, common::mcp_fixtures::CANARY_VALUE);
    let ctx = setup(
        "resources_polluted",
        r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#,
    );
    let (reply, history, bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call(
                "c1",
                "list_mcp_resources",
                serde_json::json!({"server": "fx"}),
            )]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "{reply:?}");
    let result = tool_result(&history, "c1");
    assert_eq!(result["isError"], true, "定向污染拒绝：{result}");
    let all = format!(
        "{}{}",
        serde_json::to_string(&history).unwrap(),
        serde_json::to_string(&bodies).unwrap()
    );
    assert!(!all.contains(common::mcp_fixtures::CANARY_VALUE));
}

#[test]
fn sec_malformed_list_response_fails_as_protocol_error() {
    // 畸形分页：resources 非数组 → 客户端协议错误（不崩溃、不伪造成功），
    // 该台按部分失败记 errors（汇总）。
    let ctx = setup("resources_malformed", "");
    let (reply, history, _bodies) = run_resource_turn(
        &ctx,
        vec![
            tc_response(&[call("c1", "list_mcp_resources", serde_json::json!({}))]),
            fin("完成"),
        ],
    );
    assert!(reply.is_ok(), "畸形响应不拖垮本轮：{reply:?}");
    let result = tool_result(&history, "c1");
    assert!(
        result.get("isError").is_none(),
        "单台畸形按部分失败（R3）：{result}"
    );
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(
        !errors[0]["error"].as_str().unwrap().is_empty(),
        "脱敏错误原因非空"
    );
}

#[test]
fn sec_oversize_read_is_rejected_by_transport_bound() {
    // 超大输入：单条读取结果 9 MiB > 8 MiB 传输上限 → 有界接收拒绝并关闭
    // 连接，不截断成成功。
    let ctx = setup("resources_only", "");
    let (content, failed) = ctx.snapshot.call_resource_tool(
        "read_mcp_resource",
        r#"{"server":"fx","uri":"tchat://guide/huge"}"#,
        &AtomicBool::new(false),
    );
    assert!(failed, "超限读取失败：{content}");
    assert!(content.contains("MCP 客户端错误"), "标准客户端错误：{content}");
    assert!(!content.contains("xxxx"), "不截断成部分成功");
}

// ---------- M6-3 审查修复的回归用例 ----------

#[test]
fn review_list_pollution_catches_escaped_secret() {
    // Spec 审查：敏感值含引号/反斜杠时，仅扫 JSON 序列化字符串会因转义
    // 漏检——列举/模板项须逐字段检查原文。
    let tricky = "canary\"quote\\slash值";
    let guard = common::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-review-polluted");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(common::mcp_fixtures::CANARY, tricky);
    let fixture = common::mcp_fixtures::FIXTURE.replace('\\', "/");
    let record = dir.path().join("record.jsonl");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "resources_polluted", "--record", "{rec}"], "cwd": "{tmp}", "env_vars": ["{canary}"]}}}}}}"#,
        rec = record.display(),
        tmp = std::env::temp_dir().display(),
        canary = common::mcp_fixtures::CANARY,
    );
    std::fs::write(dir.path().join("mcp.json"), json).unwrap();
    let manager = terminal_chat::mcp::manager::McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let (content, failed) = snapshot.call_resource_tool(
        "list_mcp_resources",
        r#"{"server":"fx"}"#,
        &AtomicBool::new(false),
    );
    manager.close();
    drop(guard);
    assert!(failed, "转义形态的凭证也必须被拒绝：{content}");
    assert!(
        !content.contains(tricky),
        "错误形态不回显凭证：{content}"
    );
    assert!(!content.contains("canary"), "转义后的片段也不出现：{content}");
}

#[test]
fn review_execution_uses_snapshot_so_disconnected_server_reports_unavailable() {
    // Spec 审查：执行共用快照——轮内断连台仍在快照中，定向调用按快照路由
    // 到该台并返回不可用错误（连接已关闭），不得静默剔除成「未知服务器」。
    let ctx = setup("resources_die_after_read", "");
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let (content, failed) = snapshot.call_resource_tool(
        "read_mcp_resource",
        r#"{"server":"fx","uri":"tchat://guide/hello"}"#,
        &AtomicBool::new(false),
    );
    assert!(!failed, "首次读取成功：{content}");
    // 等服务器进程退出传播到连接关闭标志。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let (c2, f2) = snapshot.call_resource_tool(
            "read_mcp_resource",
            r#"{"server":"fx","uri":"tchat://guide/config"}"#,
            &AtomicBool::new(false),
        );
        if f2 {
            // 核心回归：断连台按快照路由失败（未发送/状态未知/连接关闭
            // 均为正确传输失败形态），不得静默剔除成「未知或未启用」。
            assert!(
                !c2.contains("未知或未启用"),
                "断连台不得被静默剔除：{c2}"
            );
            assert!(
                c2.contains("MCP 客户端错误"),
                "按快照命中该台并以客户端错误失败：{c2}"
            );
            ctx.close();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("10 秒内连接未进入关闭状态");
}
