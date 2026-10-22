//! 017：`update_plan` 工具的最小参数解析与结果处理。
//! 参数三类型复用 `codex-protocol` shim（上游 plan_tool.rs 移植，字段/枚举/
//! Serde 属性原样）；本模块提供：arguments 解析、本地两字段结果
//!（`{"ok":bool,"error":string|null}`）、成功判定与展示投影提取——
//! 工具执行（turn 分派）、行式输出与 TUI/恢复投影共用，不重复解析规则。
//! 计划只是步骤与进度的记录：本模块不执行步骤、不验证完成、不触文件或命令。

use codex_protocol::plan_tool::{PlanItemArg, StepStatus, UpdatePlanArgs};
use serde_json::{json, Value};

/// 成功结果（模型面，入史 content 同文）：成功含义＝本次更新被接受，
/// 等同上游 `Plan updated` 确认；不重复返回整份计划。
pub fn ok_result() -> String {
    json!({ "ok": true, "error": null }).to_string()
}

/// 失败结果：参数错误等具体原因（取消场景复用同一形状，见 turn 层）。
fn err_result(reason: &str) -> String {
    json!({ "ok": false, "error": reason }).to_string()
}

/// 执行一次 `update_plan`：解析成功 → 成功结果；解析失败 → 两字段失败结果。
/// 参数错误按工具结果回传，不升级为协议错误或重试（017 §3.2）。
pub fn execute(arguments: &str) -> String {
    match parse_arguments(arguments) {
        Ok(_) => ok_result(),
        Err(e) => err_result(&e),
    }
}

/// 解析参数（Serde 复用上游规则，017 §3.1）：非法 JSON、未知字段（顶层与
/// 步骤项）、缺字段、错误类型与非法状态均拒绝；空数组、空步骤字符串、
/// 多个 `in_progress` 与省略/`null` 的 `explanation` 按 Serde 语义接受，
/// 不额外加严。
pub fn parse_arguments(arguments: &str) -> Result<UpdatePlanArgs, String> {
    serde_json::from_str::<UpdatePlanArgs>(arguments)
        .map_err(|e| format!("工具参数不是有效 JSON：{e}"))
}

/// 结果是否明确成功：`ok==true` 且 `error==null`（两字段必须显式存在，
/// 缺 `error` 不算「明确成功」——恢复投影据此不推断）。
pub fn result_is_success(result: &str) -> bool {
    match serde_json::from_str::<Value>(result) {
        Ok(v) => v.get("ok") == Some(&Value::Bool(true)) && v.get("error") == Some(&Value::Null),
        Err(_) => false,
    }
}

/// 结果中的失败原因（`error` 为字符串时）；成功或形状不符返回 None。
pub fn result_error(result: &str) -> Option<String> {
    serde_json::from_str::<Value>(result)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
}

/// 成功快照提取（实时与恢复投影共用）：结果明确成功**且**参数可按同一规则
/// 解析时才返回计划；失败、取消、结果未知或参数损坏返回 None（调用方按
/// 错误/文本降级，不画成功清单）。
pub fn success_snapshot(arguments: &str, result: &str) -> Option<UpdatePlanArgs> {
    if result_is_success(result) {
        parse_arguments(arguments).ok()
    } else {
        None
    }
}

/// 展示副本（017 §3.3）：步骤与说明经终端过滤（`display::sanitize_for_terminal`）
/// ——只过滤副本；原始调用参数按现有历史契约由 turn 层原样保存。
pub fn sanitized_for_display(args: &UpdatePlanArgs) -> UpdatePlanArgs {
    UpdatePlanArgs {
        explanation: args
            .explanation
            .as_deref()
            .map(crate::display::sanitize_for_terminal),
        plan: args
            .plan
            .iter()
            .map(|item| PlanItemArg {
                step: crate::display::sanitize_for_terminal(&item.step),
                status: item.status.clone(),
            })
            .collect(),
    }
}

/// 行式成功输出（017 §3.3）：说明与步骤清单，用「待办／进行中／已完成」
/// 标明状态，不依赖颜色；空数组明确显示「暂无步骤」；步骤顺序与原文保留。
/// 文本进入终端前由宿主经 `say`（sanitize）打印。
pub fn cli_lines(args: &UpdatePlanArgs) -> Vec<String> {
    let total = args.plan.len();
    let completed = args
        .plan
        .iter()
        .filter(|i| matches!(i.status, StepStatus::Completed))
        .count();
    let mut lines = vec![if total == 0 {
        "计划已更新（暂无步骤）".to_string()
    } else {
        format!("计划已更新（{completed}/{total} 步完成）")
    }];
    if let Some(expl) = args
        .explanation
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        lines.push(format!("说明：{expl}"));
    }
    for item in &args.plan {
        let status = match item.status {
            StepStatus::Pending => "待办",
            StepStatus::InProgress => "进行中",
            StepStatus::Completed => "已完成",
        };
        lines.push(format!("  [{status}] {}", item.step));
    }
    lines
}

/// 行式失败/取消输出：原因如实，不画清单。
pub fn cli_failure_line(error: &str) -> String {
    format!("计划更新未生效：{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(status: &str) -> PlanItemArg {
        PlanItemArg {
            step: format!("步骤-{status}"),
            status: serde_json::from_value(json!(status)).unwrap(),
        }
    }

    /// §3.1 接受面：三种状态、省略/null 说明、空数组、空步骤、多个进行中。
    #[test]
    fn parse_accepts_upstream_surface() {
        let args = parse_arguments(
            r#"{"explanation":"实现完成，开始验证","plan":[
                {"step":"接入计划工具","status":"completed"},
                {"step":"验证计划显示与恢复","status":"in_progress"},
                {"step":"收尾","status":"pending"}]}"#,
        )
        .unwrap();
        assert_eq!(args.explanation.as_deref(), Some("实现完成，开始验证"));
        assert_eq!(args.plan.len(), 3);
        assert!(matches!(args.plan[0].status, StepStatus::Completed));
        assert!(matches!(args.plan[1].status, StepStatus::InProgress));
        assert!(matches!(args.plan[2].status, StepStatus::Pending));
        assert_eq!(args.plan[1].step, "验证计划显示与恢复");
        // explanation 省略与 null 均为无说明（上游 Serde 语义）
        let a = parse_arguments(r#"{"plan":[]}"#).unwrap();
        assert_eq!(a.explanation, None, "省略 explanation");
        assert!(a.plan.is_empty(), "空数组接受");
        let a = parse_arguments(r#"{"explanation":null,"plan":[]}"#).unwrap();
        assert_eq!(a.explanation, None, "null explanation");
        // 空步骤字符串、多个 in_progress 不额外拒绝
        let a = parse_arguments(
            r#"{"plan":[{"step":"","status":"in_progress"},{"step":"并行","status":"in_progress"}]}"#,
        )
        .unwrap();
        assert_eq!(a.plan[0].step, "");
        assert!(matches!(a.plan[1].status, StepStatus::InProgress));
    }

    /// §3.1 拒绝面：未知字段（顶层/步骤项）、缺字段、错误类型、非法状态、
    /// 非 JSON、非对象。
    #[test]
    fn parse_rejects_invalid_inputs() {
        for bad in [
            r#"{"steps":[]}"#.to_string(), // 顶层未知字段
            r#"{"plan":[{"step":"a","status":"pending","note":"x"}]}"#.to_string(), // 步骤项未知字段
            r#"{"plan":[{"status":"pending"}]}"#.to_string(),   // 缺 step
            r#"{"plan":[{"step":"a"}]}"#.to_string(),           // 缺 status
            r#"{"plan":[{"step":5,"status":"pending"}]}"#.to_string(), // step 类型错
            r#"{"plan":[{"step":"a","status":"blocked"}]}"#.to_string(), // 非法状态
            r#"{"plan":[{"step":"a","status":"Completed"}]}"#.to_string(), // 大小写敏感
            r#"{"explanation":"x"}"#.to_string(),               // 缺 plan
            r#"{"plan":{}}"#.to_string(),                       // plan 非数组
            "not json".to_string(),
            String::new(),
        ] {
            assert!(
                parse_arguments(&bad).is_err(),
                "应拒绝：{bad}"
            );
        }
    }

    /// §3.2 结果形状：成功两字段；失败两字段带原因；参数错误按工具结果回传。
    #[test]
    fn execute_returns_two_field_results() {
        assert_eq!(execute(r#"{"plan":[]}"#), r#"{"ok":true,"error":null}"#);
        let bad = execute(r#"{"plan":[{"step":"a","status":"nope"}]}"#);
        let v: Value = serde_json::from_str(&bad).unwrap();
        assert_eq!(v["ok"], json!(false), "{v}");
        let err = v["error"].as_str().unwrap();
        assert!(!err.is_empty(), "失败结果带具体原因：{v}");
        // 非法 JSON 同为两字段失败，不 panic
        let bad = execute("[");
        let v: Value = serde_json::from_str(&bad).unwrap();
        assert_eq!(v["ok"], json!(false), "{v}");
    }

    /// 成功判定：ok=true+error=null 才算；缺字段/ok 缺省/error 非 null 均不算。
    #[test]
    fn result_success_requires_explicit_ok_and_null_error() {
        assert!(result_is_success(r#"{"ok":true,"error":null}"#));
        assert!(!result_is_success(r#"{"ok":true}"#), "缺 error 不算明确成功");
        assert!(!result_is_success(r#"{"ok":true,"error":"x"}"#));
        assert!(!result_is_success(r#"{"ok":false,"error":null}"#));
        assert!(!result_is_success(r#"{"error":"x"}"#));
        assert!(!result_is_success("not json"));
        assert_eq!(
            result_error(r#"{"ok":false,"error":"具体原因"}"#).as_deref(),
            Some("具体原因")
        );
        assert_eq!(result_error(r#"{"ok":true,"error":null}"#), None);
    }

    /// 成功快照：结果成功且参数可解析才 Some；其余降级 None。
    #[test]
    fn snapshot_only_from_success_and_parseable_arguments() {
        let args = r#"{"explanation":"e","plan":[{"step":"s","status":"completed"}]}"#;
        let snap = success_snapshot(args, r#"{"ok":true,"error":null}"#).unwrap();
        assert_eq!(snap.explanation.as_deref(), Some("e"));
        assert_eq!(snap.plan.len(), 1);
        assert!(matches!(snap.plan[0].status, StepStatus::Completed));
        // 参数损坏（结果却成功——历史损坏/手改）：不推断成功
        assert!(success_snapshot("corrupt", r#"{"ok":true,"error":null}"#).is_none());
        assert!(success_snapshot(args, r#"{"ok":false,"error":"x"}"#).is_none());
        assert!(success_snapshot(args, "not json").is_none());
        assert!(success_snapshot(args, r#"{"ok":true}"#).is_none());
    }

    /// 展示副本过滤：控制序列剥离，原始参数不动（副本≠原件）。
    #[test]
    fn display_copy_sanitized_but_original_untouched() {
        // JSON 字符串层以 \u 转义注入终端控制序列（ESC＝清屏/标题伪造、BEL＝OSC 终止）
        let raw = "{\"explanation\":\"说\\u001b[2J明\",\"plan\":[{\"step\":\"步\\u0007骤\",\"status\":\"pending\"}]}";
        let args = parse_arguments(raw).unwrap();
        let display = sanitized_for_display(&args);
        assert_eq!(display.explanation.as_deref(), Some("说明"));
        assert_eq!(display.plan[0].step, "步骤");
        // 原件保留原始字节（历史/模型契约不变）
        let orig = parse_arguments(&raw).unwrap();
        assert!(orig.explanation.as_deref().unwrap().contains('\u{1b}'));
        assert!(orig.plan[0].step.contains('\u{7}'));
    }

    /// 行式输出：计数头＋说明＋三种状态标签＋原文顺序；空数组「暂无步骤」。
    #[test]
    fn cli_lines_statuses_and_empty_plan() {
        let args = UpdatePlanArgs {
            explanation: Some("实现完成，开始验证".into()),
            plan: vec![
                step("completed"),
                step("in_progress"),
                step("pending"),
            ],
        };
        assert_eq!(
            cli_lines(&args),
            vec![
                "计划已更新（1/3 步完成）".to_string(),
                "说明：实现完成，开始验证".to_string(),
                "  [已完成] 步骤-completed".to_string(),
                "  [进行中] 步骤-in_progress".to_string(),
                "  [待办] 步骤-pending".to_string(),
            ]
        );
        // 空 explanation（空串/空白）不输出说明行
        let args = UpdatePlanArgs {
            explanation: Some("   ".into()),
            plan: vec![],
        };
        assert_eq!(cli_lines(&args), vec!["计划已更新（暂无步骤）".to_string()]);
        let args = UpdatePlanArgs { explanation: None, plan: vec![] };
        assert_eq!(cli_lines(&args), vec!["计划已更新（暂无步骤）".to_string()]);
        // 失败行
        assert_eq!(cli_failure_line("用户取消，本调用未执行"), "计划更新未生效：用户取消，本调用未执行");
    }
}
