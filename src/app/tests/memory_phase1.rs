//! 013-M3 集成测试：第一阶段提炼（上游 openai/codex@ce0323f 语义移植）。
//!
//! 时间全部经注入时钟固定；目录用独立临时目录，结束清理；模型全部 mock
//! （闭包注入或本地 HTTP mock——`common::spawn_mock`，覆盖真实请求组装路径，
//! 不调用真实模型）；断言同时覆盖返回值、数据库状态与请求体内容。

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use terminal_chat::catalog::{EffectiveConnection, ModelInfo};
use terminal_chat::memory::{
    claim_stage1_jobs_for_startup, redact_secrets, read_recent_rollout_items,
    rollout_token_limit, run_stage1_job, serialize_filtered_history_items,
    Stage1JobClaimOutcome, Stage1JobOutcome, MemoryStore, Stage1JobClaim,
    DEFAULT_ROLLOUT_TOKEN_LIMIT, MAX_RECORD_BYTES, MAX_ROLLOUT_BYTES,
};
use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord, ToolDisplay};
use terminal_chat::api;

const T0: i64 = 1_700_000_000;

// —— 环境与构造辅助 ——

struct SteppedClock(Arc<Mutex<i64>>);

impl SteppedClock {
    fn at(t: i64) -> Self {
        Self(Arc::new(Mutex::new(t)))
    }
    fn clock(&self) -> terminal_chat::memory::Clock {
        let t = Arc::clone(&self.0);
        Arc::new(move || *t.lock().unwrap())
    }
    fn advance(&self, secs: i64) {
        *self.0.lock().unwrap() += secs;
    }
}

struct Env {
    tmp: common::TempDir,
    sessions: PathBuf,
    clock: SteppedClock,
    store: MemoryStore,
}

fn env(tag: &str) -> Env {
    let tmp = common::TempDir::new(&format!("m3-{tag}"));
    let sessions = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let clock = SteppedClock::at(T0);
    let store = MemoryStore::open(&tmp.path().join("memories"), clock.clock()).unwrap();
    Env { tmp, sessions, clock, store }
}

fn text(role: HistoryRole, content: &str) -> HistoryItem {
    HistoryItem::Text(HistoryMessage { role, content: content.to_string(), user_input_event: None })
}

fn user_msg(content: &str) -> HistoryItem {
    text(HistoryRole::User, content)
}

fn assistant_msg(content: &str) -> HistoryItem {
    text(HistoryRole::Assistant, content)
}

fn tool_result(id: &str, content: &str) -> HistoryItem {
    HistoryItem::ToolResult {
        image: None,
        tool_call_id: id.to_string(),
        content: content.to_string(),
        display: None,
    }
}

/// 落一个合成会话文件：首行 meta `{"v":1}`＋逐行 HistoryItem JSON。
fn write_session(dir: &Path, name: &str, items: &[HistoryItem]) -> PathBuf {
    let path = dir.join(format!("{name}.jsonl"));
    let mut body = String::from("{\"v\":1}\n");
    for item in items {
        body.push_str(&serde_json::to_string(item).unwrap());
        body.push('\n');
    }
    std::fs::write(&path, body).unwrap();
    path
}

/// 直接领取并构造 M2 形状的候选（M3 输入契约）；session_path 须为真实会话文件。
fn claim_direct(store: &MemoryStore, thread: &str, source: i64, session_path: &Path) -> Stage1JobClaim {
    match store
        .try_claim_stage1_job(thread, "cur", source, 3600, 8)
        .unwrap()
    {
        Stage1JobClaimOutcome::Claimed { ownership_token } => Stage1JobClaim {
            thread_id: thread.to_string(),
            session_path: session_path.to_path_buf(),
            updated_at_ms: source.saturating_mul(1000),
            source_updated_at: source,
            ownership_token,
        },
        other => panic!("布置失败：应可领取，实际 {other:?}"),
    }
}

/// 模型响应体：content＝给定文本。
fn ok_response(content: &str) -> Value {
    json!({
        "choices": [{
            "message": {"content": content},
            "finish_reason": "stop"
        }]
    })
}

const VALID_OUTPUT: &str =
    r#"{"raw_memory":"经验正文","rollout_summary":"任务摘要","rollout_slug":"slug-a"}"#;
const EMPTY_OUTPUT: &str = r#"{"raw_memory":"","rollout_summary":"","rollout_slug":""}"#;

fn model_with_window(context_length: Option<u64>) -> ModelInfo {
    ModelInfo {
        id: "m3-model".to_string(),
        context_length,
        ..ModelInfo::default()
    }
}

/// 运行一个提炼任务：chat 闭包返回完整响应 JSON（或错误）；捕获发出的请求体。
fn run_job(
    env: &Env,
    claim: &Stage1JobClaim,
    model: &ModelInfo,
    reply: &str,
) -> (Stage1JobOutcome, Arc<Mutex<Option<Value>>>) {
    let seen: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let seen2 = Arc::clone(&seen);
    let outcome = run_stage1_job(&env.store, claim, model, "/tmp/proj", &move |req| {
        *seen2.lock().unwrap() = Some(req.clone());
        Ok(ok_response(reply))
    });
    (outcome, seen)
}

fn job_row(env: &Env, thread: &str) -> terminal_chat::memory::JobRow {
    env.store.job_row("memory_stage1", thread).unwrap().unwrap()
}

// —— A11：输出合同 ——

#[test]
fn m3_a11_schema_violations_mark_failed_with_retry() {
    let e = env("a11");

    // 三类违规：非 JSON、缺字段/多字段、类型错误——全部失败＋重试记账。
    for bad in [
        "这不是 JSON",
        r#"{"rollout_summary":"s"}"#,
        r#"{"raw_memory":"m","rollout_summary":"s","rollout_slug":"x","extra":1}"#,
        r#"{"raw_memory":"m","rollout_summary":123,"rollout_slug":"x"}"#,
    ] {
        // 每次违规用独立线程布置（互不影响重试状态）。
        let thread = format!("t{}", bad.len());
        let path = write_session(&e.sessions, &thread, &[user_msg("hi")]);
        let claim = claim_direct(&e.store, &thread, T0 - 100, &path);
        let _ = path;
        let (outcome, _) = run_job(&e, &claim, &model_with_window(None), bad);
        assert_eq!(outcome, Stage1JobOutcome::Failed, "违规输出必须失败：{bad}");
        let row = job_row(&e, &thread);
        assert_eq!(row.status, "error");
        assert_eq!(row.retry_at, Some(T0 + 3_600), "重试时间＝now+3600");
        assert_eq!(row.retry_remaining, 2, "失败递减重试计数");
        assert!(row.last_error.as_deref().unwrap_or("").contains("解析"), "错误信息应说明解析失败");
        assert!(e.store.stage1_output(&thread).unwrap().is_none(), "失败不得写入结果");
    }
}

// —— A12：三态 ——

#[test]
fn m3_a12_valid_output_persists_and_replaces() {
    let e = env("a12v");
    let path = write_session(
        &e.sessions,
        "t",
        &[user_msg("问题"), assistant_msg("回答")],
    );
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);
    let _ = path;

    let (outcome, _) = run_job(&e, &claim, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput);

    let row = job_row(&e, "t");
    assert_eq!(row.status, "done");
    assert_eq!(row.last_success_watermark, Some(T0 - 100));
    let out = e.store.stage1_output("t").unwrap().unwrap();
    assert_eq!(out.raw_memory, "经验正文");
    assert_eq!(out.rollout_summary, "任务摘要");
    assert_eq!(out.rollout_slug.as_deref(), Some("slug-a"));
    assert_eq!(out.source_updated_at, T0 - 100);
    // 有效结果推进全局合并任务（M1 语义）。
    assert!(
        e.store.job_row("memory_consolidate_global", "global").unwrap().is_some(),
        "mark_succeeded 应入队全局合并"
    );

    // 更新后重新提炼：新版本替换旧结果。
    e.store.register_thread("t", "enabled", (T0 - 50) * 1000).unwrap();
    let claim2 = claim_direct(&e.store, "t", T0 - 50, &path);
    let (outcome2, _) = run_job(
        &e,
        &claim2,
        &model_with_window(None),
        r#"{"raw_memory":"新正文","rollout_summary":"新摘要","rollout_slug":"slug-b"}"#,
    );
    assert_eq!(outcome2, Stage1JobOutcome::SucceededWithOutput);
    let out2 = e.store.stage1_output("t").unwrap().unwrap();
    assert_eq!(out2.raw_memory, "新正文");
    assert_eq!(out2.source_updated_at, T0 - 50, "旧结果只被更新版本替换");
}

#[test]
fn m3_a12_empty_output_without_old_result() {
    let e = env("a12e");
    let path = write_session(&e.sessions, "t", &[user_msg("闲聊")]);
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);
    let _ = path;

    let (outcome, _) = run_job(&e, &claim, &model_with_window(None), EMPTY_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::SucceededNoOutput);
    let row = job_row(&e, "t");
    assert_eq!(row.status, "done");
    assert_eq!(row.last_success_watermark, Some(T0 - 100), "空结果也记成功水位");
    assert!(e.store.stage1_output("t").unwrap().is_none(), "无结果行");
    // 无旧结果可删 → 不传递变化（全局任务不入队）。
    assert!(
        e.store.job_row("memory_consolidate_global", "global").unwrap().is_none(),
        "无删除不得入队全局合并"
    );
}

#[test]
fn m3_a12_empty_output_with_old_result_deletes_and_propagates() {
    let e = env("a12d");
    let path = write_session(&e.sessions, "t", &[user_msg("v1")]);
    let _ = path;
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);
    let (outcome, _) = run_job(&e, &claim, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput);
    // 先清掉首轮入队的全局行，隔离观察第二轮。
    {
        let conn = rusqlite::Connection::open(
            terminal_chat::memory::memories_db_path(&e.tmp.path().join("memories")),
        )
        .unwrap();
        conn.execute("DELETE FROM jobs WHERE kind = 'memory_consolidate_global'", [])
            .unwrap();
    }

    e.store.register_thread("t", "enabled", (T0 - 50) * 1000).unwrap();
    let claim2 = claim_direct(&e.store, "t", T0 - 50, &path);
    let (outcome2, _) = run_job(&e, &claim2, &model_with_window(None), EMPTY_OUTPUT);
    assert_eq!(outcome2, Stage1JobOutcome::SucceededNoOutput);
    assert!(e.store.stage1_output("t").unwrap().is_none(), "空结果删除旧记录");
    let row = job_row(&e, "t");
    assert_eq!(row.status, "done");
    assert_eq!(row.last_success_watermark, Some(T0 - 50));
    assert!(
        e.store.job_row("memory_consolidate_global", "global").unwrap().is_some(),
        "删除旧结果必须传递变化（入队全局合并）"
    );
}

// —— A09/A10：所有权 ——

#[test]
fn m3_a09_stale_owner_cannot_commit_after_takeover() {
    let e = env("a09");
    let path = write_session(&e.sessions, "t", &[user_msg("x")]);
    let _ = path;
    let stale = claim_direct(&e.store, "t", T0 - 100, &path);

    // 租期到期被新所有者接管（M1 契约）。
    e.clock.advance(3_601);
    let fresh = claim_direct(&e.store, "t", T0 - 100, &path);

    let (outcome, _) = run_job(&e, &stale, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::Failed, "失权所有者提交必须失败");
    assert!(
        e.store.stage1_output("t").unwrap().is_none(),
        "失权所有者不得写入结果"
    );
    let row = job_row(&e, "t");
    assert_eq!(row.status, "running", "接管者任务行不被旧所有者改动");
    assert_ne!(
        row.ownership_token.as_deref(),
        Some(stale.ownership_token.as_str())
    );

    // A10：新所有者完成后，旧所有者的迟到成功也不得覆盖。
    let (outcome2, _) = run_job(&e, &fresh, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome2, Stage1JobOutcome::SucceededWithOutput);
    let (outcome3, _) = run_job(&e, &stale, &model_with_window(None), r#"{"raw_memory":"篡改","rollout_summary":"s","rollout_slug":null}"#);
    assert_eq!(outcome3, Stage1JobOutcome::Failed);
    let out = e.store.stage1_output("t").unwrap().unwrap();
    assert_eq!(out.raw_memory, "经验正文", "迟到提交不得覆盖新所有者结果");

    // 失权所有者的失败记账同样被拒：任务行保持新所有者的 done，
    // 不被改写成 error（Failed 结果≠污染新行）。
    let (outcome4, _) = run_job(&e, &stale, &model_with_window(None), "坏输出");
    assert_eq!(outcome4, Stage1JobOutcome::Failed);
    let row_after = job_row(&e, "t");
    assert_eq!(row_after.status, "done", "失权失败记账不得改写新所有者状态");
    assert_eq!(row_after.last_success_watermark, Some(T0 - 100));
}

// —— A13：三个脱敏位置 ——

#[test]
fn m3_a13_secret_redaction_input_storage_and_parse() {
    let e = env("a13");
    let secret = "sk-ABCDEFGHIJKLMNOPQRSTUVwx123";
    let path = write_session(
        &e.sessions,
        "t",
        &[user_msg(&format!("我的密钥 {secret} 请保管")), assistant_msg("好的")],
    );
    let _ = path;
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);

    // 位置 1（输入）：发出的请求体不得含原始密钥。
    let reply = format!(r#"{{"raw_memory":"密钥 {secret} 泄漏","rollout_summary":"摘要 {secret}","rollout_slug":"s"}}"#);
    let (outcome, seen) = run_job(&e, &claim, &model_with_window(None), &reply);
    assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput);
    let body = seen.lock().unwrap().clone().unwrap();
    let body_str = serde_json::to_string(&body).unwrap();
    assert!(!body_str.contains(secret), "请求体不得包含原始密钥");
    assert!(body_str.contains("[REDACTED_SECRET]"), "请求体应含脱敏标记");

    // 位置 2（存储）：入库结果不得含原始密钥。
    let out = e.store.stage1_output("t").unwrap().unwrap();
    assert!(!out.raw_memory.contains(secret));
    assert!(out.raw_memory.contains("[REDACTED_SECRET]"));
    assert!(out.rollout_summary.contains("[REDACTED_SECRET]"));

    // 位置 3（解析面）：parse 直接对生成结果脱敏（独立于存储）。
    let parsed = terminal_chat::memory::StageOneOutput::parse(&reply).unwrap();
    assert!(parsed.raw_memory.unwrap().contains("[REDACTED_SECRET]"));
}

// —— 输入边界：单记录超限、总量、预算裁剪方向 ——

#[test]
fn m3_oversize_record_skipped_and_total_budget_keeps_newest() {
    let e = env("budget");
    let mut items = vec![
        user_msg(&"x".repeat(MAX_RECORD_BYTES + 100)), // 超过 1 MiB 的单条 → 跳过
        user_msg("keep-new-1"),
        assistant_msg("keep-new-2"),
        user_msg("keep-old-marker"),
    ];
    // 总量超过 8 MiB：追加约 9 MiB 的普通条目，最旧的 keep-old-marker 应被挤掉。
    let big = "y".repeat(600 * 1024);
    for i in 0..16 {
        items.push(user_msg(&format!("fill-{i}-{big}")));
    }
    let path = write_session(&e.sessions, "t", &items);
    let _ = path;

    let read = read_recent_rollout_items(&path).unwrap();
    let total: usize = read
        .iter()
        .map(|i| serde_json::to_vec(i).unwrap().len())
        .sum();
    assert!(total <= MAX_ROLLOUT_BYTES, "保留总量不得超 8 MiB（实际 {total}）");
    assert!(!read.iter().any(|i| matches!(i, HistoryItem::Text(HistoryMessage { content, ..}) if content.starts_with('x'))), "超限单条被跳过");
    assert!(!read.iter().any(|i| matches!(i, HistoryItem::Text(HistoryMessage { content, ..}) if content.contains("keep-old-marker"))), "总量预算从最旧挤起");
    assert!(
        read.iter().any(|i| matches!(i, HistoryItem::Text(HistoryMessage { content, ..}) if content.starts_with("fill-15"))),
        "最新条目保留"
    );
    // 来源顺序恢复（fill-15 在 fill-14 之后）。
    let idx = |needle: &str| {
        read.iter()
            .position(|i| matches!(i, HistoryItem::Text(HistoryMessage { content, ..}) if content.contains(needle)))
            .unwrap()
    };
    assert!(idx("fill-14") < idx("fill-15"), "恢复来源顺序");

    let items_ref: Vec<HistoryItem> = read;
    let serialized = serialize_filtered_history_items(&items_ref).unwrap();
    assert!(!serialized.contains("keep-old-marker"));
}

#[test]
fn m3_model_budget_truncates_middle_keeps_head_and_tail() {
    let e = env("trunc");
    let head = "HEAD-MARKER-开头";
    let tail = "TAIL-MARKER-结尾";
    let middle = "m".repeat(20_000);
    let path = write_session(
        &e.sessions,
        "t",
        &[user_msg(&format!("{head}{middle}{tail}"))],
    );
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);
    let _ = path;

    // 窗口 2000 → 70% ＝ 1400 tokens ＝ 5600 字节预算：中段被裁，首尾保留。
    let (outcome, seen) = run_job(&e, &claim, &model_with_window(Some(2_000)), VALID_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput);
    let body = serde_json::to_string(&seen.lock().unwrap().clone().unwrap()).unwrap();
    assert!(body.contains(head), "头部保留");
    assert!(body.contains(tail), "尾部保留");
    assert!(body.contains("tokens truncated"), "中段裁剪带标记");
}

#[test]
fn m3_token_limit_percent_and_fallback() {
    assert_eq!(rollout_token_limit(&model_with_window(None)), DEFAULT_ROLLOUT_TOKEN_LIMIT);
    assert_eq!(DEFAULT_ROLLOUT_TOKEN_LIMIT, 150_000);
    assert_eq!(rollout_token_limit(&model_with_window(Some(10_000))), 7_000, "有效窗口 70%");
    assert_eq!(rollout_token_limit(&model_with_window(Some(0))), DEFAULT_ROLLOUT_TOKEN_LIMIT, "零窗口回退");
}

// —— 请求组装：selected_model、系统模板、注入剥离 ——

#[test]
fn m3_selected_model_and_system_prompt_enter_request_body() {
    let e = env("reqbody");
    let path = write_session(
        &e.sessions,
        "t",
        &[user_msg("你好"), assistant_msg("你好，有什么可以帮你")],
    );
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);
    let _ = path;

    let resp = ok_response(VALID_OUTPUT).to_string();
    let mock = common::spawn_mock(vec![common::MockResp::json(200, resp)]);
    let conn = EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: terminal_chat::catalog::SUPPORTED_PROTOCOL.to_string(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = ModelInfo {
        id: "m3-selected-model".to_string(),
        context_length: Some(8_000),
        ..ModelInfo::default()
    };
    let outcome = run_stage1_job(&e.store, &claim, &model, "/tmp/proj", &|req| {
        terminal_chat::api::chat_with_body(&conn, "k", &model, req, &api::agent())
    });
    assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput);

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1);
    let body: Value = serde_json::from_str(&reqs[0].body).unwrap();
    assert_eq!(body["model"], "m3-selected-model", "D6：selected_model 实际进入请求体");
    // 格式约束字段：M3 不发送任何结构化输出端点参数——机器级保证在响应侧
    // （严格解析拒绝违规输出→失败＋重试），请求侧由上游模板原文承载字段契约。
    assert!(body.get("response_format").is_none(), "不发送 response_format");
    assert!(body.get("output_schema").is_none(), "不发送 output_schema");
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    let system_text = messages[0]["content"].as_str().unwrap();
    assert_eq!(
        system_text,
        terminal_chat::memory::STAGE_ONE_SYSTEM,
        "系统消息＝上游 stage_one_system 模板原文"
    );
    assert!(
        system_text.contains("Return exactly one JSON object with required keys"),
        "模板承载字段契约"
    );
    assert!(system_text.contains("No additional keys"), "模板承载拒绝额外键");
    assert_eq!(messages[1]["role"], "user");
    let user_text = messages[1]["content"].as_str().unwrap();
    assert!(user_text.contains("rollout_path"), "输入模板含来源路径占位渲染");
    assert!(user_text.contains("你好"), "会话内容进入输入模板");
    assert!(user_text.contains("/tmp/proj"), "cwd 渲染");
    assert!(
        user_text.contains("produce JSON with `raw_memory`, `rollout_summary`, and `rollout_slug`"),
        "输入模板点名三契约键"
    );
    assert!(body.get("tools").is_none(), "提炼请求不携带工具");
}

#[test]
fn m3_context_injections_stripped_from_input() {
    let e = env("strip");
    let agents_md = "# AGENTS.md instructions for /tmp\n\n<INSTRUCTIONS>\n规则\n</INSTRUCTIONS>";
    // 015（c9）：真实用户轮上下文消息（AGENTS＋环境合并块，带替换声明）同被剔除
    let declared_ctx = "The following context replaces the previously provided AGENTS.md instructions and environment context.\n\n# AGENTS.md instructions for /tmp\n\n<INSTRUCTIONS>\n规则\n</INSTRUCTIONS>\n\n<environment_context>\n  <cwd>/tmp</cwd>\n  <shell>/bin/sh</shell>\n  <current_date>2025-10-09</current_date>\n</environment_context>";
    let skill = "<skill>\n<name>demo</name>\n</skill>";
    let path = write_session(
        &e.sessions,
        "t",
        &[
            user_msg(agents_md),
            user_msg(declared_ctx),
            user_msg(skill),
            user_msg("真实用户输入"),
            HistoryItem::ToolResult {
                image: None,
                tool_call_id: "c1".into(),
                content: "tool 输出".into(),
                display: Some(ToolDisplay::Exec { duration_ms: 42 }),
            },
        ],
    );
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);
    let _ = path;

    let (outcome, seen) = run_job(&e, &claim, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput);
    let body = serde_json::to_string(&seen.lock().unwrap().clone().unwrap()).unwrap();
    assert!(!body.contains("AGENTS.md instructions"), "注入说明不作为用户事实");
    assert!(
        !body.contains("replaces the previously provided"),
        "015 带声明的上下文消息整条剔除"
    );
    assert!(!body.contains("<environment_context>"), "环境块不作为用户事实");
    assert!(!body.contains("<skill>"), "技能注入剥离");
    assert!(body.contains("真实用户输入"), "真实用户输入保留");
    assert!(!body.contains("duration_ms"), "展示数据不进模型请求");
    assert!(body.contains("tool 输出"), "工具结果内容保留");
}

// —— 失败面：读取失败、头部无效、模型失败、空回复 ——

#[test]
fn m3_read_and_header_failures_mark_failed() {
    let e = env("readfail");
    // 文件不存在。
    let claim = claim_direct(&e.store, "missing", T0 - 100, &e.sessions.join("missing.jsonl"));
    let (outcome, _) = run_job(&e, &claim, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::Failed);
    assert_eq!(job_row(&e, "missing").status, "error");

    // 头部无效（首行不是 meta）。
    let bad = e.sessions.join("badmeta.jsonl");
    std::fs::write(
        &bad,
        serde_json::to_string(&user_msg("直接正文")).unwrap(),
    )
    .unwrap();
    let claim2 = claim_direct(&e.store, "badmeta", T0 - 100, &bad);
    let read = read_recent_rollout_items(&bad);
    assert!(read.is_err(), "无效头部必须整拒");
    let (outcome2, _) = run_job(&e, &claim2, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome2, Stage1JobOutcome::Failed);
    assert_eq!(job_row(&e, "badmeta").status, "error");
}

#[test]
fn m3_model_failure_and_empty_reply_mark_failed() {
    let e = env("modelfail");
    let path = write_session(&e.sessions, "a", &[user_msg("x")]);
    let claim_a = claim_direct(&e.store, "a", T0 - 100, &path);
    let _ = path;

    // 模型网络/HTTP 失败（经真实 chat_with_body＋mock 500）。
    let mock = common::spawn_mock(vec![common::MockResp::json(500, "{\"error\":\"boom\"}")]);
    let conn = EffectiveConnection {
        base_url: mock.url.clone(),
        protocol: terminal_chat::catalog::SUPPORTED_PROTOCOL.to_string(),
        key_ref: None,
        request_params: Default::default(),
    };
    let model = model_with_window(None);
    let outcome = run_stage1_job(&e.store, &claim_a, &model, "/tmp", &|req| {
        terminal_chat::api::chat_with_body(&conn, "k", &model, req, &api::agent())
    });
    assert_eq!(outcome, Stage1JobOutcome::Failed);
    let row = job_row(&e, "a");
    assert_eq!(row.status, "error");
    assert!(row.last_error.is_some());

    // 空回复（content 空）。
    let path_b = write_session(&e.sessions, "b", &[user_msg("y")]);
    let claim_b = claim_direct(&e.store, "b", T0 - 100, &path_b);
    let _ = path_b;
    let (outcome_b, _) = run_job(&e, &claim_b, &model_with_window(None), "");
    assert_eq!(outcome_b, Stage1JobOutcome::Failed, "空回复按失败处理");
    assert_eq!(job_row(&e, "b").status, "error");
    assert!(e.store.stage1_output("b").unwrap().is_none());
}

// —— 全链：M2 候选 → M3 提炼 ——

#[test]
fn m3_full_chain_scan_claim_distill() {
    let e = env("chain");
    for (name, t) in [("s1", 12i64 * 3600), ("s2", 13 * 3600)] {
        let path = write_session(
            &e.sessions,
            name,
            &[user_msg(&format!("{name} 的问题")), assistant_msg("回答")],
        );
        let _ = path;
        e.store.register_thread(name, "enabled", (T0 - t) * 1000).unwrap();
    }

    let claims = claim_stage1_jobs_for_startup(
        &e.store,
        &e.sessions,
        "cur",
        terminal_chat::memory::Stage1StartupClaimParams {
            scan_limit: 5_000,
            max_claimed: 2,
            max_age_days: 10,
            min_rollout_idle_hours: 6,
            lease_seconds: 3_600,
        },
    )
    .unwrap();
    assert_eq!(claims.len(), 2, "M2 领取两个候选");

    // 顺序处理（D10），各自成功入库。
    for claim in &claims {
        let (outcome, _) = run_job(&e, claim, &model_with_window(None), VALID_OUTPUT);
        assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput, "{}", claim.thread_id);
    }
    for name in ["s1", "s2"] {
        let out = e.store.stage1_output(name).unwrap().unwrap();
        assert_eq!(out.raw_memory, "经验正文");
    }
}

// —— 独立纯函数面：脱敏直连 ——

#[test]
fn m3_redact_secrets_direct() {
    assert_eq!(
        redact_secrets("Bearer abcdefghijklmnopqrst".into()),
        "Bearer [REDACTED_SECRET]"
    );
    assert_eq!(
        redact_secrets("api_key = supersecretvalue123".into()),
        "api_key = [REDACTED_SECRET]"
    );
}

#[test]
fn m3_failure_reason_redacted_before_storing() {
    let e = env("failredact");
    let path = write_session(&e.sessions, "t", &[user_msg("x")]);
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);

    let secret = "sk-ABCDEFGHIJKLMNOPQRSTUVwx123";
    let seen: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let seen2 = Arc::clone(&seen);
    let outcome = run_stage1_job(&e.store, &claim, &model_with_window(None), "/tmp/p", &move |req| {
        *seen2.lock().unwrap() = Some(req.clone());
        Err(format!("上游错误，回显 {secret}"))
    });
    assert_eq!(outcome, Stage1JobOutcome::Failed);
    let row = job_row(&e, "t");
    let err = row.last_error.unwrap();
    assert!(!err.contains(secret), "失败信息不得回显密钥");
    assert!(err.contains("[REDACTED_SECRET]"), "失败信息须脱敏：{err}");
}

#[test]
fn m3_directory_named_like_session_fails_gracefully() {
    let e = env("dirsess");
    std::fs::create_dir_all(e.sessions.join("dir.jsonl")).unwrap();
    let claim = claim_direct(&e.store, "dir", T0 - 100, &e.sessions.join("dir.jsonl"));
    let (outcome, _) = run_job(&e, &claim, &model_with_window(None), VALID_OUTPUT);
    assert_eq!(outcome, Stage1JobOutcome::Failed, "目录项存在性不能替代实际装载");
    assert_eq!(job_row(&e, "dir").status, "error");
}

// —— 定点补验：真实存储 I/O 故障（区别于失权拒提交） ——

/// 确定性写入故障：把记忆库目录改名——SQLite journal 以打开时的主库路径
/// 派生（`…/memories.sqlite-journal`），父目录失联后任何写事务的 journal
/// 创建即 ENOENT（对 root 与普通用户同样生效；读取经既有 fd 不受影响）。
struct StorageBroken {
    root: PathBuf,
    parked: PathBuf,
}

impl StorageBroken {
    fn break_it(e: &Env) -> Self {
        let root = e.tmp.path().join("memories");
        let parked = e.tmp.path().join("memories-parked");
        std::fs::rename(&root, &parked).expect("改名记忆库目录");
        Self { root, parked }
    }
    fn restore(&self) {
        std::fs::rename(&self.parked, &self.root).expect("恢复记忆库目录");
    }
}

impl Drop for StorageBroken {
    fn drop(&mut self) {
        // 断言中途 panic 也保证恢复，让 TempDir 清理可用。
        if self.parked.exists() {
            let _ = std::fs::rename(&self.parked, &self.root);
        }
    }
}

#[test]
fn m3_storage_io_error_on_success_commit_preserves_transaction_contract() {
    let e = env("ioerr-succ");
    let path = write_session(&e.sessions, "t", &[user_msg("问题"), assistant_msg("回答")]);
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);

    let broken = StorageBroken::break_it(&e);
    let (outcome, seen) = run_job(&e, &claim, &model_with_window(None), VALID_OUTPUT);
    broken.restore();

    assert_eq!(
        outcome,
        Stage1JobOutcome::Failed,
        "存储错误必须传播为失败，不得报成功"
    );
    assert!(
        seen.lock().unwrap().is_some(),
        "请求确实发出且解析成功——失败发生在存储层"
    );
    // 无部分提交：任务行/结果行/水位/全局变化信号全部未动。
    let row = job_row(&e, "t");
    assert_eq!(row.status, "running", "任务行不被存储错误部分改写");
    assert_eq!(row.last_success_watermark, None, "成功水位未推进");
    assert_eq!(row.retry_remaining, 3, "存储错误≠业务失败，不消耗重试计数");
    assert_eq!(row.lease_until, Some(T0 + 3_600), "租期保持");
    assert!(row.last_error.is_none(), "未走业务失败记账");
    assert!(
        e.store.stage1_output("t").unwrap().is_none(),
        "结果行未写入（事务回滚）"
    );
    assert!(
        e.store.job_row("memory_consolidate_global", "global").unwrap().is_none(),
        "全局变化信号未发出"
    );
}

#[test]
fn m3_storage_io_error_on_failure_accounting_swallowed_gracefully() {
    let e = env("ioerr-acct");
    let path = write_session(&e.sessions, "t", &[user_msg("x")]);
    let claim = claim_direct(&e.store, "t", T0 - 100, &path);

    // 采样失败（坏 JSON）且记账写入同时故障：记账错误被吞没（上游
    // `let _ = mark_stage1_job_failed` 语义），结果仍如实 Failed，行不损坏。
    let broken = StorageBroken::break_it(&e);
    let (outcome, _) = run_job(&e, &claim, &model_with_window(None), "坏 JSON");
    broken.restore();

    assert_eq!(outcome, Stage1JobOutcome::Failed);
    let row = job_row(&e, "t");
    assert_eq!(row.status, "running", "记账 I/O 失败不得改写任务行");
    assert_eq!(row.retry_at, None, "重试时间未写入");
    assert_eq!(row.retry_remaining, 3, "重试计数未消耗");
    assert!(row.last_error.is_none(), "错误原因未写入（记账本身失败）");
    assert!(e.store.stage1_output("t").unwrap().is_none());
}
