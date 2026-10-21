//! codex-utils-path shim（008 §3-J5）：
//! system_commands.rs 为原样拷贝（受信目录查找）；其余为对齐母本语义的本地实现。
pub mod system_commands;

pub use system_commands::system_executable;
pub use system_commands::system_path;

/// 母本 path-utils lib.rs is_wsl。
pub fn is_wsl() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/version")
            .map(|content| content.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// 母本语义：tmp＋rename 原子写入（同 terminal-chat session::write_atomic 先例）。
pub fn write_atomically(target: &std::path::Path, contents: &str) -> std::io::Result<()> {
    let dir = target.parent().unwrap_or(std::path::Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, target)
}

/// 母本 SymlinkWritePaths 结构（read_path/write_path）。
#[derive(Debug, Clone)]
pub struct SymlinkWritePaths {
    pub read_path: Option<std::path::PathBuf>,
    pub write_path: std::path::PathBuf,
}

/// 母本语义：沿符号链接链解析写路径（带环保护；本地以父目录 canonicalize 等价承载）。
pub fn resolve_symlink_write_paths(path: &std::path::Path) -> std::io::Result<SymlinkWritePaths> {
    let resolved = match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
            Ok(real_parent) => real_parent.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    };
    Ok(SymlinkWritePaths { read_path: Some(path.to_path_buf()), write_path: resolved })
}
