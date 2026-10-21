//! codex-config shim（008 §3-J5）：保留文件消费的配置类型最小子集。
//! tui_keymap.rs / key_aliases.rs 为原样拷贝（仅剥 schemars）。
//! Tui/TuiEffects/TuiNotificationSettings/RightClickPaste/CopyOnSelect/NotificationCondition/
//! TuiRendering 取母本 config@97771b1 真实定义（去 schemars/TS）；其余桩按需补全。
pub mod tui_keymap;
pub mod key_aliases;

pub mod types {
    pub use crate::Tui;
    /// 母本 config/src/tui_rendering.rs TuiRendering——原样语义（默认全开）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TuiRendering {
        pub mermaid: bool,
        pub math: bool,
        pub tables: bool,
        pub lists: bool,
    }
    impl Default for TuiRendering {
        fn default() -> Self {
            Self { mermaid: true, math: true, tables: true, lists: true }
        }
    }

    /// 母本 config/src/tui_effects.rs TuiEffects——原样语义（默认全开）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TuiEffects {
        pub starfield: bool,
        pub shimmer: bool,
        pub welcome: bool,
        pub effort: bool,
        pub progress: bool,
        pub title: bool,
    }
    impl Default for TuiEffects {
        fn default() -> Self {
            Self { starfield: true, shimmer: true, welcome: true, effort: true, progress: true, title: true }
        }
    }

    /// 母本 config/src/types.rs NotificationCondition。
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum NotificationCondition {
        #[default]
        Unfocused,
        Always,
    }

    /// 母本 config/src/types.rs:723 NotificationMethod——原样变体集与 Display。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum NotificationMethod {
        #[default]
        Auto,
        Osc9,
        Bel,
    }

    impl std::fmt::Display for NotificationMethod {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                NotificationMethod::Auto => write!(f, "auto"),
                NotificationMethod::Osc9 => write!(f, "osc9"),
                NotificationMethod::Bel => write!(f, "bel"),
            }
        }
    }

    /// 母本 config/src/types.rs RightClickPaste。
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum RightClickPaste {
        #[default]
        Auto,
        On,
        Off,
    }

    /// 母本 config/src/types.rs CopyOnSelect。
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum CopyOnSelect {
        #[default]
        Auto,
        Always,
        Never,
    }

    /// 母本 config/src/types.rs TuiNotificationSettings。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub struct TuiNotificationSettings {
        pub notifications: Notifications,
        pub method: NotificationMethod,
        pub condition: NotificationCondition,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)] // 桩：字段按需补全
    pub struct Notifications {}

    pub use codex_protocol::config_types::AltScreenMode; // 母本经 protocol re-export

    // 母本 types.rs:37-41 对 tui_keymap 的 re-export。
    pub use crate::tui_keymap::KeybindingSpec;
    pub use crate::tui_keymap::KeybindingsSpec;
    pub use crate::tui_keymap::MAX_FUNCTION_KEY;
    pub use crate::tui_keymap::TuiKeymap;

    #[derive(Debug, Clone, PartialEq, Eq)] // 桩
    pub enum ApprovalsReviewer {}

    /// 母本 History 设置——桩（字段按需补全）。
    #[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub struct History {} // 桩
    /// 母本 Notice 设置——桩（字段按需补全）。
    #[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub struct Notice {} // 桩
    /// 母本 pet 锚点（types.rs:765 附近 ScreenBottom 等）——桩。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum PetAnchor {
        #[default]
        Unspecified,
    } // 桩（本地无该设置面；变体按需对齐母本）
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum SessionPickerView {
        #[default]
        Unspecified,
    } // 桩（本地无该设置面；变体按需对齐母本）
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum AgentsOverviewGrouping {
        #[default]
        Unspecified,
    } // 桩（本地无该设置面；变体按需对齐母本）
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    pub enum ResumeCwd {
        #[default]
        Unspecified,
    } // 桩（本地无该设置面；变体按需对齐母本）
    /// 母本 ModelAvailabilityNuxConfig（shown_count map）。
    #[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    pub struct ModelAvailabilityNuxConfig {
        pub shown_count: std::collections::HashMap<String, u32>,
    }
    /// 母本 resize-reflow 上限。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TerminalResizeReflowMaxRows {
        Auto,
        Disabled,
        Limit(usize),
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TerminalResizeReflowConfig {
        pub max_rows: TerminalResizeReflowMaxRows,
    }
    impl Default for TerminalResizeReflowConfig {
        fn default() -> Self {
            Self { max_rows: TerminalResizeReflowMaxRows::Auto }
        }
    }
}

pub mod config_toml {
    use super::types;
    /// 母本 config_toml::RealtimeAudioToml——桩。
    #[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
    pub struct RealtimeAudioToml {} // 桩

    /// 母本 ConfigToml 中 from_bootstrap 消费字段子集。
    #[derive(Debug, Clone, Default, serde::Deserialize)]
    pub struct ConfigToml {
        #[serde(default)]
        pub tui: Option<super::Tui>,
        #[serde(default)]
        pub disable_paste_burst: Option<bool>,
        #[serde(default)]
        pub history: Option<types::History>,
        #[serde(default)]
        pub notice: Option<types::Notice>,
    }

    /// 母本 core config::ConfigTomlLoadResult。
    #[derive(Debug, Clone)]
    pub struct ConfigTomlLoadResult {
        pub config_toml: ConfigToml,
        pub config_layer_stack: super::ConfigLayerStack,
    }
}

/// 母本 ConfigLayerStack：本地以单层（内置默认）实现其消费接口。
#[derive(Debug, Clone, Default)]
pub struct ConfigLayerStack {
    builtin: std::collections::BTreeMap<String, toml::Value>,
}

/// 母本 ConfigLayerSource。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLayerSource {
    BuiltIn,
    Project { path: std::path::PathBuf },
    User,
}

/// 母本 ConfigLayer。
#[derive(Debug, Clone)]
pub struct ConfigLayer {
    pub name: ConfigLayerSource,
    pub config: toml::Value,
}

impl ConfigLayerStack {
    pub fn effective_config(&self) -> &toml::Value {
        static EMPTY: std::sync::OnceLock<toml::Value> = std::sync::OnceLock::new();
        EMPTY.get_or_init(|| toml::Value::Table(Default::default()))
    }
    pub fn effective_user_config(&self) -> Option<&toml::Value> {
        None // 本地无用户 TOML 层
    }
    pub fn get_user_config_file(&self) -> Option<&codex_utils_absolute_path::AbsolutePathBuf> {
        None
    }
    pub fn layers_low_to_high(&self) -> &[ConfigLayer] {
        &[]
    }
}

/// 母本 merge_toml_values：递归合并 TOML 表（本地实现语义对齐：高层数值覆盖低层）。
pub fn merge_toml_values(target: &mut toml::Value, source: &toml::Value) {
    match (target, source) {
        (toml::Value::Table(t), toml::Value::Table(s)) => {
            for (k, v) in s {
                match t.get_mut(k) {
                    Some(tv @ toml::Value::Table(_)) if v.is_table() => merge_toml_values(tv, v),
                    _ => {
                        t.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (t, s) => *t = s.clone(),
    }
}

/// 母本 config/src/types.rs Tui——真实字段集（serde/schemars 剥离；f64 字段故无 Eq，与母本 derive 集一致）。
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tui {
    pub notification_settings: types::TuiNotificationSettings,
    pub animations: bool,
    pub screen_reader_detection_done: Option<bool>,
    pub effects: types::TuiEffects,
    pub rendering: types::TuiRendering,
    pub show_tooltips: bool,
    pub show_server_version_notice: bool,
    pub auto_recap: bool,
    pub disable_paste_burst: Option<bool>,
    pub vim_mode_default: bool,
    pub question_esc_back: bool,
    pub raw_output_mode: bool,
    pub fullscreen_transcript: bool,
    pub mouse_scroll_speed: Option<f64>,
    pub copy_on_select: types::CopyOnSelect,
    pub copy_on_select_clear_selection: bool,
    pub right_click_paste: types::RightClickPaste,
    pub alternate_screen: types::AltScreenMode,
    pub status_line: Option<Vec<String>>,
    pub status_line_use_colors: bool,
    pub terminal_title: Option<Vec<String>>,
    pub theme: Option<String>,
    pub pet: Option<String>,
    pub pet_anchor: types::PetAnchor,
    pub session_picker_view: Option<types::SessionPickerView>,
    pub agents_overview_grouping: types::AgentsOverviewGrouping,
    pub resume_cwd: types::ResumeCwd,
    pub keymap: types::TuiKeymap,
    pub model_availability_nux: types::ModelAvailabilityNuxConfig,
    pub terminal_resize_reflow_max_rows: Option<usize>,
}
// 注意：母本 Tui::default 多数字段经 serde default = true；本地 Default::default()
// 全 false/None——local_settings 装载处以显式默认值对齐母本语义（登记于装载适配）。

/// 母本 codex_config::Config——桩（编译驱动补全字段）。
#[derive(Debug, Clone, Default)]
pub struct Config {} // 桩
