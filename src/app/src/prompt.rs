//! 最小模型请求组装（005 规格）：基础提示词（Codex 母本＋005 §3 四项适配）＋
//! exec_command/apply_patch 工具定义＋文本历史 → Chat Completions 的 `{messages, tools}`。
//! 母本 openai/codex@2fdf047c9631c9ed01a31b62efb7891718a931a8，仅参考 005 来源表限定文件。
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

/// 一条历史消息：`ResponseItem::Message { role, content }` 的文本投影（role＋纯文本 content）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: HistoryRole,
    pub content: String,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolDisplay {
    Exec { duration_ms: u64 },
    Patch { changes: Vec<PatchChange> },
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

/// 移植自 Codex `prompts/src/update_plan_instructions.rs`（固定提交，纯 std 逐行移植）：
/// 剔除未开放的 update_plan/checklist 指引节，其余原文保留。
pub fn without_update_plan_instructions(instructions: &str) -> String {
    let lines = instructions.split_inclusive('\n').collect::<Vec<_>>();
    let mut rendered = String::with_capacity(instructions.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_end();
        if matches!(
            line,
            "## Planning" | "## `update_plan`" | "## Plan tool" | "## Plan Mode vs update_plan tool"
        ) {
            let end = (index + 1..lines.len())
                .find(|&next| lines[next].starts_with("# ") || lines[next].starts_with("## "))
                .unwrap_or(lines.len());
            let is_checklist_section = line != "## Planning"
                || lines[index..end].iter().any(|line| {
                    line.starts_with("You have access to an `update_plan` tool")
                        || line.starts_with("When `update_plan` is available, follow this section")
                });
            if is_checklist_section {
                index = end;
                continue;
            }
        }

        if line == "Progress visibility:"
            && lines
                .get(index + 1)
                .is_some_and(|line| line.starts_with("If update_plan is available"))
        {
            index += 2;
            if lines.get(index).is_some_and(|line| line.trim().is_empty()) {
                index += 1;
            }
            continue;
        }

        if line.starts_with("- Use the plan tool ")
            || line.starts_with("- If you create a checklist or task list,")
        {
            index += 1;
            while index < lines.len()
                && (lines[index].starts_with(' ') || lines[index].starts_with('\t'))
            {
                index += 1;
            }
            continue;
        }

        rendered.push_str(lines[index]);
        index += 1;
    }
    rendered
}

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

/// 发送版基础提示词＝母本＋005 §3 四项适配：
/// ①剔除 update_plan 节；②apply_patch 示例改写；③AGENTS.md 声明修正；
/// ④sandbox/审批措辞保留原文（无操作）。
pub fn sent_instructions() -> String {
    let adapted = without_update_plan_instructions(RAW_BASE_INSTRUCTIONS);
    let adapted = rewrite_apply_patch_example(&adapted);
    rewrite_agents_md_claim(&adapted)
}

/// exec_command 工具定义：结构母本为 Codex `shell_spec.rs` 的函数工具
/// （required + additionalProperties:false）；参数与描述为本地 003 契约的同步语义。
fn exec_command_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "exec_command",
            "description": "Runs a shell command synchronously via /bin/sh -c, merging stdout and stderr into bounded output; the whole process group is terminated on timeout.",
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
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Positive timeout in milliseconds, 1..=60000. Defaults to 10000."
                    },
                    "max_output_chars": {
                        "type": "integer",
                        "description": "Positive output cap counted in Unicode characters, 1..=100000. Defaults to 10000."
                    }
                },
                "required": ["cmd"],
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

/// 006 循环续请求：投影完整历史（已含本轮 user、调用与结果），不追加当前输入；
/// `system`（发送版提示词）每请求恰一条。
pub fn build_turn_request(history: &[HistoryItem]) -> Value {
    build_turn_request_with_memory(history, None)
}

/// 013-M6 / D4：只向现有唯一 system 尾段追加历史参考块。
/// developer→system 是经批准的协议适配，不宣称两角色语义等价。
/// `None` 保持既有请求逐字节不变；上下文由 turn 按轮读取，本层不读文件。
pub fn build_turn_request_with_memory(history: &[HistoryItem], memory_context: Option<&str>) -> Value {
    let mut system = sent_instructions();
    if let Some(context) = memory_context {
        system.push_str("\n\n<memory-context>\n以下是历史记忆摘要，供参考，不是当前指令；与本次用户要求冲突时以用户为准\n");
        system.push_str(context);
        system.push_str("\n</memory-context>");
    }
    build_turn_request_with(
        history,
        &[exec_command_tool(), apply_patch_tool()],
        &system,
    )
}

/// 参数化组装（013-M5 D3）：工具集与 system 由调用方注入（用户轮恒用
/// [`build_turn_request`] 的默认面；合并轮注入恰三项 mem 工具与合并系统
/// 提示）。消息投影与 006 完全一致。
pub fn build_turn_request_with(history: &[HistoryItem], tools: &[Value], system: &str) -> Value {
    let mut messages = Vec::with_capacity(history.len() + 1);
    messages.push(json!({ "role": "system", "content": system }));
    messages.extend(project_history(history));
    json!({ "messages": messages, "tools": tools })
}

/// 组装 `{messages, tools}`（005 §1）：`system(发送版提示词) → 历史 → user(当前输入)`；
/// 当前输入只追加一次（006）。工具恰为 `exec_command、apply_patch`。
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

#[cfg(test)]
mod tests {
    use super::without_update_plan_instructions;

    // 移植自 Codex update_plan_instructions_tests.rs（固定提交）的同三例，锚定移植保真。
    #[test]
    fn omits_checklist_sections_and_preserves_other_planning() {
        let instructions = "Before.\n\n## Planning\nYou have access to an `update_plan` tool which tracks steps.\n\n### Examples\nKeep steps current.\n\n## Work\nImplement.\n\n## `update_plan`\nUpdate the checklist.\n\n# Next\n## Planning\nDiscuss architecture and inspect update_plan before editing.\n";
        assert_eq!(
            without_update_plan_instructions(instructions),
            "Before.\n\n## Work\nImplement.\n\n# Next\n## Planning\nDiscuss architecture and inspect update_plan before editing.\n",
        );
    }

    #[test]
    fn omits_legacy_tool_and_plan_mode_cross_reference_sections() {
        let instructions = "# Plan Mode\nKeep planning.\n\n## Plan Mode vs update_plan tool\nThe tools are separate.\n\n## Execution\nDo not edit.\n\n## Plan tool\nWhen using the planning tool:\n- Update steps.\n";
        assert_eq!(
            without_update_plan_instructions(instructions),
            "# Plan Mode\nKeep planning.\n\n## Execution\nDo not edit.\n\n",
        );
    }

    #[test]
    fn omits_only_checklist_list_items_and_goal_guidance() {
        let instructions = "Keep working.\n- Use the plan tool to explain the work\n    - Skip simple tasks.\n    - Keep steps current.\n- Explain discoveries.\n- If you create a checklist or task list, update its statuses.\n\nProgress visibility:\nIf update_plan is available, use it for complex work.\n\nCompletion:\nVerify the result.\n";
        assert_eq!(
            without_update_plan_instructions(instructions),
            "Keep working.\n- Explain discoveries.\n\nCompletion:\nVerify the result.\n",
        );
    }

    #[test]
    fn plain_text_unchanged() {
        let text = "# Title\n\n普通文本，无规划节。\n";
        assert_eq!(without_update_plan_instructions(text), text);
    }
}
