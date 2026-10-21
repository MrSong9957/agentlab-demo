//! Narrow conversion helpers for approval-related app-server payloads.
//!
//! The TUI mostly keeps app-server approval types intact. These helpers cover
//! the remaining cases where the UI consumes a private file-change display
//! model or needs to translate a granted permission response for outbound
//! submission.

use crate::diff_model::FileChange;
use codex_app_server_protocol::FileUpdateChange;
use codex_app_server_protocol::PatchChangeKind;
use std::collections::HashMap;
use std::path::PathBuf;

// 008-TRIM: granted_permission_profile_from_request 等审批转换（审批无本地事件源，008 §1）

pub(crate) fn file_update_changes_to_display(
    changes: Vec<FileUpdateChange>,
) -> HashMap<PathBuf, FileChange> {
    changes
        .into_iter()
        .map(|mut change| {
            let path = PathBuf::from(change.path);
            let file_change = match change.kind {
                PatchChangeKind::Add => FileChange::Add {
                    content: change.diff,
                },
                PatchChangeKind::Delete => FileChange::Delete {
                    content: change.diff,
                },
                PatchChangeKind::Update { move_path } => {
                    if let Some(path) = &move_path
                        && let Some(diff) = change
                            .diff
                            .strip_suffix(&format!("\n\nMoved to: {}", path.display()))
                    {
                        change.diff.truncate(diff.len());
                    }
                    FileChange::Update {
                        unified_diff: change.diff,
                        move_path,
                    }
                }
            };
            (path, file_change)
        })
        .collect()
}

// 008-TRIM: mod tests（覆盖被裁审批/权限转换的测试，008 §1）
