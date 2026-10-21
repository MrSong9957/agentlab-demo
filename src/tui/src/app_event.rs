//! 008-ADAPT（本地新写，非母本 vendor）：AppEvent 子集。
//!
//! 母本 `app_event.rs`（1,773 行）是 codex 全功能应用的事件总线（webrtc/plugins/
//! file_search/approvals…）。008 §3-J5 把事件边界收窄为本地驱动（ChatDriver）实际
//! 生产与消费的子集：保留代码中生产发送点仅 `AppendMessageHistoryEntry`、
//! `InsertHistoryCell`、`LookupMessageHistoryEntry`（chat_composer_history）、
//! `TranscriptCopyClosed`、`OpenUrlInBrowser`；`Interrupt` 承接母本
//! `AppEventSender::interrupt()`（母本发 `CodexOp(AppCommand::interrupt())`）。
//! 载荷类型取 shim/本地最小定义；被裁功能（文件搜索/审批/模型弹窗等）的发送点
//! 随 008-TRIM 一并移除，不进入本枚举。

use crate::history_cell::HistoryCell;
use codex_protocol::ThreadId;

#[derive(Debug)]
pub enum AppEvent {
    /// 母本变体同义：把已完成的 cell 插入应用自持历史（Owned transcript）。
    InsertHistoryCell(Box<dyn HistoryCell>),
    /// 母本变体同义：composer 历史追加（chat_composer_history）。
    AppendMessageHistoryEntry { thread_id: ThreadId, text: String },
    /// 母本变体同义：按 offset/log_id 查找历史条目（chat_composer_history 搜索；
    /// log_id 为本地历史日志号 u64——对齐消费点）。
    LookupMessageHistoryEntry {
        thread_id: ThreadId,
        offset: usize,
        log_id: u64,
    },
    /// 母本变体同义：transcript 复制模式关闭通知。
    TranscriptCopyClosed,
    /// 母本变体同义：在浏览器打开链接（hyperlink 点击）。
    OpenUrlInBrowser { url: String },
    /// 母本变体同义：按游标批量查找历史（chat_composer_history 批量回填；
    /// 游标为 message-history 游标，log_id 为本地历史日志号 u64——对齐消费点）。
    LookupMessageHistoryBatch {
        thread_id: ThreadId,
        cursor: codex_message_history::HistoryBatchCursor,
        log_id: u64,
    },
    /// 本地变体：承接母本 `AppEventSender::interrupt()`（Esc 中断当前任务）。
    Interrupt,
}

/// 008-ADAPT：母本 app_event.rs 的 composer 输入历史查找响应（chat_composer_history 流程保留）。
/// 载荷为本地最小定义：游标以 u64 承载（母本 codex_message_history::HistoryBatchCursor），
/// 条目为 (id, text) 二元组语义（母本 HistoryEntry）。
#[derive(Debug, Clone)]
pub enum HistoryLookupResponse {
    Entry {
        offset: usize,
        log_id: u64,
        entry: Option<String>,
    },
    Batch {
        cursor: codex_message_history::HistoryBatchCursor,
        log_id: u64,
        entries: Vec<HistoryBatchEntryResponse>,
        next_older_cursor: Option<codex_message_history::HistoryBatchCursor>,
    },
    BatchError {
        cursor: codex_message_history::HistoryBatchCursor,
        log_id: u64,
    },
}

/// 008-ADAPT：母本 app_event.rs 的历史批量条目响应载荷（chat_composer_history 批量回填；
/// 字段形状对齐消费点 search_batch.rs：逐条 offset/entry）。
#[derive(Debug, Clone)]
pub struct HistoryBatchEntryResponse {
    pub offset: usize,
    pub entry: Option<String>,
}
