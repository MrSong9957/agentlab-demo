use super::*;
use std::sync::mpsc;

fn overlay() -> (
    RequestUserInputOverlay,
    mpsc::Receiver<Result<ToolRequestUserInputResponse, RequestUserInputError>>,
) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (reply, rx) = mpsc::channel();
    let question: ToolRequestUserInputQuestion = serde_json::from_value(serde_json::json!({
        "id":" raw-id ","header":"选择","question":"方向？","isOther":true,
        "options":[{"label":"保留","description":"兼容"}]
    }))
    .unwrap();
    (
        RequestUserInputOverlay::new_with_keymap(
            vec![question],
            UserInputReply::new(reply),
            Arc::new(AtomicBool::new(true)),
            str::to_owned,
            AppEventSender::new(tx),
            true,
            RuntimeKeymap::defaults(),
        ),
        rx,
    )
}

#[test]
fn selection_and_elapsed_time_are_not_submission_and_reply_is_once() {
    let (mut view, rx) = overlay();
    view.pre_draw_tick(Instant::now() + Duration::from_secs(86_400));
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let response = rx.recv().unwrap().unwrap();
    assert_eq!(response.answers[" raw-id "].answers, ["保留"]);
    view.finish_user_input(RequestUserInputError::Cancelled);
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn pasted_note_preserves_control_bytes_as_task_data() {
    let (mut view, rx) = overlay();
    let note = "中文\x1b]52;c;data\x07补充";
    view.handle_paste(note.into());
    view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let response = rx.recv().unwrap().unwrap();
    assert_eq!(
        response.answers[" raw-id "].answers,
        ["保留".to_string(), format!("user_note: {note}")]
    );
}

#[test]
fn view_close_and_stale_request_end_only_their_own_channel() {
    let (view, rx) = overlay();
    drop(view);
    assert_eq!(rx.recv().unwrap(), Err(RequestUserInputError::Closed));
    let (mut stale, rx) = overlay();
    stale.live.store(false, Ordering::SeqCst);
    assert!(stale.pre_draw_tick(Instant::now()));
    assert_eq!(rx.recv().unwrap(), Err(RequestUserInputError::Closed));
    let (mut next, rx) = overlay();
    stale.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    next.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(rx.recv().unwrap().is_ok());
}

#[test]
fn not_yet_displayed_request_drops_as_host_exit() {
    let (tx, rx) = mpsc::channel();
    drop(UserInputReply::new(tx));
    assert_eq!(rx.recv().unwrap(), Err(RequestUserInputError::HostExited));
}

#[test]
fn late_answer_from_old_view_cannot_answer_the_next_question() {
    let (mut old, old_rx) = overlay();
    drop(old_rx); // 已结束宿主仍可能有迟到的 UI 事件
    let (mut next, next_rx) = overlay();
    old.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        matches!(next_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "旧回答不能进入新请求"
    );
    next.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        next_rx.recv().unwrap().unwrap().answers[" raw-id "].answers,
        ["保留"]
    );
}

#[test]
fn note_cursor_follows_visible_chinese_text() {
    use ratatui::{buffer::Buffer, layout::Rect};
    let (mut view, _rx) = overlay();
    view.handle_paste("中文补充".into());
    let area = Rect::new(0, 0, 80, 20);
    let notes = view
        .layout_sections(crate::bottom_pane::selection_popup_common::menu_surface_inset(area))
        .notes_area;
    let mut buffer = Buffer::empty(area);
    view.render(area, &mut buffer);
    assert_eq!(buffer[(notes.x, notes.y)].symbol(), "中");
    assert_eq!(
        view.cursor_pos_impl(area),
        Some((notes.x + 8, notes.y)),
        "光标必须跟随实际显示文字"
    );
}
