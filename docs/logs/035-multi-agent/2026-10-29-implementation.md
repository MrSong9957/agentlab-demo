# 035 多 Agent 实施记录（2026-10-29）

- 分支：`feat/035-multi-agent`（基线 main＝`4766d61`，与计划一致）。
- 实施链：M1–M5 一次推进＋定向回归；M6 综合测试/安全攻防/独立审查（记录见本目录后续文件）；M7 全量验证（结果见文末）。
- 本记录由实施 Agent 维护；证据命令的工作目录均为 `/workspace`，环境统一 `env -u TCHAT_SANDBOX`。

## 1. 交付形态

新增模块 `src/app/src/agents/`（`mod/args/registry/mailbox/events/runtime`）＋接入点：

| 落点 | 内容 |
|---|---|
| `agents/args.rs` | 六工具参数严格校验（手写 visitor＋`deserialize_map`：拒绝未知/重复字段、null、数组形式、浮点/负数/字符串数字；message ≤16 KiB 原文保留；task_name `[a-z][a-z0-9_]{0,31}`；target 路径严格解析——无点路径/编码斜线/多层级/尾斜线） |
| `agents/registry.rs` | 状态机（starting/running/stopping/completed/failed/interrupted/closed；前三计槽）＋组预算原子核算（8 Agent/8 assignment/24 子请求/256 子调用/128 消息/1 MiB/64 MiB 事件，含响应 10 MiB/结果 2 MiB 预留-计费-释放） |
| `agents/mailbox.rs` | 每 Agent FIFO＋接收者级容量（32 条/256 KiB）＋abandon |
| `agents/events.rs` | 七类 V3 AgentEvent 构造、child_record 可携带判定（拒递归事件/检查点/JS 事件）、终态报告 64 KiB 降级（片段＋原长＋SHA-256）、`validate_agent_chain`（每组 seq 连续、created 先行、group_closed 唯一收尾） |
| `agents/runtime.rs` | AgentManager（每根用户轮 `begin_turn` 新 group）＋GroupRuntime（六工具执行、子轮循环、FinalAnswer 邮箱门、终态恰一次发布、writer 互斥串行提交）＋`SpawnContext`（冻结活动前缀/已声明动态集合/MCP/能力/可信 system）＋全局 3 子槽原子预留释放 |
| `turn.rs` | TurnExtras.agents；wait_agent 批末整批拒绝（前缀切片写法）；六工具分派门；批末交付（消息/终态投影为带来源 user 输入＋事件入内存＋通知转发）；终文封口门（未交付先交付→剩余额度再判断；有活动子任务→关组＋未收尾错误）；轮出口统一封组（wrapper 拆分，全部出口必经） |
| `session.rs` | `HistoryItem::AgentEvent` 仅 V3（V1/V2 携带整拒、损坏事件行整拒）；`commit_agent_event_root`（根线程，随行排空待写段）/`commit_agent_event_pump`（子线程，仅事件行）；`agent_untracked` 计数＋flush 吸收（与 orch_untracked 分开核销）；装载链校验接入 |
| `proc_session.rs` | `OwnerTag`（Root｜Agent{group,agent,assignment}，宿主逻辑标签非 UID）；`start_with_owner`/`interact_as_owner`（兄弟猜句柄拒绝）；`reap_agent_assignment`（SIGTERM→3s→SIGKILL 有界，killpg 走宿主 PGID）/`reap_agent_group` |
| `prompt.rs` | `AgentEvent`/`AgentEventPayload`/`AgentReport` 类型＋投影过滤；`build_turn_request_with_agents`（六工具声明追加，关闭路径逐字节一致）；四个工具定义转 `pub(crate)` 供子目录复用；`append_memory_context` 抽取共用 |
| `catalog.rs` | `supports_multi_agent`（六精确名全配）/`multi_agent_missing`（部分配置提示一次） |
| `main.rs` | 会话共享化 `Arc<Mutex<Session>>`；AgentManager 持有；六工具 CLI 摘要行；部分配置一次性提示 |
| `tui_host.rs` | 同款装配（AgentExtras＋发送快照工厂）；TurnSteerChannel `pending_hint`（预取缓存，非消费探测——wait 的 steer 唤醒）；Agent 事件按来源归组展示一行（child_record 不重画，展示副本 sanitize＋截断） |
| `api/native*`、`memory/rollout_input.rs` | 五协议原生投影与记忆提炼输入过滤 AgentEvent |

**本地调度适配**（§1.7「有界通道交根调度泵」的实现形态）：子线程与根线程经
`writer 锁 → session 锁` 单一锁链串行提交事件——语义等价：全组单追加写者、
根阻塞（模型请求/wait/自身工具）期间子事件持续落盘、seq 在 writer 域内分配＝
文件顺序。未采用独立泵线程＋通道的形态（少一类线程与通道背压面；锁序纪律
在 runtime.rs 头注释成文）。fork 种子按规格记引用（context_id/prefix_end/
fork 模式＋任务原文），运行时用不可变共享快照，不复制父历史。

## 2. 关键实现取舍（本轮新增）

- **子轮独立循环**（`child_loop`）而非复用根循环：根循环携带 steer/异步问询/
  上下文切换/权限/编排等根专属机制；子轮白名单式声明（业务四工具＋按能力
  view_image＋023 资源＋冻结动态 MCP＋仅向根的 send_message）天然满足
  「不声明即不可执行」的隔离证明。请求编码与业务分派复用既有函数
  （`build_turn_request_with`＋`_history` 五协议投影＋`execute_core_tool_owned`）。
- **子线程提交直写**（泵路径不排空根待写段）：文件顺序＝真实提交时序；
  事件行与根记录的内存顺序差异由 `agent_untracked` 吸收计数兜底（事件
  进入内存的顺序与写盘顺序一致，FIFO 保证计数与行一一对应）。轮出口封组
  后的排空期事件只落文件、由 /resume 呈现（控制事件不进模型投影，无重复
  注入面）。
- **终文邮箱门**：子终文在 core 单锁段内判定（邮箱有未交付补充输入→交付并
  继续同一 assignment；空→发布 completed；已中断→丢弃晚到终文）；与根侧
  send/followup 在同一锁上线性化，实现 §1.4 竞态裁决。
- **followup 续接**：空闲 Agent 的保留上下文（`Arc<Mutex<Vec<HistoryItem>>>`）
  复用；终文在 Complete 门入保留上下文（续接从终文后继续；持久化证据由
  终态事件报告承载，不重复 child_record）；空闲期排队消息在新 assignment
  开端交付。

## 3. 验证证据

### 3.1 定向（M0/M1–M5）

```bash
# M0 基线（2026-10-29-m0-baseline.log，exit=0）
env -u TCHAT_SANDBOX cargo test --locked --test tool_loop --test prompt_assembly \
  --test proc_session --test native_model_apis --test session --test session_cli
# 166 通过/0 失败（33+28+18+40+27+20）

# 新增集成套件（tests/multi_agent.rs；C01–C17 可自动化子集）
env -u TCHAT_SANDBOX cargo test --locked --test multi_agent
# 18 通过/0 失败（单线程与默认并行各两轮均绿；套件内 SUITE_LOCK 串行化
# ＋子槽真实排空等待——全局 3 槽是进程级共享，并行测试互抢会造成伪
# CAPACITY_LIMIT，033「覆盖缝串行」教训的再次应用）

# 受影响模块定向回归（exit=0）
env -u TCHAT_SANDBOX cargo test --locked --test session --test session_cli \
  --test prompt_assembly --test tool_loop --test native_model_apis \
  --test context_management --test memory_runner --test message_to_user \
  --test mcp_http --test mcp_resources --test mcp_turn --test memory_phase2
# 12 套件 353 通过/0 失败

# 库单测（含 agents 模块）
env -u TCHAT_SANDBOX cargo test --locked --lib
# 289 通过/0 失败
```

实施中经「修复→复验」闭合的缺陷（红→绿证据在 git 历史）：

1. 子线程提交 state_changed(running) 事件但未同步注册表状态——终文门把
   Running 误判为非运行、全部子任务误成 interrupted（探针复现→修复→
   c02/c03/c05/c06/c09/c11 全部转绿）。
2. spawn/followup 保存失败与预算拒绝路径泄漏全局子槽（4 处）——补
   `fetch_sub` 释放；c15b 由槽位排空等待覆盖。
3. c15 坏链用例首轮未触发装载拒绝：007 装载规则「头部非 user 记录丢弃至
   首个 user」把纯事件文件合法裁空——用例补 user 前缀后 seq 空洞整拒生效
   （规则本身正确，不改生产代码）。

### 3.2 测试有效性

关键断言按「真实行为」取证：屏障证明三子模型请求真实重叠（C03）；子真实
执行 exec（副作用文件/运行中句柄）并按 assignment 归属登记（C11）；真实
Session 落盘＋装载（C15）；fork=all 携带闭合前缀且 spawn 批不入（C04）；
send 不产生新请求以请求体计数证明（C05）。Mock 仅在模型网络边界（根/子
sender 注入），与规格 §1.8 一致。

### 3.3 审查与安全（M6）

- 独立只读代码审查：`2026-10-29-code-review.md`（新上下文代理，按
  rules/REVIEW.md）——2 阻塞（B1 排空期事件破坏装载、B2 预算旁路真实执行）
  ＋3 非阻塞（N1–N3）全部修复并补回归（multi_agent 含 b1/b2 用例 20/20）。
- 安全攻防：`docs/security/reports/2026-10-29-035-multi-agent-security.md`
  （Security-Testing-Gate 代理；8 项探针留作回归
  `tests/multi_agent_security_probe.rs`）。F1/F2 与审查同源已闭合；
  **F3（句柄存在性预言）已修复**（归属不符与不存在共用统一文案，探针
  断言更新为不可区分，8/8 复验）；F4（正文行首标记伪造，模型层面）/
  F5（重放去重账本结构性等价未显式实现）/F6（旧二进制按坏行跳过事件，
  发布顺序注意）登记不修。
- 通过面（攻防实测）：23 组非法 target 零受理、提权字段注入全拒、邮箱
  洪泛第 33 条先拒、伪造 V3/V1/V2 事件 12/12 整拒、OSC/ANSI 展示过滤、
  AGENT_NOT_FOUND 统一文案、跨归属句柄操作全拒。

### 3.4 全量验证（M7，2026-10-29）

- 命令与结果（完整日志：`2026-10-29-m7-full-verification.log`，86 个测试
  目标，exit=0）：

```bash
env -u TCHAT_SANDBOX cargo check --locked                                        # 通过
env -u TCHAT_SANDBOX cargo test --locked --workspace --no-fail-fast
# 3854 通过 / 0 失败 / 3 既有忽略（相对 034 交付基线 3811/0/3 净增 43：
#   multi_agent 20、multi_agent_security_probe 8、prompt_assembly s20 1、
#   lib 内 agents 模块单测 14）
```

- 锁文件（Cargo.lock/Cargo.toml/src/app/Cargo.toml）与 main 一致——零依赖
  变更（sha2 沿用 034 既有依赖）。
- 最终内容哈希：`2026-10-29-final-hashes.txt`（21 个代码/测试文件＋3 锁文件）。

## 4. 验证缺口（如实登记；2026-10-29 收尾轮更新）

- ~~真实 bwrap 沙箱矩阵~~ **已补**（收尾轮）：`multi_agent::c13_bwrap_owner_
  reap_matrix`（子进程模式，033 先例）——真实 bwrap 下子 exec 进入 PID ns
  （ns 内 pid=1）、运行中句柄按 assignment 登记、终态按宿主 PGID 回收且
  /proc 扫描无残留；无 bwrap 环境输出 skip 原因跳过。PTY 形态/64 总容量
  压满/共享 MCP 并发下的回收仍为已登记边界（规格 §3.3）。
- ~~性能基线（C18）~~ **基线已冻结**（收尾轮，[c18-baseline.md](c18-baseline.md)）：
  A1 8 Agent 满容量 p50 15/p95 31ms；A2 3 子屏障并发 p50 9/p95 48ms；A3
  24 请求＋256 调用压满 p50 78/p95 91ms；峰值 RSS Δ 6.9–14.1 MiB；单迭代
  会话磁盘 33–129 KiB；请求数按脚本断言。未设通过线；fork 分形/分项计时/
  大历史追加/真实模型联测/费用未覆盖（规格 §1.8 口径）。
- **E2E（PTY 全链）**：035 未改 REPL 基本行为；五协议根→子全链在集成层
  验证（native_state 归属，C16 子集）。PTY 级 CLI 验收未扩充。
- 组消息预算在并发 send 竞态下存在有界过收窗口（检查与入账非同锁段，
  ±4 条量级；不先 accepted 后丢弃，超出即拒绝后续）——首版已知边界。
- F4/F5/F6（安全攻防登记项）：正文行首标记伪造属模型层提示注入面（宿主
  边界不受影响）；§1.4 重放去重账本以结构性等价实现（响应恰解析一次、
  事件恰提交一次、恢复不重放）未建显式账本；旧二进制装载含 agent_event
  的 V3 按坏行跳过——升级发布同批部署新二进制。

## 5. 文件清单

代码：`src/app/src/agents/{mod,args,registry,mailbox,events,runtime}.rs`（新增）、
`src/app/tests/multi_agent.rs`（新增）；接入修改：`turn.rs`、`session.rs`、
`prompt.rs`、`catalog.rs`、`proc_session.rs`、`main.rs`、`tui_host.rs`、
`lib.rs`、`api/native.rs`、`api/native/{anthropic,gemini}.rs`、
`memory/rollout_input.rs`；测试适配：`context_management.rs`、
`message_to_user.rs`、`mcp_{http,resources,turn}.rs`、`memory_phase2.rs`、
`prompt_assembly.rs`（＋s20）。
