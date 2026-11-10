# AGENTS.md — 项目规则

## 1. 项目原则

- 用简化技术规范（ASD-STE100 中文版）解释或写作。
- 默认采用最小实现和必要验证。不得删减需求，或自动增加功能、通用接口、未来兼容、性能优化。
- 优先复用经过验证的成熟开源方案，只实现必要部分。通过调研核实适用性，不得仅凭模型记忆或仓库星数选择。
- 用户确认关键需求和取舍。查清仓库事实，在授权范围内决定普通实现细节。已明确的事项不得重复澄清。

## 2. 项目文档

写计划前，读取并遵守 `rules/规格文档.md`。代码审查前，读取并遵守 `rules/REVIEW.md`。

功能规格和证据记录与代码同仓，统一存放在 `docs/`：

| 目录 | 要求 |
|---|---|
| `docs/specs/` | 一个功能一份规格，不合并不同功能。文件名为 `编号-功能英文名.md`，如 `001-daily-checkin.md`。 |
| `docs/logs/` | 存放设计依据和验证证据。 |

实现前按模板编写或更新规格，并由用户确认。规格写清本轮目标、输入输出、边界、约束和可观察的验收结果；已有规格更新原文件，不建重复文档。简单任务可简写，但约束内容必须完整。

规格与已确认的要求一致，沿用既有名称和说法，必要术语就地解释。写清触发条件、执行动作和可观察结果，不用抽象术语代替具体行为。不得擅自增删要求、改变取舍，或把未确认问题写成结论。用户只看规格，即可核对是否忠实表达了讨论结果。

规格保留调研汇总及各专家原始输出的链接。实现变化后，更新相关规格、验收实测、文档和注释。局部实现原因写在代码附近。将草稿中的有效结论提炼进正式文档。

## 3. 开发流程

Agent 自主选择路径，默认最小路径。所有开发任务，包括简单任务，都必须完成共同准备。

### 共同准备

```text
ce-brainstorm（按需）
→ 计划前调研
→ 讨论
→ 编写／更新规格文档并由用户确认
→ 计划并经用户确认
→ Agent 准备任务分支
```

- 用户未想好需求，或目标、预期行为、关键取舍不清时，使用 `ce-brainstorm`；需求清楚时直接调研。
- 计划前调研：按 `rules/计划前调研.md` 执行。查阅官方文档、上游实现和项目现状，选择符合当前约束的最佳实践。结论附出处，区分事实与推断；资料不足时说明。不得自动扩大功能；额外调查须解决当前决策的具体疑问。
- 讨论：与用户共定方案，只谈核心问题。普通执行由 Agent 负责，已约定的要求不得简化。新增功能先调研再讨论；每个执行间隔先汇报完成的工作，再定下一步。
- 规格：按第 2 节执行。
- 计划：用 `ce-plan` 写计划并落盘，结构如下；经用户确认后才开始实现。
  - 整体级：本轮范围与不做的部分；步骤清单，每步一个模块或一个函数；必要的依赖及原因。
  - 每步：改动对象与不碰的部分；实现前写入测试场景（场景、给定与输入、期望输出／状态／报错、测试文件与用例名、按名称过滤的命令），至少覆盖适用的正常、边界、异常；实现要点一两句，取舍引用调研结论；完成定义为测试通过并逐条对照期望。
  - 替代规则：重构步不新增场景，行为变化按新行为补场景；无测试可能的改动写明替代验证（命令与可观察结果）。
  - 不写入：完整代码、调研过程、未来兼容、与规格重复的目标。简单任务的计划可简写，但范围、步骤、测试场景（含输入与期望）不得省略。

### 最小路径

```text
ce-work
→ ce-simplify-code
→ ce-code-review
→ 最小验证
→ 返回候选版本
```

### 完整路径

```text
ce-work
→ comprehensive-testing
→ Security Testing（适用时）
→ ce-simplify-code
→ ce-code-review
→ 全量验证和修复
→ ce-compound
→ 返回候选版本
```

- 执行按计划小步推进：可测步骤先写测试、确认失败，再实现、确认通过；通过该步验证后再做下一步，不得攒到最后一起验证。每步改动保持可审查、可回退。
- 疑难问题使用 `ce-debug`。同一方法反复失败时，先补充证据，再继续处理。
- `comprehensive-testing` 覆盖规格验收、边界、异常和受影响回归，检查实际结果、状态及副作用。
- 两条路径都须按风险执行安全测试：已识别安全风险或新增安全敏感面时，执行 `Security Testing`。执行或跳过均说明依据；执行时必须通过。
- 安全测试通过或确认不适用后，才执行 `ce-simplify-code`。简化改变代码时，先重跑受影响检查，包括相关安全检查，再进入代码审查。
- 按所选路径完成最终验证及其他适用检查。之后再改影响结果的代码时，重跑受影响检查和受影响范围的局部验证。

## 4. 场景与验证方法

| 场景 | 验证方式 |
|---|---|
| Web | 启动真实项目，用无头浏览器模拟操作，检查页面结果和相关数据。 |
| 普通 CLI | 启动真实进程，检查标准输出、错误输出、退出码和文件变化。 |
| 交互式 CLI / TUI | 用伪终端模拟输入和按键，检查渲染后的屏幕、光标和状态。Windows 使用 ConPTY，优先采用 `node-pty` 和 `@xterm/headless`。 |
| 终端缩放、滚动历史等宿主行为 | 在目标终端补充验证。模拟屏幕通过仅算部分证据。 |
| Bug 修复 | 在最低可复现层增加或更新回归测试，确认修复前失败、修复后通过。 |
| 行为或接口变更 | 补充或复用回归测试；涉及对外接口、Schema 或持久化数据时，执行适用的兼容性检查。 |
| 关键测试或高风险模块 | 保持断言不变，用前后对照或临时破坏被测行为，证明测试能检出错误。已有有效证据时不重复证明。高风险模块按需做变异测试。 |
| 完整、稳定的输出 | 使用聚焦快照或黄金文件。先检查差异，确认符合需求后才更新预期，并说明原因。 |
| 外部 API 或网络服务 | 按需录制并回放响应，使测试可离线复现，不依赖真实密钥。去除录制内容中的敏感信息。 |
| 并行、随机或时间相关测试 | 隔离环境，固定影响断言的输入，保留失败种子或输入。时间逻辑使用可控时钟；真实计时设置超时和容差，不断言精确耗时。 |
| CI | 核对触发事件、分支和平台。设置并核验过时任务取消机制，避免重复触发。重跑前检查目标提交已有结果，优先补跑失败或未完成任务。不得用旧任务验证新版本。用日志核对耗时、缓存命中和保存结果。超时先区分正常耗时与卡死。 |
| 静态质量检查 | 按改动范围执行项目已有的类型检查、代码检查及确定性检查。 |

缺少工具时，先尝试自动化替代方案，不得直接转交用户手测。仍无法验证时，说明未验证范围、原因和影响，不得计为通过。

## 5. 项目信息

在实际仓库中核实并填写本轮相关项，不得猜测命令。不适用时注明原因。

- 技术栈、运行时、包管理器：Rust CLI `terminal-chat`（edition 2021，rustup stable rustc/cargo 1.99.0，Cargo，Cargo.lock 锁版本）；纯 Rust 项目。
- 核心入口与模块边界：根 Cargo.toml 为 workspace 入口、默认成员 `src/app`（主程序 crate）；TUI 渲染层在 `src/tui/`＋`src/shims/`＋`src/vendor/`；`src/app/src/lib.rs` 导出 24 个模块（api、apply_patch、catalog、context、context_window、display、exec、mcp、memory、message_to_user、plan、proc_session、prompt、sandbox、permissions、session、turn、user_input、user_input_async、tui_host、tool_search、view_image、js_orchestration、agents），`src/app/src/main.rs` 是终端循环与命令入口；模块职责与目录迁移背景见 `rules/project-lessons.md`。
- 必须保持的行为，如接口、配置和已有数据兼容性：已验收契约覆盖 REPL 回显与退出语义（001）、`config/` 配置与 `/refresh`（002）、exec_command/apply_patch 执行契约（003/004，016 修订：可续接进程会话）、tool_calls `id`/`name` 原样保留（006，017 起用户轮四工具＋update_plan）、进程会话生命周期与取消/时限/容量边界（016，已交付）、会话格式与锁语义（007）、TUI 按键单一职责（010/012）、凭证与输入安全边界、记忆存储位置与 V1 范围（013/014）；契约明细与来源见 `rules/project-lessons.md` §4，行为变更须先核对对应规格。
- 测试夹具与快照位置、测试实际使用的路径环境变量：集成与 E2E 测试在 `src/app/tests/`，共享辅助在 `src/app/tests/common/`，输入夹具在 `src/app/tests/fixtures/`；项目未使用快照测试框架。构建与测试受全局 `CARGO_TARGET_DIR=/workspace/target` 影响，在工作区副本或隔离目录运行 Cargo 须显式 `--target-dir`。

| 用途 | 命令与工作目录 | 覆盖范围与适用条件 |
|---|---|---|
| 启动／构建 | 宿主机起容器：`docker compose -f .devcontainer/compose.yaml up -d --build`；容器内 `/workspace`：构建 `cargo build --locked`，交互运行 `cargo run`，管道运行 `echo hello \| cargo run --quiet`；进入容器：`docker exec -it agentlab-demo-dev bash` | devcontainer 环境启动与容器内构建、运行。 |
| 单元／集成测试及单项排查 | 容器内 `/workspace`：`cargo test`；按名称过滤单个用例：`cargo test <用例名>` | 单元＋集成测试（mock HTTP）。过滤运行仅用于排查，不能代替完整路径的交付验证。 |
| E2E | 容器内 `/workspace`：`cargo test --test e2e_acceptance` | 真实 CLI＋PTY＋本地 mock；覆盖范围与记录见 `docs/records/2026-10-09-e2e-acceptance.md`。 |
| typecheck／lint | 容器内 `/workspace`：优先 `cargo clippy --all-targets -- -D warnings`；clippy 不可用时兜底 `cargo check`。`cargo fmt` 当前未安装，用前需 `rustup component add fmt`（即 `rustup component add rustfmt`） | clippy 覆盖 check 全部诊断，装了就不再单独跑 check；clippy/fmt 需先补装组件。 |
| 兼容性及其他规定检查 | 审查用 `ce-code-review` 技能并读取 `rules/REVIEW.md`；安全检查按 `.agents/agents/Security-Testing-Gate.md`；CI 用 `.github/workflows/ci.yml`（三平台矩阵，复用全量验证入口加 `--locked`）。 | 对外接口兼容性检查不适用：纯 Rust CLI，无对外 Schema/Proto 接口。CI 细节见 `docs/specs/038-github-actions-ci.md`。 |
| 快照更新 | 不适用：项目未使用快照测试框架，无既有快照基线。 | 不适用。 |
| 全量验证 | 容器内 `/workspace`：`cargo clippy --all-targets -- -D warnings && cargo test --workspace --no-fail-fast`（clippy 不可用时以 `cargo check` 替代前半段），并报告通过、失败、跳过与未运行数量。 | 已包含：静态检查（clippy/check）＋单元＋集成＋回归全集。须独立执行：REPL 交互行为改动按 001 规格补伪终端验收。环境前提：全局 `CARGO_TARGET_DIR=/workspace/target`，工作区副本或隔离目录运行 Cargo 须显式 `--target-dir`，否则污染共享构建产物。 |

全量验证使用一个命令入口，从仓库根目录非交互运行。无参数时执行约定范围内的全部单元、集成和回归用例，支持按名称过滤排查。用退出码和输出报告结果。全量范围不自动扩展为整个 monorepo。

## 6. 参考资料与缺失处理

- Obsidian 仓库：宿主机 `E:\Files\Obsidian\`，Docker 容器 `/vault`。通过 wiki MCP 保存记忆文档和可沉淀的知识。
- Obsidian GitHub 仓库：MrSong9957/Obsidian，保存个人知识、常用技能和 Agent 代理。

规则或技能缺失时，先查项目和参考资料，按需将技能安装到 Agent 工具全局目录。仍不可用时，说明缺失内容和替代方法。