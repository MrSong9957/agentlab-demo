// 凭证写盘共享核心：由 lib 单元测试（src/catalog.rs 的 #[cfg(test)] 模块）与
// 集成测试（tests/common/mod.rs）各自 include! 文本引入。Rust 的测试目标边界
// 导致两侧无法互相引用（src 侧 #[cfg(test)] 项对集成测试目标不可见，tests/
// 目录对 lib 单元测试目标不可见），include! 是不改产品代码前提下的单一事实来源。
//
// 宿主作用域须已提供 `fn cred_dir() -> std::path::PathBuf`（返回专用凭证目录），
// 由各侧按自己的 crate 前缀解析（lib 内为 crate::，集成测试为 terminal_chat::）。

/// 在专用凭证目录内写一份凭证文件，返回 file: 引用（相对路径，目录内解析）。
/// 调用前必须已设置 TCHAT_USER_DIR（lib 单元测试经 UserDirGuard、集成测试经
/// set_env），否则 panic，防止写入真实用户环境。
pub fn write_cred_file_shared(tag: &str, key: &str) -> String {
    assert!(
        std::env::var("TCHAT_USER_DIR").is_ok(),
        "写凭证文件前必须设置 TCHAT_USER_DIR：未设置可能写入真实用户环境"
    );
    let dir = cred_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let name = format!("cred-{tag}.json");
    std::fs::write(dir.join(&name), serde_json::json!({ "test_cred": key }).to_string()).unwrap();
    format!("file:{name}#test_cred")
}
