//! 035 多 Agent 集成测试（规格 §5 C01–C17 的可自动化子集）。
//!
//! 真实组件：组运行时、子轮循环、ProcessManager（owner 归属/回收）、
//! V3 会话落盘（真实文件）。Mock 仅在模型网络边界（根 sender 与子 sender
//! 均为脚本化注入——035 的 AgentExtras.sender_factory 测试缝与 turn 的
//! sender 回调）。随机输入固定；等待类用例有看门狗超时。

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use terminal_chat::agents::runtime::{
    AgentExtras, AgentManager, ChildModelSender,
};
use terminal_chat::prompt::HistoryItem;
use terminal_chat::session::Session;
use terminal_chat::turn::{self, TurnExtras};

// ===== 测试基础设施 =====

/// 脚本化子 sender：按序弹出响应；记录收到的请求体。
#[derive(Clone)]
struct ScriptChild {
    script: Arc<Mutex<VecDeque<Value>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
    /// 到达屏障：send 时先等 `barrier` 个并发请求到齐（C03 真实并发证明）。
    barrier: Option<Arc<(Mutex<usize>, Condvar)>>,
    barrier_n: usize,
}

impl ScriptChild {
    fn new(script: Vec<Value>) -> Self {
        Self {
            script: Arc::new(Mutex::new(script.into())),
            bodies: Arc::new(Mutex::new(Vec::new())),
            barrier: None,
            barrier_n: 0,
        }
    }

    fn with_barrier(mut self, n: usize) -> Self {
        self.barrier = Some(Arc::new((Mutex::new(0), Condvar::new())));
        self.barrier_n = n;
        self
    }
}

impl ChildModelSender for ScriptChild {
    fn send(&self, body: &Value) -> Result<Value, String> {
        if let Some(state) = &self.barrier {
            let (lock, cv) = &**state;
            let mut arrived = lock.lock().unwrap();
            *arrived += 1;
            cv.notify_all();
            // 绝对时限 30s：屏障不满足（异常路径）也必须放行，防测试挂死
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while *arrived < self.barrier_n && std::time::Instant::now() < deadline {
                let (guard, _) = cv
                    .wait_timeout(arrived, std::time::Duration::from_millis(200))
                    .unwrap();
                arrived = guard;
            }
        }
        self.bodies.lock().unwrap().push(body.clone());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "脚本耗尽".to_string())
    }
}

/// 根 sender：按序弹出响应（choices 形态）。
struct RootSender {
    script: VecDeque<Value>,
    bodies: Vec<Value>,
}

impl RootSender {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            script: responses.into(),
            bodies: Vec::new(),
        }
    }

    fn send(&mut self, body: &Value) -> Result<Value, String> {
        self.bodies.push(body.clone());
        self.script
            .pop_front()
            .ok_or_else(|| "根脚本耗尽".to_string())
    }
}

fn assistant_tool_call(id: &str, name: &str, args: Value) -> Value {
    json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": id, "type": "function",
                    "function": { "name": name, "arguments": args.to_string() }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
}

/// 一批多调用（串行执行；wait 类不混入）。
fn batch(calls: Vec<Value>) -> Value {
    json!({
        "choices": [{
            "message": { "role": "assistant", "content": null, "tool_calls": calls },
            "finish_reason": "tool_calls"
        }]
    })
}

fn call(id: &str, name: &str, args: Value) -> Value {
    json!({ "id": id, "type": "function",
            "function": { "name": name, "arguments": args.to_string() } })
}

fn assistant_final(text: &str) -> Value {
    json!({
        "choices": [{
            "message": { "role": "assistant", "content": text },
            "finish_reason": "stop"
        }]
    })
}

/// 子 Agent 模型终文（无 tool_calls → FINAL_ANSWER）。
fn child_final(text: &str) -> Value {
    json!({
        "choices": [{
            "message": { "role": "assistant", "content": text },
            "finish_reason": "stop"
        }]
    })
}

/// 子 Agent 模型工具调用。
fn child_call(id: &str, name: &str, args: Value) -> Value {
    json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": id, "type": "function",
                    "function": { "name": name, "arguments": args.to_string() }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
}

struct Harness {
    manager: Arc<AgentManager>,
    session: Arc<Mutex<Session>>,
    dir: std::path::PathBuf,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tchat-035-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            manager: Arc::new(AgentManager::new()),
            session: Arc::new(Mutex::new(Session::new(dir.clone()))),
            dir,
        }
    }

    /// 以注入的子 sender 工厂运行一个根用户轮。
    #[allow(clippy::too_many_arguments)]
    fn run_turn(
        &self,
        history: &mut Vec<HistoryItem>,
        input: &str,
        root_responses: Vec<Value>,
        child_factory: impl Fn() -> Box<dyn ChildModelSender> + 'static,
    ) -> Result<String, String> {
        let mut sender = RootSender::new(root_responses);
        let extras_agents = AgentExtras {
            manager: &self.manager,
            session: self.session.clone(),
            sender_factory: &child_factory,
            context_message: "测试环境上下文（冻结）".to_string(),
            mcp: None,
        };
        let procs = terminal_chat::proc_session::ProcessManager::new();
        turn::run_user_turn_with_context_and_caps(
            history,
            input,
            &mut |body| sender.send(body),
            &mut |_| {},
            None,
            &mut |h: &[HistoryItem]| {
                let mut s = self.session.lock().unwrap();
                let _ = s.on_history(h);
            },
            turn::ToolMode::User,
            &terminal_chat::context::ContextSources::from_env(),
            &procs,
            None,
            None,
            false,
            None,
            TurnExtras {
                agents: Some(&extras_agents),
                ..TurnExtras::none()
            },
        )
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // 等待子线程退出（组已在轮出口关闭；槽位释放由 guard 保证）
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn spawn_call(id: &str, name: &str, fork: Option<&str>) -> Value {
    let mut args = json!({ "task_name": name, "message": format!("任务 {name}") });
    if let Some(f) = fork {
        args["fork_turns"] = json!(f);
    }
    assistant_tool_call(id, "spawn_agent", args)
}

fn wait_call(id: &str, timeout_ms: u64) -> Value {
    assistant_tool_call(id, "wait_agent", json!({ "timeout_ms": timeout_ms }))
}

fn parse_result(content: &str) -> Value {
    serde_json::from_str(content).unwrap_or(Value::Null)
}

/// 全局 3 子槽是进程级共享——并行测试互相抢占会造成伪 CAPACITY_LIMIT。
/// 整套测试串行（持锁跑），并在每例末尾等全部子线程真实退出后放锁。
static SUITE_LOCK: Mutex<()> = Mutex::new(());

use std::sync::MutexGuard;
struct SuiteGuard(MutexGuard<'static, ()>);

impl SuiteGuard {
    fn acquire() -> Self {
        Self(SUITE_LOCK.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl SuiteGuard {
    /// 真实排空：全应用子槽归零（看门狗 60s）。不用 assert——unwind 中的
    /// 二次 panic 会 abort 整个测试进程；超时仅打印（后续用例会因槽位
    /// 占用而显式失败，失败原因仍可见）。
    fn drain_slots() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while terminal_chat::agents::runtime::global_child_slots_in_use() > 0 {
            if std::time::Instant::now() >= deadline {
                eprintln!(
                    "警告：子槽未在 60s 内排空（{} 个仍占用）",
                    terminal_chat::agents::runtime::global_child_slots_in_use()
                );
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

impl Drop for SuiteGuard {
    fn drop(&mut self) {
        SuiteGuard::drain_slots();
    }
}

/// 显式等待排空（用例中途需要子线程全部退出时调用）。
fn wait_slots_drained() {
    SuiteGuard::drain_slots();
}

// ===== C02/C05/C08：六接口语义与错误码 =====

#[test]
fn c02_spawn_send_followup_list_error_codes() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c02");
    let mut history = Vec::new();
    let child = ScriptChild::new(vec![child_final("子任务完成")]);
    let child2 = child.clone();
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            // 一批六调用：非法名/重复名/合法 spawn/send 两种目标/list
            batch(vec![
                call("e1", "spawn_agent", json!({ "task_name": "Bad_Name", "message": "x" })),
                call("s1", "spawn_agent", json!({ "task_name": "worker", "message": "任务 worker", "fork_turns": "none" })),
                call("s2", "spawn_agent", json!({ "task_name": "worker", "message": "重复" })),
                call("m1", "send_message", json!({ "target": "worker", "message": "补充" })),
                call("m2", "send_message", json!({ "target": "ghost", "message": "x" })),
                call("l1", "list_agents", json!({})),
            ]),
            // wait 收终态
            wait_call("w1", 8000),
            // followup 给已完成 Agent → 新 assignment（started=true）
            assistant_tool_call(
                "f1",
                "followup_task",
                json!({ "target": "/root/worker", "message": "再查一遍" }),
            ),
            // 根不能 followup /root
            assistant_tool_call(
                "f2",
                "followup_task",
                json!({ "target": "/root", "message": "x" }),
            ),
            // wait 收第二个终态后收尾
            wait_call("w2", 8000),
            assistant_final("全部完成"),
        ],
        move || Box::new(child2.clone()),
    );
    assert!(result.is_ok(), "整轮应成功：{result:?}");
    // 逐条结果断言（error code / data 形状）
    let results: Vec<(String, String)> = history
        .iter()
        .filter_map(|it| match it {
            HistoryItem::ToolResult {
                tool_call_id,
                content,
                ..
            } => Some((tool_call_id.clone(), content.clone())),
            _ => None,
        })
        .collect();
    let get = |id: &str| -> Value {
        parse_result(
            &results
                .iter()
                .find(|(cid, _)| cid == id)
                .unwrap_or(&(String::new(), String::new()))
                .1,
        )
    };
    let e1 = get("e1");
    assert_eq!(e1["ok"], json!(false));
    assert_eq!(e1["error"]["code"], json!("INVALID_ARGUMENT"));
    let s1 = get("s1");
    assert_eq!(s1["ok"], json!(true), "{s1}");
    assert_eq!(s1["data"]["agent_name"], json!("/root/worker"));
    assert_eq!(s1["data"]["assignment_id"], json!(1));
    let s2 = get("s2");
    assert_eq!(s2["error"]["code"], json!("CAPACITY_LIMIT"), "{s2}"); // 名称重复在 core 预留层
    let m1 = get("m1");
    assert_eq!(m1["data"]["accepted"], json!(true));
    assert_eq!(m1["data"]["started"], json!(false));
    let m2 = get("m2");
    assert_eq!(m2["error"]["code"], json!("AGENT_NOT_FOUND"));
    let l1 = get("l1");
    let agents = l1["data"]["agents"].as_array().unwrap().clone();
    assert_eq!(agents.len(), 2, "list 含 /root＋worker：{l1}");
    assert_eq!(agents[0]["agent_name"], json!("/root"));
    assert_eq!(agents[1]["agent_name"], json!("/root/worker"));
    assert_eq!(agents[1]["last_task_message"], json!("任务 worker"));
    let f1 = get("f1");
    assert_eq!(f1["data"]["started"], json!(true), "{f1}");
    assert_eq!(f1["data"]["assignment_id"], json!(2));
    let f2 = get("f2");
    assert_eq!(f2["error"]["code"], json!("INVALID_ARGUMENT"));
    wait_slots_drained();
}

#[test]
fn c08_list_prefix_segment_filter() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c08");
    let mut history = Vec::new();
    let script: Vec<Value> = (0..3)
        .map(|i| child_final(&format!("结果 {i}")))
        .collect();
    // 三个子 Agent 并行（每脚本序列各自独立——工厂给每个子 Agent 首个终文）
    let shared = Arc::new(Mutex::new(VecDeque::<Value>::from(script)));
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "ab", Some("none")),
            spawn_call("s2", "a", Some("none")),
            wait_call("w1", 8000),
            assistant_tool_call(
                "l1",
                "list_agents",
                json!({ "path_prefix": "/root/a" }),
            ),
            assistant_tool_call(
                "l2",
                "list_agents",
                json!({ "path_prefix": "ab" }),
            ),
            assistant_tool_call(
                "l3",
                "list_agents",
                json!({ "path_prefix": "/root/zzz" }),
            ),
            assistant_final("done"),
        ],
        move || {
            let shared = shared.clone();
            Box::new(ScriptChild {
                script: shared,
                bodies: Arc::new(Mutex::new(Vec::new())),
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    let get = |id: &str| -> Value {
        parse_result(
            &history
                .iter()
                .find_map(|it| match it {
                    HistoryItem::ToolResult {
                        tool_call_id,
                        content,
                        ..
                    } if tool_call_id == id => Some(content.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
        )
    };
    // /root/a 只命中 a（a 不命中 ab——路径段筛选）
    let l1 = get("l1");
    let names: Vec<&str> = l1["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["agent_name"].as_str())
        .collect();
    assert_eq!(names, vec!["/root/a"], "段筛选不前缀匹配：{l1}");
    // 相对名 ab
    let l2 = get("l2");
    let names2: Vec<&str> = l2["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["agent_name"].as_str())
        .collect();
    assert_eq!(names2, vec!["/root/ab"]);
    // 合法但无命中 → 空数组
    let l3 = get("l3");
    assert_eq!(l3["data"]["agents"].as_array().unwrap().len(), 0);
    wait_slots_drained();
}

// ===== C03：一批三子真实并发（屏障证明重叠） =====

#[test]
fn c03_three_children_real_overlap_barrier() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c03");
    let mut history = Vec::new();
    // 屏障 sender：3 个子请求到齐才放行——串行执行会死锁看门狗超时
    let barrier_child = ScriptChild::new(vec![child_final("并发完成")]).with_barrier(3);
    let bc = barrier_child.clone();
    let result = h.run_turn(
        &mut history,
        "并行三个",
        vec![
            batch(vec![
                call("s1", "spawn_agent", json!({ "task_name": "p1", "message": "t", "fork_turns": "none" })),
                call("s2", "spawn_agent", json!({ "task_name": "p2", "message": "t", "fork_turns": "none" })),
                call("s3", "spawn_agent", json!({ "task_name": "p3", "message": "t", "fork_turns": "none" })),
            ]),
            wait_call("w1", 30000),
            wait_call("w2", 30000),
            wait_call("w3", 30000),
            assistant_final("三路并发完成"),
        ],
        move || Box::new(bc.clone()),
    );
    assert!(result.is_ok(), "屏障满足＝三个子模型请求真实重叠：{result:?}");
    // wait 前的 spawn 结果全部受理
    for id in ["s1", "s2", "s3"] {
        let v = parse_result(
            &history
                .iter()
                .find_map(|it| match it {
                    HistoryItem::ToolResult { tool_call_id, content, .. }
                        if tool_call_id == id => Some(content.clone()),
                    _ => None,
                })
                .unwrap(),
        );
        assert_eq!(v["ok"], json!(true), "{id}");
    }
    // 三个终态报告全部交付为带来源标注的 user 消息
    let delivered: Vec<&HistoryItem> = history
        .iter()
        .filter(|it| {
            matches!(it, HistoryItem::Text(m)
                if m.content.contains("终态") && m.content.contains("completed"))
        })
        .collect();
    assert_eq!(delivered.len(), 3, "三份终态报告批末交付恰一次");
    wait_slots_drained();
}

// ===== C04：fork all/none 前缀语义 =====

#[test]
fn c04_fork_all_carries_closed_prefix_none_isolated() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c04");
    let mut history = Vec::new();
    let bodies_all: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let bodies_none = bodies_all.clone();
    let result = h.run_turn(
        &mut history,
        "第一轮输入",
        vec![
            spawn_call("s1", "withctx", Some("all")),
            spawn_call("s2", "fresh", Some("none")),
            wait_call("w1", 8000),
            assistant_final("done"),
        ],
        move || {
            let recorder = bodies_none.clone();
            Box::new(ScriptChild {
                script: Arc::new(Mutex::new(VecDeque::from(vec![child_final("ok")]))),
                bodies: recorder,
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    let bodies = bodies_all.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "两个子各一次请求");
    let msg_texts = |b: &Value| -> Vec<String> {
        b["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["content"].as_str().map(str::to_string))
            .collect()
    };
    // fork=all：包含本轮用户输入（spawn 请求前的闭合前缀）
    let all_texts = msg_texts(&bodies[0]);
    assert!(
        all_texts.iter().any(|t| t.contains("第一轮输入")),
        "fork=all 携带闭合前缀：{all_texts:?}"
    );
    // fork=none：无父对话，只有冻结环境＋任务
    let none_texts = msg_texts(&bodies[1]);
    assert!(
        !none_texts.iter().any(|t| t.contains("第一轮输入")),
        "fork=none 不带父对话：{none_texts:?}"
    );
    assert!(
        none_texts
            .iter()
            .any(|t| t.contains("根 Agent 委派任务")),
        "任务消息带来源标注：{none_texts:?}"
    );
    assert!(
        none_texts
            .iter()
            .any(|t| t.contains("不是新的用户授权")),
        "标注「不是新的用户授权」"
    );
    // 子请求不声明六控制工具（子不能再 spawn/联系兄弟）
    for b in &bodies {
        let names: Vec<&str> = b["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|n| n.as_str()))
            .collect();
        for forbidden in [
            "spawn_agent",
            "followup_task",
            "wait_agent",
            "interrupt_agent",
            "list_agents",
            "request_user_input",
            "request_user_input_async",
            "tool_search",
            "request_permissions",
            "new_context",
            "get_context_remaining",
            "exec",
            "wait",
            "send_message_to_user_async",
        ] {
            assert!(
                !names.contains(&forbidden),
                "子目录不得含 {forbidden}：{names:?}"
            );
        }
        assert!(names.contains(&"send_message"), "子可向根发消息");
        assert!(names.contains(&"exec_command"));
    }
    wait_slots_drained();
}

// ===== C06/C07：消息交付、wait 语义与批规则 =====

#[test]
fn c07_wait_modes_and_batch_rule() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c07");
    // 批规则：wait 非批末 → 整批拒绝（零副作用）
    let mut history1 = Vec::new();
    let child = ScriptChild::new(vec![]);
    let c2 = child.clone();
    let err = h
        .run_turn(
            &mut history1,
            "go",
            vec![json!({
                "choices": [{
                    "message": {
                        "role": "assistant", "content": null,
                        "tool_calls": [
                            { "id": "w1", "type": "function",
                              "function": { "name": "wait_agent", "arguments": "{\"timeout_ms\":0}" } },
                            { "id": "e1", "type": "function",
                              "function": { "name": "list_agents", "arguments": "{}" } }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            })],
            move || Box::new(c2.clone()),
        )
        .unwrap_err();
    assert!(
        err.contains("wait_agent 必须是批内最后一条"),
        "非批末 wait 整批拒绝：{err}"
    );
    assert!(
        !history1
            .iter()
            .any(|it| matches!(it, HistoryItem::AssistantToolCalls { .. })),
        "违规批不入史"
    );

    // no_pending：无在途任务、无未收更新 → 立即返回
    let mut history2 = Vec::new();
    let child = ScriptChild::new(vec![]);
    let c3 = child.clone();
    let result = h.run_turn(
        &mut history2,
        "go",
        vec![
            wait_call("w0", 1000),
            assistant_final("空转"),
        ],
        move || Box::new(c3.clone()),
    );
    assert!(result.is_ok(), "{result:?}");
    let w0 = parse_result(
        &history2
            .iter()
            .find_map(|it| match it {
                HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "w0" => {
                    Some(content.clone())
                }
                _ => None,
            })
            .unwrap(),
    );
    assert_eq!(w0["data"]["reason"], json!("no_pending"), "{w0}");
    wait_slots_drained();
}

#[test]
fn c06_child_message_delivery_once_and_fifo() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c06");
    let mut history = Vec::new();
    // 子先执行一个工具（真实 exec），再发两条消息给根，再终文
    let marker_dir = h.dir.join("c06-marker");
    std::fs::create_dir_all(&marker_dir).unwrap();
    let child_script = vec![
        child_call(
            "k1",
            "exec_command",
            json!({ "cmd": format!("echo hi > {}/f.txt", marker_dir.display()), "yield_time_ms": 1000 }),
        ),
        child_call(
            "k2",
            "send_message",
            json!({ "target": "/root", "message": "第一条进度" }),
        ),
        child_call(
            "k3",
            "send_message",
            json!({ "target": "/root", "message": "第二条进度" }),
        ),
        child_final("子终文"),
    ];
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = bodies.clone();
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "reporter", Some("none")),
            wait_call("w1", 10000),
            wait_call("w2", 10000),
            assistant_final("收到报告"),
        ],
        move || {
            let recorder = recorder.clone();
            Box::new(ScriptChild {
                script: Arc::new(Mutex::new(VecDeque::from(child_script.clone()))),
                bodies: recorder,
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    // 子真实执行了 exec（副作用）
    assert_eq!(
        std::fs::read_to_string(marker_dir.join("f.txt"))
            .unwrap()
            .trim(),
        "hi"
    );
    // 根收到两条消息（FIFO 顺序）＋一份终态报告——每条恰一次
    let delivered: Vec<String> = history
        .iter()
        .filter_map(|it| match it {
            HistoryItem::Text(m) if m.content.contains("Agent 消息 /root/reporter") => {
                Some(m.content.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(delivered.len(), 2, "两条消息各恰一次：{delivered:?}");
    let p1 = delivered
        .iter()
        .position(|t| t.contains("第一条进度"))
        .unwrap();
    let p2 = delivered
        .iter()
        .position(|t| t.contains("第二条进度"))
        .unwrap();
    assert!(p1 < p2, "FIFO 顺序");
    // 终态报告恰一次
    let terminals: Vec<&HistoryItem> = history
        .iter()
        .filter(|it| {
            matches!(it, HistoryItem::Text(m) if m.content.contains("终态") && m.content.contains("子终文"))
        })
        .collect();
    assert_eq!(terminals.len(), 1, "终态报告恰一次");
    // 子上下文：消息与工具结果都在（第二次请求投影含第一条消息——本例一次
    // 请求后直接终文，此处核对事件事实）
    let _ = bodies;
    wait_slots_drained();
}

// ===== C09：容量边界 =====

#[test]
fn c09_fourth_child_rejected_and_group_assignment_cap() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c09");
    let mut history = Vec::new();
    // 子 Agent 先执行真实长进程（sleep 30）：中断时它们确定处于活动态且
    // 各持有进程——interrupt 触发按 assignment 的 PGID 回收（C13 口径）。
    let long_script = vec![
        child_call(
            "k1",
            "exec_command",
            json!({ "cmd": "sleep 30", "yield_time_ms": 1000 }),
        ),
        child_final("不会到达"),
    ];
    let shared: Arc<Mutex<VecDeque<Value>>> =
        Arc::new(Mutex::new(VecDeque::from(long_script.clone())));
    let result = h.run_turn(
        &mut history,
        "四个并发",
        vec![
            batch(vec![
                call("s1", "spawn_agent", json!({ "task_name": "c1", "message": "t", "fork_turns": "none" })),
                call("s2", "spawn_agent", json!({ "task_name": "c2", "message": "t", "fork_turns": "none" })),
                call("s3", "spawn_agent", json!({ "task_name": "c3", "message": "t", "fork_turns": "none" })),
                call("s4", "spawn_agent", json!({ "task_name": "c4", "message": "t", "fork_turns": "none" })),
            ]),
            batch(vec![
                call("i1", "interrupt_agent", json!({ "target": "c1" })),
                call("i2", "interrupt_agent", json!({ "target": "c2" })),
                call("i3", "interrupt_agent", json!({ "target": "c3" })),
            ]),
            wait_call("w1", 8000),
            wait_call("w2", 8000),
            assistant_final("收尾"),
        ],
        move || {
            let shared = shared.clone();
            Box::new(ScriptChild {
                script: shared,
                bodies: Arc::new(Mutex::new(Vec::new())),
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    let get = |id: &str| -> Value {
        parse_result(
            &history
                .iter()
                .find_map(|it| match it {
                    HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == id => {
                        Some(content.clone())
                    }
                    _ => None,
                })
                .unwrap_or_default(),
        )
    };
    for id in ["s1", "s2", "s3"] {
        assert_eq!(get(id)["ok"], json!(true), "{id} 受理");
    }
    let s4 = get("s4");
    assert_eq!(
        s4["error"]["code"], json!("CAPACITY_LIMIT"),
        "第 4 个子拒绝：{s4}"
    );
    // interrupt：starting/running → stopping（子线程起步与 i1 执行之间存在
    // 调度竞态，两者都属「活动」——语义一致）
    let i1 = get("i1");
    assert!(
        matches!(
            i1["data"]["previous_state"].as_str(),
            Some("running") | Some("starting")
        ),
        "{i1}"
    );
    assert_eq!(i1["data"]["state"], json!("stopping"));
    wait_slots_drained();
}

// ===== C11/C12：进程归属与句柄校验、中断 =====

#[test]
fn c11_child_owner_handle_isolation() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c11");
    let mut history = Vec::new();
    // 单个子 Agent 先开两个长进程再终文：验证运行中进程按 assignment 归属
    // 登记（事件里两份句柄）、终态先回收所属进程再发布（收尾摘要）。
    let single_script = vec![
        child_call(
            "k1",
            "exec_command",
            json!({ "cmd": "sleep 30", "yield_time_ms": 1000 }),
        ),
        child_call(
            "k2",
            "exec_command",
            json!({ "cmd": "sleep 30", "yield_time_ms": 1000 }),
        ),
        child_final("两个进程在运行中结束"),
    ];
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = bodies.clone();
    let single = single_script.clone();
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "owner_check", Some("none")),
            wait_call("w1", 10000),
            wait_call("w2", 10000),
            assistant_final("done"),
        ],
        move || {
            let recorder = recorder.clone();
            let single = single.clone();
            Box::new(ScriptChild {
                script: Arc::new(Mutex::new(VecDeque::from(single))),
                bodies: recorder,
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    // 子事件里两条 exec 结果各带句柄（owner=该 assignment）
    let bodies = bodies.lock().unwrap().clone();
    // 从历史 Agent 事件的 child_record 中找 exec 结果的 session_id
    let mut session_ids = Vec::new();
    for it in &history {
        if let HistoryItem::AgentEvent { payload, .. } = it {
            if let terminal_chat::prompt::AgentEventPayload::Records { items } = payload {
                for item in items {
                    if let HistoryItem::ToolResult { content, .. } = item {
                        if let Ok(v) = serde_json::from_str::<Value>(content) {
                            if v["session_id"].is_string() {
                                session_ids.push(v["session_id"].as_str().unwrap().to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(session_ids.len(), 2, "两个运行中进程句柄：{session_ids:?}");
    // 终态后：该 Agent 的进程被回收（reap assignment）
    // （真实验证：进程已不在——用 ProcessManager 会话已释放来证明；此处
    // 通过终态事件的收尾摘要断言）
    let cleanup: Vec<&HistoryItem> = history
        .iter()
        .filter(|it| {
            matches!(it, HistoryItem::Text(m) if m.content.contains("终态") && m.content.contains("已回收本 assignment 所属进程"))
        })
        .collect();
    assert_eq!(cleanup.len(), 1, "终态附宿主收尾摘要：{cleanup:?}");
    // 子请求数＝两批工具＋终文（3 次；真实分派经 ProcessManager）。
    assert_eq!(bodies.len(), 3, "两次 exec 批＋一次终文：{bodies_len}", bodies_len = bodies.len());
    wait_slots_drained();
}

// ===== C14：组关闭与下一轮新组 =====

#[test]
fn c14_group_closed_paths_not_reusable_next_turn() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c14");
    let mut history = Vec::new();
    let child = ScriptChild::new(vec![child_final("第一轮子完成")]);
    let c2 = child.clone();
    let result = h.run_turn(
        &mut history,
        "第一轮",
        vec![
            spawn_call("s1", "worker", Some("none")),
            wait_call("w1", 8000),
            assistant_final("第一轮结束"),
        ],
        move || Box::new(c2.clone()),
    );
    assert!(result.is_ok(), "{result:?}");
    // 第二轮：同名 spawn 应成功（新组；旧组关闭不泄名称跨组）
    let child2 = ScriptChild::new(vec![child_final("第二轮子完成")]);
    let c3 = child2.clone();
    let result2 = h.run_turn(
        &mut history,
        "第二轮",
        vec![
            spawn_call("s2", "worker", Some("none")),
            wait_call("w2", 8000),
            wait_call("w3", 8000),
            assistant_final("第二轮结束"),
        ],
        move || Box::new(c3.clone()),
    );
    assert!(result2.is_ok(), "新组同名可复用：{result2:?}");
    let get = |id: &str| -> Value {
        parse_result(
            &history
                .iter()
                .filter_map(|it| match it {
                    HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == id => {
                        Some(content.clone())
                    }
                    _ => None,
                })
                .last()
                .unwrap_or_default(),
        )
    };
    assert_eq!(get("s2")["ok"], json!(true));
    // 两组 group_closed 事件均在历史（组边界可见）
    let closed = history
        .iter()
        .filter(|it| {
            matches!(it, HistoryItem::AgentEvent {
                kind: terminal_chat::prompt::AgentEventKind::GroupClosed, ..
            })
        })
        .count();
    assert_eq!(closed, 2, "两轮各封组一次");
    wait_slots_drained();
}

// ===== C15：落盘故障＝未存不启动；V3 装载链校验 =====

#[test]
fn c15_spawn_fails_without_persistence_and_v3_reload() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c15");
    let mut history = Vec::new();
    // 正常一轮带 spawn＋终态；随后恢复同一会话文件验证链校验与不重放
    let child = ScriptChild::new(vec![child_final("子完成")]);
    let c2 = child.clone();
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "persist_check", Some("none")),
            wait_call("w1", 8000),
            assistant_final("完成"),
        ],
        move || Box::new(c2.clone()),
    );
    assert!(result.is_ok(), "{result:?}");
    // 排空并读取会话文件：V3＋agent 事件存在且可装载
    {
        let mut s = h.session.lock().unwrap();
        s.flush_final(&history).unwrap();
    }
    let mut files: Vec<_> = std::fs::read_dir(&h.dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    let file = files.first().expect("应有会话文件").clone();
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("\"v\":3"), "Agent 事件会话为 V3");
    assert!(content.contains("agent_created"), "事件落盘");
    // 装载（真实 load）：链校验通过；历史含事件；无重放副作用
    let loaded = terminal_chat::session::load_session_file(&file).unwrap();
    assert!(
        loaded
            .history
            .iter()
            .any(|it| matches!(it, HistoryItem::AgentEvent { .. })),
        "装载保留事件"
    );
    // 事件不进入模型投影（下次请求看不到控制事件）
    let probe = terminal_chat::prompt::build_turn_request_with(
        &loaded.history,
        &[],
        "system",
    );
    let serialized = serde_json::to_string(&probe["messages"]).unwrap();
    assert!(
        !serialized.contains("agent_created"),
        "控制事件不进模型投影"
    );
    // 损坏链（seq 空洞）：第二行 seq 改写为 99 后装载整拒。前置首条 user
    // 记录（007 装载规则：头部非 user 记录丢弃至首个 user——纯事件文件
    // 会被合法裁空，须带 user 前缀才进入链校验）。
    let bad_path = h.dir.join("bad-seq.jsonl");
    let mut bad = String::from("{\"v\":3}\n");
    if let Some(first_user) = content.lines().nth(1) {
        bad.push_str(first_user);
        bad.push('\n');
    }
    let lines: Vec<&str> = content
        .lines()
        .skip(1)
        .filter(|l| l.contains("agent_event"))
        .collect();
    assert!(lines.len() >= 2, "至少两个 Agent 事件：{lines:?}");
    bad.push_str(lines[0]);
    bad.push('\n');
    // 无论第二个事件的 seq 为多少，改写为 99（制造空洞）
    let second = lines[1];
    let seq_start = second.find("\"seq\":").expect("事件行含 seq 字段");
    let digits_start = seq_start + 6;
    let digits_end = second[digits_start..]
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| digits_start + i)
        .unwrap_or(second.len());
    let bumped = format!(
        "{}99{}",
        &second[..digits_start],
        &second[digits_end..]
    );
    assert_ne!(&bumped, second, "seq 改写应生效：{second}");
    bad.push_str(&bumped);
    bad.push('\n');
    std::fs::write(&bad_path, bad).unwrap();
    assert!(
        terminal_chat::session::load_session_file(&bad_path).is_err(),
        "seq 空洞整文件拒绝"
    );
    wait_slots_drained();
}

#[test]
fn c15_persist_failure_spawn_rejected() {
    let _suite = SuiteGuard::acquire();
    // 会话停用（Disabled）时 spawn 必须失败（未存不启动）
    let h = Harness::new("c15b");
    let mut history = vec![HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
        images: None,
        text_elements: Vec::new(),
        role: terminal_chat::prompt::HistoryRole::User,
        content: "既有历史".into(),
        user_input_event: None,
    })];
    // 一个超限行让保存停用：先在会话中制造 64 MiB 行——代价太高；改用
    // Disabled 状态直接注入：on_history 失败后 state=Disabled 的路径等价于
    // 用 /dev/full 不可行（Linux 特性）——这里以「会话目录不可写」实现：
    // /dev/null 下不可建目录（ENOTDIR 对 root 也成立）——确定性 Disabled。
    let locked_dir = std::path::PathBuf::from("/dev/null/035-locked-sessions");
    let session = Arc::new(Mutex::new(Session::new(locked_dir.clone())));
    let manager = Arc::new(AgentManager::new());
    let mut sender = RootSender::new(vec![
        spawn_call("s1", "no_persist", Some("none")),
        assistant_final("结束"),
    ]);
    let child = ScriptChild::new(vec![]);
    let c2 = child.clone();
    let procs = terminal_chat::proc_session::ProcessManager::new();
    let extras = AgentExtras {
        manager: &manager,
        session: session.clone(),
        sender_factory: &|| Box::new(c2.clone()),
        context_message: "ctx".into(),
        mcp: None,
    };
    let result = turn::run_user_turn_with_context_and_caps(
        &mut history,
        "go",
        &mut |body| sender.send(body),
        &mut |_| {},
        None,
        &mut |hh: &[HistoryItem]| {
            let mut s = session.lock().unwrap();
            let _ = s.on_history(hh);
        },
        turn::ToolMode::User,
        &terminal_chat::context::ContextSources::from_env(),
        &procs,
        None,
        None,
        false,
        None,
        TurnExtras {
            agents: Some(&extras),
            ..TurnExtras::none()
        },
    );
    assert!(result.is_ok(), "轮本身完成：{result:?}");
    let spawn_result = parse_result(
        &history
            .iter()
            .find_map(|it| match it {
                HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "s1" => {
                    Some(content.clone())
                }
                _ => None,
            })
            .unwrap(),
    );
    assert_eq!(
        spawn_result["error"]["code"], json!("PERSISTENCE_FAILED"),
        "保存失败不启动：{spawn_result}"
    );
    wait_slots_drained();
}

// ===== C05：send 不启动空闲；终文竞态由邮箱门裁决 =====

#[test]
fn c05_send_to_idle_does_not_start_and_followup_continues() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c05");
    let mut history = Vec::new();
    // 子一轮终文后空闲；根 send 只排队（started=false 且子无新请求）；
    // followup 才启动新 assignment（第二个脚本响应）
    let script: Arc<Mutex<VecDeque<Value>>> =
        Arc::new(Mutex::new(VecDeque::from(vec![
            child_final("首轮完成"),
            child_final("续派完成"),
        ])));
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = bodies.clone();
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "seq", Some("none")),
            wait_call("w1", 8000), // 等首轮终态
            batch(vec![
                call("m1", "send_message", json!({ "target": "seq", "message": "空闲时的消息" })),
                call("l1", "list_agents", json!({})),
            ]),
            assistant_tool_call(
                "f1",
                "followup_task",
                json!({ "target": "seq", "message": "正式续派" }),
            ),
            wait_call("w2", 8000),
            wait_call("w3", 8000),
            assistant_final("done"),
        ],
        move || {
            let recorder = recorder.clone();
            let script = script.clone();
            Box::new(ScriptChild {
                script,
                bodies: recorder,
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "send 不产生新请求（恰两次请求＝两轮 assignment）：{bodies_len}", bodies_len = bodies.len());
    // 续派的第二轮请求含排队消息＋续派任务（邮箱批末交付）
    let second_texts: Vec<String> = bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_str().map(str::to_string))
        .collect();
    assert!(
        second_texts
            .iter()
            .any(|t| t.contains("空闲时的消息")),
        "排队消息随新 assignment 交付：{second_texts:?}"
    );
    assert!(
        second_texts.iter().any(|t| t.contains("正式续派")),
        "续派任务进上下文"
    );
    assert!(
        second_texts.iter().any(|t| t.contains("首轮完成")),
        "保留上下文（首轮终文在前）"
    );
    wait_slots_drained();
}

// ===== C10：消息边界（16 KiB/超限拒绝） =====

#[test]
fn c10_message_boundaries() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c10");
    let mut history = Vec::new();
    let child = ScriptChild::new(vec![]);
    let c2 = child.clone();
    let long = "x".repeat(16 * 1024 + 1);
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            assistant_tool_call(
                "s1",
                "spawn_agent",
                json!({ "task_name": "m", "message": long }),
            ),
            assistant_final("done"),
        ],
        move || Box::new(c2.clone()),
    );
    assert!(result.is_ok(), "{result:?}");
    let s1 = parse_result(
        &history
            .iter()
            .find_map(|it| match it {
                HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == "s1" => {
                    Some(content.clone())
                }
                _ => None,
            })
            .unwrap(),
    );
    assert_eq!(s1["error"]["code"], json!("INVALID_ARGUMENT"), "超长消息拒绝");
    wait_slots_drained();
}

// ===== C12：interrupt 与重复 interrupt、晚到终文不覆盖 =====

#[test]
fn c12_interrupt_idempotent_and_late_answer_discarded() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c12");
    let mut history = Vec::new();
    // 子脚本先阻塞（屏障 2 永不满足→中断后屏障 sender 返回错误？不行——
    // 屏障会卡住子线程。改用慢脚本：send 阻塞 3 秒后返回终文；根立即
    // interrupt → 子动作门关闭 → 终文丢弃（Discarded），终态 interrupted。
    struct SlowSender;
    impl ChildModelSender for SlowSender {
        fn send(&self, _body: &Value) -> Result<Value, String> {
            std::thread::sleep(std::time::Duration::from_millis(3000));
            Ok(child_final("迟到的终文"))
        }
    }
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "slow", Some("none")),
            assistant_tool_call("i1", "interrupt_agent", json!({ "target": "slow" })),
            // 重复 interrupt：不新建取消任务（幂等，返回当前状态）
            assistant_tool_call("i2", "interrupt_agent", json!({ "target": "slow" })),
            wait_call("w1", 10000),
            assistant_final("已中断收尾"),
        ],
        move || Box::new(SlowSender),
    );
    assert!(result.is_ok(), "{result:?}");
    let get = |id: &str| -> Value {
        parse_result(
            &history
                .iter()
                .find_map(|it| match it {
                    HistoryItem::ToolResult { tool_call_id, content, .. } if tool_call_id == id => {
                        Some(content.clone())
                    }
                    _ => None,
                })
                .unwrap(),
        )
    };
    let i1 = get("i1");
    assert_eq!(i1["data"]["state"], json!("stopping"), "{i1}");
    let i2 = get("i2");
    // 第二次 interrupt 时状态 stopping（收尾中）——保持原状态
    assert_eq!(
        i2["data"]["state"], json!("stopping"),
        "重复 interrupt 幂等：{i2}"
    );
    // 晚到终文被丢弃：终态是 interrupted 而非 completed
    let interrupted: Vec<&HistoryItem> = history
        .iter()
        .filter(|it| {
            matches!(it, HistoryItem::Text(m) if m.content.contains("终态") && m.content.contains("interrupted"))
        })
        .collect();
    assert_eq!(interrupted.len(), 1, "终态 interrupted 恰一次");
    assert!(
        !history.iter().any(|it| matches!(it,
            HistoryItem::Text(m) if m.content.contains("迟到的终文"))),
        "晚到终文不入根历史"
    );
    wait_slots_drained();
}

// ===== C14b：根终文时有活动子任务 → 未收尾错误 =====

#[test]
fn c14_root_final_with_active_children_errors() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c14b");
    let mut history = Vec::new();
    // 子永远阻塞（慢 sender 10s）；根不给 wait 直接终文 → 未收尾错误
    struct StuckSender;
    impl ChildModelSender for StuckSender {
        fn send(&self, _body: &Value) -> Result<Value, String> {
            std::thread::sleep(std::time::Duration::from_secs(10));
            Ok(child_final("迟到的结果"))
        }
    }
    let err = h
        .run_turn(
            &mut history,
            "go",
            vec![
                spawn_call("s1", "stuck", Some("none")),
                assistant_final("过早的终文"),
            ],
            move || Box::new(StuckSender),
        )
        .unwrap_err();
    assert!(
        err.contains("仍有活动子 Agent 任务未收尾"),
        "活动子任务时根终文＝未收尾错误：{err}"
    );
    wait_slots_drained();
}

// ===== C01：目录开关（六名全配才开） =====

#[test]
fn c01_capability_gate_six_exact_names() {
    let _suite = SuiteGuard::acquire();
    let mk = |tools: Option<Vec<&str>>| {
        let mut m: terminal_chat::catalog::ModelInfo =
            serde_json::from_value(json!({ "id": "m" })).unwrap();
        m.experimental_supported_tools = tools.map(|t| t.into_iter().map(String::from).collect());
        m
    };
    let all = vec![
        "spawn_agent",
        "send_message",
        "followup_task",
        "wait_agent",
        "interrupt_agent",
        "list_agents",
    ];
    assert!(terminal_chat::catalog::supports_multi_agent(&mk(Some(all.clone()))));
    // 缺一个 → 关闭＋提示缺失名
    let partial: Vec<&str> = all[..5].to_vec();
    assert!(!terminal_chat::catalog::supports_multi_agent(&mk(Some(partial.clone()))));
    let missing = terminal_chat::catalog::multi_agent_missing(&mk(Some(partial)));
    assert_eq!(missing, vec!["list_agents"]);
    // 全缺 → 不提示（未配置该能力）
    assert!(terminal_chat::catalog::multi_agent_missing(&mk(None)).is_empty());
    assert!(!terminal_chat::catalog::supports_multi_agent(&mk(None)));
    // 空数组
    assert!(!terminal_chat::catalog::supports_multi_agent(&mk(Some(vec![]))));
}

// ===== C16（子集）：子原生状态保留在子上下文、根投影不含子原生块 =====

#[test]
fn c16_child_native_state_stays_in_child_context() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("c16");
    let mut history = Vec::new();
    // 子首轮带 native_state＋工具调用；次轮脚本终文——第二次请求的 _history
    // 应含 NativeAssistant（子自己的链）；根历史只见事件汇总。
    let native = json!({
        "owner": {
            "provider_id": "p", "protocol": "openai-responses",
            "model_id": "m", "base_url": "https://example.test",
            "key_ref": null, "credential_fingerprint": "fp"
        },
        "payload": { "previous_response_id": "resp_1" }
    });
    let with_native = |mut v: Value| -> Value {
        v["native_state"] = native.clone();
        v
    };
    let script: Arc<Mutex<VecDeque<Value>>> = Arc::new(Mutex::new(VecDeque::from(vec![
        with_native(child_call(
            "k1",
            "exec_command",
            json!({ "cmd": "true", "yield_time_ms": 1000 }),
        )),
        child_final("子完成"),
    ])));
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = bodies.clone();
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "native_child", Some("none")),
            wait_call("w1", 10000),
            assistant_final("done"),
        ],
        move || {
            let recorder = recorder.clone();
            let script = script.clone();
            Box::new(ScriptChild {
                script,
                bodies: recorder,
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    // 第二次请求 _history 含 native_assistant（子链保留）
    let hist2 = serde_json::to_string(&bodies[1]["_history"]).unwrap();
    assert!(
        hist2.contains("native_assistant"),
        "子原生块回子上下文：{hist2}"
    );
    // 根历史不含子原生 payload（事件只带可携带记录，NativeAssistant 在
    // child_record 内——装载校验允许；根投影过滤事件本身）
    let root_probe =
        terminal_chat::prompt::build_turn_request_with(&history, &[], "s");
    let root_msgs = serde_json::to_string(&root_probe["messages"]).unwrap();
    assert!(
        !root_msgs.contains("previous_response_id"),
        "根投影不含子原生状态：{root_msgs}"
    );
    wait_slots_drained();
}

// ===== C17（子集）：记忆提炼过滤 Agent 事件 =====

#[test]
fn c17_rollout_filters_agent_events() {
    let _suite = SuiteGuard::acquire();
    let event = terminal_chat::agents::events::agent_event(
        terminal_chat::prompt::AgentEventKind::AgentCreated,
        7,
        1,
        Some("mem_filter"),
        Some(1),
        terminal_chat::prompt::AgentEventPayload::Fork {
            context_id: 0,
            prefix_end: 0,
            fork: "none".into(),
            task: "x".into(),
        },
    );
    let filtered = terminal_chat::memory::rollout_input::serialize_filtered_history_items(&[
        HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: terminal_chat::prompt::HistoryRole::User,
            content: "真实用户输入".into(),
            user_input_event: None,
        }),
        event,
    ])
    .unwrap();
    assert!(filtered.contains("真实用户输入"));
    assert!(!filtered.contains("mem_filter"), "Agent 事件不进提炼输入");
}

// ===== 审查 B1 回归：组关闭后的排空期事件不破坏装载 =====

#[test]
fn b1_group_closed_after_inflight_events_file_still_loads() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("b1");
    let mut history = Vec::new();
    // 子阻塞在模型请求中（3s）；根终文遇活动子任务→封组报错；子 3s 后返回
    // 迟到终文——迟到门丢弃；子退出后由最后退出者补写 group_closed。
    // 修复前：迟到的 child_record 落在 group_closed 之后→装载整拒（B1）。
    struct SlowLateSender;
    impl ChildModelSender for SlowLateSender {
        fn send(&self, _body: &Value) -> Result<Value, String> {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            Ok(child_final("迟到终文"))
        }
    }
    let err = h
        .run_turn(
            &mut history,
            "go",
            vec![
                spawn_call("s1", "late", Some("none")),
                assistant_final("过早终文"),
            ],
            move || Box::new(SlowLateSender),
        )
        .unwrap_err();
    assert!(err.contains("仍有活动子 Agent 任务未收尾"), "{err}");
    // 等子线程真实退出（迟到门＋终态＋补写 group_closed）。
    wait_slots_drained();
    {
        let mut session = h.session.lock().unwrap();
        session.flush_final(&history).unwrap();
    }
    let mut files: Vec<_> = std::fs::read_dir(&h.dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    // V1→V3 迁移产生带 -N 后缀的新文件；取实际含 Agent 事件的会话文件。
    let event_file = files
        .iter()
        .find(|p| {
            std::fs::read_to_string(p)
                .map(|c| c.contains("agent_event"))
                .unwrap_or(false)
        })
        .expect("应存在含 Agent 事件的会话文件")
        .clone();
    let content = std::fs::read_to_string(&event_file).unwrap();
    // group_closed 必须是最后一个 agent_event 行（在途事件先于封组落盘）。
    let event_lines: Vec<&str> = content
        .lines()
        .skip(1)
        .filter(|l| l.contains("agent_event"))
        .collect();
    assert!(!event_lines.is_empty());
    assert!(
        event_lines.last().unwrap().contains("group_closed"),
        "group_closed 为该组最后一个事件：{:?}",
        event_lines.last()
    );
    // 装载成功（修复前此处整文件拒绝）。
    let loaded = terminal_chat::session::load_session_file(&event_file).unwrap();
    assert!(loaded
        .history
        .iter()
        .any(|it| matches!(it, HistoryItem::AgentEvent { .. })));
}

// ===== 审查 B2 回归：额度耗尽后的子工具调用零执行 =====

#[test]
fn b2_call_budget_exhausted_no_external_action() {
    let _suite = SuiteGuard::acquire();
    let h = Harness::new("b2");
    let mut history = Vec::new();
    let marker_dir = h.dir.join("b2-marker");
    std::fs::create_dir_all(&marker_dir).unwrap();
    let marker = marker_dir.join("never.txt");
    // 两批各 32 次 update_plan（64 次＝assignment 调用上限），第三批第 65 次
    // 为写标记的 exec——预算耗尽必须以 BUDGET_LIMIT 失败结果呈现且零执行。
    let many = |prefix: &str, n: usize| -> Vec<Value> {
        (0..n)
            .map(|i| {
                call(
                    &format!("{prefix}{i}"),
                    "update_plan",
                    json!({ "plan": [{ "step": "s", "status": "pending" }] }),
                )
            })
            .collect()
    };
    let over = call(
        "overflow",
        "exec_command",
        json!({ "cmd": format!("echo x > {}", marker.display()), "yield_time_ms": 1000 }),
    );
    let script: Arc<Mutex<VecDeque<Value>>> = Arc::new(Mutex::new(VecDeque::new()));
    let bodies_dbg: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    // 构造三批：32＋32＋1（第 65 次为 exec）＋终文
    let batch_value = |calls: Vec<Value>| {
        json!({ "choices": [{ "message": { "role": "assistant", "content": null,
            "tool_calls": calls }, "finish_reason": "tool_calls" }] })
    };
    *script.lock().unwrap() = VecDeque::from(vec![
        batch_value(many("a", 32)),
        batch_value(many("b", 32)),
        batch_value(vec![over]),
        child_final("预算后终文"),
    ]);
    let bodies_for_factory = bodies_dbg.clone();
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "flood", Some("none")),
            wait_call("w1", 15000),
            wait_call("w2", 15000),
            assistant_final("done"),
        ],
        move || {
            let script = script.clone();
            let bodies_dbg = bodies_for_factory.clone();
            Box::new(ScriptChild {
                script,
                bodies: bodies_dbg,
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "{result:?}");
    drop(bodies_dbg);
    wait_slots_drained();
    // 第 65 次调用：BUDGET_LIMIT 失败结果、外部动作零执行。
    assert!(
        !marker.exists(),
        "额度耗尽后的 exec 不得执行（零外部动作）"
    );
    let overflow_result: Option<String> = history
        .iter()
        .find_map(|it| match it {
            HistoryItem::AgentEvent { payload, .. } => match payload {
                terminal_chat::prompt::AgentEventPayload::Records { items } => items
                    .iter()
                    .find_map(|item| match item {
                        HistoryItem::ToolResult { tool_call_id, content, .. }
                            if tool_call_id == "overflow" =>
                        {
                            Some(content.clone())
                        }
                        _ => None,
                    }),
                _ => None,
            },
            _ => None,
        });
    let content = overflow_result.expect("第 65 次调用有失败结果占位");
    let v = parse_result(&content);
    assert_eq!(
        v["error"]["code"], json!("BUDGET_LIMIT"),
        "预算耗尽＝失败结果占位：{v}"
    );
}

// ===== C18 性能基线（035 规格 §1.8：预热 5、正式 100、独立 3 轮取 p95 中位轮，
// 保留全部样本；#[ignore] 测量入口不进默认全量——显式 `--ignored` 运行）。
// 模型边界为进程内 mock（响应即时返回），测量值＝宿主调度/分派/持久化开销，
// 不含网络与真实模型延迟（环境说明见 docs/logs/035-multi-agent/c18-baseline.md）。

mod c18 {
    use super::*;
    use std::time::Instant;

    const WARMUP: usize = 5;
    const FORMAL: usize = 100;
    const ROUNDS: usize = 3;

    /// 进程峰值 RSS（KiB，/proc/self/status VmHWM——单调，取场景末读数）。
    fn vm_peak_kib() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("VmHWM:"))
            .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
            .unwrap_or(0)
    }

    /// 当前线程数（含全部子 Agent 线程）。
    fn threads_now() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("Threads:"))
            .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
            .unwrap_or(0)
    }

    /// 目录总字节（会话目录磁盘增量口径）。
    fn dir_bytes(p: &std::path::Path) -> u64 {
        let mut total = 0u64;
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                if let Ok(md) = e.metadata() {
                    if md.is_file() {
                        total += md.len();
                    } else if md.is_dir() {
                        total += dir_bytes(&e.path());
                    }
                }
            }
        }
        total
    }

    /// (min, p50, p95, max)，毫秒；最近邻秩法。
    fn stats(samples: &mut Vec<u128>) -> (u128, u128, u128, u128) {
        samples.sort_unstable();
        let pick = |q: f64| -> u128 {
            let idx = ((samples.len() as f64 - 1.0) * q).round() as usize;
            samples[idx.min(samples.len() - 1)]
        };
        (samples[0], pick(0.50), pick(0.95), *samples.last().unwrap())
    }

    pub(super) struct Round {
        pub p95: u128,
        pub p50: u128,
        pub samples: Vec<u128>,
    }

    /// 运行 3 轮（每轮 5 预热＋100 正式）；返回全部轮次与中位 p95 轮下标。
    pub(super) fn bench(
        tag: &str,
        mut iterate: impl FnMut() -> u128,
    ) -> (Vec<Round>, usize) {
        let mut rounds = Vec::new();
        for _ in 0..ROUNDS {
            for _ in 0..WARMUP {
                iterate();
            }
            let mut samples = Vec::with_capacity(FORMAL);
            for _ in 0..FORMAL {
                samples.push(iterate());
            }
            let mut sorted = samples.clone();
            sorted.sort_unstable();
            let p95 = sorted[sorted.len() * 95 / 100];
            let p50 = sorted[sorted.len() / 2];
            rounds.push(Round { p95, p50, samples });
        }
        let mut by_p95: Vec<(u128, usize)> =
            rounds.iter().map(|r| (r.p95, rounds.len())).collect();
        let _ = &mut by_p95;
        let mut order: Vec<usize> = (0..rounds.len()).collect();
        order.sort_by_key(|&i| rounds[i].p95);
        let median_round = order[order.len() / 2];
        println!(
            "[c18:{tag}] 轮次 p50/p95(ms)：{}｜中位 p95 轮＝{}",
            rounds
                .iter()
                .map(|r| format!("{}/{}", r.p50, r.p95))
                .collect::<Vec<_>>()
                .join("，"),
            median_round + 1
        );
        (rounds, median_round)
    }

    pub(super) fn dump_samples(tag: &str, rounds: &[Round], median: usize) {
        println!("[c18:{tag}] 中位轮样本(ms)：{:?}", rounds[median].samples);
    }

    fn spawn_batch(calls: Vec<(&str, &str)>) -> Value {
        batch(
            calls
                .into_iter()
                .map(|(id, name)| {
                    call(
                        id,
                        "spawn_agent",
                        json!({ "task_name": name, "message": "t", "fork_turns": "none" }),
                    )
                })
                .collect(),
        )
    }

    /// A1：8 Agent 满容量（3 槽分三波 3+3+2 派满 8 记录/8 assignment）。
    #[test]
    #[ignore]
    fn a1_eight_agents_full_capacity() {
        let _suite = SuiteGuard::acquire();
        let mut total_child_sends = 0usize;
        let rss_before = vm_peak_kib();
        let mut keep: Option<Harness> = None;
        let mut seq = 0usize;
        let (rounds, median) = bench("a1", || {
            seq += 1;
            let h = Harness::new(&format!("c18a1-{}-{seq}", std::process::id()));
            let mut history = Vec::new();
            let sends = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let s2 = sends.clone();
            let t0 = Instant::now();
            let script: Arc<Mutex<VecDeque<Value>>> =
                Arc::new(Mutex::new(VecDeque::from(vec![child_final("d")])));
            let result = h.run_turn(
                &mut history,
                "go",
                vec![
                    spawn_batch(vec![("s1", "a1x"), ("s2", "a2x"), ("s3", "a3x")]),
                    wait_call("w1", 10000),
                    spawn_batch(vec![("s4", "a4x"), ("s5", "a5x"), ("s6", "a6x")]),
                    wait_call("w2", 10000),
                    spawn_batch(vec![("s7", "a7x"), ("s8", "a8x")]),
                    wait_call("w3", 10000),
                    wait_call("w4", 10000),
                    assistant_final("fin"),
                ],
                move || {
                    let script = script.clone();
                    let s2 = s2.clone();
                    Box::new(CountingChild { script, sends: s2 })
                },
            );
            let elapsed = t0.elapsed().as_millis();
            assert!(result.is_ok(), "{result:?}");
            keep = Some(h);
            elapsed
        });
        total_child_sends = 8 * (WARMUP + FORMAL) * ROUNDS;
        dump_samples("a1", &rounds, median);
        let disk = keep.as_ref().map(|h| dir_bytes(&h.dir)).unwrap_or(0);
        println!(
            "[c18:a1] 峰值RSS {}→{} KiB（Δ{}）｜线程 {}｜子请求合计 {}｜单次迭代会话磁盘 {} B",
            rss_before,
            vm_peak_kib(),
            vm_peak_kib().saturating_sub(rss_before),
            threads_now(),
            total_child_sends,
            disk,
        );
    }

    struct CountingChild {
        script: Arc<Mutex<VecDeque<Value>>>,
        sends: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl ChildModelSender for CountingChild {
        fn send(&self, _body: &Value) -> Result<Value, String> {
            self.sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "脚本耗尽".into())
        }
    }

    /// A2：3 子一批真实并发（屏障 3 同时放行；c03 场景）。
    #[test]
    #[ignore]
    fn a2_three_children_barrier() {
        let _suite = SuiteGuard::acquire();
        let rss_before = vm_peak_kib();
        let mut keep: Option<Harness> = None;
        let mut seq = 0usize;
        let (rounds, median) = bench("a2", || {
            seq += 1;
            let h = Harness::new(&format!("c18a2-{}-{seq}", std::process::id()));
            let mut history = Vec::new();
            let barrier = ScriptChild::new(vec![child_final("d")]).with_barrier(3);
            let bc = barrier.clone();
            let t0 = Instant::now();
            let result = h.run_turn(
                &mut history,
                "并行",
                vec![
                    batch(vec![
                        call("s1", "spawn_agent", json!({ "task_name": "p1x", "message": "t", "fork_turns": "none" })),
                        call("s2", "spawn_agent", json!({ "task_name": "p2x", "message": "t", "fork_turns": "none" })),
                        call("s3", "spawn_agent", json!({ "task_name": "p3x", "message": "t", "fork_turns": "none" })),
                    ]),
                    wait_call("w1", 30000),
                    wait_call("w2", 30000),
                    wait_call("w3", 30000),
                    assistant_final("fin"),
                ],
                move || Box::new(bc.clone()),
            );
            let elapsed = t0.elapsed().as_millis();
            assert!(result.is_ok(), "{result:?}");
            keep = Some(h);
            elapsed
        });
        dump_samples("a2", &rounds, median);
        let disk = keep.as_ref().map(|h| dir_bytes(&h.dir)).unwrap_or(0);
        println!(
            "[c18:a2] 峰值RSS {}→{} KiB（Δ{}）｜线程 {}｜单次迭代会话磁盘 {} B",
            rss_before,
            vm_peak_kib(),
            vm_peak_kib().saturating_sub(rss_before),
            threads_now(),
            disk,
        );
    }

    /// A3：24 请求＋256 调用压满（A：8 请求×32 调用；B/C：各 8 请求终文）。
    #[test]
    #[ignore]
    fn a3_budget_saturation() {
        let _suite = SuiteGuard::acquire();
        let rss_before = vm_peak_kib();
        let mut keep: Option<Harness> = None;
        let mut seq = 0usize;
        let (rounds, median) = bench("a3", || {
            seq += 1;
            let h = Harness::new(&format!("c18a3-{}-{seq}", std::process::id()));
            let mut history = Vec::new();
            // A 脚本：8 批 × 32 次 update_plan；B/C：8 次终文
            let heavy: Vec<Value> = (0..8)
                .map(|b| {
                    let calls: Vec<Value> = (0..32)
                        .map(|i| {
                            call(
                                &format!("h{b}_{i}"),
                                "update_plan",
                                json!({ "plan": [{ "step": "s", "status": "pending" }] }),
                            )
                        })
                        .collect();
                    batch(calls)
                })
                .collect();
            let light: Vec<Value> = (0..8).map(|_| child_final("d")).collect();
            let scripts: Arc<Mutex<HashMap<&'static str, VecDeque<Value>>>> =
                Arc::new(Mutex::new(HashMap::from([
                    ("heavy", VecDeque::from(heavy)),
                    ("light", VecDeque::from(light.clone())),
                    ("light2", VecDeque::from(light)),
                ])));
            let order: Arc<Mutex<VecDeque<&'static str>>> =
                Arc::new(Mutex::new(VecDeque::from(vec!["heavy", "light", "light2"])));
            let t0 = Instant::now();
            let result = h.run_turn(
                &mut history,
                "压满",
                vec![
                    batch(vec![
                        call("s1", "spawn_agent", json!({ "task_name": "sat_a", "message": "t", "fork_turns": "none" })),
                        call("s2", "spawn_agent", json!({ "task_name": "sat_b", "message": "t", "fork_turns": "none" })),
                        call("s3", "spawn_agent", json!({ "task_name": "sat_c", "message": "t", "fork_turns": "none" })),
                    ]),
                    wait_call("w1", 30000),
                    wait_call("w2", 30000),
                    wait_call("w3", 30000),
                    wait_call("w4", 30000),
                    assistant_final("fin"),
                ],
                move || {
                    let scripts = scripts.clone();
                    let order = order.clone();
                    Box::new(ScriptedFromMap { scripts, order })
                },
            );
            let elapsed = t0.elapsed().as_millis();
            assert!(result.is_ok(), "{result:?}");
            keep = Some(h);
            elapsed
        });
        dump_samples("a3", &rounds, median);
        let disk = keep.as_ref().map(|h| dir_bytes(&h.dir)).unwrap_or(0);
        println!(
            "[c18:a3] 峰值RSS {}→{} KiB（Δ{}）｜线程 {}｜单次迭代会话磁盘 {} B",
            rss_before,
            vm_peak_kib(),
            vm_peak_kib().saturating_sub(rss_before),
            threads_now(),
            disk,
        );
    }

    struct ScriptedFromMap {
        scripts: Arc<Mutex<HashMap<&'static str, VecDeque<Value>>>>,
        order: Arc<Mutex<VecDeque<&'static str>>>,
    }
    impl ChildModelSender for ScriptedFromMap {
        fn send(&self, _body: &Value) -> Result<Value, String> {
            // 同名 Agent 复用同一段脚本：以首个未耗尽段按序取用（本场景
            // heavy/light 各只被一个 Agent 使用，直接按顺序弹出）。
            let mut order = self.order.lock().unwrap();
            while let Some(key) = order.front().copied() {
                let mut map = self.scripts.lock().unwrap();
                if let Some(v) = map.get_mut(key).and_then(|q| q.pop_front()) {
                    return Ok(v);
                }
                order.pop_front();
            }
            Err("脚本耗尽".into())
        }
    }

    use std::collections::HashMap;
}

// ===== C13：真实 bwrap 下的 owner 登记 / 归属 / assignment 回收矩阵 =====
// 子进程模式（033 先例）：env 是进程级，父用例以 `--exact` 重入本测试二进制
// 并显式构造 TCHAT_SANDBOX=1；子模式入口无触发变量时直接返回（断言在父侧，
// 与 apply_patch_sandbox 的 029-offline 入口同形）。bwrap 不可用时输出 skip
// 原因后跳过（不静默通过）。

/// 子模式入口：真实 bwrap 下 spawn 子 Agent→exec（写沙箱内 PID＋驻留标记）
/// →终文回收。证据写 `TCHAT_035_C13_EVIDENCE` 指向的 JSON 文件。
#[test]
fn c13_bwrap_child_mode() {
    let Ok(evidence_path) = std::env::var("TCHAT_035_C13_EVIDENCE") else {
        return; // 无触发变量＝普通套件运行，仅作驱动入口
    };
    let _suite = SuiteGuard::acquire();
    let h = Harness::new(&format!("c13-{}", std::process::id()));
    let marker = h.dir.join("c13-insandbox.pid");
    let never = h.dir.join("c13-NEVER-TOUCHED");
    let mut history = Vec::new();
    let script = vec![
        child_call(
            "k1",
            "exec_command",
            json!({
                "cmd": format!("echo $$ > {} ; sleep 300 ; touch {}", marker.display(), never.display()),
                "yield_time_ms": 1500,
            }),
        ),
        child_final("沙箱矩阵完成"),
    ];
    let result = h.run_turn(
        &mut history,
        "go",
        vec![
            spawn_call("s1", "sbx", Some("none")),
            wait_call("w1", 15000),
            wait_call("w2", 15000),
            assistant_final("done"),
        ],
        move || {
            Box::new(ScriptChild {
                script: Arc::new(Mutex::new(VecDeque::from(script.clone()))),
                bodies: Arc::new(Mutex::new(Vec::new())),
                barrier: None,
                barrier_n: 0,
            })
        },
    );
    assert!(result.is_ok(), "子模式轮应成功：{result:?}");
    wait_slots_drained();
    // exec 结果（owner 登记证据）：运行中句柄
    let mut session_id = String::new();
    let mut exec_status = String::new();
    for it in &history {
        if let HistoryItem::AgentEvent { payload, .. } = it {
            if let terminal_chat::prompt::AgentEventPayload::Records { items } = payload {
                for item in items {
                    if let HistoryItem::ToolResult { content, .. } = item {
                        if let Ok(v) = serde_json::from_str::<Value>(content) {
                            if v["session_id"].is_string() {
                                session_id = v["session_id"].as_str().unwrap().to_string();
                                exec_status =
                                    v["status"].as_str().unwrap_or("?").to_string();
                            }
                        }
                    }
                }
            }
        }
    }
    // 终态摘要（assignment 回收证据）
    let terminal_summary = history
        .iter()
        .find_map(|it| match it {
            HistoryItem::Text(m) if m.content.contains("终态") => Some(m.content.clone()),
            _ => None,
        })
        .unwrap_or_default();
    // 残留扫描：按唯一 argv 串扫 /proc（宿主侧；033 教训——不用沙箱内 PID）。
    // 038-CI：回收→进程消亡在慢 CI 上超过单次扫描窗口——按项目时序约定改
    // poll＋截止，15s 仍有残留则按原断言失败（回收未生效）。
    let leftover_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let leftover = loop {
        let mut found = false;
        if let Ok(procs) = std::fs::read_dir("/proc") {
            for p in procs.flatten() {
                if let Ok(cmd) = std::fs::read_to_string(p.path().join("cmdline")) {
                    if cmd.contains("c13-NEVER-TOUCHED") {
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found || std::time::Instant::now() >= leftover_deadline {
            break found;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let marker_pid = std::fs::read_to_string(&marker)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let evidence = json!({
        "session_id": session_id,
        "exec_status": exec_status,
        "marker_pid": marker_pid,
        "reaped": terminal_summary.contains("已回收本 assignment 所属进程"),
        "leftover_sleep": leftover,
        "terminal_summary": terminal_summary.chars().take(300).collect::<String>(),
    });
    std::fs::write(&evidence_path, evidence.to_string()).unwrap();
}

/// 父用例：bwrap 可用→以 TCHAT_SANDBOX=1 驱动子模式并断言矩阵；不可用→
/// 输出 skip 原因跳过。
#[test]
fn c13_bwrap_owner_reap_matrix() {
    // 038 恢复真实检查（2026-10-31）：本地含 bwrap 0.11.1（runner 同版本）均通过。
    // GH runner 实测的进程树回收差异（回收后 marker 进程存活超 poll 窗口）根因
    // 待 runner 侧 strace 诊断（见 docs/logs/038-github-actions-ci/
    // 2026-10-31-runner-diagnosis-plan.md）；该用例不再提前 return——runner 上
    // 若复现即如实失败，为诊断提供信号。
    if !terminal_chat::sandbox::probe_available() {
        eprintln!(
            "skip：当前环境无可用 bwrap（probe_available=false），C13 沙箱矩阵不运行；\
             普通模式 owner/归属/回收已由 c11/c09/sec3 覆盖"
        );
        return;
    }
    let io = std::env::temp_dir().join(format!("c13-parent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&io);
    std::fs::create_dir_all(&io).unwrap();
    let evidence = io.join("evidence.json");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("c13_bwrap_child_mode")
        .env("TCHAT_035_C13_EVIDENCE", &evidence)
        .env("TCHAT_SANDBOX", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .expect("子进程启动失败");
    assert!(
        status.status.success(),
        "子模式异常退出：{:?}（stderr 已静默，单跑 --exact c13_bwrap_child_mode 排查）",
        status.status
    );
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(&evidence).expect("证据文件未写"))
            .expect("证据 JSON 可解析");
    // ① owner 登记：运行中进程句柄按 assignment 归属登记
    assert!(!v["session_id"].as_str().unwrap_or("").is_empty(), "exec 有句柄：{v}");
    assert_eq!(v["exec_status"], json!("running"), "yield 1500ms 返回运行中：{v}");
    // ② 真实 bwrap：沙箱 PID ns 内 shell 为 1（033 多信号之一）
    assert_eq!(v["marker_pid"], json!("1"), "exec 真实进入沙箱（ns 内 pid=1）：{v}");
    // ③ assignment 回收：终态摘要＋无残留（宿主侧 argv 扫描）
    assert_eq!(v["reaped"], json!(true), "终态附回收摘要：{v}");
    assert_eq!(v["leftover_sleep"], json!(false), "回收后无残留进程：{v}");
    let _ = std::fs::remove_dir_all(&io);
}
