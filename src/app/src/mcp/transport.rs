//! 022 §2.4 接收限额与传输边界。
//!
//! stdio：函数级移植 Codex `rmcp-client/src/bounded_stdio_transport.rs`
//! （固定提交 d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4，MIT/Apache-2.0）的
//! 8 MiB 行界限，关闭语义移植 `local_stdio_transport.rs`（关 stdin 后最多等
//! 3 秒再终止），并按本项目要求补独立进程组回收与环境白名单
//! （移植 `rmcp-client/src/utils.rs` 的 Unix 缺省白名单）。
//! HTTP：函数级移植 SDK `transport/common/reqwest/streamable_http_client.rs`
//! 的 `StreamableHttpClient` 适配器与 `client_side_sse.rs` 的 SSE 单事件
//! 限额器（同 rev，Apache-2.0；SDK 版本为 `pub(crate)`，无法直接复用）；
//! 错误体与 JSON 响应体改为有界读取（8 MiB）；重定向保持禁用。
//! 环境：只转交平台白名单与显式 `env_vars`，不继承整份宿主环境。

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{ready, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{BoxStream, Stream, StreamExt};
use http::{HeaderName, HeaderValue};
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::{RoleClient, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    StreamableHttpPostResponse, StreamableHttpError,
};
use rmcp::transport::Transport;
use sse_stream::{Error as SseError, Sse, SseStream};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex as AsyncMutex;

/// 单条 JSON 消息／HTTP JSON 体／单个 SSE 事件上限（022 §2.4：8 MiB）。
pub const MAX_JSON_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
/// stdio 关闭后等待退出的宽限（沿用上游 3 秒）。
pub const CLOSE_GRACE: Duration = Duration::from_secs(3);
/// stderr 有界诊断尾部上限。
pub const STDERR_TAIL_BYTES: usize = 32 * 1024;

/// 平台缺省环境白名单（移植 Codex `utils.rs` 的 Unix 名单；Windows 运行时
/// 验证不在本轮范围）。
pub const DEFAULT_ENV_VARS: &[&str] = &[
    "HOME",
    "LOGNAME",
    "PATH",
    "SHELL",
    "USER",
    "__CF_USER_TEXT_ENCODING",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "TZ",
];

/// 构造子进程环境：先清空继承环境，再转交平台白名单与显式引用的现有变量。
/// 返回（键值对、缺失的显式变量名——调用方据此让该服务器准备失败）。
pub fn build_child_env(env_vars: &[String]) -> (Vec<(String, String)>, Vec<String>) {
    let mut out: HashMap<String, String> = HashMap::new();
    let mut missing = Vec::new();
    let defaults = DEFAULT_ENV_VARS.iter().map(|s| s.to_string());
    for name in defaults.chain(env_vars.iter().cloned()) {
        if out.contains_key(&name) {
            continue;
        }
        match std::env::var(&name) {
            Ok(value) => {
                out.insert(name, value);
            }
            Err(_) if env_vars.contains(&name) => missing.push(name),
            Err(_) => {}
        }
    }
    let mut list: Vec<(String, String)> = out.into_iter().collect();
    list.sort();
    (list, missing)
}

pub struct BoundedStdioTransport {
    stdin: Arc<AsyncMutex<Option<ChildStdin>>>,
    stdout: BufReader<ChildStdout>,
    pending_line: Vec<u8>,
    /// 超限或无法解析的行后置位（此后连接视为已断、不可复用）。
    poisoned: Arc<AtomicBool>,
}

impl BoundedStdioTransport {
    pub fn new(stdin_slot: Arc<AsyncMutex<Option<ChildStdin>>>, stdout: ChildStdout) -> Self {
        Self {
            stdin: stdin_slot,
            stdout: BufReader::new(stdout),
            pending_line: Vec::new(),
            poisoned: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn poisoned(&self) -> Arc<AtomicBool> {
        self.poisoned.clone()
    }

    async fn receive_message(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        loop {
            let bytes = match self.stdout.fill_buf().await {
                Ok(bytes) => bytes,
                Err(_) => return None,
            };
            if bytes.is_empty() {
                if self.pending_line.is_empty() {
                    return None;
                }
                return self.decode_pending_message();
            }
            let newline = bytes.iter().position(|b| *b == b'\n');
            let content_len = newline.unwrap_or(bytes.len());
            if content_len > MAX_JSON_MESSAGE_BYTES.saturating_sub(self.pending_line.len()) {
                // 超限：不分配完整消息，结束传输（连接关闭，在途调用报错）。
                self.pending_line.clear();
                self.poisoned.store(true, Ordering::SeqCst);
                return None;
            }
            self.pending_line.extend_from_slice(&bytes[..content_len]);
            let consumed = content_len + usize::from(newline.is_some());
            self.stdout.consume(consumed);
            if newline.is_some() {
                if let Some(message) = self.decode_pending_message() {
                    return Some(message);
                }
            }
        }
    }

    fn decode_pending_message(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        let line = std::mem::take(&mut self.pending_line);
        let line = line.strip_suffix(b"\r").unwrap_or(&line);
        match serde_json::from_slice(line) {
            Ok(message) => Some(message),
            // 无法解析的行：置损坏标志；接收循环继续读下一行（坏行跳过）。
            Err(_) => {
                self.poisoned.store(true, Ordering::SeqCst);
                None
            }
        }
    }
}

impl Transport<RoleClient> for BoundedStdioTransport {
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let stdin = Arc::clone(&self.stdin);
        async move {
            let mut message = serde_json::to_vec(&item).map_err(io::Error::other)?;
            message.push(b'\n');
            let mut guard = stdin.lock().await;
            let stdin = guard
                .as_mut()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "MCP stdin 已关闭"))?;
            stdin.write_all(&message).await?;
            stdin.flush().await
        }
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        self.receive_message()
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.stdin.lock().await.take();
        Ok(())
    }
}

/// 已启动的 stdio 子进程：独立进程组回收、有界 stderr 尾部、有界关闭。
pub struct StdioProcess {
    /// 子进程 PID（＝进程组 ID；process_group(0) 保证）。
    pub pid: u32,
    /// 与传输共享的 stdin 槽位（close＝取走并丢弃 → 对端收到 EOF）。
    stdin_slot: Arc<AsyncMutex<Option<ChildStdin>>>,
    child: Mutex<Option<Child>>,
    #[cfg(target_os = "linux")]
    sandbox_payload: Option<crate::sandbox::SandboxPayloadIdentity>,
    /// 有界 stderr 尾部（原始字节；展示／日志前必须先做凭证污染检查）。
    pub stderr_tail: Arc<Mutex<Vec<u8>>>,
    poisoned: Arc<AtomicBool>,
}

impl StdioProcess {
    /// 直接按程序与参数数组启动（不经隐式 shell）；stdin／stdout 为管道，
    /// stderr 由本项目排空；进程进入独立进程组。返回（进程、协议传输）。
    pub fn spawn(
        command: &str,
        args: &[String],
        cwd: &std::path::Path,
        env: &[(String, String)],
    ) -> io::Result<(Self, BoundedStdioTransport)> {
        use std::process::Stdio;
        // 033：沙箱模式（TCHAT_SANDBOX）下经 bwrap 包装启动——文件系统策略在
        // 主进程构造（全盘只读＋cwd 可写＋.env 遮蔽）；沙箱不可用＝错误，不静默
        // 回退。普通模式按原样启动，行为零变化。
        // 031：启用动态策略时 MCP stdio 走**基线**快照（受限读树等）——长期
        // 连接不继承任何本轮临时授权（§1.6：需更宽范围由管理员改基线并重启）。
        let mut cmd_parts = vec![command.to_string()];
        cmd_parts.extend(args.iter().cloned());
        // 031 与 033 的所有沙箱分支都使用相同 gate。普通模式保持原 argv。
        #[cfg(target_os = "linux")]
        let sandbox_gate = crate::sandbox::sandbox_requested()
            || crate::permissions::frozen().is_some();
        #[cfg(target_os = "linux")]
        if sandbox_gate {
            let (g_program, g_args) =
                crate::sandbox::mcp_stdio_gate_wrap(&command.to_string(), args);
            let mut wrapped: Vec<String> = Vec::with_capacity(g_args.len() + 1);
            wrapped.push(g_program);
            wrapped.extend(g_args);
            cmd_parts = wrapped;
        }
        let mut cmd;
        // 031 分支的快照/argv 构造失败＝Err（fail-closed：不回退 033 固定策略）。
        let permission_launch = match crate::permissions::frozen() {
            Some(f) => {
                let snap = crate::permissions::baseline_snapshot(&f)
                    .map_err(|e| io::Error::other(format!("031 基线快照构建失败：{e}")))?;
                let launch = crate::sandbox::permission_launch_argv_with_payload_program(
                    cmd_parts.clone(),
                    &snap,
                    cwd,
                    Some(command),
                )?;
                Some(launch)
            }
            None => None,
        };
        // seccomp fd 守卫存活到下方 spawn 语句执行完毕（绑定保持所有权）。
        let _seccomp_guard;
        #[cfg(target_os = "linux")]
        let sandbox_requested_launch = sandbox_gate;
        let sandbox_env = if let Some(launch) = permission_launch {
            _seccomp_guard = launch.seccomp;
            cmd = tokio::process::Command::new(&launch.program);
            cmd.args(&launch.args);
            #[cfg(unix)]
            unsafe {
                match _seccomp_guard.as_ref() {
                    Some(g) => {
                        let fd = g.raw();
                        cmd.pre_exec(move || {
                            crate::sandbox::close_inherited_fds_except_cloexec(fd)
                        });
                    }
                    None => {
                        cmd.pre_exec(crate::sandbox::close_inherited_fds_mark_cloexec);
                    }
                }
            }
            Some(launch.env)
        } else {
            match crate::sandbox::tool_launch_argv(cmd_parts)? {
                Some((program, sandbox_args, child_env)) => {
                    cmd = tokio::process::Command::new(program);
                    cmd.args(sandbox_args);
                    // 033-B2：同 exec 面——继承 fd ≥3 标记 CLOEXEC（execve 时关闭）；
                    // tokio Command 自带 pre_exec（委托 std）。仅沙箱分支、unix。
                    #[cfg(unix)]
                    unsafe {
                        cmd.pre_exec(crate::sandbox::close_inherited_fds_mark_cloexec);
                    }
                    // 白名单环境在下方 env_clear **之后**注入（env_clear 是清除不是
                    // 覆盖——先注会被清掉，033 审查轮修正）。
                    Some(child_env)
                }
                None => {
                    cmd = tokio::process::Command::new(command);
                    cmd.args(args);
                    None
                }
            }
        };
        cmd.current_dir(cwd)
            .env_clear()
            // 注入顺序（2026-10-27 宿主验收实测修正）：平台缺省白名单＋mcp.json
            // 显式 env 先落地，沙箱白名单**最后**落地——普通模式（sandbox_env
            // 为空）与既有行为完全一致；沙箱模式下 HOME/PATH/TERM 以沙箱策略值
            //（HOME＝锚定根）优先。初版顺序相反：平台白名单的 HOME（主进程值，
            // 如 /home/dev）后落覆盖了沙箱锚定 HOME——真实分支首跑即暴露
            //（a033_sandboxed_stdio 断言）。
            .envs(env.iter().map(|(k, v)| (k, v)))
            .envs(sandbox_env.unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        // 033 生命周期：spawn 经 runtime worker 分离任务执行。句柄丢弃时的回收
        // 分两层：kill_on_drop 只终止直接子进程（覆盖本函数构造 StdioProcess
        // 之前的提前返回路径；tokio 默认 false 不回收）；进程组级后备回收由
        // StdioProcess 的 Drop impl 承担。正常路径句柄由连接持有，显式 close/
        // 进程组终止不受影响。
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn()?;
        // 038 方案 A：沙箱启动确认闸——载荷自停（包装形态：exec 前已 STOP），
        // 确认「隔离＋清理标识」后放行（SIGCONT）。超时/失败不放行：整组终止
        // ＋有界回收；LeaderGone＝--die-with-parent 级联已生效（载荷未执行）。
        // 确认调用不带 cancel（None）：连接取消/超时只丢 JoinHandle，本分离
        // 任务照常跑完（确认后可能已放行），回收靠任务结果丢弃时的 Drop
        // ＋kill_on_drop。
        #[cfg(target_os = "linux")]
        let mut sandbox_payload = None;
        #[cfg(target_os = "linux")]
        if sandbox_requested_launch {
            match crate::sandbox::confirm_sandbox_isolation_and_release(
                child.id().ok_or_else(|| io::Error::other("MCP 进程已退出"))?,
                None,
            ) {
                Ok(crate::sandbox::SandboxConfirm::Released(identity)) => {
                    sandbox_payload = Some(identity);
                }
                Ok(crate::sandbox::SandboxConfirm::LeaderGone) => {
                    return Err(io::Error::other(
                        "MCP 服务器监督进程在确认前退出（载荷未执行）",
                    ));
                }
                Err(e) => return Err(io::Error::other(format!("MCP 服务器隔离确认失败：{e}"))),
            }
        }
        let pid = child.id().ok_or_else(|| io::Error::other("MCP 进程已退出"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("MCP stdin 未接管道"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("MCP stdout 未接管道"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("MCP stderr 未接管道"))?;
        let stdin_slot = Arc::new(AsyncMutex::new(Some(stdin)));
        let transport = BoundedStdioTransport::new(stdin_slot.clone(), stdout);
        let poisoned = transport.poisoned();
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        spawn_stderr_drain(stderr, stderr_tail.clone());
        Ok((
            Self {
                pid,
                stdin_slot,
                child: Mutex::new(Some(child)),
                #[cfg(target_os = "linux")]
                sandbox_payload,
                stderr_tail,
                poisoned,
            },
            transport,
        ))
    }

    pub fn poisoned(&self) -> Arc<AtomicBool> {
        self.poisoned.clone()
    }

    /// 有界关闭：关闭 stdin（EOF）→ 最多等 3 秒 → 终止并回收整个进程组。
    pub async fn close(&self) {
        self.stdin_slot.lock().await.take();
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            match tokio::time::timeout(CLOSE_GRACE, child.wait()).await {
                Ok(Ok(_)) => {}
                _ => {
                    #[cfg(target_os = "linux")]
                    if let Some(payload) = self.sandbox_payload.as_ref() {
                        let _ = crate::sandbox::signal_sandbox_payload_group(
                            payload,
                            libc::SIGKILL,
                        );
                        let ns = payload.ns_inode.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            crate::sandbox::sweep_sandbox_namespace(&ns)
                        }).await;
                        // Let the namespace PID 1 reap after the D2 sweep. If it does
                        // not exit, retain the existing bounded process-group fallback.
                        if tokio::time::timeout(CLOSE_GRACE, child.wait())
                            .await
                            .is_err()
                        {
                            kill_process_group(self.pid);
                            let _ = child.start_kill();
                            let _ =
                                tokio::time::timeout(CLOSE_GRACE, child.wait()).await;
                        }
                    } else {
                        kill_process_group(self.pid);
                        let _ = child.wait().await;
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        kill_process_group(self.pid);
                        let _ = child.wait().await;
                    }
                }
            }
            // 040：沙箱化 MCP 服务器的 bwrap 监督进程已在上方路径收割，
            // 就地回收其被收养的 ns-PID-1 reaper（非沙箱子进程无匹配）。
            #[cfg(target_os = "linux")]
            crate::sandbox::reap_adopted_bwrap_reapers();
        }
    }
}

impl Drop for StdioProcess {
    fn drop(&mut self) {
        // 后备回收：tokio Child 的 kill 只作用于直接子进程，孙进程需按组终止。
        #[cfg(target_os = "linux")]
        if let Some(payload) = self.sandbox_payload.as_ref() {
            let _ = crate::sandbox::signal_sandbox_payload_group(payload, libc::SIGKILL);
        }
        kill_process_group(self.pid);
    }
}

fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        // 负数 PID＝终止整个进程组；组已不存在时静默（进程已退出）。
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

/// stderr 持续排空：只保留最后 [`STDERR_TAIL_BYTES`] 字节。
fn spawn_stderr_drain(stderr: tokio::process::ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    tokio::spawn(async move {
        let mut reader = stderr;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    let mut tail = tail.lock().unwrap();
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > STDERR_TAIL_BYTES {
                        let excess = tail.len() - STDERR_TAIL_BYTES;
                        tail.drain(..excess);
                    }
                }
            }
        }
    });
}

/// 移植 SDK `SseEventSizeLimiter`：逐字节跟踪当前事件累计尺寸（注释行不计），
/// 空行结束事件。事件超过上限即失败（在完整解析前拒绝）。
#[derive(Debug)]
struct SseEventSizeLimiter {
    max_size: usize,
    retained_size: usize,
    line_size: usize,
    line_is_comment: bool,
    previous_was_cr: bool,
}

impl SseEventSizeLimiter {
    fn new(max_size: usize) -> Self {
        Self {
            max_size,
            retained_size: 0,
            line_size: 0,
            line_is_comment: false,
            previous_was_cr: false,
        }
    }

    fn observe(&mut self, chunk: &[u8]) -> Result<(), ()> {
        for &byte in chunk {
            if self.previous_was_cr {
                self.previous_was_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.finish_line()?;
                    self.previous_was_cr = true;
                }
                b'\n' => self.finish_line()?,
                _ => {
                    if self.line_size == 0 {
                        self.line_is_comment = byte == b':';
                    }
                    self.line_size = self.line_size.saturating_add(1);
                    self.check_limit()?;
                }
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> Result<(), ()> {
        if self.line_size == 0 {
            self.retained_size = 0;
        } else if !self.line_is_comment {
            // SSE 解析器在拼接多个 data 字段时会插入换行。
            self.retained_size = self
                .retained_size
                .saturating_add(self.line_size)
                .saturating_add(1);
        }
        self.line_size = 0;
        self.line_is_comment = false;
        self.check_limit()
    }

    fn check_limit(&self) -> Result<(), ()> {
        if self.retained_size.saturating_add(self.line_size) > self.max_size {
            Err(())
        } else {
            Ok(())
        }
    }
}

fn event_too_large_error(max_size: usize) -> SseError {
    SseError::Body(
        format!("SSE event exceeds the maximum size of {max_size} bytes").into(),
    )
}

/// 有界 SSE 字节流：事件超限时终止流并回报错误（原始字节原样透传）。
struct BoundedSseByteStream<S> {
    inner: S,
    limiter: SseEventSizeLimiter,
    failed: bool,
}

impl<S, E> Stream for BoundedSseByteStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = Result<Bytes, SseError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.failed {
            return Poll::Ready(None);
        }
        match ready!(this.inner.poll_next_unpin(cx)) {
            Some(Ok(chunk)) => {
                if this.limiter.observe(&chunk).is_err() {
                    this.failed = true;
                    Poll::Ready(Some(Err(event_too_large_error(this.limiter.max_size))))
                } else {
                    Poll::Ready(Some(Ok(chunk)))
                }
            }
            Some(Err(error)) => {
                this.failed = true;
                Poll::Ready(Some(Err(SseError::Body(Box::new(error)))))
            }
            None => Poll::Ready(None),
        }
    }
}

/// 组装有界 SSE 事件流（替代 SDK 的 pub(crate) bounded_sse_stream）。
fn bounded_sse_stream(stream: reqwest::Response) -> BoxStream<'static, Result<Sse, SseError>> {
    let bytes = stream
        .bytes_stream()
        .map(|r| r.map_err(|e| SseError::Body(Box::new(e))));
    let bounded = BoundedSseByteStream {
        inner: bytes,
        limiter: SseEventSizeLimiter::new(MAX_JSON_MESSAGE_BYTES),
        failed: false,
    };
    SseStream::from_bytes_stream(bounded).boxed()
}

#[derive(Debug)]
pub enum HttpError {
    Reqwest(reqwest::Error),
    /// 响应体超过 8 MiB 上限（在完整分配／解析前拒绝）。
    BodyTooLarge,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reqwest(e) => write!(f, "{e}"),
            Self::BodyTooLarge => write!(f, "HTTP 响应体超过 8 MiB 上限"),
        }
    }
}

impl std::error::Error for HttpError {}

impl From<reqwest::Error> for HttpError {
    fn from(e: reqwest::Error) -> Self {
        Self::Reqwest(e)
    }
}

fn client_err(e: reqwest::Error) -> StreamableHttpError<HttpError> {
    StreamableHttpError::Client(HttpError::Reqwest(e))
}

/// 有界读取响应体：累计超过上限即中止（不完整分配）。
async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, StreamableHttpError<HttpError>> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(client_err)?;
        let Some(chunk) = chunk else {
            return Ok(out);
        };
        if out.len() + chunk.len() > MAX_JSON_MESSAGE_BYTES {
            return Err(StreamableHttpError::Client(HttpError::BodyTooLarge));
        }
        out.extend_from_slice(&chunk);
    }
}

/// 移植 SDK `impl StreamableHttpClient for reqwest::Client` 的适配器；
/// `text()`／`bytes()` 改为 [`bounded_body`]（8 MiB）。
#[derive(Clone)]
pub struct BoundedHttpClient {
    inner: reqwest::Client,
}

impl BoundedHttpClient {
    /// 关闭自动重定向（认证头不得重放到跳转目标）＋禁用空闲连接池
    /// （沿用 SDK 默认客户端配置）。
    pub fn new() -> Result<Self, String> {
        let mut builder = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none());
        // 022 §5.4：测试注入的自签根（doc(hidden) 接缝）；生产默认 None，
        // 只用系统/WebPKI 根，不放宽任何证书校验。
        // 038 Windows/macOS CI 实证（43139153907）：rustls feature 的默认
        // 验证器＝platform-verifier（系统证书存储）——自签测试根不在系统
        // 存储中，add_root_certificate 的注入根在 win/mac 平台验证路径不可
        // 达（ubuntu 平台验证器降级读 rustls 根存储故绿）。测试经
        // `set_client_builder_hook_for_tests` 装饰 builder（rustls 配置的
        // 具名构造在 dev-dependency 侧闭包内完成）——校验严格性不变，仅
        // 信任根来源不同；生产路径（无 hook）走 platform-verifier 不变。
        if let Some(hook) = client_builder_hook() {
            builder = hook(builder);
        } else if let Some(pem) = extra_root().as_ref() {
            let cert = reqwest::tls::Certificate::from_pem(pem)
                .map_err(|e| format!("测试根证书无效：{e}"))?;
            builder = builder.add_root_certificate(cert);
        }
        let inner = builder
            .build()
            .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))?;
        Ok(Self { inner })
    }
}

/// 022 §5.4：仅测试用的额外信任根（进程级、默认空）。生产代码不调用 setter。
static EXTRA_ROOT: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);

/// 038（同上）：测试侧 builder 装饰器（Arc 可克隆——每次连接重建客户端
/// 都须命中；构造在 dev-dependency 侧闭包内，产品不具名具体 TLS 栈类型）。
type ClientBuilderHook =
    std::sync::Arc<dyn Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync>;
static BUILDER_HOOK: std::sync::Mutex<Option<ClientBuilderHook>> = std::sync::Mutex::new(None);

fn client_builder_hook() -> Option<ClientBuilderHook> {
    BUILDER_HOOK.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn extra_root() -> Option<Vec<u8>> {
    EXTRA_ROOT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 测试接缝：注入/复位 TLS mock 的自签根证书。不进入任何生产默认路径。
#[doc(hidden)]
pub fn set_extra_root_certificate_for_tests(pem: Option<&[u8]>) {
    *EXTRA_ROOT.lock().unwrap_or_else(|e| e.into_inner()) = pem.map(<[u8]>::to_vec);
}

/// 038 测试接缝（同 `set_extra_root_certificate_for_tests` 约定）：装饰
/// HTTP 客户端 builder（如 rustls `tls_backend_preconfigured`＋自签根——
/// 具名 TLS 栈构造在 dev-dependency 侧闭包内完成）。持续生效（每次连接
/// 重建客户端都命中）；生产代码不调用。
#[doc(hidden)]
pub fn set_client_builder_hook_for_tests(
    hook: std::sync::Arc<dyn Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync>,
) {
    *BUILDER_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
}

/// 038 测试接缝：显式复位 builder 装饰器（hook 为一次性取用，此为防御性
/// 复位接口）。
#[doc(hidden)]
pub fn clear_client_builder_hook_for_tests() {
    *BUILDER_HOOK.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn apply_custom_headers(
    builder: reqwest::RequestBuilder,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<reqwest::RequestBuilder, StreamableHttpError<HttpError>> {
    let mut builder = builder;
    for (name, value) in custom_headers {
        builder = builder.header(name, value);
    }
    Ok(builder)
}

impl StreamableHttpClient for BoundedHttpClient {
    type Error = HttpError;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let mut request = self
            .inner
            .get(uri.as_ref())
            .header("Accept", "text/event-stream, application/json");
        if let Some(session_id) = session_id {
            request = request.header("MCP-Session-Id", session_id.as_ref());
        }
        if let Some(last_event_id) = last_event_id {
            request = request.header("Last-Event-ID", last_event_id);
        }
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        request = apply_custom_headers(request, custom_headers)?;
        let response = request.send().await.map_err(client_err)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let body = bounded_body(response).await?;
            return Err(StreamableHttpError::UnexpectedServerResponse(
                String::from_utf8_lossy(&body).into_owned().into(),
            ));
        }
        let response = response.error_for_status().map_err(client_err)?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                let ct = ct.as_bytes();
                if !ct.starts_with(b"text/event-stream") && !ct.starts_with(b"application/json") {
                    return Err(StreamableHttpError::UnexpectedContentType(Some(
                        String::from_utf8_lossy(ct).to_string(),
                    )));
                }
            }
            None => return Err(StreamableHttpError::UnexpectedContentType(None)),
        }
        Ok(bounded_sse_stream(response))
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request = self.inner.delete(uri.as_ref());
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        request = request.header("MCP-Session-Id", session.as_ref());
        request = apply_custom_headers(request, custom_headers)?;
        let response = request.send().await.map_err(client_err)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        response.error_for_status().map_err(client_err)?;
        Ok(())
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let mut request = self
            .inner
            .post(uri.as_ref())
            .header("Accept", "text/event-stream, application/json");
        if let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        request = apply_custom_headers(request, custom_headers)?;
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header("MCP-Session-Id", session_id.as_ref());
        }
        let response = request.json(&message).send().await.map_err(client_err)?;
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            let body = bounded_body(response).await.unwrap_or_default();
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("HTTP {status}: {}", String::from_utf8_lossy(&body)).into(),
            ));
        }
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            // 会话失效：reinit_on_expired_session=false 时原样报错（不重放）。
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let content_length = response.content_length();
        let session_id = response
            .headers()
            .get("MCP-Session-Id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        if status.is_success()
            && content_length == Some(0)
            && matches!(
                message,
                ClientJsonRpcMessage::Notification(_)
                    | ClientJsonRpcMessage::Response(_)
                    | ClientJsonRpcMessage::Error(_)
            )
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if !status.is_success() {
            let body = bounded_body(response).await.unwrap_or_default();
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.starts_with("application/json"))
            {
                if let Ok(parsed) = serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    if matches!(parsed, ServerJsonRpcMessage::Error(_)) {
                        return Ok(StreamableHttpPostResponse::Json(parsed, session_id));
                    }
                }
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("HTTP {status}: {}", String::from_utf8_lossy(&body)).into(),
            ));
        }
        match content_type.as_deref() {
            Some(ct) if ct.starts_with("text/event-stream") => {
                Ok(StreamableHttpPostResponse::Sse(
                    bounded_sse_stream(response),
                    session_id,
                ))
            }
            Some(ct) if ct.starts_with("application/json") => {
                let body = bounded_body(response).await?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(parsed) => Ok(StreamableHttpPostResponse::Json(parsed, session_id)),
                    Err(_)
                        if matches!(
                            message,
                            ClientJsonRpcMessage::Notification(_)
                                | ClientJsonRpcMessage::Response(_)
                                | ClientJsonRpcMessage::Error(_)
                        ) =>
                    {
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                    Err(e) => Err(StreamableHttpError::UnexpectedServerResponse(
                        format!("JSON-RPC 解析失败：{e}").into(),
                    )),
                }
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(content_type)),
        }
    }
}

/// 组建 Streamable HTTP 传输（认证与自定义头在连接准备时注入；
/// `reinit_on_expired_session` 显式关闭，404 会话失效不重放原 POST）。
pub fn build_http_transport(
    uri: &str,
    auth_header: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<StreamableHttpClientTransport<BoundedHttpClient>, String> {
    let client = BoundedHttpClient::new()?;
    let mut config = StreamableHttpClientTransportConfig::with_uri(uri);
    if let Some(auth) = auth_header {
        config = config.auth_header(auth);
    }
    config.custom_headers = custom_headers;
    config.reinit_on_expired_session = false;
    config.max_sse_event_size = MAX_JSON_MESSAGE_BYTES;
    Ok(StreamableHttpClientTransport::with_client(client, config))
}
