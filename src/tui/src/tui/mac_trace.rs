//! [r33-trace]（038 取证，暂驻）：mac 挂点直证日志——文件通道
//! `/tmp/tchat-mac-trace-<pid>.log`（append，per-pid 文件，绕开 libtest 捕获与 PTY
//! 屏幕污染）。打点面：桥读层（poll/read/send）、主循环事件流（Term/Tick/
//! App/Draw）、submit 点、帧请求与发出、主循环退出——停滞发生时最后一笔
//! 事件的时间戳与所在环节即挂点。取证后拆除。
//!
//! 每次写入独立 open/append（~20µs）；并发多进程同文件追加（append 原子性
//! 以行内完整写入保证——单行 <4KB，POSIX O_APPEND 原子）。仅 macOS 编译。

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static WRITE_LOCK: Mutex<()> = Mutex::new(());
static PID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

static N_TERM: AtomicU64 = AtomicU64::new(0);
static N_TICK: AtomicU64 = AtomicU64::new(0);
static N_APP: AtomicU64 = AtomicU64::new(0);
static N_DRAWREQ: AtomicU64 = AtomicU64::new(0);
static N_FRAME_REQ: AtomicU64 = AtomicU64::new(0);
static N_FRAME_SENT: AtomicU64 = AtomicU64::new(0);
static N_POLL_TRUE: AtomicU64 = AtomicU64::new(0);
static N_POLL_FALSE: AtomicU64 = AtomicU64::new(0);

fn trace_path() -> String {
    let pid = PID.get_or_init(std::process::id);
    format!("/tmp/tchat-mac-trace-{pid}.log")
}

fn bump(c: &AtomicU64) -> u64 {
    c.fetch_add(1, Ordering::Relaxed) + 1
}

/// 追加一行：`[elapsed s] pid=<pid> <msg>`。
pub fn trace(msg: &str) {
    let start = START.get_or_init(Instant::now);
    let pid = PID.get_or_init(std::process::id);
    let line = format!("[{:.3}] pid={pid} {msg}\n", start.elapsed().as_secs_f64());
    if let Ok(_g) = WRITE_LOCK.lock() {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(trace_path()) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

pub fn trace_term() {
    trace(&format!("ev Term #{}", bump(&N_TERM)));
}
pub fn trace_tick() {
    trace(&format!(
        "ev Tick #{} (term={} app={} drawreq={} framereq={} framesent={} polltrue={} pollfalse={})",
        bump(&N_TICK),
        N_TERM.load(Ordering::Relaxed),
        N_APP.load(Ordering::Relaxed),
        N_DRAWREQ.load(Ordering::Relaxed),
        N_FRAME_REQ.load(Ordering::Relaxed),
        N_FRAME_SENT.load(Ordering::Relaxed),
        N_POLL_TRUE.load(Ordering::Relaxed),
        N_POLL_FALSE.load(Ordering::Relaxed),
    ));
}
pub fn trace_app() {
    trace(&format!("ev App #{}", bump(&N_APP)));
}
pub fn trace_drawreq() {
    trace(&format!("ev DrawReq #{}", bump(&N_DRAWREQ)));
}
pub fn trace_frame_req() {
    let n = bump(&N_FRAME_REQ);
    if n % 20 == 1 {
        trace(&format!("frame req #{n}"));
    }
}
pub fn trace_frame_sent() {
    let n = bump(&N_FRAME_SENT);
    if n % 20 == 1 {
        trace(&format!("frame sent #{n}"));
    }
}
pub fn trace_poll(ready: bool) {
    if ready {
        let n = bump(&N_POLL_TRUE);
        if n % 20 == 1 {
            trace(&format!("bridge poll=true #{n}"));
        }
    } else {
        let n = bump(&N_POLL_FALSE);
        if n % 200 == 1 {
            trace(&format!("bridge poll=false #{n}"));
        }
    }
}
