//! codex-protocol shim（008 §3-J5）：仅含 tchat-tui 保留文件消费的类型最小子集。
//! 逐类型来源：openai/codex@97771b1 codex-rs/protocol 对应定义（字段/变体按需最小化，编译驱动补全）。
/// 母本 config_types ApprovalsReviewer——桩（变体按需补全）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ApprovalsReviewer {}

    pub mod num_format {
    /// 008 本地映射（登记）：母本为 icu_decimal＋sys_locale 的 locale 感知分组；
    /// 本地实现固定 en-US 逗号分组（输出确定，利于快照稳定），负数带符号。
    pub fn format_with_separators(n: i64) -> String {
        let s = n.to_string();
        let neg = s.starts_with('-');
        let digits = if neg { &s[1..] } else { &s[..] };
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        if neg { format!("-{out}") } else { out }
    }
}

pub mod thread_id {
    /// 母本为 Uuid newtype；本地以 String 承载（会话 ID 边界值由驱动生成）。
    #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct ThreadId(pub String);
    impl ThreadId {
        pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
        pub fn from_string(s: &str) -> Result<Self, String> {
            if s.is_empty() { Err("empty thread id".into()) } else { Ok(Self(s.to_string())) }
        }
    }
    impl std::fmt::Display for ThreadId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
    }
}
pub use thread_id::ThreadId;

pub mod config_types {
    /// 母本 protocol/src/config_types.rs AltScreenMode（默认 Auto）。
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum AltScreenMode {
        #[default]
        Auto,
        Always,
        Never,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] // 桩：变体按需补全
    pub enum CollaborationModeMask {}
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] // 桩：变体按需补全
    pub enum CollaborationMode {}

    /// 母本 protocol.rs:1095 AskForApproval——原样变体集。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum AskForApproval {
        UnlessTrusted,
        #[default]
        OnRequest,
        Never,
    }
}

pub mod openai_models {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] // 桩：变体按需补全
    pub enum ModelAccessPrograms {}

    /// 母本 openai_models.rs:62 ReasoningEffort——原样变体集与 as_str。
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub enum ReasoningEffort {
        None,
        Minimal,
        Low,
        #[default]
        Medium,
        High,
        XHigh,
        Max,
        Ultra,
        Persistent,
        Custom(String),
    }

    impl ReasoningEffort {
        pub fn as_str(&self) -> &str {
            match self {
                Self::None => "none",
                Self::Minimal => "minimal",
                Self::Low => "low",
                Self::Medium => "medium",
                Self::High => "high",
                Self::XHigh => "xhigh",
                Self::Max => "max",
                Self::Ultra => "ultra",
                Self::Persistent => "persistent",
                Self::Custom(_) => "custom",
            }
        }
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] // 桩
    pub enum ModelPreset {}
}

pub mod models {
    /// 母本 models.rs ActivePermissionProfile——变体按消费补全。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ActivePermissionProfile {}

    /// 母本 models.rs:425 PermissionProfile——原样变体集。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PermissionProfile {
        Managed {
            file_system: ManagedFileSystemPermissions,
            network: crate::permissions::NetworkSandboxPolicy,
        },
        Disabled,
        External {
            network: crate::permissions::NetworkSandboxPolicy,
        },
    }

    /// 母本 models.rs:303 ManagedFileSystemPermissions——原样变体集。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ManagedFileSystemPermissions {
        Restricted {
            entries: Vec<FileSystemSandboxEntry>,
            glob_scan_max_depth: Option<std::num::NonZeroUsize>,
        },
        Unrestricted,
    }

    /// 母本沙箱条目类型的最小占位（本地 Disabled 路径不构造 Restricted；字段按需对齐）。
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct FileSystemSandboxEntry {}

    /// 母本 models.rs:415-418 内建权限档位保留标识。
    pub const BUILT_IN_PERMISSION_PROFILE_READ_ONLY: &str = ":read-only";
    pub const BUILT_IN_PERMISSION_PROFILE_WORKSPACE: &str = ":workspace";
    pub const BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS: &str = ":danger-full-access";
    /// 母本 models.rs:1813——原样。
    pub fn local_image_label_text(label_number: usize) -> String {
        format!("[Image #{label_number}]")
    }
}

pub mod mcp {
    #[derive(Debug, Clone, PartialEq)] // 桩
    pub struct CallToolResult {}
    #[derive(Debug, Clone, PartialEq)] // 桩
    pub struct Tool {}
}

pub mod approvals { // 桩
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum NetworkPolicyAmendment {}
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ExecPolicyAmendment {}
}

pub mod permissions {
    /// 母本 permissions.rs:86 NetworkSandboxPolicy——原样变体集与方法。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum NetworkSandboxPolicy {
        #[default]
        Restricted,
        Enabled,
    }

    impl NetworkSandboxPolicy {
        pub fn is_enabled(self) -> bool {
            matches!(self, NetworkSandboxPolicy::Enabled)
        }
    }
}

pub mod plan_tool; // 017：真实三类型（008 桩被计划 cell 裁剪时移除，见 history_cell/mod.rs 登记）

pub mod parse_command {
    /// 母本 protocol/src/parse_command.rs ParsedCommand——原样变体集。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ParsedCommand {
        Read {
            cmd: String,
            name: String,
            /// (Best effort) Path to the file being read by the command. When
            /// possible, this is an absolute path, though when relative, it should
            /// be resolved against the `cwd`` that will be used to run the command
            /// to derive the absolute path.
            path: std::path::PathBuf,
        },
        ListFiles {
            cmd: String,
            path: Option<String>,
        },
        Search {
            cmd: String,
            query: Option<String>,
            path: Option<String>,
        },
        Unknown {
            cmd: String,
        },
    }
}

pub mod account { // 桩
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PlanType {}
}

pub mod user_input {
    /// 母本 user_input.rs:10。
    pub const MAX_USER_INPUT_TEXT_CHARS: usize = 1 << 20;
    /// 母本 user_input.rs:113 ByteRange。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ByteRange {
        pub start: usize,
        pub end: usize,
    }

    impl From<std::ops::Range<usize>> for ByteRange {
        fn from(range: std::ops::Range<usize>) -> Self {
            Self { start: range.start, end: range.end }
        }
    }
    /// 母本 user_input.rs:61 TextElement——原样结构与方法。
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TextElement {
        /// Byte range in the parent `text` buffer that this element occupies.
        pub byte_range: ByteRange,
        /// Optional human-readable placeholder for the element, displayed in the UI.
        placeholder: Option<String>,
    }

    impl TextElement {
        pub fn new(byte_range: ByteRange, placeholder: Option<String>) -> Self {
            Self { byte_range, placeholder }
        }

        pub fn map_range<F>(&self, map: F) -> Self
        where
            F: FnOnce(ByteRange) -> ByteRange,
        {
            Self {
                byte_range: map(self.byte_range),
                placeholder: self.placeholder.clone(),
            }
        }

        pub fn set_placeholder(&mut self, placeholder: Option<String>) {
            self.placeholder = placeholder;
        }

        #[doc(hidden)]
        pub fn _placeholder_for_conversion_only(&self) -> Option<&str> {
            self.placeholder.as_deref()
        }

        pub fn placeholder<'a>(&'a self, text: &'a str) -> Option<&'a str> {
            self.placeholder
                .as_deref()
                .or_else(|| text.get(self.byte_range.start..self.byte_range.end))
        }
    }
}

pub mod items { // 桩
    pub struct ResponseItem; // 桩：按需展开
}

pub mod turn_input { // 桩
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] // 桩
    pub enum CyberAccessProgram {}
}

pub mod error { // 桩
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum UnexpectedResponseError {}
}

#[cfg(test)]
mod num_format_tests {
    use super::num_format::format_with_separators;

    #[test]
    fn groups_thousands_en_us() {
        assert_eq!(format_with_separators(0), "0");
        assert_eq!(format_with_separators(999), "999");
        assert_eq!(format_with_separators(1_000), "1,000");
        assert_eq!(format_with_separators(12_345), "12,345");
        assert_eq!(format_with_separators(1_234_567), "1,234,567");
        assert_eq!(format_with_separators(-12_345), "-12,345");
    }
}

/// 母本 shell_environment.rs scrub_non_inheritable_env_vars——按母本真实签名
/// （std::process::Command）原样语义：剥离 BASH_FUNC* 导出与非可继承变量。
pub mod shell_environment {
    const NON_INHERITABLE_ENV_VARS: &[&str] = &["SHLVL", "_", "PWD", "OLDPWD", "BASHOPTS"];

    fn is_non_inheritable_env_var(name: &str) -> bool {
        name.starts_with("BASH_FUNC")
            || name.starts_with("BASH_VERSINFO")
            || NON_INHERITABLE_ENV_VARS.contains(&name)
    }

    pub fn scrub_non_inheritable_env_vars(command: &mut std::process::Command) {
        let configured_names = command
            .get_envs()
            .map(|(name, _)| name.to_os_string())
            .collect::<Vec<_>>();
        for name in NON_INHERITABLE_ENV_VARS {
            command.env_remove(name);
        }
        for name in std::env::vars_os()
            .map(|(name, _)| name)
            .chain(configured_names)
        {
            if name.to_str().is_some_and(is_non_inheritable_env_var) {
                command.env_remove(name);
            }
        }
    }
}
