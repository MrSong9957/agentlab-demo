//! 013-M7：D2 触发的后台记忆管线运行时＋D9 命令面实现。
//!
//! 上游来源：openai/codex @ ce0323f2f2d3b25e82806cd555237da3db4e1897
//! （Apache-2.0）——`memories/write/src/start.rs::start_memories_startup_task`
//! 的编排顺序（store 可用→layout→seed→prune→配额→phase1→phase2）与
//! `control.rs::clear_memory_roots_contents` 的重置语义。本地适配：
//! ①触发点＝每个带输入、通过 route/origin 门并成功启动的新用户轮
//! （手册 §7.1 `turn_processor` 三条件的本地对等，D2 批准边界）；
//! ②异步任务＝独立 `std::thread`（进程退出不 join＝「不保证完成」）；
//! ③prune 每次启动清理一批（M9），配额检查无接口（D8）跳过；
//! ④后台完全静默（上游 `tracing::warn` 无本地日志面——失败不上屏、
//! 不写文件，状态经 `/memory-status` 观察）；⑤重置沿用 control.rs 语义
//! （清记忆根内容、保留根目录、拒绝符号链接根）——本地 `memories.sqlite`
//! 在记忆根内（M1 布局），清根内容＝生成面＋状态面一次达成，sessions
//! 目录（原始会话）不在根内、天然保留。
//!
//! 会话登记（threads 写入侧）：`ThreadRegistrar` 在每个入史点回调后以
//! `(stem, durable_len)` 变化检测推进 `updated_at_ms`（毫秒，会话写入侧
//! 事实——`list_sessions` 仅返回名称与字节数，不能当时间用；秒级
//! `source_updated_at` 换算沿用 M2 `div_euclid(1000)` 语义，不在本层重复）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::api;
use crate::catalog::{self, EffectiveConnection, ModelInfo};
use crate::memory::config::MemoriesConfig;
use crate::memory::phase2::{self, ThreadFileMeta};
use crate::memory::store::{LatestErrorJob, MemoryStore};
use crate::session::Session;

/// 上游 `templates/extensions/ad_hoc/instructions.md` 原文（逐字节拷贝）。
const AD_HOC_INSTRUCTIONS: &str = include_str!("templates/extensions_ad_hoc_instructions.md");

// ---------- 013-M7 收口：重置失效边界（2026-10-19 用户确认） ----------
//
// `/memory-reset confirm` 成功返回后，重置前启动的旧任务不得再回写生成
// 记忆或重建基线。承载＝`.epoch` 代数文件＋根目录 flock＋各任务领取时
// 捕获的**内存代数快照**（task_epoch：stage1 每任务前、phase2 领取同
// 临界区、mem_write 持任务快照在共享锁内校验——phase2.rs:163、
// mem_tools.rs:378-398、workspace.rs:365-367；014 §4 勘误：历史上曾述
// 的共享凭据文件 `.pipeline-epoch` 不存在）。写入有效性＝代数文件与
// 快照一致；写入入口（mem_write/sync/git 收尾/首次建库）在共享锁内
// 「检查＋写入」同一临界区，与 reset 的排他清空互斥——排除「检查通过
// →reset 返回→旧任务才写入」的交错竞态；跨进程有效（flock 与代数文件
// 均为共享介质）。代际失配的旧任务在阶段边界（首次建库/每任务前/
// phase2 前/收尾前）中止；SQL 记账落其已打开连接（重置后＝已删 inode，
// 不污染新库）。

/// 重置代数文件名（记忆根内）。
pub const EPOCH_FILE: &str = ".epoch";

fn read_epoch_file(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// 当前重置代数（无 `.epoch`＝0——尚未发生任何重置）。
pub fn read_epoch(root: &Path) -> u64 {
    read_epoch_file(&root.join(EPOCH_FILE)).unwrap_or(0)
}

/// 写入代数（tmp＋rename 原子替换）。
fn write_epoch(root: &Path, v: u64) -> Result<(), String> {
    let path = root.join(EPOCH_FILE);
    let tmp = root.join(format!("{EPOCH_FILE}.tmp-{}", std::process::id()));
    crate::catalog::write_private_file(&tmp, &v.to_string())
        .map_err(|e| format!("写入重置代数失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("重置代数落位失败：{e}"))
}

/// 根目录 flock（跨进程互斥；Unix 目录 fd 可 flock）。
#[cfg(unix)]
fn flock_root(root: &Path, exclusive: bool) -> Option<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::File::open(root).ok()?;
    let ok = unsafe {
        libc::flock(
            f.as_raw_fd(),
            if exclusive {
                libc::LOCK_EX
            } else {
                libc::LOCK_SH
            },
        ) == 0
    };
    if ok {
        Some(f)
    } else {
        None
    }
}

/// 共享写守卫（RAII）：持有期间 reset 的排他清空被互斥——「检查代数＋
/// 实际修改」由此构成原子域（任务凭据绑定/prepare/同步/Git 收尾共用）。
#[cfg(unix)]
pub struct SharedWriteGuard {
    _file: std::fs::File,
}

/// 取得根目录共享锁（写入临界区入口；跨进程有效）。失败返回 None——
/// 调用方按无法进入临界区处理（中止该次写入）。
#[cfg(unix)]
pub fn lock_shared(root: &Path) -> Option<SharedWriteGuard> {
    flock_root(root, false).map(|_file| SharedWriteGuard { _file })
}

/// 首次建库守卫：共享锁内「检查代数＋建库」同临界区（防检查-建库窗口内
/// reset）；代际失配返回 None（调用方中止）。Windows 无 flock——退化为
/// 仅检查（登记限制，见记录）。
fn open_store_if_valid(root: &Path, epoch: u64, clock: crate::memory::Clock) -> Option<MemoryStore> {
    #[cfg(unix)]
    {
        let _lock = flock_root(root, false);
        if read_epoch(root) != epoch {
            return None;
        }
        return MemoryStore::open(root, clock).ok();
    }
    #[cfg(not(unix))]
    {
        if read_epoch(root) != epoch {
            return None;
        }
        MemoryStore::open(root, clock).ok()
    }
}

// ---------- 013-M7 收口：后台触发直接计数 ----------
//
// 生产后台启动入口（`spawn_memory_pipeline` 生产壳，enabled gate 后）的
// 进程内原子计数——`/memory-status` 可见（费用知情的可观察面），非日志
// 系统/持久化审计/环境变量后门；跨进程不可见（每进程各自计数，如实）。

static PIPELINE_SPAWNS: AtomicU64 = AtomicU64::new(0);

/// 本进程累计后台管线启动次数（每用户轮成功启动＋1；重复触发各计一次）。
pub fn pipeline_spawn_count() -> u64 {
    PIPELINE_SPAWNS.load(Ordering::Relaxed)
}

/// 记忆根（M1 布局：`<user_dir>/memories`）。
pub fn memories_root(user_dir: &Path) -> PathBuf {
    user_dir.join("memories")
}

/// 装载当前 `memories` 配置段：manual.json 无段＝默认（`enabled=false`），
/// 装载失败按默认（会话不因配置错误中断——与 M6 读取侧同一宽容面）。
pub fn load_memories_config() -> MemoriesConfig {
    load_memories_config_from(&catalog::config_dir())
}

/// 显式配置目录版（测试隔离入口；生产行 [`load_memories_config`]）。
pub fn load_memories_config_from(config_dir: &Path) -> MemoriesConfig {
    catalog::load_manual(config_dir)
        .ok()
        .and_then(|m| m.memories)
        .unwrap_or_default()
}

/// 后台管线的模型发送面（D6：当前 `selected_model`，由触发轮的 route 快照）。
#[derive(Clone)]
pub struct ModelSend {
    pub provider_id:String,
    pub conn: EffectiveConnection,
    pub key: String,
    pub model: ModelInfo,
    pub agent: ureq::Agent,
}

/// D2 触发（生产壳）：带输入新用户轮成功启动后调用，sessions 目录取
/// 环境解析值。`enabled=false` 零副作用（不 spawn、不建目录或数据库）；
/// 后台静默、失败不上屏；进程退出不等待。
///
/// 重复触发无害：stage1 领取按 `max_running_jobs` 挡、phase2 经全局任务
/// 锁互斥（M1/M2/M5 既有契约承接，不另造调度框架）。
pub fn spawn_memory_pipeline(
    user_dir: &Path,
    config: &MemoriesConfig,
    current_thread_id: Option<String>,
    send: ModelSend,
) {
    if let Some(_handle) = spawn_memory_pipeline_in(
        user_dir,
        &crate::session::sessions_dir(),
        config,
        current_thread_id,
        send,
    ) {
        // 实际后台启动入口计数（enabled gate 已过＝一次真实启动）
        PIPELINE_SPAWNS.fetch_add(1, Ordering::Relaxed);
    }
}

/// 可测核心：显式 sessions 目录；返回后台线程句柄（生产壳丢弃＝退出
/// 不等待；测试用 `join` 有界等待）。`enabled=false` 返回 `None`。
#[must_use]
pub fn spawn_memory_pipeline_in(
    user_dir: &Path,
    sessions_dir: &Path,
    config: &MemoriesConfig,
    current_thread_id: Option<String>,
    send: ModelSend,
) -> Option<std::thread::JoinHandle<()>> {
    if !config.enabled {
        return None;
    }
    let root = memories_root(user_dir);
    let config = config.clone();
    let sessions_dir = sessions_dir.to_path_buf();
    Some(std::thread::spawn(move || {
        let chat = |body: &serde_json::Value| {
            api::chat_with_provider_body(&send.provider_id,&send.conn,&send.key,&send.model,body,&send.agent)
        };
        run_memory_pipeline_once(
            &root,
            &sessions_dir,
            &config,
            current_thread_id.as_deref().unwrap_or(""),
            &send.model,
            &chat,
            crate::memory::store::system_clock(),
        )
    }))
}

/// 单次管线（生产后台与确定性宿主共用；仅模型边界与 Clock 可替换）。
/// 上游 `start.rs` 编排序的本地承载。`epoch`＝启动时重置
/// 代数——各阶段边界比对，失配（期间发生过重置）即静默中止：旧任务
/// 不得再准备、建库或生成（重置失效边界，2026-10-19 确认）。
pub fn run_memory_pipeline_once(
    root: &Path,
    sessions_dir: &Path,
    config: &MemoriesConfig,
    current_thread_id: &str,
    model: &ModelInfo,
    chat: &dyn Fn(&serde_json::Value) -> Result<serde_json::Value, String>,
    clock: crate::memory::Clock,
) {
    if !config.enabled {
        return;
    }
    let params1 = crate::memory::candidates::params_from_config(config);
    let params2 = phase2::Phase2Params::from_config(config);
    let epoch = read_epoch(root);
    // 上游同序：store 不可用→跳过（warn 静默化）；首次建库在共享锁内
    // 「检查＋建库」同临界区（不得在重置后重建状态库）
    let store = match open_store_if_valid(root, epoch, clock) {
        Some(s) => s,
        None => return,
    };
    if read_epoch(root) != epoch {
        return;
    }
    // seed extension instructions（上游原序；create_new 已存在跳过）——
    // 「复核代数＋播种」在共享锁内同一临界区（重置后旧任务不得补种生成
    // 文件；检查与写不分离）
    {
        let _guard = lock_shared(root);
        if read_epoch(root) != epoch {
            return;
        }
        let _ = seed_ad_hoc_instructions(root);
    }
    if read_epoch(root) != epoch {
        return;
    }
    // M9：seed 后、候选领取前只清理一批；失败不阻断后续阶段（上游同款）。
    // 检查代数与实际 DELETE 在同一根共享锁临界区；不持锁进入模型请求。
    {
        let Some(_guard) = lock_shared(root) else {
            return;
        };
        if read_epoch(root) != epoch {
            return;
        }
        let _ = store.prune_stage1_outputs_for_retention(
            params2.max_unused_days,
            crate::memory::retention::PRUNE_BATCH_SIZE,
        );
    }
    // 配额无接口（D8）：跳过。
    // phase1：startup 扫描领取＋逐个提炼（每任务前复核代数）
    let claims = match crate::memory::candidates::claim_stage1_jobs_for_startup(
        &store,
        sessions_dir,
        current_thread_id,
        params1,
    ) {
        Ok(c) => c,
        Err(_) => return,
    };
    for claim in &claims {
        if read_epoch(root) != epoch {
            return; // 重置后旧任务不得继续提炼
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let _ = crate::memory::phase1::run_stage1_job(
            &store,
            claim,
            model,
            &cwd.display().to_string(),
            chat,
        );
    }
    if read_epoch(root) != epoch {
        return; // 重置后旧任务不得进入 phase2（重新 prepare/建基线）
    }
    // phase2：全局合并（真实会话水合，非合成成功数据）
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolve = |thread_id: &str| resolve_thread_meta(sessions_dir, &cwd, thread_id);
    let _ = phase2::run_phase2_consolidation(
        &store,
        root,
        "memory-runner",
        params2,
        &resolve,
        chat,
        None,
    );
}

/// 上游 `extensions::seed_extension_instructions`（ad_hoc）：在记忆根内
/// 播种扩展说明（`create_new`——已存在跳过，不覆盖）。
fn seed_ad_hoc_instructions(root: &Path) -> std::io::Result<()> {
    let dir = root.join("extensions").join("ad_hoc");
    std::fs::create_dir_all(&dir)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("instructions.md"))
    {
        Ok(mut file) => std::io::Write::write_all(&mut file, AD_HOC_INSTRUCTIONS.as_bytes()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// phase2 `resolve_thread` 的真实会话水合（M5 合成解析器的生产替换）：
/// `rollout_path`＝sessions 目录下该 stem 的 jsonl；`cwd`＝当前进程目录、
/// `git_branch`＝None（会话工作目录与分支未持久化——M3 登记差异延续）。
/// 未登记/文件不存在→None（调用方按同步失败处理，不静默丢行）。
pub fn resolve_thread_meta(
    sessions_dir: &Path,
    cwd: &Path,
    thread_id: &str,
) -> Option<ThreadFileMeta> {
    let rollout_path = sessions_dir.join(format!("{thread_id}.jsonl"));
    if !rollout_path.is_file() {
        return None;
    }
    Some(ThreadFileMeta {
        rollout_path,
        cwd: cwd.to_path_buf(),
        git_branch: None,
    })
}

/// threads 登记推进器：缓存库连接，以 `(stem, durable_len)` 变化检测
/// 「确有新持久化事实」才 upsert（`updated_at_ms`＝当前墙钟毫秒——
/// 生产语义与 session 文件名时间戳同源的实时事实，测试经目录隔离）。
/// 登记失败静默（不阻塞主循环；下次变化重试）。
#[derive(Default)]
pub struct ThreadRegistrar {
    store: Option<MemoryStore>,
    last: Option<(String, usize)>,
    clock: Option<crate::memory::Clock>,
}

impl ThreadRegistrar {
    /// 确定性宿主/验收使用同一秒级 Clock；生产默认保留原毫秒写入时间。
    pub fn with_clock(clock: crate::memory::Clock) -> Self {
        Self {
            clock: Some(clock),
            ..Default::default()
        }
    }

    /// 在每个入史点回调（`on_history`）之后调用：仅当会话有追加目标且
    /// `(stem, durable_len)` 相对上次登记变化时推进。`enabled=false` 或
    /// 无目标＝零副作用（不建目录或数据库）；只读操作与自动注入块不产生
    /// 新持久化记录，天然不推进。
    pub fn note_persisted(&mut self, user_dir: &Path, config: &MemoriesConfig, session: &Session) {
        if !config.enabled {
            return;
        }
        let Some(stem) = session.current_target_stem() else {
            return;
        };
        let durable = session.durable_history_len();
        if self
            .last
            .as_ref()
            .is_some_and(|(s, d)| *s == stem && *d == durable)
        {
            return;
        }
        let root = memories_root(user_dir);
        let store = match &mut self.store {
            Some(s) => s,
            None => match MemoryStore::open(
                &root,
                self.clock
                    .clone()
                    .unwrap_or_else(crate::memory::store::system_clock),
            ) {
                Ok(s) => self.store.insert(s),
                Err(_) => return,
            },
        };
        let now_ms = self
            .clock
            .as_ref()
            .map(|clock| clock().saturating_mul(1000))
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            });
        let mode = if config.generate_memories {
            "enabled"
        } else {
            "disabled"
        };
        if store.register_thread(&stem, mode, now_ms).is_ok() {
            self.last = Some((stem, durable));
        }
    }
}

/// 进程内共享句柄（两驱动各持一份；登记器自身无跨线程锁——调用方在
/// autosave 锁内串行访问）。
pub type SharedRegistrar = Mutex<ThreadRegistrar>;

/// D9 `/memory-status`（生产壳）：读当前配置段。
pub fn memory_status_text(user_dir: &Path) -> String {
    memory_status_text_in(user_dir, &load_memories_config())
}

/// 显式配置版（测试隔离入口；只读——不启动后台任务、不建目录）。
pub fn memory_status_text_in(user_dir: &Path, config: &MemoriesConfig) -> String {
    let root = memories_root(user_dir);
    let mut out = format!(
        "记忆功能：{}（use_memories={}）",
        if config.enabled {
            "已启用"
        } else {
            "未启用"
        },
        config.use_memories
    );
    out.push_str(&format!("\n记忆根：{}", root.display()));
    // 触发计数（费用知情的可观察面——本进程累计，与库状态无关）
    out.push_str(&format!(
        "\n本进程累计触发后台管线：{} 次",
        pipeline_spawn_count()
    ));
    if !root.is_dir() {
        out.push_str("\n生成记忆：未初始化（记忆根不存在）");
        return out;
    }
    out.push_str(&format!(
        "\n总摘要：{}",
        if root.join("memory_summary.md").is_file() {
            "存在"
        } else {
            "不存在"
        }
    ));
    // 只读边界：无库不因 status 建库（open 即建库＋迁移，须先检查文件存在）
    if !root.join("memories.sqlite").is_file() {
        out.push_str("\n状态库：未初始化");
        return out;
    }
    match MemoryStore::open(&root, crate::memory::store::system_clock()) {
        Ok(store) => {
            let outputs = store.count_stage1_outputs().unwrap_or(0);
            let (running, succeeded, failed) = store.count_jobs_by_status().unwrap_or((0, 0, 0));
            out.push_str(&format!(
                "\n第一阶段产物：{outputs} 条\n后台任务：运行中 {running}／成功 {succeeded}／失败等待重试 {failed}"
            ));
            // 014 §2：最小失败展示——阶段＋脱敏截断原因＋剩余重试次数＋
            // `jobs.retry_at` 本值（下次可重试时间，不以剩余次数替代）；
            // 合并冷却与领取原语同源推导（不可推导不显示）。只读，不建库。
            if let Some(err) = store.latest_error_job().ok().flatten() {
                let LatestErrorJob {
                    kind,
                    last_error,
                    retry_at,
                    retry_remaining,
                } = err;
                let stage = match kind.as_str() {
                    crate::memory::store::JOB_KIND_MEMORY_STAGE1 => "第一阶段",
                    crate::memory::store::JOB_KIND_MEMORY_CONSOLIDATE_GLOBAL => "全局合并",
                    other => other,
                };
                // 先脱敏后截断：截断可能切开密钥前缀使正则失效，顺序不可颠倒。
                // 截断复用展示边界 display::truncate_chars（内容 ≤200 字符），
                // 超限时追加 `…` 标注（展示总长 201——规格口径为内容上限）。
                let raw = last_error.unwrap_or_else(|| "（无原因记录）".into());
                let reason = crate::memory::secrets::redact_secrets(raw);
                let reason = if reason.chars().count() > 200 {
                    let mut t = crate::display::truncate_chars(&reason, 200);
                    t.push('…');
                    t
                } else {
                    reason
                };
                let retry_at_text = match retry_at {
                    Some(at) => {
                        let now = (crate::memory::store::system_clock())();
                        if at > now {
                            format!("{at}（还有 {} 分钟）", (at.saturating_sub(now) + 59) / 60)
                        } else {
                            format!("{at}（现在可重试）")
                        }
                    }
                    None => "—".into(),
                };
                out.push_str(&format!(
                    "\n最近失败：{stage}｜剩余重试 {retry_remaining}｜下次可重试：{retry_at_text}｜原因：{reason}"
                ));
            }
            if let Some(secs) = store.phase2_success_cooldown_remaining().ok().flatten() {
                out.push_str(&format!(
                    "\n合并冷却：剩 {} 分钟（上次成功后 6 小时内不再领取）",
                    (secs + 59) / 60
                ));
            }
        }
        Err(e) => out.push_str(&format!("\n状态库：无法读取（{e}）")),
    }
    out
}

/// D9 `/memory-reset confirm`：显式确认后清生成记忆（含状态库——本地库
/// 在记忆根内），保留根目录与原始会话（sessions 不在根内）。
/// 缺少 `confirm` 不执行任何清理；符号链接根拒绝（上游 control.rs 边界）。
/// 重置失效边界（2026-10-19 确认）：清空与代数自增在根目录**排他锁**内
/// 完成——成功返回后，重置前启动的旧任务不得再回写或重建（其写入入口
/// 的「检查＋写入」在共享锁内，与本清空互斥）。
pub fn memory_reset(user_dir: &Path, args: &str) -> String {
    if args.trim() != "confirm" {
        return "用法：/memory-reset confirm（清理生成记忆与状态、保留原始会话；缺少 confirm 不执行）".into();
    }
    let root = memories_root(user_dir);
    match std::fs::symlink_metadata(&root) {
        Ok(m) if m.file_type().is_symlink() => {
            return format!("错误：记忆根是符号链接，拒绝清理（{}）", root.display());
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return "记忆根不存在，无需清理（原始会话不受影响）".into();
        }
        Err(e) => return format!("错误：读取记忆根元数据失败：{e}"),
    }
    // 排他锁内：清空＋代数自增（与一切写入者的共享锁互斥——返回后旧任务
    // 的配对凭据必然失配，写入入口一律拒绝）
    #[cfg(unix)]
    let _excl = flock_root(&root, true);
    let next_epoch = read_epoch(&root) + 1;
    let mut cleared = 0usize;
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) => return format!("错误：读取记忆根失败：{e}"),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ok = match entry.file_type() {
            Ok(t) if t.is_dir() => std::fs::remove_dir_all(&path).is_ok(),
            Ok(_) => std::fs::remove_file(&path).is_ok(),
            Err(_) => false,
        };
        if ok {
            cleared += 1;
        }
    }
    if let Err(e) = write_epoch(&root, next_epoch) {
        return format!("错误：{e}");
    }
    // 如实报告：清理后以真实目录状态复核（`.epoch` 是本次写入的重置代数
    // 标记，不属生成内容），不虚报成功
    let remaining = std::fs::read_dir(&root)
        .map(|it| {
            it.filter(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|e| e.file_name().into_string().ok())
                    .is_some_and(|n| n != EPOCH_FILE)
            })
            .count()
        })
        .unwrap_or(0);
    if remaining == 0 {
        format!("已清理生成记忆（{cleared} 项；重置前启动的后台任务已失效，原始会话不受影响）")
    } else {
        format!("部分清理：{cleared} 项已删，仍残留 {remaining} 项（可重试）")
    }
}
