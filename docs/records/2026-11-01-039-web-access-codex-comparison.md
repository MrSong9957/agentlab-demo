# 039 Web 工具：本项目方案与 Codex 开源实现差异对比

日期：2026-11-01。文风：ASD-STE100 近似达标中文版。短句。主动语态。术语唯一。

## 1. 目的

本文对比两条 web 工具路线：

- **Codex 路线**：openai/codex 开源仓库（Rust，提交 `cb1bc0ae4ed996c67a6e118bf79a7435f0d3f0d2`）的实际实现。
- **本项目路线**：039 调研方案（pi-web-access 底层逻辑＋四专家裁决），见 [039 调研汇总](../think-tank/2026-11-01-039-web-access.md)。

本文只陈述差异与借鉴点。本文不改两边的方案。

## 2. 证据基线

| 文件 | 证据 |
|---|---|
| `codex-rs/core/src/tools/hosted_spec.rs` | `create_web_search_tool`（14-46 行）：mode 映射为托管参数；生成 `ToolSpec::WebSearch` |
| `codex-rs/protocol/src/config_types.rs` | `WebSearchMode`（376-382 行，默认 `Cached`）；`restrict_to`（386-393 行）；`WebSearchFilters` 仅 `allowed_domains`（466-468 行）；`WebSearchConfig`（493-497 行） |
| `codex-rs/core/src/tools/spec_plan.rs` | 注册条件（704-713 行）：standalone `web/run` 扩展优先；否则要求 provider `capabilities().web_search`；两者皆无＝不声明工具 |
| `codex-rs/core/src/web_search.rs` | `WebSearchItem.results` 为服务端透传的 opaque JSON（17-23 行注释）；`WebSearchAction` 含 `Search`/`OpenPage`/`FindInPage`（32-46 行） |
| `codex-rs/core/src/tools/handlers/`（目录遍历） | 无任何 fetch 处理器。本地抓取能力不存在 |
| `codex-rs/tui/src/app_server_session/web_search.rs` | 会话旗标（`web_search`/`web_search_cached`/`web_search_request`）只能继承或移除，不得越过配置层放宽 |

托管搜索的计费、缓存与数据细节属于 OpenAI 服务端，客户端代码不可见。本文不下结论。

## 3. 根差异（一句话）

Codex 的 web_search 是**托管工具**：客户端只声明参数，搜索与网页阅读在 OpenAI 服务端执行。本项目路线是**本地执行**：宿主进程自己发请求、自己解析、自己防护。

其余全部差异由这一条派生。

## 4. 差异总表

| 维度 | Codex | 本项目 039 方案 |
|---|---|---|
| 执行位置 | OpenAI 服务端 | 宿主进程本地 |
| 搜索供给 | provider 托管，随模型走 | 自带后端：Tavily＋Brave 密钥，DDG 兜底开关 |
| 网页抓取 | 无本地 fetch；`OpenPage`/`FindInPage` 在服务端执行 | `web_fetch` 本地抓取＋readability-rs＋htmd 提取 |
| SSRF 防护 | 无此攻击面（请求不出宿主） | 必须做：解析层 Resolve 闸＋手动重定向逐跳校验 |
| 内容加工 | 服务端控制（`search_context_size`） | 客户端提取，原文直给模型；无小模型摘要 |
| 默认态 | `Cached`（默认旧快照，社区骂点） | 默认关（环境变量门控）；开即实时 |
| 域名过滤 | 仅 `allowed_domains` | `allowed`/`blocked` 二选一（对齐 Claude Code） |
| provider 依赖 | 强绑定：无 `capabilities().web_search` 即无工具 | 无：五协议通用，不依赖模型供应商 |
| 结果结构 | opaque JSON 透传，字段不受客户端控 | 自解析 `title/url/snippet`，结构可控＋025 脱敏扩面 |
| 审批面 | 无 web 审批，纯配置门控 | 相同：不进 031，配置门控（域名白名单） |
| 离线行为 | 离线＝无 web 能力 | 离线＝无 web 能力（相同） |
| 新字段演进 | 服务端加字段，客户端零发布即透传 | 字段在客户端定义，演进需本地改结构 |

## 5. 关键差异说明

### 5.1 客户端复杂度

Codex 客户端的 web 代码总量约百余行，全是类型与声明。本项目方案含 HTTP、DNS、SSRF、提取、并发、缓存。这是两条路线的代价差：Codex 把复杂度换成 provider 锁定，本项目把 provider 自由换成实现量。

### 5.2 模式语义

Codex 用四态 `WebSearchMode`（Disabled/Cached/Indexed/Live），默认 `Cached`；`restrict_to` 保证会话请求只能收窄、不得放宽。本项目无缓存语义：门控关闭即无工具，开启即实时。社区对「默认旧快照」的不满（见侦察兵简报）不支持照抄默认值；但「只能收窄」的覆盖语义值得照抄（见第 6 节）。

### 5.3 抓取能力

Codex 没有本地 fetch。模型读网页全文的唯一途径是托管搜索的服务端动作。本项目 `web_fetch` 补的正是这块：任意 URL、全文 Markdown、脱敏后入史。此能力 Codex 路线给不了。

### 5.4 失真与保真

Codex 的搜索结果由服务端加工，字段不可控，客户端只透传。本项目自解析结果、原文直给。社区「内容给错」的吐槽对托管路线成立；本项目路线的对应风险是提取器质量，由 Mozilla 130 例 golden 验收覆盖。

## 6. 从 Codex 借鉴的三点

1. **`restrict_to` 收窄语义**：会话级开关不得越过配置级白名单。落到本项目＝会话请求只能在域名白名单内做交集，不得新增放行域。
2. **opaque 透传字段**：搜索结果结构留一个 opaque `details` 区，新字段先行透传，避免每次后端演进都动本地 schema。
3. **模式枚举显式化**：`Disabled/Cached/Indexed/Live` 的显式枚举＋「默认不声明工具」的做法，优于布尔开关。本项目门控可沿用「默认不声明」语义（环境变量未设＝工具不进 prompt）。

## 7. 结论

两条路线不冲突：Codex 证明「客户端零实现」可行，但代价是 provider 锁定、无 fetch、离线全无。本项目选本地执行，换取五协议通用与完整抓取能力；代价是实现量与 SSRF 防护责任。差异表中第三、四行（抓取、SSRF）是本项目必须自担的边界，已由 039 组合方案第 1、2 条覆盖。

## 8. 引用

- Codex 仓库：https://github.com/openai/codex （提交 `cb1bc0ae4ed996c67a6e118bf79a7435f0d3f0d2`，2026-11-01 查询）
- 本项目调研汇总：`docs/think-tank/2026-11-01-039-web-access.md`
- 参考实现考古：`docs/think-tank/raw/2026-11-01-039-web-access-参考实现考古.md`
- 早前 Codex 工具面对比（钉住提交 `d138398`）：`docs/records/2026-10-21-codex-tools-comparison.md`
