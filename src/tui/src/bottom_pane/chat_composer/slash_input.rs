//! Slash-command input parsing, cursor detection, and completion helpers.

use std::ops::Range;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use crate::bottom_pane::prompt_args::parse_slash_name;
use crate::bottom_pane::slash_commands::BuiltinCommandFlags;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::bottom_pane::slash_commands::SlashCommandItem;
use crate::bottom_pane::slash_commands::find_slash_command;
use crate::bottom_pane::slash_commands::has_slash_command_prefix;
use codex_protocol::user_input::ByteRange;
use codex_protocol::user_input::TextElement;

use super::ChatComposer;
use super::InputResult;
use super::QueuedInputAction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlashValidation {
    Immediate,
    Deferred,
}

pub(super) enum SubmissionValidation {
    Valid,
    UnknownCommand(String),
}

pub(super) struct InlineCommand<'a> {
    pub(super) command: SlashCommandItem,
    pub(super) rest: &'a str,
    pub(super) rest_offset: usize,
}

pub(super) struct SlashInput<'a> {
    enabled: bool,
    is_bash_mode: bool,
    command_flags: BuiltinCommandFlags,
    service_tier_commands: &'a [ServiceTierCommand],
}

impl<'a> SlashInput<'a> {
    pub(super) fn new(
        enabled: bool,
        is_bash_mode: bool,
        command_flags: BuiltinCommandFlags,
        service_tier_commands: &'a [ServiceTierCommand],
    ) -> Self {
        Self {
            enabled,
            is_bash_mode,
            command_flags,
            service_tier_commands,
        }
    }

    pub(super) fn validate_submission(
        &self,
        text: &str,
        input_starts_with_space: bool,
    ) -> SubmissionValidation {
        if !self.enabled {
            return SubmissionValidation::Valid;
        }
        let Some((name, _rest, _rest_offset)) = parse_slash_name(text) else {
            return SubmissionValidation::Valid;
        };
        if input_starts_with_space || name.contains('/') {
            return SubmissionValidation::Valid;
        }
        if self.command(name).is_some() {
            SubmissionValidation::Valid
        } else {
            SubmissionValidation::UnknownCommand(name.to_string())
        }
    }

    pub(super) fn bare_command(&self, text: &str) -> Option<SlashCommandItem> {
        if !self.enabled || self.is_bash_mode {
            return None;
        }
        let first_line = text.lines().next().unwrap_or("");
        let (name, rest, _rest_offset) = parse_slash_name(first_line)?;
        if !rest.is_empty() {
            return None;
        }
        let command = self.command(name)?;
        if command.supports_inline_args()
            && parse_slash_name(text).is_some_and(|(_, full_rest, _)| !full_rest.is_empty())
        {
            return None;
        }
        Some(command)
    }

    pub(super) fn inline_command<'text>(&self, text: &'text str) -> Option<InlineCommand<'text>> {
        if !self.enabled || self.is_bash_mode || text.starts_with(' ') {
            return None;
        }

        let (name, rest, rest_offset) = parse_slash_name(text)?;
        if rest.is_empty() || name.contains('/') {
            return None;
        }

        let command = self.command(name)?;
        command.supports_inline_args().then_some(InlineCommand {
            command,
            rest,
            rest_offset,
        })
    }

    pub(super) fn should_parse_on_dequeue(&self, text: &str) -> bool {
        self.enabled && !text.starts_with(' ') && text.trim().starts_with('/')
    }

    pub(super) fn command_element_range(
        &self,
        first_line: &str,
        cursor: usize,
    ) -> Option<Range<usize>> {
        if self.is_bash_mode {
            return None;
        }
        let (name, _rest, _rest_offset) = parse_slash_name(first_line)?;
        if name.contains('/') {
            return None;
        }
        let element_end = 1 + name.len();
        // A draft tail can make an in-progress prefix look complete ("/re" + "view").
        // Keep it editable until the cursor leaves the command name.
        if cursor <= first_line.len() && (1..element_end).contains(&cursor) {
            return None;
        }
        let has_space_after = first_line
            .get(element_end..)
            .and_then(|tail| tail.chars().next())
            .is_some_and(char::is_whitespace);
        if !has_space_after {
            return None;
        }
        self.command(name).is_some().then_some(0..element_end)
    }

    pub(super) fn is_editing_command_name(&self, first_line: &str, cursor: usize) -> bool {
        let Some((name, rest)) = command_under_cursor(first_line, cursor) else {
            return false;
        };
        if !self.enabled {
            return false;
        }
        if name.is_empty() {
            return rest.is_empty();
        }

        has_slash_command_prefix(name, self.command_flags, self.service_tier_commands)
    }

    // 008-TRIM: command_popup 过滤方法（弹窗，首批无弹窗，008 §1）    }

    pub(super) fn command(&self, name: &str) -> Option<SlashCommandItem> {
        find_slash_command(name, self.command_flags, self.service_tier_commands)
    }
}

pub(super) fn queued_input_action(
    prepared_text: &str,
    defer_slash_validation: bool,
) -> QueuedInputAction {
    if defer_slash_validation && prepared_text.starts_with('/') {
        QueuedInputAction::ParseSlash
    } else if prepared_text.starts_with('!') {
        QueuedInputAction::RunShell
    } else {
        QueuedInputAction::Plain
    }
}

impl ChatComposer {
    pub(super) fn builtin_command_flags(&self) -> BuiltinCommandFlags {
        BuiltinCommandFlags {
            collaboration_modes_enabled: self.collaboration_modes_enabled,
            connectors_enabled: self.connectors_enabled,
            plugins_command_enabled: self.plugins_command_enabled,
            token_activity_command_enabled: self.token_activity_command_enabled,
            service_tier_commands_enabled: self.service_tier_commands_enabled,
            daybreak_command_description: self.popups.daybreak_command_description,
            goal_command_enabled: self.goal_command_enabled,
            voice_command_enabled: self.voice_command_enabled,
            worktrees_enabled: self.worktrees_enabled,
            allow_elevate_sandbox: self.windows_degraded_sandbox_active,
            side_conversation_active: self.side_conversation_active,
        }
    }

    pub fn set_daybreak_command_description(&mut self, description: Option<&'static str>) {
        self.popups.daybreak_command_description = description;
    }

    pub fn set_worktrees_enabled(&mut self, enabled: bool) {
        self.worktrees_enabled = enabled;
    }

    // 008-TRIM: handle_key_event_with_slash_popup 与 complete_selected_slash_command…（弹窗按键路由/补全，首批无弹窗，008 §1）
    pub(super) fn sync_slash_command_elements(&mut self) {
        if !self.slash_commands_enabled() {
            return;
        }
        let text = self.draft.textarea.text();
        let first_line_end = text.find('\n').unwrap_or(text.len());
        let first_line = &text[..first_line_end];
        let cursor = self.draft.textarea.cursor();
        let desired_range = self.slash_input().command_element_range(first_line, cursor);
        // Slash commands are only valid at byte 0 of the first line.
        // Any slash-shaped element not matching the current desired prefix is stale.
        let mut has_desired = false;
        let mut stale_ranges = Vec::new();
        for elem in self.draft.textarea.text_elements() {
            let Some(payload) = elem.placeholder(text) else {
                continue;
            };
            if payload.strip_prefix('/').is_none() {
                continue;
            }
            let range = elem.byte_range.start..elem.byte_range.end;
            if desired_range.as_ref() == Some(&range) {
                has_desired = true;
            } else {
                stale_ranges.push(range);
            }
        }

        for range in stale_ranges {
            self.draft.textarea.remove_element_range(range);
        }

        if let Some(range) = desired_range
            && !has_desired
        {
            self.draft.textarea.add_element_range(range);
        }
    }
}

// 008-TRIM: selected_command_dispatches_immediately_on_tab / selected_command_completion（弹窗补全，首批无弹窗，008 §1）

pub(super) fn prepared_args(prepared_text: &str) -> Option<(&str, usize)> {
    let (_, prepared_rest, prepared_rest_offset) = parse_slash_name(prepared_text)?;
    Some((prepared_rest, prepared_rest_offset))
}

/// Translate full-text element ranges into command-argument ranges.
///
/// `rest_offset` is the byte offset where `rest` begins in the full text.
pub(super) fn args_elements(
    rest: &str,
    rest_offset: usize,
    text_elements: &[TextElement],
) -> Vec<TextElement> {
    if rest.is_empty() || text_elements.is_empty() {
        return Vec::new();
    }
    text_elements
        .iter()
        .filter_map(|elem| {
            if elem.byte_range.end <= rest_offset {
                return None;
            }
            let start = elem.byte_range.start.saturating_sub(rest_offset);
            let mut end = elem.byte_range.end.saturating_sub(rest_offset);
            if start >= rest.len() {
                return None;
            }
            end = end.min(rest.len());
            (start < end).then_some(elem.map_range(|_| ByteRange { start, end }))
        })
        .collect()
}

pub(super) fn command_popup_filter_text(first_line: &str, cursor: usize) -> Option<String> {
    let (name, _rest) = command_under_cursor(first_line, cursor)?;
    Some(format!("/{name}"))
}

/// If the cursor is currently within a slash command on the first line,
/// extract the command fragment before the cursor and the rest of the line after it.
fn command_under_cursor(first_line: &str, cursor: usize) -> Option<(&str, &str)> {
    if !first_line.starts_with('/') {
        return None;
    }
    if cursor > first_line.len() || !first_line.is_char_boundary(cursor) {
        return None;
    }

    let name_start = 1usize;
    let name_end = first_line[name_start..]
        .find(char::is_whitespace)
        .map(|idx| name_start + idx)
        .unwrap_or_else(|| first_line.len());

    let cursor = if cursor <= name_start {
        name_end
    } else {
        cursor
    };
    if cursor > name_end {
        return None;
    }

    let name = &first_line[name_start..cursor];
    let rest = &first_line[cursor..];

    Some((name, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use crate::bottom_pane::AppEventSender;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    fn test_composer() -> ChatComposer {
        let (tx, _rx) = unbounded_channel::<AppEvent>();
        ChatComposer::new(
            /*has_input_focus*/ true,
            AppEventSender::new(tx),
            /*enhanced_keys_supported*/ false,
            "Ask Codex to do anything".to_string(),
            /*disable_paste_burst*/ false,
        )
    }

    fn press(composer: &mut ChatComposer, code: KeyCode) -> InputResult {
        composer
            .handle_key_event(KeyEvent::new(code, KeyModifiers::NONE))
            .0
    }

    fn composer_with_text_at_cursor(text: &str, cursor: usize) -> ChatComposer {
        let mut composer = test_composer();
        composer.draft.textarea.set_text_clearing_elements(text);
        composer.draft.textarea.set_cursor(cursor);
        composer.sync_popups();
        composer
    }

    fn composer_with_draft_tail(prefix: &str, draft: &str) -> ChatComposer {
        composer_with_text_at_cursor(&format!("{prefix}{draft}"), prefix.len())
    }

    // 008-TRIM: fn unavailable_side_conversation_command_still_opens_popup（覆盖被裁
    // command 弹窗（ActivePopup::Command），008 §1）

    // 008-TRIM: fn side_conversation_tab_completes_available_match_after_unavailable_rows（覆盖被裁 command/slash 弹窗补全与消散（ActivePopup::Command 裁剪，008 §1））

    // 008-TRIM: fn side_conversation_tab_does_not_queue_unavailable_match_while_task_running（覆盖被裁 command/slash 弹窗补全与消散（ActivePopup::Command 裁剪，008 §1））

    #[test]
    fn side_conversation_tab_queues_exact_available_match_while_task_running() {
        let mut composer = test_composer();
        composer.set_side_conversation_active(/*active*/ true);
        composer.set_task_running(/*running*/ true);
        composer
            .draft
            .textarea
            .set_text_clearing_elements("/agents");
        composer.draft.textarea.set_cursor(/*pos*/ 7);
        composer.sync_popups();

        assert_eq!(
            press(&mut composer, KeyCode::Tab),
            InputResult::Queued {
                text: "/agents".to_string(),
                text_elements: Vec::new(),
                action: QueuedInputAction::ParseSlash,
                pending_pastes: Vec::new(),
            }
        );
        assert!(composer.draft.textarea.is_empty());
    }

    #[test]
    fn side_conversation_slash_is_inserted_after_unavailable_match() {
        let mut composer = test_composer();
        composer.set_side_conversation_active(/*active*/ true);
        composer.set_disable_paste_burst(/*disabled*/ true);
        composer.draft.textarea.set_text_clearing_elements("/new");
        composer.draft.textarea.set_cursor(/*pos*/ 4);
        composer.sync_popups();

        assert_eq!(press(&mut composer, KeyCode::Char('/')), InputResult::None);
        assert_eq!(composer.draft.textarea.text(), "/new/");
    }

    // 008-TRIM: fn esc_dismisses_slash_popup_while_idle（覆盖被裁 command/slash 弹窗补全与消散（ActivePopup::Command 裁剪，008 §1））

    // 008-TRIM: fn slash_completion_preserves_existing_draft_tail_for_inline_arg_commands（覆盖被裁 command/slash 弹窗补全与消散（ActivePopup::Command 裁剪，008 §1））

    // 008-TRIM: fn slash_completion_does_not_preserve_existing_draft_tail_for_other_commands（覆盖被裁 command/slash 弹窗补全与消散（ActivePopup::Command 裁剪，008 §1））

    // 008-TRIM: fn slash_completion_does_not_turn_command_suffix_into_args（覆盖被裁 command/slash 弹窗补全与消散（ActivePopup::Command 裁剪，008 §1））
}
