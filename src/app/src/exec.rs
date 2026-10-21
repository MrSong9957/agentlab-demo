//! 同步命令执行（exec_command，003 规格）：在指定目录同步执行一条命令，
//! 完成、超时或取消后返回结果。参考 Codex 同步版本思路（单次执行、无会话管理）。
//!
//! - Shell 由运行时固定为 `/bin/sh -c`；stdin 关闭；不使用 PTY。
//! - 工作目录只决定从哪里执行，不构成权限隔离（坑001，权限由执行环境控制）。
//! - 超时/取消会终止并回收整个进程组（含子进程）；终止不回滚已发生的文件修改。
//! - 双流并发读取、按到达顺序合并（坑003：等退出后再读管道会互相等待）；
//!   缓存只保留前 N 个码点，超限后继续读取并丢弃，内存有界。

use serde::Serialize;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 默认超时（毫秒）。
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// 超时上限（毫秒，2026-10-10 安全修复 N2）：参数校验层拒绝更大值，防恶意模型无限挂住会话。
pub const MAX_TIMEOUT_MS: u64 = 60_000;
/// 默认输出上限（Unicode 码点数）。
pub const DEFAULT_MAX_OUTPUT_CHARS: usize = 10_000;
/// 输出上限的硬上限（2026-10-10 安全修复 N4）：防恶意模型用超大捕获上限放大内存。
pub const MAX_OUTPUT_CHARS_LIMIT: usize = 100_000;

/// 运行时固定的 Shell。
const SHELL: &str = "/bin/sh";
/// 等待子进程退出 / 检查取消标志的轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// 进程结束后等待读取线程收尾的上限；超时则返回已收集的输出。
const READER_GRACE: Duration = Duration::from_secs(2);
/// 读取管道的块大小。
const READ_BUF: usize = 8 * 1024;

/// exec_command 工具入参：只接受四个字段，多余字段视为非法。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecArgs {
    /// 必填；不能全为空白；按执行环境 Shell 语法解释。
    pub cmd: String,
    /// 默认会话当前目录；相对路径基于该目录解析。
    pub workdir: Option<String>,
    /// 正整数；默认 [`DEFAULT_TIMEOUT_MS`]。
    pub timeout_ms: Option<u64>,
    /// 正整数；默认 [`DEFAULT_MAX_OUTPUT_CHARS`]，字符按 Unicode 码点计数。
    pub max_output_chars: Option<usize>,
}

impl ExecArgs {
    /// 从工具边界的 JSON 值解析并校验；任何非法输入返回错误说明，不执行命令。
    pub fn from_value(v: &serde_json::Value) -> Result<Self, String> {
        let Some(obj) = v.as_object() else {
            return Err("参数必须是对象".into());
        };
        for key in obj.keys() {
            if !matches!(key.as_str(), "cmd" | "workdir" | "timeout_ms" | "max_output_chars") {
                return Err(format!("未知参数：{key}"));
            }
        }
        let cmd = match obj.get("cmd") {
            None | Some(serde_json::Value::Null) => return Err("缺少必填参数 cmd".into()),
            Some(c) => c.as_str().ok_or("cmd 必须是字符串")?,
        };
        if cmd.trim().is_empty() {
            return Err("cmd 不能全为空白".into());
        }
        let workdir = match obj.get("workdir") {
            None | Some(serde_json::Value::Null) => None,
            Some(w) => Some(w.as_str().ok_or("workdir 必须是字符串")?.to_string()),
        };
        let timeout_ms = bounded_int(obj, "timeout_ms", MAX_TIMEOUT_MS)?;
        let max_output_chars = bounded_int(obj, "max_output_chars", MAX_OUTPUT_CHARS_LIMIT as u64)?
            .map(|n| n as usize);
        Ok(Self { cmd: cmd.to_string(), workdir, timeout_ms, max_output_chars })
    }
}

/// 提取带上限的正整数字段：超过上限直接拒绝（不悄悄调整参数）。
fn bounded_int(
    obj: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    max: u64,
) -> Result<Option<u64>, String> {
    match positive_int(obj, name)? {
        Some(n) if n > max => Err(format!("{name} 必须为 1..={max} 的正整数（毫秒/字符上限见工具说明）")),
        other => Ok(other),
    }
}

/// 提取正整数字段：缺失或 null 为默认值；零、负数、浮点、字符串均非法。
fn positive_int(
    obj: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<Option<u64>, String> {
    match obj.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        // is_u64 只对真正的非负整数成立，浮点（如 1.5、1.0）不算
        Some(v) if v.is_u64() => match v.as_u64() {
            Some(n) if n > 0 => Ok(Some(n)),
            _ => Err(format!("{name} 必须为正整数")),
        },
        Some(_) => Err(format!("{name} 必须为正整数")),
    }
}

/// 执行结果状态：自然结束（含非零退出码、自行被信号终止）均为 Completed；
/// 成功须同时满足 status=completed 且 exit_code=0（坑002：有输出不代表成功）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecStatus {
    Completed,
    Timeout,
    Cancelled,
    Error,
}

/// 执行结果：五个字段始终存在。
#[derive(Debug, PartialEq, Serialize)]
pub struct ExecResult {
    pub status: ExecStatus,
    /// 实际整数退出码；被信号终止或未执行时为 null。
    pub exit_code: Option<i32>,
    /// stdout/stderr 合并输出；UTF-8 解码，非法字节替换为 U+FFFD。
    pub output: String,
    /// 输出是否因超过上限被省略。
    pub truncated: bool,
    /// 错误说明；正常完成（status=completed）时为 null。
    pub error: Option<String>,
}

impl ExecResult {
    /// 参数/启动失败：未执行命令，无退出码无输出。
    fn error(msg: String) -> Self {
        Self {
            status: ExecStatus::Error,
            exit_code: None,
            output: String::new(),
            truncated: false,
            error: Some(msg),
        }
    }
}

/// 工具入口：解析校验入参并同步执行；参数/启动失败返回 error 且不执行命令。
/// `cancel` 由调用方持有：置 true 时终止进程组并返回 cancelled。
pub fn exec_command(input: &serde_json::Value, cancel: Option<&AtomicBool>) -> ExecResult {
    let args = match ExecArgs::from_value(input) {
        Ok(a) => a,
        Err(msg) => return ExecResult::error(msg),
    };
    run(&args, cancel)
}

/// 解析工作目录：缺省为会话当前目录；相对路径基于该目录；必须已是存在的目录。
fn resolve_workdir(workdir: Option<&str>) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("无法确定会话当前目录：{e}"))?;
    let dir = match workdir {
        None => cwd,
        Some(w) => cwd.join(w),
    };
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("workdir 目录不存在：{}", dir.display()))
    }
}

/// 终止整个进程组（含孙进程）并回收组长；组长已退出时的失败可忽略（竞态）。
fn kill_group(child: &mut Child) {
    let pgid = child.id() as libc::pid_t;
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
    let _ = child.wait(); // 回收，避免僵尸
}

fn run(args: &ExecArgs, cancel: Option<&AtomicBool>) -> ExecResult {
    let workdir = match resolve_workdir(args.workdir.as_deref()) {
        Ok(p) => p,
        Err(msg) => return ExecResult::error(msg),
    };
    let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let cap = args.max_output_chars.unwrap_or(DEFAULT_MAX_OUTPUT_CHARS);

    let mut child = match Command::new(SHELL)
        .arg("-c")
        .arg(&args.cmd)
        .current_dir(&workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0) // 独立进程组，组长即本子进程：终止时可覆盖其全部后代
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ExecResult::error(format!("进程启动失败：{e}")),
    };

    let sink = Arc::new(Mutex::new(TextSink::new(cap)));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let mut readers = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        readers.push(spawn_reader(pipe, sink.clone(), done_tx.clone()));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(spawn_reader(pipe, sink.clone(), done_tx.clone()));
    }
    drop(done_tx);

    let deadline = Instant::now() + timeout;
    let (status, err_msg) = loop {
        match child.try_wait() {
            Ok(Some(_)) => break (ExecStatus::Completed, None),
            Ok(None) => {}
            Err(e) => {
                kill_group(&mut child);
                break (ExecStatus::Error, Some(format!("进程状态检查失败：{e}")));
            }
        }
        if cancel.is_some_and(|f| f.load(Ordering::SeqCst)) {
            kill_group(&mut child);
            break (ExecStatus::Cancelled, None);
        }
        if Instant::now() >= deadline {
            kill_group(&mut child);
            break (ExecStatus::Timeout, None);
        }
        thread::sleep(POLL_INTERVAL);
    };

    // Completed 路径 try_wait 已回收，wait 立即返回同一状态；被 SIGKILL 的进程无整数退出码
    let exit_code = child.wait().ok().and_then(|s| s.code());
    // 等读取线程排干管道（进程死后 EOF 很快到达）；超时则放弃未收尾部分
    let _ = done_rx.recv_timeout(READER_GRACE);
    let _ = done_rx.recv_timeout(READER_GRACE);
    let (output, truncated) = sink.lock().unwrap_or_else(|e| e.into_inner()).finish();
    let _ = readers; // 线程已通过 done 通道收尾，JoinHandle 无需再 join

    let error = match status {
        ExecStatus::Completed => None,
        ExecStatus::Timeout => Some(format!("执行超时（{} ms），已终止进程组", timeout.as_millis())),
        ExecStatus::Cancelled => Some("执行已取消，已终止进程组".into()),
        ExecStatus::Error => err_msg,
    };
    ExecResult { status, exit_code, output, truncated, error }
}

/// 读取单条输出流直到 EOF：按块推入共享缓存；管道断开（Err）按 EOF 处理。
fn spawn_reader<R: Read + Send + 'static>(
    mut pipe: R,
    sink: Arc<Mutex<TextSink>>,
    done: mpsc::Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; READ_BUF];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink.lock().unwrap_or_else(|e| e.into_inner()).push(&buf[..n]),
            }
        }
        let _ = done.send(());
    })
}

/// 流式有界输出缓存：两路输出按到达顺序合并；只保留前 cap 个码点，
/// 超限后继续丢弃并置 truncated；跨块的 UTF-8 序列自动拼接。
struct TextSink {
    out: String,
    chars: usize,
    cap: usize,
    capped: bool,
    truncated: bool,
    /// 尾部不完整的 UTF-8 序列，等待下一块补全。
    pending: Vec<u8>,
}

impl TextSink {
    fn new(cap: usize) -> Self {
        Self {
            out: String::new(),
            chars: 0,
            cap,
            capped: false,
            truncated: false,
            pending: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if self.capped {
            if !chunk.is_empty() {
                self.truncated = true;
            }
            return;
        }
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(chunk);
        let mut rest: &[u8] = &buf;
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    self.append(s);
                    rest = &[];
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        self.append(std::str::from_utf8(&rest[..valid]).unwrap());
                    }
                    match e.error_len() {
                        // 尾部不完整序列：暂存等待下一块
                        None => {
                            self.pending.extend_from_slice(&rest[valid..]);
                            rest = &[];
                        }
                        // 非法子序列：按 from_utf8_lossy 语义替换为一个 U+FFFD
                        Some(len) => {
                            self.append("\u{FFFD}");
                            rest = &rest[valid + len..];
                        }
                    }
                }
            }
            if self.capped {
                break;
            }
        }
        if self.capped {
            self.pending.clear();
        }
    }

    /// 追加文本：超过剩余额度时只保留到上限并标记截断。
    fn append(&mut self, s: &str) {
        if self.capped {
            if !s.is_empty() {
                self.truncated = true;
            }
            return;
        }
        let remaining = self.cap - self.chars;
        let n = s.chars().count();
        if n <= remaining {
            self.out.push_str(s);
            self.chars += n;
        } else {
            let keep: String = s.chars().take(remaining).collect();
            self.out.push_str(&keep);
            self.chars += remaining;
            self.capped = true;
            self.truncated = true;
        }
    }

    /// 收尾：EOF 仍未补全的序列替换为一个 U+FFFD（额度已满则只标记截断）。
    fn finish(&mut self) -> (String, bool) {
        if !self.pending.is_empty() {
            self.pending.clear();
            self.append("\u{FFFD}");
        }
        (std::mem::take(&mut self.out), self.truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- TextSink：流式解码与有界缓存 ----

    fn sink_run(cap: usize, chunks: &[&[u8]]) -> (String, bool) {
        let mut sink = TextSink::new(cap);
        for c in chunks {
            sink.push(c);
        }
        sink.finish()
    }

    #[test]
    fn sink_keeps_valid_text() {
        assert_eq!(sink_run(100, &[b"hello"]), ("hello".into(), false));
    }

    #[test]
    fn sink_joins_multibyte_split_across_chunks() {
        // “你” = E4 BD A0 拆三块；“🎉” = F0 9F 8E 89 拆两块
        assert_eq!(
            sink_run(100, &[&[0xE4], &[0xBD], &[0xA0], &[0xF0, 0x9F], &[0x8E, 0x89]]),
            ("你🎉".into(), false)
        );
    }

    #[test]
    fn sink_replaces_invalid_bytes() {
        // 0xFF 非法；0xC3 后跟 0x28 不是合法续字节：各替换一个 U+FFFD
        assert_eq!(sink_run(100, &[b"a\xff\xc3("]), ("a\u{FFFD}\u{FFFD}(".into(), false));
    }

    #[test]
    fn sink_cap_truncates_and_stays_bounded() {
        let (out, truncated) = sink_run(5, &[b"h\xc3\xa9llo", b"more"]);
        assert_eq!(out, "héllo");
        assert!(truncated, "丢弃了后续内容必须标记截断");
        let mut sink = TextSink::new(5);
        sink.push(b"h\xc3\xa9llo");
        let len_before = sink.out.len();
        sink.push(b"much more output");
        assert_eq!(sink.out.len(), len_before, "超限后缓存不再增长");
    }

    #[test]
    fn sink_cap_cuts_inside_chunk() {
        // 6 码点取前 3：a🎉b
        let (out, truncated) = sink_run(3, &["a🎉bcd".as_bytes()]);
        assert_eq!(out, "a🎉b");
        assert!(truncated);
    }

    #[test]
    fn sink_cap_reached_exactly_then_more() {
        // 恰好填满额度：内容正确且未截断（原 sink_cap_exact 场景并入，“héllo”＝5 码点）
        let (full, truncated) = sink_run(5, &[b"h\xc3\xa9llo"]);
        assert_eq!(full, "héllo");
        assert!(!truncated, "恰满不截断");
        // 上一块恰好填满额度、未截断；再来任意字节都必须标记截断
        let (out, truncated) = sink_run(2, &[b"ab", &[0x01]]);
        assert_eq!(out, "ab");
        assert!(truncated, "恰满后再来必标截断");
    }

    #[test]
    fn sink_incomplete_tail_at_eof_replaced() {
        assert_eq!(sink_run(100, &[&[0xE4]]), ("\u{FFFD}".into(), false));
    }

    #[test]
    fn sink_truncation_replacement_counts_one_char() {
        // 额度 2，内容 a + 非法字节 + b：a� 占满额度，b 被丢弃
        let (out, truncated) = sink_run(2, &[b"a\xffb"]);
        assert_eq!(out, "a\u{FFFD}");
        assert!(truncated);
    }

    // ---- ExecArgs::from_value：参数校验 ----

    #[test]
    fn args_happy_path_with_all_fields() {
        let a = ExecArgs::from_value(&json!({
            "cmd": "ls", "workdir": "/tmp", "timeout_ms": 100, "max_output_chars": 50
        }))
        .unwrap();
        assert_eq!(
            a,
            ExecArgs {
                cmd: "ls".into(),
                workdir: Some("/tmp".into()),
                timeout_ms: Some(100),
                max_output_chars: Some(50),
            }
        );
        // null 可选字段视为缺省
        let a = ExecArgs::from_value(&json!({ "cmd": "ls", "workdir": null })).unwrap();
        assert_eq!(a.workdir, None);
    }

    #[test]
    fn args_reject_non_object_and_unknown_field() {
        assert!(ExecArgs::from_value(&json!(5)).unwrap_err().contains("对象"));
        assert!(ExecArgs::from_value(&json!("cmd")).unwrap_err().contains("对象"));
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "evil": 1 }))
            .unwrap_err()
            .contains("未知参数"));
    }

    #[test]
    fn args_reject_bad_cmd() {
        assert!(ExecArgs::from_value(&json!({})).unwrap_err().contains("cmd"));
        assert!(ExecArgs::from_value(&json!({ "cmd": "" })).unwrap_err().contains("cmd"));
        assert!(ExecArgs::from_value(&json!({ "cmd": " \t " })).unwrap_err().contains("cmd"));
        assert!(ExecArgs::from_value(&json!({ "cmd": 5 })).unwrap_err().contains("cmd"));
    }

    #[test]
    fn args_reject_bad_numbers_and_workdir() {
        for bad in [0, -5] {
            let e = ExecArgs::from_value(&json!({ "cmd": "ls", "timeout_ms": bad })).unwrap_err();
            assert!(e.contains("timeout_ms"), "{e}");
            let e =
                ExecArgs::from_value(&json!({ "cmd": "ls", "max_output_chars": bad })).unwrap_err();
            assert!(e.contains("max_output_chars"), "{e}");
        }
        for bad in [json!("500"), json!(1.5)] {
            let e =
                ExecArgs::from_value(&json!({ "cmd": "ls", "timeout_ms": bad })).unwrap_err();
            assert!(e.contains("timeout_ms"), "{e}");
        }
        let e = ExecArgs::from_value(&json!({ "cmd": "ls", "workdir": 3 })).unwrap_err();
        assert!(e.contains("workdir"), "{e}");
    }

    // ---- 输出契约：五个字段始终存在、状态小写 ----

    #[test]
    fn result_serializes_spec_shape() {
        let r = ExecResult {
            status: ExecStatus::Timeout,
            exit_code: None,
            output: "x".into(),
            truncated: true,
            error: Some("e".into()),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({ "status": "timeout", "exit_code": null, "output": "x", "truncated": true, "error": "e" })
        );
    }
}
