//! 003/016 规格 exec_command 集成测试：真实进程、真实输出与文件副作用。
//! 016 起 exec_command 默认可续接（经进程会话管理器执行）：短命令在等待窗口内
//! 直接完成；长命令返回 running＋会话编号。进程会话生命周期（查询、输入、
//! 终止、容量、独立总时限的完整行为）见 tests/proc_session.rs。

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use terminal_chat::exec::{ExecArgs, ExecResult, ExecStatus, WriteStdinArgs};
use terminal_chat::proc_session::ProcessManager;

/// 每个用例独立临时目录，用例名保证并行唯一，结束清理。
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tchat-exec-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 采样日志文件大小；文件未创建视为 0。
fn log_size(path: &PathBuf) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 后台写入循环 + 长睡眠前台：直接子进程是 sh，孙进程持续写文件。
fn writer_cmd(log: &PathBuf) -> String {
    format!(
        "while true; do echo x >> {log}; sleep 0.05; done & sleep 30",
        log = log.display()
    )
}

/// 单次调用入口（参数层解析＋管理器执行）：与 turn 层分派同构——
/// 解析失败按 error 结果回传；短命令在等待窗口内完成、不留会话。
fn run(input: &serde_json::Value, cancel: Option<&AtomicBool>) -> ExecResult {
    let mgr = ProcessManager::new();
    let r = run_with(&mgr, input, cancel);
    mgr.close();
    r
}

fn run_with(
    mgr: &ProcessManager,
    input: &serde_json::Value,
    cancel: Option<&AtomicBool>,
) -> ExecResult {
    match ExecArgs::from_value(input) {
        Ok(args) => mgr.start(&args, cancel).result,
        Err(msg) => ExecResult::error(msg),
    }
}

/// 场景：空命令、零/负超时、非整数、零上限、额外参数、缺失参数、不存在的目录
/// → 一律 error，且无进程执行、无命令副作用（016：u64::MAX 合法，不再属非法）。
#[test]
fn invalid_inputs_error_without_side_effects() {
    let dir = temp_dir("invalid");
    let flag = dir.join("flag");
    let touch = format!("touch {flag}", flag = flag.display());

    let cases: Vec<(serde_json::Value, &str)> = vec![
        (json!({}), "cmd"),                                                    // 缺少必填 cmd
        (json!({ "cmd": "" }), "cmd"),                                         // 空命令
        (json!({ "cmd": " \t " }), "cmd"),                                     // 全空白
        (json!({ "cmd": &touch, "timeout_ms": 0 }), "timeout_ms"),             // 零超时
        (json!({ "cmd": &touch, "timeout_ms": -5 }), "timeout_ms"),            // 负超时
        (json!({ "cmd": &touch, "timeout_ms": "500" }), "timeout_ms"),         // 非整数
        (json!({ "cmd": &touch, "timeout_ms": 1.0 }), "timeout_ms"),           // 小数（含 1.0）
        (json!({ "cmd": &touch, "yield_time_ms": 249 }), "yield_time_ms"),     // 等待越下界
        (json!({ "cmd": &touch, "yield_time_ms": 30001 }), "yield_time_ms"),   // 等待越上界
        (json!({ "cmd": &touch, "tty": "yes" }), "tty"),                       // tty 非布尔
        (json!({ "cmd": &touch, "max_output_chars": 0 }), "max_output_chars"), // 零上限
        (json!({ "cmd": &touch, "unexpected": 1 }), "未知"),                    // 额外参数
        (
            json!({ "cmd": &touch, "workdir": dir.join("no-such-dir").display().to_string() }),
            "workdir", // 目录不存在
        ),
    ];

    for (input, hint) in &cases {
        let r = run(input, None);
        assert_eq!(r.status, ExecStatus::Error, "输入 {input} 应为 error，实际 {r:?}", input = input);
        assert!(r.exit_code.is_none(), "未执行不应有退出码：{input}");
        assert_eq!(r.output, "", "未执行不应有输出：{input}");
        assert!(!r.truncated);
        assert!(r.session_id.is_none(), "未执行＝空句柄：{input}");
        let err = r.error.unwrap_or_default();
        assert!(err.contains(hint), "错误信息应包含「{hint}」，实际：{err}");
    }
    assert!(!flag.exists(), "非法输入不得执行命令、留下副作用");
    let _ = fs::remove_dir_all(&dir);
}

/// 子进程模式入口：由 `TCHAT_WORKDIR_REL` 触发（父进程以 `--exact` 重入本测试二进制、
/// cwd 设为外部会话目录）。在会话目录内以**相对 workdir** 调用真实执行入口，
/// 并以无 workdir 的 `pwd` 落盘证明缺省即会话当前目录；两组结果 JSON 写入
/// `TCHAT_WORKDIR_OUTCOME` 后退出。子进程内所有相对路径都落在外部会话目录。
fn workdir_child_mode() {
    let Some(_) = std::env::var_os("TCHAT_WORKDIR_REL") else {
        return; // 父进程模式：无事发生
    };
    let outcome = std::env::var_os("TCHAT_WORKDIR_OUTCOME").expect("缺少 TCHAT_WORKDIR_OUTCOME");
    fs::create_dir_all("sub").unwrap();
    fs::write("sub/in-rel.txt", "rel-cwd-content\n").unwrap();
    let mgr = ProcessManager::new();
    let relative = run_with(&mgr, &json!({ "cmd": "cat in-rel.txt", "workdir": "sub" }), None);
    let default = run_with(&mgr, &json!({ "cmd": "pwd > pwd-default.txt" }), None);
    mgr.close();
    std::fs::write(
        &outcome,
        json!({
            "relative": serde_json::to_value(&relative).unwrap(),
            "default": serde_json::to_value(&default).unwrap(),
        })
        .to_string(),
    )
    .unwrap();
    std::process::exit(0);
}

/// 子进程模式入口（cd 场景）：由 `TCHAT_CD_OUTCOME` 触发（父进程重入本测试
/// 二进制、cwd＝外部会话目录，目标目录经 `TCHAT_CD_TARGET` 传入）。在**同一**
/// `ProcessManager` 内连续执行 `cd 目标 && pwd` 与缺省 workdir 的 `pwd`，
/// 两组结果 JSON 写入 outcome 后退出。
fn cd_child_mode() {
    let Some(outcome) = std::env::var_os("TCHAT_CD_OUTCOME") else {
        return; // 父进程模式：无事发生
    };
    let target = std::env::var("TCHAT_CD_TARGET").expect("缺少 TCHAT_CD_TARGET");
    let mgr = ProcessManager::new();
    let after_cd = run_with(&mgr, &json!({ "cmd": format!("cd {target} && pwd") }), None);
    let back = run_with(&mgr, &json!({ "cmd": "pwd" }), None);
    mgr.close();
    std::fs::write(
        &outcome,
        json!({
            "after_cd": serde_json::to_value(&after_cd).unwrap(),
            "back": serde_json::to_value(&back).unwrap(),
        })
        .to_string(),
    )
    .unwrap();
    std::process::exit(0);
}

/// 场景：指定目录读取相对路径文件 → 读到该目录中的目标内容；
/// 相对 workdir 基于会话当前目录解析；缺省 workdir 即会话当前目录。
/// 相对/缺省两场景经子进程隔离执行（cwd＝外部会话目录），绝对 workdir 场景路径绝对。
#[test]
fn workdir_resolves_relative_paths() {
    workdir_child_mode(); // 子进程模式：在会话目录内执行相对/缺省场景后已退出
    // 绝对 workdir：命令内相对路径基于该目录
    let dir = temp_dir("workdir");
    fs::write(dir.join("relative.txt"), "hello-relative\n").unwrap();
    let r = run(
        &json!({ "cmd": "cat relative.txt", "workdir": dir.display().to_string() }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Completed);
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output.trim(), "hello-relative");
    assert_eq!(r.error, None);
    let _ = fs::remove_dir_all(&dir);

    // 相对 workdir 与缺省 workdir：独立子进程承载（会话当前目录＝外部临时目录）
    assert!(
        std::env::var_os("TCHAT_WORKDIR_REL").is_none(),
        "workdir 子进程模式到达了未设防的用例，防止进程链递归"
    );
    let host = temp_dir("workdir-host");
    let session = host.join("session");
    fs::create_dir_all(&session).unwrap();
    let outcome = host.join("outcome.json");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("workdir_resolves_relative_paths")
        .arg("--exact")
        .env("TCHAT_WORKDIR_REL", "1")
        .env("TCHAT_WORKDIR_OUTCOME", &outcome)
        .current_dir(&session)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("workdir 子进程启动失败");
    // 先取证后清理：任何断言失败也不留残留
    let child_ok = status.status.success();
    let child_stderr = String::from_utf8_lossy(&status.stderr).to_string();
    let outcome_text = fs::read_to_string(&outcome).ok();
    let rel_file = fs::read_to_string(session.join("sub/in-rel.txt")).ok();
    let pwd_text = fs::read_to_string(session.join("pwd-default.txt")).ok();
    let session_canon = fs::canonicalize(&session).ok();
    let pwd_canon = pwd_text
        .as_deref()
        .and_then(|t| std::fs::canonicalize(t.trim()).ok());
    let _ = fs::remove_dir_all(&host);
    assert!(child_ok, "workdir 子进程异常退出：{child_stderr}");
    let v: serde_json::Value = serde_json::from_str(&outcome_text.unwrap_or_else(|| {
        panic!("子进程成功退出但未写结果（entry 漂移或缺 workdir_child_mode 调用？）")
    }))
    .unwrap();
    // 相对 workdir：读到的内容来自会话目录下的 sub/（退出码 0、无错误）
    assert_eq!(v["relative"]["status"], json!("completed"), "{}", v["relative"]);
    assert_eq!(v["relative"]["exit_code"], json!(0));
    assert_eq!(v["relative"]["output"].as_str().unwrap().trim(), "rel-cwd-content");
    assert_eq!(v["relative"]["error"], json!(null));
    assert_eq!(rel_file.as_deref(), Some("rel-cwd-content\n"), "子进程在会话目录创建的输入文件");
    // 缺省 workdir：pwd 落盘于会话目录，且内容即该目录（证明缺省＝会话当前目录）
    assert_eq!(v["default"]["status"], json!("completed"), "{}", v["default"]);
    assert_eq!(
        pwd_canon.as_ref().and_then(|p| p.to_str()),
        session_canon.as_ref().and_then(|p| p.to_str()),
        "缺省 workdir 的输出落在会话目录且 pwd 即该目录"
    );
}

/// 正常路径：真实 Shell 计算 7×3，并把 21 同时落盘和返回，防止只回显命令或伪报完成。
#[test]
fn command_computes_product_and_writes_exact_file() {
    let dir = common::TempDir::new("exec-product");
    let product = dir.path().join("product.txt");
    assert!(!product.exists());

    let r = run(
        &json!({
            "cmd": "printf '%s\\n' \"$((7 * 3))\" > product.txt && cat product.txt",
            "workdir": dir.path().display().to_string(),
        }),
        None,
    );

    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.exit_code, Some(0), "{r:?}");
    assert_eq!(r.error, None);
    assert!(!r.truncated);
    assert!(r.session_id.is_none(), "完成不留会话");
    assert_eq!(r.output, "21\n");
    assert_eq!(fs::read(&product).unwrap(), b"21\n");
}

/// 场景：stdout/stderr 各输出一行、退出码 7 → 两行都返回，completed、exit_code=7。
#[test]
fn stdout_stderr_merged_exit_code_preserved() {
    let r = run(&json!({ "cmd": "echo out-line; echo err-line 1>&2; exit 7" }), None);
    assert_eq!(r.status, ExecStatus::Completed);
    assert_eq!(r.exit_code, Some(7));
    assert_eq!(r.error, None);
    assert!(!r.truncated);
    let lines: Vec<&str> = r.output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert!(
        lines.contains(&"out-line") && lines.contains(&"err-line"),
        "stdout/stderr 两行都应返回，实际：{out:?}",
        out = r.output
    );
}

/// 非法 UTF-8 字节端到端：输出按 UTF-8 解码，非法字节替换为 U+FFFD。
#[test]
fn invalid_utf8_bytes_replaced_end_to_end() {
    let r = run(&json!({ "cmd": "printf 'a\\377b\\376c'" }), None);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.output, "a\u{FFFD}b\u{FFFD}c");
}

/// 场景（016 新语义）：显式总运行时限到期 → status=timeout、进程组终止、
/// 后续不再写文件。等待窗口（yield）大于时限时，单次调用内即观察到超时。
#[test]
fn timeout_kills_process_group_and_stops_writes() {
    let dir = temp_dir("timeout");
    let log = dir.join("w.log");
    let r = run(
        &json!({ "cmd": writer_cmd(&log), "timeout_ms": 400, "yield_time_ms": 8000 }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Timeout, "{r:?}");
    assert!(r.exit_code.is_none(), "被信号终止无整数退出码：{r:?}");
    assert!(r.error.is_some(), "超时应给出说明");

    let s1 = log_size(&log);
    assert!(s1 > 0, "写入循环确实运行过");
    thread::sleep(Duration::from_millis(400));
    let s2 = log_size(&log);
    assert_eq!(s1, s2, "超时终止后不应继续写文件（{s1} -> {s2}）");
    let _ = fs::remove_dir_all(&dir);
}

/// 场景（016 新语义）：取消当前轮不终止已启动进程——返回 cancelled＋保留句柄，
/// 进程继续写文件；随后 terminate=true 终止整个受管理进程组并停止写入。
#[test]
fn cancel_keeps_process_then_terminate_stops_writes() {
    let dir = temp_dir("cancel");
    let log = dir.join("w.log");
    let mgr = ProcessManager::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let setter = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        setter.store(true, Ordering::SeqCst);
    });
    let r = run_with(
        &mgr,
        &json!({ "cmd": writer_cmd(&log), "yield_time_ms": 4000 }),
        Some(&cancel),
    );
    assert_eq!(r.status, ExecStatus::Cancelled, "{r:?}");
    assert!(r.error.is_some(), "取消应给出说明");
    let sid = r.session_id.expect("取消后仍保留可续接句柄");

    // 016：取消等待 ≠ 进程停止——进程继续写文件
    let s1 = log_size(&log);
    thread::sleep(Duration::from_millis(400));
    let s2 = log_size(&log);
    assert!(s2 > s1, "取消后受管理进程应继续运行（{s1} -> {s2}）");

    // 明确终止回收：整个进程组停止写入
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid, "terminate": true })).unwrap();
    let r = mgr.interact(&term, None).result;
    assert_eq!(r.status, ExecStatus::Terminated, "{r:?}");
    let s3 = log_size(&log);
    thread::sleep(Duration::from_millis(400));
    assert_eq!(s3, log_size(&log), "终止后不再写文件");
    mgr.close();
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：连续输出（含中文/emoji）超过上限 → 字符数恰为上限、truncated=true、
/// 进程正常收尾（completed、exit_code=0）；输出是完整输出的确定性前缀。
#[test]
fn oversized_output_truncated_to_char_cap() {
    // 100 行 ×（“行”+2 位数字+🎉+换行 = 5 字符）= 500 码点 > 上限 200
    let cmd = "i=0; while [ $i -lt 100 ]; do printf '行%02d🎉\\n' $i; i=$((i+1)); done";
    let full: String = (0..100)
        .map(|i| format!("行{i:02}🎉\n"))
        .collect();
    let r = run(
        &json!({ "cmd": cmd, "max_output_chars": 200 }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.exit_code, Some(0), "进程应正常收尾");
    assert_eq!(r.error, None);
    assert!(r.truncated, "超限必须标记 truncated");
    assert_eq!(r.output.chars().count(), 200, "字符数应恰为上限");
    let expect: String = full.chars().take(200).collect();
    assert_eq!(r.output, expect, "输出应为完整输出的前 200 码点");
}

/// 命令内 cd 不改变下一次调用的默认目录（范围约束）：同一 ProcessManager 内
/// 连续调用，第一次 `cd 目标 && pwd` 的输出精确等于目标目录，第二次缺省
/// workdir 的 `pwd` 精确等于原会话目录。经子进程隔离承载（会话当前目录＝
/// 外部临时目录），不靠 Cargo.toml 存在性推断目录。
#[test]
fn cd_inside_command_does_not_stick() {
    cd_child_mode(); // 子进程模式：执行两组调用后已退出
    assert!(
        std::env::var_os("TCHAT_CD_OUTCOME").is_none(),
        "cd 子进程模式到达了未设防的用例，防止进程链递归"
    );
    let host = temp_dir("cd-host");
    let session = host.join("session");
    let target = host.join("target-dir");
    fs::create_dir_all(&session).unwrap();
    fs::create_dir_all(&target).unwrap();
    let outcome = host.join("outcome.json");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("cd_inside_command_does_not_stick")
        .arg("--exact")
        .env("TCHAT_CD_OUTCOME", &outcome)
        .env("TCHAT_CD_TARGET", target.display().to_string())
        .current_dir(&session)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("cd 子进程启动失败");
    // 先取证后清理：任何断言失败也不留残留
    let child_ok = status.status.success();
    let child_stderr = String::from_utf8_lossy(&status.stderr).to_string();
    let outcome_text = fs::read_to_string(&outcome).ok();
    let target_canon = fs::canonicalize(&target).ok();
    let session_canon = fs::canonicalize(&session).ok();
    let _ = fs::remove_dir_all(&host);
    assert!(child_ok, "cd 子进程异常退出：{child_stderr}");
    let v: serde_json::Value = serde_json::from_str(&outcome_text.unwrap_or_else(|| {
        panic!("子进程成功退出但未写结果（entry 漂移或缺 cd_child_mode 调用？）")
    }))
    .unwrap();
    // 第一次：cd 后 pwd 输出精确等于目标目录（自然完成、退出码 0）
    assert_eq!(v["after_cd"]["status"], json!("completed"), "{}", v["after_cd"]);
    assert_eq!(v["after_cd"]["exit_code"], json!(0));
    assert_eq!(
        v["after_cd"]["output"].as_str().unwrap().trim(),
        target_canon.as_ref().and_then(|p| p.to_str()).unwrap_or_default(),
        "cd 后 pwd 必须输出目标目录"
    );
    // 第二次：缺省 workdir 的 pwd 精确等于原会话目录（不得沿用上次目标目录）
    assert_eq!(v["back"]["status"], json!("completed"), "{}", v["back"]);
    assert_eq!(v["back"]["exit_code"], json!(0));
    assert_eq!(
        v["back"]["output"].as_str().unwrap().trim(),
        session_canon.as_ref().and_then(|p| p.to_str()).unwrap_or_default(),
        "命令内 cd 不得改变后续调用的默认目录"
    );
}

// ---------- 高风险清单补充（2026-10-10；016 语义适配）----------

/// EXEC-B04：stdout/stderr 同时 MB 级输出、上限极小——两路都继续消费到 EOF，
/// 命令正常收尾（任一管道未排空都会把子进程阻塞），前缀与截断标志正确。
#[test]
fn exec_b04_dual_stream_mb_drain_after_cap() {
    let cmd = "i=0; while [ $i -lt 40000 ]; do echo out-wwwwwwwwww; echo err-eeeeeeeeee 1>&2; \
               i=$((i+1)); done";
    let r = run(
        &json!({ "cmd": cmd, "yield_time_ms": 30000, "max_output_chars": 200 }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.exit_code, Some(0), "两路排空后命令必须正常收尾：{r:?}");
    assert!(r.truncated, "{r:?}");
    assert_eq!(r.output.chars().count(), 200, "码点恰为上限");
    // 跨流时序无契约：不钉先后；200 码点为精确前缀（可落在行中）。完整性检查：
    // 完整行必须是两条合法流之一，尾段（截断处）必须是某一合法行的前缀——无损坏、无混合
    let (complete, tail) = r.output.rsplit_once('\n').unwrap_or(("", r.output.as_str()));
    for line in complete.lines() {
        assert!(
            line == "out-wwwwwwwwww" || line == "err-eeeeeeeeee",
            "完整行必须属于两条合法流：{line}"
        );
    }
    if !tail.is_empty() {
        let ok_prefix = "out-wwwwwwwwww\n".starts_with(tail) || "err-eeeeeeeeee\n".starts_with(tail);
        assert!(ok_prefix, "截断尾段必须是合法行前缀：{tail}");
    }
}

/// EXEC-B05：合法 UTF-8 多字节字符恰好落在读取块边界（8 KiB）上——重组后与原文
/// 逐字一致，不得因读取边界产生 U+FFFD。
#[test]
fn exec_b05_utf8_multibyte_across_read_boundary() {
    // 8190 个 A 后跟“中🙂”（3+4 字节），再 99 个 b：多字节序列跨越 8192 边界
    let cmd = "printf '%0.sA' $(seq 1 8190); printf '中🙂'; printf '%0.sb' $(seq 1 99)";
    let r = run(&json!({ "cmd": cmd, "yield_time_ms": 30000 }), None);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    let mut expected = String::new();
    expected.push_str(&"A".repeat(8190));
    expected.push_str("中🙂");
    expected.push_str(&"b".repeat(99));
    assert_eq!(r.output, expected, "跨读取边界的合法 UTF-8 必须无损重组");
    assert!(!r.output.contains('\u{FFFD}'), "不得出现替换符");
}

/// EXEC-B07：并发临界——A 组被显式时限终止期间，独立进程组的 B 正常完成不受误杀。
#[test]
fn exec_b07_concurrent_groups_isolated() {
    let dir_a = temp_dir("b07-a");
    let log_a = dir_a.join("w.log");
    let cmd_a = writer_cmd(&log_a);
    let a = thread::spawn(move || {
        run(&json!({ "cmd": cmd_a, "timeout_ms": 400, "yield_time_ms": 8000 }), None)
    });
    // A 仍在运行时，B（独立进程组）反复快速执行并全部成功
    for i in 0..5 {
        let r = run(&json!({ "cmd": format!("echo ok-{i}") }), None);
        assert_eq!(r.status, ExecStatus::Completed, "B 不受 A 的组终止影响：{r:?}");
        assert_eq!(r.output.trim(), format!("ok-{i}"));
    }
    let ra = a.join().unwrap();
    assert_eq!(ra.status, ExecStatus::Timeout, "{ra:?}");
    let s1 = log_size(&log_a);
    assert!(s1 > 0, "A 确实运行过");
    thread::sleep(Duration::from_millis(300));
    assert_eq!(s1, log_size(&log_a), "A 组停止写盘");
    let _ = fs::remove_dir_all(&dir_a);
}

/// EXEC-P01（016 语义，含并入的极大时限分支）：u64::MAX 时限随**真正启动的**
/// 进程生效（不只解析参数——参数层接受另见 n2a；该进程确以 running 启动）；
/// 随后经取消（不杀）＋明确终止（回收进程组）完成验证——组长 killpg ESRCH
/// 或已知孙进程 Z/X 终态。
#[test]
fn exec_p01_huge_timeout_accepted_then_terminate_reaps_group() {
    let dir = temp_dir("p01");
    // 契约后半：取消不终止；terminate 回收整个进程组。
    // 合并点：实际启动的命令携带 timeout_ms=u64::MAX——
    // 若启动路径拒绝极大值，本组返回 error 而非 cancelled，用例失败。
    let pgid_file = dir.join("pgid.txt");
    let kid_file = dir.join("kid.txt");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag_done = Arc::new(AtomicBool::new(false));
    let done = flag_done.clone();
    let c2 = cancel.clone();
    let pgid_path = pgid_file.clone();
    let kid_path = kid_file.clone();
    let h = thread::spawn(move || {
        let mgr2 = ProcessManager::new();
        let r = run_with(
            &mgr2,
            &json!({ "cmd": format!(
                "echo $$ > {p}; sleep 30 & echo $! > {k}; wait",
                p = pgid_path.display(), k = kid_path.display()
            ), "yield_time_ms": 8000, "timeout_ms": u64::MAX }),
            Some(&*c2),
        );
        done.store(true, Ordering::SeqCst);
        (r, mgr2)
    });
    thread::sleep(Duration::from_millis(500));
    assert!(!flag_done.load(Ordering::SeqCst), "命令应仍在运行");
    cancel.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !flag_done.load(Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "取消未在期限内结束等待");
        thread::sleep(Duration::from_millis(10));
    }
    let (r, mgr2) = h.join().unwrap();
    assert_eq!(r.status, ExecStatus::Cancelled, "{r:?}");
    let sid = r.session_id.expect("取消后保留句柄");
    let pgid: i32 = fs::read_to_string(&pgid_file).unwrap().trim().parse().unwrap();
    let kid: i32 = fs::read_to_string(&kid_file).unwrap().trim().parse().unwrap();
    // 取消不杀：组长进程组仍存在
    thread::sleep(Duration::from_millis(200));
    let errno_after_cancel = unsafe {
        let rc = libc::killpg(pgid, 0);
        if rc == 0 { None } else { std::io::Error::last_os_error().raw_os_error() }
    };
    assert_eq!(errno_after_cancel, None, "取消当前轮不得终止进程组");
    // 明确终止：回收整个受管理进程组
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid, "terminate": true })).unwrap();
    assert_eq!(mgr2.interact(&term, None).result.status, ExecStatus::Terminated);
    let kid_state = |pid: i32| -> Option<char> {
        fs::read_to_string(format!("/proc/{pid}/stat")).ok()?.rsplit_once(')')?.1.trim_start().chars().next()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let errno = unsafe {
            let rc = libc::killpg(pgid, 0);
            if rc == 0 { None } else { std::io::Error::last_os_error().raw_os_error() }
        };
        if errno == Some(libc::ESRCH) || matches!(kid_state(kid), None | Some('Z') | Some('X')) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "终止后进程组未回收：pgid={pgid} kid={kid}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let _ = fs::remove_dir_all(&dir);
}

/// N4a：max_output_chars 上限 100000（码点）——超限拒绝执行、零启动；
/// 恰上限合法且截断语义正确（前缀精确、truncated、排空、退出码真实）。
#[test]
fn n4a_max_output_chars_upper_bound() {
    let dir = temp_dir("n4a");
    let flag = dir.join("probe.txt");
    let touch = format!("touch {}", flag.display());
    for bad in [100_001usize, 1_000_000, 200_000_000] {
        let r = run(&json!({ "cmd": &touch, "max_output_chars": bad }), None);
        let err = r.error.clone().unwrap_or_default();
        assert_eq!(r.status, ExecStatus::Error, "max_output_chars={bad} 应拒绝：{r:?}");
        assert!(err.contains("max_output_chars") && err.contains("100000"), "{err}");
    }
    assert!(!flag.exists(), "拒绝不得启动进程");
    // 恰上限 100000 合法：大输出截断为精确前缀、truncated、正常收尾
    let big = "i=0; while [ $i -lt 30000 ]; do echo 0123456789; i=$((i+1)); done"; // 300000 码点
    let r = run(&json!({ "cmd": big, "max_output_chars": 100_000, "yield_time_ms": 30000 }), None);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.exit_code, Some(0), "排空后正常收尾：{r:?}");
    assert!(r.truncated, "{r:?}");
    assert_eq!(r.output.chars().count(), 100_000, "码点恰为上限");
    let expect: String = "0123456789\n".repeat(10000).chars().take(100_000).collect();
    assert_eq!(r.output, expect, "前缀逐字精确");
    let _ = fs::remove_dir_all(&dir);
}

/// EXEC-E03：cmd 含 NUL 字节——启动失败明确报错，不 panic、无副作用。
#[test]
fn exec_e03_nul_in_command_fails_to_start() {
    let dir = temp_dir("e03-nul");
    let flag = dir.join("flag");
    let r = run(
        &json!({ "cmd": format!("touch {}\u{0}", flag.display()), "yield_time_ms": 5000 }),
        None,
    );
    let err = r.error.clone().unwrap_or_default();
    assert_eq!(r.status, ExecStatus::Error, "{r:?}");
    assert!(r.exit_code.is_none() && r.output.is_empty(), "{r:?}");
    assert!(err.contains("进程启动失败"), "启动级错误：{err}");
    assert!(!flag.exists(), "未启动不得有副作用");
    let _ = fs::remove_dir_all(&dir);
}

/// N2a（016 反转）：旧 60000 上限取消——60001、u64::MAX 均为合法时限值，
/// 参数层接受；60000 边界值正常执行。
#[test]
fn n2a_timeout_upper_bound_removed_by_016() {
    let dir = temp_dir("n2a");
    let flag = dir.join("probe.txt");
    let touch = format!("touch {}", flag.display());
    for ok in [60_001u64, u64::MAX] {
        let args = ExecArgs::from_value(&json!({ "cmd": &touch, "timeout_ms": ok }))
            .unwrap_or_else(|e| panic!("timeout_ms={ok} 应合法（016 取消上限）：{e}"));
        assert_eq!(args.timeout_ms, Some(ok));
    }
    // 合法值正常执行（时限内完成）
    let r = run(&json!({ "cmd": &touch, "timeout_ms": 60000 }), None);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert!(flag.exists(), "合法时限不阻止执行");
    let _ = fs::remove_dir_all(&dir);
}

/// N2b：最小合法 timeout_ms=1 在公开参数入口被接受，不是被悄悄夹到默认值；
/// 真实 1ms 时限对长睡眠生效（等待窗口大于时限，单次调用内观察到超时）。
#[test]
fn min_timeout_accepted_and_expires_as_one_millisecond() {
    use terminal_chat::exec::ExecArgs;

    let args = ExecArgs::from_value(&json!({ "cmd": "echo hi", "timeout_ms": 1 }))
        .expect("timeout_ms=1 必须是最小合法值");
    assert_eq!(args.timeout_ms, Some(1), "参数层不得把 1 改成默认值");

    let r = run(&json!({ "cmd": "sleep 5", "timeout_ms": 1, "yield_time_ms": 8000 }), None);
    assert_eq!(r.status, ExecStatus::Timeout, "1ms 后应终止长睡眠：{r:?}");
    assert_eq!(r.exit_code, None, "被终止的进程没有自然退出码：{r:?}");
    let err = r.error.clone().unwrap_or_default();
    assert!(err.contains("时限"), "超时错误应说明时限到期：{err}");
}

/// N4b：最小合法 max_output_chars=1；字符上限按 Unicode 码点截断，
/// “中🙂文”在 1／2／3 时分别保留完整码点，绝不按 UTF-8 字节切断。
#[test]
fn min_output_cap_truncates_at_codepoint_boundaries() {
    let text = "中🙂文";
    for (cap, expected) in [(1usize, "中"), (2, "中🙂"), (3, text)] {
        let r = run(
            &json!({ "cmd": "printf '%s' '中🙂文'", "max_output_chars": cap }),
            None,
        );
        assert_eq!(r.status, ExecStatus::Completed, "cap={cap}：{r:?}");
        assert_eq!(r.exit_code, Some(0), "cap={cap}：{r:?}");
        assert_eq!(r.error, None);
        assert_eq!(r.output, expected, "cap={cap} 必须保留完整码点");
        assert_eq!(r.output.as_bytes(), expected.as_bytes(), "cap={cap} 不得截断 UTF-8 字节");
        assert_eq!(r.truncated, cap < text.chars().count(), "cap={cap} 的截断标记");
    }

    // 码点额度为 1 时，后续 100 KB 仍须被读取丢弃，子进程正常收尾而不是堵塞。
    let r = run(
        &json!({
            "cmd": "printf '中'; yes 0123456789012345678901234567890 | head -c 100000",
            "max_output_chars": 1,
            "yield_time_ms": 30000,
        }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Completed, "超限后仍须排空管道：{r:?}");
    assert_eq!(r.exit_code, Some(0), "超限不得导致子进程阻塞：{r:?}");
    assert_eq!(r.output, "中");
    assert!(r.truncated);
}
