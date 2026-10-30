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
use crate::proc_session::ProcessManager;
use crate::prompt::{
    build_turn_request_with, HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord,
    ToolDisplay, UserInputEvent,
};
use serde_json::{json, Value};
use std::path::Path;

/// 每用户轮模型请求上限（006：固定常量，无配置系统）。
pub const MAX_MODEL_REQUESTS_PER_TURN: usize = 8;
/// 单批 tool_calls 上限（2026-10-10 安全修复 N3：入史/执行前检查，超限整批拒绝）。
pub const MAX_TOOL_CALLS_PER_BATCH: usize = crate::prompt::MAX_TOOL_CALLS_PER_BATCH;

fn with_native(message: HistoryItem, state: &Option<crate::prompt::NativeState>) -> HistoryItem {
    match state {
        Some(state) => HistoryItem::NativeAssistant {
            message: Box::new(message), state: Some(state.clone()),
        },
        None => message,
    }
}

/// 一轮的工具面（013-M5 D3 参数化；017 四工具）：`User`＝既有用户轮契约
/// （恰 `exec_command`/`apply_patch`/`write_stdin`/`update_plan`，此外无新工具）；
/// `MemoryMerge`＝内部合并轮（恰 `mem_read`/`mem_write`/`mem_list`，
/// 限 `root` 记忆根；请求组装注入三项工具与合并系统提示，**不声明也不执行
/// update_plan**）。**权限在执行分派层生效**：不在当前模式注册表内的工具名
///（含另一模式的工具）一律按未知工具拒绝执行——仅靠请求体列出工具不构成
/// 权限保护。
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
/// 018 阶段 B：异步问询的等待/回答/关闭展示事件——数据为宿主过滤前原文，
/// 展示副本由宿主侧过滤；不携带活动问询通道。
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
    /// 显式等待开始：TUI 自动打开本轮未决问询窗口（不创建第二次问询）。
    UserInputAsyncWait {
        request_id: String,
    },
    /// 已受理回答入史：同一关联展示更新为已答，不伪造第二个工具结果。
    UserInputAsyncAnswered {
        request_id: String,
        questions: Vec<crate::user_input::Question>,
        response: crate::user_input::Response,
    },
    /// 未决问询关闭（task_completed/cancelled/request_limit 等）。
    UserInputAsyncClosed {
        request_id: String,
        questions: Vec<crate::user_input::Question>,
        reason: String,
    },
    /// 021：运行中接续输入恰一次入史（宿主确认提交回显：推用户消息 cell、
    /// 清除接入预览）。`input` 为宿主受理的原始提交（id 关联）。
    SteerIngested { input: SteerInput },
    /// 021：接续输入未入史交还宿主（图片准备失败或轮终止原因）——恢复为
    /// 待处理完整消息并暂停自动发送（§2.3-6）；已入史输入不经此事件。
    SteerRestored {
        inputs: Vec<SteerInput>,
        reason: String,
    },
}

/// 021 §2.3：运行中接续的普通输入（宿主生成提交标识；与宿主提交同构，
/// 图片保留路径——正式入史前才读取准备，不提前冻结）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerInput {
    /// 宿主生成的提交标识（入史/恢复回执按 id 恰一次关联）。
    pub id: u64,
    pub text: String,
    pub image_paths: Vec<std::path::PathBuf>,
    pub text_elements: Vec<codex_protocol::user_input::TextElement>,
}

/// 021 §2.3-5：终文竞态原子判定的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum SteerSeal {
    /// 无待处理输入且已封口：调用方可结束本轮；此后提交由宿主回执"当前轮已结束"。
    Closed,
    /// 已受理输入（按受理顺序）；受理保持开放，调用方继续当前轮。
    Inputs(Vec<SteerInput>),
}

/// 021 §2.3：运行中普通输入接续通道（每轮独立；宿主实现）。
/// 提交侧只经短锁入队，不获取 turn 长时间持有的 history/autosave 锁；
/// 收取输入与封闭当前轮在同一接续状态上原子判定。
pub trait SteerChannel {
    /// 非阻塞收取已受理输入（受理保持开放；入史边界由循环固定在完整批末）。
    fn drain(&mut self) -> Vec<SteerInput>;
    /// 035：非消费性探测「是否有已受理未收取输入」（wait_agent 的 steer
    /// 唤醒判定——只观察不收取，收取仍固定在批末 drain）。默认 false＝
    /// 宿主无探测能力（steer 原因退化为 timeout）。
    fn pending_hint(&self) -> bool {
        false
    }
    /// 终文竞态原子判定（§2.3-5）：已有已受理输入→收取且保持受理开放；
    /// 为空→原子封口。
    fn seal(&mut self) -> SteerSeal;
    /// 无条件封口：撤回受理资格并返回已受理未消化输入（取消/失败/额度出口
    /// 交还宿主恢复）。每个轮出口恰调用一次。
    fn close(&mut self) -> Vec<SteerInput>;
}

/// 021：用户轮附加能力与接续通道（宿主按每轮快照传入；既有入口默认全关）。
/// 022：`mcp`＝本轮冻结的 MCP 工具快照（声明与分派共用；None＝无 MCP；
/// MemoryMerge 强制忽略）。
/// 030：`tool_search`＝模型目录显式声明 tool_search（声明与执行共用快照；
/// 实际声明还要求本轮快照存在可搜索动态工具；MemoryMerge 强制忽略）。
/// 032：`context`＝上下文管理工具宿主（查询计量＋检查点提交；None＝不声明
/// 不执行；MemoryMerge 强制忽略——内部记忆轮不开放上下文工具）。
/// 035：`agents`＝多 Agent 宿主（六工具声明与执行同门；模型目录六名全配
/// 已由调用方核对；MemoryMerge 强制忽略——内部记忆轮不声明不执行六工具）。
pub struct TurnExtras<'a> {
    /// 模型目录显式声明 send_message_to_user_async（声明与执行共用快照）。
    pub send_message: bool,
    /// 022：本轮 MCP 快照（工具定义＋原始身份映射＋连接句柄）。
    pub mcp: Option<&'a crate::mcp::manager::McpSnapshot>,
    /// 030：模型目录显式声明 tool_search（本地目录搜索；旧入口默认关闭）。
    pub tool_search: bool,
    /// 运行中普通输入接续通道（None＝无运行中接续能力；MemoryMerge 强制忽略）。
    pub steer: Option<&'a mut dyn SteerChannel>,
    /// 032：上下文管理工具宿主（None＝两工具均不声明不执行）。
    pub context: Option<crate::context_window::ContextHost<'a>>,
    /// 031：权限确认宿主（None＝不声明不执行 request_permissions；MemoryMerge
    /// 强制忽略——内部记忆轮无申请入口）。实际生效还需启动装配的冻结策略
    /// （031 后端可用）与模型目录能力声明。
    pub permissions: Option<crate::permissions::PermissionExtras<'a>>,
    /// 034：编排宿主（None＝不声明不执行 exec/wait；模型目录双 opt-in 已由
    /// 调用方核对；MemoryMerge 强制忽略——内部记忆轮始终不声明不执行）。
    pub orchestration: Option<crate::js_orchestration::runtime::OrchestrationExtras<'a>>,
    /// 035：多 Agent 宿主（None＝不声明不执行六工具）。
    pub agents: Option<&'a crate::agents::runtime::AgentExtras<'a>>,
}

impl TurnExtras<'_> {
    /// 既有入口的兼容取值：无消息能力、无 MCP、无搜索、无接续通道、无上下文工具。
    pub fn none() -> Self {
        Self {
            send_message: false,
            mcp: None,
            tool_search: false,
            steer: None,
            context: None,
            permissions: None,
            orchestration: None,
            agents: None,
        }
    }
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
/// 单一生产者 [`skipped_without_execution`]，非模糊文本匹配）。
pub const SKIPPED_BY_CANCEL_MESSAGE: &str = "用户取消，本调用未执行";

/// 016：本轮被取消的 turn 错误统一前缀（含工具批取消与在途请求取消两条路径）；
/// 宿主以此常量判定"取消当前轮"，不反解析消息文本（012 教训）。
pub const CANCELLED_TURN_ERROR_PREFIX: &str = "用户已取消";

fn skipped_without_execution(
    name: &str,
    mode: &ToolMode<'_>,
    error: &str,
    cancelled: bool,
) -> String {
    if let ToolMode::MemoryMerge { .. } = mode {
        if let Some(shape) = crate::memory::mem_tools::skipped_by_cancel_shape(name) {
            return shape;
        }
        return json!({ "error": error }).to_string();
    }
    match name {
        "exec_command" | "write_stdin" => serde_json::to_string(&ExecResult {
            status: if cancelled {
                ExecStatus::Cancelled
            } else {
                ExecStatus::Error
            },
            exit_code: None,
            output: String::new(),
            truncated: false,
            error: Some(error.to_string()),
            // 016：未执行调用不启动进程，返回空句柄
            session_id: None,
        })
        .expect("ExecResult 可序列化"),
        "apply_patch" => serde_json::to_string(&ApplyPatchResult {
            ok: false,
            changed_files: Vec::new(),
            state_known: true,
            error: Some(error.to_string()),
        })
        .expect("ApplyPatchResult 可序列化"),
        // 017：update_plan 未执行＝两字段失败结构（不产生成功计划快照）
        "update_plan" => json!({ "ok": false, "error": error }).to_string(),
        // 030：tool_search 未执行＝三字段失败结构（§1.5：空 tools、明确原因）
        name if name == crate::tool_search::TOOL_NAME => {
            crate::tool_search::failure_result(error).to_string()
        }
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
    ToolCallRecord::parse_batch(message).map(|records| {
        records.into_iter().map(|call| ParsedToolCall {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
        }).collect()
    })
}


/// M2（034）：用户轮核心四工具的共用执行核心——直接调用（turn 主循环）与
/// 嵌套调用（034 cell 叶子）共用同一分派与参数解析（先证明直接调用行为未变：
/// 全量回归覆盖）。`grants`＝031 轮授权（直接调用＝当前轮状态；嵌套＝cell
/// 出生时冻结的快照，§1.5 逻辑轮授权不被 cell 延长）。未知工具按通用单
/// `error` 字段结构回传（006 既有契约）。
pub(crate) fn execute_core_tool(
    name: &str,
    arguments: &str,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    procs: &ProcessManager,
    grants: Option<&crate::permissions::TurnGrants>,
) -> (String, Option<ToolDisplay>) {
    execute_core_tool_owned(name, arguments, cancel, procs, grants, &crate::proc_session::OwnerTag::Root)
}

/// M2（035）：带进程归属的执行核心——子 Agent 的 exec/write_stdin 按
/// (group, agent, assignment) 登记（句柄校验与按 owner 回收）；根调用
/// （直接与嵌套）沿用 [`execute_core_tool`]＝Root。
pub(crate) fn execute_core_tool_owned(
    name: &str,
    arguments: &str,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    procs: &ProcessManager,
    grants: Option<&crate::permissions::TurnGrants>,
    owner: &crate::proc_session::OwnerTag,
) -> (String, Option<ToolDisplay>) {
    // 031 出生快照——以冻结策略为键（031 §1.2-3：无宿主同样取「基线＋空授权」；
    // 构建失败＝毒丸，本次执行显式失败）。
    let empty = crate::permissions::TurnGrants::default();
    let exec_snapshot: Option<std::sync::Arc<crate::sandbox::PolicySnapshot>> =
        crate::permissions::frozen().map(|f| {
            std::sync::Arc::new(match crate::permissions::build_snapshot(
                &f.policy,
                grants.unwrap_or(&empty),
                &f.deny_ctx,
            ) {
                Ok(s) => s,
                Err(e) => crate::sandbox::PolicySnapshot::poisoned(e),
            })
        });
    match name {
        "exec_command" => match serde_json::from_str::<Value>(arguments) {
            Ok(v) => match crate::exec::ExecArgs::from_value(&v) {
                Ok(args) => {
                    let started = std::time::Instant::now();
                    let extra = grants.is_some_and(|g| !g.is_empty());
                    let outcome =
                        procs.start_with_owner(&args, cancel, exec_snapshot.as_ref(), extra, owner.clone());
                    let display = ToolDisplay::Exec {
                        duration_ms: u64::try_from(started.elapsed().as_millis())
                            .unwrap_or(u64::MAX),
                    };
                    (
                        serde_json::to_string(&outcome.result).expect("ExecResult 可序列化"),
                        Some(display),
                    )
                }
                Err(msg) => (
                    serde_json::to_string(&ExecResult::error(msg))
                        .expect("ExecResult 可序列化"),
                    None,
                ),
            },
            Err(e) => (
                serde_json::to_string(&ExecResult::error(format!("工具参数不是有效 JSON：{e}")))
                    .expect("ExecResult 可序列化"),
                None,
            ),
        },
        "write_stdin" => match serde_json::from_str::<Value>(arguments) {
            Ok(v) => match crate::exec::WriteStdinArgs::from_value(&v) {
                Ok(args) => {
                    let started = std::time::Instant::now();
                    let outcome = procs.interact_as_owner(&args, cancel, owner);
                    let duration_ms =
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let display = Some(
                        outcome
                            .session_cmd
                            .map(|(session_id, command)| ToolDisplay::Stdin {
                                session_id,
                                command,
                                duration_ms,
                            })
                            .unwrap_or_else(|| ToolDisplay::Stdin {
                                session_id: args.session_id.clone(),
                                command: String::new(),
                                duration_ms,
                            }),
                    );
                    (
                        serde_json::to_string(&outcome.result).expect("ExecResult 可序列化"),
                        display,
                    )
                }
                Err(msg) => (
                    serde_json::to_string(&ExecResult::error(msg))
                        .expect("ExecResult 可序列化"),
                    None,
                ),
            },
            Err(e) => (
                serde_json::to_string(&ExecResult::error(format!("工具参数不是有效 JSON：{e}")))
                    .expect("ExecResult 可序列化"),
                None,
            ),
        },
        "apply_patch" => match serde_json::from_str::<Value>(arguments) {
            Ok(v) => {
                // 033：沙箱模式下补丁经 bwrap 包装的本二进制 --internal-apply
                // 子进程应用；031 动态策略启用时走出生快照（毒丸＝显式错误）。
                let (r, changes) = match exec_snapshot.as_ref().map(|s| s.valid()) {
                    Some(Some(snap)) => crate::apply_patch::apply_patch_sandboxed(&v, Some(snap)),
                    Some(None) => {
                        let reason = exec_snapshot
                            .as_ref()
                            .and_then(|s| s.poison.clone())
                            .unwrap_or_else(|| "未知原因".into());
                        (
                            crate::apply_patch::ApplyPatchResult::error(
                                format!("生效策略无法编码：{reason}（本次补丁拒绝，不回退）"),
                                Vec::new(),
                                false,
                            ),
                            Vec::new(),
                        )
                    }
                    None if crate::sandbox::sandbox_requested() => {
                        crate::apply_patch::apply_patch_sandboxed(&v, None)
                    }
                    None => crate::apply_patch::apply_patch_with_display(&v),
                };
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
        // 017：update_plan——解析参数、回传两字段结果；不执行步骤、不触文件。
        "update_plan" => (crate::plan::execute(arguments), None),
        other => (
            json!({
                "error": format!("未知工具 {other}，可用：exec_command、apply_patch、write_stdin、update_plan")
            })
            .to_string(),
            None,
        ),
    }
}

/// M2（034 §1.2）：嵌套调用的宿主分派——可调用范围＝exec_command/
/// write_stdin/apply_patch/update_plan＋023 资源工具＋022 动态 MCP 工具
/// （cell 出生时当前请求已声明集合的交集）。伪造名称、未声明/未加载工具
/// 零调用；条件宿主工具（问询/消息/搜索/权限/上下文/exec/wait 自身）不在
/// 嵌套目录，落入未知工具错误。`frozen_grants`＝cell 出生时轮授权快照。
pub(crate) fn dispatch_nested_leaf(
    procs: &ProcessManager,
    mcp: Option<&crate::mcp::manager::McpSnapshot>,
    declared_dynamic: &std::collections::HashSet<String>,
    frozen_grants: &Option<crate::permissions::TurnGrants>,
    tool: &str,
    args: &Value,
    cancel: &std::sync::atomic::AtomicBool,
) -> crate::js_orchestration::runtime::LeafOutcome {
    use crate::js_orchestration::runtime::{LeafExecution, LeafOutcome};
    let arguments = serde_json::to_string(args).unwrap_or_default();
    // 资源工具（023）：快照资源能力决定。
    if mcp.is_some_and(|s| s.has_resources()) && crate::mcp::resources::is_resource_tool(tool) {
        let (content, _failed) = mcp
            .expect("已确认存在")
            .call_resource_tool(tool, &argument_json(args), cancel);
        return LeafOutcome {
            result_json: content,
            display: None,
            execution: LeafExecution::Completed,
        };
    }
    // 动态 MCP（022）：目录精确命中＋cell 出生时已声明集合（030 §1.4-2：正在
    // 运行的 cell 不扩展目录——恢复出的 Schema、同轮后续搜索结果或猜测名称
    // 都不增加调用资格）。
    if let Some(entry) = mcp.and_then(|s| s.entry(tool)) {
        if declared_dynamic.contains(&entry.model_name) {
            let (content, _failed) = if !args.is_object() {
                (
                    crate::mcp::call::client_error_json("参数", "arguments 必须是 JSON 对象"),
                    true,
                )
            } else {
                mcp.expect("已确认命中").call(entry, args.clone(), cancel)
            };
            return LeafOutcome {
                result_json: content,
                display: None,
                execution: LeafExecution::Completed,
            };
        }
    }
    // 核心四工具（与直接调用同一分派）。
    let (result_json, display) =
        execute_core_tool(tool, &arguments, Some(cancel), procs, frozen_grants.as_ref());
    LeafOutcome {
        result_json,
        display,
        execution: LeafExecution::Completed,
    }
}

/// 参数 JSON 的字符串形态（资源工具入参；与主循环分派同源）。
fn argument_json(args: &Value) -> String {
    serde_json::to_string(args).unwrap_or_default()
}


/// 034：turn 侧叶子分派器（cell 出生时冻结的快照：进程管理器克隆、MCP 快照
/// Arc、当前请求已声明动态集合与 031 轮授权；「逻辑轮授权不被 cell 延长」
/// ——cell 生命周期内固定使用出生时点快照，§1.5）。
struct TurnLeafDispatcher {
    procs: ProcessManager,
    mcp: Option<std::sync::Arc<crate::mcp::manager::McpSnapshot>>,
    declared_dynamic: std::collections::HashSet<String>,
    grants: Option<crate::permissions::TurnGrants>,
}

impl crate::js_orchestration::runtime::LeafDispatcher for TurnLeafDispatcher {
    fn dispatch(
        &self,
        tool: &str,
        args: &Value,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> crate::js_orchestration::runtime::LeafOutcome {
        dispatch_nested_leaf(
            &self.procs,
            self.mcp.as_deref(),
            &self.declared_dynamic,
            &self.grants,
            tool,
            args,
            cancel,
        )
    }
}

/// 034：cell 出生时的工具目录构建——来源＝当前请求声明面（`body.tools`，与
/// 模型看到的一致），成员＝嵌套允许集（core 四工具＋023 资源工具＋cell 出生
/// 时已声明的动态 MCP 工具）；exec/wait 自身与条件宿主工具不进目录（伪造
/// 名称零调用，§1.2）。
fn orchestration_catalog(
    body: &Value,
    mcp: Option<&crate::mcp::manager::McpSnapshot>,
    declared_dynamic: &std::collections::HashSet<String>,
) -> Vec<crate::js_orchestration::runtime::ToolCatalogEntry> {
    use crate::js_orchestration::runtime::ToolCatalogEntry;
    let mut out = Vec::new();
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return out;
    };
    let is_serial = |name: &str| {
        matches!(name, "apply_patch" | "update_plan" | "write_stdin")
    };
    let allowed = |name: &str| -> bool {
        if matches!(name, "exec_command" | "write_stdin" | "apply_patch" | "update_plan") {
            return true;
        }
        // 资源工具：快照存在资源能力时的固定三项
        if mcp.is_some_and(|s| s.has_resources()) && crate::mcp::resources::is_resource_tool(name)
        {
            return true;
        }
        // 动态 MCP：恰为当前请求已声明集合内的模型名（030 §1.4-2 冻结）
        declared_dynamic.contains(name)
    };
    for tool in tools {
        let Some(name) = tool.pointer("/function/name").and_then(Value::as_str) else {
            continue;
        };
        if !allowed(name) {
            continue;
        }
        out.push(ToolCatalogEntry {
            name: name.to_string(),
            description: tool
                .pointer("/function/description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            parameters: tool
                .pointer("/function/parameters")
                .cloned()
                .unwrap_or_else(|| json!({})),
            serial: is_serial(name),
        });
    }
    out
}


/// 034：观察结果→exec/wait 固定字段 JSON（§1.3：ok/status/cell_id/output/
/// output_truncated/error；仅 running/completed 为 ok=true）。
fn orchestration_observation_json(obs: &crate::js_orchestration::runtime::Observation) -> String {
    json!({
        "ok": obs.ok,
        "status": obs.status.as_str(),
        "cell_id": obs.cell_id,
        "output": obs.output,
        "output_truncated": obs.output_truncated,
        "error": obs.error,
    })
    .to_string()
}

/// 参数级失败结果（创建 cell 前：cell_id=null；§1.3）。
fn orchestration_argument_error(message: String) -> String {
    json!({
        "ok": false,
        "status": "failed",
        "cell_id": null,
        "output": [],
        "output_truncated": false,
        "error": { "code": "INVALID_ARGUMENTS", "message": message },
    })
    .to_string()
}

/// 句柄级失败结果（带真实句柄）。
fn orchestration_handle_error(
    cell_id: Option<String>,
    code: &str,
    message: String,
) -> String {
    json!({
        "ok": false,
        "status": "failed",
        "cell_id": cell_id,
        "output": [],
        "output_truncated": false,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

/// 已提交编排事件的内存入史（事件已由 sink 落盘——07 游标已越过，on_record
/// 不产生二次写；memory/threads 登记照常推进）。
fn push_orchestration_events(
    cell: &mut crate::js_orchestration::runtime::CellHandle,
    history: &mut Vec<HistoryItem>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
) {
    let events = crate::js_orchestration::runtime::take_committed_events(cell);
    if events.is_empty() {
        return;
    }
    for event in events {
        history.push(event);
    }
    on_record(history);
}

/// 034 §1.3/§1.4：exec/wait 的分派执行（独占批已由批规则校验；本函数内完成
/// 参数解析、CELL_BUSY/CELL_NOT_FOUND 判定、cell 生命周期与观察）。
#[allow(clippy::too_many_arguments)]
fn execute_orchestration_call(
    call: &ParsedToolCall,
    active_cell: &mut Option<crate::js_orchestration::runtime::CellHandle>,
    orch: &mut crate::js_orchestration::runtime::OrchestrationExtras<'_>,
    body: &Value,
    declared_dynamic: &std::collections::HashSet<String>,
    grants: Option<crate::permissions::TurnGrants>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    procs: &ProcessManager,
    history: &mut Vec<HistoryItem>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
) -> String {
    use crate::js_orchestration::runtime::CellStatus;
    if call.name == crate::js_orchestration::EXEC_TOOL_NAME {
        let args = match crate::js_orchestration::args::parse_exec_arguments(&call.arguments) {
            Ok(args) => args,
            Err(e) => return orchestration_argument_error(e),
        };
        // 同轮最多一个未消费终态 cell：有旧 cell 时新 exec 按 CELL_BUSY 失败
        // 并保留旧 cell（不淘汰，§1.3）。
        if let Some(old) = active_cell.as_ref() {
            return orchestration_handle_error(
                Some(old.id.clone()),
                "CELL_BUSY",
                format!(
                    "当前逻辑用户轮已有未消费的编排 cell {}：先用 wait 取回或终止后再发起新 exec",
                    old.id
                ),
            );
        }
        let catalog = orchestration_catalog(body, orch.mcp.as_deref(), declared_dynamic);
        let dispatcher: std::sync::Arc<dyn crate::js_orchestration::runtime::LeafDispatcher> =
            std::sync::Arc::new(TurnLeafDispatcher {
                procs: procs.clone(),
                mcp: orch.mcp.clone(),
                declared_dynamic: declared_dynamic.clone(),
                grants,
            });
        let arguments_json = call.arguments.clone();
        let config = crate::js_orchestration::runtime::CellConfig {
            code: &args.code,
            arguments_json: &arguments_json,
            catalog,
            exec_call_id: &call.id,
            turn_budget: orch.turn_budget.clone(),
            dispatcher,
            worker_exe: None,
        };
        let mut cell =
            match crate::js_orchestration::runtime::spawn_cell(&config, orch.sink, history) {
                Ok(cell) => cell,
                Err(e) => {
                    // 区分保存失败（PERSIST_FAILED）与进程启动失败（审查 P2-7）。
                    let (code, prefix) = if e.contains("cell_started 保存失败") {
                        ("PERSIST_FAILED", "编排 cell 创建失败")
                    } else {
                        ("WORKER_SPAWN_FAILED", "编排 worker 启动失败")
                    };
                    return orchestration_handle_error(None, code, format!("{prefix}：{e}（无回退）"));
                }
            };
        let obs_budget = crate::js_orchestration::protocol::observation_budget(args.max_output_tokens);
        let observation =
            cell.observe(args.yield_time_ms, obs_budget, orch.sink, cancel, history);
        push_orchestration_events(&mut cell, history, on_record);
        let result = orchestration_observation_json(&observation);
        if !cell.is_closed() {
            *active_cell = Some(cell);
        }
        return result;
    }
    // wait
    let args = match crate::js_orchestration::args::parse_wait_arguments(&call.arguments) {
        Ok(args) => args,
        Err(e) => return orchestration_argument_error(e),
    };
    // terminate：忽略等待时间，原子关闭（§1.4）。
    if args.terminate {
        // terminate 同样要求 cell_id 原样精确匹配（§1.4；审查 P2-6——不因
        // 终止意图放宽句柄校验）。
        let Some(cell) = active_cell.as_mut() else {
            return orchestration_handle_error(None, "CELL_NOT_FOUND", "当前没有可终止的编排 cell".into());
        };
        if cell.id != args.cell_id {
            return orchestration_handle_error(
                None,
                "CELL_NOT_FOUND",
                "cell_id 无效或已消费（句柄仅在当前逻辑用户轮内有效）".into(),
            );
        }
        let observation = cell.terminate(orch.sink, history);
        push_orchestration_events(cell, history, on_record);
        *active_cell = None;
        return orchestration_observation_json(&observation);
    }
    // 只能等待本轮已由 exec 返回的 cell；无效/旧/已消费句柄统一 CELL_NOT_FOUND
    //（不泄露另一会话是否存在，§1.4）。
    let Some(cell) = active_cell.as_mut() else {
        return orchestration_handle_error(
            None,
            "CELL_NOT_FOUND",
            "cell_id 无效或已消费（句柄仅在当前逻辑用户轮内有效）".into(),
        );
    };
    if cell.id != args.cell_id {
        return orchestration_handle_error(
            None,
            "CELL_NOT_FOUND",
            "cell_id 无效或已消费（句柄仅在当前逻辑用户轮内有效）".into(),
        );
    }
    let obs_budget = crate::js_orchestration::protocol::observation_budget(args.max_tokens);
    let observation = cell.observe(args.yield_time_ms, obs_budget, orch.sink, cancel, history);
    push_orchestration_events(cell, history, on_record);
    // 仅句柄真正释放（终态且输出取尽，runtime 已 close）才丢弃——终态输出
    // 跨多次观察交付时句柄保留（审查 P1-1；§1.4「重复 wait 只返回增量」）。
    if cell.is_closed() {
        *active_cell = None;
    }
    orchestration_observation_json(&observation)
}

/// 执行单个调用：按当前 [`ToolMode`] 的注册表分派；参数错误与工具失败按各
/// 工具既有结果结构回传 JSON 字符串（构造结果结构体序列化，字段与工具本体
/// 同源），保留失败/状态字段，不伪造成功；未注册工具（含跨模式工具名）不
/// 执行——分派层即权限边界（D3）。
/// 016：exec_command／write_stdin 经 `procs` 执行——进程跨调用跨轮保留，
/// 取消当前轮不再自动终止已启动进程（等待返回 cancelled＋句柄）。
/// 012：同时返回**展示数据**（exec 真实时长；apply_patch 逐文件真实差异；
/// write_stdin 原命令关联）——随历史记录持久、随通知回推，但不混入结果 JSON
/// （模型契约不变）；mem 工具为内部合并轮专用，无展示数据（None）。
fn execute_tool(
    call: &ParsedToolCall,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    mode: &ToolMode<'_>,
    procs: &ProcessManager,
    mut perm: Option<&mut crate::permissions::TurnPermState<'_>>,
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
    // 031：request_permissions——声明与执行同门（§1.2-4：无 031 状态或无
    // 宿主＝未注册工具，伪造调用按未知工具契约拒绝，不启动确认界面）。
    // 其余工具委托 [`execute_core_tool`]（M2 起与嵌套调用共用同一分派）。
    let grants = perm.as_ref().map(|st| st.grants.clone());
    match call.name.as_str() {
        name if name == crate::permissions::TOOL_NAME => match perm {
            Some(st) => {
                let frozen = crate::permissions::frozen()
                    .expect("轮内权限状态存在则 031 冻结策略已装配");
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
                (
                    crate::permissions::execute_request_permissions(
                        &call.arguments,
                        &cwd,
                        &frozen.policy,
                        &mut st.grants,
                        st.host,
                        &frozen.deny_ctx,
                    ),
                    None,
                )
            }
            None => (
                json!({
                    "error": format!(
                        "未知工具 {}，当前模式未启用权限申请（需要 031 策略与确认宿主）",
                        call.name
                    )
                })
                .to_string(),
                None,
            ),
        },
        _ => execute_core_tool(
            &call.name,
            &call.arguments,
            cancel,
            procs,
            grants.as_ref(),
        ),
    }
}

/// 运行一个用户轮（`ToolMode::User` 固定面）。语义见 [`run_tool_turn_with_mode`]；
/// `procs`＝016 进程会话管理器（跨工具调用、跨用户轮保留进程）。
pub fn run_tool_turn(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    procs: &ProcessManager,
) -> Result<String, String> {
    run_tool_turn_with_mode(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        ToolMode::User,
        procs,
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
    procs: &ProcessManager,
) -> Result<String, String> {
    run_user_turn_with_context_and_host(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        sources,
        procs,
        None,
        false,
        None,
    )
}

/// 018：原上下文入口保留无宿主行为；只在交互宿主传入问答接口。
/// 019：`image_input`＝本轮模型图片能力快照（声明与执行分派共用，轮内固定）。
pub fn run_user_turn_with_context_and_host(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    sources: &crate::context::ContextSources,
    procs: &ProcessManager,
    input_host: Option<&mut crate::user_input::InputHost<'_>>,
    image_input: bool,
    user_images: Option<&crate::prompt::UserImagePayload>,
) -> Result<String, String> {
    run_user_turn_with_context_and_hosts(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        sources,
        procs,
        input_host,
        None,
        image_input,
        user_images,
    )
}

/// 018 阶段 B：TUI 用户轮同时传入同步与异步问答宿主（七工具）；
/// CLI 仍走 [`run_user_turn_with_context_and_host`]（五工具，仅同步）。
/// 019：`image_input`＝本轮模型图片能力快照；MemoryMerge 内部轮不经本入口。
/// 021 兼容入口：无消息能力、无运行中接续通道。
pub fn run_user_turn_with_context_and_hosts(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    sources: &crate::context::ContextSources,
    procs: &ProcessManager,
    input_host: Option<&mut crate::user_input::InputHost<'_>>,
    async_host: Option<&mut crate::user_input_async::AsyncInputHost<'_>>,
    image_input: bool,
    user_images: Option<&crate::prompt::UserImagePayload>,
) -> Result<String, String> {
    run_user_turn_with_context_and_caps(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        sources,
        procs,
        input_host,
        async_host,
        image_input,
        user_images,
        TurnExtras::none(),
    )
}

/// 021 完整入口：在既有宿主参数之上接收 [`TurnExtras`]（消息能力声明/执行
/// 共用快照＋运行中普通输入接续通道）。TUI／行式宿主按模型目录声明传入。
#[allow(clippy::too_many_arguments)]
pub fn run_user_turn_with_context_and_caps(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    sources: &crate::context::ContextSources,
    procs: &ProcessManager,
    input_host: Option<&mut crate::user_input::InputHost<'_>>,
    async_host: Option<&mut crate::user_input_async::AsyncInputHost<'_>>,
    image_input: bool,
    user_images: Option<&crate::prompt::UserImagePayload>,
    extras: TurnExtras<'_>,
) -> Result<String, String> {
    // 020 A05：空白校验与底层一致（纯图合法）
    let has_user_images = user_images.is_some_and(|p| !p.data_urls.is_empty());
    if current_input.trim().is_empty() && !has_user_images {
        return Err(BLANK_INPUT_ERROR.into());
    }
    if !matches!(mode, ToolMode::MemoryMerge { .. }) {
        let built = crate::context::build_context_message(sources);
        for notice in &built.notices {
            notify(TurnNotify::Info(notice.clone()));
        }
        // 032：视图损坏时明确失败（不忽略标记回退全历史）；去重基线直接扫完整
        // 历史——seed 中的上下文消息是最后一条顶层上下文消息的原样副本
        //（build_seed 复制 last_context_body(history)），两处扫描结果恒等
        //（等价性由 m5_context_dedup… 回归锁定），免去每用户轮的整视图深克隆
        //（审查修复 #16）。
        crate::context_window::active_view(history).map_err(|reason| reason)?;
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
                images: None,
                text_elements: Vec::new(),
                role: HistoryRole::User,
                content: text,
                user_input_event: None,
            }));
            on_record(history);
        }
    }
    run_tool_turn_with_hosts_extras(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        procs,
        input_host,
        async_host,
        image_input,
        user_images,
        extras,
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
/// `procs`＝进程会话管理器（016）：仅 `User`/`UserWithMemory` 分派使用；
/// `MemoryMerge` 不经它执行任何工具。
/// 返回最终回复；错误（协议/请求失败/上限）时历史保留已发生记录，不自动重发。
/// 016：取消当前轮只停止等待与剩余调用，不终止已启动进程。
pub fn run_tool_turn_with_mode(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    procs: &ProcessManager,
) -> Result<String, String> {
    run_tool_turn_with_host(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        procs,
        None,
    )
}

/// 018：同步宿主等待接口。MemoryMerge 即使误传接口也不会声明或调用它。
/// 019：不声明 view_image（图片能力快照固定 false 的兼容入口）。
pub fn run_tool_turn_with_host(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    procs: &ProcessManager,
    input_host: Option<&mut crate::user_input::InputHost<'_>>,
) -> Result<String, String> {
    run_tool_turn_with_hosts(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        procs,
        input_host,
        None,
        false,
        None,
    )
}

/// 018 阶段 B：本轮唯一未决异步问询。`channel`＝`Some` 表示尚未收到终态；
/// `answer`＝已受理未入史回答（占用该项，完成入史后才释放）。
struct PendingAsyncInput {
    request_id: String,
    questions: Vec<crate::user_input::Question>,
    channel: Option<crate::user_input_async::AsyncReplyChannel>,
    answer: Option<crate::user_input::Response>,
}

/// 检查点收取回执（工具完成/模型返回后调用）：只暂存，入史固定在批末。
/// 返回 `Some(error)`＝通道终态为失败（已受理答案不存在于该情形）。
fn drain_pending_async(
    pending: &mut Option<PendingAsyncInput>,
) -> Option<crate::user_input::InputError> {
    let Some(p) = pending else { return None };
    if p.answer.is_some() {
        return None;
    }
    let Some(channel) = p.channel.as_mut() else {
        return None;
    };
    match channel.receiver.try_recv() {
        Ok(Ok(response)) => {
            p.channel = None; // 一次性终态：此后迟到/重复回执不再消费
            if crate::user_input::validate_response(&p.questions, &response).is_ok() {
                p.answer = Some(response);
                None
            } else {
                Some(crate::user_input::InputError::InvalidResponse)
            }
        }
        Ok(Err(error)) => {
            p.channel = None;
            Some(error)
        }
        // 未见过终态的断开＝通道断开（正常一次性通道见值后不再进入本分支）
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            p.channel = None;
            Some(crate::user_input::InputError::Closed)
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
    }
}

/// 已受理回答入史（§8.4/8.6）：正文保留 request_id 与答案原文＋来源标记，
/// 随下一请求供模型使用；同时通知宿主更新同一关联展示。
fn record_async_answer(
    history: &mut Vec<HistoryItem>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    notify: &mut dyn FnMut(TurnNotify),
    pending: &PendingAsyncInput,
) {
    let response = pending
        .answer
        .as_ref()
        .expect("入史前必已有已受理回答");
    history.push(HistoryItem::Text(HistoryMessage {
        images: None,
        text_elements: Vec::new(),
        role: HistoryRole::User,
        content: crate::user_input_async::answer_event_content(&pending.request_id, response),
        user_input_event: Some(UserInputEvent::Answer {
            request_id: pending.request_id.clone(),
        }),
    }));
    on_record(history);
    notify(TurnNotify::UserInputAsyncAnswered {
        request_id: pending.request_id.clone(),
        questions: pending.questions.clone(),
        response: response.clone(),
    });
}

/// 关闭未决问询（§8.5）：仅未答项记录关闭事件；已答项的终态即答案事件，
/// 不追加第二条记录。释放单项占用。
fn close_pending_async(
    history: &mut Vec<HistoryItem>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    notify: &mut dyn FnMut(TurnNotify),
    pending: &mut Option<PendingAsyncInput>,
    reason: &str,
) {
    if let Some(p) = pending.take() {
        if p.answer.is_none() {
            history.push(HistoryItem::Text(HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: HistoryRole::User,
                content: crate::user_input_async::close_event_content(&p.request_id, reason),
                user_input_event: Some(UserInputEvent::Closed {
                    request_id: p.request_id.clone(),
                }),
            }));
            on_record(history);
            notify(TurnNotify::UserInputAsyncClosed {
                request_id: p.request_id.clone(),
                questions: p.questions.clone(),
                reason: reason.to_string(),
            });
        }
    }
}

/// 模型/协议失败收尾（§8.5）：已受理答案先恰一次入史，再仅对未答项
/// 记 model_error 关闭（已答项终态即答案事件，不追加第二条记录）。
fn close_pending_on_model_error(
    history: &mut Vec<HistoryItem>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    notify: &mut dyn FnMut(TurnNotify),
    pending: &mut Option<PendingAsyncInput>,
) {
    if let Some(p) = pending.as_ref().filter(|p| p.answer.is_some()) {
        record_async_answer(history, on_record, notify, p);
        *pending = None;
    }
    close_pending_async(
        history,
        on_record,
        notify,
        pending,
        crate::user_input_async::CLOSE_MODEL_ERROR,
    );
}

/// 021：轮出口统一封口接续通道；未入史的已受理输入恰一次交还宿主恢复
///（§2.3-6：保留完整消息并暂停自动发送，不丢弃、不入史）。
fn close_steer_restore(
    steer: &mut Option<&mut dyn SteerChannel>,
    notify: &mut dyn FnMut(TurnNotify),
    reason: &str,
) {
    let Some(channel) = steer.as_mut() else { return };
    let inputs = channel.close();
    if !inputs.is_empty() {
        notify(TurnNotify::SteerRestored {
            inputs,
            reason: reason.to_string(),
        });
    }
}

/// 021 §2.3-2/6：接入一份已受理的运行中普通输入（完整批末调用）。
/// 图片在正式入史前读取准备（复用 020 核心、本轮能力快照与取消检查点）；
/// 失败＝SteerRestored 明确交还宿主恢复，不入史、不降级为纯文本。
/// 入史即确认提交：SteerIngested 恰一次回执（正文＋附件路径原值）。
fn ingest_steer_input(
    history: &mut Vec<HistoryItem>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    input: SteerInput,
    image_input: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    notify: &mut dyn FnMut(TurnNotify),
) {
    let payload = if input.image_paths.is_empty() {
        None
    } else {
        match crate::view_image::prepare_user_images(&input.image_paths, image_input, cancel) {
            Ok(data_urls) => Some(crate::prompt::UserImagePayload {
                data_urls,
                text_elements: input.text_elements.clone(),
            }),
            Err(reason) => {
                notify(TurnNotify::SteerRestored {
                    inputs: vec![input],
                    reason,
                });
                return;
            }
        }
    };
    let has_images = payload.as_ref().is_some_and(|p| !p.data_urls.is_empty());
    history.push(HistoryItem::Text(HistoryMessage {
        images: has_images.then(|| payload.as_ref().expect("已确认存在").data_urls.clone()),
        text_elements: payload
            .as_ref()
            .map(|p| p.text_elements.clone())
            .unwrap_or_default(),
        role: HistoryRole::User,
        content: input.text.clone(),
        user_input_event: None,
    }));
    on_record(history);
    notify(TurnNotify::SteerIngested { input });
}

/// 018 阶段 B 核心循环：在 [`run_tool_turn_with_host`] 之上接入异步发布、
/// 显式等待与批末回答接入。契约见 018 §8.3–8.5：
/// - 发布成功立即返回 `{"accepted":true}`；每轮同一时刻最多一项未决；
/// - 回答只在完整工具批配对之后、下一次请求组装之前入史；
/// - `wait_for_user_input` 必须是批内最后一条调用（入史/执行前整批拒绝）；
/// - 第 8 次请求不再发布新问题、不为未答问题开始阻塞等待；
/// - 结束/取消/额度耗尽路径原子处理已受理回答并关闭未决项。
/// 019：`image_input`＝本轮模型图片能力快照（用户轮开始时固定，声明与
/// view_image 执行分派共用；MemoryMerge 内部轮不声明也不执行 view_image）。
/// 021 兼容入口：无消息能力、无运行中接续通道。
pub fn run_tool_turn_with_hosts(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    procs: &ProcessManager,
    input_host: Option<&mut crate::user_input::InputHost<'_>>,
    async_host: Option<&mut crate::user_input_async::AsyncInputHost<'_>>,
    image_input: bool,
    user_images: Option<&crate::prompt::UserImagePayload>,
) -> Result<String, String> {
    run_tool_turn_with_hosts_extras(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        procs,
        input_host,
        async_host,
        image_input,
        user_images,
        TurnExtras::none(),
    )
}

/// 021 核心循环：在既有宿主参数之上接收 [`TurnExtras`]——
/// - `send_message`：send_message_to_user_async 声明/执行共用快照；成功即回
///   `{"accepted":true}`，消息经宿主 ToolEnd 投影，不结束当前轮；
/// - `steer`：运行中普通输入接续通道（§2.3）。收取输入与封闭当前轮在同一
///   接续状态上原子判定：完整工具批及结果配对（含异步答案）之后、下一请求
///   组装之前接入已受理输入；模型返回纯文本时 `seal` 先行——有受理输入则
///   保留正文继续，为空才原子封口结束；取消/失败/额度每个出口 `close` 恰
///   一次，未入史输入交还宿主恢复。
/// MemoryMerge 内部轮强制忽略两者（声明与执行双层隔离）。
#[allow(clippy::too_many_arguments)]
pub fn run_tool_turn_with_hosts_extras(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    procs: &ProcessManager,
    input_host: Option<&mut crate::user_input::InputHost<'_>>,
    async_host: Option<&mut crate::user_input_async::AsyncInputHost<'_>>,
    image_input: bool,
    user_images: Option<&crate::prompt::UserImagePayload>,
    extras: TurnExtras<'_>,
) -> Result<String, String> {
    let result = run_tool_turn_body(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        procs,
        input_host,
        async_host,
        image_input,
        user_images,
        extras,
    );
    // 031 §1.7-4：统一轮出口收尾——额外权限进程（含子孙）终止并回收；收尾
    // 失败＝Err 追加报告（不宣称已安全撤销，宿主据此拒绝开始下一轮）。基线
    // 权限进程不受影响（016 可续接保留）。所有出口（成功/取消/模型错误/
    // 额度耗尽/参数失败）都经此路径。
    if let Err(failure) = procs.reap_extra_permission_sessions() {
        return Err(format!("{failure}（本轮额外权限到期收尾失败）"));
    }
    result
}

/// [`run_tool_turn_with_hosts_extras`] 的原主体（私有）：全部轮出口最终回到
/// 公开包装的统一收尾门。035：本包装另承载多 Agent 组的创建与**轮出口
/// 统一封组**（成功/取消/模型错误/额度耗尽全部出口都经此——封组幂等；
/// 在途传输排空由子线程按自身门继续，占槽至真实释放）。
fn run_tool_turn_body(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    procs: &ProcessManager,
    input_host: Option<&mut crate::user_input::InputHost<'_>>,
    async_host: Option<&mut crate::user_input_async::AsyncInputHost<'_>>,
    image_input: bool,
    user_images: Option<&crate::prompt::UserImagePayload>,
    extras: TurnExtras<'_>,
) -> Result<String, String> {
    // 035：每根逻辑用户轮一个新 group_id（旧组路径不可执行、旧回调不能命中
    // 新组）；合并轮不装配（六工具双层隔离——不声明不执行）。
    let agent_runtime = extras
        .agents
        .filter(|_| !matches!(mode, ToolMode::MemoryMerge { .. }))
        .map(|a| a.manager.begin_turn(a.session.clone(), procs.clone()));
    let result = turn_body_inner(
        history,
        current_input,
        sender,
        notify,
        cancel,
        on_record,
        mode,
        procs,
        input_host,
        async_host,
        image_input,
        user_images,
        extras,
        agent_runtime.as_ref(),
    );
    if let Some(runtime) = &agent_runtime {
        let reason = match &result {
            Ok(_) => "turn_completed",
            Err(_) => "turn_exit_error",
        };
        runtime.close_for_turn_exit(history, on_record, reason);
    }
    result
}

/// 主体（私有）：`agent_runtime`＝本用户轮的 Agent 组（None＝能力关闭）。
#[allow(clippy::too_many_arguments)]
fn turn_body_inner(
    history: &mut Vec<HistoryItem>,
    current_input: &str,
    sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
    notify: &mut dyn FnMut(TurnNotify),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    mode: ToolMode<'_>,
    procs: &ProcessManager,
    mut input_host: Option<&mut crate::user_input::InputHost<'_>>,
    mut async_host: Option<&mut crate::user_input_async::AsyncInputHost<'_>>,
    image_input: bool,
    user_images: Option<&crate::prompt::UserImagePayload>,
    extras: TurnExtras<'_>,
    agent_runtime: Option<&std::sync::Arc<crate::agents::runtime::GroupRuntime>>,
) -> Result<String, String> {
    // 020 A05：空文字＋有图＝合法纯图消息；空文字＋无图仍拒绝
    let has_user_images = user_images.is_some_and(|p| !p.data_urls.is_empty());
    if current_input.trim().is_empty() && !has_user_images {
        return Err(BLANK_INPUT_ERROR.into());
    }
    let merge_mode = matches!(mode, ToolMode::MemoryMerge { .. });
    if merge_mode {
        input_host = None;
        async_host = None;
    }
    // 021：合并轮双层隔离——即使误传能力与通道也不声明、不接收、不执行；
    // 误传通道立即封口，已受理输入交还宿主恢复（不静默丢弃）
    let send_message = extras.send_message && !merge_mode;
    // 022：合并轮双层隔离——即使误传快照也不声明、不执行（与消息工具同源）。
    let mcp_snapshot: Option<&crate::mcp::manager::McpSnapshot> =
        if merge_mode { None } else { extras.mcp };
    let mut steer = if merge_mode {
        let late = extras.steer.map(|c| c.close()).unwrap_or_default();
        if !late.is_empty() {
            notify(TurnNotify::SteerRestored {
                inputs: late,
                reason: "merge_turn".to_string(),
            });
        }
        None
    } else {
        extras.steer
    };
    // 032：上下文工具宿主——合并轮双层隔离（误传也不声明、不执行）。
    let mut context_host = if merge_mode { None } else { extras.context };
    // 034：编排宿主——合并轮双层隔离（内部记忆轮始终不声明不执行 exec/wait）。
    let mut orch_extras = if merge_mode { None } else { extras.orchestration };
    // 031：轮内权限状态——宿主经 extras 传入（调用方已核对模型能力与交互
    // 前提），冻结策略未装配（后端不可用）时整体不生效；合并轮双层隔离。
    let mut permission_state: Option<crate::permissions::TurnPermState<'_>> =
        if merge_mode {
            None
        } else {
            match (extras.permissions, crate::permissions::frozen()) {
                (Some(p), Some(_)) => Some(crate::permissions::TurnPermState {
                    grants: crate::permissions::TurnGrants::new(),
                    host: p.host,
                }),
                _ => None,
            }
        };
    let permissions_declared = permission_state.is_some();
    // 取消状态不跨用户轮（006：每轮开始复位）
    if let Some(c) = cancel {
        c.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    // 032：本用户轮起点（seed 第 4 项的扫描基线——当前轮用户输入/接续输入/
    // 问询回答都在其后；此前的记录属归档前缀或上一轮）。
    let turn_user_start = history.len();
    history.push(HistoryItem::Text(HistoryMessage {
        // 020：准备后的用户附件随同一用户消息入史（纯图消息 content 可为标记/空）
        images: has_user_images.then(|| user_images.expect("已确认存在").data_urls.clone()),
        text_elements: has_user_images
            .then(|| user_images.expect("已确认存在").text_elements.clone())
            .unwrap_or_default(),
        role: HistoryRole::User,
        content: current_input.to_string(),
        user_input_event: None,
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
    let mut pending_async: Option<PendingAsyncInput> = None;
    // 034：活动编排 cell（同轮最多一个未消费终态句柄；所有轮出口统一收尾）。
    let mut orch_cell: Option<crate::js_orchestration::runtime::CellHandle> = None;
    // §1.5：每逻辑用户轮 128 叶子额度——跨同轮全部 cell 共享（审查 P1-2）。
    let turn_leaf_budget = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(
        crate::js_orchestration::runtime::TURN_LEAF_QUOTA,
    ));
    // 轮出口收尾：关闭受理、取消在途、回收 worker、记录 cell_closed 并入史。
    macro_rules! close_orch {
        () => {{
            if let (Some(cell), Some(orch)) = (orch_cell.as_mut(), orch_extras.as_mut()) {
                cell.close_for_turn_exit(orch.sink, history);
                push_orchestration_events(cell, history, on_record);
                orch_cell = None;
            }
        }};
    }
    // 同轮已入史答案的 request_id（§8.4）：后续批次再等待这些 ID 只确认
    // 就绪，不重问、不重复记录；未在此列的 ID 仍按无未决/不匹配拒绝。
    let mut recorded_answer_ids: Vec<String> = Vec::new();
    // 032 §1.5：切换候选与已提交标记。候选隔离于普通待写段（不在历史——
    // 失败不被自动补写/退出 flush/手动保存复活）；每用户轮最多一次成功提交。
    let mut pending_switch: Option<PendingSwitch> = None;
    let mut switch_committed = false;
    // 030：搜索模式——模型开关＋合并轮隔离（误传也不声明不执行）；快照
    // 存在可搜索动态工具才启用（S01：无可搜索项不声明，保留原全量路径）。
    // 目录总量检查（≤2048 项/10 MiB）在首次模型请求前完成：超限明确结束
    // 本轮，不先发半份目录、不静默切回全量声明（§1.6）。索引惰性建立、
    // 同轮拟合一次；轮结束整体释放（§1.4-7）。
    let mut search_state: Option<crate::tool_search::SearchState> = None;
    let mut tool_search_definition: Option<Value> = None;
    if extras.tool_search && !merge_mode {
        if let Some(snapshot) = mcp_snapshot {
            // 审查 #7：先借用快照条目做容量计量（瞬态序列化，超限即停），
            // 过门后才物化 owned 目录——不在门前整表克隆 Schema。
            if let Err(reason) = crate::tool_search::check_snapshot_capacity(snapshot) {
                close_steer_restore(
                    &mut steer,
                    notify,
                    crate::user_input_async::CLOSE_REQUEST_LIMIT,
                );
                close_pending_async(
                    history,
                    on_record,
                    notify,
                    &mut pending_async,
                    crate::user_input_async::CLOSE_REQUEST_LIMIT,
                );
                close_orch!();
                return Err(reason);
            }
            let entries: Vec<crate::tool_search::CatalogEntry> = snapshot
                .dynamic_entries()
                .iter()
                .filter_map(crate::tool_search::catalog_entry_from_tool)
                .collect();
            if !entries.is_empty() {
                tool_search_definition = Some(crate::tool_search::tool_definition(
                    &crate::tool_search::sources_json(&entries),
                ));
                search_state = Some(crate::tool_search::SearchState::new(entries));
            }
        }
    }
    loop {
        if requests >= MAX_MODEL_REQUESTS_PER_TURN {
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_REQUEST_LIMIT);
            close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_REQUEST_LIMIT);
            close_orch!();
            return Err(cap_error());
        }
        // 续请求使用完整历史（含本轮 user 与已有结果），不再次追加当前输入。
        // 032：活动视图（最近检查点 seed＋其后真实历史）统一进入 messages 投影
        // 与 _history；检查点语义损坏时明确失败，不静默回退全历史（§1.6/§2）。
        let active_items = match crate::context_window::active_view(&*history) {
            Ok(view) => view.items(),
            Err(reason) => {
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                close_orch!();
                return Err(reason);
            }
        };
        let empty_tools: Vec<Value> = Vec::new();
        // 030：搜索模式按累计加载拆分声明（固定资源工具＋已加载动态定义，
        // 按快照身份序）；关闭走原全量路径（逐字节兼容）。`declared_dynamic`
        // 为本次请求「已声明且可执行」的动态集合——只有它能授权本响应里的
        // 动态调用（§1.4-2；同批先搜索后猜名不升级）。
        let mut assembled_mcp: Vec<Value>;
        let declared_dynamic: std::collections::HashSet<String>;
        let mcp_definitions: &[Value] = match (&search_state, mcp_snapshot) {
            (Some(state), Some(snapshot)) => {
                assembled_mcp = snapshot.resource_definitions().to_vec();
                // 已加载定义来自搜索状态自身条目（敏感排除不改变对应关系）
                assembled_mcp.extend(state.loaded_definitions());
                declared_dynamic = state.loaded_names();
                &assembled_mcp
            }
            _ => {
                declared_dynamic = mcp_snapshot
                    .map(|s| {
                        s.dynamic_entries()
                            .iter()
                            .map(|e| e.model_name.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                mcp_snapshot
                    .map(|s| s.definitions())
                    .unwrap_or(&empty_tools)
            }
        };
        // 032：声明快照——查询按目录开关；切换还要求真实检查点提交宿主（§1.2）。
        let context_query = context_host
            .as_ref()
            .is_some_and(|h| h.query_declared);
        let context_switch = context_host
            .as_ref()
            .is_some_and(|h| h.switch_declared && h.checkpoints.is_some());
        let orch_declared = orch_extras.is_some();
        // 035：六工具声明与执行同门（extras 装配＝目录六名全配＋共享会话）。
        let agents_declared = agent_runtime.is_some();
        let mut body = match &mode {
            ToolMode::User => crate::prompt::build_turn_request_with_agents(
                &active_items,
                None,
                input_host.is_some(),
                async_host.is_some(),
                image_input,
                send_message,
                context_query,
                context_switch,
                permissions_declared,
                tool_search_definition.as_ref(),
                mcp_definitions,
                orch_declared,
                agents_declared,
            ),
            ToolMode::UserWithMemory { .. } => crate::prompt::build_turn_request_with_agents(
                &active_items,
                memory_context.as_deref(),
                input_host.is_some(),
                async_host.is_some(),
                image_input,
                send_message,
                context_query,
                context_switch,
                permissions_declared,
                tool_search_definition.as_ref(),
                mcp_definitions,
                orch_declared,
                agents_declared,
            ),
            ToolMode::MemoryMerge { system, .. } => build_turn_request_with(
                &active_items,
                &crate::memory::mem_tools::tool_definitions(),
                system,
            ),
        };
        // 024：共同历史作为宿主数据交给适配器；兼容的 messages 投影仍供旧发送者
        // 使用。此字段和原生状态绝不直接出现在厂商请求中。032：与 messages 同用
        // 活动视图（不能只在 Chat messages 隐藏旧内容而由原生适配器重发）。
        body["_history"] = serde_json::to_value(&active_items).expect("活动视图可序列化");
        // 022：工具总数上限（含本轮条件原生工具）在模型请求发送前按组装结果
        // 检查，超限明确报错、不静默截断。
        let declared_tools = body["tools"].as_array().map(|t| t.len()).unwrap_or(0);
        if let Some(snapshot) = mcp_snapshot {
            if declared_tools > crate::mcp::config::MAX_TOTAL_TOOLS {
                close_steer_restore(
                    &mut steer,
                    notify,
                    crate::user_input_async::CLOSE_REQUEST_LIMIT,
                );
                close_pending_async(
                    history,
                    on_record,
                    notify,
                    &mut pending_async,
                    crate::user_input_async::CLOSE_REQUEST_LIMIT,
                );
                close_orch!();
                return Err(format!(
                    "本轮工具总数 {declared_tools} 超过上限 {}（其中 MCP {} 项）：请缩小 mcp.json 的工具名单",
                    crate::mcp::config::MAX_TOTAL_TOOLS,
                    snapshot.len()
                ));
            }
        }
        let resp = match sender(&body) {
            Ok(resp) => resp,
            Err(error) => {
                // 018-B（§8.5）：模型请求失败＝明确结束本轮——先处理已受理回答
                // （通道终态与模型失败同轮发生时以模型失败文案收尾，通道侧无新事实）
                // 021：未入史的已受理接续输入交还宿主恢复
                let _ = drain_pending_async(&mut pending_async);
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                close_orch!();
                return Err(error);
            }
        };
        requests += 1;
        // 018-B 模型返回检查点：通道终态为失败＝明确结束本轮，不处理该响应
        if let Some(error) = drain_pending_async(&mut pending_async) {
            close_steer_restore(
                &mut steer,
                notify,
                crate::user_input_async::close_reason_for(&error),
            );
            let reason = crate::user_input_async::close_reason_for(&error);
            close_pending_async(history, on_record, notify, &mut pending_async, reason);
            close_orch!();
            return Err(error.message().into());
        }
        // 先整条校验消息与整批调用结构，再写入历史、执行；协议级失败
        // 收尾统一先保存已受理回答（§8.5），未答项才按 model_error 关闭
        let Some(message) = resp.get("assistant").or_else(|| resp.pointer("/choices/0/message")).cloned() else {
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
            close_pending_on_model_error(history, on_record, notify, &mut pending_async);
            close_orch!();
            return Err("协议错误：响应缺少 choices[0].message".to_string());
        };
        let native_state = match resp.get("native_state") {
            None => None,
            Some(value) => match serde_json::from_value::<crate::prompt::NativeState>(value.clone()) {
                Ok(state) => Some(state),
                Err(_) => {
                    close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                    close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                    close_orch!();
                    return Err("协议错误：原生续接状态无效".into());
                }
            },
        };
        let calls = match parse_tool_calls(&message) {
            Ok(calls) => calls,
            Err(error) => {
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                close_orch!();
                return Err(error);
            }
        };
        let content = match message.get("content") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                close_orch!();
                return Err("协议错误：assistant.content 必须是字符串或 null".into());
            }
        };
        // N3：批大小在入史/执行之前检查，超限整批拒绝（协议级失败，历史不污染）
        if calls.len() > MAX_TOOL_CALLS_PER_BATCH {
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
            close_pending_on_model_error(history, on_record, notify, &mut pending_async);
            close_orch!();
            return Err(format!(
                "单批工具调用 {} 条超过上限 {MAX_TOOL_CALLS_PER_BATCH}，整批拒绝",
                calls.len()
            ));
        }
        // 018-B（§8.4）：等待只允许出现在批内最后一条——任何非批末等待
        // （含 [wait, exec, wait]、[wait, wait]）都在入史/执行前整批拒绝，
        // 避免预先执行尚不知道答案的动作。
        if calls[..calls.len().saturating_sub(1)]
            .iter()
            .any(|c| c.name == "wait_for_user_input")
        {
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
            close_pending_on_model_error(history, on_record, notify, &mut pending_async);
            close_orch!();
            return Err(
                "协议错误：wait_for_user_input 必须是批内最后一条调用，整批拒绝".to_string(),
            );
        }
        // 032 §1.5：new_context 每批至多一次且必须是最后一条调用——违反时在
        // 入史/执行前整批拒绝（零副作用，同 wait_for_user_input 顺序限制）。
        if let Some(last) = calls.last() {
            let switch_count = calls
                .iter()
                .filter(|c| c.name == crate::context_window::SWITCH_TOOL_NAME)
                .count();
            if switch_count > 1
                || (switch_count == 1 && last.name != crate::context_window::SWITCH_TOOL_NAME)
            {
                close_steer_restore(
                    &mut steer,
                    notify,
                    crate::user_input_async::CLOSE_MODEL_ERROR,
                );
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                close_orch!();
                return Err(
                    "协议错误：new_context 必须是批内最后一条调用且每批至多一次，整批拒绝"
                        .to_string(),
                );
            }
        }
        // 035 §1.4：wait_agent 只允许出现在批内最后一条——任何非批末等待
        //（含 [wait_agent, spawn, wait_agent]、双 wait）都在入史/执行前整批
        // 拒绝，避免先执行依赖未知观察结果的后续动作（同 wait_for_user_input
        // 前缀切片写法，018-B 审计教训）。
        if agent_runtime.is_some()
            && calls[..calls.len().saturating_sub(1)]
                .iter()
                .any(|c| c.name == crate::agents::WAIT_TOOL_NAME)
        {
            close_orch!();
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
            close_pending_on_model_error(history, on_record, notify, &mut pending_async);
            return Err(
                "协议错误：wait_agent 必须是批内最后一条调用，整批拒绝".to_string(),
            );
        }
        // 034 §1.5：exec/wait 独占批＋活动 cell 门（入史/执行前整批校验）。
        if orch_extras.is_some() {
            let is_control = |name: &str| {
                name == crate::js_orchestration::EXEC_TOOL_NAME
                    || name == crate::js_orchestration::WAIT_TOOL_NAME
            };
            let control_count = calls.iter().filter(|c| is_control(&c.name)).count();
            if control_count > 0 && calls.len() > 1 {
                // 违规整批零执行（§2「先整批拒绝」）。
                close_orch!();
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                return Err(format!(
                    "协议错误：exec/wait 必须单独成批（本批 {} 条调用），整批拒绝",
                    calls.len()
                ));
            }
            if orch_cell.is_some() && control_count == 0 && !calls.is_empty() {
                // 活动 cell 时普通工具批整批拒绝并收尾（仅 wait 可推进，§1.5）。
                close_orch!();
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                return Err(
                    "协议错误：当前存在未消费的编排 cell，仅 wait 可以推进执行，其他批次整批拒绝"
                        .into(),
                );
            }
        }
        if calls.is_empty() {
            let reply = content.unwrap_or_default();
            if reply.trim().is_empty() {
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_MODEL_ERROR);
                close_pending_on_model_error(history, on_record, notify, &mut pending_async);
                close_orch!();
                return Err("模型回复为空".into());
            }
            // 018-B 结束检查：已受理回答先于终文——继续原轮让模型处理，
            // 不把该终文提前交付为任务完成（§8.4）。
            if let Some(p) = pending_async.as_ref().filter(|p| p.answer.is_some()) {
                history.push(with_native(HistoryItem::Text(HistoryMessage {
                    images: None,
                    text_elements: Vec::new(),
                    role: HistoryRole::Assistant,
                    content: reply.clone(),
                    user_input_event: None,
                }), &native_state));
                on_record(history);
                record_async_answer(history, on_record, notify, p);
                recorded_answer_ids.push(pending_async.take().unwrap().request_id);
                // 021：受理中的接续输入保持开放，随下一批末接入
                continue;
            }
            // 034 §1.4：无 wait 就返回终文——未取终态不得当任务成功：关闭 cell
            // 并记录收尾后返回轮错误（真实副作用不回滚）。
            if orch_cell.is_some() {
                let cell_id = orch_cell.as_ref().expect("已确认存在").id.clone();
                close_orch!();
                close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_TASK_COMPLETED);
                close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_TASK_COMPLETED);
                return Err(format!(
                    "模型已给出最终回复，但编排 cell {cell_id} 的结果尚未取回：cell 已按收尾关闭（已发生的真实副作用不回滚）；请在本轮内用 wait 取回结果后再结束"
                ));
            }
            // 035 §1.5 终文封口门：原子检查邮箱与活动任务——已有未交付报告先
            // 保留终文正文再交付（与接续输入同序：回复在前、新事实在后），
            // 根在剩余额度内再判断（无剩余额度时由循环顶的既有上限出口
            // 终止）；仍有活动子任务却直接结束＝关闭整组并返回未收尾错误，
            // 不把未完成子工作报告为完成；两者皆无才接受根终文。
            if let Some(runtime) = agent_runtime {
                if runtime.pending_updates() > 0 {
                    history.push(with_native(HistoryItem::Text(HistoryMessage {
                        images: None,
                        text_elements: Vec::new(),
                        role: HistoryRole::Assistant,
                        content: reply.clone(),
                        user_input_event: None,
                    }), &native_state));
                    on_record(history);
                    runtime.deliver_root_updates(history, on_record);
                    for event in runtime.take_committed_events() {
                        history.push(event);
                    }
                    on_record(history);
                    // 已交付事实随下一请求让模型再判断（额度耗尽由循环顶报告）。
                    continue;
                }
                for event in runtime.take_committed_events() {
                    history.push(event);
                }
                on_record(history);
                if runtime.has_active() {
                    runtime.close_for_turn_exit(
                        history,
                        on_record,
                        "root_final_with_active_children",
                    );
                    close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_TASK_COMPLETED);
                    close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_TASK_COMPLETED);
                    return Err(
                        "模型已给出最终回复，但仍有活动子 Agent 任务未收尾：整组已按收尾关闭（已发生事实与真实副作用保留，不回滚，未完成子工作不报告为完成）"
                            .into(),
                    );
                }
                // 审查 N1：终态发布可能恰在 pending 检查与 has_active 检查之间
                // 落入（两次独立加锁）——接受终文前复查一次，微窗口内已受理的
                // 终态报告不丢失。
                if runtime.pending_updates() > 0 {
                    history.push(with_native(HistoryItem::Text(HistoryMessage {
                        images: None,
                        text_elements: Vec::new(),
                        role: HistoryRole::Assistant,
                        content: reply.clone(),
                        user_input_event: None,
                    }), &native_state));
                    on_record(history);
                    runtime.deliver_root_updates(history, on_record);
                    for event in runtime.take_committed_events() {
                        history.push(event);
                    }
                    on_record(history);
                    continue;
                }
            }
            // 021 终文竞态（§2.3-5）：seal 原子判定——通道内已有受理输入则保留
            // 该正文继续当前轮（先正文入史，再接入输入），不提前宣告任务完成；
            // 为空才原子封口结束。封口后的迟到提交由宿主回执"当前轮已结束"。
            let late = match steer.as_mut() {
                Some(channel) => match channel.seal() {
                    SteerSeal::Inputs(inputs) => inputs,
                    SteerSeal::Closed => Vec::new(),
                },
                None => Vec::new(),
            };
            if !late.is_empty() {
                history.push(with_native(HistoryItem::Text(HistoryMessage {
                    images: None,
                    text_elements: Vec::new(),
                    role: HistoryRole::Assistant,
                    content: reply.clone(),
                    user_input_event: None,
                }), &native_state));
                on_record(history);
                for input in late {
                    ingest_steer_input(history, on_record, input, image_input, cancel, notify);
                }
                continue;
            }
            history.push(with_native(HistoryItem::Text(HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: HistoryRole::Assistant,
                content: reply.clone(),
                user_input_event: None,
            }), &native_state));
            on_record(history);
            close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_TASK_COMPLETED);
            close_orch!();
            return Ok(reply);
        }
        // 合法批：记录 assistant（content 原样保留），按返回顺序串行执行并逐条追加结果
        history.push(with_native(HistoryItem::AssistantToolCalls {
            tool_calls: calls
                .iter()
                .map(|c| ToolCallRecord {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect(),
            content,
        }, &native_state));
        on_record(history);
        let mut cancelled_batch = false;
        let mut input_end: Option<crate::user_input::InputError> = None;
        let mut answered_in_batch = false;
        for call in &calls {
            if input_end.is_some()
                || cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst))
            {
                // N2b：取消后剩余调用不执行，但每个已记录调用都要有匹配 tool 结果
                let (error, cancelled) = match &input_end {
                    Some(reason) if !matches!(reason, crate::user_input::InputError::Cancelled) => {
                        (format!("{}；本调用未执行", reason.message()), false)
                    }
                    _ => (SKIPPED_BY_CANCEL_MESSAGE.to_string(), true),
                };
                let content = skipped_without_execution(&call.name, &mode, &error, cancelled);
                notify(TurnNotify::ToolEnd {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    result: content.clone(),
                    display: None,
                    cancelled,
                });
                history.push(HistoryItem::ToolResult {
                    image: None,
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
            // 019：三元组＝（模型面结果 JSON，展示数据，可选图片载荷）；图片
            // 载荷只入史，不进 ToolEnd 通知（§3.3）。
            let (content, display, image) = if call.name == "request_user_input"
                && !matches!(mode, ToolMode::MemoryMerge { .. })
            {
                // 018-B（§8.2）：异步未决时的新同步问询返回占用错误，不暗中
                // 改成“等待旧异步题”的别名。
                let result = if pending_async.is_some() {
                    json!({"error":"已有未决异步问询：请先等待或完成原问题后再发起同步问询"}).to_string()
                } else {
                    match input_host.as_mut() {
                        None => json!({"error":"request_user_input 不可用：当前宿主没有回答接口"})
                            .to_string(),
                        Some(host) => match crate::user_input::parse_arguments(&call.arguments) {
                            Err(error) => json!({"error":error}).to_string(),
                            Ok(questions) => match host(call, &questions).and_then(|answer| {
                                crate::user_input::validate_response(&questions, &answer)?;
                                Ok(answer)
                            }) {
                                Ok(answer) => {
                                    answered_in_batch = true;
                                    serde_json::to_string(&answer).expect("问答结果可序列化")
                                }
                                Err(reason) => {
                                    let result = json!({"error":reason.message()}).to_string();
                                    input_end = Some(reason);
                                    result
                                }
                            },
                        },
                    }
                };
                (result, None, None)
            } else if call.name == "request_user_input_async"
                && !matches!(mode, ToolMode::MemoryMerge { .. })
            {
                let outcome = if async_host.is_none() {
                    Err("request_user_input_async 不可用：当前宿主没有异步回答接口".to_string())
                } else if requests == MAX_MODEL_REQUESTS_PER_TURN {
                    Err(format!(
                        "已达本轮模型请求上限（{MAX_MODEL_REQUESTS_PER_TURN}）：不再发布新的异步问询"
                    ))
                } else if pending_async.is_some() {
                    Err("已有未决问询：请先等待其回答或本轮结束，不替换旧题".to_string())
                } else {
                    match crate::user_input_async::parse_arguments(&call.arguments, &call.id) {
                        Err(error) => Err(error),
                        Ok(questions) => match async_host
                            .as_mut()
                            .expect("能力已确认")(call, &questions)
                        {
                            Err(error) => Err(format!("问询发布失败：{error}")),
                            Ok(channel) => {
                                pending_async = Some(PendingAsyncInput {
                                    request_id: call.id.clone(),
                                    questions,
                                    channel: Some(channel),
                                    answer: None,
                                });
                                Ok(crate::user_input_async::accepted_result(&call.id))
                            }
                        },
                    }
                };
                (
                    match outcome {
                        Ok(content) => content,
                        Err(error) => json!({"error":error}).to_string(),
                    },
                    None,
                    None,
                )
            } else if call.name == "wait_for_user_input"
                && !matches!(mode, ToolMode::MemoryMerge { .. })
            {
                let outcome = if async_host.is_none() {
                    Err("wait_for_user_input 不可用：当前宿主没有异步回答接口".to_string())
                } else if requests == MAX_MODEL_REQUESTS_PER_TURN {
                    Err(format!(
                        "已达本轮模型请求上限（{MAX_MODEL_REQUESTS_PER_TURN}）：不再为未答问题开始阻塞等待"
                    ))
                } else {
                    match crate::user_input_async::parse_wait_arguments(&call.arguments) {
                        Err(error) => Err(error),
                        // 018-B（§8.4）：同轮已入史答案的 ID 再等待只确认就绪，
                        // 不重问、不重复记录；未知/异轮 ID 不在此列，仍拒绝。
                        Ok(request_id) if recorded_answer_ids.contains(&request_id) => {
                            Ok(crate::user_input_async::wait_ready_result(&request_id))
                        }
                        Ok(request_id) => match pending_async.as_mut() {
                            None => Err("当前没有未决问询可等待".to_string()),
                            Some(p) if p.request_id != request_id => Err(
                                "request_id 与本轮未决问询不匹配，不能等待其他请求".to_string(),
                            ),
                            Some(p) => {
                                if p.answer.is_some() {
                                    Ok(crate::user_input_async::wait_ready_result(&request_id))
                                } else {
                                    // 已答即就绪；未答则停在同一调用等待（§8.4）：
                                    // 复用阶段 A 的可取消等待，不轮询模型、不重复请求。
                                    let channel = p
                                        .channel
                                        .take()
                                        .expect("未收到终态前通道必在");
                                    notify(TurnNotify::UserInputAsyncWait {
                                        request_id: request_id.clone(),
                                    });
                                    let no_cancel = std::sync::atomic::AtomicBool::new(false);
                                    let cancel_flag = cancel.unwrap_or(&no_cancel);
                                    match crate::user_input::wait_for_response(
                                        channel.receiver,
                                        cancel_flag,
                                        &channel.live,
                                    ) {
                                        Ok(response) => {
                                            match crate::user_input::validate_response(
                                                &p.questions,
                                                &response,
                                            ) {
                                                Ok(()) => {
                                                    p.answer = Some(response);
                                                    Ok(crate::user_input_async::wait_ready_result(&request_id))
                                                }
                                                Err(reason) => {
                                                    input_end = Some(reason.clone());
                                                    Err(reason.message().to_string())
                                                }
                                            }
                                        }
                                        Err(reason) => {
                                            input_end = Some(reason.clone());
                                            Err(reason.message().to_string())
                                        }
                                    }
                                }
                            }
                        },
                    }
                };
                (
                    match outcome {
                        Ok(content) => content,
                        Err(error) => json!({"error":error}).to_string(),
                    },
                    None,
                    None,
                )
            } else if call.name == "view_image"
                && !matches!(mode, ToolMode::MemoryMerge { .. })
            {
                // 019：图片能力快照决定执行分派——能力关闭（text/未知）时
                // 返回能力错误且不读取文件；MemoryMerge 不执行任何用户工具。
                // 031：动态策略启用时经出生快照的沙箱内部入口读取（主进程
                // 不代读模型路径，§1.6 接入面）。
                let outcome = if image_input {
                    let cwd =
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                    // 031 出生快照（审查 #1：与 execute_tool 同源——以冻结策略
                    // 为键，无宿主同样执行基线快照）
                    let snap = crate::permissions::frozen().map(|f| {
                        let grants = permission_state
                            .as_ref()
                            .map(|st| st.grants.clone())
                            .unwrap_or_default();
                        crate::permissions::build_snapshot(&f.policy, &grants, &f.deny_ctx)
                    });
                    match snap {
                        Some(Ok(snap)) => crate::view_image::execute_sandboxed(
                            &call.arguments,
                            &cwd,
                            cancel,
                            &snap,
                        ),
                        Some(Err(e)) => {
                            let o = crate::view_image::error_outcome(&format!(
                                "生效策略无法编码：{e}（本次读取拒绝）"
                            ));
                            o
                        }
                        None => crate::view_image::execute(&call.arguments, &cwd, cancel),
                    }
                } else {
                    crate::view_image::unsupported_outcome()
                };
                (outcome.summary, None, outcome.image)
            } else if (call.name == crate::js_orchestration::EXEC_TOOL_NAME
                || call.name == crate::js_orchestration::WAIT_TOOL_NAME)
                && !merge_mode
            {
                // 034：exec/wait（声明与执行同门；未启用时落入未知工具通用错误）。
                let content = match orch_extras.as_mut() {
                    Some(orch) => execute_orchestration_call(
                        call,
                        &mut orch_cell,
                        orch,
                        &body,
                        &declared_dynamic,
                        permission_state.as_ref().map(|st| st.grants.clone()),
                        cancel,
                        procs,
                        history,
                        on_record,
                    ),
                    None => orchestration_handle_error(
                        None,
                        "TOOL_NOT_AVAILABLE",
                        format!(
                            "{} 不可用：当前模型未同时声明 exec 与 wait（experimental_supported_tools）",
                            call.name
                        ),
                    ),
                };
                (content, None, None)
            } else if call.name == "send_message_to_user_async" && !merge_mode {
                // 021：能力快照决定执行分派——关闭态（含伪造调用）返回错误且
                // 不产生消息；成功严格为 {"accepted":true}，消息由宿主经 ToolEnd
                // 投影（参数原文保留，展示副本由宿主过滤）。
                let outcome = if send_message {
                    crate::message_to_user::parse_arguments(&call.arguments)
                        .map(|_| crate::message_to_user::accepted_result())
                } else {
                    Err(format!(
                        "{} 不可用：当前模型未声明支持（experimental_supported_tools）",
                        crate::message_to_user::TOOL_NAME
                    ))
                };
                (
                    match outcome {
                        Ok(content) => content,
                        Err(error) => json!({ "error": error }).to_string(),
                    },
                    None,
                    None,
                )
            } else if call.name == crate::context_window::QUERY_TOOL_NAME && !merge_mode {
                // 032：只读计量查询——零 HTTP/零工具/零文件加载；结果按固定字段
                // 结构回传（未知≠失败，error=null）。声明与执行共用宿主快照；
                // 未声明或无宿主的伪造调用返回不可用错误，计量含当前批声明与
                // 此前已保存结果的快照（§1.3），查询本身照常入史。
                let outcome = match context_host.as_mut() {
                    Some(host) if host.query_declared => {
                        let invalid = crate::context_window::parse_query_arguments(&call.arguments)
                            .err()
                            .map(|error| json!({ "error": error }));
                        invalid.unwrap_or_else(|| {
                            crate::context_window::execute_query(
                                &crate::context_window::QueryInput {
                                    conn: host.conn,
                                    key: host.key,
                                    model: host.model,
                                    in_flight_request: &body,
                                    history,
                                },
                            )
                        })
                    }
                    _ => json!({
                        "error": format!(
                            "{} 不可用：当前模型未声明支持（experimental_supported_tools）或当前轮没有模型连接快照",
                            crate::context_window::QUERY_TOOL_NAME
                        )
                    }),
                };
                (outcome.to_string(), None, None)
            } else if call.name == crate::context_window::SWITCH_TOOL_NAME && !merge_mode {
                // 032 §1.5：new_context——先校验摘要、状态、候选视图和预算；
                // 通过即排定候选（scheduled＝等待批末提交，绝不提前 applied）。
                // 失败为 rejected＋原因；无宿主/未声明/重复/未决问询逐项失败。
                let outcome: Result<Value, String> = (|| {
                    let host = context_host.as_mut().ok_or_else(|| {
                        format!(
                            "{} 不可用：当前轮没有上下文管理宿主",
                            crate::context_window::SWITCH_TOOL_NAME
                        )
                    })?;
                    if !(host.switch_declared && host.checkpoints.is_some()) {
                        close_orch!();
                        return Err(format!(
                            "{} 不可用：当前模型未声明支持（experimental_supported_tools）或没有检查点提交宿主",
                            crate::context_window::SWITCH_TOOL_NAME
                        ));
                    }
                    if switch_committed {
                        close_orch!();
                        return Err("本用户轮已成功切换过一次上下文，再次请求拒绝".into());
                    }
                    if pending_switch.is_some() {
                        close_orch!();
                        return Err("本批已排定一个上下文切换候选".into());
                    }
                    // 未答异步问询阻止切换（已答未入史的会在批末入史并进入候选）
                    if pending_async.as_ref().is_some_and(|p| p.answer.is_none()) {
                        close_orch!();
                        return Err("存在未答异步问询：先等待或完成其回答后再切换上下文".into());
                    }
                    let summary =
                        crate::context_window::parse_switch_arguments(&call.arguments)?;
                    // 候选视图初步校验（编码/限额/严格缩小/预算；seed 按调用
                    // 时点构造——批末提交时重建并复验）。单一入口与批末复验
                    // 共用同一序列（审查修复 #15）。
                    crate::context_window::evaluate_candidate(
                        host.conn,
                        host.key,
                        host.model,
                        &body,
                        history,
                        turn_user_start,
                        &call.id,
                        &summary,
                        procs,
                    )?;
                    pending_switch = Some(PendingSwitch {
                        request_call_id: call.id.clone(),
                        summary,
                    });
                    Ok(json!({"ok":true,"status":"scheduled","error":null}))
                })();
                let result =
                    outcome.unwrap_or_else(|error| json!({"ok":false,"status":"rejected","error":error}));
                (result.to_string(), None, None)
            } else if call.name == crate::tool_search::TOOL_NAME && !merge_mode {
                // 030：本地目录搜索——零网络/零 tools/call；结果三字段结构。
                // 声明与执行共用本轮快照（未声明或无可搜索目录的伪造调用按
                // 不可用失败）；一次搜索全部成功或全部失败，此前成功加载不
                // 回滚；结果提交前检查取消（§1.5）。
                let native_declared = declared_tools.saturating_sub(declared_dynamic.len());
                let outcome = match search_state.as_mut() {
                    Some(state) if tool_search_definition.is_some() => {
                        match crate::tool_search::parse_arguments(&call.arguments) {
                            Err(reason) => crate::tool_search::failure_result(&reason),
                            Ok(parsed) => state.execute_search(
                                &parsed.query,
                                parsed.limit,
                                native_declared,
                                cancel,
                            ),
                        }
                    }
                    _ => crate::tool_search::failure_result(&format!(
                        "{} 不可用：当前模型未声明支持（experimental_supported_tools）或本轮没有可搜索工具",
                        crate::tool_search::TOOL_NAME
                    )),
                };
                (outcome.to_string(), None, None)
            } else if mcp_snapshot.is_some_and(|s| s.has_resources())
                && crate::mcp::resources::is_resource_tool(&call.name)
            {
                // 023：固定资源工具分派——快照资源能力决定（合并轮已在上文
                // 置 None 隔离）；参数校验、结果投影、污染与预算检查在快照
                // 资源调用内完成。
                let no_cancel = std::sync::atomic::AtomicBool::new(false);
                let cancel_flag = cancel.unwrap_or(&no_cancel);
                let (content, _failed) = mcp_snapshot
                    .expect("已确认存在")
                    .call_resource_tool(&call.name, &call.arguments, cancel_flag);
                (content, None, None)
            } else if let Some(entry) = mcp_snapshot
                .and_then(|s| s.entry(&call.name))
                .filter(|entry| declared_dynamic.contains(&entry.model_name))
            {
                // 022：MCP 分派——按快照精确命中（原始身份映射；不拆 `__`、
                // 不看前缀）。030：还须在**本次请求**的已声明动态集合内
                //（§1.4-2）——同批先搜索后猜名、未搜索猜名、其他轮名称即使
                // 目录命中也零调用，落入下方未知工具通用错误。参数错误在发送
                // 前返回；结果投影与凭证污染检查在快照调用内完成。
                let no_cancel = std::sync::atomic::AtomicBool::new(false);
                let cancel_flag = cancel.unwrap_or(&no_cancel);
                let content = match serde_json::from_str::<Value>(&call.arguments) {
                    Err(e) => crate::mcp::call::client_error_json(
                        "参数",
                        &format!("arguments 不是有效 JSON：{e}"),
                    ),
                    Ok(v) if !v.is_object() => crate::mcp::call::client_error_json(
                        "参数",
                        "arguments 必须是 JSON 对象",
                    ),
                    Ok(v) => {
                        let (content, _failed) = mcp_snapshot
                            .expect("已确认命中")
                            .call(entry, v, cancel_flag);
                        content
                    }
                };
                (content, None, None)
            } else if agent_runtime.is_some()
                && crate::agents::runtime::is_agent_tool(&call.name)
            {
                // 035：六控制工具（声明与执行同门；未装配时伪造调用落入未知
                // 工具通用错误）。spawn 上下文按当前请求作用域冻结：活动前缀
                // （fork 源）、已声明动态集合、MCP 快照、图片能力、可信
                // system（含根轮记忆摘要）与冻结环境上下文。
                let content = {
                    let runtime = agent_runtime.as_ref().expect("已确认存在");
                    let agents_extras = extras.agents.expect("运行时存在则 extras 装配");
                    let mut system = crate::prompt::sent_instructions();
                    crate::prompt::append_memory_context(&mut system, memory_context.as_deref());
                    let spawn_ctx = crate::agents::runtime::SpawnContext {
                        active_items: &active_items,
                        context_id: crate::context_window::current_context_id(history),
                        preamble: agents_extras.context_message.clone(),
                        system,
                        view_image: image_input,
                        mcp: agents_extras.mcp.clone(),
                        declared_dynamic: declared_dynamic.clone(),
                        sender_factory: agents_extras.sender_factory,
                    };
                    let steer_hint = |steer: &Option<&mut dyn SteerChannel>| {
                        steer.as_ref().is_some_and(|c| c.pending_hint())
                    };
                    crate::agents::runtime::execute_control_tool(
                        runtime,
                        &call.name,
                        &call.arguments,
                        history,
                        &spawn_ctx,
                        &|| steer_hint(&steer),
                    )
                };
                (content, None, None)
            } else {
                let (content, display) =
                    execute_tool(call, cancel, &mode, procs, permission_state.as_mut());
                (content, display, None)
            };
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
                image,
            });
            on_record(history);
            // 018-B 工具完成检查点：收取回执只暂存（入史固定在批末）
            if let Some(error) = drain_pending_async(&mut pending_async) {
                input_end = input_end.or(Some(error));
            }
        }
        // 018-B 批末检查点（§8.4）：完整批配对之后、下一次请求组装之前——
        // 已受理回答恰一次入史并释放单项占用。
        if let Some(error) = drain_pending_async(&mut pending_async) {
            input_end = input_end.or(Some(error));
        }
        let mut async_answer_recorded = false;
        if let Some(p) = pending_async.as_ref().filter(|p| p.answer.is_some()) {
            record_async_answer(history, on_record, notify, p);
            async_answer_recorded = true;
        }
        if async_answer_recorded {
            recorded_answer_ids.push(pending_async.take().unwrap().request_id);
        }
        if let Some(reason) = input_end {
            close_steer_restore(
                &mut steer,
                notify,
                crate::user_input_async::close_reason_for(&reason),
            );
            let close_reason = crate::user_input_async::close_reason_for(&reason);
            close_pending_async(history, on_record, notify, &mut pending_async, close_reason);
            close_orch!();
            return Err(reason.message().into());
        }
        if cancelled_batch {
            // 取消：结束本轮，保留已发生结果与历史，不自动请求模型、不重复已执行工具
            // 032：取消已赢——排定的切换候选随轮丢弃（未提交不生效）
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_CANCELLED);
            close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_CANCELLED);
            close_orch!();
            return Err("用户已取消本轮工具执行：已执行结果保留，剩余调用未执行".into());
        }
        if (answered_in_batch || async_answer_recorded)
            && cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst))
        {
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_CANCELLED);
            close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_CANCELLED);
            close_orch!();
            return Err("用户已取消本轮：已保存问询结果，不再请求模型".into());
        }
        // 021 §2.3-4（既有语义；032 屏障前移判定）：完整工具批及结果配对（含
        // 异步答案）之后、下一请求组装之前接入已受理普通输入。第 8 次请求的
        // 额度终止出口不接入新输入（保持既有行为：交还宿主恢复）。
        let quota_hit = requests == MAX_MODEL_REQUESTS_PER_TURN;
        if !quota_hit {
            let pending_inputs = match steer.as_mut() {
                Some(channel) => channel.drain(),
                None => Vec::new(),
            };
            for input in pending_inputs {
                ingest_steer_input(history, on_record, input, image_input, cancel, notify);
            }
        }
        // 035 §1.4：同一外层批末持久化交付 Agent 已收事实——消息/终态报告
        // 投影为带来源标注的普通 user 输入（下一请求可见，重复 wait 不重复
        // 插入）；已提交控制事件推入内存历史；宿主通知转发。
        if let Some(runtime) = agent_runtime {
            runtime.deliver_root_updates(history, on_record);
            for event in runtime.take_committed_events() {
                history.push(event);
            }
            on_record(history);
            for line in runtime.take_notices() {
                notify(TurnNotify::Info(line));
            }
        }
        // 032 §1.5 批末屏障（提交与取消按先后裁决）：回执前取消→候选不提交；
        // 已入史输入（含上方批末接入的接续输入与已答问询）进入候选。
        if pending_switch.is_some()
            && cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst))
        {
            pending_switch = None;
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_CANCELLED);
            close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_CANCELLED);
            close_orch!();
            return Err("用户已取消本轮：已排定的上下文切换未提交，不生效".into());
        }
        if let Some(candidate) = pending_switch.take() {
            // 提交时点重建 seed（纳入批末新入史输入）并复验；仅当检查点保存
            // 成功并收到宿主回执后才切换活动视图。第 8 次响应的合法切换也在
            // 批末保存，随后因额度终止（不发第 9 次请求，§1.5）。
            let applied = context_host.as_mut().map(|host| {
                commit_context_switch(history, on_record, procs, &body, turn_user_start, &candidate, host)
            });
            match applied {
                Some(Ok(())) => {
                    switch_committed = true;
                    // 审查 #16（§1.7-4）：成功的会话切换是轮边界出口——先终止
                    // 并回收额外权限进程再继续（带授权进程不得进入新上下文）；
                    // 回收失败按切换失败处理（保留原窗口语义之外再报告收尾失败）。
                    if let Err(failure) = procs.reap_extra_permission_sessions() {
                        history.push(HistoryItem::Text(HistoryMessage {
                            images: None,
                            text_elements: Vec::new(),
                            role: HistoryRole::User,
                            content: format!(
                                "[宿主通知，非用户输入] 上下文切换后额外权限收尾失败：{failure}"
                            ),
                            user_input_event: None,
                        }));
                        on_record(history);
                        notify(TurnNotify::Info(format!(
                            "上下文切换后额外权限收尾失败：{failure}"
                        )));
                    }
                    notify(TurnNotify::Info(format!(
                        "上下文检查点已提交（代数 {}）：旧前缀已归档，活动上下文自交接摘要继续",
                        crate::context_window::current_context_id(history)
                    )));
                }
                Some(Err(reason)) => {
                    // 032 审查修复 #5：失败对模型可见——scheduled 之后模型须能
                    // 区分已应用与未应用；带固定宿主标识的记录入史（下一请求
                    // 可见），提示可用 get_context_remaining 核对 context_id。
                    history.push(HistoryItem::Text(HistoryMessage {
                        images: None,
                        text_elements: Vec::new(),
                        role: HistoryRole::User,
                        content: format!(
                            "[宿主通知，非用户输入] 上下文切换未应用（保留原窗口）：{reason}。可用 get_context_remaining 核对 context_id 是否递增。"
                        ),
                        user_input_event: None,
                    }));
                    on_record(history);
                    notify(TurnNotify::Info(format!(
                        "上下文切换未应用（保留原窗口）：{reason}"
                    )));
                }
                None => {
                    notify(TurnNotify::Info(
                        "上下文切换未应用（保留原窗口）：没有可用的上下文管理宿主".into(),
                    ));
                }
            }
        }
        if quota_hit {
            // 第 8 批：保存全部结果后报上限，不发第 9 次请求，不生成假回复
            close_steer_restore(&mut steer, notify, crate::user_input_async::CLOSE_REQUEST_LIMIT);
            close_pending_async(history, on_record, notify, &mut pending_async, crate::user_input_async::CLOSE_REQUEST_LIMIT);
            close_orch!();
            return Err(cap_error());
        }
    }
}

fn cap_error() -> String {
    format!("已达本轮模型请求上限（{MAX_MODEL_REQUESTS_PER_TURN}）")
}

/// 032 §1.5：已排定的上下文切换候选（调用时点校验通过＝scheduled）。候选只
/// 存于轮内局部状态、隔离于普通待写段（不进历史——失败不被自动补写/退出
/// flush/手动保存复活，调研 M3 裁决）；批末屏障重建 seed 并复验后提交。
struct PendingSwitch {
    request_call_id: String,
    summary: String,
}

/// 032 §1.5 批末屏障提交：提交时点重建 seed（批末入史输入——异步回答/接续
/// 输入——纳入候选）并复验候选（晚到输入可能使候选超限/不再缩小）；仅当
/// 检查点保存成功并收到宿主回执后才把检查点推入内存历史（此后活动视图＝
/// seed＋其后真实历史；会话已在提交内落盘，on_record 无新增可写）。
/// Err＝未应用原因（旧视图保留，候选不复活）。
#[allow(clippy::too_many_arguments)]
fn commit_context_switch(
    history: &mut Vec<HistoryItem>,
    on_record: &mut dyn FnMut(&[HistoryItem]),
    procs: &ProcessManager,
    in_flight_request: &Value,
    turn_user_start: usize,
    candidate: &PendingSwitch,
    host: &mut crate::context_window::ContextHost<'_>,
) -> Result<(), String> {
    // 提交时点重建 seed（批末入史输入——异步回答/接续输入——纳入候选）并
    // 复验候选；与排定点共用 evaluate_candidate 单一入口（审查修复 #15）。
    let checkpoint = crate::context_window::evaluate_candidate(
        host.conn,
        host.key,
        host.model,
        in_flight_request,
        history,
        turn_user_start,
        &candidate.request_call_id,
        &candidate.summary,
        procs,
    )?;
    let sink = host
        .checkpoints
        .as_mut()
        .ok_or("没有可用的检查点提交宿主")?;
    sink.commit_checkpoint(history, &checkpoint)?;
    // 回执成功：切换活动视图（§1.5——scheduled/applied 由是否存在已提交
    // 检查点区分；此处为唯一 applied 路径）
    history.push(checkpoint);
    on_record(history);
    Ok(())
}


/// 034 C13 测试入口：嵌套分派（与 cell 叶子执行同一条路径）。
#[doc(hidden)]
pub fn turn_dispatch_for_test(
    procs: &ProcessManager,
    tool: &str,
    args: &Value,
    cancel: &std::sync::atomic::AtomicBool,
) -> crate::js_orchestration::runtime::LeafOutcome {
    dispatch_nested_leaf(
        procs,
        None,
        &std::collections::HashSet::new(),
        &None,
        tool,
        args,
        cancel,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试包装：为 [`run_tool_turn`] 提供独立进程会话管理器（016 签名新增参数）。
    fn run_turn(
        history: &mut Vec<HistoryItem>,
        current_input: &str,
        sender: &mut dyn FnMut(&Value) -> Result<Value, String>,
        notify: &mut dyn FnMut(TurnNotify),
        cancel: Option<&std::sync::atomic::AtomicBool>,
        on_record: &mut dyn FnMut(&[HistoryItem]),
    ) -> Result<String, String> {
        let mgr = ProcessManager::new();
        run_tool_turn(history, current_input, sender, notify, cancel, on_record, &mgr)
    }

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
        run_turn(
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

    /// 017：update_plan 分派——合法参数返回两字段成功结果（display None，
    /// 不新增持久化展示变体）；参数错误按同一两字段结构回传原因，不升级为
    /// 协议错误；内部记忆合并轮按未知工具拒绝（权限在分派层，跨模式工具名不执行）。
    #[test]
    fn update_plan_dispatch_shapes_and_memory_merge_isolation() {
        let ok_args = r#"{"explanation":"开始","plan":[{"step":"接入","status":"in_progress"}]}"#;
        let r = execute_tool(
            &ParsedToolCall {
                id: "p1".into(),
                name: "update_plan".into(),
                arguments: ok_args.into(),
            },
            None,
            &ToolMode::User,
            &ProcessManager::new(),
            None,
        );
        assert_eq!(r.0, r#"{"ok":true,"error":null}"#);
        assert!(r.1.is_none(), "017：计划结果无展示数据（display 保持 None）");
        // 参数错误（非法状态）：两字段失败＋具体原因
        let r = execute_tool(
            &ParsedToolCall {
                id: "p2".into(),
                name: "update_plan".into(),
                arguments: r#"{"plan":[{"step":"s","status":"blocked"}]}"#.into(),
            },
            None,
            &ToolMode::User,
            &ProcessManager::new(),
            None,
        );
        let v: Value = serde_json::from_str(&r.0).unwrap();
        assert_eq!(v["ok"], json!(false), "{v}");
        assert!(
            v["error"].as_str().unwrap().contains("blocked"),
            "失败原因含非法状态原文：{v}"
        );
        // 内部记忆合并轮：分派层拒绝（mem 注册表外一律未知工具）
        let root = std::path::Path::new("/nonexistent-mem-root-017");
        let r = execute_tool(
            &ParsedToolCall {
                id: "p3".into(),
                name: "update_plan".into(),
                arguments: ok_args.into(),
            },
            None,
            &ToolMode::MemoryMerge {
                root,
                system: "",
                expected_epoch: 0,
            },
            &ProcessManager::new(),
            None,
        );
        let v: Value = serde_json::from_str(&r.0).unwrap();
        assert!(
            v["error"].as_str().unwrap().contains("未知工具 update_plan"),
            "合并轮不得执行计划工具：{v}"
        );
    }

    /// 017：取消后跳过的 update_plan 调用＝相同两字段失败结构，错误沿用
    /// SKIPPED_BY_CANCEL_MESSAGE；该结果不得恢复为成功快照。
    #[test]
    fn update_plan_cancelled_skip_is_two_field_failure() {
        let shape = skipped_without_execution(
            "update_plan",
            &ToolMode::User,
            SKIPPED_BY_CANCEL_MESSAGE,
            true,
        );
        assert_eq!(
            serde_json::from_str::<Value>(&shape).unwrap(),
            json!({ "ok": false, "error": SKIPPED_BY_CANCEL_MESSAGE }),
            "取消跳过＝两字段失败结构"
        );
        let args = r#"{"plan":[{"step":"s","status":"pending"}]}"#;
        assert!(
            crate::plan::success_snapshot(args, &shape).is_none(),
            "取消跳过不得产生成功快照"
        );
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
            &ProcessManager::new(),
            None,
        );
        let v: Value = serde_json::from_str(&r.0).unwrap();
        assert!(
            v["error"].as_str().unwrap().contains("未知工具 web_search"),
            "{v}"
        );
        assert!(
            v["error"].as_str().unwrap().contains("update_plan"),
            "未知工具提示应列出 update_plan：{v}"
        );
        let r = execute_tool(
            &ParsedToolCall {
                id: "b1".into(),
                name: "exec_command".into(),
                arguments: "not-json{".into(),
            },
            None,
            &ToolMode::User,
            &ProcessManager::new(),
            None,
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
            &ProcessManager::new(),
            None,
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
        let err = run_turn(
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
            HistoryItem::Text(HistoryMessage { role: HistoryRole::User, .. })
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
                    images: None,
                    text_elements: Vec::new(),
                    role: HistoryRole::User,
                    content: "u".into(),
                    user_input_event: None,
                }
                .into(),
            );
            let err = run_turn(
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
        let reply = run_turn(
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
        let err = run_turn(
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
                    HistoryItem::Text(HistoryMessage { role: HistoryRole::User, .. })
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
        let reply = run_turn(
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
        let reply = run_turn(
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
        let reply = run_turn(
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
        // 034 实测（2026-10-29）：本用例经 execute_core_tool 真实执行 apply_patch，
        // 与 sandbox/tests.rs 的 TCHAT_SANDBOX 写者并行时会撞进其设置窗口（读者
        // 被拉入沙箱分支）——按 033「覆盖缝读者与写者同时串行」教训持同一锁。
        let _env_guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let err = run_turn(
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
                "error": "用户取消，本调用未执行", "session_id": null }),
            "exec 未执行＝cancelled 形态（016 六字段，空句柄）：{v1}"
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
        let reply = run_turn(
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

    /// 031 P01：无权限状态（未启用 031/无宿主）时，模型伪造 request_permissions
    /// 调用＝未知工具契约拒绝——分派层即权限边界，不启动确认界面或后端。
    #[test]
    fn request_permissions_without_state_is_unknown_tool() {
        let r = execute_tool(
            &ParsedToolCall {
                id: "pp1".into(),
                name: "request_permissions".into(),
                arguments: r#"{"permissions":{"network":{"enabled":true}}}"#.into(),
            },
            None,
            &ToolMode::User,
            &ProcessManager::new(),
            None,
        );
        let v: Value = serde_json::from_str(&r.0).unwrap();
        assert!(
            v["error"].as_str().unwrap().contains("未启用权限申请"),
            "伪造调用应按未知工具契约拒绝：{v}"
        );
    }
}
