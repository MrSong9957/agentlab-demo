//! feedback 单元测试（A27/A28 锚＝上游 `citations_tests.rs`/`usage_tests.rs`
//! 移植＋本地形状检查与提取）。

use super::*;

fn block(ids: &str) -> String {
    format!(
        "<oai-mem-citation>\n<citation_entries>\nMEMORY.md:10-14|note=[used build procedure]\n</citation_entries>\n<rollout_ids>\n{ids}\n</rollout_ids>\n</oai-mem-citation>"
    )
}

/// A27 锚（上游 citations_tests）：条目＋ID 解析、跨块 ID 去重。
#[test]
fn parses_entries_and_ids_with_cross_block_dedup() {
    let citation = parse_memory_citation(vec![
        block("auto-20261019T101112-4321\nauto-20261019T101112-4322"),
        block("auto-20261019T101112-4321"),
    ])
    .expect("有条目与 ID 时必须解析出引用");
    assert_eq!(
        citation.entries,
        vec![
            MemoryCitationEntry {
                path: "MEMORY.md".into(),
                line_start: 10,
                line_end: 14,
                note: "used build procedure".into(),
            },
            MemoryCitationEntry {
                path: "MEMORY.md".into(),
                line_start: 10,
                line_end: 14,
            note: "used build procedure".into(),
            }
        ],
        "条目跨块不去重（上游语义——仅 rollout_ids 去重）"
    );
    assert_eq!(
        citation.rollout_ids,
        vec![
            "auto-20261019T101112-4321".to_string(),
            "auto-20261019T101112-4322".to_string()
        ],
        "重复 ID 跨块去重且保持首现顺序"
    );
}

/// A27 锚：双空→None；仅条目→Some（IDs 空）。
#[test]
fn empty_blocks_return_none_and_entries_only_is_some() {
    let neither = "<oai-mem-citation>\n</oai-mem-citation>".to_string();
    assert_eq!(parse_memory_citation(vec![neither]), None);
    let entries_only =
        "<oai-mem-citation>\n<citation_entries>\nMEMORY.md:1-2|note=[x]\n</citation_entries>\n</oai-mem-citation>"
            .to_string();
    let citation = parse_memory_citation(vec![entries_only]).expect("仅条目也构成引用");
    assert!(citation.rollout_ids.is_empty());
    assert_eq!(citation.entries.len(), 1);
}

/// A27 锚：旧 `<thread_ids>` 标签同样接受（上游兼容语义）。
#[test]
fn legacy_thread_ids_tag_accepted() {
    let legacy = "<oai-mem-citation>\n<thread_ids>\nauto-20261019T101112-4321\n</thread_ids>\n</oai-mem-citation>".to_string();
    let citation = parse_memory_citation(vec![legacy]).expect("旧标签可解析");
    assert_eq!(citation.rollout_ids, vec!["auto-20261019T101112-4321".to_string()]);
}

/// A27 锚（上游 entry 解析）：格式缺失任一段→该行跳过；note 保留原文。
#[test]
fn entry_lines_parse_or_skip() {
    let body = "<oai-mem-citation>\n<citation_entries>\nMEMORY.md:234-236|note=[responsesapi citation extraction code pointer]\nrollout_summaries/2026-02-17.md:10-12|note=[weekly report format]\n缺冒号的行\npath-no-range|note=[x]\nMEMORY.md:1-2|note=[未闭合\n</citation_entries>\n</oai-mem-citation>".to_string();
    let citation = parse_memory_citation(vec![body]).expect("有条目即构成引用");
    assert_eq!(citation.entries.len(), 2, "仅两行合法条目被解析");
    assert_eq!(citation.entries[0].line_start, 234);
    assert_eq!(citation.entries[0].line_end, 236);
    assert_eq!(citation.entries[1].path, "rollout_summaries/2026-02-17.md");
    assert_eq!(citation.entries[1].note, "weekly report format");
}

#[test]
fn extracts_blocks_from_reply_text() {
    assert!(extract_citation_blocks("没有引用的回复").is_empty());
    let reply = format!("答案在此。\n{}\n收尾。", block("auto-20261019T101112-4321"));
    let blocks = extract_citation_blocks(&reply);
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].contains("rollout_ids"));
}

#[test]
fn local_thread_id_shape_check() {
    assert!(is_local_thread_id("auto-20261019T101112-4321"), "真实会话格式");
    assert!(is_local_thread_id("auto-synthetic-cobalt"), "合成夹具风格同属 auto- 命名空间");
    assert!(!is_local_thread_id(""), "空 ID 非法");
    assert!(!is_local_thread_id("MEMORY.md"), "非 auto- 前缀非法");
    assert!(!is_local_thread_id("auto-/etc/passwd"), "路径分隔符非法");
    assert!(!is_local_thread_id("auto-has space"), "空白非法");
    assert!(!is_local_thread_id("auto-UPPER"), "大写不在安全字符集");
    assert!(!is_local_thread_id(&format!("auto-{}", "a".repeat(128))), "超长非法");
}

#[test]
fn cited_ids_filter_invalid_and_dedup() {
    let reply = format!(
        "回复。\n{}",
        block("auto-20261019T101112-4321\nnot-a-session\nauto-20261019T101112-4321\nauto-20261019T101112-4322\n")
    );
    assert_eq!(
        cited_thread_ids(&reply),
        vec![
            "auto-20261019T101112-4321".to_string(),
            "auto-20261019T101112-4322".to_string()
        ],
        "非法形状过滤＋重复去重"
    );
    assert!(cited_thread_ids("无引用回复").is_empty());
}

/// A28 面：分类覆盖五类产物（`memories_v2/` 归一），但仅是观测语义。
#[test]
fn usage_kind_classifies_memory_paths_only() {
    assert_eq!(
        memory_usage_kind_for_path("/x/memories/MEMORY.md"),
        Some(MemoriesUsageKind::MemoryMd)
    );
    assert_eq!(
        memory_usage_kind_for_path("memories/memory_summary.md"),
        Some(MemoriesUsageKind::MemorySummary)
    );
    assert_eq!(
        memory_usage_kind_for_path("memories/raw_memories.md"),
        Some(MemoriesUsageKind::RawMemories)
    );
    assert_eq!(
        memory_usage_kind_for_path("memories/rollout_summaries/a.md"),
        Some(MemoriesUsageKind::RolloutSummaries)
    );
    assert_eq!(
        memory_usage_kind_for_path("memories_v2/skills/b.md"),
        Some(MemoriesUsageKind::Skills),
        "v2 路径归一后分类"
    );
    assert_eq!(memory_usage_kind_for_path("src/main.rs"), None);
    assert_eq!(memory_usage_kind_for_path("memories/other.txt"), None);
    assert_eq!(MemoriesUsageKind::MemoryMd.as_tag(), "memory_md");
}
