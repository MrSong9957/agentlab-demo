//! 013-M5（D3，2026-10-18 用户批准）：合并轮受限文件工具
//! `mem_read`/`mem_write`/`mem_list`。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——`ext/memories/src/local.rs`（`resolve_scoped_path` 边界：
//! 拒绝绝对路径/`..` 越界/隐藏组件/符号链接组件/穿越非目录）、`local/path.rs`
//! （隐藏判定、排序枚举、symlink 拒绝）、`local/read.rs`（行号 1 起、
//! `max_lines`、token 预算截断）、`local/list.rs`（单层列目录、排序、游标
//! 分页）。常量取自 `ext/memories/src/lib.rs`（`DEFAULT_READ_MAX_TOKENS`
//! ＝20,000、`DEFAULT/MAX_LIST_RESULTS`＝2,000）与 `local.rs`
//! （`MAX_MEMORY_FILE_BYTES`＝1 MiB）。
//!
//! 本地扩展（D3 批准边界，2026-10-18）：`memories.sqlite`/
//! `memories.sqlite-journal` 与隐藏路径一并不向模型暴露——读取、写入、
//! 列表展示全部拒绝（上游库文件在记忆根外无此需求；任意层级同名即拒，
//! 保守边界）。`mem_write` 为批准边界定义的常规文件新建/覆盖（上游无
//! 合并轮写工具）。工具结果 JSON（本地适配，登记 013 §2.2）：读取/列表
//! 沿用上游响应字段（`path`/`start_line_number`/`content`/`truncated`；
//! `entries`/`next_cursor`）；写入带 `ok` 标志；失败回传单 `error` 字段
//! 结构——与项目工具错误面惯例一致。

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::memory::truncate::{truncate_text, TruncationPolicy};

/// 工具名（注册表恰为三项——合并轮白名单，D3）。
pub const MEMORY_TOOL_READ: &str = "mem_read";
pub const MEMORY_TOOL_WRITE: &str = "mem_write";
pub const MEMORY_TOOL_LIST: &str = "mem_list";

/// 单文件写入上限（上游 `MAX_MEMORY_FILE_BYTES`）。
pub const MAX_MEMORY_FILE_BYTES: usize = 1024 * 1024;
/// 读取默认 token 预算（上游 `DEFAULT_READ_MAX_TOKENS`）。
pub const DEFAULT_READ_MAX_TOKENS: usize = 20_000;
/// 列目录默认上限（上游 `DEFAULT_LIST_MAX_RESULTS`）。
pub const DEFAULT_LIST_MAX_RESULTS: usize = 2_000;
/// 列目录硬上限（上游 `MAX_LIST_RESULTS`）。
pub const MAX_LIST_RESULTS: usize = 2_000;

/// 不向模型暴露的文件名（D3 批准边界；任意层级同名即拒——保守边界）。
const EXCLUDED_FILE_NAMES: [&str; 2] = ["memories.sqlite", "memories.sqlite-journal"];

/// 三项工具定义（请求体 `tools` 数组元素；schema 结构与既有工具同母本：
/// required＋`additionalProperties:false`）。
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": MEMORY_TOOL_READ,
                "description": "Reads a regular file inside the memory root. Lines are 1-based; content is truncated to a token budget (default 20,000 tokens).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path relative to the memory root." },
                        "line_offset": { "type": "integer", "description": "1-based starting line; must be >= 1. Defaults to 1." },
                        "max_lines": { "type": "integer", "description": "Maximum number of lines to read; must be >= 1 when present." },
                        "max_tokens": { "type": "integer", "description": "Token budget for the returned content; 0 selects the default 20,000 tokens." }
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": MEMORY_TOOL_WRITE,
                "description": "Creates or overwrites a regular file inside the memory root (max 1 MiB per write). The parent directory must already exist.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path relative to the memory root." },
                        "content": { "type": "string", "description": "Full file content to write (overwrites)." }
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": MEMORY_TOOL_LIST,
                "description": "Lists one directory level inside the memory root, sorted; hidden entries, symlinks and the sqlite database are never shown.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path relative to the memory root; defaults to the root itself." },
                        "cursor": { "type": "string", "description": "Pagination cursor (a non-negative integer as string)." },
                        "max_results": { "type": "integer", "description": "Maximum entries to return; capped at 2,000. Defaults to 2,000." }
                    },
                    "additionalProperties": false
                }
            }
        }),
    ]
}

/// 取消后未执行调用的结果形状：已知 mem 工具按各自结构（写入带 `ok`
/// 标志）、其他返回 None（由调用方落单 `error` 通用结构）。
pub fn skipped_by_cancel_shape(name: &str) -> Option<String> {
    let message = crate::turn::SKIPPED_BY_CANCEL_MESSAGE;
    match name {
        MEMORY_TOOL_WRITE => Some(json!({ "ok": false, "error": message }).to_string()),
        MEMORY_TOOL_READ | MEMORY_TOOL_LIST => Some(json!({ "error": message }).to_string()),
        _ => None,
    }
}

/// 执行一个 mem 工具调用：`arguments` 为模型给出的 JSON 字符串；结果总是
/// 合法 JSON 字符串（成功载荷或失败结构），不经 shell、不触模型。
/// `expected_epoch`＝任务领取时的重置代数快照（不可变任务凭据）——
/// mem_write 据此校验（013-M7 收口：防旧任务借新代 prepare 恢复写入）。
pub fn execute(name: &str, arguments: &str, root: &Path, expected_epoch: u64) -> String {
    match name {
        MEMORY_TOOL_READ => match serde_json::from_str::<Value>(arguments) {
            Ok(v) => mem_read(root, &v),
            Err(e) => err_json(format!("工具参数不是有效 JSON：{e}")),
        },
        MEMORY_TOOL_WRITE => match serde_json::from_str::<Value>(arguments) {
            Ok(v) => mem_write(root, &v, expected_epoch),
            Err(e) => err_json(format!("工具参数不是有效 JSON：{e}")),
        },
        MEMORY_TOOL_LIST => match serde_json::from_str::<Value>(arguments) {
            Ok(v) => mem_list(root, &v),
            Err(e) => err_json(format!("工具参数不是有效 JSON：{e}")),
        },
        other => err_json(format!(
            "未知工具 {other}，可用：{MEMORY_TOOL_READ}、{MEMORY_TOOL_WRITE}、{MEMORY_TOOL_LIST}"
        )),
    }
}

fn err_json(message: String) -> String {
    json!({ "error": message }).to_string()
}

/// 参数对象解析：必须为 JSON 对象且键集恰为 `allowed` 的子集。
fn args_object(v: &Value, allowed: &[&str]) -> Result<Map<String, Value>, String> {
    let obj = v.as_object().ok_or("参数必须是对象")?;
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("未知参数：{key}"));
        }
    }
    Ok(obj.clone())
}

fn arg_str(obj: &Map<String, Value>, name: &str) -> Result<String, String> {
    match obj.get(name) {
        None | Some(Value::Null) => Err(format!("缺少必填参数 {name}")),
        Some(s) => s
            .as_str()
            .map(str::to_string)
            .ok_or(format!("{name} 必须是字符串")),
    }
}

/// 非负整数字段：缺失或 null 为 None；负数/浮点/字符串非法。
fn opt_uint(obj: &Map<String, Value>, name: &str) -> Result<Option<u64>, String> {
    match obj.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) if v.is_u64() => Ok(Some(v.as_u64().unwrap())),
        Some(_) => Err(format!("{name} 必须为非负整数")),
    }
}

/// 保守隐藏/排除名检查：组件以 `.` 开头或为排除文件名。
fn is_hidden_component(component: Component<'_>) -> bool {
    matches!(
        component,
        Component::Normal(name) if name.to_string_lossy().starts_with('.')
    )
}

fn is_excluded_component(component: Component<'_>) -> bool {
    matches!(
        component,
        Component::Normal(name)
            if EXCLUDED_FILE_NAMES.contains(&name.to_string_lossy().as_ref())
    )
}

fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

fn is_excluded_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| EXCLUDED_FILE_NAMES.contains(&name.to_string_lossy().as_ref()))
}

fn display_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn symlink_metadata_or_none(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取路径元数据 {} 失败：{e}", path.display())),
    }
}

/// 解析受限路径（上游 `resolve_scoped_path`＋D3 扩展）：相对路径、无越界、
/// 无隐藏/排除组件、无符号链接组件、不穿越非目录；尾部不存在＝合法（供
/// 新建写入）。`None`＝记忆根本身。
fn resolve_scoped_path(root: &Path, relative: Option<&str>) -> Result<PathBuf, String> {
    let Some(relative) = relative else {
        return Ok(root.to_path_buf());
    };
    let rel = Path::new(relative);
    if rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!("路径必须位于记忆根内：{relative}"));
    }
    if rel.components().any(is_hidden_component) {
        return Err(format!("隐藏路径不可访问：{relative}"));
    }
    if rel.components().any(is_excluded_component) {
        return Err(format!("路径不可访问：{relative}"));
    }
    if rel.as_os_str().is_empty() {
        return Err("路径不能为空".into());
    }

    let components = rel.components().collect::<Vec<_>>();
    let mut scoped = root.to_path_buf();
    for (idx, component) in components.iter().enumerate() {
        scoped.push(component.as_os_str());
        let Some(metadata) = symlink_metadata_or_none(&scoped)? else {
            // 中途不存在：余下组件原样拼接（上游同款——尾部缺失合法）
            for remaining in components.iter().skip(idx + 1) {
                scoped.push(remaining.as_os_str());
            }
            return Ok(scoped);
        };
        if metadata.file_type().is_symlink() {
            return Err(format!("路径不得包含符号链接：{relative}"));
        }
        if idx + 1 < components.len() && !metadata.is_dir() {
            return Err(format!("路径穿过非目录组件：{relative}"));
        }
    }
    Ok(scoped)
}

/// mem_read（上游 `local/read.rs::read`）。
fn mem_read(root: &Path, v: &Value) -> String {
    let run = || -> Result<Value, String> {
        let obj = args_object(v, &["path", "line_offset", "max_lines", "max_tokens"])?;
        let path = arg_str(&obj, "path")?;
        let line_offset = opt_uint(&obj, "line_offset")?.unwrap_or(1);
        if line_offset == 0 {
            return Err("line_offset 必须为正整数（行号从 1 起）".into());
        }
        let line_offset =
            usize::try_from(line_offset).map_err(|_| "line_offset 超出范围".to_string())?;
        let max_lines = opt_uint(&obj, "max_lines")?;
        if max_lines == Some(0) {
            return Err("max_lines 必须为正整数".into());
        }
        let max_lines = max_lines.map(|n| usize::try_from(n).unwrap_or(usize::MAX));
        let max_tokens = opt_uint(&obj, "max_tokens")?.unwrap_or(0);
        let max_tokens = if max_tokens == 0 {
            DEFAULT_READ_MAX_TOKENS
        } else {
            usize::try_from(max_tokens).unwrap_or(usize::MAX)
        };

        let scoped = resolve_scoped_path(root, Some(&path))?;
        let Some(metadata) = symlink_metadata_or_none(&scoped)? else {
            return Err(format!("文件不存在：{path}"));
        };
        if metadata.file_type().is_symlink() {
            return Err(format!("路径不得为符号链接：{path}"));
        }
        if !metadata.is_file() {
            return Err(format!("路径不是常规文件：{path}"));
        }
        let content = std::fs::read_to_string(&scoped).map_err(|e| format!("读取文件失败：{e}"))?;

        let start_byte = line_start_byte_offset(&content, line_offset)?;
        let end_byte = line_end_byte_offset(&content, start_byte, max_lines);
        let slice = &content[start_byte..end_byte];
        let truncated_content = truncate_text(slice, TruncationPolicy::Tokens(max_tokens));
        let truncated = end_byte < content.len() || truncated_content != slice;
        Ok(json!({
            "path": path,
            "start_line_number": line_offset,
            "content": truncated_content,
            "truncated": truncated,
        }))
    };
    run().map(|v| v.to_string()).unwrap_or_else(err_json)
}

fn line_start_byte_offset(content: &str, line_offset: usize) -> Result<usize, String> {
    if line_offset == 1 {
        return Ok(0);
    }
    let mut current = 1usize;
    for (idx, ch) in content.char_indices() {
        if ch == '\n' {
            current += 1;
            if current == line_offset {
                return Ok(idx + 1);
            }
        }
    }
    Err(format!("line_offset {line_offset} 超出文件行数"))
}

fn line_end_byte_offset(content: &str, start_byte: usize, max_lines: Option<usize>) -> usize {
    let Some(max_lines) = max_lines else {
        return content.len();
    };
    let mut lines_seen = 1usize;
    for (relative_idx, ch) in content[start_byte..].char_indices() {
        if ch == '\n' {
            if lines_seen == max_lines {
                return start_byte + relative_idx + 1;
            }
            lines_seen += 1;
        }
    }
    content.len()
}

/// mem_write（D3 批准边界）：常规文件新建/覆盖，≤1 MiB，父目录须存在，
/// 不触隐藏/排除/符号链接路径；0600 权限经 `catalog::write_private_file`。
fn mem_write(root: &Path, v: &Value, expected_epoch: u64) -> String {
    let run = || -> Result<Value, String> {
        let obj = args_object(v, &["path", "content"])?;
        let path = arg_str(&obj, "path")?;
        let content = arg_str(&obj, "content")?;
        if content.len() > MAX_MEMORY_FILE_BYTES {
            return Err(format!(
                "单次写入内容 {} 字节超过上限 {MAX_MEMORY_FILE_BYTES} 字节",
                content.len()
            ));
        }

        let scoped = resolve_scoped_path(root, Some(&path))?;
        match symlink_metadata_or_none(&scoped)? {
            Some(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!("路径不得为符号链接：{path}"));
                }
                if !metadata.is_file() {
                    return Err(format!("目标已存在且不是常规文件：{path}"));
                }
            }
            None => {
                let parent = scoped
                    .parent()
                    .ok_or_else(|| format!("路径缺少父目录：{path}"))?;
                let parent_ok = symlink_metadata_or_none(parent)?.is_some_and(|m| m.is_dir());
                if !parent_ok {
                    return Err(format!("父目录不存在（不自动创建）：{path}"));
                }
            }
        }
        // 013-M7 收口（第四轮）：校验对象＝**任务自身的代数快照**（领取时
        // 绑定、不可变），不是共享配对文件——新代任务的 prepare 不会让旧
        // 任务的凭据恢复。「检查＋写入」在根目录共享锁内同一临界区（与
        // 重置的排他清空互斥——排除交错竞态；跨进程有效）。
        #[cfg(unix)]
        {
            let _lock = crate::memory::runner::lock_shared(root);
            if crate::memory::runner::read_epoch(root) != expected_epoch {
                return Err("记忆已被重置，本任务写入已中止（任务代数失效）".into());
            }
            crate::catalog::write_private_file(&scoped, &content)
                .map_err(|e| format!("写入文件失败：{e}"))?;
        }
        #[cfg(not(unix))]
        {
            if crate::memory::runner::read_epoch(root) != expected_epoch {
                return Err("记忆已被重置，本任务写入已中止（任务代数失效）".into());
            }
            crate::catalog::write_private_file(&scoped, &content)
                .map_err(|e| format!("写入文件失败：{e}"))?;
        }
        Ok(json!({ "ok": true, "path": path, "bytes": content.len() }))
    };
    match run() {
        Ok(v) => v.to_string(),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

/// mem_list（上游 `local/list.rs::list`：单层、排序、游标分页；隐藏/
/// 排除/符号链接条目不展示——D3 批准边界）。
fn mem_list(root: &Path, v: &Value) -> String {
    let run = || -> Result<Value, String> {
        let obj = args_object(v, &["path", "cursor", "max_results"])?;
        let path = match obj.get("path") {
            None | Some(Value::Null) => None,
            Some(p) => Some(p.as_str().ok_or("path 必须是字符串")?.to_string()),
        };
        let max_results = opt_uint(&obj, "max_results")?
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .unwrap_or(DEFAULT_LIST_MAX_RESULTS)
            .min(MAX_LIST_RESULTS);
        let start_index = match obj.get("cursor") {
            None | Some(Value::Null) => 0usize,
            Some(c) => c
                .as_str()
                .ok_or("cursor 必须是字符串")?
                .parse::<usize>()
                .map_err(|_| "cursor 必须是非负整数".to_string())?,
        };

        let start = resolve_scoped_path(root, path.as_deref())?;
        let Some(metadata) = symlink_metadata_or_none(&start)? else {
            return Err(format!("路径不存在：{}", path.unwrap_or_default()));
        };
        if metadata.file_type().is_symlink() {
            return Err("路径不得为符号链接".into());
        }

        let mut entries: Vec<Value> = Vec::new();
        if metadata.is_file() {
            entries.push(json!({
                "path": display_relative_path(root, &start),
                "entry_type": "file",
            }));
        } else if metadata.is_dir() {
            let mut children: Vec<PathBuf> = match std::fs::read_dir(&start) {
                Ok(dir) => dir.filter_map(|e| e.ok().map(|e| e.path())).collect(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(format!("读取目录失败：{e}")),
            };
            children.sort();
            for child in children {
                let file_type = match std::fs::symlink_metadata(&child) {
                    Ok(m) => m.file_type(),
                    Err(_) => continue, // 迭代中消失的条目忽略
                };
                if file_type.is_symlink() || is_hidden_path(&child) || is_excluded_path(&child) {
                    continue;
                }
                let entry_type = if file_type.is_dir() {
                    "directory"
                } else if file_type.is_file() {
                    "file"
                } else {
                    continue;
                };
                entries.push(json!({
                    "path": display_relative_path(root, &child),
                    "entry_type": entry_type,
                }));
            }
        }
        if start_index > entries.len() {
            return Err(format!(
                "cursor {start_index} 超过结果总数 {}",
                entries.len()
            ));
        }
        let end_index = start_index.saturating_add(max_results).min(entries.len());
        let next_cursor = (end_index < entries.len()).then(|| end_index.to_string());
        let truncated = next_cursor.is_some();
        Ok(json!({
            "path": path,
            "entries": entries[start_index..end_index].to_vec(),
            "next_cursor": next_cursor,
            "truncated": truncated,
        }))
    };
    run().map(|v| v.to_string()).unwrap_or_else(err_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_and_excluded_component_checks() {
        assert!(is_hidden_component(
            Path::new(".git").components().next().unwrap()
        ));
        assert!(is_hidden_component(
            Path::new("a/.x").components().nth(1).unwrap()
        ));
        assert!(!is_hidden_component(
            Path::new("ab").components().next().unwrap()
        ));
        assert!(is_excluded_component(
            Path::new("memories.sqlite").components().next().unwrap()
        ));
        assert!(is_excluded_component(
            Path::new("sub/memories.sqlite-journal")
                .components()
                .nth(1)
                .unwrap()
        ));
        assert!(is_hidden_path(Path::new("a/.hidden")));
        assert!(!is_hidden_path(Path::new("a/visible")));
        assert!(is_excluded_path(Path::new("x/memories.sqlite")));
    }

    #[test]
    fn line_offsets() {
        let text = "a\nbb\nccc";
        assert_eq!(line_start_byte_offset(text, 1).unwrap(), 0);
        assert_eq!(line_start_byte_offset(text, 2).unwrap(), 2);
        assert_eq!(line_start_byte_offset(text, 3).unwrap(), 5);
        assert!(line_start_byte_offset(text, 4).is_err());
        assert_eq!(line_end_byte_offset(text, 0, None), text.len());
        assert_eq!(line_end_byte_offset(text, 0, Some(1)), 2);
        assert_eq!(line_end_byte_offset(text, 2, Some(1)), 5);
        assert_eq!(line_end_byte_offset(text, 5, Some(9)), text.len());
    }
}
