# 032 上下文管理：规格依据

- 日期：2026-10-26；工作目录：`/workspace`。
- 授权范围：原表「上下文管理」一项，两个工具和相关预算/压缩/历史统一形成[规格与实施计划](../../specs/032-context-management.md)，不推进下一项「JavaScript 工具编排」。
- 状态：待实现；本轮只读源码并写文档，没有构建/测试、模型计数或压缩请求，没有更改会话文件。

## 当前实现与设计依据

| 核对点 | 源码/规格 | 对032的约束 |
|---|---|---|
| 现有注入 | [015](../../specs/015-turn-context-injection.md)、[context.rs](../../../src/app/src/context.rs)固定32768字节规则正文 | 是注入字节预算，不是模型剩余token；每轮读取和去重继续保持 |
| 当前请求 | [prompt.rs](../../../src/app/src/prompt.rs)将完整共同history投影，turn还传_history；[turn.rs](../../../src/app/src/turn.rs)无两工具，固定8请求/32调用 | 活动视图要同时进入messages与_history；不能reset额度或只改一条协议路径 |
| 近似换算 | [memory/truncate.rs](../../../src/app/src/memory/truncate.rs)复用[shim truncate.rs](../../../src/shims/codex-utils-string/src/truncate.rs)的ceil(bytes/4) | 可复用近似函数，不称精确token。要计整个编码快照和工具/系统，不能只算可见正文 |
| 容量字段 | [catalog.rs](../../../src/app/src/catalog.rs)有context_length/max_output和实验工具开关；0仍可保留，未知不按模型名猜 | 计量要处理未知/0/无输出预留/溢出，不能用默认假值冒充真实容量 |
| 编码与usage | [api.rs](../../../src/app/src/api.rs)在post前实际序列化10MiB检查；[native.rs](../../../src/app/src/api/native.rs)及[Anthropic](../../../src/app/src/api/native/anthropic.rs)/[Gemini](../../../src/app/src/api/native/gemini.rs)编码与post相连，usage只是返回数据 | 抽最小纯编码供计量与发送共用；历史usage不当成当前余量，多模态/opaque无法计量时记unknown |
| 原生状态 | [024](../../specs/024-native-model-apis.md)与NativeAssistant保存原始块和owner，续接不得伪造 | 新窗口显式舍弃旧活动链，原始块仍完整留档；不把摘要伪装成签名或已有真实结果 |
| 会话 | [007](../../specs/007-session-persistence.md)、[session.rs](../../../src/app/src/session.rs)支持V1/V2、追加游标、锁和提交点；普通on_record无保存成功回执 | 需要V3检查点和专用回执；旧程序明确拒绝，不能先内存切换再异步说保存失败 |
| 下游读取 | [memory/rollout_input.rs](../../../src/app/src/memory/rollout_input.rs)独立校验v=1或2、反向预算扫描；[runner.rs](../../../src/app/src/memory/runner.rs)登记durable history | V3必须用真实文件验证全链路，不把checkpoint/seed复制当成新经历，也不重复扫描全历史 |
| 持续进程 | [proc_session.rs](../../../src/app/src/proc_session.rs)live_sessions含“已退出未取输出”记录；[016](../../specs/016-process-sessions-plan.md)进程跨工具调用和用户轮 | 重设活动窗口不重开对话或重置进程；运行说明不能把全部登记句柄误报正在运行 |
| 同轮输入 | [018](../../specs/018-user-input.md)/[021](../../specs/021-send-message-to-user-async.md)已有专用答案和接续输入提交点 | 未答问询阻止切换，批末已入史输入纳入seed；新到输入按通道时序继续，不静默丢弃 |

工作区状态与前轮不同：多处先前源码改动已不在未提交列表中。本轮按当前文件核对，没有还原、暂存或重写这些变化；029–031仍为文档方案，不能据“继续下一项”标成实现/验收通过。

## 实际读取的固定上游

GitHub连接器按 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4` 读取：

- [get_context_remaining.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/get_context_remaining.rs)、[spec](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/get_context_remaining_spec.rs)：空对象输入，tokens_left可未知；本项目额外标记estimate/unknown与计量口径。
- [new_context_window.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/new_context_window.rs)、[spec](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/new_context_window_spec.rs)：工具名new_context，上游输入为空，不自动总结、不清环境。032主动增加summary，明确为本地适配，而不是把上游reset误写为自动摘要。
- [session/context_window.rs](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/session/context_window.rs)：活动窗口/自动压缩阈值与任务累计用量分开。本项目没有相同的计数/预算服务，不照搬上游数值或认为本地usage已具备该能力。

已实际打开[OpenAI Docs计数说明](https://developers.openai.com/api/docs/guides/token-counting)与[压缩说明](https://developers.openai.com/api/docs/guides/compaction)：原生计数/托管压缩需要各自协议及服务，本项选择离线估算和模型交接，不增加网络请求，不宣称精确计数、原生缓存收益或免费。

Gemini输出预留的位置额外按实际打开的 [generateContent参考](https://ai.google.dev/api/generate-content)与[Interactions参考](https://ai.google.dev/api/interactions-api)核实，分别为generationConfig.maxOutputTokens和generation_config.max_output_tokens；按最终编码请求读取，不按SDK参数别名猜测。

## 方案边界与未测事项

接口、状态、格式和验收统一见规格，不复制第二份清单。关键取舍是显式summary、完整transcript保留、批末提交成功后改变活动视图、V3防旧程序忽略检查点，以及不重置逻辑用户轮/运行环境。

64KiB摘要/128KiB参数与一次切换/轮是新增边界；10MiB请求、64MiB会话行和8/32预算分别来自既有契约。没有性能、token准确率、实际模型摘要保真或费用实测；M0必须冻结原型/样本和性能阈值。

## 文档验证

本轮仅检查项目范围、编号、五节、实施计划、用例待测、JSON与本地链接/空白。首次检查在[2026-10-26-doc-check.log](2026-10-26-doc-check.log)发现项目经验标题锚点不匹配，已按实际标题修正；修复后的命令、工作目录、退出码和原始输出见[2026-10-26-doc-check-2.log](2026-10-26-doc-check-2.log)。检查不代表功能已实现或运行验证通过。
