//! 007 CLI E2E：自动保存/恢复闭环、命令面、双进程锁降级、权限与凭证边界、
//! 确认流（PTY）。全部真实 CLI（CARGO_BIN_EXE）＋本地 mock＋独立临时目录；
//! 时间戳注入固定（TCHAT_SESSION_TS）；所有等待 poll＋截止时间。

mod common;

use common::pty::PtyCli;
use terminal_chat::prompt::{HistoryItem, HistoryRole};
use common::{spawn_mock, MockResp, TempDir};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use terminal_chat::catalog;
use terminal_chat::prompt::build_request;
use terminal_chat::session::load_session_file;

const WATCHDOG: Duration = Duration::from_secs(30);

fn chat_body(content: &str) -> String {
    json!({ "choices": [{ "message": { "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

fn tc_exec(id: &str, cmd: &str) -> String {
    json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [{ "id": id, "type": "function",
            "function": { "name": "exec_command",
                "arguments": json!({ "cmd": cmd }).to_string() } }]
    }, "finish_reason": "tool_calls" }] })
    .to_string()
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

fn seed_grant(user_dir: &Path, origin: &str, key: &str) {
    fs::create_dir_all(user_dir).unwrap();
    let store = json!({ "grants": [
        { "origin": origin, "cred_fp": catalog::fingerprint(key), "added_at": 0 }
    ]});
    fs::write(user_dir.join("authorized-origins.json"), store.to_string()).unwrap();
}

/// 会话夹具：cfg（配置）＋user（TCHAT_USER_DIR）＋mock；预授权，避免交互询问。
struct Fixture {
    _cfg: TempDir,
    user: TempDir,
    _work: TempDir,
    mock: common::Mock,
    key: &'static str,
}

impl Fixture {
    fn new(responses: Vec<MockResp>) -> Self {
        let cfg = TempDir::new("sc-cli-cfg");
        let user = TempDir::new("sc-cli-user");
        let work = TempDir::new("sc-cli-work");
        let mock = spawn_mock(responses);
        write_manual(cfg.path(), &mock.url, "env:TCHAT_SC_KEY");
        let origin = catalog::origin_of(&mock.url).unwrap();
        seed_grant(user.path(), &origin, "sc-key-value");
        Fixture { _cfg: cfg, user, _work: work, mock, key: "sc-key-value" }
    }

    fn envs(&self) -> Vec<(String, String)> {
        self.base_envs()
            .into_iter()
            .chain([("TCHAT_SC_KEY".to_string(), self.key.to_string())])
            .collect()
    }

    fn base_envs(&self) -> Vec<(String, String)> {
        vec![
            ("TCHAT_CONFIG_DIR".to_string(), self._cfg.path().display().to_string()),
            ("TCHAT_USER_DIR".to_string(), self.user.path().display().to_string()),
            // 注入时钟：文件名确定，字典序＝时间序
            ("TCHAT_SESSION_TS".to_string(), "20260101T000000".to_string()),
        ]
    }

    fn sessions(&self) -> PathBuf {
        sessions_dir_for(self.user.path())
    }

    /// 列出 user 目录下全部 auto 会话文件名（排序）。
    fn autos(&self) -> Vec<String> {
        let dir = self.sessions();
        let mut names: Vec<String> = fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| {
                    let n = e.unwrap().file_name().into_string().ok()?;
                    n.starts_with("auto-").then_some(n)
                })
                .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn sessions_dir_for(user: &Path) -> PathBuf {
    user.join("sessions")
}

/// 管道模式运行真实 CLI（工作目录＝独立临时目录，不读真实用户环境/项目 .env）。
fn run_pipe(project: &Path, envs: &[(String, String)], stdin_text: &str) -> (String, String, Option<i32>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.current_dir(project).env("TERM", "xterm");
    for k in ["TCHAT_USER_DIR", "TCHAT_CONFIG_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
        cmd.env_remove(k);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("CLI 子进程启动失败");
    child.stdin.take().unwrap().write_all(stdin_text.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("等待 CLI 子进程失败");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

/// 后台排干线程：读至 EOF，内容并入共享缓冲（防管道缓冲塞满阻塞子进程）。
fn spawn_drain(
    mut stream: impl std::io::Read + Send + 'static,
    sink: std::sync::Arc<std::sync::Mutex<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
    })
}

/// 长驻管道 CLI：保持 stdin 打开（跨进程持锁/多轮交互），按需追加写入；
/// stdout/stderr 由后台线程持续收集，支持 wait_for 同步关键时序（如"已获锁"）。
struct LivePipe {
    child: Child,
    stdin: std::process::ChildStdin,
    output: std::sync::Arc<std::sync::Mutex<String>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl LivePipe {
    fn spawn(project: &Path, envs: &[(String, String)]) -> LivePipe {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.current_dir(project).env("TERM", "xterm");
        for k in ["TCHAT_USER_DIR", "TCHAT_CONFIG_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
            cmd.env_remove(k);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("CLI 子进程启动失败");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let output = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stdout_reader = spawn_drain(stdout, output.clone());
        let _stderr_reader = spawn_drain(stderr, output.clone());
        LivePipe { child, stdin, output, reader: Some(stdout_reader) }
    }

    /// 等待输出中出现 needle（后台线程持续收集）。
    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + WATCHDOG;
        while !self.output.lock().unwrap().contains(needle) {
            assert!(
                Instant::now() < deadline,
                "等待 {:?} 超时，已捕获：{}",
                needle,
                self.output.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, text: &str) {
        self.stdin.write_all(text.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn finish(mut self) -> (String, Option<i32>) {
        drop(self.stdin);
        let deadline = Instant::now() + WATCHDOG;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("长驻 CLI 未在期限内退出");
                }
            }
        };
        // stdout/stderr 由后台线程增量收集：等读线程到 EOF 后取共享缓冲
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        (self.output.lock().unwrap().clone(), status.code())
    }
}

fn unix_mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// 等待谓词成立（poll＋截止），用于等待子进程落盘等异步效应。
fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "等待条件超时");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------- E1：自动保存默认生效＋恢复闭环 ----------

/// 两轮对话（含一次 exec 工具调用）后不 /save 直接 exit→auto 文件存在且含全部
/// 记录；新启动 /resume（无参）装载完整，首轮请求投影与内存历史逐条一致；
/// 目录 0700、文件 0600；凭证值不出现在会话文件（§6-G 不主动保存鉴权配置），
/// 用户输入原样保留。
#[test]
fn e1_auto_save_default_and_resume_roundtrip() {
    let probe_dir = TempDir::new("sc-e1-probe");
    let probe = probe_dir.path().join("p.txt");
    let fx = Fixture::new(vec![
        MockResp::json(200, tc_exec("call-e1", &format!("echo side-effect > {}", probe.display()))),
        MockResp::json(200, chat_body("第一轮完成")),
        MockResp::json(200, chat_body("第二轮回复")),
    ]);
    let work = TempDir::new("sc-e1-run1");
    let (out, err, code) = run_pipe(
        work.path(),
        &fx.envs(),
        &format!("你好\n再问一次\nexit\n"),
    );
    assert_eq!(code, Some(0), "stdout：{out}\nstderr：{err}");
    assert!(out.contains("Agent：第一轮完成") && out.contains("Agent：第二轮回复"), "{out}");
    // 副作用真实发生（工具确实执行）
    assert!(probe.exists());

    // auto 文件：恰一个，含全部记录（meta＋user＋tool_calls＋result＋assistant＋user＋assistant）
    let autos = fx.autos();
    assert_eq!(autos.len(), 1, "{autos:?}");
    let auto_path = fx.sessions().join(&autos[0]);
    let loaded = load_session_file(&auto_path).unwrap();
    assert_eq!(loaded.skipped_lines, 0);
    assert_eq!(loaded.repairs, 0);
    assert_eq!(loaded.history.len(), 7, "{:?}",
        loaded.history.iter().map(|h| serde_json::to_string(h).unwrap()).collect::<Vec<_>>());
    // 015：首条为自动上下文消息（user 角色），次条为本轮输入
    assert!(matches!(&loaded.history[0],
        terminal_chat::prompt::HistoryItem::Text(m)
            if m.role == terminal_chat::prompt::HistoryRole::User
                && terminal_chat::context::context_body(&m.content).is_some()));
    assert_eq!(serde_json::to_string(&loaded.history[1]).unwrap(),
        serde_json::to_string(&terminal_chat::prompt::HistoryItem::Text(
            terminal_chat::prompt::HistoryMessage {
                role: terminal_chat::prompt::HistoryRole::User,
                content: "你好".into(),
            })).unwrap());
    // 工具结果为真实 exec 结果结构（第 4 项：ctx→user→tool_calls→result）
    match &loaded.history[3] {
        terminal_chat::prompt::HistoryItem::ToolResult { tool_call_id, content, .. } => {
            assert_eq!(tool_call_id, "call-e1");
            let v: Value = serde_json::from_str(content).unwrap();
            assert_eq!(v["status"], json!("completed"));
            assert_eq!(v["exit_code"], json!(0));
        }
        other => panic!("第 4 条应为工具结果：{other:?}"),
    }
    // 权限：目录 0700、文件 0600
    assert_eq!(unix_mode(&fx.sessions()), 0o700);
    assert_eq!(unix_mode(&auto_path), 0o600);
    // 凭证边界：已配置凭证值不出现在会话文件；用户输入原样保留
    let raw = fs::read_to_string(&auto_path).unwrap();
    assert!(!raw.contains(fx.key), "凭证值不得主动进入会话文件");
    assert!(raw.contains("你好") && raw.contains("再问一次"));

    // 新启动 /resume（无参）：装载完整，首轮请求投影与内存历史逐条一致
    //（015：同 cwd 复跑——上下文内容不变不触发替换注入，投影才逐条一致）
    let (out2, err2, code2) = run_pipe(
        work.path(),
        &fx.envs(),
        "/resume\n续聊\nexit\n",
    );
    assert_eq!(code2, Some(0), "{out2}{err2}");
    assert!(out2.contains(&loaded.summary()), "恢复摘要：{out2}");
    assert_eq!(fx.mock.count(), 4, "两次运行共 4 次请求");
    let body: Value = serde_json::from_str(&fx.mock.requests()[3].body).unwrap();
    let expected = build_request(&loaded.history, "续聊").unwrap();
    assert_eq!(body["messages"], expected["messages"], "首轮请求投影与恢复历史逐条一致");
    // 本轮新记录也自动落盘（同文件续写）
    let loaded2 = load_session_file(&auto_path).unwrap();
    assert_eq!(loaded2.history.len(), 9);
    assert_eq!(loaded2.history[7],
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            role: terminal_chat::prompt::HistoryRole::User,
            content: "续聊".into(),
        }));
}

/// 请求失败的轮次其 user 记录同样落盘（006 t8 语义）。
#[test]
fn e3_failed_request_turn_user_record_persisted() {
    let fx = Fixture::new(vec![MockResp::json(500, "{\"error\":{\"message\":\"boom\"}}")]);
    let work = TempDir::new("sc-e3");
    let (out, _err, code) = run_pipe(work.path(), &fx.envs(), "这轮会失败\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("错误："), "{out}");
    wait_until(|| fx.autos().len() == 1);
    let loaded = load_session_file(&fx.sessions().join(&fx.autos()[0])).unwrap();
    assert_eq!(loaded.history.len(), 2, "失败轮 user 记录已落盘：{:?}",
        loaded.history.iter().map(|h| serde_json::to_string(h).unwrap()).collect::<Vec<_>>());
    assert!(matches!(&loaded.history[0],
        terminal_chat::prompt::HistoryItem::Text(m)
            if m.role == terminal_chat::prompt::HistoryRole::User
                && terminal_chat::context::context_body(&m.content).is_some()),
        "015：自动上下文消息先于 user 入史");
    assert!(matches!(&loaded.history[1],
        terminal_chat::prompt::HistoryItem::Text(m)
            if m.role == terminal_chat::prompt::HistoryRole::User && m.content == "这轮会失败"));
}

// ---------- E4/E5/E13：命令面 ----------

/// /sessions 列表（名称＋字节数；空/多项）；列表不打开正文（chmod 000 仍列出）；
/// 命令不触发联网；未知命令帮助行含三命令；非斜杠输入仍按聊天处理。
#[test]
fn e5_sessions_list_and_command_surface() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("回复"))]);
    let work = TempDir::new("sc-e5");
    // 空列表
    let (out, _e, code) = run_pipe(work.path(), &fx.base_envs(), "/sessions\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("暂无会话"), "{out}");
    // 无参 /resume 无 auto 会话 → 明确报错
    let (out, _e, _c) = run_pipe(work.path(), &fx.base_envs(), "/resume\nexit\n");
    assert!(out.contains("错误：") && out.contains("自动会话"), "{out}");

    // 造两个会话文件（一个 auto 一个快照）＋一次真实对话
    std::fs::create_dir_all(fx.sessions()).unwrap();
    fs::write(fx.sessions().join("auto-20260101T000000-1.jsonl"), "{\"v\":1}\n{\"x\":1}\n").unwrap();
    fs::write(fx.sessions().join("manual-snap.jsonl"), "{\"v\":1}\n").unwrap();
    let (out, _e, code) = run_pipe(work.path(), &fx.envs(), "聊天一句\n/sessions\n/bogus\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Agent：回复"), "非斜杠输入仍按聊天处理：{out}");
    // 列表含两文件与字节数（auto-2026...-<pid> 是本运行新会话；快照 8 字节）
    assert!(out.contains("manual-snap  8 字节"), "{out}");
    // chmod 000 某会话正文后列表仍正常（行为性证明不读正文）
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(fx.sessions().join("manual-snap.jsonl"),
        fs::Permissions::from_mode(0o000)).unwrap();
    let out_chmod = run_pipe(work.path(), &fx.base_envs(), "/sessions\nexit\n").0;
    assert!(out_chmod.contains("manual-snap"), "chmod 000 仍可列出：{out_chmod}");
    // 帮助行含三命令（用含 /bogus 的 run2 输出）
    assert!(
        out.contains("/save <名称>") && out.contains("/resume [名称]") && out.contains("/sessions"),
        "{out}"
    );
    // 命令轮零联网（本测试 mock 仅响应聊天一句）
    assert_eq!(fx.mock.count(), 1, "命令不得触发联网");
}

/// 无参 /resume 取字典序最大 auto 文件。
#[test]
fn e4_resume_no_arg_picks_lexicographically_latest() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("ok"))]);
    let work = TempDir::new("sc-e4");
    std::fs::create_dir_all(fx.sessions()).unwrap();
    fs::write(fx.sessions().join("auto-20260101T000000-1.jsonl"),
        "{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"旧\"}\n").unwrap();
    fs::write(fx.sessions().join("auto-20260601T120000-2.jsonl"),
        "{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"新\"}\n").unwrap();
    let (out, _e, code) = run_pipe(work.path(), &fx.envs(), "/resume\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("恢复 1 条"), "{out}");
    assert!(out.contains("（跳过 0 行，修复 0 处）"), "{out}");
    // 装载的是字典序最大者（"新"）——通过续聊请求投影验证
    let (out, _e, _c) = run_pipe(work.path(), &fx.envs(), "/resume\n验证\nexit\n");
    let body: Value = serde_json::from_str(&fx.mock.requests()[0].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[1]["content"], json!("新"), "无参 resume 取字典序最大 auto：{out}");
}

// ---------- E6/E7：快照与完整上下文往返 ----------

/// 快照→auto 完整上下文往返：/resume 快照（两轮历史）→聊两轮→exit→新启动
/// /resume（无参，取新 auto 文件）→历史＝快照前缀＋新轮全部记录；快照字节不变。
#[test]
fn e7_snapshot_to_auto_full_context_roundtrip() {
    let fx = Fixture::new(vec![
        MockResp::json(200, chat_body("新轮一回复")),
        MockResp::json(200, chat_body("新轮二回复")),
        MockResp::json(200, chat_body("验证轮")),
    ]);
    // 预置快照：两轮历史
    let snap_lines = format!(
        "{{\"v\":1}}\n{}\n{}\n{}\n{}\n",
        r#"{"type":"text","role":"user","content":"旧问一"}"#,
        r#"{"type":"text","role":"assistant","content":"旧答一"}"#,
        r#"{"type":"text","role":"user","content":"旧问二"}"#,
        r#"{"type":"text","role":"assistant","content":"旧答二"}"#,
    );
    std::fs::create_dir_all(fx.sessions()).unwrap();
    let snap = fx.sessions().join("snap.jsonl");
    fs::write(&snap, &snap_lines).unwrap();

    // 运行一：恢复快照→聊两轮→exit
    let work = TempDir::new("sc-e7-run1");
    let (out, _e, code) = run_pipe(work.path(), &fx.envs(), "/resume snap\n新轮一\n新轮二\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("恢复 4 条"), "{out}");
    assert!(out.contains("Agent：新轮一回复") && out.contains("Agent：新轮二回复"), "{out}");
    // 快照文件字节全程不变
    assert_eq!(fs::read_to_string(&snap).unwrap(), snap_lines);
    // 本运行的新 auto 文件以完整上下文开篇
    let autos: Vec<String> = fx.autos().into_iter().filter(|n| n != "auto-20260101T000000-1.jsonl").collect();
    let run1_auto = fx.sessions().join(&autos[0]);
    let loaded = load_session_file(&run1_auto).unwrap();
    assert_eq!(loaded.history.len(), 9, "快照 4 条＋新轮 5 条（含 015 上下文）");

    // 运行二：无参 /resume 取新 auto 文件（字典序最大）→ 首轮请求投影逐条一致
    //（015：同 cwd 复跑——上下文内容不变不触发替换注入）
    let (out2, _e, code2) = run_pipe(work.path(), &fx.envs(), "/resume\n验证轮输入\nexit\n");
    assert_eq!(code2, Some(0), "{out2}");
    let body: Value = serde_json::from_str(&fx.mock.requests()[2].body).unwrap();
    let expected = build_request(&loaded.history, "验证轮输入").unwrap();
    assert_eq!(body["messages"], expected["messages"], "历史＝快照前缀＋新轮全部记录");
}

/// 空历史 /save 往返：仅 meta 快照合法；/resume 该快照＝空会话。
#[test]
fn e14_empty_snapshot_roundtrip() {
    let fx = Fixture::new(vec![]);
    let work = TempDir::new("sc-e14");
    let (out, _e, code) = run_pipe(work.path(), &fx.base_envs(), "/save empty\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("已保存快照 empty（0 条记录）"), "{out}");
    assert_eq!(fs::read_to_string(fx.sessions().join("empty.jsonl")).unwrap(), "{\"v\":1}\n");
    let (out, _e, _c) = run_pipe(work.path(), &fx.base_envs(), "/resume empty\nexit\n");
    assert!(out.contains("恢复 0 条（跳过 0 行，修复 0 处）"), "{out}");
    // 空会话恢复后未产生新记录即退出：不创建 auto 文件
    assert!(fx.autos().is_empty(), "未产生新记录不创建 auto 文件");
}

// ---------- E10：中断后副作用未知（零重放） ----------

/// 文件含 user→assistant(exec touch probe)→中断（无结果）；probe 已存在（副作用
/// 已发生）→装载修复：合成结果文案含"执行结果未知"且不含"未执行/取消"；首轮请求
/// 该调用携带该结果；probe 不被再次触碰（零重放）。
#[test]
fn e10_interrupted_side_effect_unknown_zero_replay() {
    let probe_dir = TempDir::new("sc-e10");
    let probe = probe_dir.path().join("probe.txt");
    fs::write(&probe, "EXISTING-CONTENT").unwrap();
    let before = fs::metadata(&probe).unwrap().modified().unwrap();

    let fx = Fixture::new(vec![MockResp::json(200, chat_body("知道了"))]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    fs::write(
        fx.sessions().join("auto-20260101T000000-1.jsonl"),
        format!(
            "{{\"v\":1}}\n{}\n{}\n",
            r#"{"type":"text","role":"user","content":"请 touch probe"}"#,
            json!({
                "type": "tool_calls",
                "tool_calls": [{ "id": "call-e10", "name": "exec_command",
                    "arguments": json!({ "cmd": format!("echo PWNED > {}", probe.display()) }).to_string() }],
                "content": null
            }).to_string()
        ),
    )
    .unwrap();

    let work = TempDir::new("sc-e10-run");
    let (out, _e, code) = run_pipe(
        work.path(),
        &fx.envs(),
        "/resume auto-20260101T000000-1\n看看结果\nexit\n",
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("恢复 3 条（跳过 0 行，修复 1 处）"), "{out}");
    // 首轮请求：该调用携带合成结果（通用未知错误结构）
    let body: Value = serde_json::from_str(&fx.mock.requests()[0].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let tool_msg = msgs.iter().find(|m| m["role"] == json!("tool")).expect("应有 tool 消息");
    assert_eq!(tool_msg["tool_call_id"], json!("call-e10"));
    let content = tool_msg["content"].as_str().unwrap();
    assert!(content.contains("执行结果未知"), "{content}");
    assert!(!content.contains("未执行") && !content.contains("取消"), "禁言确定性结论：{content}");
    // 零重放：probe 内容与 mtime 不变，未被再次触碰
    assert_eq!(fs::read_to_string(&probe).unwrap(), "EXISTING-CONTENT");
    assert_eq!(fs::metadata(&probe).unwrap().modified().unwrap(), before);
}

// ---------- E8/E9：双进程独占锁 ----------

/// 双进程冲突降级：A /resume auto-X（持锁续写）；B 同名 /resume→获锁失败：
/// 只读装载＋提示"正被另一进程编辑"＋B 的新记录写 B 自己的新 auto 文件（完整
/// 上下文开篇）；A、B 文件各自有效互不损坏；A 退出后新进程可正常续写 auto-X。
#[test]
fn e8_two_process_lock_conflict_degrades() {
    let fx = Fixture::new(vec![
        MockResp::json(500, "{\"error\":{\"message\":\"B 的请求失败也无妨\"}}"),
        MockResp::json(200, chat_body("C-续写")),
    ]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    let target = "auto-20260101T000000-1.jsonl";
    let original = "{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"原始记录\"}\n";
    fs::write(fx.sessions().join(target), original).unwrap();

    // A：恢复并持锁驻留（不产生新记录，锁持有至退出）
    let work_a = TempDir::new("sc-e8-a");
    let mut a = LivePipe::spawn(work_a.path(), &fx.envs());
    a.send("/resume auto-20260101T000000-1\n");
    a.wait_for("恢复 1 条"); // A 装载完成＝已持锁，之后 B 才会撞锁

    // B：同名恢复→降级只读＋提示；随后聊天一轮（请求 500：user 记录仍落盘到 B 自己的新文件）
    let work_b = TempDir::new("sc-e8-b");
    let (out_b, _e, code_b) = run_pipe(work_b.path(), &fx.envs(), "/resume auto-20260101T000000-1\nB 的新消息\nexit\n");
    assert_eq!(code_b, Some(0), "{out_b}");
    assert!(out_b.contains("该会话正被另一进程编辑"), "{out_b}");
    assert!(out_b.contains("恢复 1 条"), "{out_b}");
    // 原文件零修改（A 未产生新记录；B 降级不写原文件）
    assert_eq!(fs::read_to_string(fx.sessions().join(target)).unwrap(), original);

    // A 退出（排空无待写）；此后新进程可正常获锁续写
    let (out_a, code_a) = a.finish();
    assert_eq!(code_a, Some(0), "{out_a}");
    let work_c = TempDir::new("sc-e8-c");
    let (out_c, _e, code_c) = run_pipe(work_c.path(), &fx.envs(), "/resume auto-20260101T000000-1\nC 续写\nexit\n");
    assert_eq!(code_c, Some(0), "{out_c}");
    assert!(out_c.contains("Agent：C-续写"), "{out_c}");
    assert!(!out_c.contains("正被另一进程编辑"), "{out_c}");
    // auto-X 最终内容：原始记录＋C 轮（015 上下文＋user＋assistant）——A/B 均未损坏它
    let loaded = load_session_file(&fx.sessions().join(target)).unwrap();
    assert_eq!(loaded.history.len(), 4, "{:?}",
        loaded.history.iter().map(|h| serde_json::to_string(h).unwrap()).collect::<Vec<_>>());
    // B 的新记录在 B 自己的新 auto 文件，且以完整上下文开篇（原始记录＋015 上下文＋B 新消息）
    let b_auto: Vec<String> = fx.autos().into_iter().filter(|n| n != target).collect();
    assert_eq!(b_auto.len(), 1, "B 恰创建一个自己的 auto 文件：{b_auto:?}");
    let b_loaded = load_session_file(&fx.sessions().join(&b_auto[0])).unwrap();
    assert_eq!(b_loaded.history.len(), 3, "完整上下文开篇：{:?}",
        b_loaded.history.iter().map(|h| serde_json::to_string(h).unwrap()).collect::<Vec<_>>());
    assert!(matches!(&b_loaded.history[0],
        terminal_chat::prompt::HistoryItem::Text(m)
            if m.role == terminal_chat::prompt::HistoryRole::User && m.content == "原始记录"));
    assert!(matches!(&b_loaded.history[1],
        terminal_chat::prompt::HistoryItem::Text(m)
            if m.role == terminal_chat::prompt::HistoryRole::User
                && terminal_chat::context::context_body(&m.content).is_some()),
        "015：上下文消息随 B 轮入史");
    assert!(matches!(&b_loaded.history[2],
        terminal_chat::prompt::HistoryItem::Text(m)
            if m.role == terminal_chat::prompt::HistoryRole::User && m.content == "B 的新消息"));
}

/// 写者半行暂停（受控夹具：测试进程扮演暂停中的写者——持锁＋已写无换行半行）：
/// B 同名 /resume→获锁失败降级只读，原文件字节零修改（半行原样保留、不补 \n、
/// 不截断）；写者补完整行并释放后，auto-X 记录完整有效、可再次装载。
#[test]
fn e9_writer_halfline_pause_degraded_readonly_zero_modification() {
    let fx = Fixture::new(vec![]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    let target = "auto-20260101T000000-1.jsonl";
    let path = fx.sessions().join(target);
    let half = "{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"半".as_bytes();
    {
        let mut f = std::fs::OpenOptions::new().append(true).create(true).read(true).open(&path).unwrap();
        assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        f.write_all(half).unwrap();
        // 持锁期间触发 B 的降级读
        let work = TempDir::new("sc-e9-b");
        let (out, _e, code) = run_pipe(work.path(), &fx.envs(), "/resume auto-20260101T000000-1\nexit\n");
        assert_eq!(code, Some(0), "{out}");
        assert!(out.contains("该会话正被另一进程编辑"), "{out}");
        assert!(out.contains("恢复 0 条（跳过 1 行，修复 0 处）"), "半行按坏行跳过：{out}");
        assert_eq!(fs::read(&path).unwrap(), half.to_vec(), "原文件字节零修改（不补 \\n、不截断）");
        // 写者恢复执行：补完整行（记录合法闭合）后释放锁
        let tail = "写完成\"}".as_bytes();
        let mut f2 = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f2.write_all(tail).unwrap();
        f2.write_all(b"\n").unwrap();
    }
    // auto-X 记录完整有效、可再次装载（新进程正常恢复）
    let work2 = TempDir::new("sc-e9-c");
    let (out, _e, code) = run_pipe(work2.path(), &fx.envs(), "/resume auto-20260101T000000-1\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("恢复 1 条（跳过 0 行，修复 0 处）"), "{out}");
}

// ---------- E11/E12：退出排空失败与名称安全 ----------

/// 退出排空失败打印明确错误、退出码不变（RLIMIT 使追加失败；成功部分已提交）。
#[test]
fn e12_exit_flush_failure_prints_error_exit_code_unchanged() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("回复正文"))]);
    let work = TempDir::new("sc-e12");
    // 预初始化配置目录（首轮 CLI 会写 auto_catalog.json，受限进程会写失败）
    let init = TempDir::new("sc-e12-init");
    let (_io, _ie, _ic) = run_pipe(init.path(), &fx.base_envs(), "/models\nexit\n");
    // 预置 auto 会话（含 1 条已提交记录），软限＝其长度：任何追加立即 EFBIG
    std::fs::create_dir_all(fx.sessions()).unwrap();
    let target = "auto-20260101T000000-1.jsonl";
    let seeded = "{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"已有\"}\n";
    fs::write(fx.sessions().join(target), seeded).unwrap();
    let limit = seeded.len() as u64;

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.current_dir(work.path()).env("TERM", "xterm");
    for k in ["TCHAT_USER_DIR", "TCHAT_CONFIG_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
        cmd.env_remove(k);
    }
    for (k, v) in fx.envs() {
        cmd.env(k, v);
    }
    unsafe {
        cmd.pre_exec(move || {
            let lim = libc::rlimit { rlim_cur: limit, rlim_max: libc::RLIM_INFINITY };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &lim) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            Ok(())
        });
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("受限 CLI 启动失败");
    child.stdin.take().unwrap().write_all("/resume auto-20260101T000000-1\n新消息\nexit\n".as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "退出码不变（001 契约）：{stdout}");
    assert!(stdout.contains("再见!"), "{stdout}");
    assert!(stdout.contains("自动会话追加失败") || stdout.contains("排空失败"), "失败明确报错：{stdout}");
    // 文件＝已提交记录（截回后无半行、无新记录）
    assert_eq!(fs::read_to_string(fx.sessions().join(target)).unwrap(), seeded);
}

/// 名称安全（CLI 级）：非法名称/保留前缀拒绝且 sessions 目录外零文件。
#[test]
fn e11_name_safety_cli_level() {
    let fx = Fixture::new(vec![]);
    let work = TempDir::new("sc-e11");
    let user_root = fs::read_dir(fx.user.path()).unwrap().count();
    for bad in ["/save ../escape", "/save /abs/path", "/save auto-1", "/resume ../x"] {
        let (out, _e, code) = run_pipe(work.path(), &fx.base_envs(), &format!("{bad}\nexit\n"));
        assert_eq!(code, Some(0), "{bad}：{out}");
        assert!(out.contains("错误："), "{bad} 应明确报错：{out}");
    }
    // 空名称：用法提示（非错误），同样零文件操作
    let (out, _e, _c) = run_pipe(work.path(), &fx.base_envs(), "/save \nexit\n");
    assert!(out.contains("用法：/save"), "{out}");
    // user 根下仍只有 sessions 子目录（无逃逸文件）
    let mut entries: Vec<String> = fs::read_dir(fx.user.path()).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok()).collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["authorized-origins.json".to_string()],
        "非法名称零文件操作：仅剩 fixture 预置的授权记录，sessions 都未创建：{entries:?}"
    );
    let _ = user_root;
}

// ---------- PTY：覆盖/丢弃确认 ----------

/// PTY 确认流：快照覆盖 y 生效、n 拒绝；非空历史丢弃（含续写目标切换）y 生效、
/// n 拒绝；管道模式均拒绝。
/// 008-ADAPT：覆盖/丢弃的 y/n 交互确认仅存在于行式路径——TUI 内 /save、/resume
/// 不经母本命令注册表即被 composer 拒为 Unrecognized command（PTY 实测），确认流
/// 无法在 TUI 内触发。以 TERM=dumb 走 008 既有行式退化路径（tui_should_run 判否）
/// 保留原交互契约验证，007 语义不变。
#[test]
fn e6_pty_confirm_overwrite_and_discard() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("第一轮"))]);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.env("TERM", "dumb"); // 008-ADAPT：行式退化路径（原 xterm→行式）
    for (k, v) in fx.envs() {
        cmd.env(k, v);
    }
    let mut pty = PtyCli::spawn(cmd);
    pty.wait("你> ");
    pty.send("第一条");
    pty.wait("Agent：第一轮");
    // /save 名称两次：第二次询问覆盖，n 拒绝（文件保持第一次内容）
    pty.send("/save mine");
    pty.wait("已保存快照 mine（3 条记录）");
    pty.send("/save mine");
    pty.wait("快照 mine 已存在，覆盖？");
    pty.send("n");
    pty.wait("已取消保存");
    // y 覆盖生效
    pty.send("/save mine");
    pty.wait("快照 mine 已存在，覆盖？");
    pty.send("y");
    pty.wait("已保存快照 mine（3 条记录）");
    // 非空历史丢弃：/resume 需确认，n 拒绝（历史保留），y 生效
    pty.send("/resume mine");
    pty.wait("恢复会话将丢弃当前 3 条历史记录，继续？");
    pty.send("n");
    pty.wait("已取消恢复");
    pty.send("/resume mine");
    pty.wait("恢复会话将丢弃当前 3 条历史记录，继续？");
    pty.send("y");
    pty.wait("恢复 3 条（跳过 0 行，修复 0 处）");
    pty.send("exit");
    pty.wait("再见!");
    let (out, code) = pty.finish();
    assert_eq!(code, Some(0), "{out}");

    // 管道模式（非交互）：非空历史恢复拒绝；快照覆盖拒绝（新名称不受限）
    let work = TempDir::new("sc-e6-pipe");
    let (out, _e, _c) = run_pipe(work.path(), &fx.envs(), "你好\n/resume mine\n/save mine\n/save fresh\nexit\n");
    assert!(out.contains("非交互模式下恢复会话要求当前历史为空"), "{out}");
    assert!(out.contains("非交互模式下拒绝覆盖已有快照"), "{out}");
    assert!(out.contains("已保存快照 fresh"), "新名称不受限：{out}");
}


/// 路径边界（软链接逃逸）：sessions 下的 auto-*.jsonl 指向目录外文件——
/// canonicalize 包含性复核必须拒绝（锁/装载/追加都不得触及目标文件）。
#[test]
fn e15_symlink_escape_rejected() {
    let fx = Fixture::new(vec![]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    let outside = TempDir::new("sc-e15-out");
    let victim = outside.path().join("victim.jsonl");
    fs::write(&victim, "{\"v\":1}\n").unwrap();
    std::os::unix::fs::symlink(&victim, fx.sessions().join("auto-link-1.jsonl")).unwrap();

    let work = TempDir::new("sc-e15");
    let (out, _e, code) = run_pipe(work.path(), &fx.envs(), "/resume auto-link-1\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("错误：") && out.contains("逃出 sessions 目录"), "{out}");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "{\"v\":1}\n", "目录外文件零触碰");
}

/// 切换会话前排空失败中止恢复（§1 落盘集合＝内存集合）：受限进程追加失败产生
/// 待写段后 /resume 确认丢弃——排空仍失败→中止恢复并明确报错，退出码不变。
/// 008-ADAPT：丢弃确认 y/n 与排空中止仅存在于行式路径——TUI 内 /resume 不经
/// 母本命令注册表即被 composer 拒为 Unrecognized command（PTY 实测）。以
/// TERM=dumb 走 008 既有行式退化路径保留原契约验证。
#[test]
fn e16_resume_switch_aborts_when_drain_fails() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("受限回复"))]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    let target = "auto-20260101T000000-1.jsonl";
    let seeded = "{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"已有\"}\n";
    fs::write(fx.sessions().join(target), seeded).unwrap();
    fs::write(fx.sessions().join("snap.jsonl"), "{\"v\":1}\n").unwrap();
    let limit = seeded.len() as u64;
    // 预初始化配置目录（避开受限进程写 auto_catalog）
    let init = TempDir::new("sc-e16-init");
    let _ = run_pipe(init.path(), &fx.base_envs(), "/models\nexit\n");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.env("TERM", "dumb"); // 008-ADAPT：行式退化路径（原 xterm→行式）
    for k in ["TCHAT_USER_DIR", "TCHAT_CONFIG_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
        cmd.env_remove(k);
    }
    for (k, v) in fx.envs() {
        cmd.env(k, v);
    }
    unsafe {
        cmd.pre_exec(move || {
            let lim = libc::rlimit { rlim_cur: limit, rlim_max: libc::RLIM_INFINITY };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &lim) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            Ok(())
        });
    }
    let mut pty = PtyCli::spawn(cmd);
    pty.wait("你> ");
    pty.send("/resume auto-20260101T000000-1");
    pty.wait("恢复 1 条");
    pty.wait("你> ");
    pty.send("新消息");
    pty.wait("自动会话追加失败");
    pty.wait("你> ");
    pty.send("/resume snap");
    pty.wait("丢弃当前");
    pty.send("y");
    pty.wait("已中止恢复");
    pty.send("exit");
    pty.wait("再见!");
    let (out, code) = pty.finish();
    assert_eq!(code, Some(0), "{out}");
    // 文件保持已提交记录（无半行）
    assert_eq!(fs::read_to_string(fx.sessions().join(target)).unwrap(), seeded);
}

/// 双进程并发 /save 同名快照（§1：不加锁、末者胜）：两个真实 CLI 各自恢复
/// 不同的大历史后同时保存——最终文件必为某一份完整快照（与某一方种子字节
/// 精确一致）、无混写、无 tmp 残留。
#[test]
fn e17_two_process_concurrent_save_same_snapshot() {
    let fx = Fixture::new(vec![]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    // 两份大历史种子（经生产 save_snapshot 生成，字节即期望快照内容）
    let hist_a: Vec<HistoryItem> = (0..1500)
        .map(|i| HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            role: HistoryRole::User,
            content: format!("A-user-{i:05}-{}", "填充".repeat(120)),
        }))
        .collect();
    let hist_b: Vec<HistoryItem> = (0..1500)
        .map(|i| HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            role: HistoryRole::User,
            content: format!("B-user-{i:05}-{}", "填充".repeat(120)),
        }))
        .collect();
    terminal_chat::session::save_snapshot(&fx.sessions(), "seedA", &hist_a).unwrap();
    terminal_chat::session::save_snapshot(&fx.sessions(), "seedB", &hist_b).unwrap();
    let bytes_a = fs::read(fx.sessions().join("seedA.jsonl")).unwrap();
    let bytes_b = fs::read(fx.sessions().join("seedB.jsonl")).unwrap();

    // 双进程：各自恢复种子（快照只读，历史非空但恢复时目标内存为空→无需确认）
    let work_a = TempDir::new("sc-e17-a");
    let work_b = TempDir::new("sc-e17-b");
    let mut a = LivePipe::spawn(work_a.path(), &fx.base_envs());
    a.send("/resume seedA\n");
    let mut b = LivePipe::spawn(work_b.path(), &fx.base_envs());
    b.send("/resume seedB\n");
    // 双方就绪后同时下发保存（快照写约 1 MB，窗口重叠）
    a.send("/save race\n");
    b.send("/save race\n");
    let (out_a, code_a) = a.finish();
    let (out_b, code_b) = b.finish();
    assert_eq!((code_a, code_b), (Some(0), Some(0)), "{out_a}{out_b}");
    // 管道模式契约：目标已存在即拒绝覆盖——后到者的 exists 检查若落在先到者
    // rename 之后，按契约输出"拒绝覆盖"（非失败）。至少一方成功保存。
    let saved_a = out_a.contains("已保存快照 race");
    let saved_b = out_b.contains("已保存快照 race");
    assert!(
        saved_a || saved_b,
        "至少一方完成保存：{out_a}{out_b}"
    );
    for (tag, out) in [("A", &out_a), ("B", &out_b)] {
        let _ = tag;
        assert!(
            out.contains("已保存快照 race") || out.contains("拒绝覆盖") || out.is_empty(),
            "每方结果只能是保存成功或时序性拒绝覆盖：{out}"
        );
    }

    // 最终文件＝某一方完整快照（字节精确）、无 tmp 残留
    let final_bytes = fs::read(fx.sessions().join("race.jsonl")).unwrap();
    assert!(
        final_bytes == bytes_a || final_bytes == bytes_b,
        "并发同名保存的最终文件必须是某一方完整快照：长度 {}（A={} B={}）",
        final_bytes.len(),
        bytes_a.len(),
        bytes_b.len()
    );
    let residue: Vec<String> = std::fs::read_dir(fx.sessions()).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(residue.is_empty(), "并发后无 tmp 残留：{residue:?}");
    // 落败方内容仍是有效会话（装载完整）
    let winner_is_a = final_bytes == bytes_a;
    let loaded = terminal_chat::session::load_session_file(&fx.sessions().join("race.jsonl")).unwrap();
    assert_eq!(loaded.history.len(), 1500);
    assert_eq!(
        loaded.history[0],
        HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            role: HistoryRole::User,
            content: format!("{}-user-00000-{}", if winner_is_a { "A" } else { "B" }, "填充".repeat(120)),
        })
    );
}

// ---------- 合并前补测：范围 1（工具输出保真与凭证边界） ----------

/// e18：S（工具输出秘密）三处按序列化层次解码逐字一致；C（配置凭证）不因配置
/// 加载主动进入会话文件或请求正文（认证头使用 C 属正常）；固定空历史与提示
/// 前提下的精确计数（请求 2 次、请求1 messages=2、请求2 messages=4、逻辑记录
/// 4 条、文件 5 行含 meta、0600）。
#[test]
fn e18_tool_secret_fidelity_and_credential_boundary() {
    let secret_dir = TempDir::new("sc-e18-sec");
    let secret_file = secret_dir.path().join("s.txt");
    let s_secret = "TOOLSEC-Ω93\"二行\t\u{1}控制符\n多行";
    fs::write(&secret_file, s_secret).unwrap();
    let fx = Fixture::new(vec![
        MockResp::json(200, tc_exec("call-s", &format!("cat {}", secret_file.display()))),
        MockResp::json(200, chat_body("已读取")),
    ]);
    let work = TempDir::new("sc-e18");
    let (out, _e, code) = run_pipe(work.path(), &fx.envs(), "读取秘密\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Agent：已读取"), "{out}");

    // 网络请求：恰 2 次；messages 计数与内容（空历史＋固定提示前提）
    let reqs = fx.mock.requests();
    assert_eq!(reqs.len(), 2, "工具批＋终文恰 2 请求");
    let body1: Value = serde_json::from_str(&reqs[0].body).unwrap();
    let body2: Value = serde_json::from_str(&reqs[1].body).unwrap();
    let m1 = body1["messages"].as_array().unwrap();
    let m2 = body2["messages"].as_array().unwrap();
    // 015：请求1＝system＋ctx＋user；请求2 再加 assistant 批与 tool 结果
    assert_eq!(m1.len(), 3, "请求1＝system＋ctx＋user");
    assert_eq!(m2.len(), 5, "请求2＝system＋ctx＋user＋assistant 批＋tool 结果");
    assert_eq!(m1[0]["role"], json!("system"), "system 恒首位（提示为编译期固定内容）");
    assert!(m1[1]["content"].as_str().unwrap().starts_with("<environment_context>"), "015 上下文消息");
    assert_eq!(m1[2]["content"], json!("读取秘密"));
    assert_eq!(m2[3]["tool_calls"][0]["id"], json!("call-s"));

    // S 保真（请求层）：tool 消息 content 为字符串，二次解码后 .output == S
    let tool_content = m2[4]["content"].as_str().unwrap();
    let exec_result: Value = serde_json::from_str(tool_content).expect("tool content 为 ExecResult JSON");
    assert_eq!(exec_result["output"].as_str().unwrap(), s_secret, "请求层 .output 与 S 逐字一致");
    assert_eq!(exec_result["exit_code"], json!(0));
    assert_eq!(m2[4]["tool_call_id"], json!("call-s"));

    // S 保真（会话文件层）：6 行（meta＋5 记录，含 015 上下文）；ToolResult 行二次解码一致
    let autos = fx.autos();
    assert_eq!(autos.len(), 1);
    let auto_path = fx.sessions().join(&autos[0]);
    let raw_text = fs::read_to_string(&auto_path).unwrap();
    let raw_lines: Vec<&str> = raw_text.lines().collect();
    assert_eq!(raw_lines.len(), 6, "meta＋5 条记录（含 015 上下文）");
    let rec3: Value = serde_json::from_str(raw_lines[4]).unwrap();
    assert_eq!(rec3["type"], json!("tool_result"));
    let file_exec: Value = serde_json::from_str(rec3["content"].as_str().unwrap()).unwrap();
    assert_eq!(file_exec["output"].as_str().unwrap(), s_secret, "会话文件层 .output 与 S 逐字一致");

    // C 边界：凭证值不在会话文件、不在任何请求正文；认证头含 C 属正常
    let c = fx.key;
    assert!(!raw_text.contains(c), "C 不入会话文件");
    for r in &reqs {
        assert!(!r.body.contains(c), "C 不入请求正文");
    }
    let bearer = format!("Bearer {c}");
    assert_eq!(reqs[0].auth.as_deref(), Some(bearer.as_str()), "认证头正常使用 C（不计入边界）");
    assert_eq!(unix_mode(&auto_path), 0o600);
}

/// e19：未授权两分——(a) 已有 auto：本轮零请求、零新增记录、文件字节不变；
/// (b) 尚无 auto：本轮零请求、零入史、不创建 auto 文件。
#[test]
fn e19_unauthorized_zero_request_and_no_persistence() {
    // (a) 已有 auto 文件 + 指纹不匹配的授权记录
    let cfg = TempDir::new("sc-e19a-cfg");
    let user = TempDir::new("sc-e19a-user");
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("不该出现"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_E19_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    fs::create_dir_all(user.path().join("sessions")).unwrap();
    let seeded_auto = "{\"v\":1}\n{\"type\":\"text\",\"role\":\"user\",\"content\":\"已有记录\"}\n";
    fs::write(user.path().join("sessions").join("auto-a-1.jsonl"), seeded_auto).unwrap();
    // 授权记录指纹与凭证不匹配（换 Key 场景）
    let wrong_fp = catalog::fingerprint("other-key-value");
    fs::write(
        user.path().join("authorized-origins.json"),
        json!({ "grants": [ { "origin": origin, "cred_fp": wrong_fp, "added_at": 0 } ] }).to_string(),
    )
    .unwrap();
    let work = TempDir::new("sc-e19a-run");
    let (out, _e, code) = run_pipe_with_envs(work.path(), &[
        ("TCHAT_CONFIG_DIR", cfg.path().display().to_string()),
        ("TCHAT_USER_DIR", user.path().display().to_string()),
        ("TCHAT_E19_KEY", "e19-key-value".to_string()),
    ], "你好\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("错误："), "未授权必须明确拒绝：{out}");
    assert_eq!(mock.count(), 0, "零请求");
    assert_eq!(
        fs::read_to_string(user.path().join("sessions").join("auto-a-1.jsonl")).unwrap(),
        seeded_auto,
        "已有 auto 文件字节不变（零新增记录）"
    );

    // (b) 尚无 auto：不创建 auto 文件
    let user2 = TempDir::new("sc-e19b-user");
    fs::create_dir_all(user2.path().join("sessions")).unwrap();
    fs::write(
        user2.path().join("authorized-origins.json"),
        json!({ "grants": [ { "origin": origin, "cred_fp": wrong_fp, "added_at": 0 } ] }).to_string(),
    )
    .unwrap();
    let work2 = TempDir::new("sc-e19b-run");
    let (out2, _e2, code2) = run_pipe_with_envs(work2.path(), &[
        ("TCHAT_CONFIG_DIR", cfg.path().display().to_string()),
        ("TCHAT_USER_DIR", user2.path().display().to_string()),
        ("TCHAT_E19_KEY", "e19-key-value".to_string()),
    ], "你好\nexit\n");
    assert_eq!(code2, Some(0), "{out2}");
    assert_eq!(mock.count(), 0, "仍零请求");
    let autos: Vec<String> = fs::read_dir(user2.path().join("sessions")).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.starts_with("auto-"))
        .collect();
    assert!(autos.is_empty(), "零入史：不得创建 auto 文件：{autos:?}");
}

/// 与 run_pipe 相同但接受 String 值环境（e19 直配场景）。
fn run_pipe_with_envs(project: &Path, envs: &[( &str, String )], stdin_text: &str) -> (String, String, Option<i32>) {
    let owned: Vec<(String, String)> = envs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    run_pipe(project, &owned, stdin_text)
}

// ---------- 合并前补测：范围 2（恢复历史与请求上限 CLI 闭环） ----------

const REQ_LIMIT: usize = 10 * 1024 * 1024;

/// 生成全有效短记录会话至 ≥min_bytes 后自然停止，返回（记录数, 实际文件字节数）。
fn gen_session_min(path: &Path, min_bytes: usize, tag: &str) -> (usize, usize) {
    let filler = "边界填充数据0123456789".repeat(2); // 66 B
    let mut buf = String::from("{\"v\"1}\n");
    buf = String::from("{\"v\":1}\n");
    let mut total = buf.len();
    let mut i = 0usize;
    while total < min_bytes {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        let content = format!("{tag}-{i:06}-{filler}");
        let line = json!({ "type": "text", "role": role, "content": content }).to_string();
        buf.push_str(&line);
        buf.push('\n');
        total += line.len() + 1;
        i += 1;
    }
    std::fs::write(path, buf).unwrap();
    (i, total)
}

/// e21：超限轮 CLI 闭环——恢复大会话后聊天：零请求、无重试、错误含 10 MiB、
/// user 按入史路径保存（文件恰＋1 行）；/save、/sessions、再恢复不受影响。
#[test]
fn e21_over_limit_cli_closure_and_independence() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("不应到达"))]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    // 投影（非文件大小）必须超限：行→消息每条省 ~15B，文件取 +3 MiB 余量；
    // 测试内自证前提（独立重算投影 > L）
    let (n, _t_bytes) = gen_session_min(&fx.sessions().join("auto-big-1.jsonl"), REQ_LIMIT + 3 * 1024 * 1024, "big");
    let loaded = terminal_chat::session::load_session_file(&fx.sessions().join("auto-big-1.jsonl")).unwrap();
    let proj = serde_json::to_vec(&terminal_chat::prompt::build_request(&loaded.history, "x").unwrap()).unwrap().len();
    assert!(proj > REQ_LIMIT, "前提自证：投影 {proj} 须超限");
    let work = TempDir::new("sc-e21");
    let (out, _e, code) = run_pipe(work.path(), &fx.envs(), "/resume auto-big-1\n你好\nexit\n");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("错误：") && out.contains("10 MiB"), "超限明确报错：{out}");
    assert_eq!(fx.mock.count(), 0, "零请求、无重试");
    // user 已入史并落盘：文件恰＋2 行（015 上下文＋本轮 user）
    let lines = fs::read_to_string(fx.sessions().join("auto-big-1.jsonl")).unwrap().lines().count();
    assert_eq!(lines, n + 1 + 2, "meta＋{n} 条恢复记录＋本轮 ctx＋user");

    // 独立性：/save、/sessions、再恢复（新进程，空历史无确认门）
    let work2 = TempDir::new("sc-e21-save");
    let (out2, _e2, _c2) = run_pipe(work2.path(), &fx.envs(), "/resume auto-big-1\n/save kept\n/sessions\nexit\n");
    assert!(out2.contains(&format!("已保存快照 kept（{} 条记录）", n + 2)), "{out2}");
    assert!(out2.contains("kept  "), "/sessions 列出快照及字节数：{out2}");
    assert!(out2.contains("恢复 ") && out2.contains("（跳过 0 行，修复 0 处）"), "再恢复 N+1 条：{out2}");
}

/// e22：同等文件字节数、不同有效投影——甲（全有效）超限零请求；乙（含 1.5 MiB
/// 坏行）投影在上限内恰一请求，且最终请求字节＝按恢复投影独立计算的预期值
/// （不以文件大小推断请求大小）。
#[test]
fn e22_same_file_size_different_projection() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("乙可达"))]);
    std::fs::create_dir_all(fx.sessions()).unwrap();
    // 甲：全有效（自然填充，实际字节即 T）；前提自证：甲投影超限、乙投影达限内
    let (_n_a, t_bytes) =
        gen_session_min(&fx.sessions().join("auto-pj-a.jsonl"), REQ_LIMIT + 3 * 1024 * 1024, "pa");
    let loaded_a = terminal_chat::session::load_session_file(&fx.sessions().join("auto-pj-a.jsonl")).unwrap();
    let proj_a = serde_json::to_vec(&terminal_chat::prompt::build_request(&loaded_a.history, "x").unwrap()).unwrap().len();
    assert!(proj_a > REQ_LIMIT, "甲投影须超限：{proj_a}");
    let work_a = TempDir::new("sc-e22-a");
    let (out_a, _ea, _ca) = run_pipe(work_a.path(), &fx.envs(), "/resume auto-pj-a\n你好\nexit\n");
    assert!(out_a.contains("10 MiB"), "甲投影超限：{out_a}");
    assert_eq!(fx.mock.count(), 0);

    // 乙：同文件字节数，含 ≥1.5 MiB 坏行（无效 JSON，装载跳过）
    let mut path_b = fx.sessions().join("auto-pj-b.jsonl");
    let (n_b, valid_bytes) =
        gen_session_min(&path_b, t_bytes - 2 * 1024 * 1024, "pb");
    let bad_len = t_bytes - valid_bytes - 1; // 补齐到与甲完全同字节
    assert!(bad_len >= 1536 * 1024, "坏行 ≥1.5 MiB：{bad_len}");
    {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(&path_b).unwrap();
        f.write_all(vec![b'x'; bad_len].as_slice()).unwrap();
        f.write_all(b"\n").unwrap();
    }
    assert_eq!(fs::metadata(&path_b).unwrap().len() as usize, t_bytes, "两文件字节相等");

    // 乙：投影 < L → 恰一请求；请求字节＝独立计算（投影＋探针测得的固定开销）
    let work_b = TempDir::new("sc-e22-b");
    let t0 = std::time::Instant::now();
    let (out_b, _eb, _cb) = run_pipe(work_b.path(), &fx.envs(), "/resume auto-pj-b\n你好\nexit\n");
    assert!(out_b.contains("（跳过 1 行，修复 0 处）"), "坏行跳过：{out_b}");
    assert!(out_b.contains("Agent：乙可达"), "乙请求可达：{out_b}");
    assert_eq!(fx.mock.count(), 1, "恰一请求");
    let sent: Value = serde_json::from_str(&fx.mock.requests()[0].body).unwrap();
    let sent_len = serde_json::to_vec(&sent).unwrap().len();
    assert!(sent_len <= REQ_LIMIT - 1, "最终请求字节 {sent_len} 应在上限内");
    // 独立核对：装载投影（n_b 条文本记录）重建请求，字节＋固定开销＝实测值
    // CLI 已向乙文件追加本轮 015 上下文＋user＋终文 3 条：核对基准＝运行前 n_b 条＋本轮输入
    let loaded_b = terminal_chat::session::load_session_file(&path_b).unwrap();
    assert_eq!(loaded_b.history.len(), n_b + 3, "装载＝运行前记录＋本轮 ctx＋user＋终文");
    let pre_run = &loaded_b.history[..n_b];
    let proj_b = serde_json::to_vec(&terminal_chat::prompt::build_request(pre_run, "x").unwrap()).unwrap().len();
    assert!(proj_b < REQ_LIMIT, "乙投影须在限内：{proj_b}");
    let expected_messages = terminal_chat::prompt::build_request(pre_run, "你好").unwrap();
    let expected_projection = serde_json::to_vec(&expected_messages).unwrap().len();
    let fixed_overhead = sent_len.saturating_sub(expected_projection);
    assert!(
        fixed_overhead <= 4096,
        "投影 {expected_projection}＋固定开销 {fixed_overhead} 应重组实测 {sent_len}（开销为 model 等固定字段＋015 上下文）"
    );
    // 内容一致性：装载投影逐条一致；恰一条 015 上下文消息位于本轮输入之前
    let msgs = sent["messages"].as_array().unwrap();
    let exp_msgs = expected_messages["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), exp_msgs.len() + 1, "messages 数＝装载投影数＋system＋本轮 user＋一条上下文");
    let last = msgs.len();
    assert_eq!(msgs[0], exp_msgs[0], "system 一致");
    assert_eq!(msgs[last - 2]["role"], json!("user"), "015 上下文为 user 消息");
    assert!(
        msgs[last - 2]["content"].as_str().unwrap().starts_with("<environment_context>"),
        "上下文＝环境块形态（恰在本轮输入之前）"
    );
    assert_eq!(msgs[last - 1], exp_msgs[exp_msgs.len() - 1], "本轮输入在末位");
    for (m, e) in msgs[1..last - 2].iter().zip(exp_msgs[1..].iter()) {
        assert_eq!(m, e, "投影逐条一致");
    }
}

/// UTC 基本格式时间戳 → UNIX 秒（Howard Hinnant days_from_civil）。
fn utc_secs_from_basic(ts: &str) -> i64 {
    let y: i64 = ts[0..4].parse().unwrap();
    let mo: i64 = ts[4..6].parse().unwrap();
    let d: i64 = ts[6..8].parse().unwrap();
    let hh: i64 = ts[9..11].parse().unwrap();
    let mi: i64 = ts[11..13].parse().unwrap();
    let ss: i64 = ts[13..15].parse().unwrap();
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + hh * 3600 + mi * 60 + ss
}

/// e23（范围 3）：可信进程环境注入——合法 TS/PID 得精确文件名；无注入时文件名
/// 匹配真实时钟 UTC 秒级区间与实际子进程 pid；非法 TS 形状按既有规则回退真实时钟。
#[test]
fn e23_session_name_injection_and_fallback() {
    // (a) 合法注入：精确文件名
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("ok"))]);
    let work = TempDir::new("sc-e23a");
    let mut envs = fx.envs();
    envs.retain(|(k, _)| k != "TCHAT_SESSION_TS");
    envs.push(("TCHAT_SESSION_TS".to_string(), "20260101T000000".to_string()));
    envs.push(("TCHAT_SESSION_PID".to_string(), "12345".to_string()));
    let (_o, _e, _c) = run_pipe(work.path(), &envs, "你好\nexit\n");
    assert_eq!(fx.autos(), vec!["auto-20260101T000000-12345.jsonl".to_string()]);

    // (b)/(c) 清除注入（含继承值）后跑长驻子进程：取实际 pid 与 UTC 区间
    for (tag, ts_opt) in [("real", None), ("bad-shape", Some("20260209g"))] {
        let fx2 = Fixture::new(vec![MockResp::json(200, chat_body("ok"))]);
        let work2 = TempDir::new(&format!("sc-e23-{tag}"));
        let mut envs2 = fx2.envs();
        envs2.retain(|(k, _)| k != "TCHAT_SESSION_TS");
        if let Some(ts) = ts_opt {
            envs2.push(("TCHAT_SESSION_TS".to_string(), ts.to_string()));
        }
        let t_start = std::time::SystemTime::now();
        let mut cli = LivePipe::spawn(work2.path(), &envs2);
        let child_pid = cli.child.id();
        cli.send("你好\n");
        cli.wait_for("Agent：ok");
        let t_end = std::time::SystemTime::now();
        let (_out, code) = cli.finish();
        assert_eq!(code, Some(0));
        let autos = fx2.autos();
        assert_eq!(autos.len(), 1, "{tag}：恰一个 auto 文件");
        let name = autos[0].strip_suffix(".jsonl").unwrap();
        let (ts_part, pid_part) = name.rsplit_once('-').unwrap();
        let ts = ts_part.strip_prefix("auto-").unwrap();
        assert_eq!(pid_part.parse::<u32>().unwrap(), child_pid, "{tag}：pid 为实际子进程 pid");
        assert_eq!(ts.len(), 15, "{tag}：UTC 基本格式 15 字符");
        assert_eq!(ts.as_bytes()[8], b'T', "{tag}：T 分隔");
        assert!(ts.bytes().enumerate().all(|(i, b)| i == 8 || b.is_ascii_digit()), "{tag}：数字");
        // UTC 秒级区间校验（文件名时钟与断言同时区同精度，含两端容截断）
        let ts_secs = utc_secs_from_basic(ts);
        let start_secs = t_start.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let end_secs = t_end.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert!(
            ts_secs >= start_secs && ts_secs <= end_secs,
            "{tag}：文件名时间 {ts}（{ts_secs}）须在 [{start_secs}, {end_secs}] UTC 秒区间"
        );
    }
}

// ---------- 合并前补测：范围 6（授权失败、部分写入、并发完整性） ----------

/// 子进程探针（测试二进制自执行）：按 RLIMIT 限额写 k+4096 字节到 scratch，
/// 证明"写过正数字节后失败"（文件恰 k 字节＋write Err），非零字节即刻失败。
fn rlimit_probe_child() {
    let Some(spec_path) = std::env::var_os("TCHAT_CLI_PROBE") else { return };
    let spec: Value = serde_json::from_str(&fs::read_to_string(&spec_path).unwrap()).unwrap();
    let limit = spec["limit"].as_u64().unwrap() as libc::rlim_t;
    let scratch = std::path::PathBuf::from(spec["scratch"].as_str().unwrap());
    unsafe {
        let mut old = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &mut old), 0);
        let lim = libc::rlimit { rlim_cur: limit, rlim_max: old.rlim_max };
        assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &lim), 0);
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
    let mut f = fs::File::create(&scratch).unwrap();
    let data = vec![b'p'; limit as usize + 4096];
    let outcome = f.write_all(&data);
    let size = fs::metadata(&scratch).unwrap().len();
    let report = json!({
        "write_failed": outcome.is_err(),
        "size": size,
        "limit": limit,
    });
    fs::write(spec["out"].as_str().unwrap(), report.to_string()).unwrap();
    std::process::exit(0);
}

fn run_rlimit_probe(limit: u64) -> (bool, u64) {
    let io = TempDir::new("sc-probe6");
    let spec = io.path().join("in.json");
    let out = io.path().join("out.json");
    let scratch = io.path().join("scratch.bin");
    fs::write(&spec, json!({ "limit": limit, "scratch": scratch.display().to_string(), "out": out.display().to_string() }).to_string()).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("rlimit_probe_child_marker")
        .arg("--exact")
        .env("TCHAT_CLI_PROBE", &spec)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + WATCHDOG;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                assert!(st.success(), "探针异常：{st}");
                let v: Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
                return (v["write_failed"].as_bool().unwrap(), v["size"].as_u64().unwrap());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => panic!("探针超时"),
        }
    }
}

#[test]
fn rlimit_probe_child_marker() {
    rlimit_probe_child();
    // 夹具证明：限额 512 下写 512+4096 字节 → 恰写过 512 字节后失败
    let (failed, size) = run_rlimit_probe(512);
    assert!(failed, "write 必须以 EFBIG 失败");
    assert_eq!(size, 512, "写过正数字节（512）后失败，非零字节即刻失败");
}

/// e24（范围 6）：目录占位→替换失败——错误明确、零请求、占位原样、无本轮 tmp 残留。
#[test]
fn e24_grant_placeholder_rename_failure() {
    let fx = Fixture::new(vec![MockResp::json(200, chat_body("不应到达"))]);
    let placeholder = fx.user.path().join("authorized-origins.json");
    let _ = fs::remove_file(&placeholder);
    fs::create_dir(&placeholder).unwrap();
    let work = TempDir::new("sc-e24");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.env("TERM", "xterm").current_dir(work.path());
    for (k, v) in fx.envs() {
        cmd.env(k, v);
    }
    let mut pty = common::pty::PtyCli::spawn(cmd);
    // 008-ADAPT：授权询问发生在 TUI 之前的启动期授权门（无需先发消息触发），
    // 提问文本不变；同意→持久化失败→报错并以退出码 1 终止（不进入 TUI、无 "再见!"）
    pty.wait("允许并记住此授权？");
    pty.send("y");
    pty.wait("错误：");
    let (out, code) = pty.finish();
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(fx.mock.count(), 0, "替换失败必须零请求");
    assert!(placeholder.is_dir(), "占位原样");
    let residue: Vec<String> = fs::read_dir(fx.user.path()).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(residue.is_empty(), "无本轮 tmp 残留：{residue:?}");
    let autos = fx.autos();
    assert!(autos.is_empty(), "零新增对话记录（授权门先于入史）：{autos:?}");
}

/// e25（范围 6）：RLIMIT 部分写入两初始状态——(a) 无旧目标：失败后目标仍不存在、
/// 下次仍需确认；(b) 旧含 A 新增 B 失败：旧文件字节不变、A 仍有效（下次放行）、
/// B 仍需确认；两状态本轮均零请求、零新增记录。夹具先证正字节部分写入。
#[test]
fn e25_grant_partial_write_two_states() {
    use std::os::unix::process::CommandExt;
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("A-轮到它"))]);

    // ---- (a) 无旧目标 ----
    let cfg_a = TempDir::new("sc-e25a-cfg");
    let user_a = TempDir::new("sc-e25a-user");
    write_manual(cfg_a.path(), &mock.url, "env:TCHAT_E25A_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    let run_a = |limit: u64| {
        let work = TempDir::new("sc-e25a-run");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.env("TERM", "xterm").current_dir(work.path())
            .env("TCHAT_CONFIG_DIR", cfg_a.path())
            .env("TCHAT_USER_DIR", user_a.path())
            .env("TCHAT_E25A_KEY", "e25a-key");
        unsafe {
            cmd.pre_exec(move || {
                let lim = libc::rlimit { rlim_cur: limit, rlim_max: libc::RLIM_INFINITY };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &lim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                Ok(())
            });
        }
        let mut pty = common::pty::PtyCli::spawn(cmd);
        // 008-ADAPT：TUI 前启动期授权门——同意→写失败（RLIMIT）→报错退出码 1
        pty.wait("允许并记住此授权？");
        pty.send("y");
        pty.wait("错误：");
        pty.finish()
    };
    // 预初始化配置（免无关写失败），限额 64（0<64<单条授权 JSON）
    let init = TempDir::new("sc-e25a-init");
    let envs_a: Vec<(String, String)> = vec![
        ("TCHAT_CONFIG_DIR".into(), cfg_a.path().display().to_string()),
        ("TCHAT_USER_DIR".into(), user_a.path().display().to_string()),
        ("TCHAT_E25A_KEY".into(), "e25a-key".into()),
    ];
    let _ = run_pipe(init.path(), &envs_a, "/models\nexit\n");
    let (failed, size) = run_rlimit_probe(64);
    assert!(failed && size == 64, "夹具证明：正字节部分写入（{size}）");
    let (out_a, code_a) = run_a(64);
    assert_eq!(code_a, Some(1), "{out_a}"); // 008-ADAPT：启动期授权失败→退出码 1
    assert_eq!(mock.count(), 0, "(a) 零请求");
    assert!(!user_a.path().join("authorized-origins.json").exists(), "(a) 失败后目标仍不存在");
    let autos_a: Vec<String> = fs::read_dir(user_a.path().join("sessions"))
        .map(|rd| rd.filter_map(|e| e.unwrap().file_name().into_string().ok())
            .filter(|n| n.starts_with("auto-")).collect())
        .unwrap_or_default();
    assert!(autos_a.is_empty(), "(a) 零新增对话记录");
    // 下次（无限制）仍需确认
    let work = TempDir::new("sc-e25a-next");
    let (out, _e, _c) = run_pipe(work.path(), &envs_a, "你好\nexit\n");
    assert!(out.contains("错误：") && out.contains("非交互"), "(a) 新授权未持久化，下次仍需确认：{out}");
    assert_eq!(mock.count(), 0);

    // ---- (b) 旧文件含 A，新增 B 失败 ----
    let cfg_b = TempDir::new("sc-e25b-cfg");
    let user_b = TempDir::new("sc-e25b-user");
    write_manual(cfg_b.path(), &mock.url, "env:TCHAT_E25B_KEY");
    let fp_a = catalog::fingerprint("e25b-key-a");
    fs::write(
        user_b.path().join("authorized-origins.json"),
        json!({ "grants": [ { "origin": origin, "cred_fp": fp_a, "added_at": 0 } ] }).to_string(),
    ).unwrap();
    let old_bytes = fs::read_to_string(user_b.path().join("authorized-origins.json")).unwrap();
    let envs_b: Vec<(String, String)> = vec![
        ("TCHAT_CONFIG_DIR".into(), cfg_b.path().display().to_string()),
        ("TCHAT_USER_DIR".into(), user_b.path().display().to_string()),
        ("TCHAT_E25B_KEY".into(), "e25b-key-b".into()),
    ];
    let envs_b_a: Vec<(String, String)> = vec![
        envs_b[0].clone(), envs_b[1].clone(),
        ("TCHAT_E25B_KEY".into(), "e25b-key-a".into()),
    ];
    let init_b = TempDir::new("sc-e25b-init");
    let _ = run_pipe(init_b.path(), &envs_b, "/models\nexit\n");
    // 限额＝旧文件大小＋8：新内容（追加一条授权）必超 → 正字节部分写入
    let limit_b = old_bytes.len() as u64 + 8;
    let (failed_b, size_b) = run_rlimit_probe(limit_b);
    assert!(failed_b && size_b == limit_b, "夹具证明 b：正字节部分写入（{size_b}）");
    let run_b = |limit: u64| {
        let work = TempDir::new("sc-e25b-run");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.env("TERM", "xterm").current_dir(work.path())
            .env("TCHAT_CONFIG_DIR", cfg_b.path())
            .env("TCHAT_USER_DIR", user_b.path())
            .env("TCHAT_E25B_KEY", "e25b-key-b");
        unsafe {
            cmd.pre_exec(move || {
                let lim = libc::rlimit { rlim_cur: limit, rlim_max: libc::RLIM_INFINITY };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &lim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                Ok(())
            });
        }
        let mut pty = common::pty::PtyCli::spawn(cmd);
        // 008-ADAPT：TUI 前启动期授权门——同意→部分写失败（RLIMIT）→报错退出码 1
        pty.wait("允许并记住此授权？");
        pty.send("y");
        pty.wait("错误：");
        pty.finish()
    };
    let (out_b, code_b) = run_b(limit_b);
    assert_eq!(code_b, Some(1), "{out_b}");
    let count_after_fail = mock.count();
    assert_eq!(
        fs::read_to_string(user_b.path().join("authorized-origins.json")).unwrap(),
        old_bytes,
        "(b) 旧授权文件字节不变"
    );
    let residue_b: Vec<String> = fs::read_dir(user_b.path()).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.contains(".tmp.")).collect();
    assert!(residue_b.is_empty(), "(b) 无 tmp 残留");
    let autos_b: Vec<String> = fs::read_dir(user_b.path().join("sessions"))
        .map(|rd| rd.filter_map(|e| e.unwrap().file_name().into_string().ok())
            .filter(|n| n.starts_with("auto-")).collect())
        .unwrap_or_default();
    assert!(autos_b.is_empty(), "(b) 零新增对话记录（授权门先于入史）");
    // 后续：A 放行（无需询问、请求可达）；B 仍需确认
    let work_b1 = TempDir::new("sc-e25b-a");
    let (out_b1, _e1, _c1) = run_pipe(work_b1.path(), &envs_b_a, "你好\nexit\n");
    assert!(out_b1.contains("Agent：A-轮到它"), "(b) A 仍有效（旧文件完好、无需询问）：{out_b1}");
    let work_b2 = TempDir::new("sc-e25b-b");
    let (out_b2, _e2, _c2) = run_pipe(work_b2.path(), &envs_b, "你好\nexit\n");
    assert!(out_b2.contains("错误：") && out_b2.contains("非交互"), "(b) B 仍需确认：{out_b2}");
}

/// e26（范围 6）：并发授权写入——双写者都成功持久化；终态无条件可解析、
/// 完整配对（无交叉混配）、无未同意条目、无本轮 tmp 残留；允许已接受条目丢失。
#[test]
fn e26_concurrent_grant_integrity() {
    let mock_a = spawn_mock(vec![MockResp::json(200, chat_body("并发A-OK"))]);
    let mock_b = spawn_mock(vec![MockResp::json(200, chat_body("并发B-OK"))]);
    let cfg_a = TempDir::new("sc-e26a-cfg");
    let cfg_b = TempDir::new("sc-e26b-cfg");
    let user = TempDir::new("sc-e26-user"); // 两写者共享同一授权文件
    write_manual(cfg_a.path(), &mock_a.url, "env:TCHAT_E26A_KEY");
    write_manual(cfg_b.path(), &mock_b.url, "env:TCHAT_E26B_KEY");
    let origin_a = catalog::origin_of(&mock_a.url).unwrap();
    let origin_b = catalog::origin_of(&mock_b.url).unwrap();
    let fp_a = catalog::fingerprint("e26-key-a");
    let fp_b = catalog::fingerprint("e26-key-b");

    let mk = |cfg: &TempDir, key_var: &str, key: &str, work: &TempDir| -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
        cmd.env("TERM", "xterm").current_dir(work.path())
            .env("TCHAT_CONFIG_DIR", cfg.path())
            .env("TCHAT_USER_DIR", user.path())
            .env(key_var, key);
        cmd
    };
    let work_a = TempDir::new("sc-e26-run-a");
    let work_b = TempDir::new("sc-e26-run-b");
    let mut a = common::pty::PtyCli::spawn(mk(&cfg_a, "TCHAT_E26A_KEY", "e26-key-a", &work_a));
    // 008-ADAPT：授权询问发生在 TUI 之前的启动期授权门（无需先发消息触发）
    a.wait("允许并记住此授权？");
    let mut b = common::pty::PtyCli::spawn(mk(&cfg_b, "TCHAT_E26B_KEY", "e26-key-b", &work_b));
    b.wait("允许并记住此授权？");
    // 两写者同时确认（各自完整快照原子替换，末者胜）
    a.send("y");
    b.send("y");
    // 授权通过后各自进入 TUI（空历史占位＝就绪）
    // 008：启动无占位 cell（footer 提示＝TUI 就绪标记）
    a.wait_screen("? for shortcuts");
    b.wait_screen("? for shortcuts");
    a.send("你好");
    b.send("你好");
    // 008-ADAPT：回复无 "Agent：" 前缀，等待回复原文（屏幕级差分等待）
    a.wait_screen("并发A-OK");
    b.wait_screen("并发B-OK");
    a.send("/exit"); // 008-ADAPT：TUI 退出走 /exit
    a.wait("再见!");
    b.send("/exit");
    b.wait("再见!");
    let (out_a, code_a) = a.finish();
    let (out_b, code_b) = b.finish();
    assert_eq!((code_a, code_b), (Some(0), Some(0)), "{out_a}{out_b}");
    // 两写者都完成了成功持久化＋放行（各自的 mock 各恰一请求）
    assert_eq!(mock_a.count(), 1, "A 写者成功持久化并放行");
    assert_eq!(mock_b.count(), 1, "B 写者成功持久化并放行");

    // 注：两写窗口是否真实重叠属概率（串行时仍须全过不变量）；损坏路径的
    // 检出力由原子写构造关闭，本用例钉住的是无条件不变量＋双写者成功持久化。
    // 终态（无条件）：可解析、完整配对、⊆ 已同意、无 tmp 残留
    let text = fs::read_to_string(user.path().join("authorized-origins.json")).unwrap();
    let store: Value = serde_json::from_str(&text).expect("终态必须可解析");
    let grants = store["grants"].as_array().unwrap();
    assert!(grants.len() == 1 || grants.len() == 2, "1–2 条（允许已接受条目丢失）：{grants:?}");
    let mut seen = std::collections::HashSet::new();
    for g in grants {
        let o = g["origin"].as_str().unwrap();
        let f = g["cred_fp"].as_str().unwrap();
        let is_a = o == origin_a && f == fp_a;
        let is_b = o == origin_b && f == fp_b;
        assert!(is_a || is_b, "完整配对，无交叉混配：{o} {f}");
        assert!(seen.insert((o.to_string(), f.to_string())), "无重复条目");
    }
    let residue: Vec<String> = fs::read_dir(user.path()).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.contains(".tmp.")).collect();
    assert!(residue.is_empty(), "无本轮 tmp 残留：{residue:?}");
}
