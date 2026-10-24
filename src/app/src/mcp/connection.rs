//! 022 §2.2／2.4：单服务器连接——Initialize 生命周期、完整分页发现、
//! 单次 `tools/call` 与有界关闭。
//!
//! Initialize 显式选择 `LATEST_WITH_INITIALIZE`（该 SDK 为 2025-11-25），
//! 不使用默认 `LATEST`（无需 initialize 的新生命周期）。不声明 sampling、
//! roots、elicitation 或 Tasks 能力；服务器反向请求 roots／elicitation
//! 覆盖 SDK 默认值统一返回 method-not-found（本项目明确拒绝策略）。
//! 客户端回调收到 `notifications/tools/list_changed` 时置目录过期标记。
//! 单次调用走单次请求句柄（不驱动 MRTR），超时／取消的本地等待立即返回，
//! 取消通知发送任务最多存活 1 秒；迟到结果按请求关联丢弃。

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData as McpError;
use rmcp::handler::client::ClientHandler;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientRequest, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ServerResult, Tool,
};
// roots 反向请求类型沿 SDK（SEP-2577 起弃用，仅用于 method-not-found 签名）。
#[allow(deprecated)]
use rmcp::model::ListRootsResult;
use rmcp::service::{
    serve_client_with_lifecycle, ClientLifecycleMode, MaybeSendFuture, NotificationContext,
    PeerRequestOptions, RequestContext, RoleClient, RunningService, ServiceError,
};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use super::config::{McpServerConfig, McpTransport};
use super::secrets;
use super::transport;

/// 分页边界（移植 Codex pagination.rs，022 §2.2 沿用）。
pub const MAX_CATALOG_PAGES: usize = 100;
pub const MAX_CATALOG_ITEMS: usize = 2_048;
pub const MAX_CURSOR_BYTES: usize = 64 * 1024;
/// 取消／超时通知发送任务的最长存活时间（022 §2.4）。
pub const CANCEL_NOTIFY_BOUND: Duration = Duration::from_secs(1);

/// 连接建立失败（不含凭证值；文本由调用方做污染检查后才可展示）。
#[derive(Debug, Clone)]
pub struct McpConnectError(pub String);

impl std::fmt::Display for McpConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn connect_err(msg: impl Into<String>) -> McpConnectError {
    McpConnectError(msg.into())
}

/// 一次 `tools/call` 的失败分类（022 §2.4 状态语义）。
#[derive(Debug, Clone)]
pub enum CallFailure {
    /// 调用未发出（本地拒绝或发送失败）——远端确定未执行。
    NotSent(String),
    /// 已发出但无法确认结果（超时／取消／断连）——远端执行状态未知。
    SentUnknown(String),
    /// 已发出且远端明确拒绝（JSON-RPC error）——远端确定未执行业务操作。
    SentRejected(String),
    /// 已发出且收到本项目不支持的协议响应（input_required／Task）。
    Unsupported(String),
}

/// 客户端 handler：显式 Initialize 版本、拒绝反向请求、转发目录变化计数。
struct HostClientHandler {
    stale_epoch: Arc<AtomicU64>,
}

impl HostClientHandler {
    /// 022 §2.2：显式选择 LATEST_WITH_INITIALIZE（2025-11-25），不使用
    /// 默认 get_info 的 LATEST（无需 initialize 的新生命周期）。
    fn client_config() -> rmcp::model::ClientConfig {
        rmcp::model::ClientConfig::new(
            rmcp::model::ClientCapabilities::default(),
            rmcp::model::Implementation::new("terminal-chat", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
    }
}

#[allow(deprecated)]
impl ClientHandler for HostClientHandler {
    fn get_info(&self) -> rmcp::model::ClientConfig {
        Self::client_config()
    }

    // 022 §2.2：覆盖 SDK 默认「roots 返回空集合」，改为 method-not-found。
    fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListRootsResult, McpError>> + MaybeSendFuture + '_ {
        std::future::ready(Err(McpError::method_not_found::<
            rmcp::model::ListRootsRequestMethod,
        >()))
    }

    // 覆盖 SDK 默认 elicitation decline，改为 method-not-found。
    fn create_elicitation(
        &self,
        _params: rmcp::model::ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<rmcp::model::ElicitResult, McpError>> + MaybeSendFuture + '_
    {
        std::future::ready(Err(McpError::method_not_found::<
            rmcp::model::ElicitationCreateRequestMethod,
        >()))
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        // 目录变化：单调递增计数（比较语义见 manager 的重列判定）。
        self.stale_epoch.fetch_add(1, Ordering::SeqCst);
        std::future::ready(())
    }
}

/// 单服务器连接的共享状态（快照、分派与展示共用）。
pub struct ConnectionState {
    /// 目录变化计数（notifications/tools/list_changed；与服务任务共享同一
    /// Arc；发布目录时记录当时计数，二者不等＝目录过期）。
    pub stale_epoch: Arc<AtomicU64>,
    /// 连接已关闭／断连（不自动重连，仅显式 reload 重建）。
    pub closed: AtomicBool,
    /// 传输损坏（超限／不可解析行）——连接不可复用（与 stdio 传输共享
    /// 同一标志）。
    pub poisoned: Arc<AtomicBool>,
    /// stdio stderr 有界尾部（原始字节；展示／日志前必须先做凭证污染检查）。
    pub stderr_tail: Arc<std::sync::Mutex<Vec<u8>>>,
    service: AsyncMutex<Option<RunningService<RoleClient, HostClientHandler>>>,
}

impl ConnectionState {
    fn new(
        stderr_tail: Arc<std::sync::Mutex<Vec<u8>>>,
        poisoned: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Self {
            stale_epoch: Arc::new(AtomicU64::new(0)),
            closed: AtomicBool::new(false),
            poisoned,
            stderr_tail,
            service: AsyncMutex::new(None),
        }
        .into()
    }

    /// SDK 服务循环是否已看到传输关闭（断连判定；同步查询经异步锁）。
    pub async fn transport_closed(&self) -> bool {
        match self.service.lock().await.as_ref() {
            Some(service) => service.is_transport_closed(),
            None => true,
        }
    }

    /// stderr 尾部快照（调用方先做污染检查再展示）。
    pub fn stderr_tail_snapshot(&self) -> Option<String> {
        let tail = self.stderr_tail.lock().unwrap();
        if tail.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&tail).to_string())
        }
    }
}

/// 连接（stdio 进程句柄或 HTTP 会话）＋共享状态。
pub struct McpConnection {
    pub state: Arc<ConnectionState>,
    /// stdio 进程句柄（有界关闭与进程组回收用；HTTP 为 None）。
    stdio: Option<Arc<transport::StdioProcess>>,
}

/// 两种传输的统一包装（错误统一为 io::Error，供 SDK 的 IntoTransport 使用）。
enum HostTransport {
    Stdio(transport::BoundedStdioTransport),
    Http(rmcp::transport::streamable_http_client::StreamableHttpClientTransport<transport::BoundedHttpClient>),
}

type PinBoxRes =
    std::pin::Pin<Box<dyn Future<Output = Result<(), std::io::Error>> + Send>>;
type PinBoxOpt<'a> = std::pin::Pin<
    Box<dyn Future<Output = Option<rmcp::service::RxJsonRpcMessage<RoleClient>>> + Send + 'a>,
>;

#[allow(refining_impl_trait_internal)]
impl rmcp::transport::Transport<RoleClient> for HostTransport {
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        match self {
            Self::Stdio(t) => Box::pin(t.send(item)) as PinBoxRes,
            // 先取出 'static 的发送 future，再包装错误映射（async move 不得
            // 捕获 &mut self 引用）。
            Self::Http(t) => {
                let fut = t.send(item);
                Box::pin(async move {
                    fut.await.map_err(|e| std::io::Error::other(e.to_string()))
                })
            }
        }
    }

    fn receive(
        &mut self,
    ) -> impl Future<Output = Option<rmcp::service::RxJsonRpcMessage<RoleClient>>> + Send + '_
    {
        match self {
            Self::Stdio(t) => Box::pin(t.receive()) as PinBoxOpt,
            Self::Http(t) => Box::pin(t.receive()),
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        match self {
            Self::Stdio(t) => t.close().await,
            Self::Http(t) => t.close().await.map_err(|e| std::io::Error::other(e.to_string())),
        }
    }
}


impl McpConnection {
    /// 建立连接并完成 Initialize（不含发现）。失败时回收已启动的进程
    /// （句柄 drop＝进程组后备回收）。
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpConnectError> {
        let (state, stdio, transport) = match &config.transport {
            McpTransport::Stdio(stdio) => {
                let (env, missing) = transport::build_child_env(&stdio.env_vars);
                if !missing.is_empty() {
                    return Err(connect_err(format!("环境变量缺失：{}", missing.join("、"))));
                }
                let (process, t) = transport::StdioProcess::spawn(
                    &stdio.command,
                    &stdio.args,
                    &stdio.cwd,
                    &env,
                )
                .map_err(|e| connect_err(format!("启动服务器进程失败：{e}")))?;
                let process = Arc::new(process);
                let state = ConnectionState::new(
                    process.stderr_tail.clone(),
                    process.poisoned(),
                );
                (state, Some(process), HostTransport::Stdio(t))
            }
            McpTransport::Http(http) => {
                let auth = secrets::resolve_http_auth_refs(http);
                if !auth.missing.is_empty() {
                    return Err(connect_err(format!("凭证缺失：{}", auth.missing.join("、"))));
                }
                let state = ConnectionState::new(
                    Arc::new(std::sync::Mutex::new(Vec::new())),
                    Arc::new(AtomicBool::new(false)),
                );
                let mut custom_headers = std::collections::HashMap::new();
                for (name, value) in &auth.custom_headers {
                    let name = http::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|e| connect_err(format!("请求头名非法：{e}")))?;
                    let value = http::HeaderValue::from_str(value)
                        .map_err(|_| connect_err("请求头值非法"))?;
                    custom_headers.insert(name, value);
                }
                let t = transport::build_http_transport(&http.url, auth.auth_header, custom_headers)
                    .map_err(connect_err)?;
                (state, None, HostTransport::Http(t))
            }
        };
        let handler = HostClientHandler {
            stale_epoch: state.stale_epoch.clone(),
        };
        let service =
            serve_client_with_lifecycle(handler, transport, ClientLifecycleMode::Initialize)
                .await
                .map_err(|e| connect_err(format!("初始化失败：{e}")))?;
        // 版本与能力校验：不兼容版本或未声明 tools 能力记为不可用。
        let info = service
            .peer_info()
            .ok_or_else(|| connect_err("初始化完成但缺少服务器信息"))?;
        if !info.protocol_version.has_initialize() {
            return Err(connect_err(format!(
                "服务器返回不兼容协议版本 {}（需要 initialize 生命周期）",
                info.protocol_version.as_str()
            )));
        }
        if info.capabilities.tools.is_none() {
            return Err(connect_err("服务器未声明 tools 能力"));
        }
        state.service.lock().await.replace(service);
        Ok(Self { state, stdio })
    }

    /// 完整分页发现（022 §2.2：≤100 页、≤2048 项、游标 ≤64 KiB、重复游标
    /// 拒绝、整体时限；空字符串游标也是有效游标；失败不发布半份目录）。
    pub async fn list_all_tools(&self, overall: Duration) -> Result<Vec<Tool>, McpConnectError> {
        let guard = self.state.service.lock().await;
        let service = guard.as_ref().ok_or_else(|| connect_err("连接已关闭"))?;
        let collect = async {
            let mut collected: Vec<Tool> = Vec::new();
            let mut cursor: Option<String> = None;
            let mut seen: HashSet<String> = HashSet::new();
            let mut pages = 0usize;
            loop {
                if pages == MAX_CATALOG_PAGES {
                    return Err(connect_err(format!(
                        "tools/list 超过分页上限 {MAX_CATALOG_PAGES} 页"
                    )));
                }
                pages += 1;
                let params =
                    cursor.map(|c| PaginatedRequestParams::default().with_cursor(Some(c)));
                let result: ListToolsResult = service
                    .peer()
                    .list_tools(params)
                    .await
                    .map_err(|e| connect_err(format!("tools/list 失败：{e}")))?;
                if collected.len() + result.tools.len() > MAX_CATALOG_ITEMS {
                    return Err(connect_err(format!(
                        "tools/list 超过目录上限 {MAX_CATALOG_ITEMS} 项"
                    )));
                }
                collected.extend(result.tools);
                let Some(next) = result.next_cursor else {
                    return Ok(collected);
                };
                if next.len() > MAX_CURSOR_BYTES {
                    return Err(connect_err(format!(
                        "tools/list 游标超过 {MAX_CURSOR_BYTES} 字节"
                    )));
                }
                if !seen.insert(next.clone()) {
                    return Err(connect_err("tools/list 返回重复游标（服务目录错误）"));
                }
                cursor = Some(next);
            }
        };
        tokio::time::timeout(overall, collect)
            .await
            .map_err(|_| connect_err(format!("目录发现超过整体时限 {overall:?}")))?
    }

    /// 单次 `tools/call`（022 §2.4）：`timeout` 为本次调用总时限（不因进度
    /// 消息延长）；`cancel` 置位时立即结束本地等待并发送有界取消通知。
    pub async fn call_tool_once(
        &self,
        raw_tool: &str,
        arguments: Value,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<ServerResult, CallFailure> {
        let guard = self.state.service.lock().await;
        let service = match guard.as_ref() {
            Some(s) => s,
            None => return Err(CallFailure::NotSent("连接已关闭".into())),
        };
        let params = CallToolRequestParams::new(raw_tool.to_string())
            .with_arguments(arguments.as_object().cloned().unwrap_or_default());
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        // 发送：失败＝未发出（McpError 是远端在发送阶段的明确拒绝）。
        let mut handle = service
            .peer()
            .send_cancellable_request(request, PeerRequestOptions::no_options())
            .await
            .map_err(|e| match e {
                ServiceError::McpError(err) => CallFailure::SentRejected(format!("{err}")),
                other => CallFailure::NotSent(format!("{other}")),
            })?;
        // 本地等待：总时限与取消标志都在本地侧绑定；等待结束不依赖通知 I/O，
        // 迟到结果随句柄一起被丢弃（按请求关联）。
        tokio::select! {
            result = &mut handle.rx => {
                match result {
                    Ok(Ok(server_result)) => match server_result {
                        ServerResult::CallToolResult(_) => Ok(server_result),
                        // 不支持 input_required／Task 等后续交互响应。
                        ServerResult::InputRequiredResult(_)
                        | ServerResult::CreateTaskResult(_) => Err(CallFailure::Unsupported(
                            "服务器返回 input_required／Task 响应，本版本不支持".into(),
                        )),
                        other => Err(CallFailure::SentRejected(format!(
                            "不支持的协议响应：{other:?}"
                        ))),
                    },
                    Ok(Err(ServiceError::McpError(err))) => {
                        Err(CallFailure::SentRejected(format!("{err}")))
                    }
                    Ok(Err(other)) => Err(CallFailure::SentUnknown(format!("{other}"))),
                    Err(_) => Err(CallFailure::SentUnknown(
                        "传输已关闭，响应未到达".into(),
                    )),
                }
            }
            _ = tokio::time::sleep(timeout) => {
                spawn_bounded_cancel(handle, "调用超时");
                Err(CallFailure::SentUnknown("调用超时，远端执行状态未知".into()))
            }
            _ = wait_cancel_flag(cancel) => {
                spawn_bounded_cancel(handle, "用户取消");
                Err(CallFailure::SentUnknown("用户取消，远端执行状态未知".into()))
            }
        }
    }

    /// 传输是否已断（断连后连接不再声明工具，仅显式 reload 重建）。
    pub fn is_transport_closed(&self) -> bool {
        self.state.closed.load(Ordering::SeqCst) || self.state.poisoned.load(Ordering::SeqCst)
    }

    /// 有界关闭（022 §2.5）：结束服务循环；stdio 关 stdin 后最多等 3 秒，
    /// 再终止并回收整个进程组。
    pub async fn close(&self) {
        self.state.closed.store(true, Ordering::SeqCst);
        let service = self.state.service.lock().await.take();
        if let Some(service) = service {
            let _ = service.cancel().await;
        }
        if let Some(process) = &self.stdio {
            process.close().await;
        }
    }
}

/// 有界取消通知：任务最多存活 [`CANCEL_NOTIFY_BOUND`]，等待侧不依赖其完成。
fn spawn_bounded_cancel(
    handle: rmcp::service::RequestHandle<RoleClient>,
    reason: &str,
) {
    let reason = reason.to_string();
    tokio::spawn(async move {
        let _ = tokio::time::timeout(CANCEL_NOTIFY_BOUND, handle.cancel(Some(reason))).await;
    });
}

/// 轮询同步取消标志（50ms 粒度；同步 AtomicBool 桥接进异步等待）。
pub(crate) async fn wait_cancel_flag(cancel: &AtomicBool) {
    loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
