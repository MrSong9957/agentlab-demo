//! 020 用户附图集成验收：提交准备（能力→读取→准备）、turn 入史、纯图消息、
//! 请求投影、消息链路（队列语义由 TUI 驱动层另测）。真实图片＋本地临时目录，
//! 网络边界本地 mock。

mod common;

use base64::Engine as _;

use common::{spawn_mock, MockResp, TempDir};
use serde_json::{json, Value};
use std::path::Path;
use terminal_chat::catalog;
use terminal_chat::prompt::{HistoryItem, UserImagePayload};
use terminal_chat::turn::{self, ToolMode};
use terminal_chat::proc_session::ProcessManager;

fn png_bytes(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let img = image::ImageBuffer::from_pixel(width, height, image::Rgba(rgba));
    let mut cursor = std::io::Cursor::new(Vec::new());
    img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
    cursor.into_inner()
}

fn write_png(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

// ---------- L1：准备接口 ----------

/// 能力确认＋真实文件 → 准备后 data URL（复用 019 核心：小图源字节保留）。
#[test]
fn prepare_user_images_success_reuses_core() {
    let dir = TempDir::new("uimg-prep");
    let bytes = png_bytes(3, 2, [10, 20, 30, 255]);
    let p = write_png(dir.path(), "a.png", &bytes);
    let urls = terminal_chat::view_image::prepare_user_images(&[p.clone()], true, None).unwrap();
    assert_eq!(urls.len(), 1);
    let url = &urls[0];
    assert!(url.starts_with("data:image/png;base64,"));
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        bytes,
        "小 PNG 源字节保留（复用 019 核心）"
    );
}

/// 能力拒绝先于文件打开：能力未知＋缺失路径 → 只报能力错误（A11）。
#[test]
fn prepare_user_images_capability_gate_before_open() {
    let payload = terminal_chat::view_image::prepare_user_images(
        &[std::path::PathBuf::from("/definitely/missing/x.png")],
        false,
        None,
    )
    .unwrap_err();
    assert!(
        payload.contains("未确认支持图片输入"),
        "能力错误优先于文件错误（证明未读文件）：{payload}"
    );
}

/// 坏图/缺失/目录明确失败；多图时逐图错误带路径。
#[test]
fn prepare_user_images_invalid_paths_fail_with_path() {
    let dir = TempDir::new("uimg-bad");
    std::fs::create_dir_all(dir.path().join("d")).unwrap();
    let err = terminal_chat::view_image::prepare_user_images(
        &[dir.path().join("d"), dir.path().join("nope.png")],
        true,
        None,
    )
    .unwrap_err();
    assert!(err.contains("d") || err.contains("普通文件"), "目录错误：{err}");
    let err = terminal_chat::view_image::prepare_user_images(&[dir.path().join("nope.png")], true, None)
        .unwrap_err();
    assert!(err.contains("nope.png"), "错误带路径：{err}");
}

/// CLI 合成助手：无 composer 场景按图片数生成前置结构化标记＋元素区间。
#[test]
fn compose_cli_message_prefix_markers_and_elements() {
    let (content, elements) = terminal_chat::view_image::compose_cli_user_message("看这些", 2);
    assert_eq!(content, "[Image #1] [Image #2] 看这些");
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0].byte_range.start, 0);
    assert_eq!(elements[0].byte_range.end, 10);
    assert_eq!(&content[elements[0].byte_range.start..elements[0].byte_range.end], "[Image #1]");
    assert_eq!(&content[elements[1].byte_range.start..elements[1].byte_range.end], "[Image #2]");
    // 纯图：仅标记
    let (content, elements) = terminal_chat::view_image::compose_cli_user_message("", 1);
    assert_eq!(content, "[Image #1]");
    assert_eq!(elements.len(), 1);
    // 无图：原文本
    let (content, elements) = terminal_chat::view_image::compose_cli_user_message("普通", 0);
    assert_eq!(content, "普通");
    assert!(elements.is_empty());
}

// ---------- L2：turn 入史与请求 ----------

fn fin(content: &str) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

/// 带 payloads 的用户轮：消息带 images 入史；下一请求同一条 user 消息
/// 携带图片部件；不产生 view_image 调用（tools 不含 view_image 时模型无从调用，
/// 且投影来自消息自身）。
#[test]
fn turn_enters_user_images_and_projects_in_same_message() {
    let dir = TempDir::new("uimg-turn");
    let bytes = png_bytes(2, 2, [5, 6, 7, 255]);
    let p = write_png(dir.path(), "m.png", &bytes);
    let urls = terminal_chat::view_image::prepare_user_images(&[p], true, None).unwrap();
    let payload = UserImagePayload { data_urls: urls, text_elements: Vec::new() };
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let mut sent: Vec<Value> = Vec::new();
    let mut sender = |body: &Value| -> Result<Value, String> {
        sent.push(body.clone());
        Ok(serde_json::from_str(&fin("图收到")).unwrap())
    };
    let mut notify = |_: turn::TurnNotify| {};
    let mut on_record = |_: &[HistoryItem]| {};
    let sources = terminal_chat::context::ContextSources::from_env();
    turn::run_user_turn_with_context_and_hosts(
        &mut history,
        "[Image #1] 看图",
        &mut sender,
        &mut notify,
        None,
        &mut on_record,
        ToolMode::User,
        &sources,
        &mgr,
        None,
        None,
        false,
        Some(&payload),
    )
    .unwrap();
    // 入史形态（015 上下文注入消息可能在前——按内容定位）
    let target = history
        .iter()
        .find(|h| matches!(h, HistoryItem::Text(m) if m.content.contains("看图")))
        .expect("用户消息入史");
    match target {
        HistoryItem::Text(m) => {
            assert_eq!(m.content, "[Image #1] 看图");
            assert_eq!(m.images.as_ref().map(|v| v.len()), Some(1));
        }
        other => panic!("形状：{other:?}"),
    }
    // 请求投影：同一条 user 消息 content 部件数组
    let msgs = sent[0]["messages"].as_array().unwrap();
    let user_msgs: Vec<&Value> = msgs
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .collect();
    assert_eq!(user_msgs.len(), 1, "恰一条图片用户消息：{msgs:?}");
    let parts = user_msgs[0]["content"].as_array().unwrap();
    assert!(parts.iter().any(|p| p["type"] == json!("text")));
    assert!(parts.iter().any(|p| p["type"] == json!("image_url")));
    // 可解码＝原字节
    let url = parts
        .iter()
        .find_map(|p| p["image_url"]["url"].as_str())
        .unwrap();
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        bytes
    );
}

/// 纯图消息（空文字）合法入史；纯空文本无图仍拒绝；消息文字/标记保留。
#[test]
fn pure_image_submission_and_blank_rules() {
    let dir = TempDir::new("uimg-pure");
    let p = write_png(dir.path(), "p.png", &png_bytes(2, 2, [1, 1, 1, 255]));
    let urls = terminal_chat::view_image::prepare_user_images(&[p], true, None).unwrap();
    let (content, elements) = terminal_chat::view_image::compose_cli_user_message("", 1);
    let full = UserImagePayload { data_urls: urls, text_elements: elements };
    let mut history = Vec::new();
    let mgr = ProcessManager::new();
    let mut sender = |_: &Value| -> Result<Value, String> {
        Ok(serde_json::from_str(&fin("ok")).unwrap())
    };
    let sources = terminal_chat::context::ContextSources::from_env();
    let result = turn::run_user_turn_with_context_and_hosts(
        &mut history,
        &content,
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
        Some(&full),
    );
    assert!(result.is_ok(), "纯图合法：{result:?}");
    let target = history
        .iter()
        .find(|h| matches!(h, HistoryItem::Text(m) if m.content == "[Image #1]"))
        .expect("纯图消息入史");
    match target {
        HistoryItem::Text(m) => assert_eq!(m.content, "[Image #1]"),
        other => panic!("{other:?}"),
    }
    // 空白文本＋无图仍拒绝
    let mut history2 = Vec::new();
    let err = turn::run_user_turn_with_context_and_hosts(
        &mut history2,
        "   ",
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
    )
    .unwrap_err();
    assert!(err.contains("空白"));
    // 空白文本＋有图：合法（标记或纯图）
    let mut history3 = Vec::new();
    let result = turn::run_user_turn_with_context_and_hosts(
        &mut history3,
        "  ",
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
        Some(&full),
    );
    assert!(result.is_ok(), "空文字有图可提交（A05）");
}

// ---------- L3（管道 e2e）：-i/--image 启动图片 ----------

use std::process::{Command, Stdio};

struct Fix {
    project: TempDir,
    mock: common::Mock,
}

impl Fix {
    fn new(responses: Vec<MockResp>, image_capable: bool) -> Self {
        let project = TempDir::new("uimg-cli");
        let cfg = project.path().join("config");
        let user = project.path().join("user");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let mock = spawn_mock(responses);
        let model = if image_capable {
            catalog::ModelInfo {
                id: "deepseek-flash".into(),
                input_modalities: Some(vec!["text".into(), "image".into()]),
                ..Default::default()
            }
        } else {
            catalog::ModelInfo { id: "deepseek-flash".into(), ..Default::default() }
        };
        let m = catalog::ManualRoot {
            selected_provider: Some("deepseek".into()),
            selected_model: Some("deepseek-flash".into()),
            memories: None,
            providers: [(
                "deepseek".to_string(),
                catalog::ManualProvider {
                    base_url: Some(mock.url.clone()),
                    key_ref: Some("env:TCHAT_UI_KEY".into()),
                    models: [("deepseek-flash".to_string(), model)].into_iter().collect(),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        };
        std::fs::write(cfg.join("manual.json"), serde_json::to_string_pretty(&m).unwrap())
            .unwrap();
        std::fs::write(
            user.join("authorized-origins.json"),
            json!({ "grants": [{ "origin": catalog::origin_of(&mock.url).unwrap(), "cred_fp": catalog::fingerprint("ui-key"), "added_at": 0 }] }).to_string(),
        )
        .unwrap();
        Fix { project, mock }
    }

    fn run(&self, args: &[&str], stdin_text: &str) -> (String, Option<i32>) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.current_dir(self.project.path())
            .env("TCHAT_CONFIG_DIR", self.project.path().join("config"))
            .env("TCHAT_USER_DIR", self.project.path().join("user"))
            .env("TCHAT_UI_KEY", "ui-key")
            .env("TERM", "xterm")
            .args(args);
        for k in ["TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
            cmd.env_remove(k);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("CLI 子进程启动失败");
        use std::io::Write as _;
        child.stdin.take().unwrap().write_all(stdin_text.as_bytes()).unwrap();
        let out = child.wait_with_output().expect("等待失败");
        (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code())
    }
}

fn fin_json(content: &str) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

/// A04：管道文字与启动图合并为首条消息；图片为准备后真实内容；标记前置。
#[test]
fn e2e_cli_image_flag_merges_with_first_line() {
    let work = TempDir::new("uimg-cli-a");
    let img = write_png(work.path(), "a.png", &png_bytes(4, 4, [200, 30, 40, 255]));
    let abs = img.display().to_string();
    let fix = Fix::new(vec![MockResp::json(200, fin_json("已处理"))], true);
    let (out, code) = fix.run(&["-i", &abs], "看这张\nexit\n");
    assert_eq!(fix.mock.count(), 1, "{out}");
    let body: Value = serde_json::from_str(&fix.mock.requests()[0].body).unwrap();
    // 同一条 user 消息：text 含前置标记＋文字；图片部件可解码＝源字节
    let img_msg = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == json!("user") && m["content"].is_array())
        .expect("图片用户消息");
    let parts = img_msg["content"].as_array().unwrap();
    let text = parts.iter().find(|p| p["type"] == json!("text")).unwrap();
    assert!(text["text"].as_str().unwrap().starts_with("[Image #1]"), "{text:?}");
    assert!(text["text"].as_str().unwrap().contains("看这张"));
    let url = parts
        .iter()
        .find_map(|p| p["image_url"]["url"].as_str())
        .unwrap();
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        png_bytes(4, 4, [200, 30, 40, 255]),
        "小图源字节保留"
    );
    assert_eq!(code, Some(0));
}

/// A04：重复参数＋逗号分隔形成有序附件；EOF 无文字纯图恰一次。
#[test]
fn e2e_cli_image_flag_multiple_and_pure_image_eof() {
    let work = TempDir::new("uimg-cli-b");
    let a = write_png(work.path(), "a.png", &png_bytes(2, 2, [1, 2, 3, 255]));
    let b = write_png(work.path(), "b.png", &png_bytes(3, 3, [4, 5, 6, 255]));
    let c = write_png(work.path(), "c.png", &png_bytes(2, 3, [7, 8, 9, 255]));
    let fix = Fix::new(vec![MockResp::json(200, fin_json("三张"))], true);
    // 重复参数 + 逗号分隔
    let (out, _code) = fix.run(
        &["-i", &a.display().to_string(), "--image", &format!("{},{}", b.display(), c.display())],
        "", // 空 stdin＝EOF 无文字——纯图恰一次（exit 文本按退出语义优先）
    );
    assert_eq!(fix.mock.count(), 1, "纯图恰一次请求：{out}");
    let body: Value = serde_json::from_str(&fix.mock.requests()[0].body).unwrap();
    let urls: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .filter_map(|p| p["image_url"]["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 3, "三张按序：{body:?}");
    let decode = |u: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(u.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap()
    };
    assert_eq!(decode(urls[0]), png_bytes(2, 2, [1, 2, 3, 255]), "顺序 a");
    assert_eq!(decode(urls[1]), png_bytes(3, 3, [4, 5, 6, 255]), "顺序 b");
    assert_eq!(decode(urls[2]), png_bytes(2, 3, [7, 8, 9, 255]), "顺序 c");
}

/// A11（行式形态）：能力未知＋启动图 → 明确报错、零请求、不自动转纯文本。
#[test]
fn e2e_cli_image_flag_capability_rejected_zero_request() {
    let work = TempDir::new("uimg-cli-c");
    let a = write_png(work.path(), "a.png", &png_bytes(2, 2, [1, 1, 1, 255]));
    let fix = Fix::new(vec![MockResp::json(200, fin_json("不应到达"))], false);
    let (out, _code) = fix.run(&["-i", &a.display().to_string()], "看图\nexit\n");
    assert_eq!(fix.mock.count(), 0, "零请求：{out}");
    assert!(out.contains("未确认支持图片输入"), "{out}");
}

/// A10（行式形态）：启动图文件缺失 → 明确报错、零请求、该输入不发送。
#[test]
fn e2e_cli_image_flag_missing_file_zero_request() {
    let fix = Fix::new(vec![MockResp::json(200, fin_json("不应到达"))], true);
    let (out, _code) = fix.run(&["-i", "/definitely/missing/x.png"], "看图\nexit\n");
    assert_eq!(fix.mock.count(), 0, "{out}");
    assert!(out.contains("x.png"), "错误带路径：{out}");
}

// ---------- L2：TUI 宿主提交链路（SubmittedTurn → 准备 → turn/rejected） ----------

use tchat_tui::chat_driver::{DriverEvent, SubmittedTurn};

fn sub(text: &str, paths: Vec<std::path::PathBuf>) -> SubmittedTurn {
    SubmittedTurn {
        text: text.to_string(),
        local_images: paths
            .into_iter()
            .map(|path| tchat_tui::bottom_pane::LocalImageAttachment {
                placeholder: String::new(),
                path,
            })
            .collect(),
        text_elements: Vec::new(),
    }
}

/// 宿主收到带附件提交：线程内准备→turn 入史→请求；附件文本由 composer 元素
/// 随 SubmittedTurn 携带（此处模拟 text_elements 为空、text 含标记的形态）。
#[test]
fn host_prepares_and_runs_turn_with_images() {
    let dir = TempDir::new("uimg-host");
    let p = write_png(dir.path(), "h.png", &png_bytes(2, 2, [9, 9, 9, 255]));
    // 独立配置/用户目录＋本地 mock
    let cfg = dir.path().join("config");
    let user = dir.path().join("user");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let mock = spawn_mock(vec![MockResp::json(200, fin_json("TUI 收到"))]);
    write_capable_manual(&cfg, &mock.url);
    seed_grant(&user, &mock.url);
    let _env = common::ENV_LOCK.lock().unwrap();
    std::env::set_var("TCHAT_CONFIG_DIR", &cfg);
    std::env::set_var("TCHAT_USER_DIR", &user);
    std::env::set_var("TCHAT_HOST_KEY", "host-key");
    let agent = terminal_chat::api::agent();
    let autosave = terminal_chat::session::Session::new(terminal_chat::session::sessions_dir());
    let host = std::sync::Arc::new(terminal_chat::tui_host::TuiHost::new(agent, autosave));
    let (tx, rx) = std::sync::mpsc::channel::<DriverEvent>();
    host.spawn_turn(sub("[Image #1] 看图", vec![p]), tx);
    // 排空事件直到 Final（图片准备在 turn 线程内完成）
    let mut final_text = None;
    for _ in 0..50 {
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(DriverEvent::Final(text)) => {
                final_text = Some(text);
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    std::env::remove_var("TCHAT_CONFIG_DIR");
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var("TCHAT_HOST_KEY");
    assert_eq!(final_text.as_deref(), Some("TUI 收到"));
    assert_eq!(mock.count(), 1, "恰一次请求");
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let urls: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .filter_map(|p| p["image_url"]["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 1, "用户图片部件：{body:?}");
    let b64 = urls[0].strip_prefix("data:image/png;base64,").unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        png_bytes(2, 2, [9, 9, 9, 255])
    );
    // 会话行保存图片（同一行）——由恢复系列 e2e 覆盖
}

fn write_capable_manual(cfg: &Path, base_url: &str) {
    let m = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(base_url.into()),
                key_ref: Some("env:TCHAT_HOST_KEY".into()),
                models: [(
                    "deepseek-flash".to_string(),
                    catalog::ModelInfo {
                        id: "deepseek-flash".into(),
                        input_modalities: Some(vec!["text".into(), "image".into()]),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(cfg.join("manual.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
}

fn seed_grant(user: &Path, base_url: &str) {
    std::fs::write(
        user.join("authorized-origins.json"),
        json!({ "grants": [{ "origin": catalog::origin_of(base_url).unwrap(), "cred_fp": catalog::fingerprint("host-key"), "added_at": 0 }] })
            .to_string(),
    )
    .unwrap();
}

/// 宿主准备失败（坏图）：SubmissionRejected（text＋路径＋原因），零请求，
/// 消息不入史。
#[test]
fn host_rejects_submission_on_bad_image() {
    let dir = TempDir::new("uimg-rej");
    let cfg = dir.path().join("config");
    let user = dir.path().join("user");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let mock = spawn_mock(vec![MockResp::json(200, fin_json("不应到达"))]);
    write_capable_manual(&cfg, &mock.url);
    seed_grant(&user, &mock.url);
    let _env = common::ENV_LOCK.lock().unwrap();
    std::env::set_var("TCHAT_CONFIG_DIR", &cfg);
    std::env::set_var("TCHAT_USER_DIR", &user);
    std::env::set_var("TCHAT_HOST_KEY", "host-key");
    let agent = terminal_chat::api::agent();
    let autosave = terminal_chat::session::Session::new(terminal_chat::session::sessions_dir());
    let host = std::sync::Arc::new(terminal_chat::tui_host::TuiHost::new(agent, autosave));
    let (tx, rx) = std::sync::mpsc::channel::<DriverEvent>();
    host.spawn_turn(sub("看图", vec![std::path::PathBuf::from("/missing/x.png")]), tx);
    let mut rejected = None;
    for _ in 0..10 {
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(DriverEvent::SubmissionRejected { text, image_paths, reason }) => {
                rejected = Some((text, image_paths, reason));
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    std::env::remove_var("TCHAT_CONFIG_DIR");
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var("TCHAT_HOST_KEY");
    let (text, paths, reason) = rejected.expect("应收到 SubmissionRejected");
    assert_eq!(text, "看图");
    assert_eq!(paths, vec![std::path::PathBuf::from("/missing/x.png")]);
    assert!(reason.contains("x.png"), "{reason}");
    assert_eq!(mock.count(), 0, "零请求");
}

// ---------- A14/A17：用户附件超限零请求；恢复不重读源图 ----------

/// A14（用户侧）：两张近限噪声图合计超 10 MiB 请求上限→零 HTTP（行式 -i）。
#[test]
fn e2e_cli_user_images_request_limit_zero_send() {
    let work = TempDir::new("uimg-cap");
    // RGB 噪声 1350×1000 ≈3.86 MiB/张（<4 MiB 准备上限），两张 ≈10.3 MiB > 10 MiB
    let mut make = |name: &str, seed: u64| -> std::path::PathBuf {
        let mut img = image::ImageBuffer::new(1350, 1000);
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        for (_x, _y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([next(), next(), next()]);
        }
        let mut cursor = std::io::Cursor::new(Vec::new());
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let p = work.path().join(name);
        std::fs::write(&p, cursor.into_inner()).unwrap();
        p
    };
    let a = make("big1.png", 7);
    let b = make("big2.png", 13);
    let fix = Fix::new(vec![MockResp::json(200, fin_json("不应到达"))], true);
    let (out, _code) = fix.run(&["-i", &a.display().to_string(), "--image", &b.display().to_string()], "看\nexit\n");
    assert_eq!(fix.mock.count(), 0, "超限零发送：{out}");
    assert!(out.contains("10 MiB"), "明确报错：{out}");
    assert!(!out.contains(";base64,"), "输出无载荷：{out}");
}

/// A17：首轮带图保存→删除源图→新进程 /resume＋输入→请求仍携带当时字节。
#[test]
fn e2e_cli_user_images_resume_after_source_deleted() {
    let work = TempDir::new("uimg-resume");
    let bytes = png_bytes(5, 4, [90, 80, 70, 255]);
    let img = write_png(work.path(), "gone.png", &bytes);
    let fix = Fix::new(
        vec![
            MockResp::json(200, fin_json("第一轮完成")),
            MockResp::json(200, fin_json("第二轮完成")),
        ],
        true,
    );
    let (out, code) = fix.run(&["-i", &img.display().to_string()], "看\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(fix.mock.count(), 1, "{out}");
    // 删除源图后恢复：第二轮请求的图片字节＝当时准备后字节（小图＝源字节）
    std::fs::remove_file(&img).expect("删除源图");
    let (out2, _c) = fix.run(&[], "/resume\n继续\nexit\n");
    assert!(out2.contains("恢复") || out2.contains("已装载"), "恢复输出：{out2}");
    assert_eq!(fix.mock.count(), 2, "{out2}");
    let body: Value = serde_json::from_str(&fix.mock.requests()[1].body).unwrap();
    let urls: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .filter_map(|p| p["image_url"]["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 1, "恢复后恰一图：{body:?}");
    let b64 = urls[0].strip_prefix("data:image/png;base64,").unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        bytes,
        "恢复请求字节＝当时图片（源已删除）"
    );
}

// ---------- §2.3 修复验收：队列暂停／cell 回收／准备期取消 ----------

/// §2.3：入史前拒绝后剩余 Q 不得自动续发（暂停），用户重新提交后恢复。
/// 宿主侧拒绝（能力未知）＋驱动队列语义：用真实 TUI 宿主无法直接观察驱动
/// 队列——用驱动/宿主组合的 PTY 验证（见 e2e S101）；此处先钉宿主侧拒绝
/// 幂等（重复拒绝不产生迟到提交）。
#[test]
fn host_reject_is_terminal_no_retry() {
    let dir = TempDir::new("uimg-rej2");
    let cfg = dir.path().join("config");
    let user = dir.path().join("user");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let mock = spawn_mock(vec![MockResp::json(200, fin_json("不应到达"))]);
    write_capable_manual(&cfg, &mock.url);
    seed_grant(&user, &mock.url);
    let _env = common::ENV_LOCK.lock().unwrap();
    std::env::set_var("TCHAT_CONFIG_DIR", &cfg);
    std::env::set_var("TCHAT_USER_DIR", &user);
    std::env::set_var("TCHAT_HOST_KEY", "host-key");
    let agent = terminal_chat::api::agent();
    let autosave = terminal_chat::session::Session::new(terminal_chat::session::sessions_dir());
    let host = std::sync::Arc::new(terminal_chat::tui_host::TuiHost::new(agent, autosave));
    let (tx, rx) = std::sync::mpsc::channel::<DriverEvent>();
    host.spawn_turn(sub("看图", vec![std::path::PathBuf::from("/missing/y.png")]), tx);
    let mut rejected = 0;
    for _ in range_10() {
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(DriverEvent::SubmissionRejected { .. }) => rejected += 1,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    std::env::remove_var("TCHAT_CONFIG_DIR");
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var("TCHAT_HOST_KEY");
    assert_eq!(rejected, 1, "恰一次拒绝终态，无迟到重复");
    assert_eq!(mock.count(), 0);
}

fn range_10() -> std::ops::Range<i32> {
    0..10
}

// ---------- A12/A10 补验收 ----------

/// A12：准备期取消检查点——取消标志置位后，能力检查通过、文件存在，
/// prepare_user_images 仍在读取前检查点返回取消（消息不入史路径）。
#[test]
fn prepare_user_images_cancel_checkpoint_before_read() {
    use std::sync::atomic::AtomicBool;
    let dir = TempDir::new("uimg-cancel");
    let p = write_png(dir.path(), "c.png", &png_bytes(2, 2, [3, 3, 3, 255]));
    let cancel = AtomicBool::new(true);
    let err = terminal_chat::view_image::prepare_user_images(
        &[p],
        /*capable*/ true,
        Some(&cancel),
    )
    .unwrap_err();
    assert!(err.contains("取消"), "取消先于读取成功路径：{err}");
    // 未取消时同一文件成功（对照）
    let cancel2 = AtomicBool::new(false);
    assert!(terminal_chat::view_image::prepare_user_images(&[write_png(dir.path(), "c2.png", &png_bytes(2, 2, [3, 3, 3, 255]))], true, Some(&cancel2)).is_ok());
}

/// A10：拒绝恢复的草稿合并顺序——pane 级由 TUI 测试覆盖；此处钉宿主侧
/// 拒绝载荷完整性（文本＋全部路径原样回传，供驱动按 D0＋Dnew 合并）。
#[test]
fn rejected_submission_carries_full_draft_for_restore() {
    let dir = TempDir::new("uimg-restore");
    let cfg = dir.path().join("config");
    let user = dir.path().join("user");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let mock = spawn_mock(vec![MockResp::json(200, fin_json("不应到达"))]);
    // 能力未知 manual
    let m = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(mock.url.clone()),
                key_ref: Some("env:TCHAT_HOST_KEY".into()),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(cfg.join("manual.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
    seed_grant(&user, &mock.url);
    let _env = common::ENV_LOCK.lock().unwrap();
    std::env::set_var("TCHAT_CONFIG_DIR", &cfg);
    std::env::set_var("TCHAT_USER_DIR", &user);
    std::env::set_var("TCHAT_HOST_KEY", "host-key");
    let agent = terminal_chat::api::agent();
    let autosave = terminal_chat::session::Session::new(terminal_chat::session::sessions_dir());
    let host = std::sync::Arc::new(terminal_chat::tui_host::TuiHost::new(agent, autosave));
    let (tx, rx) = std::sync::mpsc::channel::<DriverEvent>();
    let paths = vec![
        std::path::PathBuf::from("/missing/a.png"),
        std::path::PathBuf::from("/missing/b.png"),
    ];
    let expect_paths = paths.clone();
    host.spawn_turn(sub("原稿文本", paths), tx);
    let mut got = None;
    for _ in 0..10 {
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(ev) => {
                if let DriverEvent::SubmissionRejected { text, image_paths, reason } = ev {
                    got = Some((text, image_paths, reason));
                    break;
                }
            }
            Err(_) => break,
            Ok(_) => {}
        }
    }
    std::env::remove_var("TCHAT_CONFIG_DIR");
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var("TCHAT_HOST_KEY");
    let (text, image_paths, _reason) = got.expect("Rejected");
    assert_eq!(text, "原稿文本");
    assert_eq!(image_paths, expect_paths, "全部附件路径原样回传（合并恢复依据）");
    assert_eq!(mock.count(), 0);
}
