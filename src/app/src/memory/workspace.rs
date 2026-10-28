//! 013-M5（G8，2026-10-18 用户批准）：记忆根专用 Git 基线与写后校验。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——`memories/write/src/workspace.rs`（prepare/diff/reset/
//! validate 编排、差异文件渲染、`remove_memory_symlinks` 计数语义——本地
//! 已随 M4 落在 `storage_files.rs`，此处复用）＋`git-utils/src/baseline.rs`
//! （三原语语义：ensure 保健康基线、不可用元数据以全新单提交基线替换；
//! diff＝基线→当前目录；reset＝以当前内容重建单提交基线）。
//!
//! **载体适配（G8 批准）**：上游 gix＋similar 库内实现替换为**内部固定的
//! 外部 `git` 进程**——固定 argv 直启（不经 shell、不经模型 exec_command）；
//! 每条命令显式 `--git-dir=<root>/.git --work-tree=<root>` 锚定，git 不会
//! 向上发现并复用父项目仓库（批准边界「只操作记忆根自有的专用仓库」的
//! 机制承载；`git init` 本身不向上发现，亦在记忆根内创建）；`env_clear`
//! 后仅回填 `PATH`/`LC_ALL` 与 `GIT_CONFIG_COUNT=0`＋
//! `GIT_CONFIG_GLOBAL/SYSTEM=/dev/null`——阻断 env 注入（`GIT_CONFIG_KEY_*`）
//! 与全局/系统配置、并清除 `GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE` 泄漏；
//! `-c user.name/user.email/commit.gpgsign=false` 固定提交 identity；
//! 进程组终止/超时/输出上限沿 003 `exec.rs` 同构模式（双流读取线程＋轮询）。
//!
//! 本地布局差异（2026-10-18 定点修正轮核实）：上游 sqlite 库在记忆根外、
//! 基线全量遍历天然不含；本地 `memories.sqlite`（M1 落地）与
//! `memories.sqlite-journal` 在记忆根内——三原语统一排除集
//! `:(exclude)memories.sqlite`＋`:(exclude)memories.sqlite-journal`，暂存恒
//! `add -A -f`（被忽略的新摘要仍可见），差异计算前先跑同一条 add
//! （`git diff HEAD` 不含 untracked）。**曾被跟踪的排除文件**（批准边界，
//! 机制本轮落实）：reset 先 `git rm --cached --ignore-unmatch`（只动索引、
//! 不删不碰真实库文件），使其退出基线、后续 diff 彻底不可见。reset 用
//! `commit --amend` 保持单提交基线——适用前提＝本契约自建且自管理的记忆根
//! 仓库（ensure 初建后历史恒为单提交；实施轮以连续 reset 后
//! `rev-list --count`＝1 测试固化），不将 amend 泛称为任意仓库重建的等价
//! 实现（上游 reset 为破坏性删除 `.git` 重建，语义差异见 013 §3 G8 行）。

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// 差异文件名（上游 `workspace_diff::FILENAME`）。
pub const WORKSPACE_DIFF_FILENAME: &str = "phase2_workspace_diff.md";
/// 差异文件渲染上限（上游 `workspace_diff::MAX_BYTES`＝4 MiB）。
pub const WORKSPACE_DIFF_MAX_BYTES: usize = 4 * 1024 * 1024;

/// 单条 git 命令超时（毫秒；对齐 003 `MAX_TIMEOUT_MS`）。
const GIT_TIMEOUT_MS: u64 = 60_000;
/// 可测缝（crate 私有）：超时/程序覆盖——仅单元测试使用（0/空＝未覆盖）。
static GIT_TIMEOUT_MS_OVERRIDE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static GIT_PROGRAM_OVERRIDE: Mutex<String> = Mutex::new(String::new());

fn effective_timeout_ms() -> u64 {
    let v = GIT_TIMEOUT_MS_OVERRIDE.load(Ordering::Relaxed);
    if v > 0 {
        v
    } else {
        GIT_TIMEOUT_MS
    }
}

fn git_program() -> String {
    let program = GIT_PROGRAM_OVERRIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if program.is_empty() {
        "git".into()
    } else {
        program.clone()
    }
}
/// git stdout 捕获上限（Unicode 码点；差异文件渲染另有 4 MiB 合同上限，
/// 此处仅界内存。真实记忆根远小于此）。
const GIT_OUTPUT_LIMIT_CHARS: usize = 32 * 1024 * 1024;
/// git stderr 捕获上限。
const GIT_STDERR_LIMIT_CHARS: usize = 1024 * 1024;
/// 轮询间隔与读线程收尾宽限（003 同款；宽限防孙进程持有管道使 join 无限等待）。
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const READER_GRACE: Duration = Duration::from_secs(2);
const READ_BUF: usize = 8 * 1024;

/// 基线提交说明与固定 identity（本地适配：上游为
/// "Initialize Codex git baseline"＋Codex 签名）。
const BASELINE_COMMIT_MESSAGE: &str = "Initialize terminal-chat memory baseline";
const GIT_USER_NAME: &str = "terminal-chat";
const GIT_USER_EMAIL: &str = "memory@terminal-chat.invalid";

/// 文件级变更状态（上游 `GitBaselineChangeStatus`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitBaselineChangeStatus {
    Added,
    Modified,
    Deleted,
}

impl GitBaselineChangeStatus {
    /// git 风格短标签（上游 `label`）。
    pub fn label(self) -> &'static str {
        match self {
            GitBaselineChangeStatus::Added => "A",
            GitBaselineChangeStatus::Modified => "M",
            GitBaselineChangeStatus::Deleted => "D",
        }
    }
}

/// 一条变更文件（上游 `GitBaselineChange`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBaselineChange {
    pub status: GitBaselineChangeStatus,
    pub path: String,
}

/// 基线→当前的完整差异（上游 `GitBaselineDiff`：状态列表＋统一 diff 文本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBaselineDiff {
    pub changes: Vec<GitBaselineChange>,
    pub unified_diff: String,
}

impl GitBaselineDiff {
    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }
}

struct GitOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// 运行一条 git 命令（固定 argv、锚定记忆根、隔离环境、超时/取消/输出上限）。
/// 退出码原样返回（`rev-parse --verify` 的非零是判定信号，不是错误）。
fn run_git(root: &Path, args: &[&str], cancel: Option<&AtomicBool>) -> Result<GitOutput, String> {
    run_git_env(root, args, cancel, &[])
}

/// [`run_git`] 的环境扩展变体（提交时间注入用）。
fn run_git_env(
    root: &Path,
    args: &[&str],
    cancel: Option<&AtomicBool>,
    extra_env: &[(&str, String)],
) -> Result<GitOutput, String> {
    let git_dir = root.join(".git").to_string_lossy().to_string();
    let work_tree = root.to_string_lossy().to_string();
    let user_name = format!("user.name={GIT_USER_NAME}");
    let user_email = format!("user.email={GIT_USER_EMAIL}");
    let mut argv: Vec<&str> = vec![
        "--git-dir",
        &git_dir,
        "--work-tree",
        &work_tree,
        "-c",
        &user_name,
        "-c",
        &user_email,
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.quotePath=false",
        // 027 B⑤：私有仓库钩子一律不执行（E6：已植入钩子会以 git 进程身份运行，
        // 固定 argv 不构成防线）；/dev/null 子路径 stat 恒失败＝无钩子可执行。
        // 只影响本仓库调用，不改全局配置、不影响项目提交扫描 hook。
        "-c",
        "core.hooksPath=/dev/null",
    ];
    argv.extend_from_slice(args);

    let path_env = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/local/bin".into());
    let program = git_program();
    let mut command = Command::new(&program);
    command
        .args(&argv)
        .current_dir(root)
        .env_clear()
        .env("PATH", &path_env)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for (k, v) in extra_env {
        command.env(k, v);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // 独立进程组：超时/取消终止整个进程组（003 同款；unix only 与项目一致）
    command.process_group(0);

    let timeout_ms = effective_timeout_ms();
    let mut child = command
        .spawn()
        .map_err(|e| format!("启动 git 失败（args={args:?}）：{e}"))?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    let stdout = capture_bounded(child.stdout.take(), GIT_OUTPUT_LIMIT_CHARS);
    let stderr = capture_bounded(child.stderr.take(), GIT_STDERR_LIMIT_CHARS);

    let mut killed_reason: Option<&'static str> = None;
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        if let Some(cancel) = cancel {
            if cancel.load(Ordering::SeqCst) {
                killed_reason = Some("已取消");
                kill_process_group(&mut child);
                break child
                    .wait()
                    .map_err(|e| format!("等待 git 退出失败：{e}"))?;
            }
        }
        if Instant::now() >= deadline {
            killed_reason = Some("超时");
            kill_process_group(&mut child);
            break child
                .wait()
                .map_err(|e| format!("等待 git 退出失败：{e}"))?;
        }
        thread::sleep(POLL_INTERVAL);
    };
    // 读线程收尾带宽限：进程组已终止时管道应随即 EOF；孙进程持管道的异常
    // 情形不无限等待（超时按错误上抛，不吞半截输出）。
    let stdout = finish_capture(stdout)?;
    let stderr = finish_capture(stderr)?;

    if let Some(reason) = killed_reason {
        return Err(format!("git {args:?} {reason}（{timeout_ms}ms 上限/取消）"));
    }
    Ok(GitOutput {
        exit_code: status.code().unwrap_or(-1),
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn kill_process_group(child: &mut Child) {
    let pgid = child.id() as libc::pid_t;
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
}

/// 管道读取线程句柄：完成信号通道供收尾宽限判定（003 同构）。
struct Capture {
    handle: thread::JoinHandle<String>,
    done: mpsc::Receiver<()>,
}

/// 管道读取线程：按码点有界累积（超限继续读取并丢弃，内存有界——003 同款；
/// 码点计数增量维护，非整串重数）；结束时发送完成信号。
fn capture_bounded(
    pipe: Option<impl std::io::Read + Send + 'static>,
    limit_chars: usize,
) -> Capture {
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let out = read_bounded(pipe, limit_chars);
        let _ = done_tx.send(());
        out
    });
    Capture {
        handle,
        done: done_rx,
    }
}

fn read_bounded(pipe: Option<impl std::io::Read>, limit_chars: usize) -> String {
    let mut out = String::new();
    let Some(mut pipe) = pipe else { return out };
    let mut buf = [0u8; READ_BUF];
    let mut counted = 0usize;
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if counted < limit_chars {
                    let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                    counted += chunk.chars().count();
                    out.push_str(&chunk);
                }
            }
        }
    }
    out
}

/// 等待读取线程收尾：宽限内完成→join 取结果；超时→错误（不无限阻塞、
/// 不吞半截输出）。
fn finish_capture(capture: Capture) -> Result<String, String> {
    match capture.done.recv_timeout(READER_GRACE) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => capture
            .handle
            .join()
            .map_err(|_| "git 输出读取线程崩溃".to_string()),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("git 输出读取超时（进程组终止后管道未关闭）".into())
        }
    }
}

/// 期望 exit 0 的 git 调用：非零＝错误（附 stderr）。
fn git_ok(root: &Path, args: &[&str], cancel: Option<&AtomicBool>) -> Result<(), String> {
    let out = run_git(root, args, cancel)?;
    if out.exit_code != 0 {
        let detail = out.stderr.trim();
        return Err(format!(
            "git {args:?} 退出码 {}{}",
            out.exit_code,
            if detail.is_empty() {
                String::new()
            } else {
                format!("：{detail}")
            }
        ));
    }
    Ok(())
}

/// 统一的暂存命令：`add -A -f`＋排除集（三原语同一条——文件集合一致性）。
fn stage_all(root: &Path, cancel: Option<&AtomicBool>) -> Result<(), String> {
    git_ok(
        root,
        &[
            "add",
            "-A",
            "-f",
            "--",
            ".",
            ":(exclude)memories.sqlite",
            ":(exclude)memories.sqlite-journal",
        ],
        cancel,
    )
}

/// 统一的差异 pathspec（状态查询与全文 diff 用同一集合）。
fn diff_pathspec() -> Vec<&'static str> {
    vec![
        "--",
        ".",
        ":(exclude)memories.sqlite",
        ":(exclude)memories.sqlite-journal",
    ]
}

fn remove_workspace_diff(root: &Path) -> Result<(), String> {
    let path = root.join(WORKSPACE_DIFF_FILENAME);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除差异文件 {} 失败：{e}", path.display())),
    }
}

/// 准备记忆工作区（上游 `prepare_memory_workspace`）：布局（复用 M4
/// `ensure_layout`：0700 根＋symlink 清理＋rollout_summaries）→删除残留
/// 差异文件→确保基线仓库可用（健康即保持；不可用则全新单提交基线替换
/// ——仅删记忆根自有的 `.git` 元数据，不触记忆内容与父仓库）。
pub fn prepare_memory_workspace(root: &Path, cancel: Option<&AtomicBool>) -> Result<(), String> {
    crate::memory::storage_files::ensure_layout(root, &mut |_| {})
        .map_err(|e| format!("准备记忆布局失败：{e}"))?;
    remove_workspace_diff(root)?;
    // 013-M7 收口第四轮：任务凭据＝任务领取时的代数快照（phase2 内存内
    // 传递），不再写共享凭据文件——新代 prepare 不会让旧任务凭据恢复。
    ensure_baseline(root, cancel)
}

/// 基线可用性判定与初建（上游 `ensure_git_baseline_repository` 语义：
/// `rev-parse --verify HEAD` 健康→保持；否则以全新单提交基线替换）。
fn ensure_baseline(root: &Path, cancel: Option<&AtomicBool>) -> Result<(), String> {
    let probe = run_git(root, &["rev-parse", "--verify", "HEAD"], cancel)?;
    if probe.exit_code == 0 {
        return Ok(());
    }
    // 不可用元数据替换：删除记忆根自有 `.git`（NotFound 忽略），再全新初建。
    // 锚定的 git-dir 指向 `<root>/.git`，不会向上发现父仓库。
    let git_dir = root.join(".git");
    match std::fs::symlink_metadata(&git_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || meta.is_file() {
                std::fs::remove_file(&git_dir)
            } else {
                std::fs::remove_dir_all(&git_dir)
            }
            .map_err(|e| format!("移除不可用基线元数据 {} 失败：{e}", git_dir.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("读取基线元数据 {} 失败：{e}", git_dir.display())),
    }
    // `git init` 在记忆根内创建（init 不向上发现仓库；无需锚定参数）
    let init_program = git_program();
    let init = Command::new(&init_program)
        .args(["init"])
        .current_dir(root)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/local/bin".into()),
        )
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("启动 git init 失败：{e}"))?;
    if !init.status.success() {
        return Err(format!(
            "git init 失败：{}",
            String::from_utf8_lossy(&init.stderr).trim()
        ));
    }
    stage_all(root, cancel)?;
    // 初建用普通 commit（unborn HEAD 上 --amend 必败）；--allow-empty 覆盖空根。
    // 提交时间固定纪元（确定性：基线 SHA 不随真实时钟漂移）。
    let epoch = format!("{} +0000", BASELINE_EPOCH_UNIX);
    commit_with_date(
        root,
        &["commit", "--allow-empty", "-m", BASELINE_COMMIT_MESSAGE],
        cancel,
        &epoch,
    )?;
    git_ok(root, &["read-tree", "--reset", "HEAD"], cancel)?;
    Ok(())
}

/// 基线初建提交的固定时间（unix 秒；2026-01-01T00:00:00Z 附近取整值，
/// 仅作确定性锚点，无业务含义）。
const BASELINE_EPOCH_UNIX: i64 = 1_767_225_600;

/// 带提交时间的 commit（`--amend` 重建时取 HEAD 提交时间＋1 秒——即使树与
/// 内容完全相同，reset 也恒产生新提交：与真实时钟解耦，测试与幂等语义可复现）。
fn commit_with_date(
    root: &Path,
    args: &[&str],
    cancel: Option<&AtomicBool>,
    unix_ts: &str,
) -> Result<(), String> {
    let out = run_git_env(
        root,
        args,
        cancel,
        &[
            ("GIT_AUTHOR_DATE", unix_ts.to_string()),
            ("GIT_COMMITTER_DATE", unix_ts.to_string()),
        ],
    )?;
    if out.exit_code != 0 {
        let detail = out.stderr.trim();
        return Err(format!(
            "git commit 退出码 {}{}",
            out.exit_code,
            if detail.is_empty() {
                String::new()
            } else {
                format!("：{detail}")
            }
        ));
    }
    Ok(())
}

/// reset 的 amend 提交时间＝当前 HEAD 提交时间＋1 秒（无 HEAD 时用纪元）。
fn next_commit_timestamp(root: &Path, cancel: Option<&AtomicBool>) -> Result<String, String> {
    let probe = run_git(root, &["log", "-1", "--format=%ct"], cancel)?;
    if probe.exit_code == 0 {
        let ct: i64 = probe
            .stdout
            .trim()
            .parse()
            .map_err(|_| format!("HEAD 提交时间不可解析：{}", probe.stdout.trim()))?;
        return Ok(format!("{} +0000", ct.saturating_add(1)));
    }
    Ok(format!("{} +0000", BASELINE_EPOCH_UNIX))
}

/// 当前工作区差异（上游 `memory_workspace_diff`）：先删除残留差异文件，
/// 再以同一条暂存＋pathspec 计算基线→当前差异。
pub fn memory_workspace_diff(
    root: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<GitBaselineDiff, String> {
    remove_workspace_diff(root)?;
    stage_all(root, cancel)?;

    // 状态列表（-z：NUL 分隔，无引号转义歧义；--no-renames 保证恰 A/M/D）
    let mut args = vec![
        "diff".to_string(),
        "HEAD".to_string(),
        "--no-renames".to_string(),
        "--no-color".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
        "--name-status".to_string(),
        "-z".to_string(),
    ];
    args.extend(diff_pathspec().into_iter().map(String::from));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let status_out = run_git(root, &refs, cancel)?;
    if status_out.exit_code != 0 {
        return Err(format!(
            "git diff --name-status 退出码 {}：{}",
            status_out.exit_code,
            status_out.stderr.trim()
        ));
    }
    let mut changes = parse_name_status_entries(&status_out.stdout)?;

    // 013-M8 D12-A（2026-10-19 用户批准）：非法便条不进入自动提供的差异
    // 状态列表与正文——在差异产出处单点过滤，**两清单同源**（同一排除集
    // 重算）；盘上文件保留，mem_list/mem_read 旁路照常（提示词不构成技术
    // 隔离）。仅非法便条被排除，其余变化（含合法便条）保持可见。
    let invalid_note_excludes: Vec<String> = invalid_ad_hoc_note_paths(&changes, root)
        .into_iter()
        .map(|path| format!(":(exclude,literal){path}"))
        .collect();
    let mut unified_args = vec![
        "diff".to_string(),
        "HEAD".to_string(),
        "--no-renames".to_string(),
        "--no-color".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
    ];
    unified_args.extend(diff_pathspec().into_iter().map(String::from));
    if !invalid_note_excludes.is_empty() {
        // 状态列表按同一排除集重算（无过滤时不付第二次 git 代价）
        let mut status_args = vec![
            "diff".to_string(),
            "HEAD".to_string(),
            "--no-renames".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            "--no-textconv".to_string(),
            "--name-status".to_string(),
            "-z".to_string(),
        ];
        status_args.extend(diff_pathspec().into_iter().map(String::from));
        status_args.extend(invalid_note_excludes.iter().cloned());
        let refs: Vec<&str> = status_args.iter().map(String::as_str).collect();
        let status_out = run_git(root, &refs, cancel)?;
        if status_out.exit_code != 0 {
            return Err(format!(
                "git diff --name-status 退出码 {}：{}",
                status_out.exit_code,
                status_out.stderr.trim()
            ));
        }
        changes = parse_name_status_entries(&status_out.stdout)?;
        unified_args.extend(invalid_note_excludes.iter().cloned());
    }

    // 统一 diff 全文（与状态列表同一文件集合与同一格式开关）
    let refs: Vec<&str> = unified_args.iter().map(String::as_str).collect();
    let full_out = run_git(root, &refs, cancel)?;
    if full_out.exit_code != 0 {
        return Err(format!(
            "git diff 退出码 {}：{}",
            full_out.exit_code,
            full_out.stderr.trim()
        ));
    }

    Ok(GitBaselineDiff {
        changes,
        unified_diff: full_out.stdout,
    })
}

/// 解析 `git diff --name-status -z` 输出（NUL 分隔，状态与路径交替）。
fn parse_name_status_entries(stdout: &str) -> Result<Vec<GitBaselineChange>, String> {
    let mut changes = Vec::new();
    let mut tokens = stdout.split('\0');
    while let Some(status) = tokens.next() {
        if status.is_empty() {
            continue;
        }
        let Some(path) = tokens.next() else { break };
        let status = match status {
            "A" => GitBaselineChangeStatus::Added,
            "M" => GitBaselineChangeStatus::Modified,
            "D" => GitBaselineChangeStatus::Deleted,
            other => return Err(format!("git diff 未知状态 {other}（路径 {path}）")),
        };
        changes.push(GitBaselineChange {
            status,
            path: path.to_string(),
        });
    }
    Ok(changes)
}

/// 便条目录（相对记忆根；runner 播种的 instructions.md 在其上一级，不受
/// 便条规则约束）。
const AD_HOC_NOTES_DIR: &str = "extensions/ad_hoc/notes";

/// 便条文件名合同＝上游 V2 `add_ad_hoc_note` 校验原文（013-M8 D12-A 复用
/// 为差异过滤规则）：`YYYY-MM-DDTHH-MM-SS-<slug>.md`、slug 仅小写 ASCII
/// 字母/数字/连字符（首字符非连字符，1–80 字节）、总长 ≤128 字节。
fn is_valid_ad_hoc_note_file_name(name: &str) -> bool {
    name.len() <= 128 && ad_hoc_note_name_regex().is_match(name)
}

fn ad_hoc_note_name_regex() -> &'static regex_lite::Regex {
    static RE: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex_lite::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-[a-z0-9][a-z0-9-]{0,79}\.md$")
            .expect("便条文件名正则恒合法")
    })
}

/// 便条正文非全空白（≤1 MiB 读入，超限即有内容；读失败或非 UTF-8＝无效）。
fn ad_hoc_note_body_not_blank(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; 1024 * 1024 + 1];
    let mut read = 0usize;
    loop {
        match file.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(n) => {
                read += n;
                if read > 1024 * 1024 {
                    return true;
                }
            }
            Err(_) => return false,
        }
    }
    match std::str::from_utf8(&buf[..read]) {
        Ok(s) => !s.trim().is_empty(),
        Err(_) => false,
    }
}

/// 差异条目中的非法便条路径：目录内（无子目录）、文件名合规；A/M 还须
/// 正文非全空白（D 无正文可查，仅按文件名判定）。非便条路径一律保留。
fn invalid_ad_hoc_note_paths(changes: &[GitBaselineChange], root: &Path) -> Vec<String> {
    changes
        .iter()
        .filter(|change| {
            let Some(rest) = note_relative_path(&change.path) else {
                return false;
            };
            let name_invalid =
                rest.contains('/') || !is_valid_ad_hoc_note_file_name(rest);
            let body_invalid = change.status != GitBaselineChangeStatus::Deleted
                && !ad_hoc_note_body_not_blank(&root.join(&change.path));
            name_invalid || body_invalid
        })
        .map(|change| change.path.clone())
        .collect()
}

/// 便条目录内条目的相对名（目录外→None）。
fn note_relative_path(path: &str) -> Option<&str> {
    path.strip_prefix(AD_HOC_NOTES_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
}

/// 写差异文件（上游 `write_workspace_diff`）：状态列表＋diff 块＋4 MiB 截断。
pub fn write_workspace_diff(root: &Path, diff: &GitBaselineDiff) -> Result<(), String> {
    let path = root.join(WORKSPACE_DIFF_FILENAME);
    crate::catalog::write_private_file(&path, &render_workspace_diff_file(diff))
        .map_err(|e| format!("写入差异文件 {} 失败：{e}", path.display()))
}

/// 以当前内容重建基线（上游 `reset_memory_workspace_baseline`）：删差异
/// 文件→排除文件退出索引（`rm --cached --ignore-unmatch`，不删真实文件）→
/// 暂存→`commit --amend` 保单提交→`read-tree --reset` 对齐索引。
pub fn reset_memory_workspace_baseline(
    root: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    remove_workspace_diff(root)?;
    git_ok(
        root,
        &[
            "rm",
            "--cached",
            "-q",
            "--ignore-unmatch",
            "--",
            "memories.sqlite",
            "memories.sqlite-journal",
        ],
        cancel,
    )?;
    stage_all(root, cancel)?;
    let date = next_commit_timestamp(root, cancel)?;
    commit_with_date(
        root,
        &[
            "commit",
            "--amend",
            "--allow-empty",
            "-m",
            BASELINE_COMMIT_MESSAGE,
        ],
        cancel,
        &date,
    )?;
    git_ok(root, &["read-tree", "--reset", "HEAD"], cancel)?;
    Ok(())
}

/// 写后校验（上游 `validate_consolidation_artifacts`，V1）：无残留符号链接
/// （有则先移除并判无效）、`MEMORY.md` 为常规文件、`memory_summary.md`
/// 首行恰为 `v1`。
pub fn validate_consolidation_artifacts(root: &Path) -> Result<(), String> {
    let removed = crate::memory::storage_files::remove_memory_symlinks(root, &mut |_| {})
        .map_err(|e| format!("清理记忆工作区符号链接失败：{e}"))?;
    if removed != 0 {
        return Err(format!(
            "合并输出包含 {removed} 个符号链接（已移除），判无效"
        ));
    }

    let memory_path = root.join("MEMORY.md");
    let metadata = std::fs::metadata(&memory_path)
        .map_err(|e| format!("读取合并产物 {} 失败：{e}", memory_path.display()))?;
    if !metadata.is_file() {
        return Err(format!("合并产物不是常规文件：{}", memory_path.display()));
    }

    let summary_path = root.join("memory_summary.md");
    let summary = std::fs::read_to_string(&summary_path)
        .map_err(|e| format!("读取记忆摘要 {} 失败：{e}", summary_path.display()))?;
    if summary.lines().next() != Some("v1") {
        return Err(format!("记忆摘要首行不是 v1：{}", summary_path.display()));
    }
    Ok(())
}

/// 差异文件渲染（上游 `render_workspace_diff_file`＋`append_bounded_diff`）。
fn render_workspace_diff_file(diff: &GitBaselineDiff) -> String {
    let mut rendered = String::from(
        "# Memory Workspace Diff\n\n\
         Generated by Codex before Phase 2 memory consolidation. Read this file first and do not edit it.\n\n\
         ## Status\n",
    );

    if !diff.has_changes() {
        rendered.push_str("- none\n");
        return rendered;
    }

    for change in &diff.changes {
        rendered.push_str(&format!("- {} {}\n", change.status.label(), change.path));
    }
    rendered.push_str("\n## Diff\n\n```diff\n");
    append_bounded_diff(&mut rendered, &diff.unified_diff);
    rendered.push_str("```\n");
    rendered
}

fn append_bounded_diff(rendered: &mut String, diff: &str) {
    if diff.len() <= WORKSPACE_DIFF_MAX_BYTES {
        rendered.push_str(diff);
        if !diff.ends_with('\n') {
            rendered.push('\n');
        }
        return;
    }

    let boundary = previous_char_boundary(diff, WORKSPACE_DIFF_MAX_BYTES);
    rendered.push_str(&diff[..boundary]);
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str(&format!(
        "\n[workspace diff truncated at {} bytes]\n",
        WORKSPACE_DIFF_MAX_BYTES
    ));
}

fn previous_char_boundary(value: &str, max_bytes: usize) -> usize {
    if max_bytes >= value.len() {
        return value.len();
    }
    let mut index = max_bytes;
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 覆盖缝串行锁（同一测试二进制内避免覆盖/恢复交错）。
    static OVERRIDE_GUARD: Mutex<()> = Mutex::new(());

    /// 覆盖设置／恢复（Drop 恢复——panic 亦不泄漏覆盖）。
    struct OverrideGuard;

    impl OverrideGuard {
        fn new(program: String, timeout_ms: u64) -> Self {
            *GIT_PROGRAM_OVERRIDE
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = program;
            GIT_TIMEOUT_MS_OVERRIDE.store(timeout_ms, Ordering::Relaxed);
            OverrideGuard
        }
    }

    impl Drop for OverrideGuard {
        fn drop(&mut self) {
            *GIT_PROGRAM_OVERRIDE
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = String::new();
            GIT_TIMEOUT_MS_OVERRIDE.store(0, Ordering::Relaxed);
        }
    }

    /// 进程活性：/proc 条目存在**且非僵尸**（Z＝已终止待收割——本容器
    /// PID 1 不收割孤儿后代，僵尸条目会残留；终止即为清理完成）。
    fn proc_alive(pid: u32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/status")) {
            Ok(status) => !status
                .lines()
                .any(|l| l.starts_with("State:") && l.contains('Z')),
            Err(_) => false, // 条目消失＝已收割
        }
    }

    fn wait_for<T>(predicate: impl Fn() -> Option<T>, what: &str) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(v) = predicate() {
                return v;
            }
            assert!(Instant::now() < deadline, "等待超时：{what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// 超时路径行为证据（M5 公共入口直达）：
    /// ①**由超时触发**——假 git 为合法脚本（记 pid 后 `sleep 30`），无取消
    ///   标志、路径与权限正常、脚本本身可成功执行；报文含「超时」与生效
    ///   毫秒值（非取消/路径/权限错误的报文面）；
    /// ②**进程组清理**——直接子进程（sh）与其后代（sleep）经 SIGKILL 组杀
    ///   全部终止（/proc 实证消失，轮询 ≤5s）；
    /// ③**失败契约**——Err 上抛（不伪报成功）、基线未建立（`.git` 不存在，
    ///   无成功副作用）。
    #[test]
    fn git_timeout_kills_process_group_with_descendants() {
        let _serial = OVERRIDE_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp =
            std::env::temp_dir().join(format!("tchat-m5-timeout-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let script = tmp.join("slow-git");
        let pidfile = tmp.join("pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 30 &\necho \"$$ $!\" > {}\nwait\n",
                pidfile.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // 300ms：脚本已执行（写 pid＋fork sleep），远未自然结束（30s）——
        // 超时由时间触发，非取消/路径/权限原因
        let _ov = OverrideGuard::new(script.display().to_string(), 300);

        let root = tmp.join("root");
        std::fs::create_dir_all(&root).unwrap();
        // M5 入口直达超时路径（prepare→rev-parse→…→init 全部经同一 runner）
        let err = prepare_memory_workspace(&root, None).unwrap_err();
        assert!(err.contains("超时"), "超时报文：{err}");
        assert!(err.contains("300ms"), "报生效超时值：{err}");
        assert!(
            !root.join(".git").is_dir(),
            "失败契约：基线未建立（无成功副作用）"
        );

        // 进程组清理：直接子进程与其 sleep 后代均消失（pidfile 记录双 pid）
        let pids: Vec<u32> = wait_for(
            || {
                std::fs::read_to_string(&pidfile).ok().and_then(|s| {
                    let v: Vec<u32> = s
                        .split_whitespace()
                        .filter_map(|t| t.parse().ok())
                        .collect();
                    (v.len() == 2).then_some(v)
                })
            },
            "双 pid 记录出现",
        );
        let (pid, child_pid) = (pids[0], pids[1]);
        wait_for(|| (!proc_alive(pid)).then_some(()), "直接子进程退出");
        wait_for(
            || (!proc_alive(child_pid)).then_some(()),
            "后代 sleep 被组杀",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn bounded_diff_truncation_marker() {
        let mut rendered = String::new();
        append_bounded_diff(&mut rendered, &"x".repeat(WORKSPACE_DIFF_MAX_BYTES + 100));
        assert!(rendered.contains(&format!(
            "[workspace diff truncated at {} bytes]",
            WORKSPACE_DIFF_MAX_BYTES
        )));
        assert!(rendered.len() <= WORKSPACE_DIFF_MAX_BYTES + 80);

        let mut rendered = String::new();
        append_bounded_diff(&mut rendered, "small");
        assert_eq!(rendered, "small\n");

        let mut rendered = String::new();
        append_bounded_diff(&mut rendered, "no-newline-tail");
        assert_eq!(rendered, "no-newline-tail\n");
    }

    #[test]
    fn char_boundary_steps_back_to_boundary() {
        // 多字节字符不切断
        let s = "é".repeat(1024 * 1024 + 3); // 2 字节/字符
        let b = previous_char_boundary(&s, 1024 * 1024 + 1);
        assert!(s.is_char_boundary(b));
        assert!(b <= 1024 * 1024 + 1);
    }

    /// 027 B⑤：私有仓库钩子一律不执行（E6 证实固定 argv 不防已植入钩子）。
    /// 假 pre-commit 仅 `touch` 标记文件（无副作用面）；断言提交成功且标记
    /// 不出现＝hooksPath 覆盖生效，钩子（及其宿主）未被信任。
    #[test]
    fn commit_does_not_execute_planted_hooks() {
        // 与覆盖缝写者（git_timeout_* 测试）串行：git_ok 经 effective_timeout_ms
        // 读全局 GIT_TIMEOUT_MS_OVERRIDE，写方窗口内并发执行会把 300ms 测试
        // 预算泄漏进本测试的真实 git 调用（2026-10-27 全量 flake 根因：run1
        // init 超时/run2 add 超时，报文含「300ms 上限」＝覆盖值指纹；RED 复现
        // 留档）。断言不变，仅隔离并发窗口。
        let _serial = OVERRIDE_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp =
            std::env::temp_dir().join(format!("tchat-027-hooks-{}-{nanos}", std::process::id()));
        let root = tmp.join("root");
        std::fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init"], None).expect("init 仓库");
        std::fs::write(root.join("f.txt"), "x\n").unwrap();
        git_ok(&root, &["add", "."], None).expect("add 文件");
        let hooks = root.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\ntouch hook-marker\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let marker = root.join("hook-marker");
        git_ok(&root, &["commit", "-m", "t1"], None).expect("提交成功（钩子不阻断）");
        assert!(
            !marker.exists(),
            "植入的 pre-commit 钩子不得被执行（027 B⑤ hooksPath=/dev/null）"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// M8 D12-A：便条文件名合同＝上游 V2 add_ad_hoc_note 校验原文。
    #[test]
    fn m8_ad_hoc_note_file_name_contract() {
        assert!(is_valid_ad_hoc_note_file_name("2026-10-19T10-00-00-prefer-chinese.md"));
        assert!(is_valid_ad_hoc_note_file_name("2026-01-02T03-04-05-a.md"), "slug 最短 1 字符");
        assert!(!is_valid_ad_hoc_note_file_name(""), "空名非法");
        assert!(!is_valid_ad_hoc_note_file_name("2026-10-19T10-00-00-.md"), "slug 不得为空");
        assert!(!is_valid_ad_hoc_note_file_name("2026-10-19T10-00-00-Upper.md"), "slug 仅小写");
        assert!(!is_valid_ad_hoc_note_file_name("2026-10-19T10-00-00-a b.md"), "空格非法");
        assert!(!is_valid_ad_hoc_note_file_name("2026-10-19T10-00-00--a.md"), "首字符须字母/数字");
        assert!(is_valid_ad_hoc_note_file_name("2026-13-19T10-00-00-a.md"), "正则不校验日历语义（上游同款）");
        assert!(!is_valid_ad_hoc_note_file_name(&format!("2026-10-19T10-00-00-{}.md", "a".repeat(81))), "slug 超 80 字节非法");
        assert!(is_valid_ad_hoc_note_file_name(&format!("2026-10-19T10-00-00-{}.md", "a".repeat(80))), "slug 恰 80 字节合法");
        assert!(!is_valid_ad_hoc_note_file_name(&format!("2{}.md", "0".repeat(200))), "总长超 128 字节非法");
        assert!(!is_valid_ad_hoc_note_file_name("2026-10-19T10-00-00-a.txt"), "扩展名必须是 .md");
    }

    /// M8 D12-A：正文非全空白判定（读失败/非 UTF-8＝无效；>1 MiB 即有内容）。
    #[test]
    fn m8_ad_hoc_note_body_not_blank() {
        let tmp = std::env::temp_dir().join(format!(
            "tchat-m8-note-body-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let note = tmp.join("note.md");
        std::fs::write(&note, "  \n\t \n\r ").unwrap();
        assert!(!ad_hoc_note_body_not_blank(&note), "纯空白（含换行/制表/CR）＝无效");

        std::fs::write(&note, "\t 有正文 \n").unwrap();
        assert!(ad_hoc_note_body_not_blank(&note), "含非空白字符＝有效（空白仅包裹）");

        std::fs::write(&note, [0xffu8, 0xfe].as_slice()).unwrap();
        assert!(!ad_hoc_note_body_not_blank(&note), "非 UTF-8＝无效");

        assert!(!ad_hoc_note_body_not_blank(&tmp.join("absent.md")), "读失败＝无效");

        std::fs::write(&note, vec![b'a'; 1024 * 1024 + 10]).unwrap();
        assert!(ad_hoc_note_body_not_blank(&note), "超 1 MiB 即有内容（有界读不整读大文件）");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// M8 D12-A：目录内/外边界——非 notes 路径（含同层 instructions.md）不是便条。
    #[test]
    fn m8_note_relative_path_boundary() {
        assert_eq!(
            note_relative_path("extensions/ad_hoc/notes/2026-10-19T10-00-00-a.md"),
            Some("2026-10-19T10-00-00-a.md")
        );
        assert_eq!(note_relative_path("extensions/ad_hoc/instructions.md"), None);
        assert_eq!(note_relative_path("extensions/ad_hoc/notes/sub/a.md"), Some("sub/a.md"), "子目录由上层再判非法");
        assert_eq!(note_relative_path("MEMORY.md"), None);
        assert_eq!(note_relative_path("extensions/ad_hoc/notes"), None);
    }
}
