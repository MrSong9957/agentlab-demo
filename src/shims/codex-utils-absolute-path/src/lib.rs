//! codex-utils-absolute-path shim（008 §3-J5）：AbsolutePathBuf 最小本地实现。
/// 母本为规范绝对路径 newtype（serde + Display + 当前目录工具）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AbsolutePathBuf(std::path::PathBuf);
impl AbsolutePathBuf {
    pub fn new(p: std::path::PathBuf) -> Self { Self(p) }
    pub fn from_absolute_path(p: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let p = p.as_ref();
        if p.is_absolute() { Ok(Self(p.to_path_buf())) }
        else { Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not absolute")) }
    }
    pub fn from_absolute_path_checked(p: impl AsRef<std::path::Path>) -> Result<Self, String> {
        Self::from_absolute_path(p).map_err(|e| e.to_string())
    }
    pub fn current_dir() -> std::io::Result<Self> {
        Ok(Self(std::env::current_dir()?))
    }
    pub fn as_path(&self) -> &std::path::Path { &self.0 }
    pub fn into_path_buf(self) -> std::path::PathBuf { self.0 }
    pub fn join(&self, p: impl AsRef<std::path::Path>) -> Self { Self(self.0.join(p)) }
    pub fn parent(&self) -> Option<&std::path::Path> { self.0.parent() }
    pub fn file_name(&self) -> Option<&std::ffi::OsStr> { self.0.file_name() }
    pub fn display(&self) -> std::path::Display<'_> { self.0.display() }
    pub fn to_path_buf(&self) -> std::path::PathBuf { self.0.clone() }
    /// 母本 utils/absolute-path resolve_path_against_base：相对路径基于 base 解析＋词法规范化＋~ 展开。
    /// 本地实现以手工词法规范化替代母本的 absolutize crate（行为等价：去 ./、消解 ../）。
    pub fn resolve_path_against_base<P: AsRef<std::path::Path>, B: AsRef<std::path::Path>>(
        path: P,
        base_path: B,
    ) -> Self {
        let mut p = path.as_ref().to_path_buf();
        if let Some(rest) = p.to_str().and_then(|s| s.strip_prefix("~/")) {
            if let Some(home) = std::env::home_dir() {
                p = home.join(rest);
            }
        }
        if !p.is_absolute() {
            p = base_path.as_ref().join(p);
        }
        let mut out = std::path::PathBuf::new();
        for c in p.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other),
            }
        }
        Self(out)
    }
}
impl std::fmt::Display for AbsolutePathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.display().fmt(f) }
}
impl AsRef<std::path::Path> for AbsolutePathBuf {
    fn as_ref(&self) -> &std::path::Path { &self.0 }
}
impl From<AbsolutePathBuf> for std::path::PathBuf {
    fn from(v: AbsolutePathBuf) -> Self { v.0 }
}
/// 母本 utils/absolute-path lib.rs:250 test_support——原样语义。
pub mod test_support {
    use super::AbsolutePathBuf;
    use std::path::Path;
    use std::path::PathBuf;

    /// Creates a platform-absolute [`PathBuf`] from a Unix-style absolute test path.
    ///
    /// On Windows, `/tmp/example` maps to `C:\tmp\example`.
    pub fn test_path_buf(unix_path: &str) -> PathBuf {
        if cfg!(windows) {
            let mut path = PathBuf::from(r"C:\");
            path.extend(
                unix_path
                    .trim_start_matches('/')
                    .split('/')
                    .filter(|segment| !segment.is_empty()),
            );
            path
        } else {
            PathBuf::from(unix_path)
        }
    }

    /// Extension methods for converting paths into [`AbsolutePathBuf`] values in tests.
    pub trait PathExt {
        /// Converts an already absolute path into an [`AbsolutePathBuf`].
        fn abs(&self) -> AbsolutePathBuf;
    }

    impl PathExt for Path {
        fn abs(&self) -> AbsolutePathBuf {
            AbsolutePathBuf::from_absolute_path_checked(self)
                .expect("path should already be absolute")
        }
    }

    /// Extension methods for converting path buffers into [`AbsolutePathBuf`] values in tests.
    pub trait PathBufExt {
        /// Converts an already absolute path buffer into an [`AbsolutePathBuf`].
        fn abs(&self) -> AbsolutePathBuf;
    }

    impl PathBufExt for PathBuf {
        fn abs(&self) -> AbsolutePathBuf {
            self.as_path().abs()
        }
    }
}
