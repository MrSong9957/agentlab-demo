//! 033 矩阵实测 A/B/C/D/G（2026-10-27）：沙箱全链路 E2E——真实 CLI＋PTY＋本地
//! mock（127.0.0.1 随机端口），一轮连续调用 exec_command/apply_patch/update_plan
//! 三工具，工具结果回传 mock 后收尾。
//!
//! **环境双分支**：本文件不 set/remove `TCHAT_SANDBOX`——测试进程继承部署注入态
//!（compose 默认 =1）时走沙箱断言；`env -u` 起跑走普通模式对照断言。同一探针在
//! 两分支的允许/拒绝差异，即「差异确实来自沙箱」的对照证据（矩阵 G）。
//!
//! 覆盖：A 默认启用横幅＋完整调用链（请求计数、工具结果、文件副作用）；
//! B exec 常规功能（workdir、创建/追加/删除、中文＋空格路径、退出后无遗留进程）；
//! C 路径边界（相对/绝对/改 cwd/`sub/..`/符号链接/硬链接别名的假凭据读取、
//! 只读区写入拒绝）；D 哨兵 env 白名单、HOME/PATH、沙箱外哨兵进程的
//! environ/cwd/root/信号面；G 网络边界（本地监听器实测，按实际策略记录）。
//! 全程假凭据（唯一标记），不触真实 .env 正文；只对本轮创建的进程发信号。

mod common;

use common::pty::PtyCli;
use common::{spawn_mock, MockResp, TempDir};
use serde_json::{json, Value};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use terminal_chat::catalog;

// ---------- 沙箱证据判定（矩阵「所有工具测试必须确认实际进入沙箱成功分支」） ----------

/// 主进程（沙箱外）事实，供证据判定对照。
pub(crate) struct HostFacts {
    /// 主进程 PID 命名空间标识（`readlink /proc/self/ns/pid`，形如 `pid:[4026531836]`）。
    pub(crate) pid_ns: String,
    /// 沙箱外哨兵进程 pid（本轮创建、带假环境变量，已记录身份）。
    pub(crate) sentinel_pid: u32,
    /// 策略锚定根＝CLI 主进程 cwd（工作目录，含假 .env）。
    pub(crate) anchor: PathBuf,
}

/// 判定 exec 探针输出是否足以证明命令**真实运行在产品沙箱内**。
///
/// 探针输出的证据行（`KEY=VALUE`，构造见 [`matrix_a_full_chain_multi_tool`]）；
/// 以下四项**全部满足**才返回 `Ok(())`，任何缺失/不符汇入 `Err`（注明期望与实际）：
/// 1. `PROBE_NS` 存在、格式有效（`pid:[<inode>]`）且不同于主进程（沙箱外）ns；
/// 2. `PROBE_PID` 为有效数字且＝1（bwrap `--as-pid-1`：直接载荷＝新 ns 的 pid 1）；
/// 3. `HOST_SENTINEL_VISIBLE=no`（探针检查的 /proc/<pid> 即 `facts.sentinel_pid`，
///    其沙箱外存在性由调用侧前置断言保证）；
/// 4. `PROBE_HOME` 与 `facts.anchor` 按路径口径一致（环境配置证据，仅辅助——
///    不能替代命名空间隔离证据，见 `evidence_home_ok_ns_same_rejected`）。
///
/// 仅解析目标工具真实返回的探针输出（exec 结果 content），不从模型回复、提示词
/// 回显或 PTY 屏幕取证据。普通模式的输出必须被判为 `Err`（对照分支依据）。
fn sandbox_evidence(output: &str, facts: &HostFacts) -> Result<(), String> {
    let mut failed: Vec<String> = Vec::new();

    // 1. PID 命名空间身份：格式有效＋不同于主进程（空值/错误文本/任意字符串不算）。
    match probe_val(output, "PROBE_NS") {
        Some(ns) if valid_pid_ns(ns) && ns != facts.pid_ns => {}
        Some(ns) => failed.push(format!(
            "PROBE_NS：期望与主进程命名空间 {} 不同的 pid:[<inode>]，实际 {ns:?}",
            facts.pid_ns
        )),
        None => failed.push("PROBE_NS：证据行缺失（探针未运行或输出被截断）".to_string()),
    }

    // 2. 载荷 pid：--as-pid-1 语义＝新 PID ns 中的 pid 1。
    match probe_val(output, "PROBE_PID") {
        Some(pid) if pid.parse::<u32>().is_ok_and(|p| p == 1) => {}
        Some(pid) => failed.push(format!("PROBE_PID：期望 1（--as-pid-1 直接载荷），实际 {pid:?}")),
        None => failed.push("PROBE_PID：证据行缺失（探针未运行或输出被截断）".to_string()),
    }

    // 3. 沙箱外哨兵进程不可见（PID ns 隔离）。
    match probe_val(output, "HOST_SENTINEL_VISIBLE") {
        Some("no") => {}
        Some(v) => failed.push(format!(
            "HOST_SENTINEL_VISIBLE：期望 no（pid {} 在新 PID ns 中不可见），实际 {v:?}",
            facts.sentinel_pid
        )),
        None => {
            failed.push("HOST_SENTINEL_VISIBLE：证据行缺失（探针未运行或输出被截断）".to_string())
        }
    }

    // 4. HOME＝策略锚定根（白名单环境副作用）。
    match probe_val(output, "PROBE_HOME") {
        Some(h) if Path::new(h) == facts.anchor => {}
        Some(h) => failed.push(format!(
            "PROBE_HOME：期望锚定根 {}，实际 {h:?}",
            facts.anchor.display()
        )),
        None => failed.push("PROBE_HOME：证据行缺失（探针未运行或输出被截断）".to_string()),
    }

    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("；"))
    }
}

/// `readlink /proc/<pid>/ns/pid` 的输出形态：`pid:[<十进制 inode>]`。
fn valid_pid_ns(s: &str) -> bool {
    match s.strip_prefix("pid:[").and_then(|r| r.strip_suffix(']')) {
        Some(num) => !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

// ---------- 探针输出解析 ----------

/// 取探针输出中 `KEY=VALUE` 行的值；行不存在返回 None。
fn probe_val<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output
        .lines()
        .find_map(|l| l.trim_end_matches('\r').strip_prefix(key)?.strip_prefix('='))
}

fn read_host_pid_ns() -> String {
    fs::read_link("/proc/self/ns/pid")
        .expect("读取主进程 PID ns")
        .to_string_lossy()
        .into_owned()
}

// ---------- 配置夹具（与 e2e_acceptance 同构：隔离目录＋本地 mock＋假密钥） ----------

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

/// 预置授权记录（合法预授权＝此前交互确认过）。
fn seed_grant(user_dir: &Path, origin: &str, cred_fp: &str) {
    let text = json!({ "grants": [{ "origin": origin, "cred_fp": cred_fp, "added_at": 0 }] })
        .to_string();
    fs::write(user_dir.join("authorized-origins.json"), text).unwrap();
}

/// 哨兵进程守卫：本轮创建并记录身份，结束（含 panic）时终止并回收。
struct SentinelGuard(std::process::Child);

impl Drop for SentinelGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 退出后遗留进程扫描：/proc 下任何 cmdline 含本轮标记的进程（不含哨兵自身，
/// 哨兵由守卫回收）。返回 (pid, cmdline) 列表，应为空。
fn scan_leftovers(markers: &[&str]) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(cmdline) = fs::read_to_string(format!("/proc/{pid}/cmdline")) else { continue };
        if markers.iter().any(|m| cmdline.contains(m)) {
            found.push((pid, cmdline.replace(['\0', '\n'], " ")));
        }
    }
    found
}

// ---------- 矩阵 A：默认启用＋完整调用链（一轮连续调用多个工具） ----------

#[test]
fn matrix_a_full_chain_multi_tool() {
    let sandbox_mode = std::env::var("TCHAT_SANDBOX")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);

    // ---- 夹具：隔离配置/用户目录、假凭据（先证沙箱外可读＋唯一标记）、哨兵、监听器 ----
    let cfg = TempDir::new("033mx-cfg");
    let user = TempDir::new("033mx-user");
    let work = user.path().join("work");
    let home = user.path().join("home");
    fs::create_dir_all(work.join("子目录 demo")).unwrap();
    fs::create_dir_all(work.join("sub")).unwrap();
    fs::create_dir_all(&home).unwrap();

    let marker = format!("matrix-fake-033-{}", std::process::id());
    let env_file = work.join(".env");
    fs::write(&env_file, format!("TCHAT_033_FAKE_ENV_SECRET={marker}\n")).unwrap();
    assert!(
        fs::read_to_string(&env_file).unwrap().contains(&marker),
        "前置：假凭据在沙箱外存在、可读且含唯一标记"
    );
    // 路径别名：符号链接（读取经遮蔽挂载点）。硬链接场景自 B1 加固起整体
    // 拒绝启动（不再可能读出），已移入 sandbox_matrix_probes 的 b1 专测；
    // 本夹具保持单链接（nlink=1）。
    std::os::unix::fs::symlink(".env", work.join("env-alias")).unwrap();

    // 沙箱外哨兵进程（矩阵 D）：假环境变量，仅对本轮创建并记录身份的进程操作。
    let sentinel = SentinelGuard(
        Command::new("sleep")
            .arg("180")
            .env("TCHAT_033_SENTINEL_PROC", "proc-sentinel-fake-033")
            .spawn()
            .unwrap(),
    );
    let sentinel_pid = sentinel.0.id();
    // 前置证明：哨兵在沙箱外真实存在（否则 HOST_SENTINEL_VISIBLE=no 是空真）。
    assert!(
        fs::metadata(format!("/proc/{sentinel_pid}")).is_ok(),
        "前置：哨兵进程在沙箱外存在（pid={sentinel_pid}）"
    );

    // 网络边界（矩阵 G）：本地监听器（127.0.0.1 随机端口），仅接受一次探针连接。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let net_port = listener.local_addr().unwrap().port();
    let net_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        use std::sync::atomic::Ordering;
        use std::io::Write as _;
        let hits = net_hits.clone();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                hits.fetch_add(1, Ordering::Relaxed);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            }
        });
    }

    // 只读区写入目标（矩阵 C）：沙箱外先证明可写（防权限假通过），且不在可写根内。
    let ro_target = format!("/var/tmp/033mx-ro-{}", std::process::id());
    fs::write(&ro_target, b"outside").unwrap();
    fs::remove_file(&ro_target).unwrap();

    // ---- 探针脚本：一条 exec 采集 A/B/C/D/G 全部证据行 ----
    let probe_cmd = format!(
        r#"echo "PROBE_PID=$$"
echo "PROBE_NS=$(readlink /proc/self/ns/pid)"
echo "PROBE_HOME=$HOME"
echo "PROBE_PATH_SET=$( [ -n "$PATH" ] && echo yes || echo no )"
echo "PROBE_ENV=[$TCHAT_033_FAKE_ENV_SECRET][$TCHAT_033_SPAWN_SENTINEL][$TCHAT_033_SENTINEL_PROC]"
echo "ENV_REL=$( cat .env 2>/dev/null | grep -c {marker} || true )"
echo "ENV_ABS=$( cat '{work_abs}/.env' 2>/dev/null | grep -c {marker} || true )"
echo "ENV_SYMLINK=$( cat env-alias 2>/dev/null | grep -c {marker} || true )"
echo "ENV_CWDCHANGED=$( cd '子目录 demo' && cat ../.env 2>/dev/null | grep -c {marker} || true )"
echo "ENV_DOTDOT=$( cat ./sub/../.env 2>/dev/null | grep -c {marker} || true )"
echo "ENV_REL_ERR=$( (wc -c < .env) 2>&1 | head -c 90 )"
echo "HOST_SENTINEL_VISIBLE=$( [ -e /proc/{spid} ] && echo yes || echo no )"
echo "HOST_SENTINEL_ENV=$( tr '\0' '\n' < /proc/{spid}/environ 2>/dev/null | grep -c proc-sentinel || true )"
echo "HOST_SENTINEL_CWD=$( readlink /proc/{spid}/cwd 2>/dev/null || echo DENIED )"
echo "HOST_SENTINEL_ROOT=$( readlink /proc/{spid}/root 2>/dev/null || echo DENIED )"
echo "HOST_SENTINEL_SIGNAL=$( kill -0 {spid} 2>/dev/null && echo alive || echo no-signal )"
echo "RDONLY_WRITE=$( echo x > {ro} 2>/dev/null && echo WROTE || echo DENIED )"
echo "NET_LOCAL=$( curl -s -o /dev/null -w '%{{http_code}}' -m 4 http://127.0.0.1:{port}/net-probe 2>/dev/null || echo NETFAIL )"
printf 'line1\n' > '子目录 demo/中文 文件.txt'
printf 'line2\n' >> '子目录 demo/中文 文件.txt'
echo "CN_READ=$( cat '子目录 demo/中文 文件.txt' | tr '\n' '|' )"
printf 'x\n' > del-target.txt
rm -f del-target.txt
echo "DEL_GONE=$( [ ! -e del-target.txt ] && echo yes || echo no )""#,
        marker = marker,
        work_abs = work.display(),
        spid = sentinel_pid,
        ro = ro_target,
        port = net_port,
    );

    // ---- mock：第一轮返回三工具连续调用，第二轮收尾 ----
    let tool_calls = json!({"choices":[{"message":{"tool_calls":[
        {"id":"mx-exec","type":"function","function":{"name":"exec_command","arguments":json!({
            "cmd": probe_cmd,
            "workdir": work.display().to_string(),
            "timeout_ms": 20000,
            "yield_time_ms": 20000
        }).to_string()}},
        {"id":"mx-patch","type":"function","function":{"name":"apply_patch","arguments":json!({
            "patch": "*** Begin Patch\n*** Add File: 子目录 demo/patched-输出.txt\n+033-matrix-patch\n*** End Patch\n"
        }).to_string()}},
        {"id":"mx-plan","type":"function","function":{"name":"update_plan","arguments":json!({
            "explanation":"033 矩阵实测","plan":[{"step":"沙箱全链路验证","status":"completed"}]
        }).to_string()}}
    ]}}]});
    let mock = spawn_mock(vec![
        MockResp::json(200, tool_calls.to_string()),
        MockResp::json(
            200,
            json!({"choices":[{"message":{"content":"033-MATRIX-DONE"},"finish_reason":"stop"}]})
                .to_string(),
        ),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_033_M_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-033-mx-key"),
    );

    // ---- 启动真实 CLI（PTY；TCHAT_SANDBOX 继承部署注入态，不 set/remove） ----
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.current_dir(&work)
        .env("TERM", "xterm")
        .env("TCHAT_CONFIG_DIR", cfg.path())
        .env("TCHAT_USER_DIR", user.path())
        .env("TCHAT_033_M_KEY", "fake-033-mx-key")
        .env("TCHAT_033_SPAWN_SENTINEL", "spawn-sentinel-fake-033")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("APPDATA", &home)
        .env("CODEX_HOME", &home)
        .env("TZ", "UTC")
        .env("LC_ALL", "C.UTF-8")
        .env("SHELL", "/bin/sh")
        .env("TCHAT_SESSION_TS", "20261027T120000")
        .env_remove("TCHAT_PUBLIC_CATALOG_URL");
    let mut pty = PtyCli::spawn(cmd);
    pty.wait_screen("? for shortcuts");
    if sandbox_mode {
        // 默认启用证据：启动横幅报告 bubblewrap（来自部署注入，非本测试补设）。
        pty.wait_screen("工具沙箱模式");
    }
    pty.send("矩阵全链路验证开始");
    pty.wait_screen("033-MATRIX-DONE");
    pty.send_keys("\x03");
    let (out, code) = pty.finish();
    assert_eq!(code, Some(0), "Ctrl-C 退出码：{out}");

    // ---- 请求面：计数、生效端点（本轮批准的本地 mock＋假密钥）、工具结果回传 ----
    assert_eq!(mock.count(), 2, "恰好两轮请求（工具轮＋收尾轮）：{out}");
    let reqs = mock.requests();
    assert!(
        reqs.iter().all(|r| r.auth.as_deref().is_some_and(|a| a.contains("fake-033-mx-key"))),
        "生效端点使用本轮 mock 配置（假密钥）：{:?}",
        reqs.iter().map(|r| r.auth.clone()).collect::<Vec<_>>()
    );
    let body: Value = serde_json::from_str(&reqs[1].body).unwrap();
    let tool_content = |id: &str| -> Value {
        let m = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_call_id"] == id)
            .unwrap_or_else(|| panic!("第二请求缺少工具结果 {id}：{}", reqs[1].body));
        serde_json::from_str(m["content"].as_str().unwrap()).unwrap()
    };
    let exec: Value = tool_content("mx-exec");
    let patch: Value = tool_content("mx-patch");
    let plan: Value = tool_content("mx-plan");
    assert_eq!(exec["status"], "completed", "exec 探针完成：{exec}");
    let output = exec["output"].as_str().unwrap();
    assert_eq!(patch["ok"], json!(true), "apply_patch 沙箱内落盘：{patch}");
    assert_eq!(plan["ok"], json!(true), "update_plan 成功：{plan}");
    assert_eq!(plan["error"], Value::Null);

    // ---- 分支断言：沙箱 vs 普通对照 ----
    let facts = HostFacts {
        pid_ns: read_host_pid_ns(),
        sentinel_pid,
        anchor: work.clone(),
    };
    // 证据采集（--nocapture 时输出，供验证日志引用）。
    println!(
        "==== 033 矩阵 A 探针输出（{} 分支，哨兵 pid={sentinel_pid}，主进程 ns={}）====\n{output}",
        if sandbox_mode { "沙箱" } else { "普通对照" },
        facts.pid_ns,
    );
    if sandbox_mode {
        sandbox_evidence(output, &facts)
            .unwrap_or_else(|e| panic!("沙箱证据不成立（未真实进入沙箱分支）：{e}\n{output}"));
        // D 环境面：白名单外 env（dotenv 假凭据、父进程哨兵、哨兵进程变量）不得进入。
        assert_eq!(probe_val(output, "PROBE_ENV"), Some("[][][]"), "哨兵 env 全部被拒：{output}");
        assert_eq!(
            probe_val(output, "PROBE_HOME"),
            Some(work.display().to_string().as_str()),
            "HOME＝策略锚定根：{output}"
        );
        assert_eq!(probe_val(output, "PROBE_PATH_SET"), Some("yes"));
        // C 路径面：假凭据正文不可得（内容级＝grep 计 0；普通模式应为 1）。
        for key in ["ENV_REL", "ENV_ABS", "ENV_SYMLINK", "ENV_CWDCHANGED", "ENV_DOTDOT"] {
            assert_eq!(
                probe_val(output, key),
                Some("0"),
                "{key}：假凭据正文不得经任何路径形态读出：{output}"
            );
        }
        // D 进程可见性：沙箱外哨兵进程在新 PID ns 中不可见、不可控。
        assert_eq!(probe_val(output, "HOST_SENTINEL_VISIBLE"), Some("no"), "{output}");
        assert_eq!(probe_val(output, "HOST_SENTINEL_ENV"), Some("0"), "{output}");
        assert_eq!(probe_val(output, "HOST_SENTINEL_SIGNAL"), Some("no-signal"), "{output}");
        // C 只读区写入：可写根之外写入被拒（沙箱外已证明同路径可写）。
        assert_eq!(probe_val(output, "RDONLY_WRITE"), Some("DENIED"), "{output}");
        // G 网络边界：当前策略保留网络（未加 --unshare-net）——本地监听器可达即如实记录。
        assert_eq!(probe_val(output, "NET_LOCAL"), Some("200"), "{output}");
    } else {
        assert!(!out.contains("工具沙箱模式"), "普通模式不得显示沙箱横幅");
        sandbox_evidence(output, &facts)
            .err()
            .unwrap_or_else(|| panic!("普通模式输出不得被判为沙箱内（对照失效）：\n{output}"));
        // G 对照：普通模式 env 按现状继承（dotenv 假凭据＋父进程哨兵可见；哨兵进程
        // 变量本就不在 CLI 环境，恒为空），假凭据文件可读——差异来自沙箱。
        assert_eq!(
            probe_val(output, "PROBE_ENV"),
            Some(format!("[{marker}][spawn-sentinel-fake-033][]").as_str()),
            "普通模式 env 继承（对照）：{output}"
        );
        for key in ["ENV_REL", "ENV_ABS", "ENV_SYMLINK", "ENV_CWDCHANGED", "ENV_DOTDOT"] {
            assert_eq!(probe_val(output, key), Some("1"), "{key} 普通模式可读（对照）：{output}");
        }
        assert_eq!(probe_val(output, "HOST_SENTINEL_VISIBLE"), Some("yes"), "{output}");
        assert_eq!(probe_val(output, "RDONLY_WRITE"), Some("WROTE"), "{output}");
        assert_eq!(probe_val(output, "NET_LOCAL"), Some("200"), "普通模式网络同样可达：{output}");
    }

    // ---- 文件副作用（两分支一致：正常功能不回归） ----
    assert_eq!(
        fs::read_to_string(work.join("子目录 demo/patched-输出.txt")).unwrap(),
        "033-matrix-patch\n",
        "apply_patch Add 落盘"
    );
    assert_eq!(
        fs::read_to_string(work.join("子目录 demo/中文 文件.txt")).unwrap(),
        "line1\nline2\n",
        "exec 创建＋追加（中文＋空格路径）"
    );
    assert!(!work.join("del-target.txt").exists(), "exec 删除生效");
    assert!(
        fs::read_to_string(&env_file).unwrap().contains(&marker),
        "拒绝操作后外部假凭据未被修改"
    );
    assert!(
        patch["changed_files"]
            .as_array()
            .is_some_and(|a| a.iter().any(|f| f["path"].as_str().is_some_and(|p| p.contains("patched-输出.txt")))),
        "apply_patch 报告变更文件：{patch}"
    );
    // G 网络边界：本地监听器确被探针访问（两种模式一致＝策略未隔离网络，如实记录）。
    assert_eq!(net_hits.load(std::sync::atomic::Ordering::Relaxed), 1, "探针应恰好访问本地监听器一次");
    // B/E：正常退出后无本轮遗留进程。
    let work_str = work.display().to_string();
    let leftovers = scan_leftovers(&[&work_str, &marker, "033mx"]);
    assert!(leftovers.is_empty(), "退出后本轮进程应全部回收：{leftovers:?}");
    let _ = fs::remove_file(&ro_target); // 普通模式分支探针写入的清理
    drop(sentinel);
}

// ---------- B2（2026-10-27 核实轮）：launcher 遗留 fd 的完整真实链路 ----------

/// B2：测试进程（launcher 角色）预开**无 CLOEXEC** 的 fd 9（本轮假文件，经 dup2
/// 固定编号）→ 真实 terminal-chat 二进制（管道模式，`wait_with_output` 同
/// e2e_acceptance run_pipe 既有模式）→ 生产工具分派（mock 返回 exec_command）
/// → bwrap → `/proc/self/fd/9` 读取。沙箱/普通两跑**显式控制开关**（不依赖
/// 测试进程环境；默认启用证据由 matrix_a 的注入态分支承担）。
///
/// 预期（B2 加固后目标行为）：沙箱分支经 pre_exec `close_range(CLOSE_RANGE_
/// CLOEXEC)` 把继承 fd ≥3 标记为 execve 时关闭——`/proc/self/fd/N` 不再可读
/// （且 `PID=1` 链路证据仍在＝工具确实经 bwrap 执行）；普通对照分支保持可读
/// （普通模式行为零变化）。
#[test]
fn b2_real_cli_fd_chain_full_path() {
    for sandbox in [true, false] {
        let cfg = TempDir::new("033b2-cfg");
        let user = TempDir::new("033b2-user");
        let work = user.path().join("work");
        let home = user.path().join("home");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(work.join(".env"), "TCHAT_033_FAKE_ENV_SECRET=b2-chain-fake\n").unwrap();

        let fd_file = work.join("fd-chain-secret.txt");
        fs::write(&fd_file, "FD-CHAIN-FAKE-033").unwrap();
        // 动态 fd 号（固定编号会与并发用例的 PTY/套接字 fd 冲突——dup2 覆盖他人
        // 或他人 Drop 关闭我方，2026-10-27 合跑实测）：打开后清 CLOEXEC 并跨
        // spawn 保持，实际编号格式进探针命令。
        use std::os::fd::AsRawFd;
        let f = fs::File::open(&fd_file).unwrap();
        let fd_no = f.as_raw_fd();
        unsafe { assert_eq!(libc::fcntl(fd_no, libc::F_SETFD, 0), 0, "清除 CLOEXEC 失败"); }
        std::mem::forget(f);

        let tc = json!({"choices":[{"message":{"tool_calls":[
            {"id":"b2-fd","type":"function","function":{"name":"exec_command","arguments":json!({
                "cmd": format!("echo \"PID=$$\"; echo \"FD=$(cat /proc/self/fd/{fd_no} 2>&1 | head -c 40)\""),
                "workdir": work.display().to_string(),
                "timeout_ms": 15000,
                "yield_time_ms": 15000
            }).to_string()}}
        ]}}]}).to_string();
        let mock = spawn_mock(vec![
            MockResp::json(200, tc),
            MockResp::json(
                200,
                json!({"choices":[{"message":{"content":"B2-CHAIN-DONE"},"finish_reason":"stop"}]})
                    .to_string(),
            ),
        ]);
        write_manual(cfg.path(), &mock.url, "env:TCHAT_033_B2_KEY");
        seed_grant(
            user.path(),
            &catalog::origin_of(&mock.url).unwrap(),
            &catalog::fingerprint("fake-b2-key"),
        );

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.current_dir(&work)
            .env("TERM", "dumb")
            .env("TCHAT_CONFIG_DIR", cfg.path())
            .env("TCHAT_USER_DIR", user.path())
            .env("TCHAT_033_B2_KEY", "fake-b2-key")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home)
            .env("APPDATA", &home)
            .env("CODEX_HOME", &home)
            .env("TZ", "UTC")
            .env("LC_ALL", "C.UTF-8")
            .env("SHELL", "/bin/sh")
            .env("TCHAT_SESSION_TS", "20261027T130000")
            .env_remove("TCHAT_PUBLIC_CATALOG_URL");
        if sandbox {
            cmd.env("TCHAT_SANDBOX", "1");
        } else {
            cmd.env_remove("TCHAT_SANDBOX");
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("CLI 子进程启动失败");
        child
            .stdin
            .take()
            .unwrap()
            .write_all("fd 链路探针\n".as_bytes())
            .unwrap();
        let out = child.wait_with_output().expect("等待 CLI 子进程失败");
        unsafe { libc::close(fd_no); }

        assert!(
            out.status.success(),
            "管道模式退出码 {:?}；stdout：{}\nstderr：{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("B2-CHAIN-DONE"),
            "最终回复应落 stdout：{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(mock.count(), 2, "恰好两轮请求（工具轮＋收尾轮）");
        let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
        let m = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_call_id"] == "b2-fd")
            .expect("工具结果应回传 mock");
        let exec: Value = serde_json::from_str(m["content"].as_str().unwrap()).unwrap();
        assert_eq!(exec["status"], "completed", "exec 完成：{exec}");
        let output = exec["output"].as_str().unwrap();
        println!(
            "==== 033 B2 真实链路（{} 分支）====\n{output}",
            if sandbox { "沙箱" } else { "普通对照" }
        );
        if sandbox {
            // B2 加固目标：非 CLOEXEC fd 经 CLOSE_RANGE_CLOEXEC 于 execve 时关闭。
            assert!(
                !output.contains("FD-CHAIN-FAKE-033") && output.contains("No such file"),
                "沙箱分支：fd 正文不得再可读（execve 时应已关闭）：{output}"
            );
            assert!(
                output.lines().any(|l| l == "PID=1"),
                "链路证据：exec 经 bwrap --as-pid-1：{output}"
            );
        } else {
            assert!(
                output.contains("FD-CHAIN-FAKE-033"),
                "普通对照：fd 继承行为不变（零变化承诺）：{output}"
            );
            assert!(
                !output.lines().any(|l| l == "PID=1"),
                "普通对照 pid≠1：{output}"
            );
        }
    }
}

// ---------- sandbox_evidence 判定器自测（正反例；测试辅助函数开发记录，非产品缺陷 RED） ----------

mod evidence_tests {
    use super::*;

    fn facts() -> HostFacts {
        HostFacts {
            pid_ns: "pid:[4026531836]".to_string(),
            sentinel_pid: 999_999,
            anchor: PathBuf::from("/tmp/tchat-test-anchor/work"),
        }
    }

    /// 完整有效证据：ns 不同且格式有效、载荷 pid=1、哨兵不可见、HOME=锚定根。
    fn valid_output() -> String {
        [
            "PROBE_PID=1",
            "PROBE_NS=pid:[4026532999]",
            "PROBE_HOME=/tmp/tchat-test-anchor/work",
            "PROBE_PATH_SET=yes",
            "HOST_SENTINEL_VISIBLE=no",
        ]
        .join("\n")
    }

    /// 判定器通过：全部证据成立。
    #[test]
    fn evidence_valid_passes() {
        assert_eq!(sandbox_evidence(&valid_output(), &facts()), Ok(()));
    }

    /// 普通模式真实形态的输出必须被拒绝：ns 相同、载荷为真实子进程 pid、
    /// 哨兵可见、HOME 为主进程 HOME；错误信息须逐项指出。
    #[test]
    fn evidence_normal_mode_output_rejected() {
        let normal = [
            "PROBE_PID=4242",
            "PROBE_NS=pid:[4026531836]",
            "PROBE_HOME=/tmp/tchat-test-home",
            "HOST_SENTINEL_VISIBLE=yes",
        ]
        .join("\n");
        let err = sandbox_evidence(&normal, &facts()).unwrap_err();
        for key in ["PROBE_NS", "PROBE_PID", "HOST_SENTINEL_VISIBLE", "PROBE_HOME"] {
            assert!(err.contains(key), "错误信息须指出 {key}：{err}");
        }
    }

    /// 空输出（探针未运行）必须被拒绝，且四项证据全部报缺失。
    #[test]
    fn evidence_empty_output_rejected() {
        let err = sandbox_evidence("", &facts()).unwrap_err();
        for key in ["PROBE_NS", "PROBE_PID", "HOST_SENTINEL_VISIBLE", "PROBE_HOME"] {
            assert!(err.contains(key), "空输出须报 {key} 缺失：{err}");
        }
    }

    /// 缺失或异常字段必须被拒绝：ns 非法格式、pid 非数字、哨兵行缺失；
    /// 本例 HOME 正确——通过项不出现在错误信息中（HOME 不能替代缺失证据）。
    #[test]
    fn evidence_garbage_or_missing_fields_rejected() {
        let garbage = [
            "PROBE_PID=not-a-number",
            "PROBE_NS=readlink: Permission denied",
            "PROBE_HOME=/tmp/tchat-test-anchor/work",
        ]
        .join("\n");
        let err = sandbox_evidence(&garbage, &facts()).unwrap_err();
        assert!(err.contains("PROBE_NS"), "非法 ns 文本须被拒：{err}");
        assert!(err.contains("PROBE_PID"), "非数字 pid 须被拒：{err}");
        assert!(err.contains("HOST_SENTINEL_VISIBLE"), "缺失哨兵行须被拒：{err}");
        assert!(!err.contains("PROBE_HOME"), "HOME 正确时不应误报：{err}");
    }

    /// HOME 正确但命名空间与主进程相同：仍被拒绝——环境配置证据不能替代
    /// 命名空间隔离证据（本例仅 ns 一项不符）。
    #[test]
    fn evidence_home_ok_ns_same_rejected() {
        let same_ns = valid_output().replace("pid:[4026532999]", "pid:[4026531836]");
        let err = sandbox_evidence(&same_ns, &facts()).unwrap_err();
        assert!(err.contains("PROBE_NS"), "ns 相同须被拒：{err}");
        assert!(!err.contains("PROBE_PID"), "pid 证据成立不应误报：{err}");
        assert!(!err.contains("PROBE_HOME"), "HOME 证据成立不应误报：{err}");
    }
}
