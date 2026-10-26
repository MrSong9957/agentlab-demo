//! 022 §5.4（025 批次）：MCP 远程 HTTPS——真实 TLS 握手＋受控自签证书。
//!
//! 场景：(1) 注入测试根后 https 全链往返（连接、工具发现、调用）；
//! (2) 未注入根的自签证书被拒绝；(3) 生产默认不注入任何自定义根。
//! 测试信任只经 doc(hidden) 测试接缝进入，生产 `build_http_transport` 零改动。
mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{TempDir, ENV_LOCK};
use terminal_chat::mcp::manager::McpManager;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tls");
/// 全局测试根切换串行锁（接缝为进程级状态）。
static ROOT_SWITCH: Mutex<()> = Mutex::new(());

fn json_response(status: u16, body: &str, extra: &[(&str, &str)]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (n, v) in extra {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn body_of(id: serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn rid(v: &serde_json::Value) -> serde_json::Value {
    v.get("id").cloned().unwrap_or(serde_json::json!(0))
}

/// 最小 JSON-RPC 应答机：initialize／notifications／tools/list／tools/call。
fn respond(req: &str) -> Vec<u8> {
    let v: serde_json::Value = match serde_json::from_str(req) {
        Ok(v) => v,
        // GET（空体）→ 405：与 022 standard 服务器一致，SDK 据此转纯 POST 模式。
        Err(_) => return json_response(405, "{}", &[]),
    };
    match v.get("method").and_then(|m| m.as_str()).unwrap_or("") {
        "initialize" => json_response(
            200,
            &body_of(rid(&v), serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "tchat-tls-fixture", "version": "0.1"}
            })),
            &[("MCP-Session-Id", "session-tls")],
        ),
        m if m.starts_with("notifications/") => json_response(202, "", &[]),
        "tools/list" => json_response(
            200,
            &body_of(rid(&v), serde_json::json!({"tools": [{
                "name": "search",
                "description": "Search docs.",
                "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}
            }]})),
            &[],
        ),
        "tools/call" => json_response(
            200,
            &body_of(rid(&v), serde_json::json!({
                "content": [{"type": "text", "text": "tls-hit"}], "isError": false
            })),
            &[],
        ),
        _ => json_response(404, "{}", &[]),
    }
}

/// 线程版 TLS/1.1 最小服务器：每连接一个请求（无池复用），Connection: close。
fn start_tls_server() -> (String, Arc<AtomicBool>) {
    use rustls::pki_types::pem::PemObject;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let _ = provider.as_ref().clone().install_default();
    let certs = vec![
        rustls::pki_types::CertificateDer::from_pem_file(format!("{FIX}/server.crt"))
            .expect("server.crt"),
    ];
    let key = rustls::pki_types::PrivateKeyDer::from_pem_file(format!("{FIX}/server.key"))
        .expect("server.key");
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("协议版本")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("server 证书配置");
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口");
    let url = format!("https://127.0.0.1:{}/mcp", listener.local_addr().unwrap().port());
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = shutdown.clone();
    std::thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        loop {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            let Ok((stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            let config = config.clone();
            std::thread::spawn(move || {
                let mut conn = match rustls::ServerConnection::new(config) {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let mut stream = stream;
                let mut tls = rustls::Stream::new(&mut conn, &mut stream);
                // 读一个完整 HTTP 请求：header 终止符＋Content-Length 定长 body。
                let mut buf: Vec<u8> = Vec::new();
                let header_end = loop {
                    let mut tmp = [0u8; 4096];
                    match tls.read(&mut tmp) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let content_length: usize = head
                    .lines()
                    .find_map(|l| {
                        l.split_once(':').and_then(|(n, v)| {
                            n.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())
                                .flatten()
                        })
                    })
                    .unwrap_or(0);
                while buf.len() < header_end + content_length {
                    let mut tmp = [0u8; 4096];
                    match tls.read(&mut tmp) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let body = buf[header_end..header_end + content_length].to_vec();
                let response = respond(&String::from_utf8_lossy(&body));
                let _ = tls.write_all(&response);
                let _ = tls.flush();
                tls.conn.send_close_notify();
                // close_notify 必须真正写出（complete_io），否则客户端视为 TLS 截断。
                while tls.conn.wants_write() {
                    if tls.conn.complete_io(&mut tls.sock).is_err() {
                        break;
                    }
                }
            });
        }
    });
    (url, shutdown)
}

struct TlsCtx {
    _env: common::EnvLockGuard,
    manager: Arc<McpManager>,
    snapshot: Arc<terminal_chat::mcp::manager::McpSnapshot>,
    _dir: TempDir,
    _shutdown: Arc<AtomicBool>,
}

fn setup(url: &str) -> TlsCtx {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-tls");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    let json = format!(r#"{{"mcp_servers": {{"web": {{"enabled": true, "url": "{url}"}}}}}}"#);
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    TlsCtx {
        _env: common::EnvLockGuard(guard),
        manager,
        snapshot,
        _dir: dir,
        _shutdown: Arc::new(AtomicBool::new(false)),
    }
}

/// (1) 注入测试根：HTTPS 全链（TLS 握手→initialize→tools/list→tools/call）。
#[test]
fn tls_https_round_trip_with_injected_root() {
    let _seq = ROOT_SWITCH.lock().unwrap_or_else(|e| e.into_inner());
    let pem = std::fs::read(format!("{FIX}/ca.crt")).expect("ca.crt");
    terminal_chat::mcp::transport::set_extra_root_certificate_for_tests(Some(&pem));
    let (url, shutdown) = start_tls_server();
    let ctx = {
        let mut c = setup(&url);
        c._shutdown = shutdown.clone();
        c
    };
    assert_eq!(ctx.snapshot.len(), 1, "HTTPS 连接并完成工具发现");
    assert_eq!(
        ctx.snapshot.definitions()[0]["function"]["name"],
        "mcp__web__search"
    );
    let entry = ctx.snapshot.entry("mcp__web__search").expect("入口");
    let (result, failed) = ctx.manager.call(
        entry,
        serde_json::json!({"q": "tls"}),
        &AtomicBool::new(false),
    );
    assert!(!failed, "tools/call 经 TLS 成功：{result}");
    assert!(result.contains("tls-hit"), "结果回传：{result}");
    ctx.manager.close();
    shutdown.store(true, Ordering::SeqCst);
    terminal_chat::mcp::transport::set_extra_root_certificate_for_tests(None);
}

/// (2) 未注入根：自签证书被默认信任库拒绝，明确失败不静默。
#[test]
fn tls_untrusted_certificate_rejected() {
    let _seq = ROOT_SWITCH.lock().unwrap_or_else(|e| e.into_inner());
    terminal_chat::mcp::transport::set_extra_root_certificate_for_tests(None);
    let (url, shutdown) = start_tls_server();
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-tls-bad");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    let json = format!(r#"{{"mcp_servers": {{"web": {{"enabled": true, "url": "{url}"}}}}}}"#);
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    assert_eq!(snapshot.len(), 0, "不受信证书不得发布任何工具");
    manager.close();
    shutdown.store(true, Ordering::SeqCst);
    drop(dir);
    drop(common::EnvLockGuard(guard));
}

/// (3) 生产默认：接缝复位后不注入任何自定义根（连接自签即拒）。
#[test]
fn tls_production_default_has_no_injected_root() {
    let _seq = ROOT_SWITCH.lock().unwrap_or_else(|e| e.into_inner());
    terminal_chat::mcp::transport::set_extra_root_certificate_for_tests(None);
    let (url, shutdown) = start_tls_server();
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-tls-default");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    let json = format!(r#"{{"mcp_servers": {{"web": {{"enabled": true, "url": "{url}"}}}}}}"#);
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    assert_eq!(snapshot.len(), 0, "默认信任库不认自签证书");
    manager.close();
    shutdown.store(true, Ordering::SeqCst);
    drop(dir);
    drop(common::EnvLockGuard(guard));
}

