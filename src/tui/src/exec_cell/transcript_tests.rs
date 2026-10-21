//! Grouped command details retain terminal input in raw output without exposing hidden reasoning.

use super::*;
use crate::exec_cell::CommandOutput;
use crate::exec_cell::model::ExecCall;
use crate::history_cell::HistoryCell;
use crate::history_cell::new_reasoning_summary_block;
use crate::history_cell::new_unified_exec_interaction;
use codex_app_server_protocol::CommandExecutionSource;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn completed_read(name: &str, output: &str) -> ExecCell {
    let command = vec!["cat".to_owned(), name.to_owned()];
    let parsed = codex_shell_command::parse_command::parse_command(&command);
    ExecCell::new(
        ExecCall {
            call_id: name.to_owned(),
            command,
            parsed,
            output: Some(CommandOutput::new(/*exit_code*/ 0, output.to_owned())),
            source: CommandExecutionSource::UnifiedExecStartup,
            start_time: None,
            duration: Some(Duration::from_millis(/*millis*/ 5)),
            interaction_input: None,
        },
        /*animations_enabled*/ false,
    )
}

#[test]
fn raw_grouped_history_retains_terminal_input_and_omits_reasoning() {
    let mut group = completed_read("first.txt", "first output");
    group
        .group
        .push_detail(Arc::from(new_reasoning_summary_block(
            vec!["Inspecting the next file".to_owned()],
            Path::new("."),
        ) as Box<dyn HistoryCell>));
    group
        .group
        .push_detail(Arc::new(new_unified_exec_interaction(
            Some("cat first.txt".to_owned()),
            "continue\n".to_owned(),
        )));
    group
        .append_completed(completed_read("second.txt", "second output"))
        .expect("adjacent reads stay grouped");

    let raw = group
        .raw_lines()
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>();
    let rich = group
        .transcript_lines(/*width*/ 80)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(format!("Rich\n{rich}\n\nRaw\n{}", raw.join("\n")));

    let mut earlier = completed_read("earlier.txt", "earlier output");
    earlier
        .group
        .push_detail(Arc::from(new_reasoning_summary_block(
            vec!["Inspect the first file".to_owned()],
            Path::new("."),
        ) as Box<dyn HistoryCell>));
    earlier
        .group
        .push_detail(Arc::new(new_unified_exec_interaction(
            Some("cat earlier.txt".to_owned()),
            "next\n".to_owned(),
        )));
    let mut expected_raw = earlier.raw_lines();
    expected_raw.push(Line::from(""));
    expected_raw.extend(group.raw_lines());
    let mut expected_rich = earlier.transcript_hyperlink_lines(/*width*/ 80);
    expected_rich.push(HyperlinkLine::from(""));
    expected_rich.extend(group.transcript_hyperlink_lines(/*width*/ 80));
    group.prepend(earlier);
    assert_eq!(group.raw_lines(), expected_raw);
    assert_eq!(
        group.transcript_hyperlink_lines(/*width*/ 80),
        expected_rich
    );
}

/// 012 §6 C1/C4：展开态头部承载状态语义（复用紧凑头构造：marker＋标题＋
/// 时长），尾部不再拼接独立 `✓ • 时长` 行（不因拼接重复展示）；命令行＝
/// 真实执行 argv（["/bin/sh","-c",<cmd>]）剥壳后的原文。
#[test]
fn detailed_form_leads_with_status_header_without_trailing_result() {
    let command = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "echo 详细-ok && true".to_owned(),
    ];
    let cell = ExecCell::new(
        ExecCall {
            call_id: "call-d".to_owned(),
            command,
            parsed: Vec::new(),
            output: Some(CommandOutput::new(/*exit_code*/ 0, "详细-ok\n".to_owned())),
            source: CommandExecutionSource::Agent,
            start_time: None,
            duration: Some(Duration::from_millis(1030)),
            interaction_input: None,
        },
        /*animations_enabled*/ false,
    );
    let text = cell
        .transcript_lines(/*width*/ 80)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Ran"), "头部状态标题：{text}");
    assert!(text.contains("1.03s"), "头部时长：{text}");
    assert!(
        text.contains("$ echo 详细-ok && true"),
        "命令＝剥壳原文（真实 argv）：{text}"
    );
    assert!(
        !text.contains("$ 'echo"),
        "命令不得整条包裹引号：{text}"
    );
    assert!(
        !text.contains('✓'),
        "尾部独立状态行已移除（语义并入头部）：{text}"
    );
    // 输出全文保留
    assert!(text.contains("详细-ok"), "全量输出保留：{text}");
}

/// 012 §6 C4：失败与运行态的展开头具名断言——Failed (exit N)／Running 标题
/// 随头部并入 `$ cmd` 首行（成功态见 detailed_form_leads_with_status_header…）。
#[test]
fn detailed_form_header_states_failed_and_running() {
    let mk = |output: Option<CommandOutput>, duration: Option<Duration>, start: Option<std::time::Instant>| {
        ExecCell::new(
            ExecCall {
                call_id: "call-st".to_owned(),
                command: vec!["/bin/sh".to_owned(), "-c".to_owned(), "false || exit 9".to_owned()],
                parsed: Vec::new(),
                output,
                source: CommandExecutionSource::Agent,
                start_time: start,
                duration,
                interaction_input: None,
            },
            /*animations_enabled*/ false,
        )
    };
    // 失败：标题＋退出码入头，时长并列
    let failed = mk(
        Some(CommandOutput::new(9, String::new())),
        Some(Duration::from_millis(7)),
        None,
    );
    let text = failed
        .transcript_lines(/*width*/ 80)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Failed (exit 9) • 7ms $ false || exit 9"),
        "失败头（标题＋退出码＋时长＋命令）：{text}"
    );
    assert!(!text.contains('✗'), "无尾部独立状态行：{text}");
    // 运行态：Running 标题、无时长（duration None＝is_active）
    let running = mk(None, None, Some(std::time::Instant::now()));
    let text = running
        .transcript_lines(/*width*/ 80)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Running $ false || exit 9"),
        "运行头（标题＋命令，无时长）：{text}"
    );
}
