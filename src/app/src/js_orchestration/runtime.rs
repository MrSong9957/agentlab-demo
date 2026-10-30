//! 034 宿主侧 cell 运行时：worker 子进程生命周期、叶子 broker（额度/并发/
//! 串行通道）、输出缓冲与观察接口。
//!
//! 调度模型（规格 §1.4「本地调度适配」）：**观察函数单点驱动**——exec 的
//! yield 段与 wait 调用都在 turn 线程内进入 [`CellHandle::observe`]，所有
//! 事件保存（cell_started/call_started/call_finished/cell_closed）与 JS 兑现
//! （leaf 回包）都发生在这一线程；worker 读线程只把帧回送到有界事件通道
//! （不保存、不执行——「主轮驱动保存/通知，工作线程经有界事件队列回送」）。
//! 让出（无人观察）期间：worker 自行继续 JS 计算；已在途叶子照常完成但结果
//! 兑现等下一次观察；新叶子请求只留在有界队列——worker 写 stdout 阻塞形成
//! 天然背压（输出限额另有显式缓冲管理，§1.7）。
//!
//! 父进程是唯一能力与执行事实来源：叶子经注入的 [`LeafDispatcher`]（M2 从
//! turn.rs 抽取的共用分派）执行；宿主逐次核对冻结目录、额度（cell 31/轮
//! 128 原子扣除）、取消（cell 级标志传入 dispatch——终止时在途 exec_command
//! 沿 016 返回真实句柄），不信任子进程自报状态。

use super::protocol::{
    init_frame, leaf_frame, read_frame, shutdown_frame, write_frame,
    MAX_BRIDGE_RESULT_BYTES, MAX_FRAME_BYTES, MAX_INIT_FRAME_BYTES, MAX_LEAF_ARGS_BYTES,
    WORKER_AS_LIMIT, WORKER_CPU_LIMIT_SECS,
};
use crate::prompt::ToolDisplay;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// 每 cell 叶子调用上限（批次 32 份额度，exec 占 1 → 最多 31 叶子；§1.5）。
pub const CELL_LEAF_QUOTA: usize = 31;
/// 每逻辑用户轮叶子调用上限（跨 cell 共享；§1.5）。
pub const TURN_LEAF_QUOTA: usize = 128;
/// 最大在途叶子数（其余排队；§1.5）。
pub const MAX_INFLIGHT_LEAVES: usize = 4;
/// 待交付输出缓冲上限（§1.7：64 KiB；超出丢弃并标记 truncated）。
pub const PENDING_OUTPUT_LIMIT: usize = 64 * 1024;
/// cell 累计生成输出上限（含丢弃部分；超过关闭失败；§1.7）。
pub const CELL_TOTAL_OUTPUT_LIMIT: usize = 1024 * 1024;

/// 叶子调用执行状态（§1.3：不诱导自动重试未知副作用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafExecution {
    NotStarted,
    Completed,
    Unknown,
}

impl LeafExecution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Completed => "completed",
            Self::Unknown => "unknown",
        }
    }
}

/// 一次叶子调用的执行结果（宿主事实，供事件与 JS 兑现共用）。
#[derive(Debug, Clone)]
pub struct LeafOutcome {
    /// 模型面结果 JSON（与直接调用同源；业务失败保留 status/error/isError）。
    pub result_json: String,
    pub display: Option<ToolDisplay>,
    pub execution: LeafExecution,
}

/// 叶子分派器：M2 从 turn.rs 抽取的共用分派入口的 cell 侧形态（按 cell 出生
/// 时冻结的目录执行；`cancel`＝cell 级取消标志——终止时在途调用沿既有取消
/// 语义返回真实状态）。
pub trait LeafDispatcher: Send + Sync + 'static {
    fn dispatch(&self, tool: &str, args: &Value, cancel: &AtomicBool) -> LeafOutcome;
}

/// 编排事件草稿（M3 映射到 V3 `HistoryItem::OrchestrationEvent`；宿主保存
/// 是执行/兑现的前置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    CellStarted,
    CallStarted,
    CallFinished,
    CellClosed,
}

/// 待提交事件（引用形态；由 [`OrchestrationSink`] 落盘后才算发生）。
pub struct Event<'a> {
    pub kind: EventKind,
    pub cell_id: &'a str,
    pub exec_call_id: &'a str,
    /// cell 内宿主递增序号（V3 事件 seq 字段；装载校验连续性）。
    pub seq: u64,
    pub leaf_id: Option<u64>,
    pub tool_name: Option<&'a str>,
    /// cell_started＝exec 实际参数 JSON；call_started＝叶子实际参数 JSON。
    pub arguments: Option<&'a str>,
    /// call_finished 的真实结果。
    pub result: Option<&'a LeafOutcome>,
    /// cell_closed 的收尾原因。
    pub close_reason: Option<&'a str>,
}

/// V3 事件提交宿主：保存失败＝调用未发生（call_started 失败→叶子零执行；
/// call_finished 失败→cell 关闭且不兑现；§1.6）。`history`＝观察时点的完整
/// 内存历史（Session 侧以自身游标判定滞留待写段并随事件行按序落盘——
/// 032 检查点同模式，保证文件与内存顺序一致）。
pub trait OrchestrationSink {
    fn commit(
        &mut self,
        event: &Event<'_>,
        history: &[crate::prompt::HistoryItem],
    ) -> Result<(), String>;
}

/// 事件→V3 记录的转换（含大结果降级：>64 KiB 保留片段＋原始字节数＋
/// SHA-256＋truncated；§1.7）——sink 实现与 turn 侧内存入史共用同一转换。
pub fn event_to_history_item(event: &Event<'_>) -> crate::prompt::HistoryItem {
    use crate::prompt::{OrchestrationEventKind, OrchestrationEventResult};
    let degrade = |outcome: &LeafOutcome| -> OrchestrationEventResult {
        const SNIPPET: usize = 64 * 1024;
        let full = outcome.result_json.as_str();
        if full.len() <= SNIPPET {
            return OrchestrationEventResult {
                content: full.to_string(),
                original_bytes: None,
                sha256: None,
                truncated: false,
                display: outcome.display.clone(),
                execution: outcome.execution.as_str().to_string(),
            };
        }
        let mut cut = SNIPPET;
        while cut > 0 && !full.is_char_boundary(cut) {
            cut -= 1;
        }
        use sha2::Digest;
        let hash = sha2::Sha256::digest(full.as_bytes());
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        OrchestrationEventResult {
            content: full[..cut].to_string(),
            original_bytes: Some(full.len() as u64),
            sha256: Some(hex),
            truncated: true,
            display: outcome.display.clone(),
            execution: outcome.execution.as_str().to_string(),
        }
    };
    crate::prompt::HistoryItem::OrchestrationEvent {
        kind: match event.kind {
            EventKind::CellStarted => OrchestrationEventKind::CellStarted,
            EventKind::CallStarted => OrchestrationEventKind::CallStarted,
            EventKind::CallFinished => OrchestrationEventKind::CallFinished,
            EventKind::CellClosed => OrchestrationEventKind::CellClosed,
        },
        cell_id: event.cell_id.to_string(),
        exec_call_id: event.exec_call_id.to_string(),
        seq: event.seq,
        leaf_id: event.leaf_id,
        tool_name: event.tool_name.map(str::to_string),
        arguments: event.arguments.map(str::to_string),
        result: event.result.map(degrade),
        close_reason: event.close_reason.map(str::to_string),
    }
}

/// 已提交事件的内存入史队列：sink 落盘成功后由 runtime 暂存，观察返回时
/// 调用方（turn）取出推入内存历史并 on_record（07 自动追加游标已越过，
/// 不会二次落盘）。
pub fn take_committed_events(handle: &mut CellHandle) -> Vec<crate::prompt::HistoryItem> {
    std::mem::take(&mut handle.committed_events)
}

/// 编排宿主装配（turn 侧传入 TurnExtras.orchestration）。`session`＝共享会话
/// 保存器（V3 事件经 turn 侧 [`OrchestrationSink`] 实现落盘并在回执后立即
/// 推入内存历史——032 检查点「回执成功→同步入史」同模式，保证 Session 游标
/// 与内存顺序一致）。
pub struct OrchestrationExtras<'a> {
    /// V3 事件提交宿主（观察线程内调用；保存成功才算发生）。
    pub sink: &'a mut dyn OrchestrationSink,
    /// 本轮 MCP 快照（嵌套目录的动态/资源工具面；None＝无 MCP）。
    pub mcp: Option<std::sync::Arc<crate::mcp::manager::McpSnapshot>>,
    /// 每逻辑用户轮 128 叶子额度（跨同轮全部 cell 共享；§1.5/C09）。
    pub turn_budget: std::sync::Arc<AtomicUsize>,
}

/// 工具目录条目（cell 出生时冻结的深拷贝；`serial`＝串行通道成员）。
#[derive(Debug, Clone)]
pub struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    /// 参数 Schema 原样保留（不裁剪）。
    pub parameters: Value,
    /// apply_patch/update_plan/write_stdin 共用同一串行通道；其余可并发。
    pub serial: bool,
}

/// 观察结果（exec 与 wait 的统一返回；§1.3 固定字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub ok: bool,
    pub status: CellStatus,
    pub cell_id: Option<String>,
    /// 本次观察的增量输出（按 text 调用顺序）。
    pub output: Vec<String>,
    pub output_truncated: bool,
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStatus {
    Running,
    Completed,
    Terminated,
    Failed,
}

impl CellStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Terminated => "terminated",
            Self::Failed => "failed",
        }
    }
}

/// 事件通道消息：worker 帧回送（读线程）与叶子完成回执（执行线程）共用
/// 同一通道——观察循环的 recv 天然按到达序处理两者（否则 recv 阻塞期间叶子
/// 完成会无人收割）。
enum Msg {
    Worker(Value),
    LeafDone { call_id: u64, outcome: LeafOutcome },
    ReaderDead(String),
}

struct InflightLeaf {
    /// worker 侧调用 id（回包寻址）。
    call_id: u64,
    /// 宿主受理序号（事件关联）。
    leaf_id: u64,
    tool: String,
    serial: bool,
    handle: std::thread::JoinHandle<()>,
}

struct QueuedCall {
    call_id: u64,
    leaf_id: u64,
    tool: String,
    args: Value,
    serial: bool,
}

/// 兑现寻址用的最小叶子标识（收割后 InflightLeaf 已消耗 join 所有权）。
struct InflightLeafStub {
    call_id: u64,
}

/// cell 终态记录（done 帧/坏帧/EOF）。
struct Terminal {
    ok: bool,
    error: Option<String>,
    unawaited: usize,
}

/// 输出缓冲：待交付段（64 KiB，溢出丢弃＋truncated）＋累计计数（1 MiB 关闭）。
struct OutputBuffer {
    pending: Vec<String>,
    pending_bytes: usize,
    pending_truncated: bool,
    total_bytes: usize,
    total_over_limit: bool,
}

impl OutputBuffer {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            pending_bytes: 0,
            pending_truncated: false,
            total_bytes: 0,
            total_over_limit: false,
        }
    }

    fn push(&mut self, text: String) {
        self.total_bytes = self.total_bytes.saturating_add(text.len());
        if self.total_bytes > CELL_TOTAL_OUTPUT_LIMIT {
            self.total_over_limit = true; // 超累计上限：丢弃（observe 关闭失败）
            return;
        }
        if self.pending_bytes + text.len() > PENDING_OUTPUT_LIMIT {
            self.pending_truncated = true; // 待交付段溢出：丢弃并标记
            return;
        }
        self.pending_bytes += text.len();
        self.pending.push(text);
    }

    /// 延续块与末条合并（同一次 text 的分片重组；末条可能已被预算截断——
    /// 合并到其剩余部分，调用边界语义保持）。
    fn merge_last(&mut self, text: String) {
        self.total_bytes = self.total_bytes.saturating_add(text.len());
        if self.total_bytes > CELL_TOTAL_OUTPUT_LIMIT {
            self.total_over_limit = true;
            return;
        }
        match self.pending.last_mut() {
            Some(last) => {
                last.push_str(&text);
                self.pending_bytes += text.len();
            }
            None => self.push(text), // 无前块（已交付/丢弃）：独立成条
        }
    }

    /// 取走增量（按观察预算截断，UTF-8 边界）。
    fn drain(&mut self, budget: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut used = 0usize;
        while let Some(text) = self.pending.first() {
            if used + text.len() > budget {
                if out.is_empty() && budget > 0 {
                    // 首条即超预算：按 UTF-8 边界截断交付首段
                    let mut cut = budget.min(text.len());
                    while cut > 0 && !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    let head = text[..cut].to_string();
                    let rest = text[cut..].to_string();
                    self.pending_bytes -= head.len();
                    used += head.len();
                    out.push(head);
                    self.pending[0] = rest;
                }
                break;
            }
            used += text.len();
            self.pending_bytes -= text.len();
            out.push(self.pending.remove(0));
        }
        out
    }
}

/// cell 关闭原因（cell_closed 事件携带）。
#[derive(Debug, Clone, Copy)]
enum CellCloseReason {
    Terminated,
    PersistFailed,
    OutputLimit,
    TerminalConsumed,
    TurnExit,
    IpcDead,
}

impl CellCloseReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Terminated => "terminated",
            Self::PersistFailed => "persist_failed",
            Self::OutputLimit => "output_limit",
            Self::TerminalConsumed => "terminal_consumed",
            Self::TurnExit => "turn_exit",
            Self::IpcDead => "ipc_dead",
        }
    }
}

/// 一个活动 cell：worker 子进程＋broker 状态。观察/终止都在持有方（turn 线程）。
/// Drop 兜底：遗漏出口导致句柄被丢弃时至少组杀回收 worker（不写事件——
/// 事件链以孤儿 start 形态呈现「结果未知」，§1.6；审查 P2-12）。
impl Drop for CellHandle {
    fn drop(&mut self) {
        if !self.closed {
            self.cancel_flag.store(true, Ordering::SeqCst);
            let _ = kill_worker(&mut self.child);
        }
    }
}

pub struct CellHandle {
    pub id: String,
    exec_call_id: String,
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    reader_rx: mpsc::Receiver<Msg>,
    /// 事件通道发送端（叶子执行线程回送 LeafDone）。
    event_tx: mpsc::Sender<Msg>,
    /// cell 出生时冻结的工具目录（资格＋串行标记；深拷贝）。
    catalog: Vec<ToolCatalogEntry>,
    inflight: Vec<InflightLeaf>,
    queue: VecDeque<QueuedCall>,
    buffer: OutputBuffer,
    terminal: Option<Terminal>,
    /// cell 内叶子受理计数（含已完成；≤31）。
    leaf_count: usize,
    /// 每逻辑用户轮共享的叶子预算（128；跨 cell，由宿主注入）。
    turn_budget: Arc<AtomicUsize>,
    leaf_seq: u64,
    serial_busy: bool,
    dispatcher: Arc<dyn LeafDispatcher>,
    /// cell 级取消标志（终止时置位；dispatch 与观察循环读取）。
    cancel_flag: Arc<AtomicBool>,
    closed: bool,
    total_leaves_committed: usize,
    /// cell 内事件递增序号（cell_started 已用 1）。
    event_seq: u64,
    /// 已落盘事件的内存入史队列（观察返回时由 turn 取走）。
    committed_events: Vec<crate::prompt::HistoryItem>,
}

/// 进程内 cell 序号；前缀含进程启动纳秒——跨进程（/resume、重启续写）不与
/// 已装载历史中的旧 cell_id 碰撞（审查 P1-4：碰撞会使 V3 事件链校验整档
/// 拒绝）。句柄权限判定靠宿主状态而非不可猜性。
static CELL_SEQ: AtomicUsize = AtomicUsize::new(1);

fn next_cell_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("cell-{:x}-{}", nanos, CELL_SEQ.fetch_add(1, Ordering::SeqCst))
}

/// cell 创建配置（turn 侧装配）。`worker_exe`＝测试注入的主二进制路径
/// （生产 None＝current_exe；不经环境变量——env 覆盖会成为任意代码执行面）。
pub struct CellConfig<'a> {
    pub code: &'a str,
    /// exec 完整实际参数 JSON（cell_started 事件保存）。
    pub arguments_json: &'a str,
    pub catalog: Vec<ToolCatalogEntry>,
    pub exec_call_id: &'a str,
    pub turn_budget: Arc<AtomicUsize>,
    pub dispatcher: Arc<dyn LeafDispatcher>,
    pub worker_exe: Option<std::path::PathBuf>,
}

/// 启动一个 cell：spawn worker（033 包装/env_clear/进程组/RLIMIT）→ init 帧
/// → cell_started 事件（保存成功才算创建成功；失败回收 worker，§1.2）。
pub fn spawn_cell(
    config: &CellConfig<'_>,
    sink: &mut dyn OrchestrationSink,
    history: &[crate::prompt::HistoryItem],
) -> Result<CellHandle, String> {
    let id = next_cell_id();
    // init 帧容量预检（目录序列化超限＝启动前拒绝，不 spawn 进程）。
    let tools: Vec<Value> = config.catalog.iter().map(|t| tool_wire(t)).collect();
    let init = init_frame(config.code, &tools);
    {
        let mut probe = Vec::new();
        write_frame(&mut probe, &init, MAX_INIT_FRAME_BYTES)
            .map_err(|e| format!("编排初始化帧超限，已拒绝启动：{e}"))?;
    }
    let worker_exe = config
        .worker_exe
        .clone()
        .unwrap_or_else(|| std::env::current_exe().expect("定位自身可执行文件失败"));
    let mut child = spawn_worker(&worker_exe)?;
    let mut stdin = child
        .stdin
        .take()
        .expect("spawn 已 piped stdin");
    if let Err(e) = write_frame(&mut stdin, &init, MAX_INIT_FRAME_BYTES) {
        let _ = kill_worker(&mut child);
        return Err(format!("发送初始化帧失败：{e}"));
    }
    let stdout = child
        .stdout
        .take()
        .expect("spawn 已 piped stdout");
    let (tx, rx) = mpsc::channel::<Msg>();
    let event_tx = tx.clone();
    std::thread::Builder::new()
        .name("tchat-js-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_frame(&mut reader, MAX_FRAME_BYTES) {
                    Ok(Some(frame)) => {
                        if tx.send(Msg::Worker(frame)).is_err() {
                            return; // 观察方已放弃
                        }
                    }
                    Ok(None) => {
                        let _ = tx.send(Msg::ReaderDead("worker 关闭了 stdout".into()));
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::ReaderDead(format!("坏帧：{e}")));
                        return;
                    }
                }
            }
        })
        .map_err(|e| {
            let _ = kill_worker(&mut child);
            format!("启动 worker 读线程失败：{e}")
        })?;
    // cell_started 保存成功才算创建成功（§1.2）。
    let event = Event {
        kind: EventKind::CellStarted,
        cell_id: &id,
        exec_call_id: config.exec_call_id,
        seq: 1,
        leaf_id: None,
        tool_name: None,
        arguments: Some(config.arguments_json),
        result: None,
        close_reason: None,
    };
    match sink.commit(&event, history) {
        Ok(()) => {}
        Err(reason) => {
            let _ = kill_worker(&mut child);
            return Err(format!(
                "cell_started 保存失败：{reason}（cell 未创建，worker 已回收）"
            ));
        }
    }
    let committed = vec![event_to_history_item(&event)];
    Ok(CellHandle {
        id,
        exec_call_id: config.exec_call_id.to_string(),
        child,
        stdin: Some(stdin),
        reader_rx: rx,
        event_tx,
        catalog: config.catalog.clone(),
        inflight: Vec::new(),
        queue: VecDeque::new(),
        buffer: OutputBuffer::new(),
        terminal: None,
        leaf_count: 0,
        turn_budget: config.turn_budget.clone(),
        leaf_seq: 0,
        serial_busy: false,
        dispatcher: config.dispatcher.clone(),
        cancel_flag: Arc::new(AtomicBool::new(false)),
        closed: false,
        total_leaves_committed: 0,
        event_seq: 2,
        committed_events: committed,
    })
}

fn tool_wire(entry: &ToolCatalogEntry) -> Value {
    json!({
        "name": entry.name,
        "description": entry.description,
        "parameters": entry.parameters,
    })
}

/// spawn worker：本二进制 `--internal-js`；启用 033 时经 `tool_launch_argv`
/// 包装（fail-closed，不回退）；始终 env_clear＋独立进程组＋RLIMIT（AS/CPU）。
fn spawn_worker(exe: &std::path::Path) -> Result<std::process::Child, String> {
    let argv: Vec<String> = vec![
        exe.to_string_lossy().into_owned(),
        "--internal-js".to_string(),
    ];
    let (program, args, env) = match crate::sandbox::tool_launch_argv(argv.clone()) {
        Ok(Some((program, args, env))) => (program, args, env),
        Ok(None) => (
            argv[0].clone(),
            argv[1..].to_vec(),
            Vec::new(), // 普通模式：env_clear 后零环境（worker 不读 env）
        ),
        Err(e) => {
            return Err(format!(
                "沙箱包装失败，编排 worker 拒绝启动（不回退无隔离模式）：{e}"
            ))
        }
    };
    let as_limit = WORKER_AS_LIMIT;
    let cpu_limit = WORKER_CPU_LIMIT_SECS;
    let mut command = std::process::Command::new(&program);
    command
        .args(&args)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0); // 独立进程组（killpg 回收面）
        // RLIMIT：地址空间 512 MiB、累计 CPU 5 秒（SIGXCPU）；墙钟由 worker
        // 内部中断与宿主观察兜底。
        unsafe {
            command.pre_exec(move || {
                // fd 收口（§1.6「关闭无关 fd」）：worker 只应有 0/1/2 三个标准
                // 描述符——宿主进程内任何未带 CLOEXEC 的继承 fd（哨兵实测可
                // 泄漏）在 exec 前统一关闭。close_range(3, ~0)＝一条原子系统
                // 调用（fork 后 exec 前的 async-signal-safe 上下文里不能走
                // opendir/malloc 兜底）；内核 <5.9（ENOSYS）按既有 CLOEXEC
                // 纪律继续，不因卫生失败阻断启动。系统调用号 436 在
                // x86_64/aarch64/riscv64 通用表一致；其他架构跳过（依赖
                // CLOEXEC 纪律）。libc 0.2.189 未导出 SYS_close_range 常量。
                // 038-CI：close_range(436) 为 Linux 专属 syscall——macOS 等其他
                // unix 上 436 号语义不同，必须一并跳过（同 CLOEXEC 纪律理由）。
                #[cfg(all(
                    target_os = "linux",
                    any(
                        target_arch = "x86_64",
                        target_arch = "aarch64",
                        target_arch = "riscv64"
                    )
                ))]
                {
                    // best-effort：失败（ENOSYS/EINVAL）不阻断 worker 启动
                    let _ = libc::syscall(436 as libc::c_long, 3u64, u64::MAX, 0u64);
                }
                let as_lim = libc::rlimit {
                    rlim_cur: as_limit,
                    rlim_max: as_limit,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &as_lim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let cpu_lim = libc::rlimit {
                    rlim_cur: cpu_limit,
                    rlim_max: cpu_limit,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &cpu_lim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command
        .spawn()
        .map_err(|e| format!("启动编排 worker 失败：{e}"))
}

/// 组杀回收 worker 进程组（setsid 后 pgid＝pid）。
fn kill_worker(child: &mut std::process::Child) -> Result<(), String> {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        let _ = unsafe { libc::killpg(pid, libc::SIGKILL) };
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    child.wait().map(|_| ()).map_err(|e| format!("回收 worker 失败：{e}"))
}

/// 叶子拒绝错误帧：{code, message, execution}（§1.3）。
fn leaf_error(code: &str, message: impl Into<String>, execution: LeafExecution) -> Value {
    json!({ "code": code, "message": message.into(), "execution": execution.as_str() })
}

impl CellHandle {
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel_flag
    }

    /// 观察至 `yield_ms` 到期或终态收尾完成：处理回送帧、调度队列、收割在途
    /// 叶子（提交 call_finished 成功才向 JS 兑现）。`budget`＝本次观察的输出
    /// 字节预算（4×max_tokens）。终态＋输出取尽后自动关闭并释放句柄。
    pub fn observe(
        &mut self,
        yield_ms: u64,
        budget: usize,
        sink: &mut dyn OrchestrationSink,
        turn_cancel: Option<&AtomicBool>,
        history: &[crate::prompt::HistoryItem],
    ) -> Observation {
        if self.closed {
            return self.terminal_observation(budget, Some(leaf_error(
                "CELL_CLOSED",
                "cell 已关闭，句柄已释放",
                LeafExecution::NotStarted,
            )));
        }
        let deadline = Instant::now() + Duration::from_millis(yield_ms);
        loop {
            self.pump_queue(sink, history);
            if self.terminal.is_some() && self.inflight.is_empty() && self.queue.is_empty() {
                break; // 终态且收尾完成
            }
            let now = Instant::now();
            if now >= deadline || turn_cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                break;
            }
            match self
                .reader_rx
                .recv_timeout(deadline.saturating_duration_since(now))
            {
                Ok(Msg::Worker(frame)) => {
                    if !self.handle_worker_frame(frame, sink, history) {
                        break;
                    }
                }
                Ok(Msg::LeafDone { call_id, outcome }) => {
                    self.harvest_leaf(call_id, outcome, sink, history);
                }
                Ok(Msg::ReaderDead(reason)) => {
                    if self.terminal.is_none() {
                        self.terminal = Some(Terminal {
                            ok: false,
                            error: Some(format!("worker IPC 断开：{reason}")),
                            unawaited: self.inflight.len(),
                        });
                        // 在途叶子仍收割（下一次循环/收尾）
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if self.terminal.is_none() {
                        self.terminal = Some(Terminal {
                            ok: false,
                            error: Some("worker 读线程退出".into()),
                            unawaited: self.inflight.len(),
                        });
                    }
                }
            }
        }
        // 输出累计超限：关闭失败（不能靠频繁 wait 绕过，§1.7）。
        if self.buffer.total_over_limit {
            let observation = self.observation_for(budget, CellStatus::Failed, Some(leaf_error(
                "OUTPUT_LIMIT",
                "cell 累计输出超过 1 MiB 上限，已关闭",
                LeafExecution::Unknown,
            )));
            self.close(CellCloseReason::OutputLimit, sink, history);
            return observation;
        }
        let status = self.current_status();
        let error = self.terminal_error();
        let observation = self.observation_for(budget, status, error);
        // 终态已交付且输出取尽：关闭释放句柄（§1.4「终态被交付后释放句柄」）。
        if status != CellStatus::Running && self.buffer.pending.is_empty() {
            self.close(CellCloseReason::TerminalConsumed, sink, history);
        }
        observation
    }

    /// 终止（wait terminate=true / 轮出口收尾）：原子关闭受理、取消排队与
    /// 在途等待、终止并回收 worker；收取真实已发生结果后返回 terminated
    /// （完成先于终止且已提交则返回 completed，§1.4）。
    pub fn terminate(
        &mut self,
        sink: &mut dyn OrchestrationSink,
        history: &[crate::prompt::HistoryItem],
    ) -> Observation {
        if self.closed {
            return self.terminal_observation(0, None);
        }
        // 完成先于终止且已提交（无未结算）：返回 completed。
        let completed = self
            .terminal
            .as_ref()
            .is_some_and(|t| t.ok && self.inflight.is_empty() && self.queue.is_empty());
        if completed {
            let observation = self.observation_for(0, CellStatus::Completed, None);
            self.close(CellCloseReason::TerminalConsumed, sink, history);
            return observation;
        }
        // 取消在途等待：置 cell 取消标志（dispatch 沿既有取消语义返回真实状态）。
        self.cancel_flag.store(true, Ordering::SeqCst);
        let observation = self.observation_for(0, CellStatus::Terminated, None);
        self.close(CellCloseReason::Terminated, sink, history);
        observation
    }

    /// 轮出口统一收尾（宿主退出/取消/模型终文/第 8 次请求；M4 接入）。
    pub fn close_for_turn_exit(
        &mut self,
        sink: &mut dyn OrchestrationSink,
        history: &[crate::prompt::HistoryItem],
    ) {
        if !self.closed {
            self.cancel_flag.store(true, Ordering::SeqCst);
            self.close(CellCloseReason::TurnExit, sink, history);
        }
    }

    /// 从队列启动可运行的叶子（在途 <4；串行通道空闲才启动串行工具）。
    fn pump_queue(
        &mut self,
        sink: &mut dyn OrchestrationSink,
        history: &[crate::prompt::HistoryItem],
    ) {
        loop {
            if self.inflight.len() >= MAX_INFLIGHT_LEAVES || self.terminal.is_some() {
                break;
            }
            let index = self
                .queue
                .iter()
                .position(|call| !call.serial || !self.serial_busy);
            let Some(index) = index else { break };
            let call = self.queue.remove(index).expect("已定位");
            if call.serial {
                self.serial_busy = true;
            }
            self.start_leaf(call);
        }
    }

    /// 处理一帧 worker 消息；返回 false＝观察应立即结束（cell 已关闭）。
    fn handle_worker_frame(
        &mut self,
        frame: Value,
        sink: &mut dyn OrchestrationSink,
        history: &[crate::prompt::HistoryItem],
    ) -> bool {
        match frame.get("t").and_then(Value::as_str) {
            Some("call") => {
                self.accept_call(frame, sink, history);
                !self.closed
            }
            Some("output") => {
                let text = frame
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let continuation = frame.get("cont").and_then(Value::as_bool).unwrap_or(false);
                if continuation {
                    // 同一次 text 的延续块：与缓冲中同 json 标记的末条合并
                    //（§1.3「原样追加一条文本」；审查 P2-11）。
                    self.buffer.merge_last(text);
                } else {
                    self.buffer.push(text);
                }
                true // 超限由 observe 收尾判定
            }
            Some("done") => {
                let ok = frame.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let error = frame
                    .get("error")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let unawaited =
                    frame.get("unawaited").and_then(Value::as_u64).unwrap_or(0) as usize;
                self.terminal = Some(Terminal { ok, error, unawaited });
                true
            }
            _ => {
                // 未知帧＝坏帧：IPC 断开语义（§1.5）。
                self.terminal = Some(Terminal {
                    ok: false,
                    error: Some("worker 发来未知帧，按坏帧关闭".into()),
                    unawaited: self.inflight.len(),
                });
                true
            }
        }
    }

    /// 受理一次叶子调用：参数形态/大小/资格/额度原子扣除 → call_started 保存
    /// → 立即执行（在途 <4 且串行位可用）或入有界队列（§1.5）。
    fn accept_call(
        &mut self,
        frame: Value,
        sink: &mut dyn OrchestrationSink,
        history: &[crate::prompt::HistoryItem],
    ) {
        let call_id = frame.get("id").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let tool = frame
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let args = frame.get("args").cloned().unwrap_or(Value::Null);
        let reject = |code: &str, message: String, execution: LeafExecution| {
            leaf_error(code, message, execution)
        };
        let args_json = serde_json::to_string(&args).unwrap_or_default();
        // §1.5：格式可识别（帧已解析出 tool/args）即先原子扣减轮额度，再检查
        // 资格与参数——错误参数、资格拒绝和业务失败同样消耗名额（审查 P2-5）。
        if self
            .turn_budget
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_err()
        {
            self.reply_leaf(
                call_id,
                Err(reject(
                    "TURN_QUOTA",
                    "本用户轮叶子调用已达 128 次上限".into(),
                    LeafExecution::NotStarted,
                )),
            );
            return;
        }
        // 参数形态与大小检查（拒绝路径额度不回补，§1.5）。
        if !args.is_object() {
            self.reply_leaf(call_id, Err(reject(
                "INVALID_ARGS",
                "叶子参数必须是 JSON 对象".into(),
                LeafExecution::NotStarted,
            )));
            return;
        }
        if args_json.len() > MAX_LEAF_ARGS_BYTES {
            self.reply_leaf(
                call_id,
                Err(reject(
                    "ARGS_TOO_LARGE",
                    format!("叶子参数超过 {} 字节上限", MAX_LEAF_ARGS_BYTES),
                    LeafExecution::NotStarted,
                )),
            );
            return;
        }
        // 资格：冻结目录精确名（不拆 `__`、不猜只读、不信任自报，§1.2/§1.6）。
        let Some(entry) = self.catalog.iter().find(|e| e.name == tool) else {
            self.reply_leaf(
                call_id,
                Err(reject(
                    "TOOL_NOT_AVAILABLE",
                    format!("工具 {tool} 不在本 cell 的冻结目录内"),
                    LeafExecution::NotStarted,
                )),
            );
            return;
        };
        let serial = entry.serial;
        // 额度原子扣除（§1.5：格式可识别先扣额再查资格；此处资格已过再扣，
        // 两者都在零业务副作用前完成）。
        if self.leaf_count >= CELL_LEAF_QUOTA {
            self.reply_leaf(
                call_id,
                Err(reject(
                    "CELL_QUOTA",
                    "本 cell 叶子调用已达 31 次上限".into(),
                    LeafExecution::NotStarted,
                )),
            );
            return;
        }
        self.leaf_count += 1;
        self.leaf_seq += 1;
        let leaf_id = self.leaf_seq;
        // call_started 保存成功才执行（§1.6：未存不执行）。
        let seq = self.event_seq;
        let event = Event {
            kind: EventKind::CallStarted,
            cell_id: &self.id,
            exec_call_id: &self.exec_call_id,
            seq,
            leaf_id: Some(leaf_id),
            tool_name: Some(&tool),
            arguments: Some(&args_json),
            result: None,
            close_reason: None,
        };
        match sink.commit(&event, history) {
            Ok(()) => {
                self.event_seq += 1; // 成功才占用序号（失败由后续事件复用，防 seq 空洞——审查 P1-3）
                self.committed_events.push(event_to_history_item(&event));
            }
            Err(reason) => {
                self.reply_leaf(
                    call_id,
                    Err(reject(
                        "PERSIST_FAILED",
                        format!("call_started 保存失败，叶子零执行：{reason}"),
                        LeafExecution::NotStarted,
                    )),
                );
                self.close(CellCloseReason::PersistFailed, sink, history);
                return;
            }
        }
        let call = QueuedCall {
            call_id,
            leaf_id,
            tool,
            args,
            serial,
        };
        let can_start =
            self.inflight.len() < MAX_INFLIGHT_LEAVES && !(serial && self.serial_busy);
        if can_start {
            if serial {
                self.serial_busy = true;
            }
            self.start_leaf(call);
        } else {
            self.queue.push_back(call);
        }
    }

    /// 启动一个叶子执行线程（call_started 已保存；执行经注入的分派器）。
    /// 完成回执走事件通道（LeafDone）——观察循环的 recv 与 worker 帧按到达
    /// 序统一处理。
    fn start_leaf(&mut self, call: QueuedCall) {
        let dispatcher = self.dispatcher.clone();
        let cancel = self.cancel_flag.clone();
        let tool = call.tool.clone();
        let args = call.args.clone();
        let call_id = call.call_id;
        let event_tx = self.event_tx.clone();
        let handle = std::thread::Builder::new()
            .name("tchat-js-leaf".into())
            .spawn(move || {
                let outcome = dispatcher.dispatch(&tool, &args, &cancel);
                let _ = event_tx.send(Msg::LeafDone { call_id, outcome });
            })
            .expect("叶子执行线程启动失败");
        self.inflight.push(InflightLeaf {
            call_id: call.call_id,
            leaf_id: call.leaf_id,
            tool: call.tool,
            serial: call.serial,
            handle,
        });
    }

    /// 收割一个已完成的叶子（LeafDone）：提交 call_finished（成功才兑现；
    /// 失败关闭 cell）。取消语义在 dispatch 内（016：取消返回 cancelled＋
    /// 真实句柄，不杀进程）。
    fn harvest_leaf(
        &mut self,
        call_id: u64,
        outcome: LeafOutcome,
        sink: &mut dyn OrchestrationSink,
        history: &[crate::prompt::HistoryItem],
    ) {
        let Some(index) = self.inflight.iter().position(|l| l.call_id == call_id) else {
            return; // 迟到回执（cell 已关闭丢弃过）：防御性忽略
        };
        let mut leaf = self.inflight.remove(index);
        if leaf.serial {
            self.serial_busy = false;
        }
        let _ = leaf.handle.join();
        let (leaf_id, tool) = (leaf.leaf_id, leaf.tool.clone());
        self.total_leaves_committed += 1;
        let seq = self.event_seq;
        let event = Event {
            kind: EventKind::CallFinished,
            cell_id: &self.id,
            exec_call_id: &self.exec_call_id,
            seq,
            leaf_id: Some(leaf_id),
            tool_name: Some(&tool),
            arguments: None,
            result: Some(&outcome),
            close_reason: None,
        };
        match sink.commit(&event, history) {
            Ok(()) => {
                self.event_seq += 1; // 成功才占用序号（审查 P1-3）
                self.committed_events
                    .push(event_to_history_item(&event));
                self.deliver_leaf_result(&InflightLeafStub { call_id }, outcome)
            }
            Err(reason) => {
                self.reply_leaf(
                    call_id,
                    Err(leaf_error(
                        "PERSIST_FAILED",
                        format!("call_finished 保存失败，不兑现：{reason}"),
                        outcome.execution,
                    )),
                );
                self.close(CellCloseReason::PersistFailed, sink, history);
            }
        }
    }

    /// 向 JS 兑现一次已保存的叶子结果（大小桥接检查；超限拒绝桥接但事实已记录）。
    fn deliver_leaf_result(&mut self, leaf: &InflightLeafStub, outcome: LeafOutcome) {
        if outcome.result_json.len() > MAX_BRIDGE_RESULT_BYTES {
            self.reply_leaf(
                leaf.call_id,
                Err(leaf_error(
                    "RESULT_TOO_LARGE",
                    format!(
                        "结果 JSON {} 字节超过 {} 桥接上限，拒绝交付（事实已记录）",
                        outcome.result_json.len(),
                        MAX_BRIDGE_RESULT_BYTES
                    ),
                    outcome.execution,
                )),
            );
            return;
        }
        let value: Value = serde_json::from_str(&outcome.result_json).unwrap_or(Value::Null);
        self.reply_leaf(leaf.call_id, Ok(value));
    }

    fn reply_leaf(&mut self, id: u64, outcome: Result<Value, Value>) {
        let frame = leaf_frame(id, outcome);
        if let Some(stdin) = self.stdin.as_mut() {
            if write_frame(stdin, &frame, MAX_FRAME_BYTES).is_err() {
                self.stdin = None; // worker 已断：读线程 ReaderDead 收尾
            }
        }
    }

    /// 当前状态：无终态＝Running；终态 ok 且全部结算且无未 await＝Completed；
    /// 其余＝Failed（含 UNAWAITED_TOOLS——顶层兑现但仍有未结算调用时不伪报
    /// 成功，§1.3）。
    fn current_status(&self) -> CellStatus {
        if self.closed {
            return CellStatus::Failed; // 终止/持久化失败/超限/IPC 断开后的关闭
        }
        match &self.terminal {
            None => CellStatus::Running,
            Some(t) => {
                // 「顶层兑现且已提交的叶子调用全部结算才可成功完成」（§1.3）：
                // 仍有未结算（在途/排队/unawaited）时不伪报成功。
                if t.ok
                    && t.unawaited == 0
                    && self.inflight.is_empty()
                    && self.queue.is_empty()
                {
                    CellStatus::Completed
                } else {
                    CellStatus::Failed
                }
            }
        }
    }

    /// 终态错误（含 UNAWAITED_TOOLS 标记）。
    fn terminal_error(&self) -> Option<Value> {
        let t = self.terminal.as_ref()?;
        if t.ok && t.unawaited == 0 {
            return None;
        }
        let pending_now = self.inflight.len() + self.queue.len();
        let unawaited = t.unawaited.max(pending_now);
        if t.ok && unawaited > 0 {
            return Some(leaf_error(
                "UNAWAITED_TOOLS",
                format!(
                    "脚本顶层已结束但仍有 {unawaited} 个已受理叶子调用未结算（不自动替模型等待）"
                ),
                LeafExecution::Unknown,
            ));
        }
        Some(leaf_error(
            "SCRIPT_FAILED",
            t.error.clone().unwrap_or_else(|| "脚本失败".into()),
            LeafExecution::Unknown,
        ))
    }

    fn observation_for(
        &mut self,
        budget: usize,
        status: CellStatus,
        error: Option<Value>,
    ) -> Observation {
        let output = self.buffer.drain(budget);
        // 「本次观察是否丢弃过」语义：交付即复位（后续溢出再置位）——
        // 不做粘性标志（审查 P2-8）。
        let truncated = self.buffer.pending_truncated;
        self.buffer.pending_truncated = false;
        Observation {
            ok: matches!(status, CellStatus::Running | CellStatus::Completed),
            status,
            cell_id: Some(self.id.clone()),
            output,
            output_truncated: truncated,
            error,
        }
    }

    /// 已关闭句柄上的观察（固定 failed＋释放提示；§1.4「再等待失败」）。
    fn terminal_observation(&mut self, budget: usize, error: Option<Value>) -> Observation {
        let output = self.buffer.drain(budget);
        Observation {
            ok: false,
            status: CellStatus::Failed,
            cell_id: Some(self.id.clone()),
            output,
            output_truncated: false,
            error,
        }
    }

    /// cell 关闭：置取消（在途沿真实取消语义收取）→ 排队项零执行回包 →
    /// 有界收割在途（尽力保存真实结果；未收取的留在事件流为 start 孤儿，
    /// 恢复显示结果未知，§1.6）→ shutdown 帧＋worker 回收 → cell_closed。
    fn close(
        &mut self,
        reason: CellCloseReason,
        sink: &mut dyn OrchestrationSink,
        history: &[crate::prompt::HistoryItem],
    ) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.cancel_flag.store(true, Ordering::SeqCst);
        // 排队项零执行（§1.5：关闭后零新增；已扣额度如实记录为未执行）。
        let queued: Vec<QueuedCall> = self.queue.drain(..).collect();
        for call in queued {
            self.reply_leaf(
                call.call_id,
                Err(leaf_error(
                    "CELL_CLOSED",
                    "cell 已关闭，排队调用零执行",
                    LeafExecution::NotStarted,
                )),
            );
        }
        // 在途收割：取消置位后 dispatch 尽快返回（沿既有取消语义收取真实
        // 结果）；有界等待后仍未返回的留给孤儿 start（结果未知，§1.6）。
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.inflight.is_empty() && Instant::now() < deadline {
            match self
                .reader_rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Msg::LeafDone { call_id, outcome }) => {
                    if let Some(index) =
                        self.inflight.iter().position(|l| l.call_id == call_id)
                    {
                        let mut leaf = self.inflight.remove(index);
                        if leaf.serial {
                            self.serial_busy = false;
                        }
                        let _ = leaf.handle.join();
                        let seq = self.event_seq;
                        let event = Event {
                            kind: EventKind::CallFinished,
                            cell_id: &self.id,
                            exec_call_id: &self.exec_call_id,
                            seq,
                            leaf_id: Some(leaf.leaf_id),
                            tool_name: Some(&leaf.tool),
                            arguments: None,
                            result: Some(&outcome),
                            close_reason: None,
                        };
                        match sink.commit(&event, history) {
                            Ok(()) => {
                                self.event_seq += 1;
                                self.committed_events.push(event_to_history_item(&event));
                            }
                            Err(_) => {} // 关闭路径尽力保存（不兑现；失败不占号）
                        }
                    }
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // worker 终止与回收（shutdown 帧尽力发送；killpg 兜底）。
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = write_frame(stdin, &shutdown_frame(reason.as_str()), MAX_FRAME_BYTES);
        }
        self.stdin = None;
        let _ = kill_worker(&mut self.child);
        let seq = self.event_seq;
        let event = Event {
            kind: EventKind::CellClosed,
            cell_id: &self.id,
            exec_call_id: &self.exec_call_id,
            seq,
            leaf_id: None,
            tool_name: None,
            arguments: None,
            result: None,
            close_reason: Some(reason.as_str()),
        };
        if sink.commit(&event, history).is_ok() {
            self.event_seq += 1;
            self.committed_events.push(event_to_history_item(&event));
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// worker 进程 pid（env/fd 哨兵等诊断断言用）。
    pub fn worker_pid(&self) -> u32 {
        self.child.id()
    }

    /// 已交付的叶子调用计数（验收/诊断）。
    pub fn committed_leaves(&self) -> usize {
        self.total_leaves_committed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 输出缓冲：待交付段溢出丢弃＋标记；累计超限标记；预算截断按 UTF-8 边界。
    #[test]
    fn output_buffer_limits() {
        let mut buf = OutputBuffer::new();
        buf.push("a".repeat(1024));
        buf.push("b".repeat(1024));
        let out = buf.drain(1024);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 1024);
        // 溢出：待交付段超 64 KiB 丢弃并标记
        let mut small = OutputBuffer::new();
        small.push("x".repeat(PENDING_OUTPUT_LIMIT + 1));
        assert!(small.pending_truncated);
        assert!(small.pending.is_empty());
        // 累计 1 MiB 超限
        let mut total = OutputBuffer::new();
        for _ in 0..16 {
            total.push("y".repeat(PENDING_OUTPUT_LIMIT));
            assert!(!total.total_over_limit);
        }
        total.push("y".repeat(PENDING_OUTPUT_LIMIT));
        assert!(total.total_over_limit);
        // 预算截断 UTF-8 边界（中文 3 字节）
        let mut utf8 = OutputBuffer::new();
        utf8.push("中文字符".to_string()); // 12 字节
        let out = utf8.drain(4);
        assert_eq!(out[0], "中"); // 3 字节（4 不是边界，退到 3）
        assert_eq!(utf8.pending[0], "文字符");
    }

    #[test]
    fn cell_seq_ids_unique() {
        // id 形态（cell-N）；唯一性由原子序号保证（行为冒烟）。
        let a = format!("cell-{}", CELL_SEQ.fetch_add(1, Ordering::SeqCst));
        let b = format!("cell-{}", CELL_SEQ.fetch_add(1, Ordering::SeqCst));
        assert_ne!(a, b);
        assert!(a.starts_with("cell-"));
    }
}
