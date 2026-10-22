//! 013-M8 引用反馈集成：真实 turn 引擎（UserWithMemory）＋真实 SQLite
//! （临时记忆根）＋mock sender。断言状态与副作用，不只测解析返回值。

use serde_json::{json, Value};
use terminal_chat::proc_session::ProcessManager;
use terminal_chat::memory::config::MemoriesConfig;
use terminal_chat::memory::feedback::after_user_turn;
use terminal_chat::memory::store::{memories_db_path, MemoryStore};
use terminal_chat::memory::Stage1JobClaimOutcome;
use terminal_chat::turn::{run_tool_turn_with_mode, ToolMode};

struct Env {
    user_dir: std::path::PathBuf,
    root: std::path::PathBuf,
    store: MemoryStore,
}

impl Env {
    fn new(tag: &str) -> Self {
        let user_dir = common_temp(tag);
        let root = user_dir.join("memories");
        let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
        Self { user_dir, root, store }
    }

    fn seed(&self, thread: &str, source: i64) {
        self.store
            .register_thread(thread, "enabled", source * 1000)
            .unwrap();
        let Stage1JobClaimOutcome::Claimed { ownership_token } = self
            .store
            .try_claim_stage1_job(thread, "seed", source, 3600, 8)
            .unwrap()
        else {
            panic!("种子行应可领取：{thread}")
        };
        self.store
            .mark_stage1_job_succeeded(thread, &ownership_token, source, "raw", "summary", None)
            .unwrap();
    }

    fn usage(&self, thread: &str) -> (Option<i64>, Option<i64>) {
        let conn = rusqlite::Connection::open(memories_db_path(&self.root)).unwrap();
        conn.query_row(
            "SELECT usage_count, last_usage FROM stage1_outputs WHERE thread_id = ?1",
            rusqlite::params![thread],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.user_dir);
    }
}

fn common_temp(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tchat-m8-feedback-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn answer(content: &str) -> Value {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] })
}

fn tool_reply(id: &str, name: &str, args: Value) -> Value {
    json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": [{ "id": id, "type": "function", "function": {
            "name": name, "arguments": args.to_string()
        } }]
    } }] })
}

fn citation_reply(id: &str) -> String {
    format!(
        "答案。\n<oai-mem-citation>\n<citation_entries>\nMEMORY.md:1-2|note=[used]\n</citation_entries>\n<rollout_ids>\n{id}\n</rollout_ids>\n</oai-mem-citation>"
    )
}

fn enabled_config() -> MemoriesConfig {
    MemoriesConfig { enabled: true, ..Default::default() }
}

/// 真实 turn（含一次真实 exec 读取记忆文件）→ 完成位点反馈 → SQLite
/// usage_count/last_usage 实际变化。
#[test]
fn m8_turn_with_citation_records_usage_in_sqlite() {
    let e = Env::new("turn-cite");
    let id = "auto-20261019T101112-4321";
    e.seed(id, 1_699_974_800);
    std::fs::write(e.root.join("MEMORY.md"), "M8-TURN-MEMORY-CONTENT\n").unwrap();
    let config = enabled_config();
    let mut history = Vec::new();
    let step = std::cell::Cell::new(0usize);

    let reply = run_tool_turn_with_mode(
        &mut history,
        "用了记忆的问题",
        &mut |_body| {
            step.set(step.get() + 1);
            if step.get() == 1 {
                // 首请求：让模型真实读取记忆文件（工具层真执行 cat）
                Ok(tool_reply(
                    "r1",
                    "exec_command",
                    json!({ "cmd": format!("cat {}", e.root.join("MEMORY.md").display()) }),
                ))
            } else {
                Ok(answer(&citation_reply(id)))
            }
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::UserWithMemory { root: &e.root, config: &config },
    &ProcessManager::new(),
    )
    .unwrap();

    assert!(reply.contains("oai-mem-citation"), "最终回复携带引用块");
    // 工具轮真的读过记忆文件（真实副作用证据——工具结果入史）
    assert!(
        history
            .iter()
            .any(|h| serde_json::to_string(h).unwrap().contains("M8-TURN-MEMORY-CONTENT")),
        "exec 真实读取过 MEMORY.md"
    );
    assert_eq!(e.usage(id), (None, None), "反馈接线前不计数（驱动负责完成位点）");

    // 驱动位点同款调用（main.rs/tui_host.rs 在 Ok(reply) 后执行）
    after_user_turn(&e.user_dir, &config, &reply);
    let (count, last) = e.usage(id);
    assert_eq!(count, Some(1), "引用命中已有 stage1 输出＝计数 +1");
    let last = last.expect("last_usage 已写入");
    assert!(last >= 1_700_000_000, "last_usage＝写入时刻秒级时间戳：{last}");
}

/// A28：真实工具打开记忆文件但回复无引用 → 不计数（读取分类只是观测
/// 语义，从不进入使用计数）。
#[test]
fn m8_tool_read_without_citation_does_not_count() {
    let e = Env::new("tool-no-count");
    let id = "auto-20261019T101112-4321";
    e.seed(id, 1_699_974_800);
    std::fs::write(e.root.join("MEMORY.md"), "M8-READ-ONLY-CONTENT\n").unwrap();
    let config = enabled_config();
    let mut history = Vec::new();
    let step = std::cell::Cell::new(0usize);

    let reply = run_tool_turn_with_mode(
        &mut history,
        "又一个问题",
        &mut |_body| {
            step.set(step.get() + 1);
            if step.get() == 1 {
                Ok(tool_reply(
                    "r1",
                    "exec_command",
                    json!({ "cmd": format!("cat {}", e.root.join("MEMORY.md").display()) }),
                ))
            } else {
                Ok(answer("答案但无引用块"))
            }
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::UserWithMemory { root: &e.root, config: &config },
    &ProcessManager::new(),
    )
    .unwrap();

    assert_eq!(reply, "答案但无引用块");
    assert!(
        history
            .iter()
            .any(|h| serde_json::to_string(h).unwrap().contains("M8-READ-ONLY-CONTENT")),
        "工具确实打开过记忆文件"
    );
    after_user_turn(&e.user_dir, &config, &reply);
    assert_eq!(e.usage(id), (None, None), "工具读取不增加 usage_count（A28）");
}

/// 同一回复重复 ID 只计一次；非法形状与缺行 ID 不产生变化；关闭态零副作用。
#[test]
fn m8_reply_id_dedup_invalid_and_disabled() {
    let e = Env::new("dedup");
    let id = "auto-20261019T101112-4321";
    e.seed(id, 1_699_974_800);
    let config = enabled_config();

    let reply = format!(
        "回复。\n<oai-mem-citation>\n<rollout_ids>\n{id}\n{id}\nnot-a-session\nauto-20261019T999999-9999\n</rollout_ids>\n</oai-mem-citation>"
    );
    after_user_turn(&e.user_dir, &config, &reply);
    assert_eq!(e.usage(id).0, Some(1), "重复 ID 去重后恰 +1；缺行 ID 忽略");

    // 关闭态：不触碰库（连计数行都不产生）
    let e2 = Env::new("disabled");
    e2.seed(id, 1_699_974_800);
    let disabled = MemoriesConfig { enabled: false, ..Default::default() };
    after_user_turn(&e2.user_dir, &disabled, &citation_reply(id));
    assert_eq!(e2.usage(id), (None, None), "关闭态零副作用");
}

/// D12-A 批准边界②（守卫型断言）：mem_list/mem_read 旁路保留——非法便条
/// 在差异面被排除的同时，仍可经受限工具**发现与读取**（提示词指引不构成
/// 技术隔离）；隐藏文件排除合同不受影响。同一临时根内先以真实 G8 差异
/// 证明确被过滤，再证旁路，两面互为对照。
#[test]
fn m8_mem_tools_bypass_still_discovers_and_reads_invalid_notes() {
    let e = Env::new("bypass");
    let notes = e.root.join("extensions/ad_hoc/notes");
    std::fs::create_dir_all(&notes).unwrap();
    let invalid_name = "bad name!!.md"; // 差异过滤按 D12-A 排除的非法名
    std::fs::write(notes.join(invalid_name), "BYPASS-INVALID-CONTENT\n").unwrap();
    std::fs::write(notes.join(".hidden.md"), "hidden\n").unwrap();
    terminal_chat::memory::workspace::prepare_memory_workspace(&e.root, None).unwrap();

    // 对照面：同一非法便条被差异过滤排除（状态列表与正文都不含）
    let d = terminal_chat::memory::workspace::memory_workspace_diff(&e.root, None).unwrap();
    assert!(
        !d.changes.iter().any(|c| c.path.contains(invalid_name)),
        "前置：非法便条被差异过滤排除：{:?}",
        d.changes
    );
    assert!(!d.unified_diff.contains("BYPASS-INVALID-CONTENT"));

    // 旁路面：mem_list 仍能发现（含非法名；隐藏文件仍按合同排除）
    let listed = terminal_chat::memory::mem_tools::execute(
        "mem_list",
        &serde_json::json!({ "path": "extensions/ad_hoc/notes" }).to_string(),
        &e.root,
        0,
    );
    assert!(
        listed.contains(invalid_name),
        "mem_list 仍发现非法便条（批准边界②）：{listed}"
    );
    assert!(!listed.contains(".hidden.md"), "隐藏文件排除合同不变：{listed}");

    // 旁路面：mem_read 仍能读取非法便条正文
    let read = terminal_chat::memory::mem_tools::execute(
        "mem_read",
        &serde_json::json!({ "path": format!("extensions/ad_hoc/notes/{invalid_name}") })
            .to_string(),
        &e.root,
        0,
    );
    assert!(
        read.contains("BYPASS-INVALID-CONTENT"),
        "mem_read 仍读取非法便条（提示词不构成技术隔离）：{read}"
    );
}

// —— 014 §1：反馈不隐式建库（先解析；无引用零 IO；无库跳过；reset 后不重建）——

/// 014 新用例的临时目录守卫：断言失败也保证清理（同 Env 的 Drop 语义，
/// 但不预建库——本组用例需要「干净初始态」）。
struct CleanDir(std::path::PathBuf);
impl Drop for CleanDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn clean_temp(tag: &str) -> CleanDir {
    CleanDir(common_temp(tag))
}

/// 无有效引用：不得创建记忆根或数据库（零 IO 早退）。
#[test]
fn m14_feedback_without_citation_creates_nothing() {
    let user_dir = common_temp("m14-nocite");
    let config = enabled_config();
    after_user_turn(&user_dir, &config, "普通回复，没有任何引用块");
    let root = user_dir.join("memories");
    assert!(!root.is_dir(), "无引用不得创建记忆根");
    assert!(
        !root.join("memories.sqlite").is_file(),
        "无引用不得创建数据库"
    );
    let _ = std::fs::remove_dir_all(&user_dir);
}

/// 有合法引用但库不存在：跳过反馈（不建库、不建目录、不迁移），调用静默返回。
#[test]
fn m14_feedback_valid_citation_without_db_skips_silently() {
    let user_dir = common_temp("m14-nodb");
    let config = enabled_config();
    // 预置记忆根但无库——区分「根不存在」与「库不存在」两个分支
    std::fs::create_dir_all(user_dir.join("memories")).unwrap();
    after_user_turn(
        &user_dir,
        &config,
        &citation_reply("auto-20261020T101112-1111"),
    );
    assert!(
        !user_dir.join("memories/memories.sqlite").is_file(),
        "有引用但无库：必须跳过，不得建库"
    );
    let _ = std::fs::remove_dir_all(&user_dir);
}

/// 有效引用遇到 reset 已删除的数据库：不重建库，用户轮正常结束（best-effort）。
#[test]
fn m14_feedback_after_reset_with_valid_citation_does_not_recreate_db() {
    let user_dir = clean_temp("m14-reset");
    let root = user_dir.0.join("memories");
    let id = "auto-20261020T101112-2222";
    {
        let store = MemoryStore::open(&root, terminal_chat::memory::store::system_clock()).unwrap();
        store
            .register_thread(id, "enabled", 1_699_974_800_000)
            .unwrap();
    }
    let msg = terminal_chat::memory::runner::memory_reset(&user_dir.0, "confirm");
    assert!(msg.contains("已清理"), "前置：reset 成功：{msg}");
    assert!(!memories_db_path(&root).is_file(), "前置：库已被删除");
    let config = enabled_config();
    after_user_turn(&user_dir.0, &config, &citation_reply(id));
    assert!(
        !memories_db_path(&root).is_file(),
        "reset 后有效引用不得重建数据库"
    );
}

/// 014 收口：符号链接记忆根——既有 `open` 路径经 `init_memories_root` 拒绝
/// （M1 契约 `m1_open_rejects_symlink_memory_root`）；`open_existing` 不得
/// 绕过该拒绝：直接调用返回符号链接错误，反馈路径静默跳过、目标库零写入。
#[cfg(unix)]
#[test]
fn m14_feedback_rejects_symlink_memory_root() {
    let user_dir = clean_temp("m14-symlink");
    let target = user_dir.0.join("real-root");
    let id = "auto-20261020T101112-3333";
    {
        let store =
            MemoryStore::open(&target, terminal_chat::memory::store::system_clock()).unwrap();
        store
            .register_thread(id, "enabled", 1_699_974_800_000)
            .unwrap();
        let Stage1JobClaimOutcome::Claimed { ownership_token } = store
            .try_claim_stage1_job(id, "seed", 1_699_974_800, 3600, 8)
            .unwrap()
        else {
            panic!("种子领取失败");
        };
        store
            .mark_stage1_job_succeeded(id, &ownership_token, 1_699_974_800, "raw", "summary", None)
            .unwrap();
    }
    std::os::unix::fs::symlink(&target, user_dir.0.join("memories")).unwrap();

    // 直接入口：Err 且为符号链接拒绝（与 init_memories_root 同款文案）
    let res = MemoryStore::open_existing(
        &user_dir.0.join("memories"),
        terminal_chat::memory::store::system_clock(),
    );
    assert!(
        matches!(res, Err(ref e) if e.contains("符号链接")),
        "符号链接根必须拒绝（返回 Err 且含拒绝文案）"
    );

    // 反馈路径：静默跳过，符号链接目标后的真实库零写入
    after_user_turn(&user_dir.0, &enabled_config(), &citation_reply(id));
    let used: Option<i64> = {
        let conn = rusqlite::Connection::open(memories_db_path(&target)).unwrap();
        conn.query_row(
            "SELECT usage_count FROM stage1_outputs WHERE thread_id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(used, None, "符号链接根下的反馈不得写入目标库");
}
