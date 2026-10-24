# 022 MCP 外部工具规格调研

- 日期：2026-10-24。
- 交付：[022 规格草案](../specs/022-mcp-tools.md)。仅文档，未实现、注册或调用本项目 MCP 工具。
- 任务来源：[2026-10-21 工具对比](2026-10-21-codex-tools-comparison.md)「MCP 外部工具」一行；资源读取与工具发现另有条目，不合并扩大本次范围。
- 本地 HEAD：`6b8f3a310c0a0748239641b127c6fdb46ec8d2db`；工作区已有 021 实现、配置、记录和历史证据删除等未提交改动。按工作区实际源码核对，不把较早规格的固定工具数量当成现状。
- 上游基线：`openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`，与用户指定记录一致。通过 GitHub 连接读取固定目录树与文件；当前官方文档只用于功能入口及 CC API 约束，不能替代固定源码。

## 1. 已核对的本地边界

| 位置 | 事实与规格影响 |
|---|---|
| [prompt.rs](../../src/app/src/prompt.rs) | 原生四工具按宿主、图片及消息能力追加条件工具；`build_turn_request_with` 已能接收工具定义数组。MCP 必须接入同一请求构建，不另起模型客户端 |
| [turn.rs](../../src/app/src/turn.rs) | 工具声明和分派分开；批前协议校验、每轮 8 请求／每批 32 调用、call ID／name 原样、取消补齐及 MemoryMerge 隔离需要一起保留 |
| [catalog.rs](../../src/app/src/catalog.rs) | 项目配置根默认 `config`，用户私有根由 `user_dir` 与 `TCHAT_USER_DIR` 控制；已有私有目录／文件权限工具。MCP 可执行配置建议放私有根，不复用提供方 Key 和授权记录 |
| [api.rs](../../src/app/src/api.rs) | Chat Completions，保留字段不能由 `request_params` 覆盖；模型请求在完整序列化后受 10 MiB 上限约束 |
| [main.rs](../../src/app/src/main.rs)、[tui_host.rs](../../src/app/src/tui_host.rs) | `/refresh` 是模型目录功能；TUI 退出经过 `process::exit`，016 已要求显式清理。MCP 需独立状态／reload 并加入退出收尾 |
| [prompt.rs 历史类型](../../src/app/src/prompt.rs)、[会话规格](../specs/007-session-persistence.md) | V1 已保存任意工具名、arguments 和结果文本，足以记录首版 MCP；恢复不重放，缺结果为未知 |
| [测试公共设施](../../src/app/tests/common/mod.rs) | 已有本地 HTTP、环境隔离锁、响应屏障及 PTY；可扩展 MCP 边界服务，无需新增回归运行器 |
| [app 依赖](../../src/app/Cargo.toml) | 已有 tokio、serde_json、sha2、url；当前没有 MCP 协议客户端。名称哈希可用 sha2，协议层应引入成熟 SDK |

## 2. 固定上游来源

以下均实际读取；行号用于定位，不表示逐字复制整段实现。

| 来源 | 核对结果 |
|---|---|
| [Codex workspace 依赖](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/Cargo.toml#L456)、[codex-mcp 清单](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/codex-mcp/Cargo.toml) | MCP 业务层与底层客户端分离；rmcp 3.5.1 固定 SDK rev `98a54789b80110ad210495c4ad845c89e1980471`，不宜整体搬 Codex crate 及其执行、插件、鉴权依赖 |
| [mcp_types.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/config/src/mcp_types.rs#L223) | 配置区分 stdio／Streamable HTTP，含超时、环境引用及允许／禁用名单；首版 JSON／私有根／默认关闭为本项目提案 |
| [tools.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/codex-mcp/src/tools.rs#L65) | 过滤原始工具名；允许名单 None 与空集合语义不同，禁用优先。模型名与原始身份分开、净化及哈希后稳定排序；上游使用 128 字节上限，需适配 CC |
| [工具 Schema 转换](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tools/src/mcp_tool.rs#L38)、[相应测试](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/tools/src/mcp_tool_tests.rs) | 上游对 properties 缺省／null 补空对象，并保留独立输出 Schema；CC 首版仅声明输入 Schema，输出保存在 tool 文本 JSON |
| [pagination.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/codex-mcp/src/pagination.rs)、[分页测试](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/codex-mcp/src/pagination_tests.rs) | 100 页、2048 项、游标 64 KiB、重复游标检测和整体 timeout；空游标是有效值。草案沿用这些边界，不自行添加「空页即结束」 |
| [MCP 默认超时](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/codex-mcp/src/rmcp_client.rs#L112) | 启动 30 秒、工具 300 秒；旧版本或文档里的 10／60 不能混入本次基线 |
| [connection_manager.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/codex-mcp/src/connection_manager.rs#L1144)、[目录绑定](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/codex-mcp/src/connection_manager/tool_catalog.rs) | 执行再次校验服务器／名单，以原名调用；目录、客户端与绑定有明确关联。本地取每轮固定快照的必要部分，不搬插件绑定框架 |
| [MCP handler](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/mcp.rs)、[结果完成分类](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/mcp_tool_call.rs#L1052) | MCP 有专门业务调用和事件链，`is_error` 影响状态；HTTP 成功不能等同工具成功。CC 不能直接复用 Responses 的结构化结果投影 |
| [环境构造](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/rmcp-client/src/utils.rs#L16)、[本地传输关闭](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/rmcp-client/src/local_stdio_transport.rs#L92) | 环境采用平台白名单及显式附加项；关闭 stdin、最多等 3 秒再 kill。后代进程组回收仍须按本项目要求真实验证 |
| [有界 stdio 传输](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/rmcp-client/src/bounded_stdio_transport.rs#L22) | 上游新生命周期路径提供 8 MiB 行界限；legacy 路径用另一传输，因此不能声称本文 Initialize 路径天然已满足接收限额 |

SDK 也固定在相同 rev，直接核对了下列边界：

| SDK 来源 | 结论 |
|---|---|
| [workspace 清单](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/Cargo.toml)、[rmcp 特性](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/Cargo.toml) | 版本 3.5.1，Rust 最低 1.88，Apache-2.0；本地记录工具链高于此最低值，但实际依赖编译未执行，不宣称兼容验证已通过 |
| [ProtocolVersion](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/model.rs#L166)、[ClientLifecycleMode](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/service/client.rs#L637) | `LATEST` 为 2026-07-28，`LATEST_WITH_INITIALIZE` 为 2025-11-25；Initialize／Discover／Auto 是不同入口。首版明确选 Initialize，不以默认 LATEST 冒充旧握手 |
| [RequestHandle 与取消](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/service.rs#L555) | timeout 与 cancel 有对应请求通知接口；通知自身和本地等待仍需有界，不能据发出取消声称远端副作用撤销 |
| [HTTP 恢复配置](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/transport/streamable_http_client.rs#L2074) | `reinit_on_expired_session` 会在 404 后恢复会话并重发 POST；本地 006 不重放契约要求显式关闭，必须以写操作计数测试验证 |
| [默认 HTTP 客户端](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/transport/common/reqwest/streamable_http_client.rs#L398) | 关闭自动重定向，含自定义头泄漏回归；本地若自建带限额的客户端，须保留这一配置及验证 |

官方当前 [MCP 文档](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)确认 stdio、Streamable HTTP、环境变量／Bearer 与 `/mcp` 入口；这些属于参考功能范围。[Chat Completions API](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)确认 function 名字字符集及最大 64；草案的总工具数 128 为本地首版限制，不据当前页面推导各家提供方的额度。

## 3. 草案取舍及其原因

- 选择官方 SDK＋最小宿主适配，而不是重写协议或整体移植 Codex runtime；现有 Tokio 可作为接入基础，新增依赖在实施时固定并实编。
- 可执行配置放用户私有根、默认关闭，避免加载一个仓库就自动运行其中的服务器命令；不增加审批弹窗或安装工作流。此项与 JSON 字段子集均为待实施的规格提案。
- 原始身份与模型名分开，按轮固定定义、映射和连接；解决多服务器同名、CC 名字限制及通知刷新导致的在途错绑。
- stdio／HTTP 共用调用语义；首版只投影标准结果 JSON，资源接口与图片附件后置，复用现有 V1 历史和通用工具显示。
- 取消与传输故障保留远端状态未知；关闭 SDK 404 POST 重放，避免把 006 的不重试保证丢在传输层。
- `/mcp reload` 与模型 `/refresh` 分开；文件错误保留原有效配置并明确提示，合法停用／移除不允许失败回退复活旧连接。

## 4. 本轮验证与局限

本轮仅写规格，按 AGENTS.md 的文档例外检查内容、命名和链接，不执行 TDD、Cargo、comprehensive-testing、安全测试或代码审查门禁。§5 的用例全为预期，尚无 MCP 功能通过数量。

- 固定源码：GitHub 目录树与上表来源均已实际读取；链接使用固定 SHA，不依赖浮动 main。官方页面已打开核对。
- 文档检查：通过。两份新文档的 16 处本地相对链接均有效；23 个去重后的固定源码链接均与已读取目录树的 SHA、文件及定位行号一致；2 个官方页面已打开核对；1 个 JSON 配置示例解析成功、无重复键；A01–A18 共 18 项验收编号连续且均标为待测。新文件逐行空白／末尾换行及相关已跟踪文档的 `git diff --check` 通过；这些不是功能测试结果。
- 工作区保护：修改前保存工具对比与经验文件的原始副本，交付前与副本比较，只保留一行 MCP 状态更新和新增经验索引；不回滚任何原有未提交内容。
- 环境：默认 shell 沙箱因 `bwrap` 无法创建 namespace 而启动失败；只读检查通过已获自动审核的沙箱外命令完成。这是调研环境限制，不是本项目测试失败。
- 未执行：依赖安装／编译、MCP 探针、服务启动、在线模型调用、功能测试、Windows ConPTY 和性能测试。未读取真实凭证、未修改运行配置。

## 5. 规格复核与分支准备（2026-10-24）

### 5.1 实际 Git 状态与归属

- 工作区绝对路径：`/workspace`；单一工作树，开始位于本地 main。
- 实际 main／开始 HEAD：`f662cf6e4a7881923487ac40b395a20efdb77c31`，与用户给定预期一致。该提交已交付 021；上文的 `6b8f3a3` 是起草时的历史现场，本轮不再当作实施基线。
- 初始暂存区为空，程序、测试和依赖无未提交差异。已有 15 个未提交项：8 项历史证据删除、2 项前轮索引修改（工具对比、project-lessons）、3 份无关未跟踪记录、022 规格与本记录两份未跟踪文档。
- 删除项为 2026-10-10 三份 test-inventory、2026-10-14 两份布局／复用清单、2026-10-20 的 015 验证日志与清单、2026-10-21 git-sanitization-commit-map；保持删除，不恢复、不暂存。三份无关记录为 `2026-10-21-opencode-global-agents-skills-install.md`、`2026-10-21-vault-credential-recovery.md`、`2026-10-22-codex-command-execution-and-process-interaction.md`，逐字节保留。
- 本轮只修订现有 022 规格并在本记录追加复核结果，未改工具对比和 project-lessons。对其余 13 个遗留项核对存在状态／SHA-256，前后一致；本轮原文与状态备份在临时目录 `/tmp/terminal-chat-022-review-hkq_1jx2`，该备份不作为持久证据依赖。
- 关键契约明确、文档检查通过后，从上述 main 创建并进入普通分支 `feat/022-mcp-tools`；未使用新工作树、stash、reset 或强制切换。
- 当前分支 HEAD：`f662cf6e4a7881923487ac40b395a20efdb77c31`；合并目标为**本地 main**，其指针保持同一提交。尚无 022 提交，准备完成不等于实施、合并或交付授权。

### 5.2 已明确契约与实际补缺

| 复核项目 | 原有明确内容／本轮补缺 |
|---|---|
| 配置与启动信任 | 原 §2.1 已明确私有根、默认关闭、显式启用、传输互斥与环境引用。补明 stdio 以宿主用户身份执行、白名单并非沙箱、启用后不逐次审批；不增加审批或权限系统 |
| 生命周期与未知状态 | 原 §2.2／2.4／2.5 已覆盖初始化、断连、调用总时限、取消有界、3 秒关闭及退出显式回收。补明准备取消零模型请求与候选清理、业务失败和断连区分、重列期间通知不丢失／每轮至多一次；取消停止等待不保证远端停止 |
| 名称、快照与隔离 | 原 §2.3／2.4 已明确原始身份映射、64 字符、冲突拒绝、按轮定义＋映射＋连接快照、执行再次校验、8／32 预算、MemoryMerge 隔离；保留这些契约，不另造通用代理 |
| 参数与结果 | 补明 Schema 根类型、SDK 五类内容、structuredContent 任意 JSON 及 null／缺省区别、未知类型／错误结构整次失败；不承诺超出 SDK 表征的未知字段保真。标准结果数据保留受下述凭证规则约束 |
| 未支持的服务器交互 | 补明不声明 sampling／roots／elicitation／Tasks，反向请求统一 method-not-found；不递归调用模型、读工作区、开启问询或 MRTR／Task 后续请求 |
| 结果含凭证 | 发现原“完整结果／元数据保留”与未定义的凭证回显处理存在实质缺口，已提问。用户选择**拒绝受污染数据**：污染定义不发布、污染结果替换标准客户端错误，已发调用为未知；检测先于模型／落盘／展示／日志及截断。显式 env 引用值按敏感值处理，避免名称猜测；不修改其余历史或新增通用秘密扫描服务 |
| 保存恢复 | 原 §2.5 已明确复用 V1、恢复只展示不重放、缺结果为未知、后续新输入才取得当前能力；保留原契约，A14 补明命名快照的字节保护 |
| 本地实际基线 | 修正规格中“已有未提交 021 工作”的历史表述，明确 021 已在 f662cf6 交付；仍须保护其余遗留项与实施时的并行改动 |

### 5.3 固定源码复用与本地适配补证

继续采用既定 Codex SHA 与 SDK rev，未转向最新版本。此前已读来源直接复核；为内容联合与回调默认值补读下表固定文件。只读源码，不启动本项目 MCP 客户端／服务或访问业务端点。

| 固定来源 | 上游可复用事实 | 本地明确适配 |
|---|---|---|
| [SDK 单次与高层调用](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/service/client.rs#L1934) | `call_tool_once` 发单次请求；高层 `call_tool` 会驱动 input_required 后续轮 | 首版使用单次／可取消入口，拒绝非 complete 结果，避免范围外隐式多发 |
| [标准 CallToolResult](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/model.rs#L4124)、[内容联合](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/model/content.rs#L252) | 五类内容、可选 isError／元数据、structuredContent 的显式 null 区分；非 complete 不是普通结果 | CC tool 文本投影；未知内容／结构失败；污染数据在投影前拒绝，不是直接照搬 SDK 序列化 |
| [客户端回调](https://github.com/modelcontextprotocol/rust-sdk/blob/98a54789b80110ad210495c4ad845c89e1980471/crates/rmcp/src/handler/client.rs#L88) | 默认 sampling 报 method-not-found，roots 返回空集合，elicitation decline | 本地不声明三项能力，违规反向请求均明确拒绝；不能将本地拒绝策略记作上游默认 |

上游过滤、名称、分页、Initialize 选择、超时与 HTTP 404 重放开关继续引用 §2 的固定来源；本地私有 JSON、默认关闭、CC 64、快照、凭证污染拒绝及通用卡片仍是宿主适配。依赖兼容与传输限额探针留在获准实施后的第一阶段，本轮不安装或编译。

### 5.4 验收及后续必做门禁

- 规格仍为固定五节；A01–A18 保留 18 项，现逐项独立列出输入／动作、预期结果与运行状态、副作用、验证方式及实测状态，全部为**待实施／待测**。没有把静态复核计作功能通过。
- L1 覆盖配置、名称、Schema／结果、敏感数据拒绝和受控时钟；L2 用真实 SDK／turn／V1 与可控本地 stdio 子进程、HTTP／SSE；L3 为真实 CLI、TUI PTY 屏幕与宿主退出。凭证仅固定测试 canary，无真实账号或认证；未知平台条件如实记未运行。
- 后续顺序为 `TDD → comprehensive-testing → Security Testing Gate → code-review → 全量验证和修复`。Security Testing Gate 必做，已核对本机[代理说明](../../.agents/agents/Security-Testing-Gate.md)的先审查／再实测要求与 `docs/security/reports/` 落盘规则；MCP 本地边界攻防范围见规格 §5.2。
- code-review 必做标准／规格两个轴，实施完成后固定实际合并基线审查程序差异；当前 HEAD 无 022 实现提交，不启动该门禁或假称通过。错误按 diagnosing-bugs 定位，复验受影响项后再完整验证。
- 已对照 [README](../../README.md) 和[后端回归说明](../testing/backend-regression.md)核实完整入口：工作目录 `/workspace`，`cargo check --locked --offline && cargo test --workspace --locked --offline --no-fail-fast`，不带名称过滤。它与 AGENTS.md §7 是同一 workspace 全集，额外保证锁定／离线；缓存须预先就绪。隔离副本两个 Cargo 命令均用专用 `--target-dir`。本轮未执行。

### 5.5 本轮检查与移交状态

- 适用检查：5 个顶层章节、18 项六列验收且全部待测、1 个 JSON 示例（无重复键）、代码围栏、22 处本地相对链接、27 个去重后的固定源码链接（SHA／文件／行号）及空白；全部通过，`git diff --check` exit 0。安全门禁、本地传输探针、Cargo、lint、程序测试与平台验收均为本阶段**未运行**，不是失败或通过数量。
- 工作区保护检查：除本轮两份 022 文档外，13 个原有未提交项状态／字节保持；8 项删除保持删除，暂存区仍为空；程序、测试和依赖与 main 无差异。
- 一次保护脚本误报已定位：对 Git 文本整体 `strip()` 丢掉首项状态的前导空格，并非工作区变化。改用 `git status --porcelain=v1 -z` 的状态／路径结构比较后，15 项清单完全一致，零新增、零遗漏；未因此改动任何程序或用户内容。
- 随后的最终检查另捕获一项真实并行变化：新增未跟踪的 `docs/规格文档实战法.md`，总清单由初始 15 项变为 16 项；不属于本轮写入，未读取或修改其内容。改按“原清单各项状态／哈希仍一致，并单独报告新增项”检查，不要求并行工作区整个清单冻结，也不恢复或删除该文档。
- 真实阻断：**无**。凭证回显公共结果契约已由用户选择；SDK 编译、传输及 Windows 验证是后续实施任务，不把它们伪报成本阶段已验证或当前阻断。
- 准备已完成，返回 Supervisor 复核；未提交、合并、推送、删除分支、启动 /goal 或开展实现。
