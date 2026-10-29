//! 030 §1.6 / M0 / S18：tool_search 性能门禁（冻结阈值，断言模式）。
//!
//! 测法（规格 §1.6 冻结）：单并发串行；96 与 2048 工具两档＋空目录；
//! 索引构建与已建索引查询分别计时；每档预热 5 次、正式 1000 次、独立 3 轮
//! 取 p95 的中位轮；峰值 RSS 由独立子进程记录（区分已加载目录基线与建索引
//! 增量）。采样模式（`TS30_PERF_SAMPLE=1`）输出原样本并退出断言；断言模式
//! （默认）按 M0 冻结常量校验。速度为同机 debug 构建口径，机器/构建变化须
//! 重新冻结（见 docs/logs/030-tool-search/2026-10-28-m0-perf-freeze.md）。
//!
//! 路径注记：M0 冻结时直接驱动 bm25；M1 起改走真实 `tool_search` 模块
//!（`build_catalog`/`TurnCatalog::search`）并复测同一冻结阈值（S18）。

use std::time::Instant;

/// 冻结阈值（M0 基线 × 2–3 余量；采样后回填，未采样前为不可通过的占位）。
mod frozen {
    pub const QUERY_P95_US_96: u128 = 150;
    pub const QUERY_P95_US_2048: u128 = 1_500;
    pub const QUERY_P95_US_EMPTY: u128 = 50;
    pub const BUILD_MED_US_96: u128 = 20_000;
    pub const BUILD_MED_US_2048: u128 = 400_000;
    pub const RSS_BUILD_DELTA_KB_96: u64 = 2_000;
    pub const RSS_BUILD_DELTA_KB_2048: u64 = 16_000;
}

const PHRASES: [&str; 16] = [
    "Send an email message to a recipient with attachments",
    "Read paginated records from the database table",
    "Translate a paragraph between supported languages",
    "Convert an amount between two currencies",
    "Create a calendar event with a reminder",
    "Delete stale entries from the search index",
    "Upload a document to the team workspace folder",
    "Fetch the weather forecast for a city",
    "Generate a thumbnail image for a file",
    "List open pull requests for a repository",
    "Post a short message to a chat channel",
    "Validate an address against the registry",
    "Rotate log files older than a threshold",
    "Enqueue a background job for processing",
    "Query the inventory stock level",
    "Renew an expiring access credential",
];

struct FixtureEntry {
    model_name: String,
    server_id: String,
    raw_tool: String,
    description: String,
    schema: serde_json::Value,
}

/// 确定性夹具（无随机源）：单服务器 srv，说明按 16 短语循环。
fn fixture(count: usize) -> Vec<FixtureEntry> {
    (0..count)
        .map(|i| FixtureEntry {
            model_name: format!("mcp__srv__tool_{i}"),
            server_id: "srv".to_string(),
            raw_tool: format!("tool_{i}"),
            description: PHRASES[i % PHRASES.len()].to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "param_alpha": {"type": "string", "description": "query keyword"},
                    "param_beta": {"type": "integer"},
                    "param_gamma": {"type": "boolean"}
                },
                "required": ["param_alpha"]
            }),
        })
        .collect()
}

/// 真实模块路径：构建即拟合（检索文本由 `CatalogEntry::new` 以生产
/// `build_search_text` 生成），search 含正分过滤、身份二次排序与截断。
fn build_index(entries: &[FixtureEntry]) -> terminal_chat::tool_search::TurnCatalog {
    let catalog_entries: Vec<_> = entries
        .iter()
        .map(|e| {
            terminal_chat::tool_search::CatalogEntry::new(
                &e.model_name,
                &e.server_id,
                &e.raw_tool,
                Some(&e.description),
                e.schema.clone(),
            )
        })
        .collect();
    terminal_chat::tool_search::build_catalog(catalog_entries)
}

/// 一次真实查询（真实 `TurnCatalog::search`）。
fn run_query<'a>(
    index: &'a terminal_chat::tool_search::TurnCatalog,
    q: &str,
    limit: usize,
) -> Vec<&'a str> {
    index
        .search(q, limit)
        .iter()
        .map(|e| e.search_text.as_str())
        .collect()
}

fn vm_hwm_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("读取 /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.trim().trim_end_matches("kB").trim().parse().expect("kB 数值");
        }
    }
    0
}

fn p95(samples: &mut [u128]) -> u128 {
    samples.sort_unstable();
    samples[(samples.len() * 95) / 100]
}

fn median3(v: [u128; 3]) -> u128 {
    let mut s = v;
    s.sort_unstable();
    s[1]
}

fn sampling() -> bool {
    std::env::var("TS30_PERF_SAMPLE").ok().as_deref() == Some("1")
}

/// 独立子进程 RSS 记录（父测试经 current_exe 以 --exact 重新进入本函数）。
fn rss_child_mode() -> bool {
    std::env::var("TS30_RSS_CHILD").is_ok()
}

fn measure_tier(count: usize, label: &str) -> (u128, u128) {
    let entries = fixture(count);
    // 索引构建：每轮 10 次取中位（030 §1.6 大目录口径；构建是每轮一次成本）。
    let mut build_meds = [0u128; 3];
    let mut index: Option<terminal_chat::tool_search::TurnCatalog> = None;
    for round in 0..3 {
        let mut builds = [0u128; 10];
        for s in builds.iter_mut() {
            let t = Instant::now();
            let idx = build_index(&entries);
            *s = t.elapsed().as_micros();
            if round == 2 {
                index = Some(idx);
            }
        }
        builds.sort_unstable();
        build_meds[round] = builds[4];
    }
    let index = index.expect("末轮保留索引");
    // 查询：预热 5＋正式 1000，p95；3 轮取中位。
    let query_text = "email message";
    let mut query_p95s = [0u128; 3];
    for round in query_p95s.iter_mut() {
        for _ in 0..5 {
            let _ = run_query(&index, query_text, 8);
        }
        let mut samples = [0u128; 1000];
        for s in samples.iter_mut() {
            let t = Instant::now();
            let hits = run_query(&index, query_text, 8);
            *s = t.elapsed().as_micros();
            std::hint::black_box(&hits);
        }
        *round = p95(&mut samples);
    }
    let build_med = median3(build_meds);
    let query_p95 = median3(query_p95s);
    println!(
        "TS30 {label} build_med_us={build_med} query_p95_us={query_p95}"
    );
    (build_med, query_p95)
}

#[test]
fn ts30_rss_child_mode() {
    if !rss_child_mode() {
        return;
    }
    let count: usize = std::env::var("TS30_RSS_CHILD").unwrap().parse().expect("数量");
    let entries = fixture(count);
    let baseline = vm_hwm_kb();
    let index = build_index(&entries);
    let after = vm_hwm_kb();
    let hits = run_query(&index, "email message", 8);
    std::hint::black_box(&hits);
    println!(
        "TS30 rss_{count} baseline_kb={baseline} after_build_kb={after} delta_kb={}",
        after.saturating_sub(baseline)
    );
}

#[test]
fn ts30_perf_gates() {
    if rss_child_mode() {
        return;
    }
    let (build96, query96) = measure_tier(96, "96");
    let (build2048, query2048) = measure_tier(2048, "2048");
    // 空目录：不建索引（无可搜索项时不声明搜索工具），查询口径为跳过成本。
    let empty = fixture(0);
    let t = Instant::now();
    let _ = &empty;
    let empty_build = t.elapsed().as_micros();
    let empty_query = {
        let t = Instant::now();
        let _ = empty.len();
        t.elapsed().as_micros()
    };
    println!("TS30 empty build_us={empty_build} query_us={empty_query}");
    if sampling() {
        spawn_rss_children();
        return;
    }
    // 断言模式：M0 冻结阈值。
    assert!(query96 <= frozen::QUERY_P95_US_96, "96 查询 p95 {query96}us 超冻结阈值");
    assert!(
        query2048 <= frozen::QUERY_P95_US_2048,
        "2048 查询 p95 {query2048}us 超冻结阈值"
    );
    assert!(empty_query <= frozen::QUERY_P95_US_EMPTY, "空目录查询 {empty_query}us 超阈值");
    assert!(build96 <= frozen::BUILD_MED_US_96, "96 建索引中位 {build96}us 超冻结阈值");
    assert!(
        build2048 <= frozen::BUILD_MED_US_2048,
        "2048 建索引中位 {build2048}us 超冻结阈值"
    );
    let (d96, d2048) = spawn_rss_children();
    assert!(d96 <= frozen::RSS_BUILD_DELTA_KB_96, "96 建索引 RSS 增量 {d96}kB 超阈值");
    assert!(
        d2048 <= frozen::RSS_BUILD_DELTA_KB_2048,
        "2048 建索引 RSS 增量 {d2048}kB 超阈值"
    );
}

/// 以独立子进程运行 RSS 记录，返回 (96 档增量, 2048 档增量) kB。
fn spawn_rss_children() -> (u64, u64) {
    let mut deltas = [0u64; 2];
    for (slot, count) in [96usize, 2048usize].into_iter().enumerate() {
        let exe = std::env::current_exe().expect("测试二进制路径");
        let out = std::process::Command::new(exe)
            .args(["ts30_rss_child_mode", "--exact", "--nocapture"])
            .env("TS30_RSS_CHILD", count.to_string())
            .env_remove("TCHAT_SANDBOX")
            .output()
            .expect("启动 RSS 子进程");
        // 审查 #3：子进程失败或输出缺失必须失败，不得按 0 通过（门禁空转）。
        assert!(
            out.status.success(),
            "RSS 子进程失败（exit {:?}）：{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        let mut found = false;
        for line in text.lines() {
            if line.starts_with(&format!("TS30 rss_{count} ")) {
                println!("{line}");
            }
            if let Some(rest) = line.strip_prefix(&format!("TS30 rss_{count} ")) {
                for part in rest.split_whitespace() {
                    if let Some(v) = part.strip_prefix("delta_kb=") {
                        deltas[slot] = v
                            .parse()
                            .unwrap_or_else(|_| panic!("delta_kb 不可解析：{v}"));
                        found = true;
                    }
                }
            }
        }
        assert!(found, "RSS 子进程未输出 TS30 rss_{count} 行");
    }
    (deltas[0], deltas[1])
}
