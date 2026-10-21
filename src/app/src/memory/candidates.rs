//! 013 M2：候选选择——启动期第一阶段任务的筛选与领取编排。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——`codex-rs/state/src/runtime/memories.rs::
//! claim_stage1_jobs_for_startup` 的编排循环移植，常量取自
//! `codex-rs/memories/write/src/lib.rs`（`mod stage_one`）。适配点：
//! 会话以文件存在（`session::list_sessions` 目录枚举）与 `threads` 登记
//! （G6 最小表）的交集为候选池；上游的交互来源白名单/归档过滤无对应概念
//! （登记差异见 013 §2.2）。职责边界：本模块只读登记、领取任务（jobs 写入），
//! 不登记会话（登记＝会话写入方职责，后续阶段接线）、不装载会话正文（M3）、
//! 不引入任何后台执行行为（D2 后续阶段）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::memory::config::MemoriesConfig;
use crate::memory::store::{MemoryStore, Stage1JobClaimOutcome};
use crate::session;

/// 数据库候选扫描上限（上游 `THREAD_SCAN_LIMIT` 常量，非配置）。
pub const THREAD_SCAN_LIMIT: usize = 5_000;

/// 两阶段任务所有权租期秒数（上游 `JOB_LEASE_SECONDS` 常量，非配置）。
pub const JOB_LEASE_SECONDS: i64 = 3_600;

/// 启动期领取参数（上游 `Stage1StartupClaimParams`；`allowed_sources` 无本地
/// 对应概念，登记差异）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage1StartupClaimParams {
    pub scan_limit: usize,
    pub max_claimed: usize,
    pub max_age_days: i64,
    pub min_rollout_idle_hours: i64,
    pub lease_seconds: i64,
}

/// 领取成功的候选（上游 `Stage1JobClaim` 的最小投影：`ThreadMetadata` 在本地
/// 仅含登记列）。`session_path` 供 M3 装载会话正文；`source_updated_at` 为
/// 秒级时间戳（上游 `updated_at.timestamp()` 的 ms→s 截断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage1JobClaim {
    pub thread_id: String,
    pub session_path: PathBuf,
    pub updated_at_ms: i64,
    pub source_updated_at: i64,
    pub ownership_token: String,
}

/// 配置 → 领取参数（上游 `phase1::claim_startup_jobs` 的填参点）：扫描上限与
/// 租期取常量，三个窗口/数量参数经 [`MemoriesConfig::clamped`] 钳制
/// （上游在 `From<MemoriesToml>` 换算时钳制）。
pub fn params_from_config(config: &MemoriesConfig) -> Stage1StartupClaimParams {
    let clamped = config.clamped();
    Stage1StartupClaimParams {
        scan_limit: THREAD_SCAN_LIMIT,
        max_claimed: clamped.max_rollouts_per_startup,
        max_age_days: clamped.max_rollout_age_days,
        min_rollout_idle_hours: clamped.min_rollout_idle_hours,
        lease_seconds: JOB_LEASE_SECONDS,
    }
}

/// 筛选并领取启动期第一阶段任务（上游 `claim_stage1_jobs_for_startup` 编排）：
///
/// - `scan_limit`/`max_claimed` 为 0 直接空返回（不触目录与数据库）；
/// - 扫描池＝threads 登记（enabled、非当前会话、时间窗、更新时间降序、上限），
///   再与 sessions 目录枚举求交集（会话文件须存在）；
/// - 逐候选检查源版本是否需要更新（结果行或成功水位覆盖即跳过），再经
///   [`MemoryStore::try_claim_stage1_job`] 领取（`max_running_jobs`＝领取上限，
///   与上游一致）；领满即止，后续候选不再尝试；
/// - 候选身份＝（会话 ID，源更新时间）：threads 主键与目录 stem 结构性唯一，
///   不产生重复。`session_path` 仅对已通过目录交集的 stem 构造（readdir stem
///   不含路径分隔符，无越界可能）。
pub fn claim_stage1_jobs_for_startup(
    store: &MemoryStore,
    sessions_dir: &Path,
    current_thread_id: &str,
    params: Stage1StartupClaimParams,
) -> Result<Vec<Stage1JobClaim>, String> {
    if params.scan_limit == 0 || params.max_claimed == 0 {
        return Ok(Vec::new());
    }

    let on_disk: HashSet<String> = session::list_sessions(sessions_dir)
        .into_iter()
        .map(|(stem, _)| stem)
        .collect();
    let rows = store.stage1_startup_scan(
        current_thread_id,
        params.max_age_days,
        params.min_rollout_idle_hours,
        params.scan_limit,
    )?;

    let mut claimed = Vec::new();
    for (thread_id, updated_at_ms) in rows {
        if claimed.len() >= params.max_claimed {
            break;
        }
        if !on_disk.contains(&thread_id) {
            continue;
        }
        let source_updated_at = updated_at_ms.div_euclid(1000);
        if !store.stage1_source_needs_update(&thread_id, source_updated_at)? {
            continue;
        }
        if let Stage1JobClaimOutcome::Claimed { ownership_token } = store.try_claim_stage1_job(
            &thread_id,
            current_thread_id,
            source_updated_at,
            params.lease_seconds,
            params.max_claimed,
        )? {
            claimed.push(Stage1JobClaim {
                session_path: sessions_dir.join(format!("{thread_id}.jsonl")),
                thread_id,
                updated_at_ms,
                source_updated_at,
                ownership_token,
            });
        }
    }
    Ok(claimed)
}
