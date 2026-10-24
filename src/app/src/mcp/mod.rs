//! MCP 外部工具接入用户轮（022 规格）。
//!
//! 模块职责（022 §3.2）：严格配置（config）、凭证污染检查（secrets）、
//! 模型名净化与冲突拒绝（naming）、工具筛选与定义生成（catalog）、
//! 有界 stdio／Streamable HTTP 传输适配（transport）、连接生命周期
//! （connection）、单次调用与结果投影（call）、管理器与每轮快照（manager）。
//! 协议层完全复用官方 `rmcp` SDK 固定 rev，不自行实现 JSON-RPC 客户端。

pub mod call;
pub mod catalog;
pub mod config;
pub mod connection;
pub mod manager;
pub mod naming;
pub mod secrets;
pub mod transport;

pub use manager::{McpManager, McpSnapshot, ToolEntry};
