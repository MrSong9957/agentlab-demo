//! terminal-chat 库：配置与模型目录（catalog）+ 协议适配（api）+ 同步命令执行（exec）+ 文本补丁修改（apply_patch）+ 最小请求组装（prompt）+ 工具调用循环（turn）+ 会话保存与恢复（session）+ 显示边界（display）+ V1 记忆状态存储（memory，013-M1）+ MCP 外部工具（mcp，022）。

pub mod api;
pub mod apply_patch;
pub mod catalog;
pub mod context;
pub mod context_window;
pub mod display;
pub mod exec;
pub mod mcp;
pub mod memory;
pub mod message_to_user;
pub mod plan;
pub mod proc_session;
pub mod prompt;
pub mod sandbox;
pub mod session;
pub mod turn;
pub mod user_input;
pub mod user_input_async;
pub mod tui_host;
pub mod tool_search;  // 030：tool_search 本地目录搜索
pub mod view_image;

/// 串行化使用进程级环境变量的测试（TCHAT_* 为进程全局；catalog 与 tui_host 的
/// env 测试共用此锁，避免跨模块并发改写）。
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
