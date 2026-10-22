//! Transcript/history cells for the Codex TUI.
//!
//! A `HistoryCell` is the unit of display in the conversation UI, representing both committed
//! transcript entries and, transiently, an in-flight active cell that can mutate in place while
//! streaming.
//!
//! The transcript overlay (`Ctrl+T`) appends a cached live tail derived from the active cell, and
//! that cached tail is refreshed based on an active-cell cache key. Cells that change based on
//! elapsed time expose `transcript_animation_tick()`, and code that mutates the active cell in place
//! bumps the active-cell revision tracked by `ChatWidget`, so the cache key changes whenever the
//! rendered transcript output can change.

mod activity_group; // 008-REVIVE：exec_cell 活动分组显示所需（实现轮裁决）
pub(crate) use activity_group::ActivityGroup; // 008-REVIVE

use crate::diff_model::FileChange;
use crate::diff_render::create_diff_summary;
use crate::exec_cell::CommandOutput;
use crate::exec_cell::OutputLinesParams;
use crate::exec_cell::TOOL_CALL_MAX_LINES;
use crate::exec_cell::output_lines;
use crate::exec_command::relativize_to_home;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::legacy_core::config::Config;
use crate::live_wrap::take_prefix_by_width;
use crate::motion::MotionMode;
use crate::motion::ReducedMotionIndicator;
use crate::motion::activity_indicator;
use crate::render::line_utils::line_to_static;
use crate::render::line_utils::prefix_lines;
use crate::render::line_utils::push_owned_lines;
use crate::render::renderable::Renderable;
use crate::session_state::ThreadSessionState;
use crate::style::proposed_plan_style;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::HyperlinkParagraph;
use crate::terminal_hyperlinks::plain_hyperlink_lines;
use crate::terminal_hyperlinks::prefix_hyperlink_lines;
use crate::terminal_hyperlinks::visible_lines;
#[cfg(test)]
use crate::test_support::PathBufExt;
#[cfg(test)]
use crate::test_support::test_path_buf;
use crate::text_formatting::truncate_text;
use crate::tooltips;
use crate::ui_consts::LIVE_PREFIX_COLS;
use crate::update_action::UpdateAction;
use crate::version::CODEX_CLI_VERSION;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_line;
use crate::wrapping::adaptive_wrap_lines;
use codex_app_server_protocol::AskForApproval;
use codex_app_server_protocol::McpAuthStatus;
use codex_app_server_protocol::McpServerStatus;
use codex_app_server_protocol::McpServerStatusDetail;
use codex_app_server_protocol::ToolRequestUserInputAnswer;
use codex_app_server_protocol::ToolRequestUserInputQuestion;
use codex_app_server_protocol::WebSearchAction;
// 008-TRIM: use codex_config::types::McpServerTransportConfig（MCP cell 测试裁剪残留，008 §1）
// 008-TRIM: use codex_mcp::qualified_mcp_tool_name_prefix（MCP cell 测试裁剪残留，008 §1）
use codex_otel::RuntimeMetricsSummary;
use codex_protocol::account::PlanType;
use codex_protocol::approvals::ExecPolicyAmendment;
use codex_protocol::approvals::NetworkPolicyAmendment;
// 008-TRIM: use codex_protocol::mcp::Resource（MCP cell 测试裁剪残留，008 §1）
// 008-TRIM: use codex_protocol::mcp::ResourceTemplate（MCP cell 测试裁剪残留，008 §1）
use codex_protocol::models::ManagedFileSystemPermissions; // 008-REVIVE：session yolo 判定消费
use codex_protocol::models::PermissionProfile;
use codex_protocol::models::local_image_label_text;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::plan_tool::PlanItemArg; // 017 恢复（计划 cell 事件源已接入）
use codex_protocol::plan_tool::StepStatus; // 017 恢复（同上）
use codex_protocol::plan_tool::UpdatePlanArgs; // 017 恢复（同上）
use codex_protocol::user_input::TextElement;
use codex_utils_absolute_path::AbsolutePathBuf;
// 008-TRIM: use codex_utils_cli::format_env_display（MCP cell 测试裁剪残留，008 §1）
use ratatui::prelude::*;
#[cfg(test)]
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Styled;
use ratatui::style::Stylize;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use std::any::Any;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;
use url::Url;

const RAW_DIFF_SUMMARY_WIDTH: usize = 10_000;

mod activity_details; // 008-REVIVE：activity_group 消费
pub(crate) mod activity_preview;
pub(crate) use activity_preview::ActivityDisclosure; // 008-REVIVE：diff/活动预览裁剪行工具（patches/exec_cell 预览消费，实现轮裁决）
// 008-TRIM: mod approvals（008 §1 裁剪清单：无本地事件源）
mod base;
// 008-TRIM: mod dynamic（DynamicToolCallCell＝插件动态工具 cell，无本地事件源；规格 v2 首批清单曾误标为 exec 动态输出——exec 预览实在 exec_cell/，实现轮更正并登记）
mod exec;
// 008-TRIM: mod hook_cell（008 §1 裁剪清单：无本地事件源）
mod markdown_render_cache;
// 008-TRIM: mod mcp（008 §1 裁剪清单：无本地事件源）
mod messages;
mod notices;
mod patches;
mod plans; // 017 恢复（update_plan 事件源已接入，008 §1 曾裁剪）

#[cfg(test)]
#[path = "plans_tests.rs"]
mod plans_tests;
// 008-TRIM: mod request_user_input（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod search（008 §1 裁剪清单：无本地事件源）
mod separators;
mod session;
// 008-TRIM: mod spoken_artifacts（008 §1 裁剪清单：无本地事件源）
mod startup_warnings;
mod warnings;

pub(crate) use activity_details::ActivityDetails; // 008-REVIVE：activity_group 消费

// 008-TRIM: pub(crate) use approvals::*;（裁剪 cell，008 §1）
pub(crate) use base::*;
pub use base::PlainHistoryCell; // 008-ADAPT：pub（A7 安全门禁测试——错误消息入口）
// 008-TRIM: DynamicToolCallCell re-export（同上）
pub(crate) use exec::*;
// 008-TRIM: pub(crate) use hook_cell::HookCell;（裁剪 cell，008 §1）
// 008-TRIM: pub(crate) use hook_cell::new_active_hook_cell;（裁剪 cell，008 §1）
// 008-TRIM: pub(crate) use hook_cell::new_completed_hook_cell;（裁剪 cell，008 §1）
// 008-TRIM: pub(crate) use mcp::*;（裁剪 cell，008 §1）
pub use messages::*; // 008-ADAPT：pub（A7：AgentMarkdownCell 公开）
pub(crate) use notices::*;
pub(crate) use patches::*;
pub(crate) use plans::*; // 017 恢复（update_plan 事件源已接入）
// 008-TRIM: pub(crate) use request_user_input::*;（裁剪 cell，008 §1）
// 008-TRIM: pub(crate) use search::*;（裁剪 cell，008 §1）
pub(crate) use separators::*;
pub(crate) use session::*;
pub(crate) use startup_warnings::StartupWarningsCell;
pub(crate) use warnings::WarningEntry;
pub(crate) use warnings::WarningHistoryCell;
pub(crate) use warnings::WarningId;
pub(crate) use warnings::WarningKey;
pub(crate) use warnings::warning_count;
pub(crate) use warnings::warning_entries;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryRenderMode { // 008-ADAPT：pub（A7）
    Rich,
    Raw,
}

pub(crate) fn raw_lines_from_source(source: &str) -> Vec<Line<'static>> {
    if source.is_empty() {
        return Vec::new();
    }

    let mut parts = source.split('\n').collect::<Vec<_>>();
    if source.ends_with('\n') {
        parts.pop();
    }

    parts
        .into_iter()
        .map(|line| Line::from(line.to_string()))
        .collect()
}

pub(crate) fn plain_lines(lines: impl IntoIterator<Item = Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let text = line
                .spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>();
            Line::from(text)
        })
        .collect()
}

/// A single renderable unit of conversation history.
///
/// Each cell produces logical `Line`s and reports how many viewport
/// rows those lines occupy at a given terminal width. The default
/// height implementations use `Paragraph::wrap` to account for lines
/// that overflow the viewport width (e.g. long URLs that are kept
/// intact by adaptive wrapping). Concrete types only need to override
/// heights when they apply additional layout logic beyond what
/// `Paragraph::line_count` captures.
pub trait HistoryCell: std::fmt::Debug + Send + Sync + Any { // 008-ADAPT：trait 提为 pub（chat_driver::HostCell 公共边界）
    /// Retain reasoning inside an activity group, or return it for normal insertion.
    /// Call positions stay fixed as in-flight calls complete; other history cells decline it.
    fn append_reasoning(&mut self, cell: Box<dyn HistoryCell>) -> Result<(), Box<dyn HistoryCell>> {
        Err(cell)
    }

    /// Diagnostics exposed by the warnings picker, retaining their source and full text.
    fn warning_entries(&self) -> Vec<WarningEntry> {
        Vec::new()
    }

    /// Stable diagnostic identities, independent of rendered wrapping and duplicate delivery.
    fn warning_keys(&self) -> Vec<WarningKey<'_>> {
        Vec::new()
    }

    /// Original Markdown for semantic copy targets, independent of presentation.
    fn copy_source(&self) -> Option<&str> {
        None
    }

    /// Returns the logical lines for the main chat viewport.
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    /// Returns copy-friendly plain logical lines for raw scrollback mode.
    fn raw_lines(&self) -> Vec<Line<'static>>;

    /// Raw live presentation may omit transcript-only diagnostics without losing their source.
    fn live_raw_lines(&self) -> Vec<Line<'static>> {
        self.raw_lines()
    }

    /// Returns rich visible lines plus terminal hyperlink metadata.
    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        plain_hyperlink_lines(self.display_lines(width))
    }

    /// Compact presentation for the owned transcript, which can reveal details in place.
    #[allow(dead_code, reason = "Used by later layers of the TUI refresh stack.")]
    fn compact_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }

    /// Rich presentation in a viewport that can redraw previously visible rows.
    fn retained_hyperlink_lines(&self, width: u16, detailed: bool) -> Vec<HyperlinkLine> {
        if detailed {
            self.transcript_hyperlink_lines(width)
        } else {
            self.compact_hyperlink_lines(width)
        }
    }

    /// Stable, namespaced member identities used to retain disclosure across grouping and replay.
    /// Empty identities indicate ordinary content without a local disclosure control.
    fn activity_ids(&self) -> Vec<String> {
        Vec::new()
    }

    /// Available activity details, preserving source order and any upstream truncation notices.
    fn expanded_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.transcript_hyperlink_lines(width)
    }

    /// Details an activity offers beyond its compact presentation, if any.
    /// Prefer source metadata so collapsed rendering does not materialize hidden content.
    fn activity_disclosure(&self, _width: u16) -> Option<ActivityDisclosure> {
        Some(ActivityDisclosure::Generic)
    }

    fn display_lines_for_mode(&self, width: u16, mode: HistoryRenderMode) -> Vec<Line<'static>> {
        match mode {
            HistoryRenderMode::Rich => visible_lines(self.display_hyperlink_lines(width)),
            HistoryRenderMode::Raw => self.live_raw_lines(),
        }
    }

    fn display_hyperlink_lines_for_mode(
        &self,
        width: u16,
        mode: HistoryRenderMode,
    ) -> Vec<HyperlinkLine> {
        match mode {
            HistoryRenderMode::Rich => self.display_hyperlink_lines(width),
            HistoryRenderMode::Raw => plain_hyperlink_lines(self.live_raw_lines()),
        }
    }

    /// Returns the number of viewport rows needed to render this cell.
    ///
    /// The default delegates to `Paragraph::line_count` with
    /// `Wrap { trim: false }`, which measures the actual row count after
    /// ratatui's viewport-level character wrapping. This is critical
    /// for lines containing URL-like tokens that are wider than the
    /// terminal — the logical line count would undercount.
    fn desired_height(&self, width: u16) -> u16 {
        self.desired_height_for_mode(width, HistoryRenderMode::Rich)
    }

    fn desired_height_for_mode(&self, width: u16, mode: HistoryRenderMode) -> u16 {
        Paragraph::new(Text::from(self.display_lines_for_mode(width, mode)))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }

    /// Returns lines for the transcript overlay (`Ctrl+T`).
    ///
    /// Defaults to `display_lines`. Override when the transcript
    /// representation differs (e.g. `ExecCell` shows all calls with
    /// `$`-prefixed commands and exit status).
    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.display_lines(width)
    }

    /// Returns transcript-overlay lines plus terminal hyperlink metadata.
    ///
    /// Defaults to the plain transcript representation because some cells render different
    /// display and transcript content. Rich cells whose transcript mirrors their display should
    /// delegate to `display_hyperlink_lines`.
    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        plain_hyperlink_lines(self.transcript_lines(width))
    }

    /// Whether the cached transcript layout remains valid across later frames.
    ///
    /// Cells backed by external state should return `false` so the shared viewport refreshes
    /// their rendered text and source mapping each frame instead of reusing stale content.
    fn has_stable_transcript_height(&self) -> bool {
        true
    }

    fn is_stream_continuation(&self) -> bool {
        false
    }

    /// Returns a coarse "animation tick" when transcript output is time-dependent.
    ///
    /// The transcript overlay caches the rendered output of the in-flight active cell, so cells
    /// that include time-based UI (spinner, shimmer, etc.) should return a tick that changes over
    /// time to signal that the cached tail should be recomputed. Returning `None` means the
    /// transcript lines are stable, while returning `Some(tick)` during an in-flight animation
    /// allows the overlay to keep up with the main viewport.
    ///
    /// If a cell uses time-based visuals but always returns `None`, `Ctrl+T` can appear "frozen" on
    /// the first rendered frame even though the main viewport is animating.
    fn transcript_animation_tick(&self) -> Option<u64> {
        None
    }
}

impl Renderable for Box<dyn HistoryCell> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let hyperlink_lines = self.display_hyperlink_lines(area.width);
        let paragraph = HyperlinkParagraph::new(&hyperlink_lines, Style::default());
        let y = if area.height == 0 {
            0
        } else {
            let overflow = paragraph
                .line_count(area.width)
                .saturating_sub(usize::from(area.height));
            u16::try_from(overflow).unwrap_or(u16::MAX)
        };
        // Active-cell content can reflow dramatically during resize/stream updates. Clear the
        // entire draw area first so stale glyphs from previous frames never linger.
        Clear.render(area, buf);
        paragraph.scroll(y).render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        HistoryCell::desired_height(self.as_ref(), width)
    }
}

impl dyn HistoryCell {
    pub(crate) fn as_any(&self) -> &dyn Any {
        self
    }

    pub(crate) fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
