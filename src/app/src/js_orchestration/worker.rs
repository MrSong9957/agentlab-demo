//! 034 worker：隐藏入口 `--internal-js` 的 JS 编排运行时（独立子进程）。
//!
//! 只通过 stdin/stdout 的有界 IPC 帧（[`super::protocol`]）与宿主通信：读
//! `init`（源码＋工具目录）→ 装配 rquickjs（无 loader/模块文件搜索，堆/栈
//! 限制与中断）→ `eval_promise` 顶层 await 求值 → 事件循环（泵微任务 ↔ 处理
//! 叶子回包/输出/shutdown）→ `done` 帧退出。
//!
//! 桥接形态（M0 原型定型）：JS prelude 装配 `tools`/`ALL_TOOLS`/`text` 与
//! Promise 结算；Rust 注入的闭包只收发基本类型（`__dispatch(id, payload)`/
//! `__text(line, json)`）并把待发帧推入共享 outbox（事件循环统一写出——闭包
//! 要求 `'js`/捕获约束下不持有 IO 引用，避免可变别名）；宿主兑现经
//! `Persistent<Function>` 调用 JS 侧 `__settle(id, ok, value)`。worker 不读
//! 环境变量、不触碰文件/网络/进程。

use super::protocol::{
    done_frame, output_frame, read_frame, write_frame, JS_HEAP_LIMIT,
    JS_STACK_LIMIT, MAX_FRAME_BYTES, MAX_INIT_FRAME_BYTES, WORKER_WALL_LIMIT_MS,
};
use rquickjs::promise::PromiseState;
use rquickjs::{AsyncContext, AsyncRuntime, Function, Promise};
use rquickjs::Persistent;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// text() 单次值的分块上限（单 IPC 帧 ≤1 MiB；JSON 转义膨胀下取半）。
const TEXT_CHUNK: usize = 512 * 1024;

/// worker 注入的 prelude：目录装配、桥接面与 text（§1.2/§1.3 契约的 JS 侧）。
/// 内部状态（pending 表/序号）封装在 IIFE 闭包内不进全局；`__settle` 为宿主
/// 兑现入口（必须可达）——以不可写/不可配置/不可枚举属性安装（审查 P2-9）；
/// 目录深冻结（含嵌套 Schema）。
const PRELUDE: &str = r#"
(() => {
    const defs = JSON.parse(globalThis.__catalogJson);
    delete globalThis.__catalogJson;
    const pending = new Map();
    let nextId = 1;
    const hostCall = (payload) => new Promise((resolve, reject) => {
        const id = nextId++;
        pending.set(id, { resolve, reject });
        __dispatch(id, payload);
    });
    // 宿主兑现入口：不可枚举/不可写/不可配置（脚本可调用但等价于自行 resolve
    // 自己的 Promise——宿主侧不信任任何自报；改变/删除将破坏自身后续兑现）。
    Object.defineProperty(globalThis, '__settle', {
        value: (id, ok, value) => {
            const e = pending.get(id);
            if (!e) return false;
            pending.delete(id);
            if (ok) e.resolve(JSON.parse(value));
            else e.reject(JSON.parse(value));
            return true;
        },
        writable: false, configurable: false, enumerable: false,
    });
    const deepFreeze = (o) => {
        if (o === null || typeof o !== 'object') return o;
        Object.freeze(o);
        for (const k of Object.keys(o)) deepFreeze(o[k]);
        return o;
    };
    const tools = Object.create(null);
    for (const d of defs) {
        Object.defineProperty(tools, d.name, {
            value: (args) => {
                if (args === null || typeof args !== 'object' || Array.isArray(args)) {
                    return Promise.reject({ code: 'INVALID_ARGS', message: 'tool arguments must be a JSON object', execution: 'not_started' });
                }
                return hostCall(JSON.stringify({ tool: d.name, args: args }));
            },
            writable: false, configurable: false, enumerable: true,
        });
    }
    Object.freeze(tools);
    Object.defineProperty(globalThis, 'tools', { value: tools, writable: false, configurable: false });
    const catalog = deepFreeze(defs.map(d => Object.freeze({ name: d.name, description: d.description, parameters: d.parameters })));
    Object.defineProperty(globalThis, 'ALL_TOOLS', { value: catalog, writable: false, configurable: false });
    const textFn = (v) => {
        if (v === undefined) throw new TypeError('text(): undefined is not outputtable');
        if (typeof v === 'string') { __text(v, false); return; }
        __text(JSON.stringify(v), true);
    };
    Object.defineProperty(globalThis, 'text', { value: textFn, writable: false, configurable: false });
})();
"#;

/// 待发帧邮箱：JS 闭包（受 `'js` 约束，只能捕获 'static 数据）写入，事件
/// 循环统一写出——闭包不持有 IO 引用，写 IO 单点化。
type Outbox = Arc<Mutex<Vec<Value>>>;

/// 未处理拒绝账本：promise 指针身份 → 消息（handled=true 时移除；收尾时非空
/// 即失败——M0 实证「被 catch 的拒绝」会在 job 泵后以 handled=true 回调移除）。
type RejectionLedger = Arc<Mutex<HashMap<usize, String>>>;

fn outbox_push(outbox: &Outbox, frame: Value) -> bool {
    let mut box_ = outbox.lock().unwrap();
    box_.push(frame);
    true
}

/// `--internal-js` 入口：stdin/stdout 二进制帧协议；不读任何环境变量。
pub fn run_internal_js() -> i32 {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    // init 帧（≤10 MiB）：源码＋工具目录。
    let init = match read_frame(&mut input, MAX_INIT_FRAME_BYTES) {
        Ok(Some(frame)) if frame.get("t").and_then(Value::as_str) == Some("init") => frame,
        Ok(_) => {
            eprintln!("js-worker: 首帧不是 init");
            return 2;
        }
        Err(e) => {
            eprintln!("js-worker: 读取 init 帧失败：{e}");
            return 2;
        }
    };
    let code = match init.get("code").and_then(Value::as_str) {
        Some(c) if !c.trim().is_empty() => c.to_string(),
        _ => {
            eprintln!("js-worker: init 帧缺少非空 code");
            return 2;
        }
    };
    let catalog: Vec<Value> = init
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let catalog_json = match serde_json::to_string(&catalog) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("js-worker: 目录序列化失败：{e}");
            return 2;
        }
    };
    let started = Instant::now();
    let deadline = started + Duration::from_millis(WORKER_WALL_LIMIT_MS);
    match futures::executor::block_on(run_module(
        &mut input,
        &mut output,
        &code,
        &catalog_json,
        deadline,
    )) {
        Ok(()) => 0,
        Err(reason) => {
            // 引擎错误/坏帧：尽力发终态帧后退出（宿主按断开处理）。
            let _ = write_frame(
                &mut output,
                &done_frame(false, Some(&reason), 0),
                MAX_FRAME_BYTES,
            );
            3
        }
    }
}

async fn run_module<R: Read, W: Write>(
    input: &mut R,
    output: &mut W,
    code: &str,
    catalog_json: &str,
    deadline: Instant,
) -> Result<(), String> {
    let rt = AsyncRuntime::new().map_err(|e| format!("引擎初始化失败：{e:?}"))?;
    rt.set_memory_limit(JS_HEAP_LIMIT).await;
    rt.set_max_stack_size(JS_STACK_LIMIT).await;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        rt.set_interrupt_handler(Some(Box::new(move || {
            stop.load(Ordering::SeqCst) || Instant::now() > deadline
        })))
        .await;
    }
    let unhandled: RejectionLedger = Arc::new(Mutex::new(HashMap::new()));
    {
        let ledger = unhandled.clone();
        rt.set_host_promise_rejection_tracker(Some(Box::new(
            move |_ctx: rquickjs::Ctx<'_>,
                  promise: rquickjs::Value<'_>,
                  reason: rquickjs::Value<'_>,
                  handled: bool| {
                // SAFETY：JS_VALUE_GET_PTR 是对 JSValue union 的只读指针提取。
                let ptr = unsafe { rquickjs::qjs::JS_VALUE_GET_PTR(promise.as_raw()) } as usize;
                if handled {
                    ledger.lock().unwrap().remove(&ptr);
                } else {
                    let text = reason
                        .get::<rquickjs::Object>()
                        .and_then(|o| o.get::<_, String>("message"))
                        .unwrap_or_else(|_| "未知拒绝原因".into());
                    ledger.lock().unwrap().insert(ptr, text);
                }
            },
        )))
        .await;
    }
    let ctx = AsyncContext::full(&rt)
        .await
        .map_err(|e| format!("上下文初始化失败：{e:?}"))?;

    let outbox: Outbox = Arc::new(Mutex::new(Vec::new()));
    let outstanding: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let settle_cell: Arc<Mutex<Option<Persistent<Function<'static>>>>> =
        Arc::new(Mutex::new(None));
    let module_cell: Arc<Mutex<Option<Persistent<Promise<'static>>>>> =
        Arc::new(Mutex::new(None));

    // 装配：__dispatch/__text（基本类型闭包→outbox）→ 目录字符串 → prelude
    // → 用户代码；settle/module 的持久引用随后取回。
    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        let outbox_for_dispatch = outbox.clone();
        let outstanding_for_dispatch = outstanding.clone();
        ctx.globals().set(
            "__dispatch",
            Function::new(
                ctx.clone(),
                move |id: f64, payload: String| -> bool {
                    let id = id as u64;
                    // payload = {tool, args}；解析后构造宿主 call 帧。
                    let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                        return false;
                    };
                    let tool = value
                        .get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let args = value.get("args").cloned().unwrap_or(Value::Null);
                    let frame = super::protocol::call_frame(id, &tool, &args);
                    let queued = outbox_push(&outbox_for_dispatch, frame);
                    if queued {
                        outstanding_for_dispatch.lock().unwrap().insert(id, tool);
                    }
                    queued
                },
            )?,
        )?;
        let outbox_for_text = outbox.clone();
        ctx.globals().set(
            "__text",
            Function::new(
                ctx.clone(),
                move |line: String, json: bool| -> bool {
                    // 按调用顺序分块（单帧 ≤1 MiB；UTF-8 边界安全切分）。
                    let mut rest = line.as_str();
                    let queued_all = true;
                    loop {
                        let (chunk, tail) = if rest.len() > TEXT_CHUNK {
                            let mut cut = TEXT_CHUNK;
                            while !rest.is_char_boundary(cut) {
                                cut -= 1;
                            }
                            (&rest[..cut], Some(&rest[cut..]))
                        } else {
                            (rest, None)
                        };
                        outbox_push(&outbox_for_text, output_frame(chunk, json));
                        match tail {
                            Some(next) => rest = next,
                            None => break,
                        }
                    }
                    queued_all
                },
            )?,
        )?;
        ctx.globals().set("__catalogJson", catalog_json)?;
        ctx.eval::<(), _>(PRELUDE)?;
        let promise = ctx.eval_promise(code)?;
        let settle: Function = ctx.globals().get::<_, Function>("__settle")?;
        *settle_cell.lock().unwrap() = Some(Persistent::save(&ctx, settle));
        *module_cell.lock().unwrap() = Some(Persistent::save(&ctx, promise));
        Ok(())
    })
    .await
    .map_err(|e: rquickjs::Error| format!("脚本启动失败：{e:?}"))?;

    // 把 outbox 中积压的帧按序写出（每轮泵后调用；调用方持有唯一 IO 写引用）。
    let flush_outbox = |output: &mut W| -> Result<(), String> {
        let pending: Vec<Value> = std::mem::take(&mut *outbox.lock().unwrap());
        for frame in pending {
            write_frame(output, &frame, MAX_FRAME_BYTES)
                .map_err(|e| format!("发送 IPC 帧失败：{e}"))?;
        }
        Ok(())
    };

    // 事件循环：泵微任务 → flush 待发帧 → 模块状态 → 宿主帧（叶子回包/shutdown）。
    loop {
        while rt.execute_pending_job().await.map_err(|e| format!("微任务执行失败：{e:?}"))? {}
        flush_outbox(output)?;
        let module_state = {
            let module = module_cell
                .lock()
                .unwrap()
                .as_ref()
                .expect("装配后必在")
                .clone();
            ctx.with(|c| {
                module
                    .restore(&c)
                    .map(|p| p.state())
                    .map_err(|e| format!("{e:?}"))
            })
            .await
            .map_err(|e: String| e)?
        };
        match module_state {
            PromiseState::Resolved | PromiseState::Rejected => {
                // 顶层终态：泵尽剩余微任务（then/catch 链与拒绝账本稳定），
                // flush 后报告；未结算调用数即 UNAWAITED_TOOLS 证据。
                for _ in 0..1024 {
                    match rt.execute_pending_job().await {
                        Ok(true) => continue,
                        Ok(false) => break,
                        Err(_) => break, // 收尾期异常不再改变终态报告
                    }
                }
                flush_outbox(output)?;
                let outstanding_count = outstanding.lock().unwrap().len();
                let unhandled_now: Vec<String> = unhandled
                    .lock()
                    .unwrap()
                    .values()
                    .cloned()
                    .collect();
                let (ok, error) = match module_state {
                    PromiseState::Resolved if unhandled_now.is_empty() => (true, None),
                    PromiseState::Resolved => (
                        false,
                        Some(format!("未处理拒绝：{}", unhandled_now.join("；"))),
                    ),
                    _ => {
                        let reason = ctx
                            .with(|c| -> String {
                                let module = module_cell
                                    .lock()
                                    .unwrap()
                                    .as_ref()
                                    .expect("装配后必在")
                                    .clone();
                                match module.restore(&c) {
                                    Ok(p) => match p.result::<String>() {
                                        Some(Ok(s)) => s,
                                        _ => {
                                            // result() 已把异常 throw 到 ctx；catch 取回。
                                            let v = c.catch();
                                            let text: Result<String, _> = v.get();
                                            text.unwrap_or_else(|_| "脚本执行失败".into())
                                        }
                                    },
                                    Err(_) => "脚本执行失败".into(),
                                }
                            })
                            .await;
                        let unhandled_note = if unhandled_now.is_empty() {
                            String::new()
                        } else {
                            format!("；未处理拒绝：{}", unhandled_now.join("；"))
                        };
                        (false, Some(format!("{reason}{unhandled_note}")))
                    }
                };
                let frame = done_frame(ok, error.as_deref(), outstanding_count);
                write_frame(output, &frame, MAX_FRAME_BYTES)
                    .map_err(|e| format!("发送终态帧失败：{e}"))?;
                return Ok(());
            }
            PromiseState::Pending => {}
        }
        // 模块挂起中：唯一能推进的事件是宿主帧（叶子回包/shutdown）。阻塞读。
        match read_frame(input, MAX_FRAME_BYTES) {
            Ok(Some(frame)) => match frame.get("t").and_then(Value::as_str) {
                Some("leaf") => {
                    let id = frame.get("id").and_then(Value::as_u64).unwrap_or(u64::MAX);
                    let ok = frame.get("ok").and_then(Value::as_bool).unwrap_or(false);
                    let payload = if ok {
                        frame.get("result").cloned().unwrap_or(Value::Null)
                    } else {
                        frame.get("error").cloned().unwrap_or(Value::Null)
                    };
                    settle_leaf(&ctx, &settle_cell, &outstanding, id, ok, &payload).await?;
                }
                Some("shutdown") => {
                    // 宿主关闭：中断 JS（下一次引擎调用抛 Interrupted），模块
                    // Promise 将以异常终结；宿主不等待本进程的终态帧。
                    stop.store(true, Ordering::SeqCst);
                    continue;
                }
                _ => return Err("收到未知帧类型：坏帧断开".into()),
            },
            Ok(None) => {
                // 宿主关闭了 stdin：cell 已被宿主收尾（终止/取消），worker 退出。
                return Ok(());
            }
            Err(e) => return Err(format!("坏帧断开：{e}")),
        }
    }
}

/// 兑现一次叶子调用：调 JS 侧 __settle(id, ok, valueJSON)。
async fn settle_leaf(
    ctx: &AsyncContext,
    settle_cell: &Arc<Mutex<Option<Persistent<Function<'static>>>>>,
    outstanding: &Arc<Mutex<HashMap<u64, String>>>,
    id: u64,
    ok: bool,
    payload: &Value,
) -> Result<(), String> {
    let settle = {
        outstanding.lock().unwrap().remove(&id);
        match settle_cell.lock().unwrap().as_ref() {
            Some(f) => f.clone(),
            None => return Ok(()), // 装配前不可能有叶子；防御性忽略
        }
    };
    let value_json = serde_json::to_string(payload).map_err(|e| format!("{e}"))?;
    ctx.with(|c| -> Result<(), String> {
        let f: Function = settle.restore(&c).map_err(|e| format!("{e:?}"))?;
        f.call::<_, bool>((id as f64, ok, value_json))
            .map(|_| ())
            .map_err(|e| format!("settle 调用失败：{e:?}"))
    })
    .await
    .map_err(|e: String| e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js_orchestration::protocol::{init_frame, leaf_frame};
    use serde_json::json;

    /// 纯内嵌冒烟：帧构造形状（不含真实进程）。
    #[test]
    fn init_frame_shape() {
        let frame = init_frame("text(1)", &[json!({"name":"exec_command","description":"d","parameters":{}})]);
        assert_eq!(frame["t"], "init");
        assert_eq!(frame["code"], "text(1)");
        assert_eq!(frame["tools"].as_array().unwrap().len(), 1);
        let leaf_ok = leaf_frame(7, Ok(json!({"status":"completed"})));
        assert_eq!(leaf_ok["ok"], json!(true));
        let leaf_err = leaf_frame(7, Err(json!({"code":"QUOTA","message":"m","execution":"unknown"})));
        assert_eq!(leaf_err["ok"], json!(false));
        assert_eq!(leaf_err["error"]["execution"], "unknown");
    }
}
