//! 013-M3：提炼输入组装（会话记录 → 模型输入文本）。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——`memories/write/src/rollout_input.rs` 函数级移植：
//! 读取边界（总量 8 MiB、单条 1 MiB、先从最近向前扫描再恢复来源顺序、
//! 头部须为有效会话元数据）与 V1 序列化路径（过滤→JSON 数组→整体脱敏，
//! `phase1.rs::serialize_filtered_rollout_response_items`）。适配：
//! ①输入类型＝本地 `HistoryItem`（text/tool_calls/tool_result 三形态），
//! 上游的 InterAgent/Context 碎片等 V2 概念在本地会话不存在（§2.2-7
//! 「宿主注入不落会话」＝免剥离差异，标记片段匹配保留为防御）；②反向
//! 扫描以分块读实现（无上游 seekable/compressed reader 层，语义等价：
//! 从文件尾向前、超限跳过、预算尽即停）；③`tool_result.display` 展示数据
//! 经投影天然不进入序列化（012 契约）。

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::memory::secrets::redact_secrets;
use crate::prompt::{HistoryItem, HistoryMessage, HistoryRole};

/// 近期记录读取预算（处理后的记录字节总量上限）。
pub const MAX_ROLLOUT_BYTES: usize = 8 * 1024 * 1024;
/// 单条记录上限；超限记录跳过。
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;

/// 反向扫描块大小（内存有界；与上游逐记录反向扫描等价的分块实现）。
const SCAN_CHUNK_BYTES: usize = 64 * 1024;

/// 读取近期会话记录：保留能装进预算的最新记录，恢复来源顺序（旧→新）。
///
/// - 首行必须在单条上限内是有效会话元数据（`{"v":1}`），否则整体拒绝；
/// - 从文件尾向前扫描：超限单条与非 UTF-8/坏行跳过，预算耗尽即停止
///   （更旧记录不再读取）；
/// - 预算按序列化后的记录字节数计（上游 `serde_json::to_vec(&item).len()`）。
pub fn read_recent_rollout_items(path: &Path) -> Result<Vec<HistoryItem>, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("打开会话 {} 失败：{e}", path.display()))?;
    validate_session_header(&mut file)?;

    let len = file
        .metadata()
        .map_err(|e| format!("读取会话大小失败：{e}"))?
        .len() as usize;

    let mut items: Vec<HistoryItem> = Vec::new(); // 新→旧暂存，末尾反转
    let mut remaining = MAX_ROLLOUT_BYTES;
    let mut scan_end = len;
    // 最新侧尚未见到行尾 `\n` 的残段（其更旧部分待下轮块拼接）。
    let mut pending: Vec<u8> = Vec::new();

    while scan_end > 0 {
        let start = scan_end.saturating_sub(SCAN_CHUNK_BYTES);
        let mut window = read_range(&mut file, start, scan_end)?;
        window.extend_from_slice(&pending);
        pending.clear();

        let newlines: Vec<usize> = window
            .iter()
            .enumerate()
            .filter(|(_, &b)| b == b'\n')
            .map(|(i, _)| i)
            .collect();
        if newlines.is_empty() {
            pending = window;
        } else {
            // 从最新完整行向前逐行处理；任一行的预算耗尽即整体停止。
            let mut cursor = window.len();
            let mut exhausted = false;
            for &nl in newlines.iter().rev() {
                if !process_line(&window[nl + 1..cursor], &mut items, &mut remaining) {
                    exhausted = true;
                    break;
                }
                cursor = nl;
            }
            if exhausted {
                pending.clear();
                break;
            }
            // 首个换行之前的部分＝更旧侧行的已知残段。
            pending = window[..newlines[0]].to_vec();
        }
        scan_end = start;
    }
    // 文件首行（无更旧字节可拼）。
    if !pending.is_empty() {
        process_line(&pending, &mut items, &mut remaining);
    }

    items.reverse();
    Ok(items)
}

fn read_range(file: &mut std::fs::File, start: usize, end: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; end - start];
    file.seek(SeekFrom::Start(start as u64))
        .map_err(|e| format!("会话定位失败：{e}"))?;
    file.read_exact(&mut buf)
        .map_err(|e| format!("会话读取失败：{e}"))?;
    Ok(buf)
}

/// 头部校验：首行须在单条上限内是有效 meta（`{"v":1}`，007 契约）。
fn validate_session_header(file: &mut std::fs::File) -> Result<(), String> {
    let mut buf = Vec::new();
    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("会话定位失败：{e}"))?;
    (&*file)
        .take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("会话读取失败：{e}"))?;
    let line_len = buf.iter().position(|&b| b == b'\n').unwrap_or(buf.len());
    if line_len > MAX_RECORD_BYTES {
        return Err("提炼要求会话元数据在单条上限内".to_string());
    }
    let line = std::str::from_utf8(&buf[..line_len])
        .map_err(|_| "提炼要求会话元数据为有效 UTF-8".to_string())?;
    let meta: serde_json::Value = serde_json::from_str(line)
        .map_err(|_| "提炼要求会话元数据在单条上限内有效".to_string())?;
    match meta.get("v").and_then(|v| v.as_i64()) {
        Some(1) => Ok(()),
        _ => Err("提炼要求会话元数据在单条上限内有效".to_string()),
    }
}

/// 处理一行（从新到旧调用）：超限/非 UTF-8/坏行跳过；预算耗尽返回 false
/// （停止扫描，更旧记录不再读取）。
fn process_line(line: &[u8], items: &mut Vec<HistoryItem>, remaining: &mut usize) -> bool {
    if line.len() > MAX_RECORD_BYTES {
        return true;
    }
    let Ok(text) = std::str::from_utf8(line) else {
        return true;
    };
    let Ok(item) = serde_json::from_str::<HistoryItem>(text) else {
        return true;
    };
    let serialized_len = serde_json::to_vec(&item).map(|v| v.len()).unwrap_or(usize::MAX);
    let Some(next) = remaining.checked_sub(serialized_len) else {
        return false;
    };
    *remaining = next;
    items.push(item);
    true
}

/// 序列化过滤后的提炼输入（上游 V1 路径 `serialize_filtered_rollout_response_items`）：
/// 过滤→逐条投影为模型面 JSON→数组序列化→整体秘密替换。
/// `tool_result.display` 展示数据经投影排除（012：绝不进入模型请求）。
/// 过滤时机与上游差异：上游在读取侧剔除注入消息（不占 8 MiB 预算），本地
/// 在序列化侧过滤（已计入预算）——本地会话结构性不含注入块（§2.2-7），该
/// 差异当前影响为零，登记备查。
pub fn serialize_filtered_history_items(items: &[HistoryItem]) -> Result<String, String> {
    let filtered: Vec<HistoryItem> = items
        .iter()
        .filter_map(sanitize_history_item_for_memories)
        .collect();
    let projected = crate::prompt::project_history(&filtered);
    let serialized = serde_json::to_string(&projected)
        .map_err(|e| format!("会话记录序列化失败：{e}"))?;
    Ok(redact_secrets(serialized))
}

/// 上游 `sanitize_response_item_for_memories` 的本地映射：剔除不应当作用户
/// 事实的注入说明（AGENTS.md 指令块/技能块——上游按内容部件剥离，本地消息
/// 为单一文本，整条匹配即整条剔除）。上游对 user 消息清 status/加密内容，
/// 本地 `HistoryMessage` 无这些字段（无对应概念）。
pub fn sanitize_history_item_for_memories(item: &HistoryItem) -> Option<HistoryItem> {
    match item {
        HistoryItem::Text(HistoryMessage { role, content, ..})
            if matches!(role, HistoryRole::User) && is_memory_excluded_fragment(content) =>
        {
            None
        }
        // 019 §5.2：图片载荷先剥离再走记忆投影——base64 不进入提炼文本、
        // 记忆 Markdown 或摘要注入；工具文本摘要保留。读取预算不变（超单条
        // 上限的内联图片记录仍按既有规则在读取侧整条跳过）。
        HistoryItem::ToolResult { tool_call_id, content, display, image } if image.is_some() => {
            Some(HistoryItem::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: content.clone(),
                display: display.clone(),
                image: None,
            })
        }
        _ => Some(item.clone()),
    }
}

fn is_memory_excluded_fragment(text: &str) -> bool {
    matches_marked_fragment(text, "# AGENTS.md instructions", "</INSTRUCTIONS>")
        || matches_marked_fragment(text, "<skill>", "</skill>")
        // 015：用户轮上下文消息（AGENTS＋环境合并块，可带替换声明）——合并块
        // 以 </environment_context> 结尾，不命中上文「以 </INSTRUCTIONS> 结尾」
        // 的整条匹配；复用 015 同一识别器（剥声明→完整结构）剔除，
        // 既有两类剔除行为保持不变。
        || crate::context::context_body(text).is_some()
}

/// 上游 `matches_marked_fragment` 逐字移植：整条消息以起始标记开头、结束
/// 标记结尾（ASCII 大小写不敏感）才算注入块；普通内容不受影响。
fn matches_marked_fragment(text: &str, start_marker: &str, end_marker: &str) -> bool {
    let trimmed = text.trim_start();
    let starts_with_marker = trimmed
        .get(..start_marker.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(start_marker));
    let trimmed = trimmed.trim_end();
    let ends_with_marker = trimmed
        .get(trimmed.len().saturating_sub(end_marker.len())..)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(end_marker));
    starts_with_marker && ends_with_marker
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(content: &str) -> HistoryItem {
        HistoryItem::Text(HistoryMessage {
            role: HistoryRole::User,
            content: content.to_string(),
            user_input_event: None,
        })
    }

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "m3ri-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn header_must_be_valid_meta() {
        let t = Tmp::new("hdr");
        let p = t.0.join("a.jsonl");
        std::fs::write(&p, serde_json::to_string(&user("x")).unwrap()).unwrap();
        assert!(read_recent_rollout_items(&p).is_err());
        std::fs::write(&p, "{\"v\":2}\n").unwrap();
        assert!(read_recent_rollout_items(&p).is_err(), "未知版本整拒");
    }

    /// 015（c9）：用户轮上下文消息（AGENTS＋环境合并块）自提炼输入剔除——
    /// 首轮裸消息与带替换声明消息两种形态；既有 AGENTS-only/`<skill>` 剔除行为不变，
    /// 相似开头但结构不完整的普通用户文本保留。
    #[test]
    fn context_injection_messages_excluded_and_legacy_rules_kept() {
        // 015 真实格式样本（测试侧按规格 §1 独立构造）
        let bare = "# AGENTS.md instructions for /w\n\n<INSTRUCTIONS>\n规则R\n</INSTRUCTIONS>\n\n<environment_context>\n  <cwd>/w</cwd>\n  <shell>/bin/sh</shell>\n  <current_date>2025-10-09</current_date>\n</environment_context>";
        let declared = format!(
            "The following context replaces the previously provided AGENTS.md instructions and environment context.\n\n{bare}"
        );
        let env_only = "<environment_context>\n  <cwd>/w</cwd>\n  <current_date>2025-01-01</current_date>\n</environment_context>";
        for text in [bare, declared.as_str(), env_only] {
            assert!(is_memory_excluded_fragment(text), "015 ctx 应剔除：{text}");
            assert!(sanitize_history_item_for_memories(&user(text)).is_none());
        }
        // 既有行为不变：AGENTS-only（以 </INSTRUCTIONS> 结尾）与 <skill> 仍剔除
        assert!(is_memory_excluded_fragment(
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nA\n</INSTRUCTIONS>"
        ));
        assert!(is_memory_excluded_fragment("<skill>demo</skill>"));
        // 相似开头但结构不完整：不剔除；普通对话原样保留
        assert!(!is_memory_excluded_fragment("# AGENTS.md instructions for /w\n普通粘贴"));
        assert!(!is_memory_excluded_fragment("<environment_context>普通粘贴"));
        let kept = user("真实用户输入");
        assert_eq!(sanitize_history_item_for_memories(&kept), Some(kept.clone()));
    }

    /// 019 §5.2：图片载荷剥离——记忆输入保留工具文本摘要但无 data URL；
    /// 也不为图片记录新增图片提炼轮。
    #[test]
    fn image_payload_stripped_from_memory_input() {
        use crate::prompt::{HistoryItem, ToolResultImage};
        let item = HistoryItem::ToolResult {
            tool_call_id: "c1".into(),
            content: r#"{"ok":true,"path":"/x/a.png"}"#.into(),
            display: None,
            image: Some(ToolResultImage {
                image_url: "data:image/png;base64,QUJDREVGRw==".into(),
                detail: "high".into(),
            }),
        };
        let stripped = sanitize_history_item_for_memories(&item).expect("摘要保留");
        match stripped {
            HistoryItem::ToolResult { content, image, .. } => {
                assert!(image.is_none(), "载荷必须剥离");
                assert!(content.contains(r#""ok":true"#), "摘要保留：{content}");
            }
            other => panic!("形状：{other:?}"),
        }
        let text = serialize_filtered_history_items(&[item]).unwrap();
        assert!(text.contains("a.png"), "摘要进入提炼输入：{text}");
        assert!(!text.contains("base64"), "提炼输入不得含 base64：{text}");
        assert!(!text.contains("QUJDREVGRw"), "载荷不得进入提炼输入：{text}");
    }

    /// 019 §5.2：超单条上限的内联图片记录仍按既有规则整条跳过（连同摘要）。
    #[test]
    fn oversize_inline_image_record_skipped_entirely() {
        use crate::prompt::{HistoryItem, ToolResultImage};
        let t = Tmp::new("big-img");
        let p = t.0.join("g.jsonl");
        let big_b64 = "Q".repeat(MAX_RECORD_BYTES + 64);
        let line = serde_json::to_string(&HistoryItem::ToolResult {
            tool_call_id: "c9".into(),
            content: r#"{"ok":true}"#.into(),
            display: None,
            image: Some(ToolResultImage { image_url: format!("data:image/png;base64,{big_b64}"), detail: "high".into() }),
        })
        .unwrap();
        std::fs::write(&p, format!("{{\"v\":1}}\n{line}\n")).unwrap();
        let items = read_recent_rollout_items(&p).unwrap();
        assert!(items.is_empty(), "超限记录整条跳过（不参与提炼）");
    }

    #[test]
    fn keeps_source_order_and_handles_missing_trailing_newline() {
        let t = Tmp::new("ord");
        let p = t.0.join("b.jsonl");
        // 末行无换行（007 AcceptedNoNewline 形态）也应可读。
        let mut body = String::from("{\"v\":1}\n");
        for c in ["old", "mid"] {
            body.push_str(&serde_json::to_string(&user(c)).unwrap());
            body.push('\n');
        }
        body.push_str(&serde_json::to_string(&user("new")).unwrap());
        std::fs::write(&p, body).unwrap();
        let items = read_recent_rollout_items(&p).unwrap();
        let texts: Vec<&str> = items
            .iter()
            .map(|i| match i {
                HistoryItem::Text(m) => m.content.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(texts, vec!["old", "mid", "new"], "恢复来源顺序，尾行无换行可读");
    }

    #[test]
    fn multi_chunk_reassembly_matches_single_chunk() {
        let t = Tmp::new("chunks");
        let p = t.0.join("c.jsonl");
        let mut body = String::from("{\"v\":1}\n");
        // 4000 × ~2.6KB ≈ 10.4 MiB > 8 MiB 预算：最旧记录被挤出保留集。
        for i in 0..4_000 {
            body.push_str(
                &serde_json::to_string(&user(&format!("m{i}-{}", "z".repeat(2_500)))).unwrap(),
            );
            body.push('\n');
        }
        std::fs::write(&p, &body).unwrap();
        let items = read_recent_rollout_items(&p).unwrap();
        let total: usize = items
            .iter()
            .map(|i| serde_json::to_vec(i).unwrap().len())
            .sum();
        assert!(total <= MAX_ROLLOUT_BYTES);
        assert!(
            total > MAX_ROLLOUT_BYTES - 300_000,
            "预算接近用满（实际 {total}）"
        );
        // 新→旧截取：最旧条目不在保留集，最新保留，来源顺序单调。
        assert!(!items.iter().any(|i| matches!(i, HistoryItem::Text(m) if m.content.starts_with("m0-"))));
        assert!(
            items.iter().any(|i| matches!(i, HistoryItem::Text(m) if m.content.starts_with("m3999-"))),
            "最新条目保留"
        );
        let idx = |needle: &str| {
            items
                .iter()
                .position(|i| matches!(i, HistoryItem::Text(m) if m.content.starts_with(needle)))
                .unwrap()
        };
        assert!(idx("m3000-") < idx("m3999-"), "恢复来源顺序");
    }

    #[test]
    fn bad_lines_skipped() {
        let t = Tmp::new("bad");
        let p = t.0.join("d.jsonl");
        let ok = serde_json::to_string(&user("good")).unwrap();
        std::fs::write(&p, format!("{{\"v\":1}}\nnot-json\n{ok}\n")).unwrap();
        let items = read_recent_rollout_items(&p).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn oversize_record_in_middle_skipped_but_scan_continues() {
        let t = Tmp::new("mid");
        let p = t.0.join("e.jsonl");
        let mut body = String::from("{\"v\":1}\n");
        body.push_str(&serde_json::to_string(&user("old-small")).unwrap());
        body.push('\n');
        // 中间一条超限记录（>1 MiB）。
        body.push_str(&serde_json::to_string(&user(&"b".repeat(MAX_RECORD_BYTES + 64))).unwrap());
        body.push('\n');
        body.push_str(&serde_json::to_string(&user("new-small")).unwrap());
        body.push('\n');
        std::fs::write(&p, body).unwrap();
        let items = read_recent_rollout_items(&p).unwrap();
        let texts: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                HistoryItem::Text(m) => Some(m.content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["old-small", "new-small"], "超限跳过后继续向前扫描更旧记录");
    }

    #[test]
    fn oversize_header_rejected() {
        let t = Tmp::new("bighdr");
        let p = t.0.join("f.jsonl");
        let mut body = "{\"v\":1,".to_string();
        body.push_str(&"x".repeat(MAX_RECORD_BYTES));
        body.push_str("}\n");
        std::fs::write(&p, body).unwrap();
        assert!(
            read_recent_rollout_items(&p).is_err(),
            "会话元数据超单条上限须整拒"
        );
    }
}
