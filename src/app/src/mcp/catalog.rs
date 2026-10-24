//! 022 §2.2／2.3：工具目录处理——原始名去重、名单筛选、Schema 校验、
//! 凭证污染检查与 CC 函数定义生成。
//!
//! 顺序契约：先完成同服务器原始名字去重、定义校验、名单过滤，再生成模型名。
//! 原始名字重复属于服务目录错误，不能让后一条覆盖前一条。单项定义不受支持时
//! 排除该项并报告具体原因，其余有效项可用。
//! Schema 保持在普通 JSON Schema 的字段和约束：不开启 strict、不删除
//! required／enum／`$defs`；根 `type` 缺省或为 `"object"`，`properties` 缺省
//! ／null 按 Codex 补空对象；不新增完整 JSON Schema 执行验证器。

use super::secrets::SensitiveValues;
use rmcp::model::Tool;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// 名单筛选结果：保留项＋允许名单中的未知项提示。
pub struct FilterOutcome<'a> {
    pub kept: Vec<&'a Tool>,
    /// 允许名单里未出现在服务目录中的条目（状态提示用，不生成假工具）。
    pub unknown_allowlisted: Vec<String>,
}

/// 按原始工具名筛选：允许名单缺省＝全部，显式空数组＝零工具；禁用名单优先。
pub fn filter_tools<'a>(
    tools: &'a [Tool],
    enabled: Option<&HashSet<String>>,
    disabled: &HashSet<String>,
) -> FilterOutcome<'a> {
    let mut kept = Vec::new();
    for tool in tools {
        if disabled.contains(tool.name.as_ref()) {
            continue;
        }
        if let Some(allow) = enabled {
            if !allow.contains(tool.name.as_ref()) {
                continue;
            }
        }
        kept.push(tool);
    }
    let mut unknown_allowlisted = Vec::new();
    if let Some(allow) = enabled {
        for name in allow {
            if !tools.iter().any(|t| t.name.as_ref() == name.as_str()) {
                unknown_allowlisted.push(name.clone());
            }
        }
        unknown_allowlisted.sort();
    }
    FilterOutcome {
        kept,
        unknown_allowlisted,
    }
}

/// 校验并归一输入 Schema（022 §2.3）：
/// - 必须是 JSON 对象；
/// - 根 `type` 缺省或为 `"object"`，显式其他根类型拒绝；
/// - `properties` 必须是对象，缺省／null 补空对象；
/// - 其余字段原样保留（不迁就模型改写）。
/// 返回归一后的 Schema（新对象，不改写输入）。
pub fn validate_input_schema(schema: &Value) -> Result<Value, String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| "inputSchema 必须是 JSON 对象".to_string())?;
    match obj.get("type") {
        None => {}
        Some(Value::String(t)) if t == "object" => {}
        Some(other) => {
            return Err(format!(
                "inputSchema 根 type 必须缺省或为 object，遇到 {other}"
            ))
        }
    }
    match obj.get("properties") {
        None | Some(Value::Null) => {
            let mut normalized = obj.clone();
            normalized.insert("properties".into(), json!({}));
            Ok(Value::Object(normalized))
        }
        Some(Value::Object(_)) => Ok(Value::Object(obj.clone())),
        Some(other) => Err(format!(
            "inputSchema properties 必须是对象或缺省，遇到 {other}"
        )),
    }
}

/// 生成 Chat Completions 函数定义（022 §2.3：描述与 inputSchema 来自当前
/// 服务器，直接暴露参数对象）。两者均已通过凭证污染检查（调用方职责——
/// 检查先于发布）。description 缺省时省略字段（CC 允许可选描述）。
pub fn build_function_definition(
    model_name: &str,
    description: Option<&str>,
    schema: &Value,
) -> Value {
    let mut function = Map::new();
    function.insert("name".into(), json!(model_name));
    if let Some(description) = description {
        function.insert("description".into(), json!(description));
    }
    let params = schema.as_object().expect("Schema 已校验为对象").clone();
    function.insert("parameters".into(), Value::Object(params));
    json!({ "type": "function", "function": Value::Object(function) })
}

/// 汇总单服务器目录结果：原始名去重 → Schema 校验 → 污染检查 → 筛选。
/// 返回（保留的原始工具＋归一 Schema、被排除原因列表、未知允许名单项）。
/// 原始名重复返回 Err（服务目录错误，整服务器不可用）。
pub struct ServerCatalog {
    /// (原始工具名, Tool 引用, 归一 Schema)
    pub kept: Vec<(String, Tool, Value)>,
    /// 逐项排除原因（状态提示用）。
    pub excluded: Vec<String>,
    pub unknown_allowlisted: Vec<String>,
}

pub fn build_server_catalog(
    tools: &[Tool],
    enabled: Option<&HashSet<String>>,
    disabled: &HashSet<String>,
    sensitive: &SensitiveValues,
) -> Result<ServerCatalog, String> {
    let mut seen: HashSet<&str> = HashSet::new();
    for tool in tools {
        if !seen.insert(tool.name.as_ref()) {
            return Err(format!(
                "工具目录存在重复原始名：{}（服务目录错误）",
                tool.name
            ));
        }
    }
    let filtered = filter_tools(tools, enabled, disabled);
    let mut kept = Vec::new();
    let mut excluded = Vec::new();
    for tool in filtered.kept {
        // 凭证污染检查先于发布：污染定义不发布并报告通用原因。
        let description = tool.description.as_deref().unwrap_or("");
        if sensitive.contains_secret(description)
            || sensitive.contains_secret(
                &Value::Object(tool.input_schema.as_ref().clone()).to_string(),
            )
        {
            excluded.push(format!(
                "{}：工具定义包含敏感数据，未发布",
                tool.name
            ));
            continue;
        }
        match validate_input_schema(&Value::Object(tool.input_schema.as_ref().clone())) {
            Ok(schema) => kept.push((tool.name.to_string(), tool.clone(), schema)),
            Err(reason) => excluded.push(format!("{}：{}", tool.name, reason)),
        }
    }
    Ok(ServerCatalog {
        kept,
        excluded,
        unknown_allowlisted: filtered.unknown_allowlisted,
    })
}
