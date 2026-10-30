//! 035：每 Agent 独立 FIFO 邮箱（§1.4）——宿主生成递增 message_id 与发送方
//! 身份，保存 `message_queued` 后才回 accepted；相同文字的两次合法调用是
//! 两条消息，不凭正文去重。接收者在完整工具批次后、下一次模型请求前按
//! 受理顺序收取。每接收者未交付上限 32 条/256 KiB，超限先拒绝（不先
//! accepted 后丢弃）；中断/组关闭时未交付补充输入记 abandoned，不在未来
//! 任务中悄悄执行。

use super::{
    MAX_PENDING_MESSAGE_BYTES, MAX_PENDING_MESSAGES,
};

/// 一条排队消息（原始正文原样保存；from/to 为宿主路径身份）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMessage {
    pub message_id: u64,
    pub from: String,
    pub to: String,
    pub content: String,
}

/// 每 Agent 邮箱：FIFO＋接收者级容量核算（组级计数在 GroupCore，准入时
/// 一并检查——两层都过才受理）。
#[derive(Debug, Default)]
pub struct Mailbox {
    queue: std::collections::VecDeque<QueuedMessage>,
    pending_bytes: usize,
    /// 已按批交付（保留计数供核算；正文已进接收者上下文）。
    delivered: usize,
}

impl Mailbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// 受理一条消息（容量已由调用方核对；此处只入队与计数）。
    pub fn push(&mut self, message: QueuedMessage) {
        self.pending_bytes += message.content.as_bytes().len();
        self.queue.push_back(message);
    }

    /// 接收者级容量检查（条数/字节）。
    pub fn has_room(&self, content_bytes: usize) -> bool {
        self.queue.len() < MAX_PENDING_MESSAGES
            && self.pending_bytes + content_bytes <= MAX_PENDING_MESSAGE_BYTES
    }

    pub fn pending_len(&self) -> usize {
        self.queue.len()
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    /// 是否有未交付消息（wait/final-answer 邮箱门判定用）。
    pub fn has_pending(&self) -> bool {
        !self.queue.is_empty()
    }

    /// 批末收取全部未交付消息（按受理顺序；FIFO）。
    pub fn drain(&mut self) -> Vec<QueuedMessage> {
        let out: Vec<QueuedMessage> = self.queue.drain(..).collect();
        self.pending_bytes = 0;
        self.delivered += out.len();
        out
    }

    /// 中断/组关闭：放弃未交付补充输入（调用方负责记录 abandoned 事件与
    /// 原因）；返回被放弃的消息（供事件记录）。
    pub fn abandon(&mut self) -> Vec<QueuedMessage> {
        self.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: u64, content: &str) -> QueuedMessage {
        QueuedMessage {
            message_id: id,
            from: "/root".into(),
            to: "/root/a".into(),
            content: content.into(),
        }
    }

    #[test]
    fn fifo_order_and_dedup_free() {
        let mut mb = Mailbox::new();
        mb.push(msg(1, "a"));
        mb.push(msg(2, "a")); // 相同正文＝两条消息
        assert_eq!(mb.pending_len(), 2);
        let drained = mb.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message_id, 1);
        assert_eq!(drained[1].message_id, 2);
        assert!(!mb.has_pending());
    }

    #[test]
    fn capacity_limits() {
        let mut mb = Mailbox::new();
        for i in 0..MAX_PENDING_MESSAGES {
            assert!(mb.has_room(1), "第 {i} 条应有空位");
            mb.push(msg(i as u64 + 1, "x"));
        }
        assert!(!mb.has_room(1), "条数满额后拒绝");
        // 清空后字节上限
        let mut mb = Mailbox::new();
        let big = "x".repeat(MAX_PENDING_MESSAGE_BYTES);
        assert!(mb.has_room(big.len()));
        mb.push(msg(1, &big));
        assert!(!mb.has_room(1), "字节满额后拒绝");
    }
}
