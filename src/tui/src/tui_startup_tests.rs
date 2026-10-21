//! Startup input races exercised against the real TUI event loop.
// 008-TRIM: fn startup_preserves_typeahead_and_discards_buffered_action_key_after_first_draw
//（依赖未移植 startup_draft 启动管线：StartupDraft/TerminalRestoreGuard 及子测试
// startup_typeahead_pty_child，008 §1；终端输入排空/边界原语本身保留于 tui.rs
// （discard_pending_input_before_interactive_screen 等），覆盖缺口随 P3 驱动落地补回）
// 008-TRIM: fn startup_typeahead_pty_child（startup_draft 启动管线的 PTY 子测试，
// 随 startup_draft 未移植一并裁剪，008 §1）
// 008-TRIM: 辅助 wait_for_file 与 STARTUP_INPUT_CHILD_TEST 常量仅服务上述测试，一并裁剪。
