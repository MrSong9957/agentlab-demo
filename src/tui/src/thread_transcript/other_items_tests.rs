//! Persisted tool projections retain rich detail, styles, and truthful outcomes.

use super::cells;
use crate::diff_model::FileChange;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::test_support::PathBufExt;
use crate::test_support::test_path_buf;
use codex_app_server_protocol::FileUpdateChange;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::PatchChangeKind;
use codex_app_server_protocol::ThreadItem;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::path::PathBuf;
// 008-TRIM: ImageGenerationItem/SubAgentActivityKind/WebSearchAction/WebSearchItem/
// LegacyAppPathString 导入（对应 ThreadItem 变体本地缺席，008 §1）

#[test]
fn completed_patch_restores_rich_diff_and_styles() {
    let cwd = test_path_buf("/workspace").abs();
    let mut changes = vec![
        FileUpdateChange {
            path: "new.rs".to_string(),
            kind: PatchChangeKind::Add,
            diff: "fn greet() {\n    println!(\"hello\");\n}\n".to_string(),
        },
        FileUpdateChange {
            path: "old.txt".to_string(),
            kind: PatchChangeKind::Delete,
            diff: "outdated\n".to_string(),
        },
        FileUpdateChange {
            path: "src/before.rs".to_string(),
            kind: PatchChangeKind::Update {
                move_path: Some(PathBuf::from("src/after.rs")),
            },
            diff: "@@ -1 +1 @@\n-let before = 1;\n+let after = 2;\n".to_string(),
        },
    ];
    let expected = history_cell::new_patch_event(
        HashMap::from([
            (
                PathBuf::from("new.rs"),
                FileChange::Add {
                    content: changes[0].diff.clone(),
                },
            ),
            (
                PathBuf::from("old.txt"),
                FileChange::Delete {
                    content: changes[1].diff.clone(),
                },
            ),
            (
                PathBuf::from("src/before.rs"),
                FileChange::Update {
                    unified_diff: changes[2].diff.clone(),
                    move_path: Some(PathBuf::from("src/after.rs")),
                },
            ),
        ]),
        cwd.as_path(),
    );
    changes[2].diff.push_str("\n\nMoved to: src/after.rs");
    let actual = cells(
        ThreadItem::FileChange {
            id: "patch-1".to_string(),
            changes,
            status: PatchApplyStatus::Completed,
        },
        &cwd,
    );

    assert_eq!(actual.len(), 1);
    assert_eq!(
        actual[0].transcript_hyperlink_lines(/*width*/ 80),
        expected.transcript_hyperlink_lines(/*width*/ 80),
    );
}

#[test]
fn unfinished_and_rejected_patches_keep_their_outcome() {
    let cwd = test_path_buf("/workspace").abs();
    let rendered = [
        PatchApplyStatus::InProgress,
        PatchApplyStatus::Declined,
        PatchApplyStatus::Failed,
    ]
    .into_iter()
    .flat_map(|status| {
        cells(
            ThreadItem::FileChange {
                id: "patch-1".to_string(),
                changes: vec![FileUpdateChange {
                    path: "main.rs".to_string(),
                    kind: PatchChangeKind::Add,
                    diff: "fn main() {}\n".to_string(),
                }],
                status,
            },
            &cwd,
        )
    })
    .flat_map(|cell| cell.display_lines(/*width*/ 80))
    .map(|line| line.to_string())
    .collect::<Vec<_>>()
    .join("\n");

    insta::assert_snapshot!(rendered, @"
    • Patch application in progress
    • Patch application declined
    ✘ Failed to apply patch
    ");
}

// 008-TRIM: fn tool_and_notice_projection_uses_normal_transcript_presentation
// （覆盖被裁 ThreadItem::WebSearch/ImageView/ImageGeneration/SubAgentActivity/
// EnteredReviewMode/ExitedReviewMode/ContextCompaction 投影（本地 ThreadItem 仅四变体），
// 008 §1）
