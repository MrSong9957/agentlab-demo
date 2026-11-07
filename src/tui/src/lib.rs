//! tchat-tui：openai/codex@97771b1 `codex-rs/tui` 渲染层移植（008 规格）。
//!
//! 本文件是按 008 §2-D6 组合的最小 lib：仅含移植闭包的 mod 声明与必要的
//! re-export（`legacy_core` 在母本 lib.rs 中经 `codex_app_server_client` re-export，
//! 此处保持同一 crate 内路径 `crate::legacy_core` 可达）。
//! 母本 lib.rs 其余 4 千余行 app 层代码不在移植范围（008 §1）。

pub mod app_event;
pub(crate) mod app_event_sender;
pub mod activation_key_guard;
pub mod app_server_approval_conversions;
pub mod assistant_directives;
pub mod bottom_pane;
pub mod chat_driver;
pub mod chatwidget;
pub mod clipboard_paste;
pub mod empty_state_animation;
pub mod footer_hint;
pub mod git_action_directives;
pub mod clipboard_copy;
pub mod copy_input_guard;
pub mod clock_format;
pub mod color;
pub mod custom_terminal;
// 008-TRIM: pub mod daybreak（OpenAI 访问计划横幅，无本地事件源，008 §1）
pub mod diff_model;
pub mod diff_render;
pub mod exec_cell;
pub mod exec_command;
pub mod history_cell;
pub mod inline_visualization;
pub mod insert_history;
pub mod key_hint;
pub mod keymap;
pub mod line_truncation;
pub mod live_wrap;
pub mod local_settings;
pub mod markdown;
pub mod markdown_copy;
pub mod markdown_render;
pub mod markdown_stream;
pub mod markdown_text_merge;
pub mod mention_codec;
pub mod motion;
pub mod notifications;
pub mod pager_overlay;
pub mod render;
pub mod rendered_selection;
pub mod screen_reader;
pub mod session_state;
pub mod system_motion;
pub mod status_indicator_widget;
pub mod streaming;
pub mod slash_command;
pub mod status;
pub mod style;
pub mod table_detect;
pub mod terminal_hyperlinks;
pub mod terminal_palette;
pub mod terminal_title;
pub mod clipboard_html;
pub mod terminal_probe;
pub mod terminal_program_status;
// 008-TRIM: pub mod terminal_size_telemetry（otel 终端尺寸遥测上报，本地无遥测出口，008 §1）
// 008-ADAPT：母本 #[cfg(test)] 门控放宽——A7 安全集成测试（tests/）需 VT100Backend；
// vt100 经 dev-dependencies 对集成测试可见。
pub mod test_backend;
#[cfg(test)] // 008-ADAPT：仅测试消费者（母本亦然），收口以隔离模型目录测试依赖
#[cfg(test)] // 008-ADAPT：对齐母本门控
pub mod test_support;
pub mod text_formatting;
pub mod text_selection;
pub mod thread_transcript;
pub mod task_mentions;
pub mod token_usage;
pub mod tool_output;
pub mod tooltips;
pub mod turn_tip;
pub mod transcript_mode;
pub mod transcript_reflow;
pub mod transcript_view;
pub mod tui;
pub mod ui_consts;
pub mod update_action;
pub mod version;
pub mod vim_search;
pub mod width;
pub mod wrapping;

// 038-CI：A7 安全测试自 tests/ 移入 crate 内联（本 crate 其余测试同款模式）——
// 原集成测试经 Terminal::with_options 查询环境终端尺寸，CI 无 TTY 环境失败；
// crate 内联后可使用显式尺寸的测试构造器，断言本体不变。
#[cfg(test)]
#[path = "security_untrusted_text_tests.rs"]
mod security_untrusted_text_tests;

// 母本 lib.rs:103 `pub(crate) use codex_app_server_client::legacy_core;`
// ——保留文件经 `crate::legacy_core::…` 消费 config 类型（008 §3-J5 shim）。
pub use codex_app_server_client::legacy_core;

// 母本 lib.rs `pub use insert_history::insert_history_lines;`
pub use chat_driver::terminal_supported;
pub use insert_history::insert_history_lines;
