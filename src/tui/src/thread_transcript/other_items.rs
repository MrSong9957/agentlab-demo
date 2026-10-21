//! Restore historical tool and notice cells without changing live application state.
//!
//! Formatting stays in the same history cells used by live events. Only completed
//! patches render as applied changes; other statuses retain their actual outcome.

use super::TranscriptCells;
use crate::app_server_approval_conversions::file_update_changes_to_display;
use crate::history_cell;
use crate::history_cell::PlainHistoryCell;
use codex_app_server_protocol::PatchApplyStatus;
use codex_app_server_protocol::ThreadItem;
use codex_utils_absolute_path::AbsolutePathBuf;
use ratatui::style::Stylize as _;
use std::sync::Arc;

pub(super) fn cells(item: ThreadItem, cwd: &AbsolutePathBuf) -> TranscriptCells {
    let mut cells: TranscriptCells = Vec::new();
    match item {
        ThreadItem::FileChange {
            id,
            changes,
            status,
        } => match status {
            PatchApplyStatus::Completed if !changes.is_empty() => {
                cells.push(Arc::new(
                    history_cell::new_patch_event(
                        file_update_changes_to_display(changes),
                        cwd.as_path(),
                    )
                    .with_activity_id(id),
                ));
            }
            PatchApplyStatus::Completed => {}
            PatchApplyStatus::Failed => {
                cells.push(Arc::new(history_cell::new_patch_apply_failure(
                    String::new(),
                )));
            }
            PatchApplyStatus::InProgress | PatchApplyStatus::Declined => {
                let message = if status == PatchApplyStatus::Declined {
                    "Patch application declined"
                } else {
                    "Patch application in progress"
                };
                cells.push(Arc::new(history_cell::new_info_event(
                    message.to_string(),
                    /*hint*/ None,
                )));
            }
        },
        // 008-TRIM: WebSearch/ImageView/ImageGeneration/Collab/SubAgent/Review/Compaction/
        //  HookPrompt/DynamicToolCall 臂（无本地事件源，008 §1）
        // 008-TRIM: 缺席变体从收尾清单移除（本地 ThreadItem 仅四变体，008 §3-J5）
        ThreadItem::UserMessage { .. } | ThreadItem::AgentMessage { .. } | ThreadItem::CommandExecution { .. } => {}
    }
    cells
}

#[cfg(test)]
#[path = "other_items_tests.rs"]
mod tests;
