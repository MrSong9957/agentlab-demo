//! 033：apply_patch 沙箱模式——补丁经 bwrap 包装的 `--internal-apply` 子进程，
//! 读、判型、写全部在沙箱内完成。环境双分支：无 bwrap＝显式失败
//!（failIfUnavailable，绝不静默回退为进程内执行，文件不得产生）；有 bwrap＝
//! Add/Update/Delete 真实落盘验证。全程假内容，不触真实凭据。

use std::io::Write;
use std::process::{Command, Stdio};

fn temp_root(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("tchat-033-apply-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn update_patch(path: &std::path::Path, old: &str, new: &str) -> String {
    format!(
        "*** Begin Patch\n*** Update File: {}\n@@\n-{old}\n+{new}\n*** End Patch\n",
        path.display()
    )
}

fn input_of(patch: &str) -> serde_json::Value {
    serde_json::json!({ "patch": patch })
}

/// 033 沙箱用例互斥（沿用 027 F3）：TCHAT_SANDBOX 是进程级环境，
/// 同二进制内沙箱用例必须串行。
static SANDBOX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

type SandboxOutcome = (
    terminal_chat::apply_patch::ApplyPatchResult,
    Vec<terminal_chat::prompt::PatchChange>,
);

fn run_sandboxed(exe: &std::path::Path, patch: &str) -> SandboxOutcome {
    terminal_chat::apply_patch::apply_patch_sandboxed_with_exe(exe, &input_of(patch))
}

/// `--internal-apply` 内部入口：stdin 补丁 → (结果, 变更) JSON（027 的 uid
/// 字段随双账号机制移除，033 子进程与主进程同 uid，无需身份直证字段）。
#[test]
fn internal_apply_entry_reads_stdin_and_writes_result() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("entry");
    let file = root.join("f.txt");
    std::fs::write(&file, "hello\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .arg("--internal-apply")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let patch = update_patch(&file, "hello", "world");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(patch.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "exit={:?} stderr={:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let (result, changes): SandboxOutcome =
        serde_json::from_slice(&out.stdout).expect("stdout 为 (结果, 变更) JSON");
    assert!(result.ok, "result={result:?}");
    assert_eq!(result.changed_files.len(), 1);
    assert!(result.state_known);
    assert_eq!(changes.len(), 1, "展示用变更一并返回");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "world\n");
    let _ = std::fs::remove_dir_all(&root);
}

/// 沙箱 Add：有 bwrap＝文件在沙箱内落盘；无 bwrap＝显式错误（含「沙箱」字样）
/// 且文件未创建——不静默回退为进程内执行。
#[test]
fn sandbox_add_applies_or_fails_closed() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("add");
    let file = root.join("new.txt");
    let _ = std::fs::remove_file(&file);

    std::env::set_var("TCHAT_SANDBOX", "1");
    let exe: &std::path::Path = env!("CARGO_BIN_EXE_terminal-chat").as_ref();
    let patch = format!(
        "*** Begin Patch\n*** Add File: {}\n+content\n*** End Patch\n",
        file.display()
    );
    let (result, changes) = run_sandboxed(exe, &patch);
    std::env::remove_var("TCHAT_SANDBOX");
    if result.ok {
        assert_eq!(changes.len(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "content\n");
    } else {
        let msg = result.error.as_deref().unwrap_or_default();
        assert!(msg.contains("沙箱"), "失败原因应指向沙箱不可用：{msg}");
        assert!(!file.exists(), "fail-closed 时不得产生写入效果");
        assert!(!result.state_known, "未执行＝状态未知");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// 沙箱 Update/Delete（三类操作全验证）。同 uid 沙箱下主进程建的文件沙箱内
/// 可写（对比 027 的属主问题——写权限由 bwrap 可写根授予，不再依赖账号属主）。
#[test]
fn sandbox_update_and_delete() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("upd-del");
    let file = root.join("u.txt");
    std::fs::write(&file, "old-line\n").unwrap();

    std::env::set_var("TCHAT_SANDBOX", "1");
    let exe: &std::path::Path = env!("CARGO_BIN_EXE_terminal-chat").as_ref();
    let (result, _) = run_sandboxed(exe, &update_patch(&file, "old-line", "new-line"));
    if !result.ok {
        let msg = result.error.as_deref().unwrap_or_default();
        std::env::remove_var("TCHAT_SANDBOX");
        assert!(msg.contains("沙箱"), "失败原因应指向沙箱不可用：{msg}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old-line\n", "不得部分执行");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new-line\n");

    let del = format!(
        "*** Begin Patch\n*** Delete File: {}\n*** End Patch\n",
        file.display()
    );
    let (result, _) = run_sandboxed(exe, &del);
    std::env::remove_var("TCHAT_SANDBOX");
    assert!(result.ok, "沙箱 Delete 失败：{result:?}");
    assert!(!file.exists(), "Delete 后文件应移除");
    let _ = std::fs::remove_dir_all(&root);
}
