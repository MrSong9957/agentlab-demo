//! 035 宿主侧运行时：组生命周期、六工具执行、子轮循环与事件提交。
//!
//! 调度模型（§1.4/§1.7「本地调度适配」）：每个根逻辑用户轮一个 group
//!（[`AgentManager::begin_turn`]）；子轮在独立线程持有独立上下文
//!（`Arc<Mutex<Vec<HistoryItem>>>`，followup 复用）运行真实工具循环。
//! 事件的「调度泵」以**writer 互斥串行提交**实现：子线程与根线程都经
//! `writer 锁 → session 锁` 的单一锁链提交（`commit_agent_event_pump`/
//! `_root`），根阻塞于模型请求/wait/自身工具期间子事件持续落盘（根的
//! session 使用只在批末瞬时持锁），seq 在 writer 域内分配＝文件顺序，
//! 全组始终只有一个追加写者与一份权威 V3 文件。锁纪律：`core` 锁与
//! `writer` 锁**绝不同时持有**；`writer` 内只嵌套 session 锁（单向）。
//!
//! 进程归属（§1.5）：子 Agent 的 exec/write_stdin 按 (group, agent,
//! assignment) 登记（[`crate::proc_session::OwnerTag`]）；assignment 结束/
//! 中断先按宿主 PGID 回收自己的进程再发布终态；组关闭回收整组。兄弟猜
//! 句柄按 owner 不匹配拒绝；全局 64 进程容量不变。
//!
//! 预算（§1.6）：子请求 8/assignment、24/组（根 8 独立预留）；子工具调用
//! 64/assignment、256/组；消息 128 条/1 MiB 每组；事件负载 64 MiB/组
//!（响应/结果动作前预留 10 MiB/2 MiB，提交后按真实大小计费并释放）；
//! assignment 墙钟 300 秒（单调时钟；到点关动作门，不承诺网络瞬间结束，
//! 在途传输排空期间继续占槽）。

use super::args::{
    parse_list, parse_spawn, parse_target, parse_target_message, parse_target_path, parse_wait,
    ForkTurns, ListArgs, SpawnArgs, TargetArgs, TargetMessageArgs, WaitArgs,
};
use super::events::{agent_event, event_bytes, event_size_ok, terminal_report};
use super::mailbox::QueuedMessage;
use super::registry::{AgentState, GroupCore};
use super::{
    err_result, ok_result, FOLLOWUP_TOOL_NAME, INTERRUPT_TOOL_NAME, LIST_TOOL_NAME,
    MAX_CHILD_SLOTS, MAX_MESSAGE_BYTES, ROOT_PATH, SEND_TOOL_NAME, SPAWN_TOOL_NAME,
    WAIT_TOOL_NAME,
};
use crate::proc_session::{OwnerTag, ProcessManager};
use crate::prompt::{
    AgentEventKind, AgentEventPayload, AgentReport, HistoryItem, HistoryMessage, HistoryRole,
    ToolCallRecord,
};
use crate::session::Session;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 全应用子执行槽（§1.6：3 个；starting/running/stopping 与在途排空均计，
/// 新轮准入仍计未排空槽）。
static GLOBAL_CHILD_SLOTS: AtomicUsize = AtomicUsize::new(0);

fn try_reserve_slot() -> bool {
    loop {
        let cur = GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst);
        if cur >= MAX_CHILD_SLOTS {
            return false;
        }
        if GLOBAL_CHILD_SLOTS
            .compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return true;
        }
    }
}

/// 测试/诊断观测：当前占用的全应用子槽数（非承诺接口）。
#[doc(hidden)]
pub fn global_child_slots_in_use() -> usize {
    GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst)
}

/// 子线程退出时释放全局槽（guard 形态——所有出口含 panic 都释放）。
struct SlotGuard;

impl Drop for SlotGuard {
    fn drop(&mut self) {
        GLOBAL_CHILD_SLOTS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 子 Agent 模型发送器（测试可注入；生产＝api 适配器）。
pub trait ChildModelSender: Send + 'static {
    fn send(&self, body: &Value) -> Result<Value, String>;
}

/// 生产发送器：拥有型可信快照（provider/conn/key/model/agent 继承根用户轮，
/// §1.1——不提供模型覆盖；凭证只由本结构持有，不进子对话/事件/日志）。
pub struct ApiModelSend(pub crate::memory::runner::ModelSend);

impl ChildModelSender for ApiModelSend {
    fn send(&self, body: &Value) -> Result<Value, String> {
        crate::api::chat_with_provider_body(
            &self.0.provider_id,
            &self.0.conn,
            &self.0.key,
            &self.0.model,
            body,
            &self.0.agent,
        )
    }
}

/// 子 Agent 冻结工具面（spawn 请求时点快照，§1.2）：业务四工具恒在；
/// view_image 按根轮能力；023 资源工具按快照能力；022 动态 MCP 只含
/// **该请求已声明集合**（之后父搜索不扩展旧子 Agent）；外加仅向根发送的
/// send_message。六控制工具、问询/消息/搜索/权限/上下文/编排/mem 不进
/// 子目录。
pub struct ChildToolSet {
    pub view_image: bool,
    pub mcp: Option<Arc<crate::mcp::manager::McpSnapshot>>,
    pub declared_dynamic: HashSet<String>,
}

/// 子 Agent 请求的工具声明面。
pub fn child_tool_definitions(tools: &ChildToolSet) -> Vec<Value> {
    let mut defs = vec![
        crate::prompt::exec_command_tool(),
        crate::prompt::apply_patch_tool(),
        crate::prompt::write_stdin_tool(),
        crate::prompt::update_plan_tool(),
    ];
    if tools.view_image {
        defs.push(crate::view_image::tool_definition());
    }
    defs.push(super::send_tool_definition());
    if let Some(snapshot) = &tools.mcp {
        defs.extend(snapshot.resource_definitions().iter().cloned());
        // 动态定义按冻结的已声明集合过滤（声明面＝执行面，§1.2）。
        for def in snapshot.definitions() {
            let name = def.pointer("/function/name").and_then(Value::as_str);
            if name.is_some_and(|n| tools.declared_dynamic.contains(n)) {
                defs.push(def.clone());
            }
        }
    }
    defs
}

/// 交付给根的已收事实（wait 摘要只计数；正文批末交付）。
#[derive(Debug, Clone)]
pub enum RootUpdate {
    Message {
        from: String,
        message_id: u64,
        content: String,
    },
    Terminal {
        agent: String,
        assignment: u64,
        state: String,
        report: Option<AgentReport>,
    },
}

/// 子轮 FinalAnswer 邮箱门判定结果（§1.4：补充输入先受理→交付并继续同一
/// assignment；终文先提交→Agent 空闲）。
enum GateDecision {
    /// 邮箱已有未交付补充输入：按受理顺序交付，继续同一 assignment。
    Continue(Vec<QueuedMessage>),
    /// 终文成立：发布 completed（报告随终态事件与根交付走）。
    Complete,
    /// Agent 已被中断/组关闭：终文丢弃（晚到回复不能把中断改成 completed）。
    Discarded,
}

/// 组写者域：seq 分配与提交串行化（seq 只在提交成功后占用——034 审查
/// P1-3：保存失败不留空洞）。
struct GroupWriter {
    next_seq: u64,
}

/// 一个根用户轮的 Agent 组运行时。
pub struct GroupRuntime {
    pub group_id: u64,
    core: Mutex<GroupCore>,
    writer: Mutex<GroupWriter>,
    session: Arc<Mutex<Session>>,
    /// 已提交事件（等待根线程在安全点推入内存历史并 on_record）。
    committed: Mutex<VecDeque<HistoryItem>>,
    /// 已收未交付的根侧事实（消息/终态通知）。
    root_updates: Mutex<Vec<RootUpdate>>,
    /// 宿主展示通知（根线程批末转发为 TurnNotify::Info）。
    notices: Mutex<Vec<String>>,
    /// 每 Agent 保留上下文（followup 续接用；子线程与根经同一 Arc 共享）。
    contexts: Mutex<HashMap<String, Arc<Mutex<Vec<HistoryItem>>>>>,
    procs: ProcessManager,
    /// 035 审查 B1：存活子线程计数——group_closed 延迟到**全部子线程排空后**
    /// 由最后退出者提交（在途子事件必然先于 group_closed 落盘，装载链
    /// 校验「group_closed 后无该组事件」恒成立；根轮出口不被传输排空阻塞）。
    live_children: AtomicUsize,
    /// group_closed 是否已提交（根/最后子线程经 CAS 恰一次）。
    closed_event_written: AtomicBool,
}

/// turn 侧装配（TurnExtras.agents）：宿主按每轮快照传入；六工具声明与
/// 执行同门（模型目录六名全配已由调用方核对，部分配置不装配）。
pub struct AgentExtras<'a> {
    pub manager: &'a AgentManager,
    /// 共享会话写者（事件经 writer→session 单一锁链提交；tui_host 既有
    /// Arc<Mutex<Session>> 同款，main 自 035 起同样共享）。
    pub session: Arc<Mutex<Session>>,
    /// 子 Agent 模型发送器工厂（拥有型可信快照——provider/conn/key/model/
    /// agent 继承根用户轮；凭证只由发送器持有，不进子对话/事件/日志）。
    pub sender_factory: &'a dyn Fn() -> Box<dyn ChildModelSender>,
    /// 冻结环境上下文（子上下文首条 user 消息；宿主每轮构建）。
    pub context_message: String,
    /// 本轮 MCP 快照 Arc（子工具面冻结用；None＝无 MCP）。
    pub mcp: Option<Arc<crate::mcp::manager::McpSnapshot>>,
}

/// 宿主拥有的管理器（跨根用户轮；main/tui_host 持有）。
pub struct AgentManager {
    next_group_id: AtomicUsize,
    current: Mutex<Option<Arc<GroupRuntime>>>,
}

impl Default for AgentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            next_group_id: AtomicUsize::new(1),
            current: Mutex::new(None),
        }
    }

    /// 新根用户轮：新 group_id（旧组不可执行、旧回调不能命中新组）。上一组
    /// 若未正常关闭（防御）先行关闭——不持有根内存历史，group_closed 走
    /// 泵路径，事件事实已在文件中。
    pub fn begin_turn(
        &self,
        session: Arc<Mutex<Session>>,
        procs: ProcessManager,
    ) -> Arc<GroupRuntime> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = current.take() {
            old.close_blind("superseded_by_next_turn");
        }
        let group_id = self.next_group_id.fetch_add(1, Ordering::SeqCst) as u64;
        let runtime = Arc::new(GroupRuntime {
            group_id,
            core: Mutex::new(GroupCore::new(group_id)),
            writer: Mutex::new(GroupWriter { next_seq: 0 }),
            session,
            committed: Mutex::new(VecDeque::new()),
            root_updates: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
            contexts: Mutex::new(HashMap::new()),
            procs,
            live_children: AtomicUsize::new(0),
            closed_event_written: AtomicBool::new(false),
        });
        *current = Some(runtime.clone());
        runtime
    }
}

/// 组内工具错误：`(code, message)`（映射 §1.3 错误码）。
type ToolError = (&'static str, String);

impl GroupRuntime {
    /// 组是否产生过任何事件（决定 group_closed 与关闭路径是否落盘）。
    fn has_events(&self) -> bool {
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_seq
            > 0
    }

    /// 根线程提交（有根内存历史：滞留待写段随行写入）。
    fn commit_root_event(
        &self,
        history: &[HistoryItem],
        kind: AgentEventKind,
        agent: Option<&str>,
        assignment: Option<u64>,
        payload: AgentEventPayload,
    ) -> Result<HistoryItem, String> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let seq = writer.next_seq + 1;
        let event = agent_event(kind, self.group_id, seq, agent, assignment, payload);
        if !event_size_ok(&event) {
            return Err("Agent 事件超过 2 MiB 序列化上限，拒绝提交".into());
        }
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        session.commit_agent_event_root(history, &event)?;
        writer.next_seq = seq;
        drop(session);
        drop(writer);
        self.committed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event.clone());
        Ok(event)
    }

    /// 子线程提交（泵路径：只追加事件行；version 已由首个根侧提交迁移为 3）。
    fn commit_child_event(
        &self,
        kind: AgentEventKind,
        agent: Option<&str>,
        assignment: Option<u64>,
        payload: AgentEventPayload,
    ) -> Result<HistoryItem, String> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let seq = writer.next_seq + 1;
        let event = agent_event(kind, self.group_id, seq, agent, assignment, payload);
        if !event_size_ok(&event) {
            return Err("Agent 事件超过 2 MiB 序列化上限，拒绝提交".into());
        }
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        session.commit_agent_event_pump(&event)?;
        writer.next_seq = seq;
        drop(session);
        drop(writer);
        self.committed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(event.clone());
        Ok(event)
    }

    /// 组事件负载计费（提交成功后调用：按真实大小入账累计；预留的
    /// 释放/冲销由调用方经 `commit_event_bytes` 完成）。
    fn account_event(&self, event: &HistoryItem) {
        let actual = event_bytes(event);
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.event_bytes_committed = core.event_bytes_committed.saturating_add(actual);
    }

    fn notice(&self, line: String) {
        self.notices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line);
    }

    /// group_closed 恰一次提交判定（审查 B1）：CAS 防根/最后子线程双写；
    /// 无事件组（从未 spawn 成功）不写——保持「没有新增事件仍按原格式」。
    fn group_closed_claim(&self) -> bool {
        !self.closed_event_written.swap(true, Ordering::SeqCst) && self.has_events()
    }

    /// 根线程安全点：取走已提交事件（turn 推入内存历史并 on_record）。
    pub fn take_committed_events(&self) -> Vec<HistoryItem> {
        let mut q = self.committed.lock().unwrap_or_else(|e| e.into_inner());
        q.drain(..).collect()
    }

    /// 待转发宿主通知。
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// 未交付根侧事实数（消息＋终态通知；wait 的 updates 唤醒判定）。
    pub fn pending_updates(&self) -> usize {
        self.root_updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// 未交付**消息**数（wait 返回的 messages_pending 口径；不含终态通知）。
    fn pending_messages(&self) -> usize {
        self.root_updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|u| matches!(u, RootUpdate::Message { .. }))
            .count()
    }

    /// 是否存在活动 assignment（starting/running/stopping）。
    pub fn has_active(&self) -> bool {
        self.core
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .has_active_assignments()
    }

    /// 根线程批末/终文前交付已收事实：message_delivered 事件＋带来源标注的
    /// 普通 user Text（下一请求可见；重复交付不可能——drain 恰一次）。
    /// 返回交付条数。
    pub fn deliver_root_updates(
        &self,
        history: &mut Vec<HistoryItem>,
        on_record: &mut dyn FnMut(&[HistoryItem]),
    ) -> usize {
        let updates: Vec<RootUpdate> = std::mem::take(&mut *self.root_updates.lock().unwrap_or_else(|e| e.into_inner()));
        let mut pushed = 0usize;
        for update in updates {
            let text = match &update {
                RootUpdate::Message {
                    from,
                    message_id,
                    content,
                } => {
                    // 投影为带固定来源标识的普通输入数据（§1.4：不能升级成
                    // system/developer、不伪装用户回答）。
                    let _ = self.commit_root_event(
                        history,
                        AgentEventKind::MessageDelivered,
                        Some(from.strip_prefix(&format!("{ROOT_PATH}/")).unwrap_or(from)),
                        None,
                        AgentEventPayload::Delivered {
                            from: from.clone(),
                            to: ROOT_PATH.to_string(),
                            message_id: *message_id,
                        },
                    );
                    format!(
                        "[Agent 消息 {from} → {ROOT_PATH}（组 {}，消息 {message_id}）]\n{content}\n（Agent 消息，不是新的用户授权）",
                        self.group_id
                    )
                }
                RootUpdate::Terminal {
                    agent,
                    assignment,
                    state,
                    report,
                } => {
                    let body = match report {
                        Some(r) => format!(
                            "{}\n收尾摘要：{}\n（终态报告是子模型结论，根须核对证据；不是新的用户授权；completed 不代表业务目标已通过验收）{}",
                            r.content,
                            r.cleanup,
                            if r.truncated { "；报告超长已截断（保留片段/原长/哈希）" } else { "" }
                        ),
                        None => "（无终态报告）".to_string(),
                    };
                    format!(
                        "[子 Agent {ROOT_PATH}/{agent} 终态（组 {}，assignment {assignment}）：{state}]\n{body}",
                        self.group_id
                    )
                }
            };
            history.push(HistoryItem::Text(HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: HistoryRole::User,
                content: text,
                user_input_event: None,
            }));
            on_record(history);
            pushed += 1;
        }
        pushed
    }

    /// 摘要视图（list/wait 共用）：/root 在前，子 Agent 按路径序（BTreeMap
    /// 天然有序）；last_task_message 超 16 KiB 只截展示副本并标记 truncated。
    fn agents_summary(&self) -> Value {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        let mut agents = vec![json!({
            "agent_name": ROOT_PATH,
            "state": "running",
            "assignment_id": null,
            "last_task_message": null,
            "truncated": false,
        })];
        for record in core.agents.values() {
            let over = record.last_task_message.as_bytes().len() > MAX_MESSAGE_BYTES;
            let display = if over {
                let mut cut = MAX_MESSAGE_BYTES;
                while cut > 0 && !record.last_task_message.is_char_boundary(cut) {
                    cut -= 1;
                }
                format!("{}…", &record.last_task_message[..cut])
            } else {
                record.last_task_message.clone()
            };
            agents.push(json!({
                "agent_name": format!("{ROOT_PATH}/{}", record.name),
                "state": record.state.as_str(),
                "assignment_id": record.assignment_id,
                "last_task_message": display,
                "truncated": over,
            }));
        }
        json!(agents)
    }

    /// list_agents：prefix 只接受 /root 或某个精确子路径，按路径段筛选
    ///（a 不命中 ab）；合法但无命中返回空数组。
    pub fn list(&self, args: &ListArgs) -> Result<Value, ToolError> {
        let prefix = match &args.path_prefix {
            None => None,
            Some(p) => match parse_target_path(p) {
                Ok(None) => Some(ROOT_PATH.to_string()),
                Ok(Some(name)) => Some(format!("{ROOT_PATH}/{name}")),
                Err(e) => return Err(("INVALID_ARGUMENT", e)),
            },
        };
        let all = self.agents_summary();
        let agents: Vec<Value> = all
            .as_array()
            .expect("summary 为数组")
            .iter()
            .filter(|a| match &prefix {
                None => true,
                Some(p) => {
                    let name = a["agent_name"].as_str().unwrap_or("");
                    name == p.as_str() || name.starts_with(&format!("{p}/"))
                }
            })
            .cloned()
            .collect();
        Ok(json!({ "agents": agents }))
    }

    /// spawn_agent（仅根）：原子保留名称/执行槽/任务额度并保存后才启动。
    /// `self: &Arc<Self>`——子线程需要同族 Arc（组运行时只经 Arc 存在）。
    pub fn spawn(
        self: &Arc<Self>,
        history: &[HistoryItem],
        args: &SpawnArgs,
        spawn_ctx: &SpawnContext<'_>,
    ) -> Result<Value, ToolError> {
        // ① 预留（core）：名称/记录数/assignment 数；全局槽。
        let assignment_id = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.reserve_assignment(&args.task_name)
                .map_err(|e| ("CAPACITY_LIMIT", e))?
        };
        if !try_reserve_slot() {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.rollback_assignment();
            return Err((
                "CAPACITY_LIMIT",
                format!("全应用子执行槽已满（{MAX_CHILD_SLOTS}，含在途排空），本次 spawn 拒绝"),
            ));
        }
        // fork 前缀：产生 spawn 响应的请求所用的已闭合活动历史前缀（调用方
        // 传入该请求的 active_items；可携带记录过滤在 events::portable）。
        let prefix: Vec<HistoryItem> = match args.fork_turns {
            ForkTurns::All => spawn_ctx
                .active_items
                .iter()
                .filter_map(portable_clone)
                .collect(),
            ForkTurns::None => Vec::new(),
        };
        let prefix_end = prefix.len() as u64;
        // ② 保存 agent_created（含任务正文——任务计入组消息预算）。
        {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if !core.group_message_room(args.message.as_bytes().len()) {
                core.rollback_assignment();
                GLOBAL_CHILD_SLOTS.fetch_sub(1, Ordering::SeqCst);
                return Err((
                    "BUDGET_LIMIT",
                    "组累计消息预算已满（128 条/1 MiB，含初始任务/续派）".into(),
                ));
            }
        }
        let created = self.commit_root_event(
            history,
            AgentEventKind::AgentCreated,
            Some(&args.task_name),
            Some(assignment_id),
            AgentEventPayload::Fork {
                context_id: spawn_ctx.context_id,
                prefix_end,
                fork: args.fork_turns.as_str().to_string(),
                task: args.message.clone(),
            },
        );
        let event = match created {
            Ok(event) => {
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                core.commit_group_message(args.message.as_bytes().len());
                event
            }
            Err(e) => {
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                core.rollback_assignment();
                // 全局槽随失败释放（不泄槽——后续 spawn 仍可准入）
                GLOBAL_CHILD_SLOTS.fetch_sub(1, Ordering::SeqCst);
                return Err(("PERSISTENCE_FAILED", format!("agent_created 保存失败：{e}（未启动）")));
            }
        };
        self.account_event(&event);
        // ③ 发布记录＋初始上下文＋启动子线程。
        let cancel = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.publish_spawn(&args.task_name, assignment_id, args.message.clone())
        };
        let mut ctx: Vec<HistoryItem> = Vec::with_capacity(prefix.len() + 2);
        ctx.push(HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: spawn_ctx.preamble.clone(),
            user_input_event: None,
        }));
        ctx.extend(prefix);
        ctx.push(HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: task_label(&args.message, self.group_id, assignment_id),
            user_input_event: None,
        }));
        let shared_ctx = Arc::new(Mutex::new(ctx));
        self.contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(args.task_name.clone(), shared_ctx.clone());
        let spec = ChildSpec {
            runtime: self.clone(),
            agent: args.task_name.clone(),
            assignment: assignment_id,
            ctx: shared_ctx,
            initial_task: None, // 任务消息已进初始上下文
            tools: ChildToolSet {
                view_image: spawn_ctx.view_image,
                mcp: spawn_ctx.mcp.clone(),
                declared_dynamic: spawn_ctx.declared_dynamic.clone(),
            },
            sender: (spawn_ctx.sender_factory)(),
            system: spawn_ctx.system.clone(),
            deadline: Instant::now() + Duration::from_secs(super::ASSIGNMENT_WALL_LIMIT_SECS),
            wall_limit: Duration::from_secs(super::ASSIGNMENT_WALL_LIMIT_SECS),
            cancel,
        };
        self.live_children.fetch_add(1, Ordering::SeqCst);
        if std::thread::Builder::new()
            .name(format!("tchat-agent-{}", args.task_name))
            .spawn(move || child_main(spec))
            .is_err()
        {
            // 线程启动失败：释放全局槽与存活计数、记录置 failed（名称已占用，
            // assignment 已计——如实报告，不伪装受理）。
            self.live_children.fetch_sub(1, Ordering::SeqCst);
            GLOBAL_CHILD_SLOTS.fetch_sub(1, Ordering::SeqCst);
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(record) = core.agents.get_mut(&args.task_name) {
                record.state = AgentState::Failed;
                record.terminal_emitted = true; // 未能启动，无终态报告
            }
            return Err((
                "PERSISTENCE_FAILED",
                "子 Agent 线程启动失败（操作系统资源不足）：记录已保留，任务未执行".into(),
            ));
        }
        Ok(json!({
            "agent_name": format!("{ROOT_PATH}/{}", args.task_name),
            "assignment_id": assignment_id,
        }))
    }

    /// send_message（根侧）：只排入目标邮箱，不启动空闲 Agent；保存
    /// message_queued 后才回 accepted。
    pub fn send_from_root(
        &self,
        history: &[HistoryItem],
        args: &TargetMessageArgs,
    ) -> Result<Value, ToolError> {
        let name = parse_target_path(&args.target)
            .map_err(|e| ("INVALID_ARGUMENT", e))?
            .ok_or_else(|| {
                (
                    "INVALID_ARGUMENT",
                    format!("根不能用 {ROOT_PATH} 自身作为 send_message 目标"),
                )
            })?;
        let message_id = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            let record = core
                .resolve_child_mut(&name)
                .map_err(|_| ("AGENT_NOT_FOUND", format!("目标 {} 不在当前组", args.target)))?;
            if !record.mailbox.has_room(args.message.as_bytes().len())
                || !core.group_message_room(args.message.as_bytes().len())
            {
                return Err((
                    "CAPACITY_LIMIT",
                    "目标邮箱或组消息预算已满，先拒绝（不先 accepted 后丢弃）".into(),
                ));
            }
            let id = core.next_message_id;
            core.next_message_id += 1;
            id
        };
        let queued = self.commit_root_event(
            history,
            AgentEventKind::MessageQueued,
            Some(&name),
            None,
            AgentEventPayload::Queued {
                from: ROOT_PATH.to_string(),
                to: format!("{ROOT_PATH}/{name}"),
                message_id,
                content: args.message.clone(),
            },
        );
        match queued {
            Ok(event) => {
                self.account_event(&event);
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(record) = core.agents.get_mut(&name) {
                    record.mailbox.push(QueuedMessage {
                        message_id,
                        from: ROOT_PATH.to_string(),
                        to: format!("{ROOT_PATH}/{name}"),
                        content: args.message.clone(),
                    });
                }
                core.commit_group_message(args.message.as_bytes().len());
                Ok(json!({
                    "message_id": message_id,
                    "accepted": true,
                    "started": false,
                }))
            }
            Err(e) => Err((
                "PERSISTENCE_FAILED",
                format!("message_queued 保存失败：{e}（未受理）"),
            )),
        }
    }

    /// followup_task（仅根，目标非根）：空闲续派新 assignment / 运行中作
    /// 同一 assignment 补充输入；stopping 拒绝。`self: &Arc<Self>` 同 spawn。
    pub fn followup(
        self: &Arc<Self>,
        history: &[HistoryItem],
        args: &TargetMessageArgs,
        spawn_ctx: &SpawnContext<'_>,
    ) -> Result<Value, ToolError> {
        let name = parse_target_path(&args.target)
            .map_err(|e| ("INVALID_ARGUMENT", e))?
            .ok_or_else(|| {
                (
                    "INVALID_ARGUMENT",
                    format!("followup_task 的目标不能是 {ROOT_PATH}"),
                )
            })?;
        // 分派判定（core 单锁段原子）。
        enum Plan {
            Supplemental(u64),
            /// 新 assignment 编号、取消标志、回滚时还原的前一终态与任务文案。
            NewAssignment(u64, Arc<AtomicBool>, AgentState, String),
        }
        let plan = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if core.closed {
                return Err(("AGENT_NOT_FOUND", "当前组已关闭".into()));
            }
            let (state, continuable, mailbox_room, prev_task) = {
                let record = core
                    .resolve_child_mut(&name)
                    .map_err(|_| ("AGENT_NOT_FOUND", format!("目标 {} 不在当前组", args.target)))?;
                (
                    record.state,
                    record.context_continuable,
                    record.mailbox.has_room(args.message.as_bytes().len()),
                    record.last_task_message.clone(),
                )
            };
            match state {
                AgentState::Running | AgentState::Starting => {
                    if !mailbox_room || !core.group_message_room(args.message.as_bytes().len()) {
                        return Err(("CAPACITY_LIMIT", "目标邮箱或组消息预算已满".into()));
                    }
                    let id = core.next_message_id;
                    core.next_message_id += 1;
                    Plan::Supplemental(id)
                }
                AgentState::Stopping => {
                    return Err((
                        "AGENT_BUSY",
                        format!("目标 {name} 正在收尾（stopping），不可 followup 启动"),
                    ));
                }
                AgentState::Closed => {
                    return Err(("AGENT_NOT_FOUND", format!("目标 {} 不在当前组", args.target)));
                }
                AgentState::Completed | AgentState::Failed | AgentState::Interrupted => {
                    if !continuable {
                        return Err((
                            "CONTEXT_UNAVAILABLE",
                            format!("目标 {name} 的上下文已截断/损坏（context_continuable=false），不能续接"),
                        ));
                    }
                    if core.assignments_started >= super::MAX_ASSIGNMENTS_PER_GROUP {
                        return Err((
                            "CAPACITY_LIMIT",
                            format!("当前组 assignment 启动已达上限 {}", super::MAX_ASSIGNMENTS_PER_GROUP),
                        ));
                    }
                    let assignment_id = core.next_assignment_id;
                    core.next_assignment_id += 1;
                    core.assignments_started += 1;
                    let cancel = Arc::new(AtomicBool::new(false));
                    let record = core
                        .agents
                        .get_mut(&name)
                        .expect("上方 resolve 已确认存在");
                    record.assignments += 1;
                    record.assignment_id = assignment_id;
                    record.requests_used = 0;
                    record.calls_used = 0;
                    record.state = AgentState::Starting;
                    record.terminal_emitted = false;
                    record.last_task_message = args.message.clone();
                    record.cancel = cancel.clone();
                    Plan::NewAssignment(assignment_id, cancel, state, prev_task)
                }
            }
        };
        match plan {
            Plan::Supplemental(message_id) => {
                // 保存 message_queued 后入队（运行中：子轮批末收取）。
                let queued = self.commit_root_event(
                    history,
                    AgentEventKind::MessageQueued,
                    Some(&name),
                    None,
                    AgentEventPayload::Queued {
                        from: ROOT_PATH.to_string(),
                        to: format!("{ROOT_PATH}/{name}"),
                        message_id,
                        content: args.message.clone(),
                    },
                );
                match queued {
                    Ok(event) => {
                        self.account_event(&event);
                        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                        let assignment = core
                            .agents
                            .get(&name)
                            .map(|r| r.assignment_id)
                            .unwrap_or(0);
                        if let Some(record) = core.agents.get_mut(&name) {
                            record.mailbox.push(QueuedMessage {
                                message_id,
                                from: ROOT_PATH.to_string(),
                                to: format!("{ROOT_PATH}/{name}"),
                                content: args.message.clone(),
                            });
                        }
                        core.commit_group_message(args.message.as_bytes().len());
                        Ok(json!({
                            "message_id": message_id,
                            "accepted": true,
                            "started": false,
                            "assignment_id": assignment,
                        }))
                    }
                    Err(e) => Err((
                        "PERSISTENCE_FAILED",
                        format!("message_queued 保存失败：{e}（未受理）"),
                    )),
                }
            }
            Plan::NewAssignment(assignment_id, cancel, prev_state, prev_task) => {
                if !try_reserve_slot() {
                    // 回滚 assignment 预留，Agent 回到原终态展示。
                    self.rollback_followup(&name, assignment_id, prev_state, &prev_task);
                    return Err((
                        "CAPACITY_LIMIT",
                        format!("全应用子执行槽已满（{MAX_CHILD_SLOTS}），followup 拒绝"),
                    ));
                }
                {
                    let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    if !core.group_message_room(args.message.as_bytes().len()) {
                        self.rollback_followup(&name, assignment_id, prev_state, &prev_task);
                        GLOBAL_CHILD_SLOTS.fetch_sub(1, Ordering::SeqCst);
                        return Err(("BUDGET_LIMIT", "组累计消息预算已满".into()));
                    }
                }
                let started = self.commit_root_event(
                    history,
                    AgentEventKind::AssignmentStarted,
                    Some(&name),
                    Some(assignment_id),
                    AgentEventPayload::Task {
                        message: args.message.clone(),
                    },
                );
                if let Err(e) = started {
                    self.rollback_followup(&name, assignment_id, prev_state, &prev_task);
                    GLOBAL_CHILD_SLOTS.fetch_sub(1, Ordering::SeqCst);
                    return Err((
                        "PERSISTENCE_FAILED",
                        format!("assignment_started 保存失败：{e}（未启动）"),
                    ));
                }
                self.account_event(&started.expect("已确认 Ok"));
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                core.commit_group_message(args.message.as_bytes().len());
                drop(core);
                let shared_ctx = self
                    .contexts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&name)
                    .cloned();
                let Some(shared_ctx) = shared_ctx else {
                    self.rollback_followup(&name, assignment_id, prev_state, &prev_task);
                    return Err((
                        "CONTEXT_UNAVAILABLE",
                        format!("目标 {name} 的保留上下文缺失，不能续接"),
                    ));
                };
                let spec = ChildSpec {
                    runtime: self.clone(),
                    agent: name.clone(),
                    assignment: assignment_id,
                    ctx: shared_ctx,
                    initial_task: Some(args.message.clone()),
                    tools: ChildToolSet {
                        view_image: spawn_ctx.view_image,
                        mcp: spawn_ctx.mcp.clone(),
                        declared_dynamic: spawn_ctx.declared_dynamic.clone(),
                    },
                    sender: (spawn_ctx.sender_factory)(),
                    system: spawn_ctx.system.clone(),
                    deadline: Instant::now() + Duration::from_secs(super::ASSIGNMENT_WALL_LIMIT_SECS),
                    wall_limit: Duration::from_secs(super::ASSIGNMENT_WALL_LIMIT_SECS),
                    cancel,
                };
                self.live_children.fetch_add(1, Ordering::SeqCst);
                if std::thread::Builder::new()
                    .name(format!("tchat-agent-{name}"))
                    .spawn(move || child_main(spec))
                    .is_err()
                {
                    self.live_children.fetch_sub(1, Ordering::SeqCst);
                    GLOBAL_CHILD_SLOTS.fetch_sub(1, Ordering::SeqCst);
                    let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(record) = core.agents.get_mut(&name) {
                        record.state = AgentState::Failed;
                        record.terminal_emitted = true;
                    }
                    return Err((
                        "PERSISTENCE_FAILED",
                        "子 Agent 线程启动失败（操作系统资源不足）：任务未执行".into(),
                    ));
                }
                Ok(json!({
                    "message_id": Value::Null,
                    "accepted": true,
                    "started": true,
                    "assignment_id": assignment_id,
                }))
            }
        }
    }

    /// followup 启动失败回滚：assignment 额度归还，Agent 回到**前一终态与
    /// 原任务文案**（审查 N3——不得错置 failed、不得毁最近任务展示）。
    fn rollback_followup(&self, name: &str, assignment_id: u64, prev: AgentState, prev_task: &str) {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.assignments_started = core.assignments_started.saturating_sub(1);
        if core.next_assignment_id == assignment_id + 1 {
            core.next_assignment_id = assignment_id;
        }
        if let Some(record) = core.agents.get_mut(name) {
            if record.assignment_id == assignment_id {
                record.state = prev;
                record.last_task_message = prev_task.to_string();
            }
        }
    }

    /// interrupt_agent（仅根）：线性化点原子撤销受理资格、置取消、放弃未
    /// 交付补充输入、进入 stopping；空闲目标保持原状态。
    pub fn interrupt(
        &self,
        history: &[HistoryItem],
        args: &TargetArgs,
    ) -> Result<Value, ToolError> {
        let name = parse_target_path(&args.target)
            .map_err(|e| ("INVALID_ARGUMENT", e))?
            .ok_or_else(|| {
                (
                    "INVALID_ARGUMENT",
                    format!("interrupt_agent 的目标不能是 {ROOT_PATH}"),
                )
            })?;
        let (previous_state, assignment_id, do_interrupt) = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            let record = core
                .resolve_child_mut(&name)
                .map_err(|_| ("AGENT_NOT_FOUND", format!("目标 {} 不在当前组", args.target)))?;
            let prev = record.state;
            match prev {
                AgentState::Starting | AgentState::Running | AgentState::Stopping => {
                    record.cancel.store(true, Ordering::SeqCst);
                    record.state = AgentState::Stopping;
                    (prev, record.assignment_id, true)
                }
                AgentState::Completed | AgentState::Failed | AgentState::Interrupted
                | AgentState::Closed => (prev, record.assignment_id, false),
            }
        };
        if !do_interrupt {
            // 空闲/已终态：保持原状态（completed 不改成假中断）。
            return Ok(json!({
                "agent_name": format!("{ROOT_PATH}/{name}"),
                "previous_state": previous_state.as_str(),
                "state": previous_state.as_str(),
            }));
        }
        // 回收该 assignment 的命令进程（不等传输排空；killpg 走宿主 PGID）。
        let reap_note = match self.procs.reap_agent_assignment(self.group_id, &name, assignment_id)
        {
            Ok(()) => "已回收所属进程".to_string(),
            Err(e) => format!("进程收尾存在未知结果：{e}"),
        };
        // 放弃未交付补充输入并记录原因（abandoned；不在未来任务中悄悄执行）。
        let abandoned = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.agents
                .get_mut(&name)
                .map(|r| r.mailbox.abandon().len())
                .unwrap_or(0)
        };
        let event = self.commit_root_event(
            history,
            AgentEventKind::StateChanged,
            Some(&name),
            Some(assignment_id),
            AgentEventPayload::State {
                state: AgentState::Stopping.as_str().to_string(),
                reason: Some(format!(
                    "interrupt：撤销当前 assignment 受理资格；放弃 {abandoned} 条未交付补充输入；{reap_note}"
                )),
                report: None,
            },
        );
        if let Err(e) = event {
            // 中断已生效（取消与回收先于记录），如实报告记录失败。
            return Err((
                "PERSISTENCE_FAILED",
                format!("interrupt 已生效但状态记录失败：{e}"),
            ));
        }
        self.account_event(&event.expect("已确认 Ok"));
        Ok(json!({
            "agent_name": format!("{ROOT_PATH}/{name}"),
            "previous_state": previous_state.as_str(),
            "state": AgentState::Stopping.as_str(),
        }))
    }

    /// wait_agent（仅根）：只观察——被未收消息/终态通知/根 steer/超时唤醒；
    /// 超时不当完成、不关 Agent、不消费正文（批末交付）。轮询粒度 50ms
    ///（steer 通道无唤醒钩子；等待为纯本地观察，无网络）。
    pub fn wait(&self, args: &WaitArgs, steer_pending: &dyn Fn() -> bool) -> String {
        let deadline = Instant::now() + Duration::from_millis(args.timeout_ms);
        loop {
            let pending = self.pending_updates();
            if pending > 0 {
                return ok_result(json!({
                    "reason": "updates",
                    "agents": self.agents_summary(),
                    "messages_pending": self.pending_messages(),
                }));
            }
            if steer_pending() {
                return ok_result(json!({
                    "reason": "steered",
                    "agents": self.agents_summary(),
                    "messages_pending": 0,
                }));
            }
            if !self.has_active() {
                // 没有在途子任务且无未收更新：立即 no_pending。
                return ok_result(json!({
                    "reason": "no_pending",
                    "agents": self.agents_summary(),
                    "messages_pending": 0,
                }));
            }
            if Instant::now() >= deadline {
                return ok_result(json!({
                    "reason": "timeout",
                    "agents": self.agents_summary(),
                    "messages_pending": 0,
                }));
            }
            std::thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    /// 轮出口统一封组：取消全部、回收整组进程、最终收取已提交事件入内存
    /// 历史。幂等（closed 短路）。group_closed 延迟提交（审查 B1）：仍有
    /// 存活子线程时不写（在途事件必须先于 group_closed 落盘），由最后退出
    /// 的子线程经泵路径补写——根轮出口不被传输排空阻塞，装载链校验恒成立。
    pub fn close_for_turn_exit(
        &self,
        history: &mut Vec<HistoryItem>,
        on_record: &mut dyn FnMut(&[HistoryItem]),
        reason: &str,
    ) {
        {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if core.closed {
                return;
            }
            core.close_all(reason);
        }
        // 回收整组 Agent 进程（Root 与其他组不受影响）。
        if let Err(e) = self.procs.reap_agent_group(self.group_id) {
            self.notice(format!("Agent 组关闭收尾存在未知结果：{e}"));
        }
        if self.live_children.load(Ordering::SeqCst) == 0 && self.group_closed_claim() {
            let closed = self.commit_root_event(
                history,
                AgentEventKind::GroupClosed,
                None,
                None,
                AgentEventPayload::GroupClose {
                    reason: reason.to_string(),
                },
            );
            if let Ok(event) = closed {
                self.account_event(&event);
            }
        }
        // 最终收取：已提交事件推入内存历史（此后到达的排空期事件只落文件，
        // 由 /resume 呈现——控制事件不进模型投影，无重复注入风险）。
        for event in self.take_committed_events() {
            history.push(event);
        }
        on_record(history);
    }

    /// 无根内存历史时的防御关闭（新轮顶替旧组）：不推内存（事实已在文件）。
    fn close_blind(&self, reason: &str) {
        {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if core.closed {
                return;
            }
            core.close_all(reason);
        }
        let _ = self.procs.reap_agent_group(self.group_id);
        if self.live_children.load(Ordering::SeqCst) == 0 && self.group_closed_claim() {
            let _ = self.commit_child_event(
                AgentEventKind::GroupClosed,
                None,
                None,
                AgentEventPayload::GroupClose {
                    reason: reason.to_string(),
                },
            );
        }
    }

    /// 最后一个子线程退出时的补写（core 已关闭且 live 归零；CAS 恰一次）。
    fn child_exit_group_closed(&self, reason: &str) {
        let closed = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.closed
        };
        if closed && self.group_closed_claim() {
            let _ = self.commit_child_event(
                AgentEventKind::GroupClosed,
                None,
                None,
                AgentEventPayload::GroupClose {
                    reason: reason.to_string(),
                },
            );
        }
    }

    // ===== 子轮侧（child_main 协作面） =====

    /// 子→根消息：保存 message_queued（泵路径）后才回 message_id；随后排入
    /// 根侧待交付队列并唤醒 wait。
    fn child_send_to_root(
        &self,
        agent: &str,
        assignment: u64,
        content: &str,
    ) -> Result<u64, String> {
        let message_id = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            if core.closed {
                return Err("当前组已关闭，消息未受理".into());
            }
            if !core.group_message_room(content.as_bytes().len()) {
                return Err("组累计消息预算已满，消息未受理（不先 accepted 后丢弃）".into());
            }
            let id = core.next_message_id;
            core.next_message_id += 1;
            id
        };
        let queued = self.commit_child_event(
            AgentEventKind::MessageQueued,
            Some(agent),
            Some(assignment),
            AgentEventPayload::Queued {
                from: format!("{ROOT_PATH}/{agent}"),
                to: ROOT_PATH.to_string(),
                message_id,
                content: content.to_string(),
            },
        );
        match queued {
            Ok(event) => {
                self.account_event(&event);
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                core.commit_group_message(content.as_bytes().len());
                drop(core);
                self.root_updates
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(RootUpdate::Message {
                        from: format!("{ROOT_PATH}/{agent}"),
                        message_id,
                        content: content.to_string(),
                    });
                Ok(message_id)
            }
            Err(e) => Err(format!("message_queued 保存失败：{e}（未受理）")),
        }
    }

    /// FinalAnswer 邮箱门（§1.4 竞态裁决）：先回收本 assignment 进程，再在
    /// core 单锁段内判定邮箱与中断状态。
    fn final_answer_gate(&self, agent: &str, assignment: u64) -> GateDecision {
        // 终态前收尾所拥有进程（§1.5：先收尾再发布终态）。
        let _ = self.procs.reap_agent_assignment(self.group_id, agent, assignment);
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        if core.closed {
            return GateDecision::Discarded;
        }
        let Some(record) = core.agents.get_mut(agent) else {
            return GateDecision::Discarded;
        };
        match record.state {
            AgentState::Running => {
                if record.mailbox.has_pending() {
                    GateDecision::Continue(record.mailbox.drain())
                } else {
                    GateDecision::Complete
                }
            }
            _ => GateDecision::Discarded, // 已中断/已终态：晚到终文丢弃
        }
    }

    /// 终态恰一次发布（core 锁内改状态；事件提交与根通知随后）。返回是否
    /// 由本次调用发布（false＝已被中断路径或先前终态占先）。
    fn child_publish_terminal(
        &self,
        agent: &str,
        assignment: u64,
        state: AgentState,
        reason: &str,
        report: Option<AgentReport>,
    ) -> bool {
        {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            let room_ok = core.terminal_report_room().is_ok();
            let Some(record) = core.agents.get_mut(agent) else {
                return false;
            };
            if record.terminal_emitted || matches!(record.state, AgentState::Closed) {
                return false;
            }
            if !room_ok && report.is_some() {
                // 终态槽满：降级为无报告终态（如实记录原因）。
                record.terminal_emitted = true;
                record.state = state;
                return true;
            }
            record.terminal_emitted = true;
            record.state = state;
            if report.is_some() {
                core.commit_terminal_report();
            }
        }
        // 进程收尾摘要随终态（不被模型终文覆盖）。
        let event = self.commit_child_event(
            AgentEventKind::StateChanged,
            Some(agent),
            Some(assignment),
            AgentEventPayload::State {
                state: state.as_str().to_string(),
                reason: Some(reason.to_string()),
                report: report.clone(),
            },
        );
        match event {
            Ok(event) => {
                self.account_event(&event);
                if !{
                    let core_guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    core_guard.closed
                } {
                    self.root_updates
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(RootUpdate::Terminal {
                            agent: agent.to_string(),
                            assignment,
                            state: state.as_str().to_string(),
                            report,
                        });
                }
                true
            }
            Err(_) => {
                // 落盘失败：状态已发布（槽位须释放）；根通知改为结果未知。
                if !{
                    let core_guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    core_guard.closed
                } {
                    self.root_updates
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(RootUpdate::Terminal {
                            agent: agent.to_string(),
                            assignment,
                            state: state.as_str().to_string(),
                            report: None,
                        });
                }
                true
            }
        }
    }

    /// 子轮批末收取自己的邮箱（受理顺序；交付投影为带来源标注输入）。
    fn child_drain_mailbox(&self, agent: &str, assignment: u64) -> Vec<QueuedMessage> {
        let drained = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            match core.agents.get_mut(agent) {
                Some(record) => record.mailbox.drain(),
                None => Vec::new(),
            }
        };
        for message in &drained {
            let _ = self.commit_child_event(
                AgentEventKind::MessageDelivered,
                Some(agent),
                Some(assignment),
                AgentEventPayload::Delivered {
                    from: message.from.clone(),
                    to: message.to.clone(),
                    message_id: message.message_id,
                },
            );
        }
        drained
    }

    /// 子轮请求准入＋响应预留（core 单锁段原子）。
    fn child_admit_request(&self, agent: &str) -> Result<(), String> {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.admit_child_request(agent)?;
        if !core.event_room(super::RESPONSE_RESERVE_BYTES) {
            // 请求已计数（发送失败/取消也计数，§1.6），但动作不开始。
            return Err(format!(
                "组事件负载预留不足（{} MiB 上限）",
                super::MAX_GROUP_EVENT_BYTES / (1024 * 1024)
            ));
        }
        core.reserve_event_bytes(super::RESPONSE_RESERVE_BYTES);
        Ok(())
    }

    /// 子工具调用准入＋结果记录预留。
    fn child_admit_call(&self, agent: &str) -> Result<(), String> {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.admit_child_call(agent)?;
        if !core.event_room(super::RESULT_RESERVE_BYTES) {
            return Err("组事件负载预留不足，工具调用不开始".into());
        }
        core.reserve_event_bytes(super::RESULT_RESERVE_BYTES);
        Ok(())
    }

    /// 预留冲销（提交真实大小或动作失败时释放未用预留）。
    fn child_release_reserve(&self, reserved: u64, actual: Option<u64>) {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.commit_event_bytes(reserved, actual.unwrap_or(0));
    }

    fn group_closed(&self) -> bool {
        self.core.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    /// 记录失败致子上下文不可续用（§1.6：截断/损坏的原生上下文标
    /// context_continuable=false——followup 返回 CONTEXT_UNAVAILABLE）。
    fn mark_context_broken(&self, agent: &str) {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(record) = core.agents.get_mut(agent) {
            record.context_continuable = false;
        }
    }
}

/// spawn/followup 调用时点的冻结上下文（根线程构造；`build_sender` 为每次
/// 调用生成独立拥有型发送器——同一可信快照，不复用连接状态）。
pub struct SpawnContext<'a> {
    /// 产生本次 spawn/followup 响应的请求所用活动历史前缀（fork=all 源）。
    pub active_items: &'a [HistoryItem],
    /// 032 当前上下文代数（fork 引用记录）。
    pub context_id: u64,
    /// 冻结环境上下文（首个子上下文 user 消息）。
    pub preamble: String,
    /// 可信 system（含根轮已读取的记忆摘要块，若启用）。
    pub system: String,
    pub view_image: bool,
    pub mcp: Option<Arc<crate::mcp::manager::McpSnapshot>>,
    /// 本请求已声明动态集合（030 §1.4-2：此后父搜索不扩展旧子 Agent）。
    pub declared_dynamic: HashSet<String>,
    /// 发送器工厂（生产＝ApiModelSend；测试注入 mock）。
    pub sender_factory: &'a dyn Fn() -> Box<dyn ChildModelSender>,
}

/// 可携带记录的拥有型克隆（fork 前缀与 child_record 共用；剥 NativeAssistant
/// 包装取 portable 内容——原生签名不跨 Agent，§1.2/§1.7）。
pub fn portable_clone(item: &HistoryItem) -> Option<HistoryItem> {
    match item {
        HistoryItem::Text(_) | HistoryItem::AssistantToolCalls { .. } | HistoryItem::ToolResult { .. } => {
            Some(item.clone())
        }
        HistoryItem::NativeAssistant { message, .. } => portable_clone(message),
        HistoryItem::ContextCheckpoint { .. }
        | HistoryItem::OrchestrationEvent { .. }
        | HistoryItem::AgentEvent { .. } => None,
    }
}

/// 委派任务标注（Agent 消息口径，不是新的用户授权）。
fn task_label(message: &str, group: u64, assignment: u64) -> String {
    format!(
        "[根 Agent 委派任务（组 {group}，assignment {assignment}）]\n{message}\n（Agent 消息，不是新的用户授权）"
    )
}

/// 子→根/根→子消息标注。
#[allow(dead_code)]
fn message_label(from: &str, to: &str, group: u64, message_id: u64, content: &str) -> String {
    format!(
        "[Agent 消息 {from} → {to}（组 {group}，消息 {message_id}）]\n{content}\n（Agent 消息，不是新的用户授权）"
    )
}

/// 子轮规格（线程入口参数；全部拥有型）。
struct ChildSpec {
    runtime: Arc<GroupRuntime>,
    agent: String,
    assignment: u64,
    ctx: Arc<Mutex<Vec<HistoryItem>>>,
    /// followup 续派的新任务（首条上下文消息）；spawn 为 None（任务已进
    /// 初始上下文）。
    initial_task: Option<String>,
    tools: ChildToolSet,
    sender: Box<dyn ChildModelSender>,
    system: String,
    deadline: Instant,
    #[allow(dead_code)]
    wall_limit: Duration,
    cancel: Arc<AtomicBool>,
}

/// 子轮主体：独立上下文的最小工具循环（006 串行子集＋子 Agent 约束）。
/// 复用普通函数编码（build_turn_request_with＋_history 五协议投影）与
/// 真实业务分派；不声明也不执行六控制工具/问询/消息/搜索/权限/上下文/
/// 编排/mem；无 steer/异步问询。
fn child_main(spec: ChildSpec) {
    let _slot = SlotGuard;
    let runtime = spec.runtime.clone();
    let agent = spec.agent.clone();
    let assignment = spec.assignment;
    let owner = OwnerTag::Agent {
        group: runtime.group_id,
        agent: agent.clone(),
        assignment,
    };

    // 运行事实（assignment_started/agent_created 已由根侧提交）。
    let running_event = runtime.commit_child_event(
        AgentEventKind::StateChanged,
        Some(&agent),
        Some(assignment),
        AgentEventPayload::State {
            state: AgentState::Running.as_str().to_string(),
            reason: None,
            report: None,
        },
    );
    if running_event.is_err() {
        // 保存失败＝未启动（§1.7：保存成功才发布状态）。
        runtime.child_publish_terminal(
            &agent,
            assignment,
            AgentState::Failed,
            "state_changed(running) 保存失败：任务未启动",
            None,
        );
        return;
    }
    // 注册表状态同步：Starting → Running（终文门/中断/list 视图以注册表为准；
    // 事件与状态在同一提交点后一起发布）。
    {
        let mut core = runtime.core.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(record) = core.agents.get_mut(&agent) {
            if record.state == AgentState::Starting {
                record.state = AgentState::Running;
            }
        }
    }
    runtime.account_event(&running_event.expect("已确认 Ok"));

    // followup 续派任务进上下文。
    if let Some(task) = spec.initial_task.as_deref() {
        spec.ctx.lock().unwrap_or_else(|e| e.into_inner()).push(HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: task_label(task, runtime.group_id, assignment),
            user_input_event: None,
        }));
    }
    // 空闲期排队的消息在新 assignment 开端交付（send 只入队不启动；这些
    // 消息先于本 assignment 受理，作为初始输入进入上下文——§1.3/§1.4）。
    for message in runtime.child_drain_mailbox(&agent, assignment) {
        spec.ctx.lock().unwrap_or_else(|e| e.into_inner()).push(HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: message_label(&message.from, &message.to, runtime.group_id, message.message_id, &message.content),
            user_input_event: None,
        }));
    }

    let terminal = child_loop(&runtime, &spec, &agent, assignment, &owner);
    // 终态发布前收尾本 assignment 进程（final_answer 门内 Complete 路径已
    // 回收，此处兜底幂等——其他终态路径的首次回收）。
    let cleanup_note = match runtime
        .procs
        .reap_agent_assignment(runtime.group_id, &agent, assignment)
    {
        Ok(()) => "已回收本 assignment 所属进程".to_string(),
        Err(e) => format!("进程收尾存在未知结果：{e}"),
    };
    match terminal {
        ChildTerminal::Completed(report) => {
            let r = terminal_report(&report, &cleanup_note);
            runtime.child_publish_terminal(&agent, assignment, AgentState::Completed, "final_answer", Some(r));
        }
        ChildTerminal::Failed(reason) => {
            let r = terminal_report(&reason, &cleanup_note);
            runtime.child_publish_terminal(&agent, assignment, AgentState::Failed, "failed", Some(r));
        }
        ChildTerminal::Interrupted(reason) => {
            runtime.child_publish_terminal(&agent, assignment, AgentState::Interrupted, &reason, None);
        }
    }
    // 最后退出者补写 group_closed（审查 B1：在途事件先于 group_closed 落盘，
    // 装载链校验恒成立；根轮出口已提交时 CAS 短路）。
    if runtime.live_children.fetch_sub(1, Ordering::SeqCst) == 1 {
        let reason = {
            let core = runtime.core.lock().unwrap_or_else(|e| e.into_inner());
            core.close_reason.clone().unwrap_or_else(|| "turn_exit_error".into())
        };
        runtime.child_exit_group_closed(&reason);
    }
}

enum ChildTerminal {
    Completed(String),
    Failed(String),
    Interrupted(String),
}

/// 子轮循环主体（返回终态；所有出口先收尾后发布）。
fn child_loop(
    runtime: &Arc<GroupRuntime>,
    spec: &ChildSpec,
    agent: &str,
    assignment: u64,
    owner: &OwnerTag,
) -> ChildTerminal {
    let cancel_flag: &AtomicBool = &spec.cancel;
    let mut requests = 0usize;
    loop {
        // 动作门：取消/组关闭/墙钟（§1.5）。
        if cancel_flag.load(Ordering::SeqCst) || runtime.group_closed() {
            return ChildTerminal::Interrupted("interrupted".into());
        }
        if Instant::now() >= spec.deadline {
            cancel_flag.store(true, Ordering::SeqCst); // 关动作门
            return ChildTerminal::Interrupted("deadline（assignment 墙钟 300 秒到期）".into());
        }
        // 请求额度（每 assignment 8；根预算独立预留）。
        if requests >= super::MAX_CHILD_REQUESTS_PER_ASSIGNMENT {
            return ChildTerminal::Failed("子模型请求额度耗尽（每 assignment 8 次）".into());
        }
        if let Err(e) = runtime.child_admit_request(agent) {
            return ChildTerminal::Failed(e);
        }
        let reserved = super::RESPONSE_RESERVE_BYTES;
        // 组装（复用普通函数编码；_history 供五协议原生投影）。
        let items = spec.ctx.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let tools = child_tool_definitions(&spec.tools);
        let mut body = crate::prompt::build_turn_request_with(&items, &tools, &spec.system);
        body["_history"] = serde_json::to_value(&items).expect("子上下文可序列化");
        let resp = match spec.sender.send(&body) {
            Ok(resp) => resp,
            Err(e) => {
                runtime.child_release_reserve(reserved, None); // 请求已计数不返还
                return ChildTerminal::Failed(format!("模型请求失败：{e}"));
            }
        };
        requests += 1;
        // 迟到返回门（§1.5：按 group/assignment 关闭门丢弃，不执行
        // tool_calls、不入子上下文——传输排空期间组可能已关闭/被中断）。
        if cancel_flag.load(Ordering::SeqCst) || runtime.group_closed() {
            runtime.child_release_reserve(reserved, None);
            return ChildTerminal::Interrupted(
                "interrupted（响应到达前组已关闭或已中断，迟到返回丢弃）".into(),
            );
        }
        // 解析（镜像根轮协议校验；协议失败＝终态 failed）。
        let Some(message) = resp
            .get("assistant")
            .or_else(|| resp.pointer("/choices/0/message"))
            .cloned()
        else {
            runtime.child_release_reserve(reserved, None);
            return ChildTerminal::Failed("协议错误：响应缺少 choices[0].message".into());
        };
        let native_state = match resp.get("native_state") {
            None => None,
            Some(value) => match serde_json::from_value::<crate::prompt::NativeState>(value.clone())
            {
                Ok(state) => Some(state),
                Err(_) => {
                    runtime.child_release_reserve(reserved, None);
                    return ChildTerminal::Failed("协议错误：原生续接状态无效".into());
                }
            },
        };
        let calls = match ToolCallRecord::parse_batch(&message) {
            Ok(calls) => calls,
            Err(e) => {
                runtime.child_release_reserve(reserved, None);
                return ChildTerminal::Failed(e);
            }
        };
        let content = match message.get("content") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                runtime.child_release_reserve(reserved, None);
                return ChildTerminal::Failed("协议错误：assistant.content 必须是字符串或 null".into());
            }
        };
        if calls.len() > crate::prompt::MAX_TOOL_CALLS_PER_BATCH {
            runtime.child_release_reserve(reserved, None);
            return ChildTerminal::Failed(format!(
                "单批工具调用 {} 条超过上限 32，整批拒绝",
                calls.len()
            ));
        }
        if calls.is_empty() {
            let reply = content.unwrap_or_default();
            if reply.trim().is_empty() {
                runtime.child_release_reserve(reserved, None);
                return ChildTerminal::Failed("模型回复为空".into());
            }
            runtime.child_release_reserve(reserved, None);
            // FINAL_ANSWER 邮箱门（§1.4 竞态在同一门原子判定）。
            match runtime.final_answer_gate(agent, assignment) {
                GateDecision::Continue(messages) => {
                    // 补充输入先受理：交付并继续同一 assignment（不另开轮）。
                    let mut ctx = spec.ctx.lock().unwrap_or_else(|e| e.into_inner());
                    for m in messages {
                        ctx.push(HistoryItem::Text(HistoryMessage {
                            images: None,
                            text_elements: Vec::new(),
                            role: HistoryRole::User,
                            content: message_label(&m.from, &m.to, runtime.group_id, m.message_id, &m.content),
                            user_input_event: None,
                        }));
                    }
                    continue;
                }
                GateDecision::Complete => {
                    // 终文进保留上下文（followup 续接从终文之后继续；持久化
                    // 证据由终态事件的报告承载，不重复 child_record）。
                    spec.ctx
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(with_native(
                            HistoryItem::Text(HistoryMessage {
                                images: None,
                                text_elements: Vec::new(),
                                role: HistoryRole::Assistant,
                                content: reply.clone(),
                                user_input_event: None,
                            }),
                            &native_state,
                        ));
                    return ChildTerminal::Completed(reply);
                }
                GateDecision::Discarded => {
                    return ChildTerminal::Interrupted("interrupted（终文提交前已被中断，晚到回复丢弃）".into());
                }
            }
        }
        // 合法批：assistant 入子上下文（原生状态同条保留——子原生块只回该
        // 子 Agent 的相同连接），child_record 落盘。
        let assistant_item = with_native(
            HistoryItem::AssistantToolCalls {
                tool_calls: calls.clone(),
                content,
            },
            &native_state,
        );
        {
            let mut ctx = spec.ctx.lock().unwrap_or_else(|e| e.into_inner());
            ctx.push(assistant_item.clone());
        }
        match runtime.commit_child_event(
            AgentEventKind::ChildRecord,
            Some(agent),
            Some(assignment),
            AgentEventPayload::Records {
                items: vec![assistant_item],
            },
        ) {
            Ok(event) => {
                // 响应预留按真实记录大小计费（审查 N2：不再以 0 冲销）。
                let size = event_bytes(&event);
                runtime.child_release_reserve(reserved, Some(size));
                runtime.account_event(&event);
            }
            Err(e) => {
                runtime.child_release_reserve(reserved, None); // 失败释放未用预留
                runtime.mark_context_broken(agent);
                return ChildTerminal::Failed(format!("child_record 保存失败：{e}"));
            }
        }
        // 串行执行；取消/门关闭后剩余调用补未知结果（保持配对完整）。
        let mut interrupted_mid_batch = false;
        for call in &calls {
            if interrupted_mid_batch
                || cancel_flag.load(Ordering::SeqCst)
                || runtime.group_closed()
                || Instant::now() >= spec.deadline
            {
                // 未执行调用补通用失败结果（006 配对语义；不伪装执行）。
                let content = json!({ "error": crate::turn::SKIPPED_BY_CANCEL_MESSAGE }).to_string();
                let mut ctx = spec.ctx.lock().unwrap_or_else(|e| e.into_inner());
                ctx.push(HistoryItem::ToolResult {
                    tool_call_id: call.id.clone(),
                    content,
                    display: None,
                    image: None,
                });
                interrupted_mid_batch = true;
                continue;
            }
            // 调用前额度原子准入：不足＝失败结果占位、不执行（§1.6「调用
            // 前额度不足不能触发外部动作」——审查 B2）。
            if let Err(reason) = runtime.child_admit_call(agent) {
                let content = err_result("BUDGET_LIMIT", reason);
                let result_item = HistoryItem::ToolResult {
                    tool_call_id: call.id.clone(),
                    content,
                    display: None,
                    image: None,
                };
                {
                    let mut ctx = spec.ctx.lock().unwrap_or_else(|e| e.into_inner());
                    ctx.push(result_item.clone());
                }
                if let Ok(event) = runtime.commit_child_event(
                    AgentEventKind::ChildRecord,
                    Some(agent),
                    Some(assignment),
                    AgentEventPayload::Records {
                        items: vec![result_item],
                    },
                ) {
                    runtime.account_event(&event);
                }
                continue;
            }
            let call_reserved = super::RESULT_RESERVE_BYTES;
            let (content, display) =
                dispatch_child_tool(runtime, spec, agent, owner, call, cancel_flag);
            let result_item = HistoryItem::ToolResult {
                tool_call_id: call.id.clone(),
                content: content.clone(),
                display: display.clone(),
                image: None,
            };
            {
                let mut ctx = spec.ctx.lock().unwrap_or_else(|e| e.into_inner());
                ctx.push(result_item.clone());
            }
            let committed = runtime.commit_child_event(
                AgentEventKind::ChildRecord,
                Some(agent),
                Some(assignment),
                AgentEventPayload::Records {
                    items: vec![result_item],
                },
            );
            match committed {
                Ok(event) => {
                    let size = event_bytes(&event);
                    runtime.child_release_reserve(call_reserved, Some(size));
                    runtime.account_event(&event);
                }
                Err(_) => {
                    runtime.child_release_reserve(call_reserved, None);
                    runtime.mark_context_broken(agent);
                    return ChildTerminal::Failed("child_record 保存失败（工具已执行，结果记录失败）".into());
                }
            }
        }
        if interrupted_mid_batch {
            return ChildTerminal::Interrupted("interrupted（批内取消，剩余调用未执行）".into());
        }
        // 批末收取邮箱（完整工具批后、下一次模型请求前；§1.4）。
        let messages = runtime.child_drain_mailbox(agent, assignment);
        if !messages.is_empty() {
            let mut ctx = spec.ctx.lock().unwrap_or_else(|e| e.into_inner());
            for m in messages {
                ctx.push(HistoryItem::Text(HistoryMessage {
                    images: None,
                    text_elements: Vec::new(),
                    role: HistoryRole::User,
                    content: message_label(&m.from, &m.to, runtime.group_id, m.message_id, &m.content),
                    user_input_event: None,
                }));
            }
        }
    }
}

/// 子工具真实分派：业务工具经既有执行面（033 包装/031 基线快照/016 句柄
/// 归属校验）；send_message 仅向根；伪造/未声明名称零执行。
fn dispatch_child_tool(
    runtime: &Arc<GroupRuntime>,
    spec: &ChildSpec,
    agent: &str,
    owner: &OwnerTag,
    call: &ToolCallRecord,
    cancel: &AtomicBool,
) -> (String, Option<crate::prompt::ToolDisplay>) {
    // 子 send_message：target 只能精确 /root。
    if call.name == SEND_TOOL_NAME {
        let outcome = match parse_target_message(&call.arguments, SEND_TOOL_NAME) {
            Err(e) => err_result("INVALID_ARGUMENT", e),
            Ok(args) if args.target != ROOT_PATH => err_result(
                "INVALID_ARGUMENT",
                format!("子 Agent 的 send_message 目标只能是 {ROOT_PATH}（不能联系兄弟或真实用户）"),
            ),
            Ok(args) => match runtime.child_send_to_root(agent, spec.assignment, &args.message) {
                Ok(message_id) => ok_result(json!({
                    "message_id": message_id,
                    "accepted": true,
                })),
                Err(e) => err_result("PERSISTENCE_FAILED", e),
            },
        };
        return (outcome, None);
    }
    // 023 资源工具（快照能力决定）。
    if spec
        .tools
        .mcp
        .as_ref()
        .is_some_and(|s| s.has_resources())
        && crate::mcp::resources::is_resource_tool(&call.name)
    {
        let (content, _failed) = spec
            .tools
            .mcp
            .as_ref()
            .expect("已确认存在")
            .call_resource_tool(&call.name, &call.arguments, cancel);
        return (content, None);
    }
    // 022 动态 MCP：目录精确命中＋出生时冻结的已声明集合。
    if let Some(entry) = spec.tools.mcp.as_ref().and_then(|s| s.entry(&call.name)) {
        if spec.tools.declared_dynamic.contains(&entry.model_name) {
            let (content, _failed) = match serde_json::from_str::<Value>(&call.arguments) {
                Err(e) => (
                    crate::mcp::call::client_error_json(
                        "参数",
                        &format!("arguments 不是有效 JSON：{e}"),
                    ),
                    true,
                ),
                Ok(v) if !v.is_object() => (
                    crate::mcp::call::client_error_json("参数", "arguments 必须是 JSON 对象"),
                    true,
                ),
                Ok(v) => spec
                    .tools
                    .mcp
                    .as_ref()
                    .expect("已确认命中")
                    .call(entry, v, cancel),
            };
            return (content, None);
        }
    }
    // view_image（能力快照；031 基线快照内读取——不继承根本轮额外授权）。
    if call.name == "view_image" {
        if !spec.tools.view_image {
            return (
                crate::view_image::unsupported_outcome().summary,
                None,
            );
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let snap = crate::permissions::frozen().map(|f| {
            crate::permissions::build_snapshot(&f.policy, &Default::default(), &f.deny_ctx)
        });
        let outcome = match snap {
            Some(Ok(snapshot)) => {
                crate::view_image::execute_sandboxed(&call.arguments, &cwd, Some(cancel), &snapshot)
            }
            Some(Err(e)) => crate::view_image::error_outcome(&format!(
                "生效策略无法编码：{e}（本次读取拒绝）"
            )),
            None => crate::view_image::execute(&call.arguments, &cwd, Some(cancel)),
        };
        return (outcome.summary, None);
    }
    // 核心四工具（exec/write_stdin 按归属标签；apply_patch/update_plan 与
    // 根同源；未知工具通用错误）。
    crate::turn::execute_core_tool_owned(
        &call.name,
        &call.arguments,
        Some(cancel),
        &runtime.procs,
        None,
        owner,
    )
}

/// 与根轮 with_native 相同的包装（子上下文内保留原生状态）。
fn with_native(message: HistoryItem, state: &Option<crate::prompt::NativeState>) -> HistoryItem {
    match state {
        Some(state) => HistoryItem::NativeAssistant {
            message: Box::new(message),
            state: Some(state.clone()),
        },
        None => message,
    }
}

/// 六工具名判定（turn 分派门）。
pub fn is_agent_tool(name: &str) -> bool {
    matches!(
        name,
        SPAWN_TOOL_NAME
            | SEND_TOOL_NAME
            | FOLLOWUP_TOOL_NAME
            | WAIT_TOOL_NAME
            | INTERRUPT_TOOL_NAME
            | LIST_TOOL_NAME
    )
}

/// 根侧六工具入口（turn 分派；`raw`＝arguments 原文；`spawn_ctx`＝调用方
/// 按当前请求作用域预构建的冻结上下文）。wait 的 steer 探测由调用方注入
///（SteerChannel 非消费性检查）。
pub fn execute_control_tool(
    runtime: &Arc<GroupRuntime>,
    name: &str,
    raw: &str,
    history: &mut Vec<HistoryItem>,
    spawn_ctx: &SpawnContext<'_>,
    steer_pending: &dyn Fn() -> bool,
) -> String {
    match name {
        SPAWN_TOOL_NAME => match parse_spawn(raw) {
            Err(e) => err_result("INVALID_ARGUMENT", e),
            Ok(args) => finish(runtime.spawn(history, &args, spawn_ctx), |d| d),
        },
        SEND_TOOL_NAME => match parse_target_message(raw, SEND_TOOL_NAME) {
            Err(e) => err_result("INVALID_ARGUMENT", e),
            Ok(args) => finish(runtime.send_from_root(history, &args), |d| d),
        },
        FOLLOWUP_TOOL_NAME => match parse_target_message(raw, FOLLOWUP_TOOL_NAME) {
            Err(e) => err_result("INVALID_ARGUMENT", e),
            Ok(args) => finish(runtime.followup(history, &args, spawn_ctx), |d| d),
        },
        INTERRUPT_TOOL_NAME => match parse_target(raw, INTERRUPT_TOOL_NAME) {
            Err(e) => err_result("INVALID_ARGUMENT", e),
            Ok(args) => finish(runtime.interrupt(history, &args), |d| d),
        },
        LIST_TOOL_NAME => match parse_list(raw, LIST_TOOL_NAME) {
            Err(e) => err_result("INVALID_ARGUMENT", e),
            Ok(args) => finish(runtime.list(&args), |d| json!({ "agents": d["agents"] })),
        },
        WAIT_TOOL_NAME => match parse_wait(raw, WAIT_TOOL_NAME) {
            Err(e) => err_result("INVALID_ARGUMENT", e),
            Ok(args) => runtime.wait(&args, steer_pending),
        },
        other => err_result(
            "INVALID_ARGUMENT",
            format!("未知多 Agent 控制工具 {other}"),
        ),
    }
}

/// spawn 成功形（data 直接展开；err 透传）。
fn finish(result: Result<Value, ToolError>, ok: impl FnOnce(Value) -> Value) -> String {
    match result {
        Ok(data) => ok_result(ok(data)),
        Err((code, message)) => err_result(code, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局槽计数与进程内其他测试共享（进程级原子量）——串行化使用，
    /// 并按增量断言而非绝对值（033「覆盖缝读者与写者同时串行」同教训）。
    static SLOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn global_slot_reserve_release() {
        let _guard = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst);
        // 预留由 try_reserve_slot 完成（spawn 先预留再交子线程 guard 释放）。
        assert!(try_reserve_slot());
        assert!(try_reserve_slot());
        assert_eq!(
            GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst),
            before + 2,
            "两次预留各占一槽"
        );
        // 预留到满额后必须拒绝（不溢出、不泄槽）
        while GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst) < MAX_CHILD_SLOTS {
            assert!(try_reserve_slot());
        }
        assert!(!try_reserve_slot(), "满额（{MAX_CHILD_SLOTS}）后拒绝");
        // guard 释放一槽后可再取
        drop(SlotGuard);
        assert!(GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst) < MAX_CHILD_SLOTS);
        assert!(try_reserve_slot());
        // 清理本测试的全部预留（guard 无法逐个 drop——直接按差值归还）
        let taken = GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst) - before;
        GLOBAL_CHILD_SLOTS.fetch_sub(taken, Ordering::SeqCst);
        assert_eq!(GLOBAL_CHILD_SLOTS.load(Ordering::SeqCst), before);
    }

    #[test]
    fn agent_tool_name_gate() {
        for name in TOOL_NAMES_ALL {
            assert!(is_agent_tool(name), "{name} 应被识别");
        }
        assert!(!is_agent_tool("exec_command"));
        assert!(!is_agent_tool("send_message_to_user_async"));
    }

    static TOOL_NAMES_ALL: [&str; 6] = [
        SPAWN_TOOL_NAME,
        SEND_TOOL_NAME,
        FOLLOWUP_TOOL_NAME,
        WAIT_TOOL_NAME,
        INTERRUPT_TOOL_NAME,
        LIST_TOOL_NAME,
    ];
}
