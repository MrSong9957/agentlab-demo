//! 008-P3：TUI 宿主接入——ChatDriver 钩子实现与交互入口切换。
//!
//! 契约（008 规格）：
//! - 交互判定：stdin＋stdout 均为终端且 TERM 非 dumb（未设置按能力探测）才进入 TUI；
//!   其余走既有行式路径（001/003–007 契约原样）。
//! - API 非流式、turn 同步：一轮在独立线程运行 `turn::run_tool_turn`（006 契约不变），
//!   整段回复按换行边界合成 delta 回推（J6；不宣称 SSE）。
//! - Ctrl+C：TUI 内空闲＝退出（恢复终端后由 main 打印 `再见!`、退出码 0）；
//!   turn 运行中＝006 N2b 取消（置标志→SIGKILL 进程组→CLI 存活）；在途模型
//!   请求同受取消（sender 等待侧轮询标志，010 定点轮）。
//! - 授权门（003）：进入 TUI 前对所选 provider 完成一次性 origin 授权（普通终端交互）；
//!   会话中 /refresh 产生的**新增** origin 授权需求按非交互契约拒绝并提示（不静默放行）。
//! - /resume：装载历史经 `tchat_tui::chat_driver::project_history` 投影（不重放工具，007）。

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use tchat_tui::chat_driver::DriverEvent;
use tchat_tui::chat_driver::CommandOutcome as Outcome;
use tchat_tui::chat_driver::HostHistoryItem;
use tchat_tui::chat_driver::HostPatchChange;
use tchat_tui::chat_driver::HostPatchKind;
use tchat_tui::chat_driver::ToolBeginKind;

use crate::prompt::HistoryItem;
use crate::prompt::ToolDisplay;
use crate::proc_session::ProcessManager;
use crate::turn::SteerChannel as _;
use crate::{api, catalog, session, turn};

/// 是否应进入 TUI 模式。
pub fn tui_should_run() -> bool {
    use std::io::IsTerminal;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return false;
    }
    match std::env::var("TERM") {
        Ok(term) => term != "dumb" && !term.is_empty(),
        // TERM 未设置：按母本能力探测（terminal-detection）裁决，不直接拒绝
        Err(_) => tchat_tui::terminal_supported(),
    }
}

/// 021：当前轮接续通道受理资格（短锁；提交侧不获取 history/autosave 锁）。
enum SteerSlot {
    /// 接受中：提交经 tx 入队，由本轮接续通道消化。
    Open { tx: std_mpsc::Sender<turn::SteerInput> },
    /// 已封口／无活动轮：提交得到"当前轮已结束"回执。
    Closed,
}

/// 021：每轮独立的接续通道实现——rx 由轮线程持有；受理资格经短锁与提交侧
/// 互斥，`seal`/`close` 的"收取＋封口"在同一临界区内原子完成。
struct TurnSteerChannel {
    slot: Arc<Mutex<SteerSlot>>,
    rx: std_mpsc::Receiver<turn::SteerInput>,
}

impl TurnSteerChannel {
    fn drain_rx(&self) -> Vec<turn::SteerInput> {
        let mut out = Vec::new();
        while let Ok(input) = self.rx.try_recv() {
            out.push(input);
        }
        out
    }
}

impl turn::SteerChannel for TurnSteerChannel {
    fn drain(&mut self) -> Vec<turn::SteerInput> {
        self.drain_rx()
    }
    fn seal(&mut self) -> turn::SteerSeal {
        let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*guard, SteerSlot::Open { .. }) {
            return turn::SteerSeal::Closed;
        }
        // 同一临界区内判定：先收取再封口，提交侧无法插入其间——
        // 通道为空才真正封口（终文竞态原子判定，§2.3-5）
        let inputs = self.drain_rx();
        if inputs.is_empty() {
            *guard = SteerSlot::Closed;
            turn::SteerSeal::Closed
        } else {
            turn::SteerSeal::Inputs(inputs)
        }
    }
    fn close(&mut self) -> Vec<turn::SteerInput> {
        let mut guard = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        *guard = SteerSlot::Closed;
        self.drain_rx()
    }
}

/// 021：turn::SteerInput → 驱动事件载荷（id 关联恰一次消化）。
fn steer_submission_of(
    input: turn::SteerInput,
) -> tchat_tui::chat_driver::SteerSubmission {
    tchat_tui::chat_driver::SteerSubmission {
        id: input.id,
        text: input.text,
        image_paths: input.image_paths,
        text_elements: input.text_elements,
    }
}

/// TUI 会话宿主：持有跨轮状态（历史、自动落盘、取消标志）。
pub struct TuiHost {
    agent: ureq::Agent,
    history: Arc<Mutex<Vec<HistoryItem>>>,
    autosave: Arc<Mutex<session::Session>>,
    /// 启动时已授权的 (origin, 凭证指纹) 对；会话中不同 origin 或不同凭证
    /// 一律走非交互授权门（003：凭证与已授权地址绑定）。
    authorized: Arc<Mutex<Option<(String, String)>>>,
    /// 013-M7：threads 登记推进器（缓存库连接；在 autosave 锁内串行访问）。
    registrar: Arc<Mutex<crate::memory::runner::ThreadRegistrar>>,

    /// 016：进程会话管理器（当前对话；跨轮保留进程，TUI Ctrl+C 退出时关闭）。
    procs: ProcessManager,
    /// 021：当前轮接续通道受理资格（spawn 同步置 Open；seal/close 置 Closed）。
    steer_slot: Arc<Mutex<SteerSlot>>,
    /// 021：接续提交标识发生器（宿主生成；回执按 id 恰一次关联）。
    steer_seq: std::sync::atomic::AtomicU64,
    /// 022：MCP 管理器（跨轮保留连接；退出显式关闭）。
    mcp: Arc<crate::mcp::manager::McpManager>,
}

impl TuiHost {
    /// 驱动取消钩子直连（006 N2b：置标志→工具线程终止进程组）。
    pub fn set_cancel(&self, requested: bool) {
        HOST_CANCEL.store(requested, Ordering::SeqCst);
    }

    /// 016：宿主退出（TUI Ctrl+C）——显式关闭全部进程会话并回收。
    pub fn close_process_sessions(&self) {
        self.procs.close();
    }

    /// 022：宿主退出——显式关闭全部 MCP 连接（有界；不依赖 Drop）。
    pub fn close_mcp(&self) {
        self.mcp.close();
    }

    pub fn new(agent: ureq::Agent, autosave: session::Session) -> Self {
        Self {
            agent,
            history: Arc::new(Mutex::new(Vec::new())),
            autosave: Arc::new(Mutex::new(autosave)),
            authorized: Arc::new(Mutex::new(None)),
            registrar: Arc::new(Mutex::new(
                crate::memory::runner::ThreadRegistrar::default(),
            )),
            procs: ProcessManager::new(),
            steer_slot: Arc::new(Mutex::new(SteerSlot::Closed)),
            steer_seq: std::sync::atomic::AtomicU64::new(0),
            mcp: crate::mcp::manager::McpManager::new(),
        }
    }

    /// 021：运行中接续提交（驱动 Enter 路径）。Ok(id)＝已受理（当前轮批末或
    /// 终文判定点接入）；Err＝当前轮已结束（驱动转后续轮）。
    /// 只取短锁并入队，不获取 turn 长时间持有的 history/autosave 锁。
    pub fn submit_steer(
        &self,
        content: tchat_tui::chat_driver::SteerContent,
    ) -> Result<u64, String> {
        use std::sync::atomic::Ordering;
        let id = self.steer_seq.fetch_add(1, Ordering::SeqCst);
        let input = turn::SteerInput {
            id,
            text: content.text,
            image_paths: content.image_paths,
            text_elements: content.text_elements,
        };
        let mut guard = self.steer_slot.lock().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            SteerSlot::Open { tx } => match tx.send(input) {
                Ok(()) => Ok(id),
                // 接收端已不在（轮线程异常终止）＝轮不可再接续
                Err(_) => {
                    *guard = SteerSlot::Closed;
                    Err("当前轮已结束".to_string())
                }
            },
            SteerSlot::Closed => Err("当前轮已结束".to_string()),
        }
    }

    pub fn set_authorized(&self, origin: String, fp: String) {
        *self.authorized.lock().unwrap() = Some((origin, fp));
    }

    /// 进入 TUI 前的启动期 origin 授权（普通终端交互，tui_should_run 后、raw mode 前）。
    pub fn authorize_startup_origin(
        &self,
        prompt: &mut dyn FnMut(&str) -> bool,
    ) -> Result<(), String> {
        let dict = catalog::build_final().map_err(|e| format!("构建配置失败：{e}"))?;
        let (prov, cred, _model) = catalog::route(&dict)?;
        let Ok(origin) = catalog::origin_of(&prov.conn.base_url) else {
            return Ok(());
        };
        let fp = catalog::fingerprint(&cred.value);
        catalog::ensure_origin_authorized(&origin, &cred.name, &fp, true, prompt)?;
        self.set_authorized(origin, fp);
        Ok(())
    }

    /// 007 退出排空（与行式路径同契约）：on_history 写失败的滞留待写段在退出前
    /// 补写。先无嵌套快照 history 再独占 autosave——turn 线程持 history 期间经
    /// on_record 请求 autosave，反向嵌套会 ABBA（§16 死锁修复的锁序约束）。
    pub fn flush_final_session(&self) -> Result<(), String> {
        // 独立复审 P2-1：毒锁（turn 线程 panic 残留）下恢复数据而非退出路径再
        // panic——退出须能完成（无 再见!/码 101 不可接受）
        let history = self
            .history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.autosave
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush_final(&history)
            .map_err(|e| e.to_string())
    }

    /// 驱动 spawn_turn 钩子的直连（公开给 main 组装）。
    /// 020：完整提交（文本＋附件路径＋元素）——线程内正式准备图片。
    /// 021：同步建立本轮接续通道（驱动 turn_running 置位后即可提交接续输入）。
    pub fn spawn_turn(&self, submission: tchat_tui::chat_driver::SubmittedTurn, tx: std_mpsc::Sender<DriverEvent>) {
        let (steer_tx, steer_rx) = std_mpsc::channel();
        *self.steer_slot.lock().unwrap_or_else(|e| e.into_inner()) = SteerSlot::Open { tx: steer_tx };
        self.run_turn_inner(submission, tx, steer_rx);
    }

    fn run_turn_inner(
        &self,
        submission: tchat_tui::chat_driver::SubmittedTurn,
        tx: std_mpsc::Sender<DriverEvent>,
        steer_rx: std_mpsc::Receiver<turn::SteerInput>,
    ) {
        let history = self.history.clone();
        let autosave = self.autosave.clone();
        let agent = self.agent.clone();
        let authorized_guard = self.authorized.clone();
        let registrar = self.registrar.clone();
        let procs = self.procs.clone();
        // 021：接续槽位句柄（线程内经短锁与提交侧互斥，不捕获 &self）
        let steer_slot = self.steer_slot.clone();
        // 022：MCP 管理器句柄（线程内准备快照；不捕获 &self）
        let mcp = self.mcp.clone();
        let tx_panic = tx.clone();
        std::thread::spawn(move || {
            // 008：线程异常终止（如毒锁 panic）不得静默——回推 Failed 让驱动恢复
            // 可用（否则 turn_running 永久卡死；008-REVIEW 独立审查 P2-2）
            let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 置位本轮取消标志（驱动在空闲时清零）
            HOST_CANCEL.store(false, Ordering::SeqCst);
            let dict = match catalog::build_final() {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(DriverEvent::Failed(e));
                    return;
                }
            };
            let (prov, cred, model) = match catalog::route(&dict) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(DriverEvent::Failed(e));
                    return;
                }
            };
            // 020：用户附件正式准备（能力检查先于文件打开；入队阶段未读取）。
            // 失败＝SubmissionRejected：消息不入史、零请求，驱动恢复草稿。
            let user_payload: Option<crate::prompt::UserImagePayload> =
                if submission.local_images.is_empty() {
                    None
                } else {
                    let paths: Vec<std::path::PathBuf> =
                        submission.local_images.iter().map(|a| a.path.clone()).collect();
                    match crate::view_image::prepare_user_images(
                        &paths,
                        catalog::supports_image_input(&model),
                        Some(&HOST_CANCEL),
                    ) {
                        Ok(data_urls) => Some(crate::prompt::UserImagePayload {
                            data_urls,
                            text_elements: submission.text_elements.clone(),
                        }),
                        Err(reason) => {
                            let _ = tx.send(DriverEvent::SubmissionRejected {
                                text: submission.text.clone(),
                                image_paths: paths,
                                reason,
                            });
                            return;
                        }
                    }
                };
            // 003 origin 门：会话中 origin 或凭证与启动授权不一致时按非交互契约拒绝
            if let Ok(origin) = catalog::origin_of(&prov.conn.base_url) {
                let fp = catalog::fingerprint(&cred.value);
                let authorized = authorized_guard.lock().unwrap();
                let already = authorized
                    .as_ref()
                    .is_some_and(|(auth_origin, auth_fp)| *auth_origin == origin && *auth_fp == fp);
                drop(authorized);
                if !already {
                    if let Err(e) = catalog::ensure_origin_authorized(
                        &origin, &cred.name, &fp, /*interactive*/ false, &mut |_| false,
                    ) {
                        let _ = tx.send(DriverEvent::Failed(e));
                        return;
                    }
                }
            }
            // 013-M7：记忆配置每轮装载一次（manual 段；无段＝默认关闭）。
            let mem_config = crate::memory::runner::load_memories_config();
            let mem_user_dir = catalog::user_dir();
            // 013-M7 D2：带输入新轮已通过 route/origin 门＝成功启动，触发一次后台
            // 管线（enabled gate 在 runner 内；主轮成败不影响；退出不等待）。
            crate::memory::runner::spawn_memory_pipeline(
                &mem_user_dir,
                &mem_config,
                autosave.lock().unwrap().current_target_stem(),
                crate::memory::runner::ModelSend {
                    provider_id:prov.provider_id.clone(),
                    conn: prov.conn.clone(),
                    key: cred.value.clone(),
                    model: model.clone(),
                    agent: agent.clone(),
                },
            );
            autosave.lock().unwrap().prepare_native_model(&prov.conn.protocol);
            let provider_id = prov.provider_id;
            let conn = prov.conn;
            let key = cred.value;
            // 010 定点轮＋收尾轮（J10 契约②）：Ctrl+C 取消＝**停止本地等待**——
            // 当前实现未中止底层 HTTP 请求（等待侧不关 socket、不中断/不 join
            // 辅助线程），请求移入辅助线程执行、等待侧轮询取消标志；取消＝Err
            // 结束本轮（Failed 反馈行如实表述「已停止等待」，不宣称网络请求已
            // 中止）。被弃请求的辅助线程各自存活至响应到达或 agent 总超时
            // （180s）；并存数量无上限（随重叠取消次数增长，S72 实测基线＋2、
            // 各自独立回收），迟到结果发往已弃接收端丢弃——不影响后续 turn/
            // 会话/界面（S71/S72 /proc 线程数定点核查）。工具批间/执行中的取消
            // 路径不变（006 N2b）。
            let mut sender = |body: &serde_json::Value| {
                let (tx, rx) = std_mpsc::channel();
                let conn = conn.clone();
                let provider_id = provider_id.clone();
                let key = key.clone();
                let model = model.clone();
                let agent = agent.clone();
                let body = body.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(api::chat_with_provider_body(&provider_id, &conn, &key, &model, &body, &agent));
                });
                loop {
                    match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(result) => return result,
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {
                            if HOST_CANCEL.load(Ordering::SeqCst) {
                                return Err(
                                    "用户已取消本轮：已停止等待模型回复（迟到结果将被丢弃）"
                                        .to_string(),
                                );
                            }
                        }
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                            return Err("模型请求线程异常终止".to_string());
                        }
                    }
                }
            };
            // 012：工具活动结构化事件→驱动工具 cell；Info 保持单行提示通道。
            // 参数不可解析（exec 无 cmd）或未知工具回退原活动行（Info）。
            // 016：write_stdin 的 Begin/End 关联原命令（会话记录），不伪造新命令；
            // 运行中状态携带会话编号，独立呈现。
            // 018-B：异步问询等待/回答/关闭→驱动事件；展示副本在此过滤，
            // 关联 ID（request_id）保持原值。
            let procs_for_notify = procs.clone();
            let mut notify = |n: turn::TurnNotify| {
                let ev: Option<DriverEvent> = match n {
                    turn::TurnNotify::Info(line) => Some(DriverEvent::Info(line)),
                    turn::TurnNotify::UserInputAsyncWait { request_id } => {
                        Some(DriverEvent::UserInputAsyncWait { request_id })
                    }
                    turn::TurnNotify::UserInputAsyncAnswered {
                        request_id,
                        questions,
                        response,
                    } => Some(DriverEvent::UserInputAsyncUpdate {
                        request_id,
                        update: tchat_tui::chat_driver::AsyncInputUpdate::Answered {
                            questions: crate::user_input::sanitized_questions(&questions),
                            response: crate::user_input::sanitized_response(&response),
                        },
                    }),
                    turn::TurnNotify::UserInputAsyncClosed {
                        request_id,
                        questions,
                        reason,
                    } => Some(DriverEvent::UserInputAsyncUpdate {
                        request_id,
                        update: tchat_tui::chat_driver::AsyncInputUpdate::Closed {
                            questions: crate::user_input::sanitized_questions(&questions),
                            reason: crate::display::sanitize_for_terminal(&reason),
                        },
                    }),
                    // 021：接续输入回执——入史确认／未入史交还（原因已过终端过滤）
                    turn::TurnNotify::SteerIngested { input } => {
                        Some(DriverEvent::SteerIngested { input: steer_submission_of(input) })
                    }
                    turn::TurnNotify::SteerRestored { inputs, reason } => {
                        Some(DriverEvent::SteerRestored {
                            inputs: inputs.into_iter().map(steer_submission_of).collect(),
                            reason: crate::display::sanitize_for_terminal(&reason),
                        })
                    }
                    turn::TurnNotify::ToolBegin { id, name, arguments } => {
                        // 021：消息工具无运行活动可展示——成功路径直接投影为
                        // 消息 cell，失败原因在 ToolEnd 显式呈现；不打印无信息
                        // 量的活动行（成功通知不重复打印正文）。
                        if name == "send_message_to_user_async" {
                            None
                        } else {
                            let ev = if name == "write_stdin" {
                                stdin_begin_event(&procs_for_notify, &id, &arguments)
                            } else {
                                tool_begin_event(&name, &id, &arguments)
                            };
                            Some(ev.unwrap_or_else(|| {
                                DriverEvent::Info(crate::display::sanitize_for_terminal(&turn::tool_activity_line(&name, &id)))
                            }))
                        }
                    }
                    turn::TurnNotify::ToolEnd { id, name, arguments, result, display, cancelled } => {
                        // 未知工具：无 cell 可替换（Begin 已回退活动行），不发送
                        let item = tool_end_item(&name, &id, &arguments, &result, display.as_ref(), cancelled);
                        match item {
                            Some(item) => Some(DriverEvent::ToolEnd { id, item }),
                            // 021：消息工具失败/取消/结果未知——活动行已被抑制，
                            // 这里显式呈现原因，不静默（展示副本过终端过滤）
                            None if name == "send_message_to_user_async" => Some(DriverEvent::Info(
                                crate::display::sanitize_for_terminal(
                                    &crate::message_to_user::failure_line(&result),
                                ),
                            )),
                            None => None,
                        }
                    }
                };
                if let Some(ev) = ev {
                    let _ = tx.send(ev);
                }
            };
            let mut on_record = |h: &[HistoryItem]| {
                let mut session = autosave.lock().unwrap();
                if let Some(msg) = session.on_history(h) {
                    let _ = tx.send(DriverEvent::Info(msg));
                }
                // 013-M7：threads 登记——仅 (stem, durable_len) 变化时推进
                //（未成功持久化的数据不登记为已更新版本；registrar 恒在
                // autosave 锁内串行访问，无跨线程竞争）。
                let mut reg = registrar.lock().unwrap();
                reg.note_persisted(&mem_user_dir, &mem_config, &session);
            };
            // 006 N2b：工具期 SIGINT 仅置取消标志
            unsafe {
                libc::signal(
                    libc::SIGINT,
                    on_host_sigint as *const () as libc::sighandler_t,
                );
            }
            // 013-M7：开启态以 UserWithMemory 接入 M6 注入（轮首读取一次、轮内
            // 复用、不入史——turn 层承接）；关闭态保持默认 User（逐字节不变）。
            let mem_root = crate::memory::runner::memories_root(&mem_user_dir);
            let mode = if mem_config.enabled {
                turn::ToolMode::UserWithMemory {
                    root: &mem_root,
                    config: &mem_config,
                }
            } else {
                turn::ToolMode::User
            };
            // 015：用户轮上下文来源（轮线程内解析：全局规则目录、cwd、SHELL、时钟）
            let context_sources = crate::context::ContextSources::from_env();
            let mut input_host = |_: &turn::ParsedToolCall, questions: &[crate::user_input::Question]| {
                let (reply, receiver) = std_mpsc::channel();
                let live = Arc::new(AtomicBool::new(true));
                let _ = tx.send(DriverEvent::UserInput {
                    questions: questions.to_vec(),
                    reply: tchat_tui::chat_driver::UserInputReply::new(reply),
                    live: live.clone(),
                    display_filter: crate::display::sanitize_for_terminal,
                });
                crate::user_input::wait_for_response(receiver, &HOST_CANCEL, &live)
            };
            // 018 阶段 B：异步发布——校验通过后建立一次性回执通道并交给 UI 事件
            // 通道，随即返回（不等待答案）；回答/取消不获取 history 或 autosave 锁。
            let mut async_input_host =
                |call: &turn::ParsedToolCall, questions: &[crate::user_input::Question]| {
                    let (reply, receiver) = std_mpsc::channel();
                    let live = Arc::new(AtomicBool::new(true));
                    let sent = tx.send(DriverEvent::UserInputAsync {
                        request_id: call.id.clone(),
                        questions: questions.to_vec(),
                        reply: tchat_tui::chat_driver::UserInputReply::new(reply),
                        live: live.clone(),
                        display_filter: crate::display::sanitize_for_terminal,
                    });
                    if sent.is_err() {
                        return Err("问询发布通道已关闭：问询未完成".to_string());
                    }
                    Ok(crate::user_input_async::AsyncReplyChannel { receiver, live })
                };
            // 021：本轮接续通道实现（rx 由本线程持有；受理资格经短锁与提交侧互斥）
            let mut steer_channel = TurnSteerChannel { slot: steer_slot, rx: steer_rx };
            // 022：MCP 快照在普通用户轮准备时取得（首次触发配置加载与连接；
            // 取消＝结束本轮、不发模型请求；已就绪连接保留、候选回收）。
            let mcp_snapshot = match mcp.prepare_for_turn(&HOST_CANCEL) {
                Some(snapshot) => Some(snapshot),
                None => {
                    let _ = tx.send(DriverEvent::SubmissionRejected {
                        text: submission.text.clone(),
                        image_paths: submission
                            .local_images
                            .iter()
                            .map(|a| a.path.clone())
                            .collect(),
                        reason: "用户已取消（MCP 准备阶段）".to_string(),
                    });
                    return;
                }
            };
            let extras = turn::TurnExtras {
                // 022：声明与分派共用同一快照（合并轮由 turn 层强制忽略）
                mcp: mcp_snapshot.as_deref(),
                // 021：每用户轮固定能力快照（声明与执行共用）
                send_message: catalog::supports_send_message_to_user(&model),
                steer: Some(&mut steer_channel),
            };
            let result = turn::run_user_turn_with_context_and_caps(
                &mut history.lock().unwrap(),
                &submission.text,
                &mut sender,
                &mut notify,
                Some(&HOST_CANCEL),
                &mut on_record,
                mode,
                &context_sources,
                &procs,
                Some(&mut input_host),
                Some(&mut async_input_host),
                // 019：每用户轮固定能力快照（声明与执行共用）
                catalog::supports_image_input(&model),
                // 020：用户附件载荷（切片4 接线 SubmittedTurn 后携带）
                user_payload.as_ref(),
                extras,
            );
            // 021：轮收尾兜底封口（循环各出口已封口，此处幂等）；异常残留输入
            // 交还驱动恢复，不丢弃
            let late = steer_channel.close();
            if !late.is_empty() {
                let _ = tx.send(DriverEvent::SteerRestored {
                    inputs: late.into_iter().map(steer_submission_of).collect(),
                    reason: "轮已结束".to_string(),
                });
            }
            unsafe {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
            }
            match result {
                Ok(reply) => {
                    // J6（008-REVIEW 修订）：整段回复经 Delta 按行合成流式视觉；Final 携带
                    // 完整原文——驱动以 Final 替换本轮流式 cell 为单一 Markdown cell
                    //（跨行 markdown/代码块不破碎）。
                    let mut rest = reply.as_str();
                    while let Some(pos) = rest.find('\n') {
                        let (line, tail) = rest.split_at(pos);
                        let _ = tx.send(DriverEvent::Delta(format!("{line}\n")));
                        rest = &tail[1..];
                    }
                    let _ = tx.send(DriverEvent::Final(reply.clone()));
                    // 013-M8：用户轮完成位点——引用反馈 best-effort（先渲染后
                    // 计数；关闭态/解析/开库/计数失败均静默，不打断回复）。
                    crate::memory::feedback::after_user_turn(&mem_user_dir, &mem_config, &reply);
                }
                Err(e) => {
                    // 016：取消当前轮 ≠ 终止进程——Failed 消息明确区分“停止 AI
                    // 工作”与“已启动进程仍由会话管理”，并列出仍有效的编号
                    //（以 turn 常量为事实来源，不反解析消息文本）。
                    let mut msg = e;
                    if msg.starts_with(turn::CANCELLED_TURN_ERROR_PREFIX) {
                        msg.push('\n');
                        msg.push_str(
                            procs.live_sessions_note().as_deref().unwrap_or(
                                "已停止当前工作；本轮没有仍在运行的进程会话。",
                            ),
                        );
                    }
                    let _ = tx.send(DriverEvent::Failed(msg));
                }
            }
            }));
            if body.is_err() {
                // 独立复审 P2-2：panic 可能落在 SIGINT 处理器已装未恢复的区间——
                // 复位默认，避免进程级处理器残留为"仅置取消标志"
                unsafe {
                    libc::signal(libc::SIGINT, libc::SIG_DFL);
                }
                let _ = tx_panic.send(DriverEvent::Failed(
                    "内部错误：会话线程异常终止，本轮已中止".to_string(),
                ));
            }
        });
    }

    /// 斜杠命令（TUI 内）：与行式路径同一语义/实现，输出改为多行文本。
    /// 斜杠命令（/exit 已由驱动内建；其余与行式路径同一语义）。
    /// /resume 成功时返回 [`CommandOutcome::ReplaceCells`]——驱动据此把投影 cells
    /// 立即渲染进 transcript（装载历史不重放工具，007）。
    /// 022：`/mcp` 状态与 `/mcp reload`。reload 只在空闲时执行（接续槽位
    /// Open＝当前轮运行中→明确返回忙）；管理器 runtime 的 block_on 不得在
    /// TUI runtime 上下文内执行，经独立线程桥接后取回结果。
    fn cmd_mcp(&self, args: &str) -> String {
        let arg = args.trim();
        if arg == "reload" {
            let busy = matches!(
                &*self.steer_slot.lock().unwrap_or_else(|e| e.into_inner()),
                SteerSlot::Open { .. }
            );
            if busy {
                return "MCP reload：当前轮运行中，请稍后再试".to_string();
            }
            let mcp = self.mcp.clone();
            let result = std::thread::spawn(move || mcp.reload()).join();
            match result {
                Ok(Ok(())) => "MCP：已重新加载配置并重建连接".to_string(),
                Ok(Err(e)) => format!("MCP：{e}"),
                Err(_) => "MCP：reload 线程异常".to_string(),
            }
        } else if arg.is_empty() {
            self.mcp.status_lines().join("\n")
        } else {
            "用法：/mcp [reload]".to_string()
        }
    }

    pub fn run_command(&self, full: &str) -> tchat_tui::chat_driver::CommandOutcome {
        let raw = full.split_whitespace().next().unwrap_or("");
        // 008-ADAPT：驱动经母本通道回传的命令名不带前导斜杠——规范化
        let name = if raw.starts_with('/') { raw.to_string() } else { format!("/{raw}") };
        let name = name.as_str();
        let args = full.trim_start_matches(|c: char| !c.is_whitespace()).trim();
        match name {
            "/models" => Outcome::Output(self.cmd_models()),
            "/sessions" => Outcome::Output(self.cmd_sessions()),
            "/refresh" => Outcome::Output(self.cmd_refresh()),
            "/save" => Outcome::Output(self.cmd_save(args)),
            "/resume" => self.cmd_resume(args),
            // 013-M7 D9：状态观察（只读）与显式确认重置
            "/memory-status" => Outcome::Output(crate::memory::runner::memory_status_text(
                &catalog::user_dir(),
            )),
            "/memory-reset" => {
                Outcome::Output(crate::memory::runner::memory_reset(
                    &catalog::user_dir(),
                    args,
                ))
            }
            "/mcp" => Outcome::Output(self.cmd_mcp(args)),
            other => Outcome::Output(format!(
                "未知命令 {other}，可用：/models、/refresh、/save <名称>、/resume [名称]、/sessions、/memory-status、/memory-reset confirm、/mcp [reload]"
            )),
        }
    }

    fn cmd_models(&self) -> String {
        let Ok(dict) = catalog::build_final() else {
            return "错误：构建配置失败".into();
        };
        let Some(pid) = dict.selected_provider.clone() else {
            return "错误：手动配置未设置 selected_provider".into();
        };
        let Some(prov) = dict.providers.get(&pid) else {
            return format!("错误：selected_provider {pid} 不在最终字典中");
        };
        let mut out = format!("厂商 {pid}（基础地址 {}）有效模型：", prov.conn.base_url);
        if prov.models.is_empty() {
            out.push_str("\n  （名单为空）");
            return out;
        }
        for id in prov.models.keys() {
            let sel = if dict.selected_model.as_deref() == Some(id.as_str()) { " *" } else { "" };
            out.push_str(&format!("\n  {id}{sel}"));
        }
        out
    }

    fn cmd_sessions(&self) -> String {
        let list = session::list_sessions(&session::sessions_dir());
        if list.is_empty() {
            return "（暂无会话）".into();
        }
        let mut out = String::new();
        for (name, bytes) in list {
            out.push_str(&format!("{name}  {bytes} 字节\n"));
        }
        out.trim_end().to_string()
    }

    fn cmd_refresh(&self) -> String {
        // 会话中新 origin 授权不可交互（TUI 持有终端）→ 非交互路径执行
        let agent = self.agent.clone();
        let mut deny_origins = |_: &str, _: &str, _: &str| _deny_new_origin();
        let mut deny_catalog = |_: &str| _deny_new_catalog_origin();
        match catalog::refresh(&agent, &mut deny_origins, &mut deny_catalog, None) {
            Ok(report) => {
                let mut out = String::new();
                match &report.public_error {
                    Some(e) => out.push_str(&format!("公共目录同步失败，沿用基础资料：{e}\n")),
                    None if report.public_updated.is_empty() => out.push_str("公共目录：无更新\n"),
                    None => out.push_str(&format!(
                        "公共目录已更新基础资料：{}\n",
                        report.public_updated.join("、")
                    )),
                }
                for (id, n) in &report.queried {
                    out.push_str(&format!("{id}：查询到 {n} 个模型，名单已替换\n"));
                }
                for (id, reason) in &report.failed {
                    out.push_str(&format!("{id}：刷新失败（{reason}），保留旧名单\n"));
                }
                for (id, reason) in &report.skipped {
                    out.push_str(&format!("{id}：跳过查询（{reason}）\n"));
                }
                out.push_str("刷新完成。所选模型不变。");
                out
            }
            Err(e) => format!("错误：{e}"),
        }
    }

    fn cmd_save(&self, name: &str) -> String {
        if name.is_empty() {
            return "用法：/save <名称>（1–64 个字母数字._-，不以 auto- 开头）".into();
        }
        let dir = session::sessions_dir();
        if let Err(e) = session::validate_snapshot_name(name) {
            return format!("错误：{e}");
        }
        if dir.join(format!("{name}.jsonl")).exists() {
            // TUI 内覆盖确认不可交互：拒绝覆盖（登记；非交互契约同 007）
            return format!("快照 {name} 已存在：TUI 内不交互覆盖，请在行式模式覆盖或换名");
        }
        // 008 P2-10：键盘路径在 composer 层即被拒（available_during_task=false，
        // 母本行为）；此处 try_lock 为纵深防御——任何绕过 composer 的直接派发
        //（如未来重放通道）也不得等 history 锁冻结 UI
        let history = match self.history.try_lock() {
            Ok(g) => g,
            // 毒锁＝turn 线程 panic 残留（非忙）：恢复数据继续保存（P2-1）
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return "对话进行中，/save 已跳过，请在本轮结束后重试".into();
            }
        };
        let history = history.clone();
        match session::save_snapshot(&dir, name, &history) {
            Ok(n) => format!("已保存快照 {name}（{n} 条记录）"),
            Err(e) => format!("错误：{e}"),
        }
    }

    fn cmd_resume(&self, name: &str) -> tchat_tui::chat_driver::CommandOutcome {
        use tchat_tui::chat_driver::CommandOutcome;
        let target = if name.is_empty() { None } else { Some(name) };
        // 008-REVIEW 死锁修复：turn 线程持 history 锁期间经 on_record 请求 autosave 锁；
        // 此处不得反向嵌套（autosave→history ABBA）。先无嵌套快照判空，再独占 autosave。
        // 008 P2-10：try_lock 让位（纵深防御，键盘路径由 composer 门先拒）
        let history_guard = match self.history.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return CommandOutcome::Output(
                    "对话进行中，/resume 已跳过，请在本轮结束后重试".into(),
                );
            }
        };
        if !history_guard.is_empty() {
            return CommandOutcome::Output(
                "当前历史非空：TUI 内不交互丢弃，请以空会话 /resume".into(),
            );
        }
        drop(history_guard);
        // 独立复审 P1-1：先在 autosave 锁内完成 resume 并取结果，释放后再写
        // history——不持 autosave 嵌套 history（与 turn 线程 history→autosave
        // 反向，构成 ABBA 窗口）
        let resume_result = {
            let mut autosave = self.autosave.lock().unwrap_or_else(|e| e.into_inner());
            autosave.resume(target).map(|info| {
                let summary = if info.degraded {
                    format!("{}\n该会话正被另一进程编辑，已只读装载。", info.loaded.summary())
                } else {
                    info.loaded.summary()
                };
                (summary, info.loaded.history)
            })
        };
        match resume_result {
            Ok((summary, loaded)) => {
                *self.history.lock().unwrap_or_else(|e| e.into_inner()) = loaded;
                // 016：对话切换成功——关闭旧对话全部进程会话，新对话从空管理器开始。
                self.procs.reset();
                // 投影历史为 cells 交驱动渲染（不重放工具）
                let cells = self.resumed_projection();
                CommandOutcome::ReplaceCells { output: summary, cells }
            }
            Err(e) => CommandOutcome::Output(format!("错误：{e}")),
        }
    }

    /// 供驱动取走 /resume 后的投影 cells。
    /// 毒锁感知（定点复核 P2-新1）：run_command 在驱动主线程同步执行——此处裸
    /// unwrap 会以码 101 中止进程并跳过终端恢复；恢复数据继续。
    /// 登记备注（定点复核 P2-新2）：/resume 已由 composer available_during_task
    /// 门拦截；假想的未来直发通道最坏情形为 :369 阻塞至当前轮结束＋记录一致性
    /// 瑕疵，属纵深防御既定姿态。
    pub fn resumed_projection(&self) -> Vec<tchat_tui::chat_driver::HostCell> {
        let history = self
            .history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        tchat_tui::chat_driver::project_history(project_history_items(&history))
    }
}

/// 历史投影（012）：文本直映；工具结果按 id→（name, arguments）配对后，经与
/// 实时 ToolEnd **同一转换**（[`tool_end_item`]）产出 HostHistoryItem——恢复视图与
/// 实时终态同源同投影（需求⑤）。调用参数不重放（007：恢复只投影不执行）。
/// 018-B：带来源标记的回答/关闭文本（正文与标记 request_id 一致）合并进对应
/// 异步发布调用的同一展示，不作为普通用户文本重复出现；孤儿/重复/矛盾记录
/// 不伪造已答，按普通文本保留。
fn project_history_items(history: &[HistoryItem]) -> Vec<HostHistoryItem> {
    use std::collections::HashMap;
    // 018-B 预扫：request_id → 首条有效 answer/closed 事件
    use crate::prompt::HistoryMessage as HM;
    enum AsyncHistoryEvent {
        Answer(crate::user_input::Response),
        Closed(String),
    }
    let mut async_events: HashMap<String, AsyncHistoryEvent> = HashMap::new();
    for item in history {
        let HistoryItem::Text(HM {
            user_input_event: Some(event),
            content,
            ..
        }) = item
        else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        let Some(content_id) = v.get("request_id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if content_id != event.request_id() {
            continue; // 标记与正文矛盾：不合并，按普通文本保留
        }
        match event {
            crate::prompt::UserInputEvent::Answer { request_id } => {
                if let Ok(response) =
                    serde_json::from_value::<crate::user_input::Response>(v.clone())
                {
                    async_events
                        .entry(request_id.clone())
                        .or_insert(AsyncHistoryEvent::Answer(response));
                }
            }
            crate::prompt::UserInputEvent::Closed { request_id } => {
                let reason = v
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                async_events
                    .entry(request_id.clone())
                    .or_insert(AsyncHistoryEvent::Closed(reason));
            }
        }
    }
    let mut pending: HashMap<String, (String, String)> = HashMap::new();
    // 018-B：成功合并进发布 cell 的事件文本不再作为普通用户文本重复展示；
    // 未合并（孤儿/矛盾/外来题号）的事件文本按普通文本保留
    let mut merged_events: HashMap<String, ()> = HashMap::new();
    let mut out = Vec::new();
    for item in history {
        let item = item.portable();
        match item {
            HistoryItem::NativeAssistant { .. } => {}, // portable 已解包，不展示原生块
            HistoryItem::Text(m) => {
                // 015：自动上下文消息不作为真实用户发言展示（仅投影层过滤，
                // 历史与请求面不变；识别与去重共用同一识别器）
                if m.role == crate::prompt::HistoryRole::User
                    && crate::context::context_body(&m.content).is_some()
                {
                    continue;
                }
                if m.user_input_event
                    .as_ref()
                    .is_some_and(|e| merged_events.contains_key(e.request_id()))
                {
                    continue;
                }
                out.push(HostHistoryItem::Text {
                    assistant: m.role == crate::prompt::HistoryRole::Assistant,
                    content: m.content.clone(),
                });
            }
            HistoryItem::AssistantToolCalls { tool_calls, .. } => {
                for call in tool_calls {
                    pending.insert(call.id.clone(), (call.name.clone(), call.arguments.clone()));
                }
            }
            HistoryItem::ToolResult { tool_call_id, content, display, image } => {
                // 无配对或未知工具：按普通文本投影结果内容（不丢失历史）
                let paired = pending.remove(tool_call_id);
                // 016：恢复历史中的 running 只表示当时状态——不作为当前活会话
                // 展示（/resume 已重置管理器，旧句柄失效），按事实文本呈现。
                let async_event = async_events.get(tool_call_id).map(|e| match e {
                    AsyncHistoryEvent::Answer(response) => (Some(response.clone()), None),
                    AsyncHistoryEvent::Closed(reason) => (None, Some(reason.clone())),
                });
                let is_send_message = paired
                    .as_ref()
                    .is_some_and(|(name, _)| name == "send_message_to_user_async");
                let restored = paired.and_then(|(name, arguments)| {
                    if name == "request_user_input_async" {
                        restored_async_item(tool_call_id, &arguments, content, async_event)
                            .map(|(item, merged)| {
                                if merged {
                                    merged_events.insert(tool_call_id.clone(), ());
                                }
                                item
                            })
                    } else if result_was_running(content) {
                        restored_running_text(&name, &arguments, content, display.as_ref())
                    } else {
                        tool_end_item(&name, tool_call_id, &arguments, content, display.as_ref(), false)
                    }
                });
                // 019 §5.1：成功摘要但无图片载荷＝旧版本保存的记录——明确显示
                // 历史图片内容不可用，不当成已附图。
                let restored = restored.map(|mut item| {
                    if image.is_none() {
                        if let HostHistoryItem::ViewImage { ok: true, error, .. } = &mut item {
                            *error = Some(
                                "历史图片内容不可用（旧版本保存的记录，图片未随会话保存）"
                                    .to_string(),
                            );
                        }
                    }
                    item
                });
                // 021：消息工具失败/取消/结果未知——按事实文本呈现（与实时失败
                // 提示同一文案），不伪造成功、不显示原始结果 JSON；其余保持通用降级
                let item = restored.unwrap_or_else(|| {
                    if is_send_message {
                        HostHistoryItem::Text {
                            assistant: true,
                            content: crate::message_to_user::failure_line(content),
                        }
                    } else {
                        HostHistoryItem::Text {
                            assistant: true,
                            content: content.clone(),
                        }
                    }
                });
                out.push(item);
            }
        }
    }
    out
}

/// 018-B：恢复投影中的异步问询终态——发布确认＋来源标记事件合并为同一展示；
/// 只有确认而无回答/关闭记录＝中断/未保存回答、已失效（§8.6），不假定已答。
fn restored_async_item(
    request_id: &str,
    arguments: &str,
    result: &str,
    event: Option<(Option<crate::user_input::Response>, Option<String>)>,
) -> Option<(HostHistoryItem, bool)> {
    let questions = crate::user_input_async::parse_arguments(arguments, request_id).ok()?;
    let v: serde_json::Value = serde_json::from_str(result).ok()?;
    if v.get("accepted").and_then(serde_json::Value::as_bool) != Some(true) {
        return None; // 发布失败：按普通文本呈现，不伪造问题
    }
    let mut merged = false;
    let (response, closed_reason) = match event {
        // 答案 ID 只能属于原题（§8.6）：含外来题号的记录不合并、不伪造已答
        Some((Some(response), _))
            if response
                .answers
                .keys()
                .all(|key| questions.iter().any(|q| &q.id == key)) =>
        {
            merged = true;
            (Some(crate::user_input::sanitized_response(&response)), None)
        }
        Some((None, Some(reason))) => {
            merged = true;
            (None, Some(reason))
        }
        // 无事件/正文畸形/答案不属于原题＝未保存回答的中断记录（事件文本按普通文本保留）
        _ => (None, Some("中断/未保存回答，已失效".to_string())),
    };
    Some((
        HostHistoryItem::UserInputAsync {
            questions: crate::user_input::sanitized_questions(&questions),
            response,
            closed_reason,
            answerable: false,
        },
        merged,
    ))
}

/// 从工具 arguments JSON 提取 exec 的 cmd（Begin 与 End 共用）。
fn arg_cmd(arguments: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v.get("cmd").and_then(|c| c.as_str()).map(str::to_string))
}

/// 012：ToolBegin → 驱动工具事件（exec＝实际命令 arguments.cmd；patch＝标记）。
/// exec 参数不可解析/无 cmd 或未知工具返回 None（调用方回退原活动行）。
fn tool_begin_event(
    name: &str,
    id: &str,
    arguments: &str,
) -> Option<DriverEvent> {
    match name {
        "exec_command" => {
            let command = arg_cmd(arguments)?;
            if command.trim().is_empty() {
                return None;
            }
            Some(DriverEvent::ToolBegin {
                id: id.to_string(),
                kind: ToolBeginKind::Exec { command },
            })
        }
        "apply_patch" => Some(DriverEvent::ToolBegin {
            id: id.to_string(),
            kind: ToolBeginKind::Patch,
        }),
        _ => None,
    }
}

/// 016：write_stdin 的 Begin——关联原命令（取自会话记录），不伪造一条新命令；
/// 句柄未知或参数不可解析返回 None（回退原活动行）。
fn stdin_begin_event(
    procs: &ProcessManager,
    id: &str,
    arguments: &str,
) -> Option<DriverEvent> {
    let sid = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| {
            v.get("session_id")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })?;
    let command = procs.session_command(&sid)?;
    Some(DriverEvent::ToolBegin {
        id: id.to_string(),
        kind: ToolBeginKind::Exec { command },
    })
}

/// 016：结果 JSON 的 status 是否为 running（恢复投影识别“当时仍在运行”）。
fn result_was_running(result: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(|s| s == "running"))
        .unwrap_or(false)
}

/// 016：恢复历史中“当时仍在运行”的记录——按事实文本呈现（命令关联＋已收集
/// 输出＋明确“恢复后不再受管理”），不渲染为可交互的活会话 cell。
fn restored_running_text(
    name: &str,
    arguments: &str,
    result: &str,
    display: Option<&ToolDisplay>,
) -> Option<HostHistoryItem> {
    let v: serde_json::Value = serde_json::from_str(result).ok()?;
    let command = if name == "write_stdin" {
        match display {
            Some(ToolDisplay::Stdin { command, .. }) => command.clone(),
            _ => String::new(),
        }
    } else {
        arg_cmd(arguments).unwrap_or_default()
    };
    let sid = v.get("session_id").and_then(|s| s.as_str()).unwrap_or("?");
    let output = v.get("output").and_then(|o| o.as_str()).unwrap_or_default();
    let mut content = if command.trim().is_empty() {
        format!("{name} 记录时仍在运行（会话 {sid}；恢复后进程不再受本会话管理）。")
    } else {
        format!("{name}（{command}）记录时仍在运行（会话 {sid}；恢复后进程不再受本会话管理）。")
    };
    if !output.is_empty() {
        content.push_str("已收集输出：\n");
        content.push_str(output);
    }
    Some(HostHistoryItem::Text { assistant: true, content })
}

/// 012：工具终态 → HostHistoryItem（实时 ToolEnd 与 /resume 投影共用）。
/// 取消判定：exec＝结果 JSON `status=="cancelled"`（等待被取消，进程可能仍在，
/// 016 起不再等同“已终止”）；patch＝事件旗标（实时）或
/// [`turn::SKIPPED_BY_CANCEL_MESSAGE`] 精确匹配（恢复路径——单一生产者常量）。
/// 016：write_stdin 复用 Exec 形状——`command`＝关联原命令（优先取展示数据，
/// 旧记录缺失时留空降级），运行中携带会话编号（running）。
/// 018-B：`request_user_input_async`＝发布确认的实时投影（answerable=true，
/// 显示 F6 提示）；回答/关闭经 AsyncInputUpdate 更新同一 cell。
/// 解析失败返回 None（调用方按普通文本处理）。
fn tool_end_item(
    name: &str,
    id: &str,
    arguments: &str,
    result: &str,
    display: Option<&ToolDisplay>,
    cancelled_event: bool,
) -> Option<HostHistoryItem> {
    let v: serde_json::Value = serde_json::from_str(result).ok()?;
    match name {
        "exec_command" | "write_stdin" => {
            let (command, duration_ms) = if name == "write_stdin" {
                match display {
                    Some(ToolDisplay::Stdin { command, duration_ms, .. }) => {
                        (command.clone(), Some(*duration_ms))
                    }
                    _ => (String::new(), None),
                }
            } else {
                (
                    arg_cmd(arguments).unwrap_or_default(),
                    match display {
                        Some(ToolDisplay::Exec { duration_ms }) => Some(*duration_ms),
                        _ => None,
                    },
                )
            };
            let running = v
                .get("status")
                .and_then(|s| s.as_str())
                .filter(|s| *s == "running")
                .and_then(|_| v.get("session_id"))
                .and_then(|s| s.as_str())
                .map(str::to_string);
            Some(HostHistoryItem::Exec {
                command,
                output: v
                    .get("output")
                    .and_then(|o| o.as_str())
                    .unwrap_or_default()
                    .to_string(),
                exit_code: v
                    .get("exit_code")
                    .and_then(|c| c.as_i64())
                    .map(|c| c as i32),
                // 012：取消＝独立呈现（"已取消"），不机械映射"拒绝"
                cancelled: v
                    .get("status")
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| s == "cancelled"),
                error: v.get("error").and_then(|e| e.as_str()).map(str::to_string),
                duration_ms,
                running,
            })
        }
        "apply_patch" => {
            let changed = v
                .get("changed_files")
                .and_then(|f| f.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.get("path").and_then(|p| p.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let changes = match display {
                Some(ToolDisplay::Patch { changes }) => changes
                    .iter()
                    .map(|c| HostPatchChange {
                        path: c.path.clone(),
                        kind: match &c.kind {
                            crate::prompt::PatchChangeKind::Add { content } => {
                                HostPatchKind::Add { content: content.clone() }
                            }
                            crate::prompt::PatchChangeKind::Delete { content } => {
                                HostPatchKind::Delete { content: content.clone() }
                            }
                            crate::prompt::PatchChangeKind::Update { diff } => {
                                HostPatchKind::Update { diff: diff.clone() }
                            }
                        },
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let error = v.get("error").and_then(|e| e.as_str()).map(str::to_string);
            let cancelled = cancelled_event
                || error.as_deref() == Some(turn::SKIPPED_BY_CANCEL_MESSAGE);
            Some(HostHistoryItem::Patch {
                ok: v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
                cancelled,
                state_known: v
                    .get("state_known")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(true),
                error,
                changed,
                changes,
            })
        }
        // 017：计划终态——成功快照（结果明确成功且参数可解析）经终端过滤后
        // 交给 TUI 计划 cell；失败/取消只带原因；结果未知（非 JSON）或「成功但
        // 参数损坏」返回 None（调用方按普通文本降级，不推断成功）。
        "update_plan" => {
            if let Some(snapshot) = crate::plan::success_snapshot(arguments, result) {
                return Some(HostHistoryItem::Plan {
                    update: Some(crate::plan::sanitized_for_display(&snapshot)),
                    cancelled: false,
                    error: None,
                });
            }
            crate::plan::result_error(result)
                .map(|error| {
                    // 展示副本统一过终端过滤（serde 错误消息可内嵌模型可控的
                    // 字段/变体名——错误文本同样属不可信来源；CLI 侧经 say 过滤）
                    let error = crate::display::sanitize_for_terminal(&error);
                    let cancelled =
                        cancelled_event || error.as_str() == turn::SKIPPED_BY_CANCEL_MESSAGE;
                    HostHistoryItem::Plan {
                        update: None,
                        cancelled,
                        error: Some(error),
                    }
                })
        }
        // 019：查看图片终态——成功（结果 ok=true）转 ViewImage 卡片输入；失败/取消
        // 带原因。路径与错误经终端过滤（只过滤显示副本）。
        "view_image" => {
            let ok = v.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false);
            let path = v
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = crate::display::sanitize_for_terminal(path).to_string();
            if ok {
                let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
                let dims = match (
                    v.get("width").and_then(serde_json::Value::as_u64),
                    v.get("height").and_then(serde_json::Value::as_u64),
                ) {
                    (Some(w), Some(h)) => Some((w, h)),
                    _ => None,
                };
                Some(HostHistoryItem::ViewImage {
                    ok: true,
                    filename,
                    path,
                    dims,
                    error: None,
                })
            } else {
                let error = v
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(|e| {
                        if e == turn::SKIPPED_BY_CANCEL_MESSAGE {
                            e.to_string()
                        } else {
                            crate::display::sanitize_for_terminal(e).to_string()
                        }
                    })
                    .or_else(|| {
                        cancelled_event
                            .then(|| turn::SKIPPED_BY_CANCEL_MESSAGE.to_string())
                    });
                Some(HostHistoryItem::ViewImage {
                    ok: false,
                    filename: String::new(),
                    path,
                    dims: None,
                    error,
                })
            }
        }
        "request_user_input" => {
            let questions = crate::user_input::parse_arguments(arguments).ok()?;
            let response = serde_json::from_str::<crate::user_input::Response>(result)
                .ok()
                .filter(|r| crate::user_input::validate_response(&questions, r).is_ok());
            let error = v.get("error").and_then(|e| e.as_str())
                .map(crate::display::sanitize_for_terminal);
            if response.is_none() && error.is_none() { return None; }
            Some(HostHistoryItem::UserInput {
                questions: crate::user_input::sanitized_questions(&questions),
                response: response.as_ref().map(crate::user_input::sanitized_response),
                error,
            })
        }
        // 018-B：发布确认实时投影——仅 accepted=true 生成问询 cell（等待回答）；
        // 发布失败按普通文本呈现错误，不伪造问题。
        "request_user_input_async" => {
            let questions = crate::user_input_async::parse_arguments(arguments, id).ok()?;
            if v.get("accepted").and_then(serde_json::Value::as_bool) != Some(true) {
                return None;
            }
            Some(HostHistoryItem::UserInputAsync {
                questions: crate::user_input::sanitized_questions(&questions),
                response: None,
                closed_reason: None,
                answerable: true,
            })
        }
        // 021：消息成功结果→AssistantMessage（实时终态与 /resume 恢复同一转换；
        // 只展示不重发）。失败/取消/结果未知返回 None——实时路径显式呈现原因，
        // 恢复路径按普通文本降级，不推定发送成功。
        "send_message_to_user_async" => crate::message_to_user::sent_message_from_record(
            arguments,
            result,
        )
        .map(|message| HostHistoryItem::AssistantMessage {
            // §2.1：展示副本过终端过滤；调用参数与模型历史保留原值
            content: crate::display::sanitize_for_terminal(&message),
        }),
        _ => None,
    }
}

extern "C" fn on_host_sigint(_sig: libc::c_int) {
    HOST_CANCEL.store(true, Ordering::SeqCst);
}

static HOST_CANCEL: AtomicBool = AtomicBool::new(false);

fn _deny_new_origin() -> Result<(), String> {
    Err("TUI 会话中新增 origin 授权不可用（终端被 TUI 持有）；请于行式模式授权".to_string())
}

fn _deny_new_catalog_origin() -> Result<(), String> {
    Err("TUI 会话中新增目录源授权不可用；请于行式模式授权".to_string())
}

#[cfg(test)]
mod tests {
    /// 019：view_image 终态转换——成功带文件名/路径/尺寸；失败带原因（已过滤）；
    /// 恢复路径的成功摘要无载荷时标注历史图片内容不可用。
    #[test]
    fn tool_end_item_parses_view_image_results() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let ok_result = r#"{"ok":true,"path":"/w/a b.png","source_width":2304,"source_height":864,"width":2048,"height":768}"#;
        match tool_end_item("view_image", "c1", r#"{"path":"a b.png"}"#, ok_result, None, false) {
            Some(H::ViewImage { ok, filename, path, dims, error }) => {
                assert!(ok);
                assert_eq!(filename, "a b.png");
                assert_eq!(path, "/w/a b.png");
                assert_eq!(dims, Some((2048, 768)));
                assert!(error.is_none());
            }
            other => panic!("形状：{other:?}"),
        }
        // 失败：原因进入 error
        match tool_end_item(
            "view_image",
            "c2",
            r#"{"path":"x"}"#,
            r#"{"ok":false,"error":"文件不存在"}"#,
            None,
            false,
        ) {
            Some(H::ViewImage { ok, error, .. }) => {
                assert!(!ok);
                assert_eq!(error.as_deref(), Some("文件不存在"));
            }
            other => panic!("形状：{other:?}"),
        }
        // 恢复投影：成功摘要无载荷 → 历史图片内容不可用（project_history_items
        // 的后处理，见下方投影测试）
        let history = vec![
            HistoryItem::AssistantToolCalls {
                tool_calls: vec![crate::prompt::ToolCallRecord {
                    id: "c3".into(),
                    name: "view_image".into(),
                    arguments: r#"{"path":"a.png"}"#.into(),
                }],
                content: None,
            },
            HistoryItem::ToolResult {
                tool_call_id: "c3".into(),
                content: ok_result.into(),
                display: None,
                image: None,
            },
        ];
        let items = project_history_items(&history);
        match &items[0] {
            H::ViewImage { ok, error, .. } => {
                assert!(ok, "仍按成功呈现");
                assert!(
                    error.as_deref().unwrap_or_default().contains("历史图片内容不可用"),
                    "缺图说明：{error:?}"
                );
            }
            other => panic!("形状：{other:?}"),
        }
        // 有载荷的恢复项：无缺图说明
        let with_image = HistoryItem::ToolResult {
            tool_call_id: "c3".into(),
            content: ok_result.into(),
            display: None,
            image: Some(crate::prompt::ToolResultImage {
                image_url: "data:image/png;base64,QUJD".into(),
                detail: "high".into(),
            }),
        };
        let items = project_history_items(&[
            HistoryItem::AssistantToolCalls {
                tool_calls: vec![crate::prompt::ToolCallRecord {
                    id: "c3".into(),
                    name: "view_image".into(),
                    arguments: r#"{"path":"a.png"}"#.into(),
                }],
                content: None,
            },
            with_image,
        ]);
        match &items[0] {
            H::ViewImage { ok, error, .. } => {
                assert!(ok);
                assert!(error.is_none(), "有载荷不标缺图：{error:?}");
            }
            other => panic!("形状：{other:?}"),
        }
    }

    /// 012：终态转换——结果 JSON＋arguments＋display → HostHistoryItem。
    /// 覆盖两处旧缺陷的回归：exec 成功判定（原 status=="success" 恒 false）与
    /// patch changed_files 对象数组的路径提取（原按字符串提取恒空）。
    #[test]
    fn tool_end_item_parses_exec_and_patch_results() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        // exec 成功：completed+exit 0（原实现误判 status=="success"→恒失败）
        let exec_args = r#"{"cmd":"echo hi"}"#;
        let exec_result = concat!(
            r#"{"status":"completed","exit_code":0,"output":"hi"#,
            r#"\n","truncated":false,"error":null}"#
        );
        match tool_end_item("exec_command", "", exec_args, exec_result, Some(&ToolDisplay::Exec {
            duration_ms: 42,
        }), false) {
            Some(H::Exec { command, output, exit_code, cancelled, error, duration_ms, running }) => {
                assert!(running.is_none(), "终态无运行会话");
                assert_eq!(command, "echo hi", "命令取自 arguments.cmd（旧会话亦然）");
                assert_eq!(output, "hi\n");
                assert_eq!(exit_code, Some(0));
                assert!(!cancelled);
                assert!(error.is_none());
                assert_eq!(duration_ms, Some(42), "展示时长随事件传递");
            }
            other => panic!("exec 形状：{other:?}"),
        }
        // exec 取消
        let cancelled_result =
            r#"{"status":"cancelled","exit_code":null,"output":"partial","truncated":false,"error":"执行已取消，已终止进程组"}"#;
        match tool_end_item("exec_command", "", exec_args, cancelled_result, None, false) {
            Some(H::Exec { cancelled, .. }) => assert!(cancelled),
            other => panic!("cancelled 形状：{other:?}"),
        }
        // 016：运行中携带会话编号；write_stdin 关联原命令（不伪造新命令）
        let running_result = r#"{"status":"running","exit_code":null,"output":"partial","truncated":false,"error":null,"session_id":"s7"}"#;
        match tool_end_item("exec_command", "", exec_args, running_result, None, false) {
            Some(H::Exec { running, .. }) => assert_eq!(running.as_deref(), Some("s7")),
            other => panic!("running 形状：{other:?}"),
        }
        let stdin_result = r#"{"status":"running","exit_code":null,"output":"问：","truncated":false,"error":null,"session_id":"s7"}"#;
        match tool_end_item(
            "write_stdin",
            "",
            r#"{"session_id":"s7","chars":"y
"}"#,
            stdin_result,
            Some(&ToolDisplay::Stdin { session_id: "s7".into(), command: "read ans".into(), duration_ms: 5 }),
            false,
        ) {
            Some(H::Exec { command, running, duration_ms, .. }) => {
                assert_eq!(command, "read ans", "write_stdin 展示关联原命令");
                assert_eq!(running.as_deref(), Some("s7"));
                assert_eq!(duration_ms, Some(5));
            }
            other => panic!("stdin 形状：{other:?}"),
        }
        // patch：changed_files 为对象数组（{path, action}——原按字符串提取恒空）
        let patch_result = r#"{"ok":true,"changed_files":[{"path":"/w/a.txt","action":"add"}],"state_known":true,"error":null}"#;
        match tool_end_item("apply_patch", "", "{}", patch_result, Some(&ToolDisplay::Patch {
            changes: vec![crate::prompt::PatchChange {
                path: "/w/a.txt".into(),
                kind: crate::prompt::PatchChangeKind::Add { content: "x\n".into() },
            }],
        }), false) {
            Some(H::Patch { ok, changed, changes, .. }) => {
                assert!(ok);
                assert_eq!(changed, vec!["/w/a.txt".to_string()], "路径自对象数组提取");
                assert_eq!(changes.len(), 1, "展示差异自 display 传递");
            }
            other => panic!("patch 形状：{other:?}"),
        }
        // 未知工具/非 JSON → None（调用方按文本投影）
        assert!(tool_end_item("web_search", "", "{}", r#"{"error":"x"}"#, None, false).is_none());
        assert!(tool_end_item("exec_command", "", "{}", "not json", None, false).is_none());
    }

    /// 012：/resume 投影配对——arguments 随 id 配对传递（旧会话命令恢复），
    /// 无配对结果按文本投影不丢失。
    #[test]
    fn project_history_items_pairs_arguments_for_resume() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let history = vec![
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: "go".into(),
                user_input_event: None,
            }),
            HistoryItem::AssistantToolCalls {
                tool_calls: vec![crate::prompt::ToolCallRecord {
                    id: "c1".into(),
                    name: "exec_command".into(),
                    arguments: r#"{"cmd":"ls -la"}"#.into(),
                }],
                content: None,
            },
            HistoryItem::ToolResult {
                image: None,
                tool_call_id: "c1".into(),
                content: r#"{"status":"completed","exit_code":0,"output":"","truncated":false,"error":null}"#.into(),
                display: None,
            },
            HistoryItem::ToolResult {
                image: None,
                tool_call_id: "orphan".into(),
                content: "孤儿结果".into(),
                display: None,
            },
        ];
        let items = project_history_items(&history);
        match &items[1] {
            H::Exec { command, .. } => assert_eq!(command, "ls -la", "旧会话命令自 arguments 恢复"),
            other => panic!("投影形状：{other:?}"),
        }
        match &items[2] {
            H::Text { assistant, content } => {
                assert!(*assistant);
                assert_eq!(content, "孤儿结果");
            }
            other => panic!("孤儿结果按文本：{other:?}"),
        }
    }

    /// 016：恢复历史中的 running 记录——按“当时状态”文本呈现，不渲染为
    /// 当前活会话 cell（/resume 已重置管理器，旧句柄失效）。
    #[test]
    fn project_history_items_renders_running_as_past_state() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let history = vec![
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: "go".into(),
                user_input_event: None,
            }),
            HistoryItem::AssistantToolCalls {
                tool_calls: vec![crate::prompt::ToolCallRecord {
                    id: "r1".into(),
                    name: "exec_command".into(),
                    arguments: r#"{"cmd":"python server.py"}"#.into(),
                }],
                content: None,
            },
            HistoryItem::ToolResult {
                image: None,
                tool_call_id: "r1".into(),
                content: r#"{"status":"running","exit_code":null,"output":"listening","truncated":false,"error":null,"session_id":"s1"}"#.into(),
                display: None,
            },
        ];
        let items = project_history_items(&history);
        match &items[1] {
            H::Text { assistant, content } => {
                assert!(*assistant, "工具记录以事实文本呈现");
                assert!(content.contains("python server.py"), "命令关联：{content}");
                assert!(content.contains("记录时仍在运行"), "当时状态措辞：{content}");
                assert!(content.contains("恢复后进程不再受本会话管理"), "不展示为活会话：{content}");
                assert!(content.contains("listening"), "已收集输出保留：{content}");
            }
            other => panic!("running 记录不得渲染为活会话 cell：{other:?}"),
        }
    }

    /// 015：/resume 投影跳过自动上下文消息——不作为真实用户发言展示
    ///（裸消息与带替换声明两种形态；仅展示层过滤，历史与请求面不变）。
    #[test]
    fn project_history_items_skips_context_messages() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let env_block = "<environment_context>\n  <cwd>/w</cwd>\n  <current_date>2025-10-09</current_date>\n</environment_context>";
        let declared = format!(
            "The following context replaces the previously provided AGENTS.md instructions and environment context.\n\n# AGENTS.md instructions for /w\n\n<INSTRUCTIONS>\n规则\n</INSTRUCTIONS>\n\n{env_block}"
        );
        let history = vec![
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: declared,
                user_input_event: None,
            }),
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: "真实输入".into(),
                user_input_event: None,
            }),
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::Assistant,
                content: "答".into(),
                user_input_event: None,
            }),
        ];
        let items = project_history_items(&history);
        assert_eq!(items.len(), 2, "ctx 不进展示投影");
        match &items[0] {
            H::Text { assistant, content } => {
                assert!(!assistant);
                assert_eq!(content, "真实输入");
            }
            other => panic!("首项应为真实输入：{other:?}"),
        }
        assert!(matches!(&items[1], H::Text { assistant: true, .. }));
        // 环境块裸消息（无规则形态）同样跳过
        let history2 = vec![HistoryItem::Text(crate::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: crate::prompt::HistoryRole::User,
            content: env_block.to_string(),
            user_input_event: None,
        })];
        assert!(project_history_items(&history2).is_empty());
    }

    use super::*;
    use std::time::Duration;

    /// 017：update_plan 终态转换——成功＝快照（展示副本已过终端过滤）；
    /// 失败＝原因；取消（含恢复路径按 SKIPPED 常量识别）＝cancelled；
    /// 「成功但参数损坏」与结果未知＝None（普通文本降级，不推断成功）。
    #[test]
    fn tool_end_item_update_plan_shapes() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        // 说明含 ESC 序列、步骤含 BEL：展示副本过滤，快照仍重建
        let args = "{\"explanation\":\"说\\u001b[2J明\",\"plan\":[{\"step\":\"步\\u0007骤\",\"status\":\"in_progress\"}]}";
        match tool_end_item("update_plan", "", args, r#"{"ok":true,"error":null}"#, None, false) {
            Some(H::Plan { update: Some(u), cancelled, error }) => {
                assert!(!cancelled);
                assert!(error.is_none());
                assert_eq!(u.explanation.as_deref(), Some("说明"), "展示副本已过滤：{u:?}");
                assert_eq!(u.plan[0].step, "步骤");
            }
            other => panic!("成功形状：{other:?}"),
        }
        // 失败：ok=false＋原因；错误文本（可内嵌模型可控变体名）同样过终端过滤
        let esc_error = "unknown variant `\u{1b}[2Jbad`, expected pending";
        match tool_end_item(
            "update_plan",
            "",
            r#"{"plan":[]}"#,
            &serde_json::json!({ "ok": false, "error": esc_error }).to_string(),
            None,
            false,
        ) {
            Some(H::Plan { update: None, cancelled, error }) => {
                assert!(!cancelled);
                assert_eq!(
                    error.as_deref(),
                    Some("unknown variant `bad`, expected pending"),
                    "失败原因展示副本已过滤：{error:?}"
                );
            }
            other => panic!("失败形状：{other:?}"),
        }
        // 取消：恢复路径按结果中的 SKIPPED 常量识别（cancelled_event=false）
        match tool_end_item(
            "update_plan",
            "",
            r#"{"plan":[]}"#,
            &serde_json::json!({ "ok": false, "error": turn::SKIPPED_BY_CANCEL_MESSAGE }).to_string(),
            None,
            false,
        ) {
            Some(H::Plan { update: None, cancelled, .. }) => assert!(cancelled, "SKIPPED 常量识别取消"),
            other => panic!("取消形状：{other:?}"),
        }
        // 实时取消事件旗标（结果形状同上）
        match tool_end_item(
            "update_plan",
            "",
            r#"{"plan":[]}"#,
            r#"{"ok":false,"error":"别的原因"}"#,
            None,
            /*cancelled_event*/ true,
        ) {
            Some(H::Plan { update: None, cancelled, .. }) => assert!(cancelled, "事件旗标优先"),
            other => panic!("事件取消形状：{other:?}"),
        }
        // 成功结果＋参数损坏 → None（文本降级）；结果非 JSON → None
        assert!(tool_end_item("update_plan", "", "corrupt", r#"{"ok":true,"error":null}"#, None, false).is_none());
        assert!(tool_end_item("update_plan", "", args, "not json", None, false).is_none());
        // 成功但缺 error 字段（非明确成功）＋无 error 文本 → None
        assert!(tool_end_item("update_plan", "", args, r#"{"ok":true}"#, None, false).is_none());
    }

    /// 018-B：恢复投影——来源标记把回答/关闭合并进发布调用同一展示；
    /// 孤儿/矛盾/重复记录不伪造已答（按普通用户文本保留）；只有确认而无
    /// 事件＝中断/未保存回答、已失效；旧程序重存丢标记后降级为普通文本。
    #[test]
    fn project_history_items_merges_async_events_by_marker() {
        use crate::prompt::UserInputEvent;
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let args = r#"{"questions":[{"title":"S104-报告面向谁？","options":["内部评审"]}]}"#;
        let publish = |id: &str| HistoryItem::AssistantToolCalls {
            tool_calls: vec![crate::prompt::ToolCallRecord {
                id: id.into(),
                name: "request_user_input_async".into(),
                arguments: args.into(),
            }],
            content: None,
        };
        let accepted = |id: &str| HistoryItem::ToolResult {
            image: None,
            tool_call_id: id.into(),
            content: crate::user_input_async::accepted_result(id),
            display: None,
        };
        // 已答：answer 标记（正文一致）合并进发布 cell，不再作为普通文本重复
        let answer_body = r#"{"request_id":"a1","answers":{"[\"request_user_input_async\",\"a1\",0]":{"answers":["内部评审"]}}}"#;
        let history = vec![
            publish("a1"),
            accepted("a1"),
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: answer_body.into(),
                user_input_event: Some(UserInputEvent::Answer {
                    request_id: "a1".into(),
                }),
            }),
        ];
        let items = project_history_items(&history);
        assert_eq!(items.len(), 1, "回答合并进单一发布 cell：{items:?}");
        match &items[0] {
            H::UserInputAsync { response: Some(r), closed_reason: None, answerable, .. } => {
                assert!(!*answerable, "恢复不重开问询");
                assert!(r.answers.values().any(|a| a.answers == vec!["内部评审".to_string()]));
            }
            other => panic!("已答形状：{other:?}"),
        }
        // 关闭：closed 标记合并（原因透传），cell 呈关闭态
        let history = vec![
            publish("c1"),
            accepted("c1"),
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: r#"{"request_id":"c1","reason":"task_completed"}"#.into(),
                user_input_event: Some(UserInputEvent::Closed {
                    request_id: "c1".into(),
                }),
            }),
        ];
        let items = project_history_items(&history);
        match &items[0] {
            H::UserInputAsync { response: None, closed_reason: Some(reason), .. } => {
                assert_eq!(reason, "task_completed");
            }
            other => panic!("关闭形状：{other:?}"),
        }
        // 只有确认而无事件：中断/未保存回答、已失效（不假定已答/未执行）
        let items = project_history_items(&vec![publish("i1"), accepted("i1")]);
        match &items[0] {
            H::UserInputAsync { response: None, closed_reason: Some(reason), .. } => {
                assert!(reason.contains("已失效"), "{reason}");
            }
            other => panic!("中断形状：{other:?}"),
        }
        // 孤儿标记（无对应发布调用）与矛盾标记（正文 request_id 不一致）：
        // 不合并、按普通文本保留，不伪造已答
        let orphan = HistoryItem::Text(crate::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: crate::prompt::HistoryRole::User,
            content: answer_body.into(),
            user_input_event: Some(UserInputEvent::Answer {
                request_id: "ghost".into(),
            }),
        });
        let mismatch = HistoryItem::Text(crate::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: crate::prompt::HistoryRole::User,
            content: r#"{"request_id":"someone-else","answers":{}}"#.into(),
            user_input_event: Some(UserInputEvent::Answer {
                request_id: "a1".into(),
            }),
        });
        let history = vec![publish("a1"), accepted("a1"), orphan.clone(), mismatch];
        let items = project_history_items(&history);
        match &items[0] {
            H::UserInputAsync { closed_reason: Some(reason), .. } => {
                assert!(reason.contains("已失效"), "矛盾标记不得合并为已答：{reason}");
            }
            other => panic!("矛盾形状：{other:?}"),
        }
        assert!(items.len() >= 2, "孤儿/矛盾按普通文本保留：{items:?}");
        assert!(
            items.iter().any(|i| matches!(i, H::Text { content, .. } if content.contains("ghost")
                || content.contains("someone-else"))),
            "原文不丢失：{items:?}"
        );
        // 旧程序重存丢标记：同一正文按普通用户文本展示（降级不丢内容）
        let degraded = HistoryItem::Text(crate::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: crate::prompt::HistoryRole::User,
            content: answer_body.into(),
            user_input_event: None,
        });
        let items = project_history_items(&vec![publish("a1"), accepted("a1"), degraded]);
        match &items[0] {
            H::UserInputAsync { closed_reason: Some(reason), .. } => {
                assert!(reason.contains("已失效"));
            }
            other => panic!("降级发布形状：{other:?}"),
        }
        assert!(
            items.iter().any(|i| matches!(i, H::Text { assistant: false, content } if content.contains("内部评审"))),
            "丢标记后答案正文仍按用户文本可见：{items:?}"
        );
        // 答案含外来题号（标记一致但 answers 键不属于原题）：不伪造已答
        let foreign_body = r#"{"request_id":"f1","answers":{"other-question":{"answers":["X"]}}}"#;
        let history = vec![
            publish("f1"),
            accepted("f1"),
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: foreign_body.into(),
                user_input_event: Some(UserInputEvent::Answer {
                    request_id: "f1".into(),
                }),
            }),
        ];
        let items = project_history_items(&history);
        match &items[0] {
            H::UserInputAsync { response: None, closed_reason: Some(reason), .. } => {
                assert!(reason.contains("已失效"), "外来题号不合并为已答：{reason}");
            }
            other => panic!("外来题号形状：{other:?}"),
        }
        assert!(
            items.iter().any(|i| matches!(i, H::Text { assistant: false, content } if content.contains("other-question"))),
            "外来答案正文按普通文本保留：{items:?}"
        );
        // 重复 answer 标记：首条合并，第二条按普通文本保留
        let dup = HistoryItem::Text(crate::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: crate::prompt::HistoryRole::User,
            content: answer_body.into(),
            user_input_event: Some(UserInputEvent::Answer {
                request_id: "a1".into(),
            }),
        });
        let history = vec![publish("a1"), accepted("a1"), dup.clone(), dup];
        let items = project_history_items(&history);
        assert!(
            items.iter().filter(|i| matches!(i, H::UserInputAsync { response: Some(_), .. })).count() == 1,
            "重复标记不产生第二个已答 cell：{items:?}"
        );
    }

    /// 017：恢复投影——成功计划调用配对重建快照（与实时同一转换）；失败调用
    /// 不重建为成功；未配对结果按文本投影（不丢失）。
    #[test]
    fn project_history_items_restores_plan_snapshots() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let plan_args = r#"{"explanation":"说明","plan":[{"step":"步骤一","status":"completed"}]}"#;
        let history = vec![
            HistoryItem::Text(crate::prompt::HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: crate::prompt::HistoryRole::User,
                content: "go".into(),
                user_input_event: None,
            }),
            HistoryItem::AssistantToolCalls {
                tool_calls: vec![
                    crate::prompt::ToolCallRecord {
                        id: "plan-ok".into(),
                        name: "update_plan".into(),
                        arguments: plan_args.into(),
                    },
                    crate::prompt::ToolCallRecord {
                        id: "plan-bad".into(),
                        name: "update_plan".into(),
                        arguments: r#"{"plan":[]}"#.into(),
                    },
                ],
                content: None,
            },
            HistoryItem::ToolResult {
                image: None,
                tool_call_id: "plan-ok".into(),
                content: r#"{"ok":true,"error":null}"#.into(),
                display: None,
            },
            HistoryItem::ToolResult {
                image: None,
                tool_call_id: "plan-bad".into(),
                content: r#"{"ok":false,"error":"工具参数不是有效 JSON：oops"}"#.into(),
                display: None,
            },
            // 未配对孤儿结果：按文本投影不丢失
            HistoryItem::ToolResult {
                image: None,
                tool_call_id: "orphan-plan".into(),
                content: r#"{"ok":true,"error":null}"#.into(),
                display: None,
            },
        ];
        let items = project_history_items(&history);
        match &items[1] {
            H::Plan { update: Some(u), .. } => {
                assert_eq!(u.explanation.as_deref(), Some("说明"));
                assert_eq!(u.plan[0].step, "步骤一");
            }
            other => panic!("成功快照重建：{other:?}"),
        }
        match &items[2] {
            H::Plan { update: None, cancelled, error } => {
                assert!(!cancelled);
                assert!(error.as_deref().unwrap().contains("oops"));
            }
            other => panic!("失败不重建成功：{other:?}"),
        }
        match &items[3] {
            H::Text { assistant, content } => {
                assert!(*assistant);
                assert_eq!(content, r#"{"ok":true,"error":null}"#);
            }
            other => panic!("孤儿结果按文本：{other:?}"),
        }
    }

    /// P2-2（008-REVIEW 独立审查）：turn 线程 panic（如毒锁）不得静默死亡——
    /// 必须以 Failed 事件回推，驱动恢复可用（否则 turn_running 永久卡死）。
    /// 修复前：线程 panic 无事件 → recv 超时失败（红）。
    #[test]
    fn turn_thread_panic_surfaces_as_failed_event() {
        let _g = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = std::env::temp_dir().join(format!("tchat-tuihost-{}-cfg", std::process::id()));
        let user = std::env::temp_dir().join(format!("tchat-tuihost-{}-user", std::process::id()));
        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&user);
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::env::set_var("TCHAT_CONFIG_DIR", &cfg);
        std::env::set_var("TCHAT_USER_DIR", &user);
        std::env::set_var("TCHAT_TUIHOST_PANIC_KEY", "k-panic");
        // 守卫在 set_var 后立即构造：中途失败（expect 等）也清理 env（复核建议）
        struct EnvRestore;
        impl Drop for EnvRestore {
            fn drop(&mut self) {
                unsafe { std::env::remove_var("TCHAT_CONFIG_DIR") };
                unsafe { std::env::remove_var("TCHAT_USER_DIR") };
                unsafe { std::env::remove_var("TCHAT_TUIHOST_PANIC_KEY") };
            }
        }
        let _restore = EnvRestore;

        let m = catalog::ManualRoot {
            selected_provider: Some("deepseek".into()),
            selected_model: Some("deepseek-flash".into()),
            memories: None,
            providers: [(
                "deepseek".to_string(),
                catalog::ManualProvider {
                    base_url: Some("http://127.0.0.1:9".into()),
                    key_ref: Some("env:TCHAT_TUIHOST_PANIC_KEY".into()),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        };
        std::fs::write(cfg.join("manual.json"), serde_json::to_string(&m).unwrap()).unwrap();

        let host = TuiHost::new(crate::api::agent(), session::Session::new(user.join("sessions")));
        // 预授权匹配（越过 003 门，到达 history 锁点位）
        let origin = catalog::origin_of("http://127.0.0.1:9").unwrap();
        host.set_authorized(origin, catalog::fingerprint("k-panic"));

        // 毒化 history 锁：持锁 panic（catch_unwind 捕获，锁保持毒化）
        let shared = host.history.clone();
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = shared.lock().unwrap();
            panic!("poison");
        }));
        std::panic::set_hook(prev_hook);

        let (tx, rx) = std_mpsc::channel();
        host.spawn_turn(
            tchat_tui::chat_driver::SubmittedTurn {
                text: "hello".into(),
                local_images: Vec::new(),
                text_elements: Vec::new(),
            },
            tx,
        );
        let event = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("turn 线程异常死亡也必须回推终止事件（不得静默挂起驱动）");
        assert!(
            matches!(event, DriverEvent::Failed(ref msg) if msg.contains("内部错误")),
            "panic 应映射为 Failed（内部错误），实际：{event:?}"
        );
        // 毒锁善后（独立复审 P2-1）：退出排空在毒锁下恢复数据完成而非 panic
        //（否则无 再见!、退出码 101）
        host.flush_final_session()
            .expect("退出排空在毒锁下必须完成（数据恢复而非 panic）");
        // 定点复核 P2-新1：毒锁下 /resume 也不得 panic（resumed_projection 曾裸
        // unwrap——主线程 panic 跳过终端恢复）。修复前本调用 panic＝测试红。
        let msg = crate::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: crate::prompt::HistoryRole::User,
            content: "快照记录".into(),
            user_input_event: None,
        };
        session::save_snapshot(
            &user.join("sessions"),
            "poison-snap",
            &[HistoryItem::Text(msg)],
        )
        .unwrap();
        let outcome = host.run_command("/resume poison-snap");
        assert!(
            matches!(outcome, tchat_tui::chat_driver::CommandOutcome::ReplaceCells { .. }),
            "毒锁下 /resume 必须完成投影而非 panic，实际：{outcome:?}"
        );

        let _ = std::fs::remove_dir_all(&cfg);
        let _ = std::fs::remove_dir_all(&user);
    }

    /// 021：接续通道原子性——seal 在有受理输入时收取且保持开放；为空才封口；
    /// 封口后提交得到"当前轮已结束"回执；close 幂等并排空残留。
    #[test]
    fn steer_channel_seal_close_atomicity() {
        let slot = Arc::new(Mutex::new(SteerSlot::Closed));
        let (tx, rx) = std_mpsc::channel();
        let mut channel = TurnSteerChannel { slot: slot.clone(), rx };
        // 封口态 seal＝Closed；close 幂等
        assert!(matches!(channel.seal(), turn::SteerSeal::Closed));
        assert!(channel.close().is_empty());
        // 打开并受理一份输入
        *slot.lock().unwrap() = SteerSlot::Open { tx: tx.clone() };
        tx.send(turn::SteerInput {
            id: 1,
            text: "追问".into(),
            image_paths: Vec::new(),
            text_elements: Vec::new(),
        })
        .unwrap();
        match channel.seal() {
            turn::SteerSeal::Inputs(inputs) => {
                assert_eq!(inputs.len(), 1);
                assert_eq!(inputs[0].id, 1);
            }
            other => panic!("有受理输入时 seal 不得封口：{other:?}"),
        }
        // 受理仍开放：再次提交可达（drain 收取）
        tx.send(turn::SteerInput {
            id: 2,
            text: "再问".into(),
            image_paths: Vec::new(),
            text_elements: Vec::new(),
        })
        .unwrap();
        assert_eq!(channel.drain().len(), 1);
        // 通道空时 seal 原子封口；此后提交得到"当前轮已结束"
        assert!(matches!(channel.seal(), turn::SteerSeal::Closed));
        let submit_failed = {
            let guard = slot.lock().unwrap();
            match &*guard {
                SteerSlot::Open { tx } => tx
                    .send(turn::SteerInput {
                        id: 3,
                        text: "迟到".into(),
                        image_paths: Vec::new(),
                        text_elements: Vec::new(),
                    })
                    .is_err(),
                SteerSlot::Closed => true,
            }
        };
        assert!(submit_failed, "封口后提交必须失败（当前轮已结束）");
        // close 幂等：不再产生输入
        assert!(channel.close().is_empty());
    }

    /// 021：消息工具终态转换——成功重建消息（实时与恢复同一转换）；失败/取消/
    /// 结果未知返回 None（实时路径显式呈现原因，恢复按普通文本降级）。
    #[test]
    fn tool_end_item_send_message_shapes() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        // 成功：accepted＋参数 → AssistantMessage（trim 后原文）
        match tool_end_item(
            "send_message_to_user_async",
            "m1",
            r#"{"message":"  重要发现。  "}"#,
            r#"{"accepted":true}"#,
            None,
            false,
        ) {
            Some(H::AssistantMessage { content }) => assert_eq!(content, "重要发现。"),
            other => panic!("成功形状：{other:?}"),
        }
        // 展示副本过终端过滤（§2.1）：含转义序列的消息只影响展示副本
        // 模型侧 JSON 转义形态（json! 宏产出 \u 转义，与真实响应一致）
        let evil_args = serde_json::json!({ "message": "\u{1b}[2J清除\u{7}屏\rPWNED" }).to_string();
        match tool_end_item(
            "send_message_to_user_async",
            "m-esc",
            &evil_args,
            r#"{"accepted":true}"#,
            None,
            false,
        ) {
            Some(H::AssistantMessage { content }) => {
                assert_eq!(content, "清除屏PWNED", "展示副本已过滤：{content:?}");
            }
            other => panic!("转义消息形状：{other:?}"),
        }
        // 失败/取消/损坏：None（不伪造消息）
        for result in [
            r#"{"error":"message must not be empty"}"#,
            r#"{"error":"用户取消，本调用未执行"}"#,
            "not-json",
        ] {
            assert!(
                tool_end_item(
                    "send_message_to_user_async",
                    "mx",
                    r#"{"message":"m"}"#,
                    result,
                    None,
                    false,
                )
                .is_none(),
                "{result} 不得投影为消息"
            );
        }
        // 失败提示行（宿主实时路径显式呈现）
        assert!(crate::message_to_user::failure_line(r#"{"error":"x"}"#).contains("失败"));
    }

    /// 021：恢复投影——配对成功记录重建消息 cell（只展示不重发）；孤儿/失败
    /// 结果按普通文本保留，不推定发送成功。
    #[test]
    fn project_history_items_restores_send_message() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let history = vec![
            HistoryItem::AssistantToolCalls {
                tool_calls: vec![crate::prompt::ToolCallRecord {
                    id: "m1".into(),
                    name: "send_message_to_user_async".into(),
                    arguments: r#"{"message":"中途说明"}"#.into(),
                }],
                content: None,
            },
            HistoryItem::ToolResult {
                tool_call_id: "m1".into(),
                content: r#"{"accepted":true}"#.into(),
                display: None,
                image: None,
            },
            // 失败结果（取消跳过）：不重建为消息
            HistoryItem::AssistantToolCalls {
                tool_calls: vec![crate::prompt::ToolCallRecord {
                    id: "m2".into(),
                    name: "send_message_to_user_async".into(),
                    arguments: r#"{"message":"没发出去"}"#.into(),
                }],
                content: None,
            },
            HistoryItem::ToolResult {
                tool_call_id: "m2".into(),
                content: r#"{"error":"用户取消，本调用未执行"}"#.into(),
                display: None,
                image: None,
            },
        ];
        let items = project_history_items(&history);
        match &items[0] {
            H::AssistantMessage { content } => assert_eq!(content, "中途说明"),
            other => panic!("成功记录形状：{other:?}"),
        }
        match &items[1] {
            H::Text { assistant, content } => {
                assert!(*assistant);
                assert!(content.contains("用户取消"), "失败结果按普通文本保留：{content}");
            }
            other => panic!("失败记录形状：{other:?}"),
        }
    }
}
