use crate::history_cell::CompositeHistoryCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::plain_lines;
use crate::legacy_core::config::Config;
use crate::line_truncation::line_width;
use crate::token_usage::TokenUsage;
use crate::token_usage::TokenUsageInfo;
use crate::version::CODEX_CLI_VERSION;
use codex_protocol::ThreadId;
use ratatui::prelude::*;
use ratatui::style::Stylize;
use std::collections::BTreeSet;
use std::path::PathBuf;

// 008-TRIM: import 移除——AskForApproval/PlanType/ApprovalsReviewer/
// ActivePermissionProfile/BUILT_IN_PERMISSION_PROFILE_*/PermissionProfile/ReasoningEffort/
// PathUri/summarize_permission_profile/WireApi（权限/审批/沙箱、reasoning 配置、
// workspace roots 摘要）、StatusAccountDisplay/compose_account_display（账号/套餐）、
// rate_limits 系与 RemoteConnectionStatus/StatusThreadUsage（速率限额/credits、远程连接、
// thread usage）、DateTime/Local（仅服务于被裁的 rate limit 时间标签）（008 §1 裁剪：无本地事件源）
use super::format::FieldFormatter;
use super::format::push_label;
use super::helpers::compose_model_display;
use super::helpers::format_directory_display;
use super::helpers::format_tokens_compact;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;
use std::sync::Arc;
// 008-TRIM: use std::sync::RwLock（仅服务于被裁的 agents/rate-limit/thread-usage 共享状态）（008 §1 裁剪：无本地事件源）

// 008-TRIM: const CHATGPT_USAGE_URL 与 show_chatgpt_usage_link 渲染/hyperlink 附加
// （ChatGPT 账号 usage 链接，无本地事件源，008 §1 裁剪）

#[derive(Debug, Clone)]
struct StatusContextWindowData {
    percent_remaining: i64,
    tokens_in_context: i64,
    window: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct StatusTokenUsageData {
    total: i64,
    input: i64,
    output: i64,
    context_window: Option<StatusContextWindowData>,
}

// 008-TRIM: struct StatusRateLimitState 与 rate_limit_state 共享状态（速率限额刷新，
// 无本地事件源，008 §1 裁剪）

#[derive(Debug, Clone)]
pub(crate) struct StatusHistoryHandle {
    card: Arc<StatusHistoryCell>,
}

impl StatusHistoryHandle {
    pub(crate) fn copy_text(&self) -> String {
        self.card
            .content_lines(u16::MAX)
            .iter()
            .map(|line| line.to_string().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    }

    // 008-TRIM: reserve_thread_usage_label_width/finish_rate_limit_refresh/set_thread_usage
    // （thread usage 预留列宽、速率限额异步刷新回填，无本地事件源，008 §1 裁剪）
}

#[derive(Debug)]
struct StatusHistoryCell {
    model_name: String,
    model_details: Vec<String>,
    directory: PathBuf,
    model_provider: Option<String>,
    thread_name: Option<String>,
    session_id: Option<String>,
    token_usage: StatusTokenUsageData,
    // 008-TRIM: 字段移除——permissions/agents_summary/collaboration_mode/remote_connection/
    // show_chatgpt_usage_link/account/forked_from/rate_limit_state/thread_usage
    // （权限/审批/沙箱、AGENTS.md 摘要、collaboration mode、远程连接、ChatGPT 账号/
    // usage 链接、fork 来源、速率限额、thread usage——均无本地事件源，008 §1 裁剪）
}

#[cfg(test)]
pub(crate) fn new_status_output(
    config: &Config,
    token_info: Option<&TokenUsageInfo>,
    total_usage: &TokenUsage,
    session_id: &Option<ThreadId>,
    thread_name: Option<String>,
    model_name: &str,
) -> CompositeHistoryCell {
    // 008-TRIM: 参数移除——account_display/_plan_type/rate_limits/now/collaboration_mode/
    // reasoning_effort_override/forked_from（账号/套餐、速率限额、rate-limit 时间戳、
    // collaboration mode、reasoning 配置、fork 来源——均无本地事件源，008 §1 裁剪）
    new_status_output_with_handle(
        config,
        /*model_provider_id*/ None,
        token_info,
        total_usage,
        session_id,
        thread_name,
        model_name,
    )
    .0
}

// 008-TRIM: fn new_status_output_with_rate_limits（#[cfg(test)] 多限额快照入口，
// 无本地事件源，008 §1 裁剪）

#[allow(clippy::too_many_arguments)]
pub(crate) fn new_status_output_with_handle(
    config: &Config,
    model_provider_id: Option<&str>,
    token_info: Option<&TokenUsageInfo>,
    total_usage: &TokenUsage,
    session_id: &Option<ThreadId>,
    thread_name: Option<String>,
    model_name: &str,
) -> (CompositeHistoryCell, StatusHistoryHandle) {
    // 008-TRIM: 原 new_status_output_with_rate_limits_handle——参数移除
    // requires_openai_auth/remote_connection/account_display/rate_limits/_plan_type/now/
    // collaboration_mode/reasoning_effort_override/agents_summary/refreshing_rate_limits
    // （ChatGPT 认证/usage 链接、远程连接、账号/套餐、速率限额及其刷新、collaboration
    // mode、reasoning 配置、AGENTS.md 摘要——均无本地事件源，008 §1 裁剪；随参数
    // 移除更名去掉名不符实的 with_rate_limits）
    let command = PlainHistoryCell::new(vec!["/status".magenta().into()]);
    let card = Arc::new(StatusHistoryCell::new(
        config,
        model_provider_id,
        token_info,
        total_usage,
        session_id,
        thread_name,
        model_name,
    ));
    let handle = StatusHistoryHandle {
        card: Arc::clone(&card),
    };

    (
        CompositeHistoryCell::new(vec![Box::new(command), Box::new(card)]),
        handle,
    )
}

impl StatusHistoryCell {
    fn new(
        config: &Config,
        model_provider_id: Option<&str>,
        token_info: Option<&TokenUsageInfo>,
        total_usage: &TokenUsage,
        session_id: &Option<ThreadId>,
        thread_name: Option<String>,
        model_name: &str,
    ) -> Self {
        // 008-TRIM: approval_policy/permission_profile/workspace_roots/cwd(PathUri)/
        // config_entries(approval/sandbox/workdir)/reasoning effort·summaries 条目/
        // approval·permissions·sandbox 标签构建整体移除（权限/审批/沙箱、reasoning 配置，
        // 无本地事件源，008 §1 裁剪；compose_model_display 的 entries 本地恒为空）
        let model_provider = model_provider_id
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string);
        let (model_name, model_details) = compose_model_display(model_name, &[]);
        let session_id = session_id.as_ref().map(std::string::ToString::to_string);
        let default_usage = TokenUsage::default();
        let (context_usage, context_window) = match token_info {
            Some(info) => (&info.last_token_usage, info.model_context_window),
            None => (&default_usage, config.model_context_window),
        };
        let context_window = context_window.map(|window| StatusContextWindowData {
            percent_remaining: context_usage.percent_of_context_window_remaining(window),
            tokens_in_context: context_usage.tokens_in_context_window(),
            window,
        });

        let token_usage = StatusTokenUsageData {
            total: total_usage.blended_total(),
            input: total_usage.non_cached_input(),
            output: total_usage.output_tokens,
            context_window,
        };

        Self {
            model_name,
            model_details,
            directory: config.cwd.to_path_buf(),
            model_provider,
            thread_name,
            session_id,
            token_usage,
        }
    }

    fn token_usage_spans(&self) -> Vec<Span<'static>> {
        let total_fmt = format_tokens_compact(self.token_usage.total);
        let input_fmt = format_tokens_compact(self.token_usage.input);
        let output_fmt = format_tokens_compact(self.token_usage.output);

        vec![
            Span::from(total_fmt),
            Span::from(" total "),
            Span::from(" (").dim(),
            Span::from(input_fmt).dim(),
            Span::from(" input").dim(),
            Span::from(" + ").dim(),
            Span::from(output_fmt).dim(),
            Span::from(" output").dim(),
            Span::from(")").dim(),
        ]
    }

    fn context_window_spans(&self) -> Option<Vec<Span<'static>>> {
        let context = self.token_usage.context_window.as_ref()?;
        let percent = context.percent_remaining;
        let used_fmt = format_tokens_compact(context.tokens_in_context);
        let window_fmt = format_tokens_compact(context.window);

        Some(vec![
            Span::from(format!("{percent}% left")),
            Span::from(" (").dim(),
            Span::from(used_fmt).dim(),
            Span::from(" used / ").dim(),
            Span::from(window_fmt).dim(),
            Span::from(")").dim(),
        ])
    }

    // 008-TRIM: fn rate_limit_lines/rate_limit_row_lines/collect_rate_limit_labels
    // （状态卡限额行渲染与标签收集，无本地事件源，008 §1 裁剪）
}

// 008-TRIM: fn status_permission_summary/workspace_root_suffix/status_permissions_label/
// decorate_workspace_sandbox_label/status_approval_label（权限/审批/沙箱标签构建，
// 无本地事件源，008 §1 裁剪）

impl StatusHistoryCell {
    fn content_lines(&self, width: u16) -> Vec<Line<'static>> {
        let available_width = usize::from(width);
        if available_width == 0 {
            return Vec::new();
        }
        let mut lines = Vec::new();

        // 008-TRIM: account_value 组装（ChatGPT 账号显示，无本地事件源，008 §1 裁剪）

        // 008-TRIM: 基础标签 "Permissions"/"Agents.md" 移除（权限/审批/沙箱、AGENTS.md
        // 摘要，无本地事件源，008 §1 裁剪）
        let mut labels: Vec<String> = vec!["Model", "Directory"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut seen: BTreeSet<String> = labels.iter().cloned().collect();
        let thread_name = self.thread_name.as_deref().filter(|name| !name.is_empty());

        if self.model_provider.is_some() {
            push_label(&mut labels, &mut seen, "Model provider");
        }
        if thread_name.is_some() {
            push_label(&mut labels, &mut seen, "Thread name");
        }
        if self.session_id.is_some() {
            push_label(&mut labels, &mut seen, "Session");
        }
        // 008-TRIM: "Forked from"/"Collaboration mode" 标签移除（fork 来源、collaboration
        // mode，无本地事件源，008 §1 裁剪）
        push_label(&mut labels, &mut seen, "Token usage");
        if self.token_usage.context_window.is_some() {
            push_label(&mut labels, &mut seen, "Context window");
        }
        // 008-TRIM: collect_rate_limit_labels 与 thread_usage.push_labels 调用
        // （速率限额、thread usage 标签，无本地事件源，008 §1 裁剪）

        let formatter = FieldFormatter::from_labels(labels.iter().map(String::as_str));
        let value_width = formatter.value_width(available_width);

        // 008-TRIM: remote_connection "Server" 行渲染（远程连接状态，无本地事件源，008 §1 裁剪）

        let mut model_spans = vec![Span::from(self.model_name.clone())];
        if !self.model_details.is_empty() {
            model_spans.push(Span::from(" (").dim());
            model_spans.push(Span::from(self.model_details.join(", ")).dim());
            model_spans.push(Span::from(")").dim());
        }

        let directory_value = format_directory_display(&self.directory, /*max_width*/ None);

        lines.push(formatter.line("Model", model_spans));
        if let Some(model_provider) = self.model_provider.as_ref() {
            lines.push(formatter.line("Model provider", vec![Span::from(model_provider.clone())]));
        }
        lines.push(formatter.line("Directory", vec![Span::from(directory_value)]));
        // 008-TRIM: "Permissions"/"Agents.md" 行（权限/审批/沙箱、AGENTS.md 摘要，
        // 无本地事件源，008 §1 裁剪）

        // 008-TRIM: "Account" 行（ChatGPT 账号显示，无本地事件源，008 §1 裁剪）

        if let Some(thread_name) = thread_name {
            lines.push(formatter.line("Thread name", vec![Span::from(thread_name.to_string())]));
        }
        // 008-TRIM: "Collaboration mode" 行（无本地事件源，008 §1 裁剪）
        if let Some(session) = self.session_id.as_ref() {
            lines.push(formatter.line("Session", vec![Span::from(session.clone())]));
        }
        // 008-TRIM: "Forked from" 行（无本地事件源，008 §1 裁剪）

        lines.push(Line::from(Vec::<Span<'static>>::new()));
        // 008-TRIM: "Hide token usage only for ChatGPT subscribers" 条件随账号裁剪移除，
        // 本地恒显示 token 用量（008 §1 裁剪：无本地事件源）
        lines.push(formatter.line("Token usage", self.token_usage_spans()));

        if let Some(spans) = self.context_window_spans() {
            lines.push(formatter.line("Context window", spans));
        }

        // 008-TRIM: rate_limit_lines 延展与 thread_usage 行（速率限额、thread usage，
        // 无本地事件源，008 §1 裁剪）

        // Leave room for the two-column title mark even in a tiny terminal.
        let indent = if available_width >= FieldFormatter::INDENT.len() + 2 {
            FieldFormatter::INDENT
        } else {
            ""
        };
        let mut title = vec![indent.into()];
        title.extend(crate::history_cell::codex_title(CODEX_CLI_VERSION));
        let mut rendered = word_wrap_lines(
            [Line::from(title)],
            RtOptions::new(available_width).subsequent_indent(indent.into()),
        );
        rendered.push(Line::default());
        // 008-TRIM: ChatGPT usage 链接块（show_chatgpt_usage_link/CHATGPT_USAGE_URL，
        // 无本地事件源，008 §1 裁剪）
        // Keep every value, including long paths and IDs, aligned beneath its first line.
        // At widths too small for the label column, use the outer indent instead.
        let continuation = if value_width > 0 {
            formatter.continuation(Vec::new())
        } else {
            indent.into()
        };
        let options = RtOptions::new(available_width)
            .subsequent_indent(continuation)
            .word_splitter(textwrap::WordSplitter::NoHyphenation);
        rendered.extend(lines.into_iter().flat_map(|line| {
            let wrapped = word_wrap_lines([line.clone()], options.clone());
            if wrapped
                .iter()
                .any(|line| line_width(line) > available_width)
            {
                // A wide grapheme may not fit after the continuation indent.
                word_wrap_lines(
                    [line],
                    RtOptions::new(available_width)
                        .word_splitter(textwrap::WordSplitter::NoHyphenation),
                )
            } else {
                wrapped
            }
        }));

        rendered
    }
}

impl HistoryCell for Arc<StatusHistoryCell> {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.content_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        plain_lines(self.display_lines(u16::MAX))
    }

    fn display_hyperlink_lines(
        &self,
        width: u16,
    ) -> Vec<crate::terminal_hyperlinks::HyperlinkLine> {
        // 008-TRIM: usage URL hyperlink 附加循环（ChatGPT usage 链接，无本地事件源，008 §1 裁剪）
        crate::terminal_hyperlinks::plain_hyperlink_lines(self.display_lines(width))
    }

    fn transcript_hyperlink_lines(
        &self,
        width: u16,
    ) -> Vec<crate::terminal_hyperlinks::HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }
}
