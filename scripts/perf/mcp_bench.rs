//! 022 性能基准驱动（临时 example 源归档；构建方式见 scripts/perf/README.md）。
//! 用法：mcp_bench <scenario> <tools> <schema_bytes> <n> [delay_ms]
//! 场景：discover_first / steady_snapshot / relist_after_notify / call_rtt /
//!       cancel_inflight / close / probe_notify_turn（通知生效轮次探针）。
//! 输出：每样本毫秒（stdout）。
//! 注意：本文件不是仓库构建目标——按 README 复制到隔离 worktree 的
//! src/app/examples/ 后构建，避免污染工作区构建产物。
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use terminal_chat::mcp::manager::McpManager;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = args[1].clone();
    let tools: usize = args[2].parse().unwrap();
    let schema_bytes: usize = args[3].parse().unwrap();
    let n: usize = args[4].parse().unwrap();
    let _delay_ms: u64 = args.get(5).map(|s| s.parse().unwrap()).unwrap_or(0);
    let cancel = Arc::new(AtomicBool::new(false));
    let manager = McpManager::new();

    let mut samples: Vec<f64> = Vec::new();
    match scenario.as_str() {
        "discover_first" => {
            let t = Instant::now();
            let snap = manager.prepare_for_turn(&cancel).expect("prepare");
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
            eprintln!("tools_published={}", snap.len());
        }
        "steady_snapshot" => {
            let _ = manager.prepare_for_turn(&cancel).expect("warm");
            for _ in 0..n {
                let t = Instant::now();
                let _ = manager.prepare_for_turn(&cancel).expect("prepare");
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        "relist_after_notify" => {
            let snap = manager.prepare_for_turn(&cancel).expect("warm");
            let entry = snap.entry(&format!("mcp__fx__tool_{:04}", 0)).expect("tool0");
            let _ = manager.call(entry, serde_json::json!({"q": "x"}), &cancel);
            std::thread::sleep(std::time::Duration::from_millis(120));
            for _ in 0..n {
                let snap2 = manager.prepare_for_turn(&cancel).expect("prepare");
                let entry = snap2.entry(&format!("mcp__fx__tool_{:04}", 0)).expect("tool0");
                let _ = manager.call(entry, serde_json::json!({"q": "x"}), &cancel);
                std::thread::sleep(std::time::Duration::from_millis(120));
                let t = Instant::now();
                let _ = manager.prepare_for_turn(&cancel).expect("prepare");
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        "call_rtt" => {
            let snap = manager.prepare_for_turn(&cancel).expect("warm");
            let entry = snap.entry(&format!("mcp__fx__tool_{:04}", 0)).expect("tool0");
            for _ in 0..n {
                let t = Instant::now();
                let _ = manager.call(entry, serde_json::json!({"q": "x"}), &cancel);
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        "cancel_inflight" => {
            let snap = manager.prepare_for_turn(&cancel).expect("warm");
            let entry = snap.entry(&format!("mcp__fx__tool_{:04}", 0)).expect("tool0");
            for _ in 0..n {
                let c2 = Arc::new(AtomicBool::new(false));
                {
                    let c2 = c2.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(30));
                        c2.store(true, Ordering::SeqCst);
                    });
                }
                let t = Instant::now();
                let _ = manager.call(entry, serde_json::json!({"q": "x"}), &c2);
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        "probe_notify_turn" => {
            // 语义探针（最终核验）：prepare×n，轮间 NOTIFY_GAP_MS 停顿（默认
            // 30ms）、零调用；夹具 notify_after_ms 在首次 tools/list 应答后延迟
            // NOTIFY_DELAY_MS 发主动通知。生效轮次由外层从 record 判定。
            let gap: u64 = std::env::var("NOTIFY_GAP_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            for _ in 0..n {
                let _ = manager.prepare_for_turn(&cancel).expect("prepare");
                std::thread::sleep(std::time::Duration::from_millis(gap));
            }
            eprintln!("probe done");
        }
        "close" => {
            let _ = manager.prepare_for_turn(&cancel).expect("warm");
            let t = Instant::now();
            manager.close();
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        _ => panic!("unknown scenario {scenario}"),
    }
    let _ = (tools, schema_bytes);
    for s in &samples {
        println!("{s:.3}");
    }
}
