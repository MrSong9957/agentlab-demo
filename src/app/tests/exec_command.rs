//! 003 规格 exec_command 集成测试：真实进程、真实输出与文件副作用。
//! 覆盖规格第 5 节预测表：无效输入、workdir 解析、双流合并、超时/取消、输出截断。

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use terminal_chat::exec::{exec_command, ExecStatus};

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

/// 场景：空命令、零/负超时、零上限、额外参数、缺失参数、不存在的目录
/// → 一律 error，且无进程执行、无命令副作用。
#[test]
fn invalid_inputs_error_without_side_effects() {
    let dir = temp_dir("invalid");
    let flag = dir.join("flag");
    let touch = format!("touch {flag}", flag = flag.display());

    let cases: Vec<(serde_json::Value, &str)> = vec![
        (json!({}), "cmd"),                                                  // 缺少必填 cmd
        (json!({ "cmd": "" }), "cmd"),                                       // 空命令
        (json!({ "cmd": " \t " }), "cmd"),                                   // 全空白
        (json!({ "cmd": &touch, "timeout_ms": 0 }), "timeout_ms"),           // 零超时
        (json!({ "cmd": &touch, "timeout_ms": -5 }), "timeout_ms"),          // 负超时
        (json!({ "cmd": &touch, "timeout_ms": "500" }), "timeout_ms"),       // 非整数
        (json!({ "cmd": &touch, "max_output_chars": 0 }), "max_output_chars"), // 零上限
        (json!({ "cmd": &touch, "unexpected": 1 }), "未知"),                  // 额外参数
        (
            json!({ "cmd": &touch, "workdir": dir.join("no-such-dir").display().to_string() }),
            "workdir", // 目录不存在
        ),
    ];

    for (input, hint) in &cases {
        let r = exec_command(input, None);
        assert_eq!(r.status, ExecStatus::Error, "输入 {input} 应为 error，实际 {r:?}", input = input);
        assert!(r.exit_code.is_none(), "未执行不应有退出码：{input}");
        assert_eq!(r.output, "", "未执行不应有输出：{input}");
        assert!(!r.truncated);
        let err = r.error.unwrap_or_default();
        assert!(err.contains(hint), "错误信息应包含「{hint}」，实际：{err}");
    }
    assert!(!flag.exists(), "非法输入不得执行命令、留下副作用");
    let _ = fs::remove_dir_all(&dir);
}

/// 子进程模式入口：由 `TCHAT_WORKDIR_REL` 触发（父进程以 `--exact` 重入本测试二进制、
/// cwd 设为外部会话目录）。在会话目录内以**相对 workdir** 调用真实 exec_command，
/// 并以无 workdir 的 `pwd` 落盘证明缺省即会话当前目录；两组结果 JSON 写入
/// `TCHAT_WORKDIR_OUTCOME` 后退出。子进程内所有相对路径都落在外部会话目录，
/// 不在测试进程 cwd（仓库根）创建任何文件（§3 隔离）。
fn workdir_child_mode() {
    let Some(_) = std::env::var_os("TCHAT_WORKDIR_REL") else {
        return; // 父进程模式：无事发生
    };
    let outcome = std::env::var_os("TCHAT_WORKDIR_OUTCOME").expect("缺少 TCHAT_WORKDIR_OUTCOME");
    fs::create_dir_all("sub").unwrap();
    fs::write("sub/in-rel.txt", "rel-cwd-content\n").unwrap();
    let relative = exec_command(&json!({ "cmd": "cat in-rel.txt", "workdir": "sub" }), None);
    let default = exec_command(&json!({ "cmd": "pwd > pwd-default.txt" }), None);
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

/// 场景：指定目录读取相对路径文件 → 读到该目录中的目标内容；
/// 相对 workdir 基于会话当前目录解析；缺省 workdir 即会话当前目录。
/// 相对/缺省两场景经子进程隔离执行（cwd＝外部会话目录），绝对 workdir 场景路径绝对。
#[test]
fn workdir_resolves_relative_paths() {
    workdir_child_mode(); // 子进程模式：在会话目录内执行相对/缺省场景后已退出
    // 绝对 workdir：命令内相对路径基于该目录
    let dir = temp_dir("workdir");
    fs::write(dir.join("relative.txt"), "hello-relative\n").unwrap();
    let r = exec_command(
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

    let r = exec_command(
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
    assert_eq!(r.output, "21\n");
    assert_eq!(fs::read(&product).unwrap(), b"21\n");
}

/// 场景：stdout/stderr 各输出一行、退出码 7 → 两行都返回，completed、exit_code=7。
#[test]
fn stdout_stderr_merged_exit_code_preserved() {
    let r = exec_command(&json!({ "cmd": "echo out-line; echo err-line 1>&2; exit 7" }), None);
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
    let r = exec_command(&json!({ "cmd": "printf 'a\\377b\\376c'" }), None);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.output, "a\u{FFFD}b\u{FFFD}c");
}

/// 场景：超时带孙进程的命令 → status=timeout；受管理进程停止，后续不再写文件。
#[test]
fn timeout_kills_process_group_and_stops_writes() {
    let dir = temp_dir("timeout");
    let log = dir.join("w.log");
    let r = exec_command(
        &json!({ "cmd": writer_cmd(&log), "timeout_ms": 400 }),
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

/// 场景：外部取消标志带孙进程的命令 → status=cancelled；进程组停止，不再写文件。
#[test]
fn cancel_flag_stops_managed_processes() {
    let dir = temp_dir("cancel");
    let log = dir.join("w.log");
    let cancel = Arc::new(AtomicBool::new(false));
    let setter = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        setter.store(true, Ordering::SeqCst);
    });
    let r = exec_command(
        &json!({ "cmd": writer_cmd(&log), "timeout_ms": 60_000 }),
        Some(&*cancel),
    );
    assert_eq!(r.status, ExecStatus::Cancelled, "{r:?}");
    assert!(r.error.is_some(), "取消应给出说明");

    let s1 = log_size(&log);
    assert!(s1 > 0, "写入循环确实运行过");
    thread::sleep(Duration::from_millis(400));
    let s2 = log_size(&log);
    assert_eq!(s1, s2, "取消后不应继续写文件（{s1} -> {s2}）");
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
    let r = exec_command(
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

/// 超限后读取线程必须继续读取并丢弃（否则管道写满，子进程阻塞到超时）：
/// 200 KB 输出、上限 200 字符，进程仍应正常收尾。
#[test]
fn discard_after_cap_keeps_child_running_to_completion() {
    let r = exec_command(
        &json!({ "cmd": "yes 0123456789012345678901234567890 | head -c 200000", "max_output_chars": 200 }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.exit_code, Some(0), "输出超限不得导致子进程阻塞或超时");
    assert!(r.truncated);
    assert_eq!(r.output.chars().count(), 200);
    assert!(r.output.chars().all(|c| c.is_ascii()), "yes 输出应为 ASCII 前缀");
}

/// 命令内 cd 不改变下一次调用的默认目录（范围约束）。
#[test]
fn cd_inside_command_does_not_stick() {
    let dir = temp_dir("cd");
    let r = exec_command(&json!({ "cmd": format!("cd {d} && pwd", d = dir.display()) }), None);
    assert_eq!(r.status, ExecStatus::Completed);
    // 上一条 cd 不影响本条：仍在会话当前目录（crate 根）
    let r2 = exec_command(&json!({ "cmd": "test -f Cargo.toml" }), None);
    assert_eq!(r2.exit_code, Some(0), "cd 不应改变后续调用的默认目录");
    let _ = fs::remove_dir_all(&dir);
}

// ---------- 高风险清单补充（2026-10-10）----------

/// EXEC-B04：stdout/stderr 同时 MB 级输出、上限极小——两路都继续消费到 EOF，
/// 命令正常收尾（任一管道未排空都会把子进程阻塞到超时），前缀与截断标志正确。
#[test]
fn exec_b04_dual_stream_mb_drain_after_cap() {
    let cmd = "i=0; while [ $i -lt 40000 ]; do echo out-wwwwwwwwww; echo err-eeeeeeeeee 1>&2; \
               i=$((i+1)); done";
    let r = exec_command(
        &json!({ "cmd": cmd, "timeout_ms": 15000, "max_output_chars": 200 }),
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
    let r = exec_command(&json!({ "cmd": cmd, "timeout_ms": 15000 }), None);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    let mut expected = String::new();
    expected.push_str(&"A".repeat(8190));
    expected.push_str("中🙂");
    expected.push_str(&"b".repeat(99));
    assert_eq!(r.output, expected, "跨读取边界的合法 UTF-8 必须无损重组");
    assert!(!r.output.contains('\u{FFFD}'), "不得出现替换符");
}

/// EXEC-B07：并发临界——A 组超时被终止期间，独立进程组的 B 正常完成不受误杀。
#[test]
fn exec_b07_concurrent_groups_isolated() {
    let dir_a = temp_dir("b07-a");
    let log_a = dir_a.join("w.log");
    let cmd_a = writer_cmd(&log_a);
    let a = thread::spawn(move || {
        exec_command(&json!({ "cmd": cmd_a, "timeout_ms": 400 }), None)
    });
    // A 仍在运行时，B（独立进程组）反复快速执行并全部成功
    for i in 0..5 {
        let r = exec_command(&json!({ "cmd": format!("echo ok-{i}") }), None);
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

/// EXEC-P01（新契约，2026-10-10 N2）：timeout_ms=u64::MAX 在参数层被拒绝（零启动、
/// 零探针副作用）；取消路径改用合法超时验证——观察窗内命令仍运行，取消后进程组
/// 真实终止（组长 killpg ESRCH 或已知孙进程 Z/X 终态）。
#[test]
fn exec_p01_huge_timeout_rejected_and_cancel_path() {
    // 契约前半：超限值拒绝、零副作用
    let dir = temp_dir("p01");
    let flag = dir.join("flag.txt");
    let r = exec_command(
        &json!({ "cmd": format!("touch {}", flag.display()), "timeout_ms": u64::MAX }),
        None,
    );
    let err = r.error.clone().unwrap_or_default();
    assert_eq!(r.status, ExecStatus::Error, "{r:?}");
    assert!(err.contains("timeout_ms") && err.contains("60000"), "{err}");
    assert!(!flag.exists(), "拒绝不得启动进程");

    // 契约后半：合法超时（30s sleep）＋取消入口回收（原调查的取消语义保留）
    let pgid_file = dir.join("pgid.txt");
    let kid_file = dir.join("kid.txt");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag_done = Arc::new(AtomicBool::new(false));
    let done = flag_done.clone();
    let c2 = cancel.clone();
    let pgid_path = pgid_file.clone();
    let kid_path = kid_file.clone();
    let h = thread::spawn(move || {
        let r = exec_command(
            &json!({ "cmd": format!(
                "echo $$ > {p}; sleep 30 & echo $! > {k}; wait",
                p = pgid_path.display(), k = kid_path.display()
            ), "timeout_ms": 30000 }),
            Some(&*c2),
        );
        done.store(true, Ordering::SeqCst);
        r
    });
    thread::sleep(Duration::from_millis(500));
    assert!(!flag_done.load(Ordering::SeqCst), "命令应仍在运行");
    cancel.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !flag_done.load(Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "取消未在期限内生效");
        thread::sleep(Duration::from_millis(10));
    }
    let r = h.join().unwrap();
    assert_eq!(r.status, ExecStatus::Cancelled, "{r:?}");
    let pgid: i32 = fs::read_to_string(&pgid_file).unwrap().trim().parse().unwrap();
    let kid: i32 = fs::read_to_string(&kid_file).unwrap().trim().parse().unwrap();
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
            "取消后进程组未终止：pgid={pgid} kid={kid}"
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
        let r = exec_command(&json!({ "cmd": &touch, "max_output_chars": bad }), None);
        let err = r.error.clone().unwrap_or_default();
        assert_eq!(r.status, ExecStatus::Error, "max_output_chars={bad} 应拒绝：{r:?}");
        assert!(err.contains("max_output_chars") && err.contains("100000"), "{err}");
    }
    assert!(!flag.exists(), "拒绝不得启动进程");
    // 恰上限 100000 合法：大输出截断为精确前缀、truncated、正常收尾
    let big = "i=0; while [ $i -lt 30000 ]; do echo 0123456789; i=$((i+1)); done"; // 300000 码点
    let r = exec_command(&json!({ "cmd": big, "max_output_chars": 100_000, "timeout_ms": 15000 }), None);
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
    let r = exec_command(
        &json!({ "cmd": format!("touch {}\u{0}", flag.display()), "timeout_ms": 5000 }),
        None,
    );
    let err = r.error.clone().unwrap_or_default();
    assert_eq!(r.status, ExecStatus::Error, "{r:?}");
    assert!(r.exit_code.is_none() && r.output.is_empty(), "{r:?}");
    assert!(err.contains("进程启动失败"), "启动级错误：{err}");
    assert!(!flag.exists(), "未启动不得有副作用");
    let _ = fs::remove_dir_all(&dir);
}

/// N2a（恢复版，2026-10-10 验收轮）：timeout_ms 上限 60000——超限（60001、u64::MAX）
/// 在参数校验层直接拒绝，零进程启动、零探针副作用；60000 边界值正常执行。
/// （原版本在 p01 测试重写的替换区间中被意外删除，验收轮枚举核对发现并恢复，
/// 断言与原始 RED 版本一致。）
#[test]
fn n2a_timeout_upper_bound_rejected_before_start() {
    let dir = temp_dir("n2a");
    let flag = dir.join("probe.txt");
    let touch = format!("touch {}", flag.display());
    for bad in [60001u64, 100_000, u64::MAX] {
        let r = exec_command(&json!({ "cmd": &touch, "timeout_ms": bad }), None);
        let err = r.error.clone().unwrap_or_default();
        assert_eq!(r.status, ExecStatus::Error, "timeout_ms={bad} 应拒绝：{r:?}");
        assert!(err.contains("timeout_ms"), "{err}");
        assert!(err.contains("60000"), "错误应含上限：{err}");
    }
    assert!(!flag.exists(), "拒绝不得启动进程、零副作用");
    let r = exec_command(&json!({ "cmd": "echo hi", "timeout_ms": 60000 }), None);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.output.trim(), "hi");
    let _ = fs::remove_dir_all(&dir);
}

/// N2b：最小合法 timeout_ms=1 在公开参数入口被接受，不是被悄悄夹到默认值。
/// 真实 1ms 只配确定性长睡眠（5 秒），结果必为超时，不依赖执行快慢的时序竞争。
#[test]
fn min_timeout_accepted_and_expires_as_one_millisecond() {
    use terminal_chat::exec::ExecArgs;

    let args = ExecArgs::from_value(&json!({ "cmd": "echo hi", "timeout_ms": 1 }))
        .expect("timeout_ms=1 必须是最小合法值");
    assert_eq!(args.timeout_ms, Some(1), "参数层不得把 1 改成默认值");

    let r = exec_command(&json!({ "cmd": "sleep 5", "timeout_ms": 1 }), None);
    assert_eq!(r.status, ExecStatus::Timeout, "1ms 后应终止长睡眠：{r:?}");
    assert_eq!(r.exit_code, None, "被终止的进程没有自然退出码：{r:?}");
    let err = r.error.clone().unwrap_or_default();
    assert!(err.contains("1 ms"), "超时错误应报告真实的 1ms 上限：{err}");
}

/// N4b：最小合法 max_output_chars=1；字符上限按 Unicode 码点截断，
/// “中🙂文”在 1／2／3 时分别保留完整码点，绝不按 UTF-8 字节切断。
#[test]
fn min_output_cap_truncates_at_codepoint_boundaries() {
    let text = "中🙂文";
    for (cap, expected) in [(1usize, "中"), (2, "中🙂"), (3, text)] {
        let r = exec_command(
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

    // 码点额度为 1 时，后续 100 KB 仍须被读取丢弃，子进程正常收尾而不是堵塞到超时。
    let r = exec_command(
        &json!({
            "cmd": "printf '中'; yes 0123456789012345678901234567890 | head -c 100000",
            "max_output_chars": 1,
            "timeout_ms": 15000,
        }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Completed, "超限后仍须排空管道：{r:?}");
    assert_eq!(r.exit_code, Some(0), "超限不得导致子进程阻塞或超时：{r:?}");
    assert_eq!(r.output, "中");
    assert!(r.truncated);
}
