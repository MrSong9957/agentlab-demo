# 项目规则

## 1. 项目原则

- 中文交流。
- 结合项目实际，优先复用经过验证的开源成熟方案（如 GitHub 高星仓库），只实现无法复用的最小部分。

## 2. 项目文档规范

开发前读取 `rules/规格文档.md`；代码审查前读取 `rules/REVIEW.md`，按对应文档执行。

功能规格和验证日志与代码同仓库，统一放在 `docs/`：

- `docs/specs/`：功能规格。一个功能一份，不合并不同功能。文件名采用 `编号-功能英文名.md`，如 `001-daily-checkin.md`。
- `docs/logs/`：证据记录。Agent 按需自主收集并记录证据，只保留关键设计依据、测试要点、失败原因和验证结果。最终报告引用同一份证据，不重复抄写。
- `docs/solutions/`：已文档化的历史问题解法（bug、最佳实践、工作流模式），按类别组织并带 YAML frontmatter（module、tags、problem_type）；在相关区域实现或调试时可检索复用。
- After a solved, verified problem, automatically invoke the `ce-compound` skill with `mode:non-interactive` at the completion checkpoint only when the work produced durable project reasoning that is not readily recoverable from the final code, tests, types, comments, or existing documentation, and losing it would plausibly cause recurrence, material risk, or substantial rediscovery. Apply this counterfactual: if the learning document disappeared, would a future engineer reading the final implementation still be likely to repeat the mistake or redo substantial investigation? If not, do not invoke it. Completion, effort, and diff size alone are not enough. Capture at the checkpoint so a qualifying learning can ship in the PR that produced it, and only where the repository treats captured learnings as tracked, committed knowledge.
- 局部实现原因写在对应代码附近的注释中。
- 代码变更影响文档或注释时，同步更新对应内容。
- 临时草稿中的有效结论应按上述分工提炼进正式文档。

## 3. 调研与计划

- 调研：@`rules\计划前调研.md`，在编写计划前，结合任务及规格文档进行计划前的调研。调用结束后，规格文档中必须要有指向汇总文档和各专家原始输出的链接.
- 计划：使用 `ce-plan` 技能，结合规格文档、调研产物等任务相关文档编写计划。

## 4. 测试与验证流程

以下流程及第 5～7 节只适用于代码或程序行为变更；仅修改文档且不影响程序行为时，只检查内容、命名和链接，不执行代码检查和全量测试。

### 开发与验证流程

- 修改前阅读相关实现、邻近测试、测试说明和 CI 配置，复用现有框架、夹具及工具，确认预期行为和第 8 节的检查命令。行为变更、缺陷修复或重构前，运行最小相关测试确认基线。
- 按顺序执行：`ce-work` → `comprehensive-testing` → `Security Testing` → `ce-simplify-code` → `ce-code-review` → 全量验证和修复 → `ce-compound`。
- 任一阶段发现错误时，使用 `ce-debug` 定位根因，修复本次引入或与任务直接有关的问题；无关已有问题单独报告，不顺手修复。修复后复验受影响的检查，最终完成第 8 节规定的全量验证。

### 验证范围与完成条件

- 修改后先验证直接受影响的行为；公共或跨模块逻辑覆盖相关调用方与集成测试，交互流程运行相应 E2E。
- 单元、集成及回归用例统一由仓库根目录的一条命令非交互运行，无需人工确认或手测，以退出码和标准输出报告结论；默认无参数执行全部用例，支持按名称过滤排查。
- 局部或过滤运行仅用于开发排查，不能替代交付验证。交付前须执行第 8 节的全量验证及其他适用检查；规定检查通过且满足第 5 节的测试质量要求，才能报告验证完成。
- 完成规定流程和检查后，仅因新增改动、环境变化、失败或未解疑点扩大范围或重跑，不重复运行没有新增信息的检查。全量入口已包含的检查不另行重复执行。

## 5. 用例设计与测试质量

验证真实行为，不绑定与需求无关的内部实现；明确要求的架构、接口和数据约束仍须检查。

- 按“准备输入 → 执行动作 → 检查结果”组织测试。输入有代表性、可复现，覆盖关键行为、边界和错误处理；按需检查输出、状态及副作用，确认目标路径确实执行，防止空跑。不为覆盖率或形式完整增加重复、照抄实现的测试。
- 优先真实代码和组件。Mock（测试替身）仅用于外部服务、文件系统等系统边界，不替代本次要验证的核心逻辑、状态变化或业务判断。
- Bug 修复须在能稳定复现问题的最低层增加或更新回归测试，确认修复前失败、修复后通过；无法自动复现时报告验证缺口。行为和接口变更须有对应回归覆盖，已有用例充分覆盖时可复用，不机械新增。回归用例按验证目标归入单元、集成或 E2E，不作为独立测试层级。
- 关键新增或回归测试须证明能发现错误：保持断言不变，通过修复前后对照或临时破坏被测行为，确认错误时失败、恢复正确行为后通过。已有有效对照证据时，不重复证明。
- 特定字段或行为直接断言；需要核对完整稳定输出时，可用小而聚焦的快照或黄金文件。失败先查差异，确认变化符合需求后才更新预期并说明原因。不得仅为通过检查而删除或跳过测试、削弱断言、绕过检查或接受未经审查的快照。
- 高风险模块按需做变异测试（故意改坏代码，检查测试能否发现）。与本任务有关却未被发现的错误须调查；不改变实际行为的变异可说明后排除，不追求 100% 分数。

### 单元、集成与端到端测试

单元测试检查输入输出、状态变化、边界和异常；集成测试检查模块间的数据流、接口调用、状态同步和外部交互。

E2E 覆盖关键用户流程，不替代低层测试。主要检查用户最终看到的结果，日志仅辅助定位和交叉验证。

| 场景 | 验证方式 |
|---|---|
| Web | 启动真实项目，用无头浏览器（如 Playwright）模拟操作，检查页面结果和相关数据。 |
| 普通 CLI | 启动真实进程，检查标准输出、错误输出、退出码和文件变化。 |
| 交互式 CLI / TUI | 使用伪终端模拟输入和按键，检查渲染后的屏幕、光标和状态。 |

- Windows 交互测试使用 ConPTY，优先用 `node-pty` 启动真实进程、`@xterm/headless` 解析 ANSI 输出并检查屏幕状态。
- 涉及终端缩放、滚动历史等宿主终端行为时，增加目标终端验证；模拟屏幕通过仅为部分证据。
- 不得仅因缺少 `tmux`、`script` 等工具就让用户手测；应尝试适用的自动化替代方案，仍无法验证时报告缺口。

## 6. 环境隔离与可复现性

- 测试可独立运行，不依赖执行顺序或其他测试留下的数据，并行时互不影响。
- 涉及用户目录、配置、缓存或文件写入时，将实际使用的相关路径重定向到独立临时目录，结束后清理；禁止读写真实用户配置、缓存和数据。通过参数、依赖注入或被测子进程配置设置路径、环境变量和工作目录，不修改并行测试共享的进程级状态。
- 控制会影响断言的因素：普通测试固定随机输入或种子；随机生成测试保留可复现失败的种子或输入；时间逻辑使用固定输入、可替换时钟或虚拟时间；相关时区和 locale 显式指定。真实计时测试设置超时和合理容差，不断言精确耗时。
- 使用仓库现有 lockfile，工具支持锁定模式时启用，如 Cargo 的 `--locked`。常规验证不升级依赖或改写锁文件；任务确需变更时，同步更新依赖声明和锁文件，并验证受影响范围。
- 外部 API、网络服务按需使用 VCR（录制并回放响应）或等价方案，使测试可离线运行、响应固定可复现、不依赖真实密钥。录制的请求和响应须去掉密钥、令牌等敏感信息。

## 7. 静态质量与验证证据

- 按第 8 节已核实的命令执行项目适用的 typecheck（类型检查）和 lint（代码检查），不引入未使用代码、未处理异步调用等明显问题。
- 纳入项目已有的架构依赖、契约、文档示例执行、内存或未定义行为等确定性检查；项目没有的不强加。
- 涉及对外接口、Schema 或持久化数据时，执行项目适用的兼容性检查，如 `buf breaking`、`api-extractor`、`cargo semver-checks`；不适用的项目在第 8 节注明原因。
- 验证证据按第 2 节记录于 `docs/logs/`，包含实际命令、工作目录、覆盖范围和关键结果；最终报告引用同一记录，简要概括结论，不重复抄写日志。
- 区分通过、失败、未执行及环境阻塞，标明已有与新增失败；无法确认来源时明确说明。未执行、跳过或阻塞不得计为通过，局部通过不得表述为全部通过。
- 验证缺口说明原因、未覆盖范围及其对结论的影响；需要用户操作时，写清具体事项。

## 8. 项目信息

在实际项目中核实后填写，不猜测命令；不适用的项目注明原因。

- 技术栈、运行时、包管理器：Rust CLI `terminal-chat`（edition 2021，rustup stable rustc/cargo 1.99.0，Cargo，Cargo.lock 锁版本）；纯 Rust 项目。
- 核心入口与模块边界：根 Cargo.toml 为 workspace 入口、默认成员 `src/app`（主程序 crate）；TUI 渲染层在 `src/tui/`＋`src/shims/`＋`src/vendor/`；`src/app/src/lib.rs` 导出十三个模块（catalog、api、prompt、turn、exec、proc_session、apply_patch、plan、session、memory、display、tui_host、agents），`src/app/src/main.rs` 是终端循环与命令入口；模块职责与目录迁移背景见 `rules/project-lessons.md`。
- 必须保持的行为，如接口、配置和已有数据兼容性：已验收契约覆盖 REPL 回显与退出语义（001）、`config/` 配置与 `/refresh`（002）、exec_command/apply_patch 执行契约（003/004，016 修订：可续接进程会话）、tool_calls `id`/`name` 原样保留（006，017 起用户轮四工具＋update_plan）、进程会话生命周期与取消/时限/容量边界（016，已交付）、会话格式与锁语义（007）、TUI 按键单一职责（010/012）、凭证与输入安全边界、记忆存储位置与 V1 范围（013/014）；契约明细与来源见 `rules/project-lessons.md` §4，行为变更须先核对对应规格。

| 用途 | 命令与工作目录 |
|---|---|
| 启动 / 构建 | 宿主机起容器：`docker compose -f .devcontainer/compose.yaml up -d --build`；容器内 `/workspace`：构建 `cargo build --locked`，交互运行 `cargo run`，管道运行 `echo hello \| cargo run --quiet` ；进入容器：`docker exec -it agentlab-demo-dev bash` |
| 单元 / 集成测试 | 容器内 `/workspace`：`cargo test`（单元＋集成，mock HTTP） |
| 回归测试（一条命令全量运行） | 容器内 `/workspace`：`cargo check && cargo test --workspace --no-fail-fast`（与「全量验证」同一条命令，覆盖单元＋集成＋回归全集）；支持按名称过滤单个用例排查，过滤结果不能作为交付验证依据 |
| E2E | 容器内 `/workspace`：`cargo test --test e2e_acceptance`（真实 CLI＋PTY＋本地 mock；覆盖范围与记录见 `docs/records/2026-10-09-e2e-acceptance.md`） |
| typecheck / lint | 容器内 `/workspace`：typecheck 用 `cargo check`（实测通过）；`cargo clippy`/`cargo fmt` 当前工具链未安装，用前需 `rustup component add clippy rustfmt` |
| 审查 / 安全检查 / CI | 审查用 `ce-code-review` 技能并读取 `rules/REVIEW.md`；安全检查按 `.agents/agents/Security-Testing-Gate.md`；仓库内未发现 CI 配置 |
| 全量验证 | 容器内 `/workspace`：`cargo check && cargo test --workspace --no-fail-fast`，并报告通过、失败、跳过与未运行数量（定向测试不能代替最终全量回归）；REPL 交互行为改动另按 001 规格补伪终端验收。注意本环境全局 `CARGO_TARGET_DIR=/workspace/target`——在工作区副本/隔离目录里跑 Cargo 时必须显式 `--target-dir` 指向副本专用目录，否则污染共享构建产物 |

## 9. 参考资料

- Obsidian 仓库：宿主机 `E:\Files\Obsidian`，Docker 容器 `/vault`。记忆文档和可沉淀的知识通过 wiki MCP 保存到此处。
- Obsidian GitHub 仓库：[MrSong9957/Obsidian](https://github.com/MrSong9957/Obsidian)，保存个人知识、常用技能和 Agent 代理。若缺失 SKILL 技能和 Agent 代理时，按需下载到 Agent 工具全局目录使用。