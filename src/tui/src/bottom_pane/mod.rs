//! The bottom pane is the interactive footer of the chat UI.
//!
//! The pane owns the [`ChatComposer`] (editable prompt input) and a stack of transient
//! [`BottomPaneView`]s (popups/modals) that temporarily replace the composer for focused
//! interactions like selection lists. Centered views retain earlier views as a backdrop,
//! while input remains routed exclusively to the top of the stack.
//! Modal insertion, replacement, and dismissal invalidate pending key chords, including when a
//! deferred prompt appears or an approval queue advances without changing keymap contexts.
//!
//! Input routing is layered: `BottomPane` decides which local surface receives a key (view vs
//! composer), while higher-level intent such as "interrupt" or "quit" is decided by the parent
//! widget (`ChatWidget`). This split matters for Ctrl+C/Ctrl+D: the bottom pane gives the active
//! view the first chance to consume Ctrl+C (typically to dismiss itself), then lets an active
//! composer history search consume Ctrl+C as cancellation, and `ChatWidget` may treat an unhandled
//! Ctrl+C as an interrupt or as the first press of a double-press quit shortcut.
//!
//! Some UI is time-based rather than input-based, such as the transient "press again to quit"
//! hint. The pane schedules redraws so those hints can expire even when the UI is otherwise idle.
//! Inline banners sit above the composer. Number shortcuts apply only to an empty, idle composer;
//! drafts, paste bursts, and active dialogs keep their normal input routing.
//! Owned transcripts separate activity from the transcript and reserve their shared hint row
//! below activity and previews, above the composer.
pub(crate) use chat_composer::CommandPopupPlacement;
pub(crate) use chat_composer::ComposerRenderOptions;
pub(crate) use chat_composer::TranscriptFooter;
pub(crate) use composer_gap::ComposerGap;
pub(crate) use footer::footer_hint_items_line;
pub(crate) use footer::inset_footer_hint_area;
// 008-TRIM: use std::collections::VecDeque;（延迟审批队列，008 §1 裁剪：无本地事件源）
use std::path::PathBuf;

// 008-TRIM: use crate::app::app_server_requests::ResolvedAppServerRequest;（裁剪功能/模块，008 §1）
use crate::app_event::AppEvent;
// 008-TRIM: use crate::app_event::ConnectorsSnapshot;（裁剪功能/模块，008 §1）
use crate::app_event::HistoryLookupResponse;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::pending_input_preview::PendingInputPreview;
// 008-TRIM: use crate::bottom_pane::pending_thread_approvals::PendingThreadApprovals;（裁剪功能/模块，008 §1）
// 008-TRIM: use crate::bottom_pane::unified_exec_footer::UnifiedExecFooter;（裁剪功能/模块，008 §1）
use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::KeymapContext;
use crate::keymap::KeymapContextSet;
use crate::keymap::RuntimeKeymap;
use crate::render::renderable::FlexRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableItem;
use crate::terminal_palette::effective_stdout_color_level;
use crate::tui::FrameRequester;
pub(crate) use bottom_pane_view::BottomPaneView;
pub(crate) use bottom_pane_view::ViewCompletion;
pub(crate) use bottom_pane_view::ViewPresentation;
// 008-TRIM: use codex_app_server_protocol::SkillMetadata;（裁剪功能/模块，008 §1）
// 008-TRIM: use codex_app_server_protocol::ToolRequestUserInputParams;（裁剪功能/模块，008 §1）
// 008-TRIM: use codex_features::Features;（审批功能，008 §1 裁剪：无本地事件源）
// 008-TRIM: use codex_file_search::FileMatch;（裁剪功能/模块，008 §1）
// 008-TRIM: use codex_plugin::PluginCapabilitySummary;（裁剪功能/模块，008 §1）
use codex_protocol::ThreadId;
// 008-TRIM: use codex_protocol::openai_models::ReasoningEffort;（effort 动画，008 §1 裁剪）
use codex_protocol::user_input::TextElement;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
// 008-TRIM: use ratatui::style::Stylize;（被裁渲染分支专用，008 §1 裁剪：无本地事件源）
use ratatui::text::Line;
use ratatui::text::Span;
// 008-TRIM: use ratatui::widgets::Paragraph;（questions 摘要渲染，008 §1 裁剪：无本地事件源）
use std::time::Duration;
use std::time::Instant;

// 008-TRIM: mod action_required_title（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod actionable_banner（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod app_link_view（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod apply_patch_header（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod approval_overlay（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod async_questions（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod config_panel（008 §1 裁剪清单：无本地事件源）
mod empty_state_policy;
// 008-TRIM: mod hook_status（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod mcp_server_elicitation（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod multi_select_picker（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: pub(crate) use multi_select_picker::MultiSelectItem;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use multi_select_picker::MultiSelectPicker;（裁剪模块，008 §1）
// 008-TRIM(孤儿属性随被裁声明一并移除): #[cfg(test)]
// 008-TRIM(孤儿属性随被裁声明一并移除): #[path = "questions_tests.rs"]
// 008-TRIM: mod question_tests（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod questions（008 §1 裁剪清单：无本地事件源）
mod request_user_input; // 018：恢复同步问答必要子集
// 008-TRIM: mod status_line_setup（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod status_line_style（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod status_surface_preview（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod title_setup（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod user_verification（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod voice_strip（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod warnings（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod warnings_view（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: pub(crate) use action_required_title::ACTION_REQUIRED_PREVIEW_PREFIX;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use action_required_title::build_action_required_title_text;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use actionable_banner::ActionableBanner;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use actionable_banner::BannerDismissal;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use app_link_view::AppLinkElicitationTarget;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use app_link_view::AppLinkSuggestionType;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use app_link_view::AppLinkView;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use app_link_view::AppLinkViewParams;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use approval_overlay::ApplyPatchApprovalRequest;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use approval_overlay::ApprovalOverlay;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use approval_overlay::ApprovalRequest;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use approval_overlay::ExecApprovalRequest;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use approval_overlay::McpElicitationApprovalRequest;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use approval_overlay::PermissionsApprovalRequest;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use approval_overlay::format_requested_permissions_rule;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use async_questions::AsyncQuestions;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use async_questions::QuestionState;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use async_questions::QuestionSubmission;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use mcp_server_elicitation::McpServerElicitationFormRequest;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use mcp_server_elicitation::McpServerElicitationOverlay;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use request_user_input::RequestUserInputOverlay;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use status_line_style::status_line_from_segments;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use voice_strip::VoiceStripPhase;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use voice_strip::VoiceStripState;（裁剪模块，008 §1）
mod bottom_pane_view;
mod composer_gap;
// 008-TRIM: mod effort_ignition（008 §1 裁剪清单：无本地事件源）
mod view_stack;
pub(crate) use view_stack::CenteredView;
pub(crate) use view_stack::DialogOverlay;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImageAttachment {
    pub placeholder: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MentionBinding {
    /// Visible mention sigil (`$` or `@`).
    pub(crate) sigil: char,
    /// Mention token text without the leading sigil (`$` or `@`).
    pub(crate) mention: String,
    /// Canonical mention target (for example `app://...` or absolute SKILL.md path).
    pub(crate) path: String,
}
mod chat_composer;
mod chat_composer_history;
// 008-TRIM: mod command_popup（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod custom_prompt_view（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod effort_status_line（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod experimental_features_view（008 §1 裁剪清单：无本地事件源）
// 020 A02 恢复：@ 文件搜索弹窗（上游 d138398 的本地适配——同步搜索）
mod file_search_popup;
pub(crate) use file_search_popup::{FileMatch, FileSearchPopup};
mod footer;
mod list_selection_view;
// 008-TRIM: mod memories_settings_view（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod mentions_v2（008 §1 裁剪清单：无本地事件源）
mod picker_presets;
mod picker_rows;
mod picker_style;
// 008-TRIM: mod shortcut_overlay（008 §1 裁剪清单：无本地事件源）
pub(crate) use picker_style::active_tab_style;
pub(crate) use picker_style::selection_style;
mod prompt_args; // 008-REVIVE: 提交管线的斜杠解析所需（实现轮裁决，登记）
mod selection_picker_layout;
// 008-TRIM: mod skill_popup（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod skills_toggle_view（008 §1 裁剪清单：无本地事件源）
mod slash_commands; // 008-REVIVE: 斜杠解析注册表所需（内容本地化适配，登记）
pub(crate) use footer::CollaborationModeIndicator;
pub(crate) use footer::GoalStatusIndicator;
#[cfg(test)]
pub(crate) use footer::goal_status_indicator_line;
pub(crate) use list_selection_view::ColumnWidthMode;
pub(crate) use list_selection_view::ListSelectionView;
pub(crate) use list_selection_view::OnSelectionChangedCallback;
pub(crate) use list_selection_view::PickerSurface;
pub(crate) use list_selection_view::SearchMode;
pub(crate) use list_selection_view::SelectionDescriptionLayout;
pub(crate) use list_selection_view::SelectionRowDisplay;
pub(crate) use list_selection_view::SelectionToggle;
pub(crate) use list_selection_view::SelectionToggleState;
pub(crate) use list_selection_view::SelectionViewParams;
pub(crate) use list_selection_view::SideContentWidth;
pub(crate) use list_selection_view::popup_content_width;
pub(crate) use list_selection_view::side_by_side_layout_widths;
// 008-TRIM: pub(crate) use memories_settings_view::MemoriesSettingsView;（裁剪模块，008 §1）
use slash_commands::ServiceTierCommand;
// 008-TRIM: mod feedback_note_view（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod feedback_view（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: mod hooks_browser_view（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: pub(crate) use feedback_view::FeedbackAudience;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use feedback_view::feedback_classification;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use feedback_view::feedback_disabled_params;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use feedback_view::feedback_selection_params;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use feedback_view::feedback_success_cell;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use feedback_view::feedback_upload_consent_params;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use skills_toggle_view::SkillsToggleItem;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use skills_toggle_view::SkillsToggleView;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use status_line_setup::StatusLineItem;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use status_line_setup::StatusLineSetupView;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use status_surface_preview::StatusSurfacePreviewData;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use status_surface_preview::StatusSurfacePreviewItem;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use title_setup::TerminalTitleItem;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use title_setup::TerminalTitleSetupView;（裁剪模块，008 §1）
// 008-TRIM(孤儿属性随被裁声明一并移除): #[cfg(test)]
// 008-TRIM: pub(crate) use title_setup::preview_line_for_title_items;（裁剪模块，008 §1）
mod paste_burst;
mod pending_input_preview;
// 008-TRIM: mod pending_thread_approvals（008 §1 裁剪清单：无本地事件源）
mod picker_option;
pub(crate) use picker_option::picker_option_list;
pub(crate) use picker_option::picker_option_row;
pub(crate) mod popup_consts;
mod scroll_state;
mod selection_popup_common;
pub(crate) use selection_popup_common::menu_surface_padding_height;
pub(crate) use selection_popup_common::render_menu_surface;
mod selection_row_layout;
mod selection_tabs;
pub(crate) use selection_tabs::render_filled_tab_bar;
mod startup;
mod textarea;
pub(crate) use textarea::TextArea;
pub(crate) use textarea::TextAreaState;
// 008-TRIM: mod unified_exec_footer（008 §1 裁剪清单：无本地事件源）
// 008-TRIM: pub(crate) use feedback_note_view::FeedbackNoteView;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use hooks_browser_view::HooksBrowserView;（裁剪模块，008 §1）
pub(crate) use selection_tabs::SelectionTab;

/// How long the "press again to quit" hint stays visible.
///
/// This is shared between:
/// - `ChatWidget`: arming the double-press quit shortcut.
/// - `BottomPane`/`ChatComposer`: rendering and expiring the footer hint.
///
/// Keeping a single value ensures Ctrl+C and Ctrl+D behave identically.
pub(crate) const QUIT_SHORTCUT_TIMEOUT: Duration = Duration::from_secs(1);

// 008-TRIM: const APPROVAL_PROMPT_TYPING_IDLE_DELAY（延迟审批，008 §1 裁剪：无本地事件源）

/// Whether Ctrl+C/Ctrl+D require a second press to quit.
///
/// This UX experiment was enabled by default, but requiring a double press to quit feels janky in
/// practice (especially for users accustomed to shells and other TUIs). Disable it for now while we
/// rethink a better quit/interrupt design.
pub(crate) const DOUBLE_PRESS_QUIT_SHORTCUT_ENABLED: bool = false;

/// The result of offering a cancellation key to a bottom-pane surface.
///
/// This is primarily used for Ctrl+C routing: active views can consume the key to dismiss
/// themselves, and the caller can decide what higher-level action (if any) to take when the key is
/// not handled locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancellationEvent {
    Handled,
    NotHandled,
}

use crate::bottom_pane::prompt_args::parse_slash_name;
pub(crate) use chat_composer::ChatComposer;
pub(crate) use chat_composer::ChatComposerConfig;
pub(crate) use chat_composer::ClipboardPasteTarget;
pub(crate) use chat_composer::ComposerDraftSnapshot;
pub(crate) use chat_composer::InputResult;
pub(crate) use chat_composer::QueuedInputAction;
pub(crate) use chat_composer::RestrictedInputMode;
pub(crate) use chat_composer_history::HistoryEntry;
pub(crate) use textarea::KillBufferSnapshot;

use crate::status_indicator_widget::StatusDetailsCapitalization;
use crate::status_indicator_widget::StatusIndicatorWidget;
// 008-TRIM(孤儿属性随被裁声明一并移除): #[cfg(test)]
// 008-TRIM: pub(crate) use experimental_features_view::ExperimentalFeatureItem;（裁剪模块，008 §1）
// 008-TRIM: pub(crate) use experimental_features_view::ExperimentalFeaturesView;（裁剪模块，008 §1）
pub(crate) use list_selection_view::SELECTION_TOGGLE_BLOCKED_PREFIX;
pub(crate) use list_selection_view::SELECTION_TOGGLE_UNAVAILABLE_PREFIX;
pub(crate) use list_selection_view::SelectionAction;
pub(crate) use list_selection_view::SelectionItem;
pub(crate) use list_selection_view::SelectionSecondaryAction;

// 008-TRIM: struct DelayedApprovalRequest（审批功能，008 §1 裁剪）

/// Pane displayed in the lower half of the chat UI.
///
/// This is the owning container for the prompt input (`ChatComposer`) and the view stack
/// (`BottomPaneView`). It performs local input routing and renders time-based hints, while leaving
/// process-level decisions (quit, interrupt, shutdown) to `ChatWidget`.
pub(crate) struct BottomPane {
    /// Composer is retained even when a BottomPaneView is displayed so the
    /// input state is retained when the view is closed.
    composer: ChatComposer,

    /// Stack of views displayed instead of the composer (e.g. popups/modals).
    view_stack: Vec<Box<dyn BottomPaneView>>,
    pub(crate) key_chord_reset_requested: bool,
    // 008-TRIM: warnings_view/suppress_warning_keep_repeat/questions/delayed_approval_requests 字段（裁剪功能，008 §1）
    view_transition_key_guard: Option<crate::activation_key_guard::ActivationKeyGuard>,
    // 008-TRIM: last_composer_activity_at 字段（延迟审批，008 §1 裁剪：无本地事件源）

    app_event_tx: AppEventSender,
    frame_requester: FrameRequester,
    thread_id: Option<ThreadId>,

    has_input_focus: bool,
    enhanced_keys_supported: bool,
    disable_paste_burst: bool,
    is_task_running: bool,
    esc_backtrack_hint: bool,
    animations_enabled: bool,
    effects: codex_config::types::TuiEffects,

    /// Inline status indicator shown above the composer while a task is running.
    status: Option<StatusIndicatorWidget>,
    // 008-TRIM: hook_status_message/inline_banner 字段（裁剪功能，008 §1）
    /// Streaming may drop the row without losing its elapsed time or modal pause.
    status_timer: crate::status_indicator_widget::StatusTimer,
    // 008-TRIM: unified_exec_footer/pending_thread_approvals 字段（裁剪功能，008 §1）
    /// Preview of pending steers and queued drafts shown above the composer.
    pending_input_preview: PendingInputPreview,
    context_window_percent: Option<i64>,
    context_window_used_tokens: Option<i64>,
    keymap: RuntimeKeymap,
}

pub(crate) struct BottomPaneParams {
    pub(crate) app_event_tx: AppEventSender,
    pub(crate) frame_requester: FrameRequester,
    pub(crate) has_input_focus: bool,
    pub(crate) enhanced_keys_supported: bool,
    pub(crate) placeholder_text: String,
    pub(crate) disable_paste_burst: bool,
    pub(crate) animations_enabled: bool,
    pub(crate) effects: codex_config::types::TuiEffects,
    // 008-TRIM: skills 字段（裁剪功能，008 §1）
}
/// 020 §2.3：剔除系统生成的附件标记字面（`[Image #N]` 及其尾随空格）——
/// 恢复重建时统一按附件顺序重新生成标记；用户正文其余部分原样保留。
fn strip_system_image_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("[Image #") {
        let after = &rest[pos..];
        let Some(close) = after.find(']') else {
            break;
        };
        let digits = &after["[Image #".len()..close];
        if !digits.bytes().all(|b| b.is_ascii_digit()) || digits.is_empty() {
            // 非系统形态：原样保留该片段
            out.push_str(&rest[..pos + "[Image #".len()]);
            rest = &rest[pos + "[Image #".len()..];
            continue;
        }
        out.push_str(&rest[..pos]);
        let mut tail = &after[close + 1..];
        if tail.starts_with(' ') {
            tail = &tail[1..];
        }
        rest = tail;
    }
    out.push_str(rest);
    out.trim().to_string()
}

impl BottomPane {
    pub fn new(params: BottomPaneParams) -> Self {
        Self::new_with_composer_config(params, ChatComposerConfig::default())
    }

    /// Construct a bottom pane with explicitly restricted composer behavior.
    pub(crate) fn new_with_composer_config(
        params: BottomPaneParams,
        composer_config: ChatComposerConfig,
    ) -> Self {
        let BottomPaneParams {
            app_event_tx,
            frame_requester,
            has_input_focus,
            enhanced_keys_supported,
            placeholder_text,
            disable_paste_burst,
            animations_enabled,
            effects,
        } = params;
        let mut composer = ChatComposer::new_with_config(
            has_input_focus,
            app_event_tx.clone(),
            enhanced_keys_supported,
            placeholder_text,
            disable_paste_burst,
            composer_config,
        );
        composer.set_frame_requester(frame_requester.clone());
        let keymap = RuntimeKeymap::defaults();
        composer.set_keymap_bindings(&keymap);
        Self {
            composer,
            view_stack: Vec::new(),
            key_chord_reset_requested: false,
            // 008-TRIM: 裁剪字段初始化（008 §1）
            view_transition_key_guard: None,
            // 008-TRIM: last_composer_activity_at 初始化（008 §1 裁剪：无本地事件源）
            app_event_tx,
            frame_requester,
            thread_id: None,
            has_input_focus,
            enhanced_keys_supported,
            disable_paste_burst,
            is_task_running: false,
            status: None,
            status_timer: crate::status_indicator_widget::StatusTimer::default(),
            // 008-TRIM: 裁剪字段初始化（008 §1）
            pending_input_preview: PendingInputPreview::new(),
            esc_backtrack_hint: false,
            animations_enabled,
            effects,
            context_window_percent: None,
            context_window_used_tokens: None,
            keymap,
        }
    }

    // 008-TRIM: fn set_skills（裁剪功能，008 §1）

    /// Update image-paste behavior for the active composer and repaint immediately.
    ///
    /// Callers use this to keep composer affordances aligned with model capabilities.
    pub fn set_image_paste_enabled(&mut self, enabled: bool) {
        self.composer.set_image_paste_enabled(enabled);
        self.request_redraw();
    }

    pub(crate) fn set_motion_preferences(&mut self, tui: &codex_config::types::Tui) {
        self.animations_enabled = tui.animations;
        self.effects = tui.effects;
        if let Some(status) = self.status.as_mut() {
            status.set_motion_preferences(tui);
        }
        self.request_redraw();
    }

    /// Mirrors the effective reasoning effort into the composer so its next
    // 008-TRIM: set_active_reasoning_effort / _baseline（effort 功能，008 §1 裁剪）

    // 008-TRIM: fn set_plugins_command_enabled（plugins 命令，008 §1 裁剪：无本地事件源）

    pub fn set_token_activity_command_enabled(&mut self, enabled: bool) {
        self.composer.set_token_activity_command_enabled(enabled);
        self.request_redraw();
    }

    pub fn set_mentions_v2_enabled(&mut self, enabled: bool) {
        self.composer.set_mentions_v2_enabled(enabled);
        self.request_redraw();
    }

    pub fn take_mention_bindings(&mut self) -> Vec<MentionBinding> {
        self.composer.take_mention_bindings()
    }

    pub fn take_recent_submission_mention_bindings(&mut self) -> Vec<MentionBinding> {
        self.composer.take_recent_submission_mention_bindings()
    }

    /// Add a staged slash-command draft to the composer's local recall list.
    ///
    /// This should be called exactly once after `ChatWidget` dispatches a recognized command.
    /// Slash recall records the submitted command text regardless of whether the command succeeds.
    pub(crate) fn record_pending_slash_command_history(&mut self) {
        self.composer.record_pending_slash_command_history();
    }

    /// Replace all bottom-pane keymap caches from one resolved runtime keymap.
    ///
    /// The bottom pane owns several input surfaces: composer, overlays, and
    /// selection views. Applying one snapshot through this method keeps those
    /// surfaces synchronized after config reloads or interactive remaps. Callers
    /// should not update the composer directly unless they deliberately want
    /// overlays and selection views to continue using the previous bindings.
    pub fn set_keymap_bindings(&mut self, keymap: &RuntimeKeymap) {
        self.keymap = keymap.clone();
        // 008-TRIM: warnings_view/questions 键位同步（008 §1 裁剪：无本地事件源）
        self.composer.set_keymap_bindings(keymap);
        // Show the first shortcut from the same keymap ChatWidget uses to handle queued edits.
        self.pending_input_preview
            .set_edit_binding(keymap.primary_hint(KeymapContext::Chat, "edit_queued_message"));
        let interrupt_binding = keymap.primary_hint(KeymapContext::Chat, "interrupt_turn");
        self.pending_input_preview
            .set_interrupt_binding(interrupt_binding);
        if let Some(status) = self.status.as_mut() {
            status.set_interrupt_binding(interrupt_binding);
        }
        self.request_redraw();
    }

    pub(crate) fn transcript_shortcut_hint(&self) -> Option<crate::key_hint::ShortcutHint> {
        self.keymap
            .primary_hint(KeymapContext::Global, "open_transcript")
    }

    /// Clear pending attachments and mention bindings e.g. when a slash command doesn't submit text.
    pub(crate) fn drain_pending_submission_state(&mut self) {
        let _ = self.take_recent_submission_images_with_placeholders();
        let _ = self.take_remote_image_urls();
        let _ = self.take_recent_submission_mention_bindings();
        let _ = self.take_mention_bindings();
    }

    pub fn set_collaboration_modes_enabled(&mut self, enabled: bool) {
        self.composer.set_collaboration_modes_enabled(enabled);
        self.request_redraw();
    }

    pub fn set_connectors_enabled(&mut self, enabled: bool) {
        self.composer.set_connectors_enabled(enabled);
    }

    #[cfg(target_os = "windows")]
    pub fn set_windows_degraded_sandbox_active(&mut self, enabled: bool) {
        self.composer.set_windows_degraded_sandbox_active(enabled);
        self.request_redraw();
    }

    pub fn set_collaboration_mode_indicator(
        &mut self,
        indicator: Option<CollaborationModeIndicator>,
    ) {
        self.composer.set_collaboration_mode_indicator(indicator);
        self.request_redraw();
    }

    pub fn set_goal_status_indicator(&mut self, indicator: Option<GoalStatusIndicator>) {
        self.composer.set_goal_status_indicator(indicator);
        self.request_redraw();
    }

    pub fn set_ide_context_active(&mut self, active: bool) {
        self.composer.set_ide_context_active(active);
        self.request_redraw();
    }

    pub fn set_worktrees_enabled(&mut self, enabled: bool) {
        self.composer.set_worktrees_enabled(enabled);
        self.request_redraw();
    }

    pub fn set_service_tier_commands_enabled(&mut self, enabled: bool) {
        self.composer.set_service_tier_commands_enabled(enabled);
        self.request_redraw();
    }

    pub fn set_daybreak_command_description(&mut self, description: Option<&'static str>) {
        self.composer.set_daybreak_command_description(description);
        self.request_redraw();
    }

    pub fn set_service_tier_commands(&mut self, commands: Vec<ServiceTierCommand>) {
        self.composer.set_service_tier_commands(commands);
        self.request_redraw();
    }

    pub fn set_goal_command_enabled(&mut self, enabled: bool) {
        self.composer.set_goal_command_enabled(enabled);
        self.request_redraw();
    }

    pub fn set_voice_command_enabled(&mut self, enabled: bool) {
        self.composer.set_voice_command_enabled(enabled);
        self.request_redraw();
    }

    pub(crate) fn set_side_conversation_active(&mut self, active: bool) {
        self.composer.set_side_conversation_active(active);
        self.request_redraw();
    }

    pub(crate) fn set_placeholder_text(&mut self, placeholder: String) {
        self.composer.set_placeholder_text(placeholder);
        self.request_redraw();
    }

    pub(crate) fn set_parent_owned_thread(&mut self) {
        self.composer.set_parent_owned_thread();
        self.request_redraw();
    }

    pub(crate) fn set_vim_enabled(&mut self, enabled: bool) {
        self.composer.set_vim_enabled(enabled);
        // 008-TRIM: questions vim 同步（008 §1 裁剪：无本地事件源）
        self.request_redraw();
    }

    pub(crate) fn take_kill_buffer_snapshot(&mut self) -> KillBufferSnapshot {
        self.composer.take_kill_buffer_snapshot()
    }

    pub(crate) fn restore_kill_buffer_snapshot(&mut self, snapshot: KillBufferSnapshot) {
        self.composer.restore_kill_buffer_snapshot(snapshot);
    }

    pub(crate) fn toggle_vim_enabled(&mut self) -> bool {
        let enabled = self.composer.toggle_vim_enabled();
        // 008-TRIM: questions vim 同步（008 §1 裁剪：无本地事件源）
        self.request_redraw();
        enabled
    }

    pub fn status_widget(&self) -> Option<&StatusIndicatorWidget> {
        self.status.as_ref()
    }

    pub(crate) fn status_elapsed(&self) -> Option<Duration> {
        self.is_task_running
            .then(|| self.status_timer.elapsed_at(Instant::now()))
    }

    pub(crate) fn reset_status_timer(&mut self, elapsed: Duration) {
        self.status_timer.reset(elapsed);
    }

    pub(crate) fn set_status_timer_origin(&mut self, started_at: Option<Instant>) {
        self.status_timer.display_started_at = started_at;
        self.request_redraw();
    }

    // 008-TRIM: fn skills / fn plugins 访问器（skill/plugin 目录，008 §1 裁剪：无本地事件源）

    #[cfg(test)]
    pub(crate) fn context_window_percent(&self) -> Option<i64> {
        self.context_window_percent
    }

    #[cfg(test)]
    pub(crate) fn context_window_used_tokens(&self) -> Option<i64> {
        self.context_window_used_tokens
    }

    fn active_view(&self) -> Option<&dyn BottomPaneView> {
        self.view_stack.last().map(std::convert::AsRef::as_ref)
    }

    fn push_view(&mut self, view: Box<dyn BottomPaneView>) {
        self.key_chord_reset_requested = true;
        self.view_stack.push(view);
        self.schedule_active_view_frame();
        self.request_redraw();
    }

    fn pop_active_view_with_completion(&mut self, completion: Option<ViewCompletion>) {
        if self.view_stack.pop().is_some() {
            match completion {
                Some(ViewCompletion::Accepted) => {
                    while self
                        .view_stack
                        .last()
                        .is_some_and(|view| view.dismiss_after_child_accept())
                    {
                        self.view_stack.pop();
                    }
                }
                Some(ViewCompletion::Cancelled) => {
                    if let Some(view) = self.view_stack.last_mut() {
                        view.clear_dismiss_after_child_accept();
                    }
                }
                None => {}
            }
            self.on_view_stack_depth_decreased();
        }
    }

    fn on_view_stack_depth_decreased(&mut self) {
        self.key_chord_reset_requested = true;
        if self.view_stack.is_empty() {
            self.on_active_view_complete();
        }
    }

    // 008-TRIM: fn approval_prompt_delay_remaining / record_composer_activity_at /
    // maybe_show_delayed_approval_requests_at（延迟审批，008 §1 裁剪：无本地事件源）

    /// Preserve restricted drafts, allowing recovery commands only for connected unavailable threads.
    pub(crate) fn handle_restricted_key(
        &mut self,
        key: KeyEvent,
        mode: RestrictedInputMode,
    ) -> InputResult {
        self.view_stack.clear();
        // 008-TRIM: warnings_view/delayed_approval_requests 重置（008 §1 裁剪：无本地事件源）
        self.composer
            .set_input_enabled(/*enabled*/ true, /*placeholder*/ None);
        let result = self.composer.handle_restricted_key(key, mode);
        self.request_redraw();
        result
    }

    /// Forward a key event to the active view or the composer.
    pub fn handle_key_event(&mut self, key_event: KeyEvent) -> InputResult {
        self.handle_key_event_with_activation_key(key_event, /*activation_key*/ None)
    }

    pub(crate) fn handle_key_event_with_activation_key(
        &mut self,
        key_event: KeyEvent,
        activation_key: Option<KeyBinding>,
    ) -> InputResult {
        let routed_key = activation_key.unwrap_or_else(|| KeyBinding::from_event(key_event));
        if self.consume_view_transition_key(routed_key, key_event.kind) {
            return InputResult::None;
        }
        if key_event.kind == KeyEventKind::Release {
            // A modal may cover the config panel while its child editor is queued.
            // Let the panel see the release so the next fresh press is not swallowed.
            for view in &mut self.view_stack {
                view.handle_key_release(key_event);
            }
        } else if matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            && self.view_stack.len() > 1
        {
            let covered_view_count = self.view_stack.len() - 1;
            for view in &mut self.view_stack[..covered_view_count] {
                view.observe_key_event_while_covered(key_event, routed_key);
            }
        }
        // 008-TRIM: records_composer_activity 计算（仅服务于延迟审批与 warnings/questions，
        // 008 §1 裁剪：无本地事件源）
        // 008-TRIM: warnings_view 与 questions 展开态按键分支（008 §1 裁剪：无本地事件源）
        // If a modal/view is active, handle it here; otherwise forward to composer.
        if !self.view_stack.is_empty() {
            if key_event.kind == KeyEventKind::Release {
                return InputResult::None;
            }

            // We need three pieces of information after routing the key:
            // whether Esc completed the view, whether the view finished for any
            // reason, and whether a paste-burst timer should be scheduled.
            let (ctrl_c_completed, view_complete, completion, view_in_paste_burst) = {
                let last_index = self.view_stack.len() - 1;
                let view = &mut self.view_stack[last_index];
                let prefer_esc =
                    key_event.code == KeyCode::Esc && view.prefer_esc_to_handle_key_event();
                let ctrl_c_completed = key_event.code == KeyCode::Esc
                    && !prefer_esc
                    && matches!(view.on_ctrl_c(), CancellationEvent::Handled)
                    && view.is_complete();
                if ctrl_c_completed {
                    (true, true, view.completion(), false)
                } else {
                    view.handle_key_event_with_activation_key(key_event, activation_key);
                    (
                        false,
                        view.is_complete(),
                        view.completion(),
                        view.is_in_paste_burst(),
                    )
                }
            };

            if ctrl_c_completed {
                self.pop_active_view_with_completion(completion);
                if let Some(next_view) = self.view_stack.last()
                    && next_view.is_in_paste_burst()
                {
                    self.request_redraw_in(ChatComposer::recommended_paste_flush_delay());
                }
            } else if view_complete {
                self.pop_active_view_with_completion(completion);
            } else if view_in_paste_burst {
                self.request_redraw_in(ChatComposer::recommended_paste_flush_delay());
            }
            self.request_redraw();
            InputResult::None
        } else {
            // 008-TRIM: inline banner 按键挂钩（裁剪功能，008 §1）
            // Allow the configured interrupt while a task is running, even when its
            // status row is hidden or the composer has focus.
            // When a popup is active, prefer dismissing it over interrupting the task.
            if self.should_interrupt_running_task(key_event) {
                if let Some(status) = &self.status {
                    status.interrupt();
                } else {
                    self.app_event_tx.interrupt();
                }
                self.request_redraw();
                return InputResult::None;
            }
            let (input_result, needs_redraw) = self.composer.handle_key_event(key_event);
            // 008-TRIM: record_composer_activity_at 记录（延迟审批，008 §1 裁剪）
            if needs_redraw {
                self.request_redraw();
            }
            if self.composer.is_in_paste_burst() {
                self.request_redraw_in(ChatComposer::recommended_paste_flush_delay());
            }
            input_result
        }
    }

    /// Return the contexts whose ordinary handlers can consume the next key.
    pub(crate) fn keymap_contexts(&self) -> KeymapContextSet {
        // 008-TRIM: warnings_view/questions 上下文分支（008 §1 裁剪：无本地事件源）
        if let Some(view) = self.view_stack.last() {
            view.keymap_contexts()
        } else {
            self.composer.keymap_contexts()
        }
    }

    /// Handles a Ctrl+C press within the bottom pane.
    ///
    /// An active modal view is given the first chance to consume the key (typically to dismiss
    /// itself). If no view is active, Ctrl+C cancels active history search before falling back to
    /// clearing draft composer input.
    ///
    /// This method may show the quit shortcut hint as a user-visible acknowledgement that Ctrl+C
    /// was received, but it does not decide whether the process should exit; `ChatWidget` owns the
    /// quit/interrupt state machine and uses the result to decide what happens next.
    pub(crate) fn on_ctrl_c(&mut self) -> CancellationEvent {
        // 008-TRIM: warnings_view/questions Ctrl+C 分支（008 §1 裁剪：无本地事件源）
        if let Some(view) = self.view_stack.last_mut() {
            let event = view.on_ctrl_c();
            let view_complete = view.is_complete();
            let completion = view.completion();
            if matches!(event, CancellationEvent::Handled) {
                if view_complete {
                    self.pop_active_view_with_completion(completion);
                }
                self.show_quit_shortcut_hint(key_hint::ctrl(KeyCode::Char('c')));
                self.request_redraw();
            }
            event
        } else if self.composer.cancel_vim_search() || self.composer.cancel_history_search() {
            self.request_redraw();
            CancellationEvent::Handled
        } else if self.composer_is_empty() {
            CancellationEvent::NotHandled
        } else {
            self.view_stack.pop();
            self.clear_composer_for_ctrl_c();
            self.show_quit_shortcut_hint(key_hint::ctrl(KeyCode::Char('c')));
            self.request_redraw();
            CancellationEvent::Handled
        }
    }

    pub fn handle_paste(&mut self, pasted: String) {
        // 008-TRIM: config 编辑器打断、warnings_view/questions 粘贴分支与
        // record_composer_activity_at 记录（008 §1 裁剪：无本地事件源）
        if let Some(view) = self.view_stack.last_mut() {
            let needs_redraw = view.handle_paste(pasted);
            let view_complete = view.is_complete();
            if view_complete {
                self.view_stack.clear();
                self.on_active_view_complete();
            }
            if needs_redraw || view_complete {
                self.request_redraw();
            }
        } else {
            let needs_redraw = self.composer.handle_paste(pasted);
            if needs_redraw {
                self.request_redraw();
            }
        }
    }

    pub(crate) fn insert_str(&mut self, text: &str) {
        self.composer.insert_str(text);
        self.request_redraw();
    }

    pub(crate) fn pre_draw_tick(&mut self) {
        self.pre_draw_tick_at(Instant::now());
    }

    fn pre_draw_tick_at(&mut self, now: Instant) {
        self.composer.sync_popups();
        // 008-TRIM: maybe_show_delayed_approval_requests_at 调用（延迟审批，008 §1 裁剪）
        self.tick_active_view(now);
        self.schedule_active_view_frame();
    }

    fn tick_active_view(&mut self, now: Instant) {
        let Some(view) = self.view_stack.last_mut() else {
            return;
        };
        let needs_redraw = view.pre_draw_tick(now);
        let view_complete = view.is_complete();
        if view_complete {
            self.view_stack.clear();
            self.on_active_view_complete();
        }
        if needs_redraw || view_complete {
            self.key_chord_reset_requested = true;
            self.request_redraw();
        }
    }

    fn schedule_active_view_frame(&self) {
        // 008-TRIM: questions 帧调度（008 §1 裁剪：无本地事件源）
        if let Some(delay) = self.active_view().map_or_else(
            || self.composer.footer_flash_delay(),
            BottomPaneView::next_frame_delay,
        ) {
            self.request_redraw_in(delay);
        }
    }

    /// Replace the composer text with `text`.
    ///
    /// This is intended for fresh input where mention linkage does not need to
    /// survive; it routes to `ChatComposer::set_text_content`, which resets
    /// mention bindings.
    pub(crate) fn set_composer_text(
        &mut self,
        text: String,
        text_elements: Vec<TextElement>,
        local_image_paths: Vec<PathBuf>,
    ) {
        self.composer
            .set_text_content(text, text_elements, local_image_paths);
        self.composer.move_cursor_to_end();
        self.request_redraw();
    }

    /// Replace the composer text while preserving mention link targets.
    ///
    /// Use this when rehydrating a draft after a local validation/gating
    /// failure (for example unsupported image submit) so previously selected
    /// mention targets remain stable across retry.
    pub(crate) fn set_composer_text_with_mention_bindings(
        &mut self,
        text: String,
        text_elements: Vec<TextElement>,
        local_image_paths: Vec<PathBuf>,
        mention_bindings: Vec<MentionBinding>,
    ) {
        self.composer.edit_stored_draft(|composer| {
            composer.set_text_content_with_mention_bindings(
                text,
                text_elements,
                local_image_paths,
                mention_bindings,
            );
            composer.move_cursor_to_end();
        });
        self.request_redraw();
    }

    #[allow(dead_code)]
    pub(crate) fn set_composer_input_enabled(
        &mut self,
        enabled: bool,
        placeholder: Option<String>,
    ) {
        self.composer.set_input_enabled(enabled, placeholder);
        self.request_redraw();
    }

    pub(crate) fn show_shutdown_in_progress(&mut self) {
        self.view_stack.clear();
        // 008-TRIM: warnings_view 重置（008 §1 裁剪：无本地事件源）
        self.composer.show_shutdown_in_progress();
        self.request_redraw();
    }

    /// 008-ADAPT（驱动命令派发）：清空 composer 草稿。母本经 popup 派发清空；
    /// 本地直发命令时由驱动调用，防止后续输入追加到已派发的命令文本。
    pub fn clear_composer_for_command(&mut self) {
        self.composer.clear_for_command_dispatch();
    }

    pub(crate) fn clear_composer_for_ctrl_c(&mut self) {
        if let Some(text) = self.composer.clear_for_ctrl_c() {
            // 008-TRIM 适配: 本地 ThreadId shim 非 Copy，改克隆传值（语义不变）。
            if let Some(thread_id) = self.thread_id.clone() {
                self.app_event_tx
                    .send(AppEvent::AppendMessageHistoryEntry { thread_id, text });
            } else {
                tracing::warn!(
                    "failed to append Ctrl+C-cleared draft to history: no active thread id"
                );
            }
        }
        self.request_redraw();
    }

    /// Get the current composer text (for tests and programmatic checks).
    pub(crate) fn composer_text(&self) -> String {
        self.composer.current_text()
    }

    #[cfg(test)]
    pub(crate) fn composer_cursor(&self) -> usize {
        self.composer.current_cursor()
    }

    #[cfg(test)]
    pub(crate) fn composer_text_elements(&self) -> Vec<TextElement> {
        self.composer.text_elements()
    }

    pub(crate) fn composer_local_images(&self) -> Vec<LocalImageAttachment> {
        self.composer.local_images()
    }

    #[cfg(test)]
    pub(crate) fn composer_local_image_paths(&self) -> Vec<PathBuf> {
        self.composer.local_image_paths()
    }

    pub(crate) fn composer_text_with_pending(&self) -> String {
        self.composer.current_text_with_pending()
    }

    /// Returns whether the composer currently accepts interactive draft edits.
    pub(crate) fn composer_input_enabled(&self) -> bool {
        self.composer.input_enabled()
    }

    pub(crate) fn composer_pending_pastes(&self) -> Vec<(String, String)> {
        self.composer.draft_snapshot().pending_pastes
    }

    pub(crate) fn apply_external_edit(&mut self, text: String) {
        self.composer.apply_external_edit(text);
        self.request_redraw();
    }

    pub(crate) fn set_footer_hint_override(&mut self, items: Option<Vec<(String, String)>>) {
        // 008-TRIM: warnings_view 提示同步（008 §1 裁剪：无本地事件源）
        self.composer.set_footer_hint_override(items);
        self.request_redraw();
    }

    // 008-TRIM: fn set_voice_strip（voice，008 §1 裁剪：无本地事件源）

    pub(crate) fn set_remote_image_urls(&mut self, urls: Vec<String>) {
        self.composer
            .edit_stored_draft(|composer| composer.set_remote_image_urls(urls));
        self.request_redraw();
    }

    pub(crate) fn remote_image_urls(&self) -> Vec<String> {
        self.composer.remote_image_urls()
    }

    pub(crate) fn take_remote_image_urls(&mut self) -> Vec<String> {
        let urls = self.composer.take_remote_image_urls();
        self.request_redraw();
        urls
    }

    pub(crate) fn set_composer_pending_pastes(&mut self, pending_pastes: Vec<(String, String)>) {
        self.composer
            .edit_stored_draft(|composer| composer.set_pending_pastes(pending_pastes));
        self.request_redraw();
    }

    /// Update the status indicator header (defaults to "Working") and details below it.
    ///
    /// Passing `None` clears any existing details. Returns whether the active status indicator
    /// was updated and requested a redraw.
    pub(crate) fn update_status(
        &mut self,
        header: String,
        details: Option<String>,
        details_capitalization: StatusDetailsCapitalization,
        details_max_lines: usize,
    ) -> bool {
        if let Some(status) = self.status.as_mut() {
            status.update_header(header);
            status.update_details(details, details_capitalization, details_max_lines.max(1));
            self.request_redraw();
            return true;
        }
        false
    }

    /// Show the transient "press again to quit" hint for `key`.
    ///
    /// `ChatWidget` owns the quit shortcut state machine (it decides when quit is
    /// allowed), while the bottom pane owns rendering. We also schedule a redraw
    /// after [`QUIT_SHORTCUT_TIMEOUT`] so the hint disappears even if the user
    /// stops typing and no other events trigger a draw.
    pub(crate) fn show_quit_shortcut_hint(&mut self, key: KeyBinding) {
        if !DOUBLE_PRESS_QUIT_SHORTCUT_ENABLED {
            return;
        }

        self.composer
            .show_quit_shortcut_hint(key, self.has_input_focus);
        let frame_requester = self.frame_requester.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                tokio::time::sleep(QUIT_SHORTCUT_TIMEOUT).await;
                frame_requester.schedule_frame();
            });
        } else {
            // In tests (and other non-Tokio contexts), fall back to a thread so
            // the hint can still expire without requiring an explicit draw.
            std::thread::spawn(move || {
                std::thread::sleep(QUIT_SHORTCUT_TIMEOUT);
                frame_requester.schedule_frame();
            });
        }
        self.request_redraw();
    }

    /// Clear the "press again to quit" hint immediately.
    pub(crate) fn clear_quit_shortcut_hint(&mut self) {
        self.composer.clear_quit_shortcut_hint(self.has_input_focus);
        self.request_redraw();
    }

    #[cfg(test)]
    pub(crate) fn quit_shortcut_hint_visible(&self) -> bool {
        self.composer.quit_shortcut_hint_visible()
    }

    #[cfg(test)]
    pub(crate) fn status_indicator_visible(&self) -> bool {
        self.status_widget().is_some()
    }

    #[cfg(test)]
    pub(crate) fn status_line_text(&self) -> Option<String> {
        self.composer.status_line_text()
    }

    pub(crate) fn show_esc_backtrack_hint(&mut self) {
        self.esc_backtrack_hint = true;
        self.composer.set_esc_backtrack_hint(/*show*/ true);
        self.request_redraw();
    }

    pub(crate) fn clear_esc_backtrack_hint(&mut self) {
        if self.esc_backtrack_hint {
            self.esc_backtrack_hint = false;
            self.composer.set_esc_backtrack_hint(/*show*/ false);
            self.request_redraw();
        }
    }

    // esc_backtrack_hint_visible removed; hints are controlled internally.

    pub fn set_task_running(&mut self, running: bool) {
        let was_running = self.is_task_running;
        self.is_task_running = running;
        self.composer.set_task_running(running);

        if running {
            if !was_running {
                self.status_timer.reset(Duration::ZERO);
                if self.status.is_none() {
                    self.status = Some(StatusIndicatorWidget::new(
                        self.app_event_tx.clone(),
                        self.frame_requester.clone(),
                        self.animations_enabled,
                        self.effects,
                    ));
                }
                if let Some(status) = self.status.as_mut() {
                    status.set_interrupt_hint_visible(/*visible*/ true);
                    status.set_interrupt_binding(
                        self.keymap
                            .primary_hint(KeymapContext::Chat, "interrupt_turn"),
                    );
                }
        // 008-TRIM: sync_status_inline_message 调用（008 §1）
                self.request_redraw();
            }
        } else {
            // Hide the status indicator when a task completes, but keep other modal views.
            self.hide_status_indicator();
        }
    }

    pub(crate) fn set_queue_submissions(&mut self, queue_submissions: bool) {
        self.composer.set_queue_submissions(queue_submissions);
    }

    /// Hide the status indicator while leaving task-running state untouched.
    pub(crate) fn hide_status_indicator(&mut self) {
        if self.status.take().is_some() {
            self.request_redraw();
        }
    }

    pub(crate) fn ensure_status_indicator(&mut self) {
        if self.status.is_none() {
            self.status.get_or_insert_with(|| {
                StatusIndicatorWidget::new(
                    self.app_event_tx.clone(),
                    self.frame_requester.clone(),
                    self.animations_enabled,
                    self.effects,
                )
            });
            if let Some(status) = self.status.as_mut() {
                status.set_interrupt_binding(
                    self.keymap
                        .primary_hint(KeymapContext::Chat, "interrupt_turn"),
                );
            }
        // 008-TRIM: sync_status_inline_message 调用（008 §1）
            self.request_redraw();
        }
    }

    pub(crate) fn set_interrupt_hint_visible(&mut self, visible: bool) {
        if let Some(status) = self.status.as_mut() {
            status.set_interrupt_hint_visible(visible);
            self.request_redraw();
        }
    }

    pub(crate) fn set_context_window(&mut self, percent: Option<i64>, used_tokens: Option<i64>) {
        if self.context_window_percent == percent && self.context_window_used_tokens == used_tokens
        {
            return;
        }

        self.context_window_percent = percent;
        self.context_window_used_tokens = used_tokens;
        self.composer
            .set_context_window(percent, self.context_window_used_tokens);
        self.request_redraw();
    }

    pub(crate) fn set_context_window_pending(&mut self, pending: bool) {
        self.composer.set_context_window_pending(pending);
        self.request_redraw();
    }

    /// Show a generic list selection view with the provided items.
    pub(crate) fn show_selection_view(
        &mut self,
        mut params: list_selection_view::SelectionViewParams,
    ) {
        self.apply_standard_popup_hint(&mut params);
        let view = list_selection_view::ListSelectionView::new(
            params,
            self.app_event_tx.clone(),
            self.keymap.list.clone(),
        );
        self.push_view(Box::new(view));
    }

    fn apply_standard_popup_hint(&self, params: &mut list_selection_view::SelectionViewParams) {
        // Configured list actions take precedence over optional row shortcuts.
        for item in &mut params.items {
            if item.secondary_action.as_ref().is_some_and(|secondary| {
                let (code, modifiers) = secondary.key.parts();
                self.keymap
                    .list
                    .action_for(KeyEvent::new(code, modifiers))
                    .is_some()
            }) {
                item.secondary_action = None;
            }
        }
        if !params.allow_cancel {
            if params.footer_hint.is_none()
                || params.footer_hint.as_ref() == Some(&popup_consts::standard_popup_hint_line())
            {
                params.footer_hint = None;
            }
            return;
        }
        if params.footer_hint.is_none()
            || params.footer_hint.as_ref() == Some(&popup_consts::standard_popup_hint_line())
        {
            params.footer_hint = Some(popup_consts::picker_hint_line_for_keymap(&self.keymap.list));
        }
    }

    /// Replace the active selection view when it matches `view_id`.
    pub(crate) fn replace_selection_view_if_active(
        &mut self,
        view_id: &'static str,
        mut params: list_selection_view::SelectionViewParams,
    ) -> bool {
        let is_match = self
            .view_stack
            .last()
            .is_some_and(|view| view.view_id() == Some(view_id));
        if !is_match {
            return false;
        }

        let search_query = if params.is_searchable {
            self.view_stack
                .last()
                .and_then(|view| view.search_query())
                .map(str::to_string)
        } else {
            None
        };
        self.view_stack.pop();
        self.apply_standard_popup_hint(&mut params);
        let mut view = list_selection_view::ListSelectionView::new(
            params,
            self.app_event_tx.clone(),
            self.keymap.list.clone(),
        );
        if let Some(search_query) = search_query {
            view.set_search_query(search_query);
        }
        self.push_view(Box::new(view));
        true
    }

    /// Replace the newest matching selection view without disturbing views stacked above it.
    /// Preserve pending parent cleanup when an already-open child is accepted.
    pub(crate) fn replace_selection_view_if_present(
        &mut self,
        view_id: &'static str,
        mut params: list_selection_view::SelectionViewParams,
    ) -> bool {
        let Some(index) = self
            .view_stack
            .iter()
            .rposition(|view| view.view_id() == Some(view_id))
        else {
            return false;
        };

        let replaces_active_view = index + 1 == self.view_stack.len();
        self.apply_standard_popup_hint(&mut params);
        let mut view = list_selection_view::ListSelectionView::new(
            params,
            self.app_event_tx.clone(),
            self.keymap.list.clone(),
        );
        view.dismiss_after_child_accept = self.view_stack[index].dismiss_after_child_accept();
        self.view_stack[index] = Box::new(view);
        if replaces_active_view {
            self.key_chord_reset_requested = true;
            self.schedule_active_view_frame();
        }
        self.request_redraw();
        true
    }

    pub(crate) fn replace_view_if_present(
        &mut self,
        view_id: &'static str,
        view: Box<dyn BottomPaneView>,
    ) {
        if let Some(index) = self
            .view_stack
            .iter()
            .rposition(|existing| existing.view_id() == Some(view_id))
        {
            self.view_stack[index] = view;
            self.request_redraw();
        }
    }

    pub(crate) fn list_keymap(&self) -> crate::keymap::ListKeymap {
        self.keymap.list.clone()
    }

    /// Replace one or more active views whose IDs are in `view_ids` with a
    /// generic list selection view.
    pub(crate) fn replace_active_views_with_selection_view(
        &mut self,
        view_ids: &[&'static str],
        mut params: list_selection_view::SelectionViewParams,
    ) -> bool {
        let is_match = self
            .view_stack
            .last()
            .and_then(|view| view.view_id())
            .is_some_and(|view_id| view_ids.contains(&view_id));
        if !is_match {
            return false;
        }

        while self
            .view_stack
            .last()
            .and_then(|view| view.view_id())
            .is_some_and(|view_id| view_ids.contains(&view_id))
        {
            self.view_stack.pop();
        }
        self.apply_standard_popup_hint(&mut params);
        let view = list_selection_view::ListSelectionView::new(
            params,
            self.app_event_tx.clone(),
            self.keymap.list.clone(),
        );
        self.push_view(Box::new(view));
        true
    }

    pub(crate) fn selected_index_for_active_view(&self, view_id: &'static str) -> Option<usize> {
        self.view_stack
            .last()
            .filter(|view| view.view_id() == Some(view_id))
            .and_then(|view| view.selected_index())
    }

    pub(crate) fn selection_toggle_states_for_active_view(
        &self,
        view_id: &'static str,
    ) -> Option<Vec<Option<SelectionToggleState>>> {
        self.view_stack
            .last()
            .filter(|view| view.view_id() == Some(view_id))
            .map(|view| view.selection_toggle_states())
    }

    pub(crate) fn complete_selection_toggle_for_active_view(
        &mut self,
        view_id: &'static str,
        index: usize,
    ) {
        if let Some(view) = self
            .view_stack
            .last_mut()
            .filter(|view| view.view_id() == Some(view_id))
        {
            view.complete_selection_toggle(index);
        }
    }

    pub(crate) fn selected_index_for_present_view(&self, view_id: &'static str) -> Option<usize> {
        self.view_stack
            .iter()
            .rfind(|view| view.view_id() == Some(view_id))
            .and_then(|view| view.selected_index())
    }

    pub(crate) fn active_tab_id_for_active_view(&self, view_id: &'static str) -> Option<&str> {
        self.view_stack
            .last()
            .filter(|view| view.view_id() == Some(view_id))
            .and_then(|view| view.active_tab_id())
    }

    /// Forward a suggestion to its matching prompt, even beneath another view.
    pub(crate) fn apply_text_suggestion(
        &mut self,
        request_id: uuid::Uuid,
        suggestion: Option<&str>,
    ) -> bool {
        let changed = self
            .view_stack
            .iter_mut()
            .rev()
            .any(|view| view.apply_text_suggestion(request_id, suggestion));

        if changed {
            self.request_redraw();
        }

        changed
    }

    pub(crate) fn dismiss_active_view_if_id(&mut self, view_id: &'static str) -> bool {
        let is_match = self
            .view_stack
            .last()
            .is_some_and(|view| view.view_id() == Some(view_id));
        if !is_match {
            return false;
        }

        self.key_chord_reset_requested = true;
        self.view_stack.pop();
        self.request_redraw();
        true
    }

    /// Dismiss the newest matching view without disturbing views stacked above it.
    pub(crate) fn dismiss_view_by_id(&mut self, view_id: &'static str) -> bool {
        let Some(index) = self
            .view_stack
            .iter()
            .rposition(|view| view.view_id() == Some(view_id))
        else {
            return false;
        };

        let removed_active_view = index + 1 == self.view_stack.len();
        self.view_stack.remove(index);
        if removed_active_view {
            self.key_chord_reset_requested = true;
            self.schedule_active_view_frame();
        }
        self.request_redraw();
        true
    }

    /// Update the pending-input preview shown above the composer.
    pub(crate) fn set_pending_input_preview(
        &mut self,
        queued: Vec<String>,
        pending_steers: Vec<String>,
        rejected_steers: Vec<String>,
    ) {
        self.pending_input_preview.pending_steers = pending_steers;
        self.pending_input_preview.rejected_steers = rejected_steers;
        self.pending_input_preview.queued_messages = queued;
        self.request_redraw();
    }

    // 008-TRIM: fn set_pending_thread_approvals / pending_thread_approvals /
    // set_unified_exec_processes（pending thread approvals 与 unified exec footer，
    // 008 §1 裁剪：无本地事件源）

    // 008-TRIM: set_hook_status_message（hook 功能，008 §1 裁剪）

    // 008-TRIM: sync_status_inline_message（输入均属裁剪功能，008 §1）

    pub(crate) fn composer_is_empty(&self) -> bool {
        self.composer.is_empty()
    }

    pub(crate) fn composer_is_vim_enabled(&self) -> bool {
        self.composer.is_vim_enabled()
    }

    pub(crate) fn composer_should_handle_vim_insert_escape(&self, key_event: KeyEvent) -> bool {
        self.composer.should_handle_vim_insert_escape(key_event)
    }

    pub(crate) fn is_task_running(&self) -> bool {
        self.is_task_running
    }

    pub(crate) fn should_interrupt_running_task(&self, key_event: KeyEvent) -> bool {
        let is_agent_command = self
            .composer_text()
            .lines()
            .next()
            .and_then(parse_slash_name)
            .is_some_and(|(name, _, _)| matches!(name, "agents" | "subagents"));

        self.keymap.chat.interrupt_turn.is_pressed(key_event)
            && !(self.shortcut_overlay_visible()
                && key_hint::plain(KeyCode::Esc).is_press(key_event))
            && self.is_task_running
            && !(is_agent_command && key_event.code == KeyCode::Esc)
            && self.no_modal_or_popup_active()
            && !self.composer_should_handle_vim_insert_escape(key_event)
    }

    pub(crate) fn terminal_title_requires_action(&self) -> bool {
        // 008-TRIM: questions 未答计数（008 §1 裁剪：无本地事件源）
        self.active_view()
            .is_some_and(bottom_pane_view::BottomPaneView::terminal_title_requires_action)
    }

    pub(crate) fn has_centered_view(&self) -> bool {
        self.active_view()
            .is_some_and(|view| view.presentation() == ViewPresentation::Centered)
    }

    pub(crate) fn has_active_view(&self) -> bool {
        // 008-TRIM: warnings_view 分支（008 §1 裁剪：无本地事件源）
        self.has_active_modal()
    }

    /// Warnings own input without replacing the composer with an interactive modal.
    pub(crate) fn has_active_modal(&self) -> bool {
        !self.view_stack.is_empty()
    }

    pub(crate) fn active_view_accepts_input_when_disconnected(&self) -> bool {
        self.active_view()
            .is_some_and(BottomPaneView::accepts_input_when_disconnected)
    }

    pub(crate) fn active_view_will_interrupt_turn_on_key_event(&self, key_event: KeyEvent) -> bool {
        self.is_task_running
            && self
                .active_view()
                .is_some_and(|view| view.will_interrupt_turn_on_key_event(key_event))
    }

    pub(crate) fn active_view_id(&self) -> Option<&'static str> {
        self.view_stack.last().and_then(|view| view.view_id())
    }

    /// Return true when the pane is in the regular composer state without any
    /// overlays or popups and not running a task. This is the safe context to
    /// use Esc-Esc for backtracking from the main view.
    pub(crate) fn is_normal_backtrack_mode(&self) -> bool {
        // 008-TRIM: inline_banner_accepts_dismissal 条件（008 §1 裁剪：无本地事件源）
        !self.is_task_running
            && !self.shortcut_overlay_visible()
            && self.can_launch_external_editor()
    }

    pub(crate) fn shortcut_overlay_visible(&self) -> bool {
        self.no_modal_or_popup_active() && self.composer.shortcut_overlay_visible()
    }

    /// Return true when no popups or modal views are active, regardless of task state.
    pub(crate) fn can_launch_external_editor(&self) -> bool {
        // 008-TRIM: questions 展开态条件（008 §1 裁剪：无本地事件源）
        !self.has_active_view() && !self.composer.popup_active()
    }

    /// Returns true when the bottom pane has no active modal view and no active composer popup.
    ///
    /// This is the UI-level definition of "no modal/popup is active" for key routing decisions.
    /// It intentionally does not include task state, since some actions are safe while a task is
    /// running and some are not.
    pub(crate) fn no_modal_or_popup_active(&self) -> bool {
        self.can_launch_external_editor()
    }

    /// 012 §9-ADAPT（复合态收尾轮）：composer 历史搜索态查询——驱动据此在
    /// 「Enter＋工具聚焦」分支让路（输入子模式按搜索规则处理 Enter，不被
    /// 详情页入口抢占）。
    pub(crate) fn history_search_active(&self) -> bool {
        self.composer.history_search_active()
    }

    pub(crate) fn clear_composer_selection(&mut self) {
        self.composer.clear_mouse_selection();
    }

    pub(crate) fn end_composer_drag(&mut self) {
        self.composer.end_mouse_drag();
    }

    pub(crate) fn cancel_composer_primary(&mut self) {
        self.composer.cancel_primary();
    }

    pub(crate) fn finish_composer_copy(
        &mut self,
        completion: &(u64, crate::clipboard_copy::worker::CopyResult),
        visible: bool,
    ) -> Option<usize> {
        let current = visible && !self.has_active_view();
        self.composer.finish_copy(completion, current)
    }

    pub(crate) fn copy_composer_selection(
        &mut self,
        event: &crate::tui::TuiEvent,
        copy: impl FnOnce(&str) -> Result<crate::clipboard_copy::CopyStatus, String>,
    ) -> Option<(usize, Result<crate::clipboard_copy::CopyStatus, String>)> {
        // 008-TRIM: questions 展开态守卫（008 §1 裁剪：无本地事件源）
        if self.has_active_view() {
            return None;
        }
        self.composer.copy_selection(event, copy)
    }

    pub(crate) fn copy_composer_selection_on_release(
        &mut self,
        event: crossterm::event::MouseEvent,
        selection_copy: crate::tui::SelectionCopy,
        copy: impl FnOnce(
            &str,
            crate::tui::SelectionCopy,
        ) -> (
            crate::clipboard_copy::worker::CopyResult,
            Option<std::sync::Arc<()>>,
        ),
    ) -> Option<(usize, Option<crate::clipboard_copy::worker::CopyResult>)> {
        // 008-TRIM: questions 展开态守卫（008 §1 裁剪：无本地事件源）
        if self.has_active_view() {
            return None;
        }
        self.composer
            .copy_selection_on_release(event, selection_copy, copy)
    }

    pub(crate) fn clipboard_paste_target(
        &self,
        shortcut: Option<KeyCode>,
    ) -> Option<ClipboardPasteTarget> {
        if self.has_active_view() || (shortcut.is_none() && !self.no_modal_or_popup_active()) {
            return None;
        }
        self.composer.clipboard_paste_target(shortcut)
    }

    pub(crate) fn prepare_composer_mouse(&mut self, event: crossterm::event::MouseEvent) -> bool {
        if event.kind != crossterm::event::MouseEventKind::Moved {
            self.view_transition_key_guard = None;
        }
        // 008-TRIM: questions 展开态守卫（008 §1 裁剪：无本地事件源）
        if self.has_active_view() {
            self.composer.end_mouse_drag();
            return false;
        }
        self.composer.prepare_mouse(event)
    }

    pub(crate) fn handle_composer_mouse(&mut self, event: crossterm::event::MouseEvent) -> bool {
        self.composer.handle_mouse(event)
    }

    pub(crate) fn show_view(&mut self, view: Box<dyn BottomPaneView>) {
        self.push_view(view);
    }

    pub(crate) fn show_user_input(
        &mut self,
        questions: Vec<codex_protocol::request_user_input::RequestUserInputQuestion>,
        reply: crate::chat_driver::UserInputReply,
        live: std::sync::Arc<std::sync::atomic::AtomicBool>,
        display_filter: fn(&str) -> String,
    ) {
        self.show_view(Box::new(
            request_user_input::RequestUserInputOverlay::new_with_keymap(
                questions,
                reply,
                live,
                display_filter,
                self.app_event_tx.clone(),
                self.disable_paste_burst,
                self.keymap.clone(),
            ),
        ));
    }

    pub(crate) fn end_user_input(
        &mut self,
        reason: codex_protocol::request_user_input::RequestUserInputError,
    ) {
        if let Some(view) = self
            .view_stack
            .iter_mut()
            .find(|v| v.view_id() == Some("user-input"))
        {
            view.finish_user_input(reason);
        }
        self.dismiss_active_view_if_id("user-input");
    }

    pub(crate) fn suppress_current_view_key_until_release(
        &mut self,
        key: KeyBinding,
        activated_at: Instant,
    ) {
        self.view_transition_key_guard =
            Some(crate::activation_key_guard::ActivationKeyGuard::new_at(
                key,
                activated_at,
                self.enhanced_keys_supported,
            ));
    }

    pub(crate) fn consume_view_transition_key_event(&mut self, key_event: KeyEvent) -> bool {
        self.consume_view_transition_key(KeyBinding::from_event(key_event), key_event.kind)
    }

    fn consume_view_transition_key(&mut self, key: KeyBinding, kind: KeyEventKind) -> bool {
        let Some(guard) = self.view_transition_key_guard.as_mut() else {
            return false;
        };
        let consumed = guard.consume(key, kind);
        if !guard.is_active() {
            self.view_transition_key_guard = None;
        }
        consumed
    }

    // 008-TRIM: fn show_text_prompt（custom_prompt_view，008 §1 裁剪：无本地事件源）

    // 008-TRIM: fn push_approval_request / push_user_input_request /
    // push_mcp_server_elicitation_request / dismiss_app_server_request
    // （审批、请求用户输入、MCP elicitation、app-server 请求撤除，008 §1 裁剪：无本地事件源）

    fn on_active_view_complete(&mut self) {
        self.resume_status_timer_after_modal();
        self.set_composer_input_enabled(/*enabled*/ true, /*placeholder*/ None);
    }

    fn pause_status_timer_for_modal(&mut self) {
        self.status_timer.pause_at(Instant::now());
    }

    fn resume_status_timer_after_modal(&mut self) {
        self.status_timer.resume_at(Instant::now());
        self.request_redraw();
    }

    /// Height (terminal rows) required by the current bottom pane.
    pub(crate) fn request_redraw(&self) {
        self.frame_requester.schedule_frame();
    }

    pub(crate) fn request_redraw_in(&self, dur: Duration) {
        self.frame_requester.schedule_frame_in(dur);
    }

    // --- History helpers ---

    pub(crate) fn set_history_metadata(
        &mut self,
        thread_id: ThreadId,
        log_id: u64,
        entry_count: usize,
    ) {
        self.composer
            .set_history_metadata(thread_id.clone(), log_id, entry_count);
        // 008-TRIM 适配: 本地 ThreadId shim 非 Copy，先传值再保存（语义不变）。
        self.thread_id = Some(thread_id);
    }

    pub(crate) fn flush_paste_burst_if_due(&mut self) -> bool {
        // Every editor can retain buffered input after losing focus. Flush all
        // of them, matching the buffers that can defer drawing below.
        let mut flushed = self.composer.flush_paste_burst_if_due();
        if let Some(view) = self.view_stack.last_mut() {
            flushed |= view.flush_paste_burst_if_due();
        }
        // 008-TRIM: questions 冲洗（008 §1 裁剪：无本地事件源）
        flushed
    }

    /// Commit accepted typing before snapshotting the target of an asynchronous paste.
    pub(crate) fn prepare_composer_clipboard_paste(&mut self) {
        self.composer.flush_pending_paste_burst();
    }

    pub(crate) fn is_in_paste_burst(&self) -> bool {
        // 008-TRIM: questions 冲洗态（008 §1 裁剪：无本地事件源）
        self.view_stack
            .last()
            .is_some_and(|view| view.is_in_paste_burst())
            || self.composer.is_in_paste_burst()
    }

    pub(crate) fn on_history_lookup_response(&mut self, response: HistoryLookupResponse) {
        // 008-TRIM: questions 编辑器历史链（008 §1 裁剪：无本地事件源）
        let updated = match response {
            HistoryLookupResponse::Entry {
                offset,
                log_id,
                entry,
            } => self.composer.on_history_entry_response(log_id, offset, entry),
            HistoryLookupResponse::Batch {
                cursor,
                log_id,
                entries,
                next_older_cursor,
            } => self
                .composer
                .on_history_batch_response(log_id, cursor, entries, next_older_cursor),
            HistoryLookupResponse::BatchError { cursor, log_id } => {
                self.composer.on_history_batch_error(log_id, cursor)
            }
        };
        if updated {
            self.request_redraw();
        }
    }

    pub(crate) fn record_replayed_user_message_history(&mut self, entry: HistoryEntry) {
        self.composer.record_replayed_user_message_history(entry);
    }

    // 008-TRIM: fn on_file_search_result（file search，008 §1 裁剪：无本地事件源）

    
/// 020 §2.3：提交被拒后恢复草稿——被取出的 D0（文本＋附件路径）与当前
    /// 新草稿 Dnew 按 Codex 顺序合并（D0 在前、Dnew 换行接续），附件合并后由
    /// composer 重建占位符并整体重编号；不复制进队列。
    pub(crate) fn restore_rejected_submission(
        &mut self,
        text: String,
        image_paths: Vec<PathBuf>,
    ) {
        // Dnew：当前草稿（文本＋附件元素）。元素字面按区间剔除——重建时
        // 统一按合并后的附件顺序生成新标记（旧字面若保留会成为普通文本
        // 与新元素重复编号；审查修复：原先直接拼接导致重编号失效）。
        let dnew_text = self.composer.current_text();
        let dnew_elements = self.composer.text_elements();
        let mut clean_dnew = dnew_text.clone();
        for el in dnew_elements.iter().rev() {
            let end = el.byte_range.end.min(clean_dnew.len());
            if el.byte_range.start <= end {
                clean_dnew.replace_range(el.byte_range.start..end, "");
            }
        }
        let clean_dnew = clean_dnew.trim().to_string();
        let dnew_images = self.composer.local_images();
        let mut paths = image_paths;
        paths.extend(dnew_images.into_iter().map(|a| a.path));
        // 合并文本＝按附件数重建前置标记（元素化）＋D0 正文（剔除旧标记
        // 字面——D0 的标记由系统生成，重建后统一编号）＋换行接 Dnew 正文。
        let clean_d0 = strip_system_image_markers(&text);
        let mut merged = String::new();
        let mut elements: Vec<codex_protocol::user_input::TextElement> = Vec::new();
        use codex_protocol::user_input::{ByteRange, TextElement};
        for n in 1..=paths.len() {
            if !merged.is_empty() {
                merged.push(' ');
            }
            let start = merged.len();
            let marker = codex_protocol::models::local_image_label_text(n).to_string();
            merged.push_str(&marker);
            elements.push(TextElement::new(
                ByteRange { start, end: merged.len() },
                Some(marker),
            ));
        }
        if !clean_d0.is_empty() {
            if !merged.is_empty() {
                merged.push(' ');
            }
            merged.push_str(&clean_d0);
        }
        if !clean_dnew.is_empty() {
            merged.push('\n');
            merged.push_str(&clean_dnew);
        }
        self.composer.set_text_content(merged, elements, paths);
        self.composer.move_cursor_to_end();
        self.request_redraw();
    }

    pub(crate) fn attach_image(&mut self, path: PathBuf) {
        if !self.has_active_view() {
            self.composer.attach_image(path);
            self.request_redraw();
        }
    }

    #[cfg(test)]
    pub(crate) fn take_recent_submission_images(&mut self) -> Vec<PathBuf> {
        self.composer.take_recent_submission_images()
    }

    pub(crate) fn take_recent_submission_images_with_placeholders(
        &mut self,
    ) -> Vec<LocalImageAttachment> {
        self.composer
            .take_recent_submission_images_with_placeholders()
    }

    pub(crate) fn prepare_inline_args_submission(
        &mut self,
        record_history: bool,
    ) -> Option<(String, Vec<TextElement>)> {
        self.composer.prepare_inline_args_submission(record_history)
    }

    fn as_renderable(&'_ self) -> RenderableItem<'_> {
        self.as_renderable_with_options(ComposerRenderOptions::default())
    }

    pub(crate) fn as_renderable_with_options<'a>(
        &'a self,
        options: ComposerRenderOptions<'a>,
    ) -> RenderableItem<'a> {
        self.renderable_for_views(options, &self.view_stack)
    }

    pub(crate) fn centered_dialog(&self) -> Option<CenteredView<'_>> {
        self.active_view()
            .filter(|view| view.presentation() == ViewPresentation::Centered)
            // 008-TRIM: warnings_active 过滤（008 §1 裁剪：无本地事件源）
            .map(CenteredView)
    }

    pub(crate) fn backdrop_with_options<'a>(
        &'a self,
        options: ComposerRenderOptions<'a>,
    ) -> RenderableItem<'a> {
        let views = if self.centered_dialog().is_some() {
            &self.view_stack[..self.view_stack.len() - 1]
        } else {
            &self.view_stack
        };
        self.renderable_for_views(options, views)
    }

    fn renderable_for_views<'a>(
        &'a self,
        mut options: ComposerRenderOptions<'a>,
        views: &'a [Box<dyn BottomPaneView>],
    ) -> RenderableItem<'a> {
        // 008-TRIM: warnings_view 渲染分支与 inline_banner 可见性重置（008 §1 裁剪：
        // 无本地事件源）
        if let Some(view) = views.last() {
            if view.presentation() == ViewPresentation::Centered {
                RenderableItem::Owned(Box::new(view_stack::ViewStack(views)))
            } else {
                RenderableItem::Borrowed(view.as_ref())
            }
        } else {
            let mut flex = FlexRenderable::new();
            // 008-TRIM: inline_banner / hook_status_message / unified_exec_footer /
            // pending_thread_approvals / questions 渲染行（008 §1 裁剪：无本地事件源）；
            // 保留 status 行、pending input 预览与分隔空行。
            if let Some(status) = self.status_widget() {
                flex.push(
                    /*flex*/ 0,
                    RenderableItem::Owned(Box::new(status.with_timer(&self.status_timer))),
                );
            }
            let has_pending_input = !self.pending_input_preview.queued_messages.is_empty()
                || !self.pending_input_preview.pending_steers.is_empty()
                || !self.pending_input_preview.rejected_steers.is_empty();
            let has_status_or_footer = self.status_widget().is_some();
            if !has_pending_input
                && self.status_widget().is_some()
                && let Some(tip) = options.working_tip
            {
                flex.push(/*flex*/ 1, RenderableItem::Borrowed(tip));
            }
            if has_pending_input && has_status_or_footer {
                flex.push(/*flex*/ 0, RenderableItem::Owned("".into()));
            }
            flex.push(
                /*flex*/ 1,
                RenderableItem::Borrowed(&self.pending_input_preview),
            );
            // An empty shared gap already separates activity from the composer.
            if !has_pending_input
                && has_status_or_footer
                && options.composer_gap.is_none_or(|gap| gap.needs_separator)
            {
                flex.push(/*flex*/ 0, RenderableItem::Owned("".into()));
            }
            // History overlays must not clear the status and previews above this composer.
            if options.command_popup_placement == CommandPopupPlacement::Overlay
                && (has_status_or_footer || has_pending_input)
            {
                options.command_popup_placement = CommandPopupPlacement::AboveComposer;
            }
            let mut flex2 = FlexRenderable::new();
            // Clip spacing and hints before activity and previews when the composer is tall.
            let above_composer = if let Some(gap) = options.composer_gap {
                let mut column = FlexRenderable::new();
                if has_status_or_footer {
                    column.push(/*flex*/ 1, RenderableItem::Owned("".into()));
                }
                column.push(/*flex*/ 0, RenderableItem::Owned(flex.into()));
                column.push(/*flex*/ 1, RenderableItem::Borrowed(gap));
                column.into()
            } else {
                flex.into()
            };
            flex2.push(/*flex*/ 1, RenderableItem::Owned(above_composer));
            // 008-TRIM: questions 展开编辑器渲染分支（008 §1 裁剪：无本地事件源）
            let composer: RenderableItem<'_> = if options.max_height.is_none()
                && options.textarea_right_reserve == 0
                && options.warning_count == 0
                && options.footer.is_none()
                && !options.separate_status_line
                && options.command_popup_placement == CommandPopupPlacement::AboveComposer
            {
                RenderableItem::Borrowed(&self.composer)
            } else {
                RenderableItem::Owned(Box::new(ChatComposerPresentation {
                    composer: &self.composer,
                    options: self.composer.resolve_render_options(options),
                }))
            };
            flex2.push(/*flex*/ 0, composer);
            RenderableItem::Owned(Box::new(flex2))
        }
    }

    pub(crate) fn show_footer_flash(&mut self, line: Line<'static>, duration: Duration) {
        // 008-TRIM: warnings_view 闪现同步（008 §1 裁剪：无本地事件源）
        self.composer.show_footer_flash(line, duration);
        self.request_redraw();
    }

    pub(crate) fn set_status_line(&mut self, status_line: Option<Line<'static>>) {
        if self.composer.set_status_line(status_line) {
            self.request_redraw();
        }
    }

    pub(crate) fn set_luna_reserve_active(&mut self, active: bool) {
        if self.composer.set_luna_reserve_active(active) {
            self.request_redraw();
        }
    }

    pub(crate) fn set_status_line_hyperlink(&mut self, url: Option<String>) {
        if self.composer.set_status_line_hyperlink(url) {
            self.request_redraw();
        }
    }

    pub(crate) fn set_status_line_enabled(&mut self, enabled: bool) {
        if self.composer.set_status_line_enabled(enabled) {
            self.request_redraw();
        }
    }

    /// Updates the contextual footer label and requests a redraw only when it changed.
    ///
    /// This keeps the footer plumbing cheap during thread transitions where `App` may recompute
    /// the label several times while the visible thread settles.
    pub(crate) fn set_active_agent_label(&mut self, active_agent_label: Option<String>) {
        if self.composer.set_active_agent_label(active_agent_label) {
            self.request_redraw();
        }
    }

    pub(crate) fn set_side_conversation_context_label(&mut self, label: Option<String>) {
        if self.composer.set_side_conversation_context_label(label) {
            self.request_redraw();
        }
    }
}

struct ChatComposerPresentation<'a> {
    composer: &'a chat_composer::ChatComposer,
    options: ComposerRenderOptions<'a>,
}

impl Renderable for ChatComposerPresentation<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.composer
            .render_with_options(area, buf, /*mask_char*/ None, self.options);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.composer
            .desired_height_with_options(width, self.options)
            .min(self.options.max_height.unwrap_or(u16::MAX))
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.composer.cursor_pos_with_options(area, self.options)
    }

    fn cursor_style(&self, area: Rect) -> crossterm::cursor::SetCursorStyle {
        if self
            .options
            .footer
            .is_some_and(|footer| footer.is_interactive)
        {
            crossterm::cursor::SetCursorStyle::SteadyBar
        } else {
            self.composer.cursor_style(area)
        }
    }
}

impl Renderable for BottomPane {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.as_renderable().render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        self.as_renderable().desired_height(width)
    }
    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.as_renderable().cursor_pos(area)
    }

    fn cursor_style(&self, area: Rect) -> crossterm::cursor::SetCursorStyle {
        self.as_renderable().cursor_style(area)
    }
}

#[cfg(test)]
mod tests {
    // 008-TRIM: #[path = "actionable_banner_tests.rs"] mod actionable_banner_tests
    // （actionable banner，008 §1 裁剪：无本地事件源）
    #[path = "picker_hint_tests.rs"]
    mod picker_hint_tests;

    use super::*;
// 008-TRIM: use crate::app::app_server_requests::ResolvedAppServerRequest;（裁剪功能/模块，008 §1）
    // 008-TRIM 适配: use crate::app_command::AppCommand as Op（本地无 app_command；
    // 保留测试的 Interrupt 断言改用本地 AppEvent::Interrupt 变体，行为语义不变）
    use crate::app_event::AppEvent;
    use crate::status_indicator_widget::STATUS_DETAILS_DEFAULT_MAX_LINES;
    use crate::status_indicator_widget::StatusDetailsCapitalization;
    // 008-TRIM: use crate::test_support::{PathBufExt, test_path_buf} 与
    // codex_app_server_protocol::CommandExecutionApprovalDecision（被裁测试专用）
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyModifiers;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Instant;
    use tokio::sync::mpsc::unbounded_channel;

    fn snapshot_buffer(buf: &Buffer) -> String {
        let mut lines = Vec::new();
        for y in 0..buf.area().height {
            let mut row = String::new();
            for x in 0..buf.area().width {
                row.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            lines.push(row);
        }
        lines.join("\n")
    }

    fn render_snapshot(pane: &impl Renderable, area: Rect) -> String {
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);
        snapshot_buffer(&buf)
    }

    pub(super) fn test_pane(app_event_tx: AppEventSender) -> BottomPane {
        test_pane_with_disable_paste_burst(app_event_tx, /*disable_paste_burst*/ false)
    }

    /// 020 §2.3/A10（审查补充）：拒绝恢复的 D0＋Dnew 合并——已有新草稿时
    /// D0 在前、Dnew 换行接续，附件合并后整体重编号。
    #[test]
    fn restore_rejected_submission_merges_d0_then_dnew_and_renumbers() {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        let mut pane = test_pane(crate::app_event_sender::AppEventSender::new(tx));
        let img_dir = tempfile::TempDir::new().unwrap();
        let mk = |name: &str| img_dir.path().join(name);
        // Dnew：用户在准备期间新写的草稿（含一个新附件）
        pane.attach_image(mk("dnew.png"));
        pane.composer.handle_paste("新草稿".into());
        // 恢复被拒的 D0（文本＋两个附件路径）
        pane.restore_rejected_submission(
            "[Image #1] D0 文本".to_string(),
            vec![mk("d0a.png"), mk("d0b.png")],
        );
        let text = pane.composer_text();
        assert!(
            text.contains("D0 文本") && text.contains("新草稿"),
            "D0＋Dnew 都在场：{text:?}"
        );
        let d0_pos = text.find("D0 文本").expect("D0");
        let dnew_pos = text.find("新草稿").expect("Dnew");
        assert!(d0_pos < dnew_pos, "合并顺序 D0 前 Dnew 后：{text:?}");
        // 附件合并后重编号：D0 两张在前（#1/#2），Dnew 一张成 #3
        let paths = pane.composer_local_image_paths();
        assert_eq!(paths.len(), 3, "三附件合并：{paths:?}");
        assert_eq!(paths[0], mk("d0a.png"));
        assert_eq!(paths[1], mk("d0b.png"));
        assert_eq!(paths[2], mk("dnew.png"));
        assert!(text.contains("[Image #1]") && text.contains("[Image #3]"), "重编号：{text:?}");
    }

    fn test_pane_with_disable_paste_burst(
        app_event_tx: AppEventSender,
        disable_paste_burst: bool,
    ) -> BottomPane {
        BottomPane::new(BottomPaneParams {
            app_event_tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        })
    }

    // 008-TRIM: 删除覆盖被裁功能的测试（审批/请求用户输入/inline banner/
    // actionable banner/skill·command 弹窗/custom prompt/unified exec，
    // 008 §1 裁剪：无本地事件源）；Esc 打断路由、状态指示器、选择视图、
    // 粘贴与历史搜索等保留行为测试未动。

    #[derive(Default)]
    struct DismissibleView {
        id: Option<&'static str>,
        // 008-TRIM: dismiss_exec_id 字段（仅被裁剪的审批撤除测试使用）
        complete: bool,
    }

    impl Renderable for DismissibleView {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}

        fn desired_height(&self, _width: u16) -> u16 {
            0
        }
    }

    impl BottomPaneView for DismissibleView {
        fn is_complete(&self) -> bool {
            self.complete
        }

        fn view_id(&self) -> Option<&'static str> {
            self.id
        }
        // 008-TRIM: fn dismiss_app_server_request 覆写（app-server 请求撤除，
        // 008 §1 裁剪：无本地事件源）
    }

    #[test]
    fn view_transition_key_guard_survives_an_intervening_modal() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let mut pane = test_pane(AppEventSender::new(tx_raw));
        let activation_key = crate::key_hint::plain(KeyCode::Enter);
        pane.suppress_current_view_key_until_release(activation_key, Instant::now());
        pane.push_view(Box::new(DismissibleView::default()));

        assert!(pane.consume_view_transition_key(activation_key, KeyEventKind::Repeat));
        assert!(pane.consume_view_transition_key(activation_key, KeyEventKind::Release));
        assert!(!pane.consume_view_transition_key(activation_key, KeyEventKind::Press));
    }

    #[test]
    fn transition_events_reach_a_view_covered_by_a_modal() {
        #[derive(Default)]
        struct CoveredEvents {
            press_seen: Cell<bool>,
            release_seen: Cell<bool>,
            // 008-TRIM: interrupted（config 编辑器打断，008 §1 裁剪：无本地事件源）
        }

        struct TrackingView(Rc<CoveredEvents>);

        impl Renderable for TrackingView {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}

            fn desired_height(&self, _width: u16) -> u16 {
                0
            }
        }

        impl BottomPaneView for TrackingView {
            fn observe_key_event_while_covered(
                &mut self,
                _key_event: KeyEvent,
                _activation_key: KeyBinding,
            ) {
                self.0.press_seen.set(true);
            }

            fn handle_key_release(&mut self, _key_event: KeyEvent) {
                self.0.release_seen.set(true);
            }
            // 008-TRIM: fn interrupt_config_editor_transition 覆写（008 §1 裁剪）
        }

        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let mut pane = test_pane(AppEventSender::new(tx_raw));
        let events = Rc::new(CoveredEvents::default());
        pane.push_view(Box::new(TrackingView(events.clone())));
        pane.push_view(Box::new(DismissibleView::default()));

        pane.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        pane.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        pane.handle_paste("pasted".to_string());

        assert!(events.press_seen.get());
        assert!(events.release_seen.get());
    }

    #[derive(Default)]
    struct CompletingView {
        id: Option<&'static str>,
        complete: bool,
    }

    impl Renderable for CompletingView {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}

        fn desired_height(&self, _width: u16) -> u16 {
            0
        }
    }

    impl BottomPaneView for CompletingView {
        fn handle_key_event(&mut self, key_event: KeyEvent) {
            if key_event.code == KeyCode::Enter {
                self.complete = true;
            }
        }

        fn is_complete(&self) -> bool {
            self.complete
        }

        fn view_id(&self) -> Option<&'static str> {
            self.id
        }
    }
    #[test]
    fn ctrl_c_cancels_history_search_without_clearing_draft_or_showing_quit_hint() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: true,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });
        pane.insert_str("draft");

        pane.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(pane.composer.popup_active());

        assert_eq!(CancellationEvent::Handled, pane.on_ctrl_c());
        assert_eq!(pane.composer_text(), "draft");
        assert!(!pane.composer.popup_active());
        assert!(!pane.quit_shortcut_hint_visible());
        pane.composer.set_vim_enabled(/*enabled*/ true);
        pane.handle_key_event(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(CancellationEvent::Handled, pane.on_ctrl_c());
        assert_eq!(pane.composer_text(), "draft");
        assert!(!pane.composer.popup_active());
    }

    #[test]
    fn completing_top_view_preserves_underlying_view() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let mut pane = test_pane(AppEventSender::new(tx_raw));

        pane.push_view(Box::new(DismissibleView {
            id: Some("underlying"),
            complete: false,
        }));
        pane.push_view(Box::new(CompletingView {
            id: Some("top"),
            complete: false,
        }));

        pane.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(pane.view_stack.len(), 1);
        assert_eq!(
            pane.view_stack.last().and_then(|view| view.view_id()),
            Some("underlying")
        );
    }
    #[test]
    fn status_indicator_visible_during_command_execution() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        // Begin a task: show initial status.
        pane.set_task_running(/*running*/ true);

        // Use a height that allows the status line to be visible above the composer.
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);

        let bufs = snapshot_buffer(&buf);
        assert!(bufs.contains("• Working"), "expected Working header");

        pane.reset_status_timer(Duration::from_secs(/*secs*/ 42));
        pane.hide_status_indicator();
        pane.pause_status_timer_for_modal();
        let paused = pane.status_timer.elapsed_at(Instant::now());
        pane.ensure_status_indicator();
        assert_snapshot!(
            "status_timer_survives_hidden_row",
            render_snapshot(&pane, area)
        );
        assert_eq!(
            pane.status_timer
                .elapsed_at(Instant::now() + Duration::from_secs(/*secs*/ 10)),
            paused
        );
        pane.resume_status_timer_after_modal();
        assert!(
            pane.status_timer
                .elapsed_at(Instant::now() + Duration::from_secs(/*secs*/ 10))
                >= paused + Duration::from_secs(/*secs*/ 10)
        );
        pane.set_task_running(/*running*/ false);
        pane.set_task_running(/*running*/ true);
        assert!(pane.status_timer.elapsed_at(Instant::now()) < paused);
    }
    #[test]
    fn status_and_composer_fill_height_without_bottom_padding() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        // Activate spinner (status view replaces composer) with no live ring.
        pane.set_task_running(/*running*/ true);

        // Use height == desired_height; expect spacer + status + composer rows without trailing padding.
        let height = pane.desired_height(/*width*/ 30);
        assert!(
            height >= 3,
            "expected at least 3 rows to render spacer, status, and composer; got {height}"
        );
        let area = Rect::new(0, 0, 30, height);
        assert_snapshot!(
            "status_and_composer_fill_height_without_bottom_padding",
            render_snapshot(&pane, area)
        );
    }

    #[test]
    fn status_only_snapshot() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_task_running(/*running*/ true);

        let width = 48;
        let height = pane.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        assert_snapshot!("status_only_snapshot", render_snapshot(&pane, area));
    }
    #[test]
    fn status_with_details_and_queued_messages_snapshot() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_task_running(/*running*/ true);
        pane.update_status(
            "Working".to_string(),
            Some("First detail line\nSecond detail line".to_string()),
            StatusDetailsCapitalization::CapitalizeFirst,
            STATUS_DETAILS_DEFAULT_MAX_LINES,
        );
        pane.set_pending_input_preview(
            vec!["Queued follow-up question".to_string()],
            Vec::new(),
            Vec::new(),
        );

        let width = 48;
        let height = pane.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        assert_snapshot!(
            "status_with_details_and_queued_messages_snapshot",
            render_snapshot(&pane, area)
        );
    }

    #[test]
    fn queued_messages_visible_when_status_hidden_snapshot() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_task_running(/*running*/ true);
        pane.set_pending_input_preview(
            vec!["Queued follow-up question".to_string()],
            Vec::new(),
            Vec::new(),
        );
        pane.hide_status_indicator();

        let width = 48;
        let height = pane.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        assert_snapshot!(
            "queued_messages_visible_when_status_hidden_snapshot",
            render_snapshot(&pane, area)
        );
    }

    #[test]
    fn status_and_queued_messages_snapshot() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_task_running(/*running*/ true);
        pane.set_pending_input_preview(
            vec!["Queued follow-up question".to_string()],
            Vec::new(),
            Vec::new(),
        );

        let width = 48;
        let height = pane.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        assert_snapshot!(
            "status_and_queued_messages_snapshot",
            render_snapshot(&pane, area)
        );
    }

    #[test]
    fn remote_images_render_above_composer_text() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_remote_image_urls(vec![
            "https://example.com/one.png".to_string(),
            "data:image/png;base64,aGVsbG8=".to_string(),
        ]);

        assert_eq!(pane.composer_text(), "");
        let width = 48;
        let height = pane.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let snapshot = render_snapshot(&pane, area);
        assert!(snapshot.contains("[Image #1]"));
        assert!(snapshot.contains("[Image #2]"));
    }

    #[test]
    fn drain_pending_submission_state_clears_remote_image_urls() {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_remote_image_urls(vec!["https://example.com/one.png".to_string()]);
        assert_eq!(pane.remote_image_urls().len(), 1);

        pane.drain_pending_submission_state();

        assert!(pane.remote_image_urls().is_empty());
    }
    fn esc_with_agent_command_without_popup_does_not_interrupt_task() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_task_running(/*running*/ true);

        // Repro: `/subagents ` hides the popup (cursor past command name). Esc should
        // keep editing command text instead of interrupting the running task.
        pane.insert_str("/subagents ");
        assert!(
            !pane.composer.popup_active(),
            "expected command popup to be hidden after entering `/subagents `"
        );

        pane.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        while let Ok(ev) = rx.try_recv() {
            assert!(
                !matches!(ev, AppEvent::Interrupt),
                "expected Esc to not send Op::Interrupt while typing `/subagents`"
            );
        }
        assert_eq!(pane.composer_text(), "/subagents ");
    }

    #[test]
    fn esc_release_after_dismissing_agent_picker_does_not_interrupt_task() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_task_running(/*running*/ true);
        pane.show_selection_view(SelectionViewParams {
            title: Some("Agents".to_string()),
            items: vec![SelectionItem {
                name: "Main".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });

        pane.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        ));
        pane.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));

        while let Ok(ev) = rx.try_recv() {
            assert!(
                !matches!(ev, AppEvent::Interrupt),
                "expected Esc release after dismissing agent picker to not interrupt"
            );
        }
        assert!(
            pane.no_modal_or_popup_active(),
            "expected Esc press to dismiss the agent picker"
        );
    }

    #[test]
    fn esc_interrupts_running_task_when_no_popup() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_task_running(/*running*/ true);

        pane.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(
            matches!(rx.try_recv(), Ok(AppEvent::Interrupt)),
            "expected Esc to send Op::Interrupt while a task is running"
        );

        pane.hide_status_indicator();
        pane.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::Interrupt)
        ));
    }

    #[test]
    fn remapped_interrupt_turn_uses_configured_key_including_agent_drafts() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = test_pane(tx);
        let mut keymap = RuntimeKeymap::defaults();
        keymap.chat.interrupt_turn = vec![crate::key_hint::plain(KeyCode::F(12))];
        pane.set_keymap_bindings(&keymap);
        pane.set_task_running(/*running*/ true);
        pane.insert_str("/subagents ");

        pane.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(
            rx.try_recv().is_err(),
            "expected Esc to remain local after remapping interruption"
        );

        pane.handle_key_event(KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE));
        assert!(
            matches!(rx.try_recv(), Ok(AppEvent::Interrupt)),
            "expected configured key to interrupt while `/subagents` is being edited"
        );

        pane.hide_status_indicator();
        pane.handle_key_event(KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE));
        assert!(matches!(
            rx.try_recv(),
            Ok(AppEvent::Interrupt)
        ));
    }

    #[test]
    fn selection_view_esc_respects_remapped_list_cancel() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = test_pane(tx);
        let mut keymap = RuntimeKeymap::defaults();
        keymap.list.cancel = vec![crate::key_hint::plain(KeyCode::Char('q'))];
        pane.set_keymap_bindings(&keymap);
        pane.show_selection_view(SelectionViewParams {
            title: Some("Agents".to_string()),
            items: vec![SelectionItem {
                name: "Main".to_string(),
                ..Default::default()
            }],
            on_cancel: Some(Box::new(|tx: &_| {
                tx.send(AppEvent::Interrupt); // 008-TRIM 适配: 本地无 OpenApprovalsPopup，改发可观察的本地事件
            })),
            ..Default::default()
        });

        pane.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(pane.active_view().is_some());
        assert!(rx.try_recv().is_err());

        pane.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        assert!(pane.no_modal_or_popup_active());
        assert!(matches!(rx.try_recv(), Ok(AppEvent::Interrupt))); // 008-TRIM 适配: 同上
    }

    #[test]
    fn esc_routes_to_handle_key_event_when_requested() {
        #[derive(Default)]
        struct EscRoutingView {
            on_ctrl_c_calls: Rc<Cell<usize>>,
            handle_calls: Rc<Cell<usize>>,
        }

        impl Renderable for EscRoutingView {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}

            fn desired_height(&self, _width: u16) -> u16 {
                0
            }
        }

        impl BottomPaneView for EscRoutingView {
            fn handle_key_event(&mut self, _key_event: KeyEvent) {
                self.handle_calls
                    .set(self.handle_calls.get().saturating_add(1));
            }

            fn on_ctrl_c(&mut self) -> CancellationEvent {
                self.on_ctrl_c_calls
                    .set(self.on_ctrl_c_calls.get().saturating_add(1));
                CancellationEvent::Handled
            }

            fn prefer_esc_to_handle_key_event(&self) -> bool {
                true
            }
        }

        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        let on_ctrl_c_calls = Rc::new(Cell::new(0));
        let handle_calls = Rc::new(Cell::new(0));
        pane.push_view(Box::new(EscRoutingView {
            on_ctrl_c_calls: Rc::clone(&on_ctrl_c_calls),
            handle_calls: Rc::clone(&handle_calls),
        }));

        pane.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(on_ctrl_c_calls.get(), 0);
        assert_eq!(handle_calls.get(), 1);
    }

    #[test]
    fn release_events_are_ignored_for_active_view() {
        #[derive(Default)]
        struct CountingView {
            handle_calls: Rc<Cell<usize>>,
        }

        impl Renderable for CountingView {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}

            fn desired_height(&self, _width: u16) -> u16 {
                0
            }
        }

        impl BottomPaneView for CountingView {
            fn handle_key_event(&mut self, _key_event: KeyEvent) {
                self.handle_calls
                    .set(self.handle_calls.get().saturating_add(1));
            }
        }

        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        let handle_calls = Rc::new(Cell::new(0));
        pane.push_view(Box::new(CountingView {
            handle_calls: Rc::clone(&handle_calls),
        }));

        pane.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Down,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        ));
        pane.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Down,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));

        assert_eq!(handle_calls.get(), 1);
    }

    #[test]
    fn paste_completion_clears_stacked_views_and_restores_composer_input() {
        #[derive(Default)]
        struct BlockingView {
            handle_calls: Rc<Cell<usize>>,
        }

        impl Renderable for BlockingView {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}

            fn desired_height(&self, _width: u16) -> u16 {
                0
            }
        }

        impl BottomPaneView for BlockingView {
            fn handle_key_event(&mut self, _key_event: KeyEvent) {
                self.handle_calls
                    .set(self.handle_calls.get().saturating_add(1));
            }
        }

        #[derive(Default)]
        struct PasteCompletesView {
            complete: bool,
        }

        impl Renderable for PasteCompletesView {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}

            fn desired_height(&self, _width: u16) -> u16 {
                0
            }
        }

        impl BottomPaneView for PasteCompletesView {
            fn handle_paste(&mut self, _pasted: String) -> bool {
                self.complete = true;
                true
            }

            fn is_complete(&self) -> bool {
                self.complete
            }
        }

        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);
        let mut pane = BottomPane::new(BottomPaneParams {
            app_event_tx: tx,
            frame_requester: FrameRequester::test_dummy(),
            has_input_focus: true,
            enhanced_keys_supported: false,
            placeholder_text: "Ask Codex to do anything".to_string(),
            disable_paste_burst: false,
            animations_enabled: true,
            effects: Default::default(),
            // 008-TRIM: skills 参数（008 §1 裁剪：无本地事件源）
        });

        pane.set_composer_input_enabled(/*enabled*/ false, /*placeholder*/ None);

        let lower_view_handle_calls = Rc::new(Cell::new(0));
        pane.push_view(Box::new(BlockingView {
            handle_calls: Rc::clone(&lower_view_handle_calls),
        }));
        pane.push_view(Box::new(PasteCompletesView::default()));

        pane.handle_paste("hello".to_string());

        assert!(
            pane.view_stack.is_empty(),
            "paste completion should tear down the active modal flow"
        );

        pane.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        let area = Rect::new(0, 0, 40, pane.desired_height(/*width*/ 40).max(2));
        assert!(pane.cursor_pos(area).is_some());
        assert_eq!(lower_view_handle_calls.get(), 0);
    }
}
