//! 007 性能档位验收（§3 性能契约，基线驱动）：
//! B0＝独立 CLI 子进程恢复空会话（仅 meta）的 VmHWM；u＝同形态独立子进程
//! 32/64 MiB 两档增量斜率 max((H64−H32)/32MiB, 0)（两档做差消去 B0 与固定开销，
//! 128 MiB 仅验收余量、不参与校准）；256 档断言 VmHWM−B0 ≤ u×N＋128 MiB；
//! 吞吐 256 档 ≥1 MiB 档 ×0.5；追加耗时比 ≤2×（1 vs 256）；/sessions 百文件
//! ≤50 ms 且 chmod 000 正文仍列出；替换窗口 ≤ B0＋u×(N_old+N_new)＋128 MiB、
//! 连续 5 次不逐次累积；VmRSS 仅观测。每项 3 次取中位数。
//! 单一 #[test] 串行执行，避免并行用例干扰计时与内存采样。
#![cfg(unix)]
// 038-CI：本文件依赖 POSIX 语义（PTY/Unix fd/文件权限位），Windows 不编译；

mod common;

use common::TempDir;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MIB: u64 = 1024 * 1024;
const WATCHDOG: Duration = Duration::from_secs(120);

fn median3(mut v: [f64; 3]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[1]
}

/// 生成"大量短记录"会话文件（≤4 KiB/条，user/assistant 交替）到指定大小。
fn gen_short_session(path: &Path, target_bytes: u64, tag: &str) -> usize {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    // 每条 ≈2 KiB：content 填充约 2000 字节（按字符截取，避免切在 UTF-8 边界）
    let filler: String = "性能档位填充数据0123456789".repeat(200).chars().take(660).collect();
    let mut w = std::io::BufWriter::new(fs::File::create(path).unwrap());
    writeln!(w, "{{\"v\":1}}").unwrap();
    let mut written = 8u64;
    let mut i = 0usize;
    while written < target_bytes {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let content = format!("{tag}-{i:06}-{filler}");
        let line = json!({ "type": "text", "role": role, "content": content }).to_string();
        writeln!(w, "{line}").unwrap();
        written += line.len() as u64 + 1;
        i += 1;
    }
    w.flush().unwrap();
    i
}

/// 生成"少量长记录"会话文件（≥4 MiB/条）到指定大小。
fn gen_long_session(path: &Path, target_bytes: u64, tag: &str) -> usize {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let per_record = 4 * MIB + 64 * 1024; // 内容主体，加 JSON 包装仍 ≥4 MiB
    let filler_unit = "长记录性能档位填充".repeat(1024); // 8 KiB
    let mut w = std::io::BufWriter::new(fs::File::create(path).unwrap());
    writeln!(w, "{{\"v\":1}}").unwrap();
    let mut written = 8u64;
    let mut i = 0usize;
    while written < target_bytes {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let mut content = format!("{tag}-{i:03}-");
        while content.len() < per_record as usize {
            content.push_str(&filler_unit);
            if content.len() + filler_unit.len() > per_record as usize {
                break;
            }
        }
        let line = json!({ "type": "text", "role": role, "content": content }).to_string();
        writeln!(w, "{line}").unwrap();
        written += line.len() as u64 + 1;
        i += 1;
    }
    w.flush().unwrap();
    i
}

struct ProcStatus {
    vm_hwm_kib: f64,
    vm_rss_kib: f64,
}

fn sample_proc(pid: u32) -> ProcStatus {
    let text = fs::read_to_string(format!("/proc/{pid}/status")).expect("读取 /proc/<pid>/status");
    let mut hwm = 0.0;
    let mut rss = 0.0;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("VmHWM:") {
            hwm = v.trim().trim_end_matches(" kB").parse().unwrap();
        }
        if let Some(v) = line.strip_prefix("VmRSS:") {
            rss = v.trim().trim_end_matches(" kB").parse().unwrap();
        }
    }
    ProcStatus { vm_hwm_kib: hwm, vm_rss_kib: rss }
}

struct RunningCli {
    child: Child,
    stdin: std::process::ChildStdin,
    output: Arc<Mutex<String>>,
}

impl RunningCli {
    /// 管道模式启动（保持 stdin 打开以便采样），stdout 由线程增量收集。
    /// 预置不可达端点＋预授权：聊天轮在授权门通过后、请求即刻失败——
    /// user 记录仍入史落盘（006 t8 语义），追加计时无需 mock。
    fn spawn(user_dir: &Path, extra_ts: &str) -> RunningCli {
        let cfg_dir = user_dir.join("cfg");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(
            cfg_dir.join("manual.json"),
            r#"{"selected_provider":"deepseek","selected_model":"deepseek-flash","providers":{"deepseek":{"base_url":"http://127.0.0.1:1","key_ref":"env:TCHAT_PERF_KEY"}}}"#,
        )
        .unwrap();
        let origin = terminal_chat::catalog::origin_of("http://127.0.0.1:1").unwrap();
        let fp = terminal_chat::catalog::fingerprint("perf-key");
        fs::write(
            user_dir.join("authorized-origins.json"),
            json!({ "grants": [ { "origin": origin, "cred_fp": fp, "added_at": 0 } ] }).to_string(),
        )
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.env("TCHAT_PERF_KEY", "perf-key");
        cmd.env("TERM", "xterm")
            .env("TCHAT_USER_DIR", user_dir)
            .env("TCHAT_SESSION_TS", format!("2026020{extra_ts}"))
            .env("TCHAT_CONFIG_DIR", user_dir.join("cfg"))
            .current_dir(user_dir);
        for k in ["TCHAT_USER_DIR", "TCHAT_CONFIG_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
            cmd.env_remove(k);
        }
        cmd.env("TCHAT_USER_DIR", user_dir);
        cmd.env("TCHAT_CONFIG_DIR", user_dir.join("cfg"));
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("性能 CLI 子进程启动失败");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let output = Arc::new(Mutex::new(String::new()));
        let sink = output.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut stdout = stdout;
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
                }
            }
        });
        let sink2 = output.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut stderr = stderr;
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink2.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
                }
            }
        });
        RunningCli { child, stdin, output }
    }

    fn send(&mut self, text: &str) {
        self.stdin.write_all(text.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    /// 等待输出出现 needle（增量缓冲），返回等待耗时。
    fn wait_output(&self, needle: &str) -> Duration {
        let deadline = Instant::now() + WATCHDOG;
        loop {
            if self.output.lock().unwrap().contains(needle) {
                return Duration::ZERO;
            }
            assert!(
                Instant::now() < deadline,
                "等待 {:?} 超时，已捕获：{}",
                needle,
                self.output.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn finish(mut self) -> Option<i32> {
        drop(self.stdin);
        let deadline = Instant::now() + WATCHDOG;
        loop {
            match self.child.try_wait() {
                Ok(Some(st)) => return st.code(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("性能 CLI 子进程未在期限内退出");
                }
            }
        }
    }
}

/// 恢复指定会话并采样进程 VmHWM/VmRSS（独立子进程；装载完成即采样，VmHWM 单调）。
fn measure_restore(user_dir: &Path, name: &str, extra_ts: &str) -> (f64, f64, Duration) {
    let mut cli = RunningCli::spawn(user_dir, extra_ts);
    cli.send(&format!("/resume {name}\n"));
    let t0 = Instant::now();
    cli.wait_output("（跳过");
    let elapsed = t0.elapsed();
    let st = sample_proc(cli.child.id());
    let code = cli.finish();
    assert_eq!(code, Some(0));
    (st.vm_hwm_kib, st.vm_rss_kib, elapsed)
}

#[test]
fn perf_tiers_baseline_driven() {
    let root = TempDir::new("sc-perf");
    let user = root.path().join("user");
    fs::create_dir_all(&user).unwrap();
    let sess = user.join("sessions");
    fs::create_dir_all(&sess).unwrap();

    // ---- 数据生成（大量短记录 ≤4 KiB/条；1/32/64/256 MiB 档） ----
    let t_gen = Instant::now();
    gen_short_session(&sess.join("auto-20250101T000000-1.jsonl"), MIB, "t1");
    let _n32 = gen_short_session(&sess.join("auto-20250102T000000-32.jsonl"), 32 * MIB, "t32");
    let _n64 = gen_short_session(&sess.join("auto-20250103T000000-64.jsonl"), 64 * MIB, "t64");
    let _n256 = gen_short_session(&sess.join("auto-20250104T000000-256.jsonl"), 256 * MIB, "t256");
    // 少量长记录（≥4 MiB/条）行为档
    let n_long = gen_long_session(&sess.join("auto-20250105T000000-L.jsonl"), 256 * MIB, "tl");
    // 空会话（仅 meta）：B0 基准
    fs::write(sess.join("auto-20250106T000000-E.jsonl"), "{\"v\":1}\n").unwrap();
    let gen_secs = t_gen.elapsed().as_secs_f64();

    // ---- B0（空会话恢复，3 次中位数） ----
    let b0_runs: [f64; 3] = std::array::from_fn(|i| {
        measure_restore(&user, "auto-20250106T000000-E", &format!("{i}")).0
    });
    let b0 = median3(b0_runs);

    // ---- u 校准（32/64 MiB 两档独立子进程，3 次中位数） ----
    let h32_runs: [f64; 3] = std::array::from_fn(|i| {
        measure_restore(&user, "auto-20250102T000000-32", &format!("{i}")).0
    });
    let h64_runs: [f64; 3] = std::array::from_fn(|i| {
        measure_restore(&user, "auto-20250103T000000-64", &format!("{i}")).0
    });
    let h32 = median3(h32_runs);
    let h64 = median3(h64_runs);
    let h32_mib = h32 / 1024.0;
    let h64_mib = h64 / 1024.0;
    let u = ((h64_mib - h32_mib) / 32.0).max(0.0);

    // ---- 256 MiB 断言档（同形态短记录）＋长记录行为档 ----
    let h256_runs: [f64; 3] = std::array::from_fn(|i| {
        measure_restore(&user, "auto-20250104T000000-256", &format!("{i}")).0
    });
    let h256 = median3(h256_runs);
    let h256_mib = h256 / 1024.0;
    let b0_mib = b0 / 1024.0;
    let bound_mib = b0_mib + u * 256.0 + 128.0;
    assert!(
        h256_mib - b0_mib <= u * 256.0 + 128.0,
        "256 MiB 档峰值 {h256_mib:.1} MiB 超出 B0＋u×N＋128 MiB＝{bound_mib:.1} MiB（B0={b0_mib:.1}, u={u:.3}）"
    );
    // 长记录行为断言：完整装载、条数正确（峰值仅观测）
    let (h_long, rss_long, _long_secs) =
        measure_restore(&user, "auto-20250105T000000-L", "9");
    let loaded = terminal_chat::session::load_session_file(&sess.join("auto-20250105T000000-L.jsonl")).unwrap();
    assert_eq!(loaded.history.len(), n_long, "长记录档全部装载");
    assert_eq!(loaded.skipped_lines, 0);

    // ---- 吞吐跨档稳定（256 档 ≥ 1 MiB 档 ×0.5，含装载耗时；两档均 3 次中位） ----
    let t1_runs: [f64; 3] = std::array::from_fn(|i| {
        measure_restore(&user, "auto-20250101T000000-1", &format!("a{i}")).2.as_secs_f64()
    });
    let (_h1, rss1, _t1_single) = measure_restore(&user, "auto-20250101T000000-1", "ax");
    let t256_runs: [f64; 3] = std::array::from_fn(|i| {
        measure_restore(&user, "auto-20250104T000000-256", &format!("c{i}")).2.as_secs_f64()
    });
    let thr1 = 1.0 / median3(t1_runs);
    let thr256 = 256.0 / median3(t256_runs);
    assert!(
        thr256 >= thr1 * 0.5,
        "256 档吞吐 {thr256:.1} MiB/s 低于 1 MiB 档 {thr1:.1} MiB/s 的一半"
    );

    // ---- 单条追加耗时比（1 vs 256 MiB，≤2×；库级纯追加路径＋批次累计计时） ----
    // 规格口径（§3 目标阈值①）：同尺寸记录（≤8 KiB）在 1 MiB 与 256 MiB 会话上
    // 的单条追加耗时比 ≤2×。直接计时 Session 追加路径（待写段序列化＋write_all），
    // 每档连续 100 条累计计时取均值（信噪比提升 ~100×）；比率阈值 2×，无噪声
    // 下限——先前 CLI 轮询观测版的 10ms 噪声底属实现层新增且改变了比例判断
    // （规格从未有过该下限），已随端到端观测一并移除（证据：实施日志第四批）。
    let append_per_item = |name: &str| -> f64 {
        use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole};
        use terminal_chat::session::Session;
        let mut s = Session::new(sess.clone());
        let info = s.resume(Some(name)).unwrap();
        assert!(!info.degraded, "性能档会话文件须可获锁续写");
        let mut h = info.loaded.history;
        let filler = "追加耗时探针".repeat(128); // 6 字节×128，整行 ≈2.4 KiB（规格口径 ≤8 KiB）
        let path = sess.join(format!("{name}.jsonl"));
        let before = fs::metadata(&path).unwrap().len();
        const N: usize = 100;
        let t0 = Instant::now();
        for i in 0..N {
            h.push(HistoryItem::Text(HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: HistoryRole::User,
                content: format!("perf-append-{i:04}-{filler}"),
                user_input_event: None,
            }));
            s.on_history(&h);
        }
        assert!(s.flush_final(&h).is_ok());
        let per = t0.elapsed().as_secs_f64() / N as f64;
        // 行为性：批次落盘字节恰＝行字节总和（O(新增记录字节)，无全量重写）
        let after = fs::metadata(&path).unwrap().len();
        let expect: u64 = h[h.len() - N..]
            .iter()
            .map(|it| serde_json::to_string(it).unwrap().len() as u64 + 1)
            .sum();
        assert_eq!(after - before, expect, "批次追加落盘字节恰＝行字节总和");
        per
    };
    let a1 = append_per_item("auto-20250101T000000-1");
    let a256 = append_per_item("auto-20250104T000000-256");
    assert!(
        a256 <= a1 * 2.0,
        "256 MiB 档单条追加 {a256:.6}s 超过 1 MiB 档 {a1:.6}s 的两倍"
    );
    println!(
        "perf-append: per1={a1:.6}s per256={a256:.6}s ratio={:.3}<=2 (N=100/档)",
        a256 / a1
    );

    // ---- /sessions：256 MiB 在场＋百文件 ≤50 ms，chmod 000 正文仍列出 ----
    for i in 0..99 {
        let p = sess.join(format!("auto-20250301T0000{i:02}-99.jsonl"));
        fs::write(&p, "{\"v\":1}\n").unwrap();
    }
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        sess.join("auto-20250104T000000-256.jsonl"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let mut cli = RunningCli::spawn(&user, "f");
    // 管道模式无提示符且重复运行时审计静默（文件已是 0600）：横幅＋短沉降就绪
    cli.wait_output("欢迎使用 Terminal Chat!");
    std::thread::sleep(Duration::from_millis(150));
    let t0 = Instant::now();
    cli.send("/sessions\n");
    cli.wait_output("auto-20250104T000000-256"); // 列表含被 chmod 000 的会话
    let list_ms = t0.elapsed().as_millis();
    let code = cli.finish();
    assert_eq!(code, Some(0));
    assert!(list_ms <= 50, "/sessions 百文件列出耗时 {list_ms} ms 超过 50 ms");

    // ---- 已有历史替换窗口（PTY y 确认）：32→64 MiB 交替，5 次不逐次累积 ----
    // 行为验证接真实 mock：替换后首轮请求只含新会话（无旧会话残留）
    let reply = |content: &str| {
        common::MockResp::json(
            200,
            json!({ "choices": [{ "message": { "content": content }, "finish_reason": "stop" }] })
                .to_string(),
        )
    };
    let mock = common::spawn_mock(vec![reply("替换后回复"), reply("替换后回复2")]);
    let mock_before_replacement = mock.count();
    // PTY 子进程：mock（行为验证）＋时钟注入；manual/grant 由 PtyPerf 助手预置
    pty_perf_prep(&user, &mock.url);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    // 008-ADAPT：替换窗口依赖 /resume 非空历史的 y 确认交互——该确认流仅存在于
    // 行式路径（TUI 内 /resume 不经母本命令注册表即被 composer 拒为 Unrecognized
    // command，PTY 实测）。以 TERM=dumb 走 008 既有行式退化路径（tui_should_run
    // 判否），007/性能契约语义不变。
    cmd.env("TERM", "dumb")
        .env("TCHAT_USER_DIR", &user)
        .env("TCHAT_CONFIG_DIR", user.join("cfg"))
        .env("TCHAT_SESSION_TS", "20260209g")
        .env("TCHAT_PERF_KEY", "perf-key")
        .current_dir(&user);
    let mut pty = PtyCli::spawn_with_watchdog(cmd, WATCHDOG);
    pty.wait("你> ");
    pty.send("/resume auto-20250102T000000-32");
    pty.wait("恢复");
    let mut peaks = Vec::new();
    let mut rss_obs = Vec::new();
    for i in 0..5 {
        pty.wait("你> ");
        pty.send("/resume auto-20250103T000000-64"); // 换成 64 MiB 会话再换回，制造真实替换窗口
        pty.wait("丢弃当前");
        let st_before = sample_proc(pty.child.id());
        rss_obs.push(st_before.vm_rss_kib);
        pty.send("y");
        pty.wait("恢复");
        let st = sample_proc(pty.child.id());
        peaks.push(st.vm_hwm_kib / 1024.0);
        rss_obs.push(st.vm_rss_kib);
        // 再换回 32 MiB：下一轮窗口
        if i < 4 {
            pty.wait("你> ");
            pty.send("/resume auto-20250102T000000-32");
            pty.wait("丢弃当前");
            pty.send("y");
            pty.wait("恢复");
        }
    }
    // 行为性：替换到小会话（1 MiB 档）后验证——大历史按 10 MiB 请求上限会被
    // 本地拒绝（006 t8 语义），行为断言须在请求可发的会话规模上进行
    pty.wait("你> ");
    pty.send("/resume auto-20250101T000000-1");
    pty.wait("丢弃当前");
    pty.send("y");
    pty.wait("恢复");
    pty.wait("你> ");
    pty.send("替换后验证轮");
    pty.wait("替换后回复");
    let code0 = {
        pty.send("exit");
        pty.wait("再见!");
        pty.finish().1
    };
    assert_eq!(code0, Some(0));
    assert_eq!(mock.count(), mock_before_replacement + 1, "恰一次验证请求");
    let body: serde_json::Value =
        serde_json::from_str(&mock.requests()[0].body).unwrap();
    let body_text = body.to_string();
    assert!(body_text.contains("t1-000001"), "首轮请求含新会话（1 MiB 档）记录");
    assert!(!body_text.contains("t32-") && !body_text.contains("t64-"), "替换后请求不得残留旧会话记录");
    let msgs = body["messages"].as_array().unwrap();
    assert!(msgs[1]["content"].as_str().unwrap().contains("t1-000000"), "首条历史来自新会话");
    let replace_bound = b0_mib + u * (32.0 + 64.0) + 128.0;
    for (i, p) in peaks.iter().enumerate() {
        assert!(
            p <= &replace_bound,
            "第 {} 次替换窗口峰值 {p:.1} MiB 超出 B0＋u×96＋128＝{replace_bound:.1} MiB",
            i + 1
        );
    }
    let peak_spread = peaks.iter().cloned().fold(f64::MIN, f64::max)
        - peaks.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        peak_spread <= 32.0,
        "5 次替换窗口峰值离散 {peak_spread:.1} MiB 过大（逐次抬升迹象）：{peaks:?}"
    );
    let _ = rss_obs; // VmRSS 仅观测记录

    // 原始逐次采样（KiB）与公式核对一并输出，供证据记录直接引用
    let raw = |runs: &[f64; 3]| -> String {
        runs.iter().map(|v| format!("{v:.0}")).collect::<Vec<_>>().join(",")
    };
    println!(
        "perf-raw: B0_runs=[{}] H32_runs=[{}] H64_runs=[{}] H256_runs=[{}] (KiB)",
        raw(&b0_runs),
        raw(&h32_runs),
        raw(&h64_runs),
        raw(&h256_runs)
    );
    println!(
        "perf-obs: gen={gen_secs:.1}s B0={b0_mib:.1}MiB H32={h32_mib:.1} H64={h64_mib:.1} H256={h256_mib:.1} \
         u={u:.3} long-peak={:.1}MiB long-rss={:.1}MiB thr1={thr1:.1}MiB/s thr256={thr256:.1}MiB/s \
         append1={a1:.6}s append256={a256:.6}s sessions={list_ms}ms peaks={peaks:?} rss-obs={:?}",
        h_long / 1024.0,
        rss_long / 1024.0,
        rss1 / 1024.0,
    );
    // 规格公式核对（§6-J）：u＝max((H64−H32)/32MiB, 0)；断言 H256−B0 ≤ u×256＋128
    println!(
        "perf-formula(spec §6-J): u=max((H64-H32)/32,0)=({h64_mib:.2}-{h32_mib:.2})/32={u:.4}; \
         assert H256-B0={h256_minus_b0:.2} <= u*256+128={u_times_256_plus_128:.2} -> {}",
        h256_mib - b0_mib <= u * 256.0 + 128.0,
        h256_minus_b0 = h256_mib - b0_mib,
        u_times_256_plus_128 = u * 256.0 + 128.0,
    );
}

use common::pty::PtyCli;

/// PTY 性能子进程预置：手动配置（指向 mock）＋预授权（避免交互询问）。
fn pty_perf_prep(user_dir: &Path, base_url: &str) {
    let cfg_dir = user_dir.join("cfg");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("manual.json"),
        json!({
            "selected_provider": "deepseek", "selected_model": "deepseek-flash",
            "providers": { "deepseek": { "base_url": base_url, "key_ref": "env:TCHAT_PERF_KEY" } }
        })
        .to_string(),
    )
    .unwrap();
    let origin = terminal_chat::catalog::origin_of(base_url).unwrap();
    fs::write(
        user_dir.join("authorized-origins.json"),
        json!({ "grants": [ { "origin": origin,
            "cred_fp": terminal_chat::catalog::fingerprint("perf-key"), "added_at": 0 } ] })
            .to_string(),
    )
    .unwrap();
}
