//! The chat composer is the bottom-pane text input state machine.
//!
//! It edits [`TextArea`] and attachments, routes popup keys, makes completed slash commands atomic,
//! and handles Enter/newlines. It shows Luna Reserve's yellow arrow and detects unbracketed paste
//! bursts, especially on Windows. Paste timing uses Tokio's clock so asynchronous flush deadlines
//! and input classification share a clock, including in paused-time tests. Copy shortcuts and right
//! clicks preserve selected draft text.
//! When enabled, fullscreen right-click paste requires an editable composer without a selection,
//! search, or blocking view. The app reads clipboard text asynchronously and delivers a normal
//! paste only while the same thread, draft, and cursor remain eligible. Intervening input or focus
//! loss cancels the pending paste; a late clipboard result cannot overwrite newer input.
//! The live voice strip renders after effort ignition, followed by the Astra sparkle when eligible.
//! Owned transcripts keep persistent status below the composer and hints on a separate final row.
//! Shortcut help expands above the composer, with its close hint replacing the final shortcuts row
//! so input and persistent status stay anchored when help opens or closes.
//! Escape dismisses visible shortcut help before editing, transcript backtracking, or interruption.
//! Fullscreen limits the composer height; wheel browsing preserves the caret until editor input.
//! Transcript interactions borrow the hint footer through `ComposerRenderOptions`;
//! its resolved presentation drives height, painting, and the focused cursor together.
//!
//! The plain-text preset keeps command prefixes literal, including `!`, so Enter and Tab
//! submit ordinary text without enabling shell mode.
//!
//! # Mention Menus
//!
//! By default, `@` lists plugins, filesystem entries, and skills. Skills are hidden when their
//! owning plugin is listed. `$` lists individual skills and apps, but not plugins.
//! Disabling `mentions_v2` restores file-only `@` search and adds plugins back to `$`.
//!
//! # Key Event Routing
//!
//! Plain Left opens agents when the local-daemon composer is empty and available for input.
//! Explicit editor remaps take precedence.
//! Most key handling goes through [`ChatComposer::handle_key_event`], which dispatches to a
//! popup-specific handler if a popup is visible and otherwise to
//! [`ChatComposer::handle_key_event_without_popup`]. After every handled key, we call
//! [`ChatComposer::sync_popups`] so UI state follows the latest buffer/cursor.
//! Fresh Vim drafts start in Insert; Normal `/` and `?` search the composer.
//! On an empty draft, a standalone Normal `/` enters Insert and opens command completion.
//! Backspace on an empty Vim search query cancels search and any pending operator.
//!
//! # Completion and Popup Dismissal
//!
//! All completion suggestions render above the composer and preserve its footer.
//! Owned transcript frames overlay suggestions with blank rows above and below, without reserving
//! layout space. Those rows show scroll arrows when suggestions extend beyond the visible menu.
//! Warning and transcript views hide suggestions without losing the draft, query, or selection.
//! Measurement, painting, and cursor placement share that layout, including clipped views.
//! Unified mention tabs retain their position across filters; Left/Right also works at a bare `@`.
//!
//! Popup targeting resolves an editable token range around the cursor and treats atomic text
//! elements as hard boundaries. When that range begins immediately after an atomic element, the
//! insertion path first adds a horizontal separator and shifts the range so the completion cannot
//! merge with the atomic element. After replacing the range, completion leaves the cursor after one
//! horizontal separator. It advances across an existing separator when no suffix would be joined;
//! otherwise it inserts a space, preserving the existing separator before a non-whitespace suffix.
//! It also inserts a space rather than crossing a line break.
//!
//! `Esc` records the active token as dismissed. A completed value that begins with `@` or `$` is
//! also re-dismissed before popup synchronization, because separator affinity can still identify
//! the completed token to the left of the cursor. Synchronization keeps the popup hidden only while
//! the query, complete token text, and ordinal among matching occurrences remain the same.
//! Whitespace and atomic-element edges delimit occurrences, while nested sigils do not. This
//! preserves dismissal across offset-only edits without suppressing a later identical token.
//!
//! Slash-command dismissal is tracked separately. Pressing `Esc` while the command popup is active
//! records the first-line `/name` token and keeps the popup closed while that token is unchanged.
//! Editing the command token clears the dismissal and allows the popup to reopen.
//!
//! # History Navigation (↑/↓)
//!
//! The Up/Down history path is managed by [`ChatComposerHistory`]. It merges:
//!
//! - Persistent cross-session history (text-only storage; task links recover element ranges and
//!   mention bindings, but attachments cannot be restored).
//! - Local in-session history (full text + text elements + local/remote image attachments).
//!
//! Plain-text history recall strips images and their placeholders.
//! When recalling a persistent entry, encoded task links restore atomic elements and bindings.
//! Recall moves the cursor to the end. Question editors copy primary history on recall/search;
//! draft capture cancels previews, and restoration resets traversal.
//! Ctrl+R searches history in the footer and previews matches in the composer.
//! Typing and pasting edit the active search query, including large pastes and image paths.
//! Background recovery updates the saved prompt without changing the query or visible preview.
//! Esc or Ctrl+C restores that prompt, including recovered answers and interrupted input.
//! Enter accepts a matching preview and discards the saved prompt; without a match, search stays open.
//! Vim undo/redo snapshots complete drafts and groups direct edits with active Vim transactions.
//! An active edit keeps one separately capped snapshot; canceling does not evict committed history.
//! Canceled history previews restore history and active commands; accepting another prompt resets them.
//! Normal-mode Ctrl+R redoes an edit, or does nothing when redo is empty. Insert-mode Ctrl+R
//! keeps prompt-history search; explicitly configured keybindings retain precedence.
//! Vim queries stay draft-local.
//!
//! Slash commands are staged for local history instead of being recorded immediately. Command
//! recall is a two-phase handoff: stage the submitted slash text here, then record it after
//! `ChatWidget` dispatches the command.
//!
//! # Question Draft Recovery
//!
//! Live terminal turns recover typed, unsubmitted question answers into the main composer.
//! Each recovered answer follows its original question as a Markdown blockquote, with blank lines
//! separating questions, answers, and existing text. The append flushes buffered input and dismisses
//! unused sparkle eligibility. The separator and recovered answer form one Vim edit; large answers
//! use atomic paste placeholders backed by their original text.
//! Recovery escapes a shell or slash command prefix before appending answers, keeping the
//! combined draft editable and its later submission literal. Shell mode's separate `!` becomes
//! text in the composer; slash-prefixed paths remain unchanged because they submit as prompts.
//!
//! # Startup Draft Handoff
//!
//! Startup uses a provisional plain-text composer: editing remains available while its owner
//! records one submit intent. Popups, attachments, and direct dispatch stay disabled. After the
//! protected startup gates finish, `prepare_startup_submission` applies normal submission parsing
//! once, including expansion of paste placeholders. [`ComposerDraftSnapshot`] transfers its
//! text, cursor, pending paste placeholders, local history, and recent activity to the fully
//! initialized composer.
//! `ChatWidget` merges the draft with any existing initial prompt and attachments, rebasing cursor
//! and placeholder positions while preserving both composers' contents. The draft remains deferred
//! until protected views close, input is enabled, and required sandbox setup completes.
//!
//! # Submission and Prompt Expansion
//!
//! Multiline pastes continue blockquote prefixes.
//! `Enter` submits immediately. `Tab` requests queuing while a task is running; if no task is
//! running, `Tab` submits just like Enter so input is never dropped.
//! Vim Replace shares Insert's composer actions; only textarea editing differs.
//! Literal completion/paste edits discard stale Replace recovery; subsequent typing records anew.
//! Token markers added during Replace are removed when restoring overwritten characters.
//! `Tab` does not submit when entering a `!` shell command.
//!
//! On submit/queue paths, the composer:
//!
//! - Expands pending paste placeholders so element ranges align with the final text.
//! - Trims whitespace and rebases elements when `trim_submission` is enabled (the default).
//!   Otherwise, preserves both.
//! - Treats a leading `!` revealed only by paste expansion as literal model input, not shell input.
//! - Prunes local attached images so only placeholders that survive expansion are sent.
//! - Preserves remote image URLs as separate attachments even when text is empty.
//!
//! When these paths clear the visible textarea after a successful submit or slash-command
//! dispatch, they intentionally preserve the textarea kill buffer. That lets users `Ctrl+K` part
//! of a draft, perform a composer action such as changing reasoning level, and then `Ctrl+Y` the
//! killed text back into the now-empty draft. Replacing the chat widget carries that buffer into
//! the fresh composer so Vim yanks survive `/new` and thread switches.
//!
//! The numeric auto-submit path used by the slash popup performs the same pending-paste expansion
//! and attachment pruning, and clears pending paste state on success.
//! Slash commands with arguments (like `/plan` and `/review`) reuse the same preparation path so
//! pasted content and text elements are preserved when extracting args.
//! Commands requiring dispatch validation retain their draft and defer busy-state checks to
//! `ChatWidget`, which has the thread state needed to accept or reject the command.
//!
//! # Parent-Owned Thread Mode
//!
//! Parent-owned subagent threads keep the draft editable while blocking agent-directed submission.
//! On the `Enter` and `Tab` submission paths, normal prompts, disallowed slash commands, and `!`
//! shell commands return `ParentOwnedInputBlocked` without clearing the draft. Bare local and
//! navigation slash commands remain available so users can leave or manage the view. Transcript
//! exports also remain available, including an explicit destination filename.
//!
//! During reconnection, `handle_restricted_key` edits the draft directly without popup dispatch,
//! composer shortcuts, or submission; `?` becomes literal input. Enter and Tab leave the draft
//! intact until reconnection succeeds, except for the local `/warnings` command.
//! When connected but the thread is unavailable, configured
//! submit keys may dispatch explicitly allowed recovery and local commands; other drafts stay put.
//! Empty-prompt agents navigation remains available with its usual editor and draft guards.
//! Collapsed pastes expand into editable text so the full draft can be copied before quitting.
//!
//! # Reasoning Effort Animations
//!
//! The composer observes the effective reasoning tier whenever model-dependent surfaces refresh.
//! The first observation, session configuration, and restored threads establish a baseline;
//! genuine changes to Max/Ultra can then queue composer and status-line transitions when motion
//! and color support allow them.
//! Repeated selections do not restart either effect, dropping below Max clears them, and
//! restoration clears any transition queued while replaying the saved session. Rendering advances
//! active transitions through the frame requester until they finish.
//!
//! # Astra Sparkle Eligibility
//!
//! A confirmed new task can offer one flourish when it starts with Astra or the user switches to
//! Astra through `/model` before entering ordinary prompt content. The draft tracks whether it is
//! untouched, contains a recognized slash command, or is permanently dismissed. An eligible slash
//! command keeps the opportunity while being typed, dispatched, or erased; a fresh Astra response
//! can arm the effect while the command is still pending. Stars stay hidden until the composer is
//! empty. Commands that insert draft content, ordinary typed, pasted, or restored drafts,
//! attachments, and real work dismiss the opportunity even if that content is later cleared.
//! Ordinary disconnected edits also dismiss it immediately, even when erased before rendering;
//! a recognized command typed and erased offline still preserves the opportunity.
//! Opening history search, typing or pasting its query, and canceling it with `Esc` or `Ctrl+C`
//! temporarily hide an active field without restarting its deadline. Any buffered keys are
//! integrated into the original draft without being classified again. A preview does not count
//! as a draft until the user accepts a nonempty match.
//! Toggling shortcut help on an empty composer also hides an active field until help closes,
//! without restarting its deadline, and leaves an unused opportunity intact. A literal `?` is
//! ordinary prompt content.
//! Startup draft snapshots carry this distinction so erased ordinary input does not become
//! untouched on handoff.
//!
//! Sparkle key and paste handling runs only for input routed to the composer; ordinary characters
//! are observed before the paste-burst buffer can hide the first edit. An edit that flushes and
//! erases a held slash also clears the tracked command. An empty main-composer `Esc`
//! can stop the active flourish. Once shown, it fades out by 15 seconds; command entry, popups, and
//! focus loss hide the field without restarting that deadline. Rendering runs after the textarea,
//! placeholder, effort ignition, and voice strip, and draws only in eligible blank cells without
//! overwriting the placeholder or normal cursor. Hidden frames do not schedule animation redraws;
//! motion settings, the starfield preference, and true-color support also gate the effect.
//!
//! # Large Paste Placeholders
//!
//! Large pastes insert an element placeholder in the buffer and store the full text in
//! `pending_pastes`. The placeholder label is derived from the pasted character count:
//!
//! - First paste of a given size uses `[Pasted Content N chars]`.
//! - Additional pending pastes of the same size add a numeric suffix (`#2`, `#3`, ...), where the
//!   next suffix is computed from the placeholders that still exist in `pending_pastes`.
//! - When all placeholders for a size are cleared or deleted, the next paste of that size reuses
//!   the base label without a suffix.
//!
//! # Remote Image Rows (Up/Down/Delete)
//!
//! Remote image URLs are rendered as non-editable `[Image #N]` rows above the textarea (inside the
//! same composer block). These rows represent image attachments rehydrated from app-server/backtrack
//! history; TUI users can remove them, but cannot type into that row region.
//!
//! Keyboard behavior:
//!
//! - `Up` at textarea cursor `0` enters remote-row selection at the last remote image.
//! - `Up`/`Down` move selection between remote rows.
//! - `Down` on the last row clears selection and returns control to the textarea.
//! - `Delete`/`Backspace` remove the selected remote image row.
//!
//! Placeholder numbering is unified across remote and local images:
//!
//! - Remote rows occupy `[Image #1]..[Image #M]`.
//! - Local placeholders are offset after that range (`[Image #M+1]..`).
//! - Deleting a remote row relabels local placeholders to keep numbering contiguous.
//!
//! # Non-bracketed Paste Bursts
//!
//! Some terminals, especially Windows, deliver pastes as rapid `KeyCode::Char`/`Enter`/`Tab` events.
//! [`PasteBurst`](super::paste_burst::PasteBurst) buffers text-producing keys (plain, Shift, or
//! Windows AltGr) and integrates buffered text through the composer, avoiding transient shortcut UI.
//! It briefly holds the first ASCII char to detect a burst. Non-ASCII chars appear immediately
//! for IME responsiveness, but still participate in burst detection.
//! Parent views must keep flushing editors that lose focus while input is buffered; a hidden
//! editor's pending burst can otherwise keep the shared draw loop waiting indefinitely.
//!
//! Submission flushes expired characters and buffers before classifying Enter, independent of
//! UI flush ticks. `disable_paste_burst` bypasses detection; setting it flushes and clears in-flight state.
//! Mouse edits flush pending typing; selection and copy behavior lives in [`mouse`]. Confirmed
//! explicit copies clear the selection while preserving the draft and cursor. Footer text uses a separate
//! read-only selection owner over its final rendered cells.
//!
//! See `codex-rs/tui/src/bottom_pane/paste_burst.rs` for the detailed state machine.
//!
//! # PasteBurst Integration Points
//!
//! The burst detector is consulted in a few specific places:
//!
//! - [`ChatComposer::handle_input_basic`]: flushes any due burst first, then intercepts plain char
//!   input to either buffer it or insert normally.
//! - [`ChatComposer::handle_non_ascii_char`]: handles the non-ASCII/IME path without holding the
//!   first char, while still allowing paste detection via retro-capture.
//! - Unmodified Tab joins detected bursts, including short Unicode prefixes, before popup dispatch.
//!   Expired bursts are flushed first so manual Tab keeps its normal shortcut behavior.
//! - [`ChatComposer::flush_paste_burst_if_due`]/[`ChatComposer::handle_paste_burst_flush`]: called
//!   from UI ticks to integrate a pending burst via [`ChatComposer::apply_paste`] or insert a
//!   held single character. Explicit paste events (terminal or clipboard) use
//!   [`ChatComposer::handle_paste`], which first commits buffered typing before classifying the
//!   pasted text. This preserves the order when a paste arrives before the next burst-flush tick;
//!   buffered keys have already been classified and go directly through `apply_paste`.
//! - Startup recovery previews held keys with the live Vim Replace policy without consuming them.
//!   Buffered pastes remain literal in recovery so large-paste and image classification stays with
//!   the live paste path.
//!
//! # Input Disabled Mode
//!
//! The composer can be temporarily read-only (`input_enabled = false`). In that mode it ignores
//! edits and renders a placeholder prompt instead of the editable textarea. This is part of the
//! overall state machine, since it affects which transitions are even possible from a given UI
//! state.
//!
use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::key_hint::ShortcutHint;
use crate::key_hint::has_ctrl_or_alt;
use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::ui_consts::FOOTER_INDENT_COLS;
use codex_message_history::HistoryBatchCursor; // 008-REVIVE（shim 已建）
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Margin;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::Widget;
// 008-TRIM: use ratatui::widgets::WidgetRef;（弹窗 render_ref，008 §1 裁剪：无本地事件源）

// 008-TRIM: use codex_protocol::openai_models::ReasoningEffort;（effort 动画，008 §1 裁剪）

use super::chat_composer_history::ChatComposerHistory;
use super::chat_composer_history::HistoryEntry;
use super::chat_composer_history::HistoryEntryResponse;
use super::chat_composer_history::HistorySearchResult;
// 008-TRIM: use super::command_popup::CommandItem;（裁剪功能，008 §1）
// 008-TRIM: use super::effort_ignition::EffortIgnition;（裁剪功能，008 §1）
// 008-TRIM: use super::effort_ignition::EffortTier;（裁剪功能，008 §1）
// 008-TRIM: use super::effort_ignition::IGNITION_FRAME_TICK;（裁剪功能，008 §1）
// 008-TRIM: use super::effort_ignition::IgnitionStyle;（裁剪功能，008 §1）
// 008-TRIM: use super::effort_status_line::EFFORT_STATUS_LINE_FRAME_TICK;（裁剪功能，008 §1）
// 008-TRIM: use super::effort_status_line::EffortStatusLineTransition;（裁剪功能，008 §1）
// 008-TRIM: use super::file_search_popup::FileSearchPopup;（裁剪功能，008 §1）
use super::footer::CollaborationModeIndicator;
use super::footer::FooterMode;
use super::footer::FooterProps;
use super::footer::GoalStatusIndicator;
use super::footer::SummaryLeft;
use super::footer::can_show_left_with_context;
use super::footer::context_window_line;
use super::footer::esc_hint_mode;
use super::footer::footer_height;
use super::footer::footer_hint_items_width;
use super::footer::footer_line_width;
use super::footer::inset_footer_hint_area;
use super::footer::max_left_width_for_right;
use super::footer::passive_footer_status_line;
use super::footer::render_context_right;
use super::footer::render_footer_from_props;
use super::footer::render_footer_hint_items;
use super::footer::render_footer_line;
use super::footer::reset_mode_after_activity;
use super::footer::side_conversation_context_line;
use super::footer::single_line_footer_layout;
use super::footer::status_line_right_indicator_line;
use super::footer::toggle_shortcut_mode;
use super::footer::uses_passive_footer_status_layout;
// 008-TRIM: use super::mentions_v2::MentionV2Popup;（裁剪功能，008 §1）
// 008-TRIM: use super::mentions_v2::MentionV2Selection;（裁剪功能，008 §1）
use super::paste_burst::CharDecision;
use super::paste_burst::PasteBurst;
use super::prompt_args::parse_slash_name;
// 008-TRIM: use super::skill_popup::MentionItem;（裁剪功能，008 §1）
// 008-TRIM: use super::skill_popup::SkillPopup;（裁剪功能，008 §1）
use super::slash_commands::ServiceTierCommand;
use super::slash_commands::SlashCommandItem;
// 008-TRIM: use super::voice_strip::VoiceStrip;（裁剪功能，008 §1）
use crate::history_cell::sanitize_user_text;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::EditorKeymap;
use crate::keymap::KeymapContext;
use crate::keymap::KeymapContextSet;
use crate::keymap::RuntimeKeymap;
use crate::keymap::VimNormalKeymap;
use crate::keymap::user_bindings;
// 008-TRIM: use crate::onboarding::mark_underlined_hyperlink;（裁剪功能，008 §1）
use crate::render::Insets;
use crate::render::RectExt;
use crate::render::renderable::Renderable;
use crate::slash_command::SlashCommand;
use crate::style::user_message_style;
use codex_protocol::ThreadId;
use codex_protocol::user_input::ByteRange;
use codex_protocol::user_input::MAX_USER_INPUT_TEXT_CHARS;
use codex_protocol::user_input::TextElement;

mod agents_navigation;
mod attachment_state;
mod completion_target;
mod composer_layout;
mod draft_state;
mod footer_state;
mod history_search;
mod inline_input;
mod mouse;
pub(crate) use mouse::ClipboardPasteTarget;
mod paste_input;
mod popup_state;
mod reconnect;
pub(crate) use reconnect::RestrictedInputMode;
mod slash_input;
mod sparkle;
mod status_surface;
mod vim_history;
mod vim_search;
mod warning_notice;

use self::attachment_state::AttachmentState;
use self::draft_state::ComposerMentionBinding;
use self::draft_state::DraftState;
pub(crate) use self::footer_state::CommandPopupPlacement;
use self::footer_state::ComposerLayout;
pub(crate) use self::footer_state::ComposerRenderOptions;
use self::footer_state::FooterState;
pub(crate) use self::footer_state::TranscriptFooter;
use self::history_search::HistorySearchSession;
use self::popup_state::ActivePopup;
// 008-TRIM: use self::popup_state::DismissedToken;（弹窗 Esc 关闭标记，008 §1 裁剪）
use self::popup_state::PopupState;
use self::slash_input::SlashInput;
use self::slash_input::SlashValidation;
use self::slash_input::SubmissionValidation;
use self::vim_history::VimHistory;
use crate::app_event::AppEvent;
// 008-TRIM: use crate::app_event::ConnectorsSnapshot;（裁剪功能，008 §1）
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::LocalImageAttachment;
use crate::bottom_pane::MentionBinding;
use crate::bottom_pane::textarea::KillBufferSnapshot;
use crate::bottom_pane::textarea::TextArea;
use crate::clipboard_paste::normalize_pasted_path;
use crate::clipboard_paste::pasted_image_format;
use crate::history_cell;
// 008-TRIM: use crate::skills_helpers::skill_display_name;（裁剪功能，008 §1）
use crate::tui::FrameRequester;
use crate::ui_consts::LIVE_PREFIX_COLS;
// 008-TRIM: #[cfg(test)] use codex_app_server_protocol::SkillInterface;（裁剪功能，008 §1）
// 008-TRIM: use codex_app_server_protocol::SkillMetadata;（裁剪功能，008 §1）
// 008-TRIM: use codex_connectors::AppInfo;（裁剪功能，008 §1）
// 008-TRIM: use codex_file_search::FileMatch;（裁剪功能，008 §1）
// 008-TRIM(孤儿属性随被裁声明一并移除): #[cfg(test)]
// 008-TRIM: use codex_plugin::AppConnectorId;（裁剪功能，008 §1）
// 008-TRIM: use codex_plugin::PluginCapabilitySummary;（裁剪功能，008 §1）
// 008-TRIM: use std::cell::OnceCell;（mention 目录惰性构建，008 §1 裁剪：无本地事件源）
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use ratatui::style::Color;

/// If the pasted content exceeds this number of characters, replace it with a
/// placeholder in the UI.
const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;

fn user_input_too_large_message(actual_chars: usize) -> String {
    format!(
        "Message exceeds the maximum length of {MAX_USER_INPUT_TEXT_CHARS} characters ({actual_chars} provided)."
    )
}

/// Result returned when the user interacts with the text area.
#[derive(Debug, PartialEq)]
pub enum InputResult {
    Submitted {
        text: String,
        text_elements: Vec<TextElement>,
    },
    Queued {
        text: String,
        text_elements: Vec<TextElement>,
        action: QueuedInputAction,
        pending_pastes: Vec<(String, String)>,
    },
    /// A bare slash command parsed by the composer.
    ///
    /// Callers that dispatch this variant are also responsible for resolving any pending local
    /// command-history entry that the composer staged before clearing the visible input.
    Command(SlashCommand),
    /// A bare model service-tier command parsed by the composer.
    ServiceTierCommand(ServiceTierCommand),
    /// An inline slash command and its trimmed argument text.
    ///
    /// The `TextElement` ranges are rebased into the argument string, while any pending local
    /// command-history entry still represents the original command invocation that should be
    /// committed only if dispatch accepts it.
    CommandWithArgs(SlashCommand, String, Vec<TextElement>),
    /// Agent-directed input was attempted while viewing a parent-owned spawned child thread.
    ParentOwnedInputBlocked,
    None,
}

fn parent_owned_command_is_allowed(command: SlashCommand, args: &str) -> bool {
    if command == SlashCommand::Export {
        return true;
    }

    args.is_empty()
        && matches!(
            command,
            SlashCommand::Feedback
                | SlashCommand::New
                | SlashCommand::Clear
                | SlashCommand::Resume
                | SlashCommand::App
                | SlashCommand::Side
                | SlashCommand::Btw
                | SlashCommand::Agents
                | SlashCommand::MultiAgents
                | SlashCommand::Vim
                | SlashCommand::Keymap
                | SlashCommand::Config
                | SlashCommand::ElevateSandbox
                | SlashCommand::Experimental
                | SlashCommand::Memories
                | SlashCommand::Quit
                | SlashCommand::Exit
                | SlashCommand::Logout
                | SlashCommand::Copy
                | SlashCommand::Raw
                | SlashCommand::Diff
                | SlashCommand::Mention
                | SlashCommand::Skills
                | SlashCommand::Import
                | SlashCommand::Hooks
                | SlashCommand::Status
                | SlashCommand::Warnings
                | SlashCommand::Daemon
                | SlashCommand::Usage
                | SlashCommand::Ide
                | SlashCommand::DebugConfig
                | SlashCommand::Title
                | SlashCommand::Statusline
                | SlashCommand::Theme
                | SlashCommand::Pets
                | SlashCommand::Ps
                | SlashCommand::Stop
                | SlashCommand::MemoryDrop
                | SlashCommand::MemoryUpdate
                | SlashCommand::Mcp
                | SlashCommand::Apps
                | SlashCommand::Plugins
                | SlashCommand::Rollout
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedInputAction {
    Plain,
    /// Preserve model-input provenance when paste expansion reveals a leading shell sigil.
    Literal,
    ParseSlash,
    RunShell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingPasteHandling {
    Expand,
    Preserve,
}

/// Shared composer behavior; defaults match the main chat input.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChatComposerConfig {
    /// Whether command/file/skill popups are allowed to appear.
    pub(crate) popups_enabled: bool,
    /// Whether `/...` input is parsed and dispatched as slash commands.
    pub(crate) slash_commands_enabled: bool,
    /// Whether a leading `!` switches the editor into shell mode.
    pub(crate) shell_commands_enabled: bool,
    /// Whether pasting a file path can attach local images.
    pub(crate) image_paste_enabled: bool,
    pub(crate) blockquote_paste_enabled: bool,
    /// Strip leading and trailing whitespace from submissions.
    pub(crate) trim_submission: bool,
    /// Embedded editors reset Vim only when their owner accepts the answer.
    pub(crate) reset_vim_on_submission: bool,
}

impl Default for ChatComposerConfig {
    fn default() -> Self {
        Self {
            popups_enabled: true,
            slash_commands_enabled: true,
            shell_commands_enabled: true,
            image_paste_enabled: true,
            blockquote_paste_enabled: true,
            trim_submission: true,
            reset_vim_on_submission: true,
        }
    }
}

impl ChatComposerConfig {
    /// Text answers support Markdown, without popups, commands, or image attachments.
    pub(crate) const fn plain_text() -> Self {
        Self {
            popups_enabled: false,
            slash_commands_enabled: false,
            shell_commands_enabled: false,
            image_paste_enabled: false,
            blockquote_paste_enabled: true,
            trim_submission: true,
            reset_vim_on_submission: true,
        }
    }
}

pub(crate) struct ChatComposer {
    draft: DraftState,
    popups: PopupState,
    app_event_tx: AppEventSender,
    history: ChatComposerHistory,
    agents_navigation_enabled: bool,
    footer: FooterState,
    has_focus: bool,
    frame_requester: Option<FrameRequester>,
    sparkle: sparkle::Sparkle,
    // 008-TRIM: effort_tier/effort_animation_style/effort_ignition/voice_strip/
    // effort_status_line_transition/effort_observed 字段（effort 动画与 voice，
    // 008 §1 裁剪：无本地事件源）
    luna_reserve_active: bool,
    attachments: AttachmentState,
    placeholder_text: String,
    blocks_direct_input: bool,
    is_task_running: bool,
    queue_submissions: bool,
    /// Slash-command draft staged for local recall after application-level dispatch.
    ///
    /// This slot is intentionally separate from `ChatComposerHistory` so inline slash commands can
    /// prepare their argument text without also double-recording the full command invocation.
    pending_slash_command_history: Option<HistoryEntry>,
    // 008-TRIM: skills/plugins/connectors_snapshot 字段（skill/plugin/connector
    // mention 目录，008 §1 裁剪：无本地事件源）
    task_mentions: Option<Vec<crate::task_mentions::TaskMention>>,
    collaboration_modes_enabled: bool,
    config: ChatComposerConfig,
    connectors_enabled: bool,
    plugins_command_enabled: bool,
    token_activity_command_enabled: bool,
    service_tier_commands_enabled: bool,
    service_tier_commands: Vec<ServiceTierCommand>,
    mentions_v2_enabled: bool,
    goal_command_enabled: bool,
    voice_command_enabled: bool,
    worktrees_enabled: bool,
    windows_degraded_sandbox_active: bool,
    side_conversation_active: bool,
    history_search: Option<HistorySearchSession>,
    vim_history: VimHistory,
    submit_keys: Vec<KeyBinding>,
    queue_keys: Vec<KeyBinding>,
    toggle_shortcuts_keys: Vec<KeyBinding>,
    history_search_previous_keys: Vec<KeyBinding>,
    history_search_next_keys: Vec<KeyBinding>,
    editor_keymap: Arc<EditorKeymap>,
    vim_normal_keymap: VimNormalKeymap,
}

// 008-TRIM: struct MentionCompletionTarget（legacy `$` mention 补全目标，
// 008 §1 裁剪：无本地事件源）

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ComposerDraft {
    pub(super) text: String,
    text_elements: Vec<TextElement>,
    local_image_paths: Vec<PathBuf>,
    remote_image_urls: Vec<String>,
    mention_bindings: Vec<MentionBinding>,
    pending_pastes: Vec<(String, String)>,
    cursor: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ComposerDraftSnapshot {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) text_elements: Vec<TextElement>,
    pub(crate) local_images: Vec<LocalImageAttachment>,
    pub(crate) remote_image_urls: Vec<String>,
    pub(crate) mention_bindings: Vec<MentionBinding>,
    pub(crate) pending_pastes: Vec<(String, String)>,
    pub(crate) startup_local_history: Vec<HistoryEntry>,
    pub(crate) last_composer_activity_at: Option<Instant>,
    pub(crate) sparkle_draft: sparkle::SparkleDraft,
}

impl ChatComposer {
    fn slash_input(&self) -> SlashInput<'_> {
        SlashInput::new(
            self.slash_commands_enabled(),
            self.draft.is_bash_mode,
            self.builtin_command_flags(),
            &self.service_tier_commands,
        )
    }

    pub fn new(
        has_input_focus: bool,
        app_event_tx: AppEventSender,
        enhanced_keys_supported: bool,
        placeholder_text: String,
        disable_paste_burst: bool,
    ) -> Self {
        Self::new_with_config(
            has_input_focus,
            app_event_tx,
            enhanced_keys_supported,
            placeholder_text,
            disable_paste_burst,
            ChatComposerConfig::default(),
        )
    }

    /// Construct a composer with explicit feature gating.
    ///
    /// This enables reuse in contexts like request-user-input where we want
    /// the same visuals and editing behavior without slash commands or popups.
    pub(crate) fn new_with_config(
        has_input_focus: bool,
        app_event_tx: AppEventSender,
        enhanced_keys_supported: bool,
        placeholder_text: String,
        disable_paste_burst: bool,
        config: ChatComposerConfig,
    ) -> Self {
        let use_shift_enter_hint = enhanced_keys_supported;
        let default_keymap = RuntimeKeymap::defaults();
        let default_editor_keymap = default_keymap.editor.clone();
        let default_vim_normal_keymap = default_keymap.vim_normal.clone();

        let mut this = Self {
            draft: DraftState::new(),
            popups: PopupState::default(),
            app_event_tx,
            history: ChatComposerHistory::new(),
            agents_navigation_enabled: false,
            footer: FooterState {
                quit_shortcut_expires_at: None,
                quit_shortcut_key: key_hint::ctrl(KeyCode::Char('c')),
                esc_backtrack_hint: false,
                use_shift_enter_hint,
                mode: FooterMode::ComposerEmpty,
                hint_override: None,
                flash: None,
                context_window_percent: None,
                context_window_used_tokens: None,
                context_window_pending: false,
                collaboration_mode_indicator: None,
                goal_status_indicator: None,
                ide_context_active: false,
                status_line_value: None,
                status_line_hyperlink_url: None,
                status_line_enabled: false,
                side_conversation_context_label: None,
                active_agent_label: None,
                external_editor_key: default_keymap
                    .primary_hint(KeymapContext::Global, "open_external_editor"),
                warning_notice_area: std::cell::Cell::default(),
                selection_revision: std::cell::Cell::default(),
                show_warnings_key: default_keymap
                    .primary_hint(KeymapContext::Global, "open_warnings"),
                show_transcript_key: default_keymap
                    .primary_hint(KeymapContext::Global, "open_transcript"),
                find_transcript_key: default_keymap
                    .primary_hint(KeymapContext::Global, "find_transcript"),
                focus_activity_key: default_keymap
                    .primary_hint(KeymapContext::Global, "focus_activity"),
                insert_newline_key: footer_insert_newline_key(
                    &default_keymap.editor.insert_newline,
                    use_shift_enter_hint,
                )
                .map(ShortcutHint::from),
                queue_key: default_keymap.primary_hint(KeymapContext::Composer, "queue"),
                toggle_shortcuts_key: default_keymap
                    .primary_hint(KeymapContext::Composer, "toggle_shortcuts"),
                history_search_key: default_keymap
                    .primary_hint(KeymapContext::Composer, "history_search_previous"),
                reasoning_down_key: default_keymap
                    .primary_hint(KeymapContext::Chat, "decrease_reasoning_effort"),
                reasoning_up_key: default_keymap
                    .primary_hint(KeymapContext::Chat, "increase_reasoning_effort"),
                toggle_voice_key: default_keymap.primary_hint(KeymapContext::Chat, "toggle_voice"),
            },
            has_focus: has_input_focus,
            frame_requester: None,
            sparkle: sparkle::Sparkle::default(),
            // 008-TRIM: effort/voice 字段初始化（008 §1 裁剪：无本地事件源）
            luna_reserve_active: false,
            attachments: AttachmentState::default(),
            placeholder_text,
            blocks_direct_input: false,
            is_task_running: false,
            queue_submissions: false,
            pending_slash_command_history: None,
            // 008-TRIM: skills/plugins/connectors_snapshot 初始化（008 §1 裁剪：无本地事件源）
            task_mentions: None,
            collaboration_modes_enabled: false,
            config,
            connectors_enabled: false,
            plugins_command_enabled: false,
            token_activity_command_enabled: false,
            service_tier_commands_enabled: false,
            service_tier_commands: Vec::new(),
            mentions_v2_enabled: false,
            goal_command_enabled: false,
            voice_command_enabled: false,
            worktrees_enabled: false,
            windows_degraded_sandbox_active: false,
            side_conversation_active: false,
            history_search: None,
            vim_history: VimHistory::default(),
            submit_keys: vec![key_hint::plain(KeyCode::Enter)],
            queue_keys: vec![key_hint::plain(KeyCode::Tab)],
            toggle_shortcuts_keys: vec![
                key_hint::plain(KeyCode::Char('?')),
                key_hint::shift(KeyCode::Char('?')),
            ],
            history_search_previous_keys: default_keymap.composer.history_search_previous.clone(),
            history_search_next_keys: default_keymap.composer.history_search_next.clone(),
            editor_keymap: default_editor_keymap,
            vim_normal_keymap: default_vim_normal_keymap,
        };
        this.draft.textarea.set_keymap_bindings(&default_keymap);
        // Apply configuration via the setter to keep side-effects centralized.
        this.set_disable_paste_burst(disable_paste_burst);
        this
    }

    pub(crate) fn set_frame_requester(&mut self, frame_requester: FrameRequester) {
        self.frame_requester = Some(frame_requester);
    }

    // 008-TRIM: fn set_active_reasoning_effort / set_active_reasoning_effort_baseline
    // （effort 动画，008 §1 裁剪：无本地事件源）
    // 008-TRIM: fn set_skill_mentions / set_plugin_mentions（skill/plugin mention
    // 目录，008 §1 裁剪：无本地事件源）

    pub(crate) fn set_task_mentions_enabled(&mut self, enabled: bool) {
        self.task_mentions = enabled.then(Vec::new);
        // 008-TRIM: refresh_mentions_v2_popup_candidates 调用与 MentionV2 弹窗重查
        // StartFileSearch 分支（mention 弹窗与 file search，008 §1 裁剪：无本地事件源）
    }

    pub(crate) fn task_mentions_enabled(&self) -> bool {
        self.task_mentions.is_some()
    }

    // 008-TRIM: fn on_task_search_result（裁剪功能方法，008 §1）

    /// Refreshes an open mention catalog when skill or plugin metadata changes.
    // 008-TRIM: fn refresh_mentions_v2_popup_candidates（裁剪功能方法，008 §1）

    // 008-TRIM: fn set_plugins_command_enabled（裁剪功能方法，008 §1）

    pub fn set_token_activity_command_enabled(&mut self, enabled: bool) {
        self.token_activity_command_enabled = enabled;
    }

    pub fn set_mentions_v2_enabled(&mut self, enabled: bool) {
        self.mentions_v2_enabled = enabled;
        self.history.set_at_mention_restore_enabled(enabled);
        self.sync_popups();
    }

    /// Toggle composer-side image paste handling.
    ///
    /// This only affects whether image-like paste content is converted into attachments; the
    /// `ChatWidget` layer still performs capability checks before images are submitted.
    pub fn set_image_paste_enabled(&mut self, enabled: bool) {
        self.config.image_paste_enabled = enabled;
    }

    // 008-TRIM: fn set_connector_mentions（connector mention 目录，008 §1 裁剪：无本地事件源）

    pub(crate) fn take_mention_bindings(&mut self) -> Vec<MentionBinding> {
        let elements = self.current_mention_elements();
        let mut ordered = Vec::new();
        for (id, sigil, mention) in elements {
            if let Some(binding) = self.draft.mention_bindings.remove(&id)
                && binding.sigil == sigil
                && binding.mention == mention
            {
                ordered.push(MentionBinding {
                    sigil: binding.sigil,
                    mention: binding.mention,
                    path: binding.path,
                });
            }
        }
        self.draft.mention_bindings.clear();
        ordered
    }

    pub fn set_collaboration_modes_enabled(&mut self, enabled: bool) {
        self.collaboration_modes_enabled = enabled;
    }

    pub fn set_connectors_enabled(&mut self, enabled: bool) {
        self.connectors_enabled = enabled;
    }

    pub fn set_service_tier_commands_enabled(&mut self, enabled: bool) {
        self.service_tier_commands_enabled = enabled;
    }

    pub fn set_service_tier_commands(&mut self, commands: Vec<ServiceTierCommand>) {
        self.service_tier_commands = commands;
        self.sync_popups();
    }

    pub fn set_goal_command_enabled(&mut self, enabled: bool) {
        self.goal_command_enabled = enabled;
    }

    pub fn set_voice_command_enabled(&mut self, enabled: bool) {
        self.voice_command_enabled = enabled;
    }

    /// Refresh the event destination when a retained editor survives reconnection.
    #[allow(dead_code, reason = "Used by later layers of the TUI refresh stack.")]
    pub(crate) fn set_app_event_sender(&mut self, app_event_tx: AppEventSender) {
        self.app_event_tx = app_event_tx;
    }

    /// Replace composer, editor, and footer-hint key bindings from one runtime snapshot.
    ///
    /// Submit and queue bindings are cached here because composer dispatch must
    /// check them before generic textarea editing. The embedded textarea receives
    /// the same snapshot's editor bindings so a live remap cannot leave submit
    /// keys updated while cursor/editing keys still use old defaults.
    pub(crate) fn set_keymap_bindings(&mut self, keymap: &RuntimeKeymap) {
        self.submit_keys = keymap.composer.submit.clone();
        self.queue_keys = keymap.composer.queue.clone();
        self.toggle_shortcuts_keys = keymap.composer.toggle_shortcuts.clone();
        self.history_search_previous_keys = keymap.composer.history_search_previous.clone();
        self.history_search_next_keys = keymap.composer.history_search_next.clone();
        self.editor_keymap = keymap.editor.clone();
        self.vim_normal_keymap = keymap.vim_normal.clone();
        self.draft.textarea.set_keymap_bindings(keymap);
        self.footer.external_editor_key =
            keymap.primary_hint(KeymapContext::Global, "open_external_editor");
        self.footer.show_warnings_key = keymap.primary_hint(KeymapContext::Global, "open_warnings");
        self.footer.show_transcript_key =
            keymap.primary_hint(KeymapContext::Global, "open_transcript");
        self.footer.find_transcript_key =
            keymap.primary_hint(KeymapContext::Global, "find_transcript");
        self.footer.focus_activity_key =
            keymap.primary_hint(KeymapContext::Global, "focus_activity");
        self.footer.insert_newline_key =
            match keymap.primary_hint(KeymapContext::Editor, "insert_newline") {
                hint @ Some(ShortcutHint::Chord { .. }) => hint,
                _ => footer_insert_newline_key(
                    &keymap.editor.insert_newline,
                    self.footer.use_shift_enter_hint,
                )
                .map(ShortcutHint::from),
            };
        self.footer.queue_key = keymap.primary_hint(KeymapContext::Composer, "queue");
        self.footer.toggle_shortcuts_key =
            keymap.primary_hint(KeymapContext::Composer, "toggle_shortcuts");
        self.footer.history_search_key =
            keymap.primary_hint(KeymapContext::Composer, "history_search_previous");
        self.footer.reasoning_down_key =
            keymap.primary_hint(KeymapContext::Chat, "decrease_reasoning_effort");
        self.footer.reasoning_up_key =
            keymap.primary_hint(KeymapContext::Chat, "increase_reasoning_effort");
        self.footer.toggle_voice_key = keymap.primary_hint(KeymapContext::Chat, "toggle_voice");
    }

    /// Return the contexts whose handlers can consume the next composer key.
    pub(crate) fn keymap_contexts(&self) -> KeymapContextSet {
        if self.draft.textarea.vim_query().is_some() {
            return KeymapContextSet::new(KeymapContext::Editor);
        }
        if self.history_search.is_some() {
            return KeymapContextSet::new(KeymapContext::Composer);
        }
        if !self.draft.input_enabled || self.popups.active() {
            return KeymapContextSet::default();
        }
        let contexts = self.draft.textarea.keymap_contexts();
        contexts.with(KeymapContext::Composer)
    }

    pub fn set_collaboration_mode_indicator(
        &mut self,
        indicator: Option<CollaborationModeIndicator>,
    ) {
        self.footer.collaboration_mode_indicator = indicator;
    }

    pub fn set_goal_status_indicator(&mut self, indicator: Option<GoalStatusIndicator>) {
        self.footer.goal_status_indicator = indicator;
    }

    pub fn set_ide_context_active(&mut self, active: bool) {
        self.footer.ide_context_active = active;
    }

    pub fn set_side_conversation_active(&mut self, active: bool) {
        self.side_conversation_active = active;
    }

    /// Compatibility shim for tests that still toggle the removed steer mode flag.
    #[cfg(test)]
    pub fn set_steer_enabled(&mut self, _enabled: bool) {}
    /// Centralized feature gating keeps config checks out of call sites.
    fn popups_enabled(&self) -> bool {
        self.config.popups_enabled
    }

    fn slash_commands_enabled(&self) -> bool {
        self.config.slash_commands_enabled
    }

    fn image_paste_enabled(&self) -> bool {
        self.config.image_paste_enabled
    }
    #[cfg(target_os = "windows")]
    pub fn set_windows_degraded_sandbox_active(&mut self, enabled: bool) {
        self.windows_degraded_sandbox_active = enabled;
    }

    pub fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.cursor_pos_with_options(area, ComposerRenderOptions::default())
    }

    /// Returns true if the composer currently contains no user-entered input.
    pub(crate) fn is_empty(&self) -> bool {
        self.draft.textarea.is_empty() && !self.draft.is_bash_mode && self.attachments.is_empty()
    }

    /// Record local persistent-history metadata so the composer can navigate
    /// cross-session history.
    pub(crate) fn set_history_metadata(
        &mut self,
        thread_id: ThreadId,
        log_id: u64,
        entry_count: usize,
    ) {
        self.history.set_metadata(thread_id, log_id, entry_count);
    }

    /// Integrate an asynchronous response to an on-demand history lookup.
    ///
    /// If the entry is present and the offset still matches the active history cursor, the
    /// composer rehydrates the entry immediately. This path intentionally routes through
    /// [`Self::apply_history_entry`] so cursor placement remains aligned with keyboard history
    /// recall semantics.
    pub(crate) fn on_history_entry_response(
        &mut self,
        log_id: u64,
        offset: usize,
        entry: Option<String>,
    ) -> bool {
        match self
            .history
            .on_entry_response(log_id, offset, entry, &self.app_event_tx)
        {
            HistoryEntryResponse::Found(entry) => {
                // Persistent ↑/↓ history is text-only (backwards-compatible and avoids persisting
                // attachments), but local in-session ↑/↓ history can rehydrate elements and image paths.
                self.apply_history_entry(entry);
                true
            }
            HistoryEntryResponse::Search(result) => {
                self.apply_history_search_result(result);
                true
            }
            HistoryEntryResponse::Ignored => false,
        }
    }

    pub(crate) fn on_history_batch_response(
        &mut self,
        log_id: u64,
        cursor: HistoryBatchCursor,
        entries: Vec<crate::app_event::HistoryBatchEntryResponse>,
        next_older_cursor: Option<HistoryBatchCursor>,
    ) -> bool {
        let result = self.history.on_batch_response(
            log_id,
            cursor,
            entries,
            next_older_cursor,
            &self.app_event_tx,
        );
        self.apply_history_batch_result(result)
    }

    /// Applies a failed batch lookup without conflating it with history exhaustion.
    ///
    /// The history state machine either schedules a bounded retry, preserves an existing match, or
    /// returns the search UI to its idle draft when no match has been selected yet.
    pub(crate) fn on_history_batch_error(
        &mut self,
        log_id: u64,
        cursor: HistoryBatchCursor,
    ) -> bool {
        let result = self
            .history
            .on_batch_error(log_id, cursor, &self.app_event_tx);
        self.apply_history_batch_result(result)
    }

    fn apply_history_batch_result(&mut self, result: Option<HistorySearchResult>) -> bool {
        let Some(result) = result else {
            return false;
        };
        self.apply_history_search_result(result);
        true
    }

    pub(crate) fn record_replayed_user_message_history(&mut self, entry: HistoryEntry) {
        self.history.record_replayed_submission(entry);
    }

    pub fn handle_paste_image_path(&mut self, pasted: &str) -> bool {
        let Some(path_buf) = normalize_pasted_path(pasted) else {
            return false;
        };

        // normalize_pasted_path already handles Windows → WSL path conversion,
        // so we can directly check the pasted path's image extension.
        // 008-TRIM: image crate 未随 008 依赖底座（D4）引入，无法解码校验尺寸；
        // 退化为扩展名判定（png/jpg/jpeg/gif/webp），非图片扩展名按普通文本粘贴。
        if !Self::is_image_path(&path_buf.to_string_lossy()) {
            return false;
        }
        let format = pasted_image_format(&path_buf);
        tracing::info!("OK: {pasted}");
        tracing::debug!("attached image format={}", format.label());
        self.attach_image(path_buf);
        true
    }

    /// Replace the composer content with text from an external editor.
    /// Clears pending paste placeholders and keeps only attachments whose
    /// placeholder labels still appear in the new text. Image placeholders
    /// are renumbered to `[Image #M+1]..[Image #N]` (where `M` is the number of
    /// remote images). Cursor is placed at the end after rebuilding elements.
    pub(crate) fn apply_external_edit(&mut self, text: String) {
        self.note_sparkle_replaced_text(&text);
        self.vim_history = VimHistory::default();
        self.draft.pending_pastes.clear();
        let (text, _) = self.imported_text_for_textarea(text, Vec::new());

        // Count placeholder occurrences in the new text.
        let mut placeholder_counts: HashMap<String, usize> = HashMap::new();
        for placeholder in self
            .attachments
            .local_images
            .iter()
            .map(|image| &image.placeholder)
        {
            if placeholder_counts.contains_key(placeholder) {
                continue;
            }
            let count = text.match_indices(placeholder).count();
            if count > 0 {
                placeholder_counts.insert(placeholder.clone(), count);
            }
        }

        // Keep attachments only while we have matching occurrences left.
        let mut kept_images = Vec::new();
        for img in self.attachments.local_images.drain(..) {
            if let Some(count) = placeholder_counts.get_mut(&img.placeholder)
                && *count > 0
            {
                *count -= 1;
                kept_images.push(img);
            }
        }
        self.attachments.local_images = kept_images;

        // Import literally so placeholders remain atomic and Replace recovery starts empty.
        self.draft.textarea_state.get_mut().follow_cursor();
        self.draft.textarea.set_text_clearing_elements("");
        let mut remaining: HashMap<&str, usize> = HashMap::new();
        for img in &self.attachments.local_images {
            *remaining.entry(img.placeholder.as_str()).or_insert(0) += 1;
        }

        let mut occurrences: Vec<(usize, &str)> = Vec::new();
        for placeholder in remaining.keys() {
            for (pos, _) in text.match_indices(placeholder) {
                occurrences.push((pos, *placeholder));
            }
        }
        occurrences.sort_unstable_by_key(|(pos, _)| *pos);

        let mut idx = 0usize;
        for (pos, ph) in occurrences {
            let Some(count) = remaining.get_mut(ph) else {
                continue;
            };
            if *count == 0 {
                continue;
            }
            if pos > idx {
                self.draft.textarea.insert_str_at(idx, &text[idx..pos]);
            }
            self.draft.textarea.insert_element(ph);
            *count -= 1;
            idx = pos + ph.len();
        }
        if idx < text.len() {
            self.draft.textarea.insert_str_at(idx, &text[idx..]);
        }

        // Keep local image placeholders normalized in attachment order after the
        // remote-image prefix.
        self.attachments
            .relabel_local_images(&mut self.draft.textarea);
        self.draft
            .textarea
            .set_cursor(self.draft.textarea.text().len());
        self.sync_popups();
    }

    /// Enable or disable Vim editing for the composer textarea.
    ///
    /// The composer flushes buffered typing and clears paste-burst state when the mode
    /// changes because Vim normal mode treats rapid character sequences as
    /// commands, not as candidate literal paste text. It also resets transient
    /// footer mode so the visible hints match the new editing surface.
    pub(crate) fn set_vim_enabled(&mut self, enabled: bool) {
        if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
            self.apply_paste(pasted);
        }
        self.draft.textarea.enable_vim_search();
        self.draft.textarea.set_vim_enabled(enabled);
        self.vim_history = VimHistory::default();
        self.draft.paste_burst.clear_after_explicit_paste();
        self.footer.mode = reset_mode_after_activity(self.footer.mode);
    }

    /// Enable Vim while keeping already-active text entry in insert mode.
    pub(crate) fn enable_vim_in_insert_mode(&mut self) {
        self.set_vim_enabled(/*enabled*/ true);
        self.resume_text_entry();
    }

    /// Resume text entry after a parent view takes focus, preserving Vim undo history.
    pub(crate) fn resume_text_entry(&mut self) {
        self.draft.textarea.enter_vim_insert_mode();
    }

    pub(crate) fn take_kill_buffer_snapshot(&mut self) -> KillBufferSnapshot {
        self.draft.textarea.take_kill_buffer_snapshot()
    }

    pub(crate) fn restore_kill_buffer_snapshot(&mut self, snapshot: KillBufferSnapshot) {
        self.draft.textarea.restore_kill_buffer_snapshot(snapshot);
    }

    /// Restore draft history transferred from the startup composer.
    pub(crate) fn restore_startup_local_history(
        &mut self,
        startup_local_history: Vec<HistoryEntry>,
    ) {
        for entry in startup_local_history {
            self.history.record_local_submission(entry);
        }
    }

    /// Toggle Vim editing and return the new enabled state.
    ///
    /// This is the app-level command target for the configurable Vim toggle
    /// keybinding; callers should use the returned value for status messages
    /// instead of rereading state after additional composer mutations.
    pub(crate) fn toggle_vim_enabled(&mut self) -> bool {
        let enabled = !self.draft.textarea.is_vim_enabled();
        self.set_vim_enabled(enabled);
        enabled
    }

    /// Return whether Vim editing is enabled.
    pub(crate) fn is_vim_enabled(&self) -> bool {
        self.draft.textarea.is_vim_enabled()
    }

    /// Return whether Escape should be routed to the textarea before popups.
    ///
    /// Vim insert mode owns Escape as a transition back to normal mode. The app
    /// event layer asks this before running generic Escape behavior so the same
    /// key does not both leave insert mode and dismiss unrelated UI.
    pub(crate) fn should_handle_vim_insert_escape(&self, key_event: KeyEvent) -> bool {
        self.draft
            .textarea
            .should_handle_vim_insert_escape(key_event)
    }

    pub(crate) fn vim_mode_indicator_span(&self) -> Option<Span<'static>> {
        self.draft.textarea.vim_mode_indicator_span()
    }

    fn mode_indicator_line(&self, show_cycle_hint: bool) -> Option<Line<'static>> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let Some(vim_mode) = self.vim_mode_indicator_span() {
            spans.push(vim_mode);
        }
        if let Some(indicators) = status_line_right_indicator_line(
            self.footer.collaboration_mode_indicator,
            self.footer.goal_status_indicator.as_ref(),
            self.footer.ide_context_active,
            show_cycle_hint,
        ) {
            if !spans.is_empty() {
                spans.push(" | ".dim());
            }
            spans.extend(indicators.spans);
        }
        if spans.is_empty() {
            None
        } else {
            Some(Line::from(spans))
        }
    }

    fn right_footer_line_with_context(&self) -> Line<'static> {
        let mut line = if self.footer.context_window_pending {
            Line::default()
        } else {
            context_window_line(
                self.footer.context_window_percent,
                self.footer.context_window_used_tokens,
            )
        };
        if let Some(vim_mode) = self.vim_mode_indicator_span() {
            line.spans.push(" | ".dim());
            line.spans.push(vim_mode);
        }
        line
    }

    pub(crate) fn current_text_with_pending(&self) -> String {
        let text = self.current_text();
        if self.draft.pending_pastes.is_empty() {
            return text;
        }

        let (text, _) = Self::expand_pending_pastes(
            &text,
            self.current_text_elements(),
            &self.draft.pending_pastes,
        );
        text
    }

    /// Returns whether the composer currently accepts interactive draft edits.
    pub(crate) fn input_enabled(&self) -> bool {
        self.draft.input_enabled
    }

    pub(crate) fn pending_pastes(&self) -> Vec<(String, String)> {
        self.draft.pending_pastes.clone()
    }

    pub(crate) fn set_pending_pastes(&mut self, pending_pastes: Vec<(String, String)>) {
        let text = self.current_text();
        self.draft.pending_pastes = pending_pastes
            .into_iter()
            .filter(|(placeholder, _)| text.contains(placeholder))
            .collect();
    }

    pub(crate) fn set_remote_image_urls(&mut self, urls: Vec<String>) {
        if !self.sparkle.history_preview && !urls.is_empty() {
            self.dismiss_sparkle();
        }
        self.attachments
            .set_remote_image_urls(urls, &mut self.draft.textarea);
        self.sync_popups();
    }

    pub(crate) fn remote_image_urls(&self) -> Vec<String> {
        self.attachments.remote_image_urls()
    }

    pub(crate) fn take_remote_image_urls(&mut self) -> Vec<String> {
        let urls = self
            .attachments
            .take_remote_image_urls(&mut self.draft.textarea);
        self.sync_popups();
        urls
    }

    /// Replace the entire composer content with `text` and reset cursor.
    ///
    /// This is the "fresh draft" path: it discards active history search and
    /// clears pending paste payloads and mention link targets. Callers restoring
    /// a previously submitted draft that must keep sigiled mention target
    /// resolution should use
    /// [`Self::set_text_content_with_mention_bindings`] instead.
    /// 008-ADAPT：命令派发后清空草稿（对齐母本 popup 派发清空行为）。
    pub fn clear_for_command_dispatch(&mut self) {
        self.draft.textarea.set_text_clearing_elements("");
        self.draft.is_bash_mode = false;
    }

    pub(crate) fn set_text_content(
        &mut self,
        text: String,
        text_elements: Vec<TextElement>,
        local_image_paths: Vec<PathBuf>,
    ) {
        if self.history_search.take().is_some() {
            self.history.reset_navigation();
            self.footer.mode = reset_mode_after_activity(self.footer.mode);
        }
        self.set_text_content_with_mention_bindings(
            text,
            text_elements,
            local_image_paths,
            Vec::new(),
        );
    }

    /// Restore draft content; clear pending input and undo history. The cursor starts at zero.
    pub(crate) fn set_text_content_with_mention_bindings(
        &mut self,
        text: String,
        text_elements: Vec<TextElement>,
        local_image_paths: Vec<PathBuf>,
        mention_bindings: Vec<MentionBinding>,
    ) {
        if !self.sparkle.history_preview {
            if local_image_paths.is_empty() {
                self.note_sparkle_replaced_text(&text);
            } else {
                self.dismiss_sparkle();
            }
        }
        // Clear any existing content, placeholders, and attachments first.
        self.footer.flash = None;
        self.vim_history = VimHistory::default();
        self.draft.textarea_state.get_mut().follow_cursor();
        self.draft.textarea.set_text_clearing_elements("");
        self.draft.is_bash_mode = false;
        self.draft.pending_pastes.clear();
        self.draft.mention_bindings.clear();

        let (text, text_elements) = self.imported_text_for_textarea(text, text_elements);
        self.draft
            .textarea
            .set_text_with_elements(&text, &text_elements);
        self.attachments
            .reset_local_images(local_image_paths, &mut self.draft.textarea);

        self.bind_mentions_from_snapshot(mention_bindings);
        self.draft.textarea.set_cursor(/*pos*/ 0);
        self.sync_popups();
    }

    pub(crate) fn current_cursor(&self) -> usize {
        self.draft.textarea.cursor() + if self.draft.is_bash_mode { 1 } else { 0 }
    }

    #[cfg(test)]
    pub(crate) fn cursor(&self) -> usize {
        self.current_cursor()
    }

    fn history_navigation_cursor(&self) -> usize {
        if self.draft.is_bash_mode && self.draft.textarea.cursor() == 0 {
            0
        } else if self.draft.textarea.is_vim_normal_mode()
            && !self.draft.textarea.text().is_empty()
            && self.draft.textarea.cursor() == self.draft.textarea.vim_normal_end_cursor()
        {
            self.current_text().len()
        } else {
            self.current_cursor()
        }
    }

    pub(crate) fn set_current_cursor(&mut self, cursor: usize) {
        let visible_cursor = if self.draft.is_bash_mode {
            cursor.saturating_sub(1)
        } else {
            cursor
        };
        self.draft
            .textarea
            .set_cursor(visible_cursor.min(self.draft.textarea.text().len()));
    }

    fn current_text_elements(&self) -> Vec<TextElement> {
        let shift = if self.draft.is_bash_mode { 1 } else { 0 };
        self.draft
            .textarea
            .text_elements()
            .into_iter()
            .filter_map(|element| Self::shift_text_element(element, shift))
            .collect()
    }

    fn shift_text_element(element: TextElement, shift: isize) -> Option<TextElement> {
        let start = element.byte_range.start.checked_add_signed(shift)?;
        let end = element.byte_range.end.checked_add_signed(shift)?;
        if start >= end {
            return None;
        }

        Some(element.map_range(|_| (start..end).into()))
    }

    pub(super) fn snapshot_draft(&self) -> ComposerDraft {
        ComposerDraft {
            text: self.current_text(),
            text_elements: self.current_text_elements(),
            local_image_paths: self.attachments.local_image_paths(),
            remote_image_urls: self.attachments.remote_image_urls(),
            mention_bindings: self.mention_bindings(),
            pending_pastes: self.draft.pending_pastes.clone(),
            cursor: self.current_cursor(),
        }
    }

    pub(super) fn restore_draft(&mut self, draft: ComposerDraft) {
        let ComposerDraft {
            text,
            text_elements,
            local_image_paths,
            remote_image_urls,
            mention_bindings,
            pending_pastes,
            cursor,
        } = draft;
        self.set_remote_image_urls(remote_image_urls);
        self.set_text_content_with_mention_bindings(
            text,
            text_elements,
            local_image_paths,
            mention_bindings,
        );
        self.set_pending_pastes(pending_pastes);
        self.set_current_cursor(cursor);
        self.sync_popups();
    }

    /// Update the placeholder text without changing input enablement.
    pub(crate) fn set_placeholder_text(&mut self, placeholder: String) {
        self.placeholder_text = placeholder;
    }

    pub(crate) fn set_parent_owned_thread(&mut self) {
        self.blocks_direct_input = true;
        self.placeholder_text = "Viewing sub-agent — direct input is disabled".to_string();
    }

    /// Move the cursor to the end of the current text buffer.
    pub(crate) fn move_cursor_to_end(&mut self) {
        self.draft
            .textarea
            .set_cursor(self.draft.textarea.text().len());
        self.sync_popups();
    }

    fn move_cursor_to_history_entry_end(&mut self) {
        let cursor = if self.draft.textarea.is_vim_normal_mode() {
            self.draft.textarea.vim_normal_end_cursor()
        } else {
            self.draft.textarea.text().len()
        };
        self.draft.textarea.set_cursor(cursor);
        self.sync_popups();
    }

    /// Convert canonical composer text into the textarea's internal representation.
    ///
    /// Shell mode stores the leading `!` as prompt state instead of editable text,
    /// so full-buffer imports must absorb that prefix before rebuilding the textarea.
    fn imported_text_for_textarea(
        &mut self,
        text: String,
        text_elements: Vec<TextElement>,
    ) -> (String, Vec<TextElement>) {
        if self.config.shell_commands_enabled
            && let Some(stripped) = text.strip_prefix('!')
        {
            self.draft.is_bash_mode = true;
            (
                stripped.to_string(),
                text_elements
                    .into_iter()
                    .filter_map(|element| Self::shift_text_element(element, /*shift*/ -1))
                    .collect(),
            )
        } else {
            self.draft.is_bash_mode = false;
            (text, text_elements)
        }
    }

    /// Flush buffered typing before clearing so cancellation preserves the complete draft in history.
    pub(crate) fn clear_for_ctrl_c(&mut self) -> Option<String> {
        if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
            self.apply_paste(pasted);
        }
        if self.is_empty() {
            return None;
        }
        let previous = self.current_text();
        let text_elements = self.current_text_elements();
        let local_image_paths = self.attachments.local_image_paths();
        let pending_pastes = std::mem::take(&mut self.draft.pending_pastes);
        let remote_image_urls = self.attachments.remote_image_urls();
        let mention_bindings = self.snapshot_mention_bindings();
        self.set_text_content(String::new(), Vec::new(), Vec::new());
        self.attachments.clear_remote_image_urls();
        self.history.reset_navigation();
        self.history.record_local_submission(HistoryEntry {
            text: previous.clone(),
            text_elements,
            local_image_paths,
            remote_image_urls,
            mention_bindings,
            pending_pastes,
        });
        Some(previous)
    }

    /// Get the current composer text.
    pub(crate) fn current_text(&self) -> String {
        if self.draft.is_bash_mode {
            format!("!{}", self.draft.textarea.text())
        } else {
            self.draft.textarea.text().to_string()
        }
    }

    /// Recall content at its history boundary, preserving pending pastes and omitting images
    /// in plain-text editors.
    fn apply_history_entry(&mut self, entry: HistoryEntry) {
        let HistoryEntry {
            text,
            text_elements,
            local_image_paths,
            remote_image_urls,
            mention_bindings,
            pending_pastes,
        } = entry;
        self.set_remote_image_urls(remote_image_urls);
        self.set_text_content_with_mention_bindings(
            text,
            text_elements,
            local_image_paths,
            mention_bindings,
        );
        self.set_pending_pastes(pending_pastes);
        if !self.config.image_paste_enabled {
            for image in self.attachments.local_images() {
                if let Some(element) = self.draft.textarea.text_elements().into_iter().find(|e| {
                    e.placeholder(self.draft.textarea.text()) == Some(image.placeholder.as_str())
                }) {
                    let range = element.byte_range;
                    self.draft
                        .textarea
                        .replace_range(range.start..range.end, "");
                }
            }
            self.attachments = AttachmentState::default();
        }
        self.history.record_recalled_text(self.current_text());
        self.move_cursor_to_history_entry_end();
    }

    pub(crate) fn text_elements(&self) -> Vec<TextElement> {
        self.current_text_elements()
    }

    #[cfg(test)]
    pub(crate) fn local_image_paths(&self) -> Vec<PathBuf> {
        self.attachments.local_image_paths()
    }

    #[cfg(test)]
    pub(crate) fn status_line_text(&self) -> Option<String> {
        self.footer.status_line_text()
    }

    pub(crate) fn local_images(&self) -> Vec<LocalImageAttachment> {
        self.attachments.local_images()
    }

    pub(crate) fn mention_bindings(&self) -> Vec<MentionBinding> {
        self.snapshot_mention_bindings()
    }

    pub(crate) fn take_recent_submission_mention_bindings(&mut self) -> Vec<MentionBinding> {
        std::mem::take(&mut self.draft.recent_submission_mention_bindings)
    }

    /// Commit the staged slash-command draft to local Up-arrow recall.
    ///
    /// Call this after command dispatch. Calling it more than once is harmless because the pending
    /// slot is consumed on the first call.
    pub(crate) fn record_pending_slash_command_history(&mut self) {
        if let Some(entry) = self.pending_slash_command_history.take() {
            self.history.record_local_submission(entry);
        }
    }

    /// Insert an attachment placeholder and track it for the next submission.
    pub fn attach_image(&mut self, path: PathBuf) {
        self.dismiss_sparkle();
        let started_vim_edit = self.begin_direct_vim_edit();
        let elements_before = self.draft.textarea.element_payloads();
        self.attachments
            .attach_image(&mut self.draft.textarea, path);
        self.reconcile_deleted_elements(elements_before);
        if started_vim_edit {
            self.finish_vim_edit();
        }
    }

    #[cfg(test)]
    pub fn take_recent_submission_images(&mut self) -> Vec<PathBuf> {
        self.attachments.take_recent_submission_images()
    }

    pub fn take_recent_submission_images_with_placeholders(&mut self) -> Vec<LocalImageAttachment> {
        self.attachments
            .take_recent_submission_images_with_placeholders()
    }

    /// Flushes any due paste-burst state.
    ///
    /// Call this from a UI tick to turn paste-burst transient state into explicit textarea edits:
    ///
    /// - If a burst times out, integrate it via [`Self::apply_paste`]. Its key events have already
    ///   been classified; [`Self::handle_paste`] classifies explicit terminal pastes instead.
    /// - If only the first ASCII char was held (flicker suppression) and no burst followed, emit it
    ///   as normal typed input.
    ///
    /// This also allows a single "held" ASCII char to render even when it turns out not to be part
    /// of a paste burst.
    pub(crate) fn flush_paste_burst_if_due(&mut self) -> bool {
        self.handle_paste_burst_flush(tokio::time::Instant::now().into_std())
    }

    /// Returns whether the composer is currently in any paste-burst related transient state.
    ///
    /// This includes actively buffering, having a non-empty burst buffer, or holding the first
    /// ASCII char for flicker suppression.
    pub(crate) fn is_in_paste_burst(&self) -> bool {
        self.draft.paste_burst.is_active()
    }

    /// Returns a delay that reliably exceeds the paste-burst timing threshold.
    ///
    /// Use this in tests to avoid boundary flakiness around the `PasteBurst` timeout.
    pub(crate) fn recommended_paste_flush_delay() -> Duration {
        PasteBurst::recommended_flush_delay()
    }

    // 008-TRIM: fn on_file_search_result（file search 弹窗，008 §1 裁剪：无本地事件源）

    fn next_large_paste_placeholder(&self, char_count: usize) -> String {
        let base = format!("[Pasted Content {char_count} chars]");
        let prefix = format!("{base} #");
        let mut max_suffix = 0usize;

        for (placeholder, _) in &self.draft.pending_pastes {
            if placeholder == &base {
                max_suffix = max_suffix.max(1);
                continue;
            }
            if let Some(suffix) = placeholder.strip_prefix(&prefix)
                && let Ok(value) = suffix.parse::<usize>()
            {
                max_suffix = max_suffix.max(value);
            }
        }

        if max_suffix == 0 {
            base
        } else {
            format!("{base} #{}", max_suffix + 1)
        }
    }

    /// Handle a key event coming from the main UI.
    pub fn handle_key_event(&mut self, key_event: KeyEvent) -> (InputResult, bool) {
        if !self.draft.input_enabled {
            return (InputResult::None, false);
        }

        if matches!(key_event.kind, KeyEventKind::Release) {
            return (InputResult::None, false);
        }

        if self.history_search.is_none()
            && !self.popups.active()
            && self.config.slash_commands_enabled
            && self.is_empty()
            && self.draft.textarea.wants_vim_search_key(key_event)
            && !self.draft.textarea.is_vim_operator_pending()
            && key_event.code == KeyCode::Char('/')
            && key_event.modifiers.is_empty()
        {
            self.draft.textarea.enter_vim_insert_mode();
        }

        self.draft.textarea_state.get_mut().follow_cursor();
        let before = self.before_sparkle_key(key_event);
        let result = self.handle_key_event_inner(key_event);
        self.after_sparkle_key(before, &result.0);
        result
    }

    fn handle_key_event_inner(&mut self, key_event: KeyEvent) -> (InputResult, bool) {
        if self.history_search.is_none()
            && !self.popups.active()
            && self.draft.textarea.wants_vim_search_key(key_event)
        {
            return self.handle_input_basic(key_event);
        }

        if self.history_search.is_some() {
            return self.handle_history_search_key(key_event);
        }

        if self.handle_vim_history_key(key_event) {
            return (InputResult::None, true);
        }

        if Self::is_history_search_key(&key_event, &self.history_search_previous_keys) {
            return self.begin_history_search();
        }

        if self.handle_paste_tab(key_event, tokio::time::Instant::now().into_std()) {
            return (InputResult::None, true);
        }

        // 008-TRIM: Command/File/Skill/MentionV2 弹窗键路由分支（mention/file/command
        // 弹窗，008 §1 裁剪：无本地事件源）；ActivePopup 仅余 None 变体。
        let result = self.handle_key_event_without_popup(key_event);
        self.reset_vim_mode_after_successful_dispatch(&result.0);
        // Update (or hide/show) popup after processing the key.
        self.sync_popups();
        result
    }

    /// Whether a popup or query owns input.
    pub(crate) fn popup_active(&self) -> bool {
        self.history_search.is_some()
            || self.draft.textarea.vim_query().is_some()
            || self.popups.active()
    }

    #[inline]
    fn clamp_to_char_boundary(text: &str, pos: usize) -> usize {
        let mut p = pos.min(text.len());
        if p < text.len() && !text.is_char_boundary(p) {
            p = text
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= p)
                .last()
                .unwrap_or(0);
        }
        p
    }

    /// Handle non-ASCII character input (often IME) while still supporting paste-burst detection.
    ///
    /// This handler exists because non-ASCII input often comes from IMEs, where characters can
    /// legitimately arrive in short bursts that should **not** be treated as paste.
    ///
    /// The key differences from the ASCII path:
    ///
    /// - We never hold the first character (`PasteBurst::on_plain_char_no_hold`), because holding a
    ///   non-ASCII char can feel like dropped input.
    /// - If a burst is detected, we may need to retroactively remove already-inserted text before
    ///   the cursor and move it into the paste buffer (see `PasteBurst::decide_begin_buffer`).
    ///
    /// Because this path mixes "insert immediately" with "maybe retro-grab later", it must clamp
    /// the cursor to a UTF-8 char boundary before slicing `textarea.text()`.
    #[inline]
    fn handle_non_ascii_char(&mut self, input: KeyEvent, now: Instant) -> (InputResult, bool) {
        if self.draft.disable_paste_burst {
            // When burst detection is disabled, treat IME/non-ASCII input as normal typing.
            // In particular, do not retro-capture or buffer already-inserted prefix text.
            self.draft.textarea.input(input);
            let text_after = self.draft.textarea.text();
            self.draft
                .pending_pastes
                .retain(|(placeholder, _)| text_after.contains(placeholder));
            return (InputResult::None, true);
        }
        if let KeyEvent {
            code: KeyCode::Char(ch),
            ..
        } = input
        {
            if self.draft.paste_burst.try_append_char_if_active(ch, now) {
                return (InputResult::None, true);
            }
            // Non-ASCII input often comes from IMEs and can arrive in quick bursts.
            // We do not want to hold the first char (flicker suppression) on this path, but we
            // still want to detect paste-like bursts. Before applying any non-ASCII input, flush
            // any existing burst buffer (including a pending first char from the ASCII path) so
            // we don't carry that transient state forward.
            if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
                self.apply_paste(pasted);
            }
            if let Some(decision) = self.draft.paste_burst.on_plain_char_no_hold(now) {
                match decision {
                    CharDecision::BufferAppend => {
                        self.draft.paste_burst.append_char_to_buffer(ch, now);
                        return (InputResult::None, true);
                    }
                    CharDecision::BeginBuffer { retro_chars } => {
                        // For non-ASCII we inserted prior chars immediately, so if this turns out
                        // to be paste-like we need to retroactively grab & remove the already-
                        // inserted prefix from the textarea before buffering the burst.
                        let cur = self.draft.textarea.cursor();
                        let txt = self.draft.textarea.text();
                        let safe_cur = Self::clamp_to_char_boundary(txt, cur);
                        let before = &txt[..safe_cur];
                        if let Some(grab) = self.draft.paste_burst.decide_begin_buffer(
                            now,
                            before,
                            retro_chars as usize,
                        ) {
                            if grab.grabbed.is_empty()
                                || self.draft.textarea.retract_paste_burst(grab.start_byte)
                            {
                                self.draft.paste_burst.append_char_to_buffer(ch, now);
                                return (InputResult::None, true);
                            }
                            self.draft.paste_burst.clear_after_explicit_paste();
                        }
                        // If decide_begin_buffer opted not to start buffering,
                        // fall through to normal insertion below.
                    }
                    _ => unreachable!("on_plain_char_no_hold returned unexpected variant"),
                }
            }
        }
        if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
            self.apply_paste(pasted);
        }
        let elements_before = self.draft.textarea.element_payloads();
        self.draft.textarea.input(input);
        self.reconcile_deleted_elements(elements_before);
        (InputResult::None, true)
    }

    // 008-TRIM: fn handle_key_event_with_file_popup / handle_key_event_with_skill_popup /
    // handle_key_event_with_mentions_v2_popup（file search/skill/mention 弹窗键处理，
    // 008 §1 裁剪：无本地事件源；ActivePopup 仅余 None 变体，无弹窗可路由）

    fn is_image_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".gif")
            || lower.ends_with(".webp")
    }

    // 008-TRIM: fn advance_past_completion_separator / dismiss_completed_prefixed_token /
    // insert_selected_file_path（file search/mention 补全插入链，008 §1 裁剪：无本地事件源）

    fn trim_text_elements(
        original: &str,
        trimmed: &str,
        elements: Vec<TextElement>,
    ) -> Vec<TextElement> {
        if trimmed.is_empty() || elements.is_empty() {
            return Vec::new();
        }
        let trimmed_start = original.len().saturating_sub(original.trim_start().len());
        let trimmed_end = trimmed_start.saturating_add(trimmed.len());

        elements
            .into_iter()
            .filter_map(|elem| {
                let start = elem.byte_range.start;
                let end = elem.byte_range.end;
                if end <= trimmed_start || start >= trimmed_end {
                    return None;
                }
                let new_start = start.saturating_sub(trimmed_start);
                let new_end = end.saturating_sub(trimmed_start).min(trimmed.len());
                if new_start >= new_end {
                    return None;
                }
                let placeholder = trimmed.get(new_start..new_end).map(str::to_string);
                Some(TextElement::new(
                    ByteRange {
                        start: new_start,
                        end: new_end,
                    },
                    placeholder,
                ))
            })
            .collect()
    }

    /// Expand large-paste placeholders using element ranges and rebuild other element spans.
    pub(crate) fn expand_pending_pastes(
        text: &str,
        mut elements: Vec<TextElement>,
        pending_pastes: &[(String, String)],
    ) -> (String, Vec<TextElement>) {
        if pending_pastes.is_empty() || elements.is_empty() {
            return (text.to_string(), elements);
        }

        // Stage 1: index pending paste payloads by placeholder for deterministic replacements.
        let mut pending_by_placeholder: HashMap<&str, VecDeque<&str>> = HashMap::new();
        for (placeholder, actual) in pending_pastes {
            pending_by_placeholder
                .entry(placeholder.as_str())
                .or_default()
                .push_back(actual.as_str());
        }

        // Stage 2: walk elements in order and rebuild text/spans in a single pass.
        elements.sort_by_key(|elem| elem.byte_range.start);

        let mut rebuilt = String::with_capacity(text.len());
        let mut rebuilt_elements = Vec::with_capacity(elements.len());
        let mut cursor = 0usize;

        for elem in elements {
            let start = elem.byte_range.start.min(text.len());
            let end = elem.byte_range.end.min(text.len());
            if start > end {
                continue;
            }
            if start > cursor {
                rebuilt.push_str(&text[cursor..start]);
            }
            let elem_text = &text[start..end];
            let placeholder = elem.placeholder(text).map(str::to_string);
            let replacement = placeholder
                .as_deref()
                .and_then(|ph| pending_by_placeholder.get_mut(ph))
                .and_then(VecDeque::pop_front);
            if let Some(actual) = replacement {
                // Stage 3: inline actual paste payloads and drop their placeholder elements.
                rebuilt.push_str(actual);
            } else {
                // Stage 4: keep non-paste elements, updating their byte ranges for the new text.
                let new_start = rebuilt.len();
                rebuilt.push_str(elem_text);
                let new_end = rebuilt.len();
                let placeholder = placeholder.or_else(|| Some(elem_text.to_string()));
                rebuilt_elements.push(TextElement::new(
                    ByteRange {
                        start: new_start,
                        end: new_end,
                    },
                    placeholder,
                ));
            }
            cursor = end;
        }

        // Stage 5: append any trailing text that followed the last element.
        if cursor < text.len() {
            rebuilt.push_str(&text[cursor..]);
        }

        (rebuilt, rebuilt_elements)
    }

    // 008-TRIM: fn skills / fn plugins / fn mentions_enabled（skill/plugin/connector
    // mention 目录，008 §1 裁剪：无本地事件源）

    fn current_prefixed_token(
        textarea: &TextArea,
        prefix: char,
        allow_empty: bool,
    ) -> Option<String> {
        completion_target::current_prefixed_token_range(textarea, prefix, allow_empty)
            .map(|(_, token)| token)
    }

    /// Extract the `@token` that the cursor is currently positioned on, if any.
    ///
    /// The returned string **does not** include the leading `@`.
    fn current_at_token(textarea: &TextArea) -> Option<String> {
        Self::current_prefixed_token(textarea, '@', /*allow_empty*/ false)
    }

    // 008-TRIM: fn current_editable_prefixed_token_range / current_editable_at_token_range_with_options
    // （仅服务于弹窗同步/补全插入，008 §1 裁剪：无本地事件源）

    // 008-TRIM: fn current_mentions_v2_token_range / current_mentions_v2_token /
    // current_mention_target / insert_selected_path / insert_selected_mention /
    // separate_completion_from_adjacent_element（mention/file 补全插入链，
    // 008 §1 裁剪：无本地事件源）

    fn mention_token_from_insert_text(insert_text: &str) -> Option<(char, String)> {
        let sigil = insert_text.chars().next()?;
        if !matches!(sigil, '$' | '@') {
            return None;
        }
        let name = &insert_text[sigil.len_utf8()..];
        if name.is_empty() {
            return None;
        }
        if name
            .as_bytes()
            .iter()
            .all(|byte| is_mention_name_char(*byte))
        {
            Some((sigil, name.to_string()))
        } else {
            None
        }
    }

    fn current_mention_elements(&self) -> Vec<(u64, char, String)> {
        self.draft
            .textarea
            .text_element_snapshots()
            .into_iter()
            .filter_map(|snapshot| {
                if let Some(binding) = self.draft.mention_bindings.get(&snapshot.id)
                    && crate::task_mentions::valid_thread_path(&binding.path).is_some()
                    && snapshot.text == format!("{}{}", binding.sigil, binding.mention)
                {
                    return Some((snapshot.id, binding.sigil, binding.mention.clone()));
                }
                Self::mention_token_from_insert_text(snapshot.text.as_str())
                    .map(|(sigil, mention)| (snapshot.id, sigil, mention))
            })
            .collect()
    }

    fn snapshot_mention_bindings(&self) -> Vec<MentionBinding> {
        let mut ordered = Vec::new();
        for (id, sigil, mention) in self.current_mention_elements() {
            if let Some(binding) = self.draft.mention_bindings.get(&id)
                && binding.sigil == sigil
                && binding.mention == mention
            {
                ordered.push(MentionBinding {
                    sigil: binding.sigil,
                    mention: binding.mention.clone(),
                    path: binding.path.clone(),
                });
            }
        }
        ordered
    }

    fn bind_mentions_from_snapshot(&mut self, mention_bindings: Vec<MentionBinding>) {
        self.draft.mention_bindings.clear();
        if mention_bindings.is_empty() {
            return;
        }

        let text = self.draft.textarea.text().to_string();
        let mut scan_from = 0usize;
        for binding in mention_bindings {
            let token = format!("{}{}", binding.sigil, binding.mention);
            let range = if crate::task_mentions::valid_thread_path(&binding.path).is_some() {
                self.draft
                    .textarea
                    .text_element_snapshots()
                    .into_iter()
                    .find(|element| element.range.start >= scan_from && element.text == token)
                    .map(|element| element.range)
            } else {
                find_next_mention_token_range(text.as_str(), token.as_str(), scan_from)
            };
            let Some(range) = range else {
                continue;
            };

            let id = if let Some(id) = self.draft.textarea.add_element_range(range.clone()) {
                Some(id)
            } else {
                self.draft
                    .textarea
                    .element_id_for_exact_range(range.clone())
            };

            if let Some(id) = id {
                self.draft.mention_bindings.insert(
                    id,
                    ComposerMentionBinding {
                        sigil: binding.sigil,
                        mention: binding.mention,
                        path: binding.path,
                    },
                );
                scan_from = range.end;
            }
        }
    }

    // 008-TRIM: fn plugin_at_mention_highlights（plugin mention 高亮，008 §1 裁剪：无本地事件源）

    /// Prepare text for submission/queuing. Returns None if submission should be suppressed.
    /// On success, clears pending paste payloads because placeholders have been expanded.
    ///
    /// When `record_history` is true, the final submission is stored for ↑/↓ recall.
    fn prepare_submission_text(
        &mut self,
        record_history: bool,
    ) -> Option<(String, Vec<TextElement>)> {
        self.prepare_submission_text_with_options(
            record_history,
            SlashValidation::Immediate,
            PendingPasteHandling::Expand,
        )
    }

    fn prepare_submission_text_with_options(
        &mut self,
        record_history: bool,
        slash_validation: SlashValidation,
        pending_paste_handling: PendingPasteHandling,
    ) -> Option<(String, Vec<TextElement>)> {
        let mut text = self.current_text();
        let original_input = text.clone();
        let original_text_elements = self.current_text_elements();
        let original_mention_bindings = self.snapshot_mention_bindings();
        let original_local_image_paths = self.attachments.local_image_paths();
        let original_pending_pastes = self.draft.pending_pastes.clone();
        let mut text_elements = original_text_elements.clone();
        let input_starts_with_space = original_input.starts_with(' ');
        self.draft.recent_submission_mention_bindings.clear();
        self.draft.textarea.set_text_clearing_elements("");
        self.draft.is_bash_mode = false;

        if pending_paste_handling == PendingPasteHandling::Expand
            && !self.draft.pending_pastes.is_empty()
        {
            // Expand placeholders so element byte ranges stay aligned.
            let (expanded, expanded_elements) =
                Self::expand_pending_pastes(&text, text_elements, &self.draft.pending_pastes);
            text = expanded;
            text_elements = expanded_elements;
        }

        if self.config.trim_submission {
            let expanded_input = text.clone();
            text = text.trim().to_string();
            text_elements = Self::trim_text_elements(&expanded_input, &text, text_elements);
        }

        if slash_validation == SlashValidation::Immediate
            && let SubmissionValidation::UnknownCommand(name) = self
                .slash_input()
                .validate_submission(&text, input_starts_with_space)
        {
            let message = format!(
                r#"Unrecognized command '/{name}'. Type "/" for a list of supported commands."#
            );
            self.app_event_tx.send(AppEvent::InsertHistoryCell(Box::new(
                history_cell::new_info_event(message, /*hint*/ None),
            )));
            self.set_text_content_with_mention_bindings(
                original_input.clone(),
                original_text_elements,
                original_local_image_paths,
                original_mention_bindings,
            );
            self.draft
                .pending_pastes
                .clone_from(&original_pending_pastes);
            self.draft.textarea.set_cursor(original_input.len());
            return None;
        }

        let actual_chars = text.chars().count();
        if actual_chars > MAX_USER_INPUT_TEXT_CHARS {
            let message = user_input_too_large_message(actual_chars);
            self.app_event_tx.send(AppEvent::InsertHistoryCell(Box::new(
                history_cell::new_error_event(message),
            )));
            self.set_text_content_with_mention_bindings(
                original_input.clone(),
                original_text_elements,
                original_local_image_paths,
                original_mention_bindings,
            );
            self.draft
                .pending_pastes
                .clone_from(&original_pending_pastes);
            self.draft.textarea.set_cursor(original_input.len());
            // Embedded composers may cover the transcript that receives the full error.
            if self.footer.hint_override.is_some() {
                self.show_footer_flash(
                    Line::from(
                        format!("Message too long; limit {MAX_USER_INPUT_TEXT_CHARS} characters")
                            .red(),
                    ),
                    Duration::from_secs(5),
                );
            }
            return None;
        }
        self.attachments
            .prune_local_images_for_submission(&text, &text_elements);
        if text.trim().is_empty() && self.attachments.is_empty() {
            return None;
        }
        self.draft.recent_submission_mention_bindings = original_mention_bindings.clone();
        if record_history && (!text.is_empty() || !self.attachments.is_empty()) {
            let preserve_literal_paste = self.slash_commands_enabled()
                && text.starts_with('!')
                && !original_input.trim_start().starts_with('!');
            let (history_text, history_text_elements) = if preserve_literal_paste {
                let history_text = original_input.trim().to_string();
                let history_text_elements = Self::trim_text_elements(
                    &original_input,
                    &history_text,
                    original_text_elements,
                );
                (history_text, history_text_elements)
            } else {
                (text.clone(), text_elements.clone())
            };
            self.history.record_local_submission(HistoryEntry {
                text: history_text,
                text_elements: history_text_elements,
                local_image_paths: self.attachments.local_image_paths(),
                remote_image_urls: self.attachments.remote_image_urls(),
                mention_bindings: original_mention_bindings,
                pending_pastes: if pending_paste_handling == PendingPasteHandling::Preserve
                    || preserve_literal_paste
                {
                    original_pending_pastes.clone()
                } else {
                    Vec::new()
                },
            });
        }
        self.draft.pending_pastes.clear();
        Some((text, text_elements))
    }

    /// Common logic for handling message submission/queuing.
    /// Returns the appropriate InputResult based on `should_queue`.
    fn handle_submission(&mut self, should_queue: bool) -> (InputResult, bool) {
        let result =
            self.handle_submission_with_time(should_queue, tokio::time::Instant::now().into_std());
        self.reset_vim_mode_after_successful_dispatch(&result.0);
        result
    }

    /// Prepare a startup-confirmed draft through the normal queue parser, independent of keymaps.
    pub(super) fn prepare_startup_submission(&mut self) -> InputResult {
        // Startup's plain-text editor cannot show shell mode; require confirmation here.
        if self.is_bang_shell_command() {
            return InputResult::None;
        }
        self.handle_submission(/*should_queue*/ true).0
    }

    fn reset_vim_mode_after_successful_dispatch(&mut self, result: &InputResult) {
        if matches!(
            result,
            InputResult::Submitted { .. }
                | InputResult::Queued { .. }
                | InputResult::Command(_)
                | InputResult::ServiceTierCommand(_)
                | InputResult::CommandWithArgs(_, _, _)
        ) && self.config.reset_vim_on_submission
        {
            self.reset_vim_mode();
        }
    }

    fn handle_submission_with_time(
        &mut self,
        should_queue: bool,
        now: Instant,
    ) -> (InputResult, bool) {
        self.handle_paste_burst_flush(now);

        if !should_queue && self.handle_paste_enter(now) {
            return (InputResult::None, true);
        }

        if let Some(result) = self.handle_parent_owned_submission() {
            return result;
        }
        if should_queue {
            if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
                self.apply_paste(pasted);
            }
            let visible_shell_command = self.is_bang_shell_command();
            let original_input = self.current_text();
            let original_text_elements = self.current_text_elements();
            let original_pending_pastes = self.draft.pending_pastes.clone();
            let raw_text = self.draft.textarea.text();
            let defer_slash_validation = self.slash_input().should_parse_on_dequeue(raw_text);
            let preserve_pending_pastes = defer_slash_validation
                && !self.draft.pending_pastes.is_empty()
                && parse_slash_name(raw_text)
                    .is_some_and(|(name, _, _)| name == SlashCommand::Goal.command());
            let pending_pastes = if preserve_pending_pastes {
                self.draft.pending_pastes.clone()
            } else {
                Vec::new()
            };
            if let Some((text, text_elements)) = self.prepare_submission_text_with_options(
                /*record_history*/ true,
                if defer_slash_validation {
                    SlashValidation::Deferred
                } else {
                    SlashValidation::Immediate
                },
                if preserve_pending_pastes {
                    PendingPasteHandling::Preserve
                } else {
                    PendingPasteHandling::Expand
                },
            ) {
                let action = if text.starts_with('!') && !visible_shell_command {
                    QueuedInputAction::Literal
                } else {
                    slash_input::queued_input_action(&text, defer_slash_validation)
                };
                let (text, text_elements, pending_pastes) = if action == QueuedInputAction::Literal
                {
                    let text = original_input.trim().to_string();
                    let text_elements =
                        Self::trim_text_elements(&original_input, &text, original_text_elements);
                    (text, text_elements, original_pending_pastes)
                } else {
                    (text, text_elements, pending_pastes)
                };
                return (
                    InputResult::Queued {
                        text,
                        text_elements,
                        action,
                        pending_pastes,
                    },
                    true,
                );
            }
            return (InputResult::None, true);
        }

        // If the first line is a bare built-in slash command (no args),
        // dispatch it even when the slash popup isn't visible. This preserves
        // the workflow: type a prefix ("/di"), press Tab to complete to
        // "/diff ", then press Enter/Ctrl+Shift+Q to run it. Tab moves the cursor beyond
        // the '/name' token and our caret-based heuristic hides the popup,
        // but Enter/Ctrl+Shift+Q should still dispatch the command rather than submit
        // literal text.
        if let Some(result) = self.try_dispatch_bare_slash_command() {
            return (result, true);
        }

        let original_input = self.current_text();
        let original_text_elements = self.current_text_elements();
        let original_mention_bindings = self.snapshot_mention_bindings();
        let original_local_image_paths = self.attachments.local_image_paths();
        let original_pending_pastes = self.draft.pending_pastes.clone();
        if let Some(result) = self.try_dispatch_slash_command_with_args() {
            return (result, true);
        }

        if let Some((text, text_elements)) =
            self.prepare_submission_text(/*record_history*/ true)
        {
            if self.slash_commands_enabled()
                && text.starts_with('!')
                && !original_input.trim_start().starts_with('!')
            {
                let text = original_input.trim().to_string();
                let text_elements =
                    Self::trim_text_elements(&original_input, &text, original_text_elements);
                (
                    InputResult::Queued {
                        text,
                        text_elements,
                        action: QueuedInputAction::Literal,
                        pending_pastes: original_pending_pastes,
                    },
                    true,
                )
            } else {
                // Do not clear local attachments here; ChatWidget drains them via
                // take_recent_submission_images().
                (
                    InputResult::Submitted {
                        text,
                        text_elements,
                    },
                    true,
                )
            }
        } else {
            // Restore suppressed input, preserving validation feedback.
            let flash = self.footer.flash.take();
            self.set_text_content_with_mention_bindings(
                original_input,
                original_text_elements,
                original_local_image_paths,
                original_mention_bindings,
            );
            self.draft.pending_pastes = original_pending_pastes;
            self.footer.flash = flash;
            (InputResult::None, true)
        }
    }

    fn handle_parent_owned_submission(&mut self) -> Option<(InputResult, bool)> {
        if !self.blocks_direct_input {
            return None;
        }

        let text = self.current_text();
        let allowed_slash_command = parse_slash_name(&text).is_some_and(|(name, args, _)| {
            matches!(
                self.slash_input().command(name),
                Some(SlashCommandItem::Builtin(command))
                    if name == command.command()
                        && parent_owned_command_is_allowed(command, args)
            )
        });
        if text.starts_with('/') && allowed_slash_command {
            return None;
        }

        Some((InputResult::ParentOwnedInputBlocked, true))
    }

    /// Check if the first line is a bare slash command (no args) and dispatch it.
    /// Returns Some(InputResult) if a command was dispatched, None otherwise.
    fn try_dispatch_bare_slash_command(&mut self) -> Option<InputResult> {
        let command = self
            .slash_input()
            .bare_command(self.draft.textarea.text())?;
        if self.reject_slash_command_if_unavailable(&command) {
            self.stage_slash_command_history(&command);
            self.record_pending_slash_command_history();
            return Some(InputResult::None);
        }
        self.stage_slash_command_history(&command);
        if !matches!(command, SlashCommandItem::Builtin(cmd) if cmd.requires_dispatch_validation())
        {
            self.draft.textarea.set_text_clearing_elements("");
            self.draft.is_bash_mode = false;
        }
        Some(match command {
            SlashCommandItem::Builtin(cmd) => InputResult::Command(cmd),
            SlashCommandItem::ServiceTier(command) => InputResult::ServiceTierCommand(command),
        })
    }

    /// Check if the input is a slash command with args (e.g., /review args) and dispatch it.
    /// Returns Some(InputResult) if a command was dispatched, None otherwise.
    fn try_dispatch_slash_command_with_args(&mut self) -> Option<InputResult> {
        let text = self.draft.textarea.text().to_string();
        let inline_command = self.slash_input().inline_command(&text)?;
        let command = inline_command.command;
        if self.reject_slash_command_if_unavailable(&command) {
            self.stage_slash_command_history(&command);
            self.record_pending_slash_command_history();
            return Some(InputResult::None);
        }

        self.stage_slash_command_history(&command);

        let mut args_elements = slash_input::args_elements(
            inline_command.rest,
            inline_command.rest_offset,
            &self.draft.textarea.text_elements(),
        );
        let trimmed_rest = inline_command.rest.trim();
        args_elements = Self::trim_text_elements(inline_command.rest, trimmed_rest, args_elements);
        let SlashCommandItem::Builtin(cmd) = command else {
            return None;
        };
        Some(InputResult::CommandWithArgs(
            cmd,
            trimmed_rest.to_string(),
            args_elements,
        ))
    }

    /// Expand pending placeholders and extract normalized inline-command args.
    ///
    /// Inline-arg commands are initially dispatched using the raw draft so command rejection does
    /// not consume user input. Once a command needs its args, this helper performs the usual
    /// submission preparation (paste expansion, element trimming) and rebases element ranges from
    /// full-text offsets to command-arg offsets.
    ///
    /// Callers that already staged slash-command history should normally pass `false` for
    /// `record_history`; otherwise a command such as `/plan investigate` would be entered into
    /// local recall through both the slash-command path and the message-submission path.
    pub(crate) fn prepare_inline_args_submission(
        &mut self,
        record_history: bool,
    ) -> Option<(String, Vec<TextElement>)> {
        let (prepared_text, prepared_elements) = self.prepare_submission_text(record_history)?;
        let (prepared_rest, prepared_rest_offset) = slash_input::prepared_args(&prepared_text)?;
        let mut args_elements =
            slash_input::args_elements(prepared_rest, prepared_rest_offset, &prepared_elements);
        let trimmed_rest = prepared_rest.trim();
        args_elements = Self::trim_text_elements(prepared_rest, trimmed_rest, args_elements);
        self.draft.textarea.enter_vim_insert_mode();
        Some((trimmed_rest.to_string(), args_elements))
    }

    fn reject_slash_command_if_unavailable(&self, command: &SlashCommandItem) -> bool {
        if !self.is_task_running
            || command.available_during_task()
            || matches!(command, SlashCommandItem::Builtin(cmd) if cmd.requires_dispatch_validation())
        {
            return false;
        }
        let message = format!(
            "'/{}' is disabled while a task is in progress.",
            command.command()
        );
        self.app_event_tx.send(AppEvent::InsertHistoryCell(Box::new(
            history_cell::new_error_event(message),
        )));
        true
    }

    /// Stage the current slash-command text for later local recall.
    ///
    /// Staging snapshots the rich composer state before the textarea is cleared. `ChatWidget`
    /// commits the staged entry after dispatch so command recall follows the submitted text, not
    /// the command outcome.
    fn stage_slash_command_history(&mut self, command: &SlashCommandItem) {
        if matches!(command, SlashCommandItem::Builtin(SlashCommand::Clear)) {
            return;
        }
        self.stage_slash_command_history_text(self.draft.textarea.text().trim().to_string());
    }

    // 008-TRIM: fn stage_selected_slash_command_history（command popup 选中项历史，
    // 008 §1 裁剪：无本地事件源）

    /// Store the provided command text and the current composer adornments in the pending slot.
    ///
    /// The pending entry intentionally has the same shape as other local history entries so recall
    /// can rehydrate attachments, mention bindings, and pending paste placeholders if command
    /// workflows start carrying those through in the future.
    fn stage_slash_command_history_text(&mut self, text: String) {
        self.pending_slash_command_history = Some(HistoryEntry {
            text,
            text_elements: self.draft.textarea.text_elements(),
            local_image_paths: self.attachments.local_image_paths(),
            remote_image_urls: self.attachments.remote_image_urls(),
            mention_bindings: self.snapshot_mention_bindings(),
            pending_pastes: self.draft.pending_pastes.clone(),
        });
    }

    fn handle_remote_image_selection_key(
        &mut self,
        key_event: &KeyEvent,
    ) -> Option<(InputResult, bool)> {
        if self.draft.textarea.mouse_selection_range().is_some() {
            return None;
        }
        let removes_remote_image = matches!(key_event.code, KeyCode::Delete | KeyCode::Backspace)
            && self.attachments.selected_remote_image_index.is_some();
        let started_vim_edit = removes_remote_image && self.begin_direct_vim_edit();
        let result = self
            .attachments
            .handle_remote_image_selection_key(key_event, &mut self.draft.textarea);
        if started_vim_edit {
            self.finish_vim_edit();
        }
        result
    }

    /// Handle key event when no popup is visible.
    fn handle_key_event_without_popup(&mut self, key_event: KeyEvent) -> (InputResult, bool) {
        if let Some((result, redraw)) = self.handle_remote_image_selection_key(&key_event) {
            return (result, redraw);
        }
        if self.attachments.selected_remote_image_index.is_some() {
            self.attachments.clear_remote_image_selection();
        }
        if self.handle_empty_prompt_shortcut(&key_event) {
            return (InputResult::None, true);
        }
        if self.draft.is_bash_mode && key_event.code == KeyCode::Esc {
            if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
                self.apply_paste(pasted);
            }
            if self.draft.textarea.is_empty() {
                self.draft.is_bash_mode = false;
                return (InputResult::None, true);
            }
        }
        if self.should_handle_vim_insert_escape(key_event) {
            return self.handle_input_basic(key_event);
        }
        if self.draft.textarea.is_vim_normal_mode() && self.draft.textarea.is_vim_operator_pending()
        {
            return self.handle_input_basic(key_event);
        }
        if self.config.shell_commands_enabled
            && self.draft.textarea.is_vim_normal_mode()
            && self.is_empty()
            && matches!(
                key_event,
                KeyEvent {
                    code: KeyCode::Char('!'),
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press | KeyEventKind::Repeat,
                    ..
                }
            )
        {
            self.footer.mode = reset_mode_after_activity(self.footer.mode);
            self.draft.is_bash_mode = true;
            self.draft.textarea.enter_vim_insert_mode();
            return (InputResult::None, true);
        }
        if key_event.code == KeyCode::Esc {
            if self.is_empty() {
                let next_mode = esc_hint_mode(self.footer.mode, self.is_task_running);
                if next_mode != self.footer.mode {
                    self.footer.mode = next_mode;
                    return (InputResult::None, true);
                }
            }
        } else {
            self.footer.mode = reset_mode_after_activity(self.footer.mode);
        }
        if self.queue_keys.is_pressed(key_event)
            && (self.is_task_running || self.queue_submissions || !self.is_bang_shell_command())
        {
            return self.handle_submission(self.is_task_running || self.queue_submissions);
        }

        if self.submit_keys.is_pressed(key_event) {
            return self.handle_submission(self.queue_submissions);
        }

        if let KeyEvent {
            code: KeyCode::Char('d'),
            modifiers: crossterm::event::KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            ..
        } = key_event
            && self.is_empty()
        {
            return (InputResult::None, false);
        }

        let (history_up_pressed, history_down_pressed) = if self.draft.textarea.is_vim_normal_mode()
        {
            if self.draft.textarea.is_vim_operator_pending() {
                (false, false)
            } else {
                (
                    self.vim_normal_keymap.move_up.is_pressed(key_event),
                    self.vim_normal_keymap.move_down.is_pressed(key_event),
                )
            }
        } else {
            (
                self.editor_keymap.move_up.is_pressed(key_event),
                self.editor_keymap.move_down.is_pressed(key_event),
            )
        };
        if history_up_pressed || history_down_pressed {
            if self.draft.textarea.mouse_selection_range().is_none()
                && self.history.should_handle_navigation(
                    &self.current_text(),
                    self.history_navigation_cursor(),
                )
            {
                let replace_entry = if history_up_pressed {
                    self.history.navigate_up(&self.app_event_tx)
                } else {
                    self.history.navigate_down(&self.app_event_tx)
                };
                if let Some(entry) = replace_entry {
                    self.apply_history_entry(entry);
                    return (InputResult::None, true);
                }
            }
            return self.handle_input_basic(key_event);
        }

        self.handle_input_basic(key_event)
    }

    fn is_bang_shell_command(&self) -> bool {
        self.config.shell_commands_enabled && self.current_text().trim_start().starts_with('!')
    }

    fn shell_mode_footer_line(&self) -> Option<Line<'static>> {
        self.is_bang_shell_command()
            .then_some(())
            .map(|_| Line::from(vec![Span::from("Shell mode").light_red()]))
    }

    /// Handles keys that mutate the textarea, including paste-burst detection.
    ///
    /// Acts as the lowest-level keypath for keys that mutate the textarea. It is also where plain
    /// character streams are converted into explicit paste operations on terminals that do not
    /// reliably provide bracketed paste.
    ///
    /// Ordering is important:
    ///
    /// - Always flush any *due* paste burst first so buffered text does not lag behind unrelated
    ///   edits.
    /// - Then handle the incoming key, intercepting only text-producing character input.
    /// - For non-text keys, flush via `flush_before_modified_input()` before applying the key;
    ///   otherwise `clear_window_after_non_char()` can leave buffered text waiting without a
    ///   timestamp to time out against.
    fn handle_input_basic(&mut self, input: KeyEvent) -> (InputResult, bool) {
        // Ignore key releases here to avoid treating them as additional input
        // (e.g., appending the same character twice via paste-burst logic).
        if !matches!(input.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return (InputResult::None, false);
        }

        self.handle_input_basic_with_time(input, tokio::time::Instant::now().into_std())
    }

    fn handle_input_basic_with_time(
        &mut self,
        input: KeyEvent,
        now: Instant,
    ) -> (InputResult, bool) {
        if !self.draft.textarea.is_vim_normal_mode() {
            self.begin_vim_edit(input);
        }
        // If we have a buffered non-bracketed paste burst and enough time has
        // elapsed since the last char, flush it before handling a new input.
        self.handle_paste_burst_flush(now);

        if !matches!(input.code, KeyCode::Esc) {
            self.footer.mode = reset_mode_after_activity(self.footer.mode);
        }

        // If we're capturing a burst and receive Enter, accumulate it instead of inserting.
        if matches!(input.code, KeyCode::Enter)
            && !self.draft.disable_paste_burst
            && self.draft.paste_burst.is_active()
            && self
                .draft
                .paste_burst
                .append_control_char_if_active('\n', now)
        {
            return (InputResult::None, true);
        }

        let has_non_text_modifier = has_ctrl_or_alt(input.modifiers)
            || input
                .modifiers
                .intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META);

        // Intercept text-producing Char inputs to optionally accumulate into a burst buffer.
        //
        // This preserves plain, Shift, and Windows AltGr input while keeping shortcut modifiers
        // out of the burst detector and flushing any in-flight text before a non-text key.
        if let KeyEvent {
            code: KeyCode::Char(ch),
            ..
        } = input
        {
            if !has_non_text_modifier
                && !self.draft.disable_paste_burst
                && self.draft.textarea.allows_paste_burst()
            {
                // Non-ASCII characters (e.g., from IMEs) can arrive in quick bursts, so avoid
                // holding the first char while still allowing burst detection for paste input.
                if !ch.is_ascii() {
                    return self.handle_non_ascii_char(input, now);
                }

                match self.draft.paste_burst.on_plain_char(ch, now) {
                    CharDecision::BufferAppend => {
                        self.draft.paste_burst.append_char_to_buffer(ch, now);
                        return (InputResult::None, true);
                    }
                    CharDecision::BeginBuffer { retro_chars } => {
                        let cur = self.draft.textarea.cursor();
                        let txt = self.draft.textarea.text();
                        let safe_cur = Self::clamp_to_char_boundary(txt, cur);
                        let before = &txt[..safe_cur];
                        if let Some(grab) = self.draft.paste_burst.decide_begin_buffer(
                            now,
                            before,
                            retro_chars as usize,
                        ) {
                            if grab.grabbed.is_empty()
                                || self.draft.textarea.retract_paste_burst(grab.start_byte)
                            {
                                self.draft.paste_burst.append_char_to_buffer(ch, now);
                                return (InputResult::None, true);
                            }
                            self.draft.paste_burst.clear_after_explicit_paste();
                        }
                        // If decide_begin_buffer opted not to start buffering,
                        // fall through to normal insertion below.
                    }
                    CharDecision::BeginBufferFromPending => {
                        // First char was held; now append the current one.
                        self.draft.paste_burst.append_char_to_buffer(ch, now);
                        return (InputResult::None, true);
                    }
                    CharDecision::RetainFirstChar => {
                        // Keep the first fast char pending momentarily.
                        return (InputResult::None, true);
                    }
                }
            }
            if let Some(pasted) = self.draft.paste_burst.flush_before_modified_input() {
                self.apply_paste(pasted);
            }
        }

        // Flush any buffered burst before applying a non-char input (arrow keys, etc).
        //
        // `clear_window_after_non_char()` clears `last_plain_char_time`. If we cleared that while
        // `PasteBurst.buffer` is non-empty, `flush_if_due()` would no longer have a timestamp to
        // time out against, and the buffered paste could remain stuck until another plain char
        // arrives.
        if !matches!(input.code, KeyCode::Char(_) | KeyCode::Enter)
            && let Some(pasted) = self.draft.paste_burst.flush_before_modified_input()
        {
            self.apply_paste(pasted);
        }
        // For non-char inputs (or after flushing), handle normally.
        // Track element removals so we can drop any corresponding placeholders without scanning
        // the full text. (Placeholders are atomic elements; when deleted, the element disappears.)
        let elements_before = if self.draft.pending_pastes.is_empty() && self.attachments.is_empty()
        {
            None
        } else {
            Some(self.draft.textarea.element_payloads())
        };

        if self.draft.is_bash_mode
            && self.draft.textarea.vim_query().is_none()
            && matches!(input.code, KeyCode::Backspace)
            && self.draft.textarea.cursor() == 0
        {
            let started_vim_edit = self.begin_direct_vim_edit();
            self.draft.is_bash_mode = false;
            if started_vim_edit {
                self.finish_vim_edit();
            }
            return (InputResult::None, true);
        }

        self.begin_vim_edit(input);
        self.draft.textarea.input(input);
        self.sync_bash_mode_from_text();

        if let Some(elements_before) = elements_before {
            self.reconcile_deleted_elements(elements_before);
        }
        self.finish_vim_edit();

        // Update the paste-burst heuristic for text, shortcut, and non-char events.
        match input.code {
            KeyCode::Char(_) => {
                if has_non_text_modifier {
                    self.draft.paste_burst.clear_window_after_non_char();
                }
            }
            KeyCode::Enter => {
                // Keep burst window alive (supports blank lines in paste).
            }
            _ => {
                // Other keys: clear burst window (buffer should have been flushed above if needed).
                self.draft.paste_burst.clear_window_after_non_char();
            }
        }

        (InputResult::None, true)
    }

    fn sync_bash_mode_from_text(&mut self) {
        if self.config.shell_commands_enabled
            && !self.draft.is_bash_mode
            && self.draft.textarea.text().starts_with('!')
        {
            self.draft.textarea.replace_range(0..1, "");
            self.draft.is_bash_mode = true;
        }
    }

    fn reconcile_deleted_elements(&mut self, elements_before: Vec<String>) {
        let elements_after: HashSet<String> =
            self.draft.textarea.element_payloads().into_iter().collect();

        let removed_payloads = elements_before
            .into_iter()
            .filter(|payload| !elements_after.contains(payload))
            .collect::<Vec<_>>();
        for removed in &removed_payloads {
            self.draft.pending_pastes.retain(|(ph, _)| ph != removed);
        }
        self.attachments
            .remove_deleted_local_placeholders(&removed_payloads, &mut self.draft.textarea);
    }

    /// Handle empty-prompt agents navigation and shortcut-overlay toggling or dismissal.
    ///
    /// This only toggles when the composer is empty and no paste burst is in
    /// progress, so typing/pasting `?` still inserts text instead of opening
    /// help. The bound key list intentionally supports terminal-variant
    /// modifier reporting (for example `?` vs `shift-?`).
    fn handle_empty_prompt_shortcut(&mut self, key_event: &KeyEvent) -> bool {
        if self.shortcut_overlay_visible() && key_hint::plain(KeyCode::Esc).is_press(*key_event) {
            self.footer.mode = reset_mode_after_activity(self.footer.mode);
            return true;
        }
        if key_event.kind != KeyEventKind::Press {
            return false;
        }

        if self.handle_agents_navigation(*key_event) {
            return true;
        }

        let Some(next) = self.empty_prompt_shortcut_toggle(key_event) else {
            return false;
        };
        self.footer.mode = next;
        true
    }

    fn empty_prompt_shortcut_toggle(&self, key_event: &KeyEvent) -> Option<FooterMode> {
        if key_event.kind == KeyEventKind::Press
            && self.footer.hint_override.is_none()
            && self.toggle_shortcuts_keys.is_pressed(*key_event)
            && self.is_empty()
            && !self.is_in_paste_burst()
        {
            let next = toggle_shortcut_mode(
                self.footer.mode,
                self.quit_shortcut_hint_visible(),
                self.is_empty(),
            );
            (next != self.footer.mode).then_some(next)
        } else {
            None
        }
    }

    pub(crate) fn sync_popups(&mut self) {
        self.sync_slash_command_elements();
        // 008-TRIM: Command/File/Skill/MentionV2 弹窗同步与 StartFileSearch 事件发送
        // （弹窗与 file search，008 §1 裁剪：无本地事件源）。保留的 slash 命令元素化
        // 与历史浏览早退结构不变。
        if self.history_search.is_some() || self.draft.textarea.vim_query().is_some() {
            self.popups.active = ActivePopup::None;
            self.popups.dismissed_file_token = None;
            self.popups.dismissed_mention_token = None;
            return;
        }
        if !self.popups_enabled() || self.draft.textarea.mouse_selection_range().is_some() {
            self.popups.active = ActivePopup::None;
            return;
        }
        let browsing_history = self
            .history
            .should_handle_navigation(&self.current_text(), self.history_navigation_cursor());
        // When browsing input history (shell-style Up/Down recall), skip all popup
        // synchronization so nothing steals focus from continued history navigation.
        if browsing_history {
            self.popups.active = ActivePopup::None;
            return;
        }
        self.popups.dismissed_file_token = None;
        self.popups.dismissed_mention_token = None;
    }

    // 008-TRIM: fn sync_command_popup / sync_file_search_popup / sync_mention_popup /
    // sync_mentions_v2_popup / mention_items / connector_brief_description /
    // connector_description（弹窗同步与 mention 目录构建，008 §1 裁剪：无本地事件源）


    fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    #[allow(dead_code)]
    pub(crate) fn set_input_enabled(&mut self, enabled: bool, placeholder: Option<String>) {
        self.draft.input_enabled = enabled;
        self.draft.input_disabled_placeholder = if enabled { None } else { placeholder };

        // Avoid leaving interactive popups open while input is blocked.
        if !enabled && self.popups.active() {
            self.popups.active = ActivePopup::None;
        }
    }

    pub(crate) fn show_shutdown_in_progress(&mut self) {
        self.set_input_enabled(/*enabled*/ false, Some("Shutting down...".to_string()));
        self.footer.quit_shortcut_expires_at = None;
        self.footer.mode = FooterMode::ComposerEmpty;
        self.footer.hint_override = Some(Vec::new());
        self.footer.flash = None;
    }

    pub fn set_task_running(&mut self, running: bool) {
        self.is_task_running = running;
    }

    pub(crate) fn set_queue_submissions(&mut self, queue_submissions: bool) {
        self.queue_submissions = queue_submissions;
    }

    pub(crate) fn set_context_window(&mut self, percent: Option<i64>, used_tokens: Option<i64>) {
        if self.footer.context_window_percent == percent
            && self.footer.context_window_used_tokens == used_tokens
        {
            return;
        }
        self.footer.context_window_percent = percent;
        self.footer.context_window_used_tokens = used_tokens;
    }

    pub(crate) fn set_context_window_pending(&mut self, pending: bool) {
        self.footer.context_window_pending = pending;
    }

    pub(crate) fn set_esc_backtrack_hint(&mut self, show: bool) {
        self.footer.esc_backtrack_hint = show;
        if show {
            self.footer.mode = esc_hint_mode(self.footer.mode, self.is_task_running);
        } else {
            self.footer.mode = reset_mode_after_activity(self.footer.mode);
        }
    }

    pub(crate) fn set_status_line(&mut self, status_line: Option<Line<'static>>) -> bool {
        if self.footer.status_line_value == status_line {
            return false;
        }
        self.footer.status_line_value = status_line;
        true
    }

    pub(crate) fn set_status_line_hyperlink(&mut self, url: Option<String>) -> bool {
        if self.footer.status_line_hyperlink_url == url {
            return false;
        }
        self.footer.status_line_hyperlink_url = url;
        true
    }

    pub(crate) fn set_status_line_enabled(&mut self, enabled: bool) -> bool {
        if self.footer.status_line_enabled == enabled {
            return false;
        }
        self.footer.status_line_enabled = enabled;
        true
    }

    pub(crate) fn set_side_conversation_context_label(&mut self, label: Option<String>) -> bool {
        if self.footer.side_conversation_context_label == label {
            return false;
        }
        self.footer.side_conversation_context_label = label;
        true
    }

    /// Replaces the contextual footer label for the currently viewed agent.
    ///
    /// Returning `false` means the value was unchanged, so callers can skip redraw work. This
    /// field is intentionally just cached presentation state; `ChatComposer` does not infer which
    /// thread is active on its own.
    pub(crate) fn set_active_agent_label(&mut self, active_agent_label: Option<String>) -> bool {
        if self.footer.active_agent_label == active_agent_label {
            return false;
        }
        self.footer.active_agent_label = active_agent_label;
        true
    }
}

fn footer_insert_newline_key(
    bindings: &[KeyBinding],
    enhanced_keys_supported: bool,
) -> Option<KeyBinding> {
    let bindings = user_bindings(bindings);
    let shift_enter = key_hint::shift(KeyCode::Enter);
    if enhanced_keys_supported && bindings.contains(&shift_enter) {
        return Some(shift_enter);
    }

    let plain_enter = key_hint::plain(KeyCode::Enter);
    bindings
        .iter()
        .copied()
        .find(|binding| *binding != plain_enter)
        .or_else(|| bindings.first().copied())
}

// 008-TRIM: fn skill_description（skill mention 目录，008 §1 裁剪：无本地事件源）

fn is_mention_name_char(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-')
}

fn ends_plaintext_at_mention(bytes: &[u8], index: usize) -> bool {
    bytes.get(index).is_none_or(|byte| {
        byte.is_ascii_whitespace()
            || *byte == b'.'
                && bytes.get(index + 1).is_none_or(|next| {
                    next.is_ascii_whitespace()
                        || !next.is_ascii_alphanumeric() && *next != b'_' && *next != b'-'
                })
            || !matches!(*byte, b'.' | b'/' | b'\\')
                && !byte.is_ascii_alphanumeric()
                && *byte != b'_'
                && *byte != b'-'
    })
}

fn ends_plaintext_at_dollar_mention(bytes: &[u8], index: usize) -> bool {
    bytes
        .get(index)
        .is_none_or(|byte| !is_mention_name_char(*byte))
}

fn starts_plaintext_at_mention(text: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }

    text.get(..index)
        .and_then(|prefix| prefix.chars().next_back())
        .is_some_and(|ch| ch.is_whitespace() || !is_mention_name_char_char(ch))
}

fn is_mention_name_char_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
}

fn find_next_mention_token_range(text: &str, token: &str, from: usize) -> Option<Range<usize>> {
    if token.is_empty() || from >= text.len() {
        return None;
    }
    let bytes = text.as_bytes();
    let token_bytes = token.as_bytes();
    let sigil = *token_bytes.first()?;
    let mut index = from;

    while index < bytes.len() {
        if bytes[index] != sigil {
            index += 1;
            continue;
        }

        let end = index.saturating_add(token_bytes.len());
        if end > bytes.len() {
            return None;
        }
        if &bytes[index..end] != token_bytes {
            index += 1;
            continue;
        }

        // Fix for restored `@` mentions: rebinding must not attach to embedded substrings such
        // as email addresses, while preserving the existing `$` mention matching behavior.
        let starts_plaintext_mention = if sigil == b'@' {
            starts_plaintext_at_mention(text, index)
        } else {
            true
        };
        // Fix for restored `@` mentions: mirror history encoding's trailing boundary so path-like
        // text such as `@sample/pkg` is not rebound as the plain `@sample` mention.
        let ends_plaintext_mention = if sigil == b'@' {
            ends_plaintext_at_mention(bytes, end)
        } else {
            ends_plaintext_at_dollar_mention(bytes, end)
        };

        if starts_plaintext_mention && ends_plaintext_mention {
            return Some(index..end);
        }

        index = end;
    }

    None
}

impl Renderable for ChatComposer {
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.cursor_pos_with_options(area, ComposerRenderOptions::default())
    }

    fn cursor_style(&self, _area: Rect) -> crossterm::cursor::SetCursorStyle {
        if self.draft.textarea.uses_vim_insert_cursor() {
            crossterm::cursor::SetCursorStyle::SteadyBar
        } else {
            crossterm::cursor::SetCursorStyle::DefaultUserShape
        }
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.desired_height_with_options(width, ComposerRenderOptions::default())
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_with_mask(area, buf, /*mask_char*/ None);
    }
}

impl ChatComposer {
    pub(crate) fn set_luna_reserve_active(&mut self, active: bool) -> bool {
        if self.luna_reserve_active == active {
            return false;
        }
        self.luna_reserve_active = active;
        true
    }
}

impl ChatComposer {
    pub(crate) fn render_with_mask(&self, area: Rect, buf: &mut Buffer, mask_char: Option<char>) {
        self.render_with_options(area, buf, mask_char, ComposerRenderOptions::default());
    }

    pub(crate) fn render_with_options(
        &self,
        area: Rect,
        buf: &mut Buffer,
        mask_char: Option<char>,
        options: ComposerRenderOptions<'_>,
    ) {
        let ComposerLayout {
            status,
            composer: composer_rect,
            remote_images: remote_images_rect,
            textarea: textarea_rect,
            popup: _, // 008-TRIM: 弹窗区域不再渲染（008 §1 裁剪）
            footer: footer_rect,
        } = self.layout_with_options(area, options);
        self.render_status_surface(status, buf, options);
        // 008-TRIM: 弹窗 above-composer 渲染块与 shortcut overlay 渲染臂（Command/
        // File/Skill/MentionV2 弹窗与快捷键帮助层，008 §1 裁剪：无本地事件源）。
        let transcript_hint_area = inset_footer_hint_area(footer_rect);
        let warning_notice = self.warning_notice_layout(transcript_hint_area, options);
        let warning_area = warning_notice.as_ref().map(|(area, _)| *area);
        match options.footer {
            Some(footer) => {
                self.render_transcript_footer(transcript_hint_area, buf, footer, warning_area);
            }
            None => {
                let footer_props = self.hint_footer_props(options);
                let show_cycle_hint = self.status_surface_height(options) == 0
                    && !footer_props.is_task_running
                    && self.footer.collaboration_mode_indicator.is_some();
                let show_shortcuts_hint = match footer_props.mode {
                    FooterMode::ComposerEmpty => !self.is_in_paste_burst(),
                    FooterMode::ComposerHasDraft => false,
                    FooterMode::HistorySearch
                    | FooterMode::QuitShortcutReminder
                    | FooterMode::ShortcutOverlay
                    | FooterMode::EscHint => false,
                };
                let show_queue_hint = match footer_props.mode {
                    FooterMode::ComposerHasDraft => footer_props.is_task_running,
                    FooterMode::HistorySearch
                    | FooterMode::QuitShortcutReminder
                    | FooterMode::ComposerEmpty
                    | FooterMode::ShortcutOverlay
                    | FooterMode::EscHint => false,
                };
                let hint_rect = if let Some(warning_area) = warning_area {
                    Rect {
                        width: warning_area
                            .x
                            .saturating_sub(footer_rect.x)
                            .saturating_sub(/*rhs*/ 2),
                        ..footer_rect
                    }
                } else {
                    footer_rect
                };
                if let Some(input) = self.draft.textarea.vim_query() {
                    input.render(inset_footer_hint_area(hint_rect), buf);
                } else if let Some(line) = self.history_search_footer_line() {
                    render_footer_line(hint_rect, buf, line);
                } else {
                    let available_width =
                        hint_rect.width.saturating_sub(FOOTER_INDENT_COLS as u16) as usize;
                    let status_line_active = uses_passive_footer_status_layout(&footer_props);
                    let combined_status_line = if status_line_active {
                        passive_footer_status_line(&footer_props)
                    } else {
                        None
                    };
                    // 008-TRIM: effort_status_line_transition 过渡行（effort 动画，
                    // 008 §1 裁剪：无本地事件源）
                    let mut truncated_status_line = if status_line_active {
                        combined_status_line.as_ref().map(|line| {
                            truncate_line_with_ellipsis_if_overflow(line.clone(), available_width)
                        })
                    } else {
                        None
                    };
                    let left_mode_indicator =
                        if status_line_active || self.status_surface_height(options) > 0 {
                            None
                        } else {
                            self.footer.collaboration_mode_indicator
                        };
                    let active_footer_hint_override = self.footer.hint_override.as_ref();
                    let mut left_width = if self.footer.flash_visible() {
                        self.footer
                            .flash
                            .as_ref()
                            .map(|flash| flash.line.width() as u16)
                            .unwrap_or(0)
                    } else if let Some(items) = active_footer_hint_override {
                        footer_hint_items_width(items)
                    } else if status_line_active {
                        truncated_status_line
                            .as_ref()
                            .map(|line| line.width() as u16)
                            .unwrap_or(0)
                    } else {
                        footer_line_width(
                            &footer_props,
                            left_mode_indicator,
                            show_cycle_hint,
                            show_shortcuts_hint,
                            show_queue_hint,
                        )
                    };
                    let right_line =
                        if let Some(label) = self.footer.side_conversation_context_label.as_ref() {
                            Some(side_conversation_context_line(label))
                        } else if let Some(line) = self.shell_mode_footer_line() {
                            Some(line)
                        } else if self.status_surface_height(options) > 0 {
                            None
                        } else if status_line_active {
                            let full = self.mode_indicator_line(show_cycle_hint);
                            let compact = self.mode_indicator_line(/*show_cycle_hint*/ false);
                            let full_width = full.as_ref().map(|l| l.width() as u16).unwrap_or(0);
                            if can_show_left_with_context(hint_rect, left_width, full_width) {
                                full
                            } else {
                                compact
                            }
                        } else {
                            Some(self.right_footer_line_with_context())
                        };
                    let right_width = right_line.as_ref().map(|l| l.width() as u16).unwrap_or(0);
                    if status_line_active
                        && let Some(max_left) = max_left_width_for_right(hint_rect, right_width)
                        && left_width > max_left
                        && let Some(line) = combined_status_line.as_ref().map(|line| {
                            truncate_line_with_ellipsis_if_overflow(line.clone(), max_left as usize)
                        })
                    {
                        left_width = line.width() as u16;
                        truncated_status_line = Some(line);
                    }
                    let can_show_left_and_context =
                        can_show_left_with_context(hint_rect, left_width, right_width);
                    let has_override =
                        self.footer.flash_visible() || active_footer_hint_override.is_some();
                    let single_line_layout = if has_override || status_line_active {
                        None
                    } else {
                        match footer_props.mode {
                            FooterMode::ComposerEmpty | FooterMode::ComposerHasDraft => {
                                // Both of these modes render the single-line footer style (with
                                // either the shortcuts hint or the optional queue hint). We still
                                // want the single-line collapse rules so the mode label can win over
                                // the context indicator on narrow widths.
                                Some(single_line_footer_layout(
                                    hint_rect,
                                    right_width,
                                    left_mode_indicator,
                                    show_cycle_hint,
                                    show_shortcuts_hint,
                                    show_queue_hint,
                                    footer_props.key_hints,
                                ))
                            }
                            FooterMode::EscHint
                            | FooterMode::HistorySearch
                            | FooterMode::QuitShortcutReminder
                            | FooterMode::ShortcutOverlay => None,
                        }
                    };
                    let show_right = if matches!(
                        footer_props.mode,
                        FooterMode::EscHint
                            | FooterMode::HistorySearch
                            | FooterMode::QuitShortcutReminder
                            | FooterMode::ShortcutOverlay
                    ) {
                        false
                    } else {
                        single_line_layout
                            .as_ref()
                            .map(|(_, show_context)| *show_context)
                            .unwrap_or(can_show_left_and_context)
                    };

                    if let Some((summary_left, _)) = single_line_layout {
                        match summary_left {
                            SummaryLeft::Default => {
                                if status_line_active {
                                    if let Some(line) = truncated_status_line.clone() {
                                        render_footer_line(hint_rect, buf, line);
                                    } else {
                                        render_footer_from_props(
                                            hint_rect,
                                            buf,
                                            &footer_props,
                                            left_mode_indicator,
                                            show_cycle_hint,
                                            show_shortcuts_hint,
                                            show_queue_hint,
                                        );
                                    }
                                } else {
                                    render_footer_from_props(
                                        hint_rect,
                                        buf,
                                        &footer_props,
                                        left_mode_indicator,
                                        show_cycle_hint,
                                        show_shortcuts_hint,
                                        show_queue_hint,
                                    );
                                }
                            }
                            SummaryLeft::Custom(line) => {
                                render_footer_line(hint_rect, buf, line);
                            }
                            SummaryLeft::None => {}
                        }
                    } else if self.footer.flash_visible() {
                        if let Some(flash) = self.footer.flash.as_ref() {
                            Widget::render(&flash.line, inset_footer_hint_area(hint_rect), buf);
                        }
                    } else if let Some(items) = active_footer_hint_override {
                        render_footer_hint_items(hint_rect, buf, items);
                    } else if status_line_active {
                        if let Some(line) = truncated_status_line {
                            render_footer_line(hint_rect, buf, line);
                        }
                    } else {
                        render_footer_from_props(
                            hint_rect,
                            buf,
                            &footer_props,
                            self.footer.collaboration_mode_indicator,
                            show_cycle_hint,
                            show_shortcuts_hint,
                            show_queue_hint,
                        );
                    }
                    if show_right && let Some(line) = &right_line {
                        render_context_right(hint_rect, buf, line);
                    }
                    // 008-TRIM: mark_underlined_hyperlink 状态行链接下划线与
                    // effort 过渡帧调度（onboarding 链接标记与 effort 动画，008 §1 裁剪）
                }
            }
        }
        self.footer.warning_notice_area.set(warning_area);
        if let Some((warning_area, line)) = warning_notice {
            line.render(warning_area, buf);
        }
        if let Some(selection) = options.rendered_selection
            && !self.popup_active()
            && options.footer.is_none_or(|footer| !footer.is_interactive)
        {
            let revision = (options.warning_count, self.footer_props().is_task_running);
            let changed = self.footer.selection_revision.replace(revision) != revision;
            let mut selection = selection.borrow_mut();
            selection.register(status.intersection(buf.area), changed);
            selection.register(footer_rect.intersection(buf.area), changed);
        }
        let style = user_message_style();
        Block::default().style(style).render(composer_rect, buf);
        if !remote_images_rect.is_empty() {
            let first = self
                .attachments
                .selected_remote_image_index
                .unwrap_or_default()
                .saturating_sub(usize::from(remote_images_rect.height) - 1);
            Paragraph::new(
                self.attachments
                    .remote_image_lines()
                    .into_iter()
                    .skip(first)
                    .collect::<Vec<_>>(),
            )
            .style(style)
            .render(remote_images_rect, buf);
        }
        if !textarea_rect.is_empty() {
            let prompt = if self.draft.input_enabled {
                if self.draft.is_bash_mode {
                    Span::from("!").light_red().bold()
                } else if self.luna_reserve_active {
                    // Reserve keeps one arrow at every reasoning effort; only its foreground changes.
                    "›"
                        .fg(crate::terminal_palette::best_color((246, 197, 67)))
                        .bold()
                } else {
                    // 008-TRIM: effort_tier 分级提示符箭头（effort 动画，008 §1 裁剪）
                    "›".bold()
                }
            } else {
                "›".dim()
            };
            buf.set_span(
                textarea_rect.x - LIVE_PREFIX_COLS,
                textarea_rect.y,
                &prompt,
                textarea_rect.width,
            );
        }

        let mut state = self.draft.textarea_state.borrow_mut();
        if options.max_height.is_none() {
            state.follow_cursor();
        }
        let textarea_is_empty = self.draft.textarea.text().is_empty() && !self.draft.is_bash_mode;
        if self.draft.input_enabled {
            if let Some(mask_char) = mask_char {
                self.draft
                    .textarea
                    .render_ref_masked(textarea_rect, buf, &mut state, mask_char);
            } else {
                // 008-TRIM: plugin_at_mention_highlights 高亮基组（plugin mention，
                // 008 §1 裁剪：无本地事件源）；保留历史搜索与 vim 搜索高亮。
                let highlights = self
                    .history_search_highlight_ranges()
                    .into_iter()
                    .chain(self.draft.textarea.vim_search_highlights())
                    .map(|range| {
                        (
                            range,
                            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                        )
                    })
                    .collect::<Vec<_>>();
                if highlights.is_empty() {
                    StatefulWidgetRef::render_ref(
                        &(&self.draft.textarea),
                        textarea_rect,
                        buf,
                        &mut state,
                    );
                } else {
                    self.draft.textarea.render_ref_styled_with_highlights(
                        textarea_rect,
                        buf,
                        &mut state,
                        Style::default(),
                        &highlights,
                    );
                }
            }
        }
        if !self.draft.input_enabled || textarea_is_empty {
            let text = if self.draft.input_enabled {
                self.placeholder_text.as_str().to_string()
            } else {
                self.draft
                    .input_disabled_placeholder
                    .as_deref()
                    .unwrap_or("Input disabled.")
                    .to_string()
            };
            if !textarea_rect.is_empty() {
                let placeholder = Span::from(text).dim();
                Line::from(vec![placeholder]).render(textarea_rect.inner(Margin::new(0, 0)), buf);
            }
        }
        // The reserved right margin indicates hidden rows without changing wrapping.
        if options.max_height.is_some()
            && self.draft.input_enabled
            && !textarea_rect.is_empty()
            && textarea_rect.right() < composer_rect.right()
        {
            if state.scroll > 0 {
                buf.set_span(textarea_rect.right(), textarea_rect.y, &"↑".dim(), 1);
            }
            if state.scroll.saturating_add(textarea_rect.height)
                < self.draft.textarea.desired_height(textarea_rect.width)
            {
                buf.set_span(
                    textarea_rect.right(),
                    textarea_rect.bottom() - 1,
                    &"↓".dim(),
                    1,
                );
            }
        }
        // 008-TRIM: effort_ignition 动画渲染与 render_voice_strip 调用
        // （effort 动画与 voice，008 §1 裁剪：无本地事件源）
        drop(state);
        self.render_sparkle(
            composer_rect,
            textarea_rect,
            self.cursor_pos_with_options(area, options),
            buf,
        );
        if options.footer.is_some_and(|footer| footer.is_interactive) {
            buf.set_style(composer_rect, Style::default().dim());
        }
    }
}

#[cfg(test)]
#[path = "chat_composer/agents_navigation_tests.rs"]
mod agents_navigation_tests;

// 008-TRIM: #[cfg(test)] mod effort_tests（effort 动画测试，008 §1 裁剪：无本地事件源；
// 测试文件未随 vendor 带入）

#[cfg(test)]
#[path = "chat_composer_enter_tests.rs"]
mod enter_tests;

#[cfg(test)]
#[path = "chat_composer/embedded_input_tests.rs"]
mod embedded_input_tests;

#[cfg(test)]
#[path = "chat_composer/paste_tests.rs"]
mod paste_tests;

#[cfg(test)]
#[path = "chat_composer/snapshot_tests.rs"]
mod snapshot_tests;

#[cfg(test)]
#[path = "chat_composer/mentions_layout_tests.rs"]
mod mentions_layout_tests;

#[cfg(test)]
mod tests {
    use super::attachment_state::AttachedImage;
    use super::*;
    use crate::test_support::PathBufExt;
    use crate::test_support::test_path_buf;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use tempfile::tempdir;

    use crate::app_event::AppEvent;

    use crate::bottom_pane::AppEventSender;
    use crate::bottom_pane::ChatComposer;
    use crate::bottom_pane::InputResult;
    use crate::bottom_pane::chat_composer::LARGE_PASTE_CHAR_THRESHOLD;
    use crate::bottom_pane::textarea::TextArea;
    use codex_protocol::models::local_image_label_text;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::sync::mpsc::unbounded_channel;

    pub(super) fn new_test_composer() -> (ChatComposer, UnboundedReceiver<AppEvent>) {
        let (tx, rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        (
            ChatComposer::new(
                /*has_input_focus*/ true,
                sender,
                /*enhanced_keys_supported*/ false,
                "Ask Codex to do anything".to_string(),
                /*disable_paste_burst*/ false,
            ),
            rx,
        )
    }

    #[test]
    fn parent_owned_thread_placeholder_snapshot() {
        snapshot_composer_state(
            "parent_owned_thread_placeholder",
            /*enhanced_keys_supported*/ false,
            ChatComposer::set_parent_owned_thread,
        );
    }

    #[test]
    fn light_terminal_palette_renders_light_composer_snapshot() {
        let colors = crate::terminal_probe::DefaultColors {
            fg: (0x55, 0x57, 0x53),
            bg: (0xff, 0xff, 0xff),
        };

        crate::terminal_palette::with_test_default_colors(colors, || {
            let (composer, _rx) = new_test_composer();
            let area = Rect::new(
                /*x*/ 0, /*y*/ 0, /*width*/ 48, /*height*/ 6,
            );
            let mut buffer = Buffer::empty(area);
            composer.render(area, &mut buffer);

            assert_eq!(
                buffer[(0, 1)].bg,
                crate::terminal_palette::rgb_color((244, 244, 244))
            );
            insta::assert_snapshot!("light_terminal_palette_composer", format!("{buffer:?}"));
        });
    }

    #[test]
    fn footer_hint_row_is_separated_from_composer() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        composer.render(area, &mut buf);

        let row_to_string = |y: u16| {
            let mut row = String::new();
            for x in 0..area.width {
                row.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            row
        };

        let mut hint_row: Option<(u16, String)> = None;
        for y in 0..area.height {
            let row = row_to_string(y);
            if row.contains("? for shortcuts") {
                hint_row = Some((y, row));
                break;
            }
        }

        let (hint_row_idx, hint_row_contents) =
            hint_row.expect("expected footer hint row to be rendered");
        assert_eq!(
            hint_row_idx,
            area.height - 1,
            "hint row should occupy the bottom line: {hint_row_contents:?}",
        );

        assert!(
            hint_row_idx > 0,
            "expected a spacing row above the footer hints",
        );

        let spacing_row = row_to_string(hint_row_idx - 1);
        assert_eq!(
            spacing_row.trim(),
            "",
            "expected blank spacing row above hints but saw: {spacing_row:?}",
        );
    }

    #[test]
    fn footer_flash_overrides_footer_hint_override() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_footer_hint_override(Some(vec![("K".to_string(), "label".to_string())]));
        composer.show_footer_flash(Line::from("FLASH"), Duration::from_secs(10));

        let area = Rect::new(0, 0, 60, 6);
        let mut buf = Buffer::empty(area);
        composer.render(area, &mut buf);

        let mut bottom_row = String::new();
        for x in 0..area.width {
            bottom_row.push(
                buf[(x, area.height - 1)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' '),
            );
        }
        assert!(
            bottom_row.contains("FLASH"),
            "expected flash content to render in footer row, saw: {bottom_row:?}",
        );
        assert!(
            !bottom_row.contains("K label"),
            "expected flash to override hint override, saw: {bottom_row:?}",
        );
    }

    #[test]
    fn footer_flash_expires_and_falls_back_to_hint_override() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_footer_hint_override(Some(vec![("K".to_string(), "label".to_string())]));
        composer.show_footer_flash(Line::from("FLASH"), Duration::from_secs(10));
        composer.footer.flash.as_mut().unwrap().expires_at =
            Instant::now() - Duration::from_secs(1);

        let area = Rect::new(0, 0, 60, 6);
        let mut buf = Buffer::empty(area);
        composer.render(area, &mut buf);

        let mut bottom_row = String::new();
        for x in 0..area.width {
            bottom_row.push(
                buf[(x, area.height - 1)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' '),
            );
        }
        assert!(
            bottom_row.contains("K label"),
            "expected hint override to render after flash expired, saw: {bottom_row:?}",
        );
        assert!(
            !bottom_row.contains("FLASH"),
            "expected expired flash to be hidden, saw: {bottom_row:?}",
        );
    }

    pub(super) fn snapshot_composer_state_with_width<F>(
        name: &str,
        width: u16,
        enhanced_keys_supported: bool,
        setup: F,
    ) where
        F: FnOnce(&mut ChatComposer),
    {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            enhanced_keys_supported,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        setup(&mut composer);
        let footer_props = composer.footer_props();
        let footer_lines = footer_height(&footer_props, width);
        let height = footer_lines + 8;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| composer.render(f.area(), f.buffer_mut()))
            .unwrap();
        insta::assert_snapshot!(name, terminal.backend());
    }

    fn snapshot_composer_state<F>(name: &str, enhanced_keys_supported: bool, setup: F)
    where
        F: FnOnce(&mut ChatComposer),
    {
        snapshot_composer_state_with_width(
            name,
            /*width*/ 100,
            enhanced_keys_supported,
            setup,
        );
    }

    #[test]
    fn footer_mode_snapshots() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        // 008-TRIM: footer_mode_shortcut_overlay / footer_mode_shortcut_overlay_queue_submissions
        // 快照（shortcut overlay 展开渲染，008 §1 裁剪：无本地事件源）；footer 模式切换
        // 行为由 shift_question_mark_toggles_shortcut_overlay_when_empty 等测试保留。

        snapshot_composer_state(
            "footer_mode_ctrl_c_quit",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer.show_quit_shortcut_hint(
                    key_hint::ctrl(KeyCode::Char('c')),
                    /*has_focus*/ true,
                );
            },
        );

        snapshot_composer_state(
            "footer_mode_ctrl_c_then_esc_hint",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer.show_quit_shortcut_hint(
                    key_hint::ctrl(KeyCode::Char('c')),
                    /*has_focus*/ true,
                );
                let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            },
        );

        snapshot_composer_state(
            "footer_mode_esc_hint_from_overlay",
            /*enhanced_keys_supported*/ true,
            |composer| {
                let _ = composer
                    .handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
                let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            },
        );

        snapshot_composer_state(
            "footer_mode_esc_hint_backtrack",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer.set_esc_backtrack_hint(/*show*/ true);
                let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            },
        );

        snapshot_composer_state(
            "footer_mode_overlay_then_external_esc_hint",
            /*enhanced_keys_supported*/ true,
            |composer| {
                let _ = composer
                    .handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
                composer.set_esc_backtrack_hint(/*show*/ true);
            },
        );

        snapshot_composer_state(
            "footer_mode_hidden_while_typing",
            /*enhanced_keys_supported*/ true,
            |composer| {
                type_chars_humanlike(composer, &['h']);
            },
        );

        snapshot_composer_state(
            "footer_mode_history_search",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer
                    .history
                    .record_local_submission(HistoryEntry::new("cargo test".to_string()));
                let _ = composer
                    .handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
                let _ = composer
                    .handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
            },
        );

        snapshot_composer_state(
            "footer_mode_history_search_unavailable",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer.set_text_content("draft".to_string(), Vec::new(), Vec::new());
                composer.begin_history_search();
                for ch in ['g', 'i', 't'] {
                    let _ = composer
                        .handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                }
                composer.apply_history_search_result(HistorySearchResult::Pending);
                composer.apply_history_search_result(HistorySearchResult::Unavailable);
            },
        );

        snapshot_composer_state(
            "footer_mode_shell_command_absorbs_bang",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer.set_status_line_enabled(/*enabled*/ true);
                composer.set_status_line(Some(Line::from(
                    "gpt-5.4 high fast · ~/code/codex-1 · Context 0% used",
                )));
                composer.set_text_content("!git status".to_string(), Vec::new(), Vec::new());
            },
        );

        snapshot_composer_state(
            "footer_mode_shell_command_escape_exits_empty_mode",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer.set_status_line_enabled(/*enabled*/ true);
                composer.set_status_line(Some(Line::from(
                    "gpt-5.4 high fast · ~/code/codex-1 · Context 0% used",
                )));
                composer.set_text_content("!".to_string(), Vec::new(), Vec::new());
                let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            },
        );
    }

    #[test]
    fn shell_command_cursor_uses_absorbed_prefix() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let area = Rect::new(0, 0, 40, 5);

        composer.set_text_content("!git".to_string(), Vec::new(), Vec::new());
        composer.move_cursor_to_end();
        assert_eq!(composer.cursor_pos(area), Some((5, 1)));

        composer.set_text_content("! git".to_string(), Vec::new(), Vec::new());
        composer.move_cursor_to_end();
        assert_eq!(composer.cursor_pos(area), Some((6, 1)));
    }

    #[test]
    fn shell_command_uses_shell_accent_style() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_status_line_enabled(/*enabled*/ true);
        composer.set_status_line(Some(Line::from(
            "gpt-5.4 high fast · ~/code/codex-1 · Context 0% used",
        )));
        composer.set_text_content("!git status".to_string(), Vec::new(), Vec::new());

        let area = Rect::new(0, 0, 100, 9);
        let mut buf = Buffer::empty(area);
        composer.render(area, &mut buf);

        let prompt_cell = &buf[(0, 1)];
        assert_eq!(prompt_cell.symbol(), "!");
        assert_eq!(prompt_cell.style().fg, Some(Color::LightRed));

        let footer_y = area.height - 1;
        let footer_text = (0..area.width)
            .map(|x| buf[(x, footer_y)].symbol().chars().next().unwrap_or(' '))
            .collect::<String>();
        let shell_label_x = footer_text
            .find("Shell mode")
            .expect("expected shell mode footer label");
        assert_eq!(
            buf[(shell_label_x as u16, footer_y)].style().fg,
            Some(Color::LightRed)
        );
    }

    // 008-TRIM: 删除覆盖被裁功能的测试（skill/plugin/connector/mention 弹窗、file search、
    // command popup 补全、effort/voice/onboarding 链接标记，008 §1 裁剪：无本地事件源）；
    // 覆盖保留行为的测试未动。含 status_line_hyperlink_marks_pr_number_cells
    // （依赖 mark_underlined_hyperlink 渲染）与 shift_question 系列之外的 shortcut overlay
    // 展开快照。

    #[test]
    fn esc_exits_empty_shell_mode() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['!']);
        assert!(composer.draft.is_bash_mode);
        assert_eq!(composer.current_text(), "!");

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(matches!(result, InputResult::None));
        assert!(needs_redraw);
        assert!(!composer.draft.is_bash_mode);
        assert_eq!(composer.current_text(), "");
    }

    #[test]
    fn esc_keeps_shell_mode_when_paste_burst_flushes_pending_text() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['!']);
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert!(composer.is_in_paste_burst());
        assert_eq!(composer.current_text(), "!");

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(matches!(result, InputResult::None));
        assert!(needs_redraw);
        assert!(composer.draft.is_bash_mode);
        assert_eq!(composer.current_text(), "!g");
    }

    #[test]
    fn footer_collapse_snapshots() {
        fn setup_collab_footer(
            composer: &mut ChatComposer,
            context_percent: i64,
            indicator: Option<CollaborationModeIndicator>,
        ) {
            composer.set_collaboration_modes_enabled(/*enabled*/ true);
            composer.set_collaboration_mode_indicator(indicator);
            composer.set_context_window(Some(context_percent), /*used_tokens*/ None);
        }

        // Empty textarea, agent idle: shortcuts hint can show, and cycle hint is hidden.
        snapshot_composer_state_with_width(
            "footer_collapse_empty_full",
            /*width*/ 120,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 100, /*indicator*/ None,
                );
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_empty_mode_cycle_with_context",
            /*width*/ 60,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 100, /*indicator*/ None,
                );
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_empty_mode_cycle_without_context",
            /*width*/ 44,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 100, /*indicator*/ None,
                );
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_empty_mode_only",
            /*width*/ 26,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 100, /*indicator*/ None,
                );
            },
        );

        // Empty textarea, plan mode idle: shortcuts hint and cycle hint are available.
        snapshot_composer_state_with_width(
            "footer_collapse_plan_empty_full",
            /*width*/ 120,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 100,
                    Some(CollaborationModeIndicator::Plan),
                );
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_plan_empty_mode_cycle_with_context",
            /*width*/ 60,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 100,
                    Some(CollaborationModeIndicator::Plan),
                );
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_plan_empty_mode_cycle_without_context",
            /*width*/ 44,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 100,
                    Some(CollaborationModeIndicator::Plan),
                );
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_plan_empty_mode_only",
            /*width*/ 26,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 100,
                    Some(CollaborationModeIndicator::Plan),
                );
            },
        );

        // Textarea has content, agent running: queue hint is shown.
        snapshot_composer_state_with_width(
            "footer_collapse_queue_full",
            /*width*/ 120,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 98, /*indicator*/ None,
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_queue_short_with_context",
            /*width*/ 50,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 98, /*indicator*/ None,
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_queue_message_without_context",
            /*width*/ 40,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 98, /*indicator*/ None,
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_queue_short_without_context",
            /*width*/ 30,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 98, /*indicator*/ None,
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_queue_mode_only",
            /*width*/ 20,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer, /*context_percent*/ 98, /*indicator*/ None,
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );

        // Textarea has content, plan mode active, agent running: queue hint + mode.
        snapshot_composer_state_with_width(
            "footer_collapse_plan_queue_full",
            /*width*/ 120,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 98,
                    Some(CollaborationModeIndicator::Plan),
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_plan_queue_short_with_context",
            /*width*/ 50,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 98,
                    Some(CollaborationModeIndicator::Plan),
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_plan_queue_message_without_context",
            /*width*/ 40,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 98,
                    Some(CollaborationModeIndicator::Plan),
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_plan_queue_short_without_context",
            /*width*/ 30,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 98,
                    Some(CollaborationModeIndicator::Plan),
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
        snapshot_composer_state_with_width(
            "footer_collapse_plan_queue_mode_only",
            /*width*/ 20,
            /*enhanced_keys_supported*/ true,
            |composer| {
                setup_collab_footer(
                    composer,
                    /*context_percent*/ 98,
                    Some(CollaborationModeIndicator::Plan),
                );
                composer.set_task_running(/*running*/ true);
                composer.set_text_content("Test".to_string(), Vec::new(), Vec::new());
            },
        );
    }

    #[test]
    fn esc_hint_stays_hidden_with_draft_content() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['d']);

        assert!(!composer.is_empty());
        assert_eq!(composer.current_text(), "d");
        assert_eq!(composer.footer.mode, FooterMode::ComposerEmpty);
        assert!(matches!(composer.popups.active, ActivePopup::None));

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(composer.footer.mode, FooterMode::ComposerEmpty);
        assert!(!composer.footer.esc_backtrack_hint);
    }

    #[test]
    fn empty_vim_insert_escape_enters_normal_without_esc_hint() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_vim_enabled(/*enabled*/ true);
        composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

        assert!(composer.is_empty());
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(matches!(result, InputResult::None));
        assert!(needs_redraw);
        assert!(composer.is_empty());
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Normal".magenta())
        );
        assert_eq!(composer.footer.mode, FooterMode::ComposerEmpty);
        assert!(!composer.footer.esc_backtrack_hint);

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Replace".cyan())
        );
        snapshot_composer_state(
            "vim_replace_mode",
            /*enhanced_keys_supported*/ true,
            |composer| {
                composer.set_vim_enabled(/*enabled*/ true);
                composer.handle_key_event(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));
                composer.handle_paste("x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1));
            },
        );
        composer.insert_str("ok");
        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));
        assert_eq!(composer.current_text(), "");
        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Normal".magenta())
        );
    }
    #[test]
    fn slash_command_dispatches_after_vim_insert_slash() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ true,
        );
        composer.set_vim_enabled(/*enabled*/ true);

        for ch in ['i', '/', 'd', 'i', 'f', 'f'] {
            let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(composer.draft.textarea.text(), "/diff");
        // 008-TRIM: command popup 断言（008 §1 裁剪：无本地事件源）

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(needs_redraw);
        assert!(composer.is_empty());
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );
        assert!(matches!(result, InputResult::Command(SlashCommand::Diff)));
    }

    #[test]
    fn inline_slash_command_dispatch_resets_vim_mode_to_insert() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ true,
        );
        composer.set_collaboration_modes_enabled(/*enabled*/ true);
        composer.set_vim_enabled(/*enabled*/ true);

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        composer.set_text_content("/plan investigate this".to_string(), Vec::new(), Vec::new());
        composer.popups.active = ActivePopup::None;
        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(needs_redraw);
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );
        match result {
            InputResult::CommandWithArgs(cmd, args, text_elements) => {
                assert_eq!(cmd, SlashCommand::Plan);
                assert_eq!(args, "investigate this");
                assert!(text_elements.is_empty());
            }
            _ => panic!("expected CommandWithArgs"),
        }
        composer
            .prepare_inline_args_submission(/*record_history*/ false)
            .unwrap();
        for code in [KeyCode::Char('x'), KeyCode::Esc, KeyCode::Char('.')] {
            composer.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert_eq!(composer.current_text(), "xx");
    }

    #[test]
    fn bang_enters_shell_mode_in_vim_normal_mode() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ true,
        );
        composer.set_vim_enabled(/*enabled*/ true);

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));

        assert!(matches!(result, InputResult::None));
        assert!(needs_redraw);
        assert!(composer.draft.is_bash_mode);
        assert_eq!(composer.current_text(), "!");
        assert_eq!(composer.draft.textarea.text(), "");
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );
    }

    #[test]
    fn shell_command_can_be_typed_after_vim_normal_bang() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ true,
        );
        composer.set_vim_enabled(/*enabled*/ true);

        for ch in ['!', 'e', 'c', 'h', 'o'] {
            let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }

        assert!(composer.draft.is_bash_mode);
        assert_eq!(composer.current_text(), "!echo");
        assert_eq!(composer.draft.textarea.text(), "echo");
        assert!(matches!(composer.popups.active, ActivePopup::None));
    }

    #[test]
    fn base_footer_mode_tracks_empty_state_after_quit_hint_expires() {
        use crossterm::event::KeyCode;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['d']);
        composer
            .show_quit_shortcut_hint(key_hint::ctrl(KeyCode::Char('c')), /*has_focus*/ true);
        composer.footer.quit_shortcut_expires_at =
            Some(Instant::now() - std::time::Duration::from_secs(1));

        assert_eq!(composer.footer_mode(), FooterMode::ComposerHasDraft);

        composer.set_text_content(String::new(), Vec::new(), Vec::new());
        assert_eq!(composer.footer_mode(), FooterMode::ComposerEmpty);
    }

    #[test]
    fn clear_for_ctrl_c_records_cleared_draft() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_text_content("draft text".to_string(), Vec::new(), Vec::new());
        assert_eq!(composer.clear_for_ctrl_c(), Some("draft text".to_string()));
        assert!(composer.is_empty());

        assert_eq!(
            composer.history.navigate_up(&composer.app_event_tx),
            Some(HistoryEntry::new("draft text".to_string()))
        );
    }

    #[test]
    fn clear_for_ctrl_c_preserves_pending_paste_history_entry() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let large = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5);
        composer.handle_paste(large.clone());
        let char_count = large.chars().count();
        let placeholder = format!("[Pasted Content {char_count} chars]");
        assert_eq!(composer.draft.textarea.text(), placeholder);
        assert_eq!(
            composer.draft.pending_pastes,
            vec![(placeholder.clone(), large.clone())]
        );

        composer.clear_for_ctrl_c();
        assert!(composer.is_empty());

        let history_entry = composer
            .history
            .navigate_up(&composer.app_event_tx)
            .expect("expected history entry");
        let text_elements = vec![TextElement::new(
            (0..placeholder.len()).into(),
            Some(placeholder.clone()),
        )];
        assert_eq!(
            history_entry,
            HistoryEntry::with_pending(
                placeholder.clone(),
                text_elements,
                Vec::new(),
                vec![(placeholder.clone(), large.clone())]
            )
        );

        composer.apply_history_entry(history_entry);
        assert_eq!(composer.draft.textarea.text(), placeholder);
        assert_eq!(
            composer.draft.pending_pastes,
            vec![(placeholder.clone(), large)]
        );
        assert_eq!(
            composer.draft.textarea.element_payloads(),
            vec![placeholder]
        );

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                assert_eq!(text, "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5));
                assert!(text_elements.is_empty());
            }
            _ => panic!("expected Submitted"),
        }
    }

    #[test]
    fn large_paste_numbering_reuses_after_ctrl_c_clear() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
        let base = format!("[Pasted Content {} chars]", paste.chars().count());

        composer.handle_paste(paste.clone());
        assert_eq!(composer.draft.textarea.text(), base);
        assert_eq!(composer.draft.pending_pastes.len(), 1);

        assert_eq!(composer.clear_for_ctrl_c(), Some(base.clone()));
        assert!(composer.draft.textarea.text().is_empty());
        assert!(composer.draft.pending_pastes.is_empty());

        composer.handle_paste(paste);
        assert_eq!(composer.draft.textarea.text(), base);
        assert_eq!(composer.draft.pending_pastes.len(), 1);
        assert_eq!(composer.draft.pending_pastes[0].0, base);
    }

    #[test]
    fn vim_mode_resets_to_insert_after_submission() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(/*enabled*/ true);
        composer.set_vim_enabled(/*enabled*/ true);

        assert!(composer.draft.textarea.is_vim_enabled());
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Normal".magenta())
        );

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        composer.set_text_content("h".to_string(), Vec::new(), Vec::new());
        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(composer.draft.textarea.is_vim_enabled());
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );
        assert!(composer.is_empty());
        match result {
            InputResult::Submitted { text, .. } => assert_eq!(text, "h"),
            _ => panic!("expected Submitted"),
        }
    }

    #[test]
    fn vim_mode_resets_to_insert_after_queued_submission() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(/*enabled*/ true);
        composer.set_task_running(/*running*/ true);
        composer.set_vim_enabled(/*enabled*/ true);

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        composer.set_text_content("queued".to_string(), Vec::new(), Vec::new());
        let (result, _) = composer.handle_submission(/*should_queue*/ true);

        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );
        assert!(composer.is_empty());
        match result {
            InputResult::Queued { text, .. } => assert_eq!(text, "queued"),
            _ => panic!("expected Queued"),
        }
    }

    #[test]
    fn vim_mode_stays_insert_after_suppressed_submission() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(/*enabled*/ true);
        composer.set_vim_enabled(/*enabled*/ true);

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        composer.set_text_content("/not-a-command".to_string(), Vec::new(), Vec::new());
        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(result, InputResult::None));
        assert_eq!(composer.draft.textarea.text(), "/not-a-command");
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );
    }

    #[test]
    fn esc_switches_vim_insert_to_normal() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_vim_enabled(/*enabled*/ true);

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        composer.set_text_content("hey".to_string(), Vec::new(), Vec::new());
        composer
            .draft
            .textarea
            .set_cursor(composer.draft.textarea.text().len());
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Insert".green())
        );
        assert_eq!(composer.draft.textarea.cursor(), "hey".len());

        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Normal".magenta())
        );
        assert_eq!(composer.draft.textarea.cursor(), "he".len());
    }

    #[test]
    fn vim_insert_uses_bar_cursor_style() {
        use crate::render::renderable::Renderable;
        use crossterm::cursor::SetCursorStyle;
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;
        use crossterm::queue;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ true,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let area = Rect::new(0, 0, 80, 10);
        let style_output = |style| {
            let mut output = Vec::new();
            queue!(output, style).expect("queue cursor style");
            output
        };
        let default = style_output(SetCursorStyle::DefaultUserShape);
        let steady_bar = style_output(SetCursorStyle::SteadyBar);

        assert_eq!(style_output(composer.cursor_style(area)), default,);

        composer.set_vim_enabled(/*enabled*/ true);
        assert_eq!(style_output(composer.cursor_style(area)), default,);

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        composer.set_text_content("hey".to_string(), Vec::new(), Vec::new());
        assert_eq!(style_output(composer.cursor_style(area)), steady_bar);

        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(style_output(composer.cursor_style(area)), default,);

        composer.handle_key_event(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));
        assert_eq!(style_output(composer.cursor_style(area)), default,);
    }

    #[test]
    fn clear_for_ctrl_c_preserves_image_draft_state() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let path = PathBuf::from("example.png");
        composer.attach_image(path.clone());
        let placeholder = local_image_label_text(/*label_number*/ 1);

        composer.clear_for_ctrl_c();
        assert!(composer.is_empty());

        let history_entry = composer
            .history
            .navigate_up(&composer.app_event_tx)
            .expect("expected history entry");
        let text_elements = vec![TextElement::new(
            (0..placeholder.len()).into(),
            Some(placeholder.clone()),
        )];
        assert_eq!(
            history_entry,
            HistoryEntry::with_pending(
                placeholder.clone(),
                text_elements,
                vec![path.clone()],
                Vec::new()
            )
        );

        composer.apply_history_entry(history_entry);
        assert_eq!(composer.draft.textarea.text(), placeholder);
        assert_eq!(composer.local_image_paths(), vec![path]);
        assert_eq!(
            composer.draft.textarea.element_payloads(),
            vec![placeholder]
        );
    }

    #[test]
    fn clear_for_ctrl_c_preserves_remote_offset_image_labels() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let remote_image_url = "https://example.com/one.png".to_string();
        composer.set_remote_image_urls(vec![remote_image_url.clone()]);
        let text = "[Image #2] draft".to_string();
        let text_elements = vec![TextElement::new(
            (0.."[Image #2]".len()).into(),
            Some("[Image #2]".to_string()),
        )];
        let local_image_path = PathBuf::from("/tmp/local-draft.png");
        composer.set_text_content(text, text_elements, vec![local_image_path.clone()]);
        let expected_text = composer.current_text();
        let expected_elements = composer.text_elements();
        assert_eq!(expected_text, "[Image #2] draft");
        assert_eq!(
            expected_elements[0].placeholder(&expected_text),
            Some("[Image #2]")
        );

        assert_eq!(composer.clear_for_ctrl_c(), Some(expected_text.clone()));

        assert_eq!(
            composer.history.navigate_up(&composer.app_event_tx),
            Some(HistoryEntry::with_pending_and_remote(
                expected_text,
                expected_elements,
                vec![local_image_path],
                Vec::new(),
                vec![remote_image_url],
            ))
        );
    }

    #[test]
    fn apply_history_entry_preserves_local_placeholders_after_remote_prefix() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let remote_image_url = "https://example.com/one.png".to_string();
        let local_image_path = PathBuf::from("/tmp/local-draft.png");
        composer.apply_history_entry(HistoryEntry::with_pending_and_remote(
            "[Image #2] draft".to_string(),
            vec![TextElement::new(
                (0.."[Image #2]".len()).into(),
                Some("[Image #2]".to_string()),
            )],
            vec![local_image_path.clone()],
            Vec::new(),
            vec![remote_image_url.clone()],
        ));

        let restored_text = composer.current_text();
        assert_eq!(restored_text, "[Image #2] draft");
        let restored_elements = composer.text_elements();
        assert_eq!(restored_elements.len(), 1);
        assert_eq!(
            restored_elements[0].placeholder(&restored_text),
            Some("[Image #2]")
        );
        assert_eq!(composer.local_image_paths(), vec![local_image_path]);
        assert_eq!(composer.remote_image_urls(), vec![remote_image_url]);
    }

    /// Behavior: `?` toggles the shortcut overlay only when the composer is otherwise empty. After
    /// any typing has occurred, `?` should be inserted as a literal character.
    #[test]
    fn question_mark_only_toggles_on_first_char() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(result, InputResult::None);
        assert!(needs_redraw, "toggling overlay should request redraw");
        assert_eq!(composer.footer.mode, FooterMode::ShortcutOverlay);

        // Toggle back to prompt mode so subsequent typing captures characters.
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(composer.footer.mode, FooterMode::ComposerEmpty);

        type_chars_humanlike(&mut composer, &['h']);
        assert_eq!(composer.draft.textarea.text(), "h");
        assert_eq!(composer.footer_mode(), FooterMode::ComposerHasDraft);

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(result, InputResult::None);
        assert!(needs_redraw, "typing should still mark the view dirty");
        let _ = flush_after_paste_burst(&mut composer);
        assert_eq!(composer.draft.textarea.text(), "h?");
        assert_eq!(composer.footer.mode, FooterMode::ComposerEmpty);
        assert_eq!(composer.footer_mode(), FooterMode::ComposerHasDraft);
    }

    #[test]
    fn shift_question_mark_toggles_shortcut_overlay_when_empty() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(true);

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));
        assert_eq!(result, InputResult::None);
        assert!(needs_redraw, "toggling overlay should request redraw");
        assert_eq!(composer.footer.mode, FooterMode::ShortcutOverlay);
    }

    /// Behavior: while a paste-like burst is being captured, `?` must not toggle the shortcut
    /// overlay; it should be treated as part of the pasted content.
    #[test]
    fn question_mark_does_not_toggle_during_paste_burst() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Force an active paste burst so this test doesn't depend on tight timing.
        composer
            .draft
            .paste_burst
            .begin_with_retro_grabbed(String::new(), Instant::now());

        for ch in ['h', 'i', '?', 't', 'h', 'e', 'r', 'e'] {
            let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert!(composer.is_in_paste_burst());
        assert_eq!(composer.draft.textarea.text(), "");

        let _ = flush_after_paste_burst(&mut composer);

        assert_eq!(composer.draft.textarea.text(), "hi?there");
        assert_ne!(composer.footer.mode, FooterMode::ShortcutOverlay);
    }

    #[test]
    fn test_current_at_token_basic_cases() {
        let test_cases = vec![
            // Valid @ tokens
            ("@hello", 3, Some("hello".to_string()), "Basic ASCII token"),
            (
                "@file.txt",
                4,
                Some("file.txt".to_string()),
                "ASCII with extension",
            ),
            (
                "hello @world test",
                8,
                Some("world".to_string()),
                "ASCII token in middle",
            ),
            (
                "@test123",
                5,
                Some("test123".to_string()),
                "ASCII with numbers",
            ),
            // Unicode examples
            ("@İstanbul", 3, Some("İstanbul".to_string()), "Turkish text"),
            (
                "@testЙЦУ.rs",
                8,
                Some("testЙЦУ.rs".to_string()),
                "Mixed ASCII and Cyrillic",
            ),
            ("@诶", 2, Some("诶".to_string()), "Chinese character"),
            ("@👍", 2, Some("👍".to_string()), "Emoji token"),
            // Invalid cases (should return None)
            ("hello", 2, None, "No @ symbol"),
            (
                "@",
                1,
                Some("".to_string()),
                "Only @ symbol triggers empty query",
            ),
            ("@ hello", 2, None, "@ followed by space"),
            ("test @ world", 6, None, "@ with spaces around"),
        ];

        for (input, cursor_pos, expected, description) in test_cases {
            let mut textarea = TextArea::new();
            textarea.insert_str(input);
            textarea.set_cursor(cursor_pos);

            let result = ChatComposer::current_at_token(&textarea);
            assert_eq!(
                result, expected,
                "Failed for case: {description} - input: '{input}', cursor: {cursor_pos}"
            );
        }
    }

    #[test]
    fn test_current_at_token_cursor_positions() {
        let test_cases = vec![
            // Different cursor positions within a token
            ("@test", 0, Some("test".to_string()), "Cursor at @"),
            ("@test", 1, Some("test".to_string()), "Cursor after @"),
            ("@test", 5, Some("test".to_string()), "Cursor at end"),
            // Multiple tokens - cursor determines which token
            ("@file1 @file2", 0, Some("file1".to_string()), "First token"),
            (
                "@file1 @file2",
                8,
                Some("file2".to_string()),
                "Second token",
            ),
            // Edge cases
            ("@", 0, Some("".to_string()), "Only @ symbol"),
            ("@a", 2, Some("a".to_string()), "Single character after @"),
            ("", 0, None, "Empty input"),
        ];

        for (input, cursor_pos, expected, description) in test_cases {
            let mut textarea = TextArea::new();
            textarea.insert_str(input);
            textarea.set_cursor(cursor_pos);

            let result = ChatComposer::current_at_token(&textarea);
            assert_eq!(
                result, expected,
                "Failed for cursor position case: {description} - input: '{input}', cursor: {cursor_pos}",
            );
        }
    }

    #[test]
    fn test_current_at_token_whitespace_boundaries() {
        let test_cases = vec![
            // Space boundaries
            (
                "aaa@aaa",
                4,
                None,
                "Connected @ token - no completion by design",
            ),
            (
                "aaa @aaa",
                5,
                Some("aaa".to_string()),
                "@ token after space",
            ),
            (
                "test @file.txt",
                7,
                Some("file.txt".to_string()),
                "@ token after space",
            ),
            // Full-width space boundaries
            (
                "test　@İstanbul",
                8,
                Some("İstanbul".to_string()),
                "@ token after full-width space",
            ),
            (
                "@ЙЦУ　@诶",
                10,
                Some("诶".to_string()),
                "Full-width space between Unicode tokens",
            ),
            // Tab and newline boundaries
            (
                "test\t@file",
                6,
                Some("file".to_string()),
                "@ token after tab",
            ),
        ];

        for (input, cursor_pos, expected, description) in test_cases {
            let mut textarea = TextArea::new();
            textarea.insert_str(input);
            textarea.set_cursor(cursor_pos);

            let result = ChatComposer::current_at_token(&textarea);
            assert_eq!(
                result, expected,
                "Failed for whitespace boundary case: {description} - input: '{input}', cursor: {cursor_pos}",
            );
        }
    }

    #[test]
    fn test_current_at_token_tracks_tokens_with_second_at() {
        let input = "npx -y @kaeawc/auto-mobile@latest";
        let token_start = input.find("@kaeawc").expect("scoped npm package present");
        let version_at = input
            .rfind("@latest")
            .expect("version suffix present in scoped npm package");
        let test_cases = vec![
            (token_start, "Cursor at leading @"),
            (token_start + 8, "Cursor inside scoped package name"),
            (version_at, "Cursor at version @"),
            (input.len(), "Cursor at end of token"),
        ];

        for (cursor_pos, description) in test_cases {
            let mut textarea = TextArea::new();
            textarea.insert_str(input);
            textarea.set_cursor(cursor_pos);

            let result = ChatComposer::current_at_token(&textarea);
            assert_eq!(
                result,
                Some("kaeawc/auto-mobile@latest".to_string()),
                "Failed for case: {description} - input: '{input}', cursor: {cursor_pos}"
            );
        }
    }

    #[test]
    fn test_current_at_token_allows_file_queries_with_second_at() {
        let input = "@icons/icon@2x.png";
        let version_at = input
            .rfind("@2x")
            .expect("second @ in file token should be present");
        let test_cases = vec![
            (0, "Cursor at leading @"),
            (8, "Cursor before second @"),
            (version_at, "Cursor at second @"),
            (input.len(), "Cursor at end of token"),
        ];

        for (cursor_pos, description) in test_cases {
            let mut textarea = TextArea::new();
            textarea.insert_str(input);
            textarea.set_cursor(cursor_pos);

            let result = ChatComposer::current_at_token(&textarea);
            assert!(
                result.is_some(),
                "Failed for case: {description} - input: '{input}', cursor: {cursor_pos}"
            );
        }
    }

    #[test]
    fn test_current_at_token_ignores_mid_word_at() {
        let input = "foo@bar";
        let at_pos = input.find('@').expect("@ present");
        let test_cases = vec![
            (at_pos, "Cursor at mid-word @"),
            (input.len(), "Cursor at end of word containing @"),
        ];

        for (cursor_pos, description) in test_cases {
            let mut textarea = TextArea::new();
            textarea.insert_str(input);
            textarea.set_cursor(cursor_pos);

            let result = ChatComposer::current_at_token(&textarea);
            assert_eq!(
                result, None,
                "Failed for case: {description} - input: '{input}', cursor: {cursor_pos}"
            );
        }
    }

    #[test]
    fn set_text_content_rebinds_at_sigiled_mentions() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let mention_bindings = vec![MentionBinding {
            sigil: '@',
            mention: "figma".to_string(),
            path: "/tmp/user/figma/SKILL.md".to_string(),
        }];
        composer.set_text_content_with_mention_bindings(
            "@figma please".to_string(),
            Vec::new(),
            Vec::new(),
            mention_bindings.clone(),
        );

        assert_eq!(composer.mention_bindings(), mention_bindings);
    }

    #[test]
    fn set_text_content_rebinds_matching_sigil_only() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let mention_bindings = vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "app://figma".to_string(),
        }];
        composer.set_text_content_with_mention_bindings(
            "@figma then $figma".to_string(),
            Vec::new(),
            Vec::new(),
            mention_bindings.clone(),
        );

        let bound_tokens = composer
            .current_mention_elements()
            .into_iter()
            .map(|(_, sigil, mention)| (sigil, mention))
            .collect::<Vec<_>>();
        assert_eq!(bound_tokens, vec![('$', "figma".to_string())]);
        assert_eq!(composer.mention_bindings(), mention_bindings);
    }

    #[test]
    fn set_text_content_rebinds_both_sigil_forms() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let mention_bindings = vec![
            MentionBinding {
                sigil: '@',
                mention: "figma".to_string(),
                path: "plugin://figma@test".to_string(),
            },
            MentionBinding {
                sigil: '$',
                mention: "figma".to_string(),
                path: "app://figma".to_string(),
            },
        ];
        composer.set_text_content_with_mention_bindings(
            "@figma then $figma".to_string(),
            Vec::new(),
            Vec::new(),
            mention_bindings.clone(),
        );

        let bound_tokens = composer
            .current_mention_elements()
            .into_iter()
            .map(|(_, sigil, mention)| (sigil, mention))
            .collect::<Vec<_>>();
        assert_eq!(
            bound_tokens,
            vec![('@', "figma".to_string()), ('$', "figma".to_string())]
        );
        assert_eq!(composer.mention_bindings(), mention_bindings);
    }

    #[test]
    fn set_text_content_rebinds_at_mentions_after_email_substrings() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let text = "foo@sample.com then @sample".to_string();
        let mention_start = text.rfind("@sample").expect("expected bound mention token");
        let mention_range = mention_start..mention_start + "@sample".len();
        let mention_bindings = vec![MentionBinding {
            sigil: '@',
            mention: "sample".to_string(),
            path: "plugin://sample@test".to_string(),
        }];
        composer.set_text_content_with_mention_bindings(
            text,
            Vec::new(),
            Vec::new(),
            mention_bindings.clone(),
        );

        // Fix coverage: only the plaintext `@sample` should be atomic; the email substring stays editable.
        assert_eq!(
            composer
                .draft
                .textarea
                .text_element_snapshots()
                .into_iter()
                .map(|snapshot| snapshot.range)
                .collect::<Vec<_>>(),
            vec![mention_range]
        );
        assert_eq!(composer.mention_bindings(), mention_bindings);
    }

    #[test]
    fn set_text_content_rebinds_at_mentions_after_punctuation() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let text = "Please ask (@sample)".to_string();
        let mention_start = text.find("@sample").expect("expected bound mention token");
        let mention_range = mention_start..mention_start + "@sample".len();
        let mention_bindings = vec![MentionBinding {
            sigil: '@',
            mention: "sample".to_string(),
            path: "plugin://sample@test".to_string(),
        }];
        composer.set_text_content_with_mention_bindings(
            text,
            Vec::new(),
            Vec::new(),
            mention_bindings.clone(),
        );

        assert_eq!(
            composer
                .draft
                .textarea
                .text_element_snapshots()
                .into_iter()
                .map(|snapshot| snapshot.range)
                .collect::<Vec<_>>(),
            vec![mention_range]
        );
        assert_eq!(composer.mention_bindings(), mention_bindings);
    }

    #[test]
    fn bound_at_mentions_do_not_block_arrow_navigation() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_text_content_with_mention_bindings(
            "go @figma now".to_string(),
            Vec::new(),
            Vec::new(),
            vec![MentionBinding {
                sigil: '@',
                mention: "figma".to_string(),
                path: "plugin://figma@debug".to_string(),
            }],
        );
        composer.draft.textarea.set_cursor("go".len());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.cursor(), "go".len() + 1);
        assert!(matches!(composer.popups.active, ActivePopup::None));

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.cursor(), "go @figma".len());
        assert!(matches!(composer.popups.active, ActivePopup::None));

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.cursor(), "go ".len());
        assert!(matches!(composer.popups.active, ActivePopup::None));

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.cursor(), "go".len());
        assert!(matches!(composer.popups.active, ActivePopup::None));
    }
    /// should submit normally (i.e. we should not misclassify this as a paste burst).
    #[test]
    fn ascii_prefix_survives_non_ascii_followup() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        assert!(composer.is_in_paste_burst());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('あ'), KeyModifiers::NONE));

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted { text, .. } => assert_eq!(text, "1あ"),
            _ => panic!("expected Submitted"),
        }
    }

    /// Behavior: a single non-ASCII char should be inserted immediately (IME-friendly) and should
    /// not create any paste-burst state.
    #[test]
    fn non_ascii_char_inserts_immediately_without_burst_state() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('あ'), KeyModifiers::NONE));

        assert_eq!(composer.draft.textarea.text(), "あ");
        assert!(!composer.is_in_paste_burst());
    }

    /// Behavior: while we're capturing a paste-like burst, Enter should be treated as a newline
    /// within the burst (not as "submit"), and the whole payload should flush as one paste.
    #[test]
    fn non_ascii_burst_buffers_enter_and_flushes_multiline() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer
            .draft
            .paste_burst
            .begin_with_retro_grabbed(String::new(), Instant::now());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('你'), KeyModifiers::NONE));
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('好'), KeyModifiers::NONE));
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

        assert!(composer.draft.textarea.text().is_empty());
        let _ = flush_after_paste_burst(&mut composer);
        assert_eq!(composer.draft.textarea.text(), "你好\nhi");
    }

    /// Behavior: a paste-like burst may include a full-width/ideographic space (U+3000). It should
    /// still be captured as a single paste payload and preserve the exact Unicode content.
    #[test]
    fn non_ascii_burst_preserves_ideographic_space_and_ascii() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer
            .draft
            .paste_burst
            .begin_with_retro_grabbed(String::new(), Instant::now());

        for ch in ['你', '　', '好'] {
            let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        for ch in ['h', 'i'] {
            let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }

        assert!(composer.draft.textarea.text().is_empty());
        let _ = flush_after_paste_burst(&mut composer);
        assert_eq!(composer.draft.textarea.text(), "你　好\nhi");
    }

    /// Behavior: a large multi-line payload containing both non-ASCII and ASCII (e.g. "UTF-8",
    /// "Unicode") should be captured as a single paste-like burst, and Enter key events should
    /// become `\n` within the buffered content.
    #[test]
    fn non_ascii_burst_buffers_large_multiline_mixed_ascii_and_unicode() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        const LARGE_MIXED_PAYLOAD: &str = "天地玄黄 宇宙洪荒\n\
日月盈昃 辰宿列张\n\
寒来暑往 秋收冬藏\n\
\n\
你好世界 编码测试\n\
汉字处理 UTF-8\n\
终端显示 正确无误\n\
\n\
风吹竹林 月照大江\n\
白云千载 青山依旧\n\
程序员 与 Unicode 同行";

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Force an active burst so the test doesn't depend on timing heuristics.
        composer
            .draft
            .paste_burst
            .begin_with_retro_grabbed(String::new(), Instant::now());

        for ch in LARGE_MIXED_PAYLOAD.chars() {
            let code = if ch == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(ch)
            };
            let _ = composer.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
        }

        assert!(composer.draft.textarea.text().is_empty());
        let _ = flush_after_paste_burst(&mut composer);
        assert_eq!(composer.draft.textarea.text(), LARGE_MIXED_PAYLOAD);
    }

    /// Behavior: while a paste-like burst is active, Enter should not submit; it should insert a
    /// newline into the buffered payload and flush as a single paste later.
    #[test]
    fn ascii_burst_treats_enter_as_newline_even_when_parent_owned() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        for parent_owned in [false, true] {
            let (mut composer, _rx) = new_test_composer();
            if parent_owned {
                composer.set_parent_owned_thread();
            }
            let mut now = Instant::now();
            let step = Duration::from_millis(1);

            let _ = composer.handle_input_basic_with_time(
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
                now,
            );
            now += step;
            let _ = composer.handle_input_basic_with_time(
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
                now,
            );
            now += step;

            let (result, _) =
                composer.handle_submission_with_time(/*should_queue*/ false, now);
            assert!(
                matches!(result, InputResult::None),
                "Enter during a burst should insert newline, not submit"
            );

            for ch in ['t', 'h', 'e', 'r', 'e'] {
                now += step;
                let _ = composer.handle_input_basic_with_time(
                    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                    now,
                );
            }

            assert!(composer.draft.textarea.text().is_empty());
            let flush_time = now + PasteBurst::recommended_active_flush_delay() + step;
            let flushed = composer.handle_paste_burst_flush(flush_time);
            assert!(flushed, "expected paste burst to flush");
            assert_eq!(composer.draft.textarea.text(), "hi\nthere");
        }
    }

    /// Behavior: startup-pending submissions are queued immediately, so Enter should flush any
    /// buffered burst text into that queued message instead of turning into a draft newline.
    #[test]
    fn queued_submission_flushes_ascii_burst_instead_of_inserting_newline() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let mut now = Instant::now();
        let step = Duration::from_millis(1);
        for ch in ['h', 'i'] {
            let _ = composer.handle_input_basic_with_time(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                now,
            );
            now += step;
        }
        assert!(composer.is_in_paste_burst());

        let (result, _) = composer.handle_submission_with_time(/*should_queue*/ true, now);

        assert_eq!(
            result,
            InputResult::Queued {
                text: "hi".to_string(),
                text_elements: Vec::new(),
                action: QueuedInputAction::Plain,
                pending_pastes: Vec::new(),
            }
        );
        assert!(composer.draft.textarea.text().is_empty());
        assert!(!composer.is_in_paste_burst());
    }

    /// Behavior: even if Enter suppression would normally be active for a burst, Enter should
    /// still dispatch a built-in slash command when the first line begins with `/`.
    #[test]
    fn slash_context_enter_ignores_paste_burst_enter_suppression() {
        use crate::slash_command::SlashCommand;
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.draft.textarea.set_text_clearing_elements("/diff");
        composer.draft.textarea.set_cursor("/diff".len());
        composer
            .draft
            .paste_burst
            .begin_with_retro_grabbed(String::new(), Instant::now());

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Command(SlashCommand::Diff)));
    }

    /// Behavior: if a burst is buffering text and the user presses a non-char key, flush the
    /// buffered burst *before* applying that key so the buffer cannot get stuck.
    #[test]
    fn non_char_key_flushes_active_burst_before_input() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Force an active burst so we can deterministically buffer characters without relying on
        // timing.
        composer
            .draft
            .paste_burst
            .begin_with_retro_grabbed(String::new(), Instant::now());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert!(composer.draft.textarea.text().is_empty());
        assert!(composer.is_in_paste_burst());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.text(), "hi");
        assert_eq!(composer.draft.textarea.cursor(), 1);
        assert!(!composer.is_in_paste_burst());
    }

    /// Behavior: enabling `disable_paste_burst` flushes any held first character (flicker
    /// suppression) and then inserts subsequent chars immediately without creating burst state.
    #[test]
    fn disable_paste_burst_flushes_pending_first_char_and_inserts_immediately() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // First ASCII char is normally held briefly. Flip the config mid-stream and ensure the
        // held char is not dropped.
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(composer.is_in_paste_burst());
        assert!(composer.draft.textarea.text().is_empty());

        composer.set_disable_paste_burst(/*disabled*/ true);
        assert_eq!(composer.draft.textarea.text(), "a");
        assert!(!composer.is_in_paste_burst());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.text(), "ab");
        assert!(!composer.is_in_paste_burst());
    }

    /// Behavior: a small explicit paste inserts text directly (no placeholder), and the submitted
    /// text matches what is visible in the textarea.
    #[test]
    fn handle_paste_small_inserts_text() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let sanitized = "_count_rows\tindent\ntwo";
        let needs_redraw =
            composer.handle_paste("_count_r\x1b[13;2:3uows\tindent\n\0two\u{7f}".to_string());
        assert!(needs_redraw);
        assert_eq!(composer.draft.textarea.text(), sanitized);
        assert!(composer.draft.pending_pastes.is_empty());

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted { text, .. } => assert_eq!(text, sanitized),
            _ => panic!("expected Submitted"),
        }
    }

    #[test]
    fn empty_enter_returns_none() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Ensure composer is empty and press Enter.
        assert!(composer.draft.textarea.text().is_empty());
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match result {
            InputResult::None => {}
            other => panic!("expected None for empty enter, got: {other:?}"),
        }
    }

    /// Behavior: a large explicit paste inserts a placeholder into the textarea, stores the full
    /// content in `pending_pastes`, and expands the placeholder to the full content on submit.
    #[test]
    fn handle_paste_large_uses_placeholder_and_replaces_on_submit() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let large = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 10);
        let needs_redraw = composer.handle_paste(large.clone());
        assert!(needs_redraw);
        let placeholder = format!("[Pasted Content {} chars]", large.chars().count());
        assert_eq!(composer.draft.textarea.text(), placeholder);
        assert_eq!(composer.draft.pending_pastes.len(), 1);
        assert_eq!(composer.draft.pending_pastes[0].0, placeholder);
        assert_eq!(composer.draft.pending_pastes[0].1, large);

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted { text, .. } => assert_eq!(text, large),
            _ => panic!("expected Submitted"),
        }
        assert!(composer.draft.pending_pastes.is_empty());
    }

    #[test]
    fn submit_at_character_limit_succeeds() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(true);
        let input = "x".repeat(MAX_USER_INPUT_TEXT_CHARS);
        composer.draft.textarea.set_text_clearing_elements(&input);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(
            result,
            InputResult::Submitted { text, .. } if text == input
        ));
    }

    #[test]
    fn oversized_submit_reports_error_and_restores_draft() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(true);
        let input = "x".repeat(MAX_USER_INPUT_TEXT_CHARS + 1);
        composer.draft.textarea.set_text_clearing_elements(&input);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(InputResult::None, result);
        assert_eq!(composer.draft.textarea.text(), input);

        let mut found_error = false;
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::InsertHistoryCell(cell) = event {
                let message = cell
                    .display_lines(/*width*/ 80)
                    .into_iter()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(message.contains(&user_input_too_large_message(input.chars().count())));
                found_error = true;
                break;
            }
        }
        assert!(found_error, "expected oversized-input error history cell");
    }

    #[test]
    fn oversized_queued_submission_reports_error_and_restores_draft() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(false);
        let input = "x".repeat(MAX_USER_INPUT_TEXT_CHARS + 1);
        composer.draft.textarea.set_text_clearing_elements(&input);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(InputResult::None, result);
        assert_eq!(composer.draft.textarea.text(), input);

        let mut found_error = false;
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::InsertHistoryCell(cell) = event {
                let message = cell
                    .display_lines(/*width*/ 80)
                    .into_iter()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(message.contains(&user_input_too_large_message(input.chars().count())));
                found_error = true;
                break;
            }
        }
        assert!(found_error, "expected oversized-input error history cell");
    }

    /// Behavior: editing that removes a paste placeholder should also clear the associated
    /// `pending_pastes` entry so it cannot be submitted accidentally.
    #[test]
    fn edit_clears_pending_paste() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let large = "y".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1);
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.handle_paste(large);
        assert_eq!(composer.draft.pending_pastes.len(), 1);

        // Any edit that removes the placeholder should clear pending_paste
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(composer.draft.pending_pastes.is_empty());
    }

    #[test]
    fn ui_snapshots() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut terminal = match Terminal::new(TestBackend::new(100, 10)) {
            Ok(t) => t,
            Err(e) => panic!("Failed to create terminal: {e}"),
        };

        let test_cases = vec![
            ("empty", None),
            ("small", Some("short".to_string())),
            ("large", Some("z".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5))),
            ("multiple_pastes", None),
            ("backspace_after_pastes", None),
        ];

        for (name, input) in test_cases {
            // Create a fresh composer for each test case
            let mut composer = ChatComposer::new(
                /*has_input_focus*/ true,
                sender.clone(),
                /*enhanced_keys_supported*/ false,
                "Ask Codex to do anything".to_string(),
                /*disable_paste_burst*/ false,
            );

            if let Some(text) = input {
                composer.handle_paste(text);
            } else if name == "multiple_pastes" {
                // First large paste
                composer.handle_paste("x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 3));
                // Second large paste
                composer.handle_paste("y".repeat(LARGE_PASTE_CHAR_THRESHOLD + 7));
                // Small paste
                composer.handle_paste(" another short paste".to_string());
            } else if name == "backspace_after_pastes" {
                // Three large pastes
                composer.handle_paste("a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 2));
                composer.handle_paste("b".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4));
                composer.handle_paste("c".repeat(LARGE_PASTE_CHAR_THRESHOLD + 6));
                // Move cursor to end and press backspace
                composer
                    .draft
                    .textarea
                    .set_cursor(composer.draft.textarea.text().len());
                composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
            }

            terminal
                .draw(|f| composer.render(f.area(), f.buffer_mut()))
                .unwrap_or_else(|e| panic!("Failed to draw {name} composer: {e}"));

            insta::assert_snapshot!(name, terminal.backend());
        }
    }

    #[test]
    fn image_placeholder_snapshots() {
        snapshot_composer_state(
            "image_placeholder_single",
            /*enhanced_keys_supported*/ false,
            |composer| {
                composer.attach_image(PathBuf::from("/tmp/image1.png"));
            },
        );

        snapshot_composer_state(
            "image_placeholder_multiple",
            /*enhanced_keys_supported*/ false,
            |composer| {
                composer.attach_image(PathBuf::from("/tmp/image1.png"));
                composer.attach_image(PathBuf::from("/tmp/image2.png"));
            },
        );
    }

    #[test]
    fn remote_image_rows_snapshots() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        snapshot_composer_state(
            "remote_image_rows",
            /*enhanced_keys_supported*/ false,
            |composer| {
                composer.set_remote_image_urls(vec![
                    "https://example.com/one.png".to_string(),
                    "https://example.com/two.png".to_string(),
                ]);
                composer.set_text_content("describe these".to_string(), Vec::new(), Vec::new());
            },
        );

        snapshot_composer_state(
            "remote_image_rows_after_delete_first",
            /*enhanced_keys_supported*/ false,
            |composer| {
                composer.set_remote_image_urls(vec![
                    "https://example.com/one.png".to_string(),
                    "https://example.com/two.png".to_string(),
                ]);
                composer.set_text_content("describe these".to_string(), Vec::new(), Vec::new());
                composer.draft.textarea.set_cursor(/*pos*/ 0);
                let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
                let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
                let _ =
                    composer.handle_key_event(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
            },
        );
    }
    fn service_tier_slash_command_dispatches_from_catalog_name() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_service_tier_commands_enabled(/*enabled*/ true);
        composer.set_service_tier_commands(vec![ServiceTierCommand {
            id: "priority".to_string(),
            name: "fast".to_string(),
            description: "Fastest inference with increased plan usage".to_string(),
        }]);
        type_chars_humanlike(&mut composer, &['/', 'f', 'a', 's', 't']);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            result,
            InputResult::ServiceTierCommand(ServiceTierCommand {
                id: "priority".to_string(),
                name: "fast".to_string(),
                description: "Fastest inference with increased plan usage".to_string(),
            })
        );
    }

    fn flush_after_paste_burst(composer: &mut ChatComposer) -> bool {
        std::thread::sleep(PasteBurst::recommended_active_flush_delay());
        composer.flush_paste_burst_if_due()
    }

    // Test helper: simulate human typing with a brief delay and flush the paste-burst buffer
    pub(super) fn type_chars_humanlike(composer: &mut ChatComposer, chars: &[char]) {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyEventKind;
        use crossterm::event::KeyModifiers;
        for &ch in chars {
            let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            std::thread::sleep(ChatComposer::recommended_paste_flush_delay());
            let _ = composer.flush_paste_burst_if_due();
            if ch == ' ' {
                let _ = composer.handle_key_event(KeyEvent::new_with_kind(
                    KeyCode::Char(' '),
                    KeyModifiers::NONE,
                    KeyEventKind::Release,
                ));
            }
        }
    }

    #[test]
    fn slash_init_dispatches_command_and_does_not_submit_literal_text() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Type the slash command.
        type_chars_humanlike(&mut composer, &['/', 'i', 'n', 'i', 't']);

        // Press Enter to dispatch the selected command.
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        // When a slash command is dispatched, the composer should return a
        // Command result (not submit literal text) and clear its textarea.
        match result {
            InputResult::Command(cmd) => {
                assert_eq!(cmd.command(), "init");
            }
            InputResult::CommandWithArgs(_, _, _) => {
                panic!("expected command dispatch without args for '/init'")
            }
            InputResult::ServiceTierCommand(command) => {
                panic!("expected init command, got service tier {command:?}")
            }
            InputResult::Submitted { text, .. } => {
                panic!("expected command dispatch, but composer submitted literal text: {text}")
            }
            InputResult::Queued { .. } => {
                panic!("expected command dispatch, but composer queued literal text")
            }
            InputResult::ParentOwnedInputBlocked => {
                panic!("expected command dispatch, but parent-owned input was blocked")
            }
            InputResult::None => panic!("expected Command result for '/init'"),
        }
        assert!(
            composer.draft.textarea.is_empty(),
            "composer should be cleared"
        );
    }

    #[test]
    fn kill_buffer_persists_after_submit() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_steer_enabled(true);
        composer.draft.textarea.insert_str("restore me");
        composer.draft.textarea.set_cursor(/*pos*/ 0);

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(composer.draft.textarea.is_empty());

        composer.draft.textarea.insert_str("hello");
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));
        assert!(composer.draft.textarea.is_empty());

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert_eq!(composer.draft.textarea.text(), "restore me");
    }

    #[test]
    fn kill_buffer_persists_after_slash_command_dispatch() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.draft.textarea.insert_str("restore me");
        composer.draft.textarea.set_cursor(/*pos*/ 0);

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(composer.draft.textarea.is_empty());

        composer.draft.textarea.insert_str("/diff");
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Command(cmd) => {
                assert_eq!(cmd.command(), "diff");
            }
            _ => panic!("expected Command result for '/diff'"),
        }
        assert!(composer.draft.textarea.is_empty());

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert_eq!(composer.draft.textarea.text(), "restore me");
    }

    #[test]
    fn slash_command_disabled_while_task_running_keeps_text() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_task_running(/*running*/ true);
        composer
            .draft
            .textarea
            .set_text_clearing_elements("/compact");

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(InputResult::None, result);
        assert_eq!("/compact", composer.draft.textarea.text());

        let mut found_error = false;
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::InsertHistoryCell(cell) = event {
                let message = cell
                    .display_lines(/*width*/ 80)
                    .into_iter()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(message.contains("disabled while a task is in progress"));
                found_error = true;
                break;
            }
        }
        assert!(found_error, "expected error history cell to be sent");
    }

    #[test]
    fn enter_queues_when_queue_submissions_is_enabled() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_queue_submissions(/*queue_submissions*/ true);
        composer
            .draft
            .textarea
            .set_text_clearing_elements("queued before session");

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            result,
            InputResult::Queued {
                text: "queued before session".to_string(),
                text_elements: Vec::new(),
                action: QueuedInputAction::Plain,
                pending_pastes: Vec::new(),
            }
        );
    }

    #[test]
    fn tab_queues_slash_led_prompts_while_task_running_without_validation() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        fn assert_queued_slash(input: &str) {
            let (tx, mut rx) = unbounded_channel::<AppEvent>();
            let sender = AppEventSender::new(tx);
            let mut composer = ChatComposer::new(
                /*has_input_focus*/ true,
                sender,
                /*enhanced_keys_supported*/ false,
                "Ask Codex to do anything".to_string(),
                /*disable_paste_burst*/ false,
            );
            composer.set_task_running(/*running*/ true);
            composer.draft.textarea.set_text_clearing_elements(input);

            let (result, _needs_redraw) =
                composer.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

            match result {
                InputResult::Queued {
                    text,
                    text_elements,
                    action,
                    ..
                } => {
                    assert_eq!(text, input);
                    assert!(text_elements.is_empty());
                    assert_eq!(action, QueuedInputAction::ParseSlash);
                }
                other => panic!("expected slash-led input to queue, got {other:?}"),
            }
            assert!(composer.draft.textarea.is_empty());
            assert!(
                rx.try_recv().is_err(),
                "queueing should not report slash errors"
            );
        }

        assert_queued_slash("/compact");
        assert_queued_slash("/review check regressions");
        assert_queued_slash("/fast");
        assert_queued_slash("/does-not-exist");
    }

    #[test]
    fn remapped_submit_does_not_fall_back_to_enter() {
        use crate::key_hint;
        use crate::keymap::RuntimeKeymap;
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer
            .draft
            .textarea
            .set_text_clearing_elements("explain the change");
        composer
            .draft
            .textarea
            .set_cursor(composer.draft.textarea.text().len());
        let mut keymap = RuntimeKeymap::defaults();
        keymap.composer.submit = vec![key_hint::ctrl(KeyCode::Char('j'))];
        composer.set_keymap_bindings(&keymap);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(InputResult::None, result);
        assert_eq!("explain the change\n", composer.draft.textarea.text());
    }

    #[test]
    fn remapped_queue_does_not_fall_back_to_tab() {
        use crate::key_hint;
        use crate::keymap::RuntimeKeymap;
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_task_running(/*running*/ true);
        composer
            .draft
            .textarea
            .set_text_clearing_elements("queue me");
        let mut keymap = RuntimeKeymap::defaults();
        keymap.composer.queue = vec![key_hint::ctrl(KeyCode::Char('q'))];
        composer.set_keymap_bindings(&keymap);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(InputResult::None, result);
        assert_eq!("queue me", composer.draft.textarea.text());
    }

    #[test]
    fn remapped_history_search_does_not_fall_back_to_ctrl_r() {
        use crate::key_hint;
        use crate::keymap::RuntimeKeymap;
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let mut keymap = RuntimeKeymap::defaults();
        keymap.composer.history_search_previous = vec![key_hint::plain(KeyCode::F(2))];
        composer.set_keymap_bindings(&keymap);

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(!composer.history_search_active());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert!(composer.history_search_active());
    }

    #[test]
    fn tab_queues_leading_space_slash_as_plain_text_while_task_running() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_task_running(/*running*/ true);
        composer
            .draft
            .textarea
            .set_text_clearing_elements(" /does-not-exist");

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        match result {
            InputResult::Queued { text, action, .. } => {
                assert_eq!(text, "/does-not-exist");
                assert_eq!(action, QueuedInputAction::Plain);
            }
            other => panic!("expected leading-space slash input to queue, got {other:?}"),
        }
    }

    #[test]
    fn tab_queues_bang_shell_prompts_while_task_running_without_execution() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        fn assert_queued_shell(input: &str, expected_text: &str) {
            let (tx, mut rx) = unbounded_channel::<AppEvent>();
            let sender = AppEventSender::new(tx);
            let mut composer = ChatComposer::new(
                /*has_input_focus*/ true,
                sender,
                /*enhanced_keys_supported*/ false,
                "Ask Codex to do anything".to_string(),
                /*disable_paste_burst*/ false,
            );
            composer.set_task_running(/*running*/ true);
            composer.draft.textarea.set_text_clearing_elements(input);

            let (result, _needs_redraw) =
                composer.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

            match result {
                InputResult::Queued {
                    text,
                    text_elements,
                    action,
                    ..
                } => {
                    assert_eq!(text, expected_text);
                    assert!(text_elements.is_empty());
                    assert_eq!(action, QueuedInputAction::RunShell);
                }
                other => panic!("expected bang shell input to queue, got {other:?}"),
            }
            assert!(composer.draft.textarea.is_empty());
            assert!(
                rx.try_recv().is_err(),
                "queueing should not show shell help immediately"
            );
        }

        assert_queued_shell("!echo hi", "!echo hi");
        assert_queued_shell("!", "!");
        assert_queued_shell(" !echo hi", "!echo hi");
    }

    #[test]
    fn slash_command_elementizes_on_space() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_collaboration_modes_enabled(/*enabled*/ true);

        type_chars_humanlike(&mut composer, &['/', 'p', 'l', 'a', 'n', ' ']);

        let text = composer.draft.textarea.text().to_string();
        let elements = composer.draft.textarea.text_elements();
        assert_eq!(text, "/plan ");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].placeholder(&text), Some("/plan"));
    }

    #[test]
    fn slash_command_elementizes_only_known_commands() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_collaboration_modes_enabled(/*enabled*/ true);

        type_chars_humanlike(&mut composer, &['/', 'U', 's', 'e', 'r', 's', ' ']);

        let text = composer.draft.textarea.text().to_string();
        let elements = composer.draft.textarea.text_elements();
        assert_eq!(text, "/Users ");
        assert!(elements.is_empty());
    }

    #[test]
    fn slash_command_element_removed_when_not_at_start() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['/', 'r', 'e', 'v', 'i', 'e', 'w', ' ']);

        let text = composer.draft.textarea.text().to_string();
        let elements = composer.draft.textarea.text_elements();
        assert_eq!(text, "/review ");
        assert_eq!(elements.len(), 1);

        composer.draft.textarea.set_cursor(/*pos*/ 0);
        type_chars_humanlike(&mut composer, &['x']);

        let text = composer.draft.textarea.text().to_string();
        let elements = composer.draft.textarea.text_elements();
        assert_eq!(text, "x/review ");
        assert!(elements.is_empty());
    }

    #[test]
    fn tab_submits_when_no_task_running() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['h', 'i']);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert!(matches!(
            result,
            InputResult::Submitted { ref text, .. } if text == "hi"
        ));
        assert!(composer.draft.textarea.is_empty());
    }

    #[test]
    fn tab_does_not_submit_for_bang_shell_command() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_task_running(/*running*/ false);

        type_chars_humanlike(&mut composer, &['!', 'l', 's']);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert!(matches!(result, InputResult::None));
        assert!(
            composer.current_text().starts_with("!ls"),
            "expected Tab not to submit or clear a `!` command"
        );
    }

    #[test]
    fn bang_prefixed_slash_text_submits_literal_shell_command() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['!', '/', 'd', 'i', 'f', 'f']);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(
            result,
            InputResult::Submitted { ref text, .. } if text == "!/diff"
        ));
    }

    #[test]
    fn slash_mention_dispatches_command_and_inserts_at() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['/', 'm', 'e', 'n', 't', 'i', 'o', 'n']);

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match result {
            InputResult::Command(cmd) => {
                assert_eq!(cmd.command(), "mention");
            }
            InputResult::CommandWithArgs(_, _, _) => {
                panic!("expected command dispatch without args for '/mention'")
            }
            InputResult::ServiceTierCommand(command) => {
                panic!("expected mention command, got service tier {command:?}")
            }
            InputResult::Submitted { text, .. } => {
                panic!("expected command dispatch, but composer submitted literal text: {text}")
            }
            InputResult::Queued { .. } => {
                panic!("expected command dispatch, but composer queued literal text")
            }
            InputResult::ParentOwnedInputBlocked => {
                panic!("expected command dispatch, but parent-owned input was blocked")
            }
            InputResult::None => panic!("expected Command result for '/mention'"),
        }
        assert!(
            composer.draft.textarea.is_empty(),
            "composer should be cleared"
        );
        composer.insert_str("@");
        assert_eq!(composer.draft.textarea.text(), "@");
    }

    #[test]
    fn slash_plan_args_preserve_text_elements() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_collaboration_modes_enabled(/*enabled*/ true);

        type_chars_humanlike(&mut composer, &['/', 'p', 'l', 'a', 'n', ' ']);
        let placeholder = local_image_label_text(/*label_number*/ 1);
        composer.attach_image(PathBuf::from("/tmp/plan.png"));

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match result {
            InputResult::CommandWithArgs(cmd, args, text_elements) => {
                assert_eq!(cmd.command(), "plan");
                assert_eq!(args, placeholder);
                assert_eq!(text_elements.len(), 1);
                assert_eq!(
                    text_elements[0].placeholder(&args),
                    Some(placeholder.as_str())
                );
            }
            _ => panic!("expected CommandWithArgs for /plan with args"),
        }
    }
    // 008-TRIM: fn complete_file 测试辅助（file search 补全，008 §1 裁剪：无本地事件源）
    /// Behavior: multiple paste operations can coexist; placeholders should be expanded to their
    /// original content on submission.
    #[test]
    fn test_multiple_pastes_submission() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Define test cases: (paste content, is_large)
        let test_cases = [
            ("x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 3), true),
            (" and ".to_string(), false),
            ("y".repeat(LARGE_PASTE_CHAR_THRESHOLD + 7), true),
        ];

        // Expected states after each paste
        let mut expected_text = String::new();
        let mut expected_pending_count = 0;

        // Apply all pastes and build expected state
        let states: Vec<_> = test_cases
            .iter()
            .map(|(content, is_large)| {
                composer.handle_paste(content.clone());
                if *is_large {
                    let placeholder = format!("[Pasted Content {} chars]", content.chars().count());
                    expected_text.push_str(&placeholder);
                    expected_pending_count += 1;
                } else {
                    expected_text.push_str(content);
                }
                (expected_text.clone(), expected_pending_count)
            })
            .collect();

        // Verify all intermediate states were correct
        assert_eq!(
            states,
            vec![
                (
                    format!("[Pasted Content {} chars]", test_cases[0].0.chars().count()),
                    1
                ),
                (
                    format!(
                        "[Pasted Content {} chars] and ",
                        test_cases[0].0.chars().count()
                    ),
                    1
                ),
                (
                    format!(
                        "[Pasted Content {} chars] and [Pasted Content {} chars]",
                        test_cases[0].0.chars().count(),
                        test_cases[2].0.chars().count()
                    ),
                    2
                ),
            ]
        );

        // Submit and verify final expansion
        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        if let InputResult::Submitted { text, .. } = result {
            assert_eq!(text, format!("{} and {}", test_cases[0].0, test_cases[2].0));
        } else {
            panic!("expected Submitted");
        }
    }

    #[test]
    fn test_placeholder_deletion() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Define test cases: (content, is_large)
        let test_cases = [
            ("a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5), true),
            (" and ".to_string(), false),
            ("b".repeat(LARGE_PASTE_CHAR_THRESHOLD + 6), true),
        ];

        // Apply all pastes
        let mut current_pos = 0;
        let states: Vec<_> = test_cases
            .iter()
            .map(|(content, is_large)| {
                composer.handle_paste(content.clone());
                if *is_large {
                    let placeholder = format!("[Pasted Content {} chars]", content.chars().count());
                    current_pos += placeholder.len();
                } else {
                    current_pos += content.len();
                }
                (
                    composer.draft.textarea.text().to_string(),
                    composer.draft.pending_pastes.len(),
                    current_pos,
                )
            })
            .collect();

        // Delete placeholders one by one and collect states
        let mut deletion_states = vec![];

        // First deletion
        composer.draft.textarea.set_cursor(states[0].2);
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        deletion_states.push((
            composer.draft.textarea.text().to_string(),
            composer.draft.pending_pastes.len(),
        ));

        // Second deletion
        composer
            .draft
            .textarea
            .set_cursor(composer.draft.textarea.text().len());
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        deletion_states.push((
            composer.draft.textarea.text().to_string(),
            composer.draft.pending_pastes.len(),
        ));

        // Verify all states
        assert_eq!(
            deletion_states,
            vec![
                (" and [Pasted Content 1006 chars]".to_string(), 1),
                (" and ".to_string(), 0),
            ]
        );
    }

    /// Behavior: if multiple large pastes share the same placeholder label (same char count),
    /// deleting one placeholder removes only its corresponding `pending_pastes` entry.
    #[test]
    fn deleting_duplicate_length_pastes_removes_only_target() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
        let placeholder_base = format!("[Pasted Content {} chars]", paste.chars().count());
        let placeholder_second = format!("{placeholder_base} #2");

        composer.handle_paste(paste.clone());
        composer.handle_paste(paste.clone());
        assert_eq!(
            composer.draft.textarea.text(),
            format!("{placeholder_base}{placeholder_second}")
        );
        assert_eq!(composer.draft.pending_pastes.len(), 2);

        composer
            .draft
            .textarea
            .set_cursor(composer.draft.textarea.text().len());
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        assert_eq!(composer.draft.textarea.text(), placeholder_base);
        assert_eq!(composer.draft.pending_pastes.len(), 1);
        assert_eq!(composer.draft.pending_pastes[0].0, placeholder_base);
        assert_eq!(composer.draft.pending_pastes[0].1, paste);
    }

    /// Behavior: large-paste placeholder numbering continues when another placeholder of the
    /// same length still exists, so a new paste gets a new unique placeholder label.
    #[test]
    fn large_paste_numbering_continues_with_same_length_placeholder() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
        let base = format!("[Pasted Content {} chars]", paste.chars().count());
        let second = format!("{base} #2");
        let third = format!("{base} #3");

        composer.handle_paste(paste.clone());
        composer.handle_paste(paste.clone());
        assert_eq!(composer.draft.textarea.text(), format!("{base}{second}"));

        composer.draft.textarea.set_cursor(base.len());
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.text(), second);
        assert_eq!(composer.draft.pending_pastes.len(), 1);
        assert_eq!(composer.draft.pending_pastes[0].0, second);

        composer
            .draft
            .textarea
            .set_cursor(composer.draft.textarea.text().len());
        composer.handle_paste(paste);

        assert_eq!(composer.draft.textarea.text(), format!("{second}{third}"));
        assert_eq!(composer.draft.pending_pastes.len(), 2);
        assert_eq!(composer.draft.pending_pastes[0].0, second);
        assert_eq!(composer.draft.pending_pastes[1].0, third);
    }

    /// Behavior: if all placeholders of a given length are removed, numbering resets to the
    /// base placeholder on the next paste.
    #[test]
    fn large_paste_numbering_reuses_after_all_deleted() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
        let base = format!("[Pasted Content {} chars]", paste.chars().count());

        composer.handle_paste(paste.clone());
        assert_eq!(composer.draft.textarea.text(), base);
        assert_eq!(composer.draft.pending_pastes.len(), 1);

        composer
            .draft
            .textarea
            .set_cursor(composer.draft.textarea.text().len());
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(composer.draft.textarea.text().is_empty());
        assert!(composer.draft.pending_pastes.is_empty());

        composer.handle_paste(paste);
        assert_eq!(composer.draft.textarea.text(), base);
        assert_eq!(composer.draft.pending_pastes.len(), 1);
        assert_eq!(composer.draft.pending_pastes[0].0, base);
    }

    #[test]
    fn test_partial_placeholder_deletion() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Define test cases: (cursor_position_from_end, expected_pending_count)
        let test_cases = [
            5, // Delete from middle - should clear tracking
            0, // Delete from end - should clear tracking
        ];

        let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
        let placeholder = format!("[Pasted Content {} chars]", paste.chars().count());

        let states: Vec<_> = test_cases
            .into_iter()
            .map(|pos_from_end| {
                composer.handle_paste(paste.clone());
                composer
                    .draft
                    .textarea
                    .set_cursor(placeholder.len() - pos_from_end);
                composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
                let result = (
                    composer.draft.textarea.text().contains(&placeholder),
                    composer.draft.pending_pastes.len(),
                );
                composer.draft.textarea.set_text_clearing_elements("");
                result
            })
            .collect();

        assert_eq!(
            states,
            vec![
                (false, 0), // After deleting from middle
                (false, 0), // After deleting from end
            ]
        );
    }

    // --- Image attachment tests ---
    #[test]
    fn attach_image_and_submit_includes_local_image_paths() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let path = PathBuf::from("/tmp/image1.png");
        composer.attach_image(path.clone());
        composer.handle_paste(" hi".into());
        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                assert_eq!(text, "[Image #1] hi");
                assert_eq!(text_elements.len(), 1);
                assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
                assert_eq!(
                    text_elements[0].byte_range,
                    ByteRange {
                        start: 0,
                        end: "[Image #1]".len()
                    }
                );
            }
            _ => panic!("expected Submitted"),
        }
        let imgs = composer.take_recent_submission_images();
        assert_eq!(vec![path], imgs);
    }

    #[test]
    fn submit_captures_recent_mention_bindings_before_clearing_textarea() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let mention_bindings = vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "/tmp/user/figma/SKILL.md".to_string(),
        }];
        composer.set_text_content_with_mention_bindings(
            "$figma please".to_string(),
            Vec::new(),
            Vec::new(),
            mention_bindings.clone(),
        );

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));
        assert_eq!(
            composer.take_recent_submission_mention_bindings(),
            mention_bindings
        );
        assert!(composer.take_mention_bindings().is_empty());
    }

    #[test]
    fn history_navigation_restores_remote_and_local_image_attachments() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let remote_image_url = "https://example.com/remote.png".to_string();
        composer.set_remote_image_urls(vec![remote_image_url.clone()]);
        let path = PathBuf::from("/tmp/image1.png");
        composer.attach_image(path.clone());

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        let _ = composer.take_remote_image_urls();
        composer.set_text_content(String::new(), Vec::new(), Vec::new());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

        let text = composer.current_text();
        assert_eq!(text, "[Image #2]");
        let text_elements = composer.text_elements();
        assert_eq!(text_elements.len(), 1);
        assert_eq!(text_elements[0].placeholder(&text), Some("[Image #2]"));
        assert_eq!(composer.local_image_paths(), vec![path]);
        assert_eq!(composer.remote_image_urls(), vec![remote_image_url]);
    }

    #[test]
    fn history_navigation_restores_remote_only_submissions() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let remote_image_urls = vec![
            "https://example.com/one.png".to_string(),
            "https://example.com/two.png".to_string(),
        ];
        composer.set_remote_image_urls(remote_image_urls.clone());

        let (submitted_text, submitted_elements) = composer
            .prepare_submission_text(/*record_history*/ true)
            .expect("remote-only submission should be prepared");
        assert_eq!(submitted_text, "");
        assert!(submitted_elements.is_empty());

        let _ = composer.take_remote_image_urls();
        composer.set_text_content(String::new(), Vec::new(), Vec::new());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(composer.current_text(), "");
        assert!(composer.text_elements().is_empty());
        assert_eq!(composer.remote_image_urls(), remote_image_urls);
    }

    #[test]
    fn history_navigation_leaves_cursor_at_end_of_line() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['f', 'i', 'r', 's', 't']);
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        type_chars_humanlike(&mut composer, &['s', 'e', 'c', 'o', 'n', 'd']);
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.text(), "second");
        assert_eq!(
            composer.draft.textarea.cursor(),
            composer.draft.textarea.text().len()
        );

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.text(), "first");
        assert_eq!(
            composer.draft.textarea.cursor(),
            composer.draft.textarea.text().len()
        );

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.text(), "second");
        assert_eq!(
            composer.draft.textarea.cursor(),
            composer.draft.textarea.text().len()
        );

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(composer.draft.textarea.is_empty());
        assert_eq!(
            composer.draft.textarea.cursor(),
            composer.draft.textarea.text().len()
        );
    }

    #[test]
    fn vim_normal_operator_pending_consumes_submit_key() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_text_content("hello".to_string(), Vec::new(), Vec::new());
        composer.set_vim_enabled(/*enabled*/ true);

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(composer.draft.textarea.is_vim_operator_pending());

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(result, InputResult::None));
        assert_eq!(composer.draft.textarea.text(), "hello");
        assert_eq!(
            composer.vim_mode_indicator_span(),
            Some("Vim: Normal".magenta())
        );
        assert!(!composer.draft.textarea.is_vim_operator_pending());
    }

    #[test]
    fn remapped_editor_history_navigation_does_not_fall_back_to_up() {
        use crate::key_hint;
        use crate::keymap::RuntimeKeymap;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['f', 'i', 'r', 's', 't']);
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        let mut keymap = RuntimeKeymap::defaults();
        Arc::make_mut(&mut keymap.editor).move_up = vec![key_hint::plain(KeyCode::F(2))];
        composer.set_keymap_bindings(&keymap);
        assert!(Arc::ptr_eq(&composer.editor_keymap, &keymap.editor));
        assert_eq!(Arc::strong_count(&keymap.editor), 3);

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(composer.draft.textarea.is_empty());

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE));
        assert_eq!(composer.draft.textarea.text(), "first");
    }

    #[test]
    fn history_navigation_from_start_of_bang_command_recalls_older_entry() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['f', 'i', 'r', 's', 't']);
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        type_chars_humanlike(&mut composer, &['!', 'g', 'i', 't']);
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(composer.current_text(), "!git");

        composer.draft.textarea.set_cursor(/*pos*/ 0);
        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(composer.current_text(), "first");
    }

    #[test]
    fn vim_normal_history_navigation_from_start_of_bang_command_recalls_older_entry() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        type_chars_humanlike(&mut composer, &['f', 'i', 'r', 's', 't']);
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        type_chars_humanlike(&mut composer, &['!', 'g', 'i', 't']);
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::Submitted { .. }));

        composer.set_vim_enabled(/*enabled*/ true);

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(composer.current_text(), "!git");
        assert_eq!(composer.draft.textarea.cursor(), "git".len() - 1);

        let (_result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(composer.current_text(), "first");
        assert_eq!(composer.draft.textarea.cursor(), "first".len() - 1);
    }

    #[test]
    fn set_text_content_reattaches_images_without_placeholder_metadata() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let placeholder = local_image_label_text(/*label_number*/ 1);
        let text = format!("{placeholder} restored");
        let text_elements = vec![TextElement::new(
            (0..placeholder.len()).into(),
            /*placeholder*/ None,
        )];
        let path = PathBuf::from("/tmp/image1.png");

        composer.set_text_content(text, text_elements, vec![path.clone()]);

        assert_eq!(composer.local_image_paths(), vec![path]);
    }

    #[test]
    fn large_paste_preserves_image_text_elements_on_submit() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let large_content = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5);
        composer.handle_paste(large_content.clone());
        composer.handle_paste(" ".into());
        let path = PathBuf::from("/tmp/image_with_paste.png");
        composer.attach_image(path.clone());

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                let expected = format!("{large_content} [Image #1]");
                assert_eq!(text, expected);
                assert_eq!(text_elements.len(), 1);
                assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
                assert_eq!(
                    text_elements[0].byte_range,
                    ByteRange {
                        start: large_content.len() + 1,
                        end: large_content.len() + 1 + "[Image #1]".len(),
                    }
                );
            }
            _ => panic!("expected Submitted"),
        }
        let imgs = composer.take_recent_submission_images();
        assert_eq!(vec![path], imgs);
    }

    #[test]
    fn large_paste_with_leading_whitespace_trims_and_shifts_elements() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let large_content = format!("  {}", "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5));
        composer.handle_paste(large_content.clone());
        composer.handle_paste(" ".into());
        let path = PathBuf::from("/tmp/image_with_trim.png");
        composer.attach_image(path.clone());

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                let trimmed = large_content.trim().to_string();
                assert_eq!(text, format!("{trimmed} [Image #1]"));
                assert_eq!(text_elements.len(), 1);
                assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
                assert_eq!(
                    text_elements[0].byte_range,
                    ByteRange {
                        start: trimmed.len() + 1,
                        end: trimmed.len() + 1 + "[Image #1]".len(),
                    }
                );
            }
            _ => panic!("expected Submitted"),
        }
        let imgs = composer.take_recent_submission_images();
        assert_eq!(vec![path], imgs);
    }

    #[test]
    fn pasted_crlf_normalizes_newlines_for_elements() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let pasted = "line1\r\nline2\r\n".to_string();
        composer.handle_paste(pasted);
        composer.handle_paste(" ".into());
        let path = PathBuf::from("/tmp/image_crlf.png");
        composer.attach_image(path.clone());

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                assert_eq!(text, "line1\nline2\n [Image #1]");
                assert!(!text.contains('\r'));
                assert_eq!(text_elements.len(), 1);
                assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
                assert_eq!(
                    text_elements[0].byte_range,
                    ByteRange {
                        start: "line1\nline2\n ".len(),
                        end: "line1\nline2\n [Image #1]".len(),
                    }
                );
            }
            _ => panic!("expected Submitted"),
        }
        let imgs = composer.take_recent_submission_images();
        assert_eq!(vec![path], imgs);
    }

    #[test]
    fn suppressed_submission_restores_pending_paste_payload() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer
            .draft
            .textarea
            .set_text_clearing_elements("/unknown ");
        composer.draft.textarea.set_cursor("/unknown ".len());
        let large_content = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5);
        composer.handle_paste(large_content.clone());
        let placeholder = composer
            .draft
            .pending_pastes
            .first()
            .expect("expected pending paste")
            .0
            .clone();

        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, InputResult::None));
        assert_eq!(composer.draft.pending_pastes.len(), 1);
        assert_eq!(
            composer.draft.textarea.text(),
            format!("/unknown {placeholder}")
        );

        composer.draft.textarea.set_cursor(/*pos*/ 0);
        composer.draft.textarea.insert_str(" ");
        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                assert_eq!(text, format!("/unknown {large_content}"));
                assert!(text_elements.is_empty());
            }
            _ => panic!("expected Submitted"),
        }
        assert!(composer.draft.pending_pastes.is_empty());
    }

    #[test]
    fn attach_image_without_text_submits_empty_text_and_images() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let path = PathBuf::from("/tmp/image2.png");
        composer.attach_image(path.clone());
        let (result, _) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            } => {
                assert_eq!(text, "[Image #1]");
                assert_eq!(text_elements.len(), 1);
                assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
                assert_eq!(
                    text_elements[0].byte_range,
                    ByteRange {
                        start: 0,
                        end: "[Image #1]".len()
                    }
                );
            }
            _ => panic!("expected Submitted"),
        }
        let imgs = composer.take_recent_submission_images();
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0], path);
        assert!(composer.attachments.local_images.is_empty());
    }

    #[test]
    fn duplicate_image_placeholders_get_suffix() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let path = PathBuf::from("/tmp/image_dup.png");
        composer.attach_image(path.clone());
        composer.handle_paste(" ".into());
        composer.attach_image(path);

        let text = composer.draft.textarea.text().to_string();
        assert!(text.contains("[Image #1]"));
        assert!(text.contains("[Image #2]"));
        assert_eq!(
            composer.attachments.local_images[0].placeholder,
            "[Image #1]"
        );
        assert_eq!(
            composer.attachments.local_images[1].placeholder,
            "[Image #2]"
        );
    }

    #[test]
    fn image_placeholder_backspace_behaves_like_text_placeholder() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        let path = PathBuf::from("/tmp/image3.png");
        composer.attach_image(path.clone());
        let placeholder = composer.attachments.local_images[0].placeholder.clone();

        // Case 1: backspace at end
        composer
            .draft
            .textarea
            .move_cursor_to_end_of_line(/*move_down_at_eol*/ false);
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(!composer.draft.textarea.text().contains(&placeholder));
        assert!(composer.attachments.local_images.is_empty());

        // Re-add and ensure backspace at element start does not delete the placeholder.
        composer.attach_image(path);
        let placeholder2 = composer.attachments.local_images[0].placeholder.clone();
        // Move cursor to roughly middle of placeholder
        if let Some(start_pos) = composer.draft.textarea.text().find(&placeholder2) {
            let mid_pos = start_pos + (placeholder2.len() / 2);
            composer.draft.textarea.set_cursor(mid_pos);
            composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
            assert!(composer.draft.textarea.text().contains(&placeholder2));
            assert_eq!(composer.attachments.local_images.len(), 1);
        } else {
            panic!("Placeholder not found in textarea");
        }
    }

    #[test]
    fn backspace_with_multibyte_text_before_placeholder_does_not_panic() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Insert an image placeholder at the start
        let path = PathBuf::from("/tmp/image_multibyte.png");
        composer.attach_image(path);
        // Add multibyte text after the placeholder
        composer.draft.textarea.insert_str("日本語");

        // Cursor is at end; pressing backspace should delete the last character
        // without panicking and leave the placeholder intact.
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        assert_eq!(composer.attachments.local_images.len(), 1);
        assert!(composer.draft.textarea.text().starts_with("[Image #1]"));
    }

    #[test]
    fn deleting_one_of_duplicate_image_placeholders_removes_one_entry() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let path1 = PathBuf::from("/tmp/image_dup1.png");
        let path2 = PathBuf::from("/tmp/image_dup2.png");

        composer.attach_image(path1);
        // separate placeholders with a space for clarity
        composer.handle_paste(" ".into());
        composer.attach_image(path2.clone());

        let placeholder1 = composer.attachments.local_images[0].placeholder.clone();
        let placeholder2 = composer.attachments.local_images[1].placeholder.clone();
        let text = composer.draft.textarea.text().to_string();
        let start1 = text.find(&placeholder1).expect("first placeholder present");
        let end1 = start1 + placeholder1.len();
        composer.draft.textarea.set_cursor(end1);

        // Backspace should delete the first placeholder and its mapping.
        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        let new_text = composer.draft.textarea.text().to_string();
        assert_eq!(
            1,
            new_text.matches(&placeholder1).count(),
            "one placeholder remains after deletion"
        );
        assert_eq!(
            0,
            new_text.matches(&placeholder2).count(),
            "second placeholder was relabeled"
        );
        assert_eq!(
            1,
            new_text.matches("[Image #1]").count(),
            "remaining placeholder relabeled to #1"
        );
        assert_eq!(
            vec![AttachedImage {
                path: path2,
                placeholder: "[Image #1]".to_string()
            }],
            composer.attachments.local_images,
            "one image mapping remains"
        );
    }

    #[test]
    fn deleting_reordered_image_one_renumbers_text_in_place() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let path1 = PathBuf::from("/tmp/image_first.png");
        let path2 = PathBuf::from("/tmp/image_second.png");
        let placeholder1 = local_image_label_text(/*label_number*/ 1);
        let placeholder2 = local_image_label_text(/*label_number*/ 2);

        // Placeholders can be reordered in the text buffer; deleting image #1 should renumber
        // image #2 wherever it appears, not just after the cursor.
        let text = format!("Test {placeholder2} test {placeholder1}");
        let start2 = text.find(&placeholder2).expect("placeholder2 present");
        let start1 = text.find(&placeholder1).expect("placeholder1 present");
        let text_elements = vec![
            TextElement::new(
                ByteRange {
                    start: start2,
                    end: start2 + placeholder2.len(),
                },
                Some(placeholder2),
            ),
            TextElement::new(
                ByteRange {
                    start: start1,
                    end: start1 + placeholder1.len(),
                },
                Some(placeholder1.clone()),
            ),
        ];
        composer.set_text_content(text, text_elements, vec![path1, path2.clone()]);

        let end1 = start1 + placeholder1.len();
        composer.draft.textarea.set_cursor(end1);

        composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        assert_eq!(
            composer.draft.textarea.text(),
            format!("Test {placeholder1} test ")
        );
        assert_eq!(
            vec![AttachedImage {
                path: path2,
                placeholder: placeholder1
            }],
            composer.attachments.local_images,
            "attachment renumbered after deletion"
        );
    }

    #[test]
    fn deleting_first_text_element_renumbers_following_text_element() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let path1 = PathBuf::from("/tmp/image_first.png");
        let path2 = PathBuf::from("/tmp/image_second.png");

        // Insert two adjacent atomic elements.
        composer.attach_image(path1);
        composer.attach_image(path2.clone());
        assert_eq!(composer.draft.textarea.text(), "[Image #1][Image #2]");
        assert_eq!(composer.attachments.local_images.len(), 2);

        // Delete the first element using normal textarea editing (forward Delete at cursor start).
        composer.draft.textarea.set_cursor(/*pos*/ 0);
        composer.handle_key_event(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));

        // Remaining image should be renumbered and the textarea element updated.
        assert_eq!(composer.attachments.local_images.len(), 1);
        assert_eq!(composer.attachments.local_images[0].path, path2);
        assert_eq!(
            composer.attachments.local_images[0].placeholder,
            "[Image #1]"
        );
        assert_eq!(composer.draft.textarea.text(), "[Image #1]");
    }

    #[test]
    fn pasting_filepath_attaches_image() {
        let tmp = tempdir().expect("create TempDir");
        let tmp_path: PathBuf = tmp.path().join("codex_tui_test_paste_image.png");
        // 008-TRIM: image crate 未引入（008 D4），粘贴路径按扩展名判定，写任意内容即可。
        std::fs::write(&tmp_path, b"png").expect("failed to write temp png");

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let needs_redraw = composer.handle_paste(tmp_path.to_string_lossy().to_string());
        assert!(needs_redraw);
        assert!(composer.draft.textarea.text().starts_with("[Image #1] "));

        let imgs = composer.take_recent_submission_images();
        assert_eq!(imgs, vec![tmp_path]);
    }

    #[test]
    fn slash_path_input_submits_without_command_error() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer
            .draft
            .textarea
            .set_text_clearing_elements("/Users/example/project/src/main.rs");

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        if let InputResult::Submitted { text, .. } = result {
            assert_eq!(text, "/Users/example/project/src/main.rs");
        } else {
            panic!("expected Submitted");
        }
        assert!(composer.draft.textarea.is_empty());
        match rx.try_recv() {
            Ok(event) => panic!("unexpected event: {event:?}"),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            Err(err) => panic!("unexpected channel state: {err:?}"),
        }
    }

    #[test]
    fn slash_with_leading_space_submits_as_text() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, mut rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer
            .draft
            .textarea
            .set_text_clearing_elements(" /this-looks-like-a-command");

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        if let InputResult::Submitted { text, .. } = result {
            assert_eq!(text, "/this-looks-like-a-command");
        } else {
            panic!("expected Submitted");
        }
        assert!(composer.draft.textarea.is_empty());
        match rx.try_recv() {
            Ok(event) => panic!("unexpected event: {event:?}"),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            Err(err) => panic!("unexpected channel state: {err:?}"),
        }
    }

    #[test]
    fn shortcut_modified_input_does_not_enter_paste_bursts() {
        snapshot_composer_state(
            "shortcut_modified_space_is_not_rendered",
            /*enhanced_keys_supported*/ false,
            |composer| {
                let now = Instant::now();
                for input in [
                    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                    KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SUPER),
                    KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
                ] {
                    composer.handle_input_basic_with_time(input, now);
                }

                assert!(
                    composer.handle_paste_burst_flush(
                        now + PasteBurst::recommended_active_flush_delay()
                    )
                );
            },
        );

        for modifiers in [
            KeyModifiers::SUPER,
            KeyModifiers::SHIFT | KeyModifiers::SUPER,
            KeyModifiers::HYPER,
            KeyModifiers::META,
        ] {
            let (mut composer, _rx) = new_test_composer();
            let now = Instant::now();

            composer
                .handle_input_basic_with_time(KeyEvent::new(KeyCode::Char(' '), modifiers), now);

            assert!(
                !composer.is_in_paste_burst(),
                "shortcut modifiers {modifiers:?} must not start a paste burst"
            );
            assert!(
                !composer.handle_paste_burst_flush(now + PasteBurst::recommended_flush_delay())
            );
            assert_eq!(composer.draft.textarea.text(), "");
        }
    }

    #[test]
    fn plain_and_shift_spaces_insert_normally() {
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let (mut composer, _rx) = new_test_composer();
            let now = Instant::now();

            composer
                .handle_input_basic_with_time(KeyEvent::new(KeyCode::Char(' '), modifiers), now);

            assert!(composer.is_in_paste_burst());
            assert_eq!(composer.draft.textarea.text(), "");
            assert!(composer.handle_paste_burst_flush(now + PasteBurst::recommended_flush_delay()));
            assert_eq!(composer.draft.textarea.text(), " ");
            assert!(!composer.is_in_paste_burst());
        }

        snapshot_composer_state(
            "plain_space_is_rendered",
            /*enhanced_keys_supported*/ false,
            |composer| {
                let now = Instant::now();
                for input in [
                    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                    KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
                    KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
                ] {
                    composer.handle_input_basic_with_time(input, now);
                }

                assert!(
                    composer.handle_paste_burst_flush(
                        now + PasteBurst::recommended_active_flush_delay()
                    )
                );
            },
        );
    }

    #[test]
    fn non_text_input_flushes_pending_paste_without_inserting_text() {
        let (mut composer, _rx) = new_test_composer();
        let now = Instant::now();

        composer.handle_input_basic_with_time(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            now,
        );
        composer.handle_input_basic_with_time(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
            now,
        );

        assert!(composer.is_in_paste_burst());
        assert_eq!(composer.draft.textarea.text(), "");

        composer.handle_input_basic_with_time(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SUPER),
            now,
        );

        assert_eq!(composer.draft.textarea.text(), "ab");
        assert!(!composer.is_in_paste_burst());
        assert!(!composer.handle_paste_burst_flush(now + PasteBurst::recommended_flush_delay()));
    }

    #[test]
    fn platform_specific_altgr_input_remains_text() {
        for modifiers in [
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT,
        ] {
            let (mut composer, _rx) = new_test_composer();
            let now = Instant::now();

            composer
                .handle_input_basic_with_time(KeyEvent::new(KeyCode::Char('@'), modifiers), now);

            if cfg!(windows) {
                assert!(composer.is_in_paste_burst());
                assert!(
                    composer.handle_paste_burst_flush(now + PasteBurst::recommended_flush_delay())
                );
                assert_eq!(composer.draft.textarea.text(), "@");
            } else {
                assert!(!composer.is_in_paste_burst());
                assert!(
                    !composer.handle_paste_burst_flush(now + PasteBurst::recommended_flush_delay())
                );
                assert_eq!(composer.draft.textarea.text(), "");
            }
        }
    }

    /// Behavior: the first fast ASCII character is held briefly to avoid flicker; if no burst
    /// follows, it should eventually flush as normal typed input (not as a paste).
    #[test]
    fn pending_first_ascii_char_flushes_as_typed() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert!(composer.is_in_paste_burst());
        assert!(composer.draft.textarea.text().is_empty());

        std::thread::sleep(ChatComposer::recommended_paste_flush_delay());
        let flushed = composer.flush_paste_burst_if_due();
        assert!(flushed, "expected pending first char to flush");
        assert_eq!(composer.draft.textarea.text(), "h");
        assert!(!composer.is_in_paste_burst());
    }

    /// Behavior: fast "paste-like" ASCII input should buffer and then flush as a single paste. If
    /// the payload is small, it should insert directly (no placeholder).
    #[test]
    fn burst_paste_fast_small_buffers_and_flushes_on_stop() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let count = 32;
        let mut now = Instant::now();
        let step = Duration::from_millis(/*millis*/ 1);
        for _ in 0..count {
            let _ = composer.handle_input_basic_with_time(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                now,
            );
            assert!(
                composer.is_in_paste_burst(),
                "expected active paste burst during fast typing"
            );
            assert!(
                composer.draft.textarea.text().is_empty(),
                "text should not appear during burst"
            );
            now += step;
        }

        assert!(
            composer.draft.textarea.text().is_empty(),
            "text should remain empty until flush"
        );
        let flush_time = now + PasteBurst::recommended_active_flush_delay() + step;
        let flushed = composer.handle_paste_burst_flush(flush_time);
        assert!(flushed, "expected buffered text to flush after stop");
        assert_eq!(composer.draft.textarea.text(), "a".repeat(count));
        assert!(
            composer.draft.pending_pastes.is_empty(),
            "no placeholder for small burst"
        );
    }

    /// Behavior: fast "paste-like" ASCII input should buffer and then flush as a single paste. If
    /// the payload is large, it should insert a placeholder and defer the full text until submit.
    #[test]
    fn burst_paste_fast_large_inserts_placeholder_on_flush() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let count = LARGE_PASTE_CHAR_THRESHOLD + 1; // > threshold to trigger placeholder
        let mut now = Instant::now();
        let step = Duration::from_millis(/*millis*/ 1);
        for _ in 0..count {
            let _ = composer.handle_input_basic_with_time(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                now,
            );
            now += step;
        }

        // Nothing should appear until we stop and flush
        assert!(composer.draft.textarea.text().is_empty());
        let flush_time = now + PasteBurst::recommended_active_flush_delay() + step;
        let flushed = composer.handle_paste_burst_flush(flush_time);
        assert!(flushed, "expected flush after stopping fast input");

        let expected_placeholder = format!("[Pasted Content {count} chars]");
        assert_eq!(composer.draft.textarea.text(), expected_placeholder);
        assert_eq!(composer.draft.pending_pastes.len(), 1);
        assert_eq!(composer.draft.pending_pastes[0].0, expected_placeholder);
        assert_eq!(composer.draft.pending_pastes[0].1.len(), count);
        assert!(composer.draft.pending_pastes[0].1.chars().all(|c| c == 'x'));
    }

    /// Behavior: human-like typing (with delays between chars) should not be classified as a paste
    /// burst. Characters should appear immediately and should not trigger a paste placeholder.
    #[test]
    fn humanlike_typing_1000_chars_appears_live_no_placeholder() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let count = LARGE_PASTE_CHAR_THRESHOLD; // 1000 in current config
        let mut now = Instant::now();
        let step = ChatComposer::recommended_paste_flush_delay();
        for _ in 0..count {
            let _ = composer.handle_input_basic_with_time(
                KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
                now,
            );
            now += step;
            let _ = composer.handle_paste_burst_flush(now);
        }

        assert_eq!(composer.draft.textarea.text(), "z".repeat(count));
        assert!(composer.draft.pending_pastes.is_empty());
    }

    #[test]
    fn slash_popup_not_activated_for_slash_space_text_history_like_input() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;
        use tokio::sync::mpsc::unbounded_channel;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        // Simulate history-like content: "/ test"
        composer.set_text_content("/ test".to_string(), Vec::new(), Vec::new());

        // After set_text_content -> sync_popups is called; popup should NOT be Command.
        assert!(
            matches!(composer.popups.active, ActivePopup::None),
            "expected no slash popup for '/ test'"
        );

        // Up should be handled by history navigation path, not slash popup handler.
        let (result, _redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(result, InputResult::None);
    }
    #[test]
    fn bare_slash_command_can_be_recalled_after_recording_pending_history() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_text_content("/diff".to_string(), Vec::new(), Vec::new());
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(result, InputResult::Command(SlashCommand::Diff));
        composer.record_pending_slash_command_history();

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(result, InputResult::None);
        assert_eq!(composer.current_text(), "/diff");
    }

    // 008-TRIM: fn popup_selected_slash_command_records_canonical_command_history（覆盖被裁 command/slash 弹窗选中（ActivePopup::Command 裁剪，008 §1））

    #[test]
    fn inline_slash_command_can_be_recalled_after_recording_pending_history() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_collaboration_modes_enabled(/*enabled*/ true);

        composer.set_text_content("/plan investigate this".to_string(), Vec::new(), Vec::new());
        composer.popups.active = ActivePopup::None;
        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        match result {
            InputResult::CommandWithArgs(cmd, args, text_elements) => {
                assert_eq!(cmd, SlashCommand::Plan);
                assert_eq!(args, "investigate this");
                assert!(text_elements.is_empty());
            }
            other => panic!("expected inline /plan command, got {other:?}"),
        }
        composer.record_pending_slash_command_history();

        let (result, _needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(result, InputResult::None);
        assert_eq!(composer.current_text(), "/plan investigate this");
    }

    #[test]
    fn apply_external_edit_rebuilds_text_and_attachments() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let placeholder = local_image_label_text(/*label_number*/ 1);
        composer.draft.textarea.insert_element(&placeholder);
        composer.attachments.local_images.push(AttachedImage {
            placeholder: placeholder.clone(),
            path: PathBuf::from("img.png"),
        });
        composer
            .draft
            .pending_pastes
            .push(("[Pasted]".to_string(), "data".to_string()));

        composer.apply_external_edit(format!("Edited {placeholder} text"));

        assert_eq!(
            composer.current_text(),
            format!("Edited {placeholder} text")
        );
        assert!(composer.draft.pending_pastes.is_empty());
        assert_eq!(composer.attachments.local_images.len(), 1);
        assert_eq!(
            composer.attachments.local_images[0].placeholder,
            placeholder
        );
        assert_eq!(
            composer.draft.textarea.cursor(),
            composer.current_text().len()
        );
    }

    #[test]
    fn apply_external_edit_absorbs_bash_prefix_without_duplication() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_text_content("!git status".to_string(), Vec::new(), Vec::new());

        composer.apply_external_edit("!git status".to_string());

        assert!(composer.draft.is_bash_mode);
        assert_eq!(composer.draft.textarea.text(), "git status");
        assert_eq!(composer.current_text(), "!git status");
    }

    #[test]
    fn apply_external_edit_can_leave_bash_mode() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_text_content("!git status".to_string(), Vec::new(), Vec::new());

        composer.apply_external_edit("git status".to_string());

        assert!(!composer.draft.is_bash_mode);
        assert_eq!(composer.draft.textarea.text(), "git status");
        assert_eq!(composer.current_text(), "git status");
    }

    #[test]
    fn apply_external_edit_can_enter_bash_mode() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );
        composer.set_text_content("git status".to_string(), Vec::new(), Vec::new());

        composer.apply_external_edit("!git status".to_string());

        assert!(composer.draft.is_bash_mode);
        assert_eq!(composer.draft.textarea.text(), "git status");
        assert_eq!(composer.current_text(), "!git status");
    }

    #[test]
    fn apply_external_edit_drops_missing_attachments() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let placeholder = local_image_label_text(/*label_number*/ 1);
        composer.draft.textarea.insert_element(&placeholder);
        composer.attachments.local_images.push(AttachedImage {
            placeholder: placeholder.clone(),
            path: PathBuf::from("img.png"),
        });

        composer.apply_external_edit("No images here".to_string());

        assert_eq!(composer.current_text(), "No images here".to_string());
        assert!(composer.attachments.local_images.is_empty());
    }

    #[test]
    fn apply_external_edit_renumbers_image_placeholders() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let first_path = PathBuf::from("img1.png");
        let second_path = PathBuf::from("img2.png");
        composer.attach_image(first_path);
        composer.attach_image(second_path.clone());

        let placeholder2 = local_image_label_text(/*label_number*/ 2);
        composer.apply_external_edit(format!("Keep {placeholder2}"));

        let placeholder1 = local_image_label_text(/*label_number*/ 1);
        assert_eq!(composer.current_text(), format!("Keep {placeholder1}"));
        assert_eq!(composer.attachments.local_images.len(), 1);
        assert_eq!(
            composer.attachments.local_images[0].placeholder,
            placeholder1
        );
        assert_eq!(composer.local_image_paths(), vec![second_path]);
        assert_eq!(
            composer.draft.textarea.element_payloads(),
            vec![placeholder1]
        );
    }

    #[test]
    fn current_text_with_pending_expands_placeholders() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let placeholder = "[Pasted Content 5 chars]".to_string();
        composer.draft.textarea.insert_element(&placeholder);
        composer
            .draft
            .pending_pastes
            .push((placeholder.clone(), "hello".to_string()));

        assert_eq!(
            composer.current_text_with_pending(),
            "hello".to_string(),
            "placeholder should expand to actual text"
        );
    }

    #[test]
    fn current_text_with_pending_expands_overlapping_placeholders() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let first_paste = "a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
        let second_paste = "b".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
        let base = format!("[Pasted Content {} chars]", first_paste.chars().count());
        let second = format!("{base} #2");

        composer.handle_paste(first_paste.clone());
        composer.handle_paste(second_paste.clone());

        assert_eq!(composer.current_text(), format!("{base}{second}"));
        assert_eq!(
            composer.current_text_with_pending(),
            format!("{first_paste}{second_paste}")
        );
    }

    #[test]
    fn apply_external_edit_limits_duplicates_to_occurrences() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        let placeholder = local_image_label_text(/*label_number*/ 1);
        composer.draft.textarea.insert_element(&placeholder);
        composer.attachments.local_images.push(AttachedImage {
            placeholder: placeholder.clone(),
            path: PathBuf::from("img.png"),
        });

        composer.apply_external_edit(format!("{placeholder} extra {placeholder}"));

        assert_eq!(
            composer.current_text(),
            format!("{placeholder} extra {placeholder}")
        );
        assert_eq!(composer.attachments.local_images.len(), 1);
    }

    #[test]
    fn remote_images_do_not_modify_textarea_text_or_elements() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_remote_image_urls(vec![
            "https://example.com/one.png".to_string(),
            "https://example.com/two.png".to_string(),
        ]);

        assert_eq!(composer.current_text(), "");
        assert_eq!(composer.text_elements(), Vec::<TextElement>::new());
    }

    #[test]
    fn attach_image_after_remote_prefix_uses_offset_label() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_remote_image_urls(vec![
            "https://example.com/one.png".to_string(),
            "https://example.com/two.png".to_string(),
        ]);
        composer.attach_image(PathBuf::from("/tmp/local.png"));

        assert_eq!(
            composer.attachments.local_images[0].placeholder,
            "[Image #3]"
        );
        assert_eq!(composer.current_text(), "[Image #3]");
    }

    #[test]
    fn prepare_submission_keeps_remote_offset_local_placeholder_numbering() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_remote_image_urls(vec!["https://example.com/one.png".to_string()]);
        let base_text = "[Image #2] hello".to_string();
        let base_elements = vec![TextElement::new(
            (0.."[Image #2]".len()).into(),
            Some("[Image #2]".to_string()),
        )];
        composer.set_text_content(
            base_text,
            base_elements,
            vec![PathBuf::from("/tmp/local.png")],
        );

        let (submitted_text, submitted_elements) = composer
            .prepare_submission_text(/*record_history*/ true)
            .expect("remote+local submission should be generated");
        assert_eq!(submitted_text, "[Image #2] hello");
        assert_eq!(
            submitted_elements,
            vec![TextElement::new(
                (0.."[Image #2]".len()).into(),
                Some("[Image #2]".to_string())
            )]
        );
    }

    #[test]
    fn prepare_submission_with_only_remote_images_returns_empty_text() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_remote_image_urls(vec!["https://example.com/one.png".to_string()]);
        let (submitted_text, submitted_elements) = composer
            .prepare_submission_text(/*record_history*/ true)
            .expect("remote-only submission should be generated");
        assert_eq!(submitted_text, "");
        assert!(submitted_elements.is_empty());
    }

    #[test]
    fn delete_selected_remote_image_relabels_local_placeholders() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_remote_image_urls(vec![
            "https://example.com/one.png".to_string(),
            "https://example.com/two.png".to_string(),
        ]);
        composer.attach_image(PathBuf::from("/tmp/local.png"));
        composer.draft.textarea.set_cursor(/*pos*/ 0);

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(
            composer.remote_image_urls(),
            vec!["https://example.com/one.png".to_string()]
        );
        assert_eq!(composer.current_text(), "[Image #2]");
        assert_eq!(
            composer.attachments.local_images[0].placeholder,
            "[Image #2]"
        );

        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(composer.remote_image_urls(), Vec::<String>::new());
        assert_eq!(composer.current_text(), "[Image #1]");
        assert_eq!(
            composer.attachments.local_images[0].placeholder,
            "[Image #1]"
        );
    }

    #[test]
    fn input_disabled_ignores_keypresses_and_hides_cursor() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_text_content("hello".to_string(), Vec::new(), Vec::new());
        composer.set_input_enabled(
            /*enabled*/ false,
            Some("Input disabled for test.".to_string()),
        );

        let (result, needs_redraw) =
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(result, InputResult::None);
        assert!(!needs_redraw);
        assert_eq!(composer.current_text(), "hello");

        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };
        assert_eq!(composer.cursor_pos(area), None);
    }

    #[test]
    fn shutdown_in_progress_disables_input_and_uses_hint_without_footer() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(
            /*has_input_focus*/ true,
            sender,
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        );

        composer.set_text_content("hello".to_string(), Vec::new(), Vec::new());
        composer.show_shutdown_in_progress();

        assert!(!composer.input_enabled());
        assert_eq!(composer.current_text(), "hello");
        assert_eq!(composer.custom_footer_height(), Some(0));

        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };
        assert_eq!(composer.cursor_pos(area), None);

        let mut terminal = Terminal::new(TestBackend::new(40, 5)).expect("terminal");
        terminal
            .draw(|f| composer.render(f.area(), f.buffer_mut()))
            .unwrap();
        insta::assert_snapshot!("shutdown_in_progress", terminal.backend());
    }
}
