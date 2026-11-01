//! 跨平台测试命令构造助手（038 蓝图实现）。
//!
//! exec 契约＝经平台 shell 执行命令串（`proc_session::SHELL`：unix `/bin/sh -c`、
//! Windows `cmd /C`）。夹具命令一律经本模块构造，禁止散落 sh 语法字面量——
//! cmd 无法执行 `;`/`$(( ))`/`printf` 等 sh 设施（038 Windows CI 实测 219 失败
//! 的主要家族之一）。
//!
//! 行尾与编码的平台事实：cmd `echo` 输出 CRLF（unix 为 LF）；输出内容断言用
//! `normalize()` 归一或按 `nl()` 取期望值——行尾风格不属契约（契约是内容、
//! 顺序与字节级截断行为）。unicode 输出经 `chcp 65001` 固定 UTF-8 码页。
//! 生成的命令串不含双引号（Rust 对 cmd 参数的引号转义不可靠）；`(...)` 包裹
//! 的 echo 防止行尾数字被解析为句柄重定向（`9>>` 陷阱）。

use std::path::Path;

/// 平台行尾（命令输出的真实平台语义）。
pub fn nl() -> &'static str {
    if cfg!(windows) {
        "\r\n"
    } else {
        "\n"
    }
}

/// 比较用行尾归一（平台行尾差异不属契约）。
pub fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn path_str(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// 子命令序列：unix 以 `;` 连接，Windows 以 `&` 连接。
pub fn seq(parts: &[String]) -> String {
    let sep = if cfg!(windows) { " & " } else { "; " };
    parts.join(sep)
}

/// 追加一行到文件（存活信号日志）。
pub fn append_line(log: &Path, text: &str) -> String {
    let log = path_str(log);
    if cfg!(windows) {
        format!("(echo {text})>>{log}")
    } else {
        format!("echo {text} >> {log}")
    }
}

/// 无限循环向文件追加一行＋亚秒延时（取消/时限/组杀测试的存活写入者）。
/// Windows 用 `for /l` step=0 无限循环＋`ping -w` 超时提供延时（TEST-NET
/// 地址 192.0.2.1 不可达，-w 超时确定性生效，不发真实网络流量）。
pub fn writer_loop(log: &Path) -> String {
    let log = path_str(log);
    if cfg!(windows) {
        // 回环 ping（ping.exe 进程约 30-60ms）作节流：约 15-30 行/秒——足够
        // 增长窗口观测、又不至于淹没输出缓存干扰时限测量。（TEST-NET 的
        // ping -w 超时在 runner 上被立即拒绝＝无延时——038 实测）
        format!("for /l %i in (1,0,2) do @((echo x)>>{log} & ping -n 1 127.0.0.1>nul)")
    } else {
        format!("while true; do echo x >> {log}; sleep 0.05; done")
    }
}

/// 无限循环向 stdout 写计数行＋亚秒延时（stdout 存活写入者）。
pub fn writer_loop_stdout() -> String {
    if cfg!(windows) {
        "for /l %i in (1,0,2) do @(echo %i & ping -n 1 127.0.0.1>nul)".to_string()
    } else {
        "while true; do date +%s%N; sleep 0.05; done".to_string()
    }
}

/// 休眠。unix `sleep`（小数）；Windows `ping -w` 超时近似（秒级为 1000ms 超时
/// 计数，亚秒为单次超时窗口）。
pub fn sleep_ms(ms: u64) -> String {
    if cfg!(windows) {
        // 回环 ping：-n K 的请求间隔≈1 秒（回复即时到达也按间隔发送），粒度 1s
        format!("ping -n {} 127.0.0.1>nul", ms / 1000 + 1)
    } else if ms % 1000 == 0 {
        format!("sleep {}", ms / 1000)
    } else {
        format!("sleep 0.{ms:03}")
    }
}

/// 输出一行到 stdout。
pub fn echo(text: &str) -> String {
    if cfg!(windows) {
        format!("(echo {text})")
    } else {
        format!("echo {text}")
    }
}

/// 输出一行到 stderr。
pub fn echo_err(text: &str) -> String {
    format!("echo {text} 1>&2")
}

/// 输出 n 行相同文本到 stdout（数量按平台缩放见调用点注释）。
pub fn count_lines(text: &str, n: u64) -> String {
    if cfg!(windows) {
        format!("for /l %i in (1,1,{n}) do @echo {text}")
    } else {
        format!("i=0; while [ $i -lt {n} ]; do echo '{text}'; i=$((i+1)); done")
    }
}

/// 输出 n 行相同文本到 stderr。
pub fn count_lines_err(text: &str, n: u64) -> String {
    if cfg!(windows) {
        format!("for /l %i in (1,1,{n}) do @echo {text} 1>&2")
    } else {
        format!("i=0; while [ $i -lt {n} ]; do echo '{text}' 1>&2; i=$((i+1)); done")
    }
}

/// 创建空文件。
pub fn touch(path: &Path) -> String {
    let p = path_str(path);
    if cfg!(windows) {
        format!("type nul>{p}")
    } else {
        format!("touch {p}")
    }
}

/// 输出一行到文件（带平台行尾；unix echo 原语义）。
pub fn echo_to_file(path: &Path, text: &str) -> String {
    let p = path_str(path);
    if cfg!(windows) {
        format!("(echo {text})>{p}")
    } else {
        format!("echo {text} > {p}")
    }
}

/// 精确字节写文件（不追加行尾）。unix＝`printf '%s'`；Windows＝`<nul set /p`
/// 写无行尾内容（受 ≤1021 字符限制——测试规模内容；内容不得以 '=' 开头）。
pub fn write_file(path: &Path, content: &str) -> String {
    let p = path_str(path);
    if cfg!(windows) {
        format!("<nul set /p={content}>{p}")
    } else {
        format!("printf '%s' '{content}' > {p}")
    }
}

/// 输出文件内容。
pub fn cat(path: &Path) -> String {
    let p = path_str(path);
    if cfg!(windows) {
        format!("type {p}")
    } else {
        format!("cat {p}")
    }
}

/// 输出 1..=n 的递增数字行（unix `seq 1 N`；Windows `for /l`）。
pub fn seq_lines(n: u64, stderr: bool) -> String {
    if cfg!(windows) {
        let redirect = if stderr { " 1>&2" } else { "" };
        format!("for /l %i in (1,1,{n}) do @echo %i{redirect}")
    } else if stderr {
        format!("seq 1 {n} >&2")
    } else {
        format!("seq 1 {n}")
    }
}
