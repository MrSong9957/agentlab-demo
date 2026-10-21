# 013-M6 记忆摘要注入实施记录（2026-10-19）

- 状态：**M6 已实现并验证，2026-10-19 用户批准后交付（实现提交经 `--ff-only` 合入 main，见 §9）**。M7–M9 未实施，V1 整体未完成。
- 分支：`feat/013-memory-m6`；基线/合并目标：`main @ 1762958fb01b60673e681f3d42006b8df8b19d5e`。
- 规格：`docs/specs/013-memory-system-v1.md` §5.1、§6 M6、A01–A03；D4 已于 2026-10-19 批准，无新增决策。
- 固定上游：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`，Apache-2.0。
- 保护项：compose、2026-10-02 记录、`db/`、`.agents/` 的初始节点内容/模式指纹与前轮一致；暂存区/stash 为空。已有 013 准备/批准登记保留；审查期间出现的其他工作见 §8。临时执行证据：`/tmp/013-m6-implementation-2rhxgjs6/`。

## 1. 门禁方法与测试边界

依次执行 tdd → comprehensive-testing → Security Testing Gate → code-review → verify:full。

- 已读取并使用 `/home/dev/.claude/skills/tdd/SKILL.md` 及 tests/mocking 指引；测试边界由用户本次任务和既定规格明确：摘要读取、可选请求组装、真实 turn＋文件/API/会话边界。沿已有授权执行，不重复询问 Skill 的 seam 确认。
- 初始核查未找到 `comprehensive-testing`、`verify:full` 本地 Skill，先按 AGENTS.md 和本次任务执行。审查期间保护目录新增出现 `.agents/skills/comprehensive-testing/{SKILL.md,reference.md}`（本任务未写入/安装），后续已只读加载并按参考 CTFL/CT-MBT/Mutation 章节完成对照审计，见 §4；`verify:full` 仍按仓库明定命令手动执行，不冒称加载。
- 安全门禁采用只读的 `.agents/agents/Security-Testing-Gate.md`，聚焦本轮范围，落盘报告；不把无用户/租户/管理接口的项目硬套为 Web 权限测试。
- 已读取 `/home/dev/.claude/skills/code-review/SKILL.md`；审查阶段按其规格/规范双轴子代理执行。用户要求不提交，审查以基线对工作区的实际差异及新增文件为准，不用空的三点提交差异冒充审查。
- 全部新增用例在 `src/app/tests/memory_inject.rs` 通过公开接口观察真实请求、进程、文件及保存恢复；仅外部 API 使用本地 mock，不替换被测注入/轮缓存/工具执行。

## 2. 基线与来源

受影响基线：`cargo test -p terminal-chat --lib --test prompt_assembly --test session_cli --test memory_phase1 --test memory_phase2 --test memory_candidates --test memory_store`，真实退出码 0，**257 通过/0 失败/0 忽略**（lib 129、候选 18、phase1 19、phase2 40、store 21、prompt 6、session 24）。M5 已交付的 2926/0/3 仅为全量历史对账值。既有依赖/TUI 告警保留，不冒称零告警。

读取侧按固定上游 `ext/memories/src/prompts.rs`：UTF-8 读取失败跳过→trim→Tokens(2,500)→空则 None→模板渲染。没有摘要首行版本校验；M5 写后校验的 `v1` 首行要求不擅自移到读取侧。固定 V1 路径，不新增版本选择或 V2 功能。模板仅按规格已登记的 Markdown 摘要/原始 JSONL 路径错述作必要适配。

固定 SHA 原文重新下载到临时证据目录，SHA256：`prompts.rs`＝`05fd1987748219a053074b9d3a6f79647875eda289f544e6047abafce7d470ab`；`read_path.md`＝`2bc7736029884b714860a6f0d6b2fd26598bba58f2213b787f01fe2a491f4326`。`lib.rs` 的摘要常量为 2,500，`config/src/types.rs` 的 `use_memories` 默认 true。未编译或运行上游测试。

- 直接复用：M3 `memory/truncate.rs`→008 `codex-utils-string`；M5 参数化请求组装与工具分派；现有会话与 API 层。
- 函数级移植：上游摘要读取/trim/截断/渲染顺序；异步文件调用适配为同步。
- 模板适配：只替换 rollout_summaries 目录下三条关联说明，明确 `.md` 摘要、`rollout_path` 指向原始 JSONL、本地 `HistoryItem` 类型及 `thread_id`；其余原文保留。黄金期望在实现前从固定上游及这三条适配建立，不从实际输出接受快照。
- 协议适配：D4 system 尾段及历史参考说明；不宣称 developer/system 等价。

## 3. TDD 证据

| 切片 | RED（真实退出码） | GREEN |
|---|---|---|
| A02 缺失摘要 | 101，缺 `memory::inject` 公共接口，非测试笔误 | 1/1，读取不创建目录 |
| A03 摘要与指引 | 101，黄金对比显示实际仅摘要，缺少整段指引 | 2/2，固定黄金一致，正文占位符保真 |
| 读取开关/配置 | 101，规格既定 `use_memories` 字段缺失 | 新增 3/3＋既有候选 18/18 |
| 摘要预算 | 101，动态长输入无中段截断标记 | 4/4，正文预算与固定指引/标记区分 |
| D4 可选组装 | 101，缺 `build_turn_request_with_memory` 公共接口 | 新增 5/5＋原 prompt 6/6 |
| 真实轮生命周期 | 101，缺 `UserWithMemory` 模式 | 新增 6/6＋M5 phase2 40/40，三请求快照/下一轮刷新/入史回调实证 |

上述 RED 均为预期接口或行为缺失，非测试笔误；未删改断言迎合实现。

## 4. comprehensive-testing

首轮补强 **18/18 通过，0 失败/忽略**，退出码 0。契约对应：A01 四开关组合及真实配置保存装载；A02 缺失/空白/非法 UTF-8/目录读取失败；A03 固定黄金指引、角色/工具/配对/无注入字节一致；动态中文/四字节字符首尾截断及 9,999/10,000/10,001 字节边界；有/无摘要轮快照、下一轮刷新；真实 snapshot 保存＋Session::resume 首轮重读＋历史/M3 隔离；M5 真实 mem_read 合并隔离；真实 exec/rg 命中与未命中经本地 HTTP API 进入下一请求（退出码分别 0/1、均 completed、不伪造输出）；恶意模板文本/路径不二次展开或执行；Linux inotify 证明三个关闭组合零 open/access、开启态有真实事件。

测试临时目录复用既有 RAII 清理，以进程号＋受控计数命名，不依赖时钟或随机源；生产 snapshot 内部临时名有既有时钟实现，但测试不依赖其取值。新增 Linux inotify 证据仅适用于本次 Linux 环境，未冒称 Windows 实测。

**关键错误检测证明：8/8 捕获、8/8 恢复复绿**。在独立源码副本中执行，全部 Cargo 命令显式 `--target-dir /tmp/013-m6-implementation-2rhxgjs6/mutation-target`，未使用共享 `/workspace/target`；副本绿基线 18/18。每项保持测试断言不变：

| 故意破坏的行为 | 检测用例 | 错误时／恢复后 |
|---|---|---|
| 忽略总开关 | `m6_a01_read_switches_and_manual_config_defaults` | 101（0/1）→0（1/0） |
| 忽略 use_memories | 同上（另一独立分支） | 101（0/1）→0（1/0） |
| 关闭态先读再返回 None（输出相同、I/O 不同） | `m6_disabled_switches_do_not_open_or_read_summary` | 101（0/1）→0（1/0） |
| 预算 2,500→2,501 | `m6_budget_boundary_is_retained_bytes_not_whole_block_bytes` | 101（0/1）→0（1/0） |
| 每请求重读替代轮快照 | `m6_round_snapshot_survives_changes_and_next_turn_refreshes` | 101（0/1）→0（1/0） |
| 把自动上下文写入 HistoryItem 并持久化 | `m6_save_resume_reloads_latest_summary_and_never_persists_injection` | 101（0/1）→0（1/0） |
| 合并轮额外读取并注入摘要 | `m6_merge_stays_uninjected_and_keeps_three_restricted_tools` | 101（0/1）→0（1/0） |
| 注入态首条角色改 developer | `m6_d4_only_system_tail_changes_and_none_is_byte_identical` | 101（0/1）→0（1/0） |

均为编译成功后的真实断言失败，未以编译错误或过滤空跑计数；各次精确执行 1 用例，恢复后精确 1 通过。副本三项被破坏源码恢复后与主工作区逐字一致，无未解释的漏检；不凑等价变异数量。完整日志与命令/退出码见临时目录 `mutation-results.json`，本表为持久证据摘要。

**Skill 对照审计**：后续出现的 comprehensive-testing Skill 已只读加载（未修改/暂存保护目录）。按 reference 的 CTFL/CT-MBT/Mutation 章节复核原有契约映射，无新增契约缺口或生产修改：

| 状态/触发 | 必须保持的结果/禁止迁移 | 保护证据 |
|---|---|---|
| 新用户轮，四开关组合 | 三关闭组合不读文件；启用时读 Some/None 快照 | 配置真实装载与 inotify 正反对照 |
| 快照 Some/None → 工具续请求 | 摘要变化/出现/删除不能改变本轮快照 | 两类快照及三请求路径 |
| 完成本轮 → 下一新轮 | 必须重新读取，不能沿用旧缓存 | 下一轮出现/更新摘要 |
| 入史 → 保存 → 恢复 → 新轮 | 自动块不得写入历史；新轮读最新，不污染 M3 | 真实 snapshot/Session::resume/phase1 |
| 读取失败 → 任务继续 | 原请求可发送，无额外目录/库写入 | 空白/坏 UTF-8/目录/缺失及完整请求对比 |
| 内部合并轮 | 禁止进入用户记忆读取状态；保持三工具 | M5 真实读取及隔离变异 |

技术选择：使用确定性多输入不变量检查（非声称完整 PBT 框架）、真实 API/文件/进程集成、既有 CLI/PTY E2E 回归与定点 Mutation。未新增覆盖率指标、全排列用例或无界 fuzz campaign：无新不可信语法解析器，边界/畸形类别及反例证明已覆盖本轮风险。拒绝把读侧版本拒绝、文件硬上限、沙箱、重试/后台并发或供应商角色实测塞入 M6；前两者违背固定上游/既定契约，后者属后续阶段。剩余风险见 §5 安全报告，实际最终命令/结果见 §7。

## 5. Security Testing Gate

按本地 Gate 文档，先核对读取/模板/组装/轮状态/保存恢复/分派源码，再运行加强恶意输入后的 M6 全目标：**18/18，退出码 0**。输入包含模板占位符、边界关闭串、shell 替换、伪造 JSON/developer 字段及终端转义；观察真实 JSON 往返、消息数量/用户原文、probe 文件不存在，以及关闭态文件 I/O/历史与合并隔离。未发现新增可复现安全缺陷；未运行真实模型，不以本地结构断言宣称角色语义等价或提示注入强隔离。

安全报告：`docs/security/reports/2026-10-19-memory-inject-security.md`（按 Gate 要求单独落盘）。继承的全量文件读取再截断、Linux 专有 I/O 证据与 M7 供应商实测边界均在报告登记，不擅自新增读取策略。

开发工具：`rustup component add clippy rustfmt` 真实失败（共享工具链缓存安装错误）；未改全局配置。改从已安装 1.99.0 channel manifest 的固定 URL 下载两组件，SHA256 核对后仅解包至临时证据目录，按当前工具链动态库路径运行；工具报告 rustfmt 1.10.0 / clippy 0.1.99（同 b940084d7e）。两个 M6 新 Rust 文件已格式化并通过 scoped rustfmt check；全库既有格式不在本轮重排。

Clippy 实测退出码 0，范围 `-p terminal-chat --all-targets`。既有告警保留：app lib 的 `apply_patch.rs::manual_find`、`memory/store.rs::type_complexity`、`tui_host.rs::for_kv_map`，旧 lib 测试及各目标复用 `common/pty` 的重复告警，依赖/TUI 告警亦保留。新 `memory_inject` 目标显示的两条是 `common/pty` 重复告警；新增 M6 源码与测试断言位置无新增告警，不冒称整个仓库零告警，不修无关代码。

## 6. code-review

使用 `/home/dev/.claude/skills/code-review/SKILL.md` 的两个并行只读子代理；基点为完整基线 SHA，提交列表为空。按用户禁止提交的要求，以 `git diff <baseline> -- <M6 tracked paths>`＋新增文件原文审查，未把空的三点提交 diff 计作审查。保护改动不进入审查范围；最终门禁与状态回填由主代理完成。

### Standards

硬违规 **0**，baseline smell 判断项 **0**。实现复用既有截断、参数化组装及工具循环，符合 AGENTS.md 复用优先；旧入口保留兼容职责，不构成无价值 Middle Man。测试使用临时目录、固定脚本响应与黄金文件，真实 rg 结果进入后续请求；快照和不入史检查真实请求与保存文件，未发现绑定内部写法的新增断言。本轴仅读取授权差异，未修改文件或运行 Cargo。

### Spec

缺失/部分需求 **0**、范围蔓延 **0**、已实现但错误 **0**。按轮快照含 None，默认 User 和合并轮隔离；2,500 tokens 复用 UTF-8 首尾截断，固定指引不裁；D4 唯一 system 尾段且无段原样、两工具不变。18 用例覆盖预算、开关、保存恢复、M3/M5 隔离及真实 exec/rg/HTTP 回传。模板仅改三条已登记格式说明，读侧不添加版本校验，与固定上游一致。M7 接线/供应商实测不计作缺陷。本轴未修改文件或运行 Cargo。

两轴分别 **0/0 发现，最高问题均无**；无需因审查修改生产代码或放宽断言。

## 7. verify:full

环境：`/workspace`，rustc/cargo 1.99.0 stable，`TZ=UTC`、`LC_ALL=C.UTF-8`，正常目标目录 `/workspace/target`。按下表顺序完成最终验证，所有命令捕获真实退出码；模型边界为本地 mock/固定脚本，不依赖真实密钥。日志前缀位于上述临时证据目录，`final-results.json` 为从原始输出汇总的元数据。

| 命令 | 退出码 | 实际结果 | 日志前缀 |
|---|---:|---|---|
| `cargo test -p terminal-chat --test memory_inject -- --list` | 0 | 列出 18 项、0 benchmarks；仅列举，不计测试通过 | `final-list` |
| `cargo test -p terminal-chat --test memory_inject --test prompt_assembly --test session_cli --test memory_phase1 --test memory_phase2` | 0 | **107 通过/0 失败/0 忽略/0 过滤**（18＋6＋24＋19＋40） | `final-targeted` |
| `cargo test -p terminal-chat --test e2e_acceptance` | 0 | **95/0/0，0 过滤**，真实 CLI/PTY 默认行为回归 | `final-e2e` |
| `cargo test -p terminal-chat --lib --test memory_candidates --test memory_store` | 0 | **168/0/0，0 过滤**（129＋18＋21），配置、恢复/工具循环和存储回归 | `final-affected` |
| `cargo check` | 0 | 0 错误；既有依赖/TUI 告警保留 | `final-check` |
| `cargo test --workspace --no-fail-fast` | 0 | **2944 通过/0 失败/3 既有忽略/0 过滤；其余未运行 0** | `final-workspace` |

对账：M5 已交付基线 **2926/0/3**＋M6 新增 **18**＝**2944/0/3**，无既有用例减少或新增忽略。上述分组和最终全量有重叠，不把重复执行累计为新增数量。全量 45 个结果目标（含无用例的 shim/doc target），M6 18 项实际非零执行。既有忽略项均在 tchat-tui：

- `bottom_pane::textarea::tests::altgr_ctrl_alt_char_inserts_literal`：仅 Windows AltGr 适用。
- `tui::owned_screen_tests::owned_screen_lifecycle_child`：由生命周期父用例启动的子入口。
- `tui::panic_tests::caught_panic_pty_child`：PTY 子入口。

静态补充检查（已在最终验证前完成）：`env LD_LIBRARY_PATH=/usr/local/rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib /tmp/013-m6-implementation-2rhxgjs6/quality-tools/bin/cargo-clippy clippy -p terminal-chat --all-targets`＝exit 0（既有告警见 §5）；使用同一 `LD_LIBRARY_PATH` 的同目录 `rustfmt --edition 2021 --config skip_children=true --check src/app/src/memory/inject.rs src/app/tests/memory_inject.rs`＝exit 0，范围仅两个新增 Rust 文件，全库 fmt 未运行。

兼容性：本次未更改 `HistoryItem`、会话格式或模型协议 Schema，`use_memories` 以 serde 默认 true 保持旧 manual.json 可装载并真实往返；配置、默认请求逐字节和会话恢复已回归。项目无独立的已发布 Rust API/IDL 兼容性门，未新增 buf/semver 等检查工具，也不宣称已执行。

最终验证起点已保存 12 项源码/模板/测试/锁文件/驱动指纹（`final-source.json`，manifest SHA256＝`14780f5aeb0f64f99c871c81c510ab6154025ad5814b2c3667f487e6bc06f66c`）；终检全部相同。其后仅本规格/实施记录等文档回填，无需无信息重复全量。限制与未实施项见安全报告：先全量读文件再截断的继承边界、Linux 专有 inotify 证据，以及 M7 两驱动接线/后台/供应商实测、M8 引用反馈、M9 清理/整链均不冒称完成。

## 8. 保护项观察

初始四项保护共 7 个节点（含根目录）逐项核对内容/文件大小/模式相同：compose 保留既有 +10 行，2026-10-02 记录保留既有 +33 行，`db/wiki.db` 只作字节指纹读取、未打开数据库，安全门禁源文件未改。过程中 `.agents/skills/comprehensive-testing/{SKILL.md,reference.md}` 与两层目录新增出现，4 个观察节点已另存指纹；本任务未写入或安装，终检仍原样，不纳入候选。

终检另发现其他并发工作：`AGENTS.md` 增补核心/前端分离与复用优先两条原则；`docs/Agent开发核心宗旨.md` 已被删除；新增 `docs/records/2026-10-18-codex-global-agents-skills-install.md`。本任务均未写入、删除或暂存，按实际状态保留，观察指纹另存 `external-work-observed.json`，不计入 M6。已重读 AGENTS.md，新内容未改变本轮门禁；已验证源码/模板/测试不变，未因此重跑无信息测试。

索引核对如实区分文件字节与暂存内容：初始 `.git/index` SHA256 为 `93dfba47…`，终检为 `6457c1c2…`，首次严格字节指纹检查因此失败；没有据此重置或覆盖索引。进一步逐项比对 **1437 条 mode/OID/stage 全部等于 HEAD 树**，无 unmerged、`git diff --cached` 为空；确认实际暂存内容未变，不宣称索引文件字节原样。并发来源无法仅凭哈希归因；内容级核对与保护项/源码核对完成后，终检 exit 0（`final-audit.json`）。

最终 Git：当前 `feat/013-memory-m6`，HEAD＝main＝merge-base＝`1762958fb01b60673e681f3d42006b8df8b19d5e`，领先/落后 0/0，1 个工作树，暂存区/stash 为空。013 初始 23 条历史修订原文均保留，M6 准备/D4 批准边界保留为历史；未自动 stash、clean、reset 或覆盖其他工作。

M6 可审阅候选严格为以下 **12 文件**（暂存区为空，新增文件仍 untracked）：

| 类别 | 文件 |
|---|---|
| 实现/模板（6） | `src/app/src/memory/{config.rs,mod.rs,inject.rs,templates/read_path.md}`、`src/app/src/{prompt.rs,turn.rs}` |
| 测试/黄金（3） | `src/app/tests/{memory_candidates.rs,memory_inject.rs}`、`src/app/tests/fixtures/memory_inject/read_context.md`（原候选测试只补配置字段，断言不变） |
| 规格/证据（3） | `docs/specs/013-memory-system-v1.md`、本记录、`docs/security/reports/2026-10-19-memory-inject-security.md` |

审阅补丁输出到临时证据目录 `final-candidate.patch`，以完整基线为起点，包含 013 既有未提交准备/批准登记及上述新增文件；所有保护项和并发外部工作均排除。最终文档 `git diff --check` 实测 exit 0（`final-diff-check.log`）；最终报告引用同一份证据，不创建重复计划。本轮未提交/合并/推送/删除分支，未调用真实模型、未访问真实记忆目录。

## 9. 批准交付与合并核验（2026-10-19）

用户本轮明确批准 M6 交付：精确提交实现、快进合并 main、核验后安全删除任务分支；不推送、不发布、不操作远程分支、不调用真实模型、不触碰真实记忆目录、不开始 M7。

- **交付前核验**：分支 `feat/013-memory-m6`，HEAD＝main＝`1762958fb01b60673e681f3d42006b8df8b19d5e`（快进前提成立），暂存区/stash 为空、无进行中合并。
- **候选与最终验证对应**：候选 12 路径以 `final-candidate-manifest.json`（补丁 SHA256＝`16d688ee…fe1667`）逐项确定，12 文件工作区 SHA-256 与清单逐一相符，`git apply --check --reverse final-candidate.patch` 通过（工作区＝补丁后状态）；12 项中有 9 项源码/模板/测试与 §7 验证起点 `final-source.json` 指纹逐一相符，其余 3 项（Cargo.lock、`main.rs`、`tui_host.rs`）核验时亦相同——提交代码与最终验证（`cargo check` 0 错误、全量 **2944/0/3**，日志独立求和 45 套件复核）同态，其后仅 .md 文档回填。规格中拟交付的 M6 状态行在实现提交内仍为「待批准」，交付标记留待本节对应文档提交翻转。
- **精确暂存**：12 路径逐项显式 `git add`（无 `add .`/`add -A`）；暂存清单恰为批准候选（6 新增＋6 修改，1434+/22-），12 个暂存 blob SHA-256 与候选清单逐一相符；未暂存剩余恰为保护/并发内容（compose、AGENTS.md、宗旨删除、2026-10-02 记录、`.agents/`、`db/`、2026-10-18 记录）；`git diff --cached --check` exit 0。
- **M6 实现提交**：`2aa000233536a2b9ceab2858344374207d6a94f9`，`feat: 013-M6 记忆摘要注入`，树 `7b26aeca030c81cf351a29b6d22d09bf0298c172`，父提交＝基线 `1762958…`；提交树内 12 文件 blob SHA-256 与候选清单逐一复核一致，保护项零进入；提交后工作区剩余差异仅为上述保护/并发内容、stash 为空。
- **快进合并**：`git checkout main`（工作区保护修改随切换原样保留）→ `git merge --ff-only feat/013-memory-m6` 成功（Fast-forward，1762958→2aa0002）；`git merge-base --is-ancestor 2aa0002… main` exit 0；`git diff --exit-code 2aa0002… main --` 为空——main 文件树＝M6 提交树，快进未引入候选外代码变化，按授权复用 §7 最终验证（2944/0/3），不机械重跑。
- **保护项与并发改动核对**：原始四项（`.devcontainer/compose.yaml` +10 行、`docs/records/2026-10-02-install-mattpocock-skills.md` +33 行、`db/` 含 wiki.db 仅字节指纹未打开、`.agents/`）及全部并发改动（`AGENTS.md` SHA256 `1ae4f024…`、宗旨文档删除、`docs/records/2026-10-18-codex-global-agents-skills-install.md` SHA256 `7131b57d…`、`.agents/skills/comprehensive-testing/` 两文件指纹同 `additional-protected.json`）全程原样、未入任何提交；与 §8 登记一致。
- 交付文档更新（本记录 §9＋规格状态翻转）单独提交 `docs(013): M6 标记为已交付`；任务分支随后按 `git branch -d`（包含关系前置核验已过、不强制删除）安全删除，main 最终 SHA 以本轮最终 Git 核验为准。M7–M9 保持未实施、V1 整体未完成、D4 及其他决策状态不变。
