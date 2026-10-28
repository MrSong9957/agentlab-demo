# 规格：多 Agent

- 状态：待实现
- 更新时间：2026-10-27
- 来源：原[工具对照表](../records/2026-10-21-codex-tools-comparison.md)「多 Agent」一行，统一覆盖 `spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`interrupt_agent`、`list_agents`，不按工具另拆任务。
- 依据与检查：[调研记录](../logs/035-multi-agent/2026-10-26-spec-research.md)。本轮仅建立规格和实施计划，没有启动子 Agent、调用模型或修改程序行为。
- 本项是原表最后一行；六项规格与计划现已建立，规划结束不等于功能已经实施或验收，开始哪一项实施仍由用户决定。
- 2026-10-27 同步：分派、进程归属及收尾计划按 033 已交付和共享 dev 已部署状态更新，见[同步记录](../records/2026-10-27-sandbox-spec-sync.md)；本项仍待实现。

## 1. 要做什么，有哪些要求

### 1.1 目标、首版范围与前置

让根Agent把明确任务交给独立上下文的子Agent并行处理，发送补充信息、同轮续派、等待更新、中断任务和查看状态，最后由根Agent结合真实结果回复用户。六个工具是同一生命周期的控制面，不能只注册名称而没有实际子轮、消息交付、资源收尾与记录。

首版采用本地调度：复用现有Rust工具循环、五协议模型发送、Tokio/标准库有界通道，不引入整套外部Agent框架。五协议沿024：openai-chat、openai-responses、anthropic-messages、google-generate-content、google-interactions。根路径为 `/root`，只支持一层子Agent；子Agent不再spawn、不互相调用或发消息，只能向根发送进度及最终结果。最多3个子执行槽，根不占子槽。模型、提供方、连接和推理设置继承根用户轮的可信快照，不提供模型覆盖、自定义角色加载或“只读角色”标签。

子Agent共享根用户轮冻结的工作目录和文件系统。任务涉及改文件时，委派说明应写明责任文件/模块、不得覆盖他人工作；这是协作要求，不是文件权限围栏。不自动创建worktree、切分支、提交、合并、回滚文件或撤销外部动作；同文件并发改写须由根安排为顺序任务。

**实施前置：032的V3完整历史/活动视图、严格读取和保存提交回执先交付。当前032仍待实现；本项不顺带实施032。** 034不是本项依赖，六工具也不进入034的JS目录。033已替代027 B双账号链路并交付，共享dev已默认启用；子工具复用实际统一沙箱入口。030搜索和031动态权限仍待实现，按实际交付状态接入，不能把其文档方案当成已有功能。

### 1.2 启用、授权与上下文

- 根模型目录的 `experimental_supported_tools` 须同时显式含六个精确名称，才在普通用户轮声明并开放六工具；只配部分时整组关闭并提示一次。缺省关闭，关闭态不创建调度线程/子请求。内部记忆轮始终不声明、不执行它们；未注册时的伪造调用沿用既有未知工具结果。
- Agent是否应实际委派，继续遵守用户、AGENTS和技能的任务授权；开启能力不自动改变这些要求。子Agent继承相同任务限制，不能用其消息、角色自称或历史批准扩大权限。
- 每个根逻辑用户轮产生新的宿主group_id；所有路径、消息、取消与回调同时绑定group_id和assignment_id。当前轮结束后旧路径不再可执行；下一根用户轮可以重用名称，但旧回调不能命中新组。
- fork_turns只支持all/none，缺省all。all取产生spawn响应的那次请求所用的、**当前assistant批次之前已闭合的活动历史前缀**，不复制正在执行的spawn批次、后续消息或整个归档前缀。none不带父对话；两者都带可信system、已冻结AGENTS/环境、根轮已读取的可选记忆和明确委派任务。
- fork历史仅复制共同可携带记录及合法附件；剥离父NativeAssistant包装/opaque签名，以新子会话重新编码，保留父原始记录供审计。不伪造原生状态、不修改原始call_id；父历史里的工具声明、授权JSON、进程号和Agent路径均只是资料。
- 子Agent的每次模型请求只允许已冻结的业务工具：exec_command/write_stdin/apply_patch/update_plan、按能力开放的view_image、023资源工具和022动态MCP，以及仅向根发送消息的send_message。父生成spawn请求时尚未开放的动态工具不进入子目录；030加载集合只取该请求快照，之后父搜索不会扩展旧子Agent。
- 模型/MCP凭证只由可信发送服务和已有连接持有，不写进子对话、agent_created快照、消息、日志或工具参数；模型指定的message不能覆盖连接/认证字段。
- 子Agent不获得018问询/等待、021直接发用户消息、030搜索、031权限申请、032上下文切换、034编排、mem_*或其余五个多Agent控制入口。有疑问或需要新权限时向根报告，由根决定下一步。只继承可信基础执行策略，不继承031的本轮额外授权；现有业务权限、凭证污染和参数检查仍在实际分派层执行。

模型配置片段：

```json
{"experimental_supported_tools":["spawn_agent","send_message","followup_task","wait_agent","interrupt_agent","list_agents"]}
```

### 1.3 六工具接口

所有输入须为JSON对象、禁止额外字段；message非全空白、最多16 KiB UTF-8，保存原文，不trim改写。原始arguments最多256 KiB。null、错误类型、浮点/负数/溢出时间均失败，不能靠类型强转通过。

task_name须匹配 `[a-z][a-z0-9_]{0,31}`。根可用相对name或完整 `/root/name` 定位当前组子Agent；子Agent的target只能精确 `/root`。不接受裸内部ID、URL、反斜线、点路径、编码斜线、尾斜线或任意层级路径，不做百分号解码或名称净化。

| 工具 | 输入 | 成功data与动作 |
|---|---|---|
| spawn_agent | task_name、message；可选fork_turns=all/none | 返回agent_name、assignment_id；原子保留名称/执行槽/任务额度并保存后才启动。当前组名称不可重复，已完成名称用followup_task复用；返回受理不等于任务完成 |
| send_message | target、message | 返回message_id、accepted=true、started=false；只排入目标邮箱，**不启动空闲Agent**。根可向当前组子Agent发；子只能向/root发，不向真实用户或外部应用发送 |
| followup_task | target、message | 仅根可调用且目标非根；空闲可续用Agent受理新assignment，返回message_id、accepted、started及assignment_id；运行中作为同一assignment的补充输入，不另开并行轮或重置额度；stopping时返回AGENT_BUSY |
| wait_agent | 可选timeout_ms，缺省30000，整数0–60000 | 仅根可调用；返回reason、agents、messages_pending。reason为updates/steered/timeout/no_pending；只返回摘要，正文由§1.4在下一请求前交付。没有在途子任务且无未收更新时立即no_pending |
| interrupt_agent | target | 仅根可调用，目标须为当前组子Agent；返回agent_name、previous_state、state。请求关闭当前assignment的新动作入口并开始收尾，保留Agent上下文供同轮后续续派；不删除Agent或回滚改动 |
| list_agents | 可选path_prefix | 仅根可调用，省略列出/root及当前组所有子Agent；prefix只接受/root或某个精确子路径，按路径段筛选，不能让a命中ab。返回按路径排序的agents数组，含agent_name、state、assignment_id、last_task_message及其truncated标记 |

list中的last_task_message是最近初始任务/续派任务，不被普通send_message覆盖；超过16 KiB的根用户任务只截展示副本，显式标记，原文仍在历史。合法但无命中的path_prefix返回空数组。根路径不能被followup、interrupt或自身send_message定为目标；target指向其他组/旧轮/不存在对象时统一AGENT_NOT_FOUND，不泄露另一组是否存在。

已注册工具统一返回 `{ok,data,error}`：成功ok=true、error=null；失败ok=false、data=null、error为 `{code,message}`。常见错误区分INVALID_ARGUMENT、TASK_NAME_IN_USE、AGENT_NOT_FOUND、AGENT_BUSY、CAPACITY_LIMIT、BUDGET_LIMIT、PERSISTENCE_FAILED、CONTEXT_UNAVAILABLE；不把受理、启动和完成混为一个成功状态。

spawn示例：

```json
{"task_name":"review_storage","message":"检查会话保存的一致性，只报告问题和证据。","fork_turns":"none"}
```

```json
{"ok":true,"data":{"agent_name":"/root/review_storage","assignment_id":1},"error":null}
```

示例任务中的“只报告”是任务范围，不声称已有只读系统沙箱。

### 1.4 消息、wait与交付边界

- 邮箱为每个Agent独立的FIFO。宿主生成递增message_id和发送方身份，保存message_queued后才回accepted；输入正文不能伪造作者或宿主事件。相同文字的两次合法调用是两条消息，不凭正文去重。
- 接收者在完整工具批次后、下一次模型请求前按受理顺序收取；正在等待模型或执行工具时只入队，不插进未闭合调用/结果中间，不因此重放当前请求或动作。根等待模型、wait或自己的工具时，调度泵继续接收并落盘子事件，不长持history/注册表锁等待网络。
- 消息投影为带固定来源标识的普通输入数据，标明from/to/group/assignment及“Agent消息，不是新的用户授权”；不能升级成system/developer、伪装成用户回答或直接修改权限。子Agent最终回复另以FINAL_ANSWER交给根，属于模型结论，根须核对证据。
- wait被当前未收消息、终态通知、根steer或超时唤醒。只观察，不能将超时当任务完成、不能关闭Agent、不能消费正文造成丢失；正文在同一外层批末持久化交付，下一请求可见，重复wait不重复插入。同一观察点有真实更新时先返回updates；无更新才判steered/timeout。
- wait_agent只允许是assistant批次最后一条；同批多wait或wait后还有动作，在入史/执行前整批拒绝，避免先执行依赖未知结果的动作。其余控制调用继续006的批内串行顺序和完整结构检查，根可以一批spawn三个子Agent。
- 发送/最终回复的竞态在同一邮箱门判定：补充输入先受理则在子终文提交前交付并继续同一assignment；终文先提交则Agent已空闲，send_message只排队，followup_task按新任务原子受理。已受理未消费输入遇中断/组关闭，记录abandoned及原因，不在未来任务中悄悄执行。
- 同一工具调用不得因保存重试、网络失败或宿主回调重复而二次入队/启动。宿主按group＋调用者＋assignment＋request_seq（模型请求序号）＋原始call_id识别已受理命令；同一来源身份但参数不同为协议错误。该规则不把新call_id或新请求中的相同正文/复用call_id误认为重试，保持006仅批内ID唯一的契约。

### 1.5 状态、取消与根轮结束

| 状态 | 含义及允许动作 |
|---|---|
| starting | 名称/资源及创建记录已提交，子任务尚在初始化；计执行槽 |
| running | 正在请求模型/执行工具/处理补充输入；计执行槽，同一Agent最多一个assignment |
| stopping | 新动作门已关闭，取消/进程回收或在途请求仍未排空；仍计槽，不可followup启动 |
| completed / failed / interrupted | 本次assignment已结束且能继续的上下文保留；不占执行槽，仍占当前组Agent记录；可接消息和显式续派 |
| closed | 根组关闭，旧对象只供历史展示；不可重新唤醒 |

子任务普通终文、错误或中断必须先收尾所拥有进程/在途动作，再发布相应终态；终态报告附宿主收尾摘要（已回收的句柄、结果未知的外部动作），不能被模型终文覆盖。completed不表示业务目标已通过验收；failed若存在被截断/损坏的原生上下文，标context_continuable=false，followup返回CONTEXT_UNAVAILABLE，不能拿坏签名续接。

- interrupt在线性化点原子撤销该assignment的受理资格、置取消标志、放弃此前未交付的补充输入并进入stopping。重复interrupt不新建取消任务；空闲目标保持原状态。完成已先提交则返回completed，不改成假中断；取消先到则晚到回复不能把它改成completed。
- 取消沿真实边界生效：exec/MCP用该assignment独立取消，不能关闭共享MCP服务器或整个ProcessManager。模型传输复用现有超时；本地等待结束不等于HTTP线程结束，仍在排空的子请求继续占用全局3个子槽之一，不让重复interrupt/followup制造无限线程。迟到返回按group/assignment关闭门丢弃，不执行tool_calls、不入子上下文。
- 子进程归属为group/agent/assignment，这是宿主可信逻辑标签，不是Unix UID；033的同宿主UID与独立PID命名空间不能替代句柄归属校验。进程全局容量仍64，不能每个子Agent各开64。子任务结束/中断/组关闭按宿主PGID回收其所拥有的命令进程，不把沙箱内PID当作宿主进程组号；兄弟Agent不能猜句柄操作它，根原有进程沿016保持原行为。首版不支持把子进程转交根长期保活，长期服务由根直接启动。
- 根终文封口前原子检查邮箱与活动任务：已有未交付报告则先交付，根在剩余8次额度内再判断；仍有活动子任务却直接结束时，关闭组并返回未收尾错误，不把未完成子工作报告为完成。没有未收更新/活任务才接受根终文。
- 根取消、模型/协议失败、请求额度耗尽、切换会话或退出均撤销整组能力、收取已发生事实并关闭子Agent；在途传输排空槽保留到真实释放。根下一轮不恢复旧group，旧传输不因同名新任务获得身份；新轮的子槽准入仍计全应用未排空槽。
- 根steer只进入根通道，唤醒wait但不自动广播或重新运行子任务；根明确send/followup/interrupt后才改变子Agent输入。根032上下文切换不重置group/预算/邮箱/授权，子Agent不调用new_context；根若034 cell尚未消费，先遵守034的独占控制规则。

### 1.6 共享资源与请求预算

| 对象 | 首版边界和处理 |
|---|---|
| 并发与记录 | 3个子执行槽（含stopping/在途排空）；每组最多8个Agent、最多8次assignment启动（初始任务＋空闲续派）；满额拒绝，不静默淘汰上下文或重用名称 |
| 模型请求 | 根仍最多8；每个子assignment最多8，整组所有子任务累计最多24；发送失败/取消也计数，followup不清累计值。根预算独立预留，本次对话调度的根＋子最多32次实际模型请求，不宣称“额外请求为0”；既有013独立后台记忆管线不计入这32次，子任务不另触发该管线 |
| 子工具调用 | 每批仍≤32；每assignment最多64次、整组子工具最多256次已受理调用，含send_message及失败调用；原子准入，续派/中断不返还已用额度；根普通调用仍按既有契约 |
| 消息 | 单条任务/消息16 KiB；用户态Agent消息每接收者最多32条未交付且完整序列化≤256 KiB；每组累计≤128条/1 MiB（含初始任务/续派），超限先拒绝，不先accepted后丢弃 |
| 终态通道 | 每次assignment预留一个终态报告槽，最多8份，每份正文≤64 KiB；另预留256 KiB持久化收尾额度（含该报告），根组关闭另留64 KiB，这些空间计入64 MiB总额。与普通消息队列分开，消息洪泛不能阻止终态通知；超长终文保留片段/原长/哈希并以RESULT_TOO_LARGE失败 |
| 墙钟 | 每assignment受理起300秒，包含模型/工具等待；单调时钟到点先关动作门并请求中断，不承诺网络在300秒瞬间结束；排空继续占槽，followup活动输入不延长期限 |
| 子历史/证据 | 单个子事件完整序列化≤2 MiB，整组新增子事件负载累计≤64 MiB；发子模型请求/执行工具前分别预留10 MiB响应或2 MiB结果记录额度，提交后按真实大小计费并释放未用预留，容量不足不开始动作 |
| 沿用硬边界 | 五协议实际请求/响应各10 MiB，session行64 MiB、进程总64；没有Agent能力时原路径不新增限制 |

模型响应的10 MiB预留同时保障其转换记录的空间，实际超过子事件2 MiB时不接受其tool_calls；用有界片段/原长/哈希记录失败，子上下文不可续用。工具已执行后结果/展示超限时也明确记录执行状态与截断证据并失败，不能截断JSON/opaque后当成功续接。新事件总负载计量包含JSON转义与来源字段；派生展示不重复算持久化负载，正在预留的空间也计总上限。调用前额度不足不能触发外部动作，不能靠丢旧记录过关。

子工具复用既有真实分派：命令、MCP stdio、补丁经033的 `sandbox::tool_launch_argv`，包装失败明确报错；view_image继续019的宿主路径，待031实际交付后才接其受限图片入口。Agent调度与模型API本身不因工具包装而处于bubblewrap内，不能宣称整个子Agent已获独立文件权限沙箱。

主进程cwd保持稳定，子任务工作目录只传给具体执行命令，不能调用全局chdir或以子workdir重建033策略锚点；模型和消息均不能改写该锚点。不把“由另一个模型提出”当作新增权限。MCP连接/冻结目录共享，取消只作用于该调用，不重新建一套认证或每子任务重复tools/list。若031已实施，子任务只用可信基线，不能从父临时grant或子消息重建授权。

### 1.7 V3历史、投影、展示与恢复

新增 `HistoryItem::AgentEvent`，包含group_id、agent_name、assignment_id、宿主递增seq与受限payload；类型为agent_created、assignment_started、child_record、message_queued、message_delivered、state_changed、group_closed。child_record只容纳真实共同消息/原生包装，不允许递归AgentEvent、检查点、JS事件或任意宿主控制对象。创建及消息/任务准入记录保存成功才发布状态、启动或回accepted；child_record保存成功后才继续依赖其结果的下一动作。

- 全组只有根AutoSession一个追加写者和一份权威V3文件。子轮独立持有上下文，事件通过有界通道交根调度泵顺序提交；线程不并发修改主Vec，不各自锁写同一auto文件。保存失败关闭组、明确未启动/已发生/结果未知，不能伪称回滚；无V3提交宿主时spawn失败。
- fork种子只在agent_created中记录早于自身、已闭合根前缀的引用（context_id、prefix_end及fork模式），运行时用不可变共享快照；不把父10 MiB历史复制8遍写成新经历。引用不能指向未来/自身，不能把其他group的子历史作为fork源；根历史已收取的旧报告仍只是普通资料，不保存实时权限。父system/记忆仍按原契约注入，不为重放额外持久化凭证或记忆正文。
- 每个子Agent的call_id/name和NativeOwner原样保留，身份组合键为group/agent/assignment/request_seq/call_id；相同call_id出现在不同子会话/请求不能互相配对。子原生块只回该子Agent的相同连接；不能把子工具结果/签名接进根或兄弟原生调用链。
- 根messages、_history和五协议投影先过滤非根child_record/控制事件，再校验原生批次；只在合法批末把发给根的message_delivered转成带来源的普通消息。子投影同理只含其fork前缀、自己的记录及已交付输入；原始记录不改写成另一Agent发起的tool_calls。
- CLI/TUI按路径显示任务、状态、来源消息、子工具及真实差异/计划，与/resume共用投影；根和子的计划区分归属，不互相覆盖。终文/报告不自动判为验证通过。参数、输出、错误的终端控制字符只过滤展示副本，不能把OSC/ANSI当终端命令执行。
- /save保存完整根/子事实与源引用；/resume只恢复展示及可携带历史，不启动子Agent、重投邮箱/任务、恢复进程/JS/授权或执行工具。旧活动状态显示已失联/结果未知；坏引用/事件/原生状态可展示可恢复部分，但禁止错误续接。
- 新记录使用032的V3及锁/提交点语义，旧程序拒绝V3；不认识AgentEvent的V3版本也须阻止续接。没有新增事件/检查点时仍按原V1/V2写入。032检查点不能把子控制事件或活句柄塞入seed，根上下文摘要不代替子原始记录。
- 记忆只登记根durable会话，不为每个子任务再启动013后台管线；`memory/rollout_input.rs`过滤fork复制、child_record及控制事件，只从根实际收取的消息/事实提取，保留Agent来源，不冒充真实用户或重复记忆。保持已有8 MiB/1 MiB预算，降级/未提取事实如实记录。

### 1.8 验证条件、性能与费用

运行测试只用临时HOME/配置/缓存/会话/工作区，本地固定模型和真实本地MCP服务；随机种子、时钟、locale与依赖锁定。调度、工具分派、通道、进程回收、落盘和投影用真实实现；Mock仅在网络边界。时间和竞态先用可替换单调时钟/屏障验证，E2E另用有界真实进程/PTY，不碰真实凭证和用户目录。

速度、峰值RSS、磁盘增量和费用均未测。M0最小原型先冻结可复现基线与p95/RSS要求，同机同版本留2–3倍余量，未冻结/复测不能待验收：根＋1/3子并发，fork all/none，消息/续派/中断/清理分别测；预热5次、正式100次、独立3轮取p95中位轮，保留全部样本。事件追加另用1千/10万历史，不反复全扫归档；记录客户端/服务器开销、CPU、线程数、所有子进程合计峰值RSS、磁盘实际新增字节和已保留种子引用大小。

并发收益以屏障证明3个子模型/工具请求真实重叠，不仅看工具名或虚构时间差；同一固定任务比较串行与并发，分别记录实际模型请求数、提供方原始usage与缺失字段，token/费用不能拿字节或线程数冒充。多Agent会增加模型调用，预算是调用数上限，不是金额上限；费用未测，不写0元或默认更省。

## 2. 哪些情况容易出错

| 情况 | 期望结果 |
|---|---|
| 缺开关/只配部分、内部记忆、无保存宿主 | 整组关闭或明确失败，零子请求，不靠主循环未知分支误执行 |
| 名称重复/非法/点路径/中文/编码斜线、消息空白/边界/类型错 | 原文合法保留；非法零受理，不净化成另一个Agent |
| 第4个活子、第9个Agent/assignment、耗尽子24请求/工具256 | 原子拒绝不泄槽，根8次预算不被子抢走，不淘汰历史 |
| send给空闲、followup运行中/终文竞态、重复回调 | send不启动；followup最多一轮，不清额度；同身份准入恰一次 |
| 消息洪泛、超长终文、wait超时/steer/已完成/no_pending | 队列有界、终态预留可达、正文不丢不重，超时不当完成 |
| 同批多个spawn、wait非批末/多wait、Agent消息恰到批中 | 先完整校验，消息仅批末交付，不破坏原生配对 |
| 子猜父/兄弟进程号、禁用MCP、伪造权限或消息作者 | 实际分派拒绝，旧grant/目录/身份不能激活新能力 |
| fork all时父批次未闭合、032已压缩、大上下文/附件/签名 | 只取请求前闭合活动前缀，引用不循环；原生签名不跨Agent |
| interrupt与tool admission/完成同时发生、在途HTTP未退出 | 关闭门后零新动作，晚结果不执行；排空计槽，不产生无限线程 |
| 子启动进程后结束/中断，其他Agent仍工作 | 只回收自己的进程，全局64不放大，不关闭共享MCP或根进程 |
| 根终文、8次耗尽、失败/cancel/会话切换/退出 | 邮箱原子封口、组关闭、已发生事实保留，不遗留可执行旧Agent |
| 保存故障、坏V3/引用/孤立start、同名新group遇旧回调 | 未存不启动；未知如实；恢复不重放，旧回调不能污染新组 |
| 子事件/原生结果超大或总容量不足 | 预留先行，明确截断证据/不可续用；不截坏JSON/签名当成功 |
| 共享文件/子计划/恶意终端字符、记忆后台触发 | 不自动合并/回滚，投影归属清楚，展示安全，记忆只登记根 |

## 3. 重要做法，为什么这样做

### 3.1 固定参考与本地取舍

- 固定上游 `openai/codex@d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4` 的[多Agent工具定义](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_spec.rs)同时含旧V1与V2；本项只对齐原表六个V2名称、all/none与消息/续派/wait的职责，不同时搬send_input/resume_agent/close_agent。
- [V2消息路径](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_v2/message_tool.rs)区分是否触发轮，[wait](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs)观察邮箱/steer/超时，[registry](https://github.com/openai/codex/blob/d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4/codex-rs/core/src/agent/registry.rs)有路径/槽位预留。本项复用语义与原子预留思路，采用本地有界注册表；不搬整套Codex Session/环境/遥测框架。
- [OpenAI Docs多Agent说明](https://developers.openai.com/api/docs/guides/responses-multi-agent)描述Responses托管动作，应用不应自行执行其multi_agent_call。本项是五协议普通函数＋本地调度，不发送托管开关或伪造托管输出；不能把当前API默认深度、并发或成本当本地保证。
- 现有模型发送可从main/tui_host的闭包中抽最小拥有型快照，MCP已持有Arc连接并逐次传取消；复用已锁定Tokio/通道与真实turn执行，不另造对话执行框架。子轮是明确新模式，不能调用会重置全局取消/读取用户输入/触发记忆后台的普通用户轮宿主入口。
- 一层/同根轮生存期、不允许模型覆盖、共享工作区、消息与终态分道、子进程归属及额外预算均为本地首版取舍；不宣称上游递归、多模型、跨轮唤醒或独立只读沙箱已经实现。

### 3.2 根据规格制定的实施计划

以下M0–M7共同完成「多 Agent」一项，均待实施，不另建六份规格。

| 步骤 | 修改落点与工作 | 验证与退出条件 |
|---|---|---|
| M0 前置与基线 | 核对工作区、032 V3实际交付、033入口与共享dev部署、030/031待实现边界；记录普通/沙箱测试原有状态。最小原型验证拥有型模型快照、3个固定子槽、有界调度泵、取消后传输排空，以及真实bwrap下owner/宿主PGID/工作区锚点；冻结§1.8性能/资源夹具与阈值 | 前置缺失不启用生产开关、不顺带实现其他行；同步网络的取消局限、线程与磁盘开销有实证，033已有验收不替代多Agent并发收尾验证 |
| M1 输入与状态机 | 新增 `src/app/src/agents/`（args/registry/mailbox/budget），lib.rs导出；先写非法输入/准入竞态红例，再实现六工具DTO、路径和原子预留/去重/消息门；新增tests/multi_agent.rs | 六接口含本地适配可判定；所有容量边界、send/followup区别、消息终文竞态可用屏障复现，无空跑Mock核心状态 |
| M2 子轮与共享分派 | turn.rs增加明确子轮模式；从main.rs/tui_host.rs抽最小拥有型sender工厂；复用普通函数编码、真实业务分派及033命令/MCPstdio/补丁包装，固定子模型/目录/上下文fork及独立取消，不改全局cwd | 根与3子真实本地模型服务并行；同一子不重入，不跨Agent复用opaque；禁止工具和旧工具集合零执行；策略锚点稳定，根现有调用兼容 |
| M3 进程归属与收尾 | proc_session.rs增加可信逻辑owner及按owner回收/句柄校验，旧根入口明确映射Root而非绕过校验，owner不来自模型参数或Unix UID；共用全局64容量；保留033命名空间并按宿主PGID回收；agents运行时实现任务/根轮出口、期限、传输排空占槽；MCP只取消当前调用 | 真实bwrap下管道/PTY子进程只回收自己，根/兄弟不受影响；不恢复跨UID补杀、不混用内外PID；晚到tool_calls零执行，反复中断/续派不泄线程/槽，根预算保留 |
| M4 V3证据与模型交付 | 基于032扩展prompt.rs/session.rs的AgentEvent/引用/严格装载/提交回执；主写者调度泵与有界事件通道；模型投影与memory/rollout_input.rs过滤、来源及根唯一登记 | start/消息保存后才执行或accepted；落盘失败、超大事件、旧group回调、错误ID/签名/坏引用有反证；根/子消息各自批末恰一次，恢复不执行 |
| M5 能力、展示与五协议 | catalog.rs/prompt.rs六项opt-in，main/tui_host传运行时与通知；TUI复用工具/聊天投影按路径归组；扩充prompt_assembly/native_model_apis/session/session_cli/e2e_acceptance | 五协议真实根spawn→子工具→消息/终态→根回复全链；CLI/PTY保存恢复一致、ANSI安全；关闭态兼容；实测并发、p95/RSS和磁盘/请求数 |
| M6 加固、安全与独立审查 | TDD后依次comprehensive-testing、Security Testing、code-review，安全按现有Security-Testing-Gate，审查读取rules/REVIEW.md；独立只读审查子代理找问题 | 越权、033路径/策略锚点、owner与同UID命名空间下取消、资源、邮箱/终文竞态、重放重点覆盖；关键测试用修复前后/临时破坏证明能检错，问题用diagnosing-bugs定位复验 |
| M7 全量验证与交付 | 修复后运行最终全量验证，同步受影响016/022/024/032接入说明、对照表与经验；证据统一本项logs | 报告通过/失败/忽略/未运行数量，适用CI存在则通过；功能/性能/资源证据齐全后最多待验收，最终状态由验收人判定 |

M0定向基线（工作目录 `/workspace`，不替代交付回归）：

```bash
env -u TCHAT_SANDBOX cargo test --locked --test tool_loop --test prompt_assembly --test proc_session --test exec_sandbox --test apply_patch_sandbox --test mcp_stdio_sandbox --test mcp_turn --test native_model_apis --test session --test session_cli --test session_perf --test memory_runner --test memory_phase1
```

实现轮另跑普通模式 `env -u TCHAT_SANDBOX cargo test --test e2e_acceptance`；最终普通回归为 `env -u TCHAT_SANDBOX cargo check && env -u TCHAT_SANDBOX cargo test --workspace --no-fail-fast`。共享dev默认注入开关，普通基线须清除；专用沙箱用例在隔离进程内自控开关，另须用真实bwrap验证子任务并发、owner、句柄拒绝及命名空间下回收，不以普通回归或已有033用例代替。在工作区副本跑Cargo必须显式 `--target-dir`。本次纯文档工作不运行这些命令，不用文档检查代替代码、性能或安全验证。

## 4. 踩过的坑，怎样解决

尚无本功能实现、真实并发/取消或费用实测结论。本轮源码核对只确认：主程序已有可复用模型发送/工具循环，记忆注释中出现failed_spawn_agent不能证明有多Agent；共享ProcessManager没有Agent归属，须在新增模式中补齐。依据见[调研记录](../logs/035-multi-agent/2026-10-26-spec-research.md)。

可复用取舍统一记录在[项目经验第7节](../../rules/project-lessons.md#7-实现取舍与问题根因)，不把预测的竞态当已发生故障。033当前状态以[真实宿主验收](../logs/033-tool-sandbox/2026-10-27-host-acceptance.md)和[共享dev部署](../logs/033-tool-sandbox/2026-10-27-dev-deploy.md)为准，不把该证据外推为本项owner/并发收尾已验证。

## 5. 先预测，再实测，最后确认是否合格

| 编号 | 动作与预计结果 | 实测/验收 |
|---|---|---|
| C01 | 六项开关全/部分/关闭、内部记忆、无V3保存宿主：只在完整条件下开放，零隐式子请求 | 待测 |
| C02 | 六接口合法/非法/多字段/UTF-8和大小边界：原文保留、严格路径、不归一成其他目标 | 待测 |
| C03 | 根一批spawn3子，屏障验证真实并发、独立历史和共享文件可见；子不能再spawn/联系兄弟 | 待测 |
| C04 | fork all/none＋父未闭合批/032压缩/合法图片/原生签名：只取闭合活动前缀，不跨Agent续签 | 待测 |
| C05 | send给运行/空闲、followup给运行/空闲、同请求重复call回调/同身份异参数、新请求复用ID：不误唤醒/重入/错误去重 | 待测 |
| C06 | 消息与子终文/根终文竞态：FIFO、批末恰一次、封口不丢已受理输入，wait无重复正文 | 待测 |
| C07 | wait超时/0轮询/steer/更新/no_pending和批中/多wait：时间不假完成，整批校验先于副作用 | 待测 |
| C08 | list前缀a/ab、状态/最近任务、错误/旧组路径：结果稳定且只含当前组，不泄其他组 | 待测 |
| C09 | 3/4并发、8/9记录与assignment、子24请求和256调用及消息边界：原子限额，根8请求保留 | 待测 |
| C10 | 超长消息/终文、队列洪泛、子2 MiB事件/64 MiB组负载：预留生效、终态不饿死，超大结果不续坏上下文 | 待测 |
| C11 | 子猜父/兄弟进程、禁止工具/MCP/临时grant、模型/角色覆盖或改变workdir：零越权；owner由宿主确定，业务经过033实际包装且策略锚点不变 | 待测 |
| C12 | interrupt抢在工具受理/完成前后、重复interrupt、模型迟到：门关闭后零新动作，排空仍占槽 | 待测 |
| C13 | 真实033沙箱下子exec管道/PTY结束/中断、64总容量、共享MCP有并发请求：按宿主PGID只回收自己的进程与等待，不损根/兄弟，不混用沙箱内PID | 待测 |
| C14 | 根终文/额度/失败/cancel/切会话/退出与同名新group：不遗留可执行旧Agent，旧回调不污染新组 | 待测 |
| C15 | 创建/消息/结果落盘故障、孤立start/坏V3/坏fork引用：未存不执行或accepted，恢复显示未知不重放 | 待测 |
| C16 | 五协议完整根子流程及相同call_id的不同Agent：结果/签名各归其主，根只收已交付报告 | 待测 |
| C17 | CLI/TUI/PTY、保存/resume、子计划/终端控制字符、记忆触发：投影归属一致，恢复只展示，记忆只登记根 | 待测 |
| C18 | 冻结并发/串行性能、RSS/磁盘/大历史追加和真实调用计数；关键反证及全量检查：阈值满足、费用未测如实 | 待测 |

实现证据统一写 `docs/logs/035-multi-agent/`，保留实际命令、工作目录、退出码、脱敏原始输出及必要PTY渲染结果。验收人按冻结规格、证据和可运行产物独立核验，不读实现源码；未测试/未定资源阈值不能通过，实现方不得自行改为已通过或为过关删用例、放宽预算。
