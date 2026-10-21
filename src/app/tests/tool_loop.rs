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
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["function"]["name"], json!("exec_command"));
    assert_eq!(tools[1]["function"]["name"], json!("apply_patch"));
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
