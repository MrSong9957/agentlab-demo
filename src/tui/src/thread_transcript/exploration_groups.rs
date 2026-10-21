//! Apply live exploration compatibility to replay and repair groups split by history pages.

use super::tools::CommandHistory;
use crate::exec_cell::ExecCell;
use crate::history_cell::HistoryCell;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::Turn;
use std::sync::Arc;

pub(crate) fn join_exploration_groups(
    older: &Arc<dyn HistoryCell>,
    newer: &Arc<dyn HistoryCell>,
    turns: &[Turn],
) -> Option<Arc<dyn HistoryCell>> {
    let older = older.as_any().downcast_ref::<ExecCell>()?;
    let newer = newer.as_any().downcast_ref::<ExecCell>()?;
    let items = adjacent_activity_items(&ids_for(older, newer), turns)?;
    let mut group = completed_group(items)?;
    group.group.details = newer.group.details.clone();
    group
        .group
        .details
        .prepend(older.group.details.clone(), older.group.calls.len());
    Some(Arc::new(group))
}

pub(crate) fn older_exploration_group(
    older: &dyn HistoryCell,
    newer: &ExecCell,
    turns: &[Turn],
) -> Option<ExecCell> {
    let older = older.as_any().downcast_ref::<ExecCell>()?;
    let items = adjacent_activity_items(&ids_for(older, newer), turns)?;
    // 008-TRIM: Reasoning 过滤（本地无 Reasoning 项，008 §1）
    let older_items = items
        .iter()
        .take(older.group.calls.len())
        .cloned()
        .collect::<Vec<_>>();
    let mut group = completed_group(&older_items)?;
    group.group.details = older.group.details.clone();
    Some(group)
}

fn ids_for<'a>(older: &'a ExecCell, newer: &'a ExecCell) -> Vec<&'a str> {
    older
        .iter_calls()
        .chain(newer.iter_calls())
        .map(|call| call.call_id.as_str())
        .collect()
}

/// 母本 computer_groups.rs:62 adjacent_activity_items——逐字移入（Reasoning 跳过分支
/// 因本地无 Reasoning 项不存在，其余逐字保留）。
pub(super) fn adjacent_activity_items<'a>(
    ids: &[&str],
    turns: &'a [Turn],
) -> Option<&'a [ThreadItem]> {
    // 008：通用相邻项查找（原实现自母本 computer_groups.rs:62 逐字移入；
    // Reasoning 跳过分支因本地无 Reasoning 项不存在，其余逐字保留）。
    let first = *ids.first()?;
    let (turn, start) = turns.iter().find_map(|turn| {
        turn.items
            .iter()
            .position(|item| item.id() == first)
            .map(|start| (turn, start))
    })?;
    let mut matched = 0;
    for (offset, item) in turn.items[start..].iter().enumerate() {
        if item.id() != ids[matched] {
            return None;
        }
        matched += 1;
        if matched == ids.len() {
            return Some(&turn.items[start..=start + offset]);
        }
    }
    None
}

pub(super) fn completed_group(items: &[ThreadItem]) -> Option<ExecCell> {
    let mut group: Option<ExecCell> = None;
    for item in items {
        let call = CommandHistory::from_item(item.clone())?.into_cell();
        if !call.is_exploring_cell() {
            return None;
        }
        if let Some(group) = &mut group {
            group.append_completed(call).ok()?;
        } else {
            group = Some(call);
        }
    }
    group
}
