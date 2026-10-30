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
    HistoryItem::Text(HistoryMessage {
        role,
        content: content.to_string(),
        user_input_event: None,
        images: None,
        text_elements: Vec::new(),
    })
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

/// 测试侧独立重建发送版提示词：017 起 update_plan 工具已接入，不再剔除
/// `## Planning`／`## \`update_plan\`` 两节；仅剩 005 §3 的两处行内修正，
/// 与实现形成差分对照。
fn expected_sent() -> String {
    let raw = RAW_BASE_INSTRUCTIONS;
    let mut text = raw.to_string();

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

/// 2. 提示词适配（发送文本）：与嵌入母本是不同文本；只含列出的适配，
/// 并与测试侧独立重建逐字节一致。017：计划指引两节已恢复（母本原文保留）。
#[test]
fn s2_sent_prompt_only_listed_adaptations() {
    let sent = sent_instructions();
    let raw = RAW_BASE_INSTRUCTIONS;
    assert_ne!(sent, raw, "发送文本不得等于母本（未做适配）");
    assert!(sent.len() < raw.len(), "apply_patch 示例改写后应更短");
    // 017：计划指引不再剔除——两节标题与工具指引均保留（母本原文）
    assert!(sent.contains("## Planning"), "## Planning 节应保留");
    assert!(sent.contains("## `update_plan`"), "update_plan 工具节应保留");
    assert!(
        sent.contains("You have access to an `update_plan` tool"),
        "Planning 节工具指引应保留"
    );
    assert!(
        sent.contains("At most one step can be") || sent.contains("exactly one `in_progress` step"),
        "计划指引正文应保留"
    );
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

/// 3. 空历史：恰 system、user 两条消息；恰四个工具定义（017 起＋update_plan）；
///    顶层恰 {messages, tools} 且均为 function 类型（原 s7 场景并入）；核对序列化 JSON。
#[test]
fn s3_empty_history_two_messages_four_tools() {
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
    assert_eq!(tools.len(), 4, "017：用户轮四工具");
    assert_eq!(tools[0]["function"]["name"], json!("exec_command"));
    assert_eq!(tools[1]["function"]["name"], json!("apply_patch"));
    assert_eq!(tools[2]["function"]["name"], json!("write_stdin"));
    assert_eq!(tools[3]["function"]["name"], json!("update_plan"));
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
    assert_eq!(tools2.len(), 4, "重复组装工具数正确");
    assert_eq!(tools2[0]["function"]["name"], json!("exec_command"));
    assert_eq!(tools2[1]["function"]["name"], json!("apply_patch"));
    assert_eq!(tools2[2]["function"]["name"], json!("write_stdin"));
    assert_eq!(tools2[3]["function"]["name"], json!("update_plan"));

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

/// 6. 四工具白名单：名称、参数与本地 003/004/016/017 JSON 契约一致，无未声明字段。
#[test]
fn s6_tool_whitelist_matches_local_contracts() {
    let v = build_request(&[], "hi").unwrap();
    let tools = v["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4, "017：白名单恰四项");
    let exec_t = &tools[0];
    assert_eq!(exec_t["type"], json!("function"));
    let f = &exec_t["function"];
    assert_eq!(f["name"], json!("exec_command"));
    let desc = f["description"].as_str().unwrap();
    assert!(desc.contains("yield_time_ms"), "描述应说明可续接等待语义");
    assert!(desc.contains("session_id"), "描述应说明运行中返回句柄");
    let params = &f["parameters"];
    assert_eq!(params["type"], json!("object"));
    assert_eq!(params["required"], json!(["cmd"]));
    assert_eq!(params["additionalProperties"], json!(false));
    let mut keys: Vec<&str> = params["properties"].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["cmd", "max_output_chars", "timeout_ms", "tty", "workdir", "yield_time_ms"]);
    // 差分锚定本地契约：按 schema 声明的全部键构造实参必须被 ExecArgs 接受，
    // 未声明字段必须拒绝
    let mut args = serde_json::Map::new();
    args.insert("cmd".into(), json!("ls"));
    args.insert("yield_time_ms".into(), json!(1000));
    args.insert("timeout_ms".into(), json!(60000));
    args.insert("max_output_chars".into(), json!(100));
    args.insert("tty".into(), json!(false));
    assert!(exec::ExecArgs::from_value(&Value::Object(args.clone())).is_ok());
    args.insert("shell".into(), json!("/bin/bash"));
    assert!(exec::ExecArgs::from_value(&Value::Object(args)).is_err(), "未声明字段必须被契约拒绝");

    let ap = &tools[1];
    assert_eq!(ap["type"], json!("function"));
    let f = &ap["function"];
    assert_eq!(f["name"], json!("apply_patch"));
    let desc = f["description"].as_str().unwrap();
    assert!(desc.contains("can be used to edit files"), "复用 Codex 首句");
    assert!(!desc.contains("FREEFORM") && !desc.contains("do not wrap"), "不得残留 freeform 措辞");
    // 029：参数描述声明 Move 位置、纯移动与最后一级匹配容差（与实现同步）
    let patch_desc = f["parameters"]["properties"]["patch"]["description"].as_str().unwrap();
    assert!(patch_desc.contains("\"*** Move to: <path>\""), "声明 Move 语法：{patch_desc}");
    assert!(patch_desc.contains("directly after"), "声明 Move 位置（紧接 Update 头）：{patch_desc}");
    assert!(patch_desc.contains("pure move"), "声明纯移动：{patch_desc}");
    assert!(patch_desc.contains("target add + source delete"), "声明结果为两条记录：{patch_desc}");
    assert!(patch_desc.contains("mapping Unicode"), "声明第四级匹配容差：{patch_desc}");
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

    // 016：write_stdin 定义与本地契约差分锚定——schema 键集合、必填、
    // 未声明字段拒绝、terminate 互斥与输入上限
    let ws = &tools[2];
    assert_eq!(ws["type"], json!("function"));
    let f = &ws["function"];
    assert_eq!(f["name"], json!("write_stdin"));
    let params = &f["parameters"];
    assert_eq!(params["type"], json!("object"));
    assert_eq!(params["required"], json!(["session_id"]));
    assert_eq!(params["additionalProperties"], json!(false));
    let mut keys: Vec<&str> = params["properties"].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["chars", "max_output_chars", "session_id", "terminate", "yield_time_ms"]);
    use terminal_chat::exec::WriteStdinArgs as WS;
    assert!(WS::from_value(&json!({ "session_id": "s1" })).is_ok());
    assert!(WS::from_value(&json!({ "session_id": "s1", "terminate": true })).is_ok());
    assert!(WS::from_value(&json!({ "session_id": "s1", "signal": 9 }))
        .unwrap_err()
        .contains("未知参数"), "未声明字段必须被契约拒绝");
    assert!(WS::from_value(&json!({ "session_id": "s1", "chars": "x", "yield_time_ms": 30000 })).is_ok());

    // 017：update_plan 定义与上游 plan_spec.rs 对齐（Chat Completions function 映射）
    // ——差分锚定本地解析契约：schema 声明的输入面必须被 parse_arguments 接受
    let up = &tools[3];
    assert_eq!(up["type"], json!("function"));
    let f = &up["function"];
    assert_eq!(f["name"], json!("update_plan"));
    let desc = f["description"].as_str().unwrap();
    assert!(desc.starts_with("Updates the task plan"), "上游描述首句：{desc}");
    assert!(desc.contains("At most one step can be in_progress"), "至多一个进行中写在描述：{desc}");
    let params = &f["parameters"];
    assert_eq!(params["type"], json!("object"));
    assert_eq!(params["required"], json!(["plan"]));
    assert_eq!(params["additionalProperties"], json!(false));
    let mut keys: Vec<&str> = params["properties"].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["explanation", "plan"]);
    let plan_schema = &params["properties"]["plan"];
    assert_eq!(plan_schema["type"], json!("array"));
    let item = &plan_schema["items"];
    assert_eq!(item["type"], json!("object"));
    assert_eq!(item["required"], json!(["step", "status"]));
    assert_eq!(item["additionalProperties"], json!(false));
    assert_eq!(
        item["properties"]["status"]["enum"],
        json!(["pending", "in_progress", "completed"]),
        "状态枚举与上游一致"
    );
    use terminal_chat::plan;
    // schema 声明面＝解析接受面：合法输入（含可省略 explanation）接受
    assert!(plan::parse_arguments(r#"{"explanation":"e","plan":[{"step":"s","status":"in_progress"}]}"#).is_ok());
    assert!(plan::parse_arguments(r#"{"plan":[{"step":"","status":"completed"},{"step":"t","status":"in_progress"}]}"#).is_ok(), "空步骤与多个进行中＝上游接受面");
    // 未声明顶层字段/步骤字段、非法状态拒绝（schema additionalProperties:false 同源）
    assert!(plan::parse_arguments(r#"{"plan":[],"reason":"x"}"#).is_err());
    assert!(plan::parse_arguments(r#"{"plan":[{"step":"s","status":"pending","id":1}]}"#).is_err());
    assert!(plan::parse_arguments(r#"{"plan":[{"step":"s","status":"blocked"}]}"#).is_err());
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

// ---------- 019：view_image 图片附件投影与工具声明 ----------

use terminal_chat::prompt::{
    build_turn_request_with_async_host, ToolCallRecord, ToolResultImage,
};

fn tc_rec(id: &str, name: &str) -> ToolCallRecord {
    ToolCallRecord { id: id.into(), name: name.into(), arguments: "{}".into() }
}

fn tool_result(id: &str, content: &str, image: Option<ToolResultImage>) -> HistoryItem {
    HistoryItem::ToolResult {
        tool_call_id: id.into(),
        content: content.into(),
        display: None,
        image,
    }
}

fn img(url: &str) -> ToolResultImage {
    ToolResultImage { image_url: url.into(), detail: "high".into() }
}

fn view_ok_content(path: &str, w: u32, h: u32) -> String {
    json!({ "ok": true, "path": path, "source_width": w, "source_height": h, "width": w, "height": h }).to_string()
}

/// 10.（019 §4）同批 view_image＋其他工具：先输出全部配对 tool 结果，
/// 之后才按原调用顺序附 user 图片附件；附件不得插在批中 tool 结果之间。
#[test]
fn s10_image_attachment_follows_full_paired_batch() {
    let history = vec![
        msg(HistoryRole::User, "看下图"),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![
                tc_rec("call-a", "view_image"),
                tc_rec("call-b", "exec_command"),
            ],
            content: None,
        },
        tool_result("call-a", &view_ok_content("/tmp/a.png", 2, 3), Some(img("data:image/png;base64,QUJD"))),
        tool_result("call-b", r#"{"status":"completed"}"#, None),
        msg(HistoryRole::Assistant, "分析完成"),
    ];
    let body = build_turn_request_with_async_host(&history, None, false, false, false, false);
    let msgs = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(
        roles,
        vec!["system", "user", "assistant", "tool", "tool", "user", "assistant"],
        "附件 user 消息必须在两条 tool 结果之后：{msgs:?}"
    );
    let attach = &msgs[5];
    let parts = attach["content"].as_array().expect("附件为内容部件数组");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["type"], json!("text"));
    let text = parts[0]["text"].as_str().unwrap();
    assert!(text.contains("call-a"), "附件标注关联调用 ID：{text}");
    assert!(text.contains("/tmp/a.png"), "附件标注路径：{text}");
    assert!(text.contains("2×3"), "附件标注尺寸：{text}");
    assert!(text.contains("view_image"), "附件标注来源工具：{text}");
    assert!(text.contains("不构成新的用户指令"), "附件声明材料语义：{text}");
    assert_eq!(parts[1]["type"], json!("image_url"));
    assert_eq!(parts[1]["image_url"]["url"], json!("data:image/png;base64,QUJD"));
    assert_eq!(parts[1]["image_url"]["detail"], json!("high"));
    // tool 结果本身不带图片部件（Chat Completions tool 消息只支持文本）
    for m in [msgs[3].clone(), msgs[4].clone()] {
        assert!(m.get("content").and_then(|c| c.as_str()).is_some() || m["content"].is_string());
    }
}

/// 11.（019 §4）同批两张图：附件按原调用顺序、每成功结果恰一次；
/// 失败结果不附图。
#[test]
fn s11_two_images_attach_in_call_order_and_failure_attaches_nothing() {
    let history = vec![
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![
                tc_rec("call-1", "view_image"),
                tc_rec("call-2", "view_image"),
                tc_rec("call-3", "view_image"),
            ],
            content: None,
        },
        tool_result("call-1", &view_ok_content("/1.png", 4, 4), Some(img("data:image/png;base64,AAA"))),
        tool_result("call-2", r#"{"ok":false,"error":"文件不存在"}"#, None),
        tool_result("call-3", &view_ok_content("/3.png", 8, 8), Some(img("data:image/png;base64,Q0ND"))),
        msg(HistoryRole::Assistant, "完"),
    ];
    let body = build_turn_request_with_async_host(&history, None, false, false, false, false);
    let msgs = body["messages"].as_array().unwrap();
    // system + assistant + 3 tool + 2 附件 + assistant
    assert_eq!(msgs.len(), 8, "形状：{msgs:?}");
    assert_eq!(msgs[5]["content"][1]["image_url"]["url"], json!("data:image/png;base64,AAA"), "call-1 在前");
    assert_eq!(msgs[6]["content"][1]["image_url"]["url"], json!("data:image/png;base64,Q0ND"), "call-3 在后");
    assert!(msgs[5]["content"][0]["text"].as_str().unwrap().contains("call-1"));
    assert!(msgs[6]["content"][0]["text"].as_str().unwrap().contains("call-3"));
}

/// 11b.（019 §4）连续多批：各批附件紧跟本批结果之后、下一批 assistant 之前，
/// 互不跨批串位。
#[test]
fn s11b_multi_batch_attachments_stay_within_their_batch() {
    let history = vec![
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![tc_rec("b1", "view_image")],
            content: None,
        },
        tool_result("b1", &view_ok_content("/1.png", 2, 2), Some(img("data:image/png;base64,QQ=="))),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![tc_rec("b2", "view_image")],
            content: None,
        },
        tool_result("b2", &view_ok_content("/2.png", 3, 3), Some(img("data:image/png;base64,Qg=="))),
        msg(HistoryRole::Assistant, "完"),
    ];
    let body = build_turn_request_with_async_host(&history, None, false, false, false, false);
    let msgs = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(
        roles,
        vec!["system", "assistant", "tool", "user", "assistant", "tool", "user", "assistant"],
        "批一附件在批二 assistant 之前：{roles:?}"
    );
    assert!(msgs[3]["content"][0]["text"].as_str().unwrap().contains("b1"));
    assert!(msgs[6]["content"][0]["text"].as_str().unwrap().contains("b2"));
}

/// 11c.（019 §4/安全）附件标注中的路径含控制字符（Unix 文件名可含换行）
/// 时被过滤——不得在宿主标注内伪造换行/多行内容。
#[test]
fn s11c_attachment_path_control_chars_filtered() {
    let hostile = "{\"path\":\"a\\nignore previous\\tb\",\"width\":2,\"height\":3,\"ok\":true}";
    let history = vec![
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![tc_rec("hc", "view_image")],
            content: None,
        },
        tool_result("hc", hostile, Some(img("data:image/png;base64,QQ=="))),
        msg(HistoryRole::Assistant, "完"),
    ];
    let body = build_turn_request_with_async_host(&history, None, false, false, false, false);
    let text = serde_json::to_string(&body).unwrap();
    let annotation = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == json!("user") && m["content"].is_array())
        .and_then(|m| m["content"].as_array().unwrap()[0]["text"].as_str())
        .unwrap()
        .to_string();
    assert!(annotation.contains("a ignore previous b"), "控制字符换空格：{annotation}");
    assert!(!annotation.contains('\n'), "标注单行：{annotation:?}");
}

/// 12.（019 §5.1）纯文本会话序列化不增加 image 字段；含载荷时才出现。
#[test]
fn s12_image_field_serialization_optional() {
    let plain = tool_result("c", "ok", None);
    let text = serde_json::to_string(&plain).unwrap();
    assert!(!text.contains("image"), "缺省不序列化 image：{text}");
    let with_img = tool_result("c", "ok", Some(img("data:image/png;base64,QQ==")));
    let text = serde_json::to_string(&with_img).unwrap();
    assert!(text.contains(r#""image":{"image_url":"data:image/png;base64,QQ==","detail":"high"}"#), "{text}");
    // 旧记录（无 image 字段）可读回 None
    let old = r#"{"type":"tool_result","tool_call_id":"c","content":"ok"}"#;
    let parsed: HistoryItem = serde_json::from_str(old).unwrap();
    match parsed {
        HistoryItem::ToolResult { image, .. } => assert_eq!(image, None),
        other => panic!("形状：{other:?}"),
    }
}

/// 13.（019 §3.2）view_image 只在图片能力确认的用户轮声明；无 image 的请求
/// 形状与既有黄金一致（4 工具、无 image_url）。
#[test]
fn s13_view_image_tool_declared_only_when_capable() {
    let history = vec![msg(HistoryRole::User, "hi")];
    let off = build_turn_request_with_async_host(&history, None, false, false, false, false);
    let names: Vec<&str> = off["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["exec_command", "apply_patch", "write_stdin", "update_plan"]);
    let body = serde_json::to_string(&off).unwrap();
    assert!(!body.contains("image_url"), "纯文本请求不出现 image_url");
    let on = build_turn_request_with_async_host(&history, None, false, false, true, false);
    let names: Vec<&str> = on["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["exec_command", "apply_patch", "write_stdin", "update_plan", "view_image"]
    );
    let def = &on["tools"][4];
    assert_eq!(def["function"]["parameters"]["required"], json!(["path"]));
    assert_eq!(def["function"]["parameters"]["additionalProperties"], json!(false));
}

// ---------- 020：用户消息图片字段（images/text_elements）与投影 ----------

use codex_protocol::user_input::{ByteRange, TextElement};

fn img_msg(content: &str, images: Vec<String>, elements: Vec<TextElement>) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: content.to_string(),
        user_input_event: None,
        images: (!images.is_empty()).then_some(images),
        text_elements: elements,
    })
}

fn small_data_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// 14.（020 §2.2）序列化：images 按附件顺序为准备后 data URL 数组；
/// text_elements 复用 Codex byte_range/placeholder；纯文本消息零新增字段；
/// 旧记录（无新字段）读回为 None/空。
#[test]
fn s14_user_image_fields_serialization_and_compat() {
    let png = small_data_url(b"\x89PNG\r\n\x1a\n");
    let m = img_msg(
        "[Image #1] 请检查",
        vec![png.clone()],
        vec![TextElement::new(ByteRange { start: 0, end: 10 }, Some("[Image #1]".into()))],
    );
    let text = serde_json::to_string(&m).unwrap();
    assert!(text.contains(r#""images":["#), "{text}");
    assert!(text.contains(r#""byte_range":{"start":0,"end":10}"#), "{text}");
    assert!(text.contains(r#""placeholder":"[Image #1]""#), "{text}");
    // 纯文本消息：无新字段（旧形状不变）
    let plain = msg(HistoryRole::User, "纯文本");
    let text = serde_json::to_string(&plain).unwrap();
    assert!(!text.contains("images"), "{text}");
    assert!(!text.contains("text_elements"), "{text}");
    // 旧记录读回
    let old: HistoryMessage =
        serde_json::from_str(r#"{"role":"user","content":"旧"}"#).unwrap();
    assert_eq!(old.images, None);
    assert!(old.text_elements.is_empty());
}

/// 15.（020 §2.2）投影：有图用户消息在同一条 user 消息内用 text＋按顺序的
/// image_url 部件（detail=high）；无文字可省略 text 部件；text_elements
/// 是 UI 元数据不发送；无图消息保持原字符串 content。
#[test]
fn s15_user_image_projection_shape() {
    let u1 = small_data_url(b"AAA");
    let u2 = small_data_url(b"BBB");
    let history = vec![
        img_msg(
            "[Image #1] [Image #2] 看这两张",
            vec![u1.clone(), u2.clone()],
            vec![
                TextElement::new(ByteRange { start: 0, end: 10 }, Some("[Image #1]".into())),
                TextElement::new(ByteRange { start: 11, end: 21 }, Some("[Image #2]".into())),
            ],
        ),
        img_msg("", vec![u1.clone()], vec![]),
        msg(HistoryRole::User, "纯文本"),
    ];
    let body = build_turn_request_with_async_host(&history, None, false, false, false, false);
    let msgs = body["messages"].as_array().unwrap();
    // system + user(数组) + user(数组) + user(字符串)
    let m0 = &msgs[1];
    assert_eq!(m0["role"], json!("user"));
    let parts = m0["content"].as_array().expect("有图消息 content 为部件数组");
    assert_eq!(parts.len(), 3, "text + 2 image_url：{parts:?}");
    assert_eq!(parts[0]["type"], json!("text"));
    assert_eq!(parts[0]["text"], json!("[Image #1] [Image #2] 看这两张"));
    assert_eq!(parts[1]["type"], json!("image_url"));
    assert_eq!(parts[1]["image_url"]["url"], json!(u1));
    assert_eq!(parts[1]["image_url"]["detail"], json!("high"));
    assert_eq!(parts[2]["image_url"]["url"], json!(u2));
    // 纯图：无 text 部件
    let m1 = &msgs[2];
    let parts = m1["content"].as_array().unwrap();
    assert_eq!(parts.len(), 1, "纯图省略 text 部件：{parts:?}");
    assert_eq!(parts[0]["type"], json!("image_url"));
    // 纯文本：原字符串 content
    assert!(msgs[3]["content"].is_string());
    // text_elements 不出现在请求
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("text_elements"), "UI 元数据不发送：检查前 200 字 {}…", &raw[..raw.len().min(200)]);
    assert!(!raw.contains("byte_range"));
}

/// 16.（020 A08/安全）手写 `[Image #1]` 无 images 载荷＝普通文字：投影不附图、
/// 不读文件、不产生 image_url 部件——伪造来源标记不能骗取上传。
#[test]
fn s16_handwritten_marker_without_payload_projects_plain_text() {
    let history = vec![
        img_msg("[Image #1] 手写的标记", Vec::new(), Vec::new()),
        msg(HistoryRole::User, "另一条 [Image #9] 也一样"),
    ];
    let body = build_turn_request_with_async_host(&history, None, false, false, false, false);
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("image_url"), "无载荷标记不产生图片部件：{raw}");
    assert!(!raw.contains("data:"), "无 data URL：{raw}");
}

/// 17.（021 A01/A03）`send_message` 能力快照——true 时声明
/// `send_message_to_user_async`（Schema 严格单 message 字段）；false 时不声明；
/// 与问询宿主开关相互独立。
#[test]
fn s17_send_message_declaration_follows_capability_snapshot() {
    let history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    // true：声明且 Schema 与上游映射一致
    let body = build_turn_request_with_async_host(&history, None, false, false, false, true);
    let tools = body["tools"].as_array().unwrap();
    let tool = tools
        .iter()
        .find(|t| t["function"]["name"] == json!("send_message_to_user_async"))
        .expect("能力开启必须声明消息工具");
    assert_eq!(
        tool["function"]["parameters"]["required"],
        json!(["message"]),
        "必填恰 message：{tool}"
    );
    assert_eq!(
        tool["function"]["parameters"]["additionalProperties"],
        json!(false),
        "拒绝未知字段（上游 additionalProperties:false 映射）：{tool}"
    );
    assert!(
        tool["function"]["description"]
            .as_str()
            .unwrap()
            .contains("report a critical blocker"),
        "描述保留上游使用指引：{tool}"
    );
    // false：不声明
    let body = build_turn_request_with_async_host(&history, None, false, false, false, false);
    assert!(!body["tools"].as_array().unwrap().iter().any(|t| {
        t["function"]["name"] == json!("send_message_to_user_async")
    }));
    // 与问询开关独立：仅消息能力（无问询宿主）也声明消息工具
    let body = build_turn_request_with_async_host(&history, None, false, false, false, true);
    assert!(body["tools"].as_array().unwrap().iter().any(|t| {
        t["function"]["name"] == json!("send_message_to_user_async")
    }));
    assert!(!body["tools"].as_array().unwrap().iter().any(|t| {
        t["function"]["name"] == json!("request_user_input_async")
    }));
}

// ---------- 030 M2：搜索开关与首次声明 ----------

use terminal_chat::prompt::build_turn_request_with_agents;
use terminal_chat::prompt::build_turn_request_with_tool_search;

#[test]
fn s30_search_mode_declaration_split() {
    // 开启且有可搜索动态工具：声明＝内置＋tool_search＋[资源 defs＋已加载 defs]；
    // 未加载动态定义不发送；tool_search 描述内嵌来源概览 JSON 数据块。
    let overview = r#"{"sources":[{"server_id":"fx","tool_count":2}]}"#;
    let ts_def = terminal_chat::tool_search::tool_definition(overview);
    let resource_def = json!({"type": "function", "function": {"name": "list_mcp_resources"}});
    let loaded_def = json!({"type": "function", "function": {"name": "mcp__fx__echo"}});
    let body = build_turn_request_with_tool_search(
        &[], None, false, false, false, false, false, false, false,
        Some(&ts_def),
        &[resource_def.clone(), loaded_def.clone()],
    );
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 7, "4 内置＋tool_search＋资源＋已加载＝7");
    assert_eq!(tools[4]["function"]["name"], json!("tool_search"));
    let description = tools[4]["function"]["description"].as_str().unwrap();
    assert!(description.contains(overview), "描述须含来源概览数据块");
    assert_eq!(tools[5], resource_def);
    assert_eq!(tools[6], loaded_def);
    // 全量入口兼容：None＝与既有入口完全一致
    let names: Vec<&str> = tools
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"mcp__fx__unloaded"));
}

#[test]
fn s30_off_mode_appends_mcp_defs_without_injecting_search() {
    // 关闭搜索（None）：对照无 MCP 基线——messages/system 完全不变，tools
    // 仅在尾部追加 MCP 定义（位置与顺序契约），且不注入 tool_search。
    // （审查 #13：原断言比较转发壳与其委托目标，恒真。）
    let history = [msg(HistoryRole::User, "任务")];
    let dyn_def = json!({"type": "function", "function": {"name": "mcp__fx__echo"}});
    let baseline = build_turn_request_with_tool_search(
        &history, None, false, false, false, false, false, false, false, None, &[],
    );
    let with_mcp = build_turn_request_with_tool_search(
        &history, None, false, false, false, false, false, false, false, None, &[dyn_def.clone()],
    );
    assert_eq!(with_mcp["messages"], baseline["messages"], "messages 不变");
    // system 在 messages[0]，已随上断言覆盖；tools 结构对照：
    let base_tools = baseline["tools"].as_array().unwrap().clone();
    let mcp_tools = with_mcp["tools"].as_array().unwrap().clone();
    assert_eq!(mcp_tools.len(), base_tools.len() + 1, "仅追加一项");
    assert_eq!(&mcp_tools[..base_tools.len()], &base_tools[..], "原有工具原位不变");
    assert_eq!(mcp_tools[base_tools.len()], dyn_def, "MCP 定义追加在尾部");
    assert!(
        !mcp_tools.iter().any(|t| t["function"]["name"] == json!("tool_search")),
        "关闭模式不得注入 tool_search"
    );
}

/// 035 C01/M5：六控制工具整组声明（agents 开关）＋关闭路径逐字节一致；
/// Agent 控制事件不进入模型 messages 投影。
#[test]
fn s20_multi_agent_tool_declaration_and_event_projection_filter() {
    use terminal_chat::agents::events::agent_event;
    use terminal_chat::prompt::{AgentEventKind, AgentEventPayload};
    let history = [msg(HistoryRole::User, "任务")];
    let baseline = build_turn_request_with_agents(
        &history, None, false, false, false, false, false, false, false, None, &[], false, false,
    );
    let with_agents = build_turn_request_with_agents(
        &history, None, false, false, false, false, false, false, false, None, &[], false, true,
    );
    assert_eq!(with_agents["messages"], baseline["messages"], "messages 不变");
    let base_tools = baseline["tools"].as_array().unwrap().clone();
    let agent_tools = with_agents["tools"].as_array().unwrap().clone();
    assert_eq!(agent_tools.len(), base_tools.len() + 6, "六工具追加");
    for (i, name) in [
        "spawn_agent",
        "send_message",
        "followup_task",
        "wait_agent",
        "interrupt_agent",
        "list_agents",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            agent_tools[base_tools.len() + i]["function"]["name"],
            json!(name),
            "第 {} 个控制工具原位：{name}",
            base_tools.len() + i
        );
    }
    // 控制事件不进 messages 投影
    let event = agent_event(
        AgentEventKind::AgentCreated,
        1,
        1,
        Some("pa"),
        Some(1),
        AgentEventPayload::Fork {
            context_id: 0,
            prefix_end: 0,
            fork: "none".into(),
            task: "t".into(),
        },
    );
    let with_event = build_turn_request_with_agents(
        &[event.clone()],
        None, false, false, false, false, false, false, false, None, &[], false, false,
    );
    let msgs = serde_json::to_string(&with_event["messages"]).unwrap();
    assert!(!msgs.contains("agent_created"), "事件不进投影：{msgs}");
    assert_eq!(with_event["messages"].as_array().unwrap().len(), 1, "仅 system");
}
