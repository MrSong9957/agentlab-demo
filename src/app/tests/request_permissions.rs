//! 031 权限申请集成测试。
//!
//! M1：可信策略严格解析（重复键/未知字段/未知版本/矛盾规则/信任来源/自动
//! 敏感边界）、request_permissions 参数解析（§1.4 全部边界）、路径规范化
//!（相对路径、符号链接、`..`、编码原样、去重与 write 覆盖）、四字段结果
//! 结构与轮次授权集合（64 条上限、覆盖判定）。
//! 后续里程碑（M2 沙箱执行、M3 宿主、M4 生命周期）用例在本文件追加。

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

use serde_json::json;
use terminal_chat::permissions::{
    self, Access, Policy, TurnGrants, POLICY_ENV_VAR,
};

/// 夹具目录：每次调用唯一（并行测试互不删除；同路径 remove+create 会竞态）。
fn temp_dir(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "tchat-031-m1-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 在可信位置（非 anchor/temp 之下）写一份策略文件并返回路径。
fn write_policy(root: &Path, name: &str, text: &str) -> PathBuf {
    let dir = root.join("policies");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, text).unwrap();
    p
}

/// 合法策略（§1.3 示例形态；路径用夹具实际目录）。
fn valid_policy_json(anchor: &Path, requestable: &Path) -> String {
    format!(
        r#"{{
  "version": 1,
  "base": {{
    "file_system": {{ "entries": [{{ "path": {{ "type": "path", "path": {anchor:?} }}, "access": "write" }}] }},
    "network": {{ "enabled": false }}
  }},
  "requestable": {{
    "file_system": {{ "entries": [{{ "path": {{ "type": "path", "path": {requestable:?} }}, "access": "write" }}] }},
    "network": {{ "enabled": true }}
  }},
  "deny_paths": [{deny:?}]
}}"#,
        anchor = anchor,
        requestable = requestable,
        deny = anchor.join(".env"),
    )
}

/// 夹具：可信根（策略所在）、工作区 anchor、可申请目录。
struct Fixtures {
    trusted: PathBuf,
    anchor: PathBuf,
    requestable: PathBuf,
}

fn fixtures() -> Fixtures {
    let root = temp_dir("fixtures");
    let trusted = root.join("trusted");
    let anchor = root.join("workspace");
    let requestable = root.join("export");
    fs::create_dir_all(&trusted).unwrap();
    fs::create_dir_all(&anchor).unwrap();
    fs::create_dir_all(&requestable).unwrap();
    // deny 路径（anchor/.env）必须是已存在的实际路径（§1.3）
    fs::write(anchor.join(".env"), "FAKE-031-ANCHOR-ENV=1\n").unwrap();
    Fixtures { trusted, anchor, requestable }
}

fn env_lock() -> MutexGuard<'static, ()> {
    common::ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------- 策略：严格解析 ----------

#[test]
fn policy_valid_example_loads() {
    let f = fixtures();
    let policy = Policy::parse(&valid_policy_json(&f.anchor, &f.requestable)).unwrap();
    assert_eq!(policy.base.entries.len(), 1);
    assert_eq!(policy.base.entries[0].path, f.anchor.canonicalize().unwrap());
    assert_eq!(policy.base.entries[0].access, Access::Write);
    assert!(!policy.base.network);
    assert!(policy.requestable.network);
    assert_eq!(policy.deny_paths, vec![f.anchor.join(".env")]);
}

#[test]
fn policy_rejects_wrong_version() {
    let f = fixtures();
    let text = valid_policy_json(&f.anchor, &f.requestable).replace("\"version\": 1", "\"version\": 2");
    let err = Policy::parse(&text).unwrap_err();
    assert!(err.contains("版本"), "错误应提及版本：{err}");
}

#[test]
fn policy_rejects_duplicate_json_keys() {
    let f = fixtures();
    let text = valid_policy_json(&f.anchor, &f.requestable);
    // 顶层重复 version 键
    let dup = text.replace("{\n  \"version\": 1,", "{\n  \"version\": 1,\n  \"version\": 1,");
    assert!(Policy::parse(&dup).is_err());
    // file_system 层重复 entries 键
    let dup2 = text.replace(
        "\"file_system\": { \"entries\":",
        "\"file_system\": { \"entries\": [], \"entries\":",
    );
    assert!(Policy::parse(&dup2).is_err(), "嵌套重复键必须拒绝");
    // path spec 层重复 path 键
    let dup3 = text.replacen(
        "\"type\": \"path\", \"path\":",
        "\"type\": \"path\", \"path\": \"/x\", \"path\":",
        1,
    );
    assert!(Policy::parse(&dup3).is_err());
}

#[test]
fn policy_rejects_unknown_fields_each_level() {
    let f = fixtures();
    let base = valid_policy_json(&f.anchor, &f.requestable);
    // 顶层未知字段
    assert!(Policy::parse(&format!("{}\n", base.replace("{\n  \"version\": 1,", "{\n  \"extra\": 1,\n  \"version\": 1,"))).is_err());
    // scope 层未知字段（base 层）
    assert!(Policy::parse(&base.replace("\"base\": {", "\"base\": { \"unexpected\": true,")).is_err());
    // file_system 层未知字段
    assert!(Policy::parse(&base.replace("\"file_system\": { \"entries\"", "\"file_system\": { \"other\": 1, \"entries\"")).is_err());
    // entry 层未知字段（渲染后单花括号）
    assert!(Policy::parse(&base.replace("\"access\": \"write\" }", "\"access\": \"write\", \"mode\": 1 }")).is_err());
    // path spec 层未知字段
    assert!(Policy::parse(&base.replace("\"type\": \"path\", \"path\":", "\"type\": \"path\", \"raw\": \"x\", \"path\":")).is_err());
}

#[test]
fn policy_rejects_missing_required_sections() {
    let f = fixtures();
    let full = valid_policy_json(&f.anchor, &f.requestable);
    assert!(Policy::parse(&full.replace("\"requestable\": {", "\"requestable_x\": {")).is_err());
    assert!(Policy::parse(&full.replace("\"base\": {", "\"base_x\": {")).is_err());
    // network 必填（bool 语义由对象表达，缺省＝未定义）
    assert!(Policy::parse(&full.replace(",\n    \"network\": { \"enabled\": false }", "")).is_err());
}

#[test]
fn policy_paths_must_be_absolute_and_existing() {
    let f = fixtures();
    // 相对路径
    let rel = valid_policy_json(&f.anchor, &f.requestable)
        .replace(&format!("{:?}", f.anchor), "\"workspace-rel\"");
    assert!(Policy::parse(&rel).unwrap_err().contains("绝对"));
    // 不存在的路径
    let missing = valid_policy_json(&f.anchor, &f.requestable)
        .replace(&format!("{:?}", f.requestable), &format!("{:?}", f.trusted.join("no-such-dir")));
    assert!(Policy::parse(&missing).unwrap_err().contains("不存在"));
}

#[test]
fn policy_rejects_contradictory_rules_and_dedups_identical() {
    let f = fixtures();
    let text = format!(
        r#"{{
  "version": 1,
  "base": {{
    "file_system": {{ "entries": [
      {{ "path": {{ "type": "path", "path": {:?} }}, "access": "read" }},
      {{ "path": {{ "type": "path", "path": {:?} }}, "access": "write" }}
    ] }},
    "network": {{ "enabled": false }}
  }},
  "requestable": {{ "file_system": {{ "entries": [] }}, "network": {{ "enabled": false }} }},
  "deny_paths": []
}}"#,
        f.anchor, f.anchor,
    );
    assert!(Policy::parse(&text).unwrap_err().contains("矛盾"));
    // 完全相同的重复规则：规范化去重（非矛盾）——渲染后为单花括号
    let text2 = text.replace("\"access\": \"read\" },", "\"access\": \"write\" },");
    let policy = Policy::parse(&text2).unwrap();
    assert_eq!(policy.base.entries.len(), 1, "相同规则应去重");
}

#[test]
fn policy_rejects_non_path_type_and_bad_access() {
    let f = fixtures();
    let text = valid_policy_json(&f.anchor, &f.requestable)
        .replace("\"type\": \"path\"", "\"type\": \"glob\"");
    assert!(Policy::parse(&text).unwrap_err().contains("type"));
    let text2 = valid_policy_json(&f.anchor, &f.requestable)
        .replace("\"access\": \"write\"", "\"access\": \"admin\"");
    assert!(Policy::parse(&text2).is_err());
}

#[test]
fn policy_rejects_deny_outside_policy_semantics() {
    let f = fixtures();
    // deny_paths 支持目录；deny 不能是相对路径
    let text = valid_policy_json(&f.anchor, &f.requestable)
        .replace(&format!("{:?}", f.anchor.join(".env")), "\"relative-deny\"");
    assert!(Policy::parse(&text).unwrap_err().contains("绝对"));
}

// ---------- 策略：信任来源 ----------

#[test]
fn policy_load_rejects_workspace_and_temp_locations() {
    let f = fixtures();
    let text = valid_policy_json(&f.anchor, &f.requestable);
    // anchor（cwd 工作区）之下：拒绝
    let in_ws = f.anchor.join("policy.json");
    fs::write(&in_ws, &text).unwrap();
    let err = Policy::load_verified(&in_ws, &f.anchor, &std::env::temp_dir()).unwrap_err();
    assert!(err.contains("工作区"), "应拒绝工作区来源：{err}");
    // 系统临时目录之下：拒绝
    let tmp = temp_dir("untrusted");
    let in_tmp = tmp.join("policy.json");
    fs::write(&in_tmp, &text).unwrap();
    let err = Policy::load_verified(&in_tmp, &f.anchor, &tmp).unwrap_err();
    assert!(err.contains("临时"), "应拒绝临时目录来源：{err}");
}

#[test]
fn policy_load_rejects_self_write_reference() {
    let f = fixtures();
    // 传不含夹具的 temp root，隔离出「自引用」检查（夹具在 /tmp 之下会先命中
    // 临时目录拒绝——那由专门用例覆盖）
    let fake_temp = f.trusted.join("fake-tmp-unused");
    // 策略放在 requestable 写面之下：自引用拒绝
    let self_dir = f.requestable.join("self");
    fs::create_dir_all(&self_dir).unwrap();
    let p = self_dir.join("policy.json");
    fs::write(&p, valid_policy_json(&f.anchor, &f.requestable)).unwrap();
    let err = Policy::load_verified(&p, &f.anchor, &fake_temp).unwrap_err();
    assert!(err.contains("可写"), "应拒绝自引用（可写来源）：{err}");
}

#[test]
fn policy_load_rejects_oversize_file() {
    let f = fixtures();
    // 1 MiB + 若干字节的合法 JSON（超限即拒，不截断后解析）
    let mut big = "{\"padding\":\"".to_string();
    big.push_str(&"x".repeat(1024 * 1024));
    big.push_str("\"}");
    assert!(big.len() > 1024 * 1024);
    let p = write_policy(&f.trusted, "big.json", &big);
    let err = Policy::load_verified(&p, &f.anchor, &std::env::temp_dir()).unwrap_err();
    assert!(err.contains("1 MiB") || err.contains("上限"), "应拒绝超限策略：{err}");
}

#[test]
fn policy_load_accepts_trusted_location() {
    let f = fixtures();
    let p = write_policy(&f.trusted, "ok.json", &valid_policy_json(&f.anchor, &f.requestable));
    // temp root 传不含夹具的目录：可信位置正例不被临时目录规则误伤
    let fake_temp = f.trusted.join("fake-tmp-unused");
    let policy = Policy::load_verified(&p, &f.anchor, &fake_temp).unwrap();
    assert_eq!(policy.base.entries[0].path, f.anchor.canonicalize().unwrap());
}

#[test]
fn policy_effective_deny_auto_merges_sensitive_boundaries() {
    let f = fixtures();
    let config_dir = f.trusted.join("config");
    let user_dir = f.trusted.join("user");
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(&user_dir).unwrap();
    let install_dir = f.trusted.join("install");
    fs::create_dir_all(&install_dir).unwrap();
    let policy = Policy::parse(&valid_policy_json(&f.anchor, &f.requestable)).unwrap();
    let deny = policy.effective_deny_paths(&permissions::DenyContext {
        anchor: f.anchor.clone(),
        config_dir: config_dir.clone(),
        user_dir: user_dir.clone(),
        install_dirs: vec![f.trusted.clone(), install_dir.clone()],
    });
    // 显式 deny 保留
    assert!(deny.contains(&f.anchor.join(".env")));
    // 自动边界：.env、配置目录、用户目录（记忆/会话私有根）、策略与安装目录
    assert!(deny.contains(&f.anchor.join(".env")));
    assert!(deny.contains(&config_dir.canonicalize().unwrap()));
    assert!(deny.contains(&user_dir.canonicalize().unwrap()));
    assert!(deny.contains(&f.trusted.canonicalize().unwrap()));
    assert!(deny.contains(&install_dir.canonicalize().unwrap()));
    // 去重
    assert_eq!(deny.len(), dedup_count(&deny), "deny 集合应去重");
}

fn dedup_count(paths: &[PathBuf]) -> usize {
    let mut sorted = paths.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted.len()
}

// ---------- 请求：参数解析（§1.4） ----------

const VALID_REQUEST: &str = r#"{
  "reason": "需要将导出文件写入目标目录。",
  "permissions": {
    "file_system": {
      "entries": [{"path": {"type": "path", "path": "/vault/export"}, "access": "write"}]
    }
  }
}"#;

#[test]
fn request_valid_example_parses() {
    let req = permissions::parse_request(VALID_REQUEST).unwrap();
    assert_eq!(req.reason.as_deref(), Some("需要将导出文件写入目标目录。"));
    assert_eq!(req.entries.len(), 1);
    assert_eq!(req.entries[0].path, "/vault/export");
    assert_eq!(req.entries[0].access, Access::Write);
    assert!(!req.network);
}

#[test]
fn request_rejects_unknown_fields_each_level() {
    // 顶层
    assert!(permissions::parse_request(&VALID_REQUEST.replace("{\n  \"reason\"", "{\n  \"environment_id\": \"e1\",\n  \"reason\"")).is_err());
    // permissions 层
    assert!(permissions::parse_request(&VALID_REQUEST.replace("\"permissions\": {", "\"permissions\": { \"scope\": \"session\",")).is_err());
    // file_system 层
    assert!(permissions::parse_request(&VALID_REQUEST.replace("\"file_system\": {", "\"file_system\": { \"deny\": [],")).is_err());
    // entry 层（注意无空格：`"write"}]`）
    assert!(permissions::parse_request(&VALID_REQUEST.replace("\"access\": \"write\"}]", "\"access\": \"write\", \"unrestricted\": true}]")).is_err());
    // network 层
    let with_net = VALID_REQUEST.replace(
        "  }\n}",
        "  },\n  \"network\": { \"enabled\": true, \"hosts\": [\"x\"] }\n}",
    );
    assert!(permissions::parse_request(&with_net).is_err());
}

#[test]
fn request_rejects_array_and_scalar_forms() {
    assert!(permissions::parse_request("[\"permissions\"]").is_err());
    assert!(permissions::parse_request("\"permissions\"").is_err());
    assert!(permissions::parse_request("null").is_err());
    assert!(permissions::parse_request("{}").is_err());
}

#[test]
fn request_rejects_duplicate_keys() {
    let dup = VALID_REQUEST.replace(
        "{\n  \"reason\":",
        "{\n  \"reason\": \"a\",\n  \"reason\": \"b\",",
    );
    assert!(permissions::parse_request(&dup).is_err());
}

#[test]
fn request_requires_at_least_one_permission() {
    let empty = r#"{"permissions": {"file_system": {"entries": []}}}"#;
    let err = permissions::parse_request(empty).unwrap_err();
    assert!(err.contains("至少") || err.contains("一项"), "应要求至少一项：{err}");
    let no_perm = r#"{"reason": "only reason"}"#;
    assert!(permissions::parse_request(no_perm).is_err());
}

#[test]
fn request_entries_bounds_1_to_32() {
    let entry = r#"{"path": {"type": "path", "path": "/data/1"}, "access": "read"}"#;
    let entries32: Vec<String> = (1..=32)
        .map(|i| entry.replace("/data/1", &format!("/data/{i}")))
        .collect();
    let ok = format!(r#"{{"permissions": {{"file_system": {{"entries": [{}]}}}}}}"#, entries32.join(","));
    assert!(permissions::parse_request(&ok).is_ok());
    let entries33: Vec<String> = (1..=33)
        .map(|i| entry.replace("/data/1", &format!("/data/{i}")))
        .collect();
    let bad = format!(r#"{{"permissions": {{"file_system": {{"entries": [{}]}}}}}}"#, entries33.join(","));
    assert!(permissions::parse_request(&bad).unwrap_err().contains("32"));
}

#[test]
fn request_network_only_enabled_true() {
    let ok = r#"{"permissions": {"network": {"enabled": true}}}"#;
    assert!(permissions::parse_request(ok).unwrap().network);
    for bad in [
        r#"{"permissions": {"network": {"enabled": false}}}"#,
        r#"{"permissions": {"network": null}}"#,
        r#"{"permissions": {"network": true}}"#,
        r#"{"permissions": {"network": {}}}"#,
    ] {
        assert!(permissions::parse_request(bad).is_err(), "应拒绝：{bad}");
    }
}

#[test]
fn request_reason_validation() {
    assert!(permissions::parse_request(r#"{"reason": "", "permissions": {"network": {"enabled": true}}}"#).is_err());
    let long = "x".repeat(4097);
    let bad = format!(r#"{{"reason": "{long}", "permissions": {{"network": {{"enabled": true}}}}}}"#);
    assert!(permissions::parse_request(&bad).is_err());
    let exact = "y".repeat(4096);
    let ok = format!(r#"{{"reason": "{exact}", "permissions": {{"network": {{"enabled": true}}}}}}"#);
    assert!(permissions::parse_request(&ok).is_ok());
    // reason 省略合法
    assert!(permissions::parse_request(r#"{"permissions": {"network": {"enabled": true}}}"#).is_ok());
}

#[test]
fn request_arguments_size_limit() {
    let entry = r#"{"path": {"type": "path", "path": "/data/1"}, "access": "read"}"#;
    let entries: Vec<String> = (1..=32)
        .map(|i| entry.replace("/data/1", &format!("/data/{i}")))
        .collect();
    let mut big = format!(r#"{{"reason": "{}", "permissions": {{"file_system": {{"entries": [{}]}}}}}}"#, "r".repeat(30000), entries.join(","));
    while big.len() <= 64 * 1024 {
        big = big.replace("\"reason\": \"", &format!("\"reason\": \"{}", "p".repeat(1024)));
    }
    assert!(big.len() > 64 * 1024);
    let err = permissions::parse_request(&big).unwrap_err();
    assert!(err.contains("64") || err.contains("KiB"), "应拒绝超限参数：{err}");
}

#[test]
fn request_path_string_validation() {
    for bad in [
        "",                                       // 空
    ] {
        let text = format!(r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": "{bad}"}}, "access": "read"}}]}}}}}}"#);
        assert!(permissions::parse_request(&text).is_err(), "空路径应拒绝");
    }
    // NUL 与控制字符
    for ctl in ["\u{0}", "\u{1}", "\u{1f}", "\u{7f}"] {
        let text = format!(r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": "/a{ctl}b"}}, "access": "read"}}]}}}}}}"#);
        assert!(permissions::parse_request(&text).is_err(), "控制字符 {ctl:?} 应拒绝");
    }
    // 4097 字节路径
    let long = "/".to_string() + &"字".repeat(1366); // 3*1366=4098 字节
    assert!(long.len() > 4096);
    let text = format!(r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": "{long}"}}, "access": "read"}}]}}}}}}"#);
    assert!(permissions::parse_request(&text).is_err());
    // 中文/空格/%XX 原样接受（存在性在规范化层检查）
    let ok = format!(r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": "/tmp/中 文%41"}}, "access": "read"}}]}}}}}}"#);
    let req = permissions::parse_request(&ok).unwrap();
    assert_eq!(req.entries[0].path, "/tmp/中 文%41");
}

#[test]
fn request_access_only_read_write() {
    for bad in ["READ", "rw", "admin", ""] {
        let text = format!(r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": "/a"}}, "access": "{bad}"}}]}}}}}}"#);
        assert!(permissions::parse_request(&text).is_err(), "access={bad:?} 应拒绝");
    }
}

// ---------- 路径规范化（§1.4） ----------

#[test]
fn normalize_resolves_relative_against_session_cwd() {
    let f = fixtures();
    let target = f.requestable.join("rel-dir");
    fs::create_dir_all(&target).unwrap();
    let req = permissions::parse_request(
        r#"{"permissions": {"file_system": {"entries": [{"path": {"type": "path", "path": "export/rel-dir"}, "access": "read"}]}}}"#,
    )
    .unwrap();
    // 会话 cwd＝夹具根（不是测试进程 cwd），相对路径基于它解析
    let root = f.anchor.parent().unwrap().to_path_buf();
    let norm = permissions::normalize_request(&req, &root).unwrap();
    assert_eq!(norm.entries[0].path, target.canonicalize().unwrap());
}

#[test]
fn normalize_rejects_symlink_component() {
    let f = fixtures();
    let real = f.requestable.join("real");
    fs::create_dir_all(&real).unwrap();
    let link = f.requestable.join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let req = permissions::parse_request(&format!(
        r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "read"}}]}}}}}}"#,
        link,
    ))
    .unwrap();
    let root = f.anchor.parent().unwrap().to_path_buf();
    let err = permissions::normalize_request(&req, &root).unwrap_err();
    assert!(err.contains("符号链接"), "应拒绝符号链接：{err}");
}

#[test]
fn normalize_rejects_missing_and_accepts_dotdot() {
    let f = fixtures();
    let root = f.anchor.parent().unwrap().to_path_buf();
    // 不存在
    let req = permissions::parse_request(&format!(
        r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "read"}}]}}}}}}"#,
        f.requestable.join("missing"),
    ))
    .unwrap();
    assert!(permissions::normalize_request(&req, &root).unwrap_err().contains("不存在"));
    // .. 解析为实际路径（不自动扩父目录，逐组件存在）
    let req2 = permissions::parse_request(&format!(
        r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "read"}}]}}}}}}"#,
        f.requestable.join("sub").join(".."),
    ))
    .unwrap();
    let norm = permissions::normalize_request(&req2, &root).unwrap();
    assert_eq!(norm.entries[0].path, f.requestable.canonicalize().unwrap());
}

#[test]
fn normalize_dedups_and_write_covers_read() {
    let f = fixtures();
    let text = format!(
        r#"{{"permissions": {{"file_system": {{"entries": [
            {{"path": {{"type": "path", "path": {:?}}}, "access": "read"}},
            {{"path": {{"type": "path", "path": {:?}}}, "access": "write"}},
            {{"path": {{"type": "path", "path": {:?}}}, "access": "read"}}
        ]}}}}}}"#,
        f.requestable, f.requestable, f.requestable,
    );
    let req = permissions::parse_request(&text).unwrap();
    let norm = permissions::normalize_request(&req, f.requestable.parent().unwrap()).unwrap();
    assert_eq!(norm.entries.len(), 1);
    assert_eq!(norm.entries[0].access, Access::Write);
}

// ---------- 结果结构（§1.5） ----------

#[test]
fn result_json_shapes() {
    let f = fixtures();
    let req = permissions::parse_request(&format!(
        r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "write"}}]}}}}}}"#,
        f.requestable,
    ))
    .unwrap();
    let norm = permissions::normalize_request(&req, f.requestable.parent().unwrap()).unwrap();
    let granted: serde_json::Value = serde_json::from_str(&permissions::granted_result(&norm)).unwrap();
    assert_eq!(granted["status"], "granted");
    assert_eq!(granted["scope"], "turn");
    assert!(granted["error"].is_null());
    assert_eq!(granted["permissions"]["file_system"]["entries"][0]["access"], "write");

    let denied: serde_json::Value = serde_json::from_str(&permissions::denied_result()).unwrap();
    assert_eq!(denied["status"], "denied");
    assert_eq!(denied["permissions"], json!({}));
    assert_eq!(denied["scope"], "turn");
    assert!(denied["error"].is_null());

    let error: serde_json::Value =
        serde_json::from_str(&permissions::error_result("参数无效")).unwrap();
    assert_eq!(error["status"], "error");
    assert_eq!(error["permissions"], json!({}));
    assert_eq!(error["scope"], "turn");
    assert_eq!(error["error"], "参数无效");
}

// ---------- 轮次授权集合（§1.7） ----------

fn granted_request_for(path: &Path, access: &str) -> permissions::NormalizedRequest {
    let req = permissions::parse_request(&format!(
        r#"{{"permissions": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "{access}"}}]}}}}}}"#,
        path,
    ))
    .unwrap();
    permissions::normalize_request(&req, path.parent().unwrap()).unwrap()
}

#[test]
fn turn_grants_coverage_semantics() {
    let f = fixtures();
    let sub = f.requestable.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let mut grants = TurnGrants::new();
    assert!(!grants.covers(&granted_request_for(&f.requestable, "read")));

    grants.grant_all(&granted_request_for(&f.requestable, "write")).unwrap();
    // write 覆盖 read；祖先覆盖后代
    assert!(grants.covers(&granted_request_for(&f.requestable, "read")));
    assert!(grants.covers(&granted_request_for(&f.requestable, "write")));
    assert!(grants.covers(&granted_request_for(&sub, "write")));
    // 相似前缀不覆盖：/a 不授权 /ab
    let sibling = f.requestable.parent().unwrap().join(format!("{}x", f.requestable.file_name().unwrap().to_string_lossy()));
    fs::create_dir_all(&sibling).unwrap();
    assert!(!grants.covers(&granted_request_for(&sibling, "read")));
    // read 不覆盖 write
    let mut g2 = TurnGrants::new();
    g2.grant_all(&granted_request_for(&f.requestable, "read")).unwrap();
    assert!(!g2.covers(&granted_request_for(&f.requestable, "write")));
    // 网络覆盖
    let net_req = permissions::parse_request(r#"{"permissions": {"network": {"enabled": true}}}"#).unwrap();
    let net_norm = permissions::normalize_request(&net_req, &f.anchor).unwrap();
    assert!(!grants.covers(&net_norm));
    let mut g3 = TurnGrants::new();
    g3.grant_all(&net_norm).unwrap();
    assert!(g3.covers(&net_norm));
}

#[test]
fn turn_grants_limit_64_after_dedup() {
    let f = fixtures();
    let root = f.requestable.parent().unwrap();
    for i in 1..=65 {
        let p = f.requestable.join(format!("d{i}"));
        fs::create_dir_all(&p).unwrap();
    }
    let mut grants = TurnGrants::new();
    for i in 1..=64 {
        grants.grant_all(&granted_request_for(&f.requestable.join(format!("d{i}")), "read")).unwrap();
    }
    let err = grants.grant_all(&granted_request_for(&f.requestable.join("d65"), "read")).unwrap_err();
    assert!(err.contains("64"), "第 65 条应拒绝：{err}");
    // 重复授予去重不占额度
    grants.grant_all(&granted_request_for(&f.requestable.join("d1"), "read")).unwrap();
}

// ---------- 环境装配（§1.2） ----------

#[test]
fn policy_env_assembly_gates() {
    let _lock = env_lock();
    let f = fixtures();
    let p = write_policy(&f.trusted, "env.json", &valid_policy_json(&f.anchor, &f.requestable));
    // 生产入口的 temp root 取 std::env::temp_dir()（读 TMPDIR）：夹具在 /tmp
    // 之下会被临时目录规则命中——把 TMPDIR 指到不含夹具的目录隔离出装配语义
    let fake_tmp = f.trusted.join("fake-tmp");
    fs::create_dir_all(&fake_tmp).unwrap();
    let saved_tmp = std::env::var("TMPDIR").ok();
    std::env::set_var("TMPDIR", &fake_tmp);

    // 未设置：None（普通/033 模式不变）
    std::env::remove_var(POLICY_ENV_VAR);
    std::env::remove_var("TCHAT_SANDBOX");
    assert!(permissions::policy_from_env(&f.anchor).unwrap().is_none());

    // 设置但未启用 TCHAT_SANDBOX：明确失败
    std::env::set_var(POLICY_ENV_VAR, &p);
    let err = permissions::policy_from_env(&f.anchor).unwrap_err();
    assert!(err.contains("TCHAT_SANDBOX"), "缺开关应失败：{err}");

    // 设置且开关启用：加载成功
    std::env::set_var("TCHAT_SANDBOX", "1");
    let loaded = permissions::policy_from_env(&f.anchor).unwrap().expect("应加载").0;
    assert_eq!(loaded.base.entries.len(), 1);

    // .env 不能注入信任根：load_dotenv_from 跳过该变量
    let dotenv = f.trusted.join(".env");
    fs::write(&dotenv, format!("{POLICY_ENV_VAR}={}\nTCHAT_SANDBOX=0\n", p.display())).unwrap();
    std::env::remove_var(POLICY_ENV_VAR);
    std::env::remove_var("TCHAT_SANDBOX");
    let n = terminal_chat::catalog::load_dotenv_from(&dotenv);
    assert_eq!(n, 0, "信任根控制变量不应从 .env 加载");

    std::env::remove_var(POLICY_ENV_VAR);
    std::env::remove_var("TCHAT_SANDBOX");
    match saved_tmp {
        Some(v) => std::env::set_var("TMPDIR", v),
        None => std::env::remove_var("TMPDIR"),
    }
}

// ---------- 边界核查补充（2026-10-29 第二轮） ----------

/// §1.3 部署前提：二进制位于工作区/临时目录之下 → 拒绝启用 031（契约的
/// 显式执行——cargo target 产物是可被模型覆盖的工作区产物）。
#[test]
fn exe_deployment_prerequisite_refuses_workspace_and_temp() {
    let f = fixtures();
    let real = permissions::verify_exe_deployment(&f.anchor, &std::env::temp_dir().with_file_name("nonexistent-tmp"));
    // 测试进程的 current_exe 在 /workspace/target 下：不在夹具 anchor 之下 → 通过
    assert!(real.is_ok(), "真实测试二进制不在夹具工作区下应通过");
    // 构造 anchor＝二进制所在目录 → 拒绝
    let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let err = permissions::verify_exe_deployment(&exe_dir, Path::new("/nonexistent-tmp")).unwrap_err();
    assert!(err.contains("工作区"), "工作区产物应拒绝：{err}");
    // temp 之下 → 拒绝（temp root 覆盖二进制的实际位置。038-CI：原为容器专属
    // 字面量 /workspace——GH runner 的二进制在 /home/runner 下，两个拒绝分支
    // 均不命中；改为按进程实际位置锚定，三端等价）
    let err = permissions::verify_exe_deployment(Path::new("/nonexistent-anchor"), &exe_dir).unwrap_err();
    assert!(err.contains("临时目录"), "临时目录应拒绝：{err}");
}

/// 拓扑限制显式回归：内容路径位于自动敏感边界（用户私有根）之下 → 快照
/// 编码拒绝（deny 优先；S140 的分离布局是正例）。
#[test]
fn snapshot_refuses_content_under_auto_deny() {
    let f = fixtures();
    let user_root = f.trusted.join("user-root");
    let inside_user = user_root.join("proj");
    fs::create_dir_all(&inside_user).unwrap();
    let policy = Policy::parse(&format!(
        r#"{{"version": 1, "base": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "write"}}]}}, "network": {{"enabled": false}}}}, "requestable": {{"file_system": {{"entries": []}}, "network": {{"enabled": false}}}}, "deny_paths": []}}"#,
        inside_user,
    ))
    .unwrap();
    let ctx = permissions::DenyContext {
        anchor: f.anchor.clone(),
        config_dir: f.trusted.join("cfg"),
        user_dir: user_root.clone(),
        install_dirs: vec![],
    };
    let err = permissions::build_snapshot(&policy, &TurnGrants::new(), &ctx).unwrap_err();
    assert!(err.contains("永久拒绝"), "内容在用户根之下应拒绝：{err}");
}

/// §1.3 文义目录级 deny（运行时白名单之外的安装目录）保留：安装目录整体
/// 不可读（nlink fail-closed＋遮蔽都按目录进行）。
#[test]
fn install_dir_outside_runtime_keeps_directory_deny() {
    let f = fixtures();
    let install = f.trusted.join("opt-install");
    fs::create_dir_all(&install).unwrap();
    let policy = Policy::parse(&format!(
        r#"{{"version": 1, "base": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "write"}}]}}, "network": {{"enabled": false}}}}, "requestable": {{"file_system": {{"entries": []}}, "network": {{"enabled": false}}}}, "deny_paths": []}}"#,
        f.anchor,
    ))
    .unwrap();
    let ctx = permissions::DenyContext {
        anchor: f.anchor.clone(),
        config_dir: f.trusted.join("cfg"),
        user_dir: f.trusted.join("user-root"),
        install_dirs: vec![install.clone()],
    };
    let snap = permissions::build_snapshot(&policy, &TurnGrants::new(), &ctx).unwrap();
    assert!(
        snap.deny_paths.iter().any(|d| d == &install.canonicalize().unwrap()),
        "白名单外安装目录应保持目录级 deny"
    );
    // 该目录内部条目（若被误当内容面）同样被拒——目录级语义生效
    let mut nested_policy = policy.clone();
    nested_policy.base.entries[0].path = install.join("bin");
    let err = permissions::build_snapshot(&nested_policy, &TurnGrants::new(), &ctx).unwrap_err();
    assert!(err.contains("永久拒绝"), "安装目录内部不可成为内容面：{err}");
}

// ---------- 第三轮：审查 #15 回归＋P02 后端部署前提 ----------

/// 授权集合助手：一次提交全部条目（模拟§1.5「单次允许全部生效」）。
fn grants_of(items: &[(PathBuf, Access)]) -> TurnGrants {
    let mut g = TurnGrants::new();
    g.grant_all(&permissions::NormalizedRequest {
        reason: None,
        entries: items
            .iter()
            .map(|(p, a)| permissions::NormalizedEntry { path: p.clone(), access: *a })
            .collect(),
        network: false,
    })
    .unwrap();
    g
}

fn deny_ctx_of(f: &Fixtures) -> permissions::DenyContext {
    permissions::DenyContext {
        anchor: f.anchor.clone(),
        config_dir: f.trusted.join("cfg"),
        user_dir: f.trusted.join("user-root"),
        install_dirs: vec![],
    }
}

/// 审查 #15：read 严格位于 write 之下＝冗余授权（§1.4「write 包含读取」的
/// 单调模型），快照须化简——否则挂载深度升序使冗余 ro-bind 遮蔽祖先写面，
/// 已批准的父目录写权限在子树内静默 EROFS（§1.6 批准对象与挂载绑定）。
/// 覆盖：本轮先 read 后 write（审查反例形态）、base read＋本轮 write、
/// 同路径 read/write；反例对照：兄弟 read 不化简、write 嵌套于 read 保留。
#[test]
fn nested_read_under_write_reduced_in_snapshot() {
    let f = fixtures();
    let parent = f.requestable.canonicalize().unwrap();
    let child = parent.join("sub");
    fs::create_dir_all(&child).unwrap();
    let policy = Policy::parse(&valid_policy_json(&f.anchor, &f.requestable)).unwrap();
    let ctx = deny_ctx_of(&f);

    // ① 同一轮先授予 read 子目录、再授予 write 父目录（审查反例顺序）
    let snap = permissions::build_snapshot(
        &policy,
        &grants_of(&[(child.clone(), Access::Read), (parent.clone(), Access::Write)]),
        &ctx,
    )
    .unwrap();
    assert!(
        snap.read_paths.iter().all(|r| !r.starts_with(&parent)),
        "write 祖先之下的 read 应被化简：read={:?} write={:?}",
        snap.read_paths,
        snap.write_paths
    );
    assert!(snap.write_paths.contains(&parent), "父目录 write 保留");

    // ② base 含子目录 read＋本轮授予父目录 write（反例的另一形态）
    let base_read_policy = Policy::parse(&format!(
        r#"{{"version": 1, "base": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "write"}}, {{"path": {{"type": "path", "path": {:?}}}, "access": "read"}}]}}, "network": {{"enabled": false}}}}, "requestable": {{"file_system": {{"entries": [{{"path": {{"type": "path", "path": {:?}}}, "access": "write"}}]}}, "network": {{"enabled": false}}}}, "deny_paths": []}}"#,
        f.anchor.canonicalize().unwrap(),
        child,
        parent,
    ))
    .unwrap();
    let snap = permissions::build_snapshot(
        &base_read_policy,
        &grants_of(&[(parent.clone(), Access::Write)]),
        &ctx,
    )
    .unwrap();
    assert!(
        snap.read_paths.iter().all(|r| !r.starts_with(&parent)),
        "base read 位于本轮 write 之下同样化简：read={:?}",
        snap.read_paths
    );

    // ③ 同路径先 read 后 write：read 被 write 覆盖（§1.4 同一路径 write 覆盖 read）
    let snap = permissions::build_snapshot(
        &policy,
        &grants_of(&[(parent.clone(), Access::Read), (parent.clone(), Access::Write)]),
        &ctx,
    )
    .unwrap();
    assert!(!snap.read_paths.contains(&parent), "同路径 read 应被 write 覆盖");

    // ④ 对照：write 嵌套于 read 之下（正常「只读区开写窗」形态）不化简；
    // 兄弟 read（requestable 内、不在任何 write 之下）同样保留
    let sibling = f.requestable.join("sib-ro");
    fs::create_dir_all(&sibling).unwrap();
    let snap = permissions::build_snapshot(
        &policy,
        &grants_of(&[
            (sibling.canonicalize().unwrap(), Access::Read),
            (child.clone(), Access::Write),
        ]),
        &ctx,
    )
    .unwrap();
    assert_eq!(
        snap.read_paths,
        vec![sibling.canonicalize().unwrap()],
        "兄弟 read 保留、无过度化简"
    );
    assert!(snap.write_paths.contains(&child), "子目录 write 保留");
}

/// §1.3/P02：沙箱后端（bwrap）缺失或位于模型可写面 → 拒绝启用 031（启动期
/// 明确失败，零模型命令执行；不回退普通/033）。
#[test]
fn bwrap_backend_deployment_refuses_missing_or_model_writable() {
    let f = fixtures();
    // 缺后端：None → 启动期明确失败
    let err = permissions::verify_bwrap_backend(None, &f.anchor, &std::env::temp_dir()).unwrap_err();
    assert!(err.contains("不可用"), "缺后端应明确失败：{err}");
    // 后端在工作区（模型可写面）之下 → 拒绝
    let ws_bwrap = f.anchor.join("tools/bwrap");
    fs::create_dir_all(ws_bwrap.parent().unwrap()).unwrap();
    fs::write(&ws_bwrap, b"#!/bin/sh\n").unwrap();
    let err =
        permissions::verify_bwrap_backend(Some(&ws_bwrap), &f.anchor, Path::new("/nonexistent-tmp"))
            .unwrap_err();
    assert!(err.contains("工作区"), "工作区后端应拒绝：{err}");
    // 后端在系统临时目录之下 → 拒绝
    let tmp_bwrap = std::env::temp_dir().join("tchat-031-fake-bwrap");
    fs::write(&tmp_bwrap, b"#!/bin/sh\n").unwrap();
    let err = permissions::verify_bwrap_backend(
        Some(&tmp_bwrap),
        Path::new("/nonexistent-anchor"),
        &std::env::temp_dir(),
    )
    .unwrap_err();
    assert!(err.contains("临时目录"), "临时目录后端应拒绝：{err}");
    let _ = fs::remove_file(&tmp_bwrap);
    // 固定安装位置（/usr 树内）→ 通过（容器实际部署形态）
    if let Some(real) = terminal_chat::sandbox::launcher_path() {
        assert!(
            permissions::verify_bwrap_backend(Some(&real), &f.anchor, &std::env::temp_dir()).is_ok(),
            "固定安装位置的后端应通过：{}",
            real.display()
        );
    }
}
