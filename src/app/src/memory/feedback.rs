//! 013-M8 引用反馈：解析回复中的记忆引用并记录来源使用计数。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! `memories/read/src/{citations.rs,usage.rs}`（Apache-2.0）——解析/去重/
//! 非法 ID 处理语义与测试锚（A27/A28）移植；`state` 运行时的
//! `record_stage1_output_usage` 落在 [`crate::memory::store`]。
//!
//! A28（指标分离，手册 §12.3）：[`memory_usage_kind_for_path`] 的读取分类
//! 只是观测语义，**从不进入使用计数**——计数唯一来源＝回复中的
//! `<oai-mem-citation>` 引用。上游的 shell 脚本解析器未移植（本地无遥测
//! 消费方），分类以路径为输入保留同等语义。

use std::path::Path;

use crate::memory::config::MemoriesConfig;
use crate::memory::runner::memories_root;
use crate::memory::store::MemoryStore;

/// 引用条目（渲染面；上游 `MemoryCitationEntry` 的反馈所需字段）。
#[derive(Debug, PartialEq, Eq)]
pub struct MemoryCitationEntry {
    pub path: String,
    pub line_start: u64,
    pub line_end: u64,
    pub note: String,
}

/// 一条回复中解析出的记忆引用（上游 `MemoryCitation`）。
#[derive(Debug, PartialEq, Eq, Default)]
pub struct MemoryCitation {
    pub entries: Vec<MemoryCitationEntry>,
    pub rollout_ids: Vec<String>,
}

/// 解析引用块文本（上游 `parse_memory_citation`：多块合并、`rollout_ids`
/// 跨块去重保持首现顺序、兼容旧 `<thread_ids>` 标签、条目与 ID 双空→None）。
pub fn parse_memory_citation(citations: Vec<String>) -> Option<MemoryCitation> {
    use std::collections::HashSet;

    let mut entries = Vec::new();
    let mut rollout_ids = Vec::new();
    let mut seen_rollout_ids = HashSet::new();

    for citation in citations {
        if let Some(entries_block) =
            extract_block(&citation, "<citation_entries>", "</citation_entries>")
        {
            entries.extend(entries_block.lines().filter_map(parse_memory_citation_entry));
        }

        if let Some(ids_block) = extract_ids_block(&citation) {
            for id in ids_block.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if seen_rollout_ids.insert(id.to_string()) {
                    rollout_ids.push(id.to_string());
                }
            }
        }
    }

    if entries.is_empty() && rollout_ids.is_empty() {
        None
    } else {
        Some(MemoryCitation {
            entries,
            rollout_ids,
        })
    }
}

/// 条目行解析（上游 `parse_memory_citation_entry`：
/// `<path>:<start>-<end>|note=[<note>]`，任一段缺失→该行跳过）。
fn parse_memory_citation_entry(line: &str) -> Option<MemoryCitationEntry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let (location, note) = line.rsplit_once("|note=[")?;
    let note = note.strip_suffix(']')?.trim().to_string();
    let (path, line_range) = location.rsplit_once(':')?;
    let (line_start, line_end) = line_range.split_once('-')?;

    Some(MemoryCitationEntry {
        path: path.trim().to_string(),
        line_start: line_start.trim().parse().ok()?,
        line_end: line_end.trim().parse().ok()?,
        note,
    })
}

fn extract_block<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let (_, rest) = text.split_once(open)?;
    let (body, _) = rest.split_once(close)?;
    Some(body)
}

fn extract_ids_block(text: &str) -> Option<&str> {
    extract_block(text, "<rollout_ids>", "</rollout_ids>")
        .or_else(|| extract_block(text, "<thread_ids>", "</thread_ids>"))
}

/// 从回复正文提取全部 `<oai-mem-citation>…</oai-mem-citation>` 块原文
/// （read_path 指引要求最终回复末尾附一个块；多余块按上游多块合并处理）。
pub fn extract_citation_blocks(reply: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = reply;
    while let Some(start) = rest.find("<oai-mem-citation>") {
        rest = &rest[start..];
        match rest.find("</oai-mem-citation>") {
            Some(end) => {
                blocks.push(rest[..end + "</oai-mem-citation>".len()].to_string());
                rest = &rest[end + "</oai-mem-citation>".len()..];
            }
            None => break, // 未闭合块不解析（残缺回复按无引用处理）
        }
    }
    blocks
}

/// 本地会话 ID 形状检查（上游 `ThreadId::try_from` UUID 严格格式的本地
/// 对应＝`auto-` 命名空间安全字符集；权威忽略仍由 SQL 缺行承担——
/// 形状合法但库中无行的 ID 不产生任何更新）。
pub fn is_local_thread_id(id: &str) -> bool {
    id.len() <= 128 && local_thread_id_regex().is_match(id)
}

fn local_thread_id_regex() -> &'static regex_lite::Regex {
    static RE: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // 真实会话 stem＝`auto-YYYYMMDDTHHMMSS-<pid>`（007 自动命名空间）；
        // 合成夹具风格（auto-synthetic-cobalt 等小写安全字符）同属命名空间。
        regex_lite::Regex::new(r"^auto-([0-9]{8}T[0-9]{6}-[0-9]+|[a-z0-9][a-z0-9._-]*)$")
            .expect("会话 ID 正则恒合法")
    })
}

/// 回复→去重后的本地会话 ID 列表（提取→解析→形状过滤；去重已在解析
/// 内按上游语义完成）。
pub fn cited_thread_ids(reply: &str) -> Vec<String> {
    parse_memory_citation(extract_citation_blocks(reply))
        .map(|citation| {
            citation
                .rollout_ids
                .into_iter()
                .filter(|id| is_local_thread_id(id))
                .collect()
        })
        .unwrap_or_default()
}

/// 读取分类（上游 `MemoriesUsageKind`：观测指标面，A28——不计数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoriesUsageKind {
    MemoryMd,
    MemorySummary,
    RawMemories,
    RolloutSummaries,
    Skills,
}

impl MemoriesUsageKind {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::MemoryMd => "memory_md",
            Self::MemorySummary => "memory_summary",
            Self::RawMemories => "raw_memories",
            Self::RolloutSummaries => "rollout_summaries",
            Self::Skills => "skills",
        }
    }
}

/// 按路径分类记忆文件读取（上游 `get_memory_usage` 语义：`memories_v2/`
/// 归一后匹配五类产物；非记忆产物→None）。
pub fn memory_usage_kind_for_path(path: &str) -> Option<MemoriesUsageKind> {
    let path = path.replace('\\', "/");
    let version_marker = path.contains("memories_v2/");
    let path = path.replace("memories_v2/", "memories/");
    let _ = version_marker; // 本地仅 V1；归一语义保留与上游一致
    if path.contains("memories/MEMORY.md") {
        Some(MemoriesUsageKind::MemoryMd)
    } else if path.contains("memories/memory_summary.md") {
        Some(MemoriesUsageKind::MemorySummary)
    } else if path.contains("memories/raw_memories.md") {
        Some(MemoriesUsageKind::RawMemories)
    } else if path.contains("memories/rollout_summaries/") {
        Some(MemoriesUsageKind::RolloutSummaries)
    } else if path.contains("memories/skills/") {
        Some(MemoriesUsageKind::Skills)
    } else {
        None
    }
}

/// 用户轮完成位点接线（两驱动共用）：best-effort 解析＋计数。关闭态零
/// 副作用；开库/解析/计数任一失败静默返回（上游「Missing rows are
/// ignored」同级的尽力语义，不打断主回复路径）。
pub fn after_user_turn(user_dir: &Path, config: &MemoriesConfig, reply: &str) {
    after_user_turn_with_clock(
        user_dir,
        config,
        reply,
        crate::memory::store::system_clock(),
    );
}

/// 完成位点共用核心；Clock 与提炼/清理可共用受控时间，不新增配置面。
/// 014 §1：先解析引用——无有效引用零 IO 早退；有引用仅打开**既有**库
/// （[`MemoryStore::open_existing`]——无 `SQLITE_OPEN_CREATE`、不建根、
/// 不建库、不迁移），库不存在或打开失败一律静默跳过，**不回退**到允许
/// 创建的 `open`（reset 删除库后反馈不得重建）。
pub fn after_user_turn_with_clock(
    user_dir: &Path,
    config: &MemoriesConfig,
    reply: &str,
    clock: crate::memory::Clock,
) {
    if !config.enabled {
        return;
    }
    let thread_ids = cited_thread_ids(reply);
    if thread_ids.is_empty() {
        return;
    }
    let root = memories_root(user_dir);
    let Ok(Some(store)) = MemoryStore::open_existing(&root, clock) else {
        return;
    };
    let _ = store.record_stage1_output_usage(&thread_ids);
}

#[cfg(test)]
#[path = "feedback_tests.rs"]
mod tests;
