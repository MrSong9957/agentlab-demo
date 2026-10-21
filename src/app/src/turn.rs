//! REPL 一轮的最小工具调用循环（006 规格）：
//! 请求 → 解析 assistant → 整批结构校验 → 串行执行白名单工具 → 追加结果 → 再请求，
//! 无 `tool_calls` 时返回最终回复。仅循环与消息拼装：不联网细节（经 sender 回调）、
//! 不新增并行/流式/审批/重试/持久化。
//! 循环骨架与"结果入历史后继续请求"语义参照 Codex `turn.rs::run_turn`/`drain_in_flight`
//! 的串行子集；调用识别参照 `stream_events_utils.rs::handle_output_item_done`
//! （识别→记录→继续标记）；错误作为结果回传模型参照 `function_call_error.rs`
//! 的 `RespondToModel` 语义（固定提交 2fdf047c）。

use crate::apply_patch::ApplyPatchResult;
use crate::exec::{ExecResult, ExecStatus};
use crate::prompt::{
    build_turn_request, build_turn_request_with, build_turn_request_with_memory, HistoryItem,
    HistoryMessage, HistoryRole, ToolCallRecord, ToolDisplay,
};
use serde_json::{json, Value};
use std::path::Path;

/// 每用户轮模型请求上限（006：固定常量，无配置系统）。
pub const MAX_MODEL_REQUESTS_PER_TURN: usize = 8;
/// 单批 tool_calls 上限（2026-10-10 安全修复 N3：入史/执行前检查，超限整批拒绝）。
pub const MAX_TOOL_CALLS_PER_BATCH: usize = 32;

/// 一轮的工具面（013-M5 D3 参数化）：`User`＝既有用户轮契约（恰
/// `exec_command`/`apply_patch`，请求组装与取消形状逐字节不变）；
/// `MemoryMerge`＝内部合并轮（恰 `mem_read`/`mem_write`/`mem_list`，
/// 限 `root` 记忆根；请求组装注入三项工具与合并系统提示）。**权限在执行
/// 分派层生效**：不在当前模式注册表内的工具名（含另一模式的工具）一律按
/// 未知工具拒绝执行——仅靠请求体列出工具不构成权限保护。
#[derive(Debug, Clone, Copy)]
pub enum ToolMode<'a> {
    User,
    /// 013-M6：与 User 相同的工具能力；仅在本轮开始时读取可选摘要。
    /// 生产驱动传入配置/路径归 M7；不修改默认 User 或内部合并轮。
    UserWithMemory {
        root: &'a Path,
        config: &'a crate::memory::MemoriesConfig,
    },
    MemoryMerge {
        root: &'a Path,
        system: &'a str,
        /// 013-M7 收口：任务领取时的重置代数快照（不可变任务凭据——
        /// mem_write 写入校验用；与共享的当前代数区分，防旧任务借新代
        /// prepare 恢复写入）。
        expected_epoch: u64,
    },
}

/// 012：一轮内的工具活动通知（结构化展示事件）。`Info` 兼容原单行提示用途；
/// `ToolBegin`/`ToolEnd` 携带调用参数与执行结果原文，由宿主映射为各自的展示通道
///（行式＝打印 [`tool_activity_line`]，TUI＝工具 cell）；**展示数据（`display`）不进入
/// 发给模型的结果 JSON**——模型契约不变。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnNotify {
    Info(String),
    ToolBegin {
        id: String,
        name: String,
        arguments: String,
    },
    ToolEnd {
        id: String,
        name: String,
        arguments: String,
        /// 模型面结果 JSON（与入史 content 同文）。
        result: String,
        display: Option<ToolDisplay>,
        /// 012：取消批内跳过（未执行）＝true——取消呈现的事实来源（不反解析
        /// 消息文本）；exec 中途被杀的取消仍由结果 JSON 的 status 表达。
        cancelled: bool,
    },
}

/// 行式宿主的工具活动行（012：格式与历史输出逐字节一致，供宿主打印）。
pub fn tool_activity_line(name: &str, id: &str) -> String {
    format!(
        "[工具] {name}（{}）",
        id.chars().take(8).collect::<String>()
    )
}

/// 空白用户输入的统一错误文案（入口预检与循环内校验共用；prompt 层
/// `build_request` 另有一处属 005 既有契约文本）。
const BLANK_INPUT_ERROR: &str = "当前用户输入不能为空或全为空白";

/// 取消后未执行调用的结果：按对应工具的既定结果结构记录未执行原因——
/// exec_command/apply_patch 构造各自结果结构体序列化（字段与错误语义同工具本体同源）；
/// 其他名称（未知工具）与未知工具回传通道一致：单 `error` 字段通用结构，
/// 不伪装成 exec 结果。
/// 取消跳过的固定消息（012：恢复投影以此常量精确识别"未执行取消"——
/// 单一生产者 [`skipped_by_cancel`]，非模糊文本匹配）。
pub const SKIPPED_BY_CANCEL_MESSAGE: &str = "用户取消，本调用未执行";

fn skipped_by_cancel(name: &str, mode: &ToolMode<'_>) -> String {
    let error = SKIPPED_BY_CANCEL_MESSAGE;
    if let ToolMode::MemoryMerge { .. } = mode {
        if let Some(shape) = crate::memory::mem_tools::skipped_by_cancel_shape(name) {
            return shape;
        }
        return json!({ "error": error }).to_string();
    }
    match name {
        "exec_command" => serde_json::to_string(&ExecResult {
            status: ExecStatus::Cancelled,
            exit_code: None,
            output: String::new(),
            truncated: false,
            error: Some(error.to_string()),
        })
        .expect("ExecResult 可序列化"),
        "apply_patch" => serde_json::to_string(&ApplyPatchResult {
            ok: false,
            changed_files: Vec::new(),
            state_known: true,
            error: Some(error.to_string()),
        })
        .expect("ApplyPatchResult 可序列化"),
        _ => json!({ "error": error }).to_string(),
    }
}

/// 从 assistant 消息解析出的一条工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 整批校验 `tool_calls` 协议结构：每个调用须有非空 `id`/`name`、`type=="function"`、
/// 字符串 `arguments`，批内 `id` 不重复。任一项无效即整批无效（不执行、不入历史）。
/// `id`/`name` **原样保留**（006 §1）：trim 仅用于"全空白"判断，存储、唯一性判断
/// 与分派均使用原始值；`arguments` 字符串内的 JSON 无效不在此检查——那属参数错误，
/// 按工具失败结构回传。
fn parse_tool_calls(message: &Value) -> Result<Vec<ParsedToolCall>, String> {
    let Some(tc) = message.get("tool_calls") else {
        return Ok(Vec::new());
    };
    if tc.is_null() {
        return Ok(Vec::new());
    }
    let arr = tc
        .as_array()
        .ok_or_else(|| "协议错误：tool_calls 不是数组".to_string())?;
    let mut calls = Vec::with_capacity(arr.len());
    let mut seen = std::collections::HashSet::new();
    for c in arr {
        let id = c.get("id").and_then(Value::as_str).unwrap_or("");
        if id.trim().is_empty() {
            return Err("协议错误：工具调用缺少非空 id".into());
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("协议错误：批内调用 id 重复：{id}"));
        }
        if c.get("type").and_then(Value::as_str) != Some("function") {
            return Err("协议错误：工具调用 type 必须为 function".to_string());
        }
        let func = c
            .get("function")
            .ok_or_else(|| "协议错误：工具调用缺少 function 字段".to_string())?;
        let name = func.get("name").and_then(Value::as_str).unwrap_or("");
        if name.trim().is_empty() {
            return Err("协议错误：工具调用缺少非空工具名".to_string());
        }
        let arguments = func
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| "协议错误：工具调用 arguments 必须是字符串".to_string())?;
        calls.push(ParsedToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        });
    }
    Ok(calls)
}

/// 执行单个调用：按当前 [`ToolMode`] 的注册表分派；参数错误与工具失败按各
/// 工具既有结果结构回传 JSON 字符串（构造结果结构体序列化，字段与工具本体
/// 同源），保留失败/状态字段，不伪造成功；未注册工具（含跨模式工具名）不
/// 执行——分派层即权限边界（D3）。
/// 012：同时返回**展示数据**（exec 真实时长；apply_patch 逐文件真实差异）——随
/// 历史记录持久、随通知回推，但不混入结果 JSON（模型契约不变）；mem 工具
/// 为内部合并轮专用，无展示数据（None）。
fn execute_tool(
    call: &ParsedToolCall,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    mode: &ToolMode<'_>,
) -> (String, Option<ToolDisplay>) {
    if let ToolMode::MemoryMerge {
        root,
        expected_epoch,
        ..
    } = mode
    {
        return (
            crate::memory::mem_tools::execute(&call.name, &call.arguments, root, *expected_epoch),
            None,
        );
    }
    match call.name.as_str() {
        "exec_command" => match serde_json::from_str::<Value>(&call.arguments) {
            Ok(v) => {
                let started = std::time::Instant::now();
                let r = crate::exec::exec_command(&v, cancel);
                let display = ToolDisplay::Exec {
                    duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                };
                (
                    serde_json::to_string(&r).expect("ExecResult 可序列化"),
                    Some(display),
                )
            }
            Err(e) => (
                serde_json::to_string(&ExecResult {
                    status: ExecStatus::Error,
                    exit_code: None,
                    output: String::new(),
                    truncated: false,
                    error: Some(format!("工具参数不是有效 JSON：{e}")),
                })
                .expect("ExecResult 可序列化"),
                None,
            ),
        },
        "apply_patch" => match serde_json::from_str::<Value>(&call.arguments) {
            Ok(v) => {
                let (r, changes) = crate::apply_patch::apply_patch_with_display(&v);
                (
                    serde_json::to_string(&r).expect("ApplyPatchResult 可序列化"),
                    Some(ToolDisplay::Patch { changes }),
                )
            }
            Err(e) => (
                serde_json::to_string(&ApplyPatchResult {
                    ok: false,
                    changed_files: Vec::new(),
                    state_known: true,
                    error: Some(format!("工具参数不是有效 JSON：{e}")),
                })
                .expect("ApplyPatchResult 可序列化"),
                None,
            ),
        },
        other => (
            json!({
                "error": format!("未知工具 {other}，可用：exec_command、apply_patch")
            })
            .to_string(),
            None,
        ),
    }
}

/// 运行一个用户轮（`ToolMode::User` 固定面——签名与行为与 006/012 契约
/// 逐字节一致）。语义见 [`run_tool_turn_with_mode`]。
pub fn run_tool_turn(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
) -> Result<String, String> {
    run_tool_turn_with_mode(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        ToolMode::User,
    )
}

/// 015：用户轮入口——在压入本轮用户输入**之前**注入上下文消息（仅 `User`/
/// `UserWithMemory`；`MemoryMerge` 内部轮不读取不注入），随后完全复用既有循环
/// [`run_tool_turn_with_mode`]（签名与行为不变）。空白输入校验先于上下文写入：
/// 拒绝时历史与 `on_record` 零新增。来源读取每用户轮恰一次，轮内续请求不重读。
/// 内容与历史中最后一条上下文消息相同则不重复注入；不同则注入带替换声明的
/// 新消息（比较前剥除声明，基线由历史承载）。
pub fn run_user_turn_with_context(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    sources: &crate::context::ContextSources,
) -> Result<String, String> {
    if current_input.trim().is_empty() {
        return Err(BLANK_INPUT_ERROR.into());
    }
    if !matches!(mode, ToolMode::MemoryMerge { .. }) {
        let built = crate::context::build_context_message(sources);
        for notice in &built.notices {
            notify(TurnNotify::Info(notice.clone()));
        }
        let inject_text = match crate::context::last_context_body(history) {
            Some(previous) if previous == built.message => None,
            Some(_) => Some(format!(
                "{}\n\n{}",
                crate::prompt::CONTEXT_REPLACEMENT_DECLARATION,
                built.message
            )),
            None => Some(built.message),
        };
        if let Some(text) = inject_text {
            history.push(HistoryItem::Text(HistoryMessage {
                role: HistoryRole::User,
                content: text,
            }));
            on_record(history);
        }
    }
    run_tool_turn_with_mode(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
    )
}

/// 运行一轮（013-M5 D3 参数化）：`mode` 决定请求组装的工具集/system 与
/// 执行分派注册表；循环、整批校验、取消与上限语义对各模式完全一致
/// （合并轮沿用每轮 8 请求、每批 32 上限）。
/// `history` 为此前全部记录（调用方持有）；当前输入在本函数内
/// **只追加一次**。`sender` 发送 `{messages, tools}`（附 model 由其负责）并返回完整
/// 响应 JSON；`notify` 输出工具活动结构化事件（[`TurnNotify`]，过滤由调用方负责）。
/// `on_record` 在每个入史点（user/assistant 终文/tool_calls/每条结果）随完整历史回调
/// 一次——007 自动追加落盘的接入点，调用方负责游标与提交。
/// 返回最终回复；错误（协议/请求失败/上限）时历史保留已发生记录，不自动重发。
pub fn run_tool_turn_with_mode(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
) -> Result<String, String> {
    if current_input.trim().is_empty() {
        return Err(BLANK_INPUT_ERROR.into());
    }
    // 取消状态不跨用户轮（006：每轮开始复位）
    if let Some(c) = cancel {
        c.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    history.push(HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: current_input.to_string(),
    }));
    on_record(history);
    // 按用户轮读取一次，包含 None 的快照也固定；轮内续请求不重读。
    // 不把自动上下文写入 HistoryItem；合并轮与默认 User 均无读取入口。
    let memory_context = match &mode {
        ToolMode::UserWithMemory { root, config } => {
            crate::memory::inject::read_memory_context(root, config)
        }
        _ => None,
    };
    let mut requests = 0usize;
    loop {
        if requests >= MAX_MODEL_REQUESTS_PER_TURN {
            return Err(cap_error());
        }
        // 续请求使用完整历史（含本轮 user 与已有结果），不再次追加当前输入
        let body = match &mode {
            ToolMode::User => build_turn_request(history),
            ToolMode::UserWithMemory { .. } => {
                build_turn_request_with_memory(history, memory_context.as_deref())
            }
            ToolMode::MemoryMerge { system, .. } => build_turn_request_with(
                history,
                &crate::memory::mem_tools::tool_definitions(),
                system,
            ),
        };
        let resp = sender(&body)?;
        requests += 1;
        // 先整条校验消息与整批调用结构，再写入历史、执行
        let message = resp
            .pointer("/choices/0/message")
            .cloned()
            .ok_or_else(|| "协议错误：响应缺少 choices[0].message".to_string())?;
        let calls = parse_tool_calls(&message)?;
        let content = match message.get("content") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err("协议错误：assistant.content 必须是字符串或 null".into()),
        };
        // N3：批大小在入史/执行之前检查，超限整批拒绝（协议级失败，历史不污染）
        if calls.len() > MAX_TOOL_CALLS_PER_BATCH {
            return Err(format!(
                "单批工具调用 {} 条超过上限 {MAX_TOOL_CALLS_PER_BATCH}，整批拒绝",
                calls.len()
            ));
        }
        if calls.is_empty() {
            let reply = content.unwrap_or_default();
            if reply.trim().is_empty() {
                return Err("模型回复为空".into());
            }
            history.push(HistoryItem::Text(HistoryMessage {
                role: HistoryRole::Assistant,
                content: reply.clone(),
            }));
            on_record(history);
            return Ok(reply);
        }
        // 合法批：记录 assistant（content 原样保留），按返回顺序串行执行并逐条追加结果
        history.push(HistoryItem::AssistantToolCalls {
            tool_calls: calls
                .iter()
                .map(|c| ToolCallRecord {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect(),
            content,
        });
        on_record(history);
        let mut cancelled_batch = false;
        for call in &calls {
            if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst)) {
                // N2b：取消后剩余调用不执行，但每个已记录调用都要有匹配 tool 结果
                let content = skipped_by_cancel(&call.name, &mode);
                notify(TurnNotify::ToolEnd {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    result: content.clone(),
                    display: None,
                    cancelled: true,
                });
                history.push(HistoryItem::ToolResult {
                    tool_call_id: call.id.clone(),
                    content,
                    display: None,
                });
                on_record(history);
                cancelled_batch = true;
                continue;
            }
            notify(TurnNotify::ToolBegin {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            let (content, display) = execute_tool(call, cancel, &mode);
            notify(TurnNotify::ToolEnd {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                result: content.clone(),
                display: display.clone(),
                cancelled: false,
            });
            history.push(HistoryItem::ToolResult {
                tool_call_id: call.id.clone(),
                content,
                display,
            });
            on_record(history);
        }
        if cancelled_batch {
            // 取消：结束本轮，保留已发生结果与历史，不自动请求模型、不重复已执行工具
            return Err("用户已取消本轮工具执行：已执行结果保留，剩余调用未执行".into());
        }
        if requests == MAX_MODEL_REQUESTS_PER_TURN {
            // 第 8 批：保存全部结果后报上限，不发第 9 次请求，不生成假回复
            return Err(cap_error());
        }
    }
}

fn cap_error() -> String {
    format!("已达本轮模型请求上限（{MAX_MODEL_REQUESTS_PER_TURN}）")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg_with_tool_calls(tool_calls: Value) -> Value {
        json!({ "role": "assistant", "content": null, "tool_calls": tool_calls })
    }

    #[test]
    fn parse_accepts_valid_batch_and_rejects_bad_structure() {
        // 无 tool_calls / null：无调用
        assert!(
            parse_tool_calls(&json!({ "role": "assistant", "content": "hi" }))
                .unwrap()
                .is_empty()
        );
        assert!(parse_tool_calls(&msg_with_tool_calls(Value::Null))
            .unwrap()
            .is_empty());
        // 合法批
        let calls = parse_tool_calls(&msg_with_tool_calls(json!([
            { "id": "a", "type": "function", "function": { "name": "exec_command", "arguments": "{}" } },
            { "id": "b", "type": "function", "function": { "name": "apply_patch", "arguments": "not json" } }
        ])))
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[1].arguments, "not json",
            "arguments JSON 无效不在结构层拦截"
        );
        // 缺 id / type 错 / 缺名 / arguments 非字符串 / 批内重复 id → 协议错误
        for bad in [
            json!([{ "type": "function", "function": { "name": "x", "arguments": "{}" } }]),
            json!([{ "id": "a", "type": "custom", "function": { "name": "x", "arguments": "{}" } }]),
            json!([{ "id": "a", "type": "function", "function": { "arguments": "{}" } }]),
            json!([{ "id": "a", "type": "function", "function": { "name": "x", "arguments": 5 } }]),
            json!([
                { "id": "dup", "type": "function", "function": { "name": "x", "arguments": "{}" } },
                { "id": "dup", "type": "function", "function": { "name": "y", "arguments": "{}" } }
            ]),
        ] {
            let err = parse_tool_calls(&msg_with_tool_calls(bad)).unwrap_err();
            assert!(err.contains("协议错误"), "{err}");
        }
        // tool_calls 非数组
        assert!(parse_tool_calls(&msg_with_tool_calls(json!(5))).is_err());
    }

    /// 012：工具活动结构化事件——Begin（含参数原文）先于执行、End（含结果 JSON＋
    /// 展示数据）随结果；行式活动行格式与历史输出逐字节一致。
    #[test]
    fn tool_events_pair_begin_end_and_carry_display() {
        let dir = std::env::temp_dir().join(format!("tchat-012-evt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tc = json!({ "choices": [{ "message": {
            "role": "assistant", "content": null,
            "tool_calls": [
                { "id": "call-012-exec-aa", "type": "function",
                  "function": { "name": "exec_command",
                                "arguments": json!({ "cmd": "echo t012" }).to_string() } }
            ]
        }, "finish_reason": "tool_calls" }] });
        let fin = json!({ "choices": [{ "message": { "role": "assistant", "content": "fin" },
            "finish_reason": "stop" }] });
        let mut history = Vec::new();
        let mut events: Vec<TurnNotify> = Vec::new();
        let mut sent = 0usize;
        run_tool_turn(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                Ok(if sent == 1 { tc.clone() } else { fin.clone() })
            },
            &mut |n| events.push(n),
            None,
            &mut |_| {},
        )
        .unwrap();
        // Begin 在前（参数原文），End 在后（结果 JSON＋exec 展示时长）
        let (begin, rest): (Vec<_>, Vec<_>) = events
            .iter()
            .partition(|n| matches!(n, TurnNotify::ToolBegin { .. }));
        assert_eq!(begin.len(), 1, "恰一次 Begin：{events:?}");
        match &begin[0] {
            TurnNotify::ToolBegin {
                id,
                name,
                arguments,
            } => {
                assert_eq!(id, "call-012-exec-aa");
                assert_eq!(name, "exec_command");
                assert_eq!(arguments, &json!({ "cmd": "echo t012" }).to_string());
            }
            other => panic!("Begin 形状：{other:?}"),
        }
        let end = rest
            .iter()
            .find(|n| matches!(n, TurnNotify::ToolEnd { .. }))
            .expect("必有 End");
        match end {
            TurnNotify::ToolEnd {
                id,
                name,
                arguments,
                result,
                display,
                cancelled,
            } => {
                assert_eq!(id, "call-012-exec-aa");
                assert_eq!(name, "exec_command");
                assert_eq!(arguments, &json!({ "cmd": "echo t012" }).to_string());
                let v: Value = serde_json::from_str(result).unwrap();
                assert_eq!(v["status"], json!("completed"), "{v}");
                assert!(v["output"].as_str().unwrap().contains("t012"), "{v}");
                assert!(
                    matches!(display, Some(ToolDisplay::Exec { duration_ms: _ })),
                    "End 携带 exec 展示数据：{display:?}"
                );
                assert!(!cancelled, "真实执行非取消");
            }
            other => panic!("End 形状：{other:?}"),
        }
        // 展示数据随历史记录持久（HistoryItem::ToolResult.display），模型面 content 原样
        match history.iter().find(|h| {
            matches!(
                h,
                HistoryItem::ToolResult { tool_call_id, .. }
                    if tool_call_id == "call-012-exec-aa"
            )
        }) {
            Some(HistoryItem::ToolResult {
                content, display, ..
            }) => {
                let v: Value = serde_json::from_str(content).unwrap();
                assert!(
                    v.get("duration_ms").is_none() && v.get("display").is_none(),
                    "模型面结果不含展示字段：{v}"
                );
                assert!(display.is_some(), "展示数据入史（供 /resume 投影）");
            }
            other => panic!("历史含工具结果：{other:?}"),
        }
        // 行式活动行格式（逐字节＝006 历史输出）
        assert_eq!(
            tool_activity_line("exec_command", "call-012-exec-aa"),
            "[工具] exec_command（call-012）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_unknown_tool_and_bad_json_arguments() {
        let r = execute_tool(
            &ParsedToolCall {
                id: "u1".into(),
                name: "web_search".into(),
                arguments: "{}".into(),
            },
            None,
            &ToolMode::User,
        );
        let v: Value = serde_json::from_str(&r.0).unwrap();
        assert!(
            v["error"].as_str().unwrap().contains("未知工具 web_search"),
            "{v}"
        );
        let r = execute_tool(
            &ParsedToolCall {
                id: "b1".into(),
                name: "exec_command".into(),
                arguments: "not-json{".into(),
            },
            None,
            &ToolMode::User,
        );
        let v: Value = serde_json::from_str(&r.0).unwrap();
        assert_eq!(
            v["status"],
            json!("error"),
            "exec 参数错误按其失败结构回传：{v}"
        );
        let r = execute_tool(
            &ParsedToolCall {
                id: "b2".into(),
                name: "apply_patch".into(),
                arguments: "[".into(),
            },
            None,
            &ToolMode::User,
        );
        let v: Value = serde_json::from_str(&r.0).unwrap();
        assert_eq!(
            v["ok"],
            json!(false),
            "apply_patch 参数错误按其失败结构回传：{v}"
        );
    }

    #[test]
    fn non_string_content_is_protocol_error_and_history_clean() {
        // 输入类别：content 为数组（非字符串非 null）→ 协议错误；历史不污染（仅本轮 user）
        let mut history = Vec::new();
        let bad = json!({ "choices": [{ "message": {
            "role": "assistant", "content": ["分段"], "tool_calls": []
        } }] });
        let err = run_tool_turn(
            &mut history,
            "hi",
            &mut |_| Ok(bad.clone()),
            &mut |_| {},
            None,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.contains("content 必须是字符串或 null"), "{err}");
        assert_eq!(history.len(), 1, "协议错误不得写入 assistant/工具记录");
        assert!(matches!(
            &history[0],
            HistoryItem::Text(HistoryMessage {
                role: HistoryRole::User,
                ..
            })
        ));
    }

    /// N3：单批 ≤32——33/300 整批拒绝（入史/执行前，零副作用、历史不污染）；32 条合法调用按序执行。
    #[test]
    fn n3_batch_size_limit() {
        let dir = std::env::temp_dir().join(format!("tchat-n3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |n: usize| -> Value {
            let calls: Vec<Value> = (0..n)
                .map(|i| {
                    json!({ "id": format!("c{i:03}"), "type": "function",
                    "function": { "name": "exec_command",
                        "arguments": json!({ "cmd": format!("true") }).to_string() } })
                })
                .collect();
            json!({ "choices": [{ "message": { "role": "assistant", "content": null,
                "tool_calls": calls }, "finish_reason": "tool_calls" }] })
        };
        // 33 与 300：整批拒绝、零副作用、历史不污染
        for n in [33usize, 300] {
            let mut history = Vec::new();
            history.push(
                HistoryMessage {
                    role: HistoryRole::User,
                    content: "u".into(),
                }
                .into(),
            );
            let err = run_tool_turn(
                &mut history,
                "go",
                &mut |_| Ok(mk(n)),
                &mut |_| {},
                None,
                &mut |_| {},
            )
            .unwrap_err();
            assert!(err.contains("超过上限 32"), "{n} 条应整批拒绝：{err}");
            assert_eq!(history.len(), 2, "{n}：仅原历史＋本轮 user，无调用记录污染");
        }
        // 32 条恰上限：全部按序执行（副作用计数证明）
        let dir32 = dir.join("32");
        std::fs::create_dir_all(&dir32).unwrap();
        let calls: Vec<Value> = (0..32)
            .map(|i| json!({ "id": format!("c{i:03}"), "type": "function",
                "function": { "name": "exec_command",
                    "arguments": json!({ "cmd": format!("echo x > {}/f{i}", dir32.display()) }).to_string() } }))
            .collect();
        let tc = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
            "tool_calls": calls }, "finish_reason": "tool_calls" }] });
        let fin = json!({ "choices": [{ "message": { "role": "assistant", "content": "done32" },
            "finish_reason": "stop" }] });
        let mut history = Vec::new();
        let mut sent = 0;
        let reply = run_tool_turn(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                if sent == 1 {
                    Ok(tc.clone())
                } else {
                    Ok(fin.clone())
                }
            },
            &mut |_| {},
            None,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(reply, "done32");
        assert_eq!(sent, 2, "调用条数不计入 8 次请求预算");
        let files = std::fs::read_dir(&dir32).unwrap().count();
        assert_eq!(files, 32, "32 条全部真实执行");
        let results = history
            .iter()
            .filter(|h| matches!(h, HistoryItem::ToolResult { .. }))
            .count();
        assert_eq!(results, 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turn_cap_and_batch8_semantics() {
        // 恒返合法调用：恰 8 次请求后上限错误；历史含 8 批 assistant+结果；当前 user 恰 1 条
        let mut history = Vec::new();
        let mut sent = 0;
        let tc = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
            "tool_calls": [{ "id": "c", "type": "function",
            "function": { "name": "exec_command", "arguments": "{\"cmd\":\"true\"}" } }] },
            "finish_reason": "tool_calls" }] });
        let err = run_tool_turn(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                Ok(tc.clone())
            },
            &mut |_| {},
            None,
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(sent, 8, "恰 8 次请求");
        assert!(err.contains("已达本轮模型请求上限（8）"), "{err}");
        assert_eq!(
            history
                .iter()
                .filter(|h| matches!(h, HistoryItem::ToolResult { .. }))
                .count(),
            8,
            "第 8 批结果已保存"
        );
        assert_eq!(
            history
                .iter()
                .filter(|h| matches!(
                    h,
                    HistoryItem::Text(HistoryMessage {
                        role: HistoryRole::User,
                        ..
                    })
                ))
                .count(),
            1,
            "当前 user 只追加一次"
        );
    }

    /// LOOP-B05：单批十条合法短调用——全部按序真实执行（各自落盘一个文件），
    /// 十条结果与 id 一一对应；调用条数与 8 次请求预算互不相干（恰 2 次请求）。
    #[test]
    fn loop_b05_ten_call_batch_serial_with_real_side_effects() {
        use std::path::Path;
        let dir = std::env::temp_dir().join(format!("tchat-loop-b05-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let calls: Vec<Value> = (0..10)
        .map(|i| {
            json!({ "id": format!("call-b05-{i:02}"), "type": "function",
                "function": { "name": "exec_command",
                    "arguments": json!({ "cmd": format!("echo {} > {}/f{i}.txt", i * 7, dir.display()) }).to_string() } })
        })
        .collect();
        let tc = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": calls }, "finish_reason": "tool_calls" }] });
        let mut history = Vec::new();
        let mut sent = 0;
        let reply = run_tool_turn(
            &mut history,
            "十连发",
            &mut |_| {
                sent += 1;
                if sent == 1 {
                    Ok(tc.clone())
                } else {
                    Ok(json!({ "choices": [{ "message":
            { "role": "assistant", "content": "十连完成" }, "finish_reason": "stop" }] }))
                }
            },
            &mut |_| {},
            None,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(reply, "十连完成");
        assert_eq!(sent, 2, "调用条数不计入请求预算：恰 2 次模型请求");
        // 真实副作用：十个文件，内容与调用序号对应
        for i in 0..10 {
            let p = Path::new(&dir).join(format!("f{i}.txt"));
            assert_eq!(
                std::fs::read_to_string(&p).unwrap().trim(),
                format!("{}", i * 7),
                "{p:?}"
            );
        }
        // 十条结果按序与 id 一一对应
        let results: Vec<&HistoryItem> = history
            .iter()
            .filter(|h| matches!(h, HistoryItem::ToolResult { .. }))
            .collect();
        assert_eq!(results.len(), 10);
        for (i, h) in results.iter().enumerate() {
            let HistoryItem::ToolResult {
                tool_call_id,
                content,
                ..
            } = h
            else {
                unreachable!()
            };
            assert_eq!(tool_call_id, &format!("call-b05-{i:02}"), "顺序与 id 对应");
            let v: Value = serde_json::from_str(content).unwrap();
            assert_eq!(v["exit_code"], json!(0), "真实执行成功：{v}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 审查整改（006 §1「原样保留」）：tool_calls 的 id/name 不做归一化——
    /// trim 仅用于"全空白"判断；存储、唯一性判断与分派均使用原值。
    /// 场景 A：id 含前后空白 → assistant 记录、tool_call_id、续请求投影逐字一致；
    /// 场景 C：name " exec_command " → 按未知工具回传单 error 结构，探针不创建（不得归一化后执行）。
    #[test]
    fn tool_call_id_and_name_preserved_verbatim() {
        let probe_dir = std::env::temp_dir().join(format!("tchat-fidelity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&probe_dir);
        std::fs::create_dir_all(&probe_dir).unwrap();
        let probe = probe_dir.join("probe.txt");
        let calls = json!([
            { "id": " id-a ", "type": "function",
              "function": { "name": "exec_command",
                "arguments": json!({ "cmd": format!("echo A > {}", probe.display()) }).to_string() } },
            { "id": "id-b", "type": "function",
              "function": { "name": " exec_command ",
                "arguments": json!({ "cmd": format!("echo C > {}", probe.display()) }).to_string() } },
        ]);
        let tc = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": calls }, "finish_reason": "tool_calls" }] });
        let fin = json!({ "choices": [{ "message": { "role": "assistant", "content": "fin" },
        "finish_reason": "stop" }] });
        let mut history = Vec::new();
        let mut sent = 0;
        let mut second_body = Value::Null;
        let reply = run_tool_turn(
            &mut history,
            "go",
            &mut |body| {
                sent += 1;
                if sent == 1 {
                    Ok(tc.clone())
                } else {
                    second_body = body.clone();
                    Ok(fin.clone())
                }
            },
            &mut |_| {},
            None,
            &mut |_| {},
        )
        .unwrap_or_else(|e| panic!("含空白 id/name 的合法批应正常执行：{e}"));
        assert_eq!(reply, "fin");
        assert_eq!(sent, 2);
        // 场景 A：assistant 记录与 tool 结果的 id 均为原始 " id-a "（含空白）
        let HistoryItem::AssistantToolCalls { tool_calls, .. } = &history[1] else {
            panic!("历史应记录 assistant 工具调用：{history:?}")
        };
        assert_eq!(
            tool_calls[0].id, " id-a ",
            "assistant 记录原始 id（不 trim）"
        );
        let results: Vec<(&String, &String)> = history
            .iter()
            .filter_map(|h| match h {
                HistoryItem::ToolResult {
                    tool_call_id,
                    content,
                    ..
                } => Some((tool_call_id, content)),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2, "每个 id 恰一条结果");
        assert_eq!(results[0].0, " id-a ", "tool_call_id 与原始 id 逐字一致");
        assert_eq!(
            results[0].0, &tool_calls[0].id,
            "结果与 assistant 记录同源原值"
        );
        // 场景 A 的合法调用真实执行：id 含空白不影响分派，探针由该调用创建
        let v0: Value = serde_json::from_str(results[0].1).unwrap();
        assert_eq!(
            v0["status"],
            json!("completed"),
            "id 含空白的 exec 正常执行：{v0}"
        );
        assert_eq!(std::fs::read_to_string(&probe).unwrap().trim(), "A");
        // 场景 C：name 含空白 → 未知工具单 error 结构（名字原样出现在错误中），无第二个副作用
        let vc: Value = serde_json::from_str(results[1].1).unwrap();
        assert_eq!(results[1].0, "id-b");
        assert!(
            vc.as_object().map(|o| o.len()) == Some(1) && vc["error"].is_string(),
            "未知工具回传单 error 字段通用结构：{vc}"
        );
        let cerr = vc["error"].as_str().unwrap();
        assert!(
            cerr.contains("未知工具") && cerr.contains(" exec_command "),
            "错误含原始名字：{cerr}"
        );
        assert_eq!(
            std::fs::read_to_string(&probe).unwrap().trim(),
            "A",
            "name 含空白不得归一化后执行"
        );
        // 续请求原样投影：assistant.tool_calls[0].id 与 tool.tool_call_id 都含空白原值
        let msgs = second_body["messages"].as_array().unwrap();
        let assistant_msg = msgs
            .iter()
            .find(|m| m["role"] == json!("assistant") && m.get("tool_calls").is_some())
            .expect("续请求应含 assistant 工具调用记录");
        assert_eq!(
            assistant_msg["tool_calls"][0]["id"],
            json!(" id-a "),
            "续请求 assistant id 原样"
        );
        let tool_msgs: Vec<&Value> = msgs.iter().filter(|m| m["role"] == json!("tool")).collect();
        assert_eq!(tool_msgs.len(), 2);
        assert_eq!(
            tool_msgs[0]["tool_call_id"],
            json!(" id-a "),
            "续请求 tool_call_id 原样"
        );
        assert_eq!(
            tool_msgs[0]["tool_call_id"], assistant_msg["tool_calls"][0]["id"],
            "tool_call_id 与 assistant id 逐字匹配"
        );
        // 场景 D（行为锁定）：id/name 全空白仍整批拒绝（trim 仅用于空白判断）
        assert!(parse_tool_calls(&msg_with_tool_calls(json!([
            { "id": "   ", "type": "function", "function": { "name": "x", "arguments": "{}" } }
        ])))
        .unwrap_err()
        .contains("缺少非空 id"));
        assert!(parse_tool_calls(&msg_with_tool_calls(json!([
            { "id": "a", "type": "function", "function": { "name": " \t ", "arguments": "{}" } }
        ])))
        .unwrap_err()
        .contains("缺少非空工具名"));
        let _ = std::fs::remove_dir_all(&probe_dir);
    }

    /// 审查整改（006 §1）：批内 id 唯一性按**原始值**判断——
    /// 原始 id 不同、trim 后相同（"dup1" 与 " dup1"）的两项各自合法执行、结果按原值匹配。
    #[test]
    fn tool_call_id_uniqueness_uses_original_values() {
        let dir = std::env::temp_dir().join(format!("tchat-uniq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let calls = json!([
            { "id": "dup1", "type": "function",
              "function": { "name": "exec_command",
                "arguments": json!({ "cmd": format!("echo B0 > {}/f0.txt", dir.display()) }).to_string() } },
            { "id": " dup1", "type": "function",
              "function": { "name": "exec_command",
                "arguments": json!({ "cmd": format!("echo B1 > {}/f1.txt", dir.display()) }).to_string() } },
        ]);
        let tc = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": calls }, "finish_reason": "tool_calls" }] });
        let fin = json!({ "choices": [{ "message": { "role": "assistant", "content": "fin-u" },
        "finish_reason": "stop" }] });
        let mut history = Vec::new();
        let mut sent = 0;
        let reply = run_tool_turn(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                if sent == 1 {
                    Ok(tc.clone())
                } else {
                    Ok(fin.clone())
                }
            },
            &mut |_| {},
            None,
            &mut |_| {},
        )
        .unwrap_or_else(|e| panic!("原始 id 不同、trim 后相同应各自执行，不得判重复：{e}"));
        assert_eq!(reply, "fin-u");
        let results: Vec<(&String, &String)> = history
            .iter()
            .filter_map(|h| match h {
                HistoryItem::ToolResult {
                    tool_call_id,
                    content,
                    ..
                } => Some((tool_call_id, content)),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2, "两条各自一条结果");
        assert_eq!(results[0].0, "dup1", "第一条按原始 id 匹配");
        assert_eq!(results[1].0, " dup1", "第二条按原始 id（含空白）匹配");
        for (r, (id, content)) in results.iter().enumerate() {
            let v: Value = serde_json::from_str(content).unwrap();
            assert_eq!(v["status"], json!("completed"), "id={id} 真实执行：{v}");
            assert_eq!(v["exit_code"], json!(0));
            assert_eq!(
                std::fs::read_to_string(dir.join(format!("f{r}.txt")))
                    .unwrap()
                    .trim(),
                format!("B{r}"),
                "副作用与调用一一对应"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 审查整改（取消分形）：取消后未执行调用按工具返回对应结果结构——
    /// exec/apply_patch 用各自结构体形态；**未知工具用单 error 通用结构，不伪装 exec**。
    /// 未执行调用无副作用、每个 id 恰一条结果、无自动续请求；下一用户轮完整历史并正常运行。
    #[test]
    fn cancelled_batch_skips_per_tool_shapes_and_unknown_generic_error() {
        let dir = std::env::temp_dir().join(format!("tchat-cancel-shape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let patch_a = format!(
            "*** Begin Patch\n*** Add File: {}/a.txt\n+seed\n*** End Patch",
            dir.display()
        );
        let patch_b = format!(
            "*** Begin Patch\n*** Add File: {}/b.txt\n+never\n*** End Patch",
            dir.display()
        );
        let calls = json!([
            { "id": "p1", "type": "function",
              "function": { "name": "apply_patch", "arguments": json!({ "patch": patch_a }).to_string() } },
            { "id": "e2", "type": "function",
              "function": { "name": "exec_command",
                "arguments": json!({ "cmd": format!("echo NEVER > {}/skipped.txt", dir.display()) }).to_string() } },
            { "id": "a3", "type": "function",
              "function": { "name": "apply_patch", "arguments": json!({ "patch": patch_b }).to_string() } },
            { "id": "u4", "type": "function",
              "function": { "name": "web_search", "arguments": "{}" } },
        ]);
        let tc = json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": calls }, "finish_reason": "tool_calls" }] });
        let fin = json!({ "choices": [{ "message": { "role": "assistant", "content": "fin2" },
        "finish_reason": "stop" }] });
        let mut history = Vec::new();
        let mut sent = 0;
        let mut notified = 0usize;
        let err = run_tool_turn(
            &mut history,
            "go",
            &mut |_| {
                sent += 1;
                Ok(tc.clone())
            },
            &mut |n: TurnNotify| {
                // 012：活动事件结构化——以 ToolBegin（执行开始信号）计数：第一条调用真实
                // 执行，其余三条不开始（确定性，无计时）。ToolEnd 不计入（取消批也发终态）。
                if let TurnNotify::ToolBegin { .. } = n {
                    notified += 1;
                    cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            },
            Some(&cancel),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.contains("用户已取消"), "{err}");
        assert_eq!(sent, 1, "取消即结束本轮，无自动续请求");
        assert_eq!(notified, 1, "仅第一条真实执行");
        // 第一条真实生效；其余三条无副作用
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "seed\n"
        );
        assert!(
            !dir.join("skipped.txt").exists(),
            "未执行的 exec 不得有副作用"
        );
        assert!(
            !dir.join("b.txt").exists(),
            "未执行的 apply_patch 不得有副作用"
        );
        // 四个 id 各恰一条结果，形状按工具分形
        let results: Vec<(&String, &String)> = history
            .iter()
            .filter_map(|h| match h {
                HistoryItem::ToolResult {
                    tool_call_id,
                    content,
                    ..
                } => Some((tool_call_id, content)),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 4, "每个已记录调用都有匹配结果");
        assert_eq!(results[0].0, "p1");
        assert_eq!(results[1].0, "e2");
        assert_eq!(results[2].0, "a3");
        assert_eq!(results[3].0, "u4");
        let v1: Value = serde_json::from_str(results[1].1).unwrap();
        assert_eq!(
            v1,
            json!({ "status": "cancelled", "exit_code": null, "output": "", "truncated": false,
                "error": "用户取消，本调用未执行" }),
            "exec 未执行＝cancelled 形态：{v1}"
        );
        let v2: Value = serde_json::from_str(results[2].1).unwrap();
        assert_eq!(
            v2,
            json!({ "ok": false, "changed_files": [], "state_known": true,
                "error": "用户取消，本调用未执行" }),
            "apply_patch 未执行＝ok=false＋state_known=true：{v2}"
        );
        let v3: Value = serde_json::from_str(results[3].1).unwrap();
        assert_eq!(
            v3,
            json!({ "error": "用户取消，本调用未执行" }),
            "未知工具未执行＝单 error 通用结构，不得伪装 exec 结果：{v3}"
        );
        // 下一用户轮：取消状态复位、完整历史携带、正常运行
        let mut sent2 = 0;
        let mut body2 = Value::Null;
        let reply = run_tool_turn(
            &mut history,
            "next",
            &mut |b| {
                sent2 += 1;
                body2 = b.clone();
                Ok(fin.clone())
            },
            &mut |_| {},
            Some(&cancel),
            &mut |_| {},
        )
        .unwrap_or_else(|e| panic!("取消后下一轮应正常运行：{e}"));
        assert_eq!(reply, "fin2");
        assert_eq!(sent2, 1);
        let msgs2 = body2["messages"].as_array().unwrap();
        let tool_msgs2: Vec<&Value> = msgs2
            .iter()
            .filter(|m| m["role"] == json!("tool"))
            .collect();
        let ids2: Vec<&str> = tool_msgs2
            .iter()
            .filter_map(|m| m["tool_call_id"].as_str())
            .collect();
        assert_eq!(ids2, vec!["p1", "e2", "a3", "u4"], "续请求完整携带四条结果");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
