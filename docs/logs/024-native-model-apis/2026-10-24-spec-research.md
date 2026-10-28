# 024 原生模型接口：规格依据

- 日期：2026-10-24
- 对应规格：[024-native-model-apis.md](../../specs/024-native-model-apis.md)
- 本轮范围：读取新项目规范、现有实现与官方资料，编写规格；未修改运行时代码、启动项目、发送真实模型请求或执行程序测试。
- 本地基线：分支 `feat/023-mcp-resources`，HEAD `ad7959222e7227afecb6811f38ca2b62ea6ef7f2`。工作区已有 AGENTS、文档及 MCP 资源相关改动；本次保留这些改动，不将其算作 024 的实现。

## 1. 新规范与已确认范围

已读取 [AGENTS.md](../../../AGENTS.md)及其引用的[规格文档实战法](../../../rules/规格文档实战法.md)：规格固定五节；新证据进入 `docs/logs/`；文档改动只检查内容、命名和链接；实现与验收分开；没有性能基线时不编造数字。

用户已要求加入 Gemini 在内的原生支持，先处理接口，再继续工具计划。因此不再沿用早先“Messages 是否接入另定”的建议。旧的[接口讨论记录](../../records/2026-10-24-model-api-protocol-comparison.md)保留为历史，并加上当前决定入口。

本规格是一个功能：**让同一个 Agent 通过明确选择的原生协议完成模型与工具多轮交互**。四类接口共用业务、配置和保存约束，放在同一规格；Gemini 的两种原生线路分别命名，不隐藏切换。

## 2. 官方接口事实

以下资料通过官方页面读取；开源实现使用固定提交。只读资料不等于完成真实服务验证。

| 来源 | 核对结论 | 对规格的影响 |
|---|---|---|
| OpenAI [迁移到 Responses](https://developers.openai.com/api/docs/guides/migrate-to-responses)、[函数调用](https://developers.openai.com/api/docs/guides/function-calling) | Chat Completions 仍受支持，Responses 是官方建议的新项目接口；Responses 使用不同种类的 Items，函数结果按 `call_id` 配对 | 保留兼容通道，新增真正的 Responses 适配；不能把 Item ID 当函数调用 ID |
| OpenAI [推理状态](https://developers.openai.com/api/docs/guides/reasoning) | 无状态请求可携带输出与必要的 reasoning Items；当前 `store:false` 场景默认返回 `encrypted_content`，旧的 `include` 写法仍兼容 | 使用客户端历史，保留不透明续接内容，不把它压成最终文字；不依赖旧示例必须手动 include 的假设 |
| Anthropic [Messages API](https://platform.claude.com/docs/en/api/messages/create) | 原生 Messages 具有工具定义、`tool_use`／`tool_result` 和输出上限等自己的结构 | 原生鉴权和内容块单独编码；不用 OpenAI 兼容入口冒充 |
| Anthropic [定义工具](https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools)、[thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | 工具参数使用 `input_schema`；工具轮需要按协议保留相关 thinking／签名内容和顺序 | 共同层执行工具，适配器保留原生块；保存、恢复不能只处理可见文本 |
| Anthropic 官方 SDK [版本常量](https://github.com/anthropics/anthropic-sdk-python/blob/202db72c87d7c6eaa72129c67ce67fd343a3a20c/src/anthropic/_constants.py) | 固定提交声明 `ANTHROPIC_API_VERSION = "2026-10-21"`，SDK 默认还带重试策略 | 规格明确初始版本；接入客户端时必须核查并关闭自动重试，不照搬默认超时与重试 |
| Google [Interactions 概览](https://ai.google.dev/gemini-api/docs/interactions-overview) | 截至本次查询，Interactions 自 2026-06 起正式发布并推荐用于新项目；`generateContent` 仍完整支持；Interactions 默认保存状态，可设置 `store=false` | Gemini 原生不再只等于 generateContent；新入口明确选择，使用无状态历史 |
| Google [Interactions API](https://ai.google.dev/api/interactions-api) | 参考页默认展示 `/v1beta/`，同时明确提供稳定 `v1`；模型响应含有序 Steps，函数结果的 `call_id` 对应调用的 `id` | 选择正式版路径，固定其样本；不能仅照旧资料读取 `outputs` 数组，也不因失败转 beta |
| Google [函数调用](https://ai.google.dev/gemini-api/docs/function-calling) | Interactions 无状态调用要求完整携带用户输入、原样模型 Steps（包括 thought／function_call）和函数结果；旧原生协议有自身的 Parts 和签名约束 | Gemini 两条线路分别转换和回放；只提取函数名和参数不足以续接 |
| Google [generateContent API](https://ai.google.dev/api/generate-content) | 传统内容生成与流式生成有不同原生操作入口 | 保留单独的 `google-generate-content` 协议和原生流式路径，不透过 OpenAI 兼容桥接 |

Google 正式版路径的选择依据是参考页的稳定版说明；本轮没有向该推理端点发送请求。Anthropic 版本同样只是文档／SDK 依据，后续必须用匹配版本的固定样本和真实服务烟测分层证明兼容性。

## 3. 开源复用边界

- Codex 固定提交 `970111f6cb62cc02fe36a82ea7d71b37a2889c97` 的 [WireApi 与 provider 配置](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/model-provider-info/src/lib.rs)只保留 Responses，显式拒绝 `wire_api="chat"`。这是本项目新增原生 Responses 的参考，不能据此删除仍有价值的兼容通道。未证明可以直接搬入其整套 Agent。
- Rust 候选 [Rig](https://github.com/0xPlaygrounds/rig/tree/55cd481a2421fd5da468b5bd6afc6ac876883599/crates/rig-core/src/providers)固定到 `55cd481a2421fd5da468b5bd6afc6ac876883599`，已确认有 OpenAI、Anthropic、Gemini provider 实现。**本轮只核实候选与目录，未做依赖选型、编译或原生状态往返探针**；不能宣称它已满足 Interactions、V2 保存或无重试要求。
- 官方 Python SDK 只作为版本／字段依据，本项目仍为 Rust；不因查了 SDK 就引入 Python 运行依赖。
- 复用前需证明无隐含重试、可取消、有界接收和原生块往返；如果只满足一部分，复用该部分，补最小适配，不新增独立 Agent 执行框架。

## 4. 本地必须解决的问题

| 已读取的实现 | 当前现状 | 规格要求 |
|---|---|---|
| [catalog.rs](../../../src/app/src/catalog.rs) | 协议常量仅 `openai-chat`；已有 `protocol`、`key_ref`、请求参数、模型能力和连接身份 | 复用配置结构和身份机制，扩展明确协议枚举及原生模型目录 |
| [api.rs](../../../src/app/src/api.rs) | 对话、列表、鉴权和回复解析以 Chat Completions 为前提；已有请求／响应 10 MiB、连接 10 秒、总计 180 秒、禁止重定向 | 保留这些边界，各协议独立编解码；错误显示沿用已有凭证保护，不能扩成“所有内容都脱敏”的承诺 |
| [main.rs](../../../src/app/src/main.rs)、[tui_host.rs](../../../src/app/src/tui_host.rs)、[memory/runner.rs](../../../src/app/src/memory/runner.rs)、[memory/phase1.rs](../../../src/app/src/memory/phase1.rs) | 普通对话和内部记忆均存在直接 Chat Completions 请求／解析依赖 | 所有生产调用一起适配，不能只换主界面入口 |
| [prompt.rs](../../../src/app/src/prompt.rs) | 现有共同历史保存文本、工具调用、结果和图片，无法完整表达厂商原生推理／签名块 | 增加必要的原生状态保留与归属约束，不用 Chat Completions JSON 当通用历史 |
| [session.rs](../../../src/app/src/session.rs)及 [007](../../specs/007-session-persistence.md) | 会话 meta 为 V1；装载器拒绝未知版本；未知普通记录会被忽略 | 不能把重要原生状态随意塞进可忽略扩展；明确 V2，新程序读两版，旧程序拒绝 V2，V1 原文件不混写 |
| [006 工具循环](../../specs/006-tool-call-loop.md)及 [022 MCP 工具](../../specs/022-mcp-tools.md) | 结构错误整批拒绝；未知函数名、非法 JSON／工具参数错误按单项失败回传，同批合法调用照常执行 | 交叉核对时修正草稿中过度整批拒绝的表述，五协议保留既有失败语义，以 N21 单列验收 |

这些是只读检查结论，不是已修复问题。MCP 资源分支内另有在途实现，本规格仅追加先后顺序，不改动其运行时代码。

## 5. 本次交付与验证

- 新增 024 规格、文档目录和本记录；为 023 加接口优先说明；为历史接口讨论和项目经验加当前决定指针。
- 024 按五节编写，状态为“待实现”；性能数值待最小原型基线，功能验收项均为待测；未把资料核对写成程序通过。
- 文档检查完成：6 份相关文档、024 固定五节、21 个验收项均为待测、90 个本地链接有效；目录覆盖全部 24 份规格。使用 Python 3 检查结构、链接、尾部空白和文件换行，退出码 0；本次相关路径的 `git diff --check` 退出码 0。交叉核对 006 后已修正工具错误语义。原有文档的 CRLF 提示未通过格式整理消除，不改动用户文件。
- Cargo、E2E、性能、安全测试和真实模型调用：本轮未运行。原因：只改文档，AGENTS 明确要求不执行程序检查和全量测试。
