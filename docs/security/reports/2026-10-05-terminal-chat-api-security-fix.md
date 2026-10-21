# F1～F7 安全修复记录（对应 2026-10-05 攻防测试报告）

> 2026-10-19 证据归类：当前捕获与结构化结果见[安全证据说明](../evidence/2026-10-05/README.md)，历史 `logs/security-20261005/`／`docs/records/security-20261005/`、脚本及运行命令对应[原始快照](../../records/evidence-index.md#security-originals)，不作为当前测试入口。下文历史结论与修订记录保持原意。

- **日期**：2026-10-05（收尾修订：同日）
- **依据**：[2026-10-05-terminal-chat-api-security.md](2026-10-05-terminal-chat-api-security.md)
- **范围**：仅修复 F1～F7；F8（参数取值校验）、F9（客户端限流）按授权边界本轮不改，保持信息级记录
- **方法**：每项先补真实回归测试（红：修复前因目标漏洞失败；绿：修复后通过），再实现；全程使用本机模拟服务、假 Key、独立临时目录

## 测试结果总览

| 套件 | 结果 |
|---|---|
| `cargo check --locked` | 通过，无警告 |
| `cargo test --locked` | 90 通过（单元 41 + 集成 49），0 失败 |
| `uv run pytest -q` | 2 通过 |
| 攻防用例（logs/security-20261005/attack.py，最终版） | **38 条全部完成**：PASS 31、WARN 1（C8d，F8 范围信息项）、INFO 2（C8b/C15）、OK 4（N1～N4）；0 漏洞确认，逐 ID 清单见文末附录 A |
| 全局泄露断言 | 全部用例输出中无伪造密钥、受害文件字段、真实 .env 密钥明文（`results.json` `leak_check: []`） |

> **数量勘误**：本报告初稿写"37 条"，原因是初稿写于补回第 34 条原始用例 `C9-https 任意主机（无白名单）` 之前；最终 `results.json` 为 38 条（原 34 条 + N1～N4），已逐 ID 核对齐全（见附录 A）。

## 逐项修复

### F1 配置篡改导致 Key 外带 —— 已修复（原：高危）

- **修改**：`src/catalog.rs` 新增授权记录 `OriginGrant`（origin + 凭证 SHA-256 指纹），存放于用户私有目录 `authorized-origins.json`（0600，项目配置之外，`manual.json` 无权自行授予）；`ensure_origin_authorized`（已授权放行；未授权时交互模式询问 y/N 并持久化，非交互模式直接拒绝，fail-closed）；`origin_of`（origin 归一化，默认端口省略）；`refresh()` 注入 `authorize` 回调，每个带凭证的模型查询前检查，拒绝则进入 failed 且不发请求。`src/api.rs`：agent 改为 `redirects(0)`，`send`/`chat` 两条路径对 3xx 明确拒绝（"拒绝跟随重定向"），凭证永远不会发往重定向目标。`src/main.rs`：`cmd_chat`/`cmd_refresh` 发送凭证前统一过授权门（`io::stdin().is_terminal()` 判定交互性）。
- **红→绿**：红 = 攻防用例 C1（修复前 Key 原样送达攻击者 mock，修复前实测复跑确认，归档 `v1-prefix-evidence/`）；绿 = `refresh_refuses_unauthorized_origin_without_request`（拒绝时 0 请求）、`origin_grant_store_semantics`（换 Key/换地址必须重新授权、记录损坏 fail-closed、0600 落盘）、`redirect_targets_never_receive_credentials` / `list_models_refuses_redirects`（重定向目标 0 请求）。
- **保留合法场景**：预授权后正常对话（C1 正常分支、`bin_pipe_session_chats_and_lists_models` 预授权适配）、自定义厂商/代理（N2）、PTY 交互确认后持久化授权（N4，伪终端实测）。

### F2 文件凭证越界读取 —— 已修复（原：高危）

- **修改**：`src/catalog.rs` 新增 `user_dir()`（`TCHAT_USER_DIR` 覆盖；默认 Unix `$HOME/.config/terminal-chat`，Windows `%APPDATA%\terminal-chat`）与 `credential_dir()`（专用凭证目录）；`resolve_credential` 的 `file:` 分支重写：目录缺失报迁移提示；相对路径按专用目录解析；绝对路径先 `fs::canonicalize` 解析软链接与 `..`，再做**组件级**包含校验（非字符串前缀）。
- **红→绿**：红 = `file_credential_confined_to_credential_dir`（修复前目录外文件被正常读取）；绿 = 同测试覆盖目录外拒绝/目录内可读/相对路径解析/`../` 穿越/软链接逃逸五类场景。
- **迁移方式**：旧绝对路径引用会得到明确错误："凭证文件必须位于专用凭证目录 … 内；迁移方式：将文件移动到该目录后改用 `file:文件名#字段`"。

### F3 终端控制序列注入 —— 已修复（原：中危）

- **修改**：新增 `src/display.rs` 显示边界模块：`sanitize_for_terminal` 完整解析并剥离 ECMA-48 序列（CSI 参数/中间/终止字节、OSC/DCS/SOS/PM/APC 字符串序列含 BEL 与 ST 终止、两字符转义如 `ESC c` 终端复位）、全部 C0/C1/DEL 控制字符；保留中英文、换行、Tab、其他可打印字符；`truncate_chars` 按字符截断（中文安全）。`src/api.rs`：`describe_status_error` 的 JSON `error.message` 与原始响应统一截断 300 字符。`src/main.rs`：全部不可信输出（模型回复、错误、`/models` 元数据、`/refresh` 报告）经 `say()` 过滤。
- **红→绿**：红 = `json_error_message_capped_at_300_chars`（修复前 320 字符全量回显）＋攻防用例 C3/C4/C12 修复前实测；绿 = display 模块 7 个单元测试（序列类型逐类覆盖、载荷不残留、正文保留）＋C3/C4 复跑（输出无任何 ESC/BEL，正文保留）。
- **注意**：OSC 序列的整个载荷（含文本）都会被剥离——这是正确行为（OSC 载荷属于序列而非正文）。

### F4 公共目录投毒 —— 已修复（原：中危；收尾阶段收紧测试例外）

- **修改**：
  - **正式路径一律强制 HTTPS**：`validate_public_catalog_url` 只接受 `https:`，本机回环 http 也不例外，**无任何环境开关**。收尾前版本曾提供 `TCHAT_PUBLIC_CATALOG_INSECURE_LOOPBACK=1` 环境开关放行回环 http——收尾审查认定"环境开关在正式二进制中可用"不满足"仅用于测试"，已从产品代码完全移除；设置该变量不再影响任何判定（有专项测试与 E2E 证明，见下）。
  - **测试注入替代开关**：`sync_public` 新增 `url_override: Option<&str>` 参数（`#[doc(hidden)]`，仅供测试注入本机模拟目录地址，注入路径跳过 HTTPS 校验但仍做 origin 授权与字段校验）；`refresh` 透传该参数；正式二进制（`src/main.rs`）固定传 `None`。本机 http 模拟的"公共目录正常同步"能力由 Rust 集成测试经注入保留（如 `public_sync_requires_authorization_and_validates_fields`、`refresh_success_*`）。
  - **入库前校验**：`valid_model_id`（拒绝控制字符/空白/超长 id，非法标识不入库）＋展示名清洗（`sanitize_for_terminal`）并限长 200；`list_models` 同样校验（非法 id 整次查询失败，保持"名单完整性"既有哲学）并清洗名称。
  - **重定向**：全局 `redirects(0)`，3xx 拒绝（与 F1 共用机制）。
- **正式路径拒绝明文 HTTP 的验证**（收尾新增）：
  - 集成测试 `production_catalog_path_rejects_plaintext_http_even_with_stale_switch`：正式路径（`None`）+ 明文 http 目录 + 旧开关环境变量仍存在 → 同步被拒（提示 HTTPS）、明文目录 0 次被访问、旧目录保留、恶意数据不入库；
  - 单元测试 `public_catalog_url_rules`：回环 http 拒绝，且设置旧开关变量后判定不变（变量已不被读取）；
  - E2E 攻防用例 C12：真实二进制在"带旧开关"与"不带"两种场景下均拒绝，明文目录 0 次被访问，`auto_catalog.json` 未被投毒。
- **红→绿**：红 = 攻防用例 C12 修复前复跑（明文 http 拉取成功＋恶意字段落盘，归档 `v1-prefix-evidence/`）＋收尾时确认正式二进制曾接受该开关；绿 = 上述全部测试。

### F5 分页游标注入查询参数 —— 已修复（原：低危）

- **修改**：`src/api.rs` `list_models` 分页 URL 改用 `url` crate 查询参数接口（`Url::query_pairs_mut().append_pair("after", cursor)`，每页先清空 query），取消字符串拼接。
- **红→绿**：红 = `pagination_cursor_is_single_encoded_param`（修复前游标 `a&b#c d模式` 的 `&` 被拆成第二个查询参数）；绿 = 同测试：`&`、`#`、中文整体编码为单个参数值且可无损还原；攻防用例 C5 六种变体全部通过。
- **边界说明**：空白与控制字符 id 现被 F4 入库校验直接拒绝，因此空格**不可能再作为游标出现**（其被拒绝的行为已验证：C5-space）；`&`/`#`/中文等合法字符经真实流程验证单参数无损传递。

### F6 超大响应耗尽内存 —— 已修复（原：低危）

- **修改**：`src/api.rs` 新增 `MAX_BODY_BYTES = 10 MiB` 与 `LimitedReader`（对 `into_reader()` 增量读取并按**实际解码字节**计数，不依赖 Content-Length、不先整包载入；超限立即返回 InvalidData）；`read_limited_string` 统一用于聊天成功/错误响应、模型列表、公共目录（后两者经 `send()`）；错误路径保留状态码分类（"状态码 401：响应体超过 10 MiB 上限…"）。单元测试覆盖恰好达到上限/超 1 字节边界。
- **红→绿**：红 = 三个集成测试（11 MiB 聊天回复/名单/错误体修复前均被完整读入并成功）；绿 = 全部通过（C7 攻防用例 11MB/30MB 亦确认超限报错）。

### F7 配置和凭证权限过宽 —— 已修复（Unix 实测；Windows 与上级目录风险如实列出）

- **修改**：`src/catalog.rs` 权限边界：`ensure_private_dir`（0700）、`write_private_file`（0600，对**已存在文件**也显式收紧——OpenOptions 的 mode 只在创建时生效，收尾修正）、`apply_mode`（**设置后重新读取元数据确认**，未生效即报错"权限设置未生效：期望…实际…（当前环境可能不支持权限设置）"，挂载环境静默忽略的情况会被如实暴露）；`audit_file_permissions`（启动时审计：`.env`、`config/manual.json`、`config/auto_catalog.json` 收紧为 0600；`config` 上级目录对其他用户可写时**仅告警不自动修改**——不递归修改工作区；只读元数据，不打印秘密值）；`write_atomic` 临时文件与最终文件均 0600。`src/main.rs` 启动时执行审计。
- **Windows 收口（收尾阶段修正表述与实现）**：此前的"文件继承用户配置目录 ACL（仅当前用户可访问）"仅为推断，未提供检查依据，已撤回。现实现为：Windows 上 `apply_mode`/`file_mode` 不做任何检查与调整，`audit_file_permissions` 在 Windows 下**明确输出未验证告警**（"权限检查未实现且未经实测，不宣称仅当前用户可访问；请用 icacls 自行核对"）。交叉编译检查（`cargo check --target x86_64-pc-windows-gnu`）因环境无法下载 Windows std 目标而未执行——Windows 代码路径**未验证**，如实列为剩余限制。
- **红→绿**：红 = 原报告 C13（`.env` 777，归档 `v1-prefix-evidence/`）；绿 = C13 复跑（真实工作区 `.env`、`config/*.json` 均已 0600，审计输出"已将 … 收紧为 0600"）＋`private_fs_modes_enforced` / `audit_tightens_and_reports_without_secrets` 单元测试。
- **上级目录现状（如实记录）**：本工作区 `/workspace` 与 `/tmp` 实测仍为 world-writable（`drwxrwxrwx`），每次启动审计都会输出"上级目录 … 对其他用户可写，config 可能被整体替换；未自动修改"——**该风险保留为持续告警，未消除**（消除需收紧工作区权限，超出"不递归修改整个工作区"的边界）。

## 兼容性变化（升级须知）

1. **首次使用需授权**：新的 base_url（含默认厂商 api.deepseek.com）首次发送凭证时，交互模式会询问"允许并记住？"；非交互模式（管道）未授权会明确拒绝，可交互确认一次或预先在 `~/.config/terminal-chat/authorized-origins.json` 写入授权记录（`origin` + `cred_fp`＝密钥 SHA-256 前 16 位十六进制）。
2. **换 Key 需重新授权**：授权与凭证指纹绑定，密钥轮换后需再次确认。
3. **`file:` 凭证引用必须位于专用凭证目录**：`~/.config/terminal-chat/credentials/`（相对路径按该目录解析）；旧绝对路径引用会得到含迁移方式的明确错误。
4. **公共目录仅接受 HTTPS，无开关**：默认 `https://models.dev` 首次 `/refresh` 时交互确认一次；明文 http 源（含本机回环）一律拒绝并沿用旧目录。本机模拟测试请经 Rust 集成测试的 `url_override` 注入（`sync_public`/`refresh` 的测试参数），不存在环境开关。
5. **重定向不再跟随**：所有出站请求对 3xx 明确报错；如服务地址迁移需更新配置。
6. **模型 id 校验**：含空白/控制字符或超长的 id 会被拒绝（公共目录跳过该条、厂商名单整次失败）。
7. **配置文件权限**：启动审计把 `.env`/`config/*.json` 收紧为 0600；权限设置未生效（挂载环境）或 Windows 平台会明确报告限制，不谎报已解决。

## 剩余限制（如实列出）

- F8（已确认参数的取值合法性）与 F9（无客户端限流、超大输入）按授权边界未改，攻防用例保持信息级记录（C8d、C15）。
- HTTPS 目标的域名可信性依赖用户对授权提示的判断（TOFU）：一旦用户确认了恶意地址，客户端不感知其恶意。
- **Windows 平台权限行为未实现且未验证**（环境无法交叉编译检查），运行时会输出未验证告警。
- **上级目录风险按实际场景区分（收尾核对后更正）**：`/workspace` 为 777 且**无 sticky 位**，其他用户可重命名/替换 `config/` 与 `.env`（完整性风险真实存在，泄露风险因文件 0600 已消除）——审计告警成立，需人工收紧才能根治；`/tmp` 为 1777（**带 sticky 位**），其他用户不能替换属主为当前用户的目录，且程序实际使用的临时目录为 mkdtemp 创建的 0700、产品写入的敏感文件为 0600——**实际无暴露**，审计在该场景的告警属保守过严（启发式未考虑 sticky 位）。改进审计的 sticky 位判断属产品代码变更，按本轮边界未做，已记录为后续可改进项。
- 权限位在部分网络挂载上可能静默失效——现已通过"设置后复读确认"检测并如实报错，但该失败分支依赖环境触发，无法在本机主动构造。
- 公共目录"本机 http 模拟正常同步"的 E2E 覆盖因正式路径强制 HTTPS 而不可达，该正常路径由 Rust 集成测试经测试注入覆盖（E2E 层面 N3 验证的是"公共同步失败被如实报告且厂商查询正常"）。

## 修改文件清单

| 文件 | 变更 |
|---|---|
| `src/api.rs` | F5 查询参数编码；F6 响应体上限；F1 重定向禁用＋3xx 拒绝；F4 目录来源校验（仅 HTTPS）＋`url_override` 测试注入＋id 与名称校验；300 字符截断（F3） |
| `src/catalog.rs` | F1 授权存储与检查、refresh 回调与注入透传；F2 用户目录与凭证边界；F4 目录来源授权助手；F7 权限边界（含设置后复读确认）、0600 原子写、Windows 未验证告警 |
| `src/main.rs` | F3 `say()` 显示边界；F1 交互授权门与 y/N 询问；F7 启动审计；F4 正式路径传 None |
| `src/display.rs`（新增） | F3 终端转义过滤与按字符截断 |
| `src/lib.rs` | 导出 display 模块 |
| `tests/llm_flow.rs` | 新增 13 个集成回归测试；既有测试适配授权门、凭证目录与注入参数（业务断言未放宽） |
| `tests/common/mod.rs` | mock 支持 Location 头；凭证文件助手迁入专用目录并加防呆 |
| `logs/security-20261005/attack.py` | 保留原攻击输入与观测点，漏洞用例改为防护验证，新增正常路径 N1～N4（含 PTY 交互授权）；C12 覆盖"旧开关失效"场景；N3 反映正式路径语义 |
| `AGENTS.md` | 项目信息同步（003 安全修复要点） |

## 附录 A：攻防用例逐 ID 核对（最终 `results.json`，38 条）

| # | ID | 判定 | # | ID | 判定 |
|---|---|---|---|---|---|
| 1 | C0 | PASS | 20 | C9-大写 scheme | PASS |
| 2 | C1 | PASS | 21 | C9-大写 host | PASS |
| 3 | C1b | PASS | 22 | C9-userinfo 伪装 | PASS |
| 4 | C2 | PASS | 23 | C9-前缀伪装域名 | PASS |
| 5 | C3 | PASS | 24 | C9-十进制 IP 等价 127.0.0.1 | PASS |
| 6 | C4 | PASS | 25 | C9-IPv6 本机 | PASS |
| 7 | C5-space | PASS | 26 | C9-file 协议 | PASS |
| 8 | C5-amp | PASS | 27 | C9-https 任意主机（无白名单） | PASS |
| 9 | C5-hash | PASS | 28 | C10 | PASS |
| 10 | C5-crlf | PASS | 29 | C11 | PASS |
| 11 | C5-slash | PASS | 30 | C11b | PASS |
| 12 | C5-unicode | PASS | 31 | C12 | PASS |
| 13 | C6 | PASS | 32 | C13 | PASS |
| 14 | C7-11MB | PASS | 33 | C15 | INFO |
| 15 | C7-30MB | PASS | 34 | C16 | PASS |
| 16 | C8a | PASS | 35 | N1 | OK |
| 17 | C8b | INFO | 36 | N2 | OK |
| 18 | C8c | PASS | 37 | N3 | OK |
| 19 | C8d | WARN | 38 | N4 | OK |

> 原 34 条 = 上表 1～34（C 系列，其中 C5 六个子项、C7 两个子项、C9 八个子项）；35～38 为新增正常路径 N1～N4。INFO 为 C8b/C15，WARN 为 C8d（F8/F9 范围，按授权边界保持信息记录）。C8b/C8c/C8d/C10 曾因 harness 预置缺陷被 F1 授权门先行拦截（详见附录 B），已修正预置并补跑，当前判定均以真实请求为据。

## 附录 B：收尾核对更正记录（同日，仅记录与测试脚本，未动产品代码/断言/权限）

### B.1 C8b/C8c/C8d/C10：harness 预置缺陷导致结论未按声称路径验证

- **实际发现**：最终一轮结果中，C8b（发出请求=False）、C8c（0 请求）、C8d（`max_tokens=None`，却标 PASS）、C10（0 请求）四个用例被 F1 授权门先行拦截——C8 三个子用例复用了 8a 的授权目录而各自新建了不同端口的 mock，C10 使用空授权目录。**防护结论本身最终成立，但"验证路径"与用例声称的观测点不符**：C8b 的"大小写变体无覆盖"未经过真实请求，C8c/C10 的 0 请求原因被授权门混淆，C8d 的 PASS 为假阳性。属 harness 缺陷，非产品问题。
- **修改**（仅测试脚本 `attack.py`，断言只增不减）：C8 各子用例改为各自预授权自己的 mock 地址；C10 预授权"含 CRLF 的凭证值"以隔离出密钥头安全检查；C8b 增加 `supported_params=["Model"]` 注入使请求真实上线（否则会被能力检查提前拒绝、无法观察）；C8a/C8c/C10 增加拒绝原因断言（"保留字段"/"能力未知"/"换行或控制字符"），排除授权门混淆；`main()` 支持定向补跑合并写回 `results.json`。
- **更正前后**：
  - C8b：前 `发出请求=False；请求体含 Model 键=False`（推理性结论）→ 后 `发出请求=True；请求体同时含 Model（变体）与 小写 model 保持原值`（线级验证，证据 `C8b-request-body.json.txt`：`{"Model":"attacker-model",…,"model":"deepseek-flash"}`）；
  - C8c：前 `出站请求数=0`（原因未区分）→ 后 `0 请求且拒绝原因含"能力未知"`（真实原因确认）；
  - C8d：前 **PASS**（`max_tokens=None`，假阳性）→ 后 **WARN**（`max_tokens=-5, temperature=42` 真实直发，证据 `C8d-request-body.json.txt`，F8 范围信息项）；
  - C10：前 `0 请求`（原因未区分）→ 后 `0 请求且拒绝原因含"换行或控制字符"`（确认由密钥头安全检查拦截而非授权门）。
- **补跑方式**：`python3 attack.py C8`、`python3 attack.py C10`（定向合并写回）；其余 34 条证据未变。
- **原报告核对结论**：原始攻防报告对 C8b/C8d 的记载与 v1 证据一致（v1 的 C8d 请求体即含 `-5/42`、C8b 如实记载未发出请求），无需更正；需要更正的是修复报告附录 A 中 C8d 的判定（PASS→WARN）与本节说明。

### B.2 `/tmp` 权限判断：审计告警在该场景保守过严，记录更正

- **实际核对（只读元数据）**：`/tmp` = `drwxrwxrwt (1777) root:root`——**带 sticky 位**；`/workspace` = `drwxrwxrwx (777) root:root`——无 sticky。程序实际使用的 E2E 临时目录（`/tmp/tchat-sec-cfg-*`、`/tmp/tchat-sec-user-*`）为 mkdtemp 创建的 `0700`；产品写入的 `auto_catalog.json` 为 `0600`（write_atomic），`manual.json` 经启动审计收紧为 0600（N3/C12 输出有"已将 … 收紧为 0600"记录）。个别 0644 残留为测试夹具或修复前旧文件，均位于 0700 目录内，其他用户不可读。
- **更正前后**：
  - 前（修复报告剩余限制）："本工作区 `/workspace`、`/tmp` 实测 world-writable，审计每次启动告警，需人工收紧目录权限才能根治"——把两者并列，未区分 sticky 位；
  - 后：`/workspace` 777 **无 sticky** → 其他用户可替换 `config/`、`.env`，完整性风险真实成立（泄露风险因 0600 消除），告警成立、需人工根治；`/tmp` 1777 **带 sticky** → 其他用户不能替换属主为当前用户的目录，加上 0700 目录 + 0600 文件，**实际无暴露**，该场景告警属保守过严。
- **边界**：审计启发式未考虑 sticky 位属产品代码可改进项，按本轮"不修改产品代码"边界未做，记录为后续改进；F7 结论保持"Unix 应用文件权限已修复；Windows 未实现、未验证；上级目录风险按实际场景记录"。

## 附录 C：证据索引

| 路径（logs/security-20261005/） | 内容 |
|---|---|
| `attack.py` / `results.json` | 攻防驱动脚本最终版 / 最终 38 条结构化结果（权威） |
| `results-prefix-fix.json` | 修复前（v1）34 条结果，含 C1/C2/C12/C13 漏洞确认记录 |
| `v1-prefix-evidence/` | 修复前原始证据归档（C1 Key 外带、C3/C4 转义透传、C5 请求行、C12 投毒落盘等） |
| `C1-stdout-unauth.txt` / `C1-stdout-auth.txt` | 未授权拒绝 / 预授权放行 |
| `C8b-request-body.json.txt` / `C8d-request-body.json.txt` | 大小写变体线级验证 / 危险取值直通（收尾补跑） |
| `C12-stdout.txt` | 明文 http 目录在带/不带旧开关两种场景下均被拒绝 |
| `N3-stdout.txt` | 正式路径刷新：厂商查询成功、公共同步失败如实报告 |
| `security-test-log.md` | 测试要点、失败原因与验证结论（含收尾记录） |

---

## 2026-10-10 合并前整改补充：授权记录原子写（本轮契约与预测，未执行项标待验证）

背景：合并前风险调查发现 `grant_origin`（`catalog.rs:444`）经 `write_private_file`（原地 truncate＋write_all）写入 `authorized-origins.json`——并发授权的字节级交错可产生混合/截断文件；两个 RLIMIT/占位类写入失败也可能留下截断目标。本节记录已确认契约与事前预测；全部条目在实施验证前均为**待验证**，验证后在本节补实测结论。

### 已确认契约（沿用原报告授权语义，只改写入方式）

1. `grant_origin` 改为：`ensure_private_dir(user_dir)`（0700）后复用 `write_atomic` 写 `authorized-origins.json`。
2. `write_atomic` 强化为**排他创建**：tmp 打开改 `create_new`——不复用已有 tmp 路径、不跟随已有软链接；**打开失败直接返回错误**；**仅本次成功创建的 tmp 才进入失败清理范围**（创建失败不得删除原已存在的路径）。
3. 保持：tmp 与目标同目录、创建即 0600、完整 `write_all` 后才 `rename`、失败保留旧目标完整、成功替换后最终文件 0600、无 fsync。
4. 并发语义＝末者胜完整快照：终态必为某一写者的完整授权文件；**保留已接受的读改写条目丢失**（上次同意可能丢失并重新询问，fail-safe）；不新增合并机制、锁框架或 fsync 承诺。
5. 安全不变量：不产生交叉混配的 (origin, fingerprint)；不新增未经同意的授权；写失败/未同意一律零请求（fail-closed）。

### 事前预测（源码推导，待验证）

- 目录占位场景失败点从"打开/写入"移至 rename（EISDIR）；既有 e2e AUTH-E02 的"错误＋零请求＋占位原样"结论应保持，需补"无本轮 tmp 残留"断言。
- `stale_tmp_does_not_block_save`（catalog.rs:1833 单测）预置固定名 `auto_catalog.json.tmp`，与生成名（pid.纳秒.序号）不碰撞——create_new 化后应仍通过（待验证）。
- RLIMIT 部分写入（正数字节后 EFBIG）落在 tmp；旧目标字节不变、tmp 被清理、本轮零请求零新增对话记录、下次仍需确认（待验证）。
- `save_manual`/`save_auto`/refresh/配置流为受影响回归面（create_new 行为变化仅碰撞角）（待验证）。

### 验证计划（摘要）

冲突保护（预置 tmp 路径含软链接边界）经最小内部测试接缝控制 tmp 路径（仅 cfg(test) 可见）；真实生产路径验证：目录占位（rename 失败）、RLIMIT 正字节部分写入（SIGXFSZ 忽略＋配置预初始化；夹具先以探针证明写过正数字节后失败）、无旧目标/旧含 A 新增 B 两初始状态、双写者并发（均须成功持久化＋终态无条件可解析/完整配对/无未同意条目/无 tmp 残留）、静态坏文件装载容错单独验证。验证证据补记于本节与 `logs/2026-10-10-007-implementation.md`。


### 实测结论（2026-10-10 本轮补记，全部预测已验证）

- 冲突保护（单测 `preexisting_tmp_path_is_protected` / `preexisting_tmp_symlink_is_not_followed`，RED→GREEN）：预置 tmp 路径（文件/软链接）不覆盖、不删除、不写穿；目标不被替换。
- `grant_origin` 原子写（单测 `grant_write_failure_preserves_old_store`，RED→GREEN）：写入失败（预置 tmp 目录占位）报错、旧授权文件字节不变、A 有效 B 未获授权；成功路径无 tmp 残留。
- 目录占位→替换失败（CLI `e24_grant_placeholder_rename_failure`）：错误明确、零请求、占位原样、无本轮 tmp 残留、零新增对话记录。
- RLIMIT 正字节部分写两状态（CLI `e25_grant_partial_write_two_states`＋探针 `rlimit_probe_child_marker`）：探针先证明"写过正数字节后失败"（限额 512→恰 512 字节＋EFBIG）；(a) 无旧目标失败后目标仍不存在、下次仍需确认；(b) 旧含 A 新增 B 失败：旧文件字节不变、A 下次直接放行、B 仍需确认；两状态本轮零请求零新增记录。
- 并发完整性（CLI `e26_concurrent_grant_integrity`）：双写者均成功持久化并放行（各自 mock 恰一请求）；终态无条件可解析、完整 (origin,fp) 配对无混配、无未同意条目、无本轮 tmp 残留；1–2 条皆可（已接受条目丢失保留）。
- 静态坏文件容错（单测 `broken_store_consent_replaces_with_single_pair`）：损坏→询问→同意后整文件覆盖恰 1 对。
- 受影响回归：`stale_tmp_does_not_block_save`、`save_manual`/`save_auto`、配置流、refresh、AUTH-E02 等经全量回归通过（数量见 logs）。
- 定向变异：清理守卫移除→冲突测试杀；同名释放移除→同名测试杀；grant 回退原地写→原子写测试杀。
- 附带修复（审查发现）：rename 失败错误信息补终路径上下文；授权文件替换后权限恒 0600（tmp 0600 经 rename 继承）。
