//! 031：权限申请——可信策略、请求解析、路径规范化与轮次授权。
//!
//! 职责（M1）：`TCHAT_PERMISSION_POLICY` 可信策略文件的严格加载与信任来源
//! 校验；`request_permissions` 工具参数的严格解析（§1.4 全部边界）；相对
//! 路径到会话 cwd 的逐组件规范化（拒绝符号链接与缺失组件）；四字段结果
//! 结构（§1.5）；本轮授权集合的授予/覆盖/64 条上限语义（§1.7）。
//!
//! 严格 JSON：serde_json 的 Map 反序列化会**静默合并重复键**（022 教训），
//! 本模块所有输入（策略与请求）都经 [`parse_strict_json`]——递归 visitor
//! 在每一层对象上拒绝重复键；未知字段由各层校验显式拒绝。

use std::path::{Path, PathBuf};

use serde_json::Value;

pub const TOOL_NAME: &str = "request_permissions";
/// 可信启动变量（加入 TRUST_CONTROL_VARS；.env 不得注入）。
pub const POLICY_ENV_VAR: &str = "TCHAT_PERMISSION_POLICY";

/// 策略文件上限（§1.8 静态边界）：恰好允许、超过拒绝、不截断。
pub const POLICY_MAX_BYTES: usize = 1024 * 1024;
/// 工具 arguments 原始 JSON 上限（§1.8）。
pub const ARGS_MAX_BYTES: usize = 64 * 1024;
/// 单条路径/reason 的 UTF-8 字节上限（§1.8）。
pub const TEXT_MAX_BYTES: usize = 4096;
/// 单请求文件系统条目上限（§1.4）。
pub const ENTRIES_MAX: usize = 32;
/// 本轮累计去重后的额外路径上限（§1.7）。
pub const GRANT_PATHS_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

impl Access {
    fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }

    /// write 包含读取（§1.4）：授权 write 即覆盖对同路径 read 的需求。
    fn covers(self, other: Access) -> bool {
        matches!((self, other), (Access::Write, _) | (Access::Read, Access::Read))
    }
}

#[derive(Debug, Clone)]
pub struct FsEntry {
    pub path: PathBuf,
    pub access: Access,
}

#[derive(Debug, Clone)]
pub struct FsScope {
    pub entries: Vec<FsEntry>,
    pub network: bool,
}

/// 已加载并校验的 031 策略（版本 1）。路径均为**已存在的绝对实际路径**
/// （加载时逐条 canonicalize；不存在即拒绝——§1.3）。
#[derive(Debug, Clone)]
pub struct Policy {
    pub base: FsScope,
    pub requestable: FsScope,
    /// 显式 deny_paths（不含自动合并的敏感边界——那是
    /// [`Policy::effective_deny_paths`] 的职责）。
    pub deny_paths: Vec<PathBuf>,
}

/// 自动敏感边界的装配输入（§1.3：.env、应用凭证/配置/会话/记忆私有根、
/// 策略与应用/bwrap 安装目录、宿主控制套接字类位置）。生产环境由
/// [`deny_context_for`] 从进程状态构建；测试显式传参以隔离环境。
pub struct DenyContext {
    /// 主进程 cwd（启动期会话工作区锚点）。
    pub anchor: PathBuf,
    pub config_dir: PathBuf,
    pub user_dir: PathBuf,
    /// 策略与应用/bwrap 安装目录（可信部署位置，全部 deny）。
    pub install_dirs: Vec<PathBuf>,
}

/// 生产 DenyContext：catalog 目录解析＋策略/自身二进制/bwrap 安装位置。
/// §1.3 契约为「安装目录」级 deny；**当安装位置位于固定运行时白名单树
/// （/usr、/etc——usrmerge 部署下 bwrap 即在 /usr/bin）之内时退为单文件
/// deny**：目录级会遮蔽 shell/coreutils，与 §1.6「载荷必须能运行」冲突，
/// 且 dpkg 合法硬链接（perl↔perl5.x）使目录级递归 nlink 检查每次误拒
/// （e2e S140 实证）。保护对象不变（应用/bwrap 二进制与策略本身）；运行时
/// 白名单之外的安装目录保持**目录级** deny（契约文义在该场景完整保留）。
pub fn deny_context_for(anchor: &Path, policy_path: &Path) -> DenyContext {
    let mut install_dirs = vec![policy_dir_canonical(policy_path)];
    let mut push_install = |p: PathBuf| {
        let in_runtime_tree = p.starts_with("/usr") || p.starts_with("/etc");
        if in_runtime_tree {
            install_dirs.push(p);
        } else if let Some(dir) = p.parent() {
            install_dirs.push(dir.to_path_buf());
        }
    };
    if let Ok(exe) = std::env::current_exe() {
        push_install(exe);
    }
    // bwrap 解析结果与沙箱执行共用（sandbox::resolve_launcher 的探测缓存）。
    if let Some(bwrap) = crate::sandbox::launcher_path() {
        push_install(bwrap);
    }
    DenyContext {
        anchor: anchor.to_path_buf(),
        config_dir: crate::catalog::config_dir(),
        user_dir: crate::catalog::user_dir(),
        install_dirs,
    }
}

/// §1.3 部署前提检查：应用二进制必须来自固定的不可写安装位置——位于工作区
/// 锚点或系统临时目录之下（可被模型写覆盖的工作区产物）即拒绝启用 031
/// （nlink=1 的 workspace 二进制不能靠硬链接检查兜住；cargo target/debug
/// 的硬链接对只是恰好先撞上该检查）。既有契约的显式执行，非新增限制。
pub fn verify_exe_deployment(anchor: &Path, temp_root: &Path) -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("定位自身二进制失败（{e}）：拒绝启动 031 模式"))?;
    let canon = exe.canonicalize().unwrap_or(exe);
    let anchor_canon = anchor.canonicalize().unwrap_or_else(|_| anchor.to_path_buf());
    let temp_canon = temp_root.canonicalize().unwrap_or_else(|_| temp_root.to_path_buf());
    if canon.starts_with(&anchor_canon) {
        return Err(format!(
            "应用二进制 {} 位于工作区 {} 之内：031 不从可被模型覆盖的工作区产物自再执行\
             （部署到固定不可写安装位置后重试）",
            canon.display(),
            anchor_canon.display()
        ));
    }
    if canon.starts_with(&temp_canon) {
        return Err(format!(
            "应用二进制 {} 位于系统临时目录之下：不是可信部署位置（拒绝启动 031）",
            canon.display()
        ));
    }
    Ok(canon)
}

/// §1.3/P02 部署前提检查：031 的沙箱后端（bwrap）必须已解析且不在模型可写
/// 面之内（§1.3「不通过工作目录或模型 PATH 查找」——模型可替换的 bwrap 是
/// 整个隔离的可绕过原语）。`None`＝后端缺失：按 P02「缺扩展后端」在启动期
/// 明确失败（不留到首个工具调用逐次发作）；位于工作区/系统临时目录之下同样
/// 拒绝启用 031。既有部署契约的显式执行（033 固定 /usr/bin/bwrap 部署记录），
/// 非新增限制。
pub fn verify_bwrap_backend(
    bwrap: Option<&Path>,
    anchor: &Path,
    temp_root: &Path,
) -> Result<(), String> {
    let Some(bwrap) = bwrap else {
        return Err(
            "沙箱后端 bwrap 不可用：031 需要固定沙箱后端（TCHAT_SANDBOX 模式），拒绝启用 031（不回退普通模式）"
                .to_string(),
        );
    };
    let canon = bwrap.canonicalize().unwrap_or_else(|_| bwrap.to_path_buf());
    let anchor_canon = anchor.canonicalize().unwrap_or_else(|_| anchor.to_path_buf());
    let temp_canon = temp_root.canonicalize().unwrap_or_else(|_| temp_root.to_path_buf());
    if canon.starts_with(&anchor_canon) {
        return Err(format!(
            "沙箱后端 {} 位于工作区 {} 之内：不通过模型可写位置查找后端（拒绝启动 031）",
            canon.display(),
            anchor_canon.display()
        ));
    }
    if canon.starts_with(&temp_canon) {
        return Err(format!(
            "沙箱后端 {} 位于系统临时目录之下：不是可信部署位置（拒绝启动 031）",
            canon.display()
        ));
    }
    Ok(())
}

fn policy_dir_canonical(policy_path: &Path) -> PathBuf {
    policy_path
        .parent()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[derive(Debug)]
pub struct RawFsEntry {
    pub path: String,
    pub access: Access,
}

#[derive(Debug)]
pub struct RawRequest {
    pub reason: Option<String>,
    pub entries: Vec<RawFsEntry>,
    pub network: bool,
}

#[derive(Debug)]
pub struct NormalizedEntry {
    pub path: PathBuf,
    pub access: Access,
}

#[derive(Debug)]
pub struct NormalizedRequest {
    pub reason: Option<String>,
    pub entries: Vec<NormalizedEntry>,
    pub network: bool,
}

/// 本轮（turn）累计授权集合：不持久化、不跨轮（§1.7）。
#[derive(Debug, Default, Clone)]
pub struct TurnGrants {
    pub entries: Vec<FsEntry>,
    pub network: bool,
}

impl TurnGrants {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && !self.network
    }

    /// 授予一次已规范化的请求：全部生效或全部失败（§1.7-1）。合并去重后
    /// 超过 [`GRANT_PATHS_MAX`] 条则整次拒绝，不部分应用。
    pub fn grant_all(&mut self, req: &NormalizedRequest) -> Result<(), String> {
        let mut merged = self.entries.clone();
        for entry in &req.entries {
            match merged.iter_mut().find(|e| e.path == entry.path) {
                Some(existing) => {
                    if entry.access == Access::Write {
                        existing.access = Access::Write;
                    }
                }
                None => merged.push(FsEntry {
                    path: entry.path.clone(),
                    access: entry.access,
                }),
            }
        }
        if merged.len() > GRANT_PATHS_MAX {
            return Err(format!(
                "本轮额外授权路径累计 {0} 条，超过上限 {1} 条（去重后计量；拒绝本次授予）",
                merged.len(),
                GRANT_PATHS_MAX
            ));
        }
        self.entries = merged;
        self.network = self.network || req.network;
        Ok(())
    }

    /// 请求是否已被当前授权**完全覆盖**（§1.5：已覆盖的合法请求直接 granted，
    /// 不重复问）。路径按组件包含判定（`/data/a` 不覆盖 `/data/ab`）。
    pub fn covers(&self, req: &NormalizedRequest) -> bool {
        if req.network && !self.network {
            return false;
        }
        req.entries.iter().all(|entry| {
            self.entries
                .iter()
                .any(|g| entry.path.starts_with(&g.path) && g.access.covers(entry.access))
        })
    }
}

// ---------- 严格 JSON（递归重复键拒绝） ----------

/// 递归严格 Value：每个对象层拒绝重复键（serde_json Map 会静默合并——022
/// 配置侧先例在策略/请求侧同样成立）。非对象根由各层校验拒绝。
fn parse_strict_json(text: &str) -> Result<Value, String> {
    use serde::de::Deserializer as _;
    let mut de = serde_json::Deserializer::from_str(text);
    de.deserialize_any(StrictVisitor).map_err(|e| format!("JSON 解析失败：{e}"))
}

struct StrictVisitor;

impl<'de> serde::de::DeserializeSeed<'de> for StrictVisitor {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}

impl<'de> serde::de::Visitor<'de> for StrictVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("任意 JSON 值（对象层拒绝重复键）")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut arr = Vec::new();
        while let Some(item) = seq.next_element_seed(StrictVisitor)? {
            arr.push(item);
        }
        Ok(Value::Array(arr))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut obj = serde_json::Map::new();
        while let Some(k) = map.next_key::<String>()? {
            if obj.contains_key(&k) {
                return Err(serde::de::Error::custom(format!("重复字段 {k:?}")));
            }
            let v = map.next_value_seed(StrictVisitor)?;
            obj.insert(k, v);
        }
        Ok(Value::Object(obj))
    }
}

// ---------- Value 校验辅助 ----------

fn expect_object<'a>(v: &'a Value, ctx: &str) -> Result<&'a serde_json::Map<String, Value>, String> {
    v.as_object()
        .ok_or_else(|| format!("{ctx} 必须是对象（收到 {v}）"))
}

/// 字段集精确匹配：required 必须全部在场，其余键一律拒绝（未知字段不忽略）。
fn expect_exact_fields(
    obj: &serde_json::Map<String, Value>,
    ctx: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<(), String> {
    for r in required {
        if !obj.contains_key(*r) {
            return Err(format!("{ctx} 缺少必填字段 {r:?}"));
        }
    }
    for k in obj.keys() {
        if !required.contains(&k.as_str()) && !optional.contains(&k.as_str()) {
            return Err(format!("{ctx} 不支持字段 {k:?}（未知字段拒绝）"));
        }
    }
    Ok(())
}

fn expect_string<'a>(v: &'a Value, ctx: &str) -> Result<&'a str, String> {
    v.as_str()
        .ok_or_else(|| format!("{ctx} 必须是字符串（收到 {v}）"))
}

/// §1.4 路径/reason 字节规则：非空、≤[`TEXT_MAX_BYTES`]、无 NUL 与 ASCII
/// 控制字符（0x00..=0x1F、0x7F）。中文、空格、字面 `%XX` 原样接受。
fn validate_text_bytes(s: &str, ctx: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err(format!("{ctx} 不能为空"));
    }
    if s.len() > TEXT_MAX_BYTES {
        return Err(format!("{ctx} 超过 {0} UTF-8 字节（实为 {1} 字节）", TEXT_MAX_BYTES, s.len()));
    }
    if s.bytes().any(|b| b <= 0x1F || b == 0x7F) {
        return Err(format!("{ctx} 含 NUL/ASCII 控制字符（拒绝）"));
    }
    Ok(())
}

// ---------- 策略解析 ----------

impl Policy {
    /// 严格解析策略文本（§1.3：version=1；重复键/未知字段/矛盾规则拒绝；
    /// 路径为已存在的绝对实际路径，加载即 canonicalize）。
    pub fn parse(text: &str) -> Result<Policy, String> {
        let root = parse_strict_json(text)?;
        let obj = expect_object(&root, "策略")?;
        expect_exact_fields(obj, "策略", &["version", "base", "requestable", "deny_paths"], &[])?;
        match obj["version"].as_u64() {
            Some(1) => {}
            other => {
                return Err(format!(
                    "策略版本不支持（version={other:?}；仅支持 1）"
                ));
            }
        }
        let base = parse_scope(&obj["base"], "base")?;
        let requestable = parse_scope(&obj["requestable"], "requestable")?;
        let deny_paths = parse_deny_paths(&obj["deny_paths"])?;
        // 显式 deny 与内容面矛盾：deny 路径（递归）之下的 base/requestable 条目
        // ＝「保护的内容又被授权暴露」，加载即拒绝（deny 优先不可折衷）。
        // 自动敏感边界（如工作区 .env）不在此列——那是挂载层 carve-out 的
        // 正常用途（deny 嵌套在允许面之内）。
        for scope in [&base, &requestable] {
            for entry in &scope.entries {
                if deny_paths.iter().any(|d| entry.path == *d || entry.path.starts_with(d)) {
                    return Err(format!(
                        "策略矛盾：{} 条目 {} 位于永久拒绝路径之下（deny 优先，拒绝加载）",
                        if std::ptr::eq(scope, &base) { "base" } else { "requestable" },
                        entry.path.display()
                    ));
                }
            }
        }
        Ok(Policy { base, requestable, deny_paths })
    }

    /// 从可信位置加载：先做信任来源校验（§1.3），再严格解析。
    /// `anchor`＝主进程 cwd（工作区），`temp_root`＝系统临时目录——策略文件
    /// 位于二者之下、或位于自身 base/requestable 的可写面之下，均拒绝。
    pub fn load_verified(path: &Path, anchor: &Path, temp_root: &Path) -> Result<Policy, String> {
        if !path.is_absolute() {
            return Err(format!("策略路径必须是绝对路径（收到 {}）", path.display()));
        }
        let meta = std::fs::metadata(path).map_err(|e| {
            format!("策略文件 {} 不可访问（{e}）：拒绝启动 031 模式", path.display())
        })?;
        // 审查 #5：信任来源检查必须用 canonical 路径——`..`/符号链接的词法
        // 形态可绕过组件前缀比较（与 verify_exe_deployment 对 exe 的处理一致）。
        let path = &path.canonicalize().map_err(|e| {
            format!("策略路径 {} 无法规范化（{e}）：拒绝启动 031 模式", path.display())
        })?;
        if !meta.is_file() {
            return Err(format!("策略路径 {} 不是常规文件", path.display()));
        }
        if meta.len() > POLICY_MAX_BYTES as u64 {
            return Err(format!(
                "策略文件超过 1 MiB 上限（{} 字节）：拒绝加载",
                meta.len()
            ));
        }
        let anchor_canon = anchor.canonicalize().unwrap_or_else(|_| anchor.to_path_buf());
        let temp_canon = temp_root.canonicalize().unwrap_or_else(|_| temp_root.to_path_buf());
        if path.starts_with(&anchor_canon) {
            return Err(format!(
                "策略文件 {} 位于工作区 {} 之下：策略不能来自 cwd 项目配置（拒绝启动 031）",
                path.display(),
                anchor_canon.display()
            ));
        }
        if path.starts_with(&temp_canon) {
            return Err(format!(
                "策略文件 {} 位于系统临时目录 {} 之下：不是可信部署位置（拒绝启动 031）",
                path.display(),
                temp_canon.display()
            ));
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            format!("策略文件 {} 读取失败（{e}）", path.display())
        })?;
        let policy = Policy::parse(&text)?;
        // 自引用：策略位于自身可写面（base 或 requestable 的 write 条目）之下
        // ⇒ 模型最终可改写策略文件，破坏可信部署位置（§1.3）。
        for scope in [&policy.base, &policy.requestable] {
            for entry in &scope.entries {
                if entry.access == Access::Write && path.starts_with(&entry.path) {
                    return Err(format!(
                        "策略文件 {} 位于自身可写条目 {} 之下：可写策略来源拒绝（拒绝启动 031）",
                        path.display(),
                        entry.path.display()
                    ));
                }
            }
        }
        Ok(policy)
    }

    /// 有效 deny 集＝显式 deny_paths ＋自动敏感边界（§1.3），去重。仅保留
    /// 实际存在的路径（缺失＝当下无对象可保护，挂载层按 no-op 处理）。
    pub fn effective_deny_paths(&self, ctx: &DenyContext) -> Vec<PathBuf> {
        let mut deny: Vec<PathBuf> = self.deny_paths.clone();
        deny.push(ctx.anchor.join(".env"));
        deny.push(ctx.config_dir.clone());
        deny.push(ctx.user_dir.clone());
        deny.extend(ctx.install_dirs.iter().cloned());
        let mut out: Vec<PathBuf> = Vec::new();
        for p in deny {
            // 存在性三分类（fail-closed 语义在挂载层；此处 missing＝无对象）
            let Ok(md) = std::fs::metadata(&p) else { continue };
            let _ = md;
            let canon = p.canonicalize().unwrap_or(p);
            if !out.contains(&canon) {
                out.push(canon);
            }
        }
        out.sort_by_key(|p| p.components().count());
        out
    }
}

fn parse_scope(v: &Value, ctx: &str) -> Result<FsScope, String> {
    let obj = expect_object(v, ctx)?;
    expect_exact_fields(obj, ctx, &["file_system", "network"], &[])?;
    let fs = expect_object(&obj["file_system"], &format!("{ctx}.file_system"))?;
    expect_exact_fields(fs, &format!("{ctx}.file_system"), &["entries"], &[])?;
    let arr = obj["file_system"]["entries"]
        .as_array()
        .ok_or_else(|| format!("{ctx}.file_system.entries 必须是数组"))?;
    let net_obj = expect_object(&obj["network"], &format!("{ctx}.network"))?;
    expect_exact_fields(net_obj, &format!("{ctx}.network"), &["enabled"], &[])?;
    let network = net_obj["enabled"]
        .as_bool()
        .ok_or_else(|| format!("{ctx}.network.enabled 必须是布尔值"))?;
    let mut entries = Vec::new();
    for (i, e) in arr.iter().enumerate() {
        let ectx = format!("{ctx}.file_system.entries[{i}]");
        let eobj = expect_object(e, &ectx)?;
        expect_exact_fields(eobj, &ectx, &["path", "access"], &[])?;
        let path = parse_path_spec(&eobj["path"], &format!("{ectx}.path"))?;
        let access = parse_access(&eobj["access"], &ectx)?;
        entries.push(FsEntry { path, access });
    }
    dedup_entries(&mut entries, ctx)?;
    Ok(FsScope { entries, network })
}

fn parse_path_spec(v: &Value, ctx: &str) -> Result<PathBuf, String> {
    let obj = expect_object(v, ctx)?;
    expect_exact_fields(obj, ctx, &["type", "path"], &[])?;
    let ty = expect_string(&obj["type"], &format!("{ctx}.type"))?;
    if ty != "path" {
        return Err(format!("{ctx}.type 只支持 \"path\"（收到 {ty:?}）"));
    }
    let raw = expect_string(&obj["path"], &format!("{ctx}.path"))?;
    validate_text_bytes(raw, &format!("{ctx}.path"))?;
    let p = Path::new(raw);
    if !p.is_absolute() {
        return Err(format!("{ctx}.path 必须是绝对路径（收到 {raw}）"));
    }
    // 已存在的绝对实际路径（§1.3）：加载即验证存在并固定 canonical 形态。
    match std::fs::metadata(p) {
        Ok(_) => p
            .canonicalize()
            .map_err(|e| format!("{ctx}.path {} 无法规范化（{e}）", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!(
            "{ctx}.path {} 不存在：策略路径必须是已存在的绝对实际路径",
            p.display()
        )),
        Err(e) => Err(format!("{ctx}.path {} 状态读取失败（{e}）：拒绝", p.display())),
    }
}

fn parse_access(v: &Value, ctx: &str) -> Result<Access, String> {
    let s = expect_string(v, &format!("{ctx}.access"))?;
    match s {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(format!("{ctx}.access 只支持 \"read\"/\"write\"（收到 {other:?}）")),
    }
}

fn parse_deny_paths(v: &Value) -> Result<Vec<PathBuf>, String> {
    let arr = v
        .as_array()
        .ok_or_else(|| "deny_paths 必须是数组".to_string())?;
    let mut out = Vec::new();
    for (i, e) in arr.iter().enumerate() {
        let raw = expect_string(e, &format!("deny_paths[{i}]"))?;
        validate_text_bytes(raw, &format!("deny_paths[{i}]"))?;
        let p = Path::new(raw);
        if !p.is_absolute() {
            return Err(format!(
                "deny_paths[{i}] 必须是绝对路径（收到 {raw}）"
            ));
        }
        match std::fs::metadata(p) {
            Ok(_) => out.push(p.canonicalize().map_err(|e| {
                format!("deny_paths[{i}] {} 无法规范化（{e}）", p.display())
            })?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "deny_paths[{i}] {} 不存在：策略路径必须是已存在的绝对实际路径",
                    p.display()
                ));
            }
            Err(err) => {
                return Err(format!(
                    "deny_paths[{i}] {} 状态读取失败（{err}）：拒绝",
                    p.display()
                ));
            }
        }
    }
    // deny 集去重（同一路径重复列出不是矛盾）
    out.dedup();
    Ok(out)
}

/// 同一路径的重复规则：完全相同→去重；access 不同→矛盾拒绝（§1.3）。
fn dedup_entries(entries: &mut Vec<FsEntry>, ctx: &str) -> Result<(), String> {
    let mut out: Vec<FsEntry> = Vec::new();
    for e in entries.drain(..) {
        match out.iter_mut().find(|x| x.path == e.path) {
            Some(existing) => {
                if existing.access != e.access {
                    return Err(format!(
                        "{ctx} 中路径 {} 存在互相矛盾的重复规则（{:?} vs {:?}）",
                        e.path.display(),
                        existing.access,
                        e.access
                    ));
                }
            }
            None => out.push(e),
        }
    }
    *entries = out;
    Ok(())
}

// ---------- 请求解析（§1.4） ----------

/// 解析工具 arguments 原始 JSON。所有层级拒绝未知字段与重复键；数组/标量
/// 根形态拒绝（serde 派生会接受数组位置形式——030 教训，手写校验规避）。
pub fn parse_request(raw: &str) -> Result<RawRequest, String> {
    if raw.len() > ARGS_MAX_BYTES {
        return Err(format!(
            "arguments 超过 {0} KiB（实为 {1} 字节）：整次拒绝，不截断",
            ARGS_MAX_BYTES / 1024,
            raw.len()
        ));
    }
    let root = parse_strict_json(raw)?;
    let obj = expect_object(&root, "请求")?;
    expect_exact_fields(obj, "请求", &["permissions"], &["reason"])?;
    let reason = match obj.get("reason") {
        None => None,
        Some(v) => {
            let s = expect_string(v, "reason")?;
            validate_text_bytes(s, "reason")?;
            Some(s.to_string())
        }
    };
    let perm = expect_object(&obj["permissions"], "permissions")?;
    expect_exact_fields(perm, "permissions", &[], &["file_system", "network"])?;
    let mut entries = Vec::new();
    let mut network = false;
    if let Some(fs) = perm.get("file_system") {
        let fs_obj = expect_object(fs, "permissions.file_system")?;
        expect_exact_fields(fs_obj, "permissions.file_system", &["entries"], &[])?;
        let arr = fs_obj["entries"]
            .as_array()
            .ok_or_else(|| "permissions.file_system.entries 必须是数组".to_string())?;
        if arr.is_empty() {
            return Err("permissions.file_system.entries 不能为空（与 network 至少一项）".to_string());
        }
        if arr.len() > ENTRIES_MAX {
            return Err(format!(
                "permissions.file_system.entries 超过 {0} 项（实为 {1} 项）：拒绝",
                ENTRIES_MAX,
                arr.len()
            ));
        }
        for (i, e) in arr.iter().enumerate() {
            let ectx = format!("permissions.file_system.entries[{i}]");
            let eobj = expect_object(e, &ectx)?;
            expect_exact_fields(eobj, &ectx, &["path", "access"], &[])?;
            let pobj = expect_object(&eobj["path"], &format!("{ectx}.path"))?;
            expect_exact_fields(pobj, &format!("{ectx}.path"), &["type", "path"], &[])?;
            let ty = expect_string(&pobj["type"], &format!("{ectx}.path.type"))?;
            if ty != "path" {
                return Err(format!("{ectx}.path.type 只支持 \"path\"（收到 {ty:?}）"));
            }
            let path = expect_string(&pobj["path"], &format!("{ectx}.path.path"))?;
            validate_text_bytes(path, &format!("{ectx}.path.path"))?;
            let access = parse_access(&eobj["access"], &ectx)?;
            entries.push(RawFsEntry {
                path: path.to_string(),
                access,
            });
        }
    }
    if let Some(net) = perm.get("network") {
        let net_obj = expect_object(net, "permissions.network")?;
        expect_exact_fields(net_obj, "permissions.network", &["enabled"], &[])?;
        match net_obj["enabled"].as_bool() {
            Some(true) => network = true,
            _ => {
                return Err(
                    "permissions.network 只支持 {\"enabled\": true}（不以 false/null 隐式撤销）"
                        .to_string(),
                );
            }
        }
    }
    if entries.is_empty() && !network {
        return Err("请求至少需要一项实际权限（file_system.entries 或 network）".to_string());
    }
    Ok(RawRequest { reason, entries, network })
}

/// 路径规范化（§1.4）：相对路径基于会话 cwd 解析（不基于上一条 shell 的
/// cd）；词法消除 `.`/`..` 后**逐组件**验证存在且无符号链接；不做 URL
/// 解码、Unicode 归一化或环境展开；重复项去重、同路径 write 覆盖 read。
pub fn normalize_request(req: &RawRequest, cwd: &Path) -> Result<NormalizedRequest, String> {
    let mut entries: Vec<NormalizedEntry> = Vec::new();
    for raw in &req.entries {
        let p = Path::new(&raw.path);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        };
        let lex = crate::sandbox::absolute_lexical(&joined);
        let mut cur = PathBuf::from("/");
        for comp in lex.components() {
            match comp {
                std::path::Component::Normal(part) => {
                    cur.push(part);
                    match std::fs::symlink_metadata(&cur) {
                        Ok(md) => {
                            if md.file_type().is_symlink() {
                                return Err(format!(
                                    "路径 {} 含符号链接组件 {}：逐组件验证拒绝（不解码、不跟随）",
                                    lex.display(),
                                    cur.display()
                                ));
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            return Err(
                                "路径组件不存在：缺失输出目录应另行申请已存在父目录".to_string(),
                            );
                        }
                        Err(e) => {
                            return Err("路径组件状态不可判定：拒绝".to_string());
                        }
                    }
                }
                std::path::Component::RootDir => {}
                _ => {}
            }
        }
        match entries.iter_mut().find(|e| e.path == lex) {
            Some(existing) => {
                if raw.access == Access::Write {
                    existing.access = Access::Write;
                }
            }
            None => entries.push(NormalizedEntry {
                path: lex,
                access: raw.access,
            }),
        }
    }
    Ok(NormalizedRequest {
        reason: req.reason.clone(),
        entries,
        network: req.network,
    })
}

// ---------- 结果结构（§1.5） ----------

fn permissions_json(req: &NormalizedRequest) -> Value {
    let mut perm = serde_json::Map::new();
    if !req.entries.is_empty() {
        perm.insert(
            "file_system".to_string(),
            serde_json::json!({
                "entries": req.entries.iter().map(|e| serde_json::json!({
                    "path": { "type": "path", "path": e.path.display().to_string() },
                    "access": e.access.as_str(),
                })).collect::<Vec<_>>()
            }),
        );
    }
    if req.network {
        perm.insert("network".to_string(), serde_json::json!({ "enabled": true }));
    }
    Value::Object(perm)
}

/// granted：permissions 为本次实际规范化、完整覆盖的请求；不含可重放令牌。
pub fn granted_result(req: &NormalizedRequest) -> String {
    serde_json::json!({
        "status": "granted",
        "permissions": permissions_json(req),
        "scope": "turn",
        "error": null,
    })
    .to_string()
}

/// denied：用户明确拒绝；不扩大当前授权。
pub fn denied_result() -> String {
    serde_json::json!({
        "status": "denied",
        "permissions": {},
        "scope": "turn",
        "error": null,
    })
    .to_string()
}

/// error：参数/策略/后端错误、取消或宿主断开；error 为脱敏原因。
pub fn error_result(reason: &str) -> String {
    // 错误文案经终端显示前仍会过 display 过滤；此处先做长度收敛。
    let brief: String = reason.chars().take(300).collect();
    serde_json::json!({
        "status": "error",
        "permissions": {},
        "scope": "turn",
        "error": brief,
    })
    .to_string()
}

// ---------- 环境装配（§1.2） ----------

/// 读取 TCHAT_PERMISSION_POLICY：
/// - 未设置 → `Ok(None)`（普通/033 模式行为不变，不声明本工具）；
/// - 已设置 → 必须已启用 `TCHAT_SANDBOX` 且策略通过信任来源校验与严格解析，
///   任一失败 → `Err`（启动明确失败，不回退普通 shell 或 033 固定策略）。
pub fn policy_from_env(anchor: &Path) -> Result<Option<(Policy, PathBuf)>, String> {
    let Ok(raw) = std::env::var(POLICY_ENV_VAR) else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Err("TCHAT_PERMISSION_POLICY 已设置但为空：拒绝启动 031 模式".to_string());
    }
    if !crate::sandbox::sandbox_requested() {
        return Err(format!(
            "TCHAT_PERMISSION_POLICY 需要 TCHAT_SANDBOX 同时启用（031 以 033 沙箱后端为前提）：拒绝启动 031 模式"
        ));
    }
    let path = PathBuf::from(raw.trim());
    Policy::load_verified(&path, anchor, &std::env::temp_dir()).map(|p| Some((p, path)))
}

// ---------- 工具定义（声明面，§1.2-4/§1.4） ----------

/// request_permissions 工具定义（普通函数适配五协议；始终直接声明，不进入
/// 030 延迟工具目录）。声明门控（§1.2-4）：031 后端可用＋普通用户轮＋模型
/// `experimental_supported_tools` 精确包含工具名＋专用交互宿主。
pub fn tool_definition() -> Value {
    serde_json::json!({"type":"function","function":{
        "name": TOOL_NAME,
        "description":"Request additional local file read/write or network permission for the current user turn. The user must explicitly approve each request in a dedicated confirmation UI; approval covers only the current turn and already-granted items are not re-asked. Paths must be absolute existing real paths (relative paths resolve against the session cwd); symlinks and missing components are rejected. write includes read. Permanent deny paths, admin limits and existing OS permissions always stay in force.",
        "parameters":{"type":"object","properties":{
            "reason":{"type":"string","description":"Optional short explanation (max 4096 UTF-8 bytes) shown to the user as untrusted notes."},
            "permissions":{"type":"object","properties":{
                "file_system":{"type":"object","properties":{
                    "entries":{"type":"array","description":"1-32 path permission requests","items":{"type":"object","properties":{
                        "path":{"type":"object","properties":{
                            "type":{"type":"string","const":"path"},
                            "path":{"type":"string","description":"Absolute existing real path (or relative to session cwd); no glob, URI, ~ or variable expansion."}
                        },"required":["type","path"],"additionalProperties":false},
                        "access":{"type":"string","enum":["read","write"]}
                    },"required":["path","access"],"additionalProperties":false}}
                },"required":["entries"],"additionalProperties":false},
                "network":{"type":"object","properties":{
                    "enabled":{"type":"boolean","const":true,"description":"Request broad IP network access for local commands this turn."}
                },"required":["enabled"],"additionalProperties":false}
            },"additionalProperties":false}
        },"required":["permissions"],"additionalProperties":false}
    }})
}

// ---------- 确认宿主（§1.5，M3） ----------

/// 用户对一次权限申请的决定（typed 回执：普通聊天、排队消息、模型伪造的
/// “用户已同意”都不能构造出 Allow）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny,
}

/// 确认界面的展示数据（宿主过滤前原文由宿主侧自行处理显示边界）。
pub struct PermissionRequestView {
    /// 会话 cwd（申请解析基准，随申请展示）。
    pub cwd: PathBuf,
    /// 规范化后的路径与访问级别（用户所见＝实际批准对象）。
    pub entries: Vec<(PathBuf, Access)>,
    pub network: bool,
    /// 模型 reason（未信任说明，单独标注展示）。
    pub reason: Option<String>,
}

/// 权限确认宿主（§1.5）：默认拒绝、无超时批准；一个宿主同一时刻只允许一
/// 个权限确认。EOF/宿主断开＝Err（结果 error，不伪造允许）。
pub trait PermissionHost {
    /// 展示完整权限项目并阻塞等待用户回执。返回 Err 的文案进入 error 结果
    ///（脱敏由 [`error_result`] 收敛）。
    fn confirm(&mut self, view: &PermissionRequestView) -> Result<PermissionDecision, String>;
}

// ---------- 轮内权限状态（§1.7，M4） ----------

/// 一轮内的可变权限状态：本轮授权集合＋确认宿主。每用户轮新建（新轮授权
/// 集合为空，§1.8）；成功/取消/失败/切换/退出的全部出口统一收尾（到期收回，
/// 由 ProcessManager 的额外权限回收执行）。
pub struct TurnPermState<'a> {
    pub grants: TurnGrants,
    pub host: &'a mut dyn PermissionHost,
}

/// TurnExtras.permissions 的载荷：专用交互宿主（§1.2-4 的宿主半边；后端与
/// 模型能力半边由 turn 装配时核对）。
pub struct PermissionExtras<'a> {
    pub host: &'a mut dyn PermissionHost,
}

// ---------- 冻结策略与执行快照（§1.2/§1.6） ----------

/// 启动时冻结的 031 策略（策略＋自动敏感边界装配上下文）。CLI/TUI 分流前
/// 统一装配，两宿主取得同一快照；dotenv 不能重配信任根（TRUST_CONTROL_VARS）。
pub struct FrozenPolicy {
    pub policy: Policy,
    pub deny_ctx: DenyContext,
}

static FROZEN: std::sync::Mutex<Option<std::sync::Arc<FrozenPolicy>>> =
    std::sync::Mutex::new(None);

/// 启动装配（幂等；main 分流前调用一次）：成功后进程内固定。失败＝Err，
/// 宿主必须以明确错误退出 031 模式，不进入交互循环。
pub fn install_from_env(anchor: &Path) -> Result<Option<std::sync::Arc<FrozenPolicy>>, String> {
    let mut guard = FROZEN.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(f) = guard.as_ref() {
        return Ok(Some(f.clone()));
    }
    let installed = match policy_from_env(anchor)? {
        None => None,
        Some((policy, path)) => {
            // §1.3 部署前提：二进制不在工作区/临时目录（先于策略装配检查）
            verify_exe_deployment(anchor, &std::env::temp_dir())?;
            // §1.3/P02：沙箱后端必须已解析且不在模型可写面之内（缺后端/
            // 可替换后端＝部署错误，启动期明确失败，零模型命令执行）。
            verify_bwrap_backend(
                crate::sandbox::launcher_path().as_deref(),
                anchor,
                &std::env::temp_dir(),
            )?;
            // 审查 #12：路径单次读取（trim 后的原值）——二次不 trim 地重读会使
            // 带空白的值装配出错误的 deny 安装目录（静默落空）。
            let frozen = std::sync::Arc::new(FrozenPolicy {
                deny_ctx: deny_context_for(anchor, &path),
                policy,
            });
            // 启动期基线预检（安全攻防 H1 修复配套）：空授权的生效快照必须可
            // 编码——部署拓扑使自动敏感边界成为内容面祖先等情形，启动即明确
            // 失败（§1.2-2「策略不可执行时启动明确失败」），不留到工具调用期
            // 以毒丸形态逐面发作。
            baseline_snapshot(&frozen)?;
            Some(frozen)
        }
    };
    *guard = installed.clone();
    Ok(installed)
}

/// 已冻结的 031 策略（None＝未启用 031 模式）。
pub fn frozen() -> Option<std::sync::Arc<FrozenPolicy>> {
    FROZEN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}


/// 基线快照：无本轮授权的执行面（MCP stdio 等长期连接用——不继承任何
/// 本轮临时授权，§1.6）。
pub fn baseline_snapshot(frozen: &FrozenPolicy) -> Result<crate::sandbox::PolicySnapshot, String> {
    build_snapshot(&frozen.policy, &TurnGrants::new(), &frozen.deny_ctx)
}

/// 生效策略快照：基线＋本轮授权合并为出生冻结的挂载面（§1.6）。同时完成
/// 「提交 granted 前」的可表达性校验（§1.7-1）：授权必须是管理员上限的
/// 子集（重断言，不信任调用方）；内容面与有效 deny 的矛盾拒绝；网络位合并。
pub fn build_snapshot(
    policy: &Policy,
    grants: &TurnGrants,
    ctx: &DenyContext,
) -> Result<crate::sandbox::PolicySnapshot, String> {
    for g in &grants.entries {
        let ok = policy.requestable.entries.iter().any(|r| {
            g.path.starts_with(&r.path) && r.access.covers(g.access)
        });
        if !ok {
            return Err(format!(
                "授权路径 {} 超出管理员 requestable 上限：无法编码为执行快照",
                g.path.display()
            ));
        }
    }
    if grants.network && !policy.requestable.network {
        return Err("网络授权超出管理员 requestable 上限：无法编码为执行快照".to_string());
    }
    let mut read_paths: Vec<PathBuf> = policy
        .base
        .entries
        .iter()
        .filter(|e| e.access == Access::Read)
        .map(|e| e.path.clone())
        .collect();
    let mut write_paths: Vec<PathBuf> = policy
        .base
        .entries
        .iter()
        .filter(|e| e.access == Access::Write)
        .map(|e| e.path.clone())
        .collect();
    for g in &grants.entries {
        match g.access {
            Access::Write => write_paths.push(g.path.clone()),
            Access::Read => read_paths.push(g.path.clone()),
        }
    }
    read_paths.dedup();
    write_paths.dedup();
    // 审查 #15：read 严格位于 write 之下＝冗余授权，化简掉（§1.4「write 包含
    // 读取及该路径内文件变更能力」的单调授权模型：access 级别只累加不互斥，
    // 收窄原语只有 deny——read 不构成「只读 carve-out」）。不化简时，挂载面
    // 按深度升序组装会使该冗余 read 以 ro-bind 后挂遮蔽祖先写面，用户已批准
    // 的父目录写权限在子树内静默 EROFS，且 grants.covers 判「已覆盖」使再次
    // 申请无法修复（§1.6「路径批准对象与实际挂载/读写对象须绑定」「不能表达
    // 的策略不得静默降级」）。
    read_paths.retain(|r| !write_paths.iter().any(|w| r == w || r.starts_with(w)));
    let deny_paths = policy.effective_deny_paths(ctx);
    // 有效 deny（含自动敏感边界）与内容面矛盾：内容路径位于 deny 之下＝保护
    // 的内容又被授权暴露（解析层已拒显式组合；此处覆盖自动边界与部署拓扑）。
    for p in read_paths.iter().chain(write_paths.iter()) {
        if deny_paths.iter().any(|d| p == d || p.starts_with(d)) {
            return Err(format!(
                "策略矛盾：内容路径 {} 位于永久拒绝路径之下（deny 优先，拒绝启动）",
                p.display()
            ));
        }
    }
    // 启动 argv 容量校验（§1.7-1）：每条内容路径在 bwrap argv 中约出现 4 次
    //（bind 源/目标＋别名遮蔽）；总量超过保守上限即拒绝——不把 E2BIG 留到
    // spawn 时才发现（那时模型命令已进入半执行状态）。
    let argv_bytes: usize = read_paths
        .iter()
        .chain(write_paths.iter())
        .map(|p| p.as_os_str().len() * 4 + 32)
        .sum();
    if argv_bytes > 1_000_000 {
        return Err(format!(
            "生效策略的启动 argv 容量超限（约 {argv_bytes} 字节 > 1000000）：拒绝编码"
        ));
    }
    Ok(crate::sandbox::PolicySnapshot {
        read_paths,
        write_paths,
        deny_paths,
        network: policy.base.network || grants.network,
        poison: None,
    })
}

// ---------- 申请执行（分派层，§1.4/§1.5/§1.7） ----------

/// 基线是否完全覆盖请求（§1.5：已覆盖的合法请求直接 granted，不重复问）。
pub fn base_covers(policy: &Policy, req: &NormalizedRequest) -> bool {
    req.entries.iter().all(|e| {
        policy
            .base
            .entries
            .iter()
            .any(|b| e.path.starts_with(&b.path) && b.access.covers(e.access))
    }) && (!req.network || policy.base.network)
}

/// 执行一次 request_permissions 调用（分派层入口）：解析→规范化→上限/覆盖
/// 判定→宿主确认→**预演验证后原子提交**（§1.7-1：单次允许全部生效或全部
/// 失败；提交前完成生效策略编码与可表达性校验，失败不发布新增许可）。
/// 返回结果 JSON 字符串（四字段结构）。
pub fn execute_request_permissions(
    raw_arguments: &str,
    cwd: &Path,
    policy: &Policy,
    grants: &mut TurnGrants,
    host: &mut dyn PermissionHost,
    deny_ctx: &DenyContext,
) -> String {
    let req = match parse_request(raw_arguments) {
        Ok(r) => r,
        Err(e) => return error_result(&e),
    };
    let norm = match normalize_request(&req, cwd) {
        Ok(n) => n,
        Err(e) => return error_result(&e),
    };
    // 管理员上限预检（避免无谓弹窗；提交时重断言）。
    for e in &norm.entries {
        let ok = policy.requestable.entries.iter().any(|r| {
            e.path.starts_with(&r.path) && r.access.covers(e.access)
        });
        if !ok {
            return error_result(&format!(
                "请求路径 {} 超出管理员允许申请的上限（requestable）",
                e.path.display()
            ));
        }
    }
    if norm.network && !policy.requestable.network {
        return error_result("网络权限不在管理员允许申请的范围内（requestable）");
    }
    // 永久拒绝优先（§1.4；安全攻防 L1 修复）：请求条目命中有效 deny 集
    //（显式或自动敏感边界）＝整次 error，不弹确认；已授权祖先也不能把嵌套
    // 的永久拒绝「覆盖」成 granted（执行层遮蔽仍然有效，此处消除结果误导）。
    let deny = policy.effective_deny_paths(deny_ctx);
    for e in &norm.entries {
        if deny.iter().any(|d| e.path == *d || e.path.starts_with(d)) {
            return error_result(&format!(
                "请求路径 {} 位于永久拒绝路径之下：永久拒绝优先，不可申请",
                e.path.display()
            ));
        }
    }
    // 已被基线或当前有效授权完全覆盖：直接 granted，不重复问。
    if base_covers(policy, &norm) || grants.covers(&norm) {
        return granted_result(&norm);
    }
    // §1.7-1（审查 #11）：超限在**确认前**失败——预演试算命中 64 上限即
    // 整次 error，不弹确认窗（用户不应为必然失败的申请按下允许）。
    {
        let mut trial = TurnGrants {
            entries: grants.entries.clone(),
            network: grants.network,
        };
        if let Err(e) = trial.grant_all(&norm) {
            return error_result(&e);
        }
    }
    // 宿主确认（默认拒绝；Err＝error，不扩大授权）。
    let view = PermissionRequestView {
        cwd: cwd.to_path_buf(),
        entries: norm.entries.iter().map(|e| (e.path.clone(), e.access)).collect(),
        network: norm.network,
        reason: norm.reason.clone(),
    };
    let decision = match host.confirm(&view) {
        Ok(d) => d,
        Err(e) => return error_result(&format!("权限确认宿主已断开：{e}")),
    };
    match decision {
        PermissionDecision::Deny => denied_result(),
        PermissionDecision::Allow => {
            // 预演：授予＋编码＋可表达性全部通过后才提交（原子）。
            let mut trial = TurnGrants {
                entries: grants.entries.clone(),
                network: grants.network,
            };
            if let Err(e) = trial.grant_all(&norm) {
                return error_result(&e);
            }
            if let Err(e) = build_snapshot(policy, &trial, deny_ctx) {
                return error_result(&format!("生效策略编码失败，本次新增许可未发布：{e}"));
            }
            *grants = trial;
            granted_result(&norm)
        }
    }
}
