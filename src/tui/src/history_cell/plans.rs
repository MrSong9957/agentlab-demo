//! 计划更新历史 cell（017 恢复，008 §1 曾因无本地事件源裁剪）。
//! 母本 codex@d138398 `tui/src/history_cell/plans.rs` 的 `PlanUpdateCell`／
//! `new_plan_update` 最小子集：Proposed Plan 流式与 Plan Mode cell 不移植。
//! 本地适配：活动 ID 由宿主传入（tool_call_id／投影合成 id），不另生成随机 UUID。

use super::*;
use crate::style::accent_color;

/// 成功计划快照 cell：说明＋步骤清单按状态渲染。
pub(crate) fn new_plan_update(activity_id: String, update: UpdatePlanArgs) -> PlanUpdateCell {
    let UpdatePlanArgs { explanation, plan } = update;
    PlanUpdateCell {
        activity_id,
        explanation,
        plan,
    }
}

#[derive(Debug)]
pub(crate) struct PlanUpdateCell {
    activity_id: String,
    explanation: Option<String>,
    plan: Vec<PlanItemArg>,
}

impl HistoryCell for PlanUpdateCell {
    fn activity_ids(&self) -> Vec<String> {
        vec![self.activity_id.clone()]
    }

    fn activity_disclosure(&self, width: u16) -> Option<super::ActivityDisclosure> {
        (self
            .explanation
            .as_ref()
            .is_some_and(|explanation| !explanation.trim().is_empty())
            || self.plan.len() > super::activity_preview::DETAIL_PREVIEW_LINES
            || self.plan.iter().any(|item| {
                let step = item.step.split_whitespace().collect::<Vec<_>>().join(" ");
                // The compact row reserves four columns for indentation and two for its marker.
                Line::from(step).width() + 6 > usize::from(width)
            }))
        .then_some(super::ActivityDisclosure::Generic)
    }

    fn compact_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        use super::activity_preview::DETAIL_PREVIEW_LINES;
        use super::activity_preview::clipped_line;

        let completed = self
            .plan
            .iter()
            .filter(|item| matches!(item.status, StepStatus::Completed))
            .count();
        let total = self.plan.len();
        let mut lines = vec![clipped_line(
            Line::from(vec![
                "• ".dim(),
                "Updated Plan".bold(),
                format!(" · {completed}/{total} complete").dim(),
            ]),
            width,
        )];
        let mut steps: Vec<_> = self.plan.iter().collect();
        steps.sort_by_key(|item| match item.status {
            StepStatus::InProgress => 0,
            StepStatus::Pending => 1,
            StepStatus::Completed => 2,
        });
        for (index, item) in steps.into_iter().take(DETAIL_PREVIEW_LINES).enumerate() {
            let (marker, style) = match item.status {
                StepStatus::Completed => ("✔ ", Style::default().crossed_out().dim()),
                StepStatus::InProgress => ("□ ", Style::default().fg(accent_color()).bold()),
                StepStatus::Pending => ("□ ", Style::default().dim()),
            };
            let step = item.step.split_whitespace().collect::<Vec<_>>().join(" ");
            lines.push(clipped_line(
                Line::from(vec![
                    if index == 0 { "  └ " } else { "    " }.dim(),
                    marker.set_style(style),
                    step.set_style(style),
                ]),
                width,
            ));
        }
        if self.plan.is_empty() {
            lines.push(clipped_line(
                Line::from("  └ (no steps provided)".dim()),
                width,
            ));
        }
        lines
    }

    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        visible_lines(self.display_hyperlink_lines(width))
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        let render_note = |text: &str| -> Vec<HyperlinkLine> {
            let wrap_width = width.saturating_sub(4).max(1) as usize;
            let note = HyperlinkLine::new(Line::from(text.to_string().dim().italic()));
            crate::terminal_hyperlinks::adaptive_wrap_hyperlink_lines(
                &[note],
                RtOptions::new(wrap_width),
            )
        };

        let render_step = |status: &StepStatus, text: &str| -> Vec<HyperlinkLine> {
            let (box_str, step_style) = match status {
                StepStatus::Completed => ("✔ ", Style::default().crossed_out().dim()),
                StepStatus::InProgress => ("□ ", Style::default().fg(accent_color()).bold()),
                StepStatus::Pending => ("□ ", Style::default().dim()),
            };

            let opts = RtOptions::new(width.saturating_sub(4).max(1) as usize)
                .initial_indent(box_str.into())
                .subsequent_indent("  ".into());
            let step = HyperlinkLine::new(Line::from(text.to_string().set_style(step_style)));
            let mut wrapped =
                crate::terminal_hyperlinks::adaptive_wrap_hyperlink_lines(&[step], opts);
            crate::terminal_hyperlinks::retain_initial_prefix(&mut wrapped, box_str);
            wrapped
        };

        let mut lines = vec![HyperlinkLine::new(
            vec!["• ".dim(), "Updated Plan".bold()].into(),
        )];

        let mut indented_lines = vec![];
        let note = self
            .explanation
            .as_ref()
            .map(|s| s.trim())
            .filter(|t| !t.is_empty());
        if let Some(expl) = note {
            indented_lines.extend(render_note(expl));
        };

        if self.plan.is_empty() {
            indented_lines.push(HyperlinkLine::new(Line::from(
                "(no steps provided)".dim().italic(),
            )));
        } else {
            for PlanItemArg { step, status } in self.plan.iter() {
                indented_lines.extend(render_step(status, step));
            }
        }
        lines.extend(prefix_hyperlink_lines(
            indented_lines,
            "  └ ".dim(),
            "    ".into(),
        ));

        lines
    }

    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from("Updated Plan")];
        if let Some(explanation) = self
            .explanation
            .as_ref()
            .map(|s| s.trim())
            .filter(|t| !t.is_empty())
        {
            lines.extend(raw_lines_from_source(explanation));
        }
        if self.plan.is_empty() {
            lines.push(Line::from("(no steps provided)"));
        } else {
            for PlanItemArg { step, status } in &self.plan {
                lines.push(Line::from(format!("{status:?}: {step}")));
            }
        }
        lines
    }
}
