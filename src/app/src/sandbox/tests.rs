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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
// 038：纯快照构造（不依赖 bwrap//proc）。macOS 上 temp_dir 经产品侧
// canonicalize 落在 /private/var/…——断言对齐产品自身的规范化。Windows：
// temp 可写根 argv 断言依赖 bind 布局（Linux 沙箱设施），门控。
#[cfg(not(target_os = "windows"))]
fn policy_for_includes_temp_writable_root() {
    let ws = tmp_ws("policy-temp");
    let policy = policy_for(&ws);
    let temp_canon =
        std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir());
    assert!(
        policy.writable_roots.iter().any(|p| p == &temp_canon),
        "temp 目录应在可写根内"
    );
    let args =
        create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    // 038：产品侧对可写根做 canonicalize（macOS temp_dir 落在 /var→/private/var
    // 符号链接后）——断言用同源规范化后的 temp 路径比较。
    let temp_canon =
        std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir());
    let tmp_expected = [
        "--bind".to_string(),
        temp_canon.to_string_lossy().into_owned(),
        temp_canon.to_string_lossy().into_owned(),
    ];
    let tmp_bind = args.windows(3).any(|w| w == tmp_expected);
    assert!(tmp_bind, "argv 应包含 temp 可写根 bind：{args:?}");
    fs::remove_dir_all(&ws).ok();
}

/// T1c deny 目标为目录＝`--perms 000`＋`--tmpfs`＋`--remount-ro` 遮蔽
///（上游 `append_existing_unreadable_path_args` 目录分支）。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
/// `--unshare-pid`（bwrap 内建 PID 1 回收器等待初始载荷）
/// ＋`--proc /proc`（新 procfs 保留 `/proc/self/fd`，apply_patch Update 的 F2
/// 写绑定恢复；主进程从沙箱 /proc 视图消失＝G3 读/kill 面关闭）。成对语义下
/// 不再有 `--tmpfs /proc`（上游：tmpfs 仅在未挂新 procfs 时遮蔽继承视图）。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
fn pid_namespace_and_proc_paired() {
    let ws = tmp_ws("pidns");
    fs::write(ws.join(".env"), "FAKE-TEST-TOKEN=7\n").unwrap();
    let policy = policy_for(&ws);
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    assert!(
        !args.iter().any(|arg| arg == "--as-pid-1"),
        "应由 bubblewrap 内建 PID 1 回收器等待载荷：{args:?}"
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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

/// B1（2026-10-27 加固）：deny 目标为常规文件且存在多个硬链接（nlink>1）＝
/// 拒绝组装——路径级遮蔽（挂载在路径上）不覆盖同 inode 的其他路径，别名不可
/// 枚举，只能拒绝该次启动（fail closed，不回退普通模式）。只处理「检查时已
/// 存在」的硬链接：检查到挂载之间及运行期的并发变化仍是边界（不构成完整
/// 竞态防护），见 docs/logs/033-tool-sandbox/2026-10-27-b1-b2-risk-verification.md。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
fn deny_target_with_hardlinks_fails_closed() {
    let ws = tmp_ws("deny-hardlink");
    // 夹具预清理（幂等）：上次失败运行的残留会让 hard_link EEXIST 掩盖真实信号。
    let _ = fs::remove_dir_all(&ws);
    fs::create_dir_all(&ws).unwrap();
    let env = ws.join(".env");
    fs::write(&env, "FAKE-TEST-TOKEN=1\n").unwrap();
    fs::hard_link(&env, ws.join("env-alias")).unwrap();
    let policy = policy_for(&ws);
    let err = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap_err();
    assert!(
        err.contains("硬链接") && err.contains(&path_to_string(&env)),
        "fail-closed 错误应指明受保护文件与硬链接，实际：{err}"
    );
    fs::remove_dir_all(&ws).ok();
}

/// B1 控制组：单链接（nlink=1）deny 目标照常遮蔽（既有 T1 层级语义不变）。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
fn deny_target_single_link_still_masks() {
    let ws = tmp_ws("deny-single");
    let env = ws.join(".env");
    fs::write(&env, "FAKE-TEST-TOKEN=2\n").unwrap();
    let policy = policy_for(&ws);
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    assert!(
        args.windows(3)
            .any(|w| w == ["--ro-bind", "/dev/null", &path_to_string(&env)]),
        "单链接 deny 照常遮蔽：{args:?}"
    );
    fs::remove_dir_all(&ws).ok();
}

/// B1 边界口径：可写根之外的符号链接 deny（不触发可写符号链接 fail-closed
/// 分支）＝检查**跟随到的实际文件**的链接数（符号链接自身恒为 1，查链接本身
/// 会漏判）；实际文件多链接时同样拒绝。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
fn rootless_symlink_deny_follows_to_actual_file_nlink() {
    let ws = tmp_ws("deny-sl-nlink");
    let outside = std::env::temp_dir().join(format!("tchat-sb-out-sl-{}", std::process::id()));
    // 夹具预清理（幂等）：残留的 alias 硬链接/.env 符号链接会让 EEXIST 掩盖真实信号。
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).unwrap();
    let target = outside.join("real-secret");
    fs::write(&target, "FAKE-TEST-TOKEN=3\n").unwrap();
    fs::hard_link(&target, outside.join("alias")).unwrap();
    symlink_or_skip(&target, &outside.join(".env"));
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![ws.clone()],
        unreadable_roots: vec![outside.join(".env")],
    };
    let err = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap_err();
    assert!(
        err.contains("硬链接"),
        "应跟随符号链接检查实际文件链接数并拒绝，实际：{err}"
    );
    let _ = fs::remove_dir_all(&outside);
    fs::remove_dir_all(&ws).ok();
}

/// B1 收口（2026-10-27）：deny 目标 stat 失败且**非** NotFound/NotADirectory＝
/// 显式拒绝（链接数与目标状态不明，fail closed）。稳定触发：可写根之外的自指
/// 符号链接（metadata 恒 ELOOP；不触发可写符号链接分支——该分支只拒绝
/// **可写根内**的符号链接，遍历对根外符号链接放行）。此前 `exists()` 口径对
/// 任何 stat 错误都返回 false，会把这类错误静默当 missing 跳过检查（不遮蔽、
/// 不拒绝＝fail-open，本轮 RED 留档）。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
fn deny_target_metadata_error_fails_closed() {
    let ws = tmp_ws("deny-eloop");
    let outside = std::env::temp_dir().join(format!("tchat-sb-out-loop-{}", std::process::id()));
    // 夹具预清理（幂等）：残留的循环符号链接会让 symlink EEXIST 掩盖真实信号。
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).unwrap();
    let loop_path = outside.join(".env");
    symlink_or_skip(&loop_path, &loop_path); // 自指循环：metadata() 恒 ELOOP
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![ws.clone()],
        unreadable_roots: vec![loop_path.clone()],
    };
    let err = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap_err();
    assert!(
        err.contains("状态读取失败") && err.contains(&path_to_string(&loop_path)),
        "非 NotFound stat 错误应显式拒绝，实际：{err}"
    );
    let _ = fs::remove_dir_all(&outside);
    fs::remove_dir_all(&ws).ok();
}

/// B1 收口：deny 目标父目录不可穿越（权限 000，EACCES）＝同样显式拒绝。
/// 符号链接遍历遇 lstat 错误按上游语义 break 跳过（不构成拦截），非 NotFound
/// 分类必须由 metadata 检查兜底。夹具目录在工作区之外，锚定根不受影响；
    #[cfg(unix)] // 038：POSIX 权限/符号链接语义用例（Windows 无等价设施），见 specs/038 §5
/// 断言后先恢复权限再清理（否则自身 remove_dir_all 也失败）。
#[test]
fn deny_target_parent_permission_error_fails_closed() {
    use std::os::unix::fs::PermissionsExt;
    let ws = tmp_ws("deny-eacces");
    let box_ = std::env::temp_dir().join(format!("tchat-sb-box-{}", std::process::id()));
    let _ = fs::remove_dir_all(&box_);
    fs::create_dir_all(&box_).unwrap();
    let env = box_.join(".env");
    fs::write(&env, "FAKE-TEST-TOKEN=4\n").unwrap();
    fs::set_permissions(&box_, fs::Permissions::from_mode(0o000)).unwrap();
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![ws.clone()],
        unreadable_roots: vec![env.clone()],
    };
    let err = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap_err();
    fs::set_permissions(&box_, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        err.contains("状态读取失败") && err.contains(&path_to_string(&env)),
        "EACCES 应显式拒绝而非静默跳过检查，实际：{err}"
    );
    let _ = fs::remove_dir_all(&box_);
    fs::remove_dir_all(&ws).ok();
}

/// 控制组：路径组件非目录（ENOTDIR）与 NotFound 同类＝既有 missing no-op
/// 语义不变（不遮蔽、不拒绝；missing_deny_path_is_noop 覆盖 NotFound 面）。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
fn deny_target_enotdir_is_noop() {
    let ws = tmp_ws("deny-enotdir");
    let notdir = ws.join("plain-file");
    fs::write(&notdir, "x").unwrap();
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![ws.clone()],
        unreadable_roots: vec![notdir.join(".env")],
    };
    let args = create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false).unwrap();
    assert!(
        !args.iter().any(|a| a.contains(".env")),
        "ENOTDIR 与 NotFound 同类：no-op 不遮蔽：{args:?}"
    );
    fs::remove_dir_all(&ws).ok();
}

/// T5 cwd 符号链接别名：命令 cwd 进入 canonical（上游 --chdir 分支）。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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
    // 038 Windows 实证：PathBuf::join("..") 在 push 时折叠（raw 即干净路径，
    // 词法非清洁形态无法经 join 构造）——以字符串拼接保留 `sub\..` 原样；
    // 该形态仅 unix 上可写经（verbatim 路径内核不解析 ..，.env 改写跳过，
    // 边界对象由上方的 ws\.env 承载）。
    let raw = PathBuf::from(format!(
        "{}{}sub{}..",
        ws.display(),
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    ));
    #[cfg(not(windows))]
    fs::write(raw.join(".env"), "FAKE-TEST-TOKEN=6\n").unwrap(); // 同一文件（经解析）
    let policy = SandboxPolicy {
        full_disk_read: true,
        writable_roots: vec![raw.clone()],
        unreadable_roots: vec![ws.join(".env")],
    };
    let err = match create_bwrap_command_args(vec!["sh".into()], &policy, &ws, false) {
        Err(e) => e,
        Ok(argv) => panic!(
            "非词法清洁可写根必须显式拒绝：诊断 raw={raw:?} segments={segs:?} lex={lex:?} argv_n={argv}",
            raw = raw.to_string_lossy(),
            segs = raw
                .to_string_lossy()
                .split(['/', '\\'])
                .collect::<Vec<_>>(),
            lex = absolute_lexical(&raw).to_string_lossy(),
            argv = argv.len()
        ),
    };
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
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
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

// ---------- 031：动态策略（M2） ----------

fn sbx_dir(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "tchat-031-sbx-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// P06(b) 点 1/2：deny 目录内部文件的预存硬链接 → 拒绝（反例变体 A 闭合）。
#[cfg(unix)]
#[test]
fn deny_dir_internal_hardlink_refuses_launch() {
    let root = sbx_dir("p06-dir");
    let private = root.join("private");
    let allowed = root.join("allowed");
    fs::create_dir_all(&private).unwrap();
    fs::create_dir_all(&allowed).unwrap();
    fs::write(private.join("secret.txt"), "FAKE-P06-SECRET").unwrap();
    // 预存硬链接到「允许树」
    fs::hard_link(private.join("secret.txt"), allowed.join("alias.txt")).unwrap();

    // 无别名（nlink=1）：通过
    verify_deny_boundaries(&[private.join("nothing")], &DEFAULT_TRAVERSAL_BUDGET).unwrap();
    let mut deny = vec![private.clone()];
    // 目录内部存在 nlink=2 的文件 → 拒绝启动（fail-closed）
    let err = verify_deny_boundaries(&deny, &DEFAULT_TRAVERSAL_BUDGET).unwrap_err();
    assert!(err.contains("硬链接"), "目录内部硬链接应拒绝：{err}");
    deny.clear();
    drop(deny);
    fs::remove_dir_all(&root).ok();
}

/// P06(b) 点 1：deny 文件本身的 nlink 检查（033 B1 推广到 031 入口）。
#[cfg(unix)]
#[test]
fn deny_file_hardlink_refuses_launch() {
    let root = sbx_dir("p06-file");
    fs::write(root.join("a.txt"), "x").unwrap();
    fs::hard_link(root.join("a.txt"), root.join("b.txt")).unwrap();
    let err = verify_deny_boundaries(
        &[root.join("a.txt")],
        &DEFAULT_TRAVERSAL_BUDGET,
    )
    .unwrap_err();
    assert!(err.contains("硬链接"));
    fs::remove_file(root.join("b.txt")).unwrap();
    verify_deny_boundaries(&[root.join("a.txt")], &DEFAULT_TRAVERSAL_BUDGET).unwrap();
    fs::remove_dir_all(&root).ok();
}

/// P06(b) 点 2：遍历失败（不可读子目录）＝拒绝，不静默跳过。
#[cfg(unix)]
#[test]
fn deny_dir_traversal_failure_refuses() {
    use std::os::unix::fs::PermissionsExt;
    let root = sbx_dir("p06-eacces");
    let private = root.join("private");
    let sub = private.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("s.txt"), "x").unwrap();
    fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000)).unwrap();
    let err = verify_deny_boundaries(&[private.clone()], &DEFAULT_TRAVERSAL_BUDGET).unwrap_err();
    // 恢复权限再清理（夹具卫生）
    fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.contains("遍历失败") || err.contains("读取失败"), "不可访问应拒绝：{err}");
    fs::remove_dir_all(&root).ok();
}

/// P06(b) 点 4：条目数预算超限＝拒绝（fail-closed，不静默截断）。
#[test]
fn deny_traversal_budget_entries_refuses() {
    let root = sbx_dir("p06-budget");
    let private = root.join("private");
    fs::create_dir_all(&private).unwrap();
    for i in 0..10 {
        fs::write(private.join(format!("f{i}.txt")), "x").unwrap();
    }
    let budget = TraversalBudget { max_entries: 5, timeout_ms: 60_000 };
    let err = verify_deny_boundaries(&[private.clone()], &budget).unwrap_err();
    assert!(err.contains("预算"), "条目超限应拒绝：{err}");
    let ok_budget = TraversalBudget { max_entries: 20, timeout_ms: 60_000 };
    verify_deny_boundaries(&[private.clone()], &ok_budget).unwrap();
    fs::remove_dir_all(&root).ok();
}

/// P06(b) 点 4：墙钟超时＝拒绝（timeout=0 立即触发）。
#[test]
fn deny_traversal_budget_timeout_refuses() {
    let root = sbx_dir("p06-timeout");
    let private = root.join("private");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("f.txt"), "x").unwrap();
    let budget = TraversalBudget { max_entries: 100, timeout_ms: 0 };
    // 至少一个目录项后即超时（deadline 在遍历前已过）
    let err = verify_deny_boundaries(&[private.clone()], &budget).unwrap_err();
    assert!(err.contains("时限"), "超时应拒绝：{err}");
    fs::remove_dir_all(&root).ok();
}

/// P06(b) 点 3：mountinfo 别名枚举——同设备 bind 的两个挂载点互相显形
///（不能仅靠 st_dev 变化检测）。以本进程真实 /proc/self/mountinfo 验证：
/// 任一路径至少映射出自身（identity 投影）。
#[test]
#[cfg(target_os = "linux")]
// 038-CI：依赖 bwrap//proc/mountinfo（Linux 专属），见 docs/specs/038 §5。
fn deny_aliases_identity_projection() {
    let mounts = parse_mountinfo();
    assert!(!mounts.is_empty(), "mountinfo 应可解析");
    let root = sbx_dir("alias-id");
    let aliases = deny_aliases(&root, &mounts);
    assert!(aliases.contains(&root), "canonical 自身必须在别名集中");
    fs::remove_dir_all(&root).ok();
}

/// mountinfo octal 转义还原（`\040`＝空格）。
#[test]
fn mountinfo_unescapes_octal() {
    let p = unescape_mountinfo_path("/mnt/my\\040dir");
    assert_eq!(p, PathBuf::from("/mnt/my dir"));
}

/// dev 分解与 mountinfo 第 3 列同构（真实根路径上验证）。
#[test]
#[cfg(target_os = "linux")]
// 038-CI：依赖 bwrap//proc/mountinfo（Linux 专属），见 docs/specs/038 §5。
fn path_dev_matches_mountinfo_format() {
    let mounts = parse_mountinfo();
    let root = PathBuf::from("/");
    let dev = path_dev(&root).expect("根路径应有设备号");
    assert!(dev.contains(':'), "major:minor 形态：{dev}");
    // 根设备应出现在 mountinfo 中（/ 挂载必然存在）
    assert!(
        mounts.iter().any(|m| m.mount_point == PathBuf::from("/") && m.dev == dev),
        "根设备 {dev} 应与 mountinfo 一致"
    );
}

/// 受限读树布局：固定运行时白名单＋内容挂载＋逐实例 deny 遮蔽＋网络开关。
#[test]
fn permission_layout_runtime_and_carveouts() {
    let root = sbx_dir("layout");
    let data = root.join("data");
    let sub = data.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let secret_dir = data.join("private");
    fs::create_dir_all(&secret_dir).unwrap();
    let snapshot = PolicySnapshot {
        read_paths: vec![],
        write_paths: vec![data.clone()],
        deny_paths: vec![secret_dir.clone()],
        network: false,
        poison: None,
    };
    let mut args = Vec::new();
    create_permission_filesystem_args(&mut args, &snapshot, None).unwrap();
    let joined = args.join(" ");
    // 受限读树：tmpfs 根＋/usr ro＋usrmerge 符号链接＋/etc ro＋私有 /tmp
    assert!(joined.contains("--tmpfs /"));
    assert!(joined.contains("--ro-bind /usr /usr"));
    assert!(joined.contains("--symlink usr/bin /bin"));
    assert!(joined.contains("--ro-bind /etc /etc"));
    assert!(joined.contains("--tmpfs /tmp"));
    // 内容挂载（write）
    assert!(joined.contains(&format!("--bind {d} {d}", d = data.display())));
    // 该实例下的 deny 遮蔽（目录：perms 000 tmpfs remount-ro）
    assert!(
        joined.contains(&format!("--tmpfs {s}", s = secret_dir.display())),
        "deny 目录应在该挂载实例后遮蔽：{joined}"
    );
    // 网络关闭在 flags 层（此处只验证布局函数不含网络项）
    fs::remove_dir_all(&root).ok();
}

/// The 031 MCP gate launches setsid first, but still mounts the original server binary.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn mcp_gate_permission_launch_mounts_original_server_program() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tmp_ws("mcp-gate-program");
    let server = dir.join("mcp-server");
    fs::write(&server, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
    let server = server.canonicalize().unwrap();
    let server_str = server.to_string_lossy().into_owned();
    let (program, args) = mcp_stdio_gate_wrap(&server_str, &[]);
    let command = std::iter::once(program).chain(args).collect();
    let snapshot = PolicySnapshot {
        read_paths: vec![],
        write_paths: vec![],
        deny_paths: vec![],
        network: false,
        poison: None,
    };
    let launch = permission_launch_argv_with_payload_program(
        command,
        &snapshot,
        &dir,
        Some(&server_str),
    )
    .expect("031 MCP gate launch arguments");
    assert!(launch.args.windows(3).any(|window| {
        window[0] == "--ro-bind" && window[1] == server_str && window[2] == server_str
    }), "原 MCP 可执行文件必须保留单文件只读挂载：{:?}", launch.args);
    fs::remove_dir_all(dir).unwrap();
}

/// 毒丸快照：启动入口显式拒绝（不产出 argv）。
#[test]
fn poisoned_snapshot_refuses_launch() {
    let snap = PolicySnapshot::poisoned("矛盾".into());
    assert!(snap.valid().is_none());
    let err = permission_launch_argv(vec!["/bin/true".into()], &snap, Path::new("/"))
        .unwrap_err();
    assert!(err.to_string().contains("无法编码"));
}

/// seccomp 过滤器形状：8 字节整除、RET 收尾、长度 > 最小门（M0 教训）。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn seccomp_filter_shape_valid() {
    let filter = build_seccomp_filter();
    assert_eq!(filter.len() % 8, 0, "bwrap 按 8 字节整除校验");
    // 末条必须 RET（BPF_RET|BPF_K=0x06；ALLOW=0x7fff0000）
    let n = filter.len();
    let last_code = u16::from_le_bytes(filter[n - 8..n - 6].try_into().unwrap());
    let last_k = u32::from_le_bytes(filter[n - 4..].try_into().unwrap());
    assert_eq!(last_code, 0x06);
    assert_eq!(last_k, 0x7FFF_0000);
    // 门：第二条（arch JEQ）后跟 KILL_PROCESS
    let kill_k = u32::from_le_bytes(filter[16 + 4..16 + 8].try_into().unwrap());
    assert_eq!(kill_k, 0x8000_0000);
}

/// memfd 构造：可读回且偏移归零（bwrap 从当前偏移读取）。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn seccomp_fd_roundtrip() {
    let guard = build_seccomp_fd().unwrap().expect("x86_64 应有过滤器");
    let fd = guard.raw();
    let filter = build_seccomp_filter();
    let mut buf = vec![0u8; filter.len()];
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    assert_eq!(n as usize, filter.len(), "应从偏移 0 读到完整过滤器");
    assert_eq!(buf, filter);
}

/// 外层先返回 EINVAL，避免宿主的 ENOSYS 掩盖产品过滤器缺失。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn seccomp_filter_blocks_clone3_and_preserves_openat2() {
    const CHILD: &str = "TCHAT_SECCOMP_CLONE3_TEST_CHILD";
    // 子进程全部断言通过后打印的最终标记；父裁决与子打印共用此常量，防单点失配。
    // 039 扩展：变更条目 errno 实测——328 移除后 pwritev2 不再 EPERM；428 实拒
    // open_tree（注释纠偏的实体验证）；442 加入后 mount_setattr 实拒。
    const FILTER_MARKER: &str = "产品过滤器：clone3=ENOSYS openat2=ENOSYS pwritev2!=EPERM open_tree=EPERM mount_setattr=EPERM fspick=EPERM";
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sandbox::tests::seccomp_filter_blocks_clone3_and_preserves_openat2", "--nocapture"])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() { break; }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("seccomp 受控子进程超时");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let out = child.wait_with_output().unwrap();
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "受控子进程失败：\n{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        // 守卫父裁决：libtest 对零匹配的 --exact 仍以退出码 0 结束，只查退出码会把
        // "子进程什么都没跑"误判为通过；最终标记位于子进程两条核心断言之后，
        // 存在即证明断言已真实执行。
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(FILTER_MARKER),
            "子进程未执行到目标测试的最终断言（--exact 名称可能失配：libtest 零匹配仍退出 0）"
        );
        return;
    }

    // 仅在子进程中叠加测试过滤器：对两个目标系统调用返回 EINVAL。
    let outer = [
        libc::sock_filter { code: 0x20, jt: 0, jf: 0, k: 0 },
        libc::sock_filter { code: 0x15, jt: 1, jf: 0, k: libc::SYS_clone3 as u32 },
        libc::sock_filter { code: 0x15, jt: 0, jf: 1, k: libc::SYS_openat2 as u32 },
        libc::sock_filter { code: 0x06, jt: 0, jf: 0, k: 0x0005_0000 | libc::EINVAL as u32 },
        libc::sock_filter { code: 0x06, jt: 0, jf: 0, k: 0x7fff_0000 },
    ];
    let install = |filters: &[libc::sock_filter]| {
        let program = libc::sock_fprog { len: filters.len() as u16, filter: filters.as_ptr() as *mut _ };
        let result = unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) };
        assert_eq!(result, 0, "安装 seccomp 失败：{}", std::io::Error::last_os_error());
    };
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) }, 0);
    install(&outer);
    let probe = |nr| {
        let result = unsafe { libc::syscall(nr, 0, 0, 0, 0, 0, 0) };
        assert_eq!(result, -1, "探针只使用无效参数");
        std::io::Error::last_os_error().raw_os_error().unwrap()
    };
    assert_eq!(probe(libc::SYS_clone3), libc::EINVAL, "外层 clone3 对照未生效");
    assert_eq!(probe(libc::SYS_openat2), libc::EINVAL, "外层 openat2 对照未生效");
    eprintln!("外层对照：clone3=EINVAL openat2=EINVAL");

    // 从产品 memfd 读取实际过滤器，按内核 sock_filter 格式装载。
    let guard = build_seccomp_fd().unwrap().unwrap();
    let mut bytes = vec![0u8; build_seccomp_filter().len()];
    let n = unsafe { libc::read(guard.raw(), bytes.as_mut_ptr().cast(), bytes.len()) };
    assert_eq!(n, bytes.len() as isize);
    assert_eq!(bytes.len() % 8, 0);
    let filters: Vec<libc::sock_filter> = bytes.chunks_exact(8).map(|chunk| libc::sock_filter {
        code: u16::from_le_bytes([chunk[0], chunk[1]]), jt: chunk[2], jf: chunk[3],
        k: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
    }).collect();
    install(&filters);
    assert_eq!(probe(libc::SYS_openat2), libc::ENOSYS, "437 的既有行为不得改变");
    assert_eq!(probe(libc::SYS_clone3), libc::ENOSYS, "产品过滤器应拒绝 clone3");
    // 039 变更条目 errno 实测：探针参数全零只产生内核侧错误（EBADF/EFAULT/
    // EINVAL 或 count=0 的成功），不会产生真实副作用；EPERM 只能来自过滤器。
    let probe_any = |nr| {
        let result = unsafe { libc::syscall(nr, 0, 0, 0, 0, 0, 0) };
        if result == -1 {
            std::io::Error::last_os_error().raw_os_error().unwrap()
        } else {
            0
        }
    };
    assert_ne!(
        probe_any(libc::SYS_pwritev2),
        libc::EPERM,
        "328 移除后 pwritev2 不得再被拒（误拒普通写路径）"
    );
    assert_eq!(
        probe(libc::SYS_open_tree),
        libc::EPERM,
        "428 应实拒 open_tree（原 328 注释意图由 428 承担）"
    );
    assert_eq!(
        probe(libc::SYS_mount_setattr),
        libc::EPERM,
        "442 加入后 mount_setattr 应实拒（mount 族补齐）"
    );
    assert_eq!(
        probe(libc::SYS_fspick),
        libc::EPERM,
        "433 加入后 fspick 应实拒（mount 族补齐，用户裁决 2026-11-09）"
    );
    eprintln!("{FILTER_MARKER}");
}





/// 运行时挂载面（/usr、/etc ro-bind）同样是遮蔽实例：deny 别名落在其下的
/// 必须在 ro-bind 之后 carve（M1 变异首跑未检出的覆盖缺口——deny 优先对
/// 全部挂载面成立）。
#[test]
fn runtime_mounts_also_carve_denies() {
    let root = sbx_dir("rt-carve");
    let snapshot = PolicySnapshot {
        read_paths: vec![],
        write_paths: vec![root.clone()],
        deny_paths: vec![PathBuf::from("/etc/hosts")],
        network: false,
        poison: None,
    };
    let mut args = Vec::new();
    create_permission_filesystem_args(&mut args, &snapshot, None).unwrap();
    let joined = args.join(" ");
    let etc_idx = joined.find("--ro-bind /etc /etc").expect("/etc ro-bind 应存在");
    let carve_idx = joined
        .find("--ro-bind /dev/null /etc/hosts")
        .expect("/etc 下的 deny 文件应被 carve");
    assert!(
        carve_idx > etc_idx,
        "运行时 deny 遮蔽必须在 /usr、/etc ro-bind 之后：{joined}"
    );
    fs::remove_dir_all(&root).ok();
}

/// 审查 #3 回归：带可选字段（master:N/shared:N）的 mountinfo 行不得被丢弃
///（proc(5)：可选字段零或多——固定下标会把带传播标记的 bind 挂载整行漏掉）。
#[test]
fn mountinfo_optional_fields_are_parsed() {
    // 真实容器形态（/cache 卷，ext4 8:48，master:30）——列序：id parent dev
    // root mount_point options [optional...] - fstype source super_options
    let text = "453 452 8:48 /data/docker/volumes/x/_data /cache rw,relatime master:30 - ext4 /dev/sdd rw";
    let mounts = parse_mountinfo_from(text);
    assert_eq!(mounts.len(), 1, "带 master:30 的行必须被解析");
    assert_eq!(mounts[0].mount_point, PathBuf::from("/cache"));
    assert_eq!(mounts[0].dev, "8:48");
    assert_eq!(mounts[0].root, PathBuf::from("/data/docker/volumes/x/_data"));
    // 多个可选字段 + 无可选字段两种形态（双行文本）
    let text2 = "1 0 0:1 / / rw shared:1 master:2 - overlay none rw\n3 2 0:2 / /other ro - tmpfs none ro";
    let m2 = parse_mountinfo_from(text2);
    assert_eq!(m2.len(), 2, "shared+master 与无可选字段行都应解析");
    assert_eq!(m2[1].mount_point, PathBuf::from("/other"));
}

/// 审查 #3 回归：同设备双挂载的别名互相显形（跨挂载投影，非 identity）。
/// 场景（deny 路径真实存在——deny_aliases 经 path_dev 取设备号）：文件系统
/// 内 <T>/a/x 同时经两条挂载可见——挂载一（root=<T>/a，point=<T>/a，即原
/// 位）与挂载二（root=<T> 整盘 bind 到 <T>/m2）；deny <T>/a/x 的别名必须
/// 同时显形 <T>/a/x 与 <T>/m2/a/x，遮蔽须覆盖两者。
#[test]
#[cfg(target_os = "linux")] // 038：mountinfo/bwrap/WSL 等 Linux 设施测试
fn deny_aliases_cross_mount_projection() {
    let t = sbx_dir("cross-mount");
    let a = t.join("a");
    fs::create_dir_all(&a).unwrap();
    fs::write(a.join("x"), "k").unwrap();
    let m2 = t.join("m2");
    let dev = path_dev(&a.join("x")).expect("真实文件应有设备号");
    let mounts = vec![
        MountInfo { root: a.clone(), mount_point: a.clone(), dev: dev.clone() },
        MountInfo { root: t.clone(), mount_point: m2.clone(), dev },
    ];
    let aliases = deny_aliases(&a.join("x"), &mounts);
    assert!(aliases.contains(&a.join("x")), "identity 别名：{aliases:?}");
    assert!(
        aliases.contains(&m2.join("a").join("x")),
        "跨挂载别名必须显形（bind 到 m2 的同一文件）：{aliases:?}"
    );
    fs::remove_dir_all(&t).ok();
}

#[test]
#[cfg(target_os = "linux")]
fn leader_pidfd_exit_check_preserves_wait_owner() {
    let mut child = std::process::Command::new("true")
        .spawn()
        .expect("启动短命监督进程");
    let pidfd = open_pidfd(child.id() as libc::pid_t).expect("固定监督进程 PIDFD");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !pidfd_is_exited(&pidfd).expect("轮询监督进程 PIDFD") {
        assert!(std::time::Instant::now() < deadline, "监督进程未在 2 秒内退出");
        let mut pfd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&pidfd),
            events: libc::POLLIN,
            revents: 0,
        };
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let timeout = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let result = unsafe { libc::poll(&mut pfd, 1, timeout) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            panic!("等待 PIDFD 退出事件失败：{error}");
        }
    }

    assert!(child.try_wait().expect("原 Child 仍可 wait").is_some());
    assert!(pidfd_is_exited(&pidfd).expect("再次查询原进程退出状态"));
}



/// 039 回归锚：拒绝清单条目 ↔ libc `SYS_*` 符号逐条对照。编号漂移（clone3
/// 曾被误标为 437 的同类错误）在此必然失败：清单或对照表任一侧改动都必须
/// 有意识地同步另一侧，语义（符号名）与编号保持三方一致（注释＝实际拒绝＝
/// 官方名）。附升序断言，保持清单可读性与新增条目的插入纪律。
#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn seccomp_deny_lists_match_libc_symbols() {
    let eperm: &[(u32, &str)] = &[
        (libc::SYS_ptrace as u32, "ptrace"),
        (libc::SYS_pivot_root as u32, "pivot_root"),
        (libc::SYS_mount as u32, "mount"),
        (libc::SYS_umount2 as u32, "umount2"),
        (libc::SYS_swapon as u32, "swapon"),
        (libc::SYS_swapoff as u32, "swapoff"),
        (libc::SYS_reboot as u32, "reboot"),
        (libc::SYS_init_module as u32, "init_module"),
        (libc::SYS_delete_module as u32, "delete_module"),
        (libc::SYS_kexec_load as u32, "kexec_load"),
        (libc::SYS_unshare as u32, "unshare"),
        (libc::SYS_fanotify_init as u32, "fanotify_init"),
        (libc::SYS_name_to_handle_at as u32, "name_to_handle_at"),
        (libc::SYS_open_by_handle_at as u32, "open_by_handle_at"),
        (libc::SYS_setns as u32, "setns"),
        (libc::SYS_process_vm_readv as u32, "process_vm_readv"),
        (libc::SYS_process_vm_writev as u32, "process_vm_writev"),
        (libc::SYS_finit_module as u32, "finit_module"),
        (libc::SYS_kexec_file_load as u32, "kexec_file_load"),
        (libc::SYS_bpf as u32, "bpf"),
        (libc::SYS_userfaultfd as u32, "userfaultfd"),
        (libc::SYS_open_tree as u32, "open_tree"),
        (libc::SYS_move_mount as u32, "move_mount"),
        (libc::SYS_fsopen as u32, "fsopen"),
        (libc::SYS_fsconfig as u32, "fsconfig"),
        (libc::SYS_fsmount as u32, "fsmount"),
        (libc::SYS_fspick as u32, "fspick"),
        (libc::SYS_mount_setattr as u32, "mount_setattr"),
    ];
    let enosys: &[(u32, &str)] = &[
        (libc::SYS_clone3 as u32, "clone3"),
        (libc::SYS_openat2 as u32, "openat2"),
    ];
    for (list, table, label) in [
        (SECCOMP_DENY_EPERM, eperm, "EPERM"),
        (SECCOMP_DENY_ENOSYS, enosys, "ENOSYS"),
    ] {
        assert_eq!(
            list.len(),
            table.len(),
            "{label} 清单条目数与对照表不一致：清单={list:?}"
        );
        for (nr, name) in table {
            assert!(
                list.contains(nr),
                "{label} 清单缺少 {name}（SYS_{name}={nr}）——编号漂移或漏登记"
            );
        }
        for (i, nr) in list.iter().enumerate() {
            assert!(
                table.iter().any(|(sym, _)| sym == nr),
                "{label} 清单第 {i} 项 {nr} 无符号对照——更新对照表或修正编号"
            );
            assert!(
                i == 0 || list[i - 1] < *nr,
                "{label} 清单第 {i} 项 {nr} 破坏升序"
            );
        }
    }

/// 040 负向签名保护：收割只作用于匹配签名（PPid=本进程＋comm=bwrap＋
/// NSpid 末级=1）的收养 reaper——非 bwrap 的收养子进程（此处＝普通 sh 僵尸）
/// 必须原样保留，由其父进程（本测试）正常 wait 回收。误收＝偷走其他组件的
/// wait 责任（016 会话/JS worker 依赖各自收割），属所有权破坏。
#[test]
#[cfg(target_os = "linux")]
fn reap_adopted_bwrap_reapers_leaves_non_bwrap_children_alone() {
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("true")
        .spawn()
        .expect("spawn 非 bwrap 子进程");
    // 轮询确认子进程已真成 Z 态（不调 waitpid——waitpid 即收割，会掩盖后续
    // 敏感性）：此时它满足「PPid=本进程＋Z 态」的收养语义，仅 comm/NSpid
    // 与 bwrap reaper 不同。收割若签名失守即在此误收。
    let pid = child.id() as i32;
    let mut zombie_confirmed = false;
    for _ in 0..500 {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            if let Some((_, rest)) = stat.rsplit_once(") ") {
                if rest.split_whitespace().next() == Some("Z") {
                    zombie_confirmed = true;
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(zombie_confirmed, "子进程未如期进入 Z 态（环境异常）");
    crate::sandbox::reap_adopted_bwrap_reapers();
    // 收割后仍能由本测试亲自回收＝收割未触碰它（若被误收，此处 ECHILD）。
    let mut status = 0;
    let rc = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    assert_eq!(
        rc,
        child.id() as i32,
        "收割误收了非 bwrap 子进程（wait 责任被偷走）"
    );
    let _ = child.wait();
}
