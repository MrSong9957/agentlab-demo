//! Expanded command history with source-backed selection and chronological activity details.
//! Raw history omits transcript-only reasoning while retaining full command output.

use super::model::ExecCell;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::history_cell::HistoryRenderMode;
use crate::render::highlight::highlight_bash_to_lines;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::adaptive_wrap_hyperlink_lines;
use crate::terminal_hyperlinks::plain_hyperlink_lines;
use crate::wrapping::RtOptions;
use codex_ansi_escape::ansi_escape_line;
use codex_utils_elapsed::format_duration;
use ratatui::prelude::*;

impl ExecCell {
    pub(super) fn detailed_hyperlink_lines(
        &self,
        width: u16,
        mode: HistoryRenderMode,
    ) -> Vec<HyperlinkLine> {
        let mut lines: Vec<HyperlinkLine> = vec![];
        for (i, call) in self.iter_calls().enumerate() {
            if i > 0 {
                lines.push("".into());
            }
            let script = strip_bash_lc_and_escape(&call.command);
            let highlighted_script = highlight_bash_to_lines(&script);
            let mut cmd_display = adaptive_wrap_hyperlink_lines(
                &plain_hyperlink_lines(highlighted_script),
                RtOptions::new(width as usize)
                    .initial_indent("$ ".magenta().into())
                    .subsequent_indent("    ".into()),
            );
            // 012-ADAPT（vendored 偏离登记，docs/specs/012-tui-tool-details.md
            // §6.2/K-1）：展开态以状态头开场（复用紧凑头构造：marker＋标题＋
            // 时长），状态归属明确；母本此形态无头行、状态在尾部独立行——本地
            // 改由头部承载并移除尾部行（命令/状态/时长不因拼接重复展示）。
            // 头部并入 `$ cmd` 首行（display 路径同法，render.rs 首行前缀拼接），
            // source.prefix_bytes 记账使复制文本仍＝命令＋输出（UI 标签不入复制）。
            let mut header_spans: Vec<Span<'static>> = vec![
                self.status_marker_span(call),
                " ".into(),
                self.status_title(call).bold(),
            ];
            if let Some(duration) = call.duration {
                header_spans.push(Span::from(format!(" • {}", format_duration(duration))).dim());
            }
            header_spans.push(" ".into());
            match cmd_display.first_mut() {
                Some(first) => {
                    let header_bytes: usize =
                        header_spans.iter().map(|span| span.content.len()).sum();
                    // 注：若未来此首行携带 hyperlink 注记，前缀拼接后须同步平移
                    // hyperlink.columns（参见 prefix_hyperlink_lines 的移位处理）；
                    // 当前命令行走 plain_hyperlink_lines，无链接注记。
                    first.line.spans.splice(0..0, header_spans);
                    if let Some(source) = first.source.as_mut() {
                        source.prefix_bytes += header_bytes;
                    }
                }
                None => lines.push(HyperlinkLine::from(Line::from(header_spans))),
            }
            // 拼接后首行可超 width——transcript 渲染经 TextLayout/logical_lines 按
            // source 前缀（initial_indent）二次重排，故不在此预扣头部宽度
            lines.extend(cmd_display);

            if let Some(output) = call.output.as_ref() {
                if !call.is_unified_exec_interaction() {
                    let wrap_width = width.max(/*other*/ 1) as usize;
                    let wrap_opts = RtOptions::new(wrap_width);
                    for unwrapped in output
                        .transcript_lines()
                        .map(|line| ansi_escape_line(line.as_ref()))
                    {
                        lines.extend(adaptive_wrap_hyperlink_lines(
                            &[unwrapped.into()],
                            wrap_opts.clone(),
                        ));
                    }
                }
                // 012-ADAPT：尾部独立 `✓/✗ • 时长` 行移除——状态/退出码/时长已由
                // 头部承载（见上），不因拼接重复展示
            }
            lines.extend(self.group.details.lines_after(i + 1, width, mode));
        }
        lines
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
