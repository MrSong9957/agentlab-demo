# 后端高风险测试优先级评估

日期：2026-10-21。代码基线：`bb84d180e259b6aa361abd0ad250a935d4800258` 及当前工作区。用户要求独立子代理按资金、权限、数据后果筛选重点，本次不实施补测或修复。

由三个独立子代理分别只读核对权限与凭证、持久化与记忆数据、模型费用与业务范围，主代理汇总并核对关键断言。已先检查 Git 状态；原有 AGENTS.md、历史记录和未跟踪文件均保留。本次只新增本记录，未修改实现、测试或 project-lessons.md。

当前是 Rust `terminal-chat` 共用后端，已实现模型连接、工具调用、会话保存恢复和可选记忆。未发现支付、余额、订单或多租户 RBAC 功能；资金风险主要是模型调用费用及 API Key 泄露后的额度滥用。依据：[模块入口](/workspace/src/app/src/lib.rs:1)、[模块职责](/workspace/project-lessons.md:14)。

P0 表示应优先保证的安全或数据底线；P1 表示紧随其后的核心回归。这里排序的是测试投入，并非已确认漏洞严重度。已有测试引用仅说明可复用的证据入口，不代表本轮执行通过。

| 优先级／功能 | 出错后果 | 最小高价值验证 | 实现与现有测试入口 |
|---|---|---|---|
| P0 凭证授权、读取与外发 | 权限、数据、间接资金：Key 或聊天内容发到错误目标，凭证被滥用；项目配置重定位信任根后读取不该读取的凭证。 | 聊天和 refresh 未授权零请求；换地址／Key 不复用旧授权；重定向不向新目标发请求；项目 .env 不能重定位信任根；凭证文件不能经路径或链接逃出凭证根；错误回显不泄露假 Key。 | [origin 门](/workspace/src/app/src/catalog.rs:470)、[信任根](/workspace/src/app/src/catalog.rs:198)、[拒绝重定向](/workspace/src/app/src/api.rs:16)、[未授权 CLI](/workspace/src/app/tests/llm_flow.rs:749)、[换 Key](/workspace/src/app/tests/llm_flow.rs:796)、[重定向](/workspace/src/app/tests/llm_flow.rs:1559)、[错误脱敏](/workspace/src/app/tests/llm_flow.rs:1625)。 |
| P0 工具执行权限与停止语义 | 权限、数据：后台记忆处理升级为宿主用户级命令／文件修改；用户取消后受管理子进程仍继续改文件。 | MemoryMerge 仅放行 mem_*；exec/apply_patch 拒绝时真实目标零副作用，合法记忆调用确实执行；超时／取消后受管理进程组停止写盘，独立命令不被误杀；迟到模型结果不执行工具。 | [模式分派](/workspace/src/app/src/turn.rs:185)、[记忆路径限制](/workspace/src/app/src/memory/mem_tools.rs:215)、[进程组回收](/workspace/src/app/src/exec.rs:171)、[拒绝跨模式工具](/workspace/src/app/tests/memory_phase2.rs:719)、[超时](/workspace/src/app/tests/exec_command.rs:192)、[取消](/workspace/src/app/tests/exec_command.rs:213)、[独立组](/workspace/src/app/tests/exec_command.rs:335)。迟到结果契约见 [项目经验](/workspace/project-lessons.md:77)。 |
| P0 补丁新增、修改、删除及失败状态 | 数据：覆盖／删除错误文件；半写后误报成功或误报无改动，使后续工具依据错误状态继续修改。 | 完整解析失败无写入；多文件中途失败核对每个文件真实字节、前序成功清单和 state_known；写入不确定必须重读；操作系统只读权限不可绕过。 | [失败语义](/workspace/docs/specs/004-apply-patch.md:35)、[半写注入](/workspace/src/app/tests/apply_patch.rs:304)、[只读权限](/workspace/src/app/tests/apply_patch.rs:496)、[非普通文件](/workspace/src/app/tests/apply_patch.rs:1005)。 |
| P0 会话自动保存与恢复 | 数据，兼副作用：写满／半写导致历史丢失；双写者截坏文件；恢复时重放命令或补丁。 | 追加失败补写恰一次；快照失败保留旧版；两进程恢复同一会话时失锁方对原文件零修改；恢复只投影历史，工具执行次数不增加；中断调用不得虚称未执行。 | [锁顺序](/workspace/src/app/src/session.rs:648)、[半写处理](/workspace/src/app/src/session.rs:791)、[持久化回归](/workspace/src/app/tests/session.rs:914)、[零重放](/workspace/src/app/tests/session_cli.rs:489)、[锁冲突 CLI](/workspace/src/app/tests/session_cli.rs:599)。 |
| P0 启用记忆后的更新、清理、reset 与任务所有权 | 数据、权限边界：误删原始会话或根外目标；reset 后旧任务重新写入；过期任务覆盖新结果；成功水位错误导致后续永久漏处理。 | reset 缺 confirm 零删除，sessions 与链接目标字节不变；reset 后旧任务不能改新代资源；租约接管后旧令牌提交无效；清理保护已选中／近期使用内容；文件、Git、SQL 失败状态真实。 | [reset](/workspace/src/app/src/memory/runner.rs:536)、[提交令牌](/workspace/src/app/src/memory/store.rs:427)、[清理 SQL](/workspace/src/app/src/memory/store.rs:1235)、[范围约束](/workspace/src/app/src/memory/retention.rs:25)、[reset 回归](/workspace/src/app/tests/memory_runner.rs:551)、[代际回归](/workspace/src/app/tests/memory_runner.rs:1049)、[租约](/workspace/src/app/tests/memory_store.rs:218)、[清理](/workspace/src/app/tests/memory_retention.rs:215)。 |
| P1 自动上下文外发与记忆脱敏 | 数据：超出约定来源的本地文本自动外发；受支持秘密进入长期记忆后被反复发送；项目规则被提炼成用户事实。 | 用标记文件检查实际请求的来源与范围；AGENTS 跳过链接／非普通文件；关闭记忆零读取；受支持秘密在提炼请求、结果、数据库／Markdown 中脱敏；上下文消息被排除，普通相似用户文本保真。 | [来源加载](/workspace/src/app/src/context.rs:208)、[提炼输入过滤](/workspace/src/app/src/memory/rollout_input.rs:153)、[输出脱敏](/workspace/src/app/src/memory/phase1_output.rs:34)、[上下文测试](/workspace/src/app/tests/context_injection.rs:297)、[秘密输入](/workspace/src/app/tests/memory_phase1.rs:321)、[秘密输出](/workspace/src/app/tests/memory_phase1.rs:493)、[关闭态读取](/workspace/src/app/tests/memory_inject.rs:810)。 |
| P1 模型调用止损、失败不重放与后台去重 | 资金、数据：循环或重复后台处理持续消耗额度，重复执行工具造成文件副作用。 | 连续调用恰 8 次、无第 9 次，保存第 8 批结果；超过 32 个工具整批拒绝；后续 500／断线不重发已执行副作用；记忆关闭零请求，同源版本去重、租约、失败退避和合并冷却阻止重复调用。 | [请求循环](/workspace/src/app/src/turn.rs:368)、[失败停止](/workspace/src/app/src/turn.rs:383)、[候选去重](/workspace/src/app/src/memory/candidates.rs:103)、[请求上限](/workspace/src/app/tests/tool_loop.rs:298)、[失败不重发](/workspace/src/app/tests/tool_loop.rs:320)、[断线](/workspace/src/app/tests/llm_flow.rs:1111)、[记忆关闭](/workspace/src/app/tests/memory_runner.rs:109)、[候选](/workspace/src/app/tests/memory_candidates.rs:341)、[冷却](/workspace/src/app/tests/memory_phase2.rs:224)。 |

本轮发现的测试证据缺口：`dispatch_rejects_unregistered_tools_in_merge_mode` 的 apply_patch 目标是相对路径 `probe2`（[输入](/workspace/src/app/tests/memory_phase2.rs:727)），实际断言的是临时 `outside/probe2`（[断言](/workspace/src/app/tests/memory_phase2.rs:752)）；该路径未作为补丁 cwd 传入。应优先把被测目标和副作用断言对齐。测试仍核对“未知工具”错误及合法 mem_write 成功（[结果断言](/workspace/src/app/tests/memory_phase2.rs:770)），因此这处证据不足不等于权限实现存在漏洞。本轮仅静态核对，未运行或变异该用例。

需要保留的已批准边界与待核项：

- 用户轮 exec 的 cwd 不是沙箱；apply_patch 允许绝对路径，多文件失败不回滚。停止工具不会撤销此前文件变更。测试不得擅自增加审批／事务承诺。依据：[exec](/workspace/src/app/src/exec.rs:4)、[补丁规格](/workspace/docs/specs/004-apply-patch.md:11)。
- AGENTS、环境上下文与同一用户的全局记忆摘要外发属于既有行为。原始会话按契约保留用户文本和工具输出，不承诺通用脱敏；依靠私有文件权限保护，应用不主动保存鉴权配置。依据：[015 边界](/workspace/docs/specs/015-turn-context-injection.md:54)、[007 凭证边界](/workspace/docs/specs/007-session-persistence.md:215)。
- 记忆文件／Git／SQLite 不具备跨存储原子事务。测试应验证失败状态与后续处理，不要求全回滚。依据：[实现](/workspace/src/app/src/memory/phase2.rs:325)、[回归](/workspace/src/app/tests/memory_phase2.rs:1589)。
- 记忆摘要注入先整读后截断（[inject.rs](/workspace/src/app/src/memory/inject.rs:23)）；未见该入口自身的摘要／根符号链接与超大文件专项测试。整读有既有规格依据，此处仅登记待核实的本地文件信任边界，未复现泄露或拒绝服务。字符串级恶意内容测试不能证明真实模型抵抗提示注入。
- 每轮 8 次请求是次数上限，不是费用上限。后台管线无真实金额预算／配额检查（[runner.rs](/workspace/src/app/src/memory/runner.rs:281)）。主轮取消不撤销已经启动的记忆任务，不能把继续运行一概当取消缺陷。

可以暂缓的是新增专项投入，已有回归保留：

| 功能／范围 | 可以暂缓的内容与可能后果 |
|---|---|
| 模型目录、会话列表、记忆状态展示 | 排序、列宽、字节数展示、提示措辞；主要影响可读性。实际连接路由、秘密脱敏、终端转义过滤仍属安全检查。 |
| 记忆摘要措辞与文风 | 更多模板／语言组合的逐句质量评分；主要影响回答质量。秘密、错误持久化、删除传播仍优先。 |
| 性能与真实供应商矩阵 | 超出现有规模的压测、所有模型逐个联网；主要影响特定环境可用性与接入信心。资源上限和实际使用供应商关键流程保留。 |
| 支付、余额、订单、租户权限矩阵 | 当前未实现，属于不适用；以后引入时重新评估。 |

验证范围：只读审查源码、规格、既有测试和历史安全记录；未运行 Cargo、E2E、安全攻击、真实供应商请求或变异测试，不引用历史通过数量作为当前结论。产物只有本记录。文档检查：全部本地链接目标存在且行号有效，无行尾空白；no-index 差异检查无空白错误输出（退出 1 表示新文件差异，不记作测试通过）。
