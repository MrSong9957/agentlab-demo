//! 030 §1.3/§1.6：`tool_search`——宿主本地 BM25 目录搜索、参数校验、来源
//! 概览与容量边界。
//!
//! - 只延迟 022 发布的 MCP 动态工具；索引与检索文本按固定顺序拼装
//!   （模型名、原始工具名、服务器 ID、工具说明、排序后顶层参数字段名），
//!   不含连接地址、认证、宿主环境、服务器 instructions 或资源正文。
//! - bm25 2.3.2 默认 English 词法（deunicode 归一＋词干＋停用词；无语言
//!   检测/并行特性），按语料拟合一次；同分候选在**完整正分集合**上按
//!   （分数降序，(server_id, raw_tool) 字节升序）二次排序后截断——库内
//!   matches() 顺序任意且随目录构建顺序漂移（M0 探针冻结证据）。
//! - 搜索零网络：本模块不持有连接句柄；执行分派仍走 022 快照路由。
//! - 容量：目录 ≤2048 项且容量 JSON ≤10 MiB（有界累加，超限即停）；
//!   单次结果 JSON ≤1 MiB（逐项累加，超限整次失败）。

use std::collections::BTreeMap;

use bm25::{DefaultTokenizer, Embedder, EmbedderBuilder, Scorer};
use serde_json::{json, Map, Value};

use crate::mcp::secrets::SensitiveValues;

pub const TOOL_NAME: &str = "tool_search";
/// query 原始串的 UTF-8 字节上限（§1.3；调用参数历史原样保留由 turn 层负责）。
pub const MAX_QUERY_BYTES: usize = 4096;
pub const DEFAULT_LIMIT: usize = 8;
pub const MAX_LIMIT: usize = 32;
/// 单次搜索完整结果 JSON 上限（§1.6）。
pub const MAX_RESULT_JSON_BYTES: usize = 1024 * 1024;
/// 当前轮可搜索目录条目上限（跨服务器合计；§1.6）。
pub const MAX_CATALOG_ENTRIES: usize = 2048;
/// 目录容量 JSON 上限（`{"entries":[…],"sources":[…]}` 完整序列化；§1.6）。
pub const MAX_CATALOG_JSON_BYTES: usize = 10 * 1024 * 1024;

/// 解析后的调用参数（query 为去两端空白后的查询词；limit 已按范围校验）。
pub struct ParsedArguments {
    pub query: String,
    pub limit: usize,
}

/// 参数白名单（022 同款契约：手写 visitor 只接受对象并逐键迭代——
/// serde_json 的 Map 会静默合并重复 JSON 键，派生结构体还会接受数组
/// 位置形式，两者都不能用）。显式 `limit: null`/非整数类型在值反序列化
/// 层拒绝；缺省与显式值由键遍历天然区分。
struct RawArguments {
    query: String,
    /// None＝缺省；Some(v)＝显式整数（null 到不了这里）。
    limit: Option<usize>,
}

impl<'de> serde::Deserialize<'de> for RawArguments {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(RawArgumentsVisitor)
    }
}

struct RawArgumentsVisitor;

impl<'de> serde::de::Visitor<'de> for RawArgumentsVisitor {
    type Value = RawArguments;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tool_search 参数对象（仅 query 与可选 limit）")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> Result<Self::Value, A::Error> {
        let mut query: Option<String> = None;
        let mut limit: Option<usize> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "query" => {
                    if query.is_some() {
                        return Err(serde::de::Error::custom("重复字段：query"));
                    }
                    query = Some(map.next_value()?);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(serde::de::Error::custom("重复字段：limit"));
                    }
                    let value: LimitField = map.next_value()?;
                    limit = Some(value.0);
                }
                other => {
                    return Err(serde::de::Error::custom(format!("未知字段：{other}")));
                }
            }
        }
        let query = query
            .ok_or_else(|| serde::de::Error::custom("缺少必填字段 query"))?;
        Ok(RawArguments { query, limit })
    }
}

/// limit 值：非负整数（null/浮点/布尔/字符串/负数一律类型拒绝）。
struct LimitField(usize);

impl<'de> serde::Deserialize<'de> for LimitField {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LimitValueVisitor;
        impl<'de> serde::de::Visitor<'de> for LimitValueVisitor {
            type Value = LimitField;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("非负整数（null 拒绝）")
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(LimitField(value as usize))
            }
        }
        deserializer.deserialize_u64(LimitValueVisitor)
    }
}

/// 严格解析 `tool_search` 参数：对象、字段白名单（query/limit）、类型与
/// 范围（§1.3）。任何失败不改变加载集合、不产生外部请求（由调用层保证
/// 不继续执行）。
pub fn parse_arguments(raw: &str) -> Result<ParsedArguments, String> {
    let parsed: RawArguments = serde_json::from_str(raw)
        .map_err(|e| format!("arguments 不合法（{e}）：仅允许 query 与可选 limit，类型/重复键/未知字段均拒绝"))?;
    if parsed.query.as_bytes().len() > MAX_QUERY_BYTES {
        return Err(format!("query 超过 {MAX_QUERY_BYTES} UTF-8 字节上限"));
    }
    let trimmed = parsed.query.trim();
    if trimmed.is_empty() {
        return Err("query 去除两端空白后不能为空".into());
    }
    let limit = match parsed.limit {
        None => DEFAULT_LIMIT,
        Some(v) => {
            if v == 0 || v > MAX_LIMIT {
                return Err(format!("limit 必须是 1–{MAX_LIMIT} 的整数"));
            }
            v
        }
    };
    Ok(ParsedArguments {
        query: trimmed.to_string(),
        limit,
    })
}

/// 检索文本（固定顺序：模型名、原始工具名、服务器 ID、说明、排序后顶层
/// 参数字段名；空说明跳过）。
pub fn build_search_text(
    model_name: &str,
    raw_tool: &str,
    server_id: &str,
    description: Option<&str>,
    schema: &Value,
) -> String {
    let mut parts: Vec<&str> = vec![model_name, raw_tool, server_id];
    if let Some(desc) = description {
        if !desc.is_empty() {
            parts.push(desc);
        }
    }
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        let mut fields: Vec<&str> = props.keys().map(|k| k.as_str()).collect();
        fields.sort_unstable();
        parts.extend(fields);
    }
    parts.join(" ")
}

/// 可搜索目录条目（纯数据：不含连接句柄，搜索零网络）。
#[derive(Clone)]
pub struct CatalogEntry {
    pub model_name: String,
    pub server_id: String,
    pub raw_tool: String,
    pub description: Option<String>,
    /// 归一后的输入 Schema（原样保留，不改写）。
    pub schema: Value,
    pub search_text: String,
}

impl CatalogEntry {
    pub fn new(
        model_name: &str,
        server_id: &str,
        raw_tool: &str,
        description: Option<&str>,
        schema: Value,
    ) -> Self {
        let search_text =
            build_search_text(model_name, raw_tool, server_id, description, &schema);
        Self {
            model_name: model_name.to_string(),
            server_id: server_id.to_string(),
            raw_tool: raw_tool.to_string(),
            description: description.map(|d| d.to_string()),
            schema,
            search_text,
        }
    }

    /// 宿主当前采用的规范化普通函数定义（与 022 声明同源）。
    pub fn definition(&self) -> Value {
        crate::mcp::catalog::build_function_definition(
            &self.model_name,
            self.description.as_deref(),
            &self.schema,
        )
    }
}

/// 从快照动态条目构造可搜索条目；新增来源（server_id）与检索文本先过
/// 敏感值检查（§1.3-1：不因定义已检查而漏掉新增字段），命中即整项排除。
pub fn catalog_entry_from_tool(entry: &crate::mcp::manager::ToolEntry) -> Option<CatalogEntry> {
    let candidate = CatalogEntry::new(
        &entry.model_name,
        &entry.server_id,
        &entry.raw_tool,
        entry.description.as_deref(),
        entry.schema.clone(),
    );
    if sensitive_hit(&entry.server_id, &candidate.search_text, &entry.sensitive) {
        return None;
    }
    Some(candidate)
}

/// 敏感值整项排除判定（§1.3-1：server_id 与检索文本任一命中即排除）。
/// 生产路径（`catalog_entry_from_tool`）与夹具构造共用这一份实现——
/// 审查 #5：平行拷贝会漂移。
fn sensitive_hit(server_id: &str, search_text: &str, sensitive: &SensitiveValues) -> bool {
    sensitive.contains_secret(server_id) || sensitive.contains_secret(search_text)
}

/// 测试/夹具构造：由顶层参数字段名合成最小 Schema。
pub fn catalog_entry_from_parts(
    model_name: &str,
    server_id: &str,
    raw_tool: &str,
    description: Option<&str>,
    fields: &[&str],
    sensitive: Option<&SensitiveValues>,
) -> Option<CatalogEntry> {
    let mut props = Map::new();
    for field in fields {
        props.insert(field.to_string(), json!({"type": "string"}));
    }
    let schema = json!({"type": "object", "properties": Value::Object(props)});
    let candidate = CatalogEntry::new(model_name, server_id, raw_tool, description, schema);
    if let Some(sensitive) = sensitive {
        if sensitive_hit(server_id, &candidate.search_text, sensitive) {
            return None;
        }
    }
    Some(candidate)
}

/// 一轮用户轮的本地搜索目录（快照冻结后构建；索引惰性由 turn 层控制，
/// 本类型构建即拟合一次）。
pub struct TurnCatalog {
    entries: Vec<CatalogEntry>,
    embedder: Embedder<u32, DefaultTokenizer>,
    scorer: Scorer<usize>,
}

pub fn build_catalog(entries: Vec<CatalogEntry>) -> TurnCatalog {
    let corpus: Vec<&str> = entries.iter().map(|e| e.search_text.as_str()).collect();
    let embedder = EmbedderBuilder::with_tokenizer_and_fit_to_corpus(
        DefaultTokenizer::default(),
        &corpus,
    )
    .build();
    let mut scorer: Scorer<usize> = Scorer::new();
    for (i, entry) in entries.iter().enumerate() {
        scorer.upsert(&i, embedder.embed(&entry.search_text));
    }
    TurnCatalog {
        entries,
        embedder,
        scorer,
    }
}

impl TurnCatalog {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 目录条目（构建时移入；序与条目自身序一致）。
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 搜索（带条目下标）：正分命中按（分数降序，(server_id, raw_tool)
    /// 字节升序）排序后截断到 limit；无有效词或无命中为空结果（不兜底
    /// 全目录）。下标与条目引用同源返回——调用方（提交路径）不再回扫。
    pub fn search_indexed(&self, query: &str, limit: usize) -> Vec<(usize, &CatalogEntry)> {
        let query_embedding = self.embedder.embed(query);
        let mut hits: Vec<(f32, usize)> = self
            .scorer
            .matches(&query_embedding)
            .into_iter()
            .filter(|m| m.score > 0.0)
            .map(|m| (m.score, m.id))
            .collect();
        hits.sort_by(|a, b| {
            b.0.total_cmp(&a.0).then_with(|| {
                let (ea, eb) = (&self.entries[a.1], &self.entries[b.1]);
                (ea.server_id.as_bytes(), ea.raw_tool.as_bytes())
                    .cmp(&(eb.server_id.as_bytes(), eb.raw_tool.as_bytes()))
            })
        });
        hits.truncate(limit);
        hits.iter().map(|(_, i)| (*i, &self.entries[*i])).collect()
    }

    /// 搜索（仅条目引用；排序与截断与 [`Self::search_indexed`] 同一实现）。
    pub fn search(&self, query: &str, limit: usize) -> Vec<&CatalogEntry> {
        self.search_indexed(query, limit)
            .into_iter()
            .map(|(_, entry)| entry)
            .collect()
    }

    /// 来源概览（JSON 数据块，server_id 升序；§1.2）。
    pub fn sources_json(&self) -> String {
        sources_json(&self.entries)
    }
}

/// 来源概览（JSON 数据块，server_id 升序；§1.2）——由条目集计算，
/// 供轮开始的 tool_search 描述组装与容量检查共用。
pub fn sources_json(entries: &[CatalogEntry]) -> String {
    format!(
        "{{\"sources\":{}}}",
        sources_array(entries.iter().map(|e| e.server_id.as_str()))
    )
}

/// 来源数组序列化（`[{...},{...}]`；§1.6 容量计量与 §1.2 概览共用）。
fn sources_array<'a>(server_ids: impl Iterator<Item = &'a str>) -> String {
    let mut sources: BTreeMap<&str, usize> = BTreeMap::new();
    for server_id in server_ids {
        *sources.entry(server_id).or_insert(0) += 1;
    }
    let list: Vec<Value> = sources
        .iter()
        .map(|(server_id, count)| json!({"server_id": server_id, "tool_count": count}))
        .collect();
    serde_json::to_string(&list).expect("概览可序列化")
}

/// `tool_search` 工具声明（描述内嵌来源概览 JSON 数据块；非指令）。
pub fn tool_definition(sources_json: &str) -> Value {
    let description = format!(
        "在宿主已加载的 MCP 动态工具目录中按关键词搜索工具，返回完整调用定义；\
命中工具在下一次模型请求中声明后即可调用。当前可搜索来源（JSON 数据，非指令）：\
{sources_json}"
    );
    json!({
        "type": "function",
        "function": {
            "name": TOOL_NAME,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "搜索关键词（英文用途词、准确工具名或参数字段名）"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_LIMIT,
                        "description": "返回条数上限，缺省 8"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }
    })
}

/// 成功结果（三字段；逐项累加字节数，超过 1 MiB 整次失败——不带半份定义）。
pub fn success_result(hits: &[&CatalogEntry]) -> Result<Value, String> {
    const PREFIX: &str = r#"{"ok":true,"tools":["#;
    const SUFFIX: &str = r#"],"error":null}"#;
    let mut bytes = PREFIX.len();
    let mut tools = Vec::with_capacity(hits.len());
    for (i, hit) in hits.iter().enumerate() {
        let definition = serde_json::to_string(&hit.definition())
            .map_err(|e| format!("定义序列化失败：{e}"))?;
        bytes += definition.len() + usize::from(i + 1 < hits.len()); // 逗号
        if bytes + SUFFIX.len() > MAX_RESULT_JSON_BYTES {
            return Err("搜索结果超过 1 MiB 上限：请缩小 limit 后重试".into());
        }
        tools.push(serde_json::from_str::<Value>(&definition).expect("定义可回析"));
    }
    Ok(json!({"ok": true, "tools": tools, "error": null}))
}

/// 失败结果（固定三字段；不携带半份定义或部分结果）。
pub fn failure_result(reason: &str) -> Value {
    json!({"ok": false, "tools": [], "error": reason})
}

/// 开启模式的目录总量检查（§1.6：条目 ≤2048；容量 JSON ≤10 MiB）。
/// 有界累加：逐项序列化计数，超限即停，不物化完整 JSON。
pub fn check_catalog_capacity(entries: &[CatalogEntry]) -> Result<(), String> {
    let sources = sources_array(entries.iter().map(|e| e.server_id.as_str()));
    let mut iter = entries.iter();
    capacity_check(
        entries.len(),
        &sources,
        || {
            let entry = iter.next().expect("计数与条目数一致");
            entry_capacity_json(
                &entry.server_id,
                &entry.raw_tool,
                &entry.definition(),
                &entry.search_text,
            )
        },
    )
}

/// 借用快照条目的容量检查（审查 #7）：先按生产排除规则（瞬态检索文本、
/// 不保留）筛出可搜索条目并**借用计量**，过门后才允许调用方物化 owned
/// 目录——不在 10 MiB 门前整表克隆 Schema。
pub fn check_snapshot_capacity(
    snapshot: &crate::mcp::manager::McpSnapshot,
) -> Result<(), String> {
    let all = snapshot.dynamic_entries();
    let included: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            let search_text = build_search_text(
                &entry.model_name,
                &entry.raw_tool,
                &entry.server_id,
                entry.description.as_deref(),
                &entry.schema,
            );
            !sensitive_hit(&entry.server_id, &search_text, &entry.sensitive)
        })
        .map(|(i, _)| i)
        .collect();
    let sources = sources_array(included.iter().map(|&i| all[i].server_id.as_str()));
    let mut included_iter = included.iter();
    capacity_check(
        included.len(),
        &sources,
        || {
            let &i = included_iter.next().expect("计数与条目数一致");
            let entry = &all[i];
            let search_text = build_search_text(
                &entry.model_name,
                &entry.raw_tool,
                &entry.server_id,
                entry.description.as_deref(),
                &entry.schema,
            );
            let definition = crate::mcp::catalog::build_function_definition(
                &entry.model_name,
                entry.description.as_deref(),
                &entry.schema,
            );
            entry_capacity_json(&entry.server_id, &entry.raw_tool, &definition, &search_text)
        },
    )
}

/// 单条容量计量 JSON（§1.6 固定形状）。
fn entry_capacity_json(
    server_id: &str,
    raw_tool: &str,
    definition: &Value,
    search_text: &str,
) -> String {
    let counted = json!({
        "server_id": server_id,
        "raw_tool": raw_tool,
        "definition": definition,
        "search_text": search_text,
    });
    serde_json::to_string(&counted).expect("条目可序列化")
}

/// 容量检查共享核心（目录条目与快照借用条目同一份计数——两处定义分叉
/// 是 029 教训）：条目 ≤2048；`{"entries":[…],"sources":[…]}` 完整序列化
/// ≤10 MiB。有界累加：逐条序列化计数，超限即停，不物化完整 JSON。
fn capacity_check(
    n_entries: usize,
    sources_array_json: &str,
    mut next_entry_json: impl FnMut() -> String,
) -> Result<(), String> {
    if n_entries > MAX_CATALOG_ENTRIES {
        return Err(format!(
            "当前轮可搜索目录 {n_entries} 项，超过 {MAX_CATALOG_ENTRIES} 项上限（跨服务器合计）"
        ));
    }
    let mut bytes = "{\"entries\":[".len();
    for i in 0..n_entries {
        let serialized = next_entry_json();
        bytes += serialized.len() + usize::from(i + 1 < n_entries);
        if bytes > MAX_CATALOG_JSON_BYTES {
            return Err(format!(
                "当前轮可搜索目录容量超过 10 MiB（{MAX_CATALOG_JSON_BYTES} 字节）上限"
            ));
        }
    }
    bytes += "],\"sources\":".len() + sources_array_json.len() + 1;
    if bytes > MAX_CATALOG_JSON_BYTES {
        return Err(format!(
            "当前轮可搜索目录容量超过 10 MiB（{MAX_CATALOG_JSON_BYTES} 字节）上限"
        ));
    }
    Ok(())
}

/// 一轮用户轮的搜索执行状态（turn 层持有；轮结束丢弃——不跨轮、不进
/// 快照）。条目在轮开始收集并完成容量检查；BM25 索引惰性建立（首次搜索
/// 时把条目**移入**索引拟合，同轮仅一次、零克隆）；`loaded` 为累计已加载
/// 条目（按自身条目序——与快照条目序无关，敏感排除不改变对应关系）。
pub struct SearchState {
    /// 仅在索引建立前持有（建立时整体移入 `index`）。
    entries: Option<Vec<CatalogEntry>>,
    index: Option<TurnCatalog>,
    /// 累计已加载条目下标（**升序维护**＝目录/快照身份序；声明顺序由它
    /// 直接投影，§1.4-2）。单账本：名字集合由下标投影，不再另存一份。
    loaded: Vec<usize>,
}

impl SearchState {
    pub fn new(entries: Vec<CatalogEntry>) -> Self {
        Self {
            entries: Some(entries),
            index: None,
            loaded: Vec::new(),
        }
    }

    /// 累计已加载条目的完整定义（升序下标＝快照身份序；下标在自身条目
    /// 空间内，敏感排除不影响对应关系）。
    pub fn loaded_definitions(&self) -> Vec<Value> {
        let entries = self.loaded_entries();
        self.loaded
            .iter()
            .map(|&i| entries[i].definition())
            .collect()
    }

    /// 已加载模型名集合（由 `loaded` 下标投影——单账本派生，本次请求的
    /// 可执行动态集合）。
    pub fn loaded_names(&self) -> std::collections::HashSet<String> {
        self.loaded_entries()
            .iter()
            .enumerate()
            .filter(|(i, _)| self.loaded.binary_search(i).is_ok())
            .map(|(_, entry)| entry.model_name.clone())
            .collect()
    }

    fn loaded_entries(&self) -> &[CatalogEntry] {
        self.index
            .as_ref()
            .map(|catalog| catalog.entries())
            .or_else(|| self.entries.as_deref())
            .expect("条目必在索引或待建列表之一")
    }

    fn catalog(&mut self) -> &TurnCatalog {
        if self.index.is_none() {
            let entries = self.entries.take().expect("条目仅在索引建立前持有");
            self.index = Some(build_catalog(entries));
        }
        self.index.as_ref().expect("已建立索引")
    }

    /// 执行一次搜索：本地检索→结果容量→声明预算→提交加载集合。全部成功
    /// 或全部失败；此前成功加载不回滚。`native_declared`＝本次请求中非
    /// 动态已加载部分的声明数（内置＋条件原生＋tool_search＋资源工具）。
    /// 提交前检查取消：已取消不发布迟到结果、不加载新增定义。
    pub fn execute_search(
        &mut self,
        query: &str,
        limit: usize,
        native_declared: usize,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Value {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst)) {
            return failure_result("搜索已取消");
        }
        // 检索（带下标）与结果构造在索引借用内完成；命中下标直接来自
        // 检索结果，不再按名字回扫目录（审查 #6）。
        let (result, hit_indices): (Value, Vec<usize>) = {
            let catalog = self.catalog();
            let hits = catalog.search_indexed(query, limit);
            let refs: Vec<&CatalogEntry> = hits.iter().map(|(_, entry)| *entry).collect();
            match success_result(&refs) {
                Ok(result) => (result, hits.into_iter().map(|(i, _)| i).collect()),
                Err(_) => return failure_result("搜索结果超过 1 MiB 上限：请缩小 limit 后重试"),
            }
        };
        // 新颖性＝不在已加载下标集合（单账本一次判定；重复命中不占预算）。
        let new_indices: Vec<usize> = hit_indices
            .into_iter()
            .filter(|&i| self.loaded.binary_search(&i).is_err())
            .collect();
        // 声明预算（§1.6）：常驻＋tool_search＋累计加载 ≤128；超限本次搜索
        // 失败，不自动淘汰已加载项。
        let projected = native_declared + self.loaded.len() + new_indices.len();
        if projected > crate::mcp::config::MAX_TOTAL_TOOLS {
            return failure_result(&format!(
                "累计声明工具数将达 {projected}，超过 {} 上限：请缩小 limit 或在新用户轮处理",
                crate::mcp::config::MAX_TOTAL_TOOLS
            ));
        }
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst)) {
            return failure_result("搜索已取消");
        }
        // 提交：返回的定义与实际加入加载集合的定义完全相符；升序插入维持
        // 快照身份序（§1.4-2，审查 #2）。
        for i in new_indices {
            let pos = self.loaded.partition_point(|&x| x < i);
            self.loaded.insert(pos, i);
        }
        result
    }
}

/// 展示行（M5/S16）：query、命中数与模型工具名；失败显示具体原因。
/// 展示副本由宿主统一终端过滤；不回显完整 Schema。
pub fn search_display_line(arguments: &str, result: &str) -> String {
    let query = serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|v| {
            v.get("query")
                .and_then(|q| q.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let parsed: Value = serde_json::from_str(result).unwrap_or(Value::Null);
    if parsed["ok"] == json!(true) {
        let names: Vec<&str> = parsed["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t["function"]["name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        let mut line = format!(
            "[工具] tool_search「{query}」命中 {} 项：{}",
            names.len(),
            names.join("、")
        );
        if line.chars().count() > 160 {
            line = line.chars().take(160).collect::<String>() + "…";
        }
        line
    } else {
        let reason = parsed["error"].as_str().unwrap_or("结果不可解析");
        format!("[工具] tool_search「{query}」失败：{reason}")
    }
}
