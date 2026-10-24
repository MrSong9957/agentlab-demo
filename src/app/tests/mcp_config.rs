//! 022 A01／A02 L1 验收：mcp.json 严格解析。
//!
//! A01：缺文件、空集合、默认／显式关闭 → 原工具面保持，MCP 不可用／未加载。
//! A02：非法／重复配置键、传输混用、cwd／URL／header／超时错误、缺凭证 →
//!      具体失败；停用条目不解析凭证（本层只存环境变量名，值读取在连接准备层）。
//!
//! 本层不启动进程、不发网络请求；临时目录覆盖 TCHAT_USER_DIR。

mod common;

use common::{TempDir, ENV_LOCK};
use terminal_chat::mcp::config::{
    load_mcp_file, parse_mcp_file, McpTransport, DEFAULT_STARTUP_TIMEOUT_SEC,
    DEFAULT_TOOL_TIMEOUT_SEC,
};

fn user_dir_with(content: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new("mcp-config");
    let path = dir.path().join("mcp.json");
    std::fs::write(&path, content).expect("写入 mcp.json");
    (dir, path.parent().expect("父目录").to_path_buf())
}

fn parse_ok(content: &str) -> terminal_chat::mcp::config::McpFile {
    parse_mcp_file(content).expect("应解析成功")
}

fn parse_ok_str(content: String) -> terminal_chat::mcp::config::McpFile {
    parse_mcp_file(&content).expect("应解析成功")
}

fn parse_err(content: &str) -> String {
    parse_mcp_file(content).expect_err("应解析失败").0
}

// ---------- A01：缺文件／空集合／关闭 ----------

#[test]
fn a01_missing_file_means_no_mcp_and_no_file_creation() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new("mcp-config");
    let file = dir.path().join("mcp.json");
    let loaded = load_mcp_file(dir.path()).expect("缺失文件＝无配置");
    assert!(loaded.servers.is_empty());
    assert!(!file.exists(), "不得创建 mcp.json");
}

#[test]
fn a01_empty_servers_and_disabled_entries_load_without_tools() {
    let file = parse_ok(r#"{"mcp_servers": {}}"#);
    assert!(file.servers.is_empty());

    let file = parse_ok(
        r#"{"mcp_servers": {"off": {"enabled": false, "command": "/bin/true"}}}"#,
    );
    assert_eq!(file.servers.len(), 1);
    assert!(!file.servers[0].1.enabled, "缺省 enabled=false");

    // enabled=true 但结构错误仍拒绝（command 与 url 同给）
    assert!(parse_mcp_file(
        r#"{"mcp_servers": {"off": {"enabled": true, "command": "/bin/true", "url": "http://127.0.0.1:1/mcp"}}}"#
    )
    .is_err());
}

// ---------- A02：非法结构矩阵 ----------

#[test]
fn a02_top_level_unknown_field_rejected() {
    let e = parse_err(r#"{"mcp_servers": {}, "servers": {}}"#);
    assert!(e.contains("未知配置字段"), "实际：{e}");
}

#[test]
fn a02_duplicate_json_keys_rejected_at_every_level() {
    // 顶层重复
    let e = parse_err(r#"{"mcp_servers": {}, "mcp_servers": {}}"#);
    assert!(e.contains("重复"), "实际：{e}");
    // 服务器 ID 重复
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "/bin/true"}, "a": {"command": "/bin/true"}}}"#,
    );
    assert!(e.contains("重复"), "实际：{e}");
    // 服务器字段重复：不用后一个覆盖前一个，直接拒绝
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "/bin/true", "command": "/bin/false"}}}"#,
    );
    assert!(e.contains("重复配置字段"), "实际：{e}");
}

#[test]
fn a02_unknown_server_field_rejected() {
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "/bin/true", "env": {"K": "V"}}}}"#,
    );
    assert!(e.contains("未知配置字段") && e.contains("env"), "实际：{e}");
}

#[test]
fn a02_blank_server_id_rejected() {
    let e = parse_err(r#"{"mcp_servers": {"  ": {"command": "/bin/true"}}}"#);
    assert!(e.contains("空白服务器 ID"), "实际：{e}");
}

#[test]
fn a02_command_and_url_mutually_exclusive() {
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "/bin/true", "url": "http://127.0.0.1:9/mcp"}}}"#,
    );
    assert!(e.contains("只能选择一个"), "实际：{e}");

    let e = parse_err(r#"{"mcp_servers": {"a": {"enabled": true}}}"#);
    assert!(e.contains("缺少 command"), "实际：{e}");
}

#[test]
fn a02_cross_transport_fields_rejected() {
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "/bin/true", "bearer_token_env_var": "T"}}}"#,
    );
    assert!(e.contains("不匹配"), "实际：{e}");
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"url": "http://127.0.0.1:9/mcp", "env_vars": ["A"]}}}"#,
    );
    assert!(e.contains("不匹配"), "实际：{e}");
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"url": "http://127.0.0.1:9/mcp", "cwd": "/tmp"}}}"#,
    );
    assert!(e.contains("不匹配"), "实际：{e}");
}

#[test]
fn a02_relative_command_with_path_rejected_bare_name_and_absolute_allowed() {
    let e = parse_err(r#"{"mcp_servers": {"a": {"command": "sub/prog"}}}"#);
    assert!(e.contains("绝对路径"), "实际：{e}");

    let file = parse_ok(r#"{"mcp_servers": {"a": {"enabled": true, "command": "prog"}}}"#);
    match &file.servers[0].1.transport {
        McpTransport::Stdio(s) => {
            assert_eq!(s.command, "prog");
            assert!(s.args.is_empty());
            assert!(s.env_vars.is_empty());
            assert!(s.cwd.is_absolute(), "缺省 cwd＝宿主启动目录");
        }
        other => panic!("应为 stdio：{other:?}"),
    }

    let file = parse_ok(r#"{"mcp_servers": {"a": {"enabled": true, "command": "/bin/true"}}}"#);
    match &file.servers[0].1.transport {
        McpTransport::Stdio(s) => assert_eq!(s.command, "/bin/true"),
        other => panic!("应为 stdio：{other:?}"),
    }
}

#[test]
fn a02_cwd_must_be_existing_absolute_directory() {
    let e = parse_err(r#"{"mcp_servers": {"a": {"command": "p", "cwd": "rel/dir"}}}"#);
    assert!(e.contains("绝对路径"), "实际：{e}");

    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "p", "cwd": "/nonexistent-tchat-mcp-cwd-0x1"}}}"#,
    );
    assert!(e.contains("已存在的目录"), "实际：{e}");

    let ok = parse_ok_str(format!(
        r#"{{"mcp_servers": {{"a": {{"command": "p", "cwd": "{}"}}}}}}"#,
        std::env::temp_dir().display()
    ))
    .servers;
    match &ok[0].1.transport {
        McpTransport::Stdio(s) => assert_eq!(s.cwd, std::env::temp_dir()),
        other => panic!("应为 stdio：{other:?}"),
    }
}

#[test]
fn a02_url_validation_scheme_loopback_userinfo_fragment() {
    // 非 loopback HTTP 拒绝
    let e = parse_err(r#"{"mcp_servers": {"a": {"url": "http://mcp.example.com/mcp"}}}"#);
    assert!(e.contains("HTTPS"), "实际：{e}");
    // loopback HTTP 允许
    parse_ok(r#"{"mcp_servers": {"a": {"enabled": true, "url": "http://127.0.0.1:8000/mcp"}}}"#);
    parse_ok(r#"{"mcp_servers": {"a": {"enabled": true, "url": "http://localhost:8000/mcp"}}}"#);
    parse_ok(r#"{"mcp_servers": {"a": {"enabled": true, "url": "http://[::1]:8000/mcp"}}}"#);
    // 其他 scheme 拒绝
    let e = parse_err(r#"{"mcp_servers": {"a": {"url": "ftp://mcp.example.com/mcp"}}}"#);
    assert!(e.contains("http"), "实际：{e}");
    // userinfo 拒绝
    let e = parse_err(r#"{"mcp_servers": {"a": {"url": "https://user:pw@mcp.example.com/mcp"}}}"#);
    assert!(e.contains("userinfo"), "实际：{e}");
    // fragment 拒绝
    let e = parse_err(r#"{"mcp_servers": {"a": {"url": "https://mcp.example.com/mcp#frag"}}}"#);
    assert!(e.contains("fragment"), "实际：{e}");
}

#[test]
fn a02_http_headers_validation_dedup_reserved_and_authorization() {
    // 保留字头拒绝
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"url": "http://127.0.0.1:9/mcp", "env_http_headers": {"Accept": "A"}}}}"#,
    );
    assert!(e.contains("不允许覆盖"), "实际：{e}");
    // 头名大小写重复拒绝
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"url": "http://127.0.0.1:9/mcp", "env_http_headers": {"X-A": "A", "x-a": "B"}}}}"#,
    );
    assert!(e.contains("大小写重复"), "实际：{e}");
    // Authorization 与 bearer 互斥
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"url": "http://127.0.0.1:9/mcp", "bearer_token_env_var": "T", "env_http_headers": {"Authorization": "T"}}}}"#,
    );
    assert!(e.contains("一个来源"), "实际：{e}");
    // 换行头名拒绝
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"url": "http://127.0.0.1:9/mcp", "env_http_headers": {"X-A\nB": "A"}}}}"#,
    );
    assert!(e.contains("头名非法"), "实际：{e}");
    // 合法头保留声明顺序
    let file = parse_ok(
        r#"{"mcp_servers": {"a": {"enabled": true, "url": "http://127.0.0.1:9/mcp", "env_http_headers": {"X-Tenant-ID": "TENANT", "X-Trace": "TRACE"}, "bearer_token_env_var": "TOKEN"}}}"#,
    );
    match &file.servers[0].1.transport {
        McpTransport::Http(h) => {
            assert_eq!(
                h.env_http_headers,
                vec![
                    ("X-Tenant-ID".to_string(), "TENANT".to_string()),
                    ("X-Trace".to_string(), "TRACE".to_string()),
                ]
            );
            assert_eq!(h.bearer_token_env_var.as_deref(), Some("TOKEN"));
        }
        other => panic!("应为 http：{other:?}"),
    }
}

#[test]
fn a02_timeout_validation_positive_integers_with_defaults() {
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "p", "startup_timeout_sec": 0}}}"#,
    );
    assert!(e.contains("正整数秒"), "实际：{e}");
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "p", "startup_timeout_sec": -1}}}"#,
    );
    assert!(e.contains("正整数秒"), "实际：{e}");
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "p", "tool_timeout_sec": 1.5}}}"#,
    );
    assert!(e.contains("正整数秒"), "实际：{e}");
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "p", "tool_timeout_sec": "30"}}}"#,
    );
    assert!(e.contains("正整数秒"), "实际：{e}");

    let file = parse_ok(
        r#"{"mcp_servers": {"a": {"command": "p", "startup_timeout_sec": 7, "tool_timeout_sec": 11}}}"#,
    );
    let cfg = &file.servers[0].1;
    assert_eq!(cfg.startup_timeout.as_secs(), 7);
    assert_eq!(cfg.tool_timeout.as_secs(), 11);

    let file = parse_ok(r#"{"mcp_servers": {"a": {"command": "p"}}}"#);
    let cfg = &file.servers[0].1;
    assert_eq!(
        cfg.startup_timeout.as_secs(),
        DEFAULT_STARTUP_TIMEOUT_SEC,
        "缺省 30 秒（Codex 基线）"
    );
    assert_eq!(cfg.tool_timeout.as_secs(), DEFAULT_TOOL_TIMEOUT_SEC);
}

#[test]
fn a02_tool_lists_reject_blank_names_and_preserve_empty_allowlist() {
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"command": "p", "enabled_tools": ["x", "  "]}}}"#,
    );
    assert!(e.contains("空白项"), "实际：{e}");

    // 显式空数组＝零工具（与缺省＝全部相区分）
    let file = parse_ok(
        r#"{"mcp_servers": {"a": {"command": "p", "enabled_tools": [], "disabled_tools": ["b"]}}}"#,
    );
    let cfg = &file.servers[0].1;
    assert_eq!(cfg.enabled_tools, Some(Vec::new()));
    assert_eq!(cfg.disabled_tools, vec!["b".to_string()]);
}

#[test]
fn a02_wrong_types_rejected() {
    let e = parse_err(r#"{"mcp_servers": {"a": {"command": 42}}}"#);
    assert!(e.contains("必须是字符串"), "实际：{e}");
    let e = parse_err(r#"{"mcp_servers": {"a": {"command": "p", "args": "x"}}}"#);
    assert!(e.contains("数组"), "实际：{e}");
    let e = parse_err(r#"{"mcp_servers": {"a": {"command": "p", "enabled": "yes"}}}"#);
    assert!(e.contains("布尔"), "实际：{e}");
    let e = parse_err(r#"{"mcp_servers": {"a": [1, 2]}}"#);
    assert!(e.contains("对象"), "实际：{e}");
    let e = parse_err(
        r#"{"mcp_servers": {"a": {"url": "http://127.0.0.1:9/mcp", "env_http_headers": []}}}"#,
    );
    assert!(e.contains("必须是对象"), "实际：{e}");
    let e = parse_err(r#"{"mcp_servers": {"a": {"command": "p", "env_vars": [1]}}}"#);
    assert!(e.contains("字符串"), "实际：{e}");
}

#[test]
fn a02_disabled_entry_still_validated_but_credential_free() {
    // 停用条目结构错误同样拒绝（校验结构）
    let e = parse_err(
        r#"{"mcp_servers": {"off": {"enabled": false, "url": "http://mcp.example.com/mcp"}}}"#,
    );
    assert!(e.contains("HTTPS"), "实际：{e}");
    // 停用条目合法时只保存环境变量名，不读取宿主值（连接准备层职责）
    let file = parse_ok(
        r#"{"mcp_servers": {"off": {"enabled": false, "url": "https://mcp.example.com/mcp", "bearer_token_env_var": "NO_SUCH_VAR_TCHAT_022"}}}"#,
    );
    match &file.servers[0].1.transport {
        McpTransport::Http(h) => {
            assert_eq!(h.bearer_token_env_var.as_deref(), Some("NO_SUCH_VAR_TCHAT_022"));
        }
        other => panic!("应为 http：{other:?}"),
    }
}

#[test]
fn a01_load_from_user_dir_uses_tchat_user_dir_shape() {
    // load_mcp_file 按 user_dir/mcp.json 读取；manager 层传 catalog::user_dir()。
    let (dir_guard, user_dir) = user_dir_with(
        r#"{"mcp_servers": {"docs": {"enabled": true, "command": "/bin/true"}}}"#,
    );
    let file = load_mcp_file(&user_dir).expect("加载成功");
    assert_eq!(file.servers.len(), 1);
    assert_eq!(file.servers[0].0, "docs");
    assert!(file.servers[0].1.enabled);
    drop(dir_guard);
}
