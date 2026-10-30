//! 配置与模型目录：结构定义、内置资料、合并规则、凭证指纹与文件保存。
//! 规则来源：docs/specs/002-llm-api-key.md

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 临时文件序号：同进程内保证唯一命名。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 测试接缝（仅单测）：固定 tmp 名的纳秒与序号，用于确定性预置冲突路径。
#[cfg(test)]
static TEST_TMP_PIN: std::sync::Mutex<Option<(u32, u64)>> = std::sync::Mutex::new(None);

/// 当前实际支持的协议；未支持的协议明确报错。
pub const SUPPORTED_PROTOCOL: &str = "openai-chat";

// ---------- 数据结构 ----------

/// 模型记录：能力查不到就留空表示未知，不按模型名称猜。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    /// 手动确认支持的请求参数名；None 表示能力未知。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_params: Option<Vec<String>>,
    /// 019：输入模态（仅 `text`/`image` 子集）。None＝未知（关闭 view_image）；
    /// 只含 `text` 同样关闭。用户在模型配置对象内手动确认，不按名称猜测。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "parse_input_modalities"
    )]
    pub input_modalities: Option<Vec<String>>,
    /// 021：模型目录显式声明的实验性支持工具（精确名匹配）。None/空＝不启用；
    /// 其他名称不自动获得本地执行权限，也不注册未知工具。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental_supported_tools: Option<Vec<String>>,
}

/// 019：模态字段解析——未知值（如 audio）按配置格式错误拒绝，不静默忽略。
fn parse_input_modalities<'de, D>(d: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Vec<String>>::deserialize(d)?;
    match raw {
        None => Ok(None),
        Some(list) => {
            for m in &list {
                if m != "text" && m != "image" {
                    return Err(serde::de::Error::custom(format!(
                        "input_modalities 只支持 text/image，未知值 {m}"
                    )));
                }
            }
            Ok(Some(list))
        }
    }
}

/// 手动配置：铺开保存，只写需要覆盖或新增的内容。
/// 未知字段一律报错：拼错字段名（如 base_ur）不能被静默忽略。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ManualRoot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, ManualProvider>,
    /// 013-M1（D5）：记忆配置段；缺省＝未配置（默认关闭），旧配置零改动兼容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memories: Option<crate::memory::MemoriesConfig>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ManualProvider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_params: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "map_is_empty")]
    pub models: BTreeMap<String, ModelInfo>,
}

fn map_is_empty(m: &BTreeMap<String, ModelInfo>) -> bool {
    m.is_empty()
}

/// 自动目录：内置资料/公共目录写 base，厂商查询写 query，分别保存。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct AutoCatalog {
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub providers: BTreeMap<String, AutoProvider>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct AutoProvider {
    #[serde(default)]
    pub base: BaseData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<QueryList>,
}

/// 基础资料：来自内置资料或公共目录同步。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct BaseData {
    pub source: String,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_ref: Option<String>,
    #[serde(default, skip_serializing_if = "base_models_is_empty")]
    pub models: BTreeMap<String, ModelInfo>,
}

fn base_models_is_empty(m: &BTreeMap<String, ModelInfo>) -> bool {
    m.is_empty()
}

/// 查询名单：绑定查询时的连接身份，可为空名单。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct QueryList {
    pub updated_at: u64,
    pub identity: Identity,
    pub models: Vec<ModelInfo>,
}

/// 连接身份：地址、协议、凭证名＋实际凭证的本地不可逆指纹。
/// 凭证名只负责定位凭证；同名引用下换了 Key，指纹变化，身份即失效。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub base_url: String,
    pub protocol: String,
    pub cred_name: String,
    pub cred_fp: String,
}

/// 合并后的生效连接信息（合并规则第 1 步的产物）。
#[derive(Clone, Debug, Default)]
pub struct EffectiveConnection {
    pub base_url: String,
    pub protocol: String,
    pub key_ref: Option<String>,
    pub request_params: BTreeMap<String, serde_json::Value>,
}

/// 凭证读取结果：名字始终可解析，值可能缺失。
/// 手动实现 Debug：任何调试输出都不得包含凭证明文。
#[derive(Clone)]
pub struct CredOutcome {
    pub name: String,
    pub value: Option<String>,
    pub error: Option<String>,
}

impl std::fmt::Debug for CredOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredOutcome")
            .field("name", &self.name)
            .field("value", &self.value.as_ref().map(|_| "***"))
            .field("error", &self.error)
            .finish()
    }
}

#[derive(Clone)]
pub struct Credential {
    pub name: String,
    pub value: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("name", &self.name)
            .field("value", &"***")
            .finish()
    }
}

/// 一次调用的配置快照：调用开始时固定，目录刷新不影响。
#[derive(Clone, Debug)]
pub struct Resolved {
    pub provider_id: String,
    pub conn: EffectiveConnection,
    pub credential: Option<Credential>,
    pub credential_error: Option<String>,
    pub identity: Identity,
    pub models: BTreeMap<String, ModelInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct FinalDict {
    pub providers: BTreeMap<String, Resolved>,
    pub selected_provider: Option<String>,
    pub selected_model: Option<String>,
}

// ---------- 凭证与指纹 ----------

/// 信任根控制变量：目录解析与公共目录地址的运行时入口（含默认目录回退）。
/// 只允许来自可信的启动环境——项目 .env 是不可信输入（可能随克隆仓库携带），
/// 不得经它重定位配置/授权记录/凭证根或目录来源（2026-10-09 安全报告 F1）。
/// 名单来自全库 env 读取点审计：`config_dir`、`user_dir`、公共目录 URL 回退。
/// 注意不能用 `TCHAT_` 前缀一刀切：普通凭证变量允许从 .env 加载。
const TRUST_CONTROL_VARS: [&str; 9] = [
    "TCHAT_CONFIG_DIR",
    "TCHAT_USER_DIR",
    "TCHAT_PUBLIC_CATALOG_URL",
    // 033：沙箱模式只能由管理员 wrapper（真实环境）启用；.env 不是信任根，
    // 不得借它开启沙箱（沿用 027 安全攻防 F2 的信任根结论）
    "TCHAT_SANDBOX",
    // 031：可信策略路径只能来自可信启动环境；.env 不得启用、关闭或改换策略
    "TCHAT_PERMISSION_POLICY",
    // 007 会话时钟/pid 注入提供者：仅测试子进程经真实环境注入；.env 固定时间戳
    // 会破坏 auto 文件名"字典序＝时间序"的最近会话选择，按 F1 先例跳过
    "TCHAT_SESSION_TS",
    "TCHAT_SESSION_PID",
    "HOME",
    "APPDATA",
];

/// 从 .env 文件读取 KEY=VALUE 写入进程环境；已存在的变量不覆盖；
/// [信任根控制变量](TRUST_CONTROL_VARS)一律跳过。文件缺失或格式异常静默跳过：
/// .env 是环境变量的可选补充来源，不是信任根。
pub fn load_dotenv_from(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else { return 0 };
    let mut loaded = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let k = k.trim();
        // eq_ignore_ascii_case：Windows 环境变量名不区分大小写，小写变体同样能命中
        // env::var("TCHAT_CONFIG_DIR") 等读取点，必须一并拒绝（安全审查必修项）
        if k.is_empty()
            || TRUST_CONTROL_VARS.iter().any(|t| k.eq_ignore_ascii_case(t))
            || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let mut v = v.trim();
        if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\'')))
        {
            v = &v[1..v.len() - 1];
        } else if let Some(idx) = v.find(" #") {
            v = v[..idx].trim_end();
        }
        if v.is_empty() || std::env::var_os(k).is_some() {
            continue;
        }
        std::env::set_var(k, v);
        loaded += 1;
    }
    loaded
}

/// 实际凭证的本地不可逆指纹（SHA-256 前 16 位十六进制）。
pub fn fingerprint(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_string()
}

/// 解析凭证引用：env:名称 读环境变量；file:路径#名称 读 JSON 凭证文件字段。
/// 值缺失时不中断合并，错误留给请求前的路由检查报告。
pub fn resolve_credential(key_ref: &str) -> CredOutcome {
    let bad = |msg: String| CredOutcome { name: String::new(), value: None, error: Some(msg) };
    let Some((scheme, rest)) = key_ref.split_once(':') else {
        return bad("凭证引用格式不支持，应为 env:名称 或 file:路径#名称".into());
    };
    match scheme {
        "env" => {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return bad("env 引用缺少环境变量名".into());
            }
            match std::env::var(&name) {
                Ok(v) if !v.is_empty() => CredOutcome { name: name.clone(), value: Some(v), error: None },
                Ok(_) => CredOutcome { name: name.clone(), value: None, error: Some(format!("环境变量 {name} 存在但为空")) },
                Err(_) => CredOutcome { value: None, error: Some(format!("环境变量 {name} 未设置（可在项目根目录 .env 中配置）")), name },
            }
        }
        "file" => {
            let Some((path, name)) = rest.rsplit_once('#') else {
                return bad("file 引用缺少 #名称，应为 file:路径#名称".into());
            };
            let (path, name) = (path.trim(), name.trim());
            if path.is_empty() || name.is_empty() {
                return bad("file 引用的路径或名称为空".into());
            }
            // 安全边界：凭证文件只能位于专用凭证目录内。
            // 用真实路径（canonicalize，解析软链接与 ..）做包含性校验，不能只比较字符串前缀。
            let dir = credential_dir();
            let dir_real = match std::fs::canonicalize(&dir) {
                Ok(d) => d,
                Err(_) => {
                    return CredOutcome {
                        name: name.into(),
                        value: None,
                        error: Some(format!(
                            "专用凭证目录 {} 不存在：可将凭证文件移动到该目录后，改用 file:文件名#字段 引用",
                            dir.display()
                        )),
                    }
                }
            };
            let requested = Path::new(path);
            let target = if requested.is_relative() {
                dir.join(requested)
            } else {
                requested.to_path_buf()
            };
            let real = match std::fs::canonicalize(&target) {
                Ok(p) => p,
                Err(e) => {
                    return CredOutcome {
                        name: name.into(),
                        value: None,
                        error: Some(format!(
                            "凭证文件 {path} 读取失败：{e}；目录内文件可用相对路径 file:文件名#字段 引用"
                        )),
                    }
                }
            };
            if !real.starts_with(&dir_real) {
                return CredOutcome {
                    name: name.into(),
                    value: None,
                    error: Some(format!(
                        "凭证文件必须位于专用凭证目录 {} 内（路径穿越或软链接逃逸已拦截）；迁移方式：将 {path} 移动到该目录后改用 file:文件名#字段",
                        dir.display()
                    )),
                };
            }
            let read = std::fs::read_to_string(&real)
                .map_err(|e| format!("凭证文件 {} 读取失败：{e}", real.display()))
                .and_then(|text| {
                    serde_json::from_str::<serde_json::Value>(&text)
                        .map_err(|e| format!("凭证文件 {} 不是有效 JSON：{e}", real.display()))
                })
                .and_then(|v| {
                    v.get(name)
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string())
                        .ok_or_else(|| format!("凭证文件 {} 中没有字段 {name}", real.display()))
                });
            match read {
                Ok(v) if v.is_empty() => CredOutcome { name: name.into(), value: None, error: Some(format!("凭证文件 {path} 中 {name} 为空")) },
                Ok(v) => CredOutcome { name: name.into(), value: Some(v), error: None },
                Err(e) => CredOutcome { name: name.into(), value: None, error: Some(e) },
            }
        }
        other => bad(format!("凭证引用前缀 {other}: 不支持，应为 env:名称 或 file:路径#名称")),
    }
}

fn identity_of(conn: &EffectiveConnection, cred: &CredOutcome) -> Identity {
    Identity {
        base_url: conn.base_url.clone(),
        protocol: conn.protocol.clone(),
        cred_name: cred.name.clone(),
        cred_fp: cred.value.as_deref().map(fingerprint).unwrap_or_default(),
    }
}

/// 校验生效地址：必须是绝对 http(s) 地址；远程主机要求 HTTPS，
/// 仅本机地址（127.0.0.1/localhost/::1）允许 HTTP 供本地测试；不做域名白名单。
/// 合法的 HTTPS 只代表传输加密，不代表域名可信或没有拼错。
pub fn validate_base_url(base_url: &str) -> Result<(), String> {
    if base_url.trim().is_empty() {
        return Err("请求地址为空：请在手动配置中填写 base_url".into());
    }
    let parsed = url::Url::parse(base_url).map_err(|_| {
        if base_url.contains("://") {
            format!("请求地址无效：{base_url:?}")
        } else {
            format!("请求地址缺少 http(s):// 前缀：{base_url:?}")
        }
    })?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = parsed
                .host_str()
                .unwrap_or_default()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_ascii_lowercase();
            if matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
                Ok(())
            } else {
                Err(format!("远程地址必须使用 HTTPS：{base_url}"))
            }
        }
        other => {
            if base_url.contains("://") {
                Err(format!("不支持的地址协议 {other}:，仅支持 http/https"))
            } else {
                Err(format!("请求地址缺少 http(s):// 前缀：{base_url:?}"))
            }
        }
    }
}

// ---------- 凭证外发授权（F1） ----------

/// 授权记录：把凭证（不可逆指纹）绑定到已授权的出站地址 origin。
/// 与凭证指纹绑定：换了 Key 即视为新授权，防止旧授权被静默沿用。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OriginGrant {
    pub origin: String,
    pub cred_fp: String,
    pub added_at: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct OriginStore {
    grants: Vec<OriginGrant>,
}

/// 授权记录存放位置：用户私有目录，项目配置（manual.json）无权自行授予。
pub fn authorized_origins_path() -> PathBuf {
    user_dir().join("authorized-origins.json")
}

fn load_origin_store() -> OriginStore {
    // 读取/解析失败一律按"无授权"处理（fail-closed）
    std::fs::read_to_string(authorized_origins_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn is_origin_authorized(origin: &str, cred_fp: &str) -> bool {
    load_origin_store()
        .grants
        .iter()
        .any(|g| g.origin == origin && g.cred_fp == cred_fp)
}

/// 记录一条授权：写入用户私有目录（0600），写失败即报错（fail-closed）。
pub fn grant_origin(origin: &str, cred_fp: &str) -> Result<(), String> {
    let mut store = load_origin_store();
    if store.grants.iter().any(|g| g.origin == origin && g.cred_fp == cred_fp) {
        return Ok(());
    }
    store.grants.push(OriginGrant {
        origin: origin.into(),
        cred_fp: cred_fp.into(),
        added_at: now_secs(),
    });
    let content = serde_json::to_string_pretty(&store)
        .map_err(|e| format!("授权记录序列化失败：{e}"))?;
    // 2026-10-10 整改：授权记录改用 tmp＋rename 原子替换（失败保留旧授权文件完整；
    // 并发末者胜完整快照；保留已接受的读改写条目丢失）。目录私有性与最终 0600
    // 分别由 ensure_private_dir 与 write_atomic 的创建参数保证。
    let dir = user_dir();
    ensure_private_dir(&dir)?;
    write_atomic(&dir, "authorized-origins.json", &content)
}

/// 出站 origin：scheme://host[:port]，来自实际请求地址，与凭证绑定后存入授权记录。
pub fn origin_of(base_url: &str) -> Result<String, String> {
    let u = url::Url::parse(base_url).map_err(|e| format!("请求地址无效：{e}"))?;
    match u.origin() {
        url::Origin::Tuple(_, _, _) => Ok(u.origin().ascii_serialization()),
        url::Origin::Opaque(_) => Err(format!("请求地址 {base_url} 不支持来源授权")),
    }
}

/// 请求前授权检查：已授权放行；未授权时——
/// 交互模式询问用户（同意则写入授权记录），非交互模式直接拒绝（不发凭证）。
pub fn ensure_origin_authorized(
    origin: &str,
    cred_name: &str,
    cred_fp: &str,
    interactive: bool,
    prompt: &mut dyn FnMut(&str) -> bool,
) -> Result<(), String> {
    if is_origin_authorized(origin, cred_fp) {
        return Ok(());
    }
    if !interactive {
        return Err(format!(
            "凭证 {cred_name} 未授权发送到 {origin}：非交互模式不做确认，已拒绝发送。\
             可在交互模式运行确认，或预先在 {} 写入授权记录",
            authorized_origins_path().display()
        ));
    }
    let question = format!(
        "即将把凭证 {cred_name}（指纹 {}）发送到 {origin}。允许并记住此授权？(y/N) ",
        &cred_fp[..cred_fp.len().min(8)]
    );
    if prompt(&question) {
        grant_origin(origin, cred_fp)?;
        Ok(())
    } else {
        Err(format!("已拒绝将凭证 {cred_name} 发送到 {origin}"))
    }
}

// ---------- 公共目录来源授权（F4） ----------

/// 无凭证来源（公共目录）在授权记录中的固定指纹标记。
pub const CATALOG_CRED_MARK: &str = "catalog";

pub fn is_catalog_origin_authorized(origin: &str) -> bool {
    is_origin_authorized(origin, CATALOG_CRED_MARK)
}

pub fn grant_catalog_origin(origin: &str) -> Result<(), String> {
    grant_origin(origin, CATALOG_CRED_MARK)
}

/// 公共目录来源授权检查：语义与凭证授权一致（非交互未授权即拒绝，fail-closed）。
pub fn ensure_catalog_origin_authorized(
    origin: &str,
    interactive: bool,
    prompt: &mut dyn FnMut(&str) -> bool,
) -> Result<(), String> {
    if is_catalog_origin_authorized(origin) {
        return Ok(());
    }
    if !interactive {
        return Err(format!(
            "公共目录来源 {origin} 未授权：非交互模式不做确认，已跳过同步（沿用旧目录）。\
             可在交互模式运行确认，或预先在 {} 写入授权记录（cred_fp=\"catalog\"）",
            authorized_origins_path().display()
        ));
    }
    let question = format!("即将从 {origin} 同步公共目录基础数据（不含凭证）。允许并记住？(y/N) ");
    if prompt(&question) {
        grant_catalog_origin(origin)?;
        Ok(())
    } else {
        Err(format!("已拒绝从 {origin} 同步公共目录"))
    }
}

// ---------- 内置资料 ----------

/// 内置兜底资料：能力字段来自厂商接口返回，未知就留空。
pub fn builtin() -> BTreeMap<String, AutoProvider> {
    let mut providers = BTreeMap::new();
    providers.insert(
        "deepseek".to_string(),
        AutoProvider {
            base: BaseData {
                source: "builtin".into(),
                updated_at: 0,
                base_url: Some("https://api.deepseek.com".into()),
                protocol: Some(SUPPORTED_PROTOCOL.into()),
                key_ref: Some("env:DEEPSEEK_API_KEY".into()),
                models: BTreeMap::from([
                    (
                        "deepseek-flash".to_string(),
                        ModelInfo {
                            id: "deepseek-flash".into(),
                            display_name: Some("DeepSeek-V4.1-Flash".into()),
                            context_length: Some(1_048_576),
                            max_output: Some(393_216),
                            ..Default::default()
                        },
                    ),
                    (
                        "deepseek-v4-pro".to_string(),
                        ModelInfo {
                            id: "deepseek-v4-pro".into(),
                            display_name: Some("DeepSeek-V4-Pro".into()),
                            context_length: Some(1_048_576),
                            max_output: Some(393_216),
                            ..Default::default()
                        },
                    ),
                ]),
            },
            query: None,
        },
    );
    providers.insert(
        "openai".to_string(),
        AutoProvider {
            base: BaseData {
                source: "builtin".into(),
                updated_at: 0,
                base_url: Some("https://api.openai.com/v1".into()),
                protocol: Some(SUPPORTED_PROTOCOL.into()),
                key_ref: Some("env:OPENAI_API_KEY".into()),
                models: BTreeMap::new(),
            },
            query: None,
        },
    );
    providers
}

// ---------- 合并 ----------

/// 步骤 1：合并厂商连接信息（内置资料 → 自动目录基础资料 → 手动覆盖）。
pub fn effective_connection(
    manual: &ManualRoot,
    auto: &AutoCatalog,
    id: &str,
) -> EffectiveConnection {
    let builtin = builtin();
    effective_connection_with(&builtin, manual, auto, id)
}

fn effective_connection_with(
    builtin: &BTreeMap<String, AutoProvider>,
    manual: &ManualRoot,
    auto: &AutoCatalog,
    id: &str,
) -> EffectiveConnection {
    let man = manual.providers.get(id);
    let auto_base = auto.providers.get(id).map(|p| &p.base);
    let builtin_base = builtin.get(id).map(|p| &p.base);
    let first = |a: Option<&String>, b: Option<&String>, c: Option<&String>| {
        a.cloned().or_else(|| b.cloned()).or_else(|| c.cloned())
    };
    let base_url = first(
        man.and_then(|m| m.base_url.as_ref()),
        auto_base.and_then(|b| b.base_url.as_ref()),
        builtin_base.and_then(|b| b.base_url.as_ref()),
    )
    .unwrap_or_default();
    let protocol = first(
        man.and_then(|m| m.protocol.as_ref()),
        auto_base.and_then(|b| b.protocol.as_ref()),
        builtin_base.and_then(|b| b.protocol.as_ref()),
    )
    .unwrap_or_default();
    let key_ref = first(
        man.and_then(|m| m.key_ref.as_ref()),
        auto_base.and_then(|b| b.key_ref.as_ref()),
        builtin_base.and_then(|b| b.key_ref.as_ref()),
    );
    let mut request_params = BTreeMap::new();
    if let Some(mp) = man.and_then(|m| m.request_params.as_ref()) {
        request_params.extend(mp.clone());
    }
    EffectiveConnection { base_url, protocol, key_ref, request_params }
}

/// 字段级叠加：只覆盖来源里明确写出的字段。
fn overlay_model(dst: &mut ModelInfo, src: &ModelInfo) {
    if src.display_name.is_some() {
        dst.display_name = src.display_name.clone();
    }
    if src.disabled.is_some() {
        dst.disabled = src.disabled;
    }
    if src.context_length.is_some() {
        dst.context_length = src.context_length;
    }
    if src.max_output.is_some() {
        dst.max_output = src.max_output;
    }
    if src.supported_params.is_some() {
        dst.supported_params = src.supported_params.clone();
    }
    if src.input_modalities.is_some() {
        dst.input_modalities = src.input_modalities.clone();
    }
    if src.experimental_supported_tools.is_some() {
        dst.experimental_supported_tools = src.experimental_supported_tools.clone();
    }
    dst.id = src.id.clone();
}

fn base_models_of(
    builtin: &BTreeMap<String, AutoProvider>,
    auto: &AutoCatalog,
    id: &str,
) -> BTreeMap<String, ModelInfo> {
    let mut models: BTreeMap<String, ModelInfo> = builtin
        .get(id)
        .map(|p| p.base.models.clone())
        .unwrap_or_default();
    if let Some(entry) = auto.providers.get(id) {
        for (mid, m) in &entry.base.models {
            let mut info = models.get(mid).cloned().unwrap_or_default();
            overlay_model(&mut info, m);
            models.insert(mid.clone(), info);
        }
    }
    models
}

/// 按文档顺序合成最终字典（内存中生成，不落盘）：
/// 1. 合并连接信息；2. 按身份筛选自动查询名单；3. 合并模型信息并叠加手动覆盖。
pub fn merge_final(manual: &ManualRoot, auto: &AutoCatalog) -> FinalDict {
    let builtin = builtin();
    let ids: BTreeSet<String> = builtin
        .keys()
        .chain(auto.providers.keys())
        .chain(manual.providers.keys())
        .cloned()
        .collect();
    let mut providers = BTreeMap::new();
    for id in ids {
        let conn = effective_connection_with(&builtin, manual, auto, &id);
        let cred = conn.key_ref.as_deref().map(resolve_credential);
        let identity = match &cred {
            Some(c) => identity_of(&conn, c),
            None => Identity {
                base_url: conn.base_url.clone(),
                protocol: conn.protocol.clone(),
                cred_name: String::new(),
                cred_fp: String::new(),
            },
        };
        // 步骤 2：只保留身份一致的查询名单；不一致的旧名单排除。
        let valid_query = auto
            .providers
            .get(&id)
            .and_then(|p| p.query.as_ref())
            .filter(|q| q.identity == identity);
        // 步骤 3：名单有效用名单，失效或缺失退回基础资料；再叠加手动覆盖。
        let mut models = match valid_query {
            Some(q) => {
                let base = base_models_of(&builtin, auto, &id);
                let mut models = BTreeMap::new();
                for m in &q.models {
                    let mut info = base.get(&m.id).cloned().unwrap_or_default();
                    overlay_model(&mut info, m);
                    models.insert(m.id.clone(), info);
                }
                models
            }
            None => base_models_of(&builtin, auto, &id),
        };
        if let Some(man) = manual.providers.get(&id) {
            for (mid, ov) in &man.models {
                let mut info = models.get(mid).cloned().unwrap_or_default();
                overlay_model(&mut info, ov);
                info.id = mid.clone();
                models.insert(mid.clone(), info);
            }
        }
        providers.insert(
            id.clone(),
            Resolved {
                provider_id: id.clone(),
                conn,
                credential: cred.as_ref().and_then(|c| {
                    c.value
                        .clone()
                        .map(|value| Credential { name: c.name.clone(), value })
                }),
                credential_error: cred.as_ref().and_then(|c| c.error.clone()),
                identity,
                models,
            },
        );
    }
    FinalDict {
        providers,
        selected_provider: manual.selected_provider.clone(),
        selected_model: manual.selected_model.clone(),
    }
}

// ---------- 最终字典 ----------

/// 从配置目录加载手动配置与自动目录，在内存中合成最终字典。
pub fn build_final() -> Result<FinalDict, String> {
    let dir = config_dir();
    let manual = load_manual(&dir)?;
    let auto = load_auto(&dir)?;
    Ok(merge_final(&manual, &auto))
}

// ---------- 请求路由 ----------

/// 请求前路由检查：密钥能读到、模型未禁用；返回本次调用的固定快照。
/// 通过后由调用方发起请求；失败原因明确说明。
pub fn route(dict: &FinalDict) -> Result<(Resolved, Credential, ModelInfo), String> {
    let pid = dict
        .selected_provider
        .clone()
        .ok_or("手动配置未设置 selected_provider")?;
    let mid = dict
        .selected_model
        .clone()
        .ok_or("手动配置未设置 selected_model")?;
    let prov = dict
        .providers
        .get(&pid)
        .ok_or_else(|| format!("selected_provider {pid} 不在最终字典中"))?;
    let model = prov.models.get(&mid).ok_or_else(|| {
        format!("模型 {mid} 不在厂商 {pid} 的有效名单中，可运行 /refresh 或在手动配置新增")
    })?;
    if model.disabled == Some(true) {
        return Err(format!("模型 {mid} 已禁用（disabled = true）"));
    }
    validate_base_url(&prov.conn.base_url)?;
    let cred = prov.credential.clone().ok_or_else(|| {
        prov.credential_error
            .clone()
            .unwrap_or_else(|| "未配置密钥引用 key_ref".into())
    })?;
    Ok((prov.clone(), cred, model.clone()))
}

// ---------- 参数能力 ----------

/// 019：模型图片输入能力判断——只有用户在配置中明确声明 `input_modalities`
/// 含 `image` 才开放（缺字段＝未知、只 text＝关闭，均返回 false）；
/// 不按模型名称猜测、不做网络探测。
pub fn supports_image_input(model: &ModelInfo) -> bool {
    model
        .input_modalities
        .as_ref()
        .is_some_and(|list| list.iter().any(|m| m == "image"))
}

/// 021：模型目录显式声明 send_message_to_user_async——唯一开关来源
///（`experimental_supported_tools` 精确匹配该工具名）；缺省/空数组/只含
/// 其他名称均关闭。不按模型名称猜测、不串用问询能力、不做网络探测。
pub fn supports_send_message_to_user(model: &ModelInfo) -> bool {
    model
        .experimental_supported_tools
        .as_ref()
        .is_some_and(|list| list.iter().any(|t| t == crate::message_to_user::TOOL_NAME))
}

/// 032：experimental_supported_tools 精确匹配声明 get_context_remaining
/// （声明与执行共用同一快照；缺省/空数组/只含其他名＝不声明）。
pub fn supports_get_context_remaining(model: &ModelInfo) -> bool {
    model
        .experimental_supported_tools
        .as_ref()
        .is_some_and(|list| {
            list.iter()
                .any(|t| t == crate::context_window::QUERY_TOOL_NAME)
        })
}

/// 032：experimental_supported_tools 精确匹配声明 new_context。
pub fn supports_new_context(model: &ModelInfo) -> bool {
    model
        .experimental_supported_tools
        .as_ref()
        .is_some_and(|list| {
            list.iter().any(|t| t == crate::context_window::SWITCH_TOOL_NAME)
        })
}

/// 030：experimental_supported_tools 精确匹配声明 tool_search（§1.2：唯一
/// 开关来源；缺省/空数组/只含其他名称均保留原全量声明路径，不按模型名或
/// 协议名猜测）。
pub fn supports_tool_search(model: &ModelInfo) -> bool {
    model
        .experimental_supported_tools
        .as_ref()
        .is_some_and(|list| {
            list.iter()
                .any(|t| t == crate::tool_search::TOOL_NAME)
        })
}

/// 034：exec/wait 双 opt-in——`experimental_supported_tools` 必须同时显式含
/// 精确名称 `exec` 与 `wait` 才开放这一对工具；缺省、空数组、只含其一或
/// 只含其他名称均关闭。
pub fn supports_js_orchestration(model: &ModelInfo) -> bool {
    let list = model.experimental_supported_tools.as_deref().unwrap_or(&[]);
    let has_exec = list.iter().any(|t| t == crate::js_orchestration::EXEC_TOOL_NAME);
    let has_wait = list.iter().any(|t| t == crate::js_orchestration::WAIT_TOOL_NAME);
    has_exec && has_wait
}

/// 034：只配了一个时返回缺失的精确名（宿主给一次配置提示用；两者都缺或
/// 都在时返回 None）。
pub fn js_orchestration_missing(model: &ModelInfo) -> Option<&'static str> {
    let list = model.experimental_supported_tools.as_deref().unwrap_or(&[]);
    let has_exec = list.iter().any(|t| t == crate::js_orchestration::EXEC_TOOL_NAME);
    let has_wait = list.iter().any(|t| t == crate::js_orchestration::WAIT_TOOL_NAME);
    match (has_exec, has_wait) {
        (true, true) | (false, false) => None,
        (true, false) => Some(crate::js_orchestration::WAIT_TOOL_NAME),
        (false, true) => Some(crate::js_orchestration::EXEC_TOOL_NAME),
    }
}

/// 035：六控制工具整组 opt-in——`experimental_supported_tools` 须同时显式
/// 含全部六个精确名称（spawn_agent/send_message/followup_task/wait_agent/
/// interrupt_agent/list_agents）才开放；缺任一即整组关闭（§1.2：只配部分
/// 时整组关闭并提示一次，不按子集开放）。
pub fn supports_multi_agent(model: &ModelInfo) -> bool {
    let list = model.experimental_supported_tools.as_deref().unwrap_or(&[]);
    crate::agents::TOOL_NAMES
        .iter()
        .all(|name| list.iter().any(|t| t == *name))
}

/// 035：返回缺失的精确名列表（部分配置时宿主提示一次；全配或全缺为空）。
pub fn multi_agent_missing(model: &ModelInfo) -> Vec<&'static str> {
    let list = model.experimental_supported_tools.as_deref().unwrap_or(&[]);
    let missing: Vec<&'static str> = crate::agents::TOOL_NAMES
        .iter()
        .filter(|name| !list.iter().any(|t| t == *name))
        .copied()
        .collect();
    // 全缺＝未配置该能力（不提示）；部分缺＝配置不完整（提示缺失名）。
    if missing.len() == crate::agents::TOOL_NAMES.len() {
        Vec::new()
    } else {
        missing
    }
}

/// 031：experimental_supported_tools 精确匹配声明 request_permissions（§1.2-4
/// 的模型能力半边；实际声明还要求 031 冻结策略与专用交互宿主——见 turn 装配）。
pub fn supports_request_permissions(model: &ModelInfo) -> bool {
    model
        .experimental_supported_tools
        .as_ref()
        .is_some_and(|list| {
            list.iter()
                .any(|t| t == crate::permissions::TOOL_NAME)
        })
}

#[cfg(test)]
mod ts30_switch_tests {
    use super::*;

    fn model_with(tools: Option<Vec<&str>>) -> ModelInfo {
        ModelInfo {
            experimental_supported_tools: tools.map(|t| {
                t.into_iter().map(|s| s.to_string()).collect::<Vec<_>>()
            }),
            ..Default::default()
        }
    }

    /// 030：tool_search 开关——精确名匹配；缺省/空数组/其他名/近似名均关闭。
    #[test]
    fn supports_js_orchestration_dual_opt_in() {
        let model_with = |list: Option<Vec<&str>>| ModelInfo {
            id: "m".into(),
            experimental_supported_tools: list.map(|l| l.into_iter().map(String::from).collect()),
            ..Default::default()
        };
        // 双 opt-in 才开放；缺省/空/只含其一/只含其他均关闭
        assert!(supports_js_orchestration(&model_with(Some(vec!["exec", "wait"]))));
        assert!(supports_js_orchestration(&model_with(Some(vec!["wait", "exec", "other"]))));
        assert!(!supports_js_orchestration(&model_with(None)));
        assert!(!supports_js_orchestration(&model_with(Some(vec![]))));
        assert!(!supports_js_orchestration(&model_with(Some(vec!["exec"]))));
        assert!(!supports_js_orchestration(&model_with(Some(vec!["wait"]))));
        assert!(!supports_js_orchestration(&model_with(Some(vec!["exec_command", "wait"]))));
        // 部分配置提示：只配一个时返回缺失名
        assert_eq!(js_orchestration_missing(&model_with(Some(vec!["exec"]))), Some("wait"));
        assert_eq!(js_orchestration_missing(&model_with(Some(vec!["wait"]))), Some("exec"));
        assert_eq!(js_orchestration_missing(&model_with(Some(vec!["exec", "wait"]))), None);
        assert_eq!(js_orchestration_missing(&model_with(None)), None);
    }

    #[test]
    fn supports_tool_search_exact_name_only() {
        assert!(supports_tool_search(&model_with(Some(vec!["tool_search"]))));
        assert!(supports_tool_search(&model_with(Some(vec!["other", "tool_search"]))));
        assert!(!supports_tool_search(&model_with(None)));
        assert!(!supports_tool_search(&model_with(Some(vec![]))));
        assert!(!supports_tool_search(&model_with(Some(vec!["tool_searching"]))));
        assert!(!supports_tool_search(&model_with(Some(vec!["get_context_remaining"]))));
    }
}

/// 参数能力检查：能力未知不算不支持（不带可选参数的基础对话照常发送）；
/// 显式使用的参数必须先确认能力；已确认不支持的发送前报错。
pub fn check_request_param(model: &ModelInfo, param: &str) -> Result<(), String> {
    match &model.supported_params {
        None => Err(format!(
            "参数 {param} 能力未知：先在手动配置中确认模型 {} 的 supported_params 再使用",
            model.id
        )),
        Some(list) if list.iter().any(|p| p == param) => Ok(()),
        Some(_) => Err(format!("参数 {param} 已确认不被模型 {} 支持", model.id)),
    }
}

// ---------- 文件保存与加载 ----------

pub fn config_dir() -> PathBuf {
    std::env::var("TCHAT_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config"))
}

/// 用户私有目录：存放授权记录与专用凭证目录，位于项目配置之外。
/// 测试可用 TCHAT_USER_DIR 覆盖。
pub fn user_dir() -> PathBuf {
    if let Ok(d) = std::env::var("TCHAT_USER_DIR") {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    {
        std::env::var("APPDATA")
            .map(|d| PathBuf::from(d).join("terminal-chat"))
            .unwrap_or_else(|_| PathBuf::from("terminal-chat"))
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .map(|d| PathBuf::from(d).join(".config").join("terminal-chat"))
            .unwrap_or_else(|_| PathBuf::from(".terminal-chat"))
    }
}

/// 专用凭证目录：file: 凭证引用只能指向该目录内的文件。
pub fn credential_dir() -> PathBuf {
    user_dir().join("credentials")
}

// ---------- 权限边界（F7） ----------

#[cfg(unix)]
fn apply_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("设置权限失败：{e}"))?;
    // 设置后重新读取元数据确认：挂载环境可能静默忽略权限位，未生效必须如实报告
    let actual = std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .map_err(|e| format!("读取权限失败：{e}"))?;
    if actual != mode {
        return Err(format!(
            "权限设置未生效：期望 {:03o}，实际 {:03o}（当前环境可能不支持权限设置）",
            mode, actual
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    // Windows：本程序不实现 ACL 检查与调整，权限状态未知，
    // 调用方（audit_file_permissions）会明确输出"未验证"告警
    Ok(())
}

/// 实际生效的文件权限位（只读元数据，不读内容）。
#[cfg(unix)]
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Option<u32> {
    None
}

/// 创建/收紧私有目录为 0700（Unix）。设置失败时返回明确错误：
/// 挂载环境可能不支持权限位，调用方必须如实报告，不能视为已解决。
pub fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    apply_mode(dir, 0o700).map_err(|e| format!("{}：{e}", dir.display()))
}

/// 以 0600（Unix）写敏感文件：对已存在的文件也显式收紧权限（OpenOptions 的
/// mode 只在创建时生效），写后重新读取元数据确认；Windows 行为未实测。
pub fn write_private_file(path: &Path, content: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
        f.write_all(content.as_bytes())
            .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content).map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    }
    // 已存在文件的权限不会被创建参数改变：显式收紧并复读确认
    apply_mode(path, 0o600)
}

/// 启动时权限审计：.env 与配置文件尝试收紧为 0600，上级目录可写仅告警。
/// 只读文件元数据，不读取、不打印任何秘密值；不递归修改工作区。
/// Windows 平台未实现也未实测权限检查，会明确输出未验证告警。
pub fn audit_file_permissions(env_file: Option<&Path>, config_dir: &Path) -> Vec<String> {
    let mut report = Vec::new();
    #[cfg(windows)]
    report.push(
        "警告：Windows 平台的权限检查未实现且未经实测，本程序不宣称配置与凭证仅当前用户可访问；\
         请用 icacls 自行核对此目录下文件的 ACL"
            .to_string(),
    );
    if let Some(env_path) = env_file {
        if env_path.exists() {
            match file_mode(env_path) {
                Some(0o600) => {}
                Some(_) => match apply_mode(env_path, 0o600) {
                    Ok(()) => report.push(format!("已将 {} 权限收紧为 0600", env_path.display())),
                    Err(e) => report.push(format!(
                        "警告：{} 权限过宽且收紧失败（{e}）。当前环境（如挂载卷）可能不支持权限设置，明文密钥可能被同机其他用户读取，请手动收紧",
                        env_path.display()
                    )),
                },
                None => report.push(format!("警告：{} 状态无法读取", env_path.display())),
            }
        }
    }
    for name in ["manual.json", "auto_catalog.json"] {
        let p = config_dir.join(name);
        if let Some(mode) = file_mode(&p) {
            if mode != 0o600 {
                match apply_mode(&p, 0o600) {
                    Ok(()) => report.push(format!("已将 {} 权限收紧为 0600", p.display())),
                    Err(e) => {
                        report.push(format!("警告：{} 权限过宽且收紧失败（{e}）", p.display()))
                    }
                }
            }
        }
    }
    if let Some(parent) = config_dir.parent() {
        if let Some(mode) = file_mode(parent) {
            if mode & 0o002 != 0 {
                report.push(format!(
                    "警告：上级目录 {} 对其他用户可写，config 可能被整体替换；未自动修改，建议手动收紧",
                    parent.display()
                ));
            }
        }
    }
    report
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn default_manual() -> ManualRoot {
    ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: BTreeMap::from([(
            "deepseek".to_string(),
            ManualProvider {
                key_ref: Some("env:DEEPSEEK_API_KEY".into()),
                ..Default::default()
            },
        )]),
    }
}

pub fn load_manual(dir: &Path) -> Result<ManualRoot, String> {
    let path = dir.join("manual.json");
    if !path.exists() {
        let m = default_manual();
        save_manual(dir, &m)?;
        return Ok(m);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("手动配置 {} 格式错误：{e}", path.display()))
}

pub fn save_manual(dir: &Path, m: &ManualRoot) -> Result<(), String> {
    let content = serde_json::to_string_pretty(m).map_err(|e| format!("手动配置序列化失败：{e}"))?;
    write_atomic(dir, "manual.json", &content)
}

pub fn load_auto(dir: &Path) -> Result<AutoCatalog, String> {
    let path = dir.join("auto_catalog.json");
    if !path.exists() {
        let mut a = AutoCatalog { updated_at: now_secs(), providers: builtin() };
        save_auto(dir, &mut a)?;
        return Ok(a);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("自动目录 {} 损坏：{e}；可删除该文件后由内置资料重建", path.display()))
}

/// 校验后先写临时文件，成功后替换旧文件，避免中途退出损坏目录。
pub fn save_auto(dir: &Path, a: &mut AutoCatalog) -> Result<(), String> {
    a.updated_at = now_secs();
    let content =
        serde_json::to_string_pretty(a).map_err(|e| format!("自动目录序列化失败：{e}"))?;
    write_atomic(dir, "auto_catalog.json", &content)
}

/// 原子保存：在目标目录写独立临时文件（进程号＋纳秒＋序号命名），完整写盘后替换。
/// 敏感临时文件与最终文件均以 0600 创建（Unix）；并发写入按"最后成功替换者为准"
/// ——保证文件始终完整，不保证最新查询结果胜出；失败时只清理本次临时文件。
fn write_atomic(dir: &Path, name: &str, content: &str) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let final_path = dir.join(name);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    #[cfg(test)]
    let (nanos, seq) = {
        let pin = *TEST_TMP_PIN.lock().unwrap();
        pin.map(|(n, s)| (n, s)).unwrap_or((nanos, seq))
    };
    let tmp = dir.join(format!("{name}.tmp.{}.{}.{}", std::process::id(), nanos, seq));
    // 排他创建（2026-10-10 整改）：不复用已有 tmp 路径、不跟随软链接；
    // 仅本次成功创建的 tmp 才进入失败清理范围——创建失败（如路径已被占）
    // 直接返回，不动任何已存在的路径。
    let mut created = false;
    let result = (|| -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            created = true;
            f.write_all(content.as_bytes())?;
        }
        #[cfg(not(unix))]
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            created = true;
            use std::io::Write;
            f.write_all(content.as_bytes())?;
        }
        std::fs::rename(&tmp, &final_path).map_err(|e| {
            std::io::Error::new(e.kind(), format!("替换 {} 失败（tmp {}）：{e}", final_path.display(), tmp.display()))
        })
    })()
    .map_err(|e| format!("写 {} 失败：{e}", tmp.display()));
    if result.is_err() && created {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

// ---------- 自动更新 ----------

/// 刷新报告：逐厂商说明成功、失败或跳过原因。
#[derive(Default, Debug)]
pub struct RefreshReport {
    pub public_updated: Vec<String>,
    pub public_error: Option<String>,
    pub queried: Vec<(String, usize)>,
    pub failed: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
}

/// 自动更新：先同步公共目录基础资料，再对有凭证的厂商查询模型名单。
/// 完整查询成功才替换该入口旧名单；失败保留身份匹配的旧名单。
/// 只写自动目录，不碰手动配置——刷新不切换默认模型。
/// 每个带凭证的查询前调用 authorize 做 origin 授权检查（F1）；
/// 拒绝时该厂商进入 failed，不发出任何带凭证请求。
/// `catalog_url_override` 仅供测试注入本机模拟目录地址（见 sync_public）；
/// 正式二进制传 None，公共目录一律强制 HTTPS。
pub fn refresh(
    agent: &ureq::Agent,
    authorize: &mut dyn FnMut(&str, &str, &str) -> Result<(), String>,
    authorize_catalog: &mut dyn FnMut(&str) -> Result<(), String>,
    catalog_url_override: Option<&str>,
) -> Result<RefreshReport, String> {
    let dir = config_dir();
    let manual = load_manual(&dir)?;
    let mut auto = load_auto(&dir)?;
    let mut report = RefreshReport::default();
    // 手动新增、目录里还没有的厂商，先建空条目
    let ids: Vec<String> = manual
        .providers
        .keys()
        .chain(auto.providers.keys())
        .cloned()
        .collect();
    for id in ids {
        auto.providers.entry(id).or_default();
    }
    // F4：公共目录同步前做来源校验与授权；失败保留旧目录
    match crate::api::sync_public(&mut auto, agent, authorize_catalog, catalog_url_override) {
        Ok(v) => report.public_updated = v,
        Err(e) => report.public_error = Some(e),
    }
    let ids: Vec<String> = auto.providers.keys().cloned().collect();
    for id in ids {
        let conn = effective_connection(&manual, &auto, &id);
        if let Err(e) = validate_base_url(&conn.base_url) {
            report.failed.push((id, e));
            continue;
        }
        let Some(key_ref) = conn.key_ref.clone() else {
            continue; // 没有凭证引用，无法查询
        };
        let cred = resolve_credential(&key_ref);
        let Some(secret) = cred.value.clone() else {
            report
                .skipped
                .push((id.clone(), cred.error.unwrap_or_else(|| "无可用凭证".into())));
            continue;
        };
        // F1：凭证只发往已授权的 origin；拒绝则跳过该厂商且不发请求
        let origin = match origin_of(&conn.base_url) {
            Ok(o) => o,
            Err(e) => {
                report.failed.push((id.clone(), e));
                continue;
            }
        };
        if let Err(e) = authorize(&origin, &cred.name, &fingerprint(&secret)) {
            report.failed.push((id.clone(), e));
            continue;
        }
        match crate::api::list_models(&conn, &secret, agent) {
            Ok(models) => {
                let count = models.len();
                if let Some(entry) = auto.providers.get_mut(&id) {
                    entry.query = Some(QueryList {
                        updated_at: now_secs(),
                        identity: identity_of(&conn, &cred),
                        models,
                    });
                }
                report.queried.push((id, count));
            }
            // 失败：不改写该入口的名单，身份匹配的旧名单继续有效
            Err(e) => report.failed.push((id, e)),
        }
    }
    save_auto(&dir, &mut auto)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行化使用进程级环境变量的测试（TCHAT_USER_DIR 等为进程全局）。
    /// 委托 crate 级共享锁：tui_host 等其他模块的 env 测试与其互斥。
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn manual_with(base_url: Option<&str>, models: BTreeMap<String, ModelInfo>) -> ManualRoot {
        ManualRoot {
            selected_provider: Some("deepseek".into()),
            selected_model: Some("deepseek-flash".into()),
            memories: None,
            providers: BTreeMap::from([(
                "deepseek".to_string(),
                ManualProvider {
                    base_url: base_url.map(|s| s.into()),
                    key_ref: Some("env:DEEPSEEK_API_KEY".into()),
                    models,
                    ..Default::default()
                },
            )]),
        }
    }

    #[test]
    fn file_credential_confined_to_credential_dir() {
        let _g = env_lock();
        let root =
            std::env::temp_dir().join(format!("tchat-credbound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("TCHAT_USER_DIR", &root);
        // 专用目录外的凭证文件：必须拒绝（修复前会被正常读取）
        let outside = root.join("outside.json");
        std::fs::write(&outside, r#"{"k":"v-outside"}"#).unwrap();
        let err = resolve_credential(&format!("file:{}#k", outside.display()))
            .error
            .unwrap();
        assert!(err.contains("专用凭证目录"), "实际错误：{err}");
        // 目录内合法文件：可读
        let creds = root.join("credentials");
        std::fs::create_dir_all(&creds).unwrap();
        std::fs::write(creds.join("inner.json"), r#"{"k":"v-inside"}"#).unwrap();
        let c = resolve_credential(&format!("file:{}#k", creds.join("inner.json").display()));
        assert_eq!(c.value.as_deref(), Some("v-inside"));
        // 相对路径按专用目录解析
        let c2 = resolve_credential("file:inner.json#k");
        assert_eq!(c2.value.as_deref(), Some("v-inside"));
        // 相对路径穿越：拒绝
        let err2 = resolve_credential("file:../outside.json#k").error.unwrap();
        assert!(err2.contains("专用凭证目录"), "实际错误：{err2}");
        // 软链接逃逸：目录内软链接指向目录外文件 → 拒绝
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, creds.join("evil.json")).unwrap();
            let err3 = resolve_credential(&format!("file:{}#k", creds.join("evil.json").display()))
                .error
                .unwrap();
            assert!(err3.contains("专用凭证目录"), "实际错误：{err3}");
        }
        std::env::remove_var("TCHAT_USER_DIR");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn private_fs_modes_enforced() {
        let root = std::env::temp_dir().join(format!("tchat-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let sub = root.join("private");
        ensure_private_dir(&sub).unwrap();
        #[cfg(unix)]
        assert_eq!(file_mode(&sub), Some(0o700), "私有目录必须 0700");
        let f = sub.join("secret.json");
        write_private_file(&f, r#"{"k":"v"}"#).unwrap();
        #[cfg(unix)]
        assert_eq!(file_mode(&f), Some(0o600), "敏感文件必须 0600");
        // 覆盖写保持 0600
        write_private_file(&f, r#"{"k":"v2"}"#).unwrap();
        #[cfg(unix)]
        assert_eq!(file_mode(&f), Some(0o600));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), r#"{"k":"v2"}"#);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audit_tightens_and_reports_without_secrets() {
        let root = std::env::temp_dir().join(format!("tchat-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).unwrap();
        // .env 过宽（0777）且含秘密值；报告只允许出现路径，不允许出现秘密值
        let env_path = root.join(".env");
        std::fs::write(&env_path, "TOKEN=super-secret-value-xyz\n").unwrap();
        std::fs::write(root.join("config/manual.json"), "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o777)).unwrap();
            std::fs::set_permissions(
                root.join("config/manual.json"),
                std::fs::Permissions::from_mode(0o666),
            )
            .unwrap();
        }
        let report = audit_file_permissions(Some(&env_path), &root.join("config"));
        #[cfg(unix)]
        {
            assert_eq!(file_mode(&env_path), Some(0o600), ".env 应被收紧");
            assert_eq!(
                file_mode(&root.join("config/manual.json")),
                Some(0o600),
                "配置文件应被收紧"
            );
        }
        let joined = report.join("\n");
        assert!(!joined.contains("super-secret-value-xyz"), "报告不得包含秘密值：{joined}");
        // 上级目录可写：仅告警，不自动修改
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
            let report2 = audit_file_permissions(Some(&env_path), &root.join("config"));
            assert!(
                report2.iter().any(|l| l.contains("上级目录") && l.contains("未自动修改")),
                "应告警上级目录可写：{report2:?}"
            );
            assert_eq!(file_mode(&root), Some(0o777), "上级目录未被自动修改");
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn origin_grant_store_semantics() {
        let _user = UserDirGuard::new("origins");
        let origin = "https://api.example.com";
        // 初始无授权
        assert!(!is_origin_authorized(origin, "fp1"));
        // 授权后：同 origin + 同凭证指纹放行；换 Key 或换地址不放行
        grant_origin(origin, "fp1").unwrap();
        assert!(is_origin_authorized(origin, "fp1"));
        assert!(!is_origin_authorized(origin, "fp2"), "换 Key 必须重新授权");
        assert!(!is_origin_authorized("https://b.example.com", "fp1"), "换地址必须重新授权");
        // 授权记录落盘为私有文件
        #[cfg(unix)]
        assert_eq!(file_mode(&authorized_origins_path()), Some(0o600));
        // 记录损坏：按无授权处理（fail-closed）
        std::fs::write(authorized_origins_path(), "{broken").unwrap();
        assert!(!is_origin_authorized(origin, "fp1"));
        // 非交互：未授权直接拒绝，不询问
        assert!(ensure_origin_authorized(origin, "K", "fp1", false, &mut |_| true).is_err());
        // 交互拒绝：不写授权
        let mut deny = |q: &str| {
            assert!(q.contains(origin), "询问内容应包含地址：{q}");
            assert!(q.contains("指纹"), "询问内容应包含指纹：{q}");
            false
        };
        assert!(ensure_origin_authorized(origin, "K", "fp1", true, &mut deny).is_err());
        assert!(!is_origin_authorized(origin, "fp1"));
        // 交互同意：写入授权并放行
        let mut called = 0;
        ensure_origin_authorized(origin, "K", "fp3", true, &mut |q| {
            called += 1;
            assert!(q.contains(origin));
            true
        })
        .unwrap();
        assert_eq!(called, 1, "已授权后不应重复询问");
        assert!(is_origin_authorized(origin, "fp3"));
    }

    #[test]
    fn origin_of_normalizes() {
        assert_eq!(
            origin_of("https://api.deepseek.com").unwrap(),
            "https://api.deepseek.com"
        );
        assert_eq!(
            origin_of("https://api.deepseek.com/v1").unwrap(),
            "https://api.deepseek.com"
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8080/v1").unwrap(),
            "http://127.0.0.1:8080"
        );
        // 默认端口归一化
        assert_eq!(
            origin_of("https://api.deepseek.com:443/v1").unwrap(),
            "https://api.deepseek.com"
        );
        assert!(origin_of("not a url").is_err());
    }

    #[test]
    fn route_checks_credential_and_disabled_model() {
        let _user = UserDirGuard::new("route-ok");
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo { disabled: Some(true), ..Default::default() },
        );
        let manual = manual_with(None, models);
        let auto = AutoCatalog { providers: builtin(), ..Default::default() };
        let dict = merge_final(&manual, &auto);
        // 已选模型被禁用：路由检查拦截
        let err = route(&dict).unwrap_err();
        assert!(err.contains("已禁用"), "实际错误：{err}");
        // 选中不存在的模型：说明原因
        let mut manual2 = manual_with(None, BTreeMap::new());
        manual2.selected_model = Some("no-such-model".into());
        let err = route(&merge_final(&manual2, &auto)).unwrap_err();
        assert!(err.contains("不在厂商 deepseek 的有效名单中"), "实际错误：{err}");
        // 未设置 Key：报缺失原因
        let mut manual3 = manual_with(None, BTreeMap::new());
        manual3.providers.get_mut("deepseek").unwrap().key_ref = Some("env:TCHAT_ROUTE_UNSET".into());
        let err = route(&merge_final(&manual3, &auto)).unwrap_err();
        assert!(err.contains("TCHAT_ROUTE_UNSET 未设置"), "实际错误：{err}");
        // 正常情况：返回本次调用的快照
        let key_ref = write_cred_file_shared("route-ok", "key-A");
        let mut manual4 = manual_with(None, BTreeMap::new());
        manual4.providers.get_mut("deepseek").unwrap().key_ref = Some(key_ref);
        let (prov, cred, model) = route(&merge_final(&manual4, &auto)).unwrap();
        assert_eq!(prov.provider_id, "deepseek");
        assert_eq!(model.id, "deepseek-flash");
        assert_eq!(cred.value, "key-A");
    }

    #[test]
    fn dotenv_skips_trust_control_vars() {
        let _g = env_lock();
        let dir =
            std::env::temp_dir().join(format!("tchat-dotenv-trust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(
            &path,
            "TCHAT_CONFIG_DIR=/tmp/evil-config\n\
             TCHAT_USER_DIR=/tmp/evil-user\n\
             TCHAT_PUBLIC_CATALOG_URL=http://evil.example/x\n\
             HOME=/tmp/evil-home\n\
             APPDATA=/tmp/evil-appdata\n\
             tchat_config_dir=/tmp/evil-lower\n\
             tchat_user_dir=/tmp/evil-lower\n\
             home=/tmp/evil-lower\n\
             appdata=/tmp/evil-lower\n\
             NORMAL_DOTENV_KEY=ok-value\n",
        )
        .unwrap();
        // 保存并清空信任变量，模拟干净启动环境
        let saved: Vec<(&str, Option<std::ffi::OsString>)> = [
            "TCHAT_CONFIG_DIR",
            "TCHAT_USER_DIR",
            "TCHAT_PUBLIC_CATALOG_URL",
            "HOME",
            "APPDATA",
        ]
        .iter()
        .map(|k| (*k, std::env::var_os(k)))
        .collect();
        for (k, _) in &saved {
            std::env::remove_var(k);
        }
        std::env::remove_var("NORMAL_DOTENV_KEY");
        let n = load_dotenv_from(&path);
        // F1：信任根变量不得由项目 .env 注入；普通凭证变量照常加载
        assert_eq!(n, 1, "只应加载普通变量");
        for (k, _) in &saved {
            assert!(
                std::env::var_os(k).is_none(),
                "{k} 不得由项目 .env 注入（F1 信任根劫持）"
            );
        }
        assert_eq!(std::env::var("NORMAL_DOTENV_KEY").unwrap(), "ok-value");
        // 还原启动环境
        std::env::remove_var("NORMAL_DOTENV_KEY");
        for (k, v) in saved {
            if let Some(v) = v {
                std::env::set_var(k, v);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dotenv_loads_missing_vars_without_override() {
        let dir =
            std::env::temp_dir().join(format!("tchat-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(
            &path,
            "# 注释\nTCHAT_DOTENV_A=1\nTCHAT_DOTENV_B=\"x y\"\nTCHAT_DOTENV_A=2\nBAD LINE\nTCHAT_DOTENV_C=3 # inline\n",
        )
        .unwrap();
        std::env::set_var("TCHAT_DOTENV_A", "preset");
        // A 已存在不覆盖、不计数；注释行、非法行跳过；引号与行内注释正确处理
        let n = load_dotenv_from(&path);
        assert_eq!(n, 2);
        assert_eq!(std::env::var("TCHAT_DOTENV_A").unwrap(), "preset");
        assert_eq!(std::env::var("TCHAT_DOTENV_B").unwrap(), "x y");
        assert_eq!(std::env::var("TCHAT_DOTENV_C").unwrap(), "3");
        std::env::remove_var("TCHAT_DOTENV_A");
        std::env::remove_var("TCHAT_DOTENV_B");
        std::env::remove_var("TCHAT_DOTENV_C");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- 暴露用例：先钉住正确业务预期，再最小修复 ----------

    #[test]
    fn debug_output_redacts_key() {
        let cred = Credential { name: "K".into(), value: "sk-victim-secret".into() };
        assert!(!format!("{cred:?}").contains("sk-victim-secret"));
        assert!(!format!("{cred:#?}").contains("sk-victim-secret"));
        let outcome = CredOutcome {
            name: "K".into(),
            value: Some("sk-victim-secret".into()),
            error: None,
        };
        assert!(!format!("{outcome:?}").contains("sk-victim-secret"));
    }

    #[test]
    fn route_rejects_remote_http_and_empty_base_url() {
        let auto = AutoCatalog { providers: builtin(), ..Default::default() };
        // 远程 http：必须拒绝，防止 Key 以明文协议外发
        let mut manual = manual_with(None, BTreeMap::new());
        manual.providers.get_mut("deepseek").unwrap().base_url =
            Some("http://api.example.com".into());
        let err = route(&merge_final(&manual, &auto)).unwrap_err();
        assert!(err.contains("HTTPS"), "实际错误：{err}");
        // 显式空地址：报错，而不是静默退回基础地址
        let mut manual2 = manual_with(None, BTreeMap::new());
        manual2.providers.get_mut("deepseek").unwrap().base_url = Some("".into());
        let err = route(&merge_final(&manual2, &auto)).unwrap_err();
        assert!(err.contains("地址"), "实际错误：{err}");
    }

    #[test]
    fn unknown_manual_field_rejected() {
        let dir =
            std::env::temp_dir().join(format!("tchat-typo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manual.json");
        std::fs::write(
            &path,
            r#"{"selected_provider":"deepseek","selected_model":"deepseek-flash","providers":{"deepseek":{"base_ur":"https://x"}}}"#,
        )
        .unwrap();
        let err = load_manual(&dir).unwrap_err();
        assert!(err.contains("base_ur"), "实际错误：{err}");
        // 根层未知字段同样被明确拒绝（同一 deny_unknown_fields 机制）
        std::fs::write(
            &path,
            r#"{"unknown_root_field":1,"selected_provider":"deepseek","selected_model":"deepseek-flash","providers":{}}"#,
        )
        .unwrap();
        let err = load_manual(&dir).unwrap_err();
        assert!(err.contains("unknown_root_field"), "实际错误：{err}");
        // 程序不自动改写用户文件
        assert!(std::fs::read_to_string(&path).unwrap().contains("unknown_root_field"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manual_overrides_only_written_fields() {
        let manual = manual_with(Some("https://proxy.example.com/v1"), BTreeMap::new());
        let auto = AutoCatalog { providers: builtin(), ..Default::default() };
        let dict = merge_final(&manual, &auto);
        let p = &dict.providers["deepseek"];
        // 地址用手动值；协议、凭证引用未覆盖，沿用基础值
        assert_eq!(p.conn.base_url, "https://proxy.example.com/v1");
        assert_eq!(p.conn.protocol, SUPPORTED_PROTOCOL);
        assert_eq!(p.conn.key_ref.as_deref(), Some("env:DEEPSEEK_API_KEY"));
    }

    /// 019：input_modalities 字段——手动确认值经逐字段覆盖进入最终字典；
    /// 缺字段（未知）与只 text 均关闭图片能力；能力判断不按名称猜测。
    #[test]
    fn input_modalities_overlay_and_capability() {
        let auto = AutoCatalog { providers: builtin(), ..Default::default() };
        // 手动声明 text+image → 保留并开放
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo {
                input_modalities: Some(vec!["text".into(), "image".into()]),
                ..Default::default()
            },
        );
        let manual = manual_with(None, models);
        let dict = merge_final(&manual, &auto);
        let m = &dict.providers["deepseek"].models["deepseek-flash"];
        assert_eq!(
            m.input_modalities,
            Some(vec!["text".to_string(), "image".to_string()])
        );
        assert!(crate::catalog::supports_image_input(m));
        // 只 text → 关闭
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo { input_modalities: Some(vec!["text".into()]), ..Default::default() },
        );
        let dict = merge_final(&manual_with(None, models), &auto);
        assert!(!crate::catalog::supports_image_input(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
        // 未声明（内置/自动侧无该字段）＝未知 → 关闭
        let dict = merge_final(&manual_with(None, BTreeMap::new()), &auto);
        assert!(!crate::catalog::supports_image_input(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
        // 手动覆盖未写模态字段时不抹掉自动侧已有值（逐字段覆盖语义）
        let mut auto2 = AutoCatalog { providers: builtin(), ..Default::default() };
        let mut base_models = BTreeMap::new();
        base_models.insert(
            "deepseek-flash".to_string(),
            ModelInfo {
                input_modalities: Some(vec!["text".into(), "image".into()]),
                ..Default::default()
            },
        );
        auto2.providers.get_mut("deepseek").unwrap().base.models = base_models;
        let mut manual_ov = BTreeMap::new();
        manual_ov.insert(
            "deepseek-flash".to_string(),
            ModelInfo { display_name: Some("仅改名".into()), ..Default::default() },
        );
        let dict = merge_final(&manual_with(None, manual_ov), &auto2);
        assert!(crate::catalog::supports_image_input(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
    }

    /// 021：experimental_supported_tools——精确名匹配启用 send_message_to_user_async；
    /// 缺省/空数组/其他名称关闭；手动值逐字段覆盖自动侧（显式空数组可关闭目录声明）。
    #[test]
    fn experimental_supported_tools_overlay_and_capability() {
        let auto = AutoCatalog { providers: builtin(), ..Default::default() };
        // 手动精确声明 → 开放
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo {
                experimental_supported_tools: Some(vec![
                    "send_message_to_user_async".to_string()
                ]),
                ..Default::default()
            },
        );
        let dict = merge_final(&manual_with(None, models), &auto);
        let m = &dict.providers["deepseek"].models["deepseek-flash"];
        assert!(crate::catalog::supports_send_message_to_user(m));
        // 空数组＝显式关闭
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo {
                experimental_supported_tools: Some(Vec::new()),
                ..Default::default()
            },
        );
        let dict = merge_final(&manual_with(None, models), &auto);
        assert!(!crate::catalog::supports_send_message_to_user(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
        // 其他工具名不启用（精确匹配，不按名称/问询能力推断）
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo {
                experimental_supported_tools: Some(vec![
                    "request_user_input_async".to_string(),
                    "web_search".to_string(),
                ]),
                ..Default::default()
            },
        );
        let dict = merge_final(&manual_with(None, models), &auto);
        assert!(!crate::catalog::supports_send_message_to_user(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
        // 未声明＝关闭
        let dict = merge_final(&manual_with(None, BTreeMap::new()), &auto);
        assert!(!crate::catalog::supports_send_message_to_user(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
        // 手动覆盖未写该字段时不抹掉自动侧已有值（逐字段覆盖语义）
        let mut auto2 = AutoCatalog { providers: builtin(), ..Default::default() };
        let mut base_models = BTreeMap::new();
        base_models.insert(
            "deepseek-flash".to_string(),
            ModelInfo {
                experimental_supported_tools: Some(vec![
                    "send_message_to_user_async".to_string()
                ]),
                ..Default::default()
            },
        );
        auto2.providers.get_mut("deepseek").unwrap().base.models = base_models;
        let mut manual_ov = BTreeMap::new();
        manual_ov.insert(
            "deepseek-flash".to_string(),
            ModelInfo { display_name: Some("仅改名".into()), ..Default::default() },
        );
        let dict = merge_final(&manual_with(None, manual_ov), &auto2);
        assert!(crate::catalog::supports_send_message_to_user(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
        // 手动显式空数组覆盖自动侧声明（可关闭目录声明）
        let mut manual_off = BTreeMap::new();
        manual_off.insert(
            "deepseek-flash".to_string(),
            ModelInfo {
                experimental_supported_tools: Some(Vec::new()),
                ..Default::default()
            },
        );
        let dict = merge_final(&manual_with(None, manual_off), &auto2);
        assert!(!crate::catalog::supports_send_message_to_user(
            &dict.providers["deepseek"].models["deepseek-flash"]
        ));
    }

    /// 019：input_modalities 只接受 text/image 子集；audio 等未知值明确报错。
    #[test]
    fn input_modalities_rejects_unknown_values() {
        let dir = std::env::temp_dir().join(format!("tchat-modality-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manual.json");
        std::fs::write(
            &path,
            r#"{"selected_provider":"deepseek","selected_model":"deepseek-flash","providers":{"deepseek":{"models":{"deepseek-flash":{"id":"deepseek-flash","input_modalities":["text","audio"]}}}}}"#,
        )
        .unwrap();
        let err = load_manual(&dir).unwrap_err();
        assert!(
            err.contains("input_modalities") || err.contains("audio"),
            "实际错误：{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manual_model_overlay_add_and_restore() {
        let mut override_map = BTreeMap::new();
        override_map.insert(
            "deepseek-flash".to_string(),
            ModelInfo { display_name: Some("我的模型".into()), ..Default::default() },
        );
        override_map.insert(
            "my-model".to_string(),
            ModelInfo { display_name: Some("手动新增".into()), ..Default::default() },
        );
        let manual = manual_with(None, override_map);
        let auto = AutoCatalog { providers: builtin(), ..Default::default() };
        let dict = merge_final(&manual, &auto);
        let models = &dict.providers["deepseek"].models;
        // 手动新增模型加入，不覆盖其他模型
        assert!(models.contains_key("my-model"));
        assert!(models.contains_key("deepseek-v4-pro"));
        // 覆盖字段用手动值
        assert_eq!(models["deepseek-flash"].display_name.as_deref(), Some("我的模型"));
        // 未覆盖字段保留自动值
        assert_eq!(models["deepseek-flash"].context_length, Some(1_048_576));
        // 删除覆盖字段（不带该字段重建）恢复自动值
        let mut restored_map = BTreeMap::new();
        restored_map.insert(
            "deepseek-flash".to_string(),
            ModelInfo { context_length: Some(99), ..Default::default() },
        );
        let manual2 = manual_with(None, restored_map);
        let dict2 = merge_final(&manual2, &auto);
        assert_eq!(
            dict2.providers["deepseek"].models["deepseek-flash"].display_name,
            Some("DeepSeek-V4.1-Flash".into())
        );
        // 删除地址覆盖字段（manual2 未写 base_url）恢复自动地址
        assert_eq!(
            dict2.providers["deepseek"].conn.base_url,
            "https://api.deepseek.com"
        );
    }

    // 单元测试目标无法引用 tests/（仅集成测试可见），用 include! 文本引入共享实现；
    // 共享核心见 tests/common/cred_file_write.rs，本侧按 lib 内部路径解析凭证目录。
    include!("../tests/common/cred_file_write.rs");

    fn cred_dir() -> std::path::PathBuf {
        credential_dir()
    }

    /// 测试期用户私有目录：串行化设置 TCHAT_USER_DIR，结束恢复并清理。
    struct UserDirGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        path: PathBuf,
    }

    impl UserDirGuard {
        fn new(tag: &str) -> Self {
            let lock = env_lock();
            let path =
                std::env::temp_dir().join(format!("tchat-user-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            std::env::set_var("TCHAT_USER_DIR", &path);
            UserDirGuard { _lock: lock, path }
        }
    }

    impl Drop for UserDirGuard {
        fn drop(&mut self) {
            std::env::remove_var("TCHAT_USER_DIR");
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn fingerprint_rules() {
        assert_eq!(fingerprint("same"), fingerprint("same"));
        assert_ne!(fingerprint("key-A"), fingerprint("key-B"));
        // 独立固定向量（FIPS 180-2 附录 B 例 1）：SHA-256("abc") 前 16 位十六进制。
        // 期望值来自标准向量而非被测函数现场生成，钉死「摘要前缀」规则本身。
        assert_eq!(fingerprint("abc"), "ba7816bf8f01cfea");
        assert_eq!(fingerprint("x").len(), 16);
    }

    #[test]
    fn credential_env_and_file() {
        std::env::set_var("TCHAT_TEST_KEY", "v1");
        let c = resolve_credential("env:TCHAT_TEST_KEY");
        assert_eq!(c.value.as_deref(), Some("v1"));
        assert_eq!(c.name, "TCHAT_TEST_KEY");
        std::env::remove_var("TCHAT_TEST_KEY");
        let c = resolve_credential("env:TCHAT_TEST_KEY");
        assert!(c.value.is_none());
        assert!(c.error.unwrap().contains("未设置"));
        let c = resolve_credential("weird:ref");
        assert!(c.error.unwrap().contains("不支持"));
        // 已设置但值为空：报"为空"，不当成有效值
        std::env::set_var("TCHAT_TEST_KEY", "");
        let c = resolve_credential("env:TCHAT_TEST_KEY");
        assert!(c.value.is_none());
        assert!(c.error.unwrap().contains("为空"));
        std::env::remove_var("TCHAT_TEST_KEY");
    }

    // ---------- 六类清单补测 ----------

    #[test]
    fn validate_base_url_rules() {
        assert!(validate_base_url("https://api.deepseek.com").is_ok());
        assert!(validate_base_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_base_url("http://localhost:9000").is_ok());
        assert!(validate_base_url("http://[::1]:9000").is_ok());
        // 远程 http 拒绝；合法 https 不代表域名可信，这里只管传输协议
        assert!(validate_base_url("http://api.example.com").unwrap_err().contains("HTTPS"));
        assert!(validate_base_url("").unwrap_err().contains("地址为空"));
        assert!(validate_base_url("  ").unwrap_err().contains("地址为空"));
        assert!(validate_base_url("api.example.com").unwrap_err().contains("前缀"));
        assert!(validate_base_url("ftp://files.example.com").unwrap_err().contains("ftp"));
        assert!(validate_base_url("https://api.deepseek.com with space").is_err());
    }

    #[test]
    fn credential_file_error_branches() {
        let _user = UserDirGuard::new("credfile");
        let dir = credential_dir();
        std::fs::create_dir_all(&dir).unwrap();
        // 文件不存在
        let err = resolve_credential("file:nope.json#k")
            .error
            .unwrap();
        assert!(err.contains("读取失败"), "实际错误：{err}");
        // 非 JSON
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        let err = resolve_credential("file:bad.json#k").error.unwrap();
        assert!(err.contains("JSON"), "实际错误：{err}");
        // 缺字段 / 值为空
        let partial = dir.join("partial.json");
        std::fs::write(&partial, r#"{"other":"v"}"#).unwrap();
        let err = resolve_credential("file:partial.json#k").error.unwrap();
        assert!(err.contains("没有字段 k"), "实际错误：{err}");
        let empty = dir.join("empty.json");
        std::fs::write(&empty, r#"{"k":""}"#).unwrap();
        let err = resolve_credential("file:empty.json#k").error.unwrap();
        assert!(err.contains("为空"), "实际错误：{err}");
        // file: 缺 #名称
        let err = resolve_credential("file:bad.json")
            .error
            .unwrap();
        assert!(err.contains("#名称"), "实际错误：{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn identity_fields_each_invalidate() {
        let _user = UserDirGuard::new("ident");
        let make_auto = |identity: Identity| {
            let mut auto = AutoCatalog { providers: builtin(), ..Default::default() };
            auto.providers.get_mut("deepseek").unwrap().query = Some(QueryList {
                updated_at: 1,
                identity,
                models: vec![ModelInfo { id: "only-from-query".into(), ..Default::default() }],
            });
            auto
        };
        let base_identity = Identity {
            base_url: "https://a.example.com".into(),
            protocol: SUPPORTED_PROTOCOL.into(),
            cred_name: "n1".into(),
            cred_fp: fingerprint("k1"),
        };
        let manual_for = |base_url: &str, key_ref: &str| ManualRoot {
            selected_provider: Some("deepseek".into()),
            selected_model: Some("deepseek-flash".into()),
            memories: None,
            providers: BTreeMap::from([(
                "deepseek".to_string(),
                ManualProvider {
                    base_url: Some(base_url.into()),
                    protocol: None,
                    key_ref: Some(key_ref.into()),
                    ..Default::default()
                },
            )]),
        };
        let dir = credential_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let k1 = dir.join("k1.json");
        let k2 = dir.join("k2.json");
        std::fs::write(&k1, r#"{"n1":"k1","n2":"k2"}"#).unwrap();
        // 控制组：身份完全一致 → 名单有效
        let ref_n1 = "file:k1.json#n1";
        let dict = merge_final(&manual_for("https://a.example.com", &ref_n1), &make_auto(base_identity.clone()));
        assert!(dict.providers["deepseek"].models.contains_key("only-from-query"));
        // 地址变化 → 失效
        let dict = merge_final(&manual_for("https://b.example.com", &ref_n1), &make_auto(base_identity.clone()));
        assert!(!dict.providers["deepseek"].models.contains_key("only-from-query"));
        // 协议变化 → 失效
        let mut m = manual_for("https://a.example.com", &ref_n1);
        m.providers.get_mut("deepseek").unwrap().protocol = Some("other-proto".into());
        let dict = merge_final(&m, &make_auto(base_identity.clone()));
        assert!(!dict.providers["deepseek"].models.contains_key("only-from-query"));
        // 凭证名变化（同名文件不同字段）→ 失效
        let ref_n2 = "file:k1.json#n2";
        let dict = merge_final(&manual_for("https://a.example.com", &ref_n2), &make_auto(base_identity.clone()));
        assert!(!dict.providers["deepseek"].models.contains_key("only-from-query"));
        // 同名不同 Key（k2.json 同字段 n1 值不同）→ 失效
        std::fs::write(&k2, r#"{"n1":"k1-CHANGED"}"#).unwrap();
        let ref_changed = "file:k2.json#n1";
        let dict = merge_final(&manual_for("https://a.example.com", &ref_changed), &make_auto(base_identity.clone()));
        assert!(!dict.providers["deepseek"].models.contains_key("only-from-query"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_false_and_zero_semantics() {
        let _user = UserDirGuard::new("explicit-false");
        let auto = AutoCatalog { providers: builtin(), ..Default::default() };
        // 显式 disabled:false 恢复可选（不被当作未填写）
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo { disabled: Some(false), ..Default::default() },
        );
        let key_ref = write_cred_file_shared("explicit-false", "key-A");
        let mut manual = manual_with(None, models);
        manual.providers.get_mut("deepseek").unwrap().key_ref = Some(key_ref);
        let dict = merge_final(&manual, &auto);
        assert_eq!(dict.providers["deepseek"].models["deepseek-flash"].disabled, Some(false));
        assert!(route(&dict).is_ok());
        // context_length:0 保留为 Some(0)，不当成 None
        let mut models = BTreeMap::new();
        models.insert(
            "deepseek-flash".to_string(),
            ModelInfo { context_length: Some(0), ..Default::default() },
        );
        let dict = merge_final(&manual_with(None, models), &auto);
        assert_eq!(dict.providers["deepseek"].models["deepseek-flash"].context_length, Some(0));
    }

    #[test]
    fn corrupt_files_reported_and_not_rewritten() {
        let dir = std::env::temp_dir().join(format!("tchat-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let auto_path = dir.join("auto_catalog.json");
        std::fs::write(&auto_path, r#"{"providers": { "deep" "#).unwrap();
        let before = std::fs::read_to_string(&auto_path).unwrap();
        let err = load_auto(&dir).unwrap_err();
        assert!(err.contains("损坏"), "实际错误：{err}");
        assert!(err.contains("auto_catalog.json"), "实际错误：{err}");
        // 程序不自动覆盖损坏文件
        assert_eq!(std::fs::read_to_string(&auto_path).unwrap(), before);
        let manual_path = dir.join("manual.json");
        std::fs::write(&manual_path, r#"{"selected_provider": "deep""#).unwrap();
        let before = std::fs::read_to_string(&manual_path).unwrap();
        let err = load_manual(&dir).unwrap_err();
        assert!(err.contains("格式错误"), "实际错误：{err}");
        assert_eq!(std::fs::read_to_string(&manual_path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_run_creates_default_files() {
        let dir = std::env::temp_dir().join(format!("tchat-first-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let manual = load_manual(&dir).unwrap();
        let auto = load_auto(&dir).unwrap();
        assert_eq!(manual.selected_provider.as_deref(), Some("deepseek"));
        assert_eq!(manual.selected_model.as_deref(), Some("deepseek-flash"));
        assert!(auto.providers.contains_key("deepseek"));
        assert!(dir.join("manual.json").exists());
        assert!(dir.join("auto_catalog.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readonly_dir_write_fails_cleanly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tchat-readonly-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 已有可用数据
        let mut auto = AutoCatalog { providers: builtin(), ..Default::default() };
        save_auto(&dir, &mut auto).unwrap();
        let before = std::fs::read_to_string(dir.join("auto_catalog.json")).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // 权限限制真实生效（root 不受限制则跳过本用例）
        if std::fs::write(dir.join("probe"), "x").is_ok() {
            let _ = std::fs::remove_file(dir.join("probe"));
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("跳过：当前用户不受目录权限限制（root）");
            return;
        }
        let mut auto2 = AutoCatalog { providers: builtin(), ..Default::default() };
        let err = save_auto(&dir, &mut auto2).unwrap_err();
        assert!(err.contains("失败"), "实际错误：{err}");
        // 无半写文件、已有数据未损坏、失败后不留本次临时文件
        assert_eq!(std::fs::read_to_string(dir.join("auto_catalog.json")).unwrap(), before);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_tmp_does_not_block_save() {
        let dir = std::env::temp_dir().join(format!("tchat-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 其他进程遗留的旧命名临时文件
        std::fs::write(dir.join("auto_catalog.json.tmp"), "leftover").unwrap();
        let mut auto = AutoCatalog { providers: builtin(), ..Default::default() };
        // 打可辨识标记：证明本次保存的新内容确实落盘（而非 load_auto 重建默认文件）
        auto.providers
            .get_mut("deepseek")
            .unwrap()
            .base
            .models
            .insert("stale-tmp-marker".into(), ModelInfo { id: "stale-tmp-marker".into(), ..Default::default() });
        save_auto(&dir, &mut auto).unwrap();
        let loaded = load_auto(&dir).unwrap();
        assert!(
            loaded.providers["deepseek"].base.models.contains_key("stale-tmp-marker"),
            "新内容必须落盘：遗留临时文件不得使保存静默失效"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("auto_catalog.json.tmp")).unwrap(),
            "leftover"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- 高风险清单补充（2026-10-10）----------

    /// NET-B03：回环豁免名单精确性——127.0.0.2 属 127/8 回环段但不在项目名单内，
    /// 按本地契约必须拒绝明文 HTTP（名单为 localhost/127.0.0.1/[::1] 三者）。
    #[test]
    fn net_b03_loopback_list_precise() {
        for allowed in ["http://127.0.0.1:8080", "http://localhost:9000", "http://[::1]:9000"] {
            assert!(validate_base_url(allowed).is_ok(), "{allowed} 应豁免");
        }
        let err = validate_base_url("http://127.0.0.2:8080").unwrap_err();
        assert!(err.contains("HTTPS"), "127.0.0.2 不在豁免名单：{err}");
    }

    /// AUTH-B05（待决项受控调查）：两线程并发首次授权不同 origin——只断言安全
    /// 不变量：文件始终可解析、每条 grant 都是本次双方同意之一、结构完整；
    /// 是否两条都保留属未定承诺（见清单待补规则），实际结果记录在断言消息。
    #[test]
    fn auth_b05_concurrent_grants_investigation() {
        let _user = UserDirGuard::new("b05");
        let origin_a = "http://127.0.0.1:11111";
        let origin_b = "http://127.0.0.1:22222";
        let h1 = std::thread::spawn(|| grant_origin("http://127.0.0.1:11111", "fpA"));
        let h2 = std::thread::spawn(|| grant_origin("http://127.0.0.1:22222", "fpB"));
        h1.join().unwrap().unwrap();
        h2.join().unwrap().unwrap();
        let text = std::fs::read_to_string(authorized_origins_path()).unwrap();
        let store: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("并发写后文件必须可解析：{e}\n{text}"));
        let grants = store["grants"].as_array().unwrap();
        for g in grants {
            let origin = g["origin"].as_str().unwrap_or("");
            let fp = g["cred_fp"].as_str().unwrap_or("");
            let valid = (origin == "http://127.0.0.1:11111" && fp == "fpA")
                || (origin == "http://127.0.0.1:22222" && fp == "fpB");
            assert!(valid, "不得出现未经同意或混合的组合：{g}（全文 {text}）");
        }
        // 调查记录：读-改-写竞争下实际保留条数（1=后写覆盖，2=都保留）
        eprintln!("AUTH-B05 调查：并发授权后实际保留 {} 条（完整文本：{text}）", grants.len());
    }

    /// TRUST-B05：凭证根自身为符号链接——两侧 canonicalize 后根内引用仍可用、
    /// 逃逸仍拒绝（不得只规范化引用而保留未规范化的根）。
    #[test]
    fn trust_b05_credential_root_symlink() {
        let _g = env_lock();
        let real = std::env::temp_dir().join(format!("tchat-root-real-{}", std::process::id()));
        let link = std::env::temp_dir().join(format!("tchat-root-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&real);
        let _ = std::fs::remove_file(&link);
        std::fs::create_dir_all(real.join("credentials")).unwrap();
        std::fs::write(real.join("credentials/inner.json"), r#"{"k":"v-root-link"}"#).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        std::env::set_var("TCHAT_USER_DIR", &link);
        // 经链接根的根内引用：可用
        let c = resolve_credential("file:inner.json#k");
        assert_eq!(c.value.as_deref(), Some("v-root-link"), "链接根下合法引用应可用");
        // 经链接根的逃逸：拒绝且不泄露内容
        std::fs::write(real.join("secret.json"), r#"{"k":"v-outside"}"#).unwrap();
        let err = resolve_credential("file:../secret.json#k").error.unwrap();
        assert!(err.contains("专用凭证目录"), "经链接根逃逸必须拒绝：{err}");
        assert!(!err.contains("v-outside"), "{err}");
        std::env::remove_var("TCHAT_USER_DIR");
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&real);
    }

    /// TRUST-B06：.env 的 BOM 与 CRLF——BOM 行键名非法按既有规则跳过（不支持即跳过，
    /// 不新增 BOM 能力）、不 panic；CRLF 行正常解析；控制变量在两种形态下都不生效。
    #[test]
    fn trust_b06_dotenv_bom_and_crlf() {
        let _g = env_lock();
        let dir = std::env::temp_dir().join(format!("tchat-dotenv-b06-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // BOM＋CRLF：首键带 BOM 非法字符→跳过；后续 CRLF 行正常加载
        let mut text = String::from("\u{feff}TCHAT_CONFIG_DIR=/tmp/evil\r\n");
        text.push_str("TCHAT_B06_CRLF=crlf-value\r\n");
        std::fs::write(dir.join(".env"), text).unwrap();
        for k in ["TCHAT_CONFIG_DIR", "TCHAT_B06_CRLF"] {
            std::env::remove_var(k);
        }
        let n = load_dotenv_from(&dir.join(".env"));
        assert_eq!(n, 1, "仅 CRLF 正常行加载，BOM 行跳过");
        assert!(std::env::var_os("TCHAT_CONFIG_DIR").is_none(), "BOM 形态控制变量不得生效");
        assert_eq!(std::env::var("TCHAT_B06_CRLF").unwrap(), "crlf-value");
        std::env::remove_var("TCHAT_B06_CRLF");
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// 静态坏文件＋交互同意：整文件覆盖为新 store 恰 1 对（不并旧垃圾）。
    #[test]
    fn broken_store_consent_replaces_with_single_pair() {
        let _user = UserDirGuard::new("d7");
        std::fs::write(authorized_origins_path(), "{broken").unwrap();
        ensure_origin_authorized("https://d7.example.com", "K", "fpD7", true, &mut |_| true)
            .unwrap();
        let text = std::fs::read_to_string(authorized_origins_path()).unwrap();
        let store: serde_json::Value = serde_json::from_str(&text).unwrap();
        let grants = store["grants"].as_array().unwrap();
        assert_eq!(grants.len(), 1, "整文件覆盖恰 1 对");
        assert_eq!(grants[0]["origin"], "https://d7.example.com");
        assert_eq!(grants[0]["cred_fp"], "fpD7");
        assert!(is_origin_authorized("https://d7.example.com", "fpD7"));
    }

    /// 2026-10-10 合并前整改：grant_origin 原子写——失败保留旧授权、A 有效 B 未获授权。
    #[test]
    fn grant_write_failure_preserves_old_store() {
        let _user = UserDirGuard::new("grantsafe");
        let a = "https://a.example.com";
        let b = "https://b.example.com";
        grant_origin(a, "fpA").unwrap();
        let before = std::fs::read_to_string(authorized_origins_path()).unwrap();
        // 预置 tmp 路径为目录：原子写打开必须失败且不得清理预置路径
        *TEST_TMP_PIN.lock().unwrap() = Some((11, 11));
        let tmp = user_dir()
            .join(format!("authorized-origins.json.tmp.{}.11.11", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir(&tmp).unwrap();
        let err = grant_origin(b, "fpB").unwrap_err();
        assert!(err.contains("失败"), "授权写入失败必须明确报错：{err}");
        assert!(tmp.is_dir(), "预置路径（目录）不被删除——仅清理本次创建的 tmp");
        assert_eq!(
            std::fs::read_to_string(authorized_origins_path()).unwrap(),
            before,
            "旧授权文件字节不变"
        );
        assert!(is_origin_authorized(a, "fpA"), "A 仍有效");
        assert!(!is_origin_authorized(b, "fpB"), "B 不得获得授权");
        *TEST_TMP_PIN.lock().unwrap() = None;
        // 成功路径：解除占位后授权 B 落盘，A/B 均有效，无 tmp 残留
        let _ = std::fs::remove_dir_all(&tmp);
        grant_origin(b, "fpB").unwrap();
        assert!(is_origin_authorized(a, "fpA") && is_origin_authorized(b, "fpB"));
        let residue: Vec<String> = std::fs::read_dir(user_dir()).unwrap()
            .filter_map(|e| e.unwrap().file_name().into_string().ok())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(residue.is_empty(), "成功后无 tmp 残留：{residue:?}");
    }

    fn tmp_conflict_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tchat-tmpconf-{}-{tag}", std::process::id()))
    }

    /// 2026-10-10 整改：tmp 冲突保护——预置路径与内容不得被覆盖或删除；
    /// 与其余 TEST_TMP_PIN/env 使用者共用 env_lock 串行（锁域一致防 pin 互踩）。
    #[test]
    fn preexisting_tmp_path_is_protected() {
        let _guard = env_lock();
        let d = tmp_conflict_dir("pre");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        *TEST_TMP_PIN.lock().unwrap() = Some((7, 7));
let tmp = d.join(format!("state.json.tmp.{}.7.7", std::process::id()));
        std::fs::write(&tmp, "PREEXISTING").unwrap();
        let err = write_atomic(&d, "state.json", "NEW").unwrap_err();
        assert!(err.contains("失败"), "冲突必须明确报错：{err}");
        assert_eq!(
            std::fs::read_to_string(&tmp).unwrap(),
            "PREEXISTING",
            "预置 tmp 路径内容不得被覆盖，也不得在失败清理中被删除"
        );
        assert!(!d.join("state.json").exists(), "冲突时目标不得被替换");
        *TEST_TMP_PIN.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&d);
    }

        #[test]
    fn preexisting_tmp_symlink_is_not_followed() {
        let _guard = env_lock();
        let d = tmp_conflict_dir("sym");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        *TEST_TMP_PIN.lock().unwrap() = Some((9, 9));
let victim = d.join("victim.txt");
        std::fs::write(&victim, "VICTIM").unwrap();
        let tmp = d.join(format!("state.json.tmp.{}.9.9", std::process::id()));
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
        let err = write_atomic(&d, "state.json", "NEW").unwrap_err();
        assert!(err.contains("失败"), "软链接占位必须拒绝：{err}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "VICTIM", "不得写穿链接");
        assert!(
            std::fs::symlink_metadata(&tmp).unwrap().file_type().is_symlink(),
            "链接本体保持"
        );
        assert!(!d.join("state.json").exists());
        *TEST_TMP_PIN.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&d);
    }
}
