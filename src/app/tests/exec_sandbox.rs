//! 033：exec_command 沙箱模式集成测试。
//!
//! **独立测试文件＝独立测试进程**：TCHAT_SANDBOX 是进程级环境变量，沙箱用例
//! 与普通模式用例必须分进程运行——同进程并发时沙箱用例的 set 窗口会污染
//! 正在读该变量的普通用例（exec_command.rs 20 用例 10 失败的实测教训）。
//!
//! 覆盖：管道模式 failIfUnavailable／同 uid 沙箱＋哨兵 env 拒绝＋.env 遮蔽
//!（真实访问验证）、PTY 回显＋超时组杀（同 uid 下 killpg 直接有效，027 跨
//! uid 补杀已随双账号机制删除）。全程假内容，不触真实凭据。

mod common;

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;

use serde_json::json;
use terminal_chat::exec::{ExecArgs, ExecResult, ExecStatus};
use terminal_chat::proc_session::ProcessManager;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tchat-exec-sb-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn log_size(path: &PathBuf) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn writer_cmd(log: &PathBuf) -> String {
    format!(
        "while true; do echo x >> {log}; sleep 0.05; done & sleep 30",
        log = log.display()
    )
}

fn run(input: &serde_json::Value, cancel: Option<&AtomicBool>) -> ExecResult {
    let mgr = ProcessManager::new();
    let r = match ExecArgs::from_value(input) {
        Ok(args) => mgr.start(&args, cancel).result,
        Err(msg) => ExecResult::error(msg),
    };
    mgr.close();
    r
}

/// 进程内沙箱用例互斥：本文件内用例仍共用测试进程（env 进程级）。
static SANDBOX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 033：沙箱模式（TCHAT_SANDBOX=1）双分支——
/// 无 bwrap：请求沙箱必须显式报错（failIfUnavailable），零输出零副作用，
/// 绝不静默回退为无沙箱执行；有 bwrap：`id -un`＝本进程用户（同 uid 沙箱）、
/// 主进程哨兵 env 不进入子进程（白名单环境）、**策略锚定根**（crate root＝
/// 主进程 cwd）的 .env 被遮蔽——workdir 故意指向别处，验证 deny 不随模型可控
/// 的 workdir 转移（G1 锚定逃逸回归）。真实访问验证，非标记位。
#[test]
fn sandbox_mode_wraps_tool_or_fails_closed_without_fallback() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    struct Guard;
    impl Guard {
        fn set() -> Self {
            std::env::set_var("TCHAT_SANDBOX", "1");
            std::env::set_var("TCHAT_SANDBOX_SENTINEL", "FAKE-SECRET-VALUE");
            // 锚定根（crate root＝测试进程 cwd）下的假凭据文件；结束删除。
            // 内容 FAKE，路径在仓库内短暂存在（finally 清理）。
            std::fs::write(
                Path::new(env!("CARGO_MANIFEST_DIR")).join(".env"),
                "FAKE-SECRET-LINE=1\n",
            )
            .unwrap();
            Self
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            std::env::remove_var("TCHAT_SANDBOX");
            std::env::remove_var("TCHAT_SANDBOX_SENTINEL");
            let _ = fs::remove_file(Path::new(env!("CARGO_MANIFEST_DIR")).join(".env"));
        }
    }
    let _guard = Guard::set();

    let anchor_env = Path::new(env!("CARGO_MANIFEST_DIR")).join(".env");
    let dir = temp_dir("sandbox-env");
    fs::write(dir.join(".env"), "FAKE-SECRET-LINE=1\n").unwrap();
    let r = run(
        &json!({
            "cmd": format!(
                "id -un && echo [$TCHAT_SANDBOX_SENTINEL] && test ! -s {anchor} && echo ENV-MASKED",
                anchor = anchor_env.display()
            ),
            // workdir 故意非锚定根：deny 不随 workdir 转移（G1 回归）
            "workdir": dir.display().to_string(),
            "timeout_ms": 10_000,
        }),
        None,
    );
    if terminal_chat::sandbox::probe_available() {
        assert_eq!(r.status, ExecStatus::Completed, "沙箱执行：{r:?}");
        let user = std::process::Command::new("id")
            .arg("-un")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap();
        assert!(r.output.contains(&user), "同 uid 沙箱下 id -un 应为本进程用户：{r:?}");
        assert!(r.output.contains("[]"), "哨兵 env 不得进入沙箱子进程：{r:?}");
        assert!(
            r.output.contains("ENV-MASKED"),
            "锚定根 .env 应被遮蔽为空（workdir 转移不影响 deny）：{r:?}"
        );
        assert!(
            !r.output.contains("FAKE-SECRET-LINE"),
            ".env 正文不得被沙箱内工具读取：{r:?}"
        );
    } else {
        assert_eq!(r.status, ExecStatus::Error, "沙箱不可用必须显式报错：{r:?}");
        let msg = r.error.as_deref().unwrap_or_default();
        assert!(msg.contains("bwrap"), "错误应指向 bwrap 不可用：{msg}");
        assert!(r.output.is_empty(), "不得回退为无沙箱执行：{r:?}");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// 033：沙箱模式 PTY/超时语义不回归——有 bwrap 时验证 PTY 写入与超时组杀
///（同 uid 下 killpg 直接有效）；无 bwrap 时请求必须显式失败。
#[test]
fn sandbox_mode_pty_timeout_semantics_preserved() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    struct Guard;
    impl Guard {
        fn set() -> Self {
            std::env::set_var("TCHAT_SANDBOX", "1");
            Self
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            std::env::remove_var("TCHAT_SANDBOX");
        }
    }
    let _guard = Guard::set();

    if !terminal_chat::sandbox::probe_available() {
        let dir = temp_dir("sb-pty-closed");
        let r = run(
            &json!({ "cmd": "true", "tty": true, "workdir": dir.display().to_string(), "timeout_ms": 10_000 }),
            None,
        );
        assert_eq!(r.status, ExecStatus::Error, "沙箱不可用必须显式报错：{r:?}");
        assert!(r.error.as_deref().unwrap_or_default().contains("bwrap"));
        let _ = fs::remove_dir_all(&dir);
        return;
    }

    let dir = temp_dir("sb-pty");
    let echo = dir.join("echo.txt");
    let r = run(
        &json!({ "cmd": format!("printf pty-ok > {}", echo.display()), "tty": true, "timeout_ms": 10_000 }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Completed, "PTY 沙箱执行：{r:?}");
    assert_eq!(fs::read_to_string(&echo).unwrap_or_default(), "pty-ok");

    // 超时：写循环被组杀（bwrap→sh→循环链同进程组），文件停止增长。
    let dir2 = temp_dir("sb-timeout");
    let log = dir2.join("w.log");
    let r = run(
        &json!({ "cmd": writer_cmd(&log), "timeout_ms": 400, "yield_time_ms": 8000 }),
        None,
    );
    assert_eq!(r.status, ExecStatus::Timeout, "{r:?}");
    let s1 = log_size(&log);
    assert!(s1 > 0, "写入循环确实运行过");
    thread::sleep(Duration::from_millis(400));
    assert_eq!(s1, log_size(&log), "超时后写循环应被组杀停止");

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&dir2);
}
