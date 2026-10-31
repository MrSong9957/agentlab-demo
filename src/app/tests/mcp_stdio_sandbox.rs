//! 033：MCP stdio 沙箱模式。**独立测试文件＝独立测试进程**：TCHAT_SANDBOX 是
//! 进程级环境变量，沙箱用例与 mcp_stdio.rs 的普通模式用例必须分进程运行
//!（同进程并发读 env 的污染由进程边界消除）。全程 canary 假值，不触真实凭据。
#![cfg(unix)]
// 038-CI：本文件依赖 POSIX 语义（fd 继承/pre_exec），Windows 不编译；沙箱用例自带可用性分支（无 bwrap＝显式失败断言），macOS 照常运行。

mod common;

use std::sync::atomic::AtomicBool;

use common::mcp_fixtures::{read_record, setup_no_prepare, CANARY_VALUE};

/// 033：沙箱模式下 stdio 服务器经 bwrap 包装启动；mcp.json 显式 env（契约
/// 注入）在沙箱白名单环境之后保留；主进程污染 env 不进入服务器。无 bwrap＝
/// 显式失败且服务器不启动（failIfUnavailable，不静默回退）。
#[test]
fn a033_sandboxed_stdio_keeps_explicit_env_or_fails_closed() {
    let (_env, _dir, record_dir, record_path, manager) =
        setup_no_prepare("echo", r#", "env_vars": ["TCHAT_MCP_TOKEN_A"]"#);
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(record_dir.path(), std::fs::Permissions::from_mode(0o777))
            .expect("放开 record 目录");
    }
    std::env::set_var("TCHAT_SANDBOX", "1");
    std::env::set_var("TCHAT_POISON_KEY", "poison-033");

    let prepared = manager.prepare_for_turn(&AtomicBool::new(false));
    let snapshot = match prepared {
        Some(s) => s,
        None => {
            // 无 bwrap 且管理器报告失败：显式失败，服务器不得启动
            //（record 零内容＝未回退证据；错误文案断言见 exec_sandbox 用例）。
            manager.close();
            std::env::remove_var("TCHAT_SANDBOX");
            std::env::remove_var("TCHAT_POISON_KEY");
            assert!(
                read_record(&record_path).is_empty(),
                "fail-closed 时不得启动服务器"
            );
            return;
        }
    };
    let entry = match snapshot.entry("mcp__fx__echo") {
        Some(e) => e,
        None => {
            // 启动失败在 022 容错语义下表现为空快照：仅当沙箱确实不可用时
            // 接受（服务器未执行＝未回退证据）；有 bwrap 却无工具＝真回归。
            manager.close();
            let sandbox_ok = terminal_chat::sandbox::probe_available();
            std::env::remove_var("TCHAT_SANDBOX");
            std::env::remove_var("TCHAT_POISON_KEY");
            assert!(
                !sandbox_ok,
                "有 bwrap 却未发现工具＝真实回归（非沙箱不可用）"
            );
            assert!(
                read_record(&record_path).is_empty(),
                "沙箱不可用时服务器不得执行（不静默回退）"
            );
            return;
        }
    };
    let (result, failed) =
        manager.call(&entry, serde_json::json!({"text": "iso"}), &AtomicBool::new(false));
    manager.close();
    std::env::remove_var("TCHAT_SANDBOX");
    std::env::remove_var("TCHAT_POISON_KEY");
    assert!(!failed, "沙箱下工具调用失败：{result}");
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["content"][0]["text"], "echo:iso");

    let startup = read_record(&record_path)
        .into_iter()
        .find(|e| e["kind"] == "startup")
        .expect("服务器 startup 记录");
    let env = startup["env"].as_object().expect("env 快照");
    assert_eq!(
        env.get("TCHAT_MCP_TOKEN_A").and_then(|v| v.as_str()),
        Some(CANARY_VALUE),
        "显式 env 注入在沙箱下保留"
    );
    // 沙箱白名单环境在 MCP 面同样生效（env_clear 后注入——033 审查轮修正的
    // 回归断言：HOME 应为策略锚定根＝crate root（主进程 cwd），而非主进程 HOME）。
    assert_eq!(
        env.get("HOME").and_then(|v| v.as_str()),
        Some(env!("CARGO_MANIFEST_DIR")),
        "沙箱白名单 HOME＝策略锚定根（crate root）"
    );
    assert!(
        !env.contains_key("TCHAT_POISON_KEY"),
        "主进程污染 env 不得进入服务器"
    );
}
