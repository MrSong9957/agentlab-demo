//! M0 原型（方案 B）：JS prelude 装配 Promise，Rust 桥接只收发基本类型。
//! 验证：顶层 await、宿主异步兑现、微任务泵、未处理拒绝、内存/栈/中断限制。
use rquickjs::{promise::PromiseState, AsyncContext, AsyncRuntime, Function, Object, Promise};
use rquickjs::Persistent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// worker 侧注入的 prelude：桥接面（tools 经 __hostCall）、text() 输出。
const PRELUDE: &str = r#"
globalThis.__pending = new Map();
globalThis.__nextId = 1;
globalThis.__hostCall = function(payload) {
    return new Promise((resolve, reject) => {
        const id = __nextId++;
        __pending.set(id, { resolve, reject });
        __dispatch(id, payload);
    });
};
globalThis.__settle = function(id, ok, value) {
    const e = __pending.get(id);
    if (!e) return false;
    __pending.delete(id);
    if (ok) e.resolve(value); else e.reject(new Error(value));
    return true;
};
globalThis.text = function(v) {
    if (typeof v === 'string') { __text(v, false); }
    else { __text(JSON.stringify(v), true); }
};
"#;

struct PendingSink {
    settle: Persistent<Function<'static>>,
}

async fn run(mode: &str) -> Result<String, String> {
    let rt = AsyncRuntime::new().map_err(|e| format!("{e:?}"))?;
    rt.set_memory_limit(64 * 1024 * 1024).await;
    rt.set_max_stack_size(512 * 1024).await;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let deadline0 = Instant::now() + Duration::from_secs(6);
        rt.set_interrupt_handler(Some(Box::new(move || {
            stop.load(Ordering::SeqCst) || Instant::now() > deadline0
        })))
        .await;
    }
    // 未处理拒绝账本：ptr 身份 → 消息；handled=true（获得 handler）时移除。
    let unhandled: Arc<Mutex<HashMap<usize, String>>> = Arc::new(Mutex::new(HashMap::new()));
    {
        let un = unhandled.clone();
        rt.set_host_promise_rejection_tracker(Some(Box::new(
            move |_ctx: rquickjs::Ctx<'_>, p: rquickjs::Value<'_>, reason: rquickjs::Value<'_>, handled: bool| {
                let ptr = unsafe { rquickjs::qjs::JS_VALUE_GET_PTR(p.as_raw()) } as usize;
                if handled {
                    un.lock().unwrap().remove(&ptr);
                } else {
                    let text = reason
                        .get::<Object>()
                        .and_then(|o| o.get::<_, String>("message"))
                        .unwrap_or_else(|_| "?".into());
                    un.lock().unwrap().insert(ptr, text);
                }
            },
        )))
        .await;
    }
    let ctx = AsyncContext::full(&rt).await.map_err(|e| format!("ctx: {e:?}"))?;

    let (req_tx, req_rx): (Sender<(u64, String)>, Receiver<(u64, String)>) = channel();
    let texts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 待兑现表由 JS 侧 __pending Map 持有；宿主只记 id。
    let inflight: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));

    let code: &str = match mode {
        "bridge" => r#"
            globalThis.__out = [];
            const a = await __hostCall(JSON.stringify({cmd:"echo", v:"hello"}));
            __out.push(JSON.parse(a).v);
            const b = await __hostCall(JSON.stringify({cmd:"echo", v:"world"}));
            __out.push(JSON.parse(b).v);
            const p = __hostCall(JSON.stringify({cmd:"echo", v:"race"})).then(r => __out.push(JSON.parse(r).v));
            await Promise.all([p]);
            text("输出一");
            text({n: 2});
            __out.join("|")
        "#,
        "reject" => r#"
            Promise.reject(new Error("boom-unhandled"));
            await Promise.reject(new Error("handled")).catch(e => "caught:" + e.message);
            "done"
        "#,
        "loop" => "while(true){}",
        "oom" => &format!("let a = [{}]; while(true){{ a = a.concat(a); }}", "1,".repeat(64)),
        "stack" => "(function f(){ return f(); })()",
        _ => return Err("unknown mode".into()),
    };

    // 装配 prelude + 用户代码：__dispatch/__text 是基本类型闭包（无生命周期问题）。
    let module_promise: Persistent<Promise<'static>> = ctx
        .with(|ctx| -> Result<Persistent<Promise<'static>>, rquickjs::Error> {
            let req_tx = req_tx.clone();
            let inflight = inflight.clone();
            ctx.globals().set(
                "__dispatch",
                Function::new(
                    ctx.clone(),
                    move |id: f64, payload: String| -> bool {
                        inflight.lock().unwrap().push(id as u64);
                        req_tx.send((id as u64, payload)).is_ok()
                    },
                )?,
            )?;
            let texts = texts.clone();
            ctx.globals().set(
                "__text",
                Function::new(
                    ctx.clone(),
                    move |line: String, _json: bool| -> bool {
                        texts.lock().unwrap().push(line);
                        true
                    },
                )?,
            )?;
            ctx.eval::<(), _>(PRELUDE)?;
            let promise = ctx.eval_promise(code)?;
            Ok(Persistent::save(&ctx, promise))
        })
        .await
        .map_err(|e| format!("eval 启动失败: {e:?}"))?;

    let sink: PendingSink = ctx
        .with(|ctx| -> Result<PendingSink, rquickjs::Error> {
            let f: Function = ctx.globals().get::<_, Function>("__settle")?;
            Ok(PendingSink { settle: Persistent::save(&ctx, f) })
        })
        .await
        .map_err(|e| format!("sink: {e:?}"))?;

    async fn settle(ctx: &AsyncContext, sink: &PendingSink, id: u64, result: Result<String, String>) -> Result<(), String> {
        ctx.with(|c| -> Result<(), String> {
            let f = sink.settle.clone().restore(&c).map_err(|e| format!("{e:?}"))?;
            let (ok, v) = match result {
                Ok(v) => (true, v),
                Err(e) => (false, e),
            };
            f.call::<_, bool>((id as f64, ok, v))
                .map(|_| ())
                .map_err(|e| format!("settle: {e:?}"))
        })
        .await
        .map_err(|e: String| e)
    }

    let deadline = Instant::now() + Duration::from_secs(6);
    let result = loop {
        if Instant::now() > deadline {
            stop.store(true, Ordering::SeqCst);
            rt.idle().await;
            break Err("事件循环超时".into());
        }
        while rt.execute_pending_job().await.map_err(|e| format!("job: {e:?}"))? {}
        let state = ctx
            .with(|c| {
                module_promise
                    .clone()
                    .restore(&c)
                    .map(|p| p.state())
                    .map_err(|e| format!("{e:?}"))
            })
            .await
            .map_err(|e: String| e)?;
        if state != PromiseState::Pending {
            let out = ctx
                .with(|c| {
                    module_promise
                        .clone()
                        .restore(&c)
                        .and_then(|p| p.finish::<rquickjs::Value>())
                        .map_err(|e| format!("{e:?}"))
                        .and_then(|v| {
                            let t = format!("{:?}", v.type_of());
                            let s: Result<String, _> = v.get();
                            Ok(format!("{t}:{}", s.unwrap_or_default()))
                        })
                })
                .await;
            break out;
        }
        // 模块挂起：处理宿主请求（模拟主进程执行工具后回包）
        let mut got = false;
        while let Ok((id, payload)) = req_rx.try_recv() {
            got = true;
            // 模拟一次真实工具：回显 payload 的 v 字段
            // 原型：payload 形如 {"cmd":"echo","v":"..."}——手写取 v 值
            let v = payload
                .split("\"v\":\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap_or("?");
            let reply = format!("{{\"v\":\"echo:{v}\"}}");
            settle(&ctx, &sink, id, Ok(reply)).await?;
            inflight.lock().unwrap().retain(|&x| x != id);
        }
        if !got {
            // 没有可推进事件：防忙转（真实场景此处阻塞读 IPC）
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    // 收尾：泵完剩余微任务，收集未处理拒绝
    let _ = rt.execute_pending_job().await;
    let un: Vec<String> = unhandled
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    let texts_out = texts.lock().unwrap().clone();
    result.map(|v| format!("{v} || texts={texts_out:?} || unhandled={un:?}"))
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "bridge".into());
    let started = Instant::now();
    match futures::executor::block_on(run(&mode)) {
        Ok(v) => println!("[{mode}] OK ({:.0?}): {}", started.elapsed(), &v[..v.len().min(300)]),
        Err(e) => println!("[{mode}] ERR ({:.0?}): {e}", started.elapsed()),
    }
}
