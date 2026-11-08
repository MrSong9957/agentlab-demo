//! 007 会话保存与恢复验收：序列化表征、装载与修复、写入器、命令面。
//! 全部走 `terminal_chat::session` 公共接口与真实文件系统（独立临时目录，
//! TCHAT_USER_DIR 覆盖），不 mock 内部逻辑；网络边界仅在 CLI E2E 用本地 mock。
#![cfg(unix)]
// 038-CI：本文件依赖 POSIX 语义（PTY/Unix fd/文件权限位），Windows 不编译；

mod common;

use common::TempDir;
use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord};
use terminal_chat::session::{load_session_file, TailState, LINE_LIMIT};

fn text(role: HistoryRole, content: &str) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        role,
        content: content.into(),
        user_input_event: None,
        images: None,
        text_elements: Vec::new(),
    })
}

fn batch(id: &str, name: &str, arguments: &str) -> HistoryItem {
    HistoryItem::AssistantToolCalls {
        tool_calls: vec![ToolCallRecord {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }],
        content: None,
    }
}

fn result(id: &str, content: &str) -> HistoryItem {
    HistoryItem::ToolResult {
        tool_call_id: id.into(),
        content: content.into(),
        display: None,
        image: None,
    }
}

fn line(item: &HistoryItem) -> String {
    serde_json::to_string(item).unwrap()
}

/// 落盘一个会话文件（原始字节直写，含坏行/超长行等损坏形态）。
fn seed(path: &std::path::Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn session_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("sessions").join("auto-20260101T000000-1.jsonl")
}

const META: &str = r#"{"v":1}"#;
const UNKNOWN_MSG: &str = "会话中断，该调用可能已执行，执行结果未知";

fn synth(id: &str) -> HistoryItem {
    HistoryItem::ToolResult {
        tool_call_id: id.into(),
        content: format!("{{\"error\":\"{UNKNOWN_MSG}\"}}"),
        display: None,
        image: None,
    }
}

// ---------- 装载：meta 门（§6-A：唯一整文件拒绝情形） ----------

#[test]
fn meta_gate_rejects_bad_meta_and_unknown_version() {
    let dir = TempDir::new("s-meta");
    for (tag, content) in [
        ("空文件", ""),
        ("坏 JSON meta", "{oops\n"),
        // 032 前 v=3 为未知版本；现已是合法版本（上下文检查点），未知上移到 v=4
        ("v=4", "{\"v\":4}\n"),
        ("缺 v", "{\"x\":1}\n"),
        ("非对象 meta", "[1,2]\n"),
    ] {
        let p = session_path(&dir);
        seed(&p, content);
        let err = load_session_file(&p).unwrap_err();
        assert!(err.contains("meta") || err.contains("v"), "{tag} 应整文件拒绝：{err}");
    }
    // v=1/2/3 均为合法版本门（032 起）
    for version in ["{\"v\":1}\n", "{\"v\":2}\n", "{\"v\":3}\n"] {
        let p = session_path(&dir);
        seed(&p, version);
        assert!(load_session_file(&p).is_ok(), "{version} 应通过版本门");
    }
}

#[test]
fn meta_only_empty_session_is_legal() {
    let dir = TempDir::new("s-empty");
    let p = session_path(&dir);
    seed(&p, "{\"v\":1}\n");
    let loaded = load_session_file(&p).unwrap();
    assert!(loaded.history.is_empty());
    assert_eq!(loaded.skipped_lines, 0);
    assert_eq!(loaded.repairs, 0);
    assert_eq!(loaded.tail, TailState::Terminated);
    // 仅 meta、无换行：装载成功（EOF 即行尾），尾态＝末行完整无换行
    seed(&p, META);
    let loaded = load_session_file(&p).unwrap();
    assert!(loaded.history.is_empty());
    assert_eq!(loaded.tail, TailState::AcceptedNoNewline);
}

// ---------- 装载：行容差 ----------

#[test]
fn complete_tail_without_newline_accepted_and_truncated_tail_skipped() {
    let dir = TempDir::new("s-tail");
    let p = session_path(&dir);
    // 完整 JSON 末行无 \n：接受该记录
    seed(
        &p,
        &format!("{META}\n{}\n{}", line(&text(HistoryRole::User, "u1")), line(&text(HistoryRole::Assistant, "a1"))),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(loaded.history, vec![text(HistoryRole::User, "u1"), text(HistoryRole::Assistant, "a1")]);
    assert_eq!(loaded.tail, TailState::AcceptedNoNewline);

    // 末行 JSON 截断：跳过＋计数＋行号
    seed(
        &p,
        &format!("{META}\n{}\n{{\"type\":\"text\",\"role\":\"ass", line(&text(HistoryRole::User, "u1"))),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(loaded.history, vec![text(HistoryRole::User, "u1")]);
    assert_eq!(loaded.skipped_lines, 1);
    assert_eq!(loaded.skipped_line_numbers, vec![3]);
    assert_eq!(loaded.tail, TailState::Bad);
}

#[test]
fn mid_file_bad_line_skipped_and_following_lines_load() {
    let dir = TempDir::new("s-mid");
    let p = session_path(&dir);
    seed(
        &p,
        &format!(
            "{META}\n{}\n{{bad json\n{}\n",
            line(&text(HistoryRole::User, "u1")),
            line(&text(HistoryRole::Assistant, "a1"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![text(HistoryRole::User, "u1"), text(HistoryRole::Assistant, "a1")]
    );
    assert_eq!(loaded.skipped_lines, 1);
    assert_eq!(loaded.skipped_line_numbers, vec![3]);
}

/// 超长物理行（65 MiB 无换行坏行）记超限坏行后有界丢弃，其后正常行照常装载。
/// 生成与装载过程缓冲有界（丢弃模式 64 KiB 块），不整体载入坏行。
#[test]
fn overlong_line_bounded_skip_then_continue() {
    let dir = TempDir::new("s-overlong");
    let p = session_path(&dir);
    let mut content = String::new();
    content.push_str(META);
    content.push('\n');
    content.push_str(&line(&text(HistoryRole::User, "u1")));
    content.push('\n');
    content.push_str(&"{\"type\":\"text\",\"role\":\"user\",\"content\":\"");
    let huge = "x".repeat(65 * 1024 * 1024);
    content.push_str(&huge);
    content.push_str(&format!("\"\n{}\n", line(&text(HistoryRole::Assistant, "after"))));
    seed(&p, &content);
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![text(HistoryRole::User, "u1"), text(HistoryRole::Assistant, "after")]
    );
    assert_eq!(loaded.skipped_lines, 1, "65 MiB 无换行坏行计一次超限");
    assert_eq!(loaded.skipped_line_numbers, vec![3]);
    assert_eq!(loaded.tail, TailState::Terminated);
}

// ---------- 装载：批 id 校验（不信任文件内容） ----------

#[test]
fn duplicate_call_ids_rejected_as_bad_line_and_results_orphaned() {
    let dir = TempDir::new("s-dupid");
    let p = session_path(&dir);
    // 批内重复 id：整行按坏行跳过；其紧随结果按孤儿丢弃（不得归并前批）
    let dup_batch = r#"{"type":"tool_calls","tool_calls":[
        {"id":"d1","name":"exec_command","arguments":"{}"},
        {"id":"d1","name":"exec_command","arguments":"{}"}],"content":null}"#
        .replace('\n', "");
    seed(
        &p,
        &format!(
            "{META}\n{}\n{dup_batch}\n{}\n",
            line(&text(HistoryRole::User, "u1")),
            line(&result("d1", "r1"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(loaded.history, vec![text(HistoryRole::User, "u1")], "重复 id 批与其结果都不入历史");
    assert_eq!(loaded.skipped_lines, 1);
    assert_eq!(loaded.repairs, 1, "孤儿结果丢弃计一处修复");

    // 跨批重复 id：与更早批 id 重复 → 该行按坏行跳过
    seed(
        &p,
        &format!(
            "{META}\n{}\n{}\n{}\n{}\n{}\n",
            line(&text(HistoryRole::User, "u1")),
            line(&batch("c1", "exec_command", "{}")),
            line(&result("c1", "ok")),
            line(&batch("c1", "exec_command", "{}")),
            line(&result("c1", "again"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![
            text(HistoryRole::User, "u1"),
            batch("c1", "exec_command", "{}"),
            result("c1", "ok"),
        ],
        "跨批重复 id 行跳过，第二条结果按孤儿丢弃"
    );
    assert_eq!(loaded.skipped_lines, 1);
    assert_eq!(loaded.repairs, 1);
}

// ---------- 装载：配对修复 ----------

#[test]
fn dangling_calls_synthesize_generic_unknown_result() {
    let dir = TempDir::new("s-dangle");
    let p = session_path(&dir);
    // 副作用可能已发生（006 坑：结果缺失≠未执行）——合成结果只表达"未知"
    seed(
        &p,
        &format!(
            "{META}\n{}\n{}\n",
            line(&text(HistoryRole::User, "touch probe")),
            line(&batch("e1", "exec_command", "{\"cmd\":\"touch probe\"}"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![
            text(HistoryRole::User, "touch probe"),
            batch("e1", "exec_command", "{\"cmd\":\"touch probe\"}"),
            synth("e1"),
        ]
    );
    assert_eq!(loaded.repairs, 1, "合成一处");
    // 文案约束：含"执行结果未知"，禁言"未执行/取消/失败"
    let HistoryItem::ToolResult { content, .. } = &loaded.history[2] else { panic!() };
    assert!(content.contains("执行结果未知"), "{content}");
    assert!(!content.contains("未执行") && !content.contains("取消"), "{content}");
    // 合成结果是合法 JSON（通用单 error 结构）
    let v: serde_json::Value = serde_json::from_str(content).unwrap();
    assert_eq!(v["error"].as_str().unwrap(), UNKNOWN_MSG);
}

#[test]
fn duplicate_results_keep_first_and_count_rest() {
    let dir = TempDir::new("s-dupres");
    let p = session_path(&dir);
    seed(
        &p,
        &format!(
            "{META}\n{}\n{}\n{}\n{}\n",
            line(&text(HistoryRole::User, "u1")),
            line(&batch("k1", "exec_command", "{}")),
            line(&result("k1", "first")),
            line(&result("k1", "second"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![text(HistoryRole::User, "u1"), batch("k1", "exec_command", "{}"), result("k1", "first")],
        "同 id 后续结果丢弃"
    );
    assert_eq!(loaded.repairs, 1);
}

/// 被拒批仍是分组边界：前批悬空调用不得被后批紧随结果满足（伪造结果）。
#[test]
fn rejected_batch_still_group_boundary() {
    let dir = TempDir::new("s-boundary");
    let p = session_path(&dir);
    let dup_batch = serde_json::json!({
        "type": "tool_calls",
        "tool_calls": [
            { "id": "b1", "name": "exec_command", "arguments": "{}" },
            { "id": "b1", "name": "exec_command", "arguments": "{}" }
        ],
        "content": null
    })
    .to_string();
    seed(
        &p,
        &format!(
            "{META}\n{}\n{}\n{dup_batch}\n{}\n",
            line(&text(HistoryRole::User, "u1")),
            line(&batch("a1", "exec_command", "{}")),
            // 该结果按文件顺序属于被拒批的区段；其 id 恰与前批悬空调用相同——
            // 若被拒批不充当分组边界，它会被贴给前批（伪造结果）
            line(&result("a1", "属于被拒批的结果"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![
            text(HistoryRole::User, "u1"),
            batch("a1", "exec_command", "{}"),
            synth("a1"),
        ],
        "前批合成未知结果（不得被后批结果满足）；后批结果按孤儿丢弃"
    );
    assert_eq!(loaded.skipped_lines, 1);
    assert_eq!(loaded.repairs, 2, "合成一处＋孤儿丢弃一处");
}

#[test]
fn orphan_and_interleaved_results_dropped() {
    let dir = TempDir::new("s-orphan");
    let p = session_path(&dir);
    seed(
        &p,
        &format!(
            "{META}\n{}\n{}\n{}\n{}\n",
            line(&text(HistoryRole::User, "u1")),
            line(&result("no-batch", "孤儿")),
            line(&batch("c1", "exec_command", "{}")),
            line(&result("other-id", "不属于当前批的结果"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![text(HistoryRole::User, "u1"), batch("c1", "exec_command", "{}"), synth("c1")],
        "无批结果与跨批交错结果丢弃"
    );
    assert_eq!(loaded.repairs, 3, "孤儿×2＋合成×1");
}

/// 多调用批：部分有结果——已有结果保留、缺的合成；顺序与文件一致。
#[test]
fn multi_call_batch_partial_results() {
    let dir = TempDir::new("s-multi");
    let p = session_path(&dir);
    let two_calls = HistoryItem::AssistantToolCalls {
        tool_calls: vec![
            ToolCallRecord { id: "m1".into(), name: "exec_command".into(), arguments: "{}".into() },
            ToolCallRecord { id: "m2".into(), name: "apply_patch".into(), arguments: "{}".into() },
        ],
        content: None,
    };
    seed(
        &p,
        &format!(
            "{META}\n{}\n{}\n{}\n",
            line(&text(HistoryRole::User, "u")),
            line(&two_calls),
            line(&result("m2", "second-result"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![text(HistoryRole::User, "u"), two_calls.clone(), result("m2", "second-result"), synth("m1")],
        "结果按 id 匹配（不按位置），缺失合成"
    );
    assert_eq!(loaded.repairs, 1);
}

// ---------- 装载：头部非 user（§6-F） ----------

#[test]
fn leading_non_user_dropped_to_first_user() {
    let dir = TempDir::new("s-lead");
    let p = session_path(&dir);
    seed(
        &p,
        &format!(
            "{META}\n{}\n{}\n{}\n",
            line(&text(HistoryRole::Assistant, "a0")),
            line(&text(HistoryRole::User, "u1")),
            line(&text(HistoryRole::Assistant, "a1"))
        ),
    );
    let loaded = load_session_file(&p).unwrap();
    assert_eq!(
        loaded.history,
        vec![text(HistoryRole::User, "u1"), text(HistoryRole::Assistant, "a1")]
    );
    assert_eq!(loaded.repairs, 1, "头部丢弃计数");

    // 全部非 user：空会话
    seed(
        &p,
        &format!("{META}\n{}\n", line(&text(HistoryRole::Assistant, "only"))),
    );
    let loaded = load_session_file(&p).unwrap();
    assert!(loaded.history.is_empty());
    assert_eq!(loaded.repairs, 1);
}

// ---------- 装载：摘要与确定性 ----------

#[test]
fn summary_counts_and_deterministic_reload() {
    let dir = TempDir::new("s-summary");
    let p = session_path(&dir);
    let content = format!(
        "{META}\n{{bad\n{}\n{}\n{{also bad\n{}\n",
        line(&text(HistoryRole::User, "u1")),
        line(&batch("z1", "exec_command", "{}")),
        line(&text(HistoryRole::Assistant, "a1"))
    );
    seed(&p, &content);
    let first = load_session_file(&p).unwrap();
    assert_eq!(first.summary(), "恢复 4 条（跳过 2 行，修复 1 处）");
    assert_eq!(first.skipped_line_numbers, vec![2, 5]);
    // 重新装载确定性：坏行照旧跳过、修复照旧重现（含合成结果逐字节同前）
    let second = load_session_file(&p).unwrap();
    assert_eq!(first.history, second.history);
    assert_eq!(first.skipped_lines, second.skipped_lines);
    assert_eq!(first.repairs, second.repairs);
}


// ---------- 序列化表征（§3 定稿；测试锁定往返一致） ----------

/// 首批表征必须与规格 §3 逐字一致：tag 前置、id/name/arguments 原样字节保留、
/// content 保留字符串或 null。往返后 PartialEq 相等且序列化逐字节同值。
#[test]
fn serialization_representation_roundtrip() {
    let items = vec![
        HistoryItem::Text(HistoryMessage {
        images: None,
        text_elements: Vec::new(), role: HistoryRole::User, content: "你好".into() , user_input_event: None}),
        HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::Assistant,
            content: "回复\"含引号\"与\\反斜杠".into(),
            user_input_event: None,
        }),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: " id-a ".into(),
                name: "exec_command".into(),
                arguments: "{\"cmd\":\"true\"}".into(),
            }],
            content: None,
        },
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: "b".into(),
                name: "apply_patch".into(),
                arguments: "not json".into(),
            }],
            content: Some("先改文件".into(),
        )},
        HistoryItem::ToolResult {
            image: None,
            tool_call_id: " id-a ".into(),
            content: "{\"status\":\"completed\",\"output\":\"ok\"}".into(),
            display: None,
        },
    ];
    let expected = [
        r#"{"type":"text","role":"user","content":"你好"}"#,
        r#"{"type":"text","role":"assistant","content":"回复\"含引号\"与\\反斜杠"}"#,
        r#"{"type":"tool_calls","tool_calls":[{"id":" id-a ","name":"exec_command","arguments":"{\"cmd\":\"true\"}"}],"content":null}"#,
        r#"{"type":"tool_calls","tool_calls":[{"id":"b","name":"apply_patch","arguments":"not json"}],"content":"先改文件"}"#,
        r#"{"type":"tool_result","tool_call_id":" id-a ","content":"{\"status\":\"completed\",\"output\":\"ok\"}"}"#,
    ];
    for (item, exp) in items.iter().zip(expected) {
        let line = serde_json::to_string(item).unwrap();
        assert_eq!(line, exp, "序列化表征与规格 §3 不一致");
        let back: HistoryItem = serde_json::from_str(&line).unwrap();
        assert_eq!(&back, item, "往返后记录逐字一致（id/name/arguments 原样保留）");
    }
}

// ---------- 写入器（Slice C） ----------

use std::os::fd::AsRawFd;
use std::path::Path;

use terminal_chat::session::{
    latest_auto, list_sessions, save_snapshot, validate_snapshot_name, Session,
};

fn user_dir(tag: &str) -> (TempDir, std::path::PathBuf) {
    let t = TempDir::new(tag);
    let dir = t.path().join("sessions");
    (t, dir)
}

fn auto_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().unwrap();
            n.starts_with("auto-").then(|| n)
        })
        .collect();
    names.sort();
    names
}

/// 自动追加：每条记录入史即提交；文件增长恰＝该行字节数、前缀字节不变（无全量重写）。
#[test]
fn incremental_append_grows_exactly_line_bytes_prefix_stable() {
    let (_t, dir) = user_dir("w-incr");
    let mut s = Session::new(dir.clone());
    let h1 = vec![text(HistoryRole::User, "第一条")];
    s.on_history(&h1);
    let mut h = h1.clone();
    let mut expected = format!("{META}\n{}\n", line(&h1[0])).into_bytes();
    // 三轮追加（含工具调用与结果形态）
    for item in [
        text(HistoryRole::Assistant, "回复一"),
        batch("c1", "exec_command", "{}"),
        result("c1", "{\"status\":\"completed\"}"),
    ] {
        h.push(item.clone());
        assert_eq!(s.on_history(&h), None, "正常追加不报错");
        expected.extend_from_slice(format!("{}\n", line(&item)).as_bytes());
        let actual = std::fs::read(&dir.join(&auto_files(&dir)[0])).unwrap();
        assert_eq!(actual.len(), expected.len(), "文件增长恰＝该行字节数");
        assert!(actual.starts_with(&expected[..]), "前缀字节不变（无全量重写）");
    }
    // 退出排空：无待写段，成功且静默
    assert_eq!(s.flush_final(&h), Ok(()));
    assert_eq!(std::fs::read(&dir.join(&auto_files(&dir)[0])).unwrap(), expected);
    // 权限：目录 0700、文件 0600
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(
        std::fs::metadata(dir.join(&auto_files(&dir)[0])).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// 快照只读装载后继续聊天：新 auto 文件以完整上下文开篇（坑011），快照字节不变。
#[test]
fn snapshot_resume_then_new_auto_starts_with_full_context() {
    let (_t, dir) = user_dir("w-ctx");
    std::fs::create_dir_all(&dir).unwrap();
    let snap = dir.join("snap1.jsonl");
    let snap_content = format!(
        "{META}\n{}\n{}\n",
        line(&text(HistoryRole::User, "旧问题")),
        line(&text(HistoryRole::Assistant, "旧回答"))
    );
    std::fs::write(&snap, &snap_content).unwrap();

    let mut s = Session::new(dir.clone());
    let info = s.resume(Some("snap1")).unwrap();
    assert!(!info.degraded, "快照装载不涉锁");
    assert_eq!(info.loaded.history.len(), 2);

    let mut h = info.loaded.history.clone();
    h.push(text(HistoryRole::User, "新问题"));
    h.push(text(HistoryRole::Assistant, "新回答"));
    assert_eq!(s.on_history(&h), None);
    assert_eq!(s.flush_final(&h), Ok(()));

    // 新 auto 文件＝meta＋全部 4 条（快照前缀＋新轮）；快照原样
    let auto = dir.join(&auto_files(&dir)[0]);
    let expect = format!(
        "{META}\n{}\n{}\n{}\n{}\n",
        line(&text(HistoryRole::User, "旧问题")),
        line(&text(HistoryRole::Assistant, "旧回答")),
        line(&text(HistoryRole::User, "新问题")),
        line(&text(HistoryRole::Assistant, "新回答"))
    );
    assert_eq!(std::fs::read_to_string(&auto).unwrap(), expect);
    assert_eq!(std::fs::read_to_string(&snap).unwrap(), snap_content);
    // 重装载新 auto 文件＝完整历史（不丢前缀）
    let re = load_session_file(&auto).unwrap();
    assert_eq!(re.history, h);
}

/// 续写尾置处理（§3）：完整无换行尾行→补恰一个 \n；截断尾行→截到最后一个 \n 后。
#[test]
fn resume_tail_handling_two_cases() {
    let (_t, dir) = user_dir("w-tail");
    std::fs::create_dir_all(&dir).unwrap();
    let good_prefix = format!("{META}\n{}\n", line(&text(HistoryRole::User, "u1")));

    // 用例一：尾行完整 JSON 无 \n → 装载接受；续写前补恰一个 \n
    let p = dir.join("auto-t1-1.jsonl");
    std::fs::write(&p, format!("{good_prefix}{}", line(&text(HistoryRole::Assistant, "a1")))).unwrap();
    let mut s = Session::new(dir.clone());
    let info = s.resume(Some("auto-t1-1")).unwrap();
    assert_eq!(info.loaded.tail, TailState::AcceptedNoNewline);
    let after_meta = format!("{good_prefix}{}\n", line(&text(HistoryRole::Assistant, "a1")));
    assert_eq!(std::fs::read(&p).unwrap(), after_meta.as_bytes(), "补恰一个换行、前缀（含该尾行）字节不变");
    // 此后追加独立成行
    let mut h = info.loaded.history;
    h.push(text(HistoryRole::User, "u2"));
    s.on_history(&h);
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        format!("{after_meta}{}\n", line(&text(HistoryRole::User, "u2")))
    );

    // 用例二：尾部截断 JSON（崩溃半写遗留）→ 装载跳过；续写前截到最后 \n 后
    let p2 = dir.join("auto-t2-1.jsonl");
    std::fs::write(&p2, format!("{good_prefix}{{\"type\":\"text\",\"role\":\"ass")).unwrap();
    let mut s2 = Session::new(dir.clone());
    let info2 = s2.resume(Some("auto-t2-1")).unwrap();
    assert_eq!(info2.loaded.history, vec![text(HistoryRole::User, "u1")]);
    assert_eq!(std::fs::read(&p2).unwrap(), good_prefix.clone().into_bytes(), "截断字节被丢弃、已提交记录不变");
    let mut h2 = info2.loaded.history;
    h2.push(text(HistoryRole::Assistant, "a2"));
    s2.on_history(&h2);
    assert_eq!(
        std::fs::read_to_string(&p2).unwrap(),
        format!("{good_prefix}{}\n", line(&text(HistoryRole::Assistant, "a2"))),
        "新记录独立成行、不与残留粘连"
    );
}

/// 独占锁（§6-I）：获锁失败→降级只读、原文件零修改；释放后新会话可续写。
/// 相位 3 经独立子进程验证（spawn 在释放之后、与本测试进程 fd 表隔离）——
/// 本进程内"释放后立即再锁"的判定受并发 spawn 线程的 fork→exec 窗口干扰
/// （子进程短暂持有本进程 fd 副本，取证见 logs/2026-10-10-007-implementation.md
/// 第四批），子进程结构天然排除该干扰且更贴近生产行为（新进程恢复）。
#[test]
fn flock_conflict_degrades_to_readonly_then_resumable() {
    session_probe_child(); // 探针子进程模式入口（自执行时拦截）
    let (_t, dir) = user_dir("w-lock");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("auto-x-1.jsonl");
    let content = format!("{META}\n{}\n", line(&text(HistoryRole::User, "被占用")));
    std::fs::write(&p, &content).unwrap();

    // 测试进程扮演持锁写者（flock 跨打开文件描述互斥，同进程亦冲突）
    let holder = std::fs::File::options().append(true).read(true).open(&p).unwrap();
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);

    let mut s = Session::new(dir.clone());
    let info = s.resume(Some("auto-x-1")).unwrap();
    assert!(info.degraded, "获锁失败应降级");
    assert_eq!(info.loaded.history, vec![text(HistoryRole::User, "被占用")]);
    assert_eq!(std::fs::read(&p).unwrap(), content.as_bytes(), "降级只读：原文件零修改");

    // 降级后新记录写自己的新 auto 文件（完整上下文开篇）
    let mut h = info.loaded.history;
    h.push(text(HistoryRole::User, "降级后的新消息"));
    assert_eq!(s.on_history(&h), None);
    assert_eq!(std::fs::read(&p).unwrap(), content.as_bytes(), "原 auto 文件仍零修改");
    let autos = auto_files(&dir);
    assert_eq!(autos.len(), 2, "新记录写入本运行自己的新 auto 文件");
    let new_auto = dir.join(autos.iter().find(|n| *n != "auto-x-1.jsonl").unwrap());
    let re = load_session_file(&new_auto).unwrap();
    assert_eq!(re.history, h, "新 auto 文件以完整上下文开篇");

    // 写者释放：新会话（独立子进程）可正常获锁续写，追加落盘
    drop(holder);
    let out = run_probe(
        "flock_conflict_degrades_to_readonly_then_resumable",
        serde_json::json!({
            "case": "resume_lock",
            "dir": dir.display().to_string(),
            "name": "auto-x-1",
        }),
    );
    assert_eq!(out["degraded"], serde_json::json!(false), "写者释放后新会话必须能续写：{out}");
    assert_eq!(out["append_ok"], serde_json::json!(true), "获锁后追加必须落盘：{out}");
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        format!("{content}{}\n", line(&text(HistoryRole::Assistant, "续写")))
    );
}

/// 行上限（§6-E）：超 64 MiB 记录→停用＋报错一次、绝不截断、文件停在最后提交点；
/// 后续记录仅内存；/save 对该内容拒绝。
#[test]
fn over_limit_record_disables_and_save_rejects() {
    let (_t, dir) = user_dir("w-limit");
    let mut s = Session::new(dir.clone());
    let big = "x".repeat(64 * 1024 * 1024 + 16);
    let mut h = vec![text(HistoryRole::User, "正常记录")];
    assert_eq!(s.on_history(&h), None);
    let committed = std::fs::read(&dir.join(&auto_files(&dir)[0])).unwrap();
    h.push(text(HistoryRole::User, &big));
    let first = s.on_history(&h);
    assert!(first.is_some(), "超限记录必须明确报错");
    assert!(first.unwrap().contains("64"), "报错提及上限");
    let after = std::fs::read(&dir.join(&auto_files(&dir)[0])).unwrap();
    assert_eq!(after, committed, "文件停在最后提交点，绝不截断写入");
    // 再来一条：静默（已停用）
    h.push(text(HistoryRole::Assistant, "停用后"));
    assert_eq!(s.on_history(&h), None, "停用后不再重复报错");
    assert_eq!(std::fs::read(&dir.join(&auto_files(&dir)[0])).unwrap(), committed);
    // /save 拒绝
    let err = save_snapshot(&dir, "big", &h).unwrap_err();
    assert!(err.contains("64"), "保存拒绝提及上限：{err}");
    assert!(!dir.join("big.jsonl").exists() && dir.join("big.jsonl.tmp.9.9.9").exists() == false);
    // 无 tmp 残留
    let residue: Vec<_> = std::fs::read_dir(&dir).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(residue.is_empty(), "失败保存不留 tmp：{residue:?}");
}

/// 读侧 64 MiB 边界（§6-E）：恰好上限的记录可装载；超 1 字节跳过并准确计数。
#[test]
fn line_limit_accepts_exact_and_skips_one_byte_over() {
    let dir = TempDir::new("s-line-limit");
    let path = session_path(&dir);
    let overhead = line(&text(HistoryRole::User, "")).len();

    let exact_line = line(&text(HistoryRole::User, &"x".repeat(LINE_LIMIT - overhead)));
    assert_eq!(exact_line.len(), LINE_LIMIT, "构造恰好 64 MiB 的记录行");
    seed(&path, &format!("{META}\n{exact_line}\n"));
    let loaded = load_session_file(&path).unwrap();
    assert_eq!(loaded.history.len(), 1, "恰好 64 MiB 必须可装载");
    assert_eq!(loaded.skipped_lines, 0, "恰好上限不得计为坏行");
    assert_eq!(loaded.tail, TailState::Terminated);

    let over_line = line(&text(HistoryRole::User, &"x".repeat(LINE_LIMIT - overhead + 1)));
    assert_eq!(over_line.len(), LINE_LIMIT + 1, "构造超上限 1 字节的记录行");
    seed(&path, &format!("{META}\n{over_line}\n"));
    let loaded = load_session_file(&path).unwrap();
    assert!(loaded.history.is_empty(), "超限记录必须跳过");
    assert_eq!(loaded.skipped_lines, 1, "跳过的行准确计数");
    assert_eq!(loaded.skipped_line_numbers, vec![2], "行号指向超限记录");
    assert_eq!(loaded.tail, TailState::Terminated);
}

/// 名称校验：非法名称明确报错且零文件操作（目录都不创建）。
#[test]
fn name_validation_rejects_before_any_fs_effect() {
    let (_t, dir) = user_dir("w-name");
    assert!(validate_snapshot_name("").is_err());
    for bad in ["../x", "/abs", "a/b", "a\\b", "空格 名", "名字!", "auto-reserved", &"x".repeat(65)] {
        assert!(validate_snapshot_name(bad).is_err(), "{bad:?} 应拒绝");
    }
    assert!(validate_snapshot_name("ok-name_1.2").is_ok());
    // /save 对非法名称零文件操作
    for bad in ["../escape", "auto-1", ""] {
        let before: Vec<String> = std::fs::read_dir(dir.parent().unwrap()).unwrap()
            .filter_map(|e| e.unwrap().file_name().into_string().ok()).collect();
        assert!(save_snapshot(&dir, bad, &[text(HistoryRole::User, "x")]).is_err());
        let after: Vec<String> = std::fs::read_dir(dir.parent().unwrap()).unwrap()
            .filter_map(|e| e.unwrap().file_name().into_string().ok()).collect();
        assert_eq!(before, after, "非法名称 {bad:?}：零文件操作");
    }
}

/// 手动快照：内容＝meta＋逐记录行、0600、计数返回；目录 0700。
#[test]
fn save_snapshot_writes_meta_and_records_atomically() {
    let (_t, dir) = user_dir("w-save");
    let h = vec![
        text(HistoryRole::User, "问题"),
        text(HistoryRole::Assistant, "回答"),
    ];
    let n = save_snapshot(&dir, "snap", &h).unwrap();
    assert_eq!(n, 2);
    assert_eq!(
        std::fs::read_to_string(dir.join("snap.jsonl")).unwrap(),
        format!("{META}\n{}\n{}\n", line(&h[0]), line(&h[1]))
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(dir.join("snap.jsonl")).unwrap().permissions().mode() & 0o777, 0o600);
    // 空历史快照合法：仅 meta
    let n0 = save_snapshot(&dir, "empty", &[]).unwrap();
    assert_eq!(n0, 0);
    assert_eq!(std::fs::read_to_string(dir.join("empty.jsonl")).unwrap(), format!("{META}\n"));
}

/// 列表与最近 auto：readdir＋stat（不打开正文）；字典序最大。
#[test]
fn list_and_latest_auto() {
    let (_t, dir) = user_dir("w-list");
    assert!(list_sessions(&dir).is_empty());
    assert_eq!(latest_auto(&dir), None);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in [
        ("auto-20260101T000000-1.jsonl", 10u64),
        ("auto-20260601T000000-2.jsonl", 20),
        ("manual.jsonl", 30),
    ] {
        std::fs::write(dir.join(name), vec![0u8; bytes as usize]).unwrap();
    }
    let listed = list_sessions(&dir);
    assert_eq!(
        listed,
        vec![
            ("auto-20260101T000000-1".to_string(), 10),
            ("auto-20260601T000000-2".to_string(), 20),
            ("manual".to_string(), 30),
        ]
    );
    assert_eq!(latest_auto(&dir).as_deref(), Some("auto-20260601T000000-2"));
}

/// 无参 resume：无 auto 会话时明确报错；当前内存不因失败的 resume 改变。
#[test]
fn resume_without_auto_reports_error() {
    let (_t, dir) = user_dir("w-nolatest");
    let mut s = Session::new(dir.clone());
    let err = s.resume(None).unwrap_err();
    assert!(err.contains("自动会话"), "{err}");
}

// ---------- 子进程夹具：RLIMIT_FSIZE 半写注入（同 004 EFBIG 模式） ----------

/// 子进程模式入口：由 `TCHAT_SESSION_PROBE_INPUT` 触发。收紧本进程 RLIMIT_FSIZE
/// 并忽略 SIGXFSZ 后，经真实 `Session` 入口执行指定场景，把观测 JSON 写到
/// `TCHAT_SESSION_PROBE_OUTCOME` 后退出。限额与信号只存在于本子进程。
fn session_probe_child() {
    let Some(input_path) = std::env::var_os("TCHAT_SESSION_PROBE_INPUT") else {
        return;
    };
    let outcome_path = std::env::var_os("TCHAT_SESSION_PROBE_OUTCOME").expect("缺少 PROBE_OUTCOME");
    let spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&input_path).unwrap()).unwrap();
    let dir = std::path::PathBuf::from(spec["dir"].as_str().unwrap());
    let events = match spec["case"].as_str().unwrap() {
        "half_write" => {
            let soft = spec["limit_soft"].as_u64().unwrap() as libc::rlim_t;
            let raise = spec["raise_to"].as_u64().unwrap() as libc::rlim_t;
            unsafe {
                let mut old = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
                assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &mut old), 0);
                let lim = libc::rlimit { rlim_cur: soft, rlim_max: old.rlim_max };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &lim), 0);
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            }
            let mut s = Session::new(dir.clone());
            let mut h = vec![
                text(HistoryRole::User, "第一条正常记录"),
                text(HistoryRole::Assistant, "第二条正常记录"),
            ];
            let notice0 = s.on_history(&h);
            let b1 = std::fs::metadata(dir.join(&auto_files(&dir)[0])).unwrap().len();
            // 触发半写：该行超过剩余限额
            h.push(text(HistoryRole::User, &"半写触发".repeat(60)));
            let notice1 = s.on_history(&h);
            let after_fail = std::fs::metadata(dir.join(&auto_files(&dir)[0])).unwrap().len();
            // 退出排空仍受限：明确失败
            let flush_err = s.flush_final(&h).is_err();
            // 恢复写能力（抬高软限到未降过的硬限之下）→ 补写待写段＋新记录
            unsafe {
                let lim = libc::rlimit { rlim_cur: raise, rlim_max: libc::RLIM_INFINITY };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &lim), 0);
            }
            h.push(text(HistoryRole::Assistant, "恢复后的新记录"));
            let notice2 = s.on_history(&h);
            let final_len = std::fs::metadata(dir.join(&auto_files(&dir)[0])).unwrap().len();
            serde_json::json!({
                "notice0": notice0.is_some(),
                "b1": b1,
                "notice1": notice1.is_some(),
                "after_fail": after_fail,
                "flush_err": flush_err,
                "notice2": notice2.is_some(),
                "final_len": final_len,
            })
        }
        "save_fail" => {
            let soft = spec["limit_soft"].as_u64().unwrap() as libc::rlim_t;
            unsafe {
                let mut old = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
                assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &mut old), 0);
                let lim = libc::rlimit { rlim_cur: soft, rlim_max: old.rlim_max };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &lim), 0);
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            }
            let h = vec![text(HistoryRole::User, &"很长的内容".repeat(200))];
            let err = save_snapshot(&dir, "snap", &h).is_err();
            serde_json::json!({ "save_err": err })
        }
        "resume_lock" => {
            // 独立子进程恢复指定会话并追加一条：报告是否降级、追加是否落盘
            // （供"写者释放后新会话可续写"的子进程验证——验证者 fd 表与本
            // 测试进程隔离，不受其并发 spawn 的 fork→exec 窗口干扰）
            let name = spec["name"].as_str().unwrap();
            let mut s = Session::new(dir.clone());
            let info = s.resume(Some(name)).unwrap();
            let degraded = info.degraded;
            let mut h = info.loaded.history;
            let append_ok = !degraded && {
                h.push(text(HistoryRole::Assistant, "续写"));
                s.on_history(&h).is_none() && s.flush_final(&h).is_ok()
            };
            serde_json::json!({ "degraded": degraded, "append_ok": append_ok })
        }
        other => panic!("未知探针场景 {other}"),
    };
    std::fs::write(&outcome_path, events.to_string()).unwrap();
    std::process::exit(0);
}

/// 探针 fork 与锁断言互斥闸：`spawn()` 的 fork 瞬时复制全进程 fd 表（含其他测试
/// 持有的会话 flock）；O_CLOEXEC 要到 exec 才关闭，fork→exec 窗口内锁被子进程
/// 的引用拖住，并发的锁转移断言会误判 Busy（2026-10-15 审计定位：
/// new_auto_writer × half_write 探针配对约 75% 复现；单线程与隔离运行不现）。
/// 已验证范围＝测试间互扰（已复现并修复）；生产侧对等发生未证实（无直接证据；
/// CLI 不并发断言锁转移、窗口为 fork+exec 固有语义仅是机理分析）。
static PROBE_FORK_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_probe(entry: &str, spec: serde_json::Value) -> serde_json::Value {
    assert!(
        std::env::var_os("TCHAT_SESSION_PROBE_INPUT").is_none(),
        "探针子进程模式到达了未设防的用例"
    );
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let io = TempDir::new(&format!("s-probe-{seq}"));
    let input = io.path().join("in.json");
    let outcome = io.path().join("out.json");
    std::fs::write(&input, spec.to_string()).unwrap();
    // fork→exec 窗口全程持闸（子进程退出即窗口彻底关闭）
    let _fork_guard = PROBE_FORK_GUARD.lock().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg(entry)
        .arg("--exact")
        .env("TCHAT_SESSION_PROBE_INPUT", &input)
        .env("TCHAT_SESSION_PROBE_OUTCOME", &outcome)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("探针子进程启动失败");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                assert!(st.success(), "探针子进程异常退出：{st}");
                return serde_json::from_str(&std::fs::read_to_string(&outcome).unwrap())
                    .unwrap();
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("探针子进程看门狗超时");
            }
        }
    }
}

/// 提交点与半写截回（§5）：注入部分写失败→文件截回 durable_len：已提交记录逐字节
/// 不变、无半行残留；恢复后补写该记录恰一份完整行（不重复、不粘连）；排空失败
/// 返回明确错误。
#[test]
fn half_write_truncated_back_then_catchup_exactly_once() {
    session_probe_child();
    let (_t, dir) = user_dir("w-half");
    let h = vec![
        text(HistoryRole::User, "第一条正常记录"),
        text(HistoryRole::Assistant, "第二条正常记录"),
        text(HistoryRole::User, &"半写触发".repeat(60)),
        text(HistoryRole::Assistant, "恢复后的新记录"),
    ];
    let committed_len = format!("{META}\n{}\n{}\n", line(&h[0]), line(&h[1])).len();
    let line3 = format!("{}\n", line(&h[2])).len();
    let line4 = format!("{}\n", line(&h[3])).len();
    let out = run_probe(
        "half_write_truncated_back_then_catchup_exactly_once",
        serde_json::json!({
            "case": "half_write",
            "dir": dir.display().to_string(),
            // 已提交两条后仅剩 line3-16 字节余量：第三条写入中途 EFBIG
            "limit_soft": (committed_len + line3 - 16) as u64,
            "raise_to": 4 * 1024 * 1024u64,
        }),
    );
    assert_eq!(out["notice0"], serde_json::json!(false), "正常写入无提示");
    assert_eq!(out["b1"], serde_json::json!(committed_len as u64));
    assert!(out["notice1"].as_bool().unwrap(), "半写失败必须报错");
    assert_eq!(
        out["after_fail"], out["b1"],
        "截回 durable_len：已提交记录逐字节不变、无半行残留"
    );
    assert_eq!(out["flush_err"], serde_json::json!(true), "受限期内排空失败返回明确错误");
    assert_eq!(out["notice2"], serde_json::json!(false), "恢复后补写不再报错");
    assert_eq!(
        out["final_len"],
        serde_json::json!((committed_len + line3 + line4) as u64),
        "补写恰一份完整行＋新记录一行"
    );
    // 装载复核：四条记录恰各一份、顺序与内存一致、无粘连坏行
    let auto = dir.join(&auto_files(&dir)[0]);
    let loaded = load_session_file(&auto).unwrap();
    assert_eq!(loaded.history, h);
    assert_eq!(loaded.skipped_lines, 0);
    assert_eq!(loaded.repairs, 0);
}

/// 保存失败保护旧快照（§5）：EFBIG 使手动快照写中途失败→旧快照字节不变、
/// 无 tmp 残留、错误明确。
#[test]
fn save_failure_keeps_old_snapshot_intact() {
    session_probe_child();
    let (_t, dir) = user_dir("w-savefail");
    std::fs::create_dir_all(&dir).unwrap();
    let old = format!("{META}\n{}\n", line(&text(HistoryRole::User, "旧快照内容")));
    std::fs::write(dir.join("snap.jsonl"), &old).unwrap();
    let out = run_probe(
        "save_failure_keeps_old_snapshot_intact",
        serde_json::json!({
            "case": "save_fail",
            "dir": dir.display().to_string(),
            "limit_soft": 64u64, // 远小于新内容：写中途失败
        }),
    );
    assert_eq!(out["save_err"], serde_json::json!(true), "保存失败必须明确报错");
    assert_eq!(std::fs::read_to_string(dir.join("snap.jsonl")).unwrap(), old, "旧快照字节不变");
    let residue: Vec<String> = std::fs::read_dir(&dir).unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(residue.is_empty(), "失败保存不留 tmp：{residue:?}");
}

/// 创建失败→停用：sessions 路径被普通文件占位，首次追加落盘即失败，
/// 报错一次后仅内存（后续记录静默、无文件副作用）。
#[test]
fn creation_failure_disables_with_single_report() {
    let t = TempDir::new("w-createfail");
    let dir = t.path().join("sessions"); // 占位为普通文件
    std::fs::write(&dir, "not a dir").unwrap();
    let mut s = Session::new(dir.clone());
    let mut h = vec![text(HistoryRole::User, "u1")];
    let first = s.on_history(&h);
    assert!(first.is_some() && first.unwrap().contains("失败"), "创建失败明确报错");
    h.push(text(HistoryRole::Assistant, "a1"));
    assert_eq!(s.on_history(&h), None, "停用后静默");
    assert_eq!(s.flush_final(&h), Ok(()), "停用后排空无事可做");
    assert_eq!(std::fs::read_to_string(&dir).unwrap(), "not a dir", "占位文件原样");
}

/// 修复一致性·续写闭环（§5）：含坏行＋悬空调用的 auto 文件→续写追加两轮→
/// 重装载＝首次装载投影＋追加记录（坏行照旧跳过、合成结果逐字节重现）。
#[test]
fn repair_consistency_across_continuation() {
    let (_t, dir) = user_dir("w-consist");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("auto-20260101T000000-1.jsonl");
    let bad = "{{broken";
    let content = format!(
        "{META}\n{bad}\n{}\n{}\n",
        line(&text(HistoryRole::User, "旧问题")),
        line(&batch("k1", "exec_command", "{}"))
    );
    std::fs::write(&p, &content).unwrap();

    // 首次装载（auto 续写路径：锁＋尾置＋游标＝装载后历史长度）
    let mut s = Session::new(dir.clone());
    let info = s.resume(Some("auto-20260101T000000-1")).unwrap();
    let first = info.loaded.history.clone();
    assert_eq!(first.len(), 3);
    assert_eq!(info.loaded.skipped_lines, 1);

    // 续写追加两轮
    let mut h = first.clone();
    h.push(text(HistoryRole::Assistant, "新答一"));
    h.push(text(HistoryRole::User, "新问二"));
    assert_eq!(s.on_history(&h), None);
    assert_eq!(s.flush_final(&h), Ok(()));

    // 重装载：坏行照旧跳过、修复照旧重现（合成结果逐字节同前）、新记录照常装载
    let re = load_session_file(&p).unwrap();
    assert_eq!(re.skipped_lines, 1, "旧损坏照旧跳过");
    assert_eq!(re.repairs, 1, "修复照旧重现");
    assert_eq!(re.history, h, "重装载＝首次装载投影＋追加记录");
    assert_eq!(re.history[2], first[2], "合成结果逐字节同前");
    // 磁盘不被修复改写：坏行原样在文件里
    assert!(std::fs::read_to_string(&p).unwrap().contains(bad));
}

// ---------- 并发快照（§1：不加锁、tmp＋rename 原子替换、末者胜） ----------

fn expected_snapshot_bytes(history: &[HistoryItem]) -> Vec<u8> {
    let mut out = format!("{META}\n").into_bytes();
    for item in history {
        out.extend_from_slice(format!("{}\n", line(item)).as_bytes());
    }
    out
}

fn big_history(tag: &str, n: usize) -> Vec<HistoryItem> {
    (0..n)
        .map(|i| {
            if i % 2 == 0 {
                text(HistoryRole::User, &format!("{tag}-user-{i:05}-{}", "填充".repeat(200)))
            } else {
                text(HistoryRole::Assistant, &format!("{tag}-asst-{i:05}"))
            }
        })
        .collect()
}

fn tmp_residue(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .filter(|n| n.contains(".tmp."))
        .collect()
}

/// 受控顺序＋真实并发双验证：最终文件必为某一份完整快照（字节精确）、
/// 无混写、无 tmp 残留；受控顺序下最后成功替换者生效。
#[test]
fn concurrent_save_last_complete_snapshot_wins() {
    let (_t, dir) = user_dir("w-race");
    std::fs::create_dir_all(&dir).unwrap();
    let hist_a = big_history("A", 2000);
    let hist_b = big_history("B", 2000);
    let hist_a2 = big_history("A2", 1500);

    // 受控顺序：A → B → A2，最后成功替换者（A2）生效
    save_snapshot(&dir, "ordered", &hist_a).unwrap();
    save_snapshot(&dir, "ordered", &hist_b).unwrap();
    save_snapshot(&dir, "ordered", &hist_a2).unwrap();
    assert_eq!(
        std::fs::read(dir.join("ordered.jsonl")).unwrap(),
        expected_snapshot_bytes(&hist_a2),
        "最后成功替换者生效"
    );
    assert!(tmp_residue(&dir).is_empty());

    // 真实并发：双线程 barrier 同发同名快照 ×10 轮——rename 原子替换保证
    // 最终文件恰为某一份完整快照，绝无交错混写
    use std::sync::{Arc, Barrier};
    for round in 0..10 {
        let barrier = Arc::new(Barrier::new(2));
        let dir2 = dir.clone();
        let b1 = barrier.clone();
        let h1 = hist_a.clone();
        let t1 = std::thread::spawn(move || {
            b1.wait();
            save_snapshot(&dir2, "race", &h1)
        });
        let dir3 = dir.clone();
        let b2 = barrier.clone();
        let h2 = hist_b.clone();
        let t2 = std::thread::spawn(move || {
            b2.wait();
            save_snapshot(&dir3, "race", &h2)
        });
        let r1 = t1.join().unwrap();
        let r2 = t2.join().unwrap();
        assert!(r1.is_ok() && r2.is_ok(), "并发保存都应成功：{r1:?} {r2:?}");
        let final_bytes = std::fs::read(dir.join("race.jsonl")).unwrap();
        let expect_a = expected_snapshot_bytes(&hist_a);
        let expect_b = expected_snapshot_bytes(&hist_b);
        assert!(
            final_bytes == expect_a || final_bytes == expect_b,
            "第 {round} 轮：最终文件必须是某一份完整快照（长度 {}，A={} B={}）",
            final_bytes.len(),
            expect_a.len(),
            expect_b.len()
        );
        assert!(tmp_residue(&dir).is_empty(), "并发后无 tmp 残留");
    }
}

/// 同名恢复（合并门禁，007 规格同名契约）：当前写者（经 resume 持锁 auto-X）再次
/// 同名恢复并确认后，X 仍是续写目标——后续追加到 X、不因自持锁另建 auto 文件；
/// 历史替换、独占保持（非降级）、记录不重复、恢复零重放。实现复用已持锁描述符
/// 直接重装载（锁全程不离开写者，无释放再重抢的竞争窗口）。
/// 注：新建的 auto 文件同样打开即锁，任何 Writing 态都是持锁态。
#[test]
fn same_name_resume_keeps_original_as_append_target() {
    let (_t, dir) = user_dir("w-samename");
    std::fs::create_dir_all(&dir).unwrap();
    // 种子 auto 文件（磁盘直写）
    let stem = "auto-x-1";
    let path = dir.join("auto-x-1.jsonl");
    let seed = format!(
        "{META}\n{}\n{}\n",
        line(&text(HistoryRole::User, "旧一")),
        line(&text(HistoryRole::Assistant, "旧答一")),
    );
    std::fs::write(&path, &seed).unwrap();

    // 第一次恢复：获锁成为当前写者（Writing 持锁）
    let mut s = Session::new(dir.clone());
    let info = s.resume(Some(stem)).unwrap();
    assert!(!info.degraded);
    let mut h1 = info.loaded.history.clone();
    h1.push(text(HistoryRole::User, "持锁期间新记录"));
    assert_eq!(s.on_history(&h1), None);

    // 同名恢复（CLI 层此位置之前已经过丢弃确认与待写段排空）——复用已持锁
    // 描述符，直接断言非降级（无任何重试参与，保持变异检测力）
    let info2 = s.resume(Some(stem)).unwrap();
    assert!(!info2.degraded, "自持锁不得降级为另进程编辑");
    assert_eq!(info2.loaded.history, h1, "历史替换为装载投影（含持锁期间记录）");

    // X 仍是续写目标：新记录追加到 X，不另建 auto 文件
    let mut h2 = info2.loaded.history.clone();
    h2.push(text(HistoryRole::User, "同名恢复后新记录"));
    assert_eq!(s.on_history(&h2), None);
    assert_eq!(s.flush_final(&h2), Ok(()));
    assert_eq!(auto_files(&dir), vec!["auto-x-1.jsonl".to_string()], "不得因自持锁另建 auto 文件");

    // 记录不重复＋零重放：重装载＝3 条记录恰各一份、无坏行无修复
    let re = load_session_file(&path).unwrap();
    assert_eq!(re.history, h2);
    assert_eq!(re.skipped_lines, 0);
    assert_eq!(re.repairs, 0);
}

/// 新建 auto 的写者互斥（§6-I"成为续写目标时先取锁"涵盖新建路径）：A 新建
/// auto-X 并保持写入状态；B 恢复 X→不得获得续写权限（降级只读、零修改）、
/// B 的新记录写 B 自己的新 auto 文件；A 仍正常追加；A 释放后新会话才能续写 X。
#[test]
fn new_auto_writer_lock_excludes_second_writer() {
    // 本测试断言锁释放/获取的时序语义，与探针 fork 的 fd 继承窗口互斥
    let _fork_guard = PROBE_FORK_GUARD.lock().unwrap();
    let (_t, dir) = user_dir("w-newlock");
    // A：首条记录触发新建 auto-X（完整上下文开篇）
    let mut a = Session::new(dir.clone());
    let mut ha = vec![text(HistoryRole::User, "A-第一条")];
    assert_eq!(a.on_history(&ha), None);
    let autos = auto_files(&dir);
    assert_eq!(autos.len(), 1, "A 恰新建一个 auto 文件：{autos:?}");
    let x_file = autos[0].clone();
    let x_name = x_file.strip_suffix(".jsonl").unwrap().to_string();
    let x_path = dir.join(&x_file);
    let a1_bytes = std::fs::read(&x_path).unwrap();
    let expect_a1 = format!("{META}\n{}\n", line(&ha[0])).into_bytes();
    assert_eq!(a1_bytes, expect_a1);

    // B：恢复 X → A 仍处写入状态，B 必须降级只读、原文件零修改
    let mut b = Session::new(dir.clone());
    let info = b.resume(Some(&x_name)).unwrap();
    assert!(info.degraded, "A 持锁写者存在时 B 不得获得 X 的续写权限");
    assert_eq!(info.loaded.history, ha, "降级只读装载＝已提交记录");
    assert_eq!(std::fs::read(&x_path).unwrap(), a1_bytes, "B 降级：X 零修改");

    // B 的新记录写 B 自己的新 auto 文件（完整上下文开篇）；X 仍只有 A 的 1 条
    let mut hb = info.loaded.history.clone();
    hb.push(text(HistoryRole::User, "B-新消息"));
    assert_eq!(b.on_history(&hb), None);
    let autos = auto_files(&dir);
    assert_eq!(autos.len(), 2, "B 恰新建自己的一个 auto 文件：{autos:?}");
    let b_auto = dir.join(autos.iter().find(|n| **n != x_file).unwrap());
    assert_eq!(load_session_file(&b_auto).unwrap().history, hb, "B 新文件以完整上下文开篇");
    assert_eq!(load_session_file(&x_path).unwrap().history, ha, "B 未向 X 写入任何记录");

    // A 仍正常追加（B 的降级与并行读不影响写者）
    ha.push(text(HistoryRole::Assistant, "A-第二条"));
    assert_eq!(a.on_history(&ha), None);
    assert_eq!(load_session_file(&x_path).unwrap().history, ha, "A 追加如常");

    // A 释放（进程退出等价物）后：新会话才能获得 X 的续写权限
    drop(a);
    let mut c = Session::new(dir.clone());
    let info_c = c.resume(Some(&x_name)).unwrap();
    assert!(!info_c.degraded, "A 释放后新会话必须能续写 X");
    let mut hc = info_c.loaded.history;
    hc.push(text(HistoryRole::Assistant, "C-续写"));
    assert_eq!(c.on_history(&hc), None);
    assert_eq!(c.flush_final(&hc), Ok(()));
    assert_eq!(load_session_file(&x_path).unwrap().history, hc);
}

/// 同名恢复的受控竞争（合并门禁）：写者持锁 auto-X 期间，并发抢锁者高频竞争 X
/// 的锁，写者反复同名恢复（重装载）。若锁在恢复过程中离开过写者（释放再重抢
/// 的竞争窗口），高频抢锁者必然至少得手一次——断言得手次数恒为 0、每次恢复都
/// 保持独占（非降级）、装载投影与追加游标正确。这是"用受控竞争事件验证"而非
/// 仅观察恢复前后的两次锁状态。
#[test]
fn same_name_resume_survives_concurrent_lock_poacher() {
    let (_t, dir) = user_dir("w-poach");
    std::fs::create_dir_all(&dir).unwrap();
    let stem = "auto-x-1";
    let path = dir.join("auto-x-1.jsonl");
    let seed = format!("{META}\n{}\n", line(&text(HistoryRole::User, "种子")));
    std::fs::write(&path, &seed).unwrap();

    // 写者获锁
    let mut s = Session::new(dir.clone());
    let info = s.resume(Some(stem)).unwrap();
    assert!(!info.degraded);

    // 抢锁者（另一线程，flock 跨打开文件描述互斥、同进程亦冲突）：高频尝试
    // flock X，得手即计数并立即释放——它的得手次数就是"锁离开写者"的直接证据
    let poached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (p_path, p_poached, p_stop) = (path.clone(), poached.clone(), stop.clone());
    let poacher = std::thread::spawn(move || {
        while !p_stop.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok(f) = std::fs::OpenOptions::new().append(true).read(true).open(&p_path) {
                if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    p_poached.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN); }
                }
                drop(f);
            }
        }
    });

    // 压力下反复同名恢复＋追加
    let mut h = info.loaded.history.clone();
    for i in 0..200 {
        h.push(text(HistoryRole::User, &format!("轮{i:03}")));
        assert_eq!(s.on_history(&h), None, "追加不受抢锁者影响");
        let info_i = s.resume(Some(stem)).unwrap();
        assert!(!info_i.degraded, "第 {i} 轮同名恢复被挤成降级——锁在恢复中离开过写者");
        assert_eq!(info_i.loaded.history, h, "重装载＝内存历史（游标正确）");
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    poacher.join().unwrap();
    assert_eq!(
        poached.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "抢锁者得手过——写者的锁存在离开窗口（释放再重抢）"
    );
    assert_eq!(s.flush_final(&h), Ok(()));
    assert_eq!(load_session_file(&path).unwrap().history, h, "文件恰含全部记录一份");
    assert_eq!(auto_files(&dir), vec!["auto-x-1.jsonl".to_string()], "全程未另建 auto 文件");
}

/// §7 缺陷修复回归：撞名候选保持单一 `auto-` 前缀（`auto-<ts>-<pid>-<N>.jsonl`）；
/// 撞名候选与时间更晚的正常 auto 文件共存时，无参恢复（latest_auto 字典序最大
/// ＝时间序最新）必须选中更晚文件——行为断言（装载历史内容），非仅名称形态。
/// 旧实现候选名重复拼接前缀产生 `auto-auto-*`，第 6 字节 'a'>'2' 使其字典序
/// 压过一切正常 auto 文件；本测试在旧实现上以"无参恢复选中较旧撞名会话"失败。
#[test]
fn collision_candidate_single_prefix_and_latest_picks_newer_session() {
    let _env = common::ENV_LOCK.lock().unwrap();
    let (_t, dir) = user_dir("w-collide");
    std::fs::create_dir_all(&dir).unwrap();
    // 多重撞名占位：首选名与 -1 候选名均被占用（验证 50 次重试链保留）
    std::fs::write(
        dir.join("auto-20260101T000000-7.jsonl"),
        format!("{META}\n{}\n", line(&text(HistoryRole::User, "占位旧会话"))),
    )
    .unwrap();
    std::fs::write(
        dir.join("auto-20260101T000000-7-1.jsonl"),
        format!("{META}\n{}\n", line(&text(HistoryRole::User, "占位一"))),
    )
    .unwrap();
    // 注入提供者：新会话首选名指向被占用名字（ENV_LOCK 防并行 env 串扰）
    std::env::set_var("TCHAT_SESSION_TS", "20260101T000000");
    std::env::set_var("TCHAT_SESSION_PID", "7");
    let mut s = Session::new(dir.clone());
    let h = vec![text(HistoryRole::User, "撞名后的新记录")];
    assert_eq!(s.on_history(&h), None, "撞名重试后创建成功，无错误提示");
    std::env::remove_var("TCHAT_SESSION_TS");
    std::env::remove_var("TCHAT_SESSION_PID");

    // 行为核心断言（先行）：更晚正常会话在场，无参恢复必须装载它的内容——
    // 旧实现在此以"选中较旧撞名会话"失败，先于任何名称形态断言
    std::fs::write(
        dir.join("auto-20260102T000000-8.jsonl"),
        format!("{META}\n{}\n", line(&text(HistoryRole::User, "更晚会话记录"))),
    )
    .unwrap();
    assert_eq!(latest_auto(&dir).as_deref(), Some("auto-20260102T000000-8"));
    let mut newer = Session::new(dir.clone());
    let info = newer
        .resume(None)
        .unwrap_or_else(|e| panic!("无参恢复不应失败：{e}"));
    assert_eq!(
        info.loaded.history,
        vec![text(HistoryRole::User, "更晚会话记录")],
        "无参恢复必须选中时间更晚的正常会话（§1：字典序最大＝时间序最新）"
    );
    // 恢复后更晚文件即续写目标（锁语义不回归）
    let mut h2 = info.loaded.history;
    h2.push(text(HistoryRole::Assistant, "更晚续写"));
    assert_eq!(newer.on_history(&h2), None);
    assert_eq!(newer.flush_final(&h2), Ok(()));
    assert_eq!(load_session_file(&dir.join("auto-20260102T000000-8.jsonl")).unwrap().history, h2);

    let autos = auto_files(&dir);
    // 形态断言（补充）：候选名单一 auto- 前缀；多重撞名换 -2
    assert!(
        autos.iter().all(|n| !n.starts_with("auto-auto-")),
        "撞名候选不得重复 auto- 前缀：{autos:?}"
    );
    assert!(
        autos.contains(&"auto-20260101T000000-7-2.jsonl".to_string()),
        "多重撞名按 -N 重试：{autos:?}"
    );
    // 候选内容完整（完整上下文开篇不回归）
    let cand = load_session_file(&dir.join("auto-20260101T000000-7-2.jsonl")).unwrap();
    assert_eq!(cand.history, h);
}

/// 012：tool_result 可选 display 展示字段——新记录往返保真；旧格式（无字段）装载为
/// None，不报错不迁移；模型面 content 原样。
#[test]
fn tool_result_display_roundtrip_and_legacy_default_none() {
    use terminal_chat::prompt::{PatchChange, PatchChangeKind, ToolCallRecord, ToolDisplay};
    let dir = TempDir::new("s012-disp");
    // 真实历史形状：user 开篇（007 §6-F 头部规则）＋结果与 tool_calls 批次成对
    // （配对修复语义：孤立结果行按修复跳过），夹具须还原真实会话结构。
    let hist = vec![
        HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: "012 展示字段往返".into(),
            user_input_event: None,
        }),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: "call-x".into(),
                name: "apply_patch".into(),
                arguments: "{}".into(),
            }],
            content: None,
        },
        HistoryItem::ToolResult {
            image: None,
            tool_call_id: "call-x".into(),
            content: r#"{"status":"completed","exit_code":0,"output":"o"}"#.into(),
            display: Some(ToolDisplay::Patch {
                changes: vec![PatchChange {
                    path: "/tmp/a.txt".into(),
                    kind: PatchChangeKind::Update {
                        diff: "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y".into(),
                    },
                }],
            }),
        },
    ];
    terminal_chat::session::save_snapshot(&dir.path(), "disp", &hist).unwrap();
    // 序列化行含 display；模型面 content 字段原样保留
    let line = std::fs::read_to_string(dir.path().join("disp.jsonl")).unwrap();
    assert!(line.contains("\"display\":"), "新字段持久：{line}");
    assert!(line.contains(r#"exit_code\":0"#), "content 原样：{line}");
    // 往返
    let loaded =
        terminal_chat::session::load_session_file(&dir.path().join("disp.jsonl")).unwrap();
    assert_eq!(loaded.history.len(), 3, "配对装载不丢记录：{:?}", loaded.history);
    assert_eq!(loaded.repairs, 0, "真实形状零修复：{}", loaded.repairs);
    match &loaded.history[2] {
        HistoryItem::ToolResult { tool_call_id, content, display, .. } => {
            assert_eq!(tool_call_id, "call-x");
            assert!(content.starts_with("{\"status\""));
            assert!(matches!(display, Some(ToolDisplay::Patch { changes }) if changes.len() == 1),
                "展示数据往返保真：{display:?}");
        }
        other => panic!("形状：{other:?}"),
    }
    // 旧格式（无 display 字段）→ None（serde 缺省兼容，不报错）。user 开篇＋结果与
    // tool_calls 配对（007 §6-F 头部规则与配对修复语义）。
    let legacy = concat!(
        "{\"v\":1}\n",
        "{\"type\":\"text\",\"role\":\"user\",\"content\":\"旧会话提问\"}\n",
        "{\"type\":\"tool_calls\",\"tool_calls\":[{\"id\":\"c1\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"echo x\\\"}\"}],\"content\":null}\n",
        "{\"type\":\"tool_result\",\"tool_call_id\":\"c1\",\"content\":\"{\\\"ok\\\":true}\"}\n"
    );
    std::fs::write(dir.path().join("legacy.jsonl"), legacy).unwrap();
    let loaded =
        terminal_chat::session::load_session_file(&dir.path().join("legacy.jsonl")).unwrap();
    match &loaded.history[2] {
        HistoryItem::ToolResult { display, .. } => assert!(display.is_none(), "旧记录→None"),
        other => panic!("旧记录形状：{other:?}"),
    }
}

// ---------- 019：图片载荷字段的保存与恢复 ----------

use terminal_chat::prompt::ToolResultImage;

fn img_result(id: &str, content: &str, url: &str) -> HistoryItem {
    HistoryItem::ToolResult {
        tool_call_id: id.into(),
        content: content.into(),
        display: None,
        image: Some(ToolResultImage { image_url: url.into(), detail: "high".into() }),
    }
}

/// 图片载荷内联结果行往返：快照保存→装载字节等同（不重读原文件）。
#[test]
fn image_payload_roundtrip_inline_in_result_row() {
    let dir = TempDir::new("s-img-rt");
    let history = vec![
        text(HistoryRole::User, "看图"),
        batch("vi-1", "view_image", r#"{"path":"a.png"}"#),
        img_result(
            "vi-1",
            r#"{"ok":true,"path":"/w/a.png","width":2,"height":2}"#,
            "data:image/png;base64,QUJDRA==",
        ),
        text(HistoryRole::Assistant, "完成"),
    ];
    let path = dir.path().join("snap.jsonl");
    seed(&path, &format!("{META}\n"));
    let n = terminal_chat::session::save_snapshot(dir.path(), "snap", &history).unwrap();
    assert_eq!(n, 4);
    let loaded = load_session_file(&path).unwrap();
    assert_eq!(loaded.history.len(), 4, "不增不减");
    match &loaded.history[2] {
        HistoryItem::ToolResult { tool_call_id, content, image, .. } => {
            assert_eq!(tool_call_id, "vi-1");
            assert!(content.contains("\"ok\":true"));
            let img = image.as_ref().expect("载荷随结果行恢复");
            assert_eq!(img.image_url, "data:image/png;base64,QUJDRA==");
            assert_eq!(img.detail, "high");
        }
        other => panic!("形状：{other:?}"),
    }
    // 原始行内联（同一结果行携带 image 字段，无独立附件行）
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"image\":{\"image_url\":\"data:image/png;base64,QUJDRA==\""), "{raw}");
    assert_eq!(raw.lines().count(), 5, "meta＋4 行");
}

/// 非法图片载荷行按坏行处理：错误 MIME／错误 detail／非法 base64／远程 URL／
/// 超限额字段——跳过该行并补未知结果。
#[test]
fn invalid_image_payload_rows_are_bad_lines() {
    let dir = TempDir::new("s-img-bad");
    let path = session_path(&dir);
    let good_summary = r#"{"ok":true,"path":"/w/a.png"}"#;
    for (tag, image_field) in [
        ("错误 MIME", "\"image\":{\"image_url\":\"data:image/tiff;base64,QUJD\",\"detail\":\"high\"}"),
        ("错误 detail", "\"image\":{\"image_url\":\"data:image/png;base64,QUJD\",\"detail\":\"low\"}"),
        ("非法 base64", "\"image\":{\"image_url\":\"data:image/png;base64,!!!!\",\"detail\":\"high\"}"),
        ("远程 URL", "\"image\":{\"image_url\":\"https://evil/x.png\",\"detail\":\"high\"}"),
        ("超限额", &format!("\"image\":{{\"image_url\":\"data:image/png;base64,{}\",\"detail\":\"high\"}}", "Q".repeat(6 * 1024 * 1024))),
    ] {
        let body = format!(
            "{META}\n{}\n{}\n{}\n",
            line(&text(HistoryRole::User, "看")),
            line(&batch("vi-x", "view_image", r#"{"path":"a.png"}"#)),
            format!(
                r#"{{"type":"tool_result","tool_call_id":"vi-x","content":{}}},{}"#,
                serde_json::to_string(good_summary).unwrap(),
                image_field
            ),
        );
        seed(&path, &body);
        let loaded = load_session_file(&path).unwrap();
        assert_eq!(loaded.skipped_lines, 1, "{tag}：坏行 {loaded:?}");
        match &loaded.history.last().unwrap() {
            HistoryItem::ToolResult { content, .. } => {
                assert!(content.contains(UNKNOWN_MSG), "{tag} 补未知结果：{content}");
            }
            other => panic!("{tag} 形状：{other:?}"),
        }
    }
}

/// 旧程序降级夹具（019 §5.1）：
/// ①旧程序读含 image 的会话行——旧形状解析器（无 image 字段的变体）忽略该
///   字段并解析成功，重存即丢图（不可宣称无损兼容，已确认降级）；
/// ②旧程序读含 input_modalities 的模型配置——deny_unknown_fields 拒绝
///   （回退旧程序前需移除该字段）。
#[test]
fn downgrade_fixtures_old_reader_drops_image_and_rejects_new_config() {
    // ①旧形状会话行解析器：无 image 字段的 ToolResult（serde 默认忽略未知字段）
    #[derive(serde::Deserialize, serde::Serialize, Debug)]
    #[serde(tag = "type")]
    enum OldShape {
        #[serde(rename = "tool_result")]
        ToolResult { tool_call_id: String, content: String },
    }
    let new_line = serde_json::to_string(&img_result(
        "vi-1",
        r#"{"ok":true}"#,
        "data:image/png;base64,QUJDRA==",
    ))
    .unwrap();
    let old: OldShape = serde_json::from_str(&new_line).expect("旧程序可读（忽略新字段）");
    match &old {
        OldShape::ToolResult { tool_call_id, content } => {
            assert_eq!(tool_call_id, "vi-1");
            assert!(content.contains("\"ok\":true"), "文本摘要保留");
        }
    }
    // 旧程序重存＝按旧形状序列化，载荷丢失（降级事实由夹具钉死）
    let rewritten = serde_json::to_string(&old).unwrap();
    assert!(!rewritten.contains("image"), "重存丢图：{rewritten}");
    // ②旧形状模型配置解析器（deny_unknown_fields，同 catalog::ModelInfo 旧版）
    #[derive(serde::Deserialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct OldModel {
        id: String,
    }
    let err = serde_json::from_str::<OldModel>(
        r#"{"id":"m","input_modalities":["text","image"]}"#,
    );
    assert!(err.is_err(), "旧程序拒绝新配置字段：{err:?}");
}

// ---------- 020：用户消息图片字段的装载校验 ----------

use codex_protocol::user_input::{ByteRange, TextElement};

fn user_img_msg(content: &str, images: Vec<String>, elements: Vec<TextElement>) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: content.to_string(),
        user_input_event: None,
        images: (!images.is_empty()).then_some(images),
        text_elements: elements,
    })
}

fn b64_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// 020 §2.2：合法用户图片行完整往返；坏字段按 007 坏行处理（跳过＋首 user 修复）。
#[test]
fn user_image_rows_valid_roundtrip_and_bad_lines() {
    let dir = TempDir::new("s-uimg");
    let path = session_path(&dir);
    let ok = user_img_msg(
        "[Image #1] 看",
        vec![b64_url(b"\x89PNGok")],
        vec![TextElement::new(ByteRange { start: 0, end: 10 }, Some("[Image #1]".into()))],
    );
    let raw_line = line(&ok);
    seed(&path, &format!("{META}\n{raw_line}\n"));
    let loaded = load_session_file(&path).unwrap();
    assert!(
        loaded.skipped_lines == 0,
        "合法行被跳过：{raw_line} → {loaded:?}"
    );
    match &loaded.history[0] {
        HistoryItem::Text(m) => {
            assert_eq!(m.images.as_ref().map(|v| v.len()), Some(1));
            assert_eq!(m.text_elements.len(), 1);
        }
        other => panic!("形状：{other:?}"),
    }
    // 坏行族：assistant 带图／问询标记带图／远程 URL／坏 base64／超限额／
    // 标记越界（#2 只有 1 图）／标记文字形态错／区间重叠／非 UTF-8 边界
    let u1 = b64_url(b"AAA");
    let mk_el = |s: usize, e: usize| TextElement::new(ByteRange { start: s, end: e }, None);
    let cases: Vec<(&str, HistoryItem)> = vec![
        ("assistant 带图", HistoryItem::Text(HistoryMessage {
            role: HistoryRole::Assistant,
            content: "a".into(),
            user_input_event: None,
            images: Some(vec![u1.clone()]),
            text_elements: vec![],
        })),
        ("问询标记带图", HistoryItem::Text(HistoryMessage {
            role: HistoryRole::User,
            content: "答".into(),
            user_input_event: Some(terminal_chat::prompt::UserInputEvent::Answer {
                request_id: "r".into(),
            }),
            images: Some(vec![u1.clone()]),
            text_elements: vec![],
        })),
        ("远程 URL", user_img_msg("[Image #1]", vec!["https://e/x.png".into()], vec![mk_el(0, 10)])),
        ("坏 base64", user_img_msg("[Image #1]", vec!["data:image/png;base64,!!!!".into()], vec![mk_el(0, 10)])),
        ("超限额", user_img_msg(
            "[Image #1]",
            vec![format!("data:image/png;base64,{}", "Q".repeat(6 * 1024 * 1024))],
            vec![mk_el(0, 10)],
        )),
        ("标记越界", user_img_msg("[Image #2]", vec![u1.clone()], vec![mk_el(0, 10)])),
        ("标记文字错", user_img_msg("图片1", vec![u1.clone()], vec![mk_el(0, 10)])),
        ("区间重叠", user_img_msg(
            "[Image #1][Image #1]",
            vec![u1.clone()],
            vec![mk_el(0, 10), mk_el(5, 15)],
        )),
        ("重复绑定", user_img_msg(
            "[Image #1][Image #1]",
            vec![u1.clone(), u1.clone()],
            vec![mk_el(0, 10), mk_el(11, 21)],
        )),
        ("非 UTF-8 边界", {
            // content = "中"（3 字节）+ 标记；区间 1..11 落在"中"中间
            let content = "中[Image #1]";
            user_img_msg(content, vec![u1.clone()], vec![mk_el(1, 11)])
        }),
    ];
    for (tag, item) in cases {
        let body = format!(
            "{META}\n{}\n{}\n{}\n",
            line(&text(HistoryRole::User, "先")),
            line(&batch("u0", "exec_command", "{}")),
            line(&item),
        );
        seed(&path, &body);
        let loaded = load_session_file(&path).unwrap();
        assert_eq!(loaded.skipped_lines, 1, "{tag}：{loaded:?}");
        let imgs: Vec<&HistoryItem> = loaded
            .history
            .iter()
            .filter(|h| matches!(h, HistoryItem::Text(m) if m.images.is_some()))
            .collect();
        assert!(imgs.is_empty(), "{tag}：坏图行不得入史");
    }
}

/// 旧记录兼容：无新字段的行照常；多张图＋标记按序校验通过。
#[test]
fn user_image_old_rows_and_multi_image_order() {
    let dir = TempDir::new("s-uimg2");
    let path = session_path(&dir);
    let u1 = b64_url(b"AAA");
    let u2 = b64_url(b"BBB");
    let ok = user_img_msg(
        "[Image #1] 中间 [Image #2]",
        vec![u1, u2],
        vec![
            TextElement::new(ByteRange { start: 0, end: 10 }, None),
            TextElement::new(ByteRange { start: 18, end: 28 }, None),
        ],
    );
    seed(&path, &format!("{META}\n{}\n", line(&ok)));
    let loaded = load_session_file(&path).unwrap();
    assert_eq!(loaded.skipped_lines, 0);
    assert_eq!(loaded.history.len(), 1);
}

/// 029：Move 工具结果（有序两条记录）与展示（Add/Delete 两卡片）往返保真；
/// 装载只读历史，不重放移动——会话外同路径的现有文件不被触碰。
#[test]
fn move_tool_result_and_display_roundtrip_without_replay() {
    use terminal_chat::prompt::{PatchChange, PatchChangeKind, ToolCallRecord, ToolDisplay};
    let dir = TempDir::new("s029-mv");
    // 恢复不得触碰的真实文件对（与历史记录中的路径同名的哨兵）
    let fs_pair = TempDir::new("s029-mv-fs");
    let src = fs_pair.path().join("旧 名.txt");
    let dst = fs_pair.path().join("新 名.txt");
    std::fs::write(&src, "SENTINEL-029\n").unwrap();

    let hist = vec![
        HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: "029 移动历史".into(),
            user_input_event: None,
        }),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: "call-mv029".into(),
                name: "apply_patch".into(),
                arguments: "{}".into(),
            }],
            content: None,
        },
        HistoryItem::ToolResult {
            image: None,
            tool_call_id: "call-mv029".into(),
            content: serde_json::json!({
                "ok": true,
                "changed_files": [
                    { "path": dst.display().to_string(), "action": "add" },
                    { "path": src.display().to_string(), "action": "delete" }
                ],
                "state_known": true,
                "error": null
            })
            .to_string()
            .into(),
            display: Some(ToolDisplay::Patch {
                changes: vec![
                    PatchChange {
                        path: dst.display().to_string(),
                        kind: PatchChangeKind::Add { content: "新内容\n".into() },
                    },
                    PatchChange {
                        path: src.display().to_string(),
                        kind: PatchChangeKind::Delete { content: Some("旧内容\n".into()) },
                    },
                ],
            }),
        },
    ];
    terminal_chat::session::save_snapshot(&dir.path(), "mv029", &hist).unwrap();
    let loaded =
        terminal_chat::session::load_session_file(&dir.path().join("mv029.jsonl")).unwrap();
    assert_eq!(loaded.history.len(), 3, "零修复装载：{}", loaded.repairs);
    assert_eq!(loaded.repairs, 0);
    match &loaded.history[2] {
        HistoryItem::ToolResult { tool_call_id, content, display, .. } => {
            assert_eq!(tool_call_id, "call-mv029");
            let v: serde_json::Value = serde_json::from_str(content).unwrap();
            assert_eq!(v["changed_files"].as_array().unwrap().len(), 2, "两条记录保真：{v}");
            assert_eq!(v["changed_files"][0]["action"], serde_json::json!("add"), "顺序保真");
            assert_eq!(v["changed_files"][1]["action"], serde_json::json!("delete"));
            let Some(ToolDisplay::Patch { changes }) = display else {
                panic!("展示字段应保留：{display:?}")
            };
            assert_eq!(changes.len(), 2, "两张卡片：{changes:?}");
            assert!(matches!(&changes[0].kind, PatchChangeKind::Add { content } if content == "新内容\n"));
            assert!(matches!(&changes[1].kind, PatchChangeKind::Delete { content } if content.as_deref() == Some("旧内容\n")));
        }
        other => panic!("形状：{other:?}"),
    }
    // 恢复不重放：装载仅读历史，同名路径的现有文件不被移动/删除
    assert_eq!(
        std::fs::read_to_string(&src).unwrap(),
        "SENTINEL-029\n",
        "历史中的 delete 不得在装载时重放"
    );
    assert!(!dst.exists(), "历史中的 add 不得在装载时重放");
}
