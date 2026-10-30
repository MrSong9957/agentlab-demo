//! 035：六工具参数严格校验（§1.3）——所有输入须为 JSON 对象、禁止额外
//! 字段与重复键；null/错误类型/浮点/负数/溢出一律失败，不靠类型强转通过
//!（手写 visitor＋`deserialize_map`，022/030 已验证的先例：派生
//! `Deserialize` 会接受数组位置形式并把显式 null 折叠成缺省）。
//! message 非全空白、≤16 KiB UTF-8，**保存原文不 trim 改写**（trim 仅用于
//! 空白判断）；原始 arguments ≤256 KiB 在入口预检。

use super::{
    MAX_ARGUMENTS_BYTES, MAX_MESSAGE_BYTES, SPAWN_TOOL_NAME,
};

/// task_name 合法形态：`[a-z][a-z0-9_]{0,31}`（§1.3）。
pub fn valid_task_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    let mut len = 1usize;
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return false;
        }
        len += 1;
        if len > 32 {
            return false;
        }
    }
    true
}

/// message 公共校验：非全空白、≤16 KiB；返回原文（不 trim）。
fn check_message(message: String) -> Result<String, String> {
    if message.as_bytes().len() > MAX_MESSAGE_BYTES {
        return Err(format!("message 超过 {MAX_MESSAGE_BYTES} UTF-8 字节上限"));
    }
    if message.trim().is_empty() {
        return Err("message 不能为空或全空白".into());
    }
    Ok(message)
}

/// 入口预检：原始 arguments 字节预算。
fn check_raw(raw: &str) -> Result<(), String> {
    if raw.as_bytes().len() > MAX_ARGUMENTS_BYTES {
        return Err(format!(
            "arguments 超过 {MAX_ARGUMENTS_BYTES} UTF-8 字节上限"
        ));
    }
    Ok(())
}

/// spawn_agent 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnArgs {
    pub task_name: String,
    pub message: String,
    /// all/none；缺省 all。
    pub fork_turns: ForkTurns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkTurns {
    All,
    None,
}

impl ForkTurns {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::None => "none",
        }
    }
}

/// fork_turns 字段值：仅精确字符串 "all"/"none"（null/其他字符串拒绝）。
struct ForkField(ForkTurns);

impl<'de> serde::Deserialize<'de> for ForkField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = ForkField;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("字符串 all 或 none")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "all" => Ok(ForkField(ForkTurns::All)),
                    "none" => Ok(ForkField(ForkTurns::None)),
                    other => Err(serde::de::Error::custom(format!(
                        "fork_turns 仅允许 all/none，得到 {other:?}"
                    ))),
                }
            }
        }
        deserializer.deserialize_str(V)
    }
}

impl<'de> serde::Deserialize<'de> for SpawnArgs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = SpawnArgs;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("spawn_agent 参数对象（task_name、message、可选 fork_turns）")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut task_name: Option<String> = None;
                let mut message: Option<String> = None;
                let mut fork: Option<ForkField> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "task_name" => {
                            if task_name.is_some() {
                                return Err(serde::de::Error::custom("重复字段：task_name"));
                            }
                            task_name = Some(map.next_value()?);
                        }
                        "message" => {
                            if message.is_some() {
                                return Err(serde::de::Error::custom("重复字段：message"));
                            }
                            message = Some(map.next_value()?);
                        }
                        "fork_turns" => {
                            if fork.is_some() {
                                return Err(serde::de::Error::custom("重复字段：fork_turns"));
                            }
                            fork = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::custom(format!("未知字段：{other}")));
                        }
                    }
                }
                let task_name = task_name
                    .ok_or_else(|| serde::de::Error::custom("缺少必填字段 task_name"))?;
                let message = message
                    .ok_or_else(|| serde::de::Error::custom("缺少必填字段 message"))?;
                Ok(SpawnArgs {
                    task_name,
                    message,
                    fork_turns: fork.map(|f| f.0).unwrap_or(ForkTurns::All),
                })
            }
        }
        deserializer.deserialize_map(V)
    }
}

pub fn parse_spawn(raw: &str) -> Result<SpawnArgs, String> {
    check_raw(raw)?;
    let args: SpawnArgs = serde_json::from_str(raw).map_err(|e| {
        format!("{SPAWN_TOOL_NAME} 参数不合法（{e}）：仅允许 task_name、message 与可选 fork_turns")
    })?;
    if !valid_task_name(&args.task_name) {
        return Err(format!(
            "task_name 须匹配 [a-z][a-z0-9_]{{0,31}}，得到 {:?}",
            args.task_name
        ));
    }
    Ok(SpawnArgs {
        message: check_message(args.message)?,
        ..args
    })
}

/// send_message / followup_task 参数（target＋message）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetMessageArgs {
    pub target: String,
    pub message: String,
}

impl<'de> serde::Deserialize<'de> for TargetMessageArgs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = TargetMessageArgs;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("参数对象（target、message）")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut target: Option<String> = None;
                let mut message: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "target" => {
                            if target.is_some() {
                                return Err(serde::de::Error::custom("重复字段：target"));
                            }
                            target = Some(map.next_value()?);
                        }
                        "message" => {
                            if message.is_some() {
                                return Err(serde::de::Error::custom("重复字段：message"));
                            }
                            message = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::custom(format!("未知字段：{other}")));
                        }
                    }
                }
                let target = target
                    .ok_or_else(|| serde::de::Error::custom("缺少必填字段 target"))?;
                let message = message
                    .ok_or_else(|| serde::de::Error::custom("缺少必填字段 message"))?;
                Ok(TargetMessageArgs { target, message })
            }
        }
        deserializer.deserialize_map(V)
    }
}

pub fn parse_target_message(raw: &str, tool: &str) -> Result<TargetMessageArgs, String> {
    check_raw(raw)?;
    let args: TargetMessageArgs = serde_json::from_str(raw)
        .map_err(|e| format!("{tool} 参数不合法（{e}）：仅允许 target 与 message"))?;
    if args.target.is_empty() {
        return Err("target 不能为空".into());
    }
    Ok(TargetMessageArgs {
        message: check_message(args.message)?,
        ..args
    })
}

/// interrupt_agent 参数（仅 target）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetArgs {
    pub target: String,
}

impl<'de> serde::Deserialize<'de> for TargetArgs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = TargetArgs;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("参数对象（target）")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut target: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "target" => {
                            if target.is_some() {
                                return Err(serde::de::Error::custom("重复字段：target"));
                            }
                            target = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::custom(format!("未知字段：{other}")));
                        }
                    }
                }
                let target = target
                    .ok_or_else(|| serde::de::Error::custom("缺少必填字段 target"))?;
                Ok(TargetArgs { target })
            }
        }
        deserializer.deserialize_map(V)
    }
}

pub fn parse_target(raw: &str, tool: &str) -> Result<TargetArgs, String> {
    check_raw(raw)?;
    let args: TargetArgs = serde_json::from_str(raw)
        .map_err(|e| format!("{tool} 参数不合法（{e}）：仅允许 target"))?;
    if args.target.is_empty() {
        return Err("target 不能为空".into());
    }
    Ok(args)
}

/// wait_agent 参数（可选 timeout_ms，整数 0–60000，缺省 30000）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitArgs {
    pub timeout_ms: u64,
}

/// timeout_ms 字段值：仅非负整数（null/浮点/布尔/字符串/负数拒绝）。
struct TimeoutField(u64);

impl<'de> serde::Deserialize<'de> for TimeoutField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = TimeoutField;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("非负整数（null 拒绝）")
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(TimeoutField(value))
            }
        }
        deserializer.deserialize_u64(V)
    }
}

/// wait_agent 超时上限（§1.3）。
pub const MAX_WAIT_TIMEOUT_MS: u64 = 60000;
pub const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30000;

impl<'de> serde::Deserialize<'de> for WaitArgs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = WaitArgs;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("wait_agent 参数对象（可选 timeout_ms）")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut timeout: Option<TimeoutField> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "timeout_ms" => {
                            if timeout.is_some() {
                                return Err(serde::de::Error::custom("重复字段：timeout_ms"));
                            }
                            timeout = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::custom(format!("未知字段：{other}")));
                        }
                    }
                }
                Ok(WaitArgs {
                    timeout_ms: timeout.map(|t| t.0).unwrap_or(DEFAULT_WAIT_TIMEOUT_MS),
                })
            }
        }
        deserializer.deserialize_map(V)
    }
}

pub fn parse_wait(raw: &str, tool: &str) -> Result<WaitArgs, String> {
    check_raw(raw)?;
    let args: WaitArgs = serde_json::from_str(raw)
        .map_err(|e| format!("{tool} 参数不合法（{e}）：仅允许可选 timeout_ms"))?;
    if args.timeout_ms > MAX_WAIT_TIMEOUT_MS {
        return Err(format!(
            "timeout_ms 必须是 0–{MAX_WAIT_TIMEOUT_MS} 的整数，得到 {}",
            args.timeout_ms
        ));
    }
    Ok(args)
}

/// list_agents 参数（可选 path_prefix：/root 或某个精确子路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListArgs {
    pub path_prefix: Option<String>,
}

impl<'de> serde::Deserialize<'de> for ListArgs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = ListArgs;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("list_agents 参数对象（可选 path_prefix）")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut prefix: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "path_prefix" => {
                            if prefix.is_some() {
                                return Err(serde::de::Error::custom("重复字段：path_prefix"));
                            }
                            prefix = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::custom(format!("未知字段：{other}")));
                        }
                    }
                }
                Ok(ListArgs { path_prefix: prefix })
            }
        }
        deserializer.deserialize_map(V)
    }
}

pub fn parse_list(raw: &str, tool: &str) -> Result<ListArgs, String> {
    check_raw(raw)?;
    let args: ListArgs = serde_json::from_str(raw)
        .map_err(|e| format!("{tool} 参数不合法（{e}）：仅允许可选 path_prefix"))?;
    if let Some(prefix) = args.path_prefix.as_deref() {
        if prefix.is_empty() {
            return Err("path_prefix 不能为空字符串（省略即列全组）".into());
        }
    }
    Ok(args)
}

/// target 路径严格校验（§1.3）：根可用相对 name 或完整 `/root/name` 定位
/// 当前组子 Agent；不接受裸内部 ID、URL、反斜线、点路径、编码斜线、尾斜线
/// 或任意层级路径，不做百分号解码或名称净化。
///
/// 返回 `Ok(None)`＝target 恰为 `/root`；`Ok(Some(name))`＝当前组子 Agent
/// 名（两种写法归一）。非法形态返回 Err（INVALID_ARGUMENT 层）。
pub fn parse_target_path(target: &str) -> Result<Option<String>, String> {
    if target == super::ROOT_PATH {
        return Ok(None);
    }
    let name = if let Some(rest) = target.strip_prefix("/root/") {
        rest
    } else {
        target
    };
    if name.is_empty() {
        return Err("target 路径非法：名称段为空".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("target 路径非法：只支持一层 /root/<name>，不接受多层级路径".into());
    }
    if name.contains('.') {
        return Err("target 路径非法：不接受点路径".into());
    }
    if name.contains('%') {
        return Err("target 路径非法：不做百分号解码".into());
    }
    if !valid_task_name(name) {
        return Err(format!(
            "target 名称须匹配 [a-z][a-z0-9_]{{0,31}}，得到 {name:?}"
        ));
    }
    Ok(Some(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_roundtrip_and_defaults() {
        let a = parse_spawn(r#"{"task_name":"review","message":"检查一致性"}"#).unwrap();
        assert_eq!(a.fork_turns, ForkTurns::All);
        let a = parse_spawn(r#"{"task_name":"a1","message":"x","fork_turns":"none"}"#).unwrap();
        assert_eq!(a.fork_turns, ForkTurns::None);
        // 原文保留：不 trim 改写（空白消息除外）
        let a = parse_spawn(r#"{"task_name":"a1","message":"  内容  "}"#).unwrap();
        assert_eq!(a.message, "  内容  ");
    }

    #[test]
    fn spawn_rejects_bad_shapes() {
        // 数组位置形式 / 未知字段 / 重复键 / null message / 浮点 fork / 非法名
        for raw in [
            r#"["a1","x"]"#,
            r#"{"task_name":"a1","message":"x","extra":1}"#,
            r#"{"task_name":"a1","task_name":"a2","message":"x"}"#,
            r#"{"task_name":"a1","message":null}"#,
            r#"{"task_name":"a1","message":"x","fork_turns":1}"#,
            r#"{"task_name":"A1","message":"x"}"#,
            r#"{"task_name":"1a","message":"x"}"#,
            r#"{"task_name":"a","message":"x","fork_turns":"ALL"}"#,
        ] {
            assert!(parse_spawn(raw).is_err(), "应拒绝：{raw}");
        }
        // 全空白与超长消息
        assert!(parse_spawn(r#"{"task_name":"a","message":" \t "}"#).is_err());
        let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
        assert!(parse_spawn(&format!(r#"{{"task_name":"a","message":"{long}"}}"#)).is_err());
        // 33 字符名
        let long_name = format!("a{}", "b".repeat(32));
        assert!(parse_spawn(&format!(r#"{{"task_name":"{long_name}","message":"x"}}"#)).is_err());
    }

    #[test]
    fn wait_and_list_shapes() {
        assert_eq!(
            parse_wait("{}", "w").unwrap().timeout_ms,
            DEFAULT_WAIT_TIMEOUT_MS
        );
        assert_eq!(parse_wait(r#"{"timeout_ms":0}"#, "w").unwrap().timeout_ms, 0);
        assert!(parse_wait(r#"{"timeout_ms":60001}"#, "w").is_err());
        assert!(parse_wait(r#"{"timeout_ms":-1}"#, "w").is_err());
        assert!(parse_wait(r#"{"timeout_ms":1.5}"#, "w").is_err());
        assert!(parse_wait(r#"{"timeout_ms":null}"#, "w").is_err());
        assert!(parse_wait(r#"{"timeout_ms":"30"}"#, "w").is_err());
        let l = parse_list("{}", "l").unwrap();
        assert_eq!(l.path_prefix, None);
        assert!(parse_list(r#"{"path_prefix":""}"#, "l").is_err());
    }

    #[test]
    fn target_path_resolution() {
        assert_eq!(parse_target_path("/root").unwrap(), None);
        assert_eq!(parse_target_path("review").unwrap().as_deref(), Some("review"));
        assert_eq!(
            parse_target_path("/root/review").unwrap().as_deref(),
            Some("review")
        );
        for bad in [
            "/root/", "/root/a/b", "a/b", "a\\b", ".", "..", "a/../b", "a%2Fb",
            "/root/%2e", "A1", "1a", "a.b", "-a", "a b",
        ] {
            assert!(parse_target_path(bad).is_err(), "应拒绝：{bad}");
        }
        // 裸 "root" 是合法相对名（子 Agent 可叫 root，路径 /root/root）
        assert_eq!(parse_target_path("root").unwrap().as_deref(), Some("root"));
    }
}
