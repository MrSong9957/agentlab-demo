//! File-search popup for `@` completions (020 A02 revival of the 008-trimmed
//! upstream popup). Local adaptation of codex `file_search_popup.rs` @ d138398:
//! synchronous in-composer search results instead of the async file-search
//! service; substring match highlighting instead of fuzzy-match indices.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::WidgetRef;

use super::popup_consts::MAX_POPUP_ROWS;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use crate::width::display_width;

/// A single search result (local stand-in for `codex_file_search::FileMatch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileMatch {
    pub(crate) path: PathBuf,
}

/// Visual state for the file-search popup.
pub(crate) struct FileSearchPopup {
    /// Query corresponding to the `matches` currently shown.
    display_query: String,
    matches: Vec<FileMatch>,
    /// Shared selection/scroll state.
    state: ScrollState,
}

impl Default for FileSearchPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSearchPopup {
    pub(crate) fn new() -> Self {
        Self {
            display_query: String::new(),
            matches: Vec::new(),
            state: ScrollState::new(),
        }
    }

    /// Update the query and reset selection for the incoming result set.
    pub(crate) fn set_query(&mut self, query: &str) {
        self.display_query = query.to_string();
        self.matches.clear();
        self.state.reset();
    }

    /// Replace matches (already filtered for this query by the caller).
    pub(crate) fn set_matches(&mut self, matches: Vec<FileMatch>) {
        self.matches = matches.into_iter().take(MAX_POPUP_ROWS).collect();
        let len = self.matches.len();
        self.state.clamp_selection(len);
        self.state.ensure_visible(len, len.min(MAX_POPUP_ROWS));
    }

    /// Move selection cursor up.
    pub(crate) fn move_up(&mut self) {
        let len = self.matches.len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, len.min(MAX_POPUP_ROWS));
    }

    /// Move selection cursor down.
    pub(crate) fn move_down(&mut self) {
        let len = self.matches.len();
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, len.min(MAX_POPUP_ROWS));
    }

    pub(crate) fn selected_match(&self) -> Option<&PathBuf> {
        self.state
            .selected_idx
            .and_then(|idx| self.matches.get(idx))
            .map(|file_match| &file_match.path)
    }

    pub(crate) fn calculate_required_height(&self) -> u16 {
        self.matches.len().clamp(/*min*/ 1, MAX_POPUP_ROWS) as u16 + 2
    }
}

impl WidgetRef for &FileSearchPopup {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let rows_all: Vec<GenericDisplayRow> = self
            .matches
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                let path = m.path.to_string_lossy();
                let width = usize::from(area.width.saturating_sub(/*rhs*/ 2));
                // Keep the filename visible for long paths; the transcript keeps the full path.
                let name = if display_width(&path) <= width {
                    path.to_string()
                } else {
                    // 长路径只显示文件名（真正截断由 render_rows 行渲染完成）
                    m.path
                        .file_name()
                        .unwrap_or(m.path.as_os_str())
                        .to_string_lossy()
                        .into_owned()
                };
                // Substring match positions (char indices) for highlighting.
                let match_indices = find_substring_char_indices(&name, &self.display_query);
                GenericDisplayRow {
                    category_tag: None,
                    selection_style: Some(super::picker_style::selection_style()),
                    name,
                    name_prefix_spans: vec![
                        if self.state.selected_idx == Some(idx) {
                            "› "
                        } else {
                            "  "
                        }
                        .into(),
                    ],
                    match_indices,
                    display_shortcut: None,
                    description: None,
                    wrap_indent: None,
                    is_disabled: false,
                    disabled_reason: None,
                }
            })
            .collect();
        let empty_message = if self.matches.is_empty() {
            if self.display_query.is_empty() {
                "Type to search files"
            } else {
                "No matching files"
            }
        } else {
            ""
        };
        super::picker_rows::render_rows_single_line(
            area,
            buf,
            &rows_all,
            &self.state,
            self.matches.len().min(MAX_POPUP_ROWS),
            empty_message,
        );
    }
}

/// Case-insensitive substring char-index list for highlight rendering.
fn find_substring_char_indices(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    if needle.is_empty() {
        return None;
    }
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).collect();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    let lowered: Vec<char> = h.iter().flat_map(|c| c.to_lowercase()).collect();
    // lowering can change length; fall back to no highlight on mismatch
    if lowered.len() != h.len() {
        return None;
    }
    for start in 0..=(h.len() - n.len()) {
        if lowered[start..start + n.len()] == n[..] {
            return Some((start..start + n.len()).collect());
        }
    }
    None
}
