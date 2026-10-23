//! 018-ADAPT: Codex d138398 同步问答子集；本地独立回送通道，排除自动空答、队列、App Server 和图片。
//! Request-user-input overlay state machine.
//!
//! Core behaviors:
//! - Each question can be answered by selecting one option and/or providing notes.
//! - Notes are stored per question and appended as extra answers.
//! - Typing while focused on options jumps into notes to keep freeform input fast.
//! - The composer submit binding advances to the next question; the last question submits all answers.
//! - Freeform-only questions submit an empty answer list when empty.
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::style::Stylize;
use ratatui::text::Line;
mod layout;
pub(super) mod render;

use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::ChatComposer;
use crate::bottom_pane::ChatComposerConfig;
use crate::bottom_pane::InputResult;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::bottom_pane::scroll_state::ScrollState;
use crate::bottom_pane::selection_popup_common::GenericDisplayRow;
use crate::bottom_pane::selection_popup_common::measure_rows_height;
use crate::footer_hint::shortcut;
use crate::key_hint::KeyBinding;
use crate::key_hint::KeyBindingListExt;
use crate::key_hint::ShortcutHint;
use crate::keymap::KeymapContext;
use crate::keymap::ListAction;
use crate::keymap::ListKeymap;
use crate::keymap::RuntimeKeymap;
use crate::render::renderable::Renderable;

use crate::chat_driver::UserInputReply;
use codex_protocol::request_user_input::{
    RequestUserInputAnswer as ToolRequestUserInputAnswer, RequestUserInputError,
    RequestUserInputQuestion as ToolRequestUserInputQuestion,
    RequestUserInputResponse as ToolRequestUserInputResponse,
};
use codex_protocol::user_input::TextElement;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use unicode_width::UnicodeWidthStr;

const NOTES_PLACEHOLDER: &str = "Add notes";
const ANSWER_PLACEHOLDER: &str = "Type your answer (optional)";
// Keep in sync with ChatComposer's minimum composer height.
const MIN_COMPOSER_HEIGHT: u16 = 3;
const SELECT_OPTION_PLACEHOLDER: &str = "Select an option to add notes";
pub(super) const TIP_SEPARATOR: &str = " | ";
pub(super) const DESIRED_SPACERS_BETWEEN_SECTIONS: u16 = 2;
const OTHER_OPTION_LABEL: &str = "None of the above";
const OTHER_OPTION_DESCRIPTION: &str = "Optionally, add details in notes (tab)";
const UNANSWERED_CONFIRM_TITLE: &str = "Submit with unanswered questions?";
const UNANSWERED_CONFIRM_GO_BACK: &str = "Go back";
const UNANSWERED_CONFIRM_GO_BACK_DESC: &str = "Return to the first unanswered question";
const UNANSWERED_CONFIRM_SUBMIT: &str = "Proceed";
const UNANSWERED_CONFIRM_SUBMIT_DESC_SINGULAR: &str = "question";
const UNANSWERED_CONFIRM_SUBMIT_DESC_PLURAL: &str = "questions";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Options,
    Notes,
}

#[derive(Default, Clone, PartialEq)]
struct ComposerDraft {
    text: String,
    text_elements: Vec<TextElement>,
    pending_pastes: Vec<(String, String)>,
}

impl ComposerDraft {
    fn text_with_pending(&self) -> String {
        if self.pending_pastes.is_empty() {
            return self.text.clone();
        }
        debug_assert!(
            !self.text_elements.is_empty(),
            "pending pastes should always have matching text elements"
        );
        let (expanded, _) = ChatComposer::expand_pending_pastes(
            &self.text,
            self.text_elements.clone(),
            &self.pending_pastes,
        );
        expanded
    }
}

struct AnswerState {
    // Scrollable cursor state for option navigation/highlight.
    options_state: ScrollState,
    // Per-question notes draft.
    draft: ComposerDraft,
    // Whether the answer for this question has been explicitly submitted.
    answer_committed: bool,
    // Whether the notes UI has been explicitly opened for this question.
    notes_visible: bool,
}

pub(crate) struct RequestUserInputOverlay {
    questions: Vec<ToolRequestUserInputQuestion>,
    reply: Option<UserInputReply>,
    live: Arc<AtomicBool>,
    display_filter: fn(&str) -> String,
    // Reuse the shared chat composer so notes/freeform answers match the
    // primary input styling and behavior.
    composer: ChatComposer,
    // One entry per question: selection state plus a stored notes draft.
    answers: Vec<AnswerState>,
    current_idx: usize,
    focus: Focus,
    done: bool,
    pending_submission_draft: Option<ComposerDraft>,
    confirm_unanswered: Option<ScrollState>,
    composer_submit_keys: Vec<KeyBinding>,
    composer_submit_hint: Option<ShortcutHint>,
    interrupt_turn_keys: Vec<KeyBinding>,
    interrupt_turn_hint: Option<ShortcutHint>,
    list_keymap: ListKeymap,
}

impl RequestUserInputOverlay {
    pub(crate) fn new_with_keymap(
        questions: Vec<ToolRequestUserInputQuestion>,
        mut reply: UserInputReply,
        live: Arc<AtomicBool>,
        display_filter: fn(&str) -> String,
        app_event_tx: AppEventSender,
        disable_paste_burst: bool,
        keymap: RuntimeKeymap,
    ) -> Self {
        // Use the same composer widget, but disable popups/slash-commands and
        // image-path attachment so it behaves like a focused notes field.
        reply.displayed();
        let mut composer = ChatComposer::new_with_config(
            true,
            app_event_tx.clone(),
            true,
            ANSWER_PLACEHOLDER.to_string(),
            disable_paste_burst,
            ChatComposerConfig::plain_text(),
        );
        composer.set_keymap_bindings(&keymap);
        // The overlay renders its own footer hints, so keep the composer footer empty.
        composer.set_footer_hint_override(Some(Vec::new()));
        let mut overlay = Self {
            questions,
            reply: Some(reply),
            live,
            display_filter,
            composer,
            answers: Vec::new(),
            current_idx: 0,
            focus: Focus::Options,
            done: false,
            pending_submission_draft: None,
            confirm_unanswered: None,
            composer_submit_keys: keymap.composer.submit.clone(),
            composer_submit_hint: keymap.primary_hint(KeymapContext::Composer, "submit"),
            interrupt_turn_keys: keymap.chat.interrupt_turn.clone(),
            interrupt_turn_hint: keymap.primary_hint(KeymapContext::Chat, "interrupt_turn"),
            list_keymap: keymap.list,
        };
        overlay.reset_for_request();
        overlay.ensure_focus_available();
        overlay.restore_current_draft();
        overlay
    }

    fn current_index(&self) -> usize {
        self.current_idx
    }

    fn current_question(&self) -> Option<&ToolRequestUserInputQuestion> {
        self.questions.get(self.current_index())
    }

    fn current_answer_mut(&mut self) -> Option<&mut AnswerState> {
        let idx = self.current_index();
        self.answers.get_mut(idx)
    }

    fn current_answer(&self) -> Option<&AnswerState> {
        let idx = self.current_index();
        self.answers.get(idx)
    }

    fn question_count(&self) -> usize {
        self.questions.len()
    }

    pub(super) fn progress_prefix_text(&self) -> String {
        if self.question_count() > 0 {
            let idx = self.current_index() + 1;
            let total = self.question_count();
            let base = format!("Question {idx}/{total}");
            let unanswered = self.unanswered_count();
            if unanswered > 0 {
                format!("{base} ({unanswered} unanswered)")
            } else {
                base
            }
        } else {
            "No questions".to_string()
        }
    }

    fn has_options(&self) -> bool {
        self.current_question()
            .and_then(|question| question.options.as_ref())
            .is_some_and(|options| !options.is_empty())
    }

    fn options_len(&self) -> usize {
        self.current_question()
            .map(Self::options_len_for_question)
            .unwrap_or(0)
    }

    fn option_index_for_digit(&self, ch: char) -> Option<usize> {
        if !self.has_options() {
            return None;
        }
        let digit = ch.to_digit(10)?;
        if digit == 0 {
            return None;
        }
        let idx = (digit - 1) as usize;
        (idx < self.options_len()).then_some(idx)
    }

    fn selected_option_index(&self) -> Option<usize> {
        if !self.has_options() {
            return None;
        }
        self.current_answer()
            .and_then(|answer| answer.options_state.selected_idx)
    }

    fn notes_has_content(&self, idx: usize) -> bool {
        if idx == self.current_index() {
            !self.composer.current_text_with_pending().trim().is_empty()
        } else {
            !self.answers[idx].draft.text.trim().is_empty()
        }
    }

    pub(super) fn notes_ui_visible(&self) -> bool {
        if !self.has_options() {
            return true;
        }
        let idx = self.current_index();
        self.current_answer()
            .is_some_and(|answer| answer.notes_visible || self.notes_has_content(idx))
    }

    pub(super) fn wrapped_question_lines(
        &self,
        width: u16,
    ) -> Vec<crate::terminal_hyperlinks::HyperlinkLine> {
        self.current_question()
            .map(|q| {
                (self.display_filter)(&q.question)
                    .split('\n')
                    .flat_map(|text| {
                        let source = crate::terminal_hyperlinks::annotate_web_urls_in_line(
                            text.to_owned().into(),
                        );
                        let wrapped =
                            crate::wrapping::wrap_ranges_trim(text, usize::from(width.max(1)))
                                .into_iter()
                                .map(|range| crate::wrapping::WrappedLine {
                                    line: (&text[range.clone()]).into(),
                                    range,
                                    prefix_bytes: 0,
                                })
                                .collect();
                        crate::terminal_hyperlinks::remap_source_wrapped_line(&source, wrapped)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn focus_is_notes(&self) -> bool {
        matches!(self.focus, Focus::Notes)
    }

    fn confirm_unanswered_active(&self) -> bool {
        self.confirm_unanswered.is_some()
    }

    pub(super) fn option_rows(&self) -> Vec<GenericDisplayRow> {
        self.current_question()
            .and_then(|question| question.options.as_ref().map(|options| (question, options)))
            .map(|(question, options)| {
                let selected_idx = self
                    .current_answer()
                    .and_then(|answer| answer.options_state.selected_idx);
                let mut rows = options
                    .iter()
                    .enumerate()
                    .map(|(idx, opt)| {
                        let selected = selected_idx.is_some_and(|sel| sel == idx);
                        let prefix = if selected { '›' } else { ' ' };
                        let label = (self.display_filter)(&opt.label);
                        let number = idx + 1;
                        let prefix_label = format!("{prefix} {number}. ");
                        let wrap_indent = UnicodeWidthStr::width(prefix_label.as_str());
                        GenericDisplayRow {
                            selection_style: Some(crate::bottom_pane::selection_style()),
                            name: format!("{prefix_label}{label}"),
                            description: Some((self.display_filter)(&opt.description)),
                            wrap_indent: Some(wrap_indent),
                            ..Default::default()
                        }
                    })
                    .collect::<Vec<_>>();

                if Self::other_option_enabled_for_question(question) {
                    let idx = options.len();
                    let selected = selected_idx.is_some_and(|sel| sel == idx);
                    let prefix = if selected { '›' } else { ' ' };
                    let number = idx + 1;
                    let prefix_label = format!("{prefix} {number}. ");
                    let wrap_indent = UnicodeWidthStr::width(prefix_label.as_str());
                    rows.push(GenericDisplayRow {
                        selection_style: Some(crate::bottom_pane::selection_style()),
                        name: format!("{prefix_label}{OTHER_OPTION_LABEL}"),
                        description: Some(OTHER_OPTION_DESCRIPTION.to_string()),
                        wrap_indent: Some(wrap_indent),
                        ..Default::default()
                    });
                }

                rows
            })
            .unwrap_or_default()
    }

    pub(super) fn options_required_height(&self, width: u16) -> u16 {
        if !self.has_options() {
            return 0;
        }

        let rows = self.option_rows();
        if rows.is_empty() {
            return 1;
        }

        let mut state = self
            .current_answer()
            .map(|answer| answer.options_state)
            .unwrap_or_default();
        if state.selected_idx.is_none() {
            state.selected_idx = Some(0);
        }

        measure_rows_height(&rows, &state, rows.len(), width.max(1))
    }

    pub(super) fn options_preferred_height(&self, width: u16) -> u16 {
        if !self.has_options() {
            return 0;
        }

        let rows = self.option_rows();
        if rows.is_empty() {
            return 1;
        }

        let mut state = self
            .current_answer()
            .map(|answer| answer.options_state)
            .unwrap_or_default();
        if state.selected_idx.is_none() {
            state.selected_idx = Some(0);
        }

        measure_rows_height(&rows, &state, rows.len(), width.max(1))
    }

    fn capture_composer_draft(&self) -> ComposerDraft {
        ComposerDraft {
            text: self.composer.current_text(),
            text_elements: self.composer.text_elements(),
            pending_pastes: self.composer.pending_pastes(),
        }
    }

    fn save_current_draft(&mut self) {
        let draft = self.capture_composer_draft();
        let notes_empty = draft.text.trim().is_empty();
        if let Some(answer) = self.current_answer_mut() {
            if answer.answer_committed && answer.draft != draft {
                answer.answer_committed = false;
            }
            answer.draft = draft;
            if !notes_empty {
                answer.notes_visible = true;
            }
        }
    }

    fn restore_current_draft(&mut self) {
        self.composer
            .set_placeholder_text(self.notes_placeholder().to_string());
        self.composer.set_footer_hint_override(Some(Vec::new()));
        let Some(answer) = self.current_answer() else {
            self.composer
                .set_text_content(String::new(), Vec::new(), Vec::new());
            self.composer.move_cursor_to_end();
            return;
        };
        let draft = answer.draft.clone();
        self.composer
            .set_text_content(draft.text, draft.text_elements, Vec::new());
        self.composer.set_pending_pastes(draft.pending_pastes);
        self.composer.move_cursor_to_end();
    }

    fn notes_placeholder(&self) -> &'static str {
        if self.has_options() && self.selected_option_index().is_none() {
            SELECT_OPTION_PLACEHOLDER
        } else if self.has_options() {
            NOTES_PLACEHOLDER
        } else {
            ANSWER_PLACEHOLDER
        }
    }

    fn sync_composer_placeholder(&mut self) {
        self.composer
            .set_placeholder_text(self.notes_placeholder().to_string());
    }

    fn footer_tips(&self) -> Vec<Line<'static>> {
        let mut tips = Vec::new();
        let notes_visible = self.notes_ui_visible();
        if self.has_options() {
            if self.selected_option_index().is_some() && !notes_visible {
                tips.push(shortcut("tab", "to add notes"));
            }
            if self.selected_option_index().is_some() && notes_visible {
                tips.push(Line::from(
                    [
                        crate::key_hint::key_label_spans("tab"),
                        vec![" to clear notes".dim()],
                    ]
                    .concat(),
                ));
            }
        }

        let question_count = self.question_count();
        let is_last_question = self.current_index().saturating_add(1) >= question_count;
        let submit_key = if self.focus_is_notes() || !self.has_options() {
            self.composer_submit_hint.map(ShortcutHint::display_label)
        } else {
            self.list_keymap
                .primary_hint(ListAction::Accept)
                .map(ShortcutHint::display_label)
        };
        if let Some(submit_key) = submit_key {
            let submit_tip = if question_count == 1 {
                shortcut(&submit_key, "to submit answer")
            } else if is_last_question {
                shortcut(&submit_key, "to submit all")
            } else {
                shortcut(&submit_key, "to submit answer")
            };
            tips.push(submit_tip);
        }
        if question_count > 1 {
            if self.has_options() && !self.focus_is_notes() {
                tips.push(shortcut("←/→", "to navigate questions"));
            } else if !self.has_options() {
                tips.push(shortcut(
                    &format!(
                        "{} / {}",
                        crate::key_hint::ctrl(KeyCode::Char('p')).display_label(),
                        crate::key_hint::ctrl(KeyCode::Char('n')).display_label()
                    ),
                    "change question",
                ));
            }
        }
        if let Some(interrupt_key) = self.interrupt_turn_hint {
            tips.push(shortcut(&interrupt_key.display_label(), "to interrupt"));
        }
        tips
    }

    pub(super) fn footer_tip_lines(&self, width: u16) -> Vec<Vec<Line<'static>>> {
        self.wrap_footer_tips(width, self.footer_tips())
    }

    pub(super) fn footer_tip_lines_with_prefix(
        &self,
        width: u16,
        prefix: Option<Line<'static>>,
    ) -> Vec<Vec<Line<'static>>> {
        let mut tips = Vec::new();
        if let Some(prefix) = prefix {
            tips.push(prefix);
        }
        tips.extend(self.footer_tips());
        self.wrap_footer_tips(width, tips)
    }

    fn wrap_footer_tips(&self, width: u16, tips: Vec<Line<'static>>) -> Vec<Vec<Line<'static>>> {
        crate::footer_hint::wrap_hint_rows(
            tips,
            width,
            UnicodeWidthStr::width(TIP_SEPARATOR),
            Line::width,
        )
    }

    pub(super) fn footer_required_height(&self, width: u16) -> u16 {
        self.footer_tip_lines(width).len() as u16
    }

    /// Ensure the focus mode is valid for the current question.
    fn ensure_focus_available(&mut self) {
        if self.question_count() == 0 {
            return;
        }
        if !self.has_options() {
            self.focus = Focus::Notes;
            if let Some(answer) = self.current_answer_mut() {
                answer.notes_visible = true;
            }
            return;
        }
        if matches!(self.focus, Focus::Notes) && !self.notes_ui_visible() {
            self.focus = Focus::Options;
            self.sync_composer_placeholder();
        }
    }

    /// Rebuild local answer state from the current request.
    fn reset_for_request(&mut self) {
        self.answers = self
            .questions
            .iter()
            .map(|question| {
                let has_options = question
                    .options
                    .as_ref()
                    .is_some_and(|options| !options.is_empty());
                let mut options_state = ScrollState::new();
                if has_options {
                    options_state.selected_idx = Some(0);
                }
                AnswerState {
                    options_state,
                    draft: ComposerDraft::default(),
                    answer_committed: false,
                    notes_visible: !has_options,
                }
            })
            .collect();

        self.current_idx = 0;
        self.focus = Focus::Options;
        self.composer
            .set_text_content(String::new(), Vec::new(), Vec::new());
        self.confirm_unanswered = None;
        self.pending_submission_draft = None;
    }

    fn options_len_for_question(question: &ToolRequestUserInputQuestion) -> usize {
        let options_len = question
            .options
            .as_ref()
            .map(std::vec::Vec::len)
            .unwrap_or(0);
        if Self::other_option_enabled_for_question(question) {
            options_len + 1
        } else {
            options_len
        }
    }

    fn other_option_enabled_for_question(question: &ToolRequestUserInputQuestion) -> bool {
        question.is_other
            && question
                .options
                .as_ref()
                .is_some_and(|options| !options.is_empty())
    }

    fn option_label_for_index(
        question: &ToolRequestUserInputQuestion,
        idx: usize,
    ) -> Option<String> {
        let options = question.options.as_ref()?;
        if idx < options.len() {
            return options.get(idx).map(|opt| opt.label.clone());
        }
        if idx == options.len() && Self::other_option_enabled_for_question(question) {
            return Some(OTHER_OPTION_LABEL.to_string());
        }
        None
    }

    /// Move to the next/previous question, wrapping in either direction.
    fn move_question(&mut self, next: bool) {
        let len = self.question_count();
        if len == 0 {
            return;
        }
        self.save_current_draft();
        let offset = if next { 1 } else { len.saturating_sub(1) };
        self.current_idx = (self.current_idx + offset) % len;
        self.restore_current_draft();
        self.ensure_focus_available();
    }

    fn jump_to_question(&mut self, idx: usize) {
        if idx >= self.question_count() {
            return;
        }
        self.save_current_draft();
        self.current_idx = idx;
        self.restore_current_draft();
        self.ensure_focus_available();
    }

    /// Synchronize selection state to the currently focused option.
    fn select_current_option(&mut self, committed: bool) {
        if !self.has_options() {
            return;
        }
        let options_len = self.options_len();
        let updated = if let Some(answer) = self.current_answer_mut() {
            answer.options_state.clamp_selection(options_len);
            answer.answer_committed = committed;
            true
        } else {
            false
        };
        if updated {
            self.sync_composer_placeholder();
        }
    }

    /// Clear the current option selection and hide notes when empty.
    fn clear_selection(&mut self) {
        if !self.has_options() {
            return;
        }
        if let Some(answer) = self.current_answer_mut() {
            answer.options_state.reset();
            answer.draft = ComposerDraft::default();
            answer.answer_committed = false;
            answer.notes_visible = false;
        }
        self.pending_submission_draft = None;
        self.composer
            .set_text_content(String::new(), Vec::new(), Vec::new());
        self.composer.move_cursor_to_end();
        self.sync_composer_placeholder();
    }

    fn clear_notes_and_focus_options(&mut self) {
        if !self.has_options() {
            return;
        }
        if let Some(answer) = self.current_answer_mut() {
            answer.draft = ComposerDraft::default();
            answer.answer_committed = false;
            answer.notes_visible = false;
        }
        self.pending_submission_draft = None;
        self.composer
            .set_text_content(String::new(), Vec::new(), Vec::new());
        self.composer.move_cursor_to_end();
        self.focus = Focus::Options;
        self.sync_composer_placeholder();
    }

    /// Ensure there is a selection before allowing notes entry.
    fn ensure_selected_for_notes(&mut self) {
        if let Some(answer) = self.current_answer_mut() {
            answer.notes_visible = true;
        }
        self.sync_composer_placeholder();
    }

    /// Advance to next question, or submit when on the last one.
    fn go_next_or_submit(&mut self) {
        if self.current_index() + 1 >= self.question_count() {
            self.save_current_draft();
            if self.unanswered_count() > 0 {
                self.open_unanswered_confirmation();
            } else {
                self.submit_answers();
            }
        } else {
            self.move_question(/*next*/ true);
        }
    }

    /// Build the response payload and dispatch it to the app.
    fn submit_answers(&mut self) {
        self.confirm_unanswered = None;
        self.save_current_draft();
        let mut answers = HashMap::new();
        for (idx, question) in self.questions.iter().enumerate() {
            let answer_state = &self.answers[idx];
            let options = question.options.as_ref();
            // For option questions we may still produce no selection.
            let selected_idx =
                if options.is_some_and(|opts| !opts.is_empty()) && answer_state.answer_committed {
                    answer_state.options_state.selected_idx
                } else {
                    None
                };
            // Notes are appended as extra answers. For freeform questions, only submit when
            // the user explicitly committed the draft.
            let notes = if answer_state.answer_committed {
                answer_state.draft.text_with_pending().trim().to_string()
            } else {
                String::new()
            };
            let selected_label = selected_idx
                .and_then(|selected_idx| Self::option_label_for_index(question, selected_idx));
            let mut answer_list = selected_label.into_iter().collect::<Vec<_>>();
            if !notes.is_empty() {
                answer_list.push(format!("user_note: {notes}"));
            }
            answers.insert(
                question.id.clone(),
                ToolRequestUserInputAnswer {
                    answers: answer_list,
                },
            );
        }
        if let Some(reply) = self.reply.take() {
            reply.send(Ok(ToolRequestUserInputResponse { answers }));
        }
        self.done = true;
    }

    fn open_unanswered_confirmation(&mut self) {
        let mut state = ScrollState::new();
        state.selected_idx = Some(0);
        self.confirm_unanswered = Some(state);
    }

    fn close_unanswered_confirmation(&mut self) {
        self.confirm_unanswered = None;
    }

    fn unanswered_question_count(&self) -> usize {
        self.unanswered_count()
    }

    fn unanswered_submit_description(&self) -> String {
        let count = self.unanswered_question_count();
        let suffix = if count == 1 {
            UNANSWERED_CONFIRM_SUBMIT_DESC_SINGULAR
        } else {
            UNANSWERED_CONFIRM_SUBMIT_DESC_PLURAL
        };
        format!("Submit with {count} unanswered {suffix}")
    }

    fn first_unanswered_index(&self) -> Option<usize> {
        let current_text = self.composer.current_text();
        self.questions
            .iter()
            .enumerate()
            .find(|(idx, _)| !self.is_question_answered(*idx, &current_text))
            .map(|(idx, _)| idx)
    }

    fn unanswered_confirmation_rows(&self) -> Vec<GenericDisplayRow> {
        let selected = self
            .confirm_unanswered
            .as_ref()
            .and_then(|state| state.selected_idx)
            .unwrap_or(0);
        let entries = [
            (
                UNANSWERED_CONFIRM_SUBMIT,
                self.unanswered_submit_description(),
            ),
            (
                UNANSWERED_CONFIRM_GO_BACK,
                UNANSWERED_CONFIRM_GO_BACK_DESC.to_string(),
            ),
        ];
        entries
            .iter()
            .enumerate()
            .map(|(idx, (label, description))| {
                let prefix = if idx == selected { '›' } else { ' ' };
                let number = idx + 1;
                GenericDisplayRow {
                    selection_style: Some(crate::bottom_pane::selection_style()),
                    name: format!("{prefix} {number}. {label}"),
                    description: Some(description.clone()),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn is_question_answered(&self, idx: usize, _current_text: &str) -> bool {
        let Some(question) = self.questions.get(idx) else {
            return false;
        };
        let Some(answer) = self.answers.get(idx) else {
            return false;
        };
        let has_options = question
            .options
            .as_ref()
            .is_some_and(|options| !options.is_empty());
        if has_options {
            answer.options_state.selected_idx.is_some() && answer.answer_committed
        } else {
            answer.answer_committed
        }
    }

    /// Count questions that would submit an empty answer list.
    fn unanswered_count(&self) -> usize {
        let current_text = self.composer.current_text();
        self.questions
            .iter()
            .enumerate()
            .filter(|(idx, _question)| !self.is_question_answered(*idx, &current_text))
            .count()
    }

    /// Compute the preferred notes input height for the current question.
    fn notes_input_height(&self, width: u16) -> u16 {
        let min_height = MIN_COMPOSER_HEIGHT;
        self.composer
            .desired_height(width.max(1))
            .clamp(min_height, min_height.saturating_add(5))
    }

    fn apply_submission_to_draft(&mut self, text: String, text_elements: Vec<TextElement>) {
        if let Some(answer) = self.current_answer_mut() {
            answer.draft = ComposerDraft {
                text: text.clone(),
                text_elements: text_elements.clone(),
                pending_pastes: Vec::new(),
            };
        }
        self.composer
            .set_text_content(text, text_elements, Vec::new());
        self.composer.move_cursor_to_end();
        self.composer.set_footer_hint_override(Some(Vec::new()));
    }

    fn apply_submission_draft(&mut self, draft: ComposerDraft) {
        if let Some(answer) = self.current_answer_mut() {
            answer.draft = draft.clone();
        }
        self.composer
            .set_text_content(draft.text, draft.text_elements, Vec::new());
        self.composer.set_pending_pastes(draft.pending_pastes);
        self.composer.move_cursor_to_end();
        self.composer.set_footer_hint_override(Some(Vec::new()));
    }

    fn handle_composer_input_result(&mut self, result: InputResult) -> bool {
        match result {
            InputResult::Submitted {
                text,
                text_elements,
            }
            | InputResult::Queued {
                text,
                text_elements,
                ..
            } => {
                if self.has_options()
                    && matches!(self.focus, Focus::Notes)
                    && !text.trim().is_empty()
                {
                    let options_len = self.options_len();
                    if let Some(answer) = self.current_answer_mut() {
                        answer.options_state.clamp_selection(options_len);
                    }
                }
                if self.has_options() {
                    if let Some(answer) = self.current_answer_mut() {
                        answer.answer_committed = true;
                    }
                } else if let Some(answer) = self.current_answer_mut() {
                    answer.answer_committed = !text.trim().is_empty();
                }
                let draft_override = self.pending_submission_draft.take();
                if let Some(draft) = draft_override {
                    self.apply_submission_draft(draft);
                } else {
                    self.apply_submission_to_draft(text, text_elements);
                }
                self.go_next_or_submit();
                true
            }
            _ => false,
        }
    }

    fn handle_confirm_unanswered_key_event(&mut self, key_event: KeyEvent) {
        if key_event.kind == KeyEventKind::Release {
            return;
        }
        let Some(state) = self.confirm_unanswered.as_mut() else {
            return;
        };

        match key_event.code {
            KeyCode::Esc | KeyCode::Backspace => {
                self.close_unanswered_confirmation();
                if let Some(idx) = self.first_unanswered_index() {
                    self.jump_to_question(idx);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.move_up_wrap(/*len*/ 2);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.move_down_wrap(/*len*/ 2);
            }
            KeyCode::Enter => {
                let selected = state.selected_idx.unwrap_or(0);
                self.close_unanswered_confirmation();
                if selected == 0 {
                    self.submit_answers();
                } else if let Some(idx) = self.first_unanswered_index() {
                    self.jump_to_question(idx);
                }
            }
            KeyCode::Char('1') | KeyCode::Char('2') => {
                let idx = if matches!(key_event.code, KeyCode::Char('1')) {
                    0
                } else {
                    1
                };
                state.selected_idx = Some(idx);
            }
            _ => {}
        }
    }
}

impl BottomPaneView for RequestUserInputOverlay {
    fn keymap_contexts(&self) -> crate::keymap::KeymapContextSet {
        if self.confirm_unanswered_active() {
            return crate::keymap::KeymapContextSet::default();
        }
        if matches!(self.focus, Focus::Options) {
            return crate::keymap::KeymapContextSet::new(crate::keymap::KeymapContext::List)
                .with(crate::keymap::KeymapContext::Chat);
        }
        self.composer
            .keymap_contexts()
            .with(crate::keymap::KeymapContext::Chat)
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        true
    }

    fn will_interrupt_turn_on_key_event(&self, key_event: KeyEvent) -> bool {
        if KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL).is_press(key_event) {
            return self.confirm_unanswered_active()
                || !self.focus_is_notes()
                || self.composer.current_text_with_pending().is_empty();
        }

        key_event.kind != KeyEventKind::Release
            && !self.confirm_unanswered_active()
            && !(matches!(key_event.code, KeyCode::Esc)
                && self.has_options()
                && self.notes_ui_visible())
            && self.interrupt_turn_keys.is_pressed(key_event)
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if key_event.kind == KeyEventKind::Release {
            return;
        }

        if self.confirm_unanswered_active() {
            self.handle_confirm_unanswered_key_event(key_event);
            return;
        }

        if self.interrupt_turn_keys.is_pressed(key_event) {
            self.finish_user_input(RequestUserInputError::Cancelled);
            self.done = true;
            return;
        }

        if self.focus_is_notes() && self.composer_submit_keys.is_pressed(key_event) {
            self.ensure_selected_for_notes();
            self.pending_submission_draft = Some(self.capture_composer_draft());
            let (result, _) = self.composer.handle_key_event(key_event);
            if !self.handle_composer_input_result(result) {
                self.pending_submission_draft = None;
                if self.has_options() {
                    self.select_current_option(/*committed*/ true);
                }
                self.go_next_or_submit();
            }
            return;
        }

        // Question navigation is always available.
        match key_event {
            KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::PageUp,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.move_question(/*next*/ false);
                return;
            }
            KeyEvent {
                code: KeyCode::PageDown,
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_question(/*next*/ true);
                return;
            }
            KeyEvent {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE,
                ..
            } if self.has_options() && matches!(self.focus, Focus::Options) => {
                self.move_question(/*next*/ false);
                return;
            }
            _ if self.has_options()
                && matches!(self.focus, Focus::Options)
                && self.list_keymap.action_for(key_event) == Some(ListAction::MoveLeft) =>
            {
                self.move_question(/*next*/ false);
                return;
            }
            KeyEvent {
                code: KeyCode::Char('l'),
                modifiers: KeyModifiers::NONE,
                ..
            }
            | KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::NONE,
                ..
            } if self.has_options() && matches!(self.focus, Focus::Options) => {
                self.move_question(/*next*/ true);
                return;
            }
            _ if self.has_options()
                && matches!(self.focus, Focus::Options)
                && self.list_keymap.action_for(key_event) == Some(ListAction::MoveRight) =>
            {
                self.move_question(/*next*/ true);
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::Options => {
                let options_len = self.options_len();
                // Keep selection synchronized as the user moves.
                match (self.list_keymap.action_for(key_event), key_event.code) {
                    (Some(ListAction::MoveUp), _) | (_, KeyCode::Up | KeyCode::Char('k')) => {
                        let moved = if let Some(answer) = self.current_answer_mut() {
                            answer.options_state.move_up_wrap(options_len);
                            answer.answer_committed = false;
                            true
                        } else {
                            false
                        };
                        if moved {
                            self.sync_composer_placeholder();
                        }
                    }
                    (Some(ListAction::MoveDown), _) | (_, KeyCode::Down | KeyCode::Char('j')) => {
                        let moved = if let Some(answer) = self.current_answer_mut() {
                            answer.options_state.move_down_wrap(options_len);
                            answer.answer_committed = false;
                            true
                        } else {
                            false
                        };
                        if moved {
                            self.sync_composer_placeholder();
                        }
                    }
                    (_, KeyCode::Char(' ')) => {
                        self.select_current_option(/*committed*/ true);
                    }
                    (_, KeyCode::Backspace | KeyCode::Delete) => {
                        self.clear_selection();
                    }
                    (_, KeyCode::Tab) | (Some(ListAction::Accept), _) | (_, KeyCode::Enter)
                        if self.selected_option_index().is_some()
                            && (key_event.code == KeyCode::Tab
                                || self.current_question().is_some_and(|question| {
                                    Self::other_option_enabled_for_question(question)
                                        && self.selected_option_index()
                                            == question.options.as_ref().map(Vec::len)
                                })) =>
                    {
                        self.focus = Focus::Notes;
                        self.ensure_selected_for_notes();
                    }
                    (Some(ListAction::Accept), _) | (_, KeyCode::Enter) => {
                        let has_selection = self.selected_option_index().is_some();
                        if has_selection {
                            self.select_current_option(/*committed*/ true);
                        }
                        self.go_next_or_submit();
                    }
                    (_, KeyCode::Char(ch)) => {
                        if let Some(option_idx) = self.option_index_for_digit(ch) {
                            if let Some(answer) = self.current_answer_mut() {
                                answer.options_state.selected_idx = Some(option_idx);
                            }
                            self.select_current_option(/*committed*/ true);
                            self.go_next_or_submit();
                        }
                    }
                    _ => {}
                }
            }
            Focus::Notes => {
                let notes_empty = self.composer.current_text_with_pending().trim().is_empty();
                if self.has_options() && matches!(key_event.code, KeyCode::Tab) {
                    self.clear_notes_and_focus_options();
                    return;
                }
                if self.has_options() && matches!(key_event.code, KeyCode::Backspace) && notes_empty
                {
                    self.save_current_draft();
                    if let Some(answer) = self.current_answer_mut() {
                        answer.notes_visible = false;
                    }
                    self.focus = Focus::Options;
                    self.sync_composer_placeholder();
                    return;
                }
                if self.has_options() && matches!(key_event.code, KeyCode::Up | KeyCode::Down) {
                    let options_len = self.options_len();
                    match key_event.code {
                        KeyCode::Up => {
                            let moved = if let Some(answer) = self.current_answer_mut() {
                                answer.options_state.move_up_wrap(options_len);
                                answer.answer_committed = false;
                                true
                            } else {
                                false
                            };
                            if moved {
                                self.sync_composer_placeholder();
                            }
                        }
                        KeyCode::Down => {
                            let moved = if let Some(answer) = self.current_answer_mut() {
                                answer.options_state.move_down_wrap(options_len);
                                answer.answer_committed = false;
                                true
                            } else {
                                false
                            };
                            if moved {
                                self.sync_composer_placeholder();
                            }
                        }
                        _ => {}
                    }
                    return;
                }
                self.ensure_selected_for_notes();
                if matches!(
                    key_event.code,
                    KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
                ) && let Some(answer) = self.current_answer_mut()
                {
                    answer.answer_committed = false;
                }
                let before = self.capture_composer_draft();
                let (result, _) = self.composer.handle_key_event(key_event);
                let submitted = self.handle_composer_input_result(result);
                if !submitted {
                    let after = self.capture_composer_draft();
                    if before != after
                        && let Some(answer) = self.current_answer_mut()
                    {
                        answer.answer_committed = false;
                    }
                }
            }
        }
    }

    fn terminal_title_requires_action(&self) -> bool {
        true
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.finish_user_input(RequestUserInputError::Cancelled);
        CancellationEvent::Handled
    }

    fn is_complete(&self) -> bool {
        self.done
    }

    fn handle_paste(&mut self, pasted: String) -> bool {
        if pasted.is_empty() {
            return false;
        }
        if matches!(self.focus, Focus::Options) {
            // Treat pastes the same as typing: switch into notes.
            self.focus = Focus::Notes;
        }
        self.ensure_selected_for_notes();
        if let Some(answer) = self.current_answer_mut() {
            answer.answer_committed = false;
        }
        // 018：聊天粘贴会先改写控制字符；问答须保存原文，仅过滤 render 副本。
        // 纯文字问询不走图片与大粘贴占位路径，仍复用同一编辑器与每题草稿。
        self.composer.flush_pending_paste_burst();
        self.composer.insert_str(&pasted);
        true
    }

    fn flush_paste_burst_if_due(&mut self) -> bool {
        self.composer.flush_paste_burst_if_due()
    }

    fn is_in_paste_burst(&self) -> bool {
        self.composer.is_in_paste_burst()
    }

    fn pre_draw_tick(&mut self, _now: Instant) -> bool {
        if !self.live.load(Ordering::SeqCst) && !self.done {
            self.finish_user_input(RequestUserInputError::Closed);
            return true;
        }
        false
    }

    fn next_frame_delay(&self) -> Option<Duration> {
        self.composer.footer_flash_delay()
    }

    fn view_id(&self) -> Option<&'static str> {
        Some("user-input")
    }

    fn finish_user_input(&mut self, reason: RequestUserInputError) {
        if let Some(reply) = self.reply.take() {
            reply.send(Err(reason));
        }
        self.done = true;
    }
}

impl Drop for RequestUserInputOverlay {
    fn drop(&mut self) {
        self.finish_user_input(RequestUserInputError::Closed);
    }
}

#[cfg(test)]
mod tests;
