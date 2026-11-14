//! 038 macOS CI：专属读线程事件源（crossterm EventStream 的 mac 替代）。
//!
//! 取证（docs/logs/2026-11-13-mac-fixes/report.md R11–R20）：crossterm
//! EventStream 的「后台唤醒任务＋双 waker」结构在 mac 上输入延迟数十秒后
//! 泄洪——sample 线程栈证主线程 99.7% park 于 Condvar 等唤醒、一个 tokio
//! worker 满负荷自旋（调度扫描不 park）、FRAME_TICK=100ms 的 tick 罕见命中
//! ＝唤醒/计时器 starvation；`use-dev-tty` 备用事件源三轮无效＝停滞不在
//! tty 事件源，而在 EventStream 与 tokio 运行时的交互。分类探针证打字/
//! 提交/Ctrl+C 全部健康（数据滞留内核 tty 缓冲，下一个输入字节触发泄洪）。
//!
//! 替代结构：独立 `std::thread` 以 crossterm 公共 API `poll(100ms)+read()`
//! 串行采样 tty（不经 tokio 调度/计时器驱动），事件经 tokio mpsc 通道供
//! `select!` 消费——通道 send 直接唤醒等待者（mpsc 内部 unparker，与
//! EventStream 的「后台任务阻塞 poll＋waker 回注」双跳结构无关）。读线程
//! pause/退出经原子标志＋rx drop 双保险（poll 超时窗口 ≤100ms 内生效）。
//!
//! 与 EventStream 的语义对照：
//! - 事件源/解析/事件类型/顺序：同一 crossterm 全局 reader＋同一解析器
//!   （公共 API `poll`/`read` 与 EventStream 内部同路径）——逐条等价；
//! - EOF/错误：read/poll 错误以 `Err` 入通道，消费方既有 `Some(Err(_))`
//!   路径承接（与 EventStream 一致）；
//! - Drop：stop 标志＋rx drop 双保险，线程 ≤100ms 内退出（EventStream 的
//!   shutdown 标志＋waker 同形态）；
//! - pause/resume（EventBroker drop/recreate）：桥被 drop＝线程退出＝不再
//!   读 stdin（放手语义一致）；resume 新建桥＝惰性重建（crossterm 内部
//!   reader 为全局单例，重建零成本）。
//!
//! 仅 macOS 编译（cfg(target_os = "macos")）：linux/windows 保持 EventStream
//! 原路径，编译产物零变化。

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use crossterm::event::Event;
use futures::stream::Stream;

use super::event_stream::EventResult;

/// 专属读线程事件源：实现 [`Stream`]（`Item = io::Result<Event>`，
/// 与 `crossterm::event::EventStream` 同 trait 同 Item——消费方 `select!` 代码
/// 不变）。
pub struct ThreadedEventSource {
    rx: tokio::sync::mpsc::UnboundedReceiver<EventResult>,
    /// Drop 时置位；读线程 poll(100ms) 窗口内见到后退出。
    stop: Arc<AtomicBool>,
}

impl ThreadedEventSource {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<EventResult>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        // 线程创建失败 panic（与 EventStream 后台任务 spawn 失败同语义）。
        std::thread::Builder::new()
            .name("tchat-tty-events".into())
            .spawn(move || {
                // [r33-trace]（取证，暂驻）：读层挂点直证
                super::mac_trace::trace("bridge thread enter");
                while !stop_flag.load(Ordering::SeqCst) {
                    match crossterm::event::poll(Duration::from_millis(100)) {
                        // 就绪：read 取事件（阻塞态但 poll 已确认就绪，
                        // 无界等待面与 EventStream 的 poll_internal(None) 同）。
                        Ok(true) => {
                            super::mac_trace::trace_poll(true);
                            match crossterm::event::read() {
                                Ok(ev) => {
                                    super::mac_trace::trace(&format!(
                                        "bridge read {:?}",
                                        std::mem::discriminant(&ev)
                                    ));
                                    if tx.send(Ok(ev)).is_err() {
                                        // 消费方已 drop：退出（EventStream 的
                                        // shutdown 标志同形态）。
                                        super::mac_trace::trace("bridge send failed, exit");
                                        break;
                                    }
                                }
                                Err(e) => {
                                    super::mac_trace::trace(&format!("bridge read err {e}"));
                                    let _ = tx.send(Err(e));
                                    break;
                                }
                            }
                        }
                        // 空闲窗口：让出调度（pause/stop 在此生效）。
                        Ok(false) => {
                            super::mac_trace::trace_poll(false);
                            // [r33-trace]：每 50 次空闲采样一次内核缓冲量
                            // （100ms×50＝5s 一次；停滞期可见字节进出）。
                            let n = super::mac_trace::poll_false_count();
                            if n % 50 == 0 {
                                super::mac_trace::trace_stdin_bytes();
                            }
                        }
                        Err(e) => {
                            super::mac_trace::trace(&format!("bridge poll err {e}"));
                            let _ = tx.send(Err(e));
                            break;
                        }
                    }
                }
                super::mac_trace::trace("bridge thread exit");
            })
            .expect("tty 事件读线程创建失败");
        Self { rx, stop }
    }
}

impl Default for ThreadedEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ThreadedEventSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // rx 随 self drop：读线程 send 失败同样促退（双保险）。
    }
}

impl Stream for ThreadedEventSource {
    type Item = EventResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<EventResult>> {
        // 通道闭合（读线程退出）→ Ready(None)：与 EventStream 关闭语义一致。
        Pin::new(&mut self.rx).poll_recv(cx)
    }
}
