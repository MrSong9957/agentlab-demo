# 033 沙箱进程生命周期修复（2026-10-28，已交付——任务 `a3aef1c`、合并 `dc9ef7b`、
# 正式二进制 `713e4c28…`，部署收口见 2026-10-28-lifecycle-deploy.md）

**范围**：修复沙箱进程绑定短寿轮线程的生命周期缺陷（恢复 016 取消/yield/跨轮保留
契约）＋修正 s103 回收测试。**与 B1/B2 已交付成果、部署产物（正式二进制
`4ab2dcc1…` 未替换）零交集**。归因依据：
`docs/logs/033-tool-sandbox/2026-10-28-e2e-failure-attribution.md`。

## 1. 最小设计

**根因**：bwrap `--die-with-parent` 的 PDEATHSIG 锚定「创建者内核线程」；exec 面
spawn 原先在随轮退出的 turn 线程内执行（tui_host 每轮 spawn 轮线程），轮结束＝
线程退出＝整树 SIGKILL——016 的跨调用/跨轮保留、取消不终止在沙箱模式全部失效。

**修复（与上游 issue #46702 报告者的候选方案同向：持久化 OS 线程执行进程创建）**：

1. **exec 面（proc_session.rs）**：ProcessManager 专属**长寿创建线程**
   （`tchat-proc-spawner`）执行 fork/exec（spawn_piped/spawn_pty 原函数整体移入）。
   PDEATHSIG 锚点＝管理器生命周期（全部克隆 Drop／进程退出才触发——宿主异常退出
   仍保证载荷停止执行，die-with-parent 保留）。仅**串行化创建本身**（std mpsc
   请求/回复通道，请求方持管理器锁等待——既有顺序，spawn 本就持锁）；工具执行、
   监控、读取照旧按会话并发。创建线程不取任何管理器锁（无死锁环）。关闭顺序：
   `close()` 先组杀回收，创建线程随通道关闭（全部克隆 Drop）自然结束——先回收、
   后结束。线程创建失败＝通道无接收端，`start()` 的 send/recv **显式报错**
   （fail closed，不静默回退调用线程）。
2. **MCP stdio 面（mcp/connection.rs）**：`StdioProcess::spawn` 经 manager 专用
   runtime 的 `tokio::spawn` 移交**核心 worker 线程**执行（多线程 runtime 的常驻
   线程，不随任务退休——**区别于 ~10s 空闲退休的 blocking 池**，后者正是上游
   issue 的触发机制，goal 明令禁用）。PDEATHSIG 锚点＝manager runtime 生命周期。
   Tokio 上下文核实：manager 持独立 `Runtime`（manager.rs:209），原 block_on 内联
   spawn 在**调用者（轮）线程**执行——同病，故需移交。
3. **apply_patch 面（apply_patch.rs）**：子进程**同步完成于调用内**（spawn 后等待
   结果 JSON），不跨轮存活——PDEATHSIG 锚定无实际影响，**不接入**（避免无关重构；
   注释已注明同步语义）。
4. **s103 测试修正（e2e_acceptance.rs）**：沙箱注入态下服务器记录的 pid 是 ns 内
   pid（恒 1）——存活/回收判定切换为**外层 argv 扫描**（`scan_arg_procs`：bwrap
   包装与 python 载荷均含 fixture 路径；僵尸进程 cmdline 为空天然排除，不把僵尸
   计为泄漏，不以 /proc/1 为据）；普通模式保留原 pid 断言。

**安全配置不变**：`--die-with-parent`、`--unshare-user/pid/ipc`、`--as-pid-1`、
`--new-session`、`--cap-drop ALL`、`--proc /proc`、B2 pre_exec CLOEXEC 标记、B1
nlink/元数据三分类、env 白名单、fail-closed 全部原样（B2 的 pre_exec 随 spawn 函数
移入创建线程执行——fork 时生效，线程无关）。

## 2. 上游来源与本地适配

- 参考：openai/codex **issue #46702**（2026-10-28 核对）。上游事实：①机制同款
  （"A Codex OS thread creates a sandbox helper, which successfully arms
  PR_SET_PDEATHSIG… The helper immediately receives SIGTERM"；"Linux ties that
  signal to the lifetime of the creating OS thread"）；②触发器为 Tokio blocking
  线程 ~10s 空闲退休；③**未合入**（issue Open，0.155.1/`be2951e`、main `595cc91`/
  `444da31`、0.159.2、0.162.1 均核查存在）；④issue 内方案均为本地候选：报告者的
  持久 spawn 线程（与本修复同向）、lywaterman 的「对管道子进程去掉 PDEATHSIG＋
  pidfd/subreaper 兜底」（**不采纳**：削弱宿主异常退出的清树保证）、第三方 MCP
  wrapper 的 Python 监督进程（**不采纳**：引入外部组件；且上游 helper 架构本身
  也带此 bug——「存在 helper」不构成生命周期正确依据）。
- 本地适配差异：上游 spawn 点在 Tokio worker/blocking 池；本项目 exec 面为纯
  std 线程模型 → 采用 std mpsc＋专属 `std::thread`（无退休风险）；MCP 面为
  自持 runtime → 采用 `tokio::spawn` 移交核心 worker。许可证：未复用上游代码
  （设计方向参考公开 issue 讨论），无许可义务引入。

## 3. RED → GREEN

- **RED（修复前，沙箱注入态，`…-attr`/2026-10-28-post-merge-suites.log＋新增
  回归首轮运行）**：新增 `src/app/tests/sandbox_lifecycle.rs` 四用例——
  `lifecycle_pipe_session_survives_turn_end_and_stays_queryable`（:162 轮结束后
  写盘必须继续＝FAILED）、`lifecycle_pty_session_survives_turn_end_and_accepts_
  input`（:244 跨轮输入未送达＝FAILED）、`lifecycle_mcp_server_survives_turn_end_
  and_reload_reaps`（:291 外层扫描空＝FAILED）、`lifecycle_sandbox_payload_stops_
  after_cli_sigkill`（保底保证，修复前即过——树早已随轮死）。env -u 口径 4/4 绿
  （普通模式不受缺陷影响）。既有 RED：n2b/n2b2 部署态失败
  （2026-10-28-post-merge-suites.log）。
- **GREEN**：实现后沙箱注入态 4/4 全绿；n2b/n2b2 沙箱注入态通过（原关键断言满足：
  取消后写盘继续、批内跳过、无续请求、下一轮、退出清理）；env -u 对照 4/4。
  修复前后对照即敏感性证明（未修复态＝注入缺失态，断言未动）。

## 4. 双模式与全量结果

| 项 | 沙箱注入态（TCHAT_SANDBOX=1） | 普通模式（env -u） |
|---|---|---|
| 新增回归 sandbox_lifecycle | 4/4 ✅ | 4/4 ✅ |
| n2b/n2b2 | ✅（修复前 FAILED） | ✅ |
| s103 | ✅（外层 argv 判定） | ✅（原 pid 断言） |
| 沙箱专项 6 目标（matrix/e2e_sandbox/lifecycle/三型） | 27/27 ✅ | — |
| 完整 e2e_acceptance | **143/143 ✅**（修复前 140/3；审查修复后冻结态复跑 ✅） | **143/143 ✅** |

**独立驱动交叉验证**（python attr_drive，与 Rust 测试架不同实现）：修复后
取消契约 17 项观测全过——取消后写盘 8→24 继续、写入进程/sh/bwrap 存活、退出
清理无遗留（修复前同架 17 项中核心 5 项 FAIL，见归因记录 §2）。

**标准验收口径最终全量（冻结后一次）**：PTY 200x50＋TERM=dumb＋`env -u
TCHAT_SANDBOX`＋独立 target `/tmp/tchat-lc-target`＋check/test 串行＋
`--locked --offline`：`cargo check && cargo test --workspace --no-fail-fast
--locked --offline` → **exit 0；71 目标 3533 passed / 0 failed / 3 ignored**
（3529＋4 新增；零失败）。日志：`2026-10-28-lifecycle-final-full-pty.log`、
`-lifecycle-e2e-sandbox-full.log`、`-lifecycle-e2e-env-u.log`。

## 5. 安全与审查

- 审查（code-review 技能，独立子代理，high，48 次工具调用含实跑取证）：**生产代码
  改动（长寿创建线程、tokio worker 移交、kill_on_drop 收口）经锁序、取消窗口、
  runtime 上下文、spawn-登记原子性核查未发现可确认缺陷**。5 项发现全部集中测试
  有效性/文档，已全部修复：①T4（SIGKILL 场景）扫描标记 `lc-kill-{pid}` 恒不在
  cmdline（TempDir 命名 pid 在 tag 前）＝断言空转，普通模式 2.4s 通过即实证——
  标记改每测试唯一 work 路径，并重构为「先证存活→SIGKILL→三分类（活/僵尸/消失，
  活=失败；僵尸=已死未收割不计泄漏）」，敏感性验证：移除 `--die-with-parent` →
  FAILED 并精确指出仍存活 pid ✓；②T1 退出清理断言同病同修；③s103 全局 fixture
  标记被并行 MCP 用例污染（假失败/假通过）——并入每测试唯一 record 路径（AND）；
  ④代码注释的归因文档悬空引用——随本分支入库；⑤T2 冗余三连等待与 no-op——清理。
  审查中发现并补强（审查者独立实跑触发窗口排查）：**transport spawn 移交分离任务
  后，连接建立被取消/超时丢弃时已启动进程会孤儿化（tokio 默认 kill_on_drop=false，
  既有注释「句柄 drop＝进程组后备回收」的语义此前并不成立）**——`kill_on_drop(true)`
  落地使「spawn 成功但登记失败仍有明确所有者并能回收」成立（connection.rs 注释）。
- 重点自查（goal 三）：①取消/接收端消失：cancel flag＋会话持有不变（n2b 绿）；
  spawn-登记窗口与修复前等价（start 持锁内 spawn→登记，panic 窗口微秒级且为
  既有语义，已在审查范围）；②启动线程仅创建线程退出条件＝全部克隆 Drop；
  错误经回复通道传播为显式 ExecResult::error；锁序：请求方持管理器锁等待、
  创建线程零取锁，无死锁环；③stdio/PTY/进程组/退出原因/超时契约：spawn 函数
  整体移入创建线程，pre_exec/process_group/管道结构逐字节未动（monitor/reader
  线程不变）；④B1/B2/白名单/fail-closed：三型 fail-closed 套件（f_fail_closed）
  沙箱注入态全绿。
- clippy：`--lib --tests` exit 0；本轮改动文件零新增告警（proc_session:538/544
  两条为存量 thread_local 文档注释 lint，行号因新增平移，不在本轮 diff 中）。
- 审查修复后重跑：lifecycle 双口径 4/4、MCP 家族 6 目标 117/0、s103 双口径、
  变异敏感性（--die-with-parent 移除→FAILED 指认存活 pid）、最终全量重跑（见 §4，
  数字不变 3533/0/3）。

## 6. 剩余边界与状态

- 载荷**运行期自开**进程/线程仍不受 die-with-parent 之外的生命周期管理（沙箱 ns
  内部进程树语义不变）；MCP 服务器进程生命周期现＝manager runtime 生命周期
  （reload/宿主关闭显式回收，宿主异常退出经 die-with-parent 停止）。
- apply_patch 面未接入创建线程（同步语义无需），已在代码注释注明。
- **已交付**（2026-10-28）：任务提交 `a3aef1c`、合并 `dc9ef7b`、正式二进制
  `713e4c28…`（部署后验证 22＋32 PASS/0 FAIL＋取消契约新旧对照矩阵），收口见
  [部署记录](2026-10-28-lifecycle-deploy.md)；任务分支与工作树暂时保留。
