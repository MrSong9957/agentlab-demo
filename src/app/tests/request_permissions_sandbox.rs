//! 031 权限申请沙箱集成测试（真实 bwrap，M2 执行面）。
//!
//! 与 exec_sandbox 同源约定：**独立测试文件＝独立测试进程**；本文件直接经
//! `permission_launch_argv` 构造真实 bwrap 启动（不经进程级 TCHAT_SANDBOX 开
//! 关——快照显式传入），验证：受限读树正/负例、deny 优先（含目录遮蔽）、
//! 网络开关、启动前 deny 边界检查的 fail-closed（工具零执行）、PID 命名空
//! 间隔离真实生效。全程假数据，不触真实凭据。
#![cfg(unix)]
// 038-CI：本文件依赖 POSIX 语义（fd 继承/pre_exec），Windows 不编译；沙箱用例自带可用性分支（无 bwrap＝显式失败断言），macOS 照常运行。

use std::fs;
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;
use terminal_chat::exec::ExecArgs;
use terminal_chat::permissions::{self, Access, DenyContext};
use terminal_chat::proc_session::ProcessManager;
use terminal_chat::sandbox::{self, PolicySnapshot};

/// 038-CI：031 性能冻结阈值（P10/P16）为校准机器口径（同 032/034 冻结约定
/// ——机器或构建变化须重新冻结）。CI 托管机器临时且不可比，置
/// `TCHAT_P31_PERF_SAMPLE=1` 按同款采样约定跳过冻结断言（场景照跑、样本照
/// 输出）；本地默认断言，行为零变化。
fn perf_sample_mode() -> bool {
    std::env::var("TCHAT_P31_PERF_SAMPLE").is_ok()
}

fn root_dir(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "tchat-031-it-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn deny_ctx_for(anchor: &Path) -> DenyContext {
    DenyContext {
        anchor: anchor.to_path_buf(),
        config_dir: anchor.join("fake-config"),
        user_dir: anchor.join("fake-user"),
        install_dirs: vec![anchor.join("fake-install")],
    }
}

/// 夹具：anchor（基线 write）＋可申请目录＋敏感假文件。
struct Fx {
    root: PathBuf,
    anchor: PathBuf,
    requestable: PathBuf,
    outside: PathBuf,
}

fn fx() -> Fx {
    let root = root_dir("fixtures");
    let anchor = root.join("workspace");
    let requestable = root.join("export");
    let outside = root.join("outside");
    for d in [&anchor, &requestable, &outside] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(anchor.join(".env"), "FAKE-031-ENV=1\n").unwrap();
    fs::write(outside.join("host-secret.txt"), "FAKE-HOST-SECRET\n").unwrap();
    Fx { root, anchor, requestable, outside }
}

fn policy_json(fx: &Fx, net: bool) -> String {
    let anchor = fx.anchor.canonicalize().unwrap();
    let requestable = fx.requestable.canonicalize().unwrap();
    let entry = |p: &Path| {
        format!(
            r#"{{"path": {{"type": "path", "path": {:?}}}, "access": "write"}}"#,
            p
        )
    };
    format!(
        r#"{{"version": 1, "base": {{"file_system": {{"entries": [{}]}}, "network": {{"enabled": false}}}}, "requestable": {{"file_system": {{"entries": [{}]}}, "network": {{"enabled": {net}}}}}, "deny_paths": []}}"#,
        entry(&anchor),
        entry(&requestable),
    )
}

fn snapshot_for(fx: &Fx, grants_paths: &[(&Path, Access)], network: bool) -> PolicySnapshot {
    let policy = permissions::Policy::parse(&policy_json(fx, network)).unwrap();
    let mut grants = permissions::TurnGrants::new();
    for (p, a) in grants_paths {
        grants
            .grant_all(&permissions::NormalizedRequest {
                reason: None,
                entries: vec![permissions::NormalizedEntry {
                    path: p.canonicalize().unwrap(),
                    access: *a,
                }],
                network: false,
            })
            .unwrap();
    }
    grants.network = network;
    permissions::build_snapshot(&policy, &grants, &deny_ctx_for(&fx.anchor)).unwrap()
}

/// 以快照直接运行一条载荷命令（真实 bwrap；等待完成）。cwd＝anchor（基线
/// write 面——受限树内合法落点；未挂载路径会正确 fail-closed）。
fn run_payload(snapshot: &PolicySnapshot, script: &str, cwd: &Path) -> std::io::Result<std::process::Output> {
    let launch = sandbox::permission_launch_argv(
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            script.to_string(),
        ],
        snapshot,
        cwd,
    )?;
    let mut c = Command::new(&launch.program);
    c.args(&launch.args);
    c.env_clear();
    c.envs(launch.env.iter().cloned());
    let out = c.output()?;
    drop(launch); // seccomp fd 守卫在 spawn 完成后即可释放
    Ok(out)
}

fn bwrap_available() -> bool {
    sandbox::probe_available()
}

/// P06：受限读树——批准面可用、未批面 ENOENT、deny 正文不可得、敏感 .env 遮蔽。
#[test]
fn restricted_tree_read_write_and_deny_real() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let secret_in_requestable = f.requestable.join("private");
    fs::create_dir_all(&secret_in_requestable).unwrap();
    fs::write(secret_in_requestable.join("s.txt"), "FAKE-REQ-SECRET\n").unwrap();
    fs::write(f.anchor.join("ok.txt"), "OK-DATA\n").unwrap();

    let snap = snapshot_for(&f, &[(&f.requestable.clone(), Access::Read)], false);
    // anchor 基线 write＋requestable read 授权
    let out = run_payload(
        &snap,
        &format!(
            "cat {ok} && test -w {anchor} && echo WRITE-OK; \
             cat {outside} 2>&1; \
             cat {envf} 2>&1; \
             cat {sreq} 2>&1",
            ok = f.anchor.join("ok.txt").display(),
            anchor = f.anchor.display(),
            outside = f.outside.join("host-secret.txt").display(),
            envf = f.anchor.join(".env").display(),
            sreq = secret_in_requestable.join("s.txt").display(),
        ),
        &f.anchor.clone(),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "载荷应成功：{:?}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("OK-DATA"), "批准面可读：{text}");
    assert!(text.contains("WRITE-OK"), "基线 write 生效：{text}");
    // 未批树不可见（ENOENT——受限读树不挂载该子树）
    assert!(
        text.contains("No such file") || text.contains("不存在"),
        "未批准路径应 ENOENT：{text}"
    );
    // deny（自动敏感边界 .env）：读取被拒（遮蔽后 open EACCES 或读到空——
    // /dev/null bind 表现为打开失败；033 语义）
    assert!(!text.contains("FAKE-031-ENV"), ".env 正文不可得：{text}");
    // 显式授权 read 的 requestable 目录：内容可读
    // 审查 #8：read 授权面正例——真实断言（原 `|| true` 恒真，形同虚设）
    assert!(text.contains("FAKE-REQ-SECRET"), "read 授权面内容可读：{text}");
    fs::remove_dir_all(&f.root).ok();
}

/// P05：网络开关——未启用时回环外不可达；启用后本地服务可达。
#[test]
fn network_switch_real() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    // 本地回环服务（假数据）
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = std::thread::spawn(move || {
        if let Ok((c, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut c = c;
            let mut buf = [0u8; 64];
            let _ = c.read(&mut buf);
            let _ = c.write_all(b"FAKE-NET-REPLY");
        }
    });
    // 网络关闭：连接被拒（unshare-net 内无路由）
    let snap_off = snapshot_for(&f, &[], false);
    let out = run_payload(
        &snap_off,
        &format!(
            "bash -c 'exec 3<>/dev/tcp/127.0.0.1/{p}' && echo NET-ON || echo NET-OFF",
            p = addr.port()
        ),
        &f.anchor.clone(),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("NET-OFF"), "网络关闭应不可达：{text}");
    drop(snap_off);
    // 网络启用（requestable network=true）：可达
    let snap_on = snapshot_for(&f, &[], true);
    let out = run_payload(
        &snap_on,
        &format!(
            "bash -c 'exec 3<>/dev/tcp/127.0.0.1/{p}' && echo NET-ON || echo NET-OFF",
            p = addr.port()
        ),
        &f.anchor.clone(),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("NET-ON"), "网络启用应可达：{text}");
    drop(snap_on);
    let _ = srv.join();
    fs::remove_dir_all(&f.root).ok();
}

/// P06(b)：deny 目录内部文件的预存硬链接（反例变体 A）——启动前检查拒绝，
/// 工具零执行（载荷标记不产生）。
#[cfg(unix)]
#[test]
fn deny_dir_internal_hardlink_zero_execution_real() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let private = f.requestable.join("private");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("secret.txt"), "FAKE-P06-SECRET").unwrap();
    // 预存硬链接放进获批 read 树
    fs::hard_link(private.join("secret.txt"), f.requestable.join("alias.txt")).unwrap();

    let mut snap = snapshot_for(&f, &[(&f.requestable.clone(), Access::Read)], false);
    // deny 集合显式加入该目录（策略层路径），验证启动拒绝
    snap.deny_paths.push(private.canonicalize().unwrap());
    let marker = f.root.join("TOOL-RAN");
    let err = run_payload(
        &snap,
        &format!("echo ran > {}", marker.display()),
        &f.anchor.clone(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("硬链接"),
        "启动前检查应拒绝（fail-closed）：{err}"
    );
    assert!(!marker.exists(), "工具零执行");
    // 去掉别名后（nlink 回 1）：可启动且别名路径遮蔽后不可读
    fs::remove_file(f.requestable.join("alias.txt")).unwrap();
    let snap2 = snapshot_for(&f, &[(&f.requestable.clone(), Access::Read)], false);
    let mut snap2 = snap2;
    snap2.deny_paths.push(private.canonicalize().unwrap());
    let out = run_payload(
        &snap2,
        &format!(
            "cat {p} 2>&1; ls {r}",
            p = private.join("secret.txt").display(),
            r = f.requestable.display(),
        ),
        &f.anchor.clone(),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("FAKE-P06-SECRET"), "deny 正文不可得：{text}");
    fs::remove_dir_all(&f.root).ok();
}

/// P09：旧 shell 不升级——write_stdin 对旧进程发送命令不能获得后来批准的
/// 权限（快照出生时冻结）。
#[test]
fn old_shell_does_not_gain_later_permissions() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    // 基线快照（无 requestable 授权）启动长命 shell
    let base_snap = snapshot_for(&f, &[], false);
    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(false);
    let args = ExecArgs::from_value(&json!({
        "cmd": format!("sleep 30"),
        "yield_time_ms": 250,
        "workdir": f.anchor.display().to_string(),
    }))
    .unwrap();
    let base_snap = std::sync::Arc::new(base_snap);
    let out = mgr.start_with_policy(&args, Some(&cancel), Some(&base_snap), false);
    let sid = out.result.session_id.clone().expect("应返回句柄");
    // 之后（模拟同轮稍后的授权）：经 write_stdin 向旧 shell 发读取命令——
    // 旧进程不升级（其出生快照无该挂载面；write_stdin 不扩大出生权限）
    fs::write(f.requestable.join("later.txt"), "LATER-DATA\n").unwrap();
    let r = mgr
        .interact(
            &terminal_chat::exec::WriteStdinArgs::from_value(&json!({
                "session_id": sid,
                "chars": format!("cat {}\n", f.requestable.join("later.txt").display()),
                "yield_time_ms": 1500,
            }))
            .unwrap(),
            None,
        )
        .result;
    let output = r.output.clone();
    assert!(
        !output.contains("LATER-DATA"),
        "旧 shell 不升级权限（出生快照冻结）：{output}"
    );
    mgr.close();
    fs::remove_dir_all(&f.root).ok();
}

/// PID 命名空间隔离真实生效（P13）：沙箱内看不到宿主进程；/proc 是新实例。
#[test]
fn pid_namespace_isolated_real() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let snap = snapshot_for(&f, &[], false);
    let host_marker = f.root.join("host-proc-visible");
    let my_pid = std::process::id();
    let out = run_payload(
        &snap,
        &format!("test -d /proc/{my_pid} && touch {m}; ls /proc | wc -l", m = host_marker.display()),
        &f.anchor.clone(),
    )
    .unwrap();
    assert!(out.status.success());
    assert!(
        !host_marker.exists(),
        "宿主进程不应出现在沙箱 /proc"
    );
    fs::remove_dir_all(&f.root).ok();
}

/// 运行期新建硬链接不可达（M0 H4 语义在 031 布局下保持）。
#[cfg(unix)]
#[test]
fn runtime_hardlink_to_denied_unreachable() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let private = f.requestable.join("private");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("s.txt"), "FAKE-RT\n").unwrap();
    let mut snap = snapshot_for(&f, &[(&f.requestable.clone(), Access::Write)], false);
    snap.deny_paths.push(private.canonicalize().unwrap());
    let out = run_payload(
        &snap,
        &format!(
            "ln {p} {r}/new-alias 2>&1 && echo LINK-OK || echo LINK-DENIED",
            p = private.join("s.txt").display(),
            r = f.requestable.display(),
        ),
        &f.anchor.clone(),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("LINK-DENIED") || text.contains("No such file") || text.contains("Permission denied"),
        "运行期对 deny 目标建链应失败：{text}"
    );
    fs::remove_dir_all(&f.root).ok();
}

/// 基线进程照常可续接（016 语义不因 031 收尾门被误伤）＋额外权限进程到期终止。
#[test]
fn extra_permission_processes_reaped_baseline_kept() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(false);
    // 基线长命进程（extra=false）
    let base = std::sync::Arc::new(snapshot_for(&f, &[], false));
    let baseline_out = mgr.start_with_policy(
        &ExecArgs::from_value(&json!({
            "cmd": "sleep 30", "yield_time_ms": 250,
            "workdir": f.anchor.display().to_string()
        })).unwrap(),
        Some(&cancel),
        Some(&base),
        false,
    );
    let baseline_id = baseline_out.result.session_id.clone().unwrap();
    // 额外权限长命进程（extra=true）
    let granted = std::sync::Arc::new(snapshot_for(&f, &[(&f.requestable.clone(), Access::Read)], true));
    let extra_out = mgr.start_with_policy(
        &ExecArgs::from_value(&json!({
            "cmd": "sleep 30", "yield_time_ms": 250,
            "workdir": f.anchor.display().to_string()
        })).unwrap(),
        Some(&cancel),
        Some(&granted),
        true,
    );
    let extra_id = extra_out.result.session_id.clone().unwrap();
    // 收尾门：额外权限进程被终止，基线保留运行
    mgr.reap_extra_permission_sessions().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        mgr.session_running(&baseline_id),
        Some(true),
        "基线进程应继续运行（016 可续接）"
    );
    assert_eq!(
        mgr.session_running(&extra_id),
        Some(false),
        "额外权限进程应已终止（§1.7-5）"
    );
    mgr.close();
    fs::remove_dir_all(&f.root).ok();
}

/// 快照含毒丸：start_with_policy 显式失败、零命令执行。
#[test]
fn poisoned_snapshot_zero_execution() {
    let f = fx();
    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(false);
    let poison = std::sync::Arc::new(PolicySnapshot::poisoned("策略矛盾".into()));
    let marker = f.root.join("RAN");
    let out = mgr.start_with_policy(
        &ExecArgs::from_value(&json!({
            "cmd": format!("echo x > {}", marker.display()),
            "yield_time_ms": 500,
        }))
        .unwrap(),
        Some(&cancel),
        Some(&poison),
        true,
    );
    assert_eq!(out.result.status, terminal_chat::exec::ExecStatus::Error);
    assert!(
        out.result.error.as_deref().unwrap_or("").contains("无法编码"),
        "毒丸错误应可见：{:?}",
        out.result.error
    );
    assert!(!marker.exists(), "毒丸快照零命令执行");
    mgr.close();
    fs::remove_dir_all(&f.root).ok();
}

/// exec_command 完整链路（request→grant→执行→到期）经 ProcessManager：
/// 先拒绝（未批树不可见），授权后同命令成功。
#[test]
fn request_grant_then_exec_full_chain() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    fs::write(f.requestable.join("data.txt"), "GRANTED-DATA\n").unwrap();
    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(false);
    let cmd = format!("cat {}", f.requestable.join("data.txt").display());
    // 基线：requestable 未挂载 → cat 失败（无输出）
    let base = std::sync::Arc::new(snapshot_for(&f, &[], false));
    let r0 = mgr.start_with_policy(
        &ExecArgs::from_value(&json!({
            "cmd": cmd, "yield_time_ms": 2000,
            "workdir": f.anchor.display().to_string()
        })).unwrap(),
        Some(&cancel),
        Some(&base),
        false,
    ).result;
    assert!(!r0.output.contains("GRANTED-DATA"), "未授权时不可读：{}", r0.output);
    // 授权 read 后：同命令成功
    let granted = std::sync::Arc::new(snapshot_for(&f, &[(&f.requestable.clone(), Access::Read)], false));
    let r1 = mgr.start_with_policy(
        &ExecArgs::from_value(&json!({
            "cmd": cmd, "yield_time_ms": 2000,
            "workdir": f.anchor.display().to_string()
        })).unwrap(),
        Some(&cancel),
        Some(&granted),
        true,
    ).result;
    assert!(
        r1.output.contains("GRANTED-DATA"),
        "授权后应可读：{}（err={:?}）",
        r1.output,
        r1.error
    );
    mgr.close();
    fs::remove_dir_all(&f.root).ok();
}

/// P10：view_image 经 031 沙箱内部入口——批准面图片可读，未批面拒绝且主进程
/// 不代读（内部入口失败＝显式错误，不回退进程内读取）。
#[test]
fn view_image_sandboxed_read_boundary() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    // 最小合法 PNG（1×1，image crate 生成——手写 CRC 曾致解码失败）
    let mut png_buf = Vec::new();
    {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([12u8, 34, 56]));
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png_buf), image::ImageFormat::Png)
            .unwrap();
    }
    fs::write(f.requestable.join("img.png"), &png_buf).unwrap();
    // 基线（requestable 未授权）：读取被拒（文件在未挂载面——内部入口无法打开）
    let base = snapshot_for(&f, &[], false);
    let args = format!("{{\"path\": {:?}}}", f.requestable.join("img.png"));
    let o1 = terminal_chat::view_image::execute_sandboxed_with_exe(
        &args, &f.anchor, None, &base, Path::new(env!("CARGO_BIN_EXE_terminal-chat")),
    );
    assert!(o1.image.is_none(), "未授权读取应失败：{}", o1.summary);
    // 授权 read：图片可读（载荷存在，主进程未代读）
    let granted = snapshot_for(&f, &[(&f.requestable.clone(), Access::Read)], false);
    let o2 = terminal_chat::view_image::execute_sandboxed_with_exe(
        &args, &f.anchor, None, &granted, Path::new(env!("CARGO_BIN_EXE_terminal-chat")),
    );
    assert!(o2.image.is_some(), "授权后应可读：{}", o2.summary);
    assert!(o2.summary.contains("img.png"), "摘要含路径：{}", o2.summary);
    fs::remove_dir_all(&f.root).ok();
}

/// P16：启动前检查性能测量与预算冻结依据（§1.8：单并发、预热 5、100 次×
/// 3 轮取 p95 中位轮）。测两条路径：①生产形态 deny 集（.env＋config＋user
/// 根＋策略目录＋二进制/bwrap 文件）的 permission_launch_argv 端到端组装
/// （含递归 nlink 检查，不含 spawn）；②合成大 deny 树（1000 文件）的纯遍
/// 测速率。断言用冻结阈值×余量，作为性能回归门禁；样本同步落盘存证。
#[test]
fn p16_launch_check_perf_and_budget_freeze() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    // 生产形态 deny 集：config/user 根放真实规模夹具（数十项即可代表真实
    // 部署；合成大树单测速率上限）
    let cfg_dir = f.root.join("prod-config");
    let user_dir = f.root.join("prod-user");
    for d in [&cfg_dir, &user_dir] {
        fs::create_dir_all(d).unwrap();
        for i in 0..24 {
            fs::write(d.join(format!("f{i}.json")), "{}\n").unwrap();
        }
    }
    // 专用安装目录（拷贝 exe＋bwrap，nlink=1——对应真实 /opt 部署；cargo
    // deps 目录满是硬链接产物，不代表部署形态）
    let install = f.root.join("opt-install");
    fs::create_dir_all(&install).unwrap();
    fs::copy(std::env::current_exe().unwrap(), install.join("terminal-chat")).unwrap();
    if let Some(bwrap) = sandbox::launcher_path() {
        fs::copy(bwrap, install.join("bwrap")).unwrap();
    }
    let policy = permissions::Policy::parse(&policy_json(&f, false)).unwrap();
    let ctx = permissions::DenyContext {
        anchor: f.anchor.clone(),
        config_dir: cfg_dir.clone(),
        user_dir: user_dir.clone(),
        install_dirs: vec![install],
    };
    let snap = permissions::build_snapshot(&policy, &permissions::TurnGrants::new(), &ctx)
        .expect("生产形态 deny 集应可编码（安装目录在白名单内按文件 deny）");

    let mut rounds_p95: Vec<u128> = Vec::new();
    let mut all_samples: Vec<u128> = Vec::new();
    // 预热 5 次
    for _ in 0..5 {
        let _ = sandbox::permission_launch_argv(
            vec!["/bin/true".into()],
            &snap,
            &f.anchor,
        )
        .unwrap();
    }
    for _round in 0..3 {
        let mut samples: Vec<u128> = Vec::new();
        for _ in 0..100 {
            let t0 = std::time::Instant::now();
            let launch = sandbox::permission_launch_argv(
                vec!["/bin/true".into()],
                &snap,
                &f.anchor,
            )
            .unwrap();
            drop(launch);
            samples.push(t0.elapsed().as_micros());
        }
        samples.sort();
        let p95 = samples[samples.len() * 95 / 100];
        rounds_p95.push(p95);
        all_samples.extend(samples);
    }
    rounds_p95.sort();
    let median_p95_us = rounds_p95[1];

    // 合成大 deny 树：1000 文件的递归遍历速率
    let big = f.root.join("big-deny");
    fs::create_dir_all(&big).unwrap();
    for i in 0..1000 {
        fs::write(big.join(format!("e{i}")), "x").unwrap();
    }
    let mut rate_samples: Vec<u128> = Vec::new();
    for _ in 0..5 {
        let t0 = std::time::Instant::now();
        sandbox::verify_deny_boundaries(
            &[big.clone()],
            &sandbox::TraversalBudget { max_entries: 100_000, timeout_ms: 60_000 },
        )
        .unwrap();
        rate_samples.push(t0.elapsed().as_micros());
    }
    rate_samples.sort();
    let walk_1000_us = rate_samples[2]; // 中位

    // 落盘存证（追加式；真实样本随测随存）
    // 审查 #9：样本落测试临时目录（不得在测试内截断重写仓库冻结证据文件）；
    // 冻结依据以人工测量的归档样本为准（docs/logs/031-request-permissions/）。
    let log_path = std::env::temp_dir().join(format!(
        "tchat-031-p16-samples-{}.txt",
        std::process::id()
    ));
    let mut log = format!(
        "# P16 启动前检查性能样本（自动测试生成，环境：{} 内核 {}）\n",
        std::env::consts::OS,
        std::env::var("KERNEL_VER").unwrap_or_default()
    );
    log += &format!(
        "# 生产形态 permission_launch_argv（含递归 nlink 检查＋seccomp memfd 构造）p95 中位轮＝{median_p95_us}µs；三轮 p95＝{rounds_p95:?}\n"
    );
    log += &format!(
        "# 合成 1000 文件 deny 树递归遍历中位＝{walk_1000_us}µs（≈{per_entry_ns}ns/项）\n",
        per_entry_ns = walk_1000_us * 1000 / 1000,
    );
    log += &format!(
        "# 100 次样本（第 1 轮，µs）＝{:?}\n",
        &all_samples[..100.min(all_samples.len())]
    );
    let _ = fs::write(&log_path, log);

    // 冻结阈值（余量断言）：端到端组装 p95 中位轮 < 50ms（阈值＝观测值数十
    // 倍量级，容纳更大 deny 树）；1000 项遍历 < 100ms（对应 20000 项 ≈ 2s 内、
    // 2000ms 超时预算与 20000 条目预算在实测速率下有 ≥10× 余量——见证据记录
    // 的冻结说明）。038-CI：两项均为校准机器口径（同 032/034 冻结约定），
    // CI 置 TCHAT_P31_PERF_SAMPLE=1 按同款采样约定跳过断言；本地默认断言。
    if perf_sample_mode() {
        eprintln!("P10 采样：median_p95={median_p95_us}µs walk_1000={walk_1000_us}µs（跳过冻结断言）");
    } else {
        assert!(
            median_p95_us < 50_000,
            "启动检查 p95 中位轮 {median_p95_us}µs 超过 50ms 冻结上限"
        );
        assert!(
            walk_1000_us < 100_000,
            "1000 项遍历 {walk_1000_us}µs 超过 100ms 上限"
        );
    }
    fs::remove_dir_all(&f.root).ok();
}

/// 审查 #1 回归（§1.2-3）：031 冻结策略下**无权限宿主**（非交互/模型未声明
/// 能力）时基线约束同样生效——exec 的 deny 覆盖路径不可读，不回退 033 全盘。
/// 直接以基线快照（空授权）经 start_with_policy 验证，模拟 perm=None 路径。
#[test]
fn baseline_enforced_without_permission_host() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    fs::write(f.outside.join("host-secret.txt"), "FAKE-NOHOST-SECRET\n").unwrap();
    // 基线快照（空授权——与 turn.rs frozen() 无 perm 状态时构建的同构）
    let base = std::sync::Arc::new(snapshot_for(&f, &[], false));
    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(false);
    let r = mgr
        .start_with_policy(
            &ExecArgs::from_value(&json!({
                "cmd": format!("cat {}", f.outside.join("host-secret.txt").display()),
                "yield_time_ms": 4000,
                "workdir": f.anchor.display().to_string(),
            }))
            .unwrap(),
            Some(&cancel),
            Some(&base),
            false,
        )
        .result;
    assert!(
        !r.output.contains("FAKE-NOHOST-SECRET"),
        "无宿主时基线约束同样生效（不回退 033 全盘读）：{}",
        r.output
    );
    mgr.close();
    fs::remove_dir_all(&f.root).ok();
}

/// 审查 #2 回归（§1.7-7）：收尾失败门——reap 失败置位 extra_reap_gate，
/// 成功 reap 后清除；门存在的语义（宿主拒绝下一轮）在位。
/// 制造真实不可杀进程不现实：以公共 API 语义验证门的双向迁移（无 extra
/// 会话时 reap 成功清门；门查询返回最近失败原因）。失败注入路径由
/// reap_extra_permission_sessions 的 Err 分支单测（进程组 kill 失败注入）
/// 在 sandbox_matrix_probes 模式的 EPERM 注入先例下后续补齐。
#[test]
fn extra_reap_gate_latch_semantics() {
    let f = fx();
    let mgr = ProcessManager::new();
    // 无额外会话：reap 成功，门为 None
    mgr.reap_extra_permission_sessions().unwrap();
    assert!(mgr.extra_reap_gate().is_none(), "无失败时门应为空");
    mgr.close();
    fs::remove_dir_all(&f.root).ok();
}

/// 审查 #15 行为回归（真实 bwrap）：同一轮先授予 read 子目录、再授予 write
/// 父目录时，已批准的父目录写权限在子树内不得静默 EROFS（§1.4「write 包含
/// 读取」——read 授权不构成只读 carve-out，收窄原语只有 deny；§1.6 批准对象
/// 与实际挂载/读写对象须绑定）。修复前形态：快照按深度升序挂载，冗余子目录
/// ro-bind 后挂遮蔽父目录 rw 面，且 grants.covers 判「已覆盖」使再次申请无法
/// 修复——对已批准写权限的静默降级。
#[test]
fn nested_read_grant_does_not_mask_approved_write_face() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let parent = f.requestable.canonicalize().unwrap();
    let child = parent.join("sub");
    fs::create_dir_all(&child).unwrap();
    // 反例形态的授权顺序：先 read 子目录、后 write 父目录
    let snap = snapshot_for(
        &f,
        &[(&child, Access::Read), (&f.requestable, Access::Write)],
        false,
    );
    assert!(
        snap.read_paths.iter().all(|r| !r.starts_with(&parent)),
        "write 祖先之下的 read 不应进入挂载面：read={:?}",
        snap.read_paths
    );
    // 行为断言：父目录写授权覆盖子树——子目录内创建/写入必须成功
    let target = child.join("written.txt");
    let script = format!("printf granted-write > {} && cat {}", target.display(), target.display());
    let out = run_payload(&snap, &script, &f.anchor).unwrap();
    assert!(
        out.status.success(),
        "已批准父目录写权限在子树内被静默降级为只读：{}（stderr={:?}）",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "granted-write");
    fs::remove_dir_all(&f.root).ok();
}

// ---------- P16 补（第三轮）：完整启动 p95＋峰值 RSS ----------

/// 打开一对伪终端（镜像 proc_session::open_pty 的生产机制：master O_CLOEXEC、
/// grantpt/unlockpt、ptsname_r）。
fn open_pty_pair() -> std::io::Result<(std::fs::File, std::fs::File)> {
    unsafe {
        let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let ok = libc::grantpt(master_fd) == 0
            && libc::unlockpt(master_fd) == 0
            && libc::fcntl(master_fd, libc::F_SETFD, libc::FD_CLOEXEC) == 0;
        if !ok {
            let e = std::io::Error::last_os_error();
            libc::close(master_fd);
            return Err(e);
        }
        let mut name = [0u8; 256];
        if libc::ptsname_r(master_fd, name.as_mut_ptr().cast(), name.len()) != 0 {
            let e = std::io::Error::last_os_error();
            libc::close(master_fd);
            return Err(e);
        }
        let slave_fd = libc::open(name.as_ptr().cast(), libc::O_RDWR | libc::O_NOCTTY);
        if slave_fd < 0 {
            let e = std::io::Error::last_os_error();
            libc::close(master_fd);
            return Err(e);
        }
        Ok((
            std::fs::File::from(std::os::unix::io::OwnedFd::from_raw_fd(master_fd)),
            std::fs::File::from(std::os::unix::io::OwnedFd::from_raw_fd(slave_fd)),
        ))
    }
}

/// 生产形态 PTY 完整启动（镜像 proc_session::spawn_pty：三路从设备＋pre_exec
/// setsid/TIOCSCTTY＋策略启动的 PID ns 复核）。本文件面向 Linux/bwrap 环境。
fn run_payload_pty(snapshot: &PolicySnapshot, cwd: &Path) -> std::io::Result<std::process::ExitStatus> {
    let (master, slave) = open_pty_pair()?;
    let launch = sandbox::permission_launch_argv(
        vec!["/bin/true".to_string()],
        snapshot,
        cwd,
    )?;
    let in_f = slave.try_clone()?;
    let out_f = slave.try_clone()?;
    let err_f = slave;
    let mut c = Command::new(&launch.program);
    c.args(&launch.args)
        .env_clear()
        .envs(launch.env.iter().cloned())
        .current_dir(cwd)
        .stdin(std::process::Stdio::from(in_f))
        .stdout(std::process::Stdio::from(out_f))
        .stderr(std::process::Stdio::from(err_f));
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = c.spawn()?;
    drop(c);
    sandbox::verify_child_pid_namespace(child.id())?;
    drop(launch); // seccomp fd 守卫：spawn 已返回
    drop(master);
    child.wait()
}

/// 已回收子进程的峰值 RSS（KB，Linux）——RUSAGE_CHILDREN 为进程内单调最大值，
/// 跨并发用例构成沙箱启动峰值的上界口径。
fn children_peak_rss_kb() -> i64 {
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_CHILDREN, &mut ru), 0);
        ru.ru_maxrss
    }
}

/// §1.8/P16：完整启动（组装＋spawn＋bwrap execve＋载荷退出）按 faces 分别测
/// （pipes/PTY/补丁/图片；单并发、预热 5、100 次×3 轮取 p95 中位轮；排除人的
/// 答复时间——全程无确认交互）。冻结阈值（实测基线 2–3 倍余量，见证据
/// 2026-10-29-p16-full-launch-rss-samples.txt）同时作为性能回归门禁。
#[test]
fn p16_full_launch_faces_and_peak_rss() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_terminal-chat"));
    let base = snapshot_for(&f, &[], false);
    let read_snap = snapshot_for(&f, &[(&f.requestable, Access::Read)], false);

    // 计量框架（§1.8 口径）：3 轮×100 次取每轮 p95，报告中位轮；run 的入参
    // ＝全局样本序号（faces 按需用作唯一目标名）。
    fn time_rounds(mut run: impl FnMut(usize)) -> (u128, Vec<u128>) {
        let mut rounds = Vec::new();
        let mut i = 0usize;
        for _ in 0..3 {
            let mut samples = Vec::new();
            for _ in 0..100 {
                let t0 = std::time::Instant::now();
                run(i);
                i += 1;
                samples.push(t0.elapsed().as_micros());
            }
            samples.sort();
            rounds.push(samples[95]);
        }
        let mut sorted = rounds.clone();
        sorted.sort();
        (sorted[1], rounds)
    }

    // 面一：exec pipes（/bin/sh -c true 全链）
    for _ in 0..5 {
        run_payload(&base, "true", &f.anchor).unwrap();
    }
    let (pipes_p95, pipes_rounds) = time_rounds(|_i| {
        run_payload(&base, "true", &f.anchor).unwrap();
    });

    // 面二：exec PTY（生产 spawn_pty 机制全链）
    {
        for _ in 0..5 {
            run_payload_pty(&base, &f.anchor).unwrap();
        }
        let (pty_p95, pty_rounds) = time_rounds(|_i| {
            run_payload_pty(&base, &f.anchor).unwrap();
        });
        if perf_sample_mode() {
            eprintln!("p16 PTY 启动采样：{pty_p95}µs（三轮 {pty_rounds:?}，跳过冻结断言）");
        } else {
            assert!(
                pty_p95 < 30_000,
                "PTY 完整启动 p95 中位轮 {pty_p95}µs 超过 30ms 冻结上限"
            );
        }
        eprintln!("p16 full-launch PTY p95 中位轮＝{pty_p95}µs（三轮 {pty_rounds:?}）");
    }

    // 面三：apply_patch（--internal-apply 载荷全链；每次新增一个小文件）。
    // 载荷 chdir 取进程 cwd：必须落在已挂载面内（anchor），否则 bwrap 拒启造成
    // 「快速失败」的假样本（诊断实证：cwd 未挂载时 ~6.5ms 恒定失败）。
    // set_current_dir 是进程全局——本文件用例全用绝对路径，短暂切换不影响。
    let patch_dir = f.anchor.join("perf-patch");
    fs::create_dir_all(&patch_dir).unwrap();
    let patch_input = |i: usize| {
        serde_json::json!({
            "patch": format!(
                "*** Begin Patch\n*** Add File: {}/f{i}.txt\n+perf\n*** End Patch\n",
                patch_dir.display()
            )
        })
    };
    let saved_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&f.anchor).unwrap();
    for i in 0..5 {
        let (r, _) = terminal_chat::apply_patch::apply_patch_sandboxed_with_exe(&exe, &patch_input(i), Some(&base));
        assert!(r.ok, "预热补丁必须成功：{:?}", r.error);
    }
    let (patch_p95, patch_rounds) = time_rounds(|i| {
        let (r, _) = terminal_chat::apply_patch::apply_patch_sandboxed_with_exe(&exe, &patch_input(300 + i), Some(&base));
        assert!(r.ok, "计量补丁必须成功：{:?}", r.error);
    });
    std::env::set_current_dir(&saved_cwd).unwrap();
    // 防空跑：305 个补丁文件真实落盘
    let created = fs::read_dir(&patch_dir).unwrap().count();
    assert_eq!(created, 305, "补丁面必须真实生效（预期 305 个文件，实得 {created}）");

    // 面四：view_image（--internal-view-image 载荷全链，1×1 PNG）
    let mut png_buf = Vec::new();
    {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([12u8, 34, 56]));
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png_buf), image::ImageFormat::Png)
            .unwrap();
    }
    fs::write(f.requestable.join("perf.png"), &png_buf).unwrap();
    let img_args = format!("{{\"path\": {:?}}}", f.requestable.join("perf.png"));
    let mut img_ok = false;
    for _ in 0..5 {
        let o = terminal_chat::view_image::execute_sandboxed_with_exe(
            &img_args, &f.anchor, None, &read_snap, &exe,
        );
        img_ok |= o.image.is_some();
    }
    assert!(img_ok, "图片面预热必须至少成功一次（防空跑）");
    let mut sampled_ok = false;
    let (image_p95, image_rounds) = time_rounds(|_i| {
        let o = terminal_chat::view_image::execute_sandboxed_with_exe(
            &img_args, &f.anchor, None, &read_snap, &exe,
        );
        sampled_ok |= o.image.is_some();
    });
    assert!(sampled_ok, "图片面计量轮必须成功（防空跑）");

    let peak_rss_kb = children_peak_rss_kb();

    eprintln!(
        "# P16 完整启动样本（µs）pipes p95中位轮＝{pipes_p95}（三轮 {pipes_rounds:?}）\
         patch p95中位轮＝{patch_p95}（三轮 {patch_rounds:?}）\
         image p95中位轮＝{image_p95}（三轮 {image_rounds:?}）峰值RSS上界＝{peak_rss_kb}KB"
    );

    // 冻结阈值（2026-10-29 实测基线 2–3 倍余量；WSL2 容器、debug 载荷二进制
    // ——release 载荷显著更快，阈值为 debug 口径上界）：pipes/PTY 完整启动
    // p95 中位轮 30ms（实测 ~4–5ms）；载荷二进制面（补丁/图片）250ms（实测
    // p95 中位轮 ~65–75ms，含 debug 二进制启动 ~60ms 与偶发 ~155ms 尖峰）；
    // 峰值 RSS 上界 128MiB（实测 ~13.7MB）。超限＝性能回归，阻断交付。
    assert!(pipes_p95 < 30_000 || perf_sample_mode(), "pipes 完整启动 p95 中位轮 {pipes_p95}µs 超过 30ms 冻结上限");
    assert!(patch_p95 < 250_000 || perf_sample_mode(), "补丁完整启动 p95 中位轮 {patch_p95}µs 超过 250ms 冻结上限");
    assert!(image_p95 < 250_000 || perf_sample_mode(), "图片完整启动 p95 中位轮 {image_p95}µs 超过 250ms 冻结上限");
    assert!(
        peak_rss_kb < 128 * 1024,
        "沙箱启动子进程峰值 RSS {peak_rss_kb}KB 超过 128MiB 冻结上限"
    );
    fs::remove_dir_all(&f.root).ok();
}
