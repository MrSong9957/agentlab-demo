//! 035：多 Agent——根 Agent 把明确任务交给独立上下文的子 Agent 并行处理。
//! 六个工具是同一生命周期的控制面：`spawn_agent`/`send_message`/`followup_task`
//! /`wait_agent`/`interrupt_agent`/`list_agents`（上游 codex `d138398` 的 V2
//! 名称；不搬 send_input/resume_agent/close_agent）。
//!
//! 架构（规格 §1.4/§1.7「本地调度」）：每个根逻辑用户轮一个 group；子轮在
//! 独立线程持有独立上下文运行（复用普通函数编码与真实业务分派）；事件经
//! 有界通道交根调度泵线程顺序落盘（V3 `HistoryItem::AgentEvent`）；全组只有
//! 根 AutoSession 一个追加写者（共享 `Arc<Mutex<Session>>`，事件行先行落盘、
//! 由 Session 的 untracked 计数在 flush 时吸收——034 同机制）。
//!
//! 分层：[`args`]（六工具参数严格校验）、[`registry`]（注册表/状态机/预算/
//! 路径解析）、[`mailbox`]（每 Agent FIFO 邮箱与容量）、[`events`]（V3 事件
//! 构造与链校验）、[`runtime`]（宿主侧：六工具执行、子轮循环、调度泵）。

pub mod args;
pub mod events;
pub mod mailbox;
pub mod registry;
pub mod runtime;

/// 六工具名（模型目录 `experimental_supported_tools` 须同时显式含全部六个
/// 精确名称才整组开放；只配部分时整组关闭并提示一次，§1.2）。
pub const SPAWN_TOOL_NAME: &str = "spawn_agent";
pub const SEND_TOOL_NAME: &str = "send_message";
pub const FOLLOWUP_TOOL_NAME: &str = "followup_task";
pub const WAIT_TOOL_NAME: &str = "wait_agent";
pub const INTERRUPT_TOOL_NAME: &str = "interrupt_agent";
pub const LIST_TOOL_NAME: &str = "list_agents";

/// 全部六工具名（顺序即声明顺序）。
pub const TOOL_NAMES: [&str; 6] = [
    SPAWN_TOOL_NAME,
    SEND_TOOL_NAME,
    FOLLOWUP_TOOL_NAME,
    WAIT_TOOL_NAME,
    INTERRUPT_TOOL_NAME,
    LIST_TOOL_NAME,
];

/// 根路径（唯一根；子 Agent 路径为 `/root/<name>`，只支持一层）。
pub const ROOT_PATH: &str = "/root";

/// 单条任务/消息上限（UTF-8 字节，§1.3）。
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// 原始 arguments 上限（进入参数解析前的字节预算）。
pub const MAX_ARGUMENTS_BYTES: usize = 256 * 1024;

/// 子执行槽（starting/running/stopping 均计；全应用计数，§1.6）。
pub const MAX_CHILD_SLOTS: usize = 3;
/// 每组最多 Agent 记录数。
pub const MAX_AGENTS_PER_GROUP: usize = 8;
/// 每组最多 assignment 启动数（初始任务＋空闲续派）。
pub const MAX_ASSIGNMENTS_PER_GROUP: usize = 8;
/// 每个子 assignment 模型请求上限。
pub const MAX_CHILD_REQUESTS_PER_ASSIGNMENT: usize = 8;
/// 整组子模型请求累计上限（根 8 次独立预留，合计 32，§1.6）。
pub const MAX_CHILD_REQUESTS_PER_GROUP: usize = 24;
/// 每 assignment 子工具调用上限（含 send_message 及失败调用）。
pub const MAX_CHILD_CALLS_PER_ASSIGNMENT: usize = 64;
/// 整组子工具调用上限。
pub const MAX_CHILD_CALLS_PER_GROUP: usize = 256;
/// 每接收者未交付消息条数上限。
pub const MAX_PENDING_MESSAGES: usize = 32;
/// 每接收者未交付消息序列化字节上限。
pub const MAX_PENDING_MESSAGE_BYTES: usize = 256 * 1024;
/// 每组累计消息（初始任务/续派/Agent 消息）条数/字节上限。
pub const MAX_GROUP_MESSAGES: usize = 128;
pub const MAX_GROUP_MESSAGE_BYTES: usize = 1024 * 1024;
/// 终态报告槽（每 assignment 预留一个；最多 8 份）。
pub const MAX_TERMINAL_REPORTS: usize = 8;
/// 单份终态报告正文上限。
pub const MAX_REPORT_BYTES: usize = 64 * 1024;
/// 组持久化收尾额度（含终态报告；另留组关闭 64 KiB；均计入 64 MiB 总额）。
pub const GROUP_CLOSEDOWN_BUDGET: usize = 256 * 1024;
pub const GROUP_CLOSE_BUDGET: usize = 64 * 1024;
/// 每 assignment 墙钟上限（受理起，单调时钟，§1.6）。
pub const ASSIGNMENT_WALL_LIMIT_SECS: u64 = 300;
/// 单个子事件完整序列化上限。
pub const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;
/// 整组新增子事件负载累计上限。
pub const MAX_GROUP_EVENT_BYTES: u64 = 64 * 1024 * 1024;
/// 子模型请求响应预留（发送前预留，提交后按真实大小计费，§1.6）。
pub const RESPONSE_RESERVE_BYTES: u64 = 10 * 1024 * 1024;
/// 子工具结果记录预留。
pub const RESULT_RESERVE_BYTES: u64 = 2 * 1024 * 1024;
/// 子事件通道容量（有界；背压由发送方阻塞形成）。
pub const EVENT_CHANNEL_CAPACITY: usize = 256;
/// 每 assignment 终态摘要里保留的收尾说明上限。
pub const MAX_CLEANUP_NOTE_BYTES: usize = 2048;

/// 统一结果：成功 `{ok:true,data,error:null}`；失败 `{ok:false,data:null,
/// error:{code,message}}`（§1.3）。
pub fn ok_result(data: serde_json::Value) -> String {
    serde_json::json!({ "ok": true, "data": data, "error": null }).to_string()
}

pub fn err_result(code: &str, message: impl Into<String>) -> String {
    serde_json::json!({
        "ok": false,
        "data": null,
        "error": { "code": code, "message": message.into() }
    })
    .to_string()
}

/// spawn_agent 工具定义（§1.3 接口；上游 V2 语义的本地声明面）。
pub fn spawn_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": SPAWN_TOOL_NAME,
            "description": "Starts a sub-agent with an independent context that works on the given task in parallel (at most 3 run concurrently per app). The sub-agent shares the current working directory and file system; it cannot spawn further agents or message other sub-agents, and reports back to /root only. Acceptance is not completion: use wait_agent to observe updates before relying on results. If the task edits files, name the responsible files in the message so concurrent work does not overwrite others.",
            "parameters": {
                "type": "object",
                "properties": {
                    "task_name": {
                        "type": "string",
                        "description": "Short name matching [a-z][a-z0-9_]{0,31}; unique among live agents of the current turn; the agent lives at /root/<task_name>."
                    },
                    "message": {
                        "type": "string",
                        "description": "The task in natural language, non-blank, at most 16 KiB UTF-8. Stored verbatim."
                    },
                    "fork_turns": {
                        "type": "string",
                        "enum": ["all", "none"],
                        "description": "all (default) starts the sub-agent from the closed active conversation prefix before this spawn request; none starts with no parent conversation. Both carry the trusted system prompt, frozen environment context and this task message."
                    }
                },
                "required": ["task_name", "message"],
                "additionalProperties": false
            }
        }
    })
}

/// send_message 工具定义（只入队，不启动空闲 Agent，§1.3）。
pub fn send_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": SEND_TOOL_NAME,
            "description": "Queues a message in the target agent's mailbox without starting an idle agent. The receiver reads pending messages after its current complete tool batch, before its next model request. From the root you may target a sub-agent of the current turn by name or full path; a sub-agent may only target /root. Same text twice is two messages; no deduplication by content.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Recipient path: a current-group sub-agent (relative name or /root/<name>) from the root, or exactly /root from a sub-agent."
                    },
                    "message": {
                        "type": "string",
                        "description": "Message body, non-blank, at most 16 KiB UTF-8. Stored verbatim."
                    }
                },
                "required": ["target", "message"],
                "additionalProperties": false
            }
        }
    })
}

/// followup_task 工具定义（仅根；空闲续派/运行中补充输入，§1.3）。
pub fn followup_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": FOLLOWUP_TOOL_NAME,
            "description": "Root only. On an idle agent (completed/failed/interrupted) this accepts a new assignment on the retained context and starts it; on a running agent the input is supplemental to the current assignment (no parallel turn, no budget reset); on a stopping agent it returns AGENT_BUSY.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Current-group sub-agent path (relative name or /root/<name>); never /root."
                    },
                    "message": {
                        "type": "string",
                        "description": "Task or supplemental input, non-blank, at most 16 KiB UTF-8. Stored verbatim."
                    }
                },
                "required": ["target", "message"],
                "additionalProperties": false
            }
        }
    })
}

/// wait_agent 工具定义（仅根；观察者，§1.3/§1.4）。
pub fn wait_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": WAIT_TOOL_NAME,
            "description": "Root only; must be the last call of an assistant batch. Observes the current group: wakes on undelivered reports/messages, terminal notifications or steer input, else times out. Returns only a summary; message bodies are delivered as labeled inputs after the current batch, before the next request. Timeout never means completion and never closes an agent. With no in-flight child task and no undelivered updates it returns no_pending immediately.",
            "parameters": {
                "type": "object",
                "properties": {
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Wait bound in milliseconds, 0..=60000. Defaults to 30000. 0 polls once without blocking."
                    }
                },
                "additionalProperties": false
            }
        }
    })
}

/// interrupt_agent 工具定义（仅根；关闭动作门进入收尾，§1.3/§1.5）。
pub fn interrupt_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": INTERRUPT_TOOL_NAME,
            "description": "Root only. Atomically revokes the target's current assignment admission, sets its cancel flag, abandons its undelivered supplemental inputs and enters stopping; the agent context is retained for followup in the same turn. Does not delete the agent or roll back changes. Repeated calls do not create new cancel tasks; an idle target keeps its state; a completed assignment stays completed.",
            "parameters": {
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Current-group sub-agent path (relative name or /root/<name>); never /root."
                    }
                },
                "required": ["target"],
                "additionalProperties": false
            }
        }
    })
}

/// list_agents 工具定义（仅根；按路径排序的组视图，§1.3）。
pub fn list_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": LIST_TOOL_NAME,
            "description": "Root only. Lists /root and the current group's sub-agents sorted by path, with state, assignment_id and the latest initial/followup task message (truncated for display beyond 16 KiB). Optional path_prefix accepts /root or an exact sub-path and filters by path segment (a does not match ab); a legal prefix with no match returns an empty list.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path_prefix": {
                        "type": "string",
                        "description": "/root or an exact sub-agent path; omitted lists the whole current group."
                    }
                },
                "additionalProperties": false
            }
        }
    })
}

/// 六工具定义（根用户轮声明面；仅当目录同时声明六名时进入）。
pub fn tool_definitions() -> Vec<serde_json::Value> {
    vec![
        spawn_tool_definition(),
        send_tool_definition(),
        followup_tool_definition(),
        wait_tool_definition(),
        interrupt_tool_definition(),
        list_tool_definition(),
    ]
}

/// CLI/TUI 共用的结果摘要行（宿主展示副本；调用方经显示过滤输出）。
pub fn cli_display_line(name: &str, result: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(result) else {
        return format!("[Agent] {name}：结果不可解析");
    };
    if v["ok"] != serde_json::json!(true) {
        let error = &v["error"];
        return format!(
            "[Agent] {name} 失败（{}）：{}",
            error.get("code").and_then(|c| c.as_str()).unwrap_or("?"),
            error.get("message").and_then(|m| m.as_str()).unwrap_or("?")
        );
    }
    let data = &v["data"];
    match name {
        SPAWN_TOOL_NAME => format!(
            "[Agent] 已受理子任务 {}（assignment {}；受理不代表完成，用 wait_agent 观察）",
            data["agent_name"].as_str().unwrap_or("?"),
            data["assignment_id"]
        ),
        SEND_TOOL_NAME => format!(
            "[Agent] 消息已入队（message {}；不启动空闲 Agent）",
            data["message_id"]
        ),
        FOLLOWUP_TOOL_NAME => format!(
            "[Agent] 续派已受理（assignment {}，started={}）",
            data["assignment_id"], data["started"]
        ),
        WAIT_TOOL_NAME => {
            let agents = data["agents"].as_array().map(|a| a.len()).unwrap_or(0);
            format!(
                "[Agent] wait：{}（agents {agents} 项，messages_pending {}）",
                data["reason"].as_str().unwrap_or("?"),
                data["messages_pending"]
            )
        }
        INTERRUPT_TOOL_NAME => format!(
            "[Agent] {} 中断：{} → {}",
            data["agent_name"].as_str().unwrap_or("?"),
            data["previous_state"].as_str().unwrap_or("?"),
            data["state"].as_str().unwrap_or("?")
        ),
        LIST_TOOL_NAME => {
            let count = data["agents"].as_array().map(|a| a.len()).unwrap_or(0);
            format!("[Agent] 列出 {count} 个 Agent（含 /root）")
        }
        other => format!("[Agent] {other} 完成"),
    }
}
