//! Messages 的原生内容块；opaque thinking／签名不经文字投影。
use super::*;

fn push_message(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if let Some(last) = messages.last_mut().filter(|m| m["role"] == role) {
        last["content"]
            .as_array_mut()
            .expect("内容块数组")
            .extend(blocks);
    } else {
        messages.push(json!({"role":role,"content":blocks}));
    }
}

pub(super) fn request(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    request: &Value,
    agent: &ureq::Agent,
    conversation: &Conversation,
) -> Result<Value, String> {
    let mut messages = Vec::new();
    for item in &conversation.history {
        match item {
            HistoryItem::NativeAssistant {
                state: Some(state), message,
            } if state.owner == conversation.owner => {
                check_saved(message,state)?;
                let blocks = state
                    .payload
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or("原生状态缺少 Messages content，不能续接")?;
                push_message(&mut messages, "assistant", blocks.clone());
            }
            HistoryItem::NativeAssistant { .. } => {
                return Err("原生状态缺失或来源已改变，不能续接此调用链".into())
            }
            HistoryItem::Text(m) => push_message(
                &mut messages,
                m.role.as_str(),
                content_parts("anthropic-messages", &m.content, &user_images(m))?,
            ),
            HistoryItem::AssistantToolCalls {
                tool_calls,
                content,
            } => {
                let mut blocks = Vec::new();
                if let Some(content) = content {
                    blocks.push(json!({"type":"text","text":content}));
                }
                for call in tool_calls {
                    let input: Value = serde_json::from_str(&call.arguments)
                        .map_err(|_| "历史工具参数不能表示为 Messages input")?;
                    blocks.push(
                        json!({"type":"tool_use","id":call.id,"name":call.name,"input":input}),
                    );
                }
                push_message(&mut messages, "assistant", blocks);
            }
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                image,
                ..
            } => push_message(
                &mut messages,
                "user",
                vec![json!({
                    "type":"tool_result","tool_use_id":tool_call_id,
                    "content":content_parts("anthropic-messages",content,&image.as_ref().map(|i|vec![i.image_url.as_str()]).unwrap_or_default())?,"is_error":failed_result(content)
                })],
            ),
        }
    }
    let mut body = json!({"model":model.id,"messages":messages});
    if !conversation.system.is_empty() {
        body["system"] = json!(conversation.system);
    }
    if let Some(limit) = model.max_output {
        body["max_tokens"] = json!(limit);
    }
    if let Some(tools) = request.get("tools") {
        body["tools"] = Value::Array(
            tools
                .as_array()
                .ok_or("工具定义必须是数组")?
                .iter()
                .map(|tool| {
                    let definition = tool.get("function").ok_or("普通函数缺少定义")?;
                    Ok(
                        json!({"name":definition["name"],"description":definition["description"],
                "input_schema":definition["parameters"]}),
                    )
                })
                .collect::<Result<Vec<_>, String>>()?,
        );
    }
    apply_params(conn, model, &mut body)?;
    if !body.get("max_tokens").is_some_and(Value::is_u64) {
        return Err("Messages 必须配置 max_tokens 或已知的模型 max_output".into());
    }
    let (response, context) = post_chat_json(conn, key, body, agent)?;
    decode(conversation.owner.clone(), response).map_err(|e| format!("{e}{context}"))
}

pub(super) fn decode(current: NativeOwner, response: Value) -> Result<Value, String> {
    if response["role"] != "assistant" {
        return Err("Messages 响应缺少 assistant 角色".into());
    }
    if !matches!(
        response["stop_reason"].as_str(),
        Some("end_turn" | "tool_use" | "stop_sequence")
    ) {
        return Err("Messages 响应未完成或结束原因不受支持".into());
    }
    let blocks = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or("Messages 响应缺少 content 数组")?;
    let mut content = String::new();
    let mut calls = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => content.push_str(
                block["text"]
                    .as_str()
                    .ok_or("Messages text 缺少字符串 text")?,
            ),
            Some("thinking") => {
                if !block["thinking"].is_string() || !block["signature"].is_string() {
                    return Err("Messages thinking／signature 格式无效".into());
                }
            }
            Some("redacted_thinking") => {
                if !block["data"].is_string() {
                    return Err("Messages redacted_thinking 缺少 data".into());
                }
            }
            Some("tool_use") => {
                if !block["input"].is_object() {
                    return Err("Messages tool_use.input 必须是对象".into());
                }
                calls.push(json!({"id":block.get("id"),"type":"function","function":{
                    "name":block.get("name"),"arguments":block["input"].to_string()
                }}));
            }
            _ => return Err("Messages 返回了不支持的内容块".into()),
        }
    }
    normalized(
        current,
        response,
        content,
        calls,
    )
}

