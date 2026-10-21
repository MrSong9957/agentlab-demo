//! 用户轮上下文注入（015 规格）：项目规则（AGENTS.md 链）＋环境上下文的来源
//! 加载、消息构造与去重识别。仅读取与组装：不联网、不执行命令、不写文件。
//! 来源全部参数化（[`ContextSources`]），测试注入临时目录与固定时钟；
//! 块内布局对齐调查样本 item5（标题后空行、内容行包装、段间分隔行）。

use crate::memory::Clock;
use crate::prompt::{HistoryItem, HistoryMessage, HistoryRole};
use std::io::Read;
use std::path::{Path, PathBuf};

/// 规则正文（全局＋项目链拼接，含段间分隔）固定字节预算（015 §1；Codex
/// `project_doc_max_bytes` 默认值）。标题、`<INSTRUCTIONS>` 包装行与环境块
/// 不占该预算。
const CONTENT_BUDGET_BYTES: usize = 32_768;

/// 段间分隔（全局→项目根→cwd 逐来源之间，字节计入预算）。
const SEGMENT_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";

/// 上下文组装来源（015 §1）：宿主每用户轮解析一次后传入。
#[derive(Clone)]
pub struct ContextSources {
    /// 全局规则目录（002 `config_dir` 机制，`TCHAT_CONFIG_DIR` 可覆盖）。
    pub global_dir: PathBuf,
    /// 进程工作目录：项目链起点（向上找最近含 `.git` 的祖先）与环境块 cwd。
    pub cwd: PathBuf,
    /// 环境变量 `SHELL`；缺失传 `None`。
    pub shell: Option<String>,
    /// 注入时钟（013 `Clock` 模式）：UTC 日期来源。
    pub clock: Clock,
}

impl ContextSources {
    /// 宿主生产来源：全局目录＝`config_dir()`，cwd＝进程工作目录（失败回退
    /// `.`），shell＝`SHELL`，时钟＝系统时钟。测试注入临时目录与固定时钟。
    pub fn from_env() -> Self {
        Self {
            global_dir: crate::catalog::config_dir(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            shell: std::env::var("SHELL").ok(),
            clock: crate::memory::system_clock(),
        }
    }
}

/// 组装结果：`message` 为完整上下文消息文本（不含替换声明）；
/// `notices` 为跳过/截断提示行（含路径与原因类别，不含规则正文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltContext {
    pub message: String,
    pub notices: Vec<String>,
}

/// 加载来源的元数据判定（015 文件异常边界）：仅自动加载「`AGENTS.md` 本身是
/// 普通、非符号链接文件」的来源；缺失属正常路径（静默）。
enum SourceProbe {
    /// 普通文件（含字节大小）。
    Regular(u64),
    Missing,
    Symlink,
    NotRegular,
    /// 元数据读取失败（权限等）。
    MetaError,
}

fn probe_source(path: &Path) -> SourceProbe {
    match std::fs::symlink_metadata(path) {
        Ok(md) => {
            if md.file_type().is_symlink() {
                SourceProbe::Symlink
            } else if md.is_file() {
                SourceProbe::Regular(md.len())
            } else {
                SourceProbe::NotRegular
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SourceProbe::Missing,
        Err(_) => SourceProbe::MetaError,
    }
}

fn skip_notice(path: &Path, reason: &str) -> String {
    format!("上下文加载：跳过 {}（{reason}）", path.display())
}

/// 有界读取并解码：至多 `read_limit` 字节（调用方给剩余预算＋3，用于多字节
/// 边界判定）。文件长于读取上限且缓冲尾是不完整多字节序列时按字符边界收尾
/// （读取边界，非文件损坏）；文件本身非法 UTF-8 或读取失败则跳过并记原因。
fn load_source_text(
    path: &Path,
    file_size: u64,
    read_limit: usize,
    notices: &mut Vec<String>,
) -> Option<String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            notices.push(skip_notice(path, "读取失败"));
            return None;
        }
    };
    let mut buf = Vec::new();
    if file.take(read_limit as u64).read_to_end(&mut buf).is_err() {
        notices.push(skip_notice(path, "读取失败"));
        return None;
    }
    let file_longer = file_size > read_limit as u64;
    match String::from_utf8(buf) {
        Ok(text) => Some(text),
        Err(e) => {
            if e.utf8_error().error_len().is_none() && file_longer {
                // 读取边界截断多字节字符：回退到完整字符（valid_up_to 保证
                // 前缀仍是合法 UTF-8，重建不会失败）
                let valid = e.utf8_error().valid_up_to();
                String::from_utf8(e.into_bytes()[..valid].to_vec()).ok()
            } else {
                notices.push(skip_notice(path, "非法 UTF-8"));
                None
            }
        }
    }
}

/// 预算耗尽后停止加载（015 §1）：不再打开任何后续来源文件；仅以元数据判定
/// 是否存在后续普通文件来源，存在则记一行提示（不读正文，纯空白来源可能被
/// 多报——罕见角形，换取「不再读取」的严格性）。
fn has_regular_source(path: &Path) -> bool {
    matches!(probe_source(path), SourceProbe::Regular(_))
}

/// 拼接预算内的 UTF-8 前缀；超出则按字符边界回退并标记截断。
enum BudgetPrefix<'a> {
    Within(&'a str),
    Truncated(&'a str),
}

fn utf8_prefix(text: &str, max: usize) -> BudgetPrefix<'_> {
    if text.len() <= max {
        BudgetPrefix::Within(text)
    } else {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        BudgetPrefix::Truncated(&text[..end])
    }
}

/// 项目规则链（015 §1）：从 cwd 向上找最近含 `.git` 的祖先（含 cwd 自身）
/// 作为根，返回根→cwd 目录序；无 `.git` 祖先时仅 cwd 一个目录。
fn project_chain_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut root = None;
    let mut dir = cwd;
    loop {
        if dir.join(".git").symlink_metadata().is_ok() {
            root = Some(dir);
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    let Some(root) = root else {
        return vec![cwd.to_path_buf()];
    };
    let mut chain = vec![cwd.to_path_buf()];
    let mut cur = cwd;
    while cur != root {
        cur = cur.parent().expect("root 由 cwd 逐级向上扫描所得，必为祖先");
        chain.push(cur.to_path_buf());
    }
    chain.reverse();
    chain
}

/// XML 文本转义（环境块值；标题行非 XML 上下文，不转义）。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// UTC 日期 `YYYY-MM-DD`（注入时钟 epoch 秒；chrono 无 clock feature，013-M4 模式）。
fn utc_date(clock: &Clock) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(clock(), 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".into())
}

fn environment_block(sources: &ContextSources) -> String {
    let cwd = sources.cwd.display().to_string();
    let mut block = String::from("<environment_context>\n");
    block.push_str(&format!("  <cwd>{}</cwd>\n", xml_escape(&cwd)));
    if let Some(shell) = sources.shell.as_deref() {
        if !shell.trim().is_empty() {
            block.push_str(&format!("  <shell>{}</shell>\n", xml_escape(shell)));
        }
    }
    block.push_str(&format!(
        "  <current_date>{}</current_date>\n",
        utc_date(&sources.clock)
    ));
    block.push_str("</environment_context>");
    block
}

/// 加载来源并构造上下文消息（015 §1 字节级形态）。规则正文按「全局→项目
/// 根→cwd」顺序拼接为单一 `content`（来源文本首尾 trim，段间固定分隔，分隔
/// 计入预算）；读取按剩余预算有界，预算耗尽（含恰好占满）停止加载后续来源。
pub fn build_context_message(sources: &ContextSources) -> BuiltContext {
    let mut notices = Vec::new();
    let mut content = String::new();
    let mut has_project = false;

    let chain = project_chain_dirs(&sources.cwd);
    let paths: Vec<PathBuf> = std::iter::once(sources.global_dir.join("AGENTS.md"))
        .chain(chain.into_iter().map(|d| d.join("AGENTS.md")))
        .collect();

    for (index, path) in paths.iter().enumerate() {
        let remaining = CONTENT_BUDGET_BYTES.saturating_sub(content.len());
        if remaining == 0 {
            // 终态审查修订：扫描全部剩余来源的存在性（仅元数据），首个后续
            // 路径缺失/异常不得掩蔽更后链上目录的普通来源
            if paths[index..].iter().any(|p| has_regular_source(p)) {
                notices.push(format!(
                    "上下文加载：规则正文预算（{CONTENT_BUDGET_BYTES} 字节）已耗尽，停止加载后续 AGENTS.md 来源"
                ));
            }
            break;
        }
        let file_size = match probe_source(&path) {
            SourceProbe::Regular(size) => size,
            SourceProbe::Missing => continue,
            SourceProbe::Symlink => {
                notices.push(skip_notice(&path, "符号链接"));
                continue;
            }
            SourceProbe::NotRegular => {
                notices.push(skip_notice(&path, "非普通文件"));
                continue;
            }
            SourceProbe::MetaError => {
                notices.push(skip_notice(&path, "读取失败"));
                continue;
            }
        };
        let Some(text) = load_source_text(&path, file_size, remaining + 3, &mut notices) else {
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            // 缺失/纯空白来源：正常路径，静默跳过
            continue;
        }
        if index > 0 {
            has_project = true;
        }
        let candidate = if content.is_empty() {
            trimmed.to_string()
        } else {
            format!("{content}{SEGMENT_SEPARATOR}{trimmed}")
        };
        match utf8_prefix(&candidate, CONTENT_BUDGET_BYTES) {
            BudgetPrefix::Within(within) => content = within.to_string(),
            BudgetPrefix::Truncated(prefix) => {
                content = prefix.to_string();
                notices.push(format!(
                    "上下文加载：{} 规则正文超出 {CONTENT_BUDGET_BYTES} 字节预算，已按 UTF-8 边界截断",
                    path.display()
                ));
                break;
            }
        }
    }

    let mut message = String::new();
    if !content.is_empty() {
        if has_project {
            message.push_str(&format!(
                "# AGENTS.md instructions for {}\n\n",
                sources.cwd.display()
            ));
        } else {
            message.push_str("# AGENTS.md instructions\n\n");
        }
        message.push_str("<INSTRUCTIONS>\n");
        message.push_str(&content);
        message.push_str("\n</INSTRUCTIONS>\n\n");
    }
    message.push_str(&environment_block(sources));
    BuiltContext { message, notices }
}

/// 识别完整上下文消息并返回规范化正文：先剥除至多一条完整替换声明及其约定
/// 分隔（声明行＋空行，仅逐字节前缀匹配时剥除），再识别完整结构（以
/// `# AGENTS.md instructions` 或 `<environment_context>` 开头、以
/// `</environment_context>` 结尾且含完整 INSTRUCTIONS 包装/块间结构）。
/// 仅相似标题或标记开头、结构不完整的普通用户文本不认定为上下文。
pub fn context_body(text: &str) -> Option<&str> {
    let stripped = text
        .strip_prefix(crate::prompt::CONTEXT_REPLACEMENT_DECLARATION)
        .and_then(|rest| rest.strip_prefix("\n\n"))
        .unwrap_or(text);
    let is_full = stripped.ends_with("</environment_context>")
        && ((stripped.starts_with("# AGENTS.md instructions")
            && stripped.contains("\n\n<INSTRUCTIONS>\n")
            && stripped.contains("\n</INSTRUCTIONS>\n\n<environment_context>\n"))
            || stripped.starts_with("<environment_context>\n  <cwd>"));
    is_full.then_some(stripped)
}

/// 向后扫描历史 user 文本项，取最后一条上下文消息的规范化正文（去重比较
/// 基线；resume 后自动恢复，零新增持久化状态）。
pub fn last_context_body(history: &[HistoryItem]) -> Option<&str> {
    history.iter().rev().find_map(|item| match item {
        HistoryItem::Text(HistoryMessage { role, content }) if *role == HistoryRole::User => {
            context_body(content)
        }
        _ => None,
    })
}
