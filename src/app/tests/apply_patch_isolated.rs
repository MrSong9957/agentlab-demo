//! 027 B：apply_patch 隔离模式——读、判型、写全部在目标用户侧子进程完成。
//!
//! 执行身份证据：`--internal-apply` 结果附子进程真实 uid；隔离链路目标用户取
//! `TCHAT_TEST_ISOLATED_USER`（默认 nobody，uid 与测试进程必然不同＝身份直证；
//! 无 NOPASSWD sudo 的环境可改设为与本进程同 uid 以维持可运行性）。
//! 全程假内容，不触真实凭据。

use std::io::Write;
use std::process::{Command, Stdio};

fn temp_root(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("tchat-027-apply-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn test_user() -> String {
    std::env::var("TCHAT_TEST_ISOLATED_USER").unwrap_or_else(|_| "nobody".into())
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

/// 027 B 隔离用例互斥（安全攻防 F3）：TCHAT_ISOLATED_TOOLS 是进程级环境，
/// 同二进制内隔离用例必须串行。
static ISOLATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

type IsolatedOutcome = (
    terminal_chat::apply_patch::ApplyPatchResult,
    Vec<terminal_chat::prompt::PatchChange>,
    Option<u32>,
);

fn run_isolated(exe: &std::path::Path, patch: &str) -> IsolatedOutcome {
    terminal_chat::apply_patch::apply_patch_isolated_with_exe(exe, &input_of(patch))
}

/// `--internal-apply` 内部入口：stdin 补丁 → (结果, 变更, uid) JSON。
#[test]
fn internal_apply_entry_reads_stdin_and_writes_result() {
    let _serial = ISOLATION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    let (result, changes, uid): IsolatedOutcome =
        serde_json::from_slice(&out.stdout).expect("stdout 为 (结果, 变更, uid) JSON");
    assert!(result.ok, "result={result:?}");
    assert_eq!(result.changed_files.len(), 1);
    assert!(result.state_known);
    assert_eq!(changes.len(), 1, "展示用变更一并返回");
    let my_uid = unsafe { libc::getuid() };
    assert_eq!(uid, Some(my_uid), "直跑入口 uid＝本进程");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "world\n");
    let _ = std::fs::remove_dir_all(&root);
}

/// 隔离链路：sudo 以目标用户（默认 nobody）身份执行 Add——uid 直证执行侧。
#[test]
fn isolated_apply_runs_as_target_user() {
    let _serial = ISOLATION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("isolated");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777))
        .expect("测试目录需对目标用户可写");
    let file = root.join("new.txt");
    let _ = std::fs::remove_file(&file);

    std::env::set_var("TCHAT_ISOLATED_TOOLS", test_user());
    let exe: &std::path::Path = env!("CARGO_BIN_EXE_terminal-chat").as_ref();
    let patch = format!(
        "*** Begin Patch\n*** Add File: {}\n+content\n*** End Patch\n",
        file.display()
    );
    let (result, changes, uid) = run_isolated(exe, &patch);
    std::env::remove_var("TCHAT_ISOLATED_TOOLS");
    let expect_uid = Command::new("/usr/bin/id")
        .args(["-u", &test_user()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().unwrap())
        .expect("解析目标用户 uid");
    assert!(result.ok, "隔离下新增失败：{result:?}");
    assert_eq!(uid, Some(expect_uid), "执行子进程 uid＝目标用户（身份直证）");
    assert_eq!(changes.len(), 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "content\n");
    let _ = std::fs::remove_dir_all(&root);
}

/// 隔离下 Update/Delete（规格验收清单要求三类操作）。文件先经隔离 Add 建立
/// （目标用户属主）——Update 需要打开已存在文件写入，属主才行（目录 0777
/// 只授予创建/删除；dev 属主 0644 文件对目标用户不可写，与 9p 写位实测同构）。
#[test]
fn isolated_apply_update_and_delete() {
    let _serial = ISOLATION_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("upd-del");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
    }
    let file = root.join("u.txt");
    std::env::set_var("TCHAT_ISOLATED_TOOLS", test_user());
    let exe: &std::path::Path = env!("CARGO_BIN_EXE_terminal-chat").as_ref();
    let add = format!(
        "*** Begin Patch\n*** Add File: {}\n+old-line\n*** End Patch\n",
        file.display()
    );
    let (added, _, _) = run_isolated(exe, &add);
    assert!(added.ok, "隔离 Add（建立属主）：{added:?}");

    let (result, _, uid) = run_isolated(exe, &update_patch(&file, "old-line", "new-line"));
    assert!(result.ok, "隔离 Update 失败：{result:?}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new-line\n");
    let expect_uid: u32 = Command::new("/usr/bin/id")
        .args(["-u", &test_user()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse().unwrap())
        .unwrap();
    assert_eq!(uid, Some(expect_uid), "Update 执行身份直证");

    let del = format!(
        "*** Begin Patch\n*** Delete File: {}\n*** End Patch\n",
        file.display()
    );
    let (result, _, _) = run_isolated(exe, &del);
    std::env::remove_var("TCHAT_ISOLATED_TOOLS");
    assert!(result.ok, "隔离 Delete 失败：{result:?}");
    assert!(!file.exists(), "Delete 后文件应移除");
    let _ = std::fs::remove_dir_all(&root);
}
