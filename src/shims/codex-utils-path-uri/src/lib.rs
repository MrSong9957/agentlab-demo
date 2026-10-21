//! codex-utils-path-uri shim（008 §3-J5）：LegacyAppPathString/PathUri 真实 newtype 结构
//! 与消费方法（api_path_string.rs 的 to_inferred_path_uri/render_for_ui/into_string、
//! PathUri::inferred_native_path_string 的本地回退实现——本地无 Windows UNC 场景，
//! 惯例推断退化为 Unix 绝对路径判定，登记）。

/// 母本 api_path_string.rs:38 LegacyAppPathString——String newtype。
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct LegacyAppPathString(pub String);

impl LegacyAppPathString {
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Parses this API string as an absolute path using the convention inferred from its spelling.
    pub fn to_inferred_path_uri(&self) -> Option<PathUri> {
        PathUri::try_from(self.clone()).ok()
    }

    /// Renders this API path for display in a user interface.
    pub fn render_for_ui(&self) -> String {
        self.to_inferred_path_uri()
            .map(|path| path.inferred_native_path_string())
            .unwrap_or_else(|| self.0.clone())
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for LegacyAppPathString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 母本 PathUri 的最小本地映射：绝对路径字符串 URI。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathUri(pub String);

impl PathUri {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// 母本按推断惯例渲染原生路径。本地映射：
    /// - `/{盘符}:` 形态去前导斜杠（Windows 盘符惯例）；
    /// - 其余（含 `//host/…` UNC）原样输出。
    pub fn inferred_native_path_string(&self) -> String {
        let s = &self.0;
        let bytes = s.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            return s[1..].to_string();
        }
        s.clone()
    }

    /// 母本 PathUri::basename：末段路径名（根返回 None——母本语义）。
    pub fn basename(&self) -> Option<&str> {
        self.0.rsplit('/').find(|s| !s.is_empty())
    }

    /// 母本 PathUri::parse：解析 URI 字符串（Result 语义对齐）。
    /// 本地映射覆盖消费面输入形态：绝对 Unix 路径、file:///（含 AfterPath 切片带 scheme）、
    /// file://host/share（UNC）、file://localhost；路径部分按百分比解码后存储。
    pub fn parse(s: &str) -> Result<Self, String> {
        let decoded = |p: &str| {
            urlencoding::decode(p)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| p.to_string())
        };
        if let Some(rest) = s.strip_prefix("file://localhost") {
            let p = decoded(rest);
            return Ok(Self(if p.starts_with('/') { p } else { format!("/{p}") }));
        }
        if let Some(rest) = s.strip_prefix("file://") {
            // UNC：file://host/share/… → //host/share/…
            if let Some((host, path)) = rest.split_once('/') {
                if !host.is_empty() && host != "localhost" {
                    let p = decoded(path);
                    return Ok(Self(format!("//{host}/{p}")));
                }
            }
            let p = decoded(rest);
            return Ok(Self(if p.starts_with('/') { p } else { format!("/{p}") }));
        }
        if s.starts_with('/') {
            return Ok(Self(decoded(s)));
        }
        Err(format!("not an absolute path: {s}"))
    }
}

impl std::fmt::Display for PathUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<LegacyAppPathString> for PathUri {
    type Error = String;

    fn try_from(value: LegacyAppPathString) -> Result<Self, Self::Error> {
        let s = value.0;
        if s.starts_with('/') {
            Ok(Self(s))
        } else {
            Err(format!("not an absolute path: {s}"))
        }
    }
}

/// 母本 PathConvention——本地仅 Unix。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathConvention {
    Posix,
}
