//! 008-ADAPT（本地提取，非母本 chatwidget vendor）：
//! 母本 chatwidget.rs 33,820 行不整体移植（008 §1）；此处仅承载保留模块
//! （transcript_view/pager_overlay 等）消费的类型提取，逐类型注明母本行号。

/// 母本 chatwidget.rs:801 ActiveCellTranscriptKey——逐字提取（doc 注释保留）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ActiveCellTranscriptKey {
    /// Whether unchanged keys may reuse the live layout across redraws.
    /// External state can change a cell without advancing the widget's revision.
    pub(crate) cacheable: bool,
    /// Cache-busting revision for in-place updates.
    ///
    /// Many active cells are updated incrementally while streaming (for example when exec groups
    /// add output or change status), and the transcript overlay caches its live tail, so this
    /// revision gives a cheap way to say "same active cell, but its transcript output is different
    /// now". Callers bump it on any mutation that can affect `HistoryCell::transcript_lines`.
    pub(crate) revision: u64,
    /// Whether the active cell continues the prior stream, which affects
    /// spacing between transcript blocks.
    pub(crate) is_stream_continuation: bool,
    /// Optional animation tick for time-dependent transcript output.
    ///
    /// When this changes, the overlay recomputes the cached tail even if the revision and width
    /// are unchanged, which is how shimmer/spinner visuals can animate in the overlay without any
    /// underlying data change.
    pub(crate) animation_tick: Option<u64>,
}
