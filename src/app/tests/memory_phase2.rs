//! 013-M5 集成测试：第二阶段合并（上游 openai/codex@ce0323f 语义移植）。
//!
//! 时间全部经注入时钟固定；记忆根/仓库用独立临时目录，结束清理；合并代理
//! 全 mock（闭包注入，经 `run_tool_turn_with_mode` 覆盖真实请求组装与工具
//! 分派路径，不调用真实模型）；Git 断言直接驱动临时仓库内的一次性 git
//! 命令（仅测试侧观测）。断言同时覆盖数据库状态、基线（HEAD/树）、
//! 文件副作用与模型调用次数。

mod common;

use std::path::{Path, PathBuf};
use terminal_chat::proc_session::ProcessManager;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use terminal_chat::memory::mem_tools;
use terminal_chat::memory::workspace;
use terminal_chat::memory::{
    build_consolidation_prompt, memories_db_path, run_phase2_consolidation, MemoryStore,
    Phase2Outcome, Phase2Params, ThreadFileMeta, CONSOLIDATION_SYSTEM, JOB_HEARTBEAT_SECONDS,
};
use terminal_chat::turn::{run_tool_turn, run_tool_turn_with_mode, ToolMode};

const T0: i64 = 1_700_000_000;
const HOUR: i64 = 3_600;

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
    root: PathBuf,
    clock: SteppedClock,
    store: MemoryStore,
}

fn env(tag: &str) -> Env {
    let tmp = common::TempDir::new(&format!("m5-{tag}"));
    let root = tmp.path().join("memories");
    let clock = SteppedClock::at(T0);
    let store = MemoryStore::open(&root, clock.clock()).unwrap();
    Env { tmp, root, clock, store }
}

/// 造一行 stage1 结果（登记线程＋领取＋成功提交——真实路径入数）。
fn seed_row(e: &Env, thread: &str, source: i64, raw: &str, summary: &str) {
    e.store.register_thread(thread, "enabled", source * 1000).unwrap();
    let outcome = e
        .store
        .try_claim_stage1_job(thread, "seed", source, 3600, 8)
        .unwrap();
    let terminal_chat::memory::Stage1JobClaimOutcome::Claimed { ownership_token } = outcome
    else {
        panic!("种子行应可领取：{thread}")
    };
    e.store
        .mark_stage1_job_succeeded(thread, &ownership_token, source, raw, summary, None)
        .unwrap();
}

/// 测试侧直写 usage 计数（M8 起生产写侧＝引用反馈 record_stage1_output_usage；
/// 此处仅绕开反馈链路直造排序输入）。
fn set_usage(e: &Env, thread: &str, usage_count: Option<i64>, last_usage: Option<i64>) {
    let conn = rusqlite::Connection::open(memories_db_path(&e.root)).unwrap();
    conn.execute(
        "UPDATE stage1_outputs SET usage_count = ?1, last_usage = ?2 WHERE thread_id = ?3",
        rusqlite::params![usage_count, last_usage, thread],
    )
    .unwrap();
}

/// 合成线程元数据解析器（M7 前无登记写侧；路径仅入文件头）。
fn synthetic_meta(e: &Env) -> impl Fn(&str) -> Option<ThreadFileMeta> + '_ {
    move |t: &str| {
        Some(ThreadFileMeta {
            rollout_path: e.tmp.path().join("sessions").join(format!("{t}.jsonl")),
            cwd: e.tmp.path().join("w"),
            git_branch: None,
        })
    }
}

fn params(n: usize) -> Phase2Params {
    Phase2Params { max_raw_memories: n, max_unused_days: 30 }
}

fn tool_call(id: &str, name: &str, args: Value) -> Value {
    json!({
        "id": id,
        "type": "function",
        "function": { "name": name, "arguments": args.to_string() }
    })
}

fn assistant_with_calls(calls: Vec<Value>) -> Value {
    json!({ "choices": [{ "message": {
        "role": "assistant", "content": null, "tool_calls": calls
    }, "finish_reason": "tool_calls" }] })
}

fn final_reply(text: &str) -> Value {
    json!({ "choices": [{ "message": {
        "role": "assistant", "content": text
    }, "finish_reason": "stop" }] })
}

/// 「合格合并代理」脚本：mem_write 写 MEMORY.md＋memory_summary.md（v1 首行），
/// 然后终文。计数器记录模型调用次数。
fn good_agent(e: &Env) -> (Arc<AtomicUsize>, impl Fn(&Value) -> Result<Value, String> + '_) {
    write_agent(e, 0)
}

/// 可注入 hook 的合并代理脚本：每次模型调用前先跑 `hook`（模拟长时间请求
/// 期间的时钟推进/外部接管/外部破坏）。
fn write_agent(
    e: &Env,
    advance_per_call: i64,
) -> (Arc<AtomicUsize>, impl Fn(&Value) -> Result<Value, String> + '_) {
    let calls = Arc::new(AtomicUsize::new(0));
    let first = assistant_with_calls(vec![
        tool_call(
            "w-mem",
            "mem_write",
            json!({ "path": "MEMORY.md", "content": "# Memory\n\n- merged\n" }),
        ),
        tool_call(
            "w-sum",
            "mem_write",
            json!({ "path": "memory_summary.md", "content": "v1\n- merged summary\n" }),
        ),
    ]);
    let fin = final_reply("consolidated");
    let counter = Arc::clone(&calls);
    let agent = move |_body: &Value| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        if advance_per_call > 0 {
            e.clock.advance(advance_per_call);
        }
        if n == 0 {
            Ok(first.clone())
        } else if n == 1 {
            Ok(fin.clone())
        } else {
            Err("模型不应被第三次调用".into())
        }
    };
    (calls, agent)
}

/// 「写两产物」工具批（合并代理最小合格动作——测试脚本复用形状）。
fn merge_artifact_batch() -> Value {
    assistant_with_calls(vec![
        tool_call("w-mem", "mem_write", json!({ "path": "MEMORY.md", "content": "# M\n" })),
        tool_call("w-sum", "mem_write", json!({ "path": "memory_summary.md", "content": "v1\n" })),
    ])
}

fn run(e: &Env, agent: &dyn Fn(&Value) -> Result<Value, String>) -> Phase2Outcome {
    let resolver = synthetic_meta(e);
    run_phase2_consolidation(&e.store, &e.root, "w-test", params(64), &resolver, agent, None)
}

fn job_row(e: &Env) -> terminal_chat::memory::JobRow {
    e.store
        .job_row("memory_consolidate_global", "global")
        .unwrap()
        .expect("全局任务行存在")
}

/// 测试侧一次性 git 观测（仅临时记忆根内运行；不触工作仓库）。
fn git(root: &Path, args: &[&str]) -> (i32, String) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .output()
        .expect("git 可用");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

fn head(root: &Path) -> String {
    let (code, out) = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(code, 0, "HEAD 应存在：{out}");
    out
}

fn commit_count(root: &Path) -> usize {
    let (_, out) = git(root, &["rev-list", "--count", "HEAD"]);
    out.parse().unwrap()
}

fn tree_has(root: &Path, path: &str) -> bool {
    let (_, out) = git(root, &["ls-tree", "-r", "--name-only", "HEAD", "--", path]);
    !out.is_empty()
}

// —— A16：冷却、租期、心跳与接管 ——

/// 成功后 6 小时冷却；边界精确（6h-1s 拒绝、恰 6h 可再领取）；冷却内不触目录。
#[test]
fn a16_cooldown_blocks_until_six_hours() {
    let e = env("cooldown");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (calls, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "首轮真实合并＝工具批＋终文两次请求");

    e.clock.advance(6 * HOUR - 1);
    assert_eq!(run(&e, &agent), Phase2Outcome::SkippedCooldown);
    e.clock.advance(1);
    // 恰 6h：可领取；内容无变化且产物有效 → A17 短路（零模型调用）
    let (calls2, agent2) = good_agent(&e);
    assert_eq!(run(&e, &agent2), Phase2Outcome::SucceededNoWorkspaceChanges { sql_committed: true });
    assert_eq!(calls2.load(Ordering::SeqCst), 0, "短路不得调用模型");
}

/// 租期内他人领取＝SkippedRunning；租期过后可被接管（旧令牌失权）。
#[test]
fn a16_lease_blocks_and_allows_takeover() {
    let e = env("lease");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (calls, agent) = good_agent(&e);
    // 直接占据全局任务（不动时钟）——运行中租约挡住第二领取者
    let taken = e.store.try_claim_global_phase2_job("holder", 3600).unwrap();
    assert!(matches!(
        taken,
        terminal_chat::memory::Phase2JobClaimOutcome::Claimed { .. }
    ));
    assert_eq!(run(&e, &agent), Phase2Outcome::SkippedRunning, "租期内他人被挡");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "租期内不得发起任何模型请求");
    e.clock.advance(3601);
    // 租期过后：接管成功（旧令牌失权由 A22 场景验证）
    let taken = e.store.try_claim_global_phase2_job("taker", 3600).unwrap();
    assert!(matches!(taken, terminal_chat::memory::Phase2JobClaimOutcome::Claimed { .. }));
}

/// 运行期心跳（上游生命周期：代理执行期间周期续租）：模型请求**阻塞期间**，
/// 受控时钟跨过多个 90s 心跳点——每个点后数据库租期须已续到该时刻＋租期
/// （请求未完成即可观测，非请求前后置）；有效续租期间第二领取者无法接管。
/// 轮询等待（真实 ≤5s）为测试同步手段，断言基于受控时钟值，非真实等待。
#[test]
fn heartbeat_renews_during_blocked_model_request() {
    let e = env("hb-blocked");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let first = merge_artifact_batch();
    let store = &e.store;
    let clock = &e.clock;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let agent = move |_b: &Value| {
        if counter.fetch_add(1, Ordering::SeqCst) > 0 {
            return Ok(final_reply("done"));
        }
        for step in [1i64, 2, 3] {
            // 请求保持未完成：推进受控时钟跨过 90s 心跳点
            clock.advance(JOB_HEARTBEAT_SECONDS);
            wait_lease(store, T0 + step * JOB_HEARTBEAT_SECONDS + 3600, &format!("tick {step}"));
            // 有效续租期间：他人领取被挡（租期恒在当前时刻＋3600）
            assert!(
                matches!(
                    store.try_claim_global_phase2_job("taker", 3600).unwrap(),
                    terminal_chat::memory::Phase2JobClaimOutcome::SkippedRunning
                ),
                "续租有效期间不得被接管（tick {step}）"
            );
        }
        Ok(first.clone())
    };
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
}

/// 轮询等待租期续到 `expected_min`（测试同步：真实 ≤5s 上限；断言值为受控时钟）。
fn wait_lease(store: &MemoryStore, expected_min: i64, label: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let lease = store
            .job_row("memory_consolidate_global", "global")
            .unwrap()
            .expect("全局任务行存在")
            .lease_until;
        if lease.is_some_and(|l| l >= expected_min) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("{label}：请求阻塞期间心跳未续租（期望 ≥{expected_min}，实际 {lease:?}）");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// 心跳墙钟退出边界（契约缺口回归）：**生产模型请求在途**、墙钟已跨过旧
/// 实现「10s 自限时带」后，业务时钟再进一个 90s 点——心跳线程必须仍然
/// 存活并续租（退出仅由请求结束信号驱动，不得有独立于请求生命周期的
/// 提前退出）。墙钟等待（≈10.5s）为捕获该停止条件的行为证据所必需，
/// 与业务时钟断言分立；不真实等待 90s/3,600s。
#[test]
fn heartbeat_survives_wall_clock_beyond_old_belt_while_request_in_flight() {
    let e = env("hb-wall");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let first = merge_artifact_batch();
    let store = &e.store;
    let clock = &e.clock;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let agent = move |_b: &Value| {
        if counter.fetch_add(1, Ordering::SeqCst) > 0 {
            return Ok(final_reply("done"));
        }
        // 请求在途：业务心跳点 1 续租（证明心跳存活）
        clock.advance(JOB_HEARTBEAT_SECONDS);
        wait_lease(store, T0 + JOB_HEARTBEAT_SECONDS + 3600, "墙钟跨越前");
        // 墙钟跨过旧 10s 自限时带（请求仍未完成）
        std::thread::sleep(std::time::Duration::from_millis(10_500));
        // 业务时钟再进一个 90s 点：心跳线程必须仍存活并续租
        clock.advance(JOB_HEARTBEAT_SECONDS);
        wait_lease(store, T0 + 2 * JOB_HEARTBEAT_SECONDS + 3600, "墙钟跨越后");
        Ok(first.clone())
    };
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
}

/// panic 展开：合并代理请求内 panic——展开经 sender（Drop 保护置停止位）
/// → `thread::scope` 展开路径 join 收口不挂死（测试能完成即证明）；且
/// panic 后心跳线程已终止（业务时钟推进＋真实等待后租期不再变化）。
#[test]
fn heartbeat_thread_stops_on_panic_unwind() {
    let e = env("hb-panic");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let store = &e.store;
    let clock = &e.clock;
    let agent = move |_b: &Value| -> Result<Value, String> {
        // 先让心跳续租一次（线程确认存活），再 panic
        clock.advance(JOB_HEARTBEAT_SECONDS);
        wait_lease(store, T0 + JOB_HEARTBEAT_SECONDS + 3600, "panic 前续租");
        panic!("模拟代理请求内崩溃");
    };
    let resolver = synthetic_meta(&e);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_phase2_consolidation(&e.store, &e.root, "w-panic", params(64), &resolver, &agent, None)
    }));
    assert!(result.is_err(), "panic 应向上传播（不吞没）");
    // 心跳线程已停：业务时钟再进一个 90s 点＋真实等待后，租期停在 panic 前值
    e.clock.advance(JOB_HEARTBEAT_SECONDS);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let lease = job_row(&e).lease_until;
    assert_eq!(lease, Some(T0 + JOB_HEARTBEAT_SECONDS + 3600), "panic 后无续租（线程已回收）");
    // 任务行保持运行态（panic 不做失败记账——由租期到期接管承接，上游同语义）
    let row = job_row(&e);
    assert_eq!(row.status, "running");
    assert!(row.last_error.is_none());
}

// —— A17/A18：无差异短路与输出缺失 ——

/// A17：基线已含当前内容且产物有效 → 成功水位＋selected 快照照写、不调模型、
/// 不 reset（HEAD 不变）、不残留差异文件。
#[test]
fn a17_no_diff_short_circuit_marks_without_model_or_reset() {
    let e = env("a17");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (_, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    let head_after_first = head(&e.root);
    e.clock.advance(6 * HOUR);

    let (calls2, agent2) = good_agent(&e);
    assert_eq!(
        run(&e, &agent2),
        Phase2Outcome::SucceededNoWorkspaceChanges { sql_committed: true }
    );
    assert_eq!(calls2.load(Ordering::SeqCst), 0, "零模型调用");
    assert_eq!(head(&e.root), head_after_first, "短路不 reset 基线");
    let row = job_row(&e);
    assert_eq!(row.status, "done");
    assert_eq!(row.last_success_watermark, Some(T0 - 100), "成功水位＝本轮快照最大源时间");
    assert!(!e.root.join(workspace::WORKSPACE_DIFF_FILENAME).exists(), "不残留差异文件");
    // selected 快照照写（t1 选中）
    let outputs = e.store.list_stage1_outputs().unwrap();
    assert!(outputs.iter().any(|o| o.thread_id == "t1" && o.selected_for_phase2));
}

/// A18：无差异但产物缺失 → 不走捷径。真实构造＝基线损坏重建后（ensure 以
/// 当前无产物树重立基线）同步无变化：`has_changes()==false` 且产物缺失——
/// 校验是短路的必要条件，必须重新合并补齐产物。
#[test]
fn a18_missing_artifacts_merge_path() {
    let e = env("a18");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (_, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    e.clock.advance(6 * HOUR);

    // 产物缺失＋基线损坏：ensure 重建的基线＝当前（无产物）树 → 无差异
    std::fs::remove_file(e.root.join("memory_summary.md")).unwrap();
    std::fs::remove_file(e.root.join("MEMORY.md")).unwrap();
    std::fs::write(e.root.join(".git").join("HEAD"), "garbage\n").unwrap();
    let head_corrupt_rebuild = {
        workspace::prepare_memory_workspace(&e.root, None).unwrap();
        head(&e.root)
    };
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(!d.has_changes(), "前置：重建基线后同步无差异");

    let (calls2, agent2) = good_agent(&e);
    assert!(
        matches!(run(&e, &agent2), Phase2Outcome::Succeeded { sql_committed: true }),
        "产物缺失不得短路"
    );
    assert_eq!(calls2.load(Ordering::SeqCst), 2, "必须重新合并（工具批＋终文）");
    assert!(e.root.join("memory_summary.md").exists(), "代理补齐摘要");
    assert!(e.root.join("MEMORY.md").exists());
    assert_ne!(head(&e.root), head_corrupt_rebuild, "本轮 reset 重建基线");
}

// —— A19：来源选择 ——

/// 排序＝usage_count↓ → COALESCE(last_usage, source_updated_at)↓ →
/// source_updated_at↓ → thread_id↓；窗口过滤；threads 启用门；上限与
/// thread_id 升序返回；分页（>512 行）。
#[test]
fn a19_selection_ordering_window_threads_and_paging() {
    let e = env("a19");
    seed_row(&e, "t-used", T0 - 50, "r", "s"); // usage 3, last_usage 新
    set_usage(&e, "t-used", Some(3), Some(T0 - 10));
    seed_row(&e, "t-hot", T0 - 20, "r", "s"); // usage 1, last_usage 最新
    set_usage(&e, "t-hot", Some(1), Some(T0 - 5));
    seed_row(&e, "t-fresh", T0 - 30, "r", "s"); // 从未用过，source 新
    seed_row(&e, "t-stale-unused", T0 - 90 * 86_400, "r", "s"); // 从未用过且超窗
    set_usage(&e, "t-stale-unused", None, None);
    seed_row(&e, "t-stale-used", T0 - 10, "r", "s"); // 用过但 last_usage 超窗
    set_usage(&e, "t-stale-used", Some(9), Some(T0 - 60 * 86_400));
    seed_row(&e, "t-disabled", T0 - 10, "r", "s"); // threads 禁用
    {
        let conn = rusqlite::Connection::open(memories_db_path(&e.root)).unwrap();
        conn.execute(
            "UPDATE threads SET memory_mode = 'disabled' WHERE thread_id = 't-disabled'",
            [],
        )
        .unwrap();
    }
    // 空内容行不入选
    seed_row(&e, "t-empty", T0 - 10, "  ", "");

    let sel = e.store.get_phase2_input_selection(10, 30).unwrap();
    let ids: Vec<&str> = sel.iter().map(|r| r.thread_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["t-fresh", "t-hot", "t-used"],
        "入选集正确、返回按 thread_id 稳定升序（排序决定取舍）：{ids:?}"
    );

    let top1 = e.store.get_phase2_input_selection(1, 30).unwrap();
    assert_eq!(top1.len(), 1);
    assert_eq!(top1[0].thread_id, "t-used", "n 截断取排序首位");

    assert!(e.store.get_phase2_input_selection(0, 30).unwrap().is_empty(), "n=0 空返回");

    // 分页：600 行、n=600 → 全量返回（页大小 512）
    {
        let conn = rusqlite::Connection::open(memories_db_path(&e.root)).unwrap();
        for i in 0..600 {
            let t = format!("z-{i:04}");
            conn.execute(
                "INSERT INTO stage1_outputs (thread_id, source_updated_at, raw_memory, rollout_summary, generated_at)
                 VALUES (?1, ?2, 'r', 's', ?3)",
                rusqlite::params![t, T0 - i, T0],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO threads (thread_id, memory_mode, updated_at_ms) VALUES (?1, 'enabled', ?2)",
                rusqlite::params![t, T0 * 1000],
            )
            .unwrap();
        }
    }
    let paged = e.store.get_phase2_input_selection(600, 30).unwrap();
    assert_eq!(paged.len(), 600, "跨页取满");
    let mut sorted = paged.iter().map(|r| r.thread_id.clone()).collect::<Vec<_>>();
    let mut expected = sorted.clone();
    expected.sort();
    assert_eq!(sorted, expected, "返回按 thread_id 升序");
    sorted.dedup();
    assert_eq!(sorted.len(), 600, "无重复");
}

// —— A21/A22：无效输出与失权 ——

/// A21：代理自称完成但产物无效 → 失败、不 reset、按 retry 记账；
/// 退避过后可重试成功。
#[test]
fn a21_invalid_output_fails_without_reset_and_retries() {
    let e = env("a21");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    // 「自称完成」代理：只回终文不写产物
    let bad = |_b: &Value| Ok(final_reply("I consolidated everything"));
    assert_eq!(run(&e, &bad), Phase2Outcome::Failed("failed_invalid_artifacts"));
    assert_eq!(commit_count(&e.root), 1, "未 reset：基线仍为初建单提交");
    let row = job_row(&e);
    assert_eq!(row.status, "error");
    assert_eq!(row.retry_remaining, 2, "3→2");
    assert_eq!(row.retry_at, Some(T0 + 3600), "重试退避 3600s");
    assert!(row.last_error.unwrap().contains("failed_invalid_artifacts"));

    e.clock.advance(3601);
    let (_, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
}

/// 合并失败不得推进成功水位：先成功建立旧水位，6 小时后新来源触发合并但
/// 产物缺失 → 失败记账，水位仍停在旧值，不冒充已处理新来源。
#[test]
fn failed_merge_does_not_advance_success_watermark() {
    let e = env("fail-watermark");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (_, good) = good_agent(&e);
    assert!(matches!(run(&e, &good), Phase2Outcome::Succeeded { sql_committed: true }));
    assert_eq!(job_row(&e).last_success_watermark, Some(T0 - 100));

    e.clock.advance(6 * HOUR);
    seed_row(&e, "t2", T0 - 50, "raw2", "summary2");
    let root = e.root.clone();
    let bad = move |_b: &Value| {
        // 模拟无效产物：必填文件被删除，写后校验失败。
        std::fs::remove_file(root.join("MEMORY.md")).unwrap();
        Ok(final_reply("done without required artifacts"))
    };
    assert_eq!(run(&e, &bad), Phase2Outcome::Failed("failed_invalid_artifacts"));

    let row = job_row(&e);
    assert_eq!(row.status, "error");
    assert_eq!(
        row.last_success_watermark,
        Some(T0 - 100),
        "失败不得推进成功水位"
    );
}

/// A22：失权（任务行已被他人令牌占据）→ 校验通过后再确认发现失权：不
/// reset、不成功、不把接读者标失败（上游 do nothing）。**租期驱动的接管
/// 已被运行期心跳阻断**（见 `heartbeat_renews_during_blocked_model_request`
/// ——时钟推进期间续租照常、他人领取恒 SkippedRunning）；「实际被接管」的
/// 终态（worker 崩溃后租期过期被领取、或心跳存储故障后失守）经直接改行
/// 模拟，聚焦失权后的处理面——旧所有者不能越权完成成功提交。
#[test]
fn a22_lost_ownership_after_validation_does_nothing() {
    let e = env("a22");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::clone(&counter);
    let first = merge_artifact_batch();
    let root = e.root.clone();
    let agent = move |_b: &Value| {
        if c2.fetch_add(1, Ordering::SeqCst) == 0 {
            // 模拟外部接管终态：任务行被他人令牌占据（心跳/再确认将失配）
            let conn = rusqlite::Connection::open(memories_db_path(&root)).unwrap();
            conn.execute(
                "UPDATE jobs SET worker_id = 'taker', ownership_token = 'taker-token' \
                 WHERE kind = 'memory_consolidate_global' AND job_key = 'global'",
                [],
            )
            .unwrap();
            Ok(first.clone())
        } else {
            Ok(final_reply("done"))
        }
    };
    assert_eq!(run(&e, &agent), Phase2Outcome::LostOwnershipAfterValidation);
    // 不 reset（基线单提交未变）、不成功记账、不把接读者的运行行标失败
    assert_eq!(commit_count(&e.root), 1);
    let row = job_row(&e);
    assert_eq!(row.status, "running", "接读者任务不受影响");
    assert_eq!(row.worker_id.as_deref(), Some("taker"));
    assert!(row.last_error.is_none(), "不替他人写失败");
}

// —— 失败路径与状态/副作用 ——

/// prepare 失败（根路径为文件）→ failed_prepare_workspace；sync 失败（线程
/// 元数据缺失）→ failed_sync_workspace_inputs；模型失败 → failed_agent；
/// reset 失败（.git 被外部破坏）→ failed_workspace_commit 且不写 SQL 成功。
#[test]
fn failure_reasons_and_side_effects() {
    // prepare：根为既有普通文件
    let tmp = common::TempDir::new("m5-prepare-fail");
    let not_dir = tmp.path().join("not-a-dir");
    std::fs::write(&not_dir, "x").unwrap();
    let clock = SteppedClock::at(T0);
    let store = MemoryStore::open(&tmp.path().join("elsewhere"), clock.clock()).unwrap();
    let resolver = |_t: &str| -> Option<ThreadFileMeta> { None };
    let agent = |_b: &Value| -> Result<Value, String> { unreachable!("prepare 失败在前") };
    assert_eq!(
        run_phase2_consolidation(&store, &not_dir, "w", params(64), &resolver, &agent, None),
        Phase2Outcome::Failed("failed_prepare_workspace")
    );

    // sync：线程元数据缺失
    let e = env("sync-fail");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let resolver = |_t: &str| -> Option<ThreadFileMeta> { None };
    let agent = |_b: &Value| -> Result<Value, String> { unreachable!("sync 失败在模型前") };
    assert_eq!(
        run_phase2_consolidation(&e.store, &e.root, "w", params(64), &resolver, &agent, None),
        Phase2Outcome::Failed("failed_sync_workspace_inputs")
    );
    assert_retry_accounting(&e, "sync-fail", T0);

    // 模型失败 → failed_agent
    let e = env("agent-fail");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let agent = |_b: &Value| -> Result<Value, String> { Err("模型不可用".into()) };
    assert_eq!(run(&e, &agent), Phase2Outcome::Failed("failed_agent"));
    let row = job_row(&e);
    assert_eq!(row.status, "error");
    assert!(row.last_error.unwrap().contains("failed_agent"));
    assert_retry_accounting(&e, "agent-fail", T0);

    // reset 失败：代理写完产物后外部破坏 .git → failed_workspace_commit、不写成功
    let e = env("reset-fail");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);
    let root = e.root.clone();
    let first = merge_artifact_batch();
    let agent = move |_b: &Value| {
        if c.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(first.clone())
        } else {
            // 终文前破坏基线元数据（测试外部破坏，非代理能力）
            std::fs::write(root.join(".git").join("HEAD"), "garbage\n").unwrap();
            Ok(final_reply("done"))
        }
    };
    assert_eq!(run(&e, &agent), Phase2Outcome::Failed("failed_workspace_commit"));
    let row = job_row(&e);
    assert_ne!(row.status, "done", "reset 失败不得写 SQL 成功");
}

// —— 水位与快照版本 ——

/// 成功水位＝本轮所选快照的最大 source_updated_at；合并期间新入库的行不
/// 冒充已处理版本（selected/水位均不含）。
#[test]
fn watermark_uses_selected_snapshot_not_latest() {
    let e = env("watermark");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    seed_row(&e, "t2", T0 - 200, "raw", "summary");
    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);
    let first = merge_artifact_batch();
    let root = e.root.clone();
    let agent = move |_b: &Value| {
        if c.fetch_add(1, Ordering::SeqCst) == 0 {
            // 合并期间外部写入一行更新的 stage1 结果（不含 enqueue）
            let conn = rusqlite::Connection::open(memories_db_path(&root)).unwrap();
            conn.execute(
                "INSERT INTO stage1_outputs (thread_id, source_updated_at, raw_memory, rollout_summary, generated_at)
                 VALUES ('t3', ?1, 'r3', 's3', ?1)",
                rusqlite::params![T0],
            )
            .unwrap();
            Ok(first.clone())
        } else {
            Ok(final_reply("done"))
        }
    };
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    let row = job_row(&e);
    // 上游 enqueue 语义：更新版本入队时旧水位＋1（t2@T0-200 入队把
    // T0-100 推到 T0-99——新输入到达即推进，防旧水位短路）；成功水位＝
    // max(领取水位, 快照源时间)＝T0-99，且不含合并期间新入的 t3@T0。
    assert_eq!(row.last_success_watermark, Some(T0 - 99), "水位＝领取水位（含 enqueue +1）与快照最大值");
    let outputs = e.store.list_stage1_outputs().unwrap();
    let t3 = outputs.iter().find(|o| o.thread_id == "t3").unwrap();
    assert!(!t3.selected_for_phase2, "合并期间的新行不进本轮 selected");
}

// —— 合并轮请求组装与工具面（经真实分派） ——

/// 合并轮请求体：tools 恰三项 mem_*、system＝CONSOLIDATION_SYSTEM、首条
/// user＝渲染后的合并提示（含根路径与差异文件名，占位符无残留）。
#[test]
fn merge_request_shape_tools_and_prompt() {
    let e = env("req-shape");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bodies);
    let first = merge_artifact_batch();
    let agent = move |b: &Value| {
        let n = {
            let mut g = sink.lock().unwrap();
            g.push(b.clone());
            g.len()
        };
        if n == 1 {
            Ok(first.clone())
        } else {
            Ok(final_reply("done"))
        }
    };
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    let captured = bodies.lock().unwrap().clone();
    let body = &captured[0];
    let tools: Vec<&str> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, vec!["mem_read", "mem_write", "mem_list"], "恰三项、无其他");
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], json!("system"));
    assert_eq!(msgs[0]["content"], json!(CONSOLIDATION_SYSTEM));
    assert_eq!(msgs[1]["role"], json!("user"));
    let prompt = msgs[1]["content"].as_str().unwrap();
    let expected = build_consolidation_prompt(&e.root);
    assert_eq!(prompt, expected, "用户消息＝合并提示全文");
    assert!(prompt.contains(&e.root.display().to_string()));
    assert!(prompt.contains("phase2_workspace_diff.md"));
    assert!(!prompt.contains("{{ "), "占位符无残留");
}

/// 分派层拒绝未注册工具：合并轮收到 exec_command/apply_patch/web_search
/// 不执行、零副作用、单 error 结构；同批合法 mem_write 照常执行。
/// 同一批调用在用户轮（User 模式）exec 照常执行——对照零回归。
#[test]
fn dispatch_rejects_unregistered_tools_in_merge_mode() {
    let e = env("dispatch");
    let dir = e.tmp.path().join("outside with space");
    std::fs::create_dir_all(&dir).unwrap();
    let probe = dir.join("probe.txt");
    let patch_probe = dir.join("probe2.txt");

    let calls = json!([
        tool_call("x1", "exec_command", json!({ "cmd": "echo PWNED > probe.txt", "workdir": dir.display().to_string() })),
        tool_call("x2", "apply_patch", json!({ "patch": format!("*** Begin Patch\n*** Add File: {}\n+x\n*** End Patch", patch_probe.display()) })),
        tool_call("x3", "web_search", json!({ "q": "x" })),
        // 030 S15：合并轮伪造 tool_search 沿用未知工具通用错误契约（§1.3）
        tool_call("x3b", "tool_search", json!({ "query": "x" })),
        tool_call("x4", "mem_write", json!({ "path": "MEMORY.md", "content": "# M\n" })),
    ]);
    let tc = assistant_with_calls(calls.as_array().unwrap().clone());
    let fin = final_reply("done");
    let mut history = Vec::new();
    let mut sent = 0usize;
    let root = e.root.clone();
    let system = CONSOLIDATION_SYSTEM;
    run_tool_turn_with_mode(
        &mut history,
        "merge",
        &mut |_| {
            sent += 1;
            if sent == 1 { Ok(tc.clone()) } else { Ok(fin.clone()) }
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::MemoryMerge { root: &root, system, expected_epoch: 0 },
    &ProcessManager::new(),
    )
    .unwrap();

    assert!(!probe.exists(), "exec_command 不得在合并轮执行");
    assert!(!patch_probe.exists(), "apply_patch 不得在合并轮执行");
    let results: Vec<(&String, &String)> = history
        .iter()
        .filter_map(|h| match h {
            terminal_chat::prompt::HistoryItem::ToolResult { tool_call_id, content, .. } => {
                Some((tool_call_id, content))
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 5);
    for (id, content) in &results {
        let v: Value = serde_json::from_str(content).unwrap();
        match id.as_str() {
            "x4" => {
                assert_eq!(v["ok"], json!(true), "合法 mem_write 执行：{v}");
                assert!(root.join("MEMORY.md").exists());
            }
            id => {
                assert!(
                    v.get("error").and_then(Value::as_str).unwrap_or("").contains("未知工具"),
                    "{id} 应按未知工具拒绝：{v}"
                );
                let names = v["error"].as_str().unwrap();
                assert!(names.contains("mem_read") && names.contains("mem_write") && names.contains("mem_list"),
                    "错误信息列出合并轮可用工具：{names}");
            }
        }
    }

    // 对照：同一 exec 调用在用户轮照常执行（默认契约零回归）
    let mut history2 = Vec::new();
    let tc2 = assistant_with_calls(vec![tool_call(
        "u1",
        "exec_command",
        json!({ "cmd": "echo USER > probe.txt", "workdir": dir.display().to_string() }),
    )]);
    let mut sent2 = 0usize;
    run_tool_turn(
        &mut history2,
        "go",
        &mut |_| {
            sent2 += 1;
            if sent2 == 1 { Ok(tc2.clone()) } else { Ok(fin.clone()) }
        },
        &mut |_| {},
        None,
        &mut |_| {},
    &ProcessManager::new(),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&probe).unwrap(), "USER\n", "用户轮 exec 正常");
}

/// 合并轮取消：首个调用执行后取消——未执行 mem 调用按各自形状回传
/// （write ok=false、read/list 单 error）。
#[test]
fn merge_cancel_shapes_per_tool() {
    let e = env("cancel-shape");
    let cancel = AtomicBool::new(false);
    let calls = json!([
        tool_call("c1", "mem_read", json!({ "path": "MEMORY.md" })),
        tool_call("c2", "mem_write", json!({ "path": "a.md", "content": "x" })),
        tool_call("c3", "mem_list", json!({})),
        tool_call("c4", "exec_command", json!({ "cmd": "true" })),
    ]);
    let tc = assistant_with_calls(calls.as_array().unwrap().clone());
    let mut history = Vec::new();
    let root = e.root.clone();
    let system = CONSOLIDATION_SYSTEM;
    let err = run_tool_turn_with_mode(
        &mut history,
        "merge",
        &mut |_| Ok(tc.clone()),
        &mut |n: terminal_chat::turn::TurnNotify| {
            if matches!(n, terminal_chat::turn::TurnNotify::ToolBegin { .. }) {
                cancel.store(true, Ordering::SeqCst);
            }
        },
        Some(&cancel),
        &mut |_| {},
        ToolMode::MemoryMerge { root: &root, system, expected_epoch: 0 },
    &ProcessManager::new(),
    )
    .unwrap_err();
    assert!(err.contains("用户已取消"));
    let results: Vec<(&String, Value)> = history
        .iter()
        .filter_map(|h| match h {
            terminal_chat::prompt::HistoryItem::ToolResult { tool_call_id, content, .. } => Some((
                tool_call_id,
                serde_json::from_str::<Value>(content).unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 4, "每个已记录调用恰一条结果");
    assert!(!e.root.join("a.md").exists(), "取消后的 mem_write 不得执行");
    assert_eq!(
        results[1].1,
        json!({ "ok": false, "error": "用户取消，本调用未执行" }),
        "mem_write 未执行形状"
    );
    assert_eq!(
        results[2].1,
        json!({ "error": "用户取消，本调用未执行" }),
        "mem_list 未执行形状"
    );
    assert_eq!(
        results[3].1,
        json!({ "error": "用户取消，本调用未执行" }),
        "未注册工具未执行＝通用结构"
    );
}

/// 合并轮沿用每轮 8 请求上限（恒返合法调用 → 第 8 批后报上限）。
#[test]
fn merge_turn_request_cap_shared() {
    let e = env("cap");
    let tc = assistant_with_calls(vec![tool_call("r", "mem_list", json!({}))]);
    let mut history = Vec::new();
    let mut sent = 0usize;
    let root = e.root.clone();
    let system = CONSOLIDATION_SYSTEM;
    let err = run_tool_turn_with_mode(
        &mut history,
        "merge",
        &mut |_| {
            sent += 1;
            Ok(tc.clone())
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::MemoryMerge { root: &root, system, expected_epoch: 0 },
    &ProcessManager::new(),
    )
    .unwrap_err();
    assert_eq!(sent, 8);
    assert!(err.contains("已达本轮模型请求上限（8）"), "{err}");
}

// —— mem_* 工具语义（直接执行层） ——

fn exec_tool(e: &Env, name: &str, args: &Value) -> Value {
    serde_json::from_str(&mem_tools::execute(name, &args.to_string(), &e.root, 0)).unwrap()
}

#[test]
fn mem_read_line_budget_and_errors() {
    let e = env("mem-read");
    std::fs::write(e.root.join("notes.md"), "l1\nl2\nl3\nl4\n").unwrap();

    let v = exec_tool(&e, "mem_read", &json!({ "path": "notes.md" }));
    assert_eq!(v["content"], json!("l1\nl2\nl3\nl4\n"));
    assert_eq!(v["start_line_number"], json!(1));
    assert_eq!(v["truncated"], json!(false));

    let v = exec_tool(&e, "mem_read", &json!({ "path": "notes.md", "line_offset": 2, "max_lines": 2 }));
    assert_eq!(v["content"], json!("l2\nl3\n"));
    assert_eq!(v["start_line_number"], json!(2));
    assert_eq!(v["truncated"], json!(true), "max_lines 截到文件中部＝truncated");

    let v = exec_tool(&e, "mem_read", &json!({ "path": "notes.md", "line_offset": 4 }));
    assert_eq!(v["content"], json!("l4\n"));
    assert_eq!(v["truncated"], json!(false), "恰至文件尾");

    // token 预算：内容 8KB、max_tokens=16（≈64B）→ 中间截断
    std::fs::write(e.root.join("big.md"), format!("{}\n", "x".repeat(8192))).unwrap();
    let v = exec_tool(&e, "mem_read", &json!({ "path": "big.md", "max_tokens": 16 }));
    assert!(v["truncated"] == json!(true));
    assert!(v["content"].as_str().unwrap().len() < 8192);

    // 参数错误
    for bad in [
        json!({ "path": "notes.md", "line_offset": 0 }),
        json!({ "path": "notes.md", "max_lines": 0 }),
        json!({ "path": "notes.md", "line_offset": 99 }),
        json!({ "path": "missing.md" }),
        json!({ "path": "rollout_summaries" }),
        json!({}),
    ] {
        let v = exec_tool(&e, "mem_read", &bad);
        assert!(v.get("error").is_some_and(Value::is_string), "参数 {bad} 应报错：{v}");
    }
}

#[test]
fn mem_read_boundaries() {
    let e = env("mem-read-bound");
    std::fs::write(e.root.join("ok.md"), "x").unwrap();
    std::fs::create_dir_all(e.root.join("sub")).unwrap();
    std::os::unix::fs::symlink("ok.md", e.root.join("link.md")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", e.root.join("abs.md")).unwrap();
    // 库文件文本化：拒绝必须来自边界本身，而非二进制内容恰非 UTF-8
    std::fs::write(e.root.join("memories.sqlite"), "plain-text-db").unwrap();
    std::fs::write(e.root.join("memories.sqlite-journal"), "plain-j").unwrap();
    std::fs::write(e.root.join("sub").join("memories.sqlite"), "nested-db").unwrap();

    let cases = [
        (json!({ "path": "/etc/passwd" }), "记忆根内"),          // 绝对路径
        (json!({ "path": "../secrets" }), "记忆根内"),           // 父目录越界
        (json!({ "path": "sub/../../x" }), "记忆根内"),          // 归一化后越界
        (json!({ "path": ".git/config" }), "隐藏路径"),          // 隐藏路径
        (json!({ "path": "memories.sqlite" }), "不可访问"),      // 库文件
        (json!({ "path": "memories.sqlite-journal" }), "不可访问"),
        (json!({ "path": "sub/memories.sqlite" }), "不可访问"),  // 任意层级同名
        (json!({ "path": "link.md" }), "符号链接"),              // 符号链接终态
        (json!({ "path": "abs.md" }), "符号链接"),               // 指向外的符号链接
        (json!({ "path": "" }), "不能为空"),                     // 空路径
    ];
    for (bad, expect) in cases {
        let v = exec_tool(&e, "mem_read", &bad);
        let err = v
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{bad} 必须拒绝：{v}"));
        assert!(err.contains(expect), "{bad} 拒绝理由应含「{expect}」：{err}");
    }
}

#[test]
fn mem_write_semantics_and_limits() {
    let e = env("mem-write");
    std::fs::create_dir_all(e.root.join("sub")).unwrap();

    let v = exec_tool(&e, "mem_write", &json!({ "path": "new.md", "content": "hello" }));
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(std::fs::read_to_string(e.root.join("new.md")).unwrap(), "hello");
    assert_eq!(v["bytes"], json!(5));

    let v = exec_tool(&e, "mem_write", &json!({ "path": "new.md", "content": "over" }));
    assert_eq!(v["ok"], json!(true), "覆盖常规文件");
    assert_eq!(std::fs::read_to_string(e.root.join("new.md")).unwrap(), "over");

    let v = exec_tool(&e, "mem_write", &json!({ "path": "sub/nested.md", "content": "n" }));
    assert_eq!(v["ok"], json!(true));

    for bad in [
        json!({ "path": "no-such-dir/x.md", "content": "x" }), // 父目录不存在
        json!({ "path": "sub", "content": "x" }),              // 目录目标
        json!({ "path": ".git/config", "content": "x" }),      // 隐藏路径
        json!({ "path": "memories.sqlite", "content": "x" }),  // 库文件
        json!({ "path": "../out.md", "content": "x" }),        // 越界
        json!({ "path": "/tmp/out.md", "content": "x" }),      // 绝对路径
        json!({ "content": "x" }),                             // 缺 path
        json!({ "path": "only-path" }),                        // 缺 content
        json!({ "path": 5, "content": "x" }),                  // 类型错
    ] {
        let v = exec_tool(&e, "mem_write", &bad);
        assert_eq!(v["ok"], json!(false), "{bad} 必须拒绝：{v}");
        assert!(v.get("error").is_some_and(Value::is_string));
    }

    // 符号链接目标拒绝
    std::os::unix::fs::symlink("new.md", e.root.join("ln.md")).unwrap();
    let v = exec_tool(&e, "mem_write", &json!({ "path": "ln.md", "content": "x" }));
    assert_eq!(v["ok"], json!(false), "符号链接目标拒绝：{v}");

    // 超上限内容拒绝（1 MiB＋1 字节）
    let big = "x".repeat(1024 * 1024 + 1);
    let v = exec_tool(&e, "mem_write", &json!({ "path": "big.md", "content": big }));
    assert_eq!(v["ok"], json!(false), "超 1 MiB 拒绝");
    assert!(!e.root.join("big.md").exists(), "拒绝时不留半成品");
    // 恰上限成功
    let edge = "x".repeat(1024 * 1024);
    let v = exec_tool(&e, "mem_write", &json!({ "path": "edge.md", "content": edge }));
    assert_eq!(v["ok"], json!(true), "恰 1 MiB 允许");
}

#[test]
fn mem_list_filtering_paging_and_errors() {
    let e = env("mem-list");
    std::fs::write(e.root.join("MEMORY.md"), "# M").unwrap();
    std::fs::write(e.root.join("memory_summary.md"), "v1\n").unwrap();
    std::fs::write(e.root.join("memories.sqlite"), "db").unwrap();
    std::fs::write(e.root.join("memories.sqlite-journal"), "j").unwrap();
    std::fs::write(e.root.join(".hidden"), "h").unwrap();
    std::fs::create_dir_all(e.root.join("rollout_summaries")).unwrap();
    std::fs::create_dir_all(e.root.join(".secret-dir")).unwrap();
    std::os::unix::fs::symlink("MEMORY.md", e.root.join("ln.md")).unwrap();

    let v = exec_tool(&e, "mem_list", &json!({}));
    let entries = v["entries"].as_array().unwrap().clone();
    let paths: Vec<&str> = entries.iter().map(|x| x["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        vec!["MEMORY.md", "memory_summary.md", "rollout_summaries"],
        "隐藏/库文件/符号链接条目全部不展示：{paths:?}"
    );
    assert_eq!(entries[0]["entry_type"], json!("file"));
    assert_eq!(entries[2]["entry_type"], json!("directory"));
    assert_eq!(v["truncated"], json!(false));
    assert!(v.get("next_cursor").is_none() || v["next_cursor"].is_null());

    // 分页
    let v = exec_tool(&e, "mem_list", &json!({ "max_results": 1 }));
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    assert_eq!(v["next_cursor"], json!("1"));
    assert_eq!(v["truncated"], json!(true));
    let v = exec_tool(&e, "mem_list", &json!({ "cursor": "1", "max_results": 1 }));
    assert_eq!(v["entries"][0]["path"], json!("memory_summary.md"));

    // 子目录与文件路径
    std::fs::write(e.root.join("rollout_summaries").join("a.md"), "s").unwrap();
    let v = exec_tool(&e, "mem_list", &json!({ "path": "rollout_summaries" }));
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    let v = exec_tool(&e, "mem_list", &json!({ "path": "MEMORY.md" }));
    assert_eq!(v["entries"][0]["path"], json!("MEMORY.md"));

    for bad in [
        json!({ "cursor": "x" }),
        json!({ "cursor": "99" }),
        json!({ "path": "nope" }),
        json!({ "path": ".git" }),
        json!({ "path": "memories.sqlite" }),
    ] {
        let v = exec_tool(&e, "mem_list", &bad);
        assert!(v.get("error").is_some_and(Value::is_string), "{bad} 应报错：{v}");
    }
}

// —— G8：基线三原语 ——

#[test]
fn git_ensure_creates_and_preserves_baseline() {
    let e = env("git-ensure");
    std::fs::write(e.root.join("seed.md"), "s").unwrap();
    workspace::prepare_memory_workspace(&e.root, None).unwrap();
    let h1 = head(&e.root);
    assert_eq!(commit_count(&e.root), 1, "初建＝单提交基线");
    assert!(tree_has(&e.root, "seed.md"), "既有内容入基线");

    workspace::prepare_memory_workspace(&e.root, None).unwrap();
    assert_eq!(head(&e.root), h1, "健康基线保持不动（幂等）");

    // 损坏恢复：unborn/损坏 HEAD → rev-parse 失败 → init 幂等重建＋普通提交
    std::fs::write(e.root.join(".git").join("HEAD"), "garbage\n").unwrap();
    workspace::prepare_memory_workspace(&e.root, None).unwrap();
    assert_eq!(commit_count(&e.root), 1, "重建后仍单提交");
}

#[test]
fn git_diff_reports_changes_and_honors_exclusions() {
    let e = env("git-diff");
    std::fs::write(e.root.join("base.md"), "v1\n").unwrap();
    workspace::prepare_memory_workspace(&e.root, None).unwrap();

    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(!d.has_changes(), "同步后无变化");

    std::fs::write(e.root.join("new.md"), "n").unwrap();
    std::fs::write(e.root.join("base.md"), "v2\n").unwrap();
    std::fs::write(e.root.join("memories.sqlite"), "db-bytes").unwrap();
    std::fs::write(e.root.join("memories.sqlite-journal"), "j").unwrap();

    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    let paths: Vec<&str> = d.changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, vec!["base.md", "new.md"], "排除集外才报差异：{paths:?}");
    assert_eq!(d.changes[0].status.label(), "M");
    assert_eq!(d.changes[1].status.label(), "A");
    assert!(d.unified_diff.contains("+n"), "统一 diff 含新增内容");
    assert!(!d.unified_diff.contains("db-bytes"), "库内容不入 diff");

    // 删除检测：先 reset 让 new.md 入基线，再删除 → D
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    std::fs::remove_file(e.root.join("new.md")).unwrap();
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(
        d.changes.iter().any(|c| c.path == "new.md" && c.status.label() == "D"),
        "基线内文件删除可检测：{:?}",
        d.changes
    );

    // 临时差异文件三处前置删除：写入后计算 diff 不得把它当输入
    workspace::write_workspace_diff(&e.root, &d).unwrap();
    assert!(e.root.join(workspace::WORKSPACE_DIFF_FILENAME).exists());
    let d2 = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(!d2.changes.iter().any(|c| c.path == workspace::WORKSPACE_DIFF_FILENAME));
    assert!(!e.root.join(workspace::WORKSPACE_DIFF_FILENAME).exists(), "diff 计算已删除临时文件");

    // .gitignore＝*.md 时新摘要仍可检测（-f 强制入基线）
    std::fs::write(e.root.join(".gitignore"), "*.md\n").unwrap();
    std::fs::write(e.root.join("fresh-summary.md"), "fs").unwrap();
    let d3 = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(d3.changes.iter().any(|c| c.path == "fresh-summary.md"), "被忽略的新文件仍可见：{:?}", d3.changes);
}

// —— M8 D12-A（2026-10-19 批准）：便条差异过滤 ——

const M8_NOTE_DIR: &str = "extensions/ad_hoc/notes";

/// 合法便条进入差异（状态列表＋正文）；非法便条（文件名/长度/子目录/空白
/// 正文）从两清单同时排除；同层 instructions.md 不受便条规则影响。
#[test]
fn m8_note_diff_filter_valid_visible_invalid_excluded() {
    let e = env("m8-note-filter");
    workspace::prepare_memory_workspace(&e.root, None).unwrap();
    std::fs::create_dir_all(e.root.join(M8_NOTE_DIR)).unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("2026-10-19T10-00-00-prefer-chinese.md"), "用户明确要求：默认用中文。\n").unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("bad name!!.md"), "有正文但名字非法\n").unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("2026-10-19T10-00-00-UPPER.md"), "大写 slug 非法\n").unwrap();
    std::fs::write(
        e.root.join(M8_NOTE_DIR).join(&format!("2026-10-19T10-00-00-{}.md", "a".repeat(90))),
        "名字超长\n",
    )
    .unwrap();
    std::fs::create_dir_all(e.root.join(M8_NOTE_DIR).join("sub")).unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("sub").join("2026-10-19T10-00-00-a.md"), "子目录非平铺\n").unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("2026-10-19T10-00-00-blank.md"), "  \t \n ").unwrap();
    std::fs::write(e.root.join("extensions/ad_hoc/instructions.md"), "seed instructions\n").unwrap();

    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    let paths: Vec<&str> = d.changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "extensions/ad_hoc/instructions.md",
            "extensions/ad_hoc/notes/2026-10-19T10-00-00-prefer-chinese.md",
        ],
        "恰一合法便条与同层指引可见，其余全部排除：{paths:?}"
    );
    assert!(d.unified_diff.contains("默认用中文"), "合法便条正文入差异");
    assert!(!d.unified_diff.contains("名字非法"), "非法名便条正文不入差异");
    assert!(!d.unified_diff.contains("名字超长"), "超长名便条不入差异");
    assert!(!d.unified_diff.contains("子目录非平铺"), "子目录便条不入差异");
    assert!(!d.unified_diff.contains("seed instructions") == false, "指引文件照常入差异");
    assert!(d.has_changes());
}

/// 混合变化不误删正常差异；仅非法便条单独变化＝无差异（不触发合并）。
#[test]
fn m8_note_diff_filter_mixed_and_invalid_only() {
    let e = env("m8-note-mixed");
    std::fs::write(e.root.join("MEMORY.md"), "base\n").unwrap();
    workspace::prepare_memory_workspace(&e.root, None).unwrap();

    // 混合：正常文件变化＋非法便条
    std::fs::write(e.root.join("MEMORY.md"), "changed\n").unwrap();
    std::fs::create_dir_all(e.root.join(M8_NOTE_DIR)).unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("not-a-note.md"), "非法名\n").unwrap();
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    let paths: Vec<&str> = d.changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, vec!["MEMORY.md"], "正常变化保留、非法便条排除：{paths:?}");
    assert!(d.unified_diff.contains("+changed"), "正常差异正文保留");
    assert!(!d.unified_diff.contains("非法名"), "非法便条正文排除");

    // 基线推进后，仅再出现非法便条 → has_changes=false（A17 无差异短路键）
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("another bad.md"), "又一条非法\n").unwrap();
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(!d.has_changes(), "仅非法便条变化＝无差异（不触发合并）");
    assert!(d.unified_diff.trim().is_empty(), "正文同样为空");

    // 删除路径：合法名删除可见（D），非法名删除被排除
    std::fs::write(e.root.join(M8_NOTE_DIR).join("2026-10-19T11-00-00-gone.md"), "待删\n").unwrap();
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    std::fs::remove_file(e.root.join(M8_NOTE_DIR).join("2026-10-19T11-00-00-gone.md")).unwrap();
    std::fs::remove_file(e.root.join(M8_NOTE_DIR).join("another bad.md")).unwrap();
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    let paths: Vec<&str> = d.changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, vec!["extensions/ad_hoc/notes/2026-10-19T11-00-00-gone.md"], "恰合法名删除可见：{paths:?}");
    assert_eq!(d.changes[0].status.label(), "D");
}

/// 非法便条单独变化时，phase2 走既有无差异短路——不调用模型、产物有效
/// 即 SucceededNoWorkspaceChanges（D12-A 批准语义的管线级证据）。
#[test]
fn m8_invalid_note_only_takes_no_changes_shortcut() {
    let e = env("m8-note-shortcut");
    seed_row(&e, "auto-20261019T101112-4321", T0 - 100, "raw", "summary");
    // 先真实合并一次：生成有效产物＋基线（A17 前提）
    let (calls, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    assert!(calls.load(Ordering::SeqCst) >= 1);
    // 越过 6 小时成功冷却（A16），使第二轮可领取
    e.clock.advance(6 * HOUR + 1);

    // 仅新增一条非法便条
    std::fs::create_dir_all(e.root.join(M8_NOTE_DIR)).unwrap();
    std::fs::write(e.root.join(M8_NOTE_DIR).join("invalid!.md"), "非法便条\n").unwrap();

    // chat 任何模型请求都视为失败（短路成功则零请求）
    let model_called = std::cell::Cell::new(0usize);
    let resolver = synthetic_meta(&e);
    let outcome = run_phase2_consolidation(
        &e.store,
        &e.root,
        "w-m8-note",
        params(64),
        &resolver,
        &|_body| {
            model_called.set(model_called.get() + 1);
            Err::<serde_json::Value, String>("无差异短路下不得调用模型".to_string())
        },
        None,
    );
    assert_eq!(model_called.get(), 0, "短路＝零模型请求");
    assert!(matches!(
        outcome,
        Phase2Outcome::SucceededNoWorkspaceChanges { .. }
    ));
    // 盘上非法便条保留（D12-A：不删除、不写入时拒绝）
    assert!(e.root.join(M8_NOTE_DIR).join("invalid!.md").is_file());
    // 对照：合法便条单独变化＝有差异（不会被误过滤）
    std::fs::write(
        e.root.join(M8_NOTE_DIR).join("2026-10-19T10-00-00-valid.md"),
        "合法便条\n",
    )
    .unwrap();
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(d.has_changes(), "合法便条可见＝不误伤");
    assert!(d.changes.iter().any(|c| c.path.ends_with("2026-10-19T10-00-00-valid.md")));
}

#[test]
fn git_reset_single_commit_and_tracked_exclusion() {
    let e = env("git-reset");
    std::fs::write(e.root.join("a.md"), "a\n").unwrap();
    workspace::prepare_memory_workspace(&e.root, None).unwrap();

    std::fs::write(e.root.join("b.md"), "b\n").unwrap();
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    assert_eq!(commit_count(&e.root), 1, "reset 后仍单提交（amend）");
    assert!(tree_has(&e.root, "b.md"));
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(!d.has_changes(), "reset 后差异清零");

    // 连续 reset：单提交基线恒成立
    std::fs::write(e.root.join("c.md"), "c\n").unwrap();
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    assert_eq!(commit_count(&e.root), 1);

    // 曾被跟踪的排除文件：手工构造「旧基线曾提交 memories.sqlite」
    std::fs::write(e.root.join("memories.sqlite"), "real-db").unwrap();
    {
        // 一次性临时构造（绕过生产排除集，模拟历史遗留基线）
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&e.root)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/local/bin")
                .env("GIT_CONFIG_COUNT", "0")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("LC_ALL", "C")
                .output()
                .unwrap()
        };
        let out = run(&["add", "-f", "memories.sqlite"]);
        assert!(out.status.success(), "遗留暂存应成功");
        let out = run(&[
            "-c",
            "user.name=legacy",
            "-c",
            "user.email=legacy@x",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--amend",
            "--allow-empty",
            "-m",
            "legacy-baseline",
        ]);
        assert!(
            out.status.success(),
            "遗留基线构造应成功：{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(tree_has(&e.root, "memories.sqlite"), "前置：遗留基线确实跟踪库文件");
    // diff：库文件改动不报（pathspec 排除对已跟踪文件同样生效）
    std::fs::write(e.root.join("memories.sqlite"), "real-db-mutated").unwrap();
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(!d.changes.iter().any(|c| c.path == "memories.sqlite"), "已跟踪库文件不报差异");
    // reset：库文件退出基线且真实文件不被删除
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    assert!(!tree_has(&e.root, "memories.sqlite"), "reset 后基线不再含库文件");
    assert!(e.root.join("memories.sqlite").exists(), "排除不删真实库文件");
    assert_eq!(
        std::fs::read_to_string(e.root.join("memories.sqlite")).unwrap(),
        "real-db-mutated",
        "内容不被破坏"
    );
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(!d.has_changes(), "退出基线后库文件改动彻底不可见");
}

/// 记忆根嵌在父仓库内：三原语不得发现/复用/改动父仓库（显式
/// --git-dir/--work-tree 锚定＋.git 缺失时也不向上搜索）。
#[test]
fn git_primitives_never_touch_parent_repository() {
    let tmp = common::TempDir::new("m5-parent-repo");
    let parent = tmp.path().join("parent");
    std::fs::create_dir_all(&parent).unwrap();
    std::fs::write(parent.join("tracked.txt"), "v1").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "tracked.txt"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "base",
        ],
    ] {
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(&parent)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert!(out.status.success(), "父仓库构造 {args:?}");
    }
    let parent_head = git(&parent, &["rev-parse", "HEAD"]).1;
    let root = parent.join("memories");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("m.md"), "m").unwrap();

    // .git 尚不存在：ensure/init 不得向上发现父仓库
    workspace::prepare_memory_workspace(&root, None).unwrap();
    assert!(root.join(".git").is_dir(), "基线建在记忆根内");
    assert_eq!(git(&parent, &["rev-parse", "HEAD"]).1, parent_head, "父 HEAD 不变");
    workspace::memory_workspace_diff(&root, None).unwrap();
    workspace::reset_memory_workspace_baseline(&root, None).unwrap();
    assert_eq!(git(&parent, &["rev-parse", "HEAD"]).1, parent_head, "父 HEAD 仍不变");
    let (_, status) = {
        let out = std::process::Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=no"])
            .current_dir(&parent)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .env("GIT_CONFIG_COUNT", "0")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    assert!(status.is_empty(), "父仓库已跟踪文件零改动：{status}");
    let (_, staged) = git(&parent, &["diff", "--cached", "--name-only"]);
    assert!(staged.is_empty(), "父仓库索引零改动：{staged}");
}

/// 差异文件渲染：状态列表＋diff 块；无变化＝none；超 4 MiB 截断标记。
#[test]
fn workspace_diff_file_render_and_truncation() {
    let e = env("diff-render");
    let d = terminal_chat::memory::GitBaselineDiff {
        changes: vec![
            terminal_chat::memory::GitBaselineChange {
                status: terminal_chat::memory::GitBaselineChangeStatus::Added,
                path: "x.md".into(),
            },
        ],
        unified_diff: "+++ x.md\n+new\n".into(),
    };
    workspace::write_workspace_diff(&e.root, &d).unwrap();
    let text = std::fs::read_to_string(e.root.join(workspace::WORKSPACE_DIFF_FILENAME)).unwrap();
    assert!(text.starts_with("# Memory Workspace Diff"));
    assert!(text.contains("## Status"));
    assert!(text.contains("- A x.md"));
    assert!(text.contains("```diff"));
    assert!(text.contains("+new"));

    let empty = terminal_chat::memory::GitBaselineDiff {
        changes: vec![],
        unified_diff: String::new(),
    };
    workspace::write_workspace_diff(&e.root, &empty).unwrap();
    let text = std::fs::read_to_string(e.root.join(workspace::WORKSPACE_DIFF_FILENAME)).unwrap();
    assert!(text.contains("- none"));
    assert!(!text.contains("```diff"), "无差异不含 diff 块");

    let huge = terminal_chat::memory::GitBaselineDiff {
        changes: vec![terminal_chat::memory::GitBaselineChange {
            status: terminal_chat::memory::GitBaselineChangeStatus::Modified,
            path: "big.md".into(),
        }],
        unified_diff: "x".repeat(5 * 1024 * 1024),
    };
    workspace::write_workspace_diff(&e.root, &huge).unwrap();
    let text = std::fs::read_to_string(e.root.join(workspace::WORKSPACE_DIFF_FILENAME)).unwrap();
    assert!(text.contains("[workspace diff truncated at 4194304 bytes]"), "截断标记存在");
    assert!(text.len() < 5 * 1024 * 1024, "文件有界");
}

/// 写后校验（V1）：MEMORY.md 常规文件＋摘要首行 v1＋无符号链接（有则先删并报无效）。
#[test]
fn validate_artifacts_rules() {
    let e = env("validate");
    assert!(workspace::validate_consolidation_artifacts(&e.root).is_err(), "产物缺失＝无效");

    std::fs::write(e.root.join("MEMORY.md"), "# M").unwrap();
    assert!(workspace::validate_consolidation_artifacts(&e.root).is_err(), "仍缺摘要");
    std::fs::write(e.root.join("memory_summary.md"), "not-v1\n").unwrap();
    assert!(workspace::validate_consolidation_artifacts(&e.root).is_err(), "首行非 v1");
    std::fs::write(e.root.join("memory_summary.md"), "v1\n- s").unwrap();
    workspace::validate_consolidation_artifacts(&e.root).unwrap();

    std::fs::remove_file(e.root.join("MEMORY.md")).unwrap();
    std::fs::create_dir(e.root.join("MEMORY.md")).unwrap();
    assert!(workspace::validate_consolidation_artifacts(&e.root).is_err(), "MEMORY.md 非常规文件");
    std::fs::remove_dir(e.root.join("MEMORY.md")).unwrap();
    std::fs::write(e.root.join("MEMORY.md"), "# M").unwrap();

    std::os::unix::fs::symlink("memory_summary.md", e.root.join("evil.md")).unwrap();
    assert!(workspace::validate_consolidation_artifacts(&e.root).is_err(), "符号链接＝无效输出");
    assert!(!e.root.join("evil.md").exists(), "校验已移除链接");
    workspace::validate_consolidation_artifacts(&e.root).unwrap();
}

// —— 配置隔离与固定 identity ——

/// 注入 env 配置（GIT_CONFIG 注入面/GIT_DIR 泄漏）下三原语照常工作、输出
/// 格式不变（a//b/ 前缀保留）；无全局 identity 时提交用固定 identity。
/// 敌意 env 由**子进程**承载（重新执行本测试二进制）：避免本进程
/// `set_var` 与并行测试/生产代码 `getenv` 的数据竞争（POSIX UB）。
#[test]
fn git_config_isolation_and_fixed_identity() {
    let exe = std::env::current_exe().unwrap();
    let tmp = common::TempDir::new("m5-isolation-parent");
    let evil_config = tmp.path().join("evil.gitconfig");
    std::fs::write(&evil_config, "[diff]\n\tnoprefix = true\n").unwrap();
    let excludes = tmp.path().join("excludes");
    std::fs::write(&excludes, "*.md\n").unwrap();
    let out = std::process::Command::new(exe)
        .args(["git_config_isolation_inner", "--exact"])
        .env("M5_ISOLATION_CHILD", "1")
        .env("M5_EVIL_GITCONFIG", &evil_config)
        .env("M5_EVIL_EXCLUDES", &excludes)
        .env("GIT_DIR", "/definitely/not/here.git")
        .env("GIT_WORK_TREE", "/definitely/not/here")
        .env("GIT_INDEX_FILE", "/definitely/not/here.idx")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "diff.noprefix")
        .env("GIT_CONFIG_VALUE_0", "true")
        .env("GIT_CONFIG_KEY_1", "core.excludesfile")
        .env("GIT_CONFIG_VALUE_1", &excludes)
        .env("GIT_CONFIG_GLOBAL", &evil_config)
        .env("GIT_CONFIG_SYSTEM", &evil_config)
        .output()
        .expect("测试二进制可重新执行");
    assert!(
        out.status.success(),
        "子进程隔离用例应通过：\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 隔离场景的实际执行体（仅当 `M5_ISOLATION_CHILD` 置位时执行——由父用例
/// 以敌意 env 重新执行本二进制触发；正常套件运行时恒空过）。
#[test]
fn git_config_isolation_inner() {
    if std::env::var_os("M5_ISOLATION_CHILD").is_none() {
        return;
    }
    let e = env("git-isolation-child");
    std::fs::write(e.root.join("a.md"), "a\n").unwrap();
    workspace::prepare_memory_workspace(&e.root, None).unwrap();
    std::fs::write(e.root.join("b.md"), "b\n").unwrap();
    let d = workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(
        d.changes.iter().any(|c| c.path == "b.md"),
        "excludesfile 注入不得隐藏新文件：{:?}",
        d.changes
    );
    assert!(
        d.unified_diff.contains("a/b.md") || d.unified_diff.contains("b/b.md"),
        "noprefix 注入不得改变 diff 前缀格式：{}",
        d.unified_diff.lines().take(6).collect::<Vec<_>>().join("\n")
    );
    workspace::reset_memory_workspace_baseline(&e.root, None).unwrap();
    let (_, name) = git(&e.root, &["log", "-1", "--format=%an|%ae"]);
    assert_eq!(name, "terminal-chat|memory@terminal-chat.invalid", "固定 identity：{name}");
}

// —— 取消传播（G8：取消标志传至 git 子进程） ——

/// 取消预置：基线操作立即让位（不等待 git 完成）。
#[test]
fn git_cancel_flag_stops_primitives() {
    let e = env("git-cancel");
    std::fs::write(e.root.join("a.md"), "a").unwrap();
    let cancel = AtomicBool::new(true);
    let r = workspace::prepare_memory_workspace(&e.root, Some(&cancel));
    assert!(r.is_err(), "预置取消应中止基线操作：{r:?}");
}

// —— 模板渲染 ——

#[test]
fn consolidation_prompt_renders_root_and_diff_name() {
    let e = env("prompt-render");
    let p = build_consolidation_prompt(&e.root);
    assert!(p.contains(&e.root.display().to_string()));
    assert!(p.contains("phase2_workspace_diff.md"));
    assert!(p.contains("is a git repository managed by Codex"), "模板 git 假设原文保留");
    assert!(!p.contains("{{ "), "占位符全部渲染");
    assert!(!p.contains("memory extensions"), "extensions 块为空（V1 无该概念）");
}

// —— 合并轮隔离 ——

/// phase2 全程不建 sessions/、不写记忆根外的用户会话文件。
#[test]
fn phase2_keeps_internal_session_in_memory() {
    let e = env("no-sessions");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (_, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    assert!(!e.tmp.path().join("sessions").exists(), "不落 sessions/");
    assert!(!e.tmp.path().join("w").exists(), "合成元数据目录不落盘（仅渲染进文件头）");
}

/// 领取后失败的记账面（各路径一致）：状态 error、重试计数 3→2、退避
/// `now+3,600s`（`now`＝该轮领取时钟）。
fn assert_retry_accounting(e: &Env, label: &str, now: i64) {
    let row = job_row(e);
    assert_eq!(row.status, "error", "{label}");
    assert_eq!(row.retry_remaining, 2, "{label}：重试计数 3→2");
    assert_eq!(row.retry_at, Some(now + 3600), "{label}：退避 3,600s");
}

/// 非原子语义（上游 §9.8/G8⑨）：校验失败＝部分文件变化**不回滚**——代理
/// 已写的有效摘要与额外文件保留在磁盘，基线不重置、按失败记账。
#[test]
fn partial_changes_not_rolled_back_on_validation_failure() {
    let e = env("partial-no-rollback");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let tc = assistant_with_calls(vec![
        tool_call("p1", "mem_write", json!({ "path": "memory_summary.md", "content": "v1
- partial
" })),
        tool_call("p2", "mem_write", json!({ "path": "extra.md", "content": "kept
" })),
    ]);
    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);
    let agent = move |_b: &Value| {
        if c.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(tc.clone())
        } else {
            Ok(final_reply("done but incomplete"))
        }
    };
    assert_eq!(run(&e, &agent), Phase2Outcome::Failed("failed_invalid_artifacts"));
    // 不回滚：部分产物保留
    assert_eq!(
        std::fs::read_to_string(e.root.join("memory_summary.md")).unwrap(),
        "v1\n- partial\n"
    );
    assert_eq!(std::fs::read_to_string(e.root.join("extra.md")).unwrap(), "kept\n");
    assert_eq!(commit_count(&e.root), 1, "校验失败不 reset");
    assert_retry_accounting(&e, "invalid-artifacts", T0);
}

/// failed_workspace_status：diff 步骤失败（.git 去写权限→add 不可写索引）
/// → 领取后失败记账（非静默）。
#[test]
fn diff_failure_marks_failed_workspace_status() {
    let e = env("diff-fail");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (_, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    e.clock.advance(6 * HOUR);
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(e.root.join(".git"), std::fs::Permissions::from_mode(0o500))
            .unwrap();
    }
    let (_, agent2) = good_agent(&e);
    assert_eq!(run(&e, &agent2), Phase2Outcome::Failed("failed_workspace_status"));
    assert_retry_accounting(&e, "diff-fail", T0 + 6 * HOUR);
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(e.root.join(".git"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
}

/// 合并产物 containment：成功轮后记忆根外零文件（全树遍历断言）。
#[test]
fn merge_outputs_confined_to_memory_root() {
    let e = env("confine");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let (_, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    let mut outside = Vec::new();
    let mut stack = vec![e.tmp.path().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if !path.starts_with(&e.root) {
                outside.push(path);
            }
        }
    }
    assert!(outside.is_empty(), "记忆根外不得有产物：{outside:?}");
}

/// 合并进度（M5 输出面）：成功后 `consolidation_progress.max_thread_count`
/// ＝本轮 selected 行数（由既有 mark_global_phase2_job_succeeded 写入）。
#[test]
fn consolidation_progress_tracks_selected_count() {
    let e = env("progress");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    seed_row(&e, "t2", T0 - 200, "raw", "summary");
    let (_, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    let conn = rusqlite::Connection::open(memories_db_path(&e.root)).unwrap();
    let count: i64 = conn
        .query_row("SELECT max_thread_count FROM consolidation_progress", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2, "本轮 selected 两行");
}

/// 工具定义恰三项（注册表面）。
#[test]
fn tool_definitions_exactly_three_mem_tools() {
    let defs = mem_tools::tool_definitions();
    let names: Vec<&str> = defs
        .iter()
        .map(|d| d["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["mem_read", "mem_write", "mem_list"]);
    for d in &defs {
        assert_eq!(d["type"], json!("function"));
        assert!(d["function"]["parameters"]["properties"].is_object());
        assert_eq!(d["function"]["parameters"]["additionalProperties"], json!(false));
    }
}

/// 合并轮沿用单批 32 上限（33 条整批拒绝、零副作用、历史不污染）。
#[test]
fn merge_turn_batch_limit_shared() {
    let e = env("merge-batch");
    let calls: Vec<Value> = (0..33)
        .map(|i| tool_call(&format!("b{i:03}"), "mem_list", json!({})))
        .collect();
    let tc = assistant_with_calls(calls);
    let mut history = Vec::new();
    let root = e.root.clone();
    let system = CONSOLIDATION_SYSTEM;
    let err = run_tool_turn_with_mode(
        &mut history,
        "merge",
        &mut |_| Ok(tc.clone()),
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::MemoryMerge { root: &root, system, expected_epoch: 0 },
    &ProcessManager::new(),
    )
    .unwrap_err();
    assert!(err.contains("超过上限 32"), "{err}");
    assert_eq!(history.len(), 1, "仅本轮 user，无调用记录污染（N3 入史前拒绝）");
}

/// 合并参数默认值（上游 256/30）与配置钳制（Phase2Params::from_config）。
#[test]
fn phase2_params_defaults_and_clamp() {
    use terminal_chat::memory::config::MemoriesConfig;
    let p = Phase2Params::from_config(&MemoriesConfig::default());
    assert_eq!(p.max_raw_memories, 256, "上游默认 256");
    assert_eq!(p.max_unused_days, 30, "上游默认 30");

    let cfg = MemoriesConfig {
        max_raw_memories_for_consolidation: 0,
        max_unused_days: 100_000,
        ..MemoriesConfig::default()
    };
    let p = Phase2Params::from_config(&cfg);
    assert_eq!(p.max_raw_memories, 1, "钳到下限 1");
    assert_eq!(p.max_unused_days, 365, "钳到上限 365");
}

/// 取消传播：合并代理首请求期间置取消标志 → 工具批中断 → failed_agent
/// 记账（与上游 Interrupted 同路）。
#[test]
fn cancel_during_merge_marks_failed_agent() {
    let e = env("cancel-agent");
    seed_row(&e, "t1", T0 - 100, "raw", "summary");
    let cancel = AtomicBool::new(false);
    let first = assistant_with_calls(vec![tool_call(
        "w1",
        "mem_write",
        json!({ "path": "MEMORY.md", "content": "# M\n" }),
    )]);
    let agent = |_b: &Value| {
        cancel.store(true, Ordering::SeqCst); // 首请求后即取消
        Ok(first.clone())
    };
    let resolver = synthetic_meta(&e);
    assert_eq!(
        run_phase2_consolidation(
            &e.store,
            &e.root,
            "w-cancel",
            params(64),
            &resolver,
            &agent,
            Some(&cancel)
        ),
        Phase2Outcome::Failed("failed_agent")
    );
    let row = job_row(&e);
    assert_eq!(row.status, "error");
    assert!(row.last_error.unwrap().contains("failed_agent"));
}

/// 空选择：首轮仍合并（同步产物构成首轮差异），第二轮短路零模型调用。
#[test]
fn empty_selection_first_merge_then_short_circuit() {
    let e = env("empty-sel");
    let (calls, agent) = good_agent(&e);
    assert!(matches!(run(&e, &agent), Phase2Outcome::Succeeded { sql_committed: true }));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "空选择首轮照常合并（同步产物入差异）");
    assert!(e.root.join("raw_memories.md").exists());
    e.clock.advance(6 * HOUR);
    let (calls2, agent2) = good_agent(&e);
    assert_eq!(
        run(&e, &agent2),
        Phase2Outcome::SucceededNoWorkspaceChanges { sql_committed: true }
    );
    assert_eq!(calls2.load(Ordering::SeqCst), 0);
}

/// 030 S15 守护组合：extras.tool_search=true 且 MemoryMerge——即使误传
/// 开关也不声明、不执行（turn 层双层隔离），伪造 tool_search 沿未知工具
/// 通用错误契约拒绝。
#[test]
fn merge_mode_ignores_tool_search_flag() {
    use terminal_chat::turn::{
        run_user_turn_with_context_and_caps, TurnExtras,
    };
    let e = env("ts30-merge");
    let calls = json!([
        tool_call("x1", "tool_search", json!({ "query": "email" })),
        tool_call("x2", "mem_write", json!({ "path": "MEMORY.md", "content": "# M\n" })),
    ]);
    let tc = assistant_with_calls(calls.as_array().unwrap().clone());
    let fin = final_reply("done");
    let mut history = Vec::new();
    let mut sent = 0usize;
    let root = e.root.clone();
    let system = CONSOLIDATION_SYSTEM;
    let sources = terminal_chat::context::ContextSources::from_env();
    let reply = run_user_turn_with_context_and_caps(
        &mut history,
        "merge",
        &mut |_| {
            sent += 1;
            if sent == 1 { Ok(tc.clone()) } else { Ok(fin.clone()) }
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::MemoryMerge { root: &root, system, expected_epoch: 0 },
        &sources,
        &ProcessManager::new(),
        None,
        None,
        false,
        None,
        TurnExtras {
            mcp: None,
            send_message: false,
            tool_search: true,
            steer: None,
            context: None,
        },
    )
    .unwrap();
    assert_eq!(reply, "done");
    let x1 = history
        .iter()
        .find_map(|h| match h {
            terminal_chat::prompt::HistoryItem::ToolResult { tool_call_id, content, .. }
                if tool_call_id == "x1" =>
            {
                Some(serde_json::from_str::<Value>(content).unwrap())
            }
            _ => None,
        })
        .expect("缺少 x1 结果");
    assert!(
        x1.get("error").and_then(Value::as_str).unwrap_or("").contains("未知工具"),
        "合并轮伪造搜索按未知工具拒绝：{x1}"
    );
}
