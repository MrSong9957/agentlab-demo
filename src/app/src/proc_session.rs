//! 016 规格：默认可续接的进程会话管理。
//!
//! `exec_command` 与 `write_stdin` 的宿主实现：启动命令（管道或 PTY）→ 返回
//! 会话句柄 → 后台持续收集输出 → 按句柄查询新输出／发送输入／明确终止 →
//! 回收资源。关键取舍（016 已批准）：
//!
//! - 等待（`yield_time_ms`）只决定本次调用何时返回，不终止进程；
//!   总运行时限（`timeout_ms`）由监控线程从成功启动按单调时钟独立计时，
//!   到期终止并回收，不依赖后续查询，也不因读取输出重置。
//! - 取消当前轮只停止本次等待与后续调用，已启动进程仍登记、保留并继续
//!   收集输出；进程生命周期状态在会话记录内，不共享会被下一轮复位的
//!   全局取消标志。
//! - 每对话最多 64 条会话记录（含已退出但最后输出未取走），满额拒绝新建；
//!   每进程一份 1 MiB 待取缓存（超限保留头尾并标记截断），取走后不保留；
//!   等待与读写期间不持有管理器全局锁，取消／查询／退出／终止的竞态经
//!   会话内互斥锁串行化。
//!
//! 平台：仅 Linux 本地执行；Shell 固定 `/bin/sh -c`；句柄绑定本管理器
//! （宿主实例＋当前对话），跨实例或重启后即失效。

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// 038 ConPTY：ChildHandle::Raw 的句柄级 wait/退出码读取与伪终端拆除。
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
#[cfg(windows)]
use windows_sys::Win32::System::Console::{ClosePseudoConsole, HPCON};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;

use crate::exec::{
    cap_output, resolve_workdir, ExecArgs, ExecResult, ExecStatus, WriteStdinArgs,
    MAX_SESSIONS,
};

/// 运行时固定的 Shell：unix＝/bin/sh -c；Windows＝cmd /C（标准平台等价，
/// 038——exec 契约＝经平台 shell 执行命令字符串，退出码经 shell 传递）。
#[cfg(unix)]
const SHELL: &str = "/bin/sh";
#[cfg(windows)]
const SHELL: &str = "cmd";
/// unix＝"-c"；windows＝"/C"（cmd 的执行开关）。
#[cfg(unix)]
const SHELL_EXEC_FLAG: &str = "-c";
#[cfg(windows)]
const SHELL_EXEC_FLAG: &str = "/C";
/// 监控线程轮询子进程退出／终止请求的间隔。
const MONITOR_POLL: Duration = Duration::from_millis(10);
/// 等待循环的单次等待上限（取消标志响应粒度）。
const WAIT_SLICE: Duration = Duration::from_millis(50);
/// 读取线程 poll 间隔（也是收尾放行检查粒度）。
const READ_POLL: i32 = 50;
/// 子进程退出后等待读取线程排干输出通道的上限；超则置 collect_done 并放行
/// 读取线程退出（后代继承管道/PTY 时不能无限等待）。
const COLLECT_GRACE: Duration = Duration::from_secs(2);
/// 明确终止的整个收尾上限（016：3 秒内有界收尾）。
const TERMINATE_GRACE: Duration = Duration::from_secs(3);
/// 读取通道的块大小。
const READ_BUF: usize = 8 * 1024;
/// 每进程待取缓存的字节上限（016：单份 1 MiB，超限保留头尾）。
const PENDING_CAP: usize = 1024 * 1024;
/// 输入分块提交的块大小：块间检查中止标志（取消当前轮时停止提交
/// 尚未发送的输入）；单块内阻塞不可移植中断。
const INPUT_CHUNK: usize = 4096;

/// 工具交互结果：模型面 [`ExecResult`] 之外的展示补充——write_stdin 的
/// 展示需关联原命令（016 §3.3），不伪造一条新命令。
pub struct ToolOutcome {
    pub result: ExecResult,
    /// (会话句柄, 原命令)：调用后仍有可续接会话时提供。
    pub session_cmd: Option<(String, String)>,
}

/// 035：进程归属标签——**宿主可信逻辑标签，不是 Unix UID**（033 的同宿主
/// UID 与独立 PID 命名空间不能替代句柄归属校验）。根轮进程＝`Root`（沿
/// 016 原行为）；子 Agent 进程＝(group, agent, assignment)。兄弟 Agent 猜
/// 句柄操作它人的会话时按 owner 不匹配拒绝；assignment 结束/中断/组关闭
/// 按 (group, agent) 回收其所拥有的命令进程（宿主 PGID，非沙箱内 PID）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerTag {
    Root,
    Agent {
        group: u64,
        agent: String,
        assignment: u64,
    },
}

impl From<ExecResult> for ToolOutcome {
    fn from(result: ExecResult) -> Self {
        Self { result, session_cmd: None }
    }
}

/// 进程会话管理器：宿主（行式／TUI）每对话持有一个，跨用户轮存活。
/// 线程安全；`close` 后拒绝新启动；clone 共享同一份记录。
#[derive(Clone)]
pub struct ProcessManager {
    inner: Arc<MgrInner>,
}

struct MgrInner {
    state: Mutex<MgrState>,
    /// 工具进程创建线程的请求通道（033 生命周期修复，见 [`ProcessManager::new`]
    /// 内线程注释）。Mutex 仅弥补 std mpsc Sender 非 Sync（发送为 O(1) 入队）；
    /// 接收端为专属长寿线程，退出条件＝全部管理器克隆 Drop（通道关闭）。
    spawner_tx: Mutex<std::sync::mpsc::Sender<SpawnJob>>,
}

/// 创建线程的请求（033 生命周期修复）：仅串行化 fork/exec 本身；工具执行、
/// 监控与读取仍按会话并发，创建线程不取任何管理器锁。
/// 031：`policy`＝工具调用开始时冻结的出生策略快照（Arc 携带；创建线程
/// 不临时重读授权——排队创建不因后续授权变化升级权限）；`extra`＝本进程
/// 是否使用了基线之外的额外授权（到期收尾只针对此类进程，§1.7-5）。
struct SpawnJob {
    cmd: String,
    workdir: std::path::PathBuf,
    tty: bool,
    policy: Option<std::sync::Arc<crate::sandbox::PolicySnapshot>>,
    reply: std::sync::mpsc::Sender<std::io::Result<Spawned>>,
}

struct MgrState {
    closed: bool,
    next_id: u64,
    sessions: HashMap<String, Arc<Session>>,
    /// 审查 #2（§1.7-7）：收尾失败闩锁——置位后宿主须拒绝开始下一用户轮，
    /// 直到再次 reap 成功清除（不可杀的额外权限进程仍带着授权存活）。
    reap_failed: Option<String>,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        let (spawner_tx, spawner_rx) = std::sync::mpsc::channel::<SpawnJob>();
        // 033 生命周期修复（归因：docs/logs/033-tool-sandbox/
        // 2026-10-28-e2e-failure-attribution.md）：bwrap `--die-with-parent` 的
        // PDEATHSIG 锚定「创建者内核线程」——此前 spawn 在随轮退出的 turn 线程内
        // 执行，轮结束即整树 SIGKILL（016 跨轮保留/取消契约失效，n2b/n2b2 部署态
        // 失败根因）。工具进程的 fork/exec 改由管理器**专属长寿线程**执行：
        // PDEATHSIG 锚点＝管理器生命周期（全部克隆 Drop／进程退出才触发——宿主
        // 异常退出仍保证载荷停止执行）；仅串行化创建本身，工具执行与监控照旧
        // 并发。关闭顺序：close() 先组杀回收，本线程随通道关闭（克隆全部 Drop）
        // 自然结束——先回收、后结束。线程创建失败＝通道无接收端，start() 的
        // send/recv 显式报错（fail closed，不静默回退调用线程）。
        let _ = std::thread::Builder::new()
            .name("tchat-proc-spawner".into())
            .spawn(move || {
                while let Ok(job) = spawner_rx.recv() {
                    let res = if job.tty {
                        spawn_pty(&job.cmd, &job.workdir, job.policy.as_deref())
                    } else {
                        spawn_piped(&job.cmd, &job.workdir, job.policy.as_deref())
                    };
                    let _ = job.reply.send(res);
                }
            });
        Self {
            inner: Arc::new(MgrInner {
                state: Mutex::new(MgrState {
                    closed: false,
                    next_id: 1,
                    sessions: HashMap::new(),
                    reap_failed: None,
                }),
                spawner_tx: Mutex::new(spawner_tx),
            }),
        }
    }

    /// 仍登记（含已退出未取走输出）的会话列表，按句柄排序；供宿主展示
    /// “已启动的进程仍由会话管理”。
    pub fn live_sessions(&self) -> Vec<(String, String)> {
        let st = self.lock_mgr();
        let mut v: Vec<(String, String)> = st
            .sessions
            .values()
            .map(|s| (s.id.clone(), s.cmd.clone()))
            .collect();
        v.sort();
        v
    }

    /// 查询句柄对应会话的原命令（write_stdin 展示关联原命令用）。
    pub fn session_command(&self, session_id: &str) -> Option<String> {
        self.lock_mgr().sessions.get(session_id).map(|s| s.cmd.clone())
    }

    /// 032：句柄对应会话的真实运行状态（None＝未登记；true＝仍在运行，
    /// false＝已退出但句柄仍登记/输出未取尽——不把后者称作运行中）。
    pub fn session_running(&self, session_id: &str) -> Option<bool> {
        self.lock_mgr()
            .sessions
            .get(session_id)
            .map(|s| matches!(s.phase(), Phase::Running { .. }))
    }

    /// 会话输入发送侧进度（字节；真实写线程累计，只读观测）——用于区分
    /// 发送侧提交与接收侧消费（内核缓冲排空不增加本计数）。
    /// `#[doc(hidden)]`：非承诺接口，仅测试/诊断观测用。
    #[doc(hidden)]
    pub fn session_input_written(&self, session_id: &str) -> Option<u64> {
        self.lock_mgr().sessions.get(session_id).map(|s| s.input_written())
    }

    /// 016：取消当前轮时的统一提示——有活会话则列出编号与命令（宿主共用，
    /// 行式/TUI 同一文案）；无活会话返回 None，由宿主使用无会话文案。
    pub fn live_sessions_note(&self) -> Option<String> {
        let live = self.live_sessions();
        if live.is_empty() {
            return None;
        }
        let list = live
            .iter()
            .map(|(id, cmd)| format!("{id}（{cmd}）"))
            .collect::<Vec<_>>()
            .join("、");
        Some(format!("已停止当前工作；已启动的进程仍由会话管理：{list}"))
    }

    /// 关闭管理器：拒绝新启动，终止并回收全部进程（宿主退出／对话切换）。
    /// 逐会话有界收尾；个别回收失败不阻塞其余会话的清理。
    pub fn close(&self) {
        let taken: Vec<Arc<Session>> = {
            let mut st = self.lock_mgr();
            st.closed = true;
            st.sessions.drain().map(|(_, s)| s).collect()
        };
        for sess in taken {
            sess.request_kill(KillReq::Terminate);
            if !sess.wait_collected(Instant::now() + TERMINATE_GRACE) {
                // 宽限已过：读取线程由 readers_may_stop 放行，不无限等待
                sess.release_readers();
            }
        }
    }

    /// 对话切换成功后重置为空管理器：先关闭旧会话（复用 [`Self::close`]
    /// 的终止回收），再恢复可启动状态。与 `close` 不同，重置后可再次启动。
    pub fn reset(&self) {
        self.close();
        let mut st = self.lock_mgr();
        st.closed = false;
        st.next_id = 1;
    }

    fn lock_mgr(&self) -> std::sync::MutexGuard<'_, MgrState> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn remove_if_present(&self, id: &str) {
        self.lock_mgr().sessions.remove(id);
    }

    /// exec_command 入口：参数校验已在参数层完成；本方法校验截止时间安全
    /// 性并启动进程，等待 `yield_time_ms` 后返回（进程继续运行，不重新启动）。
    pub fn start(&self, args: &ExecArgs, cancel: Option<&AtomicBool>) -> ToolOutcome {
        self.start_with_policy(args, cancel, None, false)
    }

    /// 审查 #2（§1.7-7）：收尾失败门查询——Some(reason) 表示存在未证实终止
    /// 的额外权限进程，宿主必须拒绝开始下一用户轮（可先重试 reap 以恢复）。
    pub fn extra_reap_gate(&self) -> Option<String> {
        self.lock_mgr().reap_failed.clone()
    }

    /// 031 §1.7-4/7：轮出口统一收尾——终止并回收全部「额外权限」进程。
    /// 统一先 SIGTERM，再共享 3 秒有界窗口等待全部（不逐项串行 3 秒）；到期
    /// 仍未终止的组升级 SIGKILL（走 monitor 的既有终止/回收路径）；无法证实
    /// 全部已终止＝Err（收尾失败如实报告，不宣称已安全撤销，宿主据此拒绝开
    /// 始下一轮）。基线权限进程不受影响（016 可续接语义保留）。
    pub fn reap_extra_permission_sessions(&self) -> Result<(), String> {
        let extras: Vec<Arc<Session>> = {
            let st = self.lock_mgr();
            st.sessions.values().filter(|s| s.extra).cloned().collect()
        };
        if extras.is_empty() {
            self.lock_mgr().reap_failed = None;
            return Ok(());
        }
        // 统一温和终止（一次发齐）。
        for s in &extras {
            if let Phase::Running { pgid } = s.phase() {
                request_group_term(pgid);
            }
        }
        // 共享有界窗口。
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if extras.iter().all(|s| !matches!(s.phase(), Phase::Running { .. })) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // 升级强杀（monitor 回收），再短窗核实。
        for s in &extras {
            if matches!(s.phase(), Phase::Running { .. }) {
                s.request_kill(KillReq::Terminate);
            }
        }
        let hard_deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let pending: Vec<String> = extras
                .iter()
                .filter(|s| matches!(s.phase(), Phase::Running { .. }))
                .map(|s| s.id.clone())
                .collect();
            if pending.is_empty() {
                return Ok(());
            }
            if Instant::now() >= hard_deadline {
                let msg = format!(
                    "额外权限进程未能全部终止（{}）：不宣称已安全撤销，拒绝开始下一轮",
                    pending.join("、")
                );
                self.lock_mgr().reap_failed = Some(msg.clone());
                return Err(msg);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 035 §1.5：回收一个 assignment 的命令进程（子轮终态/中断调用）——
    /// 精确匹配 (group, agent, assignment)，不误伤同 Agent 后续 assignment
    /// 或兄弟进程。统一先 SIGTERM、共享 3 秒有界窗口、到期升级 SIGKILL
    /// （monitor 回收路径），随后移除记录；未证实全部终止＝Err（如实报告，
    /// 不宣称已安全撤销）。
    pub fn reap_agent_assignment(
        &self,
        group: u64,
        agent: &str,
        assignment: u64,
    ) -> Result<(), String> {
        self.reap_where(&|owner: &OwnerTag| {
            matches!(
                owner,
                OwnerTag::Agent {
                    group: g,
                    agent: a,
                    assignment: asg,
                } if *g == group && a == agent && *asg == assignment
            )
        })
        .map_err(|pending| {
            format!("Agent 进程未能全部终止（{}）：不宣称已安全撤销", pending.join("、"))
        })
    }

    /// 035 §1.5：回收整组 Agent 进程（组关闭；该组全部 Agent 全部
    /// assignment；Root 与其他组不受影响）。
    pub fn reap_agent_group(&self, group: u64) -> Result<(), String> {
        self.reap_where(&|owner: &OwnerTag| {
            matches!(owner, OwnerTag::Agent { group: g, .. } if *g == group)
        })
        .map_err(|pending| {
            format!("Agent 组进程未能全部终止（{}）：不宣称已安全撤销", pending.join("、"))
        })
    }

    /// 回收谓词选中的会话（上方两方法的共用实现；含全局 64 容量释放与
    /// killpg 走宿主 PGID——沙箱内 PID 不是宿主进程组号）。
    fn reap_where(&self, pred: &dyn Fn(&OwnerTag) -> bool) -> Result<(), Vec<String>> {
        let owned: Vec<Arc<Session>> = {
            let st = self.lock_mgr();
            st.sessions
                .values()
                .filter(|s| pred(&s.owner))
                .cloned()
                .collect()
        };
        if owned.is_empty() {
            return Ok(());
        }
        for s in &owned {
            if let Phase::Running { pgid } = s.phase() {
                request_group_term(pgid);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if owned.iter().all(|s| !matches!(s.phase(), Phase::Running { .. })) {
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        for s in &owned {
            if matches!(s.phase(), Phase::Running { .. }) {
                s.request_kill(KillReq::Terminate);
            }
        }
        let hard_deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if owned.iter().all(|s| !matches!(s.phase(), Phase::Running { .. })) {
                break;
            }
            if Instant::now() >= hard_deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut pending = Vec::new();
        {
            let mut st = self.lock_mgr();
            for s in &owned {
                if matches!(s.phase(), Phase::Running { .. }) {
                    pending.push(s.id.clone());
                } else {
                    st.sessions.remove(&s.id);
                }
            }
        }
        if pending.is_empty() {
            Ok(())
        } else {
            Err(pending)
        }
    }

    /// [`ProcessManager::start`] 的 031 变体：`policy`＝出生策略快照（调用
    /// 开始时构建，经创建线程携带——不临时重读授权）；`extra`＝是否使用基线
    /// 之外的额外授权（到期收尾只针对此类进程，§1.7-5——016 对该条的修订：
    /// 额外权限进程不跨用户轮存活）。毒丸快照＝显式错误，零命令执行。
    pub fn start_with_policy(
        &self,
        args: &ExecArgs,
        cancel: Option<&AtomicBool>,
        policy: Option<&std::sync::Arc<crate::sandbox::PolicySnapshot>>,
        extra: bool,
    ) -> ToolOutcome {
        self.start_with_owner(args, cancel, policy, extra, OwnerTag::Root)
    }

    /// 035：带归属标签的启动——子 Agent 的命令进程按 (group, agent,
    /// assignment) 登记（句柄校验与按 owner 回收的依据；根入口沿用
    /// `start_with_policy`＝Root）。
    pub fn start_with_owner(
        &self,
        args: &ExecArgs,
        cancel: Option<&AtomicBool>,
        policy: Option<&std::sync::Arc<crate::sandbox::PolicySnapshot>>,
        extra: bool,
        owner: OwnerTag,
    ) -> ToolOutcome {
        if let Some(reason) = policy.and_then(|p| p.poison.as_ref()) {
            return ExecResult::error(format!(
                "生效策略无法编码：{reason}（本次执行拒绝，不回退普通模式）"
            ))
            .into();
        }
        let workdir = match resolve_workdir(args.workdir.as_deref()) {
            Ok(p) => p,
            Err(msg) => return ExecResult::error(msg).into(),
        };
        // 数值→时长→截止时间安全性：在允许执行前校验（失败不产生命令副作用）；
        // 实际截止时间在启动成功后计算（§3.1“从成功启动开始计时”）。
        if args.timeout_ms.is_some_and(|ms| compute_deadline(ms).is_none()) {
            let ms = args.timeout_ms.unwrap_or(0);
            return ExecResult::error(format!(
                "timeout_ms={ms} 无法安全转换为截止时间，拒绝执行"
            ))
            .into();
        }
        // 取消发生在启动前：不启动进程（零命令副作用），返回未执行原因与空句柄。
        if let Some(r) = cancelled_before_start(cancel) {
            return r.into();
        }

        // 持管理器锁完成容量检查＋启动＋登记：退出与在途启动并发时不漏管
        //（close 持同一锁取走全部记录，此后启动见 closed 拒绝）。
        let (session, sp, deadline) = {
            let mut st = self.lock_mgr();
            if st.closed {
                return ExecResult::error("执行会话已关闭，拒绝新启动".into()).into();
            }
            if st.sessions.len() >= MAX_SESSIONS {
                return ExecResult::error(format!(
                    "会话记录已达上限（{MAX_SESSIONS} 条，含已退出未取走输出的记录）；\
                     请查询收尾或明确终止不再使用的会话后再启动"
                ))
                .into();
            }
            if let Some(r) = cancelled_before_start(cancel) {
                return r.into();
            }
            let spawned = {
                // 033 生命周期修复：fork/exec 经管理器长寿创建线程执行（PDEATHSIG
                // 锚点＝管理器生命周期；原先随轮退出的调用线程会连带杀树）。等待
                // 回复期间持管理器锁是既有顺序（spawn 本就持锁执行）；创建线程
                // 不取任何锁，无死锁环。
                let (reply_tx, reply_rx) = std::sync::mpsc::channel();
                let job = SpawnJob {
                    cmd: args.cmd.clone(),
                    workdir: workdir.clone(),
                    tty: args.tty,
                    policy: policy.cloned(),
                    reply: reply_tx,
                };
                if let Err(_e) = self
                    .inner
                    .spawner_tx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .send(job)
                {
                    return ExecResult::error("进程启动失败：工具创建线程已退出".into()).into();
                }
                match reply_rx.recv() {
                    Ok(res) => res,
                    Err(_) => {
                        return ExecResult::error("进程启动失败：工具创建线程已退出".into())
                            .into()
                    }
                }
            };
            match spawned {
                Ok(mut sp) => {
                    // 038 方案 A：沙箱启动确认闸——在调用线程内同步执行（cancel
                    // 引用仅此处可用）。载荷由自停前缀停在首条用户命令之前；
                    // 确认「隔离成立＋本会话清理标识（载荷 pid ns）已取得」后
                    // 放行（SIGCONT）。超时/失败/取消＝不放行：确认函数内整组
                    // 终止＋有界回收（016），本调用返回 Error；monitor 尚未
                    // 启动，无收尸竞争。确认失败路径的 child 对象随 sp drop
                    // （已死无僵尸）。
                    #[cfg(unix)]
                    if policy.is_some() {
                        match crate::sandbox::confirm_sandbox_isolation_and_release(sp.child.id(), cancel) {
                            Ok(crate::sandbox::SandboxConfirm::Released(ns)) => sp.ns_inode = Some(ns),
                            Ok(crate::sandbox::SandboxConfirm::LeaderGone) => {
                                // --die-with-parent 级联已终止载荷：按 016 自然
                                // 退出收尾（monitor 启动后立即收到退出状态）。
                            }
                            Err(e) => {
                                return ExecResult::error(format!("进程启动失败：{e}")).into();
                            }
                        }
                    }
                    // 启动成功——此刻起计算总运行时限（单调时钟；可表示性已预检）
                    let deadline = args
                        .timeout_ms
                        .map(|ms| compute_deadline(ms).expect("预检已确认可表示"));
                    #[cfg(test)]
                    TEST_DEADLINE_JOURNAL.with_borrow_mut(|j| j.push(deadline));
                    let id = format!("s{}", st.next_id);
                    st.next_id += 1;
                    let readers_total = if args.tty { 1 } else { 2 };
                    let session = Arc::new(Session::new(
                        id,
                        args.cmd.clone(),
                        sp.child.id() as GroupId,
                        sp.master.clone(),
                        readers_total,
                        extra,
                        owner,
                        sp.ns_inode.clone(),
                    ));
                    st.sessions.insert(session.id.clone(), session.clone());
                    (session, sp, deadline)
                }
                Err(e) => {
                    return ExecResult::error(format!("进程启动失败：{e}")).into();
                }
            }
        };

        // 后台线程：监控（退出回收／时限终止）＋ 读取（持续收集输出）。
        #[cfg(windows)]
        {
            // 作业挂接失败＝失去树终止能力（016 契约）——显式 panic，不静默。
            let (job_pid, job_handle) = sp.child.raw_process();
            if let Err(e) = win::assign_raw_process(job_pid, job_handle) {
                panic!("作业对象挂接失败（Windows 树终止不可用）：{e}");
            }
        }
        let monitor_session = session.clone();
        std::thread::spawn(move || {
            monitor_thread(monitor_session, sp.child, deadline)
        });
        if let Some(out) = sp.stdout {
            spawn_reader(session.clone(), out);
        }
        if let Some(err) = sp.stderr {
            spawn_reader(session.clone(), err);
        }
        if let Some(pty_reader) = &sp.pty_reader {
            // 038 ConPTY：输入/输出双句柄——读取线程接 ConPTY 输出通道。
            if let Ok(reader) = pty_reader.try_clone() {
                spawn_reader(session.clone(), reader);
            }
        } else if let Some(master) = &sp.master {
            if let Ok(reader) = master.try_clone() {
                spawn_reader(session.clone(), reader);
            }
        }

        // 本次调用等待：到期返回 running＋句柄；提前结束返回终态并移除记录。
        let deadline = Instant::now() + Duration::from_millis(args.yield_time_ms);
        match session.wait_and_collect(deadline, cancel, args.max_output_chars) {
            Some(r) => ToolOutcome {
                result: r,
                session_cmd: Some((session.id.clone(), args.cmd.clone())),
            },
            None => {
                let r = session.assemble_terminal(args.max_output_chars);
                self.remove_if_present(&session.id);
                r.into()
            }
        }
    }

    /// write_stdin 入口：查询／输入／中断／明确终止共用一套句柄校验与
    /// 终态收尾；参数互斥与输入上限在参数层已校验。
    pub fn interact(&self, args: &WriteStdinArgs, cancel: Option<&AtomicBool>) -> ToolOutcome {
        self.interact_as_owner(args, cancel, &OwnerTag::Root)
    }

    /// 035：带归属校验的 write_stdin——会话 owner 须与调用者一致（兄弟
    /// Agent 猜句柄操作它人会话＝拒绝；根入口 `interact`＝Root 原行为）。
    pub fn interact_as_owner(
        &self,
        args: &WriteStdinArgs,
        cancel: Option<&AtomicBool>,
        caller: &OwnerTag,
    ) -> ToolOutcome {
        let Some(session) = self.lock_mgr().sessions.get(&args.session_id).cloned() else {
            return ExecResult::error(format!(
                "未知或已失效的会话句柄：{}（句柄仅在当前对话内有效）",
                args.session_id
            ))
            .into();
        };
        if &session.owner != caller {
            // 035 安全 F3：与「不存在」共用同一文案——句柄为顺序号，可枚举
            // 探测；归属不符若可区分即构成存在性预言（操作本就被拒，这里
            // 只消除信息泄露面）。
            return ExecResult::error(format!(
                "未知或已失效的会话句柄：{}（句柄仅在当前对话内有效）",
                args.session_id
            ))
            .into();
        }
        let command = session.cmd.clone();
        let attach = |mut outcome: ToolOutcome| {
            if outcome.result.session_id.is_some() {
                outcome.session_cmd = Some((args.session_id.clone(), command.clone()));
            }
            outcome
        };

        // 已到终态（最后输出未取走）：取走最终输出与真实退出状态并移除记录。
        if session.is_collected() {
            let r = session.assemble_terminal(args.max_output_chars);
            self.remove_if_present(&args.session_id);
            return attach(r.into());
        }

        // 明确终止：终止整个受管理进程组并回收，3 秒内有界收尾；
        // 清理失败如实报告，不误报“已终止”（进程保留、句柄保留，可重试）。
        if args.terminate {
            session.clear_kill_failed();
            session.request_kill(KillReq::Terminate);
            let deadline = Instant::now() + TERMINATE_GRACE;
            loop {
                if session.is_collected() {
                    let r = session.assemble_terminal(args.max_output_chars);
                    self.remove_if_present(&args.session_id);
                    return attach(r.into());
                }
                if let Some((_, errno)) = session.kill_failed_snapshot() {
                    return attach(
                        ExecResult::error_with_session(
                            format!(
                                "会话 {} 终止失败（errno={errno}），未确认清理成功；进程仍在运行，句柄保留，可重试 terminate",
                                args.session_id
                            ),
                            args.session_id.clone(),
                        )
                        .into(),
                    );
                }
                let now = Instant::now();
                if now >= deadline {
                    return attach(
                        ExecResult::error_with_session(
                            format!(
                                "会话 {} 终止后 {} 秒内未能完成回收，请稍后重试或检查进程状态",
                                args.session_id,
                                TERMINATE_GRACE.as_secs()
                            ),
                            args.session_id.clone(),
                        )
                        .into(),
                    );
                }
                std::thread::sleep(WAIT_SLICE.min(deadline.saturating_duration_since(now)));
            }
        }

        // 输入：管道会话关闭文字输入（仅 Ctrl+C 特例）；PTY 原样写入字符。
        if !args.chars.is_empty() {
            if args.chars != "\u{3}" && !session.tty() {
                return attach(ExecResult {
                    status: ExecStatus::Error,
                    exit_code: None,
                    output: String::new(),
                    truncated: false,
                    error: Some(
                        "管道会话不支持文字输入（stdin 已关闭）；仅支持发送 Ctrl+C（chars=\"\\u0003\"）请求中断，或 terminate=true 明确终止"
                            .into(),
                    ),
                    session_id: Some(args.session_id.clone()),
                }
                .into());
            }
            if session.tty() {
                // 写入在辅助线程进行——对不读输入的程序写入可能阻塞，
                // 必须可取消，不能阻塞整个宿主。
                let (tx, rx) = mpsc::channel::<std::io::Result<()>>();
                let writer = match session.input_writer() {
                    Some(w) => w,
                    None => {
                        return attach(ExecResult::error(format!(
                            "会话 {} 无可用输入通道",
                            args.session_id
                        ))
                        .into())
                    }
                };
                let bytes = args.chars.clone();
                // 中止标志：取消时停止提交尚未发送的分块（进行中的分块至多
                // 再送达一块——块内 syscall 不可移植中断）。
                let abort = Arc::new(AtomicBool::new(false));
                let writer_abort = abort.clone();
                let writer_session = session.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(writer.write_chunked(
                        bytes.as_bytes(),
                        &writer_abort,
                        &writer_session,
                    ));
                });
                // 写入阶段与输出等待共用同一等待窗口（yield 本身有界）；
                // 程序不读输入时窗口到期如实返回运行中。
                let write_deadline = Instant::now() + Duration::from_millis(args.yield_time_ms);
                loop {
                    match rx.recv_timeout(WAIT_SLICE) {
                        Ok(Ok(())) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Err(e)) => {
                            if e.kind() == std::io::ErrorKind::Interrupted
                                && cancel.is_some_and(|c| c.load(Ordering::SeqCst))
                            {
                                // 写入线程因中止标志退出＝取消路径
                                abort.store(true, Ordering::SeqCst);
                                let r =
                                    input_cancelled_result(&session, args.max_output_chars, &args.session_id);
                                return attach(r.into());
                            }
                            return attach(
                                ExecResult::error_with_session(
                                    format!("向会话 {} 写入输入失败：{e}", args.session_id),
                                    args.session_id.clone(),
                                )
                                .into(),
                            );
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                                // 取消：停止等待，并停止提交尚未发送的输入分块；
                                // 已成功写入的内容无法撤回，如实说明。
                                abort.store(true, Ordering::SeqCst);
                                let r =
                                    input_cancelled_result(&session, args.max_output_chars, &args.session_id);
                                return attach(r.into());
                            }
                            if Instant::now() >= write_deadline {
                                // 等待窗口内未能写完（程序不读输入）：如实返回运行中。
                                let r = session
                                    .collect_partial(args.max_output_chars, args.session_id.clone());
                                return attach(r.into());
                            }
                        }
                    }
                }
            } else {
                // 管道会话的 Ctrl+C：向进程组发送 SIGINT（只请求中断，不保证退出）。
                session.interrupt();
            }
        }

        // 等待新输出／终态；取消只结束本次等待，不动进程。
        let deadline = Instant::now() + Duration::from_millis(args.yield_time_ms);
        match session.wait_and_collect(deadline, cancel, args.max_output_chars) {
            Some(r) => attach(r.into()),
            None => {
                let r = session.assemble_terminal(args.max_output_chars);
                self.remove_if_present(&args.session_id);
                attach(r.into())
            }
        }
    }
}

/// 截止时间计算单点（单调时钟；016 §3.1“数值→时长→截止时间安全性”）。
///
/// 本平台依据（如实说明，非业务上限）：std 的 Linux `Instant` 可表示范围
/// （秒域 i64，约 ±9.2e18 秒）远大于 u64 毫秒域（`u64::MAX` 毫秒 ≈
/// 1.8e16 秒），故 `checked_add` 对全部 u64 毫秒值返回 `Some`——数值防御
/// 分支在本平台不可自然触达，只能经下方测试接缝注入失败验证拒绝路径。
///
/// 测试接缝（仅 `cfg(test)`，线程局部，不新增公共配置/时钟框架）：
/// - [`TEST_DEADLINE_FAILS`] 置位后强制返回 `None`（验证“拒绝执行”）；
/// - [`TEST_DEADLINE_JOURNAL`] 记录每次成功启动的截止时间（验证省略＝
///   无时限、大于旧上限的合法值被接受、查询不改变截止）。
fn compute_deadline(ms: u64) -> Option<Instant> {
    #[cfg(test)]
    {
        if TEST_DEADLINE_FAILS.with(std::cell::Cell::get) {
            return None;
        }
    }
    Instant::now().checked_add(Duration::from_millis(ms))
}

/// 测试接缝：置位后本线程的截止时间计算强制失败（见 [`compute_deadline`]）。
#[cfg(test)]
thread_local! {
    static TEST_DEADLINE_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 测试接缝：本线程成功启动的会话截止时间记录（None＝未设总时限）。
#[cfg(test)]
thread_local! {
    static TEST_DEADLINE_JOURNAL: std::cell::RefCell<Vec<Option<Instant>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// 测试接缝：置为指定进程组后，对该组的 SIGKILL 终止注入 EPERM 失败
/// （仅 cfg(test)；按 pgid 限定，不影响并行其他测试的真实终止）。
#[cfg(test)]
static TEST_KILLPG_FAIL_PGD: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(-1);

/// 单调时钟当前读数——监控线程**到期判定**的唯一入口（016 §3.1）。
/// 测试接缝：`TEST_CLOCK_SHIFT_MS` 置位后读数整体前移，可稳定驱动真实
/// 到期判定（终止真实进程、验证副作用），不依赖宽松的真实时间容差；
/// 截止时间的**计算**（compute_deadline）仍用真实时钟。仅无时限会话
/// 不经过本判定，故前移不影响未设 timeout 的并行用例。
fn monotonic_now() -> Instant {
    #[cfg(test)]
    {
        let shift = TEST_CLOCK_SHIFT_MS.load(Ordering::SeqCst);
        if shift > 0 {
            return Instant::now() + Duration::from_millis(shift);
        }
    }
    Instant::now()
}

/// 测试接缝：单调读数前移量（毫秒）。进程内静态（监控线程跨线程读取）；
/// 仅 SEAM_LOCK 串行的接缝测试在设有时限的会话上使用。
#[cfg(test)]
static TEST_CLOCK_SHIFT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 测试接缝的串行锁（journal/killpg/时钟前移注入类测试共用，防相互交错）。
/// **约束**：凡创建带 `timeout_ms` 会话的单测必须持有本锁——`TEST_CLOCK_SHIFT_MS`
/// 为进程内静态，无锁的并行限时用例会被前移读数误判到期。
#[cfg(test)]
static SEAM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 收口缝类型（仅 cfg(test)）：监控门控状态——Paused＝测试已布防、监控未达；
/// Parked＝监控已停靠（不持会话锁）；Released＝放行。
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TestGateState {
    Paused,
    Parked,
    Released,
}

/// 收口缝类型（仅 cfg(test)）：门控共享句柄（状态＋通知）。
#[cfg(test)]
type TestGate = Arc<(Mutex<TestGateState>, Condvar)>;

/// 收口缝守卫（仅 cfg(test)）：Drop 自动放行——panic 展开亦经过，监控不会
/// 因测试失败而悬挂（另有 10s 有界兜底）。
#[cfg(test)]
struct TestGateGuard(TestGate);

#[cfg(test)]
impl Drop for TestGateGuard {
    fn drop(&mut self) {
        let (lock, cv) = &*self.0;
        let mut g = lock.lock().unwrap_or_else(|p| p.into_inner());
        *g = TestGateState::Released;
        cv.notify_all();
    }
}

/// 收口缝（仅 cfg(test)）：监控循环顶部停靠点。**不持会话锁**等待——测试在
/// 停靠后提交显式 terminate（request_kill 的覆盖规则把挂起的 Deadline 请求
/// 替换为 Terminate），提交可见即自动放行：把「terminate 提交」确定性编排到
/// 「Deadline 轮询重试消费」之前，消除 10ms tick 的调度竞态（2026-10-27
/// 全量 flake 根因；被验证的语义本身不变）。放行条件：测试释放（守卫 Drop）／
/// kill 请求已被替换为 Terminate／10s 有界兜底——异常路径监控仍可推进、
/// 进程仍可回收。未布防（None／非 Paused）即时返回，并行用例零影响。
#[cfg(test)]
fn test_gate_wait(session: &Session) {
    let Some(gate) = session.lock().test_gate.clone() else {
        return;
    };
    let (lock, cv) = &*gate;
    let mut g = lock.lock().unwrap_or_else(|p| p.into_inner());
    if *g != TestGateState::Paused {
        return;
    }
    *g = TestGateState::Parked;
    cv.notify_all();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if *g == TestGateState::Released
            || session.kill_snapshot() == Some(KillReq::Terminate)
        {
            return;
        }
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let slice = Duration::from_millis(5).min(deadline.saturating_duration_since(now));
        let (ng, _) = cv.wait_timeout(g, slice).unwrap_or_else(|e| e.into_inner());
        g = ng;
    }
}

/// 终止进程组的受检入口：返回 Ok（已发信号）／Gone（组已不存在）／Failed
///（失败，含 errno）。测试注入仅对 [`TEST_KILLPG_FAIL_PGD`] 匹配的组生效。
/// 组硬终止（unix＝killpg(SIGKILL)＋errno 分类；Windows＝作业对象整树终止——
/// 后代默认继承作业，TerminateJobObject 覆盖整树；组不存在＝句柄已关闭）。
#[cfg(unix)]
fn kill_group_sigkill(pgid: GroupId) -> KillOutcome {
    let rc = unsafe { libc::killpg(pgid, libc::SIGKILL) };
    if rc == 0 {
        return KillOutcome::Ok;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EPERM);
    if errno == libc::ESRCH {
        KillOutcome::Gone
    } else {
        KillOutcome::Failed(errno)
    }
}

/// Windows：整树终止。SIGINT 温和中断无同构等价——以整树终止实现（残差登记）。
#[cfg(windows)]
fn kill_group_sigkill(pgid: GroupId) -> KillOutcome {
    match win::terminate_tree(pgid) {
        Ok(()) => KillOutcome::Ok,
        Err(e) if e.to_string().contains("不存在") => KillOutcome::Gone,
        Err(e) => KillOutcome::Failed(e.raw_os_error().unwrap_or(5)),
    }
}

/// 组温和终止（unix＝killpg(SIGTERM)——3 秒窗口后硬终止兜底）。
#[cfg(unix)]
fn request_group_term(pgid: GroupId) {
    unsafe {
        libc::killpg(pgid, libc::SIGTERM);
    }
}

/// Windows：温和档无同构等价——以整树终止实现。
#[cfg(windows)]
fn request_group_term(pgid: GroupId) {
    let _ = kill_group_sigkill(pgid);
}

/// 组中断请求（unix＝killpg(SIGINT)——只请求中断，不保证退出）。
#[cfg(unix)]
fn request_group_interrupt(pgid: GroupId) {
    // 038 取证（pipe_ctrl_c②）：kill 前后状态快照（结案后移除）
    let pre = std::fs::read_to_string(format!("/proc/{pgid}/status")).ok();
    unsafe {
        libc::killpg(pgid, libc::SIGINT);
    }
    std::thread::sleep(Duration::from_millis(150));
    let post = std::fs::read_to_string(format!("/proc/{pgid}/status")).ok();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
        .open("/dev/shm/int-trace")
    {
        use std::io::Write as _;
        let _ = writeln!(f, "== pgid {pgid}");
        for (tag, st) in [("PRE", &pre), ("POST150ms", &post)] {
            match st {
                Some(text) => {
                    for l in text.lines() {
                        if l.starts_with("State:")
                            || l.starts_with("SigIgn")
                            || l.starts_with("SigBlk")
                            || l.starts_with("SigCgt")
                            || l.starts_with("PPid:")
                        {
                            let _ = writeln!(f, "{tag} {l}");
                        }
                    }
                }
                None => {
                    let _ = writeln!(f, "{tag} <gone>");
                }
            }
        }
    }
}

/// Windows：中断档以整树终止实现（残差登记）。
#[cfg(windows)]
fn request_group_interrupt(pgid: GroupId) {
    let _ = kill_group_sigkill(pgid);
}

fn kill_group_checked(pgid: GroupId) -> KillOutcome {
    #[cfg(test)]
    {
        if TEST_KILLPG_FAIL_PGD.load(Ordering::SeqCst) == pgid as i64 {
            return KillOutcome::Failed(libc::EPERM);
        }
    }
    kill_group_sigkill(pgid)
}

/// killpg 的三种结局。
enum KillOutcome {
    /// 信号已发送。
    Ok,
    /// 组已不存在（此前已退出）——视同已终止，继续回收。
    Gone,
    /// 发送失败（errno）——进程仍在运行，不得报告已终止。
    Failed(i32),
}

/// 取消发生在启动前的统一结果：无命令副作用、空句柄。
fn cancelled_before_start(cancel: Option<&AtomicBool>) -> Option<ExecResult> {
    cancel
        .is_some_and(|c| c.load(Ordering::SeqCst))
        .then(|| ExecResult {
            status: ExecStatus::Cancelled,
            exit_code: None,
            output: String::new(),
            truncated: false,
            error: Some("用户取消，本调用未执行".into()),
            session_id: None,
        })
}

// ---- 会话记录 ----

/// 一次受管理的进程执行：生命周期状态＋待取输出缓存＋输入通道。
struct Session {
    id: String,
    cmd: String,
    /// 038 D2：沙箱 pid 命名空间标识（spawn 时 /proc/<leader>/ns/pid 的
    /// readlink）——组长死后按它补杀残留成员；非沙箱会话为 None。
    ns_inode: Option<String>,
    /// 031：本进程是否以「基线＋额外授权」出生（§1.7-5——到期收尾只针对
    /// 此类进程；基线进程继续 016 可续接语义）。
    extra: bool,
    /// 035：宿主可信归属标签（句柄校验与按 owner 回收的依据）。
    owner: OwnerTag,
    st: Mutex<SessionState>,
    cv: Condvar,
}

struct SessionState {
    phase: Phase,
    pending: PendingBuffer,
    /// 终态且输出通道已收尾（读取线程 EOF 或宽限到）：可组装最终结果。
    collect_done: bool,
    /// 读取线程放行退出标志（进程已死且宽限已过／管理器关闭）。
    readers_may_stop: bool,
    /// 终止请求（时限到期或 terminate=true）；由监控线程执行。
    kill: Option<KillReq>,
    /// 最近一次终止请求的失败（原因＋errno）——如实记录，不伪装已终止；
    /// 进程仍在运行、请求保留重试；成功终止或自然退出后不再被读取。
    kill_failed: Option<(KillReq, i32)>,
    /// PTY master（输入通道）；管道会话为 None。
    master: Option<Arc<File>>,
    /// 输入发送侧进度（字节）：真实写线程每成功写出一块累计——区分
    /// “发送侧已提交”与“接收侧已消费”（内核缓冲排空≠新发送）。
    input_written: u64,
    /// 已报告 EOF 的读取线程数。
    reader_eof: usize,
    /// 读取线程总数（管道 2：stdout/stderr；PTY 1：master）。
    readers: usize,
    /// 收口缝（仅 cfg(test)）：监控循环门控——见 [`test_gate_wait`]。
    #[cfg(test)]
    test_gate: Option<TestGate>,
}

/// 进程组标识：unix＝pgid；Windows＝主子进程 PID（作业对象查找键）。
#[cfg(unix)]
type GroupId = libc::pid_t;
#[cfg(windows)]
type GroupId = u32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running { pgid: GroupId },
    /// 自然退出（含被外部信号杀死：code=None，不编造退出码）。
    Exited { code: Option<i32> },
    /// 总运行时限到期被终止。
    TimedOut,
    /// 被明确终止。
    Terminated,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KillReq {
    Deadline,
    Terminate,
}

impl Session {
    /// 生命周期快照（032 运行态说明用；克隆轻量）。
    fn phase(&self) -> Phase {
        self.st.lock().unwrap_or_else(|e| e.into_inner()).phase.clone()
    }

    fn new(
        id: String,
        cmd: String,
        pgid: GroupId,
        master: Option<Arc<File>>,
        readers: usize,
        extra: bool,
        owner: OwnerTag,
        ns_inode: Option<String>,
    ) -> Self {
        Self {
            id,
            cmd,
            extra,
            owner,
            ns_inode,
            st: Mutex::new(SessionState {
                phase: Phase::Running { pgid },
                pending: PendingBuffer::new(),
                collect_done: false,
                readers_may_stop: false,
                kill: None,
                kill_failed: None,
                input_written: 0,
                master,
                reader_eof: 0,
                readers,
                #[cfg(test)]
                test_gate: None,
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tty(&self) -> bool {
        self.lock().master.is_some()
    }

    fn is_collected(&self) -> bool {
        self.lock().collect_done
    }

    /// 写线程报告一块输入已成功送出（发送侧真实进度）。
    fn note_input_written(&self, n: usize) {
        self.lock().input_written += n as u64;
    }

    /// 输入发送侧进度（字节）。
    fn input_written(&self) -> u64 {
        self.lock().input_written
    }

    /// 最近一次终止失败的快照（进程仍在运行的如实说明依据）。
    fn kill_failed_snapshot(&self) -> Option<(KillReq, i32)> {
        self.lock().kill_failed
    }

    /// 新终止尝试开始时清除上次失败记录——每次尝试只报告本次观察到的失败，
    /// 不让陈旧失败阻塞后续重试。
    fn clear_kill_failed(&self) {
        self.lock().kill_failed = None;
    }

    /// 收口缝（仅 cfg(test)）：布防监控门控，返回守卫（Drop 自动放行）。
    /// 会话级定位，无全局覆盖污染；仅在 SEAM_LOCK 串行的接缝测试内使用。
    #[cfg(test)]
    fn arm_monitor_gate(&self) -> TestGateGuard {
        let gate: TestGate = Arc::new((Mutex::new(TestGateState::Paused), Condvar::new()));
        self.lock().test_gate = Some(gate.clone());
        TestGateGuard(gate)
    }

    /// 收口缝（仅 cfg(test)）：读门控状态（测试侧等停靠用）。
    #[cfg(test)]
    fn monitor_gate_state(&self) -> Option<TestGateState> {
        let gate = self.lock().test_gate.clone()?;
        let (lock, _) = &*gate;
        let state = *lock.lock().unwrap_or_else(|p| p.into_inner());
        Some(state)
    }

    /// 收口缝（仅 cfg(test)）：终止请求快照（门控自动放行条件）。
    #[cfg(test)]
    fn kill_snapshot(&self) -> Option<KillReq> {
        self.lock().kill
    }

    /// 请求终止进程组并唤醒监控线程（轮询兜底）；只对运行中会话生效。
    /// 用户显式 terminate 优先于已过期挂起的时限请求（时限已到而终止尚未
    /// 完成，显式终止成功后事由归 terminate——请求共存时不得静默丢弃）。
    fn request_kill(&self, req: KillReq) {
        let mut st = self.lock();
        if matches!(st.phase, Phase::Running { .. })
            && (st.kill.is_none()
                || (st.kill == Some(KillReq::Deadline) && req == KillReq::Terminate))
        {
            st.kill = Some(req);
            drop(st);
            self.cv.notify_all();
        }
    }

    /// 管道会话的 Ctrl+C：向进程组发送 SIGINT（只请求中断，不保证退出）。
    fn interrupt(&self) {
        let st = self.lock();
        if let Phase::Running { pgid } = st.phase {
            request_group_interrupt(pgid);
        }
    }

    /// PTY 输入通道的写入端。
    fn input_writer(&self) -> Option<InputWriter> {
        self.lock().master.clone().map(InputWriter)
    }

    /// 等待直到终态输出收尾完成；超时返回 false。
    fn wait_collected(&self, deadline: Instant) -> bool {
        let mut st = self.lock();
        while !st.collect_done {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _) = self
                .cv
                .wait_timeout(st, deadline.saturating_duration_since(now).min(WAIT_SLICE))
                .unwrap_or_else(|e| e.into_inner());
            st = guard;
        }
        true
    }

    /// 宽限后强制放行读取线程（管理器关闭兜底）。
    fn release_readers(&self) {
        let mut st = self.lock();
        st.readers_may_stop = true;
        drop(st);
        self.cv.notify_all();
    }

    /// 等待新输出或终态：返回 None 表示终态收尾完成（调用方组装最终结果并
    /// 移除记录）；Some(result) 为运行中／取消的中间结果。
    fn wait_and_collect(
        &self,
        deadline: Instant,
        cancel: Option<&AtomicBool>,
        max_output_chars: usize,
    ) -> Option<ExecResult> {
        let mut st = self.lock();
        loop {
            if st.collect_done {
                return None;
            }
            if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                // 取消等待 ≠ 进程停止：保留句柄，取走已收集输出。
                let (output, truncated) = drain(&mut st, max_output_chars, false);
                drop(st);
                return Some(ExecResult {
                    status: ExecStatus::Cancelled,
                    exit_code: None,
                    output,
                    truncated,
                    error: Some(format!(
                        "等待被用户取消；进程仍在运行，由会话 {} 管理",
                        self.id
                    )),
                    session_id: Some(self.id.clone()),
                });
            }
            let now = Instant::now();
            if now >= deadline {
                let note = kill_failed_note(&st, &self.id);
                let (output, truncated) = drain(&mut st, max_output_chars, false);
                drop(st);
                return Some(ExecResult {
                    status: ExecStatus::Running,
                    exit_code: None,
                    output,
                    truncated,
                    error: note,
                    session_id: Some(self.id.clone()),
                });
            }
            let slice = deadline.saturating_duration_since(now).min(WAIT_SLICE);
            let (guard, _) = self
                .cv
                .wait_timeout(st, slice)
                .unwrap_or_else(|e| e.into_inner());
            st = guard;
        }
    }

    /// 不等待：取走当前已收集输出，返回运行中结果（写入超时等场景）。
    fn collect_partial(&self, max_output_chars: usize, id: String) -> ExecResult {
        let mut st = self.lock();
        let note = kill_failed_note(&st, &id);
        let (output, truncated) = drain(&mut st, max_output_chars, false);
        drop(st);
        ExecResult {
            status: ExecStatus::Running,
            exit_code: None,
            output,
            truncated,
            error: note,
            session_id: Some(id),
        }
    }

    /// 不等待：仅取走当前已收集输出（调用方自组状态——如写入路径的取消）。
    fn drain_now(&self, max_output_chars: usize) -> (String, bool) {
        let mut st = self.lock();
        drain(&mut st, max_output_chars, false)
    }

    /// 组装终态结果并清空缓存（最终输出只含最后待取部分，不重新汇总）。
    fn assemble_terminal(&self, max_output_chars: usize) -> ExecResult {
        let mut st = self.lock();
        let (output, truncated) = drain(&mut st, max_output_chars, true);
        let (status, exit_code, error) = match st.phase {
            Phase::Exited { code } => (ExecStatus::Completed, code, None),
            Phase::TimedOut => (
                ExecStatus::Timeout,
                None,
                Some("总运行时限到期，已终止并回收进程组".into()),
            ),
            Phase::Terminated => (
                ExecStatus::Terminated,
                None,
                Some("已明确终止并回收进程组".into()),
            ),
            Phase::Running { .. } => {
                // collect_done 仅在终态后置位；防御性兜底，不应到达。
                (ExecStatus::Error, None, Some("会话状态异常".into()))
            }
        };
        drop(st);
        ExecResult {
            status,
            exit_code,
            output,
            truncated,
            error,
            session_id: None,
        }
    }
}

/// PTY 输入写入端（独立封装：写入在辅助线程执行，可取消等待）。
struct InputWriter(Arc<File>);

impl InputWriter {
    /// 分块提交输入：块间检查中止标志——取消当前轮后停止提交尚未发送的
    /// 输入（016 §3.2“停止后续写入”）；单块内的阻塞写入不可移植中断
    ///（进程终止后写入返回 EIO 自然结束，线程数量有界）。
    fn write_chunked(
        &self,
        bytes: &[u8],
        abort: &AtomicBool,
        session: &Session,
    ) -> std::io::Result<()> {
        for chunk in bytes.chunks(INPUT_CHUNK) {
            if abort.load(Ordering::SeqCst) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "输入已取消，停止提交后续输入",
                ));
            }
            let mut f = &*self.0;
            std::io::Write::write_all(&mut f, chunk)?;
            session.note_input_written(chunk.len());
        }
        Ok(())
    }
}

/// 取消输入写入的结果构造：取走已收集输出，如实说明“取消前已写入的输入
/// 无法撤回”（已成功写入的部分保留在进程中，不做撤回）。
fn input_cancelled_result(
    session: &Session,
    max_output_chars: usize,
    session_id: &str,
) -> ExecResult {
    let (output, truncated) = session.drain_now(max_output_chars);
    ExecResult {
        status: ExecStatus::Cancelled,
        exit_code: None,
        output,
        truncated,
        error: Some(
            "等待被用户取消；进程仍在运行，取消前已写入的输入无法撤回".into(),
        ),
        session_id: Some(session_id.to_string()),
    }
}

/// 运行中会话附带的终止失败说明（016 §3.3：错误不表示进程退出、不删会话）。
fn kill_failed_note(st: &SessionState, id: &str) -> Option<String> {
    st.kill_failed.map(|(req, errno)| match req {
        KillReq::Deadline => format!(
            "总运行时限已到期，但终止进程组失败（errno={errno}），进程仍在运行，由会话 {id} 管理"
        ),
        KillReq::Terminate => format!(
            "终止进程组失败（errno={errno}），进程仍在运行，由会话 {id} 管理"
        ),
    })
}

/// 取走待取缓存：`final_drain` 表示终态收尾（尾部不完整 UTF-8 序列按 U+FFFD
/// 收尾）。先按码点上限截断（被省略部分不承诺补发），取走后缓存不再保留旧输出。
fn drain(st: &mut SessionState, max_output_chars: usize, final_drain: bool) -> (String, bool) {
    let (text, cache_truncated) = st.pending.take(final_drain);
    let (capped, over_cap) = cap_output(text, max_output_chars);
    (capped, over_cap || cache_truncated)
}

// ---- 待取缓存：单份 1 MiB，超限保留头尾并标记截断 ----

/// 单份待取输出缓存：增量 UTF-8 解码（跨块拼接、非法字节按 U+FFFD 替换），
/// 保留开头与结尾各半（字节计），中段省略并在取出时以标记注明省略字节数
///（按解码后文本字节计）。跨 take 边界保留尾部不完整序列，待下一块补全。
struct PendingBuffer {
    decoder_pending: Vec<u8>,
    head: String,
    tail: String,
    omitted: u64,
}

impl PendingBuffer {
    fn new() -> Self {
        Self {
            decoder_pending: Vec::new(),
            head: String::new(),
            tail: String::new(),
            omitted: 0,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let mut buf = std::mem::take(&mut self.decoder_pending);
        buf.extend_from_slice(chunk);
        let mut rest: &[u8] = &buf;
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    self.append_decoded(s);
                    rest = &[];
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        self.append_decoded(std::str::from_utf8(&rest[..valid]).unwrap());
                    }
                    match e.error_len() {
                        // 尾部不完整序列：暂存等待下一块（终态收尾时替换）
                        None => {
                            self.decoder_pending.extend_from_slice(&rest[valid..]);
                            rest = &[];
                        }
                        // 非法子序列：按 from_utf8_lossy 语义替换为一个 U+FFFD
                        Some(len) => {
                            self.append_decoded("\u{FFFD}");
                            rest = &rest[valid + len..];
                        }
                    }
                }
            }
        }
    }

    /// 解码后的文本进入头/尾缓存：头保留开头一半额度，尾保留最新字节；
    /// 总量超限后从尾部缓存前端省略（即中段），省略字节数累计。
    fn append_decoded(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let head_cap = PENDING_CAP / 2;
        if self.head.len() < head_cap {
            let remaining = head_cap - self.head.len();
            let cut = floor_char_boundary(s, remaining);
            self.head.push_str(&s[..cut]);
            let rest = &s[cut..];
            if !rest.is_empty() {
                self.tail.push_str(rest);
            }
        } else {
            self.tail.push_str(s);
        }
        let over = (self.head.len() + self.tail.len()) as i64 - PENDING_CAP as i64;
        if over > 0 {
            let evict = ceil_char_boundary(&self.tail, over as usize);
            self.tail.drain(..evict);
            self.omitted += evict as u64;
        }
    }

    /// 取走全部待取输出并清空（`decoder_pending` 跨取边界保留以正确拼接）；
    /// `final_drain` 时尾部不完整序列替换为一个 U+FFFD。返回文本与是否截断。
    fn take(&mut self, final_drain: bool) -> (String, bool) {
        if final_drain && !self.decoder_pending.is_empty() {
            self.decoder_pending.clear();
            self.append_decoded("\u{FFFD}");
        }
        let mut out = std::mem::take(&mut self.head);
        let truncated = self.omitted > 0;
        if truncated {
            out.push_str(&format!("\n…[已省略 {} 字节输出]…\n", self.omitted));
        }
        out.push_str(&self.tail);
        self.tail.clear();
        self.omitted = 0;
        (out, truncated)
    }
}

/// 不超过 `limit` 的最大字符边界（s 为合法 UTF-8）。
fn floor_char_boundary(s: &str, limit: usize) -> usize {
    if limit >= s.len() {
        return s.len();
    }
    let mut i = limit;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 不小于 `limit` 的最小字符边界（用于从头省略整字符）。
fn ceil_char_boundary(s: &str, limit: usize) -> usize {
    if limit >= s.len() {
        return s.len();
    }
    let mut i = limit;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

// ---- 启动与后台线程 ----

/// 一次成功启动的产物。
struct Spawned {
    child: ChildHandle,
    /// PTY master（输入通道）；管道模式为 None。
    master: Option<Arc<File>>,
    /// ConPTY 输出通道（038）：Windows PTY 的输入/输出是两个独立句柄，不能
    /// 像 unix master 那样一份句柄既写又读——读取线程经本字段接入。
    pty_reader: Option<Arc<File>>,
    /// 038 D2：沙箱 ns 标识（unix 沙箱 spawn 时捕获）；None＝无沙箱。
    ns_inode: Option<String>,
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
}

/// 033：构造 exec_command 的启动命令——普通模式 `/bin/sh -c <cmd>`（既有行为，
/// 环境按现状继承）；沙箱模式（`TCHAT_SANDBOX` 非空）`<bwrap> <沙箱参数> --
/// /bin/sh -c <cmd>`：Command env_clear＋白名单环境（PATH/HOME/TERM，主进程
/// 凭据 env 不进工具——027 R1 环境面防线在此保留）；沙箱不可用时返回 Err，
/// 由调用侧显式报错（禁止静默回退到无沙箱执行）。策略锚定主进程 cwd（G1 修
/// 复），workdir 仅作沙箱内 --chdir，不参与策略。
/// 031：`policy` 携带时改走 [`crate::sandbox::permission_launch_argv`]（受限
/// 读树＋deny 优先＋网络开关＋seccomp 的动态策略入口）；seccomp fd 守卫须
/// 存活到 spawn 返回——本函数内联完成 spawn 前‥后的全部时序。
/// 启动参数中间形态（038 ConPTY）：std Command 无读取接口，Windows PTY 路径
/// 需要原始 program/argv/env 交给 CreateProcessW——两个消费方（std Command
/// 构造与 ConPTY 启动）共用同一决策，行为零分歧。
struct ToolArgv {
    program: String,
    args: Vec<String>,
    /// Some＝白名单环境（env_clear＋仅此清单）；None＝继承进程环境（普通
    /// 模式既有行为）。
    env: Option<Vec<(String, String)>>,
    /// 沙箱包装启动（bwrap 为 program）：unix 侧继承 fd 收口行为随分支保留。
    sandbox_wrapped: bool,
    #[cfg(unix)]
    seccomp: Option<crate::sandbox::SeccompFd>,
}

fn tool_argv(
    cmd: &str,
    workdir: &std::path::Path,
    policy: Option<&crate::sandbox::PolicySnapshot>,
) -> std::io::Result<ToolArgv> {
    let _ = workdir;
    // 038 方案 A：沙箱载荷自停前缀（unix；Windows 无 bwrap 路径，载荷原样）。
    #[cfg(unix)]
    let cmd: std::borrow::Cow<'_, str> = if policy.is_some() {
        std::borrow::Cow::Owned(format!("{}{cmd}", crate::sandbox::sandbox_gate_prefix()))
    } else {
        std::borrow::Cow::Borrowed(cmd)
    };
    #[cfg(not(unix))]
    let cmd: std::borrow::Cow<'_, str> = std::borrow::Cow::Borrowed(cmd);
    if let Some(snapshot) = policy {
        let launch = crate::sandbox::permission_launch_argv(
            vec![SHELL.to_string(), SHELL_EXEC_FLAG.to_string(), cmd.to_string()],
            snapshot,
            workdir,
        )?;
        Ok(ToolArgv {
            program: launch.program,
            args: launch.args,
            env: Some(launch.env),
            sandbox_wrapped: true,
            #[cfg(unix)]
            seccomp: launch.seccomp,
        })
    } else {
        match crate::sandbox::tool_launch_argv(vec![
            SHELL.to_string(),
            SHELL_EXEC_FLAG.to_string(),
            cmd.to_string(),
        ])? {
            Some((program, args, child_env)) => Ok(ToolArgv {
                program,
                args,
                env: Some(child_env),
                sandbox_wrapped: true,
                #[cfg(unix)]
                seccomp: None,
            }),
            None => Ok(ToolArgv {
                program: SHELL.to_string(),
                args: vec![SHELL_EXEC_FLAG.to_string(), cmd.to_string()],
                env: None,
                sandbox_wrapped: false,
                #[cfg(unix)]
                seccomp: None,
            }),
        }
    }
}

fn build_tool_command(
    cmd: &str,
    workdir: &std::path::Path,
    policy: Option<&crate::sandbox::PolicySnapshot>,
) -> std::io::Result<(Command, Option<crate::sandbox::SeccompFd>)> {
    let t = tool_argv(cmd, workdir, policy)?;
    let mut c = Command::new(&t.program);
    c.args(&t.args);
    if t.sandbox_wrapped {
        // 031：seccomp fd 守卫须存活到 spawn——经返回值交给调用方。
        // Some＝close_range 版收口（除 seccomp fd 外全部关闭）；None＝033-B2
        // 的 CLOEXEC 标记版（launcher 遗留 fd execve 时关闭；仅沙箱分支）。
        #[cfg(unix)]
        match t.seccomp.as_ref() {
            Some(guard) => {
                let fd = guard.raw();
                unsafe {
                    c.pre_exec(move || {
                        crate::sandbox::close_inherited_fds_except_cloexec(fd)
                    });
                }
            }
            None => unsafe {
                c.pre_exec(crate::sandbox::close_inherited_fds_mark_cloexec);
            },
        }
        #[cfg(windows)]
        let _ = &c;
    }
    match t.env {
        Some(env) => {
            c.env_clear();
            c.envs(env);
        }
        None => {}
    }
    Ok((c, {
        #[cfg(unix)]
        {
            t.seccomp
        }
        #[cfg(windows)]
        {
            None
        }
    }))
}

/// 管道模式：stdin 关闭，stdout/stderr 管道；独立进程组。
/// 031：spawn 后 best-effort 验证 bwrap 子进程的 PID 命名空间确已隔离
///（仍存活且与主进程同 ns ⇒ 立即整组终止并报错——隔离创建失败的执行前
/// 拒绝链闭合；子进程已退出时以 bwrap 自身 fail-hard 语义为准）。
fn spawn_piped(
    cmd: &str,
    workdir: &std::path::Path,
    policy: Option<&crate::sandbox::PolicySnapshot>,
) -> std::io::Result<Spawned> {
    let (mut c, _seccomp_guard) = build_tool_command(cmd, workdir, policy)?;
    c.current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // 组长即本子进程：终止时覆盖其全部后代（std process_group 为 unix-only API；
    // Windows 树终止经作业对象，见 win 模块）。
    #[cfg(unix)]
    c.process_group(0);
    let mut child = c.spawn()?;
    // 038 方案 A：沙箱确认闸在 start_with_owner 同步路径执行（cancel 引用
    // 仅在调用线程可用——spawner 线程无法持有 &AtomicBool）；本函数只负责
    // 起进程（载荷自停前缀使其停在首条用户命令之前）。
    #[cfg(unix)]
    let ns_inode = None;
    #[cfg(not(unix))]
    let ns_inode = None;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    Ok(Spawned {
        child: ChildHandle::Std(child),
        master: None,
        pty_reader: None,
        ns_inode,
        stdout,
        stderr,
    })
}

/// PTY 模式：分配真实伪终端作为控制终端；会话组长自身成组
///（pre_exec setsid，不用 process_group——setpgid 后 setsid 会失败）。
/// 031：同管道模式的 spawn 后 PID 命名空间验证（TTY 组终止路径）。
/// Windows 版（038 ConPTY，008 A6 的 Windows 等价实现）：CreatePseudoConsole
/// ＋PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE 属性清单＋CreateProcessW。会话语义
/// 对齐 unix 版：ConPTY 输出端＝读取线程通道；输入端＝master（Arc<File> 输入
/// 通道）；整树终止经作业对象（spawn 后挂接）；伪终端拆除顺序＝进程退出→
/// ClosePseudoConsole（ChildHandle::Raw::wait，monitor 唯一 wait 点）。
/// 沙箱请求（TCHAT_SANDBOX）仍经 tool_argv fail-closed——bwrap 缺失＝Err。
#[cfg(windows)]
fn spawn_pty(
    cmd: &str,
    workdir: &std::path::Path,
    policy: Option<&crate::sandbox::PolicySnapshot>,
) -> std::io::Result<Spawned> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Console::{
        ClosePseudoConsole, CreatePseudoConsole, COORD, HPCON,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW,
        TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
    };

    // —— 管道对：输入（ConPTY 读端可继承）与输出（ConPTY 写端可继承）——
    fn inherit_pipe() -> std::io::Result<(HANDLE, HANDLE)> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let (mut r, mut w): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
        if unsafe { CreatePipe(&mut r, &mut w, &sa as *const _, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((r, w))
    }
    let (pty_in_read, our_in_write) = inherit_pipe()?;
    let (our_out_read, pty_out_write) = inherit_pipe()?;

    // —— 伪终端（80×24，与 unix open_pty 的初始窗口一致）——
    let mut hpc: HPCON = 0;
    let create_pc =
        unsafe { CreatePseudoConsole(COORD { X: 80, Y: 24 }, pty_in_read, pty_out_write, 0, &mut hpc) };
    if create_pc != 0 {
        unsafe {
            CloseHandle(pty_in_read);
            CloseHandle(pty_out_write);
            CloseHandle(our_in_write);
            CloseHandle(our_out_read);
        }
        return Err(std::io::Error::from_raw_os_error(create_pc));
    }
    // ConPTY 已持有两端句柄，父进程侧立即关闭（文档顺序）。
    unsafe {
        CloseHandle(pty_in_read);
        CloseHandle(pty_out_write);
    }

    // —— 属性清单：挂 PSEUDOCONSOLE（唯一属性）——
    let mut attr_size: usize = 0;
    unsafe {
        // 预查询：传 null 必失败但填充所需字节数（文档约定）。
        let _ = InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
    }
    let mut attr_buf = vec![0u8; attr_size];
    if unsafe {
        InitializeProcThreadAttributeList(attr_buf.as_mut_ptr() as *mut _, 1, 0, &mut attr_size)
    } == 0
    {
        unsafe { ClosePseudoConsole(hpc) };
        return Err(std::io::Error::last_os_error());
    }
    if unsafe {
        UpdateProcThreadAttribute(
            attr_buf.as_mut_ptr() as *mut _,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            &mut hpc as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<HPCON>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    } == 0
    {
        unsafe { ClosePseudoConsole(hpc) };
        return Err(std::io::Error::last_os_error());
    }

    // —— 命令行：tool_argv（沙箱 fail-closed 在此生效）＋Windows 引号拼接 ——
    let t = tool_argv(cmd, workdir, policy)?;
    let mut cmdline = quote_windows_arg(&t.program);
    for a in &t.args {
        cmdline.push(' ');
        cmdline.push_str(&quote_windows_arg(a));
    }
    let mut cmdline_w: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
    let workdir_w: Vec<u16> =
        workdir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let env_block: Option<Vec<u16>> = t.env.as_ref().map(|env| {
        let mut b: Vec<u16> = Vec::new();
        for (k, v) in env {
            b.extend(format!("{k}={v}").encode_utf16());
            b.push(0);
        }
        b.push(0);
        b
    });

    let mut siex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    siex.StartupInfo = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    siex.lpAttributeList = attr_buf.as_mut_ptr() as *mut _;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmdline_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0, // bInheritHandles=FALSE：可继承性已在 ConPTY 端句柄满足
            EXTENDED_STARTUPINFO_PRESENT,
            env_block
                .as_ref()
                .map(|b| b.as_ptr() as *mut std::ffi::c_void)
                .unwrap_or(std::ptr::null_mut()),
            workdir_w.as_ptr(),
            &siex.StartupInfo,
            &mut pi,
        )
    } == 0
    {
        let e = std::io::Error::last_os_error();
        unsafe {
            ClosePseudoConsole(hpc);
            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);
        }
        return Err(e);
    }
    unsafe { CloseHandle(pi.hThread) };
    let pid = pi.dwProcessId;
    let process: HANDLE = pi.hProcess;
    let _ = HANDLE_FLAG_INHERIT; // 继承性经 SECURITY_ATTRIBUTES 传递；显式消除未用告警

    // 作业对象挂接（016 整树终止）：与管道模式同机制；挂接失败＝失去树终止
    // 能力——终止已启动的进程并显式报错，不静默降级。
    if let Err(e) = win::assign_raw_process(pid, process) {
        unsafe {
            TerminateProcess(process, 1);
            let _ = WaitForSingleObject(process, 5_000);
            ClosePseudoConsole(hpc);
            CloseHandle(process);
        }
        return Err(e);
    }

    let master = Arc::new(unsafe { File::from_raw_handle(our_in_write) });
    let pty_reader = Arc::new(unsafe { File::from_raw_handle(our_out_read) });
    Ok(Spawned {
        child: ChildHandle::Raw { process, pid, hpc },
        master: Some(master),
        pty_reader: Some(pty_reader),
        ns_inode: None,
        stdout: None,
        stderr: None,
    })
}

/// Windows 命令行引号拼接（CreateProcessW lpCommandLine 约定）：含空白或
/// 引号的参数加引号，反斜杠仅在与收尾引号相邻时成对转义。
#[cfg(windows)]
fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"')
    {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
                backslashes = 0;
                out.push('"');
            }
            _ => {
                out.extend(std::iter::repeat('\\').take(backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat('\\').take(backslashes * 2));
    out.push('"');
    out
}

/// 038 D2：按 ns 标识补杀命名空间残留成员（unix）。组长（bwrap）死后，
/// ns 内成员在宿主侧以孤儿存活且不属组长进程组——killpg 覆盖不到；成员的
/// NSpid 链也不含组长 pid（链只映射该进程自己跨 ns 的 pid）。以 spawn 时
/// 捕获的 ns inode（/proc/<leader>/ns/pid 的 readlink）精确圈定同 ns 成员：
/// 扫描 /proc，ns/pid readlink 相同者 SIGKILL。有界：一轮＋150ms 重试
/// （内核 ns 拆解异步）；分离线程调用，不阻塞终态收口。仅同 uid 进程
/// 可杀，无越权面。
#[cfg(unix)]
fn sweep_namespace_members(ns_inode: String) {
    fn kill_round(target: &str) -> usize {
        let mut killed = 0usize;
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return 0;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.parse::<libc::pid_t>().is_err() {
                continue;
            }
            let Ok(link) = std::fs::read_link(format!("/proc/{}/ns/pid", name)) else {
                continue;
            };
            if link.to_string_lossy() == target
                && unsafe { libc::kill(name.parse::<libc::pid_t>().unwrap(), libc::SIGKILL) } == 0
            {
                killed += 1;
            }
        }
        killed
    }
    let _first = kill_round(&ns_inode);
    std::thread::sleep(Duration::from_millis(150));
    let _retry = kill_round(&ns_inode);
}

#[cfg(unix)]
fn spawn_pty(
    cmd: &str,
    workdir: &std::path::Path,
    policy: Option<&crate::sandbox::PolicySnapshot>,
) -> std::io::Result<Spawned> {
    let (master, slave) = open_pty()?;
    // 三路指向同一从设备；pre_exec 内 setsid＋挂控制终端。
    let in_f = slave.try_clone()?;
    let out_f = slave.try_clone()?;
    let err_f = slave;
    let (mut c, _seccomp_guard) = build_tool_command(cmd, workdir, policy)?;
    c.current_dir(workdir)
        .stdin(Stdio::from(in_f))
        .stdout(Stdio::from(out_f))
        .stderr(Stdio::from(err_f));
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // fd 0 已是从设备（std 先 dup2 再跑 pre_exec）：挂为控制终端，
            // 终端产生的信号才能送达前台进程组。
            // 038：apple ioctl request 形参为 c_ulong，显式提升（Linux 无操作）。
            if libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = c.spawn()?;
    drop(c); // 关闭父进程持有的从设备副本（Command 内 Stdio 的 fd）
    // 038 方案 A：沙箱确认闸在 start_with_owner 同步路径执行（同 spawn_piped）。
    #[cfg(unix)]
    let ns_inode = None;
    let master = Arc::new(master);
    Ok(Spawned {
        child: ChildHandle::Std(child),
        master: Some(master),
        pty_reader: None,
        ns_inode,
        stdout: None,
        stderr: None,
    })
}

/// 038：macOS ptsname 静态缓冲的串行化锁（见 open_pty 内注释）。
#[cfg(target_os = "macos")]
static PTY_NAME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 打开一对伪终端（master 带 O_CLOEXEC，从设备以 O_CLOEXEC 打开）。
#[cfg(unix)]
fn open_pty() -> std::io::Result<(File, File)> {
    unsafe {
        let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut ok = libc::grantpt(master_fd) == 0 && libc::unlockpt(master_fd) == 0;
        ok = ok
            && libc::fcntl(master_fd, libc::F_SETFD, libc::FD_CLOEXEC) == 0;
        if !ok {
            let e = std::io::Error::last_os_error();
            libc::close(master_fd);
            return Err(e);
        }
        // 038：libc crate 在 apple 目标不提供 ptsname_r（ptsname 返回进程级
        // 静态缓冲、非线程安全）——分支内经 PTY_NAME_LOCK 串行化并立即拷贝；
        // Linux 的 ptsname_r 语义不变。
        #[cfg(not(target_os = "macos"))]
        let c_path = {
            let mut name = [0u8; 256];
            if libc::ptsname_r(
                master_fd,
                name.as_mut_ptr().cast(),
                name.len(),
            ) != 0
            {
                let e = std::io::Error::last_os_error();
                libc::close(master_fd);
                return Err(e);
            }
            let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            std::ffi::CString::new(&name[..len])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?
        };
        #[cfg(target_os = "macos")]
        let c_path = {
            let _guard = PTY_NAME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let ptr = libc::ptsname(master_fd);
            if ptr.is_null() {
                let e = std::io::Error::last_os_error();
                libc::close(master_fd);
                return Err(e);
            }
            std::ffi::CStr::from_ptr(ptr).to_owned()
        };
        let slave_fd = libc::open(
            c_path.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        );
        if slave_fd < 0 {
            let e = std::io::Error::last_os_error();
            libc::close(master_fd);
            return Err(e);
        }
        // 初始窗口尺寸（80×24）；不提供缩放能力。
        let ws = libc::winsize { ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 };
        libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
        Ok((File::from_raw_fd(master_fd), File::from_raw_fd(slave_fd)))
    }
}

/// 子进程句柄抽象（038 ConPTY）：std Child（管道模式、unix PTY）或 ConPTY
/// 原生进程句柄（Windows PTY——CreateProcessW 的产物不构回 std Child）。
/// 统一轮询/回收接口供监控线程使用，016 生命周期契约平台等价。
/// 安全：HANDLE 为裸指针但句柄级操作（Wait/GetExitCode/Close/PseudoConsole
/// 拆除）可跨线程调用；Raw 的句柄所有权随枚举整体移交监控线程（唯一持有
/// 者），无别名访问。
enum ChildHandle {
    Std(Child),
    /// ConPTY 子进程：process 句柄负责 wait/终止回收；hpc 负责 ClosePseudo
    /// Console（wait 到终态后关闭——先进程退出再拆伪终端的文档顺序）。
    #[cfg(windows)]
    Raw { process: HANDLE, pid: u32, hpc: HPCON },
}

#[cfg(windows)]
unsafe impl Send for ChildHandle {}


impl ChildHandle {
    fn id(&self) -> u32 {
        match self {
            ChildHandle::Std(c) => c.id(),
            #[cfg(windows)]
            ChildHandle::Raw { pid, .. } => *pid,
        }
    }

    /// 作业挂接用的（pid, 原生句柄）——Std 经 AsRawHandle；Raw 自持句柄。
    #[cfg(windows)]
    fn raw_process(&self) -> (u32, HANDLE) {
        match self {
            ChildHandle::Std(c) => {
                use std::os::windows::io::AsRawHandle;
                (c.id(), unsafe { c.as_raw_handle() as HANDLE })
            }
            ChildHandle::Raw { process, pid, .. } => (*pid, *process),
        }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            ChildHandle::Std(c) => c.try_wait(),
            #[cfg(windows)]
            ChildHandle::Raw { process, .. } => unsafe {
                match WaitForSingleObject(*process, 0) {
                    WAIT_OBJECT_0 => {
                        let mut code: u32 = 0;
                        if GetExitCodeProcess(*process, &mut code) == 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(Some(std::process::ExitStatus::from_raw(code)))
                    }
                    WAIT_TIMEOUT => Ok(None),
                    _ => Err(std::io::Error::last_os_error()),
                }
            },
        }
    }

    /// 终态回收：Std＝阻塞 wait；Raw＝阻塞等待＋读退出码＋拆伪终端＋关句柄
    ///（monitor_thread 的唯一调用点；Drop 兜底不再重复清理）。
    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        match self {
            ChildHandle::Std(c) => c.wait(),
            #[cfg(windows)]
            ChildHandle::Raw { process, hpc, .. } => unsafe {
                loop {
                    match WaitForSingleObject(*process, 5_000) {
                        WAIT_OBJECT_0 => break,
                        WAIT_FAILED => return Err(std::io::Error::last_os_error()),
                        _ => {} // WAIT_TIMEOUT：继续等（016 宽限语义内的有界阻塞）
                    }
                }
                let mut code: u32 = 0;
                if GetExitCodeProcess(*process, &mut code) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                ClosePseudoConsole(*hpc);
                CloseHandle(*process);
                Ok(std::process::ExitStatus::from_raw(code))
            },
        }
    }
}

/// 监控线程：拥有子进程句柄——轮询退出、执行终止请求／时限到期；退出后等
/// 读取线程收尾（有界），置 collect_done。终态回收（wait）只发生在这里。
fn monitor_thread(session: Arc<Session>, mut child: ChildHandle, deadline: Option<Instant>) {
    let pgid = child.id() as GroupId;
    let terminal = loop {
        // 收口缝（仅 cfg(test)）：门控停靠点——见 test_gate_wait（不持锁）。
        #[cfg(test)]
        test_gate_wait(&session);
        let mut st = session.lock();
        // 终止请求（terminate=true／管理器关闭），或总运行时限到期：
        // 到期独立生效，不依赖查询在场。
        let mut reason = st.kill;
        if reason.is_none() {
            if let Some(dl) = deadline {
                if monotonic_now() >= dl {
                    reason = Some(KillReq::Deadline);
                    st.kill = reason;
                }
            }
        }
        if let Some(reason) = reason {
            // killpg 本身快速，可持锁。
            match kill_group_checked(pgid) {
                KillOutcome::Ok => {
                    drop(st);
                    let _ = child.wait(); // 回收组长
                    // 038 D2（CI 实证）：沙箱（bwrap 命名空间）成员的宿主侧
                    // 进程组与组长不同——killpg 只覆盖组长；ns 内残留成员在
                    // 组长死后仍以孤儿形态存活（marker 被 init 收养）。按
                    // NSpid 链补杀——分离线程执行（两轮扫描＋间隔约数百 ms，
                    // 不得阻塞终态收口时序）。
                    #[cfg(unix)]
                    {
                        let ns = session.ns_inode.clone();
                        std::thread::spawn(move || {
                            if let Some(ns_inode) = ns {
                                sweep_namespace_members(ns_inode);
                            }
                        });
                    }
                    break match reason {
                        KillReq::Deadline => Phase::TimedOut,
                        KillReq::Terminate => Phase::Terminated,
                    };
                }
                KillOutcome::Gone => {
                    // 组已不存在＝我们发信号前进程已死：按自然退出如实归属，
                    // 不宣称发生过“终止并回收”。
                    drop(st);
                    #[cfg(unix)]
                    {
                        let ns = session.ns_inode.clone();
                        std::thread::spawn(move || {
                            if let Some(ns_inode) = ns {
                                sweep_namespace_members(ns_inode);
                            }
                        });
                    }
                    let code = child.wait().ok().and_then(|s| s.code());
                    break Phase::Exited { code };
                }
                KillOutcome::Failed(errno) => {
                    // 终止失败：未确认清理成功——不得伪装已终止/超时回收；
                    // 如实记录，保持运行（不漏管），保留请求在后续轮询重试。
                    st.kill_failed = Some((reason, errno));
                }
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                #[cfg(unix)]
                {
                    let ns = session.ns_inode.clone();
                    std::thread::spawn(move || {
                        if let Some(ns_inode) = ns {
                            sweep_namespace_members(ns_inode);
                        }
                    });
                }
                break Phase::Exited { code: status.code() };
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.wait();
                break Phase::Exited { code: None };
            }
        }
        let (guard, _) = session
            .cv
            .wait_timeout(st, MONITOR_POLL)
            .unwrap_or_else(|e| e.into_inner());
        drop(guard);
    };
    {
        let mut st = session.lock();
        st.phase = terminal;
        st.kill_failed = None;
    }
    session.cv.notify_all();

    // 子进程已死：等读取线程排干输出通道（EOF 很快到达；后代继承通道时
    // 至多等宽限，不无限等待）。
    let grace = Instant::now() + COLLECT_GRACE;
    let mut st = session.lock();
    while st.reader_eof < st.readers && Instant::now() < grace {
        let (guard, _) = session
            .cv
            .wait_timeout(st, WAIT_SLICE)
            .unwrap_or_else(|e| e.into_inner());
        st = guard;
    }
    st.readers_may_stop = true;
    st.collect_done = true;
    drop(st);
    session.cv.notify_all();
}

/// 读取线程：poll＋读取一路输出通道直到 EOF；持续推入待取缓存并唤醒等待方。
/// `readers_may_stop` 置位后退出（宽限已过，通道由后代持有时不无限等待）。
/// Windows 读取线程：阻塞 Read 直到 EOF（树终止后管道关闭必然 EOF；
/// 宽限语义由 collect_done 的自然退出覆盖，无需 poll 中断）。
#[cfg(windows)]
fn spawn_reader<R: Read + Send + 'static>(session: Arc<Session>, mut pipe: R) {
    std::thread::spawn(move || {
        let mut buf = [0u8; READ_BUF];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let stop = {
                        let mut st = session.lock();
                        st.pending.push(&buf[..n]);
                        let stop = st.readers_may_stop;
                        drop(st);
                        session.cv.notify_all();
                        stop
                    };
                    if stop {
                        break;
                    }
                }
            }
        }
    });
}

#[cfg(unix)]
fn spawn_reader<R: Read + AsRawFd + Send + 'static>(session: Arc<Session>, mut pipe: R) {
    std::thread::spawn(move || {
        let fd = pipe.as_raw_fd();
        let mut buf = [0u8; READ_BUF];
        loop {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let r = unsafe { libc::poll(&mut pfd, 1, READ_POLL) };
            if r > 0 && pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut st = session.lock();
                        st.pending.push(&buf[..n]);
                        drop(st);
                        session.cv.notify_all();
                    }
                }
            }
            if session.lock().readers_may_stop {
                break;
            }
        }
        let mut st = session.lock();
        st.reader_eof += 1;
        drop(st);
        session.cv.notify_all();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 038 方案 A：沙箱启动确认闸（用户确认契约，确定性单测） ----------
    //
    // 集成层（tests/exec_sandbox.rs gate03）验证放行路径；本组单测直接操纵
    // 确认闸输入，确定性复现「取消/超时/LeaderGone/清理边界」——集成层无法
    // 从外部构造这些窗口（本容器 bwrap setup <1ms，时间注入不可控）。

    /// 确认期取消（cancel 预置位）：不放行——真实 bwrap 载荷（touch 标记文件）
    /// 未执行（文件不存在），整组已被终止，调用者 ns 的无关哨兵不波及。
    #[cfg(unix)]
    #[test]
    fn gate_confirm_cancel_does_not_execute() {
        if !crate::sandbox::probe_available() {
            return; // 无 bwrap 环境：确认闸随沙箱后端分支，用例照常通过
        }
        let _g = crate::TEST_ENV_LOCK.lock().unwrap();
        std::env::set_var("TCHAT_SANDBOX", "1");
        let dir = std::env::temp_dir().join(format!(
            "tchat-gate-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("gate-mark");
        // 无关哨兵：与沙箱会话同在调用者 ns——清理不得波及。
        let mut sentinel = std::process::Command::new("sleep")
            .arg("20")
            .spawn()
            .expect("哨兵启动");
        let launch = crate::sandbox::tool_launch_argv(vec![
            SHELL.to_string(),
            SHELL_EXEC_FLAG.to_string(),
            format!(
                "{}touch {}; sleep 30",
                crate::sandbox::sandbox_gate_prefix(),
                mark.display()
            ),
        ])
        .unwrap()
        .expect("沙箱可用时必须包装");
        let mut c = Command::new(&launch.0);
        c.args(&launch.1)
            .env_clear()
            .envs(launch.2.iter().cloned())
            .process_group(0);
        let mut child = c.spawn().expect("bwrap 启动");
        let cancel = AtomicBool::new(true); // 预置位：确认第一循环即取消
        let r = crate::sandbox::confirm_sandbox_isolation_and_release(child.id(), Some(&cancel));
        assert!(r.is_err(), "取消必须不放行：{r:?}");
        assert!(
            r.unwrap_err().to_string().contains("取消"),
            "错误须说明取消"
        );
        // 用户命令未执行（载荷停在 STOP，随后被整组终止）。
        assert!(!mark.exists(), "确认取消时用户命令不得执行");
        // 016 有界清理：监督进程已被回收——confirm 的 kill_group 内联
        // waitpid 已收尸（child.wait 返回 ECHILD 即为已回收的证据）。
        match child.wait() {
            Ok(status) => assert!(!status.success(), "被终止的启动无成功退出码：{status:?}"),
            Err(e) if e.raw_os_error() == Some(libc::ECHILD) => {}
            Err(e) => panic!("意外的 wait 错误：{e}"),
        }
        // 清理不波及调用者 ns：无关哨兵仍存活。
        assert!(
            sentinel.try_wait().unwrap().is_none(),
            "调用者 ns 无关进程不得被清理波及"
        );
        let _ = sentinel.kill();
        let _ = sentinel.wait();
        std::env::remove_var("TCHAT_SANDBOX");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 确认超时：以无载荷的普通进程（sleep，children 空且同 ns）为监督者，
    /// 载荷定位永不可能→注入 1ms 窗口→确定性超时：不放行＋整组终止。
    #[cfg(unix)]
    #[test]
    fn gate_confirm_timeout_does_not_release() {
        std::env::set_var("TCHAT_SANDBOX_CONFIRM_TIMEOUT_MS", "1");
        let mut leader = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .expect("sleep 启动");
        let r = crate::sandbox::confirm_sandbox_isolation_and_release(leader.id(), None);
        std::env::remove_var("TCHAT_SANDBOX_CONFIRM_TIMEOUT_MS");
        let err = r.expect_err("无载荷监督进程必须确认超时");
        assert!(err.to_string().contains("超时"), "{err}");
        // 016 有界清理：监督者已被终止回收（confirm 内联 waitpid 已收尸）。
        match leader.wait() {
            Ok(status) => assert!(!status.success()),
            Err(e) if e.raw_os_error() == Some(libc::ECHILD) => {}
            Err(e) => panic!("意外的 wait 错误：{e}"),
        }
    }

    /// LeaderGone：监督进程不存在（确认前已消失）→不报错、不放行、无清理
    /// 标识——016 自然退出语义由监控线程收尾。
    #[cfg(unix)]
    #[test]
    fn gate_confirm_leader_gone_is_quiet() {
        let r = crate::sandbox::confirm_sandbox_isolation_and_release(3_999_999_999, None);
        assert!(matches!(r, Ok(crate::sandbox::SandboxConfirm::LeaderGone)), "{r:?}");
    }

    /// 监督进程在真实载荷被定位后、放行前死亡：确认结果不得报告 Released。
    #[cfg(unix)]
    #[test]
    fn gate_confirm_supervisor_dies_before_release_does_not_report_released() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::process::{CommandExt, ExitStatusExt};

        if !crate::sandbox::probe_available() {
            return;
        }
        let _g = crate::TEST_ENV_LOCK.lock().unwrap();
        std::env::set_var("TCHAT_SANDBOX", "1");
        let launch = crate::sandbox::tool_launch_argv(vec![
            "/bin/sleep".to_string(),
            "30".to_string(),
        ])
        .unwrap()
        .expect("沙箱可用时必须包装");
        let mut command = Command::new(&launch.0);
        command
            .args(&launch.1)
            .env_clear()
            .envs(launch.2.iter().cloned())
            .process_group(0);
        let mut child = command.spawn().expect("启动 bwrap");
        let mut payload_pidfd = None;
        let mut payload_pid = None;
        let mut release_calls = 0;
        let mut abort_calls = 0;
        let result = crate::sandbox::confirm_sandbox_with_release(
            child.id(),
            None,
            |payload, _, _| {
                release_calls += 1;
                let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, payload, 0) };
                if raw < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                payload_pid = Some(payload);
                payload_pidfd = Some(unsafe { OwnedFd::from_raw_fd(raw as i32) });
                assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGKILL) }, 0);
                let deadline = Instant::now() + Duration::from_secs(2);
                let status = loop {
                    if let Some(status) = child.try_wait()? {
                        break status;
                    }
                    if Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "bwrap 外力终止后未在 2 秒内退出",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                };
                assert_eq!(status.signal(), Some(libc::SIGKILL));
                Ok(crate::sandbox::SandboxRelease::Pending)
            },
            |leader_pid| {
                abort_calls += 1;
                if unsafe { libc::kill(leader_pid as libc::pid_t, libc::SIGKILL) } != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
                Ok(())
            },
        )
        .expect("bwrap 消失应返回 LeaderGone");
        assert!(matches!(result, crate::sandbox::SandboxConfirm::LeaderGone));
        assert_eq!(release_calls, 1, "只允许在载荷已定位后触发一次屏障");
        assert_eq!(abort_calls, 0, "监督进程已退出时不伪造清理成功");
        let payload_fd = payload_pidfd.expect("屏障前已固定载荷身份");
        let mut pfd = libc::pollfd {
            fd: payload_fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert!(unsafe { libc::poll(&mut pfd, 1, 0) } >= 0);
        if pfd.revents & (libc::POLLIN | libc::POLLHUP) == 0 {
            eprintln!(
                "DIAGNOSTIC_CLEANUP_AFTER_PRODUCT_ASSERTION payload_pid={}",
                payload_pid.expect("payload PID 已记录")
            );
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    payload_fd.as_raw_fd(),
                    libc::SIGKILL,
                    0,
                    0,
                )
            };
            if rc < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                panic!("诊断清理孤儿 payload 失败：{}", std::io::Error::last_os_error());
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            assert!(unsafe { libc::poll(&mut pfd, 1, 0) } >= 0);
            if pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                break;
            }
            assert!(Instant::now() < deadline, "bwrap 退出后载荷仍未停止");
            std::thread::sleep(Duration::from_millis(2));
        }
        std::env::remove_var("TCHAT_SANDBOX");
    }

    fn sink_run(chunks: &[&[u8]]) -> PendingBuffer {
        let mut b = PendingBuffer::new();
        for c in chunks {
            b.push(c);
        }
        b
    }

    #[test]
    fn pending_keeps_valid_text_and_multibyte_across_chunks() {
        // “你” = E4 BD A0 拆三块；“🎉” = F0 9F 8E 89 拆两块
        let mut b = sink_run(&[&[0xE4], &[0xBD], &[0xA0], &[0xF0, 0x9F], &[0x8E, 0x89]]);
        assert_eq!(b.take(true).0, "你🎉");
    }

    #[test]
    fn pending_replaces_invalid_bytes() {
        let mut b = sink_run(&[b"a\xff\xc3("]);
        assert_eq!(b.take(true).0, "a\u{FFFD}\u{FFFD}(");
    }

    #[test]
    fn pending_incomplete_tail_replaced_only_at_final_drain() {
        let mut b = PendingBuffer::new();
        b.push(&[0xE4]); // “你”的首字节
        // 非终态取走：不替换、不吞字节——序列跨取边界保留
        let (text, _) = b.take(false);
        assert_eq!(text, "");
        b.push(&[0xBD, 0xA0, b'\n']); // 补全
        assert_eq!(b.take(true).0, "你\n");
    }

    #[test]
    fn pending_over_cap_keeps_head_and_tail_with_marker() {
        // 小上限不可注入——用足够长的 ASCII 验证结构：head=前 512KiB、
        // tail=最后字节、中段省略并计数
        let big: String = (0..300_000).map(|i| format!("{i:07}\n")).collect();
        assert!(big.len() > PENDING_CAP + 1024, "样本须超缓存上限");
        let mut b = PendingBuffer::new();
        b.push(big.as_bytes());
        let head_expect: String = (0..).map(|i| format!("{i:07}\n")).take(65_000).collect();
        let (out, truncated) = b.take(true);
        assert!(truncated, "超限必须标记截断");
        assert!(out.starts_with(&head_expect), "保留开头：{}…", &out[..64]);
        assert!(out.ends_with("299999\n"), "保留结尾");
        let marker_pos = out.find("已省略").expect("中段省略标记");
        assert!(marker_pos > 0 && marker_pos < out.len(), "标记位于头尾之间");
        // 取走后缓存清空：再次取走为空且不截断
        let (again, again_trunc) = b.take(true);
        assert_eq!(again, "");
        assert!(!again_trunc);
    }

    #[test]
    fn pending_eviction_counts_bytes_at_char_boundary() {
        // 混合多字节内容超限：省略计数按字符边界整字符省略，输出仍为合法 UTF-8。
        // “中文🎉x”每组 11 字节（中3＋文3＋🎉4＋x1），120000 组＝1,320,000 字节。
        let chunk: String = "中文🎉x".repeat(120_000);
        let total = chunk.len();
        assert_eq!(total, 1_320_000, "样本字节数自检");
        let mut b = PendingBuffer::new();
        b.push(chunk.as_bytes());
        let (out, truncated) = b.take(true);
        assert!(truncated);
        assert!(out.starts_with("中文🎉x中文🎉x"), "头部从原文起点开始");
        // 省略标记唯一，且把输出分为“头部原文＋标记＋尾部原文”三段
        let marker = "\n…[已省略 ";
        let marker_start = out.find(marker).expect("省略标记存在");
        assert_eq!(
            out.rfind(marker),
            Some(marker_start),
            "省略标记必须唯一：{out:?}"
        );
        let head = &out[..marker_start];
        let after = &out[marker_start + marker.len()..];
        let digits_end = after.find(" 字节输出]…\n").expect("标记字节数段");
        let omitted: u64 = after[..digits_end].parse().expect("省略计数为十进制数字");
        let tail = &after[digits_end + " 字节输出]…\n".len()..];
        // 头尾分别精确等于输入的对应前缀／后缀（非空、按字符边界切分）
        assert!(!head.is_empty() && !tail.is_empty(), "头尾均保留内容");
        assert!(
            chunk.starts_with(head),
            "头部必须是输入前缀（{} 字节）",
            head.len()
        );
        assert!(
            chunk.ends_with(tail),
            "尾部必须是输入后缀（{} 字节）",
            tail.len()
        );
        // 省略字节数＝输入总字节－保留头尾字节（按字节计，不是字符数）
        assert_eq!(
            omitted,
            (total - head.len() - tail.len()) as u64,
            "省略计数必须等于实际省略字节数"
        );
        // 合法 UTF-8 输入不得出现替换符；输出有界（头尾各半＋标记）
        assert!(!out.contains('\u{FFFD}'), "合法输入无意外替换符");
        assert!(out.len() <= PENDING_CAP + 128, "输出有界：{}", out.len());
        // 取走后清空：再次取走为空且不截断
        let (again, again_trunc) = b.take(true);
        assert_eq!(again, "");
        assert!(!again_trunc);
    }

    #[test]
    fn char_boundary_helpers() {
        assert_eq!(floor_char_boundary("a🎉b", 2), 1); // 🎉 前
        assert_eq!(floor_char_boundary("a🎉b", 10), 6);
        assert_eq!(ceil_char_boundary("a🎉b", 1), 1);
        assert_eq!(ceil_char_boundary("a🎉b", 2), 5); // 跳过 🎉（字节 1..5）
        assert_eq!(ceil_char_boundary("a🎉b", 10), 6);
    }

// ---------- 016 剩余验收：可控时间接缝 / 终止失败注入（单测）----------

/// 读取当前线程的截止时间记录。
fn journal_snapshot() -> Vec<Option<Instant>> {
    TEST_DEADLINE_JOURNAL.with_borrow(|j| j.clone())
}

/// killpg 注入守卫：Drop 时自动解除（断言 panic 不把全局注入泄漏给后续用例）。
struct KillpgFailGuard;
impl Drop for KillpgFailGuard {
    fn drop(&mut self) {
        TEST_KILLPG_FAIL_PGD.store(-1, Ordering::SeqCst);
    }
}

/// 对指定进程组注入 killpg 失败；返回守卫，drop(guard) 或离开作用域即解除。
#[cfg(unix)]
fn arm_killpg_fail(pgid: libc::pid_t) -> KillpgFailGuard {
    TEST_KILLPG_FAIL_PGD.store(pgid as i64, Ordering::SeqCst);
    KillpgFailGuard
}

/// 取指定会话的进程组 id（仅测试：注入 killpg 失败需精确限定目标）。
#[cfg(unix)]
fn session_pgid(mgr: &ProcessManager, id: &str) -> libc::pid_t {
    let pgid = {
        let st = mgr.lock_mgr();
        let sess = st.sessions.get(id).expect("会话应在册").clone();
        let sst = sess.lock();
        match sst.phase {
            Phase::Running { pgid } => pgid,
            _ => panic!("会话 {id} 应运行中"),
        }
    };
    pgid
}

fn writer_cmd(log: &std::path::Path) -> String {
    format!("while true; do echo x >> {}; sleep 0.05; done", log.display())
}

/// 接缝①：省略 timeout_ms＝不设截止（journal 记 None）；正整数（含 >60000 与
/// u64::MAX）＝有截止；查询不新增、不改变已记录的截止时间。
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_deadline_journal_omission_and_acceptance() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    TEST_DEADLINE_JOURNAL.with_borrow_mut(|j| j.clear());
    let dir = std::env::temp_dir().join(format!("tchat-seam-{}-journal", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = ProcessManager::new();
    let args = |timeout_ms: Option<u64>| {
        let mut v = serde_json::json!({ "cmd": "sleep 30", "yield_time_ms": 250 });
        if let Some(t) = timeout_ms {
            v["timeout_ms"] = serde_json::json!(t);
        }
        ExecArgs::from_value(&v).unwrap()
    };
    // 省略：无固定总时限
    assert_eq!(mgr.start(&args(None), None).result.status, ExecStatus::Running);
    // 大于旧 60000 上限的合法值、u64::MAX（本平台可表示）：接受
    assert_eq!(mgr.start(&args(Some(180_000)), None).result.status, ExecStatus::Running);
    assert_eq!(mgr.start(&args(Some(u64::MAX)), None).result.status, ExecStatus::Running);
    let journal = journal_snapshot();
    assert_eq!(journal.len(), 3, "每次成功启动恰记录一条");
    assert_eq!(journal[0], None, "省略 timeout_ms＝不设截止时间");
    assert!(journal[1].is_some(), "180000 合法值有截止");
    assert!(journal[2].is_some(), "u64::MAX 在本平台可表示，有截止");
    // 查询不重置/不延长：不新增记录、已有 Instant 逐值不变
    let stdin = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s2", "yield_time_ms": 5000 })).unwrap();
    let r = mgr.interact(&stdin, None).result;
    assert_eq!(r.status, ExecStatus::Running, "s2 仍在运行：{r:?}");
    let after = journal_snapshot();
    assert_eq!(after, journal, "查询不触碰截止时间记录");
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 接缝②：截止时间计算边界注入失败——拒绝执行、零命令副作用
///（数值防御分支在本平台不可自然触达，见 compute_deadline 文档）。
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_deadline_injection_rejects_execution() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tchat-seam-{}-inj", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let probe = dir.join("probe");
    let mgr = ProcessManager::new();
    TEST_DEADLINE_FAILS.with(|f| f.set(true));
    let args = ExecArgs::from_value(&serde_json::json!({
        "cmd": format!("touch {}", probe.display()),
        "yield_time_ms": 250,
        "timeout_ms": 5000
    }))
    .unwrap();
    let r = mgr.start(&args, None).result;
    TEST_DEADLINE_FAILS.with(|f| f.set(false));
    assert_eq!(r.status, ExecStatus::Error, "{r:?}");
    assert!(r.error.as_deref().unwrap().contains("拒绝执行"), "{r:?}");
    assert!(r.session_id.is_none());
    assert!(!probe.exists(), "拒绝不产生命令副作用");
    assert!(mgr.live_sessions().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 接缝③：终止失败注入——未确认清理成功不得返回“已终止”；进程保留可查询；
/// 不误伤其他会话；解除注入后重试 terminate 成功。
    #[cfg(unix)] // 038：killpg 注入为 unix 设施测试
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_killpg_injection_terminate_reports_failure_keeps_session() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tchat-seam-{}-kill", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log1 = dir.join("w1.log");
    let log2 = dir.join("w2.log");
    let mgr = ProcessManager::new();
    let a1 = ExecArgs::from_value(&serde_json::json!({
        "cmd": writer_cmd(&log1), "yield_time_ms": 250 })).unwrap();
    let a2 = ExecArgs::from_value(&serde_json::json!({
        "cmd": writer_cmd(&log2), "yield_time_ms": 250 })).unwrap();
    assert_eq!(mgr.start(&a1, None).result.status, ExecStatus::Running);
    assert_eq!(mgr.start(&a2, None).result.status, ExecStatus::Running);
    // 就绪：两组写入循环确实在写
    let deadline0 = Instant::now() + Duration::from_secs(10);
    while std::fs::metadata(&log1).map(|m| m.len()).unwrap_or(0) < 10
        || std::fs::metadata(&log2).map(|m| m.len()).unwrap_or(0) < 10
    {
        assert!(Instant::now() < deadline0, "写入循环未就绪");
        std::thread::sleep(Duration::from_millis(30));
    }
    // 注入：仅对 s1 的进程组（守卫 Drop 自动解除，panic 不污染其他用例）
    let _kill_fail = arm_killpg_fail(session_pgid(&mgr, "s1"));
    let term = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "terminate": true })).unwrap();
    let r = mgr.interact(&term, None).result;
    // 核心断言：清理失败不得伪装“已终止”
    assert_eq!(r.status, ExecStatus::Error, "未确认清理成功不得返回 terminated：{r:?}");
    let err = r.error.unwrap_or_default();
    assert!(err.contains("终止") && err.contains("失败"), "错误须如实说明终止失败：{err}");
    assert_eq!(r.session_id.as_deref(), Some("s1"), "失败后句柄保留（不漏管）");
    // 目标进程仍存活（继续写）；错误与实际状态一致
    let s1_before = std::fs::metadata(&log1).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(350));
    assert!(
        std::fs::metadata(&log1).map(|m| m.len()).unwrap_or(0) > s1_before,
        "终止失败后目标进程应仍在运行"
    );
    // 不误清理其他会话：s2 仍运行并可正常操作
    let q2 = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s2", "yield_time_ms": 5000 })).unwrap();
    assert_eq!(mgr.interact(&q2, None).result.status, ExecStatus::Running);
    // 目标仍可查询（如实说明终止失败）
    let q1 = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "yield_time_ms": 5000 })).unwrap();
    let rq = mgr.interact(&q1, None).result;
    assert_eq!(rq.status, ExecStatus::Running, "失败进程保留可查询：{rq:?}");
    // 解除注入后重试 terminate：真实终止、日志停止
    drop(_kill_fail);
    let term2 = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "terminate": true })).unwrap();
    let r2 = mgr.interact(&term2, None).result;
    assert_eq!(r2.status, ExecStatus::Terminated, "解除注入后重试成功：{r2:?}");
    let s1_after = std::fs::metadata(&log1).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(std::fs::metadata(&log1).map(|m| m.len()).unwrap_or(0), s1_after, "终止后停止写入");
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 接缝④：时限到期的终止失败——不得伪装成“已终止回收”的 timeout；
/// 进程保留、如实说明，解除注入后可终止。
    #[cfg(unix)] // 038：killpg 注入为 unix 设施测试
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_killpg_injection_deadline_not_fake_timeout() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tchat-seam-{}-dl", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("w.log");
    let mgr = ProcessManager::new();
    let a = ExecArgs::from_value(&serde_json::json!({
        "cmd": writer_cmd(&log), "yield_time_ms": 250, "timeout_ms": 700 })).unwrap();
    assert_eq!(mgr.start(&a, None).result.status, ExecStatus::Running);
    let _kill_fail = arm_killpg_fail(session_pgid(&mgr, "s1"));
    // 跨过时限（无查询也在计时；注入使终止失败）
    std::thread::sleep(Duration::from_millis(1300));
    let q = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "yield_time_ms": 5000 })).unwrap();
    let r = mgr.interact(&q, None).result;
    assert_ne!(r.status, ExecStatus::Timeout, "终止失败不得伪装 timeout：{r:?}");
    assert_ne!(r.status, ExecStatus::Terminated, "{r:?}");
    assert_eq!(r.status, ExecStatus::Running, "进程仍在运行：{r:?}");
    let err = r.error.unwrap_or_default();
    assert!(err.contains("终止") && err.contains("失败"), "如实说明终止失败：{err}");
    // 进程仍存活
    let before = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(300));
    assert!(std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0) > before);
    // 收口缝：布防门控并等监控停靠（会话级同步点；停靠处**不持会话锁**，
    // 不阻塞下方 request_kill）。停靠后挂起的 Deadline 请求不可能被 10ms
    // 轮询重试消费——显式 terminate 的提交被确定性编排到消费之前（提交可见
    // 即自动放行），下方断言验证的覆盖语义不变，不再依赖抢在 tick 前执行
    //（2026-10-27 负载竞态 flake 的确定性修复；RED 证据见收口记录）。
    let sess = {
        let st = mgr.lock_mgr();
        st.sessions.get("s1").expect("会话应在册").clone()
    };
    let _gate = sess.arm_monitor_gate();
    let parked_deadline = Instant::now() + Duration::from_secs(5);
    while sess.monitor_gate_state() != Some(TestGateState::Parked) {
        assert!(Instant::now() < parked_deadline, "监控未停靠收口门控");
        std::thread::sleep(Duration::from_millis(2));
    }
    // 解除注入后显式 terminate：用户显式终止覆盖仍挂起的时限请求，
    // 终止成功后事由归 terminate（请求共存不被静默丢弃）
    drop(_kill_fail);
    let term = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "terminate": true })).unwrap();
    let r2 = mgr.interact(&term, None).result;
    assert_eq!(r2.status, ExecStatus::Terminated, "显式 terminate 优先完成终止：{r2:?}");
    let s_end = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0), s_end, "终止后停止写入");
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- 三轮：可控时钟驱动真实到期判定（稳定、无宽松容差）----------

/// 时钟前移守卫：Drop 自动复位（断言 panic 不把前移泄漏给后续用例）。
struct ClockShiftGuard;
impl Drop for ClockShiftGuard {
    fn drop(&mut self) {
        TEST_CLOCK_SHIFT_MS.store(0, Ordering::SeqCst);
    }
}

/// 将到期判定的单调读数前移 ms 毫秒（真实终止仍由监控线程执行）。
fn arm_clock_shift(ms: u64) -> ClockShiftGuard {
    TEST_CLOCK_SHIFT_MS.store(ms, Ordering::SeqCst);
    ClockShiftGuard
}

/// 日志停止增长的有界判定（真实副作用：写入循环死亡）。
fn wait_log_frozen(log: &std::path::Path) -> u64 {
    let size = || std::fs::metadata(log).map(|m| m.len()).unwrap_or(0);
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut prev = size();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let now = size();
        if now == prev {
            return now;
        }
        prev = now;
        assert!(Instant::now() < deadline, "日志未停止增长（{prev}）");
    }
}

/// 时钟前移到恰过截止：无任何后续查询，监控线程自行到期终止真实进程
///（日志停止增长＝进程组死亡），事后一次查询确认 timeout 与记录移除。
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_clock_shift_expiry_without_query() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tchat-seam3-{}-exp", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("w.log");
    let mgr = ProcessManager::new();
    let a = ExecArgs::from_value(&serde_json::json!({
        "cmd": writer_cmd(&log), "yield_time_ms": 250,
        "timeout_ms": 3_600_000_u64
    }))
    .unwrap();
    assert_eq!(mgr.start(&a, None).result.status, ExecStatus::Running);
    // 写入循环确实在运行（有界等待日志非空——真实副作用证据）
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0) == 0 {
            assert!(Instant::now() < deadline, "写入循环未启动");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    // 前移到截止之后（原截止＝启动+3_600_000ms；无查询参与）
    let _shift = arm_clock_shift(3_600_500);
    let frozen = wait_log_frozen(&log);
    assert!(frozen > 0, "写入循环确实运行过");
    let q = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "yield_time_ms": 5000 })).unwrap();
    let r = mgr.interact(&q, None).result;
    assert_eq!(r.status, ExecStatus::Timeout, "到期终止如实报告：{r:?}");
    assert_eq!(r.exit_code, None);
    // 记录移除
    let again = mgr.interact(&q, None).result;
    assert_eq!(again.status, ExecStatus::Error);
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 取消当前轮（等待被取消、进程保留）后，原截止时间仍然生效：
/// 前移到恰过**启动时**截止即终止——若取消会重置/暂停时限则不会到期。
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_clock_shift_deadline_survives_cancel() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tchat-seam3-{}-cxl", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("w.log");
    let mgr = ProcessManager::new();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let c2 = cancel.clone();
    let a = ExecArgs::from_value(&serde_json::json!({
        "cmd": writer_cmd(&log), "yield_time_ms": 4000,
        "timeout_ms": 3_600_000_u64
    }))
    .unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c2.store(true, Ordering::SeqCst);
    });
    let out = mgr.start(&a, Some(&cancel)).result;
    assert_eq!(out.status, ExecStatus::Cancelled, "{out:?}");
    let sid = out.session_id.expect("取消后保留句柄");
    cancel.store(false, Ordering::SeqCst);
    // 取消后进程仍在写
    let q = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": sid, "yield_time_ms": 5000 })).unwrap();
    assert_eq!(mgr.interact(&q, None).result.status, ExecStatus::Running);
    // 前移恰过启动时截止：仍到期（取消不重置/不暂停）
    let _shift = arm_clock_shift(3_600_500);
    wait_log_frozen(&log);
    let r = mgr.interact(&q, None).result;
    assert_eq!(r.status, ExecStatus::Timeout, "取消不改变原截止：{r:?}");
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 查询与输入不延长截止：先查询（管道会话）并发送输入（PTY 会话），
/// 再前移到恰过**启动时**截止——若查询/输入重置截止则不会到期。
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_clock_shift_query_and_input_do_not_extend_deadline() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tchat-seam3-{}-ext", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mgr = ProcessManager::new();
    let timeout = 3_600_000_u64;
    // 管道会话：跨“轮”（不同 run_tool_turn 间等价于跨调用查询同一句柄）
    let a1 = ExecArgs::from_value(&serde_json::json!({
        "cmd": "sleep 60", "yield_time_ms": 250, "timeout_ms": timeout
    }))
    .unwrap();
    let out1 = mgr.start(&a1, None).result;
    assert_eq!(out1.status, ExecStatus::Running);
    // PTY 会话：发送输入
    let a2 = ExecArgs::from_value(&serde_json::json!({
        "cmd": "stty raw -echo 2>/dev/null; sleep 60", "yield_time_ms": 250,
        "tty": true, "timeout_ms": timeout
    }))
    .unwrap();
    let out2 = mgr.start(&a2, None).result;
    assert_eq!(out2.status, ExecStatus::Running);
    let sid2 = out2.session_id.unwrap();
    // 查询 s1（阻塞至查询窗口结束，取走输出——重置缺陷的触发点）
    let q1 = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "yield_time_ms": 5000 })).unwrap();
    assert_eq!(mgr.interact(&q1, None).result.status, ExecStatus::Running);
    // 输入 s2（PTY 原样写入）
    let input = WriteStdinArgs::from_value(&serde_json::json!({
        "session_id": sid2, "chars": "hi\n", "yield_time_ms": 250
    }))
    .unwrap();
    let r = mgr.interact(&input, None).result;
    assert_eq!(r.status, ExecStatus::Running, "{r:?}");
    // 前移恰过启动时截止（两会话几乎同时启动）：若查询/输入重置了截止，
    // 有效截止将晚于前移读数——不会到期，断言失败。
    let _shift = arm_clock_shift(timeout + 500);
    let q2 = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "yield_time_ms": 5000 })).unwrap();
    let q3 = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": sid2, "yield_time_ms": 5000 })).unwrap();
    let r1 = mgr.interact(&q2, None).result;
    let r2 = mgr.interact(&q3, None).result;
    assert_eq!(r1.status, ExecStatus::Timeout, "查询不延长截止：{r1:?}");
    assert_eq!(r2.status, ExecStatus::Timeout, "输入不延长截止：{r2:?}");
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// 省略 timeout_ms：单调读数前移一小时后进程仍存活（不套用任何固定总时限），
/// 日志持续增长；随后明确终止成功。
#[cfg(unix)] // 038：seam 时钟注入经子进程 sh 语法命令（unix 设施）
#[test]
fn seam_clock_shift_omitted_timeout_has_no_fixed_limit() {
    let _g = SEAM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tchat-seam3-{}-omit", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("w.log");
    let mgr = ProcessManager::new();
    let a = ExecArgs::from_value(&serde_json::json!({
        "cmd": writer_cmd(&log), "yield_time_ms": 250
    }))
    .unwrap();
    assert_eq!(mgr.start(&a, None).result.status, ExecStatus::Running);
    // 读数前移 1 小时（远超旧 10 秒默认）：不设固定总时限＝任何时间流逝都不终止
    let _shift = arm_clock_shift(3_600_000);
    std::thread::sleep(Duration::from_millis(600));
    let s1 = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(400));
    let s2 = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    assert!(s2 > s1, "时间流逝后进程仍在运行（无固定总时限）：{s1} -> {s2}");
    let q = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "yield_time_ms": 5000 })).unwrap();
    assert_eq!(mgr.interact(&q, None).result.status, ExecStatus::Running);
    let term = WriteStdinArgs::from_value(&serde_json::json!({ "session_id": "s1", "terminate": true })).unwrap();
    assert_eq!(mgr.interact(&term, None).result.status, ExecStatus::Terminated);
    mgr.close();
    let _ = std::fs::remove_dir_all(&dir);
}
}

// ===== Windows 平台层（038）：作业对象＝进程组契约的等价物 =====
//
// 树终止契约（016：terminate/timeout/取消整组回收含后代）：子进程 spawn 后
// 立即 AssignProcessToJobObject；组硬终止＝TerminateJobObject（整树——后代
// 默认继承作业）；句柄随注册表存活，KILL_ON_JOB_CLOSE 兜底泄漏防护。
#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        SetInformationJobObject, TerminateJobObject,
    };

    /// HANDLE（*mut c_void）非 Send——JobHandle newtype 经显式 unsafe impl
    /// Send 授权跨线程移动（内核句柄天然可跨线程使用；专用于本注册表）。
    #[derive(Clone, Copy)]
    struct JobHandle(HANDLE);
    unsafe impl Send for JobHandle {}

    static JOBS: std::sync::LazyLock<Mutex<HashMap<u32, JobHandle>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

    pub fn assign_child_to_job(pid: u32, child: &Child) -> io::Result<()> {
        assign_raw_process(pid, child.as_raw_handle() as HANDLE)
    }

    /// 038 ConPTY：CreateProcessW 产物的原生句柄挂接（无 std Child 可用）。
    pub fn assign_raw_process(pid: u32, process: HANDLE) -> io::Result<()> {
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                CloseHandle(job);
                return Err(io::Error::last_os_error());
            }
            if AssignProcessToJobObject(job, process) == 0 {
                CloseHandle(job);
                return Err(io::Error::last_os_error());
            }
        }
        JOBS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pid, JobHandle(job));
        Ok(())
    }

    pub fn terminate_tree(pid: u32) -> io::Result<()> {
        let job = JOBS.lock().unwrap_or_else(|e| e.into_inner()).remove(&pid);
        match job {
            Some(job) => unsafe {
                let ok = TerminateJobObject(job.0, 1);
                CloseHandle(job.0);
                if ok == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            },
            None => Err(io::Error::other("作业句柄不存在（会话未注册或已终止）")),
        }
    }
}

#[cfg(all(test, unix))]
mod ns_sweep_tests {
    use super::*;

    /// D2 回归辅助：与 confirm_sandbox_isolation_and_release 的载荷定位同构
    ///（children 中 ns≠self 的首个进程）；confirm 已把该逻辑用于放行闸，
    /// 本辅助仅服务回归断言（不改产品行为）。
    #[cfg(unix)]
    fn test_payload_ns_inode(leader_pid: u32) -> Option<String> {
        let self_ns = std::fs::read_link("/proc/self/ns/pid").ok()?;
        let tasks = std::fs::read_dir(format!("/proc/{leader_pid}/task")).ok()?;
        for t in tasks.flatten() {
            let Ok(text) = std::fs::read_to_string(t.path().join("children")) else {
                continue;
            };
            for pid_str in text.split_whitespace() {
                let Ok(pid) = pid_str.parse::<u32>() else {
                    continue;
                };
                if let Ok(ns) = std::fs::read_link(format!("/proc/{pid}/ns/pid")) {
                    if ns != self_ns {
                        return Some(ns.to_string_lossy().into_owned());
                    }
                }
            }
        }
        None
    }

    /// 038 D2 修正回归（2026-11-02）：ns 补杀圈定必须是载荷 ns≠调用者 ns。
    /// 旧实现读组长（bwrap）自身 ns＝调用者 ns，sweep 会 SIGKILL 调用者 ns
    /// 全体进程（容器内 zcode server 连坐死亡实证）。断言双向：①完整沙箱
    /// 形态下捕获到载荷 ns 且≠self ns；②非沙箱进程（无新 ns 子进程）返回
    /// None（降级不补杀）。bwrap 不可用的环境（CI 裸机）跳过——圈定断言
    /// 依赖真实 ns 结构。
    #[test]
    fn payload_ns_inode_targets_sandbox_not_caller() {
        let probe = std::process::Command::new("bwrap")
            .args(["--unshare-user", "--unshare-ipc", "--ro-bind", "/", "/", "true"])
            .status();
        if !probe.map(|s| s.success()).unwrap_or(false) {
            eprintln!("bwrap 不可用，跳过 ns 圈定回归（无真实 ns 结构可比对）");
            return;
        }
        // 完整沙箱形态（033 argv 的 ns 集：NEWNS|NEWUSER|NEWPID|NEWIPC），
        // 载荷长跑保证 children 可读窗口。
        let mut leader = std::process::Command::new("bwrap")
            .args([
                "--unshare-user",
                "--unshare-pid",
                "--unshare-ipc",
                "--dev-bind",
                "/",
                "/",
                "sleep",
                "5",
            ])
            .spawn()
            .expect("bwrap 完整沙箱形态 spawn 失败");
        let leader_pid = leader.id();
        let self_ns = std::fs::read_link("/proc/self/ns/pid")
            .expect("self ns/pid 必须可读")
            .to_string_lossy()
            .into_owned();
        // 载荷出现轮询（verify_child_pid_namespace 同窗，放宽至 500ms）。
        let deadline = Instant::now() + Duration::from_millis(500);
        let ns = loop {
            match test_payload_ns_inode(leader_pid) {
                Some(ns) => break ns,
                None if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                None => panic!("沙箱载荷 ns 未捕获（500ms 窗口；bwrap 载荷应已在新 ns）"),
            }
        };
        assert_ne!(
            ns, self_ns,
            "载荷 ns 不得等于调用者 ns——旧实现（读组长自身）即此错误形态，\
             该形态下 sweep 会杀调用者 ns 全体"
        );
        // 反向：无沙箱子进程的普通进程必须 None（降级不补杀，killpg 兜底）。
        let mut plain = std::process::Command::new("sleep").arg("2").spawn().unwrap();
        assert_eq!(
            test_payload_ns_inode(plain.id()),
            None,
            "无新 ns 子进程必须返回 None"
        );
        let _ = plain.kill();
        let _ = plain.wait();
        let _ = leader.kill();
        let _ = leader.wait();
    }
}
