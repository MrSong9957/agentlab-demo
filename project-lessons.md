# project-lessons.md

项目经验沉淀（Agent 维护）：项目定位、目录迁移背景、必须保持的行为明细、实现取舍、问题根因、验证结论演进与环境注意事项。命令、工具链等操作性信息以 AGENTS.md 第 7 节为准，本文不重复。本文与 013 记忆系统（memory 模块）无关，仅为 Agent 工作产物。

## 1. 项目定位

- 前后端分离：后端核心自成一体，所有前端的共用核心；前端适配不同平台和形态，如 Web 网站（参考 ChatGPT 网页版）、Agent CLI、Agent 桌面级应用。
- 核心技术栈 Rust，优先复用成熟框架或开源仓库，避免重复造轮子：直接参考或复用 Codex 开源 GitHub 仓库 `https://github.com/openai/codex`（各模块注明上游锚点提交，见模块头注释）。

## 2. 目录布局与模块职责（009 目录整理，2026-10-14）

- 迁移映射（规格 `docs/specs/009-project-layout.md`）：主程序 crate `src/`→`src/app/src/`、`tests/`→`src/app/tests/`、`assets/`→`src/app/assets/`；渲染层 `tui/`→`src/tui/`、`shims/`→`src/shims/`、`vendor/`→`src/vendor/`；日志 `logs/`→`docs/records/`。历史规格与记录中的旧路径按此映射理解。
- 根 Cargo.toml 为 workspace 入口，默认成员 `src/app`。
- 模块职责速查（`src/app/src/`）：`catalog.rs`（配置结构、内置资料合并、凭证指纹、目录刷新、路由检查、origin 授权、权限审计）、`api.rs`（协议适配、对话、分页、公共目录同步、来源/字段校验、响应体上限）、`prompt.rs`（005 最小模型请求组装）、`turn.rs`（006 工具调用循环）、`exec.rs`（003 同步命令执行）、`apply_patch.rs`（004 文本补丁修改）、`session.rs`（007 会话保存与恢复）、`memory/`（013/014 V1 记忆系统）、`display.rs`（显示边界：终端转义序列过滤）、`tui_host.rs`（008 TUI 宿主接入）；`main.rs` 是终端循环与命令入口。

## 3. 功能实现状态与演进索引

逐规格一句话状态；细节以对应规格 `docs/specs/` 与记录 `docs/records/` 为准。

- 001 CLI REPL：已验收，交互验收历史完成。
- 002 LLM 配置与模型目录：已实现（`config/` 手动＋自动目录、`/refresh`、非流式对话）；**重试机制截至 014 未实现**——006 turn 契约明示「不新增并行/流式/审批/重试」，如需重试须先修订契约。
- 003 exec_command：已实现（2026-10-08）。
- 004 apply_patch：已实现（2026-10-08）；F2 修复（2026-10-09）Update 仅允许常规文件（FIFO/设备/目录拒绝）。
- 005 最小模型请求组装（`prompt.rs`）、006 工具调用循环（`turn.rs`）：已实现，均标注 Codex 上游锚点提交。
- 007 会话保存与恢复：已实现（2026-10-10，四批整改，记录 `docs/records/2026-10-10-007-implementation.md`）。
- 008 TUI 渲染层移植：**实现中——A6 Windows ConPTY 未闭合**（无已授权 Windows 环境，验收规则：未闭合保持「实现中」）；A1–A5、A7–A10 已实测通过。
- 010 备用屏选择/复制、011 滚动条、012 工具详情：已实现并本地验证（2026-10-15 起，012 另有 2026-10-16 三轮迭代：展开态状态头、Enter 提交修复、独立详情页）。008 规格头部（2026-10-16 快照）将三者登记为「待审阅候选」，其后的审阅收口状态**待核实**。
- 013 记忆系统 V1：已交付（2026-10-19 用户最终批准，M1–M9）。范围七项：单会话提炼、结果与任务状态、Markdown 同步、全局合并、摘要注入、通用文件检索、引用反馈与用户修改便条；V2、双写、专用记忆工具（`memories.*`）不纳入。上游锚点 codex@`ce0323f2`；参考 `/vault/sources/agent/Codex记忆系统-框架实现与Agent执行手册.md`。范围外/暂缓/未验证项按规格 §7/§8 原登记保留。
- 014 记忆四项最小修订：已交付（2026-10-20，提交 `463ed8f`，记录 `docs/records/2026-10-20-014-implementation.md`）。已知本地适配：§3 资格随 reset 清除**与 Codex 跨重置资格保留不同**。

## 4. 必须保持的行为（契约明细）

按规格与安全修复已验收的契约；行为变更须先核对对应规格并按 AGENTS.md 门禁补回归。

- REPL（001）：回显 `Agent：`（全角冒号）、空输入不回显；`exit`/Ctrl+C/Ctrl+D 打印 `再见!` 且退出码 0（动画期间 Ctrl+C 仍为默认 SIGINT 退出码 130，按规格保留）；非法 UTF-8 不 panic、回显无 U+FFFD；管道等非终端输出跳过动画帧；rustyline 必须保留 `buffer-redux` 特性（关闭会丢多行粘贴输入）；TERM 为 `dumb`/未设置时交互退化属已知限制。
- 配置（002）：`config/` 存手动配置与自动目录、`/refresh` 刷新、普通非流式对话；重试机制未实现。
- 工具执行（003/004）：exec_command 固定 `/bin/sh -c`、超时/取消用 SIGKILL 终止整个进程组并回收、输出双流按码点有界截断、非法字节替换 U+FFFD；apply_patch 仅 Add/Update/Delete（Update 仅常规文件）、完整解析后按序执行失败不回滚、写入/删除类失败 `state_known=false` 须重读、未改行保留原行尾。
- 工具调用（006）：`id`/`name` 原样保留（trim 仅用于全空白判断，存储/唯一性判断/分派均用原始值）；取消时已知工具的未执行调用按各自结果结构、未知工具回传单 `error` 字段通用结构。
- 安全边界（003 安全修复及后续加固）：凭证外发须过 origin 授权门（重定向一律拒绝）；`file:` 凭证仅可引用专用凭证目录内文件；公共目录一律强制 HTTPS；远端文本显示前经 `display::sanitize_for_terminal` 过滤、服务端回显凭证值须脱敏；响应体与最终模型请求 JSON 上限各 10 MiB；timeout_ms 1..=60000、max_output_chars 1..=100000、单批 tool_calls ≤32、patch ≤1 MiB、单文件读入/生成内容 ≤4 MiB 且 ≤100000 逻辑行；`.env`/配置文件 0600 且 `.env` 不得注入信任根控制变量（名单见 §6）。
- 会话（007）：默认自动追加＋`/save`/`/resume`/`/sessions`；恢复的工具调用只作历史投影绝不重放；坏 meta 唯一整拒、行损坏跳行＋计数＋配对修复；auto 续写独占锁、获锁失败降级只读零修改；行 64 MiB 上限超限停用不截断；sessions 目录 0700/文件 0600。
- TUI 按键单一职责（010/012，定案）：Ctrl+C 一律退出（含运行中，完整收尾）；Esc 仅取消（空闲无动作、不清选区/不回底/不关页、绝不退搜索）；q 关详情页；清选区＝点击输入区；回底＝Ctrl+End；键盘永不复制；搜索出口＝Enter。
- 记忆系统（013/014）：存储在用户目录 `memories/`（SQLite＋manual.json memories 段）；V1 范围与排除项按规格 013（V2、双写、专用记忆工具不纳入）。

## 5. 按键行为演进与取代关系

当前定案见 §4「TUI 按键单一职责」。阅读历史记录时的取代关系：

- 010 实施轮的「K5＝Ctrl+C 恒按 J10 优先级」已被 2026-10-15 按键单一职责轮**取代**（Ctrl+C 一律退出）；此后 2026-10-17 两轮收尾消除 Esc 例外（Esc 绝不退搜索）并补复合态 Enter 让路规则。来源：`docs/records/2026-10-15-single-responsibility-keys.md` 及其 §7/§9 收尾。
- 早期「全量 2741/0/3」等为实现轮历史值，各阶段计数散见规格与记录头部，以日期判断新旧，**勿引用为当前状态**。

## 6. 安全修复根因与报告索引

边界数值清单见 §4「安全边界」；此处记根因、通道与来源。

- 2026-10-05 API 安全修复：`docs/security/reports/2026-10-05-terminal-chat-api-security-fix.md`（origin 授权门、`file:` 凭证限定目录、公共目录强制 HTTPS 无环境开关、显示前 sanitize）。本机模拟 HTTPS 强制的测试注入通道：`sync_public`/`refresh` 的 `url_override` 参数。
- F1（2026-10-09，`docs/security/reports/2026-10-09-backend-security.md`）：`.env` 不得注入信任根控制变量，跳过名单 `TCHAT_CONFIG_DIR`/`TCHAT_USER_DIR`/`TCHAT_PUBLIC_CATALOG_URL`/`HOME`/`APPDATA`（大小写不敏感）；后扩展 `TCHAT_SESSION_TS`/`TCHAT_SESSION_PID`（007 注入提供者）。普通凭证变量不受影响。
- 2026-10-10 高风险清单轮：凭证值脱敏（`api.rs::redact_credential`）、错误消息 300 字符统一。
- 014 符号链接拒绝对齐：`docs/security/reports/2026-10-20-014-memory-minimal-revisions-security.md`。
- 2026-10-21 仓库凭证脱敏：根 `.env` 自最早提交被跟踪，清空工作区或新增忽略规则都不能清除已有历史。已精确替换历史密钥、清除旧 reflog 和废弃对象，保留提交元数据及空值 `.env`；忽略范围按用户要求仅讨论。扫描命中的固定脱敏测试样例须结合测试上下文判定，不能为扫描清零改掉样例或新增豁免。证据与旧新提交映射见 `docs/records/2026-10-21-github-sanitization.md`。

## 7. 实现取舍与问题根因

- 007 并发与原子性：auto 续写独占锁采用「打开即锁→同 fd 装载/尾置/追加」，防止双写者；新建 auto 文件 `create_new` 后先取锁再写 meta，撞 Busy 换候选名；同名 auto 恢复复用已持锁描述符直接重装载（锁全程不离开写者，2026-10-10 第四批改为复用实现）；`/save` 快照与 `grant_origin` 授权记录均 tmp＋rename 原子替换（`write_atomic` 排他创建、仅清理本次 tmp、失败保留旧文件）。
- 010 取消语义：turn 运行中取消扩展到在途模型请求——`tui_host` sender 辅助线程＋等待侧轮询标志；底层请求至响应/超时自然结束，迟到结果经已弃 channel 丢弃（S71 `/proc` 线程回收实证；S72 重叠取消：迟到 tool_calls 不触发、不入史）。
- 011 滚动条：vendored `transcript_view.rs` 仅追加 mod 声明，新写 `scrollbar.rs`——RowCache 行数持久缓存（Weak 指针身份＋宽度键，命中不触 layout，2000 cell 稳定帧 70µs）；收尾轮修正端点映射：`ScrollbarState.content_length`＝最大可滚动顶行+1（总行数−视口+1）而非正文总行数。
- 012 投影一致性：工具展示实时终态与 `/resume` 走同一投影 `chat_driver::project_tool_item`；apply_patch 差异＝执行时真实前后缓冲的 diffy 统一 diff；鼠标 Down 清滚动条 RowCache（详情展开首次可达）。
- 013/014 存储：rusqlite bundled（DDL 逐字取自 codex `state/memory_migrations/*.sql`，SQL/事务语义移植自 `memories.rs`，sqlx→rusqlite 适配见 `store.rs` 头注释）；用户目录 `memories/`＋manual.json memories 段（D1/D5 用户确认 2026-10-18）。
- 2026-10-21 记忆迁移并发：`run_migrations` 的事务外快检不能作为唯一判据；TUI 登记器与后台管线同时打开新库时，两连接都可读到「0001 未应用」，先拿写锁者完成后，后到者若直接重放无 `IF NOT EXISTS` 的 DDL 会报 `table stage1_outputs already exists`，管线静默退出。修复为 `apply_migration` 在 `BEGIN IMMEDIATE` 事务内复核 `schema_migrations`，已应用则提交跳过；回归 `memory::store::tests::migration_rechecks_after_concurrent_apply`，详见 `docs/records/2026-10-21-backend-regression.md`。

## 8. 验证结论演进

- 当前基线（2026-10-21 后端回归收口）：容器内 `cargo check --locked --offline && cargo test --workspace --locked --offline --no-fail-fast` exit 0，50 个目标 **3045 通过/0 失败/3 既有忽略/0 过滤**；`e2e_acceptance` **106/0/0**。3 个忽略为 Windows AltGr 专项及两个父测试启动的子进程 helper。期间修复记忆库并发迁移竞态（见 §7），S81 修复后 20/20＋10/10 复跑通过；证据 `docs/records/2026-10-21-backend-regression.md`。
- 当前基线（014 收口，2026-10-20）：`cargo test --workspace --no-fail-fast` 49 套件 **3019 通过/0 失败/3 既有忽略/0 过滤**；e2e `--test e2e_acceptance` **106/0/0**。来源：`docs/records/2026-10-20-014-implementation.md`。
- 已知抖动：e2e s44（TUI 滚轮时序）在全量并行下偶发失败、单独运行通过（014 首轮 105/1，判定既有 PTY 抖动，双跑如实登记）。
- 验证摘要与原件索引：`docs/testing/verification-history.md`（最新 v14；014 收口未另立索引条目，以 014 记录为准）。
- 013 对照与上下文调研（未提交，供后续参考）：`docs/records/2026-10-20-013-memory-codex-comparison.md`、`docs/records/2026-10-20-codex-context-assembly.md`。

## 9. 环境注意事项

- 进入容器：宿主机项目根目录 `docker compose -f .devcontainer/compose.yaml exec -u dev -w /workspace dev bash`（原 AGENTS.md 第 7 节命令表「进入容器」行，精简时移出）。其余环境注意事项（CARGO_TARGET_DIR、clippy/rustfmt 安装）已在 AGENTS.md 第 7 节命令表，不重复。
- Agent 初始化按实际运行工具确认用户级目录；Codex 可用 `skills/list` 的 `scope=user` 和新会话 `thread/start.instructionSources` 验证技能及双层规则，避免只凭目录示例或模型自述判断。当前容器的 shell 沙箱故障会阻止技能按需读取依赖；通过原生技能输入同时选择依赖可验证显式调用路径，但不能据此宣称单独调用已恢复。具体安装、上游文档差异与验证边界见 [Codex 初始化记录](docs/records/2026-10-02-install-mattpocock-skills.md#2026-10-20codex-用户级初始化v29-个)。

## 10. 待核实清单

- 008 A6 Windows ConPTY：无已授权 Windows 环境未实测，实测通过前规格保持「实现中」。
- 010 宿主粘贴：OSC 52 已发送可证，实际粘贴效果需现场验证。
- 010/011/012 的「待审阅候选」状态是否已在后续轮次审阅收口（见 §3）。
