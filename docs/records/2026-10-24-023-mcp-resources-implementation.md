# 023 MCP 资源接入用户轮——实施记录

- 日期：2026-10-24（M7 终验门禁于 2026-10-25 容器重启后收口，见 §4）。分支：`feat/023-mcp-resources`（基线与合并目标：本地 `main` @ `ad79592`）。
- 规格：[023-mcp-resources](../specs/023-mcp-resources.md)（契约 §5.1，R1=A／R2=B／R3=A 用户定稿；M0 记录 §5.5；预测→实测 §5.6）。
- 安全报告：[2026-10-24-023-mcp-resources-security](../security/reports/2026-10-24-023-mcp-resources-security.md)。
- 方法：TDD（每切片 RED→GREEN）→ comprehensive-testing → Security Testing → code-review 双轴 → 全量验证；变异敏感性证明 7 处。

## 1. 变更清单

| 文件 | 变更 |
|---|---|
| `src/app/src/mcp/resources.rs` | **新增**：固定三工具定义、参数校验、定向列举/模板/读取、跨服务器汇总（完整分页、R1 单一整体时限、R3 台级 errors、8 MiB 预算、逐字段污染检查含 base64 解码字节）、失败脱敏 |
| `src/app/src/mcp/connection.rs` | 能力判断改「tools 或 resources」；连接冻结 `capabilities`；`list_resources_once`／`list_resource_templates_once`／`read_resource_once`（单次请求句柄＋超时/取消，`resource_request_once` 公共骨架）；`CallFailure` 实现 Display |
| `src/app/src/mcp/manager.rs` | 快照纳入资源服务器条目（发布时按 ID 排序，执行共用快照）；`call_resource`；仅资源服务器不发 `tools/list`；prepare 每轮同步断连真值；`/mcp` 资源能力状态行；脱敏与断连标记共用入口 |
| `src/app/src/mcp/secrets.rs` | `contains_secret_bytes`（blob 解码字节检查）＋`sanitize_detail` 共用脱敏 |
| `src/app/src/turn.rs` | 固定资源工具分派分支（合并轮经既有 `mcp_snapshot` 置 None 隔离） |
| `src/app/src/mcp/mod.rs` | 声明 resources 模块＋文档头 |
| `src/app/tests/mcp_resources.rs` | **新增**：36 个用例（m1/m2/m3/m4/m5/m6/sec/review）＋2026-10-25 R1 窗口组合补证 1 例（现共 37） |
| `src/app/tests/mcp_http.rs` | 新增 023 资源面 JSON＋SSE 用例（2） |
| `src/app/tests/e2e_acceptance.rs` | 新增 s105（资源工具展示＋恢复零重放） |
| `src/app/tests/common/mcp_fixtures.rs` | `setup_two`（双服务器）；`TestCtx::Drop` 兜底（panic 时也回收连接与 hang 夹具子进程） |
| `src/app/tests/fixtures/mcp_stdio_server.py` | resources 能力协商＋`resources/list`（分页/容量/污染/畸形场景）＋`templates/list`＋`read`（多段/blob/污染/慢/死场景） |
| `src/app/tests/mcp_stdio.rs` | 1 处文案断言更新（`未声明 tools 能力`→`未声明 tools 或 resources 能力`，拒绝行为保留） |
| `docs/specs/023-mcp-resources.md` | §5.1.1–§5.1.4 契约固化＋R1–R3 定稿＋§5.5 M0＋§5.6 实测 |
| `docs/security/reports/2026-10-24-023-mcp-resources-security.md` | **新增**：安全报告 |

## 2. 关键设计决定（实现顺序中固化）

1. **固定工具名**：`list_mcp_resources`／`list_mcp_resource_templates`／`read_mcp_resource` 为原生样式固定名（非 `mcp__` 动态名），按资源能力条件声明，追加在动态 MCP 工具之后；声明与执行一致（实现一个声明一个，M1 起逐步开放）。
2. **汇总结果形态**：`{"resources":[{…,"server":"fx"},…], "errors":[{"server","error"}]?}`——每项携带服务器归属（模型后续 read 直接取）；模板汇总同构（`resourceTemplates`）。无错误省略 `errors`（R3）。
3. **R1 实现**：进入处理器即 `deadline = now + max(参与台 tool_timeout)`；页请求以「到 deadline 的剩余」为限；请求失败先判取消/整体时限再归类单台失败（`failure_into_abort`）。
4. **污染检查分层**：read 走 `contents_polluted`（text/blob 原文＋base64 解码字节＋`_meta` 逐值）；列举/模板/汇总走 `value_polluted`（投影 JSON 递归字符串叶子原文——修复审查发现的「序列化转义漏检」）；命中一律整次失败替换标准客户端错误。
5. **执行共用快照**：`call_resource` 使用调用方快照的资源列表（审查修复：轮内断连台保留在快照，按快照路由返回不可用/记 errors，不静默剔除、R1 基数不变）。
6. **prepare 每轮同步断连真值**：成功调用后死亡的进程在下一轮 prepare 即被发现并移出发布（022 §2.2「确认断开不声明」的实现缺口，023 断连契约所需；Spec 审查标注为顺带修复 022 行为，已明示并保留）。

## 3. 测试与证据

- **新增用例 36（mcp_resources）＋2（mcp_http）＋1（e2e s105）＝39；2026-10-25 交付证据收口补 R1 窗口组合 1 例（mcp_resources 共 37、全计 40）**；全部随 `cargo test --workspace` 运行。
- **RED→GREEN**：M1（连接/声明、真实链路）、M2（模板、读取、blob 污染——RED 为真实安全缺口）、M3（汇总 8 例）、M5（/mcp 状态）均先失败后通过。
- **变异敏感性（10 处，全部恢复并复验）**：分派断开；未知服务器绕过；记忆轮隔离绕过；半份目录输出；汇总污染检查移除；取消降级为部分失败；blob 字节检查（即 M2-3 RED）；断言放宽后复证（2026-10-25：`call_resource` 临时改为按实时断连状态剔除后再路由，`review_execution…` 准确失败于「断连台不得被静默剔除：…未知或未启用的资源服务器：fx」；恢复后单用例通过——放宽后的断言仍识别原缺陷，未削弱规格「执行共用快照」要求）；R1 窗口组合双变异（2026-10-25：`execute_aggregate` 的 `max()`→`sum()` 致文案「整体时限 5s」被 `m3_aggregate_deadline_window_is_shared_max_not_per_server` 文案数值断言捕获；页请求超时「到 deadline 剩余」→各台自身 timeout 致部分结果（fy 数据＋fx errors）而非整次失败被 isError 断言捕获——max 组合、每台重计时、求和三种语义均被该用例区分）。
- **双轴审查发现与处置**：
  - Spec-1 列举/模板污染仅扫序列化字符串（转义漏检）→ 修复＋`review_list_pollution_catches_escaped_secret`（含引号/反斜杠 canary，RED→GREEN）。
  - Spec-2 执行未共用快照（断连台静默剔除）→ 修复＋`review_execution_uses_snapshot_so_disconnected_server_reports_unavailable`（RED→GREEN）。
  - Spec 观察：prepare 断连同步属顺带修复 022 缺口（保留，见 §2.6）；汇总页间不查 cancel 与 022 分页同模式（页请求发出后立即取消返回，可接受）。
  - Standards-1 mod.rs 文档头失真 → 已修；重复代码（脱敏×3、ResourceServerEntry 构造×2、断连标记×2）→ 已合并/消除；`resource_request_once` 与 `call_tool_once` 边界复制与 SDK 类型差异分支保留（判断性接受）。
  - Standards 观察：s105 `sleep(300ms)` 时序否定断言与 022 s104 同构（既有先例，保留）。

## 4. 验证命令与结果

| 命令（容器 `/workspace`） | 退出码 | 结果 |
|---|---|---|
| `cargo check` | 0 | 通过（既有警告，无新增；最后一次成功在 TestCtx Drop 兜底落地后） |
| `cargo test --workspace --no-fail-fast`（双轴审查修复后、末尾两处改动前） | 0 | **3443 通过／0 失败／3 忽略**（023 前 3406→3443） |
| `cargo test --workspace --no-fail-fast`（review 断言简化＋Drop 兜底后第一次复验） | 101 | 3444 通过／1 失败／3 忽略——失败为 `review_execution…` 在并发下的传输错误文案形态（断言过窄，已放宽为「不含未知或未启用＋客户端错误」，放宽后单用例 3 连跑通过） |
| 其后多轮全量复验 | 101 | 72／177／134 个失败，全部为 `EAGAIN`（PTY/线程 spawn 失败），分布于 e2e／proc_session／user_input_security 等与 023 无代码交集的目标；单跑均通过。根因：容器累积 **16,000+ 僵尸进程**（孤儿被 reparent 到 PID 1，而 PID 1 是 `sleep`，不收割），内核资源耗尽致 rustc／rust-lld／GNU ld 相继崩溃（ICE／signal 6）。清除增量缓存与更换链接器均未解决；唯一恢复途径是容器重启（清僵尸），超出本轮授权 |

定向（环境恶化前全部通过）：`cargo test --test mcp_resources`（37）／`--test mcp_stdio`（35）／`--test mcp_turn`（13）／`--test mcp_http`（13，含 023 两用例）／`--test e2e_acceptance s105`。

**终验状态（已关闭）**：最终候选（含审查修复、断言放宽、Drop 兜底）已于 2026-10-25 完成全量绿门禁（§「2026-10-25 M7 终验收口」），此前「全量绿证据未覆盖末尾两处测试侧改动」的缺口不复存在。

**环境事件处置**：已清理泄漏的 `mcp_stdio_server.py` 夹具进程并删除 `target/debug/incremental`（增量缓存疑似损坏）；新增 `TestCtx::Drop` 兜底（panic 时也回收连接与 hang 夹具子进程，防再泄漏）。僵尸进程无法由容器内清理（PID 1 不收割），需重启容器。

**2026-10-24 深夜补验（容器仍未重启）**：指令要求容器重启后补跑，但实测容器未重启（uptime 连续 3 天 18 小时＋，僵尸 16,670，容器内无 docker CLI／socket，无法自行重启）。本轮完成两层诊断与一层突破：

1. **编译链接层根因确认并绕过**：rust-lld 的 LLVM 线程池在本环境**确定性**崩溃（最小 hello world 链接 10/10 全崩，`std::system_error: Resource temporarily unavailable`→abort；rustc codegen 线程同因 ICE）；GNU 工具链健康（cc＋`/usr/bin/ld` 链接正常）。以 PATH 前置 `cc` shim（剥离 rustc 注入的 `-fuse-ld=lld`）绕过后：**`cargo check` exit 0**，`cargo test --workspace` 的全部 34 个测试目标**全部成功编译链接**（此前被 lld 崩溃阻断的编译层完全打通）。
2. **测试运行层仍被环境阻断**：全量高并发 446 失败、串行单线程（`-j 1 -- --test-threads=1`）仍 512 失败、442 处 EAGAIN——连不 fork 子进程之外的一切 spawn 全灭（mcp 系列目标 0 passed 启动即崩、测试内 `cargo metadata` fork 失败、cargo jobserver 线程启动失败、shell 自身 `fork: retry`）。小进程 Python 20 线程／30 fork 正常——判断为 **WSL2 VM 全局线程／任务资源耗尽**（容器内可见 task 16.7k，但 namespaced 计数与 VM 全局判定不一致），只能靠容器／VM 重启清除。
**第三次补验尝试（同夜，容器仍未重启，PID 1 启动时间不变）**：环境进入短周期振荡——「好相位」窗口仅数分钟（期间 bash／python fork、posix_spawn 全部正常，`cargo check` 再次 exit 0），但任意一轮全量测试的高并发 spawn 立即耗尽配额回到「坏相位」（446–556 个 EAGAIN 失败），串行单线程复验在启动阶段即被拒（cargo jobserver 线程 EAGAIN），且宿主侧执行 shell 与会话基础设施本身开始出现 `fork: retry`。**结论：每次全量重试都会制造新的孤儿进程→僵尸，主动恶化环境；已停止重试。**在容器重启（宿主机操作，容器内无 docker CLI／socket；该容器与 codex／opencode／另一 zcode 会话共享，重启会终止它们）之前，任何完整全量绿在本环境不可达；重启后按 §「恢复后操作」执行标准命令即可收口。

3. **恢复后操作**：容器重启后环境干净，rust-lld 底层线程 spawn 恢复正常，GNU ld shim（`/tmp/gnuld`，随重启消失，勿持久化）应当不再需要；直接在 `/workspace` 跑 `cargo check && cargo test --workspace --no-fail-fast` 即为最终门禁。若 lld 崩溃仍现（说明根因在工具链而非环境），再启用 shim 方案并另行上报。**→ 已于 2026-10-25 执行完毕，见下节。**

### 2026-10-25 M7 终验收口（容器重启后）

**环境预检**：僵尸 0（重启前 16,670）；fork 60 并发／20 子进程／6 路并行 rustc 编译均成功；默认链接器（rustc→`/usr/bin/cc`，无 RUSTFLAGS、无 cargo config 覆盖）单次＋并行压测均正常——**未启用 GNU ld 绕行**（`/tmp/gnuld` 仍在但未使用，实际配置为默认工具链；此结论仅覆盖本次运行，不代表此前 lld 崩溃根因已修，系环境重启后未复现）。

**末尾两处测试改动补审（code-review）**：

1. `review_execution…` 断言放宽（「不含未知或未启用＋含 MCP 客户端错误」）：变异证明仍识别原缺陷（见 §3 第 8 处变异）；放宽仅放弃具体传输错误子形态（未发送/状态未知/连接关闭，时序竞态下均合法），规格「执行共用快照、不静默剔除」语义由断言 1 完整守住——**未削弱规格要求**。
2. `TestCtx::Drop` 兜底：`manager.close()` 幂等（连接已被 take 后二次调用为空操作，显式 close 后 Drop 无副作用）；有界（stdin EOF→最多 3 s grace→SIGKILL 进程组＋wait 回收，另有 `StdioProcess::Drop` 组杀后备）；锁均带 poisoned 恢复无死锁；`Drop::drop` 先于 `_env` 释放执行，无跨测试干扰；TestCtx 仅 mcp_stdio/mcp_resources 使用，屏障线程/调用线程均不依赖其存活——**无阻塞、无他测影响**。全量负载下无夹具进程残留（`mcp_stdio_server.py` 存活数 0）。

**最终门禁（候选冻结后，两命令各自完整日志见 [docs/logs/023-mcp-resources/](../logs/023-mcp-resources/)）**：

| 命令（容器 `/workspace`，2026-10-25） | 退出码 | 结果 |
|---|---|---|
| `cargo check` | 0 | 通过（476 条警告全为 `src/shims`／`src/tui` vendor 层及 `src/app/src/prompt.rs:190` 既有警告，该文件不在 023 改动清单；无新增） |
| `cargo test --workspace --no-fail-fast`（第 1 次） | 101 | 3444 通过／1 失败／3 忽略（63 目标）。唯一失败 `proc_session::tests::seam_killpg_injection_deadline_not_fake_timeout`（lib 单测，016 @ `5f6a860` 引入；`proc_session.rs` 与基线 `ad79592` 零差异）：注入解除后监控线程对挂起 Deadline 请求的轮询重试与测试线程显式 terminate 竞速，两种终态归类均为产品如实行为，断言「terminate 优先」仅在测试线程先达时成立——全量并发负载下偶发反转。取证：隔离 3/3 通过、代码级机制定位、与 023 无代码交集（与 M0 记录的 `perf_tiers_baseline_driven` 负载抖动同类处置） |
| `cargo test --workspace --no-fail-fast`（第 2 次） | 0 | **3445 通过／0 失败／3 忽略**（63 目标；总数 3448 与第 1 次完全一致，无缺失/新增用例）——**全量绿，覆盖含末尾两处测试侧改动的最终候选** |

**运行后环境**：两轮全量遗留 369 个僵尸（359 sleep／8 sh／2 python3，全部 ppid=1，PID 1 `sleep` 不收割的既有容器限制）——**持续累积的环境风险**（该容器内每跑一轮全量都会新增）；此前约 16k 为 2026-10-24 发生进程耗尽当时的观测值，不构成固定安全阈值。

### 2026-10-25 交付证据收口（R1 补证＋lint 认定＋记录校正）

**R1 验收缺口澄清**：核心行为「共享整体窗口」原有同 timeout 双台用例 `m3_aggregate_respects_single_overall_deadline` 已钉住（整次失败／串行后续台零请求／不发布半份数据）；「跨台 max 组合」此前确无行为验证——同 timeout 下 max／每台重计时／求和三种实现均能通过原用例，属核心契约验证缺口而非普通剩余风险。已补 `m3_aggregate_deadline_window_is_shared_max_not_per_server`（fx 挂起 timeout 1s＋fy 4s）：错误文案数值「整体时限 4s」＋经时 ≥2 s 宽松下界＋fy 零请求三重结构断言区分三种语义，双变异证明（见 §3 第 9–10 处）；不做精确毫秒断言、不新增调度敏感性能测试。规格 §5.6 M3 行与 §5 遗留口径同步更新为「测试精度限制」。

**lint 门禁认定**：见 §5 lint 条目——`cargo check`（typecheck）为 AGENTS.md §7 已核实强制项并实测通过；clippy／rustfmt 工具链未安装（实测确认），非本环境强制交付检查，如实记录未执行。

**交付差异统计（校正口径）**：修改 10 个 src 文件（+867/−39：`mcp/{connection,manager,mod,secrets}.rs`、`turn.rs` 与 5 个测试文件）；**新增**代码文件 `src/app/src/mcp/resources.rs`（647 行）与 `src/app/tests/mcp_resources.rs`（1628 行，37 用例）；新增文档：规格（281 行）、本记录、安全报告（59 行）及终验日志（`docs/logs/023-mcp-resources/`，含 2026-10-25 两轮 M7 门禁与本轮 R1 补证共 5 份）。工作区另有他人改动（AGENTS.md、规格文档实战法、024 相关文档等）保留未动；`src/app/config/manual.json` 按指令未处理。

**最终完整验证（R1 补证候选，2026-10-25）**：`cargo check` exit 0（`2026-10-25-r1supp-cargo-check.log`，既有警告无新增）；`cargo test --workspace --no-fail-fast` **exit 0，3446 通过／0 失败／3 忽略（63 目标，恰为此前 3445＋新用例 1）**（`2026-10-25-r1supp-cargo-test.log`）。

## 5. 验证缺口与遗留

- **Windows ConPTY**：未运行（本环境 Linux/WSL2）；PTY 覆盖经 Linux node-pty 等价路径（e2e 既有设施）。
- **lint（非本环境强制门禁，如实记录未执行）**：AGENTS.md §7 的强制交付验证（「全量验证」行）为 `cargo check && cargo test --workspace --no-fail-fast`，typecheck 已含其中并实测通过；`cargo clippy`／`cargo fmt` 在 §7 明确记为「当前工具链未安装，用前需 `rustup component add`」——实测 `rustup component list --installed` 仅 cargo／rust-std／rustc，`cargo clippy --version` 报未安装。据此 clippy／rustfmt 非本环境已核实的强制交付检查，本次未安装、未执行（不写成已通过；安装并纳入门禁属环境与规范变更，超出本任务授权）。
- **R1 跨台组合为测试精度限制，非核心行为缺口**：同 timeout 双台用例 `m3_aggregate_respects_single_overall_deadline`（mcp_resources.rs:684）钉住共享窗口语义——整次失败（isError）、串行后续台零请求（record 恰 1 条 `list_resources`）、不发布已收集数据；跨台 max 组合由 2026-10-25 补证用例 `m3_aggregate_deadline_window_is_shared_max_not_per_server`（fx 挂起 1s＋fy 4s）结构化钉住：错误文案数值「整体时限 4s」区分 max 与求和（sum 变异实测 5s 被捕获）、经时 ≥2 s 宽松下界与 fy 零请求区分 max 与每台重新计时（变异实测部分结果被捕获）。剩余精度限制：不做毫秒级时长等值断言（AGENTS.md 测试规范禁止依赖调度敏感断言，有意为之）。
- **会话泄漏文件** `src/app/config/manual.json`（M0 记录的既有测试隔离缺陷）按指令不纳入本次交付、未修复。
- **规格顺序更新并存**：规格头部出现「先完成 024 再继续本计划」的外部更新（2026-10-24，与 project-lessons 024 决定同源）；本轮按用户直接指令完成 023 M1–M7，顺序由用户裁决。

## 6. 结论

§4 完成标准逐项有证据（规格 §5.6）；门禁状态：TDD／综合测试／安全实测／双轴审查闭环；**最终门禁全量绿**——2026-10-25 容器重启后 `cargo check` exit 0＋`cargo test --workspace --no-fail-fast` exit 0（3445/0/3），交付证据收口（R1 跨台 max 组合补证用例＋双变异、lint 门禁认定、记录校正）后复验 **exit 0（3446/0/3，63 目标）**，覆盖最终候选（含末尾两处测试侧改动与 R1 补证用例）。clippy／rustfmt 非本环境强制门禁、工具链未安装，如实记录未执行（§5）。交付候选位于 `feat/023-mcp-resources` 工作区，**未提交、未合并、未推送、未发布**——等待用户批准提交、合并和删除任务分支。
