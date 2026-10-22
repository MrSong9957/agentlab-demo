//! 008-ADAPT（本地新写）：ChatDriver——terminal-chat 的最小 TUI 驱动。
//!
//! 母本的驱动层（app.rs/chatwidget.rs，约 3.5 万行）不整体移植（008 §1）；本模块以
//! 数百行复刻其核心调用序列（参照 chatwidget/{rendering,streaming,turn_lifecycle}.rs）：
//! 拥有终端输出与光标（tui::Tui + custom_terminal），组合 BottomPane（输入）与
//! TranscriptView（Owned transcript 视口），流式经 StreamController（稳定区＋活动区），
//! 完成项插入保留历史 cells。
//!
//! 对 terminal-chat 的边界：本模块不执行 turn——经 [`DriverHooks`] 由宿主在独立线程
//! 运行同步 turn 逻辑（003–007 契约不变），并经 [`DriverEvent`] 回推增量/完成/错误。
//! 按键契约（单一职责，2026-10-15 重定——替代 008 §3-J10 的 Ctrl+C 取消分工，
//! 见 012 §9）：接管前 SIGINT 130（宿主 raw-mode 前）；**Ctrl+C＝一律退出**（任何
//! 页面/选区/聚焦/任务态——运行中退出走宿主完整收尾（顺序＝终端恢复→置取消
//! →SIGKILL 工具进程组并回收→会话排空→`再见!` 码 0，见 main.rs run_tui/Quit 路径）；**Esc＝仅取消**（运行中一次按键一次
//! 取消，经 `set_cancel_requested`→宿主 HOST_CANCEL 链；空闲无动作）。两键均在
//! 驱动统一拦截（先于视图/composer/页路由），防止 vendored 选区、聚焦、阅读器
//! （Esc-to-latest）与 composer interrupt 分支接管；**q＝详情页关闭**（母本
//! PagerKeymap.close）。替代入口：清选区＝点击输入区、回底＝Ctrl+End、解焦＝F4、
//! 收 ? 提示＝再按 ?。

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::Event as CrosstermEvent;
use crossterm::event::EventStream;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use futures::StreamExt;
use ratatui::layout::Rect;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::BottomPane;
use crate::bottom_pane::BottomPaneParams;
use crate::bottom_pane::InputResult;
use crate::chatwidget::ActiveCellTranscriptKey;
use crate::history_cell::AgentMarkdownCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::streaming::controller::StreamController;
use crate::markdown_render::ListSpacing;
use crate::transcript_view::TranscriptView;
use crate::tui::Tui;

/// 宿主→驱动事件（一轮 turn 的回推流）。
#[derive(Debug)]
pub enum DriverEvent {
    /// 增量文本（按宿主确定性切分）。
    Delta(String),
    /// 一轮成功完成：最终完整回复文本。
    Final(String),
    /// 一轮失败：错误文本（已由宿主按 003 规则截断/脱敏）。
    Failed(String),
    /// 单行提示（目录刷新结果、落盘告警等）——渲染为 info 单元。
    Info(String),
    /// 012：工具调用开始（exec＝运行态 ExecCell；patch＝运行中标记行）。
    ToolBegin {
        id: String,
        kind: ToolBeginKind,
    },
    /// 012：工具调用终态——`item` 与 `/resume` 投影同源（HostHistoryItem），经
    /// `project_tool_item` 同一实现渲染，保证实时终态与恢复视图一致。
    ToolEnd {
        id: String,
        item: HostHistoryItem,
    },
}

/// 012：工具开始事件的展示形态。
#[derive(Debug, Clone)]
pub enum ToolBeginKind {
    /// exec_command：实际命令（arguments.cmd 原文）。
    Exec { command: String },
    /// apply_patch（差异在结束时才真实可得——运行中仅标记行，不虚构部分差异）。
    Patch,
}

/// 驱动→宿主钩子。宿主实现全部业务（turn 执行、命令、会话、退出清理）。
pub struct DriverHooks {
    /// 提交用户文本：宿主在独立线程运行一轮，经 sender 回推 DriverEvent。
    /// 驱动保证同一时刻至多一个未完成 turn。
    pub spawn_turn: Box<dyn FnMut(String, std_mpsc::Sender<DriverEvent>) + Send>,
    /// 斜杠命令分发（/models /refresh /save /resume /sessions 等）。返回值经 Info 回显；
    /// `ShouldExit` 表示命令请求退出（/exit）。
    pub run_command: Box<dyn FnMut(&str) -> CommandOutcome + Send>,
    /// turn 运行中的取消标志句柄（006 N2b：宿主置标志→SIGKILL 进程组→CLI 存活）。
    pub set_cancel_requested: Box<dyn FnMut(bool) + Send>,
    /// 启动通知（F7 权限审计等本地生成行）——渲染为既有通知 cell（008 P2-7）。
    pub startup_notices: Vec<String>,
}

/// 命令结果。
#[derive(Debug)]
pub enum CommandOutcome {
    /// 输出文本（多行）。
    Output(String),
    /// 输出文本＋替换全部历史 cells（/resume 投影：装载历史立即渲染，不重放工具）。
    ReplaceCells {
        output: String,
        cells: Vec<HostCell>,
    },
    /// 请求退出。
    ShouldExit,
}

/// 驱动结论。
#[derive(Debug)]
pub enum DriverExit {
    /// 用户请求退出（exit/Ctrl+C 空闲/D）。
    Quit,
}

const FRAME_TICK: Duration = Duration::from_millis(100);

// 012 §8：独立工具详情页状态——单 cell 页＋进入前主视图书签（内容锚点恢复）
struct ToolDetail {
    overlay: crate::pager_overlay::Overlay,
    bookmark: crate::transcript_view::TranscriptBookmark,
    /// 打开 cell 的活动 id（exec 页＝`exec:{id}`、patch 页＝`{id}`）——工具在
    /// 浏览期间到达终态时按此刷新页内 cell
    activity: Option<String>,
}

/// 打开工具详情页：清主视图活动聚焦（K-D——焦点不残留，返回后 Enter 归提交）、
/// 快照主视图书签、以单 cell 构建页（复用 TranscriptOverlay 全套滚动/选区/复制）。
fn open_tool_detail(
    transcript: &mut TranscriptView,
    cells: &[Arc<dyn HistoryCell>],
    index: usize,
    keymap: &crate::keymap::RuntimeKeymap,
) -> ToolDetail {
    transcript.clear_activity_focus();
    let bookmark = transcript.bookmark(cells);
    let activity = cells
        .get(index)
        .and_then(|cell| cell.activity_ids().first().cloned());
    let overlay = crate::pager_overlay::Overlay::new_transcript(
        vec![Arc::clone(&cells[index])],
        keymap.pager.clone(),
        /*copy_on_select*/ false,
        /*mouse_scroll_speed*/ 1.0,
    )
    .as_tool_page("tool details".to_owned());
    ToolDetail { overlay, bookmark, activity }
}

/// 运行交互 TUI 主循环直至退出。终端已由 `tui::init()` 初始化（raw mode＋备用屏策略）。
pub async fn run(mut tui: Tui, mut hooks: DriverHooks) -> io::Result<DriverExit> {
    tui.set_alt_screen_enabled(true);
    tui.set_owned_screen(true)?; // 008：默认全屏 Owned transcript（§1 模式定稿）
    let (app_tx, mut app_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
    let app_event_tx = AppEventSender::new(app_tx.clone());
    let frame_requester = tui.frame_requester();

    let mut pane = BottomPane::new(BottomPaneParams {
        app_event_tx: app_event_tx.clone(),
        frame_requester,
        has_input_focus: true,
        enhanced_keys_supported: true,
        placeholder_text: String::new(),
        disable_paste_burst: false,
        animations_enabled: true,
        effects: Default::default(),
    });
    let keymap = crate::keymap::RuntimeKeymap::defaults();
    pane.set_keymap_bindings(&keymap);

    let mut transcript = TranscriptView::default();
    // 008：启动为空 transcript（"No transcript content available" 占位由
    // thread_to_transcript_cells 仅在 /resume 空会话投影时给出；启动期注入会在
    // 有内容后残留为永久 cell）
    let mut cells: Vec<Arc<dyn HistoryCell>> = Vec::new();
    // 008 P2-7：启动通知（F7 审计）复用既有通知 cell——与行式输出同内容
    for line in &hooks.startup_notices {
        cells.push(Arc::new(PlainHistoryCell::new(vec![
            ratatui::text::Line::from(line.clone()),
        ])));
    }
    let mut stream: Option<StreamController> = None;
    // 011：滚动条拖动状态机——按下归属沟位后，Drag/Up 即便指针越出沟位也归它；
    // 行数持久缓存（宽键＋Weak 指针身份），呈现态变化时显式失效（见键路由）
    let mut scrollbar_drag: Option<crate::transcript_view::scrollbar::ScrollDrag> = None;
    let mut scrollbar_rows: crate::transcript_view::scrollbar::RowCache =
        crate::transcript_view::scrollbar::RowCache::default();
    let cwd: PathBuf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

    // turn 回推通道（非阻塞读取驱动侧）
    let (turn_tx, turn_rx) = std_mpsc::channel::<DriverEvent>();
    let mut turn_rx = Some(turn_rx);
    let mut turn_running = false;
    // 本轮流式 cell 的下标集合（008-REVIEW 收敛：Final/Failed 仅移除这些 cell——
    // turn 期间推入的 Info/错误 cell（工具活动行、落盘告警——006/007 通知契约）
    // 必须保留；倒序 remove 免受下标漂移，也不保留跨事件窗口的绝对下标）
    let mut stream_indices: Vec<usize> = Vec::new();
    // 012：工具 cell 记账（tool_call_id → cells 下标）。ToolEnd 按下标替换为终态
    // cell；Final/Failed 后下标因流式 cell 移除而漂移——turn 终结即清空（此后不再
    // 替换）。工具 cell 绝不入 stream_indices（需求⑧：最终回复不覆盖工具详情）。
    let mut tool_cells: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // 008：turn 运行中提交的排队输入（母本排队语义；一次重放一条，至多一个在飞 turn）
    let mut queued_inputs: VecDeque<String> = VecDeque::new();
    // 012 §8：独立工具详情页槽位（母本 app.overlay 同构；打开期间终端事件全路由页）
    let mut tool_detail: Option<ToolDetail> = None;

    // 首帧
    draw(&mut tui, &pane, &mut transcript, &mut scrollbar_rows, &cells, stream.as_ref())?;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(FRAME_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 011 §7：帧调度通知（FrameScheduler 合并＋120fps 限频后的 Draw 唤醒）——
    // 部件侧动画/冲刷定时/剪贴板完成经此请求绘制；驱动侧状态变化直接置脏
    let mut draw_rx = tui.subscribe_draw_notifications();
    // 011 §7：脏标记——循环尾仅在有未显示变化时绘制（取消无条件 draw）。
    // 置脏来源：驱动侧状态变化（cells/滚动/选区/沟位/resize/命令结果）与
    // Draw 通知；composer 键/粘贴路径自请求（request_redraw），不在此置脏
    let mut needs_draw = false;

    loop {
        // 事件多路：crossterm / tick / app event / draw 通知
        let ev = tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(ev)) => Some(Ev::Term(ev)),
                Some(Err(_)) => None,
                None => None, // 事件流关闭：退出主循环
            },
            _ = ticker.tick() => Some(Ev::Tick),
            app = app_rx.recv() => app.map(Ev::App),
            _ = draw_rx.recv() => Some(Ev::DrawRequested),
        };
        let Some(ev) = ev else { break };
        match ev {
            Ev::Tick => {
                // 011 §7：tick 推进产生可见变化才置脏（无变化＝零帧）
                let mut changed = false;
                // 010：剪贴板工人 poll（坑 1：OSC 52 终端写发生在 poll 内——不 poll
                // 则复制永远不落地）；完成分派＝composer 票据择一（母本 app/clipboard.rs
                // finish_clipboard 同序：composer 完成回显在视图 gap 行，视图票据自洽）
                // 012 §8：详情页打开期间让位——poll 由页内 handle_event 执行（否则
                // 抢走页内 finish_copy 的完成分派）
                if tool_detail.is_none()
                    && let Some(completion) = tui.clipboard.poll().cloned()
                {
                    changed = true;
                    let current = tui.is_owned_screen();
                    let completion = &completion;
                    if let Some(characters) =
                        pane.finish_composer_copy(completion, /*visible*/ current)
                    {
                        transcript.show_copy_feedback(&completion.1, characters);
                    }
                    if transcript.finish_copy(&cells, completion, current) == Some(true) {
                        transcript.jump_to_latest();
                    }
                }
                // 排空 turn 回推（非阻塞）
                if let Some(rx) = turn_rx.as_mut() {
                    while let Ok(event) = rx.try_recv() {
                        changed = true;
                        match event {
                            DriverEvent::Delta(delta) => {
                                let controller = stream.get_or_insert_with(|| new_stream(&cwd));
                                controller.push(&delta);
                                if let Some(controller) = stream.as_mut() {
                                    let (cell, _idle) = controller.on_commit_tick();
                                    if let Some(cell) = cell {
                                        stream_indices.push(cells.len());
                                        cells.push(Arc::from(cell));
                                    }
                                }
                            }
                            DriverEvent::Final(text) => {
                                // 008-REVIEW：仅移除本轮流式 cell（倒序），以完整回复
                                // 建单一 cell；turn 期间推入的 Info/错误 cell 保留
                                for idx in stream_indices.drain(..).rev() {
                                    cells.remove(idx);
                                }
                                // 012：流式移除使工具下标漂移——turn 终结即清空记账
                                tool_cells.clear();
                                finish_stream(&mut stream, &mut cells, &cwd, text, false);
                                turn_running = false;
                                pane.set_task_running(false);
                                (hooks.set_cancel_requested)(false);
                            }
                            DriverEvent::Failed(text) => {
                                for idx in stream_indices.drain(..).rev() {
                                    cells.remove(idx);
                                }
                                tool_cells.clear();
                                finish_stream(&mut stream, &mut cells, &cwd, text, true);
                                turn_running = false;
                                pane.set_task_running(false);
                                (hooks.set_cancel_requested)(false);
                            }
                            DriverEvent::Info(line) => {
                                cells.push(Arc::new(PlainHistoryCell::new(vec![
                                    ratatui::text::Line::from(line),
                                ])));
                            }
                            DriverEvent::ToolBegin { id, kind } => {
                                // 012：exec＝运行态 ExecCell（spinner＋实际命令）；
                                // patch＝运行中标记行（差异在结束才真实可得，不虚构）
                                let cell: Box<dyn HistoryCell> = match kind {
                                    ToolBeginKind::Exec { command } => {
                                        Box::new(crate::exec_cell::new_active_exec_command(
                                            id.clone(),
                                            // 012 §6：按 003 真实执行 argv（/bin/sh -c <cmd>）
                                            // 存储——母本剥壳链（extract_bash_command 认
                                            // -c＋Sh）剥出原文，展示不整条加引号
                                            shell_argv(&command),
                                            Vec::new(),
                                            codex_app_server_protocol::CommandExecutionSource::Agent,
                                            /*interaction_input*/ None,
                                            /*animations_enabled*/ true,
                                        ))
                                    }
                                    ToolBeginKind::Patch => {
                                        use ratatui::style::Stylize as _;
                                        Box::new(PlainHistoryCell::new(vec![
                                            ratatui::text::Line::from("正在应用补丁…").dim(),
                                        ]))
                                    }
                                };
                                tool_cells.insert(id, cells.len());
                                cells.push(Arc::from(cell));
                            }
                            DriverEvent::ToolEnd { id, item } => {
                                // 012：终态经与 /resume 同一投影（project_tool_item）；
                                // 按记账下标替换运行态 cell（1→N 时修正其余记账下标）
                                let projected = project_tool_item(&item, &id, &cwd);
                                let tool_cells_removed_idx = tool_cells.remove(&id);
                                match tool_cells_removed_idx {
                                    Some(idx) if idx < cells.len() => {
                                        let delta = projected.len() as isize - 1;
                                        cells.splice(idx..idx + 1, projected);
                                        if delta != 0 {
                                            for v in tool_cells.values_mut() {
                                                if *v > idx {
                                                    *v = (*v as isize + delta) as usize;
                                                }
                                            }
                                        }
                                    }
                                    _ => cells.extend(projected),
                                }
                                // 012 §8：浏览中的工具到达终态——同 id 页内 cell 刷新
                                // （exec 页活动 id＝`exec:{id}`、patch 页＝`{id}`）
                                if let Some(detail) = tool_detail.as_mut() {
                                    let exec_page =
                                        detail.activity.as_deref() == Some(&format!("exec:{id}"));
                                    let patch_page =
                                        detail.activity.as_deref() == Some(&format!("patch:{id}"));
                                    if exec_page || patch_page {
                                        let page_cell = match tool_cells_removed_idx {
                                            Some(i) => cells.get(i).map(Arc::clone),
                                            None => cells.last().map(Arc::clone),
                                        };
                                        if let Some(cell) = page_cell {
                                            if let crate::pager_overlay::Overlay::Transcript(o) =
                                                &mut detail.overlay
                                            {
                                                o.replace_cells(vec![cell]);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // 011 §7：粘贴突发冲刷有产出才置脏（composer 冲刷定时亦经 Draw
                // 通知请求；此处为 tick 兜底路径）
                if pane.flush_paste_burst_if_due() {
                    changed = true;
                }
                // 008：turn 空闲且有排队输入→按空闲提交同语义重放一条（不丢输入；
                // 母本排队语义：当前 turn 结束后依次处理）
                if !turn_running && !queued_inputs.is_empty() {
                    changed = true;
                    let text = queued_inputs.pop_front().unwrap();
                    pane.set_pending_input_preview(
                        queued_inputs.iter().cloned().collect(),
                        Vec::new(),
                        Vec::new(),
                    );
                    match classify_idle_submission(&text) {
                        Some(IdleSubmission::Exit) => return Ok(DriverExit::Quit),
                        Some(IdleSubmission::Command { name, full }) => {
                            if handle_builtin_exit(&name) {
                                return Ok(DriverExit::Quit);
                            }
                            if apply_command_outcome(
                                (hooks.run_command)(&full),
                                &mut cells,
                                &mut transcript,
                            ) {
                                return Ok(DriverExit::Quit);
                            }
                        }
                        Some(IdleSubmission::Turn(text)) => {
                            spawn_turn_cells(&mut cells, &mut stream_indices, &text);
                            turn_running = true;
                            pane.set_task_running(true);
                            (hooks.spawn_turn)(text, turn_tx.clone());
                        }
                        None => {}
                    }
                }
                needs_draw |= changed;
            }
            Ev::DrawRequested => {
                // 011 §7：部件帧请求（经 FrameScheduler 合并限频）——唤醒即绘制
                needs_draw = true;
            }
            Ev::App(app_event) => {
                // 011 §7：app 事件低频（插入 cell/中断）——保守置脏
                needs_draw = true;
                match app_event {
                    AppEvent::InsertHistoryCell(cell) => cells.push(Arc::from(cell)),
                    AppEvent::AppendMessageHistoryEntry { .. } => {
                        // 008：composer 已在提交路径内建记录本地历史（record_local_submission）；
                        // 本事件为母本跨会话持久日志钩子，本地无持久源（见下条），no-op。
                    }
                    AppEvent::LookupMessageHistoryEntry { .. }
                    | AppEvent::LookupMessageHistoryBatch { .. } => {
                        // 008：不可达——驱动未调用 set_history_metadata（persistent_entry_count
                        // 恒 0，查找仅在 offset < persistent_entry_count 时发出）。会话内上/下
                        // 键回溯由 composer local_history 即时返回（001 rustyline 内存历史等价：
                        // 两者均无跨会话持久化）。
                    }
                    AppEvent::TranscriptCopyClosed => {}
                    AppEvent::OpenUrlInBrowser { url } => {
                        let _ = webbrowser::open(&url); // 008-ADAPT：本地经 webbrowser 打开
                    }
                    AppEvent::Interrupt => {
                        if turn_running {
                            (hooks.set_cancel_requested)(true);
                        }
                    }
                    }
            }
            Ev::Term(term_event) => {
                // 012 §8＋§9（单一职责轮）：详情页打开期间终端事件全路由页（母本
                // app.rs overlay 槽位同构）。Ctrl+C/Ctrl+D/Esc 在路由前由驱动统一
                // 拦截：Ctrl+C＝一律退出（运行中退出走宿主完整收尾——不在此取消；
                // 母本 pager close 默认含 ctrl+c＝「仅关闭页面」，不得继承）；
                // Esc＝仅取消（页保持打开，反馈经页内 notice 呈现，不改正查看的
                // 旧工具 cell；空闲无动作）；q 关页由页内 PagerKeymap.close 承接。
                if tool_detail.is_some() {
                    if let CrosstermEvent::Key(key) = &term_event {
                        if key.kind == KeyEventKind::Press && is_ctrl_c(key) {
                            return Ok(DriverExit::Quit);
                        } else if key.kind == KeyEventKind::Press
                            && is_ctrl_d(key)
                            && pane.composer_text_with_pending().trim().is_empty()
                        {
                            return Ok(DriverExit::Quit);
                        } else if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
                            && key.code == KeyCode::Esc
                            && key.modifiers.is_empty()
                        {
                            if turn_running {
                                (hooks.set_cancel_requested)(true);
                                if let Some(detail) = tool_detail.as_mut() {
                                    detail.overlay.set_notice(
                                        "已取消当前任务：已停止等待模型回复（迟到结果将被丢弃）"
                                            .to_string(),
                                    );
                                }
                            }
                            // 消费该键：空闲无动作也不得落入页（页 Esc 关闭已由 q 取代）
                            tui.frame_requester().schedule_frame();
                            continue;
                        }
                    }
                    let tui_ev = match term_event {
                        CrosstermEvent::Key(k) => crate::tui::TuiEvent::Key(k),
                        CrosstermEvent::Paste(p) => crate::tui::TuiEvent::Paste(p),
                        CrosstermEvent::Mouse(m) => crate::tui::TuiEvent::Mouse(m),
                        CrosstermEvent::Resize(cols, rows) => {
                            let size =
                                ratatui::layout::Size { width: cols, height: rows };
                            let _ = tui.screen_size_for_event(
                                &crate::tui::TuiEvent::Resize(size),
                            )?;
                            crate::tui::TuiEvent::Resize(size)
                        }
                        CrosstermEvent::FocusGained => crate::tui::TuiEvent::FocusGained,
                        CrosstermEvent::FocusLost => crate::tui::TuiEvent::FocusLost,
                    };
                    if let Some(detail) = tool_detail.as_mut() {
                        detail.overlay.handle_event(&mut tui, tui_ev)?;
                        if detail.overlay.is_done() {
                            let detail = tool_detail.take().unwrap();
                            transcript.restore_bookmark(detail.bookmark);
                            // 关闭后主视图重绘（内容锚点恢复——需求⑤）
                            tui.frame_requester().schedule_frame();
                        }
                    }
                    // 页自调度帧（overlay.handle_event）保证尾绘制冲 flush；continue
                    // 使主视图/编辑域不接事件（需求④关闭不外溢）
                    continue;
                }
                // 011 §7：鼠标非 Moved 事件即时渲染（母本 owned_transcript.rs:449
                // 对齐）；Moved 悬停全链路无状态变化（prepare_composer_mouse 仅 Down
                // 动作、视图 handle_mouse Moved 落 `_ => None`、textarea 显式忽略）
                // ——不置脏、零帧，事件处理本身保留
                if let CrosstermEvent::Mouse(mouse) = &term_event {
                    needs_draw |= mouse.kind != crossterm::event::MouseEventKind::Moved;
                }
                match term_event {
                    CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                        if is_ctrl_c(&key) {
                            // 单一职责轮（012 §9）：Ctrl+C 一律退出——任何选区/
                            // 聚焦/任务态；运行中退出走宿主完整收尾（置取消→
                            // SIGKILL 工具进程组并回收→会话排空→终端恢复），不在
                            // 此取消（取消归 Esc）。
                            return Ok(DriverExit::Quit);
                        } else if key.code == KeyCode::Esc && key.modifiers.is_empty() {
                            // 单一职责轮（012 §9）：Esc＝仅取消——运行中一次按键
                            // 一次取消（经取消链路停止本地等待，迟到结果弃置）；
                            // 空闲＝无动作。消费该键，不落入视图/composer（防止
                            // vendored Esc-to-latest/清选区/清焦/关页/composer
                            // interrupt 分支接管）。不 jump：Esc 不得回底/清选区；
                            // 选区冻结视口下反馈行可能暂不可见——用户点击输入区
                            // 或 Ctrl+End 后可见（登记取舍）。
                            if turn_running {
                                (hooks.set_cancel_requested)(true);
                                // 011 §7：取消请求无可视变化，保持「任意按键即时
                                // 反馈」惯例（尾条件绘制）
                                needs_draw = true;
                            }
                        } else {
                            // 008-J10：Ctrl+D 退出条件＝空输入行（001 行式实测一致：
                            // rustyline 仅空行返回 Eof→再见!+0）。非空草稿不退出——交
                            // 母本 composer 键位（textarea 无 Ctrl+D 删字符映射→无操作；
                            // rustyline 非空行为前向删字符，编辑域差异，规格登记）
                            if is_ctrl_d(&key)
                                && pane.composer_text_with_pending().trim().is_empty()
                            {
                                return Ok(DriverExit::Quit);
                            }
                            // 010-K5（单一职责轮修订）：选区是鼠标覆盖层，键盘永不
                            // 复制——任何路由键先恢复无选区态再按无选区路由处理
                            // （Enter＝提交、字符＝composer；Ctrl+C/Esc 已在前被驱动
                            // 拦截、不清选区——清选区入口＝点击输入区）。不恢复则
                            // has_active_interaction 会让全部键（含 Enter/母本复制键）
                            // 落入选区分支。
                            if transcript.has_selection_range() {
                                transcript.end_selection(&cells);
                                // 011 §7：选区清除是可见变化——该键即便落 composer
                                // 无自请求也须重绘（坑 2）
                                needs_draw = true;
                            }
                            // 012：视图与 composer 的键归属（母本 owned 分发序：视图先，
                            // 视图放弃的键落回 composer——Ok(false) 语义）。
                            // - Ctrl+T＝detailed 呈现切换（母本 open/close_transcript：
                            //   owned 模式为同视图 set_presentation，折叠提示中 ctrl+t
                            //   由此真实可用，需求⑦）；
                            // - transcript_owns_key：滚动键（A3）＋F4 活动聚焦入口＋
                            //   视图交互激活期间的全部键（母本 has_active_interaction 门）。
                            let mut owned_by_view = false;
                            if is_ctrl_t(&key) {
                                // 012 §8：Ctrl+T＝打开（聚焦或最近）工具详情页——不再
                                // 切换全局原地 detailed 呈现（需求⑧：入口与新交互一致）
                                let index = transcript
                                    .focused_activity_index(&cells)
                                    .or_else(|| {
                                        cells.iter().rposition(|c| !c.activity_ids().is_empty())
                                    });
                                if let Some(i) = index {
                                    tool_detail =
                                        Some(open_tool_detail(&mut transcript, &cells, i, &keymap));
                                }
                                scrollbar_rows.clear();
                                owned_by_view = true;
                            } else if key.code == KeyCode::Enter
                                && key.modifiers.is_empty()
                                && transcript.is_activity_focused()
                            {
                                if pane.history_search_active() {
                                    // §9 复合态收尾：composer 历史搜索是输入子
                                    // 模式——Enter 按搜索规则处理（接受/恢复原稿），
                                    // 不被详情页入口抢占；同时解除工具聚焦（输入权
                                    // 交还 composer——退出搜索后再次 Enter 按正常
                                    // 输入规则，不被旧焦点抢走，s80）。落穿到
                                    // composer 路由（不设 owned_by_view）。
                                    transcript.clear_activity_focus();
                                    scrollbar_rows.clear();
                                    needs_draw = true;
                                } else {
                                    // 012 §8 需求⑦：聚焦工具＋Enter＝打开详情页
                                    //（非提交、非原地展开——预存草稿不受影响）
                                    match transcript.focused_activity_index(&cells) {
                                        Some(i) => {
                                            tool_detail = Some(open_tool_detail(
                                                &mut transcript, &cells, i, &keymap,
                                            ));
                                        }
                                        None => transcript.clear_activity_focus(),
                                    }
                                    scrollbar_rows.clear();
                                    owned_by_view = true;
                                }
                            } else if transcript_owns_key(
                                &transcript,
                                &key,
                                turn_running,
                                pane.has_active_view(),
                            ) && transcript.handle_key(key, &cells).is_some()
                            {
                                // 011：视图消费的键可能改变呈现（聚焦态 Enter 开页
                                // 折叠改变行数）——行数缓存失效，下一帧重算（滚动键不
                                // 改行数，多一次重算代价有界且罕见）
                                scrollbar_rows.clear();
                                owned_by_view = true;
                            }
                            // 011 §7：视图消费/Ctrl+T 必有可见变化（呈现/滚动/展开）
                            needs_draw |= owned_by_view;
                            if !owned_by_view {
                                // 008-J10：裸 exit 等价 /exit（001 命令语义保留）——仅在
                                // composer 将处理该键时判定（活动聚焦态 Enter 归视图）。
                                // 修饰键须为空：Shift+Enter 是 composer 换行键（多行输入）
                                let is_bare_exit = matches!(key.code, KeyCode::Enter)
                                    && key.modifiers.is_empty()
                                    && pane.composer_text_with_pending().trim() == "exit"
                                    // §9 复合态收尾（复审 P2-1）：搜索态 composer 文本
                                    // ＝预览稿（原稿/匹配项）——恰为 "exit" 时 Enter 的
                                    // 语义是「按搜索规则处理」，不得触发裸退出
                                    && !pane.history_search_active();
                                if is_bare_exit {
                                    return Ok(DriverExit::Quit);
                                }
                                match pane.handle_key_event(key) {
                                    InputResult::Submitted { text, .. } => {
                                        if text.trim().is_empty() {
                                            // 空输入不提交（001）——无状态变化，不置脏
                                        } else if turn_running {
                                            // 008：turn 运行中提交（母本为 steer 注入
                                            // 当前 turn；本地同步 turn 不支持）→ 排队待
                                            // 重放——至多一个在飞 turn（J11），不丢输入
                                            queue_input(&mut queued_inputs, &mut pane, &text);
                                            // 011 §7：排队预览是可见变化
                                            // （set_pending_input_preview 不自请求，坑 1）
                                            needs_draw = true;
                                        } else {
                                            spawn_turn_cells(
                                                &mut cells,
                                                &mut stream_indices,
                                                &text,
                                            );
                                            turn_running = true;
                                            pane.set_task_running(true);
                                            (hooks.spawn_turn)(text, turn_tx.clone());
                                            needs_draw = true;
                                        }
                                    }
                                    InputResult::Command(command) => {
                                        pane.clear_composer_for_command();
                                        let name = command.command().to_string();
                                        if handle_builtin_exit(&name) {
                                            return Ok(DriverExit::Quit);
                                        }
                                        if apply_command_outcome(
                                            (hooks.run_command)(&name),
                                            &mut cells,
                                            &mut transcript,
                                        ) {
                                            return Ok(DriverExit::Quit);
                                        }
                                        needs_draw = true;
                                    }
                                    InputResult::CommandWithArgs(command, args, _) => {
                                        // 008-ADAPT：带参命令提交即清空草稿（母本 popup
                                        // 派发行为；本地直发）
                                        pane.clear_composer_for_command();
                                        let name = command.command().to_string();
                                        let full = if args.trim().is_empty() {
                                            name.clone()
                                        } else {
                                            format!("{name} {}", args.trim())
                                        };
                                        if handle_builtin_exit(&name) {
                                            return Ok(DriverExit::Quit);
                                        }
                                        if apply_command_outcome(
                                            (hooks.run_command)(&full),
                                            &mut cells,
                                            &mut transcript,
                                        ) {
                                            return Ok(DriverExit::Quit);
                                        }
                                        needs_draw = true;
                                    }
                                    InputResult::Queued { text, .. } => {
                                        // 008：queue_keys 提交（母本排队路径；turn 期间
                                        // Enter 走上面的 Submitted）。排队项经
                                        // pending_input_preview 显示在 composer 上方
                                        // （母本行为），turn 结束后由 Tick 重放。
                                        queue_input(&mut queued_inputs, &mut pane, &text);
                                        needs_draw = true;
                                    }
                                    _ => {}
                                }
                                // 011 §7：其余 composer 结果（None/突发缓冲）不自置脏——
                                // composer 在状态变化时自请求（request_redraw），突发缓冲
                                // 字符遵循母本「honor paste batching」不逐键上屏
                            }
                        }
                    }
                    CrosstermEvent::Paste(pasted) => {
                        // 012 §7：粘贴是输入动作——母本 Paste 分支（owned_transcript.rs:
                        // 640-649）：先清 transcript 选区；非搜索编辑态清活动聚焦
                        // （点击展开/收起经 toggle_activity 置焦，粘贴不清焦会让后续
                        // Enter 被旧工具的 ListAction::Accept 吞掉——用户复现④）；
                        // paste_search 本地不可达（010 未接线搜索），调用仅为母本
                        // 逐字对齐（无活动搜索时恒 false→清焦）
                        transcript.end_selection(&cells);
                        if !transcript.paste_search(&pasted) {
                            transcript.clear_activity_focus();
                        }
                        let _ = pane.handle_paste(pasted);
                        // 011 §7：bracketed paste 单事件低频——保守置脏（handle_paste
                        // 亦自请求；标记合并）
                        needs_draw = true;
                    }
                    CrosstermEvent::Mouse(mouse) => {
                        // 010：母本分发序（app/owned_transcript.rs:447-487）——composer
                        // 准备无条件先行（冲刷 paste burst 首字符，坑 4）；沟位（011）
                        // 归滚动条状态机且判定先于选区；transcript 正文区 Left 拖选/
                        // 右键复制经 view.handle_mouse；composer 区（含其右键复制）后行。
                        let composer_ready = pane.prepare_composer_mouse(mouse);
                        let screen = tui.screen_size_for_layout()?;
                        let (transcript_area, _pane_area) = split_screen_areas(&pane, screen);
                        let in_transcript = mouse.row < transcript_area.height;
                        // 与 draw 共用同一沟位推导（K4：单一几何来源）
                        let (_, transcript_gutter) = split_transcript_gutter(transcript_area);
                        let gutter_column = in_transcript
                            && transcript_gutter.width > 0
                            && mouse.column >= transcript_gutter.x;
                        match mouse.kind {
                            crossterm::event::MouseEventKind::Down(
                                crossterm::event::MouseButton::Left,
                            ) if gutter_column => {
                                scrollbar_drag = crate::transcript_view::scrollbar::gutter_press(
                                    &mut transcript,
                                    &cells,
                                    &mut scrollbar_rows,
                                    mouse.row,
                                );
                            }
                            crossterm::event::MouseEventKind::Drag(
                                crossterm::event::MouseButton::Left,
                            ) if scrollbar_drag.is_some() => {
                                let drag = scrollbar_drag.unwrap();
                                // 指针越出沟位时按视口内行号钳制（拖动归属）
                                let row = mouse.row.min(transcript_area.height.saturating_sub(1));
                                crate::transcript_view::scrollbar::drag_to(
                                    &mut transcript, &cells, &mut scrollbar_rows, &drag, row,
                                );
                            }
                            crossterm::event::MouseEventKind::Up(
                                crossterm::event::MouseButton::Left,
                            ) if scrollbar_drag.is_some() => {
                                scrollbar_drag = None;
                            }
                            // 010：transcript 正文区（含滚轮——母本同链路分数行累积）
                            _ if in_transcript => {
                                use crossterm::event::MouseEventKind as Mk;
                                // 012 §8：披露行（详情入口）点击＝打开独立详情页——
                                // 不再走 pointer_down 原地展开（disclosure_hit 纯命中）；
                                // 该事件被入口消费，不落选区/链接路径
                                let mut detail_opened = false;
                                if matches!(mouse.kind, Mk::Down(crossterm::event::MouseButton::Left))
                                    && mouse.modifiers.is_empty()
                                {
                                    scrollbar_rows.clear();
                                    if let Some(index) =
                                        transcript.disclosure_hit(&cells, mouse.column, mouse.row)
                                    {
                                        tool_detail = Some(open_tool_detail(
                                            &mut transcript, &cells, index, &keymap,
                                        ));
                                        needs_draw = true;
                                        detail_opened = true;
                                    }
                                }
                                if !detail_opened
                                    && let Some(action) = transcript.handle_mouse(mouse, &cells)
                                {
                                    // 010：复制动作经视图既有管线（Markdown 序列化＋
                                    // 策略＝EXPLICIT；反馈写入 copy_feedback，gap 行渲染）
                                    transcript.copy_action(
                                        &mut tui,
                                        &cells,
                                        &action,
                                        crate::clipboard_copy::CopyFormat::Markdown,
                                    );
                                    // ViewAction::OpenLink：010-K5 范围外（不启用打开
                                    // 链接）——丢弃；Changed 无需额外处理（循环尾 draw）
                                }
                            }
                            // 010：composer 区——母本 copy 事件（右键复制）先于通用鼠标
                            _ => {
                                if composer_ready {
                                    let term_ev = crate::tui::TuiEvent::Mouse(mouse);
                                    let _ = pane.copy_composer_selection(&term_ev, |text| {
                                        tui.copy_transcript_selection(
                                            text,
                                            crate::clipboard_copy::CopyFormat::PlainText,
                                        )
                                    });
                                    if pane.handle_composer_mouse(mouse) {
                                    // 母本：composer 处理后清 transcript 选区（非滚轮）
                                        if !matches!(
                                            mouse.kind,
                                            crossterm::event::MouseEventKind::ScrollUp
                                                | crossterm::event::MouseEventKind::ScrollDown
                                        ) {
                                            transcript.end_selection(&cells);
                                            // 012 §7：母本同处 clear_activity_focus
                                            // （owned_transcript.rs:480-482；另含
                                            // cancel_search——本地搜索不可达未接线，
                                            // 登记不调）——点击输入区＝输入意图，
                                            // 活动聚焦交还，后续 Enter 归提交
                                            transcript.clear_activity_focus();
                                        }
                                    }
                                }
                            }
                        }
                    }
                    CrosstermEvent::Resize(cols, rows) => {
                        // 008-A3（审查轮修复）：Resize 必须经尺寸机制（母本
                        // screen_size_for_event(Resize)）——丢弃事件会令布局与
                        // 鼠标分区永远停留在旧尺寸
                        let size = ratatui::layout::Size { width: cols, height: rows };
                        let _ = tui
                            .screen_size_for_event(&crate::tui::TuiEvent::Resize(size))?;
                        // 011 §7：新几何必须重绘（可见变化）
                        needs_draw = true;
                    }
                    _ => {}
                }
            }
        }
        // 011 §7：循环尾条件绘制——仅有未显示变化时执行（原无条件 draw 移除）；
        // 最后一次状态变化即时落屏（无跨事件延迟），Draw 通知经 FrameScheduler
        // 合并限频后到达
        if needs_draw {
            if let Some(detail) = tool_detail.as_mut() {
                // 012 §8：详情页自绘（含标题/沟位/提示；Draw 事件幂等）
                detail.overlay.handle_event(&mut tui, crate::tui::TuiEvent::Draw)?;
            } else {
                draw(&mut tui, &pane, &mut transcript, &mut scrollbar_rows, &cells, stream.as_ref())?;
            }
            needs_draw = false;
        }
    }
    Ok(DriverExit::Quit)
}

enum Ev {
    Term(CrosstermEvent),
    Tick,
    App(AppEvent),
    /// 011 §7：FrameScheduler 帧通知（部件请求，已合并＋限频）
    DrawRequested,
}



fn is_ctrl_c(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL))
        && !key.modifiers.contains(KeyModifiers::SHIFT)
}

fn is_ctrl_d(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL))
        && !key.modifiers.contains(KeyModifiers::SHIFT)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

/// 012：Ctrl+T（母本 keymap `open_transcript`/`pager.close_transcript` 同键，
/// keymap.rs:1658/1908——owned 模式为同视图 detailed 呈现切换）。
fn is_ctrl_t(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL))
        && !key.modifiers.contains(KeyModifiers::SHIFT)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

/// 008-A3/012：transcript 键归属判定——母本 owned 主视图语义
/// （app.rs→handle_owned_transcript_event→transcript_view.handle_key）。
/// - **活动聚焦激活（has_active_interaction）期间全部键归视图**（母本
///   has_active_interaction 门：聚焦后方向键/Enter/Esc/F4 由 disclosure 处理，
///   不落 composer）；视图放弃的键由调用方落回 composer。
/// - **F4**（母本 `focus_activity` 默认键）＝活动聚焦入口（进入后由上一门接管）。
/// - 滚动键面（A3）：PageUp/PageDown（无修饰）、JumpTarget 跳转（Ctrl+Home/End、
///   Alt+</> 等）。Esc 不归视图——单一职责轮（012 §9）由驱动统一拦截（运行中＝
///   取消、空闲＝无动作）；回底替代入口＝Ctrl+End（JumpTarget::Latest）。
/// - 范围边界（010 未实施）：选择起点 Ctrl+Space、find 键、选区/搜索状态在此
///   构型下不可达，不另路由。
fn transcript_owns_key(
    view: &TranscriptView,
    key: &KeyEvent,
    turn_running: bool,
    popup_active: bool,
) -> bool {
    // 母本门（app.rs no_modal_or_popup_active）：弹窗/视图激活时输入归 composer
    if popup_active {
        return false;
    }
    if key.kind != KeyEventKind::Press {
        return false;
    }
    // 012：视图交互激活（活动聚焦；选区/搜索/复制模式在 010 前不可达）期间键归视图
    if view.has_active_interaction() {
        // §9 硬化：交互激活（选区/聚焦/搜索）期间 Esc 也不归视图——驱动前置
        // 拦截为主防线，此处兜底防止拦截重构后 Esc 落入选区清/Esc-to-latest
        if crate::key_hint::normalize_key_parts(key.code, key.modifiers).0 == KeyCode::Esc
            && key.modifiers.is_empty()
        {
            return false;
        }
        return true;
    }
    let (code, modifiers) = crate::key_hint::normalize_key_parts(key.code, key.modifiers);
    // 012：F4＝活动聚焦入口（母本 focus_activity 默认键，经 handle_key 的
    // handle_disclosure_key 臂接管）
    if code == KeyCode::F(4) && modifiers.is_empty() {
        return true;
    }
    if matches!(code, KeyCode::PageUp | KeyCode::PageDown) && modifiers.is_empty() {
        return true;
    }
    crate::transcript_view::JumpTarget::from_key(*key).is_some()
}

/// 空闲提交分类（008：排队输入重放与按键提交共用同一语义）——裸 `exit` 由驱动
/// 拦截退出（J10）；`/name`（首个词可解析为已知斜杠命令）派发命令；其余为 turn。
#[derive(Debug)]
enum IdleSubmission {
    Exit,
    Turn(String),
    Command { name: String, full: String },
}

fn classify_idle_submission(text: &str) -> Option<IdleSubmission> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "exit" {
        return Some(IdleSubmission::Exit);
    }
    // 斜杠判定用原文（前导空格＝普通文本，对齐 composer 空闲解析语义）。
    // 已知与否一律派发命令：宿主 run_command 对未知命令输出提示（与空闲按键
    // 路径的拒绝语义一致——不发给模型；008-REVIEW 独立审查 P1-2）
    if let Some(body) = text.strip_prefix('/') {
        let name = body.split_whitespace().next().unwrap_or("");
        if !name.is_empty() {
            return Some(IdleSubmission::Command {
                name: name.to_string(),
                full: trimmed.to_string(),
            });
        }
    }
    Some(IdleSubmission::Turn(trimmed.to_string()))
}

/// 008：turn 开始时的 cell 记账——推入用户消息 cell（母本 transcript 含用户
/// 消息；与 /resume 投影同类型 UserHistoryCell，实时视图与恢复视图一致）并清空
/// 本轮流式 cell 下标集合。
fn spawn_turn_cells(
    cells: &mut Vec<Arc<dyn HistoryCell>>,
    stream_indices: &mut Vec<usize>,
    text: &str,
) {
    stream_indices.clear();
    if !text.trim().is_empty() {
        cells.push(Arc::new(crate::history_cell::UserHistoryCell {
            spoken: false,
            message: text.to_string(),
            text_elements: Vec::new(),
            local_image_paths: Vec::new(),
            remote_image_urls: Vec::new(),
        }));
    }
}

/// 008：入队一条 turn 期间提交的输入并更新 composer 上方的排队预览（母本行为）。
fn queue_input(queued: &mut VecDeque<String>, pane: &mut BottomPane, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    queued.push_back(text.trim().to_string());
    pane.set_pending_input_preview(
        queued.iter().cloned().collect(),
        Vec::new(),
        Vec::new(),
    );
}

/// 应用命令结果到 transcript；返回 true 表示请求退出。
fn apply_command_outcome(
    outcome: CommandOutcome,
    cells: &mut Vec<Arc<dyn HistoryCell>>,
    transcript: &mut TranscriptView,
) -> bool {
    match outcome {
        CommandOutcome::Output(out) => push_info_lines(cells, &out),
        CommandOutcome::ReplaceCells { output, cells: new_cells } => {
            // 先替换历史投影，summary/degraded 告警渲染其后（否则被整体覆盖丢弃）
            *cells = new_cells;
            push_info_lines(cells, &output);
            transcript.jump_to_latest();
        }
        CommandOutcome::ShouldExit => return true,
    }
    false
}

fn handle_builtin_exit(name: &str) -> bool {
    matches!(name, "exit" | "quit")
}

fn new_stream(cwd: &std::path::Path) -> StreamController {
    StreamController::new_with_inline_visualizations(
        None,
        cwd,
        crate::history_cell::HistoryRenderMode::Rich,
        None,
    )
        .with_list_spacing(ListSpacing::Compact) // Owned 模式紧凑列表（chatwidget/streaming.rs:230）
}

fn finish_stream(
    stream: &mut Option<StreamController>,
    cells: &mut Vec<Arc<dyn HistoryCell>>,
    cwd: &std::path::Path,
    text: String,
    is_error: bool,
) {
    // 008-REVIEW：Final 携带完整原文，流式 controller 直接丢弃
    //（稳定区行已由驱动侧 truncate 移除，不重复）。
    let _ = stream.take();
    if text.trim().is_empty() && !is_error {
        return;
    }
    if is_error {
        cells.push(Arc::new(PlainHistoryCell::new(vec![ratatui::text::Line::from(
            ratatui::style::Stylize::dim(format!("错误：{text}")),
        )])));
    } else {
        cells.push(Arc::new(AgentMarkdownCell::new_with_inline_visualizations(
            text,
            cwd,
            /*inline_visualization_context*/ None,
        )));
    }
}

fn draw(
    tui: &mut Tui,
    pane: &BottomPane,
    transcript: &mut TranscriptView,
    scrollbar_rows: &mut crate::transcript_view::scrollbar::RowCache,
    cells: &[Arc<dyn HistoryCell>],
    stream: Option<&StreamController>,
) -> io::Result<()> {
    // 预取 screen 仅作 draw 的高度提示（内部 height.min(新尺寸) 只钳放大方向）；
    // 渲染排布一律以闭包内真实帧缓冲为准（见下）。
    let screen = tui.screen_size_for_layout()?;
    // 010：拖选期间指针停在视口边缘＝自动滚动（母本 Draw 事件内 tick_selection＋
    // schedule_frame_in 自续帧链；011 §7 接线——本地维持 FRAME_TICK 节奏＝既有
    // 边缘自动滚动速率，避免改变行为）
    let autoscroll_advanced = transcript.tick_selection(cells);
    let cells_ref = cells.clone();
    let now = std::time::Instant::now();
    // 011 §7：反馈到期延迟穿出渲染闭包（本地原先丢弃该返回值）
    let mut gap_expiry: Option<Duration> = None;
    let gap_expiry_ref = &mut gap_expiry;
    tui.draw(screen.height, move |frame| {
        let buf = frame.buffer_mut();
        // 008-A3 收尾（2026-10-15 竞态修复）：区域推导一律取自真实帧缓冲尺寸。
        // 预取的 screen 与帧缓冲在 SIGWINCH 到达窗口内可能不一致（尺寸监视在
        // 两次取值间更新），按预取布局写入新缓冲会越界 panic（composer 曾以旧
        // 行号 47 写 120x40 缓冲）——证据 docs/records/2026-10-15-008-a3-followup.md。
        let size = ratatui::layout::Size { width: buf.area.width, height: buf.area.height };
        let (transcript_area, pane_area) = split_screen_areas(pane, size);
        // 011：历史区最右 1 列为滚动条沟位（K1 常驻）；内容按剩余宽度折行，
        // 选区/折行/搜索全部基于同一收窄区域。沟位切分与渲染同取真实帧缓冲尺寸。
        let (transcript_content, transcript_gutter) = split_transcript_gutter(transcript_area);
        // 流式活动尾部（live tail）同步；无活动流时清除（防御：流结束后旧尾区
        // 行不得残留渲染——008-REVIEW 独立审查 P2-1）
        if let Some(controller) = stream {
            let lines = controller.current_tail_lines();
            let key = ActiveCellTranscriptKey {
                cacheable: false,
                revision: 0,
                is_stream_continuation: true,
                animation_tick: None,
            };
            transcript.sync_live_tail(transcript_content.width, Some(key), move |_| Some(lines.clone()));
        } else {
            transcript.sync_live_tail(transcript_content.width, None, |_| None);
        }
        use crate::render::renderable::Renderable as _;
        // 008-A1：光标定位——composer 聚焦时终端光标置于其编辑位（母本 chatwidget
        // 渲染行为；Frame 光标经 CustomTerminal 写终端）
        let cursor = pane.cursor_pos(pane_area);
        transcript.render(transcript_content, buf, &cells_ref);
        // 011：几何与渲染同帧（render 之后取布局快照；内容不足一屏时沟位留空）
        crate::transcript_view::scrollbar::render_gutter(
            transcript,
            &cells_ref,
            scrollbar_rows,
            transcript_gutter,
            buf,
        );
        // 010：提示沟行（transcript 与 pane 之间的保留行）——复制反馈（五态文案）
        // 与回底控制；无内容时留空。011 §7：反馈到期改经到期帧收口（下见
        // schedule_frame_in(delay)），不再依赖 tick 重绘
        let gap_area = (transcript_area.height < pane_area.y).then(|| {
            Rect { height: 1, ..transcript_content }
        });
        *gap_expiry_ref = transcript.render_composer_gap(gap_area, /*hint*/ None, buf, now);
        pane.render(pane_area, buf);
        if let Some((x, y)) = cursor {
            frame.set_cursor_position((x, y));
        }
    })?;
    // 011 §7：绘制后自续帧——边缘自动滚动推进中（保持 FRAME_TICK 节奏）与
    // 复制反馈到期（延迟到达时清屏收口）。Pending 反馈无到期（返回 None），
    // 由 Tick 剪贴板 poll 完成分派置脏收口
    if autoscroll_advanced {
        tui.frame_requester().schedule_frame_in(FRAME_TICK);
    }
    if let Some(delay) = gap_expiry {
        tui.frame_requester().schedule_frame_in(delay);
    }
    Ok(())
}

fn push_info_lines(cells: &mut Vec<Arc<dyn HistoryCell>>, out: &str) {
    for line in out.lines() {
        cells.push(Arc::new(PlainHistoryCell::new(vec![ratatui::text::Line::from(
            line.to_string(),
        )])));
    }
}

fn use_renderable_height(pane: &BottomPane, width: u16) -> u16 {
    use crate::render::renderable::Renderable as _;
    pane.desired_height(width).max(1)
}

/// 008-A3（滚动修复）：transcript/pane 区域推导（draw 渲染与鼠标分发共用——
/// 布局变动时两处同步，避免鼠标分区与渲染区漂移）。
/// 011：transcript 区切出内容区（宽−1）与最右 1 列沟位；宽 0 时两者皆空。
fn split_transcript_gutter(transcript_area: Rect) -> (Rect, Rect) {
    let content_width = transcript_area.width.saturating_sub(/*gutter*/ 1);
    let content = Rect {
        width: content_width,
        ..transcript_area
    };
    let gutter = Rect {
        x: transcript_area.x + content_width,
        width: transcript_area.width - content_width,
        ..transcript_area
    };
    (content, gutter)
}

fn split_screen_areas(
    pane: &BottomPane,
    screen: ratatui::layout::Size,
) -> (Rect, Rect) {
    let pane_height = use_renderable_height(pane, screen.width).min(screen.height);
    // 010：composer 上方恒保留 1 行提示沟（复制反馈/回底控制——
    // render_composer_gap 渲染目标行；母本由 pane ComposerRenderOptions 内含，
    // 本地以驱动侧保留行实现——draw 与鼠标分发共用本函数，几何单一来源）。
    // 极矮屏（不足 transcript+pane+2）放弃沟行，反馈不渲染。
    let gap = u16::from(screen.height > pane_height.saturating_add(2));
    let transcript_area = Rect {
        x: 0,
        y: 0,
        width: screen.width,
        height: screen.height.saturating_sub(pane_height + gap),
    };
    let pane_area = Rect {
        x: 0,
        y: transcript_area.height + gap,
        width: screen.width,
        height: pane_height,
    };
    (transcript_area, pane_area)
}


// ===== 公共投影入口（terminal-chat /resume）=====
// HistoryCell 为 pub(crate)；宿主不可见。此处以宿主无关的枚举
// （[`HostHistoryItem`]，语义对齐 terminal-chat::prompt::HistoryItem 的最小子集）
// 承接投影，内部走母本 thread_to_transcript_cells 同一实现——不重放工具。

/// 宿主历史条目（/resume 投影输入；012 起亦为实时工具终态的输入——同源同投影）。
#[derive(Debug, Clone)]
pub enum HostHistoryItem {
    /// 用户或助手文本消息。
    Text { assistant: bool, content: String },
    /// exec_command 终态。`command` 为空＝参数不可解析（降级展示，不伪造命令）；
    /// 成功＝`error` 为 None 且退出码 0；`cancelled`＝独立"已取消"呈现（非"拒绝"）。
    /// 016：`running`＝进程仍在运行（携带会话编号，write_stdin 可续接）；
    /// write_stdin 的结果复用本形状，`command`＝关联的原命令（不伪造新命令）。
    Exec {
        command: String,
        output: String,
        exit_code: Option<i32>,
        cancelled: bool,
        error: Option<String>,
        duration_ms: Option<u64>,
        running: Option<String>,
    },
    /// apply_patch 终态。`changes` 为空＝无真实差异数据（旧会话——明确降级，
    /// 不伪造 `+0 −0`）；`changed`＝降级文件清单（结果 JSON 路径）；
    /// `cancelled`＝批内跳过（独立"已取消"呈现，非失败）。
    Patch {
        ok: bool,
        cancelled: bool,
        state_known: bool,
        error: Option<String>,
        changed: Vec<String>,
        changes: Vec<HostPatchChange>,
    },
}

/// 012：apply_patch 逐文件真实差异（应用时的真实前后内容）。
#[derive(Debug, Clone)]
pub struct HostPatchChange {
    pub path: String,
    pub kind: HostPatchKind,
}

#[derive(Debug, Clone)]
pub enum HostPatchKind {
    /// 新增文件：真实新内容全文。
    Add { content: String },
    /// 删除文件：真实旧内容全文；`None`＝读入超限/失败（明确降级）。
    Delete { content: Option<String> },
    /// 修改文件：真实前后内容生成的统一 diff。
    Update { diff: String },
}

/// 将宿主历史投影为 transcript cells（静态渲染，不重放工具）。
///
/// 012：文本运行仍经母本 `thread_items_to_transcript_cells`（恢复视图文本渲染与
/// 008 行为一致）；工具项（Exec/Patch）经 [`project_tool_item`] 直接构造——与实时
/// ToolEnd 替换**同一实现**（实时终态与恢复历史同渲染，需求⑤）。
pub fn project_history(items: Vec<HostHistoryItem>) -> Vec<std::sync::Arc<dyn HistoryCell>> {
    use codex_app_server_protocol::UserInput;
    use codex_app_server_protocol::ThreadItem;
    use ratatui::style::Stylize as _;

    let cwd: PathBuf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let abs_cwd = codex_utils_absolute_path::AbsolutePathBuf::new(cwd.clone());
    let mut out: Vec<std::sync::Arc<dyn HistoryCell>> = Vec::new();
    let mut text_items: Vec<ThreadItem> = Vec::new();
    let mut next_id = 0usize;

    macro_rules! flush_text {
        () => {
            if !text_items.is_empty() {
                let cells = crate::thread_transcript::thread_items_to_transcript_cells(
                    /*thread_id*/ None,
                    &abs_cwd,
                    std::mem::take(&mut text_items),
                    crate::thread_transcript::RawReasoningVisibility::Hidden,
                    None,
                );
                out.extend(cells);
            }
        };
    }

    for item in items {
        next_id += 1;
        let id = format!("hist-{next_id}");
        match item {
            HostHistoryItem::Text { assistant, content } => {
                if assistant {
                    text_items.push(ThreadItem::AgentMessage { id, text: content });
                } else {
                    text_items.push(ThreadItem::UserMessage {
                        id,
                        client_id: None,
                        content: vec![UserInput::Text {
                            text: content,
                            text_elements: Vec::new(),
                        }],
                    });
                }
            }
            tool @ (HostHistoryItem::Exec { .. } | HostHistoryItem::Patch { .. }) => {
                flush_text!();
                out.extend(project_tool_item(&tool, &id, &cwd));
            }
        }
    }
    flush_text!();
    if out.is_empty() {
        // 008：空会话占位（与原 thread_to_transcript_cells 空投影行为一致）
        out.push(std::sync::Arc::new(PlainHistoryCell::new(vec![
            "No transcript content available".italic().dim().into(),
        ])));
    }
    out
}

/// 012：单个工具终态 → cells。实时 ToolEnd 替换与 `/resume` 投影共用本实现。
/// `id`＝disclosure activity id（`exec:<id>`/`patch:<id>`）：实时＝tool_call_id，
/// 恢复＝投影合成 id；运行态→终态替换后展开状态按 id 保持。
/// 012 §6：003 真实执行 argv（`exec.rs`＝`Command::new("/bin/sh").arg("-c").arg(cmd)`）。
/// 展示层按此存储——母本 `extract_bash_command`（`-lc|-c`＋Zsh/Bash/Sh）剥壳出原文，
/// 含空格/引号/中文/运算符的命令不被 `escape_command` 整条加引号；不伪造、不拆词。
fn shell_argv(cmd: &str) -> Vec<String> {
    vec!["/bin/sh".to_owned(), "-c".to_owned(), cmd.to_owned()]
}

fn project_tool_item(
    item: &HostHistoryItem,
    id: &str,
    cwd: &std::path::Path,
) -> Vec<std::sync::Arc<dyn HistoryCell>> {
    use ratatui::style::Stylize as _;
    match item {
        HostHistoryItem::Text { .. } => Vec::new(), // 文本经 thread 投影，不在此处理
        HostHistoryItem::Exec { command, output, exit_code, cancelled, error, duration_ms, running } => {
            if let Some(session_id) = running {
                // 016：进程仍在运行——独立"运行中"呈现，携带可续接会话编号；
                // 不误报完成，输出如实展示已收集部分
                let mut lines = Vec::new();
                if !command.trim().is_empty() {
                    lines.push(ratatui::text::Line::from(format!("$ {command}")).dim());
                }
                lines.push(ratatui::text::Line::from(format!(
                    "仍在运行（会话 {session_id}）；可用 write_stdin 查询新输出或继续交互"
                ).dim()));
                if let Some(err) = error {
                    lines.push(ratatui::text::Line::from(err.clone()).dim());
                }
                for line in output.lines() {
                    lines.push(ratatui::text::Line::from(format!("  {line}")).dim());
                }
                return vec![Arc::new(PlainHistoryCell::new(lines))];
            }
            if *cancelled {
                // 需求④：取消明确显示为"已取消"，不机械映射"拒绝"；输出与原因如实呈现
                return vec![Arc::new(cancelled_exec_cell(command, error.as_deref(), output))];
            }
            if command.trim().is_empty() {
                // 参数不可解析：无命令可展示——降级行（不伪造命令），但输出与
                // 状态仍按结果 JSON 呈现（012 §3-K4：留空降级≠丢信息）
                let status = if *exit_code == Some(0) && error.is_none() {
                    "exec_command 已完成".to_string()
                } else {
                    format!("exec_command 失败（退出码 {code}）", code = exit_code.unwrap_or(1))
                };
                let mut lines = vec![ratatui::text::Line::from(status).dim()];
                if let Some(err) = error {
                    lines.push(ratatui::text::Line::from(err.clone()).dim());
                }
                for line in output.lines() {
                    lines.push(ratatui::text::Line::from(format!("  {line}")).dim());
                }
                return vec![Arc::new(PlainHistoryCell::new(lines))];
            }
            let success = error.is_none() && *exit_code == Some(0);
            // 失败且无整数退出码（超时/错误/被信号终止）按母本中断惯例记 1
            let code = if success { 0 } else { exit_code.unwrap_or(1) };
            // 失败原因（超时/取消外错误）并入输出展示——真实字段，不新增结论
            let mut aggregated = output.clone();
            if !success && let Some(err) = error.as_ref().filter(|e| !e.trim().is_empty()) {
                if !aggregated.is_empty() {
                    aggregated.push('\n');
                }
                aggregated.push_str(err);
            }
            let mut cell = crate::exec_cell::new_active_exec_command(
                id.to_string(),
                // 012 §6：按 003 真实执行 argv 存储（exec.rs＝Command::new("/bin/sh")
                // .arg("-c").arg(cmd)）——母本剥壳展示原文，实时/恢复同一路径
                shell_argv(command),
                Vec::new(),            // parsed 留空：is_exploring_call 要求非空，不误分组
                codex_app_server_protocol::CommandExecutionSource::Agent,
                /*interaction_input*/ None,
                /*animations_enabled*/ false,
            );
            let done = cell.complete_call(
                id,
                crate::exec_cell::CommandOutput::new(code, aggregated),
                Duration::from_millis(duration_ms.unwrap_or(0)),
            );
            debug_assert!(done, "新构造 cell 必含自身 call_id");
            vec![Arc::new(cell)]
        }
        HostHistoryItem::Patch { ok, cancelled, state_known, error, changed, changes } => {
            // 需求④：批内跳过的补丁＝独立"已取消"（非失败、非"拒绝"）
            if *cancelled {
                let mut lines = vec![ratatui::text::Line::from("apply_patch 已取消").dim()];
                if let Some(err) = error {
                    lines.push(ratatui::text::Line::from(err.clone()).dim());
                }
                return vec![Arc::new(PlainHistoryCell::new(lines))];
            }
            let mut out: Vec<Arc<dyn HistoryCell>> = Vec::new();
            if !changes.is_empty() {
                let map: std::collections::HashMap<PathBuf, crate::diff_model::FileChange> =
                    changes
                        .iter()
                        .map(|c| {
                            let kind = match &c.kind {
                                HostPatchKind::Add { content } => {
                                    crate::diff_model::FileChange::Add { content: content.clone() }
                                }
                                HostPatchKind::Delete { content } => {
                                    crate::diff_model::FileChange::Delete {
                                        content: content.clone().unwrap_or_default(),
                                    }
                                }
                                HostPatchKind::Update { diff } => {
                                    crate::diff_model::FileChange::Update {
                                        unified_diff: diff.clone(),
                                        move_path: None,
                                    }
                                }
                            };
                            (PathBuf::from(&c.path), kind)
                        })
                        .collect();
                out.push(Arc::new(
                    crate::history_cell::new_patch_event(map, cwd)
                        .with_activity_id(id.to_string()),
                ));
            }
            if !ok {
                // state_known=false：写入/删除中途失败，相关文件可能被部分修改
                // ——如实标注（012 §5-A2），不得静默
                let mut detail = error.clone().unwrap_or_default();
                if !state_known {
                    detail.push_str("（变更状态未知，相关文件可能被部分修改，须重读）");
                }
                out.push(Arc::new(crate::history_cell::new_patch_apply_failure(detail)));
            }
            if out.is_empty() {
                // 需求⑥：缺真实差异＝明确降级（不伪造空 diff 的 +0 −0 计数）
                out.push(Arc::new(degraded_patch_cell(*ok, error.as_deref(), changed)));
            }
            out
        }
    }
}

/// 012：取消的 exec 呈现（独立"已取消"，非"拒绝"）——形状对齐母本
/// historical_tool_fallback（暗色 `$ cmd`＋状态行＋输出），文案为本地明确语义。
fn cancelled_exec_cell(command: &str, error: Option<&str>, output: &str) -> PlainHistoryCell {
    use ratatui::style::Stylize as _;
    let mut lines = Vec::new();
    if !command.trim().is_empty() {
        lines.push(ratatui::text::Line::from(format!("$ {command}")).dim());
    }
    let mut status = "已取消".to_string();
    if let Some(err) = error.filter(|e| !e.trim().is_empty()) {
        status.push_str("（");
        status.push_str(err);
        status.push('）');
    }
    lines.push(ratatui::text::Line::from(status).dim());
    for line in output.lines() {
        lines.push(ratatui::text::Line::from(format!("  {line}")).dim());
    }
    PlainHistoryCell::new(lines)
}

/// 012：旧会话补丁记录的降级呈现——文件清单＋明示缺差异，不伪造计数。
fn degraded_patch_cell(
    ok: bool,
    error: Option<&str>,
    changed: &[String],
) -> PlainHistoryCell {
    use ratatui::style::Stylize as _;
    let title = if ok { "已应用补丁（早期记录未保存差异详情）" } else { "补丁未成功（早期记录未保存差异详情）" };
    let mut lines = vec![ratatui::text::Line::from(title).dim()];
    if let Some(err) = error.filter(|e| !e.trim().is_empty()) {
        lines.push(ratatui::text::Line::from(err.to_string()).dim());
    }
    for path in changed {
        lines.push(ratatui::text::Line::from(format!("  └ {path}")).dim());
    }
    PlainHistoryCell::new(lines)
}

/// 宿主侧 cell 句柄（HistoryCell 已按 008-ADAPT 提为 pub）。
pub type HostCell = std::sync::Arc<dyn HistoryCell>;

/// 008-P3 公共入口：初始化终端（raw mode/备用屏策略）→ 组装 Tui → 运行主循环。
/// 终端恢复由 Tui/守卫 Drop 承担（母本语义）。
pub async fn run_interactive(hooks: DriverHooks) -> io::Result<DriverExit> {
    let initialized = crate::tui::init()?;
    let mut tui = Tui::new(
        initialized.terminal,
        initialized.enhanced_keys_supported,
        initialized.stderr_guard,
    );
    tui.terminal_app_over_ssh = initialized.terminal_app_over_ssh;
    run(tui, hooks).await
}

/// 008-ADAPT：TERM 未设置时的能力探测（terminal-detection；dumb/未知视为不支持）。
pub fn terminal_supported() -> bool {
    let info = codex_terminal_detection::terminal_info();
    !matches!(info.name, codex_terminal_detection::TerminalName::Dumb)
}

// ===== A7 安全门禁测试所需的公共再导出（008-ADAPT：门禁直查真实渲染管线；
// 底层项已按需提 pub 并登记）=====
pub use crate::test_backend::VT100Backend;

#[cfg(test)]
mod tests {
    use super::classify_idle_submission;
    use super::HostHistoryItem;
    use super::HostPatchChange;
    use super::HostPatchKind;
    use super::IdleSubmission;
    use super::project_tool_item;
    use super::transcript_owns_key;
    use super::TranscriptView;

    /// 008-J10：裸 exit 与空输入分类。
    #[test]
    fn classify_bare_exit_and_empty() {
        assert!(matches!(classify_idle_submission("exit"), Some(IdleSubmission::Exit)));
        assert!(matches!(classify_idle_submission("  exit  "), Some(IdleSubmission::Exit)));
        assert!(classify_idle_submission("").is_none());
        assert!(classify_idle_submission("   ").is_none());
    }

    /// 已知斜杠命令（含带参与尾随空白）分类为命令派发；full 保留 trim 后原文。
    /// 前导空格的斜杠文本＝普通文本（对齐 composer 空闲解析语义）。
    #[test]
    fn classify_known_slash_command() {
        match classify_idle_submission("/save seed") {
            Some(IdleSubmission::Command { name, full }) => {
                assert_eq!(name, "save");
                assert_eq!(full, "/save seed");
            }
            other => panic!("expect Command, got {other:?}"),
        }
        match classify_idle_submission("/models  ") {
            Some(IdleSubmission::Command { name, full }) => {
                assert_eq!(name, "models");
                assert_eq!(full, "/models");
            }
            other => panic!("expect Command, got {other:?}"),
        }
        assert!(matches!(
            classify_idle_submission(" /models"),
            Some(IdleSubmission::Turn(_))
        ));
    }

    /// 未知斜杠命令也派发命令通道（宿主 run_command 输出未知命令提示）——
    /// 与空闲按键路径的拒绝语义一致：不发给模型（008-REVIEW 独立审查 P1-2）。
    #[test]
    fn classify_unknown_slash_routes_to_command_not_model() {
        match classify_idle_submission("/definitely-not-a-command arg") {
            Some(IdleSubmission::Command { name, full }) => {
                assert_eq!(name, "definitely-not-a-command");
                assert_eq!(full, "/definitely-not-a-command arg");
            }
            other => panic!("expect Command, got {other:?}"),
        }
    }

    /// 普通文本与裸 quit 分类为 turn（quit 仅在斜杠形式 /quit 时为内建退出）。
    #[test]
    fn classify_plain_text_and_bare_quit_are_turns() {
        assert!(matches!(
            classify_idle_submission("你好 world"),
            Some(IdleSubmission::Turn(_))
        ));
        assert!(matches!(classify_idle_submission("quit"), Some(IdleSubmission::Turn(_))));
    }

    /// A3/012＋§9（单一职责轮）：键路由契约——分页/跳转/F4 归 transcript；
    /// Esc 一律不归视图（驱动统一拦截：运行中＝取消、空闲＝无动作）。
    #[test]
    fn scroll_key_routing_contract() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;
        let key = |c: KeyCode, m: KeyModifiers| KeyEvent::new(c, m);
        // 分页（无修饰）
        assert!(transcript_owns_key(
            &TranscriptView::default(),
            &key(KeyCode::PageUp, KeyModifiers::NONE),
            /*turn_running*/ false,
            /*popup_active*/ false
        ));
        assert!(transcript_owns_key(
            &TranscriptView::default(),
            &key(KeyCode::PageDown, KeyModifiers::NONE),
            /*turn_running*/ false,
            /*popup_active*/ false
        ));
        // 带修饰的分页不路由（Shift+PageUp 等留给 composer，母本同语义）
        assert!(!transcript_owns_key(
            &TranscriptView::default(),
            &key(KeyCode::PageUp, KeyModifiers::SHIFT),
            /*turn_running*/ false,
            /*popup_active*/ false
        ));
        // 跳转键（Ctrl+Home/End）
        assert!(transcript_owns_key(
            &TranscriptView::default(),
            &key(KeyCode::Home, KeyModifiers::CONTROL),
            /*turn_running*/ false,
            /*popup_active*/ false
        ));
        assert!(transcript_owns_key(
            &TranscriptView::default(),
            &key(KeyCode::End, KeyModifiers::CONTROL),
            /*turn_running*/ false,
            /*popup_active*/ false
        ));
        // 012：F4（活动聚焦入口，无修饰）归 transcript；带修饰不路由
        assert!(transcript_owns_key(
            &TranscriptView::default(),
            &key(KeyCode::F(4), KeyModifiers::NONE),
            false,
            false
        ));
        assert!(!transcript_owns_key(
            &TranscriptView::default(),
            &key(KeyCode::F(4), KeyModifiers::SHIFT),
            false,
            false
        ));
        // 范围边界：选择起点/普通字符/上下键（输入历史）不路由
        for (c, m) in [
            (KeyCode::Char(' '), KeyModifiers::CONTROL),
            (KeyCode::Char('a'), KeyModifiers::NONE),
            (KeyCode::Up, KeyModifiers::NONE),
            (KeyCode::Down, KeyModifiers::NONE),
            (KeyCode::Enter, KeyModifiers::NONE),
        ] {
            assert!(
                !transcript_owns_key(&TranscriptView::default(), &key(c, m), false, false),
                "不应路由 {c:?}"
            );
        }
    }

    /// §9 单一职责轮：Esc 归属——任何状态（空闲/运行中、浏览/跟随、弹窗）一律
    /// 不归 transcript（驱动统一拦截）；Ctrl+End 承接回底。
    #[test]
    fn esc_never_routes_to_transcript() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;
        // 构造已浏览状态：带一个多行 cell 上翻，使 position 离开 Latest
        let mut browsing = TranscriptView::default();
        let cells: Vec<std::sync::Arc<dyn crate::history_cell::HistoryCell>> = vec![
            std::sync::Arc::new(crate::history_cell::PlainHistoryCell::new(
                (0..30).map(|i| ratatui::text::Line::from(format!("行{i}"))).collect(),
            )),
        ];
        // 经真实 render 设置视口区域（scroll 在 area 为空时早退）
        let area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 20 };
        let mut buf = ratatui::buffer::Buffer::empty(area);
        browsing.render(area, &mut buf, &cells);
        browsing.scroll(&cells, /*rows*/ -3);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(browsing.can_return_to_latest());
        // §9：Esc 一律不归视图（空闲浏览态也不回底——回底＝Ctrl+End）
        assert!(!transcript_owns_key(&browsing, &esc, /*turn_running*/ false, /*popup_active*/ false));
        assert!(!transcript_owns_key(&browsing, &esc, /*turn_running*/ true, /*popup_active*/ false));
        let following = TranscriptView::default();
        assert!(!following.can_return_to_latest());
        assert!(!transcript_owns_key(&following, &esc, false, false));
        // Ctrl+End（JumpTarget::Latest）承接回底——浏览态仍归视图
        assert!(transcript_owns_key(
            &browsing,
            &KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL),
            false,
            false
        ));
        // 弹窗/视图激活时输入归 composer（母本 no_modal_or_popup_active 门）
        assert!(!transcript_owns_key(
            &browsing,
            &KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            false,
            /*popup_active*/ true
        ));
    }

    /// 012：投影文本助手——cells 的 display_lines 拼接为可断言文本。
    fn cells_text(cells: &[std::sync::Arc<dyn crate::history_cell::HistoryCell>]) -> String {
        use crate::history_cell::HistoryCell as _;
        let mut out = String::new();
        for cell in cells {
            for line in cell.display_lines(80) {
                for span in line.spans {
                    out.push_str(&span.content);
                }
                out.push('\n');
            }
        }
        out
    }

    /// 012 §6 C3：命令展示与 003 真实执行 argv（["/bin/sh","-c",<cmd>]）一致——
    /// 母本剥壳链剥出原文；含空格/嵌套引号/中文/shell 运算符的命令逐字可见、
    /// 不被整条包裹引号（旧实现单元素 argv 经 escape_command 整体加引号）。
    #[test]
    fn exec_command_displays_verbatim_without_shell_wrapping() {
        let cases = [
            "echo t012-ok",
            "echo \"a b\" | grep 'x && y'",
            "printf '中文测试 输出'",
            "cd /tmp && rm -f x || echo failed",
        ];
        for cmd in cases {
            let cells = project_tool_item(
                &HostHistoryItem::Exec {
                    command: cmd.into(),
                    output: String::new(),
                    exit_code: Some(0),
                    cancelled: false,
                    error: None,
                    duration_ms: Some(3),
                running: None,
                },
                "call-argv",
                std::path::Path::new("/"),
            );
            let text = cells_text(&cells);
            assert!(
                text.contains(&format!("Ran {cmd}")),
                "命令原文紧随标题逐字可见：{cmd}\n{text}"
            );
            assert!(
                !text.contains(&format!("Ran '{cmd}'")),
                "不得整条包裹引号：{cmd}\n{text}"
            );
        }
    }

    /// 012 需求④：取消＝独立"已取消"呈现——不出现"拒绝/Declined"文案；
    /// 命令与（部分）输出如实展示。
    #[test]
    fn cancelled_exec_projects_explicit_cancelled_label() {
        let cells = project_tool_item(
            &HostHistoryItem::Exec {
                command: "sleep 30".into(),
                output: "partial-out".into(),
                exit_code: None,
                cancelled: true,
                error: Some("执行已取消，已终止进程组".into()),
                duration_ms: None,
                running: None,
            },
            "call-c",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("$ sleep 30"), "命令可见：{text}");
        assert!(text.contains("已取消"), "取消文案：{text}");
        assert!(!text.contains("Declined") && !text.contains("拒绝"), "不得映射拒绝：{text}");
        assert!(text.contains("partial-out"), "部分输出保留：{text}");
        // 未执行（批内跳过）形态：无输出行也成立
        let cells = project_tool_item(
            &HostHistoryItem::Exec {
                command: "touch x".into(),
                output: String::new(),
                exit_code: None,
                cancelled: true,
                error: Some("用户取消，本调用未执行".into()),
                duration_ms: None,
                running: None,
            },
            "call-s",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("已取消") && text.contains("用户取消，本调用未执行"), "{text}");
    }

    /// 012 需求④（审查轮修复回归）：批内跳过的 apply_patch＝"已取消"，
    /// 不呈现 "Failed to apply patch"（失败 cell）。
    #[test]
    fn cancelled_patch_projects_explicit_cancelled_not_failure() {
        let cells = project_tool_item(
            &HostHistoryItem::Patch {
                ok: false,
                cancelled: true,
                state_known: true,
                error: Some("用户取消，本调用未执行".into()),
                changed: Vec::new(),
                changes: Vec::new(),
            },
            "call-pc",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("apply_patch 已取消"), "取消文案：{text}");
        assert!(text.contains("用户取消，本调用未执行"), "原因如实：{text}");
        assert!(!text.contains("Failed to apply patch"), "不得呈现为失败：{text}");
        // state_known=false 的真实失败：失败 cell＋状态未知标注（012 §5-A2）
        let cells = project_tool_item(
            &HostHistoryItem::Patch {
                ok: false,
                cancelled: false,
                state_known: false,
                error: Some("写入修改目标失败 /w/a.txt".into()),
                changed: Vec::new(),
                changes: Vec::new(),
            },
            "call-pf",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("Failed to apply patch"), "{text}");
        assert!(text.contains("变更状态未知"), "状态未知须标注：{text}");
    }

    /// 012 §3-K4（审查轮修复回归）：exec 缺 arguments.cmd＝命令留空降级，
    /// 输出与状态仍按结果 JSON 呈现（不丢信息）。
    #[test]
    fn exec_without_cmd_keeps_output_and_status() {
        let cells = project_tool_item(
            &HostHistoryItem::Exec {
                command: String::new(),
                output: "partial-out-line".into(),
                exit_code: Some(3),
                cancelled: false,
                error: None,
                duration_ms: None,
                running: None,
            },
            "call-nc",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("退出码 3"), "状态保留：{text}");
        assert!(text.contains("partial-out-line"), "输出保留：{text}");
        assert!(!text.contains("$"), "不伪造空命令行：{text}");
    }

    /// 012：成功/失败 exec 终态——命令、输出、退出码经真实 ExecCell 渲染。
    #[test]
    fn exec_terminal_states_render_via_exec_cell() {
        let ok = project_tool_item(
            &HostHistoryItem::Exec {
                command: "echo t012-ok".into(),
                output: "t012-ok\n".into(),
                exit_code: Some(0),
                cancelled: false,
                error: None,
                duration_ms: Some(12),
                running: None,
            },
            "call-ok",
            std::path::Path::new("/"),
        );
        let text = cells_text(&ok);
        assert!(text.contains("echo t012-ok"), "命令：{text}");
        assert!(text.contains("t012-ok"), "输出：{text}");
        assert!(!text.contains("Failed"), "成功不得显示失败：{text}");
        let bad = project_tool_item(
            &HostHistoryItem::Exec {
                command: "exit 7".into(),
                output: String::new(),
                exit_code: Some(7),
                cancelled: false,
                error: None,
                duration_ms: None,
                running: None,
            },
            "call-bad",
            std::path::Path::new("/"),
        );
        let text = cells_text(&bad);
        assert!(text.contains("7"), "退出码可见：{text}");
    }

    /// 012 需求⑥：旧会话补丁（无真实 diff）＝明确降级——不伪造 +0 −0 计数；
    /// 失败补丁呈现失败 cell；成功补丁呈现真实差异 cell。
    #[test]
    fn patch_projection_degrades_and_renders_diff() {
        // 降级：changes 空＋changed 清单
        let cells = project_tool_item(
            &HostHistoryItem::Patch {
                ok: true,
                cancelled: false,
                state_known: true,
                error: None,
                changed: vec!["/w/a.txt".into(), "/w/b.txt".into()],
                changes: Vec::new(),
            },
            "hist-p",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("早期记录未保存差异详情"), "明示降级：{text}");
        assert!(text.contains("/w/a.txt") && text.contains("/w/b.txt"), "清单保留：{text}");
        assert!(!text.contains("+0"), "不得伪造计数：{text}");
        // 失败：PatchFailureCell（含错误信息）
        let cells = project_tool_item(
            &HostHistoryItem::Patch {
                ok: false,
                cancelled: false,
                state_known: true,
                error: Some("第 3 行：更新块不包含任何变更行".into()),
                changed: Vec::new(),
                changes: Vec::new(),
            },
            "hist-f",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("Failed to apply patch"), "失败标题：{text}");
        assert!(text.contains("更新块"), "错误细节：{text}");
        // 成功：真实 diff 渲染（路径＋增删行）
        let cells = project_tool_item(
            &HostHistoryItem::Patch {
                ok: true,
                cancelled: false,
                state_known: true,
                error: None,
                changed: vec!["/w/u.txt".into()],
                changes: vec![HostPatchChange {
                    path: "/w/u.txt".into(),
                    kind: HostPatchKind::Update {
                        diff: "--- a\n+++ b\n@@ -1 +1 @@\n-old-line\n+new-line".into(),
                    },
                }],
            },
            "hist-u",
            std::path::Path::new("/"),
        );
        let text = cells_text(&cells);
        assert!(text.contains("u.txt"), "路径：{text}");
        assert!(text.contains("new-line"), "新增行：{text}");
        assert!(text.contains("old-line"), "删除行（diff 上下文）：{text}");
    }

    /// Ctrl+D/Ctrl+C 判定排除冲突修饰键（Shift+Enter 是换行、AltGr 含 ALT——
    /// 008-REVIEW 独立审查 P2-6）。
    #[test]
    fn ctrl_key_detection_excludes_conflicting_modifiers() {
        use crossterm::event::KeyCode;
        use crossterm::event::KeyEvent;
        use crossterm::event::KeyModifiers;
        let ctrl_d = |m: KeyModifiers| {
            super::is_ctrl_d(&KeyEvent::new(KeyCode::Char('d'), m))
        };
        assert!(ctrl_d(KeyModifiers::CONTROL));
        assert!(!ctrl_d(KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        assert!(!ctrl_d(KeyModifiers::CONTROL | KeyModifiers::ALT));
    }
}
