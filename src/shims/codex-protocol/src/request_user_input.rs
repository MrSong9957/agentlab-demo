//! Codex d138398：protocol/src/request_user_input.rs 的问答类型子集。
//! 仅去掉未使用的 JsonSchema/TS 派生；不恢复事件和 App Server 包装。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestUserInputQuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(rename = "isOther", default)]
    pub is_other: bool,
    #[serde(rename = "isSecret", default)]
    pub is_secret: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<RequestUserInputQuestionOption>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestUserInputAnswer {
    pub answers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RequestUserInputResponse {
    pub answers: HashMap<String, RequestUserInputAnswer>,
}

/// Codex d138398 `protocol/src/items.rs::AsyncUserInputQuestion` 的必要子集：
/// `title`＋可省略的字符串数组 `options`，拒绝未知字段——不是同步工具的
/// id/header/选项说明结构。归一化到本地 [`RequestUserInputQuestion`]。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AsyncUserInputQuestion {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// 018 本地宿主通道终态；不进入 V1 持久化或模型参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestUserInputError {
    Cancelled,
    HostExited,
    Closed,
    InvalidResponse,
}

impl RequestUserInputError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Cancelled => "用户已取消本轮：问询未完成",
            Self::HostExited => "宿主退出：问询未完成",
            Self::Closed => "回答通道关闭：问询未完成",
            Self::InvalidResponse => "回答无效：包含本次问询之外的问题 ID，问询未完成",
        }
    }
}
