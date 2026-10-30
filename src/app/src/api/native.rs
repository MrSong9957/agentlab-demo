//! 原生模型编解码；共同历史与工具执行仍由宿主维护。
//! Responses Items 参考 Codex 970111f6 和官方无状态接口。
use super::*;
use crate::prompt::{HistoryItem, HistoryMessage, HistoryRole, NativeOwner, NativeState, ToolCallRecord};

mod anthropic;
mod gemini;

/// 原生回放前核对记录与载荷；opaque 内容不解密、不改写。
pub(super) fn check_saved(message: &HistoryItem, state: &NativeState) -> Result<(), String> {
    let (text, calls): (&str, &[ToolCallRecord]) = match message {
        HistoryItem::Text(m) if m.role == HistoryRole::Assistant => (&m.content, &[]),
        HistoryItem::AssistantToolCalls { content, tool_calls } =>
            (content.as_deref().unwrap_or(""), tool_calls),
        _ => return Err("原生 assistant 记录无效，不能续接".into()),
    };
    let owner=state.owner.clone();
    let payload=state.payload.clone();
    let decoded=match owner.protocol.as_str() {
        "openai-responses"=>decode_response(owner,payload),
        "anthropic-messages"=>anthropic::decode(owner,payload),
        "google-interactions"=>gemini::decode_interactions(owner,payload),
        "google-generate-content"=>gemini::decode_generate(owner,payload,&[],Some(calls)),
        _=>return Err("原生状态协议无效，不能续接".into()),
    }.map_err(|e|format!("原生状态无效，不能续接：{e}"))?;
    let decoded_calls=ToolCallRecord::parse_batch(&decoded["assistant"])?;
    if decoded["assistant"]["content"].as_str()!=Some(text) || decoded_calls.as_slice()!=calls {
        return Err("原生状态与共同 assistant 记录不一致，不能续接".into());
    }
    Ok(())
}

fn image_data(url: &str) -> Result<(&str, &str), String> {
    use base64::Engine;
    let (meta, data) = url
        .split_once(',')
        .ok_or("图片必须是准备后的本地 data URL")?;
    let mime = meta
        .strip_prefix("data:")
        .and_then(|s| s.strip_suffix(";base64"))
        .filter(|s| matches!(*s, "image/png" | "image/jpeg" | "image/gif" | "image/webp"))
        .ok_or("图片 MIME 或 Base64 格式不支持")?;
    if data.is_empty()
        || base64::engine::general_purpose::STANDARD
            .decode(data)
            .is_err()
    {
        return Err("图片 Base64 格式无效".into());
    }
    Ok((mime, data))
}

/// 只转换宿主已经准备的图片；这里不读取文件或访问远端 URL。
fn content_parts(protocol: &str, text: &str, images: &[&str]) -> Result<Vec<Value>, String> {
    let mut parts = vec![match protocol {
        "openai-responses" => json!({"type":"input_text","text":text}),
        "google-generate-content" => json!({"text":text}),
        _ => json!({"type":"text","text":text}),
    }];
    for url in images {
        let (mime, data) = image_data(url)?;
        parts.push(match protocol {
            "openai-responses"=>json!({"type":"input_image","image_url":url,"detail":"high"}),
            "anthropic-messages"=>json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}),
            "google-interactions"=>json!({"type":"image","mime_type":mime,"data":data,"resolution":"high"}),
            _=>json!({"inlineData":{"mimeType":mime,"data":data},"mediaResolution":{"level":"MEDIA_RESOLUTION_HIGH"}}),
        });
    }
    Ok(parts)
}

fn user_images(message: &HistoryMessage) -> Vec<&str> {
    message
        .images
        .as_ref()
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect()
}

pub(super) fn is_native(protocol: &str) -> bool {
    matches!(
        protocol,
        "openai-responses"
            | "anthropic-messages"
            | "google-interactions"
            | "google-generate-content"
    )
}

pub(super) struct Conversation {
    owner: NativeOwner,
    history: Vec<HistoryItem>,
    system: String,
}

pub(super) fn failed_result(content: &str) -> bool {
    serde_json::from_str::<Value>(content)
        .ok()
        .is_some_and(|v| {
            v["isError"] == true
                || v["ok"] == false
                || matches!(v["status"].as_str(), Some("error" | "cancelled"))
                || v.get("error").is_some_and(|e| !e.is_null())
        })
}

pub(super) fn apply_params(
    conn: &EffectiveConnection,
    model: &ModelInfo,
    body: &mut Value,
) -> Result<(), String> {
    for (name, value) in &conn.request_params {
        if matches!(
            name.as_str(),
            "model"
                | "messages"
                | "input"
                | "contents"
                | "instructions"
                | "system"
                | "systemInstruction"
                | "system_instruction"
                | "tools"
                | "store"
                | "previous_response_id"
                | "previous_interaction_id"
                | "conversation"
                | "agent"
                | "background"
                | "_history"
                | "_provider_id"
        ) {
            return Err(format!("request_params 不能包含保留字段 {name}"));
        }
        if name == "stream" && !value.is_boolean() {
            return Err("stream 必须是布尔值".into());
        }
        // stream 与其它可选参数走同一能力门（§1.2/§1.6）：
        // 已确认不支持或能力未知都零 HTTP 本地报错。
        if !(conn.protocol == "anthropic-messages" && name == "max_tokens") {
            check_request_param(model, name)?;
        }
        body[name] = value.clone();
    }
    Ok(())
}

pub(super) fn normalized(
    owner: NativeOwner,
    payload: Value,
    content: String,
    calls: Vec<Value>,
) -> Result<Value, String> {
    if content.is_empty() && calls.is_empty() {
        return Err("原生响应没有可见文字或函数调用".into());
    }
    if payload.get("error").is_some_and(|e|!e.is_null()) {
        return Err("原生响应包含服务端错误".into());
    }
    let assistant=json!({"content":content,"tool_calls":calls});
    validate_assistant(&assistant)?;
    let usage = payload
        .get("usage")
        .or_else(|| payload.get("usageMetadata"))
        .cloned();
    let finish=payload.get("stop_reason").or_else(||payload.get("status"))
        .or_else(||payload.pointer("/candidates/0/finishReason")).cloned();
    Ok(json!({"assistant":assistant,"finish_reason":finish,
        "usage":usage,"native_state":NativeState { owner, payload }}))
}

pub(super) fn owner(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
) -> NativeOwner {
    NativeOwner {
        provider_id: request
            .get("_provider_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        protocol: conn.protocol.clone(),
        model_id: model.id.clone(),
        base_url: conn.base_url.clone(),
        key_ref: conn.key_ref.clone(),
        credential_fingerprint: crate::catalog::fingerprint(key),
    }
}

/// 兼容旧公开入口；生产工具循环传共同历史，不由 CC 投影还原原生块。
pub(super) fn history(request: &Value) -> Result<Vec<HistoryItem>, String> {
    if let Some(items) = request.get("_history") {
        return serde_json::from_value(items.clone()).map_err(|_| "共同历史格式无效".into());
    }
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("模型请求缺少 messages 数组")?;
    messages
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|message| {
            let role = match message["role"].as_str() {
                Some("user") => HistoryRole::User,
                Some("assistant") => HistoryRole::Assistant,
                _ => return Err("旧请求入口需要共同历史来表达工具记录".into()),
            };
            Ok(HistoryItem::Text(HistoryMessage {
                role,
                content: message["content"]
                    .as_str()
                    .ok_or("旧请求消息内容必须是字符串")?
                    .into(),
                user_input_event: None,
                images: None,
                text_elements: Vec::new(),
            }))
        })
        .collect()
}

fn historical_text(role: HistoryRole, content: String, images: Option<Vec<String>>) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        role,
        content,
        images,
        user_input_event: None,
        text_elements: Vec::new(),
    })
}

/// 切换来源只带走已闭合的可见事实，不把另一模型的签名或调用伪装成本轮调用。
pub(super) fn for_owner(
    items: Vec<HistoryItem>,
    current: &NativeOwner,
) -> Result<Vec<HistoryItem>, String> {
    let mut output = Vec::new();
    let mut pending = std::collections::HashSet::new();
    for item in items {
        if !pending.is_empty() && !matches!(item, HistoryItem::ToolResult { .. }) {
            return Err("未闭合的原生调用链不能跨来源续接".into());
        }
        match item {
            HistoryItem::NativeAssistant { state: None, .. } => {
                return Err("原生状态损坏或缺失，不能续接".into())
            }
            HistoryItem::NativeAssistant {
                message,
                state: Some(state),
            } if state.owner != *current => match *message {
                HistoryItem::Text(m) if m.role == HistoryRole::Assistant => {
                    output.push(HistoryItem::Text(m))
                }
                HistoryItem::AssistantToolCalls {
                    tool_calls,
                    content,
                } => {
                    let mut text = String::from("[历史工具调用，来自另一模型来源]\n");
                    if let Some(content) = content {
                        text.push_str(&content);
                        text.push('\n');
                    }
                    for call in tool_calls {
                        if !pending.insert(call.id.clone()) {
                            return Err("历史调用 ID 重复，不能续接".into());
                        }
                        text.push_str(&format!(
                            "{}（{}）：{}\n",
                            call.name, call.id, call.arguments
                        ));
                    }
                    output.push(historical_text(HistoryRole::Assistant, text, None));
                }
                _ => return Err("原生 assistant 记录损坏，不能续接".into()),
            },
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                image,
                ..
            } if !pending.is_empty() => {
                if !pending.remove(&tool_call_id) {
                    return Err("历史工具结果与原生调用不配对，不能续接".into());
                }
                output.push(historical_text(
                    HistoryRole::User,
                    format!("[历史工具结果 {tool_call_id}]\n{content}"),
                    image.map(|i| vec![i.image_url]),
                ));
            }
            item => output.push(item),
        }
    }
    if !pending.is_empty() {
        return Err("未闭合的原生调用链不能跨来源续接".into());
    }
    Ok(output)
}

fn text_message(role: &str, content: &str) -> Value {
    json!({"role":role,"content":content})
}

/// §1.4：旧历史的工具块若无法按目标协议合法表达（如非法 JSON 参数对
/// 对象参数协议），整块转成明确标注的历史文本；不伪造调用块，不阻断会话。
/// Responses 的 arguments 本就是字符串，无需转换。
fn textualize_unexpressible(items: Vec<HistoryItem>, protocol: &str) -> Vec<HistoryItem> {
    if protocol == "openai-responses" {
        return items;
    }
    let expressible = |arguments: &str| {
        serde_json::from_str::<Value>(arguments)
            .map(|v| v.is_object())
            .unwrap_or(false)
    };
    let mut output = Vec::new();
    let mut pending = std::collections::HashSet::new();
    for item in items {
        match item {
            HistoryItem::AssistantToolCalls {
                tool_calls,
                content,
            } if tool_calls.iter().any(|c| !expressible(&c.arguments)) => {
                let mut text = String::from("[历史工具调用，无法按目标协议表达参数]\n");
                if let Some(content) = content {
                    text.push_str(&content);
                    text.push('\n');
                }
                for call in tool_calls {
                    pending.insert(call.id.clone());
                    text.push_str(&format!(
                        "{}（{}）：{}\n",
                        call.name, call.id, call.arguments
                    ));
                }
                output.push(historical_text(HistoryRole::Assistant, text, None));
            }
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                image,
                ..
            } if pending.contains(&tool_call_id) => {
                pending.remove(&tool_call_id);
                output.push(historical_text(
                    HistoryRole::User,
                    format!("[历史工具结果 {tool_call_id}]\n{content}"),
                    image.map(|i| vec![i.image_url]),
                ));
            }
            item => output.push(item),
        }
    }
    output
}

fn response_input(items: &[HistoryItem], current: &NativeOwner) -> Result<Vec<Value>, String> {
    let mut input = Vec::new();
    for item in items {
        match item {
            HistoryItem::NativeAssistant {
                state: Some(state), message,
            } if state.owner == *current => {
                check_saved(message,state)?;
                let output = state
                    .payload
                    .get("output")
                    .and_then(Value::as_array)
                    .ok_or("原生状态缺少 Responses output，不能续接")?;
                input.extend(output.iter().cloned());
            }
            HistoryItem::NativeAssistant { .. } => {
                return Err("原生状态缺失或来源已改变，不能续接此调用链".into());
            }
            // 032：检查点标记不进入原生输入——活动视图由调用方展开为 seed＋尾部；
            // 直达此处（误传完整历史）时防御性跳过，不把 seed 伪装成原生块。
            // 034：编排事件同样不进入原生输入（模型只看外层 exec/wait 汇总）。
            // 035：Agent 控制事件同过滤（根投影不含非根 child_record/控制事件）。
            HistoryItem::ContextCheckpoint { .. }
            | HistoryItem::OrchestrationEvent { .. }
            | HistoryItem::AgentEvent { .. } => {}
            HistoryItem::Text(m) => {
                let images = user_images(m);
                input.push(if images.is_empty() {text_message(m.role.as_str(),&m.content)} else {
                    json!({"role":m.role.as_str(),"content":content_parts("openai-responses",&m.content,&images)?})
                });
            }
            HistoryItem::AssistantToolCalls {
                tool_calls,
                content,
            } => {
                if let Some(content) = content {
                    input.push(text_message("assistant", content));
                }
                for call in tool_calls {
                    input.push(json!({"type":"function_call","call_id":call.id,
                        "name":call.name,"arguments":call.arguments}));
                }
            }
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                image,
                ..
            } => {
                let output = if let Some(image) = image {
                    json!(content_parts(
                        "openai-responses",
                        content,
                        &[&image.image_url]
                    )?)
                } else {
                    json!(content)
                };
                input.push(
                    json!({"type":"function_call_output","call_id":tool_call_id,"output":output}),
                );
            }
        }
    }
    Ok(input)
}

/// 032 M1：原生会话纯准备（零 HTTP）——来源过滤（for_owner）、不可表达参数
/// 文本化与 system 指令提取；不做图片能力门（计量对图片内容返回未知，见 032 §1.3）。
pub(super) fn conversation_for(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
) -> Result<Conversation, String> {
    let current = owner(conn, key, model, request);
    let items = textualize_unexpressible(for_owner(history(request)?, &current)?, &conn.protocol);
    let instructions = request
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].as_str().ok_or("系统指令必须是字符串"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Conversation {
        owner: current,
        history: items,
        system: instructions.join("\n\n"),
    })
}

fn history_has_images(items: &[HistoryItem]) -> bool {
    items.iter().any(|item| match item.portable() {
        HistoryItem::Text(m) => !user_images(m).is_empty(),
        HistoryItem::ToolResult { image, .. } => image.is_some(),
        _ => false,
    })
}

/// 032 M1：按协议的纯 body 组装（零 HTTP；发送与只读计量共用）。
fn native_body(
    conn: &EffectiveConnection,
    model: &ModelInfo,
    request: &Value,
    conversation: &Conversation,
) -> Result<Value, String> {
    match conn.protocol.as_str() {
        "anthropic-messages" => anthropic::build_body(conn, model, request, conversation),
        "google-interactions" => gemini::build_interactions_body(conn, model, request, conversation),
        "google-generate-content" => gemini::build_generate_body(conn, model, request, conversation),
        _ => build_responses_body(conn, model, request, conversation),
    }
}

/// 032 M1：Responses 纯 body 组装。
fn build_responses_body(
    conn: &EffectiveConnection,
    model: &ModelInfo,
    request: &Value,
    conversation: &Conversation,
) -> Result<Value, String> {
    let input = response_input(&conversation.history, &conversation.owner)?;
    let mut body = json!({"model":model.id,"input":input,"store":false});
    if !conversation.system.is_empty() {
        body["instructions"] = json!(conversation.system.clone());
    }
    if let Some(tools) = request.get("tools") {
        body["tools"] = Value::Array(
            tools
                .as_array()
                .ok_or("工具定义必须是数组")?
                .iter()
                .map(|tool| {
                    let mut definition = tool.get("function").cloned().ok_or("普通函数缺少定义")?;
                    let map = definition.as_object_mut().ok_or("函数定义必须是对象")?;
                    map.insert("type".into(), json!("function"));
                    // 既有工具含可选参数，不允许隐含 strict 改写这些语义。
                    map.entry("strict").or_insert(json!(false));
                    Ok(definition)
                })
                .collect::<Result<Vec<_>, String>>()?,
        );
    }
    apply_params(conn, model, &mut body)?;
    Ok(body)
}

pub(super) fn request(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
    agent: &ureq::Agent,
) -> Result<Value, String> {
    let conversation = conversation_for(conn, key, model, request)?;
    if history_has_images(&conversation.history) && !crate::catalog::supports_image_input(model) {
        return Err(format!(
            "当前模型 {} 未确认支持图片输入（input_modalities 缺少 image），已拒绝发送",
            model.id
        ));
    }
    let body = native_body(conn, model, request, &conversation)?;
    let (response, context) = post_chat_json(conn, key, body, agent)?;
    let owner = conversation.owner.clone();
    match conn.protocol.as_str() {
        "anthropic-messages" => anthropic::decode(owner, response).map_err(|e| format!("{e}{context}")),
        "google-interactions" => {
            gemini::decode_interactions(owner, response).map_err(|e| format!("{e}{context}"))
        }
        "google-generate-content" => gemini::decode_generate(
            owner,
            response,
            &conversation.history,
            None,
        )
        .map_err(|e| format!("{e}{context}")),
        _ => decode_response(owner, response).map_err(|e| format!("{e}{context}")),
    }
}

/// 032 M1：原生协议只读计量（零 HTTP）：与真实发送共用会话准备、纯 body 组装
/// 与 [`super::finalize_outbound`] 定型；计数终点＝最终发送 body。
pub(super) fn measure_outbound(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
) -> Result<u64, String> {
    let conversation = conversation_for(conn, key, model, request)?;
    let body = native_body(conn, model, request, &conversation)?;
    let outbound = super::finalize_outbound(conn, body)?;
    super::count_body_bytes(&outbound.body)
}

/// 032 审查修复 #9：原生协议的纯 body 组装出口（有界计量用——与发送共用
/// 会话准备与定型，计数由调用方的限计量写入器承担）。
pub(super) fn build_body_for_measure(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
) -> Result<Value, String> {
    let conversation = conversation_for(conn, key, model, request)?;
    native_body(conn, model, request, &conversation)
}

pub(super) fn decode_response(current: NativeOwner, response: Value) -> Result<Value, String> {
    if response.get("status").and_then(Value::as_str) != Some("completed") {
        return Err("Responses 响应未完成".into());
    }
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or("Responses 响应缺少 output 数组")?;
    let mut content = String::new();
    let mut calls = Vec::new();
    for item in output {
        match item["type"].as_str() {
            Some("reasoning") => {}
            Some("function_call") => {
                calls.push(
                    json!({"id":item.get("call_id"),"type":"function","function":{
                        "name":item.get("name"),"arguments":item.get("arguments")
                    }}),
                );
            }
            Some("message") => {
                let parts = item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or("Responses message 缺少 content 数组")?;
                for part in parts {
                    if part["type"] != "output_text" {
                        return Err("Responses 返回了不支持的消息内容".into());
                    }
                    content.push_str(
                        part.get("text")
                            .and_then(Value::as_str)
                            .ok_or("Responses output_text 缺少 text")?,
                    );
                }
            }
            _ => return Err("Responses 返回了不支持的输出 Item".into()),
        }
    }
    normalized(current, response, content, calls)
}

