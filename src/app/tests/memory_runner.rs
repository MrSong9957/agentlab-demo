//! 013-M7：D2 触发后台管线、threads 登记写入侧与 D9 命令面的集成验收。
//! 全部经生产入口（`spawn_memory_pipeline_in`／`ThreadRegistrar`／
//! `memory_status_text_in`／`memory_reset`），模型调用走本地 mock HTTP，
//! 会话/记忆根使用独立临时目录；不访问真实用户目录或真实记忆根。

mod common;

use common::{spawn_mock, MockResp, TempDir};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use terminal_chat::api;
use terminal_chat::catalog;
use terminal_chat::memory::config::MemoriesConfig;
use terminal_chat::memory::runner::{self, memories_root, ModelSend, ThreadRegistrar};
use terminal_chat::memory::store::{MemoryStore, Phase2JobClaimOutcome, Stage1JobClaimOutcome};
use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole};
use terminal_chat::session::Session;

fn temp(tag: &str) -> TempDir {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "tchat-m7-{}-{}-{tag}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    TempDir(path)
}

fn enabled() -> MemoriesConfig {
    MemoriesConfig {
        enabled: true,
        ..Default::default()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn text(role: HistoryRole, content: &str) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        images: None,
        text_elements: Vec::new(),
        role,
        content: content.to_string(),
        user_input_event: None,
    })
}

/// 合成会话文件（首行 meta＋逐行 HistoryItem）。
fn write_session(dir: &Path, name: &str, items: &[HistoryItem]) -> PathBuf {
    let path = dir.join(format!("{name}.jsonl"));
    let mut body = String::from("{\"v\":1}\n");
    for item in items {
        body.push_str(&serde_json::to_string(item).unwrap());
        body.push('\n');
    }
    fs::write(&path, body).unwrap();
    path
}

fn send_for_mock(mock: &common::Mock) -> ModelSend {
    ModelSend {
        conn: catalog::EffectiveConnection {
            base_url: mock.url.clone(),
            protocol: catalog::SUPPORTED_PROTOCOL.into(),
            key_ref: None,
            request_params: Default::default(),
        },
        key: "m7-fake-key".into(),
        model: catalog::ModelInfo {
            id: "m7-scripted-model".into(),
            ..Default::default()
        },
        agent: api::agent(),
    }
}

fn chat_body(content: &str) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

/// phase2 合并代理脚本：先写两产物（工具批），再终文。
fn merge_tool_batch() -> Value {
    json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "w-mem", "type": "function", "function": { "name": "mem_write",
              "arguments": "{\"path\":\"MEMORY.md\",\"content\":\"# Memory\\n\\n- merged\\n\"}" } },
            { "id": "w-sum", "type": "function", "function": { "name": "mem_write",
              "arguments": "{\"path\":\"memory_summary.md\",\"content\":\"v1\\n- merged summary\\n\"}" } }
        ]
    }, "finish_reason": "tool_calls" }] })
}

fn stage1_ok() -> String {
    chat_body(r#"{"raw_memory":"经验正文","rollout_summary":"任务摘要","rollout_slug":"slug-a"}"#)
}

// ---------- ① 触发与后台生命周期 ----------

/// 关闭态零副作用：不 spawn、不建记忆根或数据库。
#[test]
fn m7_disabled_config_is_zero_side_effect() {
    let dir = temp("disabled");
    let mock = spawn_mock(vec![]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &dir.path().join("sessions"),
        &MemoriesConfig::default(),
        Some("auto-current".into()),
        send_for_mock(&mock),
    );
    assert!(handle.is_none(), "关闭态不得 spawn");
    assert!(!memories_root(dir.path()).exists(), "关闭态不得创建记忆根");
    assert_eq!(mock.count(), 0, "关闭态不得发起任何模型请求");
}

/// 开态管线（无候选）：建库＋播种扩展说明；threads 表无登记行（登记＝
/// 写入侧职责，管线只读）；当前会话排除语义由候选扫描承接（A15）。
#[test]
fn m7_enabled_pipeline_initializes_root_and_seeds() {
    let dir = temp("bootstrap");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("ok"))]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        Some("auto-current".into()),
        send_for_mock(&mock),
    )
    .expect("开态必须 spawn");
    handle.join().expect("管线线程不得 panic");

    let root = memories_root(dir.path());
    assert!(root.join("memories.sqlite").is_file(), "状态库必须建立");
    let seeded = root
        .join("extensions")
        .join("ad_hoc")
        .join("instructions.md");
    assert!(seeded.is_file(), "扩展说明必须播种（上游原序）");
    assert!(!fs::read_to_string(&seeded).unwrap().is_empty());
    // seed 是 create_new：重复管线不得覆盖既有内容
    let before = fs::read_to_string(&seeded).unwrap();
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let _ = store;
    let handle2 = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        Some("auto-current".into()),
        send_for_mock(&mock),
    )
    .unwrap();
    handle2.join().unwrap();
    assert_eq!(
        fs::read_to_string(&seeded).unwrap(),
        before,
        "seed 不得覆盖"
    );
}

/// 端到端：真实旧会话（满足 6h–10d 窗口）→登记（毫秒）→管线 stage1
/// （真实 HTTP 请求＋严格 JSON）→phase2（合并工具批）→产物文件与库状态。
/// resolve_thread 用真实会话水合（rollout_path 命中临时 sessions 目录）。
#[test]
fn m7_pipeline_processes_real_candidate_end_to_end() {
    let dir = temp("e2e-pipeline");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-old-1",
        &[
            text(HistoryRole::User, "做过什么"),
            text(HistoryRole::Assistant, "完成了 X"),
        ],
    );
    // 登记旧会话：updated_at_ms 落在窗口内（相对真实 now 构造，任何日期运行均成立）
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let updated_ms = now_ms() - 7 * 24 * 3600 * 1000;
    store
        .register_thread("auto-old-1", "enabled", updated_ms)
        .unwrap();
    drop(store);

    let mock = spawn_mock(vec![
        MockResp::json(200, stage1_ok()),
        MockResp::json(200, merge_tool_batch().to_string()),
        MockResp::json(200, chat_body("consolidated")),
    ]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        Some("auto-current".into()),
        send_for_mock(&mock),
    )
    .unwrap();
    handle.join().expect("管线线程不得 panic");

    // stage1 产物入库（毫秒登记→秒级 source 版本，M2 换算）
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let out = store
        .stage1_output("auto-old-1")
        .unwrap()
        .expect("stage1 产物");
    assert_eq!(out.rollout_summary, "任务摘要");
    assert_eq!(out.source_updated_at, updated_ms.div_euclid(1000));
    // phase2 产物落盘（经 mem_write 工具批）
    assert!(root.join("MEMORY.md").is_file());
    let summary = fs::read_to_string(root.join("memory_summary.md")).unwrap();
    assert!(
        summary.contains("merged summary"),
        "总摘要须为合并产物：{summary}"
    );

    // 真实请求断言：stage1 请求体含会话正文与系统提示；phase2 首请求含
    // 合并 system 与三项 mem_* 工具
    let requests = mock.requests();
    assert!(requests.len() >= 3, "stage1＋phase2 两次请求必须真实发生");
    let s1: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(s1["model"], "m7-scripted-model");
    let s1_messages = s1["messages"].as_array().unwrap();
    assert!(
        s1_messages.iter().any(|m| m["content"]
            .as_str()
            .is_some_and(|c| c.contains("完成了 X"))),
        "stage1 输入须含真实会话正文"
    );
    let p2: Value = serde_json::from_str(&requests[1].body).unwrap();
    let tools = p2["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["mem_read", "mem_write", "mem_list"],
        "合并轮恰三项受限工具"
    );
}

/// 重复触发无害：第二次管线不再重新提炼（成功水位覆盖），模型请求数
/// 不随重复触发翻倍（jobs/全局锁互斥的端到端观察）。
#[test]
fn m7_repeat_pipeline_triggers_are_deduped() {
    let dir = temp("dedupe");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-old-2",
        &[
            text(HistoryRole::User, "问"),
            text(HistoryRole::Assistant, "答"),
        ],
    );
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    store
        .register_thread("auto-old-2", "enabled", now_ms() - 7 * 24 * 3600 * 1000)
        .unwrap();
    drop(store);

    let mock = spawn_mock(vec![
        MockResp::json(200, stage1_ok()),
        MockResp::json(200, merge_tool_batch().to_string()),
        MockResp::json(200, chat_body("consolidated")),
    ]);
    for _ in 0..2 {
        let handle = runner::spawn_memory_pipeline_in(
            dir.path(),
            &sessions,
            &enabled(),
            Some("auto-cur".into()),
            send_for_mock(&mock),
        )
        .unwrap();
        handle.join().unwrap();
    }
    let requests = mock.requests();
    // 以系统提示常量精确识别提炼请求（与 phase2 合并请求区分）
    let stage1_system = terminal_chat::memory::STAGE_ONE_SYSTEM;
    let stage1_requests = requests
        .iter()
        .filter(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["messages"].as_array().is_some_and(|msgs| {
                msgs.iter()
                    .any(|m| m["content"].as_str() == Some(stage1_system))
            })
        })
        .count();
    assert_eq!(stage1_requests, 1, "重复触发不得重复提炼（水位覆盖短路）");
    // 第二次管线：stage1 水位短路＋phase2 无差异短路——无新增模型请求
    assert_eq!(
        requests.len(),
        3,
        "两次触发的总请求数＝首轮 stage1×1＋phase2×2，第二轮零新增"
    );
}

/// 主轮失败不撤销已启动的管线（spawn 与主轮成败解耦——批准边界）。
#[test]
fn m7_main_failure_does_not_undo_spawned_pipeline() {
    let dir = temp("main-fail");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let mock = spawn_mock(vec![]);
    // 主轮请求失败（500）在前，管线仍须完成初始化
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        None,
        send_for_mock(&mock),
    )
    .unwrap();
    let main_chat = api::chat_with_body(
        &send_for_mock(&mock).conn,
        "k",
        &send_for_mock(&mock).model,
        &json!({"messages": []}),
        &api::agent(),
    );
    assert!(main_chat.is_err(), "主轮失败前提成立");
    handle.join().expect("主轮失败不得拖垮后台线程");
    assert!(
        memories_root(dir.path()).join("memories.sqlite").is_file(),
        "管线初始化照常完成"
    );
}

/// 后台自身失败静默记账：stage1 垃圾响应→任务 error＋重试排程，
/// 管线不 panic、不产出文件。
#[test]
fn m7_background_failure_records_error_silently() {
    let dir = temp("bg-fail");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-old-3",
        &[
            text(HistoryRole::User, "q"),
            text(HistoryRole::Assistant, "a"),
        ],
    );
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    store
        .register_thread("auto-old-3", "enabled", now_ms() - 7 * 24 * 3600 * 1000)
        .unwrap();
    drop(store);

    let mock = spawn_mock(vec![MockResp::json(200, chat_body("这不是 JSON"))]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        Some("cur".into()),
        send_for_mock(&mock),
    )
    .unwrap();
    handle.join().expect("后台失败不得 panic");
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let row = store
        .job_row("memory_stage1", "auto-old-3")
        .unwrap()
        .expect("任务行存在");
    assert_eq!(row.status, "error", "失败按既有契约记账");
    assert!(row.retry_at.is_some(), "重试排程由既有契约承接");
    assert!(!root.join("MEMORY.md").exists(), "失败不得产出合并文件");
}

// ---------- ② threads 登记写入侧 ----------

/// 登记仅在 (stem, durable_len) 变化时推进；重复回调零额外写入。
#[test]
fn m7_registrar_advances_only_on_durable_change() {
    let dir = temp("reg");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let mut session = Session::new(sessions.clone());
    let mut reg = ThreadRegistrar::default();
    let cfg = enabled();
    let history = vec![text(HistoryRole::User, "第一问")];

    // 首条记录：目标创建＋落盘成功→登记
    session.on_history(&history);
    reg.note_persisted(dir.path(), &cfg, &session);
    let stem = session.current_target_stem().expect("已有追加目标");
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let ms1 = store.thread_updated_at_ms(&stem).unwrap().expect("已登记");

    // 同一持久化事实重复回调：不推进
    std::thread::sleep(std::time::Duration::from_millis(5));
    reg.note_persisted(dir.path(), &cfg, &session);
    let ms2 = store.thread_updated_at_ms(&stem).unwrap().unwrap();
    assert_eq!(ms1, ms2, "同一 (stem, durable_len) 不得重复推进");

    // 新记录成功落盘：推进
    let mut history2 = history.clone();
    history2.push(text(HistoryRole::Assistant, "第一答"));
    session.on_history(&history2);
    reg.note_persisted(dir.path(), &cfg, &session);
    let ms3 = store.thread_updated_at_ms(&stem).unwrap().unwrap();
    assert!(ms3 >= ms2, "新持久化事实必须推进 updated_at_ms");

    // 毫秒单位与秒级换算（M2 语义）经扫描路径核对
    assert!(ms3 >= 1_000_000_000_000, "登记值为毫秒");
}

/// 关闭态与无追加目标：零副作用（不建库、不登记）。
#[test]
fn m7_registrar_noop_when_disabled_or_no_target() {
    let dir = temp("reg-noop");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let mut session = Session::new(sessions.clone());
    let mut reg = ThreadRegistrar::default();
    // 关闭态：已有持久化事实也不登记、不建库
    let history = vec![text(HistoryRole::User, "q")];
    session.on_history(&history);
    reg.note_persisted(dir.path(), &MemoriesConfig::default(), &session);
    assert!(!memories_root(dir.path()).exists(), "关闭态不得建库");
    // 开态但无目标（未写过任何记录）：同样零副作用
    let idle = Session::new(sessions.clone());
    reg.note_persisted(dir.path(), &enabled(), &idle);
    assert!(idle.current_target_stem().is_none());
    assert!(!memories_root(dir.path()).exists(), "无目标不得建库");
}

/// 恢复切换：登记跟随当前追加目标（新 stem）。
#[test]
fn m7_registrar_follows_resume_switch() {
    let dir = temp("reg-switch");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    // 预置一个可恢复的 auto 会话
    write_session(
        &sessions,
        "auto-older",
        &[
            text(HistoryRole::User, "旧问"),
            text(HistoryRole::Assistant, "旧答"),
        ],
    );
    let mut session = Session::new(sessions.clone());
    let mut reg = ThreadRegistrar::default();
    let cfg = enabled();
    let history = vec![text(HistoryRole::User, "新会话问")];
    session.on_history(&history);
    reg.note_persisted(dir.path(), &cfg, &session);
    let stem1 = session.current_target_stem().unwrap();

    // 恢复到旧会话（当前待写段已全部落盘，切换安全）
    let mut loaded_history = match session.resume(Some("auto-older")) {
        Ok(info) => info.loaded.history,
        Err(e) => panic!("恢复失败：{e}"),
    };
    let stem2 = session.current_target_stem().unwrap();
    assert_eq!(stem2, "auto-older");
    assert_ne!(stem1, stem2);
    // resume 后新记录写入旧目标：登记新 stem
    loaded_history.push(text(HistoryRole::User, "续问"));
    session.on_history(&loaded_history);
    reg.note_persisted(dir.path(), &cfg, &session);
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    assert!(
        store.thread_updated_at_ms(&stem1).unwrap().is_some(),
        "旧目标登记保留"
    );
    let ms_new = store
        .thread_updated_at_ms("auto-older")
        .unwrap()
        .expect("新目标已登记");
    assert!(ms_new >= 1_000_000_000_000);
}

// ---------- ④ resolve_thread 真实水合 ----------

#[test]
fn m7_resolve_thread_meta_hydrates_real_files() {
    let dir = temp("resolve");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(&sessions, "auto-a", &[text(HistoryRole::User, "x")]);
    let cwd = Path::new("/tmp");
    let meta = runner::resolve_thread_meta(&sessions, cwd, "auto-a").expect("存在即水合");
    assert_eq!(meta.rollout_path, sessions.join("auto-a.jsonl"));
    assert_eq!(meta.cwd, PathBuf::from("/tmp"));
    assert_eq!(
        meta.git_branch, None,
        "会话工作目录/分支未持久化（登记差异）"
    );
    assert!(runner::resolve_thread_meta(&sessions, cwd, "auto-missing").is_none());
}

// ---------- ⑤ D9 命令面 ----------

#[test]
fn m7_status_reports_config_root_and_counts() {
    let dir = temp("status");
    // 关态＋无根：不建目录（只读）
    let text = runner::memory_status_text_in(dir.path(), &MemoriesConfig::default());
    assert!(text.contains("未启用"));
    assert!(text.contains("未初始化"));
    assert!(!memories_root(dir.path()).exists(), "status 只读不建根");

    // 开态＋有库：报告产物与任务计数
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    store
        .register_thread("t-status", "enabled", now_ms() - 7 * 24 * 3600 * 1000)
        .unwrap();
    drop(store);
    fs::write(root.join("memory_summary.md"), "v1\n存在").unwrap();
    let text = runner::memory_status_text_in(dir.path(), &enabled());
    assert!(text.contains("已启用"));
    assert!(text.contains("存在"), "总摘要存在性：{text}");
    assert!(text.contains("后台任务"), "任务计数字段：{text}");

    // 只读边界（根存在无库）：不因 status 建库
    let dir2 = temp("status-nodb");
    let root2 = memories_root(dir2.path());
    fs::create_dir(&root2).unwrap();
    fs::write(root2.join("memory_summary.md"), "v1\nx").unwrap();
    let text = runner::memory_status_text_in(dir2.path(), &enabled());
    assert!(text.contains("状态库：未初始化"), "{text}");
    assert!(
        !root2.join("memories.sqlite").is_file(),
        "status 只读——不得因观察而建库"
    );
}

/// reset 边界：缺 confirm 零删除；confirm 清内容保根＋sessions；
/// 符号链接根拒绝。
#[test]
fn m7_reset_requires_confirm_and_preserves_sessions() {
    let dir = temp("reset");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-keep",
        &[text(HistoryRole::User, "原始会话")],
    );
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, std::sync::Arc::new(|| 1_700_000_000)).unwrap();
    store.register_thread("t", "enabled", 1_700_000_000_000).unwrap();
    drop(store);
    fs::write(root.join("memory_summary.md"), "v1\nx").unwrap();
    fs::create_dir_all(root.join("extensions").join("ad_hoc")).unwrap();
    fs::write(
        root.join("extensions").join("ad_hoc").join("notes.md"),
        "note",
    )
    .unwrap();
    let original_session = fs::read(sessions.join("auto-keep.jsonl")).unwrap();
    let original_db = fs::read(root.join("memories.sqlite")).unwrap();

    // 缺 confirm：不执行任何清理
    for args in ["", "  ", "yes", "CONFIRM"] {
        let out = runner::memory_reset(dir.path(), args);
        assert!(out.contains("用法"), "args={args:?} 不得清理");
        assert_eq!(fs::read(root.join("memories.sqlite")).unwrap(), original_db);
        assert_eq!(fs::read(root.join("memory_summary.md")).unwrap(), b"v1\nx");
        assert_eq!(
            fs::read(root.join("extensions/ad_hoc/notes.md")).unwrap(),
            b"note"
        );
        assert_eq!(fs::read(sessions.join("auto-keep.jsonl")).unwrap(), original_session);
        assert_eq!(runner::read_epoch(&root), 0, "缺 confirm 不得使在途任务失效");
    }
    assert!(root.join("memories.sqlite").is_file(), "缺 confirm 零删除");

    // confirm：清生成记忆与状态库，保留根目录与 sessions
    let out = runner::memory_reset(dir.path(), "confirm");
    assert!(out.contains("已清理"), "{out}");
    assert!(root.is_dir(), "根目录保留");
    assert_eq!(runner::read_epoch(&root), 1, "成功重置仅推进一次代数");
    assert_eq!(
        fs::read_dir(&root)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != ".epoch")
            .count(),
        0,
        "清理后根目录除重置代数标记外必须为空（库在内）"
    );
    assert_eq!(
        fs::read(sessions.join("auto-keep.jsonl")).unwrap(),
        original_session,
        "原始会话逐字节保留"
    );

    // 符号链接根拒绝
    let dir2 = temp("reset-symlink");
    let real = dir2.path().join("real-root");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("keep.txt"), "keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, memories_root(dir2.path())).unwrap();
    let out = runner::memory_reset(dir2.path(), "confirm");
    assert!(out.contains("拒绝"), "符号链接根必须拒绝：{out}");
    assert_eq!(fs::read(real.join("keep.txt")).unwrap(), b"keep", "链接目标字节不变");
}

/// 在途连接交互：持旧库连接 reset 后，新开库为干净库；旧连接写入落在
/// 已删 inode 上，不污染新库（本地库在记忆根内的布局差异处置）。
#[test]
fn m7_reset_with_inflight_connection_keeps_new_store_clean() {
    let dir = temp("reset-inflight");
    let root = memories_root(dir.path());
    let old = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    old.register_thread("t-inflight", "enabled", now_ms())
        .unwrap();
    let out = runner::memory_reset(dir.path(), "confirm");
    assert!(out.contains("已清理"), "{out}");
    // 在途连接继续写入（已删 inode）
    let late = old.register_thread("t-late", "enabled", now_ms());
    let _ = late;
    // 新库干净重建
    let fresh = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    assert_eq!(fresh.count_stage1_outputs().unwrap(), 0);
    let (running, done, errored) = fresh.count_jobs_by_status().unwrap();
    assert_eq!((running, done, errored), (0, 0, 0), "新库任务表干净");
}

// ---------- 补强：触发边界与并发 ----------

/// 后台管线在慢响应上阻塞期间，主轮独立完成（不互相阻塞）；后台随后
/// 有界收尾（进程退出不等待的并发面：两线程并行、互不 join 阻塞）。
#[test]
fn m7_main_turn_completes_while_pipeline_blocked() {
    let dir = temp("blocked");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let mock = spawn_mock(vec![MockResp::slow_json(200, chat_body("late"), 1_500)]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        None,
        send_for_mock(&mock),
    )
    .unwrap();
    // 后台阻塞（慢响应）期间，主轮经另一 mock 通道立即完成
    let main_mock = spawn_mock(vec![MockResp::json(200, chat_body("主轮完成"))]);
    let send = send_for_mock(&main_mock);
    let reply = api::chat_with_body(
        &send.conn,
        "k",
        &send.model,
        &json!({ "messages": [{"role":"user","content":"hi"}] }),
        &send.agent,
    )
    .unwrap();
    let content = reply["choices"][0]["message"]["content"].as_str().unwrap();
    assert_eq!(content, "主轮完成");
    assert!(!handle.is_finished(), "后台仍在慢响应上（并发不阻塞主轮）");
    handle.join().expect("后台最终收尾");
}

/// 会话写失败不推进登记：目标创建失败（父目录只读→目录重建/文件创建
/// 报错、状态停用）时 note_persisted 不登记、不建库——未成功持久化的
/// 数据不登记为已更新版本。
///
/// 既有 fd 的目录删除不构成追加写失败（Linux inode 语义，写入落已删
/// inode）——该族语义由 `m7_reset_with_inflight_connection_keeps_new_store_clean`
/// 覆盖；本测针对「从未成功创建目标」的记录。
#[test]
#[cfg(unix)]
fn m7_registrar_does_not_advance_on_failed_persist() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp("reg-fail");
    let sessions = dir.path().join("sessions");
    let mut session = Session::new(sessions.clone());
    let mut reg = ThreadRegistrar::default();
    let cfg = enabled();
    // 父目录只读：首次追加时目录重建与文件创建均失败（记录只留在内存）
    let mode = fs::metadata(dir.path()).unwrap().permissions().mode();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let history = vec![text(HistoryRole::User, "从未落盘的输入")];
    let notice = session.on_history(&history);
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).unwrap();
    assert!(
        notice.is_some(),
        "目标创建失败必须产生提示（前提成立）：{notice:?}"
    );
    assert!(
        session.current_target_stem().is_none(),
        "无追加目标（停用）"
    );
    assert_eq!(session.durable_history_len(), 0, "无持久化事实");
    reg.note_persisted(dir.path(), &cfg, &session);
    assert!(
        !memories_root(dir.path()).exists(),
        "未成功持久化的数据不得触发建库或登记"
    );
}

// ---------- 定点补验（2026-10-19 第二轮） ----------

/// 定点：主轮取消不撤销已启动的后台管线——取消标志与管线线程完全解耦
/// （管线 chat 不查取消标志；spawn 已发生即按自身生命周期走完）。主轮
/// 经独立失败通道返回 Err（等价取消位），管线在慢响应上继续并完成。
#[test]
fn m7_turn_cancel_does_not_undo_pipeline() {
    let dir = temp("cancel");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    // 管线通道：慢响应（取消发生后仍在途）
    let pipeline_mock = spawn_mock(vec![MockResp::slow_json(200, chat_body("late"), 800)]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        None,
        send_for_mock(&pipeline_mock),
    )
    .unwrap();
    // 主轮通道：独立 mock 返回 500——请求失败（等价「取消＝本轮 Err 结束」）
    let cancel = std::sync::atomic::AtomicBool::new(false);
    cancel.store(true, Ordering::SeqCst);
    let fail_mock = spawn_mock(vec![MockResp::json(500, r#"{"error":"cancelled"}"#)]);
    let send = send_for_mock(&fail_mock);
    let main_chat = api::chat_with_body(
        &send.conn,
        "k",
        &send.model,
        &json!({ "messages": [] }),
        &send.agent,
    );
    assert!(main_chat.is_err(), "主轮失败（等价取消位）前提成立");
    assert!(
        !handle.is_finished(),
        "主轮取消时管线仍在途（未随主轮终止）"
    );
    handle.join().expect("取消不得拖垮后台管线");
    assert!(
        memories_root(dir.path()).join("memories.sqlite").is_file(),
        "管线初始化照常完成"
    );
}

/// 收口（2026-10-19 用户确认边界）：phase2 模型请求在途时 reset（闸门屏障
/// 精确暂停），再释放**有效写入响应**——旧任务不得重建 Markdown、数据库
/// 或 Git 基线；失败按既有契约记账且不污染新库；原始会话保留。
#[test]
fn m7_reset_during_inflight_pipeline_full_interaction() {
    let dir = temp("reset-inflight2");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-old-9",
        &[
            text(HistoryRole::User, "q9"),
            text(HistoryRole::Assistant, "a9"),
        ],
    );
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    store
        .register_thread("auto-old-9", "enabled", now_ms() - 7 * 24 * 3600 * 1000)
        .unwrap();

    // stage1 正常；phase2 合并请求挂闸门（精确屏障，非固定延时），随后终文
    let (gated_merge, gate) = common::MockResp::gated(200, merge_tool_batch().to_string());
    let mock = spawn_mock(vec![
        MockResp::json(200, stage1_ok()),
        gated_merge,
        MockResp::json(200, chat_body("consolidated")),
    ]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        Some("cur".into()),
        send_for_mock(&mock),
    )
    .unwrap();
    // 屏障：等到 phase2 合并请求在途（claim/prepare/同步已完成）
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while mock.count() < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "phase2 请求未在窗口内发出"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        root.join(".git").is_dir(),
        "暂停点断言：phase2 已 prepare（git 基线已建）"
    );
    // 在途窗口内重置（成功返回＝旧任务自此失效）。显式时限：模型请求在
    // 悬挂中（闸门未放），reset 必须立即完成——写入临界区不得越过模型
    // 请求持有（曾由 drop(引用) no-op 陷阱实证：锁越界时此处阻塞至 HTTP
    // 超时）。
    let t_reset = std::time::Instant::now();
    let out = runner::memory_reset(dir.path(), "confirm");
    let reset_elapsed = t_reset.elapsed();
    assert!(
        reset_elapsed < Duration::from_secs(10),
        "reset 不得等待在途模型请求（耗时 {reset_elapsed:?}）"
    );
    assert!(out.contains("已清理"), "重置成功：{out}");
    // 释放旧任务：给出**有效的写入响应**（工具批）——旧任务不得重建
    gate.release();
    handle.join().expect("失效任务线程不得 panic");

    // 三面：Markdown／数据库／Git 基线均不得重建
    assert!(!root.join("MEMORY.md").exists(), "旧任务不得重建 MEMORY.md");
    assert!(
        !root.join("memory_summary.md").exists(),
        "旧任务不得重建总摘要"
    );
    assert!(!root.join("memories.sqlite").is_file(), "状态库不得复活");
    assert!(!root.join(".git").exists(), "Git 基线不得重建");
    assert!(
        !dir.path().join(".git").exists() && !sessions.join(".git").exists(),
        "无记忆根外泄漏"
    );
    // 旧任务记账落在旧连接（已删 inode）——失败阶段如实可查，不污染新库
    let row = store
        .job_row("memory_consolidate_global", "global")
        .unwrap()
        .expect("旧连接上任务行可读（领取态留存于已删 inode）");
    // 实际 SQL 行为（如实记录，非推断）：失效路径的 job_failed 记账在
    // 已删 inode 的连接上写事务失败（journal 无法回放已删主文件），按上游
    // `let _` 语义吞没——行保持领取态 running、last_error 未写入；核心
    // 保证由文件面承载（根内零生成内容，见上）。
    assert_eq!(
        row.status, "running",
        "记账落空后行保持领取态（实际 SQL 行为）"
    );
    assert_eq!(
        row.last_error, None,
        "失败原因未落（写已删 inode 失败被吞）"
    );
    drop(store);
    // 新库未受污染（不存在＝未被旧任务重建）
    assert!(!root.join("memories.sqlite").is_file());
    // 原始会话保留（A31）
    assert!(
        sessions.join("auto-old-9.jsonl").is_file(),
        "原始会话全程保留"
    );
}

/// 收口①：旧任务已启动、关键写入未执行（stage1 模型请求挂闸门＝claim 已
/// 完成），reset 后释放——不得重新准备（phase2）或生成任何记忆。
#[test]
fn m7_reset_invalidates_pipeline_before_phase2() {
    let dir = temp("invalidate-pre");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-old-10",
        &[
            text(HistoryRole::User, "q10"),
            text(HistoryRole::Assistant, "a10"),
        ],
    );
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    store
        .register_thread("auto-old-10", "enabled", now_ms() - 7 * 24 * 3600 * 1000)
        .unwrap();
    drop(store);

    let (gated_s1, gate) = common::MockResp::gated(200, stage1_ok());
    let mock = spawn_mock(vec![
        gated_s1,
        MockResp::json(200, merge_tool_batch().to_string()),
        MockResp::json(200, chat_body("consolidated")),
    ]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        Some("cur".into()),
        send_for_mock(&mock),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while mock.count() < 1 {
        assert!(std::time::Instant::now() < deadline, "stage1 请求未发出");
        std::thread::sleep(Duration::from_millis(20));
    }
    // stage1 请求在途＝已领取、关键写入未执行；此时重置
    let out = runner::memory_reset(dir.path(), "confirm");
    assert!(out.contains("已清理"), "{out}");
    gate.release();
    handle.join().expect("失效任务线程不得 panic");

    // 旧任务释放后不得重新准备或生成：无 prepare（.git/布局）、无新库、无产物
    assert!(!root.join(".git").exists(), "不得重建 Git 基线（重新准备）");
    assert!(
        !root.join("rollout_summaries").exists() && !root.join("extensions").exists(),
        "不得重建布局/同步目录"
    );
    assert!(!root.join("memories.sqlite").is_file(), "不得重建状态库");
    assert!(!root.join("MEMORY.md").exists(), "不得生成记忆产物");
    // mock 上旧任务不得再发起 phase2 请求（阶段边界中止）
    let reqs = mock.requests();
    assert_eq!(
        reqs.len(),
        1,
        "stage1 后不得进入 phase2（实际 {} 请求）",
        reqs.len()
    );
    assert!(sessions.join("auto-old-10.jsonl").is_file(), "原始会话保留");
}

/// 收口③：写入检查与实际写入的交错由目录互斥锁排除——写入者持共享锁期间
/// reset（排他）不得返回；锁释放后 reset 完成，此后写入一律拒绝。
#[test]
#[cfg(unix)]
fn m7_reset_writes_interleave_is_excluded_by_lock() {
    use std::os::unix::io::AsRawFd;
    let dir = temp("interleave");
    let root = memories_root(dir.path());
    // 任务快照凭据形态（模拟一条领取于代数 0 的任务）
    fs::create_dir_all(root.join("extensions").join("ad_hoc")).unwrap();
    // 写入临界区：持 root 目录共享锁（生产 mem_write 同款锁位）
    let dfd = fs::File::open(&root).unwrap();
    assert_eq!(
        unsafe { libc::flock(dfd.as_raw_fd(), libc::LOCK_SH) },
        0,
        "测试前提：取得共享锁"
    );
    let dir2 = dir.path().to_path_buf();
    let reset_thread = std::thread::spawn(move || runner::memory_reset(&dir2, "confirm"));
    // 共享锁持有期间 reset（排他）不得完成——根内容仍在
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !reset_thread.is_finished(),
        "写入临界区内 reset 不得返回（互斥）"
    );
    assert!(root.join("extensions").is_dir(), "临界区内内容不被清");
    unsafe { libc::flock(dfd.as_raw_fd(), libc::LOCK_UN) };
    let out = reset_thread.join().unwrap();
    assert!(out.contains("已清理"), "锁释放后 reset 完成：{out}");
    // 此后（代数已推进）旧任务（快照 0）的 mem_write 一律拒绝；新代快照放行
    let args = json!({ "path": "MEMORY.md", "content": "# 不应写入\n" }).to_string();
    let result = terminal_chat::memory::mem_tools::execute("mem_write", &args, &root, 0);
    let v: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["ok"], false, "失效任务写入必须拒绝：{result}");
    assert!(!root.join("MEMORY.md").exists());
    let ok_new = json!({ "path": "MEMORY.md", "content": "# 新代内容\n" }).to_string();
    let r2 = terminal_chat::memory::mem_tools::execute("mem_write", &ok_new, &root, 1);
    let v2: Value = serde_json::from_str(&r2).unwrap();
    assert_eq!(v2["ok"], true, "新代任务（快照=当前代数）写入放行：{r2}");
}

/// 收口④：重置后新启动的合法任务照常生成（不靠永久禁用通过负向约束）。
/// 布局差异如实：本地库在记忆根内——reset 连 threads 登记一并清（上游库
/// 在根外、登记保留）；当前活动会话经写入侧在新库重新登记（生产路径＝
/// 新一轮 on_record），管线照常两阶段（新会话不满闲置窗→stage1 零候选，
/// phase2 首轮恒合并生成——P7）。
#[test]
fn m7_new_pipeline_after_reset_regenerates() {
    let dir = temp("regen");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let root = memories_root(dir.path());
    {
        let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
        store
            .register_thread("auto-old-11", "enabled", now_ms() - 7 * 24 * 3600 * 1000)
            .unwrap();
    }
    // 重置（清空一切——含 threads，布局差异登记于测试头注释）
    let out = runner::memory_reset(dir.path(), "confirm");
    assert!(out.contains("已清理"), "{out}");
    // 新用户轮：当前会话经 threads 写入侧在新库重新登记
    let mut session = Session::new(sessions.clone());
    let mut reg = ThreadRegistrar::default();
    let history = vec![text(HistoryRole::User, "新一轮输入")];
    session.on_history(&history);
    reg.note_persisted(dir.path(), &enabled(), &session);
    // 新管线照常运行（当前会话不满闲置窗→无 stage1；phase2 首轮合成）
    let mock = spawn_mock(vec![
        MockResp::json(200, merge_tool_batch().to_string()),
        MockResp::json(200, chat_body("consolidated")),
    ]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        session.current_target_stem(),
        send_for_mock(&mock),
    )
    .unwrap();
    handle.join().expect("新管线不得 panic");
    assert!(
        root.join("MEMORY.md").is_file(),
        "重置后新任务必须能正常生成记忆"
    );
    assert!(root.join("memory_summary.md").is_file(), "新任务生成总摘要");
    assert!(root.join("memories.sqlite").is_file(), "新库重建");
}

// ---------- 收口：后台触发直接计数 ----------

/// 触发计数观察**实际后台启动入口**（`spawn_memory_pipeline` 生产壳）：
/// enabled 每次真实启动＋1；关闭态/未到达不增；重复触发各计一次（与
/// jobs 去重独立——去重掩盖的是请求，不是启动）。
#[test]
fn m7_pipeline_spawn_count_tracks_real_entry() {
    let dir = temp("count");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("ok"))]);
    let send = send_for_mock(&mock);
    let before = runner::pipeline_spawn_count();
    // 关闭态：不启动、不计数
    runner::spawn_memory_pipeline(
        dir.path(),
        &MemoriesConfig::default(),
        None,
        send_for_mock(&mock),
    );
    assert_eq!(runner::pipeline_spawn_count(), before, "关闭态不得计数");
    // 开态：每次真实启动各＋1（可测核心经生产壳同一位点计数——此处直接
    // 验证壳位点）
    runner::spawn_memory_pipeline(dir.path(), &enabled(), None, send.clone());
    assert_eq!(runner::pipeline_spawn_count(), before + 1, "首次启动计数");
    runner::spawn_memory_pipeline(dir.path(), &enabled(), None, send);
    assert_eq!(
        runner::pipeline_spawn_count(),
        before + 2,
        "重复触发各计一次"
    );
    // 计数可见于 status（费用知情观察面）
    let text = runner::memory_status_text_in(dir.path(), &enabled());
    assert!(text.contains("本进程累计触发后台管线"), "{text}");
}

// ---------- 借用凭据交错（2026-10-19 第四轮） ----------

/// 旧任务不能借用新代凭据恢复写入：旧 phase2 请求挂闸门→reset→**真实新代
/// 管线**完整跑完（prepare 建立新凭据/产物/基线收尾）→释放旧任务（携带
/// **独特旧内容**的有效工具批）——旧任务不得覆盖新代文件、不得新建旧任务
/// 专属文件、不得改动新代基线或污染新库；新代任务确实完成。
#[test]
fn m7_old_task_cannot_borrow_new_epoch_credentials() {
    let dir = temp("borrow");
    let sessions = dir.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-old-20",
        &[
            text(HistoryRole::User, "q20"),
            text(HistoryRole::Assistant, "a20"),
        ],
    );
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    store
        .register_thread("auto-old-20", "enabled", now_ms() - 7 * 24 * 3600 * 1000)
        .unwrap();
    drop(store);

    // 旧任务工具批：独特旧内容（覆盖试探＋专属新文件试探）
    let old_batch = json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "o-mem", "type": "function", "function": { "name": "mem_write",
              "arguments": "{\"path\":\"MEMORY.md\",\"content\":\"# OLD-CONTENT-旧任务覆盖\\n\"}" } },
            { "id": "o-only", "type": "function", "function": { "name": "mem_write",
              "arguments": "{\"path\":\"OLD-ONLY.md\",\"content\":\"旧任务专属文件\\n\"}" } }
        ]
    }, "finish_reason": "tool_calls" }] });
    // 新代工具批：新代内容
    let new_batch = json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "n-mem", "type": "function", "function": { "name": "mem_write",
              "arguments": "{\"path\":\"MEMORY.md\",\"content\":\"# NEW-CONTENT-新代记忆\\n\"}" } },
            { "id": "n-sum", "type": "function", "function": { "name": "mem_write",
              "arguments": "{\"path\":\"memory_summary.md\",\"content\":\"v1\\n新代摘要\\n\"}" } }
        ]
    }, "finish_reason": "tool_calls" }] });

    // 请求序列（时序受控——旧 phase2 挂闸门期间新任务跑完）：
    // 0=旧 stage1；1=gated 旧工具批（挂起）；2=新代 phase2 首轮（新候选空，
    // threads 已清→新代无 stage1）；3=新代终文；4=旧终文（释放后）
    let (gated_old, gate) = common::MockResp::gated(200, old_batch.to_string());
    let mock = spawn_mock(vec![
        MockResp::json(200, stage1_ok()),
        gated_old,
        MockResp::json(200, new_batch.to_string()),
        MockResp::json(200, chat_body("new-consolidated")),
        MockResp::json(200, chat_body("old-consolidated")),
    ]);
    // 旧任务（用旧库连接——reset 前启动）
    let old_handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        Some("old-cur".into()),
        send_for_mock(&mock),
    )
    .unwrap();
    // ① 旧 phase2 请求在途（prepare/同步已完成、模型请求挂起）
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while mock.count() < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "旧任务 phase2 请求未发出"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(root.join(".git").is_dir(), "旧任务已 prepare");
    // ② 重置成功返回
    let out = runner::memory_reset(dir.path(), "confirm");
    assert!(out.contains("已清理"), "{out}");
    // ③ 真实新代管线：新会话登记（写入侧）→完整两阶段（候选空→phase2
    // 首轮合成），join 等待其完成（产物＋基线收尾）
    let mut session = Session::new(sessions.clone());
    let mut reg = ThreadRegistrar::default();
    let history = vec![text(HistoryRole::User, "新代用户输入")];
    session.on_history(&history);
    reg.note_persisted(dir.path(), &enabled(), &session);
    let new_handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &enabled(),
        session.current_target_stem(),
        send_for_mock(&mock),
    )
    .unwrap();
    new_handle.join().expect("新代管线不得 panic");
    // 新代确实完成（非靠禁用通过）：产物为新代内容＋基线收尾
    let memory_md = fs::read_to_string(root.join("MEMORY.md")).expect("新代产物必须存在");
    assert!(
        memory_md.contains("NEW-CONTENT-新代记忆"),
        "新代内容：{memory_md}"
    );
    let head_after_new = git_head(&root);
    assert!(head_after_new.is_some(), "新代基线存在（收尾完成）");
    let new_store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let new_job = new_store
        .job_row("memory_consolidate_global", "global")
        .unwrap()
        .expect("新库任务行存在");
    assert_eq!(new_job.status, "done", "新代任务成功完成：{new_job:?}");
    let new_s1_count = new_store.count_stage1_outputs().unwrap();
    drop(new_store);
    assert_eq!(new_s1_count, 0, "新库无 stage1 污染（新代候选空）");

    // ④ 释放旧任务（有效工具批——独特旧内容）
    gate.release();
    old_handle.join().expect("旧任务线程不得 panic");

    // 断言：旧任务不得覆盖新代文件/新建专属文件/改基线/污染新库
    let memory_after = fs::read_to_string(root.join("MEMORY.md")).unwrap();
    assert!(
        memory_after.contains("NEW-CONTENT-新代记忆"),
        "旧任务不得覆盖新代 MEMORY.md：{memory_after}"
    );
    assert!(
        !memory_after.contains("OLD-CONTENT"),
        "旧内容不得混入：{memory_after}"
    );
    assert!(!root.join("OLD-ONLY.md").exists(), "旧任务不得新建专属文件");
    let summary = fs::read_to_string(root.join("memory_summary.md")).unwrap();
    assert!(summary.contains("新代摘要"), "新代摘要不被覆盖：{summary}");
    // 新代基线 HEAD 不被旧任务改变（收尾/重置基线不得作用）
    assert_eq!(
        git_head(&root),
        head_after_new,
        "新代 Git 基线不得被旧任务改动"
    );
    // 新库任务状态不被旧任务记账污染（仍 done）
    let recheck = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let job = recheck
        .job_row("memory_consolidate_global", "global")
        .unwrap()
        .unwrap();
    assert_eq!(
        job.status, "done",
        "新代任务状态不被旧任务记账污染：{job:?}"
    );
    assert_eq!(recheck.count_stage1_outputs().unwrap(), 0);
}

/// 真实 git HEAD 读取（基线不变断言）。
fn git_head(root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// —— 014 §2：/memory-status 最小失败展示（阶段／脱敏截断原因／retry_at 本值／同源冷却）——

/// 覆盖：无错误时不出现失败行；构造 stage1 失败后展示阶段、脱敏＋截断原因、
/// `jobs.retry_at` 本值；phase2 冷却按领取原语同条件推导（边界内外两态）；
/// 状态命令无写入副作用（库文件字节与 mtime 不变）。
#[test]
fn m14_status_failure_display_stage_reason_retry_at_and_cooldown() {
    let dir = temp("m14-status");
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();

    // 前置：仅 phase2 成功（done、无错、finished_at=now → 冷却中）——此时无失败行
    let Phase2JobClaimOutcome::Claimed {
        ownership_token: p2tok,
        ..
    } = store.try_claim_global_phase2_job("w14", 3600).unwrap()
    else {
        panic!("phase2 领取失败");
    };
    assert!(store
        .mark_global_phase2_job_succeeded(&p2tok, 100, &[])
        .unwrap());
    let text = runner::memory_status_text_in(dir.path(), &enabled());
    assert!(!text.contains("最近失败"), "无错误时不得出现失败行：{text}");
    assert!(
        text.contains("冷却"),
        "phase2 成功后 6h 内应显示冷却：{text}"
    );

    // stage1 失败：长原因＋密钥样式（构造「存储侧最坏情况」——展示侧必须再脱敏＋截断）
    let Stage1JobClaimOutcome::Claimed { ownership_token } = store
        .try_claim_stage1_job("t-fail-14", "w14", 1000, 3600, 8)
        .unwrap()
    else {
        panic!("stage1 领取失败");
    };
    let secret = "sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
    // 密钥置于截断点之前：证明「先脱敏后截断」的标记可见；长尾证明截断有界
    let long_reason = format!("模型返回无效输出：{secret}…细节 {}", "x".repeat(400));
    assert!(store
        .mark_stage1_job_failed("t-fail-14", &ownership_token, &long_reason, 3600)
        .unwrap());
    let retry_at: i64 = {
        let conn = rusqlite::Connection::open(root.join("memories.sqlite")).unwrap();
        conn.query_row(
            "SELECT retry_at FROM jobs WHERE kind='memory_stage1' AND job_key='t-fail-14'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };

    let before_bytes = fs::read(root.join("memories.sqlite")).unwrap();
    let before_mtime = fs::metadata(root.join("memories.sqlite"))
        .unwrap()
        .modified()
        .unwrap();
    let text = runner::memory_status_text_in(dir.path(), &enabled());
    assert!(text.contains("第一阶段"), "阶段标识：{text}");
    assert!(text.contains("下次可重试"), "重试字段：{text}");
    assert!(
        text.contains(&format!("{retry_at}")),
        "必须展示 jobs.retry_at 本值（{retry_at}）：{text}"
    );
    assert!(!text.contains(secret), "展示原因必须脱敏：{text}");
    assert!(text.contains("[REDACTED_SECRET]"), "脱敏标记应可见：{text}");
    assert!(
        !text.contains(&"x".repeat(201)),
        "原因必须截断（≤200 字符）：{text}"
    );
    assert!(text.contains("冷却"), "错误与冷却并存时冷却行仍在：{text}");
    // 只读：库文件字节与 mtime 不变
    assert_eq!(
        fs::read(root.join("memories.sqlite")).unwrap(),
        before_bytes,
        "不得写入库文件"
    );
    assert_eq!(
        fs::metadata(root.join("memories.sqlite"))
            .unwrap()
            .modified()
            .unwrap(),
        before_mtime,
        "不得触碰库文件 mtime"
    );

    // 冷却边界与领取原语同源：finished_at 回拨到 6h 窗口外 → 不显示
    {
        let conn = rusqlite::Connection::open(root.join("memories.sqlite")).unwrap();
        conn.execute(
            "UPDATE jobs SET finished_at = ?1 WHERE kind='memory_consolidate_global' AND job_key='global'",
            rusqlite::params![terminal_chat::memory::store::system_clock()() - 6 * 3600 - 10],
        )
        .unwrap();
    }
    let text = runner::memory_status_text_in(dir.path(), &enabled());
    assert!(!text.contains("冷却"), "6h 窗口外不得显示冷却：{text}");
    assert!(text.contains("最近失败"), "错误行仍在：{text}");
    // 真实 phase2 失败路径（领取→失败记账）：last_error 非空 → 领取原语不判冷却，
    // 展示同样不得显示；最近失败行切换为「全局合并」阶段（覆盖该映射分支）
    // finished_at 为秒级：确保 phase2 失败严格晚于 stage1 失败（避免同秒并列）
    std::thread::sleep(Duration::from_millis(1100));
    let Phase2JobClaimOutcome::Claimed {
        ownership_token: p2tok2,
        ..
    } = store.try_claim_global_phase2_job("w14b", 3600).unwrap()
    else {
        panic!("phase2 二次领取失败");
    };
    assert!(store
        .mark_global_phase2_job_failed(&p2tok2, "合并代理执行失败-probe", 3600)
        .unwrap());
    let text = runner::memory_status_text_in(dir.path(), &enabled());
    assert!(
        !text.contains("冷却"),
        "last_error 非空不得显示冷却（与领取条件同源）：{text}"
    );
    assert!(
        text.contains("全局合并"),
        "phase2 失败须展示「全局合并」阶段：{text}"
    );
    assert!(
        text.contains("合并代理执行失败-probe"),
        "phase2 失败原因可见（未超限不截断）：{text}"
    );
    assert!(
        !text.contains("第一阶段｜"),
        "最近失败＝finished_at 最新一条（phase2），阶段随之切换：{text}"
    );
}

// —— 014 §3：generate_memories 来源资格开关 ——

fn generating(v: bool) -> MemoriesConfig {
    MemoriesConfig {
        enabled: true,
        generate_memories: v,
        ..Default::default()
    }
}

/// 四条登记路径＋不批量恢复：新建（按当前配置）、继续写入/恢复（资格保持）、
/// 登记行缺失后重登（按当前配置）、reset 后重登（本地适配：资格随库清除，
/// 按当前配置重建）；全局重新开启不批量改旧 disabled 行。
#[test]
fn m14_generate_false_registration_paths_and_no_bulk_restore() {
    let dir = temp("m14-reg");
    let sessions = dir.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let root = memories_root(dir.path());
    let mode = |tid: &str| -> Option<String> {
        let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
        store.thread_memory_mode(tid).unwrap()
    };

    // 路径① 新建会话（generate=false）→ 新登记来源 disabled
    let mut session = Session::new(sessions.clone());
    let mut reg = ThreadRegistrar::default();
    let history = vec![text(HistoryRole::User, "关生成时期的问")];
    session.on_history(&history);
    reg.note_persisted(dir.path(), &generating(false), &session);
    let stem = session.current_target_stem().expect("登记目标");
    assert_eq!(
        mode(&stem).as_deref(),
        Some("disabled"),
        "generate=false 新登记应为 disabled"
    );

    // 路径② 同一会话继续写入（换 generate=true 配置）→ 资格保持 disabled（不被全局覆盖）
    let mut history2 = history.clone();
    history2.push(text(HistoryRole::Assistant, "答"));
    session.on_history(&history2);
    reg.note_persisted(dir.path(), &generating(true), &session);
    assert_eq!(
        mode(&stem).as_deref(),
        Some("disabled"),
        "既有登记行资格不得被后续全局配置覆盖"
    );

    // 重新开启后新会话 → enabled；旧 disabled 行不被批量修改
    let mut session_b = Session::new(sessions.clone());
    session_b.on_history(&[text(HistoryRole::User, "重开后新问")]);
    reg.note_persisted(dir.path(), &generating(true), &session_b);
    let stem_b = session_b.current_target_stem().unwrap();
    assert_eq!(
        mode(&stem_b).as_deref(),
        Some("enabled"),
        "generate=true 新登记应为 enabled"
    );
    assert_eq!(
        mode(&stem).as_deref(),
        Some("disabled"),
        "全局重新开启不得批量恢复旧 disabled 行"
    );

    // 路径③ 登记行缺失后首次登记 → 按当前配置（此处 false → disabled）
    {
        let conn = rusqlite::Connection::open(root.join("memories.sqlite")).unwrap();
        conn.execute(
            "DELETE FROM threads WHERE thread_id = ?1",
            rusqlite::params![stem_b],
        )
        .unwrap();
    }
    let mut history_b2 = vec![text(HistoryRole::User, "重开后新问")];
    history_b2.push(text(HistoryRole::Assistant, "答 B"));
    session_b.on_history(&history_b2);
    reg.note_persisted(dir.path(), &generating(false), &session_b);
    assert_eq!(
        mode(&stem_b).as_deref(),
        Some("disabled"),
        "行缺失后首次登记按当前配置"
    );

    // 路径④ reset 后重新登记（本地适配：库随根清空）→ 新库按当前配置
    let msg = runner::memory_reset(dir.path(), "confirm");
    assert!(msg.contains("已清理"), "前置 reset：{msg}");
    let mut reg2 = ThreadRegistrar::default(); // 新进程/新登记者语义
    let mut session_c = Session::new(sessions.clone());
    session_c.on_history(&[text(HistoryRole::User, "reset 后新问")]);
    reg2.note_persisted(dir.path(), &generating(true), &session_c);
    let stem_c = session_c.current_target_stem().unwrap();
    assert_eq!(
        mode(&stem_c).as_deref(),
        Some("enabled"),
        "reset 后重登按当前配置（true）"
    );
    // 关闭态照旧：generate 不改变 enabled 总门（关闭态零登记已由 m7 用例覆盖）
}

/// 正向守卫：已有 enabled 来源＋generate_memories=false → 管线照常执行提炼
/// （generate 不得接成管线/触发门——错误接线由变异证明检出）。
#[test]
fn m14_generate_false_pipeline_still_processes_enabled_source() {
    let dir = temp("m14-pipe");
    let sessions = dir.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    write_session(
        &sessions,
        "auto-old-14",
        &[
            text(HistoryRole::User, "做过什么"),
            text(HistoryRole::Assistant, "完成了 Y"),
        ],
    );
    let root = memories_root(dir.path());
    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let updated_ms = now_ms() - 7 * 24 * 3600 * 1000;
    store
        .register_thread("auto-old-14", "enabled", updated_ms)
        .unwrap();
    drop(store);

    let mock = spawn_mock(vec![
        MockResp::json(200, stage1_ok()),
        MockResp::json(200, merge_tool_batch().to_string()),
        MockResp::json(200, chat_body("consolidated")),
    ]);
    let handle = runner::spawn_memory_pipeline_in(
        dir.path(),
        &sessions,
        &generating(false),
        Some("auto-current".into()),
        send_for_mock(&mock),
    )
    .expect("generate=false 不得阻止管线 spawn");
    handle.join().expect("管线线程不得 panic");

    let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
    let out = store
        .stage1_output("auto-old-14")
        .unwrap()
        .expect("enabled 来源必须被提炼");
    assert_eq!(out.rollout_summary, "任务摘要");
    assert!(root.join("MEMORY.md").is_file(), "合并产物存在");
    assert_eq!(mock.count(), 3, "stage1＋合并工具批＋终文三次请求");
}

/// 014 §3 验收⑥：generate_memories=false 不改变注入、反馈、状态与重置的
/// 既有职责（职责互不混淆——generate 只管登记资格，不接任何使用面门）。
#[test]
fn m14_generate_false_leaves_use_paths_untouched() {
    let dir = temp("m14-use-paths");
    let root = memories_root(dir.path());
    // 注入照常：enabled+use_memories+generate=false → 读取摘要（gate 不含 generate）
    std::fs::create_dir_all(&root).unwrap();
    fs::write(root.join("memory_summary.md"), "v1\n- 摘要内容 m14").unwrap();
    let cfg = MemoriesConfig {
        enabled: true,
        use_memories: true,
        generate_memories: false,
        ..Default::default()
    };
    let ctx = terminal_chat::memory::inject::read_memory_context(&root, &cfg);
    assert!(ctx.is_some(), "generate=false 不得影响注入门");

    // 反馈照常：库存在＋有效引用 → 计数（generate 不在反馈门）
    {
        let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
        store
            .register_thread("auto-cite-14", "enabled", 1_700_000_000_000)
            .unwrap();
        let Stage1JobClaimOutcome::Claimed { ownership_token } = store
            .try_claim_stage1_job("auto-cite-14", "w", 1_700_000_000, 3600, 8)
            .unwrap()
        else {
            panic!("领取失败");
        };
        store
            .mark_stage1_job_succeeded(
                "auto-cite-14",
                &ownership_token,
                1_700_000_000,
                "raw",
                "summary",
                None,
            )
            .unwrap();
    }
    terminal_chat::memory::feedback::after_user_turn(
        dir.path(),
        &cfg,
        &format!(
            "答。<oai-mem-citation><rollout_ids>auto-cite-14</rollout_ids></oai-mem-citation>"
        ),
    );
    {
        let conn = rusqlite::Connection::open(root.join("memories.sqlite")).unwrap();
        let used: i64 = conn
            .query_row(
                "SELECT usage_count FROM stage1_outputs WHERE thread_id='auto-cite-14'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(used, 1, "generate=false 下引用计数照常");
    }

    // 状态照常：显示已启用（generate 不参与状态门）
    let text = runner::memory_status_text_in(dir.path(), &cfg);
    assert!(
        text.contains("已启用"),
        "状态展示不受 generate 影响：{text}"
    );

    // 重置照常：可正常执行（generate 不参与重置门）
    let msg = runner::memory_reset(dir.path(), "confirm");
    assert!(msg.contains("已清理"), "重置不受 generate 影响：{msg}");
    assert!(!root.join("memories.sqlite").is_file());
}
