use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn seeds_only_missing_preferences_and_never_probes_again() -> anyhow::Result<()> {
    for (input, detected, expected) in [
        (
            "",
            true,
            "[tui]\nanimations = false\nscreen_reader_detection_done = true\n",
        ),
        ("", false, "[tui]\nscreen_reader_detection_done = true\n"),
        (
            "[tui]\nanimations = true\n",
            true,
            "[tui]\nanimations = true\nscreen_reader_detection_done = true\n",
        ),
        (
            "[tui]\nanimations = false\n",
            true,
            "[tui]\nanimations = false\nscreen_reader_detection_done = true\n",
        ),
        (
            "[tui]\nscreen_reader_detection_done = false\n",
            true,
            "[tui]\nscreen_reader_detection_done = false\n",
        ),
        (
            "[tui]\nscreen_reader_detection_done = true\n",
            true,
            "[tui]\nscreen_reader_detection_done = true\n",
        ),
    ] {
        let home = tempfile::tempdir()?;
        let path = home.path().join("config.toml");
        std::fs::write(&path, input)?;
        initialize_file(
            &path,
            /*animations_configured*/ false,
            std::future::ready(detected),
        )
        .await?;
        initialize_file(
            &path,
            /*animations_configured*/ false,
            async { panic!("repeated detection") },
        )
        .await?;
        assert_eq!(std::fs::read_to_string(&path)?, expected);
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn timeout_is_recorded_without_changing_animations() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let path = home.path().join("config.toml");
    let started = tokio::time::Instant::now();
    initialize_file(
        &path,
        /*animations_configured*/ false,
        std::future::pending(),
    )
    .await?;
    assert_eq!(tokio::time::Instant::now() - started, DETECTION_TIMEOUT);
    assert_eq!(
        std::fs::read_to_string(path)?,
        "[tui]\nscreen_reader_detection_done = true\n"
    );
    Ok(())
}

#[tokio::test]
async fn preserves_preferences_from_other_layers_and_edits_during_detection() -> anyhow::Result<()>
{
    let home = tempfile::tempdir()?;
    let path = home.path().join("config.toml");
    initialize_file(
        &path,
        /*animations_configured*/ true,
        std::future::ready(true),
    )
    .await?;
    assert_eq!(
        std::fs::read_to_string(&path)?,
        "[tui]\nscreen_reader_detection_done = true\n"
    );
    std::fs::write(&path, "")?;
    initialize_file(&path, /*animations_configured*/ false, async {
        std::fs::write(
            &path,
            "# My preferences\n[tui]\nanimations = true # keep this\n",
        )
        .unwrap();
        true
    })
    .await?;
    assert_eq!(
        std::fs::read_to_string(path)?,
        "# My preferences\n[tui]\nanimations = true # keep this\nscreen_reader_detection_done = true\n"
    );
    Ok(())
}

#[tokio::test]
async fn persisted_default_loads_and_renders_without_animation() -> anyhow::Result<()> {
    use crate::app_event_sender::AppEventSender;
    use crate::legacy_core::config::Config;
    use crate::render::renderable::Renderable;
    use crate::status_indicator_widget::StatusIndicatorWidget;
    use crate::status_indicator_widget::StatusTimer;
    use crate::tui::FrameRequester;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // 008-ADAPT: 母本从临时 config.toml 装载 animations=false；本地无配置装载层
    // （codex-config shim 无 loader），直接以解析结果构造 Config，渲染断言保持不变。
    let config = Config {
        animations: false,
        ..Config::default()
    };
    assert!(!config.animations);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (frame_requester, mut frames) = FrameRequester::test_channel();
    let mut status = StatusIndicatorWidget::new(
        AppEventSender::new(tx),
        frame_requester,
        config.animations,
        Default::default(),
    );
    status.update_header("Reading the terminal output".into());
    let mut timer = StatusTimer::default();
    timer.pause_at(std::time::Instant::now());
    let mut terminal = Terminal::new(TestBackend::new(/*width*/ 60, /*height*/ 1))?;
    terminal.draw(|frame| {
        status
            .with_timer(&timer)
            .render(frame.area(), frame.buffer_mut())
    })?;
    assert_eq!(
        frames.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );
    insta::assert_snapshot!(terminal.backend());
    Ok(())
}

// 008-TRIM: fn temporary_cli_override_does_not_suppress_the_saved_default（覆盖未移植配置
// 装载/CLI 覆盖层叠栈：ConfigBuilder.cli_overrides＋initialize_with_probe 经真实层栈写回，
// shim ConfigLayerStack 无用户层（get_user_config_file 恒 None，探测直接跳过），008 §1）

// 008-TRIM: fn detected_preference_survives_failed_persistence（同上依赖真实层栈的用户
// 配置文件路径；shim 桩栈下 initialize_with_probe 恒返回 (Animated, Ok(()))，断言无法
// 承载，008 §1；探测失败路径的错误传播仍由 initialize_file 系测试覆盖）

#[tokio::test]
async fn malformed_edit_during_detection_does_not_expose_config_contents() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let path = home.path().join("config.toml");
    let error = initialize_file(&path, /*animations_configured*/ false, async {
        std::fs::write(&path, "token = secret-token").unwrap();
        true
    })
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "invalid TOML in user config");
    Ok(())
}
