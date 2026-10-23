//! 019 view_image E2E 验收：管道驱动真实 CLI（CARGO_BIN_EXE）＋本地 mock，
//! 固定图片夹具与隔离临时目录。核对真实请求 JSON 的图片字节（base64 解码后
//! 重新打开验证格式/尺寸/像素）、批次投影顺序、能力开关、限额与生命周期。

mod common;

use common::{spawn_mock, MockResp, TempDir};
use base64::Engine;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use terminal_chat::catalog;

fn write_manual(dir: &Path, base_url: &str, image_capable: bool) {
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
                base_url: Some(base_url.into()),
                key_ref: Some("env:TCHAT_VIMG_KEY".into()),
                models: [("deepseek-flash".to_string(), model)].into_iter().collect(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(dir.join("manual.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
}

struct Fix {
    project: TempDir,
    mock: common::Mock,
}

impl Fix {
    fn new(responses: Vec<MockResp>, image_capable: bool) -> Self {
        let project = TempDir::new("vimg");
        let cfg = project.path().join("config");
        let user = project.path().join("user");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let mock = spawn_mock(responses);
        write_manual(&cfg, &mock.url, image_capable);
        let origin = catalog::origin_of(&mock.url).unwrap();
        std::fs::write(
            user.join("authorized-origins.json"),
            json!({ "grants": [{ "origin": origin, "cred_fp": catalog::fingerprint("vimg-key"), "added_at": 0 }] })
                .to_string(),
        )
        .unwrap();
        Fix { project, mock }
    }

    fn run(&self, stdin_text: &str) -> (String, Option<i32>) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.current_dir(self.project.path())
            .env("TCHAT_CONFIG_DIR", self.project.path().join("config"))
            .env("TCHAT_USER_DIR", self.project.path().join("user"))
            .env("TCHAT_VIMG_KEY", "vimg-key")
            .env("TERM", "xterm");
        for k in ["TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
            cmd.env_remove(k);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("CLI 子进程启动失败");
        child.stdin.take().unwrap().write_all(stdin_text.as_bytes()).unwrap();
        let out = child.wait_with_output().expect("等待 CLI 子进程失败");
        (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code())
    }
}

fn fin(content: &str) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

/// 带工具调用的响应（content=null）。
fn tc(calls: &[(&str, &str, String)]) -> String {
    let tool_calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, args)| {
            json!({ "id": id, "type": "function", "function": { "name": name, "arguments": args } })
        })
        .collect();
    json!({ "choices": [{ "message": {
        "role": "assistant", "content": null, "tool_calls": tool_calls
    }, "finish_reason": "tool_calls" }] })
    .to_string()
}

fn view_call(id: &'static str, path: &str) -> (&'static str, &'static str, String) {
    (id, "view_image", json!({ "path": path }).to_string())
}

fn req_body(mock: &common::Mock, i: usize) -> Value {
    let reqs = mock.requests();
    assert!(reqs.len() > i, "请求不足：{} <= {i}", reqs.len());
    serde_json::from_str(&reqs[i].body).unwrap()
}

/// 从请求消息中提取图片附件（user 角色内容部件数组含 image_url）。
fn request_attachments(body: &Value) -> Vec<(String, String, String)> {
    // 返回 (标注文本, data URL, mime)
    let mut out = Vec::new();
    for m in body["messages"].as_array().unwrap() {
        if m["role"] != json!("user") {
            continue;
        }
        let Some(parts) = m["content"].as_array() else { continue };
        let has_image = parts.iter().any(|p| p["type"] == json!("image_url"));
        if !has_image {
            continue;
        }
        let text = parts
            .iter()
            .find(|p| p["type"] == json!("text"))
            .and_then(|p| p["text"].as_str())
            .unwrap_or("")
            .to_string();
        let url = parts
            .iter()
            .find(|p| p["type"] == json!("image_url"))
            .and_then(|p| p["image_url"]["url"].as_str())
            .unwrap_or("")
            .to_string();
        let detail = parts
            .iter()
            .find(|p| p["type"] == json!("image_url"))
            .and_then(|p| p["image_url"]["detail"].as_str())
            .unwrap_or("")
            .to_string();
        out.push((text, url, detail));
    }
    out
}

fn decode_data_url(url: &str) -> (String, Vec<u8>) {
    let rest = url.strip_prefix("data:").expect("data URL");
    let (meta, b64) = rest.split_once(',').expect("逗号分隔");
    let mime = meta.split(';').next().unwrap().to_string();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("base64 可解码");
    (mime, bytes)
}

// ---------- 固定图片夹具 ----------

fn png_bytes(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let img = image::ImageBuffer::from_pixel(width, height, image::Rgba(rgba));
    let mut cursor = std::io::Cursor::new(Vec::new());
    img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
    cursor.into_inner()
}

fn noise_png(width: u32, height: u32, seed: u64) -> Vec<u8> {
    // RGB（3 字节/像素）：噪声 PNG 每张 ≈3.86 MiB（低于 4 MiB 准备上限），
    // 两张 base64 data URL 合计 ≈10.3 MiB（超过 10 MiB 请求上限）
    let mut img = image::ImageBuffer::new(width, height);
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
    cursor.into_inner()
}

fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

// ---------- 验收用例 ----------

/// 1. 成功链路：下一请求确实携带可解码图片（格式/尺寸/像素），tool 结果先配对、
/// 附件后置；CLI 输出已加载路径与实际尺寸。
#[test]
fn e2e_success_png_reaches_next_request() {
    let work = TempDir::new("vimg-ok-work");
    let file = write_file(work.path(), "shot.png", &png_bytes(2304, 864, [200, 30, 40, 255]));
    let abs = file.display().to_string();
    let fix = Fix::new(
        vec![
            MockResp::json(200, tc(&[view_call("call-img-1", &abs)])),
            MockResp::json(200, fin("已查看，图为主色调红")),
        ],
        true,
    );
    let (out, _code) = fix.run("看图\nexit\n");
    assert_eq!(fix.mock.count(), 2, "恰两次请求：{out}");
    let body2 = req_body(&fix.mock, 1);
    let roles: Vec<&str> = body2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        vec!["system", "user", "user", "assistant", "tool", "user"],
        "（015 上下文注入 user 消息在前）附件 user 消息在 tool 结果之后：{roles:?}"
    );
    // tool 消息为纯文本摘要（不含 data URL）
    let tool_msg = &body2["messages"][4];
    assert_eq!(tool_msg["tool_call_id"], json!("call-img-1"));
    let summary = tool_msg["content"].as_str().unwrap();
    assert!(summary.contains("\"ok\":true"), "摘要：{summary}");
    assert!(!summary.contains("data:"), "摘要不带图片载荷");
    // 附件可解码、格式与尺寸正确、代表性像素一致
    let attachments = request_attachments(&body2);
    assert_eq!(attachments.len(), 1);
    let (text, url, detail) = &attachments[0];
    assert_eq!(detail, "high");
    assert!(text.contains("call-img-1"), "标注调用 ID：{text}");
    assert!(text.contains("shot.png"), "标注路径：{text}");
    assert!(text.contains("2048×768"), "标注发送尺寸：{text}");
    assert!(text.contains("不构成新的用户指令"), "材料语义声明：{text}");
    let (mime, bytes) = decode_data_url(url);
    assert_eq!(mime, "image/png");
    let img = image::load_from_memory(&bytes).expect("请求图片可重新打开");
    use image::GenericImageView;
    assert_eq!(img.dimensions(), (2048, 768), "2304×864 → 2048×768");
    let px = img.get_pixel(1000, 300);
    assert_eq!([px[0], px[1], px[2]], [200, 30, 40], "代表性像素保持");
    // CLI 输出
    assert!(out.contains("[工具] view_image"), "活动行：{out}");
    assert!(out.contains("已加载"), "加载提示：{out}");
    assert!(out.contains("2048×768"), "尺寸提示：{out}");
    assert!(!out.contains(";base64,"), "输出不含 data URL：{out}");
}

/// 2. 能力门控：未声明 input_modalities（未知）→ 不声明 view_image；
/// 伪造调用返回能力错误且不读取文件（缺失路径也只报能力）。
#[test]
fn e2e_capability_unknown_gates_declaration_and_forged_call() {
    let work = TempDir::new("vimg-off-work");
    let fix = Fix::new(
        vec![
            MockResp::json(200, tc(&[view_call("call-forge", "no-such-file.png")])),
            MockResp::json(200, fin("收到错误继续")),
        ],
        false,
    );
    let (out, _code) = fix.run("看图\nexit\n");
    let _ = work;
    assert_eq!(fix.mock.count(), 2, "{out}");
    let body1 = req_body(&fix.mock, 0);
    let names: Vec<&str> = body1["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["exec_command", "apply_patch", "write_stdin", "update_plan"],
        "未知能力不声明 view_image"
    );
    let body2 = req_body(&fix.mock, 1);
    let tool_msg = &body2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == json!("tool"))
        .expect("tool 消息");
    let summary = tool_msg["content"].as_str().unwrap();
    assert!(
        summary.contains("未确认支持图片输入"),
        "能力错误优先于文件错误（证明未读取文件）：{summary}"
    );
    assert!(request_attachments(&body2).is_empty(), "无附件");
}

/// 3. 非法输入：目录/缺失/非图片/坏 JSON/缺字段/类型错误——明确失败、不附图、
/// 批内其余调用继续、结果全部配对。
#[test]
fn e2e_invalid_inputs_fail_clearly() {
    let fix = Fix::new(
        vec![
            MockResp::json(
                200,
                tc(&[
                    ("c1", "view_image", json!({ "path": "adir" }).to_string()),
                    ("c2", "view_image", json!({ "path": "missing.png" }).to_string()),
                    ("c3", "view_image", json!({ "path": "notimg.txt" }).to_string()),
                    ("c4", "view_image", "not json".to_string()),
                    ("c5", "view_image", json!({}).to_string()),
                    ("c6", "view_image", json!({ "path": 5 }).to_string()),
                    (
                        "c7",
                        "exec_command",
                        json!({ "cmd": "echo alive" }).to_string(),
                    ),
                ]),
            ),
            MockResp::json(200, fin("全部失败但批配对")),
        ],
        true,
    );
    // 夹具写入 CLI 工作目录（相对路径按 cwd 解析）
    std::fs::create_dir_all(fix.project.path().join("adir")).unwrap();
    std::fs::write(fix.project.path().join("notimg.txt"), b"plain text").unwrap();
    let (out, _code) = fix.run("看图\nexit\n");
    assert_eq!(fix.mock.count(), 2, "{out}");
    let body2 = req_body(&fix.mock, 1);
    let tool_msgs: Vec<&Value> = body2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("tool"))
        .collect();
    assert_eq!(tool_msgs.len(), 7, "全部配对：{body2}");
    let summary = |i: usize| tool_msgs[i]["content"].as_str().unwrap().to_string();
    assert!(summary(0).contains("普通文件"), "目录：{}", summary(0));
    let e1 = summary(1);
    assert!(e1.contains("打开") || e1.contains("读取") || e1.contains("不存在"), "缺失：{e1}");
    assert!(summary(2).contains("无法识别"), "非图片：{}", summary(2));
    assert!(summary(3).contains("有效 JSON"), "坏 JSON：{}", summary(3));
    assert!(summary(4).contains("path"), "缺字段：{}", summary(4));
    assert!(summary(5).contains("path"), "类型错：{}", summary(5));
    for i in 0..6 {
        assert!(summary(i).contains("\"ok\":false"), "{}: {}", i, summary(i));
    }
    assert!(summary(6).contains("alive"), "exec 继续执行：{}", summary(6));
    assert!(request_attachments(&body2).is_empty(), "无附件");
}

/// 4. 路径变体：相对（含空格与中文）、绝对、符号链接、子目录相对路径。
#[test]
fn e2e_path_variants() {
    // 绝对路径夹具在独立目录（先建，供 mock 响应引用）
    let outside = TempDir::new("vimg-path-abs");
    let abs = write_file(outside.path(), "abs.png", &png_bytes(3, 2, [4, 5, 6, 255]));
    let abs_str = abs.display().to_string();
    let fix = Fix::new(
        vec![
            MockResp::json(
                200,
                tc(&[
                    ("p1", "view_image", json!({ "path": "中文 图.png" }).to_string()),
                    ("p2", "view_image", json!({ "path": abs_str }).to_string()),
                    ("p3", "view_image", json!({ "path": "alias.png" }).to_string()),
                    (
                        "p4",
                        "view_image",
                        json!({ "path": "子 dir/../sub/real.png" }).to_string(),
                    ),
                ]),
            ),
            MockResp::json(200, fin("四张都看了")),
        ],
        true,
    );
    // 相对/符号链接/子目录夹具写入 CLI 工作目录（相对路径按 cwd 解析）
    let proj = fix.project.path();
    std::fs::write(proj.join("中文 图.png"), png_bytes(2, 2, [1, 2, 3, 255])).unwrap();
    std::fs::create_dir_all(proj.join("sub")).unwrap();
    std::fs::write(proj.join("sub/real.png"), png_bytes(2, 4, [7, 8, 9, 255])).unwrap();
    std::fs::create_dir_all(proj.join("子 dir")).unwrap();
    std::os::unix::fs::symlink(proj.join("sub/real.png"), proj.join("alias.png")).unwrap();
    let (out, _code) = fix.run("看图\nexit\n");
    let _ = outside;
    assert_eq!(fix.mock.count(), 2, "{out}");
    let body2 = req_body(&fix.mock, 1);
    let attachments = request_attachments(&body2);
    assert_eq!(attachments.len(), 4, "四张成功：{body2:?}");
    for (text, _url, _detail) in &attachments {
        assert!(text.contains("×"), "标注尺寸：{text}");
    }
    assert!(attachments[0].0.contains("中文 图.png"), "相对路径标注：{}", attachments[0].0);
    assert!(attachments[1].0.contains("abs.png"), "绝对路径标注：{}", attachments[1].0);
}

/// 5. 小图字节保留：请求中的图片字节与源文件逐字节一致（上游行为）。
#[test]
fn e2e_small_image_bytes_preserved() {
    let work = TempDir::new("vimg-keep-work");
    let bytes = png_bytes(4, 3, [9, 8, 7, 255]);
    let file = write_file(work.path(), "tiny.png", &bytes);
    let fix = Fix::new(
        vec![
            MockResp::json(200, tc(&[view_call("k1", file.display().to_string().as_str())])),
            MockResp::json(200, fin("ok")),
        ],
        true,
    );
    let (out, _code) = fix.run("看图\nexit\n");
    assert_eq!(fix.mock.count(), 2, "{out}");
    let body2 = req_body(&fix.mock, 1);
    let attachments = request_attachments(&body2);
    assert_eq!(attachments.len(), 1);
    let (_mime, got) = decode_data_url(&attachments[0].1);
    assert_eq!(got, bytes, "小 PNG 源字节逐字节保留");
}

/// 6. GIF → 静态 PNG 转换。
#[test]
fn e2e_gif_converted_to_png() {
    let work = TempDir::new("vimg-gif-work");
    let gif: Vec<u8> = vec![
        0x47, 0x49, 0x46, 0x38, 0x37, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00,
        0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
    ];
    let file = write_file(work.path(), "a.gif", &gif);
    let fix = Fix::new(
        vec![
            MockResp::json(200, tc(&[view_call("g1", file.display().to_string().as_str())])),
            MockResp::json(200, fin("gif ok")),
        ],
        true,
    );
    let (out, _code) = fix.run("看图\nexit\n");
    assert_eq!(fix.mock.count(), 2, "{out}");
    let body2 = req_body(&fix.mock, 1);
    let attachments = request_attachments(&body2);
    assert_eq!(attachments.len(), 1);
    let (mime, bytes) = decode_data_url(&attachments[0].1);
    assert_eq!(mime, "image/png", "GIF 转静态 PNG");
    assert!(image::load_from_memory(&bytes).is_ok(), "转换后可解码");
}

/// 7. 同批看图＋其他工具：tool 结果全部先配对，两个附件按调用序后置，
/// 批中不插 user 消息；先失败后成功只附成功项。
#[test]
fn e2e_batch_ordering_and_fail_then_success() {
    let work = TempDir::new("vimg-batch-work");
    let a = write_file(work.path(), "a.png", &png_bytes(2, 2, [1, 1, 1, 255]));
    let b = write_file(work.path(), "b.png", &png_bytes(3, 3, [2, 2, 2, 255]));
    let fix = Fix::new(
        vec![
            MockResp::json(
                200,
                tc(&[
                    view_call("a1", a.display().to_string().as_str()),
                    (
                        "e1",
                        "exec_command",
                        json!({ "cmd": "echo mid" }).to_string(),
                    ),
                    view_call("b1", b.display().to_string().as_str()),
                ]),
            ),
            MockResp::json(200, fin("批一完成")),
        ],
        true,
    );
    let (out, _code) = fix.run("看图\nexit\n");
    assert_eq!(fix.mock.count(), 2, "{out}");
    let body2 = req_body(&fix.mock, 1);
    let roles: Vec<&str> = body2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        vec!["system", "user", "user", "assistant", "tool", "tool", "tool", "user", "user"],
        "（015 上下文注入在前）三条 tool 先配对，两个附件按序后置：{roles:?}"
    );
    let attachments = request_attachments(&body2);
    assert_eq!(attachments.len(), 2);
    assert!(attachments[0].0.contains("a1"), "调用序：{attachments:?}");
    assert!(attachments[1].0.contains("b1"), "调用序：{attachments:?}");
}

/// 8. 累计图片计入 10 MiB 请求上限：两张近 4 MiB 噪声图使下一请求超限，
/// 网络请求计数为零（mock 仍停在第 1 次），CLI 明确报错。
#[test]
fn e2e_request_body_limit_zero_send() {
    let work = TempDir::new("vimg-cap-work");
    // 1350×1000 噪声 RGB PNG ≈3.86 MiB（< 4 MiB 准备上限）；base64 后 ≈5.15 MiB，
    // 两张 ≈10.3 MiB > 10 MiB 请求上限。
    let a = write_file(work.path(), "big1.png", &noise_png(1350, 1000, 7));
    let b = write_file(work.path(), "big2.png", &noise_png(1350, 1000, 13));
    let fix = Fix::new(
        vec![
            MockResp::json(
                200,
                tc(&[
                    view_call("big1", a.display().to_string().as_str()),
                    view_call("big2", b.display().to_string().as_str()),
                ]),
            ),
            MockResp::json(200, fin("不应到达")),
        ],
        true,
    );
    let (out, _code) = fix.run("看图\nexit\n");
    assert_eq!(fix.mock.count(), 1, "超限请求零发送：{out}");
    assert!(out.contains("10 MiB"), "明确报错：{out}");
    assert!(!out.contains(";base64,"), "输出不含载荷：{out}");
}

/// 9. 提供方拒绝图片请求：如实报错、不重试、不降为纯文本；错误回显的
/// data URL 载荷被遮蔽。
#[test]
fn e2e_provider_rejection_no_retry_no_payload_leak() {
    let work = TempDir::new("vimg-reject-work");
    let a = write_file(work.path(), "r.png", &png_bytes(2, 2, [5, 5, 5, 255]));
    let long_b64 = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5ejAxMjM0NTY3ODk=";
    let err_body = json!({ "error": { "message": format!("bad image data:image/png;base64,{long_b64}{long_b64} rejected") } }).to_string();
    let fix = Fix::new(
        vec![
            MockResp::json(200, tc(&[view_call("r1", a.display().to_string().as_str())])),
            MockResp::json(400, err_body),
        ],
        true,
    );
    let (out, _code) = fix.run("看图\nexit\n");
    assert_eq!(fix.mock.count(), 2, "不重试：{out}");
    assert!(out.contains("400") || out.contains("失败"), "如实报错：{out}");
    assert!(!out.contains(long_b64), "回显载荷被遮蔽：{out}");
}

/// 10. 生命周期：第 8 批的 view_image 结果照常保存（不发第 9 次请求），
/// 下一用户轮请求自然携带已保存图片。
#[test]
fn e2e_eighth_batch_saved_and_next_turn_carries_image() {
    let work = TempDir::new("vimg-8th-work");
    let a = write_file(work.path(), "s.png", &png_bytes(2, 2, [6, 6, 6, 255]));
    let ids = ["c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7"];
    let mut responses: Vec<MockResp> = Vec::new();
    for id in ids {
        responses.push(MockResp::json(200, tc(&[view_call(
            id,
            a.display().to_string().as_str(),
        )])));
    }
    responses.push(MockResp::json(200, fin("下一轮完成")));
    let fix = Fix::new(responses, true);
    let (out, _code) = fix.run("看图\n继续\nexit\n");
    // 第一轮 8 次请求后达上限（错误），第二轮第 9 次请求携带图片
    assert_eq!(fix.mock.count(), 9, "{out}");
    assert!(out.contains("上限"), "第 8 批后报上限：{out}");
    let body9 = req_body(&fix.mock, 8);
    let attachments = request_attachments(&body9);
    assert!(
        attachments.iter().any(|(t, _u, _d)| t.contains("c7")),
        "第 8 批结果在下一轮请求中：{:?}",
        attachments.len()
    );
}

/// 11. 取消：模型返回工具批后置取消标志——未开始的调用补配对结果且不附图。
/// （lib 级：真实循环＋真实取消标志；路径不存在也证明未执行。）
#[test]
fn lib_cancel_skips_view_image_without_payload() {
    use std::sync::atomic::AtomicBool;
    use terminal_chat::prompt::HistoryItem;
    use terminal_chat::turn::{
        run_tool_turn_with_hosts, ToolMode, TurnNotify,
    };
    use terminal_chat::proc_session::ProcessManager;
    let cancel = AtomicBool::new(false);
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let tc_body = tc(&[view_call("cc", "definitely-missing.png")]);
    let mut sender = |_: &Value| -> Result<Value, String> {
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(serde_json::from_str(&tc_body).unwrap())
    };
    let mut notify = |_: TurnNotify| {};
    let mut on_record = |_: &[HistoryItem]| {};
    let result = run_tool_turn_with_hosts(
        &mut history,
        "看图",
        &mut sender,
        &mut notify,
        Some(&cancel),
        &mut on_record,
        ToolMode::User,
        &mgr,
        None,
        None,
        true,
        None
    );
    assert!(result.is_err(), "取消轮报错");
    let tool_results: Vec<&HistoryItem> = history
        .iter()
        .filter(|h| matches!(h, HistoryItem::ToolResult { .. }))
        .collect();
    assert_eq!(tool_results.len(), 1, "配对补齐");
    match tool_results[0] {
        HistoryItem::ToolResult { content, image, .. } => {
            assert!(content.contains("取消") || content.contains("未执行"), "{content}");
            assert!(image.is_none(), "跳过的调用不附图");
        }
        other => panic!("形状：{other:?}"),
    }
}

/// 11b.（019 §3.2）MemoryMerge 内部合并轮：不声明也不执行 view_image——
/// 伪造调用落入合并轮未知工具错误，不读取文件（缺失路径也只报未知工具）。
#[test]
fn lib_memory_merge_rejects_forged_view_image_without_reading() {
    use std::sync::atomic::AtomicBool;
    use terminal_chat::memory::runner::load_memories_config;
    use terminal_chat::proc_session::ProcessManager;
    use terminal_chat::prompt::HistoryItem;
    use terminal_chat::turn::{run_tool_turn_with_hosts, ToolMode, TurnNotify};
    let root = work_dir("merge-root");
    let config = load_memories_config();
    let system = "合并系统提示";
    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let mut sent = 0;
    let mut sender = |_: &Value| -> Result<Value, String> {
        sent += 1;
        if sent == 1 {
            Ok(serde_json::from_str(&tc(&[view_call("m-fake", "definitely-missing.png")])).unwrap())
        } else {
            Ok(serde_json::from_str(&fin("合并完成")).unwrap())
        }
    };
    let mut notify = |_: TurnNotify| {};
    let mut on_record = |_: &[HistoryItem]| {};
    let cancel = AtomicBool::new(false);
    let result = run_tool_turn_with_hosts(
        &mut history,
        "合并",
        &mut sender,
        &mut notify,
        Some(&cancel),
        &mut on_record,
        ToolMode::MemoryMerge {
            root: &root,
            system,
            expected_epoch: 0,
        },
        &mgr,
        None,
        None,
        true,
        None
    );
    let reply = result.expect("合并轮照常走完");
    assert_eq!(reply, "合并完成");
    match history.iter().find(|h| matches!(h, HistoryItem::ToolResult { .. })) {
        Some(HistoryItem::ToolResult { content, image, .. }) => {
            assert!(content.contains("未知工具"), "合并轮分派拒绝：{content}");
            assert!(image.is_none(), "不附图");
        }
        other => panic!("应有配对结果：{other:?}"),
    }
}

fn work_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tchat-vimg-{}-{tag}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// 12. 发送前能力检查（API 层）：含图片消息的请求发往未确认图片能力的模型
/// → 本地明确错误、零网络请求；能力确认后正常发送。
#[test]
fn lib_api_checks_image_capability_before_send() {
    use terminal_chat::api;
    use terminal_chat::catalog::{EffectiveConnection, ModelInfo, SUPPORTED_PROTOCOL};
    let mock = spawn_mock(vec![MockResp::json(200, fin("ok"))]);
    let conn = EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: SUPPORTED_PROTOCOL.into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let body = json!({
        "messages": [
            { "role": "user", "content": [
                { "type": "text", "text": "图" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,QUJD", "detail": "high" } }
            ]}
        ]
    });
    let text_model = ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat_with_body(&conn, "k", &text_model, &body, &api::agent()).unwrap_err();
    assert!(err.contains("图片") && err.contains("m"), "本地错误：{err}");
    assert_eq!(mock.count(), 0, "零发送");
    let img_model = ModelInfo {
        id: "m".into(),
        input_modalities: Some(vec!["text".into(), "image".into()]),
        ..Default::default()
    };
    api::chat_with_body(&conn, "k", &img_model, &body, &api::agent()).unwrap();
    assert_eq!(mock.count(), 1);
}

/// 13. 保存与恢复：源文件删除后恢复会话，下一请求仍携带当时字节；
/// 恢复不重读源文件、不重新执行工具。
#[test]
fn e2e_resume_after_source_deleted_uses_saved_bytes() {
    let work = TempDir::new("vimg-resume-work");
    let bytes = png_bytes(5, 4, [77, 66, 55, 255]);
    let file = write_file(work.path(), "gone.png", &bytes);
    let abs = file.display().to_string();
    let fix = Fix::new(
        vec![
            MockResp::json(200, tc(&[view_call("r-1", &abs)])),
            MockResp::json(200, fin("第一轮完成")),
        ],
        true,
    );
    let (out, code) = fix.run("看图\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(fix.mock.count(), 2, "{out}");
    // 删除源文件后恢复：第二轮请求携带的图片字节等于原文件字节
    std::fs::remove_file(&file).expect("删除源图");
    let (out2, _c2) = fix.run("/resume\n继续看\nexit\n");
    assert!(out2.contains("已恢复") || out2.contains("恢复"), "恢复输出：{out2}");
    assert!(fix.mock.count() >= 3, "第二轮有请求：{out2}");
    let body3 = req_body(&fix.mock, 2);
    let attachments = request_attachments(&body3);
    assert_eq!(attachments.len(), 1, "恢复后恰一附件");
    let (_mime, got) = decode_data_url(&attachments[0].1);
    assert_eq!(got, bytes, "恢复请求字节＝当时图片（源已删除）");
}

/// 14. 损坏图片字段：按 007 坏行规则跳过并补未知结果；后续请求无附件。
#[test]
fn e2e_corrupted_image_field_bad_line() {
    let work = TempDir::new("vimg-corrupt-work");
    let a = write_file(work.path(), "ok.png", &png_bytes(2, 2, [3, 3, 3, 255]));
    let fix = Fix::new(
        vec![
            MockResp::json(200, tc(&[view_call("x1", a.display().to_string().as_str())])),
            MockResp::json(200, fin("done")),
        ],
        true,
    );
    let (out, code) = fix.run("看图\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    // 找到 auto 会话文件，把 image 字段改成非法 base64
    let sessions = fix.project.path().join("user").join("sessions");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&sessions)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    names.sort();
    let target = names.last().unwrap().clone();
    let text = std::fs::read_to_string(&target).unwrap();
    let corrupted = text.replace(
        "\"image\":{\"image_url\":\"data:image/png;base64,",
        "\"image\":{\"image_url\":\"data:image/png;base64,!!!!",
    );
    assert_ne!(corrupted, text, "夹具必须真的损坏了 image 字段");
    std::fs::write(&target, corrupted).unwrap();
    let fix2 = Fix {
        project: fix.project,
        mock: fix.mock,
    };
    let (out2, _c) = fix2.run("/resume\n继续\nexit\n");
    assert!(
        out2.contains("跳过") || out2.contains("损坏") || out2.contains("修复"),
        "坏行报告：{out2}"
    );
    let body3 = req_body(&fix2.mock, 2);
    assert!(request_attachments(&body3).is_empty(), "损坏行不产生附件");
    // 结果行被配对修复为未知结果
    let tool_msgs: Vec<&Value> = body3["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("tool"))
        .collect();
    assert!(
        tool_msgs.iter().any(|m| { m["content"].as_str().unwrap().contains("结果未知") }),
        "补未知结果：{tool_msgs:?}"
    );
}
