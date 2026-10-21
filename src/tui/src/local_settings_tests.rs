use super::*;
// 008-ADAPT: 母本经 ConfigBuilder/LoaderOverrides 从临时 config.toml 装载 Config；
// 本地未移植配置装载层（codex-config shim 无 loader），保留测试改为直接构造 shim Config
// 结构体（公共字段），LocalSettings 行为断言保持不变；装载/层叠/CLI 覆盖优先级相关
// 断言随依赖未移植而裁剪（见逐测试标记）。
use crate::legacy_core::config::Config;
use codex_config::types::RightClickPaste;
use codex_terminal_detection::Multiplexer;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn launch_screen_mode_survives_configuration_reload() -> anyhow::Result<()> {
    use crate::transcript_mode::TranscriptMode;
    use codex_config::types::AltScreenMode;

    let home = tempfile::tempdir()?;
    let mut config = Config {
        codex_home: AbsolutePathBuf::from_absolute_path(home.path())?,
        ..Config::default()
    };
    config.tui_fullscreen_transcript = true;
    config.tui_copy_on_select = CopyOnSelect::Always;
    config.tui_copy_on_select_clear_selection = true;
    config.tui_right_click_paste = RightClickPaste::On;
    config.tui_alternate_screen = AltScreenMode::Auto;
    config.animations = true;
    config.tui_auto_recap = true;
    config.tui_effects.starfield = true;
    config.disable_paste_burst = false;
    config.show_tooltips = true;

    for (alternate_screen, owned, expected_mode, expected_alt) in [
        (true, true, TranscriptMode::Owned, AltScreenMode::Auto),
        (true, false, TranscriptMode::Terminal, AltScreenMode::Auto),
        (false, true, TranscriptMode::Terminal, AltScreenMode::Never),
    ] {
        let mut tui = crate::tui::test_support::make_test_tui()?;
        tui.set_alt_screen_enabled(alternate_screen);
        tui.set_owned_screen(owned)?;
        let local = LocalSettings::for_tui(&config, &tui);
        assert_eq!(
            (local.transcript_mode, local.tui.alternate_screen),
            (expected_mode, expected_alt),
        );

        let mut reloaded_config = config.clone();
        reloaded_config.tui_fullscreen_transcript = false;
        reloaded_config.tui_copy_on_select = CopyOnSelect::Never;
        reloaded_config.tui_copy_on_select_clear_selection = false;
        reloaded_config.tui_right_click_paste = RightClickPaste::Off;
        reloaded_config.tui_alternate_screen = AltScreenMode::Never;
        reloaded_config.tui_theme = Some("nord".into());
        reloaded_config.animations = false;
        reloaded_config.tui_auto_recap = false;
        reloaded_config.tui_effects.starfield = false;
        reloaded_config.disable_paste_burst = true;
        reloaded_config.show_tooltips = false;
        let mut expected_reload = LocalSettings::from(&reloaded_config);
        expected_reload.transcript_mode = expected_mode;
        expected_reload.tui.alternate_screen = expected_alt;
        expected_reload.tui.animations = local.tui.animations;
        expected_reload.tui.auto_recap = local.tui.auto_recap;
        expected_reload.tui.effects.starfield = local.tui.effects.starfield;
        expected_reload.tui.disable_paste_burst = local.tui.disable_paste_burst;
        expected_reload.tui.show_tooltips = local.tui.show_tooltips;
        assert_eq!(local.reloaded(&reloaded_config), expected_reload);

        let mut expected_launch = LocalSettings::from(&reloaded_config);
        expected_launch.transcript_mode = expected_mode;
        expected_launch.tui.alternate_screen = expected_alt;
        assert_eq!(
            LocalSettings::for_tui(&reloaded_config, &tui),
            expected_launch
        );
        tui.set_owned_screen(/*owned*/ false)?;
    }
    Ok(())
}

#[tokio::test]
async fn system_motion_suppresses_animations_without_changing_saved_preferences()
-> anyhow::Result<()> {
    use crate::motion::MotionMode;

    for configured in [true, false] {
        let home = tempfile::tempdir()?;
        let config = Config {
            codex_home: AbsolutePathBuf::from_absolute_path(home.path())?,
            animations: configured,
            ..Config::default()
        };
        let animated = LocalSettings::with_accessibility_preferences(
            &config,
            MotionMode::Animated,
            MotionMode::Animated,
        );
        let reduced = LocalSettings::with_accessibility_preferences(
            &config,
            MotionMode::Reduced,
            MotionMode::Animated,
        );
        let mut expected = animated.clone();
        expected.tui.animations = false;
        assert_eq!(reduced, expected);
        assert_eq!(animated.tui.animations, configured);
        assert_eq!(config.animations, configured);
    }
    Ok(())
}

// 008-TRIM: fn local_load_preserves_defaults_and_resolved_overrides（覆盖未移植配置装载/
// 层叠栈/CLI 覆盖解析：load_config_toml_with_layer_stack、ConfigLoadOptions、LoaderOverrides，
// 008 §1；shim ConfigLayerStack 为单层桩，无装载语义可验）

#[tokio::test]
async fn copy_on_select_respects_terminal_defaults_and_config_overrides() -> anyhow::Result<()> {
    for (_configured, _launch_override, expected) in [
        (None, None, CopyOnSelect::Auto),
        (Some("auto"), None, CopyOnSelect::Auto),
        (Some("always"), None, CopyOnSelect::Always),
        (Some("never"), None, CopyOnSelect::Never),
        (Some("never"), Some("always"), CopyOnSelect::Always),
        (Some("always"), Some("never"), CopyOnSelect::Never),
        (Some("always"), Some("auto"), CopyOnSelect::Auto),
        (Some("never"), Some("auto"), CopyOnSelect::Auto),
    ] {
        let home = tempfile::tempdir()?;
        // 008-ADAPT: 母本经 config.toml＋CLI 覆盖解析出期望值；本地无装载层，直接以解析
        // 后期望值构造 Config（configured/launch_override → expected 的优先级本身属装载域）。
        let config = Config {
            codex_home: AbsolutePathBuf::from_absolute_path(home.path())?,
            tui_copy_on_select: expected,
            ..Config::default()
        };
        let local = LocalSettings::from(&config);
        assert_eq!(
            (config.tui_copy_on_select, local.tui.copy_on_select),
            (expected, expected),
        );
        for (name, version, multiplexer, default_enabled) in [
            (TerminalName::Iterm2, None, None, true),
            (TerminalName::AppleTerminal, None, None, true),
            (TerminalName::Ghostty, Some("1.2.0"), None, false),
            (TerminalName::Ghostty, Some("1.3.0"), None, false),
            (TerminalName::Ghostty, Some("1.1.3"), None, true),
            (TerminalName::Ghostty, Some("1.2.0-dev"), None, true),
            (TerminalName::Ghostty, Some("invalid"), None, true),
            (TerminalName::Ghostty, None, None, true),
            (TerminalName::Kitty, None, None, !cfg!(target_os = "macos")),
            (TerminalName::WindowsTerminal, None, None, false),
            (
                TerminalName::VsCode,
                None,
                None,
                !cfg!(target_os = "windows"),
            ),
            (TerminalName::Alacritty, None, None, true),
            (TerminalName::GnomeTerminal, None, None, true),
            (TerminalName::Konsole, None, None, true),
            (TerminalName::Vte, None, None, true),
            (TerminalName::WarpTerminal, None, None, true),
            (TerminalName::WezTerm, None, None, true),
            (TerminalName::Dumb, None, None, true),
            (TerminalName::Unknown, None, None, true),
            (
                TerminalName::Ghostty,
                Some("1.3.0"),
                Some(Multiplexer::Tmux { version: None }),
                true,
            ),
            (
                TerminalName::Kitty,
                None,
                Some(Multiplexer::Zellij { version: None }),
                true,
            ),
            (
                TerminalName::WindowsTerminal,
                None,
                Some(Multiplexer::Tmux { version: None }),
                true,
            ),
        ] {
            let terminal = TerminalInfo {
                name,
                multiplexer,
                term_program: None,
                version: version.map(str::to_owned),
                term: None,
            };
            assert_eq!(
                local.copy_on_select(&terminal),
                match expected {
                    CopyOnSelect::Auto => default_enabled,
                    CopyOnSelect::Always => true,
                    CopyOnSelect::Never => false,
                },
                "terminal={terminal:?}, override={expected:?}",
            );
        }
    }
    Ok(())
}

// 008-TRIM: fn local_writes_preserve_selected_user_file_and_home_destinations（覆盖未移植
// 配置写入/编辑层：config::edit::ConfigEditsBuilder、syntax_theme_edit、SessionPickerViewMode，
// 008 §1；shim ConfigLayerStack 无用户层，user_config_path 恒为 codex_home 回退值）

// 008-TRIM: fn screen_reader_default_yields_to_preferences_on_reload（覆盖未移植配置层叠栈：
// "层内已配置 animations 优先于 screen reader Reduced 默认"的语义需真实层栈，shim
// ConfigLayerStack 为单层空桩（effective_config 恒空表），无法承载该断言，008 §1）
