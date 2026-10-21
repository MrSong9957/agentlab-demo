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
}

impl TuiHost {
    /// 驱动取消钩子直连（006 N2b：置标志→工具线程终止进程组）。
    pub fn set_cancel(&self, requested: bool) {
        HOST_CANCEL.store(requested, Ordering::SeqCst);
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
    pub fn spawn_turn(&self, input: String, tx: std_mpsc::Sender<DriverEvent>) {
        self.run_turn_inner(input, tx);
    }

    fn run_turn_inner(
        &self,
        input: String,
        tx: std_mpsc::Sender<DriverEvent>,
    ) {
        let history = self.history.clone();
        let autosave = self.autosave.clone();
        let agent = self.agent.clone();
        let authorized_guard = self.authorized.clone();
        let registrar = self.registrar.clone();
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
                    conn: prov.conn.clone(),
                    key: cred.value.clone(),
                    model: model.clone(),
                    agent: agent.clone(),
                },
            );
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
                let key = key.clone();
                let model = model.clone();
                let agent = agent.clone();
                let body = body.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(api::chat_with_body(&conn, &key, &model, &body, &agent));
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
            let mut notify = |n: turn::TurnNotify| {
                let ev: Option<DriverEvent> = match n {
                    turn::TurnNotify::Info(line) => Some(DriverEvent::Info(line)),
                    turn::TurnNotify::ToolBegin { id, name, arguments } => {
                        Some(tool_begin_event(&name, &id, &arguments).unwrap_or_else(|| {
                            DriverEvent::Info(turn::tool_activity_line(&name, &id))
                        }))
                    }
                    turn::TurnNotify::ToolEnd { id, name, arguments, result, display, cancelled } => {
                        // 未知工具：无 cell 可替换（Begin 已回退活动行），不发送
                        tool_end_item(&name, &arguments, &result, display.as_ref(), cancelled)
                            .map(|item| DriverEvent::ToolEnd { id, item })
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
            let result = turn::run_user_turn_with_context(
                &mut history.lock().unwrap(),
                &input,
                &mut sender,
                &mut notify,
                Some(&HOST_CANCEL),
                &mut on_record,
                mode,
                &context_sources,
            );
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
                    let _ = tx.send(DriverEvent::Failed(e));
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
            other => Outcome::Output(format!(
                "未知命令 {other}，可用：/models、/refresh、/save <名称>、/resume [名称]、/sessions、/memory-status、/memory-reset confirm"
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
        for (id, _m) in &prov.models {
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
fn project_history_items(history: &[HistoryItem]) -> Vec<HostHistoryItem> {
    use std::collections::HashMap;
    let mut pending: HashMap<String, (String, String)> = HashMap::new();
    let mut out = Vec::new();
    for item in history {
        match item {
            HistoryItem::Text(m) => {
                // 015：自动上下文消息不作为真实用户发言展示（仅投影层过滤，
                // 历史与请求面不变；识别与去重共用同一识别器）
                if m.role == crate::prompt::HistoryRole::User
                    && crate::context::context_body(&m.content).is_some()
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
            HistoryItem::ToolResult { tool_call_id, content, display } => {
                // 无配对或未知工具：按普通文本投影结果内容（不丢失历史）
                let item = pending
                    .remove(tool_call_id)
                    .and_then(|(name, arguments)| {
                        tool_end_item(&name, &arguments, content, display.as_ref(), false)
                    })
                    .unwrap_or(HostHistoryItem::Text {
                        assistant: true,
                        content: content.clone(),
                    });
                out.push(item);
            }
        }
    }
    out
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

/// 012：工具终态 → HostHistoryItem（实时 ToolEnd 与 /resume 投影共用）。
/// 取消判定：exec＝结果 JSON `status=="cancelled"`（被杀执行，新旧会话通用）；
/// patch＝事件旗标（实时）或 [`turn::SKIPPED_BY_CANCEL_MESSAGE`] 精确匹配（恢复
/// 路径——单一生产者常量）。解析失败返回 None（调用方按普通文本处理）。
fn tool_end_item(
    name: &str,
    arguments: &str,
    result: &str,
    display: Option<&ToolDisplay>,
    cancelled_event: bool,
) -> Option<HostHistoryItem> {
    let v: serde_json::Value = serde_json::from_str(result).ok()?;
    match name {
        "exec_command" => {
            let command = arg_cmd(arguments).unwrap_or_default();
            let duration_ms = match display {
                Some(ToolDisplay::Exec { duration_ms }) => Some(*duration_ms),
                _ => None,
            };
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
        match tool_end_item("exec_command", exec_args, exec_result, Some(&ToolDisplay::Exec {
            duration_ms: 42,
        }), false) {
            Some(H::Exec { command, output, exit_code, cancelled, error, duration_ms }) => {
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
        match tool_end_item("exec_command", exec_args, cancelled_result, None, false) {
            Some(H::Exec { cancelled, .. }) => assert!(cancelled),
            other => panic!("cancelled 形状：{other:?}"),
        }
        // patch：changed_files 为对象数组（{path, action}——原按字符串提取恒空）
        let patch_result = r#"{"ok":true,"changed_files":[{"path":"/w/a.txt","action":"add"}],"state_known":true,"error":null}"#;
        match tool_end_item("apply_patch", "{}", patch_result, Some(&ToolDisplay::Patch {
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
        assert!(tool_end_item("web_search", "{}", r#"{"error":"x"}"#, None, false).is_none());
        assert!(tool_end_item("exec_command", "{}", "not json", None, false).is_none());
    }

    /// 012：/resume 投影配对——arguments 随 id 配对传递（旧会话命令恢复），
    /// 无配对结果按文本投影不丢失。
    #[test]
    fn project_history_items_pairs_arguments_for_resume() {
        use tchat_tui::chat_driver::HostHistoryItem as H;
        let history = vec![
            HistoryItem::Text(crate::prompt::HistoryMessage {
                role: crate::prompt::HistoryRole::User,
                content: "go".into(),
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
                tool_call_id: "c1".into(),
                content: r#"{"status":"completed","exit_code":0,"output":"","truncated":false,"error":null}"#.into(),
                display: None,
            },
            HistoryItem::ToolResult {
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
                role: crate::prompt::HistoryRole::User,
                content: declared,
            }),
            HistoryItem::Text(crate::prompt::HistoryMessage {
                role: crate::prompt::HistoryRole::User,
                content: "真实输入".into(),
            }),
            HistoryItem::Text(crate::prompt::HistoryMessage {
                role: crate::prompt::HistoryRole::Assistant,
                content: "答".into(),
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
            role: crate::prompt::HistoryRole::User,
            content: env_block.to_string(),
        })];
        assert!(project_history_items(&history2).is_empty());
    }

    use super::*;
    use std::time::Duration;

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
        host.spawn_turn("hello".into(), tx);
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
            role: crate::prompt::HistoryRole::User,
            content: "快照记录".into(),
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
}
