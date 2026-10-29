//! 034 §1.3/§1.4：exec 与 wait 的参数严格校验。
//!
//! 只接受无额外字段的 JSON 对象；null/浮点/负数/溢出一律失败，不做整数强转。
//! pragma 前缀（上游 `// @exec:` 首行配置注释，见上游 execute_spec.rs 的
//! CODE_MODE_FREEFORM_GRAMMAR）显式报参数错误——不把错误配置当普通注释忽略。

use serde_json::Value;

/// exec `code` 上限（64 KiB UTF-8）。
pub const MAX_CODE_BYTES: usize = 64 * 1024;
/// exec `yield_time_ms`：0..=60000，缺省 10000（0＝不等待）。
pub const EXEC_MIN_YIELD_MS: u64 = 0;
pub const EXEC_MAX_YIELD_MS: u64 = 60_000;
pub const EXEC_DEFAULT_YIELD_MS: u64 = 10_000;
/// `max_output_tokens`：1..=16000，缺省 10000（本地 UTF-8 字节预算）。
pub const MAX_OUTPUT_TOKENS_LIMIT: u64 = 16_000;
pub const DEFAULT_OUTPUT_TOKENS: u64 = 10_000;
/// wait `cell_id` 上限（128 UTF-8 字节，非空，原样精确匹配）。
pub const MAX_CELL_ID_BYTES: usize = 128;
/// 原始 exec arguments 整体上限（JSON 转义后 512 KiB；§1.7）。
pub const MAX_EXEC_ARGUMENTS_BYTES: usize = 512 * 1024;

/// 解析后的 exec 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecArgs {
    /// 原样保存的源码（非全空白；编译为异步脚本，支持顶层 await）。
    pub code: String,
    pub yield_time_ms: u64,
    pub max_output_tokens: u64,
}

/// 解析后的 wait 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitArgs {
    pub cell_id: String,
    pub yield_time_ms: u64,
    pub max_tokens: u64,
    pub terminate: bool,
}

/// 上游 freeform 工具的首行 pragma（`// @exec:`）——本项目 JSON 输入不接受，
/// 显式报参数错误（§1.3：不静默当注释忽略）。
fn is_pragma_line(first_line: &str) -> bool {
    let trimmed = first_line.trim_start_matches([' ', '\t']);
    trimmed.starts_with("// @exec:")
}

/// exec 参数解析：对象恰含 `code`（必填）与 `yield_time_ms`/`max_output_tokens`
/// （可选整数）；其他字段、类型错误、越界、pragma 前缀均失败。
pub fn parse_exec_arguments(arguments: &str) -> Result<ExecArgs, String> {
    if arguments.len() > MAX_EXEC_ARGUMENTS_BYTES {
        return Err(format!(
            "exec 参数超过 {} KiB 上限，已拒绝",
            MAX_EXEC_ARGUMENTS_BYTES / 1024
        ));
    }
    let value: Value = serde_json::from_str(arguments)
        .map_err(|e| format!("exec 参数不是有效 JSON：{e}"))?;
    let object = value.as_object().ok_or("exec 参数必须是 JSON 对象")?;
    for key in object.keys() {
        if !matches!(key.as_str(), "code" | "yield_time_ms" | "max_output_tokens") {
            return Err(format!("未知参数：{key}（exec 只接受 code/yield_time_ms/max_output_tokens）"));
        }
    }
    let code = match object.get("code") {
        None | Some(Value::Null) => return Err("缺少必填参数 code".into()),
        Some(c) => c.as_str().ok_or("code 必须是字符串")?,
    };
    if code.trim().is_empty() {
        return Err("code 不能为空或全空白".into());
    }
    let code_bytes = code.len();
    if code_bytes > MAX_CODE_BYTES {
        return Err(format!(
            "code 超过 {} KiB UTF-8 上限（实际 {} 字节）",
            MAX_CODE_BYTES / 1024,
            code_bytes
        ));
    }
    // 首行（按 \n 切；CRLF 由 trim_end 处理）：上游 pragma 前缀显式拒绝。
    if let Some(first_line) = code.split('\n').next() {
        if is_pragma_line(first_line) {
            return Err(
                "code 首行是 // @exec: pragma：本工具不接受上游 freeform 的首行配置注释，请使用 JSON code 字段直接提供源码"
                    .into(),
            );
        }
    }
    let yield_time_ms = optional_bounded_int(object, "yield_time_ms", EXEC_MIN_YIELD_MS, EXEC_MAX_YIELD_MS)?
        .unwrap_or(EXEC_DEFAULT_YIELD_MS);
    let max_output_tokens =
        optional_bounded_int(object, "max_output_tokens", 1, MAX_OUTPUT_TOKENS_LIMIT)?
            .unwrap_or(DEFAULT_OUTPUT_TOKENS);
    Ok(ExecArgs {
        code: code.to_string(),
        yield_time_ms,
        max_output_tokens,
    })
}

/// wait 参数解析：对象恰含 `cell_id`（必填）与 `yield_time_ms`/`max_tokens`/
/// `terminate`（可选）；禁止额外字段。
pub fn parse_wait_arguments(arguments: &str) -> Result<WaitArgs, String> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|e| format!("wait 参数不是有效 JSON：{e}"))?;
    let object = value.as_object().ok_or("wait 参数必须是 JSON 对象")?;
    for key in object.keys() {
        if !matches!(key.as_str(), "cell_id" | "yield_time_ms" | "max_tokens" | "terminate") {
            return Err(format!(
                "未知参数：{key}（wait 只接受 cell_id/yield_time_ms/max_tokens/terminate）"
            ));
        }
    }
    let cell_id = match object.get("cell_id") {
        None | Some(Value::Null) => return Err("缺少必填参数 cell_id".into()),
        Some(c) => c.as_str().ok_or("cell_id 必须是字符串")?,
    };
    if cell_id.is_empty() || cell_id.trim().is_empty() {
        return Err("cell_id 不能为空或全空白".into());
    }
    if cell_id.len() > MAX_CELL_ID_BYTES {
        return Err(format!(
            "cell_id 超过 {} UTF-8 字节上限（实际 {} 字节）",
            MAX_CELL_ID_BYTES,
            cell_id.len()
        ));
    }
    let yield_time_ms = optional_bounded_int(object, "yield_time_ms", 0, 60_000)?
        .unwrap_or(10_000);
    let max_tokens = optional_bounded_int(object, "max_tokens", 1, MAX_OUTPUT_TOKENS_LIMIT)?
        .unwrap_or(DEFAULT_OUTPUT_TOKENS);
    let terminate = match object.get("terminate") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("terminate 必须是布尔值".into()),
    };
    Ok(WaitArgs {
        cell_id: cell_id.to_string(),
        yield_time_ms,
        max_tokens,
        terminate,
    })
}

/// 可选整数字段：缺失/null＝None；零值按范围校验（exec 的 yield_time_ms 合法
/// 0，wait 同）；浮点（含 1.0）、负数、字符串、越界一律失败——不强制转换。
fn optional_bounded_int(
    object: &serde_json::Map<String, Value>,
    name: &str,
    min: u64,
    max: u64,
) -> Result<Option<u64>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) if v.is_u64() => match v.as_u64() {
            Some(n) if (min..=max).contains(&n) => Ok(Some(n)),
            Some(n) => Err(format!("{name} 必须为 {min}..={max} 的整数（实际 {n}）")),
            None => Err(format!("{name} 必须为 {min}..={max} 的整数")),
        },
        Some(_) => Err(format!(
            "{name} 必须为 {min}..={max} 的整数（不接受浮点、负数、字符串或 null 之外的类型）"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exec_defaults_and_bounds() {
        let a = parse_exec_arguments(r#"{"code":"text(1)"}"#).unwrap();
        assert_eq!(a.yield_time_ms, EXEC_DEFAULT_YIELD_MS);
        assert_eq!(a.max_output_tokens, DEFAULT_OUTPUT_TOKENS);
        assert_eq!(a.code, "text(1)");
        // 边界：yield 0 与 60000、tokens 1 与 16000 恰好通过
        assert!(parse_exec_arguments(r#"{"code":"x","yield_time_ms":0}"#).is_ok());
        assert!(parse_exec_arguments(r#"{"code":"x","yield_time_ms":60000}"#).is_ok());
        assert!(parse_exec_arguments(r#"{"code":"x","max_output_tokens":16000}"#).is_ok());
        assert!(parse_exec_arguments(r#"{"code":"x","max_output_tokens":1}"#).is_ok());
    }

    #[test]
    fn exec_rejects_bad_shapes() {
        // 非对象 / 缺 code / 空白 / 未知字段 / 类型错误 / 浮点 / 越界
        assert!(parse_exec_arguments("null").is_err());
        assert!(parse_exec_arguments(r#""code""#).is_err());
        assert!(parse_exec_arguments("{}").is_err());
        assert!(parse_exec_arguments(r#"{"code":"   \t "}"#).is_err());
        assert!(parse_exec_arguments(r#"{"code":5}"#).is_err());
        assert!(parse_exec_arguments(r#"{"code":"x","runtime":"node"}"#).is_err());
        assert!(parse_exec_arguments(r#"{"code":"x","yield_time_ms":60001}"#).is_err());
        assert!(parse_exec_arguments(r#"{"code":"x","yield_time_ms":1.0}"#).is_err());
        assert!(parse_exec_arguments(r#"{"code":"x","yield_time_ms":-1}"#).is_err());
        assert!(parse_exec_arguments(r#"{"code":"x","max_output_tokens":0}"#).is_err());
        assert!(parse_exec_arguments(r#"{"code":"x","max_output_tokens":16001}"#).is_err());
        // 64 KiB 恰好通过、超过拒绝
        let exact = "x".repeat(MAX_CODE_BYTES);
        assert!(parse_exec_arguments(&json!({"code": exact}).to_string()).is_ok());
        let over = "x".repeat(MAX_CODE_BYTES + 1);
        assert!(parse_exec_arguments(&json!({"code": over}).to_string()).is_err());
    }

    #[test]
    fn exec_rejects_pragma_prefix_and_keeps_crlf_source() {
        // 上游 freeform 首行 pragma 显式拒绝（不静默当注释）
        let e = parse_exec_arguments("{\"code\":\"// @exec: tools=[shell]\\ntext(1)\"}")
            .unwrap_err();
        assert!(e.contains("pragma"), "{e}");
        // 普通注释不是 pragma：原样通过
        assert!(parse_exec_arguments("{\"code\":\"// 普通\\ntext(1)\"}").is_ok());
        // CRLF 源码原样保留
        let a = parse_exec_arguments("{\"code\":\"text(1)\\r\\n\"}").unwrap();
        assert_eq!(a.code, "text(1)\r\n");
    }

    #[test]
    fn wait_defaults_and_bounds() {
        let w = parse_wait_arguments(r#"{"cell_id":"c1"}"#).unwrap();
        assert_eq!(w.yield_time_ms, 10_000);
        assert_eq!(w.max_tokens, 10_000);
        assert!(!w.terminate);
        assert!(parse_wait_arguments(r#"{"cell_id":"c1","yield_time_ms":0}"#).is_ok());
        assert!(parse_wait_arguments(r#"{"cell_id":"c1","terminate":true}"#).is_ok());
        assert!(parse_wait_arguments(r#"{"cell_id":"c1","terminate":null}"#).is_ok());
    }

    #[test]
    fn wait_rejects_bad_shapes() {
        assert!(parse_wait_arguments("{}").is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":""}"#).is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":"  "}"#).is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":5}"#).is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":"c","cell":"c"}"#).is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":"c","yield_time_ms":60001}"#).is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":"c","yield_time_ms":0.5}"#).is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":"c","max_tokens":0}"#).is_err());
        assert!(parse_wait_arguments(r#"{"cell_id":"c","terminate":"yes"}"#).is_err());
        // 128 字节边界
        let exact = "c".repeat(MAX_CELL_ID_BYTES);
        assert!(parse_wait_arguments(&json!({"cell_id": exact}).to_string()).is_ok());
        let over = "c".repeat(MAX_CELL_ID_BYTES + 1);
        assert!(parse_wait_arguments(&json!({"cell_id": over}).to_string()).is_err());
    }
}
