//! 013-M4：真实 SQLite 快照 → Markdown 文件副作用；模型仅在 M3 边界注入固定响应。
//! 上游语义锚：openai/codex@ce0323f2f2d3b25e82806cd555237da3db4e1897
//! codex-rs/memories/write/src/{storage.rs,storage_tests.rs,lib.rs,workspace.rs} (Apache-2.0)。
//! 全部路径显式传入临时目录，业务时钟固定；不访问真实配置、会话或记忆根。

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use terminal_chat::catalog::ModelInfo;
use terminal_chat::memory::{
    memories_db_path, run_stage1_job, sync_stage1_snapshot, MemoryStore, Stage1FileInput,
    Stage1JobClaim, Stage1JobClaimOutcome, Stage1JobOutcome,
};
use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole};

const A: &str = "1970-01-01T00-01-40-001z-alpha___topic.md";
const B: &str = "1970-01-01T00-03-20-001A.md";
const NEW_A: &str = "1970-01-01T00-01-41-001z-new.md";
const GOLD: &str = "fixtures/memory_storage_files";

// 复用项目 TempDir 的 RAII 清理；目录名只用 pid＋计数器，不读取时钟或随机源。
fn temp_dir(tag: &str) -> common::TempDir {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    loop {
        let count = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("tchat-m4-{}-{tag}-{count}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return common::TempDir(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("创建测试临时目录失败：{error}"),
        }
    }
}

struct Env {
    tmp: common::TempDir,
    root: PathBuf,
    store: MemoryStore,
}

impl Env {
    fn new(tag: &str) -> Self {
        let tmp = temp_dir(tag);
        let root = tmp.path().join("memories");
        let store = MemoryStore::open(&root, Arc::new(|| 1_700_000_000)).unwrap();
        Self { tmp, root, store }
    }

    fn claim(&self, id: &str, source: i64) -> String {
        match self.store.try_claim_stage1_job(id, "m4-worker", source, 3600, 8).unwrap() {
            Stage1JobClaimOutcome::Claimed { ownership_token } => ownership_token,
            other => panic!("无法准备来源 {id}: {other:?}"),
        }
    }

    fn seed(&self, id: &str, source: i64, raw: &str, summary: &str, slug: Option<&str>) {
        let token = self.claim(id, source);
        assert!(self.store.mark_stage1_job_succeeded(id, &token, source, raw, summary, slug).unwrap());
    }

    fn initial(&self) {
        // 故意反序入库；全量快照必须恢复 thread_id 升序。
        self.seed("b", 200, "beta raw", "beta summary", None);
        self.seed("a", 100, " \nalpha raw\nnext\n ", "alpha summary\nline two", Some("Alpha / Topic"));
    }

    fn empty(&self, id: &str, source: i64) {
        let token = self.claim(id, source);
        assert!(self.store.mark_stage1_job_succeeded_no_output(id, &token).unwrap());
    }

    fn inputs(&self) -> Vec<Stage1FileInput> {
        self.store.list_stage1_outputs().unwrap().into_iter().map(|row| {
            let path = PathBuf::from(format!("/fixture/rollouts/{}.jsonl", row.thread_id));
            Stage1FileInput::from_row(row, path, PathBuf::from("/fixture/project"), None).unwrap()
        }).collect()
    }

    fn sync(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        sync_stage1_snapshot(&self.root, &self.inputs(), &mut |w| warnings.push(w.to_owned())).unwrap();
        warnings
    }

    fn db_bytes(&self) -> Vec<u8> {
        std::fs::read(memories_db_path(&self.root)).unwrap()
    }
}

fn gold(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join(GOLD).join(name)).unwrap()
}

/// 比较完整文件集合与字节，数据库单独断言；链接用例在清理后才调用。
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, output: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_str().unwrap().replace('\\', "/");
            if path == memories_db_path(root) { continue; }
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink(), "意外残留链接：{relative}");
            if kind.is_dir() { walk(root, &path, output); }
            else { output.insert(relative, std::fs::read(path).unwrap()); }
        }
    }
    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}

fn initial_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("raw_memories.md".into(), gold("initial_raw.md")),
        (format!("rollout_summaries/{A}"), gold("initial_a.md")),
        (format!("rollout_summaries/{B}"), gold("initial_b.md")),
    ])
}

#[test]
fn m4_snapshot_is_full_sorted_and_read_only() {
    let e = Env::new("snapshot");
    e.initial();
    let before = e.db_bytes();
    let rows = e.store.list_stage1_outputs().unwrap();
    assert_eq!(rows.iter().map(|r| r.thread_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(rows[0], e.store.stage1_output("a").unwrap().unwrap());
    assert_eq!(rows[1], e.store.stage1_output("b").unwrap().unwrap());
    assert_eq!(e.db_bytes(), before);
}

#[test]
fn m4_a20_add_matches_golden_files() {
    let e = Env::new("add");
    e.initial();
    let db = e.db_bytes();
    assert!(e.sync().is_empty());
    assert_eq!(files(&e.root), initial_files());
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_a20_modify_replaces_bytes_and_prunes_old_canonical_name() {
    let e = Env::new("modify");
    e.initial(); e.sync();
    e.seed("a", 101, "updated raw", "updated summary", Some("new"));
    let db = e.db_bytes();
    assert!(e.sync().is_empty());
    assert_eq!(files(&e.root), BTreeMap::from([
        ("raw_memories.md".into(), gold("updated_raw.md")),
        (format!("rollout_summaries/{NEW_A}"), gold("updated_a.md")),
        (format!("rollout_summaries/{B}"), gold("initial_b.md")),
    ]));
    assert!(!e.root.join("rollout_summaries").join(A).exists());
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_a20_reduce_consumes_no_output_deletion_without_changing_watermark() {
    let e = Env::new("reduce");
    e.initial(); e.sync();
    e.empty("a", 101);
    let job = e.store.job_row("memory_stage1", "a").unwrap().unwrap();
    assert_eq!(job.last_success_watermark, Some(101));
    let db = e.db_bytes();
    e.sync();
    assert_eq!(files(&e.root), BTreeMap::from([
        ("raw_memories.md".into(), gold("remaining_raw.md")),
        (format!("rollout_summaries/{B}"), gold("initial_b.md")),
    ]));
    assert_eq!(e.store.job_row("memory_stage1", "a").unwrap().unwrap(), job);
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_a20_zero_writes_exact_placeholder_and_removes_all_summaries() {
    let e = Env::new("zero");
    e.initial(); e.sync();
    e.empty("a", 101); e.empty("b", 201);
    let db = e.db_bytes();
    e.sync();
    assert_eq!(files(&e.root), BTreeMap::from([("raw_memories.md".into(), gold("empty_raw.md"))]));
    assert_eq!(std::fs::read_dir(e.root.join("rollout_summaries")).unwrap().count(), 0);
    assert!(e.store.list_stage1_outputs().unwrap().is_empty());
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_identical_snapshot_has_identical_file_set_and_bytes() {
    let e = Env::new("idempotent");
    e.initial(); e.sync();
    let before = files(&e.root);
    let db = e.db_bytes();
    assert!(e.sync().is_empty());
    assert_eq!(files(&e.root), before);
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_prune_uses_keep_set_direct_level_and_lowercase_md_only() {
    let e = Env::new("boundary");
    e.initial(); e.sync();
    let summaries = e.root.join("rollout_summaries");
    for name in ["old.md", "a.md", ".hidden.md", "old.MD", "old.md.bak", "plain"] {
        std::fs::write(summaries.join(name), name).unwrap();
    }
    std::fs::create_dir(summaries.join("nested")).unwrap();
    std::fs::write(summaries.join("nested/keep.md"), "nested").unwrap();
    std::fs::write(summaries.join(A), "manual edits in generated file").unwrap();
    for (path, text) in [("MEMORY.md", "shared"), ("memory_summary.md", "summary"),
        ("skills/x/SKILL.md", "skill"), ("extensions/ad_hoc/notes/n.md", "note"),
        (".git/config", "baseline"), ("phase2_workspace_diff.md", "future diff")] {
        let path = e.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let before = files(&e.root);
    assert!(e.sync().is_empty());
    let mut expected = before;
    for name in ["old.md", "a.md", ".hidden.md"] { expected.remove(&format!("rollout_summaries/{name}")); }
    expected.insert(format!("rollout_summaries/{A}"), gold("initial_a.md"));
    assert_eq!(files(&e.root), expected);
}

#[test]
fn m4_summary_write_failure_keeps_actual_partial_changes_and_database() {
    let e = Env::new("summary-fail"); e.initial();
    let summaries = e.root.join("rollout_summaries");
    std::fs::create_dir_all(summaries.join(B)).unwrap(); // keep 集合内目录：写文件确定失败。
    std::fs::write(summaries.join("old.md"), "stale").unwrap();
    std::fs::write(e.root.join("raw_memories.md"), "old raw").unwrap();
    let db = e.db_bytes();
    let mut warnings = Vec::new();
    let error = sync_stage1_snapshot(&e.root, &e.inputs(), &mut |w| warnings.push(w.to_owned())).unwrap_err();
    assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(warnings.is_empty());
    assert_eq!(std::fs::read(summaries.join(A)).unwrap(), gold("initial_a.md"));
    assert!(!summaries.join("old.md").exists());
    assert!(summaries.join(B).is_dir());
    assert_eq!(std::fs::read(e.root.join("raw_memories.md")).unwrap(), b"old raw");
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_raw_write_failure_does_not_roll_back_summary_writes() {
    let e = Env::new("raw-fail"); e.initial();
    std::fs::create_dir(e.root.join("raw_memories.md")).unwrap();
    let db = e.db_bytes();
    assert!(sync_stage1_snapshot(&e.root, &e.inputs(), &mut |_| {}).is_err());
    assert!(e.root.join("raw_memories.md").is_dir());
    assert_eq!(files(&e.root), BTreeMap::from([
        (format!("rollout_summaries/{A}"), gold("initial_a.md")),
        (format!("rollout_summaries/{B}"), gold("initial_b.md")),
    ]));
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_prune_failure_warns_and_continues_to_write_outputs() {
    let e = Env::new("prune-fail"); e.initial();
    let summaries = e.root.join("rollout_summaries");
    std::fs::create_dir_all(summaries.join("blocked.md")).unwrap();
    std::fs::write(summaries.join("blocked.md/child"), "untouched").unwrap();
    std::fs::write(summaries.join("remove.md"), "stale").unwrap();
    let db = e.db_bytes();
    let warnings = e.sync();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("failed pruning outdated rollout summary"));
    assert!(warnings[0].contains("blocked.md"));
    let mut expected = initial_files();
    expected.insert("rollout_summaries/blocked.md/child".into(), b"untouched".to_vec());
    assert_eq!(files(&e.root), expected);
    assert_eq!(e.db_bytes(), db);
}

#[cfg(unix)]
#[test]
fn m4_layout_removes_links_across_root_without_following_targets() {
    use std::os::unix::fs::symlink;
    let e = Env::new("links"); e.initial();
    let outside = e.tmp.path().join("outside");
    std::fs::create_dir_all(outside.join("nested")).unwrap();
    std::fs::write(outside.join("target"), "outside file").unwrap();
    std::fs::write(outside.join("nested/stale.md"), "outside summary").unwrap();
    let before = files(&outside);
    std::fs::create_dir_all(e.root.join("skills/x")).unwrap();
    std::fs::create_dir_all(e.root.join(".git")).unwrap();
    for path in ["raw_memories.md", "skills/x/inside", ".git/link", "dangling"] {
        let target = if path == "dangling" { outside.join("absent") } else { outside.join("target") };
        symlink(target, e.root.join(path)).unwrap();
    }
    symlink(outside.join("nested"), e.root.join("rollout_summaries")).unwrap();
    let warnings = e.sync();
    assert_eq!(warnings.len(), 5);
    assert_eq!(files(&outside), before);
    assert_eq!(files(&e.root), initial_files());
    assert!(!e.root.join("skills/x/inside").exists());
    assert!(!e.root.join(".git/link").exists());
    assert!(std::fs::symlink_metadata(e.root.join("dangling")).is_err());
}

#[cfg(unix)]
#[test]
fn m4_root_symlink_is_rejected_and_left_intact() {
    use std::os::unix::fs::symlink;
    let tmp = temp_dir("root-link");
    let target = tmp.path().join("target"); std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("keep.md"), "outside").unwrap();
    let before = files(&target);
    let root = tmp.path().join("memories"); symlink(&target, &root).unwrap();
    assert!(sync_stage1_snapshot(&root, &[], &mut |_| {}).is_err());
    assert!(std::fs::symlink_metadata(root).unwrap().file_type().is_symlink());
    assert_eq!(files(&target), before);
}

#[test]
fn m4_filename_matches_upstream_uuid_timestamp_hash_and_slug_rules() {
    let e = Env::new("naming");
    let id = "0194f5a6-89ab-7cde-8123-456789abcdef";
    e.seed(id, 123, "raw", "summary", None);
    let row = e.store.stage1_output(id).unwrap().unwrap();
    for (slug, expected) in [(None, "2025-02-11T15-35-19-jqmb.md".to_owned()),
        (Some("".to_owned()), "2025-02-11T15-35-19-jqmb.md".to_owned()),
        (Some("///中文___".to_owned()), "2025-02-11T15-35-19-jqmb.md".to_owned()),
        (Some("A".repeat(80)), format!("2025-02-11T15-35-19-jqmb-{}.md", "a".repeat(60))),
        (Some("../Unsafe / Path".to_owned()), "2025-02-11T15-35-19-jqmb-___unsafe___path.md".to_owned())] {
        let mut row = row.clone(); row.rollout_slug = slug;
        let input = Stage1FileInput::from_row(row, "/fixture/r.jsonl".into(), "/fixture".into(), Some("main".into())).unwrap();
        sync_stage1_snapshot(&e.root, &[input], &mut |_| {}).unwrap();
        let names: Vec<_> = std::fs::read_dir(e.root.join("rollout_summaries")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names, [expected]);
        let summary = std::fs::read_to_string(e.root.join("rollout_summaries").join(&names[0])).unwrap();
        assert_eq!(summary, format!("thread_id: {id}\nupdated_at: 1970-01-01T00:02:03+00:00\nrollout_path: /fixture/r.jsonl\ncwd: /fixture\ngit_branch: main\n\nsummary\n"));
    }
}

#[test]
fn m4_out_of_range_source_timestamp_is_rejected_before_file_projection() {
    let e = Env::new("timestamp"); e.seed("a", 100, "raw", "summary", None);
    let mut row = e.store.stage1_output("a").unwrap().unwrap(); row.source_updated_at = i64::MAX;
    assert!(Stage1FileInput::from_row(row, "/fixture/r".into(), "/fixture".into(), None).is_err());
    assert!(files(&e.root).is_empty());
}

#[test]
fn m4_a13_m3_parse_store_export_redacts_body_and_exact_slug_projection() {
    let e = Env::new("a13");
    let secret = "sk-ABCDEFGHIJKLMNOPQRSTUVwx123";
    let sessions = e.tmp.path().join("sessions"); std::fs::create_dir(&sessions).unwrap();
    let path = sessions.join("a.jsonl");
    let item = HistoryItem::Text(HistoryMessage { role: HistoryRole::User, content: "remember the task".into() });
    std::fs::write(&path, format!("{{\"v\":1}}\n{}\n", serde_json::to_string(&item).unwrap())).unwrap();
    let session_before = std::fs::read(&path).unwrap();
    let token = e.claim("a", 100);
    let claim = Stage1JobClaim { thread_id: "a".into(), session_path: path.clone(), updated_at_ms: 100_000,
        source_updated_at: 100, ownership_token: token };
    let output = serde_json::json!({"raw_memory":format!("raw {secret}"), "rollout_summary":format!("summary {secret}"), "rollout_slug":secret}).to_string();
    let calls = std::cell::Cell::new(0);
    let outcome = run_stage1_job(&e.store, &claim, &ModelInfo { id: "fixture-model".into(), ..ModelInfo::default() }, "/fixture/project", &|_| {
        calls.set(calls.get() + 1);
        Ok(serde_json::json!({"choices":[{"message":{"content":output},"finish_reason":"stop"}]}))
    });
    assert_eq!(outcome, Stage1JobOutcome::SucceededWithOutput);
    assert_eq!(calls.get(), 1, "真实 M3 采样路径必须经过 mock 模型边界");
    let row = e.store.list_stage1_outputs().unwrap().pop().unwrap();
    assert_eq!(row.raw_memory, "raw [REDACTED_SECRET]");
    assert_eq!(row.rollout_summary, "summary [REDACTED_SECRET]");
    assert_eq!(row.rollout_slug.as_deref(), Some("[REDACTED_SECRET]"));
    let input = Stage1FileInput::from_row(row, path.clone(), "/fixture/project".into(), None).unwrap();
    let db = e.db_bytes();
    sync_stage1_snapshot(&e.root, &[input], &mut |_| {}).unwrap();
    let actual: BTreeMap<_, _> = files(&e.root).into_iter().map(|(name, bytes)| {
        assert!(!name.contains(secret));
        let content = String::from_utf8(bytes).unwrap(); assert!(!content.contains(secret));
        (name, content.replace(e.tmp.path().to_str().unwrap(), "/fixture").into_bytes())
    }).collect();
    assert_eq!(actual, BTreeMap::from([
        ("raw_memories.md".into(), gold("a13_raw.md")),
        ("rollout_summaries/1970-01-01T00-01-40-001z-_redacted_secret.md".into(), gold("a13_summary.md")),
    ]));
    assert_eq!(std::fs::read(path).unwrap(), session_before);
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_full_snapshot_has_no_256_limit_or_age_filter() {
    let e = Env::new("full");
    for index in (0..260).rev() {
        e.seed(&format!("thread-{index:03}"), 100, "raw", "summary", None);
    }
    let rows = e.store.list_stage1_outputs().unwrap();
    assert_eq!(rows.len(), 260);
    assert_eq!(rows.first().unwrap().thread_id, "thread-000");
    assert_eq!(rows.last().unwrap().thread_id, "thread-259");
    let db = e.db_bytes();
    e.sync();
    let actual = files(&e.root);
    assert_eq!(actual.len(), 261, "260 摘要＋完整 raw 合集");
    let raw = String::from_utf8(actual["raw_memories.md"].clone()).unwrap();
    assert_eq!(raw.matches("## Thread `").count(), 260);
    assert!(raw.find("## Thread `thread-000`").unwrap() < raw.find("## Thread `thread-259`").unwrap());
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_export_uses_supplied_snapshot_without_rereading_changed_database() {
    let e = Env::new("frozen"); e.initial();
    let frozen = e.inputs();
    e.seed("a", 101, "new database raw", "new summary", Some("new"));
    let db = e.db_bytes();
    sync_stage1_snapshot(&e.root, &frozen, &mut |_| {}).unwrap();
    assert_eq!(files(&e.root), initial_files());
    assert_eq!(e.db_bytes(), db);
    assert_eq!(e.store.stage1_output("a").unwrap().unwrap().source_updated_at, 101);
}

#[test]
fn m4_layout_failure_preserves_existing_regular_file_and_database() {
    let e = Env::new("layout-fail"); e.initial();
    std::fs::write(e.root.join("rollout_summaries"), "blocking regular file").unwrap();
    std::fs::write(e.root.join("raw_memories.md"), "old raw").unwrap();
    let before = files(&e.root);
    let db = e.db_bytes();
    assert!(sync_stage1_snapshot(&e.root, &e.inputs(), &mut |_| {}).is_err());
    assert_eq!(files(&e.root), before);
    assert_eq!(e.db_bytes(), db);
}

#[test]
fn m4_regular_file_root_is_rejected_without_overwriting_it() {
    let tmp = temp_dir("file-root");
    let root = tmp.path().join("memories"); std::fs::write(&root, "existing file").unwrap();
    assert!(sync_stage1_snapshot(&root, &[], &mut |_| {}).is_err());
    assert_eq!(std::fs::read(root).unwrap(), b"existing file");
}

#[test]
fn m4_negative_timestamp_keeps_utc_date_without_clamping_to_epoch() {
    let e = Env::new("negative-time"); e.seed("a", -1, "raw", "summary", None);
    e.sync();
    let path = e.root.join("rollout_summaries/1969-12-31T23-59-59-001z.md");
    assert_eq!(std::fs::read_to_string(path).unwrap(),
        "thread_id: a\nupdated_at: 1969-12-31T23:59:59+00:00\nrollout_path: /fixture/rollouts/a.jsonl\ncwd: /fixture/project\n\nsummary\n");
}

#[cfg(unix)]
#[test]
fn m4_security_private_permissions_are_created_and_existing_modes_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new("permissions"); e.initial(); e.sync();
    for path in [&e.root, &e.root.join("rollout_summaries")] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o777)).unwrap();
    }
    for path in [e.root.join("raw_memories.md"), e.root.join("rollout_summaries").join(A)] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666)).unwrap();
    }
    e.sync();
    for path in [&e.root, &e.root.join("rollout_summaries")] {
        assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o700);
    }
    for path in [e.root.join("raw_memories.md"), e.root.join("rollout_summaries").join(A),
        e.root.join("rollout_summaries").join(B), memories_db_path(&e.root)] {
        assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(files(&e.root), initial_files());
}

#[cfg(unix)]
#[test]
fn m4_security_canonical_file_link_is_removed_and_warning_is_terminal_safe() {
    use std::os::unix::fs::symlink;
    let e = Env::new("canonical-link"); e.initial();
    let outside = e.tmp.path().join("external"); std::fs::write(&outside, "external bytes").unwrap();
    std::fs::create_dir(e.root.join("rollout_summaries")).unwrap();
    symlink(&outside, e.root.join("rollout_summaries").join(A)).unwrap();
    std::fs::create_dir(e.root.join("rollout_summaries/bad\u{1b}[31m.md")).unwrap();
    let warnings = e.sync();
    assert_eq!(std::fs::read(&outside).unwrap(), b"external bytes");
    assert_eq!(std::fs::read(e.root.join("rollout_summaries").join(A)).unwrap(), gold("initial_a.md"));
    assert_eq!(warnings.len(), 2);
    assert!(warnings.iter().all(|w| !w.contains('\u{1b}')));
    assert!(warnings.iter().any(|w| w.contains("failed pruning outdated rollout summary") && w.contains("bad.md")));
}

#[test]
fn m4_security_fresh_empty_root_is_private_and_needs_no_database() {
    let tmp = temp_dir("fresh-empty");
    let root = tmp.path().join("new/memories");
    let mut warnings = Vec::new();
    sync_stage1_snapshot(&root, &[], &mut |w| warnings.push(w.to_owned())).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(files(&root), BTreeMap::from([("raw_memories.md".into(), gold("empty_raw.md"))]));
    assert!(root.join("rollout_summaries").is_dir());
    assert!(!memories_db_path(&root).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(root.join("raw_memories.md")).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
