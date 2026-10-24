//! 022 §2.3：MCP 工具模型名生成。
//!
//! 通常模型名为 `mcp__<server>__<tool>`；非法字符替换为下划线；发生净化／
//! 分隔歧义冲突或超长时附稳定短哈希并截断可读前缀，最终名称符合
//! `[A-Za-z0-9_-]{1,64}`。短哈希基于原始 `(server, tool)` 身份，使用现有
//! sha2；同一目录不同枚举顺序结果一致，剩余冲突显式拒绝，不覆盖注册表。
//! 名字是 API 标识，原始身份是路由依据——两者不可混用。

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Chat Completions 函数名上限（022 §2.3：本项目 CC 上限为 64，不沿用
/// Codex 的 128）。
pub const MAX_MODEL_TOOL_NAME: usize = 64;
/// 短哈希长度（移植 Codex tools.rs 的 12 位十六进制后缀）。
const HASH_LEN: usize = 12;

/// 名称净化（移植 Codex `sanitize_responses_api_tool_name`）：仅保留 ASCII
/// 字母、数字与下划线，其余字符（含连字符）替换为下划线；全空结果回退为
/// 单个下划线。净化产物天然满足 CC 名字字符集。
pub fn sanitize_part(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            sanitized.push(c);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

/// 未附加哈希的可读基础名。
fn base_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize_part(server), sanitize_part(tool))
}

/// 原始身份短哈希（sha2；输入为 `\0` 连接的原始 server／tool，与枚举顺序无关）。
fn identity_hash(server: &str, tool: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server.as_bytes());
    hasher.update([0u8]);
    hasher.update(tool.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .chars()
        .take(HASH_LEN)
        .collect()
}

/// 原始名字去重后按净化基础名分组，冲突或超长条目附哈希并截断前缀；
/// 返回与输入同序的最终模型名。原始名重复＝服务目录错误，整体失败。
pub fn assign_model_names(
    items: &[(String, String)],
) -> Result<Vec<String>, NameAssignError> {
    let mut seen_raw = HashSet::new();
    for (server, tool) in items {
        if !seen_raw.insert((server.clone(), tool.clone())) {
            return Err(NameAssignError::DuplicateRaw {
                server: server.clone(),
                tool: tool.clone(),
            });
        }
    }
    // 同一基础名对应多个不同原始身份＝净化／分隔歧义冲突（含 `__` 名字）。
    let mut groups: HashMap<String, usize> = HashMap::new();
    for (server, tool) in items {
        *groups.entry(base_name(server, tool)).or_default() += 1;
    }
    let mut out = Vec::with_capacity(items.len());
    for (server, tool) in items {
        let base = base_name(server, tool);
        let collides = groups.get(base.as_str()).copied().unwrap_or(1) > 1;
        if collides || base.len() > MAX_MODEL_TOOL_NAME {
            // 附稳定短哈希并截断可读前缀：全部 ASCII（净化后），字符数＝字节数。
            let suffix = format!("_{}", identity_hash(server, tool));
            let max_prefix = MAX_MODEL_TOOL_NAME - suffix.len();
            let mut truncated = base;
            truncated.truncate(max_prefix);
            out.push(format!("{truncated}{suffix}"));
        } else {
            out.push(base);
        }
    }
    // 残余冲突显式拒绝（不覆盖注册表）。
    let mut seen_final = HashSet::new();
    for name in &out {
        if !seen_final.insert(name.clone()) {
            return Err(NameAssignError::UnresolvableCollision(name.clone()));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameAssignError {
    /// 同服务器目录中原始工具名重复。
    DuplicateRaw { server: String, tool: String },
    /// 附加哈希后仍冲突（显式拒绝，不覆盖）。
    UnresolvableCollision(String),
}

impl std::fmt::Display for NameAssignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateRaw { server, tool } => {
                write!(f, "服务器 {server} 工具目录存在重复原始名：{tool}")
            }
            Self::UnresolvableCollision(name) => {
                write!(f, "模型名冲突无法消解：{name}")
            }
        }
    }
}

