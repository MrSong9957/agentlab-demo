//! Persistent status survives transient footer modes and suggestion overlays.

use super::*;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc::unbounded_channel;

fn composer() -> ChatComposer {
    let (tx, _rx) = unbounded_channel();
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        AppEventSender::new(tx),
        /*enhanced_keys_supported*/ false,
        "Ask Codex".to_string(),
        /*disable_paste_burst*/ true,
    );
    composer.set_status_line_enabled(/*enabled*/ true);
    composer.set_status_line(Some(Line::from("MODEL · ~/project · Context 20% used")));
    composer
}

fn render(
    composer: &ChatComposer,
    width: u16,
    footer: Option<&TranscriptFooter>,
) -> (String, Option<(u16, u16)>) {
    render_with_height(composer, width, footer, /*height*/ None)
}

fn render_with_height(
    composer: &ChatComposer,
    width: u16,
    footer: Option<&TranscriptFooter>,
    height: Option<u16>,
) -> (String, Option<(u16, u16)>) {
    let options = composer.resolve_render_options(ComposerRenderOptions {
        separate_status_line: true,
        command_popup_placement: CommandPopupPlacement::Overlay,
        footer,
        ..ComposerRenderOptions::default()
    });
    let height = height.unwrap_or_else(|| composer.desired_height_with_options(width, options));
    let area = Rect::new(/*x*/ 0, /*y*/ 8, width, height);
    let mut buf = Buffer::empty(Rect::new(/*x*/ 0, /*y*/ 0, width, area.bottom()));
    composer.render_with_options(area, &mut buf, /*mask_char*/ None, options);
    let text = buf
        .content
        .chunks(usize::from(width))
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, composer.cursor_pos_with_options(area, options))
}

#[test]
fn passive_activity_keeps_shortcuts_only_when_the_complete_hint_fits() {
    let mut composer = composer();
    let mut line = Line::from(key_hint::key_label_spans("ctrl+o t/f4"));
    line.push_span(" inspect activity".dim());
    let mut footer = TranscriptFooter {
        text: line.into(),
        cursor_column: None,
        is_interactive: false,
    };
    for (width, expected) in [
        (80, "ctrl+o t/f4 inspect activity · ? shortcuts"),
        (40, "ctrl+o t/f4 inspect activity"),
    ] {
        let (text, _) = render(&composer, width, Some(&footer));
        assert_eq!(text.lines().last().map(str::trim), Some(expected));
    }
    footer.is_interactive = true;
    let (text, _) = render(&composer, /*width*/ 80, Some(&footer));
    assert_eq!(
        text.lines().last().map(str::trim),
        Some("ctrl+o t/f4 inspect activity")
    );
    footer.is_interactive = false;
    composer.footer.toggle_shortcuts_key = None;
    let (text, _) = render(&composer, /*width*/ 80, Some(&footer));
    assert_eq!(
        text.lines().last().map(str::trim),
        Some("ctrl+o t/f4 inspect activity")
    );
}

#[test]
fn fullscreen_plan_indicator_keeps_the_cycle_hint_when_it_fits() {
    let mut composer = composer();
    composer.set_collaboration_modes_enabled(/*enabled*/ true);
    composer.set_collaboration_mode_indicator(Some(CollaborationModeIndicator::Plan));
    let (wide, _) = render(&composer, /*width*/ 100, /*footer*/ None);
    assert!(
        wide.contains("Plan mode (⇧tab to cycle)"),
        "idle fullscreen must explain how to leave Plan mode: {wide}"
    );
    insta::assert_snapshot!("fullscreen_plan_cycle_hint", wide);

    let (narrow, _) = render(&composer, /*width*/ 44, /*footer*/ None);
    assert!(narrow.contains("Plan mode"), "{narrow}");
    assert!(!narrow.contains("⇧tab"), "{narrow}");

    composer.set_task_running(/*running*/ true);
    let (running, _) = render(&composer, /*width*/ 100, /*footer*/ None);
    assert!(running.contains("Plan mode"), "{running}");
    assert!(!running.contains("⇧tab"), "{running}");
    composer.set_task_running(/*running*/ false);

    let footer = TranscriptFooter {
        text: "Search: verify".into(),
        cursor_column: Some(8),
        is_interactive: true,
    };
    let (search, _) = render(&composer, /*width*/ 100, Some(&footer));
    assert!(search.contains("Plan mode"), "{search}");
    assert!(!search.contains("⇧tab"), "{search}");

    // Configured items such as git-branch can have no value outside a repository.
    composer.set_status_line(/*status_line*/ None);
    let (empty_narrow, _) = render(&composer, /*width*/ 24, /*footer*/ None);
    assert_eq!(
        empty_narrow.lines().rev().nth(1).map(str::trim),
        Some("Plan mode")
    );
    let (empty_wide, _) = render(&composer, /*width*/ 100, /*footer*/ None);
    assert!(
        empty_wide.contains("Plan mode (⇧tab to cycle)"),
        "{empty_wide}"
    );

    composer.set_input_enabled(/*enabled*/ false, /*placeholder*/ None);
    let (read_only, _) = render(&composer, /*width*/ 100, /*footer*/ None);
    assert_eq!(
        read_only.lines().rev().nth(1).map(str::trim),
        Some("Plan mode")
    );
    composer.set_input_enabled(/*enabled*/ true, /*placeholder*/ None);
    composer.set_parent_owned_thread();
    let (parent_owned, _) = render(&composer, /*width*/ 100, /*footer*/ None);
    assert_eq!(
        parent_owned.lines().rev().nth(1).map(str::trim),
        Some("Plan mode")
    );
}

// 008-TRIM: fn shortcut_help_stays_above_the_prompt_and_bottom_status（覆盖被裁 shortcut overlay 渲染（008 §1，footer.rs 已裁剪渲染））

#[test]
fn shortcut_help_keeps_bottom_anchored_composer_status_and_cursor_stable() {
    let (width, available_height) = (40, 12);
    let mut composer = composer();
    let mut geometry = Vec::new();
    for event in [
        None,
        Some(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
        Some(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ] {
        if let Some(event) = event {
            composer.handle_key_event(event);
        }
        let options = composer.resolve_render_options(ComposerRenderOptions {
            separate_status_line: true,
            ..ComposerRenderOptions::default()
        });
        let height = composer
            .desired_height_with_options(width, options)
            .min(available_height);
        let area = Rect::new(/*x*/ 3, 80 - height, width, height);
        let ComposerLayout {
            composer: surface,
            footer,
            status,
            ..
        } = composer.layout_with_options(area, options);
        geometry.push((
            surface,
            status,
            footer,
            composer.cursor_pos_with_options(area, options),
        ));
    }
    assert_eq!(geometry, vec![geometry[0]; 3]);
}

// 008-TRIM: fn shortcut_help_keeps_input_and_close_hint_visible_when_height_is_tiny（覆盖被裁 shortcut overlay 渲染（008 §1，footer.rs 已裁剪渲染））

// 008-TRIM: fn shortcut_help_close_row_uses_runtime_binding_and_narrow_fallback（覆盖被裁 shortcut overlay 渲染（008 §1，footer.rs 已裁剪渲染））

#[test]
fn status_survives_hints_feedback_and_queue() {
    let mut composer = composer();
    composer.set_collaboration_mode_indicator(Some(CollaborationModeIndicator::Plan));
    let mut states = Vec::new();
    states.push(render(&composer, /*width*/ 64, /*footer*/ None).0);
    composer.footer.mode = FooterMode::ShortcutOverlay;
    composer.show_footer_flash("Copied selection".into(), Duration::from_secs(/*secs*/ 3));
    states.push(render(&composer, /*width*/ 64, /*footer*/ None).0);
    composer.footer.flash = None;
    composer.footer.mode = FooterMode::ComposerEmpty;
    composer.set_footer_hint_override(Some(vec![("Ctrl+X".into(), "waiting for chord".into())]));
    states.push(render(&composer, /*width*/ 64, /*footer*/ None).0);
    composer.set_footer_hint_override(/*items*/ None);
    composer.set_text_content("Continue the task".into(), Vec::new(), Vec::new());
    composer.set_task_running(/*running*/ true);
    states.push(render(&composer, /*width*/ 64, /*footer*/ None).0);
    // 008-ADAPT: 母本此处循环 File/Skill 弹窗下的状态保持；两类弹窗属被裁功能（008 §1，
    // ActivePopup 仅存 None），该循环随之裁剪，无弹窗状态断言保留。
    for state in &states {
        assert_eq!(state.matches("Plan mode").count(), 1);
        assert!(state.contains("MODEL · ~/project · Context 20% used"));
    }
    insta::assert_snapshot!("persistent_status_and_messages", states.join("\n---\n"));
}

#[test]
fn transcript_search_keeps_status_and_query_cursor() {
    let composer = composer();
    let footer = TranscriptFooter {
        text: vec![
            Line::from("Find: needle"),
            Line::from("Enter next · Esc close"),
        ]
        .into(),
        cursor_column: Some(8),
        is_interactive: true,
    };
    let (text, cursor) = render(&composer, /*width*/ 48, Some(&footer));
    assert_eq!(cursor, Some((10, text.lines().count() as u16 - 2)));
    assert!(text.contains("MODEL · ~/project · Context 20% used"));
}

#[test]
fn input_history_search_uses_the_final_row() {
    let mut composer = composer();
    composer.begin_history_search();
    let (text, cursor) = render(&composer, /*width*/ 48, /*footer*/ None);
    assert_eq!(
        cursor.map(|(_, y)| y),
        Some(text.lines().count() as u16 - 1)
    );
}

// 008-TRIM: fn slash_popup_preserves_footer_and_selection_style（覆盖被裁 command/slash 弹窗（ActivePopup::Command 裁剪，008 §1））

// 008-TRIM: fn activity_shortcut_help_follows_runtime_binding（覆盖被裁 shortcut overlay 渲染（008 §1，footer.rs 已裁剪渲染））
