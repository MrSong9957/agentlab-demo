//! 022 L2：真实 rmcp 客户端＋受控本地 stdio 子进程。
//!
//! 覆盖 A03（直接参数启动／白名单环境／连接复用）、A05（版本／能力／分页
//! 边界）、A06（目录命名）、A07（名单语义）、A08（Schema／反向请求）、
//! A11（取消／超时／迟到结果）、A12（断连副作用保留）、A13（目录变化
//! 通知）、A15（8 MiB 行界）、A16（污染定义／结果／stderr 的拒绝）的
//! 传输层部分。夹具：固定仓库测试程序 tests/fixtures/mcp_stdio_server.py
//! （python3，本地受控、无网络）；凭证均为测试 canary（TCHAT_MCP_TOKEN_A）。

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::{TempDir, ENV_LOCK};

use common::mcp_fixtures::{
    definition_names, read_record, setup, setup_no_prepare, setup_with, EnvGuard, TestCtx,
    CANARY, CANARY_VALUE, FIXTURE,
};
use terminal_chat::mcp::manager::McpManager;

// ---------- A03：直接参数启动、白名单环境、连接复用 ----------

#[test]
fn a03_stdio_invocation_record_and_connection_reuse() {
    let ctx = setup("echo", r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#);
    assert_eq!(ctx.snapshot.len(), 1);
    assert_eq!(
        ctx.snapshot.definitions()[0]["function"]["name"],
        "mcp__fx__echo"
    );

    let entry = ctx.snapshot.entry("mcp__fx__echo").expect("快照含 echo");
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"text": "你好"}),
        &AtomicBool::new(false),
    );
    assert!(!failed);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["content"][0]["text"], "echo:你好");

    // 第二轮：复用连接（仍是一次 startup、两次调用）。
    let snapshot2 = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("第二轮准备");
    assert_eq!(snapshot2.len(), 1);
    let entry2 = snapshot2.entry("mcp__fx__echo").unwrap();
    let _ = ctx
        .manager
        .call(&entry2, serde_json::json!({"text": "2"}), &AtomicBool::new(false));

    ctx.close();
    let record = read_record(&ctx.record_path);
    assert_eq!(
        record.iter().filter(|e| e["kind"] == "startup").count(),
        1,
        "连接跨轮复用（一次启动）"
    );
    let calls: Vec<&serde_json::Value> =
        record.iter().filter(|e| e["kind"] == "call").collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["name"], "echo", "按原始名调用");
    assert_eq!(calls[0]["arguments"]["text"], "你好");
}

#[test]
fn a03_env_whitelist_explicit_vars_pass_others_blocked() {
    let ctx = setup("echo", r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#);
    std::env::set_var("TCHAT_POISON_KEY", "poison-value-022");
    let record = read_record(&ctx.record_path);
    let startup = record
        .iter()
        .find(|e| e["kind"] == "startup")
        .expect("有 startup 记录");
    let env = startup["env"].as_object().unwrap();
    assert_eq!(
        env.get(CANARY).map(|v| v.as_str()),
        Some(Some(CANARY_VALUE)),
        "显式引用的变量转交"
    );
    assert!(
        !env.contains_key("TCHAT_POISON_KEY"),
        "未引用的宿主变量不转交"
    );
    assert!(env.contains_key("PATH"), "平台白名单基础变量转交");
    let argv = startup["argv"].as_array().unwrap();
    assert!(argv.len() >= 4, "直接参数数组启动（无隐式 shell）：{argv:?}");
    assert_eq!(
        argv.iter().filter(|a| a.as_str() == Some("/bin/sh")).count(),
        0,
        "不经 shell"
    );
    std::env::remove_var("TCHAT_POISON_KEY");
}

#[test]
fn a03_shell_metacharacters_passed_verbatim_no_shell() {
    // 攻击面：模型或配置中的 shell 元字符不得被解释——直接 argv 传递。
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-stdio");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    let record_dir = TempDir::new("mcp-record");
    let record_path = record_dir.path().join("record.jsonl");
    let fixture = FIXTURE.replace('\\', "/");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "echo", "--record", "{}", "; rm -rf /tmp/x", "`id`", "$(whoami)"], "cwd": "{}"}}}}}}"#,
        record_path.display().to_string().replace('\\', "/"),
        std::env::temp_dir().display()
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = Arc::new(McpManager::new());
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    let entry = snapshot.entry("mcp__fx__echo").expect("目录正常发布");
    let (result, failed) = manager.call(
        entry,
        serde_json::json!({"text": "x"}),
        &AtomicBool::new(false),
    );
    assert!(!failed, "{result}");
    manager.close();
    let record = read_record(&record_path);
    let startup = record.iter().find(|e| e["kind"] == "startup").unwrap();
    let argv = startup["argv"].as_array().unwrap();
    let argv_str: Vec<&str> = argv.iter().map(|a| a.as_str().unwrap_or("")).collect();
    assert!(
        argv_str.contains(&"; rm -rf /tmp/x"),
        "元字符按字面参数传递：{argv:?}"
    );
    assert!(argv_str.contains(&"`id`"));
    assert!(argv_str.contains(&"$(whoami)"));
    assert_eq!(
        argv.iter().filter(|a| a.as_str() == Some("/bin/sh")).count(),
        0,
        "零 shell"
    );
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var(CANARY);
    drop(guard);
}

#[test]
fn a03_missing_env_var_fails_server_only() {
    std::env::remove_var("TCHAT_MCP_MISSING_VAR");
    let (_env, _dir, _record_dir, record_path, manager) =
        setup_no_prepare("echo", r#", "env_vars": ["TCHAT_MCP_MISSING_VAR"]"#);
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    assert_eq!(snapshot.len(), 0, "缺变量服务器不可用");
    assert!(
        manager
            .status_lines()
            .iter()
            .any(|l| l.contains("环境变量缺失")),
        "状态含脱敏错误"
    );
    manager.close();
    let record = read_record(&record_path);
    assert!(record.iter().all(|e| e["kind"] != "call"), "零调用");
}

// ---------- A05：版本／能力／分页边界 ----------

#[test]
fn a05_empty_catalog_is_ready_with_zero_tools() {
    let ctx = setup("empty_catalog", "");
    assert_eq!(ctx.snapshot.len(), 0, "声明 tools 能力＋空目录＝就绪、零工具");
    ctx.close();
}

#[test]
fn a05_no_tools_capability_fails() {
    let ctx = setup("no_tools_capability", "");
    assert_eq!(ctx.snapshot.len(), 0);
    assert!(ctx
        .manager
        .status_lines()
        .iter()
        .any(|l| l.contains("未声明 tools 能力")));
    ctx.close();
}

#[test]
fn a05_incompatible_protocol_version_fails() {
    let ctx = setup("init_no_initialize", "");
    assert_eq!(ctx.snapshot.len(), 0);
    assert!(ctx
        .manager
        .status_lines()
        .iter()
        .any(|l| l.contains("不兼容协议版本")));
    ctx.close();
}

#[test]
fn a05_multi_page_listing_completes() {
    let ctx = setup("paged2", "");
    assert_eq!(
        definition_names(&ctx.snapshot),
        vec!["mcp__fx__echo", "mcp__fx__second"]
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert_eq!(
        record.iter().filter(|e| e["kind"] == "list").count(),
        2,
        "两页目录"
    );
}

#[test]
fn a05_empty_cursor_page_is_a_valid_cursor() {
    let ctx = setup("empty_cursor_page", "");
    assert_eq!(ctx.snapshot.len(), 2, "空字符串游标继续翻页");
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert_eq!(record.iter().filter(|e| e["kind"] == "list").count(), 2);
}

#[test]
fn a05_pagination_over_100_pages_fails_without_partial_publish() {
    let ctx = setup("paged101", "");
    assert_eq!(ctx.snapshot.len(), 0, "失败不发布半份目录");
    assert!(ctx
        .manager
        .status_lines()
        .iter()
        .any(|l| l.contains("分页上限")));
    ctx.close();
}

#[test]
fn a05_catalog_item_limit_2049_fails() {
    let ctx = setup("items_over", "");
    assert_eq!(ctx.snapshot.len(), 0);
    assert!(ctx
        .manager
        .status_lines()
        .iter()
        .any(|l| l.contains("2048")));
    ctx.close();
}

#[test]
fn a05_repeated_cursor_fails() {
    let ctx = setup("cursor_loop", "");
    assert_eq!(ctx.snapshot.len(), 0);
    assert!(
        ctx.manager
            .status_lines()
            .iter()
            .any(|l| l.contains("重复游标"))
    );
    ctx.close();
}

#[test]
fn a05_cursor_over_64k_fails() {
    let ctx = setup("cursor_64k", "");
    assert_eq!(ctx.snapshot.len(), 0);
    assert!(ctx
        .manager
        .status_lines()
        .iter()
        .any(|l| l.contains("游标超过")));
    ctx.close();
}

#[test]
fn a05_startup_timeout_fails_within_bound() {
    let ctx = setup("hang", r#", "startup_timeout_sec": 1"#);
    let snapshot = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(snapshot.len(), 0);
    assert!(
        ctx.manager
            .status_lines()
            .iter()
            .any(|l| l.contains("总时限")),
        "启动总时限失败可见：{:?}",
        ctx.manager.status_lines()
    );
    ctx.close();
}

// ---------- A06／A07／A08：命名、名单与 Schema ----------

#[test]
fn a06_unicode_and_long_tool_names_get_valid_model_names() {
    let ctx = setup("unicode_tools", "");
    let names = definition_names(&ctx.snapshot);
    assert_eq!(names.len(), 3);
    for n in &names {
        assert!(n.starts_with("mcp__fx__"), "统一前缀＋服务器 ID：{n}");
        assert!(n.len() <= 64);
        assert!(
            n.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "最终名符合 CC 字符集：{n}"
        );
    }
    // 原始名路由：调用任一工具都按原始名到达服务器。
    let entry = ctx.snapshot.entry(&names[0]).unwrap();
    let (result, failed) = ctx
        .manager
        .call(entry, serde_json::json!({}), &AtomicBool::new(false));
    assert!(!failed);
    assert!(result.contains("echo:"), "夹具按原始名回显：{result}");
    ctx.close();
    let record = read_record(&ctx.record_path);
    let call = record.iter().find(|e| e["kind"] == "call").unwrap();
    let raw = call["name"].as_str().unwrap_or("");
    assert!(
        raw == "查询" || raw == "a__b" || raw == "x".repeat(70),
        "原始名送达：{call}"
    );
}

#[test]
fn a06_duplicate_raw_names_fail_whole_server() {
    let ctx = setup("same_name_tools", "");
    assert_eq!(ctx.snapshot.len(), 0, "重复原始名＝服务目录错误，不覆盖");
    assert!(ctx
        .manager
        .status_lines()
        .iter()
        .any(|l| l.contains("重复原始名")));
    ctx.close();
}

#[test]
fn a08_bad_schema_tools_excluded_good_ones_kept() {
    let ctx = setup("bad_schema_tool", "");
    assert_eq!(ctx.snapshot.len(), 1, "仅 ok 工具发布");
    assert_eq!(
        ctx.snapshot.definitions()[0]["function"]["name"],
        "mcp__fx__ok"
    );
    let lines = ctx.manager.status_lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("排除") && l.contains("root_string")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("排除") && l.contains("props_array")),
        "{lines:?}"
    );
    ctx.close();
}

#[test]
fn a08_reverse_requests_get_method_not_found() {
    for scenario in ["reverse_sampling", "reverse_roots", "reverse_elicitation"] {
        let ctx = setup(scenario, "");
        assert_eq!(ctx.snapshot.len(), 1, "{scenario} 目录正常");
        let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
        let (result, failed) = ctx.manager.call(
            &entry,
            serde_json::json!({"text": "x"}),
            &AtomicBool::new(false),
        );
        assert!(!failed, "{scenario} 反向请求不使调用失败：{result}");
        assert!(result.contains("\"done\""), "{scenario} 反向请求被拒后调用正常完成：{result}");
        ctx.close();
    }
}

#[test]
fn a07_allowlist_filters_publish_and_unknown_entries_reported() {
    let ctx = setup(
        "paged2",
        r#", "enabled_tools": ["echo", "ghost"], "disabled_tools": []"#,
    );
    assert_eq!(ctx.snapshot.len(), 1, "允许名单筛选：仅 echo 发布");
    assert_eq!(
        ctx.snapshot.definitions()[0]["function"]["name"],
        "mcp__fx__echo"
    );
    assert!(
        ctx.manager
            .status_lines()
            .iter()
            .any(|l| l.contains("ghost")),
        "未知允许项提示"
    );
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record
            .iter()
            .filter(|e| e["kind"] == "call")
            .all(|e| e["name"] == "echo"),
        "零越名单调用"
    );
}

// ---------- A11：超时与取消 ----------

#[test]
fn a11_tool_timeout_returns_unknown_state_and_keeps_connection() {
    let ctx = setup_with("echo", r#", "tool_timeout_sec": 1"#, 2000);
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"text": "slow"}),
        &AtomicBool::new(false),
    );
    assert!(failed);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["isError"], true);
    let text = v["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("MCP 客户端错误"), "标准失败形态：{text}");
    assert!(text.contains("状态未知"), "超时＝远端状态未知：{text}");
    // 健康连接保留：下一轮仍发布（超时不关闭连接）。
    let snapshot2 = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(snapshot2.len(), 1, "超时不关闭连接");
    ctx.close();
}

#[test]
fn a11_cancel_during_call_marks_unknown_and_drops_late_result() {
    let ctx = setup_with("echo", "", 3000);
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            cancel.store(true, Ordering::SeqCst);
        })
    };
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"text": "cancelled"}),
        &cancel,
    );
    cancel_thread.join().unwrap();
    assert!(failed);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    let text = v["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("取消") && text.contains("状态未知"),
        "取消＝已发未知：{text}"
    );
    assert!(!result.contains("echo:cancelled"), "迟到结果不投影");
    // 远端延迟后仍会写结果（本地不声称已停止）；记录恰一次调用，无重试。
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert_eq!(
        record.iter().filter(|e| e["kind"] == "call").count(),
        1,
        "一次调用，不自动重试"
    );
}

// ---------- A12：断连（副作用保留、无自动重放） ----------

#[test]
fn a12_server_death_after_side_effect_marks_unknown_once() {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-stdio");
    let side_effect_dir = TempDir::new("mcp-side-effect");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    let record_dir = TempDir::new("mcp-record");
    let record_path = record_dir.path().join("record.jsonl");
    let target = side_effect_dir.path().join("written.txt");
    let fixture = FIXTURE.replace('\\', "/");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "write_then_die", "--record", "{}"], "cwd": "{}"}}}}}}"#,
        record_path.display().to_string().replace('\\', "/"),
        std::env::temp_dir().display()
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .unwrap();
    let entry = snapshot.entry("mcp__fx__echo").unwrap();
    let (result, failed) = manager.call(
        entry,
        serde_json::json!({"path": target.to_string_lossy(), "text": "PAYLOAD"}),
        &AtomicBool::new(false),
    );
    // 响应先于进程退出到达：首个调用成功且副作用保留。
    assert!(!failed, "首个调用正常完成：{result}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap_or_default(),
        "PAYLOAD",
        "首个副作用保留"
    );
    // 进程已退出：下一次调用报错（状态未知），无自动重放、无自动重连。
    let (result2, failed2) = manager.call(
        entry,
        serde_json::json!({"path": target.to_string_lossy(), "text": "SECOND"}),
        &AtomicBool::new(false),
    );
    assert!(failed2, "断连后调用失败");
    let text2 = serde_json::from_str::<serde_json::Value>(&result2).unwrap()["content"][0]
        ["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        text2.contains("MCP 客户端错误"),
        "断连按标准失败形态：{text2}"
    );
    // 进程死亡落在「未发出／已发出」边界：两种分类都符合 §2.4（区分即可，
    // 不强制其一——分类取决于 send 与服务循环退出的相对时序）。
    assert!(
        text2.contains("状态未知") || text2.contains("未发送"),
        "分类必为二者之一：{text2}"
    );
    // 无重放副作用：进程已死，SECOND 不可能被写入。
    assert_ne!(
        std::fs::read_to_string(&target).unwrap_or_default(),
        "SECOND",
        "无第二次副作用（无重放）"
    );
    // 断连后：下一轮不声明工具。
    let snapshot2 = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(snapshot2.len(), 0, "断连连接不再发布工具");
    assert!(manager.status_lines().iter().any(|l| l.contains("不可用")));
    manager.close();
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var(CANARY);
    drop(guard);
}

// ---------- A15：stdio 8 MiB 行界 ----------

#[test]
fn a15_oversize_line_closes_connection_without_success() {
    let ctx = setup("oversize_line", "");
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"text": "x"}),
        &AtomicBool::new(false),
    );
    assert!(failed);
    assert!(
        !result.contains("xxxxxxxx"),
        "超限结果不截断成合法成功：{} 字节",
        result.len()
    );
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["isError"], true);
    // 连接超限关闭：下一轮不发布。
    let snapshot2 = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(snapshot2.len(), 0);
    ctx.close();
}

#[test]
fn a15_unparseable_line_fails_discovery_or_call() {
    let ctx = setup("bad_json", "");
    if ctx.snapshot.is_empty() {
        // 坏行在发现阶段到达：发现失败，服务器不可用。
        assert!(
            ctx.manager
                .status_lines()
                .iter()
                .any(|l| l.contains("不可用"))
        );
    } else {
        let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
        let (result, failed) = ctx.manager.call(
            &entry,
            serde_json::json!({"text": "x"}),
            &AtomicBool::new(false),
        );
        assert!(failed);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result).unwrap()["isError"],
            true
        );
        let snapshot2 = ctx
            .manager
            .prepare_for_turn(&AtomicBool::new(false))
            .unwrap();
        assert_eq!(snapshot2.len(), 0, "坏行后连接关闭");
    }
    ctx.close();
}

// ---------- A16：污染定义／结果／stderr（stdio 传输部分） ----------

#[test]
fn a16_polluted_tool_definition_is_not_published() {
    let ctx = setup(
        "polluted_definition",
        r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#,
    );
    assert_eq!(ctx.snapshot.len(), 1, "仅 clean 工具发布");
    assert_eq!(
        ctx.snapshot.definitions()[0]["function"]["name"],
        "mcp__fx__clean"
    );
    let defs = serde_json::to_string(ctx.snapshot.definitions()).unwrap();
    assert!(!defs.contains(CANARY_VALUE), "污染定义不发布：{defs}");
    assert!(
        ctx.manager
            .status_lines()
            .iter()
            .any(|l| l.contains("排除") && l.contains("敏感数据")),
        "通用原因提示"
    );
    ctx.close();
}

#[test]
fn a16_polluted_result_replaced_with_client_error() {
    let ctx = setup(
        "polluted_result",
        r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#,
    );
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"text": "x"}),
        &AtomicBool::new(false),
    );
    assert!(failed);
    assert!(!result.contains(CANARY_VALUE), "污染结果不入投影：{result}");
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["isError"], true);
    assert!(
        v["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("MCP 客户端错误"),
        "标准客户端错误：{v}"
    );
    ctx.close();
}

#[test]
fn a16_stderr_tail_is_sanitized_in_status() {
    let ctx = setup(
        "stderr_secret",
        r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#,
    );
    assert_eq!(ctx.snapshot.len(), 1, "stderr 排空不阻断正常服务");
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry,
        serde_json::json!({"text": "ok"}),
        &AtomicBool::new(false),
    );
    assert!(!failed, "stderr 持续输出不干扰调用：{result}");
    let lines = ctx.manager.status_lines();
    let stderr_lines: Vec<&String> =
        lines.iter().filter(|l| l.contains("stderr：")).collect();
    assert!(!stderr_lines.is_empty(), "状态含 stderr 尾部：{lines:?}");
    for l in stderr_lines {
        assert!(!l.contains(CANARY_VALUE), "stderr 脱敏：{l}");
    }
    ctx.close();
}

// ---------- A13（stdio 部分）：目录变化通知 → 下一轮重列 ----------

#[test]
fn a13_list_changed_relists_on_next_turn() {
    let ctx = setup("notify_list_changed", "");
    assert_eq!(ctx.snapshot.len(), 1);
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    // 调用触发 notifications/tools/list_changed。
    let _ = ctx.manager.call(
        &entry,
        serde_json::json!({"text": "n"}),
        &AtomicBool::new(false),
    );
    let lists_before = read_record(&ctx.record_path)
        .iter()
        .filter(|e| e["kind"] == "list")
        .count();
    // 下一轮：完整重列（记录里的 list 次数增加），连接复用不重建。
    let snapshot2 = ctx
        .manager
        .prepare_for_turn(&AtomicBool::new(false))
        .unwrap();
    assert_eq!(snapshot2.len(), 1);
    let entry2 = snapshot2.entry("mcp__fx__echo").unwrap();
    let (result, failed) = ctx.manager.call(
        &entry2,
        serde_json::json!({"text": "n2"}),
        &AtomicBool::new(false),
    );
    assert!(!failed, "重列后调用可用：{result}");
    assert!(result.contains("\"ok\""));
    ctx.close();
    let record = read_record(&ctx.record_path);
    assert!(
        record.iter().filter(|e| e["kind"] == "list").count() > lists_before,
        "通知后下一轮完整重列"
    );
    assert_eq!(
        record.iter().filter(|e| e["kind"] == "startup").count(),
        1,
        "重列不重建连接"
    );
}

// ---------- A05 补充：prepare 取消零模型请求、候选回收 ----------

#[test]
fn a05_prepare_cancelled_returns_none_and_closes_candidates() {
    let (_env, _dir, _record_dir, record_path, manager) = setup_no_prepare("echo", "");
    let cancel = Arc::new(AtomicBool::new(false));
    cancel.store(true, Ordering::SeqCst);
    let snapshot = manager.prepare_for_turn(&cancel);
    assert!(snapshot.is_none(), "取消＝None（宿主不发模型请求）");
    manager.close();
    let record = read_record(&record_path);
    assert!(
        record.iter().all(|e| e["kind"] != "call"),
        "取消后零调用"
    );
}

// ---------- A06 补充：跨服务器同名 → 稳定短哈希消解（全局唯一） ----------

#[test]
fn a06_cross_server_same_tool_names_resolved_with_hash() {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-stdio");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    let record_dir = TempDir::new("mcp-record");
    let record_path = record_dir.path().join("record.jsonl");
    let fixture = FIXTURE.replace('\\', "/");
    // 两台服务器目录同名（echo）；第二台记录到同一 record（不同 kind 前缀可区分 server）。
    let json = format!(
        r#"{{"mcp_servers": {{"aa": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "echo", "--record", "{rec}"], "cwd": "{cwd}"}}, "bb": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "echo", "--record", "{rec}"], "cwd": "{cwd}"}}}}}}"#,
        rec = record_path.display().to_string().replace('\\', "/"),
        cwd = std::env::temp_dir().display(),
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = Arc::new(McpManager::new());
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不取消");
    assert_eq!(snapshot.len(), 2, "两台服务器各一台 echo 全部发布");
    let names: Vec<String> = (0..snapshot.len())
        .map(|i| {
            snapshot.definitions()[i]["function"]["name"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_ne!(names[0], names[1], "跨服务器同名必须消解：{names:?}");
    for n in &names {
        assert!(n.starts_with("mcp__"), "{n}");
        assert!(n.len() <= 64);
    }
    // 各自路由正确：调用两台都成功，record 中两次原始调用。
    let mut called_servers = 0;
    for name in &names {
        let entry = snapshot.entry(name).unwrap();
        let (result, failed) = manager
            .call(entry, serde_json::json!({"text": "x"}), &AtomicBool::new(false));
        assert!(!failed, "{name}: {result}");
        called_servers += 1;
    }
    assert_eq!(called_servers, 2);
    manager.close();
    let record = read_record(&record_path);
    assert_eq!(record.iter().filter(|e| e["kind"] == "call").count(), 2);
    // 第二台服务器的 argv 含其 ID（通过场景参数顺序无法区分——两次 echo 原始名相同，路由由连接句柄保证）。
    assert_eq!(
        record.iter().filter(|e| e["kind"] == "startup").count(),
        2,
        "两台独立子进程"
    );
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var(CANARY);
    drop(guard);
}

#[test]
fn a13_notification_during_relist_keeps_stale_marker() {
    // A13：重列期间到达的新通知必须保留过期标记——第三轮仍会重列。
    let ctx = setup("notify_twice", "");
    assert_eq!(ctx.snapshot.len(), 1);
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    // 轮 1：调用触发通知 1。
    let _ = ctx.manager.call(&entry, serde_json::json!({"text": "n"}), &AtomicBool::new(false));
    // 轮 2：重列（list 第 2 页时夹具发通知 2）。
    let _ = ctx.manager.prepare_for_turn(&AtomicBool::new(false)).unwrap();
    let lists_two = read_record(&ctx.record_path)
        .iter()
        .filter(|e| e["kind"] == "list")
        .count();
    // 轮 3：通知 2 保留的过期标记生效——继续重列。
    let _ = ctx.manager.prepare_for_turn(&AtomicBool::new(false)).unwrap();
    ctx.close();
    let record = read_record(&ctx.record_path);
    let lists_three = record.iter().filter(|e| e["kind"] == "list").count();
    assert!(
        lists_three > lists_two,
        "重列期间的新通知保留：轮2 列 {lists_two} 次，轮3 列 {lists_three} 次"
    );
}

// ---------- 022 性能 P2（方案 A RED→GREEN）：条件化 settle ----------

/// P2-a：无 MCP 配置时 prepare_for_turn 零等待（当前无条件 50ms settle 必失败）。
/// debug 构建的阈值用宽松上界（50ms 的一半），release 复测在 scripts/perf 里做。
#[test]
fn p2a_prepare_without_mcp_config_has_no_settle_wait() {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-perf-empty");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    let manager = Arc::new(McpManager::new());
    let cancel = AtomicBool::new(false);
    // 首次（加载空配置）＋第二次（稳态）都计时。
    let _ = manager.prepare_for_turn(&cancel).unwrap();
    let t = std::time::Instant::now();
    let snap = manager.prepare_for_turn(&cancel).unwrap();
    let elapsed = t.elapsed();
    assert_eq!(snap.len(), 0);
    assert!(
        elapsed < std::time::Duration::from_millis(25),
        "无 MCP 配置的稳态 prepare 应零等待（P2）：{:?}",
        elapsed
    );
    manager.close();
    std::env::remove_var("TCHAT_USER_DIR");
    drop(guard);
}

/// P2-b：有连接、无近期调用、无通知的稳态 prepare 应跳过 settle。
#[test]
fn p2b_steady_prepare_skips_settle_when_idle() {
    let ctx = setup("echo", "");
    let cancel = AtomicBool::new(false);
    // 连接建立后不发起任何调用（无在途通知可等待）。
    let t = std::time::Instant::now();
    let snap = ctx.manager.prepare_for_turn(&cancel).unwrap();
    let elapsed = t.elapsed();
    assert_eq!(snap.len(), 1);
    assert!(
        elapsed < std::time::Duration::from_millis(25),
        "空闲稳态 prepare 应跳过 settle（P2）：{:?}",
        elapsed
    );
    ctx.close();
}

/// P2 语义保护：无近期调用时收到**主动**通知（服务器自发，非本机调用触发）
/// ——下一轮必须完整重列（条件化 settle 不得丢掉这种通知）。
/// 夹具 notify_on_list：每次 tools/list 后发一次目录变化通知（形成「上一轮
/// 重列后无调用＋服务器主动通知」的组合）。
#[test]
fn p2_active_notification_without_recent_call_still_relists() {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-perf-notify");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    let record_dir = TempDir::new("mcp-record");
    let record_path = record_dir.path().join("record.jsonl");
    let fixture = FIXTURE.replace('\\', "/");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "notify_on_list", "--record", "{}"], "cwd": "{}"}}}}}}"#,
        record_path.display().to_string().replace('\\', "/"),
        std::env::temp_dir().display()
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = Arc::new(McpManager::new());
    let cancel = AtomicBool::new(false);
    // 轮 1：首连（list 触发一次主动通知——发布基准在列出前，本轮目录即过期）。
    let _ = manager.prepare_for_turn(&cancel).unwrap();
    let lists_1 = read_record(&record_path)
        .iter()
        .filter(|e| e["kind"] == "list")
        .count();
    // 受控同步：夹具在每轮 list 响应后 30ms 发通知——轮间停 100ms 确保通知
    // 已由宿主受理（epoch 已增），再进入下一轮（消除「通知在网络在途」与
    // 「宿主已受理」的混淆；受理后的重列才是本用例的判定对象）。
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = manager.prepare_for_turn(&cancel).unwrap();
    let lists_2 = read_record(&record_path)
        .iter()
        .filter(|e| e["kind"] == "list")
        .count();
    assert!(
        lists_2 > lists_1,
        "无近期调用时的主动通知必须触发下一轮重列：轮1={lists_1} 轮2={lists_2}"
    );
    // 轮 3：继续重列（每轮 list 都再发通知，链式过期持续）。
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = manager.prepare_for_turn(&cancel).unwrap();
    let lists_3 = read_record(&record_path)
        .iter()
        .filter(|e| e["kind"] == "list")
        .count();
    assert!(lists_3 > lists_2);
    manager.close();
    std::env::remove_var("TCHAT_USER_DIR");
    std::env::remove_var(CANARY);
    drop(guard);
}

// ---------- a12 分类确定性补充：发送前确定断连 → 未发出 ----------

/// 类1（确定性，非竞态）：连接已显式关闭（service 已回收）后经旧快照调用——
/// 请求**未发出**（NotSent），服务器调用计数为零；与 a11/a18 的「已发出状态
/// 未知」（gate 确认在途）构成两类的确定性对照。a12 原用例的进程死亡边界为
/// 竞态场景（二选一），不与本用例重复。
#[test]
fn a12_call_after_close_is_not_sent_with_zero_server_calls() {
    let ctx = setup("echo", "");
    let entry = ctx.snapshot.entry("mcp__fx__echo").unwrap();
    // 显式关停（生产入口）：连接回收。
    ctx.manager.close();
    // 经旧快照调用：发送前即失败。
    let (result, failed) = ctx.manager.call(
        entry,
        serde_json::json!({"text": "after-close"}),
        &AtomicBool::new(false),
    );
    assert!(failed);
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["isError"], true);
    let text = v["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("未发送") && text.contains("MCP 客户端错误"),
        "发送前确定断连＝未发出：{text}"
    );
    // 服务器零该次调用（write_before 任何 record 调用计数）。
    let record = read_record(&ctx.record_path);
    let calls = record
        .iter()
        .filter(|e| e["kind"] == "call" && e["arguments"]["text"] == "after-close")
        .count();
    assert_eq!(calls, 0, "服务器该次调用计数为零（请求未到达）");
}
