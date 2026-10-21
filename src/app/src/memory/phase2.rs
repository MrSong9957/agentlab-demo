//! 013-M5：第二阶段合并（全局协调＋来源选择＋差异驱动合并＋写后校验＋
//! 基线重置＋成功/失败记账）。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——`memories/write/src/phase2.rs::run`（编排序与失败原因
//! 枚举逐字保留）、`mod job`（claim/failed/succeed 记账语义）、
//! `mod stage_two` 常量（租期 3,600s/心跳 90s/重试延迟 3,600s）、
//! `prompts.rs::build_consolidation_prompt_for_version`（V1 模板渲染）。
//!
//! 适配（013 §6 M5，已登记）：①合并代理＝参数化工具集的
//! [`crate::turn::run_tool_turn_with_mode`]（`ToolMode::MemoryMerge`——恰
//! `mem_read/mem_write/mem_list` 三工具，执行分派层拒绝一切未注册工具，
//! 含 exec_command/apply_patch）；②心跳挂点＝每次模型请求前经 sender 闭包
//! 续租＋校验通过后再确认一次（上游为事件循环周期心跳；本项目模型调用为
//! 同步阻塞，请求前续租为最细可行粒度）；③上游 `failed_spawn_agent`/
//! `failed_sandbox_policy` 无对应概念（无独立子进程与沙箱配置面）——合并轮
//! 创建失败并入 `failed_agent`（D3 边界即隔离承载）；④线程元数据（threads
//! 启用门在 SQL 内、`rollout_path`/`cwd`/`git_branch` 水合）经注入解析器
//! ——M7 接登记写侧，本轮测试用合成解析器；⑤合并轮会话为纯内存态（不落
//! `sessions/`、不注入记忆、不写用户会话文件——`on_record` 恒 no-op）；
//! ⑥非原子语义如实保留（上游 §9.8）：部分文件变化不回滚、reset 后 SQL
//! 失败不虚报成功（`sql_committed=false`）、失权提交仅记录不补偿。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::memory::config::MemoriesConfig;
use crate::memory::store::{MemoryStore, Phase2JobClaimOutcome, SelectedOutput, Stage1OutputRow};
use crate::prompt::HistoryItem;

/// 全局合并任务租期秒数（上游 `stage_two::JOB_LEASE_SECONDS`）。
pub const JOB_LEASE_SECONDS: i64 = 3_600;
/// 心跳间隔秒数（上游 `stage_two::JOB_HEARTBEAT_SECONDS`）：合并代理执行
/// 期间（含模型请求阻塞期间）按受控时钟每 ≥该间隔续租一次——上游事件
/// 循环周期心跳的本地承载（请求前另有确定性续租，见 run 内 ①）。
pub const JOB_HEARTBEAT_SECONDS: i64 = 90;
/// 心跳线程真实轮询间隔（读受控时钟；非业务时序，仅采样密度）。
const HEARTBEAT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);
/// 失败重试延迟秒数（上游 `stage_two::JOB_RETRY_DELAY_SECONDS`）。
pub const JOB_RETRY_DELAY_SECONDS: i64 = 3_600;

/// 合并代理系统提示（本地包装层）：角色框定＋已登记适配注记（013 §2.2
/// 模板假设核对——模板原文不改，此处追加「无脚本执行能力」说明）。
pub const CONSOLIDATION_SYSTEM: &str = "You are the memory consolidation agent. Consolidate the memory workspace strictly with the provided file tools (mem_read, mem_write, mem_list) inside the memory root.\nNote: 本环境无脚本执行能力；skills 可含说明/模板文件，不含可执行脚本。";

/// 合并模板（上游 `templates/memories/consolidation.md` 原文，逐字节拷贝）。
const CONSOLIDATION_TEMPLATE: &str = include_str!("templates/consolidation.md");

/// 合并轮参数（上游填参点 `config.memories.max_raw_memories_for_consolidation`
/// 与 `max_unused_days`；经 [`MemoriesConfig::clamped`] 钳制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phase2Params {
    pub max_raw_memories: usize,
    pub max_unused_days: i64,
}

impl Phase2Params {
    pub fn from_config(config: &MemoriesConfig) -> Self {
        let clamped = config.clamped();
        Phase2Params {
            max_raw_memories: clamped.max_raw_memories_for_consolidation,
            max_unused_days: clamped.max_unused_days,
        }
    }
}

/// 线程文件元数据（上游自 state 库 threads 行水合；本地经注入解析器，
/// M7 接登记写侧）。`None`＝未登记/禁用——该行不进合并输入（选择 SQL
/// 的 threads 启用门已过滤；解析器 `None` 视为同步失败，不静默丢行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadFileMeta {
    pub rollout_path: PathBuf,
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
}

/// 一轮合并的结果（可观测面；成败以数据库与基线真实状态为准）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase2Outcome {
    /// 领取本身失败（存储错误；上游仅计数返回，不进失败记账）。
    ClaimFailed,
    SkippedRetryUnavailable,
    SkippedRunning,
    SkippedCooldown,
    /// 无差异短路（A17）：不调模型、不 reset；`sql_committed`＝成功记账
    /// 是否落库（失权时 false——不虚报）。
    SucceededNoWorkspaceChanges {
        sql_committed: bool,
    },
    /// 完整合并成功；`sql_committed` 同上（reset 已发生、SQL 失权未落库时
    /// false——非原子语义如实呈现，不补偿不虚报）。
    Succeeded {
        sql_committed: bool,
    },
    /// 校验通过后再确认时发现失权：不 reset、不成功记账、不失败记账
    /// （上游「do nothing」）。
    LostOwnershipAfterValidation,
    /// 领取后失败（上游失败原因枚举；已按 retry 记账，失权时走兜底）。
    Failed(&'static str),
}

/// 渲染合并代理输入提示（上游 `build_consolidation_prompt_for_version`
/// V1 路径）：四个占位符一次性替换（元数据先行；本地 V1 无 extensions
/// 概念，两个 extensions 块恒为空串——`memory_root`/差异文件名多次出现，
/// 全量替换）。
pub fn build_consolidation_prompt(memory_root: &Path) -> String {
    CONSOLIDATION_TEMPLATE
        .replace("{{ memory_root }}", &memory_root.display().to_string())
        .replace("{{ memory_extensions_folder_structure }}", "")
        .replace("{{ memory_extensions_primary_inputs }}", "")
        .replace(
            "{{ phase2_workspace_diff_file }}",
            crate::memory::workspace::WORKSPACE_DIFF_FILENAME,
        )
}

/// 运行一轮全局合并（上游 `phase2::run` 编排序）：
///
/// 领取全局锁 → prepare/ensure → 来源选择（快照）→ 同步（M4）→ diff →
/// 无差异且产物有效短路（不调模型、不 reset）→ 写差异文件 → 合并代理
/// （受限工具轮＋每请求心跳）→ 写后校验 → 所有权再确认 → reset 基线 →
/// 带所有权保护的 SQL 成功提交。
///
/// `chat` 为模型发送闭包（`{messages, tools}` 形状，返回完整响应 JSON——
/// 与 `run_tool_turn` sender 同契约，内部经真实请求组装与工具分派）；
/// `cancel` 传入合并轮与 git 子进程（用户取消→代理轮 Err→`failed_agent`，
/// 与上游 Interrupted 同路）。
pub fn run_phase2_consolidation(
    store: &MemoryStore,
    root: &Path,
    worker_id: &str,
    params: Phase2Params,
    resolve_thread: &dyn Fn(&str) -> Option<ThreadFileMeta>,
    chat: &dyn Fn(&Value) -> Result<Value, String>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Phase2Outcome {
    use crate::memory::storage_files::Stage1FileInput;
    use crate::memory::workspace;

    // 1. 先领取全局锁，再触碰工作区（上游编排序）。013-M7 核查轮：
    // 「领取＋任务代数快照绑定＋检查＋prepare」在根目录共享锁内同一
    // 临界区——reset 的排他清空无法插入领取与绑定之间（旧任务不可能把
    // 重置后的新代数读作自身凭据），也无法插入检查与 prepare 之间。
    let claim_guard = crate::memory::runner::lock_shared(root);
    let (ownership_token, claim_watermark) =
        match store.try_claim_global_phase2_job(worker_id, JOB_LEASE_SECONDS) {
            Ok(Phase2JobClaimOutcome::Claimed {
                ownership_token,
                input_watermark,
            }) => (ownership_token, input_watermark),
            Ok(Phase2JobClaimOutcome::SkippedRetryUnavailable) => {
                return Phase2Outcome::SkippedRetryUnavailable
            }
            Ok(Phase2JobClaimOutcome::SkippedRunning) => return Phase2Outcome::SkippedRunning,
            Ok(Phase2JobClaimOutcome::SkippedCooldown) => return Phase2Outcome::SkippedCooldown,
            Err(_) => return Phase2Outcome::ClaimFailed,
        };

    // 1a. 任务代数快照：**在领取所在的同一临界区内绑定**（不可变任务
    // 凭据；与共享的当前代数分立——新代任务的任何动作不会让旧任务凭据
    // 恢复）。
    let task_epoch = crate::memory::runner::read_epoch(root);
    if crate::memory::runner::read_epoch(root) != task_epoch {
        drop(claim_guard);
        return job_failed(store, &ownership_token, "failed_reset_invalidated");
    }
    // 2. prepare/ensure（布局＋残留差异文件清理＋基线可用性）——仍在
    // 同一临界区（检查与实际修改不分离）
    if workspace::prepare_memory_workspace(root, cancel).is_err() {
        drop(claim_guard);
        return job_failed(store, &ownership_token, "failed_prepare_workspace");
    }
    drop(claim_guard);

    // 3. 来源选择（本轮快照——后续入库的新版本不冒充已处理）
    let raw_memories =
        match store.get_phase2_input_selection(params.max_raw_memories, params.max_unused_days) {
            Ok(rows) => rows,
            Err(_) => return job_failed(store, &ownership_token, "failed_load_stage1_outputs"),
        };
    let new_watermark = get_watermark(claim_watermark, &raw_memories);

    // 4. 同步（M4 `sync_stage1_snapshot`：先摘要后 raw＋prune＋symlink 清理）
    let inputs: Result<Vec<Stage1FileInput>, String> = raw_memories
        .iter()
        .map(|row| {
            let meta = resolve_thread(&row.thread_id).ok_or_else(|| {
                format!(
                    "线程 {} 缺少文件元数据（threads 登记未水合）",
                    row.thread_id
                )
            })?;
            Stage1FileInput::from_row(row.clone(), meta.rollout_path, meta.cwd, meta.git_branch)
                .map_err(|e| e.to_string())
        })
        .collect();
    let inputs = match inputs {
        Ok(inputs) => inputs,
        Err(_) => return job_failed(store, &ownership_token, "failed_sync_workspace_inputs"),
    };
    {
        // 核查轮：「检查＋Markdown 同步」同临界区（reset 无法插入其间）
        let Some(_g) = crate::memory::runner::lock_shared(root) else {
            return job_failed(store, &ownership_token, "failed_sync_workspace_inputs");
        };
        if crate::memory::runner::read_epoch(root) != task_epoch {
            return job_failed(store, &ownership_token, "failed_reset_invalidated");
        }
        if crate::memory::storage_files::sync_stage1_snapshot(root, &inputs, &mut |_| {}).is_err() {
            return job_failed(store, &ownership_token, "failed_sync_workspace_inputs");
        }
        crate::memory::retention::prune_old_extension_resources(root, store.now(), &mut |_| {});
    }

    // 5. 差异（含同一条前置暂存与排除集）
    let workspace_diff = match workspace::memory_workspace_diff(root, cancel) {
        Ok(diff) => diff,
        Err(_) => return job_failed(store, &ownership_token, "failed_workspace_status"),
    };

    // 6. 无差异短路（A17）：产物有效才走捷径——照写成功水位与 selected 快照，
    //    不调模型、不 reset 基线。
    if !workspace_diff.has_changes() && workspace::validate_consolidation_artifacts(root).is_ok() {
        let selected = selected_outputs(&raw_memories);
        let sql_committed = store
            .mark_global_phase2_job_succeeded(&ownership_token, new_watermark, &selected)
            .unwrap_or(false);
        return Phase2Outcome::SucceededNoWorkspaceChanges { sql_committed };
    }

    // 7. 写差异文件（代理的第一输入）
    if workspace::write_workspace_diff(root, &workspace_diff).is_err() {
        return job_failed(store, &ownership_token, "failed_workspace_diff_file");
    }

    // 8. 合并代理：受限工具轮（恰三项 mem_*；分派层拒未注册工具），
    //    临时内存会话（不落 sessions/、不注入记忆）。
    //    心跳生命周期＝上游「代理执行期间周期续租」的同步阻塞承载：
    //    ①每请求前确定性续租一次；②**请求阻塞期间**由作用域内心跳线程按
    //    受控时钟每 JOB_HEARTBEAT_SECONDS 续租（请求未完成租期照常推进——
    //    上游事件循环周期心跳的等价行为；线程**唯一**退出条件＝停止位，
    //    无任何独立于请求生命周期的提前退出）；③请求返回（正常/错误）即
    //    停止并 join；panic 展开经 Drop 保护置停止位，`thread::scope` 展开
    //    路径 join 收口（线程 ≤一个轮询周期退出，不挂死）。存储错误容忍
    //    ——上游对心跳失败仅记录。
    let prompt = build_consolidation_prompt(root);
    let mut history: Vec<HistoryItem> = Vec::new();
    let turn_result = std::thread::scope(|scope| {
        /// panic 展开＝置停止位（正常路径由显式 store 置位；Drop 兜底展开）。
        struct StopHeartbeatOnDrop(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for StopHeartbeatOnDrop {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let mut sender = |body: &Value| -> Result<Value, String> {
            let _ = store.heartbeat_global_phase2_job(&ownership_token, JOB_LEASE_SECONDS);
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_flag = std::sync::Arc::clone(&stop);
            let token = ownership_token.as_str();
            let heartbeat = scope.spawn(move || {
                // 初值＝i64::MIN：线程首查必续租一次（与①的请求前续租幂等重复，
                // 无害）——若用当前时钟初始化，线程启动与受控时钟推进之间存在
                // 竞态窗口，会跳过首个 90s 点（测试实测捕获）
                let mut last = i64::MIN;
                while !stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    let now = store.now();
                    if now.saturating_sub(last) >= JOB_HEARTBEAT_SECONDS {
                        last = now;
                        let _ = store.heartbeat_global_phase2_job(token, JOB_LEASE_SECONDS);
                    }
                    std::thread::sleep(HEARTBEAT_POLL_INTERVAL);
                }
            });
            let _stop_on_unwind = StopHeartbeatOnDrop(std::sync::Arc::clone(&stop));
            let result = chat(body);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = heartbeat.join();
            result
        };
        crate::turn::run_tool_turn_with_mode(
            &mut history,
            &prompt,
            &mut sender,
            &mut |_| {},
            cancel,
            &mut |_| {},
            crate::turn::ToolMode::MemoryMerge {
                root,
                system: CONSOLIDATION_SYSTEM,
                expected_epoch: task_epoch,
            },
        )
    });
    if turn_result.is_err() {
        // 上游：先清理代理可能留下的符号链接（失败忽略），再按 failed_agent 记账
        let _ = crate::memory::storage_files::remove_memory_symlinks(root, &mut |_| {});
        return job_failed(store, &ownership_token, "failed_agent");
    }

    // 9. 写后校验（符号链接/MEMORY.md/摘要 v1 首行）
    if workspace::validate_consolidation_artifacts(root).is_err() {
        return job_failed(store, &ownership_token, "failed_invalid_artifacts");
    }

    // 9a-11（核查轮）：「任务代数核对＋所有权再确认＋Git 基线收尾」在共享
    // 锁内同一临界区——合并请求期间发生过重置的旧任务不得 reset 基线/提交
    // 成功（新代 prepare 不恢复旧凭据），且检查与基线修改不分离（reset 无法
    // 插入其间；失败阶段如实记账，SQL 落其已打开连接，不污染重置后新库）。
    // 上游序 9a→10→11 保持（heartbeat 为毫秒级 SQL，入临界区无碍）。
    {
        let _g = crate::memory::runner::lock_shared(root);
        if crate::memory::runner::read_epoch(root) != task_epoch {
            return job_failed(store, &ownership_token, "failed_reset_invalidated");
        }

        // 10. 所有权再确认：失权＝静默让位（不 reset、不成功、不把接读者标失败）
        match store.heartbeat_global_phase2_job(&ownership_token, JOB_LEASE_SECONDS) {
            Ok(false) => return Phase2Outcome::LostOwnershipAfterValidation,
            Err(_) => return job_failed(store, &ownership_token, "failed_confirm_ownership"),
            Ok(true) => {}
        }

        // 11. reset 基线（在 SQL 成功提交之前——非原子语义如实保留）
        if workspace::reset_memory_workspace_baseline(root, cancel).is_err() {
            return job_failed(store, &ownership_token, "failed_workspace_commit");
        }
    }

    // 12. 带所有权保护的 SQL 成功提交（失权返回 false：仅如实呈现，不补偿）
    let selected = selected_outputs(&raw_memories);
    let sql_committed = store
        .mark_global_phase2_job_succeeded(&ownership_token, new_watermark, &selected)
        .unwrap_or(false);
    Phase2Outcome::Succeeded { sql_committed }
}

/// 领取后失败的记账（上游 `mod job::failed`）：先按所有权条件标失败；
/// 失权（返回 false）则走 `mark_global_phase2_job_failed_if_unowned` 兜底；
/// 记账自身的存储错误按上游 `let _` 语义吞没（任务行保持原状）。
fn job_failed(store: &MemoryStore, ownership_token: &str, reason: &'static str) -> Phase2Outcome {
    let marked = store
        .mark_global_phase2_job_failed(ownership_token, reason, JOB_RETRY_DELAY_SECONDS)
        .unwrap_or(false);
    if !marked {
        let _ = store.mark_global_phase2_job_failed_if_unowned(
            ownership_token,
            reason,
            JOB_RETRY_DELAY_SECONDS,
        );
    }
    Phase2Outcome::Failed(reason)
}

/// 新水位（上游 `get_watermark`）：领取时水位与全部输入行
/// `source_updated_at` 的最大值。
fn get_watermark(claim_watermark: i64, raw_memories: &[Stage1OutputRow]) -> i64 {
    let mut watermark = claim_watermark;
    for memory in raw_memories {
        watermark = watermark.max(memory.source_updated_at);
    }
    watermark
}

/// 选中来源快照（本轮实际使用的行版本——不是结束时的最新版本）。
fn selected_outputs(raw_memories: &[Stage1OutputRow]) -> Vec<SelectedOutput> {
    raw_memories
        .iter()
        .map(|row| SelectedOutput {
            thread_id: row.thread_id.clone(),
            source_updated_at: row.source_updated_at,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watermark_is_max_of_claim_and_inputs() {
        let rows = vec![
            Stage1OutputRow {
                thread_id: "a".into(),
                source_updated_at: 100,
                raw_memory: "r".into(),
                rollout_summary: "s".into(),
                rollout_slug: None,
                generated_at: 1,
                selected_for_phase2: false,
                selected_for_phase2_source_updated_at: None,
            },
            Stage1OutputRow {
                thread_id: "b".into(),
                source_updated_at: 250,
                raw_memory: "r".into(),
                rollout_summary: "s".into(),
                rollout_slug: None,
                generated_at: 1,
                selected_for_phase2: false,
                selected_for_phase2_source_updated_at: None,
            },
        ];
        assert_eq!(get_watermark(300, &rows), 300, "领取水位更高时保持");
        assert_eq!(get_watermark(10, &rows), 250, "输入行更高时取最大");
        assert_eq!(get_watermark(7, &[]), 7);
    }

    #[test]
    fn prompt_replaces_all_occurrences() {
        let p = build_consolidation_prompt(Path::new("/mem-root"));
        assert!(p.contains("/mem-root"));
        assert!(!p.contains("{{"));
        assert!(p.contains("phase2_workspace_diff.md"));
    }
}
