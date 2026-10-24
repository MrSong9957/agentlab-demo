//! 022 §2.4：调用结果投影与标准客户端错误形态。
//!
//! 标准 `CallToolResult` 序列化为 `role=tool.content` 的 JSON 字符串：保留
//! content 顺序、`structuredContent`（含显式 null）、`isError` 和服务端
//! `_meta` 的数据语义，不只取第一段 text；缺省 `isError` 不擅自改成 false。
//! 服务端 `isError:true` 原样作为工具失败回传（模型可继续），HTTP／JSON-RPC
//! 成功不能误画为业务成功。
//! 凭证污染检查发生在投影之前：污染的调用结果整份替换为标准客户端错误，
//! 原始回包不入史、不展示、不写普通日志。

use rmcp::model::CallToolResult;
use serde_json::{json, Value};

/// 标准客户端错误（022 §2.4 表格形态）：
/// `{"content":[{"type":"text","text":"MCP 客户端错误（原因）：说明"}],"isError":true}`。
pub fn client_error_json(kind: &str, detail: &str) -> String {
    json!({
        "content": [{ "type": "text", "text": format!("MCP 客户端错误（{kind}）：{detail}") }],
        "isError": true
    })
    .to_string()
}

/// 把成功收到的 `CallToolResult` 投影为工具结果 JSON 字符串。
/// 投影经 serde 保留字段语义；序列化失败（理论上不可达）按协议错误处理。
pub fn project_result(result: &CallToolResult) -> String {
    match serde_json::to_value(result) {
        Ok(value) => value_to_tool_text(&value),
        Err(e) => client_error_json("协议", &format!("结果序列化失败：{e}")),
    }
}

/// 给定已序列化的结果 JSON，产出工具文本。
fn value_to_tool_text(value: &Value) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{CallToolResult, ContentBlock};

    #[test]
    fn client_error_shape_matches_spec() {
        let text = client_error_json("断连", "连接已关闭");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["isError"], json!(true));
        assert_eq!(v["content"][0]["type"], "text");
        assert!(
            v["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("MCP 客户端错误（断连）：")
        );
    }

    #[test]
    fn project_result_preserves_order_and_fields() {
        // SDK CallToolResult 为 non_exhaustive，逐字段赋值。
        let mut result = CallToolResult::default();
        result.content = vec![
            ContentBlock::text("第一段"),
            ContentBlock::text("第二段"),
        ];
        result.structured_content = Some(Value::Null);
        let v: Value = serde_json::from_str(&project_result(&result)).unwrap();
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "第一段");
        assert_eq!(v["content"][1]["text"], "第二段");
        // 显式 null 与缺省相区分：存在 structuredContent: null
        assert!(v.get("structuredContent").is_some_and(Value::is_null));
        // 缺省 isError 不擅自补 false
        assert!(v.get("isError").is_none());
    }

    #[test]
    fn project_result_keeps_is_error_true_and_meta() {
        let mut result = CallToolResult::default();
        result.content = vec![ContentBlock::text("boom")];
        result.is_error = Some(true);
        result.meta = Some(
            serde_json::from_value(json!({"server_key": "v"}))
                .expect("meta 对象"),
        );
        let v: Value = serde_json::from_str(&project_result(&result)).unwrap();
        assert_eq!(v["isError"], json!(true));
        assert_eq!(v["_meta"]["server_key"], "v");
    }
}
