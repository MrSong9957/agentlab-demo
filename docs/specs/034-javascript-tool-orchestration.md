# 规格：JavaScript 工具编排

- 状态：待实现
- 更新时间：2026-10-27
- 来源：原[工具对照表](../records/2026-10-21-codex-tools-comparison.md)「JavaScript 工具编排」一行，统一覆盖 `exec`、`wait`；两者不另拆任务。
- 依据与检查：[调研记录](../logs/034-javascript-tool-orchestration/2026-10-26-spec-research.md)。本轮仅建立规格及实施计划，没有安装运行时、执行脚本或修改程序行为。
- 033 编号已用于[已交付的工具沙箱](033-tool-sandbox.md)，本项使用 034；六项规划现已建立，开始哪一项实施仍由用户决定。
- 2026-10-27 同步：worker 与叶子接入计划按 033 实际入口及共享 dev 已部署状态更新，见[同步记录](../records/2026-10-27-sandbox-spec-sync.md)；本项仍待实现。

## 1. 要做什么，有哪些要求

### 1.1 目标、适配与实施前置

让模型用一段 JavaScript 组合已开放的工具：顺序依赖用 `await`，独立操作用 `Promise.all`，在脚本中筛选结果，通过 `text(...)` 返回需要模型继续判断的内容。`exec` 运行 JavaScript，不把源码交给 shell，也不替换 `exec_command`。

首版选择本地 QuickJS 子进程，复用 rquickjs 0.14.0；不是上游 V8 的兼容实现。保持项目 Rust 主体，不引入 Node/Deno、包安装器或整套 Agent 框架。五种协议（024 的 Chat Completions、Responses、Anthropic Messages、Gemini generateContent、Gemini Interactions）均通过普通函数调用接入；`exec` 使用 JSON 的 `code` 字段，主动适配上游 raw JavaScript＋首行 pragma，不发送 Responses 托管 `programmatic_tool_calling`/`program` 类型。

**实施前置：032 的 V3 会话、完整历史/活动视图分离及可报告保存失败的提交入口先交付，再接入本项的持久化与生产开关。** 当前032仍待实现，不能把前置能力写成已有；M0先核对。这里复用其基建，不要求开启两个上下文工具，不在本项顺带实施032。033已交付并在共享dev默认启用，替代了027 B双账号链路；031动态权限仍待实现。编排接入现有033包装，worker与其资源限制的实际组合仍须本项验证，不能把固定沙箱解释成已有动态授权。

首版只输出文本/JSON，不增加图片、音频、`store/load`、定时器、`notify`、`yield_control` 或跨脚本全局变量。`view_image` 及问询、发消息、权限确认、工具搜索、上下文切换保留直接入口；本项不编写多 Agent 规格或调度器。

### 1.2 启用、目录与调用范围

- 模型目录 `experimental_supported_tools` 必须同时显式含精确名称 `exec`、`wait`，普通用户轮才开放这一对工具；只有一个时两者均关闭并给出一次配置提示。未配置的请求、工具集和串行行为保持原状；内部记忆轮始终不声明、不执行两工具。
- 运行时初始化失败、子进程启动失败或已配置隔离不可用，明确返回失败，不回退到主进程 eval、shell、外部 Node 或无隔离启动。未启用编排时不创建运行时子进程。
- 没有可用的V3提交宿主（包括自动保存已停用/失败）时exec失败；cell_started未保存成功前不启动worker、不执行叶子。纯内存运行不冒充可恢复的编排。
- 可嵌套调用范围是当前请求已经声明且允许执行的 `exec_command`、`write_stdin`、`apply_patch`、`update_plan`、023资源工具及022动态MCP工具的交集。未开放的资源接口、服务器或工具不因编排可用而开放。
- `exec`、`wait`、`mem_*`、018问询/等待、021发消息、030搜索、031权限、032上下文工具均不进入嵌套目录；伪造这些名称不能触发任何宿主动作。业务MCP工具仍按既有策略执行，不凭名称或描述猜它是否只读、是否获批。
- 脚本得到只含该交集的 `tools` 和 `ALL_TOOLS`。后者条目为 `{name, description, parameters}`，参数Schema原样保留。工具键采用原始模型工具名，推荐 `tools[entry.name](args)`；不额外净化成JS标识符，不按 `__` 拆名路由。对象无原型、只读，目录数据深拷贝，不含连接、令牌、权限对象或可变宿主引用。
- 若030已实施，脚本只能访问产生 `exec` 响应的请求快照中已声明的动态工具。先顶层搜索、下一次模型请求再启动脚本；正在运行的cell不扩展目录。恢复出的Schema、同轮后续搜索结果或猜测名称都不能给旧cell增加调用资格。

配置示例（为模型条目的相关片段，不是新增配置文件）：

```json
{"experimental_supported_tools":["exec","wait"]}
```

### 1.3 exec：输入、JavaScript 与输出

输入是无额外字段的JSON对象：

| 字段 | 规则 |
|---|---|
| `code` | 必填，非全空白字符串，最多64 KiB UTF-8；原样保存，编译为异步模块，支持顶层await |
| `yield_time_ms` | 可选整数，缺省10000，范围0–60000；0表示不等待，不是无超时 |
| `max_output_tokens` | 可选整数，缺省10000，范围1–16000；按本地UTF-8字节预算换算，不是模型分词器计数 |

不接受裸源码、Markdown围栏、文件路径代替code、首行配置pragma或运行时/权限覆盖字段；pragma前缀显式报参数错误，避免把错误配置当普通注释忽略。其他语法错误由引擎返回，零叶子调用。

- 每次有效 `exec` 创建一个独立cell和新的JS运行时；不同cell不共享JS变量。当前逻辑用户轮最多保留一个未取走终态的cell，包括已完成但尚未取结果的cell；有旧cell时新exec返回 `CELL_BUSY`，不淘汰旧cell。
- 全局仅提供标准JS计算能力、`tools`、`ALL_TOOLS` 和 `text(value)`；没有Node、console、文件/网络/进程API、`std/os`模块、动态库加载器或模块文件搜索。`std` Cargo feature只支持Rust标准库，不向JS开放同名系统模块。字符串eval也不能获得新的宿主能力。
- 所有 `tools[name](args)` 只接受JSON对象，调用既有参数解析器；成功桥接返回既有结果JSON解析出的对象。业务失败仍保留原有 `status/error/isError` 等字段，不因Promise兑现改成成功。调用资格、序列化、容量或桥接失败则reject，错误含 `code/message/execution`；`execution` 为 `not_started`、`completed` 或 `unknown`，不诱导自动重试未知副作用。
- `text` 对字符串原样追加一条文本；其他可JSON序列化值按JSON追加。undefined、BigInt、循环引用或序列化异常拒绝；输出按调用顺序保存，不因没有 `text` 自动打印返回值。脚本内捕获异常继续执行的语义遵循JS，但不能捕获并解除宿主关闭、限额或权限撤销。
- 顶层Promise兑现且已提交的叶子调用全部结算才可成功完成。若顶层已结束仍有排队/在途调用，标记 `UNAWAITED_TOOLS` 并收尾；不静默丢弃已经发生的调用，不自动替模型等待遗忘的Promise。未处理拒绝或脚本抛错同样关闭cell、保留真实证据。

exec与wait的正常结果固定为下列字段：

```json
{"ok":true,"status":"running","cell_id":"opaque-cell-id","output":["已得到第一部分结果"],"output_truncated":false,"error":null}
```

`status` 取 `running/completed/terminated/failed`；仅前两者 `ok=true`。参数/无效句柄失败为 `failed`，创建cell前的 `cell_id=null`；其他结果带真实句柄。`error` 非空时是 `{code,message}`，不把业务叶子失败与脚本失败混淆。每次观察只返回尚未交付的输出，终态输出也遵守预算。

### 1.4 wait、让出与终止

`wait` 输入为 `{cell_id, yield_time_ms?, max_tokens?, terminate?}`，禁止额外字段。cell_id为必填非空字符串，最多128 UTF-8字节且原样精确匹配；时间范围同exec、缺省10000；max_tokens范围同exec、缺省10000；terminate须为布尔，缺省false。null、浮点、负数或溢出均失败，不将整数强制转换。

- 只能等待同一运行实例、同一逻辑用户轮已由exec返回的cell；其他实例/用户轮/已消费句柄统一 `CELL_NOT_FOUND`，不泄露另一会话是否存在。无效参数不消费任何输出或终态。
- 到达yield时间，返回截至该观察点的新输出与 `running`；yield是观察期限，不是脚本寿命，不重置CPU、墙钟、调用或输出额度。重复wait只返回增量，终态被交付后释放句柄，再等待失败。
- **本地调度适配：让出后JS可继续计算、已在途工具可完成，但新叶子请求仅留在有界队列；下一次wait驱动宿主持久化和继续调度。** 结果先由宿主保存，再交还JS。不在等待下一次模型响应期间背后接受新的业务动作；不同于上游可持续后台分派的路径，不宣称完全等价。
- terminate=true忽略等待时间，原子关闭新调用受理，取消排队和在途等待、终止并回收JS子进程，收取真实已发生结果后返回terminated；若完成先于终止且已提交，返回completed。不能以取消等待为名重开cell或清空预算。
- 终止脚本不撤销文件写入、MCP服务端已受理动作，也不擅自杀016管理器中的命令进程。内层exec_command取消沿016返回真实进程句柄；已有进程可在cell关闭后由直接write_stdin续接，仍受既有寿命和权限限制。MCP取消后无法确定副作用的，明确unknown，不重放。
- 本轮取消、模型/协议失败、8次请求耗尽、无wait就返回终文、宿主退出都关闭cell并记录收尾。未取终态时的模型终文不得当任务成功；返回“尚有编排结果未取回”的轮错误。下一用户轮、/resume和应用重启不恢复JS堆或活cell。

### 1.5 批次、额度与并发

- 保留每逻辑用户轮8次模型请求、普通批次32条调用上限；本项不增加模型请求。含exec或wait的assistant批次必须恰好只有该一条控制调用；先做完整协议/顺序校验，违规整批零执行，不先执行其他动作。活动cell未消费时仅wait可以推进执行；单条exec按CELL_BUSY失败并保留旧cell，其他批次整批拒绝并收尾。
- 每个cell使用启动exec所在批次的32份调用额度，其中exec占1份，最多再受理31个叶子调用；wait不能补充旧cell额度。另新增每逻辑用户轮最多128次叶子调用；请求数、context_id变化和错误重试都不能重置这些计数。父进程收到格式可识别的叶子请求先原子扣额，再检查资格和参数；错误参数、资格拒绝和业务失败也消耗该名额，坏IPC帧则直接关闭cell。
- 最多4个叶子在途，其余按提交顺序进入队列；总队列受31次cell额度约束，不能先分配无限Promise/线程再检查。`apply_patch/update_plan/write_stdin`采用同一串行通道；其他合格叶子可并发。脚本负责仅对互不依赖的工作用Promise.all；不承诺MCP写操作或命令具备事务/自动冲突检测。
- 调用序号/证据ID按受理顺序分配，实际完成顺序可不同，Promise.all的返回顺序遵循JS。证据保留两个顺序；内层ID由宿主生成并关联外层原始call_id，不占用或伪造提供方call_id。外层id/name继续原样保留。
- 新请求/权限检查与关闭使用同一父进程受理门；关闭后拒绝新动作，排队项零执行，迟到返回不重新推进JS。既有执行路径继续检查取消、MCP身份/凭证污染/参数与进程容量。若031实施，其逻辑轮授权/到期不被cell延长；其特殊权限进程仍按031回收。
- 018已答输入和021接续输入仍只在完整外层批末入史；不自动注入JS或重跑脚本。模型收到改向输入后可先用wait终止旧cell，再决定新的动作。取消按钮直接关闭受理门，不等待下一次模型请求。

### 1.6 运行时、历史与展示

子进程采用可信当前可执行文件的隐藏 `--internal-js` 入口，在dotenv/模型目录/记忆初始化之前分流。只通过有界IPC接收源码/目录和发送请求/输出；始终env_clear、关闭无关fd、独立进程组。启用033时，由宿主调用现有 `sandbox::tool_launch_argv` 包装worker，策略锚定稳定的主进程工作区，不由JS或模型workdir重定；包装失败直接报错。关闭033时保留本节的worker语言/IPC/资源限制，不能因包装失败自动切换模式。

033使用相同宿主UID、独立PID命名空间与新procfs，取消和回收沿宿主进程组处理，不恢复旧跨UID补杀。worker的RLIMIT、IPC、超时与bwrap组合在M0实测；仅凭已有033命令测试不能宣称JS运行时已验证。JS不提供文件/网络API是语言能力限制；033当前网络保留，不能据此宣称内核已断网。

父进程是唯一能力与执行事实来源：逐次核对冻结目录、参数、预算、取消和原始MCP映射，复用既有工具执行函数；命令、MCP stdio、补丁继续经过033各自的真实沙箱入口，不在worker内直接执行业务。不信任子进程自报“已获批/已执行/新工具”。若031以后交付，策略也由宿主权限模块提交，JS不得提供自己的profile。JS子进程不持有模型/MCP认证、AutoSession或主历史引用。主轮驱动保存/通知，工作线程经有界事件队列回送，不并发改Vec/history或持锁等待网络。

新增V3 `HistoryItem::OrchestrationEvent`：`cell_started/call_started/call_finished/cell_closed`，统一携带cell_id、外层exec_call_id、宿主递增序号；调用事件另带宿主叶子ID和精确工具名。start保存实际参数；finish保存真实结果、ToolDisplay、执行状态；大结果按下节显式记录摘要/截断与哈希。只允许宿主构造，不把工具返回JSON反解析成事件。

- 第一次cell开始前，复用032的迁移/锁/提交机制写入V3完整历史和cell_started；call_started保存成功才执行叶子，call_finished保存成功才向JS兑现结果。保存失败关闭cell并明确报告已发生或未发生；不能假装副作用已回滚。进程崩溃只留下start时，恢复显示结果未知，不自动重发。
- 事件不能插成提供方的assistant tool_calls/tool_results。所有模型投影在原生批次配对之前过滤事件，含messages、_history和五协议；模型只看到外层exec/wait的调用和汇总结果。032选最近检查点的扫描须忽略事件，活动窗口切换前必须消费/关闭cell，不能将运行句柄写进seed当权限。
- 展示/原始历史和/save保留事件；实时与/resume使用同一投影，按cell归组展示源码、叶子参数、真实结果/差异、运行/让出/终止/未知状态。内层命令、补丁和成功update_plan复用各自真实投影，由事件建立关联，不把脚本中的计划文字当作成功更新。终端控制字符只过滤展示副本；不把源码或输出直接当ANSI执行。孤立的start不得显示完成，运行中的脚本恢复后显示已失联，不能显示仍可wait。
- V3读取器扩展严格校验事件次序/关联/容量。旧程序拒绝V3；仅支持032而不认识编排事件的V3程序也必须禁止错误续接。/resume不执行JS、叶子、MCP重新发现或恢复授权。没有编排事件/上下文检查点的会话仍走原V1/V2兼容路径。
- `memory/rollout_input.rs`按原有预算提取真实叶子事实和来源，过滤exec/wait外壳及重复汇总，不能把脚本源码当成已经执行的工具。大记录按现有预算降级并如实记录缺口；不放宽记忆限额、不把审计事件或摘要当新用户指令。

### 1.7 资源、测试条件与未测事项

以下为建议冻结的新增产品边界，不是实测性能值；不新增用户配置项：

| 对象 | 上限/处理 |
|---|---|
| code / 原始exec arguments | 64 KiB / 512 KiB UTF-8；计JSON转义后的参数预算，超限启动前拒绝 |
| 目录/初始化IPC | 完整序列化≤10 MiB；不裁剪Schema；其他单IPC帧≤1 MiB，有界读取后解析 |
| 叶子参数 / 可送JS的叶子结果 | 参数64 KiB，既有完整结果JSON256 KiB；结果过大时拒绝桥接并报告execution，禁止把截断JSON当成功对象 |
| 超大叶子结果证据 | 保留完整实际参数；结果保留最多64 KiB UTF-8片段、原始字节数、SHA-256和truncated标记，明确原文未完整保存；正常结果原样保留；单事件仍≤007的64 MiB行限 |
| 输出 | 待交付缓冲≤64 KiB；每次观察最多4×max_tokens字节（exec使用max_output_tokens），边界按UTF-8；缓冲或观察超额丢弃多余输出并标output_truncated，不留无限积压；cell累计生成输出（含丢弃部分）≤1 MiB，超过关闭失败，不能靠频繁wait绕过 |
| 活动/调用 | 同轮1个未消费cell；31叶子/cell、128叶子/轮、4在途；关闭后原子拒绝新增 |
| JS堆/栈/进程 | QuickJS堆64 MiB、JS栈512 KiB；Linux worker地址空间512 MiB（不是RSS承诺）；超限失败不影响主进程继续处理下一轮 |
| CPU / 墙钟 | worker累计CPU 5秒，创建成功起墙钟120秒（含让出/模型等待）；中断回调＋父监控/进程限制，不因wait重置；终止整组并wait回收，不把终止脚本等同于杀业务进程 |
| 模型/会话 | 保持完整请求10 MiB、单行64 MiB与8请求/32普通批次；事件不塞进模型请求，不自动删原始历史 |

容量计算使用有界序列化；结果过大、OOM、崩溃、IPC断开都关闭调用门，已执行但拿不到结果时为unknown，不自动重试。rquickjs的内存限制不得使用会令该API失效的自定义/Rust allocator配置，M0/M1检查实际feature解析及分配失败行为。

测试在Linux、锁定依赖、独立临时HOME/配置/缓存/会话/工作区、固定locale/时钟/随机种子下进行；外部模型/MCP用离线固定响应边界，JS引擎、进程、桥接、落盘和投影用真实实现。时间边界低层用可替换时钟/屏障，真实进程用有界看门狗；不改真实用户环境或凭证。

速度、RSS、构建增量和费用均未测。M0先用固定版本最小原型冻结测法和p95/RSS阈值（同机基线留2–3倍余量），未完成不得进入验收：单并发，预热5次，正式100次、独立3轮取p95中位轮并保存全部样本；分别测冷启动纯JS、1/4/31叶子、串行/4并发、yield→wait、取消回收。1千/10万历史测事件追加，不能每次全扫描归档；RSS分别记主进程、worker、二者合计。并发有效性以屏障证明4个独立调用实际重叠，不靠模糊延迟断言；等待开销与真实外部工具耗时分开。

上下文效果使用4个固定32 KiB文本结果、最终仅输出总数的同一任务，对照直接调用与脚本两路五协议真实请求：计入exec源码、目录说明、外层结果和所有新增历史后，脚本路最终请求须更小；只记字节，不冒称精确token/缓存/费用收益。编排额外模型请求数为0；模型/MCP实际调用费用未测，不填0元。

## 2. 哪些情况容易出错

| 情况 | 期望结果 |
|---|---|
| 未开关、只开一个、内部记忆轮或启动/隔离失败 | 不创建隐式执行入口；无回退；不执行伪造工具 |
| 源码空白/围栏/pragma、非法JSON、null/浮点时间、未知字段、中文/CRLF | 精确校验，合法原文保存；非法输入不创建cell、不消费输出 |
| 同批exec＋其他工具、先搜索同批exec、活动cell时普通动作 | 先整批拒绝，零该批副作用；既有cell明确收尾 |
| 重复exec、错误/旧轮/已消费cell_id、反复wait或重复终止 | 不偷取其他cell；增量不重复；不增加调用或时间预算 |
| 未声明/未加载/禁用工具、伪造名称/路由/批准JSON | 父进程拒绝，零业务请求；原始MCP身份不靠字符串拆分 |
| Promise.all、同句柄写入、补丁/计划并发、调用31/32或轮128/129边界 | 4在途和串行通道有效；额度原子扣除，超限零新增；失败不回滚既有副作用 |
| 顶层抛错/未await、业务失败被catch、无text、超大结果 | 不伪造成功；真实失败字段保留；输出/桥接截断明确，未知操作不重试 |
| 无限循环/递归/分配、海量输出/Promise、IPC坏帧或worker崩溃 | 有界失败、取消可达、worker回收；主进程和下一轮仍可工作 |
| yield后模型等待、模型终文/失败/第8次响应、cancel/退出 | 新叶子只排队；所有出口关闭门；没有第9次请求或跨轮活cell |
| 终止时命令已启动或MCP服务端已受理，晚到返回 | 命令沿016保留真实句柄；不声称回滚；迟到结果不触发下一动作 |
| 保存前后故障、崩溃只留start、坏V3事件、重复事件 | 未落start不执行；结果未存不兑现；恢复显式未知/禁止错误续接，不重放 |
| /save、/resume、032切换、记忆提取和原生签名混合 | 全历史保留，模型投影不夹伪调用，旧能力不恢复，来源与事实不重复 |
| 中文/emoji截断边界、ANSI/OSC52/控制字符、Schema恶意说明 | 模型原文/身份保持；展示安全；目录数据不成为更高权限指令 |

## 3. 重要做法，为什么这样做

### 3.1 复用依据与取舍

- 固定上游 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4` 的[exec声明](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/execute_spec.rs)、[wait声明](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/wait_spec.rs)与[运行时说明/解析](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode-protocol/src/description.rs)用于确定组合、增量观察和工具代理职责；[delegate](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/code_mode/delegate.rs)保留cell创建时的step并走宿主分派，不是直接在JS里执行Rust业务。
- 上游[code-mode crate](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode/src/lib.rs)与[进程provider](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/code-mode/src/remote_session.rs)依赖外部host，不把移植两个handler当成已经获得V8执行器。这里仅复用接口思路，采用成熟可嵌入引擎，不自写JS解释器。
- rquickjs [v0.14.0清单](https://github.com/DelSkayn/rquickjs/blob/d7ef5eeae702fea24c03643064de454f1c1dd4b0/Cargo.toml)为MIT、Rust最低1.87；拟用 `=0.14.0`、关闭默认feature，只启用 `std,futures`，新增传递依赖写Cargo.lock。不开 `full/loader/dyn-load/rust-alloc/allocator`；[AsyncRuntime文档](https://docs.rs/rquickjs/0.14.0/rquickjs/struct.AsyncRuntime.html)明确中断、栈/内存接口及自定义allocator时内存限制失效，不能只看API存在就宣称配额生效。
- [OpenAI Docs](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)介绍托管V8与宿主工具往返；其Responses类型、allowed_callers和加载时机不直接套到本项目。本项选择普通函数＋本地执行，主动缩小辅助函数/后台调度范围，费用、缓存及V8兼容性均无等价承诺。
- 复用016的进程/取消、022/023的真实路由/污染检查、024的普通函数适配及033的统一沙箱；030请求执行集合与032的V3提交按其实际交付状态接入。027保留的环境清理、fd等独立修复继续有效。先抽最小共享叶子执行入口，不重写各工具，不搬运上游扩展/遥测/多Agent框架。

### 3.2 根据规格制定的实施计划

M0–M7均属于原表「JavaScript 工具编排」这一项；当前全部待实施。

| 步骤 | 修改落点与工作 | 验证与退出条件 |
|---|---|---|
| M0 核对前置与最小原型 | 核对工作区、032 V3交付、033实际入口和共享dev部署；记录普通/沙箱测试基线。隔离原型验证固定rquickjs的顶层await、Promise桥接、中断/堆/栈限制，以及worker经真实bwrap启动时的IPC、RLIMIT、宿主进程组回收和最小features；冻结§1.7性能夹具和阈值 | 前置未交付则不启用生产功能、不顺带实施其他项；原型/锁定依赖/构建代价及Linux资源限制可复现，不用033已有验收代替worker验证 |
| M1 参数与worker | 新增 `src/app/src/js_orchestration/`（args/runtime/protocol），lib.rs导出；main.rs在配置加载前接隐藏worker入口，宿主按033统一入口包装；Cargo.toml/Cargo.lock锁依赖；先写非法输入红例，再接真实引擎与有界IPC | 加入 `tests/js_orchestration.rs`，纯JS、对象参数、业务失败、无IO/模块、OOM/循环/坏帧/假凭证env/fd哨兵均经真实进程验证；普通与bwrap两模式、包装失败均覆盖，引擎错误不杀主程序 |
| M2 共用分派与调用门 | 从turn.rs现有执行分支抽最小叶子分派给直接/嵌套共用，保留命令/MCPstdio/补丁的033包装；复用ProcessManager/McpSnapshot，新增有界broker、序号、额度、4并发及串行通道 | 先证明直接调用行为未变；真实CLI/MCP叶子与沙箱门参与，伪造/未加载/跨模式零执行；屏障证明重叠和关闭后零新增 |
| M3 V3证据与提交 | 基于032扩展prompt.rs的HistoryItem事件、session.rs验证/锁/保存回执；主轮消费事件，start先存后调度、finish先存后兑现；更新模型投影、api/native适配和memory/rollout_input.rs | 事件不进入模型原生调用链，全部读取器一致；注入磁盘失败/截断/孤立start，恢复不重放/不误报成功；增量追加不反复全扫 |
| M4 exec/wait生命周期 | turn.rs接独占批规则、cell状态机、请求级工具快照、yield/增量wait/终止/全部轮出口；对接已有问答与steer批末边界 | 覆盖running→终态消费、完成/取消竞态、yield后新调用仅排队、未await/额度/过期/第8次请求；进程句柄与真实MCP未知副作用语义保持 |
| M5 声明、展示与五协议 | catalog.rs/prompt.rs按双opt-in定义工具，main.rs/tui_host.rs传能力与提交宿主；src/tui聊天投影复用通用工具cell并按编排关联展示；扩充prompt_assembly/native_model_apis/session/session_cli/e2e_acceptance | 五协议本地模型服务走真实JS→叶子→外层结果；真实CLI/PTY运行和恢复一致、ANSI安全；无开关兼容，完整请求字节对照及p95/RSS有原始样本 |
| M6 加固与独立审查 | TDD后按项目顺序执行comprehensive-testing、Security Testing、code-review；安全按既有Security-Testing-Gate，审查读取rules/REVIEW.md，独立子代理只读找问题 | 越权/凭据/IPC/同UID命名空间/策略锚点/宿主进程组取消/资源/重放重点覆盖；关键用例用修复前后或临时破坏证明会失败；发现错误用diagnosing-bugs定位并复验 |
| M7 全量验证与交付 | 修复后完成全量命令，更新受影响的016/022/024/032接入说明、对照表及项目经验；证据统一保存在本项logs | 报告通过/失败/忽略/未运行数量，适用CI有则通过；功能、性能、资源与证据齐全后最多标待验收，最终状态由验收人决定 |

M0已有测试基线命令（工作目录 `/workspace`，只作基线，不替代交付回归）：

```bash
env -u TCHAT_SANDBOX cargo test --locked --test tool_loop --test prompt_assembly --test proc_session --test exec_sandbox --test apply_patch_sandbox --test mcp_stdio_sandbox --test mcp_turn --test mcp_resources --test native_model_apis --test session --test session_cli --test session_perf
```

实现轮另跑普通模式真实流程 `env -u TCHAT_SANDBOX cargo test --test e2e_acceptance`，最终普通回归执行 `env -u TCHAT_SANDBOX cargo check && env -u TCHAT_SANDBOX cargo test --workspace --no-fail-fast`。共享dev默认注入开关，普通基线须清除；专用沙箱测试在隔离进程内自控开关，另须验证真实bwrap下worker及嵌套叶子的新行为，不能以不可用分支替代。在隔离副本中必须显式 `--target-dir`，避免污染共享构建目录；不为本次纯文档工作运行这些命令。rquickjs新增C依赖须按实际工具链验证，不把Rust typecheck当引擎安全测试。

## 4. 踩过的坑，怎样解决

本项尚未实现，没有脚本运行、性能或取消实测结论。真实核对发现：现有工具循环/函数适配不能替代JS运行时，上游code-mode crate也是host客户端；引用依据见[调研记录](../logs/034-javascript-tool-orchestration/2026-10-26-spec-research.md)。

可复用规划判断统一放在[项目经验第7节](../../rules/project-lessons.md#7-实现取舍与问题根因)，不把预计风险写成已发生故障。033早期环境探针保留历史语境，当前以[真实宿主验收](../logs/033-tool-sandbox/2026-10-27-host-acceptance.md)和[共享dev部署](../logs/033-tool-sandbox/2026-10-27-dev-deploy.md)为准；它们不能作为本项JS/IPC/资源限制已验证的证据。

## 5. 先预测，再实测，最后确认是否合格

| 编号 | 动作与预计结果 | 实测/验收 |
|---|---|---|
| C01 | 双opt-in开/关/缺一、内部记忆、运行时失败：只在完整条件下开放，禁用路径零worker/零副作用 | 待测 |
| C02 | 输入类型/额外字段/围栏/pragma/空白/长度/中文CRLF边界：严格校验，合法源码原样，失败不启动 | 待测 |
| C03 | 真实顺序JS读取叶子结果后决定下一调用：参数来自实际结果，最终仅显式text进入模型 | 待测 |
| C04 | 四个独立叶子Promise.all屏障：确实重叠且≤4在途，结果顺序正确；串行通道不重叠 | 待测 |
| C05 | 原参数/状态/isError、脚本catch/未处理拒绝/未await/无text：业务状态不变，不伪报成功 | 待测 |
| C06 | 循环、递归、OOM、坏IPC、输出洪泛及结果容量：限额有效、明确截断/unknown，主进程可继续 | 待测 |
| C07 | yield→多次wait→完成/失败/终止：增量恰一次；已在途可完、新叶子待wait；终态消费后无句柄 | 待测 |
| C08 | 完成/终止/晚到结果竞态：关闭后零新动作，不重复兑现，不声称回滚真实副作用 | 待测 |
| C09 | 批次31/32叶子、轮128/129、控制混批、31队列/4在途边界：原子限额，wait和错误不补充额度 | 待测 |
| C10 | 未加载/禁用/未知名、伪造原始身份/授权、跨实例/轮旧句柄：零越权调用，不泄露其他cell | 待测 |
| C11 | 033模式worker假环境/fd哨兵、语言无fs/net/modules、假.env遮蔽、改变workdir及包装失败：策略锚点不变、无弱回退；真实bwrap/IPC/限制/回收组合有效，不把语言无网络API当内核断网 | 待测 |
| C12 | 第8次响应、模型失败/终文、cancel、退出、接续输入：完整记录和关闭，无第9次请求/跨轮JS | 待测 |
| C13 | 终止时exec进程仍存在、MCP已受理：真实句柄可续接，未知结果不重放，031到期不延长 | 待测 |
| C14 | start/finish提交失败、崩溃/坏V3/重复序号：未存不执行或兑现，恢复显示未知并阻止错误续接 | 待测 |
| C15 | 五协议＋原生签名＋保存/恢复/032基建：外层正确配对，内层事件不混入，无执行/能力复活 | 待测 |
| C16 | 记忆/实时CLI/TUI/PTY/resume：真实叶子事实可追溯且不重复；源码输出控制字符安全 | 待测 |
| C17 | 固定32 KiB×4任务比较：计齐所有历史后脚本最终完整请求更小，额外模型请求为0 | 待测 |
| C18 | §1.7冻结性能/RSS/大历史追加、资源与取消用例，关键反证及全量验证：满足冻结要求且证据齐全 | 待测 |

证据写入 `docs/logs/034-javascript-tool-orchestration/`，含命令、工作目录、退出码、脱敏原始输出及必要PTY渲染结果；本项文档检查不能代替这些运行结果。验收人按冻结规格和可运行产物独立核验，不读实现源码；实现方无权自行标已通过或为过关放宽限额、删除用例。
