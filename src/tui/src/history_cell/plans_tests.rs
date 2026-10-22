//! 计划更新 cell 渲染测试（017 恢复）。前三例移植自母本
//! codex@d138398 `history_cell/tests.rs` 的 plan_update 三测（适配本地
//! `new_plan_update(activity_id, args)` 签名——活动 ID 由宿主传入），
//! 锚定清单渲染保真；其余为 017 本地补充（空计划、中文、宿主 ID）。

use super::*;
use pretty_assertions::assert_eq;

/// 与 tests.rs 同义的行拼接助手（兄弟模块私有，不跨模块复用）。
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

#[test]
fn plan_update_with_note_and_wrapping_snapshot() {
    // Long explanation forces wrapping; include long step text to verify step wrapping and alignment.
    let update = UpdatePlanArgs {
        explanation: Some(
            "I’ll update Grafana call error handling by adding retries and clearer messages when the backend is unreachable."
                .to_string(),
        ),
        plan: vec![
            PlanItemArg {
                step: "Investigate existing error paths and logging around HTTP timeouts".into(),
                status: StepStatus::Completed,
            },
            PlanItemArg {
                step: "Harden Grafana client error handling with retry/backoff and user‑friendly messages".into(),
                status: StepStatus::InProgress,
            },
            PlanItemArg {
                step: "Add tests for transient failure scenarios and surfacing to the UI".into(),
                status: StepStatus::Pending,
            },
            PlanItemArg { step: "Document the retry behavior".into(), status: StepStatus::Pending },
        ],
    };

    let cell = new_plan_update("call-plan-snap".to_string(), update);
    // Narrow width to force wrapping for both the note and steps
    let lines = cell.display_lines(/*width*/ 32);
    let compact =
        render_lines(&visible_lines(cell.compact_hyperlink_lines(/*width*/ 32))).join("\n");
    let rendered = format!(
        "Compact\n{compact}\n\nFull\n{}",
        render_lines(&lines).join("\n")
    );
    insta::assert_snapshot!(rendered);
}

#[test]
fn plan_update_without_note_snapshot() {
    let update = UpdatePlanArgs {
        explanation: None,
        plan: vec![
            PlanItemArg {
                step: "Define error taxonomy".into(),
                status: StepStatus::InProgress,
            },
            PlanItemArg {
                step: "Implement mapping to user messages".into(),
                status: StepStatus::Pending,
            },
        ],
    };

    let cell = new_plan_update("call-plan-2".to_string(), update);
    let lines = cell.display_lines(/*width*/ 40);
    let rendered = render_lines(&lines).join("\n");
    insta::assert_snapshot!(rendered);
}

#[test]
fn plan_update_does_not_split_url_like_tokens_in_note_or_step() {
    let note_url = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890";
    let step_url = "example.test/api/v1/projects/beta-team/releases/2026-02-17/builds/0987654321/artifacts/reports/performance";
    let update = UpdatePlanArgs {
        explanation: Some(format!(
            "Investigate failures under {note_url} immediately."
        )),
        plan: vec![PlanItemArg {
            step: format!("Validate callbacks under {step_url} before rollout."),
            status: StepStatus::InProgress,
        }],
    };

    let cell = new_plan_update("call-plan-url".to_string(), update);
    let rendered = render_lines(&cell.display_lines(/*width*/ 30));

    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains(note_url))
            .count(),
        1,
        "expected full note URL-like token in one rendered line, got: {rendered:?}"
    );
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains(step_url))
            .count(),
        1,
        "expected full step URL-like token in one rendered line, got: {rendered:?}"
    );
}

/// 017 本地：空计划＝上游无步骤提示；活动 ID 由宿主传入原样保留；
/// 中文步骤按原文渲染；raw 行含状态 Debug 名（复制源形态）。
#[test]
fn plan_update_empty_plan_host_id_and_chinese_steps() {
    let cell = new_plan_update(
        "call-plan-empty".to_string(),
        UpdatePlanArgs { explanation: None, plan: vec![] },
    );
    let rendered = render_lines(&cell.display_lines(/*width*/ 40)).join("\n");
    assert!(rendered.contains("Updated Plan"), "标题：{rendered}");
    assert!(rendered.contains("(no steps provided)"), "空计划提示：{rendered}");
    assert_eq!(cell.activity_ids(), vec!["call-plan-empty".to_string()]);

    let cell = new_plan_update(
        "hist-7".to_string(),
        UpdatePlanArgs {
            explanation: Some("中文说明：两步完成".into()),
            plan: vec![
                PlanItemArg { step: "接入计划工具".into(), status: StepStatus::Completed },
                PlanItemArg { step: "验证中文步骤显示".into(), status: StepStatus::InProgress },
                PlanItemArg { step: "收尾".into(), status: StepStatus::Pending },
            ],
        },
    );
    let rendered = render_lines(&cell.display_lines(/*width*/ 40)).join("\n");
    assert!(rendered.contains("中文说明：两步完成"), "说明：{rendered}");
    assert!(rendered.contains("✔ 接入计划工具"), "完成标记：{rendered}");
    assert!(rendered.contains("□ 验证中文步骤显示"), "进行中标记：{rendered}");
    assert!(rendered.contains("□ 收尾"), "待办标记：{rendered}");
    let raw = render_lines(&cell.raw_lines()).join("\n");
    assert!(raw.contains("InProgress: 验证中文步骤显示"), "raw 状态名：{raw}");
}
