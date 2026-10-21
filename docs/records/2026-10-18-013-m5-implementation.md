# 013-M5 第二阶段合并实施记录（2026-10-18）

- 分支：`feat/013-memory-m5`（基线 main @ `b43b2e79bc79ce3e38639ad29668ea768b7302fb`，未提交）
- 规格：`docs/specs/013-memory-system-v1.md` §6 M5＋已批准 D3/G8 边界（2026-10-18 用户确认，登记于规格）
- 固定上游：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`（本轮经 raw.githubusercontent 拉取核对：phase2.rs/workspace.rs/baseline.rs/lib.rs 常量/prompts.rs/ext-memories local{.rs,/path.rs,/list.rs,/read.rs}/consolidation.md/state/src/runtime/memories.rs 的 get_phase2_input_selection/config types.rs 钳制值）
- 保护项核对：`.devcontainer/compose.yaml`、`docs/records/2026-10-02-install-mattpocock-skills.md`、`db/`、`.agents/`（本轮中途出现的门禁 Skill 文件，root 属主）全程未触碰。

## 0. 心跳契约修复与超时补验轮（2026-10-18，同日后续指令轮）

**纠正本记录此前结论**：§8/规格登记的「心跳＝每模型请求前按 90s 节流续租（同步阻塞下的承载）」**不满足已批准契约**——请求阻塞期间无续租。真实生命周期（本节为权威）：

- **实现**（phase2.rs；退出机制经同日「心跳退出边界定点收口轮」修订，见 §0a）：①每请求前确定性续租一次；②**请求阻塞期间**由 `thread::scope` 心跳线程按受控时钟每 `JOB_HEARTBEAT_SECONDS`（90s）续租（真实 5ms 轮询读注入时钟；上游事件循环周期心跳的同步阻塞等价承载）；③请求返回即停（停止位＋join；线程退出条件唯一为停止位、**无墙钟自限时**，panic 由 `StopHeartbeatOnDrop` 置位收口，`thread::scope` join 不挂死）。存储错误容忍（上游对心跳失败仅记录）。
- **RED 复现**（新增 `heartbeat_renews_during_blocked_model_request`）：mock 模型请求保持未完成，受控时钟跨 3 个 90s 点——修复前租期停在请求前值 `Some(T0+3600)`，5s 轮询等不到 `T0+90+3600` → 测试失败＝缺口复现；修复后通过（4 次复跑稳定）。断言含：每个心跳点后租期已续到该时刻＋3,600s（**请求未完成即可观测**）；有效续租期间第二领取者恒 `SkippedRunning`（不得接管）。轮询等待（真实 ≤5s）仅为测试同步手段，断言值全部来自受控时钟，未真实等待 90s/3,600s。
- **a22 重写**：原「时钟推进越过租期＋他人领取」的接管路径**已被运行期心跳阻断**（时钟推进期间续租照常——契约生效的直接证据，新心跳用例逐点断言）。失权终态（worker 崩溃后租期过期被领取、或心跳存储故障后失守）改为直接改行模拟「已被他人令牌占据」，聚焦失权后的处理面：再确认 `Ok(false)`→`LostOwnershipAfterValidation`、不 reset、不成功、不替接读者写失败——旧所有者不能越权完成成功提交。M1 所有权语义（`WHERE ownership_token=?` 条件提交）零改动。心跳存储错误被容忍的后果＝租期最终过期→同失权路径（登记；未注入存储故障实例）。
- **超时行为证据**（新增单元测试 `git_timeout_kills_process_group_with_descendants`，M5 公共入口直达）：crate 私有可测缝（程序/超时覆盖）＋假 git（合法脚本：后台 `sleep 30`＋记录双 pid 后 `wait`）＋300ms 生效超时——①由超时触发（无取消标志、路径/权限正常、脚本本身可运行，报文含「超时」与**生效**毫秒值）；②进程组清理：直接子进程（sh）与后代（sleep）经 SIGKILL 组杀全部终止（/proc 状态实证：本容器 PID 1 不收割孤儿，僵尸条目残留——活性判定以 State≠Z 为准，**终止即清理完成**，如实登记）；③失败契约：Err 上抛、基线未建立（`.git` 不存在）、不伪报成功。
- **超时补验过程的真实缺陷修复**：报文原报常量 60,000ms 而非生效值（误导）；`git init` 分支原不经统一程序缝（入口覆盖不完整）——均已修，全部 git 调用（含 init）单一缝。
- 受影响回归：memory_phase2 38/38（4 次复跑稳定）、`--lib memory::` 29/29（＋超时用例）；全量见 §7（本轮后复跑）。

## 0a. 心跳退出边界定点收口轮（2026-10-18，同日第三指令轮）

**核实结论（源码实证，非报告措辞）**：上轮的 `HEARTBEAT_THREAD_BELT`（原 phase2.rs:42，10s）位于**生产**心跳线程循环内（`run_phase2_consolidation`→sender→`scope.spawn`），计时源＝`Instant::now()` 墙钟、无测试门控——**非测试看门狗，而是生产生命周期的墙钟提前退出**：模型请求在途超 10s（LLM 常态）即停跳，后续跨租期即可被接管。全程续租契约缺口成立，按既有授权修复。

**修复**（phase2.rs）：删除墙钟带；线程**唯一**退出条件＝停止位；panic 展开改由 `StopHeartbeatOnDrop`（Drop 置位，绑定于 `chat` 前）收口——`thread::scope` 展开路径 join ≤一个轮询周期，无时间带、不挂死。正常/错误返回保持显式置位＋join。

**证据（tdd）**：
- **RED**：新用例 `heartbeat_survives_wall_clock_beyond_old_belt_while_request_in_flight`——请求在途：业务 tick 续租（证明存活）→**真实墙钟 10.5s**（跨过旧 10s 带；与业务时钟断言分立，不以「业务跨点而墙钟未到」替代）→业务时钟再进 90s——修复前租期停在 `Some(T0+90+3600)`，5s 轮询等不到 `T0+180+3600`（失败复现）；修复后通过。
- **panic 路径**：新用例 `heartbeat_thread_stops_on_panic_unwind`——chat 内 panic：`catch_unwind` 捕获（测试完成本身证明 scope join 收口不挂死）＋panic 后业务时钟推进＋真实等待，租期停在 panic 前值（线程已回收）＋任务行保持 running（panic 不做失败记账，由租期到期接管承接——上游同语义）。
- 正常/错误路径退出：既有 38 用例覆盖（每请求 join；取消/模型错误路径照走）。
- 心跳 3 用例 4 复跑稳定；套件 40/40（38＋2 新例）×3 复跑。

**code-review（skill 双轴，基点 main，聚焦本轮）**：规格轴——行为符合契约（①请求前续租 ②阻塞期全程覆盖（先于 chat 启动、返回后置位 join）③即停；「不得用最终所有权检查替代续租」成立——续租在前、第 10 步再确认仅额外确认；M1 所有权语义零改动）；唯一待办＝登记文本滞后（已更正 §0 与规格修订行）。标准轴——硬项＝同一文档漂移（已更正）；焦点核对全过（退出单一来源、Drop 生命周期、10.5s 为行为学最小诚实成本、无挂死路径）。判断题处置：`flag`→`stop_flag`（取）、删除死 `#[allow(clippy::too_many_arguments)]`（7 参未触 >7 阈值，取）、测试「两产物批」6 处同形提取 `merge_artifact_batch()`（取）；**未取并登记**：`last` 以当前时钟初始化（消除首跳重复续租）——引入「线程取样与受控时钟推进」竞态跳过首个 90s 点，三心跳用例实测捕获后回退 `i64::MIN`（幂等重复无害；坑入 §8）、轮询密度 5ms 保持（改密度徒增时序风险）。观察级登记：请求间工具执行间隙无心跳（每请求前续租＋租期 3,600s 覆盖，「请求返回即停」已登记）；心跳线程自身 panic 被 join 吞没（无挂死）；`store.now()` 理论 panic 边缘。

**最终计数**：见 §7。本轮无真实模型调用、未触碰真实记忆目录、未提交。

## 1. 准备与门禁方法

- 门禁序列：tdd → comprehensive-testing → Security Testing Gate → code-review → verify:full（AGENTS.md §7）。
- 受影响基线先行：`cargo check` 通过（既有 noop_method_call 告警为交接原状）；`cargo test --workspace --test memory_store --test memory_candidates --test memory_phase1 --test memory_storage_files`＝81/0 通过（M5 前基线）。
- 指定门禁 Skill 核查：`security-review` 因仓库无 `origin/HEAD`（无远程）在入口 `git diff origin/HEAD...` 处失败——按 M4 先例转**手动安全检查**（§5），未冒称 Skill 执行。`code-review` skill 可用（显式基点 `main`），双轴子代理报告见 §6。

## 2. 来源、复用与适配

新增 `src/app/src/memory/`：

| 本地 | 上游来源（ce0323f） | 语义保留 | 本地适配（登记） |
|---|---|---|---|
| `phase2.rs` | `memories/write/src/phase2.rs::run`＋`mod job`＋`mod stage_two` 常量 | 编排序（领取→prepare→选择→水位→同步→diff→短路/合并→校验→再确认→reset→SQL 成功）、失败原因枚举逐字（failed_prepare_workspace/failed_load_stage1_outputs/failed_sync_workspace_inputs/failed_workspace_status/failed_workspace_diff_file/failed_agent/failed_invalid_artifacts/failed_confirm_ownership/failed_workspace_commit）、重试延迟 3600s、`job::failed` 失权兜底 | ①合并代理＝`run_tool_turn_with_mode`（ToolMode::MemoryMerge）；②心跳挂点＝每模型请求前续租＋校验后再确认（上游事件循环周期心跳→同步阻塞下的最细可行粒度）；③`failed_spawn_agent`/`failed_sandbox_policy` 无对应概念并入 `failed_agent`；④线程元数据经注入解析器（M7 接登记写侧），解析缺失＝sync 失败不静默丢行；⑤合并轮会话纯内存态；⑥`Phase2Outcome` 显式区分 `sql_committed`（非原子语义不虚报） |
| `workspace.rs` | `memories/write/src/workspace.rs`＋`git-utils/src/baseline.rs` | prepare/diff/reset/validate 编排、`GitBaselineDiff{changes,unified_diff}`、差异文件渲染（Status 列表＋diff 块＋4 MiB 截断标记逐字）、remove_workspace_diff 三处前置删除、校验＝symlink 计数/MEMORY.md 常规文件/摘要首行 v1 | **载体改外部 git 进程（G8 批准）**：固定 argv 直启（每条 `--git-dir=<root>/.git --work-tree=<root>` 锚定＋`git init` cwd=root——git 永不向上发现父仓库）；`env_clear` 后仅回填 PATH/LC_ALL＋`GIT_CONFIG_COUNT=0`＋`GIT_CONFIG_GLOBAL/SYSTEM=/dev/null`；`-c user.name/user.email/commit.gpgsign=false/core.quotePath=false` 固定；60s 超时＋进程组 SIGKILL＋取消标志＋输出上限（stdout 32 MiB/stderr 1 MiB 码点，003 同构双流读取线程）；ensure＝`rev-parse --verify HEAD` 判健康，不可用→**删除记忆根自有 `.git` 后全新初建**（上游破坏性替换语义；规格预留的「init 幂等修复」对损坏 HEAD 不成立——reinit 不重写 HEAD 文件，实测确认）；初建普通 commit（unborn 上 amend 必败）＋`--allow-empty` 覆盖空根＋`read-tree --reset`；**提交时间确定性**：初建固定纪元、amend 取 HEAD 时间＋1s（真实时钟同秒 amend 产生相同 SHA 的坑，见 §7-P3）；diff＝同一条 `add -A -f`＋排除集 pathspec 的 `--name-status -z`（无引号歧义、`--no-renames` 恰 A/M/D）与全文两查、`--no-color/--no-ext-diff/--no-textconv`；reset＝删差异文件→`git rm --cached -q --ignore-unmatch`（**曾被跟踪的排除文件退出基线机制**：只动索引、不删真实库文件；仅排除 pathspec 时 `diff HEAD` 虽不可见但旧基线仍含该文件，amend 会保留——故必须 rm --cached）→add→`commit --amend --allow-empty`→`read-tree --reset` |
| `mem_tools.rs` | `ext/memories/src/local.rs`＋`local/{path,list,read}.rs`＋`ext/memories/src/lib.rs` 常量 | resolve_scoped_path 边界（绝对/`..`/符号链接组件/穿越非目录拒绝、尾部缺失合法）、mem_read（行号 1 起/`max_lines`/token 预算中间截断/`truncated` 判定/行偏移越界错）、mem_list（单层、排序、游标分页、`next_cursor`）、上限 20,000 tokens/2,000 条/1 MiB | **D3 批准边界扩展**：隐藏组件与 `memories.sqlite`/`memories.sqlite-journal`（任意层级同名）读取/写入/列表展示全拒；mem_write＝常规文件新建/覆盖＋父目录须存在＋0600（catalog::write_private_file）；结果 JSON 沿上游字段＋失败单 `error` 结构＋写入 `ok` 标志（本地惯例） |
| `templates/consolidation.md` | `memories/write/templates/memories/consolidation.md` | 逐字节拷贝（51237 字节，cmp 校验一致） | 不改原文；git 句/rg「or equivalent」/skills 段按规格登记保留，适配注记（无脚本执行能力）入 CONSOLIDATION_SYSTEM 包装层 |

修改既有：

| 文件 | 改动 |
|---|---|
| `turn.rs` | `ToolMode{User,MemoryMerge{root,system}}` 参数化：`run_tool_turn` 原签名委托 `run_tool_turn_with_mode`（用户轮逐字节不变）；`execute_tool`/`skipped_by_cancel` 按 mode 分派——**合并轮对 exec_command/apply_patch/未注册名一律未知工具拒执行**（错误信息列出 mem 三项）；合并轮取消形状经 `mem_tools::skipped_by_cancel_shape` |
| `prompt.rs` | `build_turn_request_with(history, tools, system)`（`build_turn_request` 委托默认面） |
| `store.rs` | `get_phase2_input_selection(n, max_unused_days)`：候选 SQL 逐字（非空行＋last_usage/source 窗口＋usage_count/COALESCE/源时间/thread_id 四键排序＋LIMIT/OFFSET 分页 512）＋threads 启用门＋全行水合＋thread_id 升序；cutoff 经注入时钟 |
| `config.rs` | ＋`max_raw_memories_for_consolidation`（默认 256，1–4096）＋`max_unused_days`（默认 30，0–365）——上游 config types.rs 同值同钳制 |
| `storage_files.rs` | `ensure_layout`/`remove_memory_symlinks` 提为 pub(crate)（M5 复用），后者返回移除计数（上游计数语义，校验判定用） |
| `mod.rs` | 新模块导出（phase2 常量仅心跳新名，租期/重试延迟与 stage1 同值不再重复导出） |

## 3. TDD 与补强

- **RED**：先写 `src/app/tests/memory_phase2.rs` 全量用例＋可编译空骨架（`Err("未实现")`）→ `cargo test --workspace --test memory_phase2`＝**4 通过/25 失败**（29 例）——目标行为缺失导致失败（接口存在后行为级失败，非仅编译失败）。
- **GREEN**：实现后 29/29；comprehensive 补强 4 例（合并轮批 32 上限、参数默认/钳制、取消传播→failed_agent、空选择首轮合并后短路）→ 33/33。
- code-review 后按规格轴/标准轴发现再补 5 例（§9.8 不回滚、failed_workspace_status、重试记账断言、产物 containment 全树断言、合并进度）＋隔离测试重构 → **38/38**。
- 模块内单测 6 例（mem_tools 边界函数/行偏移、workspace 截断渲染/字符边界、phase2 水位/模板渲染）。
- 既有测试适配 1 处：`memory_candidates.rs` 配置结构体字面量补新字段（顺带补两条新钳制断言）。
- 时间全经 SteppedClock 注入；目录全用 `common::TempDir`；模型全 mock 闭包（经 `run_tool_turn_with_mode` 真实请求组装与工具分派）；git 断言为测试侧一次性临时仓库内命令（不触 /workspace 工作仓库）；env 敌意注入由子进程承载（与并行测试零共享）。

## 4. 错误检测证明（变异轮）

方法：临时破坏实现 → 定向测试必须失败 → 恢复（与备份逐字节 diff 验证）→ 复跑通过。

| # | 变异 | 目标测试 | 结果 |
|---|---|---|---|
| A | 移除排除文件名拒绝 | mem_read_boundaries | 失败→恢复通过 |
| B | 移除隐藏路径拒绝 | mem_read_boundaries | 失败→恢复通过 |
| C | 合并轮分派放行 exec_command | dispatch_rejects_unregistered_tools_in_merge_mode | 失败→恢复通过 |
| D | 移除 sender 心跳 | heartbeat_extends_lease_during_merge | 失败→恢复通过 |
| E | 短路不校验产物 | a18_missing_artifacts_merge_path | 失败→恢复通过 |
| F | reset 去掉 rm --cached | git_reset_single_commit_and_tracked_exclusion | 失败→恢复通过 |
| G | 去掉 --git-dir/--work-tree 锚定 | git_primitives_never_touch_parent_repository | 失败→恢复通过 |
| H | 移除 env_clear＋GIT 隔离 | git_config_isolation_and_fixed_identity | 失败→恢复通过 |
| I | 跳过校验后所有权再确认 | a22_lost_ownership_after_validation_does_nothing | 失败→恢复通过 |
| — | 破坏符号链接拒绝（resolve 层/终态层/单层） | mem_read_boundaries | **不可破坏**：三层冗余（resolve 组件检查＋终态 symlink 检查＋`symlink_metadata().is_file()` 对符号链接恒 false）——按 AGENTS.md「不改变实际行为的改动可说明后排除」登记 |

变异轮副产品（测试盲区修复）：mem_read_boundaries 原以二进制 sqlite 恰非 UTF-8 的读取失败侥幸通过变异 A——改为文本化库文件＋按拒绝理由断言（「记忆根内/隐藏路径/不可访问/符号链接/不能为空」）。

## 5. Security Testing Gate（手动；Skill 入口失败见 §1）

补记：会话中途（2026-10-18 15:02）`.agents/agents/Security-Testing-Gate.md`（root 属主）出现在工作区——门禁 Skill 定义本体本轮中途落位，未在本会话可用技能列表内、无法经 Skill 工具加载执行，不冒称已运行。其核心方法「先审查代码、再真刀真枪打一遍」与本节实际做法一致：全部结论以**真实载荷攻防**为据（真实分派路径的越界/注入载荷、真实子进程 env 注入、真实白名单绕过尝试、真实父仓库嵌套 git 操作），非纯静态审查。

检查面与结论（全部有测试或代码证据）：

1. **mem_* 路径边界（D3）**：绝对路径/父目录越界/隐藏/排除名/符号链接组件与终态/穿越非目录/目录与特殊文件目标全拒（变异 A/B 捕获单点移除）；写仅限常规文件新建/覆盖＋父目录存在；1 MiB/20k tokens/2,000 条预算；路径含 NUL 走 syscall 错误路径不 panic。
2. **分派权限**：合并轮白名单在 `execute_tool` 分派层生效——mock 模型返回 exec_command/apply_patch/web_search 均不执行、零副作用（真实探针文件验证；变异 C 捕获绕过）。请求体 tools 列表不构成保护（分派独立判定）。
3. **Git 子进程（G8）**：固定 argv 不经 shell；仓库锚定防向上发现父仓库（变异 G）；env_clear＋GIT_CONFIG 注入面/GIT_DIR 泄漏阻断（变异 H，含 diff.noprefix/core.excludesfile 实际注入验证）；固定 identity＋gpgsign 关闭；60s 超时＋进程组终止＋取消（取消路径与超时共用 kill 逻辑）；输出码点上限；`rm --cached` 仅限两个固定名、不删真实文件。
4. **失败副作用**：失败原因恒静态字符串（无秘密/模型内容入 last_error）；reset 失败不写 SQL 成功、短路不 reset、失权不记账（A21/A22/失败路径用例）。
5. **文件面**：差异文件与 mem 写入经 write_private_file（0600）；记忆根外零写入（父仓库 HEAD/index/工作区不变断言＋sessions/ 不创建）。
6. **资源界**：选择上限 4096 钳制＋分页；git 输出 32 MiB；差异文件 4 MiB；换算 saturating（008 shim 既有验证）。
7. **已知限制（与上游同界，登记）**：resolve 与实际读写之间存在 TOCTOU 窗口（上游 local.rs 同款；合并轮工具串行执行、无并发攻击面，外部并发修改不在防御范围）。

## 6. code-review（skill 双轴，基点 main；两子代理并行）

**标准轴发现与处置**：
1. 硬违规·未使用常量 `JOB_HEARTBEAT_SECONDS` → **已修**：心跳改为上游 90s 间隔节流承载（距上次续租 ≥90s 才续、首请求必续、校验后再确认无条件）——常量真实生效；`MemoryStore::now` 提为 pub（节流判定）。
2. 判断题·env 隔离测试 `set_var` 与并行测试 `getenv` 数据竞争（POSIX UB）→ **已修**：敌意 env 改由**子进程**承载（父用例重执行本测试二进制 `--exact git_config_isolation_inner`＋`M5_ISOLATION_CHILD` 门；子进程 panic 传导为非零退出，非空转）。
3. 判断题·`run_git`/读取线程与 exec.rs 同构复制 → G8 规格 已登记「exec.rs 无非 shell 原语、须私有新写」豁免；**补齐**读取收尾宽限（完成信号通道＋2s 超时，防孙进程持管道使 join 无限等待——与 exec.rs 对齐）。
4. 判断题·threads 启用门 SQL 两现 → **已修**：提取 `thread_enabled` 助手共用。
5. 判断题·Middle-Man：三个无 cancel 包装仅测试使用 → **已修**：API 合一（普通名承载 `cancel` 参数，删除包装）。
6. 判断题·`(String, i64)` 裸元组 → 保留：单函数内部使用、两字段、即上游选择键形状，单用建型不偿失（说明后排除）。

**规格轴发现与处置**：
- (a)1 §9.8 非原子契约无用例 → **已补**：`partial_changes_not_rolled_back_on_validation_failure`（校验失败后已写产物保留、不 reset、按失败记账）。
- (a)2 `failed_workspace_status` 零用例＋prepare/sync/reset 失败未断言重试记账 → **已补**：`diff_failure_marks_failed_workspace_status`（.git 置 0500 → add 写索引失败）＋`assert_retry_accounting`（全部领取后失败路径断言 error/3→2/退避 now+3,600s）。
- (a)3 合并产物 containment 仅 proxy、consolidation_progress 无测试 → **已补**：`merge_outputs_confined_to_memory_root`（tmp 全树遍历、根外零文件）＋`consolidation_progress_tracks_selected_count`。
- (b) 范围蔓延：无（两 config 参数＝§4 表值；参数化＝D3 明文；从严「任意层级同名排除」无害且登记）。
- (c)1 心跳适配规格未登记 → 本轮规格回填（见 013 修订行）。
- (c)2 ensure「删自有 .git 重建」与规格字面「init 幂等修复」不一致 → 规格回填（reinit 不重写损坏 HEAD，实测；上游 ensure 本为破坏性替换语义）。
- (c)3 提交时间确定性（纪元＋HEAD+1s）未登记 → 规格回填。

审查后重跑：memory_phase2 38/38（新增 5 例＋隔离测试重构）；全量见 §7。模板逐字节（51,237 字节 sha256 校验）与 SQL 逐字同构经子代理独立核对确认。

## 7. verify:full 与最终状态

- `cargo check`：0 错误（新代码零告警；既有 noop_method_call/tui 告警为交接原状）。
- 心跳修复与超时补验轮（§0）后复跑全量＝2924/0/3；**心跳退出边界定点收口轮（§0a，最终候选）后复跑全量**：`cargo test --workspace --no-fail-fast`＝**2926 通过 / 0 失败 / 3 既有忽略**（M4 交接 2879＋M5 新增 47：memory_phase2 集成 40＋模块单测 7——含墙钟边界与 panic 两新用例；含 e2e 95/95 在全量内通过）。
- 规格验收命令实测（非零且通过）：五测试目标 18+19+40+23+21＝121＋`cargo test -p terminal-chat --lib memory::` 29。
- 工作区：M5 改动未提交（分支 feat/013-memory-m5，无新提交）；保护项（compose 修改、records 修改、db/、.agents/）原样未触碰。

## 8. 关键判断与坑

- **P1 enqueue 水位 +1**：`mark_stage1_job_succeeded` 触发的 `enqueue_global_consolidation` 对「旧版本新输入」走 `COALESCE(input_watermark,0)+1`（上游逐字 SQL）——成功水位可高于全部源时间戳（实测 T0-99＞T0-100）。语义：新输入到达即推进、防旧水位短路。测试据此断言。
- **P2 diff 的删除检测**：`git diff HEAD` 只对 HEAD 内文件报删除；stage_all 仅暂存不提交。删除用例须先 reset 入基线再删（测试重排后覆盖）。
- **P3 amend 同秒同树＝同 SHA**：真实时钟下两次 reset 落同一秒且树相同时 HEAD 不变（a18 在全量并发下偶发）——提交时间改为确定性派生（初建固定纪元、amend=HEAD 时间＋1s），reset 恒产生新提交。
- **P4 upstream ensure 的「init 修复」不覆盖损坏 HEAD**：reinit 不重写既有 HEAD 文件——不可用元数据须删除 `.git` 全新初建（上游 ensure_git_baseline_repository 同为破坏性替换）。规格 §3 G8 行的「幂等 reinit 修复结构」据此修正登记。
- **P5 选择排序≠返回排序**：usage_count 等四键排序决定**取舍**（top-N），返回按 thread_id 升序稳定（上游同款）——测试两侧分别断言。
- **P6 A18 真实构造**：文件基线下「删除产物」本身即产生差异，短路条件不可达；真实路径＝基线损坏重建（ensure 以当前无产物树重立）后同步无差异（测试按此构造，变异 E 可捕获）。
- **P7 首轮恒合并**：ensure 初建基线先于 sync，故首轮同步产物必构成差异（含空选择）——与上游一致，无特殊分支。
- **P8 心跳线程 `last` 初始化竞态（§0a 轮实测）**：`last` 以当前时钟初始化（意图消除与「请求前续租」的幂等重复）会引入「线程取样 vs 受控时钟推进」竞态——线程若在测试推进后才取样，首个 90s 点被跳过、三心跳用例齐败；回退 `i64::MIN`（首查必续，重复无害）。教训：涉及采样线程与注入时钟的交互，初始化基准必须取「恒过期」值。

## 9. 限制与后续交接

- ~~心跳粒度＝每请求前~~（§0 已纠正：请求阻塞期间由作用域心跳线程续租，90s 受控时钟间隔）。
- ~~超时行为未单独行为测试~~（§0 已补：入口直达超时路径＋后代组杀＋失败契约实证；「与 003 同构」不再作为证据替代）。
- 心跳存储故障未注入实例（容忍语义＝代码路径＋失权后果经改行模拟覆盖，见 §0）。
- 线程元数据解析器为 M7 接线点（threads 登记写侧落建后替换合成解析器）。
- Windows：git 锚定/进程组为 unix 路径（与项目现状一致，未实测）。
- M6–M9 未实施；本阶段不构成 V1 完成。

## 10. 批准交付与合并核验（2026-10-19）

用户本轮明确批准 M5 交付：提交实现、快进合并 main、核验后安全删除任务分支；不推送、不发布、不操作远程分支、不开始 M6–M9。

- **交付前核验**：分支 `feat/013-memory-m5`，HEAD＝main＝merge-base＝`b43b2e79bc79ce3e38639ad29668ea768b7302fb`（快进前提成立），暂存区空、无 stash。候选 14 路径与工作区实际差异一一对应；`phase2.rs` 含最终心跳修复（`StopHeartbeatOnDrop`、停止位唯一退出、无墙钟带，§0a），`workspace.rs` 含超时/进程组契约（§0）。**验证对应性**：全部 Rust 源码最后改动 ≤16:19:08（phase2.rs），最终测试二进制构建于 16:19:34（`target/debug/deps/memory_phase2-35b15a231f0a6fa2`），其后仅两个 .md 文档更新（不参与编译；`templates/consolidation.md` 为 include_str! 资产、mtime 14:11 早于构建）——提交时代码与 §7 最终验证（`cargo check` 0 错误、全量 **2926/0/3**）的状态一致。
- **保护项交付前基线**：`.devcontainer/compose.yaml`（+10 行）、`docs/records/2026-10-02-install-mattpocock-skills.md`（+33 行）修改保留未暂存；`db/`（含 wiki.db，未打开）、`.agents/`（含 Security-Testing-Gate.md）untracked 原样。
- **精确暂存**：14 路径逐项显式 `git add`（无 `add -A`）；暂存清单恰为批准清单（5 新增＋9 修改，4639+/37-），未暂存剩余恰为四项保护内容；`git diff --cached --check` 通过；暂存版规格状态行仍为「M5 待批准」（交付标记在本交付文档提交中翻转）。
- **M5 实现提交**：`96e590856b5efa0698e98b0ada385d8f37d950e0`，`feat: 013-M5 第二阶段合并`，14 文件（4639+/37-），树 `a7355825c1d08a01903c452da5e29905af6d28c8`；提交文件清单与批准清单逐项一致，保护项零进入。
- **快进合并**：`git switch main`（基线仍 `b43b2e79…`，工作区保护修改随切换保留）→ `git merge --ff-only feat/013-memory-m5` 成功（Fast-forward）；合并后 main＝`96e590856b5efa0698e98b0ada385d8f37d950e0`；`git merge-base --is-ancestor 96e5908… main` exit 0；main 树＝M5 提交树＝`a7355825…`、`git diff 96e5908… main` 为空——快进未引入候选外代码变化，按授权复用 §7 最终验证（2926/0/3），不机械重跑。
- 合并核验通过后 013 M5 标记**已交付**（M6–M9 保持未实施、V1 整体不标完成、其他决策状态不变）；交付文档提交仅限 013 与本记录两文件。任务分支的安全删除（`git branch -d`，包含关系前置核验已过、不强制删除）与 main 最终 SHA 由本轮最终 Git 核验报告给出；不涉及远端。
