//! 013-M9 lifecycle: deletion must reach the actual phase2 tools and Git baseline.
#[path = "common/memory_v1.rs"]
mod support;
use serde_json::json;
use std::cell::Cell;
use std::fs;
use support::{answer, artifacts, git, tools, Env, DAY, T0};
use terminal_chat::memory::{run_phase2_consolidation, Phase2Outcome, Phase2Params};

#[test]
fn resource_deletion_reaches_merge_before_baseline_reset() {
    let e = Env::new("resource-propagation");
    let resources = e.root.join("extensions/test/resources");
    fs::create_dir_all(&resources).unwrap();
    fs::write(
        e.root.join("extensions/test/instructions.md"),
        "test extension",
    )
    .unwrap();
    let resource = resources.join("2023-11-15T22-13-20-expiring.md");
    fs::write(&resource, "RESOURCE-ORIGIN-M9\n").unwrap();
    let params = Phase2Params {
        max_raw_memories: 256,
        max_unused_days: 30,
    };
    let calls = Cell::new(0);
    let first = run_phase2_consolidation(
        &e.store,
        &e.root,
        "m9",
        params,
        &|_| None,
        &|_| {
            let i = calls.get();
            calls.set(i + 1);
            Ok(if i == 0 {
                artifacts("manual\n", "v1\nresource present\n")
            } else {
                answer("done")
            })
        },
        None,
    );
    assert!(matches!(
        first,
        Phase2Outcome::Succeeded {
            sql_committed: true
        }
    ));
    let head = git(&e.root, &["rev-parse", "HEAD"]);
    assert!(resource.exists());
    e.set_time(T0 + 15 * DAY);
    calls.set(0);

    let second = run_phase2_consolidation(
        &e.store,
        &e.root,
        "m9",
        params,
        &|_| None,
        &|body| {
            let i = calls.get();
            calls.set(i + 1);
            Ok(match i {
                0 => tools(&[("mem_read", json!({"path":"phase2_workspace_diff.md"}))]),
                1 => {
                    let sent = body.to_string();
                    assert!(
                        sent.contains("-RESOURCE-ORIGIN-M9") && sent.contains("expiring.md"),
                        "actual tool result must contain the resource deletion: {sent}"
                    );
                    artifacts("manual\n", "v1\nresource expired\n")
                }
                _ => answer("done"),
            })
        },
        None,
    );

    assert!(
        !resource.exists(),
        "resource must expire at the exact fourteen-day cutoff"
    );
    assert!(matches!(
        second,
        Phase2Outcome::Succeeded {
            sql_committed: true
        }
    ));
    assert_eq!(calls.get(), 3);
    assert_ne!(git(&e.root, &["rev-parse", "HEAD"]), head);
    assert!(git(
        &e.root,
        &["diff", "HEAD", "--", "extensions/test/resources"]
    )
    .is_empty());
    assert!(!e.root.join("phase2_workspace_diff.md").exists());
    assert_eq!(
        fs::read_to_string(e.root.join("memory_summary.md")).unwrap(),
        "v1\nresource expired\n"
    );
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct EnvVars(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvVars {
    fn isolated(e: &Env) -> Self {
        let rg_config = e.dir.join("rg-config");
        fs::write(&rg_config, "--replace\nBROKEN-RG-CONFIG-M9\n").unwrap();
        let values = [
            ("RIPGREP_CONFIG_PATH", rg_config.into_os_string()),
            ("HOME", e.dir.join("home").into_os_string()),
            ("TCHAT_USER_DIR", e.dir.clone().into_os_string()),
            ("TCHAT_CONFIG_DIR", e.dir.join("config").into_os_string()),
            ("TCHAT_SESSION_TS", "20231107T221320".into()),
            ("TCHAT_SESSION_PID", "4101".into()),
            ("TZ", "UTC".into()),
        ];
        let old = values
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (k, v) in values {
            std::env::set_var(k, v);
        }
        Self(old)
    }
}
impl Drop for EnvVars {
    fn drop(&mut self) {
        for (k, v) in &self.0 {
            if let Some(v) = v {
                std::env::set_var(k, v)
            } else {
                std::env::remove_var(k)
            }
        }
    }
}

fn source(
    e: &Env,
    reg: &mut terminal_chat::memory::runner::ThreadRegistrar,
    pid: &str,
    content: &str,
) -> (terminal_chat::session::Session, String) {
    use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole};
    std::env::set_var("TCHAT_SESSION_PID", pid);
    let mut session = terminal_chat::session::Session::new(e.sessions.clone());
    let history = vec![HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: content.into(),
    })];
    session.on_history(&history);
    assert_eq!(session.durable_history_len(), 1);
    reg.note_persisted(&e.dir, &enabled(), &session);
    let id = session.current_target_stem().unwrap();
    assert_eq!(
        e.store.thread_updated_at_ms(&id).unwrap(),
        Some(e.now() * 1000)
    );
    (session, id)
}
fn enabled() -> terminal_chat::memory::MemoriesConfig {
    terminal_chat::memory::MemoriesConfig {
        enabled: true,
        max_rollout_age_days: 90,
        ..Default::default()
    }
}
fn pipeline(e: &Env, current: &str, script: &support::Script) {
    terminal_chat::memory::runner::run_memory_pipeline_once(
        &e.root,
        &e.sessions,
        &enabled(),
        current,
        &terminal_chat::catalog::ModelInfo::default(),
        &|b| script.send(b),
        e.clock(),
    );
    script.finished();
}
fn usage(e: &Env, id: &str) -> (Option<i64>, Option<i64>) {
    rusqlite::Connection::open(terminal_chat::memory::memories_db_path(&e.root))
        .unwrap()
        .query_row(
            "SELECT usage_count,last_usage FROM stage1_outputs WHERE thread_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}
fn citation(id: &str) -> String {
    format!("SHARED-M9\n<oai-mem-citation>\n<rollout_ids>\n{id}\n{id}\n</rollout_ids>\n</oai-mem-citation>")
}

fn assert_projection(e: &Env, expected_raw: &str, expected_summaries: &[(&str, &str)]) {
    // JSON string encoding preserves the raw file's exact trailing blank line.
    let expected_raw: String = serde_json::from_str(expected_raw).unwrap();
    let normalize = |text: String| {
        text.replace(e.sessions.to_str().unwrap(), "<SESSIONS>")
            .replace(std::env::current_dir().unwrap().to_str().unwrap(), "<CWD>")
    };
    assert_eq!(
        normalize(fs::read_to_string(e.root.join("raw_memories.md")).unwrap()),
        expected_raw
    );
    let mut names: Vec<_> = fs::read_dir(e.root.join("rollout_summaries"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_owned())
        .collect();
    names.sort();
    let mut expected_names: Vec<_> = expected_summaries.iter().map(|(name, _)| *name).collect();
    expected_names.sort();
    assert_eq!(names, expected_names);
    for (name, expected) in expected_summaries {
        assert_eq!(
            normalize(fs::read_to_string(e.root.join("rollout_summaries").join(name)).unwrap()),
            *expected,
            "projection: {name}"
        );
    }
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn isolated_rg(e: &Env) -> String {
    let rg = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("rg"))
        .find(|path| path.is_file())
        .expect("real rg required");
    let home = shell_quote(e.dir.join("home").to_str().unwrap());
    format!("env -i HOME={home} XDG_CONFIG_HOME={home} XDG_CACHE_HOME={home} LC_ALL=C TZ=UTC {} --no-config --line-number --fixed-strings --color=never -- 'SHARED-M9' {}", shell_quote(rg.to_str().unwrap()), shell_quote(e.root.join("MEMORY.md").to_str().unwrap()))
}

#[test]
fn same_chain_registration_to_retention_remerge_and_injection() {
    use support::{Script, Step};
    use terminal_chat::memory::{
        feedback::after_user_turn_with_clock, runner::ThreadRegistrar, workspace,
    };
    use terminal_chat::turn::{run_tool_turn_with_mode, ToolMode};
    let _env_lock = ENV_LOCK.lock().unwrap();
    let e = Env::new("same-chain");
    let _vars = EnvVars::isolated(&e);
    let mut registrar = ThreadRegistrar::with_clock(e.clock());
    e.set_time(T0 - 7 * DAY - 1);
    let (mut a, id_a) = source(
        &e,
        &mut registrar,
        "4101",
        "SESSION-A-M9: SHARED-M9 and A-ONLY-M9",
    );
    e.set_time(T0 - 7 * DAY);
    let (_b, id_b) = source(
        &e,
        &mut registrar,
        "4102",
        "SESSION-B-M9: SHARED-M9 and B-ONLY-M9",
    );
    e.set_time(T0 - DAY);
    let (mut current, id_c) = source(
        &e,
        &mut registrar,
        "4103",
        "CURRENT-M9 must not be extracted",
    );
    let original_a = fs::read(e.sessions.join(format!("{id_a}.jsonl"))).unwrap();
    let original_b = fs::read(e.sessions.join(format!("{id_b}.jsonl"))).unwrap();
    let resources = e.root.join("extensions/source/resources");
    fs::create_dir_all(&resources).unwrap();
    fs::write(
        e.root.join("extensions/source/instructions.md"),
        "source resource",
    )
    .unwrap();
    let resource = resources.join("2023-11-15T22-13-20-source.md");
    fs::write(&resource, "RESOURCE-A-M9\n").unwrap();
    e.set_time(T0);
    let initial_manual = include_str!("fixtures/memory_lifecycle/initial_MEMORY.md");
    let initial_summary = include_str!("fixtures/memory_lifecycle/initial_summary.md");
    let retained_manual = include_str!("fixtures/memory_lifecycle/retained_MEMORY.md");
    let retained_summary = include_str!("fixtures/memory_lifecycle/retained_summary.md");
    let first = Script::new(vec![
        Step::request(
            &["SESSION-B-M9", &id_b],
            answer(
                r#"{"raw_memory":"RAW-B-M9 SHARED-M9 B-ONLY-M9","rollout_summary":"SUMMARY-B-M9","rollout_slug":"m9-b"}"#,
            ),
        ),
        Step::request(
            &["SESSION-A-M9", &id_a],
            answer(
                r#"{"raw_memory":"RAW-A-M9 SHARED-M9 A-ONLY-M9","rollout_summary":"SUMMARY-A-M9","rollout_slug":"m9-a"}"#,
            ),
        ),
        Step::request(
            &["phase2_workspace_diff.md"],
            tools(&[
                ("mem_read", json!({"path":"raw_memories.md"})),
                ("mem_read", json!({"path":"phase2_workspace_diff.md"})),
            ]),
        ),
        Step::tool_result(
            &["RAW-A-M9", "RAW-B-M9", &id_a, &id_b],
            artifacts(initial_manual, initial_summary),
        ),
        Step::tool_result(&["true"], answer("merged from A and B")),
    ]);
    pipeline(&e, &id_c, &first);
    assert_eq!(first.requests.borrow().len(), 5);
    assert_projection(
        &e,
        include_str!("fixtures/memory_lifecycle/initial_raw.json"),
        &[
            (
                "2023-11-07T22-13-19-cpp9-m9_a.md",
                include_str!("fixtures/memory_lifecycle/initial_a.md"),
            ),
            (
                "2023-11-07T22-13-20-cppa-m9_b.md",
                include_str!("fixtures/memory_lifecycle/b.md"),
            ),
        ],
    );
    assert!(e.store.stage1_output(&id_c).unwrap().is_none());
    for (id, source_time, raw) in [
        (&id_a, T0 - 7 * DAY - 1, "RAW-A-M9"),
        (&id_b, T0 - 7 * DAY, "RAW-B-M9"),
    ] {
        let row = e.store.stage1_output(id).unwrap().unwrap();
        assert_eq!(row.source_updated_at, source_time);
        assert!(row.raw_memory.contains(raw));
        assert!(row.selected_for_phase2);
        assert_eq!(row.selected_for_phase2_source_updated_at, Some(source_time));
        let job = e.store.job_row("memory_stage1", id).unwrap().unwrap();
        assert_eq!(job.status, "done");
        assert_eq!(job.last_success_watermark, Some(source_time));
    }
    assert_eq!(
        fs::read_to_string(e.root.join("MEMORY.md")).unwrap(),
        initial_manual
    );
    assert_eq!(
        fs::read_to_string(e.root.join("memory_summary.md")).unwrap(),
        initial_summary
    );
    assert_eq!(git(&e.root, &["rev-list", "--count", "HEAD"]).trim(), "1");
    assert!(!workspace::memory_workspace_diff(&e.root, None)
        .unwrap()
        .has_changes());
    let first_head = git(&e.root, &["rev-parse", "HEAD"]);
    let mut history =
        terminal_chat::session::load_session_file(&e.sessions.join(format!("{id_c}.jsonl")))
            .unwrap()
            .history;
    e.set_time(T0 + 22 * DAY);
    let user = Script::new(vec![
        Step::request(
            &["<memory-context>", "A-ONLY-M9", "B-ONLY-M9"],
            tools(&[(
                "exec_command",
                json!({"cmd":isolated_rg(&e),"workdir":e.dir,"timeout_ms":5000}),
            )]),
        ),
        Step::tool_result(&["SHARED-M9", &id_b], answer(&citation(&id_b))),
    ]);
    let reply = run_tool_turn_with_mode(
        &mut history,
        "Use the shared experience",
        &mut |b| user.send(b),
        &mut |_| {},
        None,
        &mut |h| {
            current.on_history(h);
            registrar.note_persisted(&e.dir, &enabled(), &current);
        },
        ToolMode::UserWithMemory {
            root: &e.root,
            config: &enabled(),
        },
    )
    .unwrap();
    user.finished();
    {
        let requests = user.requests.borrow();
        assert_eq!(requests.len(), 2);
        for body in requests.iter() {
            let messages = body["messages"].as_array().unwrap();
            assert_eq!(messages.iter().filter(|m| m["role"] == "system").count(), 1);
            assert_eq!(messages[0]["role"], "system");
            let system = messages[0]["content"].as_str().unwrap();
            assert_eq!(system.matches("<memory-context>").count(), 1);
            assert!(system.contains(initial_summary.trim()));
        }
        assert_eq!(
            requests[0]["messages"][0], requests[1]["messages"][0],
            "same round uses the same injected snapshot"
        );
    }
    assert_eq!(
        usage(&e, &id_b),
        (None, None),
        "tool reading must not count as citation feedback"
    );
    after_user_turn_with_clock(&e.dir, &enabled(), &reply, e.clock());
    assert_eq!(usage(&e, &id_b), (Some(1), Some(T0 + 22 * DAY)));
    assert_eq!(usage(&e, &id_a), (None, None));
    assert!(
        !fs::read_to_string(e.sessions.join(format!("{id_c}.jsonl")))
            .unwrap()
            .contains("<memory-context>")
    );
    e.set_time(T0 + 24 * DAY);
    assert!(
        e.store
            .stage1_output(&id_a)
            .unwrap()
            .unwrap()
            .selected_for_phase2
    );
    let selected = e.store.get_phase2_input_selection(256, 30).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|r| r.thread_id.as_str())
            .collect::<Vec<_>>(),
        vec![id_b.as_str()]
    );
    let next = Script::new(vec![
        Step::request(
            &["phase2_workspace_diff.md"],
            tools(&[("mem_read", json!({"path":"phase2_workspace_diff.md"}))]),
        ),
        Step::tool_result(
            &["-RAW-A-M9", "-RESOURCE-A-M9", &id_a],
            artifacts(retained_manual, retained_summary),
        ),
        Step::tool_result(&["true"], answer("keep B-supported shared knowledge")),
    ]);
    pipeline(&e, &id_c, &next);
    let old = e
        .store
        .stage1_output(&id_a)
        .unwrap()
        .expect("last baseline selection protects A during startup prune");
    assert!(
        !old.selected_for_phase2,
        "real phase2 success removes A from the selected snapshot"
    );
    assert!(
        e.store
            .stage1_output(&id_b)
            .unwrap()
            .unwrap()
            .selected_for_phase2
    );
    assert!(!resource.exists());
    assert_projection(
        &e,
        include_str!("fixtures/memory_lifecycle/retained_raw.json"),
        &[(
            "2023-11-07T22-13-20-cppa-m9_b.md",
            include_str!("fixtures/memory_lifecycle/b.md"),
        )],
    );
    let raw = fs::read_to_string(e.root.join("raw_memories.md")).unwrap();
    assert!(!raw.contains("RAW-A-M9"));
    assert!(raw.contains("RAW-B-M9"));
    let summaries: Vec<_> = fs::read_dir(e.root.join("rollout_summaries"))
        .unwrap()
        .map(|p| fs::read_to_string(p.unwrap().path()).unwrap())
        .collect();
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].contains(&id_b));
    assert!(!summaries[0].contains(&id_a));
    assert_eq!(
        fs::read_to_string(e.root.join("MEMORY.md")).unwrap(),
        retained_manual
    );
    assert_eq!(
        fs::read_to_string(e.root.join("memory_summary.md")).unwrap(),
        retained_summary
    );
    assert_ne!(git(&e.root, &["rev-parse", "HEAD"]), first_head);
    assert_eq!(git(&e.root, &["rev-list", "--count", "HEAD"]).trim(), "1");
    assert!(!workspace::memory_workspace_diff(&e.root, None)
        .unwrap()
        .has_changes());
    let final_user = |b: &serde_json::Value| {
        let system = b["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains(retained_summary.trim()));
        assert!(!system.contains("A-ONLY-M9"));
        Ok(answer("SHARED-M9 still has source B"))
    };
    run_tool_turn_with_mode(
        &mut history,
        "What remains?",
        &mut |b| final_user(b),
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::UserWithMemory {
            root: &e.root,
            config: &enabled(),
        },
    )
    .unwrap();
    // Keep the age window open: the job watermark, not age or idle rejection, prevents re-extraction.
    let scan = e.store.stage1_startup_scan(&id_c, 90, 6, 5000).unwrap();
    assert!(scan.iter().any(|(id, _)| id == &id_a));
    let old_job = e.store.job_row("memory_stage1", &id_a).unwrap();
    e.set_time(T0 + 24 * DAY + 1);
    pipeline(&e, &id_c, &Script::new(vec![]));
    assert!(
        e.store.stage1_output(&id_a).unwrap().is_none(),
        "unselected stale A must be physically pruned on the next startup"
    );
    assert_eq!(e.store.job_row("memory_stage1", &id_a).unwrap(), old_job);
    assert_eq!(
        fs::read(e.sessions.join(format!("{id_a}.jsonl"))).unwrap(),
        original_a
    );
    assert_eq!(
        fs::read(e.sessions.join(format!("{id_b}.jsonl"))).unwrap(),
        original_b
    );
    // A real append creates a newer source version after cleanup; the old watermark
    // must prevent duplicate work without suppressing this legitimate re-extraction.
    e.set_time(T0 + 24 * DAY + 2);
    let mut updated =
        terminal_chat::session::load_session_file(&e.sessions.join(format!("{id_a}.jsonl")))
            .unwrap()
            .history;
    updated.push(terminal_chat::prompt::HistoryItem::Text(
        terminal_chat::prompt::HistoryMessage {
            role: terminal_chat::prompt::HistoryRole::User,
            content: "SESSION-A-V2-M9: NEW-A-V2-M9 supersedes the old A experience".into(),
        },
    ));
    a.on_history(&updated);
    registrar.note_persisted(&e.dir, &enabled(), &a);
    let new_source = e.now();
    assert_eq!(
        e.store.thread_updated_at_ms(&id_a).unwrap(),
        Some(new_source * 1000)
    );
    e.set_time(new_source + 6 * 3600);
    let renewed_manual = include_str!("fixtures/memory_lifecycle/renewed_MEMORY.md");
    let renewed_summary = include_str!("fixtures/memory_lifecycle/renewed_summary.md");
    let renewed = Script::new(vec![
        Step::request(
            &["SESSION-A-V2-M9", &id_a],
            answer(
                r#"{"raw_memory":"RAW-A-V2-M9 NEW-A-V2-M9 SHARED-M9","rollout_summary":"SUMMARY-A-V2-M9","rollout_slug":"m9-a-v2"}"#,
            ),
        ),
        Step::request(
            &["phase2_workspace_diff.md"],
            tools(&[("mem_read", json!({"path":"raw_memories.md"}))]),
        ),
        Step::tool_result(
            &["RAW-A-V2-M9", "RAW-B-M9"],
            artifacts(renewed_manual, renewed_summary),
        ),
        Step::tool_result(&["true"], answer("new version merged")),
    ]);
    pipeline(&e, &id_c, &renewed);
    assert_projection(
        &e,
        include_str!("fixtures/memory_lifecycle/renewed_raw.json"),
        &[
            (
                "2023-12-08T22-13-22-cpp9-m9_a_v2.md",
                include_str!("fixtures/memory_lifecycle/renewed_a.md"),
            ),
            (
                "2023-11-07T22-13-20-cppa-m9_b.md",
                include_str!("fixtures/memory_lifecycle/b.md"),
            ),
        ],
    );
    let new_row = e.store.stage1_output(&id_a).unwrap().unwrap();
    assert_eq!(new_row.source_updated_at, new_source);
    assert_eq!(
        new_row.selected_for_phase2_source_updated_at,
        Some(new_source)
    );
    assert!(new_row.raw_memory.contains("RAW-A-V2-M9"));
    assert_eq!(
        e.store
            .job_row("memory_stage1", &id_a)
            .unwrap()
            .unwrap()
            .last_success_watermark,
        Some(new_source)
    );
    assert_eq!(
        fs::read_to_string(e.root.join("MEMORY.md")).unwrap(),
        renewed_manual
    );
    assert_eq!(
        fs::read_to_string(e.root.join("memory_summary.md")).unwrap(),
        renewed_summary
    );
    assert_eq!(
        fs::read(e.sessions.join(format!("{id_b}.jsonl"))).unwrap(),
        original_b
    );
    println!("M9 same-chain observed: registration/candidates, strict extraction, Markdown/Git, injection/rg, citation, selected protection, expiry/remerge, next-batch prune with watermark; sources={id_a},{id_b}");
}

#[test]
fn pipeline_prunes_one_batch_even_while_phase2_is_running() {
    use terminal_chat::memory::{runner::run_memory_pipeline_once, Phase2JobClaimOutcome};
    let e = Env::new("startup-batch");
    for n in (0..201).rev() {
        e.seed(&format!("auto-run-{n:03}"), T0 - 31 * DAY);
    }
    assert!(matches!(
        e.store
            .try_claim_global_phase2_job("other-worker", 3600)
            .unwrap(),
        Phase2JobClaimOutcome::Claimed { .. }
    ));
    let running = e
        .store
        .job_row("memory_consolidate_global", "global")
        .unwrap();
    let old_resource = e
        .root
        .join("extensions/test/resources/2023-10-31T22-13-19-old.md");
    fs::create_dir_all(old_resource.parent().unwrap()).unwrap();
    fs::write(&old_resource, "not pruned before phase2 claim").unwrap();
    fs::write(
        old_resource
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("instructions.md"),
        "exists",
    )
    .unwrap();
    let no_chat = |_: &serde_json::Value| -> Result<serde_json::Value, String> {
        panic!("running global task must block merge model")
    };
    run_memory_pipeline_once(
        &e.root,
        &e.sessions,
        &enabled(),
        "current",
        &Default::default(),
        &no_chat,
        e.clock(),
    );
    assert_eq!(
        e.ids(),
        ["auto-run-200"],
        "one startup must prune exactly one 200-row batch"
    );
    assert!(
        old_resource.exists(),
        "resource pruning runs only after a successful phase2 claim"
    );
    assert!(!e.root.join(".git").exists());
    assert_eq!(
        e.store
            .job_row("memory_consolidate_global", "global")
            .unwrap(),
        running
    );
    run_memory_pipeline_once(
        &e.root,
        &e.sessions,
        &enabled(),
        "current",
        &Default::default(),
        &no_chat,
        e.clock(),
    );
    assert!(e.ids().is_empty());
    assert_eq!(e.store.count_jobs_by_status().unwrap(), (1, 201, 0));
}

#[test]
fn prune_failure_is_best_effort_and_recovers_next_startup() {
    use support::{Script, Step};
    let e = Env::new("prune-failure");
    e.seed("auto-stale", T0 - 31 * DAY);
    let sql = rusqlite::Connection::open(terminal_chat::memory::memories_db_path(&e.root)).unwrap();
    sql.execute_batch("CREATE TRIGGER fail_prune BEFORE DELETE ON stage1_outputs BEGIN SELECT RAISE(ABORT, 'injected prune failure'); END;").unwrap();
    let job = e.store.job_row("memory_stage1", "auto-stale").unwrap();
    let script = Script::new(vec![
        Step::request(
            &["phase2_workspace_diff.md"],
            artifacts("manual\n", "v1\nbest effort\n"),
        ),
        Step::tool_result(&["true"], answer("done")),
    ]);
    pipeline(&e, "current", &script);
    assert!(e.store.stage1_output("auto-stale").unwrap().is_some());
    assert_eq!(
        fs::read_to_string(e.root.join("memory_summary.md")).unwrap(),
        "v1\nbest effort\n"
    );
    sql.execute_batch("DROP TRIGGER fail_prune;").unwrap();
    pipeline(&e, "current", &Script::new(vec![]));
    assert!(
        e.ids().is_empty(),
        "following startup retries cleanup without losing job watermark"
    );
    assert_eq!(e.store.job_row("memory_stage1", "auto-stale").unwrap(), job);
}

#[test]
fn reset_between_selection_and_sync_preserves_new_epoch_resource() {
    use std::sync::mpsc;
    use std::time::Duration;
    use terminal_chat::memory::{runner, MemoryStore, ThreadFileMeta};
    let e = Env::new("reset-before-sync");
    e.seed("auto-reset-source", T0 - DAY);
    fs::write(
        e.sessions.join("auto-reset-source.jsonl"),
        "original session\n",
    )
    .unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let root = e.root.clone();
    let sessions = e.sessions.clone();
    let clock = e.clock();
    let worker = std::thread::spawn(move || {
        let store = MemoryStore::open(&root, clock).unwrap();
        run_phase2_consolidation(
            &store,
            &root,
            "reset",
            Phase2Params {
                max_raw_memories: 256,
                max_unused_days: 30,
            },
            &|id| {
                ready_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                Some(ThreadFileMeta {
                    rollout_path: sessions.join(format!("{id}.jsonl")),
                    cwd: sessions.clone(),
                    git_branch: None,
                })
            },
            &|_| panic!("invalidated phase2 must stop before model"),
            None,
        )
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let reset = runner::memory_reset(&e.dir, "confirm");
    assert!(!reset.contains("错误"), "{reset}");
    let fresh = e
        .root
        .join("extensions/fresh/resources/2023-10-31T22-13-19-fresh.md");
    fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    fs::write(&fresh, "NEW-EPOCH-RESOURCE").unwrap();
    fs::write(
        fresh
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("instructions.md"),
        "exists",
    )
    .unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap(),
        Phase2Outcome::Failed("failed_reset_invalidated")
    );
    assert_eq!(fs::read_to_string(&fresh).unwrap(), "NEW-EPOCH-RESOURCE");
    assert!(!e.root.join(".git").exists());
    assert!(!e.root.join("memories.sqlite").exists());
    assert_eq!(
        fs::read_to_string(e.sessions.join("auto-reset-source.jsonl")).unwrap(),
        "original session\n"
    );
}

#[test]
fn reset_during_model_wait_does_not_hold_file_lock_or_rebuild_old_outputs() {
    use std::sync::mpsc;
    use std::time::Duration;
    use terminal_chat::memory::{runner, MemoryStore};
    let e = Env::new("reset-model-wait");
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let root = e.root.clone();
    let clock = e.clock();
    let calls = Cell::new(0);
    let worker = std::thread::spawn(move || {
        let store = MemoryStore::open(&root, clock).unwrap();
        run_phase2_consolidation(
            &store,
            &root,
            "reset",
            Phase2Params {
                max_raw_memories: 256,
                max_unused_days: 30,
            },
            &|_| None,
            &|_| {
                let i = calls.get();
                calls.set(i + 1);
                if i == 0 {
                    ready_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                    Ok(artifacts("OLD-MUST-NOT-WRITE\n", "v1\nold\n"))
                } else {
                    Ok(answer("done"))
                }
            },
            None,
        )
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let (reset_tx, reset_rx) = mpsc::channel();
    let dir = e.dir.clone();
    let resetter = std::thread::spawn(move || {
        reset_tx
            .send(runner::memory_reset(&dir, "confirm"))
            .unwrap();
    });
    let reset = reset_rx.recv_timeout(Duration::from_secs(3));
    if reset.is_err() {
        release_tx.send(()).unwrap();
        let _ = worker.join();
        let _ = resetter.join();
        panic!("model request held the file modification lock");
    }
    assert!(!reset.unwrap().contains("错误"));
    resetter.join().unwrap();
    let fresh = e
        .root
        .join("extensions/fresh/resources/2023-10-31T22-13-19-fresh.md");
    fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    fs::write(&fresh, "NEW-EPOCH-WHILE-WAITING").unwrap();
    fs::write(
        fresh
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("instructions.md"),
        "exists",
    )
    .unwrap();
    release_tx.send(()).unwrap();
    let outcome = worker.join().unwrap();
    assert!(matches!(outcome, Phase2Outcome::Failed(_)), "{outcome:?}");
    assert_eq!(
        fs::read_to_string(fresh).unwrap(),
        "NEW-EPOCH-WHILE-WAITING"
    );
    for name in ["MEMORY.md", "memory_summary.md", ".git", "memories.sqlite"] {
        assert!(!e.root.join(name).exists(), "old task rebuilt {name}");
    }
}
