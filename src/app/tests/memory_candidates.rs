//! 013-M2 集成测试：候选选择（上游 openai/codex@ce0323f
//! `claim_stage1_jobs_for_startup` 语义移植）。
//!
//! 时间全部经注入时钟固定（AGENTS.md §3：不依赖真实当前时间、不 sleep）；
//! 目录用独立临时目录，结束清理，不触碰真实用户环境与真实会话数据；
//! 断言同时覆盖返回值（候选内容/顺序/源版本）与实际状态变化（jobs/stage1_outputs）。
//! 窗口语义（上游 SQL 逐字）：`updated_at_ms >= now-年龄窗口`（含）且
//! `updated_at_ms <= now-闲置窗口`（含），`ORDER BY updated_at_ms DESC LIMIT 扫描上限`。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use terminal_chat::catalog;
use terminal_chat::memory::{
    memories_db_path, claim_stage1_jobs_for_startup, params_from_config, MemoriesConfig,
    MemoryStore, Stage1StartupClaimParams, JOB_LEASE_SECONDS, THREAD_SCAN_LIMIT,
};

const T0: i64 = 1_700_000_000;
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

fn ms(secs: i64) -> i64 {
    secs.saturating_mul(1000)
}

/// 唯一临时目录＋离开时清理。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tchat-m2-{tag}-{}-{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 可步进时钟：固定初值，测试显式推进（秒）——候选窗口、退避与租期共用。
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
    fn now(&self) -> i64 {
        *self.0.lock().unwrap()
    }
}

/// 测试环境：临时根（记忆库）＋sessions 目录＋可步进时钟的 store。
struct Env {
    tmp: TempDir,
    sessions: PathBuf,
    clock: SteppedClock,
    store: MemoryStore,
}

fn env(tag: &str) -> Env {
    let tmp = TempDir::new(tag);
    let sessions = tmp.0.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let clock = SteppedClock::at(T0);
    let store =
        MemoryStore::open(&tmp.0.join("memories"), clock.clock()).expect("打开记忆库");
    Env { tmp, sessions, clock, store }
}

/// 在 sessions 目录落一个合成会话文件（M2 只需存在性，不装载正文）。
fn put_session(dir: &Path, name: &str) {
    std::fs::write(dir.join(format!("{name}.jsonl")), b"{\"v\":1}\n").unwrap();
}

/// 登记会话并给定更新时间（秒）。
fn register(env: &Env, name: &str, updated_at_secs: i64) {
    env.store.register_thread(name, "enabled", ms(updated_at_secs)).unwrap();
}

/// 直接改 threads.memory_mode（M2 尚无生产写入面；污染/禁用标记属后续阶段，
/// 本测试仅布置数据库状态以验证候选过滤行为）。
fn set_memory_mode(env: &Env, name: &str, mode: &str) {
    let conn = rusqlite::Connection::open(memories_db_path(&env.tmp.0.join("memories")))
        .expect("直连测试库");
    conn.execute(
        "UPDATE threads SET memory_mode = ?1 WHERE thread_id = ?2",
        rusqlite::params![mode, name],
    )
    .unwrap();
}

/// 上游默认参数（独立字面量；常量正确性另有专门断言钉住）。
fn default_params() -> Stage1StartupClaimParams {
    Stage1StartupClaimParams {
        scan_limit: 5_000,
        max_claimed: 2,
        max_age_days: 10,
        min_rollout_idle_hours: 6,
        lease_seconds: 3_600,
    }
}

fn claim_all(
    env: &Env,
    current: &str,
    params: Stage1StartupClaimParams,
) -> Vec<(String, i64, i64)> {
    claim_stage1_jobs_for_startup(&env.store, &env.sessions, current, params)
        .expect("候选选择应成功")
        .into_iter()
        .map(|c| (c.thread_id, c.updated_at_ms, c.source_updated_at))
        .collect()
}

fn no_job(env: &Env, name: &str) -> bool {
    env.store
        .job_row("memory_stage1", name)
        .unwrap()
        .is_none()
}

// —— A15：首次使用 ——

#[test]
fn m2_a15_first_use_yields_no_candidates() {
    let e = env("a15");
    put_session(&e.sessions, "a");
    put_session(&e.sessions, "b");

    assert!(claim_all(&e, "cur", default_params()).is_empty());
    assert!(no_job(&e, "a"), "不得创建任何任务行");
    assert!(no_job(&e, "b"));
}

// —— A04：当前会话与非 enabled 模式排除 ——

#[test]
fn m2_a04_excludes_current_session_and_non_enabled_modes() {
    let e = env("a04");
    for name in ["cur", "dis", "pol", "ok"] {
        register(&e, name, T0 - 12 * HOUR);
        put_session(&e.sessions, name);
    }
    set_memory_mode(&e, "dis", "disabled");
    set_memory_mode(&e, "pol", "polluted");

    let claimed = claim_all(&e, "cur", default_params());
    assert_eq!(claimed.len(), 1, "仅普通 enabled 会话可入选：{claimed:?}");
    assert_eq!(claimed[0].0, "ok");
    assert!(e.store.job_row("memory_stage1", "ok").unwrap().is_some());
    assert!(no_job(&e, "cur"), "当前会话不得领取");
    assert!(no_job(&e, "dis"), "disabled 不得领取");
    assert!(no_job(&e, "pol"), "polluted 不得领取");
}

// —— A05：闲置/年龄窗口（含恰等边界，两侧余量） ——

#[test]
fn m2_a05_idle_and_age_boundaries_inclusive() {
    let e = env("a05");
    let cases = [
        ("fresh_edge_plus1", T0 - 6 * HOUR + 1, false), // 闲置不足（+1s）
        ("idle_edge", T0 - 6 * HOUR, true),             // 恰满 6 小时：入选
        ("mid", T0 - 12 * HOUR, true),
        ("age_edge", T0 - 10 * DAY, true), // 恰满 10 天：入选
        ("age_edge_minus1", T0 - 10 * DAY - 1, false), // 过旧（-1s）
    ];
    for (name, updated_at, _) in cases {
        register(&e, name, updated_at);
        put_session(&e.sessions, name);
    }
    // 毫秒级余量：钉住窗口比较是毫秒精度（不因换算丢亚秒）。
    e.store
        .register_thread("idle_edge_plus1ms", "enabled", ms(T0 - 6 * HOUR) + 1)
        .unwrap();
    put_session(&e.sessions, "idle_edge_plus1ms");

    let mut params = default_params();
    params.max_claimed = 8;
    let claimed = claim_all(&e, "cur", params);
    let ids: Vec<&str> = claimed.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["idle_edge", "mid", "age_edge"], "恰等边界入选、两侧排除、按更新时间降序");
    for (name, _, want) in cases {
        assert_eq!(no_job(&e, name), !want, "{name} 任务行与入选判断应一致");
    }
    assert!(no_job(&e, "idle_edge_plus1ms"), "毫秒级更新仍受闲置上界约束");
}

// —— 排序与领取上限 ——

#[test]
fn m2_desc_order_and_claim_limit() {
    let e = env("claimcap");
    for (name, t) in [("t1", T0 - 40 * HOUR), ("t2", T0 - 30 * HOUR), ("t3", T0 - 20 * HOUR)] {
        register(&e, name, t);
        put_session(&e.sessions, name);
    }

    let claimed = claim_all(&e, "cur", default_params());
    let ids: Vec<&str> = claimed.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["t3", "t2"], "降序且只领取 2 个");
    assert!(no_job(&e, "t1"), "领取上限外的会话不得创建任务行");
    let row = e.store.job_row("memory_stage1", "t3").unwrap().unwrap();
    assert_eq!(row.lease_until, Some(T0 + 3_600), "租期＝now+任务有效期");
    assert_eq!(row.status, "running");
    assert_eq!(row.worker_id.as_deref(), Some("cur"), "worker＝触发扫描的当前会话");
}

// —— 扫描上限与领取上限区分 ——

#[test]
fn m2_scan_limit_bounds_pool_before_claiming() {
    // 场景一：扫描上限 2 < 合格会话 3——最旧者不进入扫描池（无任务行）。
    let e = env("scanlimit");
    for (name, t) in [("old", T0 - 90 * HOUR), ("mid", T0 - 50 * HOUR), ("new", T0 - 20 * HOUR)] {
        register(&e, name, t);
        put_session(&e.sessions, name);
    }
    let mut params = default_params();
    params.scan_limit = 2;
    let claimed = claim_all(&e, "cur", params);
    let ids: Vec<&str> = claimed.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["new", "mid"], "扫描池＝最新 scan_limit 个");
    assert!(no_job(&e, "old"), "扫描上限外的会话不被考虑");

    // 场景二：领取上限 1——先到者领取，其余未被尝试（无任务行）。
    let e2 = env("claimone");
    for (name, t) in [("old", T0 - 90 * HOUR), ("mid", T0 - 50 * HOUR), ("new", T0 - 20 * HOUR)] {
        register(&e2, name, t);
        put_session(&e2.sessions, name);
    }
    let mut params2 = default_params();
    params2.max_claimed = 1;
    let claimed2 = claim_all(&e2, "cur", params2);
    let ids2: Vec<&str> = claimed2.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids2, vec!["new"]);
    assert!(no_job(&e2, "mid"), "领取已满后不再尝试后续候选");
    assert!(no_job(&e2, "old"));
}

#[test]
fn m2_scan_budget_consumed_by_rows_failing_later_filters() {
    // 扫描上限约束数据库行（上游计数口径）：最新行随后被目录交集剔除时，
    // 预算已被消耗，不回补给更旧的行。
    let e = env("scanbudget");
    register(&e, "newest_nofile", T0 - 20 * HOUR);
    register(&e, "older_withfile", T0 - 30 * HOUR);
    put_session(&e.sessions, "older_withfile");

    let mut p = default_params();
    p.scan_limit = 1;
    assert!(
        claim_all(&e, "cur", p).is_empty(),
        "预算被无文件行消耗，不回补"
    );
    assert!(no_job(&e, "older_withfile"));

    let mut p2 = default_params();
    p2.scan_limit = 2;
    let claimed = claim_all(&e, "cur", p2);
    let ids: Vec<&str> = claimed.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["older_withfile"]);
}

#[test]
fn m2_backoff_and_exhausted_threads_skipped_but_loop_continues() {
    // §7.3「失败状态」行：退避等待与重试耗尽的候选跳过，但不影响其余候选领取。
    let e = env("skipstates");
    register(&e, "backoff", T0 - 20 * HOUR);
    register(&e, "exhausted", T0 - 30 * HOUR);
    register(&e, "fresh", T0 - 40 * HOUR);
    for name in ["backoff", "exhausted", "fresh"] {
        put_session(&e.sessions, name);
    }

    // 耗尽 exhausted：同版本领取-失败 ×3（每次推进越过退避时刻；M1 语义：
    // 同版本重试不恢复计数）。推进后最终时刻 T2，全部会话仍在窗口内。
    for _ in 0..3 {
        let token = match e
            .store
            .try_claim_stage1_job("exhausted", "w", T0 - 30 * HOUR, 3600, 8)
            .unwrap()
        {
            terminal_chat::memory::Stage1JobClaimOutcome::Claimed { ownership_token } => {
                ownership_token
            }
            other => panic!("布置失败：应可领取，实际 {other:?}"),
        };
        assert!(e
            .store
            .mark_stage1_job_failed("exhausted", &token, "err", 3600)
            .unwrap());
        e.clock.advance(3_601);
    }
    let t2 = e.clock.now();
    let exhausted = e.store.job_row("memory_stage1", "exhausted").unwrap().unwrap();
    assert_eq!(exhausted.retry_remaining, 0, "三次失败后重试耗尽");

    // 布置 backoff：在 T2 领取后立即失败（retry_at＝T2+3600，未来）。
    let token = match e
        .store
        .try_claim_stage1_job("backoff", "w", T0 - 20 * HOUR, 3600, 8)
        .unwrap()
    {
        terminal_chat::memory::Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
        other => panic!("布置失败：应可领取，实际 {other:?}"),
    };
    assert!(e
        .store
        .mark_stage1_job_failed("backoff", &token, "err", 3600)
        .unwrap());

    let claimed = claim_all(&e, "cur", default_params());
    let ids: Vec<&str> = claimed.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["fresh"], "退避/耗尽跳过，其余候选继续领取");
    let backoff = e.store.job_row("memory_stage1", "backoff").unwrap().unwrap();
    assert_eq!(backoff.status, "error");
    assert_eq!(backoff.retry_at, Some(t2 + 3_600), "退避中的行不被启动扫描重置");
}

#[test]
fn m2_immediate_rerun_blocked_by_active_leases() {
    // 同参数立即重跑（对应 D2 每轮触发的重复检查）：活动租期挡住重复领取。
    let e = env("rerun");
    register(&e, "a", T0 - 12 * HOUR);
    register(&e, "b", T0 - 13 * HOUR);
    put_session(&e.sessions, "a");
    put_session(&e.sessions, "b");

    let first = claim_all(&e, "cur", default_params());
    assert_eq!(first.len(), 2);
    let second = claim_all(&e, "cur", default_params());
    assert!(second.is_empty(), "租期内的运行任务不得重复领取：{second:?}");
    for id in ["a", "b"] {
        let row = e.store.job_row("memory_stage1", id).unwrap().unwrap();
        assert_eq!(row.status, "running", "首跑的任务行保持运行");
    }
}

#[test]
fn m2_zero_scan_or_claim_limit_short_circuits() {
    let e = env("zerolimit");
    register(&e, "a", T0 - 12 * HOUR);
    put_session(&e.sessions, "a");

    let mut p = default_params();
    p.scan_limit = 0;
    assert!(claim_all(&e, "cur", p).is_empty());
    let mut p2 = default_params();
    p2.max_claimed = 0;
    assert!(claim_all(&e, "cur", p2).is_empty());
    assert!(no_job(&e, "a"));
}

// —— A06/A07/A08：已处理版本与重新可提炼 ——

#[test]
fn m2_a06_already_succeeded_version_not_reclaimed() {
    let e = env("a06");
    register(&e, "a", T0 - 12 * HOUR);
    register(&e, "b", T0 - 13 * HOUR);
    put_session(&e.sessions, "a");
    put_session(&e.sessions, "b");

    let first = claim_all(&e, "cur", default_params());
    assert_eq!(first.len(), 2);
    for (id, _, source) in &first {
        let token = e
            .store
            .job_row("memory_stage1", id)
            .unwrap()
            .unwrap()
            .ownership_token
            .clone();
        assert!(e
            .store
            .mark_stage1_job_succeeded(id, &token.expect("运行行必有令牌"), *source, "r", "s", None)
            .unwrap());
    }

    let second = claim_all(&e, "cur", default_params());
    assert!(second.is_empty(), "同版本已成功，不再领取：{second:?}");
    for id in ["a", "b"] {
        let row = e.store.job_row("memory_stage1", id).unwrap().unwrap();
        assert_eq!(row.status, "done", "状态保持成功，不被重置");
    }
}

#[test]
fn m2_a07_no_output_success_still_blocks_and_marks_watermark() {
    let e = env("a07");
    register(&e, "a", T0 - 12 * HOUR);
    put_session(&e.sessions, "a");

    let first = claim_all(&e, "cur", default_params());
    assert_eq!(first.len(), 1);
    let token = e
        .store
        .job_row("memory_stage1", "a")
        .unwrap()
        .unwrap()
        .ownership_token
        .expect("运行行必有令牌");
    assert!(e
        .store
        .mark_stage1_job_succeeded_no_output("a", &token)
        .unwrap());
    assert!(e.store.stage1_output("a").unwrap().is_none(), "空结果删除结果行");

    let second = claim_all(&e, "cur", default_params());
    assert!(second.is_empty(), "无内容成功仍不重复提炼：{second:?}");
    let row = e.store.job_row("memory_stage1", "a").unwrap().unwrap();
    assert_eq!(
        row.last_success_watermark,
        Some(T0 - 12 * HOUR),
        "成功版本标记存在（水位＝源秒级时间戳）"
    );
}

#[test]
fn m2_a08_updated_session_re_eligible_with_new_version() {
    let e = env("a08");
    register(&e, "a", T0 - 12 * HOUR);
    put_session(&e.sessions, "a");

    let first = claim_all(&e, "cur", default_params());
    assert_eq!(first.len(), 1);
    let (token, old_source) = {
        let row = e.store.job_row("memory_stage1", "a").unwrap().unwrap();
        (row.ownership_token.clone().unwrap(), first[0].2)
    };
    assert!(e
        .store
        .mark_stage1_job_succeeded("a", &token, old_source, "r", "s", None)
        .unwrap());
    assert!(claim_all(&e, "cur", default_params()).is_empty(), "版本未变时不重领");

    // 会话再次更新（仍在闲置/年龄窗口内），亚秒余量钉住 ms→s 截断。
    let new_ms = ms(T0 - 7 * HOUR) + 999;
    e.store.register_thread("a", "enabled", new_ms).unwrap();

    let third = claim_all(&e, "cur", default_params());
    assert_eq!(third.len(), 1, "更新后重新可提炼：{third:?}");
    assert_eq!(third[0].0, "a");
    assert_eq!(third[0].1, new_ms, "更新时间取登记的最新值");
    assert_eq!(third[0].2, T0 - 7 * HOUR, "源版本＝ms→s 截断");
}

// —— sessions 目录与 threads 登记的交集 ——

#[test]
fn m2_candidates_need_both_file_and_registration() {
    let e = env("join");
    register(&e, "registered_only", T0 - 12 * HOUR); // 登记但文件已删
    put_session(&e.sessions, "file_only"); // 文件但未登记
    register(&e, "both", T0 - 20 * HOUR);
    put_session(&e.sessions, "both");

    let claimed = claim_all(&e, "cur", default_params());
    let ids: Vec<&str> = claimed.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["both"]);
    assert!(no_job(&e, "registered_only"), "会话文件不存在不得领取");
    assert!(no_job(&e, "file_only"), "未登记会话不得领取");
}

#[test]
fn m2_missing_sessions_dir_yields_no_candidates() {
    let e = env("nodir");
    register(&e, "a", T0 - 12 * HOUR);
    // sessions 目录不存在（不创建）。
    assert!(claim_all(&e, "cur", default_params()).is_empty());
    assert!(no_job(&e, "a"));
}

// —— 配置：默认值、旧配置兼容、越界钳制、常量钉住 ——

fn write_manual(dir: &Path, body: &str) {
    std::fs::write(dir.join("manual.json"), body).unwrap();
}

#[test]
fn m2_config_defaults_old_config_compat_and_params_mapping() {
    let tmp = TempDir::new("cfg-default");
    // M1 时代的旧配置（仅 enabled）零改动兼容，新字段取上游默认。
    write_manual(&tmp.0, r#"{"memories":{"enabled":true}}"#);
    let m = catalog::load_manual(&tmp.0).expect("旧 memories 段必须兼容");
    let memories = m.memories.expect("段应存在");
    assert!(memories.enabled);
    assert_eq!(memories.max_rollouts_per_startup, 2);
    assert_eq!(memories.max_rollout_age_days, 10);
    assert_eq!(memories.min_rollout_idle_hours, 6);

    let params = params_from_config(&memories);
    assert_eq!(params.scan_limit, 5_000);
    assert_eq!(params.max_claimed, 2);
    assert_eq!(params.max_age_days, 10);
    assert_eq!(params.min_rollout_idle_hours, 6);
    assert_eq!(params.lease_seconds, 3_600);

    // 常量钉住上游值（codex-rs/memories/write/src/lib.rs stage_one）。
    assert_eq!(THREAD_SCAN_LIMIT, 5_000);
    assert_eq!(JOB_LEASE_SECONDS, 3_600);

    // 空段＝全默认（沿 M1 语义，新字段同样有默认）。
    let tmp2 = TempDir::new("cfg-empty");
    write_manual(&tmp2.0, r#"{"memories":{}}"#);
    let m2 = catalog::load_manual(&tmp2.0).expect("空段应可解析");
    let mem2 = m2.memories.expect("段应存在");
    assert_eq!(mem2.max_rollouts_per_startup, 2);
    assert_eq!(mem2.max_rollout_age_days, 10);
    assert_eq!(mem2.min_rollout_idle_hours, 6);
}

#[test]
fn m2_config_clamps_out_of_range_values() {
    // 上游 From<MemoriesToml> 钳制：1–128 / 0–90 / 1–48（M5 追加 1–4096/0–365）。
    let cfg = MemoriesConfig {
        enabled: true,
        use_memories: true,
        generate_memories: true,
        max_rollouts_per_startup: 999,
        max_rollout_age_days: 400,
        min_rollout_idle_hours: 0,
        max_raw_memories_for_consolidation: 99_999,
        max_unused_days: 100_000,
    };
    let params = params_from_config(&cfg);
    assert_eq!(params.max_claimed, 128);
    assert_eq!(params.max_age_days, 90);
    assert_eq!(params.min_rollout_idle_hours, 1);
    let clamped = cfg.clamped();
    assert_eq!(clamped.max_raw_memories_for_consolidation, 4096, "M5：1–4096 钳制");
    assert_eq!(clamped.max_unused_days, 365, "M5：0–365 钳制");

    let tmp = TempDir::new("cfg-clamp");
    write_manual(
        &tmp.0,
        r#"{"memories":{"enabled":true,"max_rollouts_per_startup":0,"max_rollout_age_days":-5,"min_rollout_idle_hours":100}}"#,
    );
    let m = catalog::load_manual(&tmp.0).expect("越界值可解析（钳制在参数构造）");
    let params = params_from_config(m.memories.as_ref().expect("段应存在"));
    assert_eq!(params.max_claimed, 1);
    assert_eq!(params.max_age_days, 0);
    assert_eq!(params.min_rollout_idle_hours, 48);
}

/// 配置边界必须约束真实领取：0 不是停用，超大数量不能放大后台提炼额度。
#[test]
fn m2_config_claim_limits_bound_actual_jobs() {
    for (configured, candidates, expected) in [(0, 3, 1), (2, 3, 2), (999, 129, 128)] {
        let e = env(&format!("config-claim-{configured}"));
        write_manual(
            &e.tmp.0,
            &format!(r#"{{"memories":{{"enabled":true,"max_rollouts_per_startup":{configured}}}}}"#),
        );
        for i in 0..candidates {
            let name = format!("candidate-{i:03}");
            register(&e, &name, T0 - 12 * HOUR - i as i64);
            put_session(&e.sessions, &name);
        }

        let manual = catalog::load_manual(&e.tmp.0).expect("可表示的配置值应可装载");
        let params = params_from_config(manual.memories.as_ref().unwrap());
        let claimed = claim_all(&e, "cur", params);

        assert_eq!(claimed.len(), expected, "配置 {configured}：实际领取数量");
        for i in 0..candidates {
            let name = format!("candidate-{i:03}");
            if i < expected {
                assert_eq!(claimed[i].0, name, "优先领取最新候选");
                let row = e.store.job_row("memory_stage1", &name).unwrap().unwrap();
                assert_eq!(row.status, "running");
                assert_eq!(row.input_watermark, Some(T0 - 12 * HOUR - i as i64));
                assert_eq!(row.last_success_watermark, None, "领取不能冒充处理成功");
            } else {
                assert!(no_job(&e, &name), "配置 {configured}：上限外不创建任务行");
            }
        }
    }
}

/// 负数／超出 usize 表示范围属于格式错误，不能当作可钳制的领取上限。
#[test]
fn m2_config_rejects_negative_and_unrepresentable_claim_limits() {
    for invalid in ["-1", "18446744073709551616"] {
        let tmp = TempDir::new("invalid-claim-limit");
        let input = format!(r#"{{"memories":{{"enabled":true,"max_rollouts_per_startup":{invalid}}}}}"#);
        write_manual(&tmp.0, &input);

        let error = catalog::load_manual(&tmp.0).expect_err("无效数量必须拒绝装载");

        assert!(error.contains("格式错误"), "{invalid}：{error}");
        assert_eq!(std::fs::read_to_string(tmp.0.join("manual.json")).unwrap(), input);
        assert!(!tmp.0.join("memories").exists(), "拒绝配置不应建立记忆库");
    }
}

#[test]
fn m2_config_default_matches_serde_defaults() {
    // 审查修复证明：`Default` 手写实现与 `{}` 反序列化必须同值（derive 会把
    // 数量参数置 0，造成两面分裂）。
    let by_default = MemoriesConfig::default();
    let by_serde: MemoriesConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(by_default, by_serde);
    assert_eq!(by_default.max_rollouts_per_startup, 2);
    assert_eq!(by_default.max_rollout_age_days, 10);
    assert_eq!(by_default.min_rollout_idle_hours, 6);
    assert!(!by_default.enabled);
}
