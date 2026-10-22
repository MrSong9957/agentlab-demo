//! 013-M1 集成测试：记忆状态存储与骨架（上游 openai/codex@ce0323f2 语义移植）。
//!
//! 时间全部经注入时钟固定（AGENTS.md §3：不依赖真实当前时间）；目录用独立临时目录，
//! 结束清理，不触碰真实用户环境；所有断言同时覆盖返回值、数据库状态与文件副作用。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use terminal_chat::memory::{
    memories_db_path, new_token, MemoryStore, Phase2JobClaimOutcome, SelectedOutput,
    Stage1JobClaimOutcome,
};

/// 唯一临时目录＋离开时清理。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tchat-m1-{tag}-{}-{}",
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

/// 可步进时钟：固定初值，测试显式推进（秒）。
struct SteppedClock(Arc<Mutex<i64>>);

impl SteppedClock {
    fn fixed(t: i64) -> Self {
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

fn open_store(root: &std::path::Path, clock: &SteppedClock) -> MemoryStore {
    MemoryStore::open(root, clock.clock()).expect("打开记忆库")
}

fn claim(store: &MemoryStore, thread: &str, source_updated_at: i64) -> String {
    match store
        .try_claim_stage1_job(thread, "worker-1", source_updated_at, 3600, 8)
        .unwrap()
    {
        Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
        other => panic!("应领取成功，实际 {other:?}"),
    }
}

const T0: i64 = 1_700_000_000;

// —— 初始化与文件副作用 ——

#[test]
fn m1_open_creates_dir_and_db_with_required_permissions_and_is_idempotent() {
    let tmp = TempDir::new("init");
    let root = tmp.0.join("memories");
    let clock = SteppedClock::fixed(T0);
    let store = MemoryStore::open(&root, clock.clock()).expect("首次打开应成功");

    assert!(root.is_dir(), "记忆根目录应创建");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "记忆根目录应 0700，实际 {dir_mode:o}");
        let db = memories_db_path(&root);
        assert!(db.is_file(), "数据库文件应创建于 {}", db.display());
        let db_mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(db_mode, 0o600, "数据库文件应 0600，实际 {db_mode:o}");
    }

    // 幂等重开：不报错、表可查（job_row 对未建表会返回 SQL 错误，None 即表已存在）。
    let reopened = MemoryStore::open(&root, SteppedClock::fixed(T0).clock());
    assert!(reopened.is_ok(), "重复打开应幂等");
    assert_eq!(
        store
            .thread_memory_mode("never-registered")
            .expect("threads 表应存在"),
        None
    );
    drop(store);
}

#[cfg(unix)]
#[test]
fn m1_open_rejects_symlink_memory_root() {
    let tmp = TempDir::new("symlink");
    let real = tmp.0.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = tmp.0.join("memories");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let err = MemoryStore::open(&link, SteppedClock::fixed(T0).clock());
    assert!(err.is_err(), "符号链接路径必须拒绝");
}

#[test]
fn m1_tokens_are_unique() {
    let a = new_token();
    let b = new_token();
    assert_ne!(a, b, "所有权令牌必须唯一");
    assert!(!a.is_empty());
}

// —— 第一阶段：领取→成功往返 ——

#[test]
fn m1_stage1_claim_succeed_roundtrip_and_global_enqueue() {
    let tmp = TempDir::new("roundtrip");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token = claim(&store, "t1", 100);
    assert_eq!(store.job_row("memory_stage1", "t1").unwrap().unwrap().status, "running");

    let ok = store
        .mark_stage1_job_succeeded("t1", &token, 100, "raw-甲", "summary-乙", Some("slug-x"))
        .unwrap();
    assert!(ok, "所有权有效时应提交成功");

    let out = store.stage1_output("t1").unwrap().expect("结果应已入库");
    assert_eq!(out.thread_id, "t1");
    assert_eq!(out.source_updated_at, 100);
    assert_eq!(out.raw_memory, "raw-甲");
    assert_eq!(out.rollout_summary, "summary-乙");
    assert_eq!(out.rollout_slug.as_deref(), Some("slug-x"));
    assert_eq!(out.generated_at, T0, "generated_at 取注入时钟");
    assert!(!out.selected_for_phase2);

    let job = store.job_row("memory_stage1", "t1").unwrap().unwrap();
    assert_eq!(job.status, "done");
    assert_eq!(job.last_success_watermark, Some(100));
    assert_eq!(job.lease_until, None, "完成后应清租约");

    // mark_succeeded 内部应已推进全局输入水位：领取全局任务时返回该水位。
    match store.try_claim_global_phase2_job("worker-1", 3600).unwrap() {
        Phase2JobClaimOutcome::Claimed { input_watermark, .. } => {
            assert_eq!(input_watermark, 100, "全局输入水位应推进到 source_updated_at");
        }
        other => panic!("全局任务应可领取，实际 {other:?}"),
    }
}

#[test]
fn m1_stage1_uptodate_versions_skip_without_new_claim() {
    let tmp = TempDir::new("uptodate");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token = claim(&store, "t1", 100);
    store
        .mark_stage1_job_succeeded("t1", &token, 100, "r", "s", None)
        .unwrap();

    // 已有结果版本 ≥ 请求版本 → 跳过（A06/A07：空结果版本同样由水位记住）。
    assert_eq!(
        store.try_claim_stage1_job("t1", "w", 100, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::SkippedUpToDate
    );
    assert_eq!(
        store.try_claim_stage1_job("t1", "w", 99, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::SkippedUpToDate
    );
    // 更新版本可重新提炼（A08）。
    assert!(matches!(
        store.try_claim_stage1_job("t1", "w", 101, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::Claimed { .. }
    ));
}

#[test]
fn m1_stage1_race_second_claim_and_running_cap() {
    let tmp = TempDir::new("race");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let _token_a = claim(&store, "t1", 100);
    // 同会话并发领取：有效租约占线。
    assert_eq!(
        store.try_claim_stage1_job("t1", "worker-2", 100, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::SkippedRunning
    );
    // 不同会话、运行数上限 1：第二个也被拒（A09 的单进程面）。
    assert_eq!(
        store.try_claim_stage1_job("t2", "worker-2", 100, 3600, 1).unwrap(),
        Stage1JobClaimOutcome::SkippedRunning
    );
    // 上限 2 时第二个会话可领取。
    assert!(matches!(
        store.try_claim_stage1_job("t2", "worker-2", 100, 3600, 2).unwrap(),
        Stage1JobClaimOutcome::Claimed { .. }
    ));
}

#[test]
fn m1_stage1_lease_expiry_allows_takeover_and_stale_owner_rejected() {
    let tmp = TempDir::new("lease");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token_a = claim(&store, "t1", 100); // 租约 3600s
    clock.advance(3601); // 越过租期边界

    // 新所有者可接管。
    let token_b = match store
        .try_claim_stage1_job("t1", "worker-2", 100, 3600, 8)
        .unwrap()
    {
        Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
        other => panic!("租期到期后应可接管，实际 {other:?}"),
    };
    assert_ne!(token_a, token_b);

    // 旧令牌提交被拒（A10），且不产生任何结果行。
    let stale = store
        .mark_stage1_job_succeeded("t1", &token_a, 100, "旧数据", "旧摘要", None)
        .unwrap();
    assert!(!stale, "失权旧令牌必须拒提交");
    assert!(store.stage1_output("t1").unwrap().is_none(), "旧令牌不得留下结果");

    // 新令牌正常提交。
    assert!(store
        .mark_stage1_job_succeeded("t1", &token_b, 100, "新数据", "新摘要", None)
        .unwrap());
    assert_eq!(store.stage1_output("t1").unwrap().unwrap().raw_memory, "新数据");
}

#[test]
fn m1_stage1_stale_token_cannot_overwrite_newer_owner_result() {
    let tmp = TempDir::new("a10");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token_a = claim(&store, "t1", 100);
    clock.advance(3601);
    // 新所有者以更新版本接管并先完成。
    let token_b = match store
        .try_claim_stage1_job("t1", "worker-2", 200, 3600, 8)
        .unwrap()
    {
        Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
        other => panic!("{other:?}"),
    };
    assert!(store
        .mark_stage1_job_succeeded("t1", &token_b, 200, "新结果", "新摘要", None)
        .unwrap());

    // 迟到的旧任务（token_a，仍是 running 行时代领取）提交：必须被拒且不覆盖新结果。
    let late = store
        .mark_stage1_job_succeeded("t1", &token_a, 100, "迟到旧结果", "旧摘要", None)
        .unwrap();
    assert!(!late, "旧任务不得覆盖新任务结果（A10）");
    let out = store.stage1_output("t1").unwrap().unwrap();
    assert_eq!(out.source_updated_at, 200);
    assert_eq!(out.raw_memory, "新结果");
}

#[test]
fn m1_stage1_failure_retry_accounting_and_backoff() {
    let tmp = TempDir::new("retry");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token = claim(&store, "t1", 100);
    assert!(store
        .mark_stage1_job_failed("t1", &token, "模型超时", 3600)
        .unwrap());

    let job = store.job_row("memory_stage1", "t1").unwrap().unwrap();
    assert_eq!(job.status, "error");
    assert_eq!(job.retry_at, Some(T0 + 3600), "retry_at＝now＋延迟");
    assert_eq!(job.retry_remaining, 2, "重试计数递减");
    assert_eq!(job.last_error.as_deref(), Some("模型超时"));
    assert_eq!(job.lease_until, None, "失败后释放占用");

    // 退避期内不可领取。
    assert_eq!(
        store.try_claim_stage1_job("t1", "w", 100, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::SkippedRetryBackoff
    );
    // 更新的输入版本恢复领取资格（上游 SQL 的 watermark 分支）。
    assert!(matches!(
        store.try_claim_stage1_job("t1", "w", 150, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::Claimed { .. }
    ));
}

#[test]
fn m1_stage1_retry_exhausted_blocks_until_newer_version() {
    let tmp = TempDir::new("exhaust");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    // 同一版本连续失败才会耗尽（上游语义：更新的输入版本会恢复重试计数，
    // 见手册 §8.7——所以这里三轮都用 source_updated_at=100）。
    let mut first = true;
    for expected_remaining in [2, 1, 0] {
        if !first {
            clock.advance(3601); // 越过上一轮 retry_at（delay=0）与租约
        }
        first = false;
        let token = match store
            .try_claim_stage1_job("t1", "w", 100, 3600, 8)
            .unwrap()
        {
            Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
            other => panic!("重试未耗尽前应可领取，实际 {other:?}"),
        };
        assert!(store.mark_stage1_job_failed("t1", &token, "err", 0).unwrap());
        assert_eq!(
            store.job_row("memory_stage1", "t1").unwrap().unwrap().retry_remaining,
            expected_remaining
        );
    }
    assert_eq!(
        store.try_claim_stage1_job("t1", "w", 100, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::SkippedRetryExhausted,
        "重试耗尽后同版本不得再领取"
    );
    assert!(matches!(
        store.try_claim_stage1_job("t1", "w", 999, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::Claimed { .. },
    ), "更新版本恢复处理机会");
}

#[test]
fn m1_stage1_no_output_deletes_old_result_and_records_success_version() {
    let tmp = TempDir::new("noout");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token = claim(&store, "t1", 100);
    assert!(store
        .mark_stage1_job_succeeded("t1", &token, 100, "旧内容", "旧摘要", None)
        .unwrap());

    clock.advance(3601);
    let token2 = match store
        .try_claim_stage1_job("t1", "w", 200, 3600, 8)
        .unwrap()
    {
        Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
        other => panic!("{other:?}"),
    };
    // 空结果：记录成功版本并删除旧结果（A12 存储面）。
    assert!(store.mark_stage1_job_succeeded_no_output("t1", &token2).unwrap());
    assert!(store.stage1_output("t1").unwrap().is_none(), "空结果应删除旧内容");
    assert_eq!(
        store.job_row("memory_stage1", "t1").unwrap().unwrap().last_success_watermark,
        Some(200)
    );
    // 该版本之后不再重复提炼。
    assert_eq!(
        store.try_claim_stage1_job("t1", "w", 200, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::SkippedUpToDate
    );
}

// —— 全局合并任务（第二阶段协调，M1 面：领取/心跳/成功/失败）——

#[test]
fn m1_phase2_claim_heartbeat_cooldown_and_selection_baseline() {
    let tmp = TempDir::new("phase2");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    // 先造一条 stage1 结果供选中基线断言。
    let token1 = claim(&store, "t1", 100);
    store
        .mark_stage1_job_succeeded("t1", &token1, 100, "r", "s", None)
        .unwrap();

    let token = match store.try_claim_global_phase2_job("worker-1", 3600).unwrap() {
        Phase2JobClaimOutcome::Claimed { ownership_token, input_watermark } => {
            assert_eq!(input_watermark, 100);
            ownership_token
        }
        other => panic!("{other:?}"),
    };
    // 运行中＋有效租约：他人不可领取。
    assert_eq!(
        store.try_claim_global_phase2_job("worker-2", 3600).unwrap(),
        Phase2JobClaimOutcome::SkippedRunning
    );
    // 心跳续期；错误令牌心跳无效。
    assert!(store.heartbeat_global_phase2_job(&token, 60).unwrap());
    assert!(!store.heartbeat_global_phase2_job("bogus", 60).unwrap());

    assert!(store
        .mark_global_phase2_job_succeeded(
            &token,
            100,
            &[SelectedOutput { thread_id: "t1".into(), source_updated_at: 100 }],
        )
        .unwrap());

    let out = store.stage1_output("t1").unwrap().unwrap();
    assert!(out.selected_for_phase2, "选中的来源应标记基线");
    assert_eq!(out.selected_for_phase2_source_updated_at, Some(100));
    assert_eq!(
        store.job_row("memory_consolidate_global", "global").unwrap().unwrap().last_success_watermark,
        Some(100)
    );

    // 成功冷却 6 小时（A16）：上游条件＝finished_at > now−6h——恰满 6 小时即可领取，
    // 在边界两侧留确定余量（§19.2 时间用例规则）。
    clock.advance(6 * 3600 - 1);
    assert_eq!(
        store.try_claim_global_phase2_job("worker-2", 3600).unwrap(),
        Phase2JobClaimOutcome::SkippedCooldown
    );
    // 越过冷却边界（再推 2 秒余量）即可领取。
    clock.advance(2);
    assert!(matches!(
        store.try_claim_global_phase2_job("worker-2", 3600).unwrap(),
        Phase2JobClaimOutcome::Claimed { .. }
    ));
}

#[test]
fn m1_phase2_failure_backoff_wrong_token_rejected_and_if_unowned_scope() {
    let tmp = TempDir::new("p2fail");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token = match store.try_claim_global_phase2_job("w1", 3600).unwrap() {
        Phase2JobClaimOutcome::Claimed { ownership_token, .. } => ownership_token,
        other => panic!("{other:?}"),
    };
    assert!(store
        .mark_global_phase2_job_failed(&token, "输出校验失败", 3600)
        .unwrap());
    let job = store.job_row("memory_consolidate_global", "global").unwrap().unwrap();
    assert_eq!(job.status, "error");
    assert_eq!(job.retry_at, Some(T0 + 3600));
    assert_eq!(job.retry_remaining, 2);

    // 退避期内不可领取。
    assert_eq!(
        store.try_claim_global_phase2_job("w2", 3600).unwrap(),
        Phase2JobClaimOutcome::SkippedRetryUnavailable
    );
    // 错误令牌不得提交成功（A22 存储面）。
    assert!(!store
        .mark_global_phase2_job_succeeded("bogus", 1, &[])
        .unwrap());
    // 失权兜底：令牌不匹配且非 NULL → 不生效。
    assert!(!store
        .mark_global_phase2_job_failed_if_unowned("other-token", "兜底", 3600)
        .unwrap());

    // 退避结束后重试计数递减不减破 0：连续失败 3 次→0 后仍可记账（max(...,0)）。
    clock.advance(3601);
    for _ in 0..3 {
        let t = match store.try_claim_global_phase2_job("w2", 3600).unwrap() {
            Phase2JobClaimOutcome::Claimed { ownership_token, .. } => ownership_token,
            other => panic!("{other:?}"),
        };
        assert!(store.mark_global_phase2_job_failed(&t, "e", 0).unwrap());
        clock.advance(1);
    }
    assert_eq!(
        store.job_row("memory_consolidate_global", "global").unwrap().unwrap().retry_remaining,
        0,
        "重试计数递减不减破 0"
    );
}

// —— G6：threads 登记表 ——

#[test]
fn m1_register_thread_mode_from_caller_and_refresh_keeps_mode() {
    let tmp = TempDir::new("threads");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    store.register_thread("s-auto-1", "enabled", 1000).unwrap();
    assert_eq!(
        store.thread_memory_mode("s-auto-1").unwrap().as_deref(),
        Some("enabled")
    );
    // 重复登记只刷新更新时间，不覆盖模式（014 §3：全局配置变更不改旧行）。
    store.register_thread("s-auto-1", "disabled", 2000).unwrap();
    assert_eq!(
        store.thread_memory_mode("s-auto-1").unwrap().as_deref(),
        Some("enabled")
    );
    assert_eq!(
        store.thread_updated_at_ms("s-auto-1").unwrap(),
        Some(2000),
        "冲突路径仍刷新更新时间"
    );
    // 014 §3：generate=false 新登记 disabled；再以 enabled 重登不批量改。
    store.register_thread("s-off-1", "disabled", 3000).unwrap();
    store.register_thread("s-new-1", "enabled", 4000).unwrap();
    assert_eq!(
        store.thread_memory_mode("s-off-1").unwrap().as_deref(),
        Some("disabled")
    );
    assert_eq!(store.thread_memory_mode("unknown").unwrap(), None);
}

#[test]
fn m1_stage1_zero_lease_is_immediately_takeable() {
    // 边界：lease_seconds 钳 0（上游 .max(0)）＝租约即刻到期，他人可立即接管。
    let tmp = TempDir::new("lease0");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);

    let token_a = match store
        .try_claim_stage1_job("t1", "w1", 100, 0, 8)
        .unwrap()
    {
        Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        store.try_claim_stage1_job("t1", "w2", 100, 3600, 8).unwrap(),
        Stage1JobClaimOutcome::Claimed { .. }
    ), "零租约应立即可被接管");
    // 原所有者随之失权。
    assert!(!store
        .mark_stage1_job_succeeded("t1", &token_a, 100, "r", "s", None)
        .unwrap());
}

#[test]
fn m1_reopen_preserves_state() {
    // 持久化往返：重开后任务/结果/登记全部保留（迁移幂等不重置数据）。
    let tmp = TempDir::new("reopen");
    let root = tmp.0.join("memories");
    let clock = SteppedClock::fixed(T0);
    {
        let store = open_store(&root, &clock);
        let token = claim(&store, "t1", 100);
        store
            .mark_stage1_job_succeeded("t1", &token, 100, "r", "s", Some("slug"))
            .unwrap();
        store.register_thread("s1", "enabled", 42).unwrap();
    }
    let reopened = open_store(&root, &clock);
    let out = reopened.stage1_output("t1").unwrap().expect("结果应保留");
    assert_eq!(out.rollout_slug.as_deref(), Some("slug"));
    assert_eq!(
        reopened.job_row("memory_stage1", "t1").unwrap().unwrap().status,
        "done"
    );
    assert_eq!(
        reopened.thread_memory_mode("s1").unwrap().as_deref(),
        Some("enabled")
    );
}

#[test]
fn m1_config_empty_memories_section_defaults_to_disabled() {
    // 等价类：段存在但为空对象＝全部默认（enabled=false）。
    let tmp = TempDir::new("cfg-empty");
    write_manual(&tmp.0, r#"{"memories":{}}"#);
    let m = terminal_chat::catalog::load_manual(&tmp.0).expect("空段应可解析");
    assert!(!m.memories.expect("段应存在").enabled);
}

#[test]
fn m1_two_store_instances_serialize_via_busy_timeout() {
    // 跨连接并发：独立 SQLite 连接持有同库 BEGIN IMMEDIATE 写锁，被测写者
    //（另一真实 MemoryStore，默认连接配置——测试不补设 busy_timeout／忙处理器）
    // 在锁释放前已进入操作且保持未完成（有界探测证实）；默认等待期内释放后
    // 原调用成功，结果完整且跨连接可见（A09 的跨连接面）。
    use std::sync::mpsc;

    let tmp = TempDir::new("concurrent");
    let root = tmp.0.join("memories");
    let clock = SteppedClock::fixed(T0);
    let a = open_store(&root, &clock);
    let b = open_store(&root, &clock);
    let token_a = claim(&a, "t1", 100);
    let token_b = claim(&b, "t2", 100);

    // 独立连接持锁：BEGIN IMMEDIATE 即取写锁；释放信号到达（或发送端被 drop）
    // 后回滚退出——断言失败 panic 时发送端 drop 也会放行，不泄漏持锁线程。
    let db = memories_db_path(&root);
    let (lock_ready_tx, lock_ready_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let blocker = std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        lock_ready_tx.send(()).unwrap();
        let _ = release_rx.recv(); // 持锁直到放行（发送端 drop 亦放行）
        conn.execute_batch("ROLLBACK").unwrap();
    });
    lock_ready_rx.recv().unwrap(); // 写锁已就位

    // 被测写者：store a 在持锁期间发起写（默认 busy_timeout 5s 内等待）
    let (writer_started_tx, writer_started_rx) = mpsc::channel::<()>();
    let writer = {
        let a = a;
        let started = writer_started_tx;
        std::thread::spawn(move || {
            started.send(()).unwrap();
            let r = a.mark_stage1_job_succeeded("t1", &token_a, 100, "raw-1", "sum-1", Some("slug-1"));
            (a, r)
        })
    };
    writer_started_rx.recv().unwrap();
    // 有界探测：锁仍持有时写者必须仍在等待（否则竞争未发生，测试无效）
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!writer.is_finished(), "写锁持有期间写者应仍在等待（未发生竞争？）");
    // 默认等待期内放行 → 原调用成功
    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let (a, marked) = loop {
        if writer.is_finished() {
            let (a, r) = writer.join().unwrap();
            break (a, r.unwrap());
        }
        assert!(
            std::time::Instant::now() < deadline,
            "锁释放后写者未在期限内完成（默认等待不生效？）"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    blocker.join().unwrap();
    assert!(marked, "锁竞争下原调用应成功写入");
    // 第二实例交错写 + 完整结果与跨连接可见性
    assert!(b
        .mark_stage1_job_succeeded("t2", &token_b, 100, "raw-2", "sum-2", None)
        .unwrap());
    let out_a = a.stage1_output("t1").unwrap().expect("t1 结果应保留");
    assert_eq!(out_a.raw_memory, "raw-1");
    assert_eq!(out_a.rollout_summary, "sum-1");
    assert_eq!(out_a.rollout_slug.as_deref(), Some("slug-1"));
    // 两连接交叉读到对方的完整值
    let cross_b = b.stage1_output("t1").unwrap().expect("b 应读到 t1");
    assert_eq!(cross_b.raw_memory, "raw-1");
    assert_eq!(cross_b.rollout_slug.as_deref(), Some("slug-1"));
    let cross_a = a.stage1_output("t2").unwrap().expect("a 应读到 t2");
    assert_eq!(cross_a.raw_memory, "raw-2");
    assert_eq!(cross_a.rollout_slug, None);
}

// —— D5：manual.json memories 段兼容 ——

fn write_manual(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("manual.json"), body).unwrap();
}

#[test]
fn m1_config_without_memories_section_stays_compatible() {
    let tmp = TempDir::new("cfg-old");
    write_manual(
        &tmp.0,
        r#"{"selected_provider":"deepseek","selected_model":"deepseek-flash","providers":{"deepseek":{"key_ref":"env:DEEPSEEK_API_KEY"}}}"#,
    );
    let m = terminal_chat::catalog::load_manual(&tmp.0).expect("旧配置必须零改动兼容");
    assert_eq!(m.selected_provider.as_deref(), Some("deepseek"));
    assert!(m.memories.is_none(), "无段＝未配置（默认关闭）");
}

#[test]
fn m1_config_parses_memories_section() {
    let tmp = TempDir::new("cfg-new");
    write_manual(&tmp.0, r#"{"memories":{"enabled":true}}"#);
    let m = terminal_chat::catalog::load_manual(&tmp.0).expect("新段应可解析");
    let memories = m.memories.expect("段应存在");
    assert!(memories.enabled);
}

#[test]
fn m1_config_rejects_wrong_type_and_unknown_keys() {
    let tmp = TempDir::new("cfg-bad");
    write_manual(&tmp.0, r#"{"memories":{"enabled":"yes"}}"#);
    assert!(terminal_chat::catalog::load_manual(&tmp.0).is_err(), "类型错误必须拒绝");

    // 完整样本（enabled＋max_rollouts_per_startup＋bogus 未知键）必须拒绝
    //（deny_unknown_fields）——覆盖自 m2_config_unknown_key_still_rejected 迁入
    let tmp2 = TempDir::new("cfg-unknown");
    write_manual(
        &tmp2.0,
        r#"{"memories":{"enabled":true,"max_rollouts_per_startup":2,"bogus":1}}"#,
    );
    assert!(
        terminal_chat::catalog::load_manual(&tmp2.0).is_err(),
        "未知键必须拒绝（deny_unknown_fields）"
    );

    // 对照：完整样本去掉 bogus 后可解析且值正确（拒绝确实由未知键引起）
    let tmp3 = TempDir::new("cfg-known");
    write_manual(
        &tmp3.0,
        r#"{"memories":{"enabled":true,"max_rollouts_per_startup":2}}"#,
    );
    let m = terminal_chat::catalog::load_manual(&tmp3.0).expect("去掉未知键后应可解析");
    let memories = m.memories.expect("段应存在");
    assert!(memories.enabled);
    assert_eq!(memories.max_rollouts_per_startup, 2);
}

// —— 014 §3：资格筛选与清理规则（验证既有正确实现，不重写）——

/// stage1 候选扫描与 phase2 来源选择都尊重 memory_mode；保留清理不加
/// memory_mode 过滤（disabled 行的过期产物照删——与上游 SQL 一致）。
#[test]
fn m14_selection_gates_respect_mode_and_prune_ignores_mode() {
    let tmp = TempDir::new("m14-gates");
    let clock = SteppedClock::fixed(T0);
    let store = open_store(&tmp.0.join("memories"), &clock);
    let old = T0 - 7 * 24 * 3600; // 落在 10 天年龄窗内

    // 生产 API 播种（014 后 register_thread 原生支持 disabled 新行）
    store
        .register_thread("gate-on", "enabled", old * 1000)
        .unwrap();
    store
        .register_thread("gate-off", "disabled", old * 1000)
        .unwrap();

    // stage1 候选扫描：enabled 入选、disabled 排除
    let scanned = store
        .stage1_startup_scan("auto-current", 10, 6, 100)
        .unwrap()
        .into_iter()
        .map(|(tid, _)| tid)
        .collect::<Vec<_>>();
    assert!(
        scanned.contains(&"gate-on".to_string()),
        "enabled 应入候选：{scanned:?}"
    );
    assert!(
        !scanned.contains(&"gate-off".to_string()),
        "disabled 不得入候选：{scanned:?}"
    );

    // phase2 来源选择：为两线程各成功一份产物，disabled 的产物不得入选
    for tid in ["gate-on", "gate-off"] {
        let Stage1JobClaimOutcome::Claimed { ownership_token } =
            store.try_claim_stage1_job(tid, "w", old, 3600, 8).unwrap()
        else {
            panic!("领取 {tid}");
        };
        store
            .mark_stage1_job_succeeded(tid, &ownership_token, old, "raw-m14", "summary-m14", None)
            .unwrap();
    }
    let selected = store
        .get_phase2_input_selection(256, 30)
        .unwrap()
        .into_iter()
        .map(|row| row.thread_id)
        .collect::<Vec<_>>();
    assert!(selected.contains(&"gate-on".to_string()), "enabled 产物应入选：{selected:?}");
    assert!(!selected.contains(&"gate-off".to_string()), "disabled 产物不得入选：{selected:?}");

    // 保留清理不加 memory_mode 过滤：两行同 OLD 且未被使用 → 一并删除
    let ancient = T0 - 40 * 24 * 3600; // 超过 30 天未用
    let conn = rusqlite::Connection::open(tmp.0.join("memories/memories.sqlite")).unwrap();
    conn.execute(
        "UPDATE stage1_outputs SET source_updated_at = ?1, last_usage = NULL",
        rusqlite::params![ancient],
    )
    .unwrap();
    drop(conn);
    let deleted = store.prune_stage1_outputs_for_retention(30, 200).unwrap();
    assert_eq!(deleted, 2, "disabled 行的过期产物照删（清理无 mode 过滤）");
}
