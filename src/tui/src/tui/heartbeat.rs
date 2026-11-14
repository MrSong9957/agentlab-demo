//! 038 macOS CI：心跳源（tokio Interval 的 mac 替代）。
//!
//! 取证（docs/logs/2026-11-13-mac-fixes/report.md R25 全量栈）：该 runner
//!（Anka 虚拟化）的 kqueue EVFILT 层失效——tokio driver 的 kevent 收不到
//! timer 超时事件（worker kevent 连续阻塞、FRAME_TICK=100ms 3 秒零唤醒）。
//! 心跳＝独立 `std::thread` 以 `std::thread::sleep(period)` 计时（内核
//! clocksleep，不经 kqueue），经 tokio mpsc `try_send` 唤醒主循环——send→
//! waker→主线程 CachedParkThread unparker→pthread cond signal，全链路无
//! kqueue。bounded(1)＋try_send 满则丢＝[`tokio::time::MissedTickBehavior::
//! Skip`] 等价语义。
//!
//! 仅 macOS 编译；linux/windows 保持 tokio Interval 原路径，构建零变化。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 心跳源：[`Self::tick`] 与 `tokio::time::Interval::tick` 同签名（select!
/// 分支代码不变）。
pub struct Heartbeat {
    rx: tokio::sync::mpsc::Receiver<()>,
    /// Drop 时置位；心跳线程 sleep 窗口内见到后退出。
    stop: Arc<AtomicBool>,
}

impl Heartbeat {
    pub fn new(period: Duration) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        // 线程创建失败 panic（与 ThreadedEventSource 读线程同语义）。
        std::thread::Builder::new()
            .name("tchat-heartbeat".into())
            .spawn(move || {
                while !stop_flag.load(Ordering::SeqCst) {
                    std::thread::sleep(period);
                    // 满则丢＝Skip 语义（主循环忙时错过的心跳不堆积）。
                    let _ = tx.try_send(());
                }
            })
            .expect("心跳线程创建失败");
        Self { rx, stop }
    }

    /// 等待下一个心跳（`Interval::tick` 同签名；通道闭合后永久 Pending——
    /// 防闭合后忙转，进程退出场景随 runtime 一起消亡）。
    pub async fn tick(&mut self) {
        while self.rx.recv().await.is_some() {
            // 每 tick 立即返回一次；None（发送端已退）→ 永久挂起。
            return;
        }
        std::future::pending::<()>().await;
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
