//! Gemini 原生步骤／内容；不经 OpenAI 兼容入口。
use super::*;
use std::collections::HashMap;

/// 032 M1：generateContent 纯 body 组装（零 HTTP；发送与只读计量共用）。
pub(super) fn build_generate_body(
    conn: &EffectiveConnection,
    model: &ModelInfo,
    request: &Value,
    conversation: &Conversation,
) -> Result<Value, String> {
    let mut contents = Vec::new();
    let mut names = HashMap::<String, (String, Option<String>)>::new();
    for item in &conversation.history {
        match item {
            HistoryItem::NativeAssistant {
                state: Some(state),
                message,
            } if state.owner == conversation.owner => {
                check_saved(message,state)?;
                let content = state
                    .payload
                    .pointer("/candidates/0/content")
                    .ok_or("原生状态缺少 Gemini content，不能续接")?;
                let parts = content["parts"]
                    .as_array()
                    .ok_or("原生状态缺少 Gemini parts")?;
                let calls = match message.portable() {
                    HistoryItem::AssistantToolCalls { tool_calls, .. } => tool_calls.as_slice(),
                    _ => &[],
                };
                let functions: Vec<_> =
                    parts.iter().filter_map(|p| p.get("functionCall")).collect();
                if functions.len() != calls.len() {
                    return Err("Gemini 原生调用映射损坏".into());
                }
                for (function, call) in functions.iter().zip(calls) {
                    let name = function["name"]
                        .as_str()
                        .ok_or("原生 functionCall 缺少 name")?;
                    if name != call.name {
                        return Err("Gemini 原生调用映射不一致".into());
                    }
                    names.insert(
                        call.id.clone(),
                        (name.into(), function["id"].as_str().map(str::to_owned)),
                    );
                }
                contents.push(content.clone());
            }
            HistoryItem::NativeAssistant { .. } => {
                return Err("原生状态缺失或来源已改变，不能续接此调用链".into())
            }
            // 032：检查点标记不进入原生内容（防御性跳过，同 project_history）。
            // 035：Agent 控制事件同过滤。
            HistoryItem::ContextCheckpoint { .. }
            | HistoryItem::OrchestrationEvent { .. }
            | HistoryItem::AgentEvent { .. } => {}
            HistoryItem::Text(m) => contents.push(json!({
                "role":if m.role==HistoryRole::User {"user"} else {"model"},
                "parts":content_parts("google-generate-content",&m.content,&user_images(m))?
            })),
            HistoryItem::AssistantToolCalls {
                tool_calls,
                content,
            } => {
                let mut parts = Vec::new();
                if let Some(content) = content {
                    parts.push(json!({"text":content}));
                }
                for call in tool_calls {
                    let args: Value = serde_json::from_str(&call.arguments)
                        .map_err(|_| "历史参数不能表示为 Gemini args")?;
                    parts.push(json!({"functionCall":{"name":call.name,"args":args}}));
                    names.insert(call.id.clone(), (call.name.clone(), None));
                }
                contents.push(json!({"role":"model","parts":parts}));
            }
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                image,
                ..
            } => {
                let (name, id) = names
                    .get(tool_call_id)
                    .ok_or("Gemini 工具结果缺少调用映射")?;
                let response = serde_json::from_str::<Value>(content)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({"output":content}));
                let mut result = json!({"name":name,"response":response});
                if let Some(id) = id {
                    result["id"] = json!(id);
                }
                if let Some(image) = image {
                    let (mime, data) = image_data(&image.image_url)?;
                    result["parts"] = json!([{"inlineData":{"mimeType":mime,"data":data}}]);
                }
                contents.push(json!({"role":"user","parts":[{"functionResponse":result}]}));
            }
        }
    }
    let mut body = json!({"model":model.id,"contents":contents});
    if !conversation.system.is_empty() {
        body["systemInstruction"] = json!({"parts":[{"text":conversation.system}]});
    }
    if let Some(tools) = request.get("tools") {
        let definitions = tools
            .as_array()
            .ok_or("工具定义必须是数组")?
            .iter()
            .map(|tool| {
                let mut definition = tool.get("function").cloned().ok_or("普通函数缺少定义")?;
                let fields=definition.as_object_mut().ok_or("函数定义必须是对象")?;
                fields.remove("strict");
                // Rig 同源做法：parameters 是 OpenAPI Schema，完整 JSON Schema 用此字段。
                if let Some(schema)=fields.remove("parameters") {
                    fields.insert("parametersJsonSchema".into(),schema);
                }
                Ok(definition)
            })
            .collect::<Result<Vec<_>, String>>()?;
        body["tools"] = json!([{"functionDeclarations":definitions}]);
    }
    apply_params(conn, model, &mut body)?;
    Ok(body)
}

pub(super) fn decode_generate(current: NativeOwner, response: Value, history: &[HistoryItem], saved_calls: Option<&[ToolCallRecord]>) -> Result<Value, String> {
    let candidates = response["candidates"]
        .as_array()
        .ok_or("Gemini 响应缺少 candidates 数组")?;
    if candidates.len() != 1
        || candidates[0]["finishReason"] != "STOP"
        || candidates[0]["content"]["role"] != "model"
    {
        return Err("Gemini 响应未完成、被阻止或包含多个候选".into());
    }
    let parts = candidates[0]["content"]["parts"]
        .as_array()
        .ok_or("Gemini 响应缺少 parts 数组")?;
    let mut content = String::new();
    let mut calls = Vec::new();
    let mut used = std::collections::HashSet::new();
    for item in history {
        if let HistoryItem::AssistantToolCalls { tool_calls, .. } = item.portable() {
            used.extend(tool_calls.iter().map(|c| c.id.clone()));
        }
    }
    for (index, part) in parts.iter().enumerate() {
        if part.get("thoughtSignature").is_some_and(|s| !s.is_string()) {
            return Err("Gemini thoughtSignature 必须是字符串".into());
        }
        if let Some(function) = part.get("functionCall") {
            // 官方 FunctionCall.args 可省略（无参函数）：缺省按空对象参与，
            // 显式存在但非对象仍是协议错误。
            let args = match function.get("args").filter(|a| !a.is_null()) {
                None => json!({}),
                Some(a) if a.is_object() => a.clone(),
                Some(_) => return Err("Gemini functionCall args 必须是对象".into()),
            };
            let id = match function.get("id") {
                Some(id) => id
                    .as_str()
                    .ok_or("Gemini functionCall id 必须是字符串")?
                    .to_owned(),
                None if saved_calls.is_some() => saved_calls
                    .and_then(|saved| saved.get(calls.len()))
                    .map(|call| call.id.clone())
                    .ok_or("Gemini 原生调用映射缺失，不能续接")?,
                None => {
                    // 仅内部使用；不把此值写进厂商 functionResponse.id。
                    let mut id = format!("host-gemini-{}-{index}", history.len());
                    while used.contains(&id) {
                        id.push('_');
                    }
                    id
                }
            };
            used.insert(id.clone());
            calls.push(json!({"id":id,"type":"function","function":{
                "name":function.get("name"),"arguments":args.to_string()
            }}));
        } else if let Some(text) = part.get("text") {
            let text = text.as_str().ok_or("Gemini text 必须是字符串")?;
            if part["thought"] != true {
                content.push_str(text);
            }
        } else {
            return Err("Gemini 返回了不支持的输出 Part".into());
        }
    }
    normalized(
        current,
        response,
        content,
        calls,
    )
}

/// 032 M1：Interactions 纯 body 组装（零 HTTP；发送与只读计量共用）。
pub(super) fn build_interactions_body(
    conn: &EffectiveConnection,
    model: &ModelInfo,
    request: &Value,
    conversation: &Conversation,
) -> Result<Value, String> {
    let mut input = Vec::new();
    let mut names = HashMap::<String, String>::new();
    for item in &conversation.history {
        match item {
            HistoryItem::NativeAssistant {
                state: Some(state), message,
            } if state.owner == conversation.owner => {
                check_saved(message,state)?;
                let steps = state
                    .payload
                    .get("steps")
                    .and_then(Value::as_array)
                    .ok_or("原生状态缺少 Interactions steps，不能续接")?;
                for step in steps {
                    if step["type"] == "function_call" {
                        let id = step["id"].as_str().ok_or("原生 function_call 缺少 id")?;
                        let name = step["name"]
                            .as_str()
                            .ok_or("原生 function_call 缺少 name")?;
                        names.insert(id.into(), name.into());
                    }
                }
                input.extend(steps.iter().cloned());
            }
            HistoryItem::NativeAssistant { .. } => {
                return Err("原生状态缺失或来源已改变，不能续接此调用链".into())
            }
            // 032：检查点标记不进入原生步骤（防御性跳过，同 project_history）。
            // 035：Agent 控制事件同过滤。
            HistoryItem::ContextCheckpoint { .. }
            | HistoryItem::OrchestrationEvent { .. }
            | HistoryItem::AgentEvent { .. } => {}
            HistoryItem::Text(m) => input.push(json!({
                "type":if m.role==HistoryRole::User {"user_input"} else {"model_output"},
                "content":content_parts("google-interactions",&m.content,&user_images(m))?
            })),
            HistoryItem::AssistantToolCalls {
                tool_calls,
                content,
            } => {
                if let Some(content) = content {
                    input.push(
                        json!({"type":"model_output","content":[{"type":"text","text":content}]}),
                    );
                }
                for call in tool_calls {
                    let arguments: Value = serde_json::from_str(&call.arguments)
                        .map_err(|_| "历史参数不能表示为 Interactions arguments")?;
                    input.push(json!({"type":"function_call","id":call.id,"name":call.name,"arguments":arguments}));
                    names.insert(call.id.clone(), call.name.clone());
                }
            }
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                image,
                ..
            } => {
                let name = names
                    .get(tool_call_id)
                    .ok_or("历史工具结果缺少匹配的函数名")?;
                input.push(json!({"type":"function_result","call_id":tool_call_id,"name":name,
                    "result":content_parts("google-interactions",content,&image.as_ref().map(|i|vec![i.image_url.as_str()]).unwrap_or_default())?,
                    "is_error":failed_result(content)}));
            }
        }
    }
    let mut body = json!({"model":model.id,"input":input,"store":false});
    if !conversation.system.is_empty() {
        body["system_instruction"] = json!(conversation.system);
    }
    if let Some(tools) = request.get("tools") {
        body["tools"] = Value::Array(
            tools
                .as_array()
                .ok_or("工具定义必须是数组")?
                .iter()
                .map(|tool| {
                    let mut definition = tool.get("function").cloned().ok_or("普通函数缺少定义")?;
                    let fields = definition.as_object_mut().ok_or("函数定义必须是对象")?;
                    fields.remove("strict");
                    fields.insert("type".into(), json!("function"));
                    Ok(definition)
                })
                .collect::<Result<Vec<_>, String>>()?,
        );
    }
    apply_params(conn, model, &mut body)?;
    Ok(body)
}

pub(super) fn decode_interactions(current: NativeOwner, response: Value) -> Result<Value, String> {
    if !matches!(
        response["status"].as_str(),
        Some("completed" | "requires_action")
    ) {
        return Err("Interactions 响应未完成或失败".into());
    }
    let steps = response
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("Interactions 响应缺少 steps 数组")?;
    let mut content = String::new();
    let mut calls = Vec::new();
    for step in steps {
        match step["type"].as_str() {
            Some("thought") => {
                if step.get("signature").is_some_and(|v| !v.is_string()) {
                    return Err("Interactions thought signature 必须是字符串".into());
                }
            }
            Some("model_output") => {
                for part in step
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or("model_output 缺少 content 数组")?
                {
                    if part["type"] != "text" {
                        return Err("Interactions 返回了不支持的输出内容".into());
                    }
                    content.push_str(
                        part["text"]
                            .as_str()
                            .ok_or("Interactions text 缺少字符串 text")?,
                    );
                }
            }
            Some("function_call") => {
                if !step["arguments"].is_object() {
                    return Err("Interactions arguments 必须是对象".into());
                }
                calls.push(json!({"id":step.get("id"),"type":"function","function":{
                    "name":step.get("name"),"arguments":step["arguments"].to_string()
                }}));
            }
            _ => return Err("Interactions 返回了不支持的 Step".into()),
        }
    }
    if response["status"] == "requires_action" && calls.is_empty() {
        return Err("Interactions 请求了不支持的后续动作".into());
    }
    normalized(
        current,
        response,
        content,
        calls,
    )
}

