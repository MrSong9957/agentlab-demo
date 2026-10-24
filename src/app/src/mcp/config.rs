//! 022 §2.1：`mcp.json` 严格解析。
//!
//! 顶层固定 `{"mcp_servers": {<服务器 ID>: {…}}}`；拒绝未知配置字段、空白
//! ID／工具名、重复 JSON 键和错误类型，不用后一个同名字段覆盖前一个。
//! 文件缺失等同无 MCP 配置：不创建文件、不启动进程、不连接服务器。
//! 停用（enabled=false）条目仍校验配置结构，但不解析凭证、不检查程序是否
//! 安装、不发网络请求——凭证只在连接准备时按环境变量名读取实际值。

use serde::de::{MapAccess, Visitor};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 启动／工具超时缺省值（022 §2.1：来自 Codex 基线 30／300 秒）。
pub const DEFAULT_STARTUP_TIMEOUT_SEC: u64 = 30;
pub const DEFAULT_TOOL_TIMEOUT_SEC: u64 = 300;

/// 本地工具声明总数上限（022 §2.3：含本轮条件原生工具；CC 请求面边界）。
pub const MAX_TOTAL_TOOLS: usize = 128;

/// 传输层管理的保留字请求头（022 §2.1：不允许覆盖）。
pub const RESERVED_HTTP_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "connection",
    "accept",
    "content-type",
    "mcp-protocol-version",
    "mcp-session-id",
];

/// stdio 传输配置。
#[derive(Debug, Clone, PartialEq)]
pub struct StdioConfig {
    /// 程序：绝对路径，或从受控 PATH 查找的裸程序名（带路径的相对形式已拒绝）。
    pub command: String,
    /// 参数数组：直接按程序和参数启动，不经隐式 shell。
    pub args: Vec<String>,
    /// 已有绝对目录；缺省＝本次宿主启动目录（加载时解析）。
    pub cwd: PathBuf,
    /// 仅转交列出的宿主环境变量（缺失时该服务器准备失败）。
    pub env_vars: Vec<String>,
}

/// Streamable HTTP 传输配置。认证值只在连接准备时读取。
#[derive(Debug, Clone, PartialEq)]
pub struct HttpConfig {
    /// 完整 MCP 端点：合法 http(s) 绝对地址，无 userinfo／fragment；
    /// 远程 HTTPS，仅 loopback 允许 HTTP；不拼接 `/mcp`，禁止自动重定向。
    pub url: String,
    /// Bearer 凭证的环境变量名（None＝匿名连接）。
    pub bearer_token_env_var: Option<String>,
    /// 额外请求头（header 名 → 环境变量名），保持声明顺序。
    pub env_http_headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum McpTransport {
    Stdio(StdioConfig),
    Http(HttpConfig),
}

/// 单台服务器通过结构校验后的配置（不含任何凭证实际值）。
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerConfig {
    /// 只有显式 true 才连接；默认 false。
    pub enabled: bool,
    pub transport: McpTransport,
    /// 允许名单：None＝缺省（全部工具），Some(空)＝零工具。
    pub enabled_tools: Option<Vec<String>>,
    /// 禁用名单：优先于允许名单。
    pub disabled_tools: Vec<String>,
    /// 初始化及首次完整发现共用的启动总时限。
    pub startup_timeout: Duration,
    /// 单次调用总时限（不因进度消息延长）。
    pub tool_timeout: Duration,
}

/// 解析后的 mcp.json：按 JSON 声明顺序保存服务器条目（固定 ID 顺序）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct McpFile {
    pub servers: Vec<(String, McpServerConfig)>,
}

/// 配置加载失败（整体不发布新配置）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigError(pub String);

impl std::fmt::Display for McpConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for McpConfigError {}

fn err(msg: impl Into<String>) -> McpConfigError {
    McpConfigError(msg.into())
}

/// 读取并严格解析 `user_dir/mcp.json`；文件缺失＝无 MCP 配置（不创建文件）。
pub fn load_mcp_file(user_dir: &Path) -> Result<McpFile, McpConfigError> {
    let path = user_dir.join("mcp.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(McpFile::default()),
        Err(e) => return Err(err(format!("读取 mcp.json 失败：{e}"))),
    };
    parse_mcp_file(&text)
}

/// 解析 mcp.json 文本（首次加载与 reload 复用同一入口）。
pub fn parse_mcp_file(text: &str) -> Result<McpFile, McpConfigError> {
    let root: RawTop = serde_json::from_str(text)
        .map_err(|e| err(format!("mcp.json 解析失败：{e}")))?;
    let mut servers = Vec::new();
    for (id, fields) in root.0.mcp_servers.0 {
        // 重复 ID 已在 ServersVisitor 反序列化层拒绝；此处只查空白 ID。
        if id.trim().is_empty() {
            return Err(err("mcp_servers 含空白服务器 ID"));
        }
        let config = parse_server(&id, fields)?;
        servers.push((id, config));
    }
    Ok(McpFile { servers })
}

// ---- 手写 serde visitor：serde_json 的 Map 反序列化会静默合并重复键，
// ---- 与「拒绝重复 JSON 键」契约冲突，因此各层对象都手动迭代键值对。

struct RawTop(RawTopFields);

impl<'de> serde::Deserialize<'de> for RawTop {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(TopVisitor)
    }
}

struct TopVisitor;

impl<'de> Visitor<'de> for TopVisitor {
    type Value = RawTop;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("mcp.json 顶层对象")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut mcp_servers: Option<RawServers> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "mcp_servers" => {
                    if mcp_servers.is_some() {
                        return Err(serde::de::Error::custom("重复配置字段：mcp_servers"));
                    }
                    mcp_servers = Some(map.next_value()?);
                }
                other => {
                    return Err(serde::de::Error::custom(format!("未知配置字段：{other}")));
                }
            }
        }
        Ok(RawTop(RawTopFields {
            mcp_servers: mcp_servers.unwrap_or(RawServers(Vec::new())),
        }))
    }
}

struct RawTopFields {
    mcp_servers: RawServers,
}

struct RawServers(Vec<(String, RawServerFields)>);

impl<'de> serde::Deserialize<'de> for RawServers {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ServersVisitor)
    }
}

struct ServersVisitor;

impl<'de> Visitor<'de> for ServersVisitor {
    type Value = RawServers;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("mcp_servers 对象")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "重复服务器 ID：{key}"
                )));
            }
            let fields: RawServerFields = map.next_value()?;
            out.push((key, fields));
        }
        Ok(RawServers(out))
    }
}

/// 单服务器原始条目（键值按出现顺序，重复键已在 visitor 拒绝）。
struct RawServerFields {
    entries: Vec<(String, serde_json::Value)>,
}

impl<'de> serde::Deserialize<'de> for RawServerFields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ServerVisitor;
        impl<'de> Visitor<'de> for ServerVisitor {
            type Value = RawServerFields;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("服务器配置对象")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                let mut seen = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom(format!("重复配置字段：{key}")));
                    }
                    let value: serde_json::Value = map.next_value()?;
                    out.push((key, value));
                }
                Ok(RawServerFields { entries: out })
            }
        }
        deserializer.deserialize_map(ServerVisitor)
    }
}

const KNOWN_FIELDS: &[&str] = &[
    "enabled",
    "command",
    "args",
    "cwd",
    "env_vars",
    "url",
    "bearer_token_env_var",
    "env_http_headers",
    "enabled_tools",
    "disabled_tools",
    "startup_timeout_sec",
    "tool_timeout_sec",
];

fn lookup<'a>(
    entries: &'a [(String, serde_json::Value)],
    name: &str,
) -> Option<&'a serde_json::Value> {
    entries.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn opt_string(
    id: &str,
    field: &str,
    v: &serde_json::Value,
) -> Result<String, McpConfigError> {
    match v {
        serde_json::Value::String(s) => Ok(s.clone()),
        _ => Err(err(format!("服务器 {id}：{field} 必须是字符串"))),
    }
}

fn string_list(
    id: &str,
    field: &str,
    v: &serde_json::Value,
) -> Result<Vec<String>, McpConfigError> {
    let arr = v
        .as_array()
        .ok_or_else(|| err(format!("服务器 {id}：{field} 必须是字符串数组")))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item
            .as_str()
            .ok_or_else(|| err(format!("服务器 {id}：{field} 的元素必须是字符串")))?;
        if s.trim().is_empty() {
            return Err(err(format!("服务器 {id}：{field} 含空白项")));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

fn opt_timeout(
    id: &str,
    field: &str,
    v: &serde_json::Value,
) -> Result<u64, McpConfigError> {
    match v {
        // u64 语义：拒绝负数、小数与字符串；零单独说明。
        serde_json::Value::Number(n) => match n.as_u64() {
            Some(sec) if sec > 0 => Ok(sec),
            Some(_) => Err(err(format!("服务器 {id}：{field} 必须是正整数秒（拒绝零）"))),
            None => Err(err(format!("服务器 {id}：{field} 必须是正整数秒"))),
        },
        _ => Err(err(format!("服务器 {id}：{field} 必须是正整数秒"))),
    }
}

fn parse_server(id: &str, raw: RawServerFields) -> Result<McpServerConfig, McpConfigError> {
    for (key, _) in &raw.entries {
        if !KNOWN_FIELDS.contains(&key.as_str()) {
            return Err(err(format!("服务器 {id}：未知配置字段：{key}")));
        }
    }
    let get = |name: &str| lookup(&raw.entries, name);

    let enabled = match get("enabled") {
        None => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| err(format!("服务器 {id}：enabled 必须是布尔值")))?,
    };

    let command = match get("command") {
        None => None,
        Some(v) => Some(opt_string(id, "command", v)?),
    };
    let url = match get("url") {
        None => None,
        Some(v) => Some(opt_string(id, "url", v)?),
    };
    let transport = match (command, url) {
        (Some(command), None) => McpTransport::Stdio(parse_stdio(id, command, &raw.entries)?),
        (None, Some(url)) => McpTransport::Http(parse_http(id, url, &raw.entries)?),
        (Some(_), Some(_)) => {
            return Err(err(format!("服务器 {id}：command 与 url 只能选择一个")));
        }
        (None, None) => {
            return Err(err(format!(
                "服务器 {id}：缺少 command（stdio）或 url（HTTP）"
            )));
        }
    };

    // 跨传输字段混用按配置错误拒绝（字段出现即拒绝，不做类型兜底）。
    let foreign_fields: &[&str] = match &transport {
        McpTransport::Stdio(_) => &["bearer_token_env_var", "env_http_headers"],
        McpTransport::Http(_) => &["args", "cwd", "env_vars"],
    };
    for field in foreign_fields {
        if get(field).is_some() {
            return Err(err(format!(
                "服务器 {id}：{field} 与所选传输不匹配（跨传输字段混用）"
            )));
        }
    }

    let enabled_tools = match get("enabled_tools") {
        None => None,
        Some(v) => Some(string_list(id, "enabled_tools", v)?),
    };
    let disabled_tools = match get("disabled_tools") {
        None => Vec::new(),
        Some(v) => string_list(id, "disabled_tools", v)?,
    };
    let startup_timeout_sec = match get("startup_timeout_sec") {
        None => DEFAULT_STARTUP_TIMEOUT_SEC,
        Some(v) => opt_timeout(id, "startup_timeout_sec", v)?,
    };
    let tool_timeout_sec = match get("tool_timeout_sec") {
        None => DEFAULT_TOOL_TIMEOUT_SEC,
        Some(v) => opt_timeout(id, "tool_timeout_sec", v)?,
    };

    Ok(McpServerConfig {
        enabled,
        transport,
        enabled_tools,
        disabled_tools,
        startup_timeout: Duration::from_secs(startup_timeout_sec),
        tool_timeout: Duration::from_secs(tool_timeout_sec),
    })
}

fn parse_stdio(
    id: &str,
    command: String,
    entries: &[(String, serde_json::Value)],
) -> Result<StdioConfig, McpConfigError> {
    if command.trim().is_empty() {
        return Err(err(format!("服务器 {id}：command 不能为空白")));
    }
    if !Path::new(&command).is_absolute() && command.contains('/') {
        return Err(err(format!(
            "服务器 {id}：command 带路径时必须是绝对路径（相对路径拒绝）"
        )));
    }
    let args = match lookup(entries, "args") {
        None => Vec::new(),
        Some(v) => string_list(id, "args", v)?,
    };
    let cwd = match lookup(entries, "cwd") {
        None => std::env::current_dir()
            .map_err(|e| err(format!("服务器 {id}：读取宿主启动目录失败：{e}")))?,
        Some(v) => {
            let s = opt_string(id, "cwd", v)?;
            let p = PathBuf::from(&s);
            if !p.is_absolute() {
                return Err(err(format!("服务器 {id}：cwd 必须是绝对路径")));
            }
            if !p.is_dir() {
                return Err(err(format!("服务器 {id}：cwd 必须是已存在的目录")));
            }
            p
        }
    };
    let env_vars = match lookup(entries, "env_vars") {
        None => Vec::new(),
        Some(v) => string_list(id, "env_vars", v)?,
    };
    Ok(StdioConfig { command, args, cwd, env_vars })
}

fn parse_http(
    id: &str,
    url: String,
    entries: &[(String, serde_json::Value)],
) -> Result<HttpConfig, McpConfigError> {
    if url.trim().is_empty() {
        return Err(err(format!("服务器 {id}：url 不能为空白")));
    }
    let parsed = url::Url::parse(&url)
        .map_err(|e| err(format!("服务器 {id}：url 不是合法绝对地址：{e}")))?;
    match parsed.scheme() {
        "https" => {}
        "http" => {
            let host = parsed.host_str().unwrap_or("");
            let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
            if !loopback {
                return Err(err(format!(
                    "服务器 {id}：HTTP 仅允许 localhost／127.0.0.1／::1（远程必须 HTTPS）"
                )));
            }
        }
        other => {
            return Err(err(format!(
                "服务器 {id}：url 仅支持 http／https，遇到 {other}"
            )));
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(err(format!("服务器 {id}：url 不允许携带 userinfo")));
    }
    if parsed.fragment().is_some() {
        return Err(err(format!("服务器 {id}：url 不允许携带 fragment")));
    }

    let bearer_token_env_var = match lookup(entries, "bearer_token_env_var") {
        None => None,
        Some(v) => {
            let s = opt_string(id, "bearer_token_env_var", v)?;
            if s.trim().is_empty() {
                return Err(err(format!(
                    "服务器 {id}：bearer_token_env_var 不能为空白"
                )));
            }
            Some(s)
        }
    };
    let mut env_http_headers: Vec<(String, String)> = Vec::new();
    if let Some(v) = lookup(entries, "env_http_headers") {
        let map = v
            .as_object()
            .ok_or_else(|| err(format!("服务器 {id}：env_http_headers 必须是对象")))?;
        let mut seen_casing: Vec<String> = Vec::new();
        for (name, value) in map {
            if name.is_empty() || name.contains(['\r', '\n']) {
                return Err(err(format!(
                    "服务器 {id}：env_http_headers 头名非法：{name:?}"
                )));
            }
            let lower = name.to_ascii_lowercase();
            if RESERVED_HTTP_HEADERS.contains(&lower.as_str()) {
                return Err(err(format!(
                    "服务器 {id}：env_http_headers 不允许覆盖传输层管理的头：{name}"
                )));
            }
            if seen_casing
                .iter()
                .any(|prev| prev.eq_ignore_ascii_case(&lower))
            {
                return Err(err(format!(
                    "服务器 {id}：env_http_headers 头名大小写重复：{name}"
                )));
            }
            seen_casing.push(lower);
            let env_var = value.as_str().ok_or_else(|| {
                err(format!(
                    "服务器 {id}：env_http_headers[{name}] 的值必须是环境变量名字符串"
                ))
            })?;
            if env_var.trim().is_empty() {
                return Err(err(format!(
                    "服务器 {id}：env_http_headers[{name}] 的环境变量名不能为空白"
                )));
            }
            env_http_headers.push((name.clone(), env_var.to_string()));
        }
    }
    // Authorization 只能有一个来源。
    if bearer_token_env_var.is_some()
        && env_http_headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    {
        return Err(err(format!(
            "服务器 {id}：Authorization 只能有一个来源（bearer_token_env_var 与 env_http_headers 互斥）"
        )));
    }
    Ok(HttpConfig {
        url,
        bearer_token_env_var,
        env_http_headers,
    })
}
