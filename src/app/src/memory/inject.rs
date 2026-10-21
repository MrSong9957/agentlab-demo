//! 013-M6：V1 摘要读取。协议角色包装由 prompt 层负责。
//!
//! 来源：openai/codex @ ce0323f2f2d3b25e82806cd555237da3db4e1897
//! `codex-rs/ext/memories/src/prompts.rs`（Apache-2.0），同步文件读取适配。

use super::config::MemoriesConfig;
use super::truncate::{truncate_text, TruncationPolicy};
use std::path::Path;

// 上游同提交 `ext/memories/templates/memories/read_path.md`；仅修正规格
// §2.2 登记的 Markdown 摘要与原始 JSONL 说明（相关三行），其余原文保留。
const READ_PATH_TEMPLATE: &str = include_str!("templates/read_path.md");

/// 固定上游 `ext/memories/src/lib.rs` 原值；仅约束摘要保留正文，
/// 截断标记、固定指引及协议包装在此预算之外。
pub const MEMORY_SUMMARY_TOKEN_LIMIT: usize = 2_500;

/// 读取可选记忆上下文；缺失/读取失败不创建目录或数据库。
pub fn read_memory_context(root: &Path, config: &MemoriesConfig) -> Option<String> {
    if !config.enabled || !config.use_memories {
        return None;
    }
    let summary = std::fs::read_to_string(root.join("memory_summary.md")).ok()?;
    let summary = truncate_text(
        summary.trim(),
        TruncationPolicy::Tokens(MEMORY_SUMMARY_TOKEN_LIMIT),
    );
    if summary.is_empty() {
        return None;
    }
    let base_path = root.display().to_string();
    let (before, after) = READ_PATH_TEMPLATE
        .split_once("{{ memory_summary }}")
        .expect("内置读取模板含唯一摘要占位符");
    // 只渲染模板，元数据与摘要中的模板样式文本不被再次展开。
    Some(format!(
        "{}{}{}",
        before.replace("{{ base_path }}", &base_path),
        summary,
        after.replace("{{ base_path }}", &base_path)
    ))
}
