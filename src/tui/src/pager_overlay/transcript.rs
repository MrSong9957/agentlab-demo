//! Standalone transcript chrome around the same viewport used by the main conversation.
//!
//! Inline Ctrl+T and the resume preview own their cells here. The viewport exclusively owns
//! scrolling, wrapped layouts, selection, search and the live tail.

use super::*;
use crate::history_cell::HistoryRenderMode;
use crate::history_cell::SessionHeaderHistoryCell;
use crate::history_cell::SessionInfoCell;
use crate::keymap::RuntimeKeymap;
use crate::motion::MotionMode;
use crate::transcript_view::TranscriptBookmark;
use crate::transcript_view::TranscriptView;
use crate::transcript_view::ViewAction;
use crossterm::event::KeyEventKind;
use crossterm::event::MouseButton;
use crossterm::event::MouseEventKind;

pub(crate) struct TranscriptOverlay {
    pub(super) view: Box<TranscriptView>,
    pub(crate) motion: MotionMode,
    pub(super) cells: Vec<Arc<dyn HistoryCell>>,
    keymap: PagerKeymap,
    pub(super) highlight_cell: Option<usize>,
    pending_highlight: Option<usize>,
    content_area: Rect,
    cursor: Option<(u16, u16)>,
    notice: Option<String>,
    pub(crate) key_chord_hint: Option<Vec<(String, String)>>,
    pub(crate) browsing_footer: Option<Line<'static>>,
    is_done: bool,
    // 012-ADAPT（§8 独立详情页）：页模式——单工具 cell、Esc 一次关闭、
    // 右列 011 沟位滚动条（渲染＋拖动复用主视图同一套实现）
    page_title: Option<String>,
    page_rows: crate::transcript_view::scrollbar::RowCache,
    page_drag: Option<crate::transcript_view::scrollbar::ScrollDrag>,
}

impl TranscriptOverlay {
    pub(crate) fn bookmark(&mut self) -> TranscriptBookmark {
        self.view.bookmark(&self.cells)
    }

    pub(crate) fn restore_bookmark(&mut self, bookmark: TranscriptBookmark) {
        self.browsing_footer = None;
        self.set_highlight_cell(/*cell*/ None);
        self.view.restore_bookmark(bookmark);
    }

    pub(crate) fn set_presentation(&mut self, detailed: bool, mode: HistoryRenderMode) {
        self.view.set_presentation(detailed, mode);
    }

    pub(crate) fn is_detailed(&self) -> bool {
        self.view.is_detailed()
    }

    /// 012 §9（单一职责轮）：驱动侧页内取消反馈——notice 呈现在页状态行，
    /// 不改页内容（正在查看的旧工具 cell 不受影响）。
    pub(crate) fn set_notice(&mut self, notice: String) {
        self.notice = Some(notice);
    }

    pub(crate) fn new(cells: Vec<Arc<dyn HistoryCell>>, keymap: PagerKeymap) -> Self {
        let mut view = TranscriptView::default();
        view.set_presentation(/*detailed*/ true, HistoryRenderMode::Rich);
        Self {
            view: Box::new(view),
            motion: MotionMode::Reduced,
            cells,
            keymap,
            highlight_cell: None,
            pending_highlight: None,
            content_area: Rect::default(),
            cursor: None,
            notice: None,
            key_chord_hint: None,
            browsing_footer: None,
            is_done: false,
            page_title: None,
            page_rows: crate::transcript_view::scrollbar::RowCache::default(),
            page_drag: None,
        }
    }

    /// 012-ADAPT（§8）：进入单工具详情页模式——标题、Esc 一次关闭、沟位滚动条；
    /// 打开即定位内容顶部（工具命令/状态头先行——视图默认 Latest 在底部）。
    pub(crate) fn as_tool_page(mut self, title: String) -> Self {
        self.page_title = Some(title);
        self.view.jump_to_beginning(&self.cells);
        self
    }

    fn is_tool_page(&self) -> bool {
        self.page_title.is_some()
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let browsing = self.browsing_footer.is_some()
            && !self.view.has_active_interaction()
            && self.view.history != TranscriptHistoryState::Failed;
        let chrome_height = if browsing { 2 } else { 5 };
        let content_height = area.height.saturating_sub(chrome_height);
        self.content_area = Rect::new(
            area.x,
            area.y.saturating_add(/*rhs*/ 1),
            area.width,
            content_height,
        );
        // 012-ADAPT（§8）：详情页切出右列 1 格沟位（与主视图 K1 同法），
        // 正文收窄 1 列渲染后画 011 滚动条
        let (body_area, gutter_area) = if self.is_tool_page() {
            let content_width = self.content_area.width.saturating_sub(/*gutter*/ 1);
            let body = Rect { width: content_width, ..self.content_area };
            let gutter = Rect {
                x: self.content_area.x + content_width,
                width: self.content_area.width - content_width,
                ..self.content_area
            };
            (body, gutter)
        } else {
            (self.content_area, Rect::default())
        };
        self.view.render(body_area, buf, &self.cells);
        if self.is_tool_page() {
            crate::transcript_view::scrollbar::render_gutter(
                &mut self.view,
                &self.cells,
                &mut self.page_rows,
                gutter_area,
                buf,
            );
        }
        if let Some(index) = self.pending_highlight.take() {
            self.view.ensure_entry_visible(&self.cells, index);
            self.view.render(self.content_area, buf, &self.cells);
        }
        let header = Rect::new(area.x, area.y, area.width, area.height.min(/*other*/ 1));
        Span::from("/ ".repeat(area.width as usize / 2))
            .dim()
            .render(header, buf);
        // 012-ADAPT（§8）：详情页标题＝工具页标识（母本固定 TRANSCRIPT）
        let title = self
            .page_title
            .as_deref()
            .map(str::to_uppercase)
            .unwrap_or_else(|| "T R A N S C R I P T".to_owned());
        format!("/ {title}").dim().render(header, buf);
        let status = Rect::new(
            area.x,
            self.content_area.bottom(),
            area.width,
            /*height*/ 1,
        )
        .intersection(area);
        self.cursor = None;
        if browsing && let Some(footer) = &self.browsing_footer {
            if let Some(items) = &self.key_chord_hint {
                crate::bottom_pane::footer_hint_items_line(items).render(status, buf);
            } else {
                Widget::render(footer, status, buf);
            }
            return;
        }
        let hints = Rect::new(area.x, status.bottom(), area.width, /*height*/ 2).intersection(area);
        let latest_navigation = self
            .keymap
            .primary_hint("jump_bottom", &self.keymap.jump_bottom)
            .map_or_else(String::new, |hint| {
                format!("{} latest", hint.display_label())
            });
        // 012-ADAPT（§8）：详情页不走 footer_with_navigation（其占位吞掉提示区）——
        // 走状态行＋render_hints，保证固定「q back to conversation」提示在位
        let footer = if self.is_tool_page() {
            None
        } else {
            self.view
                .footer_with_navigation(status.width, self.motion, &latest_navigation)
        };
        if let Some(mut footer) = footer {
            self.cursor = footer
                .cursor_column
                .map(|column| (status.x + column, status.y));
            if let Some(notice) = &self.notice {
                let notice = Line::from(notice.clone()).dim();
                if self.view.is_search_editing() {
                    footer.text.lines.truncate(/*len*/ 1);
                    footer.text.lines.push(notice);
                } else {
                    footer.text = notice.into();
                }
            }
            Paragraph::new(footer.text).render(
                Rect::new(status.x, status.y, status.width, /*height*/ 3).intersection(area),
                buf,
            );
        } else {
            if let Some(notice) = &self.notice {
                Line::from(notice.as_str()).dim().render(status, buf);
            } else {
                self.view
                    .status_line_with_navigation(
                        &format!(
                            "{} select",
                            crate::key_hint::ctrl(KeyCode::Char(' ')).display_label()
                        ),
                        self.motion,
                    )
                    .render(status, buf);
            }
            self.render_hints(hints, buf);
        }
        if let Some(items) = &self.key_chord_hint {
            let row = Rect::new(hints.x, hints.y, hints.width, 1).intersection(area);
            Clear.render(row, buf);
            crate::bottom_pane::footer_hint_items_line(items).render(row, buf);
        }
    }

    pub(crate) fn draw(&mut self, tui: &mut tui::Tui) -> Result<()> {
        let mut scanning = false;
        tui.draw(u16::MAX, |frame| {
            self.view.prepare_width(frame.area().width);
            scanning = self.view.advance_search(&self.cells);
            self.render(frame.area(), frame.buffer);
            if let Some(cursor) = self.cursor {
                frame.set_cursor_position(cursor);
            }
        })?;
        let loading = self.view.is_loading_history() && self.motion == MotionMode::Animated;
        if scanning || loading {
            tui.frame_requester()
                .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
        }
        Ok(())
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        if let Some(completion) = tui.clipboard.poll()
            && let Some(follow) =
                self.view
                    .finish_copy(&self.cells, completion, /*current*/ true)
        {
            self.notice = Some(match &completion.1 {
                Ok(status) => status.message("selection"),
                Err(error) => format!("Copy failed: {error}"),
            });
            if follow {
                self.view.jump_to_latest();
                self.is_done = self.browsing_footer.is_some();
            }
        }
        if matches!(event, TuiEvent::Resume | TuiEvent::FocusLost) {
            self.view.cancel_primary();
            self.view.end_drag();
        }
        // Apply a queued prompt jump before navigation can move away from it.
        if self.pending_highlight.is_some()
            && (matches!(&event, TuiEvent::Key(key) if key.kind != KeyEventKind::Release)
                || matches!(&event, TuiEvent::Mouse(mouse) if mouse.kind != MouseEventKind::Moved))
        {
            self.draw(tui)?;
        }
        let action = match event {
            TuiEvent::Key(key) => self.handle_key(key),
            TuiEvent::Mouse(mut mouse) => {
                // 012-ADAPT（§8）：详情页沟位列（最右 1 列）归滚动条状态机——
                // 与主视图同一套 gutter_press/drag_to（011 端点/钳制语义复用）
                if self.is_tool_page() {
                    // 011 同款「按下归属」语义：Drag/Up 归滚动条状态机直至抬指
                    // （指针越出沟位列不漏成选区，page_drag 不滞留）
                    let gutter_x = self.content_area.right().saturating_sub(1);
                    let in_gutter = mouse.column >= gutter_x
                        && mouse.row >= self.content_area.y
                        && mouse.row < self.content_area.bottom();
                    if let Some(drag) = self.page_drag.as_ref() {
                        match mouse.kind {
                            MouseEventKind::Drag(MouseButton::Left) => {
                                let row = mouse
                                    .row
                                    .min(self.content_area.bottom().saturating_sub(1));
                                crate::transcript_view::scrollbar::drag_to(
                                    &mut self.view,
                                    &self.cells,
                                    &mut self.page_rows,
                                    drag,
                                    row,
                                );
                                tui.frame_requester().schedule_frame();
                                return Ok(());
                            }
                            MouseEventKind::Up(MouseButton::Left) => {
                                self.page_drag = None;
                                tui.frame_requester().schedule_frame();
                                return Ok(());
                            }
                            _ => {}
                        }
                    } else if in_gutter {
                        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                            self.page_drag = crate::transcript_view::scrollbar::gutter_press(
                                &mut self.view,
                                &self.cells,
                                &mut self.page_rows,
                                mouse.row,
                            );
                            tui.frame_requester().schedule_frame();
                            return Ok(());
                        }
                    }
                }
                if matches!(
                    mouse.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) {
                    mouse.column = self.content_area.x;
                    mouse.row = self.content_area.bottom().saturating_sub(1);
                }
                self.view.handle_mouse(mouse, &self.cells)
            }
            TuiEvent::Paste(text) => {
                if self.view.is_search_editing() {
                    self.view.end_selection(&self.cells);
                }
                self.view.paste_search(&text).then_some(ViewAction::Changed)
            }
            TuiEvent::Draw | TuiEvent::Resize(_) | TuiEvent::FocusGained | TuiEvent::Resume => {
                if matches!(event, TuiEvent::Draw) && self.view.tick_selection(&self.cells) {
                    tui.frame_requester()
                        .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                }
                self.draw(tui)?;
                return Ok(());
            }
            TuiEvent::FocusLost => None,
        };
        if let Some(action) = action {
            self.apply_action(tui, action);
            tui.frame_requester().schedule_frame();
        }
        Ok(())
    }

    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }

    pub(crate) fn is_scrolled_to_bottom(&self) -> bool {
        self.view.is_following()
    }

    pub(crate) fn set_keymap_bindings(&mut self, keymap: &RuntimeKeymap) {
        self.view.set_keymap_bindings(keymap);
    }

    pub(crate) fn begin_search(&mut self) {
        self.view.begin_search();
    }

    pub(crate) fn is_search_editing(&self) -> bool {
        self.view.is_search_editing()
    }

    pub(crate) fn owns_interaction_key(&self, key: KeyEvent) -> bool {
        self.view.owns_interaction_key(key)
    }

    pub(crate) fn has_active_interaction(&self) -> bool {
        self.view.has_active_interaction()
    }

    pub(crate) fn needs_history(&mut self) -> bool {
        self.view.needs_history(&self.cells)
    }

    pub(crate) fn history_state(&self) -> TranscriptHistoryState {
        self.view.history
    }

    pub(crate) fn set_history_state(
        &mut self,
        state: TranscriptHistoryState,
    ) -> TranscriptHistoryState {
        let previous = std::mem::replace(&mut self.view.history, state);
        if previous == TranscriptHistoryState::LoadingBeginning
            && state == TranscriptHistoryState::Complete
        {
            self.view.jump_to_entry(&self.cells, /*index*/ 0);
        }
        previous
    }

    pub(crate) fn should_load_older(&mut self, key: KeyEvent) -> bool {
        self.should_load_from_start(key)
            || (self.view.needs_history(&self.cells)
                && [
                    &self.keymap.scroll_up,
                    &self.keymap.page_up,
                    &self.keymap.half_page_up,
                ]
                .iter()
                .any(|bindings| bindings.is_pressed(key)))
    }

    pub(crate) fn should_load_from_start(&self, key: KeyEvent) -> bool {
        self.keymap.jump_top.is_pressed(key)
    }

    pub(crate) fn insert_cell(&mut self, cell: Arc<dyn HistoryCell>) {
        self.cells.push(cell);
    }

    /// Insert old history after the session header; stable viewport anchors keep their content.
    pub(crate) fn prepend(&mut self, cells: Vec<Arc<dyn HistoryCell>>) -> usize {
        if cells.is_empty() {
            return 0;
        }
        let index = self
            .cells
            .iter()
            .rposition(|cell| {
                cell.as_any().is::<SessionInfoCell>()
                    || cell.as_any().is::<SessionHeaderHistoryCell>()
            })
            .map_or(/*default*/ 0, |index| index + 1);
        let added = cells.len();
        self.cells.splice(index..index, cells);
        for highlight in [&mut self.highlight_cell, &mut self.pending_highlight] {
            if let Some(highlight) = highlight.as_mut().filter(|highlight| **highlight >= index) {
                *highlight += added;
            }
        }
        self.view.set_highlight(self.highlight_cell);
        self.view.history_loaded(&self.cells, index..index + added);
        index
    }

    pub(crate) fn replace_cells(&mut self, cells: Vec<Arc<dyn HistoryCell>>) {
        self.cells = cells;
        self.view.restart_search();
        self.view.history_loaded(&self.cells, 0..0);
        self.highlight_cell = self
            .highlight_cell
            .filter(|index| *index < self.cells.len());
        self.pending_highlight = self
            .pending_highlight
            .filter(|index| *index < self.cells.len());
        self.view.set_highlight(self.highlight_cell);
    }

    /// Grouping may change compact previews; retain the reader's revision before replacing cells.
    pub(crate) fn regroup_cells(
        &mut self,
        range: std::ops::Range<usize>,
        consolidated: Arc<dyn HistoryCell>,
    ) {
        self.view
            .replace_group(&self.cells, range.clone(), &consolidated);
        self.consolidate_cells(range, consolidated);
    }

    /// The removed tail's index continues to identify the live group that absorbed its calls.
    pub(crate) fn absorb_tail_into_live(&mut self, previous_revision: u64, hydrated_revision: u64) {
        self.view
            .absorb_tail_into_live(&self.cells, previous_revision, hydrated_revision);
        self.cells.pop();
    }

    pub(crate) fn consolidate_cells(
        &mut self,
        range: std::ops::Range<usize>,
        consolidated: Arc<dyn HistoryCell>,
    ) {
        let end = range.end.min(self.cells.len());
        let start = range.start.min(end);
        if start == end {
            return;
        }
        self.view
            .replace_range(&self.cells, start..end, &consolidated);
        for highlight in [&mut self.highlight_cell, &mut self.pending_highlight] {
            *highlight = highlight.map(|index| {
                if index < start {
                    index
                } else if index < end {
                    start
                } else {
                    index - (end - start - 1)
                }
            });
        }
        self.cells.splice(start..end, [consolidated]);
        self.view.set_highlight(self.highlight_cell);
    }

    pub(crate) fn sync_live_tail(
        &mut self,
        width: u16,
        key: Option<ActiveCellTranscriptKey>,
        compute_lines: impl FnOnce(u16) -> Option<Vec<HyperlinkLine>>,
    ) -> bool {
        self.view.sync_live_tail(width, key, compute_lines)
    }

    pub(crate) fn sync_search_live_tail(
        &mut self,
        width: u16,
        key: Option<ActiveCellTranscriptKey>,
        compute_lines: impl FnOnce(u16) -> Option<Vec<HyperlinkLine>>,
    ) {
        self.view.sync_search_live_tail(width, key, compute_lines);
    }

    pub(crate) fn set_highlight_cell(&mut self, cell: Option<usize>) {
        self.highlight_cell = cell.filter(|index| *index < self.cells.len());
        self.pending_highlight = self.highlight_cell;
        self.view.set_highlight(self.highlight_cell);
    }

    /// Apply prompt navigation before scrolling, even when both keys precede the next draw.
    pub(crate) fn scroll(&mut self, rows: isize) {
        if let Some(index) = self.pending_highlight.take() {
            self.view.ensure_entry_visible(&self.cells, index);
        }
        self.view.scroll(&self.cells, rows);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<ViewAction> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        // 012-ADAPT（§9 单一职责轮）：详情页 Esc 直关分支移除——Esc 由驱动统一
        // 拦截（运行中＝取消＋页保持、空闲＝无动作），不可达此处；关闭＝q（经
        // 下方 keymap.close，母本原生能力）或 Ctrl+T（close_transcript 切换）。
        if !self.is_tool_page()
            && !self.view.is_search_editing()
            && (!self.view.has_active_interaction() || !self.view.owns_interaction_key(key))
            && self.keymap.find.is_pressed(key)
        {
            self.view.begin_search();
            return Some(ViewAction::Changed);
        }
        if self.view.has_active_interaction()
            && let Some(action) = self.view.handle_key(key, &self.cells)
        {
            return Some(action);
        }
        // 012-ADAPT（§9）：页模式启用 keymap.close（q 关页——单一职责轮契约；
        // close 绑定含 ctrl+c 但被驱动在路由前拦截为恒退出，实际可达成员仅 q）
        if self.keymap.close.is_pressed(key) || self.keymap.close_transcript.is_pressed(key) {
            self.is_done = true;
            return Some(ViewAction::Changed);
        }
        if self.view.navigate_pager(key, &self.cells, &self.keymap) {
            return Some(ViewAction::Changed);
        }
        self.view.handle_key(key, &self.cells)
    }

    pub(crate) fn cancel_pending_jump(&mut self) {
        self.view.cancel_beginning();
    }

    fn apply_action(&mut self, tui: &mut tui::Tui, action: ViewAction) {
        self.notice = None;
        if let Some((_, result)) = self.view.copy_action(
            tui,
            &self.cells,
            &action,
            crate::clipboard_copy::CopyFormat::Markdown,
        ) {
            self.notice = Some(match result {
                Ok(status) => status.message("selection"),
                Err(error) => error,
            });
        } else if let ViewAction::OpenLink(url) = action
            && let Err(error) = webbrowser::open(&url)
        {
            self.notice = Some(format!("Could not open link: {error}"));
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let first = Rect::new(area.x, area.y, area.width, /*height*/ 1).intersection(area);
        render_navigation_hints(first, buf, &self.keymap);
        let second = Rect::new(area.x, first.bottom(), area.width, /*height*/ 1).intersection(area);
        // 012-ADAPT（§9 单一职责轮）：详情页固定「q 返回对话」提示——Esc 已专
        // 取消（驱动拦截不进页）；键标取 keymap.close 首绑定（默认 q）
        if self.is_tool_page() {
            render_key_hints(
                second,
                buf,
                &[(
                    super::first_or_empty(&self.keymap, "close", &self.keymap.close),
                    "back to conversation",
                )],
            );
            return;
        }
        let mut pairs = vec![(
            first_or_empty(&self.keymap, "close", &self.keymap.close),
            "close",
        )];
        if !self.keymap.find.is_empty() {
            pairs.push((
                first_or_empty(&self.keymap, "find", &self.keymap.find),
                "find",
            ));
        }
        pairs.push((vec![key_hint::plain(KeyCode::Esc).into()], "browse prompts"));
        if self.highlight_cell.is_some() {
            pairs.push((vec![key_hint::plain(KeyCode::Right).into()], "to edit next"));
            pairs.push((
                vec![key_hint::plain(KeyCode::Enter).into()],
                "to edit message",
            ));
        }
        render_key_hints(second, buf, &pairs);
    }
}
