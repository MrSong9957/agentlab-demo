//! 022 §2.2／2.5：MCP 管理器——配置加载、连接生命周期、目录发布与每轮快照。
//!
//! - 首次普通用户轮准备时或显式 `/mcp reload` 时加载配置；帮助、模型目录、
//!   会话列表与单纯恢复展示不触发 MCP 启动；内部记忆轮不创建管理器。
//! - 连接在普通用户轮之间复用；`notifications/tools/list_changed` 只标记
//!   过期，下一轮完整重列（至多每轮一次；重列期间的新通知保留过期标记）。
//! - 重列失败后该服务器不可用，只有显式 reload 才重连；确认断开的连接
//!   不在后续新轮声明工具。
//! - reload 先完整解析新配置再替换；解析失败保留原配置并提示「未应用
//!   新配置」；移除／停用立即关闭旧连接；新连接失败不回用旧连接。
//! - 宿主退出显式关闭全部连接（stdio 关 stdin 后有界 3 秒＋进程组回收）。
//!
//! 并发模型：专用 tokio runtime（每连接一个服务任务），同步侧经 `block_on`
//! 桥接；`operate` 互斥保证 prepare 与 reload 不并发（宿主忙检查之外的
//! 第二道防线）。身份比较仅在内存中进行，不输出指纹。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError, Weak};
use std::time::Duration;

use serde_json::Value;

use super::call;
use super::catalog::{build_function_definition, build_server_catalog};
use super::config::{load_mcp_file, McpServerConfig, McpTransport};
use super::connection::{wait_cancel_flag, CallFailure, McpConnection};
use super::naming::assign_model_names;
use super::secrets::{collect_sensitive_values, SensitiveValues};

/// 每个用户轮冻结的 MCP 工具快照：定义＋原始身份映射＋连接句柄，
/// 所有本轮模型请求和实际分派共用它（不可变）。
#[derive(Default)]
pub struct McpSnapshot {
    /// 按 (server_id, raw_tool) 稳定排序。
    entries: Vec<ToolEntry>,
    /// 与 entries 同序的 CC 函数定义。
    definitions: Vec<Value>,
    /// 归属管理器（弱引用；调用分派经其 runtime 驱动）。
    manager: Option<Weak<McpManager>>,
}

impl McpSnapshot {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 本快照发布的工具数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// CC 函数定义（追加在原生工具之后）。
    pub fn definitions(&self) -> &[Value] {
        &self.definitions
    }

    /// 按模型名精确查找（只查映射；不拆 `__`、不看前缀）。
    pub fn entry(&self, model_name: &str) -> Option<&ToolEntry> {
        self.entries.iter().find(|e| e.model_name == model_name)
    }

    /// 执行一次外部调用（分派入口；内部委托管理器——结果投影与凭证污染
    /// 检查逻辑与 manager.call 同源）。返回（工具结果文本、isError）。
    pub fn call(
        &self,
        entry: &ToolEntry,
        arguments: Value,
        cancel: &AtomicBool,
    ) -> (String, bool) {
        match &self.manager {
            Some(manager) => match manager.upgrade() {
                Some(manager) => manager.call(entry, arguments, cancel),
                None => (
                    super::call::client_error_json("内部", "MCP 管理器已释放"),
                    true,
                ),
            },
            None => (
                super::call::client_error_json("内部", "MCP 管理器不可用"),
                true,
            ),
        }
    }
}

/// 快照内单个可调用工具。
pub struct ToolEntry {
    pub model_name: String,
    pub server_id: String,
    pub raw_tool: String,
    /// 服务器提供的工具描述（已通过凭证污染检查；进 CC 定义）。
    pub description: Option<String>,
    /// 归一后的输入 Schema（声明与执行一致性由同一份 Schema 保证）。
    pub schema: Value,
    pub connection: Arc<McpConnection>,
    pub sensitive: Arc<SensitiveValues>,
    pub tool_timeout: Duration,
}

/// 单台服务器的已发布目录（连接健康时有效）。
struct ServerListing {
    sensitive: Arc<SensitiveValues>,
    /// (原始工具名, 描述, 归一 Schema)，按原始名稳定排序；模型名由发布时
    /// 全局分配（跨服务器冲突经稳定短哈希消解，见 `naming::assign_model_names`）。
    tools: Vec<(String, Option<String>, Value)>,
    /// 发布时的目录变化计数（与连接计数不等＝过期）。
    listed_epoch: u64,
}

struct ServerState {
    id: String,
    config: McpServerConfig,
    connection: Option<Arc<McpConnection>>,
    /// 脱敏后的错误（连接／发现失败）。
    error: Option<String>,
    listing: Option<Arc<ServerListing>>,
    /// 允许名单中的未知项提示。
    unknown_allowlisted: Vec<String>,
    /// 被排除项原因（Schema／污染；脱敏后）。
    excluded: Vec<String>,
}

struct Inner {
    /// 配置已加载（含「文件缺失＝空配置」）；false＝尚未尝试。
    loaded: bool,
    load_error: Option<String>,
    servers: Vec<ServerState>,
}

/// MCP 管理器。
pub struct McpManager {
    runtime: tokio::runtime::Runtime,
    inner: Mutex<Inner>,
    /// prepare 与 reload 互斥。
    operate: Mutex<()>,
    /// 自身弱引用（快照回呼分派；由 [`McpManager::new`] 的 Arc 装配）。
    self_weak: OnceLock<Weak<McpManager>>,
}


enum PrepError {
    Cancelled,
    Failed(String),
}

impl McpManager {
    /// 创建管理器（返回 Arc 以装配快照回呼引用）。
    pub fn new() -> Arc<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("创建 MCP runtime");
        let manager = Arc::new(Self {
            runtime,
            inner: Mutex::new(Inner {
                loaded: false,
                load_error: None,
                servers: Vec::new(),
            }),
            operate: Mutex::new(()),
            self_weak: OnceLock::new(),
        });
        let _ = manager.self_weak.set(Arc::downgrade(&manager));
        manager
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 每个普通用户轮开始时取得快照。首次调用触发配置加载与连接建立；
    /// 之后的轮次复用健康连接、重列过期目录。取消时回收本轮候选连接并
    /// 返回 None：宿主结束本轮、不发送模型请求（已就绪连接保留）。
    pub fn prepare_for_turn(&self, cancel: &AtomicBool) -> Option<Arc<McpSnapshot>> {
        let operate = self.operate.lock().unwrap_or_else(|e| e.into_inner());
        if cancel.load(Ordering::SeqCst) {
            return None;
        }
        self.ensure_loaded();
        // 方案 A（022 §2.2-6）：通知计数的读取基准在重列出列前取得；同步
        // 等待是**条件性**的——仅当存在活动连接且其上次调用后可能有在途
        // 通知（notify_grace）时等待一次到达窗口（50ms，计入轮准备耗时）。
        // 无 MCP 配置／空闲连接／纯主动通知（到达即原子计数）均零等待。
        let needs_settle: bool = {
            let inner = self.lock();
            inner.servers.iter().any(|s| {
                s.connection
                    .as_ref()
                    .is_some_and(|c| c.state.notify_grace.load(Ordering::SeqCst))
            })
        };
        if needs_settle {
            self.runtime.block_on(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
            let inner = self.lock();
            for s in &inner.servers {
                if let Some(c) = &s.connection {
                    c.state.notify_grace.store(false, Ordering::SeqCst);
                }
            }
        }
        // 逐服务器准备（固定 ID 顺序；单项失败不影响其他服务器与原生工具）。
        let plan: Vec<(String, u8)> = {
            let inner = self.lock();
            inner
                .servers
                .iter()
                .map(|s| {
                    let first_connect =
                        s.config.enabled && s.connection.is_none() && s.error.is_none();
                    let needs_relist = s
                        .connection
                        .as_ref()
                        .zip(s.listing.as_ref())
                        .is_some_and(|(c, l)| {
                            c.state.stale_epoch.load(Ordering::SeqCst) != l.listed_epoch
                        });
                    (s.id.clone(), if first_connect { 1 } else if needs_relist { 2 } else { 0 })
                })
                .collect()
        };
        let mut new_connections: Vec<Arc<McpConnection>> = Vec::new();
        for (id, work) in plan {
            if cancel.load(Ordering::SeqCst) {
                drop(operate);
                for conn in new_connections {
                    self.runtime.block_on(conn.close());
                }
                return None;
            }
            if work == 0 {
                continue;
            }
            let config = {
                let inner = self.lock();
                match inner.servers.iter().find(|s| s.id == id) {
                    Some(server) => server.config.clone(),
                    None => continue,
                }
            };
            let outcome: Result<(Arc<McpConnection>, ServerListing, Vec<String>, Vec<String>), PrepError> = if work == 1 {
                self.connect_and_list(&config, cancel)
                    .map(|(c, l, u, e)| (Arc::new(c), l, u, e))
            } else {
                // 重列：不重建连接；失败则该服务器本轮不可用（reload 恢复）。
                let connection = {
                    let inner = self.lock();
                    inner
                        .servers
                        .iter()
                        .find(|s| s.id == id)
                        .and_then(|s| s.connection.clone())
                };
                match connection {
                    Some(conn) => self
                        .relist(&config, &conn, cancel)
                        .map(|(l, u, e)| (conn, l, u, e)),
                    None => Err(PrepError::Failed("连接不在发布状态".into())),
                }
            };
            match outcome {
                Ok((connection, listing, unknown, excluded)) => {
                    // 仅本轮**新建**的连接记为候选（取消时回收）；重列复用的
                    // 既有健康连接保留（022 §2.2）。
                    if work == 1 {
                        new_connections.push(connection.clone());
                    }
                    let mut inner = self.lock();
                    if let Some(server) = inner.servers.iter_mut().find(|s| s.id == id) {
                        server.connection = Some(connection);
                        server.listing = Some(Arc::new(listing));
                        server.error = None;
                        server.unknown_allowlisted = unknown;
                        server.excluded = excluded;
                    }
                }
                Err(PrepError::Cancelled) => {
                    drop(operate);
                    for conn in new_connections {
                        self.runtime.block_on(conn.close());
                    }
                    return None;
                }
                Err(PrepError::Failed(reason)) => {
                    let mut inner = self.lock();
                    if let Some(server) = inner.servers.iter_mut().find(|s| s.id == id) {
                        server.connection = None;
                        server.listing = None;
                        server.error = Some(reason);
                    }
                }
            }
        }
        drop(operate);
        Some(self.publish_snapshot())
    }

    /// `/mcp` 状态显示（022 §2.2：配置加载状态、启用／连接状态、可用模型名
    /// 与原始名、过滤数量、脱敏错误）。
    pub fn status_lines(&self) -> Vec<String> {
        let inner = self.lock();
        if let Some(error) = &inner.load_error {
            return vec![format!("MCP：未加载（{error}）")];
        }
        if !inner.loaded {
            return vec!["MCP：未加载（发送消息后按需启动）".to_string()];
        }
        if inner.servers.is_empty() {
            return vec!["MCP：未配置（mcp.json 缺失或无服务器条目）".to_string()];
        }
        let mut lines = vec![format!(
            "MCP：已加载 mcp.json（{} 台服务器，{} 台启用）",
            inner.servers.len(),
            inner.servers.iter().filter(|s| s.config.enabled).count()
        )];
        for server in &inner.servers {
            let transport_name = match &server.config.transport {
                McpTransport::Stdio(_) => "stdio",
                McpTransport::Http(_) => "http",
            };
            if !server.config.enabled {
                lines.push(format!("{0}（{transport_name}）：已停用", server.id));
                continue;
            }
            let connected = server
                .connection
                .as_ref()
                .is_some_and(|c| !c.is_transport_closed());
            match (connected, &server.listing) {
                (true, Some(listing)) => {
                    lines.push(format!(
                        "{0}（{transport_name}）：已连接｜工具 {1} 个（排除 {2} 项）",
                        server.id,
                        listing.tools.len(),
                        server.excluded.len()
                    ));
                    // 模型名按本服务器目录本地视角显示（全局冲突时实际发布名
                    // 会附加哈希，见 publish_snapshot）。
                    let identities: Vec<(String, String)> = listing
                        .tools
                        .iter()
                        .map(|(raw, _, _)| (server.id.clone(), raw.clone()))
                        .collect();
                    let local_names =
                        assign_model_names(&identities).unwrap_or_default();
                    for ((raw, _, _), name) in listing.tools.iter().zip(local_names.iter()) {
                        lines.push(format!("  {name} ← {raw}"));
                    }
                    for reason in &server.excluded {
                        lines.push(format!("  排除：{reason}"));
                    }
                    for name in &server.unknown_allowlisted {
                        lines.push(format!("  提示：允许名单中的未知工具 {name}"));
                    }
                    if let Some(conn) = &server.connection {
                        if let Some(tail) = conn.state.stderr_tail_snapshot() {
                            if let Some(safe) =
                                sanitize_diagnostic(&tail, &listing.sensitive)
                            {
                                for line in safe.lines().take(3) {
                                    lines.push(format!("  stderr：{line}"));
                                }
                            }
                        }
                    }
                }
                (_, _) => {
                    let reason = server.error.clone().unwrap_or_else(|| "不可用".into());
                    lines.push(format!("{0}（{transport_name}）：不可用：{reason}", server.id));
                }
            }
        }
        lines
    }

    /// `/mcp reload`（022 §2.2：只在空闲时执行；先完整解析新配置再替换）。
    /// 解析失败保留原配置并返回「未应用新配置」；运行中返回「忙」。
    pub fn reload(&self) -> Result<(), String> {
        let operate = match self.operate.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err("忙".to_string()),
            Err(TryLockError::Poisoned(_)) => return Err("内部状态损坏".to_string()),
        };
        let user_dir = crate::catalog::user_dir();
        let file = load_mcp_file(&user_dir).map_err(|e| format!("未应用新配置：{e}"))?;
        // 解析成功：关闭全部旧连接（有界），再按新配置重建启用条目；
        // 新连接失败不回用旧配置。
        let old: Vec<Arc<McpConnection>> = {
            let mut inner = self.lock();
            inner
                .servers
                .drain(..)
                .filter_map(|s| s.connection)
                .collect()
        };
        for conn in old {
            self.runtime.block_on(conn.close());
        }
        {
            let mut inner = self.lock();
            inner.load_error = None;
            inner.loaded = true;
            inner.servers = file
                .servers
                .into_iter()
                .map(|(id, config)| ServerState {
                    id,
                    config,
                    connection: None,
                    error: None,
                    listing: None,
                    unknown_allowlisted: Vec::new(),
                    excluded: Vec::new(),
                })
                .collect();
        }
        let never = AtomicBool::new(false);
        let enabled_ids: Vec<String> = {
            let inner = self.lock();
            inner
                .servers
                .iter()
                .filter(|s| s.config.enabled)
                .map(|s| s.id.clone())
                .collect()
        };
        for id in enabled_ids {
            let config = {
                let inner = self.lock();
                match inner.servers.iter().find(|s| s.id == id) {
                    Some(server) => server.config.clone(),
                    None => continue,
                }
            };
            match self.connect_and_list(&config, &never) {
                Ok((connection, listing, unknown, excluded)) => {
                    let connection = Arc::new(connection);
                    let mut inner = self.lock();
                    if let Some(server) = inner.servers.iter_mut().find(|s| s.id == id) {
                        server.connection = Some(connection);
                        server.listing = Some(Arc::new(listing));
                        server.unknown_allowlisted = unknown;
                        server.excluded = excluded;
                    }
                }
                Err(PrepError::Cancelled) => unreachable!("reload 不响应取消"),
                Err(PrepError::Failed(reason)) => {
                    let mut inner = self.lock();
                    if let Some(server) = inner.servers.iter_mut().find(|s| s.id == id) {
                        server.connection = None;
                        server.listing = None;
                        server.error = Some(reason);
                    }
                }
            }
        }
        drop(operate);
        Ok(())
    }

    /// 宿主退出：显式关闭全部 MCP 连接（有界；远程连接只关闭本地端）。
    /// 关闭后 prepare 不自动重连——已关闭连接只有显式 reload 才重新启动
    ///（022 §2.2；宿主退出路径 close 后不再有下一轮，此处为契约收口）。
    pub fn close(&self) {
        // 与 prepare/reload 互斥：避免关停与建立连接竞态导致漏关漏标
        //（正在建立、尚未回存的连接会被本轮关停跳过）。
        let _operate = self.operate.lock().unwrap_or_else(|e| e.into_inner());
        let old: Vec<Arc<McpConnection>> = {
            let mut inner = self.lock();
            inner
                .servers
                .iter_mut()
                .filter_map(|s| {
                    let conn = s.connection.take();
                    if conn.is_some() {
                        s.listing = None;
                        s.error = Some("已随宿主关闭（reload 可恢复）".to_string());
                    }
                    conn
                })
                .collect()
        };
        for conn in old {
            self.runtime.block_on(conn.close());
        }
    }

    /// 执行一次外部调用（同步桥接；022 §2.4）。返回 CC 工具结果文本与
    /// 「isError」。凭证污染检查在结果入史之前：污染结果整份替换为标准
    /// 客户端错误（已发调用状态未知），原始回包不入史、不展示、不写日志。
    pub fn call(
        &self,
        entry: &ToolEntry,
        arguments: Value,
        cancel: &AtomicBool,
    ) -> (String, bool) {
        let result = self.runtime.block_on(entry.connection.call_tool_once(
            &entry.raw_tool,
            arguments,
            entry.tool_timeout,
            cancel,
        ));
        let sensitive = &entry.sensitive;
        match result {
            Ok(rmcp::model::ServerResult::CallToolResult(result)) => {
                let projected = call::project_result(&result);
                if sensitive.contains_secret(&projected) {
                    (
                        call::client_error_json(
                            "敏感数据",
                            "调用结果包含凭证数据，已拒绝（已发调用的远端执行状态未知）",
                        ),
                        true,
                    )
                } else {
                    let is_error = serde_json::from_str::<Value>(&projected)
                        .ok()
                        .and_then(|v| v.get("isError").and_then(Value::as_bool))
                        .unwrap_or(false);
                    (projected, is_error)
                }
            }
            Ok(rmcp::model::ServerResult::InputRequiredResult(_)
            | rmcp::model::ServerResult::CreateTaskResult(_)) => (
                call::client_error_json(
                    "协议",
                    "服务器返回 input_required／Task 响应，本版本不支持",
                ),
                true,
            ),
            Ok(other) => (
                call::client_error_json(
                    "协议",
                    &sanitize_owned(format!("不支持的协议响应：{other:?}"), sensitive),
                ),
                true,
            ),
            Err(failure) => {
                // 断连／传输损坏：连接不再声明工具（下一轮不发布，reload 恢复）。
                let sdk_closed = self
                    .runtime
                    .block_on(entry.connection.state.transport_closed());
                if sdk_closed || entry.connection.is_transport_closed() {
                    entry
                        .connection
                        .state
                        .closed
                        .store(true, Ordering::SeqCst);
                    self.note_disconnected(entry);
                }
                let (kind, detail) = match &failure {
                    CallFailure::NotSent(d) => ("未发送", sanitize_owned(d.clone(), sensitive)),
                    CallFailure::SentUnknown(d) => {
                        ("状态未知", sanitize_owned(d.clone(), sensitive))
                    }
                    CallFailure::SentRejected(d) => {
                        ("协议", sanitize_owned(d.clone(), sensitive))
                    }
                    CallFailure::Unsupported(d) => {
                        ("协议", sanitize_owned(d.clone(), sensitive))
                    }
                };
                (call::client_error_json(kind, &detail), true)
            }
        }
    }

    /// 调用后发现连接断开：从发布状态移除（后续新轮不声明工具）。
    fn note_disconnected(&self, entry: &ToolEntry) {
        let mut inner = self.lock();
        if let Some(server) = inner.servers.iter_mut().find(|s| {
            s.connection
                .as_ref()
                .is_some_and(|c| Arc::ptr_eq(c, &entry.connection))
        }) {
            server.listing = None;
            server.error = Some("连接已断开（reload 可恢复）".to_string());
        }
    }

    fn ensure_loaded(&self) {
        let mut inner = self.lock();
        if inner.loaded {
            return;
        }
        let user_dir = crate::catalog::user_dir();
        match load_mcp_file(&user_dir) {
            Ok(file) => {
                inner.servers = file
                    .servers
                    .into_iter()
                    .map(|(id, config)| ServerState {
                        id,
                        config,
                        connection: None,
                        error: None,
                        listing: None,
                        unknown_allowlisted: Vec::new(),
                        excluded: Vec::new(),
                    })
                    .collect();
                inner.load_error = None;
            }
            Err(e) => {
                inner.load_error = Some(e.0);
                inner.servers = Vec::new();
            }
        }
        inner.loaded = true;
    }

    /// 连接＋完整发现＋目录发布（启动总时限覆盖初始化与首次发现）。
    fn connect_and_list(
        &self,
        config: &McpServerConfig,
        cancel: &AtomicBool,
    ) -> Result<(McpConnection, ServerListing, Vec<String>, Vec<String>), PrepError> {
        // 凭证污染检查先于发布：敏感值集合在准备时固定。
        let sensitive = Arc::new(build_sensitive(config));
        let outcome = self.runtime.block_on(async {
            tokio::select! {
                // 初始化与首次完整发现共用**单一**启动总时限（022 §2.1）。
                outcome = tokio::time::timeout(config.startup_timeout, async {
                    let connection = McpConnection::connect(config)
                        .await
                        .map_err(|e| PrepError::Failed(sanitize_owned(e.0, &sensitive)))?;
                    // 发布基准在列出前记录：列出期间／之后到达的目录变化
                    // 计数大于基准 → 下一轮继续重列（不吞通知）。
                    let listed_epoch = connection.state.stale_epoch.load(Ordering::SeqCst);
                    let tools = connection
                        .list_all_tools(config.startup_timeout)
                        .await
                        .map_err(|e| PrepError::Failed(sanitize_owned(e.0, &sensitive)))?;
                    Ok((connection, tools, listed_epoch))
                }) => match outcome {
                    Ok(inner) => inner,
                    Err(_) => Err(PrepError::Failed(format!(
                        "启动超过总时限 {:?}（初始化＋首次发现共用）",
                        config.startup_timeout
                    ))),
                },
                _ = wait_cancel_flag(cancel) => Err(PrepError::Cancelled),
            }
        });
        let (connection, tools, listed_epoch) = outcome?;
        let (listing, unknown, excluded) =
            publish_tools(config, tools, &sensitive, listed_epoch).map_err(PrepError::Failed)?;
        Ok((connection, listing, unknown, excluded))
    }

    /// 重列（目录过期后）：完整重列该服务器；重列期间的新通知保留过期标记。
    fn relist(
        &self,
        config: &McpServerConfig,
        connection: &Arc<McpConnection>,
        cancel: &AtomicBool,
    ) -> Result<(ServerListing, Vec<String>, Vec<String>), PrepError> {
        let sensitive = Arc::new(build_sensitive(config));
        // 发布基准在列出前记录：重列期间／之后到达的目录变化保留给下一轮
        //（022 §2.2「重列期间收到的新通知必须保留过期标记」）。
        let listed_epoch = connection.state.stale_epoch.load(Ordering::SeqCst);
        let tools = self
            .runtime
            .block_on(async {
                tokio::select! {
                    tools = connection.list_all_tools(config.startup_timeout) => tools
                        .map_err(|e| PrepError::Failed(sanitize_owned(e.0, &sensitive))),
                    _ = wait_cancel_flag(cancel) => Err(PrepError::Cancelled),
                }
            })?;
        publish_tools(config, tools, &sensitive, listed_epoch)
            .map(|(l, u, e)| (l, u, e))
            .map_err(PrepError::Failed)
    }

    fn publish_snapshot(&self) -> Arc<McpSnapshot> {
        let manager: Option<Weak<McpManager>> = self.self_weak.get().cloned();
        let inner = self.lock();
        // 聚合全部可用服务器目录，按原始身份稳定排序后**全局**分配模型名：
        // 跨服务器同名冲突与单服务器内一样经稳定短哈希消解（022 §2.3），
        // 残余冲突显式拒绝（本轮该工具缺失，不覆盖注册表）。
        let mut flattened: Vec<(
            String,
            String,
            Option<String>,
            Value,
            Arc<SensitiveValues>,
            Duration,
            Arc<McpConnection>,
        )> = Vec::new();
        for server in &inner.servers {
            let Some(listing) = &server.listing else {
                continue;
            };
            let Some(connection) = &server.connection else {
                continue;
            };
            if connection.is_transport_closed() {
                continue;
            }
            for (raw_tool, description, schema) in &listing.tools {
                flattened.push((
                    server.id.clone(),
                    raw_tool.clone(),
                    description.clone(),
                    schema.clone(),
                    listing.sensitive.clone(),
                    server.config.tool_timeout,
                    connection.clone(),
                ));
            }
        }
        flattened.sort_by(|a, b| {
            (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str()))
        });
        let identities: Vec<(String, String)> = flattened
            .iter()
            .map(|(s, t, _, _, _, _, _)| (s.clone(), t.clone()))
            .collect();
        let names = match assign_model_names(&identities) {
            Ok(names) => names,
            Err(_e) => {
                // 命名整体失败（目录级错误）：本轮零 MCP 工具（不覆盖注册表）。
                return Arc::new(McpSnapshot {
                    entries: Vec::new(),
                    definitions: Vec::new(),
                    manager,
                });
            }
        };
        let entries: Vec<ToolEntry> = flattened
            .iter()
            .zip(names.iter())
            .map(
                |(
                    (server_id, raw_tool, description, schema, sensitive, tool_timeout, connection),
                    name,
                )| {
                    ToolEntry {
                        model_name: name.clone(),
                        server_id: server_id.clone(),
                        raw_tool: raw_tool.clone(),
                        description: description.clone(),
                        schema: schema.clone(),
                        connection: connection.clone(),
                        sensitive: sensitive.clone(),
                        tool_timeout: *tool_timeout,
                    }
                },
            )
            .collect();
        let definitions = entries
            .iter()
            .map(|e| {
                build_function_definition(&e.model_name, e.description.as_deref(), &e.schema)
            })
            .collect();
        Arc::new(McpSnapshot {
            entries,
            definitions,
            manager,
        })
    }
}

// ---- 目录发布（连接与重列共用） ----

fn publish_tools(
    config: &McpServerConfig,
    tools: Vec<rmcp::model::Tool>,
    sensitive: &Arc<SensitiveValues>,
    listed_epoch: u64,
) -> Result<(ServerListing, Vec<String>, Vec<String>), String> {
    let (enabled_set, disabled_set) = tool_sets(config);
    let catalog = build_server_catalog(&tools, enabled_set.as_ref(), &disabled_set, sensitive)?;
    let mut tools_out: Vec<(String, Option<String>, Value)> = catalog
        .kept
        .iter()
        .map(|(raw, tool, schema)| {
            (raw.clone(), tool.description.as_deref().map(String::from), schema.clone())
        })
        .collect();
    tools_out.sort_by(|a, b| a.0.as_str().cmp(&b.0.as_str()));
    let excluded: Vec<String> = catalog
        .excluded
        .iter()
        .map(|reason| sanitize_owned(reason.clone(), sensitive))
        .collect();
    Ok((
        ServerListing {
            sensitive: sensitive.clone(),
            tools: tools_out,
            listed_epoch,
        },
        catalog.unknown_allowlisted,
        excluded,
    ))
}

fn tool_sets(config: &McpServerConfig) -> (Option<HashSet<String>>, HashSet<String>) {
    let enabled = config
        .enabled_tools
        .as_ref()
        .map(|v| v.iter().cloned().collect::<HashSet<_>>());
    let disabled = config.disabled_tools.iter().cloned().collect::<HashSet<_>>();
    (enabled, disabled)
}

fn build_sensitive(config: &McpServerConfig) -> SensitiveValues {
    match &config.transport {
        McpTransport::Stdio(stdio) => collect_sensitive_values(&stdio.env_vars, None),
        McpTransport::Http(http) => {
            let auth = super::secrets::resolve_http_auth_refs(http);
            collect_sensitive_values(&[], Some(&auth))
        }
    }
}

/// 诊断／错误文本脱敏：包含敏感值时替换为通用诊断（不回显匹配值、
/// 原始片段或指纹）。
fn sanitize_owned(text: String, sensitive: &SensitiveValues) -> String {
    if sensitive.contains_secret(&text) {
        "（细节包含敏感数据，已省略）".to_string()
    } else {
        text
    }
}

/// stderr 尾部展示脱敏：污染时通用诊断；否则去除控制字符并截断。
fn sanitize_diagnostic(text: &str, sensitive: &SensitiveValues) -> Option<String> {
    if sensitive.contains_secret(text) {
        return Some("（诊断包含敏感数据，已省略）".to_string());
    }
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(512)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

use super::naming::NameAssignError;

impl From<NameAssignError> for String {
    fn from(e: NameAssignError) -> String {
        e.to_string()
    }
}
