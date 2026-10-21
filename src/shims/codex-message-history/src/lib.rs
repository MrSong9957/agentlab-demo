//! codex-message-history shim（008 §3-J5）：仅 HistoryBatchCursor（chat_composer_history 消费）。
/// 母本 message-history/src/batch.rs HistoryBatchCursor 的最小等价：
/// 绝对历史偏移游标（end_offset）；本地历史为内存全量，锚点字段未消费不引入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryBatchCursor {
    pub(crate) end_offset: usize,
}

impl HistoryBatchCursor {
    pub fn new(end_offset: usize) -> Self {
        Self { end_offset }
    }

    pub fn initial(end_offset: usize) -> Self {
        Self::new(end_offset)
    }
    pub fn end_offset(&self) -> usize {
        self.end_offset
    }
}
