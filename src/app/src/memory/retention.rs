//! 013-M9：扩展资源保留清理。
//!
//! 来源：openai/codex@ce0323f2f2d3b25e82806cd555237da3db4e1897
//! codex-rs/memories/write/src/extensions/prune.rs（Apache-2.0）。
//! tokio fs→std fs、now→已有 Clock 秒值、tracing→调用方告警出口；
//! 目录/文件判断、UTC 前缀、包含等号的 14 天边界及 best-effort 保留。
//! 调用方负责在既有 epoch/根共享锁临界区内执行，模型等待不持锁。

use std::path::Path;

/// 固定上游 stage_one::PRUNE_BATCH_SIZE，单次后台启动不循环清空。
pub const PRUNE_BATCH_SIZE: usize = 200;
const RESOURCE_RETENTION_SECONDS: i64 = 14 * 86_400;
const FILENAME_TS_FORMAT: &str = "%Y-%m-%dT%H-%M-%S";

/// 扩展说明按上游仅判断存在（不要求常规文件）；仅扫描 resources 直属
/// 常规 .md。缺目录/不可解析跳过，删除失败告警后继续，不扫描 notes。
pub fn prune_old_extension_resources(
    root: &Path,
    now_seconds: i64,
    warnings: &mut dyn FnMut(&str),
) {
    let cutoff = now_seconds.saturating_sub(RESOURCE_RETENTION_SECONDS);
    let extensions_root = root.join("extensions");
    // 独立调用亦不跟随扫描根或 resources 目录的链接；管线的 M4 layout
    // 已清理链接，此处保留同一根内边界，不允许经链接删除根外资源。
    if !is_directory_without_link(root) || !is_directory_without_link(&extensions_root) {
        return;
    }
    let mut extensions = match std::fs::read_dir(&extensions_root) {
        Ok(entries) => entries,
        Err(err) => {
            warn(warnings, format!("读取扩展目录失败：{err}"));
            return;
        }
    };
    while let Some(Ok(extension)) = extensions.next() {
        let path = extension.path();
        if !extension.file_type().is_ok_and(|t| t.is_dir())
            || !path.join("instructions.md").try_exists().unwrap_or(false)
        {
            continue;
        }
        let resources = path.join("resources");
        if !is_directory_without_link(&resources) {
            continue;
        }
        let mut resources = match std::fs::read_dir(&resources) {
            Ok(entries) => entries,
            Err(err) => {
                warn(warnings, format!("读取扩展资源失败：{err}"));
                continue;
            }
        };
        while let Some(Ok(resource)) = resources.next() {
            if !resource.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let path = resource.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".md") {
                continue;
            }
            let Some(timestamp) = resource_timestamp(name) else {
                continue;
            };
            if timestamp > cutoff {
                continue;
            }
            if let Err(err) = std::fs::remove_file(&path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    warn(
                        warnings,
                        format!("删除过期扩展资源 {} 失败：{err}", path.display()),
                    );
                }
            }
        }
    }
}

fn is_directory_without_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn resource_timestamp(name: &str) -> Option<i64> {
    Some(
        chrono::NaiveDateTime::parse_from_str(name.get(..19)?, FILENAME_TS_FORMAT)
            .ok()?
            .and_utc()
            .timestamp(),
    )
}

fn warn(warnings: &mut dyn FnMut(&str), message: String) {
    warnings(&crate::display::sanitize_for_terminal(&message));
}
