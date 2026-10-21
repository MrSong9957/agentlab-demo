//! codex-shell-command shim（008 §3-J5）：原样拷贝母本 shell-command 的
//! parse_command.rs / bash.rs / shell_detect.rs / powershell.rs（exec cell 渲染与
//! exec_command 展示所需）；command_safety/shell_snapshot* 未拷贝（本地无消费点）。
pub mod parse_command;
pub mod bash;
pub mod shell_detect;
pub mod powershell;
pub mod command_safety;
