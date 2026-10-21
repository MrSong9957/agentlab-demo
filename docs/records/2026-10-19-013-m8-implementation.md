# 013-M8 实施记录：引用反馈与便条差异过滤（2026-10-19）

状态：**已实现并离线验证（含补验轮：mem_read 旁路显式断言用例＋m5「读取不计数」变异证明），交付候选待批准**。分支 `feat/013-memory-m8`（基线 main @ `1ea1a3bf78c7a54527bf2cbdfe0af44ae13bd3f3`），合并目标 main。本轮零真实供应商请求、零真实用户数据访问、全部临时目录＋mock；未暂存/提交/合并/推送/删除分支。

## 0. 批准与边界（D12-A）

用户批准方案 A（2026-10-19），批准登记全文见规格 §5 D12。要点：

1. 非法便条不进入自动提供的差异状态列表和正文；
2. mem_list/mem_read 仍可发现、读取（提示词指引不构成技术隔离）；
3. 不提供通用写入时拒绝（exec/apply_patch 合同不动）；
4. 不覆盖保证仅适用于 apply_patch Add（create_new 固有）；
5. **A25 契约调整**：从「写入门禁层校验」调整为「差异层过滤」，非法＝差异排除＋盘上保留（非写入报错）。

其余前置全部复用既有批准：D1（SQL 移植约定）、D3（mem 工具注册表冻结）、D4（用户轮两工具不变）、D5、G8（git 基线）、A23（exec 无路径约束不虚称受限）。

## 1. 文件清单

| 文件 | 变化 |
|---|---|
| `src/app/src/memory/feedback.rs`（新，226 行） | 上游 `citations.rs` 移植（`parse_memory_citation`：多块合并、`rollout_ids` 跨块去重保序、兼容旧 `<thread_ids>`、双空→None、条目行解析容错）＋`extract_citation_blocks`（`<oai-mem-citation>` 块提取，未闭合不解析）＋`is_local_thread_id`（上游 UUID 严格格式的本地对应＝`^auto-([0-9]{8}T[0-9]{6}-[0-9]+|[a-z0-9][a-z0-9._-]*)$`，≤128 字节；权威忽略仍由 SQL 缺行承担）＋`cited_thread_ids`＋`memory_usage_kind_for_path`（上游 `usage.rs` 五类分类的路径级移植，`memories_v2/` 归一保留；仅观测语义）＋`after_user_turn`（enabled gate→开库→解析→计数，全失败静默） |
| `src/app/src/memory/feedback_tests.rs`（新） | 8 用例（A27/A28 锚＋本地形状/提取/过滤） |
| `src/app/src/memory/store.rs` | `record_stage1_output_usage(&[String]) -> Result<usize, String>`：上游 `record_stage1_output_usage` SQL 逐字移植（单 `BEGIN IMMEDIATE` 事务内逐 ID `UPDATE stage1_outputs SET usage_count = COALESCE(usage_count, 0) + 1, last_usage = ? WHERE thread_id = ?`；`now`＝Clock 秒，与 `source_updated_at` 同单位；缺行忽略返回受影响行数；空列表早退 Ok(0)；调用方去重——与上游 ID 列表来自已去重解析一致）＋2 用例（计数/时间戳推进；缺行忽略/批量/空列表） |
| `src/app/src/memory/workspace.rs` | D12-A 差异过滤：`memory_workspace_diff` 在状态列表解析后经 `invalid_ad_hoc_note_paths` 识别非法便条，非空时**两清单以同一 `:(exclude,literal)` 排除集重算**（无过滤时不付第二次 git 代价）；校验＝文件名上游 V2 正则（`^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-[a-z0-9][a-z0-9-]{0,79}\.md$`，总长 ≤128 字节，正则自身保证 ≥24）＋平铺目录边界（子目录非法）＋A/M 正文非全空白（≤1 MiB 有界读，非 UTF-8/读失败＝无效；D 无正文仅按名判定）。解析循环提取为 `parse_name_status_entries`。既有排除集（memories.sqlite*）、4 MiB 截断、基线/reset/所有权语义零改动；`stage_all` 与基线 reset 照常包含非法便条（基线＝盘面真值，仅差异视图过滤）。＋3 校验单测 |
| `src/app/src/main.rs` | 行式驱动：用户轮 `Ok(reply)` 先 `say` 回显后 `after_user_turn`（best-effort） |
| `src/app/src/tui_host.rs` | TUI 驱动：`Ok(reply)` 先 Delta 流式＋Final 渲染，后 `after_user_turn`（避免计数写库延迟首屏；失败静默） |
| `src/app/src/memory/mod.rs` | 注册 `feedback` 模块＋导出 `after_user_turn`/`cited_thread_ids` |
| `src/app/tests/memory_feedback.rs`（新） | 4 集成用例（真实 `run_tool_turn_with_mode`＋UserWithMemory＋mock sender＋真实 exec `cat` 记忆文件＋临时根真实 SQLite）：①含引用回复→反馈→`usage_count=1`＋`last_usage` 写入（反馈前显式断言未计数）；②工具真实打开记忆文件但无引用→不计数（A28 守卫型，m5 变异证明可检出）；③重复 ID 恰 +1/非法形状与缺行忽略/关闭态零副作用；④mem_read 旁路边界（D12-A 批准②显式断言：差异面过滤＋mem_list 仍发现＋mem_read 仍读取＋隐藏文件合同不变） |
| `src/app/tests/memory_phase2.rs` | ＋3 差异过滤用例：合法便条与同层 instructions.md 可见、非法名/大写 slug/超长/子目录/空白正文双清单排除；混合变化不误删正常差异、仅非法便条＝`has_changes=false` 且正文为空、合法名删除可见（D）/非法名删除排除；先真实合并建立产物后仅加非法便条→`SucceededNoWorkspaceChanges` 零模型请求且盘上保留＋对照合法便条不误伤。`set_usage` 注释同步（M8 起有生产写侧） |
| `src/app/tests/e2e_acceptance.rs` | ＋S89（TUI PTY：真实回复带引用→SQLite 计数恰 +1，后台悬于 silent 互不干扰）＋S90（行式管道同款）；共用播种/断言助手 |

生产代码不新增配置项、环境变量、HTTP 面、命令面；`docs/specs/013-memory-system-v1.md` 同步登记（状态行/修订/M8 实测状态/A25/A27–A28 矩阵/D12 批准）。

## 2. 来源复用（固定上游 `openai/codex@ce0323f2f2d3b25e82806cd555237da3db4e1897`）

| 本地 | 上游 | 移植方式 |
|---|---|---|
| `feedback.rs::parse_memory_citation`/`parse_memory_citation_entry`/`extract_block`/`extract_ids_block` | `codex-rs/memories/read/src/citations.rs`（85 行） | 语义逐条移植（多块合并/HashSet 去重保序/`<thread_ids>` 兼容/条目 `path:start-end\|note=[…]` 容错/双空 None）；`ThreadId` 类型→本地 String |
| `feedback.rs::memory_usage_kind_for_path` | `memories/read/src/usage.rs`（75 行）`get_memory_usage` | 五类匹配＋`memories_v2/` 归一移植为路径级；**shell 脚本解析器（`parse_shell_script`）未移植**——本地无遥测消费方（上游仅 `core/src/memory_usage.rs` 发 counter），如实登记；A28 语义不依赖解析器 |
| `store.rs::record_stage1_output_usage` | `state/src/runtime/memories.rs` 同名函数 | SQL 文本逐字；`Utc::now().timestamp()`→注入 Clock（秒）；逐 ID 单立即事务、缺行忽略、行数合计——全保留 |
| `workspace.rs` 便条文件名合同 | `ext/memories/src/tools/ad_hoc_note.rs` 参数校验（正则＋长度 24–128＋正文 ≥1） | 正则逐字复用为差异过滤规则（D12-A 批准的用途迁移） |

## 3. RED→GREEN 与错误检测证明

- **RED（实现前实跑）**：feedback 单测 8/8 断言失败（桩实现）；`memory_feedback` 2/3 失败（守卫用例 `tool_read_without_citation` 桩下天然绿，属守卫型）；e2e S89/S90 双失败（usage_count 为 None）。store 与 workspace 过滤实现先于用例首跑（首跑两处失败均为测试侧笔误——基线时序颠倒、A16 六小时冷却——修正后绿），未取得实现侧 RED，以变异证明补强。
- **变异证明（破坏→失败→恢复→通过，全部实跑）**：
  - **m1** 过滤禁用（`invalid_ad_hoc_note_paths` 直接返回空）→ 差异过滤 2 用例失败；恢复后绿。
  - **m2** 计数 SQL 去 `last_usage` → store 2 用例失败；恢复后绿。
  - **m3** TUI 驱动接线移除 → S89 失败；恢复后绿（行式接线由 S90 对称覆盖，未单独变异）。
  - **m4** 解析去重移除 → 去重锚与 `cited_ids` 2 用例失败；恢复后绿。
  - **m5** 「工具打开即计数」错误类注入（`feedback.rs` 加 `record_usage_for_all_stage1_outputs` 错误接线＋`turn.rs` exec_command 分支在 UserWithMemory 模式下调用——模拟把读取当引用计数的错误实现）→ 守卫用例 `m8_tool_read_without_citation_does_not_count` 失败；恢复后绿。
  - 未单独变异：行式接线（与 TUI 位点同构，S90 对称覆盖）——如实登记。
- **mem_read 旁路边界用例**（D12-A 批准②的显式断言，守卫型）：`m8_mem_tools_bypass_still_discovers_and_reads_invalid_notes`——同一临时根内先以真实 G8 差异证非法便条被过滤（两面互为对照），再经 `mem_tools::execute` 实调 mem_list（仍发现非法名、隐藏文件仍按合同排除）与 mem_read（仍读取正文），证明提示词指引不构成技术隔离。
- 变异期间全部经备份恢复，最终工作区无残留（`git status` 仅本候选差异）。

## 4. 门禁（真实退出码）

- `cargo check` → **exit 0**（本轮两次实跑：实现后、重定位后）。
- `cargo test --workspace --no-fail-fast` → **exit 0，2997 通过/0 失败/3 既有忽略**（基线 2975＋本轮 22——含补验轮新增 mem_read 旁路边界用例；两次终跑：主轮 2996/0/3、补验轮 2997/0/3；受影响范围基线在实现前实跑：lib memory 29/29、memory_runner 22/22、e2e S8x 10/10）。
- 定向验收（非零执行实证）：`cargo test --lib memory::`（42）、`--test memory_feedback`（3）、`--test memory_phase2 m8_`（3）、`--test e2e_acceptance s89`/`s90`（各 1）——过滤均命中非零目标，无空跑。
- clippy/rustfmt **未运行**：工具链组件下载失败（`rustup component add` 报下载错误，AGENTS.md §7 已注明默认未安装）；非选择性跳过，如实登记。

## 5. 安全适用性（手审）

无新增攻击面：零新 HTTP/环境变量/命令面；计数 SQL 全参数化；差异排除 pathspec 以独立 argv 传入（不经 shell）、`--` 分隔后置、`:(exclude,literal)` 阻断 glob/选项注入（路径源自 git 自身 `-z` 输出）；正文检查 ≤1 MiB 有界读；反馈链路失败静默不影响主回复/退出路径；不触碰真实用户数据（全部临时目录）。与既有 F1/显示脱敏/授权门无交互。

## 6. 边界、限制与未验证项

- **mem_read 旁路如实保留**（D12-A 批准第 2 点）：合并代理可经 mem_list/mem_read 直读非法便条；差异过滤不构成技术隔离。
- **不覆盖保证仅 apply_patch Add**；apply_patch Update/Delete 与 exec 改写/删除便条不拦截（上游 V1 同一现实）。
- **A25 语义＝差异层过滤**：非法便条留在盘上、不触发合并（仅非法便条单独变化时走 A17 无差异短路）；无写入时报错。
- 上游 shell 读取分类器未移植（无消费方）；A28 由路径级分类＋引擎级测试承载。
- 真实供应商上的引用产生行为未实测（回复内容取决于模型；既有 M7 供应商边界不变，不因本轮冒充）。
- usage 计数只在用户轮完成位点（两驱动）生效；合并轮（MemoryMerge）回复不参与反馈——与上游「引用来自用户回复」语义一致，未单测（无生产路径），如实登记。
- M9（保留清理/整链验收）未实施；V1 未完成；m18 类既有继承限制不因本轮变化。

## 7. Git 状态（候选停点）

分支 `feat/013-memory-m8`（自 main @ `1ea1a3b` 新建，同名分支先确认未占用、单工作树）；候选＝规格＋实施记录两文档、6 个修改源文件/测试文件、3 个新文件（feedback.rs/feedback_tests.rs/memory_feedback.rs）；暂存区空、stash 0；`git diff --check` 通过（exit 0）；保护项（AGENTS.md/CLAUDE.md/.gitignore/compose.yaml/.env/config/db 等）零改动；未提交、未合并、未推送、未删除分支。**停在「M8 交付候选待批准」。**

## 8. 交付记录（2026-10-19 用户批准后执行）

批准范围＝已验证的 12 个候选路径（8 修改＋4 新增，逐项与实施记录 §1/§7 清单一致）。交付核验：分支 `feat/013-memory-m8`、基线 HEAD `1ea1a3bf78c7a54527bf2cbdfe0af44ae13bd3f3`（＝准备轮参照 main，实际以 Git 关系核实）、暂存区空、stash 0、单工作树；工作区恰为 12 路径无候选外内容；五个变异涉及文件 `MUTATION` 残留 0、/tmp 备份未纳入；终验（2997/0/3）后仅两份文档有过编辑，源码/测试与验证态一致。

12 个批准路径：`docs/specs/013-memory-system-v1.md`、`docs/records/2026-10-19-013-m8-implementation.md`、`src/app/src/main.rs`、`src/app/src/tui_host.rs`、`src/app/src/memory/{mod.rs,store.rs,workspace.rs,feedback.rs,feedback_tests.rs}`、`src/app/tests/{e2e_acceptance.rs,memory_phase2.rs,memory_feedback.rs}`。

- **实施提交** `71e370e61fdb26c012a7c3370f27682ea0a99cbf`（`feat: 013-M8 引用反馈与便条差异过滤`，父＝`1ea1a3b`）：12 路径自 /tmp 清单经 `--pathspec-from-file` 逐文件显式暂存；暂存集合与批准清单 diff 为空（4A＋8M）；`git diff --cached --check` 通过；保护项零进入。
- **快进合并**：`git merge --ff-only` 入 main（1ea1a3b→71e370e6）；`merge-base --is-ancestor` 证实包含；合并后 main 文件树与实施提交逐字节一致（`git diff` 为空）。快进未引入变化，**引用交付前实测验证**：`cargo check` exit 0；`cargo test --workspace --no-fail-fast` exit 0，**2997 通过/0 失败/3 既有忽略**——不机械重跑、不追加真实调用。
- **交付文档提交**（本节＋规格状态翻转，`docs(013): M8 标记为已交付`）相对实施提交仅修改规格与本文档两份；D12-A 边界原文保留（自动差异过滤、mem_list/mem_read 旁路仍存在、通用写入不受此门禁约束、不覆盖仅 apply_patch Add）；历史记录、测试限制、变异说明（含守卫型用例与未单独变异项如实登记）全部保留；历史 manifest 与封批账本未改写。
- **分支清理**：确认实施提交已包含于 main 后 `git branch -d feat/013-memory-m8`（非强制）。
- 不推送、不发布、不操作远程、不开始 M9；**V1 未完成**（M9 未实施）。
