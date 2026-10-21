//! codex-app-server-client shim（008 §3-J5）：仅承载 `legacy_core` 路径。
//! 母本 tui lib.rs:103 `pub(crate) use codex_app_server_client::legacy_core;`
//! ——保留文件经 `crate::legacy_core::config::…` 消费 Config 系类型。
//! 字段集＝保留文件（local_settings/history_cell 等）消费的最小子集；
//! Default 取母本 serde default 语义（doc 注释声明的默认值），显式实现并登记。
#[derive(Debug, Clone)]
pub struct AppServerRequestHandle {} // 桩（task_mentions 惰性 vendor 所需，本地不发起请求）

/// 母本 protocol.rs:1237 WritableRoot——原样字段（本地真值：无只读子路径）。
#[derive(Debug, Clone)]
pub struct WritableRoot {
    pub root: std::path::PathBuf,
    pub read_only_subpaths: Vec<std::path::PathBuf>,
    pub protected_metadata_names: Vec<String>,
}

impl WritableRoot {
    /// 母本 is_path_writable：路径在根内且不在只读子路径下（本地无只读子路径恒真）。
    pub fn is_path_writable(&self, path: &std::path::Path) -> bool {
        path.starts_with(&self.root)
    }
}

/// 本地最小文件系统沙箱策略（无沙箱：一切允许）。
#[derive(Debug, Clone, Default)]
pub struct FileSystemSandboxPolicy {
    pub allows_all: bool,
}

impl FileSystemSandboxPolicy {
    pub fn allows_read(&self, _path: &std::path::Path) -> bool {
        true
    }
    pub fn allows_write(&self, _path: &std::path::Path) -> bool {
        true
    }
    /// 本地真值：无外层沙箱＝全盘可写。
    pub fn has_full_disk_write_access(&self) -> bool {
        true
    }
    /// 本地真值：可写根＝传入 cwd 自身（无只读子路径、无保护元数据名）。
    pub fn get_writable_roots_with_cwd(&self, cwd: &std::path::Path) -> Vec<WritableRoot> {
        vec![WritableRoot {
            root: cwd.to_path_buf(),
            read_only_subpaths: Vec::new(),
            protected_metadata_names: Vec::new(),
        }]
    }
    /// 本地真值：无沙箱下任意本地路径可写。
    pub fn can_write_local_path_with_cwd(&self, _path: &std::path::Path, _cwd: &std::path::Path) -> bool {
        true
    }
}

pub mod legacy_core {
    pub mod config {
        use codex_config::ConfigLayerStack;
        use codex_config::config_toml::ConfigToml;
        pub use codex_config::config_toml::ConfigTomlLoadResult;
        pub use codex_config::types::TerminalResizeReflowConfig;
        pub use codex_config::types::TerminalResizeReflowMaxRows;

        /// 母本 Constrained<T> 的最小映射（value() 语义一致）。
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Constrained<T> {
            inner: T,
        }

        impl<T> Constrained<T> {
            pub fn new(value: T) -> Self {
                Self { inner: value }
            }
            pub fn value(&self) -> &T {
                &self.inner
            }
        }

        /// 母本 core Permissions 的本地真实映射：
        /// 003 本地工具直接执行（无审批、无外层沙箱）——approval=Never、profile=Disabled。
        #[derive(Debug, Clone, PartialEq)]
        pub struct Permissions {
            pub approval_policy: Constrained<codex_protocol::config_types::AskForApproval>,
        }

        impl Permissions {
            pub fn effective_permission_profile(&self) -> codex_protocol::models::PermissionProfile {
                codex_protocol::models::PermissionProfile::Disabled
            }

            pub fn active_permission_profile(
                &self,
            ) -> Option<codex_protocol::models::ActivePermissionProfile> {
                None
            }

            /// 本地映射：无沙箱（Disabled），文件系统策略为空限制恒真判定。
            pub fn file_system_sandbox_policy(&self) -> crate::FileSystemSandboxPolicy {
                crate::FileSystemSandboxPolicy::default()
            }
        }

        /// 母本 codex-core config::Config 的消费子集（字段名与类型对齐）。
        #[derive(Debug, Clone)]
        pub struct Config {
            pub tui_notifications: codex_config::types::TuiNotificationSettings,
            pub animations: bool,
            pub show_tooltips: bool,
            pub tui_show_server_version_notice: bool,
            pub tui_auto_recap: bool,
            pub disable_paste_burst: bool,
            pub tui_effects: codex_config::types::TuiEffects,
            pub tui_rendering: codex_config::types::TuiRendering,
            pub tui_vim_mode_default: bool,
            pub tui_question_esc_back: bool,
            pub tui_raw_output_mode: bool,
            pub tui_fullscreen_transcript: bool,
            pub tui_mouse_scroll_speed: Option<f64>,
            pub tui_copy_on_select: codex_config::types::CopyOnSelect,
            pub tui_copy_on_select_clear_selection: bool,
            pub tui_right_click_paste: codex_config::types::RightClickPaste,
            pub tui_alternate_screen: codex_config::types::AltScreenMode,
            pub tui_status_line: Option<Vec<String>>,
            pub tui_status_line_use_colors: bool,
            pub tui_terminal_title: Option<Vec<String>>,
            pub tui_theme: Option<String>,
            pub tui_pet: Option<String>,
            pub tui_pet_anchor: codex_config::types::PetAnchor,
            pub tui_session_picker_view: codex_config::types::SessionPickerView,
            pub tui_agents_overview_grouping: codex_config::types::AgentsOverviewGrouping,
            pub tui_resume_cwd: codex_config::types::ResumeCwd,
            pub tui_keymap: codex_config::types::TuiKeymap,
            pub model_availability_nux: codex_config::types::ModelAvailabilityNuxConfig,
            pub terminal_resize_reflow: TerminalResizeReflowConfig,
            pub history: codex_config::types::History,
            pub notices: codex_config::types::Notice,
            pub config_layer_stack: ConfigLayerStack,
            pub cwd: codex_utils_absolute_path::AbsolutePathBuf,
            pub permissions: Permissions,
            // 008 §3-J5 字段补全（2026-10-14 登记，status/ 保留路径消费）：
            // 母本 core/src/config/mod.rs:651 `pub model_context_window: Option<i64>`
            // （状态卡上下文窗口回退：token_info 为 None 时取配置窗口）。
            pub model_context_window: Option<i64>,
            pub codex_home: codex_utils_absolute_path::AbsolutePathBuf,
            // 其余字段按编译需求补全（登记）。
        }

        impl Default for Config {
            fn default() -> Self {
                Self {
                    tui_notifications: Default::default(),
                    animations: true,
                    show_tooltips: true,
                    tui_show_server_version_notice: true,
                    tui_auto_recap: true,
                    disable_paste_burst: false,
                    tui_effects: Default::default(),
                    tui_rendering: Default::default(),
                    tui_vim_mode_default: false,
                    tui_question_esc_back: true,
                    tui_raw_output_mode: false,
                    tui_fullscreen_transcript: true,
                    tui_mouse_scroll_speed: None,
                    tui_copy_on_select: Default::default(),
                    tui_copy_on_select_clear_selection: false,
                    tui_right_click_paste: Default::default(),
                    tui_alternate_screen: Default::default(),
                    tui_status_line: None,
                    tui_status_line_use_colors: false,
                    tui_terminal_title: None,
                    tui_theme: None,
                    tui_pet: None,
                    tui_pet_anchor: Default::default(),
                    tui_session_picker_view: Default::default(),
                    tui_agents_overview_grouping: Default::default(),
                    tui_resume_cwd: Default::default(),
                    tui_keymap: Default::default(),
                    model_availability_nux: Default::default(),
                    terminal_resize_reflow: Default::default(),
                    history: Default::default(),
                    notices: Default::default(),
                    config_layer_stack: Default::default(),
                    cwd: codex_utils_absolute_path::AbsolutePathBuf::current_dir()
                        .unwrap_or_else(|_| codex_utils_absolute_path::AbsolutePathBuf::new(std::path::PathBuf::from("/"))),
                    permissions: Permissions {
                        approval_policy: Constrained::new(
                            codex_protocol::config_types::AskForApproval::Never,
                        ),
                    },
                    model_context_window: None,
                    codex_home: codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(
                        std::env::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp")),
                    )
                    .unwrap_or_else(|_| codex_utils_absolute_path::AbsolutePathBuf::new(std::path::PathBuf::from("/tmp"))),
                }
            }
        }
    }
}
