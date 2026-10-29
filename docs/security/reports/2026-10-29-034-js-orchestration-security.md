# 034「JavaScript 工具编排」安全攻防测试报告

- 日期：2026-10-29
- 范围：`feat/034-javascript-tool-orchestration` 分支已实现的 034 功能——`src/app/src/js_orchestration/`（args/protocol/worker/runtime）与 `turn.rs`／`session.rs`／`prompt.rs`／`memory/rollout_input.rs` 集成层
- 方法：先代码审查后真打——审查实现与规格 §1.7/§2 的偏差，再以真实 worker 子进程、假 worker 恶意帧、伪造会话文件、直连桥接原语、E2E 真实 CLI＋本地 mock 逐项攻击；普通模式运行（`env -u TCHAT_SANDBOX`）
- 测试代码：`src/app/tests/js_orchestration_security.rs`（18 用例，独立 TempDir，env 读写共锁）

## 一、结论

| 项 | 数量 |
|---|---|
| 本轮新增攻击用例 | 18 |
| 通过 / 失败 | 18 / 0（修复后） |
| 确认漏洞（本轮窗口内） | 4（1 项本轮修复，3 项同窗口并行审查流已修复；均保留回归用例） |
| 受影响既有套件复验 | js_orchestration 28、js_orchestration_perf 7、exec_sandbox 7、apply_patch_sandbox 2、tool_loop 18、session 40、prompt_assembly 20 —— 全部通过 |

命令（工作目录 /workspace）：

```bash
env -u TCHAT_SANDBOX cargo test --locked --test js_orchestration_security
env -u TCHAT_SANDBOX cargo test --locked --test js_orchestration --test js_orchestration_perf
env -u TCHAT_SANDBOX cargo test --locked --test exec_sandbox --test apply_patch_sandbox --test tool_loop --test session --test prompt_assembly
```

注意：测试窗口（16:40–17:32）内存在并行开发/审查流对同一批文件的修改（prompt.rs、turn.rs、runtime.rs、worker.rs，标注「审查 P1-x/P2-x」）。下述发现 A/B/C 的修复来自该并行流；本轮源码修改仅一处（发现 D 的 fd 收口）。发现 A/B 的时点证据为本轮 16:40 前后对源码的直接读取记录。

## 二、发现与修复

### D（中）——worker 继承 fd 泄漏：宿主进程 ≥3 的非 CLOEXEC fd 进入 worker 【本轮修复】

- 违反边界：规格 §1.6「始终 env_clear、**关闭无关 fd**、独立进程组」；实现只有 env_clear＋process_group＋RLIMIT，无 fd 收口。
- 复现：测试进程用 `libc::open`（无 O_CLOEXEC）打开哨兵文件 → `spawn_cell` 启动挂起 cell → 读 `/proc/<worker-pid>/fd`：哨兵路径以 fd 3 出现在 worker 内（红例输出：`继承 fd 泄漏进 worker：Some(("3", ".../fd-sentinel.txt"))`）。env 哨兵同用例验证 env_clear 生效（`/proc/<pid>/environ` 无宿主哨兵，绿）。
- 影响：worker JS 无文件/进程 API，不能直接读 fd——但泄漏 fd 违反冻结边界，与未来任何 worker 逃逸链组合即成凭证/描述符外泄面（例如模型目录句柄、连接 fd）。
- 修复（本轮）：`runtime.rs spawn_worker` 的 `pre_exec` 在 setrlimit 前调用 `close_range(3, ~0)`（syscall 436；libc 0.2.189 未导出常量，x86_64/aarch64/riscv64 通用表一致；fork 后 exec 前 async-signal-safe 上下文不能用 opendir 兜底）。best-effort：ENOSYS（内核 <5.9）或其他架构按既有 CLOEXEC 纪律继续，不阻断启动。
- 复验：`env_and_fd_sentinels_absent_from_worker` 红→绿；bwrap 包装分支（`js_orchestration` 套件 28 用例含真实 bwrap 模式与包装失败模式）全部通过——收口对 033 包装无副作用。

### A（高）——编排事件被投影为模型用户消息（上下文注入通道）【同窗口并行修复】

- 时点证据：16:40 前后 `prompt.rs project_history` 的 `OrchestrationEvent` 分支为
  `out.push(json!({"role":"user","content":"MUTATION-orch-event-leak"}))`——每条已入史事件（cell_started/call_started/call_finished/cell_closed）在下一次模型请求中各渲染为一条注入用户消息，违反 §1.6「所有模型投影在原生批次配对之前过滤事件」。
- 攻击影响：其一，正常轮内每次 exec/wait 观察都向模型上下文注入伪 user 行（污染＋膨胀）；其二，能写会话文件的攻击者构造**合法事件链**（seq 连续、start 开头、至多一次 close）即可通过 `validate_orchestration_chain`，随后每条伪事件都成为模型可见的注入消息——V3 装载校验只保证链形，不构成写授权。
- 修复（并行流，17:14 落盘）：事件分支改为仅 flush、不产出消息（与 ContextCheckpoint 同构）。
- 回归用例：`e2e_events_never_enter_model_projection`（真实 CLI：exec 完成后下一请求的消息面断言——无事件字段形态、user 消息仅本轮输入与环境上下文）。

### B（高）——每逻辑用户轮 128 叶子额度被第二次 exec 重置【同窗口并行修复】

- 时点证据：16:40 前后 `turn.rs execute_orchestration_call` 的 exec 分支每次 `Arc::new(AtomicUsize::new(TURN_LEAF_QUOTA))`——cell 消费（wait 取回/终止）后同轮新 exec 获得全新 128。
- 攻击影响：`exec(31 叶子) → wait 消费 → exec …` 循环即可让单逻辑用户轮执行 31×N 个叶子，128/轮上限失效（§1.5「请求数、context_id 变化和错误重试都不能重置这些计数」）。
- 修复（并行流，17:32 落盘）：预算上移至 `run_turn` 轮作用域（`turn_leaf_budget`，审查 P1-2），跨同轮全部 cell 共享。
- 回归用例：`e2e_turn_leaf_quota_not_reset_by_second_exec`（真实 CLI：同轮 5 个 cell，4×31＋第 5 cell 第 129 帧起必须 TURN_QUOTA）。

### C（中→低残余）——叶子受理额度次序：错误参数／资格拒绝不占额度【同窗口并行修复＋残余说明】

- 时点证据：16:40 前后 `runtime.rs accept_call` 的次序为 参数形态 → 大小 → 冻结目录资格 → 扣额，注释自述「此处资格已过再扣」——目录外试探帧零成本，可无限探测冻结目录/桥接面，31/128 受理上限只约束「合格」调用。
- 修复（并行流，17:32 落盘，审查 P2-5）：轮额度（128）改为帧可识别即**先行原子扣减**，错误参数/资格拒绝不回补。
- 残余（Low，记录不修改）：cell 内 31 名额仍只计「资格通过」的调用——目录外试探不消耗 cell 名额。但每逻辑轮 128 帧硬上限（含全部被拒帧）已从总量上封死探测面（每轮最多 128 个可识别帧，第 129 帧即使资格满足也零执行）；规格 §1.5「错误参数、资格拒绝…也消耗该名额」中「该名额」覆盖 cell 名额的字面读法与实现存在解释差，建议在规格或代码注释中明确「cell 31 计合格受理、轮 128 计全部可识别帧」的口径。
- 回归用例：`quota_charged_for_invalid_args_and_unknown_tools`（`__dispatch` 直发 100 伪帧→合格调用仍执行；第 128 帧执行后第 129 帧静默试探＋合格观察均 TURN_QUOTA 零执行）。

### 非漏洞的验证结论（记录）

- 装载层「头部非 user 丢弃」（`repair_pairs` §6-F）会把无 user 前缀的纯事件文件整体剥离且不计 skipped——这不是伪造链绕过（事件未进入历史、不产生任何投影/记忆面），但说明事件链校验的有效前提是文件含真实转录头；伪造事件若要存活必须位于 user 行之后，那里链校验整文件拒绝。首轮红例（伪造链「装载成功」）源于测试夹具未含 user 头，已修正用例。
- V1/V2 携带事件、V3 损坏事件行、seq 断裂/重复/孤儿 finish/闭环后再事件/exec_call_id 变更/参数与结果容量越界/截断缺哈希——全部整文件拒绝（`forged_orchestration_events_rejected_on_load`，12 个子案）。
- 事件不进记忆提炼（源码不泄漏、外层 exec/wait 控制调用剔除、叶子事实恰一次合成）：`memory_extraction_filters_orchestration_shell`。

## 三、用例清单与攻击面覆盖

| # | 用例 | 攻击面 | 结果 |
|---|---|---|---|
| 1 | forged_tool_names_and_dispatch_bridge_zero_execution | 越权：伪造 exec/wait/条件宿主工具名、`__`拆名、大小写、路径形、`__proto__`；`__dispatch` 直发帧夹带伪造批准 JSON；包装层不存在的名称连帧都形不成 | 通过（伪造零执行；批准字段仅为普通参数） |
| 2 | catalog_surface_frozen_and_minimal | 目录面：tools/ALL_TOOLS 冻结不可写、无原型、条目仅 name/description/parameters 且深冻结、内部 pending 表不进全局 | 通过 |
| 3 | env_and_fd_sentinels_absent_from_worker | 凭据：env_clear 哨兵＋继承 fd 哨兵（/proc 实证） | fd 泄漏→修复后通过 |
| 4 | fake_worker_unknown_frame_closes_cell_host_survives | IPC：未知帧类型（伪 escalate 字段）→ cell 关闭、主进程存活 | 通过 |
| 5 | fake_worker_oversize_frame_closes_cell | IPC：长度前缀声明 >1 MiB 超长帧 → 有界拒绝、零执行 | 通过 |
| 6 | fake_worker_bad_json_frame_closes_cell | IPC：坏 JSON 帧 → 断开 | 通过 |
| 7 | fake_worker_forged_call_frames_zero_execution | IPC：恶意 worker 伪造 6 类调用帧（含 apply_patch 伪目录外资格＋伪造沙箱豁免字段）→ 宿主受理门零执行 | 通过 |
| 8 | pending_output_overflow_marks_truncated | 输出洪泛：单次 text 超 64 KiB 待交付缓冲 → 丢弃＋output_truncated，cell 存活 | 通过 |
| 9 | total_output_flood_closes_cell_host_continues | 输出洪泛：累计 >1 MiB（200×60 KB）→ OUTPUT_LIMIT 关闭，后续 cell 正常 | 通过 |
| 10 | quota_charged_for_invalid_args_and_unknown_tools | 额度：试探帧占轮额度、128/轮硬上限、第 129 帧零执行 | 通过（当前语义） |
| 11 | cpu_limit_kills_spin_worker_host_continues | 资源：`for(;;){}` 被 RLIMIT_CPU 5 秒回收（<18s 观察期内），主进程继续 | 通过 |
| 12 | parser_rejects_type_coercion_and_pragma_probes | 绕过前端直打解析器：浮点/布尔/字符串冒充整数、pragma 前缀、未知字段、JSON 语法走私按原文精确匹配 | 通过 |
| 13 | forged_orchestration_events_rejected_on_load | V3 伪造事件：12 种断裂/容量/旧版本/损坏行全部整文件拒绝（含合法链对照） | 通过 |
| 14 | forged_chain_with_unknown_outer_call_stays_inert_in_projection | 伪造链装载后惰性：不复活执行入口、不进提炼投影外壳 | 通过 |
| 15 | memory_extraction_filters_orchestration_shell | 记忆：源码/外层 exec/wait 不进提炼，叶子事实恰一次 | 通过 |
| 16 | e2e_events_never_enter_model_projection | 模型投影零事件泄漏（发现 A 回归门） | 通过 |
| 17 | e2e_turn_leaf_quota_not_reset_by_second_exec | 轮额度不被第二次 exec 重置（发现 B 回归门） | 通过 |
| 18 | e2e_cell_id_guess_uniform_not_found | cell_id 猜测：有/无活动 cell 拒绝文案一致、cell_id=null、不泄露存在性；未消费句柄终文必须轮错误 | 通过 |

## 四、残余风险与建议

1. cell 名额口径（见发现 C 残余）：建议规格/注释明确「cell 31 计合格受理」，或把 31 也改为「帧可识别即扣」，二者取一并留档。
2. fd 收口为 best-effort（内核 <5.9 或非三大架构不强制）：033 已启用默认部署环境为现代内核，可接受；若未来要求硬边界，可在 ENOSYS 时按策略失败启动（需先确认部署基线）。
3. 未覆盖（超出本轮范围，建议后续）：bwrap 模式下的同批哨兵深测（当前仅普通模式实测 fd/env 哨兵；bwrap 分支经既有套件验证功能不回退）、031 动态授权与编排 cell 生命周期的交叉攻击、022/023 业务工具自身权限面（属各自编号的安全轮）。
4. 并行修改窗口提示：本轮与审查流（P1/P2 标记）同窗口工作，`runtime.rs` 同文件双写——建议合并前以全量 `cargo test --workspace --no-fail-fast` 做最终收敛验证（AGENTS.md 第 8 节）。

## 五、本轮改动清单

- 新增 `src/app/tests/js_orchestration_security.rs`（18 用例）。
- 修改 `src/app/src/js_orchestration/runtime.rs`：`spawn_worker` pre_exec 增加 close_range fd 收口（发现 D 修复，约 20 行）。
- 报告：本文件。
