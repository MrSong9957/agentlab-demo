//! 模型协议适配、原生模型目录与有界 HTTP；不自动重试或切换协议。

use crate::catalog::{
    check_request_param, now_secs, AutoCatalog, EffectiveConnection, ModelInfo, SUPPORTED_PROTOCOL,
};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;
use url::Url;

mod native;
mod stream;

pub fn agent() -> ureq::Agent {
    agent_with_timeouts(Duration::from_secs(10), Duration::from_secs(180))
}

/// 可配置超时的客户端：连接超时与总超时（读响应、重定向、读 body）分开。
/// 重定向一律禁用（redirects=0）：凭证只发往配置并已授权的地址，
/// 3xx 由调用方明确拒绝，防止凭证被转发到未授权地址。
pub fn agent_with_timeouts(connect: Duration, total: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout(total)
        .redirects(0)
        .build()
}

fn auth_header(key: &str) -> String {
    format!("Bearer {key}")
}

fn http_context(response:&ureq::Response,key:&str)->String {
    let id=response.header("x-request-id").or_else(||response.header("request-id"))
        .map(|s|crate::display::sanitize_for_terminal(&crate::display::truncate_chars(&redact_credential(s.into(),key),200)));
    format!("（HTTP {}{}）",response.status(),id.map(|s|format!(", request_id={s}")).unwrap_or_default())
}

fn model_error(conn:&EffectiveConnection,key:&str,model:&ModelInfo,request:&Value,error:String)->String {
    let provider=request.get("_provider_id").and_then(Value::as_str).unwrap_or("未指定厂商配置");
    redact_data_urls(redact_credential(format!("厂商 {provider}，模型 {}，协议 {}：{error}",model.id,conn.protocol),key))
}

fn validate_model_base(base_url:&str)->Result<(),String> {
    crate::catalog::validate_base_url(base_url)?;
    let url=Url::parse(base_url).map_err(|_|"基础地址无效")?;
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("模型基础地址不能包含用户名、密码、query 或 fragment；鉴权使用 key_ref".into());
    }
    Ok(())
}

fn validate_request(conn:&EffectiveConnection,key:&str,request:&Value)->Result<(),String> {
    validate_model_base(&conn.base_url)?;
    check_key_header_safe(key)?;
    if !request.is_object() || !request.get("messages").is_some_and(Value::is_array) {
        return Err("模型请求必须是对象并包含 messages 数组".into());
    }
    if request.get("tools").is_some_and(|t|!t.is_array()) {return Err("工具定义必须是数组".into());}
    if conn.request_params.get("stream").is_some_and(|s|!s.is_boolean()) {return Err("stream 必须是布尔值".into());}
    Ok(())
}

fn validate_assistant(message: &Value) -> Result<(), String> {
    if !message.is_object() { return Err("响应的 assistant 必须是对象".into()); }
    if let Some(calls)=message.get("tool_calls").and_then(Value::as_array) {
        if calls.len()>crate::prompt::MAX_TOOL_CALLS_PER_BATCH {
            return Err(format!("单批工具调用 {} 条超过上限 32，整批拒绝",calls.len()));
        }
    }
    crate::prompt::ToolCallRecord::parse_batch(message)?;
    Ok(())
}

/// 响应体读取上限：按实际读到的字节计，防止恶意服务端耗尽内存。
pub const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;

/// 逐块计数读取器：实际解码字节数超过上限即报错，不依赖 Content-Length。
struct LimitedReader<R: std::io::Read> {
    inner: R,
    remaining: u64,
}

impl<R: std::io::Read> std::io::Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n == 0 {
            return Ok(0);
        }
        if n as u64 > self.remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "body over limit",
            ));
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

fn body_read_error(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::InvalidData {
        format!(
            "响应体超过 {} MiB 上限，已中止读取",
            MAX_BODY_BYTES / 1024 / 1024
        )
    } else {
        format!("读取响应失败：{e}")
    }
}

/// 在上限内读取响应体为字符串（增量读取，不先整包载入）。
fn read_limited_string(res: ureq::Response) -> Result<String, String> {
    let mut limited = LimitedReader { inner: res.into_reader(), remaining: MAX_BODY_BYTES };
    let mut text = String::new();
    limited.read_to_string(&mut text).map_err(body_read_error)?;
    Ok(text)
}

/// Key 里的换行/控制字符无法安全放入请求头，且底层报错会回显其值：
/// 发送前拦截，给出明确错误，避免泄露。
fn check_key_header_safe(key: &str) -> Result<(), String> {
    if key.chars().any(|c| c == '\n' || c == '\r' || c.is_control()) {
        return Err("密钥包含换行或控制字符，无法用于请求头".into());
    }
    Ok(())
}

/// 把厂商错误响应翻译成明确原因；密钥相关内容由调用方保证不落日志。
/// 服务端可控内容统一截断到 300 字符（按字符截断，中文安全）。
fn describe_status_error(status: u16, body: &str, secret: Option<&str>) -> String {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.to_string());
    // 先脱敏再截断：截断会把横跨边界的 Key 切成片段，事后的整串替换匹配不到残留
    let msg = match secret {
        Some(s) => redact_credential(msg, s),
        None => msg,
    };
    // 019：图片载荷先遮蔽再截断（同因：截断后残留无法整串匹配）
    let msg = redact_data_urls(msg);
    let msg = crate::display::truncate_chars(&msg, 300);
    match status {
        401 => format!("鉴权失败（401）：Key 无效或未授权。服务返回：{msg}"),
        403 => format!("无权限（403）。服务返回：{msg}"),
        404 => format!("接口不存在（404）：检查基础地址与协议是否正确。服务返回：{msg}"),
        429 => format!("触发限流（429）。服务返回：{msg}"),
        s if (300..400).contains(&s) => format!(
            "拒绝跟随重定向（{s}）：出站地址必须与配置并已授权的地址一致；如服务地址变更，请更新配置并重新授权。服务返回：{msg}"
        ),
        s if s >= 500 => format!("服务端错误（{s}）。服务返回：{msg}"),
        _ => {
            let lower = msg.to_lowercase();
            if lower.contains("context length") || lower.contains("maximum context") {
                format!("上下文超限（{status}）：{msg}")
            } else {
                format!("请求失败（{status}）：{msg}")
            }
        }
    }
}

fn send(req: ureq::Request, secret: Option<&str>) -> Result<String, String> {
    match req.call() {
        Ok(res) => {
            // 重定向一律拒绝：ureq redirects(0) 下 3xx 可能以 Ok 形式返回
            let status = res.status();
            if (300..400).contains(&status) {
                return Err(describe_status_error(status, "", secret));
            }
            read_limited_string(res)
        }
        Err(ureq::Error::Status(code, res)) => match read_limited_string(res) {
            Ok(text) => Err(describe_status_error(code, &text, secret)),
            Err(e) => Err(format!("状态码 {code}：{e}")),
        },
        Err(e) => Err(format!("网络错误：{e}")),
    }
}

/// 参数能力检查放在组装请求体之前，不通过的参数不发送任何请求。
fn checked_params(
    conn: &EffectiveConnection,
    model: &ModelInfo,
    body: &mut Value,
) -> Result<(), String> {
    for (k, v) in &conn.request_params {
        if matches!(k.as_str(), "model" | "messages") {
            return Err(format!("request_params 不能包含保留字段 {k}"));
        }
        check_request_param(model, k)?;
        body.as_object_mut()
            .expect("请求体必须是对象")
            .insert(k.clone(), v.clone());
    }
    Ok(())
}

/// 错误文本脱敏：服务端可能在 error.message 中回显请求凭证，任何进入用户可见
/// 错误的文本都必须先抹掉当前凭证值（300 字符截断不能成为唯一防线）。
fn redact_credential(msg: String, secret: &str) -> String {
    if secret.is_empty() {
        msg
    } else {
        msg.replace(secret, "***")
    }
}

/// 019：错误回显中的图片载荷遮蔽——服务端错误可能回显请求里的 data URL，
/// 先替换载荷（保留 MIME 提示）再截断，避免 base64 大段进入用户可见输出。
fn redact_data_urls(msg: String) -> String {
    let mut out = String::with_capacity(msg.len());
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if msg[i..].starts_with("data:") {
            let rest = &msg[i + 5..];
            // 形如 <mime>[;base64],<载荷>；载荷＝到下一个非 base64/非 URL 安全字符
            if let Some((meta, payload_start)) = split_data_url_meta(rest) {
                let payload = &rest[payload_start..];
                let payload_len = payload
                    .find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '=' | '/' | '+' | '-' | '_'))
                    .unwrap_or(payload.len());
                out.push_str("data:");
                out.push_str(meta);
                out.push_str(&format!("<图片载荷已遮蔽：{} 字符>", payload_len));
                i += 5 + payload_start + payload_len;
                continue;
            }
        }
        let ch = msg[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// data URL 元数据段（含结尾逗号前的 `;base64` 等参数）：返回 (meta, 载荷起始偏移)。
fn split_data_url_meta(rest: &str) -> Option<(&str, usize)> {
    let comma = rest.find(',')?;
    let meta = &rest[..comma];
    if meta.is_empty() || meta.len() > 64 {
        return None;
    }
    if !meta.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b';' | b'-' | b'+')) {
        return None;
    }
    Some((meta, comma + 1))
}

/// 出站请求体上限（2026-10-10 安全修复 N4）：含 model/messages/tools/request_params
/// 序列化后的最终 JSON 总字节，超限零发送。
pub const MAX_REQUEST_BODY_BYTES: u64 = 10 * 1024 * 1024;

/// 限计量写入器：写满上限即失败，不物化超限缓冲（检查过程不生成无界大字符串）。
struct LimitCountingWriter {
    written: u64,
    limit: u64,
}

impl std::io::Write for LimitCountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = buf.len() as u64;
        if self.written + n > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "request body over limit",
            ));
        }
        self.written += n;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 032 M1：定型出站请求（纯函数，零 HTTP/零鉴权）——按协议完成 URL 构造与
/// 最终 body 纯化（Gemini 移除用于 URL 的 model/stream）。真实发送
/// （[`post_chat_json`]）与只读计量（[`measure_request_body_bytes`]）共用本阶段，
/// 计量终点＝实际发送字节（不计量移除 model/stream 之前的中间表示）。
pub(crate) struct Outbound {
    pub url: String,
    pub body: Value,
    pub streaming: bool,
}

pub(crate) fn finalize_outbound(
    conn: &EffectiveConnection,
    mut body: Value,
) -> Result<Outbound, String> {
    let streaming = body.get("stream") == Some(&json!(true));
    let mut url = Url::parse(&conn.base_url).map_err(|e| format!("基础地址无效：{e}"))?;
    {
        let mut path = url.path_segments_mut().map_err(|_| "基础地址不能拼接操作路径")?;
        path.pop_if_empty();
        match conn.protocol.as_str() {
            "openai-responses" => { path.push("responses"); }
            "anthropic-messages" => { path.push("messages"); }
            "google-interactions" => { path.push("interactions"); }
            "google-generate-content" => {
                let model = body.as_object_mut().and_then(|b| b.remove("model"))
                    .and_then(|v| v.as_str().map(str::to_owned)).ok_or("Gemini 请求缺少模型 ID")?;
                // 资源名前缀只去一次，其余整体作为一个路径段编码，不解释已有 % 转义。
                let operation = if streaming { "streamGenerateContent" } else { "generateContent" };
                path.push("models").push(&format!("{}:{operation}", model.strip_prefix("models/").unwrap_or(&model)));
                body.as_object_mut().expect("已检查对象").remove("stream");
            }
            _ => { path.push("chat").push("completions"); }
        }
    }
    if conn.protocol == "google-generate-content" && streaming {
        url.query_pairs_mut().append_pair("alt", "sse");
    }
    Ok(Outbound { url: url.into(), body, streaming })
}

/// 发送已构造完成的对话请求，返回解析后的响应 JSON 与 HTTP 上下文：chat 与
/// chat_with_body 共用的 URL 构造、鉴权头、重定向拒绝、请求体上限与错误映射路径。
/// 032 M1 起 URL/body 定型经 [`finalize_outbound`]（与只读计量共用）。
fn post_chat_json(
    conn: &EffectiveConnection,
    key: &str,
    body: Value,
    agent: &ureq::Agent,
) -> Result<(Value,String), String> {
    let outbound = finalize_outbound(conn, body)?;
    let streaming = outbound.streaming;
    // N4b：发送前限计量序列化检查（超限零发送，本轮由调用方结束且不重发）
    let mut counter = LimitCountingWriter { written: 0, limit: MAX_REQUEST_BODY_BYTES };
    if serde_json::to_writer(&mut counter, &outbound.body).is_err() {
        return Err(format!(
            "模型请求体超过 {} MiB 上限，已拒绝发送（含 model/messages/tools/request_params 序列化后总字节）",
            MAX_REQUEST_BODY_BYTES / 1024 / 1024
        ));
    }
    let req = agent.post(outbound.url.as_str()).set("Content-Type", "application/json")
        .set("Accept",if streaming {"text/event-stream"} else {"application/json"});
    let req = if conn.protocol == "anthropic-messages" {
        req.set("x-api-key", key).set("anthropic-version", "2026-10-21")
    } else if matches!(conn.protocol.as_str(), "google-interactions" | "google-generate-content") {
        req.set("x-goog-api-key", key)
    } else {
        req.set("Authorization", &auth_header(key))
    };
    match req.send_json(outbound.body) {
        Ok(r) => {
            let status = r.status();
            let context=http_context(&r,key);
            if (300..400).contains(&status) {
                return Err(format!("{}{context}",describe_status_error(status, "", Some(key))));
            }
            let content_type=r.header("Content-Type").unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            let result=(|| {
                let expected=if streaming {"text/event-stream"} else {"application/json"};
                if content_type!=expected {return Err(format!("非预期 Content-Type：{content_type}，期望 {expected}"));}
                let text=read_limited_string(r)?;
                let value:Value=if streaming {stream::decode(&conn.protocol,&text)} else {
                    serde_json::from_str(&text).map_err(|e|format!("响应解析失败：{e}"))
                }?;
                if value.get("error").is_some_and(|e|!e.is_null()) {
                    return Err("响应包含服务端 error，已拒绝继续".into());
                }
                if conn.protocol=="openai-chat" {
                    if let Some(message)=value.pointer("/choices/0/message") {
                        validate_assistant(message)?;
                        if message.get("tool_calls").and_then(Value::as_array).is_some_and(|calls|!calls.is_empty())
                            && value.pointer("/choices/0/finish_reason").and_then(Value::as_str)
                                .is_some_and(|r|!matches!(r,"stop"|"tool_calls")) {
                            return Err("工具响应未完整结束，整批拒绝".into());
                        }
                    }
                }
                Ok(value)
            })();
            result.map(|value|(value,context.clone())).map_err(|e|format!("{e}{context}"))
        }
        Err(ureq::Error::Status(code, r)) => {
            let context=http_context(&r,key);
            match read_limited_string(r) {
                Ok(text) => Err(format!("{}{context}",describe_status_error(code,&text,Some(key)))),
                Err(e) => Err(format!("状态码 {code}：{e}{context}")),
            }
        }
        Err(e) => Err(format!("网络错误：{e}")),
    }
}

/// 非流式对话：发送用户输入，返回完整回复文本。
pub fn chat(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    input: &str,
    agent: &ureq::Agent,
) -> Result<String, String> {
    if native::is_native(&conn.protocol) {
        return reply_text(&chat_with_body(conn, key, model,
            &json!({"messages":[{"role":"user","content":input}]}), agent)?);
    }
    if conn.protocol != SUPPORTED_PROTOCOL {
        return Err(format!(
            "协议 {} 未支持，当前仅支持 {}",
            conn.protocol, SUPPORTED_PROTOCOL
        ));
    }
    let mut body = json!({
        "model": model.id,
        "messages": [{ "role": "user", "content": input }],
    });
    validate_request(conn,key,&body).map_err(|e|model_error(conn,key,model,&body,e))?;
    checked_params(conn, model, &mut body)?;
    check_key_header_safe(key)?;
    let (v,context) = post_chat_json(conn,key,body,agent).map_err(|e|model_error(conn,key,model,&Value::Null,e))?;
    reply_text(&v).map_err(|e|model_error(conn,key,model,&Value::Null,format!("{e}{context}")))
}

/// 从 chat 响应 JSON 提取回复文本（`chat` 与 013-M3 提炼采样共用；错误文案
/// 单一来源）。content 为空即失败，含 finish_reason 诊断与 reasoning-only
/// 截断提示。
pub(crate) fn reply_text(v: &Value) -> Result<String, String> {
    let content = v
        .pointer("/assistant/content")
        .or_else(|| v.pointer("/choices/0/message/content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if content.is_empty() {
        let finish = v
            .pointer("/choices/0/finish_reason")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        let reasoning = v
            .pointer("/choices/0/message/reasoning_content")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        if !reasoning.is_empty() {
            return Err(format!(
                "模型只输出了思考过程就被截断（finish_reason={finish}），未产生正式回复；可提高 max_tokens"
            ));
        }
        return Err(format!("回复为空（finish_reason={finish}）"));
    }
    Ok(content.to_string())
}

/// 019：请求消息面是否含图片内容（user 部件 `image_url`）。只做形态检查，
/// 不解码载荷——发送前的能力门与 10 MiB 计量各自独立。
fn request_has_image_content(request: &Value) -> bool {
    request
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|m| {
                m.get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts
                            .iter()
                            .any(|p| p.get("type").and_then(Value::as_str) == Some("image_url"))
                    })
            })
        })
}

/// 完整请求体对话（006 工具循环）：`request` 为 005 组装产物 `{messages, tools}`，
/// 附上 `model` 与已校验的 request_params 后发送，返回完整响应 JSON（含 tool_calls）。
/// 协议检查、鉴权、重定向拒绝与错误映射与 `chat` 完全一致；`chat` 原行为不变。
pub fn chat_with_provider_body(
    provider_id: &str,
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
    agent: &ureq::Agent,
) -> Result<Value, String> {
    let mut request = request.clone();
    request.as_object_mut().ok_or("模型请求必须是对象")?.insert("_provider_id".into(),json!(provider_id));
    chat_with_body(conn, key, model, &request, agent)
}

pub fn chat_with_body(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
    agent: &ureq::Agent,
) -> Result<Value, String> {
    chat_with_body_inner(conn,key,model,request,agent).map_err(|e|model_error(conn,key,model,request,e))
}

fn chat_with_body_inner(
    conn:&EffectiveConnection,key:&str,model:&ModelInfo,request:&Value,agent:&ureq::Agent,
)->Result<Value,String> {
    validate_request(conn,key,request)?;
    if native::is_native(&conn.protocol) {
        return native::request(conn, key, model, request, agent);
    }
    if conn.protocol != SUPPORTED_PROTOCOL {
        return Err(format!(
            "协议 {} 未支持，当前仅支持 {}",
            conn.protocol, SUPPORTED_PROTOCOL
        ));
    }
    // 032 M1：纯组装与只读计量共用；图片能力门与密钥头检查留在发送侧。
    let body = build_chat_body(conn, key, model, request)?;
    // 019 §3.2：发送前再次检查图片消息与所选模型能力——恢复图片历史后切到
    // 文本/能力未知模型时明确本地拒绝（零发送），不静默丢图、不发不支持的请求。
    if request_has_image_content(&body) && !crate::catalog::supports_image_input(model) {
        return Err(format!("当前模型 {} 未确认支持图片输入（input_modalities 缺少 image），已拒绝发送",model.id));
    }
    check_key_header_safe(key)?;
    post_chat_json(conn,key,body,agent).map(|(value,_)|value)
}

/// 032 M1：openai-chat 纯 body 组装（零 HTTP）——保留字段前置拒绝、`_history`
/// 投影（兼容 messages 与共同历史共用同一活动视图）、`model` 注入与已校验
/// request_params 并入。真实发送与只读计量共用；图片能力门不在本层（计量对
/// 图片内容返回未知而非失败，见 032 §1.3）。`key` 仅参与来源身份指纹，不进入 body。
pub(crate) fn build_chat_body(
    conn:&EffectiveConnection,
    key:&str,
    model:&ModelInfo,
    request:&Value,
) -> Result<Value,String> {
    // 006 收尾：核心请求字段（model/messages/tools）由程序组装，request_params
    // 一律不得携带——shared 检查只硬拒 model/messages，此处补 tools 并统一前置拒绝
    if let Some(k) = conn
        .request_params
        .keys()
        .find(|k| matches!(k.as_str(), "model" | "messages" | "tools" | "_history" | "_provider_id"))
    {
        return Err(format!("request_params 不能包含保留字段 {k}"));
    }
    let mut body = request.clone();
    if request.get("_history").is_some() {
        let current=native::owner(conn,key,model,request);
        let items=native::for_owner(native::history(request)?,&current)?;
        let mut messages:Vec<Value>=request.get("messages").and_then(Value::as_array).into_iter().flatten()
            .filter(|m|m["role"]=="system").cloned().collect();
        messages.extend(crate::prompt::project_history(&items));
        body["messages"]=json!(messages);
    }
    body.as_object_mut().ok_or("模型请求必须是对象")?.remove("_history");
    body.as_object_mut().expect("已确认对象").remove("_provider_id");
    body["model"] = json!(model.id);
    checked_params(conn, model, &mut body)?;
    Ok(body)
}

/// 032 M1：只读计量出站请求字节数（零 HTTP/零工具/零文件加载）。
/// `request` 为 turn 组装产物（messages/tools/_history，与真实发送同构）；
/// 各协议经同一纯组装与 [`finalize_outbound`] 定型后计数——计量终点＝最终
/// 发送 body（Gemini 已移除 URL 化的 model/stream）。
pub fn measure_request_body_bytes(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
) -> Result<u64, String> {
    if native::is_native(&conn.protocol) {
        return native::measure_outbound(conn, key, model, request);
    }
    if conn.protocol != SUPPORTED_PROTOCOL {
        return Err(format!(
            "协议 {} 未支持，当前仅支持 {}",
            conn.protocol, SUPPORTED_PROTOCOL
        ));
    }
    let body = build_chat_body(conn, key, model, request)?;
    let outbound = finalize_outbound(conn, body)?;
    count_body_bytes(&outbound.body)
}

/// 032 审查修复 #9：有界计量——超过 limit 即停止并返回 Ok(None)，不完成
/// 全量序列化（切换体积校验对超限旧窗口只需下界结论）。
pub fn measure_request_body_bytes_bounded(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
    limit: u64,
) -> Result<Option<u64>, String> {
    let body = if native::is_native(&conn.protocol) {
        native::build_body_for_measure(conn, key, model, request)?
    } else if conn.protocol == SUPPORTED_PROTOCOL {
        build_chat_body(conn, key, model, request)?
    } else {
        return Err(format!(
            "协议 {} 未支持，当前仅支持 {}",
            conn.protocol, SUPPORTED_PROTOCOL
        ));
    };
    let outbound = finalize_outbound(conn, body)?;
    let mut counter = LimitCountingWriter { written: 0, limit };
    if serde_json::to_writer(&mut counter, &outbound.body).is_err() {
        return Ok(None); // 超限即停
    }
    Ok(Some(counter.written))
}

/// 最终 body 的流式字节计数（不物化整串；Value 已在内存，计数无上限——
/// 10 MiB 发送硬边界由发送侧 N4b 独立把关）。
fn count_body_bytes(body: &Value) -> Result<u64, String> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len() as u64;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, body).map_err(|e| format!("计量序列化失败：{e}"))?;
    Ok(counter.0)
}

/// 查询厂商模型列表；有分页（has_more/after 游标）就读完整。
pub fn list_models(
    conn: &EffectiveConnection,
    key: &str,
    agent: &ureq::Agent,
) -> Result<Vec<ModelInfo>, String> {
    if conn.protocol != SUPPORTED_PROTOCOL && !native::is_native(&conn.protocol) {
        return Err(format!(
            "协议 {} 未支持，当前仅支持 {}",
            conn.protocol, SUPPORTED_PROTOCOL
        ));
    }
    validate_model_base(&conn.base_url)?;
    check_key_header_safe(key)?;
    let google=conn.protocol.starts_with("google-");
    let messages=conn.protocol=="anthropic-messages";
    let base = conn.base_url.trim_end_matches('/');
    let mut url =
        Url::parse(&format!("{base}/models")).map_err(|e| format!("基础地址无效：{e}"))?;
    let mut out = Vec::new();
    let mut cursors=std::collections::HashSet::new();
    for page in 1..=100 {
        let request=agent.get(url.as_str());
        let request=if google {request.set("x-goog-api-key",key)}
            else if messages {request.set("x-api-key",key).set("anthropic-version","2026-10-21")}
            else {request.set("Authorization",&auth_header(key))};
        let text = send(request, Some(key))
            .map_err(|e| redact_credential(e, key))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| redact_credential(format!("模型列表响应解析失败：{e}"), key))?;
        let field=if google {"models"} else {"data"};
        let Some(items) = v.get(field).and_then(|d| d.as_array()) else {
            return Err(format!("模型列表响应缺少 {field} 数组"));
        };
        for (idx, item) in items.iter().enumerate() {
            // 缺 ID / 非法 ID 条目使名单完整性无法确认：整次查询按失败处理，不写半份名单
            let Some(raw_id) = item.get(if google {"name"} else {"id"}).and_then(|x| x.as_str()) else {
                return Err(format!(
                    "模型列表第 {page} 页第 {} 条缺少 id，无法确认名单完整，已放弃本次替换",
                    idx + 1
                ));
            };
            let id=if google {raw_id.strip_prefix("models/").ok_or("Google 模型资源名缺少 models/ 前缀")?} else {raw_id};
            if !valid_model_id(id) {
                return Err(format!(
                    "模型列表第 {page} 页第 {} 条 id 非法（含空白/控制字符或超长），已放弃本次替换",
                    idx + 1
                ));
            }
            let mut info = ModelInfo { id: id.to_string(), ..Default::default() };
            // 只映射响应里明确返回的能力字段，没返回的不补造；展示文字过显示边界
            if let Some(n) = item.get(if google {"displayName"} else if messages {"display_name"} else {"name"}).and_then(|x| x.as_str()) {
                info.display_name = Some(crate::display::sanitize_for_terminal(
                    &crate::display::truncate_chars(n, 200),
                ));
            }
            if let Some(c) = item.get(if google {"inputTokenLimit"} else {"context_window"}).and_then(|x| x.as_u64()) {
                info.context_length = Some(c);
            }
            if let Some(o) = item.get(if google {"outputTokenLimit"} else {"max_output_tokens"}).and_then(|x| x.as_u64()) {
                info.max_output = Some(o);
            }
            out.push(info);
        }
        let has_more = if google {v.get("nextPageToken").is_some_and(|t|t!="" && !t.is_null())}
            else {v.get("has_more").and_then(Value::as_bool).unwrap_or(false)};
        if !has_more {
            return Ok(out);
        }
        // 分页必须能确认完整：空页无游标、游标不前进都按失败处理，不空转
        let cursor=if google {v.get("nextPageToken").and_then(Value::as_str)}
            else if messages {v.get("last_id").and_then(Value::as_str)}
            else {items.last().and_then(|i|i.get("id")).and_then(Value::as_str)};
        let Some(cursor) = cursor.filter(|s|!s.is_empty()) else {
            return Err(format!("分页异常：第 {page} 页声明还有更多数据但没有可用游标"));
        };
        if !cursors.insert(cursor.to_owned()) {
            return Err(format!("分页异常：游标未前进（{cursor}），中止读取"));
        }
        // 游标经查询参数接口整体编码后再拼接，服务端可控数据不进入 URL 结构
        url.set_query(None);
        url.query_pairs_mut().append_pair(if google {"pageToken"} else if messages {"after_id"} else {"after"}, cursor);
    }
    Err("分页超过 100 页仍未读完，中止".into())
}

/// 公共目录地址校验（F4）：一律要求 HTTPS，无任何环境开关例外。
/// 本机模拟测试通过 sync_public 的 url_override 测试注入参数实现，
/// 正式二进制不传入该参数，明文 HTTP（含本机回环）在正式路径必然被拒绝。
pub fn validate_public_catalog_url(url_str: &str) -> Result<(), String> {
    let u = Url::parse(url_str).map_err(|e| format!("公共目录地址无效：{e}"))?;
    match u.scheme() {
        "https" => Ok(()),
        other => Err(format!(
            "公共目录必须使用 HTTPS（当前 {other}:）：{url_str}"
        )),
    }
}

/// 模型标识合法性：非空、长度受限，不含控制字符与空白（注入面）；
/// 非 ASCII 可打印字符（如中文）允许，会在出站 URL 与显示层被编码/清洗。
pub fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= 200
        && id.chars().all(|c| !c.is_control() && !c.is_whitespace())
}

/// 从公共目录同步基础资料（例如 Models.dev，无需厂商 Key）。
/// 只更新目录里已有的厂商；同步前校验来源（F4），入库前验证字段（F4）；
/// 同步失败由调用方决定如何提示，旧目录保持不变。
///
/// `url_override` 仅供测试注入本机模拟服务地址（跳过 HTTPS 校验）；
/// 正式二进制一律传 None，地址来自环境覆盖或默认值并强制 HTTPS。
#[doc(hidden)]
pub fn sync_public(
    auto: &mut AutoCatalog,
    agent: &ureq::Agent,
    authorize_catalog: &mut dyn FnMut(&str) -> Result<(), String>,
    url_override: Option<&str>,
) -> Result<Vec<String>, String> {
    let url = match url_override {
        Some(u) => u.to_string(),
        None => std::env::var("TCHAT_PUBLIC_CATALOG_URL")
            .unwrap_or_else(|_| "https://models.dev/api.json".into()),
    };
    if url_override.is_none() {
        // 正式路径：一律强制 HTTPS（本机回环 http 也不例外）
        validate_public_catalog_url(&url)?;
    }
    let origin = crate::catalog::origin_of(&url)?;
    authorize_catalog(&origin)?;
    let text = send(agent.get(&url), None)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("公共目录解析失败：{e}"))?;
    let mut updated = Vec::new();
    for (id, entry) in auto.providers.iter_mut() {
        let Some(models) = v.get(id).and_then(|p| p.get("models")).and_then(|m| m.as_object())
        else {
            continue;
        };
        for (mid, mv) in models {
            // F4：非法标识拒绝入库（含控制字符/空白/超长的 id 不进本地目录）
            if !valid_model_id(mid) {
                continue;
            }
            let mut info = entry.base.models.get(mid).cloned().unwrap_or_default();
            if let Some(n) = mv.get("name").and_then(|x| x.as_str()) {
                // F4：展示文字清理危险控制字符并限长
                info.display_name = Some(crate::display::sanitize_for_terminal(
                    &crate::display::truncate_chars(n, 200),
                ));
            }
            if let Some(c) = mv.pointer("/limit/context").and_then(|x| x.as_u64()) {
                info.context_length = Some(c);
            }
            if let Some(o) = mv.pointer("/limit/output").and_then(|x| x.as_u64()) {
                info.max_output = Some(o);
            }
            info.id = mid.clone();
            entry.base.models.insert(mid.clone(), info);
        }
        entry.base.source = "public".into();
        entry.base.updated_at = now_secs();
        updated.push(id.clone());
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_catalog_url_rules() {
        // 正式路径：一律强制 HTTPS，本机回环 http 也不例外，无任何开关
        assert!(validate_public_catalog_url("https://models.dev/api.json").is_ok());
        assert!(validate_public_catalog_url("http://attacker.example.com/api.json").is_err());
        assert!(validate_public_catalog_url("http://127.0.0.1:1234/pub").is_err());
        assert!(validate_public_catalog_url("http://localhost:1/pub").is_err());
        // 旧的测试开关已移除：设置它也不影响正式路径判定
        std::env::set_var("TCHAT_PUBLIC_CATALOG_INSECURE_LOOPBACK", "1");
        assert!(validate_public_catalog_url("http://127.0.0.1:1234/pub").is_err());
        std::env::remove_var("TCHAT_PUBLIC_CATALOG_INSECURE_LOOPBACK");
        assert!(validate_public_catalog_url("ftp://x/pub").is_err());
        assert!(validate_public_catalog_url("not a url").is_err());
    }

    #[test]
    fn model_id_rules() {
        assert!(valid_model_id("deepseek-flash"));
        assert!(valid_model_id("openrouter/meta-llama/Llama-3.1"));
        assert!(valid_model_id("a&b#c模式"));
        assert!(!valid_model_id(""));
        assert!(!valid_model_id("bad id"));
        assert!(!valid_model_id("bad\r\nid"));
        assert!(!valid_model_id("\u{1b}[2Jevil"));
        assert!(!valid_model_id(&"x".repeat(201)));
    }

    #[test]
    fn limited_reader_allows_exact_limit_and_stops_over() {        use std::io::Read;
        // 恰好达到上限：完整读取成功
        let exact = vec![b'a'; MAX_BODY_BYTES as usize];
        let mut r = LimitedReader {
            inner: std::io::Cursor::new(exact),
            remaining: MAX_BODY_BYTES,
        };
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), MAX_BODY_BYTES as usize);
        // 超过上限 1 字节：立即报错（InvalidData）
        let over = vec![b'b'; MAX_BODY_BYTES as usize + 1];
        let mut r = LimitedReader {
            inner: std::io::Cursor::new(over),
            remaining: MAX_BODY_BYTES,
        };
        let mut out = Vec::new();
        let err = r.read_to_end(&mut out).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
