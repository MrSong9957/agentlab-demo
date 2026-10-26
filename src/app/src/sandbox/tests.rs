//! sandbox 单元测试：argv 组装（纯逻辑，不依赖 bwrap）＋fail-closed 安全检查＋
//! 启用判定。bwrap 真实执行的端到端验证按环境探测结果分支断言，见
//! exec_command / mcp_stdio 集成测试（无 bwrap 环境＝failIfUnavailable 语义，
//! 有 bwrap 环境＝真实访问验证，两种环境都有断言、无静默跳过）。

use super::*;
use std::fs;

/// 每用例独立临时工作区（规范：独立临时目录、结束后清理；不触真实用户环境）。
fn tmp_ws(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tchat-sandbox-{}-{tag}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    // canonical 化，规避 /tmp 级符号链接别名对 --chdir 分支的干扰（该分支
    // 由 cwd_alias 测试单独覆盖）。
    fs::canonicalize(&dir).unwrap()
}

/// T1 策略 argv：全盘只读 ＋ 工作区可写 ＋ .env 遮蔽，参数顺序与上游 mount 层级
/// 一致（ro-bind 根→dev→bind 可写根→根内 deny 遮蔽→会话/命名空间→cap-drop→--）。
/// 手写单可写根策略以精确断言层级（policy_for 的 temp 可写根另测）。
#[test]
fn policy_argv_binds_root_writable_and_masks_env() {
    let ws = tmp_ws("argv");
    fs::write(ws.join(".env"), "FAKE-TEST-TOKEN=1\n").unwrap();
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![ws.clone()],
        unreadable_roots: vec![ws.join(".env")],
    };
    let args = create_bwrap_command_args(
        vec!["sh".into(), "-c".into(), "echo hi".into()],
        &policy,
        &ws,
        true,
    )
    .unwrap();
    let expected = vec![
        "--as-pid-1".to_string(),
        "--new-session".to_string(),
        "--die-with-parent".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        // 可写根 bind（deny 遮蔽在其后，遮蔽覆盖 bind——上游层级不变式）
        "--bind".to_string(),
        path_to_string(&ws),
        path_to_string(&ws),
        "--ro-bind".to_string(),
        "/dev/null".to_string(),
        path_to_string(&ws.join(".env")),
        // WSL interop 遮蔽（G4）
        "--tmpfs".to_string(),
        "/run/WSL".to_string(),
        // PID/IPC/用户命名空间＋新 procfs 成对（G4-R1/G3 修复：/proc/self/fd
        // 消费者恢复＋主进程退出沙箱 /proc 视图；不再 tmpfs /proc）
        "--unshare-user".to_string(),
        "--unshare-pid".to_string(),
        "--unshare-ipc".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--".to_string(),
        "sh".to_string(),
        "-c".to_string(),
        "echo hi".to_string(),
    ];
    assert_eq!(args, expected, "deny 遮蔽必须出现在可写根 bind 之后");
    fs::remove_dir_all(&ws).ok();
}

/// T1g WSL interop 遮蔽按 `/run/WSL` 存在性条件化（上游 `mask_wsl_interop:
/// … && Path::new(WSL_INTEROP_DIR).is_dir()`，linux_run_main.rs:278）。
/// 2026-10-27 宿主验收发现的缺陷回归：适配初版无条件 `--tmpfs /run/WSL`，
/// 在无该目录的环境（Docker 容器）里 bwrap 须在 ro-bind 根上 mkdir 目标，
/// 直接 EROFS 失败——整个沙箱不可用。两分支：目录在＝遮蔽（WSL 部署面），
/// 不在＝不产生该参数（Docker/裸机部署面）。
#[test]
fn wsl_interop_mask_follows_dir_presence_flag() {
    let ws = tmp_ws("wsl-mask");
    fs::write(ws.join(".env"), "FAKE-TEST-TOKEN=8\n").unwrap();
    let policy = policy_for(&ws);
    let with_mask =
        create_bwrap_command_args(vec!["sh".into()], &policy, &ws, true).unwrap();
    assert!(
        with_mask.windows(2).any(|w| w == ["--tmpfs", "/run/WSL"]),
        "mask=true（/run/WSL 存在）应保留遮蔽：{with_mask:?}"
    );
    let without_mask =
        create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    assert!(
        !without_mask.iter().any(|a| a == "/run/WSL"),
        "mask=false（/run/WSL 不存在）不得产生遮蔽参数（mkdir 于 ro 根必 EROFS）：{without_mask:?}"
    );
    fs::remove_dir_all(&ws).ok();
}

/// T1b 生产策略（policy_for）包含 temp 可写根（对齐 Claude Code 默认写面），
/// 且可写根按深度排序——宽根（/tmp）先 bind、深根（工作区）后 bind 重开。
#[test]
fn policy_for_includes_temp_writable_root() {
    let ws = tmp_ws("policy-temp");
    let policy = policy_for(&ws);
    assert!(
        policy.writable_roots.iter().any(|p| p == &std::env::temp_dir()),
        "temp 目录应在可写根内"
    );
    let args =
        create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    let tmp_bind = args
        .windows(3)
        .any(|w| w == ["--bind", "/tmp", "/tmp"]);
    assert!(tmp_bind, "argv 应包含 temp 可写根 bind：{args:?}");
    fs::remove_dir_all(&ws).ok();
}

/// T1c deny 目标为目录＝`--perms 000`＋`--tmpfs`＋`--remount-ro` 遮蔽
///（上游 `append_existing_unreadable_path_args` 目录分支）。
#[test]
fn denied_directory_is_masked_with_readonly_tmpfs() {
    let ws = tmp_ws("deny-dir");
    fs::create_dir_all(ws.join("secrets")).unwrap();
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![ws.clone()],
        unreadable_roots: vec![ws.join("secrets")],
    };
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    let mask_at = args
        .windows(6)
        .position(|w| {
            w == ["--perms", "000", "--tmpfs", &path_to_string(&ws.join("secrets")),
                  "--remount-ro", &path_to_string(&ws.join("secrets"))]
        })
        .expect("目录 deny 应产生 perms 000＋tmpfs＋remount-ro 遮蔽：{args:?}");
    let bind_at = args
        .windows(3)
        .position(|w| w == ["--bind", &path_to_string(&ws), &path_to_string(&ws)])
        .expect("可写根 bind 必须存在");
    assert!(mask_at > bind_at, "目录遮蔽必须在可写根 bind 之后（层级不变式）");
    fs::remove_dir_all(&ws).ok();
}

/// T1d deny 在全部可写根之外（rootless 分支）＝仍被遮蔽（不因不可写而漏保护）。
#[test]
fn deny_outside_writable_roots_is_still_masked() {
    let ws = tmp_ws("rootless");
    let outside_dir = std::env::temp_dir().join(format!("tchat-sb-outside-{}", std::process::id()));
    fs::create_dir_all(&outside_dir).unwrap();
    let outside_env = outside_dir.join(".env");
    fs::write(&outside_env, "FAKE-TEST-TOKEN=3\n").unwrap();
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![ws.clone()],
        unreadable_roots: vec![outside_env.clone()],
    };
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    assert!(
        args.windows(3).any(|w| w == ["--ro-bind", "/dev/null", &path_to_string(&outside_env)]),
        "可写根之外的 deny 路径仍应被遮蔽：{args:?}"
    );
    let _ = fs::remove_file(&outside_env);
    let _ = fs::remove_dir_all(&outside_dir);
    fs::remove_dir_all(&ws).ok();
}

/// T1e 可写根本身是符号链接＝bind canonical 目标（上游 `mount_root` 语义）。
#[test]
fn writable_root_symlink_binds_canonical_target() {
    let real = tmp_ws("ws-real");
    fs::write(real.join(".env"), "FAKE-TEST-TOKEN=4\n").unwrap();
    let link_parent = tmp_ws("ws-link-parent");
    let link = link_parent.join("ws-link");
    let _ = fs::remove_file(&link);
    symlink_or_skip(&real, &link);
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![link.clone()],
        unreadable_roots: vec![link.join(".env")],
    };
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &real, false).unwrap();
    assert!(
        args.windows(3).any(|w| w == ["--bind", &path_to_string(&real), &path_to_string(&real)]),
        "符号链接可写根应 bind canonical 目标：{args:?}"
    );
    // deny 路径 remap 到 canonical 视图（remap_paths_for_symlink_target）。
    assert!(
        args.windows(3)
            .any(|w| w == ["--ro-bind", "/dev/null", &path_to_string(&real.join(".env"))]),
        "deny 路径应随可写根 remap 到 canonical：{args:?}"
    );
    let _ = fs::remove_file(&link);
    fs::remove_dir_all(&real).ok();
    fs::remove_dir_all(&link_parent).ok();
}

/// T1f 探测：bwrap 路径无法执行（ENOENT 场景以不存在路径注入）＝显式错误含原因，
/// 而非 panic 或静默 Ok（failIfUnavailable 的探测层证据；dry-run 成功分支需真实
/// bwrap，属环境阻断项）。
#[test]
fn probe_with_nonexistent_bwrap_reports_error() {
    let fake = std::env::temp_dir().join(format!("tchat-sb-no-such-{}", std::process::id()));
    let _ = fs::remove_file(&fake);
    let err = resolve_launcher_with(&fake).unwrap_err();
    assert!(err.contains("无法执行"), "实际：{err}");
}

/// T9（G4-R1/G3 修复回归，先 RED 后 GREEN）：PID 命名空间与新 procfs 成对恢复——
/// `--unshare-pid`＋`--as-pid-1`（上游 launcher 语义：bwrap 为子 ns 的 pid 1）
/// ＋`--proc /proc`（新 procfs 保留 `/proc/self/fd`，apply_patch Update 的 F2
/// 写绑定恢复；主进程从沙箱 /proc 视图消失＝G3 读/kill 面关闭）。成对语义下
/// 不再有 `--tmpfs /proc`（上游：tmpfs 仅在未挂新 procfs 时遮蔽继承视图）。
#[test]
fn pid_namespace_and_proc_paired() {
    let ws = tmp_ws("pidns");
    fs::write(ws.join(".env"), "FAKE-TEST-TOKEN=7\n").unwrap();
    let policy = policy_for(&ws);
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    assert_eq!(
        args[0], "--as-pid-1",
        "bwrap 应为子 PID 命名空间的 pid 1（上游 launcher 语义）：{args:?}"
    );
    assert!(
        args.windows(2).any(|w| w == ["--unshare-pid", "--unshare-ipc"]),
        "应成对 unshare PID/IPC 命名空间：{args:?}"
    );
    let proc_at = args
        .windows(2)
        .position(|w| w == ["--proc", "/proc"])
        .expect("应挂新 procfs（/proc/self/fd 消费者依赖）：{args:?}");
    assert!(
        !args.windows(2).any(|w| w == ["--tmpfs", "/proc"]),
        "新 procfs 已挂，不得再 tmpfs 遮蔽（上游成对语义）：{args:?}"
    );
    // 文件遮蔽仍在新 procfs 挂载之前（挂载序不变式不因成对机制改变）。
    let mask_at = args
        .windows(3)
        .position(|w| w == ["--ro-bind", "/dev/null", &path_to_string(&ws.join(".env"))])
        .expect("deny 遮蔽必须存在：{args:?}");
    assert!(mask_at < proc_at, "文件遮蔽先于 /proc 挂载（层级不变式）：{args:?}");
    fs::remove_dir_all(&ws).ok();
}

/// T2 deny 路径穿可写目录内的符号链接＝组装失败（上游 TOCTTOU fail-closed，
/// 注释逐字保留的那条路径）。
#[test]
fn deny_path_through_writable_symlink_fails_closed() {
    let ws = tmp_ws("symlink");
    fs::create_dir_all(ws.join("real")).unwrap();
    symlink_or_skip(&ws.join("real"), &ws.join("sub"));
    let mut policy = policy_for(&ws);
    policy.unreadable_roots = vec![ws.join("sub").join(".env")];
    let err = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap_err();
    assert!(
        err.contains("writable symlink") && err.contains("deny-read"),
        "fail-closed 错误应指明 deny 路径与可写符号链接，实际：{err}"
    );
    fs::remove_dir_all(&ws).ok();
}

/// T3 deny 目标自身是符号链接＝组装失败（与 027 open_nofollow_chain「符号链接
/// 拒绝」同语义：遮蔽只保护当前目标，逻辑路径可被沙箱内改写）。
#[test]
fn deny_target_itself_symlink_fails_closed() {
    let ws = tmp_ws("deny-symlink");
    fs::write(ws.join("secret"), "FAKE-TEST-TOKEN=2\n").unwrap();
    symlink_or_skip(&ws.join("secret"), &ws.join(".env"));
    let policy = policy_for(&ws);
    let err = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap_err();
    assert!(err.contains("writable symlink"), "实际：{err}");
    fs::remove_dir_all(&ws).ok();
}

/// T4 deny 路径不存在＝no-op 不遮蔽（上游预创建空文件的差异点，见适配注释）。
#[test]
fn missing_deny_path_is_noop() {
    let ws = tmp_ws("missing");
    let policy = policy_for(&ws);
    let args =
        create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    assert!(
        !args.iter().any(|a| a.contains("absent")),
        "不存在的 deny 路径不应产生任何参数"
    );
    fs::remove_dir_all(&ws).ok();
}

/// T5 cwd 符号链接别名：命令 cwd 进入 canonical（上游 --chdir 分支）。
#[test]
fn symlinked_command_cwd_gets_chdir() {
    let ws = tmp_ws("cwd-alias");
    let alias = tmp_ws("cwd-alias-link-parent").join("alias");
    let _ = fs::remove_file(&alias);
    symlink_or_skip(&ws, &alias);
    let policy = policy_for(&ws);
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &alias, false).unwrap();
    let idx = args
        .iter()
        .position(|a| a == "--chdir")
        .expect("符号链接别名 cwd 应触发 --chdir");
    assert_eq!(args[idx + 1], path_to_string(&ws), "chdir 目标应为 canonical 工作区");
    let _ = fs::remove_file(&alias);
    fs::remove_dir_all(&ws).ok();
}

/// T6 启用判定：TCHAT_SANDBOX 未设/空＝false；非空＝true（进程级 env，锁内执行）。
#[test]
fn sandbox_requested_reads_env() {
    let _g = crate::TEST_ENV_LOCK.lock().unwrap();
    std::env::remove_var("TCHAT_SANDBOX");
    assert!(!sandbox_requested());
    std::env::set_var("TCHAT_SANDBOX", "");
    assert!(!sandbox_requested(), "空值不启用");
    std::env::set_var("TCHAT_SANDBOX", "1");
    assert!(sandbox_requested());
    std::env::remove_var("TCHAT_SANDBOX");
}

/// T2b（G2 回归，安全攻防 S4a）：非 canonical 可写根（`sub/..` 内核解析回父级）
/// ＝拒绝组装——词法失配会让遮蔽先于该根 bind，挂载序后写者胜、遮蔽被重开。
#[test]
fn non_canonical_writable_root_fails_closed() {
    let ws = tmp_ws("s4a");
    fs::write(ws.join(".env"), "FAKE-TEST-TOKEN=5\n").unwrap();
    fs::create_dir_all(ws.join("sub")).unwrap();
    let raw = ws.join("sub").join(".."); // 内核解析回 ws；词法非 canonical
    fs::write(raw.join(".env"), "FAKE-TEST-TOKEN=6\n").unwrap(); // 同一文件（经解析）
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![raw],
        unreadable_roots: vec![ws.join(".env")],
    };
    let err = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap_err();
    assert!(
        err.contains("词法非清洁"),
        "非词法清洁可写根必须显式拒绝：{err}"
    );
    fs::remove_dir_all(&ws).ok();
}

/// T7 统一入口：未启用＝None 且不触发探测；启用后按环境分支——无 bwrap＝
/// 显式 Err（failIfUnavailable，绝不返回 None 静默回退）；有 bwrap＝真实包装。
/// 策略锚定为主进程 cwd（G1 修复）：接口不收目录参数，调用方无法转移锚定。
#[test]
fn tool_launch_argv_fails_closed_when_unavailable() {
    let _g = crate::TEST_ENV_LOCK.lock().unwrap();
    std::env::remove_var("TCHAT_SANDBOX");
    let ws = tmp_ws("launch");
    match tool_launch_argv(vec!["sh".into()]) {
        Ok(None) => {}
        other => panic!("未启用沙箱应返回 None，实际 {other:?}"),
    }
    std::env::set_var("TCHAT_SANDBOX", "1");
    let result = tool_launch_argv(vec!["sh".into(), "-c".into(), "id".into()]);
    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("bwrap") || msg.contains("writable symlink"),
                "不可用原因应指向 bwrap（或 fail-closed），实际：{msg}"
            );
        }
        Ok(Some((program, args, env))) => {
            // 有 bwrap 的环境：真实包装断言（证据面，等价于沙箱环境下的 T1）。
            assert!(program.ends_with("bwrap"), "program={program}");
            assert_eq!(args.last().map(String::as_str), Some("id"));
            assert!(env.iter().all(|(k, _)| k != "ANTHROPIC_API_KEY"));
        }
        Ok(None) => panic!("已请求沙箱却返回 None＝静默回退，禁止"),
    }
    std::env::remove_var("TCHAT_SANDBOX");
    fs::remove_dir_all(&ws).ok();
}

/// T8 白名单环境：凭据类 env（.env dotenv 加载面）不进沙箱子进程环境。
#[test]
fn child_env_whitelist_excludes_credentials() {
    let _g = crate::TEST_ENV_LOCK.lock().unwrap();
    std::env::set_var("TCHAT_SANDBOX_TEST_CRED", "FAKE-SECRET");
    std::env::set_var("TERM", "dumb");
    let ws = tmp_ws("env");
    let env = tool_child_env(&ws);
    assert!(env.iter().any(|(k, v)| k == "HOME" && Path::new(v) == ws));
    assert!(env.iter().any(|(k, _)| k == "PATH"));
    assert!(env.iter().any(|(k, v)| k == "TERM" && v == "dumb"));
    assert!(
        !env.iter().any(|(k, _)| k == "TCHAT_SANDBOX_TEST_CRED"),
        "白名单外的 env（含潜在凭据）不得进入工具子进程"
    );
    std::env::remove_var("TCHAT_SANDBOX_TEST_CRED");
    std::env::remove_var("TERM");
    fs::remove_dir_all(&ws).ok();
}

fn symlink_or_skip(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        panic!("符号链接用例仅适用于 unix");
    }
}
