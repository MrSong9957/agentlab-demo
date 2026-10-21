//! Test-only helpers shared across the TUI crate.

pub(crate) use codex_utils_absolute_path::test_support::PathBufExt;
pub(crate) use codex_utils_absolute_path::test_support::test_path_buf;

pub(crate) fn test_path_display(path: &str) -> String {
    test_path_buf(path).display().to_string()
}

// 008-TRIM: TEST_MODEL_PRESETS（依赖 codex-models-manager 与
// ModelPreset::mark_default_by_picker_visibility，模型目录/skills 无本地测试消费者且 shim
// 未接线，008 §1；无保留测试引用）
// 008-TRIM: session_source_cli/skill_scope_user/skill_scope_repo/from_app_server_wire
// （SessionSource/SkillScope 属 skills/config_panel 会话来源面，无本地事件源且无保留测试
// 引用，008 §1）
// 008-TRIM: box_future/boxed_future! 宏（chatwidget 异步驱动测试助手，chatwidget 不移植，
// 008 §3）
