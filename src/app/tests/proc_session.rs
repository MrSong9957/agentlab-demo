//! 016 规格 exec_command／write_stdin 集成测试：真实进程、真实输出与副作用。
//! 覆盖：默认可续接、独立总时限、取消与进程寿命分开、PTY 输入与终止、
//! 容量与所有权、输出缓存与增量语义。异步流程用就绪信号与有界等待，
//! 不靠固定睡眠猜执行顺序。

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use terminal_chat::exec::{ExecArgs, ExecStatus, WriteStdinArgs};
use terminal_chat::proc_session::ProcessManager;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tchat-proc-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 采样日志文件大小；未创建视为 0。
fn log_size(path: &PathBuf) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 有界等待条件成立（60ms 轮询）；超时返回最后一次观测值所在错误。
fn wait_until<T: Clone + PartialEq + std::fmt::Debug>(mut probe: impl FnMut() -> T, want: T, what: &str) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = probe();
        if got == want {
            return got;
        }
        if Instant::now() > deadline {
            panic!("等待 {what} 超时（最后观测 {got:?}，期望 {want:?}）");
        }
        std::thread::sleep(Duration::from_millis(60));
    }
}

/// 有界等待条件为真。
fn wait_until_true(mut probe: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !probe() {
        if Instant::now() > deadline {
            panic!("等待 {what} 超时");
        }
        std::thread::sleep(Duration::from_millis(60));
    }
}

fn exec_args(cmd: &str, yield_ms: u64) -> ExecArgs {
    ExecArgs::from_value(&json!({ "cmd": cmd, "yield_time_ms": yield_ms })).unwrap()
}

fn stdin_args(session_id: &str) -> WriteStdinArgs {
    WriteStdinArgs::from_value(&json!({ "session_id": session_id })).unwrap()
}

fn query(mgr: &ProcessManager, session_id: &str) -> terminal_chat::exec::ExecResult {
    mgr.interact(&stdin_args(session_id), None).result
}

// ---------- 默认行为与参数 ----------

/// 工具先返回（running＋句柄），同一个进程继续执行；后续查询不重新启动；
/// 结束返回真实退出码与最终输出；终态取走后记录移除（句柄失效）。
#[test]
fn running_returns_session_process_continues_query_completes() {
    let dir = temp_dir("running-basic");
    // 可计数追加副作用：每执行一次命令，hits.log 增加一行——重启会留下第二行
    let hits = dir.join("hits.log");
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("echo x >> {}; sleep 0.6; echo DONE", hits.display()),
        "yield_time_ms": 250,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running, "{:?}", out.result);
    let sid = out.result.session_id.clone().expect("运行中必须返回句柄");
    assert!(out.result.exit_code.is_none());
    // 进程确已启动：日志恰一行（就绪信号，不猜时序）
    wait_until(
        || fs::read_to_string(&hits).unwrap_or_default().lines().count(),
        1,
        "命令开始执行（hits 恰一行）",
    );
    // 等待窗口过后查询：终态＋最终输出（DONE），退出码 0
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output.trim(), "DONE", "终态返回最后待取输出");
    assert!(r.session_id.is_none(), "终态无可续接会话");
    // 查询不得重新启动命令：追加日志仍恰一行
    assert_eq!(
        fs::read_to_string(&hits).unwrap().lines().count(),
        1,
        "查询不得重新启动命令（启动次数仍为 1）"
    );
    // 记录已移除：同句柄再查询 → 未知句柄错误
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Error);
    assert!(r.error.unwrap().contains("未知或已失效"), "句柄取走后失效");
    let _ = fs::remove_dir_all(&dir);
}

/// 短命令在等待窗口内直接完成：completed＋退出码，不产生会话记录。
#[test]
fn short_command_completes_inline_without_session() {
    let mgr = ProcessManager::new();
    let out = mgr.start(&exec_args("echo hi", 10_000), None);
    assert_eq!(out.result.status, ExecStatus::Completed);
    assert_eq!(out.result.exit_code, Some(0));
    assert_eq!(out.result.output.trim(), "hi");
    assert!(out.result.session_id.is_none());
    assert!(mgr.live_sessions().is_empty(), "短命令不留会话记录");
}

/// 多次查询无旧输出重放：每次只返回尚未取走的新输出。
#[test]
fn incremental_output_without_replay() {
    let mgr = ProcessManager::new();
    let out = mgr.start(&exec_args("echo ONE; sleep 0.8; echo TWO", 300), None);
    assert_eq!(out.result.status, ExecStatus::Running);
    assert_eq!(out.result.output.trim(), "ONE", "先取走第一段");
    let sid = out.result.session_id.unwrap();
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Completed);
    assert_eq!(r.output.trim(), "TWO", "只含新输出，无 ONE 重放");
    assert_eq!(r.exit_code, Some(0));
}

// ---------- 总运行时限 ----------

/// 显式总时限独立生效：无任何后续查询也会到期终止并回收；日志停止增长；
/// 事后查询返回 timeout 状态。
#[test]
fn explicit_timeout_kills_without_any_query() {
    let dir = temp_dir("timeout-noquery");
    let log = dir.join("log");
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("while true; do echo x >> {}; sleep 0.05; done", log.display()),
        "yield_time_ms": 250,
        "timeout_ms": 1500,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running);
    let sid = out.result.session_id.unwrap();
    // 不查询，纯等待：到期由后台计时终止
    wait_until_true(|| log_size(&log) > 10, "日志开始增长");
    std::thread::sleep(Duration::from_millis(2400)); // 跨过 1500ms 时限＋宽限
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Timeout, "{r:?}");
    assert_eq!(r.exit_code, None, "被终止无整数退出码");
    assert!(r.error.as_deref().unwrap().contains("时限"));
    // 终止后日志不再增长（进程组被杀，含后台孙进程）
    let s1 = log_size(&log);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(log_size(&log), s1, "超时终止后停止写入");
    let _ = fs::remove_dir_all(&dir);
}

/// 读取输出不重置时限：时限内发起的查询等待至启动时刻起的截止时间即见
/// timeout（若读取重置计时，死亡会推迟到“读取时刻＋时限”，测量值越界）。
/// 空查询最少等待 5 秒且阻塞到终态——以查询耗时区间证明死亡时刻。
#[test]
fn read_does_not_reset_deadline() {
    let mgr = ProcessManager::new();
    let timeout_ms = 1000u64;
    let args = ExecArgs::from_value(&json!({
        "cmd": "while true; do date +%s%N; sleep 0.05; done",
        "yield_time_ms": 250,
        "timeout_ms": timeout_ms,
    }))
    .unwrap();
    let started = Instant::now();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running);
    let sid = out.result.session_id.unwrap();
    // 在时限窗口内开始读取（t≈启动后 250ms；重置缺陷会把死亡推迟到
    // 250+1000=1250ms 处，本次查询耗时≈1000ms；正确实现≈750ms）。
    std::thread::sleep(Duration::from_millis(250));
    let query_started = Instant::now();
    let r = query(&mgr, &sid);
    let query_elapsed = query_started.elapsed();
    assert_eq!(r.status, ExecStatus::Timeout, "读取不暂停时限：{r:?}");
    // 死亡时刻＝启动＋timeout（±300ms 容差）；查询从 250ms 处开始等待
    let death_from_start = started.elapsed();
    let expect = Duration::from_millis(timeout_ms);
    let tol = Duration::from_millis(300);
    assert!(
        death_from_start >= expect && death_from_start <= expect + tol,
        "时限按启动时刻独立到期：实际死亡 {death_from_start:?}，期望 {expect:?}±{tol:?}"
    );
    assert!(
        query_elapsed < Duration::from_millis(1100),
        "查询未把时限重置为“读取时刻＋时限”：{query_elapsed:?}"
    );
}

/// 大于旧上限（60000）的普通大值时限被接受；u64 毫秒全域在本平台单调时钟
///（秒域 i64）均可安全表示截止时间。u64::MAX 极大值分支已并入
/// `exec_command::exec_p01_huge_timeout_accepted_then_terminate_reaps_group`
///（真正启动的进程携带该时限），参数层边界另见 n2a。
/// “无法安全表示→拒绝执行”分支为防御代码（可逆变异证明其存在）。
#[test]
fn timeout_bounds_accept_large_values() {
    let mgr = ProcessManager::new();
    // 180000 > 旧 60000 上限：合法（不实际等待）
    let args = ExecArgs::from_value(&json!({
        "cmd": "sleep 60",
        "yield_time_ms": 250,
        "timeout_ms": 180000u64,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running, "timeout_ms=180000：{:?}", out.result);
    mgr.close();
}

// ---------- 取消与进程寿命分开 ----------

/// 取消当前等待不杀进程：返回 cancelled＋保留句柄；进程继续写文件；
/// 取消标志复位（下一轮）后仍可查询；显式终止随后生效。
#[test]
fn cancel_wait_keeps_process_alive_and_queryable() {
    let dir = temp_dir("cancel-keeps");
    let log = dir.join("log");
    let mgr = ProcessManager::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_flag = cancel.clone();
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("while true; do echo x >> {}; sleep 0.05; done", log.display()),
        "yield_time_ms": 4000,
    }))
    .unwrap();
    // 等待期间置取消标志（模拟用户取消当前轮）
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        cancel_flag.store(true, Ordering::SeqCst);
    });
    let out = mgr.start(&args, Some(&cancel));
    assert_eq!(out.result.status, ExecStatus::Cancelled, "{:?}", out.result);
    let sid = out.result.session_id.expect("取消后仍保留句柄");
    assert!(out.result.error.as_deref().unwrap().contains("仍在运行"));
    // 进程继续运行：日志持续增长
    wait_until_true(|| log_size(&log) > 30, "取消后进程继续写日志");
    // 取消标志复位（下一用户轮），不影响保留的进程
    cancel.store(false, Ordering::SeqCst);
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Running, "复位取消不影响保留进程");
    assert_eq!(r.session_id.as_deref(), Some(sid.as_str()));
    // 显式终止收尾
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid, "terminate": true })).unwrap();
    let r = mgr.interact(&term, None).result;
    assert_eq!(r.status, ExecStatus::Terminated, "{r:?}");
    let s1 = log_size(&log);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(log_size(&log), s1, "终止后停止写入");
    let _ = fs::remove_dir_all(&dir);
}

/// 取消发生在启动前：不启动进程（零命令副作用），返回未执行原因与空句柄。
#[test]
fn cancel_before_start_skips_execution() {
    let dir = temp_dir("cancel-before");
    let probe = dir.join("probe");
    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(true);
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("touch {}", probe.display()),
        "yield_time_ms": 250,
    }))
    .unwrap();
    let out = mgr.start(&args, Some(&cancel));
    assert_eq!(out.result.status, ExecStatus::Cancelled);
    assert!(out.result.session_id.is_none(), "未执行＝空句柄");
    assert!(out.result.error.unwrap().contains("未执行"));
    assert!(!probe.exists(), "取消发生在启动前不得产生副作用");
    assert!(mgr.live_sessions().is_empty());
    let _ = fs::remove_dir_all(&dir);
}

/// 取消不暂停总时限：取消当前轮后，显式时限仍在后台到期并终止。
#[test]
fn deadline_survives_cancel() {
    let dir = temp_dir("deadline-cancel");
    let log = dir.join("log");
    let mgr = ProcessManager::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel.clone();
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("while true; do echo x >> {}; sleep 0.05; done", log.display()),
        "yield_time_ms": 4000,
        "timeout_ms": 1200,
    }))
    .unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        cancel_for_thread.store(true, Ordering::SeqCst);
    });
    let out = mgr.start(&args, Some(&cancel));
    assert_eq!(out.result.status, ExecStatus::Cancelled);
    let sid = out.result.session_id.unwrap();
    cancel.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(2000)); // 跨过时限
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Timeout, "取消不暂停时限：{r:?}");
    let _ = fs::remove_dir_all(&dir);
}

// ---------- 输入与控制（PTY） ----------

/// 真实 PTY 可交互：程序检测到终端、发出提示；发送回答（含换行）后继续执行。
#[test]
fn pty_interactive_read_answer() {
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": "printf 'NAME: '; read name; echo \"HELLO $name\"",
        "yield_time_ms": 5000,
        "tty": true,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running, "{:?}", out.result);
    let sid = out.result.session_id.unwrap();
    assert!(out.result.output.contains("NAME:"), "提示已收集：{:?}", out.result.output);
    // 发送回答（显式换行）
    let input = WriteStdinArgs::from_value(&json!({
        "session_id": sid, "chars": "world\n", "yield_time_ms": 5000
    }))
    .unwrap();
    let r = mgr.interact(&input, None).result;
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(r.exit_code, Some(0));
    assert!(r.output.contains("HELLO world"), "回答送达原程序：{:?}", r.output);
}

/// PTY 中程序确认 isatty。
#[test]
fn pty_child_sees_terminal() {
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": "if [ -t 0 ]; then echo ISATTY; else echo NOTTY; fi",
        "yield_time_ms": 10_000,
        "tty": true,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Completed);
    assert!(out.result.output.contains("ISATTY"), "{:?}", out.result.output);
    // 管道模式（tty=false）不是终端
    let out = mgr.start(&exec_args("if [ -t 0 ]; then echo ISATTY; else echo NOTTY; fi", 10_000), None);
    assert_eq!(out.result.status, ExecStatus::Completed);
    assert!(out.result.output.contains("NOTTY"), "{:?}", out.result.output);
}

/// 管道会话拒绝文字输入，但原会话仍可查询；仅 Ctrl+C（\u0003）以中断信号发送。
#[test]
fn pipe_rejects_text_input_session_still_queryable() {
    let mgr = ProcessManager::new();
    let out = mgr.start(&exec_args("sleep 30", 250), None);
    let sid = out.result.session_id.unwrap();
    let input = WriteStdinArgs::from_value(&json!({
        "session_id": sid, "chars": "hello", "yield_time_ms": 5000
    }))
    .unwrap();
    let r = mgr.interact(&input, None).result;
    assert_eq!(r.status, ExecStatus::Error, "{r:?}");
    assert!(r.error.as_deref().unwrap().contains("不支持文字输入"));
    assert_eq!(r.session_id.as_deref(), Some(sid.as_str()), "会话不受参数错误影响");
    // 仍可查询
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Running, "会话未被破坏");
    // 收尾
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid, "terminate": true })).unwrap();
    assert_eq!(mgr.interact(&term, None).result.status, ExecStatus::Terminated);
}

/// 管道 Ctrl+C：可中断的进程收到 SIGINT 退出；忽略中断的进程仍运行，
/// 随后 terminate=true 终止整个受管理进程组。
#[test]
fn pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process() {
    let dir = temp_dir("ctrlc-term");
    let log = dir.join("log");
    let mgr = ProcessManager::new();

    // ① 可中断：while 循环进程组收到 SIGINT 后退出
    let out = mgr.start(&exec_args("while true; do echo x; sleep 0.1; done", 250), None);
    let sid = out.result.session_id.unwrap();
    let int = WriteStdinArgs::from_value(&json!({
        "session_id": sid, "chars": "\u{0003}", "yield_time_ms": 5000
    }))
    .unwrap();
    let r = mgr.interact(&int, None).result;
    assert_eq!(r.status, ExecStatus::Completed, "SIGINT 退出按自然结束：{r:?}");
    assert_eq!(r.exit_code, None, "信号退出无整数退出码，不编造 0");

    // ② 忽略中断：trap '' INT 的循环不退出；terminate 仍能终止并停止写入
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("trap '' INT; while true; do echo x >> {}; sleep 0.05; done", log.display()),
        "yield_time_ms": 250,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    let sid2 = out.result.session_id.unwrap();
    let int2 = WriteStdinArgs::from_value(&json!({
        "session_id": sid2, "chars": "\u{0003}", "yield_time_ms": 1000
    }))
    .unwrap();
    let r = mgr.interact(&int2, None).result;
    assert_eq!(r.status, ExecStatus::Running, "忽略中断＝仍在运行：{r:?}");
    wait_until_true(|| log_size(&log) > 20, "忽略中断后仍写日志");
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid2, "terminate": true })).unwrap();
    let r = mgr.interact(&term, None).result;
    assert_eq!(r.status, ExecStatus::Terminated, "{r:?}");
    assert!(r.error.as_deref().unwrap().contains("终止"), "清理结果如实说明");
    let s1 = log_size(&log);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(log_size(&log), s1, "终止后整个进程组停止写入（含后台孙进程）");
    let _ = fs::remove_dir_all(&dir);
}

/// 单次输入上限：恰 64 KiB 通过并由程序完整收到；超限整次拒绝、零写入。
#[test]
fn input_limit_exact_passes_over_limit_rejects_with_zero_write() {
    let dir = temp_dir("input-limit");
    let payload = dir.join("payload");
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        // raw 模式避免行规范缓冲上限，dd 恰读 65536 字节后退出
        "cmd": format!("stty raw -echo 2>/dev/null; head -c 65536 > {}; echo DD-OK", payload.display()),
        "yield_time_ms": 5000,
        "tty": true,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running, "{:?}", out.result);
    let sid = out.result.session_id.unwrap();
    // ① 超限：参数层整次拒绝（不进入执行，无部分发送路径）
    let over = WriteStdinArgs::from_value(&json!({
        "session_id": sid, "chars": "x".repeat(64 * 1024 + 1)
    }));
    assert!(over.unwrap_err().contains("整次拒绝"), "超限在发送前拒绝");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(log_size(&payload), 0, "超限输入零写入");
    // 会话仍可用
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Running, "拒绝不影响原会话");
    // ② 恰 64 KiB：通过；dd 收满退出（有界等待终态），echo DD-OK 可见
    let exact = WriteStdinArgs::from_value(&json!({
        "session_id": sid, "chars": "x".repeat(64 * 1024), "yield_time_ms": 5000
    }))
    .unwrap();
    let mut r = mgr.interact(&exact, None).result;
    // 写入后进程随即退出；有界等待终态（空查询最多各等 5s）
    let deadline = Instant::now() + Duration::from_secs(15);
    while r.status == ExecStatus::Running {
        assert!(Instant::now() < deadline, "等待 dd 完成超时");
        r = query(&mgr, &sid);
    }
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert_eq!(fs::metadata(&payload).unwrap().len(), 64 * 1024, "完整送达 64 KiB");
    assert!(r.output.contains("DD-OK"), "echo DD-OK 可见：{:?}", r.output);
    let _ = fs::remove_dir_all(&dir);
}

/// 向不读输入的 PTY 程序写入：等待可取消，不阻塞整个宿主；进程保留。
#[test]
fn blocked_input_is_cancellable_and_session_kept() {
    let mgr = ProcessManager::new();
    // 程序不读 stdin（sleep 保持运行）
    let args = ExecArgs::from_value(&json!({
        "cmd": "sleep 30", "yield_time_ms": 250, "tty": true
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running, "{:?}", out.result);
    let sid = out.result.session_id.unwrap();
    // raw 关闭回显：sleep 不读输入，master 写入缓冲打满后阻塞
    // （单次 64KiB 大于 pty 缓冲；写入线程阻塞，主等待被取消打断）
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        cancel_for_thread.store(true, Ordering::SeqCst);
    });
    let input = WriteStdinArgs::from_value(&json!({
        "session_id": sid, "chars": "y".repeat(64 * 1024), "yield_time_ms": 5000
    }))
    .unwrap();
    let started = Instant::now();
    let r = mgr.interact(&input, Some(&cancel)).result;
    assert!(started.elapsed() < Duration::from_secs(3), "取消必须及时结束等待");
    assert_eq!(r.status, ExecStatus::Cancelled, "{r:?}");
    assert_eq!(r.session_id.as_deref(), Some(sid.as_str()), "进程保留");
    // 进程仍在：取消后可终止
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid, "terminate": true })).unwrap();
    assert_eq!(mgr.interact(&term, None).result.status, ExecStatus::Terminated);
}

// ---------- 容量与所有权 ----------

/// 64 条会话上限：满额拒绝新建、不淘汰旧服务；释放（明确终止）后可再建。
#[test]
fn capacity_64_rejects_65th_without_eviction() {
    let mgr = ProcessManager::new();
    // 并行启动 64 个 sleep：每个快速返回 running
    let counter = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..64)
        .map(|_| {
            let mgr = mgr.clone();
            let counter = counter.clone();
            std::thread::spawn(move || {
                let out = mgr.start(&exec_args("sleep 60", 250), None);
                assert_eq!(out.result.status, ExecStatus::Running);
                counter.fetch_add(1, Ordering::SeqCst);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(counter.load(Ordering::SeqCst), 64);
    assert_eq!(mgr.live_sessions().len(), 64);
    // 第 65 条：拒绝；已有进程不被杀（仍可查询）
    let out = mgr.start(&exec_args("sleep 60", 250), None);
    assert_eq!(out.result.status, ExecStatus::Error, "{:?}", out.result);
    assert!(out.result.error.as_deref().unwrap().contains("上限"));
    let r = query(&mgr, "s1");
    assert_eq!(r.status, ExecStatus::Running, "满额不淘汰旧服务");
    // 明确终止释放一条：可再建
    let term = WriteStdinArgs::from_value(&json!({ "session_id": "s1", "terminate": true })).unwrap();
    assert_eq!(mgr.interact(&term, None).result.status, ExecStatus::Terminated);
    let out = mgr.start(&exec_args("sleep 60", 250), None);
    assert_eq!(out.result.status, ExecStatus::Running, "释放后可再建");
    mgr.close();
    assert!(mgr.live_sessions().is_empty());
}

/// 句柄绑定管理器实例：另一管理器（另一对话）不识别同句柄。
#[test]
fn handles_scoped_to_manager_instance() {
    let mgr_a = ProcessManager::new();
    let mgr_b = ProcessManager::new();
    let out = mgr_a.start(&exec_args("sleep 5", 250), None);
    let sid = out.result.session_id.unwrap();
    let r = mgr_b.interact(&stdin_args(&sid), None).result;
    assert_eq!(r.status, ExecStatus::Error);
    assert!(r.error.unwrap().contains("未知或已失效"), "句柄不跨实例");
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid, "terminate": true })).unwrap();
    assert_eq!(mgr_a.interact(&term, None).result.status, ExecStatus::Terminated);
}

/// 管理器关闭：拒绝新启动；全部进程被终止回收。
#[test]
fn close_kills_all_and_rejects_new_starts() {
    let dir = temp_dir("close-all");
    let log = dir.join("log");
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("while true; do echo x >> {}; sleep 0.05; done", log.display()),
        "yield_time_ms": 250,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    let sid = out.result.session_id.unwrap();
    let out2 = mgr.start(&exec_args("sleep 60", 250), None);
    let sid2 = out2.result.session_id.unwrap();
    mgr.close();
    let s1 = log_size(&log);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(log_size(&log), s1, "关闭终止全部进程");
    assert!(mgr.live_sessions().is_empty());
    let out = mgr.start(&exec_args("true", 1000), None);
    assert_eq!(out.result.status, ExecStatus::Error, "关闭后拒绝新启动");
    // 句柄随关闭失效
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Error);
    let _ = sid2;
    let _ = fs::remove_dir_all(&dir);
}

/// 对话切换（reset）：旧进程关闭、新对话从空管理器开始。
#[test]
fn reset_closes_old_and_allows_fresh_start() {
    let mgr = ProcessManager::new();
    let out = mgr.start(&exec_args("sleep 30", 250), None);
    let sid = out.result.session_id.unwrap();
    mgr.reset();
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Error, "旧句柄随切换失效");
    let out = mgr.start(&exec_args("echo fresh", 10_000), None);
    assert_eq!(out.result.status, ExecStatus::Completed, "重置后可启动");
}

// ---------- 输出与退出 ----------

/// 双流大量输出不互锁；大输出按码点上限截断并标记。
#[test]
fn big_dual_stream_completes_and_truncates() {
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": "seq 1 60000; seq 1 60000 >&2",
        "yield_time_ms": 5000,
        "max_output_chars": 5000,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Completed, "{:?}", out.result.status);
    assert!(out.result.truncated, "超码点上限标记截断");
    assert!(out.result.output.chars().count() <= 5000);
    assert!(out.result.output.starts_with("1\n2\n"), "保留前缀");
}

/// 超过 1 MiB 待取缓存的输出：仍能完成并返回有界输出（头尾保留语义由
/// proc_session 单测证明；此处验证取走后缓存有界、无旧内容重放）。
#[test]
fn oversized_pending_stays_bounded() {
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": "sleep 0.4; seq 1 400000",
        "yield_time_ms": 250,
        "max_output_chars": 100000,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running);
    assert!(out.result.output.chars().count() <= 100000, "首次取走即有界");
    let sid = out.result.session_id.unwrap();
    // 有界等待终态（空查询最多各等 5s；正常一两次即终态）
    let deadline = Instant::now() + Duration::from_secs(15);
    let final_r = loop {
        let r = query(&mgr, &sid);
        if r.status == ExecStatus::Completed {
            break r;
        }
        assert_eq!(r.status, ExecStatus::Running, "意外终态：{r:?}");
        assert!(Instant::now() < deadline, "等待 seq 完成超时");
    };
    assert!(final_r.output.chars().count() <= 100000, "输出有界");
    assert!(!final_r.output.is_empty(), "终段输出非空");
    assert!(final_r.truncated, "缓存超限标记截断");
}

/// UTF-8 多字节与非法字节端到端：合法保留、非法替换为 U+FFFD。
#[test]
fn utf8_and_invalid_bytes_end_to_end() {
    let mgr = ProcessManager::new();
    let out = mgr.start(&exec_args("printf 'a\\344\\270\\255\\377\\377b\\n'", 10_000), None);
    assert_eq!(out.result.status, ExecStatus::Completed);
    assert!(out.result.output.contains('中'), "{:?}", out.result.output);
    assert!(out.result.output.contains('\u{FFFD}'), "非法字节替换");
    assert!(out.result.output.ends_with("b\n"));
}

// ---------- 工具循环集成（turn 层）----------

use terminal_chat::prompt::HistoryItem;
use terminal_chat::turn::{run_tool_turn, run_tool_turn_with_mode, ToolMode};

fn tc_exec(id: &str, cmd: &str, yield_ms: u64) -> serde_json::Value {
    json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [{ "id": id, "type": "function",
            "function": { "name": "exec_command",
                "arguments": json!({ "cmd": cmd, "yield_time_ms": yield_ms }).to_string() } }]
    }, "finish_reason": "tool_calls" }] })
}

/// 同一用户轮内：exec 返回 running＋句柄 → write_stdin 带同一句柄查询 → 终态。
/// 证明跨工具调用续接同一进程（不重新启动）、结果配对、续请求携带全部结果。
#[test]
fn turn_exec_running_then_write_stdin_across_requests() {
    let mgr = ProcessManager::new();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mut sent = 0usize;
    let reply = run_tool_turn(
        &mut history,
        "go",
        &mut |body: &serde_json::Value| {
            sent += 1;
            Ok(match sent {
                1 => tc_exec("c1", "sleep 0.5; echo DONE", 250),
                2 => {
                    // 从第 2 请求体中取 c1 结果的 session_id——同轮续接
                    let sid = body["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|m| m["role"] == "tool" && m["tool_call_id"] == "c1")
                        .and_then(|m| m["content"].as_str())
                        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
                        .and_then(|v| v["session_id"].as_str().map(str::to_string))
                        .expect("第 2 请求须携带 c1 的运行中结果（含句柄）");
                    json!({ "choices": [{ "message": {
                        "role": "assistant", "content": null,
                        "tool_calls": [{ "id": "c2", "type": "function",
                            "function": { "name": "write_stdin",
                                "arguments": json!({ "session_id": sid, "yield_time_ms": 5000 }).to_string() } }]
                    }, "finish_reason": "tool_calls" }] })
                }
                _ => json!({ "choices": [{ "message":
                    { "content": "fin" }, "finish_reason": "stop" }] }),
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        &mgr,
    )
    .unwrap();
    assert_eq!(reply, "fin");
    assert_eq!(sent, 3, "恰 3 次模型请求");
    // 结果配对与内容：c1＝running＋句柄；c2＝completed DONE
    let results: Vec<(String, serde_json::Value)> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult { tool_call_id, content, .. } => Some((
                tool_call_id.clone(),
                serde_json::from_str(content).unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "c1");
    assert_eq!(results[0].1["status"], "running");
    assert!(results[0].1["session_id"].is_string());
    assert_eq!(results[1].0, "c2");
    assert_eq!(results[1].1["status"], "completed");
    assert_eq!(results[1].1["output"].as_str().unwrap().trim(), "DONE");
    assert!(mgr.live_sessions().is_empty(), "终态取走后不留记录");
}

/// 内部记忆合并轮拒绝执行新工具：write_stdin 按 mem 注册表外的未知工具回传。
#[test]
fn turn_memory_merge_rejects_write_stdin() {
    let dir = temp_dir("mem-merge");
    let root = dir.join("memroot");
    fs::create_dir_all(&root).unwrap();
    let mgr = ProcessManager::new();
    let mut history: Vec<HistoryItem> = Vec::new();
    let mut sent = 0usize;
    let reply = run_tool_turn_with_mode(
        &mut history,
        "merge",
        &mut |_| {
            sent += 1;
            Ok(match sent {
                1 => json!({ "choices": [{ "message": {
                    "role": "assistant", "content": null,
                    "tool_calls": [{ "id": "m1", "type": "function",
                        "function": { "name": "write_stdin",
                            "arguments": json!({ "session_id": "s1" }).to_string() } }]
                }, "finish_reason": "tool_calls" }] }),
                _ => json!({ "choices": [{ "message":
                    { "content": "merged" }, "finish_reason": "stop" }] }),
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::MemoryMerge { root: &root, system: "consolidate", expected_epoch: 1 },
        &mgr,
    )
    .unwrap();
    assert_eq!(reply, "merged");
    let HistoryItem::ToolResult { content, .. } = history
        .iter()
        .find(|h| matches!(h, HistoryItem::ToolResult { tool_call_id, .. } if tool_call_id == "m1"))
        .unwrap()
    else {
        unreachable!()
    };
    let v: serde_json::Value = serde_json::from_str(content).unwrap();
    assert!(
        v["error"].as_str().unwrap().contains("未知工具"),
        "记忆内部轮不得执行 write_stdin：{v}"
    );
    assert!(mgr.live_sessions().is_empty());
    let _ = fs::remove_dir_all(&dir);
}

/// 8 次模型请求上限耗尽：已启动进程保留；下一用户轮凭同一句柄继续。
#[test]
fn turn_cap_exhaustion_keeps_sessions_for_next_turn() {
    let mgr = ProcessManager::new();
    let mut history: Vec<HistoryItem> = Vec::new();
    let err = run_tool_turn(
        &mut history,
        "go",
        &mut |_| Ok(tc_exec("c", "sleep 30", 250)),
        &mut |_| {},
        None,
        &mut |_| {},
        &mgr,
    )
    .unwrap_err();
    assert!(err.contains("上限"), "{err}");
    assert_eq!(mgr.live_sessions().len(), 8, "每批一条：8 个会话全部保留");
    // 下一用户轮：查询 s1 仍可续接（运行中）
    let mut sent2 = 0usize;
    let reply = run_tool_turn(
        &mut history,
        "next",
        &mut |_| {
            sent2 += 1;
            Ok(match sent2 {
                1 => json!({ "choices": [{ "message": {
                    "role": "assistant", "content": null,
                    "tool_calls": [{ "id": "q1", "type": "function",
                        "function": { "name": "write_stdin",
                            "arguments": json!({ "session_id": "s1", "yield_time_ms": 5000 }).to_string() } }]
                }, "finish_reason": "tool_calls" }] }),
                _ => json!({ "choices": [{ "message":
                    { "content": "next-fin" }, "finish_reason": "stop" }] }),
            })
        },
        &mut |_| {},
        None,
        &mut |_| {},
        &mgr,
    )
    .unwrap();
    assert_eq!(reply, "next-fin");
    let results: Vec<serde_json::Value> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "q1" => {
                Some(serde_json::from_str(content).unwrap())
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "running", "额度耗尽不杀进程：{:?}", results[0]);
    mgr.close();
}

/// 已退出但最后输出未取走的记录仍占容量：满额拒绝不因进程已退出而放宽。
#[test]
fn exited_undrained_session_still_occupies_capacity() {
    let mgr = ProcessManager::new();
    // s1：返回 running 后自然退出，不查询——记录保留（终态＋待取输出）
    let out = mgr.start(&exec_args("sleep 0.3; echo tail-line", 250), None);
    let sid = out.result.session_id.unwrap();
    std::thread::sleep(Duration::from_millis(1200)); // 进程已退出，输出未取走
    assert!(mgr.live_sessions().iter().any(|(id, _)| id == &sid), "已退出未取走的记录仍在册");
    // 填满其余 63 条（并行启动）
    let mut mgr_view = mgr.clone();
    let _ = &mut mgr_view;
    let handles: Vec<_> = (0..63)
        .map(|_| {
            let m = mgr.clone();
            std::thread::spawn(move || {
                assert_eq!(m.start(&exec_args("sleep 60", 250), None).result.status, ExecStatus::Running);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(mgr.live_sessions().len(), 64);
    // 第 65 条拒绝
    assert_eq!(mgr.start(&exec_args("sleep 60", 250), None).result.status, ExecStatus::Error);
    // 查询收尾释放容量：取走 s1 终态输出后可再建
    let r = query(&mgr, &sid);
    assert_eq!(r.status, ExecStatus::Completed, "{r:?}");
    assert!(r.output.contains("tail-line"), "最后待取输出不丢");
    assert_eq!(mgr.start(&exec_args("sleep 60", 250), None).result.status, ExecStatus::Running);
    mgr.close();
}


/// 016 验收：PTY 阻塞写入的取消——先用输入侧进度（cnt 文件）确认写入路径
/// 已在实际交付且远未完成（阻塞中），再取消；验证 3 秒内返回、cancelled＋
/// 保留句柄、进程与其他会话不受影响、取消后不再提交尚未发送的输入、
/// 已发送部分保留且如实说明、随后仍可查询与终止。
/// 读端为真实慢速读取器（python3，每 4 KiB 落盘计数并小睡），不靠固定睡眠
/// 猜阻塞位置——取消时写入进度以 cnt 实测为准。
#[test]
fn pty_cancel_stops_further_input_submission() {
    let dir = temp_dir("pty-cancel-stop");
    let data = dir.join("data.bin");
    let cnt = dir.join("cnt");
    let script = dir.join("reader.py");
    std::fs::write(
        &script,
        concat!(
            "import os, time, sys\n",
            "data, cnt = sys.argv[1], sys.argv[2]\n",
            "f = open(data, \"wb\")\n",
            "n = 0\n",
            "while n < 65536:\n",
            "    b = os.read(0, 4096)\n",
            "    if not b: break\n",
            "    f.write(b); f.flush()\n",
            "    n += len(b)\n",
            "    open(cnt, \"w\").write(str(n))\n",
            "    time.sleep(0.05)\n",
            "print(\"READER-PASS-THRESHOLD\", file=sys.stderr, flush=True)\n",
        ),
    )
    .unwrap();
    let cmd = format!(
        "stty raw -echo 2>/dev/null; python3 {script} {data} {cnt}; echo ALL-CONSUMED; sleep 30",
        script = script.display(),
        data = data.display(),
        cnt = cnt.display(),
    );
    let mgr = ProcessManager::new();
    // 另一会话：验证取消不误伤其他会话
    let other = mgr
        .start(&exec_args("sleep 30", 250), None);
    assert_eq!(other.result.status, ExecStatus::Running);

    let args = ExecArgs::from_value(&serde_json::json!({
        "cmd": cmd, "yield_time_ms": 5000, "tty": true
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running, "{:?}", out.result);
    let sid = out.result.session_id.unwrap();

    // 交互线程：写入 64 KiB（读端每 4 KiB 约 55ms，全程约 0.9s——取消时必然仍在写）
    let cancel = Arc::new(AtomicBool::new(false));
    let mgr_w = mgr.clone();
    let sid_w = sid.clone();
    let cancel_w = cancel.clone();
    let handle = std::thread::spawn(move || {
        let input = WriteStdinArgs::from_value(&serde_json::json!({
            "session_id": sid_w, "chars": "B".repeat(64 * 1024), "yield_time_ms": 30000
        }))
        .unwrap();
        mgr_w.interact(&input, Some(&cancel_w)).result
    });

    // 读端进度计数：python 截断写非原子——读取失败（瞬时空/半写）沿用上次观测
    let read_cnt = |prev: u64| -> u64 {
        std::fs::read_to_string(&cnt)
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(prev)
    };
    // 就绪信号（输入侧实测）：读端已收到至少 4096 字节——写入确在进行且远未完成
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed = 0u64;
    loop {
        observed = read_cnt(observed);
        if observed >= 4096 {
            break;
        }
        assert!(Instant::now() < deadline, "读端进度未就绪（cnt={observed}）");
        std::thread::sleep(Duration::from_millis(20));
    }
    // 写入仍在进行（进度远小于 64 KiB）：此刻取消
    let at_cancel = read_cnt(observed);
    assert!(at_cancel < 64 * 1024 - 8192, "取消时应仍有大量未发送输入（{at_cancel}）");
    let cancelled_at = Instant::now();
    cancel.store(true, Ordering::SeqCst);

    // 3 秒内返回 cancelled＋保留句柄＋如实说明
    let r = handle.join().unwrap();
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(3),
        "取消须在 3 秒内返回（实际 {:?}）",
        cancelled_at.elapsed()
    );
    assert_eq!(r.status, ExecStatus::Cancelled, "{r:?}");
    assert_eq!(r.session_id.as_deref(), Some(sid.as_str()), "保留有效句柄");
    let err = r.error.unwrap_or_default();
    assert!(err.contains("无法撤回"), "已发送部分如实说明：{err}");

    // 核心断言：取消后不再继续提交尚未发送的输入——读端进度趋于稳定
    //（内核已缓冲与在途分块仍会送达，属已提交部分；旧实现会持续提交至
    // 读端收满 64 KiB——稳定值恒为 65536，可确定区分）
    let mut prev = at_cancel;
    let settled: u64 = {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            std::thread::sleep(Duration::from_millis(400));
            let n = read_cnt(prev);
            if n == prev {
                break n;
            }
            prev = n;
            assert!(Instant::now() < deadline, "读端进度未稳定（{at_cancel} -> {n}）");
        }
    };
    assert!(
        settled < 64 * 1024,
        "取消后不得继续提交至全部送达（{at_cancel} -> {settled}）"
    );
    assert!(settled >= at_cancel, "已发送部分不回退（在途/缓冲允许送达）");
    // 取消前已成功发送的部分保留（data.bin 与 cnt 一致且非零）
    assert_eq!(std::fs::metadata(&data).unwrap().len(), settled);
    assert!(settled >= 4096, "取消前已有部分送达");

    // 进程仍存活且会话可查询；其他会话不受影响
    cancel.store(false, Ordering::SeqCst);
    let q = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": sid, "yield_time_ms": 5000 })).unwrap();
    let rq = mgr.interact(&q, None).result;
    assert_eq!(rq.status, ExecStatus::Running, "取消后进程保留可查询：{rq:?}");
    let other_id = other.result.session_id.clone().unwrap();
    let qo = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": other_id, "yield_time_ms": 5000 })).unwrap();
    assert_eq!(mgr.interact(&qo, None).result.status, ExecStatus::Running, "其他会话不受影响");

    // 随后仍可明确终止
    let term = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": sid, "terminate": true })).unwrap();
    assert_eq!(mgr.interact(&term, None).result.status, ExecStatus::Terminated);
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 016：向不读输入的 PTY 程序写入且不取消——写入等待窗口到期后如实返回
/// 运行中（不报错、不阻塞宿主），进程保留，随后可终止。
#[test]
fn pty_write_window_expiry_returns_running_for_nonreading_program() {
    let mgr = ProcessManager::new();
    let args = ExecArgs::from_value(&json!({
        "cmd": "stty raw -echo 2>/dev/null; sleep 30",
        "yield_time_ms": 250,
        "tty": true,
    }))
    .unwrap();
    let out = mgr.start(&args, None);
    assert_eq!(out.result.status, ExecStatus::Running, "{:?}", out.result);
    let sid = out.result.session_id.unwrap();
    // 64 KiB 写入、等待窗口 250ms：程序不读输入 → 窗口到期如实返回运行中
    let input = WriteStdinArgs::from_value(&json!({
        "session_id": sid, "chars": "x".repeat(64 * 1024), "yield_time_ms": 250
    }))
    .unwrap();
    let started = Instant::now();
    let r = mgr.interact(&input, None).result;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "写入等待有界（实际 {:?}）",
        started.elapsed()
    );
    assert_eq!(r.status, ExecStatus::Running, "不报错、不伪装完成：{r:?}");
    assert_eq!(r.session_id.as_deref(), Some(sid.as_str()));
    // 进程保留，随后明确终止成功
    let term = WriteStdinArgs::from_value(&json!({ "session_id": sid, "terminate": true })).unwrap();
    assert_eq!(mgr.interact(&term, None).result.status, ExecStatus::Terminated);
}
