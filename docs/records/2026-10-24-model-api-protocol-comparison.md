# 模型接口核对：Chat Completions、Responses 与 Messages

- 日期：2026-10-24。
- 目的：先判断长期接口路线，再写工具搜索实施计划。
- 范围：OpenAI 官方接口说明、Codex 开源源码、Claude Code 和智谱／Z.ai ZCode 的官方接入文档。只做源码与文档核对，未改程序或调用真实模型。

后续决定（2026-10-24）：用户已要求保留 Chat Completions，并加入 Responses、Anthropic Messages 和 Gemini 原生支持，先处理接口，再继续工具实现计划。下文“尚未作为实施决定”的建议保留为讨论记录，当前要求以 [024 原生模型接口](../specs/024-native-model-apis.md)为准；新增调研见其证据链接。

## 1. 先分清两个问题

**Chat Completions 是成熟、仍受支持的接口，可以长期作为兼容通道。项目只实现它，是当前范围的取舍，不能据此把接口本身叫作临时 MVP 方案。**

OpenAI 同时明确推荐新项目使用 Responses，并建议现有业务逐步迁移。这说明“仍能长期使用”和“是否最适合承载新的 OpenAI Agent 能力”需要分别判断。[官方迁移说明](https://developers.openai.com/api/docs/guides/migrate-to-responses)

这已涉及实际能力：当前官方说明中，GPT-6 Astra、GPT-6.1 Sol 的工具调用要求 Responses；GPT-6 Sol、GPT-6 Luna 通过 Chat Completions 调用函数时，只支持 `reasoning_effort: "none"`。这条限制针对这些 OpenAI 模型，不推及其他厂商的兼容接口。[官方部署检查清单](https://developers.openai.com/api/docs/guides/deployment-checklist#choose-a-model-for-the-workload)

## 2. 三个产品实际使用什么

这里比较 Agent 发出的模型请求格式；网关后面可以再转换协议，不能把网关的上游协议当成客户端协议。

| 产品 | 已确认的模型协议 | 依据与边界 |
|---|---|---|
| Codex | Responses | 本次读取 main 固定到 `970111f6cb62cc02fe36a82ea7d71b37a2889c97`。`WireApi` 只保留 `Responses`；配置 `wire_api="chat"` 会明确报不再支持。HTTP／WebSocket 是传输选择，不是 Chat Completions 与 Responses 的区别 |
| Claude Code | 默认是 Anthropic Messages；另支持 Bedrock InvokeModel 和 Google rawPredict 等厂商封装 | Anthropic 格式的推理端点为 `/v1/messages`。它不属于 OpenAI Responses／Chat Completions 的二选一 |
| ZCode（智谱／Z.ai） | 可确认支持 Anthropic 和 OpenAI Chat 兼容接入 | 官方文档明确列出两种协议，并讨论 OpenAI Chat 兼容通道的能力差异；没有找到足够官方依据确认其所有 OpenAI 通道是否采用 Responses，不能宣称统一只用某一种 |

来源：[Codex 协议枚举与 chat 拒绝逻辑](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/model-provider-info/src/lib.rs)、[Claude Code 网关协议](https://code.claude.com/docs/en/llm-gateway-protocol#api-formats)、[ZCode 连接模型](https://zcode.z.ai/cn/docs/configuration)。ZCode 此项只确认文档公开的能力，未将同名的其他开源项目当作其实现依据。

因此，不能用“主流 Agent 都采用同一种 API”来决定本项目路线，也不能仅凭“OpenAI 兼容”或 Base URL 推断具体协议。

## 3. 对本项目的判断

本项目[当前 API 层](../../src/app/src/api.rs)只实现 `openai-chat`，实际请求 `/chat/completions`；[请求组装](../../src/app/src/prompt.rs)和工具结果投影也以该格式为基础。这是已实现的兼容通道，仍有保留价值。

**规划建议，尚未作为实施决定：**

1. 保留 Chat Completions，继续服务已接入的第三方模型与兼容端点。
2. 若继续对齐 Codex 和新的 OpenAI Agent 能力，优先增加 Responses 通道；不要再把“始终只用 Chat Completions”当作工具搜索计划的固定前提。
3. 工具目录、检索、权限和执行尽量共用；模型接口负责各自的工具声明、请求和结果回传。Anthropic Messages 是否接入，按实际模型需求另定，不提前建设整套多协议框架。

这样可以继续复用 Codex 的检索思路，并分别决定 Responses 的原生搜索结果回传与 Chat Completions 的普通函数适配。

Responses 也允许手动携带历史并设置 `store:false`，不必为了接入它就把会话管理全部交给服务端。[状态管理与存储选项](https://developers.openai.com/api/docs/guides/migrate-to-responses#4-decide-when-to-store-state)

## 4. 本轮结论的使用边界

- 当前事实与建议分开：协议现状已核对，项目迁移顺序仍需结合目标模型确定。
- Codex 源码固定提交；产品和模型接口限制以本次查询日期为准。
- 未运行模型调用、协议兼容测试或性能测试；不据这些文档宣称迁移后的质量或耗时已验证。
