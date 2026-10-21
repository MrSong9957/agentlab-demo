//! Production mention overlays retain the draft, status footer and editor cursor across scopes.
// 008-TRIM: 本文件整组测试覆盖被裁 mention/skills/file-search/plugins 弹窗（008 §1；
// popup_state.rs ActivePopup 已裁 Command/File/Skill/MentionV2 变体，本地 / 命令原样
// 提交由驱动处理）：
// 008-TRIM: fn mention_scopes_overlay_history_without_moving_composer_or_footer（覆盖被裁 mention 弹窗，008 §1）
// 008-TRIM: fn mention_menu_reserves_height_above_composer_and_owns_cursor_over_interactive_footer（覆盖被裁 mention 弹窗，008 §1）
// 008-TRIM: fn fallback_completions_keep_the_draft_cursor_and_footer_anchored（覆盖被裁 skills/file-search 弹窗，008 §1）
// 008-TRIM: 辅助 composer/render_overlay/buffer_text 仅服务上述测试，一并裁剪。
