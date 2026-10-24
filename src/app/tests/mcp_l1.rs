//! 022 L1：凭证污染检查（A16 核心）、模型命名（A06）、目录筛选与 Schema
//! 校验（A07／A08 的本层部分）。
//!
//! 命名黄金值、筛选语义、Schema 归一、敏感值集合的输入输出与状态迁移。

use std::collections::HashSet;

use rmcp::model::Tool;
use serde_json::json;

use terminal_chat::mcp::catalog::{build_server_catalog, validate_input_schema};
use terminal_chat::mcp::naming::{assign_model_names, sanitize_part, NameAssignError};
use terminal_chat::mcp::secrets::{bearer_sensitive_values, SensitiveValues};

fn tool(name: &str, schema: serde_json::Value) -> Tool {
    // SDK Tool 为 non_exhaustive，只能逐字段赋值。
    let mut t = Tool::default();
    t.name = name.to_string().into();
    t.input_schema = std::sync::Arc::new(schema.as_object().expect("schema 对象").clone());
    t
}

// ---------- secrets：敏感值集合与污染判定（A16 核心） ----------

#[test]
fn secrets_bearer_value_and_header_forms_are_sensitive() {
    let values = SensitiveValues::from_values(bearer_sensitive_values("sk-tok-022"));
    assert!(values.contains_secret("前缀 sk-tok-022 后缀"));
    assert!(values.contains_secret("Bearer sk-tok-022"));
    assert!(values.contains_secret("Authorization: Bearer sk-tok-022"));
    // 子串包含判定：含敏感值的文本一律视为污染（含其扩展形式）。
    assert!(values.contains_secret("sk-tok-022-别的值"));
    assert!(!values.contains_secret("无关文本"));
}

#[test]
fn secrets_empty_values_are_ignored_and_duplicates_deduped() {
    let values = SensitiveValues::from_values(["".to_string(), "v".to_string(), "v".to_string()]);
    assert!(!values.contains_secret(""));
    assert!(values.contains_secret("xvx"));
}

#[test]
fn secrets_cross_chunk_composition_is_covered_by_final_string_check() {
    // 检测发生在最终合成字符串上：分块拼出的完整结果仍被检出。
    let values = SensitiveValues::from_values(bearer_sensitive_values("SECTOK"));
    let composed = format!("{}{}{}", "par", "t1 SECT", "OK part2");
    assert!(values.contains_secret(&composed));
}

// ---------- naming：模型名生成（A06） ----------

#[test]
fn naming_basic_form_and_sanitization() {
    // 移植 Codex 净化规则：仅 ASCII 字母数字与下划线保留，连字符同样替换。
    assert_eq!(sanitize_part("a-b.c/д"), "a_b_c__");
    let names = assign_model_names(&[("docs".into(), "search".into())]).unwrap();
    assert_eq!(names, vec!["mcp__docs__search"]);
    // 全部非法字符 → 下划线（"mcp__" + "___" + "__" + "__"）。
    let names = assign_model_names(&[("日内瓦".into(), "查询".into())]).unwrap();
    assert_eq!(names, vec!["mcp_________"]);
}

#[test]
fn naming_golden_values_for_unicode_punctuation_and_double_underscore() {
    let pairs = vec![
        ("a__b".to_string(), "c".to_string()),
        ("a".to_string(), "b__c".to_string()),
        ("中文".to_string(), "工具!.?".to_string()),
    ];
    let names = assign_model_names(&pairs).unwrap();
    // 前两项是分隔歧义冲突（净化后同为 mcp__a__b__c）→ 各附稳定短哈希。
    assert_ne!(names[0], names[1]);
    for n in &names[..2] {
        assert!(n.starts_with("mcp__a__b__c_"), "实际：{n}");
        assert_eq!(n.len(), "mcp__a__b__c_".len() + 12);
    }
    // "mcp__" + 净化("中文")="__" + "__" + 净化("工具!.?")="_____" → 14 字符。
    assert_eq!(names[2], "mcp___________");
}

#[test]
fn naming_order_independent_and_stable_for_reversed_enumeration() {
    let forward = vec![
        ("s".to_string(), "t1".to_string()),
        ("s".to_string(), "t2".to_string()),
    ];
    let mut reversed = forward.clone();
    reversed.reverse();
    let mut fwd = assign_model_names(&forward).unwrap();
    let mut rev = assign_model_names(&reversed).unwrap();
    fwd.sort();
    rev.sort();
    assert_eq!(fwd, rev, "枚举顺序不影响同一目录的命名集合");
}

#[test]
fn naming_conflicts_get_stable_hash_suffix_within_64_chars() {
    // 分隔歧义冲突：两个不同原始身份净化到同一基础名 → 都附哈希。
    let names = assign_model_names(&[
        ("a__b".to_string(), "c".to_string()),
        ("a".to_string(), "b__c".to_string()),
    ])
    .unwrap();
    assert_ne!(names[0], names[1]);
    for n in &names {
        assert!(n.starts_with("mcp__a__b__c_"), "冲突名应带哈希后缀：{n}");
        assert_eq!(n.len(), "mcp__a__b__c_".len() + 12);
        assert!(
            n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "最终名只含合法字符：{n}"
        );
    }
    // 同输入两次结果一致（稳定短哈希）
    let again = assign_model_names(&[
        ("a__b".to_string(), "c".to_string()),
        ("a".to_string(), "b__c".to_string()),
    ])
    .unwrap();
    assert_eq!(names, again);
}

#[test]
fn naming_overlong_names_are_hashed_and_truncated_to_64() {
    let long_tool = "t".repeat(80);
    let names = assign_model_names(&[("s".into(), long_tool.clone())]).unwrap();
    assert_eq!(names[0].len(), 64);
    assert!(names[0].starts_with("mcp__s__"));
    assert!(names[0].ends_with(char::is_alphanumeric));
}

#[test]
fn naming_duplicate_raw_name_is_directory_error() {
    let err = assign_model_names(&[("s".into(), "t".into()), ("s".into(), "t".into())])
        .unwrap_err();
    assert!(matches!(err, NameAssignError::DuplicateRaw { .. }));
}

// ---------- catalog：Schema 校验（A08 本层） ----------

#[test]
fn schema_root_type_rules() {
    // 缺省 type 允许
    validate_input_schema(&json!({"properties": {}})).unwrap();
    // 显式 object 允许
    validate_input_schema(&json!({"type": "object"})).unwrap();
    // 其他根类型拒绝
    let e = validate_input_schema(&json!({"type": "string"})).unwrap_err();
    assert!(e.contains("object"), "实际：{e}");
    // 非 JSON 对象拒绝
    assert!(validate_input_schema(&json!("x")).is_err());
    assert!(validate_input_schema(&json!(null)).is_err());
}

#[test]
fn schema_properties_defaults_to_empty_object_and_keeps_constraints() {
    // properties null → 补空对象
    let out = validate_input_schema(&json!({"type": "object", "properties": null})).unwrap();
    assert_eq!(out["properties"], json!({}));
    // 缺省 properties → 补空对象
    let out = validate_input_schema(&json!({"type": "object"})).unwrap();
    assert_eq!(out["properties"], json!({}));
    // 非对象 properties 拒绝
    assert!(validate_input_schema(&json!({"properties": []})).is_err());
    // required／enum／$defs 原样保留（不删、不压缩）
    let schema = json!({
        "type": "object",
        "properties": {"q": {"type": "string", "enum": ["a", "b"]}},
        "required": ["q"],
        "$defs": {"x": {"type": "number"}},
        "additionalProperties": false
    });
    let out = validate_input_schema(&schema).unwrap();
    assert_eq!(out, schema, "合法 Schema 必须逐字段保留");
}

// ---------- catalog：筛选（A07 本层） ----------

#[test]
fn filter_allowlist_semantics_none_all_empty_zero_disabled_wins() {
    let tools = vec![tool("a", json!({"type": "object"})), tool("b", json!({"type": "object"}))];
    let none: Option<HashSet<String>> = None;
    let out = build_server_catalog(&tools, none.as_ref(), &HashSet::new(), &SensitiveValues::default())
        .unwrap();
    assert_eq!(out.kept.len(), 2, "缺省允许名单＝全部");

    let empty = Some(HashSet::new());
    let out = build_server_catalog(&tools, empty.as_ref(), &HashSet::new(), &SensitiveValues::default())
        .unwrap();
    assert_eq!(out.kept.len(), 0, "显式空数组＝零工具");

    let mut allow = HashSet::new();
    allow.insert("a".to_string());
    allow.insert("ghost".to_string());
    let mut deny = HashSet::new();
    deny.insert("a".to_string());
    let out = build_server_catalog(&tools, Some(&allow), &deny, &SensitiveValues::default())
        .unwrap();
    assert_eq!(out.kept.len(), 0, "禁用名单优先于允许名单");
    assert_eq!(out.unknown_allowlisted, vec!["ghost".to_string()]);
}

#[test]
fn filter_duplicate_raw_names_fail_whole_server() {
    let tools = vec![tool("dup", json!({"type": "object"})), tool("dup", json!({"type": "object"}))];
    let err = match build_server_catalog(&tools, None, &HashSet::new(), &SensitiveValues::default()) {
        Err(e) => e,
        Ok(_) => panic!("重复原始名应为服务目录错误"),
    };
    assert!(err.contains("重复原始名"), "实际：{err}");
}

#[test]
fn filter_polluted_definitions_are_excluded_with_generic_reason() {
    let sensitive = SensitiveValues::from_values(bearer_sensitive_values("CANTOK"));
    // 描述被污染
    let mut t = tool("clean_name", json!({"type": "object"}));
    t.description = Some("uses CANTOK internally".into());
    let tools = vec![
        t,
        tool("schema_ok", json!({"type": "object"})),
    ];
    let out = build_server_catalog(&tools, None, &HashSet::new(), &sensitive).unwrap();
    assert_eq!(out.kept.len(), 1);
    assert_eq!(out.kept[0].0, "schema_ok");
    assert!(out.excluded[0].contains("敏感数据"), "通用原因：{}", out.excluded[0]);
    assert!(!out.excluded[0].contains("CANTOK"), "原因不得回显敏感值");

    // Schema 被污染
    let schema = json!({"type": "object", "properties": {"k": {"description": "CANTOK"}}});
    let tools = vec![tool("schema_polluted", schema)];
    let out = build_server_catalog(&tools, None, &HashSet::new(), &sensitive).unwrap();
    assert!(out.kept.is_empty());
    assert_eq!(out.excluded.len(), 1);
}
