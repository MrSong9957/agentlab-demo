//! 013-M9 retention: real SQL/file effects, controlled time and no user data.
#[path = "common/memory_v1.rs"]
mod support;
use support::{Env, DAY, T0};

#[test]
fn retention_cutoff_usage_and_selected_protection() {
    let e = Env::new("cutoff");
    let cutoff = T0 - 30 * DAY;
    for (id, source) in [
        ("auto-old", cutoff - 1),
        ("auto-exact", cutoff),
        ("auto-next", cutoff + 1),
        ("auto-renewed", cutoff - 10),
        ("auto-last-old", T0 - 1),
        ("auto-selected", cutoff - 20),
    ] {
        e.seed(id, source);
    }
    e.store
        .record_stage1_output_usage(&["auto-renewed".into()])
        .unwrap();
    e.set_time(cutoff - 1);
    e.store
        .record_stage1_output_usage(&["auto-last-old".into()])
        .unwrap();
    e.set_time(T0);
    e.select(&["auto-selected"]);
    let jobs: Vec<_> = e
        .ids()
        .iter()
        .map(|id| e.store.job_row("memory_stage1", id).unwrap())
        .collect();

    assert_eq!(
        e.store.prune_stage1_outputs_for_retention(30, 200).unwrap(),
        2
    );

    assert_eq!(
        e.ids(),
        ["auto-exact", "auto-next", "auto-renewed", "auto-selected"]
    );
    for (id, before) in [
        "auto-exact",
        "auto-last-old",
        "auto-next",
        "auto-old",
        "auto-renewed",
        "auto-selected",
    ]
    .iter()
    .zip(jobs)
    {
        assert_eq!(
            e.store.job_row("memory_stage1", id).unwrap(),
            before,
            "job and success watermark preserved"
        );
    }
}

#[test]
fn retention_batch_order_watermarks_and_new_version() {
    use terminal_chat::memory::Stage1JobClaimOutcome;
    let e = Env::new("batch");
    // Insert in reverse ID order; tied recency must still prune thread_id ASC.
    for n in (0..202).rev() {
        e.seed(&format!("auto-batch-{n:03}"), T0 - 31 * DAY);
    }
    std::fs::write(
        e.sessions.join("auto-batch-000.jsonl"),
        "original session bytes\n",
    )
    .unwrap();
    let original = std::fs::read(e.sessions.join("auto-batch-000.jsonl")).unwrap();
    let before = e
        .store
        .job_row("memory_stage1", "auto-batch-000")
        .unwrap()
        .unwrap();
    assert_eq!(
        e.store.prune_stage1_outputs_for_retention(30, 0).unwrap(),
        0
    );
    assert_eq!(e.ids().len(), 202);
    assert_eq!(
        e.store.prune_stage1_outputs_for_retention(30, 200).unwrap(),
        200
    );
    assert_eq!(e.ids(), ["auto-batch-200", "auto-batch-201"]);
    assert_eq!(
        e.store.prune_stage1_outputs_for_retention(30, 200).unwrap(),
        2
    );
    assert!(e.ids().is_empty());
    assert_eq!(
        e.store
            .job_row("memory_stage1", "auto-batch-000")
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        e.store
            .try_claim_stage1_job("auto-batch-000", "retry", T0 - 31 * DAY, 3600, 8)
            .unwrap(),
        Stage1JobClaimOutcome::SkippedUpToDate
    );
    let newer = T0 - 7 * DAY;
    let Stage1JobClaimOutcome::Claimed { ownership_token } = e
        .store
        .try_claim_stage1_job("auto-batch-000", "new", newer, 3600, 8)
        .unwrap()
    else {
        panic!("new version must still be processed")
    };
    assert!(e
        .store
        .mark_stage1_job_succeeded(
            "auto-batch-000",
            &ownership_token,
            newer,
            "new raw",
            "new summary",
            None
        )
        .unwrap());
    assert_eq!(
        e.store
            .stage1_output("auto-batch-000")
            .unwrap()
            .unwrap()
            .source_updated_at,
        newer
    );
    assert_eq!(
        std::fs::read(e.sessions.join("auto-batch-000.jsonl")).unwrap(),
        original
    );
}

#[test]
fn retention_zero_days_keeps_equal_and_future() {
    let e = Env::new("zero");
    e.seed("auto-past", T0 - 1);
    e.seed("auto-equal", T0);
    e.seed("auto-future", T0 + 1);
    assert_eq!(
        e.store.prune_stage1_outputs_for_retention(-1, 200).unwrap(),
        1
    );
    assert_eq!(e.ids(), ["auto-equal", "auto-future"]);
}

#[test]
fn resources_filename_boundary_instructions_and_notes() {
    use std::fs;
    use terminal_chat::memory::retention::prune_old_extension_resources;
    let e = Env::new("resources");
    let resources = e.root.join("extensions/test/resources");
    fs::create_dir_all(&resources).unwrap();
    // Upstream uses existence, not is_file: a directory named instructions.md qualifies.
    fs::create_dir(e.root.join("extensions/test/instructions.md")).unwrap();
    let old = "2023-10-31T22-13-19-old.md";
    let exact = "2023-10-31T22-13-20-exact.md";
    let recent = "2023-10-31T22-13-21-recent.md";
    for name in [
        old,
        exact,
        recent,
        "invalid.md",
        "2023-02-30T22-13-20-invalid.md",
        "2023-10-31T22-13-19-old.MD",
    ] {
        fs::write(resources.join(name), name).unwrap();
    }
    let nested = resources.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join(old), "nested").unwrap();
    let ignored = e.root.join("extensions/ignored/resources");
    fs::create_dir_all(&ignored).unwrap();
    fs::write(ignored.join(old), "no instructions").unwrap();
    let notes = e.root.join("extensions/ad_hoc/notes");
    fs::create_dir_all(&notes).unwrap();
    fs::write(notes.join(old), "user note").unwrap();
    fs::write(
        e.root.join("extensions/ad_hoc/instructions.md"),
        "instructions",
    )
    .unwrap();
    let mut warnings = Vec::new();
    prune_old_extension_resources(&e.root, T0, &mut |w| warnings.push(w.to_owned()));
    assert!(!resources.join(old).exists());
    assert!(!resources.join(exact).exists());
    for name in [
        recent,
        "invalid.md",
        "2023-02-30T22-13-20-invalid.md",
        "2023-10-31T22-13-19-old.MD",
    ] {
        assert!(resources.join(name).is_file(), "keep {name}");
    }
    assert!(nested.join(old).is_file());
    assert!(ignored.join(old).is_file());
    assert_eq!(fs::read_to_string(notes.join(old)).unwrap(), "user note");
    assert!(warnings.is_empty());
    prune_old_extension_resources(&e.dir.join("absent"), T0, &mut |_| {
        panic!("missing root is no-op")
    });
}

#[cfg(unix)]
#[test]
fn resources_do_not_follow_any_scan_directory_or_file_link() {
    use std::fs;
    use std::os::unix::fs::symlink;
    use terminal_chat::memory::retention::prune_old_extension_resources;
    let e = Env::new("links");
    let outside = e.dir.join("outside");
    fs::create_dir_all(outside.join("resources")).unwrap();
    fs::write(outside.join("instructions.md"), "exists").unwrap();
    let old = "2023-10-31T22-13-19-old.md";
    fs::write(outside.join("resources").join(old), "OUTSIDE-UNCHANGED").unwrap();
    let ext = e.root.join("extensions");
    fs::create_dir_all(ext.join("inside/resources")).unwrap();
    fs::write(ext.join("inside/instructions.md"), "exists").unwrap();
    symlink(&outside, ext.join("linked-extension")).unwrap();
    symlink(
        outside.join("resources").join(old),
        ext.join("inside/resources").join(old),
    )
    .unwrap();
    fs::create_dir(ext.join("linked-resources")).unwrap();
    fs::write(ext.join("linked-resources/instructions.md"), "exists").unwrap();
    symlink(
        outside.join("resources"),
        ext.join("linked-resources/resources"),
    )
    .unwrap();
    prune_old_extension_resources(&e.root, T0, &mut |_| {});
    assert_eq!(
        fs::read_to_string(outside.join("resources").join(old)).unwrap(),
        "OUTSIDE-UNCHANGED"
    );
    assert!(fs::symlink_metadata(ext.join("inside/resources").join(old))
        .unwrap()
        .file_type()
        .is_symlink());
    let linked_root = e.dir.join("root-link");
    symlink(&e.root, &linked_root).unwrap();
    prune_old_extension_resources(&linked_root, T0, &mut |_| {});
    let other = e.dir.join("other-root");
    fs::create_dir(&other).unwrap();
    symlink(&ext, other.join("extensions")).unwrap();
    prune_old_extension_resources(&other, T0, &mut |_| {});
    assert_eq!(
        fs::read_to_string(outside.join("resources").join(old)).unwrap(),
        "OUTSIDE-UNCHANGED"
    );
}

#[cfg(unix)]
#[test]
fn resource_delete_failure_warns_and_other_extensions_continue() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use terminal_chat::memory::retention::prune_old_extension_resources;
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "permission fault test requires non-root execution"
    );
    let e = Env::new("delete-failure");
    let old = "2023-10-31T22-13-19-old.md";
    for ext in ["blocked", "writable"] {
        let dir = e.root.join(format!("extensions/{ext}/resources"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(old), "expired").unwrap();
        fs::write(dir.parent().unwrap().join("instructions.md"), "exists").unwrap();
    }
    let blocked = e.root.join("extensions/blocked/resources");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o500)).unwrap();
    let mut warnings = Vec::new();
    prune_old_extension_resources(&e.root, T0, &mut |w| warnings.push(w.to_owned()));
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(blocked.join(old).exists());
    assert!(!e
        .root
        .join("extensions/writable/resources")
        .join(old)
        .exists());
    assert!(
        warnings.iter().any(|w| w.contains(old)),
        "failed deletion must be reported: {warnings:?}"
    );
}

#[test]
fn retention_order_uses_recency_then_source_then_id() {
    let e = Env::new("order");
    for (id, source) in [
        ("auto-unused", T0 - 40 * DAY),
        ("auto-a", T0 - 50 * DAY),
        ("auto-b", T0 - 45 * DAY),
        ("auto-c", T0 - 45 * DAY),
        ("auto-newer", T0 - 34 * DAY),
    ] {
        e.seed(id, source);
    }
    e.set_time(T0 - 35 * DAY);
    e.store
        .record_stage1_output_usage(&["auto-a".into(), "auto-b".into(), "auto-c".into()])
        .unwrap();
    e.set_time(T0);
    for expected in ["auto-unused", "auto-a", "auto-b", "auto-c", "auto-newer"] {
        assert_eq!(
            e.store.prune_stage1_outputs_for_retention(30, 1).unwrap(),
            1
        );
        assert!(
            e.store.stage1_output(expected).unwrap().is_none(),
            "oldest remaining row must be {expected}"
        );
    }
}
