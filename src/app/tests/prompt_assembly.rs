//! 005 规格验收：最小模型请求组装九类场景，全部核对真实序列化 JSON。
//! 母本一致性用独立锚点（字节数＋SHA-256）验证；发送文本用测试侧独立重建做差分对照，
//! 不与实现自我比较。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use terminal_chat::prompt::{
    build_request, sent_instructions, HistoryItem, HistoryMessage, HistoryRole,
    RAW_BASE_INSTRUCTIONS,
};
use terminal_chat::{apply_patch, exec};

/// 005 规格给定的母本锚点（固定提交 2fdf047c 的 models-manager/prompt.md）。
const RAW_LEN: usize = 20903;
const RAW_SHA256: &str = "ac8ae107a0d72fe3476b430afb161ea4e67da2e446d778aefc44828160559807";

fn msg(role: HistoryRole, content: &str) -> HistoryItem {
    HistoryItem::Text(HistoryMessage { role, content: content.to_string() })
}

/// 1. 来源一致性（嵌入母本）：字节数与 SHA-256 独立锚定，非实现自我比较。
#[test]
fn s1_embedded_source_matches_pinned_commit() {
    assert_eq!(RAW_BASE_INSTRUCTIONS.len(), RAW_LEN, "嵌入母本字节数漂移");
    let hex: String = Sha256::digest(RAW_BASE_INSTRUCTIONS.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hex, RAW_SHA256, "嵌入母本与固定提交不一致");
}

/// 测试侧独立重建发送版提示词：按 005 §3 的描述剔除两节＋两处行内修正，
/// 与实现（Codex 过滤器移植＋替换函数）形成差分对照。
fn expected_sent() -> String {
    let raw = RAW_BASE_INSTRUCTIONS;
    let mut kept: Vec<&str> = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l == "## Planning" || l == "## `update_plan`" {
            i += 1;
            while i < lines.len() && !lines[i].starts_with("# ") && !lines[i].starts_with("## ") {
                i += 1;
            }
            continue;
        }
        kept.push(l);
        i += 1;
    }
    let mut text = kept.join("\n");
    text.push('\n'); // 母本以换行结尾，lines() 会丢掉它

    let old_agents = "The contents of the AGENTS.md file at the root of the repo and any directories from the CWD up to the root are included with the developer message and don't need to be re-read.";
    // 015 反转 005 适配 3：声明改为「已由 user 消息提供、无需重读」
    let new_agents = "The applicable AGENTS.md instructions and environment context are provided in a user message before the current input and don't need to be re-read.";
    assert_eq!(text.matches(old_agents).count(), 1);
    text = text.replacen(old_agents, new_agents, 1);

    let example_marker = "- Use the `apply_patch` tool to edit files";
    let line = raw.lines().find(|l| l.contains(example_marker)).unwrap();
    let prefix = "{\"command\":[\"apply_patch\",";
    let start = line.find(prefix).unwrap();
    let end = line.len() - "]}".len();
    let old_arg = &line[start..];
    let new_arg = format!("{{\"patch\":{}}}", &line[start + prefix.len()..end]);
    assert_eq!(text.matches(old_arg).count(), 1);
    text = text.replacen(old_arg, &new_arg, 1);
    text
}

/// 2. 提示词适配（发送文本）：与嵌入母本是不同文本；只含 005 §3 列出的适配，
/// 并与测试侧独立重建逐字节一致。
#[test]
fn s2_sent_prompt_only_listed_adaptations() {
    let sent = sent_instructions();
    let raw = RAW_BASE_INSTRUCTIONS;
    assert_ne!(sent, raw, "发送文本不得等于母本（未做适配）");
    assert!(sent.len() < raw.len(), "剔除两节后应更短");
    assert!(!sent.contains("update_plan"), "update_plan 指引应已剔除");
    assert!(!sent.contains("## Planning"), "## Planning 节应已剔除");
    assert!(
        !sent.contains("{\"command\":[\"apply_patch\""),
        "apply_patch 旧 shell 包装示例应已改写"
    );
    assert_eq!(sent.matches("{\"patch\":").count(), 1, "改写后的示例应恰一处");
    assert!(
        !sent.contains("included with the developer message"),
        "AGENTS.md 注入声明应已修正"
    );
    // 015（c8）：适配 3 反转——旧声明（母本句与 005 句）零残留，新声明恰一处
    assert!(!sent.contains("not injected automatically"), "005 旧声明不得残留");
    assert!(!sent.contains("read the applicable AGENTS.md files"), "005 旧声明不得残留");
    assert_eq!(
        sent.matches("provided in a user message before the current input").count(),
        1,
        "015 新声明恰出现一次"
    );
    assert!(
        sent.contains("don't need to be re-read"),
        "新声明保留无需重读语义"
    );
    // 第 4 项适配＝保留原文：已知差异措辞仍在
    assert!(sent.contains("escalated"), "sandbox/审批升级措辞应保留原文");
    assert!(sent.contains("## Task execution"), "无关章节不得被改动");
    // 差分对照：与测试侧独立重建逐字节一致
    assert_eq!(sent, expected_sent(), "发送文本与独立重建不一致（存在未列出的改动）");
}

/// 3. 空历史：恰 system、user 两条消息；恰两个工具定义；顶层恰 {messages, tools} 且均为
///    function 类型（原 s7 场景并入）；核对序列化 JSON。
#[test]
fn s3_empty_history_two_messages_two_tools() {
    let v = build_request(&[], "你好").unwrap();
    let text = serde_json::to_string(&v).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap(); // 验证真实 JSON，不只看内部结构
    let messages = v["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], json!("system"));
    assert_eq!(messages[0]["content"], json!(sent_instructions()));
    assert_eq!(messages[1]["role"], json!("user"));
    assert_eq!(messages[1]["content"], json!("你好"));
    let tools = v["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["function"]["name"], json!("exec_command"));
    assert_eq!(tools[1]["function"]["name"], json!("apply_patch"));
    // 协议映射（原 s7 断言并入）：顶层恰 {messages, tools}（model 等由发送层附加）、tools 全为 function
    let mut top: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    top.sort_unstable();
    assert_eq!(top, vec!["messages", "tools"], "组装产物恰两字段");
    assert!(tools.iter().all(|t| t["type"] == json!("function")), "tools 均为 function 类型");
}

/// 4. 两轮既有历史：角色、顺序、原文逐字保留；与当前输入文字相同也不去重；
///    特殊字符样本（原 s5 场景）逐字投影与往返；同一输入重复组装两次各自核对（原 s8 场景）。
#[test]
fn s4_history_preserved_no_dedup() {
    let tricky = "中文“引号”'单引号'\n第二行\t制表符 🎉 反斜杠\\ 结束";
    let history = vec![
        msg(HistoryRole::User, tricky),
        msg(HistoryRole::Assistant, "Agent：收到"),
    ];
    let snapshot: Vec<(HistoryRole, String)> = history
        .iter()
        .map(|h| match h {
            HistoryItem::Text(m) => (m.role, m.content.clone()),
            other => panic!("纯文本历史不应出现工具记录：{other:?}"),
        })
        .collect();
    let v = build_request(&history, tricky).unwrap(); // 本轮输入与首条历史同文
    let messages = v["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["system", "user", "assistant", "user"], "顺序固定");
    assert_eq!(messages[1]["content"], json!(tricky), "历史原文逐字保留（特殊字符）");
    assert_eq!(messages[2]["content"], json!("Agent：收到"));
    assert_eq!(messages[3]["content"], json!(tricky), "当前输入在末位逐字投影");
    // 不去重：与历史同文的本轮输入作为消息内容恰出现两次
    let hits = messages
        .iter()
        .filter(|m| m["content"] == json!(tricky))
        .count();
    assert_eq!(hits, 2);
    // 序列化为真实 JSON：换行以 \n 转义；再解析后特殊字符逐字往返一致（原 s5 断言）
    let text = serde_json::to_string(&v).unwrap();
    assert!(text.contains("\\n"), "换行必须被 JSON 转义而非裸写");
    let back: Value = serde_json::from_str(&text).unwrap();
    let back_msgs = back["messages"].as_array().unwrap();
    assert_eq!(back_msgs[1]["content"], json!(tricky), "历史特殊字符往返一致");
    assert_eq!(back_msgs[3]["content"], json!(tricky), "本轮输入特殊字符往返一致");

    // 重复组装（005“重复组装”验收）：同一输入再组装一次，对结果分别核对
    // 预期消息与工具——不以“两份相等”作为正确性证据
    let again = build_request(&history, tricky).unwrap();
    let msgs2 = again["messages"].as_array().unwrap();
    assert_eq!(msgs2.len(), 4, "重复组装消息数正确");
    let roles2: Vec<&str> = msgs2
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles2, vec!["system", "user", "assistant", "user"], "重复组装角色序正确");
    assert_eq!(msgs2[1]["content"], json!(tricky), "重复组装历史逐字正确");
    assert_eq!(msgs2[3]["content"], json!(tricky), "重复组装本轮输入逐字正确");
    let tools2 = again["tools"].as_array().unwrap();
    assert_eq!(tools2.len(), 2, "重复组装工具数正确");
    assert_eq!(tools2[0]["function"]["name"], json!("exec_command"));
    assert_eq!(tools2[1]["function"]["name"], json!("apply_patch"));

    // 组装（含重复组装）不改历史输入
    let after: Vec<(HistoryRole, String)> = history
        .iter()
        .map(|h| match h {
            HistoryItem::Text(m) => (m.role, m.content.clone()),
            other => panic!("纯文本历史不应出现工具记录：{other:?}"),
        })
        .collect();
    assert_eq!(snapshot, after);
}

/// 6. 两工具白名单：名称、参数与本地 003/004 JSON 契约一致，无未实现字段。
#[test]
fn s6_tool_whitelist_matches_local_contracts() {
    let v = build_request(&[], "hi").unwrap();
    let tools = v["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2, "白名单恰两项");
    let exec_t = &tools[0];
    assert_eq!(exec_t["type"], json!("function"));
    let f = &exec_t["function"];
    assert_eq!(f["name"], json!("exec_command"));
    assert!(!f["description"].as_str().unwrap().contains("PTY"), "描述应为同步语义");
    let params = &f["parameters"];
    assert_eq!(params["type"], json!("object"));
    assert_eq!(params["required"], json!(["cmd"]));
    assert_eq!(params["additionalProperties"], json!(false));
    let mut keys: Vec<&str> = params["properties"].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["cmd", "max_output_chars", "timeout_ms", "workdir"]);
    // 差分锚定本地 003 契约：按 schema 构造最小实参必须被 ExecArgs 接受，越界字段必须拒绝
    let mut args = serde_json::Map::new();
    for k in ["cmd", "workdir", "timeout_ms", "max_output_chars"] {
        args.insert(
            k.to_string(),
            serde_json::Value::String("/tmp".into()),
        );
    }
    args.insert("cmd".into(), json!("ls"));
    args.insert("timeout_ms".into(), json!(100));
    args.insert("max_output_chars".into(), json!(100));
    args.remove("workdir").unwrap();
    assert!(exec::ExecArgs::from_value(&Value::Object(args.clone())).is_ok());
    args.insert("tty".into(), json!(true));
    assert!(exec::ExecArgs::from_value(&Value::Object(args)).is_err(), "未声明字段必须被契约拒绝");

    let ap = &tools[1];
    assert_eq!(ap["type"], json!("function"));
    let f = &ap["function"];
    assert_eq!(f["name"], json!("apply_patch"));
    let desc = f["description"].as_str().unwrap();
    assert!(desc.contains("can be used to edit files"), "复用 Codex 首句");
    assert!(!desc.contains("FREEFORM") && !desc.contains("do not wrap"), "不得残留 freeform 措辞");
    assert!(!desc.contains("Move to"), "不得声明未实现的 Move to");
    let params = &f["parameters"];
    assert_eq!(params["required"], json!(["patch"]));
    assert_eq!(params["additionalProperties"], json!(false));
    let keys: Vec<&str> = params["properties"].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["patch"]);
    // 差分锚定本地 004 契约：仅接受 patch 字段；额外字段在解析前拒绝
    let r = apply_patch::apply_patch(&json!({ "patch": "", "evil": 1 }));
    assert!(r.error.unwrap().contains("未知参数"));
    let r = apply_patch::apply_patch(&json!({ "patch": "*** Begin Patch\n*** End Patch" }));
    assert!(r.error.unwrap().contains("空补丁"), "patch 为唯一有效字段（到达解析层）");
}

/// 9. 空白本轮输入：明确错误，不产生请求片段；历史不变。
#[test]
fn s9_blank_input_rejected() {
    let history = vec![msg(HistoryRole::User, "历史保留")];
    for blank in ["", "  \n\t ", "\u{3000}"] {
        let err = build_request(&history, blank).unwrap_err();
        assert!(err.contains("空白") || err.contains("空"), "错误应说明空白输入：{err}");
    }
    assert_eq!(history.len(), 1);
    match &history[0] {
        HistoryItem::Text(m) => assert_eq!(m.content, "历史保留", "拒绝路径不得改动历史"),
        other => panic!("纯文本历史不应出现工具记录：{other:?}"),
    }
}
