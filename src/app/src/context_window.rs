//! 032 上下文管理：离线预算估算（get_context_remaining）、活动视图与检查点
//! 语义校验（new_context 的执行与批末屏障在 turn 层接入）。
//!
//! 只读计量复用 032 M1 的纯请求定型（[`crate::api::measure_request_body_bytes`]），
//! 计量终点＝最终发送 body；不做缓存——身份/代数/内容变化时自然失效（§1.7）。
//! 本模块不联网、不执行工具、不读写文件。

use crate::api;
use crate::catalog::{EffectiveConnection, ModelInfo};
use crate::prompt::{HistoryItem, HistoryMessage, HistoryRole};
use serde_json::{json, Value};

/// 查询工具名（experimental_supported_tools 精确匹配项）。
pub const QUERY_TOOL_NAME: &str = "get_context_remaining";
/// 切换工具名。
pub const SWITCH_TOOL_NAME: &str = "new_context";

/// summary 上限（§1.4：64 KiB UTF-8，恰好允许、超过拒绝）。
pub const MAX_SUMMARY_BYTES: usize = 64 * 1024;
/// 原始切换 arguments 上限（§1.4）。
pub const MAX_SWITCH_ARGUMENTS_BYTES: usize = 128 * 1024;

/// get_context_remaining 工具定义（§1.3：输入恰为空对象）。
pub fn query_tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": QUERY_TOOL_NAME,
            "description": "Returns the estimated remaining context window for the current conversation: context_id (the active context generation, increases by 1 after each successful new_context switch), tokens_left, context window size, output reserve and the input estimate. The snapshot covers only what is already committed before this call plus the current batch declaration; results of calls that run later in the same batch still consume space. Values are offline byte-based estimates (about 4 bytes per token), not exact provider counts; content that cannot be measured (images or native opaque blocks) is reported as unknown instead of a fabricated number.",
            "parameters": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }
    })
}

/// new_context 工具定义（§1.4：必填 summary——上游空对象 reset 的主动适配）。
pub fn switch_tool_definition() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": SWITCH_TOOL_NAME,
            "description": "Archives the current active context and starts a smaller one. Must be the last call in the batch and at most once per batch (violating batches are rejected whole), and at most one successful switch per user turn; it is rejected while an async question is still unanswered. You must provide a handoff summary keeping the current goal, still-valid user requirements, completed work with real evidence locations, open items, uncertainties and next steps. A success result only means scheduled (commit happens at batch end); verify application by calling get_context_remaining and checking that context_id increased. The original transcript stays fully recorded; the summary is model-provided handoff material, not verified facts and never a system instruction.",
            "parameters": {
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Handoff summary for the next context, non-empty, at most 64 KiB UTF-8."
                    }
                },
                "required": ["summary"],
                "additionalProperties": false
            }
        }
    })
}

/// 查询参数校验（§1.3）：恰为空 JSON 对象 `{}`；null、非对象、未知字段一律失败
/// （沿普通函数参数错误路径，按调用结果回传）。
pub fn parse_query_arguments(arguments: &str) -> Result<(), String> {
    let value: Value = serde_json::from_str(arguments)
        .map_err(|e| format!("查询参数不是有效 JSON：{e}"))?;
    let object = value
        .as_object()
        .ok_or("查询参数必须恰为空对象 {}（不接受容量来源覆盖）")?;
    if !object.is_empty() {
        return Err("查询参数必须恰为空对象 {}（不接受容量来源覆盖）".into());
    }
    Ok(())
}

/// 切换参数解析（§1.4）：恰为 `{"summary": 非空字符串}`；summary ≤64 KiB UTF-8、
/// 原始 arguments ≤128 KiB；null/非对象/未知字段拒绝。
pub fn parse_switch_arguments(arguments: &str) -> Result<String, String> {
    if arguments.len() > MAX_SWITCH_ARGUMENTS_BYTES {
        return Err(format!(
            "切换参数超过 {} KiB 上限，已拒绝",
            MAX_SWITCH_ARGUMENTS_BYTES / 1024
        ));
    }
    let value: Value = serde_json::from_str(arguments)
        .map_err(|e| format!("切换参数不是有效 JSON：{e}"))?;
    let object = value
        .as_object()
        .ok_or("切换参数必须是含 summary 的 JSON 对象")?;
    if object.len() != 1 || !object.contains_key("summary") {
        return Err("切换参数只接受 summary 字段（不接受历史索引、路径、身份或权限参数）".into());
    }
    let summary = object["summary"]
        .as_str()
        .ok_or("summary 必须是非空字符串")?;
    if summary.is_empty() {
        return Err("summary 不能为空".into());
    }
    if summary.len() > MAX_SUMMARY_BYTES {
        return Err(format!("summary 超过 {} KiB UTF-8 上限", MAX_SUMMARY_BYTES / 1024));
    }
    Ok(summary.to_string())
}

/// 正整数判定（参数值来自已校验 request_params 或模型目录）。
fn positive_u64(value: &Value) -> Option<u64> {
    value.as_u64().filter(|v| *v > 0)
}

/// 输出预留 R（§1.3）：先取当前已校验请求实际设置的正整数输出上限（按协议
/// 取参数位置）；未显式设置时以正整数 `model.max_output` 作保守参考。
/// 互斥上限并存、零值、缺失或不合法组合不猜测（Err 携带明确原因）。
pub fn output_reserve(conn: &EffectiveConnection, model: &ModelInfo) -> Result<u64, String> {
    let params = &conn.request_params;
    let explicit: Option<Option<u64>> = match conn.protocol.as_str() {
        // Chat：max_completion_tokens 与 max_tokens 互斥——并存即歧义
        "openai-chat" => {
            let a = params.get("max_completion_tokens");
            let b = params.get("max_tokens");
            match (a, b) {
                (Some(_), Some(_)) => {
                    return Err("输出预留未知：max_completion_tokens 与 max_tokens 同时设置".into())
                }
                (Some(v), None) | (None, Some(v)) => Some(positive_u64(v)),
                (None, None) => None,
            }
        }
        "openai-responses" => params.get("max_output_tokens").map(positive_u64),
        "anthropic-messages" => params.get("max_tokens").map(positive_u64),
        // Gemini generateContent：generationConfig.maxOutputTokens（嵌套对象）
        "google-generate-content" => params
            .get("generationConfig")
            .and_then(|v| v.get("maxOutputTokens"))
            .map(positive_u64),
        // Interactions：generation_config.max_output_tokens（嵌套对象）
        "google-interactions" => params
            .get("generation_config")
            .and_then(|v| v.get("max_output_tokens"))
            .map(positive_u64),
        other => return Err(format!("输出预留未知：协议 {other} 不在映射内")),
    };
    match explicit {
        Some(Some(limit)) => Ok(limit),
        Some(None) => Err("输出预留未知：请求设置的输出上限不是正整数".into()),
        None => model
            .max_output
            .filter(|v| *v > 0)
            .ok_or_else(|| "输出预留未知：请求未设置输出上限且模型目录缺少正整数 max_output".into()),
    }
}

/// 上下文窗口 C（§1.3）：模型目录正整数 context_length；缺失/零/不合法 → Err。
pub fn context_window_tokens(model: &ModelInfo) -> Result<u64, String> {
    model
        .context_length
        .filter(|v| *v > 0)
        .ok_or_else(|| "上下文窗口未知：模型目录缺少正整数 context_length".into())
}

/// 032 M3：检查点提交宿主——由宿主实现（生产＝会话保存器）。与普通 on_record
/// 的无返回值回调不同，本 trait 必须可报告失败；失败时检查点不得进入内存历史
/// （失败候选不被后续补写/退出 flush/手动保存复活，§1.5 与调研 M3 裁决）。
pub trait CheckpointHost {
    /// 提交检查点：先把滞留待写段＋检查点行写入目标（首次创建 V3 auto 目标时
    /// 以完整 transcript 开篇），全部成功后返回已提交条数（真实回执）；任一
    /// 失败截回已提交长度并返回原因。成功回执是活动视图切换的唯一依据。
    fn commit_checkpoint(
        &mut self,
        history: &[HistoryItem],
        checkpoint: &HistoryItem,
    ) -> Result<usize, String>;
}

/// 032：上下文管理工具宿主（查询与切换的声明/执行共用快照；每用户轮固定）。
/// `query_declared`/`switch_declared`＝模型目录开关；`checkpoints`＝None 表示
/// 无真实检查点提交宿主——此时不声明、不执行 new_context（§1.2）。
pub struct ContextHost<'a> {
    pub conn: &'a EffectiveConnection,
    pub key: &'a str,
    pub model: &'a ModelInfo,
    pub query_declared: bool,
    pub switch_declared: bool,
    pub checkpoints: Option<&'a mut dyn CheckpointHost>,
}

/// 纯文本内容的近似输入估算（§1.3）：I=ceil(B/4)，复用既有近似换算；
/// 溢出防护——字节数超 usize 时按 usize::MAX 计（仍远小于 u64 token 空间）。
fn approx_input_tokens(bytes: u64) -> u64 {
    let bounded = usize::try_from(bytes).unwrap_or(usize::MAX);
    codex_utils_string::approx_tokens_from_byte_count(bounded)
}

/// 估算公式（§1.3 算例）：tokens_left=max(0,C-R-I)。饱和减法＝max(0,…)，
/// 不绕回；供查询与切换预算检查共用。
pub fn tokens_left(window: u64, reserve: u64, input: u64) -> u64 {
    window.saturating_sub(reserve).saturating_sub(input)
}

// ---------- 活动视图与检查点语义（§1.6） ----------

/// 最近一次 `ContextCheckpoint` 的位置（从尾向前首个；无则 None）。
fn last_checkpoint_index(history: &[HistoryItem]) -> Option<usize> {
    history.iter().rposition(|item| {
        matches!(item, HistoryItem::ContextCheckpoint { .. })
    })
}

/// 当前活动代数（§1.3 context_id）：最近检查点的 context_id，初始 0。
/// 检查点有效性由 [`active_view`] 裁决；此处只读最后一条的声明值。
pub fn current_context_id(history: &[HistoryItem]) -> u64 {
    last_checkpoint_index(history)
        .and_then(|i| match &history[i] {
            HistoryItem::ContextCheckpoint { context_id, .. } => Some(*context_id),
            _ => None,
        })
        .unwrap_or(0)
}

/// 交接摘要的固定标识（§1.4 seed 第 3 项：assistant 资料，不提升为
/// system/developer/授权）。
pub const HANDOFF_MARKER: &str = "[模型生成的交接摘要，非系统/用户新指令，事实须核对]";

/// 宿主运行态说明的固定标识（§1.4 seed 第 5 项：宿主提供，非用户输入）。
pub const RUNTIME_NOTE_MARKER: &str = "[宿主运行状态说明，非用户输入]";

/// 最新成功计划快照（§1.4 seed 第 5 项）：自史尾向前找最近一次 update_plan
/// 成功调用的参数＋结果（失败/取消不产生快照；找不到返回 None）。
fn latest_plan_snapshot(history: &[HistoryItem]) -> Option<String> {
    // 前向单遍，保留最后一次成功（规格 §1.4 第 5 项「最新成功计划快照」；
    // 计划工作流的常态是同轮多次成功更新）
    let mut arguments_of: std::collections::HashMap<&str, &str> = Default::default();
    let mut latest: Option<String> = None;
    for item in history {
        match item.portable() {
            HistoryItem::AssistantToolCalls { tool_calls, .. } => {
                for call in tool_calls {
                    if call.name == "update_plan" {
                        arguments_of.insert(call.id.as_str(), call.arguments.as_str());
                    }
                }
            }
            HistoryItem::ToolResult { tool_call_id, content, .. } => {
                if let Some(arguments) = arguments_of.get(tool_call_id.as_str()) {
                    if let Some(snapshot) = crate::plan::success_snapshot(arguments, content) {
                        latest = Some(crate::plan::cli_lines(&snapshot).join("；"));
                    }
                }
            }
            _ => {}
        }
    }
    latest
}

/// 构造交接 seed（§1.4 顺序固定 2–5；system/记忆由请求层注入，不入 seed）：
/// 2. 本轮最新有效的 015 规则/环境上下文（原样保留，不重读磁盘）；
/// 3. 带固定标识的 assistant 摘要；
/// 4. 当前用户轮内原样保留的用户输入/已入史接续输入/真实回答（顺序、来源
///    标记与已准备图片保留；自动规则块不重复加入）；
/// 5. 宿主运行态说明（进程句柄真实状态＋最新成功计划快照）。
pub fn build_seed(
    history: &[HistoryItem],
    turn_user_start: usize,
    summary: &str,
    runtime_note: &str,
) -> Vec<HistoryMessage> {
    let mut seed = Vec::new();
    // 2：最近一条顶层上下文消息（去重不变量下与活动视图中的最近上下文同文；
    // 原样保留，含替换声明）
    if let Some(body) = crate::context::last_context_body(history) {
        seed.push(HistoryMessage {
            role: HistoryRole::User,
            content: body.to_string(),
            user_input_event: None,
            images: None,
            text_elements: Vec::new(),
        });
    }
    // 3：带标识的交接摘要（assistant 资料）
    seed.push(HistoryMessage {
        role: HistoryRole::Assistant,
        content: format!("{HANDOFF_MARKER}\n\n{summary}"),
        user_input_event: None,
        images: None,
        text_elements: Vec::new(),
    });
    // 4：当前用户轮的 user 输入（含接续输入与问询回答；排除自动规则块）
    for item in &history[turn_user_start.min(history.len())..] {
        if let HistoryItem::Text(m) = item.portable() {
            if m.role == HistoryRole::User
                && crate::context::context_body(&m.content).is_none()
            {
                seed.push(m.clone());
            }
        }
    }
    // 5：宿主运行态说明
    seed.push(HistoryMessage {
        role: HistoryRole::User,
        content: runtime_note.to_string(),
        user_input_event: None,
        images: None,
        text_elements: Vec::new(),
    });
    seed
}

/// 构造检查点记录（§1.6：context_id＝上一有效代数＋1；提交时点由调用方
/// 重建 seed 以纳入批末新入史输入）。
pub fn build_checkpoint(
    history: &[HistoryItem],
    turn_user_start: usize,
    request_call_id: &str,
    summary: &str,
    runtime_note: &str,
) -> HistoryItem {
    HistoryItem::ContextCheckpoint {
        context_id: current_context_id(history) + 1,
        request_call_id: request_call_id.to_string(),
        summary: summary.to_string(),
        seed: build_seed(history, turn_user_start, summary, runtime_note),
    }
}

/// 检查点的候选视图条目（seed 展开；唯一映射点，审查修复 #15）。
pub fn checkpoint_view(checkpoint: &HistoryItem) -> Vec<HistoryItem> {
    match checkpoint {
        HistoryItem::ContextCheckpoint { seed, .. } => {
            seed.iter().map(|m| HistoryItem::Text(m.clone())).collect()
        }
        _ => Vec::new(),
    }
}

/// 候选构造＋校验单一入口（审查修复 #15：分派排定与批末复验共用同一序列）。
/// 返回构造完成的检查点（提交方直接提交；调用方自行决定是否保留）。
#[allow(clippy::too_many_arguments)]
pub fn evaluate_candidate(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    in_flight_request: &Value,
    history: &[HistoryItem],
    turn_user_start: usize,
    request_call_id: &str,
    summary: &str,
    procs: &crate::proc_session::ProcessManager,
) -> Result<HistoryItem, String> {
    let runtime_note = runtime_state_note(procs, history);
    let checkpoint = build_checkpoint(
        history,
        turn_user_start,
        request_call_id,
        summary,
        &runtime_note,
    );
    let old_view = active_view(history).map_err(|e| e)?.items();
    let candidate_view = checkpoint_view(&checkpoint);
    validate_switch(
        conn,
        key,
        model,
        in_flight_request,
        &old_view,
        &candidate_view,
    )?;
    Ok(checkpoint)
}

/// 候选请求体：与在途请求同工具/同 system/同参数，`_history` 换为候选视图。
fn candidate_request(in_flight_request: &Value, view: &[HistoryItem]) -> Value {
    let mut request = in_flight_request.clone();
    request["_history"] = serde_json::to_value(view).expect("候选视图可序列化");
    request
}

/// 候选体积裁决（§1.5）：候选实际请求须过 10 MiB 硬边界；旧窗口未超限时还须
/// 严格小于旧请求字节；旧窗口已超限时有界计量的超限结论即为下界——合规候选
/// 据此证明严格缩小，不为比较而完整物化超限 JSON（计数经流式计量，不物化）。
pub fn switch_size_verdict(old_bytes: Option<u64>, new_bytes: u64) -> Result<(), String> {
    const LIMIT: u64 = api::MAX_REQUEST_BODY_BYTES;
    if new_bytes > LIMIT {
        return Err(format!(
            "候选活动请求 {} 字节超过 {} MiB 上限",
            new_bytes,
            LIMIT / 1024 / 1024
        ));
    }
    match old_bytes {
        Some(old) if new_bytes >= old => Err(format!(
            "候选活动请求（{new_bytes} 字节）不小于当前窗口请求（{old} 字节），无可压缩空间"
        )),
        // 旧窗口超限（None＝有界计数已越限）：候选合规即严格缩小
        None => Ok(()),
        Some(_) => Ok(()),
    }
}

/// 切换候选校验（§1.5 handler 前置：摘要/状态/候选视图/预算）：
/// 构造 seed→候选请求计量→10 MiB/严格缩小→预算可估算时 I+R<C。
/// `old_view`/`candidate_view` 均为 HistoryItem 序列（候选＝seed 展开）。
pub fn validate_switch(
    conn: &EffectiveConnection,
    key: &str,
    model: &ModelInfo,
    in_flight_request: &Value,
    old_view: &[HistoryItem],
    candidate_view: &[HistoryItem],
) -> Result<(), String> {
    let candidate = candidate_request(in_flight_request, candidate_view);
    let new_bytes = api::measure_request_body_bytes(conn, key, model, &candidate)
        .map_err(|e| format!("候选请求无法编码：{e}"))?;
    let old = candidate_request(in_flight_request, old_view);
    // 032 审查修复 #9/#11：旧窗口计量有界（超限即停，不完成全量序列化）；
    // 编码失败（Err）与超限分离——失败时裁决不可用（fail-closed），唯一允许
    // 跳过「严格缩小」比较的情形是真实超限的有界下界结论（§1.5）。
    let old_bytes = match api::measure_request_body_bytes_bounded(
        conn,
        key,
        model,
        &old,
        api::MAX_REQUEST_BODY_BYTES,
    ) {
        Ok(Some(bytes)) => Some(bytes),
        Ok(None) => None,
        Err(reason) => {
            return Err(format!("当前活动窗口无法编码，切换裁决不可用：{reason}"));
        }
    };
    switch_size_verdict(old_bytes, new_bytes)?;
    if estimable(candidate_view) {
        if let (Ok(window), Ok(reserve)) =
            (context_window_tokens(model), output_reserve(conn, model))
        {
            let estimate = approx_input_tokens(new_bytes);
            // 032 审查修复：饱和加法——u64 极大合法预留不回绕放行
            if estimate.saturating_add(reserve) >= window {
                return Err(format!(
                    "候选估算输入 {estimate}＋输出预留 {reserve} 不小于上下文窗口 {window}，预算不足"
                ));
            }
        }
    }
    Ok(())
}

/// 宿主运行态说明文本（§1.4 第 5 项）：已登记进程句柄及真实状态＋最新成功
/// 计划快照；不把已退出未取走输出的句柄称作仍在运行，不重放旧授权/工具结果。
pub fn runtime_state_note(procs: &crate::proc_session::ProcessManager, history: &[HistoryItem]) -> String {
    let mut note = format!("{RUNTIME_NOTE_MARKER}\n");
    let live = procs.live_sessions();
    if live.is_empty() {
        note.push_str("- 无已登记进程会话\n");
    } else {
        for (id, cmd) in live {
            let state = match procs.session_running(&id) {
                Some(true) => "运行中",
                Some(false) => "已退出（句柄仍登记，输出可能未取尽）",
                None => "状态未知",
            };
            note.push_str(&format!("- 进程会话 {id}（{cmd}）：{state}\n"));
        }
    }
    match latest_plan_snapshot(history) {
        Some(text) => note.push_str(&format!("- 当前计划快照：{text}\n")),
        None => note.push_str("- 无已保存计划\n"),
    }
    note
}

/// 查询结果的一行展示（CLI/TUI 共用；界面显示"估算/未知"，不只显示貌似
/// 精确的百分比，§1.3）。
pub fn query_display_line(result_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(result_json) else {
        return format!("[工具] {QUERY_TOOL_NAME} 结果无法解析");
    };
    if let Some(error) = v["error"].as_str() {
        return format!("[工具] 上下文余量查询失败：{error}");
    }
    let id = v["context_id"].as_u64().unwrap_or(0);
    if v["accuracy"].as_str() == Some("estimate") {
        format!(
            "[工具] 上下文余量（估算，代数 {id}）：约 {} / {} tokens 可用（输入估算 {}，输出预留 {}，请求字节 {}）",
            v["tokens_left"].as_u64().unwrap_or(0),
            v["context_window_tokens"].as_u64().unwrap_or(0),
            v["input_tokens_estimate"].as_u64().unwrap_or(0),
            v["output_reserve_tokens"].as_u64().unwrap_or(0),
            v["input_bytes"].as_u64().unwrap_or(0),
        )
    } else {
        let reason = v["unknown_reason"].as_str().unwrap_or("原因未知");
        let known = match (v["context_window_tokens"].as_u64(), v["output_reserve_tokens"].as_u64()) {
            (Some(c), Some(r)) => format!("（窗口 {c}，预留 {r} 已知）"),
            (Some(c), None) => format!("（窗口 {c} 已知）"),
            (None, Some(r)) => format!("（预留 {r} 已知）"),
            _ => String::new(),
        };
        format!("[工具] 上下文余量：未知（代数 {id}）{known}——{reason}")
    }
}

/// 切换调用结果的一行展示（scheduled＝等待批末提交；applied 由检查点回执的
/// Info 通知另行呈现）。
pub fn switch_call_display_line(result_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(result_json) else {
        return format!("[工具] {SWITCH_TOOL_NAME} 结果无法解析");
    };
    if v["ok"] == json!(true) {
        "[工具] 上下文切换已排定：将在本批结束后提交（scheduled，非已应用）".to_string()
    } else {
        format!(
            "[工具] 上下文切换被拒绝：{}",
            v["error"].as_str().unwrap_or("原因未知")
        )
    }
}
/// 检查点链全量语义校验（032 审查修复 #12：装载与提交两个状态变化点执行；
/// 请求/查询路径改走 [`active_view`] 的轻量本地校验，不再随归档前缀线性扫描）：
/// 代数自 1 连续、批位闭合（前缀无未配对调用）、request_call_id 关联成功
/// 排定的 new_context 调用、seed 仅 User/Assistant 文本（assistant 不带附件）。
pub fn validate_checkpoint_chain(history: &[HistoryItem]) -> Result<(), String> {
    checkpoint_semantics(history)
}

fn checkpoint_semantics(history: &[HistoryItem]) -> Result<(), String> {
    let mut expected_id = 1u64;
    let mut open_calls: Vec<String> = Vec::new();
    // 已见 new_context 调用 id（前向单遍维护——关联检查 O(1)，不随检查点数
    // 或历史长度二次恶化，§1.8 资源边界）
    let mut switch_calls: std::collections::HashSet<String> = Default::default();
    let mut switch_arguments: std::collections::HashMap<String, String> = Default::default();
    for item in history {
        match item {
            HistoryItem::AssistantToolCalls { tool_calls, .. } => {
                for call in tool_calls {
                    open_calls.push(call.id.clone());
                    if call.name == SWITCH_TOOL_NAME {
                        switch_arguments.insert(call.id.clone(), call.arguments.clone());
                    }
                }
            }
            HistoryItem::ToolResult { tool_call_id, content, .. } => {
                open_calls.retain(|id| id != tool_call_id);
                // 032 审查修复 #24：关联判定只认「成功排定」的切换调用——
                // 结果须为 {"ok":true,"status":"scheduled"}（被拒绝的调用不能
                // 挂载检查点，C12 伪造拒绝）
                let scheduled = serde_json::from_str::<Value>(content)
                    .ok()
                    .is_some_and(|v| v["ok"] == json!(true) && v["status"] == json!("scheduled"));
                if scheduled {
                    if let Some(arguments) = switch_arguments.get(tool_call_id.as_str()) {
                        if serde_json::from_str::<Value>(arguments)
                            .ok()
                            .and_then(|v| v.get("summary").cloned())
                            .is_some()
                        {
                            switch_calls.insert(tool_call_id.clone());
                        }
                    }
                }
            }
            HistoryItem::ContextCheckpoint {
                context_id,
                request_call_id,
                summary,
                seed,
            } => {
                if *context_id != expected_id {
                    return Err(format!(
                        "上下文检查点代数不连续（期望 {expected_id}，实为 {context_id}）"
                    ));
                }
                if !open_calls.is_empty() {
                    return Err("上下文检查点位于未闭合工具批次中".into());
                }
                if request_call_id.is_empty() || !switch_calls.contains(request_call_id.as_str()) {
                    return Err("上下文检查点缺少关联的 new_context 调用".into());
                }
                if summary.is_empty() || summary.len() > MAX_SUMMARY_BYTES {
                    return Err("上下文检查点 summary 为空或超过 64 KiB".into());
                }
                if seed.iter().any(|m| {
                    m.role == HistoryRole::Assistant
                        && m.images.as_ref().is_some_and(|v| !v.is_empty())
                }) {
                    return Err("上下文检查点 seed 的 assistant 消息不能携带附件".into());
                }
                expected_id += 1;
            }
            _ => {}
        }
    }
    Ok(())
}

/// 活动视图（§1.6）：最近有效检查点的 seed＋其后真实历史；无检查点＝完整历史。
/// 语义校验失败返回 Err——调用方（请求组装/查询）不得静默回退全历史。
pub enum ActiveView<'a> {
    Full {
        history: &'a [HistoryItem],
    },
    Checkpoint {
        seed: &'a [HistoryMessage],
        tail: &'a [HistoryItem],
    },
}

impl ActiveView<'_> {
    /// 视图条目（请求组装与计量共用；检查点本身不进入，seed 展开为其消息）。
    pub fn items(&self) -> Vec<HistoryItem> {
        match self {
            Self::Full { history } => history.to_vec(),
            Self::Checkpoint { seed, tail } => {
                let mut items: Vec<HistoryItem> =
                    seed.iter().map(|m| HistoryItem::Text(m.clone())).collect();
                items.extend_from_slice(tail);
                items
            }
        }
    }
}

/// 解析活动视图：无检查点＝Full；有检查点＝校验语义后取最近检查点。
/// 从尾向前定位——有检查点时只扫活动尾部，查询不随归档前缀线性恶化（§1.8）。
pub fn active_view(history: &[HistoryItem]) -> Result<ActiveView<'_>, String> {
    let Some(index) = last_checkpoint_index(history) else {
        return Ok(ActiveView::Full { history });
    };
    // 032 审查修复 #12：链级校验（代数连续等）在装载与提交两个状态变化点
    // 执行（validate_checkpoint_chain / build_checkpoint 自证）；本路径只做
    // 最近检查点的轻量本地校验——自检查点向前回扫至紧邻批（O(活动尾部)），
    // 不随归档前缀线性恶化（§1.8/C17）。
    validate_last_checkpoint_locally(history, index)
        .map_err(|e| format!("活动上下文检查点无效：{e}"))?;
    let HistoryItem::ContextCheckpoint { seed, .. } = &history[index] else {
        unreachable!("已按变体定位");
    };
    Ok(ActiveView::Checkpoint {
        seed,
        tail: &history[index + 1..],
    })
}

/// 最近检查点的本地校验（O(检查点紧邻批)）：批位闭合（自检查点向前回扫，
/// 紧邻批的全部调用已有配对结果）、request_call_id 关联成功排定的
/// new_context 调用（结果 {"ok":true,"status":"scheduled"}）、seed 形状
///（assistant 不带附件、summary 非空且 ≤64 KiB）、代数 ≥1。
fn validate_last_checkpoint_locally(history: &[HistoryItem], index: usize) -> Result<(), String> {
    let HistoryItem::ContextCheckpoint { context_id, request_call_id, summary, seed } =
        &history[index]
    else {
        unreachable!("已按变体定位");
    };
    if *context_id == 0 {
        return Err("上下文检查点代数为 0".into());
    }
    if summary.is_empty() || summary.len() > MAX_SUMMARY_BYTES {
        return Err("上下文检查点 summary 为空或超过 64 KiB".into());
    }
    if seed.iter().any(|m| {
        m.role == HistoryRole::Assistant && m.images.as_ref().is_some_and(|v| !v.is_empty())
    }) {
        return Err("上下文检查点 seed 的 assistant 消息不能携带附件".into());
    }
    // 自检查点向前回扫：pending 收集已见结果；遇到的第一个批声明（紧邻批）
    // 其全部调用须已有配对结果（否则检查点位于未闭合批中）；继续回扫直到
    // 找到 request_call_id 关联且结果为成功排定的 new_context 调用——合法
    // 提交中该调用就在同轮紧邻位置，回扫距离有界（O(活动尾部)）。
    let mut pending: std::collections::HashMap<&str, &str> = Default::default();
    let mut linked_scheduled = false;
    let mut first_batch = true;
    for item in history[..index].iter().rev() {
        match item.portable() {
            HistoryItem::ToolResult { tool_call_id, content, .. } => {
                pending.insert(tool_call_id.as_str(), content.as_str());
            }
            HistoryItem::AssistantToolCalls { tool_calls, .. } => {
                let unclosed = tool_calls
                    .iter()
                    .any(|call| !pending.contains_key(call.id.as_str()));
                if first_batch && unclosed {
                    return Err("上下文检查点位于未闭合工具批次中".into());
                }
                first_batch = false;
                for call in tool_calls {
                    if call.id == *request_call_id && call.name == SWITCH_TOOL_NAME {
                        linked_scheduled = pending
                            .get(call.id.as_str())
                            .and_then(|content| serde_json::from_str::<Value>(content).ok())
                            .is_some_and(|v| {
                                v["ok"] == json!(true) && v["status"] == json!("scheduled")
                            });
                    }
                }
                if linked_scheduled {
                    break;
                }
            }
            _ => {}
        }
    }
    if request_call_id.is_empty() || !linked_scheduled {
        return Err("上下文检查点缺少成功排定的 new_context 调用关联".into());
    }
    Ok(())
}

/// 活动视图是否含不可计量内容（§1.3）：图片附件/图片载荷（Base64 不按 /4 冒充
/// token）或原生 opaque 块（NativeAssistant——签名/加密内容不可文本估算）。
fn estimable(view: &[HistoryItem]) -> bool {
    !view.iter().any(|item| match item {
        HistoryItem::NativeAssistant { .. } => true,
        item => match item.portable() {
            HistoryItem::Text(m) => m.images.as_ref().is_some_and(|v| !v.is_empty()),
            HistoryItem::ToolResult { image, .. } => image.is_some(),
            _ => false,
        },
    })
}

/// 查询结果的固定字段骨架（§1.3）：未知字段先置 null，按已知度回填。
struct QueryOutcome {
    tokens_left: Option<u64>,
    accuracy: &'static str,
    context_window: Option<u64>,
    output_reserve: Option<u64>,
    input_estimate: Option<u64>,
    input_bytes: Option<u64>,
    context_id: u64,
    unknown_reason: Option<String>,
    error: Option<String>,
}

impl QueryOutcome {
    fn to_json(&self) -> Value {
        json!({
            "tokens_left": self.tokens_left,
            "accuracy": self.accuracy,
            "context_window_tokens": self.context_window,
            "output_reserve_tokens": self.output_reserve,
            "input_tokens_estimate": self.input_estimate,
            "input_bytes": self.input_bytes,
            "context_id": self.context_id,
            "unknown_reason": self.unknown_reason,
            "error": self.error,
        })
    }
}

/// 查询计量输入：当前批在途请求（工具/系统与本批声明一致）＋执行时点的已提交
/// 历史前缀（含本批声明与此前已保存结果，§1.3 快照定义）。
pub struct QueryInput<'a> {
    pub conn: &'a EffectiveConnection,
    pub key: &'a str,
    pub model: &'a ModelInfo,
    /// 当前批真实发送的请求体（messages/tools/_history 与发送同构）。
    pub in_flight_request: &'a Value,
    /// 查询执行时点的完整内存历史（活动视图由此派生）。
    pub history: &'a [HistoryItem],
}

/// 执行查询（§1.3）：零 HTTP/零工具/零文件加载；不改变历史或环境。
/// 返回固定字段结果 JSON（参数/本地故障 error 非空；不可计量→unknown）。
pub fn execute_query(input: &QueryInput<'_>) -> Value {
    let view = match active_view(input.history) {
        Ok(view) => view,
        Err(reason) => {
            return QueryOutcome {
                tokens_left: None,
                accuracy: "unknown",
                context_window: None,
                output_reserve: None,
                input_estimate: None,
                input_bytes: None,
                context_id: current_context_id(input.history),
                unknown_reason: None,
                error: Some(reason),
            }
            .to_json()
        }
    };
    let items = view.items();
    let context_id = current_context_id(input.history);
    // 计量请求＝在途请求骨架构造（同工具/同 system/同参数）＋当前快照活动视图
    let mut request = input.in_flight_request.clone();
    request["_history"] = serde_json::to_value(&items).expect("活动视图可序列化");
    let bytes = match api::measure_request_body_bytes(input.conn, input.key, input.model, &request)
    {
        Ok(bytes) => bytes,
        Err(reason) => {
            // 前缀不能编码/协议不支持＝不可计量（§1.3），不伪造数字
            return QueryOutcome {
                tokens_left: None,
                accuracy: "unknown",
                context_window: None,
                output_reserve: None,
                input_estimate: None,
                input_bytes: None,
                context_id,
                unknown_reason: Some(format!("当前快照无法按协议编码为最终请求体：{reason}")),
                error: None,
            }
            .to_json();
        }
    };
    let window = context_window_tokens(input.model);
    let reserve = output_reserve(input.conn, input.model);
    if !estimable(&items) {
        let reason = match (&window, &reserve) {
            (Err(_), _) => format!("包含图片或不可计量的原生内容，且{}", window.as_ref().unwrap_err()),
            (Ok(_), Err(_)) => format!("包含图片或不可计量的原生内容，且{}", reserve.as_ref().unwrap_err()),
            (Ok(_), Ok(_)) => {
                "包含图片或不可计量的原生内容（Base64/密文不按字节估算 token）".to_string()
            }
        };
        return QueryOutcome {
            tokens_left: None,
            accuracy: "unknown",
            context_window: window.ok(),
            output_reserve: reserve.ok(),
            input_estimate: None,
            input_bytes: Some(bytes),
            context_id,
            unknown_reason: Some(reason),
            error: None,
        }
        .to_json();
    }
    match (window, reserve) {
        (Ok(window), Ok(reserve)) => {
            let estimate = approx_input_tokens(bytes);
            QueryOutcome {
                tokens_left: Some(tokens_left(window, reserve, estimate)),
                accuracy: "estimate",
                context_window: Some(window),
                output_reserve: Some(reserve),
                input_estimate: Some(estimate),
                input_bytes: Some(bytes),
                context_id,
                unknown_reason: None,
                error: None,
            }
        }
        .to_json(),
        (window, reserve) => {
            let reason = match (&window, &reserve) {
                (Err(e), _) => e.clone(),
                (_, Err(e)) => e.clone(),
                _ => "不可计量".into(),
            };
            QueryOutcome {
                tokens_left: None,
                accuracy: "unknown",
                context_window: window.ok(),
                output_reserve: reserve.ok(),
                input_estimate: None,
                input_bytes: Some(bytes),
                context_id,
                unknown_reason: Some(reason),
                error: None,
            }
        }
        .to_json(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with(window: Option<u64>, output: Option<u64>) -> ModelInfo {
        ModelInfo {
            id: "m".into(),
            context_length: window,
            max_output: output,
            ..Default::default()
        }
    }

    fn conn(protocol: &str, params: &[(&str, Value)]) -> EffectiveConnection {
        EffectiveConnection {
            base_url: "https://example.test".into(),
            protocol: protocol.into(),
            key_ref: None,
            request_params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn query_arguments_must_be_exactly_empty_object() {
        assert!(parse_query_arguments("{}").is_ok());
        for bad in [
            "null", "[]", "\"x\"", "5", "{\"tokens_left\":1}", "{\"context_window_tokens\":8}", "{ }",
        ] {
            // "{ }" 带空白但仍是空对象——serde 归一化后通过
            if bad == "{ }" {
                assert!(parse_query_arguments(bad).is_ok(), "空白空对象应通过");
                continue;
            }
            assert!(parse_query_arguments(bad).is_err(), "{bad} 应拒绝");
        }
    }

    #[test]
    fn switch_arguments_shape_and_limits() {
        assert_eq!(
            parse_switch_arguments(r#"{"summary":"交接"}"#).unwrap(),
            "交接"
        );
        assert!(parse_switch_arguments("null").is_err());
        assert!(parse_switch_arguments("{}").is_err());
        assert!(parse_switch_arguments(r#"{"summary":""}"#).is_err());
        assert!(parse_switch_arguments(r#"{"summary":5}"#).is_err());
        assert!(parse_switch_arguments(r#"{"summary":"s","history":1}"#).is_err());
        let big = "x".repeat(MAX_SUMMARY_BYTES + 1);
        assert!(parse_switch_arguments(&json!({ "summary": big }).to_string()).is_err());
        let exact = "x".repeat(MAX_SUMMARY_BYTES);
        assert!(parse_switch_arguments(&json!({ "summary": exact }).to_string()).is_ok());
        let oversized = json!({ "summary": "x".repeat(MAX_SWITCH_ARGUMENTS_BYTES) }).to_string();
        assert!(parse_switch_arguments(&oversized).is_err(), "原始 arguments 超限拒绝");
    }

    /// §1.3 算例：B=32768、C=32768、R=4096 → I=8192、剩余 20480。
    #[test]
    fn spec_arithmetic_example() {
        let estimate = approx_input_tokens(32768);
        assert_eq!(estimate, 8192);
        assert_eq!(tokens_left(32768, 4096, estimate), 20480);
        // C-R<I → 0（不绕回）
        assert_eq!(tokens_left(100, 4096, 8192), 0);
        assert_eq!(tokens_left(u64::MAX, u64::MAX, u64::MAX), 0);
    }

    /// §1.5 体积裁决：候选超 10 MiB 拒绝；旧窗口未超限时须严格缩小；
    /// 旧窗口超限（None＝有界下界）时候选合规即证明缩小。
    #[test]
    fn switch_size_verdict_branches() {
        const LIMIT: u64 = 10 * 1024 * 1024;
        // 候选超限
        assert!(switch_size_verdict(Some(LIMIT * 2), LIMIT + 1).is_err());
        // 旧未超限＋候选更小 → 通过
        assert!(switch_size_verdict(Some(2_000), 1_000).is_ok());
        // 旧未超限＋候选相等/更大 → 拒绝
        assert!(switch_size_verdict(Some(1_000), 1_000).is_err());
        assert!(switch_size_verdict(Some(1_000), 2_000).is_err());
        // 旧窗口超限（None）＋候选合规 → 通过（下界即证明）
        assert!(switch_size_verdict(None, LIMIT).is_ok());
    }

    #[test]
    fn reserve_mapping_per_protocol_and_ambiguity() {
        let model = model_with(Some(8192), Some(512));
        // Chat：显式 max_tokens；并存互斥上限→未知；零值→未知
        assert_eq!(
            output_reserve(&conn("openai-chat", &[("max_tokens", json!(1024))]), &model),
            Ok(1024)
        );
        assert!(output_reserve(
            &conn(
                "openai-chat",
                &[("max_tokens", json!(1024)), ("max_completion_tokens", json!(512))]
            ),
            &model
        )
        .is_err());
        assert!(output_reserve(&conn("openai-chat", &[("max_tokens", json!(0))]), &model).is_err());
        // 未显式设置 → model.max_output 保守参考
        assert_eq!(output_reserve(&conn("openai-chat", &[]), &model), Ok(512));
        // Responses / Messages
        assert_eq!(
            output_reserve(&conn("openai-responses", &[("max_output_tokens", json!(2048))]), &model),
            Ok(2048)
        );
        assert_eq!(
            output_reserve(&conn("anthropic-messages", &[("max_tokens", json!(128))]), &model),
            Ok(128)
        );
        // Gemini 两族嵌套位置
        assert_eq!(
            output_reserve(
                &conn(
                    "google-generate-content",
                    &[("generationConfig", json!({"maxOutputTokens": 64}))]
                ),
                &model
            ),
            Ok(64)
        );
        assert_eq!(
            output_reserve(
                &conn(
                    "google-interactions",
                    &[("generation_config", json!({"max_output_tokens": 96}))]
                ),
                &model
            ),
            Ok(96)
        );
        // 缺省＋模型无正整数 max_output → 未知
        let bare = model_with(Some(8192), None);
        assert!(output_reserve(&conn("openai-chat", &[]), &bare).is_err());
        // 窗口缺失/零 → 未知
        assert!(context_window_tokens(&model_with(None, Some(1))).is_err());
        assert!(context_window_tokens(&model_with(Some(0), Some(1))).is_err());
    }
}
