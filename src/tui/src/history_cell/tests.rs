//! Coverage for history-cell rendering, wrapping, and transcript behavior.

use super::*;
use crate::exec_cell::CommandOutput;
use crate::exec_cell::ExecCall;
use crate::exec_cell::ExecCell;
use crate::legacy_core::config::Config;
use crate::line_truncation::line_width;
use crate::render::highlight::MAX_HIGHLIGHT_LINE_BYTES;
use crate::session_state::ThreadSessionState;
use crate::wrapping::word_wrap_lines;
use codex_app_server_protocol::AskForApproval;
use codex_otel::RuntimeMetricTotals;
use codex_otel::RuntimeMetricsSummary;
use codex_protocol::ThreadId;
use codex_protocol::parse_command::ParsedCommand;
use codex_utils_absolute_path::AbsolutePathBuf;
use dirs::home_dir;
use pretty_assertions::assert_eq;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::path::PathBuf;

use codex_app_server_protocol::CommandExecutionSource as ExecCommandSource;
// 008-TRIM: ConfigBuilder/McpAuthStatus/McpServerConfig/PlanType/UnexpectedResponseError/
// http::StatusCode/json!/HashMap/CallToolResult/Tool/rmcp ContentBlock/SMALL_PNG_BASE64
// 导入（RemoteConnection 版本通知、MCP、计划、远端错误类型与协议构造面本地缺席，008 §1）

// 008-TRIM: fn connected_server_version_notice_snapshot（覆盖被裁 RemoteConnection 版本
// 通知（AppServerTarget/pending_server_version_notice/new_server_version_warning 本地裁剪），
// 008 §1）
// 008-TRIM: fn local_daemon_version_notice_snapshot（同上，LocalDaemon 目标变体）

async fn test_config() -> Config {
    // 008-ADAPT：shim Config 无 ConfigBuilder（母本异步装配管线未 vendor）；
    // 以 Default 构造并显式设 codex_home 为临时目录，保持与母本测试助手一致的目录隔离
    let mut config = Config::default();
    config.codex_home = AbsolutePathBuf::from_absolute_path(std::env::temp_dir())
        .expect("absolute temp dir");
    config
}

fn test_cwd() -> PathBuf {
    // These tests only need a stable absolute cwd; using temp_dir() avoids baking Unix- or
    // Windows-specific root semantics into the fixtures.
    std::env::temp_dir()
}

#[test]
fn streaming_agent_tail_blank_line_uses_one_viewport_row() {
    let cell = StreamingAgentTailCell::new(
        vec![
            HyperlinkLine::from("first"),
            HyperlinkLine::from(" "),
            HyperlinkLine::from("second"),
        ],
        /*is_first_line*/ false,
    );

    let rendered = cell.display_hyperlink_lines(/*width*/ 80);
    let source = rendered[1].source.as_ref().expect("blank row source");
    assert_eq!((source.prefix_bytes, source.range.clone()), (0, 0..0));
    assert_eq!(source.text.as_ref(), " ");
    let lines = visible_lines(rendered);
    insta::assert_snapshot!(render_lines(&lines).join("\n"), @"  first

  second");
    assert_eq!(cell.desired_height(/*width*/ 80), 3);
}

// 008-TRIM: fn stdio_server_config/streamable_http_server_config/string_map_to_toml_value
// （MCP 服务器配置构造助手，MCP 无本地事件源，008 §1）

fn render_lines(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn render_transcript(cell: &dyn HistoryCell) -> Vec<String> {
    render_lines(&cell.transcript_lines(u16::MAX))
}

fn assert_unstyled_lines(lines: &[Line<'static>]) {
    for line in lines {
        assert_eq!(line.style, Style::default());
        for span in &line.spans {
            assert_eq!(span.style, Style::default());
        }
    }
}

// 008-TRIM: fn image_block/text_block/resource_link_block（rmcp ContentBlock 构造助手，
// MCP 无本地事件源，008 §1）

#[test]
fn raw_lines_from_source_preserves_explicit_blank_lines() {
    let lines = raw_lines_from_source("alpha\n\nbeta\n");

    assert_eq!(
        render_lines(&lines),
        vec!["alpha".to_string(), String::new(), "beta".to_string()]
    );
    assert_unstyled_lines(&lines);
}

#[test]
fn raw_lines_from_source_preserves_trailing_blank_but_not_trailing_newline() {
    assert_eq!(
        render_lines(&raw_lines_from_source("alpha\n\n")),
        vec!["alpha".to_string(), String::new()]
    );
    assert_eq!(raw_lines_from_source(""), Vec::<Line<'static>>::new());
}

#[test]
fn source_backed_cells_render_raw_source_without_prefix_or_style() {
    let user = new_user_prompt(
        "hello\n\nworld\n".to_string(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let assistant = AgentMarkdownCell::new(
        "- item\n\n| A | B |\n| - | - |\n| x | y |\n".to_string(),
        &test_cwd(),
    );
    let reasoning = ReasoningSummaryCell::new(
        "thinking".to_string(),
        "first thought\n\nsecond thought".to_string(),
        &test_cwd(),
        /*transcript_only*/ false,
    );
    // 008-TRIM: new_proposed_plan 段与其 raw 断言（plans 无本地事件源，008 §1）

    let user_lines = user.raw_lines();
    assert_eq!(
        render_lines(&user_lines),
        vec!["hello".to_string(), String::new(), "world".to_string()]
    );
    assert_unstyled_lines(&user_lines);

    let assistant_lines = assistant.raw_lines();
    assert_eq!(
        render_lines(&assistant_lines),
        vec![
            "- item".to_string(),
            String::new(),
            "| A | B |".to_string(),
            "| - | - |".to_string(),
            "| x | y |".to_string(),
        ]
    );
    assert_unstyled_lines(&assistant_lines);

    let reasoning_lines = reasoning.raw_lines();
    assert_eq!(
        render_lines(&reasoning_lines),
        vec![
            "first thought".to_string(),
            String::new(),
            "second thought".to_string(),
        ]
    );
    assert_unstyled_lines(&reasoning_lines);
}

// 008-TRIM: fn proposed_plan_cell_renders_markdown_table（覆盖被裁 new_proposed_plan
// 计划 cell，plans 无本地事件源，008 §1）
// 008-TRIM: fn proposed_plan_cell_preserves_wrapped_table_web_links（同上）
// 008-TRIM: fn empty_mcp_output_preserves_docs_hyperlink（覆盖被裁 empty_mcp_output
// MCP 工具清单 cell，008 §1）
// 008-TRIM: fn proposed_plan_cell_unwraps_markdown_fenced_table（同 new_proposed_plan 裁剪）
// 008-TRIM: fn structured_tool_cell_renders_raw_plain_text_without_prefix_or_style
// （覆盖被裁 new_active_mcp_tool_call/McpInvocation raw 模式，008 §1）
// 008-TRIM: fn raw_mode_toggle_transcript_snapshot（user/agent/mcp 三 cell 混合 Raw 模式
// 快照；命名快照 raw_mode_toggle_transcript 内嵌 MCP 输出，部分保留必然偏离黄金快照，008 §1）

#[test]
fn composite_cell_preserves_child_web_links() {
    let destination = "https://chatgpt.com/settings/usage";
    let cell = CompositeHistoryCell::new(vec![
        Box::new(PlainHistoryCell::new(vec![Line::from("/status")])),
        Box::new(WebHyperlinkHistoryCell::new(vec![Line::from(destination)])),
    ]);

    let lines = cell.display_hyperlink_lines(/*width*/ 80);

    assert_eq!(
        lines[2].hyperlinks,
        vec![crate::terminal_hyperlinks::TerminalHyperlink::web(
            /*columns*/ 0..destination.len(),
            destination.to_string(),
        )]
    );
}

#[test]
fn image_generation_call_renders_saved_path() {
    let saved_path = test_path_buf("/tmp/generated-image.png").abs();
    let expected_saved_path = format!(
        "  └ Saved to: {}",
        Url::from_file_path(saved_path.as_path()).expect("test path should convert to file URL")
    );
    let cell = new_image_generation_call(
        "call-image-generation".to_string(),
        "completed",
        Some("A tiny blue square".to_string()),
        Some(saved_path),
    );

    assert_eq!(
        render_lines(&cell.display_lines(/*width*/ 80)),
        vec![
            "• Generated Image:".to_string(),
            "  └ A tiny blue square".to_string(),
            expected_saved_path,
        ],
    );
}

fn session_configured_event(model: &str) -> ThreadSessionState {
    ThreadSessionState {
        daybreak_enabled: false,
        // 008-ADAPT：windows_sandbox_host/approvals_reviewer 字段本地裁剪（008 §1），省略
        // 008-ADAPT：shim ThreadId::new 接受字符串（母本无参生成 Uuid）
        thread_id: ThreadId::new("01912345-1234-7123-8123-123456789abc"),
        forked_from_id: None,
        fork_parent_title: None,
        thread_name: None,
        model: model.to_string(),
        model_provider_id: "test-provider".to_string(),
        service_tier: None,
        approval_policy: AskForApproval::Never,
        // 008-ADAPT：母本 PermissionProfile::read_only()（Restricted/Restricted 只读档）在 shim
        // 无构造器，按等值字面构造以保持非 yolo 语义与黄金快照一致
        permission_profile: PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Restricted {
                entries: Vec::new(),
                glob_scan_max_depth: None,
            },
            network: NetworkSandboxPolicy::Restricted,
        },
        active_permission_profile: None,
        cwd: test_path_buf("/tmp/project").abs(),
        runtime_workspace_roots: Vec::new(),
        instruction_source_paths: Vec::new(),
        reasoning_effort: None,
        collaboration_mode: None,
        message_history: None,
        network_proxy: None,
        rollout_path: Some(PathBuf::new()),
    }
}

#[test]
fn unified_exec_interaction_cell_retains_detailed_input() {
    let input = (1..=16).map(|line| format!("line {line}\n")).collect();
    let cell = new_unified_exec_interaction(Some("cat".to_string()), input);
    let lines = render_lines(&cell.transcript_lines(/*width*/ 80));
    insta::assert_snapshot!(lines.join("\n"), @"
    ↳ Interacted with background terminal · cat
      └ line 1
        line 2
        line 3
        line 4
        line 5
        line 6
        line 7
        line 8
        line 9
        line 10
        line 11
        line 12
        line 13
        line 14
        line 15
        line 16
    ");
}

#[test]
fn unified_exec_interaction_cell_retains_detailed_wait() {
    let cell = new_unified_exec_interaction(/*command_display*/ None, String::new());
    let lines = render_transcript(&cell);
    assert_eq!(lines, vec!["• Waited for background terminal"]);
}

#[test]
fn final_message_separator_preserves_runtime_metrics_for_short_turns() {
    let summary = RuntimeMetricsSummary {
        tool_calls: RuntimeMetricTotals {
            count: 3,
            duration_ms: 2_450,
        },
        api_calls: RuntimeMetricTotals {
            count: 2,
            duration_ms: 1_200,
        },
        streaming_events: RuntimeMetricTotals {
            count: 6,
            duration_ms: 900,
        },
        websocket_calls: RuntimeMetricTotals {
            count: 1,
            duration_ms: 700,
        },
        websocket_events: RuntimeMetricTotals {
            count: 4,
            duration_ms: 1_200,
        },
        responses_api_overhead_ms: 650,
        responses_api_inference_time_ms: 1_940,
        responses_api_engine_iapi_ttft_ms: 410,
        responses_api_engine_service_ttft_ms: 460,
        responses_api_engine_iapi_tbt_ms: 1_180.0,
        responses_api_engine_service_tbt_ms: 1_240.0,
        turn_ttft_ms: 0,
        turn_ttfm_ms: 0,
    };
    let cell = FinalMessageSeparator::new(Some(12), Some(summary));
    let rendered = render_lines(&cell.display_lines(/*width*/ 600));

    assert_eq!(rendered.len(), 1);
    assert!(rendered[0].starts_with("  Worked for 12s • Local tools:"));
    assert!(rendered[0].contains("Local tools: 3 calls (2.5s)"));
    assert!(rendered[0].contains("Inference: 2 calls (1.2s)"));
    assert!(rendered[0].contains("WebSocket: 1 events send (700ms)"));
    assert!(rendered[0].contains("Streams: 6 events (900ms)"));
    assert!(rendered[0].contains("4 events received (1.2s)"));
    assert!(rendered[0].contains("Responses API overhead: 650ms"));
    assert!(rendered[0].contains("Responses API inference: 1.9s"));
    assert!(rendered[0].contains("TTFT: 410ms (iapi) 460ms (service)"));
    assert!(rendered[0].contains("TBT: 1.2s (iapi) 1.2s (service)"));
}

#[test]
fn runtime_metrics_label_rounds_fractional_tbt_milliseconds() {
    let summary = RuntimeMetricsSummary {
        responses_api_engine_iapi_tbt_ms: 2.450638,
        responses_api_engine_service_tbt_ms: 5.267279,
        ..RuntimeMetricsSummary::default()
    };

    insta::assert_snapshot!(runtime_metrics_label(summary).expect("TBT label"), @"TBT: 2ms (iapi) 5ms (service)");
}

#[test]
fn final_message_separator_includes_worked_label_after_one_minute() {
    let cell = FinalMessageSeparator::new(Some(61), /*runtime_metrics*/ None);
    let rendered = render_lines(&cell.display_lines(/*width*/ 200));

    assert_eq!(rendered.len(), 1);
    assert!(rendered[0].contains("Worked for"));
}

#[test]
fn ps_output_empty_snapshot() {
    let cell = new_unified_exec_processes_output(Vec::new());
    let rendered = render_lines(&cell.display_lines(/*width*/ 60)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[tokio::test]
async fn session_info_uses_availability_nux_tooltip_override() {
    let config = test_config().await;
    let cell = new_session_info(
        &config,
        &crate::local_settings::LocalSettings::from(&config),
        "gpt-5",
        "gpt-5",
        &session_configured_event("gpt-5"),
        /*is_first_event*/ false,
        Some("Model just became available".to_string()),
        None, /* 008-ADAPT：shim PlanType 空桩仅可构造 None；tooltip_override 恒 Some 优先，不影响断言 */
    );

    let rendered = render_transcript(&cell).join("\n");
    assert!(rendered.contains("Model just became available"));
}

#[tokio::test]
#[cfg_attr(
    target_os = "windows",
    ignore = "snapshot path rendering differs on Windows"
)]
async fn session_info_availability_nux_tooltip_snapshot() {
    let mut config = test_config().await;
    config.cwd = test_path_buf("/tmp/project").abs();
    let cell = new_session_info(
        &config,
        &crate::local_settings::LocalSettings::from(&config),
        "gpt-5",
        "gpt-5",
        &session_configured_event("gpt-5"),
        /*is_first_event*/ false,
        Some("Model just became available".to_string()),
        None, /* 008-ADAPT：shim PlanType 空桩仅可构造 None；tooltip_override 恒 Some 优先，不影响断言 */
    );

    let rendered = render_transcript(&cell).join("\n");
    insta::assert_snapshot!(rendered);
}

#[tokio::test]
async fn session_info_preserves_styled_tooltip_links() {
    let config = test_config().await;
    let cell = new_session_info(
        &config,
        &crate::local_settings::LocalSettings::from(&config),
        "gpt-5",
        "gpt-5",
        &session_configured_event("gpt-5"),
        /*is_first_event*/ false,
        Some(
            "Use **/copy** or `ctrl+y`; visit the [Codex community forum](https://example.com)."
                .to_string(),
        ),
        None, /* 008-ADAPT：shim PlanType 空桩仅可构造 None；tooltip_override 恒 Some 优先，不影响断言 */
    );

    let lines = cell.transcript_hyperlink_lines(/*width*/ 30);
    assert_eq!(lines, cell.display_hyperlink_lines(/*width*/ 30));
    assert_eq!(lines, cell.compact_hyperlink_lines(/*width*/ 30));
    assert_eq!(
        visible_lines(lines.clone()),
        cell.transcript_lines(/*width*/ 30)
    );
    let tip_start = lines
        .iter()
        .position(|line| line.line.to_string().starts_with("  Tip:"))
        .unwrap();
    let tip_lines = &lines[tip_start..];
    let command = tip_lines
        .iter()
        .flat_map(|line| &line.line.spans)
        .find(|span| span.content == "/copy")
        .unwrap();
    assert!(command.style.add_modifier.contains(Modifier::BOLD));
    let mut rendered = Vec::new();
    for line in tip_lines {
        let text = line.line.to_string();
        rendered.push(text.clone());
        for link in &line.hyperlinks {
            // This ASCII fixture makes byte offsets equal to terminal columns.
            rendered.push(format!(
                "    link {:?}: {} -> {}",
                link.columns,
                &text[link.columns.clone()],
                link.destination,
            ));
        }
    }
    insta::assert_snapshot!(rendered.join("\n"));
}

#[tokio::test]
async fn session_info_first_event_suppresses_tooltips_and_nux() {
    let config = test_config().await;
    let cell = new_session_info(
        &config,
        &crate::local_settings::LocalSettings::from(&config),
        "gpt-5",
        "gpt-5",
        &session_configured_event("gpt-5"),
        /*is_first_event*/ true,
        Some("Model just became available".to_string()),
        None, /* 008-ADAPT：shim PlanType 空桩仅可构造 None；tooltip_override 恒 Some 优先，不影响断言 */
    );

    let rendered = render_transcript(&cell).join("\n");
    assert!(!rendered.contains("Model just became available"));
    assert!(rendered.contains("To get started"));
}

#[tokio::test]
async fn session_info_hides_tooltips_when_disabled() {
    let mut config = test_config().await;
    config.show_tooltips = false;
    let cell = new_session_info(
        &config,
        &crate::local_settings::LocalSettings::from(&config),
        "gpt-5",
        "gpt-5",
        &session_configured_event("gpt-5"),
        /*is_first_event*/ false,
        Some("Model just became available".to_string()),
        None, /* 008-ADAPT：shim PlanType 空桩仅可构造 None；tooltip_override 恒 Some 优先，不影响断言 */
    );

    let rendered = render_transcript(&cell).join("\n");
    assert!(!rendered.contains("Model just became available"));
}

#[test]
fn ps_output_multiline_snapshot() {
    let cell = new_unified_exec_processes_output(vec![
        UnifiedExecProcessDetails {
            command_display: "echo hello\nand then some extra text".to_string(),
            recent_chunks: vec!["hello".to_string(), "done".to_string()],
        },
        UnifiedExecProcessDetails {
            command_display: "(\n  sleep 120\n)".to_string(),
            recent_chunks: Vec::new(),
        },
        UnifiedExecProcessDetails {
            command_display: "sleep 1\r\nsleep 120".to_string(),
            recent_chunks: Vec::new(),
        },
        UnifiedExecProcessDetails {
            command_display: "rg \"foo\" src".to_string(),
            recent_chunks: vec!["src/main.rs:12:foo".to_string()],
        },
    ]);
    let rendered = render_lines(&cell.display_lines(/*width*/ 40)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn ps_output_multiline_long_command_snapshot() {
    let cell = new_unified_exec_processes_output(vec![UnifiedExecProcessDetails {
        command_display: format!("(\n  {}\n)", "x".repeat(100)),
        recent_chunks: Vec::new(),
    }]);
    let rendered = render_lines(&cell.display_lines(/*width*/ 100)).join("\n");
    insta::assert_snapshot!(rendered);
}

// 008-TRIM: fn cyber_policy_error_event_snapshot（覆盖被裁 new_cyber_policy_error_event/
// crate::daybreak（daybreak 无本地事件源），008 §1）
// 008-TRIM: fn cyber_policy_error_event_astra_snapshot（同上）
// 008-TRIM: fn cyber_policy_error_event_available_snapshot（同上）
// 008-TRIM: fn cyber_policy_error_event_narrow_snapshot（同上）

#[test]
fn safety_access_block_event_snapshot() {
    let cell = new_safety_access_block_event();
    let rendered = render_lines(&cell.display_lines(/*width*/ 80)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn ps_output_long_command_snapshot() {
    let cell = new_unified_exec_processes_output(vec![UnifiedExecProcessDetails {
        command_display: String::from(
            "rg \"foo\" src --glob '**/*.rs' --max-count 1000 --no-ignore --hidden --follow --glob '!target/**'",
        ),
        recent_chunks: vec!["searching...".to_string()],
    }]);
    let rendered = render_lines(&cell.display_lines(/*width*/ 36)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn ps_output_halfwidth_sound_marks_snapshot() {
    let cell = new_unified_exec_processes_output(vec![UnifiedExecProcessDetails {
        command_display: "echo ｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟ".to_string(),
        recent_chunks: vec!["output ｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟ".to_string()],
    }]);
    let rendered = render_lines(&cell.display_lines(/*width*/ 24)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn ps_output_many_sessions_snapshot() {
    let cell = new_unified_exec_processes_output(
        (0..20)
            .map(|idx| UnifiedExecProcessDetails {
                command_display: format!("command {idx}"),
                recent_chunks: Vec::new(),
            })
            .collect(),
    );
    let rendered = render_lines(&cell.display_lines(/*width*/ 32)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn ps_output_chunk_leading_whitespace_snapshot() {
    let cell = new_unified_exec_processes_output(vec![UnifiedExecProcessDetails {
        command_display: "just fix".to_string(),
        recent_chunks: vec![
            "  indented first".to_string(),
            "    more indented".to_string(),
        ],
    }]);
    let rendered = render_lines(&cell.display_lines(/*width*/ 60)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_event_oversized_input_snapshot() {
    let cell = new_error_event(
        "Message exceeds the maximum length of 1048576 characters (1048577 provided).".to_string(),
    );
    let rendered = render_lines(&cell.display_lines(/*width*/ 120)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn error_event_bedrock_expired_signature_snapshot() {
    // 008-ADAPT：shim UnexpectedResponseError 为空桩（不可构造），按母本
    // protocol/src/error.rs Display（user_message ＋ ", url: {url}" 后缀）字面展开
    // error.to_string() 的输出；错误事件 cell 渲染断言不变
    let error = "Amazon Bedrock rejected the request because its AWS signature has expired. \
Refresh your AWS credentials and retry. If `AWS_BEARER_TOKEN_BEDROCK` is set, update or \
unset it, then restart Codex, url: https://bedrock-mantle.us-east-2.api.aws/openai/v1/responses";
    let cell = new_error_event(error.to_string());
    let rendered = render_lines(&cell.display_lines(/*width*/ 100)).join("\n");

    insta::assert_snapshot!(rendered);
}

// 008-TRIM: fn mcp_tools_output_masks_sensitive_values（覆盖被裁 new_mcp_tools_output
// MCP 工具清单（配置脱敏渲染），008 §1）
// 008-TRIM: fn mcp_tools_output_lists_tools_for_hyphenated_server_names（同上）
// 008-TRIM: fn mcp_tools_output_from_statuses_renders_status_only_servers
// （覆盖被裁 new_mcp_tools_output_from_statuses，008 §1）
// 008-TRIM: fn mcp_tools_output_from_statuses_renders_verbose_inventory（同上）

#[test]
fn empty_agent_message_cell_transcript() {
    let cell = AgentMessageCell::new(vec![Line::default()], /*is_first_line*/ false);
    assert_eq!(cell.transcript_lines(/*width*/ 80), vec![Line::from("  ")]);
}

#[test]
fn prefixed_wrapped_history_cell_indents_wrapped_lines() {
    let summary = Line::from(vec![
        "You ".into(),
        "approved".bold(),
        " codex to run ".into(),
        "echo something really long to ensure wrapping happens".dim(),
        " this time".bold(),
    ]);
    let cell = PrefixedWrappedHistoryCell::new(summary, "✔ ".green(), "  ");
    let rendered = render_lines(&cell.display_lines(/*width*/ 24));
    assert_eq!(
        rendered,
        vec![
            "✔ You approved codex to".to_string(),
            "  run echo something".to_string(),
            "  really long to ensure".to_string(),
            "  wrapping happens this".to_string(),
            "  time".to_string(),
        ]
    );
}

#[test]
fn prefixed_wrapped_history_cell_does_not_split_url_like_token() {
    let url_like = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890";
    let cell = PrefixedWrappedHistoryCell::new(Line::from(url_like), "✔ ".green(), "  ");
    let rendered = render_lines(&cell.display_lines(/*width*/ 24));

    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains(url_like))
            .count(),
        1,
        "expected full URL-like token in one rendered line, got: {rendered:?}"
    );
}

#[test]
fn unified_exec_interaction_details_do_not_split_url_like_stdin_token() {
    let url_like = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890";
    let cell = UnifiedExecInteractionCell::new(Some("true".to_string()), url_like.to_string());
    let rendered = render_lines(&cell.transcript_lines(/*width*/ 24));

    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains(url_like))
            .count(),
        1,
        "expected full URL-like token in one rendered line, got: {rendered:?}"
    );
}

#[test]
fn prefixed_wrapped_history_cell_height_matches_wrapped_rendering() {
    let url_like = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/artifacts/reports/performance/summary/detail/with/a/very/long/path";
    let cell: Box<dyn HistoryCell> = Box::new(PrefixedWrappedHistoryCell::new(
        Line::from(url_like),
        "✔ ".green(),
        "  ",
    ));

    let width: u16 = 24;
    let logical_height = cell.display_lines(width).len() as u16;
    let wrapped_height = cell.desired_height(width);
    assert!(
        wrapped_height > logical_height,
        "expected wrapped height to exceed logical line count ({logical_height}), got {wrapped_height}"
    );

    let area = Rect::new(0, 0, width, wrapped_height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    cell.render(area, &mut buf);

    let first_row = (0..area.width)
        .map(|x| {
            let symbol = buf[(x, 0)].symbol();
            if symbol.is_empty() {
                ' '
            } else {
                symbol.chars().next().unwrap_or(' ')
            }
        })
        .collect::<String>();
    assert!(
        first_row.contains("✔"),
        "expected first rendered row to keep the prefix visible, got: {first_row:?}"
    );
}

#[test]
fn unified_exec_interaction_details_wrap_long_input() {
    let url_like = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/artifacts/reports/performance/summary/detail/with/a/very/long/path";
    let cell: Box<dyn HistoryCell> = Box::new(UnifiedExecInteractionCell::new(
        Some("true".to_string()),
        url_like.to_string(),
    ));

    let width: u16 = 24;
    let lines = cell.transcript_hyperlink_lines(width);
    let logical_height = lines.len() as u16;
    let paragraph = HyperlinkParagraph::new(&lines, Style::default());
    let wrapped_height = u16::try_from(paragraph.line_count(width)).unwrap();
    assert!(
        wrapped_height > logical_height,
        "expected wrapped height to exceed logical line count ({logical_height}), got {wrapped_height}"
    );

    let area = Rect::new(/*x*/ 0, /*y*/ 0, width, wrapped_height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    paragraph.render(area, &mut buf);

    let first_row = (0..area.width)
        .map(|x| {
            let symbol = buf[(x, 0)].symbol();
            if symbol.is_empty() {
                ' '
            } else {
                symbol.chars().next().unwrap_or(' ')
            }
        })
        .collect::<String>();
    assert!(
        first_row.contains("Interacted with"),
        "expected first rendered row to keep the header visible, got: {first_row:?}"
    );
}

// 008-TRIM: fn web_search_history_cell_snapshot（覆盖被裁 new_web_search_call
// 搜索工具 cell（history_cell/search，非 transcript 内建搜索），008 §1）

#[test]
fn standalone_unix_update_available_history_cell_snapshot() {
    let cell =
        UpdateAvailableHistoryCell::new("9.9.9".to_string(), Some(UpdateAction::StandaloneUnix));
    let rendered = render_lines(&cell.display_lines(/*width*/ 110)).join("\n");

    insta::assert_snapshot!(rendered);
}

#[test]
fn standalone_windows_update_available_history_cell_snapshot() {
    let cell =
        UpdateAvailableHistoryCell::new("9.9.9".to_string(), Some(UpdateAction::StandaloneWindows));
    let rendered = render_lines(&cell.display_lines(/*width*/ 110)).join("\n");

    insta::assert_snapshot!(rendered);
}

#[test]
fn pnpm_update_available_history_cell_snapshot() {
    let cell =
        UpdateAvailableHistoryCell::new("9.9.9".to_string(), Some(UpdateAction::PnpmGlobalLatest));
    let rendered = render_lines(&cell.display_lines(/*width*/ 110)).join("\n");

    insta::assert_snapshot!(rendered);
}

// 008-TRIM: fn web_search_history_cell_without_detail_snapshot（覆盖被裁
// new_web_search_call，008 §1）
// 008-TRIM: fn web_search_history_cell_truncates（同上）
// 008-TRIM: fn web_search_history_cell_short_query_does_not_wrap（同上）
// 008-TRIM: fn web_search_history_cell_transcript_snapshot（同上）
// 008-TRIM: fn active_mcp_tool_call_snapshot（覆盖被裁 new_active_mcp_tool_call/
// McpInvocation 活动工具 cell，008 §1）
// 008-TRIM: fn code_mode_tool_call_uses_title_and_preserves_full_transcript（同上）
// 008-TRIM: fn code_mode_tool_call_preserves_failure_details（同上）
// 008-TRIM: fn mcp_inventory_loading_snapshot（覆盖被裁 new_mcp_inventory_loading，008 §1）
// 008-TRIM: fn mcp_inventory_loading_without_animations_is_stable（同上）
// 008-TRIM: fn completed_mcp_tool_call_success_snapshot（new_active_mcp_tool_call 完成态，008 §1）
// 008-TRIM: fn completed_mcp_tool_call_image_after_text_snapshot（同上，含 rmcp 图像块）
// 008-TRIM: fn completed_mcp_tool_call_accepts_data_url_image_blocks（同上）
// 008-TRIM: fn completed_mcp_tool_call_multiple_image_blocks_snapshot（同上）
// 008-TRIM: fn completed_mcp_tool_call_error_snapshot（同上）
// 008-TRIM: fn completed_mcp_tool_call_multiple_outputs_snapshot（同上，含 resource_link）
// 008-TRIM: fn completed_mcp_tool_call_wrapped_outputs_snapshot（同上）
// 008-TRIM: fn completed_mcp_tool_call_multiple_outputs_inline_snapshot（同上）

#[test]
fn thread_recap_loading_without_animations_snapshot() {
    let cell = ThreadRecapLoadingCell::new(/*animations_enabled*/ false);
    let rendered = render_lines(&cell.display_lines(/*width*/ 80)).join("\n");

    insta::assert_snapshot!(rendered, @"• Generating conversation recap…");
}

#[test]
fn session_header_clamps_to_narrow_width() {
    const WIDTH: u16 = 44;
    let cell = SessionHeaderHistoryCell::new(
        "gpt-5.6-sol".to_string(),
        Some(ReasoningEffortConfig::XHigh),
        PathBuf::from("project"),
        "test",
    )
    .with_yolo_mode(/*yolo_mode*/ true);

    let lines = cell.display_lines(WIDTH);
    let widths = lines.iter().map(line_width).collect::<Vec<_>>();

    assert!(widths.iter().all(|width| *width <= usize::from(WIDTH)));
    insta::assert_snapshot!(render_lines(&lines).join("\n"));
}

#[test]
#[cfg_attr(
    target_os = "windows",
    ignore = "snapshot path rendering differs on Windows"
)]
fn session_header_indicates_yolo_mode() {
    let cell = SessionHeaderHistoryCell::new(
        "gpt-5".to_string(),
        /*reasoning_effort*/ None,
        test_path_buf("/tmp/project").abs().to_path_buf(),
        "test",
    )
    .with_yolo_mode(/*yolo_mode*/ true);

    let rendered = render_lines(&cell.display_lines(/*width*/ 80)).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn session_header_truncates_halfwidth_directory() {
    let cell: Box<dyn HistoryCell> = Box::new(SessionHeaderHistoryCell::new(
        "gpt-5".to_string(),
        /*reasoning_effort*/ None,
        PathBuf::from("ｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟｶﾞﾊﾟ-project"),
        "test",
    ));

    let width = 42;
    let height = cell.desired_height(width);
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    cell.render(area, &mut buf);

    insta::assert_snapshot!("session_header_halfwidth_directory", format!("{buf:?}"));
}

#[test]
fn yolo_mode_includes_managed_full_access_profiles() {
    let permission_profile: PermissionProfile = PermissionProfile::Managed {
        network: NetworkSandboxPolicy::Enabled,
        file_system: ManagedFileSystemPermissions::Unrestricted,
    };

    assert!(has_yolo_permissions(
        AskForApproval::Never,
        &permission_profile
    ));
}

#[test]
fn yolo_mode_excludes_external_sandbox_profiles() {
    let permission_profile: PermissionProfile = PermissionProfile::External {
        network: NetworkSandboxPolicy::Enabled,
    };

    assert!(!has_yolo_permissions(
        AskForApproval::Never,
        &permission_profile
    ));
}

#[test]
fn session_header_directory_center_truncates() {
    let mut dir = home_dir().expect("home directory");
    for part in ["hello", "the", "fox", "is", "very", "fast"] {
        dir.push(part);
    }

    let formatted = SessionHeaderHistoryCell::format_directory_inner(&dir, Some(24));
    let sep = std::path::MAIN_SEPARATOR;
    let expected = format!("~{sep}hello{sep}the{sep}…{sep}very{sep}fast");
    assert_eq!(formatted, expected);
}

#[test]
fn session_header_directory_front_truncates_long_segment() {
    let mut dir = home_dir().expect("home directory");
    dir.push("supercalifragilisticexpialidocious");

    let formatted = SessionHeaderHistoryCell::format_directory_inner(&dir, Some(18));
    let sep = std::path::MAIN_SEPARATOR;
    let expected = format!("~{sep}…cexpialidocious");
    assert_eq!(formatted, expected);
}

#[test]
fn coalesces_sequential_reads_within_one_call() {
    // Build one exec cell with a Search followed by two Reads
    let call_id = "c1".to_string();
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), "echo".into()],
            parsed: vec![
                ParsedCommand::Search {
                    query: Some("shimmer_spans".into()),
                    path: None,
                    cmd: "rg shimmer_spans".into(),
                },
                ParsedCommand::Read {
                    name: "shimmer.rs".into(),
                    cmd: "cat shimmer.rs".into(),
                    path: "shimmer.rs".into(),
                },
                ParsedCommand::Read {
                    name: "status_indicator_widget.rs".into(),
                    cmd: "cat status_indicator_widget.rs".into(),
                    path: "status_indicator_widget.rs".into(),
                },
            ],
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    // Mark call complete so markers are ✓
    cell.complete_call(&call_id, CommandOutput::default(), Duration::from_millis(1));

    let lines = cell.display_lines(/*width*/ 80);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn coalesces_reads_across_multiple_calls() {
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: "c1".to_string(),
            command: vec!["bash".into(), "-lc".into(), "echo".into()],
            parsed: vec![ParsedCommand::Search {
                query: Some("shimmer_spans".into()),
                path: None,
                cmd: "rg shimmer_spans".into(),
            }],
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    // Call 1: Search only
    cell.complete_call("c1", CommandOutput::default(), Duration::from_millis(1));
    // Call 2: Read A
    assert!(cell.add_call(
        "c2".into(),
        vec!["bash".into(), "-lc".into(), "echo".into()],
        vec![ParsedCommand::Read {
            name: "shimmer.rs".into(),
            cmd: "cat shimmer.rs".into(),
            path: "shimmer.rs".into(),
        }],
        ExecCommandSource::Agent,
        /*interaction_input*/ None,
    ));
    cell.complete_call("c2", CommandOutput::default(), Duration::from_millis(1));
    // Call 3: Read B
    assert!(cell.add_call(
        "c3".into(),
        vec!["bash".into(), "-lc".into(), "echo".into()],
        vec![ParsedCommand::Read {
            name: "status_indicator_widget.rs".into(),
            cmd: "cat status_indicator_widget.rs".into(),
            path: "status_indicator_widget.rs".into(),
        }],
        ExecCommandSource::Agent,
        /*interaction_input*/ None,
    ));
    cell.complete_call("c3", CommandOutput::default(), Duration::from_millis(1));

    let lines = cell.display_lines(/*width*/ 80);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn coalesced_reads_dedupe_names() {
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: "c1".to_string(),
            command: vec!["bash".into(), "-lc".into(), "echo".into()],
            parsed: vec![
                ParsedCommand::Read {
                    name: "auth.rs".into(),
                    cmd: "cat auth.rs".into(),
                    path: "auth.rs".into(),
                },
                ParsedCommand::Read {
                    name: "auth.rs".into(),
                    cmd: "cat auth.rs".into(),
                    path: "auth.rs".into(),
                },
                ParsedCommand::Read {
                    name: "shimmer.rs".into(),
                    cmd: "cat shimmer.rs".into(),
                    path: "shimmer.rs".into(),
                },
            ],
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    cell.complete_call("c1", CommandOutput::default(), Duration::from_millis(1));
    let lines = cell.display_lines(/*width*/ 80);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn multiline_command_wraps_with_extra_indent_on_subsequent_lines() {
    // Create a completed exec cell with a multiline command
    let cmd = "set -o pipefail\ncargo test -p codex-tui --quiet".to_string();
    let call_id = "c1".to_string();
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), cmd],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    // Mark call complete so it renders as "Ran"
    cell.complete_call(&call_id, CommandOutput::default(), Duration::from_millis(1));

    // Small width to keep the wrapped continuation-indent path covered.
    let width: u16 = 28;
    let lines = cell.display_lines(width);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn single_line_command_compact_when_fits() {
    let call_id = "c1".to_string();
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["echo".into(), "ok".into()],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    cell.complete_call(&call_id, CommandOutput::default(), Duration::from_millis(1));
    // Wide enough that it fits inline
    let lines = cell.display_lines(/*width*/ 80);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn single_line_command_wraps_with_four_space_continuation() {
    let call_id = "c1".to_string();
    let long = "a_very_long_token_without_spaces_to_force_wrapping".to_string();
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), long],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    cell.complete_call(&call_id, CommandOutput::default(), Duration::from_millis(1));
    let lines = cell.display_lines(/*width*/ 24);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn single_line_command_over_highlight_limit_uses_plain_text_fallback() {
    let call_id = "c1".to_string();
    let base64_like = "A".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1);
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), base64_like],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    cell.complete_call(&call_id, CommandOutput::default(), Duration::from_millis(1));

    let rendered = render_lines(&cell.display_lines(/*width*/ 24)).join("\n");

    insta::assert_snapshot!(rendered);
}

#[test]
fn multiline_command_without_wrap_uses_branch_then_eight_spaces() {
    let call_id = "c1".to_string();
    let cmd = "echo one\necho two".to_string();
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), cmd],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    cell.complete_call(&call_id, CommandOutput::default(), Duration::from_millis(1));
    let lines = cell.display_lines(/*width*/ 80);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn multiline_command_both_lines_wrap_with_correct_prefixes() {
    let call_id = "c1".to_string();
    let cmd =
        "first_token_is_long_enough_to_wrap\nsecond_token_is_also_long_enough_to_wrap".to_string();
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), cmd],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    cell.complete_call(&call_id, CommandOutput::default(), Duration::from_millis(1));
    let lines = cell.display_lines(/*width*/ 28);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn stderr_tail_more_than_five_lines_snapshot() {
    // Build an exec cell with a non-zero exit and 10 lines on stderr to exercise
    // the three-line preview, hidden-line count, and gutter prefixes.
    let call_id = "c_err".to_string();
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), "seq 1 10 1>&2 && false".into()],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );
    let stderr: String = (1..=10)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    cell.complete_call(
        &call_id,
        CommandOutput::new(/*exit_code*/ 1, stderr),
        Duration::from_millis(1),
    );

    let rendered = cell
        .display_lines(/*width*/ 80)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn ran_cell_multiline_with_stderr_snapshot() {
    // Build an exec cell that completes (so it renders as "Ran") with a
    // command long enough that it must render on its own line under the
    // header, and include a couple of stderr lines to verify the output
    // block prefixes and wrapping.
    let call_id = "c_wrap_err".to_string();
    let long_cmd =
        "echo this_is_a_very_long_single_token_that_will_wrap_across_the_available_width";
    let mut cell = ExecCell::new(
        ExecCall {
            call_id: call_id.clone(),
            command: vec!["bash".into(), "-lc".into(), long_cmd.to_string()],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::Agent,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input: None,
        },
        /*animations_enabled*/ true,
    );

    let stderr = "error: first line on stderr\nerror: second line on stderr".to_string();
    cell.complete_call(
        &call_id,
        CommandOutput::new(/*exit_code*/ 1, stderr),
        Duration::from_millis(5),
    );

    // Narrow width to force the command to render under the header line.
    let width: u16 = 28;
    let rendered = cell
        .display_lines(width)
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(rendered);
}
#[test]
fn user_history_cell_wraps_and_prefixes_each_line_snapshot() {
    let msg = "_count_r\x1b[13;2:3uows";
    let cell = UserHistoryCell {
        spoken: false,
        message: msg.to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: Vec::new(),
    };

    // Small width to force wrapping more clearly. Effective wrap width is width-2 due to the ▌ prefix and trailing space.
    let width: u16 = 12;
    let lines = cell.display_lines(width);
    let rendered = render_lines(&lines).join("\n");

    assert_eq!(render_lines(&cell.raw_lines()), ["_count_rows"]);
    insta::assert_snapshot!(rendered);
}

#[test]
fn user_history_cell_wraps_long_urls_inside_the_message_gutter() {
    let url = "https://example.test/forwarded/threads/10930?page=1&search=&filter=all&queue=customer_support_unprocessed&sort=latest_desc&forwardedScope=all";
    let message = format!(
        "Skip tests.\n\nI just reprocessed\n{url}\ncan you check where we are with it?\n\n[Image #1]"
    );
    let image_start = message.find("[Image #1]").unwrap();
    let cell = UserHistoryCell {
        spoken: false,
        message,
        text_elements: vec![TextElement::new(
            (image_start..image_start + "[Image #1]".len()).into(),
            Some("[Image #1]".to_string()),
        )],
        local_image_paths: Vec::new(),
        remote_image_urls: Vec::new(),
    };
    let width = 64;
    let hyperlink_lines = cell.display_hyperlink_lines(width);

    assert!(
        hyperlink_lines
            .iter()
            .all(|line| line.width() <= usize::from(width)),
        "every user-message row must fit its viewport: {hyperlink_lines:?}"
    );

    let linked_rows = hyperlink_lines
        .iter()
        .filter(|line| !line.hyperlinks.is_empty())
        .collect::<Vec<_>>();
    assert!(linked_rows.len() > 1, "expected the long URL to wrap");
    assert!(
        linked_rows.iter().all(|line| {
            line.line
                .spans
                .first()
                .is_some_and(|span| span.content == "  ")
        }),
        "wrapped URL rows must retain the user-message gutter: {linked_rows:?}"
    );
    assert!(
        linked_rows.iter().all(|line| {
            line.hyperlinks
                .iter()
                .all(|hyperlink| hyperlink.destination == url)
        }),
        "each wrapped URL fragment must preserve the complete clickable destination"
    );

    insta::assert_snapshot!(
        "user_history_cell_wraps_long_urls_inside_the_message_gutter",
        render_lines(&cell.display_lines(width)).join("\n")
    );
}

#[test]
fn user_history_cell_renders_remote_image_urls() {
    let cell = UserHistoryCell {
        spoken: false,
        message: "describe these".to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: vec!["https://example.com/example.png".to_string()],
    };

    let rendered = render_lines(&cell.display_lines(/*width*/ 80)).join("\n");

    assert!(rendered.contains("[Image #1]"));
    assert!(rendered.contains("describe these"));
    insta::assert_snapshot!(rendered);
}

#[test]
fn user_image_labels_follow_the_painted_prompt_surface() {
    use ratatui::widgets::Paragraph;
    use ratatui::widgets::Widget;

    let placeholder = "[Image #1]";
    let cell = UserHistoryCell {
        spoken: false,
        message: format!("{placeholder} describe these"),
        text_elements: vec![TextElement::new(
            (0..placeholder.len()).into(),
            Some(placeholder.to_owned()),
        )],
        local_image_paths: Vec::new(),
        remote_image_urls: vec![
            "https://example.com/one.png".to_string(),
            "https://example.com/two.png".to_string(),
        ],
    };
    let mut snapshots = Vec::new();
    for (fg, bg) in [
        ((30, 30, 30), (255, 255, 255)),
        ((235, 235, 235), (18, 20, 30)),
        ((150, 150, 150), (95, 95, 95)),
    ] {
        crate::terminal_palette::with_test_default_colors(
            crate::terminal_probe::DefaultColors { fg, bg },
            || {
                let area = Rect::new(
                    /*x*/ 0, /*y*/ 0, /*width*/ 32, /*height*/ 5,
                );
                let mut buffer = Buffer::empty(area);
                Paragraph::new(cell.display_lines(area.width)).render(area, &mut buffer);
                snapshots.push(format!("{bg:?}: {buffer:?}"));
            },
        );
    }
    insta::assert_snapshot!(snapshots.join("\n"));
}

#[test]
fn user_history_cell_summarizes_inline_data_urls() {
    let mut cell = UserHistoryCell {
        spoken: false,
        message: "describe inline image".to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: vec!["data:image/png;base64,aGVsbG8=".to_string()],
    };

    let rendered = render_lines(&cell.display_lines(/*width*/ 80)).join("\n");

    assert!(rendered.contains("[Image #1]"));
    assert!(rendered.contains("describe inline image"));
    let placeholder = "[Image #1]";
    cell.message = format!("{placeholder} describe inline image");
    cell.text_elements = vec![TextElement::new(
        (0..placeholder.len()).into(),
        Some(placeholder.to_string()),
    )];
    insta::assert_snapshot!(
        "portable_image_raw_output",
        render_lines(&cell.raw_lines()).join("\n")
    );
}

#[test]
fn user_history_cell_numbers_multiple_remote_images() {
    let cell = UserHistoryCell {
        spoken: false,
        message: "describe both".to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: vec![
            "https://example.com/one.png".to_string(),
            "https://example.com/two.png".to_string(),
        ],
    };

    let rendered = render_lines(&cell.display_lines(/*width*/ 80)).join("\n");

    assert!(rendered.contains("[Image #1]"));
    assert!(rendered.contains("[Image #2]"));
    insta::assert_snapshot!(rendered);
}

#[test]
fn user_history_cell_height_matches_rendered_lines_with_remote_images() {
    let cell = UserHistoryCell {
        spoken: false,
        message: "line one\nline two".to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: vec![
            "https://example.com/one.png".to_string(),
            "https://example.com/two.png".to_string(),
        ],
    };

    let width = 80;
    let rendered_len: u16 = cell
        .display_lines(width)
        .len()
        .try_into()
        .unwrap_or(u16::MAX);
    assert_eq!(cell.desired_height(width), rendered_len);
    assert_eq!(cell.transcript_lines(width), cell.display_lines(width));
}

#[test]
fn user_history_cell_trims_trailing_blank_message_lines() {
    let cell = UserHistoryCell {
        spoken: false,
        message: "line one\n\n   \n\t \n".to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: vec!["https://example.com/one.png".to_string()],
    };

    let rendered = render_lines(&cell.display_lines(/*width*/ 80));
    let trailing_blank_count = rendered
        .iter()
        .rev()
        .take_while(|line| line.trim().is_empty())
        .count();
    assert_eq!(trailing_blank_count, 1);
    assert!(rendered.iter().any(|line| line.contains("line one")));
}

#[test]
fn user_history_cell_trims_trailing_blank_message_lines_with_text_elements() {
    let message = "tokenized\n\n\n".to_string();
    let cell = UserHistoryCell {
        spoken: false,
        message,
        text_elements: vec![TextElement::new(
            (0..8).into(),
            Some("tokenized".to_string()),
        )],
        local_image_paths: Vec::new(),
        remote_image_urls: vec!["https://example.com/one.png".to_string()],
    };

    let rendered = render_lines(&cell.display_lines(/*width*/ 80));
    let trailing_blank_count = rendered
        .iter()
        .rev()
        .take_while(|line| line.trim().is_empty())
        .count();
    assert_eq!(trailing_blank_count, 1);
    assert!(rendered.iter().any(|line| line.contains("tokenized")));
}

#[test]
fn render_uses_wrapping_for_long_url_like_line() {
    let url = "https://example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/artifacts/reports/performance/summary/detail/with/a/very/long/path/that/keeps/going/for/testing/purposes-only-and-does/not/need/to/resolve/index.html?session_id=abc123def456ghi789jkl012mno345pqr678stu901vwx234yz";
    let cell: Box<dyn HistoryCell> = Box::new(UserHistoryCell {
        spoken: false,
        message: url.to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: Vec::new(),
    });

    let width: u16 = 52;
    let height = cell.desired_height(width);
    assert!(
        height > 1,
        "expected wrapped height for long URL, got {height}"
    );

    let area = Rect::new(0, 0, width, height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    cell.render(area, &mut buf);

    let rendered = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| {
                    let symbol = crate::terminal_hyperlinks::strip_osc8(buf[(x, y)].symbol());
                    if symbol.is_empty() {
                        ' '
                    } else {
                        symbol.chars().next().unwrap_or(' ')
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let rendered_blob = rendered.join("\n");
    let rendered_url = rendered
        .iter()
        .filter(|row| !row.trim().is_empty())
        .enumerate()
        .map(|(index, row)| {
            if index == 0 {
                row.strip_prefix("› ").unwrap().trim()
            } else {
                row.trim()
            }
        })
        .collect::<String>();

    assert_eq!(
        rendered_url, url,
        "wrapped URL must preserve every character"
    );

    let non_empty_rows = rendered.iter().filter(|row| !row.trim().is_empty()).count() as u16;
    assert!(
        non_empty_rows > 3,
        "expected long URL to span multiple visible rows, got:\n{rendered_blob}"
    );
}

// 008-TRIM: fn plan_update_with_note_and_wrapping_snapshot（覆盖被裁 new_plan_update/
// UpdatePlanArgs 计划 cell，plans 无本地事件源，008 §1）
// 008-TRIM: fn plan_update_without_note_snapshot（同上）
// 008-TRIM: fn plan_update_does_not_split_url_like_tokens_in_note_or_step（同上）

#[test]
fn reasoning_summary_block() {
    let cell = new_reasoning_summary_block(
        vec!["**High level reasoning**\n\nDetailed reasoning goes here.".to_string()],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, vec!["• Detailed reasoning goes here."]);
}

#[test]
fn reasoning_summary_height_matches_wrapped_rendering_for_url_like_content() {
    let summary = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/artifacts/reports/performance/summary/detail/with/a/very/long/path/that/keeps/going";
    let cell: Box<dyn HistoryCell> = Box::new(ReasoningSummaryCell::new(
        "High level reasoning".to_string(),
        summary.to_string(),
        &test_cwd(),
        /*transcript_only*/ false,
    ));
    let width: u16 = 24;

    let logical_height = cell.display_lines(width).len() as u16;
    let wrapped_height = cell.desired_height(width);
    let expected_wrapped_height = Paragraph::new(Text::from(cell.display_lines(width)))
        .wrap(Wrap { trim: false })
        .line_count(width) as u16;
    assert_eq!(wrapped_height, expected_wrapped_height);
    assert!(
        wrapped_height >= logical_height,
        "expected wrapped height to be at least logical line count ({logical_height}), got {wrapped_height}"
    );

    assert_eq!(cell.transcript_lines(width), cell.display_lines(width));

    let area = Rect::new(0, 0, width, wrapped_height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    cell.render(area, &mut buf);

    let first_row = (0..area.width)
        .map(|x| {
            let symbol = buf[(x, 0)].symbol();
            if symbol.is_empty() {
                ' '
            } else {
                symbol.chars().next().unwrap_or(' ')
            }
        })
        .collect::<String>();
    assert!(
        first_row.contains("•"),
        "expected first rendered row to keep summary bullet visible, got: {first_row:?}"
    );
}

#[test]
fn reasoning_summary_block_returns_reasoning_cell_when_feature_disabled() {
    let cell = new_reasoning_summary_block(
        vec!["Detailed reasoning goes here.".to_string()],
        &test_cwd(),
    );

    let rendered = render_transcript(cell.as_ref());
    assert_eq!(rendered, vec!["• Detailed reasoning goes here."]);
}

#[tokio::test]
async fn reasoning_summary_block_respects_config_overrides() {
    // 008-ADAPT：母本此处对 config.model 的覆盖本就不进入 new_reasoning_summary_block
    // （母本同签名两参、不消费 config，属残留 fixture）；本地 shim Config 无 model 字段，
    // 移除死配置装配，断言不变
    let cell = new_reasoning_summary_block(
        vec!["**High level reasoning**\n\nDetailed reasoning goes here.".to_string()],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());
}

#[test]
fn reasoning_summary_block_falls_back_when_header_is_missing() {
    let cell = new_reasoning_summary_block(
        vec!["**High level reasoning without closing".to_string()],
        &test_cwd(),
    );

    let rendered = render_transcript(cell.as_ref());
    assert_eq!(rendered, vec!["• **High level reasoning without closing"]);
}

#[test]
fn reasoning_summary_block_falls_back_when_summary_is_missing() {
    let cell = new_reasoning_summary_block(
        vec!["**High level reasoning without closing**".to_string()],
        &test_cwd(),
    );

    let rendered = render_transcript(cell.as_ref());
    assert_eq!(rendered, vec!["• High level reasoning without closing"]);

    let cell = new_reasoning_summary_block(
        vec!["**High level reasoning without closing**\n\n  ".to_string()],
        &test_cwd(),
    );

    let rendered = render_transcript(cell.as_ref());
    assert_eq!(rendered, vec!["• High level reasoning without closing"]);
}

#[test]
fn reasoning_summary_block_keeps_title_only_summary_in_expanded_transcript() {
    let cell = new_reasoning_summary_block(
        vec!["**Confirming backend JSONL source**".to_string()],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(
        rendered_transcript,
        vec!["• Confirming backend JSONL source"]
    );
}

#[test]
fn reasoning_summary_block_splits_header_and_summary_when_present() {
    let cell = new_reasoning_summary_block(
        vec!["**High level plan**\n\nWe should fix the bug next.".to_string()],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, vec!["• We should fix the bug next."]);
}

#[test]
fn reasoning_summary_block_hides_empty_html_comment_parts() {
    let cell = new_reasoning_summary_block(
        vec![
            "**Checking the first thing**\n\n<!-- -->".to_string(),
            "**Checking the second thing**\n\n<!-- -->".to_string(),
        ],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, Vec::<String>::new());
}

#[test]
fn reasoning_summary_block_preserves_bold_content_after_empty_html_comment_part() {
    let cell = new_reasoning_summary_block(
        vec![
            "**Status**\n\n<!-- -->".to_string(),
            "**Important conclusion**".to_string(),
            "<!-- -->".to_string(),
        ],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, vec!["• Important conclusion"]);

    let cell = new_reasoning_summary_block(
        vec![
            "**Status**\n\n<!-- -->".to_string(),
            "**Result:** keep **this**".to_string(),
        ],
        &test_cwd(),
    );

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, vec!["• Result: keep this"]);
}

#[test]
fn reasoning_summary_block_strips_header_after_leading_empty_part() {
    let cell = new_reasoning_summary_block(
        vec![
            "**Status**\n\n<!-- -->".to_string(),
            "**Checking tests**\n\nTests passed".to_string(),
        ],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, vec!["• Tests passed"]);
}

#[test]
fn reasoning_summary_block_drops_empty_part_after_real_content() {
    let cell = new_reasoning_summary_block(
        vec![
            "**Plan**\n\ndone".to_string(),
            "**Checking tests**\n\n<!-- -->".to_string(),
        ],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, vec!["• done"]);
}

#[test]
fn reasoning_summary_block_preserves_literal_html_comment() {
    let cell = new_reasoning_summary_block(
        vec!["**Plan**\n\nUse `<!-- -->` in JSX.".to_string()],
        &test_cwd(),
    );

    let rendered_display = render_lines(&cell.display_lines(/*width*/ 80));
    assert_eq!(rendered_display, Vec::<String>::new());

    let rendered_transcript = render_transcript(cell.as_ref());
    assert_eq!(rendered_transcript, vec!["• Use <!-- --> in JSX."]);
}

#[test]
fn deprecation_notice_renders_summary_with_details() {
    let cell = new_deprecation_notice(
        "Feature flag `foo`".to_string(),
        Some("Use flag `bar` instead.".to_string()),
    );
    let lines = cell.transcript_lines(/*width*/ 80);
    assert!(cell.display_lines(/*width*/ 80).is_empty());
    assert!(cell.live_raw_lines().is_empty());
    let rendered = render_lines(&lines);
    assert_eq!(
        rendered,
        vec![
            "⚠ Feature flag `foo`".to_string(),
            "Use flag `bar` instead.".to_string(),
        ]
    );
}

#[test]
fn legacy_zsh_migration_notice_snapshot() {
    // 008-ADAPT：shim codex-features 为空桩（无 record_legacy_usage_force/
    // legacy_feature_usages），按母本 features legacy_usage_notice(ShellZshFork) 的
    // summary/details 字面展开；new_deprecation_notice 渲染断言不变
    let summary = "`[features].shell_zsh_fork` is deprecated and ignored.".to_string();
    let details = Some(
        "The patched zsh backend has been removed. Shell commands now use the standard \
shell executor. Remove `shell_zsh_fork` from [features] in config.toml or profile \
configuration files."
            .to_string(),
    );
    let cell = new_deprecation_notice(summary, details);
    insta::assert_snapshot!(render_lines(&cell.transcript_lines(/*width*/ 80)).join("\n"));
}

#[test]
fn agent_markdown_cell_renders_source_at_different_widths() {
    let source =
        "A long agent message that should wrap differently when the terminal width changes.\n";
    let cell = AgentMarkdownCell::new(source.to_string(), &test_cwd());

    let lines_80 = render_lines(&cell.display_lines(/*width*/ 80));
    assert!(
        lines_80.first().is_some_and(|line| line.starts_with("• ")),
        "first line should start with bullet prefix: {:?}",
        lines_80[0]
    );

    let lines_32 = render_lines(&cell.display_lines(/*width*/ 32));
    assert!(
        lines_32.len() > lines_80.len(),
        "narrower width should produce more wrapped lines: {lines_32:?}",
    );
}

#[test]
fn agent_markdown_cell_does_not_split_words_after_inline_markdown() {
    let source = "This paragraph is intentionally long so you can inspect soft wrapping behavior while also checking inline formatting like **bold text**, *italic text*, ***bold italic text***, `inline code`, ~~strikethrough~~, a [link to example.com](https://example.com), and a literal path like [README.md](/Users/felipe.coury/code/codex.fcoury-worktrees/README.md) without introducing manual line breaks.\n";
    let cell = AgentMarkdownCell::new(source.to_string(), &test_cwd());

    let lines = render_lines(&cell.display_lines(/*width*/ 190));
    assert!(
        lines[0].ends_with("inline code,"),
        "expected wrapping to stop before 'strikethrough': {lines:?}",
    );
    assert!(
        lines[1].starts_with("  strikethrough,"),
        "expected the next line to resume with the full word: {lines:?}",
    );
}

#[test]
fn streamed_agent_list_paragraph_preserves_item_indent_when_wrapped() {
    let cell = AgentMessageCell::new(
        vec![
            Line::from("1. Correctness issue: server tool-search completions are rejected."),
            Line::default(),
            Line::from(
                "   In next_prompt_suggestion.rs, ToolSearchCall records its call id, but a paired output is ignored and suppresses suggestions.",
            ),
        ],
        /*is_first_line*/ true,
    );

    let lines = render_lines(&cell.display_lines(/*width*/ 64));
    assert!(
        lines
            .iter()
            .filter(|line| line.contains("paired output") || line.contains("suggestions."))
            .all(|line| line.starts_with("     ")),
        "expected all wrapped paragraph rows to retain the assistant gutter and list indent: {lines:?}",
    );
    insta::assert_snapshot!(lines.join("\n"));
}

#[test]
fn agent_markdown_cell_narrow_width_shows_prefix_only() {
    let source = "narrow width coverage\n";
    let cell = AgentMarkdownCell::new(source.to_string(), &test_cwd());

    let lines = render_lines(&cell.display_lines(/*width*/ 2));
    assert_eq!(lines, vec!["• ".to_string()]);
}

#[test]
fn wrapped_and_prefixed_cells_handle_tiny_widths() {
    let user_cell = UserHistoryCell {
        spoken: false,
        message: "tiny width coverage for wrapped user history".to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: Vec::new(),
    };
    let agent_message_cell = AgentMessageCell::new(
        vec!["tiny width agent line".into()],
        /*is_first_line*/ true,
    );
    let reasoning_cell = ReasoningSummaryCell::new(
        "Plan".to_string(),
        "Reasoning summary content for tiny widths.".to_string(),
        &test_cwd(),
        /*transcript_only*/ false,
    );
    let agent_markdown_cell =
        AgentMarkdownCell::new("tiny width agent markdown line\n".to_string(), &test_cwd());

    for width in 1..=4 {
        assert!(
            !user_cell.display_lines(width).is_empty(),
            "user cell should render at width {width}",
        );
        assert!(
            !agent_message_cell.display_lines(width).is_empty(),
            "agent message cell should render at width {width}",
        );
        assert!(
            !reasoning_cell.display_lines(width).is_empty(),
            "reasoning cell should render at width {width}",
        );
        assert!(
            !agent_markdown_cell.display_lines(width).is_empty(),
            "agent markdown cell should render at width {width}",
        );
    }
}

#[test]
fn render_clears_area_when_cell_content_shrinks() {
    let area = Rect::new(0, 0, 40, 6);
    let mut buf = Buffer::empty(area);

    let first: Box<dyn HistoryCell> = Box::new(PlainHistoryCell::new(vec![
        Line::from("STALE ROW 1"),
        Line::from("STALE ROW 2"),
        Line::from("STALE ROW 3"),
        Line::from("STALE ROW 4"),
    ]));
    first.render(area, &mut buf);

    let second: Box<dyn HistoryCell> = Box::new(PlainHistoryCell::new(vec![Line::from("fresh")]));
    second.render(area, &mut buf);

    let mut rendered_rows: Vec<String> = Vec::new();
    for y in 0..area.height {
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buf.cell((x, y)).expect("cell should exist").symbol());
        }
        rendered_rows.push(row);
    }

    assert!(
        rendered_rows.iter().all(|row| !row.contains("STALE")),
        "rendered buffer should not retain stale glyphs: {rendered_rows:?}",
    );
    assert!(
        rendered_rows
            .first()
            .is_some_and(|row| row.contains("fresh")),
        "expected fresh content in first row: {rendered_rows:?}",
    );
}

#[test]
fn agent_markdown_cell_survives_insert_history_rewrap() {
    let source = "\
  Canary rollout remained at limited traffic longer than planned because p95
  latency briefly regressed during cold-cache periods.
  Regional expansion succeeded with stable error rates, though internal
  analytics lagged temporarily.
  ";
    let cell = AgentMarkdownCell::new(source.to_string(), &test_cwd());
    let width: u16 = 80;
    let lines = cell.display_lines(width);

    // Simulate what insert_history_lines does: word_wrap_lines with
    // the terminal width and no indent.
    let rewrapped = word_wrap_lines(&lines, width as usize);
    let before = render_lines(&lines);
    let after = render_lines(&rewrapped);
    assert_eq!(
        before, after,
        "word_wrap_lines should not alter lines that already fit within width"
    );
}

/// Simulate the consolidation backward-walk logic from `App::handle_event`
/// to verify it correctly identifies and replaces `AgentMessageCell` runs.
#[test]
fn consolidation_walker_replaces_agent_message_cells() {
    use std::sync::Arc;

    // Build a transcript with: [UserCell, AgentMsg(head), AgentMsg(cont), AgentMsg(cont)]
    let user = Arc::new(UserHistoryCell {
        spoken: false,
        message: "hello".to_string(),
        text_elements: Vec::new(),
        local_image_paths: Vec::new(),
        remote_image_urls: Vec::new(),
    }) as Arc<dyn HistoryCell>;
    let head = Arc::new(AgentMessageCell::new(
        vec![Line::from("line 1")],
        /*is_first_line*/ true,
    )) as Arc<dyn HistoryCell>;
    let cont1 = Arc::new(AgentMessageCell::new(
        vec![Line::from("line 2")],
        /*is_first_line*/ false,
    )) as Arc<dyn HistoryCell>;
    let cont2 = Arc::new(AgentMessageCell::new(
        vec![Line::from("line 3")],
        /*is_first_line*/ false,
    )) as Arc<dyn HistoryCell>;

    let mut transcript_cells: Vec<Arc<dyn HistoryCell>> = vec![user.clone(), head, cont1, cont2];

    // Run the same consolidation logic as the handler.
    let source = "line 1\nline 2\nline 3\n".to_string();
    let end = transcript_cells.len();
    let mut start = end;
    while start > 0
        && transcript_cells[start - 1].is_stream_continuation()
        && transcript_cells[start - 1]
            .as_any()
            .is::<AgentMessageCell>()
    {
        start -= 1;
    }
    if start > 0
        && transcript_cells[start - 1]
            .as_any()
            .is::<AgentMessageCell>()
        && !transcript_cells[start - 1].is_stream_continuation()
    {
        start -= 1;
    }

    assert_eq!(
        start, 1,
        "should find all 3 agent cells starting at index 1"
    );
    assert_eq!(end, 4);

    // Splice.
    let consolidated: Arc<dyn HistoryCell> = Arc::new(AgentMarkdownCell::new(source, &test_cwd()));
    transcript_cells.splice(start..end, std::iter::once(consolidated));

    assert_eq!(transcript_cells.len(), 2, "should be [user, consolidated]");

    // Verify first cell is still the user cell.
    assert!(
        transcript_cells[0].as_any().is::<UserHistoryCell>(),
        "first cell should be UserHistoryCell"
    );

    // Verify second cell is AgentMarkdownCell.
    assert!(
        transcript_cells[1].as_any().is::<AgentMarkdownCell>(),
        "second cell should be AgentMarkdownCell"
    );
}
