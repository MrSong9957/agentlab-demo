//! 016 规格：exec_command／write_stdin 的参数与结果层。
//!
//! 执行与进程会话管理在 [`crate::proc_session`]（默认可续接）；本模块只承载
//! 参数解析校验、结果结构与状态语义。两个工具统一返回六字段
//! `{status, exit_code, output, truncated, error, session_id}`：
//! - `running`：进程仍在运行，`session_id` 为可续接句柄，`exit_code=null`；
//! - `completed`：自然退出，如实返回退出码（非零不伪装成功，信号退出不编造 0）；
//! - `terminated`／`timeout`：明确终止／显式运行时限到期并已终止回收；
//! - `cancelled`：本次等待被取消，不表示进程停止，仍可续接时保留 `session_id`；
//! - `error`：参数、启动或交互失败，`session_id=null`（无有效会话）或保留
//!   既有会话句柄（会话仍有效时）。

use serde::Serialize;

/// exec_command 等待返回的默认时间（毫秒）；只控制本次调用等待，不是进程寿命。
pub const DEFAULT_YIELD_MS: u64 = 10_000;
/// exec_command `yield_time_ms` 取值范围。
pub const MIN_YIELD_MS: u64 = 250;
pub const MAX_YIELD_MS: u64 = 30_000;
/// write_stdin 空查询的默认等待（毫秒）。
pub const DEFAULT_QUERY_YIELD_MS: u64 = 5_000;
/// write_stdin 空查询 `yield_time_ms` 取值范围。
pub const MIN_QUERY_YIELD_MS: u64 = 5_000;
pub const MAX_QUERY_YIELD_MS: u64 = 300_000;
/// write_stdin 非空输入 `yield_time_ms` 取值范围。
pub const MIN_INPUT_YIELD_MS: u64 = 250;
pub const MAX_INPUT_YIELD_MS: u64 = 30_000;
/// 默认输出上限（Unicode 码点数）。
pub const DEFAULT_MAX_OUTPUT_CHARS: usize = 10_000;
/// 输出上限的硬上限（防恶意模型用超大捕获上限放大内存）。
pub const MAX_OUTPUT_CHARS_LIMIT: usize = 100_000;
/// write_stdin 单次输入上限（UTF-8 字节）；超限整次拒绝，不部分发送。
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
/// 每对话（管理器）的会话记录上限（016：含已退出未取走输出的记录；
/// 满额拒绝新建，不淘汰旧会话）。
pub const MAX_SESSIONS: usize = 64;

/// exec_command 工具入参：只接受六个字段，多余字段视为非法。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecArgs {
    /// 必填；不能全为空白；按执行环境 Shell 语法解释。
    pub cmd: String,
    /// 默认会话当前目录；相对路径基于该目录解析。
    pub workdir: Option<String>,
    /// 本次调用等待毫秒数；默认 [`DEFAULT_YIELD_MS`]，取 250..=30000。
    pub yield_time_ms: u64,
    /// 总运行时限（毫秒）；省略（None）＝不设固定总时限。
    /// 正整数无上限，但须能安全换算为截止时间（执行前在 proc_session 校验）。
    pub timeout_ms: Option<u64>,
    /// 默认 [`DEFAULT_MAX_OUTPUT_CHARS`]，字符按 Unicode 码点计数。
    pub max_output_chars: usize,
    /// true＝分配真实 PTY，允许后续发送字符；默认 false（管道，stdin 关闭）。
    pub tty: bool,
}

impl ExecArgs {
    /// 从工具边界的 JSON 值解析并校验；任何非法输入返回错误说明，不执行命令。
    pub fn from_value(v: &serde_json::Value) -> Result<Self, String> {
        let Some(obj) = v.as_object() else {
            return Err("参数必须是对象".into());
        };
        for key in obj.keys() {
            if !matches!(
                key.as_str(),
                "cmd" | "workdir" | "yield_time_ms" | "timeout_ms" | "max_output_chars" | "tty"
            ) {
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
        let yield_time_ms = match positive_int(obj, "yield_time_ms")? {
            Some(n) if !(MIN_YIELD_MS..=MAX_YIELD_MS).contains(&n) => {
                return Err(format!(
                    "yield_time_ms 必须为 {MIN_YIELD_MS}..={MAX_YIELD_MS} 的正整数（毫秒）"
                ))
            }
            other => other.unwrap_or(DEFAULT_YIELD_MS),
        };
        // timeout_ms：正整数即可，无旧 60000 上限；可表示性（截止时间安全计算）
        // 在执行前由 proc_session 以单调时钟校验。
        let timeout_ms = positive_int(obj, "timeout_ms")?;
        let max_output_chars =
            bounded_int(obj, "max_output_chars", MAX_OUTPUT_CHARS_LIMIT as u64)?
                .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS as u64) as usize;
        let tty = match obj.get("tty") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err("tty 必须是布尔值".into()),
        };
        Ok(Self {
            cmd: cmd.to_string(),
            workdir,
            yield_time_ms,
            timeout_ms,
            max_output_chars,
            tty,
        })
    }
}

/// write_stdin 工具入参：只接受五个字段；`yield_time_ms` 按空查询／非空输入
/// 两套范围校验并解析为最终毫秒数。
#[derive(Debug, Clone, PartialEq)]
pub struct WriteStdinArgs {
    /// 必填；宿主管理的不透明会话句柄，不是操作系统 PID。
    pub session_id: String,
    /// 默认空字符串＝只查询；非空＝原样发送，不自动补换行。
    pub chars: String,
    /// 解析后的等待毫秒数（空查询默认 5000、范围 5000..=300000；
    /// 非空输入默认 250、范围 250..=30000）。
    pub yield_time_ms: u64,
    pub max_output_chars: usize,
    /// true＝终止并回收目标进程组；不得同时发送非空 chars 或指定 yield_time_ms。
    pub terminate: bool,
}

impl WriteStdinArgs {
    pub fn from_value(v: &serde_json::Value) -> Result<Self, String> {
        let Some(obj) = v.as_object() else {
            return Err("参数必须是对象".into());
        };
        for key in obj.keys() {
            if !matches!(
                key.as_str(),
                "session_id" | "chars" | "yield_time_ms" | "max_output_chars" | "terminate"
            ) {
                return Err(format!("未知参数：{key}"));
            }
        }
        let session_id = match obj.get("session_id") {
            None | Some(serde_json::Value::Null) => {
                return Err("缺少必填参数 session_id".into())
            }
            Some(s) => s.as_str().ok_or("session_id 必须是字符串")?,
        };
        if session_id.trim().is_empty() {
            return Err("session_id 不能为空".into());
        }
        let chars = match obj.get("chars") {
            None | Some(serde_json::Value::Null) => String::new(),
            Some(c) => c.as_str().ok_or("chars 必须是字符串")?.to_string(),
        };
        let terminate = match obj.get("terminate") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err("terminate 必须是布尔值".into()),
        };
        let raw_yield = positive_int(obj, "yield_time_ms")?;
        let max_output_chars =
            bounded_int(obj, "max_output_chars", MAX_OUTPUT_CHARS_LIMIT as u64)?
                .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS as u64) as usize;
        // terminate=true 与非空输入／显式等待互斥：终止只做有界收尾，不做长等待。
        if terminate {
            if !chars.is_empty() {
                return Err("terminate=true 不能同时发送 chars".into());
            }
            if raw_yield.is_some() {
                return Err("terminate=true 不能同时指定 yield_time_ms".into());
            }
        }
        let query_mode = chars.is_empty();
        let (min, max, default) = if query_mode {
            (MIN_QUERY_YIELD_MS, MAX_QUERY_YIELD_MS, DEFAULT_QUERY_YIELD_MS)
        } else {
            (MIN_INPUT_YIELD_MS, MAX_INPUT_YIELD_MS, MIN_INPUT_YIELD_MS)
        };
        let yield_time_ms = match raw_yield {
            Some(n) if !(min..=max).contains(&n) => {
                return Err(format!(
                    "yield_time_ms 必须为 {min}..={max} 的正整数（毫秒；当前为{}等待）",
                    if query_mode { "空查询" } else { "输入" }
                ))
            }
            other => other.unwrap_or(default),
        };
        if chars.len() > MAX_INPUT_BYTES {
            return Err(format!(
                "chars 超过单次输入上限 {MAX_INPUT_BYTES} 字节（实际 {} 字节），整次拒绝",
                chars.len()
            ));
        }
        Ok(Self {
            session_id: session_id.to_string(),
            chars,
            yield_time_ms,
            max_output_chars,
            terminate,
        })
    }
}

/// 提取带上限的正整数字段：超过上限直接拒绝（不悄悄调整参数）。
fn bounded_int(
    obj: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    max: u64,
) -> Result<Option<u64>, String> {
    match positive_int(obj, name)? {
        Some(n) if n > max => Err(format!("{name} 必须为 1..={max} 的正整数")),
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

/// 执行结果状态（016 六状态）；`completed` 成功须同时满足 status=completed
/// 且 exit_code=0（坑002：有输出不代表成功）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecStatus {
    Running,
    Completed,
    Terminated,
    Timeout,
    Cancelled,
    Error,
}

/// 执行结果：六个字段始终存在；无可续接会话时 `session_id=null`。
#[derive(Debug, PartialEq, Serialize)]
pub struct ExecResult {
    pub status: ExecStatus,
    /// 实际整数退出码；运行中、被信号终止或未执行时为 null。
    pub exit_code: Option<i32>,
    /// 本次取走的新输出（stdout/stderr 合并，PTY 为合并通道）；UTF-8 解码，
    /// 非法字节替换为 U+FFFD。
    pub output: String,
    /// 输出是否因缓存或单次上限被省略。
    pub truncated: bool,
    /// 错误说明；正常完成（status=completed）时为 null。
    pub error: Option<String>,
    /// 可续接会话句柄；运行中／取消后仍存活时为 Some，其余为 null。
    #[serde(rename = "session_id")]
    pub session_id: Option<String>,
}

impl ExecResult {
    /// 带有效会话句柄的错误构造（016 §3.3：错误不表示进程退出、不删会话——
    /// 会话仍可续接时保留句柄）。
    pub fn error_with_session(msg: String, session_id: impl Into<String>) -> Self {
        Self {
            status: ExecStatus::Error,
            exit_code: None,
            output: String::new(),
            truncated: false,
            error: Some(msg),
            session_id: Some(session_id.into()),
        }
    }

    /// 参数/启动失败：无退出码无输出；无有效会话。
    pub fn error(msg: String) -> Self {
        Self {
            status: ExecStatus::Error,
            exit_code: None,
            output: String::new(),
            truncated: false,
            error: Some(msg),
            session_id: None,
        }
    }
}

/// 解析工作目录：缺省为会话当前目录；相对路径基于该目录；必须已是存在的目录。
pub(crate) fn resolve_workdir(workdir: Option<&str>) -> Result<std::path::PathBuf, String> {
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

/// 把输出按 Unicode 码点截到上限：保留前 N 个码点并标记截断。
pub(crate) fn cap_output(output: String, max_chars: usize) -> (String, bool) {
    if output.chars().count() <= max_chars {
        return (output, false);
    }
    let capped: String = output.chars().take(max_chars).collect();
    (capped, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- ExecArgs::from_value ----

    #[test]
    fn exec_args_defaults_and_full_fields() {
        let a = ExecArgs::from_value(&json!({ "cmd": "ls" })).unwrap();
        assert_eq!(
            a,
            ExecArgs {
                cmd: "ls".into(),
                workdir: None,
                yield_time_ms: DEFAULT_YIELD_MS,
                timeout_ms: None,
                max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
                tty: false,
            }
        );
        let a = ExecArgs::from_value(&json!({
            "cmd": "ls", "workdir": "/tmp", "yield_time_ms": 500,
            "timeout_ms": 180000, "max_output_chars": 50, "tty": true
        }))
        .unwrap();
        assert_eq!(a.yield_time_ms, 500);
        assert_eq!(a.timeout_ms, Some(180000));
        assert_eq!(a.max_output_chars, 50);
        assert!(a.tty);
        // null 可选字段视为缺省
        let a = ExecArgs::from_value(&json!({ "cmd": "ls", "workdir": null, "tty": null })).unwrap();
        assert_eq!(a.workdir, None);
        assert!(!a.tty);
    }

    #[test]
    fn exec_args_reject_unknown_and_bad_cmd() {
        assert!(ExecArgs::from_value(&json!(5)).unwrap_err().contains("对象"));
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "evil": 1 }))
            .unwrap_err()
            .contains("未知参数"));
        assert!(ExecArgs::from_value(&json!({})).unwrap_err().contains("cmd"));
        assert!(ExecArgs::from_value(&json!({ "cmd": " \t " }))
            .unwrap_err()
            .contains("cmd"));
        assert!(ExecArgs::from_value(&json!({ "cmd": 5 })).unwrap_err().contains("cmd"));
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "tty": "yes" }))
            .unwrap_err()
            .contains("tty"));
    }

    #[test]
    fn exec_args_reject_bad_numbers() {
        // timeout_ms 无上限：180000（大于旧 60000）合法；零/负/小数（含 1.0）/字符串非法
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "timeout_ms": 60001 })).is_ok());
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "timeout_ms": u64::MAX })).is_ok());
        for bad in [0, -5] {
            assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "timeout_ms": bad }))
                .unwrap_err()
                .contains("timeout_ms"));
            assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "max_output_chars": bad }))
                .unwrap_err()
                .contains("max_output_chars"));
        }
        for bad in [json!("500"), json!(1.5), json!(1.0)] {
            assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "timeout_ms": bad }))
                .unwrap_err()
                .contains("timeout_ms"), "{bad}");
        }
        // yield_time_ms 越界拒绝（249、30001），不悄悄调整
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "yield_time_ms": 249 }))
            .unwrap_err()
            .contains("yield_time_ms"));
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "yield_time_ms": 30001 }))
            .unwrap_err()
            .contains("yield_time_ms"));
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "yield_time_ms": 250 })).is_ok());
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "yield_time_ms": 30000 })).is_ok());
        // max_output_chars 上限 100000
        assert!(ExecArgs::from_value(&json!({ "cmd": "ls", "max_output_chars": 100001 }))
            .unwrap_err()
            .contains("max_output_chars"));
    }

    // ---- WriteStdinArgs::from_value ----

    #[test]
    fn stdin_args_query_defaults() {
        let a = WriteStdinArgs::from_value(&json!({ "session_id": "s1" })).unwrap();
        assert_eq!(
            a,
            WriteStdinArgs {
                session_id: "s1".into(),
                chars: String::new(),
                yield_time_ms: DEFAULT_QUERY_YIELD_MS,
                max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
                terminate: false,
            }
        );
        // 空查询范围 5000..=300000
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": "s1", "yield_time_ms": 4999 }))
            .unwrap_err()
            .contains("yield_time_ms"));
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": "s1", "yield_time_ms": 300001 }))
            .unwrap_err()
            .contains("yield_time_ms"));
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": "s1", "yield_time_ms": 300000 }))
            .is_ok());
    }

    #[test]
    fn stdin_args_input_defaults_and_ranges() {
        let a =
            WriteStdinArgs::from_value(&json!({ "session_id": "s1", "chars": "y\n" })).unwrap();
        assert_eq!(a.yield_time_ms, MIN_INPUT_YIELD_MS);
        // 非空输入范围 250..=30000
        assert!(WriteStdinArgs::from_value(&json!(
            { "session_id": "s1", "chars": "y", "yield_time_ms": 31_000 }
        ))
        .unwrap_err()
        .contains("yield_time_ms"));
        assert!(WriteStdinArgs::from_value(&json!(
            { "session_id": "s1", "chars": "y", "yield_time_ms": 30_000 }
        ))
        .is_ok());
        // 恰 64 KiB 通过；超限整次拒绝
        let ok = "x".repeat(MAX_INPUT_BYTES);
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": "s1", "chars": ok })).is_ok());
        let bad = "x".repeat(MAX_INPUT_BYTES + 1);
        let e = WriteStdinArgs::from_value(&json!({ "session_id": "s1", "chars": bad }))
            .unwrap_err();
        assert!(e.contains("整次拒绝"), "{e}");
    }

    #[test]
    fn stdin_args_terminate_mutex_and_bad_fields() {
        // terminate 与非空 chars / yield_time_ms 互斥
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": "s1", "terminate": true }))
            .is_ok());
        assert!(WriteStdinArgs::from_value(&json!(
            { "session_id": "s1", "terminate": true, "chars": "x" }
        ))
        .unwrap_err()
        .contains("terminate"));
        assert!(WriteStdinArgs::from_value(&json!(
            { "session_id": "s1", "terminate": true, "yield_time_ms": 5000 }
        ))
        .unwrap_err()
        .contains("terminate"));
        // session_id / chars 类型与未知字段
        assert!(WriteStdinArgs::from_value(&json!({} ))
            .unwrap_err()
            .contains("session_id"));
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": " " }))
            .unwrap_err()
            .contains("session_id"));
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": 5 }))
            .unwrap_err()
            .contains("session_id"));
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": "s1", "chars": 5 }))
            .unwrap_err()
            .contains("chars"));
        assert!(WriteStdinArgs::from_value(&json!({ "session_id": "s1", "signal": 9 }))
            .unwrap_err()
            .contains("未知参数"));
    }

    // ---- 结果形状 ----

    #[test]
    fn result_serializes_spec_shape() {
        let r = ExecResult {
            status: ExecStatus::Running,
            exit_code: None,
            output: "x".into(),
            truncated: true,
            error: None,
            session_id: Some("s1".into()),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({
                "status": "running", "exit_code": null, "output": "x",
                "truncated": true, "error": null, "session_id": "s1"
            })
        );
        let r = ExecResult::error("参数错误".into());
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({
                "status": "error", "exit_code": null, "output": "",
                "truncated": false, "error": "参数错误", "session_id": null
            })
        );
        let r = ExecResult {
            status: ExecStatus::Terminated,
            exit_code: None,
            output: String::new(),
            truncated: false,
            error: None,
            session_id: None,
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({
                "status": "terminated", "exit_code": null, "output": "",
                "truncated": false, "error": null, "session_id": null
            })
        );
    }

    #[test]
    fn cap_output_truncates_at_char_boundary() {
        let (out, t) = cap_output("a🎉bcd".into(), 3);
        assert_eq!(out, "a🎉b");
        assert!(t);
        let (out, t) = cap_output("abc".into(), 3);
        assert_eq!(out, "abc");
        assert!(!t);
    }
}
