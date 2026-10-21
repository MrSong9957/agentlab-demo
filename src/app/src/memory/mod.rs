//! 013 M1：V1 记忆系统状态存储与骨架。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——DDL 逐字拷贝自 `codex-rs/state/memory_migrations/*.sql`，
//! `MemoryStore` 的 SQL 与事务语义逐条移植自 `codex-rs/state/src/runtime/memories.rs`
//! （sqlx→rusqlite 适配见 store.rs 头注释）。D1（rusqlite bundled＋SQL 移植）与
//! D5（`<user_dir>/memories/`＋manual.json memories 段）用户已确认（2026-10-18）。

pub mod candidates;
pub mod config;
pub mod feedback;
pub mod inject;
pub mod mem_tools;
pub mod phase1;
pub mod phase1_output;
pub mod phase2;
pub mod retention;
pub mod rollout_input;
pub mod runner;
pub mod secrets;
pub mod store;
pub mod storage_files;
pub mod truncate;
pub mod workspace;

pub use candidates::{
    claim_stage1_jobs_for_startup, params_from_config, Stage1JobClaim, Stage1StartupClaimParams,
    JOB_LEASE_SECONDS, THREAD_SCAN_LIMIT,
};
pub use config::MemoriesConfig;
pub use feedback::{after_user_turn, cited_thread_ids};
pub use phase1::{
    build_stage_one_input_message, build_stage_one_request, rollout_token_limit, run_stage1_job,
    Stage1JobOutcome, DEFAULT_ROLLOUT_TOKEN_LIMIT, JOB_RETRY_DELAY_SECONDS, STAGE_ONE_SYSTEM,
};
pub use phase1_output::StageOneOutput;
pub use phase2::{
    build_consolidation_prompt, run_phase2_consolidation, Phase2Outcome, Phase2Params,
    ThreadFileMeta, CONSOLIDATION_SYSTEM, JOB_HEARTBEAT_SECONDS,
};
pub use workspace::{
    validate_consolidation_artifacts, GitBaselineChange, GitBaselineChangeStatus, GitBaselineDiff,
    WORKSPACE_DIFF_FILENAME, WORKSPACE_DIFF_MAX_BYTES,
};
pub use rollout_input::{
    read_recent_rollout_items, sanitize_history_item_for_memories, serialize_filtered_history_items,
    MAX_RECORD_BYTES, MAX_ROLLOUT_BYTES,
};
pub use secrets::redact_secrets;
pub use storage_files::{sync_stage1_snapshot, Stage1FileInput};
pub use store::{
    memories_db_path, new_token, system_clock, Clock, JobRow, MemoryStore, Phase2JobClaimOutcome,
    SelectedOutput, Stage1JobClaimOutcome, Stage1OutputRow,
};
pub use truncate::{
    approx_bytes_for_tokens, approx_token_count, truncate_text, TruncationPolicy,
};

use std::path::{Path, PathBuf};

/// 初始化记忆根目录：拒绝符号链接与既有非目录路径，创建（或复用）目录并置 0700。
pub fn init_memories_root(dir: &Path) -> Result<PathBuf, String> {
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(format!("记忆根目录不得为符号链接：{}", dir.display()));
        }
        if !meta.is_dir() {
            return Err(format!("记忆根目录已存在且不是目录：{}", dir.display()));
        }
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("创建记忆根目录 {} 失败：{e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("设置记忆根目录权限失败：{e}"))?;
    }
    Ok(dir.to_path_buf())
}
