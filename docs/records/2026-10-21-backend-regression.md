# 后端自动回归实施与验证

日期：2026-10-21。基线：`bb84d180e259b6aa361abd0ad250a935d4800258`。来源：本会话确认的七项高风险功能及正常／边界／异常设计；[范围记录](2026-10-21-backend-risk-test-priorities.md)。用户授权实现自动回归，并要求今后功能新增或修改后全量运行、主动报告结果。

实现遵循 TDD → comprehensive-testing → Security Testing → code-review → 全量验证。测试边界已由前述讨论确认：真实 CLI、公开工具与存储接口；不另加审批、沙箱或跨文件事务承诺。已有充分测试直接复用，具体映射见 [覆盖清单](../testing/backend-regression.md)。

执行前检查 Git 状态。原有 AGENTS.md、2026-10-02 安装记录的修改，以及 project-lessons.md、013 对照报告、015 补丁／日志／清单和风险记录均保留；未暂存、提交、推送。测试与文档之外只新增一处生产修复：记忆库迁移在并发打开时的登记复核。公开接口、Schema 与持久化格式未变化。

## 本次最小改动

| 文件 | 保护的可观测结果 |
|---|---|
| `src/app/tests/exec_command.rs` | 新增真实 7×3 计算，stdout 和文件必须恰为 `21\n`；新增 `timeout_ms=1` 最小合法值必须被接受并真实超时；新增 `max_output_chars=1/2/3` 按 Unicode 码点截断 `中🙂文`，超限后排空管道不阻塞 |
| `src/app/tests/apply_patch.rs` | 既有完整增删改核对 `9→10`、`6` 字节及无关文件不变；新增跨文件上下文失败：首项修改保留、第二项字节不变、第三项不执行，成功前缀清单完整且 `state_known=true` |
| `src/app/tests/session.rs` | 新增读侧 64 MiB 边界：恰上限记录可装载；超 1 字节跳过、计数与行号准确 |
| `src/app/tests/context_injection.rs` | 新增真实用户轮：异常链接和非法 UTF-8 正文不外发，有效规则与记忆摘要正常共存；`t09` 新增 32769 字节「恰超 1 字节」用例，正文恰为 32768 且无项目内容时标题不带 ` for {cwd}` |
| `src/app/tests/memory_phase2.rs` | 租约被挡时模型请求数必须为 0；新增合并失败不得推进成功水位；合并轮拒绝工具的真实目标路径与断言对齐 |
| `src/app/tests/memory_candidates.rs` | 新增配置边界经真实装载和任务领取：0→1、2→2、999→128，核对任务状态与水位；负数／超表示范围拒绝且配置字节不变 |
| `src/app/tests/memory_runner.rs` | 加强既有 reset：未确认时库、摘要、便条、会话及代际不变；成功时原始会话、外部链接目标逐字节保留；固定夹具业务时钟 |
| `src/app/tests/tool_loop.rs` | 加强 T8：500 与请求后断线均不重发，非幂等追加仅一次且下一轮可继续；T14 为 8 请求、7 次真实追加后终文 |
| `src/app/tests/e2e_acceptance.rs` | 加强 S2a：首次授权文件不含明文 Key；第二次使用授权文件逐字节不变，不重复新增 |
| `src/app/src/memory/store.rs` | 并发迁移竞态修复＋真实双连接回归：写事务内复核 `schema_migrations`，已应用则提交跳过；第三连接持写锁、busy_handler 同步两连接进入等待，不使用真实时钟 |
| `README.md`、`docs/testing/backend-regression.md` | 固定根目录全量入口、结果判读、七项覆盖映射和后续交付规则 |

合计新增 10 个回归用例，加强 8 个既有用例。未重复新建凭证、会话、保留清理、取消／超时、同源生命周期等已有充分覆盖。

## 迁移竞态：根因与修复

全量回归曾稳定暴露 `s81_m7_enabled_turn_injects_and_spawns_pipeline` 偶发失败（约 1/5～1/12）。失败日志为：

```text
[DEBUG-s81] store open failed: 执行迁移 0001_memories.sql 失败：table stage1_outputs already exists
```

TUI 登记器与后台记忆管线同时打开新库：两个连接都在事务外读到「0001 未应用」，先拿写锁者完成迁移后，后到者重放不含 `IF NOT EXISTS` 的 DDL 失败。管线在 `open_store_if_valid` 返回 `None` 后静默退出，`extensions/ad_hoc/instructions.md` 未播种，随后断言失败。

最小修复：`apply_migration` 在 `BEGIN IMMEDIATE` 写事务内再次查询 `schema_migrations`；已应用则提交并跳过。外层 `run_migrations` 的快检保留为性能快路径，两处共用 `migration_applied` 查询。新增 `memory::store::tests::migration_rechecks_after_concurrent_apply`：第三个连接持有写锁，两个连接各自在 `busy_handler` 计数确认已阻塞于 `BEGIN IMMEDIATE` 后释放，由真实并发确认一个完成、另一个在事务内复核后跳过；临时目录使用 pid＋原子计数器，不依赖真实时钟。旧实现下复现同一错误，修复后通过；M14 证明移除复核会稳定 RED，M15 进一步证明把复核移到 `BEGIN IMMEDIATE` 之前会稳定 RED。最终 store hash：`a2c16e759d0ae9da448beb11483322d17e4ea5f52b56f8ec951a07ba1c66dce5`。修复后 S81 先连跑 20/20，最终状态再连跑 10/10，均无失败。证据：`/tmp/s81-debug-11.log`、`/tmp/s81-fixed-*.log`、`/tmp/s81-run-*.log`；定向 RED／GREEN：`/tmp/terminal-chat-store-recheck-mutation2.log`、`/tmp/terminal-chat-store-recheck-final-green.log`。

## 全量与定向验证

根目录命令：

```bash
cargo check --locked --offline && cargo test --workspace --locked --offline --no-fail-fast
```

修改前实际执行 exit 0：**3035 通过、0 失败、3 既有忽略、0 过滤**，50 个目标结果（含空目标），真实 CLI／PTY `e2e_acceptance` 106/0/0。原始日志：`/tmp/terminal-chat-regression-baseline.log`。

修复迁移竞态前的全量：**3041 通过、1 失败、3 忽略**，唯一失败为上述 S81；日志 `/tmp/terminal-chat-regression-final.log`。

最终全量（最终候选，含并发测试改写，用户指定命令，均带 `--locked --offline`）：exit 0，50 个测试目标全部 `test result: ok`，**3045 通过、0 失败、3 忽略、0 过滤**；`e2e_acceptance` 106/0/0。最终日志：`/tmp/terminal-chat-regression-final4.log`，类型检查：`/tmp/terminal-chat-regression-final4-check.log`。`final3` 产生于并发测试改写前，仅作历史对照，不再作为当前候选的最终证据。

3 个忽略均为既有、非本轮新增，单独列出且不算通过：

- `bottom_pane::textarea::tests::altgr_ctrl_alt_char_inserts_literal`（仅 Windows 适用）
- `tui::owned_screen_tests::owned_screen_lifecycle_child`（由父测试启动的子进程 helper）
- `tui::panic_tests::caught_panic_pty_child`（由父测试启动的子进程 helper）

定向迭代只用于开发，不替代最终全量。所有新增用例均通过真实代码边界，不 Mock 核心执行、文件、SQLite 或业务判断；只替换模型服务和时钟。新测试不读取真实凭证、不连接真实模型，失败时临时夹具也清理。

## 错误检测能力（定向变异）

变异在隔离副本或工作树临时执行；每项保持测试断言不变，要求具体用例行为失败（编译失败不算检出），随后立即还原并校验生产源码 hash。M1–M10 的详细记录在 `/tmp/terminal-chat-regression-mutations.json`。

| 注入的错误 | 捕获断言 | 结果 |
|---|---|---|
| M1 命令只输出 21，不写文件 | product.txt 必须存在且字节正确 | 101→0 |
| M2 补丁失败时丢弃前序成功清单 | changed_files 必须含首项 update | 101→0 |
| M3 后台拒绝工具前偷偷执行补丁 | 同一绝对探针文件必须不存在 | 101→0 |
| M4 AGENTS 元数据判断跟随链接 | 实际请求只能含有效规则，外部标记不能进入 | 101→0 |
| M5 缺 confirm 时清空摘要但保留文件 | 摘要字节必须保持不变 | 101→0 |
| M6 模型请求失败后自动重发 | 错误不得被隐藏，请求与恢复契约准确 | 101→0 |
| M7 每个工具执行两次 | 7 次调用只能产生 7 次文件追加 | 101→0 |
| M8 将候选下界误改为 0 | 配置 0 必须实际领取 1 个任务 | 101→0 |
| M9 将候选上界误改为 129 | 配置 999 必须只领取 128 个任务 | 101→0 |
| M10 非法配置静默回退默认值 | 负数／超表示范围必须报错 | 101→0 |
| M11 拒绝 timeout／输出上限的 1 下界 | `timeout_ms=1` 与 `max_output_chars=1` 必须被接受 | 101→0 |
| M12 按 UTF-8 字节截断输出 | `中🙂文` 在 1/2/3 码点必须保留完整字符 | 101→0 |
| M13 正文预算允许 `max+1` | 32769 字节必须截为恰 32768 | 101→0 |
| M14 移除迁移事务内复核 | 并发后到连接必须跳过已应用迁移 | 101→0 |
| M15 将事务内复核移到 `BEGIN IMMEDIATE` 之前 | 两连接都先读到未应用，后到者重放 DDL 必须失败 | 101→0 |

M11／M12 记录：`/tmp/terminal-chat-regression-mutations-min-limits.json`；M13／M14：`/tmp/terminal-chat-regression-mutations-context-store.json`，RED／GREEN 日志分别见 `/tmp/M13-*.log`、`/tmp/M14-*.log`。M15 的 RED／GREEN：`/tmp/terminal-chat-store-recheck-mutation2.log`、`/tmp/terminal-chat-store-recheck-final-green.log`。无存活变异，工作树生产 mutation 已全部还原（context.rs `2ea46413…`、memory/store.rs `a2c16e75…`）。

## 审查结论

**Security Testing**：未发现高／中风险问题。授权文件不保存明文 Key，第二次使用不新增记录；未授权零请求、拒绝重定向、错误回显脱敏沿用既有覆盖。AGENTS 符号链接与非法 UTF-8 来源不进入实际请求，提示不泄露正文；合并轮拒绝 exec/apply_patch 时同一绝对目标零副作用。发现并修复一项低风险测试夹具错误：T8／T14 曾把未引用的绝对临时路径直接拼入 Shell，路径含空格时分词导致误判；改为固定文件名＋显式 workdir，并让夹具标签含空格，相关五例复验通过。诊断失败保留为证据，不计作正常通过。

**Standards**：最终候选 `cargo check --locked --offline` exit 0（`/tmp/terminal-chat-regression-final4-check.log`），`git diff --check` exit 0。Clippy 组件未安装，未对最终候选执行；`final3` 的 clippy 日志产生于测试改写前，不作为最终通过证据。`store.rs` 定向复审发现并修复两项：新测试曾用 `SystemTime::now()` 命名临时目录（改为 pid＋原子计数器）、顺序模拟未钉住事务内复核位置（改为第三连接持锁＋`busy_handler` 同步的真实双连接并发）；最终无硬违规。非阻塞判断题：`applied_at` 仅断言为任一调用方时间，不证明胜者；已应用分支提交空事务，保留。

**Spec**：七项功能的正常、边界与异常映射已补齐，见覆盖清单。不能稳定观测内部实现的两处保留公开结果断言、不绑定内部写法：首用授权提示的逐字文案（断言提示出现、未授权零请求、同意后实际请求）；摘要开关在单轮内的读取次数（断言关闭不注入、开启注入一次的结果）。`store.rs` 定向 Spec 轴：修复与 `013` 文首 2026-10-21 修订及 §6 M1 一致，无缺失、无 scope creep；并发回归在最终代码上实跑通过。摘要注入入口的链接／超大文件信任边界仍为待核项，本轮未新增产品承诺。

## 保留边界

用户命令的 cwd 不是沙箱；补丁和文件／Git／SQLite 合并不承诺跨操作回滚；原始会话保留用户输入和工具输出，不承诺通用脱敏；AGENTS 及同用户全局记忆可随请求外发；8 次是请求次数上限，不是金额预算。真实供应商语义、Windows 宿主专项不由 Linux 离线回归证明；以上未运行项不作为通过结论。
