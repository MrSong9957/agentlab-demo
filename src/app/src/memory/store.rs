//! 013 M1：`MemoryStore`——任务领取、所有权、租期、重试与全局合并协调（同步移植）。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! `codex-rs/state/src/runtime/memories.rs`（Apache-2.0）。SQL 语句逐字保留；
//! 适配面（登记于 013 §2.2 完整适配清单）：
//! - sqlx(async)→rusqlite(同步)；`begin_immediate()`→`transaction_with_behavior(Immediate)`，
//!   事务内早退＝drop＝回滚（与上游一致）；busy 重试→`busy_timeout(5s)`。
//! - 时钟：上游 `Utc::now().timestamp()` 内联取值→`Clock` 注入（默认系统时钟），
//!   时间语义不变（秒级 Unix 时间戳），测试可固定/步进。
//! - 所有权令牌：上游 `Uuid::new_v4()`→`new_token()`（纳秒-pid-计数器，唯一性目标等价；
//!   生成侧无需随机 UUID——uuid 依赖仅用于既有文件名解析〔storage_files.rs〕，不用于令牌）；
//!   令牌仅作 SQL 相等比较，格式不参与语义。
//! - `selected_outputs` 入参取上游 `Stage1Output` 的最小投影 `(thread_id, source_updated_at)`。
//! - 行读取返回裸 `i64/String`（上游在行映射层转 `DateTime`，由调用方换算）。
//! - 迁移执行器：上游 sqlx::migrate! → 本地 `schema_migrations` 登记表按文件名序执行一次
//!   （幂等重开）；DDL 文本本身未改动（见 migrations/*.sql 头注释）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};

pub const JOB_KIND_MEMORY_STAGE1: &str = "memory_stage1";
pub const JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL: &str = "memory_consolidate_global";
pub const MEMORY_CONSOLIDATION_JOB_KEY: &str = "global";
const PHASE2_SUCCESS_COOLDOWN_SECONDS: i64 = 6 * 60 * 60;

/// phase2 成功冷却判定（014 §2「同源」）：领取原语与状态展示共用同一
/// 条件与同一截止公式——展示不得另立冷却规则。
fn phase2_success_cooldown_cutoff(now: i64) -> i64 {
    now.saturating_sub(PHASE2_SUCCESS_COOLDOWN_SECONDS)
}

fn phase2_in_success_cooldown(
    now: i64,
    last_error: Option<&str>,
    finished_at: Option<i64>,
) -> bool {
    last_error.is_none()
        && finished_at.is_some_and(|finished_at| finished_at > phase2_success_cooldown_cutoff(now))
}
const DEFAULT_RETRY_REMAINING: i64 = 3;
/// 合并输入选择的分页大小（上游 `PHASE2_INPUT_SELECTION_PAGE_SIZE`）。
const PHASE2_INPUT_SELECTION_PAGE_SIZE: usize = 512;

/// 迁移清单（数组序即执行序；文本见同目录 migrations/，含来源头注释）。
const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_memories.sql", include_str!("migrations/0001_memories.sql")),
    ("0002_consolidation_progress.sql", include_str!("migrations/0002_consolidation_progress.sql")),
    ("0003_threads_local.sql", include_str!("migrations/0003_threads_local.sql")),
];

/// 秒级时钟（Unix 时间戳）；默认取系统时间，测试注入固定/步进值。
pub type Clock = std::sync::Arc<dyn Fn() -> i64 + Send + Sync>;

/// 系统时钟（默认）：纪元内取当前秒，纪元前回退 0（仅测试注入会偏离该值）。
pub fn system_clock() -> Clock {
    std::sync::Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    })
}

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 所有权令牌：纳秒时间戳-pid-进程内计数器（进程内严格唯一；跨进程由 pid＋纳秒区分，
/// 碰撞概率可忽略——令牌仅用于 SQL 相等比较，无格式语义）。
pub fn new_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{}-{n}", std::process::id())
}

/// 第一阶段领取结果（上游 `Stage1JobClaimOutcome` 语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage1JobClaimOutcome {
    /// 领取成功，携带本次所有权令牌。
    Claimed { ownership_token: String },
    /// 该源版本已有成功结果或成功水位，无需再提炼。
    SkippedUpToDate,
    /// 重试计数耗尽（更新的输入版本可恢复）。
    SkippedRetryExhausted,
    /// 失败退避等待中。
    SkippedRetryBackoff,
    /// 有效运行中的任务（含运行数上限）占线。
    SkippedRunning,
}

/// 全局合并任务领取结果（上游 `Phase2JobClaimOutcome` 语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase2JobClaimOutcome {
    Claimed { ownership_token: String, input_watermark: i64 },
    /// 失败退避等待中。
    SkippedRetryUnavailable,
    SkippedRunning,
    /// 上次成功结束仍在冷却期（6 小时）。
    SkippedCooldown,
}

/// 014 §2：最近一条失败任务的只读快照（`/memory-status` 展示面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestErrorJob {
    pub kind: String,
    pub last_error: Option<String>,
    pub retry_at: Option<i64>,
    /// 列为 NOT NULL（INSERT 恒 DEFAULT、UPDATE 恒算术式）——按非空建模。
    pub retry_remaining: i64,
}

/// `stage1_outputs` 行的裸读取（观测/往返断言与 M2+ 输入；时间戳为秒级整数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage1OutputRow {
    pub thread_id: String,
    pub source_updated_at: i64,
    pub raw_memory: String,
    pub rollout_summary: String,
    pub rollout_slug: Option<String>,
    pub generated_at: i64,
    pub selected_for_phase2: bool,
    pub selected_for_phase2_source_updated_at: Option<i64>,
}

/// `jobs` 行的裸读取（领取/重试/租期断言）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub kind: String,
    pub job_key: String,
    pub status: String,
    pub worker_id: Option<String>,
    pub ownership_token: Option<String>,
    pub lease_until: Option<i64>,
    pub retry_at: Option<i64>,
    pub retry_remaining: i64,
    pub last_error: Option<String>,
    pub input_watermark: Option<i64>,
    pub last_success_watermark: Option<i64>,
}

/// 合并成功时选中的来源快照（上游 `Stage1Output` 最小投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedOutput {
    pub thread_id: String,
    pub source_updated_at: i64,
}

/// 记忆状态存储。单连接＋互斥锁：CLI 单进程内串行化全部读写；
/// 跨进程并发由 SQLite 文件锁＋`busy_timeout` 承接（与上游池化写路径等价的串行语义）。
pub struct MemoryStore {
    conn: Mutex<Connection>,
    clock: Clock,
}

fn serr(ctx: &str, e: rusqlite::Error) -> String {
    format!("{ctx}：{e}")
}

impl MemoryStore {
    /// 打开（或创建）记忆库：根目录 0700（拒绝符号链接）、数据库文件 0600、
    /// 迁移按文件名序执行一次（幂等重开）。
    pub fn open(root: &Path, clock: Clock) -> Result<Self, String> {
        super::init_memories_root(root)?;

        let db = memories_db_path(root);
        let fresh = !db.exists();
        let conn = Connection::open(&db)
            .map_err(|e| format!("打开记忆数据库 {} 失败：{e}", db.display()))?;
        #[cfg(unix)]
        if fresh {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("设置数据库文件权限失败：{e}"))?;
        }
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| serr("设置 busy_timeout 失败", e))?;
        conn.pragma_update(None, "journal_mode", "DELETE")
            .map_err(|e| serr("设置 journal_mode 失败", e))?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| serr("设置 synchronous 失败", e))?;

        let mut store = Self { conn: Mutex::new(conn), clock };
        store.run_migrations()?;
        Ok(store)
    }

    /// 仅打开**既有**记忆库（014 §1）：不建根、不建库（打开标志不含
    /// `SQLITE_OPEN_CREATE`）、不跑迁移。库文件缺失返回 `Ok(None)`，其余
    /// 打开失败返回 `Err`——调用方（反馈路径）据此静默跳过，**不得回退**
    /// 到允许创建的 [`MemoryStore::open`]。`is_file` 预检只为区分
    /// None/Err；真正禁止创建由打开标志承担（预检与打开之间文件被 reset
    /// 删除时，打开失败而非重建）。
    pub fn open_existing(root: &Path, clock: Clock) -> Result<Option<Self>, String> {
        // M1 契约对齐：既有 `open` 经 `init_memories_root` 拒绝符号链接根
        // （m1_open_rejects_symlink_memory_root）——本入口不建根、不 chmod，
        // 但保留同一拒绝；检查为纯 `symlink_metadata`，无任何创建副作用。
        if let Ok(meta) = std::fs::symlink_metadata(root) {
            if meta.file_type().is_symlink() {
                return Err(format!("记忆根目录不得为符号链接：{}", root.display()));
            }
        }
        let db = memories_db_path(root);
        if !db.is_file() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("打开既有记忆数据库 {} 失败：{e}", db.display()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| serr("设置 busy_timeout 失败", e))?;
        conn.pragma_update(None, "journal_mode", "DELETE")
            .map_err(|e| serr("设置 journal_mode 失败", e))?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| serr("设置 synchronous 失败", e))?;
        Ok(Some(Self {
            conn: Mutex::new(conn),
            clock,
        }))
    }

    fn run_migrations(&mut self) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|_| "数据库连接锁中毒".to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                name TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );",
        )
        .map_err(|e| serr("建立迁移登记表失败", e))?;
        for (name, sql) in MIGRATIONS {
            let applied = migration_applied(&conn, name)?;
            if applied {
                continue;
            }
            // DDL 与登记同事务（审查修复：两步若分离，崩溃夹缝会使重开重放
            // 无 IF NOT EXISTS 的 DDL 而永久失败）。
            apply_migration(&mut conn, name, sql, (self.clock)())?;
        }
        Ok(())
    }

    /// 当前时钟读数（注入时钟的观测出口；合并轮心跳节流判定用）。
    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn.lock().map_err(|_| "数据库连接锁中毒".to_string())
    }

    /// 领取第一阶段任务（上游 `try_claim_stage1_job`，SQL 逐字移植；
    /// 参数占位符改序号形式，绑定顺序与上游一致）。
    pub fn try_claim_stage1_job(
        &self,
        thread_id: &str,
        worker_id: &str,
        source_updated_at: i64,
        lease_seconds: i64,
        max_running_jobs: usize,
    ) -> Result<Stage1JobClaimOutcome, String> {
        let now = self.now();
        let lease_until = now.saturating_add(lease_seconds.max(0));
        let max_running_jobs = max_running_jobs as i64;
        let ownership_token = new_token();

        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| serr("开启事务失败", e))?;

        let existing_source: Option<i64> = tx
            .query_row(
                "SELECT source_updated_at FROM stage1_outputs WHERE thread_id = ?1",
                params![thread_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| serr("查询既有结果失败", e))?;
        if existing_source.is_some_and(|s| s >= source_updated_at) {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(Stage1JobClaimOutcome::SkippedUpToDate);
        }

        let last_success: Option<Option<i64>> = tx
            .query_row(
                "SELECT last_success_watermark FROM jobs WHERE kind = ?1 AND job_key = ?2",
                params![JOB_KIND_MEMORY_STAGE1, thread_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| serr("查询既有任务失败", e))?;
        if last_success.is_some_and(|w| w.is_some_and(|w| w >= source_updated_at)) {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(Stage1JobClaimOutcome::SkippedUpToDate);
        }

        let rows_affected = tx
            .execute(
                r#"
INSERT INTO jobs (
    kind,
    job_key,
    status,
    worker_id,
    ownership_token,
    started_at,
    finished_at,
    lease_until,
    retry_at,
    retry_remaining,
    last_error,
    input_watermark,
    last_success_watermark
)
SELECT ?1, ?2, 'running', ?3, ?4, ?5, NULL, ?6, NULL, ?7, NULL, ?8, NULL
WHERE (
    SELECT COUNT(*)
    FROM jobs
    WHERE kind = ?9
      AND status = 'running'
      AND lease_until IS NOT NULL
      AND lease_until > ?10
) < ?11
ON CONFLICT(kind, job_key) DO UPDATE SET
    status = 'running',
    worker_id = excluded.worker_id,
    ownership_token = excluded.ownership_token,
    started_at = excluded.started_at,
    finished_at = NULL,
    lease_until = excluded.lease_until,
    retry_at = NULL,
    retry_remaining = CASE
        WHEN excluded.input_watermark > COALESCE(jobs.input_watermark, -1) THEN ?12
        ELSE jobs.retry_remaining
    END,
    last_error = NULL,
    input_watermark = excluded.input_watermark
WHERE
    (jobs.status != 'running' OR jobs.lease_until IS NULL OR jobs.lease_until <= excluded.started_at)
    AND (
        jobs.retry_at IS NULL
        OR jobs.retry_at <= excluded.started_at
        OR excluded.input_watermark > COALESCE(jobs.input_watermark, -1)
    )
    AND (
        jobs.retry_remaining > 0
        OR excluded.input_watermark > COALESCE(jobs.input_watermark, -1)
    )
    AND (
        SELECT COUNT(*)
        FROM jobs AS running_jobs
        WHERE running_jobs.kind = excluded.kind
          AND running_jobs.status = 'running'
          AND running_jobs.lease_until IS NOT NULL
          AND running_jobs.lease_until > excluded.started_at
          AND running_jobs.job_key != excluded.job_key
    ) < ?13
            "#,
                params![
                    JOB_KIND_MEMORY_STAGE1,
                    thread_id,
                    worker_id,
                    ownership_token,
                    now,
                    lease_until,
                    DEFAULT_RETRY_REMAINING,
                    source_updated_at,
                    JOB_KIND_MEMORY_STAGE1,
                    now,
                    max_running_jobs,
                    DEFAULT_RETRY_REMAINING,
                    max_running_jobs,
                ],
            )
            .map_err(|e| serr("领取任务写入失败", e))?;

        if rows_affected > 0 {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(Stage1JobClaimOutcome::Claimed { ownership_token });
        }

        let existing: Option<(String, Option<i64>, Option<i64>, i64)> = tx
            .query_row(
                "SELECT status, lease_until, retry_at, retry_remaining FROM jobs WHERE kind = ?1 AND job_key = ?2",
                params![JOB_KIND_MEMORY_STAGE1, thread_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| serr("查询任务状态失败", e))?;

        tx.commit().map_err(|e| serr("提交事务失败", e))?;

        if let Some((status, existing_lease_until, retry_at, retry_remaining)) = existing {
            if retry_remaining <= 0 {
                return Ok(Stage1JobClaimOutcome::SkippedRetryExhausted);
            }
            if retry_at.is_some_and(|retry_at| retry_at > now) {
                return Ok(Stage1JobClaimOutcome::SkippedRetryBackoff);
            }
            if status == "running"
                && existing_lease_until.is_some_and(|lease_until| lease_until > now)
            {
                return Ok(Stage1JobClaimOutcome::SkippedRunning);
            }
        }
        Ok(Stage1JobClaimOutcome::SkippedRunning)
    }

    /// 成功且有内容（上游 `mark_stage1_job_succeeded`，SQL 逐字移植）。
    pub fn mark_stage1_job_succeeded(
        &self,
        thread_id: &str,
        ownership_token: &str,
        source_updated_at: i64,
        raw_memory: &str,
        rollout_summary: &str,
        rollout_slug: Option<&str>,
    ) -> Result<bool, String> {
        let now = self.now();
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| serr("开启事务失败", e))?;

        let rows_affected = tx
            .execute(
                r#"
UPDATE jobs
SET
    status = 'done',
    finished_at = ?1,
    lease_until = NULL,
    last_error = NULL,
    last_success_watermark = input_watermark
WHERE kind = ?2 AND job_key = ?3
  AND status = 'running' AND ownership_token = ?4
                "#,
                params![now, JOB_KIND_MEMORY_STAGE1, thread_id, ownership_token],
            )
            .map_err(|e| serr("任务置 done 失败", e))?;
        if rows_affected == 0 {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(false);
        }

        tx.execute(
            r#"
INSERT INTO stage1_outputs (
    thread_id,
    source_updated_at,
    raw_memory,
    rollout_summary,
    rollout_slug,
    generated_at
) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(thread_id) DO UPDATE SET
    source_updated_at = excluded.source_updated_at,
    raw_memory = excluded.raw_memory,
    rollout_summary = excluded.rollout_summary,
    rollout_slug = excluded.rollout_slug,
    generated_at = excluded.generated_at
WHERE excluded.source_updated_at >= stage1_outputs.source_updated_at
            "#,
            params![thread_id, source_updated_at, raw_memory, rollout_summary, rollout_slug, now],
        )
        .map_err(|e| serr("写入单会话结果失败", e))?;

        enqueue_global_consolidation_tx(&tx, source_updated_at)?;

        tx.commit().map_err(|e| serr("提交事务失败", e))?;
        Ok(true)
    }

    /// 成功但无内容（上游 `mark_stage1_job_succeeded_no_output`，SQL 逐字移植）。
    pub fn mark_stage1_job_succeeded_no_output(
        &self,
        thread_id: &str,
        ownership_token: &str,
    ) -> Result<bool, String> {
        let now = self.now();
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| serr("开启事务失败", e))?;

        let rows_affected = tx
            .execute(
                r#"
UPDATE jobs
SET
    status = 'done',
    finished_at = ?1,
    lease_until = NULL,
    last_error = NULL,
    last_success_watermark = input_watermark
WHERE kind = ?2 AND job_key = ?3
  AND status = 'running' AND ownership_token = ?4
                "#,
                params![now, JOB_KIND_MEMORY_STAGE1, thread_id, ownership_token],
            )
            .map_err(|e| serr("任务置 done 失败", e))?;
        if rows_affected == 0 {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(false);
        }

        let source_updated_at: i64 = tx
            .query_row(
                "SELECT input_watermark FROM jobs WHERE kind = ?1 AND job_key = ?2 AND ownership_token = ?3",
                params![JOB_KIND_MEMORY_STAGE1, thread_id, ownership_token],
                |r| r.get(0),
            )
            .map_err(|e| serr("读取任务水位失败", e))?;

        let deleted_rows = tx
            .execute("DELETE FROM stage1_outputs WHERE thread_id = ?1", params![thread_id])
            .map_err(|e| serr("删除旧结果失败", e))?;
        if deleted_rows > 0 {
            enqueue_global_consolidation_tx(&tx, source_updated_at)?;
        }

        tx.commit().map_err(|e| serr("提交事务失败", e))?;
        Ok(true)
    }

    /// 失败（上游 `mark_stage1_job_failed`，SQL 逐字移植）。
    pub fn mark_stage1_job_failed(
        &self,
        thread_id: &str,
        ownership_token: &str,
        failure_reason: &str,
        retry_delay_seconds: i64,
    ) -> Result<bool, String> {
        let now = self.now();
        let retry_at = now.saturating_add(retry_delay_seconds.max(0));
        let conn = self.lock_conn()?;
        let rows_affected = conn
            .execute(
                r#"
UPDATE jobs
SET
    status = 'error',
    finished_at = ?1,
    lease_until = NULL,
    retry_at = ?2,
    retry_remaining = retry_remaining - 1,
    last_error = ?3
WHERE kind = ?4 AND job_key = ?5
  AND status = 'running' AND ownership_token = ?6
                "#,
                params![
                    now,
                    retry_at,
                    failure_reason,
                    JOB_KIND_MEMORY_STAGE1,
                    thread_id,
                    ownership_token
                ],
            )
            .map_err(|e| serr("任务置 error 失败", e))?;
        Ok(rows_affected > 0)
    }

    /// 记录来源使用计数（013-M8；上游 `record_stage1_output_usage`，SQL
    /// 逐字移植）：单立即事务内逐 ID `usage_count+1`＋`last_usage=now`（秒，
    /// 与 `source_updated_at` 同单位可比）；**缺行忽略**（不插入、不报错），
    /// 返回受影响行数合计。调用方负责去重（上游同款——ID 列表来自已去重
    /// 的引用解析）。
    pub fn record_stage1_output_usage(&self, thread_ids: &[String]) -> Result<usize, String> {
        if thread_ids.is_empty() {
            return Ok(0);
        }
        let now = self.now();
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| serr("开启事务失败", e))?;
        let mut updated = 0usize;
        for thread_id in thread_ids {
            updated += tx
                .execute(
                    "UPDATE stage1_outputs\nSET\n    usage_count = COALESCE(usage_count, 0) + 1,\n    last_usage = ?\nWHERE thread_id = ?",
                    params![now, thread_id],
                )
                .map_err(|e| serr("记录使用计数失败", e))?;
        }
        tx.commit().map_err(|e| serr("提交事务失败", e))?;
        Ok(updated)
    }

    /// 入队/推进全局合并输入水位（上游 `enqueue_global_consolidation`）。
    pub fn enqueue_global_consolidation(&self, input_watermark: i64) -> Result<(), String> {
        let conn = self.lock_conn()?;
        enqueue_global_consolidation_on(&conn, input_watermark)
    }

    /// 领取全局合并任务（上游 `try_claim_global_phase2_job`，SQL 逐字移植）。
    pub fn try_claim_global_phase2_job(
        &self,
        worker_id: &str,
        lease_seconds: i64,
    ) -> Result<Phase2JobClaimOutcome, String> {
        let now = self.now();
        let lease_until = now.saturating_add(lease_seconds.max(0));
        let cooldown_cutoff = phase2_success_cooldown_cutoff(now);
        let ownership_token = new_token();

        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| serr("开启事务失败", e))?;

        let existing: Option<
            (String, Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<String>),
        > = tx
            .query_row(
                "SELECT status, lease_until, retry_at, input_watermark, finished_at, last_error FROM jobs WHERE kind = ?1 AND job_key = ?2",
                params![
                    JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                    MEMORY_CONSOLIDATION_JOB_KEY,
                ],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| serr("查询全局任务失败", e))?;

        let Some((
            status,
            existing_lease_until,
            retry_at,
            input_watermark,
            finished_at,
            last_error,
        )) = existing
        else {
            let rows_affected = tx
                .execute(
                    r#"
INSERT INTO jobs (
    kind,
    job_key,
    status,
    worker_id,
    ownership_token,
    started_at,
    finished_at,
    lease_until,
    retry_at,
    retry_remaining,
    last_error,
    input_watermark,
    last_success_watermark
) VALUES (?1, ?2, 'running', ?3, ?4, ?5, NULL, ?6, NULL, ?7, NULL, 0, 0)
                    "#,
                    params![
                        JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                        MEMORY_CONSOLIDATION_JOB_KEY,
                        worker_id,
                        ownership_token,
                        now,
                        lease_until,
                        DEFAULT_RETRY_REMAINING
                    ],
                )
                .map_err(|e| serr("建立全局任务失败", e))?;
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return if rows_affected == 0 {
                Ok(Phase2JobClaimOutcome::SkippedRunning)
            } else {
                Ok(Phase2JobClaimOutcome::Claimed { ownership_token, input_watermark: 0 })
            };
        };

        let input_watermark_value = input_watermark.unwrap_or(0);
        if retry_at.is_some_and(|retry_at| retry_at > now) {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(Phase2JobClaimOutcome::SkippedRetryUnavailable);
        }
        if status == "running"
            && existing_lease_until.is_some_and(|lease_until| lease_until > now)
        {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(Phase2JobClaimOutcome::SkippedRunning);
        }
        if phase2_in_success_cooldown(now, last_error.as_deref(), finished_at) {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(Phase2JobClaimOutcome::SkippedCooldown);
        }

        let rows_affected = tx
            .execute(
                r#"
UPDATE jobs
SET
    status = 'running',
    worker_id = ?1,
    ownership_token = ?2,
    started_at = ?3,
    finished_at = NULL,
    lease_until = ?4,
    retry_at = NULL,
    last_error = NULL
WHERE kind = ?5 AND job_key = ?6
  AND (status != 'running' OR lease_until IS NULL OR lease_until <= ?7)
  AND (retry_at IS NULL OR retry_at <= ?8)
  AND (last_error IS NOT NULL OR finished_at IS NULL OR finished_at <= ?9)
                "#,
                params![
                    worker_id,
                    ownership_token,
                    now,
                    lease_until,
                    JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                    MEMORY_CONSOLIDATION_JOB_KEY,
                    now,
                    now,
                    cooldown_cutoff
                ],
            )
            .map_err(|e| serr("领取全局任务失败", e))?;

        tx.commit().map_err(|e| serr("提交事务失败", e))?;
        if rows_affected == 0 {
            Ok(Phase2JobClaimOutcome::SkippedRunning)
        } else {
            Ok(Phase2JobClaimOutcome::Claimed {
                ownership_token,
                input_watermark: input_watermark_value,
            })
        }
    }

    /// 续期运行中的全局任务租约（上游 `heartbeat_global_phase2_job`）。
    pub fn heartbeat_global_phase2_job(
        &self,
        ownership_token: &str,
        lease_seconds: i64,
    ) -> Result<bool, String> {
        let now = self.now();
        let lease_until = now.saturating_add(lease_seconds.max(0));
        let conn = self.lock_conn()?;
        let rows_affected = conn
            .execute(
                r#"
UPDATE jobs
SET lease_until = ?1
WHERE kind = ?2 AND job_key = ?3
  AND status = 'running' AND ownership_token = ?4
                "#,
                params![
                    lease_until,
                    JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                    MEMORY_CONSOLIDATION_JOB_KEY,
                    ownership_token
                ],
            )
            .map_err(|e| serr("心跳续期失败", e))?;
        Ok(rows_affected > 0)
    }

    /// 全局任务成功（上游 `mark_global_phase2_job_succeeded`，SQL 逐字移植）。
    pub fn mark_global_phase2_job_succeeded(
        &self,
        ownership_token: &str,
        completed_watermark: i64,
        selected_outputs: &[SelectedOutput],
    ) -> Result<bool, String> {
        let now = self.now();
        let mut conn = self.lock_conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| serr("开启事务失败", e))?;

        let rows_affected = tx
            .execute(
                r#"
UPDATE jobs
SET
    status = 'done',
    finished_at = ?1,
    lease_until = NULL,
    last_error = NULL,
    last_success_watermark = max(COALESCE(last_success_watermark, 0), ?2)
WHERE kind = ?3 AND job_key = ?4
  AND status = 'running' AND ownership_token = ?5
                "#,
                params![
                    now,
                    completed_watermark,
                    JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                    MEMORY_CONSOLIDATION_JOB_KEY,
                    ownership_token
                ],
            )
            .map_err(|e| serr("全局任务置 done 失败", e))?;
        if rows_affected == 0 {
            tx.commit().map_err(|e| serr("提交事务失败", e))?;
            return Ok(false);
        }

        tx.execute(
            r#"
UPDATE stage1_outputs
SET
    selected_for_phase2 = 0,
    selected_for_phase2_source_updated_at = NULL
WHERE selected_for_phase2 != 0 OR selected_for_phase2_source_updated_at IS NOT NULL
            "#,
            [],
        )
        .map_err(|e| serr("清空选中基线失败", e))?;

        for output in selected_outputs {
            tx.execute(
                r#"
UPDATE stage1_outputs
SET
    selected_for_phase2 = 1,
    selected_for_phase2_source_updated_at = ?1
WHERE thread_id = ?2 AND source_updated_at = ?3
                "#,
                params![output.source_updated_at, output.thread_id, output.source_updated_at],
            )
            .map_err(|e| serr("标记选中基线失败", e))?;
        }

        tx.execute(
            "UPDATE consolidation_progress SET max_thread_count = MAX(max_thread_count, ?1)",
            params![selected_outputs.len() as i64],
        )
        .map_err(|e| serr("更新合并进度失败", e))?;

        tx.commit().map_err(|e| serr("提交事务失败", e))?;
        Ok(true)
    }

    /// 全局任务失败（上游 `mark_global_phase2_job_failed`）。
    pub fn mark_global_phase2_job_failed(
        &self,
        ownership_token: &str,
        failure_reason: &str,
        retry_delay_seconds: i64,
    ) -> Result<bool, String> {
        let now = self.now();
        let retry_at = now.saturating_add(retry_delay_seconds.max(0));
        let conn = self.lock_conn()?;
        let rows_affected = conn
            .execute(
                r#"
UPDATE jobs
SET
    status = 'error',
    finished_at = ?1,
    lease_until = NULL,
    retry_at = ?2,
    retry_remaining = max(retry_remaining - 1, 0),
    last_error = ?3
WHERE kind = ?4 AND job_key = ?5
  AND status = 'running' AND ownership_token = ?6
                "#,
                params![
                    now,
                    retry_at,
                    failure_reason,
                    JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                    MEMORY_CONSOLIDATION_JOB_KEY,
                    ownership_token
                ],
            )
            .map_err(|e| serr("全局任务置 error 失败", e))?;
        Ok(rows_affected > 0)
    }

    /// 失权兜底失败记账（上游 `mark_global_phase2_job_failed_if_unowned`：
    /// 匹配 `token = ? OR token IS NULL`——本库运行行恒有令牌，NULL 分支
    /// 仅为上游行形态兼容保留）。
    pub fn mark_global_phase2_job_failed_if_unowned(
        &self,
        ownership_token: &str,
        failure_reason: &str,
        retry_delay_seconds: i64,
    ) -> Result<bool, String> {
        let now = self.now();
        let retry_at = now.saturating_add(retry_delay_seconds.max(0));
        let conn = self.lock_conn()?;
        let rows_affected = conn
            .execute(
                r#"
UPDATE jobs
SET
    status = 'error',
    finished_at = ?1,
    lease_until = NULL,
    retry_at = ?2,
    retry_remaining = max(retry_remaining - 1, 0),
    last_error = ?3
WHERE kind = ?4 AND job_key = ?5
  AND status = 'running'
  AND (ownership_token = ?6 OR ownership_token IS NULL)
                "#,
                params![
                    now,
                    retry_at,
                    failure_reason,
                    JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                    MEMORY_CONSOLIDATION_JOB_KEY,
                    ownership_token
                ],
            )
            .map_err(|e| serr("失权兜底记账失败", e))?;
        Ok(rows_affected > 0)
    }

    /// G6＋014 §3：登记会话。`memory_mode` 由调用方按当前
    /// `generate_memories` 配置给出（'enabled'/'disabled'）；重复登记只刷新
    /// 更新时间，**不覆盖既有模式**（上游 keeps-creation-mode 同构——全局
    /// 配置变更不批量改旧行）。
    pub fn register_thread(
        &self,
        thread_id: &str,
        memory_mode: &str,
        updated_at_ms: i64,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            r#"
INSERT INTO threads (thread_id, memory_mode, updated_at_ms)
VALUES (?1, ?2, ?3)
ON CONFLICT(thread_id) DO UPDATE SET updated_at_ms = excluded.updated_at_ms
            "#,
            params![thread_id, memory_mode, updated_at_ms],
        )
        .map_err(|e| serr("登记会话失败", e))?;
        Ok(())
    }

    /// G6：读取会话记忆模式（None＝未登记）。
    pub fn thread_memory_mode(&self, thread_id: &str) -> Result<Option<String>, String> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT memory_mode FROM threads WHERE thread_id = ?1",
            params![thread_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| serr("查询会话记忆模式失败", e))
    }

    /// 上游 `stage1_source_needs_update`（私有）逐字移植：既有成功结果
    /// （`stage1_outputs.source_updated_at`）或任务成功水位
    /// （`jobs.last_success_watermark`）覆盖当前源版本即无需更新——空结果成功
    /// （无结果行）仍由水位挡住重复提炼。与 `try_claim_stage1_job` 事务内的
    /// 同两项检查有意重叠（上游如此：预检查省一次领取尝试，事务内检查才是
    /// 权威判定），非重复代码缺陷。
    pub fn stage1_source_needs_update(
        &self,
        thread_id: &str,
        source_updated_at: i64,
    ) -> Result<bool, String> {
        let conn = self.lock_conn()?;
        let existing_source: Option<i64> = conn
            .query_row(
                "SELECT source_updated_at FROM stage1_outputs WHERE thread_id = ?1",
                params![thread_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| serr("查询既有结果失败", e))?;
        if existing_source.is_some_and(|s| s >= source_updated_at) {
            return Ok(false);
        }

        let existing_watermark: Option<Option<i64>> = conn
            .query_row(
                "SELECT last_success_watermark FROM jobs WHERE kind = ?1 AND job_key = ?2",
                params![JOB_KIND_MEMORY_STAGE1, thread_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| serr("查询任务成功水位失败", e))?;
        if existing_watermark.is_some_and(|w| w.is_some_and(|w| w >= source_updated_at)) {
            return Ok(false);
        }
        Ok(true)
    }

    /// 上游 `claim_stage1_jobs_for_startup` 的 threads 扫描段：过滤/排序/计数口径
    /// 逐字对齐——`memory_mode='enabled'`、排除当前会话、`updated_at_ms` 落在
    /// `[now−年龄窗口, now−闲置窗口]`（均含端点，毫秒）、`ORDER BY updated_at_ms
    /// DESC LIMIT 扫描上限`。G6 最小表适配：无来源/归档列，上游的交互来源白名单与
    /// 归档过滤在本项目无对应概念（会话全部产自本 CLI；登记差异见 013 §2.2）。
    /// 扫描上限约束数据库行（上游计数口径），与目录枚举无关。
    pub fn stage1_startup_scan(
        &self,
        current_thread_id: &str,
        max_age_days: i64,
        min_rollout_idle_hours: i64,
        scan_limit: usize,
    ) -> Result<Vec<(String, i64)>, String> {
        let now_ms = self.now().saturating_mul(1000);
        let max_age_cutoff_ms =
            now_ms.saturating_sub(max_age_days.max(0).saturating_mul(86_400_000));
        let idle_cutoff_ms =
            now_ms.saturating_sub(min_rollout_idle_hours.max(0).saturating_mul(3_600_000));
        let scan_limit_i64 = i64::try_from(scan_limit).unwrap_or(i64::MAX);

        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                r#"
SELECT thread_id, updated_at_ms
FROM threads
WHERE memory_mode = 'enabled'
  AND thread_id != ?1
  AND updated_at_ms >= ?2
  AND updated_at_ms <= ?3
ORDER BY updated_at_ms DESC
LIMIT ?4
                "#,
            )
            .map_err(|e| serr("准备候选扫描失败", e))?;
        let rows = stmt
            .query_map(
                params![
                    current_thread_id,
                    max_age_cutoff_ms,
                    idle_cutoff_ms,
                    scan_limit_i64
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .map_err(|e| serr("候选扫描失败", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| serr("读取候选行失败", e))?;
        Ok(rows)
    }

    /// 读取单会话结果（观测/往返断言）。
    pub fn stage1_output(&self, thread_id: &str) -> Result<Option<Stage1OutputRow>, String> {
        let conn = self.lock_conn()?;
        conn.query_row(
            r#"
SELECT thread_id, source_updated_at, raw_memory, rollout_summary, rollout_slug,
       generated_at, selected_for_phase2, selected_for_phase2_source_updated_at
FROM stage1_outputs WHERE thread_id = ?1
            "#,
            params![thread_id],
            stage1_output_row,
        )
        .optional()
        .map_err(|e| serr("查询单会话结果失败", e))
    }

    /// M4 当前全量快照：单条 SELECT 在同一连接锁内读取，按 thread_id 稳定升序。
    /// 不含 M5 选择/保留规则，不写任务、水位或 selected 标志。
    pub fn list_stage1_outputs(&self) -> Result<Vec<Stage1OutputRow>, String> {
        let conn = self.lock_conn()?;
        let mut statement = conn.prepare(
            r#"
SELECT thread_id, source_updated_at, raw_memory, rollout_summary, rollout_slug,
       generated_at, selected_for_phase2, selected_for_phase2_source_updated_at
FROM stage1_outputs ORDER BY thread_id ASC
            "#,
        ).map_err(|e| serr("准备全量 stage1 快照失败", e))?;
        let rows = statement.query_map([], stage1_output_row)
            .map_err(|e| serr("读取全量 stage1 快照失败", e))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| serr("读取全量 stage1 快照行失败", e))
    }

    /// 013-M7 `/memory-status` 只读观察：stage1 产物行数。
    pub fn count_stage1_outputs(&self) -> Result<usize, String> {
        let conn = self.lock_conn()?;
        conn.query_row("SELECT COUNT(*) FROM stage1_outputs", [], |r| {
            let n: i64 = r.get(0)?;
            Ok(n as usize)
        })
        .map_err(|e| serr("统计 stage1 产物失败", e))
    }

    /// 013-M7 `/memory-status` 只读观察：任务按状态计数
    /// （running／done／error——失败等待重试；pending 归入运行中观察面）。
    pub fn count_jobs_by_status(&self) -> Result<(usize, usize, usize), String> {
        let conn = self.lock_conn()?;
        let count = |clause: &str| -> Result<usize, String> {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM jobs WHERE status {clause}"),
                [],
                |r| {
                    let n: i64 = r.get(0)?;
                    Ok(n as usize)
                },
            )
            .map_err(|e| serr("统计任务状态失败", e))
        };
        Ok((count("IN ('running','pending')")?, count("= 'done'")?, count("= 'error'")?))
    }

    /// 014 §2：最近一条失败任务的只读快照（阶段／原因／重试字段——
    /// `/memory-status` 展示面；不新增 schema，只读既有 jobs 列）。
    pub fn latest_error_job(&self) -> Result<Option<LatestErrorJob>, String> {
        let conn = self.lock_conn()?;
        conn.query_row(
            r#"
SELECT kind, last_error, retry_at, retry_remaining FROM jobs
WHERE status = 'error' ORDER BY finished_at DESC, kind DESC, job_key DESC LIMIT 1
            "#,
            [],
            |r| {
                Ok(LatestErrorJob {
                    kind: r.get(0)?,
                    last_error: r.get(1)?,
                    retry_at: r.get(2)?,
                    retry_remaining: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| serr("查询最近失败任务失败", e))
    }

    /// 014 §2：全局合并任务的成功冷却剩余秒数——判定条件与
    /// [`MemoryStore::try_claim_global_phase2_job`] 判 `SkippedCooldown`
    /// 共用 [`phase2_in_success_cooldown`]（同源，不另立展示规则）；
    /// 展示另要求 `status='done'`——可达状态下（成功置 done、失败必置
    /// last_error）与领取条件等价，仅剔除不可达形态。不可推导（无行／
    /// 非 done／有错／窗口外）返回 `None`。
    pub fn phase2_success_cooldown_remaining(&self) -> Result<Option<i64>, String> {
        let now = self.now();
        let conn = self.lock_conn()?;
        let row: Option<(String, Option<i64>, Option<String>)> = conn
            .query_row(
                r#"
SELECT status, finished_at, last_error FROM jobs
WHERE kind = ?1 AND job_key = ?2
                "#,
                params![
                    JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
                    MEMORY_CONSOLIDATION_JOB_KEY,
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| serr("查询全局合并任务失败", e))?;
        let Some((status, finished_at, last_error)) = row else {
            return Ok(None);
        };
        if status != "done" {
            return Ok(None);
        }
        let Some(finished_at) = finished_at else {
            return Ok(None);
        };
        if !phase2_in_success_cooldown(now, last_error.as_deref(), Some(finished_at)) {
            return Ok(None);
        }
        Ok(Some(
            finished_at
                .saturating_add(PHASE2_SUCCESS_COOLDOWN_SECONDS)
                .saturating_sub(now),
        ))
    }

    /// 013-M7：threads 登记行的更新时间（毫秒）——登记写入侧与测试观察面。
    pub fn thread_updated_at_ms(&self, thread_id: &str) -> Result<Option<i64>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT updated_at_ms FROM threads WHERE thread_id = ?1")
            .map_err(|e| serr("查询 threads 登记失败", e))?;
        let mut rows = stmt
            .query(params![thread_id])
            .map_err(|e| serr("查询 threads 登记失败", e))?;
        match rows.next().map_err(|e| serr("读取 threads 登记失败", e))? {
            Some(row) => Ok(Some(row.get(0).map_err(|e| serr("读取登记时间失败", e))?)),
            None => Ok(None),
        }
    }

    /// 读取任务行（领取/重试/租期断言）。
    pub fn job_row(&self, kind: &str, job_key: &str) -> Result<Option<JobRow>, String> {
        let conn = self.lock_conn()?;
        conn.query_row(
            r#"
SELECT kind, job_key, status, worker_id, ownership_token, lease_until, retry_at,
       retry_remaining, last_error, input_watermark, last_success_watermark
FROM jobs WHERE kind = ?1 AND job_key = ?2
            "#,
            params![kind, job_key],
            |r| {
                Ok(JobRow {
                    kind: r.get(0)?,
                    job_key: r.get(1)?,
                    status: r.get(2)?,
                    worker_id: r.get(3)?,
                    ownership_token: r.get(4)?,
                    lease_until: r.get(5)?,
                    retry_at: r.get(6)?,
                    retry_remaining: r.get(7)?,
                    last_error: r.get(8)?,
                    input_watermark: r.get(9)?,
                    last_success_watermark: r.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|e| serr("查询任务行失败", e))
    }

    /// M9：固定上游同名方法的 SQL 逐字移植。仅删窗口外、未选中结果；
    /// 单条 DELETE 原子执行，任务/成功水位与合并进度原样保留。
    pub fn prune_stage1_outputs_for_retention(
        &self,
        max_unused_days: i64,
        limit: usize,
    ) -> Result<usize, String> {
        if limit == 0 {
            return Ok(0);
        }
        let cutoff = self
            .now()
            .saturating_sub(max_unused_days.max(0).saturating_mul(86_400));
        self.lock_conn()?
            .execute(
                r#"
DELETE FROM stage1_outputs
WHERE thread_id IN (
    SELECT thread_id
    FROM stage1_outputs
    WHERE selected_for_phase2 = 0
      AND COALESCE(last_usage, source_updated_at) < ?
    ORDER BY
      COALESCE(last_usage, source_updated_at) ASC,
      source_updated_at ASC,
      thread_id ASC
    LIMIT ?
)
            "#,
                params![cutoff, i64::try_from(limit).unwrap_or(i64::MAX)],
            )
            .map_err(|e| serr("清理过期初步结果失败", e))
    }

    /// 当前合并输入集（M5；上游 `get_phase2_input_selection`，SQL 逐字移植）：
    ///
    /// - 仅非空结果行（`raw_memory`/`rollout_summary` trim 后非空），且
    ///   `last_usage`（从未用过则 `source_updated_at`）不早于
    ///   `now - max_unused_days`（窗口外＝久未使用的记忆不再进入合并）；
    /// - 排序＝`usage_count DESC → COALESCE(last_usage, source_updated_at) DESC →
    ///   source_updated_at DESC → thread_id DESC`，分页（512/页）逐候选过
    ///   threads 启用门（登记且 `memory_mode='enabled'`），取前 n；
    /// - 返回行按 `thread_id ASC` 稳定排序——即本轮合并实际使用的快照版本。
    pub fn get_phase2_input_selection(
        &self,
        n: usize,
        max_unused_days: i64,
    ) -> Result<Vec<Stage1OutputRow>, String> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let now = (self.clock)();
        let cutoff = now.saturating_sub(max_unused_days.max(0).saturating_mul(86_400));

        let conn = self.lock_conn()?;
        let page_size = n.clamp(1, PHASE2_INPUT_SELECTION_PAGE_SIZE);
        let page_size_i64 = i64::try_from(page_size).unwrap_or(i64::MAX);
        let mut offset = 0_i64;
        let mut selected_keys: Vec<(String, i64)> = Vec::with_capacity(n);

        while selected_keys.len() < n {
            let candidate_rows: Vec<(String, i64)> = conn
                .prepare(
                    r#"
SELECT
    so.thread_id,
    so.source_updated_at
FROM stage1_outputs AS so
WHERE (length(trim(so.raw_memory)) > 0 OR length(trim(so.rollout_summary)) > 0)
  AND (
        (so.last_usage IS NOT NULL AND so.last_usage >= ?1)
        OR (so.last_usage IS NULL AND so.source_updated_at >= ?1)
  )
ORDER BY
    COALESCE(so.usage_count, 0) DESC,
    COALESCE(so.last_usage, so.source_updated_at) DESC,
    so.source_updated_at DESC,
    so.thread_id DESC
LIMIT ?2 OFFSET ?3
                    "#,
                )
                .map_err(|e| serr("准备合并输入候选查询失败", e))?
                .query_map(params![cutoff, page_size_i64, offset], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .map_err(|e| serr("合并输入候选查询失败", e))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| serr("读取合并输入候选行失败", e))?;

            if candidate_rows.is_empty() {
                break;
            }

            let candidate_count = i64::try_from(candidate_rows.len()).unwrap_or(i64::MAX);
            for (thread_id, source_updated_at) in candidate_rows {
                if thread_enabled(&conn, &thread_id)? {
                    selected_keys.push((thread_id, source_updated_at));
                    if selected_keys.len() >= n {
                        break;
                    }
                }
            }

            offset = offset.saturating_add(candidate_count);
        }

        let mut selected = Vec::with_capacity(selected_keys.len());
        for (thread_id, source_updated_at) in selected_keys {
            let row: Option<Stage1OutputRow> = conn
                .prepare(
                    r#"
SELECT
    so.thread_id,
    so.source_updated_at,
    so.raw_memory,
    so.rollout_summary,
    so.rollout_slug,
    so.generated_at,
    so.selected_for_phase2,
    so.selected_for_phase2_source_updated_at
FROM stage1_outputs AS so
WHERE so.thread_id = ?1 AND so.source_updated_at = ?2
                    "#,
                )
                .map_err(|e| serr("准备合并输入行查询失败", e))?
                .query_row(params![thread_id, source_updated_at], stage1_output_row)
                .optional()
                .map_err(|e| serr("合并输入行查询失败", e))?;
            let Some(row) = row else { continue };
            if thread_enabled(&conn, &row.thread_id)? {
                selected.push(row);
            }
        }

        selected.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
        Ok(selected)
    }
}

/// threads 启用门（上游 `enabled_thread_metadata` 的本地投影：登记且
/// `memory_mode='enabled'`）。候选分页与终选水合两处共用同一查询。
fn thread_enabled(conn: &Connection, thread_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM threads WHERE thread_id = ?1 AND memory_mode = 'enabled'",
        params![thread_id],
        |_| Ok(true),
    )
    .optional()
    .map_err(|e| serr("查询会话记忆模式失败", e))
    .map(|hit| hit.unwrap_or(false))
}

/// 单会话与全量快照共用同一行映射，避免字段投影分裂。
fn stage1_output_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Stage1OutputRow> {
    Ok(Stage1OutputRow {
        thread_id: r.get(0)?,
        source_updated_at: r.get(1)?,
        raw_memory: r.get(2)?,
        rollout_summary: r.get(3)?,
        rollout_slug: r.get(4)?,
        generated_at: r.get(5)?,
        selected_for_phase2: r.get::<_, i64>(6)? != 0,
        selected_for_phase2_source_updated_at: r.get(7)?,
    })
}

/// 数据库文件路径（根目录内 `memories.sqlite`）。
pub fn memories_db_path(root: &Path) -> PathBuf {
    root.join("memories.sqlite")
}

/// 单条迁移是否已登记。外层快检与写事务内复核共用同一查询，避免两处漂移。
fn migration_applied(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM schema_migrations WHERE name = ?1",
        params![name],
        |_| Ok(true),
    )
    .optional()
    .map_err(|e| serr("查询迁移状态失败", e))
    .map(|hit| hit.unwrap_or(false))
}

/// 单个迁移＝DDL＋登记**同事务**：任一步失败整体回滚，重开可干净重放。
/// 写事务内先复核登记：并发打开同一新库时，另一连接可能已在调用方快检后
/// 完成该迁移；后到者必须跳过，不能重放无 `IF NOT EXISTS` 的 DDL。
fn apply_migration(
    conn: &mut Connection,
    name: &str,
    sql: &str,
    now: i64,
) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| serr("开启迁移事务失败", e))?;
    let applied = migration_applied(&tx, name)?;
    if applied {
        tx.commit().map_err(|e| serr("提交迁移事务失败", e))?;
        return Ok(());
    }
    tx.execute_batch(sql)
        .map_err(|e| format!("执行迁移 {name} 失败：{e}"))?;
    tx.execute(
        "INSERT INTO schema_migrations (name, applied_at) VALUES (?1, ?2)",
        params![name, now],
    )
    .map_err(|e| serr("登记迁移失败", e))?;
    tx.commit().map_err(|e| serr("提交迁移事务失败", e))?;
    Ok(())
}

/// 上游 `enqueue_global_consolidation_with_executor` 的 SQL（逐字移植，占位符序号形式）。
const ENQUEUE_GLOBAL_CONSOLIDATION_SQL: &str = r#"
INSERT INTO jobs (
    kind,
    job_key,
    status,
    worker_id,
    ownership_token,
    started_at,
    finished_at,
    lease_until,
    retry_at,
    retry_remaining,
    last_error,
    input_watermark,
    last_success_watermark
) VALUES (?1, ?2, 'pending', NULL, NULL, NULL, NULL, NULL, NULL, ?3, NULL, ?4, 0)
ON CONFLICT(kind, job_key) DO UPDATE SET
    status = CASE
        WHEN jobs.status = 'running' THEN 'running'
        ELSE 'pending'
    END,
    retry_at = CASE
        WHEN jobs.status = 'running' THEN jobs.retry_at
        ELSE NULL
    END,
    retry_remaining = max(jobs.retry_remaining, excluded.retry_remaining),
    input_watermark = CASE
        WHEN excluded.input_watermark > COALESCE(jobs.input_watermark, 0)
            THEN excluded.input_watermark
        ELSE COALESCE(jobs.input_watermark, 0) + 1
    END
        "#;

fn enqueue_global_consolidation_on(conn: &Connection, input_watermark: i64) -> Result<(), String> {
    conn.execute(
        ENQUEUE_GLOBAL_CONSOLIDATION_SQL,
        params![
            JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL,
            MEMORY_CONSOLIDATION_JOB_KEY,
            DEFAULT_RETRY_REMAINING,
            input_watermark
        ],
    )
    .map_err(|e| serr("推进全局输入水位失败", e))?;
    Ok(())
}

/// 事务内路径（`Transaction` 解引用为 `Connection`，共用同一实现）。
fn enqueue_global_consolidation_tx(tx: &Transaction<'_>, input_watermark: i64) -> Result<(), String> {
    enqueue_global_consolidation_on(tx, input_watermark)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 事务纪律（D1 确认的四保留之一）：事务内后续语句失败＝drop 回滚，
    /// 已执行写入不得残留。
    #[test]
    fn transaction_failure_rolls_back_all_writes() {
        let tmp = std::env::temp_dir().join(format!(
            "tchat-m1-tx-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let clock: Clock = std::sync::Arc::new(|| 1_700_000_000);
        let store = MemoryStore::open(&tmp, clock).expect("打开应成功");
        let _cleanup = TempDrop(tmp.clone());

        let mut conn = store.lock_conn().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "INSERT INTO jobs (kind, job_key, status, retry_remaining) VALUES ('x', 'y', 'pending', 1)",
            [],
        )
        .unwrap();
        // 制造必败：主键冲突。
        let conflict = tx.execute(
            "INSERT INTO jobs (kind, job_key, status, retry_remaining) VALUES ('x', 'y', 'pending', 2)",
            [],
        );
        assert!(conflict.is_err(), "冲突插入应失败");
        drop(tx); // 未提交即释放＝回滚

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM jobs WHERE kind = 'x'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "回滚后不得残留首条写入");
    }

    /// 审查修复回归：迁移 DDL 与登记同事务——半途失败不得留下部分表或登记行，
    /// 否则崩溃夹缝后重开将重放无 IF NOT EXISTS 的 DDL 而永久失败。
    #[test]
    fn migration_failure_leaves_no_partial_apply() {
        let tmp = std::env::temp_dir().join(format!(
            "tchat-m1-mig-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut conn = Connection::open(memories_db_path(&tmp)).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at INTEGER NOT NULL);",
        )
        .unwrap();
        let _cleanup = TempDrop(tmp.clone());

        // 首句可执行、次句必败的迁移：整体必须回滚。
        let bad = "CREATE TABLE zz_partial (a INTEGER); THIS IS NOT SQL;";
        let result = apply_migration(&mut conn, "zz_bad.sql", bad, 1);
        assert!(result.is_err(), "坏迁移应失败");
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='zz_partial'",
                [],
                |_| Ok(true),
            )
            .optional()
            .unwrap()
            .unwrap_or(false);
        assert!(!table_exists, "半途 DDL 不得残留");
        let registered: bool = conn
            .query_row(
                "SELECT 1 FROM schema_migrations WHERE name = 'zz_bad.sql'",
                [],
                |_| Ok(true),
            )
            .optional()
            .unwrap()
            .unwrap_or(false);
        assert!(!registered, "失败的迁移不得登记为已应用");
    }

    /// 并发迁移回归（S81 实测竞态）：TUI 登记器与后台管线同时打开新库，
    /// 两个连接在事务外都读到“0001 未应用”；先拿写锁者完成后，后到者若
    /// 直接重放无 IF NOT EXISTS 的 DDL 会失败，后台管线随即静默退出。
    /// 因此应用迁移必须在同一写事务内复核登记状态。
    ///
    /// 用第三个连接持有写锁，两个连接都阻塞在 `BEGIN IMMEDIATE`（busy
    /// handler 计数确认二者均已进入），释放后一个先完成、另一个在同一
    /// 写事务内复核并跳过。若复核被移到事务外，二者都会先读到未应用，
    /// 后到者会重放 DDL 而失败；不依赖睡眠或真实时钟。
    #[test]
    fn migration_rechecks_after_concurrent_apply() {
        static BUSY_FIRST: AtomicU64 = AtomicU64::new(0);
        static BUSY_SECOND: AtomicU64 = AtomicU64::new(0);
        fn busy_first(_: i32) -> bool {
            BUSY_FIRST.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn busy_second(_: i32) -> bool {
            BUSY_SECOND.fetch_add(1, Ordering::SeqCst);
            true
        }

        static RACE_DIR: AtomicU64 = AtomicU64::new(0);
        let tmp = loop {
            let n = RACE_DIR.fetch_add(1, Ordering::SeqCst);
            let candidate = std::env::temp_dir().join(format!(
                "tchat-m1-mig-race-{}-{n}",
                std::process::id()
            ));
            match std::fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("创建并发迁移临时目录失败：{e}"),
            }
        };
        let _cleanup = TempDrop(tmp.clone());
        let db = memories_db_path(&tmp);
        let setup = Connection::open(&db).unwrap();
        setup
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (name TEXT PRIMARY KEY, applied_at INTEGER NOT NULL);",
            )
            .unwrap();
        drop(setup);

        let blocker = Connection::open(&db).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let first = Connection::open(&db).unwrap();
        let second = Connection::open(&db).unwrap();
        first.busy_handler(Some(busy_first)).unwrap();
        second.busy_handler(Some(busy_second)).unwrap();

        let h1 = std::thread::spawn(move || {
            let mut conn = first;
            apply_migration(&mut conn, MIGRATIONS[0].0, MIGRATIONS[0].1, 1)
        });
        let h2 = std::thread::spawn(move || {
            let mut conn = second;
            apply_migration(&mut conn, MIGRATIONS[0].0, MIGRATIONS[0].1, 2)
        });

        // 两个连接都进入 BEGIN IMMEDIATE 的 busy 重试后才释放写锁；否则
        // 后到者可能在前者提交后才开始，无法复现“双方先读到未应用”。
        let mut spins = 0u64;
        while BUSY_FIRST.load(Ordering::SeqCst) == 0
            || BUSY_SECOND.load(Ordering::SeqCst) == 0
        {
            spins += 1;
            assert!(spins < 10_000_000, "并发迁移未进入预期的写锁等待");
            std::thread::yield_now();
        }
        blocker.execute_batch("COMMIT").unwrap();

        h1.join().unwrap().expect("连接一应完成迁移或复核跳过");
        h2.join()
            .unwrap()
            .expect("连接二应完成迁移或复核跳过");

        let (count, applied_at): (i64, i64) = blocker
            .query_row(
                "SELECT COUNT(*), MAX(applied_at) FROM schema_migrations WHERE name = ?1",
                params![MIGRATIONS[0].0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "迁移只登记一次");
        assert!(
            applied_at == 1 || applied_at == 2,
            "登记时间应为任一调用方传入的时间，实际 {applied_at}"
        );
    }

    struct TempDrop(std::path::PathBuf);
    impl Drop for TempDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// M8 计数测试共用：临时库＋固定可推进时钟＋经真实 API 播种的两行
    /// stage1 结果。
    struct UsageEnv {
        tmp: std::path::PathBuf,
        clock: std::sync::Arc<std::sync::Mutex<i64>>,
        store: MemoryStore,
    }

    impl UsageEnv {
        fn new(tag: &str, t0: i64) -> Self {
            let tmp = std::env::temp_dir().join(format!(
                "tchat-m8-usage-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&tmp).unwrap();
            let clock: std::sync::Arc<std::sync::Mutex<i64>> =
                std::sync::Arc::new(std::sync::Mutex::new(t0));
            let store = MemoryStore::open(
                &tmp,
                std::sync::Arc::new({
                    let clock = std::sync::Arc::clone(&clock);
                    move || *clock.lock().unwrap()
                }),
            )
            .expect("打开应成功");
            Self { tmp, clock, store }
        }

        fn seed(&self, thread: &str, source: i64) {
            self.store
                .register_thread(thread, "enabled", source * 1000)
                .unwrap();
            let Stage1JobClaimOutcome::Claimed { ownership_token } = self
                .store
                .try_claim_stage1_job(thread, "seed", source, 3600, 8)
                .unwrap()
            else {
                panic!("种子行应可领取：{thread}")
            };
            self.store
                .mark_stage1_job_succeeded(thread, &ownership_token, source, "raw", "summary", None)
                .unwrap();
        }

        fn usage(&self, thread: &str) -> (Option<i64>, Option<i64>) {
            let conn = Connection::open(memories_db_path(&self.tmp)).unwrap();
            conn.query_row(
                "SELECT usage_count, last_usage FROM stage1_outputs WHERE thread_id = ?1",
                params![thread],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        }

        fn advance(&self, secs: i64) {
            *self.clock.lock().unwrap() += secs;
        }
    }

    impl Drop for UsageEnv {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }

    /// M8：有效引用计数＋last_usage 写入（秒，与时钟同步推进）。
    #[test]
    fn m8_record_usage_counts_and_stamps_last_usage() {
        let env = UsageEnv::new("count", 1_700_000_000);
        let id = "auto-20261019T101112-4321".to_string();
        env.seed(&id, 1_699_974_800);
        assert_eq!(env.usage(&id), (None, None), "种子行初始无计数");

        assert_eq!(env.store.record_stage1_output_usage(&[id.clone()]).unwrap(), 1);
        assert_eq!(env.usage(&id), (Some(1), Some(1_700_000_000)));

        env.advance(60);
        assert_eq!(env.store.record_stage1_output_usage(&[id.clone()]).unwrap(), 1);
        assert_eq!(
            env.usage(&id),
            (Some(2), Some(1_700_000_060)),
            "再次记录推进计数并刷新 last_usage"
        );
    }

    /// M8：缺行忽略（未知 ID 不插入、不报错、不计入返回值）；多 ID 单事务。
    #[test]
    fn m8_record_usage_ignores_missing_rows_and_batches() {
        let env = UsageEnv::new("missing", 1_700_000_000);
        let known = "auto-20261019T101112-4321".to_string();
        env.seed(&known, 1_699_974_800);
        let unknown = "auto-20261019T999999-9999".to_string();

        assert_eq!(
            env.store.record_stage1_output_usage(&[unknown.clone()]).unwrap(),
            0,
            "缺行忽略且返回 0"
        );
        assert_eq!(env.usage(&known), (None, None), "他行不受影响");

        assert_eq!(
            env.store
                .record_stage1_output_usage(&[known.clone(), unknown])
                .unwrap(),
            1,
            "仅命中行计入返回值"
        );
        assert_eq!(env.usage(&known), (Some(1), Some(1_700_000_000)));

        assert_eq!(
            env.store.record_stage1_output_usage(&[]).unwrap(),
            0,
            "空列表＝Ok(0)（上游早退）"
        );
    }
}
