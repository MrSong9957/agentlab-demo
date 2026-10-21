use super::tests::new_test_composer;
use super::*;
use pretty_assertions::assert_eq;

#[test]
fn parent_owned_thread_allows_bare_navigation_commands() {
    for (command, expected) in [
        ("/agents", SlashCommand::Agents),
        ("/subagents", SlashCommand::MultiAgents),
        ("/side", SlashCommand::Side),
        ("/btw", SlashCommand::Btw),
        ("/diff ", SlashCommand::Diff),
        ("/daemon", SlashCommand::Daemon),
        ("/warnings", SlashCommand::Warnings),
    ] {
        let (mut composer, _rx) = new_test_composer();
        composer.set_parent_owned_thread();
        composer.set_text_content(command.to_string(), Vec::new(), Vec::new());

        assert_eq!(
            composer.handle_submission(/*should_queue*/ false).0,
            InputResult::Command(expected)
        );
    }
}

// 008-TRIM: fn parent_owned_thread_allows_safe_command_selected_from_prefix（覆盖被裁 command/slash 弹窗前缀选中（ActivePopup::Command 裁剪，008 §1））

// 008-TRIM: fn left_navigates_only_from_an_empty_prompt（覆盖被裁 agents overview 导航
//（AppEvent::OpenAgentsOverview 触发），008 §1）

#[test]
fn left_respects_attachments_pastes_and_other_input_surfaces() {
    let setups: [fn(&mut ChatComposer); 4] = [
        |composer| composer.config = ChatComposerConfig::plain_text(),
        |composer| composer.set_remote_image_urls(vec!["https://example.com/image.png".into()]),
        |composer| {
            composer.handle_key_event(KeyCode::Char('x').into());
        },
        |composer| {
            composer.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        },
    ];
    for setup in setups {
        let (mut composer, mut events) = new_test_composer();
        composer.set_agents_navigation_enabled(/*enabled*/ true);
        setup(&mut composer);
        composer.handle_key_event(KeyCode::Left.into());
        // 008-ADAPT: 母本断言 Left 不触发 OpenAgentsOverview；agents overview 属被裁功能
        //（008 §1，AppEvent 无该变体），改为断言不产生任何事件（本地最小 AppEvent 下更强）。
        assert!(
            std::iter::from_fn(|| events.try_recv().ok()).next().is_none()
        );
    }
}

// 008-TRIM: fn agents_navigation_hint_snapshots（覆盖被裁 agents overview 导航提示（008 §1））

#[test]
fn left_respects_editor_and_vim_history_remaps() {
    use codex_config::types::KeybindingSpec;
    use codex_config::types::KeybindingsSpec;
    use codex_config::types::TuiKeymap;

    for vim in [false, true] {
        let (mut composer, mut events) = new_test_composer();
        composer.set_agents_navigation_enabled(/*enabled*/ true);
        composer.insert_str("previous");
        composer.handle_key_event(KeyCode::Enter.into());
        let mut config = TuiKeymap::default();
        if vim {
            config.vim_normal.move_left = Some(KeybindingsSpec::One(KeybindingSpec("h".into())));
            config.vim_normal.move_up = Some(KeybindingsSpec::One(KeybindingSpec("left".into())));
        } else {
            config.editor.move_left = Some(KeybindingsSpec::One(KeybindingSpec("ctrl-b".into())));
            config.editor.move_up = Some(KeybindingsSpec::One(KeybindingSpec("left".into())));
        }
        composer.set_keymap_bindings(&RuntimeKeymap::from_config(&config).unwrap());
        composer.set_vim_enabled(vim);
        assert!(composer.footer_props().key_hints.agents.is_none());
        composer.handle_key_event(KeyCode::Left.into());
        assert_eq!(composer.current_text(), "previous");
        assert!(events.try_recv().is_err());
    }
}
