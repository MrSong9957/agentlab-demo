//! 母本 protocol/src/plan_tool.rs（codex@d138398）：update_plan 参数三类型。
//! 保留字段、枚举与 Serde 属性（snake_case 状态、deny_unknown_fields、
//! explanation 可省略）；省去本项目不用的 JsonSchema／TS 派生与依赖。
//! 017：后端（terminal-chat::plan）与 TUI（计划 cell）共用本类型，
//! 避免两侧各维护一套参数和状态。

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanItemArg {
    pub step: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlanArgs {
    /// Arguments for the `update_plan` todo/checklist tool (not plan mode).
    #[serde(default)]
    pub explanation: Option<String>,
    pub plan: Vec<PlanItemArg>,
}
