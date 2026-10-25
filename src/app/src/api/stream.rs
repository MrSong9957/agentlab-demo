//! 024：复用 sse-stream 解析 SSE 语法；各协议只负责完整响应的组装。
use super::*;
use futures::StreamExt;

pub(super) fn decode(protocol: &str, text: &str) -> Result<Value, String> {
    let (events, done) = futures::executor::block_on(async {
        let bytes = bytes::Bytes::copy_from_slice(text.as_bytes());
        let input = futures::stream::once(async { Ok::<_, sse_stream::Error>(bytes) });
        let mut stream = Box::pin(sse_stream::SseStream::from_bytes_stream(input));
        let mut events = Vec::new();
        let mut done = false;
        while let Some(event) = stream.next().await {
            let event = event.map_err(|e| format!("SSE 解析失败：{e}"))?;
            let Some(data) = event.data.filter(|s| !s.is_empty()) else {
                continue;
            };
            if done {
                return Err("SSE 完成标记之后仍有数据".to_owned());
            }
            if data == "[DONE]" {
                done = true;
                continue;
            }
            let value: Value =
                serde_json::from_str(&data).map_err(|e| format!("SSE JSON 无效：{e}"))?;
            if !value.is_object() {
                return Err("SSE 事件必须是对象".into());
            }
            if let Some(event) = event.event.as_deref().filter(|e| *e != "message") {
                let kind = if protocol == "google-interactions" {
                    "event_type"
                } else {
                    "type"
                };
                if value.get(kind).and_then(Value::as_str) != Some(event) {
                    return Err("SSE 事件名与数据类型不一致".into());
                }
            }
            events.push(value);
        }
        Ok::<_, String>((events, done))
    })?;
    let output = match protocol {
        "openai-chat" => chat(events, done),
        "openai-responses" => responses(events),
        "anthropic-messages" => messages(events),
        "google-interactions" => interactions(events),
        "google-generate-content" => generate(events),
        _ => Err("未知 SSE 协议".into()),
    }?;
    let mut counter = LimitCountingWriter {
        written: 0,
        limit: MAX_BODY_BYTES,
    };
    serde_json::to_writer(&mut counter, &output).map_err(|_| "SSE 组装结果超过 10 MiB 上限")?;
    Ok(output)
}

fn append(target: &mut Value, field: &str, value: &Value) -> Result<(), String> {
    let fragment = value.as_str().ok_or("SSE 增量必须是字符串")?;
    if target.get(field).is_none_or(Value::is_null) {
        target[field] = json!("");
    }
    let Value::String(text) = &mut target[field] else {
        return Err("SSE 目标字段不是字符串".into());
    };
    text.push_str(fragment);
    Ok(())
}

fn chat(events: Vec<Value>, done: bool) -> Result<Value, String> {
    let mut message = json!({"role":"assistant","content":""});
    let mut calls = std::collections::BTreeMap::new();
    let mut finish = None;
    let mut usage = Value::Null;
    for event in events {
        if let Some(value) = event.get("usage") {
            usage = value.clone();
        }
        for choice in event["choices"].as_array().ok_or("SSE 缺少 choices 数组")? {
            if choice["index"] != 0 || finish.is_some() {
                return Err("SSE 候选索引或结束顺序无效".into());
            }
            if let Some(text) = choice["delta"].get("content").filter(|t| !t.is_null()) {
                append(&mut message, "content", text)?;
            }
            if let Some(fragments) = choice["delta"].get("tool_calls") {
                for fragment in fragments.as_array().ok_or("tool_calls delta 必须是数组")? {
                    let index = fragment["index"]
                        .as_u64()
                        .filter(|i| *i < 32)
                        .ok_or("工具调用索引无效或超过 32 项")?;
                    let call = calls.entry(index).or_insert_with(
                        || json!({"id":"","type":"function","function":{"name":"","arguments":""}}),
                    );
                    if let Some(kind) = fragment.get("type") {
                        if kind != "function" {
                            return Err("不支持的流式工具类型".into());
                        }
                    }
                    if let Some(id) = fragment.get("id") {
                        append(call, "id", id)?;
                    }
                    if let Some(function) = fragment.get("function") {
                        if !function.is_object() {
                            return Err("function delta 必须是对象".into());
                        }
                        for field in ["name", "arguments"] {
                            if let Some(part) = function.get(field) {
                                append(&mut call["function"], field, part)?;
                            }
                        }
                    }
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                finish = Some(reason.to_owned());
            }
        }
    }
    if !done || !matches!(finish.as_deref(), Some("stop" | "tool_calls")) {
        return Err("SSE 缺少合法完成事件".into());
    }
    if !calls.is_empty() {
        if calls.keys().copied().ne(0..calls.len() as u64) {
            return Err("工具调用索引不连续".into());
        }
        message["tool_calls"] = json!(calls.into_values().collect::<Vec<_>>());
    }
    Ok(json!({"choices":[{"message":message,"finish_reason":finish}],"usage":usage}))
}

fn responses(events: Vec<Value>) -> Result<Value, String> {
    let mut result = None;
    for event in events {
        if result.is_some() {
            return Err("Responses 完成之后仍有事件".into());
        }
        match event["type"].as_str() {
            Some("response.completed") => {
                result = Some(
                    event
                        .get("response")
                        .cloned()
                        .ok_or("完成事件缺少 response")?,
                )
            }
            Some("error" | "response.failed" | "response.incomplete") => {
                return Err("Responses SSE 失败或不完整".into())
            }
            Some(kind) if kind.starts_with("response.") => {}
            _ => return Err("Responses SSE 事件类型无效".into()),
        }
    }
    result.ok_or("Responses SSE 缺少完成事件".into())
}

fn messages(events: Vec<Value>) -> Result<Value, String> {
    let mut message = Value::Null;
    let mut open = std::collections::HashSet::new();
    let mut stopped = false;
    let mut arguments = std::collections::HashMap::<u64, String>::new();
    for event in events {
        if event["type"] == "ping" {
            continue;
        }
        if stopped {
            return Err("Messages 完成之后仍有事件".into());
        }
        match event["type"].as_str() {
            Some("message_start") if message.is_null() => {
                message = event["message"].clone();
                if !message["content"].is_array() {
                    return Err("message_start 缺少 content".into());
                }
            }
            Some("content_block_start") => {
                let blocks = message["content"]
                    .as_array_mut()
                    .ok_or("尚未收到 message_start")?;
                let index = event["index"].as_u64().ok_or("内容索引无效")?;
                if index != blocks.len() as u64 {
                    return Err("内容块索引不连续或重复".into());
                }
                blocks.push(event["content_block"].clone());
                open.insert(index);
            }
            Some("content_block_delta" | "content_block_stop") => {
                let index = event["index"]
                    .as_u64()
                    .filter(|i| open.contains(i))
                    .ok_or("内容块未开始或已经结束")?;
                let block = &mut message["content"][index as usize];
                if event["type"] == "content_block_stop" {
                    if let Some(json) = arguments.remove(&index) {
                        block["input"] =
                            serde_json::from_str(&json).map_err(|_| "流式工具参数 JSON 不完整")?;
                    }
                    open.remove(&index);
                } else {
                    let delta = &event["delta"];
                    match (block["type"].as_str(), delta["type"].as_str()) {
                        (Some("text"), Some("text_delta")) => {
                            append(block, "text", &delta["text"])?
                        }
                        (Some("thinking"), Some("thinking_delta")) => {
                            append(block, "thinking", &delta["thinking"])?
                        }
                        (Some("thinking"), Some("signature_delta")) => {
                            append(block, "signature", &delta["signature"])?
                        }
                        (Some("tool_use"), Some("input_json_delta")) => {
                            arguments.entry(index).or_default().push_str(
                                delta["partial_json"]
                                    .as_str()
                                    .ok_or("partial_json 必须是字符串")?,
                            )
                        }
                        _ => return Err("Messages SSE 增量类型不支持或与内容块不匹配".into()),
                    }
                }
            }
            Some("message_delta") if open.is_empty() && !message.is_null() => {
                let delta = event["delta"]
                    .as_object()
                    .ok_or("message_delta 缺少 delta")?;
                for (key, value) in delta {
                    message[key] = value.clone();
                }
                if let Some(usage) = event["usage"].as_object() {
                    if !message["usage"].is_object() {
                        message["usage"] = json!({});
                    }
                    for (key, value) in usage {
                        message["usage"][key] = value.clone();
                    }
                }
            }
            Some("message_stop")
                if !message.is_null() && open.is_empty() && message["stop_reason"].is_string() =>
            {
                stopped = true
            }
            _ => return Err("Messages SSE 顺序无效或流失败".into()),
        }
    }
    if !stopped {
        return Err("Messages SSE 缺少完成事件".into());
    }
    Ok(message)
}

fn interactions(events: Vec<Value>) -> Result<Value, String> {
    let mut interaction = Value::Null;
    let mut steps = Vec::new();
    let mut arguments = std::collections::HashMap::<u64, String>::new();
    let mut open = std::collections::HashSet::new();
    let mut stopped = false;
    for event in events {
        if stopped {
            return Err("Interactions 完成之后仍有事件".into());
        }
        match event["event_type"].as_str() {
            Some("interaction.created") if interaction.is_null() => {
                // 外部 SSE 的 interaction 必须先验型再保存：后续 status／usage／steps
                // 都按对象字段写入，字符串等其它类型会在索引赋值时 panic。
                let created = event["interaction"].clone();
                if !created.is_object() {
                    return Err("Interactions interaction.created 的 interaction 必须是对象".into());
                }
                interaction = created
            }
            Some("step.start") if !interaction.is_null() => {
                let index = event["index"].as_u64().ok_or("Step 索引无效")?;
                if index != steps.len() as u64 {
                    return Err("Step 索引重复或不连续".into());
                }
                let mut step = event["step"].clone();
                if step["type"] == "model_output" && step.get("content").is_none() {
                    step["content"] = json!([]);
                }
                steps.push(step);
                open.insert(index);
            }
            Some("step.delta" | "step.stop") => {
                let index = event["index"]
                    .as_u64()
                    .filter(|i| open.contains(i))
                    .ok_or("Step 未开始或已经结束")?;
                let step = &mut steps[index as usize];
                if event["event_type"] == "step.stop" {
                    if let Some(json) = arguments.remove(&index) {
                        step["arguments"] = serde_json::from_str(&json)
                            .map_err(|_| "流式 arguments JSON 不完整")?;
                    }
                    open.remove(&index);
                } else {
                    let delta = &event["delta"];
                    match (step["type"].as_str(), delta["type"].as_str()) {
                        (Some("model_output"), Some("text")) => {
                            let text = delta["text"].as_str().ok_or("text delta 必须是字符串")?;
                            step["content"]
                                .as_array_mut()
                                .ok_or("Step content 无效")?
                                .push(json!({"type":"text","text":text}));
                        }
                        (Some("thought"), Some("thought_signature")) => {
                            append(step, "signature", &delta["signature"])?
                        }
                        (Some("thought"), Some("thought_summary")) => {
                            if !delta["content"].is_object() {
                                return Err("thought_summary 缺少 content".into());
                            }
                            if step.get("summary").is_none() {
                                step["summary"] = json!([]);
                            }
                            step["summary"]
                                .as_array_mut()
                                .ok_or("thought summary 无效")?
                                .push(delta["content"].clone());
                        }
                        (Some("function_call"), Some("arguments_delta")) => {
                            arguments.entry(index).or_default().push_str(
                                delta["arguments"]
                                    .as_str()
                                    .ok_or("arguments delta 必须是字符串")?,
                            )
                        }
                        _ => return Err("Interactions SSE 增量类型不支持或与 Step 不匹配".into()),
                    }
                }
                if let Some(usage) = event.get("usage") {
                    interaction["usage"] = usage.clone();
                }
            }
            Some("interaction.status_update") if !interaction.is_null() => {
                interaction["status"] = event["status"].clone();
            }
            Some("interaction.completed") if !interaction.is_null() && open.is_empty() => {
                for (key, value) in event["interaction"]
                    .as_object()
                    .ok_or("完成事件缺少 interaction")?
                {
                    interaction[key] = value.clone();
                }
                stopped = true;
            }
            _ => return Err("Interactions SSE 顺序无效或流失败".into()),
        }
    }
    if !stopped {
        return Err("Interactions SSE 缺少完成事件".into());
    }
    interaction["steps"] = json!(steps);
    Ok(interaction)
}

fn generate(events: Vec<Value>) -> Result<Value, String> {
    let mut output = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[]}}]});
    let mut finished = false;
    for event in events {
        if let Some(candidates) = event.get("candidates").and_then(Value::as_array) {
            if candidates.len() != 1 || finished {
                return Err("Gemini SSE 候选数或结束顺序无效".into());
            }
            let candidate = &candidates[0];
            if candidate.get("index").is_some_and(|i| i != 0) {
                return Err("Gemini SSE 候选索引无效".into());
            }
            if let Some(parts) = candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
            {
                output["candidates"][0]["content"]["parts"]
                    .as_array_mut()
                    .expect("初始化数组")
                    .extend(parts.iter().cloned());
            }
            if let Some(reason) = candidate.get("finishReason") {
                output["candidates"][0]["finishReason"] = reason.clone();
                finished = true;
            }
        }
        if let Some(usage) = event.get("usageMetadata") {
            output["usageMetadata"] = usage.clone();
        }
        if event.get("error").is_some() {
            return Err("Gemini SSE 服务返回错误".into());
        }
    }
    if !finished {
        return Err("Gemini SSE 缺少完成事件".into());
    }
    Ok(output)
}
