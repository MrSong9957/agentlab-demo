//! 022 测试公共设施：stdio MCP 夹具（固定仓库测试程序）＋环境隔离。
//! mcp_stdio.rs 与 mcp_turn.rs 共用；凭证均为测试 canary。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{TempDir, ENV_LOCK};
use terminal_chat::mcp::manager::{McpManager, McpSnapshot};

pub const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp_stdio_server.py");
pub const CANARY: &str = "TCHAT_MCP_TOKEN_A";
pub const CANARY_VALUE: &str = "tchat-mcp-canary-022-secret";

/// 环境守卫：持有进程级 ENV_LOCK，退出时清理本组测试的环境变量。
pub struct EnvGuard(MutexGuard<'static, ()>);
impl EnvGuard {
    pub fn acquire(guard: MutexGuard<'static, ()>) -> Self {
        Self(guard)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        std::env::remove_var("TCHAT_USER_DIR");
        std::env::remove_var(CANARY);
    }
}

/// 单个测试的完整上下文（close 时有界回收连接）。
pub struct TestCtx {
    pub _env: EnvGuard,
    pub manager: Arc<McpManager>,
    pub snapshot: Arc<McpSnapshot>,
    pub _dir: TempDir,
    pub _record_dir: TempDir,
    pub record_path: std::path::PathBuf,
}

impl TestCtx {
    pub fn close(&self) {
        self.manager.close();
    }
}

impl Drop for TestCtx {
    /// 兜底回收：断言失败（panic unwind）时也关闭连接并回收 hang 场景
    /// 的夹具子进程（显式 close 幂等，正常路径行为不变）。
    fn drop(&mut self) {
        self.manager.close();
    }
}

/// 组装 mcp.json（stdio 夹具）并完成首次 prepare。
pub fn setup(scenario: &str, extra: &str) -> TestCtx {
    setup_with(scenario, extra, 0)
}

pub fn setup_with(scenario: &str, extra: &str, delay_ms: u64) -> TestCtx {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-stdio");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    let record_dir = TempDir::new("mcp-record");
    let record_path = record_dir.path().join("record.jsonl");
    let fixture = FIXTURE.replace('\\', "/");
    let mut args = format!("\"{fixture}\", \"--scenario\", \"{scenario}\"");
    if delay_ms > 0 {
        args.push_str(&format!(", \"--delay-ms\", \"{delay_ms}\""));
    }
    args.push_str(&format!(
        ", \"--record\", \"{}\"",
        record_path.display().to_string().replace('\\', "/")
    ));
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": true, "command": "python3", "args": [{args}], "cwd": "{}"{extra}}}}}}}"#,
        std::env::temp_dir().display()
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不应取消");
    TestCtx {
        _env: EnvGuard(guard),
        manager,
        snapshot,
        _dir: dir,
        _record_dir: record_dir,
        record_path,
    }
}

/// 组装两台服务器的 mcp.json（023 汇总／部分失败测试用），ID 固定
/// `fx`、`fy`，各自独立 scenario；两台均声明 resources 场景由调用方
/// 通过 scenario 控制。
pub fn setup_two(scenario_fx: &str, scenario_fy: &str, extra_fx: &str, extra_fy: &str) -> TestCtx {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-stdio");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    let record_dir = TempDir::new("mcp-record");
    let record_path = record_dir.path().join("record.jsonl");
    let fixture = FIXTURE.replace('\\', "/");
    let tmp = std::env::temp_dir().display().to_string();
    let json = format!(
        r#"{{"mcp_servers": {{
            "fx": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "{scenario_fx}", "--record", "{rec}"], "cwd": "{tmp}"{extra_fx}}},
            "fy": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "{scenario_fy}", "--record", "{rec}"], "cwd": "{tmp}"{extra_fy}}}
        }}}}"#,
        rec = record_path.display().to_string().replace('\\', "/"),
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    let snapshot = manager
        .prepare_for_turn(&AtomicBool::new(false))
        .expect("准备不应取消");
    TestCtx {
        _env: EnvGuard(guard),
        manager,
        snapshot,
        _dir: dir,
        _record_dir: record_dir,
        record_path,
    }
}

/// 仅写配置不 prepare（首次加载失败／取消路径用）。
pub fn setup_no_prepare(
    scenario: &str,
    extra: &str,
) -> (
    EnvGuard,
    TempDir,
    TempDir,
    std::path::PathBuf,
    Arc<McpManager>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("mcp-stdio");
    std::env::set_var("TCHAT_USER_DIR", dir.path());
    std::env::set_var(CANARY, CANARY_VALUE);
    let record_dir = TempDir::new("mcp-record");
    let record_path = record_dir.path().join("record.jsonl");
    let fixture = FIXTURE.replace('\\', "/");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": true, "command": "python3", "args": ["{fixture}", "--scenario", "{scenario}", "--record", "{}"], "cwd": "{}"{extra}}}}}}}"#,
        record_path.display().to_string().replace('\\', "/"),
        std::env::temp_dir().display()
    );
    std::fs::write(dir.path().join("mcp.json"), json).expect("写入 mcp.json");
    let manager = McpManager::new();
    (EnvGuard(guard), dir, record_dir, record_path, manager)
}

pub fn read_record(record_path: &std::path::Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(record_path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("record 行为 JSON"))
        .collect()
}

pub fn definition_names(snapshot: &McpSnapshot) -> Vec<String> {
    let mut names: Vec<String> = (0..snapshot.len())
        .map(|i| {
            snapshot.definitions()[i]["function"]["name"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

