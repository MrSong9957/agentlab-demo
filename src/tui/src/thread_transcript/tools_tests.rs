use super::*;
use crate::test_support::PathBufExt;
use crate::test_support::test_path_buf;
use crate::thread_transcript::RawReasoningVisibility;
use crate::thread_transcript::thread_items_to_transcript_cells;
use codex_app_server_protocol::CommandAction;
use pretty_assertions::assert_eq;
// 008-TRIM: McpToolCallResult/LegacyAppPathString/serde_json::json 导入
// （McpToolCall 投影与 legacy 路径类型本地缺席，008 §1）

fn command_item(status: CommandExecutionStatus) -> ThreadItem {
    ThreadItem::CommandExecution {
        id: "command".to_string(),
        plugin_id: None,
        script_path: None,
        // 008-ADAPT：model_context/sandbox_type 为母本 serde(skip) 字段，本地 ThreadItem 映射省略（shim 登记）
        command: "cargo check".to_string(),
        // 008-ADAPT：cwd 本地映射为 String（母本 LegacyAppPathString，shim 登记）
        cwd: "/tmp/project".to_string(),
        process_id: None,
        source: CommandExecutionSource::Agent,
        status,
        command_actions: vec![CommandAction::Unknown {
            command: "cargo check".to_string(),
        }],
        aggregated_output: Some(
            (1..=12)
                .map(|line| format!("output line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        exit_code: Some(0),
        duration_ms: Some(-5),
    }
}

// 008-TRIM: fn mcp_item（覆盖被裁 ThreadItem::McpToolCall 投影，MCP 无本地事件源，008 §1）

// 008-TRIM: fn completed_tools_keep_compact_and_detailed_presentations
// （Command+MCP 混合投影；命名快照 completed_tool_presentations 内嵌 MCP 输出，
// 部分保留必然偏离黄金快照，008 §1）
// 008-TRIM: fn incomplete_tool_payloads_preserve_last_known_status_and_output
// （InProgress 命令＋两个 McpToolCall 载荷；命名快照 pending_tool_presentations 内嵌 MCP
// 输出，008 §1）

#[test]
fn failed_commands_keep_failure_when_exit_code_is_missing_or_zero() {
    for exit in [None, Some(0)] {
        let mut item = command_item(CommandExecutionStatus::Failed);
        if let ThreadItem::CommandExecution { exit_code, .. } = &mut item {
            *exit_code = exit;
        }
        let command = CommandHistory::from_item(item).unwrap();
        assert_eq!((command.exit_code, command.duration), (1, Duration::ZERO));
    }
}

#[test]
fn historical_command_fallbacks_preserve_status_output_and_group_boundaries() {
    let cwd = test_path_buf("/workspace").abs();
    let exploration = |call_id: &str| {
        let mut item = command_item(CommandExecutionStatus::Completed);
        if let ThreadItem::CommandExecution {
            id,
            command,
            command_actions,
            ..
        } = &mut item
        {
            *id = call_id.to_owned();
            *command = "cat README.md".to_owned();
            *command_actions = vec![CommandAction::Read {
                command: command.clone(),
                name: "README.md".to_owned(),
                // 008-ADAPT：CommandAction::Read.path 本地映射为 String（母本 LegacyAppPathString）
                path: "/workspace/README.md".to_string(),
            }];
        }
        item
    };
    let mut snapshots = Vec::new();
    for (source, status, recorded_exit, label) in [
        (
            CommandExecutionSource::Agent,
            CommandExecutionStatus::Declined,
            None,
            "declined",
        ),
        (
            CommandExecutionSource::Agent,
            CommandExecutionStatus::Completed,
            Some(0),
            "opaque command",
        ),
        (
            CommandExecutionSource::Agent,
            CommandExecutionStatus::Completed,
            Some(0),
            "UNC command",
        ),
        (
            CommandExecutionSource::UnifiedExecInteraction,
            CommandExecutionStatus::Completed,
            Some(0),
            "completed interaction",
        ),
        (
            CommandExecutionSource::UnifiedExecInteraction,
            CommandExecutionStatus::Failed,
            Some(7),
            "failed interaction",
        ),
        (
            CommandExecutionSource::UnifiedExecInteraction,
            CommandExecutionStatus::Failed,
            None,
            "failed interaction without exit code",
        ),
        (
            CommandExecutionSource::UnifiedExecInteraction,
            CommandExecutionStatus::InProgress,
            None,
            "pending interaction",
        ),
    ] {
        let mut item = command_item(status);
        if let ThreadItem::CommandExecution {
            source: item_source,
            command,
            aggregated_output,
            exit_code,
            ..
        } = &mut item
        {
            *item_source = source;
            if label == "opaque command" {
                *command = r#"C:\Program Files\Git\bin\bash.exe -lc "echo hi""#.to_owned();
            } else if label == "UNC command" {
                *command = r"\\server\share\tool.exe -arg".to_owned();
            }
            *aggregated_output = Some("first line\n  indented output\nlast line".to_owned());
            *exit_code = recorded_exit;
        }
        let cells = thread_items_to_transcript_cells(
            /*thread_id*/ None,
            &cwd,
            [exploration("before"), item, exploration("after")],
            RawReasoningVisibility::Hidden,
            /*config*/ None,
        );
        assert_eq!(cells.len(), 3);
        for cell in [&cells[0], &cells[2]] {
            assert!(
                cell.as_any()
                    .downcast_ref::<ExecCell>()
                    .unwrap()
                    .is_exploring_cell()
            );
        }
        let [compact, detailed, raw] = [
            cells[1].display_lines(/*width*/ 80),
            cells[1].transcript_lines(/*width*/ 80),
            cells[1].raw_lines(),
        ]
        .map(|lines| {
            lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        });
        assert_eq!((&detailed, &raw), (&compact, &compact));
        snapshots.push(format!("{label}\n{compact}"));
    }
    insta::assert_snapshot!(snapshots.join("\n\n"));
}

// 008-TRIM: fn agent_tool_fallbacks_preserve_status_without_duplicating_v2_activity
// （覆盖被裁 ThreadItem::CollabAgentToolCall 投影（collab 子代理无本地事件源），008 §1）
