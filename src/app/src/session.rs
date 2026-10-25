//! 会话保存与恢复（007 规格）：默认自动追加＋可选手动快照。
//! 序列化表征与追加/提交点语义参照 Codex rollout crate（固定提交
//! 2fdf047c9631c9ed01a31b62efb7891718a931a8：`recorder.rs` 的 `write_line`＝
//! 行序列化＋`write_all`＋O_APPEND、无 fsync；加载跳坏行容差）与
//! `context_manager/normalize.rs` 的配对修复先例，按 007 §3/§6 裁决本地新写
//! （母本 Responses item 枚举/后台写者/时间戳行不适配本地契约）。
//!
//! 本模块只做会话文件读写与装载投影：不联网、不执行工具（坑001：恢复≠重放）。

use crate::catalog;
use crate::prompt::{HistoryItem, HistoryMessage, HistoryRole};
use base64::Engine as _;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// 单行（不含结尾换行）字节上限：读侧超限转有界丢弃、写侧停用不截断（§6-E）。
pub const LINE_LIMIT: usize = 64 * 1024 * 1024;

/// 会话子目录名（平铺，无母本的 YYYY/MM/DD 分层）。
pub const SESSIONS_DIR: &str = "sessions";

/// 首行 meta：仅 `{"v":1}`（§6-D：不绑定模型/配置/目录）。
pub const META_LINE: &str = "{\"v\":1}";

fn needs_v2(history: &[HistoryItem]) -> bool {
    history.iter().any(|item| matches!(item, HistoryItem::NativeAssistant { .. }))
}

fn meta_line(version: u64) -> &'static str {
    if version == 2 { "{\"v\":2}" } else { META_LINE }
}

/// 悬空调用合成结果的固定文案（§6-A：只表达结果未知，禁言未执行/取消/失败）。
pub const UNKNOWN_RESULT_ERROR: &str = "会话中断，该调用可能已执行，执行结果未知";

/// sessions 目录（user_dir 下，测试经 TCHAT_USER_DIR 隔离）。
pub fn sessions_dir() -> PathBuf {
    catalog::user_dir().join(SESSIONS_DIR)
}

/// 行上限检查（写侧共用：超限不截断——保真优先，§6-E）。
fn line_over_limit(line: &str) -> bool {
    line.len() - 1 > LINE_LIMIT
}

fn line_limit_error(what: &str) -> String {
    format!("{what}超过 64 MiB 行上限（保真优先，不截断）",)
}

/// 包含性围栏（方式同 `file:` 凭证围栏：canonicalize 双侧真实路径做前缀复核）。
fn ensure_within_dir(dir: &Path, path: &Path) -> Result<(), String> {
    if let (Ok(dir_real), Ok(path_real)) = (std::fs::canonicalize(dir), std::fs::canonicalize(path)) {
        if !path_real.starts_with(&dir_real) {
            return Err(format!("会话路径 {} 逃出 sessions 目录，已拒绝", path.display()));
        }
    }
    Ok(())
}

// ---------- 装载与修复 ----------

/// 文件尾形态（续写尾置处理的依据，§3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailState {
    /// 以 `\n` 结尾：就绪。
    Terminated,
    /// 末行完整 JSON 但无 `\n`（装载已接受）：续写前补恰一个换行。
    AcceptedNoNewline,
    /// 末行坏/截断/超限（装载已跳过）：续写前截到最后一个 `\n` 后。
    Bad,
}

/// 装载结果：修复后的历史投影＋计数（修复不回写磁盘，投影确定性，§3 规则 6）。
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub version: u64,
    pub history: Vec<HistoryItem>,
    pub skipped_lines: usize,
    /// 坏行行号（1 起，含 meta 行）。
    pub skipped_line_numbers: Vec<usize>,
    /// 修复处数：合成结果＋丢弃结果（孤儿/重复/交错）＋丢弃头部非 user 记录。
    pub repairs: usize,
    pub tail: TailState,
}

impl Loaded {
    /// 装载成功输出的单行摘要。
    pub fn summary(&self) -> String {
        format!(
            "恢复 {} 条（跳过 {} 行，修复 {} 处）",
            self.history.len(),
            self.skipped_lines,
            self.repairs
        )
    }
}

/// 有界行读：返回（是否以 `\n` 结束， 是否超限）。行内容（不含 `\n`）留在复用缓冲；
/// 超限（读满上限未见 `\n`）后以固定块丢弃至下一个 `\n` 或 EOF，缓冲恒有界。
fn read_line_bounded<R: BufRead>(r: &mut R, buf: &mut Vec<u8>) -> std::io::Result<(bool, bool)> {
    buf.clear();
    loop {
        let available = r.fill_buf()?;
        if available.is_empty() {
            return Ok((false, false));
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            r.consume(pos + 1);
            return Ok((true, buf.len() > LINE_LIMIT));
        }
        buf.extend_from_slice(available);
        let n = available.len();
        r.consume(n);
        if buf.len() > LINE_LIMIT {
            loop {
                let n = match r.fill_buf()? {
                    [] => return Ok((false, true)),
                    available => match available.iter().position(|&b| b == b'\n') {
                        Some(pos) => {
                            r.consume(pos + 1);
                            return Ok((true, true));
                        }
                        None => available.len(),
                    },
                };
                r.consume(n);
            }
        }
    }
}

/// 装载解析后的行级条目：Bad 是配对修复的分组边界（被拒批仍是边界，§6-A）。
enum Entry {
    Item(HistoryItem),
    Bad,
}

/// 019 §5.1：`tool_result.image` 载荷校验（无图片字段的记录恒通过）。
/// 只接受 `data:image/(png|jpeg|gif|webp);base64,` 形态、可解码 base64、
/// detail 恰为 `high`、解码字节不超过准备后图片上限的本地图片 data URL。
fn image_payload_ok(item: &HistoryItem) -> bool {
    let item = item.portable();
    let HistoryItem::ToolResult { image: Some(img), .. } = item else {
        return user_images_payload_ok(item);
    };
    if img.detail != "high" {
        return false;
    }
    valid_local_image_data_url(&img.image_url)
}

/// 020 §2.2：本地图片 data URL 校验（用户附件与工具载荷共用）——
/// `data:image/(png|jpeg|gif|webp);base64,` 形态、可解码、不超过准备后上限。
fn valid_local_image_data_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("data:") else {
        return false; // 远程 URL 或其他前缀
    };
    let Some((meta, b64)) = rest.split_once(',') else {
        return false;
    };
    let parts: Vec<&str> = meta.split(';').collect();
    if !parts.iter().any(|p| p.eq_ignore_ascii_case("base64")) {
        return false;
    }
    if !matches!(parts[0], "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
        return false;
    }
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(bytes) => bytes.len() <= crate::view_image::MAX_PREPARED_IMAGE_BYTES,
        Err(_) => false,
    }
}

/// 020 §2.2：用户消息图片字段校验（无图/无标记恒通过）：
/// - 仅 User 角色可携带 images／非空 text_elements；问询来源标记
///   （`user_input_event`）与图片互斥（020 §2.4：问询回答不带附件）；
/// - images 每项为有界有效的本地图片 data URL；
/// - text_elements 区间是 content 的有效 UTF-8 字节边界、按序不重叠；
///   区间文字恰为 `[Image #N]`，N 指向存在的图片（1..=len）且不重复绑定。
fn user_images_payload_ok(item: &HistoryItem) -> bool {
    let HistoryItem::Text(m) = item else { return true };
    let images = m.images.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);
    if images.is_empty() && m.text_elements.is_empty() {
        return true;
    }
    if m.role != HistoryRole::User {
        return false; // 仅用户消息可携带图片附件
    }
    if m.user_input_event.is_some() && !images.is_empty() {
        return false; // 问询回答记录不带附件（020 §2.4）
    }
    if !images.iter().all(|url| valid_local_image_data_url(url)) {
        return false;
    }
    let mut last_end = 0usize;
    let mut bound = std::collections::HashSet::new();
    for el in &m.text_elements {
        let (start, end) = (el.byte_range.start, el.byte_range.end);
        if start < last_end || end > m.content.len() || start > end {
            return false; // 重叠/越界/乱序
        }
        if !m.content.is_char_boundary(start) || !m.content.is_char_boundary(end) {
            return false; // 必须是有效 UTF-8 字节边界
        }
        let marked = &m.content[start..end];
        let Some(n) = marked
            .strip_prefix("[Image #")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|n| n.parse::<usize>().ok())
        else {
            return false; // 标记文字形态
        };
        if n == 0 || n > images.len() || !bound.insert(n) {
            return false; // 越界或重复绑定
        }
        last_end = end;
    }
    true
}

/// 批内/跨批 id 校验：同一 id 在行内或更早批出现 → 该行按坏行（§3 加载规则 3）。
fn batch_line_ok(item: &HistoryItem, seen: &mut HashSet<String>) -> bool {
    if let HistoryItem::NativeAssistant { message, .. } = item {
        if !matches!(message.as_ref(), HistoryItem::AssistantToolCalls { .. }
            | HistoryItem::Text(HistoryMessage { role: HistoryRole::Assistant, .. })) {
            return false;
        }
        return batch_line_ok(message, seen);
    }
    let HistoryItem::AssistantToolCalls { tool_calls, .. } = item else {
        return true;
    };
    let mut local = HashSet::new();
    for call in tool_calls {
        if !local.insert(call.id.as_str()) || seen.contains(call.id.as_str()) {
            return false;
        }
    }
    for call in tool_calls {
        seen.insert(call.id.clone());
    }
    true
}

/// 从任意读取器装载（流式单遍：逐行读→解析→入临时 Vec）。
/// 坏 meta 或未知 v 是唯一整文件拒绝情形；行损坏跳过＋计数＋行号；
/// 配对修复只合成/丢弃记录文本，绝不重放工具（§3 加载规则 1–5）。
pub fn load_from_reader<R: Read>(reader: R) -> Result<Loaded, String> {
    let mut reader = BufReader::with_capacity(64 * 1024, reader);
    let mut line_buf: Vec<u8> = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut skipped = 0usize;
    let mut skipped_line_numbers = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut line_no = 0usize;
    // 尾态追踪：最后处理的一行是否带 \n、是否被接受
    let mut last_had_newline = true;
    let mut last_accepted = true;
    let mut meta_pending = true;
    let mut version = 1;

    loop {
        let (had_newline, over_limit) = read_line_bounded(&mut reader, &mut line_buf)
            .map_err(|e| format!("读取会话文件失败：{e}"))?;
        if line_buf.is_empty() && !had_newline {
            break; // EOF：无更多字节
        }
        line_no += 1;
        if meta_pending {
            // meta 门：v==1/2 通过（多余字段忽略）；坏 meta/未知 v 整文件拒绝
            let parsed_version = serde_json::from_slice::<serde_json::Value>(&line_buf)
                    .ok()
                    .and_then(|v| v.get("v").and_then(|v| v.as_u64()));
            let meta_ok = !over_limit && matches!(parsed_version, Some(1 | 2));
            if !meta_ok {
                return Err(format!("会话首行 meta 无效（期望 {META_LINE}）：整文件拒绝"));
            }
            version = parsed_version.expect("已确认支持的版本");
            meta_pending = false;
            last_had_newline = had_newline;
            last_accepted = true;
            if !had_newline {
                break; // meta 即末行
            }
            continue;
        }
        let parsed: Option<HistoryItem> = if over_limit {
            None
        } else {
            serde_json::from_slice::<HistoryItem>(&line_buf).ok()
        };
        if version == 1 && parsed.as_ref().is_some_and(|item| matches!(item, HistoryItem::NativeAssistant { .. })) {
            return Err("V1 会话含原生状态，拒绝按旧格式装载".into());
        }
        // 019 §5.1：图片载荷校验——只接受有界且有效的本地图片 data URL；
        // 远程 URL、非法 base64、错误 MIME、错误 detail 或超限额按坏行处理
        //（配对修复补未知结果；不为校验加载原路径或访问网络）。
        match parsed.filter(|item| batch_line_ok(item, &mut seen) && image_payload_ok(item)) {
            Some(item) => {
                entries.push(Entry::Item(item));
                last_accepted = true;
            }
            None => {
                entries.push(Entry::Bad);
                skipped += 1;
                skipped_line_numbers.push(line_no);
                last_accepted = false;
            }
        }
        last_had_newline = had_newline;
        if !had_newline {
            break;
        }
    }
    if meta_pending {
        return Err("会话文件为空或缺少 meta 行".into());
    }

    let (repairs, mut history) = repair_pairs(entries);
    if version == 2 && skipped > 0 {
        // 损坏行可能就是必要签名，不能像普通文本一样丢掉后继续请求。
        // 保留可显示历史，并把不可续接事实持久保存，禁止恢复时补执行工具。
        history.push(HistoryItem::NativeAssistant {
            message: Box::new(HistoryItem::Text(HistoryMessage {
                role: HistoryRole::Assistant, content: String::new(),
                user_input_event: None, images: None, text_elements: Vec::new(),
            })), state: None,
        });
    }
    let tail = if last_had_newline {
        TailState::Terminated
    } else if last_accepted {
        TailState::AcceptedNoNewline
    } else {
        TailState::Bad
    };
    Ok(Loaded { version, history, skipped_lines: skipped, skipped_line_numbers, repairs, tail })
}

/// 结束开放批：未获结果的调用合成通用未知错误结果（插在该批已保留结果之后）。
fn finalize_open_batch(
    out: &mut Vec<HistoryItem>,
    open: &mut Option<Vec<(String, bool)>>,
    repairs: &mut usize,
) {
    if let Some(calls) = open.take() {
        for (id, filled) in calls {
            if !filled {
                out.push(HistoryItem::ToolResult {
                    image: None,
                    tool_call_id: id,
                    content: format!("{{\"error\":\"{UNKNOWN_RESULT_ERROR}\"}}"),
                    display: None,
                });
                *repairs += 1;
            }
        }
    }
}

/// 配对修复（§3 加载规则 4，对条目单遍处理）：每个批（含被拒判坏的行）划定一段
/// 紧随的结果区，结果只归属当前批、不得归并前批；每个调用最多保留第一条匹配
/// 结果；孤儿/重复/交错丢弃＋计数；修复后头部非 user 记录丢弃至首个 user。
fn repair_pairs(entries: Vec<Entry>) -> (usize, Vec<HistoryItem>) {
    let mut repairs = 0usize;
    let mut out: Vec<HistoryItem> = Vec::new();
    // 当前开放批的调用填充状态：id → 是否已获结果
    let mut open: Option<Vec<(String, bool)>> = None;

    for entry in entries {
        match entry {
            Entry::Bad => finalize_open_batch(&mut out, &mut open, &mut repairs),
            Entry::Item(item @ HistoryItem::NativeAssistant { .. }) => {
                finalize_open_batch(&mut out, &mut open, &mut repairs);
                if let HistoryItem::AssistantToolCalls { tool_calls, .. } = item.portable() {
                    open = Some(tool_calls.iter().map(|c| (c.id.clone(), false)).collect());
                }
                out.push(item);
            }
            Entry::Item(HistoryItem::Text(m)) => {
                finalize_open_batch(&mut out, &mut open, &mut repairs);
                out.push(HistoryItem::Text(m));
            }
            Entry::Item(HistoryItem::AssistantToolCalls { tool_calls, content }) => {
                finalize_open_batch(&mut out, &mut open, &mut repairs);
                open = Some(tool_calls.iter().map(|c| (c.id.clone(), false)).collect());
                out.push(HistoryItem::AssistantToolCalls { tool_calls, content });
            }
            Entry::Item(HistoryItem::ToolResult { tool_call_id, content, display, image }) => {
                let keep = match &mut open {
                    None => false, // 无所属批：孤儿
                    Some(calls) => match calls.iter_mut().find(|(id, _)| *id == tool_call_id) {
                        Some(slot @ (_, false)) => {
                            slot.1 = true; // 首条匹配：保留
                            true
                        }
                        Some(_) => false, // 同 id 后续结果：重复
                        None => false, // 不属于当前批：跨批交错
                    },
                };
                if keep {
                    out.push(HistoryItem::ToolResult { tool_call_id, content, display, image });
                } else {
                    repairs += 1;
                }
            }
        }
    }
    finalize_open_batch(&mut out, &mut open, &mut repairs);

    // 头部非 user 丢弃至首个 user（§6-F）；全非 user → 空会话
    let first_user = out.iter().position(|h| {
        matches!(h, HistoryItem::Text(HistoryMessage { role: HistoryRole::User, .. }))
    });
    let drop_upto = first_user.unwrap_or(out.len());
    repairs += drop_upto;
    out.drain(..drop_upto);
    (repairs, out)
}

/// 只读装载一个会话文件（快照/降级路径：文件字节零修改）。
pub fn load_session_file(path: &Path) -> Result<Loaded, String> {
    let file = File::open(path).map_err(|e| format!("打开会话 {} 失败：{e}", path.display()))?;
    load_from_reader(file)
}

// ---------- 名称与路径 ----------

/// 通用名称校验：`^[A-Za-z0-9._-]{1,64}$`（/resume 有名与 /save 共用）。
pub fn validate_session_name(name: &str) -> Result<(), String> {
    let count = name.chars().count();
    if count == 0 || count > 64 {
        return Err("会话名称长度须为 1–64 个字符".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err("会话名称只能包含字母、数字与 . _ -".into());
    }
    Ok(())
}

/// 快照名称：额外保留 `auto-` 前缀（§1：自动会话命名空间）。
pub fn validate_snapshot_name(name: &str) -> Result<(), String> {
    validate_session_name(name)?;
    if name.starts_with("auto-") {
        return Err("快照名称不能以 auto- 开头（保留给自动会话）".into());
    }
    Ok(())
}

/// 会话文件路径＋包含性复核（方式同 `file:` 凭证围栏：canonicalize 后包含性，
/// 名称规则之外的路径一律拒绝）。
fn session_file_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = dir.join(format!("{name}.jsonl"));
    ensure_within_dir(dir, &path)?;
    Ok(path)
}

// ---------- 列表与最近 auto ----------

/// 列出会话（readdir＋stat，不打开任何会话正文）：名称（stem）＋文件字节数，按名称排序。
pub fn list_sessions(dir: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        let Some(stem) = name.strip_suffix(".jsonl") else { continue };
        if let Ok(meta) = entry.metadata() {
            out.push((stem.to_string(), meta.len()));
        }
    }
    out.sort();
    out
}

/// 字典序最大的 auto 会话名（跨时间戳＝时间序最新；同秒撞名 `-N` 候选间不保证
/// 字典序递增——§7 已知限制，`-10` 字典序小于 `-9`）。
pub fn latest_auto(dir: &Path) -> Option<String> {
    list_sessions(dir)
        .into_iter()
        .map(|(stem, _)| stem)
        .filter(|stem| stem.starts_with("auto-"))
        .max()
}

// ---------- auto 命名提供者（可注入） ----------

/// UTC 基本格式时间戳 `YYYYMMDDTHHMMSS`（字典序＝时间序；无 chrono 依赖，
/// Howard Hinnant civil_from_days 算法）。
fn utc_basic_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (hh, mm, ss) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}")
}

fn valid_ts_shape(s: &str) -> bool {
    s.len() == 15
        && s.as_bytes()[8] == b'T'
        && s.bytes().enumerate().all(|(i, b)| i == 8 || b.is_ascii_digit())
}

/// `auto-<ts>-<pid>.jsonl`：时间戳与 pid 经可注入提供者（测试固定；生产取真实值）。
/// 命名不提供互斥——跨进程同文件写入由独占锁处理（坑009）。
fn auto_file_name() -> String {
    let ts = std::env::var("TCHAT_SESSION_TS")
        .ok()
        .filter(|s| valid_ts_shape(s))
        .unwrap_or_else(utc_basic_now);
    let pid = std::env::var("TCHAT_SESSION_PID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or_else(std::process::id);
    format!("auto-{ts}-{pid}.jsonl")
}

// ---------- 独占锁（§6-I） ----------

enum LockOutcome {
    Acquired,
    Busy,
    Failed(String),
}

/// 对数据文件本身的 `flock(LOCK_EX|LOCK_NB)`：进程退出/崩溃自动释放，
/// 无需母本的锁文件与协调锁清理。
fn try_flock(file: &File) -> LockOutcome {
    let r = unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(file), libc::LOCK_EX | libc::LOCK_NB) };
    if r == 0 {
        return LockOutcome::Acquired;
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        // Linux 上 EWOULDBLOCK 与 EAGAIN 同值；两条都列以覆盖值不同的平台
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => LockOutcome::Busy,
        _ => LockOutcome::Failed(err.to_string()),
    }
}

// ---------- 手动快照（/save） ----------

/// tmp＋rename 原子替换（`catalog.rs write_atomic` 模式最小适配：独立 tmp 0600、
/// 流式逐记录写入、失败仅清 tmp——保存失败不得损坏旧快照）。返回记录条数。
pub fn save_snapshot(dir: &Path, name: &str, history: &[HistoryItem]) -> Result<usize, String> {
    validate_snapshot_name(name)?;
    catalog::ensure_private_dir(dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let final_path = session_file_path(dir, name)?;
    // 流式逐记录写 tmp（不构建全量 String）；行上限逐条检查，超限整体拒绝
    static TMP_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let tmp = dir.join(format!(
        "{name}.jsonl.tmp.{}.{}.{}",
        std::process::id(),
        nanos,
        seq
    ));
    let result = (|| -> Result<usize, String> {
        #[cfg(unix)]
        let mut f = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?
        };
        #[cfg(not(unix))]
        let mut f =
            std::fs::File::create(&tmp).map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
        f.write_all(format!("{}\n", meta_line(if needs_v2(history) { 2 } else { 1 })).as_bytes())
            .map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
        for item in history {
            let line = record_line(item);
            if line_over_limit(&line) {
                return Err(format!("{}，快照已拒绝", line_limit_error("记录序列化后")));
            }
            f.write_all(line.as_bytes())
                .map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
        }
        std::fs::rename(&tmp, &final_path).map_err(|e| format!("替换 {} 失败：{e}", final_path.display()))?;
        Ok(history.len())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

// ---------- 会话写入器 ----------

/// 记录序列化行（含结尾 `\n`）。
fn record_line(item: &HistoryItem) -> String {
    let mut line =
        serde_json::to_string(item).expect("HistoryItem 序列化不可失败（纯数据类型）");
    line.push('\n');
    line
}

enum State {
    /// 尚无 auto 目标：本运行首次需要追加落盘时创建（恢复快照/降级后亦然）。
    Idle,
    /// 追加目标：打开后持锁复用的同一文件描述符。
    Writing { _path: PathBuf, file: File },
    /// 停用：此后仅内存（行超限/截回失败/创建失败）。
    Disabled,
}

/// `/resume` 结果。
#[derive(Debug)]
pub struct ResumeInfo {
    pub loaded: Loaded,
    /// 获锁失败降级只读（原文件零修改）。
    pub degraded: bool,
}

/// auto 会话写入器：单写者、单 O_APPEND 文件；提交点＝完整行（含 `\n`）
/// 一次 `write_all` 成功；半写失败截回 `durable_len`；游标 `persisted_upto`
/// 只在提交后推进（§3 自动保存语义）。内存历史本身即待写缓冲，无重复缓存。
pub struct Session {
    version: u64,
    /// 增量游标：已检查过原生记录的历史前缀长度（256 MiB 档全量扫描会使
    /// 每条追加 O(n)，见 session_perf 回归）。
    native_seen: usize,
    dir: PathBuf,
    state: State,
    persisted_upto: usize,
    durable_len: u64,
    /// 写失败提示只打一次（同一失败串流），补写成功后复位。
    error_reported: bool,
}

impl Session {
    pub fn new(dir: PathBuf) -> Self {
        Session {
            version: 1,
            native_seen: 0,
            dir,
            state: State::Idle,
            persisted_upto: 0,
            durable_len: 0,
            error_reported: false,
        }
    }

    /// 每条记录入史的同时调用：确保目标存在（首次追加时创建，完整上下文开篇），
    /// 然后按序提交整个待写段。返回需要打印一次的错误提示（正常为 None）。
    pub fn on_history(&mut self, history: &[HistoryItem]) -> Option<String> {
        // 只扫新增段：历史可能已达数百万条，全量扫描会把单条追加拖成 O(n)。
        if self.version == 1 {
            let from = self.native_seen.min(history.len());
            let has_native = history[from..]
                .iter()
                .any(|item| matches!(item, HistoryItem::NativeAssistant { .. }));
            self.native_seen = history.len();
            if has_native {
                self.prepare_native_model("native");
            }
        }
        if let Some(msg) = self.ensure_target(history) {
            return Some(msg);
        }
        match self.flush_attempt(history) {
            Some(msg) => {
                if self.error_reported {
                    None // 同一失败串流只提示一次，待补写
                } else {
                    self.error_reported = true;
                    Some(msg)
                }
            }
            None => None,
        }
    }

    /// 在新轮入史前选择格式，避免恢复 V1 后先把 user 写回旧文件。
    /// 只换追加目标，不立即建文件；旧文件和已提交前缀保持原样。
    pub fn prepare_native_model(&mut self, protocol: &str) {
        if protocol != "openai-chat" && self.version == 1 && !matches!(self.state, State::Disabled) {
            self.version = 2;
            self.state = State::Idle;
            self.persisted_upto = 0;
            self.durable_len = 0;
            self.error_reported = false;
        }
    }

    /// 退出排空：同一补写逻辑执行一次；成功静默，失败返回明确错误——
    /// 排空是最后机会，不受"追加期打印一次"纪律影响。未产生新记录即退出时
    /// 不创建文件（恢复后直接退出亦然）。
    pub fn flush_final(&mut self, history: &[HistoryItem]) -> Result<(), String> {
        self.flush_attempt(history).map_or(Ok(()), Err)
    }

    /// 013-M7：当前追加目标的会话 ID（auto 文件 stem；threads 登记的
    /// thread_id 同源）。`Idle`/`Disabled` 无追加目标——返回 `None`，
    /// 调用方不得据此登记或触发（会话尚未有可写入事实）。
    pub fn current_target_stem(&self) -> Option<String> {
        match &self.state {
            State::Writing { _path, .. } => _path
                .file_stem()?
                .to_str()
                .map(std::borrow::ToOwned::to_owned),
            _ => None,
        }
    }

    /// 013-M7：已成功持久化的记录条数（写失败滞留待写段时不推进；恢复/
    /// 切换目标时重置为装载结果）。threads 登记以 `(stem, 该值)` 变化检测
    /// 「确有新持久化事实」，不把未成功落盘的数据登记成已更新版本。
    pub fn durable_history_len(&self) -> usize {
        self.persisted_upto
    }

    /// 恢复会话：`None`＝最近 auto；`Some(auto-*)`＝独占锁续写路径（打开即锁→
    /// 持锁装载→尾置处理→游标初始化，§6-I）；`Some(其他)`＝手动快照只读装载。
    /// 失败时当前状态不变。调用方前置：切换目标前应先排空当前待写段
    /// （`flush_final`），否则成功切换会放弃旧目标的未落盘记录（CLI 的
    /// cmd_resume 已按此排空，失败即中止恢复）。
    pub fn resume(&mut self, name: Option<&str>) -> Result<ResumeInfo, String> {
        let (stem, path) = match name {
            Some(n) => {
                validate_session_name(n)?;
                (n.to_string(), session_file_path(&self.dir, n)?)
            }
            None => {
                let latest = latest_auto(&self.dir)
                    .ok_or_else(|| "没有可恢复的自动会话（sessions 目录中无 auto- 文件）".to_string())?;
                let p = session_file_path(&self.dir, &latest)?;
                (latest, p)
            }
        };
        if !stem.starts_with("auto-") {
            // 手动快照：只读装载（快照文件字节不变），本运行新记录写新 auto 文件
            let loaded = load_session_file(&path)?;
            self.version = loaded.version;
            self.native_seen = loaded.history.len();
            self.state = State::Idle;
            self.persisted_upto = 0;
            self.durable_len = 0;
            self.error_reported = false;
            return Ok(ResumeInfo { loaded, degraded: false });
        }
        // 同名恢复（2026-10-10 第四批整改）：当前写者持锁同一文件时，复用已持锁
        // 描述符直接重装载——不释放再重抢。释放与重抢之间存在竞争窗口（第三方可
        // 在窗口内获锁，把自恢复挤成降级），复用让窗口不存在，锁全程未离开本写者。
        // 仅双侧 canonicalize 均成功且相等才判同名（双侧失败不等价于同名）。
        // 失败时当前状态不变（仍持锁持游标，调用方内存历史未动）。
        let same_file = match &self.state {
            State::Writing { _path, .. } => match (std::fs::canonicalize(_path), std::fs::canonicalize(&path)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            },
            _ => false,
        };
        if same_file {
            let State::Writing { file, .. } = &mut self.state else { unreachable!() };
            use std::io::Seek;
            file.rewind()
                .map_err(|e| format!("会话 {} 重读定位失败：{e}", path.display()))?;
            let file_len = file
                .metadata()
                .map_err(|e| format!("读取会话 {} 元数据失败：{e}", path.display()))?
                .len();
            let loaded = load_from_reader(&mut *file)?;
            let durable_len = match loaded.tail {
                TailState::Terminated => file_len,
                TailState::AcceptedNoNewline => {
                    file.write_all(b"\n")
                        .map_err(|e| format!("会话 {} 尾置处理失败：{e}", path.display()))?;
                    file_len + 1
                }
                TailState::Bad => truncate_bad_tail(file, file_len)
                    .map_err(|e| format!("会话 {} 尾置处理失败：{e}", path.display()))?,
            };
            self.persisted_upto = loaded.history.len();
            self.version = loaded.version;
            self.native_seen = loaded.history.len();
            self.durable_len = durable_len;
            self.error_reported = false;
            return Ok(ResumeInfo { loaded, degraded: false });
        }
        // auto 续写：打开（读写＋追加）→立即取锁→同一 fd 持锁装载与尾置（坑012）
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .read(true)
            .open(&path)
            .map_err(|e| format!("打开会话 {} 失败：{e}", path.display()))?;
        let lock = try_flock(&file);
        match lock {
            LockOutcome::Acquired => {
                let file_len = file
                    .metadata()
                    .map_err(|e| format!("读取会话 {} 元数据失败：{e}", path.display()))?
                    .len();
                let loaded = load_from_reader(&file)?;
                let durable_len = match loaded.tail {
                    TailState::Terminated => file_len,
                    TailState::AcceptedNoNewline => {
                        // 尾行完整 JSON 缺 \n：补恰一个换行（保留该已提交记录）
                        file.write_all(b"\n")
                            .map_err(|e| format!("会话 {} 尾置处理失败：{e}", path.display()))?;
                        file_len + 1
                    }
                    TailState::Bad => truncate_bad_tail(&mut file, file_len)
                        .map_err(|e| format!("会话 {} 尾置处理失败：{e}", path.display()))?,
                };
                // 合成记录视为虚拟持久化：游标＝装载后历史长度，补写不重写它们
                self.persisted_upto = loaded.history.len();
                self.version = loaded.version;
                self.native_seen = loaded.history.len();
                self.durable_len = durable_len;
                self.error_reported = false;
                self.state = State::Writing { _path: path, file };
                Ok(ResumeInfo { loaded, degraded: false })
            }
            LockOutcome::Busy => {
                // 降级只读：原文件零修改（不补换行、不截断、不追加）；
                // 装载为获锁失败时点快照，写者进行中半行按坏行跳过
                drop(file);
                let loaded = load_session_file(&path)?;
                self.version = loaded.version;
                self.native_seen = loaded.history.len();
                self.state = State::Idle;
                self.persisted_upto = 0;
                self.durable_len = 0;
                self.error_reported = false;
                Ok(ResumeInfo { loaded, degraded: true })
            }
            LockOutcome::Failed(e) => Err(format!("会话 {} 加锁失败：{e}", path.display())),
        }
    }

    /// Idle→Writing：首次需要追加落盘时创建文件，写入 meta＋当时全部内存历史
    /// （完整上下文开篇，坑011），随后游标＝该长度。失败→停用＋错误一次。
    fn ensure_target(&mut self, history: &[HistoryItem]) -> Option<String> {
        if !matches!(self.state, State::Idle) {
            return None;
        }
        match self.create_auto_file(history) {
            Ok((path, file, bytes)) => {
                self.state = State::Writing { _path: path, file };
                self.durable_len = bytes;
                self.persisted_upto = history.len();
                None
            }
            Err(msg) => {
                self.state = State::Disabled;
                Some(msg)
            }
        }
    }

    fn create_auto_file(&self, history: &[HistoryItem]) -> Result<(PathBuf, File, u64), String> {
        catalog::ensure_private_dir(&self.dir)
            .map_err(|e| format!("自动会话目录初始化失败：{e}"))?;
        let base = auto_file_name();
        for attempt in 0..50u32 {
            // 撞名候选：base 已含 auto- 前缀，仅追加 -N（§7：重复拼前缀会产生
            // auto-auto-*，字典序压过一切 auto-<数字> 文件，破坏无参恢复的时间序）
            let name = if attempt == 0 {
                base.clone()
            } else {
                format!("{}-{attempt}.jsonl", base.strip_suffix(".jsonl").unwrap_or(&base))
            };
            let path = self.dir.join(&name);
            let opened = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    std::fs::OpenOptions::new()
                        .append(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)
                }
                #[cfg(not(unix))]
                {
                    std::fs::OpenOptions::new().append(true).create_new(true).open(&path)
                }
            };
            let mut file = match opened {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("创建自动会话文件失败：{e}")),
            };
            // 新建即锁（§6-I："成为续写目标时先取锁"涵盖新建路径——本文件此后
            // 会被本运行持续追加，正是续写目标；坑009：命名不提供互斥）。
            // Busy＝该名字已被并发恢复者抢先持锁（文件刚创建仍 0 字节，抢锁者
            // 装载空文件会得到明确错误）→ 视同名字冲突换下一候选；其余错误停用
            match try_flock(&file) {
                LockOutcome::Acquired => {}
                LockOutcome::Busy => continue,
                LockOutcome::Failed(e) => return Err(format!("自动会话文件加锁失败：{e}")),
            }
            // 包含性复核（防线同 file: 凭证围栏）
            if ensure_within_dir(&self.dir, &path).is_err() {
                let _ = std::fs::remove_file(&path);
                return Err("自动会话路径逃出 sessions 目录，已停用自动保存".into());
            }
            let mut bytes = 0u64;
            let outcome = (|| -> Result<(), String> {
                let meta = format!("{}\n", meta_line(self.version));
                file.write_all(meta.as_bytes())
                    .map_err(|e| format!("自动会话写入失败：{e}"))?;
                bytes += meta.len() as u64;
                for item in history {
                    let line = record_line(item);
                    if line_over_limit(&line) {
                        return Err(format!("{}，自动保存已停用", line_limit_error("记录序列化后")));
                    }
                    file.write_all(line.as_bytes())
                        .map_err(|e| format!("自动会话写入失败：{e}"))?;
                    bytes += line.len() as u64;
                }
                Ok(())
            })();
            match outcome {
                Ok(()) => return Ok((path, file, bytes)),
                Err(msg) => {
                    // 创建即失败：无已提交记录可保，清掉残档后停用
                    let _ = std::fs::remove_file(&path);
                    return Err(msg);
                }
            }
        }
        Err("自动会话文件名持续冲突，自动保存已停用".into())
    }

    /// 按序提交待写段 `history[persisted_upto..]`：每行独立 write_all＋独立推进；
    /// 失败→截回 durable_len（截回成功＝游标不动待补写；截回失败＝停用）；
    /// 行超限→停用不截断。返回原始失败消息（打印一次纪律由调用方施加）。
    fn flush_attempt(&mut self, history: &[HistoryItem]) -> Option<String> {
        enum Step {
            Done,
            Pending(String),
            Disable(String),
        }
        if !matches!(self.state, State::Writing { .. }) {
            return None; // 停用后仅内存，不重复报错
        }
        let step = loop {
            if self.persisted_upto >= history.len() {
                break Step::Done;
            }
            let line = record_line(&history[self.persisted_upto]);
            if line_over_limit(&line) {
                break Step::Disable(format!("{}，自动保存已停用", line_limit_error("记录序列化后")));
            }
            let State::Writing { file, .. } = &mut self.state else {
                break Step::Done; // 停用后无待写段处理
            };
            match file.write_all(line.as_bytes()) {
                Ok(()) => {
                    self.durable_len += line.len() as u64;
                    self.persisted_upto += 1;
                    self.error_reported = false;
                }
                Err(e) => {
                    // write_all 失败可能已写部分字节：截回已提交长度
                    let msg = format!("自动会话追加失败（{e}）：待写记录保留在内存，稍后补写");
                    match file.set_len(self.durable_len) {
                        Ok(()) => break Step::Pending(msg),
                        Err(te) => break Step::Disable(format!(
                            "{msg}；截回失败（{te}），自动保存已停用，文件尾部半行将由下次装载按坏行跳过"
                        )),
                    }
                }
            }
        };
        match step {
            Step::Done => None,
            Step::Disable(msg) => {
                self.state = State::Disabled;
                Some(msg)
            }
            Step::Pending(msg) => Some(msg),
        }
    }
}

/// 坏尾截断：从文件尾向前块读定位最后一个 `\n`，截到其后（丢弃从未提交的字节，
/// 不触碰已提交记录）。
fn truncate_bad_tail(file: &mut File, file_len: u64) -> std::io::Result<u64> {
    use std::io::{Seek, SeekFrom};
    const BLOCK: usize = 4096;
    let mut pos = file_len;
    let mut buf = vec![0u8; BLOCK];
    while pos > 0 {
        let start = pos.saturating_sub(BLOCK as u64);
        let n = (pos - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buf[..n])?;
        if let Some(idx) = buf[..n].iter().rposition(|&b| b == b'\n') {
            let new_len = start + idx as u64 + 1;
            file.set_len(new_len)?;
            return Ok(new_len);
        }
        pos = start;
    }
    file.set_len(0)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_user(content: &str) -> HistoryItem {
        HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: content.to_string(),
            user_input_event: None,
        })
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tchat-trunc-fail-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 截回失败→停用（§3"半写失败处理"）：真实设备注入——`/dev/full` 的
    /// write 恒 ENOSPC、ftruncate 恒 EINVAL，经生产 on_history 路径真实触发
    /// "写失败→截回失败→停用"分支（本环境无法用 RLIMIT/chattr 注入，见
    /// logs/2026-10-10-007-implementation.md；此为真实字符设备 I/O 错误，
    /// 非状态预设或替身）。验证：明确报错、停用后不再尝试追加、内存对话
    /// 继续、停用留下的半行残档可被装载器按坏行处理。
    #[test]
    fn truncate_failure_disables_autosave_via_real_device() {
        let dir = temp_dir("dev");
        // Writing 态指向 /dev/full：前两条视为已提交（游标＝2）
        let dev = std::fs::OpenOptions::new()
            .append(true)
            .read(true)
            .open("/dev/full")
            .expect("本测试需要 /dev/full（Linux 标准字符设备）");
        let mut session = Session {
            version: 1,
            native_seen: 0,
            dir: dir.clone(),
            state: State::Writing { _path: PathBuf::from("/dev/full"), file: dev },
            persisted_upto: 2,
            durable_len: 1234,
            error_reported: false,
        };
        let mut history = vec![
            text_user("已提交一"),
            HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(), role: HistoryRole::Assistant, content: "已提交二".into(), user_input_event: None }),
            text_user("第三条写入将失败"),
        ];
        // 第三条：write_all ENOSPC → set_len EINVAL → 停用＋明确报错（一次）
        let notice = session.on_history(&history).expect("截回失败必须明确报错");
        assert!(
            notice.contains("截回失败") && notice.contains("自动保存已停用"),
            "报错须指明截回失败与停用：{notice}"
        );
        // 停用后：后续记录不再尝试追加（静默），内存对话照常继续
        for content in ["第四条", "第五条"] {
            history.push(text_user(content));
            assert_eq!(session.on_history(&history), None, "停用后不再重复报错/追加");
        }
        assert_eq!(history.len(), 5, "内存对话继续：历史照常增长");
        // 停用证据（行为性）：排空无操作返回 Ok——若未停用，flush 会再次
        // 触发 /dev/full 写失败并返回 Err
        assert_eq!(
            session.flush_final(&history),
            Ok(()),
            "停用态排空应为无操作；Err 说明仍处 Writing 态（未停用）"
        );

        // 停用留下的残档形态（常规文件上截回失败＝已提交行＋未提交半行）：
        // 装载器按坏行跳过、已提交记录完整装载
        let residue = dir.join("auto-residue-1.jsonl");
        let good = serde_json::to_string(&text_user("已提交")).unwrap();
        std::fs::write(
            &residue,
            format!("{META_LINE}\n{good}\n{{\"type\":\"text\",\"role\":\"ass"),
        )
        .unwrap();
        let loaded = load_session_file(&residue).unwrap();
        assert_eq!(loaded.history, vec![text_user("已提交")]);
        assert_eq!(loaded.skipped_lines, 1, "半行按坏行跳过");
        assert_eq!(loaded.tail, TailState::Bad);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
