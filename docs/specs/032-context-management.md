# 规格：上下文管理

- 状态：待验收（实现方标记；最终由验收人判定）
- 更新时间：2026-10-28（M0–M7 实现完成；独立审查 25 项发现已修复并复验）
- 对应原表项目：[工具能力对照表](../records/2026-10-21-codex-tools-comparison.md)的「上下文管理」。`get_context_remaining`、`new_context`、预算、压缩策略与历史管理纳入同一项规格及实施计划，不拆成后续项目。
- 关联契约：[006 工具循环](006-tool-call-loop.md)、[007 会话保存](007-session-persistence.md)、[013 记忆](013-memory-system-v1.md)、[015 规则注入](015-turn-context-injection.md)、[016 进程会话](016-process-sessions-plan.md)、[018 问询](018-user-input.md)、[021 接续输入](021-send-message-to-user-async.md)、[022 MCP](022-mcp-tools.md)、[024 原生模型协议](024-native-model-apis.md)。
- 本轮只建立规格与实施计划，未改程序行为。下一项为原表「JavaScript 工具编排」，等待用户同意后才处理。

## 1. 要做什么，有哪些要求

### 1.1 目标与首版策略

让模型查询活动上下文的剩余空间，并在一个完整工具批次结束后，使用交接摘要建立较小的新活动上下文；原始对话、工具调用、结果与原生状态仍完整保存和展示。

首版选择**离线预算估算＋模型显式提供交接摘要＋持久化上下文检查点**，通过 024 的普通函数适配五协议。不开启后台自动摘要、不调用厂商计数/压缩接口、不自动裁剪历史、不重试已失败的请求；预算不足时由模型显式请求切换，无法安全切换就保持原窗口并报告原因。

上游固定版本的 `new_context` 接受空对象且不自动总结；本项目为交接当前任务，明确改为必填 `summary`。这是输入契约的主动适配，不声称无损摘要或完整复制上游。摘要由本次模型调用提供，不新增模型请求。

### 1.2 启用与作用范围

- 沿用模型目录 `experimental_supported_tools`，分别精确包含 `"get_context_remaining"`、`"new_context"` 才声明对应工具；不按模型名猜能力，不新增独立开关文件。
- `get_context_remaining` 需要当前模型/连接参数快照和只读请求计量能力；缺少 context_length 等信息可以返回未知，不能编造容量。`new_context` 还需要真实会话检查点提交宿主；无该宿主不声明、不执行，旧库入口默认关闭。
- 两工具都只供普通 `User/UserWithMemory` 轮，不开放给内部记忆提炼/合并，不通过 MCP 动态搜索延迟加载。未注册或内部轮的伪造调用沿用未知工具拒绝。
- 关闭工具开关时，V1/V2 的无检查点历史和请求行为保持原样；已经存在的 V3 检查点仍须正确读取和投影，不能因切换模型或关闭工具入口而重发已归档前缀。
- 切换活动上下文不结束用户轮、不新开逻辑对话、不清环境。即使 030/031 后续接入，也不能清除当前轮的工具加载集合、权限生命周期或请求额度。

配置示例（置于已有模型记录，保留其他字段）：

```json
{"experimental_supported_tools": ["get_context_remaining", "new_context"]}
```

### 1.3 get_context_remaining：输入与计量口径

输入必须恰为 `{}`，null、非对象、未知字段一律失败；不接受模型传入 token 数、容量或假 usage。

计量快照定义为**执行查询前已提交的活动历史前缀**，包括当前 assistant 工具批次声明和排在查询之前已经保存的结果，不包含本次查询的结果及尚未执行调用的未来结果。查询不改变历史或环境，随后按普通工具规则保存其调用与结果。

- 复用实际提供方请求的纯编码阶段，计算此快照对应的序列化请求 JSON 字节 `B`；包含 system/规则/记忆注入、活动消息、工具定义及请求参数，不包含认证头、UI display、内部 `_history/_provider_id` 或重复统计的中间表示。
- 只读计量允许编码一个尚未闭合的工具批次前缀；这个中间表示不能发送给服务端。若该协议无法表达该前缀，返回未知，不伪造工具结果来凑完整请求。真正发送前仍要求完整批次和全部真实结果。
- 对纯文本与普通函数内容，复用现有 `codex-utils-string::approx_tokens_from_byte_count`：`I=ceil(B/4)`。这个数是粗略估算，会受 JSON 包装、中文和厂商 token 化影响，不是精确计数或安全下界。
- 上下文窗口 `C` 取当前模型目录的正整数 `context_length`。输出预留 `R` 先取当前已校验请求实际设置的正整数输出上限：Chat 的 max_completion_tokens/max_tokens、Responses 的 max_output_tokens、Messages 的 max_tokens、Gemini generateContent 的 `generationConfig.maxOutputTokens`、Interactions 的 `generation_config.max_output_tokens`；未显式设置时以正整数 `model.max_output` 作为保守参考。多种互斥上限、零值、缺失或不合法组合不猜测。
- 可估算且 `0<R<C` 时，`tokens_left=max(0,C-R-I)`，明确标记 estimate。不存在统一固定预留值；不能用旧请求 usage、缓存命中或 reasoning 字段拼凑当前输入计量。
- 包含图片/文件，或适配器无法完整计量的 opaque/签名/未知原生块时，tokens_left 与 I 记 null、accuracy=unknown；已知 C/R/B 可独立保留。不得对 Base64、密文简单除以 4 后冒充模型占用。
- 不请求网络、不读取原图片/配置文件、不重列 MCP、不跑工具，也不重读本轮冻结的规则与记忆。计量数字不作为授权依据，不承诺保证服务端接受请求。

返回固定字段：`tokens_left / accuracy / context_window_tokens / output_reserve_tokens / input_tokens_estimate / input_bytes / context_id / unknown_reason / error`。context_id 是当前活动上下文代数，初始 0；accuracy 为 estimate 或 unknown。未知不是工具失败，error=null、unknown_reason 非空；参数或本地计量故障 error 非空，不改变上下文。

算例只说明口径，不是实测：若 B=32768、C=32768、R=4096，则 I=8192、估算剩余 20480。

```json
{
  "tokens_left": 20480,
  "accuracy": "estimate",
  "context_window_tokens": 32768,
  "output_reserve_tokens": 4096,
  "input_tokens_estimate": 8192,
  "input_bytes": 32768,
  "context_id": 0,
  "unknown_reason": null,
  "error": null
}
```

无窗口/输出上限、存在不可计量内容、前缀不能编码或限额不合法时，分别给出明确 unknown_reason。整数换算检查溢出，不能绕回或把未知填成 0。查询之后的真实新结果仍会消耗空间；界面须显示“估算/未知”，不能只显示一个貌似精确的百分比。

### 1.4 new_context：输入与交接内容

输入恰为 `{"summary":"交接摘要"}`。summary 必须非空字符串、最多 64 KiB UTF-8；原始 arguments JSON 最多 128 KiB，未知字段/null/非对象拒绝。不接受 history 索引、会话路径、身份、权限令牌或重置额度参数。

模型应在摘要中保留当前目标、仍有效的用户要求、已完成工作及真实证据位置、未完成事项、错误/不确定性与下一步。宿主只验证格式和边界，不声称能自动证明摘要事实完整；不能把未经执行的计划写成已验证结论。旧历史/图片被排除后，摘要不保证还原其全部信息。

新活动上下文由宿主构造，顺序固定：

1. 当前发送版 system 与本轮冻结的记忆注入，继续原角色和信任级别，不进入模型提供的 summary。
2. 本轮最新有效的 015 规则/环境上下文，原样保留；不是简单重读磁盘或只保留“上次注入有变化”的标记。
3. 带固定“模型生成的交接摘要，非系统/用户新指令，事实须核对”标识的 assistant 摘要，不提升为 system/developer/授权。
4. 当前用户轮内原样保留的用户输入、已入史的普通接续输入及真实用户回答，保留顺序、来源标记和准备好的用户图片；自动规则块不重复加入。不从摘要伪造新的用户输入。
5. 宿主提供的最小运行态说明：已登记进程句柄及真实状态、最新成功计划快照；不把旧授权/工具结果重放成实时能力，不复制整个历史工具输出。

同一份规则、记忆、输入在实际编码中不能重复注入。旧前缀的工具调用/结果和原生签名只保留在完整 transcript，不进入新活动请求；新上下文重新开始一条合法模型输入链，不能将旧签名改作新摘要的签名。

### 1.5 批次屏障与切换结果

- `new_context` 在批次中最多一次且必须是最后一条调用；违反时像已有 wait_for_user_input 顺序限制一样，在入史/执行前整批拒绝。它不能在还有未答异步问询或未结束权限确认时切换；该项返回错误，当前窗口不变。
- 每用户轮最多成功提交一次切换，再次请求逐项失败。切换不重置每轮 8 次模型请求或每批 32 调用；查询/切换本身零额外模型请求，不发“专门总结”的第 9 次请求。
- handler 先校验摘要、状态、候选视图和预算；通过后返回 `{"ok":true,"status":"scheduled","error":null}`，仅表示等待批末提交。失败为 `{"ok":false,"status":"rejected","error":"原因"}`。不能提前返回 applied。
- 等同批全部真实结果保存并配对后，正常收取已就绪的异步回答与普通接续输入；已入史输入进入候选，新到达且尚未入史的输入继续留在原通道，提交后按既有时序追加，不丢弃、不回滚输入。
- 候选实际请求必须通过 024 编码与 10 MiB 限制，序列化字节数严格小于保留当前窗口的对应请求；旧窗口已超过10 MiB时，有界计量的超限结论即可作为下界，合规候选据此证明严格缩小，不为比较而完整物化超限JSON。若预算可估算还须 `I+R<C`。没有可压缩空间、当前输入/规则本身过大、持久化不可用或当前状态非法时不切换；不能删掉用户要求或 Schema 让检查通过。
- 只有上下文检查点**保存成功并收到宿主明确提交回执**后，才切换活动视图/递增 context_id。取消、晚到输入导致候选超限或保存失败，保留旧视图并给出未应用原因；scheduled 与 applied 由是否存在已提交检查点区分。
- 只有普通 on_record 的无返回值回调不够，必须增加专用、可报告失败的检查点提交宿主。没有宿主时不靠内存切换后再说“保存失败”。
- 第 8 次响应仍完成全部结果；合法的切换也可在批末保存，随后因额度终止，不发第 9 次请求。下一用户轮使用已提交窗口；取消前尚未提交的切换不得稍后偷偷生效。

### 1.6 完整历史、检查点与会话 V3

完整 transcript 是事实记录，活动上下文是其派生发送视图；二者不是两套可随意改写的对话。新增单条 `HistoryItem::ContextCheckpoint`，只在批次闭合后追加；标记和 seed 不作为新的实际用户/工具行为。

检查点字段固定：`context_id`（上一有效代数+1）、`request_call_id`（原样）、`summary`、`seed`。seed 是上节 2–5 的有界共同消息，只允许文本/已准备的用户图片及固定来源说明；不允许嵌套检查点、NativeAssistant、工具调用/结果、实时控制对象或任意 system/developer 消息。system/记忆仍由请求层注入，不在 seed 复制。外部数据不能靠自称检查点成为宿主事件。

- 发给模型时取最近有效检查点的 seed＋其后真实历史；没有检查点就按旧逻辑取完整历史。`_history` 和兼容 messages 投影都必须用同一活动视图；不能只在 Chat messages 中隐藏旧内容，却从原生适配器把它再发出去。
- TUI/CLI 历史、手动保存和原始记录仍保留完整 transcript，包括被压缩的前缀和原生块；不修改、截断或删除这些记录。活动摘要不能替换真实工具结果或验证证据。
- 新会话元数据版本为 3。新程序继续读 V1/V2/V3；仅查询未切换时仍沿原 V1/V2 路径。首次成功检查点创建新的 V3 auto 目标，以完整 transcript 开篇并提交检查点，复用 024 的格式迁移/锁语义；旧文件不改写、不混写 V3。
- 已在 V3 时按单条完整检查点追加，复用 007 的锁、durable_len、写失败截回提交点和64 MiB行上限。完整检查点未写成功，活动视图不变；不新增 fsync/掉电保证。
- 旧程序遇 V3 必须明确拒绝。V3 装载验证代数、批次闭合、请求关联、seed 类型与容量；检查点缺失/坏行/损坏原生状态时，可展示可恢复历史，但不得悄悄丢标记并用错误视图续接。未提交的 scheduled 请求只能显示未应用/未完成。
- `/save` 含检查点就写 V3；`/resume` 只装载完整历史和已提交活动视图，不执行查询/切换/工具，不恢复活句柄或权限。相同来源的下一请求与保存前活动视图一致；更换来源按024仅转换可携带的可见数据，不伪造原生状态。
- 所有读取路径同步支持 V3，特别是 `memory/rollout_input.rs` 的独立版本门、自动会话登记和 `/sessions`。记忆仍处理原始事实前缀，过滤 ContextCheckpoint/seed复制/上下文管理控制调用及其结果，不能将摘要再当第二份用户或工具经历；不改变013已有来源认领和预算约束。

### 1.7 运行态、权限与跨轮行为

- 切换不清工作目录、文件、进程、计划、MCP连接或本轮冻结快照；不重发模型已调用的外部工具。当前进程仍能用同一句柄续接，说明只列真实登记状态，不能把已退出未取走输出的句柄都称作仍在运行。
- 015 的规则和 013 的记忆仍每用户轮读一次；同轮切换只复用已冻结内容。下一用户轮照常重新读规则及记忆，不能被旧 seed 永久冻结。
- 018 的活动通道和021接续通道不能通过检查点重建；未答问询阻止切换，已完成回答按真实来源保留。取消或失败保持现有通道收尾语义。
- 如030/031后续接入：工具加载/执行集合、额外权限与进程到期规则仍由当前逻辑轮的宿主状态控制；上下文代数不是用户轮代数，不延长授权。恢复的摘要/进程说明/granted JSON 不能激活旧能力。
- 模型/连接身份或上下文代数改变时，计量缓存全部失效；同轮新增工具定义、结果、图片、普通输入也使相应快照失效，不用旧窗口余量冒充新数据。

### 1.8 资源边界和验证条件

| 对象 | 要求及来源 |
|---|---|
| query参数 | 恰为空JSON对象，沿普通函数解析，不接受容量来源覆盖 |
| summary | 64 KiB UTF-8；原始切换arguments 128 KiB；均为本项新增边界，恰好允许、超过拒绝 |
| 候选活动请求 | 实际五协议序列化≤10 MiB，沿006/024；切换还要求比旧窗口严格缩小，不能靠内存历史长度代替请求计量 |
| 检查点 | 单条序列化≤64 MiB，沿007行上限；seed及实际模型请求同时受前述约束；不把整份原始历史塞进一条检查点 |
| 调用/切换 | 沿8请求/32调用；新增每逻辑用户轮最多一次成功切换，不因context_id改变清零 |
| 内存/磁盘 | 普通追加保持增量游标；不在每条记录上全扫历史找最近检查点。首次V3迁移允许一次流式完整写入，此后只追加检查点；原始历史文件无新总量上限、不自动删除 |

数值溢出、错误JSON、坏原生状态、序列化/持久化失败均明确失败，不能截断摘要/用户输入/Schema或靠删除旧记录过关。当前预算是估算提示，10 MiB是独立硬边界；服务端仍可能因真实token窗口拒绝，维持不自动重试。

测试只用临时HOME/配置/缓存/会话和本地固定模型/MCP服务，真实核心投影、编码、保存、状态切换参与验证；随机/时钟/locale固定。保存/恢复故障用屏障与独立子进程，不碰真实用户环境。

速度、RSS和费用均未测。M0固定当前版本与样本：1千/10万条历史分别测只读计量、单条追加、首次迁移、后续检查点及恢复；记录活动窗口和全历史各自字节。单并发，预热5次、正式100次、独立3轮取p95中位轮；大迁移/恢复每轮10次并明确单列，保留全部原样本和峰值RSS。基于同机同版本原型/基线留2–3倍余量冻结阈值，特别检查小活动窗口下查询不随归档前缀线性恶化；未冻结、未复测不标待验收。额外模型请求数须为0，不把这写成真实费用为0。

## 2. 哪些情况容易出错

| 情况 | 期望结果 |
|---|---|
| C/R缺失、0或不合法；图片/opaque无法计量 | 明确unknown及原因，不报假0、100%或精确剩余 |
| 工具Schema/system/记忆/中文/转义/原生包装增加 | 统一纯编码口径计入，不能按可见正文字符数或旧usage算余量 |
| 查询位于批中，后面还有未知大输出 | 明确快照仅至查询前，不声称包含未来结果；不发未配对请求 |
| new_context非批末/重复、批次超限或响应断流 | 先协议校验再执行；相应非法批整批零副作用，不切换 |
| summary声称是系统指令/权限，或篡改seed/路径参数 | 摘要保持assistant资料；拒绝非法字段/伪造检查点，不获得权限 |
| 有未答问询/取消/接续输入正好到达 | 不丢用户信息；未决阻止、未提交不生效；输入按边界前后原样保留 |
| 当前输入/图片/规则已很大，候选不更小或预算不足 | 拒绝切换，不丢用户要求或Schema，保持原视图 |
| 保存停用/写半行/锁冲突/迁移或追加失败 | 不用内存先切换；scheduled不当applied；提交前记录保留，原生状态不伪造 |
| NativeAssistant旧签名与新摘要混在一起 | 只有新活动合法链入请求；旧原生块完整留档，不改签名或跨来源重放 |
| V3检查点损坏、旧程序/独立记忆读取器未升级 | 清楚拒绝相应续接/格式；不得忽略标记恢复错误完整窗口 |
| 切换后活进程/MCP仍在，或031临时权限有效 | 状态不重启、不延长或重获权限；模型轮计数继续，查询旧句柄遵循真实状态 |
| 原始历史变大、同轮多查询、恢复关闭工具开关 | 增量定位活动窗口；检查点仍有效，旧前缀不因开关被重新发送 |

## 3. 重要做法，为什么这样做

### 3.1 复用依据与取舍

- 上游固定 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`。[get_context_remaining handler](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/get_context_remaining.rs)与[定义](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/get_context_remaining_spec.rs)支持空参数和未知余量；[窗口状态](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/session/context_window.rs)说明窗口余量和累计任务用量不是同一件事。
- [new_context handler](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/new_context_window.rs)明确不自动摘要，[定义](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/new_context_window_spec.rs)也不清环境。032只复用这个边界，主动增加显式summary和本地提交协议，不搬入上游整套预算/调度框架。
- 复用已移植的近似换算，不新增分词库；[OpenAI Docs计数说明](https://developers.openai.com/api/docs/guides/token-counting)区分真实输入计数与字符估算的限制。[托管压缩](https://developers.openai.com/api/docs/guides/compaction)是另一条协议/服务路线，不当成跨五协议的本地能力。
- 保留原始事实，发送活动视图；用V3让旧程序明确拒绝，避免在V1/V2里塞一个会被忽略的字段后声称恢复等价。仅在提交点检查持久化回执，不改007其他普通记录“保存失败仍可继续对话”的既有选择。

### 3.2 根据规格制定的实施计划

M0–M7全部属于原表「上下文管理」一项。

| 步骤 | 修改落点与工作 | 验证与退出条件 |
|---|---|---|
| M0 固定现状与基线 | 记录工作区及相关测试原有结果；固定五协议文本/工具/图片/opaque夹具、计量口径与输出上限映射；用小原型测增量追加/活动视图/迁移，冻结性能阈值 | 所有数值和unknown分支可判定；保存/加载/独立记忆读取路径已列全；明确与上游空参数reset差异，不先假设精确token |
| M1 分离纯请求编码 | 在`api.rs`及`api/native{,/anthropic,/gemini}.rs`抽取已有请求编码供只读计量和真实发送共用；计量终点为最终发送body，包括Gemini移除用于URL的model/stream后的定型阶段；原网络/认证/错误/限额逻辑继续复用 | 五协议原有出站黄金样本不变，计量与实际出站字节相符；计量零HTTP/零工具/零文件加载；不把内部字段/重复表示算两遍；未闭合前缀不发送 |
| M2 预算与查询TDD | 新增`src/app/src/context_window.rs`并从lib导出；复用近似计数，处理C/R/未知/溢出、代数及缓存失效；在catalog/prompt/TurnExtras按模型开关接通查询 | 空参数、估算公式、动态Schema、中文、图片/opaque未知和当前批快照全部红绿；旧入口关闭不改已有请求 |
| M3 V3检查点与提交回执 | 扩展`prompt::HistoryItem`、`session.rs`和专用checkpoint host；实现V1/V2读兼容、首次V3迁移/锁/单条提交、坏检查点阻断；候选与普通待写段隔离，明确取消与提交的裁决点；升级`memory/rollout_input.rs`及自动登记路径 | 保存前后完整事实相等；检查点成功才切换；半写/取消/恢复不误用；失败候选不被后续补写、退出flush或手动保存复活；旧程序拒绝V3、独立记忆能读V3且不重复提炼seed，计费仍沿013原口径 |
| M4 new_context与批末屏障 | `turn.rs`增加summary解析、批末位置校验、一次切换/轮、就绪输入处理及checkpoint提交；当前system/记忆/规则/用户输入与运行说明构造seed；以屏障在写前、半写、回执后插入取消和输入，对齐M3提交裁决 | 搜索/问询/进程/MCP原有状态不重建；切换保8请求32调用；候选变小且通过限额；未提交失败仍旧视图，已提交状态以回执和恢复记录为准；输入恰一次，scheduled/applied区别可观察 |
| M5 五协议、CLI/TUI与恢复 | 统一活动视图进入messages/_history及原生编码；`main.rs/tui_host.rs`传回执与模型限制；通用工具结果显示估算/未知，检查点显示已应用边界与摘要 | 真实CLI/PTY显示并可滚动看旧历史；五协议下一请求只含合法活动链；恢复零请求/零重放；来源切换与关闭工具开关仍正确 |
| M6 加固、安全与独立审查 | ce-work承接实施并保持TDD，随后按comprehensive-testing→Security Testing→ce-code-review执行，读取rules/REVIEW.md并用未参与实现的只读审查子代理 | 覆盖摘要注入、检查点伪造/破坏、原生状态、输入竞态、内存和权限状态；关键用例反证有效，修复不改冻结阈值或放宽断言 |
| M7 全量验证与文档收口 | 最终全量及受影响PTY/性能复验后执行ce-compound；同步006/007/013/015/024及表格，记录新格式和输入差异 | 规定门禁/性能阈值和验收证据齐全；报告通过/失败/忽略/未运行及精确计数/真实服务未测边界；状态最多待验收 |

M0定向基线（工作目录`/workspace`，不代替最终回归）：

```bash
cargo test --locked --test context_injection --test prompt_assembly --test tool_loop --test native_model_apis --test session --test session_cli --test session_perf --test memory_phase1 --test proc_session --test mcp_turn
```

新增`context_management`集成测试在创建后纳入根目录回归。M5补`cargo test --locked --test e2e_acceptance`并保存PTY屏幕；M7最终`cargo check && cargo test --workspace --no-fail-fast`，静态检查按工具链实际能力执行。任一阶段错误用ce-debug定位修复；副本和性能原型指定独立`--target-dir`。

### 3.3 计划前调研复核（2026-10-28）

倒序第5/6项的[汇总报告](../think-tank/2026-10-28-032-context-management.md)及完整原始输出：[多协议预算专家](../think-tank/raw/2026-10-28-032-context-management-systems.md)、[V3存储与提交回执专家](../think-tank/raw/2026-10-28-032-context-management-storage.md)、[批末竞态专家](../think-tank/raw/2026-10-28-032-context-management-security.md)、[侦察兵](../think-tank/raw/2026-10-28-032-context-management-scout.md)。

M1明确计量最终发送body；M3/M4补充失败候选隔离与取消/提交竞态的裁决和反证，保留013计费后过滤口径。§1、§2、§5不变，仍待实现/待测。门禁对齐现行AGENTS；ce-compound的复用经验统一进入`rules/project-lessons.md`。

## 4. 踩过的坑，怎样解决

实施期真实失败与修复：M0 定位全局 `TCHAT_SANDBOX` 泄漏干扰回归（[基线记录](../logs/032-context-management/2026-10-28-m0-baseline.md)）；安全攻防 8 类矩阵在覆盖范围内核心防线全部有效，仅 2 项低危观察，独立审查另确认混合批提炼泄漏与 V3 seed 内嵌图片两处 P1，均已修复（[安全报告](../security/reports/2026-10-28-032-context-management-security.md)含勘误）；独立代码审查 25 项 actionable 全部修复并附回归（[最终验证记录](../logs/032-context-management/2026-10-28-m7-full-verification.md)）。[原生接口与独立记忆版本门经验](../../rules/project-lessons.md#原生模型接口路线2026-10-24-决定2026-10-25-实现中)（仅改主会话读取器会漏掉下游）在本项以 V3 真实来源用例落实。可复用结论统一记录到[项目经验 032 节](../../rules/project-lessons.md#032-上下文管理2026-10-28-实现待验收)；方案依据见[核对记录](../logs/032-context-management/2026-10-26-spec-research.md)，此处只留引用。

## 5. 先预测，再实测，最后确认是否合格

| 编号 | 准备输入 → 执行动作 | 预测与验收标准 | 实测 |
|---|---|---|---|
| C01 | 无模型开关/无checkpoint宿主/内部记忆轮 → 查询或切换 | 条件声明与执行一致；旧路径不变，不借伪造名读取或切换 | 通过：m2_unregistered（无开关）、fix_no_checkpoint_host（无宿主腿）、sec_memory_merge（内部轮双层＋执行层断言） |
| C02 | 空/未知/非法查询参数；固定B/C/R及边界/溢出 → 计量 | 输入规则准确，ceil(B/4)与剩余公式一致；未知不填0，参数失败不改状态 | 通过：query_arguments/spec_arithmetic/reserve_mapping 单测＋m2_turn 固定字段；饱和加法（u64 极大预留）补验 |
| C03 | 五协议文本/工具/中文/JSON转义/动态Schema/当前批前缀 → 查询 | 实际共享纯编码口径；只含至当前调用前数据；零HTTP，不发送未配对前缀 | 通过：m1 五协议计量＝线上字节（mock 捕获逐字节相等）＋ct_query_snapshot（批中快照含前序结果，独立计量逐字节核对） |
| C04 | 缺模型容量/预留，图片/签名/opaque、无效限额 → 查询 | 返回明确unknown及null，不把Base64/密文或旧usage当真实token | 通过：m2_query_images_unknown＋fix_native_opaque（NativeAssistant→unknown）＋reserve 单测（互斥/零值/缺省） |
| C05 | 固定长历史与准确交接摘要 → 批末切换 → 下一请求 | 当前规则/记忆/用户输入及运行态正确，旧前缀不再发；全transcript仍完整，context_id加1 | 通过：m4_switch_happy（seed/归档/代数/声明逐项）＋e2e s32（真实 CLI/PTY 全链） |
| C06 | summary空/null/过64KiB、arguments过128KiB、未知字段/假路径/权限 → 切换 | 明确拒绝，无新窗口/无新权限；边界值按两种字节口径分别验证 | 通过：switch_arguments_shape_and_limits（64KiB 恰好/超过、128KiB 超限；恰好 128KiB 允许侧补验）＋sec_summary_privilege（权限/路径/身份字段全拒） |
| C07 | 非批末/一批多个new_context、>32或断流批；同轮第二次切换 | 非法结构整批零副作用；第二次逐项拒绝；8次请求计数不归零 | 通过：m4_batch_shape（整批零副作用）＋m4 第二次逐项拒绝＋m4_eighth（第8批封顶，变异反证3）；断流批沿既有协议失败路径 |
| C08 | 当前输入/图片/规则很大；候选不变小、估算不够或真实请求超10MiB | 拒绝或提交前失败，不截断用户/Schema、不先切再回滚，不自动调用摘要服务 | 通过：m4_not_smaller＋ct_budget_insufficient＋switch_size_verdict 单测（含旧窗口超限下界分支，#9 修复后有界反证） |
| C09 | pending异步问询、已答事件、steer入史/未入史边界及取消屏障 → 切换 | 未答阻止，输入顺序/来源不丢；取消前未提交不应用，旧窗口可辨 | 通过：m4_unanswered_async（阻止＋回答入 seed）＋ct_steer（批末入史进 seed、提交后到达按序追加）＋m4_cancel_after_scheduled（回执前取消不生效） |
| C10 | 自动保存关闭/失败、首次迁移锁冲突、检查点半写/中断 → 提交与恢复 | 专用回执失败不切换；原事实字节保留；只有完整checkpoint表示applied，scheduled不冒成功 | 通过：session 单测（/dev/full 真实设备写失败→截回失败→停用＋明确报错）＋m3_over_limit＋m4_commit_failure（未应用；#13 修复后迁移原子化，失败不动旧目标）＋变异反证2 |
| C11 | V1/V2首次切换、V3后续追加、旧程序、/save与/resume → 读取 | 版本迁移/锁/行限符合契约；旧程序拒绝V3；恢复不联网/不重放，活动视图与保存前一致 | 通过：m3_first_checkpoint（旧文件字节不变＋V3 开篇＋游标）＋m5_save_resume（视图一致＋关开关不重发）＋e2e s32 恢复段（新进程装载零重放）；旧程序拒绝由 meta 门推断（无旧二进制实测，见残留） |
| C12 | 伪造/嵌套/坏代数/坏关联/截断checkpoint及损坏原生块 → 装载 | 可显示恢复事实但阻止错误续接；不能忽略标记回退全历史或伪造签名 | 通过：#4/#24 修复后——V1/V2 走私整拒；不可解析检查点行整拒；可解析但语义坏（代数/关联非成功排定/seed 形状/seed 图片非法）分别由装载链校验/坏行/轻量校验拒绝（m3_v1_v2、ct_bad_link、fix_checkpoint_linked_to_rejected、fix_seed_assistant_images、fix_v3_seed_remote_image） |
| C13 | 五协议含原生块的合法闭合历史 → 切换并同源/换源续接 | 原始opaque留档，活动新链不携伪造旧签名/悬空结果；关闭工具开关也不重发归档前缀 | 通过：m5_native（四原生协议归档不重发）＋m5_save_resume 关开关腿；换源沿 024 for_owner 既有契约 |
| C14 | 进程/MCP/计划活跃，第8次响应切换，后续新用户轮 → 运行 | 不重启进程或MCP、真实句柄能续接，计划保留；不发第9请求，不借reset延长权限 | 通过：m4_seed_runtime_note（活进程如实列出＋句柄续接＋计划最新快照［#1 修复］）＋m4_eighth；MCP 连接腿沿快照机制（未单列用例，见残留） |
| C15 | 真实V3文件进入独立记忆读取/登记/提炼 → 处理 | 能识别V3；读取原事实、过滤检查点/seed复制/控制调用及结果，遵守原预算与来源认领 | 通过：m3_rollout（V3 可读＋纯控制批过滤）＋fix_mixed_batch（#3 修复：混合批控制调用/summary 按调用粒度剔除，真实调用配对保留） |
| C16 | 真实CLI/PTY查询/成功/失败/恢复 → 看屏幕和请求记录 | 用户看见估算/未知与已应用边界，原历史可回看；失败不画成功，摘要不变系统指令 | 通过：e2e s32（PTY 屏幕断言估算/已排定/已提交/恢复代数）＋fix_query_display_unknown（未知分支单测）；失败屏幕由 m4 未应用通知＋#5 模型可见记录覆盖 |
| C17 | M0冻结的1千/10万条原历史、小活动窗口 → 计量/追加/迁移/恢复 | 达到冻结p95/RSS；普通追加与查询不因全历史反复扫描退化；原样本与字节/落盘证据齐全 | 通过：context_perf 门禁（修复轮后复测 59.5s 绿，含 #12 装载时链校验）；#16 修复免去轮入口整视图克隆；口径边界见冻结日志 |
| C18 | 保持断言 → 旧实现/临时破坏视图选择、提交回执、请求计数或记忆版本门 → 恢复 | C05/C10/C07/C15分别发现旧前缀重发、先切后存、重置额度和遗漏下游；恢复后通过 | 通过：4/4 变异红→还原绿（docs/logs/…/2026-10-28-mutation-evidence.md） |

- C01–C18 实测结论见上表；复现命令、工作目录、退出码和脱敏原始输出统一放`docs/logs/032-context-management/`，最终验证入口为[最终验证记录](../logs/032-context-management/2026-10-28-m7-full-verification.md)，方案核对入口为[核对与文档检查记录](../logs/032-context-management/2026-10-26-spec-research.md)。
- 实施证据包括五协议固定样本与实际请求、V1/V2/V3文件/故障前后字节、记忆真实来源、PTY最终屏幕、进程/MCP请求次数、性能原样本。安全报告按安全代理规则保存并从本目录引用。
- 完成条件：M0口径/性能阈值冻结，C01–C18、规定门禁及最终全量完成，无本项未修复阻塞问题；完整历史和活动视图恢复一致，可运行材料齐全。实现方最多改待验收；精确token、真实服务成本与模型摘要保真不作已测承诺，最终验收由验收人依据规格、证据和产物判定。
- 实施记录（2026-10-28）：M0–M7 完成；独立审查（10 审查者＋验证批 8/8 confirmed）25 项发现全部修复并复验（修复说明见 `docs/logs/032-context-management/` 与 lessons）；残留未测项：旧二进制实测拒绝 V3（由 meta 门推断）、断流批/MCP 连接腿沿既有路径未单列、真实模型行为（描述契约仅静态对比）。
