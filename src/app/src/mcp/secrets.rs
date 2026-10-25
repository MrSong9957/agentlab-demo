//! 022 §2.1 凭证回显处理（2026-10-24 用户确认选择：拒绝受污染数据）。
//!
//! 每个连接在准备时固定敏感值集合：Bearer 及 `env_vars`／`env_http_headers`
//! 显式引用的非空实际值均按敏感值处理（不按变量名猜测、不遍历其他宿主环境
//! 变量），并包含已发送认证头形式。服务器定义、结果及诊断在进入模型请求、
//! 会话／记忆、展示或普通日志之前检查；检测先于截断。
//! 这不是任意第三方秘密识别器，也不扫描原有用户历史。

use std::collections::HashSet;

/// 一次连接的固定敏感值集合（Arc 共享给快照与连接）。
#[derive(Debug, Clone, Default)]
pub struct SensitiveValues {
    /// 非空敏感原文（Bearer 值、"Bearer <值>" 认证头形式、显式引用的环境值）。
    values: Vec<String>,
}

impl SensitiveValues {
    /// 从准备阶段已解析的凭证值构建；空值／缺失不进入集合（该情形下连接
    /// 准备本身失败，不降为匿名）。
    pub fn from_values(values: impl IntoIterator<Item = String>) -> Self {
        let mut values: Vec<String> = values
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        // 长值优先检查：短值可能是长值的平凡子串，先命中长值便于内部定位；
        // contains 判定本身与顺序无关。
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        SensitiveValues { values }
    }

    /// 文本是否包含任一敏感值（污染判定）。
    pub fn contains_secret(&self, text: &str) -> bool {
        self.values.iter().any(|v| text.contains(v.as_str()))
    }

    /// 字节串是否包含任一敏感值的 UTF-8 字节（023：blob 解码后检查；
    /// 敏感值均为环境变量文本，按字节子串匹配覆盖多字节字符）。
    pub fn contains_secret_bytes(&self, bytes: &[u8]) -> bool {
        self.values
            .iter()
            .any(|v| contains_subslice(bytes, v.as_bytes()))
    }

    /// 集合为空＝无敏感值（匿名、无显式环境引用的连接）。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// 诊断／错误文本脱敏：包含敏感值时替换为通用诊断（不回显匹配值、
/// 原始片段或指纹）。022／023 的错误与诊断出口共用。
pub fn sanitize_detail(text: &str, sensitive: &SensitiveValues) -> String {
    if sensitive.contains_secret(text) {
        "（细节包含敏感数据，已省略）".to_string()
    } else {
        text.to_string()
    }
}

/// 从 Bearer 值构造敏感值集合：值本身＋已发送认证头形式（`Bearer <值>`、
/// `Authorization: Bearer <值>`）。
pub fn bearer_sensitive_values(token: &str) -> Vec<String> {
    if token.is_empty() {
        return Vec::new();
    }
    vec![
        token.to_string(),
        format!("Bearer {token}"),
        format!("Authorization: Bearer {token}"),
    ]
}

/// 字节子串匹配（空的敏感值不参与；`contains_secret_bytes` 辅助）。
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.windows(needle.len()).any(|w| w == needle)
}

/// HTTP 认证解析结果（只含环境变量名到实际值的映射，供传输层使用；
/// 敏感值集合在连接准备时固定）。
pub struct ResolvedHttpAuth {
    /// Bearer 认证头值（`auth_header` 语义：不带 `Bearer ` 前缀）。
    pub auth_header: Option<String>,
    /// env_http_headers 解析出的自定义头（仅值非空者）。
    pub custom_headers: Vec<(String, String)>,
    /// 已配置但缺失／空值的凭证引用（该服务器准备失败，不降为匿名）。
    pub missing: Vec<String>,
}

/// 解析 HTTP 服务器的凭证引用（022 §2.1：配置后缺失／空值则失败，
/// 不能降为匿名）。凭证值不进入配置、会话或日志。
pub fn resolve_http_auth_refs(http: &super::config::HttpConfig) -> ResolvedHttpAuth {
    let mut missing = Vec::new();
    let auth_header = match &http.bearer_token_env_var {
        None => None,
        Some(name) => match std::env::var(name) {
            Ok(value) if !value.is_empty() => Some(value),
            _ => {
                missing.push(name.clone());
                None
            }
        },
    };
    let mut custom_headers = Vec::new();
    for (header_name, env_name) in &http.env_http_headers {
        match std::env::var(env_name) {
            Ok(value) if !value.is_empty() => {
                custom_headers.push((header_name.clone(), value));
            }
            _ => missing.push(env_name.clone()),
        }
    }
    ResolvedHttpAuth {
        auth_header,
        custom_headers,
        missing,
    }
}

/// 汇总一个连接的全部敏感值：Bearer 值（含认证头形式）＋显式引用的
/// env_vars／env_http_headers 非空实际值。
pub fn collect_sensitive_values(
    stdio_env_vars: &[String],
    http: Option<&ResolvedHttpAuth>,
) -> SensitiveValues {
    let mut values: Vec<String> = Vec::new();
    for name in stdio_env_vars {
        if let Ok(value) = std::env::var(name) {
            values.push(value);
        }
    }
    if let Some(auth) = http {
        if let Some(token) = &auth.auth_header {
            values.extend(bearer_sensitive_values(token));
        }
        for (_, value) in &auth.custom_headers {
            values.push(value.clone());
        }
    }
    SensitiveValues::from_values(values)
}
