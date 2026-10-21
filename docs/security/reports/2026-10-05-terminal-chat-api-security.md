# terminal-chat 后端接口安全攻防测试报告

> 2026-10-19 证据归类：当前捕获与结构化结果见[安全证据说明](../evidence/2026-10-05/README.md)，历史 `logs/security-20261005/`／`docs/records/security-20261005/`、脚本及运行命令对应[原始快照](../../records/evidence-index.md#security-originals)，不作为当前测试入口。下文历史结论与修订记录保持原意。

- **日期**：2026-10-05
- **测试范围**：terminal-chat（Rust CLI）的 LLM API 后端层——`src/api.rs`（chat / list_models / sync_public 出站请求）、`src/catalog.rs`（配置合并、凭证解析、路由检查、目录落盘）、`src/main.rs`（终端入口）
- **被测版本**：工作区当前构建产物 `target/debug/terminal-chat`（rustc 1.99.0 / ureq 2.12.1 / Cargo.lock 锁定）
- **方法**：先代码审查（按 8 类攻击面逐条排查），再用攻击脚本对真实二进制实测（34 个用例）
- **报告人**：安全测试 Agent（授权范围内测试，全程仅访问本机 mock，未触碰任何外部服务）

---

## 1. 结论摘要

| 结论 | 数量 |
|---|---|
| 确认漏洞（高） | 2 |
| 确认漏洞（中） | 2 |
| 确认漏洞（低） | 3 |
| 信息级记录 | 3 |
| 实测有效的防护控制 | 12 项 |

全局断言通过：**34 个用例的全部输出（stdout/stderr）中未出现任何密钥明文**（伪造密钥、受害者文件字段、真实 `.env` 密钥三重检查）。

本项目是**单用户本地 CLI 的 LLM API 客户端**，不存在 Web 服务端。用户清单中的登录、订单、RBAC 等类别按"等价攻击面"映射后测试（见第 4 节映射表），无法映射的类别已注明不适用原因。

---

## 2. 测试方法与环境

- **攻击脚本**：[logs/security-20261005/attack.py（原件快照）](<../../records/evidence-index.md#archive-651e36d7f2d538f1>)（Python 3 标准库，无外部依赖）
  - 原始套接字 mock 服务器扮演"攻击者主机"，逐字节记录收到的请求行、头部与正文；
  - 以管道 stdin 驱动**真实二进制**，捕获原始 stdout 字节（含转义序列）与退出码；
  - 每个用例使用独立临时配置目录（`TCHAT_CONFIG_DIR`）与伪造密钥，结束后清理，未读写真实用户环境；
  - 运行方式：`python3 attack.py [用例ID ...]`；结果与逐条证据写入 `logs/security-20261005/`。
- **代码审查佐证**：全程无 `std::process::Command`/shell 调用、无 `unsafe` 块（grep 核实）；`src/catalog.rs` 内置 63 处 `unwrap/expect` 中除 1 处不可达外全部位于 `#[cfg(test)]`。

---

## 3. 漏洞清单

### F1【高】配置投毒 → API Key 原样外带至任意主机（无域名白名单）

- **前提**：能写入 `config/manual.json`（或 `TCHAT_CONFIG_DIR` 指向的目录）。当前部署工作区根目录权限为 `drwxrwxrwx`，同机其他用户具备该写入能力；此外用户从网上拷贝"配置模板"是常见供应链入口。
- **后果**：API Key 被完整发送给攻击者控制的任意 HTTPS 主机（`Authorization: Bearer <key>`），造成凭证泄露与费用盗用。
- **复现**（实测）：
  1. `manual.json` 写入 `"base_url": "http://127.0.0.1:<攻击者mock端口>"`，`key_ref: "env:DEEPSEEK_API_KEY"`；
  2. 管道输入 `hi` 发起对话；
  3. 攻击者 mock 收到：`POST /chat/completions`，头部 `Authorization: Bearer sk-ATTACK-FAKE-KEY-0001`（已打码），正文含模型名与用户输入。
- **代码依据**：`catalog.rs:292` `validate_base_url` 只校验协议与本机 HTTP 白名单，注释明确"不做域名白名单"；C9 矩阵进一步证实**任何 HTTPS 主机均放行**（对 `https://127.0.0.1:<mock>` 的请求真实发起，仅因 mock 无 TLS 而握手失败）。
- **证据**：`logs/security-20261005/C1-stdout.txt.txt`、`C9-https 任意主机（无白名单）-stdout.txt.txt`
- **建议**：对 base_url 增加首次使用确认（TOFU）或厂商域名白名单；配置目录权限收紧至 0700，配置变更时向用户告警。

### F2【高】`file:` 凭证引用构成"任意可读 JSON 文件字段外带"原语

- **前提**：同 F1（能写 manual.json）。
- **后果**：本机任意可读 JSON 文件的任意字符串字段（如其他工具的 `credentials.json` 中的 token）会被当作 API Key 发往配置指定的主机——即把**本地任意 JSON 文件字段**变成可外带数据。
- **复现**（实测）：
  1. 构造受害者文件 `victim.json`：`{"api_token": "victim-file-secret-..."}`（模拟同机其他应用的凭证文件）；
  2. `manual.json` 写入 `"key_ref": "file:/path/victim.json#api_token"`，`base_url` 指向攻击者 mock；
  3. 攻击者 mock 收到 `Authorization: Bearer victim-file-secret-...`（实测命中：True）。
- **证据**：`logs/security-20261005/results.json`（C2）
- **建议**：`file:` 引用限制在专用凭证目录内（如 `~/.config/terminal-chat/credentials/`）并校验路径前缀；凭证文件权限 0600。

### F3【中】终端转义序列注入：模型回复、错误消息、目录元数据未过滤直达终端

- **前提**：接入恶意或被劫持的服务端（配合 F1/F4 门槛更低）。
- **后果**：在用户终端执行转义序列——改写窗口标题（OSC 0）、清屏并移动光标（CSI 2J）、**覆写剪贴板（OSC 52）**、插入超链接（OSC 8），可用于钓鱼与显示层欺骗。
- **复现**（实测，三条通道全部命中）：
  1. **模型回复通道**：mock 返回 `content` 含 `\x1b]0;窗口标题已被劫持\x07`、`\x1b[2J`、`\x1b]52;c;aGVsbG8=\x07`，CLI 输出原样透传（`main.rs:99` `println!("Agent：{reply}")` 无过滤）；
  2. **错误消息通道**：mock 返回 401 且 `error.message` 含转义序列，输出同样透传；且 **error.message 无长度上限**（320 个 `X` 全量回显，`api.rs:46` 的 300 字符截断仅作用于非 JSON 响应体）；
  3. **目录元数据通道（二次注入）**：见 F4，投毒的模型名经 `/models` 命令回显。
- **证据**：`logs/security-20261005/C3-stdout-raw.bin.txt`、`C4-stdout.txt.txt`、`C12-stdout.txt.txt`
- **建议**：所有来自远端的文本打印前过滤 C0 控制字符与 CSI/OSC 序列（白名单保留常规可打印字符），错误消息长度截断覆盖 `error.message` 分支。

### F4【中】公共目录同步允许明文 http，且同步内容可直接投毒本地目录

- **前提**：环境变量 `TCHAT_PUBLIC_CATALOG_URL` 可被设置（或默认 models.dev 遭遇 HTTPS 降级场景之外的部署篡改）；该 URL **无任何协议/来源校验**（`api.rs:214-218`）。
- **后果**：中间人或本地环境篡改者可向 `auto_catalog.json` 写入恶意模型元数据（名称、上下文长度等），配合 F3 形成"刷新 → 落盘 → `/models` 回显"的**二次注入链**；同时污染模型能力数据影响用户决策。
- **复现**（实测）：`TCHAT_PUBLIC_CATALOG_URL=http://127.0.0.1:<mock>/pub` 返回 `{"deepseek":{"models":{"deepseek-flash":{"name":"DeepSeek 官方\x1b]0;POISONED\x07模型"}}}}` →
  - 明文 http 拉取成功：True；
  - 恶意字段写入 `auto_catalog.json`（serde 以 `\u001b` 转义存储）：True；
  - `/models` 命令回显出原始转义序列：True。
- **证据**：`logs/security-20261005/C12-stdout.txt.txt`、`C12-auto_catalog.json.txt`
- **建议**：公共目录 URL 强制 HTTPS（与本厂 `validate_base_url` 同标准）；同步字段做类型与内容清洗。

### F5【低】分页游标未 URL 编码：查询参数注入/截断（同主机边界内）

- **前提**：服务端返回的模型 `id` 含特殊字符（服务端本身恶意或被劫持）。
- **后果**：`api.rs:207` 直接拼接 `?after={cursor}`，实测：`&` 可注入独立查询参数（`GET /models?after=abc&injected=1`）、`#` 截断游标值；CRLF 被 URL 解析器剥离（`abc\r\nX-Injected: 1` → `after=abcX-Injected:%201`），**未构成 HTTP 头注入**；空格与 Unicode 会被百分号编码。由于游标仅回流到同一服务端，跨信任边界影响有限，但属于服务端可控数据绕过编码纪律的真实缺口。
- **证据**：`logs/security-20261005/C5-*-requests.txt.txt`
- **建议**：cursor 经百分号编码后再拼接。

### F6【低】响应体无大小上限：恶意服务端可对客户端做内存放大 DoS

- **前提**：接入恶意服务端（同 F3 前提）。
- **后果**：实测 11MB 响应 → 子进程峰值 RSS 29MB；30MB → 68MB，**内存随响应体线性增长、无客户端上限**（chat 路径 `into_json`，ureq 2.12.1）。超大响应可致 CLI 内存膨胀/OOM。
- **证据**：`logs/security-20261005/results.json`（C7-11MB / C7-30MB）
- **建议**：为响应读取设置固定上限（如 10MB）并在超限时中止。

### F7【低】凭证明文落盘且文件权限过宽

- **实测现状**：`.env` 权限 `-rwxrwxrwx`（同机任何用户可读写真实密钥）；`config/*.json` 为 `-rw-r--r--`；工作区根目录 `drwxrwxrwx`（其他用户可整体替换 config 目录 → 打通 F1/F2 前提）。
- **正面确认**：`auto_catalog.json` 仅存 SHA-256 前 16 位指纹（无明文 Key）；`Credential`/`CredOutcome` 的 Debug 输出已脱敏（项目单元测试覆盖）。
- **证据**：`logs/security-20261005/results.json`（C13）
- **建议**：`.env`/凭证文件 0600、配置目录 0700；启动时检测过宽权限并告警。

### F8【信息】已确认参数的危险取值直通服务端

`supported_params` 声明 `max_tokens` 后，`"max_tokens": -5`、`"temperature": 42` 原样发送（`POST /chat/completions` 正文实测），客户端只做"能力名单"检查不做取值校验——取值合法性由服务端兜底，属设计边界而非缺陷。证据：`C8d-request-body.json.txt`。

### F9【信息】无客户端限流与输入大小限制

5MB 单行输入完整出站（请求体 5,242,948 字节）；连续输入无节流（每行一次请求）。作为客户端由服务端 429 限流兜底（429 已有明确错误翻译），风险低。证据：`results.json`（C15）。

---

## 4. 用户清单 8 类攻击面 → 本项目映射与结论

| # | 用户清单类别 | 本项目等价攻击面 | 结论 |
|---|---|---|---|
| 1 | 数据越权（改订单号/用户 ID 查改删他人数据） | 无多租户存储，无对象 ID 接口 → **不适用**。最接近的等价面：凭证/配置隔离，实测出 F1、F2（配置即"越权读取本机其他文件字段并发往外部"的通道） | 映射后确认 2 项 |
| 2 | 功能越权（普通用户调管理员接口） | 本地 CLI 无角色体系，所有命令（/models、/refresh）对本地用户可用；风险实质是凭证保护强度，归入 F7 | 不适用（无服务端 RBAC） |
| 3 | 身份认证（不登录/伪造/篡改 token） | 无登录端点；实测密钥缺失/为空的报错与不发包行为、密钥篡改（401 翻译）、密钥 CRLF 头注入 → 全部拦截（C10/C11/C11b） | 防护有效 |
| 4 | 偷改字段提权（多塞 role/价格/负数金额） | `request_params` 注入保留字段 `model`/`messages` → 发送前拦截（C8a）；大小写变体无覆盖效果（C8b）；能力未知参数默认拒绝（C8c）；已确认参数危险取值直通（F8） | 防护有效＋信息 1 项 |
| 5 | 恶意输入与注入 | SQL 注入：无 SQL，**不适用**；命令注入：无 shell/Command 调用（grep 核实），**不适用**；脚本注入映射为终端转义注入 → **F3 确认**；JSON 结构注入实测免疫（C0） | 确认 F3；JSON 免疫 |
| 6 | 绕过前端限制（直打接口绕过页面校验） | 客户端校验即"前端"：base_url 校验矩阵（C9）、远程 http 拒绝（C1b）、参数能力检查（C8c）、游标拼接缺口（F5） | 主体有效，F5 为缺口 |
| 7 | 敏感信息泄露 | 全局断言：34 用例输出无密钥明文；auto_catalog 仅指纹；错误消息回显远端内容（并入 F3 的无上限问题）；文件权限 F7 | 未发现密钥泄露；F7 |
| 8 | 资源滥用与防刷（登录/短信/下单高频） | 无登录/短信/下单端点；分页防刷上限实测有效（C6：恰好 100 页截断）；响应体无上限（F6）；无限流（F9） | C6 有效；F6/F9 |

---

## 5. 实测有效的防护控制清单

1. **JSON 注入免疫**（C0）：引号/换行/`\x01\x02`/SSTI/SQL 片段均作为完整字符串值到达，请求体结构未被破坏（`C0-request-body.json.txt`）。
2. **远程 http 明文拒绝**（C1b）：`http://attacker.example.com` 发送前拦截，0 出站请求。
3. **base_url 校验矩阵**（C9）：userinfo 伪装（`http://127.0.0.1@evil.example`）、前缀伪装域名（`localhost.evil.example`）、`file:` 协议均拒绝；大写 scheme/host 正确归一化；十进制 IP `2130706433` 被正确解析为本机回环（按设计放行）。
4. **密钥 CRLF 头注入拦截**（C10）：`check_key_header_safe` 发送前拦截，0 出站请求。
5. **保留字段防覆盖**（C8a/C8b）与**能力未知参数默认拒绝**（C8c）。
6. **缺失/空密钥明确报错且不发包**（C11/C11b）。
7. **分页防刷上限**（C6）：恶意无限 `has_more` 恰好在第 100 次请求后中止，并报"分页超过 100 页仍未读完"；游标不前进、缺 id 均中止不空转。
8. **非法 UTF-8 不 panic、U+FFFD 不进入请求体**（C16，001 规格回归确认）。
9. **密钥不落日志/输出**：34 用例全局断言通过；本地目录仅存不可逆指纹。
10. **原子写盘**：临时文件+rename，损坏文件不被自动改写（项目既有单元测试覆盖）。
11. **凭证换绑即失效**：身份指纹（地址+协议+凭证名+凭证指纹）任一变化即废弃旧名单（项目既有单元测试覆盖）。
12. **错误分类不回显密钥**：401/403/404/429/5xx 翻译均不含凭证值。

---

## 6. 复现方式

```bash
cd /workspace/logs/security-20261005
python3 attack.py            # 全部 34 个用例
python3 attack.py C3 C12     # 单项复现
```

依赖：已构建的 `/workspace/target/debug/terminal-chat`（`cargo build --locked`）。全部流量仅指向 `127.0.0.1` mock；测试期间未向任何外部主机发起请求（被拒绝的配置在请求前即拦截）。

## 7. 证据索引

> **归档说明（2026-10-05 修复阶段更新）**：本报告记录的是**修复前**的攻防实测。修复后攻击脚本已升级为防护验证版（`attack.py` 当前版本），修复前原始证据归档于 `logs/security-20261005/v1-prefix-evidence/`，修复前结构化结果为 `logs/security-20261005/results-prefix-fix.json`；修复后最终结果见 `logs/security-20261005/results.json`（38 条）与 [2026-10-05-terminal-chat-api-security-fix.md](2026-10-05-terminal-chat-api-security-fix.md)。

| 文件（logs/security-20261005/） | 内容 |
|---|---|
| `attack.py` / `results-prefix-fix.json` | 攻击驱动脚本 / 修复前 34 条用例的结构化结果（归档于 `v1-prefix-evidence/` 的为原始逐项输出） |
| `v1-prefix-evidence/C0-request-body.json.txt` | JSON 注入输入的出站请求体（结构完好） |
| `v1-prefix-evidence/C1-stdout.txt.txt` | Key 外带复现输出 |
| `v1-prefix-evidence/C3-stdout-raw.bin.txt` | ANSI 注入原始输出字节 |
| `v1-prefix-evidence/C4-stdout.txt.txt` | 错误消息全量回显（320 字符未截断）＋转义序列 |
| `v1-prefix-evidence/C5-*-requests.txt.txt` | 六种游标注入变体的实际请求行 |
| `v1-prefix-evidence/C8d-request-body.json.txt` | 危险参数值出站请求体 |
| `v1-prefix-evidence/C9-*-stdout.txt.txt` | base_url 校验矩阵逐项输出 |
| `v1-prefix-evidence/C12-stdout.txt.txt` / `v1-prefix-evidence/C12-auto_catalog.json.txt` | 公共目录投毒链路 |
| `security-test-log.md` | 测试要点、harness 缺陷修复记录、修复阶段与收尾记录 |
