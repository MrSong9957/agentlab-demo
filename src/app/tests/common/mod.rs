//! 测试辅助：本地 HTTP mock 服务（系统边界替身）与临时配置目录。
//! 各测试目标按需引用本模块的不同子集，单个目标下存在未引用项属预期。
#![allow(dead_code, unused_imports)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 串行化使用进程级环境变量的测试，避免并行竞态。
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 011：PTY 尺寸常量（e2e 沟位列/行号断言与 PtySession 建立共用单一事实源）。
pub(crate) use pty::{PTY_COLS, PTY_ROWS};

#[derive(Clone, Debug)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub auth: Option<String>,
    pub body: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RespKind {
    /// 正常返回预置响应
    Respond,
    /// 读完请求后保持连接但不响应：客户端等待直至超时
    Silent,
    /// 读完请求后立即断开：客户端看到连接被关闭
    CloseNow,
}

#[derive(Clone)]
pub struct MockResp {
    pub status: u16,
    pub delay_ms: u64,
    pub body: String,
    pub kind: RespKind,
    /// 3xx 重定向目标（带 Location 头），用于重定向防护测试
    pub location: Option<String>,
    /// 013-M7 定点：按测试信号释放的闸门（屏障同步——挂起响应直到 release）
    pub gate: Option<Arc<GateState>>,
}

/// 闸门状态：`released` 置位前挂起响应，置位后放行（一次性广播）。
pub struct GateState {
    released: Mutex<bool>,
    cvar: std::sync::Condvar,
}

/// 测试侧闸门句柄：`release()` 放行挂在该闸门上的响应。
#[derive(Clone)]
pub struct Gate(Arc<GateState>);

impl Gate {
    pub fn release(&self) {
        let mut r = self.0.released.lock().unwrap();
        *r = true;
        self.0.cvar.notify_all();
    }
}

impl MockResp {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        MockResp { status, delay_ms: 0, body: body.into(), kind: RespKind::Respond, location: None, gate: None }
    }
    pub fn slow_json(status: u16, body: impl Into<String>, delay_ms: u64) -> Self {
        MockResp { status, delay_ms, body: body.into(), kind: RespKind::Respond, location: None, gate: None }
    }

    /// 闸门响应：读完请求后挂起，直到 `Gate::release()`（返回值＝响应＋测试侧闸门）。
    pub fn gated(status: u16, body: impl Into<String>) -> (Self, Gate) {
        let gate = Arc::new(GateState {
            released: Mutex::new(false),
            cvar: std::sync::Condvar::new(),
        });
        (
            MockResp {
                status,
                delay_ms: 0,
                body: body.into(),
                kind: RespKind::Respond,
                location: None,
                gate: Some(gate.clone()),
            },
            Gate(gate),
        )
    }

    /// 3xx 重定向响应：带 Location 头（重定向防护测试用）
    pub fn redirect(location: impl Into<String>) -> Self {
        MockResp {
            status: 302,
            delay_ms: 0,
            body: String::new(),
            kind: RespKind::Respond,
            location: Some(location.into()),
            gate: None,
        }
    }
    pub fn silent() -> Self {
        MockResp { status: 0, delay_ms: 0, body: String::new(), kind: RespKind::Silent, location: None, gate: None }
    }
    pub fn close_now() -> Self {
        MockResp { status: 0, delay_ms: 0, body: String::new(), kind: RespKind::CloseNow, location: None, gate: None }
    }
}

#[derive(Clone)]
pub struct Mock {
    pub url: String,
    requests: Arc<Mutex<Vec<Req>>>,
}

impl Mock {
    pub fn requests(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// 启动本地 mock 服务：每个请求按顺序取 responses；用尽后重复最后一条。
pub fn spawn_mock(responses: Vec<MockResp>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口失败");
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let responses = Arc::new(Mutex::new(responses));
    let requests: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
    let served = Arc::new(Mutex::new(0usize));
    let req_log = requests.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let responses = responses.clone();
            let req_log = req_log.clone();
            let served = served.clone();
            thread::spawn(move || handle_conn(stream, responses, req_log, served));
        }
    });
    Mock { url, requests }
}

fn handle_conn(
    stream: TcpStream,
    responses: Arc<Mutex<Vec<MockResp>>>,
    req_log: Arc<Mutex<Vec<Req>>>,
    served: Arc<Mutex<usize>>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone 流失败"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut content_length = 0usize;
    let mut auth = None;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() {
            return;
        }
        let h = h.trim().to_string();
        if h.is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        // 头名大小写不敏感；值保持原样（Authorization 的 Bearer 大小写有断言价值）
        if let Some((name, value)) = h.split_once(':') {
            if name.trim().eq_ignore_ascii_case("authorization") {
                auth = Some(value.trim().to_string());
            }
        }
    }
    let mut stream = stream;
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok();
    }
    let body = String::from_utf8_lossy(&body).to_string();
    req_log.lock().unwrap().push(Req { method, path, auth, body });

    let resp = {
        let mut guard = responses.lock().unwrap();
        let idx = (*served.lock().unwrap()).min(guard.len().saturating_sub(1));
        *served.lock().unwrap() += 1;
        guard.get_mut(idx).expect("有兜底响应").clone()
    };
    match resp.kind {
        // 保持连接不响应：客户端一直等到读/总超时
        RespKind::Silent => {
            thread::sleep(Duration::from_secs(30));
            return;
        }
        // 立即断开：客户端看到连接被关闭
        RespKind::CloseNow => return,
        RespKind::Respond => {}
    }
    if let Some(gate) = &resp.gate {
        let mut r = gate.released.lock().unwrap();
        while !*r {
            r = gate.cvar.wait(r).unwrap();
        }
    }
    if resp.delay_ms > 0 {
        thread::sleep(Duration::from_millis(resp.delay_ms));
    }
    let bytes = resp.body.as_bytes();
    let location = resp
        .location
        .as_ref()
        .map(|l| format!("Location: {l}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {} {}\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        reason(resp.status),
        bytes.len()
    );
    stream.write_all(head.as_bytes()).ok();
    stream.write_all(bytes).ok();
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// 独立临时目录：测试结束自动清理，不污染真实用户环境。
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let p = std::env::temp_dir().join(format!(
            "tchat-test-{}-{tag}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).expect("创建临时目录失败");
        TempDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// 凭证写盘实现与 lib 单元测试共享：核心逻辑在 cred_file_write.rs（单一来源），
// 本侧仅按集成测试目标的 crate 前缀提供专用凭证目录解析。
include!("cred_file_write.rs");

fn cred_dir() -> std::path::PathBuf {
    terminal_chat::catalog::credential_dir()
}

pub use write_cred_file_shared as unique_cred_file;

pub fn cleanup_cred_file(tag: &str) {
    let _ = std::fs::remove_file(
        terminal_chat::catalog::credential_dir().join(format!("cred-{tag}.json")),
    );
}
pub mod pty;
