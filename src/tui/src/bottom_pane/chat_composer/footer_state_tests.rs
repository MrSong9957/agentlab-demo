//! Footer replacement keeps the existing layout and the focused editor's cursor together.

use super::*;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::ChatComposer;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc::unbounded_channel;

fn composer() -> ChatComposer {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        AppEventSender::new(tx),
        /*enhanced_keys_supported*/ false,
        "Ask Codex".to_string(),
        /*disable_paste_burst*/ true,
    );
    composer.set_status_line_enabled(/*enabled*/ true);
    composer.set_status_line(Some(Line::from("MODEL STATUS")));
    composer
}

fn render(
    composer: &ChatComposer,
    footer: Option<&TranscriptFooter>,
) -> (Buffer, Option<(u16, u16)>) {
    let options = composer.resolve_render_options(ComposerRenderOptions {
        footer,
        ..ComposerRenderOptions::default()
    });
    let width = 60;
    let area = Rect::new(
        /*x*/ 0,
        /*y*/ 0,
        width,
        composer.desired_height_with_options(width, options),
    );
    let mut buffer = Buffer::empty(area);
    composer.render_with_options(area, &mut buffer, /*mask_char*/ None, options);
    (buffer, composer.cursor_pos_with_options(area, options))
}

fn text(buffer: &Buffer) -> String {
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// 008-TRIM: fn retained_command_suggestions_yield_paint_and_cursor_to_find（覆盖被裁
// command 弹窗（ActivePopup::Command，本地 / 命令原样提交由驱动处理），008 §1）

#[test]
fn copying_from_find_preserves_the_query_above_selection_feedback() {
    let mut composer = composer();
    let footer = TranscriptFooter {
        text: vec![
            Line::from("Find: needle"),
            Line::from("Ctrl+C copy · Esc clear selection"),
        ]
        .into(),
        cursor_column: None,
        is_interactive: true,
    };
    composer.show_footer_flash(
        "Copied selection to clipboard".into(),
        Duration::from_secs(/*secs*/ 3),
    );
    let (buffer, cursor) = render(&composer, Some(&footer));
    assert_eq!(cursor, None);
    insta::assert_snapshot!("transcript_find_copy_footer", text(&buffer));
}

#[test]
fn interactive_query_keeps_focus_over_underlying_hints_but_empty_override_wins() {
    let mut composer = composer();
    let footer = TranscriptFooter {
        text: vec![
            Line::from("Find: needle"),
            Line::from("Enter next · Esc close"),
        ]
        .into(),
        cursor_column: Some(12),
        is_interactive: true,
    };
    let expected = render(&composer, Some(&footer));
    composer.set_footer_hint_override(Some(vec![("ctrl+c".to_string(), "quit".to_string())]));
    assert_eq!(render(&composer, Some(&footer)), expected);
    composer.set_footer_hint_override(Some(Vec::new()));
    assert_eq!(
        render(&composer, Some(&footer)),
        render(&composer, /*footer*/ None)
    );
}
