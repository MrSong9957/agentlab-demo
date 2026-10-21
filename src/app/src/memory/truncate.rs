//! 013-M3：输入/输出预算策略（D11）。
//!
//! 底层中间截断与近似换算**直接复用 008 已落的 shim `codex-utils-string`**
//! （`src/shims/codex-utils-string/src/truncate.rs`＝上游
//! `utils/string/src/truncate.rs` 逐字节拷贝，含上游测试锚——不重复移植）。
//! 本模块仅承载上游 `protocol/src/protocol.rs::TruncationPolicy`（Bytes/Tokens
//! 预算换算）与 `utils/output-truncation/src/lib.rs::truncate_text` 的收窄
//! 入口；`with_serialization_allowance`/MCP/function-output 截断属工具循环面，
//! 不在记忆管线，未移植（登记）。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）。

use codex_utils_string::truncate_middle_chars;
use codex_utils_string::truncate_middle_with_token_budget;

/// 输出/输入预算策略（上游 `TruncationPolicy` 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationPolicy {
    Bytes(usize),
    Tokens(usize),
}

impl TruncationPolicy {
    pub fn token_budget(&self) -> usize {
        match self {
            TruncationPolicy::Bytes(bytes) => {
                usize::try_from(approx_tokens_from_byte_count(*bytes)).unwrap_or(usize::MAX)
            }
            TruncationPolicy::Tokens(tokens) => *tokens,
        }
    }

    pub fn byte_budget(&self) -> usize {
        match self {
            TruncationPolicy::Bytes(bytes) => *bytes,
            TruncationPolicy::Tokens(tokens) => approx_bytes_for_tokens(*tokens),
        }
    }
}

/// 按策略截断：Bytes→中间截到字节预算；Tokens→中间截到近似 token 预算。
/// 两者都保留首尾、替换中段（上游 `truncate_text`）。
pub fn truncate_text(content: &str, policy: TruncationPolicy) -> String {
    match policy {
        TruncationPolicy::Bytes(bytes) => truncate_middle_chars(content, bytes),
        TruncationPolicy::Tokens(tokens) => truncate_middle_with_token_budget(content, tokens).0,
    }
}

pub use codex_utils_string::{
    approx_bytes_for_tokens, approx_token_count, approx_tokens_from_byte_count,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_budgets() {
        assert_eq!(TruncationPolicy::Bytes(100).byte_budget(), 100);
        assert_eq!(TruncationPolicy::Tokens(10).byte_budget(), 40);
        assert_eq!(TruncationPolicy::Tokens(10).token_budget(), 10);
        assert_eq!(TruncationPolicy::Bytes(40).token_budget(), 10);
    }

    #[test]
    fn truncate_text_dispatches_by_policy() {
        let s = "x".repeat(50);
        let by_bytes = truncate_text(&s, TruncationPolicy::Bytes(10));
        assert!(by_bytes.contains("chars truncated"), "{by_bytes}");
        let by_tokens = truncate_text(&s, TruncationPolicy::Tokens(2));
        assert!(by_tokens.contains("tokens truncated"), "{by_tokens}");
        // 预算内原样返回。
        assert_eq!(truncate_text("short", TruncationPolicy::Bytes(100)), "short");
    }

    #[test]
    fn approx_conversions() {
        assert_eq!(approx_token_count("abcd"), 1);
        assert_eq!(approx_token_count("abcde"), 2);
        assert_eq!(approx_bytes_for_tokens(3), 12);
        assert_eq!(approx_tokens_from_byte_count(5), 2);
    }
}
