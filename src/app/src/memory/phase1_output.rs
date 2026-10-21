//! 013-M3：第一阶段输出合同（V1）解析。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——`memories/write/src/phase1_output.rs` 的 V1 分支移植：
//! `LegacyOutput`（`deny_unknown_fields`＋`rollout_slug` 可缺省可空）与
//! 逐字段秘密替换。V2 分支（SummaryOutput＋9k 截断）不纳入（013 范围），
//! `output_schema`（Responses API 端点参数）无本地对应面——结构约束由本
//! 解析器承担（登记差异，见 013 §6-M3）。

use serde::Deserialize;

use crate::memory::secrets::redact_secrets;

/// 提炼输出（V1 三字段；`raw_memory` 为 `Some`——空字符串判定属调用方三态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOneOutput {
    pub raw_memory: Option<String>,
    pub rollout_summary: String,
    pub rollout_slug: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyOutput {
    raw_memory: String,
    rollout_summary: String,
    #[serde(default)]
    rollout_slug: Option<String>,
}

impl StageOneOutput {
    /// 严格解析＋秘密替换（上游 V1 `StageOneOutput::parse`）：
    /// 非 JSON、缺字段、多字段、类型错误一律失败——不自建输出修复规则。
    pub fn parse(source: &str) -> Result<Self, String> {
        let output: LegacyOutput = serde_json::from_str(source)
            .map_err(|e| format!("输出合同解析失败：{e}"))?;
        Ok(StageOneOutput {
            raw_memory: Some(redact_secrets(output.raw_memory)),
            rollout_summary: redact_secrets(output.rollout_summary),
            rollout_slug: output.rollout_slug.map(redact_secrets),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_v1_output_with_null_and_absent_slug() {
        let a = StageOneOutput::parse(r#"{"raw_memory":"m","rollout_summary":"s","rollout_slug":null}"#).unwrap();
        assert_eq!(a.raw_memory.as_deref(), Some("m"));
        assert_eq!(a.rollout_slug, None);
        let b = StageOneOutput::parse(r#"{"raw_memory":"m","rollout_summary":"s"}"#).unwrap();
        assert_eq!(b.rollout_slug, None, "缺省 slug 可解析（上游 default）");
        let c = StageOneOutput::parse(r#"{"raw_memory":"","rollout_summary":""}"#).unwrap();
        assert_eq!(c.raw_memory.as_deref(), Some(""), "空字符串原样解析（三态判定在上层）");
    }

    #[test]
    fn rejects_contract_violations() {
        for bad in [
            "not json",
            r#"{"rollout_summary":"s"}"#,
            r#"{"raw_memory":"m"}"#,
            r#"{"raw_memory":"m","rollout_summary":"s","extra":1}"#,
            r#"{"raw_memory":"m","rollout_summary":123}"#,
            r#"{"raw_memory":null,"rollout_summary":"s"}"#,
        ] {
            assert!(StageOneOutput::parse(bad).is_err(), "必须拒绝：{bad}");
        }
    }

    #[test]
    fn redacts_every_field() {
        let out = StageOneOutput::parse(
            r#"{"raw_memory":"k sk-ABCDEFGHIJKLMNOPQRSTUVwx","rollout_summary":"t sk-ABCDEFGHIJKLMNOPQRSTUVwx","rollout_slug":"sk-ABCDEFGHIJKLMNOPQRSTUVwx"}"#,
        )
        .unwrap();
        assert_eq!(out.raw_memory.as_deref(), Some("k [REDACTED_SECRET]"));
        assert_eq!(out.rollout_summary, "t [REDACTED_SECRET]");
        assert_eq!(out.rollout_slug.as_deref(), Some("[REDACTED_SECRET]"));
    }
}
