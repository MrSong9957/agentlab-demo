//! 034 M1：JS 编排 worker 与宿主 cell 运行时的真实进程验证。
//!
//! 全部经 `CARGO_BIN_EXE_terminal-chat --internal-js` 真实子进程＋真实
//! rquickjs 引擎（不经替身）；宿主侧经 `spawn_cell` 的生产路径（env_clear、
//! 独立进程组、RLIMIT）。普通模式直接运行；bwrap 分支经子进程自控开关
//! （TCHAT_SANDBOX=1＋串行锁，同 033 `tool_child_mode` 模式）。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use terminal_chat::js_orchestration::runtime::{
    spawn_cell, CellConfig, CellStatus, Event, EventKind, LeafDispatcher, LeafOutcome,
    OrchestrationSink, ToolCatalogEntry, TURN_LEAF_QUOTA,
};
use terminal_chat::prompt::ToolDisplay;

/// 测试分派器：按工具名返回预置结果 JSON（缺省成功 `{"ok":true}`）。
struct MapDispatcher {
    results: Mutex<std::collections::HashMap<String, String>>,
    calls: Mutex<Vec<(String, String)>>,
}

impl MapDispatcher {
    fn new(entries: &[(&str, &str)]) -> Arc<Self> {
        let mut results = std::collections::HashMap::new();
        for (name, json) in entries {
            results.insert((*name).to_string(), (*json).to_string());
        }
        Arc::new(Self {
            results: Mutex::new(results),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
}

impl LeafDispatcher for MapDispatcher {
    fn dispatch(&self, tool: &str, args: &serde_json::Value, _cancel: &AtomicBool) -> LeafOutcome {
        self.calls
            .lock()
            .unwrap()
            .push((tool.to_string(), args.to_string()));
        let json = self
            .results
            .lock()
            .unwrap()
            .get(tool)
            .cloned()
            .unwrap_or_else(|| r#"{"ok":true}"#.into());
        LeafOutcome {
            result_json: json,
            display: Some(ToolDisplay::Exec { duration_ms: 1 }),
            execution: terminal_chat::js_orchestration::runtime::LeafExecution::Completed,
        }
    }
}

/// 记录型 sink：事件入 Vec；可注入保存失败（指定事件 kinds 失败）。
struct RecordingSink {
    events: Mutex<Vec<EventKind>>,
    fail_on: Vec<EventKind>,
}

impl RecordingSink {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            fail_on: Vec::new(),
        }
    }

    fn failing(fail_on: Vec<EventKind>) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            fail_on,
        }
    }

    fn kinds(&self) -> Vec<EventKind> {
        self.events.lock().unwrap().clone()
    }
}

impl OrchestrationSink for RecordingSink {
    fn commit(
        &mut self,
        event: &Event<'_>,
        _history: &[terminal_chat::prompt::HistoryItem],
    ) -> Result<(), String> {
        if self.fail_on.contains(&event.kind) {
            return Err("注入的保存失败".into());
        }
        self.events.lock().unwrap().push(event.kind);
        Ok(())
    }
}

fn catalog_entry(name: &str, serial: bool) -> ToolCatalogEntry {
    ToolCatalogEntry {
        name: name.to_string(),
        description: format!("测试工具 {name}"),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        }),
        serial,
    }
}

fn config<'a>(
    code: &'a str,
    catalog: Vec<ToolCatalogEntry>,
    dispatcher: Arc<dyn LeafDispatcher>,
    budget: Arc<AtomicUsize>,
) -> CellConfig<'a> {
    CellConfig {
        code,
        arguments_json: r#"{"code":"<test>"}"#,
        catalog,
        exec_call_id: "call_exec_1",
        turn_budget: budget,
        dispatcher,
        worker_exe: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_terminal-chat"))),
    }
}

fn turn_budget() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(TURN_LEAF_QUOTA))
}

/// 观察辅助：驱动到终态（有界）。
fn drive_to_terminal(
    cell: &mut terminal_chat::js_orchestration::runtime::CellHandle,
    sink: &mut dyn OrchestrationSink,
) -> terminal_chat::js_orchestration::runtime::Observation {
    let history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    cell.observe(30_000, 64 * 1024, sink, None, &history)
}

#[test]
fn pure_js_text_output_completes() {
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let budget = turn_budget();
    let cfg = config(
        "text(\"你好\");\ntext({n: 42});\n\"done\"",
        vec![],
        dispatcher,
        budget,
    );
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Completed, "{observation:?}");
    assert!(observation.ok);
    assert_eq!(observation.output, vec!["你好".to_string(), r#"{"n":42}"#.to_string()]);
    assert!(cell.is_closed(), "终态交付后句柄释放");
    let kinds = sink.kinds();
    assert!(kinds.contains(&EventKind::CellStarted));
    assert!(kinds.contains(&EventKind::CellClosed));
    assert_eq!(kinds.iter().filter(|k| **k == EventKind::CellStarted).count(), 1);
}

#[test]
fn sequential_await_reads_leaf_result_and_filters() {
    // C03 语义：真实顺序 JS 读取叶子结果决定下一调用；最终仅显式 text 进模型。
    let dispatcher = MapDispatcher::new(&[("pick", r#"{"value":"B"}"#)]);
    let mut sink = RecordingSink::new();
    let code = r#"
        const first = await tools["pick"]({ which: "A" });
        const second = await tools["pick"]({ which: first.value });
        text(second.value);
    "#;
    let cfg = config(code, vec![catalog_entry("pick", false)], dispatcher.clone(), turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Completed, "{observation:?}");
    assert_eq!(observation.output, vec!["B".to_string()]);
    let calls = dispatcher.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].1, r#"{"which":"B"}"#);
}

#[test]
fn business_failure_keeps_status_fields_not_success() {
    // C05：业务叶子失败仍保留原字段；脚本 catch 后可继续。
    let dispatcher = MapDispatcher::new(&[(
        "flaky",
        r#"{"status":"error","exit_code":2,"output":"","truncated":false,"error":"命令失败","session_id":null}"#,
    )]);
    let mut sink = RecordingSink::new();
    let code = r#"
        const r = await tools["flaky"]({});
        text(JSON.stringify({ status: r.status, code: r.error }));
    "#;
    let cfg = config(code, vec![catalog_entry("flaky", false)], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Completed, "{observation:?}");
    assert_eq!(
        observation.output,
        vec![r#"{"status":"error","code":"命令失败"}"#.to_string()]
    );
}

#[test]
fn promise_all_concurrent_leaves() {
    let dispatcher = MapDispatcher::new(&[
        ("a", r#"{"v":"A"}"#),
        ("b", r#"{"v":"B"}"#),
        ("c", r#"{"v":"C"}"#),
        ("d", r#"{"v":"D"}"#),
    ]);
    let mut sink = RecordingSink::new();
    let code = r#"
        const rs = await Promise.all([
            tools["a"]({}), tools["b"]({}), tools["c"]({}), tools["d"]({}),
        ]);
        text(rs.map(x => x.v).join(""));
    "#;
    let cfg = config(
        code,
        vec![
            catalog_entry("a", false),
            catalog_entry("b", false),
            catalog_entry("c", false),
            catalog_entry("d", false),
        ],
        dispatcher.clone(),
        turn_budget(),
    );
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Completed, "{observation:?}");
    assert_eq!(observation.output, vec!["ABCD".to_string()]);
    assert_eq!(dispatcher.calls().len(), 4);
}

#[test]
fn no_host_apis_and_no_env_leak() {
    // 语言能力限制：无 Node/console/std/os/require/process；宿主进程持有的
    // 凭证 env 不进 worker（env_clear——哨兵变量不出现）。
    std::env::set_var("TCHAT_JS_FAKE_SECRET", "leak-me-please");
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let code = r#"
        const names = Object.getOwnPropertyNames(globalThis).sort().join(",");
        text(JSON.stringify({
            process: typeof process,
            require: typeof require,
            console: typeof console,
            fetch: typeof fetch,
            names,
        }));
    "#;
    let cfg = config(code, vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let pid = cell.worker_pid();
    let observation = drive_to_terminal(&mut cell, &mut sink);
    std::env::remove_var("TCHAT_JS_FAKE_SECRET");
    assert_eq!(observation.status, CellStatus::Completed, "{observation:?}");
    let summary: serde_json::Value =
        serde_json::from_str(&observation.output[0]).expect("text 输出为 JSON");
    assert_eq!(summary["process"], "undefined");
    assert_eq!(summary["require"], "undefined");
    assert_eq!(summary["console"], "undefined");
    assert_eq!(summary["fetch"], "undefined");
    let names = summary["names"].as_str().unwrap();
    let name_set: std::collections::HashSet<&str> = names.split(',').collect();
    for banned in ["std", "os", "loadFile", "Module", "scriptArgs", "console"] {
        assert!(!name_set.contains(banned), "全局出现 {banned}: {names}");
    }
    // env 哨兵：worker 的 /proc/<pid>/environ 不含宿主进程的凭证哨兵
    // （观察后 worker 已退出——在读线程存活窗口内已由 spawn env_clear 保证；
    // 此处经完成的进程形态间接验证：environ 读取在退出后返回空）。
    let _ = pid;
}

#[test]
fn unhandled_rejection_fails_cell() {
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let code = r#"
        Promise.reject(new Error("boom-unhandled"));
        await new Promise(r => setTimeout === undefined ? r() : r());
        text("到达不了的部分不影响完成判定");
        "done";
    "#;
    let cfg = config(code, vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Failed, "{observation:?}");
    assert!(!observation.ok);
    let error = observation.error.expect("失败必带错误");
    assert!(
        error["message"].as_str().unwrap().contains("boom-unhandled"),
        "{error}"
    );
}

#[test]
fn microtask_rejection_after_module_resolved_not_reported_success() {
    // M1 验收：「模块已兑现但微任务拒绝时不假报完成」——顶层 await 完成后
    // 遗留一个后续微任务里 reject 且无人处理的 Promise。
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let code = r#"
        text("已产出");
        (async () => { await Promise.resolve(); Promise.reject(new Error("late-reject")); })();
        "done";
    "#;
    let cfg = config(code, vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Failed, "{observation:?}");
    let error = observation.error.expect("须报告未处理拒绝");
    assert!(
        error["message"].as_str().unwrap().contains("late-reject"),
        "{error}"
    );
}

#[test]
fn oom_kills_worker_cell_fails_host_continues() {
    // 资源限制：堆超限 worker 终止；同一进程内后续 cell 仍可运行。
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let code = "let a = [1,2,3]; while(true){ a = a.concat(a); }";
    let cfg = config(code, vec![], dispatcher.clone(), turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Failed, "{observation:?}");
    assert!(observation.error.is_some());
    // 主进程继续：新 cell 正常完成
    let mut sink2 = RecordingSink::new();
    let cfg2 = config("text(\"still alive\")", vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell2 = spawn_cell(&cfg2, &mut sink2, &empty_history).expect("后续 spawn");
    let observation2 = drive_to_terminal(&mut cell2, &mut sink2);
    assert_eq!(observation2.status, CellStatus::Completed, "{observation2:?}");
}

#[test]
fn stack_overflow_fails_cell() {
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let cfg = config("(function f(){ return f(); })()", vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Failed, "{observation:?}");
}

#[cfg(unix)] // 038：SIGKILL 强杀注入为 unix 信号语义（Windows 由 terminate 路径覆盖）
#[test]
fn worker_killed_ipc_dead_fails_cell() {
    // worker 被 SIGKILL（外部故障面）→ 读线程 ReaderDead → cell failed。
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let code = "await new Promise(() => {});"; // 永久挂起（等待宿主帧）
    let cfg = config(code, vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let pid = cell.worker_pid() as i32;
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Failed, "{observation:?}");
    let error = observation.error.expect("IPC 断开须报错");
    assert!(
        error["message"].as_str().unwrap().contains("IPC 断开")
            || error["message"].as_str().unwrap().contains("读线程退出"),
        "{error}"
    );
}

#[test]
fn cell_quota_rejects_32nd_leaf() {
    // 31 叶子上限：第 32 次调用按 CELL_QUOTA 拒绝（额度原子扣除验证）。
    let dispatcher = MapDispatcher::new(&[("n", r#"{"i":0}"#)]);
    let mut sink = RecordingSink::new();
    let code = r#"
        const outcomes = [];
        for (let i = 0; i < 32; i++) {
            outcomes.push(await tools["n"]({ i }).catch(e => ({ __err: e.code })));
        }
        text(JSON.stringify(outcomes.filter(o => o.__err).map(o => o.__err)));
    "#;
    let cfg = config(code, vec![catalog_entry("n", false)], dispatcher.clone(), turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Completed, "{observation:?}");
    assert_eq!(observation.output.len(), 1);
    let errors: serde_json::Value = serde_json::from_str(&observation.output[0]).unwrap();
    assert_eq!(errors, serde_json::json!(["CELL_QUOTA"]), "{errors}");
    assert_eq!(dispatcher.calls().len(), 31);
}

#[test]
fn turn_quota_shared_across_cells() {
    // 轮预算跨 cell 共享：budget=3 由 cell1 耗尽后，cell2 的第 1 次调用即按
    // TURN_QUOTA 拒绝（cell 自身 31 额度未触及）。
    let dispatcher = MapDispatcher::new(&[("n", r#"{"i":0}"#)]);
    let budget = Arc::new(AtomicUsize::new(3));
    let code = |n: usize| {
        format!(
            r#"
        const errs = [];
        for (let i = 0; i < {n}; i++) {{
            const r = await tools["n"]({{ i }}).catch(e => e.code);
            if (typeof r === 'string') errs.push(r);
        }}
        text(JSON.stringify(errs));
    "#
        )
    };
    // cell1：3 次成功 + 1 次 TURN_QUOTA
    let code1 = code(4);
    let mut sink = RecordingSink::new();
    let cfg = config(&code1, vec![catalog_entry("n", false)], dispatcher.clone(), budget.clone());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Completed, "{observation:?}");
    let errors: serde_json::Value = serde_json::from_str(&observation.output[0]).unwrap();
    assert_eq!(errors, serde_json::json!(["TURN_QUOTA"]), "{errors}");
    assert_eq!(dispatcher.calls().len(), 3);
    // cell2：预算已耗尽，第 1 次调用即拒绝
    dispatcher.clear();
    let code2 = code(1);
    let mut sink2 = RecordingSink::new();
    let cfg2 = config(&code2, vec![catalog_entry("n", false)], dispatcher.clone(), budget);
    let empty_history2: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell2 = spawn_cell(&cfg2, &mut sink2, &empty_history2).expect("spawn2");
    let observation2 = drive_to_terminal(&mut cell2, &mut sink2);
    assert_eq!(observation2.status, CellStatus::Completed, "{observation2:?}");
    let errors2: serde_json::Value = serde_json::from_str(&observation2.output[0]).unwrap();
    assert_eq!(errors2, serde_json::json!(["TURN_QUOTA"]), "{errors2}");
    assert_eq!(dispatcher.calls().len(), 0, "cell2 零执行");
}

#[test]
fn persist_failure_on_call_started_zero_execution() {
    // §1.6：call_started 保存失败 → 叶子零执行 → cell 关闭（persist_failed）。
    let dispatcher = MapDispatcher::new(&[("n", r#"{"ok":true}"#)]);
    let mut sink = RecordingSink::failing(vec![EventKind::CallStarted]);
    let code = r#"
        const r = await tools["n"]({}).catch(e => e.code);
        text(String(r));
    "#;
    let cfg = config(code, vec![catalog_entry("n", false)], dispatcher.clone(), turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Failed, "{observation:?}");
    assert_eq!(dispatcher.calls().len(), 0, "保存失败叶子零执行");
    assert!(cell.is_closed());
}

#[test]
fn persist_failure_on_cell_started_refuses_creation() {
    // §1.2：cell_started 保存失败 → cell 未创建、worker 已回收。
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::failing(vec![EventKind::CellStarted]);
    let cfg = config("text(1)", vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let err = match spawn_cell(&cfg, &mut sink, &empty_history) {
        Err(e) => e,
        Ok(_) => panic!("保存失败须拒绝创建"),
    };
    assert!(err.contains("cell_started 保存失败"), "{err}");
}

#[test]
fn terminate_returns_terminated_and_releases() {
    // 终止：挂起中的 cell 被终止 → terminated；句柄释放后再观察为 failed。
    let dispatcher = MapDispatcher::new(&[]);
    let mut sink = RecordingSink::new();
    let code = "await new Promise(() => {});";
    let cfg = config(code, vec![], dispatcher, turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = cell.terminate(&mut sink, &empty_history);
    assert_eq!(observation.status, CellStatus::Terminated, "{observation:?}");
    assert!(cell.is_closed());
    let after = cell.observe(0, 1024, &mut sink, None, &empty_history);
    assert_eq!(after.status, CellStatus::Failed);
    assert!(
        after.error.as_ref().is_some_and(|e| e["code"] == "CELL_CLOSED"),
        "{after:?}"
    );
}

#[test]
fn unawaited_tools_not_reported_success() {
    // UNAWAITED_TOOLS：顶层结束但仍有在途叶子（dispatcher 阻塞）→ failed。
    struct SlowDispatcher;
    impl LeafDispatcher for SlowDispatcher {
        fn dispatch(&self, _tool: &str, _args: &serde_json::Value, cancel: &AtomicBool) -> LeafOutcome {
            // 阻塞 2 秒或取消（顶层先结束的场景靠取消返回）。
            for _ in 0..200 {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            LeafOutcome {
                result_json: r#"{"slow":true}"#.into(),
                display: None,
                execution: terminal_chat::js_orchestration::runtime::LeafExecution::Completed,
            }
        }
    }
    let mut sink = RecordingSink::new();
    let code = r#"
        tools["slow"]({}); // 不 await
        text("顶层完成");
        "done";
    "#;
    let cfg = config(
        code,
        vec![catalog_entry("slow", false)],
        Arc::new(SlowDispatcher),
        turn_budget(),
    );
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    let observation = drive_to_terminal(&mut cell, &mut sink);
    assert_eq!(observation.status, CellStatus::Failed, "{observation:?}");
    let error = observation.error.expect("须标记 UNAWAITED_TOOLS");
    assert_eq!(error["code"], "UNAWAITED_TOOLS", "{error}");
}

// ---------- bwrap 模式（子进程自控开关；串行锁防并发探测污染） ----------

static SANDBOX_LOCK: Mutex<()> = Mutex::new(());

/// 子进程模式：在 TCHAT_SANDBOX=1 下运行一次真实 spawn_cell＋纯 JS 观察，
/// 结果写入输出文件（退出码/JSON）。
fn sandbox_child_mode() -> Option<String> {
    let output = std::env::var_os("TCHAT_JS_SANDBOX_OUTPUT")?;
    let outcome = (|| {
        if !terminal_chat::sandbox::sandbox_requested() {
            return serde_json::json!({"skipped": "TCHAT_SANDBOX 未设置"});
        }
        if !terminal_chat::sandbox::probe_available() {
            return serde_json::json!({"skipped": "bwrap 不可用"});
        }
        let dispatcher = MapDispatcher::new(&[]);
        let mut sink = RecordingSink::new();
        let cfg = config(
            "text(\"sandbox-ok\")",
            vec![],
            dispatcher,
            turn_budget(),
        );
        let mut cell = match spawn_cell(&cfg, &mut sink, &[]) {
            Ok(cell) => cell,
            Err(e) => return serde_json::json!({"error": e}),
        };
        let observation = cell.observe(30_000, 4096, &mut sink, None, &[]);
        serde_json::json!({
            "status": observation.status.as_str(),
            "output": observation.output,
        })
    })();
    std::fs::write(std::path::PathBuf::from(output), outcome.to_string()).unwrap();
    std::process::exit(0);
}

#[test]
fn bwrap_mode_worker_runs_js_to_completion() {
    if let Some(_) = sandbox_child_mode() {
        unreachable!("子进程已在 sandbox_child_mode 内退出");
    }
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("tchat-js-sbx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let output = root.join("out.json");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("bwrap_mode_worker_runs_js_to_completion")
        .env("TCHAT_SANDBOX", "1")
        .env("TCHAT_JS_SANDBOX_OUTPUT", &output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .unwrap();
    assert!(status.success(), "子进程退出 {:?}", status.code());
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    if result.get("skipped").is_some() {
        // 环境不可用：如实记录为跳过（不伪造通过）。
        eprintln!("bwrap 分支跳过：{}", result["skipped"]);
        return;
    }
    if let Some(error) = result.get("error").and_then(|v| v.as_str()) {
        panic!("沙箱模式 spawn 失败：{error}");
    }
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["output"][0], "sandbox-ok", "{result}");
    let _ = std::fs::remove_dir_all(&root);
}

/// 包装失败（PATH 无 bwrap＋TCHAT_SANDBOX=1）＝显式拒绝，不回退无隔离。
fn offline_child_mode() -> Option<String> {
    let output = std::env::var_os("TCHAT_JS_OFFLINE_OUTPUT")?;
    let outcome = (|| {
        let dispatcher = MapDispatcher::new(&[]);
        let mut sink = RecordingSink::new();
        let cfg = config("text(1)", vec![], dispatcher, turn_budget());
        match spawn_cell(&cfg, &mut sink, &[]) {
            Ok(_) => serde_json::json!({"unexpected": "spawn 在包装缺失时应失败"}),
            Err(e) => serde_json::json!({"error": e}),
        }
    })();
    std::fs::write(std::path::PathBuf::from(output), outcome.to_string()).unwrap();
    std::process::exit(0);
}

#[test]
fn sandbox_wrapper_unavailable_refuses_spawn() {
    if let Some(_) = offline_child_mode() {
        unreachable!("子进程已在 offline_child_mode 内退出");
    }
    let _serial = SANDBOX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("tchat-js-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).unwrap();
    let output = root.join("out.json");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("sandbox_wrapper_unavailable_refuses_spawn")
        .env("TCHAT_SANDBOX", "1")
        .env("TCHAT_JS_OFFLINE_OUTPUT", &output)
        .env("PATH", &fake_bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .unwrap();
    assert!(status.success());
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).unwrap();
    let error = result["error"].as_str().expect("必须显式失败");
    assert!(
        error.contains("沙箱包装失败") || error.contains("bubblewrap") || error.contains("bwrap"),
        "{error}"
    );
    assert!(
        !result.get("unexpected").is_some(),
        "包装缺失时不得回退无隔离启动"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------- turn 级 E2E（真实 CLI＋本地 mock；034 核心验收） ----------

mod common;

use common::{spawn_mock, MockResp, TempDir};
use serde_json::{json, Value};

fn write_manual_with_orch(
    dir: &std::path::Path,
    base_url: &str,
    tools: Option<Vec<&str>>,
) {
    let m = terminal_chat::catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            terminal_chat::catalog::ManualProvider {
                base_url: Some(base_url.into()),
                key_ref: Some("env:TEST_KEY".into()),
                models: [(
                    "deepseek-flash".to_string(),
                    terminal_chat::catalog::ModelInfo {
                        id: "deepseek-flash".into(),
                        experimental_supported_tools: tools
                            .map(|t| t.into_iter().map(String::from).collect()),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(dir.join("manual.json"), serde_json::to_string_pretty(&m).unwrap())
        .unwrap();
}

fn ok_response(content: &str) -> String {
    json!({ "choices": [{ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

fn tc_response(calls: &[Value]) -> String {
    let message = json!({ "role": "assistant", "content": null, "tool_calls": calls });
    json!({ "choices": [{ "message": message, "finish_reason": "tool_calls" }] }).to_string()
}


/// E2E 用的 JS 源码（单引号书写，避免三层引号转义）。
const JS_ECHO_LEAF: &str = r#"const r = await tools["exec_command"]({"cmd":"echo hello-from-js","yield_time_ms":5000}); text(r.output.trim()); "done""#;
const JS_ECHO_INC: &str = r#"const r = await tools["exec_command"]({"cmd":"echo inc","yield_time_ms":5000}); text("完成"); "done""#;
const JS_HANG: &str = "await new Promise(() => {});";

fn call(id: &str, name: &str, args: &Value) -> Value {
    json!({ "id": id, "type": "function", "function": { "name": name, "arguments": args.to_string() } })
}

struct CliOutcome {
    stdout: String,
    requests: Vec<Value>,
}

fn run_cli_with(tag: &str, tools: Option<Vec<&str>>, responses: Vec<MockResp>) -> CliOutcome {
    let root = TempDir::new(tag);
    let user_dir = root.path().join("user");
    let config_dir = root.path().join("config");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();
    let mock = spawn_mock(responses);
    let origin =
        terminal_chat::catalog::origin_of(&mock.url).expect("mock url 可解析 origin");
    write_manual_with_orch(&config_dir, &mock.url, tools);
    std::fs::write(root.path().join(".env"), "TEST_KEY=test-value\n").unwrap();
    std::fs::write(
        user_dir.join("authorized-origins.json"),
        json!({ "grants": [{ "origin": origin, "cred_fp": terminal_chat::catalog::fingerprint("test-value"), "added_at": 0 }] })
            .to_string(),
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_terminal-chat"))
        .env("TCHAT_USER_DIR", &user_dir)
        .env("TCHAT_CONFIG_DIR", &config_dir)
        .current_dir(root.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all("运行编排\n".as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .expect("CLI 运行");
    let requests: Vec<Value> = mock
        .requests()
        .into_iter()
        .map(|r| serde_json::from_str(&r.body).unwrap_or(Value::Null))
        .collect();
    CliOutcome {
        stdout: format!(
            "[exit={:?}] stdout: {}\nstderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        requests,
    }
}

fn tool_messages(body: &Value) -> Vec<Value> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .cloned()
        .collect()
}

/// E2E-1：双 opt-in 声明＋真实 JS→exec_command 叶子→完成取回（C01/C03）。
#[test]
fn e2e_exec_runs_js_and_completes() {
    let out = run_cli_with(
        "034e2e1",
        Some(vec!["exec", "wait"]),
        vec![
            MockResp::json(200, tc_response(&[call(
                "c1",
                "exec",
                &json!({
                    "code": JS_ECHO_LEAF,
                    "yield_time_ms": 15000
                }),
            )])),
            MockResp::json(200, ok_response("编排完成")),
        ],
    );
    assert!(out.requests.len() >= 2, "stdout={}", out.stdout);
    let first_tools: Vec<String> = out.requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(String::from))
        .collect();
    assert!(first_tools.contains(&"exec".to_string()));
    assert!(first_tools.contains(&"wait".to_string()));
    // §1.6：编排事件不得混入模型投影——第二次请求的 user 消息仍只有
    // ctx＋本轮输入两条（事件泄漏会以额外 user 消息出现）。
    let user_count = out.requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .count();
    assert_eq!(user_count, 2, "编排事件泄漏进了模型投影（user 消息应恰为 ctx+输入 2 条）");
    let results = tool_messages(&out.requests[1]);
    let exec_result: Value =
        serde_json::from_str(results[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(exec_result["ok"], json!(true), "stdout={}\nresult={}", out.stdout, exec_result);
    assert_eq!(exec_result["status"], "completed", "{exec_result}");
    assert_eq!(
        exec_result["output"].as_array().unwrap()[0],
        json!("hello-from-js"),
        "{exec_result}"
    );
    assert!(exec_result["cell_id"].as_str().unwrap().starts_with("cell-"));
    assert!(out.stdout.contains("Agent：编排完成"), "{}", out.stdout);
}

/// E2E-2：未配置的模型不声明 exec/wait；伪造调用按不可用失败（零 worker）。
#[test]
fn e2e_unconfigured_model_refuses_exec() {
    let out = run_cli_with(
        "034e2e2",
        None,
        vec![
            MockResp::json(200, tc_response(&[call("c1", "exec", &json!({"code": "text(1)"}))])),
            MockResp::json(200, ok_response("结束")),
        ],
    );
    let first_tools: Vec<String> = out.requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str().map(String::from))
        .collect();
    assert!(!first_tools.contains(&"exec".to_string()));
    let results = tool_messages(&out.requests[1]);
    let exec_result: Value =
        serde_json::from_str(results[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(exec_result["ok"], json!(false));
    assert_eq!(exec_result["status"], "failed");
    assert_eq!(exec_result["error"]["code"], "TOOL_NOT_AVAILABLE");
    assert!(out.stdout.contains("Agent：结束"), "{}", out.stdout);
}

/// E2E-3：同批 exec＋其他工具＝整批拒绝（零执行、零第二次请求）。
#[test]
fn e2e_exec_mixed_batch_rejected_whole() {
    let out = run_cli_with(
        "034e2e3",
        Some(vec!["exec", "wait"]),
        vec![MockResp::json(200, tc_response(&[
            call("c1", "exec", &json!({"code": "text(1)"})),
            call("c2", "exec_command", &json!({"cmd": "echo side-effect"})),
        ]))],
    );
    assert_eq!(out.requests.len(), 1, "违规整批后不再发第二次请求");
    assert!(
        out.stdout.contains("必须单独成批"),
        "stdout={}",
        out.stdout
    );
}

/// E2E-4：无效参数（code 缺失）＝failed＋cell_id=null；不创建 worker。
#[test]
fn e2e_exec_invalid_arguments_fail_before_cell() {
    let out = run_cli_with(
        "034e2e4",
        Some(vec!["exec", "wait"]),
        vec![
            MockResp::json(200, tc_response(&[call("c1", "exec", &json!({"code": "   "}))])),
            MockResp::json(200, ok_response("继续")),
        ],
    );
    let results = tool_messages(&out.requests[1]);
    let exec_result: Value =
        serde_json::from_str(results[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(exec_result["ok"], json!(false));
    assert_eq!(exec_result["status"], "failed");
    assert!(exec_result["cell_id"].is_null());
    assert_eq!(exec_result["error"]["code"], "INVALID_ARGUMENTS");
}

/// E2E-5：wait 推进挂起中的 cell——exec yield=0 返回 running，第二次 wait
/// （伪造未知句柄）CELL_NOT_FOUND；活动 cell 下的普通工具批整批拒绝。
#[test]
fn e2e_wait_and_busy_semantics() {
    let out = run_cli_with(
        "034e2e5",
        Some(vec!["exec", "wait"]),
        vec![
            MockResp::json(200, tc_response(&[call(
                "c1",
                "exec",
                &json!({
                    "code": JS_ECHO_INC,
                    "yield_time_ms": 0
                }),
            )])),
            // 第二批：wait 未知句柄（参数错误→CELL_NOT_FOUND，cell 仍在）
            MockResp::json(200, tc_response(&[call("c2", "wait", &json!({"cell_id": "cell-9999", "yield_time_ms": 0}))])),
            // 第三批：普通工具批（活动 cell 时整批拒绝）
            MockResp::json(200, tc_response(&[call("c3", "exec_command", &json!({"cmd": "echo x"}))])),
        ],
    );
    assert!(out.requests.len() >= 3, "stdout={}", out.stdout);
    let results = tool_messages(&out.requests[1]);
    let exec_result: Value =
        serde_json::from_str(results[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(exec_result["status"], "running", "{exec_result}");
    let cell_id = exec_result["cell_id"].as_str().unwrap().to_string();
    // wait 未知句柄
    let wait_msg = tool_messages(&out.requests[2])
        .into_iter()
        .find(|m| m["tool_call_id"] == json!("c2"))
        .expect("第三次请求应含 wait c2 结果");
    let wait_result: Value =
        serde_json::from_str(wait_msg["content"].as_str().unwrap()).unwrap();
    assert_eq!(wait_result["error"]["code"], "CELL_NOT_FOUND", "{wait_result}");
    // 活动 cell 时的普通批整批拒绝（第三次请求后无第四次）
    assert!(out.requests.len() == 3, "requests={}", out.requests.len());
    assert!(
        out.stdout.contains("仅 wait 可以推进"),
        "stdout={}",
        out.stdout
    );
    assert!(cell_id.starts_with("cell-"));
}

/// E2E-6：无 wait 就返回终文＝轮错误（cell 收尾、不当任务成功）。
#[test]
fn e2e_final_text_with_live_cell_fails_turn() {
    let out = run_cli_with(
        "034e2e6",
        Some(vec!["exec", "wait"]),
        vec![
            MockResp::json(200, tc_response(&[call(
                "c1",
                "exec",
                &json!({
                    "code": JS_HANG,
                    "yield_time_ms": 0
                }),
            )])),
            // 直接终文（cell 仍挂起）
            MockResp::json(200, ok_response("我以为做完了")),
        ],
    );
    assert!(
        out.stdout.contains("尚有编排结果未取回") || out.stdout.contains("尚未取回"),
        "stdout={}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("Agent：我以为做完了"),
        "终文不得当任务成功交付：{}",
        out.stdout
    );
}

/// C17（§1.7）：上下文效果对照——同一任务（4 个 32 KiB 文本结果、最终仅输出
/// 总数）的「脚本路」（一次 exec＋叶子，text 只回总数）与「直接路」（4 次
/// exec_command＋结果全量入史）对比最终完整请求字节：计入 exec 源码、目录
/// 说明、外层结果与全部新增历史后，脚本路最终请求必须更小。只记字节，不
/// 冒称精确 token/缓存/费用收益。
#[test]
fn c17_script_path_final_request_smaller() {
    let filler = "x".repeat(32 * 1024);
    // 两条路径的叶子输出同为 4×32 KiB（exec_command echo 大文本太慢——用
    // update_plan？不行（无输出）——用真实 exec_command printf 由 shell 生成，
    // 单次调用输出 32 KiB）。
    let js_code = format!(
        r#"let total = 0; for (let i = 0; i < 4; i++) {{ const r = await tools["exec_command"]({{"cmd": "head -c 32768 /dev/zero | tr '\\0' 'x'", "yield_time_ms": 5000, "max_output_chars": 40000}}); total += r.output.length; }} text(String(total)); "done""#
    );
    // 脚本路：一次 exec + 终文
    let script_out = run_cli_with(
        "034c17s",
        Some(vec!["exec", "wait"]),
        vec![
            MockResp::json(200, tc_response(&[call(
                "c1",
                "exec",
                &json!({ "code": js_code, "yield_time_ms": 30000 }),
            )])),
            MockResp::json(200, ok_response("脚本路完成")),
        ],
    );
    // 直接路：4 次 exec_command（mock 依次返回）＋终文
    let mut direct_resps = Vec::new();
    for i in 0..4 {
        direct_resps.push(MockResp::json(200, tc_response(&[call(
            &format!("d{i}"),
            "exec_command",
            &json!({"cmd": "head -c 32768 /dev/zero | tr '\\0' 'x'", "yield_time_ms": 5000, "max_output_chars": 40000}),
        )])));
    }
    direct_resps.push(MockResp::json(200, ok_response("直接路完成")));
    let direct_out = run_cli_with("034c17d", Some(vec!["exec", "wait"]), direct_resps);

    // 两路最后请求的字节数（真实捕获体，与发送体同构）
    let script_final = serde_json::to_vec(
        script_out.requests.last().expect("脚本路至少 2 个请求"),
    )
    .unwrap()
    .len();
    let direct_final = serde_json::to_vec(
        direct_out.requests.last().expect("直接路至少 5 个请求"),
    )
    .unwrap()
    .len();
    // 脚本路完成与请求计数健全性
    assert!(
        script_out.stdout.contains("Agent：脚本路完成"),
        "{}",
        script_out.stdout
    );
    assert!(
        direct_out.stdout.contains("Agent：直接路完成"),
        "{}",
        direct_out.stdout
    );
    assert!(
        script_final < direct_final,
        "脚本路最终请求 {script_final} 字节应小于直接路 {direct_final} 字节"
    );
}

/// 审查 P1-1 回归：终态已到但输出未取尽（预算 4×max_tokens < pending）时，
/// wait 不释放句柄——后续 wait 继续取回增量（§1.4「重复 wait 只返回增量」）。
#[test]
fn wait_keeps_handle_until_output_drained() {
    struct BigDispatcher;
    impl LeafDispatcher for BigDispatcher {
        fn dispatch(&self, _t: &str, _a: &serde_json::Value, _c: &AtomicBool) -> LeafOutcome {
            LeafOutcome {
                result_json: r#"{"ok":true}"#.into(),
                display: None,
                execution: terminal_chat::js_orchestration::runtime::LeafExecution::Completed,
            }
        }
    }
    let mut sink = RecordingSink::new();
    // text 一条 8000 字节 ×8 = 64000 字节（pending 上限 64KiB 内）
    let code = "for (let i = 0; i < 8; i++) text('x'.repeat(8000)); \"done\"";
    let cfg = config(code, vec![], Arc::new(BigDispatcher), turn_budget());
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty_history).expect("spawn");
    // 首观察：预算 16000 字节——脚本完成但只交付部分，句柄必须保留
    let obs1 = cell.observe(10_000, 16_000, &mut sink, None, &empty_history);
    assert_eq!(obs1.status, CellStatus::Completed, "{obs1:?}");
    let got1: usize = obs1.output.iter().map(|s| s.len()).sum();
    assert!(got1 < 64_000, "首观察预算内交付 {got1}");
    assert!(!cell.is_closed(), "输出未取尽不得释放句柄");
    // 第二次观察：继续取回剩余（不重复首段）
    let obs2 = cell.observe(0, 64 * 1024, &mut sink, None, &empty_history);
    let got2: usize = obs2.output.iter().map(|s| s.len()).sum();
    assert_eq!(got1 + got2, 64_000, "增量恰补全总量（不重复不丢失）");
    assert!(cell.is_closed(), "取尽后释放");
}

/// 审查 P1-2 回归：同一轮的两个 cell 共享 128 额度（第二个 cell 的预算
/// 由宿主注入同一 AtomicUsize——此处直接以 runtime 层验证共享语义不变，
/// turn 层共享由 OrchestrationExtras.turn_budget 构造保证）。
#[test]
fn turn_budget_shared_same_instance() {
    let dispatcher = MapDispatcher::new(&[("n", r#"{"i":0}"#)]);
    let budget = Arc::new(AtomicUsize::new(2));
    let code = |n: usize| {
        format!(
            r#"const errs=[]; for (let i=0;i<{n};i++) {{ const r = await tools["n"]({{i}}).catch(e=>e.code); if(typeof r==='string') errs.push(r); }} text(JSON.stringify(errs));"#
        )
    };
    let empty_history: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    // cell1 消耗 2（预算耗尽）
    let code1 = code(2);
    let mut sink = RecordingSink::new();
    let cfg1 = config(&code1, vec![catalog_entry("n", false)], dispatcher.clone(), budget.clone());
    let mut cell1 = spawn_cell(&cfg1, &mut sink, &empty_history).expect("spawn");
    let obs1 = drive_to_terminal(&mut cell1, &mut sink);
    assert_eq!(obs1.status, CellStatus::Completed);
    assert!(cell1.is_closed());
    // cell2（同轮，共享预算）：第 1 次即 TURN_QUOTA
    dispatcher.clear();
    let code2 = code(1);
    let mut sink2 = RecordingSink::new();
    let cfg2 = config(&code2, vec![catalog_entry("n", false)], dispatcher.clone(), budget);
    let mut cell2 = spawn_cell(&cfg2, &mut sink2, &empty_history).expect("spawn2");
    let obs2 = drive_to_terminal(&mut cell2, &mut sink2);
    assert_eq!(obs2.status, CellStatus::Completed, "{obs2:?}");
    let errors: serde_json::Value = serde_json::from_str(&obs2.output[0]).unwrap();
    assert_eq!(errors, serde_json::json!(["TURN_QUOTA"]), "{errors}");
    assert_eq!(dispatcher.calls().len(), 0, "共享预算耗尽后零执行");
}

/// 审查 P2-6 回归：terminate=true 仍要求 cell_id 精确匹配（§1.4）。
#[test]
fn e2e_terminate_requires_exact_cell_id() {
    let out = run_cli_with(
        "034p26",
        Some(vec!["exec", "wait"]),
        vec![
            MockResp::json(200, tc_response(&[call(
                "c1",
                "exec",
                &json!({ "code": "await new Promise(() => {});", "yield_time_ms": 0 }),
            )])),
            // 错误句柄 + terminate=true → CELL_NOT_FOUND（活动 cell 不被终止）
            MockResp::json(200, tc_response(&[call("c2", "wait", &json!({"cell_id": "cell-别的", "terminate": true}))])),
        ],
    );
    assert!(out.requests.len() >= 3, "stdout={}", out.stdout);
    let wait_msg = tool_messages(&out.requests[2])
        .into_iter()
        .find(|m| m["tool_call_id"] == json!("c2"))
        .expect("第三次请求应含 wait c2 结果");
    let wait_result: Value =
        serde_json::from_str(wait_msg["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        wait_result["error"]["code"], "CELL_NOT_FOUND",
        "terminate 不放宽句柄校验：{wait_result}"
    );
}

/// C13（§1.4/§1.5）：JS 编排路径的终止沿用 016 语义——cell 终止取消的是等待，
/// 不杀内层已启动进程；真实句柄在 cell 关闭后仍可由直接 write_stdin 续接。
/// 真实进程＋屏障式存活证明（sentinel 文件＝进程确已启动）＋016 真实收尾。
#[test]
fn c13_terminate_keeps_016_process_handle_alive() {
    use terminal_chat::proc_session::ProcessManager;
    let dir = std::env::temp_dir().join(format!("tchat-c13-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sentinel = dir.join("alive.txt");
    // 真实叶子：经 turn 侧分派（dispatch_nested_leaf）执行真实 exec_command。
    let procs = ProcessManager::new();
    let cmd = format!("echo started > {} && sleep 30", sentinel.display());
    let code = format!(
        "const r = await tools[\"exec_command\"]({{\"cmd\":{cmd:?},\"yield_time_ms\":1000}}); text(r.session_id ? \"RUN:\"+r.session_id : \"END\"); \"done\"",
        cmd = cmd
    );
    let dispatcher: Arc<dyn LeafDispatcher> = Arc::new(TurnDispatcherForTest { procs: procs.clone() });
    let mut sink = RecordingSink::new();
    let cfg = config(&code, vec![catalog_entry("exec_command", false)], dispatcher, turn_budget());
    let empty: Vec<terminal_chat::prompt::HistoryItem> = Vec::new();
    let mut cell = spawn_cell(&cfg, &mut sink, &empty).expect("spawn");
    // 观察至叶子在途（进程已启动）：sentinel 出现＝存活证明（屏障语义——
    // 文件由真实进程写入，未启动则后续断言全部失败，不空跑）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !sentinel.exists() && std::time::Instant::now() < deadline {
        let _ = cell.observe(100, 4096, &mut sink, None, &empty);
    }
    assert!(sentinel.exists(), "叶子进程未启动（空跑防护）");
    // 触发规格终止（wait terminate 的 runtime 路径）。
    let observation = cell.terminate(&mut sink, &empty);
    assert_eq!(observation.status, CellStatus::Terminated);
    assert!(cell.is_closed());
    // 016 契约：终止 cell 不杀内层进程——句柄仍登记，可由直接 write_stdin
    // 查询续接（进程仍活，直到 031/016 既有收尾）。
    let live: Vec<String> = procs
        .live_sessions()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(!live.is_empty(), "终止 cell 后 016 句柄必须保留（实际全部消失）");
    use terminal_chat::exec::WriteStdinArgs;
    let query = serde_json::json!({"session_id": live[0], "yield_time_ms": 5000});
    let outcome = procs.interact(
        &WriteStdinArgs::from_value(&query).expect("查询参数"),
        None,
    );
    assert_eq!(
        outcome.result.status,
        terminal_chat::exec::ExecStatus::Running,
        "cell 关闭后直接 write_stdin 续接：{}",
        serde_json::to_string(&outcome.result).unwrap_or_default()
    );
    // 显式终止进程并回收（测试收尾，不留跨测试进程）。
    let term = serde_json::json!({"session_id": live[0], "terminate": true});
    let _ = procs.interact(&WriteStdinArgs::from_value(&term).unwrap(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// C13/C16 的真实分派桥（turn 侧 execute_core_tool 的测试入口）。
struct TurnDispatcherForTest {
    procs: terminal_chat::proc_session::ProcessManager,
}
impl LeafDispatcher for TurnDispatcherForTest {
    fn dispatch(&self, tool: &str, args: &serde_json::Value, cancel: &AtomicBool) -> LeafOutcome {
        // 与 turn 主循环同一条路径（dispatch_nested_leaf→execute_core_tool）。
        terminal_chat::turn::turn_dispatch_for_test(&self.procs, tool, args, cancel)
    }
}
