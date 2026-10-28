//! 033 生命周期修复回归（2026-10-28）：沙箱进程绑定短寿轮线程的 PDEATHSIG 缺陷。
//!
//! 缺陷（归因记录 `docs/logs/033-tool-sandbox/2026-10-28-e2e-failure-attribution.md`）：
//! `--die-with-parent` 的 PDEATHSIG 锚定「创建者内核线程」——工具 spawn 此前发生在
//! 随轮退出的 turn 线程内，轮结束即整树 SIGKILL，016「进程跨调用跨轮保留／取消不
//! 终止」在沙箱模式全部失效（n2b/n2b2 部署态失败的根因）。
//!
//! 修复：ProcessManager 专用长寿创建线程执行 fork/exec（PDEATHSIG 锚点＝管理器
//! 生命周期）；本文件按「请求线程退出、应用仍存活」的确定性时序回归。
//!
//! 环境口径：双分支读取 ambient `TCHAT_SANDBOX`（不 set/remove，部署注入态与
//! env -u 全量同一套用例）；用例集需在两种口径下各跑一遍。全部真实 CLI/TUI＋
//! 本地 mock；假凭据；收尾清理本轮创建的进程与目录。

mod common;

use terminal_chat::catalog;

use common::pty::PtyCli;
use common::{spawn_mock, MockResp, TempDir};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;

use std::time::Duration;

fn sandbox_mode() -> bool {
    std::env::var("TCHAT_SANDBOX")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn write_manual(dir: &Path, base_url: &str, key_ref: &str) {
    let m = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(base_url.into()),
                key_ref: Some(key_ref.into()),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    fs::write(dir.join("manual.json"), serde_json::to_string_pretty(&m).unwrap()).unwrap();
}

fn seed_grant(user: &Path, url: &str, key: &str) {
    let origin = catalog::origin_of(url).unwrap();
    let text = json!({ "grants": [{ "origin": origin, "cred_fp": catalog::fingerprint(key), "added_at": 0 }] });
    fs::write(user.join("authorized-origins.json"), text.to_string()).unwrap();
}

/// cmdline 含全部片段的进程 pid 集（僵尸进程 cmdline 为空，天然不计入——
/// 「活进程」与「僵尸/已死」在扫描层面即区分）。
fn scan_procs(frags: &[&str]) -> Vec<u32> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = e.file_name().to_string_lossy().parse::<u32>().ok() else { continue };
        let Ok(cl) = fs::read_to_string(format!("/proc/{pid}/cmdline")) else { continue };
        if !cl.is_empty() && frags.iter().all(|f| cl.contains(f)) {
            out.push(pid);
        }
    }
    out.sort_unstable();
    out
}

/// /proc/{pid} 状态（区分活进程与僵尸：State Z＝已死未收割，非泄漏）。
fn proc_state(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("State:"))
                .map(|l| l.trim().to_string())
        })
}

fn cli_command(cfg: &Path, user: &Path, work: &Path, key_name: &str, key: &str) -> Command {
    let home = user.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.current_dir(work)
        .env("TERM", "xterm")
        .env("TCHAT_CONFIG_DIR", cfg)
        .env("TCHAT_USER_DIR", user)
        .env(key_name, key)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("APPDATA", &home)
        .env("CODEX_HOME", &home)
        .env("TZ", "UTC")
        .env("LC_ALL", "C.UTF-8")
        .env("SHELL", "/bin/sh")
        .env("TCHAT_SESSION_TS", "20261028T100000")
        .env_remove("TCHAT_PUBLIC_CATALOG_URL");
    cmd
}

fn wait_log_growth(log: &Path, base: u64, ms: u64) -> bool {
    let dl = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < dl {
        if fs::metadata(log).map(|m| m.len()).unwrap_or(0) > base {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// 场景 1（管道面）：yield 返回的会话跨轮保留——轮结束后写盘继续、write_stdin
/// 查询可取到新输出、退出清理。修复前沙箱分支在「轮结束」即整树死亡（RED）。
#[test]
fn lifecycle_pipe_session_survives_turn_end_and_stays_queryable() {
    let sandbox = sandbox_mode();
    let cfg = TempDir::new("lc-pipe-cfg");
    let user = TempDir::new("lc-pipe-user");
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let log = work.join("w.log");
    // 扫描标记＝work 目录路径（cmdline 中的日志路径必然包含；TempDir 名含
    // pid+nanos 每测试唯一。先前用 "lc-pipe-{pid}" 不出现在任何 cmdline——
    // 断言恒真空转，2026-10-28 审查发现已修）。
    let marker = format!("{}", work.display());

    let round1 = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": [ { "id": "lc-w", "type": "function", "function": { "name": "exec_command",
            "arguments": json!({ "cmd": format!("while true; do echo tick; echo x >> {}; sleep 0.05; done", log.display()),
                                 "timeout_ms": 30000, "yield_time_ms": 1500 }).to_string() } } ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    // 轮 2：模型经 write_stdin 空查询管理仍运行的会话（跨调用管理面）。
    let round2 = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": [ { "id": "lc-q", "type": "function", "function": { "name": "write_stdin",
            "arguments": json!({ "session_id": "s1", "yield_time_ms": 5000 }).to_string() } } ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let round3 = json!({ "choices": [{ "message": { "content": "LC-PIPE-DONE", "finish_reason": "stop" } }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, round1),
        MockResp::json(200, round2),
        MockResp::json(200, round3),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_LC_KEY");
    seed_grant(user.path(), &mock.url, "fake-lc-key");

    let mut pty = PtyCli::spawn(cli_command(
        cfg.path(), user.path(), &work, "TCHAT_LC_KEY", "fake-lc-key"));
    pty.wait_screen("? for shortcuts");
    pty.send("go");
    pty.wait_screen("while true; do echo tick");
    for _ in 0..100 {
        if fs::metadata(&log).map(|m| m.len()).unwrap_or(0) > 0 { break; }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(fs::metadata(&log).map(|m| m.len()).unwrap_or(0) > 0, "写盘探针已运行");
    // 回合结束（yield 结果回模型、收尾内容渲染）——「请求线程退出、应用仍存活」。
    pty.wait_screen("LC-PIPE-DONE");
    let s1 = fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    assert!(
        wait_log_growth(&log, s1, 1200),
        "[sandbox={sandbox}] 轮结束后写盘必须继续（016 跨轮保留；修复前沙箱树随轮线程退出被 SIGKILL）"
    );

    // 轮 2：write_stdin 查询——继续管理（其结果随第三轮请求回传）。
    let body: Value = serde_json::from_str(&mock.requests()[2].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let q: Value = serde_json::from_str(
        msgs.iter()
            .find(|m| m["tool_call_id"] == json!("lc-q"))
            .expect("第二轮应携带 write_stdin 结果")["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(q["status"], json!("running"), "查询仍运行会话＝running：{q}");
    eprintln!("LC-Q-RESULT: {q}");
    assert!(
        q["output"].as_str().is_some_and(|o| o.contains("tick")),
        "write_stdin 查询应取到跨轮新增输出：{q}"
    );
    pty.wait_screen("LC-PIPE-DONE");

    pty.send("/exit");
    pty.wait("再见!");
    let (_, code) = pty.finish();
    assert_eq!(code, Some(0));
    // 退出清理：写入循环与沙箱包装均无遗留。
    let dl = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < dl && !scan_procs(&[&marker]).is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        scan_procs(&[&marker]).is_empty(),
        "退出后本轮写入进程无遗留（活进程口径，僵尸天然排除）"
    );
    assert!(scan_procs(&["bwrap", &marker]).is_empty(), "退出后无沙箱包装遗留");
}

/// 场景 2（PTY 面）：yield 会话跨轮后仍可写入（write_stdin 输入→GOT 落盘）。
#[test]
fn lifecycle_pty_session_survives_turn_end_and_accepts_input() {
    let sandbox = sandbox_mode();
    let cfg = TempDir::new("lc-pty-cfg");
    let user = TempDir::new("lc-pty-user");
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let out = work.join("out.log");
    let round1 = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": [ { "id": "lc-r", "type": "function", "function": { "name": "exec_command",
            "arguments": json!({ "cmd": format!("while IFS= read -r l; do echo \"GOT:$l\" >> {}; done", out.display()),
                                 "timeout_ms": 30000, "yield_time_ms": 1500, "tty": true }).to_string() } } ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let round2 = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": [ { "id": "lc-i", "type": "function", "function": { "name": "write_stdin",
            "arguments": json!({ "session_id": "s1", "chars": "hello\n", "yield_time_ms": 800 }).to_string() } } ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let round3 = json!({ "choices": [{ "message": { "content": "LC-PTY-DONE", "finish_reason": "stop" } }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, round1),
        MockResp::json(200, round2),
        MockResp::json(200, round3),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_LC2_KEY");
    seed_grant(user.path(), &mock.url, "fake-lc2-key");

    let mut pty = PtyCli::spawn(cli_command(
        cfg.path(), user.path(), &work, "TCHAT_LC2_KEY", "fake-lc2-key"));
    pty.wait_screen("? for shortcuts");
    pty.send("go");
    pty.wait_screen("while IFS= read -r l");
    pty.wait_screen("LC-PTY-DONE"); // 轮结束（yield 会话保留）
    let _ = sandbox;
    pty.wait_screen("LC-PTY-DONE");
    pty.wait_screen("LC-PTY-DONE");
    // 跨轮输入：写入会话的输出文件应出现 GOT:hello。
    let dl = std::time::Instant::now() + Duration::from_secs(5);
    let got = loop {
        if std::time::Instant::now() > dl { break false; }
        if fs::read_to_string(&out).map(|s| s.contains("GOT:hello")).unwrap_or(false) { break true; }
        std::thread::sleep(Duration::from_millis(50));
    };
    let ws_body: Value = serde_json::from_str(&mock.requests()[2].body).unwrap();
    let ws: Value = serde_json::from_str(
        ws_body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_call_id"] == json!("lc-i"))
            .expect("第三轮应携带 write_stdin 结果")["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        got,
        "[sandbox={sandbox}] 轮结束后 write_stdin 输入必须送达保留会话（跨轮交互）；out={:?} ws结果={ws}",
        fs::read_to_string(&out).unwrap_or_default()
    );
    pty.send("/exit");
    pty.wait("再见!");
    let (_, code) = pty.finish();
    assert_eq!(code, Some(0));
}

/// 场景 3（MCP 面）：沙箱下 MCP 服务器跨轮存活；停用 reload 后外层实际回收
///（外层 argv 扫描判定——ns 内 pid 不可跨边界使用，/proc/1 恒存在不作依据；
/// 僵尸 cmdline 为空天然排除）。
#[test]
fn lifecycle_mcp_server_survives_turn_end_and_reload_reaps() {
    let sandbox = sandbox_mode();
    let cfg = TempDir::new("lc-mcp-cfg");
    let user = TempDir::new("lc-mcp-user");
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let record_dir = TempDir::new("lc-mcp-rec");
    let record = record_dir.path().join("record.jsonl");
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp_stdio_server.py");
    let write_mcp = |enabled: bool| {
        let json = format!(
            r#"{{"mcp_servers": {{"fx": {{"enabled": {enabled}, "command": "python3", "args": ["{fixture}", "--scenario", "echo", "--record", "{record}"], "cwd": "{cwd}"}}}}}}"#,
            record = record.display(),
            cwd = std::env::temp_dir().display(),
        );
        fs::write(user.path().join("mcp.json"), json).unwrap();
    };
    write_mcp(true);
    let mock = spawn_mock(vec![
        MockResp::json(200, json!({"choices":[{"message":{"content":"LC-MCP-一轮","finish_reason":"stop"}}]}).to_string()),
        MockResp::json(200, json!({"choices":[{"message":{"content":"LC-MCP-二轮","finish_reason":"stop"}}]}).to_string()),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_LC3_KEY");
    seed_grant(user.path(), &mock.url, "fake-lc3-key");

    let mut pty = PtyCli::spawn(cli_command(
        cfg.path(), user.path(), &work, "TCHAT_LC3_KEY", "fake-lc3-key"));
    pty.wait_screen("? for shortcuts");
    pty.send("连接一次");
    pty.wait_screen("LC-MCP-一轮");
    // 证明实际启动：外层 argv 扫描（bwrap 包装与 python 载荷均含 fixture 路径）。
    let started = scan_procs(&[fixture]);
    assert!(!started.is_empty(), "[sandbox={sandbox}] MCP 服务器应已实际启动（外层身份）");
    // 轮结束（回合已收尾）后跨轮存活——修复前沙箱分支随轮线程退出被杀（RED）。
    std::thread::sleep(Duration::from_millis(1200));
    let alive_after_turn = scan_procs(&[fixture]);
    assert!(
        !alive_after_turn.is_empty(),
        "[sandbox={sandbox}] 轮结束后 MCP 服务器必须跨轮存活（016 会话语义）"
    );
    // 下一轮：健康连接复用（服务器确实在服务）。
    pty.send("还在吗");
    pty.wait_screen("LC-MCP-二轮");
    // 停用 + reload：外层实际回收。
    write_mcp(false);
    pty.send("/mcp reload");
    pty.wait_screen("已重新加载配置并重建连接");
    let dl = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < dl && !scan_procs(&[fixture]).is_empty() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        scan_procs(&[fixture]).is_empty(),
        "[sandbox={sandbox}] 停用后旧 MCP 进程应被外层回收（僵尸天然排除）"
    );
    pty.send("/exit");
    pty.wait("再见!");
    let (_, code) = pty.finish();
    assert_eq!(code, Some(0));
}

/// 场景 4：宿主异常退出（SIGKILL）后沙箱载荷停止执行（--die-with-parent 保证，
/// 经长寿创建线程仍成立：进程退出杀全部线程 → PDEATHSIG 触发）。僵尸＝已死未
/// 收割，与「活进程」区分，不计为泄漏。普通模式无 die-with-parent 机制：孤儿
/// 继续运行为既有已知差异（027B 以来），本用例仅记录并自行清理，不作契约断言。
#[test]
fn lifecycle_sandbox_payload_stops_after_cli_sigkill() {
    let sandbox = sandbox_mode();
    let cfg = TempDir::new("lc-kill-cfg");
    let user = TempDir::new("lc-kill-user");
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let log = work.join("w.log");
    // 扫描标记＝work 目录路径（同场景 1：每测试唯一；修复前 "lc-kill-{pid}"
    // 恒不匹配＝核心断言空转，2026-10-28 审查发现已修）。
    let marker = format!("{}", work.display());
    let round1 = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": [ { "id": "lc-k", "type": "function", "function": { "name": "exec_command",
            "arguments": json!({ "cmd": format!("while true; do echo x >> {}; sleep 0.05; done", log.display()),
                                 "timeout_ms": 30000, "yield_time_ms": 1500 }).to_string() } } ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, round1),
        MockResp::json(200, json!({"choices":[{"message":{"content":"LC-KILL-DONE","finish_reason":"stop"}}]}).to_string()),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_LC4_KEY");
    seed_grant(user.path(), &mock.url, "fake-lc4-key");
    let mut pty = PtyCli::spawn(cli_command(
        cfg.path(), user.path(), &work, "TCHAT_LC4_KEY", "fake-lc4-key"));
    pty.wait_screen("? for shortcuts");
    pty.send("go");
    pty.wait_screen("while true; do echo x >>");
    pty.wait_screen("LC-KILL-DONE"); // 轮结束、会话保留（修复后跨轮存活）
    // 先证存活：轮结束后载荷进程在册（跨轮保留的前提事实）。
    let pre_kill: Vec<u32> = scan_procs(&[&marker]);
    assert!(
        !pre_kill.is_empty(),
        "[sandbox={sandbox}] 轮结束后载荷应存活（跨轮保留）；扫描标记：{marker}"
    );
    // 宿主异常退出（SIGKILL）：全部线程随进程死亡 → bwrap PDEATHSIG → 载荷停止。
    let cli_pid = pty.child.id();
    unsafe { libc::kill(cli_pid as libc::pid_t, libc::SIGKILL) };
    let dl = std::time::Instant::now() + Duration::from_secs(3);
    let mut last_live = usize::MAX;
    while std::time::Instant::now() < dl {
        last_live = scan_procs(&[&marker]).len();
        if last_live == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // 逐 pid 三分类：活（cmdline 命中）／僵尸（State Z，已死未收割——容器 PID1
    // 不收割，不计为泄漏）／条目消失＝均已停止执行。
    let mut zombies = Vec::new();
    let mut gone = Vec::new();
    let mut alive = Vec::new();
    for pid in &pre_kill {
        match proc_state(*pid) {
            None => gone.push(*pid),
            Some(s) if s.contains('Z') => zombies.push(*pid),
            Some(s) => alive.push(format!("{pid}:State={s}")),
        }
    }
    let cli_state = proc_state(cli_pid).unwrap_or_else(|| "条目已消失".into());
    if sandbox {
        assert!(
            alive.is_empty(),
            "宿主 SIGKILL 后沙箱载荷必须停止执行（die-with-parent 保底）：仍存活 {alive:?}"
        );
        assert_eq!(
            last_live, 0,
            "活进程口径清零；僵尸 {zombies:?}（已死未收割，不计为泄漏）；消失 {gone:?}；CLI：{cli_state}"
        );
    } else {
        // 普通模式对照：无 die-with-parent 机制，孤儿写入进程继续运行＝既有已知
        // 差异（非 016/033 契约面），记录后由下方收尾清理。
        println!(
            "普通模式对照：CLI SIGKILL 后孤儿写入进程仍在运行（alive {alive:?}，zombies {zombies:?}，gone {gone:?}），测试收尾清理"
        );
    }
    // 清理（普通模式孤儿为既有已知差异：此处收尾不依赖被测行为）。
    for p in scan_procs(&[&marker]) {
        unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
    }
    let _ = pty.finish();
}
