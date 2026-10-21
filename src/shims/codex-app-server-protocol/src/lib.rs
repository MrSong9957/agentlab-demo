//! codex-app-server-protocol shim（008 §3-J5）：保留文件消费的母本协议类型最小子集。
//! 逐类型取自 openai/codex@97771b1 codex-rs/app-server-protocol 对应定义；
//! 当前为桩（编译驱动补全字段/变体）。本地无事件源的变体不会被构造。

macro_rules! stub_enum { ($name:ident) => { #[derive(Debug, Clone, PartialEq, Eq)] pub enum $name {} } }
macro_rules! stub_struct { ($name:ident) => { #[derive(Debug, Clone, PartialEq, Eq)] pub struct $name {} } }

/// 母本 v2/shared.rs:206 AskForApproval——原样变体集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskForApproval {
    UnlessTrusted,
    OnRequest,
    Granular {
        sandbox_approval: bool,
        rules: bool,
        skill_approval: bool,
        request_permissions: bool,
        mcp_elicitations: bool,
    },
    Never,
}

impl From<&codex_protocol::config_types::AskForApproval> for AskForApproval {
    fn from(value: &codex_protocol::config_types::AskForApproval) -> Self {
        match value {
            codex_protocol::config_types::AskForApproval::UnlessTrusted => Self::UnlessTrusted,
            codex_protocol::config_types::AskForApproval::OnRequest => Self::OnRequest,
            codex_protocol::config_types::AskForApproval::Never => Self::Never,
        }
    }
}

impl From<codex_protocol::config_types::AskForApproval> for AskForApproval {
    fn from(value: codex_protocol::config_types::AskForApproval) -> Self {
        match value {
            codex_protocol::config_types::AskForApproval::UnlessTrusted => Self::UnlessTrusted,
            codex_protocol::config_types::AskForApproval::OnRequest => Self::OnRequest,
            codex_protocol::config_types::AskForApproval::Never => Self::Never,
        }
    }
}
stub_enum!(CollabAgentTool);
stub_enum!(CollabAgentToolCallStatus);
stub_enum!(DynamicToolCallOutputContentItem);
stub_enum!(DynamicToolCallStatus);
stub_enum!(McpAuthStatus);
/// 母本 McpServerStartupFailureReason——消费变体补全。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerStartupFailureReason {
    ReauthenticationRequired,
}
stub_enum!(McpServerStatusDetail);
stub_enum!(McpServerStatus);
stub_enum!(McpToolCallStatus);
stub_struct!(SkillMetadata);
stub_struct!(ToolRequestUserInputAnswer);
stub_struct!(ToolRequestUserInputParams);
stub_struct!(ToolRequestUserInputQuestion);
stub_enum!(WebSearchAction);

// ===== ThreadItem 本地真实映射（008 §3-J5）=====
// 变体形状取自母本 app-server-protocol/src/protocol/v2/item.rs（固定提交）。
// 仅定义本地投影（/resume）与保留渲染代码消费的变体；缺席功能变体
//（Reasoning/McpToolCall/WebSearch/Plan/Image*/Hook/Review/Compaction/Sleep/
//  Collab/SubAgent/DynamicTool/FunctionCallOutput）不定义，对应投影分支按
// 008 §1 裁剪并登记。CommandExecution 的 sandbox_type/model_context 为母本
// serde(skip) 字段，本地映射省略（登记）；AgentMessage 的 phase/memory/
// delivery/questions 字段无本地来源，省略（登记）。

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UserInput {
    Text {
        text: String,
        #[serde(default)]
        text_elements: Vec<TextElementV2>,
    },
    // 008-TRIM: Image/LocalImage 变体（无本地事件源）
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextElementV2 {
    /// Byte range in the parent `text` buffer that this element occupies.
    pub byte_range: codex_protocol::user_input::ByteRange,
    /// Optional human-readable placeholder for the element, displayed in the UI.
    pub placeholder: Option<String>,
}

impl TextElementV2 {
    pub fn new(byte_range: codex_protocol::user_input::ByteRange, placeholder: Option<String>) -> Self {
        Self { byte_range, placeholder }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CommandAction {
    Read { command: String, name: String, path: String },
    ListFiles { command: String, path: Option<String> },
    Search { command: String, query: Option<String>, path: Option<String> },
    Unknown { command: String },
}

impl CommandAction {
    /// 母本 into_core 语义：映射到 core ParsedCommand。
    pub fn into_core(self) -> codex_protocol::parse_command::ParsedCommand {
        self.into()
    }
}

impl From<CommandAction> for codex_protocol::parse_command::ParsedCommand {
    fn from(value: CommandAction) -> Self {
        match value {
            CommandAction::Read { command, name, path } => Self::Read {
                cmd: command,
                name,
                path: std::path::PathBuf::from(path),
            },
            CommandAction::ListFiles { command, path } => Self::ListFiles { cmd: command, path },
            CommandAction::Search { command, query, path } => Self::Search { cmd: command, query, path },
            CommandAction::Unknown { command } => Self::Unknown { cmd: command },
        }
    }
}

/// 母本 item.rs:1083 CommandExecutionStatus——原样变体集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandExecutionStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
}

/// 母本 item.rs:1119 CommandExecutionSource——原样变体集（默认 Agent）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandExecutionSource {
    #[default]
    Agent,
    UserShell,
    UnifiedExecStartup,
    UnifiedExecInteraction,
}

/// 母本 item.rs:1146 FileUpdateChange——原样字段。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUpdateChange {
    pub path: String,
    pub kind: PatchChangeKind,
    pub diff: String,
}

/// 母本 item.rs:1156 PatchChangeKind——原样变体集。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatchChangeKind {
    Add,
    Delete,
    Update { move_path: Option<std::path::PathBuf> },
}

/// 母本 item.rs:1165 PatchApplyStatus——原样变体集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatchApplyStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadItem {
    UserMessage {
        id: String,
        client_id: Option<String>,
        content: Vec<UserInput>,
    },
    AgentMessage {
        id: String,
        text: String,
    },
    CommandExecution {
        id: String,
        #[serde(default)]
        plugin_id: Option<String>,
        #[serde(default)]
        script_path: Option<String>,
        command: String,
        cwd: String,
        process_id: Option<String>,
        #[serde(default)]
        source: CommandExecutionSource,
        status: CommandExecutionStatus,
        command_actions: Vec<CommandAction>,
        aggregated_output: Option<String>,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
    },
    FileChange {
        id: String,
        changes: Vec<FileUpdateChange>,
        status: PatchApplyStatus,
    },
}

// ===== Thread/Turn 本地真实映射（母本 protocol/v2/thread_data.rs 字段子集）=====
// 省略字段登记：Thread 的 environments/extra/section/fork 等无本地来源；
// TurnError 的 codex_error_info/misalignment 依赖深类型，本地仅保留 message/additional_details。

impl ThreadItem {
    /// 母本 ThreadItem::id()——条目 id 访问器。
    pub fn id(&self) -> &str {
        match self {
            Self::UserMessage { id, .. }
            | Self::AgentMessage { id, .. }
            | Self::CommandExecution { id, .. }
            | Self::FileChange { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub cwd: codex_utils_absolute_path::AbsolutePathBuf,
    pub turns: Vec<Turn>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: String,
    pub root_turn_id: Option<String>,
    pub items: Vec<ThreadItem>,
    #[serde(default)]
    pub items_view: TurnItemsView,
    pub status: TurnStatus,
    pub error: Option<TurnError>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnItemsView {
    NotLoaded,
    Summary,
    #[default]
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    InProgress,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    pub message: String,
    #[serde(default)]
    pub additional_details: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String); // 母本为 Uuid newtype；本地以 String 承载

pub use codex_protocol::user_input::ByteRange; // 母本经协议 re-export
stub_enum!(SortDirection);
stub_struct!(ThreadListParams);
stub_enum!(ClientRequest);
stub_enum!(ThreadGoalStatus);
stub_struct!(ThreadItemsListResponse);
