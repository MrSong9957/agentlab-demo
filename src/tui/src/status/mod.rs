//! Status output formatting and display adapters for the TUI.
//!
//! This module turns protocol-level snapshots into stable display structures used by `/status`
//! output and footer/status-line helpers, while keeping rendering concerns out of transport-facing
//! code. The status summary is borderless and wraps values without truncating paths or IDs.
//!
//! 008-TRIM: `rate_limits`/`remote_connection`/`thread_usage`/`account` 子模块整体移除
//! （008 §1 裁剪：无本地事件源——速率限额/credits、远程连接、thread usage、账号/套餐）；
//! 保留范围＝模型名、当前目录、thread 名、session、上下文窗口百分比、token 用量文本。
mod card;
mod format;
mod helpers;
// 008-TRIM: mod account（ChatGPT 账号/套餐显示，无本地事件源，008 §1 裁剪）
// 008-TRIM: mod rate_limits（速率限额/credits/spend control，无本地事件源，008 §1 裁剪）
// 008-TRIM: mod remote_connection（远程 app-server 连接状态，无本地事件源，008 §1 裁剪）
// 008-TRIM: mod thread_usage（thread 计费/credits 估算面板，无本地事件源，008 §1 裁剪）

pub(crate) use card::StatusHistoryHandle;
#[cfg(test)]
pub(crate) use card::new_status_output;
pub(crate) use card::new_status_output_with_handle;
pub(crate) use helpers::format_directory_display;
pub(crate) use helpers::format_tokens_compact;
// 008-TRIM: re-export StatusAccountDisplay/compose_agents_summary/format_reset_timestamp/
// plan_type_display_name/RateLimit*Display/rate_limit_snapshot_display*/format_credit_micros/
// format_estimated_usd_micros/new_status_output_with_rate_limits(_handle 原名)——
// 均属被裁功能或其入口（008 §1 裁剪：无本地事件源）

#[cfg(test)]
mod tests;
