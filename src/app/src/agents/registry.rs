//! 035：注册表与状态机（§1.5/§1.6）——一组最多 8 个 Agent 记录、8 次
//! assignment 启动、3 个全应用子执行槽（starting/running/stopping 计槽）；
//! 满额原子拒绝，不静默淘汰上下文或复用名称。预算（子模型请求 24/组、
//! 子工具调用 256/组、消息 128 条/1 MiB 每组、事件负载 64 MiB/组）全部
//! 在 `GroupCore` 内集中核算；准入＝「检查＋扣除」在同一锁段完成（原子
//! 拒绝不泄槽）。路径解析沿用 [`crate::agents::args::parse_target_path`]
//! 的严格规则；不存在/他组/旧轮统一 AGENT_NOT_FOUND，不泄露另一组是否存在。

use super::mailbox::Mailbox;
use super::{
    MAX_AGENTS_PER_GROUP, MAX_ASSIGNMENTS_PER_GROUP, MAX_CHILD_CALLS_PER_ASSIGNMENT,
    MAX_CHILD_CALLS_PER_GROUP, MAX_CHILD_REQUESTS_PER_ASSIGNMENT, MAX_CHILD_REQUESTS_PER_GROUP,
    MAX_GROUP_EVENT_BYTES, MAX_GROUP_MESSAGE_BYTES, MAX_GROUP_MESSAGES, MAX_TERMINAL_REPORTS,
};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Agent 状态机（§1.5 表）：starting/running/stopping 计执行槽；
/// completed/failed/interrupted 不占槽但保留上下文供同轮续派；closed＝组
/// 关闭后的历史形态，不可重新唤醒。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Starting,
    Running,
    Stopping,
    Completed,
    Failed,
    Interrupted,
    Closed,
}

impl AgentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Closed => "closed",
        }
    }

    /// 计执行槽（starting/running/stopping；含在途排空）。
    pub fn occupies_slot(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }

    /// 终态（本次 assignment 结束、上下文保留、可接消息与显式续派）。
    pub fn is_terminal_idle(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Interrupted)
    }
}

/// 一个 Agent 记录（组内唯一名；跨 assignment 保留）。
pub struct AgentRecord {
    pub name: String,
    pub state: AgentState,
    /// 当前（或最近一次）assignment 编号（组内递增，自 1）。
    pub assignment_id: u64,
    /// 最近初始任务/续派任务正文（不被普通 send_message 覆盖；展示截断
    /// 副本由 list 视图生成，原文保留）。
    pub last_task_message: String,
    /// 本 Agent 累计 assignment 数。
    pub assignments: usize,
    /// 本 Agent 当前 assignment 已用子模型请求数。
    pub requests_used: usize,
    /// 本 Agent 当前 assignment 已受理子工具调用数。
    pub calls_used: usize,
    /// failed 且原生上下文损坏（截断/坏签名）时为 false——followup 返回
    /// CONTEXT_UNAVAILABLE，不能拿坏签名续接（§1.5）。
    pub context_continuable: bool,
    /// 本 assignment 终态是否已发布（恰一次；followup 新 assignment 复位）。
    pub terminal_emitted: bool,
    /// 取消标志（interrupt/组关闭置位；子轮在动作门与等待处检查）。
    pub cancel: Arc<AtomicBool>,
    pub mailbox: Mailbox,
}

impl AgentRecord {
    fn new(name: String, assignment_id: u64, task: String, cancel: Arc<AtomicBool>) -> Self {
        Self {
            name,
            state: AgentState::Starting,
            assignment_id,
            last_task_message: task,
            assignments: 1,
            requests_used: 0,
            calls_used: 0,
            context_continuable: true,
            terminal_emitted: false,
            cancel,
            mailbox: Mailbox::new(),
        }
    }
}

/// 组预算/注册表核心（全组单锁；不含 I/O）。事件字节含预留窗口：
/// `event_bytes_reserved` 在子模型请求/工具执行前预留，提交后按真实大小
/// 计费并释放未用预留（§1.6）。
pub struct GroupCore {
    pub group_id: u64,
    /// 组事件宿主递增序号（只在提交成功后占用——034 审查 P1-3 同教训）。
    pub seq: u64,
    pub agents: BTreeMap<String, AgentRecord>,
    pub closed: bool,
    pub close_reason: Option<String>,
    /// 已启动 assignment 总数（初始＋空闲续派；≤8）。
    pub assignments_started: usize,
    /// 下一个 assignment 编号（自 1）。
    pub next_assignment_id: u64,
    /// 递增 message_id（宿主生成）。
    pub next_message_id: u64,
    /// 组级子模型请求累计（发送失败/取消也计数；followup 不清）。
    pub child_requests_group: usize,
    /// 组级子工具调用累计（含失败调用）。
    pub child_calls_group: usize,
    /// 组级消息累计（初始任务/续派/Agent 消息；条数与字节）。
    pub group_messages: usize,
    pub group_message_bytes: usize,
    /// 组级子事件负载累计（已提交字节）。
    pub event_bytes_committed: u64,
    /// 在途预留（响应/结果记录预留之和）。
    pub event_bytes_reserved: u64,
    /// 终态报告槽（每 assignment 预留一个；≤8 份）。
    pub terminal_reports: usize,
}

impl GroupCore {
    pub fn new(group_id: u64) -> Self {
        Self {
            group_id,
            seq: 0,
            agents: BTreeMap::new(),
            closed: false,
            close_reason: None,
            assignments_started: 0,
            next_assignment_id: 1,
            next_message_id: 1,
            child_requests_group: 0,
            child_calls_group: 0,
            group_messages: 0,
            group_message_bytes: 0,
            event_bytes_committed: 0,
            event_bytes_reserved: 0,
            terminal_reports: 0,
        }
    }

    /// 组消息准入检查（条数/字节；§1.6 消息行）。超限先拒绝。
    pub fn group_message_room(&self, content_bytes: usize) -> bool {
        self.group_messages < MAX_GROUP_MESSAGES
            && self.group_message_bytes + content_bytes <= MAX_GROUP_MESSAGE_BYTES
    }

    /// 组事件负载准入：已提交＋在途预留＋新预留 ≤ 64 MiB。
    pub fn event_room(&self, reserve: u64) -> bool {
        self.event_bytes_committed
            .saturating_add(self.event_bytes_reserved)
            .saturating_add(reserve)
            <= MAX_GROUP_EVENT_BYTES as u64
    }

    /// 原子预留一个新 assignment：检查 Agent 数/assignment 数/（执行槽由
    /// 调用方在全局层核对）。返回新 assignment 编号；失败返回原因（不含
    /// 代码——调用方映射）。
    pub fn reserve_assignment(&mut self, name: &str) -> Result<u64, String> {
        if self.closed {
            return Err("当前组已关闭，旧路径不可执行".into());
        }
        if self.agents.contains_key(name) {
            return Err(format!("任务名 {name} 已在当前组使用（完成后的名称用 followup_task 续用）"));
        }
        if self.agents.len() >= MAX_AGENTS_PER_GROUP {
            return Err(format!("当前组 Agent 记录已达上限 {MAX_AGENTS_PER_GROUP}"));
        }
        if self.assignments_started >= MAX_ASSIGNMENTS_PER_GROUP {
            return Err(format!("当前组 assignment 启动已达上限 {MAX_ASSIGNMENTS_PER_GROUP}"));
        }
        let assignment_id = self.next_assignment_id;
        self.next_assignment_id += 1;
        self.assignments_started += 1;
        Ok(assignment_id)
    }

    /// 发布 spawn：登记 Agent 记录（此前 reserve_assignment 已扣额度）。
    pub fn publish_spawn(
        &mut self,
        name: &str,
        assignment_id: u64,
        task: String,
    ) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        let record = AgentRecord::new(name.to_string(), assignment_id, task, cancel.clone());
        self.agents.insert(name.to_string(), record);
        cancel
    }

    /// spawn 保存失败后的额度回滚（未发布任何记录）。
    pub fn rollback_assignment(&mut self) {
        self.assignments_started = self.assignments_started.saturating_sub(1);
        self.next_assignment_id = self.next_assignment_id.saturating_sub(1);
    }

    /// 子模型请求准入：组 24/assignment 8 原子扣除（发送失败/取消也计数，
    /// 不返还，§1.6）。
    pub fn admit_child_request(&mut self, agent: &str) -> Result<(), String> {
        let record = self
            .agents
            .get_mut(agent)
            .ok_or_else(|| format!("Agent {agent} 不存在"))?;
        if record.requests_used >= MAX_CHILD_REQUESTS_PER_ASSIGNMENT {
            return Err(format!(
                "该 assignment 子模型请求已达上限 {MAX_CHILD_REQUESTS_PER_ASSIGNMENT}"
            ));
        }
        if self.child_requests_group >= MAX_CHILD_REQUESTS_PER_GROUP {
            return Err(format!(
                "整组子模型请求累计已达上限 {MAX_CHILD_REQUESTS_PER_GROUP}"
            ));
        }
        record.requests_used += 1;
        self.child_requests_group += 1;
        Ok(())
    }

    /// 子工具调用准入：assignment 64/组 256 原子扣除（含 send_message 与
    /// 失败调用，不返还）。
    pub fn admit_child_call(&mut self, agent: &str) -> Result<(), String> {
        let record = self
            .agents
            .get_mut(agent)
            .ok_or_else(|| format!("Agent {agent} 不存在"))?;
        if record.calls_used >= MAX_CHILD_CALLS_PER_ASSIGNMENT {
            return Err(format!(
                "该 assignment 子工具调用已达上限 {MAX_CHILD_CALLS_PER_ASSIGNMENT}"
            ));
        }
        if self.child_calls_group >= MAX_CHILD_CALLS_PER_GROUP {
            return Err(format!("整组子工具调用已达上限 {MAX_CHILD_CALLS_PER_GROUP}"));
        }
        record.calls_used += 1;
        self.child_calls_group += 1;
 Ok(())
    }

    /// 事件字节预留（子模型请求前 10 MiB／工具结果前 2 MiB）。
    pub fn reserve_event_bytes(&mut self, bytes: u64) {
        self.event_bytes_reserved += bytes;
    }

    /// 预算释放与真实计费：提交真实大小、归还未用预留。
    pub fn commit_event_bytes(&mut self, reserved: u64, actual: u64) {
        self.event_bytes_reserved = self.event_bytes_reserved.saturating_sub(reserved);
        self.event_bytes_committed = self.event_bytes_committed.saturating_add(actual);
    }

    /// 组消息计费（受理成功后）。
    pub fn commit_group_message(&mut self, content_bytes: usize) {
        self.group_messages += 1;
        self.group_message_bytes += content_bytes;
    }

    /// 终态报告槽检查（每 assignment 一个，≤8 份＝assignment 上限同数）。
    pub fn terminal_report_room(&self) -> Result<(), String> {
        if self.terminal_reports >= MAX_TERMINAL_REPORTS {
            return Err(format!("终态报告槽已达上限 {MAX_TERMINAL_REPORTS}"));
        }
        Ok(())
    }

    pub fn commit_terminal_report(&mut self) {
        self.terminal_reports += 1;
    }

    /// 组内活动 assignment 是否存在（starting/running/stopping——根终文
    /// 封口检查用，§1.5）。
    pub fn has_active_assignments(&self) -> bool {
        self.agents.values().any(|r| r.state.occupies_slot())
    }

    /// 解析根侧 target（相对名或 /root/name → 当前组 Agent 记录引用下标
    /// 形态）。组关闭后一律 AGENT_NOT_FOUND 语义（统一由调用方映射）。
    pub fn resolve_child(&self, target: &str) -> Result<&AgentRecord, String> {
        let name = super::args::parse_target_path(target)?
            .ok_or_else(|| "根不能用 /root 作为子工具目标".to_string())?;
        match self.agents.get(&name) {
            Some(record) if !matches!(record.state, AgentState::Closed) => Ok(record),
            _ => Err(format!(
                "目标 {target} 不在当前组（旧轮/其他组路径不可执行，且不泄露其是否存在）"
            )),
        }
    }

    pub fn resolve_child_mut(&mut self, target: &str) -> Result<&mut AgentRecord, String> {
        let name = super::args::parse_target_path(target)?
            .ok_or_else(|| "根不能用 /root 作为子工具目标".to_string())?;
        match self.agents.get_mut(&name) {
            Some(record) if !matches!(record.state, AgentState::Closed) => Ok(record),
            _ => Err(format!(
                "目标 {target} 不在当前组（旧轮/其他组路径不可执行，且不泄露其是否存在）"
            )),
        }
    }

    /// 关闭整组：全部 Agent 置 closed，返回被中断 Agent 的取消标志与
    /// 邮箱放弃消息（供事件记录）。
    pub fn close_all(&mut self, reason: &str) {
        for (_, record) in self.agents.iter_mut() {
            record.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            record.state = AgentState::Closed;
            record.mailbox.abandon();
        }
        self.closed = true;
        self.close_reason = Some(reason.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> GroupCore {
        GroupCore::new(1)
    }

    #[test]
    fn reserve_publish_rollback_limits() {
        let mut c = core();
        for i in 0..MAX_AGENTS_PER_GROUP {
            let id = c.reserve_assignment(&format!("a{i}")).unwrap();
            c.publish_spawn(&format!("a{i}"), id, "t".into());
        }
        assert!(c.reserve_assignment("extra").is_err(), "第 9 个 Agent 拒绝");
        // 名称去重：当前组名称不可重复（含 completed 名——spawn 不复用，走 followup）
        let mut c2 = core();
        let id = c2.reserve_assignment("dup").unwrap();
        c2.publish_spawn("dup", id, "t".into());
        assert!(c2.reserve_assignment("dup").is_err());
        // 回滚恢复额度
        let mut c3 = core();
        c3.reserve_assignment("x").unwrap();
        c3.rollback_assignment();
        assert_eq!(c3.assignments_started, 0);
        assert_eq!(c3.next_assignment_id, 1);
    }

    #[test]
    fn request_and_call_budgets_atomic() {
        let mut c = core();
        let id = c.reserve_assignment("a").unwrap();
        c.publish_spawn("a", id, "t".into());
        for _ in 0..MAX_CHILD_REQUESTS_PER_ASSIGNMENT {
            c.admit_child_request("a").unwrap();
        }
        assert!(c.admit_child_request("a").is_err(), "assignment 请求满额");
        // 组级
        let mut c2 = core();
        for i in 0..3 {
            let id = c2.reserve_assignment(&format!("a{i}")).unwrap();
            c2.publish_spawn(&format!("a{i}"), id, "t".into());
        }
        for round in 0..MAX_CHILD_REQUESTS_PER_GROUP {
            let agent = format!("a{}", round % 3);
            c2.admit_child_request(&agent).unwrap();
        }
        // 每个用了 8 次（3×8=24）；再扣任何 agent 都到组上限或 assignment 上限
        for i in 0..3 {
            assert!(c2.admit_child_request(&format!("a{i}")).is_err());
        }
        let mut c3 = core();
        let id = c3.reserve_assignment("a").unwrap();
        c3.publish_spawn("a", id, "t".into());
        for _ in 0..MAX_CHILD_CALLS_PER_ASSIGNMENT {
            c3.admit_child_call("a").unwrap();
        }
        assert!(c3.admit_child_call("a").is_err(), "assignment 调用满额");
    }

    #[test]
    fn event_reserve_commit_release() {
        let mut c = core();
        assert!(c.event_room(super::super::RESPONSE_RESERVE_BYTES));
        c.reserve_event_bytes(super::super::RESPONSE_RESERVE_BYTES);
        assert!(!c.event_room(u64::MAX / 2), "预留窗口计入上限");
        c.commit_event_bytes(super::super::RESPONSE_RESERVE_BYTES, 1024);
        assert_eq!(c.event_bytes_committed, 1024);
        assert_eq!(c.event_bytes_reserved, 0);
    }

    #[test]
    fn resolve_and_close_semantics() {
        let mut c = core();
        // 空组：任何目标 AGENT_NOT_FOUND 语义（含 /root 作为子目标拒绝）
        assert!(c.resolve_child("/root").is_err());
        assert!(c.resolve_child("ghost").is_err());
        assert!(!c.has_active_assignments(), "无记录时无活动");
        let id = c.reserve_assignment("a").unwrap();
        c.publish_spawn("a", id, "t".into());
        assert!(c.resolve_child("a").is_ok());
        assert!(c.resolve_child("/root/a").is_ok());
        assert!(c.resolve_child("b").is_err());
        assert!(c.has_active_assignments(), "starting 计活动槽");
        c.close_all("turn_exit");
        assert!(c.closed);
        assert!(c.resolve_child("a").is_err(), "组关闭后旧路径不可执行");
    }
}
