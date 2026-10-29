//! 024：通过本地 HTTP 边界验证真实原生接口，期望值来自固定协议样本。
mod common;

use common::{spawn_mock, MockResp};
use serde_json::{json, Value};
use terminal_chat::{api, catalog};

struct NativeDir(std::path::PathBuf);
impl NativeDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "tchat-native-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for NativeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn final_response(protocol: &str, text: &str) -> Value {
    match protocol {
        "openai-responses" => {
            json!({"status":"completed","output":[{"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":text}]}]})
        }
        "anthropic-messages" => {
            json!({"role":"assistant","type":"message","stop_reason":"end_turn",
            "content":[{"type":"text","text":text}]})
        }
        "google-interactions" => json!({"status":"completed","steps":[{"type":"model_output",
            "content":[{"type":"text","text":text}]}]}),
        "google-generate-content" => json!({"candidates":[{"finishReason":"STOP",
            "content":{"role":"model","parts":[{"text":text}]}}]}),
        _ => {
            json!({"choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}]})
        }
    }
}

fn native_connection(service: &common::Mock, protocol: &str) -> catalog::EffectiveConnection {
    catalog::EffectiveConnection {
        base_url: service.url.clone(),
        protocol: protocol.into(),
        key_ref: None,
        request_params: Default::default(),
    }
}

fn run_native_cli(dir: &NativeDir, service: &common::Mock, protocol: &str, input: &str) -> String {
    run_native_cli_params(dir, service, protocol, input, json!({}))
}

fn run_native_cli_params(
    dir: &NativeDir,
    service: &common::Mock,
    protocol: &str,
    input: &str,
    params: Value,
) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let config = dir.0.join("config");
    let user = dir.0.join("user");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(config.join("manual.json"), json!({
        "selected_provider":"native", "selected_model":"fixture-model",
        "providers":{"native":{"protocol":protocol,"base_url":service.url,
            "key_ref":"env:NATIVE_FIXTURE_KEY", "request_params":params,"models":{
                "fixture-model":{"id":"fixture-model","max_output":256,"supported_params":["stream"]}
            }}}
    }).to_string()).unwrap();
    std::fs::write(
        user.join("authorized-origins.json"),
        json!({"grants":[{
            "origin":catalog::origin_of(&service.url).unwrap(),
            "cred_fp":catalog::fingerprint("fixture-key"),"added_at":0
        }]})
        .to_string(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TCHAT_CONFIG_DIR", config)
        .env("TCHAT_USER_DIR", user)
        .env("NATIVE_FIXTURE_KEY", "fixture-key")
        .env("TERM", "xterm")
        .env("TZ", "UTC")
        .env("LANG", "C.UTF-8")
        .env("TCHAT_SESSION_TS", "20261024T120000")
        .env("TCHAT_SESSION_PID", "24")
        .current_dir(&dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn responses_text_uses_native_endpoint_and_stateless_input() {
    let service = spawn_mock(vec![MockResp::json(
        200,
        json!({
            "id": "resp_example", "status": "completed",
            "output": [{"type":"message", "role":"assistant", "content":[
                {"type":"output_text", "text":"原生回复", "annotations":[]}
            ]}]
        })
        .to_string(),
    )]);
    let conn = catalog::EffectiveConnection {
        base_url: format!("{}/gateway/v1", service.url),
        protocol: "openai-responses".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo {
        id: "test-model".into(),
        ..Default::default()
    };
    let reply = api::chat(&conn, "fixture-key", &model, "你好", &api::agent()).unwrap();
    assert_eq!(reply, "原生回复");
    let requests = service.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/gateway/v1/responses");
    assert_eq!(requests[0].auth.as_deref(), Some("Bearer fixture-key"));
    let body: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["store"], false);
    assert_eq!(body["input"], json!([{"role":"user","content":"你好"}]));
    assert!(body.get("messages").is_none());
    assert!(body.get("previous_response_id").is_none());
}

#[test]
fn responses_cli_tool_loop_preserves_reasoning_items_and_raw_call_id() {
    let dir = NativeDir::new();
    let reasoning = json!({"id":"rs_fixed","type":"reasoning","summary":[],
        "encrypted_content":"opaque:=+/中文"});
    let function = json!({"id":"item_not_call_id","type":"function_call",
        "call_id":" call_raw ","name":"exec_command",
        "arguments":r#"{"cmd":"printf native >> once.txt","yield_time_ms":1000}"#});
    let service = spawn_mock(vec![
        MockResp::json(
            200,
            json!({"status":"completed","output":[reasoning,function]}).to_string(),
        ),
        MockResp::json(
            200,
            json!({"status":"completed","output":[{
                "type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"工具完成","annotations":[]
                }]
            }]})
            .to_string(),
        ),
    ]);
    let out = run_native_cli(&dir, &service, "openai-responses", "执行工具\nexit\n");
    assert!(out.contains("Agent：工具完成"), "{out}");
    assert!(!out.contains("opaque:"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.0.join("once.txt")).unwrap(),
        "native"
    );
    let requests = service.requests();
    assert_eq!(requests.len(), 2);
    let first: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(first["tools"][0]["type"], "function");
    assert_eq!(first["tools"][0]["name"], "exec_command");
    assert!(first["tools"][0].get("function").is_none());
    let next: Value = serde_json::from_str(&requests[1].body).unwrap();
    let items = next["input"].as_array().unwrap();
    assert!(items.contains(&reasoning));
    assert!(items.contains(&function));
    let result = items
        .iter()
        .find(|v| v["type"] == "function_call_output")
        .unwrap();
    assert_eq!(result["call_id"], " call_raw ");
    assert!(result["output"].as_str().unwrap().contains("exit_code"));
}

#[test]
fn native_session_upgrades_v1_without_rewriting_and_replays_v2_after_restart() {
    let dir = NativeDir::new();
    let sessions = dir.0.join("user/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let old = sessions.join("auto-20261023T120000-23.jsonl");
    let old_bytes="{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"旧问题\"}\n{\"type\":\"text\",\"role\":\"assistant\",\"content\":\"旧回答\"}\n";
    std::fs::write(&old, old_bytes).unwrap();
    let signed =
        json!({"type":"reasoning","id":"rs_saved","summary":[],"encrypted_content":"saved:opaque"});
    let answer = |text: &str| {
        json!({"status":"completed","output":[
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}
        ]})
        .to_string()
    };
    let service=spawn_mock(vec![
        MockResp::json(200,json!({"status":"completed","output":[signed,
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"首次原生回答"}]}
        ]}).to_string()), MockResp::json(200,answer("恢复后回答"))
    ]);
    let out = run_native_cli(
        &dir,
        &service,
        "openai-responses",
        "/resume auto-20261023T120000-23\n继续\nexit\n",
    );
    assert!(out.contains("首次原生回答"), "{out}");
    assert_eq!(std::fs::read_to_string(&old).unwrap(), old_bytes);
    let new = sessions.join("auto-20261024T120000-24.jsonl");
    let saved = std::fs::read_to_string(&new).unwrap();
    assert_eq!(saved.lines().next(), Some(r#"{"v":2}"#));
    assert!(saved.contains("saved:opaque"));
    let out = run_native_cli(
        &dir,
        &service,
        "openai-responses",
        "/resume auto-20261024T120000-24\n再继续\nexit\n",
    );
    assert!(out.contains("恢复后回答"), "{out}");
    let requests = service.requests();
    assert_eq!(requests.len(), 2, "恢复不能自己发送模型请求");
    let second: Value = serde_json::from_str(&requests[1].body).unwrap();
    assert!(second["input"].as_array().unwrap().contains(&signed));
    assert!(second["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["content"] == "旧问题"));
}

#[test]
fn messages_cli_preserves_thinking_and_returns_native_tool_result() {
    let dir = NativeDir::new();
    let thinking =
        json!({"type":"thinking","thinking":"private reasoning", "signature":"signed:opaque"});
    let tool = json!({"type":"tool_use","id":"toolu_fixed","name":"exec_command",
        "input":{"cmd":"printf messages >> messages.txt","yield_time_ms":1000}});
    let service = spawn_mock(vec![
        MockResp::json(
            200,
            json!({"id":"msg_fixed","role":"assistant","type":"message",
            "content":[thinking,tool],"stop_reason":"tool_use",
            "usage":{"input_tokens":12,"output_tokens":9}})
            .to_string(),
        ),
        MockResp::json(
            200,
            json!({"id":"msg_final","role":"assistant","type":"message",
            "content":[{"type":"text","text":"Messages 完成"}],"stop_reason":"end_turn"})
            .to_string(),
        ),
    ]);
    let out = run_native_cli(&dir, &service, "anthropic-messages", "执行\nexit\n");
    assert!(out.contains("Agent：Messages 完成"), "{out}");
    assert!(!out.contains("private reasoning"));
    assert!(!out.contains("signed:opaque"));
    assert_eq!(
        std::fs::read_to_string(dir.0.join("messages.txt")).unwrap(),
        "messages"
    );
    let requests = service.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/messages");
    assert_eq!(
        requests[0].headers.get("x-api-key").map(String::as_str),
        Some("fixture-key")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("anthropic-version")
            .map(String::as_str),
        Some("2026-10-21")
    );
    assert!(requests[0].auth.is_none());
    let first: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(first["max_tokens"], 256);
    assert_eq!(first["tools"][0]["name"], "exec_command");
    assert_eq!(
        first["tools"][0]["input_schema"]["properties"]["cmd"]["type"],
        "string"
    );
    assert!(first["system"].is_string());
    let next: Value = serde_json::from_str(&requests[1].body).unwrap();
    let messages = next["messages"].as_array().unwrap();
    let assistant = messages.iter().find(|m| m["role"] == "assistant").unwrap();
    assert_eq!(assistant["content"], json!([thinking, tool]));
    let result = messages.last().unwrap()["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["type"] == "tool_result")
        .unwrap();
    assert_eq!(result["tool_use_id"], "toolu_fixed");
    assert_eq!(result["is_error"], false);
}

#[test]
fn interactions_cli_replays_steps_and_links_function_result() {
    let dir = NativeDir::new();
    let thought = json!({"type":"thought","signature":"google:opaque",
        "summary":[{"type":"text","text":"private google thought"}]});
    let call = json!({"type":"function_call","id":"google_call_fixed","name":"exec_command",
        "arguments":{"cmd":"printf google >> interactions.txt","yield_time_ms":1000}});
    let service = spawn_mock(vec![
        MockResp::json(
            200,
            json!({"id":"interaction_fixed","status":"requires_action","steps":[thought,call]})
                .to_string(),
        ),
        MockResp::json(
            200,
            json!({"id":"interaction_final","status":"completed","steps":[{
                "type":"model_output","content":[{"type":"text","text":"Interactions 完成"}]
            }]})
            .to_string(),
        ),
    ]);
    let out = run_native_cli(&dir, &service, "google-interactions", "执行\nexit\n");
    assert!(out.contains("Agent：Interactions 完成"), "{out}");
    assert!(!out.contains("private google thought"));
    assert!(!out.contains("google:opaque"));
    assert_eq!(
        std::fs::read_to_string(dir.0.join("interactions.txt")).unwrap(),
        "google"
    );
    let requests = service.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/interactions");
    assert_eq!(
        requests[0]
            .headers
            .get("x-goog-api-key")
            .map(String::as_str),
        Some("fixture-key")
    );
    assert!(requests[0].auth.is_none());
    let first: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(first["store"], false);
    assert!(first["system_instruction"].is_string());
    assert_eq!(first["tools"][0]["type"], "function");
    assert_eq!(first["tools"][0]["name"], "exec_command");
    assert_eq!(first["input"][0]["type"], "user_input");
    let next: Value = serde_json::from_str(&requests[1].body).unwrap();
    assert!(next["input"].as_array().unwrap().contains(&thought));
    assert!(next["input"].as_array().unwrap().contains(&call));
    let result = next["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["type"] == "function_result")
        .unwrap();
    assert_eq!(result["call_id"], "google_call_fixed");
    assert_eq!(result["name"], "exec_command");
    assert!(result["result"][0]["text"]
        .as_str()
        .unwrap()
        .contains("exit_code"));
    assert!(next.get("previous_interaction_id").is_none());
}

#[test]
fn generate_content_cli_preserves_parts_and_does_not_invent_wire_call_ids() {
    let dir = NativeDir::new();
    let thought = json!({"text":"private legacy thought","thought":true,
        "thoughtSignature":"Zml4ZWQtc2lnbmF0dXJl"});
    let call = json!({"functionCall":{"name":"exec_command",
        "args":{"cmd":"printf legacy >> generate.txt","yield_time_ms":1000}},
        "thoughtSignature":"Y2FsbC1zaWduYXR1cmU="});
    let service = spawn_mock(vec![
        MockResp::json(
            200,
            json!({"candidates":[{"index":0,"content":{
            "role":"model","parts":[thought,call]},"finishReason":"STOP"}]})
            .to_string(),
        ),
        MockResp::json(
            200,
            json!({"candidates":[{"index":0,"content":{
            "role":"model","parts":[{"text":"generateContent 完成"}]},"finishReason":"STOP"}]})
            .to_string(),
        ),
    ]);
    let out = run_native_cli(&dir, &service, "google-generate-content", "执行\nexit\n");
    assert!(out.contains("Agent：generateContent 完成"), "{out}");
    assert!(!out.contains("private legacy thought"));
    assert_eq!(
        std::fs::read_to_string(dir.0.join("generate.txt")).unwrap(),
        "legacy"
    );
    let requests = service.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/models/fixture-model:generateContent");
    assert_eq!(
        requests[0]
            .headers
            .get("x-goog-api-key")
            .map(String::as_str),
        Some("fixture-key")
    );
    assert!(requests[0].auth.is_none());
    let first: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert!(first.get("model").is_none());
    assert!(first["systemInstruction"]["parts"][0]["text"].is_string());
    assert_eq!(
        first["tools"][0]["functionDeclarations"][0]["name"],
        "exec_command"
    );
    assert_eq!(first["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]["properties"]["cmd"]["type"],"string");
    assert!(first["tools"][0]["functionDeclarations"][0].get("parameters").is_none());
    let next: Value = serde_json::from_str(&requests[1].body).unwrap();
    let contents = next["contents"].as_array().unwrap();
    assert_eq!(
        contents.iter().find(|c| c["role"] == "model").unwrap()["parts"],
        json!([thought, call])
    );
    let result = &contents.last().unwrap()["parts"][0]["functionResponse"];
    assert_eq!(result["name"], "exec_command");
    assert!(result["response"].is_object());
    assert!(
        result.get("id").is_none(),
        "缺少厂商 ID 时只能在宿主内部配对"
    );
}

#[test]
fn native_images_reach_user_and_tool_result_and_require_declared_capability() {
    let image = "data:image/png;base64,aW1hZ2U=";
    for protocol in [
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let service = spawn_mock(vec![MockResp::json(
            200,
            final_response(protocol, "看到了").to_string(),
        )]);
        let conn = native_connection(&service, protocol);
        let mut model = catalog::ModelInfo {
            id: "vision-fixture".into(),
            max_output: Some(256),
            ..Default::default()
        };
        let request = json!({"messages":[],"_history":[
            {"type":"text","role":"user","content":"用户图片","images":[image]},
            {"type":"tool_calls","content":null,"tool_calls":[{"id":"image_call","name":"view_image","arguments":"{}"}]},
            {"type":"tool_result","tool_call_id":"image_call","content":"{\"ok\":true}",
                "image":{"image_url":image,"detail":"high"}}
        ]});
        let error =
            api::chat_with_body(&conn, "fixture-key", &model, &request, &api::agent()).unwrap_err();
        assert!(error.contains("image"), "{protocol}: {error}");
        assert!(service.requests().is_empty(), "能力未知时不能发送");
        model.input_modalities = Some(vec!["text".into(), "image".into()]);
        api::chat_with_body(&conn, "fixture-key", &model, &request, &api::agent()).unwrap();
        let requests = service.requests();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_str(&requests[0].body).unwrap();
        match protocol {
            "openai-responses" => {
                assert_eq!(
                    body["input"][0]["content"][1],
                    json!({"type":"input_image","image_url":image,"detail":"high"})
                );
                assert_eq!(body["input"][2]["output"][1]["image_url"], image);
            }
            "anthropic-messages" => {
                assert_eq!(
                    body["messages"][0]["content"][1]["source"],
                    json!({"type":"base64","media_type":"image/png","data":"aW1hZ2U="})
                );
                assert_eq!(
                    body["messages"][2]["content"][0]["content"][1]["type"],
                    "image"
                );
            }
            "google-interactions" => {
                assert_eq!(body["input"][0]["content"][1]["data"], "aW1hZ2U=");
                assert_eq!(body["input"][2]["result"][1]["type"], "image");
            }
            _ => {
                assert_eq!(
                    body["contents"][0]["parts"][1]["inlineData"],
                    json!({"mimeType":"image/png","data":"aW1hZ2U="})
                );
                assert_eq!(
                    body["contents"][2]["parts"][0]["functionResponse"]["parts"][0]["inlineData"]
                        ["data"],
                    "aW1hZ2U="
                );
            }
        }
    }
}

#[test]
fn native_model_lists_use_provider_auth_and_read_every_page_without_guessing_capabilities() {
    for protocol in [
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let google = protocol.starts_with("google-");
        let pages = if google {
            vec![
                json!({"models":[{"name":"models/模型一","displayName":"模型一","inputTokenLimit":4096,"outputTokenLimit":512}],"nextPageToken":"page +/二"}),
                json!({"models":[{"name":"models/final"}]}),
            ]
        } else {
            vec![
                json!({"data":[{"id":"first","display_name":"First"}],"has_more":true,"last_id":"first"}),
                json!({"data":[{"id":"final"}],"has_more":false,"last_id":"final"}),
            ]
        };
        let service = spawn_mock(
            pages
                .into_iter()
                .map(|v| MockResp::json(200, v.to_string()))
                .collect(),
        );
        let mut conn = native_connection(&service, protocol);
        conn.base_url.push_str("/gateway/v1");
        let models = api::list_models(&conn, "fixture-key", &api::agent()).unwrap();
        assert_eq!(models.len(), 2, "{protocol}");
        assert_eq!(models[1].id, "final");
        assert!(models
            .iter()
            .all(|m| m.input_modalities.is_none() && m.supported_params.is_none()));
        let requests = service.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/gateway/v1/models");
        let url = url::Url::parse(&format!("http://localhost{}", requests[1].path)).unwrap();
        let query: Vec<_> = url.query_pairs().collect();
        let (parameter, cursor) = if google {
            ("pageToken", "page +/二")
        } else if protocol == "anthropic-messages" {
            ("after_id", "first")
        } else {
            ("after", "first")
        };
        assert_eq!(query, vec![(parameter.into(), cursor.into())]);
        if google {
            assert_eq!(models[0].id, "模型一");
            assert_eq!(models[0].context_length, Some(4096));
            assert_eq!(models[0].max_output, Some(512));
            assert_eq!(
                requests[0]
                    .headers
                    .get("x-goog-api-key")
                    .map(String::as_str),
                Some("fixture-key")
            );
        } else if protocol == "anthropic-messages" {
            assert_eq!(
                requests[0].headers.get("x-api-key").map(String::as_str),
                Some("fixture-key")
            );
        } else {
            assert_eq!(requests[0].auth.as_deref(), Some("Bearer fixture-key"));
        }
    }
}

fn sse(events: Vec<(&str, Value)>) -> String {
    events
        .into_iter()
        .map(|(event, value)| format!("event: {event}\r\ndata: {value}\r\n\r\n"))
        .collect()
}

#[test]
fn every_protocol_assembles_sse_text_and_rejects_missing_completion() {
    let fixtures = vec![
        (
            "openai-chat",
            sse(vec![
                (
                    "message",
                    json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"流式"},"finish_reason":null}]}),
                ),
                (
                    "message",
                    json!({"choices":[{"index":0,"delta":{"content":"完成"},"finish_reason":"stop"}]}),
                ),
            ]),
            "data: [DONE]\r\n\r\n".into(),
        ),
        (
            "openai-responses",
            sse(vec![
                (
                    "response.created",
                    json!({"type":"response.created","response":{"status":"in_progress"}}),
                ),
                (
                    "response.output_text.delta",
                    json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"流式完成"}),
                ),
            ]),
            sse(vec![(
                "response.completed",
                json!({"type":"response.completed","response":final_response("openai-responses","流式完成")}),
            )]),
        ),
        (
            "anthropic-messages",
            sse(vec![
                (
                    "message_start",
                    json!({"type":"message_start","message":{"id":"msg_stream","role":"assistant","type":"message","content":[],"stop_reason":null}}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"流式"}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"完成"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":0}),
                ),
                (
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
                ),
            ]),
            sse(vec![("message_stop", json!({"type":"message_stop"}))]),
        ),
        (
            "google-interactions",
            sse(vec![
                (
                    "interaction.created",
                    json!({"event_type":"interaction.created","interaction":{"id":"inter_stream","status":"in_progress"}}),
                ),
                (
                    "step.start",
                    json!({"event_type":"step.start","index":0,"step":{"type":"model_output"}}),
                ),
                (
                    "step.delta",
                    json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"流式"}}),
                ),
                (
                    "step.delta",
                    json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"完成"}}),
                ),
                ("step.stop", json!({"event_type":"step.stop","index":0})),
            ]),
            sse(vec![(
                "interaction.completed",
                json!({"event_type":"interaction.completed","interaction":{"id":"inter_stream","status":"completed"}}),
            )]),
        ),
        (
            "google-generate-content",
            sse(vec![(
                "message",
                json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"流式"}]}}]}),
            )]),
            sse(vec![(
                "message",
                json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"完成"}]},"finishReason":"STOP"}]}),
            )]),
        ),
    ];
    for (protocol, partial, complete) in fixtures {
        let service = common::spawn_mock_with_content_type(
            vec![
                MockResp::json(200, format!(": keepalive\r\n\r\n{partial}{complete}")),
                MockResp::json(200, partial),
            ],
            "text/event-stream; charset=utf-8",
        );
        let mut conn = native_connection(&service, protocol);
        conn.request_params.insert("stream".into(), json!(true));
        let model = catalog::ModelInfo {
            id: "stream-fixture".into(),
            max_output: Some(256),
            supported_params: Some(vec!["stream".into()]),
            ..Default::default()
        };
        assert_eq!(
            api::chat(&conn, "fixture-key", &model, "你好", &api::agent()).unwrap(),
            "流式完成",
            "{protocol}"
        );
        assert!(
            api::chat(&conn, "fixture-key", &model, "不能提交半条", &api::agent()).is_err(),
            "{protocol}"
        );
        let requests = service.requests();
        assert_eq!(requests.len(), 2);
        if protocol == "google-generate-content" {
            assert_eq!(
                requests[0].path,
                "/models/stream-fixture:streamGenerateContent?alt=sse"
            );
            assert!(serde_json::from_str::<Value>(&requests[0].body)
                .unwrap()
                .get("stream")
                .is_none());
        }
    }
}

fn final_sse(protocol: &str) -> String {
    match protocol {
        "openai-chat" => format!(
            "{}data: [DONE]\n\n",
            sse(vec![(
                "message",
                json!({"choices":[{"index":0,
            "delta":{"content":"流式工具完成"},"finish_reason":"stop"}]})
            )])
        ),
        "openai-responses" => sse(vec![(
            "response.completed",
            json!({"type":"response.completed","response":final_response(protocol,"流式工具完成")}),
        )]),
        "anthropic-messages" => sse(vec![
            (
                "message_start",
                json!({"type":"message_start","message":final_response(protocol,"流式工具完成")}),
            ),
            ("message_stop", json!({"type":"message_stop"})),
        ]),
        "google-interactions" => sse(vec![
            (
                "interaction.created",
                json!({"event_type":"interaction.created","interaction":{"id":"sse_final","status":"in_progress"}}),
            ),
            (
                "step.start",
                json!({"event_type":"step.start","index":0,"step":{"type":"model_output","content":[{"type":"text","text":"流式工具完成"}]}}),
            ),
            ("step.stop", json!({"event_type":"step.stop","index":0})),
            (
                "interaction.completed",
                json!({"event_type":"interaction.completed","interaction":{"id":"sse_final","status":"completed"}}),
            ),
        ]),
        _ => sse(vec![("message", final_response(protocol, "流式工具完成"))]),
    }
}

fn tool_sse(protocol: &str) -> (String, String) {
    let arguments = r#"{"cmd":"printf stream >> streamed.txt","yield_time_ms":1000}"#;
    let (left, right) = arguments.split_at(20);
    match protocol {
        "openai-chat" => (
            sse(vec![
                (
                    "message",
                    json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"stream_call","type":"function","function":{"name":"exec_","arguments":left}}]},"finish_reason":null}]}),
                ),
                (
                    "message",
                    json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"command","arguments":right}}]},"finish_reason":"tool_calls"}]}),
                ),
            ]),
            "data: [DONE]\n\n".into(),
        ),
        "openai-responses" => (
            sse(vec![
                (
                    "response.function_call_arguments.delta",
                    json!({"type":"response.function_call_arguments.delta","item_id":"stream_item","output_index":1,"delta":left}),
                ),
                (
                    "response.function_call_arguments.delta",
                    json!({"type":"response.function_call_arguments.delta","item_id":"stream_item","output_index":1,"delta":right}),
                ),
            ]),
            sse(vec![(
                "response.completed",
                json!({"type":"response.completed","response":{"status":"completed","output":[
                    {"type":"reasoning","encrypted_content":"stream:opaque","summary":[]},
                    {"type":"function_call","id":"stream_item","call_id":"stream_call","name":"exec_command","arguments":arguments}
                ]}}),
            )]),
        ),
        "anthropic-messages" => (
            sse(vec![
                (
                    "message_start",
                    json!({"type":"message_start","message":{"role":"assistant","type":"message","content":[],"stop_reason":null}}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"private stream thought"}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"stream:opaque"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":0}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"stream_call","name":"exec_command","input":{}}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":left}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":right}}),
                ),
                (
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":1}),
                ),
                (
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
                ),
            ]),
            sse(vec![("message_stop", json!({"type":"message_stop"}))]),
        ),
        "google-interactions" => (
            sse(vec![
                (
                    "interaction.created",
                    json!({"event_type":"interaction.created","interaction":{"id":"sse_call","status":"in_progress"}}),
                ),
                (
                    "step.start",
                    json!({"event_type":"step.start","index":0,"step":{"type":"thought"}}),
                ),
                (
                    "step.delta",
                    json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"stream:opaque"}}),
                ),
                ("step.stop", json!({"event_type":"step.stop","index":0})),
                (
                    "step.start",
                    json!({"event_type":"step.start","index":1,"step":{"type":"function_call","id":"stream_call","name":"exec_command","arguments":{}}}),
                ),
                (
                    "step.delta",
                    json!({"event_type":"step.delta","index":1,"delta":{"type":"arguments_delta","arguments":left}}),
                ),
                (
                    "step.delta",
                    json!({"event_type":"step.delta","index":1,"delta":{"type":"arguments_delta","arguments":right}}),
                ),
                ("step.stop", json!({"event_type":"step.stop","index":1})),
            ]),
            sse(vec![(
                "interaction.completed",
                json!({"event_type":"interaction.completed","interaction":{"id":"sse_call","status":"requires_action"}}),
            )]),
        ),
        _ => (
            sse(vec![(
                "message",
                json!({"candidates":[{"index":0,"content":{"role":"model","parts":[
                    {"text":"private stream thought","thought":true,"thoughtSignature":"stream:opaque"},
                    {"functionCall":{"name":"exec_command","args":serde_json::from_str::<Value>(arguments).unwrap()},"thoughtSignature":"stream:opaque"}
                ]}}]}),
            )]),
            sse(vec![(
                "message",
                json!({"candidates":[{"index":0,"finishReason":"STOP"}]}),
            )]),
        ),
    }
}

#[test]
fn sse_tool_arguments_and_signatures_are_complete_before_execution_or_save() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let dir = NativeDir::new();
        let (partial, complete) = tool_sse(protocol);
        let service = common::spawn_mock_with_content_type(
            vec![
                MockResp::json(200, format!("{partial}{complete}")),
                MockResp::json(200, final_sse(protocol)),
            ],
            "text/event-stream",
        );
        let out = run_native_cli_params(
            &dir,
            &service,
            protocol,
            "执行\nexit\n",
            json!({"stream":true}),
        );
        assert!(out.contains("Agent：流式工具完成"), "{protocol}: {out}");
        assert!(!out.contains("private stream thought") && !out.contains("stream:opaque"));
        assert_eq!(
            std::fs::read_to_string(dir.0.join("streamed.txt")).unwrap(),
            "stream"
        );
        let requests = service.requests();
        assert_eq!(requests.len(), 2, "{protocol}");
        if protocol != "openai-chat" {
            assert!(requests[1].body.contains("stream:opaque"), "{protocol}");
        }

        let interrupted = NativeDir::new();
        let service = common::spawn_mock_with_content_type(
            vec![MockResp::json(200, partial)],
            "text/event-stream",
        );
        let out = run_native_cli_params(
            &interrupted,
            &service,
            protocol,
            "执行\nexit\n",
            json!({"stream":true}),
        );
        assert!(out.contains("错误"), "{protocol}: {out}");
        assert!(
            !interrupted.0.join("streamed.txt").exists(),
            "{protocol}: 不完整流不得执行工具"
        );
        assert_eq!(service.count(), 1, "禁止自动重试");
    }
}

#[test]
fn switching_source_keeps_closed_visible_history_but_never_transfers_opaque_or_open_calls() {
    for change in ["provider", "model", "protocol", "url", "key", "chat"] {
        let target = match change {
            "protocol" => "anthropic-messages",
            "chat" => "openai-chat",
            _ => "openai-responses",
        };
        let service =
            spawn_mock(
                vec![MockResp::json(200,json!({"status":"completed","output":[
            {"type":"reasoning","encrypted_content":"owner-only:opaque","summary":[]},
            {"type":"function_call","call_id":"old_call","name":"旧工具","arguments":"{}"}
        ]}).to_string()),MockResp::json(200,final_response(target,"新来源回答").to_string())],
            );
        let conn = native_connection(&service, "openai-responses");
        let model = catalog::ModelInfo {
            id: "original".into(),
            max_output: Some(256),
            ..Default::default()
        };
        let response = api::chat_with_provider_body(
            "original-provider",
            &conn,
            "fixture-key",
            &model,
            &json!({"messages":[{"role":"user","content":"旧问题"}]}),
            &api::agent(),
        )
        .unwrap();
        let native = json!({"type":"native_assistant","message":{"type":"tool_calls","content":"",
            "tool_calls":[{"id":"old_call","name":"旧工具","arguments":"{}"}]},"state":response["native_state"]});
        let mut changed = conn.clone();
        changed.protocol = target.into();
        if change == "url" {
            changed.base_url.push_str("/another");
        }
        let mut changed_model = model.clone();
        if change == "model" {
            changed_model.id = "another-model".into();
        }
        let provider = if change == "provider" {
            "another-provider"
        } else {
            "original-provider"
        };
        let key = if change == "key" {
            "another-fixture-key"
        } else {
            "fixture-key"
        };
        let open = json!({"messages":[],"_history":[native,{"type":"text","role":"user","content":"继续"}]});
        let error = api::chat_with_provider_body(
            provider,
            &changed,
            key,
            &changed_model,
            &open,
            &api::agent(),
        )
        .unwrap_err();
        assert!(
            error.contains("未闭合") || error.contains("不能续接"),
            "{change}: {error}"
        );
        assert_eq!(service.count(), 1, "未闭合调用链发送前拒绝");
        let closed = json!({"messages":[],"_history":[native,
            {"type":"tool_result","tool_call_id":"old_call","content":"原始结果"},
            {"type":"text","role":"user","content":"继续"}]});
        api::chat_with_provider_body(
            provider,
            &changed,
            key,
            &changed_model,
            &closed,
            &api::agent(),
        )
        .unwrap();
        let requests = service.requests();
        assert_eq!(requests.len(), 2, "{change}");
        assert!(!requests[1].body.contains("owner-only:opaque"), "{change}");
        assert!(
            requests[1].body.contains("旧工具") && requests[1].body.contains("原始结果"),
            "{change}"
        );
        assert!(
            requests[1].body.contains("历史"),
            "旧工具内容应明确标注为历史"
        );
    }
}

#[test]
fn native_errors_identify_source_and_request_without_credentials_or_retry() {
    for protocol in [
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let service = spawn_mock(vec![
            MockResp::json(200, "{}"),
            MockResp::json(
                401,
                json!({"error":{"message":"invalid fixture-key"}}).to_string(),
            ),
        ]);
        let conn = native_connection(&service, protocol);
        let model = catalog::ModelInfo {
            id: "error-model".into(),
            max_output: Some(256),
            ..Default::default()
        };
        for status in ["HTTP 200", "401"] {
            let error = api::chat_with_provider_body(
                "error-provider",
                &conn,
                "fixture-key",
                &model,
                &json!({"messages":[{"role":"user","content":"你好"}]}),
                &api::agent(),
            )
            .unwrap_err();
            for expected in [
                "error-provider",
                "error-model",
                protocol,
                status,
                "offline-fixture",
            ] {
                assert!(error.contains(expected), "缺少 {expected}: {error}");
            }
            assert!(!error.contains("fixture-key"), "{error}");
            assert!(
                !error.contains(&catalog::fingerprint("fixture-key")),
                "{error}"
            );
        }
        assert_eq!(service.count(), 2, "错误不得自动重试");
    }
}

fn tool_response(protocol: &str, calls: &[Value]) -> Value {
    match protocol {
        "openai-responses" => json!({"status":"completed","output":calls.iter().map(|c|json!({
            "type":"function_call","call_id":c["id"],"name":c["function"]["name"],"arguments":c["function"]["arguments"]
        })).collect::<Vec<_>>()}),
        "anthropic-messages" => {
            json!({"role":"assistant","type":"message","stop_reason":"tool_use",
            "content":calls.iter().map(|c|json!({"type":"tool_use","id":c["id"],"name":c["function"]["name"],
                "input":serde_json::from_str::<Value>(c["function"]["arguments"].as_str().unwrap()).unwrap()})).collect::<Vec<_>>()})
        }
        "google-interactions" => {
            json!({"status":"requires_action","steps":calls.iter().map(|c|json!({
            "type":"function_call","id":c["id"],"name":c["function"]["name"],
            "arguments":serde_json::from_str::<Value>(c["function"]["arguments"].as_str().unwrap()).unwrap()
        })).collect::<Vec<_>>()})
        }
        "google-generate-content" => {
            json!({"candidates":[{"finishReason":"STOP","content":{"role":"model",
            "parts":calls.iter().map(|c|json!({"functionCall":{"id":c["id"],"name":c["function"]["name"],
                "args":serde_json::from_str::<Value>(c["function"]["arguments"].as_str().unwrap()).unwrap()}})).collect::<Vec<_>>()}}]})
        }
        _ => {
            json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":calls},"finish_reason":"tool_calls"}]})
        }
    }
}

fn function_call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}})
}

#[test]
fn every_protocol_runs_memory_extraction_and_real_restricted_merge() {
    use terminal_chat::memory::{config::MemoriesConfig, runner, store::MemoryStore};
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let dir = NativeDir::new();
        let sessions = dir.0.join("sessions");
        std::fs::create_dir(&sessions).unwrap();
        let previous = json!({"type":"native_assistant","message":{"type":"text","role":"assistant","content":"历史结论"},
            "state":{"owner":{"provider_id":"previous-source","protocol":"openai-responses","model_id":"previous-model",
                "base_url":"http://127.0.0.1:1","key_ref":null,"credential_fingerprint":"previous-opaque-owner"},
                "payload":{"status":"completed","output":[{"type":"reasoning","summary":[],"encrypted_content":"opaque-must-not-enter-memory"},
                    {"type":"message","role":"assistant","content":[{"type":"output_text","text":"历史结论"}]}]}}});
        std::fs::write(sessions.join("auto-memory-old.jsonl"),format!("{{\"v\":2}}\n{{\"type\":\"text\",\"role\":\"user\",\"content\":\"历史任务\"}}\n{previous}\n")).unwrap();
        let root = runner::memories_root(&dir.0);
        let clock: terminal_chat::memory::Clock = std::sync::Arc::new(|| 1_800_000_000);
        let store = MemoryStore::open(&root, clock.clone()).unwrap();
        store
            .register_thread(
                "auto-memory-old",
                "enabled",
                (1_800_000_000 - 7 * 86400) * 1000,
            )
            .unwrap();
        drop(store);
        let calls = vec![
            function_call(
                "memory-main",
                "mem_write",
                json!({"path":"MEMORY.md","content":"# Native memory\n- fact\n"}),
            ),
            function_call(
                "memory-summary",
                "mem_write",
                json!({"path":"memory_summary.md","content":"v1\n- 原生合并摘要\n"}),
            ),
        ];
        let service=spawn_mock(vec![
            MockResp::json(200,final_response(protocol,r#"{"raw_memory":"经验","rollout_summary":"原生任务摘要","rollout_slug":"native"}"#).to_string()),
            MockResp::json(200,tool_response(protocol,&calls).to_string()),
            MockResp::json(200,final_response(protocol,"合并完成").to_string()),
        ]);
        let conn = native_connection(&service, protocol);
        let model = catalog::ModelInfo {
            id: "memory-model".into(),
            max_output: Some(256),
            ..Default::default()
        };
        let send = |body: &Value| {
            api::chat_with_provider_body(
                "memory-provider",
                &conn,
                "fixture-key",
                &model,
                body,
                &api::agent(),
            )
        };
        runner::run_memory_pipeline_once(
            &root,
            &sessions,
            &MemoriesConfig {
                enabled: true,
                ..Default::default()
            },
            "auto-current",
            &model,
            &send,
            clock.clone(),
        );
        let store = MemoryStore::open(&root, clock).unwrap();
        let output = store
            .stage1_output("auto-memory-old")
            .unwrap()
            .expect("原生提取必须入库");
        assert_eq!(output.rollout_summary, "原生任务摘要", "{protocol}");
        assert_eq!(
            std::fs::read_to_string(root.join("MEMORY.md")).unwrap(),
            "# Native memory\n- fact\n"
        );
        assert!(std::fs::read_to_string(root.join("memory_summary.md"))
            .unwrap()
            .contains("原生合并摘要"));
        let requests = service.requests();
        assert_eq!(requests.len(), 3, "{protocol}");
        let body: Value = serde_json::from_str(&requests[1].body).unwrap();
        let definitions = if protocol == "google-generate-content" {
            &body["tools"][0]["functionDeclarations"]
        } else {
            &body["tools"]
        };
        let names: Vec<_> = definitions
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                if protocol == "openai-chat" {
                    t["function"]["name"].as_str().unwrap()
                } else {
                    t["name"].as_str().unwrap()
                }
            })
            .collect();
        assert_eq!(names, vec!["mem_read", "mem_write", "mem_list"]);
        assert!(requests[0].body.contains("历史结论"));
        assert!(!requests[0].body.contains("opaque-must-not-enter-memory"));
        assert!(!requests[0].body.contains("previous-opaque-owner"));
    }
}

#[test]
fn structured_output_uses_explicit_native_parameters_and_unknown_capability_is_local_error() {
    let schema = json!({"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false});
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let (field, format) = match protocol {
            "openai-chat" => (
                "response_format",
                json!({"type":"json_schema","json_schema":{"name":"summary","strict":true,"schema":schema}}),
            ),
            "openai-responses" => (
                "text",
                json!({"format":{"type":"json_schema","name":"summary","strict":true,"schema":schema}}),
            ),
            "anthropic-messages" => (
                "output_config",
                json!({"format":{"type":"json_schema","schema":schema}}),
            ),
            "google-interactions" => (
                "response_format",
                json!({"type":"text","mime_type":"application/json","schema":schema}),
            ),
            _ => (
                "generationConfig",
                json!({"responseMimeType":"application/json","responseJsonSchema":schema}),
            ),
        };
        let service = spawn_mock(vec![MockResp::json(
            200,
            final_response(protocol, r#"{"summary":"ok"}"#).to_string(),
        )]);
        let mut conn = native_connection(&service, protocol);
        conn.request_params.insert(field.into(), format.clone());
        let mut model = catalog::ModelInfo {
            id: "structured".into(),
            max_output: Some(256),
            ..Default::default()
        };
        assert!(api::chat(&conn, "fixture-key", &model, "摘要", &api::agent()).is_err());
        assert_eq!(service.count(), 0);
        model.supported_params = Some(vec![field.into()]);
        assert_eq!(
            api::chat(&conn, "fixture-key", &model, "摘要", &api::agent()).unwrap(),
            r#"{"summary":"ok"}"#
        );
        let body: Value = serde_json::from_str(&service.requests()[0].body).unwrap();
        assert_eq!(body[field], format, "{protocol}");
    }
}

#[test]
fn malformed_local_request_or_connection_never_reaches_http() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let service = spawn_mock(vec![MockResp::json(
            200,
            final_response(protocol, "不应请求").to_string(),
        )]);
        let conn = native_connection(&service, protocol);
        let model = catalog::ModelInfo {
            id: "local-fixture".into(),
            max_output: Some(256),
            supported_params: Some(vec!["stream".into()]),
            ..Default::default()
        };
        for body in [Value::Null, json!([]), json!({"messages":"bad"})] {
            assert!(
                api::chat_with_provider_body(
                    "fixture",
                    &conn,
                    "fixture-key",
                    &model,
                    &body,
                    &api::agent()
                )
                .is_err(),
                "{protocol}"
            );
        }
        let request = json!({"messages":[{"role":"user","content":"你好"}]});
        for field in ["model", "messages", "tools", "_history", "_provider_id"] {
            let mut changed = conn.clone();
            changed.request_params.insert(field.into(), json!([]));
            assert!(
                api::chat_with_body(&changed, "fixture-key", &model, &request, &api::agent())
                    .is_err(),
                "{protocol}: {field}"
            );
        }
        let mut changed = conn.clone();
        changed.request_params.insert("stream".into(), json!("yes"));
        assert!(
            api::chat_with_body(&changed, "fixture-key", &model, &request, &api::agent()).is_err(),
            "{protocol}: stream 类型"
        );
        for suffix in ["?route=other", "#fragment"] {
            let mut changed = conn.clone();
            changed.base_url.push_str(suffix);
            assert!(
                api::chat_with_body(&changed, "fixture-key", &model, &request, &api::agent())
                    .is_err(),
                "{protocol}: 非法基础地址"
            );
        }
        assert_eq!(service.count(), 0, "{protocol}: 本地错误必须零发送");
    }
}

#[test]
fn batch_structure_is_atomic_but_bad_tool_parameters_are_individual_failures() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        for invalid in ["33", "duplicate", "missing_name"] {
            let dir = NativeDir::new();
            let first = function_call(
                "first",
                "exec_command",
                json!({"cmd":"printf unsafe >> rejected.txt","yield_time_ms":1000}),
            );
            let mut calls = vec![first];
            if invalid == "33" {
                for n in 1..33 {
                    calls.push(function_call(&format!("call{n}"), "unknown", json!({})));
                }
            } else {
                calls.push(function_call(
                    if invalid == "duplicate" {
                        "first"
                    } else {
                        "second"
                    },
                    "unknown",
                    json!({}),
                ));
            }
            let mut response = tool_response(protocol, &calls);
            if invalid == "missing_name" {
                let target = match protocol {
                    "openai-chat" => {
                        &mut response["choices"][0]["message"]["tool_calls"][1]["function"]
                    }
                    "openai-responses" => &mut response["output"][1],
                    "anthropic-messages" => &mut response["content"][1],
                    "google-interactions" => &mut response["steps"][1],
                    _ => &mut response["candidates"][0]["content"]["parts"][1]["functionCall"],
                };
                target.as_object_mut().unwrap().remove("name");
            }
            let service = spawn_mock(vec![MockResp::json(200, response.to_string())]);
            let out = run_native_cli(&dir, &service, protocol, "执行\nexit\n");
            assert!(out.contains("错误"), "{protocol} {invalid}: {out}");
            assert!(
                !dir.0.join("rejected.txt").exists(),
                "{protocol} {invalid}: 整批结构错误不得部分执行"
            );
            assert_eq!(service.count(), 1);
        }
        let dir = NativeDir::new();
        let mut calls = vec![
            function_call("unknown", " unknown ", json!({})),
            function_call("bad-schema", "exec_command", json!({"cmd":17})),
            function_call(
                "valid",
                "exec_command",
                json!({"cmd":"printf safe >> accepted.txt","yield_time_ms":1000}),
            ),
        ];
        if matches!(protocol, "openai-chat" | "openai-responses") {
            let mut invalid = function_call("invalid-json", "exec_command", json!({}));
            invalid["function"]["arguments"] = json!("{invalid");
            calls.push(invalid);
        }
        let service = spawn_mock(vec![
            MockResp::json(200, tool_response(protocol, &calls).to_string()),
            MockResp::json(200, final_response(protocol, "逐项完成").to_string()),
        ]);
        let out = run_native_cli(&dir, &service, protocol, "执行\nexit\n");
        assert!(out.contains("Agent：逐项完成"), "{protocol}: {out}");
        assert_eq!(
            std::fs::read_to_string(dir.0.join("accepted.txt")).unwrap(),
            "safe"
        );
        let requests = service.requests();
        assert_eq!(requests.len(), 2);
        for id in ["unknown", "bad-schema", "valid"] {
            assert!(requests[1].body.contains(id), "{protocol}: {id}");
        }

        let dir = NativeDir::new();
        let calls: Vec<_> = (0..32)
            .map(|n| function_call(&format!("limit{n}"), "unknown", json!({})))
            .collect();
        let service = spawn_mock(vec![
            MockResp::json(200, tool_response(protocol, &calls).to_string()),
            MockResp::json(200, final_response(protocol, "32 项完成").to_string()),
        ]);
        let out = run_native_cli(&dir, &service, protocol, "执行\nexit\n");
        assert!(out.contains("Agent：32 项完成"), "{protocol}: {out}");
        assert_eq!(service.count(), 2);
    }
}

#[test]
fn eighth_batch_executes_and_persists_before_request_limit_stops_the_turn() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let dir = NativeDir::new();
        let responses = (1..=8)
            .map(|n| {
                MockResp::json(
                    200,
                    tool_response(
                        protocol,
                        &[function_call(
                            &format!("round{n}"),
                            "exec_command",
                            json!({"cmd":format!("printf {n} >> rounds.txt"),"yield_time_ms":1000}),
                        )],
                    )
                    .to_string(),
                )
            })
            .collect();
        let service = spawn_mock(responses);
        let out = run_native_cli(&dir, &service, protocol, "执行\nexit\n");
        assert!(out.contains("8"), "{out}");
        assert_eq!(service.count(), 8, "{protocol}");
        assert_eq!(
            std::fs::read_to_string(dir.0.join("rounds.txt")).unwrap(),
            "12345678",
            "{protocol}"
        );
        let sessions = std::fs::read_dir(dir.0.join("user/sessions")).unwrap();
        let saved = sessions
            .map(|p| std::fs::read_to_string(p.unwrap().path()).unwrap())
            .collect::<String>();
        assert!(saved.contains("round8"), "第八批须保存");
    }
}

#[test]
fn corrupt_v2_history_is_visible_but_blocks_continuation_without_http_or_replay() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        for damaged in ["{broken\n", "{\"type\":\"native_assistant\",\"message\":{\"type\":\"text\",\"role\":\"assistant\",\"content\":\"旧答复\"}}\n"] {
            let dir=NativeDir::new();let sessions=dir.0.join("user/sessions");std::fs::create_dir_all(&sessions).unwrap();
            std::fs::write(sessions.join("damaged.jsonl"),format!("{{\"v\":2}}\n{{\"type\":\"text\",\"role\":\"user\",\"content\":\"旧问题\"}}\n{damaged}")).unwrap();
            let service=spawn_mock(vec![MockResp::json(200,final_response(protocol,"不应请求").to_string())]);
            let out=run_native_cli(&dir,&service,protocol,"/resume damaged\n继续\nexit\n");
            assert!(out.contains("恢复"),"{out}");assert!(out.contains("原生状态"),"{protocol}: {out}");
            assert_eq!(service.count(),0,"{protocol}: 缺必要状态应阻止请求");
        }
    }
}

#[test]
fn model_and_gateway_encodings_do_not_change_the_selected_resource() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let service = spawn_mock(vec![MockResp::json(
            200,
            final_response(protocol, "编码保持").to_string(),
        )]);
        let mut conn = native_connection(&service, protocol);
        conn.base_url.push_str("/gateway/%2Fprefix/v1/");
        let id = if protocol == "google-generate-content" {
            "models/模型 x/%2F"
        } else {
            "模型 x/%2F"
        };
        let model = catalog::ModelInfo {
            id: id.into(),
            max_output: Some(256),
            ..Default::default()
        };
        assert_eq!(
            api::chat(&conn, "fixture-key", &model, "你好", &api::agent()).unwrap(),
            "编码保持"
        );
        let requests = service.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].path.starts_with("/gateway/%2Fprefix/v1/"),
            "{}",
            requests[0].path
        );
        if protocol == "google-generate-content" {
            assert_eq!(
                requests[0].path,
                "/gateway/%2Fprefix/v1/models/%E6%A8%A1%E5%9E%8B%20x%2F%252F:generateContent"
            );
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&requests[0].body).unwrap()["model"],
                id
            );
        }
    }
}

#[test]
fn every_protocol_enforces_exact_serialized_request_and_response_byte_limits() {
    let limit = 10 * 1024 * 1024;
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let service = spawn_mock(vec![MockResp::json(
            200,
            final_response(protocol, "边界完成").to_string(),
        )]);
        let conn = native_connection(&service, protocol);
        let model = catalog::ModelInfo {
            id: "boundary".into(),
            max_output: Some(256),
            ..Default::default()
        };
        // 独立的协议样本确定空正文开销，不调用被测编码器计算期望值。
        let empty = match protocol {
            "openai-chat" => json!({"model":"boundary","messages":[{"role":"user","content":""}]}),
            "openai-responses" => {
                json!({"model":"boundary","input":[{"role":"user","content":""}],"store":false})
            }
            "anthropic-messages" => {
                json!({"model":"boundary","messages":[{"role":"user","content":[{"type":"text","text":""}]}],"max_tokens":256})
            }
            "google-interactions" => {
                json!({"model":"boundary","input":[{"type":"user_input","content":[{"type":"text","text":""}]}],"store":false})
            }
            _ => json!({"contents":[{"role":"user","parts":[{"text":""}]}]}),
        };
        let size = limit - serde_json::to_vec(&empty).unwrap().len();
        let error = api::chat(
            &conn,
            "fixture-key",
            &model,
            &"x".repeat(size + 1),
            &api::agent(),
        )
        .unwrap_err();
        assert!(
            error.contains("请求体") && error.contains("上限"),
            "{protocol}: {error}"
        );
        assert_eq!(service.count(), 0);
        assert_eq!(
            api::chat(
                &conn,
                "fixture-key",
                &model,
                &"x".repeat(size),
                &api::agent()
            )
            .unwrap(),
            "边界完成"
        );
        assert_eq!(service.requests()[0].body.len(), limit, "{protocol}");

        let mut exact = final_response(protocol, "");
        let pointer = match protocol {
            "openai-chat" => "/choices/0/message/content",
            "openai-responses" => "/output/0/content/0/text",
            "anthropic-messages" => "/content/0/text",
            "google-interactions" => "/steps/0/content/0/text",
            _ => "/candidates/0/content/parts/0/text",
        };
        let text_len = limit - serde_json::to_vec(&exact).unwrap().len();
        *exact.pointer_mut(pointer).unwrap() = json!("x".repeat(text_len));
        let exact = exact.to_string();
        assert_eq!(exact.len(), limit);
        let over = format!("{exact} ");
        let service = spawn_mock(vec![MockResp::json(200, exact), MockResp::json(200, over)]);
        let conn = native_connection(&service, protocol);
        assert_eq!(
            api::chat(&conn, "fixture-key", &model, "你好", &api::agent())
                .unwrap()
                .len(),
            text_len,
            "{protocol}"
        );
        let error = api::chat(&conn, "fixture-key", &model, "你好", &api::agent()).unwrap_err();
        assert!(error.contains("10 MiB"), "{protocol}: {error}");
        assert_eq!(service.count(), 2);
    }
}

#[test]
fn sse_raw_total_and_assembled_output_have_separate_limits() {
    let limit = 10 * 1024 * 1024;
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let finish = final_sse(protocol);
        let exact = format!(": {}\n\n{finish}", "x".repeat(limit - finish.len() - 4));
        assert_eq!(exact.len(), limit);
        let service = common::spawn_mock_with_content_type(
            vec![
                MockResp::json(200, exact.clone()),
                MockResp::json(200, format!(" {exact}")),
            ],
            "text/event-stream",
        );
        let mut conn = native_connection(&service, protocol);
        conn.request_params.insert("stream".into(), json!(true));
        let model = catalog::ModelInfo {
            id: "stream-boundary".into(),
            max_output: Some(256),
            supported_params: Some(vec!["stream".into()]),
            ..Default::default()
        };
        assert_eq!(
            api::chat(&conn, "fixture-key", &model, "你好", &api::agent()).unwrap(),
            "流式工具完成"
        );
        assert!(
            api::chat(&conn, "fixture-key", &model, "你好", &api::agent())
                .unwrap_err()
                .contains("10 MiB")
        );
    }
    // Gemini 流式事件可省略默认候选索引和角色；组装补齐后的 JSON 比原始 SSE 更大。
    let empty="data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"\"}]},\"finishReason\":\"STOP\"}]}\n\n";
    let payload = empty.replacen(
        "\"text\":\"\"",
        &format!("\"text\":\"{}\"", "x".repeat(limit - empty.len())),
        1,
    );
    assert_eq!(payload.len(), limit);
    let service = common::spawn_mock_with_content_type(
        vec![MockResp::json(200, payload)],
        "text/event-stream",
    );
    let mut conn = native_connection(&service, "google-generate-content");
    conn.request_params.insert("stream".into(), json!(true));
    let model = catalog::ModelInfo {
        id: "assembled".into(),
        supported_params: Some(vec!["stream".into()]),
        ..Default::default()
    };
    let error = api::chat(&conn, "fixture-key", &model, "你好", &api::agent()).unwrap_err();
    assert!(error.contains("组装结果超过"), "{error}");
}

#[test]
fn tui_renders_native_tool_reply_and_discards_late_response_after_cancel() {
    use std::process::Command;
    use std::time::{Duration, Instant};
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let dir = NativeDir::new();
        let first = tool_response(
            protocol,
            &[function_call(
                "tui-first",
                "exec_command",
                json!({"cmd":"printf once >> tui-once.txt","yield_time_ms":1000}),
            )],
        );
        let late = tool_response(
            protocol,
            &[function_call(
                "tui-late",
                "exec_command",
                json!({"cmd":"printf unsafe >> tui-late.txt","yield_time_ms":1000}),
            )],
        );
        let (held, gate) = MockResp::gated(200, late.to_string());
        let service = spawn_mock(vec![
            MockResp::json(200, first.to_string()),
            MockResp::json(200, final_response(protocol, "native-tui-done").to_string()),
            held,
            MockResp::json(
                200,
                final_response(protocol, "recovered-after-cancel").to_string(),
            ),
        ]);
        let config = dir.0.join("config");
        let user = dir.0.join("user");
        std::fs::create_dir(&config).unwrap();
        std::fs::create_dir(&user).unwrap();
        std::fs::write(config.join("manual.json"),json!({"selected_provider":"native","selected_model":"fixture-model","providers":{"native":{
            "protocol":protocol,"base_url":service.url,"key_ref":"env:NATIVE_FIXTURE_KEY","models":{"fixture-model":{"id":"fixture-model","max_output":256}}}}}).to_string()).unwrap();
        std::fs::write(user.join("authorized-origins.json"),json!({"grants":[{"origin":catalog::origin_of(&service.url).unwrap(),"cred_fp":catalog::fingerprint("fixture-key"),"added_at":0}]}).to_string()).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TERM", "xterm-256color")
            .env("TCHAT_CONFIG_DIR", config)
            .env("TCHAT_USER_DIR", user)
            .env("NATIVE_FIXTURE_KEY", "fixture-key")
            .env("TCHAT_SESSION_TS", "20261025T120000")
            .env("TCHAT_SESSION_PID", "24")
            .env("TZ", "UTC")
            .env("LANG", "C.UTF-8")
            .current_dir(&dir.0);
        let mut tui = common::pty::PtyCli::spawn(cmd);
        tui.wait_screen("? for shortcuts");
        tui.send("执行");
        tui.wait_screen("native-tui-done");
        assert_eq!(
            std::fs::read_to_string(dir.0.join("tui-once.txt")).unwrap(),
            "once"
        );
        tui.send("等待后取消");
        let deadline = Instant::now() + Duration::from_secs(5);
        while service.count() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(service.count(), 3, "取消前须进入真实请求等待");
        tui.send_keys("\x1b");
        tui.wait_screen_timeout("已停止等待模型回复", Duration::from_secs(3));
        gate.release();
        tui.send("新问题");
        tui.wait_screen("recovered-after-cancel");
        assert!(
            !dir.0.join("tui-late.txt").exists(),
            "{protocol}: 迟到响应不得执行"
        );
        assert_eq!(service.count(), 4);
        tui.send("exit");
        let (output, code) = tui.finish();
        assert_eq!(code, Some(0), "{protocol}: {output}");
    }
}

#[test]
fn untrusted_error_or_incomplete_reply_never_executes_tools_or_exposes_the_key() {
    for protocol in ["openai-chat","openai-responses","anthropic-messages","google-interactions","google-generate-content"] {
        for attack in ["duplicate-key-id","error-with-calls","incomplete"] {
            let dir=NativeDir::new();
            let call=function_call("fixture-key","exec_command",json!({"cmd":"printf unsafe >> security.txt","yield_time_ms":1000}));
            let calls=if attack=="duplicate-key-id" {vec![call.clone(),call]} else {vec![call]};
            let mut reply=tool_response(protocol,&calls);
            if attack=="error-with-calls" {reply["error"]=json!({"message":"server echoed fixture-key"});}
            if attack=="incomplete" {
                match protocol {
                    "openai-chat"=>reply["choices"][0]["finish_reason"]=json!("length"),
                    "openai-responses"|"google-interactions"=>reply["status"]=json!("incomplete"),
                    "anthropic-messages"=>reply["stop_reason"]=json!("max_tokens"),
                    _=>reply["candidates"][0]["finishReason"]=json!("MAX_TOKENS"),
                }
            }
            let service=spawn_mock(vec![MockResp::json(200,reply.to_string())]);
            let out=run_native_cli(&dir,&service,protocol,"执行\nexit\n");
            assert!(out.contains("错误"),"{protocol} {attack}: {out}");
            assert!(!out.contains("fixture-key"),"{protocol} {attack}: 错误不得回显鉴权 Key: {out}");
            assert!(!dir.0.join("security.txt").exists(),"{protocol} {attack}: 不得执行出错响应的工具");
            assert_eq!(service.count(),1,"不得自动重发或继续此调用链");
        }
    }
}

#[test]
fn damaged_native_payload_or_mismatched_saved_call_is_rejected_before_http() {
    for protocol in ["openai-responses","anthropic-messages","google-interactions","google-generate-content"] {
        for damage in ["invalid-id-type","different-id","different-arguments"] {
            let service=spawn_mock(vec![MockResp::json(200,tool_response(protocol,&[
                function_call("saved_call","exec_command",json!({"cmd":"true"}))
            ]).to_string()),MockResp::json(200,final_response(protocol,"不应请求").to_string())]);
            let conn=native_connection(&service,protocol);
            let model=catalog::ModelInfo {id:"saved-model".into(),max_output:Some(256),..Default::default()};
            let first=api::chat_with_provider_body("saved-source",&conn,"fixture-key",&model,
                &json!({"messages":[{"role":"user","content":"旧问题"}]}),&api::agent()).unwrap();
            let mut state=first["native_state"].clone();
            let (pointer,id_field,args_field)=match protocol {
                "openai-responses"=>("/payload/output/0","call_id","arguments"),
                "anthropic-messages"=>("/payload/content/0","id","input"),
                "google-interactions"=>("/payload/steps/0","id","arguments"),
                _=>("/payload/candidates/0/content/parts/0/functionCall","id","args"),
            };
            let call=state.pointer_mut(pointer).unwrap();
            match damage {
                "invalid-id-type"=>call[id_field]=json!(7),
                "different-id"=>call[id_field]=json!("changed-call-id"),
                _=>call[args_field]=if protocol=="openai-responses" {json!(r#"{"cmd":"changed"}"#)} else {json!({"cmd":"changed"})},
            }
            let request=json!({"messages":[],"_history":[
                {"type":"text","role":"user","content":"旧问题"},
                {"type":"native_assistant","message":{"type":"tool_calls","content":"","tool_calls":[{
                    "id":"saved_call","name":"exec_command","arguments":r#"{"cmd":"true"}"#}]},"state":state},
                {"type":"tool_result","tool_call_id":"saved_call","content":"旧结果"},
                {"type":"text","role":"user","content":"继续"}
            ]});
            let error=api::chat_with_provider_body("saved-source",&conn,"fixture-key",&model,&request,&api::agent()).unwrap_err();
            assert!(error.contains("状态") || error.contains("续接"),"{protocol}: {error}");
            assert_eq!(service.count(),1,"{protocol} {damage}: 损坏状态零发送");
        }
    }
}

#[test]
fn stream_requires_declared_capability_before_any_request_on_every_protocol() {
    // §1.2/§1.6：stream 是可选参数，与其它 request_params 走同一能力门；
    // 已确认不支持或能力未知都必须零 HTTP 本地报错。
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        for declared in [Some(vec![]), Some(vec!["temperature".into()]), None] {
            let service = spawn_mock(vec![MockResp::json(
                200,
                final_response(protocol, "不应发送").to_string(),
            )]);
            let conn = native_connection(&service, protocol);
            let model = catalog::ModelInfo {
                id: "stream-gate".into(),
                max_output: Some(256),
                supported_params: declared,
                ..Default::default()
            };
            let mut streaming = conn.clone();
            streaming.request_params.insert("stream".into(), json!(true));
            let error = api::chat_with_body(
                &streaming,
                "fixture-key",
                &model,
                &json!({"messages":[{"role":"user","content":"你好"}]}),
                &api::agent(),
            )
            .unwrap_err();
            assert!(error.contains("stream"), "{protocol}: {error}");
            assert!(
                error.contains("已确认不被") || error.contains("能力未知"),
                "{protocol}: {error}"
            );
            assert_eq!(service.count(), 0, "{protocol}: 能力错误必须零发送");
        }
    }
}

#[test]
fn generate_content_missing_args_is_empty_object_but_non_object_is_rejected() {
    // 官方 FunctionCall.args 可省略（无参函数）：缺省按 {} 参与，不整批拒绝；
    // 显式非对象 args 仍是协议错误，零工具执行。
    let dir = NativeDir::new();
    let no_args = json!({"functionCall":{"name":"exec_command"},
        "thoughtSignature":"Y2FsbC1zaWduYXR1cmU="});
    let ok_call = json!({"functionCall":{"name":"exec_command",
        "args":{"cmd":"printf legacy >> generate.txt","yield_time_ms":1000}}});
    let service = spawn_mock(vec![
        MockResp::json(
            200,
            json!({"candidates":[{"index":0,"content":{
                "role":"model","parts":[no_args.clone(),ok_call]},"finishReason":"STOP"}]})
                .to_string(),
        ),
        MockResp::json(
            200,
            json!({"candidates":[{"index":0,"content":{
                "role":"model","parts":[{"text":"无参完成"}]},"finishReason":"STOP"}]})
                .to_string(),
        ),
    ]);
    let out = run_native_cli(&dir, &service, "google-generate-content", "执行\nexit\n");
    assert!(out.contains("Agent：无参完成"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.0.join("generate.txt")).unwrap(),
        "legacy",
        "合法兄弟调用不得因无参调用被阻断"
    );
    let requests = service.requests();
    assert_eq!(requests.len(), 2, "无参调用不得中断工具循环");
    let next: Value = serde_json::from_str(&requests[1].body).unwrap();
    let contents = next["contents"].as_array().unwrap();
    let responses: Vec<&Value> = contents
        .iter()
        .filter_map(|c| c["parts"].as_array())
        .flatten()
        .filter(|p| p.get("functionResponse").is_some())
        .map(|p| &p["functionResponse"])
        .collect();
    assert_eq!(responses.len(), 2, "两个调用都要有结果回传");
    assert!(responses.iter().all(|r| r["name"] == "exec_command"));
    let executed = responses
        .iter()
        .any(|r| r["response"].to_string().contains("exit_code"));
    assert!(executed, "合法调用按正常结果回传：{next}");
    let contents_model = contents
        .iter()
        .find(|c| c["role"] == "model")
        .expect("续接请求含模型 parts");
    assert!(
        contents_model["parts"]
            .as_array()
            .unwrap()
            .contains(&no_args),
        "缺省 args 的原生 Part 逐字回传：{contents_model}"
    );

    let rejected = NativeDir::new();
    let service = spawn_mock(vec![MockResp::json(
        200,
        json!({"candidates":[{"index":0,"content":{
            "role":"model","parts":[{"functionCall":{"name":"exec_command","args":"不是对象"}}]},
            "finishReason":"STOP"}]})
            .to_string(),
    )]);
    let out = run_native_cli(&rejected, &service, "google-generate-content", "执行\nexit\n");
    assert!(out.contains("错误"), "{out}");
    assert!(
        out.contains("args"),
        "错误要指明 args 字段：{out}"
    );
    assert!(!rejected.0.join("generate.txt").exists());
    assert_eq!(service.count(), 1);
}

#[test]
fn malformed_interactions_sse_is_a_protocol_error_not_a_crash() {
    // 外部 SSE 可把 interaction.created 的 interaction 设为字符串；
    // 必须按协议错误终止本轮，不得让进程 panic。
    let dir = NativeDir::new();
    let malicious = sse(vec![
        (
            "interaction.created",
            json!({"event_type":"interaction.created","interaction":"malformed"}),
        ),
        (
            "interaction.completed",
            json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
        ),
    ]);
    let service = common::spawn_mock_with_content_type(
        vec![MockResp::json(200, malicious)],
        "text/event-stream",
    );
    let out = run_native_cli_params(
        &dir,
        &service,
        "google-interactions",
        "执行\nexit\n",
        json!({"stream":true}),
    );
    assert!(out.contains("错误"), "{out}");
    assert_eq!(service.count(), 1, "禁止自动重试");
}

#[test]
fn unexpressible_legacy_arguments_become_marked_history_text_not_a_block() {
    // §1.4：旧 Chat Completions 历史里非法 JSON 参数的工具块，对对象参数协议
    // 无法合法表达：转成明确标注的历史文本继续会话，不整轮阻断、不伪造调用。
    for protocol in ["anthropic-messages", "google-interactions", "google-generate-content"] {
        let service = spawn_mock(vec![MockResp::json(
            200,
            final_response(protocol, "已转历史文本").to_string(),
        )]);
        let conn = native_connection(&service, protocol);
        let model = catalog::ModelInfo {
            id: "legacy".into(),
            max_output: Some(256),
            ..Default::default()
        };
        let request = json!({"messages":[],"_history":[
            {"type":"text","role":"user","content":"旧问题"},
            {"type":"tool_calls","content":"旧思考","tool_calls":[{
                "id":"legacy_call","name":"exec_command","arguments":"{invalid"}]},
            {"type":"tool_result","tool_call_id":"legacy_call","content":"旧结果"},
            {"type":"text","role":"user","content":"继续"}
        ]});
        let reply = api::chat_with_provider_body(
            "cc-source",
            &conn,
            "fixture-key",
            &model,
            &request,
            &api::agent(),
        )
        .unwrap();
        assert_eq!(reply["assistant"]["content"], "已转历史文本", "{protocol}");
        let body = &service.requests()[0].body;
        assert!(body.contains("历史工具调用"), "{protocol}: {body}");
        assert!(body.contains("[历史工具结果 legacy_call]"), "{protocol}: {body}");
        for banned in ["tool_use", "tool_result", "function_call", "functionCall", "functionResponse"] {
            assert!(!body.contains(banned), "{protocol}: 不得伪造原生调用块 {banned}");
        }
        assert_eq!(service.count(), 1, "{protocol}");
    }
}

#[test]
fn hostile_sse_shapes_and_arg_types_are_rejected_without_panic() {
    // 安全攻防：外部 SSE／响应的畸形形态必须全部走协议错误路径，
    // 不允许任何形态触发 panic 或半执行。
    for hostile in [
        json!({"event_type":"interaction.created","interaction":123}),
        json!({"event_type":"interaction.created","interaction":[]}),
        json!({"event_type":"interaction.created","interaction":null}),
        json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
        json!({"event_type":"interaction.status_update","status":"completed"}),
        json!({"event_type":"step.stop","index":0}),
    ] {
        let first = sse(vec![(
            "interaction.created",
            json!({"event_type":"interaction.created","interaction":{"id":"h","status":"in_progress"}}),
        )]);
        let payload = match hostile["event_type"].as_str() {
            Some("interaction.created") => sse(vec![(
                "interaction.created",
                hostile.clone(),
            )]),
            _ => format!("{first}{}", sse(vec![(
                hostile["event_type"].as_str().unwrap_or("interaction.status_update"),
                hostile.clone(),
            )])),
        };
        let service = common::spawn_mock_with_content_type(
            vec![MockResp::json(200, format!("{payload}{}", sse(vec![(
                "interaction.completed",
                json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
            )])))],
            "text/event-stream",
        );
        let conn = native_connection(&service, "google-interactions");
        let mut model = catalog::ModelInfo {
            id: "hostile".into(),
            max_output: Some(256),
            ..Default::default()
        };
        let mut streaming = conn.clone();
        streaming.request_params.insert("stream".into(), json!(true));
        model.supported_params = Some(vec!["stream".into()]);
        let error = api::chat_with_body(
            &streaming,
            "fixture-key",
            &model,
            &json!({"messages":[{"role":"user","content":"你好"}]}),
            &api::agent(),
        )
        .unwrap_err();
        // 核心安全断言：任何畸形形态都走 Err 协议／校验错误路径（不 panic），
        // 零重试；空响应变体由 normalized 统一拒绝（"原生响应没有…"）。
        assert!(!error.is_empty(), "{hostile}");
        assert_eq!(service.count(), 1, "{hostile}");
    }
    for bad in [json!("text"), json!(17), json!([]), json!(true)] {
        let service = spawn_mock(vec![MockResp::json(
            200,
            json!({"candidates":[{"index":0,"content":{
                "role":"model","parts":[{"functionCall":{"name":"exec_command","args":bad}}]},
                "finishReason":"STOP"}]})
                .to_string(),
        )]);
        let conn = native_connection(&service, "google-generate-content");
        let model = catalog::ModelInfo {
            id: "args-hostile".into(),
            max_output: Some(256),
            ..Default::default()
        };
        let error = api::chat_with_body(
            &conn,
            "fixture-key",
            &model,
            &json!({"messages":[{"role":"user","content":"你好"}]}),
            &api::agent(),
        )
        .unwrap_err();
        assert!(error.contains("args"), "{bad}: {error}");
        assert_eq!(service.count(), 1, "{bad}");
    }
}

#[test]
fn generate_content_null_args_is_treated_as_missing() {
    // JSON null 与缺省等价（无参函数）；官方允许省略 args。
    let service = spawn_mock(vec![MockResp::json(
        200,
        json!({"candidates":[{"index":0,"content":{
            "role":"model","parts":[{"functionCall":{"name":"exec_command","args":null}}]},
            "finishReason":"STOP"}]})
            .to_string(),
    )]);
    let conn = native_connection(&service, "google-generate-content");
    let model = catalog::ModelInfo {
        id: "null-args".into(),
        max_output: Some(256),
        ..Default::default()
    };
    let reply = api::chat_with_body(
        &conn,
        "fixture-key",
        &model,
        &json!({"messages":[{"role":"user","content":"你好"}]}),
        &api::agent(),
    )
    .unwrap();
    assert_eq!(
        reply["assistant"]["tool_calls"][0]["function"]["arguments"],
        "{}"
    );
}

#[test]
fn tui_native_image_request_carries_anthropic_image_block() {
    // 024 N07 修订：TUI×原生协议×图片三重组合——`-i` 初始图经 PTY 提交，
    // 断言 anthropic-messages 请求实际携带 base64 图片块并完成 TUI 响应流程。
    use std::process::Command;
    const PNG_2X2: [u8; 74] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2,
        8, 6, 0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 224, 18,
        145, 251, 15, 194, 12, 48, 6, 0, 38, 140, 4, 237, 162, 200, 71, 131, 0, 0, 0, 0, 73,
        69, 78, 68, 174, 66, 96, 130,
    ];
    let dir = NativeDir::new();
    let service = spawn_mock(vec![MockResp::json(
        200,
        final_response("anthropic-messages", "初始图已收").to_string(),
    )]);
    let config = dir.0.join("config");
    let user = dir.0.join("user");
    let work = dir.0.join("work");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(config.join("manual.json"), json!({
        "selected_provider":"native","selected_model":"fixture-model",
        "providers":{"native":{"protocol":"anthropic-messages","base_url":service.url,
            "key_ref":"env:NATIVE_FIXTURE_KEY","models":{"fixture-model":{
                "id":"fixture-model","max_output":256,
                "input_modalities":["text","image"]}}}}
    }).to_string()).unwrap();
    std::fs::write(user.join("authorized-origins.json"), json!({"grants":[{
        "origin":catalog::origin_of(&service.url).unwrap(),
        "cred_fp":catalog::fingerprint("fixture-key"),"added_at":0}]}).to_string()).unwrap();
    let shot = work.join("init.png");
    std::fs::write(&shot, PNG_2X2).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TERM", "xterm-256color")
        .env("TCHAT_CONFIG_DIR", &config)
        .env("TCHAT_USER_DIR", &user)
        .env("NATIVE_FIXTURE_KEY", "fixture-key")
        .env("TCHAT_SESSION_TS", "20261026T120000")
        .env("TCHAT_SESSION_PID", "26")
        .env("TZ", "UTC")
        .env("LANG", "C.UTF-8")
        .arg("-i")
        .arg(shot.display().to_string())
        .current_dir(&work);
    let mut tui = common::pty::PtyCli::spawn(cmd);
    tui.wait_screen("? for shortcuts");
    // 020 语义：`-i` 初始图待 UI 就绪自动提交（纯图首条，无用户文本）。
    tui.wait_screen("初始图已收");
    assert_eq!(service.count(), 1, "初始图恰随首次提交发送一次");
    let body: Value = serde_json::from_str(&service.requests()[0].body).unwrap();
    // anthropic-messages 结构：content 为块数组，含 base64 图片块。
    let messages = body["messages"].as_array().unwrap();
    let image_blocks: Vec<&Value> = messages
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|b| b["type"] == "image")
        .collect();
    assert_eq!(image_blocks.len(), 1, "恰一个图片块：{body}");
    assert_eq!(image_blocks[0]["source"]["type"], "base64");
    assert_eq!(image_blocks[0]["source"]["media_type"], "image/png");
    let expected = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(PNG_2X2)
    };
    assert_eq!(image_blocks[0]["source"]["data"], json!(expected), "载荷与本地图片一致");
    let image_message = messages
        .iter()
        .find(|m| m["role"] == "user" && m["content"].as_array()
            .is_some_and(|b| b.iter().any(|p| p["type"] == "image")))
        .expect("含图片块的用户消息");
    let texts: Vec<&str> = image_message["content"].as_array().unwrap()
        .iter().filter(|p| p["type"] == "text")
        .filter_map(|p| p["text"].as_str()).collect();
    // 020 语义：纯图首提交的文本块为空；environment_context（ctx）按 anthropic
    // 投影与图片合并进同一条 user 消息。
    assert!(texts.iter().any(|t| t.is_empty()), "纯图首提交含空文本块：{texts:?}");
    assert!(texts.iter().all(|t| t.is_empty() || t.starts_with("<environment_context>")),
        "其余仅 ctx：{texts:?}");
    tui.send("exit");
    let (output, code) = tui.finish();
    assert_eq!(code, Some(0), "{output}");
}

// ---------- 030 M4：搜索模式声明经五协议真实出站转换 ----------

/// S11（请求侧）：tool_search＋固定资源工具＋已加载动态定义进入各协议
/// 实际序列化请求（本地 HTTP mock 捕获线上字节）；普通函数格式各自正确；
/// 不注入任何协议原生搜索类型（无 tool_search_call / custom search item）。
#[test]
fn ts30_search_mode_tools_convert_across_five_protocols() {
    let protocols = [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ];
    for protocol in protocols {
        let overview = r#"{"sources":[{"server_id":"fx","tool_count":1}]}"#;
        let ts_def = terminal_chat::tool_search::tool_definition(overview);
        let resource_def =
            json!({"type": "function", "function": {"name": "list_mcp_resources"}});
        let loaded_def = json!({"type": "function", "function": {
            "name": "mcp__fx__send_mail",
            "description": "Send an email message to a recipient",
            "parameters": {"type": "object",
                           "properties": {"to": {"type": "string"}},
                           "required": ["to"]}}});
        let history = vec![terminal_chat::prompt::HistoryItem::Text(
            terminal_chat::prompt::HistoryMessage {
                role: terminal_chat::prompt::HistoryRole::User,
                content: "发邮件".into(),
                user_input_event: None,
                images: None,
                text_elements: Vec::new(),
            },
        )];
        let mut body = terminal_chat::prompt::build_turn_request_with_tool_search(
            &history, None, false, false, false, false, false, false,
            Some(&ts_def), &[resource_def, loaded_def],
        );
        body["_history"] = serde_json::to_value(&history).unwrap();

        let service = spawn_mock(vec![MockResp::json(
            200,
            final_response(protocol, "fin").to_string(),
        )]);
        let conn = catalog::EffectiveConnection {
            base_url: service.url.clone(),
            protocol: protocol.into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo {
            id: "fixture-model".into(),
            max_output: Some(256),
            ..Default::default()
        };
        let agent = api::agent();
        api::chat_with_body(&conn, "fixture-key", &model, &body, &agent)
            .unwrap_or_else(|e| panic!("{protocol} 发送失败：{e}"));

        let requests = service.requests();
        assert_eq!(requests.len(), 1, "{protocol}：恰一次请求");
        let wire: Value = serde_json::from_str(&requests[0].body).unwrap();
        assert!(
            !requests[0].body.contains("tool_search_call"),
            "{protocol}：不得注入协议原生搜索类型"
        );
        // 各协议普通函数格式中两个关键名字均可见，且参数 Schema 保留
        let flat_names = |wire: &Value| -> Vec<String> {
            match protocol {
                "openai-chat" => wire["tools"].as_array().unwrap().iter()
                    .map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect(),
                "openai-responses" | "google-interactions" => wire["tools"].as_array().unwrap().iter()
                    .map(|t| t["name"].as_str().unwrap().to_string()).collect(),
                "anthropic-messages" => wire["tools"].as_array().unwrap().iter()
                    .map(|t| t["name"].as_str().unwrap().to_string()).collect(),
                _ => wire["tools"][0]["functionDeclarations"].as_array().unwrap().iter()
                    .map(|t| t["name"].as_str().unwrap().to_string()).collect(),
            }
        };
        let names = flat_names(&wire);
        assert!(names.contains(&"tool_search".to_string()), "{protocol}：{names:?}");
        assert!(names.contains(&"mcp__fx__send_mail".to_string()), "{protocol}：{names:?}");
        assert!(names.contains(&"list_mcp_resources".to_string()), "{protocol}：{names:?}");
        // 已加载定义的参数（required 原样保留）
        let body_text = &requests[0].body;
        assert!(body_text.contains("\"to\""), "{protocol}：参数 Schema 须保留");
        assert!(body_text.contains("\"required\""), "{protocol}：required 须保留");
    }
}

/// S17：96 工具单服务器夹具、仅需 2 工具的任务——开启搜索后首次 tools
/// 数组字节与**使用两项工具后**的最终完整请求字节（含新增搜索历史）均须
/// 小于关闭对照；五协议分别计量（`measure_request_body_bytes`＝实际出站
/// 字节口径）。字节数不当作 token/费用/缓存收益。
#[test]
fn ts30_context_effect_bytes_search_vs_full_declaration() {
    use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord};
    use terminal_chat::tool_search::{
        build_catalog, catalog_entry_from_parts, success_result, tool_definition,
    };

    // 96 工具：仅 2 项含 email/weather 用途词（本任务要用的两工具），
    // 其余为互不重叠的通用描述——查询一次恰命中 2（96→0→2 冻结口径）。
    let entries: Vec<_> = (0..96)
        .map(|i| {
            let desc = if i == 0 {
                "Send an email message to a recipient"
            } else if i == 80 {
                "Fetch the weather forecast for a city"
            } else {
                "generic utility task for the fixture"
            };
            catalog_entry_from_parts(
                &format!("mcp__fx__tool_{i:02}"),
                "fx",
                &format!("tool_{i:02}"),
                Some(desc),
                &["param_alpha", "param_beta"],
                None,
            )
            .expect("夹具条目")
        })
        .collect();
    let all_defs: Vec<Value> = entries.iter().map(|e| e.definition()).collect();

    // 任务历史（两工具各一次调用＋结果）
    let mut history = vec![HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: "给 a@b.c 发邮件并查 c 城天气".into(),
        user_input_event: None,
        images: None,
        text_elements: Vec::new(),
    })];
    let push_call = |history: &mut Vec<HistoryItem>, id: &str, name: &str, args: &str| {
        history.push(HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: id.into(),
                name: name.into(),
                arguments: args.into(),
            }],
            content: None,
        });
    };
    let push_result = |history: &mut Vec<HistoryItem>, id: &str, content: &str| {
        history.push(HistoryItem::ToolResult {
            tool_call_id: id.into(),
            content: content.into(),
            display: None,
            image: None,
        });
    };

    // 关闭模式：请求 1（声明 96）→ 调 tool_00 → 结果 → 调 tool_80 → 结果
    let mut off_history = history.clone();
    push_call(&mut off_history, "o1", "mcp__fx__tool_00", r#"{"param_alpha":"a@b.c"}"#);
    push_result(&mut off_history, "o1", r#"{"ok":true,"sent":true}"#);
    push_call(&mut off_history, "o2", "mcp__fx__tool_80", r#"{"param_alpha":"c"}"#);
    push_result(&mut off_history, "o2", r#"{"ok":true,"temp":"22C"}"#);

    // 开启模式：请求 1（仅 tool_search）→ 搜索（命中 2）→ 调两工具 → 结果
    let catalog = build_catalog(entries.clone());
    let both: Vec<_> = catalog.search("email weather", 8);
    assert_eq!(both.len(), 2, "一次搜索恰命中本任务两项工具");
    let search_result = success_result(&both).unwrap();
    let ts_def = tool_definition(&catalog.sources_json());
    let mut on_history = history;
    push_call(
        &mut on_history,
        "s1",
        "tool_search",
        r#"{"query":"email weather","limit":8}"#,
    );
    push_result(&mut on_history, "s1", &search_result.to_string());
    push_call(&mut on_history, "s2", "mcp__fx__tool_00", r#"{"param_alpha":"a@b.c"}"#);
    push_result(&mut on_history, "s2", r#"{"ok":true,"sent":true}"#);
    push_call(&mut on_history, "s3", "mcp__fx__tool_80", r#"{"param_alpha":"c"}"#);
    push_result(&mut on_history, "s3", r#"{"ok":true,"temp":"22C"}"#);

    let loaded_defs: Vec<Value> = both.iter().map(|e| e.definition()).collect();

    // 首次请求 tools 数组字节数（同一用户消息；关闭＝内置＋96 动态，
    // 开启＝内置＋tool_search——真实组装入口）
    let first_history = vec![HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: "给 a@b.c 发邮件并查 c 城天气".into(),
        user_input_event: None,
        images: None,
        text_elements: Vec::new(),
    })];
    let first_off_body = terminal_chat::prompt::build_turn_request_with_tool_search(
        &first_history, None, false, false, false, false, false, false, None, &all_defs,
    );
    let first_on_body = terminal_chat::prompt::build_turn_request_with_tool_search(
        &first_history, None, false, false, false, false, false, false,
        Some(&ts_def), &[],
    );
    let first_off_tools = serde_json::to_string(&first_off_body["tools"]).unwrap().len();
    let first_on_tools = serde_json::to_string(&first_on_body["tools"]).unwrap().len();
    println!("TS30-S17 first_tools off={first_off_tools}B on={first_on_tools}B");
    assert!(
        first_on_tools < first_off_tools,
        "首次 tools 数组：开启 {first_on_tools}B 须小于关闭 {first_off_tools}B"
    );

    // 最终完整请求（关闭：96 定义；开启：tool_search＋2 定义＋搜索历史）
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic-messages",
        "google-interactions",
        "google-generate-content",
    ] {
        let conn = catalog::EffectiveConnection {
            base_url: "http://127.0.0.1:9".into(),
            protocol: protocol.into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo {
            id: "fixture-model".into(),
            max_output: Some(4096),
            ..Default::default()
        };
        let mut off_body =
            terminal_chat::prompt::build_turn_request_with_tool_search(&off_history, None, false, false, false, false, false, false, None, &all_defs);
        off_body["_history"] = serde_json::to_value(&off_history).unwrap();
        let mut on_body =
            terminal_chat::prompt::build_turn_request_with_tool_search(
                &on_history, None, false, false, false, false, false, false,
                Some(&ts_def), &loaded_defs,
            );
        on_body["_history"] = serde_json::to_value(&on_history).unwrap();
        let off_bytes = terminal_chat::api::measure_request_body_bytes(
            &conn, "k", &model, &off_body,
        )
        .unwrap();
        let on_bytes = terminal_chat::api::measure_request_body_bytes(
            &conn, "k", &model, &on_body,
        )
        .unwrap();
        println!("TS30-S17 final_request {protocol} off={off_bytes}B on={on_bytes}B");
        assert!(
            on_bytes < off_bytes,
            "{protocol}：最终完整请求（含搜索历史）开启 {on_bytes}B 须小于关闭 {off_bytes}B"
        );
    }
}
