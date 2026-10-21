//! Project persisted items into the same rich cells as completed live output.
//! Tool grouping spans hidden reasoning but stops at visible content and turn boundaries.
//! Only one computer or exploration group can be pending at a time.

use std::sync::Arc;

use crate::git_action_directives::parse_assistant_markdown;
use crate::history_cell::AgentMarkdownCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::PrefixedWrappedHistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::inline_visualization::InlineVisualizationContext;
use crate::legacy_core::config::Config;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::UserInput;
use codex_protocol::ThreadId;
use codex_utils_absolute_path::AbsolutePathBuf;
use ratatui::style::Stylize as _;

mod activity_pages;
// 008-TRIM: mod computer_groups（008 §1 裁剪：computer-use 无本地事件源）
mod exploration_groups; // 008-REVIVE：连续命令执行分组（本地 exec 连续调用存在，实现轮裁决）
mod other_items;
pub(crate) mod tools;

pub(crate) use activity_pages::fold_trailing_activity_details;
pub(crate) use activity_pages::is_hidden_activity_detail;
#[allow(
    unused_imports,
    reason = "Used by later layers of the TUI refresh stack."
)]
// 008-TRIM: computer_groups re-exports（computer-use 无本地事件源，008 §1）
pub(crate) use exploration_groups::join_exploration_groups;
pub(crate) use exploration_groups::older_exploration_group;

pub(crate) type TranscriptCells = Vec<Arc<dyn HistoryCell>>;

enum PendingActivity {
    // 008-TRIM: Computer 变体（computer-use 无本地事件源，008 §1）
    Exploration(crate::exec_cell::ExecCell),
}

impl PendingActivity {
    fn flush(pending: &mut Option<Self>, cells: &mut TranscriptCells) {
        let cell: Arc<dyn HistoryCell> = match pending.take() {
            Some(Self::Exploration(group)) => Arc::new(group),
            None => return,
        };
        cells.push(cell);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RawReasoningVisibility {
    Hidden,
    Visible,
}

// 008-TRIM: load_session_transcript（母本经 app_server_session 水合历史；本地 /resume 投影由
// 驱动（P3）从 HistoryItem 构建 Thread 后直调 thread_to_transcript_cells，不引入后端会话层）。

pub(crate) fn thread_to_transcript_cells(
    thread: Thread,
    raw_reasoning_visibility: RawReasoningVisibility,
    config: Option<&Config>,
) -> TranscriptCells {
    let cwd = thread.cwd;
    let thread_id = ThreadId::from_string(&thread.id).ok();
    let mut cells = thread
        .turns
        .into_iter()
        .flat_map(|turn| {
            thread_items_to_transcript_cells(
                thread_id.clone(),
                &cwd,
                turn.items,
                raw_reasoning_visibility,
                config,
            )
        })
        .collect::<TranscriptCells>();
    if cells.is_empty() {
        cells.push(Arc::new(PlainHistoryCell::new(vec![
            "No transcript content available".italic().dim().into(),
        ])));
    }
    cells
}

pub(crate) fn thread_items_to_transcript_cells(
    thread_id: Option<ThreadId>,
    cwd: &AbsolutePathBuf,
    items: impl IntoIterator<Item = ThreadItem>,
    raw_reasoning_visibility: RawReasoningVisibility,
    config: Option<&Config>,
) -> TranscriptCells {
    let inline_visualization_context = config.and_then(|config| {
        thread_id.and_then(|thread_id| InlineVisualizationContext::from_config(config, thread_id))
    });
    let mut cells: TranscriptCells = Vec::new();
    let mut pending = None;
    for item in items {
        // 008-TRIM: Reasoning 项前置并入分组（本地无 reasoning 项，008 §1）
        if let Some(cell) = tools::historical_tool_fallback(&item) {
            PendingActivity::flush(&mut pending, &mut cells);
            cells.push(Arc::new(cell));
            continue;
        }
        match item {
            // 008-TRIM: McpToolCall 臂（MCP/computer-use 无本地事件源，008 §1）
            item @ ThreadItem::CommandExecution { .. } => {
                if let Some(command) = tools::CommandHistory::from_item(item) {
                    let newer = command.into_cell();
                    if let Some(PendingActivity::Exploration(group)) = &mut pending {
                        if let Err(newer) = group.append_completed(newer) {
                            PendingActivity::flush(&mut pending, &mut cells);
                            pending = Some(PendingActivity::Exploration(newer));
                        }
                    } else {
                        pending = Some(PendingActivity::Exploration(newer));
                    }
                    if let Some(PendingActivity::Exploration(group)) = &pending
                        && group.should_flush()
                    {
                        PendingActivity::flush(&mut pending, &mut cells);
                    }
                }
            }
            item => {
                let projected = item_to_cells(
                    item,
                    cwd,
                    raw_reasoning_visibility,
                    inline_visualization_context.clone(),
                );
                if !projected.is_empty() {
                    PendingActivity::flush(&mut pending, &mut cells);
                    cells.extend(projected);
                }
            }
        }
    }
    PendingActivity::flush(&mut pending, &mut cells);
    cells
}

/// Project one item without changing the active widget or its turn lifecycle.
fn item_to_cells(
    item: ThreadItem,
    cwd: &AbsolutePathBuf,
    raw_reasoning_visibility: RawReasoningVisibility,
    inline_visualization_context: Option<InlineVisualizationContext>,
) -> TranscriptCells {
    let mut cells: TranscriptCells = Vec::new();
    match item {
        ThreadItem::UserMessage {
            content, ..
        } => {
            // 008-ADAPT（本地投影）：母本经 UserMessageItem/async_question_reply 提取文本；
            // 本地 UserInput 仅 Text 变体，message＝文本拼接、text_elements 重定基透传、
            // 图片路径/URL 恒空（无图片输入源）。
            let mut message = String::new();
            let mut text_elements = Vec::new();
            let mut consumed = 0usize;
            for input in content {
                if let UserInput::Text { text, text_elements: elements } = input {
                    if !message.is_empty() {
                        message.push('\n');
                        consumed += 1;
                    }
                    let base = consumed;
                    message.push_str(&text);
                    consumed += text.len();
                    text_elements.extend(elements.into_iter().map(|element| {
                        let mut range = element.byte_range;
                        range.start += base;
                        range.end += base;
                        codex_protocol::user_input::TextElement::new(
                            range,
                            element.placeholder,
                        )
                    }));
                }
            }
            if !message.is_empty() {
                cells.push(Arc::new(UserHistoryCell {
                    spoken: false,
                    message,
                    text_elements,
                    local_image_paths: Vec::new(),
                    remote_image_urls: Vec::new(),
                }));
            }
        }
        ThreadItem::AgentMessage { text, .. } => {
            let parsed = parse_assistant_markdown(&text, cwd.as_path());
            if !parsed.visible_markdown.trim().is_empty() {
                cells.push(Arc::new(AgentMarkdownCell::new_with_inline_visualizations(
                    parsed.visible_markdown,
                    cwd.as_path(),
                    inline_visualization_context,
                )));
            }
        }
        // 008-TRIM: FunctionCallOutput/Plan/Reasoning 臂（无本地事件源，008 §1）
        item @ ThreadItem::CommandExecution { .. } => {
            if let Some(command) = tools::CommandHistory::from_item(item) {
                cells.push(Arc::new(command.into_cell()));
            }
        }
        other => cells.extend(other_items::cells(other, cwd)),
    }
    cells
}
