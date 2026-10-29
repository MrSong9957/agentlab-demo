//! 022 L2：Streamable HTTP 传输——真实 SDK 客户端＋进程内本地 HTTP 服务。
//!
//! 覆盖 A04（完整端点、Bearer／env_http_headers 认证、3xx 重定向禁止）、
//! A12（401／404 会话失效不重放）、A15（HTTP JSON 体／SSE 单事件 8 MiB）、
//! A09 部分（JSON 与 SSE 两种响应形态的结果投影）。
//! 服务为本文件内置的线程版最小 HTTP/1.1 实现（本地受控，无外网）。

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{TempDir, ENV_LOCK};
use terminal_chat::mcp::manager::McpManager;

const CANARY: &str = "TCHAT_HTTP_TOKEN";
const CANARY_VALUE: &str = "tchat-http-canary-022-secret";

/// 一个请求的完整记录（原始头与体）。
#[derive(Clone, Debug)]
struct HttpReq {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpReq {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

type Responder = Arc<dyn Fn(usize, &HttpReq) -> Vec<u8> + Send + Sync>;

struct HttpServer {
    url: String,
    requests: Arc<Mutex<Vec<HttpReq>>>,
    shutdown: Arc<AtomicBool>,
}

fn start_shared(responder: Responder) -> HttpServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口");
    let port = listener.local_addr().unwrap().port();
    let requests: Arc<Mutex<Vec<HttpReq>>> = Arc::new(Mutex::new(Vec::new()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let req_log = requests.clone();
    let shutdown_flag = shutdown.clone();
    std::thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        loop {
            if shutdown_flag.load(Ordering::SeqCst) {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let responder = responder.clone();
                    let req_log = req_log.clone();
                    std::thread::spawn(move || {
                        handle_conn_shared(stream, req_log, responder);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    HttpServer {
        url: format!("http://127.0.0.1:{port}/mcp"),
        requests,
        shutdown,
    }
}

fn handle_conn_shared(
    stream: std::net::TcpStream,
    log: Arc<Mutex<Vec<HttpReq>>>,
    responder: Arc<dyn Fn(usize, &HttpReq) -> Vec<u8> + Send + Sync>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone 流"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = h.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok();
    }
    let req = HttpReq {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    };
    let index = {
        let mut log = log.lock().unwrap();
        log.push(req.clone());
        log.len() - 1
    };
    let response = responder(index, &req);
    let mut stream = stream;
    stream.write_all(&response).ok();
    stream.flush().ok();
}

fn json_response(status: u16, body: &str, extra_headers: &[(&str, &str)]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        302 => "Found",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (n, v) in extra_headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn json_body(id: serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn sse_response(events: &[String], session: Option<&str>) -> Vec<u8> {
    let mut body = String::new();
    for e in events {
        body.push_str(&format!("event: message\ndata: {e}\n\n"));
    }
    let mut head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(s) = session {
        head.push_str(&format!("MCP-Session-Id: {s}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn init_ok(id: serde_json::Value) -> String {
    json_body(
        id,
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "tchat-http-fixture", "version": "0.1"}
        }),
    )
}

fn list_ok(id: serde_json::Value) -> String {
    json_body(
        id,
        serde_json::json!({"tools": [{
            "name": "search",
            "description": "Search docs.",
            "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}
        }]}),
    )
}

fn call_ok(id: serde_json::Value, text: &str) -> String {
    json_body(
        id,
        serde_json::json!({"content": [{"type": "text", "text": text}], "isError": false}),
    )
}

fn request_id_of(body: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(serde_json::json!(0))
}

fn method_of(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("method").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default()
}

/// 标准 JSON 服务器：initialize→带会话、GET→405、notifications→202、
/// tools/list→目录、tools/call→结果。
fn standard_shared() -> HttpServer {
    start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => {
                json_response(200, &call_ok(request_id_of(&req.body), "search-hit"), &[])
            }
            _ => json_response(404, "{}", &[]),
        }
    }))
}

/// 环境守卫＋管理器上下文。
struct HttpCtx {
    _env: common::EnvLockGuard,
    manager: Arc<McpManager>,
    snapshot: Arc<terminal_chat::mcp::manager::McpSnapshot>,
    _dir: TempDir,
    server: HttpServer,
}

impl HttpCtx {
    fn close(&self) {
        self.manager.close();
        self.server.shutdown.store(true, Ordering::SeqCst);
    }
}

fn setup_http(extra: &str, server: HttpServer) -> HttpCtx {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-http");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    std::env::set_var("TCHAT_TENANT_022", "tenant-abc");
    let json = format!(
        r#"{{"mcp_servers": {{"web": {{"enabled": true, "url": "{}"{extra}}}}}}}"#,
        server.url
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    HttpCtx {
        _env: common::EnvLockGuard(guard),
        manager,
        snapshot,
        _dir: dir,
        server,
    }
}

// ---------- A04：端点、认证与重定向 ----------

#[test]
fn a04_endpoint_auth_and_headers_reach_server() {
    let server = standard_shared();
    let ctx = setup_http(
        r#", "bearer_token_env_var": "TCHAT_HTTP_TOKEN", "env_http_headers": {"X-Tenant-ID": "TCHAT_TENANT_022"}"#,
        server,
    );
    assert_eq!(ctx.snapshot.len(), 1);
    assert_eq!(
        ctx.snapshot.definitions()[0]["function"]["name"],
        "mcp__web__search"
    );
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"q": "mcp"}),
        &AtomicBool::new(false),
    );
    assert!(!failed, "调用成功：{result}");
    assert!(result.contains("search-hit"));
    ctx.close();
    let requests = ctx.server.requests.lock().unwrap().clone();
    // initialize 与 tools/call 均携带认证与自定义头。
    for req in &requests {
        if req.method != "POST" {
            continue;
        }
        let method = method_of(&req.body);
        if method == "initialize" || method == "tools/call" {
            let expected = format!("Bearer {CANARY_VALUE}");
            assert_eq!(
                req.header("Authorization"),
                Some(expected.as_str()),
                "Bearer 认证头：{:?}",
                req.header("Authorization")
            );
            assert_eq!(req.header("X-Tenant-ID"), Some("tenant-abc"), "自定义头");
        }
    }
    assert!(
        requests.iter().any(|r| method_of(&r.body) == "tools/call"),
        "服务器真实收到 tools/call"
    );
}

#[test]
fn a04_anonymous_connect_without_credentials() {
    let server = standard_shared();
    let ctx = setup_http("", server);
    assert_eq!(ctx.snapshot.len(), 1, "未配置认证可匿名连接");
    ctx.close();
}

#[test]
fn a04_missing_credential_fails_without_anonymous_fallback() {
    let server = standard_shared();
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-http");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::remove_var("TCHAT_HTTP_ABSENT");
    let json = format!(
        r#"{{"mcp_servers": {{"web": {{"enabled": true, "url": "{}", "bearer_token_env_var": "TCHAT_HTTP_ABSENT"}}}}}}"#,
        server.url
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    assert_eq!(snapshot.len(), 0, "凭证缺失服务器不可用");
    assert!(
        manager
            .status_lines()
            .iter()
            .any(|l| l.contains("凭证缺失")),
        "失败原因可见"
    );
    manager.close();
    let requests = server.requests.lock().unwrap().clone();
    assert!(
        requests.is_empty(),
        "凭证缺失零网络请求：{:?}",
        requests.len()
    );
    server.shutdown.store(true, Ordering::SeqCst);
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var(CANARY);
    std::env::remove_var("TCHAT_TENANT_022");
    drop(guard);
}

#[test]
fn a04_redirects_are_not_followed() {
    // 目标服务器（重定向目标）：零访问。
    let target = standard_shared();
    let target_port = target.url.rsplit(':').next().unwrap().split('/').next().unwrap().to_string();
    let redirect = start_shared(Arc::new(move |_index, _req| {
        json_response(
            302,
            "",
            &[("Location", &format!("http://127.0.0.1:{target_port}/mcp"))],
        )
    }));
    let ctx = setup_http("", redirect);
    // initialize 阶段即失败（重定向不跟随）。
    assert_eq!(ctx.snapshot.len(), 0, "重定向导致连接失败");
    ctx.close();
    let hits = target.requests.lock().unwrap().len();
    assert_eq!(hits, 0, "重定向目标零访问");
    assert!(
        ctx.manager
            .status_lines()
            .iter()
            .any(|l| l.contains("302") || l.contains("HTTP")),
        "失败原因包含重定向响应：{:?}",
        ctx.manager.status_lines()
    );
}

// ---------- A12：401／404 会话失效不重放 ----------

#[test]
fn a12_unauthorized_call_fails_without_retry() {
    let server = start_shared(Arc::new(move |index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => json_response(401, "{}", &[]),
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http("", server);
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"q": "x"}),
        &AtomicBool::new(false),
    );
    assert!(failed);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["isError"], true);
    assert!(
        v["content"][0]["text"].as_str().unwrap().contains("MCP 客户端错误"),
        "标准失败形态：{v}"
    );
    ctx.close();
    let calls = ctx
        .server
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| method_of(&r.body) == "tools/call")
        .count();
    assert_eq!(calls, 1, "401 后零重放");
}

#[test]
fn a12_session_expired_404_fails_without_reinit_replay() {
    let server = start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            // 带 session 的调用一律 404（会话失效）。
            "POST" if method == "tools/call" => json_response(404, "{}", &[]),
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http("", server);
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"q": "x"}),
        &AtomicBool::new(false),
    );
    assert!(failed, "404 会话失效按错误返回：{result}");
    ctx.close();
    let requests = ctx.server.requests.lock().unwrap().clone();
    let calls = requests
        .iter()
        .filter(|r| method_of(&r.body) == "tools/call")
        .count();
    let inits = requests
        .iter()
        .filter(|r| method_of(&r.body) == "initialize")
        .count();
    assert_eq!(calls, 1, "一次调用");
    assert_eq!(inits, 1, "reinit_on_expired_session=false：无重初始化重放");
}

// ---------- A09（HTTP 部分）：JSON 与 SSE 响应形态 ----------

#[test]
fn a09_json_response_projection() {
    let server = standard_shared();
    let ctx = setup_http("", server);
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"q": "x"}),
        &AtomicBool::new(false),
    );
    assert!(!failed);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["content"][0]["text"], "search-hit");
    assert_eq!(v["isError"], false);
    ctx.close();
}

#[test]
fn a09_sse_response_projection() {
    let server = start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => sse_response(
                &[call_ok(request_id_of(&req.body), "sse-result")],
                Some("session-abc"),
            ),
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http("", server);
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"q": "x"}),
        &AtomicBool::new(false),
    );
    assert!(!failed, "SSE 响应投影成功：{result}");
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["content"][0]["text"], "sse-result");
    ctx.close();
}

// ---------- 023 M4：HTTP 资源面（JSON 与 SSE） ----------

fn init_ok_resources(id: serde_json::Value) -> String {
    json_body(
        id,
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {"resources": {"listChanged": false}},
            "serverInfo": {"name": "tchat-http-fixture", "version": "0.1"}
        }),
    )
}

fn resources_list_ok(id: serde_json::Value) -> String {
    json_body(
        id,
        serde_json::json!({"resources": [{
            "uri": "tchat://http/doc",
            "name": "doc",
            "mimeType": "text/plain",
            "description": "HTTP 资源"
        }]}),
    )
}

fn resources_read_ok(id: serde_json::Value) -> String {
    json_body(
        id,
        serde_json::json!({"contents": [{
            "uri": "tchat://http/doc",
            "mimeType": "text/plain",
            "text": "http-resource-body"
        }]}),
    )
}

fn resource_server_shared(sse: bool) -> HttpServer {
    start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        let id = request_id_of(&req.body);
        let respond = |body: String| -> Vec<u8> {
            if sse {
                sse_response(&[body], Some("session-abc"))
            } else {
                json_response(200, &body, &[])
            }
        };
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok_resources(id),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "resources/list" => respond(resources_list_ok(id)),
            "POST" if method == "resources/read" => respond(resources_read_ok(id)),
            _ => json_response(404, "{}", &[]),
        }
    }))
}

#[test]
fn m4_http_json_resources_declare_list_and_read() {
    let server = resource_server_shared(false);
    let ctx = setup_http("", server);
    // 仅资源服务器：不发 tools/list，快照只含三个固定资源工具。
    assert!(ctx.snapshot.has_resources());
    assert_eq!(ctx.snapshot.len(), 3, "三固定资源工具");
    let (result, failed) = ctx.snapshot.call_resource_tool(
        "list_mcp_resources",
        r#"{"server":"web"}"#,
        &AtomicBool::new(false),
    );
    assert!(!failed, "JSON 列举成功：{result}");
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["server"], "web");
    assert_eq!(v["resources"][0]["uri"], "tchat://http/doc");
    let (read, failed) = ctx.snapshot.call_resource_tool(
        "read_mcp_resource",
        r#"{"server":"web","uri":"tchat://http/doc"}"#,
        &AtomicBool::new(false),
    );
    assert!(!failed, "JSON 读取成功：{read}");
    let rv: serde_json::Value = serde_json::from_str(&read).unwrap();
    assert_eq!(rv["contents"][0]["text"], "http-resource-body");
    ctx.close();
    let requests = ctx.server.requests.lock().unwrap().clone();
    let methods: Vec<String> = requests
        .iter()
        .filter(|r| r.method == "POST")
        .map(|r| method_of(&r.body))
        .collect();
    assert!(!methods.iter().any(|m| m == "tools/list"), "不发 tools/list");
    assert!(methods.iter().any(|m| m == "resources/list"));
    assert!(methods.iter().any(|m| m == "resources/read"));
    // read 请求体：URI 原样抵达。
    let read_req = requests
        .iter()
        .find(|r| r.method == "POST" && method_of(&r.body) == "resources/read")
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&read_req.body).unwrap();
    assert_eq!(body["params"]["uri"], "tchat://http/doc");
}

#[test]
fn m4_http_sse_resources_behave_identically() {
    let server = resource_server_shared(true);
    let ctx = setup_http("", server);
    assert!(ctx.snapshot.has_resources());
    let (result, failed) = ctx.snapshot.call_resource_tool(
        "list_mcp_resources",
        r#"{"server":"web"}"#,
        &AtomicBool::new(false),
    );
    assert!(!failed, "SSE 列举成功：{result}");
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["resources"][0]["uri"], "tchat://http/doc");
    let (read, failed) = ctx.snapshot.call_resource_tool(
        "read_mcp_resource",
        r#"{"server":"web","uri":"tchat://http/doc"}"#,
        &AtomicBool::new(false),
    );
    assert!(!failed, "SSE 读取成功：{read}");
    let rv: serde_json::Value = serde_json::from_str(&read).unwrap();
    assert_eq!(rv["contents"][0]["text"], "http-resource-body");
    ctx.close();
    let requests = ctx.server.requests.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .any(|r| r.method == "POST" && method_of(&r.body) == "resources/list"),
        "SSE 形态下真实请求仍发出"
    );
}

// ---------- A15：HTTP JSON 体与 SSE 事件 8 MiB ----------

#[test]
fn a15_oversize_json_body_rejected() {
    // 超过 8 MiB 的 tools/call JSON 响应体：有界读取拒绝（不完整分配）。
    let big = "y".repeat(9 * 1024 * 1024);
    let server = start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => json_response(
                200,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"content":[{{"type":"text","text":"{big}"}}]}}}}"#,
                    request_id_of(&req.body)
                ),
                &[],
            ),
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http("", server);
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"q": "x"}),
        &AtomicBool::new(false),
    );
    assert!(failed, "超限响应体失败");
    assert!(
        !result.contains("yyyyyyyy"),
        "不截断成合法成功：{} 字节",
        result.len()
    );
    ctx.close();
}

#[test]
fn a15_oversize_sse_event_rejected() {
    // 单个 SSE 事件超过 8 MiB：事件限额拒绝。
    let big = "z".repeat(9 * 1024 * 1024);
    let server = start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => {
                let event = format!(
                    "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{big}\"}}]}}}}\n\n",
                    request_id_of(&req.body)
                );
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    event.len()
                );
                let mut out = head.into_bytes();
                out.extend_from_slice(event.as_bytes());
                out
            }
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http("", server);
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"q": "x"}),
        &AtomicBool::new(false),
    );
    assert!(failed, "超限 SSE 事件失败");
    assert!(!result.contains("zzzzzzzz"), "不截断成合法成功");
    ctx.close();
}

// ---------- A18：HTTP 在途时退出（受控闸门＋生产关停序列） ----------

/// 跨线程闸门：`arrived`＝tools/call 请求已到达（在途证据）；`release`＝
/// 测试侧放行挂起的响应（制造迟到结果）。
struct HttpGate {
    arrived: Mutex<bool>,
    arrived_cv: std::sync::Condvar,
    release: Mutex<bool>,
    release_cv: std::sync::Condvar,
}

impl HttpGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            arrived: Mutex::new(false),
            arrived_cv: std::sync::Condvar::new(),
            release: Mutex::new(false),
            release_cv: std::sync::Condvar::new(),
        })
    }

    fn mark_arrived(&self) {
        *self.arrived.lock().unwrap() = true;
        self.arrived_cv.notify_all();
    }

    fn wait_arrived(&self, timeout: std::time::Duration) -> bool {
        // 按剩余时间等待（虚假唤醒不重置上界）。
        let deadline = std::time::Instant::now() + timeout;
        let mut arrived = self.arrived.lock().unwrap();
        while !*arrived {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (now, _timed_out) = self
                .arrived_cv
                .wait_timeout(arrived, remaining)
                .unwrap();
            arrived = now;
            if *arrived {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
        }
        true
    }

    fn release_now(&self) {
        *self.release.lock().unwrap() = true;
        self.release_cv.notify_all();
    }

    fn wait_release(&self) {
        let mut release = self.release.lock().unwrap();
        while !*release {
            release = self.release_cv.wait(release).unwrap();
        }
    }
}

/// A18（HTTP 维度）：真实 tools/call 在途（闸门确认请求到达）→ 宿主取消
/// 传播使调用返回「已发未知」→ 生产关停入口 `manager.close()` 有界完成、
/// 本地连接与后台服务任务收尾 → 释放闸门后迟到结果不进入任何历史、
/// 服务器账本恰一次调用（无重放／额外调用），已发调用保持执行状态未知。
#[test]
fn a18_http_inflight_close_bounded_late_result_discarded() {
    let gate = HttpGate::new();
    let gate_for_server = gate.clone();
    let server = start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "GET" => json_response(405, "{}", &[]),
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-abc")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => {
                // 真实请求已在途：置 arrived 后挂起，等待测试放行（迟到响应）。
                gate_for_server.mark_arrived();
                gate_for_server.wait_release();
                sse_response(
                    &[call_ok(request_id_of(&req.body), "迟到结果不应入史")],
                    Some("session-abc"),
                )
            }
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http(r#", "tool_timeout_sec": 30"#, server);
    assert_eq!(ctx.snapshot.len(), 1);
    let entry = ctx.snapshot.entry("mcp__web__search").unwrap();

    // 1) 线程内发起调用：请求已发出并挂在服务器闸门上（在途）。
    let cancel = Arc::new(AtomicBool::new(false));
    let manager = ctx.manager.clone();
    let snapshot = ctx.snapshot.clone();
    let cancel_for_call = cancel.clone();
    let call_handle = std::thread::spawn(move || {
        let entry = snapshot.entry("mcp__web__search").unwrap();
        manager.call(entry, serde_json::json!({"q": "inflight"}), &cancel_for_call)
    });
    // 2) 受控确认：真实 tools/call 已到达服务器（在途证据，非推断）。
    assert!(
        gate.wait_arrived(std::time::Duration::from_secs(10)),
        "tools/call 已在途"
    );
    // 3) 宿主退出序列：取消传播（停止本地等待），调用返回「已发未知」。
    cancel.store(true, Ordering::SeqCst);
    let (result, failed) = call_handle.join().unwrap();
    assert!(failed);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    let text = v["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("状态未知") && text.contains("取消"),
        "已发调用保持执行状态未知：{text}"
    );
    assert!(
        !result.contains("迟到结果不应入史"),
        "等待侧结束不消费迟到结果：{result}"
    );

    // 4) 生产关停入口：有界完成（远程操作状态不被误称停止——仅关本地）。
    let close_start = std::time::Instant::now();
    ctx.manager.close();
    let close_elapsed = close_start.elapsed();
    assert!(
        close_elapsed < std::time::Duration::from_secs(5),
        "关停有界完成（实际 {close_elapsed:?}）"
    );

    // 5) 释放闸门：服务器把迟到结果写回已关闭的连接（写失败即丢弃）。
    gate.release_now();

    // 6) 收尾断言：本地连接已关闭；再次调用零网络请求；服务器账本恰一次。
    let snapshot2 = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    assert_eq!(snapshot2.len(), 0, "关闭后的连接不再发布工具");
    let requests = ctx.server.requests.lock().unwrap().clone();
    let calls = requests
        .iter()
        .filter(|r| method_of(&r.body) == "tools/call")
        .count();
    let inits = requests
        .iter()
        .filter(|r| method_of(&r.body) == "initialize")
        .count();
    assert_eq!(calls, 1, "恰一次调用：无重放、无额外调用");
    assert_eq!(inits, 1, "无重初始化");
    ctx.close();
}

// ---------- 030 S12：HTTP JSON 传输上的搜索→加载→真实调用 ----------

/// S12（HTTP JSON 腿）：真实 HTTP MCP 服务＋真实 turn——搜索本地执行零
/// 业务请求，加载后下一请求携带定义，业务调用恰一次（SSE 腿见
/// ts30_search_load_call_over_http_sse 同构用例）。
#[test]
fn ts30_search_load_call_over_http_json() {
    use terminal_chat::context::ContextSources;
    use terminal_chat::prompt::{HistoryItem, HistoryRole};
    use terminal_chat::proc_session::ProcessManager;
    use terminal_chat::turn::{run_user_turn_with_context_and_caps, ToolMode, TurnExtras, TurnNotify};

    let calls_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = calls_seen.clone();
    let server = start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-ts30")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => {
                recorder.lock().unwrap().push(method_of(&req.body));
                json_response(
                    200,
                    &call_ok(request_id_of(&req.body), "called:search"),
                    &[],
                )
            }
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http("", server);
    assert_eq!(ctx.snapshot.len(), 1, "HTTP 目录恰一项动态工具");

    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut responses = [
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"h1","type":"function","function":{
                "name":"tool_search","arguments":r#"{"query":"search"}"#}}]},
            "finish_reason":"tool_calls"}]})
            .to_string(),
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"h2","type":"function","function":{
                "name":"mcp__web__search","arguments":r#"{"q":"x"}"#}}]},
            "finish_reason":"tool_calls"}]})
            .to_string(),
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":"HTTP 完成"},
            "finish_reason":"stop"}]}).to_string(),
    ]
    .into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().expect("预设响应")).unwrap())
    };
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "HTTP 搜索任务",
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
        TurnExtras {
            mcp: Some(&ctx.snapshot),
            send_message: false,
            tool_search: true,
            steer: None,
            context: None,
            permissions: None,
        },
    );
    assert_eq!(reply, Ok("HTTP 完成".to_string()));
    let names0: Vec<&str> = bodies[0]["tools"]
        .as_array().unwrap().iter()
        .map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(!names0.contains(&"mcp__web__search"), "HTTP 动态定义同样延迟");
    assert!(names0.contains(&"tool_search"));
    let names1: Vec<&str> = bodies[1]["tools"]
        .as_array().unwrap().iter()
        .map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(names1.contains(&"mcp__web__search"), "加载后下一请求携带定义");
    assert_eq!(*calls_seen.lock().unwrap(), vec!["tools/call"], "业务调用恰一次（搜索零业务请求）");
    ctx.close();
}

/// S12（HTTP SSE 腿）：SSE 事件形态的 HTTP MCP 服务上完成搜索→加载→
/// 真实调用（审查 #8：此前注释把该腿委托给不存在的覆盖记录）。
#[test]
fn ts30_search_load_call_over_http_sse() {
    use terminal_chat::context::ContextSources;
    use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole};
    use terminal_chat::proc_session::ProcessManager;
    use terminal_chat::turn::{run_user_turn_with_context_and_caps, ToolMode, TurnExtras};

    let calls_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = calls_seen.clone();
    let server = start_shared(Arc::new(move |_index, req| {
        let method = method_of(&req.body);
        match req.method.as_str() {
            "POST" if method == "initialize" => json_response(
                200,
                &init_ok(request_id_of(&req.body)),
                &[("MCP-Session-Id", "session-ts30sse")],
            ),
            "POST" if method.starts_with("notifications/") => json_response(202, "", &[]),
            "POST" if method == "tools/list" => {
                json_response(200, &list_ok(request_id_of(&req.body)), &[])
            }
            "POST" if method == "tools/call" => {
                recorder.lock().unwrap().push(method_of(&req.body));
                sse_response(
                    &[call_ok(request_id_of(&req.body), "called:search")],
                    Some("session-ts30sse"),
                )
            }
            _ => json_response(404, "{}", &[]),
        }
    }));
    let ctx = setup_http("", server);
    assert_eq!(ctx.snapshot.len(), 1);

    let mut history: Vec<HistoryItem> = Vec::new();
    let mgr = ProcessManager::new();
    let sources = ContextSources::from_env();
    let mut bodies: Vec<serde_json::Value> = Vec::new();
    let mut responses = [
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"s1","type":"function","function":{
                "name":"tool_search","arguments":r#"{"query":"search"}"#}}]},
            "finish_reason":"tool_calls"}]})
            .to_string(),
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"s2","type":"function","function":{
                "name":"mcp__web__search","arguments":r#"{"q":"x"}"#}}]},
            "finish_reason":"tool_calls"}]})
            .to_string(),
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":"SSE 完成"},
            "finish_reason":"stop"}]}).to_string(),
    ]
    .into_iter();
    let mut sender = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        bodies.push(body.clone());
        Ok(serde_json::from_str(&responses.next().expect("预设响应")).unwrap())
    };
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "SSE 搜索任务",
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
        TurnExtras {
            mcp: Some(&ctx.snapshot),
            send_message: false,
            tool_search: true,
            steer: None,
            context: None,
            permissions: None,
        },
    );
    assert_eq!(reply, Ok("SSE 完成".to_string()));
    let names1: Vec<&str> = bodies[0]["tools"]
        .as_array().unwrap().iter()
        .map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(!names1.contains(&"mcp__web__search"), "SSE 腿同样延迟动态定义");
    let names2: Vec<&str> = bodies[1]["tools"]
        .as_array().unwrap().iter()
        .map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert!(names2.contains(&"mcp__web__search"), "加载后下一请求携带定义");
    assert_eq!(*calls_seen.lock().unwrap(), vec!["tools/call"], "业务调用恰一次");
    ctx.close();
}
