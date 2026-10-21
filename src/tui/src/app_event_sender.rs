//! 008-ADAPT（本地新写，非母本 vendor）：AppEventSender 子集。
//!
//! 母本 `app_event_sender.rs`（171 行）包装 `UnboundedSender<AppEvent>` 并提供全功能
//! 请求方法（审批/评审/技能列表…）。008 §3-J5 仅保留本地驱动用到的 `send`/`clone`
//! /`interrupt`；其余方法随裁剪功能不进入。

use tokio::sync::mpsc::UnboundedSender;

use crate::app_event::AppEvent;

#[derive(Clone)]
pub(crate) struct AppEventSender {
    pub(crate) app_event_tx: UnboundedSender<AppEvent>,
}

impl AppEventSender {
    pub(crate) fn new(app_event_tx: UnboundedSender<AppEvent>) -> Self {
        Self { app_event_tx }
    }

    pub(crate) fn send(&self, event: AppEvent) {
        let _ = self.app_event_tx.send(event);
    }

    /// 母本语义：请求中断当前任务（母本发 `CodexOp(AppCommand::interrupt())`）。
    pub(crate) fn interrupt(&self) {
        self.send(AppEvent::Interrupt);
    }
}

impl std::fmt::Debug for AppEventSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppEventSender").finish_non_exhaustive()
    }
}
