//! terminal-chat：终端循环 + 最小 LLM 对话与模型目录入口。
//! 命令：/models 查看有效名单；/refresh 刷新目录；其余输入发送给所选模型。

use terminal_chat::prompt::HistoryItem;
use terminal_chat::proc_session::ProcessManager;
use terminal_chat::{api, catalog, display, session, turn};
use terminal_chat::tui_host;

use std::sync::atomic::{AtomicBool, Ordering};

use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, IsTerminal, Write};
use std::thread::sleep;
use std::time::Duration;

/// 显示边界：所有含不可信来源内容（模型回复、远端错误、目录元数据）的输出
/// 统一经此过滤终端控制序列后打印。
fn say(text: &str) {
    println!("{}", display::sanitize_for_terminal(text));
}

/// 工具执行期的取消标志（N2b）：SIGINT 只置位，由 exec 轮询路径停止本次等待；
/// 016 起取消不再自动终止已启动进程（见 proc_session）。
static TOOL_CANCEL: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_sig: libc::c_int) {
    TOOL_CANCEL.store(true, Ordering::SeqCst);
}

/// 020 A04：启动图片参数解析——`-i <路径>`、`--image <路径>`、`--image=<路径>`；
/// 每个值按逗号拆分，重复参数按出现顺序累积。其余参数不识别也不报错
///（不新增位置参数提示词）。值缺失按明确错误退出。
fn parse_image_args(args: impl Iterator<Item = String>) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = Vec::new();
    let mut iter = args.peekable();
    while let Some(arg) = iter.next() {
        let value = if let Some(v) = arg.strip_prefix("--image=") {
            Some(v.to_string())
        } else if arg == "-i" || arg == "--image" {
            match iter.next() {
                Some(v) => Some(v),
                None => return Err(format!("{arg} 缺少图片路径参数")),
            }
        } else {
            continue;
        };
        for part in value.expect("已取值").split(',') {
            let part = part.trim();
            if !part.is_empty() {
                out.push(std::path::PathBuf::from(part));
            }
        }
    }
    Ok(out)
}

fn main() {
    // .env 是环境变量的可选来源：启动时载入，已存在的变量不覆盖
    catalog::load_dotenv_from(std::path::Path::new(".env"));
    // 020 A04：-i/--image 启动图片（TUI 存宿主待就绪提交；行式随首条输入）
    let startup_images = match parse_image_args(std::env::args().skip(1)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("错误：{e}");
            std::process::exit(2);
        }
    };

    // 008-P3 交互入口切换：交互终端且 TERM 可用 → TUI（Owned 全屏）；否则行式原路径。
    // TUI 模式跳过开场横幅（J10 显示层替代——母本 startup/empty state 接管首屏）。
    if tui_host::tui_should_run() {
        let agent = api::agent();
        let mut autosave = session::Session::new(session::sessions_dir());
        let host = std::sync::Arc::new(tui_host::TuiHost::new(agent.clone(), {
            let mut s = session::Session::new(session::sessions_dir());
            std::mem::swap(&mut s, &mut autosave);
            autosave
        }));
        let mut rl = DefaultEditor::new().expect("初始化行编辑器失败");
        // F7 权限审计：与行式同源同参（内容/脱敏/收紧行为不变）；置于授权前＝
        // 行式"审计→读配置"顺序（0000 权限等角落先自愈再读，独立复审 P2-4）；
        // TUI 以启动通知 cell 展示（008-REVIEW 独立审查 P2-7）
        let startup_notices = catalog::audit_file_permissions(
            Some(std::path::Path::new(".env")),
            &catalog::config_dir(),
        );
        // 启动期 origin 授权（raw mode 之前的普通终端交互）
        if let Err(e) = host.authorize_startup_origin(&mut |q| prompt_y_n(&mut rl, q)) {
            println!("错误：{e}");
            std::process::exit(1);
        }
        // 020：启动图片直传驱动（就绪后提交一次）
        let exit = run_tui(host.clone(), startup_images.clone(), startup_notices);
        // 008：退出先取消仍在运行的 turn（016：取消只停止等待与后续调用，
        // 不终止已启动进程）；随后显式关闭全部进程会话与 MCP 连接并回收，
        // 再排空会话——不依赖析构（TUI 入口使用 process::exit；022 §2.5）。
        host.set_cancel(true);
        host.close_process_sessions();
        host.close_mcp();
        // 007：退出排空——与行式路径同契约（TUI 内 on_history 写失败的滞留待写段
        // 在退出前补写；008-REVIEW 独立审查 P0-2）
        if let Err(e) = host.flush_final_session() {
            say(&format!("错误：会话退出排空失败：{e}"));
        }
        // 终端恢复后打印（001 契约）
        println!("再见!");
        std::process::exit(match exit {
            Ok(_) => 0,
            Err(_) => 1,
        });
    }

    banner();

    // 权限审计（F7）：.env/配置文件收紧、上级目录可写告警；只读元数据，不含秘密值
    for line in catalog::audit_file_permissions(Some(std::path::Path::new(".env")), &catalog::config_dir()) {
        say(&line);
    }

    let agent = api::agent();
    let mut rl = DefaultEditor::new().expect("初始化行编辑器失败");
    // 006：会话历史（内存态）；007：默认自动追加落盘，/resume 恢复继续
    let mut history: Vec<HistoryItem> = Vec::new();
    let mut autosave = session::Session::new(session::sessions_dir());
    // 020：-i/--image 启动图片路径（正式提交时读取——首条可提交输入后作废）
    let mut pending_images: Vec<std::path::PathBuf> = startup_images;
    // 016：进程会话管理器（当前对话；/resume 成功切换后重置）
    let procs = ProcessManager::new();
    // 013-M7：threads 登记推进器（跨轮保持缓存库连接；enabled=false 零副作用）
    let mut registrar = terminal_chat::memory::runner::ThreadRegistrar::default();
    // 022：MCP 管理器（跨轮保留连接；退出显式关闭——不依赖 Drop）
    let mcp = terminal_chat::mcp::manager::McpManager::new();

    loop {
        match rl.readline("你> ") {
            Ok(line) => {
                // 非法字节会变成替换符 \u{FFFD}，过滤掉它，保证处理干净无乱码
                let input: String = line.chars().filter(|&c| c != '\u{FFFD}').collect();
                let input = input.trim();

                if !input.is_empty() {
                    // 加入内存历史,按上键可回溯
                    let _ = rl.add_history_entry(input);
                }

                if input == "exit" {
                    break;
                }

                // 空输入不打印空行,直接继续循环
                if !input.is_empty() {
                    let quit = handle_input(
                        &agent,
                        &mut rl,
                        input,
                        &mut history,
                        &mut autosave,
                        &mut registrar,
                        &procs,
                        &mut pending_images,
                        &mcp,
                    );
                    if quit {
                        break;
                    }
                }
            }
            // Ctrl+C:打断当前输入,直接退出
            Err(ReadlineError::Interrupted) => break,
            // Ctrl+D / EOF:正常退出；020：仍有启动图片时允许纯图恰一次（A04）
            Err(ReadlineError::Eof) => {
                if !pending_images.is_empty() {
                    let _ = handle_input(
                        &agent,
                        &mut rl,
                        "",
                        &mut history,
                        &mut autosave,
                        &mut registrar,
                        &procs,
                        &mut pending_images,
                        &mcp,
                    );
                }
                break;
            }
            // 其他读取错误(如管道输入含非法 UTF-8):不 panic,按退出处理
            Err(_) => break,
        }
    }

    // 016：宿主退出——显式关闭全部进程会话并回收，再完成会话排空和退出。
    procs.close();
    // 022：退出显式关闭全部 MCP 连接（stdio 有界 3 秒＋进程组回收）。
    mcp.close();
    // 007：退出排空——主循环退出后、进程结束前补写一次；成功静默，失败明确报错
    if let Err(e) = autosave.flush_final(&history) {
        say(&format!("错误：会话退出排空失败：{e}"));
    }
    // 结束提示:exit / Ctrl+C / Ctrl+D 都会走到这里
    println!("再见!");
}

/// 008-P3：组装驱动钩子并运行 TUI。
fn run_tui(
    host: std::sync::Arc<tui_host::TuiHost>,
    startup_images: Vec<std::path::PathBuf>,
    startup_notices: Vec<String>,
) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let host_for_turn = host.clone();
        let host_for_cmd = host.clone();
        let host_cancel = host.clone();
        let host_steer = host.clone();
        // 020：-i/--image 启动图片→驱动就绪后的首次提交（纯图一次）
        let initial_images = startup_images;
        let initial_submission = (!initial_images.is_empty()).then(|| {
            tchat_tui::chat_driver::SubmittedTurn {
                text: String::new(),
                local_images: initial_images
                    .into_iter()
                    .map(|path| tchat_tui::bottom_pane::LocalImageAttachment {
                        placeholder: String::new(),
                        path,
                    })
                    .collect(),
                text_elements: Vec::new(),
            }
        });
        let hooks = tchat_tui::chat_driver::DriverHooks {
            spawn_turn: Box::new(move |submission, tx| host_for_turn.spawn_turn(submission, tx)),
            run_command: Box::new(move |full| host_for_cmd.run_command(full)),
            set_cancel_requested: Box::new(move |requested| host_cancel.set_cancel(requested)),
            // 021：运行中接续提交——宿主短锁原子判定受理或"当前轮已结束"
            submit_steer: Box::new(move |content| host_steer.submit_steer(content)),
            initial_submission,
            startup_notices,
        };
        let result = tchat_tui::chat_driver::run_interactive(hooks).await;
        // 008-A5：终端恢复（母本语义：退出时离开备用屏＋禁用 raw mode/键盘增强/焦点）
        if let Err(err) = tchat_tui::tui::restore_after_exit() {
            eprintln!("警告：终端模式恢复失败：{err}");
        }
        result.map(|_| ()).map_err(|e| e.to_string())
    })
}

fn banner() {
    // 开场动画:转圈约 0.6s 后打印欢迎横幅;输出接管道(非终端)时跳过动画帧
    let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    if io::stdout().is_terminal() {
        for frame in frames {
            print!("\r启动中 {frame}");
            let _ = io::stdout().flush();
            sleep(Duration::from_millis(60));
        }
        println!();
    }
    println!("==============================");
    println!("  欢迎使用 Terminal Chat!");
    println!("  请输入消息,按回车发送,输入 exit 或 Ctrl+D 退出");
    println!("==============================");
}

/// 交互确认：仅接受 y/yes，其他输入（含读取错误）一律视为拒绝。
fn prompt_y_n(rl: &mut DefaultEditor, question: &str) -> bool {
    match rl.readline(question) {
        Ok(line) => matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

/// 018：唯一终端读取者仍是主线程；问答不写入 readline 聊天历史。
fn ask_cli(
    rl: &mut DefaultEditor,
    questions: &[terminal_chat::user_input::Question],
) -> Result<terminal_chat::user_input::Response, terminal_chat::user_input::InputError> {
    use terminal_chat::user_input::{Answer, InputError, Response};
    fn read(rl: &mut DefaultEditor, prompt: &str) -> Result<String, InputError> {
        if TOOL_CANCEL.load(Ordering::SeqCst) {
            return Err(InputError::Cancelled);
        }
        // Rustyline 18 的 unsupported TERM 路径使用 Stdin::read_line，遇 SIGINT
        // 会继续阻塞。仅在这个退化路径复用同一主线程的可取消行读取。
        if std::env::var("TERM")
            .is_ok_and(|term| matches!(term.to_ascii_lowercase().as_str(), "dumb" | "cons25" | "emacs"))
        {
            return read_question_line(prompt);
        }
        match rl.readline(prompt) {
            Ok(line) => Ok(line),
            Err(ReadlineError::Interrupted) => {
                TOOL_CANCEL.store(true, Ordering::SeqCst);
                Err(InputError::Cancelled)
            }
            Err(ReadlineError::Eof) => Err(InputError::HostExited),
            Err(_) => Err(InputError::Closed),
        }
    }
    let mut response = Response {
        answers: Default::default(),
    };
    loop {
        for (idx, q) in questions.iter().enumerate() {
            if response
                .answers
                .get(&q.id)
                .is_some_and(|a| !a.answers.is_empty())
            {
                continue;
            }
            say(&format!(
                "[问询 {}/{}] {}\n{}",
                idx + 1,
                questions.len(),
                q.header,
                q.question
            ));
            let options = q.options.as_ref().expect("问询已通过非空选项校验");
            for (i, option) in options.iter().enumerate() {
                say(&format!(
                    "{}. {} — {}",
                    i + 1,
                    option.label,
                    option.description
                ));
            }
            say(&format!("{}. 其他（文字补充）", options.len() + 1));
            let selected = loop {
                let line = read(rl, "选择编号（Enter 跳过）> ")?;
                if line.trim().is_empty() {
                    break None;
                }
                match line.trim().parse::<usize>() {
                    Ok(n) if n >= 1 && n <= options.len() + 1 => break Some(n - 1),
                    _ => say("编号无效，请重新输入。"),
                }
            };
            let mut answers = Vec::new();
            if let Some(index) = selected {
                answers.push(
                    options
                        .get(index)
                        .map(|o| o.label.clone())
                        .unwrap_or_else(|| "None of the above".into()),
                );
                let notes = read(rl, "补充（可留空）> ")?;
                if !notes.trim().is_empty() {
                    answers.push(format!("user_note: {}", notes.trim()));
                }
            }
            response.answers.insert(q.id.clone(), Answer { answers });
        }
        if response.answers.values().all(|a| !a.answers.is_empty()) {
            return Ok(response);
        }
        let confirmation = read(rl, "仍有未答题，确认提交？[y/N] ")?;
        if matches!(
            confirmation.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ) {
            return Ok(response);
        }
    }
}

/// 不支持行编辑的终端仍使用内核行规程（回显/退格）；主线程 poll 后非阻塞读取。
/// O_NONBLOCK 防止 poll 就绪后恰逢 Ctrl+C 清空输入造成再次阻塞；退出恢复原标志。
fn read_question_line(prompt: &str) -> Result<String, terminal_chat::user_input::InputError> {
    use terminal_chat::user_input::InputError;
    struct RestoreFlags(libc::c_int);
    impl Drop for RestoreFlags {
        fn drop(&mut self) {
            unsafe {
                libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, self.0);
            }
        }
    }
    let flags = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(InputError::Closed);
    }
    let _restore = RestoreFlags(flags);
    print!("{prompt}");
    io::stdout().flush().map_err(|_| InputError::Closed)?;
    loop {
        if TOOL_CANCEL.load(Ordering::SeqCst) {
            return Err(InputError::Cancelled);
        }
        let mut fd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut fd, 1, 50) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(InputError::Closed);
        }
        if ready == 0 {
            continue;
        }
        let mut bytes = [0u8; 4096];
        let len = unsafe { libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len()) };
        if len == 0 {
            return Err(InputError::HostExited);
        }
        if len < 0 {
            if matches!(
                io::Error::last_os_error().kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            ) {
                continue;
            }
            return Err(InputError::Closed);
        }
        let mut line = bytes[..len as usize].to_vec();
        if line.ends_with(b"\n") {
            line.pop();
            if line.ends_with(b"\r") {
                line.pop();
            }
            return String::from_utf8(line).map_err(|_| InputError::Closed);
        }
        // canonical tty 的非换行短读来自 VEOF：带草稿 Ctrl+D 也退出，不把草稿当答案。
        return Err(InputError::HostExited);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_input(
    agent: &ureq::Agent,
    rl: &mut DefaultEditor,
    input: &str,
    history: &mut Vec<HistoryItem>,
    autosave: &mut session::Session,
    registrar: &mut terminal_chat::memory::runner::ThreadRegistrar,
    procs: &ProcessManager,
    startup_images: &mut Vec<std::path::PathBuf>,
    mcp: &terminal_chat::mcp::manager::McpManager,
) -> bool {
    if let Some(rest) = input.strip_prefix('/') {
        if let Some(arg) = rest.strip_prefix("save") {
            if arg.is_empty() || arg.starts_with(char::is_whitespace) {
                cmd_save(rl, arg, history);
                return false;
            }
        }
        if let Some(arg) = rest.strip_prefix("resume") {
            if arg.is_empty() || arg.starts_with(char::is_whitespace) {
                cmd_resume(rl, arg, history, autosave, procs);
                return false;
            }
        }
        if let Some(arg) = rest.strip_prefix("memory-reset") {
            if arg.is_empty() || arg.starts_with(char::is_whitespace) {
                say(&terminal_chat::memory::runner::memory_reset(
                    &catalog::user_dir(),
                    arg,
                ));
                return false;
            }
        }
        if let Some(arg) = rest.strip_prefix("mcp") {
            if arg.is_empty() || arg.starts_with(char::is_whitespace) {
                cmd_mcp(mcp, arg);
                return false;
            }
        }
        match rest.trim() {
            "models" => cmd_models(),
            "refresh" => cmd_refresh(agent, rl),
            "sessions" => cmd_sessions(),
            "memory-status" => say(&terminal_chat::memory::runner::memory_status_text(
                &catalog::user_dir(),
            )),
            other => say(&format!(
                "未知命令 /{other}，可用：/models、/refresh、/save <名称>、/resume [名称]、/sessions、/memory-status、/memory-reset confirm、/mcp [reload]"
            )),
        }
        return false;
    }
    cmd_chat(agent, rl, input, history, autosave, registrar, procs, startup_images, mcp)
}

/// 022：`/mcp` 状态显示与 `/mcp reload`（行式宿主主循环天然串行＝空闲时执行）。
fn cmd_mcp(mcp: &terminal_chat::mcp::manager::McpManager, arg: &str) {
    let arg = arg.trim();
    if arg == "reload" {
        match mcp.reload() {
            Ok(()) => say("MCP：已重新加载配置并重建连接"),
            Err(e) => say(&format!("MCP：{e}")),
        }
        return;
    }
    if !arg.is_empty() {
        say("用法：/mcp [reload]");
        return;
    }
    for line in mcp.status_lines() {
        say(&line);
    }
}

/// `/save <名称>`：当前全部历史整体快照到 sessions/<名称>.jsonl（原子替换）。
fn cmd_save(rl: &mut DefaultEditor, arg: &str, history: &[HistoryItem]) {
    let name = arg.trim();
    if name.is_empty() {
        say("用法：/save <名称>（名称为 1–64 个字母、数字、.、_、-，且不以 auto- 开头）");
        return;
    }
    let dir = session::sessions_dir();
    if let Err(e) = session::validate_snapshot_name(name) {
        say(&format!("错误：{e}"));
        return;
    }
    // 已存在→交互确认覆盖；非交互模式拒绝覆盖（新名称不受限）
    if dir.join(format!("{name}.jsonl")).exists() {
        if !io::stdin().is_terminal() {
            say("错误：非交互模式下拒绝覆盖已有快照");
            return;
        }
        if !prompt_y_n(rl, &format!("快照 {name} 已存在，覆盖？")) {
            say("已取消保存");
            return;
        }
    }
    match session::save_snapshot(&dir, name, history) {
        Ok(n) => say(&format!("已保存快照 {name}（{n} 条记录）")),
        Err(e) => say(&format!("错误：{e}")),
    }
}

/// `/resume [名称]`：无参＝最近 auto 会话；有名＝指定会话（auto 续写或快照只读）。
/// 016：切换成功后关闭旧对话的全部进程会话，新对话从空管理器开始；
/// 恢复失败或用户放弃时不影响旧进程。
fn cmd_resume(
    rl: &mut DefaultEditor,
    arg: &str,
    history: &mut Vec<HistoryItem>,
    autosave: &mut session::Session,
    procs: &ProcessManager,
) {
    let name = arg.trim();
    let target = if name.is_empty() { None } else { Some(name) };
    // 当前内存历史非空：交互 y 确认丢弃（含续写目标切换）；非交互一律拒绝
    if !history.is_empty() {
        if !io::stdin().is_terminal() {
            say("错误：非交互模式下恢复会话要求当前历史为空（拒绝丢弃）");
            return;
        }
        if !prompt_y_n(rl, &format!("恢复会话将丢弃当前 {} 条历史记录，继续？", history.len())) {
            say("已取消恢复");
            return;
        }
        // 丢弃前排空旧会话待写段：落盘记录集合须与内存历史一致（§1）；
        // 排空失败中止恢复，旧记录保留在内存等待补写，不静默丢失
        if let Err(e) = autosave.flush_final(history) {
            say(&format!("错误：切换会话前旧会话排空失败，已中止恢复：{e}"));
            return;
        }
    }
    match autosave.resume(target) {
        Ok(info) => {
            if info.degraded {
                say("该会话正被另一进程编辑，已只读装载；本运行的新记录将写入新的自动会话文件");
            }
            say(&info.loaded.summary());
            *history = info.loaded.history;
            procs.reset();
        }
        Err(e) => say(&format!("错误：{e}")),
    }
}

/// `/sessions`：列出会话名称＋文件字节数（readdir＋stat，不打开正文）。
fn cmd_sessions() {
    let list = session::list_sessions(&session::sessions_dir());
    if list.is_empty() {
        say("（暂无会话）");
        return;
    }
    for (name, bytes) in list {
        say(&format!("{name}  {bytes} 字节"));
    }
}

fn build_dict() -> Option<catalog::FinalDict> {
    match catalog::build_final() {
        Ok(d) => Some(d),
        Err(e) => {
            say(&format!("错误：{e}"));
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_chat(
    agent: &ureq::Agent,
    rl: &mut DefaultEditor,
    input: &str,
    history: &mut Vec<HistoryItem>,
    autosave: &mut session::Session,
    registrar: &mut terminal_chat::memory::runner::ThreadRegistrar,
    procs: &ProcessManager,
    startup_images: &mut Vec<std::path::PathBuf>,
    mcp: &terminal_chat::mcp::manager::McpManager,
) -> bool {
    // 020：启动图片一次机会——进入聊天轮即作废（无论成败）；命令/退出不进此处
    let startup_paths: Vec<std::path::PathBuf> = std::mem::take(startup_images);
    let Some(dict) = build_dict() else {
        return false;
    };
    let mut exit_host = false;
    // 调用开始：route 返回的快照固定本次使用的配置与凭证，之后的刷新不影响本次调用
    match catalog::route(&dict) {
        Ok((prov, cred, model)) => {
            // F1：凭证与已授权地址绑定；新增/变更地址需显式授权，非交互模式直接拒绝
            let interactive = io::stdin().is_terminal();
            let gate = catalog::origin_of(&prov.conn.base_url).and_then(|origin| {
                catalog::ensure_origin_authorized(
                    &origin,
                    &cred.name,
                    &catalog::fingerprint(&cred.value),
                    interactive,
                    &mut |q| prompt_y_n(rl, q),
                )
            });
            match gate {
                Ok(()) => {
                    // 020：启动图片正式提交准备（能力检查先于文件打开；一次机会，
                    // 失败＝明确报错、零请求、不自动转纯文本）。合成前置标记文本。
                    let (turn_input, payload_ref): (String, Option<terminal_chat::prompt::UserImagePayload>) =
                        if startup_paths.is_empty() {
                            (input.to_string(), None)
                        } else {
                            match terminal_chat::view_image::prepare_user_images(
                                &startup_paths,
                                catalog::supports_image_input(&model),
                                None,
                            ) {
                                Ok(urls) => {
                                    let (content, elements) =
                                        terminal_chat::view_image::compose_cli_user_message(
                                            input,
                                            urls.len(),
                                        );
                                    (
                                        content,
                                        Some(terminal_chat::prompt::UserImagePayload {
                                            data_urls: urls,
                                            text_elements: elements,
                                        }),
                                    )
                                }
                                Err(e) => {
                                    say(&format!("错误：启动图片未发送：{e}"));
                                    return exit_host;
                                }
                            }
                        };
                    let payload_ref = payload_ref.as_ref();
                    // 013-M7：记忆配置每轮装载（manual 段；无段＝默认关闭）。
                    let mem_config = terminal_chat::memory::runner::load_memories_config();
                    let user_dir = catalog::user_dir();
                    // 013-M7 D2：带输入新轮已通过 route/origin 门＝成功启动，触发一次
                    // 后台管线（enabled gate 在 runner 内——关闭态零副作用）。主轮成败
                    // 不影响已启动的管线；进程退出不等待（批准边界）。
                    terminal_chat::memory::runner::spawn_memory_pipeline(
                        &user_dir,
                        &mem_config,
                        autosave.current_target_stem(),
                        terminal_chat::memory::runner::ModelSend {
                            conn: prov.conn.clone(),
                            key: cred.value.clone(),
                            model: model.clone(),
                            agent: agent.clone(),
                        },
                    );
                    // 006：工具调用循环——组装→请求→串行执行→结果回传→再请求
                    // N2b：仅工具轮期间接管 SIGINT（只置取消标志），轮结束恢复默认——
                    // 001 的动画/输入提示符 Ctrl+C 语义保持不变
                    let conn = &prov.conn;
                    let key = &cred.value;
                    let mut sender =
                        |body: &serde_json::Value| api::chat_with_body(conn, key, &model, body, agent);
                    // 012：工具活动结构化事件——行式路径保持历史输出逐字节不变
                    //（ToolBegin 打印同一活动行；ToolEnd 仅落史不打印，与 006 一致）；
                    // 016：运行中／已取消但进程仍存活的工具，明确展示会话编号；
                    // 017：update_plan 的 ToolEnd 打印计划清单/原因（成功快照说明与步骤）。
                    let mut notify = |n: turn::TurnNotify| match n {
                        // 018-B：行式 CLI 不提供异步宿主，这三个事件不可达；
                        // 021：行式宿主运行 turn 时没有第二个终端读取者，不接续
                        // 输入（steer=None），这两个事件同样不可达——显式列为
                        // 无操作，避免未来接入时静默落入其他分支。
                        turn::TurnNotify::UserInputAsyncWait { .. }
                        | turn::TurnNotify::UserInputAsyncAnswered { .. }
                        | turn::TurnNotify::UserInputAsyncClosed { .. }
                        | turn::TurnNotify::SteerIngested { .. }
                        | turn::TurnNotify::SteerRestored { .. } => {}
                        turn::TurnNotify::Info(line) => say(&line),
                        turn::TurnNotify::ToolBegin { id, name, .. } => {
                            // 021：消息工具无运行活动——成功即消息本身，失败原因
                            // 在 ToolEnd 呈现；不打印无信息量的活动行
                            if name != "send_message_to_user_async" {
                                say(&turn::tool_activity_line(&name, &id));
                            }
                        }
                        turn::TurnNotify::ToolEnd {
                            name,
                            arguments,
                            result,
                            ..
                        } => {
                            // 021：send_message 成功消息即时输出（终文前；println
                            // 按行 flush）；失败/取消/未知显式呈现原因。不重复
                            // 打印活动行（Begin 已抑制）。
                            if name == "send_message_to_user_async" {
                                match terminal_chat::message_to_user::sent_message_from_record(
                                    &arguments, &result,
                                ) {
                                    Some(message) => say(&format!("Agent：{message}")),
                                    None => say(&terminal_chat::message_to_user::failure_line(
                                        &result,
                                    )),
                                }
                            }
                            // 019：view_image——输出已加载路径与实际尺寸；失败/取消
                            // 显示原因；不输出 data URL（say 统一终端过滤）。
                            if name == "view_image" {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&result) {
                                    if v["ok"] == serde_json::json!(true) {
                                        say(&format!(
                                            "[工具] view_image 已加载 {}（发送尺寸 {}×{}，源 {}×{}）",
                                            v["path"].as_str().unwrap_or("?"),
                                            v["width"].as_u64().unwrap_or(0),
                                            v["height"].as_u64().unwrap_or(0),
                                            v["source_width"].as_u64().unwrap_or(0),
                                            v["source_height"].as_u64().unwrap_or(0),
                                        ));
                                    } else if let Some(err) = v["error"].as_str() {
                                        say(&format!("[工具] view_image 失败：{err}"));
                                    }
                                }
                            }
                            // 017：计划更新成功——输出说明与步骤清单（say 经终端过滤；
                            // 失败/取消输出原因，不把失败调用画成清单；结果未知不打印）
                            if name == "update_plan" {
                                if let Some(snapshot) =
                                    terminal_chat::plan::success_snapshot(&arguments, &result)
                                {
                                    for line in terminal_chat::plan::cli_lines(&snapshot) {
                                        say(&line);
                                    }
                                } else if let Some(err) =
                                    terminal_chat::plan::result_error(&result)
                                {
                                    say(&terminal_chat::plan::cli_failure_line(&err));
                                }
                            }
                            // 016：运行中/终止/超时三类宿主侧生命周期状态明确展示
                            //（自然完成含非零退出仍由模型回复承载——006 行式纪律）
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&result) {
                                match v["status"].as_str() {
                                    Some("running") => {
                                        let sid = v["session_id"].as_str().unwrap_or("?");
                                        say(&format!("[工具] {name} 进程仍在运行（会话 {sid}）"));
                                    }
                                    Some("terminated") => {
                                        say(&format!("[工具] {name} 已明确终止并回收"));
                                    }
                                    Some("timeout") => {
                                        say(&format!("[工具] {name} 总运行时限到期，已终止回收"));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    };
                    // 013-M7：开启态以 UserWithMemory 接入 M6 注入（轮首读取一次、
                    // 轮内复用、不入史——读取与 None 原样语义由 turn 层承接）；关闭态
                    // 保持默认 User（既有行为逐字节不变）。
                    let mem_root = terminal_chat::memory::runner::memories_root(&user_dir);
                    let mode = if mem_config.enabled {
                        turn::ToolMode::UserWithMemory {
                            root: &mem_root,
                            config: &mem_config,
                        }
                    } else {
                        turn::ToolMode::User
                    };
                    unsafe {
                        libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t);
                    }
                    // 022：MCP 快照在普通用户轮准备时取得（首次触发配置加载与
                    // 连接建立）。取消（SIGINT 置位）＝结束本轮、不发模型请求、
                    // 候选连接回收；已就绪连接保留。
                    let mcp_snapshot = match mcp.prepare_for_turn(&TOOL_CANCEL) {
                        Some(snapshot) => Some(snapshot),
                        None => {
                            unsafe {
                                libc::signal(libc::SIGINT, libc::SIG_DFL);
                            }
                            say("已取消");
                            return exit_host;
                        }
                    };
                    // 015：用户轮上下文来源（每轮解析：全局规则目录、cwd、SHELL、时钟）
                    let context_sources = terminal_chat::context::ContextSources::from_env();
                    let mut input_host = |_: &turn::ParsedToolCall,
                                          questions: &[terminal_chat::user_input::Question]| {
                        let result = ask_cli(rl, questions);
                        if matches!(result, Err(terminal_chat::user_input::InputError::HostExited)) {
                            exit_host = true;
                        }
                        result
                    };
                    let outcome = turn::run_user_turn_with_context_and_caps(
                        history,
                        &turn_input,
                        &mut sender,
                        &mut notify,
                        Some(&TOOL_CANCEL),
                        // 007：每个入史点同步进入待写段并立即尝试提交（失败只提示一次）
                        &mut |h: &[HistoryItem]| {
                            if let Some(msg) = autosave.on_history(h) {
                                say(&msg);
                            }
                            // 013-M7：threads 登记——仅 (stem, durable_len) 变化时推进
                            //（未成功持久化的数据不登记为已更新版本）。
                            registrar.note_persisted(&user_dir, &mem_config, autosave);
                        },
                        mode,
                        &context_sources,
                        procs,
                        if interactive {
                            Some(&mut input_host)
                        } else {
                            None
                        },
                        // 018-B：行式 CLI 无异步问询宿主
                        None,
                        // 019：每用户轮固定能力快照（声明与执行共用）
                        catalog::supports_image_input(&model),
                        // 020：行式用户图片（-i 启动附件，首轮后无新入口）
                        payload_ref,
                        // 021：消息能力按模型目录声明；行式宿主无第二个终端读取者，
                        // 不提供运行中接续通道（后续行仍按普通输入处理）
                        turn::TurnExtras {
                            send_message: catalog::supports_send_message_to_user(&model),
                            // 022：声明与分派共用本轮快照
                            mcp: mcp_snapshot.as_deref(),
                            steer: None,
                        },
                    );
                    unsafe {
                        libc::signal(libc::SIGINT, libc::SIG_DFL);
                    }
                    match outcome {
                        Ok(reply) => {
                            say(&format!("Agent：{reply}"));
                            // 013-M8：用户轮完成位点——引用反馈 best-effort
                            //（关闭态/解析/开库/计数失败均静默，不打断回复）。
                            terminal_chat::memory::feedback::after_user_turn(
                                &user_dir,
                                &mem_config,
                                &reply,
                            );
                        }
                        Err(e) => {
                            say(&format!("错误：{e}"));
                            // 016：取消当前轮 ≠ 终止进程——明确区分“停止 AI 工作”
                            // 与“已启动进程仍由会话管理”，列出仍有效的编号
                            //（012 教训：以常量为事实来源，不反解析消息文本）。
                            if e.starts_with(turn::CANCELLED_TURN_ERROR_PREFIX) {
                                say(procs.live_sessions_note().as_deref().unwrap_or(
                                    "已停止当前工作；本轮没有仍在运行的进程会话。",
                                ));
                            }
                        }
                    }
                }
                Err(e) => say(&format!("错误：{e}")),
            }
        }
        Err(e) => say(&format!("错误：{e}")),
    }
    exit_host
}

fn cmd_models() {
    let Some(dict) = build_dict() else { return };
    let Some(pid) = dict.selected_provider.clone() else {
        say("错误：手动配置未设置 selected_provider");
        return;
    };
    let Some(prov) = dict.providers.get(&pid) else {
        say(&format!("错误：selected_provider {pid} 不在最终字典中"));
        return;
    };
    say(&format!(
        "厂商 {pid}（基础地址 {}）有效模型：",
        prov.conn.base_url
    ));
    if prov.models.is_empty() {
        say("  （名单为空）");
        return;
    }
    for (id, m) in &prov.models {
        let name = m
            .display_name
            .clone()
            .map(|n| format!("（{n}）"))
            .unwrap_or_default();
        let flag = if m.disabled == Some(true) { "【已禁用】" } else { "" };
        let caps = match (m.context_length, m.max_output) {
            (Some(c), Some(o)) => format!("，上下文 {c}，输出上限 {o}"),
            (Some(c), None) => format!("，上下文 {c}"),
            (None, Some(o)) => format!("，输出上限 {o}"),
            (None, None) => "，能力未知".into(),
        };
        let sel = if dict.selected_model.as_deref() == Some(id.as_str()) { " *" } else { "" };
        say(&format!("  {id}{sel}{name}{flag}{caps}"));
    }
}

fn cmd_refresh(agent: &ureq::Agent, rl: &mut DefaultEditor) {
    let interactive = io::stdin().is_terminal();
    // 两个授权回调共用一个行编辑器：经 RefCell 顺序借用
    let rl_cell = std::cell::RefCell::new(&mut *rl);
    match catalog::refresh(
        agent,
        &mut |origin, name, fp| {
            catalog::ensure_origin_authorized(origin, name, fp, interactive, &mut |q| {
                prompt_y_n(&mut rl_cell.borrow_mut(), q)
            })
        },
        &mut |origin| {
            catalog::ensure_catalog_origin_authorized(origin, interactive, &mut |q| {
                prompt_y_n(&mut rl_cell.borrow_mut(), q)
            })
        },
        None, // 正式路径：不注入目录地址，一律强制 HTTPS
    ) {
        Ok(report) => {
            match &report.public_error {
                Some(e) => say(&format!("公共目录同步失败，沿用基础资料：{e}")),
                None if report.public_updated.is_empty() => say("公共目录：无更新"),
                None => say(&format!(
                    "公共目录已更新基础资料：{}",
                    report.public_updated.join("、")
                )),
            }
            for (id, n) in &report.queried {
                say(&format!("{id}：查询到 {n} 个模型，名单已替换"));
            }
            for (id, reason) in &report.failed {
                say(&format!("{id}：刷新失败（{reason}），保留身份匹配的旧名单"));
            }
            for (id, reason) in &report.skipped {
                say(&format!("{id}：跳过查询（{reason}）"));
            }
            say("刷新完成。所选模型不变，正在执行的请求不受影响。");
        }
        Err(e) => say(&format!("错误：{e}")),
    }
}
