// 008-TRIM: 本文件收敛为母本 status/tests.rs 的保留路径子集（008 §1 裁剪）。
// 随裁剪删除的测试（均只覆盖无本地事件源功能）：
// - 速率限额/credits：stale_monthly_limit_marks_fresh_rolling_snapshot_stale、
//   status_snapshot_includes_monthly_limit、status_snapshot_includes_enterprise_monthly_
//   credit_limit、status_snapshot_uses_generic_limit_labels_for_unsupported_windows、
//   status_snapshot_shows_unlimited_credits、status_snapshot_shows_positive_credits、
//   status_snapshot_shows_available_credits_without_display_balance、
//   status_snapshot_respects_unlimited_without_has_credits_flag、
//   status_snapshot_shows_missing_limits_message、status_snapshot_shows_refreshing_limits_
//   notice、transcript_overlay_remeasures_status_after_rate_limit_refresh、
//   status_snapshot_includes_credits_and_limits、status_snapshot_shows_unavailable_limits_
//   message、status_snapshot_treats_refreshing_empty_limits_as_unavailable、
//   status_snapshot_shows_stale_limits_message、status_snapshot_cached_limits_hide_credits_
//   without_flag、reset_timestamps_follow_clock_preference
// - 权限/审批/沙箱：app_server_workspace_write_profile/permissions_text_for(_width) 辅助与
//   status_permissions_*（11 项）、status_snapshot_shows_active_user_defined_profile、
//   status_snapshot_shows_auto_review_permissions、status_permissions_include_executor_
//   profile_root
// - 账号/套餐/认证：status_snapshot_shows_chatgpt_plan_without_email、
//   status_uses_server_provider_id_and_auth_requirement（含 ChatGPT usage 链接断言）
// - reasoning 配置：status_snapshot_includes_reasoning_details、
//   status_snapshot_uses_default_reasoning_when_config_empty
// - fork 来源：status_snapshot_includes_forked_from
// 另：status_snapshot_wraps_in_narrow_terminal 与 status_snapshot_wraps_halfwidth_kana_in_
// narrow_terminal 的断言对象（窄终端换行/宽度）属保留路径，但其输入与 insta 快照内容以被裁
// 行（限额、reasoning、账号、collaboration mode）为主体，裁剪后无法原样编译/比对，随裁剪
// 删除并在此登记（对应 .snap 文件按指示暂留未动）；窄终端换行回归由
// status_wraps_long_paths_and_session_ids_without_losing_text（逐宽度断言，无快照）承接。
// rate_limits.rs/remote_connection.rs/thread_usage(.rs+_tests)/account.rs 的内联测试随
// 整文件裁剪移除。

use super::new_status_output;
use super::new_status_output_with_handle;
use crate::history_cell::HistoryCell;
use crate::legacy_core::config::Config;
use crate::test_support::PathBufExt;
use crate::test_support::test_path_buf;
use crate::token_usage::TokenUsage;
use crate::token_usage::TokenUsageInfo;
use codex_protocol::ThreadId;
use codex_utils_absolute_path::AbsolutePathBuf;
use ratatui::prelude::*;
use tempfile::TempDir;

fn test_config(temp_home: &TempDir) -> Config {
    // 008-ADAPT: 母本经 ConfigBuilder 异步装载并装配审批/权限/workspace_roots；本地无
    // 对应事件源与 builder，直接构造 shim Config 子集（保留路径消费 cwd 等字段）。
    Config {
        codex_home: temp_home.path().to_path_buf().abs(),
        ..Config::default()
    }
}

fn set_workspace_cwd(config: &mut Config, cwd: AbsolutePathBuf) {
    // 008-TRIM: workspace_roots/permissions 装配移除（无本地事件源，008 §1 裁剪）
    config.cwd = cwd.clone();
}

fn token_info_for(usage: &TokenUsage, context_window: Option<i64>) -> TokenUsageInfo {
    // 008-ADAPT: 母本经 codex_models_manager 离线目录解析 context_window；本地无该目录，
    // 直接给定窗口值（保留路径断言不依赖目录内容）。
    TokenUsageInfo {
        total_token_usage: usage.clone(),
        last_token_usage: usage.clone(),
        model_context_window: context_window,
    }
}

fn render_lines(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn status_card_token_usage_excludes_cached_tokens() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home);
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());

    let usage = TokenUsage {
        input_tokens: 1_200,
        cached_input_tokens: 200,
        output_tokens: 900,
        reasoning_output_tokens: 0,
        total_tokens: 2_100,
    };

    let token_info = token_info_for(&usage, Some(272_000));
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        /*thread_name*/ None,
        "gpt-5.1-codex-max",
    );
    let rendered = render_lines(&composite.display_lines(/*width*/ 120));

    // 母本原断言（status/tests.rs:1488）恢复：缓存 token 不得显示。
    // 另加正向存在断言（token 行确实渲染）防空跑：渲染值＝非缓存输入 1.0K＋900 输出＝1.9K。
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Token usage") && line.contains("1.9K total")),
        "token usage row missing, got: {rendered:?}"
    );
    assert!(
        rendered.iter().all(|line| !line.contains("cached")),
        "token usage row missing, got: {rendered:?}"
    );
    assert!(
        rendered.iter().all(|line| !line.contains("cached")),
        "cached tokens should not be displayed, got: {rendered:?}"
    );
}

#[test]
fn status_wraps_long_paths_and_session_ids_without_losing_text() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home);
    let directory = test_path_buf("/workspace/projects/界界/ｶﾞﾞ/a-very-long-directory-name/codex");
    set_workspace_cwd(&mut config, directory.abs());
    // 008-ADAPT: 母本 ThreadId::from_string；shim ThreadId 以 String 承载，改用
    // ThreadId::new，且 shim ThreadId 非 Copy，传入时显式 clone。
    let session = ThreadId::new("00000000-0000-0000-0000-000000000123");
    let usage = TokenUsage::default();
    let (status, handle) = new_status_output_with_handle(
        &config,
        /*model_provider_id*/ None,
        /*token_info*/ None,
        &usage,
        &Some(session.clone()),
        Some("A thread with a long descriptive name".to_string()),
        "gpt-5.5",
    );
    let directory = directory.to_string_lossy();
    for width in [7, 12, 17, 18, 24, 25, 40, 80] {
        let lines = status.display_lines(width);
        assert!(
            lines.iter().all(|line| line.width() <= usize::from(width)),
            "overwide row at width {width}: {:?}",
            render_lines(&lines)
        );
        let joined = lines
            .iter()
            .map(|line| line.to_string().trim().to_string())
            .collect::<String>();
        assert!(
            joined.contains(directory.as_ref()),
            "path lost at width {width}: {joined}"
        );
        assert!(
            joined.contains(&session.to_string()),
            "session lost at width {width}: {joined}"
        );
    }
    assert!(handle.copy_text().contains(directory.as_ref()));
    assert!(handle.copy_text().contains(&session.to_string()));
}

#[test]
fn status_context_window_uses_last_usage() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home);
    config.model_context_window = Some(272_000);
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());

    let total_usage = TokenUsage {
        input_tokens: 12_800,
        cached_input_tokens: 0,
        output_tokens: 879,
        reasoning_output_tokens: 0,
        total_tokens: 102_000,
    };
    let last_usage = TokenUsage {
        input_tokens: 12_800,
        cached_input_tokens: 0,
        output_tokens: 879,
        reasoning_output_tokens: 0,
        total_tokens: 13_679,
    };

    let token_info = TokenUsageInfo {
        total_token_usage: total_usage.clone(),
        last_token_usage: last_usage,
        model_context_window: config.model_context_window,
    };
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &total_usage,
        &None,
        /*thread_name*/ None,
        "gpt-5.5",
    );
    let rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    let context_line = rendered_lines
        .into_iter()
        .find(|line| line.contains("Context window"))
        .expect("context line");

    assert!(
        context_line.contains("13.7K used / 272K"),
        "expected context line to reflect last usage tokens, got: {context_line}"
    );
    assert!(
        !context_line.contains("102K"),
        "context line should not use total aggregated tokens, got: {context_line}"
    );
}
