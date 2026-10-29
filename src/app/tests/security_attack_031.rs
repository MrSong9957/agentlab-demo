//! 031 权限申请——安全攻防攻击测试（攻击者＝模型视角）。
//!
//! 031 安全攻防回归集（审查 #10：已纳入回归集合随分支交付——H1/L1/L2 修复
//! 的执行层回归常驻于此；运行约定 `--test-threads=1`）。
//! 与 request_permissions*.rs 的既有覆盖互补：只写既有测试未覆盖的攻击面。
//! 全部使用真实 bwrap、隔离临时目录与 FAKE-* 假数据；不触真实凭证。
//!
//! 用例索引（对应安全报告 docs/security/reports/2026-10-29-031-request-permissions-security.md）：
//! - attack01：请求参数偷塞/伪造——整次拒绝且零确认弹窗
//! - attack02：路径攻击（相似前缀/相对逃逸/../%XX/中文/符号链接/缺失组件）＋执行面
//! - attack03：获批树内符号链接逃逸containment（真实 bwrap 双执行）
//! - attack04：毒丸快照下 apply_patch 是否回退 033 全盘读策略（HIGH 发现复现）
//! - attack05：64 条上限原子性/超上限零弹窗/deny 覆盖路径的误导性 granted
//! - attack06：seccomp 实弹——正常工具不误伤、清单内调用被拦（沙箱内 gcc 编译探针）
//! - attack07：setsid 守护进程跨轮存活（到期收尾必须整树回收）

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use terminal_chat::exec::ExecArgs;
use terminal_chat::permissions::{self, Access, DenyContext, PermissionDecision, PermissionHost, PermissionRequestView};
use terminal_chat::proc_session::ProcessManager;
use terminal_chat::sandbox::{self, PolicySnapshot};

// ---------- 夹具 ----------

fn root_dir(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "tchat-031-attack-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

struct Fx {
    root: PathBuf,
    anchor: PathBuf,
    reqa: PathBuf,
    sib: PathBuf, // 相似前缀目录（reqA-sib）
    outside: PathBuf,
}

fn fx() -> Fx {
    let root = root_dir("fx");
    let anchor = root.join("workspace");
    let reqa = root.join("reqA");
    let sib = root.join("reqA-sib");
    let outside = root.join("outside");
    for d in [&anchor, &reqa, &sib, &outside] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(anchor.join(".env"), "FAKE-ATTACK-ENV=SECRET\n").unwrap();
    fs::write(outside.join("host-secret.txt"), "FAKE-ATTACK-HOST-SECRET\n").unwrap();
    fs::write(sib.join("sib-secret.txt"), "FAKE-ATTACK-SIB-SECRET\n").unwrap();
    Fx { root, anchor, reqa, sib, outside }
}

fn deny_ctx(f: &Fx) -> DenyContext {
    let cfg = f.root.join("fake-cfg");
    let usr = f.root.join("fake-usr");
    let inst = f.root.join("fake-install");
    for d in [&cfg, &usr, &inst] {
        fs::create_dir_all(d).unwrap();
    }
    DenyContext {
        anchor: f.anchor.clone(),
        config_dir: cfg,
        user_dir: usr,
        install_dirs: vec![inst],
    }
}

/// 策略：base＝anchor write；requestable＝给定条目集合。
fn policy_for(f: &Fx, entries: &[(PathBuf, Access)], net: bool) -> permissions::Policy {
    let anchor = f.anchor.canonicalize().unwrap();
    let entry = |(p, a): &(PathBuf, Access)| {
        format!(
            r#"{{"path": {{"type": "path", "path": {:?}}}, "access": "{}"}}"#,
            p.canonicalize().unwrap(),
            match a {
                Access::Read => "read",
                Access::Write => "write",
            }
        )
    };
    let text = format!(
        r#"{{"version": 1, "base": {{"file_system": {{"entries": [{}]}}, "network": {{"enabled": false}}}}, "requestable": {{"file_system": {{"entries": [{}]}}, "network": {{"enabled": {net}}}}}, "deny_paths": []}}"#,
        entry(&(anchor, Access::Write)),
        entries.iter().map(entry).collect::<Vec<_>>().join(","),
    );
    permissions::Policy::parse(&text).unwrap()
}

// ---------- 宿主 ----------

struct CountingHost {
    allow: bool,
    calls: usize,
}

impl CountingHost {
    fn deny() -> Self {
        Self { allow: false, calls: 0 }
    }
    fn allow() -> Self {
        Self { allow: true, calls: 0 }
    }
}

impl PermissionHost for CountingHost {
    fn confirm(&mut self, _view: &PermissionRequestView) -> Result<PermissionDecision, String> {
        self.calls += 1;
        Ok(if self.allow { PermissionDecision::Allow } else { PermissionDecision::Deny })
    }
}

// ---------- 执行辅助（真实 bwrap，直接经 permission_launch_argv） ----------

fn run_payload(snapshot: &PolicySnapshot, script: &str, cwd: &Path) -> std::io::Result<std::process::Output> {
    let launch = sandbox::permission_launch_argv(
        vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()],
        snapshot,
        cwd,
    )?;
    let mut c = Command::new(&launch.program);
    c.args(&launch.args);
    c.env_clear();
    c.envs(launch.env.iter().cloned());
    let out = c.output()?;
    drop(launch);
    Ok(out)
}

fn bwrap_available() -> bool {
    sandbox::probe_available()
}

fn ask_args(entries: &[(&Path, &str)]) -> String {
    json!({
        "permissions": {"file_system": {"entries": entries.iter().map(|(p, a)| json!({
            "path": {"type": "path", "path": p.display().to_string()},
            "access": a,
        })).collect::<Vec<_>>()}}
    })
    .to_string()
}

fn status_of(result_json: &str) -> (String, Value) {
    let v: Value = serde_json::from_str(result_json).expect("结果必须是合法 JSON");
    let keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    assert!(
        keys.len() == 4
            && keys.contains(&"status".to_string())
            && keys.contains(&"permissions".to_string())
            && keys.contains(&"scope".to_string())
            && keys.contains(&"error".to_string()),
        "结果必须恰为四字段：{v}"
    );
    (v["status"].as_str().unwrap().to_string(), v)
}

// ================================================================
// attack01：请求参数偷塞/伪造——必须整次拒绝、零确认弹窗
// ================================================================

#[test]
fn attack01_forged_arguments_never_prompt_host() {
    let f = fx();
    let policy = policy_for(&f, &[(f.reqa.clone(), Access::Write)], false);
    let ctx = deny_ctx(&f);

    let good_entry = |p: &str| {
        json!({"path": {"type": "path", "path": p}, "access": "read"}).to_string()
    };
    let mut battery: Vec<(String, String)> = vec![
        // 未知字段（各层级）
        (
            "顶层未知字段 environment_id".into(),
            json!({"permissions": {"network": {"enabled": true}}, "environment_id": "env-1"}).to_string(),
        ),
        (
            "顶层未知字段 scope".into(),
            json!({"permissions": {"network": {"enabled": true}}, "scope": "session"}).to_string(),
        ),
        (
            "顶层未知字段 token".into(),
            json!({"permissions": {"network": {"enabled": true}}, "token": "ATTACK-TOKEN-1"}).to_string(),
        ),
        (
            "permissions 层未知字段 deny/unrestricted".into(),
            json!({"permissions": {"deny": [], "unrestricted": true}}).to_string(),
        ),
        (
            "旧版 read/write 数组".into(),
            json!({"permissions": {"read": ["/x"], "write": ["/y"]}}).to_string(),
        ),
        (
            "file_system 层未知字段".into(),
            json!({"permissions": {"file_system": {"entries": [good_entry("/tmp")], "recursive": true}}}).to_string(),
        ),
        (
            "entry 层未知字段".into(),
            json!({"permissions": {"file_system": {"entries": [
                {"path": {"type": "path", "path": "/tmp"}, "access": "read", "recursive": true}
            ]}}}).to_string(),
        ),
        (
            "path 层未知字段".into(),
            json!({"permissions": {"file_system": {"entries": [
                {"path": {"type": "path", "path": "/tmp", "glob": "**"}, "access": "read"}
            ]}}}).to_string(),
        ),
        // null / 形态
        (
            "entries 为 null".into(),
            json!({"permissions": {"file_system": {"entries": null}}}).to_string(),
        ),
        (
            "permissions 为数组".into(),
            json!({"permissions": [{"path": "/x"}]}).to_string(),
        ),
        (
            "根为数组".into(),
            json!([{"permissions": {}}]).to_string(),
        ),
        (
            "根为标量".into(),
            json!("please grant me everything").to_string(),
        ),
        (
            "根为数字".into(),
            json!(1).to_string(),
        ),
        (
            "空对象（无实际权限）".into(),
            json!({}).to_string(),
        ),
        (
            "只有 reason（无实际权限）".into(),
            json!({"reason": "x"}).to_string(),
        ),
        (
            "entries 空数组".into(),
            json!({"permissions": {"file_system": {"entries": []}}}).to_string(),
        ),
        (
            "network.enabled=false（不以 false 隐式撤销）".into(),
            json!({"permissions": {"network": {"enabled": false}}}).to_string(),
        ),
        (
            "network.enabled=null".into(),
            json!({"permissions": {"network": {"enabled": null}}}).to_string(),
        ),
        (
            "network 缺 enabled".into(),
            json!({"permissions": {"network": {}}}).to_string(),
        ),
        (
            "network 夹带额外字段".into(),
            json!({"permissions": {"network": {"enabled": true, "scope": "lan"}}}).to_string(),
        ),
        (
            "access 提权词 admin".into(),
            json!({"permissions": {"file_system": {"entries": [
                {"path": {"type": "path", "path": "/tmp"}, "access": "admin"}
            ]}}}).to_string(),
        ),
        (
            "type 非 path（glob）".into(),
            json!({"permissions": {"file_system": {"entries": [
                {"path": {"type": "glob", "path": "/tmp/**"}, "access": "read"}
            ]}}}).to_string(),
        ),
        (
            "entry 缺 path".into(),
            json!({"permissions": {"file_system": {"entries": [{"access": "read"}]}}}).to_string(),
        ),
        (
            "重复 JSON 键（顶层 permissions）".into(),
            r#"{"permissions": {"network": {"enabled": true}}, "permissions": {"network": {"enabled": true}}}"#.into(),
        ),
        (
            "重复 JSON 键（entries 层）".into(),
            r#"{"permissions": {"file_system": {"entries": [], "entries": []}}}"#.into(),
        ),
        (
            "重复 JSON 键（path 层）".into(),
            r#"{"permissions": {"file_system": {"entries": [
                {"path": {"type": "path", "path": "/a", "path": "/b"}, "access": "read"}
            ]}}}"#.into(),
        ),
        // 控制字符与超限
        (
            "路径含 NUL/控制字符".into(),
            json!({"permissions": {"file_system": {"entries": [
                {"path": {"type": "path", "path": "/tmp/a\u{1}b"}, "access": "read"}
            ]}}}).to_string(),
        ),
        (
            "reason 含 ESC 终端序列".into(),
            json!({"reason": "\u{1b}[31mINJECT\u{1b}[0m", "permissions": {"network": {"enabled": true}}}).to_string(),
        ),
        (
            "路径 4097 字节".into(),
            json!({"permissions": {"file_system": {"entries": [
                {"path": {"type": "path", "path": format!("/tmp/{}", "a".repeat(4091))}, "access": "read"}
            ]}}}).to_string(),
        ),
        (
            "reason 4097 字节".into(),
            json!({"reason": "r".repeat(4097), "permissions": {"network": {"enabled": true}}}).to_string(),
        ),
        (
            "arguments 超 64KiB".into(),
            json!({"reason": "x".repeat(70000), "permissions": {"network": {"enabled": true}}}).to_string(),
        ),
        // 33 条（超单请求 32 上限）
        (
            "33 条 entries".into(),
            {
                let items: Vec<String> = (0..33).map(|i| good_entry(&format!("/tmp/f{i}"))).collect();
                format!("{{\"permissions\": {{\"file_system\": {{\"entries\": [{}]}}}}}}", items.join(","))
            },
        ),
    ];

    let mut host = CountingHost::deny();
    for (label, args) in battery.drain(..) {
        let mut grants = permissions::TurnGrants::new();
        let out = permissions::execute_request_permissions(
            &args, &f.anchor, &policy, &mut grants, &mut host, &ctx,
        );
        let (status, v) = status_of(&out);
        assert_eq!(status, "error", "用例 [{label}] 应整次拒绝：{v}");
        assert_eq!(host.calls, 0, "用例 [{label}] 不得弹出确认界面");
        assert!(
            grants.is_empty(),
            "用例 [{label}] 不得扩大授权：{:?}",
            grants.entries
        );
    }

    // 正例对照（防空跑）：合法请求恰好弹一次窗；Deny 后无授权；Allow 后 granted。
    let mut host = CountingHost::deny();
    let mut grants = permissions::TurnGrants::new();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&f.reqa, "read")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "denied");
    assert_eq!(host.calls, 1, "合法请求应恰好弹一次确认");
    assert!(grants.is_empty());

    // 4096 字节 reason（边界内）＝合法请求
    let mut host = CountingHost::deny();
    let mut grants = permissions::TurnGrants::new();
    let args = json!({
        "reason": "r".repeat(4096),
        "permissions": {"file_system": {"entries": [
            {"path": {"type": "path", "path": f.reqa.display().to_string()}, "access": "read"}
        ]}}
    })
    .to_string();
    let out = permissions::execute_request_permissions(
        &args, &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "denied", "4096 字节 reason 应按合法请求处理（弹窗后拒绝）");
    assert_eq!(host.calls, 1);
    fs::remove_dir_all(&f.root).ok();
}

// ================================================================
// attack02：路径攻击——相似前缀/相对逃逸/../%XX/中文/符号链接/缺失组件
// ================================================================

#[test]
fn attack02_path_attacks_parse_and_enforce() {
    let f = fx();
    // requestable＝reqA write ＋ 中文目录 + 字面 %XX 目录（均为已存在实际路径）
    let zh = f.reqa.join("目录甲");
    let pct = f.reqa.join("%41");
    fs::create_dir_all(&zh).unwrap();
    fs::create_dir_all(&pct).unwrap();
    let policy = policy_for(
        &f,
        &[(f.reqa.clone(), Access::Write), (zh.clone(), Access::Write), (pct.clone(), Access::Write)],
        false,
    );
    let ctx = deny_ctx(&f);
    let mut grants = permissions::TurnGrants::new();

    // 1) 相对路径逃逸 cwd：../outside → 解析为 root/outside，超出 requestable → error、零弹窗
    let mut host = CountingHost::deny();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&f.root.join("outside"), "write")]),
        &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, v) = status_of(&out);
    assert_eq!(status, "error", "相对逃逸目标超出 requestable：{v}");
    assert_eq!(host.calls, 0, "超上限请求不得弹窗");
    assert!(
        v["error"].as_str().unwrap().contains("requestable"),
        "错误应归因 requestable：{v}"
    );

    // 2) `..` 词法穿越：reqA/./x/../x 归一为 reqA/x
    let x = f.reqa.join("x");
    fs::create_dir_all(&x).unwrap();
    let mut host = CountingHost::allow();
    let out = permissions::execute_request_permissions(
        &json!({"permissions": {"file_system": {"entries": [
            {"path": {"type": "path", "path": format!("{}/./y/../x", f.reqa.display())}, "access": "read"}
        ]}}})
        .to_string(),
        &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, v) = status_of(&out);
    assert_eq!(status, "granted", "{v}");
    let shown = v["permissions"]["file_system"]["entries"][0]["path"]["path"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(shown, x.canonicalize().unwrap().display().to_string(), "展示路径应为词法归一后的实际路径：{v}");

    // 3) 字面 %XX 不解码：%41 目录按字面批准；解码形态（A）不存在 → 拒绝
    let mut host = CountingHost::allow();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&pct, "read")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "granted", "字面 %41 目录应按字面处理");
    let decoded = f.reqa.join("A");
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&decoded, "read")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, v) = status_of(&out);
    assert_eq!(status, "error", "URL 解码形态不得命中 %41 目录：{v}");
    assert!(v["error"].as_str().unwrap().contains("不存在"), "应按缺失组件拒绝：{v}");

    // 4) 中文目录按字节原样批准（正例）
    let mut host = CountingHost::allow();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&zh, "read")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "granted", "中文路径应原样处理");

    // 5) 符号链接组件：reqA/link → outside，请求必须拒绝
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.outside, f.reqa.join("link")).unwrap();
    #[cfg(unix)]
    {
        let mut host = CountingHost::deny();
        let out = permissions::execute_request_permissions(
            &ask_args(&[(&f.reqa.join("link"), "read")]),
            &f.anchor, &policy, &mut grants, &mut host, &ctx,
        );
        let (status, v) = status_of(&out);
        assert_eq!(status, "error", "符号链接组件必须拒绝：{v}");
        assert_eq!(host.calls, 0);
    }

    // 6) 缺失组件：reqA/no/such
    let mut host = CountingHost::deny();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&f.reqa.join("no/such"), "read")]),
        &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "error", "缺失组件必须拒绝");

    // 7) 相似前缀：授权 reqA 后，reqA-sib 不被覆盖（组件包含判定）——
    //    超出 requestable 先拒绝；把它放进 requestable 后必须重新弹窗而非静默覆盖
    let mut host = CountingHost::deny();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&f.sib, "read")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "error", "reqA-sib 超出 requestable（前缀相似不等于覆盖）");
    assert_eq!(host.calls, 0);
    let policy2 = policy_for(&f, &[(f.reqa.clone(), Access::Write), (f.sib.clone(), Access::Read)], false);
    let mut host = CountingHost::deny();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&f.sib, "read")]), &f.anchor, &policy2, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "denied");
    assert_eq!(host.calls, 1, "新路径不得因与已授权路径共享字符串前缀而免确认");

    // 8) 执行面（真实 bwrap）：授权 reqA write 后——reqA 可写（正例防空跑）；
    //    reqA-sib 与 outside 内容不可见（ENOENT）；.env 正文不可得
    if bwrap_available() {
        grants.grant_all(&permissions::NormalizedRequest {
            reason: None,
            entries: vec![permissions::NormalizedEntry {
                path: f.reqa.canonicalize().unwrap(),
                access: Access::Write,
            }],
            network: false,
        })
        .unwrap();
        let snap = permissions::build_snapshot(&policy_for(&f, &[(f.reqa.clone(), Access::Write)], false), &grants, &ctx)
            .unwrap();
        let out = run_payload(
            &snap,
            &format!(
                "echo ATTACK-WRITE-OK > {r}/w.txt && cat {r}/w.txt; \
                 cat {s} 2>&1; cat {o} 2>&1; cat {e} 2>&1",
                r = f.reqa.display(),
                s = f.sib.join("sib-secret.txt").display(),
                o = f.outside.join("host-secret.txt").display(),
                e = f.anchor.join(".env").display(),
            ),
            &f.anchor,
        )
        .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("ATTACK-WRITE-OK"), "获批面写入正例（防空跑）：{text}");
        assert!(
            !text.contains("FAKE-ATTACK-SIB-SECRET") && !text.contains("FAKE-ATTACK-HOST-SECRET"),
            "相似前缀目录与未批树内容不可得：{text}"
        );
        assert!(!text.contains("FAKE-ATTACK-ENV"), ".env 正文不可得：{text}");
    }
    fs::remove_dir_all(&f.root).ok();
}

// ================================================================
// attack03：获批树内符号链接逃逸 containment（真实 bwrap）
// ================================================================

#[test]
fn attack03_symlink_escape_contained_in_sandbox() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let policy = policy_for(&f, &[(f.reqa.clone(), Access::Write)], false);
    let ctx = deny_ctx(&f);
    let mut grants = permissions::TurnGrants::new();
    grants.grant_all(&permissions::NormalizedRequest {
        reason: None,
        entries: vec![permissions::NormalizedEntry {
            path: f.reqa.canonicalize().unwrap(),
            access: Access::Write,
        }],
        network: false,
    })
    .unwrap();
    let snap = permissions::build_snapshot(&policy, &grants, &ctx).unwrap();

    // 第一跳（获批 write 面内）：创建指向 / 与指向未批树的符号链接
    let out = run_payload(
        &snap,
        &format!(
            "ln -s / {r}/to-root && ln -s {o} {r}/to-outside && echo LINKS-OK",
            r = f.reqa.display(),
            o = f.outside.display(),
        ),
        &f.anchor,
    )
    .unwrap();
    assert!(out.status.success(), "符号链接创建应成功：{}", String::from_utf8_lossy(&out.stderr));

    // 第二跳（新快照重新挂载）：经符号链接访问——
    // to-outside 悬空（未批树未挂载）；to-root 解析为沙箱根（.env 仍遮蔽）
    let out = run_payload(
        &snap,
        &format!(
            "cat {r}/to-outside/host-secret.txt 2>&1; \
             cat {r}/to-root/{a}/.env 2>&1; \
             cat {r}/to-root/{o}/host-secret.txt 2>&1; echo PROBE-DONE",
            r = f.reqa.display(),
            a = f.anchor.display(),
            o = f.outside.display(),
        ),
        &f.anchor,
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("FAKE-ATTACK-HOST-SECRET"),
        "经获批树内符号链接不得读到未批树正文：{text}"
    );
    assert!(
        !text.contains("FAKE-ATTACK-ENV"),
        "经 to-root 符号链接不得读到 .env 正文（解析必须落在沙箱命名空间内）：{text}"
    );
    assert!(text.contains("PROBE-DONE"), "探针应完整执行：{text}");
    fs::remove_dir_all(&f.root).ok();
}

// ================================================================
// attack04：毒丸快照下 apply_patch 静默回退 033 全盘读策略（HIGH 发现复现）
// ================================================================

/// 复现 turn.rs execute_tool 的分支：
/// `exec_snapshot.as_ref().and_then(|s| s.valid())` 在毒丸时得到 None，
/// 随后 `else if sandbox_requested()` 分支用 **033 固定策略**（全盘只读＋
/// 工作区/临时目录可写）执行 apply_patch——与 exec_command 的毒丸显式失败
/// 不一致，违反 §1.2-2/§1.6 的 fail-closed（不回退 033 固定策略）。
/// 本用例以真实拓扑触发毒丸（自动敏感边界 user_dir 覆盖 base 写面），
/// 对照证明：同一毒丸下 exec_command 拒绝执行，apply_patch 却以 033 策略
/// 成功改写了 031 受限树之外的宿主临时目录文件。
#[test]
fn attack04_poisoned_snapshot_apply_patch_falls_back_to_033() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    // 部署拓扑：user_dir（自动敏感边界，整体 deny）恰为 base 写面本身——
    // build_snapshot 的「内容路径位于永久拒绝路径之下」检查触发毒丸。
    // 该拓扑在真实部署可由 TCHAT_USER_DIR/HOME 误配进入工作区等途径出现，
    // 审查 #20：install_from_env 启动期已做基线预检（permissions.rs）；本
    // 用例直接构造 DenyContext 与毒丸快照，单独验证分派层的毒丸一致性。
    let ctx = DenyContext {
        anchor: f.anchor.clone(),
        config_dir: f.root.join("fake-cfg"),
        user_dir: f.anchor.clone(), // ← deny 祖先＝内容路径 ⇒ 毒丸
        install_dirs: vec![f.root.join("fake-install")],
    };
    fs::create_dir_all(f.root.join("fake-cfg")).unwrap();
    fs::create_dir_all(f.root.join("fake-install")).unwrap();
    let policy = policy_for(&f, &[(f.reqa.clone(), Access::Write)], false);
    let grants = permissions::TurnGrants::new();
    let build = permissions::build_snapshot(&policy, &grants, &ctx);
    assert!(build.is_err(), "预置条件：基线快照应无法编码（毒丸）");
    let poisoned = PolicySnapshot::poisoned(build.unwrap_err());

    // 对照 A：exec_command 同毒丸——显式失败、零执行（既有契约）
    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(false);
    let marker = f.root.join("EXEC-RAN");
    let poisoned_arc = std::sync::Arc::new(poisoned.clone());
    let out = mgr
        .start_with_policy(
            &ExecArgs::from_value(&json!({
                "cmd": format!("echo x > {}", marker.display()),
                "yield_time_ms": 500,
            }))
            .unwrap(),
            Some(&cancel),
            Some(&poisoned_arc),
            true,
        )
        .result;
    assert_eq!(out.status, terminal_chat::exec::ExecStatus::Error, "exec_command 应显式失败");
    assert!(!marker.exists(), "exec_command 毒丸零执行");
    mgr.close();

    // 回归 B（H1 修复后）：毒丸 valid()=None 时 turn 分派**不再**落入 033 回退，
    // 而是显式错误（此处直接以毒丸快照传参——与修复后 turn.rs 的
    // Some(None) 分支同构：拒绝且文件不被改写）。
    std::env::set_var("TCHAT_SANDBOX", "1"); // 与生产 031 模式同前提（031 要求该开关）
    let probe_dir = std::env::temp_dir().join(format!("attack031-patch-{}", std::process::id()));
    fs::create_dir_all(&probe_dir).unwrap();
    let probe = probe_dir.join("probe.txt");
    fs::write(&probe, "FAKE-ATTACK-OLD\n").unwrap();
    let patch = format!(
        "*** Begin Patch\n*** Update File: {}\n@@\n-FAKE-ATTACK-OLD\n+FAKE-ATTACK-NEW\n*** End Patch\n",
        probe.display()
    );
    // 修复后行为：permission_launch_argv 对毒丸直接 Err（拒绝启动）——
    // apply_patch 结果为显式错误，探针文件保持旧内容（零改写）。
    let (r, _) = terminal_chat::apply_patch::apply_patch_sandboxed_with_exe(
        Path::new(env!("CARGO_BIN_EXE_terminal-chat")),
        &json!({"patch": patch}),
        Some(&poisoned),
    );
    eprintln!("attack04 apply_patch 结果（修复后）：{:?}", r);
    assert!(!r.ok, "毒丸快照下 apply_patch 必须失败");
    assert!(
        r.error.as_deref().unwrap_or("").contains("无法编码"),
        "错误应说明生效策略不可编码：{:?}",
        r.error
    );
    let after = fs::read_to_string(&probe).unwrap();
    assert_eq!(
        after.trim(),
        "FAKE-ATTACK-OLD",
        "H1 回归：毒丸快照下 apply_patch 不得以 033 全盘策略改写受限树外文件"
    );
    let _ = fs::remove_dir_all(&probe_dir);
    let _ = fs::remove_dir_all(&f.root);
}

// ================================================================
// attack05：64 条上限原子性／超上限零弹窗／deny 覆盖路径的误导性 granted
// ================================================================

#[test]
fn attack05_grant_limit_atomicity_and_deny_covered_request() {
    let f = fx();
    // requestable 同时含 anchor write（模拟管理员允许申请工作区）
    let policy = policy_for(&f, &[(f.reqa.clone(), Access::Write), (f.anchor.clone(), Access::Write)], false);
    let ctx = deny_ctx(&f);
    let mut grants = permissions::TurnGrants::new();

    // 预置 63 条已授权路径（reqA/sub01..sub63，去重后计量）
    let mut subs = Vec::new();
    for i in 1..=63 {
        let d = f.reqa.join(format!("sub{i:02}"));
        fs::create_dir_all(&d).unwrap();
        subs.push(d);
    }
    for d in &subs {
        grants
            .grant_all(&permissions::NormalizedRequest {
                reason: None,
                entries: vec![permissions::NormalizedEntry {
                    path: d.canonicalize().unwrap(),
                    access: Access::Write,
                }],
                network: false,
            })
            .unwrap();
    }
    assert_eq!(grants.entries.len(), 63);

    // 一次申请两条新路径（63+2=65>64）：允许后必须整次失败、零部分提交
    let s64 = f.reqa.join("sub64");
    let s65 = f.reqa.join("sub65");
    fs::create_dir_all(&s64).unwrap();
    fs::create_dir_all(&s65).unwrap();
    let mut host = CountingHost::allow();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&s64, "write"), (&s65, "write")]),
        &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, v) = status_of(&out);
    assert_eq!(status, "error", "超 64 条应整次失败：{v}");
    // 审查 #11（§1.7-1 确认前失败）：超限预检在 host.confirm 之前命中——
    // 零弹窗（旧行为「允许后才失败」被钉成预期属假绿，已翻转）。
    assert_eq!(host.calls, 0, "超限须在确认前失败：不弹窗");
    assert_eq!(grants.entries.len(), 63, "原子性：失败不得部分提交");
    assert!(
        !grants.entries.iter().any(|e| e.path == s64.canonicalize().unwrap()),
        "sub64 不得被部分授予"
    );

    // 单条（63+1=64=上限）：允许后成功
    let mut host = CountingHost::allow();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&s64, "write")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "granted");
    assert_eq!(grants.entries.len(), 64, "恰好 64 条允许");

    // 已覆盖请求（sub64 write 再申请）：直接 granted、零弹窗
    let mut host = CountingHost::deny();
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&s64, "write")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, _) = status_of(&out);
    assert_eq!(status, "granted", "已覆盖请求直接 granted");
    assert_eq!(host.calls, 0, "已覆盖请求不得重复弹窗");

    // 【LOW 发现】授权覆盖下的永久拒绝路径：anchor write 已授权，
    // 请求 read anchor/.env——covers() 不查 deny 集 → 静默 granted；
    // 强制执行层仍遮蔽（下方真实 bwrap 验证），但结果对模型是误导性承诺。
    let mut host = CountingHost::deny();
    let env_path = f.anchor.join(".env");
    let out = permissions::execute_request_permissions(
        &ask_args(&[(&env_path, "read")]), &f.anchor, &policy, &mut grants, &mut host, &ctx,
    );
    let (status, v) = status_of(&out);
    // L1 修复后：deny 命中的申请＝整次 error（永久拒绝优先），不 granted 不弹窗
    assert_eq!(status, "error", "L1 回归：deny 覆盖路径不得静默 granted：{v}");
    assert!(
        v["error"].as_str().unwrap_or("").contains("永久拒绝"),
        "错误应说明永久拒绝优先：{v}"
    );
    assert_eq!(host.calls, 0, "deny 命中不得弹窗");

    // 强制执行层兜底验证：granted 之后 .env 仍不可读
    if bwrap_available() {
        let snap = permissions::build_snapshot(&policy, &grants, &ctx).unwrap();
        let out = run_payload(&snap, &format!("cat {}", env_path.display()), &f.anchor).unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("FAKE-ATTACK-ENV"), "granted 后 .env 正文仍不可得：{text}");
    }
    fs::remove_dir_all(&f.root).ok();
}

// ================================================================
// attack06：seccomp 实弹——正常工具不误伤；清单内特权调用被拦
// ================================================================

#[test]
fn attack06_seccomp_allows_toolchain_blocks_privileged_syscalls() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    if !Path::new("/usr/bin/gcc").exists() {
        eprintln!("跳过：gcc 不存在（attack06 探针依赖编译链）");
        return;
    }
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    let f = fx();
    let policy = policy_for(&f, &[(f.reqa.clone(), Access::Write)], false);
    let ctx = deny_ctx(&f);
    let snap = permissions::build_snapshot(&policy, &permissions::TurnGrants::new(), &ctx).unwrap();
    let script = r#"
set -u
sh -c 'echo SH-OK' || echo SH-FAIL
bash -c 'echo BASH-OK' 2>&1 || echo BASH-FAIL
cp /etc/hostname /tmp/cphost 2>&1 && grep -q . /tmp/cphost && echo CP-OK || echo CP-FAIL
cat > /tmp/probe.c <<'EOF'
#include <stdio.h>
#include <errno.h>
#include <string.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sched.h>
#ifndef CLONE_NEWNS
#define CLONE_NEWNS 0x00020000
#endif
static void t(const char* name, long nr, long a, long b, long c) {
  errno = 0;
  long r = syscall(nr, a, b, c);
  printf("%s=%s\n", name, r == -1 ? strerror(errno) : "Success");
}
int main(void) {
  t("mount", SYS_mount, (long)"none", (long)"/mnt", 0);
  t("umount2", SYS_umount2, (long)"/tmp", 0, 0);
  t("pivot_root", SYS_pivot_root, (long)".", (long)".", 0);
  t("bpf", SYS_bpf, 0, 0, 0);
  t("ptrace", SYS_ptrace, 0, 0, 0);
  t("setns", SYS_setns, 0, 0, 0);
  t("open_by_handle_at", SYS_open_by_handle_at, 0, 0, 0);
  t("process_vm_readv", SYS_process_vm_readv, 0, 0, 0);
  t("clone3", SYS_clone3, 0, 0, 0);
  t("unshare_newns", SYS_unshare, CLONE_NEWNS, 0, 0);
#ifdef SYS_unshare
  t("unshare_newuser", SYS_unshare, 0x10000000, 0, 0); /* CLONE_NEWUSER */
  /* 新 userns 能力（U2）不拥有外层 mount ns（属 U1）：挂载仍应 EPERM */
  t("mount_after_newuser", SYS_mount, (long)"none", (long)"/mnt2", 0);
  t("unshare_newns_after_newuser", SYS_unshare, CLONE_NEWNS, 0, 0);
#endif
#ifdef SYS_keyctl
  t("keyctl", SYS_keyctl, 0, 0, 0);
#endif
#ifdef SYS_perf_event_open
  t("perf_event_open", SYS_perf_event_open, 0, 0, 0);
#endif
  return 0;
}
EOF
gcc -o /tmp/probe /tmp/probe.c 2>/tmp/gcc.err && echo GCC-OK || { echo GCC-FAIL; cat /tmp/gcc.err; }
/tmp/probe 2>&1
echo PROBE-EXIT=$?
"#;
    let out = run_payload(&snap, script, &f.anchor).unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    eprintln!("attack06 载荷输出：\n{text}");
    // 正常工具不误伤（bash/cp/gcc 在 seccomp 下可用）
    assert!(text.contains("SH-OK"), "sh 应可用：{text}");
    assert!(text.contains("BASH-OK"), "bash 应可用：{text}");
    assert!(text.contains("CP-OK"), "cp 应可用：{text}");
    assert!(text.contains("GCC-OK"), "gcc 编译链应可用（seccomp 不误伤）：{text}");
    // 清单内调用被拦（EPERM/ENOSYS，而非 Success）
    for name in [
        "mount", "umount2", "pivot_root", "bpf", "ptrace", "setns",
        "open_by_handle_at", "process_vm_readv",
        // L2 修复回归：unshare(2) 与 clone-ns 掩码同意图——嵌套命名空间创建拦截
        "unshare_newuser",
    ] {
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("{name}=")))
            .unwrap_or_else(|| panic!("缺少 {name} 探针输出：{text}"));
        assert!(
            line.ends_with("Operation not permitted"),
            "{name} 应被 seccomp 拦截（EPERM），实测 {line}"
        );
    }
    let clone3 = text.lines().find(|l| l.starts_with("clone3=")).unwrap();
    assert!(clone3.ends_with("Function not implemented"), "clone3 应 ENOSYS：{clone3}");
    assert!(
        text.contains("PROBE-EXIT=0"),
        "探针进程本身应存活到结束（KILL_PROCESS 只对异架构）"
    );
    fs::remove_dir_all(&f.root).ok();
}

// ================================================================
// attack07：setsid 守护进程跨轮存活——到期收尾必须整树回收
// ================================================================

#[test]
fn attack07_setsid_daemon_reaped_at_turn_end() {
    if !bwrap_available() {
        eprintln!("跳过：bwrap 不可用");
        return;
    }
    if !Path::new("/usr/bin/setsid").exists() && !Path::new("/bin/setsid").exists() {
        eprintln!("跳过：setsid 不可用");
        return;
    }
    let f = fx();
    let policy = policy_for(&f, &[(f.reqa.clone(), Access::Write)], false);
    let ctx = deny_ctx(&f);
    let mut grants = permissions::TurnGrants::new();
    grants.grant_all(&permissions::NormalizedRequest {
        reason: None,
        entries: vec![permissions::NormalizedEntry {
            path: f.reqa.canonicalize().unwrap(),
            access: Access::Write,
        }],
        network: false,
    })
    .unwrap();
    let snap = std::sync::Arc::new(permissions::build_snapshot(&policy, &grants, &ctx).unwrap());
    let log = f.reqa.join("daemon.log");
    let _ = fs::remove_file(&log);

    let mgr = ProcessManager::new();
    let cancel = AtomicBool::new(false);
    let out = mgr
        .start_with_policy(
            &ExecArgs::from_value(&json!({
                "cmd": format!(
                    "setsid sh -c 'i=0; while [ $i -lt 300 ]; do echo tick >> {log}; i=$((i+1)); sleep 0.2; done' >/dev/null 2>&1 & sleep 60",
                    log = log.display()
                ),
                "yield_time_ms": 800,
                "workdir": f.anchor.display().to_string(),
            }))
            .unwrap(),
            Some(&cancel),
            Some(&snap),
            true, // 额外权限进程：轮末必须整树回收
        )
        .result;
    let sid = out.session_id.clone().expect("应返回句柄");
    std::thread::sleep(std::time::Duration::from_millis(700));
    let count = |log: &Path| -> usize {
        fs::read_to_string(log).map(|c| c.lines().count()).unwrap_or(0)
    };
    let before = count(&log);
    assert!(before > 0, "守护进程应已开始写日志（防空跑）：{:?}", out.output);

    // 轮出口收尾
    mgr.reap_extra_permission_sessions().expect("收尾应成功");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let after1 = count(&log);
    std::thread::sleep(std::time::Duration::from_millis(700));
    let after2 = count(&log);
    assert_eq!(
        mgr.session_running(&sid),
        Some(false),
        "额外权限会话应已终止"
    );
    assert_eq!(
        after1, after2,
        "setsid 守护进程必须在收尾后停写（整树回收）：{after1} vs {after2}"
    );
    assert!(
        after1 - before <= 12,
        "收尾后日志增量应有限（~0）：before={before} after1={after1}"
    );
    mgr.close();
    fs::remove_dir_all(&f.root).ok();
}
