//! 006 工具调用循环 E2E 验收（十三类）：管道驱动真实 CLI（CARGO_BIN_EXE）＋
//! 本地 mock 返回预设响应序列并捕获每个请求体；真实执行两工具于隔离临时目录。
//! 核对实际请求 JSON（消息序/角色/id 对应/内容）、工具输出、文件字节、执行顺序与历史。

mod common;

use common::{spawn_mock, MockResp, TempDir};
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use terminal_chat::catalog;

fn write_manual(dir: &Path, base_url: &str, key_ref: &str) {
    let m = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(base_url.into()),
                key_ref: Some(key_ref.into()),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(dir.join("manual.json"), serde_json::to_string_pretty(&m).unwrap())
        .unwrap();
}

fn seed_grant(user_dir: &Path, origin: &str, cred_fp: &str) {
    std::fs::write(
        user_dir.join("authorized-origins.json"),
        json!({ "grants": [{ "origin": origin, "cred_fp": cred_fp, "added_at": 0 }] })
            .to_string(),
    )
    .unwrap();
}

/// 管道运行真实 CLI：预授权（非交互直接放行）、隔离环境与 cwd。
fn run_cli(project: &Path, mock_url: &str, key: &str, stdin_text: &str) -> (String, Option<i32>) {
    let cfg = project.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let user = project.join("user");
    std::fs::create_dir_all(&user).unwrap();
    write_manual(&cfg, mock_url, "env:TCHAT_TL_KEY");
    seed_grant(&user, &catalog::origin_of(mock_url).unwrap(), &catalog::fingerprint(key));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.current_dir(project)
        .env("TCHAT_CONFIG_DIR", &cfg)
        .env("TCHAT_USER_DIR", &user)
        .env("TCHAT_TL_KEY", key)
        .env("TERM", "xterm");
    for k in ["TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
        cmd.env_remove(k);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("CLI 子进程启动失败");
    use std::io::Write;
    child.stdin.take().unwrap().write_all(stdin_text.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("等待 CLI 子进程失败");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code())
}

fn ok_response(content: &str) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

/// 带工具调用的响应；content 为 None 时为 null，Some 时保留原始字符串。
fn tc_response(calls: &[Value], content: Option<&str>) -> String {
    let tool_calls: Vec<Value> = calls.to_vec();
    let message = match content {
        Some(c) => json!({ "role": "assistant", "content": c, "tool_calls": tool_calls }),
        None => json!({ "role": "assistant", "content": null, "tool_calls": tool_calls }),
    };
    json!({ "choices": [{ "message": message, "finish_reason": "tool_calls" }] }).to_string()
}

fn call(id: &str, name: &str, arguments: String) -> Value {
    json!({ "id": id, "type": "function", "function": { "name": name, "arguments": arguments } })
}

fn add_file_patch(path: &Path, content: &str) -> String {
    format!("*** Begin Patch\n*** Add File: {}\n+{}\n*** End Patch", path.display(), content)
}

fn req_body(mock: &common::Mock, i: usize) -> Value {
    let reqs = mock.requests();
    assert!(reqs.len() > i, "请求不足：{} <= {i}", reqs.len());
    serde_json::from_str(&reqs[i].body).unwrap()
}

fn role_seq(body: &Value) -> Vec<&str> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect()
}

fn count_role(body: &Value, role: &str) -> usize {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!(role))
        .count()
}

fn tool_msgs(body: &Value) -> Vec<&Value> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("tool"))
        .collect()
}

/// T1：普通回复直接结束——恰 1 请求，含 system/user 与两项工具定义。
#[test]
fn t1_plain_reply_ends_immediately() {
    let dir = TempDir::new("tl-t1");
    let mock = spawn_mock(vec![MockResp::json(200, ok_response("直接回复-T1"))]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "你好\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Agent：直接回复-T1"), "{out}");
    assert_eq!(mock.count(), 1, "普通回复必须恰一次请求");
    let body = req_body(&mock, 0);
    assert_eq!(body["model"], json!("deepseek-flash"));
    let roles = role_seq(&body);
    // 015：ctx（环境块）先于本轮输入
    assert_eq!(roles, vec!["system", "user", "user"], "消息序 system→ctx→user");
    assert!(body["messages"][1]["content"].as_str().unwrap().starts_with("<environment_context>"));
    assert_eq!(body["messages"][2]["content"], json!("你好"));
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4, "017：用户轮四工具");
    assert_eq!(tools[0]["function"]["name"], json!("exec_command"));
    assert_eq!(tools[1]["function"]["name"], json!("apply_patch"));
    assert_eq!(tools[2]["function"]["name"], json!("write_stdin"));
}

#[test]
fn t19_pipeline_does_not_declare_questions_or_consume_chat_as_answers() {
    let dir = TempDir::new("tl-t19");
    let args=json!({"questions":[{"id":"q","header":"方向","question":"选择？","options":[{"label":"A","description":"默认"}]}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(
            200,
            tc_response(&[call("forged", "request_user_input", args)], None),
        ),
        MockResp::json(200, ok_response("无回答接口")),
        MockResp::json(200, ok_response("普通第二轮")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "fake-t19", "第一次\n选项 A\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 3);
    for i in 0..3 {
        assert_eq!(req_body(&mock, i)["tools"].as_array().unwrap().len(), 4);
    }
    let result: Value = serde_json::from_str(
        tool_msgs(&req_body(&mock, 1))[0]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(result["error"].as_str().unwrap().contains("没有回答接口"));
    let body = req_body(&mock, 2);
    let user = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .unwrap();
    assert_eq!(user["content"], "选项 A");
    assert!(out.contains("普通第二轮"));
}

/// T2：创建文件——真实落盘字节正确；第 2 请求含原样 assistant(tool_calls)＋匹配 id 的 role=tool。
#[test]
fn t2_apply_patch_creates_file_and_echoes_result() {
    let dir = TempDir::new("tl-t2");
    let target = dir.path().join("made.txt");
    let patch = add_file_patch(&target, "T2-CONTENT");
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(
            &[call("call-create01", "apply_patch", json!({ "patch": patch }).to_string())],
            None,
        )),
        MockResp::json(200, ok_response("文件已创建")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "创建文件\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(std::fs::read(&target).unwrap(), b"T2-CONTENT\n".to_vec(), "文件字节");
    assert_eq!(mock.count(), 2, "工具轮后必须再请求一次");
    let body = req_body(&mock, 1);
    assert_eq!(role_seq(&body), vec!["system", "user", "user", "assistant", "tool"]);
    assert_eq!(count_role(&body, "user"), 2, "015 ctx＋本轮 user");
    assert_eq!(count_role(&body, "system"), 1, "system 每请求恰一条");
    let assistant = &body["messages"][3];
    assert_eq!(assistant["content"], json!(null), "content null 原样保留");
    assert_eq!(assistant["tool_calls"][0]["id"], json!("call-create01"));
    assert_eq!(assistant["tool_calls"][0]["function"]["name"], json!("apply_patch"));
    let tool = &body["messages"][4];
    assert_eq!(tool["tool_call_id"], json!("call-create01"), "tool_call_id 必须匹配");
    let result: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["ok"], json!(true), "ApplyPatchResult JSON：{result}");
    assert_eq!(result["changed_files"][0]["action"], json!("add"));
    // 单行工具活动（id 按字符前 8 位）＋最终回复
    assert!(out.contains("[工具] apply_patch（call-cre"), "{out}");
    assert!(out.contains("Agent：文件已创建"), "{out}");
}

/// T2b（029 M4）：Move 补丁经真实分派——真实移动落盘；第 2 请求回传**有序两条
/// 记录**（目标 add、源 delete）的四字段结果；工具名/请求次数/id 契约不变。
#[test]
fn t2b_apply_patch_move_returns_ordered_two_records() {
    let dir = TempDir::new("tl-t2b");
    let src = dir.path().join("old-t2b.txt");
    let dst = dir.path().join("new-t2b.txt");
    std::fs::write(&src, "A\nB\n").unwrap();
    let patch = format!(
        "*** Begin Patch\n*** Update File: {s}\n*** Move to: {d}\n@@\n-B\n+B2\n*** End Patch",
        s = src.display(),
        d = dst.display()
    );
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(
            &[call("call-move01", "apply_patch", json!({ "patch": patch }).to_string())],
            None,
        )),
        MockResp::json(200, ok_response("已移动")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "移动文件\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(!src.exists(), "源已删除：{out}");
    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "A\nB2\n", "目标含修改后内容");
    assert_eq!(mock.count(), 2, "工具轮后必须再请求一次");
    let body = req_body(&mock, 1);
    assert_eq!(body["messages"][3]["tool_calls"][0]["id"], json!("call-move01"));
    assert_eq!(body["messages"][3]["tool_calls"][0]["function"]["name"], json!("apply_patch"));
    let tool = &body["messages"][4];
    assert_eq!(tool["tool_call_id"], json!("call-move01"), "id 必须匹配");
    let result: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["ok"], json!(true), "ApplyPatchResult JSON：{result}");
    assert_eq!(result["state_known"], json!(true), "{result}");
    assert_eq!(result["error"], json!(null), "{result}");
    let changed = result["changed_files"].as_array().unwrap();
    assert_eq!(changed.len(), 2, "一个成功 Move 恰两条记录：{result}");
    assert_eq!(changed[0]["path"], json!(dst.display().to_string()), "先目标 add：{result}");
    assert_eq!(changed[0]["action"], json!("add"));
    assert_eq!(changed[1]["path"], json!(src.display().to_string()), "后源 delete：{result}");
    assert_eq!(changed[1]["action"], json!("delete"));
    let keys: Vec<&str> = result.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["ok", "changed_files", "state_known", "error"], "四字段且有序");
    assert!(out.contains("[工具] apply_patch（call-mov"), "{out}");
    assert!(out.contains("Agent：已移动"), "{out}");
}

/// T3：执行命令——真实输出与非零退出码出现在回传 ExecResult JSON。
#[test]
fn t3_exec_command_returns_real_output_and_exit_code() {
    let dir = TempDir::new("tl-t3");
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(
            &[call("call-exec-77", "exec_command", json!({ "cmd": "echo out-T3; exit 7" }).to_string())],
            None,
        )),
        MockResp::json(200, ok_response("命令完成")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "执行命令\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("[工具] exec_command（call-exe"), "{out}");
    let body = req_body(&mock, 1);
    let tool = &body["messages"][4];
    assert_eq!(tool["tool_call_id"], json!("call-exec-77"));
    let result: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["status"], json!("completed"), "{result}");
    assert_eq!(result["exit_code"], json!(7), "真实退出码回传：{result}");
    assert!(result["output"].as_str().unwrap().contains("out-T3"), "{result}");
    assert!(out.contains("Agent：命令完成"), "{out}");
}

/// T4：多个调用按返回顺序串行执行（后者依赖前者产物），结果一一对应。
#[test]
fn t4_multiple_calls_run_serially_in_order() {
    let dir = TempDir::new("tl-t4");
    let target = dir.path().join("serial.txt");
    let patch = add_file_patch(&target, "SERIAL-X");
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(&[
            call("call-multi1", "apply_patch", json!({ "patch": patch }).to_string()),
            call("call-multi2", "exec_command",
                 json!({ "cmd": format!("cat {}", target.display()) }).to_string()),
        ], None)),
        MockResp::json(200, ok_response("两步完成")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "两步\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(std::fs::read(&target).unwrap(), b"SERIAL-X\n".to_vec());
    let body = req_body(&mock, 1);
    let tools = tool_msgs(&body);
    assert_eq!(tools.len(), 2, "两个调用各一条 role=tool");
    assert_eq!(tools[0]["tool_call_id"], json!("call-multi1"));
    assert_eq!(tools[1]["tool_call_id"], json!("call-multi2"), "顺序与 id 对应");
    let r2: Value = serde_json::from_str(tools[1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(r2["exit_code"], json!(0));
    assert!(r2["output"].as_str().unwrap().contains("SERIAL-X"),
        "第二个调用读到第一个调用产物（串行证据）：{r2}");
    // 两个活动行按序出现
    let p1 = out.find("[工具] apply_patch").unwrap();
    let p2 = out.find("[工具] exec_command").unwrap();
    assert!(p1 < p2, "执行顺序与返回顺序一致：{out}");
}

/// T5：错误回传——arguments 非有效 JSON 属参数错误（工具失败结构）；未知工具回传 error；
/// 同批合法调用照常执行（循环不被错误中断）。
#[test]
fn t5_param_error_and_unknown_tool_echoed_as_tool_result() {
    let dir = TempDir::new("tl-t5");
    let made = dir.path().join("t5-made.txt");
    let patch = add_file_patch(&made, "T5-OK");
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(&[
            call("call-badargs", "exec_command", "not-json{".to_string()),
            call("call-unknow1", "web_search", "{}".to_string()),
            call("call-valid01", "apply_patch", json!({ "patch": patch }).to_string()),
        ], None)),
        MockResp::json(200, ok_response("错误已回传")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "错误场景\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(std::fs::read(&made).unwrap(), b"T5-OK\n".to_vec(), "同批合法调用照常执行");
    let body = req_body(&mock, 1);
    let tools = tool_msgs(&body);
    assert_eq!(tools.len(), 3, "每个调用各一条结果");
    let r1: Value = serde_json::from_str(tools[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(r1["status"], json!("error"), "参数错误按 exec 失败结构回传：{r1}");
    assert!(r1["error"].as_str().unwrap().contains("不是有效 JSON"), "{r1}");
    let r2: Value = serde_json::from_str(tools[1]["content"].as_str().unwrap()).unwrap();
    assert!(r2["error"].as_str().unwrap().contains("未知工具 web_search"), "未知工具：{r2}");
    let r3: Value = serde_json::from_str(tools[2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(r3["ok"], json!(true), "合法调用结果如实回传：{r3}");
    assert!(out.contains("Agent：错误已回传"), "{out}");
}

/// T6：跨用户轮次历史保留——第 3 请求原样携带调用、结果与最终回复＋新 user。
#[test]
fn t6_history_carried_across_user_turns() {
    let dir = TempDir::new("tl-t6");
    let target = dir.path().join("t6.txt");
    let patch = add_file_patch(&target, "T6");
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(
            &[call("call-t6-001", "apply_patch", json!({ "patch": patch }).to_string())],
            None,
        )),
        MockResp::json(200, ok_response("完成一")),
        MockResp::json(200, ok_response("完成二")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "第一轮\n第二轮\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Agent：完成一") && out.contains("Agent：完成二"), "{out}");
    assert_eq!(mock.count(), 3);
    let body = req_body(&mock, 2);
    let roles = role_seq(&body);
    assert_eq!(
        roles,
        vec!["system", "user", "user", "assistant", "tool", "assistant", "user"],
        "第 3 请求含完整历史（015 ctx）＋新 user"
    );
    assert_eq!(count_role(&body, "system"), 1);
    assert_eq!(body["messages"][5]["content"], json!("完成一"), "最终回复入历史");
}

/// T7：循环上限——恒返调用时恰 8 请求后报上限，无第 9 次、无假回复。
#[test]
fn t7_request_cap_reports_exhaustion() {
    let dir = TempDir::new("tl-t7");
    let target = dir.path().join("t7-counter.txt");
    // 每批都真实执行（追加一行），副作用次数＝批次数
    let cmd = format!("echo x >> {}", target.display());
    let mock = spawn_mock(vec![MockResp::json(200, tc_response(
        &[call("call-t7-loop1", "exec_command", json!({ "cmd": cmd }).to_string())],
        None,
    ))]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "循环\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 8, "恰 8 次请求，无第 9 次");
    assert!(out.contains("已达本轮模型请求上限（8）"), "{out}");
    assert!(!out.contains("Agent："), "上限错误不得伪造回复：{out}");
    let lines = std::fs::read_to_string(&target).unwrap().lines().count();
    assert_eq!(lines, 8, "第 8 批调用仍执行并保存：副作用恰 8 次");
    let body8 = req_body(&mock, 7);
    assert_eq!(tool_msgs(&body8).len(), 7, "第 8 次请求只含其发出前已有的 7 批结果");
}

/// T8：500／收完请求后断线均不重发；非幂等追加只执行一次，下一轮携带结果恢复。
#[test]
fn t8_request_failure_no_retry_history_kept() {
    for (label, failure) in [
        ("500", MockResp::json(500, r#"{"error":{"message":"down"}}"#)),
        ("disconnect", MockResp::close_now()),
    ] {
        let dir = TempDir::new(&format!("tl-t8 with space-{label}"));
        let target = dir.path().join("t8-once.txt");
        let cmd = "echo T8 >> t8-once.txt";
        let mock = spawn_mock(vec![
            MockResp::json(200, tc_response(
                &[call("call-t8-001", "exec_command", json!({ "cmd": cmd, "workdir": dir.path().display().to_string() }).to_string())],
                None,
            )),
            failure,
            MockResp::json(200, ok_response("恢复完成-T8")),
        ]);
        let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "go\nagain\nexit\n");
        assert_eq!(code, Some(0), "{label}：{out}");
        assert!(out.contains("错误："), "{label}：{out}");
        assert!(out.contains("Agent：恢复完成-T8"), "{label}：下一轮须可恢复：{out}");
        assert_eq!(mock.count(), 3, "{label}：turn1=2、turn2=1，无自动重发");
        assert_eq!(std::fs::read(&target).unwrap(), b"T8\n", "{label}：真实追加恰一次");
        assert_eq!(out.matches("[工具] exec_command").count(), 1, "{label}：已执行工具不重放");
        let body3 = req_body(&mock, 2);
        assert_eq!(
            role_seq(&body3),
            vec!["system", "user", "user", "assistant", "tool", "user"],
            "{label}：请求失败后历史保留已执行记录（含 015 ctx）"
        );
        assert_eq!(body3["messages"][4]["tool_call_id"], "call-t8-001");
        let result: Value = serde_json::from_str(body3["messages"][4]["content"].as_str().unwrap()).unwrap();
        assert_eq!(result["status"], "completed", "{label}：历史保留真实执行结果");
    }
}

/// T9：协议错误——响应缺 choices 明确报错，不执行任何工具。
#[test]
fn t9_protocol_error_aborts_without_execution() {
    let dir = TempDir::new("tl-t9");
    let mock = spawn_mock(vec![MockResp::json(200, r#"{"object":"list"}"#)]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "你好\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("错误："), "{out}");
    assert!(!out.contains("[工具]"), "协议错误不得执行工具：{out}");
    assert_eq!(mock.count(), 1);
}

/// T10：带调用且 content 非空——终端不提前显示，后续请求原样携带该字符串。
#[test]
fn t10_non_null_content_preserved_but_not_shown() {
    let dir = TempDir::new("tl-t10");
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(
            &[call("call-t10-01", "exec_command", json!({ "cmd": "true" }).to_string())],
            Some("思考片段-T10"),
        )),
        MockResp::json(200, ok_response("T10 完成")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "你好\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(!out.contains("思考片段-T10"), "带调用的 content 不得提前显示：{out}");
    let body = req_body(&mock, 1);
    assert_eq!(body["messages"][3]["content"], json!("思考片段-T10"),
        "原始字符串原样回传，不改成 null");
}

/// T11：连续工具循环中当前 user 不重复——第 3 请求 user 恰 1 条、system 恰 1 条。
#[test]
fn t11_current_user_not_duplicated_in_loop() {
    let dir = TempDir::new("tl-t11");
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(
            &[call("call-t11-a1", "exec_command", json!({ "cmd": "true" }).to_string())],
            None,
        )),
        MockResp::json(200, tc_response(
            &[call("call-t11-b2", "exec_command", json!({ "cmd": "true" }).to_string())],
            None,
        )),
        MockResp::json(200, ok_response("T11 完成")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "连续循环\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 3);
    let body = req_body(&mock, 2);
    assert_eq!(count_role(&body, "user"), 2, "015 ctx＋本轮 user，循环不重复追加");
    assert_eq!(count_role(&body, "system"), 1);
    assert_eq!(tool_msgs(&body).len(), 2, "两批结果各就位");
}

/// T12：批内首项合法、后项缺 id——整批无副作用、无效 assistant 不入历史，下一轮仍合法。
#[test]
fn t12_invalid_batch_no_side_effects_history_stays_valid() {
    let dir = TempDir::new("tl-t12");
    let flag = dir.path().join("t12-flag.txt");
    let patch = add_file_patch(&flag, "T12");
    let valid = call("call-t12-ok", "apply_patch", json!({ "patch": patch }).to_string());
    let invalid = json!({ "type": "function", "function": { "name": "exec_command", "arguments": "{}" } });
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(&[valid, invalid], None)),
        MockResp::json(200, ok_response("第二轮回复")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "go\nnext\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(!flag.exists(), "整批不执行：首项合法也不得有副作用");
    assert!(!out.contains("[工具]"), "{out}");
    assert!(out.contains("错误："), "协议错误须报错：{out}");
    assert_eq!(mock.count(), 2, "下一用户轮正常发起请求");
    let body = req_body(&mock, 1);
    let roles = role_seq(&body);
    assert_eq!(
        roles,
        vec!["system", "user", "user", "user"],
        "无效 assistant 不入历史：第 2 请求仅 system＋ctx＋go＋next（本轮回复在其之后产生）"
    );
    assert!(out.contains("Agent：第二轮回复"), "{out}");
}

/// T13：第 8 批结果保留＋下一用户轮计数重置——总 16 请求；第 9 请求携带 8 批结果。
#[test]
fn t13_cap_resets_next_turn_and_carries_batch8() {
    let dir = TempDir::new("tl-t13");
    let target = dir.path().join("t13.log");
    let cmd = format!("echo x >> {}", target.display());
    let mock = spawn_mock(vec![MockResp::json(200, tc_response(
        &[call("call-t13-loop", "exec_command", json!({ "cmd": cmd }).to_string())],
        None,
    ))]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "go\nnext\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 16, "两轮各 8 次（计数重置），无第 17 次");
    assert_eq!(out.matches("已达本轮模型请求上限（8）").count(), 2, "{out}");
    let body9 = req_body(&mock, 8);
    assert_eq!(tool_msgs(&body9).len(), 8, "第 9 请求携带含第 8 批在内的全部结果");
    let lines = std::fs::read_to_string(&target).unwrap().lines().count();
    assert_eq!(lines, 16, "两轮各执行 8 批");
}

/// T14：第 8 次请求返回最终回复 → 正常结束（006 §3 上限的另一分支），不报上限错误。
#[test]
fn t14_eighth_request_final_reply_ends_normally() {
    let dir = TempDir::new("tl-t14 with space");
    let target = dir.path().join("t14-effects.txt");
    let cmd = "echo T14 >> t14-effects.txt";
    let tc = tc_response(
        &[call("call-t14-loo", "exec_command", json!({ "cmd": cmd, "workdir": dir.path().display().to_string() }).to_string())],
        None,
    );
    let mut responses: Vec<MockResp> =
        (0..7).map(|_| MockResp::json(200, tc.clone())).collect();
    responses.push(MockResp::json(200, ok_response("第八次回复-T14")));
    let mock = spawn_mock(responses);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "go\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 8, "恰 8 次请求后由最终回复结束");
    assert!(
        out.contains("Agent：第八次回复-T14"),
        "第 8 次为最终回复须正常结束：{out}"
    );
    assert!(!out.contains("已达本轮模型请求上限"), "最终回复不得被上限错误吞掉：{out}");
    let body8 = req_body(&mock, 7);
    assert_eq!(tool_msgs(&body8).len(), 7, "第 8 次请求携带此前 7 批结果");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "T14\n".repeat(7),
        "前 7 批各真实执行一次，最终回复不增加副作用"
    );
}

/// T15（017）：完整计划流程——创建计划 → 真实 exec_command → 更新完成 → 最终回复。
/// 核对每次后续请求的原始参数、调用 ID 与两字段结果配对；CLI 输出说明、步骤
/// 与状态；计划文字不产生文件副作用。
#[test]
fn t15_update_plan_flow_with_real_exec_and_cli_output() {
    let dir = TempDir::new("tl-t15");
    let made = dir.path().join("t15-made.txt");
    let cmd = format!("echo T15-OK > {}", made.display());
    let plan1 = json!({
        "explanation": "开始任务",
        "plan": [
            { "step": "接入计划工具", "status": "completed" },
            { "step": "验证计划显示与恢复", "status": "in_progress" },
        ]
    })
    .to_string();
    let plan2 = json!({
        "explanation": "实现完成，开始验证",
        "plan": [
            { "step": "接入计划工具", "status": "completed" },
            { "step": "验证计划显示与恢复", "status": "completed" },
        ]
    })
    .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(&[call("call-plan-c1", "update_plan", plan1.clone())], None)),
        MockResp::json(200, tc_response(&[call("call-plan-x1", "exec_command", json!({ "cmd": cmd, "workdir": dir.path().display().to_string() }).to_string())], None)),
        MockResp::json(200, tc_response(&[call("call-plan-c2", "update_plan", plan2.clone())], None)),
        MockResp::json(200, ok_response("任务完成-T15")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "按计划执行\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 4);
    // 真实 exec 副作用（计划调用不产生文件副作用——本目录仅此一个文件）
    assert_eq!(std::fs::read_to_string(&made).unwrap(), "T15-OK\n");
    let entries = std::fs::read_dir(dir.path().join("user").join("sessions")).unwrap().count();
    assert_eq!(entries, 1, "计划文字不触发文件副作用（仅自动会话）");
    // CLI 输出：活动行＋两次清单（状态变化可见）＋说明
    assert!(out.contains("[工具] update_plan（call-pla"), "{out}");
    assert!(out.contains("计划已更新（1/2 步完成）"), "第一次快照：{out}");
    assert!(out.contains("计划已更新（2/2 步完成）"), "第二次快照：{out}");
    assert!(out.contains("说明：开始任务") && out.contains("说明：实现完成，开始验证"), "{out}");
    let first = out.find("计划已更新（1/2").unwrap();
    let second = out.find("计划已更新（2/2").unwrap();
    assert!(first < second, "快照按提交顺序：{out}");
    assert!(out.contains("[进行中] 验证计划显示与恢复"), "{out}");
    assert!(out.contains("[已完成] 验证计划显示与恢复"), "{out}");
    assert!(out.contains("Agent：任务完成-T15"), "{out}");
    // 请求配对：第 2 请求携带原始参数与两字段成功结果（id 对应）
    let body2 = req_body(&mock, 1);
    let tools2 = tool_msgs(&body2);
    assert_eq!(tools2.len(), 1, "第 2 请求含首批结果");
    assert_eq!(tools2[0]["tool_call_id"], json!("call-plan-c1"));
    assert_eq!(
        serde_json::from_str::<Value>(tools2[0]["content"].as_str().unwrap()).unwrap(),
        json!({ "ok": true, "error": null }),
        "两字段成功结果"
    );
    let assistant2 = body2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == json!("assistant") && m.get("tool_calls").is_some())
        .unwrap();
    assert_eq!(
        assistant2["tool_calls"][0]["function"]["arguments"],
        json!(plan1),
        "原始参数逐字保留"
    );
    // 第 4 请求携带全部三条结果（计划×2＋exec×1）与最终模型回复前的完整历史
    let body4 = req_body(&mock, 3);
    let tools4 = tool_msgs(&body4);
    assert_eq!(tools4.len(), 3, "第 4 请求含三批结果");
    assert_eq!(tools4[2]["tool_call_id"], json!("call-plan-c2"));
    let v: Value = serde_json::from_str(tools4[2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(v, json!({ "ok": true, "error": null }));
    // 用户轮工具面恰四项（含 update_plan）
    let names: Vec<&str> = body2["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["exec_command", "apply_patch", "write_stdin", "update_plan"],
        "017：用户轮四工具"
    );
}

/// T16（017）：计划参数错误按两字段失败回传（不升级协议错误、不中断循环）；
/// CLI 输出原因，不画清单。
#[test]
fn t16_update_plan_param_error_echoed_not_rendered() {
    let dir = TempDir::new("tl-t16");
    let bad = json!({ "plan": [{ "step": "s", "status": "blocked" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_response(&[call("call-plan-bad", "update_plan", bad)], None)),
        MockResp::json(200, ok_response("继续-T16")),
    ]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "更新计划\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 2, "参数错误不中断循环");
    assert!(out.contains("计划更新未生效："), "{out}");
    assert!(out.contains("blocked"), "失败原因含非法状态原文：{out}");
    assert!(!out.contains("计划已更新"), "失败不得画成功清单：{out}");
    assert!(out.contains("Agent：继续-T16"), "{out}");
    let body2 = req_body(&mock, 1);
    let tools2 = tool_msgs(&body2);
    assert_eq!(tools2[0]["tool_call_id"], json!("call-plan-bad"));
    let v: Value = serde_json::from_str(tools2[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(v["ok"], json!(false), "两字段失败：{v}");
    assert!(v["error"].as_str().unwrap().contains("blocked"), "{v}");
}

/// T17（017）：计划调用按模型请求计数——恒返计划调用时恰 8 请求后报上限，
/// 第 8 批计划结果保存（ok=true），无第 9 次请求、无假回复、无自动全部完成。
#[test]
fn t17_update_plan_request_cap_counts_model_requests() {
    let dir = TempDir::new("tl-t17");
    let plan = json!({ "plan": [{ "step": "唯一步骤", "status": "in_progress" }] }).to_string();
    let mock = spawn_mock(vec![MockResp::json(200, tc_response(
        &[call("call-plan-cap1", "update_plan", plan)],
        None,
    ))]);
    let (out, code) = run_cli(dir.path(), &mock.url, "tl-key", "循环\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(mock.count(), 8, "恰 8 次请求，无第 9 次");
    assert!(out.contains("已达本轮模型请求上限（8）"), "{out}");
    assert!(!out.contains("Agent："), "上限错误不得伪造回复：{out}");
    // CLI 8 次清单均显示「进行中」——额度耗尽不自动把未完成改成完成
    assert_eq!(out.matches("[进行中] 唯一步骤").count(), 8, "{out}");
    assert!(!out.contains("[已完成]"), "不得伪造成完成：{out}");
    // 第 8 批结果已保存（第 8 请求只含此前 7 条结果；会话文件含 8 条 ok=true）
    let body8 = req_body(&mock, 7);
    assert_eq!(tool_msgs(&body8).len(), 7);
    let session_dir = dir.path().join("user").join("sessions");
    let file = std::fs::read_dir(&session_dir).unwrap().next().unwrap().unwrap().path();
    let contents: Vec<Value> = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let ok_results = contents
        .iter()
        .filter(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                && v.get("content").and_then(|c| c.as_str()).is_some_and(|c| c.contains(r#""ok":true"#))
        })
        .count();
    assert_eq!(ok_results, 8, "第 8 批计划调用结果保存后报上限：会话含 8 条成功结果");
}

/// T18（017）：CLI 批内取消——长等待 exec 期间 SIGINT 取消本轮，批内跳过的
/// update_plan 返回两字段失败（SKIPPED 常量）；CLI 输出取消原因、不画清单；
/// 取消只停止等待与剩余调用（016：不终止已启动进程，退出时由宿主关闭）。
#[test]
fn t18_cancelled_batch_skips_plan_as_two_field_failure() {
    use std::io::Write as _;
    let dir = TempDir::new("tl-t18");
    let plan = json!({ "plan": [{ "step": "被取消的步骤", "status": "in_progress" }] }).to_string();
    let batch = tc_response(
        &[
            call(
                "call-t18-exec",
                "exec_command",
                json!({ "cmd": "sleep 3", "yield_time_ms": 29000 }).to_string(),
            ),
            call("call-t18-plan", "update_plan", plan),
        ],
        None,
    );
    let mock = spawn_mock(vec![MockResp::json(200, batch)]);
    // 与 run_cli 同环境，但保留 stdin 句柄以支持轮询后发送 SIGINT 与后续输入
    let cfg = dir.path().join("config");
    let user = dir.path().join("user");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    write_manual(&cfg, &mock.url, "env:TCHAT_TL_KEY");
    seed_grant(&user, &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint("tl-key"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.current_dir(dir.path())
        .env("TCHAT_CONFIG_DIR", &cfg)
        .env("TCHAT_USER_DIR", &user)
        .env("TCHAT_TL_KEY", "tl-key")
        .env("TERM", "xterm")
        .env_remove("TCHAT_PUBLIC_CATALOG_URL")
        .env_remove("APPDATA")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("CLI 子进程启动失败");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"go\n").unwrap();
    stdin.flush().unwrap();
    // 等首轮模型请求到达（turn 内 SIGINT 处理器已安装），在 exec 等待期发送 SIGINT
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while mock.count() < 1 {
        assert!(std::time::Instant::now() < deadline, "首轮请求未到达");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    stdin.write_all(b"exit\n").unwrap();
    stdin.flush().unwrap();
    drop(stdin);
    let out = child.wait_with_output().expect("等待 CLI 子进程失败");
    let out = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.contains("用户已取消"), "取消错误呈现：{out}");
    assert!(
        out.contains("计划更新未生效：用户取消，本调用未执行"),
        "跳过的计划调用按两字段失败输出原因：{out}"
    );
    assert!(!out.contains("计划已更新"), "取消不得画成功清单：{out}");
    // 会话落盘形状：exec＝cancelled（进程保留，016）；计划＝两字段失败
    let session_dir = user.join("sessions");
    let file = std::fs::read_dir(&session_dir).unwrap().next().unwrap().unwrap().path();
    let contents: Vec<Value> = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let plan_result = contents
        .iter()
        .find(|v| v.get("tool_call_id").and_then(|t| t.as_str()) == Some("call-t18-plan"))
        .expect("计划调用结果已落盘");
    assert_eq!(
        serde_json::from_str::<Value>(plan_result["content"].as_str().unwrap()).unwrap(),
        json!({ "ok": false, "error": "用户取消，本调用未执行" }),
        "跳过的计划调用＝两字段失败"
    );
    let exec_result = contents
        .iter()
        .find(|v| v.get("tool_call_id").and_then(|t| t.as_str()) == Some("call-t18-exec"))
        .expect("exec 结果已落盘");
    let exec_v: Value = serde_json::from_str(exec_result["content"].as_str().unwrap()).unwrap();
    assert_eq!(exec_v["status"], json!("cancelled"), "取消不终止进程（016）：{exec_v}");
}
