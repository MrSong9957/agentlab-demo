//! Blocked backends leave UI work and teardown independent of clipboard progress.

use super::*;
use crate::clipboard_copy::CopyOutcome;
use pretty_assertions::assert_eq;
use std::time::Duration;

// 008-TRIM: fn blocked_copy_allows_overlay_exit_rejects_backlog_and_wakes_completion（依赖
// 未移植 app/app_server 驱动层：crate::app::test_support、chatwidget 测试桩、
// start_embedded_app_server_for_picker，008 §1；ClipboardWorker 级 backlog/busy 行为由
// 本文件其余 worker 测试覆盖）

#[test]
fn dropping_worker_does_not_wait_for_blocked_backend() {
    let mut worker = ClipboardWorker::default();
    let (started, start_rx) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (finished, finish_rx) = mpsc::channel();
    worker
        .start(
            FrameRequester::test_dummy(),
            move |_, _, _| {
                started.send(()).unwrap();
                released.recv().unwrap();
                finished.send(()).unwrap();
                (Err("unavailable".into()), None)
            },
            || |_| unreachable!("copy-only backend"),
        )
        .unwrap();
    worker
        .copy(
            "text".into(),
            CopyFormat::PlainText,
            FrameRequester::test_dummy(),
        )
        .unwrap();
    start_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 5))
        .unwrap();
    drop(worker);
    release.send(()).unwrap();
    finish_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 5))
        .unwrap();
}

#[tokio::test]
async fn expired_setup_discards_delivery_and_keeps_worker_busy_until_it_returns() {
    // Expiry must also be enforced by the worker when the UI has not polled yet.
    for poll_before_release in [true, false] {
        let mut worker = ClipboardWorker::default();
        let (frames, mut scheduled) = FrameRequester::test_channel();
        let (started, start_rx) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (writes, write_rx) = mpsc::channel();
        worker
            .start(
                frames.clone(),
                move |text, _, setup| {
                    started.send(()).unwrap();
                    released.recv().unwrap();
                    let outcome = setup.begin_delivery().map(|()| {
                        writes.send(text.to_owned()).unwrap();
                        CopyOutcome::Copied(None)
                    });
                    (outcome, None)
                },
                || |_| unreachable!("copy-only backend"),
            )
            .unwrap();
        worker
            .copy("abandoned".into(), CopyFormat::PlainText, frames.clone())
            .unwrap();
        start_rx.recv_timeout(SETUP_TIMEOUT).unwrap();
        scheduled.try_recv().unwrap();
        assert!(worker.poll().is_none());
        // Every poll re-arms the setup deadline after earlier UI draws consume it.
        assert!(scheduled.try_recv().is_ok());
        let setup = &worker.pending.as_ref().unwrap().1;
        *setup.phase.lock().unwrap() = Setup::Pending(Instant::now());
        let failure = (1, Err(SETUP_TIMEOUT_MESSAGE.into()));
        if poll_before_release {
            assert_eq!(worker.poll(), Some(&failure));
        }
        assert!(worker.is_busy());
        assert_eq!(
            worker.copy("retry".into(), CopyFormat::PlainText, frames.clone()),
            Ok(CopyStatus::Busy)
        );
        release.send(()).unwrap();
        tokio::time::timeout(SETUP_TIMEOUT, async {
            while worker.is_busy() {
                worker.poll();
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(worker.poll(), Some(&failure));
        assert!(write_rx.try_recv().is_err());
        assert!(start_rx.try_recv().is_err());
        worker
            .copy("fresh".into(), CopyFormat::PlainText, frames)
            .unwrap();
        start_rx.recv_timeout(SETUP_TIMEOUT).unwrap();
        assert_eq!(worker.poll(), Some(&failure));
        release.send(()).unwrap();
        assert_eq!(write_rx.recv_timeout(SETUP_TIMEOUT).unwrap(), "fresh");
        tokio::time::timeout(SETUP_TIMEOUT, async {
            while worker.is_busy() {
                worker.poll();
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(worker.poll(), Some(&(2, Ok(CopyStatus::Confirmed))));
    }
}

#[test]
fn drop_waits_for_response_channel_disconnect() {
    let (requests, _incoming) = mpsc::channel();
    let (outgoing, responses) = mpsc::sync_channel(/*bound*/ 0);
    let mut worker = ClipboardWorker::default();
    worker.requests = Some(requests);
    worker.responses = Some(responses);
    worker
        .copy(
            "text".into(),
            CopyFormat::PlainText,
            FrameRequester::test_dummy(),
        )
        .unwrap();
    let cleanup = std::thread::spawn(move || {
        // The second rendezvous probes cleanup after receiving a result: shutdown
        // must wait for disconnection, not mistake the first result for handoff.
        for _ in 0..2 {
            outgoing
                .send(Response {
                    result: Ok(CopyStatus::Confirmed),
                    terminal_text: None,
                })
                .unwrap();
        }
    });
    drop(worker);
    cleanup.join().unwrap();
}

#[test]
fn delivery_claim_cannot_be_abandoned() {
    let setup = CopySetup {
        phase: Mutex::new(Setup::Pending(Instant::now() + SETUP_TIMEOUT)),
        frames: FrameRequester::test_dummy(),
        owner: None,
    };
    assert_eq!(setup.begin_delivery(), Ok(()));
    assert!(
        !setup
            .phase
            .lock()
            .unwrap()
            .timed_out(Instant::now() + SETUP_TIMEOUT)
    );
    // Backend fallback checks cannot revoke a delivery that already began.
    assert_eq!(setup.begin_delivery(), Ok(()));
}

// 008-TRIM: fn right_click_paste_uses_normal_input_and_discards_stale_reads（依赖未移植
// app/app_server 驱动层：crate::app::test_support、start_embedded_app_server_for_picker、
// chatwidget::tests 渲染桩，008 §1；读取超时/丢弃迟到结果行为由
// expired_text_read_rejects_backlog_and_discards_late_completion 覆盖）

#[test]
fn expired_text_read_rejects_backlog_and_discards_late_completion() {
    let mut worker = ClipboardWorker::default();
    let (release, released) = mpsc::channel();
    worker
        .start(
            FrameRequester::test_dummy(),
            |_, _, _| unreachable!(),
            move || {
                move |_| {
                    released.recv().unwrap();
                    Ok("late".into())
                }
            },
        )
        .unwrap();
    let request = worker
        .read_text(PasteSource::Clipboard, FrameRequester::test_dummy())
        .unwrap()
        .unwrap();
    worker.pending_read.as_mut().unwrap().deadline = Instant::now();
    worker.poll();
    assert_eq!(
        worker.take_text_result(),
        Some(Err("clipboard read timed out".into()))
    );
    assert!(
        worker
            .read_text(PasteSource::Clipboard, FrameRequester::test_dummy())
            .unwrap()
            .is_none()
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while worker.is_busy() && Instant::now() < deadline {
        worker.poll();
        std::thread::yield_now();
    }
    assert!(!worker.is_busy());
    assert_eq!(worker.take_text_result(), None);
    // A completion harvested by a non-draw event must retain its acceptance deadline.
    worker.read_result = Some((
        Instant::now(),
        Arc::downgrade(&request),
        Ok("completed but no longer current".into()),
    ));
    assert_eq!(
        worker.take_text_result(),
        Some(Err("clipboard read timed out".into()))
    );
}

#[test]
fn latest_primary_is_published_before_read_but_does_not_preempt_explicit_copy() {
    let frames = FrameRequester::test_dummy();
    let (requests, incoming) = mpsc::channel();
    let (outgoing, responses) = mpsc::channel();
    let mut worker = ClipboardWorker::default();
    worker.requests = Some(requests);
    worker.responses = Some(responses);
    let complete = || {
        outgoing
            .send(Response {
                result: Ok(CopyStatus::Confirmed),
                terminal_text: None,
            })
            .unwrap();
    };
    worker
        .copy("original".into(), CopyFormat::PlainText, frames.clone())
        .unwrap();
    assert!(matches!(incoming.try_recv(), Ok(Request::Copy { .. })));
    let (result, _old) = worker.select(
        "old".into(),
        CopyFormat::PlainText,
        CopyDestination::Primary,
        frames.clone(),
    );
    assert_eq!(result, Ok(CopyStatus::Busy));
    let (result, _latest) = worker.select(
        "**visit the café**".into(),
        CopyFormat::Markdown,
        CopyDestination::ClipboardAndPrimary("visit the café".into()),
        frames.clone(),
    );
    assert_eq!(result, Ok(CopyStatus::Busy));
    let _read = worker
        .read_text(PasteSource::Primary, frames.clone())
        .unwrap()
        .unwrap();
    assert!(
        worker
            .read_text(PasteSource::Clipboard, frames.clone())
            .unwrap()
            .is_none()
    );
    assert!(incoming.try_recv().is_err());
    complete();
    assert_eq!(worker.poll(), Some(&(1, Ok(CopyStatus::Confirmed))));
    // This models a completion arriving with a real copy key, before an idle draw.
    assert_eq!(
        worker.copy("explicit".into(), CopyFormat::PlainText, frames.clone()),
        Ok(CopyStatus::Pending(2))
    );
    assert!(
        matches!(incoming.try_recv(), Ok(Request::Copy { text, destination: CopyDestination::Clipboard, .. })
        if text.as_ref() == "explicit")
    );
    worker.advance(frames.clone());
    assert!(incoming.try_recv().is_err());
    complete();
    worker.poll();
    worker.advance(frames.clone());
    assert!(
        matches!(incoming.try_recv(), Ok(Request::Copy { text, format: CopyFormat::PlainText, destination: CopyDestination::Primary, .. })
        if text.as_ref() == "visit the café")
    );
    assert!(incoming.try_recv().is_err());
    complete();
    worker.poll();
    worker.advance(frames);
    let Ok(Request::Read {
        source: PasteSource::Primary,
        response,
        ..
    }) = incoming.try_recv()
    else {
        panic!("one PRIMARY read must follow the latest publication");
    };
    response.send(Ok("visit the café".into())).unwrap();
    worker.poll();
    assert_eq!(worker.take_text_result(), Some(Ok("visit the café".into())));
    assert!(incoming.try_recv().is_err());
    drop(outgoing);
}

#[test]
fn cancelling_owners_after_poll_drops_deferred_work_without_releasing_worker_early() {
    let frames = FrameRequester::test_dummy();
    let (requests, incoming) = mpsc::channel();
    let (outgoing, responses) = mpsc::channel();
    let mut worker = ClipboardWorker::default();
    worker.requests = Some(requests);
    worker.responses = Some(responses);
    worker
        .copy("active".into(), CopyFormat::PlainText, frames.clone())
        .unwrap();
    assert!(matches!(incoming.try_recv(), Ok(Request::Copy { .. })));
    let (_, selection) = worker.select(
        "cancelled".into(),
        CopyFormat::PlainText,
        CopyDestination::Primary,
        frames.clone(),
    );
    let read = worker
        .read_text(PasteSource::Primary, frames.clone())
        .unwrap()
        .unwrap();
    worker.poll();
    drop(selection);
    drop(read);
    worker.advance(frames.clone());
    assert!(worker.is_busy());
    assert!(incoming.try_recv().is_err());
    outgoing
        .send(Response {
            result: Ok(CopyStatus::Confirmed),
            terminal_text: None,
        })
        .unwrap();
    worker.poll();
    worker.advance(frames.clone());
    assert!(!worker.is_busy());
    assert!(worker.take_text_result().is_none());
    assert!(incoming.try_recv().is_err());

    let read = worker
        .read_text(PasteSource::Primary, frames.clone())
        .unwrap()
        .unwrap();
    let Ok(Request::Read { response, .. }) = incoming.try_recv() else {
        panic!("an available read must start immediately");
    };
    response
        .send(Ok("completed before validation".into()))
        .unwrap();
    worker.poll();
    drop(read);
    assert!(worker.take_text_result().is_none());
    let (_, selection) = worker.select(
        "cancelled during setup".into(),
        CopyFormat::PlainText,
        CopyDestination::Primary,
        frames,
    );
    let Ok(Request::Copy { setup, .. }) = incoming.try_recv() else {
        panic!("selection must reach the worker");
    };
    drop(selection);
    assert_eq!(
        setup.begin_delivery(),
        Err("selection ended before clipboard delivery".into())
    );
    drop(outgoing);
}
