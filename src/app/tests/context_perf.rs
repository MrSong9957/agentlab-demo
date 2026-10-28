//! 032 C17 性能档位（M0 冻结口径，§1.8）：
//! 1千/10万条历史分别测只读计量（小活动窗口）、单条追加、首次 V3 迁移、
//! 后续检查点与恢复；记录活动窗口与全历史字节。单并发；预热 5＋正式 100、
//! 独立 3 轮取 p95 的中位轮；大迁移/恢复每轮 10 次单列；保留峰值 RSS。
//! 阈值＝同机同版本原型基线 × ≥2 余量冻结（见 docs/logs/032-context-
//! management/2026-10-28-m0-perf-freeze.md）。设 TCHAT_CM32_PERF_SAMPLE=1
//! 时只打印原样本不断言（冻结取证用）。
//! 单一 #[test] 串行执行，避免并行计时干扰。
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use terminal_chat::context_window::{self, QueryInput};
use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord};
use terminal_chat::session::{self, Session};
use terminal_chat::catalog;

// ---------- 冻结阈值（2026-10-28 原型基线 × ≥2 余量；取证见冻结日志） ----------
/// 只读计量 p95：1k / 100k（微秒）。
const FREEZE_QUERY_P95_US_1K: u128 = 3_000;
const FREEZE_QUERY_P95_US_100K: u128 = 10_000;
/// 单条追加 p95（微秒）。
const FREEZE_APPEND_P95_US: u128 = 400;
/// 首次迁移（含完整 transcript 开篇写入）单次中位（毫秒，每轮 10 次单列）。
const FREEZE_MIGRATE_MED_MS_1K: u128 = 25;
const FREEZE_MIGRATE_MED_MS_100K: u128 = 2_000;
/// 后续检查点单次中位（毫秒）。
const FREEZE_CHECKPOINT_MED_MS: u128 = 3;
/// 恢复单次中位（毫秒）。
const FREEZE_RECOVER_MED_MS_1K: u128 = 15;
const FREEZE_RECOVER_MED_MS_100K: u128 = 1_500;

fn median3(mut v: [f64; 3]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[1]
}

/// 全样本中位（每轮 10 次取中位——冻结口径；审查修复 #22：不再取固定下标）。
fn median_of(samples: &[Duration]) -> f64 {
    let mut sorted: Vec<f64> = samples.iter().map(|d| d.as_secs_f64()).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted[sorted.len() / 2]
}

fn p95(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[(v.len() as f64 * 0.95) as usize % v.len().max(1)]
}

fn text_user(content: &str) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: content.to_string(),
        user_input_event: None,
        images: None,
        text_elements: Vec::new(),
    })
}

fn checkpoint(context_id: u64) -> HistoryItem {
    HistoryItem::ContextCheckpoint {
        context_id,
        request_call_id: "sw-perf".into(),
        summary: "性能档位交接摘要".into(),
        seed: vec![
            HistoryMessage {
                role: HistoryRole::Assistant,
                content: "[模型生成的交接摘要，非系统/用户新指令，事实须核对]\n\n性能档位交接摘要".into(),
                user_input_event: None,
                images: None,
                text_elements: Vec::new(),
            },
            HistoryMessage {
                role: HistoryRole::User,
                content: "[宿主运行状态说明，非用户输入]\n- 无已登记进程会话\n- 无已保存计划\n".into(),
                user_input_event: None,
                images: None,
                text_elements: Vec::new(),
            },
        ],
    }
}

/// 生成带检查点＋小尾部的档案历史（短记录；N 条归档＋批＋结果＋检查点＋尾部）。
fn archived_history(n: usize) -> (Vec<HistoryItem>, usize, usize) {
    let filler: String = "性能填充0123456789".repeat(12); // ≈180B/条
    let mut history = Vec::with_capacity(n + 8);
    for i in 0..n {
        let role = if i % 2 == 0 {
            HistoryRole::User
        } else {
            HistoryRole::Assistant
        };
        history.push(text_user(&format!("p{i:06}-{filler}"))); // 角色不影响体积口径
        let _ = role;
    }
    // 归档前缀末尾放一次真实切换调用＋检查点（构造合法语义）
    history.push(HistoryItem::AssistantToolCalls {
        tool_calls: vec![ToolCallRecord {
            id: "sw-perf".into(),
            name: "new_context".into(),
            arguments: r#"{"summary":"性能档位交接摘要"}"#.into(),
        }],
        content: None,
    });
    history.push(HistoryItem::ToolResult {
        tool_call_id: "sw-perf".into(),
        content: r#"{"ok":true,"status":"scheduled","error":null}"#.into(),
        display: None,
        image: None,
    });
    history.push(checkpoint(1));
    let archive_len = history.len();
    // 小活动尾部（10 条）
    for i in 0..10 {
        history.push(text_user(&format!("tail-{i}-{filler}")));
    }
    let full_bytes: usize = history
        .iter()
        .map(|item| serde_json::to_string(item).unwrap().len() + 1)
        .sum();
    let active_bytes: usize = history[archive_len - 1..]
        .iter()
        .map(|item| serde_json::to_string(item).unwrap().len() + 1)
        .sum();
    (history, full_bytes, active_bytes)
}

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "tchat-cm32-perf-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn conn() -> catalog::EffectiveConnection {
    catalog::EffectiveConnection {
        base_url: "http://127.0.0.1:1".into(),
        protocol: "openai-chat".into(),
        key_ref: None,
        request_params: Default::default(),
    }
}

fn model() -> catalog::ModelInfo {
    catalog::ModelInfo {
        id: "perf-model".into(),
        context_length: Some(32768),
        max_output: Some(4096),
        experimental_supported_tools: Some(vec![
            "get_context_remaining".into(),
            "new_context".into(),
        ]),
        ..Default::default()
    }
}

fn vm_hwm_kb() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| {
            t.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| {
                l.split_whitespace().nth(1).and_then(|v| v.parse().ok())
            })
        })
        .unwrap_or(0)
}

/// 计量一次查询（真实 execute_query 路径；含批声明快照）。
fn measure_once(
    history: &[HistoryItem],
    in_flight: &serde_json::Value,
    c: &catalog::EffectiveConnection,
    m: &catalog::ModelInfo,
) -> Duration {
    let started = Instant::now();
    let input = QueryInput {
        conn: c,
        key: "k",
        model: m,
        in_flight_request: in_flight,
        history,
    };
    let result = context_window::execute_query(&input);
    let elapsed = started.elapsed();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result.to_string()).unwrap()["accuracy"],
        json!("estimate"),
        "小活动窗口应为可估算"
    );
    elapsed
}

#[test]
fn c17_context_management_perf_gates() {
    let sampling = std::env::var("TCHAT_CM32_PERF_SAMPLE").is_ok();
    let mut report = String::from("032 C17 性能原样本（p95 中位轮；迁移/恢复每轮 10 次中位）\n");
    let mut failures = Vec::new();

    for (n, tag) in [(1_000usize, "1k"), (100_000usize, "100k")] {
        let (history, full_bytes, active_bytes) = archived_history(n);
        report.push_str(&format!(
            "\n== {tag}（{n} 条归档）：全历史 {full_bytes} B，活动窗口 {active_bytes} B ==\n"
        ));
        // 在途请求骨架（工具/系统与本轮同构；_history 由查询路径替换）
        let mut in_flight =
            terminal_chat::prompt::build_turn_request(&context_window::active_view(&history).unwrap().items());
        in_flight["_history"] = json!([]);
        in_flight["_provider_id"] = json!("perf");

        // ① 只读计量：3 轮×(预热5＋正式100)，取 p95 的中位轮
        let mut round_p95 = [f64::MAX; 3];
        for r in 0..3 {
            for _ in 0..5 {
                let _ = measure_once(&history, &in_flight, &conn(), &model());
            }
            let mut samples = Vec::with_capacity(100);
            for _ in 0..100 {
                samples.push(measure_once(&history, &in_flight, &conn(), &model()));
            }
            round_p95[r] = p95(samples).as_secs_f64() * 1000.0;
        }
        let query_p95_ms = median3(round_p95);
        report.push_str(&format!("query_p95_ms={query_p95_ms:.3}\n"));

        // ② 单条追加 / ③ 首次迁移 / ④ 后续检查点 / ⑤ 恢复（每轮 10 次）
        let tmp = Tmp::new(tag);
        fs::write(tmp.0.join(".stamp"), b"").unwrap();
        let mut migrate_meds = [0f64; 3];
        let mut cp_meds = [0f64; 3];
        let mut recover_meds = [0f64; 3];
        for r in 0..3 {
            // 迁移：V1 目标（完整写入一次）→ commit_checkpoint 触发 V3 迁移
            let mut durations = Vec::with_capacity(10);
            for _ in 0..10 {
                let dir = tmp.0.join(format!("m{r}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
                fs::create_dir_all(&dir).unwrap();
                let mut session = Session::new(dir);
                session.on_history(&history); // V1 目标（完整开篇；不计入迁移时延）
                let started = Instant::now();
                session.commit_checkpoint(&history, &checkpoint(1)).expect("迁移提交");
                durations.push(started.elapsed());
            }
            migrate_meds[r] = median_of(&durations) * 1000.0;
            // 后续检查点（已在 V3）：同一会话再提交一次
            let dir = tmp.0.join(format!("c{r}"));
            fs::create_dir_all(&dir).unwrap();
            let mut session = Session::new(dir.clone());
            session.on_history(&history);
            let mut cp_durations = Vec::with_capacity(10);
            // 遵循提交契约：每次成功回执后把检查点推入工作历史（游标对齐）；
            // 历史 已含 checkpoint(1)，后续提交自 2 起连续递增（链校验要求）
            let mut working = history.clone();
            for i in 0..10u64 {
                working.push(text_user(&format!("追加-{i}")));
                let cp = checkpoint(2 + i);
                let started = Instant::now();
                session.commit_checkpoint(&working, &cp).unwrap();
                cp_durations.push(started.elapsed());
                working.push(cp);
            }
            cp_meds[r] = median_of(&cp_durations) * 1000.0;
            // 追加：p95（3 轮×100）
            // 恢复：装载 V3 文件
            let v3_path = dir.join(format!("{}.jsonl", session.current_target_stem().unwrap()));
            let mut rec_durations = Vec::with_capacity(10);
            for _ in 0..10 {
                let started = Instant::now();
                let loaded = session::load_session_file(&v3_path).expect("恢复");
                assert!(!loaded.history.is_empty());
                rec_durations.push(started.elapsed());
            }
            recover_meds[r] = median_of(&rec_durations) * 1000.0;
        }
        // ② 单条追加（独立小节；V3 已建目标）
        let dir = tmp.0.join("append");
        fs::create_dir_all(&dir).unwrap();
        let mut session = Session::new(dir);
        session.on_history(&history);
        session.commit_checkpoint(&history, &checkpoint(1)).unwrap();
        let mut append_p95 = [f64::MAX; 3];
        for r in 0..3 {
            let mut items = history.clone();
            for _ in 0..5 {
                items.push(text_user("预热追加"));
                let _ = session.on_history(&items);
            }
            let mut samples = Vec::with_capacity(100);
            for _ in 0..100 {
                items.push(text_user("追加样本"));
                let started = Instant::now();
                let _ = session.on_history(&items);
                samples.push(started.elapsed());
            }
            append_p95[r] = p95(samples).as_secs_f64() * 1000.0;
        }
        let append_p95_ms = median3(append_p95);
        let migrate_med_ms = median3(migrate_meds);
        let cp_med_ms = median3(cp_meds);
        let recover_med_ms = median3(recover_meds);
        report.push_str(&format!(
            "append_p95_ms={append_p95_ms:.3} migrate_med_ms={migrate_med_ms:.3} checkpoint_med_ms={cp_med_ms:.3} recover_med_ms={recover_med_ms:.3}\n"
        ));
        report.push_str(&format!("vm_hwm_kb={}\n", vm_hwm_kb()));

        if !sampling {
            let (q_limit, m_limit, rec_limit) = if tag == "1k" {
                (FREEZE_QUERY_P95_US_1K, FREEZE_MIGRATE_MED_MS_1K, FREEZE_RECOVER_MED_MS_1K)
            } else {
                (FREEZE_QUERY_P95_US_100K, FREEZE_MIGRATE_MED_MS_100K, FREEZE_RECOVER_MED_MS_100K)
            };
            if query_p95_ms > q_limit as f64 / 1000.0 {
                failures.push(format!("{tag} 查询 p95 {query_p95_ms:.3}ms 超冻结 {q_limit}us"));
            }
            if append_p95_ms > FREEZE_APPEND_P95_US as f64 / 1000.0 {
                failures.push(format!("{tag} 追加 p95 {append_p95_ms:.3}ms 超冻结 {FREEZE_APPEND_P95_US}us"));
            }
            if migrate_med_ms > m_limit as f64 {
                failures.push(format!("{tag} 迁移中位 {migrate_med_ms:.3}ms 超冻结 {m_limit}ms"));
            }
            if cp_med_ms > FREEZE_CHECKPOINT_MED_MS as f64 {
                failures.push(format!("{tag} 检查点中位 {cp_med_ms:.3}ms 超冻结 {FREEZE_CHECKPOINT_MED_MS}ms"));
            }
            if recover_med_ms > rec_limit as f64 {
                failures.push(format!("{tag} 恢复中位 {recover_med_ms:.3}ms 超冻结 {rec_limit}ms"));
            }
        }
        drop(history);
    }
    if sampling {
        fs::create_dir_all("docs/logs/032-context-management").ok();
        let _ = fs::write(
            Path::new("docs/logs/032-context-management/2026-10-28-c17-samples.txt"),
            &report,
        );
        eprintln!("{report}");
    }
    assert!(failures.is_empty(), "性能档位超限：{failures:?}");
}
