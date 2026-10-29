# M0 原型证据（2026-10-29）

命令（隔离目录 /tmp/m034/proto，专用 --target-dir /tmp/m034/target）：
```
cargo fetch && cargo build --target-dir /tmp/m034/target
for m in bridge reject loop oom stack; do timeout 30 /tmp/m034/target/debug/m034-proto $m; done
```

实测输出（2026-10-29，容器 /workspace 环境，gcc 13.3.0）：
```
[bridge] OK (807µs): Object: || texts=["输出一", "{\"n\":2}"] || unhandled=[]
[reject] OK (610µs): Object: || texts=[] || unhandled=["boom-unhandled"]
[loop] ERR (6s): eval 启动失败: Exception
[oom] ERR (432ms): Exception
[stack] ERR (905µs): Exception
```

结论：
- rquickjs =0.14.0（default-features=false, features=["std","futures"]）可构建；22 传递依赖入 lock（见 proto-Cargo.lock）。
- 顶层 await：ctx.eval_promise（JS_EVAL_FLAG_ASYNC）。
- Promise 桥接：JS prelude 装配 __hostCall/__settle/text；Rust 闭包只收发基本类型（避开 Function::new 闭包返回引用值的生命周期限制）；宿主持 Persistent<Function> 调 __settle(id, ok, value) 兑现。
- 微任务泵：AsyncRuntime::execute_pending_job 循环；模块完成经 Persistent<Promise> 的 state()（Pending/Resolved/Rejected）。
- 未处理拒绝：set_host_promise_rejection_tracker + JS_VALUE_GET_PTR 身份账本；handled=true 时移除（reject 模式实测：被 catch 的 "handled" 移除、仅剩 "boom-unhandled"）。
- 限制实测：堆 64 MiB（oom 432ms 触发）、栈 512 KiB（stack 905µs）、中断 handler 内联 deadline 判定（loop 6s 中断；eval 同步段可达）。
- 注意事项：ctx.eval 需显式 turbofish（::<(),_>）避免 never-type fallback deny；ctx.with(...).await；PromiseState 变体名为 Resolved；eval_promise 的 resolve 值不可靠（不作为完成值来源，完成/失败只看 state）。
