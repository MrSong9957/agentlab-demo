//! 023 §3.3／§3.4／§5.1：MCP 资源面——固定工具定义、参数校验、定向列举
//! 执行、结果投影与容量／污染检查。
//!
//! 三个固定模型工具按资源能力条件声明（声明与执行一致：未实现的接口不
//! 声明）。8 MiB 工具结果 JSON 预算统一覆盖三个工具的定向与汇总路径
//! （R2=B）；游标是 opaque 字符串，空字符串原样传回，不 trim、不归一化。
//! 结果在入史前经凭证污染检查，污染整份替换为标准客户端错误。

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

use super::call::client_error_json;
use super::connection::{CallFailure, McpConnection, MAX_CATALOG_ITEMS, MAX_CATALOG_PAGES, MAX_CURSOR_BYTES};
use super::secrets::SensitiveValues;

/// 定向／汇总资源列举的固定模型工具名（023 §1）。
pub const LIST_TOOL_NAME: &str = "list_mcp_resources";
/// 定向／汇总模板列举的固定模型工具名。
pub const TEMPLATES_TOOL_NAME: &str = "list_mcp_resource_templates";
/// 资源读取的固定模型工具名。
pub const READ_TOOL_NAME: &str = "read_mcp_resource";

/// 8 MiB 工具结果 JSON 预算（§5.1.1，R2=B：三工具统一，定向与汇总路径）。
pub const MAX_RESULT_JSON_BYTES: usize = 8 * 1024 * 1024;

/// 单页项数上限（§5.1.1：定向单页超过 2048 项判容量超限失败，不截断）。
pub const MAX_PAGE_ITEMS: usize = 2_048;

/// 快照内单个具备资源能力的服务器（声明与执行共用；M1 起）。
pub struct ResourceServerEntry {
    pub server_id: String,
    pub connection: Arc<McpConnection>,
    pub sensitive: Arc<SensitiveValues>,
    pub tool_timeout: Duration,
}

/// `list_mcp_resources` 的 CC 函数定义（参数 schema 为最终形态：`server`
/// 可选＝汇总、`cursor` 可选且仅与 `server` 同用）。
pub fn list_tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": LIST_TOOL_NAME,
            "description": "List resources exposed by MCP resource servers. Pass `server` to list one page from that server (the result may carry nextCursor); omit `server` to aggregate every server with resource capability. `cursor` must be passed back unchanged and requires `server`.",
            "parameters": {
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "目标服务器 ID；省略表示跨服务器汇总列举"
                    },
                    "cursor": {
                        "type": "string",
                        "description": "上一页返回的 nextCursor；必须与 server 同时使用并原样传回"
                    }
                }
            }
        }
    })
}

/// 固定资源工具名集合（分派入口用；声明与执行一致——仅含已实现接口）。
pub fn is_resource_tool(name: &str) -> bool {
    name == LIST_TOOL_NAME || name == TEMPLATES_TOOL_NAME || name == READ_TOOL_NAME
}

/// `read_mcp_resource` 的 CC 函数定义。
pub fn read_tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": READ_TOOL_NAME,
            "description": "Read the contents of one resource from an MCP resource server. The URI is interpreted by the target server only; URIs from templates are passed through unchanged.",
            "parameters": {
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "目标服务器 ID（必填）"
                    },
                    "uri": {
                        "type": "string",
                        "description": "要读取的资源 URI（必填；原样送达目标服务器）"
                    }
                },
                "required": ["server", "uri"]
            }
        }
    })
}

/// `list_mcp_resource_templates` 的 CC 函数定义（参数语义与列举一致）。
pub fn templates_tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": TEMPLATES_TOOL_NAME,
            "description": "List resource templates (parameterized URI patterns) exposed by MCP resource servers. Pass `server` for one page from that server; omit `server` to aggregate every server with resource capability. `cursor` must be passed back unchanged and requires `server`.",
            "parameters": {
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "目标服务器 ID；省略表示跨服务器汇总列举"
                    },
                    "cursor": {
                        "type": "string",
                        "description": "上一页返回的 nextCursor；必须与 server 同时使用并原样传回"
                    }
                }
            }
        }
    })
}

/// 解析资源工具参数：`server`／`cursor` 为字符串或缺失；其他类型是参数
/// 错误（网络发送前返回）。`cursor` 仅与 `server` 同时使用（§5.1.3）。
fn parse_list_args(args: &Value) -> Result<(Option<String>, Option<String>), String> {
    let server = match args.get("server") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("`server` 必须是字符串".into()),
    };
    let cursor = match args.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("`cursor` 必须是字符串".into()),
    };
    if cursor.is_some() && server.is_none() {
        return Err("`cursor` 必须与 `server` 同时使用".into());
    }
    Ok((server, cursor))
}

/// 在快照资源服务器中按 ID 精确查找（未命中＝未知或停用，零外部请求）。
pub fn find_server<'a>(
    servers: &'a [ResourceServerEntry],
    server_id: &str,
) -> Option<&'a ResourceServerEntry> {
    servers.iter().find(|s| s.server_id == server_id)
}

/// 投影定向单页结果：服务器归属＋SDK 序列化的资源页（camelCase，含
/// `resources` 与可选 `nextCursor`；`nextCursor` 缺省时不伪造空值）。
fn project_directed_page(server_id: &str, result: &rmcp::model::ListResourcesResult) -> Value {
    let mut value = match serde_json::to_value(result) {
        Ok(v) => v,
        Err(e) => {
            return json!({"error": format!("资源页序列化失败：{e}")});
        }
    };
    if let Some(map) = value.as_object_mut() {
        map.insert("server".into(), json!(server_id));
    }
    value
}

/// 序列化结果并执行 8 MiB 预算检查（R2=B）：超限返回整次失败错误文本，
/// 不截断、不发布已收集数据。
fn serialize_within_budget(value: &Value) -> Result<String, String> {
    let text = value.to_string();
    if text.len() > MAX_RESULT_JSON_BYTES {
        Err(format!(
            "工具结果 JSON 超过 {} 字节预算（实际 {} 字节），整次失败",
            MAX_RESULT_JSON_BYTES,
            text.len()
        ))
    } else {
        Ok(text)
    }
}

/// 递归检查 JSON 值的全部字符串叶子（023 §5.1：逐字段检查原文，不能仅
/// 扫序列化字符串——敏感值含引号／反斜杠时转义会掩盖原文包含关系）。
fn value_polluted(value: &Value, sensitive: &SensitiveValues) -> bool {
    match value {
        Value::String(s) => sensitive.contains_secret(s),
        Value::Array(items) => items.iter().any(|v| value_polluted(v, sensitive)),
        Value::Object(map) => map.values().any(|v| value_polluted(v, sensitive)),
        _ => false,
    }
}

/// 执行一次资源工具调用（管理器同步桥接后进入；M1 定向列举、M2 定向
/// 模板列举与资源读取）。返回 CC 工具结果文本与 isError。
pub fn execute(
    servers: &[ResourceServerEntry],
    runtime: &tokio::runtime::Runtime,
    tool_name: &str,
    arguments: &Value,
    cancel: &AtomicBool,
) -> (String, bool) {
    if tool_name == READ_TOOL_NAME {
        return execute_read(servers, runtime, arguments, cancel);
    }
    let (server, cursor) = match parse_list_args(arguments) {
        Ok(parsed) => parsed,
        Err(detail) => return (client_error_json("参数", &detail), true),
    };
    let Some(server_id) = server else {
        // 023 §3.3／§5.1.2：省略 server＝跨服务器汇总（R1=A：单一整体
        // 时限；R3=A：台级失败是数据）。
        let templates = tool_name == TEMPLATES_TOOL_NAME;
        return execute_aggregate(servers, runtime, templates, cancel);
    };
    let Some(entry) = find_server(servers, &server_id) else {
        return (
            client_error_json("参数", &format!("未知或未启用的资源服务器：{server_id}")),
            true,
        );
    };
    let result = if tool_name == TEMPLATES_TOOL_NAME {
        runtime
            .block_on(entry.connection.list_resource_templates_once(
                cursor,
                entry.tool_timeout,
                cancel,
            ))
            .map(|page| (page.resource_templates.len(), project_templates_page(&server_id, &page)))
    } else {
        runtime
            .block_on(
                entry
                    .connection
                    .list_resources_once(cursor, entry.tool_timeout, cancel),
            )
            .map(|page| (page.resources.len(), project_directed_page(&server_id, &page)))
    };
    match result {
        Ok((item_count, projected)) => {
            if item_count > MAX_PAGE_ITEMS {
                return (
                    client_error_json(
                        "容量",
                        &format!(
                            "单页资源 {} 项超过上限 {MAX_PAGE_ITEMS} 项，整次失败",
                            item_count
                        ),
                    ),
                    true,
                );
            }
            finish_success(entry, projected)
        }
        Err(failure) => (failure_to_client_error(&failure, &entry.sensitive), true),
    }
}

/// 成功路径收尾：预算检查（R2=B）→ 凭证污染检查（投影的字符串叶子
/// 逐字段原文，不漏转义值）→ 结果文本。
fn finish_success(entry: &ResourceServerEntry, projected: Value) -> (String, bool) {
    let text = match serialize_within_budget(&projected) {
        Ok(t) => t,
        Err(detail) => return (client_error_json("容量", &detail), true),
    };
    if value_polluted(&projected, &entry.sensitive) {
        (
            client_error_json(
                "敏感数据",
                "资源结果包含凭证数据，已拒绝（已发调用的远端执行状态未知）",
            ),
            true,
        )
    } else {
        (text, false)
    }
}

/// 执行一次资源读取（M2）：`server`＋`uri` 均必填字符串；URI 原样交指定
/// 服务器解释。结果投影保留 `contents` 顺序、文本／blob、mimeType 与
/// 服务端 `_meta`，附服务器归属与请求 URI。
fn execute_read(
    servers: &[ResourceServerEntry],
    runtime: &tokio::runtime::Runtime,
    arguments: &Value,
    cancel: &AtomicBool,
) -> (String, bool) {
    let server_id = match arguments.get("server") {
        Some(Value::String(s)) => s.clone(),
        Some(_) => return (client_error_json("参数", "`server` 必须是字符串"), true),
        None => return (client_error_json("参数", "缺少必填参数 `server`"), true),
    };
    let uri = match arguments.get("uri") {
        Some(Value::String(s)) => s.clone(),
        Some(_) => return (client_error_json("参数", "`uri` 必须是字符串"), true),
        None => return (client_error_json("参数", "缺少必填参数 `uri`"), true),
    };
    let Some(entry) = find_server(servers, &server_id) else {
        return (
            client_error_json("参数", &format!("未知或未启用的资源服务器：{server_id}")),
            true,
        );
    };
    let result =
        runtime.block_on(entry.connection.read_resource_once(&uri, entry.tool_timeout, cancel));
    match result {
        Ok(read) => {
            // 023 §5.1 敏感数据：逐字段检查解码后的文本及 blob 原始字节，
            // 再检查投影结果——不能仅扫 JSON 序列化字符串而漏掉转义值。
            if contents_polluted(&read.contents, &entry.sensitive) {
                return (
                    client_error_json(
                        "敏感数据",
                        "资源结果包含凭证数据，已拒绝（已发调用的远端执行状态未知）",
                    ),
                    true,
                );
            }
            let mut projected = match serde_json::to_value(&read) {
                Ok(v) => v,
                Err(e) => {
                    return (
                        client_error_json("协议", &format!("资源读取结果序列化失败：{e}")),
                        true,
                    )
                }
            };
            if let Some(map) = projected.as_object_mut() {
                map.insert("server".into(), json!(server_id));
                map.insert("uri".into(), json!(uri));
            }
            finish_success(entry, projected)
        }
        Err(failure) => (failure_to_client_error(&failure, &entry.sensitive), true),
    }
}

/// 投影定向单页模板结果：服务器归属＋SDK 序列化的模板页。
fn project_templates_page(
    server_id: &str,
    result: &rmcp::model::ListResourceTemplatesResult,
) -> Value {
    let mut value = match serde_json::to_value(result) {
        Ok(v) => v,
        Err(e) => {
            return json!({"error": format!("模板页序列化失败：{e}")});
        }
    };
    if let Some(map) = value.as_object_mut() {
        map.insert("server".into(), json!(server_id));
    }
    value
}

/// 逐内容项污染检查（023 §5.1）：每段 contents 的 URI、mimeType、正文
/// 原文、blob 原文与 base64 解码后的字节、`_meta` 序列化值。
fn contents_polluted(
    contents: &[rmcp::model::ResourceContents],
    sensitive: &SensitiveValues,
) -> bool {
    if sensitive.is_empty() {
        return false;
    }
    contents.iter().any(|content| match content {
        rmcp::model::ResourceContents::TextResourceContents {
            uri, mime_type, text, meta,
        } => {
            sensitive.contains_secret(uri)
                || mime_type.as_deref().is_some_and(|m| sensitive.contains_secret(m))
                || sensitive.contains_secret(text)
                || meta.as_ref().is_some_and(|m| meta_polluted(m, sensitive))
        }
        rmcp::model::ResourceContents::BlobResourceContents {
            uri, mime_type, blob, meta,
        } => {
            sensitive.contains_secret(uri)
                || mime_type.as_deref().is_some_and(|m| sensitive.contains_secret(m))
                || sensitive.contains_secret(blob)
                // base64 解码后的原始字节（解码失败按原文检查即可——
                // 编码字符串本身已查过）。
                || base64::engine::general_purpose::STANDARD
                    .decode(blob.as_bytes())
                    .is_ok_and(|decoded| sensitive.contains_secret_bytes(&decoded))
                || meta.as_ref().is_some_and(|m| meta_polluted(m, sensitive))
        }
        _ => false,
    })
}

/// `_meta` 对象逐值检查（元数据值可为任意 JSON）。
fn meta_polluted(meta: &rmcp::model::MetaObject, sensitive: &SensitiveValues) -> bool {
    match serde_json::to_value(meta) {
        Ok(value) => sensitive.contains_secret(&value.to_string()),
        Err(_) => true, // 无法序列化的元数据按污染处理（保守拒绝）
    }
}

/// 汇总中止分类：前四类是**整次失败**（取消／整体时限／预算／污染），
/// `ServerFailed` 是可归属单台的失败（记 `errors`、丢弃该台半份目录）。
enum AggregateAbort {
    Cancelled,
    Deadline,
    Budget,
    Polluted,
    ServerFailed(String),
}

/// 跨服务器汇总（023 §3.3／§5.1）：按 server_id 升序串行遍历，每台完整
/// 分页（≤100 页、≤2048 项、游标 ≤64 KiB、重复游标拒绝）；R1=A 单一
/// 整体时限＝参与台 `tool_timeout` 最大值，页请求不重新获得计时窗口；
/// R3=A 台级失败保留其余台数据并附 `errors`（全部台失败也返回空集合＋
/// errors），取消／整体时限／预算超限／污染则整次失败。
fn execute_aggregate(
    servers: &[ResourceServerEntry],
    runtime: &tokio::runtime::Runtime,
    templates: bool,
    cancel: &AtomicBool,
) -> (String, bool) {
    let overall = servers
        .iter()
        .map(|s| s.tool_timeout)
        .max()
        .unwrap_or_default();
    let deadline = std::time::Instant::now() + overall;
    let mut collected: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut budget_bytes: usize = 0;
    for entry in servers {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return (
                client_error_json("状态未知", "用户取消，远端执行状态未知"),
                true,
            );
        }
        match collect_server_pages(entry, runtime, templates, deadline, cancel, &mut collected, &mut budget_bytes) {
            Ok(()) => {}
            Err(AggregateAbort::Cancelled) => {
                return (
                    client_error_json("状态未知", "用户取消，远端执行状态未知"),
                    true,
                )
            }
            Err(AggregateAbort::Deadline) => {
                return (
                    client_error_json(
                        "状态未知",
                        &format!("汇总超过整体时限 {overall:?}，远端执行状态未知"),
                    ),
                    true,
                )
            }
            Err(AggregateAbort::Budget) => {
                return (
                    client_error_json(
                        "容量",
                        &format!(
                            "汇总结果超过 {MAX_RESULT_JSON_BYTES} 字节预算，整次失败，不发布已收集数据"
                        ),
                    ),
                    true,
                )
            }
            Err(AggregateAbort::Polluted) => {
                return (
                    client_error_json(
                        "敏感数据",
                        "资源结果包含凭证数据，已拒绝（已发调用的远端执行状态未知）",
                    ),
                    true,
                )
            }
            Err(AggregateAbort::ServerFailed(reason)) => {
                // 该台失败：丢弃半份目录（collect 内部暂存，未入 collected），
                // 记入 errors 后继续其余台。
                errors.push(json!({
                    "server": entry.server_id,
                    "error": super::secrets::sanitize_detail(&reason, &entry.sensitive),
                }));
            }
        }
    }
    let key = if templates { "resourceTemplates" } else { "resources" };
    let mut out = json!({});
    out[key] = Value::Array(collected);
    if !errors.is_empty() {
        out["errors"] = Value::Array(errors);
    }
    // 最终整体序列化仍受 8 MiB 预算约束（R2=B；收集期间已增量检查）。
    match serde_json::to_string(&out) {
        Ok(text) if text.len() <= MAX_RESULT_JSON_BYTES => (text, false),
        Ok(_) => (
            client_error_json(
                "容量",
                &format!("汇总结果超过 {MAX_RESULT_JSON_BYTES} 字节预算，整次失败"),
            ),
            true,
        ),
        Err(e) => (
            client_error_json("协议", &format!("汇总结果序列化失败：{e}")),
            true,
        ),
    }
}

/// 单台完整分页收集（成功项暂存本台，全部页通过污染与预算检查后才并入
/// `collected`——失败时不输出半份目录）。
fn collect_server_pages(
    entry: &ResourceServerEntry,
    runtime: &tokio::runtime::Runtime,
    templates: bool,
    deadline: std::time::Instant,
    cancel: &AtomicBool,
    collected: &mut Vec<Value>,
    budget_bytes: &mut usize,
) -> Result<(), AggregateAbort> {
    let mut cursor: Option<String> = None;
    let mut seen: HashSet<String> = HashSet::new();
    let mut pages = 0usize;
    let mut items_count = 0usize;
    let mut server_items: Vec<Value> = Vec::new();
    let method = if templates { "resources/templates/list" } else { "resources/list" };
    loop {
        if pages == MAX_CATALOG_PAGES {
            return Err(AggregateAbort::ServerFailed(format!(
                "{method} 超过分页上限 {MAX_CATALOG_PAGES} 页"
            )));
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(AggregateAbort::Deadline);
        }
        let remaining = deadline - now;
        let outcome: Result<(Vec<Value>, Option<String>), AggregateAbort> = if templates {
            match runtime.block_on(entry.connection.list_resource_templates_once(
                cursor.clone(),
                remaining,
                cancel,
            )) {
                Ok(page) => match page
                    .resource_templates
                    .iter()
                    .map(serde_json::to_value)
                    .collect()
                {
                    Ok(items) => Ok((items, page.next_cursor)),
                    Err(e) => Err(AggregateAbort::ServerFailed(format!(
                        "{method} 结果项序列化失败：{e}"
                    ))),
                },
                Err(failure) => Err(failure_into_abort(failure, deadline, cancel)),
            }
        } else {
            match runtime.block_on(
                entry
                    .connection
                    .list_resources_once(cursor.clone(), remaining, cancel),
            ) {
                Ok(page) => match page
                    .resources
                    .iter()
                    .map(serde_json::to_value)
                    .collect()
                {
                    Ok(items) => Ok((items, page.next_cursor)),
                    Err(e) => Err(AggregateAbort::ServerFailed(format!(
                        "{method} 结果项序列化失败：{e}"
                    ))),
                },
                Err(failure) => Err(failure_into_abort(failure, deadline, cancel)),
            }
        };
        let (items, next_cursor) = outcome?;
        pages += 1;
        items_count += items.len();
        if items_count > MAX_CATALOG_ITEMS {
            return Err(AggregateAbort::ServerFailed(format!(
                "{method} 累计超过 {MAX_CATALOG_ITEMS} 项"
            )));
        }
        server_items.extend(items);
        match next_cursor {
            None => break,
            Some(next) => {
                if next.len() > MAX_CURSOR_BYTES {
                    return Err(AggregateAbort::ServerFailed(format!(
                        "{method} 游标超过 64 KiB（{MAX_CURSOR_BYTES} 字节）"
                    )));
                }
                if !seen.insert(next.clone()) {
                    return Err(AggregateAbort::ServerFailed(format!(
                        "{method} 返回重复游标（服务目录错误）"
                    )));
                }
                cursor = Some(next);
            }
        }
    }
    // 污染与预算（R2=B）：本台全部项按字符串叶子逐字段检查（不漏转义值），
    // 累计字节超限整次失败。
    let joined = match serde_json::to_string(&server_items) {
        Ok(t) => t,
        Err(e) => {
            return Err(AggregateAbort::ServerFailed(format!(
                "{method} 结果序列化失败：{e}"
            )))
        }
    };
    if server_items
        .iter()
        .any(|v| value_polluted(v, &entry.sensitive))
    {
        return Err(AggregateAbort::Polluted);
    }
    *budget_bytes += joined.len();
    if *budget_bytes > MAX_RESULT_JSON_BYTES {
        return Err(AggregateAbort::Budget);
    }
    for mut item in server_items {
        if let Some(map) = item.as_object_mut() {
            map.insert("server".into(), json!(entry.server_id));
        }
        collected.push(item);
    }
    Ok(())
}

/// 页请求失败归因：以「到整体 deadline 的剩余时间」为限发出的请求失败后，
/// 先判取消与整体时限（整次失败），再归类为单台失败（不重放、不自动重试）。
fn failure_into_abort(
    failure: CallFailure,
    deadline: std::time::Instant,
    cancel: &AtomicBool,
) -> AggregateAbort {
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        AggregateAbort::Cancelled
    } else if std::time::Instant::now() >= deadline {
        AggregateAbort::Deadline
    } else {
        AggregateAbort::ServerFailed(format!("{failure}"))
    }
}

/// 失败分类 → 标准客户端错误（脱敏；022 §2.4 同源语义）。
fn failure_to_client_error(failure: &CallFailure, sensitive: &SensitiveValues) -> String {
    let (kind, detail) = match failure {
        CallFailure::NotSent(d) => ("未发送", d.clone()),
        CallFailure::SentUnknown(d) => ("状态未知", d.clone()),
        CallFailure::SentRejected(d) => ("协议", d.clone()),
        CallFailure::Unsupported(d) => ("协议", d.clone()),
    };
        client_error_json(kind, &super::secrets::sanitize_detail(detail.as_str(), sensitive))
}
