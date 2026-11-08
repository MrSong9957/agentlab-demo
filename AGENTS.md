# AGENTS.md — 项目规则

## 1. 项目原则

- 使用中文 ASD-STE100 标准输出。
- 默认采用最小实现和必要验证。不得删减需求。不得自动增加功能、通用接口、未来兼容或性能优化。
- 优先复用经过验证的开源成熟方案，只实现必要部分。通过调研核实适用性，不得仅凭模型记忆或仓库星数选择方案。
- 用户确认关键需求和取舍。Agent 自行调查仓库事实，在授权范围内决定普通实现细节。不得重复澄清已明确的事项。

## 2. 项目文档规范

开发前，读取并遵守 `rules/规格文档.md`。代码审查前，读取并遵守 `rules/REVIEW.md`。

功能规格和证据记录与代码存放在同一仓库，统一使用 `docs/`：

| 目录 | 要求 |
|---|---|
| `docs/specs/` | 一个功能一份规格，不合并不同功能。文件名采用 `编号-功能英文名.md`，例如 `001-daily-checkin.md`。 |
| `docs/logs/` | 存放第 7 节规定的设计依据和验证证据。 |

实现前，编写或更新并确认规格。规格必须明确目标、输入输出、边界、约束和可观察的验收结果。需要用户决定的关键取舍，必须在实现前取得确认。

已有规格时，更新原文件。简单任务按现有模板简写，不建重复文档。规格保留调研汇总及各专家原始输出的链接。

实现变化后，更新相关规格、验收实测、文档和注释。局部实现原因写在代码附近。将草稿中的有效结论提炼进正式文档。

## 3. 开发流程

### 共同准备

两条路径都先完成以下阶段：

```text
ce-brainstorm（按需）
→ 计划前调研
→ 编写／更新并确认规格
→ 计划
→ 由 Agent 准备任务分支
```

- `ce-brainstorm`：仅当用户表示没有想好需求，或你判断目标、预期行为、关键取舍需要澄清时使用。需求清楚时直接调研。仓库事实问题由 Agent 调查。
- 计划前调研：两条开发流程都按 `rules/计划前调研.md` 执行。查阅官方文档、上游实现和项目现状，选择适合当前约束的最佳实践。结论附出处，区分事实与推断。资料不足时说明。调研不自动扩大功能，额外调查须能解决当前决策中的具体疑问。
- 规格文档：一个功能对应一份规格，明确本轮要求和可观察的验收结果，并随实现更新。关键取舍确认后再实现。
- 计划：调研后，使用 `ce-plan` 生成执行计划，明确必要步骤和验证方法。

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

- 疑难问题使用 `ce-debug`。同一方法反复失败时，先补充证据，再继续处理。
- `comprehensive-testing` 覆盖规格验收、边界、异常和受影响回归，检查实际结果、状态及副作用。
- 已识别安全风险或新增安全敏感面时，执行 `Security Testing`。执行或跳过都说明依据；执行时必须通过。
- 安全测试通过或确认不适用后，执行 `ce-simplify-code`。简化改变代码时，先重新运行受影响检查，包括相关安全检查，再进入代码审查。
- 最终执行全量验证和其他适用检查。之后再改影响结果的代码时，重新运行受影响检查和受影响范围的局部验证。

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
| typecheck／lint | 容器内 `/workspace`：typecheck 用 `cargo check`；`cargo clippy`／`cargo fmt` 当前工具链未安装，用前需 `rustup component add clippy rustfmt` | typecheck 实测通过；clippy/fmt 需先补装组件。 |
| 兼容性及其他规定检查 | 审查用 `ce-code-review` 技能并读取 `rules/REVIEW.md`；安全检查按 `.agents/agents/Security-Testing-Gate.md`；CI 用 `.github/workflows/ci.yml`（三平台矩阵，复用全量验证入口加 `--locked`）。 | 对外接口兼容性检查不适用：纯 Rust CLI，无对外 Schema/Proto 接口。CI 细节见 `docs/specs/038-github-actions-ci.md`。 |
| 快照更新 | 不适用：项目未使用快照测试框架，无既有快照基线。 | 不适用。 |
| 全量验证 | 容器内 `/workspace`：`cargo check && cargo test --workspace --no-fail-fast`，并报告通过、失败、跳过与未运行数量。 | 已包含：cargo check＋单元＋集成＋回归全集。须独立执行：REPL 交互行为改动按 001 规格补伪终端验收。环境前提：全局 `CARGO_TARGET_DIR=/workspace/target`，工作区副本或隔离目录运行 Cargo 须显式 `--target-dir`，否则污染共享构建产物。 |

全量验证使用一个命令入口，从仓库根目录非交互运行。无参数时执行约定范围内的全部单元、集成和回归用例，支持按名称过滤排查。用退出码和输出报告结果。全量范围不自动扩展为整个 monorepo。

## 6. 参考资料与缺失处理

- Obsidian 仓库：宿主机 `D:\AgentLab\shared\Obsidian`，Docker 容器 `/vault`。通过 wiki MCP 保存记忆文档和可沉淀的知识。
- Obsidian GitHub 仓库：[MrSong9957/Obsidian](https://github.com/MrSong9957/Obsidian)，保存个人知识、常用技能和 Agent 代理。

规则或技能缺失时，先查项目和参考资料，按需将技能安装到 Agent 工具全局目录。仍不可用时，说明缺失内容和替代方法。
