//! 013-M4：全量 stage1 快照的 Markdown 镜像。
//!
//! 来源：openai/codex @ ce0323f2f2d3b25e82806cd555237da3db4e1897 (Apache-2.0)
//! codex-rs/memories/write/src/storage.rs（正文/命名/清理）、
//! lib.rs::ensure_layout、workspace.rs::remove_memory_symlinks、
//! phase2.rs::sync_phase2_workspace_inputs（先摘要后 raw 的次序）。
//! 适配：同步 I/O；Stage1Output → 显式元数据投影；全量 slice 不设选择上限；
//! tracing 告警 → 可注入诊断出口（终端净化复用 display）；文件写入复用
//! catalog::write_private_file（0700 目录/0600 文件，直接覆盖、不回滚）。
//! 不执行模型、不修改数据库/任务状态、不操作 Git 基线。

use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::Stage1OutputRow;

/// 上游 Stage1Output 的文件投影；原会话路径/工作目录由调用方显式提供。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage1FileInput {
    pub thread_id: String,
    pub source_updated_at: DateTime<Utc>,
    pub raw_memory: String,
    pub rollout_summary: String,
    pub rollout_slug: Option<String>,
    pub rollout_path: PathBuf,
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
}

impl Stage1FileInput {
    pub fn from_row(
        row: Stage1OutputRow,
        rollout_path: PathBuf,
        cwd: PathBuf,
        git_branch: Option<String>,
    ) -> io::Result<Self> {
        let source_updated_at = DateTime::from_timestamp(row.source_updated_at, 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "stage1 源时间戳超出范围"))?;
        Ok(Self {
            thread_id: row.thread_id,
            source_updated_at,
            raw_memory: row.raw_memory,
            rollout_summary: row.rollout_summary,
            rollout_slug: row.rollout_slug,
            rollout_path,
            cwd,
            git_branch,
        })
    }
}

/// 输入必须是完整当前快照，按 thread_id 升序（MemoryStore 的快照读取已保证）。
/// 告警出口必须交给调用方显示/记录；单个旧件删除失败仍继续，不代表清理完毕。
/// I/O 错误返回；之前已发生的文件副作用保留。与数据库没有跨存储事务。
pub fn sync_stage1_snapshot(
    root: &Path,
    memories: &[Stage1FileInput],
    warnings: &mut dyn FnMut(&str),
) -> io::Result<()> {
    ensure_layout(root, warnings)?;
    let keep = memories.iter().map(rollout_summary_file_stem).collect::<HashSet<_>>();
    prune_rollout_summaries(root, &keep, warnings)?;
    for memory in memories {
        write_rollout_summary_for_thread(root, memory)?;
    }
    // 上游两个入口各自准备布局；第二次亦保留，raw 写入前不使用链接。
    ensure_layout(root, warnings)?;
    rebuild_raw_memories_file(root, memories)
}

fn rollout_summaries_dir(root: &Path) -> PathBuf { root.join("rollout_summaries") }
fn raw_memories_file(root: &Path) -> PathBuf { root.join("raw_memories.md") }

fn warn(warnings: &mut dyn FnMut(&str), message: String) {
    warnings(&crate::display::sanitize_for_terminal(&message));
}

pub(crate) fn ensure_layout(root: &Path, warnings: &mut dyn FnMut(&str)) -> io::Result<()> {
    super::init_memories_root(root).map_err(io::Error::other)?;
    remove_memory_symlinks(root, warnings)?;
    std::fs::create_dir_all(rollout_summaries_dir(root))
}

/// 不跟随链接；包括 dangling link，亦覆盖 skills/extensions/.git 内的链接。
/// 返回移除数量（M5：写后校验以「移除了链接」为无效输出的判定，上游
/// `workspace.rs::remove_memory_symlinks` 同款计数语义）。
pub(crate) fn remove_memory_symlinks(
    root: &Path,
    warnings: &mut dyn FnMut(&str),
) -> io::Result<usize> {
    let mut directories = vec![root.to_path_buf()];
    let mut removed = 0usize;
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::FileTypeExt;
                    if file_type.is_symlink_dir() { std::fs::remove_dir(&path)?; }
                    else { std::fs::remove_file(&path)?; }
                }
                #[cfg(not(windows))]
                std::fs::remove_file(&path)?;
                warn(warnings, format!("removed symbolic link from memory workspace: {}", path.display()));
                removed += 1;
            } else if file_type.is_dir() {
                directories.push(path);
            }
        }
    }
    Ok(removed)
}

fn write_memory_file(path: impl AsRef<Path>, body: String) -> io::Result<()> {
    crate::catalog::write_private_file(path.as_ref(), &body).map_err(io::Error::other)
}

fn prune_rollout_summaries(
    root: &Path,
    keep: &HashSet<String>,
    warnings: &mut dyn FnMut(&str),
) -> io::Result<()> {
    let dir = match std::fs::read_dir(rollout_summaries_dir(root)) {
        Ok(dir) => dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in dir {
        let path = entry?.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else { continue; };
        let Some(stem) = file_name.strip_suffix(".md") else { continue; };
        if !keep.contains(stem) {
            if let Err(err) = std::fs::remove_file(&path) {
                if err.kind() != io::ErrorKind::NotFound {
                    warn(warnings, format!("failed pruning outdated rollout summary {}: {err}", path.display()));
                }
            }
        }
    }
    Ok(())
}

fn rebuild_raw_memories_file(
    root: &Path,
    memories: &[Stage1FileInput],
) -> std::io::Result<()> {
    let retained = memories;
    let mut body = String::from("# Raw Memories\n\n");

    if retained.is_empty() {
        body.push_str("No raw memories yet.\n");
        return write_memory_file(raw_memories_file(root), body);
    }

    body.push_str("Merged stage-1 raw memories (stable ascending thread-id order):\n\n");
    for memory in retained {
        writeln!(body, "## Thread `{}`", memory.thread_id).map_err(raw_memories_format_error)?;
        writeln!(
            body,
            "updated_at: {}",
            memory.source_updated_at.to_rfc3339()
        )
        .map_err(raw_memories_format_error)?;
        writeln!(body, "cwd: {}", memory.cwd.display()).map_err(raw_memories_format_error)?;
        writeln!(body, "rollout_path: {}", memory.rollout_path.display())
            .map_err(raw_memories_format_error)?;
        let rollout_summary_file = format!("{}.md", rollout_summary_file_stem(memory));
        writeln!(body, "rollout_summary_file: {rollout_summary_file}")
            .map_err(raw_memories_format_error)?;
        writeln!(body).map_err(raw_memories_format_error)?;
        body.push_str(memory.raw_memory.trim());
        body.push_str("\n\n");
    }

    write_memory_file(raw_memories_file(root), body)
}

fn write_rollout_summary_for_thread(
    root: &Path,
    memory: &Stage1FileInput,
) -> std::io::Result<()> {
    let file_stem = rollout_summary_file_stem(memory);
    let path = rollout_summaries_dir(root).join(format!("{file_stem}.md"));

    let mut body = String::new();
    writeln!(body, "thread_id: {}", memory.thread_id).map_err(rollout_summary_format_error)?;
    writeln!(
        body,
        "updated_at: {}",
        memory.source_updated_at.to_rfc3339()
    )
    .map_err(rollout_summary_format_error)?;
    writeln!(body, "rollout_path: {}", memory.rollout_path.display())
        .map_err(rollout_summary_format_error)?;
    writeln!(body, "cwd: {}", memory.cwd.display()).map_err(rollout_summary_format_error)?;
    if let Some(git_branch) = memory.git_branch.as_deref() {
        writeln!(body, "git_branch: {git_branch}").map_err(rollout_summary_format_error)?;
    }
    writeln!(body).map_err(rollout_summary_format_error)?;
    body.push_str(&memory.rollout_summary);
    body.push('\n');

    write_memory_file(path, body)
}

fn raw_memories_format_error(err: std::fmt::Error) -> std::io::Error {
    std::io::Error::other(format!("format raw memories: {err}"))
}

fn rollout_summary_format_error(err: std::fmt::Error) -> std::io::Error {
    std::io::Error::other(format!("format rollout summary: {err}"))
}

fn rollout_summary_file_stem(memory: &Stage1FileInput) -> String {
    rollout_summary_file_stem_from_parts(
        &memory.thread_id,
        memory.source_updated_at,
        memory.rollout_slug.as_deref(),
    )
}

fn rollout_summary_file_stem_from_parts(
    thread_id: &str,
    source_updated_at: chrono::DateTime<chrono::Utc>,
    rollout_slug: Option<&str>,
) -> String {
    const ROLLOUT_SLUG_MAX_LEN: usize = 60;
    const SHORT_HASH_ALPHABET: &[u8; 62] =
        b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const SHORT_HASH_SPACE: u32 = 14_776_336;

    let thread_id = thread_id.to_string();
    let (timestamp_fragment, short_hash_seed) = match Uuid::parse_str(&thread_id) {
        Ok(thread_uuid) => {
            let timestamp = thread_uuid
                .get_timestamp()
                .and_then(|uuid_timestamp| {
                    let (seconds, nanos) = uuid_timestamp.to_unix();
                    i64::try_from(seconds).ok().and_then(|secs| {
                        chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
                    })
                })
                .unwrap_or(source_updated_at);
            let short_hash_seed = (thread_uuid.as_u128() & 0xFFFF_FFFF) as u32;
            (
                timestamp.format("%Y-%m-%dT%H-%M-%S").to_string(),
                short_hash_seed,
            )
        }
        Err(_) => {
            let mut short_hash_seed = 0u32;
            for byte in thread_id.bytes() {
                short_hash_seed = short_hash_seed
                    .wrapping_mul(31)
                    .wrapping_add(u32::from(byte));
            }
            (
                source_updated_at.format("%Y-%m-%dT%H-%M-%S").to_string(),
                short_hash_seed,
            )
        }
    };
    let mut short_hash_value = short_hash_seed % SHORT_HASH_SPACE;
    let mut short_hash_chars = ['0'; 4];
    for idx in (0..short_hash_chars.len()).rev() {
        let alphabet_idx = (short_hash_value % SHORT_HASH_ALPHABET.len() as u32) as usize;
        short_hash_chars[idx] = SHORT_HASH_ALPHABET[alphabet_idx] as char;
        short_hash_value /= SHORT_HASH_ALPHABET.len() as u32;
    }
    let short_hash: String = short_hash_chars.iter().collect();
    let file_prefix = format!("{timestamp_fragment}-{short_hash}");

    let Some(raw_slug) = rollout_slug else {
        return file_prefix;
    };

    let mut slug = String::with_capacity(ROLLOUT_SLUG_MAX_LEN);
    for ch in raw_slug.chars() {
        if slug.len() >= ROLLOUT_SLUG_MAX_LEN {
            break;
        }

        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else {
            slug.push('_');
        }
    }

    while slug.ends_with('_') {
        slug.pop();
    }

    if slug.is_empty() {
        file_prefix
    } else {
        format!("{file_prefix}-{slug}")
    }
}
