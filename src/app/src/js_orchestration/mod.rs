//! 034：JavaScript 工具编排——`exec` 运行一段 JS 组合已开放工具，`wait` 增量
//! 观察运行中的编排 cell。
//!
//! 架构（M0 原型证据：docs/logs/034-javascript-tool-orchestration/2026-10-29-m0-prototype.md）：
//! - worker＝本二进制隐藏入口 `--internal-js` 的独立子进程（启用 033 时经
//!   `sandbox::tool_launch_argv` 包装，env_clear＋fd 收口＋独立进程组＋RLIMIT）；
//! - JS 引擎＝rquickjs 0.14.0（QuickJS，固定版本；顶层 await 经 eval_promise，
//!   Promise 桥接经 JS prelude＋Persistent<Function>，宿主闭包只收发基本类型）；
//! - 有界 IPC＝长度前缀 JSON 帧（初始化/目录 ≤10 MiB，其他帧 ≤1 MiB）；
//! - 父进程是唯一能力与执行事实来源：叶子调用（exec_command/apply_patch/
//!   write_stdin/update_plan/023 资源工具/022 动态 MCP 工具）在父进程复用
//!   既有分派执行，worker 不直接接触文件/网络/进程。
//!
//! 本模块分层：[`args`]（参数严格校验）、[`protocol`]（IPC 帧）、[`worker`]
//! （--internal-js 入口与 JS 事件循环）、[`runtime`]（宿主侧 cell 运行时）。

pub mod args;
pub mod protocol;
pub mod runtime;
pub mod worker;

/// 编排工具名（模型目录 `experimental_supported_tools` 精确匹配项；两者
/// 必须同时显式声明才开放，缺一即整对关闭并给一次配置提示）。
pub const EXEC_TOOL_NAME: &str = "exec";
pub const WAIT_TOOL_NAME: &str = "wait";

/// exec 工具定义（034 §1.3：JSON code 输入；主动适配上游 raw JavaScript，
/// 不发送 Responses 托管 programmatic_tool_calling/program 类型）。
pub fn exec_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": EXEC_TOOL_NAME,
            "description": "Runs a JavaScript snippet that composes the already-declared tools: use `await` for sequential dependencies, `Promise.all` for independent work, filter results in the script, and return only what the model still needs to see via `text(...)`. The `code` runs in an isolated worker with no file, network, process or module APIs; it can only call tools from the current request's declared tool set via `tools[\"<tool name>\"]({...})`, which return Promises resolving to the tool's result object. `exec` returns when yield_time_ms elapses (status running, poll later with wait) or when the script reaches a terminal state; each observation returns only newly produced output.",
            "parameters": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "JavaScript source, non-blank, at most 64 KiB UTF-8. Compiled as an async script: top-level await is supported. Output for the model is produced only by calling text(value); the script's final value is not captured."
                    },
                    "yield_time_ms": {
                        "type": "integer",
                        "description": "How long this call waits in milliseconds, 0..=60000 (0 returns immediately). Defaults to 10000. This limits observation, never the script's CPU or wall-clock budget."
                    },
                    "max_output_tokens": {
                        "type": "integer",
                        "description": "Local UTF-8 byte budget per observation, 1..=16000. Defaults to 10000."
                    }
                },
                "required": ["code"],
                "additionalProperties": false
            }
        }
    })
}

/// wait 工具定义（034 §1.4）。
pub fn wait_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": WAIT_TOOL_NAME,
            "description": "Observes a running orchestration cell started by exec. Returns output produced since the last observation plus the current status; repeated waits return only increments. terminate=true closes the cell immediately: cancels queued and in-flight waits, terminates and reaps the JS worker, and returns the terminal state.",
            "parameters": {
                "type": "object",
                "properties": {
                    "cell_id": {
                        "type": "string",
                        "description": "Cell handle returned by exec, at most 128 UTF-8 bytes, exact match. Valid only in the same run instance and the same logical user turn that created it."
                    },
                    "yield_time_ms": {
                        "type": "integer",
                        "description": "Observation window in milliseconds, 0..=60000. Defaults to 10000. Never resets CPU, wall-clock, call or output budgets."
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Local UTF-8 byte budget for this observation, 1..=16000. Defaults to 10000."
                    },
                    "terminate": {
                        "type": "boolean",
                        "description": "true to terminate the cell now, ignoring the wait time. Defaults to false."
                    }
                },
                "required": ["cell_id"],
                "additionalProperties": false
            }
        }
    })
}
