//! 共享 PTY 基础设施：真实伪终端会话驱动 CLI 子进程（交互分支、y 确认流）。
//! 同 e2e_acceptance 的 PtySession 模式，增加 consumed 游标（wait 只在新输出
//! 中匹配，避免旧串重配导致发送错拍）。
//!
//! 008-ADAPT：交互分支进入全屏 TUI（ratatui 差分渲染）。差分按 cell 输出、跳过
//! 未变化空格并插入光标跳转，含空格/多字宽字符的文本在原始字节流中不连续，
//! raw 子串等待不可靠。新增 `wait_screen`：把累计字节增量喂给 VT100 屏幕解析器
//! （tchat_tui::test_backend::VT100Backend，与主程序同依赖树），在解析后的屏幕
//! 文本上等待子串；发送行改以 CR（\r）结尾——TUI raw mode 下 Enter＝CR，且
//! TUI 之前的 rustyline 授权提问对 CR 同样按 Enter 处理。

use std::fs;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tchat_tui::test_backend::VT100Backend;

/// PTY 窗口尺寸（与 spawn_inner 的 TIOCSWINSZ 保持一致；VT100 解析器同尺寸）。
pub(crate) const PTY_COLS: u16 = 200;
pub(crate) const PTY_ROWS: u16 = 50;

pub struct PtyCli {
    master: fs::File,
    pub child: Child,
    captured: Vec<u8>,
    /// 已被 wait 消费到的位置：后续等待只在新输出中匹配。
    consumed: usize,
    /// 008-ADAPT：VT100 屏幕解析器（wait_screen 用）；vt_fed 为已喂入字节数。
    vt: VT100Backend,
    vt_fed: usize,
    watchdog: Duration,
}

impl PtyCli {
    /// 宽窗口：长提示（含 origin 地址）不换行，保证子串断言稳定。
    fn spawn_inner(mut cmd: Command, watchdog: Duration) -> PtyCli {
        unsafe {
            let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master_fd >= 0, "posix_openpt 失败：{}", std::io::Error::last_os_error());
            assert_eq!(libc::grantpt(master_fd), 0, "grantpt 失败");
            assert_eq!(libc::unlockpt(master_fd), 0, "unlockpt 失败");
            let mut name = [0u8; 256];
            assert_eq!(
                libc::ptsname_r(master_fd, name.as_mut_ptr().cast(), name.len()),
                0,
                "ptsname_r 失败"
            );
            let len = name.iter().position(|&b| b == 0).unwrap();
            let slave = std::ffi::OsString::from_vec(name[..len].to_vec());
            let c_path = std::ffi::CString::new(slave.as_encoded_bytes()).unwrap();
            let fd0 = libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(fd0 >= 0, "打开 PTY 从设备失败：{}", std::io::Error::last_os_error());
            let fd1 = libc::dup(fd0);
            let fd2 = libc::dup(fd0);
            // 008-ADAPT：窗口尺寸提为常量（VT100 解析器与 TIOCSWINSZ 同尺寸）
            let ws = libc::winsize { ws_row: PTY_ROWS, ws_col: PTY_COLS, ws_xpixel: 0, ws_ypixel: 0 };
            libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
            // 成为会话组长并把 PTY 挂为控制终端：终端产生的 SIGINT 才能送达前台进程组
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
            cmd.stdin(Stdio::from(fs::File::from_raw_fd(fd0)));
            cmd.stdout(Stdio::from(fs::File::from_raw_fd(fd1)));
            cmd.stderr(Stdio::from(fs::File::from_raw_fd(fd2)));
            let child = cmd.spawn().expect("PTY CLI 子进程启动失败");
            PtyCli {
                master: fs::File::from_raw_fd(master_fd),
                child,
                captured: Vec::new(),
                consumed: 0,
                vt: VT100Backend::new(PTY_COLS, PTY_ROWS), // 008-ADAPT：屏幕级等待
                vt_fed: 0,
                watchdog,
            }
        }
    }

    pub fn spawn(cmd: Command) -> PtyCli {
        Self::spawn_inner(cmd, Duration::from_secs(30))
    }

    pub fn spawn_with_watchdog(cmd: Command, watchdog: Duration) -> PtyCli {
        Self::spawn_inner(cmd, watchdog)
    }

    /// 轮询 master 直到新输出包含 needle；超时报告子进程状态后报错。
    /// 008-ADAPT：TUI 渲染路径下仅适合无空格且样式稳定的 raw 标记（横幅、
    /// rustyline 授权提问、退出后的“再见!”）；TUI 屏幕内容用 wait_screen。
    pub fn wait(&mut self, needle: &str) {
        let target = needle.as_bytes();
        let deadline = Instant::now() + self.watchdog;
        loop {
            if let Some(pos) = self.captured[self.consumed..]
                .windows(target.len().max(1))
                .position(|w| w == target)
            {
                self.consumed += pos + target.len();
                return;
            }
            if Instant::now() > deadline {
                let pid = self.child.id() as i32;
                let state = fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|t| {
                    t.rsplit_once(')').map(|(_, a)| a.trim_start().chars().next().unwrap_or('?'))
                });
                match self.child.try_wait() {
                    Ok(Some(st)) => panic!("等待 {:?} 时子进程已退出：{st}", needle),
                    _ => panic!(
                        "等待 {:?} 超时（子进程状态 {:?}）：\n{}",
                        needle,
                        state,
                        String::from_utf8_lossy(&self.captured)
                    ),
                }
            }
            self.poll_read();
        }
    }

    /// 轮询 master 直到 VT100 解析后的屏幕文本包含 needle；超时报告屏幕与原始
    /// 输出后报错。008-ADAPT：TUI 差分渲染使 raw 子串不可靠，屏幕级为标准等待。
    pub fn wait_screen(&mut self, needle: &str) {
        self.wait_screen_timeout(needle, self.watchdog);
    }

    /// 同 wait_screen，但允许自定义截止（如取消 3 秒 SLA）。
    pub fn wait_screen_timeout(&mut self, needle: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            self.feed_screen();
            if self.vt.to_string().contains(needle) {
                return;
            }
            if Instant::now() > deadline {
                let pid = self.child.id() as i32;
                let state = fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|t| {
                    t.rsplit_once(')').map(|(_, a)| a.trim_start().chars().next().unwrap_or('?'))
                });
                match self.child.try_wait() {
                    Ok(Some(st)) => panic!(
                        "等待屏幕 {:?} 时子进程已退出（{st}）。屏幕：\n{}\n原始输出：\n{}",
                        needle,
                        self.vt.to_string(),
                        String::from_utf8_lossy(&self.captured)
                    ),
                    _ => panic!(
                        "等待屏幕 {:?} 超时（子进程状态 {:?}）。屏幕：\n{}\n原始输出：\n{}",
                        needle,
                        state,
                        self.vt.to_string(),
                        String::from_utf8_lossy(&self.captured)
                    ),
                }
            }
            self.poll_read();
        }
    }

    /// 把尚未喂入的字节增量送进 VT100 解析器（按块喂入安全：解析器缓冲跨块的
    /// 不完整转义/UTF-8 序列）。
    fn feed_screen(&mut self) {
        let end = self.captured.len();
        if self.vt_fed < end {
            let _ = self.vt.write_all(&self.captured[self.vt_fed..end]);
            self.vt_fed = end;
        }
    }

    /// 非阻塞读一次 master（50ms poll），有数据则并入 captured。
    fn poll_read(&mut self) {
        let mut pfd = libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, 50) };
        if r > 0 && pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let mut buf = [0u8; 4096];
            let n =
                unsafe { libc::read(self.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                self.captured.extend_from_slice(&buf[..n as usize]);
            }
        }
    }

    /// 发送一行：先写文本、隔 150ms 再写 CR。008-ADAPT：
    /// - TUI raw mode 下 Enter＝CR（\n 不提交）；TUI 之前的 rustyline 授权提问
    ///   （普通终端，ICRNL）对 CR 同样按行结束；
    /// - TUI composer 的 paste-burst 启发式会把一次连打的 ≥3 个 ASCII 字符视为
    ///   粘贴突发，紧跟的 Enter 被并入草稿而非提交（PASTE_ENTER_SUPPRESS_WINDOW
    ///   ＝120ms）——分隔发送并等窗口过期，模拟真人键入节奏。
    pub fn send(&mut self, line: &str) {
        std::thread::sleep(Duration::from_millis(50));
        self.master.write_all(line.as_bytes()).expect("PTY 写入失败");
        std::thread::sleep(Duration::from_millis(150));
        self.master.write_all(b"\r").expect("PTY 写入失败");
    }

    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.captured).into_owned()
    }

    /// 结束会话：限期等待退出，返回（输出，退出码）。
    pub fn finish(mut self) -> (String, Option<i32>) {
        let deadline = Instant::now() + self.watchdog;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("PTY CLI 子进程未在期限内退出");
                }
            }
        };
        (self.output(), status.code())
    }
}

impl Drop for PtyCli {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
