//! 035：V3 `HistoryItem::AgentEvent` 的构造与链校验（§1.7）——新增事件类型
//! agent_created/assignment_started/child_record/message_queued/
//! message_delivered/state_changed/group_closed；`child_record` 只容纳真实
//! 共同消息/原生包装（Text/AssistantToolCalls/ToolResult，含 NativeAssistant
//! 包装），不允许递归 AgentEvent、检查点、JS 事件或任意宿主控制对象。
//! 创建及消息/任务准入记录保存成功才发布状态、启动或回 accepted——事件
//! 只能由宿主构造（不把工具返回 JSON 反解析成事件，034 同契约）。
//! 序号只在提交成功后占用（034 审查 P1-3：保存失败不留空洞）。

use crate::prompt::{AgentEventKind, AgentEventPayload, AgentReport, HistoryItem};
use serde_json::Value;

/// 构造一条 Agent 事件（不落盘；由 sink 提交成功后才算发生）。
#[allow(clippy::too_many_arguments)]
pub fn agent_event(
    kind: AgentEventKind,
    group_id: u64,
    seq: u64,
    agent_name: Option<&str>,
    assignment_id: Option<u64>,
    payload: AgentEventPayload,
) -> HistoryItem {
    HistoryItem::AgentEvent {
        kind,
        group_id,
        seq,
        agent_name: agent_name.map(str::to_string),
        assignment_id,
        payload,
    }
}

/// 终态报告构造：正文 ≤64 KiB；超长保留 64 KiB 片段＋原始字节数＋SHA-256
/// ＋truncated 标记，并以 RESULT_TOO_LARGE 失败语义呈现（§1.6 终态通道）。
pub fn terminal_report(content: &str, cleanup_note: &str) -> AgentReport {
    const LIMIT: usize = super::MAX_REPORT_BYTES;
    let bounded_cleanup: String = if cleanup_note.as_bytes().len() > super::MAX_CLEANUP_NOTE_BYTES {
        let mut cut = super::MAX_CLEANUP_NOTE_BYTES;
        while cut > 0 && !cleanup_note.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…（收尾说明截断）", &cleanup_note[..cut])
    } else {
        cleanup_note.to_string()
    };
    if content.as_bytes().len() <= LIMIT {
        return AgentReport {
            content: content.to_string(),
            original_bytes: None,
            sha256: None,
            truncated: false,
            cleanup: bounded_cleanup,
        };
    }
    let mut cut = LIMIT;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    use sha2::Digest;
    let hash = sha2::Sha256::digest(content.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    AgentReport {
        content: format!("{}…（终态报告超长，原文未完整保存）", &content[..cut]),
        original_bytes: Some(content.as_bytes().len() as u64),
        sha256: Some(hex),
        truncated: true,
        cleanup: bounded_cleanup,
    }
}

/// 单个子事件完整序列化上限检查（≤2 MiB，§1.6）。超限时调用方不得提交，
/// 按「明确截断证据＋不可续用」路径失败。
pub fn event_size_ok(item: &HistoryItem) -> bool {
    serde_json::to_string(item)
        .map(|s| s.as_bytes().len() <= super::MAX_EVENT_BYTES)
        .unwrap_or(false)
}

/// 序列化字节数（组负载计费口径：含 JSON 转义与来源字段）。
pub fn event_bytes(item: &HistoryItem) -> u64 {
    serde_json::to_string(item)
        .map(|s| s.as_bytes().len() as u64)
            .unwrap_or(u64::MAX)
}

/// child_record 载荷项合法性：只允许 Text/AssistantToolCalls/ToolResult
///（含 NativeAssistant 包装的共同记录）；递归 AgentEvent/检查点/JS 事件
/// 一律拒绝（构造与装载共用同一判定）。
pub fn portable_child_item(item: &HistoryItem) -> bool {
    match item {
        HistoryItem::Text(_)
        | HistoryItem::AssistantToolCalls { .. }
        | HistoryItem::ToolResult { .. } => true,
        HistoryItem::NativeAssistant { message, .. } => portable_child_item(message),
        HistoryItem::ContextCheckpoint { .. }
        | HistoryItem::OrchestrationEvent { .. }
        | HistoryItem::AgentEvent { .. } => false,
    }
}

/// V3 装载时的 Agent 事件链语义校验（session.rs 调用；整文件执行一次）：
/// - 每组 seq 自 1 连续递增（按文件顺序）；
/// - 同组同名 agent 恰一个 agent_created，且先于该 agent 的其他事件；
/// - group_closed 至多一次且为该组最后一个事件；
/// - child_record 载荷项全部为可携带记录；
/// - state_changed 的状态名合法；message 载荷 message_id 非 0。
pub fn validate_agent_chain(history: &[HistoryItem]) -> Result<(), String> {
    use std::collections::{HashMap, HashSet};
    #[derive(Default)]
    struct GroupSeen {
        next_seq: u64,
        created: HashSet<String>,
        /// 已终态（closed 后不再接受该组事件）。
        closed: bool,
    }
    let mut groups: HashMap<u64, GroupSeen> = HashMap::new();
    for (idx, item) in history.iter().enumerate() {
        let HistoryItem::AgentEvent {
            kind,
            group_id,
            seq,
            agent_name,
            assignment_id,
            payload,
        } = item
        else {
            continue;
        };
        let seen = groups.entry(*group_id).or_default();
        if seen.closed {
            return Err(format!(
                "组 {group_id} 的 group_closed 之后第 {idx} 条记录仍有该组事件"
            ));
        }
        if *seq != seen.next_seq + 1 {
            return Err(format!(
                "组 {group_id} 事件序号不连续（第 {idx} 条为 {seq}，期望 {}）",
                seen.next_seq + 1
            ));
        }
        seen.next_seq = *seq;
        match kind {
            AgentEventKind::AgentCreated => {
                let name = agent_name.as_deref().unwrap_or("");
                if name.is_empty() || !super::args::valid_task_name(name) {
                    return Err(format!("组 {group_id} 第 {idx} 条 agent_created 名称非法"));
                }
                if !seen.created.insert(name.to_string()) {
                    return Err(format!(
                        "组 {group_id} 的 agent {name} 重复 agent_created（第 {idx} 条）"
                    ));
                }
                if !matches!(payload, AgentEventPayload::Fork { .. }) {
                    return Err(format!(
                        "组 {group_id} 第 {idx} 条 agent_created 载荷须为 fork 引用"
                    ));
                }
            }
            AgentEventKind::GroupClosed => {
                if agent_name.is_some() {
                    return Err(format!(
                        "组 {group_id} 第 {idx} 条 group_closed 不应携带 agent_name"
                    ));
                }
                seen.closed = true;
            }
            _ => {
                let name = agent_name.as_deref().unwrap_or("");
                if !seen.created.contains(name) {
                    return Err(format!(
                        "组 {group_id} 第 {idx} 条事件引用未创建的 agent {name:?}"
                    ));
                }
            }
        }
        if let AgentEventPayload::Records { items } = payload {
            for (i, child) in items.iter().enumerate() {
                if !portable_child_item(child) {
                    return Err(format!(
                        "组 {group_id} 第 {idx} 条 child_record 第 {i} 项含不可携带记录"
                    ));
                }
            }
        }
        if let AgentEventPayload::State { state, .. } = payload {
            if !valid_state_name(state) {
                return Err(format!(
                    "组 {group_id} 第 {idx} 条 state_changed 状态名非法：{state}"
                ));
            }
        }
        if let AgentEventPayload::Queued { message_id, .. } = payload {
            if *message_id == 0 {
                return Err(format!("组 {group_id} 第 {idx} 条消息 message_id 为 0"));
            }
        }
        if let Some(id) = assignment_id {
            if *id == 0 {
                return Err(format!("组 {group_id} 第 {idx} 条事件 assignment_id 为 0"));
            }
        }
    }
    Ok(())
}

fn valid_state_name(state: &str) -> bool {
    matches!(
        state,
        "starting" | "running" | "stopping" | "completed" | "failed" | "interrupted" | "closed"
    )
}

/// 根 messages/_history/五协议投影的事件过滤判定（prompt/api/memory 共用）：
/// Agent 事件不进入模型投影（§1.7——根只看到六工具调用与汇总结果）。
pub fn is_agent_event(item: &HistoryItem) -> bool {
    matches!(item, HistoryItem::AgentEvent { .. })
}

/// 事件 → 展示摘要行（CLI/TUI 展示副本；正文经调用方 sanitize）。
pub fn event_display_line(item: &HistoryItem) -> Option<String> {
    let HistoryItem::AgentEvent {
        kind,
        group_id,
        agent_name,
        payload,
        ..
    } = item
    else {
        return None;
    };
    let who = agent_name.as_deref().unwrap_or("组");
    Some(match kind {
        AgentEventKind::AgentCreated => format!("[Agent g{group_id}] {who} 已创建（受理，不代表完成）"),
        AgentEventKind::AssignmentStarted => {
            format!("[Agent g{group_id}] {who} 新 assignment 已受理")
        }
        AgentEventKind::ChildRecord => return None, // 批量记录不逐条展示
        AgentEventKind::MessageQueued => match payload {
            AgentEventPayload::Queued { from, to, .. } => {
                format!("[Agent g{group_id}] {from} → {to} 消息已入队")
            }
            _ => return None,
        },
        AgentEventKind::MessageDelivered => return None,
        AgentEventKind::StateChanged => match payload {
            AgentEventPayload::State { state, report, .. } => match report {
                Some(r) => format!(
                    "[Agent g{group_id}] {who} → {state}{}",
                    if r.truncated { "（终态报告超长截断）" } else { "" }
                ),
                None => format!("[Agent g{group_id}] {who} → {state}"),
            },
            _ => return None,
        },
        AgentEventKind::GroupClosed => format!("[Agent g{group_id}] 组已关闭"),
    })
}

/// 调试/测试辅助：事件 → JSON 值。
#[allow(dead_code)]
pub(crate) fn event_value(item: &HistoryItem) -> Value {
    serde_json::to_value(item).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::HistoryMessage;

    fn text(content: &str) -> HistoryItem {
        HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: crate::prompt::HistoryRole::User,
            content: content.into(),
            user_input_event: None,
        })
    }

    fn created(group: u64, seq: u64, name: &str) -> HistoryItem {
        agent_event(
            AgentEventKind::AgentCreated,
            group,
            seq,
            Some(name),
            Some(1),
            AgentEventPayload::Fork {
                context_id: 0,
                prefix_end: 0,
                fork: "none".into(),
                task: "任务".into(),
            },
        )
    }

    #[test]
    fn chain_accepts_valid_and_rejects_breaks() {
        let ok = vec![
            created(1, 1, "a"),
            agent_event(
                AgentEventKind::StateChanged,
                1,
                2,
                Some("a"),
                Some(1),
                AgentEventPayload::State {
                    state: "running".into(),
                    reason: None,
                    report: None,
                },
            ),
            agent_event(
                AgentEventKind::GroupClosed,
                1,
                3,
                None,
                None,
                AgentEventPayload::GroupClose {
                    reason: "turn_exit".into(),
                },
            ),
        ];
        assert!(validate_agent_chain(&ok).is_ok());
        // seq 空洞
        let bad = vec![created(1, 1, "a"), created(1, 3, "b")];
        assert!(validate_agent_chain(&bad).is_err());
        // 未创建先引用
        let bad = vec![agent_event(
            AgentEventKind::StateChanged,
            1,
            1,
            Some("ghost"),
            Some(1),
            AgentEventPayload::State {
                state: "running".into(),
                reason: None,
                report: None,
            },
        )];
        assert!(validate_agent_chain(&bad).is_err());
        // 重复创建
        let bad = vec![created(1, 1, "a"), created(1, 2, "a")];
        assert!(validate_agent_chain(&bad).is_err());
        // closed 后仍有事件
        let bad = vec![
            created(1, 1, "a"),
            agent_event(
                AgentEventKind::GroupClosed,
                1,
                2,
                None,
                None,
                AgentEventPayload::GroupClose {
                    reason: "x".into(),
                },
            ),
            agent_event(
                AgentEventKind::StateChanged,
                1,
                3,
                Some("a"),
                Some(1),
                AgentEventPayload::State {
                    state: "closed".into(),
                    reason: None,
                    report: None,
                },
            ),
        ];
        assert!(validate_agent_chain(&bad).is_err());
        // child_record 含递归事件拒绝
        let bad = vec![
            created(1, 1, "a"),
            agent_event(
                AgentEventKind::ChildRecord,
                1,
                2,
                Some("a"),
                Some(1),
                AgentEventPayload::Records {
                    items: vec![created(1, 1, "a")],
                },
            ),
        ];
        assert!(validate_agent_chain(&bad).is_err());
        // 合法共同记录通过（含 NativeAssistant 包装）
        let okk = vec![
            created(1, 1, "a"),
            agent_event(
                AgentEventKind::ChildRecord,
                1,
                2,
                Some("a"),
                Some(1),
                AgentEventPayload::Records {
                    items: vec![
                        text("hi"),
                        HistoryItem::NativeAssistant {
                            message: Box::new(text("wrapped")),
                            state: None,
                        },
                    ],
                },
            ),
        ];
        assert!(validate_agent_chain(&okk).is_ok());
    }

    #[test]
    fn terminal_report_degrades_over_limit() {
        let small = terminal_report("done", "已回收 1 个进程");
        assert!(!small.truncated);
        let big = terminal_report(&"x".repeat(super::super::MAX_REPORT_BYTES + 100), "c");
        assert!(big.truncated);
        assert_eq!(big.original_bytes, Some((super::super::MAX_REPORT_BYTES + 100) as u64));
        assert!(big.sha256.as_deref().is_some_and(|h| h.len() == 64));
    }
}
