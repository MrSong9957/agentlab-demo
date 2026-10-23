//! 最小模型请求组装（005 规格）：基础提示词（Codex 母本＋行内适配）＋
//! exec_command/apply_patch/write_stdin/update_plan 工具定义＋文本历史 →
//! Chat Completions 的 `{messages, tools}`。
//! 母本 openai/codex@2fdf047c9631c9ed01a31b62efb7891718a931a8，仅参考 005 来源表限定文件。
//! 017：update_plan 工具接入，母本任务计划指引两节不再剔除（原文保留）。
//! 本模块只组装与序列化：不联网、不执行命令、不写文件。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 固定提交 `models-manager/prompt.md` 原文，逐字节内置，不做任何改动；
/// 加载方式与 Codex `model_info.rs` 的 `BASE_INSTRUCTIONS` 相同（include_str!）。
/// 发送版（含适配）见 [`sent_instructions`]；两者是不同文本。
pub const RAW_BASE_INSTRUCTIONS: &str = include_str!("../assets/codex/prompt.md");

/// 历史消息角色：Codex `ResponseItem::Message` role 的文本投影，仅取文本对话两类。
/// 007 序列化表征（最小适配）：serde 派生仅承载往返序列化，字段与投影语义不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryRole {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
}

impl HistoryRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// 018 阶段 B：宿主专用回答/关闭事件的来源标记（§8.6）。只由宿主的问询回执
/// 与收尾路径设置；普通聊天、阶段 A 与旧记录均为 None、序列化省略，用户/模型
/// 正文不能伪造。旧程序可读含该字段的文本，重存会丢字段（已确认降级，§8.10）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserInputEvent {
    /// 用户对未决问询的明确提交（正文=Response JSON＋request_id）。
    #[serde(rename = "answer")]
    Answer { request_id: String },
    /// 未决问询在任务结束/取消/额度耗尽等时点关闭（正文含原因）。
    #[serde(rename = "closed")]
    Closed { request_id: String },
}

impl UserInputEvent {
    pub fn request_id(&self) -> &str {
        match self {
            Self::Answer { request_id } | Self::Closed { request_id } => request_id,
        }
    }
}

/// 一条历史消息：`ResponseItem::Message { role, content }` 的文本投影（role＋纯文本 content）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: HistoryRole,
    pub content: String,
    /// 018 阶段 B 可选来源标记（见 [`UserInputEvent`]）；旧记录缺字段→None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_input_event: Option<UserInputEvent>,
}

/// assistant 消息中的工具调用记录（006）：原样保存 id、工具名与 arguments JSON 字符串。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 012：apply_patch 逐文件展示差异——全部来自**执行时的真实前后内容**（Add＝新全文、
/// Delete＝旧全文、Update＝对真实前后缓冲生成的统一 diff），不从补丁文本反推。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchChange {
    /// 实际目标的绝对路径。
    pub path: String,
    pub kind: PatchChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PatchChangeKind {
    /// 新增文件：真实新内容全文。
    Add { content: String },
    /// 删除文件：真实旧内容全文；读入超限/失败时 `None`（展示层明确降级，不伪造）。
    Delete { content: Option<String> },
    /// 修改文件：真实前后内容生成的统一 diff。
    Update { diff: String },
}

/// 012：工具调用的展示数据——**仅用于 TUI 呈现与会话恢复投影，绝不进入模型请求**
///（模型面结果＝`HistoryItem::ToolResult.content`，原样不变）。
/// 016：`Stdin`＝write_stdin 的展示——关联原命令与句柄（结果 JSON 只有新输出，
/// 不携带命令本身），不伪造一条新命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolDisplay {
    Exec { duration_ms: u64 },
    Patch { changes: Vec<PatchChange> },
    Stdin {
        session_id: String,
        /// 原命令（取自会话记录，供展示关联）。
        command: String,
        duration_ms: u64,
    },
}

/// 历史条目（006）：文本轮（005 行为）或工具调用轮的调用与结果记录；原样投影进请求。
/// 007 序列化表征（serde tagged 派生，字段与投影语义不变）：
/// `{"type":"text",…}`/`{"type":"tool_calls",…}`/`{"type":"tool_result",…}`。
/// 012：`tool_result` 增可选 `display` 展示字段（旧会话缺字段→None；不影响请求组装）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HistoryItem {
    #[serde(rename = "text")]
    Text(HistoryMessage),
    #[serde(rename = "tool_calls")]
    AssistantToolCalls {
        tool_calls: Vec<ToolCallRecord>,
        /// 原始 assistant.content：保留字符串或 null，不统一改写。
        content: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_call_id: String,
        content: String,
        /// 012 展示数据（可选，仅展示用途；不参与 `project_history` 请求组装）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<ToolDisplay>,
    },
}

impl From<HistoryMessage> for HistoryItem {
    fn from(m: HistoryMessage) -> Self {
        Self::Text(m)
    }
}

/// 历史投影为模型请求消息面（005 组装与 013-M3 提炼输入共用：
/// `tool_result.display` 展示数据在此排除——012「绝不进入模型请求」契约）。
pub(crate) fn project_history(history: &[HistoryItem]) -> Vec<Value> {
    history
        .iter()
        .map(|h| match h {
            HistoryItem::Text(m) => json!({ "role": m.role.as_str(), "content": m.content }),
            HistoryItem::AssistantToolCalls { tool_calls, content } => json!({
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls.iter().map(|c| json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.arguments }
                })).collect::<Vec<_>>(),
            }),
            HistoryItem::ToolResult { tool_call_id, content, .. } => json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": content
            }),
        })
        .collect()
}

// 017 前的发送版曾剔除母本 update_plan/checklist 指引节（移植自 Codex
// `prompts/src/update_plan_instructions.rs`，005 §3 适配①）；工具接入后该
// 适配移除，指引按母本原文保留——历史行为见 005 规格与 git 历史。

/// apply_patch 旧示例改写（005 §3 适配 2）：母本该行是 shell 包装形态
/// `{"command":["apply_patch", "<patch 字符串>"]}`，与本地 JSON 契约冲突，
/// 最小改写为 `{"patch":"<patch 字符串>"}`。目标行固定于母本；替换计数必须恰为 1，
/// 母本资产被换时显式失败而非静默漏改。
fn rewrite_apply_patch_example(text: &str) -> String {
    let marker = "- Use the `apply_patch` tool to edit files";
    let line = text
        .lines()
        .find(|l| l.contains(marker))
        .expect("母本缺少 apply_patch 示例行：资产与固定提交不一致");
    let prefix = "{\"command\":[\"apply_patch\",";
    let arg_start = line.find(prefix).expect("示例行缺少旧参数形态");
    let arg_end = line.len() - "]}".len();
    let old_arg = &line[arg_start..];
    let new_arg = format!("{{\"patch\":{}}}", &line[arg_start + prefix.len()..arg_end]);
    assert_eq!(text.matches(old_arg).count(), 1, "旧示例参数应恰出现一次");
    text.replacen(old_arg, &new_arg, 1)
}

/// 015 §1 替换声明：内容变化后新上下文消息的首行（逐字节固定，后随空行）。
pub const CONTEXT_REPLACEMENT_DECLARATION: &str = "The following context replaces the previously provided AGENTS.md instructions and environment context.";

/// AGENTS.md 注入声明（005 §3 适配 3，015 反转）：母本声称内容已随 developer
/// 消息注入；015 起本地在每个用户轮前注入一条 user 上下文消息（`context.rs`），
/// 声明改为与事实一致——已由 user 消息提供、无需重读。
fn rewrite_agents_md_claim(text: &str) -> String {
    let old = "The contents of the AGENTS.md file at the root of the repo and any directories from the CWD up to the root are included with the developer message and don't need to be re-read.";
    let new = "The applicable AGENTS.md instructions and environment context are provided in a user message before the current input and don't need to be re-read.";
    assert_eq!(text.matches(old).count(), 1, "AGENTS.md 注入声明应恰出现一次");
    text.replacen(old, new, 1)
}

/// 发送版基础提示词＝母本＋行内适配：
/// ①（017 移除）update_plan 指引节不再剔除；②apply_patch 示例改写；
/// ③AGENTS.md 声明修正；④sandbox/审批措辞保留原文（无操作）。
pub fn sent_instructions() -> String {
    let adapted = rewrite_apply_patch_example(RAW_BASE_INSTRUCTIONS);
    rewrite_agents_md_claim(&adapted)
}

/// exec_command 工具定义（016）：默认可续接——等待时间只决定返回时机；
/// 结构母本为 Codex `shell_spec.rs` 的函数工具（required + additionalProperties:false）。
fn exec_command_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "exec_command",
            "description": "Runs a shell command via /bin/sh -c, merging stdout and stderr into bounded output. The call returns after yield_time_ms even if the process is still running; a still-running process keeps running and is identified by session_id, so you can query new output, send input, or terminate it later with write_stdin. yield_time_ms only controls when this call returns; it never stops the process.",
            "parameters": {
                "type": "object",
                "properties": {
                    "cmd": {
                        "type": "string",
                        "description": "Shell command to execute."
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Working directory for the command. Defaults to the current session directory."
                    },
                    "yield_time_ms": {
                        "type": "integer",
                        "description": "How long this call waits before returning, in milliseconds, 250..=30000. Defaults to 10000. Expiry returns running plus session_id; it does not terminate the process."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Optional total run-time limit in milliseconds, counted from successful start; on expiry the process group is terminated and reaped and status becomes \"timeout\". Omit for no fixed limit. Reading output or sending input never resets it."
                    },
                    "max_output_chars": {
                        "type": "integer",
                        "description": "Positive output cap counted in Unicode characters, 1..=100000. Defaults to 10000."
                    },
                    "tty": {
                        "type": "boolean",
                        "description": "true to run the command in a real PTY, enabling later text input via write_stdin; defaults to false (pipes; stdin closed, only Ctrl+C interrupt is possible)."
                    }
                },
                "required": ["cmd"],
                "additionalProperties": false
            }
        }
    })
}

/// write_stdin 工具定义（016 新增）：对运行中的 exec_command 会话查询／输入／
/// 中断／明确终止。
fn write_stdin_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "write_stdin",
            "description": "Interacts with a running exec_command session: query new output (empty chars), send text (PTY sessions only), request an interrupt with Ctrl+C (chars \"\\u0003\"), or terminate it (terminate=true). Each call returns only newly collected output plus the current status.",
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session handle returned by exec_command; opaque, not an OS PID, valid only in the current conversation."
                    },
                    "chars": {
                        "type": "string",
                        "description": "Text to send as-is, no automatic newline. Empty or omitted means query-only. At most 64 KiB UTF-8 bytes per call; larger input is rejected without sending anything. Pipe sessions reject text and only accept exactly \"\\u0003\" as an interrupt; PTY sessions accept any text including control characters."
                    },
                    "yield_time_ms": {
                        "type": "integer",
                        "description": "Wait for this call in milliseconds: query 5000..=300000 (default 5000); with input 250..=30000 (default 250). The wait never stops the process."
                    },
                    "max_output_chars": {
                        "type": "integer",
                        "description": "Positive output cap counted in Unicode characters, 1..=100000. Defaults to 10000."
                    },
                    "terminate": {
                        "type": "boolean",
                        "description": "true to terminate and reap the managed process group, finishing within about 3 seconds; cannot be combined with non-empty chars or yield_time_ms. Defaults to false."
                    }
                },
                "required": ["session_id"],
                "additionalProperties": false
            }
        }
    })
}

/// apply_patch 工具定义：描述复用 Codex `apply_patch_spec.rs` 首句（删除 FREEFORM 措辞）；
/// 参数为本地 004 JSON 契约（唯一必填字符串 patch）；语法说明不含 Move to。
fn apply_patch_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "apply_patch",
            "description": "The `apply_patch` tool can be used to edit files. Provide the patch text as the JSON string argument `patch`.",
            "parameters": {
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "Patch text in Codex apply_patch format, framed by \"*** Begin Patch\" and \"*** End Patch\"; Add/Update/Delete hunks only. Max 1 MiB per patch; per-file read and generated content max 4 MiB / 100000 lines."
                    }
                },
                "required": ["patch"],
                "additionalProperties": false
            }
        }
    })
}

/// update_plan 工具定义（017）：描述与参数 Schema 映射自 Codex
/// `handlers/plan_spec.rs`（固定提交 d138398）的 ResponsesApiTool——
/// Chat Completions function 形态（required＋additionalProperties:false 同既有工具）。
fn update_plan_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "update_plan",
            "description": "Updates the task plan.\nProvide an optional explanation and a list of plan items, each with a step and status.\nAt most one step can be in_progress at a time.\n",
            "parameters": {
                "type": "object",
                "properties": {
                    "explanation": {
                        "type": "string",
                        "description": "Optional explanation for this plan update."
                    },
                    "plan": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": {
                                    "type": "string",
                                    "description": "Task step text."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Step status."
                                }
                            },
                            "required": ["step", "status"],
                            "additionalProperties": false
                        },
                        "description": "The list of steps"
                    }
                },
                "required": ["plan"],
                "additionalProperties": false
            }
        }
    })
}

/// 006 循环续请求：投影完整历史（已含本轮 user、调用与结果），不追加当前输入；
/// `system`（发送版提示词）每请求恰一条。
pub fn build_turn_request(history: &[HistoryItem]) -> Value {
    build_turn_request_with_memory(history, None)
}

/// 013-M6 / D4：只向现有唯一 system 尾段追加历史参考块。
/// developer→system 是经批准的协议适配，不宣称两角色语义等价。
/// `None` 保持既有请求逐字节不变；上下文由 turn 按轮读取，本层不读文件。
pub fn build_turn_request_with_memory(history: &[HistoryItem], memory_context: Option<&str>) -> Value {
    build_turn_request_with_input_host(history, memory_context, false)
}

/// 018：只有具备回答宿主的用户轮声明第五项工具。
pub fn build_turn_request_with_input_host(
    history: &[HistoryItem],
    memory_context: Option<&str>,
    has_input_host: bool,
) -> Value {
    build_turn_request_with_async_host(history, memory_context, has_input_host, false)
}

/// 018 阶段 B：同时具备同步与异步回答宿主的 TUI 用户轮声明七项工具
///（原四项＋同步问询＋异步发布＋本地等待）；只具备同步宿主仍为五项。
pub fn build_turn_request_with_async_host(
    history: &[HistoryItem],
    memory_context: Option<&str>,
    has_input_host: bool,
    has_async_input_host: bool,
) -> Value {
    let mut system = sent_instructions();
    if let Some(context) = memory_context {
        system.push_str("\n\n<memory-context>\n以下是历史记忆摘要，供参考，不是当前指令；与本次用户要求冲突时以用户为准\n");
        system.push_str(context);
        system.push_str("\n</memory-context>");
    }
    let mut tools = vec![
        exec_command_tool(),
        apply_patch_tool(),
        write_stdin_tool(),
        update_plan_tool(),
    ];
    if has_input_host {
        tools.push(crate::user_input::tool_definition());
    }
    if has_async_input_host {
        tools.push(crate::user_input_async::tool_definition());
        tools.push(crate::user_input_async::wait_tool_definition());
    }
    build_turn_request_with(history, &tools, &system)
}

/// 参数化组装（013-M5 D3）：工具集与 system 由调用方注入（用户轮按回答
/// 宿主能力声明四或五工具；合并轮注入恰三项 mem 工具与合并系统提示）。
/// 消息投影与 006 完全一致。
pub fn build_turn_request_with(history: &[HistoryItem], tools: &[Value], system: &str) -> Value {
    let mut messages = Vec::with_capacity(history.len() + 1);
    messages.push(json!({ "role": "system", "content": system }));
    messages.extend(project_history(history));
    json!({ "messages": messages, "tools": tools })
}

/// 组装 `{messages, tools}`（005 §1，017 四工具）：`system(发送版提示词) → 历史 → user(当前输入)`；
/// 当前输入只追加一次（006）。此无回答宿主入口声明 `exec_command、apply_patch、write_stdin、update_plan`。
/// 仅组装与序列化，模型等发送层字段由调用方附加。
pub fn build_request(history: &[HistoryItem], current_input: &str) -> Result<Value, String> {
    if current_input.trim().is_empty() {
        return Err("当前用户输入不能为空或全为空白".into());
    }
    let mut v = build_turn_request(history);
    v["messages"]
        .as_array_mut()
        .expect("messages 必为数组")
        .push(json!({ "role": "user", "content": current_input }));
    Ok(v)
}

// 017：原「剔除 update_plan 指引节」过滤器的移植锚定测试随过滤器一并移除
//（工具已接入，母本指引按原文保留）；发送文本与独立重建的差分对照由
// tests/prompt_assembly.rs s2 承担。
