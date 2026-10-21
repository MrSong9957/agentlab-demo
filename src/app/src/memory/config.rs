//! 013：manual.json 的 `memories` 配置段（D5；M1 最小面＋M2 候选筛选参数）。
//!
//! 兼容契约：段缺省＝未启用（默认 false），旧配置零改动可继续装载；
//! `deny_unknown_fields` 与 catalog 其他手动配置结构一致——未知键报错，不静默忽略。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! `codex-rs/config/src/types.rs`——默认值＝`DEFAULT_MEMORIES_*` 常量，钳制范围＝
//! `From<MemoriesToml> for MemoriesConfig` 的 `.clamp(...)`（1–128 / 0–90 / 1–48）。

use serde::{Deserialize, Serialize};

fn default_use_memories() -> bool {
    true
}

/// 014 §3：generate_memories 缺省值（上游默认 true——仅来源资格，非管线开关）。
fn default_generate_memories() -> bool {
    true
}

fn default_max_rollouts_per_startup() -> usize {
    2
}
fn default_max_rollout_age_days() -> i64 {
    10
}
fn default_min_rollout_idle_hours() -> i64 {
    6
}
fn default_max_raw_memories_for_consolidation() -> usize {
    256
}
fn default_max_unused_days() -> i64 {
    30
}

/// memories 段。本结构＝上游 `MemoriesToml` 面（装载原值，越界值保留）；
/// [`MemoriesConfig::clamped`] ＝上游 `From<MemoriesToml>` 换算后的 effective 面
/// （默认与钳制在此生效）——**消费方必须经 `clamped()`/`params_from_config`
/// 取值**，不得直接使用越界原值。候选筛选参数（M2）缺省取上游默认。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoriesConfig {
    /// 记忆总功能开关（上游 `features.memories` 语义；默认关闭）。
    #[serde(default)]
    pub enabled: bool,
    /// 013-M6：是否注入读取上下文（上游 use_memories，默认 true）；
    /// 与总开关独立，任一关闭均不读取摘要。
    #[serde(default = "default_use_memories")]
    pub use_memories: bool,
    /// 014 §3：新登记会话的来源资格开关（上游 `generate_memories`，默认 true，
    /// 兼容既有行为）。仅决定登记时 `threads.memory_mode` 初值
    /// （enabled/disabled）——不门控后台管线、提炼、合并、清理、注入或反馈。
    #[serde(default = "default_generate_memories")]
    pub generate_memories: bool,
    /// 每轮领取上限（上游 `max_rollouts_per_startup`，默认 2，1–128）。
    #[serde(default = "default_max_rollouts_per_startup")]
    pub max_rollouts_per_startup: usize,
    /// 源会话年龄窗口天数（上游 `max_rollout_age_days`，默认 10，0–90）。
    #[serde(default = "default_max_rollout_age_days")]
    pub max_rollout_age_days: i64,
    /// 源会话闲置门槛小时（上游 `min_rollout_idle_hours`，默认 6，1–48）。
    #[serde(default = "default_min_rollout_idle_hours")]
    pub min_rollout_idle_hours: i64,
    /// 合并输入的来源行上限（上游 `max_raw_memories_for_consolidation`，
    /// 默认 256，1–4096；M5）。
    #[serde(default = "default_max_raw_memories_for_consolidation")]
    pub max_raw_memories_for_consolidation: usize,
    /// 合并输入的未使用窗口天数（上游 `max_unused_days`，默认 30，0–365；M5）。
    #[serde(default = "default_max_unused_days")]
    pub max_unused_days: i64,
}

/// `Default` 与 serde 缺省字段对齐（上游 `MemoriesConfig::default()` 同值）；
/// 不得用 derive——那会把数量参数置 0，与 `{}` 反序列化结果分裂。
impl Default for MemoriesConfig {
    fn default() -> Self {
        MemoriesConfig {
            enabled: false,
            use_memories: default_use_memories(),
            generate_memories: default_generate_memories(),
            max_rollouts_per_startup: default_max_rollouts_per_startup(),
            max_rollout_age_days: default_max_rollout_age_days(),
            min_rollout_idle_hours: default_min_rollout_idle_hours(),
            max_raw_memories_for_consolidation: default_max_raw_memories_for_consolidation(),
            max_unused_days: default_max_unused_days(),
        }
    }
}

impl MemoriesConfig {
    /// 越界值钳制（上游 `From<MemoriesToml>` 逐字段 `.clamp` 语义）。
    pub fn clamped(&self) -> MemoriesConfig {
        MemoriesConfig {
            enabled: self.enabled,
            use_memories: self.use_memories,
            generate_memories: self.generate_memories,
            max_rollouts_per_startup: self.max_rollouts_per_startup.clamp(1, 128),
            max_rollout_age_days: self.max_rollout_age_days.clamp(0, 90),
            min_rollout_idle_hours: self.min_rollout_idle_hours.clamp(1, 48),
            max_raw_memories_for_consolidation: self
                .max_raw_memories_for_consolidation
                .clamp(1, 4096),
            max_unused_days: self.max_unused_days.clamp(0, 365),
        }
    }
}
