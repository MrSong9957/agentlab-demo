use crate::exec_command::relativize_to_home;
use crate::text_formatting;
use crate::width::display_width;
use std::path::Path;

// 008-TRIM: import 移除——ClockFormat/DateTime/Local（format_reset_timestamp 服务于被裁的
// 限额重置时间）、Config/PathConvention/PathUri（compose_agents_summary 服务于被裁的
// AGENTS.md 摘要）、StatusAccountDisplay/PlanType（账号/套餐）（008 §1 裁剪：无本地事件源）

// 008-TRIM: fn normalize_agents_display_path（AGENTS.md 摘要路径显示，无本地事件源，
// 008 §1 裁剪）

pub(crate) fn compose_model_display(
    model_name: &str,
    entries: &[(&str, String)],
) -> (String, Vec<String>) {
    let mut details: Vec<String> = Vec::new();
    if let Some((_, effort)) = entries.iter().find(|(k, _)| *k == "reasoning effort") {
        details.push(format!("reasoning {}", effort.to_ascii_lowercase()));
    }
    if let Some((_, summary)) = entries.iter().find(|(k, _)| *k == "reasoning summaries") {
        let summary = summary.trim();
        if summary.eq_ignore_ascii_case("none") || summary.eq_ignore_ascii_case("off") {
            details.push("summaries off".to_string());
        } else if !summary.is_empty() {
            details.push(format!("summaries {}", summary.to_ascii_lowercase()));
        }
    }

    (model_name.to_string(), details)
}

// 008-TRIM: fn compose_agents_summary（AGENTS.md 指令文件摘要，本地无指令文件发现机制，
// 无本地事件源，008 §1 裁剪）
// 008-TRIM: fn compose_account_display（ChatGPT 账号显示，无本地事件源，008 §1 裁剪）
// 008-TRIM: fn plan_type_display_name（订阅套餐显示名，依赖 codex 订阅体系，无本地事件源，
// 008 §1 裁剪）

pub(crate) fn format_tokens_compact(value: i64) -> String {
    let value = value.max(0);
    if value == 0 {
        return "0".to_string();
    }
    if value < 1_000 {
        return value.to_string();
    }

    let value_f64 = value as f64;
    let (scaled, suffix) = if value >= 1_000_000_000_000 {
        (value_f64 / 1_000_000_000_000.0, "T")
    } else if value >= 1_000_000_000 {
        (value_f64 / 1_000_000_000.0, "B")
    } else if value >= 1_000_000 {
        (value_f64 / 1_000_000.0, "M")
    } else {
        (value_f64 / 1_000.0, "K")
    };

    let decimals = if scaled < 10.0 {
        2
    } else if scaled < 100.0 {
        1
    } else {
        0
    };

    let mut formatted = format!("{scaled:.decimals$}");
    if formatted.contains('.') {
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
    }

    format!("{formatted}{suffix}")
}

pub(crate) fn format_directory_display(directory: &Path, max_width: Option<usize>) -> String {
    let formatted = if let Some(rel) = relativize_to_home(directory) {
        if rel.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display())
        }
    } else {
        directory.display().to_string()
    };

    if let Some(max_width) = max_width {
        if max_width == 0 {
            return String::new();
        }
        if display_width(&formatted) > max_width {
            return text_formatting::center_truncate_path(&formatted, max_width);
        }
    }

    formatted
}

// 008-TRIM: fn format_reset_timestamp（限额重置时间标签，服务于被裁的速率限额/spend
// control 显示，无本地事件源，008 §1 裁剪）

#[cfg(test)]
mod tests {
    use super::*;

    // 008-TRIM: test_config 辅助与 plan_type_display_name_remaps_display_labels/
    // compose_agents_summary_* 系列测试（账号/套餐、AGENTS.md 摘要——被裁功能，
    // 008 §1 裁剪：无本地事件源）

    #[test]
    fn format_directory_display_truncates_halfwidth_sound_marks() {
        let directory = Path::new("workspace").join("ｶﾞ").join("project");
        let max_width = display_width(directory.to_string_lossy().as_ref()) - 1;
        let formatted = format_directory_display(&directory, Some(max_width));

        insta::assert_snapshot!(formatted.replace('\\', "/"), @"workspace/…/project");
    }
}
