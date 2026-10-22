//! 集成测试：通过本地 mock HTTP 服务驱动真实业务代码，
//! 检查发送的 JSON、请求次数与目录变化。配置一律使用独立临时目录。

mod common;

use common::{spawn_mock, MockResp, TempDir, ENV_LOCK};
use serde_json::json;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use terminal_chat::{api, catalog};

// ---------- 辅助 ----------

struct EnvGuard<'a> {
    _lock: std::sync::MutexGuard<'a, ()>,
    user_dir: std::path::PathBuf,
    /// 本测试的公共目录地址：refresh 测试注入用（正式路径不注入，强制 HTTPS）
    public_url: String,
}

fn set_env(lock: &'static Mutex<()>, dir: &Path, public_url: &str) -> EnvGuard<'static> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static USER_DIR_SEQ: AtomicUsize = AtomicUsize::new(0);
    let g = lock.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("TCHAT_CONFIG_DIR", dir);
    std::env::set_var("TCHAT_PUBLIC_CATALOG_URL", public_url);
    // 用户私有目录（专用凭证目录的父目录）：独立可写临时目录，与配置目录解耦
    let seq = USER_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let user_dir = std::env::temp_dir().join(format!("tchat-user-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&user_dir).unwrap();
    std::env::set_var("TCHAT_USER_DIR", &user_dir);
    EnvGuard { _lock: g, user_dir, public_url: public_url.to_string() }
}

impl Drop for EnvGuard<'_> {
    fn drop(&mut self) {
        std::env::remove_var("TCHAT_CONFIG_DIR");
        std::env::remove_var("TCHAT_PUBLIC_CATALOG_URL");
        std::env::remove_var("TCHAT_USER_DIR");
        let _ = std::fs::remove_dir_all(&self.user_dir);
    }
}

fn public_catalog_mock() -> common::Mock {
    spawn_mock(vec![MockResp::json(
        200,
        json!({
            "deepseek": {
                "models": {
                    "deepseek-flash": {
                        "name": "DeepSeek V4.1 Flash",
                        "limit": { "context": 1048576, "output": 393216 }
                    }
                }
            }
        })
        .to_string(),
    )])
}

fn manual_config(mock_url: &str, key_ref: &str) -> catalog::ManualRoot {
    catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(mock_url.into()),
                key_ref: Some(key_ref.into()),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    }
}

fn write_manual(dir: &Path, m: &catalog::ManualRoot) {
    std::fs::write(
        dir.join("manual.json"),
        serde_json::to_string_pretty(m).unwrap(),
    )
    .unwrap();
}

fn write_auto(dir: &Path, a: &catalog::AutoCatalog) {
    std::fs::write(
        dir.join("auto_catalog.json"),
        serde_json::to_string_pretty(a).unwrap(),
    )
    .unwrap();
}

fn read_auto(dir: &Path) -> catalog::AutoCatalog {
    serde_json::from_str(&std::fs::read_to_string(dir.join("auto_catalog.json")).unwrap())
        .unwrap()
}

fn auto_with_old_query(mock_url: &str, key: &str) -> catalog::AutoCatalog {
    let mut auto = catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() };
    auto.providers.get_mut("deepseek").unwrap().query = Some(catalog::QueryList {
        updated_at: 1,
        identity: catalog::Identity {
            base_url: mock_url.into(),
            protocol: "openai-chat".into(),
            cred_name: "test_cred".into(),
            cred_fp: catalog::fingerprint(key),
        },
        models: vec![catalog::ModelInfo {
            id: "only-from-query".into(),
            ..Default::default()
        }],
    });
    auto
}

fn list_json(ids: &[&str], has_more: bool) -> String {
    json!({
        "object": "list",
        "data": ids.iter().map(|i| json!({
            "id": i,
            "name": format!("Name {i}"),
            "context_window": 1024u64,
            "max_output_tokens": 256u64
        })).collect::<Vec<_>>(),
        "has_more": has_more
    })
    .to_string()
}

const CHAT_OK: &str = r#"{"choices":[{"message":{"content":"收到"},"finish_reason":"stop"}]}"#;

// ---------- 对话请求 ----------

#[test]
fn chat_sends_expected_json_and_parses_reply() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "test-model".into(), ..Default::default() };
    let agent = api::agent();
    let reply = api::chat(&conn, "secret-key", &model, "你好", &agent).unwrap();
    assert_eq!(reply, "收到");
    let reqs = mock.requests();
    // 无重试机制：一次调用只发一次请求
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].path, "/chat/completions");
    assert_eq!(
        reqs[0].auth.as_deref().map(|s| s.to_ascii_lowercase()),
        Some("bearer secret-key".to_string())
    );
    let body: serde_json::Value = serde_json::from_str(&reqs[0].body).unwrap();
    // 检查实际发送的 JSON：只有模型和消息，不自动添加可选参数
    let map = body.as_object().unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "你好");
}

#[test]
fn auth_failure_is_reported_clearly() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(
        401,
        r#"{"error":{"message":"Incorrect API key"}}"#,
    )]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "bad-key", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("鉴权失败"), "实际错误：{err}");
    assert!(err.contains("Incorrect API key"));
    assert_eq!(mock.count(), 1);
}

#[test]
fn unsupported_protocol_rejected_before_request() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "anthropic-messages".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("anthropic-messages 未支持"), "实际错误：{err}");
    assert_eq!(mock.count(), 0);
}

// ---------- 参数能力 ----------

#[test]
fn confirmed_unsupported_param_blocks_before_request() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    let mut request_params = std::collections::BTreeMap::new();
    request_params.insert("temperature".to_string(), json!(0.5));
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params,
    };
    // 能力已确认：temperature 不在支持列表
    let model = catalog::ModelInfo {
        id: "m".into(),
        supported_params: Some(vec!["max_tokens".into()]),
        ..Default::default()
    };
    let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("已确认不被模型 m 支持"), "实际错误：{err}");
    assert_eq!(mock.count(), 0);
    // 能力未知：显式参数不放行
    let unknown = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "k", &unknown, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("能力未知"), "实际错误：{err}");
    assert_eq!(mock.count(), 0);
    // 已确认支持：参数随请求发送
    let ok = catalog::ModelInfo {
        id: "m".into(),
        supported_params: Some(vec!["temperature".into()]),
        ..Default::default()
    };
    api::chat(&conn, "k", &ok, "hi", &api::agent()).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(&mock.requests()[0].body).unwrap();
    assert_eq!(body["temperature"], 0.5);
}

// ---------- 模型列表分页 ----------

#[test]
fn list_models_reads_pagination_completely() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![
        MockResp::json(200, list_json(&["m1", "m2"], true)),
        MockResp::json(200, list_json(&["m3"], false)),
    ]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let models = api::list_models(&conn, "k", &api::agent()).unwrap();
    // 分页读完整：两页合计 3 个模型
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["m1", "m2", "m3"]);
    assert_eq!(models[0].display_name.as_deref(), Some("Name m1"));
    assert_eq!(models[0].context_length, Some(1024));
    assert_eq!(models[0].max_output, Some(256));
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].path, "/models");
    assert_eq!(reqs[1].path, "/models?after=m2");
}

#[test]
fn pagination_cursor_is_single_encoded_param() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    // F5 回归：游标来自服务端数据，含 & # 空格 中文时必须整体编码，
    // 不得拆散成多个查询参数或丢失片段
    let tricky = "a&b#c模式";
    let page1 = json!({
        "data": [{ "id": tricky, "name": "n" }],
        "has_more": true
    })
    .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, page1),
        MockResp::json(200, list_json(&["m3"], false)),
    ]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let models = api::list_models(&conn, "k", &api::agent()).unwrap();
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2);
    let query = reqs[1]
        .path
        .strip_prefix("/models?")
        .unwrap_or_else(|| panic!("第二页请求行不含查询串：{}", reqs[1].path));
    let (k, v) = query.split_once('=').expect("查询串应有 = 分隔");
    assert_eq!(k, "after");
    assert!(!v.contains('&'), "游标不得包含未编码的 & 分隔符：{v}");
    assert_eq!(
        percent_decode(v),
        tricky,
        "游标必须作为单个参数值无损传递"
    );
    // 业务结果正常：第一页 1 个 + 第二页 1 个
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec![tricky, "m3"]);
}

fn percent_decode(s: &str) -> String {
    // 兼容 form-urlencoded（空格可写作 +）与百分号编码两种形态
    let s = s.replace('+', " ");
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

// ---------- 响应体上限（F6） ----------

fn huge_chat_body() -> String {
    // 约 11 MiB 的合法 JSON 回复：修复前会被完整读入内存并成功返回
    format!(
        r#"{{"choices":[{{"message":{{"content":"{}"}},"finish_reason":"stop"}}]}}"#,
        "A".repeat(11 * 1024 * 1024)
    )
}

#[test]
fn chat_body_over_limit_aborts() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(200, huge_chat_body())]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("10 MiB"), "超限必须明确报错，实际：{err}");
}

#[test]
fn error_body_over_limit_reports_cap_with_status() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let big = format!(r#"{{"error":{{"message":"{}"}}}}"#, "C".repeat(11 * 1024 * 1024));
    let mock = spawn_mock(vec![MockResp::json(401, big)]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("10 MiB"), "超限必须明确报错，实际：{err}");
    assert!(err.contains("401"), "状态码分类信息必须保留，实际：{err}");
}

// ---------- 错误信息上限（F3） ----------

#[test]
fn json_error_message_capped_at_300_chars() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    // 320 个字符的 error.message：统一上限 300，超出部分不得回显
    let mock = spawn_mock(vec![MockResp::json(
        401,
        r#"{"error":{"message":"鉴权失败说明文字XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"}}"#,
    )]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("鉴权失败"), "状态分类保留：{err}");
    let x_count = err.chars().filter(|c| *c == 'X').count();
    assert!(
        x_count <= 300,
        "error.message 超过 300 字符的部分不得回显：{err}"
    );
    assert!(err.chars().count() < 340, "总长度应有界：{}", err.chars().count());
}

// ---------- 刷新流程 ----------

#[test]
fn refresh_success_replaces_old_list_and_keeps_selection() {
    let dir = TempDir::new("refresh-ok");
    let public = public_catalog_mock();
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("refresh-ok", "key-A");
    let mock = spawn_mock(vec![
        MockResp::json(200, list_json(&["m-a", "m-b"], false)),
        MockResp::json(200, list_json(&["m-c"], false)),
        MockResp::json(200, list_json(&[], false)),
    ]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let agent = api::agent();

    let report = catalog::refresh(&agent, &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(report.public_updated.contains(&"deepseek".to_string()));
    assert_eq!(report.queried, vec![("deepseek".to_string(), 2)]);
    // 单页名单（无 has_more）只发 1 次请求
    assert_eq!(mock.count(), 1);
    // 目录变化：名单替换，身份绑定查询时的地址与凭证
    let auto = read_auto(dir.path());
    let q = auto.providers["deepseek"].query.as_ref().unwrap();
    assert_eq!(q.models.len(), 2);
    assert_eq!(q.identity.base_url, mock.url);
    assert_eq!(q.identity.cred_fp, catalog::fingerprint("key-A"));
    // 基础资料来自公共目录同步
    assert_eq!(auto.providers["deepseek"].base.source, "public");
    assert_eq!(
        auto.providers["deepseek"].base.models["deepseek-flash"].display_name,
        Some("DeepSeek V4.1 Flash".into())
    );
    // 最终字典里能看到新名单，且默认模型未变
    let manual = catalog::load_manual(dir.path()).unwrap();
    let dict = catalog::merge_final(&manual, &auto);
    assert!(dict.providers["deepseek"].models.contains_key("m-a"));
    assert!(dict.providers["deepseek"].models.contains_key("m-b"));
    assert_eq!(dict.selected_model.as_deref(), Some("deepseek-flash"));

    // 第二次刷新：新名单替换旧名单
    catalog::refresh(&agent, &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    let auto2 = read_auto(dir.path());
    let ids: Vec<&str> = auto2.providers["deepseek"]
        .query
        .as_ref()
        .unwrap()
        .models
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, vec!["m-c"]);
    // 第三次刷新：合法空名单正确清空（原 refresh_with_valid_empty_list 场景并入）——
    // 不算失败、也不退回基础资料
    let report3 = catalog::refresh(&agent, &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(report3.failed.is_empty(), "空名单不算失败：{report3:?}");
    assert_eq!(report3.queried, vec![("deepseek".to_string(), 0)]);
    let auto3 = read_auto(dir.path());
    assert!(auto3.providers["deepseek"].query.as_ref().unwrap().models.is_empty());
    let manual3 = catalog::load_manual(dir.path()).unwrap();
    assert!(
        catalog::merge_final(&manual3, &auto3).providers["deepseek"].models.is_empty(),
        "空名单清空模型，不退回基础资料"
    );
    common::cleanup_cred_file("refresh-ok");
}

#[test]
fn refresh_failure_after_key_change_excludes_old_list() {
    let dir = TempDir::new("refresh-keyswap");
    let public = public_catalog_mock();
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    // 同名引用（同一凭证文件字段），实际 Key 已换成 key-B
    let key_ref = common::unique_cred_file("refresh-keyswap", "key-B");
    let mock = spawn_mock(vec![MockResp::json(401, r#"{"error":{"message":"bad key"}}"#)]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    // 旧名单绑定 key-A 时的身份
    write_auto(dir.path(), &auto_with_old_query(&mock.url, "key-A"));
    let agent = api::agent();

    let report = catalog::refresh(&agent, &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(report.failed.iter().any(|(id, r)| id == "deepseek" && r.contains("鉴权失败")));
    // 旧名单文件仍在，但身份不匹配 → 合并时排除，不套用
    let auto = read_auto(dir.path());
    let manual = catalog::load_manual(dir.path()).unwrap();
    let dict = catalog::merge_final(&manual, &auto);
    let p = &dict.providers["deepseek"];
    assert_eq!(p.identity.cred_fp, catalog::fingerprint("key-B"));
    assert!(!p.models.contains_key("only-from-query"));
    // 基础连接信息不受名单失效影响，基础资料模型兜底
    assert_eq!(p.conn.base_url, mock.url);
    assert!(p.models.contains_key("deepseek-flash"));
    common::cleanup_cred_file("refresh-keyswap");
}

/// 分页失败矩阵（合并原 missing_id_fails_whole_list / empty_page_with_has_more_fails）：
/// 任何一页畸形或失败 → 整单失败、保留旧名单、不写半份；每场景独立环境（目录/凭证/mock）。
#[test]
fn pagination_failure_keeps_old_list_intact() {
    fn scenario(name: &str, responses: Vec<MockResp>, want_requests: usize, want_err: &str) {
        let dir = TempDir::new(&format!("pagefail-{name}"));
        let public = public_catalog_mock();
        let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
        let key_ref = common::unique_cred_file(&format!("pagefail-{name}"), "key-A");
        let mock = spawn_mock(responses);
        write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
        write_auto(dir.path(), &auto_with_old_query(&mock.url, "key-A"));

        let report = catalog::refresh(&api::agent(), &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
        // 请求行为：恰好看过声明的页数才判失败
        assert_eq!(mock.count(), want_requests, "场景〔{name}〕请求数");
        assert!(
            report.failed.iter().any(|(id, r)| id == "deepseek" && r.contains(want_err)),
            "场景〔{name}〕错误分类：{report:?}"
        );
        // 磁盘状态：名单保持旧数据，未写入半份新名单
        let auto = read_auto(dir.path());
        let ids: Vec<&str> = auto.providers["deepseek"]
            .query
            .as_ref()
            .unwrap()
            .models
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, vec!["only-from-query"], "场景〔{name}〕旧名单保留");
        common::cleanup_cred_file(&format!("pagefail-{name}"));
    }
    // 第二页失败：证明确实读到了第二页才失败，不写入只有 m1 的半份名单
    scenario(
        "page2-fails",
        vec![
            MockResp::json(200, list_json(&["m1"], true)),
            MockResp::json(500, r#"{"error":{"message":"page 2 broke"}}"#),
        ],
        2,
        "page 2 broke",
    );
    // 条目缺 id：整单失败，名单不替换
    scenario(
        "missing-id",
        vec![MockResp::json(200, r#"{"data":[{"name":"no id here"},{"id":"m1"}]}"#)],
        1,
        "缺少 id",
    );
    // 空页却声明 has_more：分页异常，名单不替换
    scenario(
        "empty-page-has-more",
        vec![MockResp::json(200, r#"{"data":[],"has_more":true}"#)],
        1,
        "分页异常",
    );
}

// ---------- 快照隔离 ----------

#[test]
fn refresh_does_not_change_in_flight_request() {
    let dir = TempDir::new("snapshot");
    let public = public_catalog_mock();
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("snapshot", "key-A");
    // 慢响应对话 mock：让“对话进行中”真实存在
    let slow_chat = spawn_mock(vec![MockResp::slow_json(200, CHAT_OK, 900)]);
    let new_list = spawn_mock(vec![MockResp::json(200, list_json(&["m-new"], false))]);
    write_manual(dir.path(), &manual_config(&slow_chat.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let agent = api::agent();

    // 调用开始：固定本次使用的配置与已读取的凭证
    let dict = catalog::build_final().unwrap();
    let prov = dict.providers["deepseek"].clone();
    let cred = prov.credential.clone().unwrap();
    let model = prov.models["deepseek-flash"].clone();

    let handle = std::thread::spawn(move || {
        api::chat(&prov.conn, &cred.value, &model, "hi", &api::agent())
    });
    std::thread::sleep(std::time::Duration::from_millis(250));
    // 对话进行中：手动换地址并刷新目录
    let switched = manual_config(&new_list.url, &key_ref);
    write_manual(dir.path(), &switched);
    let report = catalog::refresh(&agent, &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert_eq!(report.queried, vec![("deepseek".to_string(), 1)]);
    // 进行中的请求仍用原配置完成，结果不受刷新影响
    let reply = handle.join().unwrap().unwrap();
    assert_eq!(reply, "收到");
    assert_eq!(slow_chat.count(), 1);
    assert_eq!(slow_chat.requests()[0].path, "/chat/completions");
    assert_eq!(new_list.count(), 1);
    assert_eq!(new_list.requests()[0].path, "/models");
    common::cleanup_cred_file("snapshot");
}

// ---------- 管道 E2E ----------

#[test]
fn bin_pipe_session_chats_and_lists_models() {
    let dir = TempDir::new("e2e-bin");
    let mock = spawn_mock(vec![MockResp::json(
        200,
        r#"{"choices":[{"message":{"content":"测试回复OK"},"finish_reason":"stop"}]}"#,
    )]);
    let manual = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(mock.url.clone()),
                key_ref: Some("env:TCHAT_E2E_KEY".into()),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    write_manual(dir.path(), &manual);
    // 自动目录不预置：由程序用内置资料初始化
    // F1：预先授权该 origin（模拟用户此前交互确认过），非交互子进程才可发送凭证
    let user = TempDir::new("e2e-user");
    std::fs::create_dir_all(user.path()).unwrap();
    let store = serde_json::json!({
        "grants": [{
            "origin": catalog::origin_of(&mock.url).unwrap(),
            "cred_fp": catalog::fingerprint("e2e-key"),
            "added_at": 0
        }]
    });
    let auth_file = user.path().join("authorized-origins.json");
    let auth_bytes_before = store.to_string();
    std::fs::write(&auth_file, auth_bytes_before.clone()).unwrap();

    use std::io::Write;
    use std::process::{Command, Stdio};
    // 015：cwd 落临时目录（无 .git 祖先）——上下文注入为环境块形态，
    // 测试不依赖仓库真实 AGENTS.md 内容
    let project = TempDir::new("e2e-bin-proj");
    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .current_dir(project.path())
        .env("TCHAT_CONFIG_DIR", dir.path())
        .env("TCHAT_USER_DIR", user.path())
        .env("TCHAT_E2E_KEY", "e2e-key")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动子进程失败");
    child
        .stdin
        .take()
        .unwrap()
        .write_all("你好\n/models\nexit\n".as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("等待子进程失败");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "退出码 {:?}，输出：{stdout}", out.status.code());
    // 真实业务代码完成对话：发送了请求、解析了回复
    assert!(stdout.contains("Agent：测试回复OK"), "实际输出：{stdout}");
    assert_eq!(mock.count(), 1);
    assert_eq!(mock.requests()[0].path, "/chat/completions");
    let body: serde_json::Value =
        serde_json::from_str(&mock.requests()[0].body).unwrap();
    assert_eq!(body["model"], "deepseek-flash");
    // 005/006 契约：请求带 system（Codex 母本适配版基础提示词）＋两项工具定义
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(
        body["messages"][0]["content"],
        json!(terminal_chat::prompt::sent_instructions())
    );
    // 015：上下文消息（环境块形态）恰在首轮输入之前
    assert_eq!(body["messages"][1]["role"], "user");
    let ctx = body["messages"][1]["content"].as_str().unwrap();
    assert!(ctx.starts_with("<environment_context>"), "ctx＝环境块：{ctx}");
    assert!(ctx.contains(&format!("<cwd>{}</cwd>", project.path().display())));
    assert!(ctx.contains("<current_date>"));
    assert_eq!(body["messages"][2]["role"], "user");
    assert_eq!(body["messages"][2]["content"], "你好");
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4, "017：四工具");
    assert_eq!(tools[0]["function"]["name"], "exec_command");
    assert_eq!(tools[1]["function"]["name"], "apply_patch");
    assert_eq!(tools[3]["function"]["name"], "update_plan");
    // C1（回归基线）：预授权路径的请求确实携带凭证；已授权命中不重写授权记录
    assert_eq!(
        mock.requests()[0].auth.as_deref(),
        Some("Bearer e2e-key"),
        "请求必须携带 Authorization 凭证头"
    );
    assert_eq!(
        std::fs::read(&auth_file).unwrap(),
        auth_bytes_before.as_bytes(),
        "已授权提前返回，不得改写授权记录"
    );
    // /models 展示内置资料模型与能力
    assert!(stdout.contains("deepseek-flash"), "实际输出：{stdout}");
    assert!(stdout.contains("上下文 1048576"), "实际输出：{stdout}");
    // 001 行为保持：结束提示与干净退出
    assert!(stdout.contains("再见!"));
    // 自动目录文件由程序初始化生成
    assert!(dir.path().join("auto_catalog.json").exists());
}

// ---------- 管道 E2E：授权门（回归基线，编号↔用例映射：C1–C3）----------
// C1 = bin_pipe_session_chats_and_lists_models（上方扩展断言）。
// 断言依据：main 以 stdin 是否终端判定交互；未授权非交互 fail-closed（001 §5
// 退出契约、F1 授权门）；授权按 (origin, 凭证指纹) 双匹配，换 Key 即失效。

/// C2：未授权 origin 的管道会话——零请求、零凭证外发、按契约提示并干净退出。
#[test]
fn bin_pipe_unauthorized_origin_rejects_without_request() {
    let dir = TempDir::new("c2-bin");
    // mock 挂正常响应：被调用即证明凭证外发失败
    let mock = spawn_mock(vec![MockResp::json(
        200,
        r#"{"choices":[{"message":{"content":"LEAK-MUST-NOT-APPEAR"},"finish_reason":"stop"}]}"#,
    )]);
    let manual = manual_config(&mock.url, "env:TCHAT_C_GATE_KEY");
    write_manual(dir.path(), &manual);
    let user = TempDir::new("c2-user"); // 不写授权记录：未授权状态
    let auth_file = user.path().join("authorized-origins.json");
    let origin = catalog::origin_of(&mock.url).unwrap();

    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .env("TCHAT_CONFIG_DIR", dir.path())
        .env("TCHAT_USER_DIR", user.path())
        .env("TCHAT_C_GATE_KEY", "c2-key-value")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动子进程失败");
    child
        .stdin
        .take()
        .unwrap()
        .write_all("你好\nexit\n".as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("等待子进程失败");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(mock.count(), 0, "未授权必须零请求，凭证零外发");
    assert_eq!(out.status.code(), Some(0), "退出码契约（001 §5）：{stdout}");
    assert!(stdout.contains("错误：凭证 TCHAT_C_GATE_KEY 未授权发送到"), "{stdout}");
    assert!(stdout.contains(&origin), "错误应含具体 origin：{stdout}");
    assert!(stdout.contains("非交互模式"), "{stdout}");
    assert!(stdout.contains("authorized-origins.json"), "错误应含授权记录路径：{stdout}");
    assert!(stdout.contains("再见!"), "错误后循环继续，exit 正常告别：{stdout}");
    assert!(!stdout.contains("Agent："), "不得出现模型回复：{stdout}");
    assert!(!stdout.contains("c2-key-value"), "密钥明文不得出现在输出：{stdout}");
    assert!(!auth_file.exists(), "非交互拒绝不得落盘授权记录");
}

/// C3：换 Key 后旧授权失效（origin 相同、指纹不同）——依旧零请求，授权记录不被改写。
#[test]
fn bin_pipe_rotated_key_invalidates_grant() {
    let dir = TempDir::new("c3-bin");
    let mock = spawn_mock(vec![MockResp::json(
        200,
        r#"{"choices":[{"message":{"content":"LEAK-MUST-NOT-APPEAR"},"finish_reason":"stop"}]}"#,
    )]);
    let manual = manual_config(&mock.url, "env:TCHAT_C_GATE_KEY");
    write_manual(dir.path(), &manual);
    // 预写旧 Key 的授权记录；子进程实际使用新 Key
    let user = TempDir::new("c3-user");
    let store = serde_json::json!({
        "grants": [{
            "origin": catalog::origin_of(&mock.url).unwrap(),
            "cred_fp": catalog::fingerprint("old-key"),
            "added_at": 0
        }]
    });
    let auth_file = user.path().join("authorized-origins.json");
    let bytes_before = store.to_string();
    std::fs::write(&auth_file, bytes_before.clone()).unwrap();

    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .env("TCHAT_CONFIG_DIR", dir.path())
        .env("TCHAT_USER_DIR", user.path())
        .env("TCHAT_C_GATE_KEY", "new-key-value")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动子进程失败");
    child
        .stdin
        .take()
        .unwrap()
        .write_all("你好\nexit\n".as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("等待子进程失败");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(mock.count(), 0, "换 Key 后旧授权失效，必须零请求");
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("错误：凭证 TCHAT_C_GATE_KEY 未授权发送到"), "{stdout}");
    assert!(stdout.contains("再见!"), "{stdout}");
    assert!(!stdout.contains("Agent："), "{stdout}");
    assert!(!stdout.contains("new-key-value"), "新密钥明文不得出现在输出：{stdout}");
    assert_eq!(
        std::fs::read(&auth_file).unwrap(),
        bytes_before.as_bytes(),
        "拒绝路径不得改写授权记录"
    );
}

// ---------- F1：凭证外发授权 ----------

#[test]
fn refresh_refuses_unauthorized_origin_without_request() {
    let dir = TempDir::new("refresh-unauth");
    let _env = set_env(&ENV_LOCK, dir.path(), "http://127.0.0.1:1/unused");
    let key_ref = common::unique_cred_file("unauth", "key-A");
    let mock = spawn_mock(vec![MockResp::json(200, list_json(&["m"], false))]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let report = catalog::refresh(
        &api::agent(),
        &mut |_origin, _name, _fp| {
            Err("凭证 test_cred 未授权发送到该地址：非交互模式拒绝".into())
        },
        &mut |_origin| Ok(()),
        None,
    )
    .unwrap();
    // 未授权厂商进入 failed、原因明确；没有发出任何带凭证的请求
    assert!(report.queried.is_empty(), "未授权不得查询：{:?}", report.queried);
    assert!(
        report
            .failed
            .iter()
            .any(|(id, r)| id == "deepseek" && r.contains("未授权")),
        "失败原因应说明未授权：{:?}",
        report.failed
    );
    assert_eq!(mock.count(), 0, "未授权时不得发出请求");
}

#[test]
fn redirect_targets_never_receive_credentials() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    // 攻击者主机：作为重定向目标
    let target = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    let mock = spawn_mock(vec![MockResp::redirect(format!(
        "{}/chat/completions",
        target.url
    ))]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "secret-key", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("重定向"), "实际错误：{err}");
    assert_eq!(target.count(), 0, "重定向目标不得收到任何请求");
    assert_eq!(mock.count(), 1, "只发一次请求，不跟随");
}

// ---------- F4：公共目录来源与字段校验 ----------

#[test]
fn public_sync_requires_authorization_and_validates_fields() {
    let dir = TempDir::new("pub-auth");
    let public = spawn_mock(vec![MockResp::json(
        200,
        json!({
            "deepseek": {"models": {
                "bad id": {"name": "坏"},
                "\u{1b}evil": {"name": "坏"},
                "good-model": {"name": "好模型\u{1b}]0;pwn\u{7}"}
            }}
        })
        .to_string(),
    )]);
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    write_manual(dir.path(), &manual_config("http://127.0.0.1:1", "env:TCHAT_NO_KEY"));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    // 目录来源未授权：同步被拒，旧目录保留（不写半份数据；URL 经测试注入）
    let report = catalog::refresh(
        &api::agent(),
        &mut |_o, _n, _f| Ok(()),
        &mut |_o| Err("公共目录来源未授权：非交互模式拒绝".into()),
        Some(&public.url),
    )
    .unwrap();
    let e = report.public_error.as_ref().unwrap();
    assert!(e.contains("未授权"), "实际：{e}");
    let auto = read_auto(dir.path());
    assert_eq!(auto.providers["deepseek"].base.source, "builtin", "同步失败保留旧目录");
    // 授权后：非法 id 拒绝入库，合法模型的展示名被清理控制序列
    let report = catalog::refresh(
        &api::agent(),
        &mut |_o, _n, _f| Ok(()),
        &mut |_o| Ok(()),
        Some(&public.url),
    )
    .unwrap();
    assert!(report.public_updated.contains(&"deepseek".to_string()));
    let auto = read_auto(dir.path());
    let models = &auto.providers["deepseek"].base.models;
    assert!(!models.contains_key("bad id"), "含空白的 id 不得入库");
    assert!(!models.contains_key("\u{1b}evil"), "含控制字符的 id 不得入库");
    // OSC 序列整个（含载荷 pwn）被剥离，只留正常文字
    assert_eq!(
        models["good-model"].display_name.as_deref(),
        Some("好模型"),
        "展示名应剥离转义序列"
    );
}

#[test]
fn production_catalog_path_rejects_plaintext_http_even_with_stale_switch() {
    let dir = TempDir::new("pub-prod-http");
    let public = spawn_mock(vec![MockResp::json(
        200,
        json!({"deepseek": {"models": {"evil-model": {"name": "evil"}}}}).to_string(),
    )]);
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    write_manual(dir.path(), &manual_config("http://127.0.0.1:1", "env:TCHAT_NO_KEY"));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    // 攻击者场景：正式路径（None 注入）+ 明文 http 目录 + 旧的开关环境变量仍然存在。
    // 开关已从产品代码移除：设置它也不得改变判定。
    std::env::set_var("TCHAT_PUBLIC_CATALOG_INSECURE_LOOPBACK", "1");
    let report = catalog::refresh(
        &api::agent(),
        &mut |_o, _n, _f| Ok(()),
        &mut |_o| Ok(()),
        None,
    )
    .unwrap();
    std::env::remove_var("TCHAT_PUBLIC_CATALOG_INSECURE_LOOPBACK");
    let e = report.public_error.as_ref().unwrap();
    assert!(e.contains("HTTPS"), "正式路径必须拒绝明文 http：{e}");
    assert_eq!(public.count(), 0, "明文 http 目录不得被访问");
    let auto = read_auto(dir.path());
    assert_eq!(auto.providers["deepseek"].base.source, "builtin", "旧目录保留");
    assert!(
        !auto.providers["deepseek"].base.models.contains_key("evil-model"),
        "明文目录数据不得入库"
    );
}

// ---------- 暴露用例：先钉住正确业务预期，再最小修复 ----------

#[test]
fn cursor_stall_fails_fast() {
    let dir = TempDir::new("cursor-stall");
    let public = public_catalog_mock();
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("cursor-stall", "key-A");
    // 每页返回相同 id 且 has_more=true：游标不前进
    let mock = spawn_mock(vec![
        MockResp::json(200, list_json(&["m1"], true)),
        MockResp::json(200, list_json(&["m1"], true)),
    ]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &auto_with_old_query(&mock.url, "key-A"));
    let report = catalog::refresh(&api::agent(), &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(
        report.failed.iter().any(|(id, r)| id == "deepseek" && r.contains("游标未前进")),
        "实际报告：{report:?}"
    );
    // 快速失败：只请求 2 次，不空转 100 次
    assert_eq!(mock.count(), 2);
    let auto = read_auto(dir.path());
    let ids: Vec<&str> = auto.providers["deepseek"]
        .query
        .as_ref()
        .unwrap()
        .models
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, vec!["only-from-query"]);
    common::cleanup_cred_file("cursor-stall");
}

// ---------- 六类清单补测（集成侧） ----------

#[test]
fn key_leak_scan_across_failures() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let victim = "sk-victim-SECRET-42";
    let fp = catalog::fingerprint(victim);
    let key_ref = common::unique_cred_file("leak-scan", victim);
    let mut messages: Vec<String> = Vec::new();
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let agent = api::agent();

    let conn_of = |url: &str| catalog::EffectiveConnection {
        base_url: url.to_string(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    // 401 / 500 / 拒绝连接 / 坏 JSON
    for (i, resp) in [
        MockResp::json(401, r#"{"error":{"message":"no"}}"#),
        MockResp::json(500, r#"{"error":{"message":"boom"}}"#),
        MockResp::json(200, "not json"),
    ]
    .into_iter()
    .enumerate()
    {
        let mock = spawn_mock(vec![resp]);
        if let Err(e) = api::chat(&conn_of(&mock.url), victim, &model, "hi", &agent) {
            messages.push(format!("场景{i}: {e}"));
        }
    }
    if let Err(e) = api::chat(&conn_of("http://127.0.0.1:1"), victim, &model, "hi", &agent) {
        messages.push(format!("拒绝连接: {e}"));
    }
    // 凭证文件缺失（错误含路径但不得含值）
    if let Some(e) = catalog::resolve_credential(&format!("file:{}#nope", "/nonexistent/dir/x.json")).error {
        messages.push(format!("凭证缺失: {e}"));
    }
    let _ = &key_ref;
    assert_eq!(messages.len(), 5, "五个失败场景必须各产生一条错误消息，否则泄露检查空跑");
    for m in &messages {
        assert!(!m.contains(victim), "泄露 Key：{m}");
        assert!(!m.contains(&fp), "泄露指纹：{m}");
    }
    common::cleanup_cred_file("leak-scan");
}

#[test]
fn timeout_no_response_keeps_list() {
    let dir = TempDir::new("timeout");
    let public = public_catalog_mock();
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("timeout", "key-A");
    // 已连接但不返回响应：客户端等待直至总超时
    let mock = spawn_mock(vec![MockResp::silent()]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &auto_with_old_query(&mock.url, "key-A"));
    let short = api::agent_with_timeouts(Duration::from_millis(300), Duration::from_millis(1200));
    let report = catalog::refresh(&short, &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert_eq!(mock.count(), 1);
    assert!(
        report.failed.iter().any(|(id, r)| id == "deepseek" && r.contains("网络错误")),
        "实际报告：{report:?}"
    );
    // 超时失败：旧名单保留
    let auto = read_auto(dir.path());
    let ids: Vec<&str> = auto.providers["deepseek"]
        .query
        .as_ref()
        .unwrap()
        .models
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(ids, vec!["only-from-query"]);
    common::cleanup_cred_file("timeout");
}

#[test]
fn disconnect_after_request_no_retry() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::close_now()]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("网络错误"), "实际错误：{err}");
    // 收到请求后断开：只尝试 1 次，不重试
    assert_eq!(mock.count(), 1);
}

#[test]
fn multi_turn_config_change_next_round() {
    let mock = spawn_mock(vec![
        MockResp::json(200, r#"{"choices":[{"message":{"content":"r1"},"finish_reason":"stop"}]}"#),
        MockResp::json(200, r#"{"choices":[{"message":{"content":"r2"},"finish_reason":"stop"}]}"#),
    ]);
    let dir = TempDir::new("multi-turn");
    let _env = set_env(&ENV_LOCK, dir.path(), "http://127.0.0.1:1/unused");
    let key_ref = common::unique_cred_file("multi-turn", "key-A");
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let agent = api::agent();

    // 第一轮：deepseek-flash
    let dict = catalog::build_final().unwrap();
    let (prov, cred, model) = catalog::route(&dict).unwrap();
    api::chat(&prov.conn, &cred.value, &model, "hi", &agent).unwrap();
    // 轮间换模型：下一轮使用新值
    let mut manual2 = manual_config(&mock.url, &key_ref);
    manual2.selected_model = Some("deepseek-v4-pro".into());
    write_manual(dir.path(), &manual2);
    let dict2 = catalog::build_final().unwrap();
    let (prov2, cred2, model2) = catalog::route(&dict2).unwrap();
    api::chat(&prov2.conn, &cred2.value, &model2, "hi", &agent).unwrap();

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&reqs[0].body).unwrap()["model"], "deepseek-flash");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&reqs[1].body).unwrap()["model"], "deepseek-v4-pro");
    common::cleanup_cred_file("multi-turn");
}

#[test]
fn reserved_params_rejected_before_request() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    let mut request_params = std::collections::BTreeMap::new();
    request_params.insert("model".to_string(), json!("evil"));
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params,
    };
    let model = catalog::ModelInfo {
        id: "m".into(),
        supported_params: Some(vec!["model".into()]),
        ..Default::default()
    };
    let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
    assert!(err.contains("保留字段"), "实际错误：{err}");
    assert_eq!(mock.count(), 0);
}

#[test]
fn response_edge_branches() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![
        MockResp::json(200, r#"{"choices":[{"message":{"content":"","reasoning_content":"thinking"},"finish_reason":"length"}]}"#),
        MockResp::json(200, r#"{"choices":[{"message":{"content":""},"finish_reason":"stop"}]}"#),
        MockResp::json(200, "not json"),
    ]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let agent = api::agent();
    let err = api::chat(&conn, "k", &model, "hi", &agent).unwrap_err();
    assert!(err.contains("思考"), "实际错误：{err}");
    let err = api::chat(&conn, "k", &model, "hi", &agent).unwrap_err();
    assert!(err.contains("回复为空"), "实际错误：{err}");
    let err = api::chat(&conn, "k", &model, "hi", &agent).unwrap_err();
    assert!(err.contains("解析失败"), "实际错误：{err}");
}

#[test]
fn base_url_slash_and_v1_paths() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![
        MockResp::json(200, CHAT_OK),
        MockResp::json(200, CHAT_OK),
    ]);
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let agent = api::agent();
    // 末尾斜杠不产生 //chat/completions
    let conn1 = catalog::EffectiveConnection {
        base_url: format!("{}/", mock.url),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    api::chat(&conn1, "k", &model, "hi", &agent).unwrap();
    // 带版本路径的基础地址
    let conn2 = catalog::EffectiveConnection {
        base_url: format!("{}/v1", mock.url),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    api::chat(&conn2, "k", &model, "hi", &agent).unwrap();
    assert_eq!(mock.requests()[0].path, "/chat/completions");
    assert_eq!(mock.requests()[1].path, "/v1/chat/completions");
}

#[test]
fn map_key_authority_in_request() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    let key_ref = common::unique_cred_file("map-key", "key-A");
    let mut manual = manual_config(&mock.url, &key_ref);
    // 手动新增模型：map 键 manual-id，记录内 id 写成 other —— 以键为权威
    manual.providers.get_mut("deepseek").unwrap().models.insert(
        "manual-id".to_string(),
        catalog::ModelInfo { id: "other".into(), ..Default::default() },
    );
    manual.selected_model = Some("manual-id".into());
    let dict = catalog::merge_final(&manual, &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let (prov, cred, model) = catalog::route(&dict).unwrap();
    assert_eq!(model.id, "manual-id");
    api::chat(&prov.conn, &cred.value, &model, "hi", &api::agent()).unwrap();
    let body: serde_json::Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    assert_eq!(body["model"], "manual-id");
    common::cleanup_cred_file("map-key");
}

#[test]
fn manual_vendor_full_custom_end_to_end() {
    let dir = TempDir::new("manual-vendor");
    let public = public_catalog_mock();
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("manual-vendor", "key-V");
    let mock = spawn_mock(vec![
        MockResp::json(200, list_json(&["m1"], false)),
        MockResp::json(200, r#"{"choices":[{"message":{"content":"vendor-ok"},"finish_reason":"stop"}]}"#),
    ]);
    // 全新厂商：builtin/auto 均无，全部信息来自手动。
    // deepseek 指向未设置的环境变量使其跳过查询，避免消耗 mock 响应序
    let mut manual = manual_config(&mock.url, "env:TCHAT_VENDOR_UNSET_KEY");
    manual.selected_provider = Some("localmock".into());
    manual.selected_model = Some("m1".into());
    manual.providers.insert(
        "localmock".to_string(),
        catalog::ManualProvider {
            base_url: Some(mock.url.clone()),
            protocol: Some("openai-chat".into()),
            key_ref: Some(key_ref.clone()),
            ..Default::default()
        },
    );
    write_manual(dir.path(), &manual);
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let agent = api::agent();

    let report = catalog::refresh(&agent, &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(report.queried.iter().any(|(id, n)| id == "localmock" && *n == 1));
    // 刷新后能对话
    let dict = catalog::build_final().unwrap();
    let (prov, cred, model) = catalog::route(&dict).unwrap();
    assert_eq!(prov.provider_id, "localmock");
    let reply = api::chat(&prov.conn, &cred.value, &model, "hi", &agent).unwrap();
    assert_eq!(reply, "vendor-ok");
    let body: serde_json::Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    assert_eq!(body["model"], "m1");
    common::cleanup_cred_file("manual-vendor");
}

#[test]
fn public_missing_provider_keeps_base() {
    let dir = TempDir::new("public-missing");
    // 公共目录不含 deepseek
    let public = spawn_mock(vec![MockResp::json(200, r#"{"other": {"models": {}}}"#)]);
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("public-missing", "key-A");
    let mock = spawn_mock(vec![MockResp::json(200, list_json(&["m1"], false))]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let report = catalog::refresh(&api::agent(), &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(report.public_updated.is_empty());
    let auto = read_auto(dir.path());
    let base = &auto.providers["deepseek"].base;
    // 基础数据全部不变，来源仍是内置
    assert_eq!(base.source, "builtin");
    assert!(base.models.contains_key("deepseek-flash"));
    common::cleanup_cred_file("public-missing");
}

#[test]
fn public_malformed_fields_no_panic() {
    let dir = TempDir::new("public-malformed");
    let public = spawn_mock(vec![MockResp::json(
        200,
        json!({"deepseek": {"models": {
            "deepseek-flash": {"name": 123, "limit": {"context": -5, "output": 393216}},
            "big-model": {"name": "Big", "limit": {"context": 18_446_744_073_709_551_615u64}}
        }}})
        .to_string(),
    )]);
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("public-malformed", "key-A");
    let mock = spawn_mock(vec![MockResp::json(200, list_json(&["m1"], false))]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let report = catalog::refresh(&api::agent(), &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(report.queried.iter().any(|(id, _)| id == "deepseek"));
    let auto = read_auto(dir.path());
    let base = &auto.providers["deepseek"].base;
    // 类型错误字段跳过映射（能力留空/保留原值），合法字段照常
    let flash = &base.models["deepseek-flash"];
    assert_eq!(flash.display_name.as_deref(), Some("DeepSeek-V4.1-Flash"));
    assert_eq!(flash.context_length, Some(1_048_576));
    assert_eq!(flash.max_output, Some(393_216));
    assert_eq!(base.models["big-model"].context_length, Some(u64::MAX));
    common::cleanup_cred_file("public-malformed");
}

#[test]
fn public_unknown_providers_not_added() {
    let dir = TempDir::new("public-unknown");
    let vendors: serde_json::Value = (0..100)
        .map(|i| (format!("vendor{i}"), json!({"models": {}})))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();
    let public = spawn_mock(vec![MockResp::json(200, vendors.to_string())]);
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("public-unknown", "key-A");
    let mock = spawn_mock(vec![MockResp::json(200, list_json(&["m1"], false))]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    catalog::refresh(&api::agent(), &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    let auto = read_auto(dir.path());
    let keys: Vec<&str> = auto.providers.keys().map(|s| s.as_str()).collect();
    assert_eq!(keys, vec!["deepseek", "openai"]);
    common::cleanup_cred_file("public-unknown");
}

#[test]
fn public_cannot_touch_connection_fields() {
    let dir = TempDir::new("public-conn");
    let public = spawn_mock(vec![MockResp::json(
        200,
        json!({"deepseek": {"base_url": "https://evil.example", "protocol": "evil", "key_ref": "env:EVIL",
            "models": {"deepseek-flash": {"name": "Evil Flash", "limit": {"context": 1, "output": 1}}}}})
        .to_string(),
    )]);
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("public-conn", "key-A");
    let mock = spawn_mock(vec![MockResp::json(200, list_json(&["m1"], false))]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    catalog::refresh(&api::agent(), &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    let auto = read_auto(dir.path());
    let base = &auto.providers["deepseek"].base;
    // 公共数据只能影响模型能力字段，连接信息原样
    assert_eq!(base.base_url.as_deref(), Some("https://api.deepseek.com"));
    assert_eq!(base.protocol.as_deref(), Some("openai-chat"));
    assert_eq!(base.key_ref.as_deref(), Some("env:DEEPSEEK_API_KEY"));
    common::cleanup_cred_file("public-conn");
}

#[test]
fn public_sync_failure_continues_query() {
    let dir = TempDir::new("public-fail");
    let public = spawn_mock(vec![MockResp::json(500, r#"{"error":"down"}"#)]);
    let _env = set_env(&ENV_LOCK, dir.path(), &public.url);
    let key_ref = common::unique_cred_file("public-fail", "key-A");
    let mock = spawn_mock(vec![MockResp::json(200, list_json(&["m1"], false))]);
    write_manual(dir.path(), &manual_config(&mock.url, &key_ref));
    write_auto(dir.path(), &catalog::AutoCatalog { providers: catalog::builtin(), ..Default::default() });
    let report = catalog::refresh(&api::agent(), &mut |_origin, _name, _fp| Ok(()), &mut |_origin| Ok(()), Some(&_env.public_url)).unwrap();
    assert!(report.public_error.is_some());
    // 同步失败不中断厂商查询
    assert!(report.queried.iter().any(|(id, _)| id == "deepseek"));
    let auto = read_auto(dir.path());
    assert_eq!(auto.providers["deepseek"].base.source, "builtin");
    common::cleanup_cred_file("public-fail");
}

#[test]
fn concurrent_saves_last_writer_wins() {
    let dir = TempDir::new("concurrent");
    let _env = set_env(&ENV_LOCK, dir.path(), "http://127.0.0.1:1/unused");
    let key_ref = common::unique_cred_file("concurrent", "key-A");
    write_manual(dir.path(), &manual_config("http://127.0.0.1:1", &key_ref));
    write_auto(dir.path(), &auto_with_old_query("http://127.0.0.1:1", "key-A"));
    fn worker(dir_path: &Path, tag: &str, models: Vec<catalog::ModelInfo>) -> String {
        for _ in 0..25 {
            let mut auto = catalog::load_auto(dir_path).unwrap();
            if let Some(entry) = auto.providers.get_mut("deepseek") {
                if let Some(q) = entry.query.as_mut() {
                    q.models = models.clone();
                }
            }
            catalog::save_auto(dir_path, &mut auto).unwrap();
        }
        tag.to_string()
    }
    let dir_path = dir.path().to_path_buf();
    let mk = |id: &str| vec![catalog::ModelInfo { id: id.to_string(), ..Default::default() }];
    let dp = dir_path.clone();
    let h1 = std::thread::spawn(move || worker(&dp, "A", mk("from-A")));
    let dp = dir_path.clone();
    let h2 = std::thread::spawn(move || worker(&dp, "B", mk("from-B")));
    h1.join().unwrap();
    h2.join().unwrap();
    // 终态 = 某一方最后一次完整写入，可解析、不混合
    let auto = read_auto(dir.path());
    let ids: Vec<&str> = auto.providers["deepseek"]
        .query
        .as_ref()
        .unwrap()
        .models
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert!(ids == vec!["from-A"] || ids == vec!["from-B"], "混合内容：{ids:?}");
    // 本次写入的临时文件都已替换掉，无残留
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
    common::cleanup_cred_file("concurrent");
}

// ---------- chat_with_body：核心字段不可被 request_params 覆盖（006 收尾）----------

/// request_params 携带 messages/tools/model 时必须在发送前拒绝：
/// 合法配置（模型声明支持该参数）也不得覆盖程序组装的核心请求字段。
#[test]
fn chat_with_body_rejects_params_overriding_core_fields() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    for reserved in ["tools", "model", "messages"] {
        let mut request_params = std::collections::BTreeMap::new();
        request_params.insert(reserved.to_string(), json!([]));
        let conn = catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: "openai-chat".into(),
            key_ref: None,
            request_params,
        };
        // 合法配置：supported_params 显式声明支持该参数（tools 情形由此过关）
        let model = catalog::ModelInfo {
            id: "m".into(),
            supported_params: Some(vec![reserved.to_string()]),
            ..Default::default()
        };
        let request = json!({ "messages": [{ "role": "user", "content": "hi" }], "tools": [] });
        let err = api::chat_with_body(&conn, "k", &model, &request, &api::agent()).unwrap_err();
        assert!(
            err.contains("保留字段") && err.contains(reserved),
            "{reserved} 必须被明确拒绝：{err}"
        );
    }
    assert_eq!(mock.count(), 0, "拒绝必须发生在任何请求发出之前");
}

/// 普通参数照常传递，核心字段来自程序组装：temperature 随请求发送，
/// model/messages/tools 保持 005 组装结果不变。
#[test]
fn chat_with_body_sends_normal_params_and_assembled_core() {
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    let mut request_params = std::collections::BTreeMap::new();
    request_params.insert("temperature".to_string(), json!(0.5));
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params,
    };
    let model = catalog::ModelInfo {
        id: "m".into(),
        supported_params: Some(vec!["temperature".into()]),
        ..Default::default()
    };
    let request = terminal_chat::prompt::build_request(&[], "你好").unwrap();
    let resp = api::chat_with_body(&conn, "k", &model, &request, &api::agent()).unwrap();
    assert_eq!(
        resp.pointer("/choices/0/message/content").and_then(|c| c.as_str()),
        Some("收到")
    );
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&reqs[0].body).unwrap();
    // 普通参数随请求发送
    assert_eq!(body["temperature"], 0.5);
    // 核心字段来自程序组装，未被配置改写
    assert_eq!(body["model"], "m");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["content"], "你好");
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4, "017：四工具");
    assert_eq!(tools[0]["function"]["name"], "exec_command");
    assert_eq!(tools[1]["function"]["name"], "apply_patch");
    assert_eq!(tools[3]["function"]["name"], "update_plan");
}

// ---------- NET-B01：全部重定向状态码一律拒绝 ----------

#[test]
fn net_b01_all_redirect_statuses_rejected() {
    for status in [300u16, 301, 302, 303, 307, 308, 399] {
        let _env = set_env(
            &ENV_LOCK,
            Path::new("/nonexistent"),
            "http://127.0.0.1:1/unused",
        );
        let target = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
        let mock = spawn_mock(vec![MockResp {
            status,
            delay_ms: 0,
            body: String::new(),
            kind: common::RespKind::Respond,
            location: Some(format!("{}/chat/completions", target.url)),
            gate: None,
        }]);
        let conn = catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: "openai-chat".into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
        let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
        assert!(err.contains("重定向"), "状态 {status} 必须按重定向拒绝：{err}");
        assert_eq!(mock.count(), 1, "{status}：首站恰一次请求");
        assert_eq!(target.count(), 0, "{status}：重定向目标零请求、零凭证");
    }
}

// ---------- SECRET-E02：服务端错误回显完整假 Key 必须脱敏 ----------
// 契约：错误消息不得回显凭证（2026-10-05 安全修复的泄露面契约）；服务端把 Key
// 放进 error.message 时，300 字符截断不能"偶然"成为唯一防线。

#[test]
fn secret_e02_server_echo_of_key_is_redacted() {
    let key = "test-key-A-0001";
    for (name, tail) in [
        ("回显在开头", String::new()),
        ("回显在前300内", "x".repeat(200)),
        ("回显在300之外", "y".repeat(400)),
    ] {
        let _env = set_env(
            &ENV_LOCK,
            Path::new("/nonexistent"),
            "http://127.0.0.1:1/unused",
        );
        let body = json!({ "error": { "message": format!("Bad key {key} rejected {tail}") } })
            .to_string();
        let mock = spawn_mock(vec![MockResp::json(401, body)]);
        let conn = catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: "openai-chat".into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
        let err = api::chat(&conn, key, &model, "hi", &api::agent()).unwrap_err();
        assert_eq!(mock.count(), 1, "{name}：请求确实发出");
        assert!(!err.contains(key), "{name}：错误不得回显 Key：{err}");
        assert!(err.contains("鉴权失败"), "{name}：分类信息保留：{err}");
    }
}

/// SECRET-E02/E04 的真实 CLI 面：401 回显 Key 时，应用全部输出（stdout＋stderr）不含 Key。
#[test]
fn secret_e02_real_cli_output_never_echoes_key() {
    let key = "test-key-B-0002";
    let dir = TempDir::new("sec-echo");
    let mock = spawn_mock(vec![MockResp::json(
        401,
        json!({ "error": { "message": format!("Invalid key {key} in request") } }).to_string(),
    )]);
    let manual = manual_config(&mock.url, "env:TCHAT_SEC_KEY");
    write_manual(dir.path(), &manual);
    let user = TempDir::new("sec-echo-user");
    std::fs::write(
        user.path().join("authorized-origins.json"),
        json!({ "grants": [{ "origin": catalog::origin_of(&mock.url).unwrap(),
            "cred_fp": catalog::fingerprint(key), "added_at": 0 }] }).to_string(),
    )
    .unwrap();

    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .env("TCHAT_CONFIG_DIR", dir.path())
        .env("TCHAT_USER_DIR", user.path())
        .env("TCHAT_SEC_KEY", key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动子进程失败");
    child.stdin.take().unwrap().write_all("你好\nexit\n".as_bytes()).unwrap();
    let out = child.wait_with_output().expect("等待子进程失败");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(mock.count(), 1, "请求确实发出且被 401 拒绝");
    assert!(!stdout.contains(key) && !stderr.contains(key),
        "Key 泄露到真实 CLI 输出——stdout：{stdout}\nstderr：{stderr}");
    assert!(stdout.contains("错误："), "错误应展示：{stdout}");
    assert_eq!(out.status.code(), Some(0));
}

/// TERM-E01：模型回复注入 OSC52/标题/清屏，真实 CLI 字节流零 ESC。
#[test]
fn term_e01_real_cli_filters_injected_sequences() {
    let dir = TempDir::new("term-e01");
    let malicious = "\u{1b}]52;c;aGVsbG8=\u{07}注入测试\u{1b}[2J\u{1b}]0;pwned\u{07}";
    let mock = spawn_mock(vec![MockResp::json(
        200,
        json!({ "choices": [{ "message": { "content": malicious }, "finish_reason": "stop" }] })
            .to_string(),
    )]);
    let manual = manual_config(&mock.url, "env:TCHAT_TERM_KEY");
    write_manual(dir.path(), &manual);
    let user = TempDir::new("term-e01-user");
    std::fs::write(
        user.path().join("authorized-origins.json"),
        json!({ "grants": [{ "origin": catalog::origin_of(&mock.url).unwrap(),
            "cred_fp": catalog::fingerprint("k"), "added_at": 0 }] }).to_string(),
    )
    .unwrap();

    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .env("TCHAT_CONFIG_DIR", dir.path())
        .env("TCHAT_USER_DIR", user.path())
        .env("TCHAT_TERM_KEY", "k")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动子进程失败");
    child.stdin.take().unwrap().write_all("你好\nexit\n".as_bytes()).unwrap();
    let out = child.wait_with_output().expect("等待子进程失败");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(mock.count(), 1);
    assert!(!stdout.as_bytes().contains(&0x1b), "真实 CLI 输出不得含 ESC 字节：{stdout:?}");
    assert!(stdout.contains("注入测试"), "正常文字保留：{stdout}");
    assert!(!stdout.contains("pwned"), "OSC 载荷与标题必须整段剥离：{stdout}");
}

/// SECRET-E02 顺序：完整假 Key 横跨 300 字符截断边界——脱敏必须发生在截断之前，
/// 否则截断把 Key 切开后，边界整串替换匹配不到残留片段。用预选独特片段（前 12 字符）
/// 检测程序截断造成的残留，不扩展为任意短子串脱敏。
#[test]
fn secret_e02_straddle_boundary_no_fragment() {
    let key = "test-key-A-0001";
    let fragment = "test-key-A"; // 10 字符独特片段，非任意短子串
    let _env = set_env(
        &ENV_LOCK,
        Path::new("/nonexistent"),
        "http://127.0.0.1:1/unused",
    );
    // 填充使 Key（15 字符）起点位于 290：截断到 300 时保留 Key 前 10 字符“test-key-A”
    let msg = format!("{}{key}", "x".repeat(290));
    assert_eq!(msg.chars().count(), 305);
    let mock = spawn_mock(vec![MockResp::json(
        401,
        json!({ "error": { "message": msg } }).to_string(),
    )]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    let err = api::chat(&conn, key, &model, "hi", &api::agent()).unwrap_err();
    assert_eq!(mock.count(), 1);
    assert!(!err.contains(key), "完整 Key 不得出现：{err}");
    assert!(!err.contains(fragment), "截断造成的 Key 片段残留：{err}");
    assert!(err.contains("鉴权失败"), "分类保留：{err}");
}

/// AUTH-E01 补充：授权记录为空文件 / JSON 数组 / 关键字段类型错误——一律按无授权
/// 处理（fail-closed），非交互零请求；未经明确恢复不得视为已授权。
#[test]
fn auth_e01_malformed_store_variants_fail_closed() {
    for (name, content) in [
        ("空文件", ""),
        ("JSON数组", "[]"),
        ("字段类型错误", r#"{"grants":[{"origin":"http://127.0.0.1:1","cred_fp":123,"added_at":0}]}"#),
        ("根为数组", r#"[{"origin":"http://127.0.0.1:1","cred_fp":"x","added_at":0}]"#),
    ] {
        let dir = TempDir::new("ae01");
        let user = TempDir::new("ae01-user");
        std::fs::write(user.path().join("authorized-origins.json"), content).unwrap();
        let manual = manual_config("http://127.0.0.1:1", "env:TCHAT_AE01_KEY");
        write_manual(dir.path(), &manual);

        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
            .env("TCHAT_CONFIG_DIR", dir.path())
            .env("TCHAT_USER_DIR", user.path())
            .env("TCHAT_AE01_KEY", "ae01-key")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("启动子进程失败");
        child.stdin.take().unwrap().write_all("你好\nexit\n".as_bytes()).unwrap();
        let out = child.wait_with_output().expect("等待子进程失败");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(0), "{name}：{stdout}");
        assert!(
            stdout.contains("未授权发送到") || stdout.contains("未授权"),
            "{name}：损坏记录必须按无授权拒绝：{stdout}"
        );
        // 记录内容保持原样（拒绝路径不落盘、不“修复”）
        assert_eq!(std::fs::read_to_string(user.path().join("authorized-origins.json")).unwrap(), content);
    }
}

/// NET-B02：3xx 指向同 origin、无 Location、Location 非法——都不产生第二跳。
#[test]
fn net_b02_redirect_edge_forms_no_second_hop() {
    for (name, resp) in [
        ("同origin", MockResp { status: 302, delay_ms: 0, body: String::new(), gate: None,
            kind: common::RespKind::Respond, location: Some("http://127.0.0.1:1/self".into()) }),
        ("无Location", MockResp::json(302, "")),
        ("Location非法", MockResp { status: 307, delay_ms: 0, body: String::new(), gate: None,
            kind: common::RespKind::Respond, location: Some("http://[invalid".into()) }),
    ] {
        let _env = set_env(&ENV_LOCK, Path::new("/nonexistent"), "http://127.0.0.1:1/unused");
        let mock = spawn_mock(vec![resp]);
        let conn = catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: "openai-chat".into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
        let err = api::chat(&conn, "k", &model, "hi", &api::agent()).unwrap_err();
        assert!(err.contains("重定向"), "{name}：3xx 一律拒绝：{err}");
        assert_eq!(mock.count(), 1, "{name}：首站恰一次，无第二跳");
    }
}

/// NET-B06：有效 JSON 正文总字节恰为 L−1 / L / L+1（独立核对字节长度）——
/// L−1 与 L 成功且内容正确，L+1 明确超限。L=10 MiB=10,485,760。
#[test]
fn net_b06_body_limit_exact_three_points() {
    const L: usize = 10 * 1024 * 1024;
    for (name, target) in [("L-1", L - 1), ("L", L), ("L+1", L + 1)] {
        let _env = set_env(&ENV_LOCK, Path::new("/nonexistent"), "http://127.0.0.1:1/unused");
        let prefix = r#"{"choices":[{"message":{"content":"BCBEGIN"#;
        let suffix = r#""},"finish_reason":"stop"}]}"#;
        let filler = "A".repeat(target - prefix.len() - suffix.len());
        let body = format!("{prefix}{filler}{suffix}");
        assert_eq!(body.len(), target, "{name}：正文总字节独立核对");
        assert!(serde_json::from_str::<serde_json::Value>(&body).is_ok(), "{name}：必须是有效完整 JSON");
        let marker = format!("BCBEGIN{}", "A".repeat(16));
        let mock = spawn_mock(vec![MockResp::json(200, body)]);
        let conn = catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: "openai-chat".into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
        let result = api::chat(&conn, "k", &model, "hi", &api::agent());
        match target {
            n if n <= L => {
                let reply = result.unwrap_or_else(|e| panic!("{name} 应成功：{e}"));
                assert!(reply.starts_with(&marker), "{name}：内容正确开头");
                assert_eq!(reply.chars().count(), target - prefix.len() - suffix.len() + "BCBEGIN".len(),
                    "{name}：内容完整无缺");
            }
            _ => {
                let err = result.unwrap_err();
                assert!(err.contains("10 MiB"), "{name}：必须明确报超限：{err}");
            }
        }
    }
}

/// SECRET-B03：Key 含 \n / \r / NUL（原 newline_key 场景并入）——逐场景发送前拒绝、
/// 零请求、错误不回显原值与片段。
#[test]
fn secret_b03_cr_and_nul_keys_rejected_without_echo() {
    let _env = set_env(&ENV_LOCK, Path::new("/nonexistent"), "http://127.0.0.1:1/unused");
    let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
    let conn = catalog::EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
    for key in ["bad\nkey-NL", "bad\rkey-CR", "bad\0key-NUL"] {
        let err = api::chat(&conn, key, &model, "hi", &api::agent())
            .err()
            .unwrap_or_else(|| panic!("含控制字符的 Key 不应发出请求：{key:?}"));
        assert!(err.contains("控制字符"), "拦截原因明确（{key:?}）：{err}");
        assert!(!err.contains(key), "错误不得回显原值（{key:?}）：{err}");
        assert!(!err.contains("bad"), "错误不得回显 Key 片段（{key:?}）：{err}");
        assert_eq!(mock.count(), 0, "发送前拒绝：零请求（{key:?}）");
    }
}

/// N1 真实 CLI 三路径：回复、工具通知（id 含 CR）、错误消息——输出零原始 CR；
/// 正常文字/换行/Tab 保留；历史中 tool_call_id 原始保真（不经显示过滤改写）。
#[test]
fn n1_real_cli_three_paths_no_raw_cr() {
    let key = "n1-key";
    let dir = TempDir::new("n1-cli");
    let mock = spawn_mock(vec![
        MockResp::json(200, json!({ "choices": [{ "message": {
            "role": "assistant", "content": null,
            "tool_calls": [{ "id": "AAAA\rBBBB", "type": "function",
                "function": { "name": "exec_command", "arguments": "{\"cmd\":\"true\"}" } }]
        }, "finish_reason": "tool_calls" }] }).to_string()),
        MockResp::json(401, json!({ "error": { "message": "line1\rOVERWRITTEN-PWNED 续" } }).to_string()),
        MockResp::json(200, json!({ "choices": [{ "message": {
            "role": "assistant", "content": "回复OK\rPWNED-REPLY\t保\t留"
        }, "finish_reason": "stop" }] }).to_string()),
    ]);
    let manual = manual_config(&mock.url, "env:TCHAT_N1_KEY");
    write_manual(dir.path(), &manual);
    let user = TempDir::new("n1-cli-user");
    std::fs::write(
        user.path().join("authorized-origins.json"),
        json!({ "grants": [{ "origin": catalog::origin_of(&mock.url).unwrap(),
            "cred_fp": catalog::fingerprint(key), "added_at": 0 }] }).to_string(),
    )
    .unwrap();

    use std::io::Write;
    use std::process::{Command, Stdio};
    // 015：cwd 落临时目录，避免注入依赖仓库真实 AGENTS.md（消息序随之固定）
    let project = TempDir::new("n1-cli-proj");
    let mut child = Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .current_dir(project.path())
        .env("TCHAT_CONFIG_DIR", dir.path())
        .env("TCHAT_USER_DIR", user.path())
        .env("TCHAT_N1_KEY", key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动子进程失败");
    child.stdin.take().unwrap().write_all("你好\ngo\nexit\n".as_bytes()).unwrap();
    let out = child.wait_with_output().expect("等待子进程失败");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    // 三路径零原始 CR 字节
    assert!(!stdout.as_bytes().contains(&0x0d), "输出不得含原始 CR：{stdout:?}");
    // 通知行：CR 剥离但 id 主体文字保留
    // take(8) 含被剥离的 \r：可见前缀为 7 个字符 AAAABBB
    assert!(stdout.contains("[工具] exec_command（AAAABBB）"), "通知行 CR 剥离：{stdout}");
    // 错误路径文字保留（CR 剥离后两段相连）
    assert!(stdout.contains("line1OVERWRITTEN-PWNED 续"), "错误路径：{stdout}");
    // 回复路径：CR 剥离、Tab 保留
    assert!(stdout.contains("回复OKPWNED-REPLY\t保\t留"), "回复路径：{stdout}");
    // 历史原始保真：第 2 个请求（401 后同轮报错，取第 3 个请求核对）assistant 的
    // tool_calls id 仍是原始含 CR 字符串（显示过滤不得改写协议数据）
    let reqs = mock.requests();
    assert!(reqs.len() >= 3, "{reqs:?}");
    let body: serde_json::Value = serde_json::from_str(&reqs[1].body).unwrap();
    assert_eq!(
        body["messages"][3]["tool_calls"][0]["id"],
        json!("AAAA\rBBBB"),
        "历史中的原始 tool_call_id 必须保真"
    );
}

/// n4c（2026-10-10 合并前补测）：多字节内容的两点边界——中文 UTF-8 字节计量。
/// 字符数 < L 而字节数达到 L/L+1：L 允许恰一请求、L+1 拒绝零发送；
/// 防字符数当字节数（n4b 填充为纯 ASCII，未覆盖多字节）。
#[test]
fn n4c_request_body_limit_multibyte_two_points() {
    const L: usize = 10 * 1024 * 1024;
    for (name, target) in [("中文-L", L), ("中文-L+1", L + 1)] {
        let _env = set_env(&ENV_LOCK, Path::new("/nonexistent"), "http://127.0.0.1:1/unused");
        let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
        let conn = catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: "openai-chat".into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
        // 内容＝固定数量中文＋ASCII 微调：每个"汉"3 字节；先给大基数多字节，
        // 尾部 ASCII 逐字节 1:1 收敛（serde_json 对 CJK 原样输出不转义）
        let han = 2_000_000;
        let build = |ascii: usize| {
            let content = format!("{}{}", "汉".repeat(han), "x".repeat(ascii));
            let mut body = json!({ "messages": [{ "role": "user", "content": content }], "tools": [] });
            body["model"] = json!("m");
            body
        };
        let mut ascii = 0usize;
        let actual = serde_json::to_vec(&build(ascii)).unwrap().len();
        assert!(actual < target, "{name}：多字节基数过大（{actual}）");
        ascii = target - actual;
        let body = build(ascii);
        let total = serde_json::to_vec(&body).unwrap().len();
        assert_eq!(total, target, "{name}：目标字节独立核对（han={han}, ascii={ascii}）");
        let content_chars = han + ascii;
        let content_bytes = 3 * han + ascii;
        // 字符数远小于字节边界；字节数贴合边界（扣除 JSON 框架开销的合理余量）
        assert!(
            content_chars < L && content_bytes + 1024 >= target,
            "{name}：字符数 {content_chars} 应远小于字节边界而字节数 {content_bytes} 贴合目标 {target}"
        );
        let request = json!({
            "messages": [{ "role": "user",
                "content": format!("{}{}", "汉".repeat(han), "x".repeat(ascii)) }],
            "tools": []
        });
        let result = api::chat_with_body(&conn, "k", &model, &request, &api::agent());
        if target <= L {
            result.unwrap_or_else(|e| panic!("{name}（ascii={ascii}）恰上限应成功：{e}"));
            assert_eq!(mock.count(), 1, "{name}：恰一次请求");
            let sent: serde_json::Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
            let sent_content = sent["messages"][0]["content"].as_str().unwrap();
            assert_eq!(sent_content.len(), content_bytes, "{name}：字节完整无损");
            assert_eq!(sent_content.chars().count(), content_chars, "{name}：字符数一致");
        } else {
            let err = result.unwrap_err();
            assert!(err.contains("10 MiB"), "{name}：超一字节必须拒绝：{err}");
            assert_eq!(mock.count(), 0, "{name}：零发送");
        }
    }
}

/// 2026-10-10 合并前补测：.env 对 TCHAT_SESSION_TS/PID 的信任根隔离——
/// 大小写变体、空值、重复声明一律跳过；普通变量正常加载；进程已有值保留。
#[test]
fn dotenv_skips_session_trust_vars() {
    let _lock = common::ENV_LOCK.lock().unwrap();
    let dir = common::TempDir::new("dotenv-ts");
    let env_path = dir.path().join(".env");
    std::fs::write(
        &env_path,
        "TCHAT_SESSION_TS=20260101T000000\n\
         tchat_session_pid=999\n\
         TCHAT_SESSION_TS=20270101T000000\n\
         TCHAT_SESSION_PID=4294967295\n\
         TChat_Session_Pid=123\n\
         TCHAT_SESSION_TS=\n\
         SC_DOTENV_NORMAL=ok\n",
    )
    .unwrap();
    for k in ["TCHAT_SESSION_TS", "TCHAT_SESSION_PID", "SC_DOTENV_NORMAL"] {
        std::env::remove_var(k);
    }
    let n = catalog::load_dotenv_from(&env_path);
    assert_eq!(n, 1, "仅普通变量加载（信任变量含大小写/空值/重复全跳过）");
    assert_eq!(std::env::var_os("TCHAT_SESSION_TS"), None, "TS 不入进程环境");
    assert_eq!(std::env::var_os("TCHAT_SESSION_PID"), None, "PID 不入进程环境");
    assert_eq!(std::env::var("SC_DOTENV_NORMAL").unwrap(), "ok");
    // 进程已有合法值＋.env 异值：进程值保留（无条件跳过≠仅不覆盖）
    std::env::set_var("TCHAT_SESSION_TS", "20250101T000000");
    let n2 = catalog::load_dotenv_from(&env_path);
    assert_eq!(n2, 0, "第二次加载零变量");
    assert_eq!(std::env::var("TCHAT_SESSION_TS").unwrap(), "20250101T000000");
    for k in ["TCHAT_SESSION_TS", "SC_DOTENV_NORMAL"] {
        std::env::remove_var(k);
    }
}

/// N4b：最终模型请求 JSON ≤10 MiB（UTF-8 字节）——L−1/L 正常发送且内容正确，
/// L+1 零发送并明确报错（限计量序列化，不先生成无界缓冲）。
#[test]
fn n4b_request_body_limit_exact_three_points() {
    const L: usize = 10 * 1024 * 1024;
    for (name, target) in [("L-1", L - 1), ("L", L), ("L+1", L + 1)] {
        let _env = set_env(&ENV_LOCK, Path::new("/nonexistent"), "http://127.0.0.1:1/unused");
        let mock = spawn_mock(vec![MockResp::json(200, CHAT_OK)]);
        let conn = catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: "openai-chat".into(),
            key_ref: None,
            request_params: Default::default(),
        };
        let model = catalog::ModelInfo { id: "m".into(), ..Default::default() };
        // 两遍收敛：ASCII 填充逐字节 1:1，按实测总长一次修正即精确
        let build = |filler: usize| {
            let mut body = json!({ "messages": [{ "role": "user", "content": "x".repeat(filler) }], "tools": [] });
            body["model"] = json!("m");
            body
        };
        let mut filler = target / 2;
        let actual = serde_json::to_vec(&build(filler)).unwrap().len();
        filler = filler + target - actual;
        let body = build(filler);
        let total = serde_json::to_vec(&body).unwrap().len();
        assert_eq!(total, target, "{name}：目标字节独立核对（filler={filler}）");
        let request = json!({ "messages": [{ "role": "user", "content": "x".repeat(filler) }], "tools": [] });
        let result = api::chat_with_body(&conn, "k", &model, &request, &api::agent());
        match target {
            n if n <= L => {
                result.unwrap_or_else(|e| panic!("{name}（filler={filler}）应成功发送：{e}"));
                assert_eq!(mock.count(), 1, "{name}：恰一次请求");
                let sent: serde_json::Value =
                    serde_json::from_str(&mock.requests()[0].body).unwrap();
                assert_eq!(sent["messages"][0]["content"].as_str().unwrap().len(), filler, "{name}：内容完整无损");
            }
            _ => {
                let err = result.unwrap_err();
                assert!(err.contains("10 MiB"), "{name}：必须明确报请求体超限：{err}");
                assert_eq!(mock.count(), 0, "{name}：超限必须零发送");
            }
        }
    }
}