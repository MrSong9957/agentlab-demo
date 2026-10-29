//! 034：worker↔宿主的有界 IPC 帧（长度前缀 JSON）。
//!
//! 帧布局：8 字节小端长度＋JSON payload。初始化帧（宿主→worker，含源码与
//! 工具目录）≤10 MiB；其他帧 ≤1 MiB（§1.7）。任何坏帧（长度越界、JSON 无效）
//! 都按 IPC 断开处理：宿主关闭 cell，worker 退出。

use serde_json::Value;
use std::io::{Read, Write};

/// 初始化帧上限（源码＋完整工具目录序列化；不裁剪 Schema）。
pub const MAX_INIT_FRAME_BYTES: u64 = 10 * 1024 * 1024;
/// 其他单帧上限。
pub const MAX_FRAME_BYTES: u64 = 1024 * 1024;

/// 叶子参数上限（§1.7：64 KiB）。
pub const MAX_LEAF_ARGS_BYTES: usize = 64 * 1024;
/// 可送 JS 的叶子结果 JSON 上限（§1.7：256 KiB；超限拒绝桥接）。
pub const MAX_BRIDGE_RESULT_BYTES: usize = 256 * 1024;

/// worker 内 JS 引擎资源限制（§1.7 冻结值；不新增用户配置项）。
pub const JS_HEAP_LIMIT: usize = 64 * 1024 * 1024;
pub const JS_STACK_LIMIT: usize = 512 * 1024;
/// worker 地址空间上限（RLIMIT_AS；Linux）。
pub const WORKER_AS_LIMIT: u64 = 512 * 1024 * 1024;
/// worker 累计 CPU 上限（秒；RLIMIT_CPU 由宿主设置，SIGXCPU 终止）。
pub const WORKER_CPU_LIMIT_SECS: u64 = 5;
/// cell 墙钟上限（毫秒；从创建成功起算，含让出/模型等待；宿主监控线程强制）。
pub const WORKER_WALL_LIMIT_MS: u64 = 120_000;

/// 每次观察的输出上限：4×max_tokens 字节（UTF-8 边界；§1.7）。
pub fn observation_budget(max_tokens: u64) -> usize {
    (max_tokens.saturating_mul(4)) as usize
}

// ---------- 宿主 → worker ----------

/// 初始化：源码＋工具目录（条目为 `{name, description, parameters}`）。
pub fn init_frame(code: &str, tools: &[Value]) -> Value {
    serde_json::json!({
        "t": "init",
        "code": code,
        "tools": tools,
    })
}

/// 叶子调用回包：成功＝既有结果 JSON 解析出的对象；失败＝
/// `{code, message, execution}`（execution ∈ not_started/completed/unknown）。
pub fn leaf_frame(id: u64, outcome: Result<Value, Value>) -> Value {
    match outcome {
        Ok(result) => serde_json::json!({ "t": "leaf", "id": id, "ok": true, "result": result }),
        Err(error) => serde_json::json!({ "t": "leaf", "id": id, "ok": false, "error": error }),
    }
}

/// 宿主关闭（终止/取消/轮出口收尾）：worker 停止受理新调用并退出。
pub fn shutdown_frame(reason: &str) -> Value {
    serde_json::json!({ "t": "shutdown", "reason": reason })
}

// ---------- worker → 宿主 ----------

/// 叶子调用请求（参数必须是 JSON 对象；宿主按冻结目录核对资格）。
pub fn call_frame(id: u64, tool: &str, args: &Value) -> Value {
    serde_json::json!({ "t": "call", "id": id, "tool": tool, "args": args })
}

/// text() 输出（按调用顺序；`json`＝值是否为 JSON 序列化产物；`cont`＝
/// 单次调用超出单帧上限的延续块——宿主缓冲按调用边界合并回一条，§1.3
/// 「text 原样追加一条文本」）。
pub fn output_frame(text: &str, json: bool) -> Value {
    serde_json::json!({ "t": "output", "text": text, "json": json, "cont": false })
}

/// 延续块帧（同一次 text 的后续分片）。
pub fn output_frame_cont(text: &str, json: bool) -> Value {
    serde_json::json!({ "t": "output", "text": text, "json": json, "cont": true })
}

/// 模块终态：`ok`＝顶层 Promise 兑现；`unawaited`＝顶层结束时仍在排队/在途
/// 的叶子数（UNAWAITED_TOOLS 证据）；`error`＝脚本失败摘要。
pub fn done_frame(ok: bool, error: Option<&str>, unawaited: usize) -> Value {
    serde_json::json!({ "t": "done", "ok": ok, "error": error, "unawaited": unawaited })
}

// ---------- 帧编解码（共用） ----------

/// 写一帧（长度前缀＋JSON）。超过 `max_len` 的帧是发送侧缺陷：显式失败。
pub fn write_frame<W: Write>(out: &mut W, frame: &Value, max_len: u64) -> std::io::Result<()> {
    let payload = serde_json::to_vec(frame)
        .map_err(|e| std::io::Error::other(format!("IPC 帧序列化失败：{e}")))?;
    if payload.len() as u64 > max_len {
        return Err(std::io::Error::other(format!(
            "IPC 帧 {} 字节超过上限 {}",
            payload.len(),
            max_len
        )));
    }
    out.write_all(&(payload.len() as u64).to_le_bytes())?;
    out.write_all(&payload)?;
    out.flush()
}

/// 有界读一帧：`Ok(None)`＝EOF（对端关闭）；长度越界或 JSON 无效＝Err（坏帧，
/// 调用方按 IPC 断开处理）。读取缓冲按长度上限有界，不物化超限帧。
pub fn read_frame<R: Read>(input: &mut R, max_len: u64) -> std::io::Result<Option<Value>> {
    let mut len_buf = [0u8; 8];
    match input.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u64::from_le_bytes(len_buf);
    if len > max_len {
        return Err(std::io::Error::other(format!(
            "IPC 帧长度 {len} 超过上限 {max_len}：坏帧断开"
        )));
    }
    let mut payload = vec![0u8; len as usize];
    input.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|e| std::io::Error::other(format!("IPC 帧 JSON 无效：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip_and_bounds() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &call_frame(1, "exec_command", &serde_json::json!({"cmd":"ls"})), MAX_FRAME_BYTES).unwrap();
        let mut cur = Cursor::new(&buf);
        let back = read_frame(&mut cur, MAX_FRAME_BYTES).unwrap().unwrap();
        assert_eq!(back["t"], "call");
        assert_eq!(back["tool"], "exec_command");
        assert_eq!(back["id"], 1);
        assert!(read_frame(&mut cur, MAX_FRAME_BYTES).unwrap().is_none(), "EOF 后 None");

        // 超限帧：写侧拒绝；读侧按坏帧报错
        let big = serde_json::json!({ "t": "output", "text": "x".repeat(MAX_FRAME_BYTES as usize + 16) });
        assert!(write_frame(&mut Vec::new(), &big, MAX_FRAME_BYTES).is_err());
        let mut oversize = Vec::new();
        oversize.extend_from_slice(&((MAX_FRAME_BYTES + 1) as u64).to_le_bytes());
        let mut cur = Cursor::new(&oversize);
        assert!(read_frame(&mut cur, MAX_FRAME_BYTES).is_err());

        // 坏 JSON
        let mut bad = Vec::new();
        bad.extend_from_slice(&3u64.to_le_bytes());
        bad.extend_from_slice(b"{}}");
        let mut cur = Cursor::new(&bad);
        assert!(read_frame(&mut cur, MAX_FRAME_BYTES).is_err());
    }
}
