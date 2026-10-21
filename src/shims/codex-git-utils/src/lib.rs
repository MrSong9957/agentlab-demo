//! codex-git-utils shim（008 §3-J5）：get_git_repo_root（diff 展示用）。
pub fn get_git_repo_root(p: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = Some(p);
    while let Some(dir) = cur {
        if dir.join(".git").exists() { return Some(dir.to_path_buf()); }
        cur = dir.parent();
    }
    None
}
