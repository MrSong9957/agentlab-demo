//! 033：apply_patch 沙箱模式——补丁经 bwrap 包装的 `--internal-apply` 子进程，
//! 读、判型、写全部在沙箱内完成。环境双分支：无 bwrap＝显式失败
//!（failIfUnavailable，绝不静默回退为进程内执行，文件不得产生）；有 bwrap＝
//! Add/Update/Delete 真实落盘验证。全程假内容，不触真实凭据。

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::json;

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
    terminal_chat::apply_patch::apply_patch_sandboxed_with_exe(exe, &input_of(patch), None)
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

// ---------- 029 M5：沙箱 Move（真实 bwrap 正例、越界目标、.env 遮蔽源、包装不可用零执行）----------

/// 沙箱 Move＋Unicode 上下文：有 bwrap＝移动/修改在沙箱内真实完成（两条记录、
/// 两张真实内容卡片、未归一化上下文保留）；无 bwrap＝显式失败且零文件变更。
#[test]
fn sandbox_move_roundtrip_with_unicode_context() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("029-move");
    let src = root.join("源 文件.txt");
    let dst = root.join("归档/目标 文件.txt");
    std::fs::write(&src, "\u{201C}标题\u{201D}\nmode=dev\n").unwrap();

    std::env::set_var("TCHAT_SANDBOX", "1");
    let exe: &std::path::Path = env!("CARGO_BIN_EXE_terminal-chat").as_ref();
    let patch = format!(
        "*** Begin Patch\n*** Update File: {s}\n*** Move to: {d}\n@@ \"标题\"\n-mode=dev\n+mode=sandbox\n*** End Patch\n",
        s = src.display(),
        d = dst.display()
    );
    let (result, changes) = run_sandboxed(exe, &patch);
    std::env::remove_var("TCHAT_SANDBOX");
    if result.ok {
        assert!(result.state_known, "{result:?}");
        assert_eq!(
            result.changed_files.iter().map(|c| (c.path.clone(), c.action)).collect::<Vec<_>>(),
            vec![
                (dst.display().to_string(), terminal_chat::apply_patch::Action::Add),
                (src.display().to_string(), terminal_chat::apply_patch::Action::Delete),
            ],
            "两条有序记录：{result:?}"
        );
        assert_eq!(changes.len(), 2, "两张展示卡片：{changes:?}");
        assert!(!src.exists(), "源已在沙箱内删除");
        // 未归一化上下文保留；替换行按补丁写入
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "\u{201C}标题\u{201D}\nmode=sandbox\n",
            "目标内容"
        );
    } else {
        let msg = result.error.as_deref().unwrap_or_default();
        assert!(msg.contains("沙箱"), "失败原因应指向沙箱不可用：{msg}");
        assert!(!dst.exists(), "fail-closed 时不得创建目标");
        assert!(src.exists(), "fail-closed 时不得删除源");
        assert_eq!(
            std::fs::read_to_string(&src).unwrap(),
            "\u{201C}标题\u{201D}\nmode=dev\n",
            "源字节不变"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// 越界目标：工作区与系统临时目录之外的路径（根文件系统 ro-bind）——目标不得
/// 创建、源不得删除；无 bwrap 分支＝沙箱显式失败且零变更。
#[test]
fn sandbox_move_out_of_writable_root_rejected() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("029-oow");
    let src = root.join("in-bounds.txt");
    std::fs::write(&src, "payload\n").unwrap();
    // bwrap 全盘 ro-bind：根下不可写（uid 1000 双保险）——确定性拒绝
    let dst = std::path::PathBuf::from("/tchat-029-out-of-bounds.txt");
    let _ = std::fs::remove_file(&dst);

    std::env::set_var("TCHAT_SANDBOX", "1");
    let exe: &std::path::Path = env!("CARGO_BIN_EXE_terminal-chat").as_ref();
    let patch = format!(
        "*** Begin Patch\n*** Update File: {s}\n*** Move to: {d}\n*** End Patch\n",
        s = src.display(),
        d = dst.display()
    );
    let (result, _) = run_sandboxed(exe, &patch);
    std::env::remove_var("TCHAT_SANDBOX");
    if result.error.as_deref().is_some_and(|m| m.contains("沙箱")) && !result.ok {
        // 无 bwrap 环境：fail-closed 分支
        assert!(!dst.exists() && src.exists(), "零文件变更");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    assert!(!result.ok, "越界目标必须失败：{result:?}");
    assert!(!result.state_known, "创建目标失败＝变更操作失败：{result:?}");
    assert_eq!(result.changed_files, vec![], "不得记录任何变更：{result:?}");
    assert!(!dst.exists(), "越界目标不得创建");
    assert_eq!(
        std::fs::read_to_string(&src).unwrap(),
        "payload\n",
        "源完整保留（不得先删源）"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 遮蔽源：锚定根（主进程 cwd，经 chdir 重定向到临时根）下的假 `.env` 作为移动
/// 源——沙箱内被 /dev/null 遮蔽（非常规文件）拒绝；假凭据正文不可得；无 bwrap
/// 分支＝沙箱显式失败。恢复 cwd 后清理。
#[test]
fn sandbox_move_masked_env_source_rejected() {
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("029-env");
    let env_file = root.join(".env");
    std::fs::write(&env_file, "FAKE-SECRET-029=中文凭据\n").unwrap();
    let dst = root.join("leaked.txt");

    let saved_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap(); // 策略锚点重定向到临时根
    std::env::set_var("TCHAT_SANDBOX", "1");
    let exe: &std::path::Path = env!("CARGO_BIN_EXE_terminal-chat").as_ref();
    let patch = format!(
        "*** Begin Patch\n*** Update File: .env\n*** Move to: {}\n*** End Patch\n",
        dst.display()
    );
    let (result, _) = run_sandboxed(exe, &patch);
    std::env::remove_var("TCHAT_SANDBOX");
    std::env::set_current_dir(saved_cwd).unwrap();
    if result.error.as_deref().is_some_and(|m| m.contains("沙箱")) && !result.ok {
        assert!(!dst.exists(), "fail-closed 不得创建目标");
        assert_eq!(
            std::fs::read_to_string(&env_file).unwrap(),
            "FAKE-SECRET-029=中文凭据\n",
            "假凭据原样保留"
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    assert!(!result.ok, "遮蔽源必须失败：{result:?}");
    assert!(result.state_known, "校验型失败（未发生写入）：{result:?}");
    assert_eq!(result.changed_files, vec![], "{result:?}");
    let msg = result.error.as_deref().unwrap_or_default();
    // 实测（2026-10-28 手动 bwrap 探针）：遮蔽目标为 /dev/null（字符设备）且在
    // user namespace 内打开即 EACCES——两种形态都使正文不可得，断言取并集。
    assert!(
        msg.contains("打开移动源失败") || msg.contains("不是常规文件"),
        "错误应指向遮蔽源不可打开/非常规：{msg}"
    );
    assert!(!msg.contains("中文凭据"), "错误不得回显假凭据正文：{msg}");
    assert!(!dst.exists(), "不得创建泄露目标");
    assert_eq!(
        std::fs::read_to_string(&env_file).unwrap(),
        "FAKE-SECRET-029=中文凭据\n",
        "假凭据正文不可得且本体不动"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 包装不可用＝零执行（033 F 同款隔离）：子进程内替换 PATH（无 bwrap）＋
/// TCHAT_SANDBOX=1 调真实 `apply_patch_sandboxed` ——探测缓存为进程级，必须经
/// 子进程绕开（本测试进程可能已缓存真实 bwrap）。源与目标零变更。
fn offline_child_mode() -> bool {
    let Some(input) = std::env::var_os("TCHAT_029_OFFLINE_INPUT") else {
        return false;
    };
    let output = std::env::var_os("TCHAT_029_OFFLINE_OUTPUT").expect("缺少 TCHAT_029_OFFLINE_OUTPUT");
    let spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&input).unwrap()).unwrap();
    let (result, changes) = terminal_chat::apply_patch::apply_patch_sandboxed(
        &serde_json::json!({ "patch": spec["patch"].as_str().unwrap() }),
        None,
    );
    std::fs::write(
        output,
        serde_json::json!({ "result": result, "changes": changes }).to_string(),
    )
    .unwrap();
    std::process::exit(0);
}

#[test]
fn sandbox_move_wrapper_unavailable_zero_execution() {
    if offline_child_mode() {
        unreachable!("子进程已在 offline_child_mode 内退出");
    }
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("029-offline");
    let src = root.join("src.txt");
    let dst = root.join("dst.txt");
    std::fs::write(&src, "keep\n").unwrap();
    let patch = format!(
        "*** Begin Patch\n*** Update File: {s}\n*** Move to: {d}\n*** End Patch\n",
        s = src.display(),
        d = dst.display()
    );
    // 隔离子进程：PATH 指向空目录（bwrap 缺失）＋开关置 1
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).unwrap();
    let io = root.join("io");
    std::fs::create_dir_all(&io).unwrap();
    let input = io.join("in.json");
    let output = io.join("out.json");
    std::fs::write(&input, serde_json::json!({ "patch": patch }).to_string()).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("sandbox_move_wrapper_unavailable_zero_execution")
        .env("TCHAT_029_OFFLINE_INPUT", &input)
        .env("TCHAT_029_OFFLINE_OUTPUT", &output)
        .env("TCHAT_SANDBOX", "1")
        .env("PATH", &fake_bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .expect("子进程启动失败");
    assert!(status.status.success(), "子进程异常退出：{status:?}");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&output).unwrap())
        .unwrap_or_else(|e| panic!("未写结果：{e}"));
    let result = &v["result"];
    assert_eq!(result["ok"], json!(false), "包装不可用必须显式失败：{result}");
    let msg = result["error"].as_str().unwrap();
    assert!(msg.contains("bwrap") || msg.contains("沙箱"), "错误指向包装不可用：{msg}");
    assert_eq!(v["changes"].as_array().unwrap().len(), 0, "零展示变更");
    // 零执行：源字节不变、目标不出现——无主进程代办
    assert_eq!(std::fs::read_to_string(&src).unwrap(), "keep\n", "源完整保留");
    assert!(!dst.exists(), "目标不得被主进程代办创建");
    let _ = std::fs::remove_dir_all(&root);
}
