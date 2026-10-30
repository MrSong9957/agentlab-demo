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
- 003 exec_command：已实现（2026-10-08）；**016 修订（2026-10-22，待批准）**——默认可续接（yield 只控返回）、独立可选 timeout_ms 总时限（无 60000 上限）、tty PTY、六字段六状态（含 session_id/running/terminated）。
- 004 apply_patch：已实现（2026-10-08）；F2 修复（2026-10-09）Update 仅允许常规文件（FIFO/设备/目录拒绝）。
- 005 最小模型请求组装（`prompt.rs`）、006 工具调用循环（`turn.rs`）：已实现，均标注 Codex 上游锚点提交；018 起无回答宿主四工具，有回答宿主的普通/带记忆摘要轮五工具；内部记忆轮仍受限。
- 007 会话保存与恢复：已实现（2026-10-10，四批整改，记录 `docs/records/2026-10-10-007-implementation.md`）。
- 008 TUI 渲染层移植：**实现中——A6 Windows ConPTY 未闭合**（无已授权 Windows 环境，验收规则：未闭合保持「实现中」）；A1–A5、A7–A10 已实测通过。
- 010 备用屏选择/复制、011 滚动条、012 工具详情：已实现并本地验证（2026-10-15 起，012 另有 2026-10-16 三轮迭代：展开态状态头、Enter 提交修复、独立详情页）。008 规格头部（2026-10-16 快照）将三者登记为「待审阅候选」，其后的审阅收口状态**待核实**。
- 013 记忆系统 V1：已交付（2026-10-19 用户最终批准，M1–M9）。范围七项：单会话提炼、结果与任务状态、Markdown 同步、全局合并、摘要注入、通用文件检索、引用反馈与用户修改便条；V2、双写、专用记忆工具（`memories.*`）不纳入。上游锚点 codex@`ce0323f2`；参考 `/vault/sources/agent/Codex记忆系统-框架实现与Agent执行手册.md`。范围外/暂缓/未验证项按规格 §7/§8 原登记保留。
- 014 记忆四项最小修订：已交付（2026-10-20，提交 `463ed8f`，记录 `docs/records/2026-10-20-014-implementation.md`）。已知本地适配：§3 资格随 reset 清除**与 Codex 跨重置资格保留不同**。
- 016 命令执行与进程交互：已实施并验证（2026-10-22，分支 feat/016-process-sessions，待批准）——默认可续接进程会话（管道＋PTY）、write_stdin 查询/输入/中断/终止、取消与寿命分开、64 条容量＋1 MiB 头尾缓存＋64 KiB 输入上限、两种宿主退出清理。记录 `docs/records/2026-10-22-016-implementation.md`。
- 017 任务计划：已批准交付（2026-10-22，分支 feat/017-update-plan 合并入 main）——只新增 update_plan：参数/状态/解析复用 Codex（shim 三类型＋Serde 原语义），两字段结果，行式清单＋TUI 计划 cell（上游 PlanUpdateCell 最小移植），V1 会话形状不变、成功快照恢复投影；母本计划指引两节恢复保留。记录 `docs/records/2026-10-22-017-implementation.md`。

- 018 用户问询：[阶段 A 已批准交付](../docs/specs/018-user-input.md)（2026-10-23，任务提交 `96682d0` 快进合入本地 `main`，本地任务分支已删除）；同步问答、独立回送、取消/退出和 V1 同源恢复已接入，安全门禁与双轴审查已收口，全量 3142 通过/0 失败/3 原有忽略；全目标 lint 保留两处旧测试错误。异步版未注册/未实施，未推送或发布。[实施证据](../docs/records/2026-10-22-018-user-input-implementation.md)。

  阶段 B：[已实施验证待批准](../docs/records/2026-10-23-018b-implementation.md)（2026-10-23，分支 feat/018-user-input-async，基线 main@dc7a4f1）——TUI 七工具（异步发布＋本地 wait_for_user_input）、每轮单项未决、F6 打开原问答窗口、回答在完整批后入史、结束检查先处理已受理回答、V1 可选来源标记＋旧版重存降级；CLI/无宿主/内部记忆轮工具面不变。全量 3171/0/3，安全门禁、六项变异与双轴审查（2 规格缺口已修复补回归）闭环；2026-10-23 批准交付本地 main（任务提交 `49060bf` 快进合入、分支已删除，未推送）。

## 4. 必须保持的行为（契约明细）

按规格与安全修复已验收的契约；行为变更须先核对对应规格并按 AGENTS.md 门禁补回归。

- REPL（001）：回显 `Agent：`（全角冒号）、空输入不回显；`exit`/Ctrl+C/Ctrl+D 打印 `再见!` 且退出码 0（动画期间 Ctrl+C 仍为默认 SIGINT 退出码 130，按规格保留）；非法 UTF-8 不 panic、回显无 U+FFFD；管道等非终端输出跳过动画帧；rustyline 必须保留 `buffer-redux` 特性（关闭会丢多行粘贴输入）；TERM 为 `dumb`/未设置时交互退化属已知限制。
- 配置（002）：`config/` 存手动配置与自动目录、`/refresh` 刷新、普通非流式对话；重试机制未实现。
- 工具执行（003/004/016）：exec_command 固定 `/bin/sh -c`、**默认可续接**——等待（yield 250..=30000）只控返回，运行中返回 session_id；可选 timeout_ms 总时限（正整数无上限，从启动按单调时钟独立计时，到期 SIGKILL 整组回收）；tty=true 分配真实 PTY；输出双流按码点有界截断、非法字节替换 U+FFFD、待取缓存单份 1 MiB 头尾保留；write_stdin：空查询／PTY 文字输入（超 64 KiB 整次拒绝）／管道仅 Ctrl+C（\u0003）中断／terminate=true 三秒有界收尾；句柄仅当前对话有效，64 条满额拒绝不淘汰。apply_patch 仅 Add/Update/Delete（Update 仅常规文件）、完整解析后按序执行失败不回滚、写入/删除类失败 `state_known=false` 须重读、未改行保留原行尾。
- 工具调用（006/016/018-B）：用户轮默认四项 exec_command/apply_patch/write_stdin/update_plan；有同步回答宿主加 request_user_input 为五项（行式 CLI）；TUI 同步＋异步宿主为七项（再加 request_user_input_async/wait_for_user_input，等待必须批内最后一条）；无宿主四项；记忆内部轮仍仅 mem 工具；`id`/`name` 原样保留（trim 仅用于全空白判断，存储/唯一性判断/分派均用原始值）；取消只停止等待与剩余调用**不终止已启动进程**（016）；取消时已知工具的未执行调用按各自结果结构（空句柄）、未知工具回传单 `error` 字段通用结构。
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
- 2026-10-21 仓库凭证脱敏：根 `.env` 自最早提交被跟踪，清空工作区或新增忽略规则都不能清除已有历史。已精确替换历史密钥、清除旧 reflog 和废弃对象，保留提交元数据及空值 `.env`；脱敏阶段的忽略范围按用户要求仅讨论。扫描命中的固定脱敏测试样例须结合测试上下文判定，不能为扫描清零改掉样例或新增豁免。证据与旧新提交映射见 `docs/records/2026-10-21-github-sanitization.md`。
- Git 忽略范围调查：第一批已按用户要求取消跟踪并忽略真实 `.env`，保留本地内容与 `0600` 权限，提供空值 `.env.example`，并忽略根目录 `.pytest_cache/`。`auto_catalog.json` 可由内置资料离线重建，但查询缓存含 `cred_fp` 等连接身份；它与共享手动配置需分开评估。`.agents` 当前保存代理／技能定义，不能直接当缓存；另有 3 份 `.snap.new` 虽命中规则仍已被跟踪，关联测试已标记 `008-TRIM`，不得靠改规则或接受新快照掩盖。事实、验证与其余待讨论范围见 `docs/records/2026-10-21-gitignore-scope-plan.md`。

## 7. 实现取舍与问题根因

- 隔离后端交付后的规格同步（2026-10-27）：先区分当前代码、目标环境部署与后续功能验收。033 已替代旧双账号执行链路并部署共享 dev；后续规格复用 `sandbox.rs`，测试引用改为 `*_sandbox`，普通回归清除继承的 `TCHAT_SANDBOX`，新增行为另验真实沙箱分支。固定可读/可写根及保留网络不能冒充动态权限；身份前提改变时须重新证明硬链接别名、进程归属和收尾边界，不能只改名或沿用旧通过结论。范围与证据见[同步记录](../docs/records/2026-10-27-sandbox-spec-sync.md)。
- 035 多 Agent 规划（2026-10-26，待实现）：原表六工具共用独立上下文、邮箱和生命周期；send只入队、followup才唤醒空闲Agent，取消等待不等于传输结束。首版一层/3子槽/同根轮，根与子请求预算分开，进程须按可信owner回收；V3子事实不能伪造成根tool_calls，032基建为前置。范围/适配/未测事项见 [035](../docs/specs/035-multi-agent.md)。
- 034 JavaScript 工具编排规划（2026-10-26，待实现）：固定上游 code-mode crate 是外部 host 客户端，移植 handler 不能获得 JS 引擎。本项选固定 rquickjs/独立 worker，嵌套调用复用当前请求执行集合；内层动作另存宿主事件，不能伪造提供方 tool_calls，故以032的V3基建为前置。JSON输入、让出后的调度限制、限额及未测事项见 [034](../docs/specs/034-javascript-tool-orchestration.md)。
- 032 上下文管理规划（2026-10-26，待实现）：015 的32768字节规则预算不是模型token余量；固定上游new_context也不自动摘要。当前方案用显式交接摘要＋完整历史/活动视图分离＋V3检查点，保存成功才切换；必须升级独立记忆版本门，不能重置逻辑轮额度、进程或权限。输入适配、估算限制与验收见 [032](../docs/specs/032-context-management.md)。
- 034 JavaScript 工具编排（2026-10-29 实施，已交付）：exec/wait 双 opt-in（experimental_supported_tools 精确双声明）；rquickjs 0.14 worker 子进程（--internal-js、env_clear、RLIMIT、033 包装 fail-closed）；观察单点驱动＋V3 编排事件（cell_started/call_started/call_finished/cell_closed，不进模型投影）；额度 cell 31/轮 128/4 在途＋串行通道；目录＝当前请求声明面冻结；全量 3781 绿。规格 [034](../docs/specs/034-javascript-tool-orchestration.md)。
- 031 权限申请规划（2026-10-27 同步，待实现）：033 固定沙箱与 TUI 权限占位不等于动态文件/网络授权。首版扩展现有统一入口，可信启动策略＋turn 授权；用户确认、同 UID 下永久边界、实际强制后端和到期收尾一起验收，额外权限进程不能沿016跨轮保活。033 已部署不代表031已验收；范围与接口见 [031](../docs/specs/031-request-permissions.md)。
- 工具对照表逐项规划（2026-10-26 用户明确）：任务单位为原表「能力」列的一整行，使用原名称；同一行的内部缺口纳入同一份规格及实施计划，不因可独立验收再拆任务。每次只交付一项，用户同意后才处理下一行尚未实现项；当前「文件补丁」统一覆盖 Move to 与 Unicode 匹配容差，见 [029](../docs/specs/029-file-patch.md)。
- 018-B 实施（2026-10-23）：①测试辅助 `PtyCli::send` 自动追加 CR——功能键/方向键须用新 `send_keys`，否则 F6 后跟 Enter 会把预选项直接提交；xterm F6＝`ESC[17~`（`ESC[15~` 是 F5）。②等待类用例经通道＋recv_timeout 交付并设看门狗，变异「取消不醒」10 秒内断言失败而非无限 join 挂死。③变异有效性证明的测试选择必须让被破坏行为可见：手工构造历史的用例检不出生产端标记删除（M1），两条调用的批次检不出批中插答（M2，需第三条调用错位）；变异脚本异常路径也必须恢复文件。④会话文件 content 内 JSON 为转义形态，断言按字段组合而非未转义字面量。
- 018-B 审计修复（2026-10-23，fix/018-user-input-audit）：①结构性批规则校验按反例形状写——「必须批末」用前缀切片（`calls[..len-1]` 任一命中即拒）比「any 存在且 last 不是」的否定式组合更全覆盖（后者漏 `[wait,exec,wait]`、`[wait,wait]`）；②多个错误出口各自处理未决状态必然漂移（sender 失败保存答案、协议错误丢答案），收敛单一收尾函数后用原变异集复验等价；③变异证据重录必须在当次同步记录真实原始哈希——事后发现不自洽时，git 提交哈希只能证明版本内容、不能替代变异前快照，只能保留原值并标注无法核验、不计为有效恢复证明（不猜改历史），有效性靠在当前候选重放重新建立；任何源码微调（含惯用法）都使证据候选哈希失效，须重放受影响变异。证据见[审计记录修复轮](../docs/records/2026-10-23-018-user-input-audit.md)。
- 018-B 交付后复核（2026-10-23）：批末规则须检查每条等待调用；已受理答案要同时覆盖请求失败与协议失败的收尾；答案入史后，同轮已答 ID 的就绪语义仍须保留。现有回归全绿但三个边界探针失败，问题尚未修复，证据统一见[审查记录](../docs/records/2026-10-23-018-user-input-audit.md)。
- 018-B 调查（2026-10-23）：Codex 异步确认不含答案，回答经活动轮输入消费。本地 V1 探针确认：批中插入文本会使后续结果被配对修复，重复同 ID 的 ToolResult 被丢弃；回答应在完整批后独立入史。可选来源字段旧程序能读、重存会丢可靠关联；兼容方案须确认，依据见 [018 §8.1/§8.6](../docs/specs/018-user-input.md#8-阶段-b最小异步问询契约草案)。
- 用户问询复核（2026-10-22）：Codex 同步工具要求非空选项并等待回答；普通模式可开放，但上游 TUI 非阻塞问询在无交互时会自动返回空回答，不能等同“必须等到用户提交”。本地 turn 运行中输入仅排队，问询须另接回答回送，否则会互相等待；底层弹窗和协议空壳不等于工具已接通。建议先复用同步问答的必要部分，异步后置，依据见 [用户问询差异复核](../docs/records/2026-10-22-codex-user-input-comparison.md)。
- 017 计划工具（2026-10-22）：①嵌入母本锚点不变前提下「停用剔除」即恢复指引——`without_update_plan_instructions` 过滤器与其上游锚定测试一并移除，prompt_assembly s2 的测试侧独立重建同步改写（不再剔节）即为防回归面；②TUI 计划 cell 移植保真用「008-TRIM 遗留同名快照逐字节对比」锚定（新旧 .snap 正文一致＝渲染零漂移），孤儿快照删除、等价新快照（plans_tests 名下）接管；③Owned transcript 对计划 cell 用母本**紧凑视图**（按状态排序、隐藏说明、计数头），说明只在完整视图（Ctrl+T 详情页/展开）——e2e 断言需按视图分治，s91 以 Ctrl+T 验证说明；④TUI 侧错误文本（serde 错误可内嵌模型可控变体名）同属不可信来源，展示副本一并过 sanitize（审查发现补齐）。
- 004 与 Codex 同名工具的兼容性（2026-10-22 复核）：`{"patch":...}` 是 Chat Completions 函数工具适配，工具定义与发送版提示词已同步；不能仅为对齐上游自由文本接口更换参数。`Move to`、Unicode 标点匹配容差是已知 MVP 取舍；移植上游时须保留本地 Add 禁止覆盖及四字段结果契约。依据与按需补齐建议见 [apply_patch 差异复核](../docs/records/2026-10-22-codex-apply-patch-comparison.md)。
- update_plan 复核（2026-10-22）：上游工具接收完整计划并发布展示事件，不执行步骤或验证完成，也不等于 Plan Mode。本地遗留计划快照不能证明功能可用，计划 cell 已因无事件源被裁掉；接入须同时恢复提示词与实时／恢复投影，并评估更新次数对每轮 8 次请求额度的影响。最小方案可用已保存的成功调用参数重建展示，避免新增独立存储或贸然扩展持久化展示枚举。依据与建议见 [update_plan 差异复核](../docs/records/2026-10-22-codex-update-plan-comparison.md)。
- 007 并发与原子性：auto 续写独占锁采用「打开即锁→同 fd 装载/尾置/追加」，防止双写者；新建 auto 文件 `create_new` 后先取锁再写 meta，撞 Busy 换候选名；同名 auto 恢复复用已持锁描述符直接重装载（锁全程不离开写者，2026-10-10 第四批改为复用实现）；`/save` 快照与 `grant_origin` 授权记录均 tmp＋rename 原子替换（`write_atomic` 排他创建、仅清理本次 tmp、失败保留旧文件）。
- 010 取消语义：turn 运行中取消扩展到在途模型请求——`tui_host` sender 辅助线程＋等待侧轮询标志；底层请求至响应/超时自然结束，迟到结果经已弃 channel 丢弃（S71 `/proc` 线程回收实证；S72 重叠取消：迟到 tool_calls 不触发、不入史）。
- 011 滚动条：vendored `transcript_view.rs` 仅追加 mod 声明，新写 `scrollbar.rs`——RowCache 行数持久缓存（Weak 指针身份＋宽度键，命中不触 layout，2000 cell 稳定帧 70µs）；收尾轮修正端点映射：`ScrollbarState.content_length`＝最大可滚动顶行+1（总行数−视口+1）而非正文总行数。
- 012 投影一致性：工具展示实时终态与 `/resume` 走同一投影 `chat_driver::project_tool_item`；apply_patch 差异＝执行时真实前后缓冲的 diffy 统一 diff；鼠标 Down 清滚动条 RowCache（详情展开首次可达）。
- 013/014 存储：rusqlite bundled（DDL 逐字取自 codex `state/memory_migrations/*.sql`，SQL/事务语义移植自 `memories.rs`，sqlx→rusqlite 适配见 `store.rs` 头注释）；用户目录 `memories/`＋manual.json memories 段（D1/D5 用户确认 2026-10-18）。
- 016 进程会话（2026-10-22）：等待/寿命/取消三者分开——yield 只控返回、timeout_ms 由监控线程独立计时（读取/输入/跨轮/取消均不重置）、取消保留进程；满额（64 含已退出未取走）拒绝新建不淘汰；每进程固定 1 监控＋1–2 读取线程，等待不持管理器全局锁；PTY 用 pre_exec setsid＋TIOCSCTTY（不能与 process_group(0) 并用——坑004）；管道/PTY 终止一律 killpg 整组；宿主退出显式 close 全部会话再排空（TUI process::exit 不走析构）。本平台 Instant（秒域 i64）下 u64 毫秒全域可表示截止时间，“无法安全表示→拒绝”为防御分支。
- 2026-10-22 测试加固（016 候选三轮）：A7 原始字节检查的"唯一标记"若只判**引入符直接相邻**会漏检真实形态序列（OSC 52 的标记隔着 `52;c;` 参数）——须按 VT 序列语法做**范围扫描**（CSI 至 0x40..=0x7E 终止符、OSC/DCS 至 BEL/ST），另保留直接组合层覆盖 nF/CR 短序列；正向对照（断言合法渲染控制确实存在）防一刀切误报。S61 类"输入动作清焦"证明可用聚焦态样式差异（披露行加粗，`vt100` Cell::bold）作非清焦证据，替代"Enter 开详情页"式清焦证明。
- 2026-10-21 记忆迁移并发：`run_migrations` 的事务外快检不能作为唯一判据；TUI 登记器与后台管线同时打开新库时，两连接都可读到「0001 未应用」，先拿写锁者完成后，后到者若直接重放无 `IF NOT EXISTS` 的 DDL 会报 `table stage1_outputs already exists`，管线静默退出。修复为 `apply_migration` 在 `BEGIN IMMEDIATE` 事务内复核 `schema_migrations`，已应用则提交跳过；回归 `memory::store::tests::migration_rechecks_after_concurrent_apply`，详见 `docs/records/2026-10-21-backend-regression.md`。

## 8. 验证结论演进

- 当前基线（2026-10-23 018-B 候选，待批准）：`cargo check && cargo test --workspace --no-fail-fast` exit 0，**3171 通过/0 失败/3 既有忽略**，相对阶段 A 交付基线 3142/0/3 **净增 29**（模块内嵌单测 5、user_input_async 集成 15、安全 3、E2E S103–S106 4、TUI 投影 2；勿以实现中间态 3147 为基线——它已含 5 项内嵌单测）；六项可逆变异全部检出；lint 仅两处旧测试错误沿用（`security_untrusted_text.rs` unused_io_amount）。记录 `docs/records/2026-10-23-018b-implementation.md`。
- 前一基线（2026-10-22 017 update-plan，已批准交付）：`cargo check && cargo test --workspace --no-fail-fast` exit 0，51 目标 **3103 通过/0 失败/3 既有忽略**（含补验收轮 s92 真实缩放 e2e）；净增 20 用例（新增 24：plan 7/turn 2/tui_host 2/chat_driver 2/plans_tests 4/tool_loop t15–t18/session_cli e21/e2e s91＋s92；移除 4：`without_update_plan_instructions` 锚定测试随过滤器移除），`git diff --check` 通过。同法实测 HEAD（895cd60，独立 worktree＋独立 target）＝3083/0/3，账目闭环；此前记录的 3079 为 016 三轮历史口径、与 HEAD 实测差 4。6 项可逆变异（漏注册/失败误画清单/成功判定/终端过滤/取消形状/缩放无 SIGWINCH）全部检出。记录 `docs/records/2026-10-22-017-implementation.md`。
- 当前基线（2026-10-22 016 三轮测试加固候选，已批准交付）：`cargo test --workspace --no-fail-fast` 51 目标 **3079 通过/0 失败/3 既有忽略**（二轮 3084−净删 5：重复用例删 4＋导出合并 2→1）；三轮可逆变异 15/15 检出（七处加固 H1–H7、删除替代 D1–D4、合并 M1–M2），零生产行为改动；记录 `docs/records/2026-10-22-test-hardening-round.md`（前两轮见 `2026-10-22-016-implementation.md`，二轮 3084/0/3 与 12 项变异为历史证据）。
- 前一基线（2026-10-21 后端回归收口）：容器内 `cargo check --locked --offline && cargo test --workspace --locked --offline --no-fail-fast` exit 0，50 个目标 **3045 通过/0 失败/3 既有忽略/0 过滤**；`e2e_acceptance` **106/0/0**。3 个忽略为 Windows AltGr 专项及两个父测试启动的子进程 helper。期间修复记忆库并发迁移竞态（见 §7），S81 修复后 20/20＋10/10 复跑通过；证据 `docs/records/2026-10-21-backend-regression.md`。
- 当前基线（014 收口，2026-10-20）：`cargo test --workspace --no-fail-fast` 49 套件 **3019 通过/0 失败/3 既有忽略/0 过滤**；e2e `--test e2e_acceptance` **106/0/0**。来源：`docs/records/2026-10-20-014-implementation.md`。
- 已知抖动：e2e s44（TUI 滚轮时序）在全量并行下偶发失败、单独运行通过（014 首轮 105/1，判定既有 PTY 抖动，双跑如实登记）。
- 验证摘要与原件索引：`docs/testing/verification-history.md`（最新 v14；014 收口未另立索引条目，以 014 记录为准）。
- 013 对照与上下文调研（未提交，供后续参考）：`docs/records/2026-10-20-013-memory-codex-comparison.md`、`docs/records/2026-10-20-codex-context-assembly.md`。

## 9. 环境注意事项

- last30days 调研覆盖核验（2026-10-28）：本机 3.31.0 的 quick 深度只执行第一个子查询、最多两个来源；参数列出更多来源不代表全部运行。先核实际 query plan/source outcomes，按需显式选源或补轮；403、空结果和无关命中分别记录，不能拼成社区共识。源码摘录、真实运行与日期资格见[倒序调研索引](../docs/think-tank/2026-10-28-plan-preresearch-index.md)；quick 限制的原始输出报告留存于 `docs/logs/plan-preresearch/2026-10-28-035/engine-hn-github-raw-v3.md`。
- 进入容器：宿主机项目根目录 `docker compose -f .devcontainer/compose.yaml exec -u dev -w /workspace dev bash`（原 AGENTS.md 第 7 节命令表「进入容器」行，精简时移出）。其余环境注意事项（CARGO_TARGET_DIR、clippy/rustfmt 安装）已在 AGENTS.md 第 7 节命令表，不重复。
- Agent 初始化按实际运行工具确认用户级目录；Codex 可用 `skills/list` 的 `scope=user` 和新会话 `thread/start.instructionSources` 验证技能及双层规则，避免只凭目录示例或模型自述判断。当前容器的 shell 沙箱故障会阻止技能按需读取依赖；通过原生技能输入同时选择依赖可验证显式调用路径，但不能据此宣称单独调用已恢复。具体安装、上游文档差异与验证边界见 [Codex 初始化记录](../docs/records/2026-10-02-install-mattpocock-skills.md#2026-10-20codex-用户级初始化v29-个)。
- Codex 原生插件验收补充（2026-10-28）：`skills/list` 与 `plugin/read` 分别核验独立技能和插件技能；CE 插件技能的实际名称带 `compound-engineering:` 前缀，不能用未加前缀的名称作断言，也不能仅凭独立技能列表判断插件缺失。本项目的知识沉淀继续遵循既有路径，不另加一套 CE 自动沉淀规则。安装方式、临时新会话的两层规则加载证据及来源限制见 [五步初始化记录](../docs/logs/agent-init/2026-10-28-initialize.md)。

## 10. 待核实清单

- 既有告警清理（独立任务，源自 035 终验登记，2026-10-29）：`cargo check --locked` 基线存在 518 条告警——tchat-tui 463、terminal-chat ~46（031–034 期代码：034 `close_orch!` 宏出口赋值×24、未用 import CellStatus、031 `mut perm`、032 `pending_switch`、033 `let mut child` 等）、shims 若干；035 新增代码零告警。清理属独立任务，禁止搭乘功能分支顺手处理。

- 008 A6 Windows ConPTY：无已授权 Windows 环境未实测，实测通过前规格保持「实现中」。
- 010 宿主粘贴：OSC 52 已发送可证，实际粘贴效果需现场验证。
- 010/011/012 的「待审阅候选」状态是否已在后续轮次审阅收口（见 §3）。

## 018 同步问询接入经验（2026-10-22）

- 用户回答须用每次独立通道回送，不能复用普通聊天队列；回答/取消不获取 turn 持有的历史锁。原调用方保留无宿主入口，能力在声明和执行两层判断。
- 已取消但尚未显示的问询事件也须保留取消原因，不能一律视作通道关闭；用真实 Esc 后再投递已创建事件的受控驱动回归检验此交界。
- 复用聊天 composer 时要区分原文与显示：其普通粘贴过滤会改变回答。问询改用原文插入，渲染时复用显示过滤；选项用原始索引取 label，关联 ID 不过滤。
- Rustyline 的 unsupported TERM 读取是阻塞标准输入，单置取消标志不能解除等待；退化路径须可取消，并始终只由主线程读取终端。适配须保持依赖的 ASCII 大小写无关判断，并验收 stdin 是终端、stdout 重定向的组合，不能只测双终端。
- 存量未格式化的大闭包不能按 diff 行号拼接 rustfmt 输出：重复片段可能被错误对齐并改变代码。格式工具成功不代表补丁正确；只格式化明确的新文件/完整函数，之后检查原有删除行并复验实际行为。
- 对带 `mod common` 的单个新测试文件运行 rustfmt 也会递归改写旧模块；最小范围检查/格式化须加 `skip_children=true`，并按各 crate 的既定 edition 运行。

## 019 图片工具规划依据（2026-10-23，未实施）

- Codex `d138398` 的 `view_image` 输出是工具结果中的结构化图片，不另插图片消息；本项目 Chat Completions 的 tool 消息只能放文本，图片须在完整工具批次后作请求投影适配。上游工具实际的 high 准备路径使用 2048＋2500 patches 预算，不能误用旧附件入口的单纯边长缩放。复用范围、协议依据与尚未实施的取舍见 [019 调研记录](../docs/records/2026-10-23-view-image-plan-research.md)。

## 019 view_image 实施经验（2026-10-23）

- 上游 image 0.25.10 的 `image::Limits` 是 `non_exhaustive`，不能结构体字面量构造，须 `Limits::default()` 后逐字段赋值；`ImageReader::limits(&mut self, …)` 是可变借用方法。解码器限额错误文案（"Image size exceeds limit"）不含具体阈值，包装错误消息时要自己带上限额数字，断言才能钉住边界。
- 生成"不可压缩"测试图时噪声三通道不能同源（`v`、`v>>1`、`v^0x5a` 之类），PNG 行过滤器能把单字节派生的像素压到近零；每通道独立推进 LCG 状态才可靠。估算 PNG 体积时记得 RGBA=4 字节/像素、RGB=3，base64 膨胀 4/3，1 MiB=1048576 字节——限额夹具三者都要算准，本项目的 10 MiB 请求上限用例第一版差 0.8% 没触发。
- 给 `HistoryItem::ToolResult` 这类全库高频枚举变体加字段：先脚本化区分构造点（字段带冒号）与模式匹配点（裸绑定或 `..`），构造点插 `image: None`、模式点补绑定或依赖既有 `..`。绝不能盲目在所有出现点插值——`image: None` 落进模式位置是合法的"匹配 None 子模式"，会静默改变分支行为而不是编译失败。
- Chat Completions 协议下图片放 user 内容部件：附件消息必须由 `project_history` 投影时现生成（不落 `HistoryItem::Text`），批末 flush；015 的每轮上下文注入会多出一条 user 消息，e2e 断言角色序列时要把它算进去。
- CLI 图片入口讨论（2026-10-23，仅文档）：Codex 的 `@` 选图直接构造用户图片附件，不触发 `view_image` 工具；两者可共享读取和图片准备函数，但用户消息与工具结果的来源、时机及历史关联仍须区分。六个入口的计数、源码与待补范围集中见 [019 §9](../docs/specs/019-view-image.md#9-cli-图片输入补充讨论2026-10-23)。
- CLI 图片细节复核（2026-10-23，仅文档）：多入口共用核心仍要保留两个消息出口；附件编号、删除、排队与恢复须绑定完整消息。Codex 的 `snapshot_local_images` 仅随远程工作区启用，32 MiB 是其传输预算，不能直接套成本地默认契约。结构与具体规则统一见 [019 §9.4／§9.6](../docs/specs/019-view-image.md#94-多个入口共用核心与两种出口)。
- CLI 图片默认方案（2026-10-23，用户要求优先复用 Codex，仅文档）：本地队列保存路径，正式提交时读取并准备后入史；不新增全队列图片字节限额，继续用原请求上限；用户附件映射上游 images／text_elements 字段；临时 PNG 沿用 keep 保留，不自加 GC。此前“入队即冻结”和四项待定建议已由 [019 §9.6](../docs/specs/019-view-image.md#96-四项具体规则优先复用-codex2026-10-23)替代，不需要用户再逐项审核。
- 020 规格准备（2026-10-23，未实施）：用户直接附图已从 019 §9 提炼为[独立五节规格](../docs/specs/020-cli-image-input.md)，019 保留旧锚点索引和工具实施证据。入史前失败恢复稿与后来草稿按 Codex 合并，入史后请求失败保留历史且不自动重发；任务基线按准备时真实 main 核实，分支及工作保护见[准备记录](../docs/records/2026-10-23-020-cli-image-input-spec-prep.md)。

## 020 CLI 用户附图实施经验（2026-10-23）

- 019 记过的坑再次复发并加深：机械脚本给 `HistoryMessage {` 构造点插新字段时，若模式匹配点（matches!/if let/filter）带 rest `..`，插入的 `images: None,` 是合法"匹配 None"子模式——编译不报错、行为静默收窄（首 user 识别、事件过滤全部失配）。修复脚本必须先剥除候选两行再判"是否含构造值调用"，且剥除自身行后判定，否则误插行会被当成构造证据而漏修；嵌套 `Some(X { .. })` 需栈式配对而非首个 `}`。手修破损后要 grep 重复块——替换脚本失败重跑可能造成 `..` 后重复残片。
- PTY 时序两个真坑：① bracketed paste 之后立刻 Enter 会被 paste-enter 抑制窗口（120ms）吞成换行——母本行为不是 bug，测试需 sleep 后再提交；② 一次 `send_raw(b"first\r")` 多字符+回车会落入 8ms 突发窗口，Enter 同样被吞——文本与回车必须分开发送并留间隔。
- 全新 Cargo target-dir 的全 workspace 测试编译在本环境超过单命令时限，后台跑完拿到的统计可能为空（被时限杀但退出码包装为 0）——先 `cargo check --workspace --all-targets` 预热或拆分跑，验证统计以日志文件内容为准，不信空输出的"成功"。
- `@` 选图类入口依赖被整层裁剪的弹窗体系（ActivePopup/FileSearchPopup/completion 插入链）时，"恢复入口"实际等于重建宿主 glue 层——排期时按重建估，不要按"复用"估。

## 020 A02 @ 选图补实施经验（2026-10-23 第二轮）

- 恢复被 008 裁剪的弹窗链，实际改动面集中在四处：popup 本体（新文件）、`ActivePopup` 变体、composer 的 sync/键路由/插入三函数、popup 区域渲染（layout 的 `popup_rect` 一直在，只是 `popup: _` 丢弃）——基建（ScrollState/picker_rows/DismissedToken/completion_target）全部保留，恢复成本远低于重建。
- composer 单测逐字符 `handle_key_event(Char)` 输入会被 paste-burst 缓冲（text 恒空、sync 无从触发）——测试输入走 `handle_paste` 通道；这同时提醒：真实用户敲键不会触发 burst（间隔>8ms），只有测试连发会。
- PTY 断言请求消息时注意 020 投影语义：无图用户消息保持**纯字符串** content（不是数组）——"找数组消息的 text"会漏掉纯文本轮；VT 屏幕断言对"已删除元素"不可靠（残影帧），删除/重编号类断言应以最终请求字节为准。
- TUI 进程退出码断言在"turn 阻塞于问询等待"时不可达（框架看门狗 SIGKILL→code None）；问询场景的收尾用 Esc 中断＋不硬断言退出码，退出语义由专门用例（S93 系列）持有。

- 020 第二轮教训：给既有弹窗体系加"新弹窗变体"时，token 检测必须沿用上游的 editable 判定（`prefixed_token_range_is_editable`）——裸 `current_prefixed_token_range` 会匹配已绑定的 `@mention` 原子元素，既有"绑定 mention 不阻塞方向键导航"契约即被打破（全量回归抓到）；弹窗键分支早退还必须保留外层统一 `sync_popups`，否则候选停留旧 query。
- PTY/集成测试之外，"合并重编号"这类状态重建逻辑必须有直接断言的组件测试：S110/S102 的 E2E 中 Dnew 恒为空，合并分支零覆盖，增量审查一句话就点破了"实现方向正确但无证据"。

## 021 执行中消息规划依据（2026-10-23 实施，2026-10-24 批准交付本地 main）

- Codex 的 `send_message_to_user_async` 发出的是 `delivery=Async` 的 assistant 消息：消息阶段即使是 FinalAnswer，也不结束 turn；成功只回 `accepted`，下一请求不另注入同文 assistant 正文。回应中途追问还依赖独立的普通输入接续，不能用本地原排队行为或 018 回答通道冒充；复用 020 完整消息，具体取舍与固定源码见 [021 调研记录](../docs/records/2026-10-23-send-message-to-user-async-plan-research.md)。

## 022 MCP 工具规划依据（2026-10-24 实施，同日批准交付本地 main）

- Codex `d138398` 的 MCP 已拆到 `codex-mcp`／`rmcp-client`，底层官方 SDK 固定 rev 的 `LATEST` 为 2026-07-28，initialize 生命周期须显式选择；上游模型名上限 128 还需适配本项目 CC 的 64。SDK HTTP 404 会话恢复可能重发原 POST，接入时须关闭并验证，才能保持 006 的不重放契约。原始身份、每轮快照及两种传输的最小复用方案见 [022 规格](../docs/specs/022-mcp-tools.md)与[固定源码调研](../docs/records/2026-10-24-022-mcp-tools-spec-research.md)。

## 022 MCP 实施经验（2026-10-24 实施，同日批准交付本地 main）

- rmcp `98a5478` 的客户端默认 `get_info` 发 `LATEST`（2026-07-28 无 initialize 生命周期）：自定义 ClientHandler 必须显式 `with_protocol_version(LATEST_WITH_INITIALIZE)`，漏掉时全部连接报「不兼容协议版本 2026-07-28」——规格 §4 预警在实测中真实踩中。
- 目录过期计数（stale_epoch）的发布基准必须在**列出前**记录：列出后 settle 再读会把重列期间的新通知当作已处理内容吞掉（违反「重列期间收到的新通知必须保留过期标记」）；列出前记录则保守多列、不丢通知。
- 该 SDK 把 `resultType:"input_required"` 反序列化为 `ServerResult::CustomResult` 而非 `InputRequiredResult`（分派层 `other` 分支承接）；「不支持协议响应」的拒绝要在两个分支都成立。
- SDK 的 reqwest 适配器与 SSE 事件限额器是 `pub(crate)`：外部只能函数级移植；移植时把 JSON 体一并改为有界读取（SDK 默认 `bytes()`/`text()` 无界）。
- serde_json 的 Map 反序列化会静默合并重复 JSON 键，「拒绝重复键」契约必须手写 visitor 逐键迭代。
- 管理器同步桥接：专用 tokio runtime＋`block_on` 驱动 SDK 异步面；`sleep()` future 必须在 block_on 的 async 块内构造（块外构造会报「no reactor running」）。

## 022 性能轮经验（2026-10-24 方案 A，同日批准交付本地 main）

- 通知计数（stale_epoch）到达即原子可见；无条件 settle 兜的是**在途通知**（响应已回、通知尚在网络管道）——因此等待只需发生在「上次调用之后」（notify_grace 条件化），纯主动通知无需等待。等待须计入轮准备耗时，不得移出计时区间。
- 「进程死亡边界」的调用分类（未发出 vs 已发出状态未知）取决于 send 与服务循环退出的相对时序，测试断言应要求**二选一且语义正确**（区分即可），不应钉死其一——过强断言会在无关改动引起的时序微移下假红。
- 基线脚本/夹具/原始数据须归档进仓库（scripts/perf＋docs/testing/perf）后，/tmp 才可清理；历史版本测量一律走隔离 worktree＋专用 --target-dir。

## 工具搜索的目录与上下文边界（2026-10-24，仅文档）

- 030 规格路线（2026-10-26，待实现）：用户同意按原表推进到「工具搜索与按需加载」；采用宿主本地搜索与普通函数适配，五协议共用，动态 MCP 工具命中后在下一请求声明。每次请求固定执行集合，目录有该项或同批刚命中都不能直接调用；完整要求、开关和边界见 [030](../docs/specs/030-tool-search.md)。

- 区分“宿主保留的工具目录”和“模型当前看到的工具定义”。Codex 客户端搜索可先给来源概览，命中后再给具体用法；来源信息可以缺省，不保证逐项列出工具名，也不能按本地／云端判断是否延迟加载。固定源码、搜索前后可见内容及本项目差距统一见[工具对比末尾的源码复核](../docs/records/2026-10-21-codex-tools-comparison.md)。

- 工具搜索规划前须先定模型协议路线，不能把“目前只支持 Chat Completions”视为已确定的长期架构。该接口仍受支持，Codex 则已移除 chat 协议，Claude Code 使用 Messages 等格式；各产品事实、来源和本项目待定建议见[模型接口核对](../docs/records/2026-10-24-model-api-protocol-comparison.md)。

## 原生模型接口路线（2026-10-24 决定，2026-10-25 实现中）

- 用户决定保留 Chat Completions，加入 Responses、Anthropic Messages 和 Gemini 原生接口；先完成接口层，再继续 MCP 资源和工具搜索。统一的是业务与工具执行，协议层须保留各厂商续接必需的原生块和签名，不能只取可见文字。具体范围、兼容规则与验收统一见 [024 规格](../docs/specs/024-native-model-apis.md)，不以兼容桥接代替原生支持。

- 会话升级不能只检查 `Session`：记忆的倒序读取器还有独立的 meta 版本门。024 首次测试虽覆盖了原生模型调用，来源仍是 V1；改成真实 V2 来源才发现提炼被拒。格式变更须盘点所有读取路径，并用新格式跑完整下游流程，见[实施记录](../docs/logs/024-native-model-apis/2026-10-24-implementation.md)。
- 本地 mock 的 HTTP 长连接会受 Nagle／延迟确认影响，可能把约 3 ms 的操作测成约 50 ms。先用同一客户端的固定响应控制组、单变量修改 mock 的 `TCP_NODELAY` 定位；不能为夹具延迟修改业务实现或倒填基线。脚本、载荷、样本和原型源文件须一起归档后冻结阈值。

- 外部 SSE／JSON 组装器先把分片保存再按字段赋值时，任何“先收下后使用”的字段都要在入口验型：serde_json 对字符串／数组做字符串键索引赋值会直接 panic（进程级崩溃，不是错误返回）。为每个畸形形态断言“走 Err 路径”，别只测 happy path 的反例。见 [实施记录“交付收口”](../docs/logs/024-native-model-apis/2026-10-24-implementation.md)。

- 协议字段的“可选”须按官方 Schema 核对：Gemini `functionCall.args` 可省略（无参函数），强制按对象校验会误拒合法响应并连带阻断同批合法调用。缺省与 null 按 `{}` 等价处理、显式类型错误仍拒绝，是兼容性与严格性的平衡点。

- 给参数加特殊分支（如 `stream` 只查类型）会绕过统一能力门：新参数接入既有校验入口（`check_request_param`）比单独分支更安全；单测夹具里已声明的能力列表（`supported_params`）要随门收紧同步补齐，夹具缺口会以“修复破坏测试”的形式暴露。

- 会话这类“每条记录都回调”的保存路径里加格式判定（如 needs_v2），必须用增量游标只看新增段：全量扫描在百万条历史（256 MiB）下把单条追加拖成毫秒级（O(n²) 累计），单元测试与小型夹具完全暴露不出来，只有基线驱动的性能分层测试能抓住。红例见 [024 实施记录](../docs/logs/024-native-model-apis/2026-10-24-implementation.md)。

## 033 沙箱矩阵实测经验（2026-10-27，验证轮）

- 复用 e2e_acceptance 的 mock 夹具时，授权预置 `seed_grant` 的 `cred_fp` 必须是**密钥值**的指纹（`catalog::fingerprint(<key 值>)`），不是任意字符串——指纹不匹配会落到交互式授权提问，PTY 等待横幅超时才暴露。工具结果断言前先核对实际 JSON 形状（如 apply_patch `changed_files` 是 `{path,action}` 对象数组）。
- 进程回收类测试必须区分 016 契约的两种终止：**取消**不杀进程（`session_id` 保留、由可续接会话持有，`close()` 才整组回收）；**超时**到期即 killpg 组杀。断言「取消后无遗留」要在 `close()` 之后或显式终止之后，否则把契约行为误判为缺陷。
- 依赖进程级环境（TCHAT_SANDBOX）的测试用**双分支＋子进程显式构造环境**：测试文件只读环境不 set/remove（沙箱/普通两分支断言互为对照）；需要精确控制 PATH/开关的场景经子进程模式（`--exact <child>` + env 构造，同 tool_child_mode 模式）——`env -u` 全量回归与部署注入态下同一套测试都成立，无并发污染。
- 判定「工具真实进入沙箱」须多信号合取（载荷 pid=1／PID ns inode ≠ 主进程／沙箱外哨兵进程不可见/HOME=锚定根），单信号各有假阳性面；探针输出缺失行本身也算证据不成立。bwrap **不关闭继承的非 CLOEXEC fd**（fd ≥3 透传，`--pass-fd` 语义的反面）；产品侧 fd 面依赖 Rust 默认 CLOEXEC。
- e2e_acceptance.rs 文件头注释（「exec_command/apply_patch 未接入 CLI」）已过时：s103 与 turn.rs 分派表明两者均已接入 CLI 工具循环（017 起四工具）。本轮未顺手改（与任务无关），后续维护该文件时应更正。

- B1/B2 核实轮补充（2026-10-27）：上游 codex `d8acdf1` 的 linux-sandbox 三文件（bwrap/linux_run_main/launcher）均无 nlink 检查与继承 fd 清理——硬链接别名与非 CLOEXEC fd 透传为**与上游共有**的边界，修复属超出上游的加固。测试内给子进程传 fd 用**动态编号**（清 CLOEXEC＋格式进命令），固定编号（如 fd 9）会与并发用例的 PTY/套接字 fd 互相踩踏（dup2 覆盖他人、他人 Drop 关闭我方），合跑才暴露。
- 测试内全局覆盖缝（进程级原子量如 `GIT_TIMEOUT_MS_OVERRIDE`）必须**写者与读者同时串行**：只有写方持锁时，读者测试与覆盖窗口并发会把测试专用值泄漏进真实路径（300ms 预算泄漏致全量偶发失败；panic 报文里的非默认数值就是覆盖值指纹，默认值对不上即可断定泄漏）。修复＝读者持有同一串行锁，断言不动；「基线也失败＋单跑通过」只是波动证据，报文指纹才能定位真根因。
- 安全分类不能用 `Path::exists()` 判「不存在」——它把任何 stat 错误（EACCES/ELOOP/ESTALE）折叠成 false，「状态不明」与「确认缺失」被合并放行（fail-open）。fail-closed 分类用一次 `fs::metadata` 三分：Ok ／ NotFound·NotADirectory 类 no-op ／ 其余显式拒绝。上游逐字保留的错误吞噬语义（如符号链接遍历 `Err(_) => break`）不改，但下游防线要兜底。稳定触发夹具：自指符号链接（ELOOP）与父目录权限 000（EACCES），无需竞态；断言后先恢复权限再清理夹具。
- 沙箱包装的 `--die-with-parent`（PDEATHSIG）锚定**创建者内核线程**而非进程：工具 spawn 发生在随用户轮退出的 turn 线程时，轮结束即整树 SIGKILL——「取消不杀／跨轮保留」契约在沙箱模式静默失效，而探针（测试主线程长寿）与库级用例全部漏过；只有真实 CLI＋短寿轮线程的部署态 e2e 才暴露（同款上游问题见 codex issue #46702，0.155–0.162 未修）。修复＝fork/exec 移交长寿创建线程（exec 面：管理器专属 std 线程＋mpsc 请求/回复；Tokio 面经 `tokio::spawn` 移交**核心 worker**——blocking 池 ~10s 空闲退休正是触发机制，不可用）。测试侧：沙箱 ns 内记录的 pid 在外层恒不可用（ns 内 pid1≈外层 /proc/1 恒存在），存活/回收判定用外层 argv 扫描（僵尸 cmdline 为空天然排除）。

## 029 实施期教训（2026-10-28）

- **门检查与解析必须共用同一定义（构造性保证优于约定）**：`logical_lines` 曾用 `str::lines()`（仅按 `\n` 切分）计数，而 `SourceFile::parse` 把孤立 `\r` 也当行尾——lone-CR 文件以「1 行」通过 10 万行读入门、实际展开百万级行（攻防 F-1，实测 165MiB 峰值）。修复后两者同源；进一步收敛方向是抽零分配 `line_ranges` 原语供两者 collect/count（审查发现 #10）。同类「两处定义悄悄分叉」也出现在 Move/Update 打开样板与读入管线（审查 #3/#7）。
- **资源上限检查要对称覆盖双向**：F-2 伪文件检查只拦「读取字节数 > fstat 尺寸」（procfs st_size=0 有内容），漏了「读取 < 尺寸」（fstat 与 read 间被并发截断）——短读残片被当完整内容移动并删源（审查 #8，data-loss 类）。教训：凡「读入窗口内对象可能变化」的校验，加检查时两个方向都要过一遍。
- **bwrap 遮蔽的真实形态**：033 的 `--ro-bind /dev/null <deny路径>` 在 user namespace 内**打开即 EACCES**（非读到空）——工具侧表现为「打开失败」而非「非常规文件」，测试断言应取并集；遮蔽路径表现为 `/dev/null` 字符设备（crw-rw-rw- nobody nogroup）。
- **cargo 多 --test 目标共享单一过滤器**：`cargo test --test A filter --test B` 中 filter 同时作用于 A 与 B——B 可能被滤成 0 运行而 exit=0，日志记成绿灯（审查 #1 的证据链缺口即由此产生）。多目标验证须整目标分别运行或各自过滤。

## 032 上下文管理（2026-10-28 实现，待验收）

- **TCHAT_SANDBOX 环境泄漏会整体扭曲测试基线**：交互 shell 遗留的 `TCHAT_SANDBOX=1` 使全部工具命令经 bwrap 包装，`trap '' INT` 等 OS 语义在沙箱内失效，proc_session 计时类用例确定性失败（029 上午基线通过、下午失败，代码未变）。**测试/验证命令一律 `env -u TCHAT_SANDBOX` 前缀**；遇到「基线 inexplicably 失败」先查环境变量再查代码。
- **检查点/双视图类功能的校验要分层放置**：链级语义校验（代数连续等）放装载与提交两个状态变化点（O(n) 各一次），请求/查询热路径只做轻量本地校验（O(活动尾部)）——首版把全量校验放进了每次请求组装，审查轮 4 位审查者独立标记线性恶化。
- **「纯函数＋单一入口」是防重复漂移的关键**：候选校验序列曾在 turn 分派臂与批末提交两处逐字重复（相距 220 行），抽成 `evaluate_candidate` 单一入口后消除；工具名/标记文案全部走导出常量，main.rs 曾裸字符串分派（改名即静默漂移）。
- **死代码零容忍的价值**：审查轮揪出的 `provider_id` 死字段、无人用的 `impl CheckpointHost for Session`、测试里的 `#[allow(dead_code)]` 与恒真断言（`late_delivered` 永不填充）都是「看起来覆盖了、实际空转」的陷阱——测试断言必须验证真实出口，不是摆设。
- **原子迁移先演练后替换**：会话 V1/V2→V3 迁移首版先切版本再写文件，磁盘满时把健康的自动保存目标永久降级；改为「局部建新文件＋完整开篇＋检查点行全部成功后一次性替换 self」后，失败路径零副作用。
- **跨模型审查路线依赖 acpx（node/npx）**：本容器有 codex CLI 但无 node，对抗镜头只能进程内承担；需要真跨模型审查时装 node ≥22.13 或预先确认路线。

## 030 工具搜索（2026-10-29 实现，待验收）

- **serde 派生结构体在严格参数契约上有两个静默坑**：①`Option` 字段把显式 `null` 折叠成缺省，双层 `Option<Option<T>>` 救不回来（外层在派生层就消费了 null）；②派生 `Deserialize` 接受**数组位置形式**——`["query"]` 会按字段序解构成功。拒绝重复键/未知字段/数组形式的模型输入契约必须手写 visitor 且只 `deserialize_map`（022 配置侧的先例在工具参数侧同样成立；bm25 门禁轮实测两者）。
- **冻结性能基线的原型必须建模真实数据形状**：M0 原型只持检索文本，RSS 阈值按其冻结；真实模块按规格须保留完整定义（Schema）供搜索结果返回，2048 档 RSS 增量 5×（1.6→7.8 MB）超原型阈值。正确处置＝按规格「实测后更新本节」以真实模块重冻结并保留双份样本＋修订说明（实施期登记），不是为过关放宽阈值、也不是让实现迁就错误基线。
- **rust_stemmers/deunicode 管线对超长单词病态**：数十万字符的连续单词（测试用 "x"×N 做 filler）使 tokenize/stem 进入分钟级 CPU；大体积夹具用**词组 filler**（`w00001 w00002 …`）既真实又线性。
- **bm25 2.3.2 `matches()` 同分顺序任意且随目录构建顺序漂移**（正/倒序目录两次运行给出不同排列）——调研预告实测证实：必须在**完整正分候选**上二次排序（分数降序＋(server_id, raw_tool) 字节升序）后截断；库的 sort_by 无身份 tie-break。
- **跨模块传下标是缺陷温床**：SearchState 曾把"敏感过滤后条目下标"交给调用方去索引未过滤的快照定义数组（实现期自审发现）；审查轮又确认按名回扫的 O(命中×目录) 冗余。结构解＝单账本（只存升序下标，名字集合投影派生）＋检索直接返回 (下标, 条目) 对，两套序天然不可分叉。
- **测量型门禁的自身失效必须可见**：RSS 子进程失败/输出缺失曾被 `unwrap_or(0)` 折叠成"0 增量＝达标"，门禁恒真。守卫自己的失败路径也要断言（exit 状态＋必需输出行存在且可解析）。

## 034 JavaScript 工具编排实施（2026-10-29，已交付）

- **rquickjs 0.14 的桥接形态**：`Function::new` 闭包返回 JS 引用类型（Promise/Value）会撞生命周期墙（闭包按 `'js` 单态化）；正解＝JS prelude 装配 Promise/结算函数，Rust 闭包只收发基本类型（`__dispatch(id,payload)` 入 outbox 邮箱、经 `Persistent<Function>` 调 JS 侧 `__settle(id,ok,value)`）。`eval_promise`（JS_EVAL_FLAG_ASYNC）支持顶层 await，但 resolve 值不可靠——完成/失败只看 `state()`（变体名是 `Resolved` 非 Fulfilled）。
- **观察函数单点驱动**（§1.4 本地调度适配的正确形状）：叶子完成与 worker 帧共用一条 mpsc 事件通道，观察循环 `recv_timeout` 单点处理——若叶子完成走独立通道，recv 阻塞期间无人收割＝死锁（观察等帧、worker 等回包）。让出期背压由 worker 写 stdout 阻塞自然形成。
- **V3 事件提交的游标吸收**：事件行先落盘、调用方观察返回后才入内存历史——直接推进 `persisted_upto` 越过事件会在下一次 commit 越界（实测 4>3 panic 两次）。解法＝`orch_untracked` 计数：commit 不推游标，flush/开篇按计数跳过内存中已写的事件前缀（吸收循环），顺序恒一致且不重写。
- **测试环境竞态的既有窗口**：turn 内嵌测试经 `execute_core_tool` 真实执行 apply_patch，与 sandbox/tests.rs 的 `TCHAT_SANDBOX` 写者并行时被拉进沙箱分支（实测 apply branch: frozen=false sandbox_req=true）——033「覆盖缝读者与写者同时串行」教训的再次实证：读者（turn::tests）持 `TEST_ENV_LOCK` 即闭合，禁用新测试也无法改变交错。
- **性能门禁在容器下的双态基线**：轻载首采（spawn 13ms）与持续运行（每场景 100×3 次 spawn 后 100–190ms）差一个数量级且可复现（CPU 配额节流）——阈值按持续运行实测冻结，门禁定位为防数量级劣化；n=30 的 p95 只容 1.5 个离群，过脆，断言与采样统一 n=100。
- **变异证明的价值再证**：「事件不进模型投影」首轮无任何测试能捕获破坏——补 user 消息计数断言后闭环。新增关键不变量必须配可失败的断言，否则视为无保护。
- **安全攻防实证的 fd 收口缺口**：worker spawn 只做了 env_clear＋进程组＋RLIMIT，漏了 fd 收口——宿主无 CLOEXEC 的哨兵 fd 实测以 fd 3 进入 worker。033 的 `close_range` 收口只在沙箱分支注入，编排 worker 普通模式也要同样处理（`close_range(3,~0)` best-effort，内核 <5.9 不阻断）。安全套件以 /proc/<pid>/fd 哨兵红→绿闭环。
- **额度「先扣后查」的口径要在规格写死**：轮 128 计全部格式可识别帧（含错误参数/资格拒绝——总量封死探测）、cell 31 计合格受理——两个口径不同层，实现与规格字面读法有解释差时按「总量封死」优先并回写规格明确。
- **独立审查四项 P1 的共性**：①wait 句柄释放条件写成 `status != Running`（终态输出跨观察交付时丢增量）——句柄释放只看 `is_closed()`；②轮额度在 exec 分支内 new（每 cell 一份 128）——跨 cell 共享预算必须由轮入口创建并注入；③事件序号「先增后存」在保存失败时留空洞（自研校验反噬整档）——序号只在提交成功后占用；④进程内自增 cell_id 在 /resume 后与旧事件碰撞——进程外唯一分量（启动纳秒）是最低成本防线。四项都在「跨边界状态」（观察间/轮间/进程间）——单轮 happy path 测试天然盲区。

## 031 权限申请实施与审查（2026-10-29，已交付）

- **seccomp cBPF 的 per-syscall 参数检查必须先门控 nr**：clone-flags 掩码块若对全部调用执行，任意调用的 args[0]（如 execve 的路径指针）按位与掩码几乎恒非零 → 全载荷 EPERM（集成首跑 `execvp /bin/sh: Operation not permitted` 实证）。结构：arch 门（偏移 4）→ nr 拒绝清单（偏移 0）→ JEQ clone 后才做 flags 掩码（偏移 16）→ RET ALLOW 收尾；8 字节整除是 bwrap 的硬校验。
- **直连 bwrap 子进程是 PID ns 之外的监督者**：其自身 ns/pid_for_children 与主进程相同属正常形态；逐启动隔离验证须遍历 `/proc/<bwrap>/task/*/children` 找**载荷**进程比对其 ns/pid，且要有界轮询（bwrap 多次 re-exec，spawn 返回时载荷可能未出现）。
- **§1.2-3 的实现陷阱：基线约束的键是「冻结策略」不是「权限宿主」**：把快照构建挂在 perm 状态上会让非交互管道/未声明能力的模型静默回退 033 全盘读（审查 P0；MCP stdio 恰好按 frozen() 正确，同会话三种执行面强度）。正确形状：`frozen().map(|f| build_snapshot(f, 无宿主时空授权, f.deny_ctx))`。
- **mountinfo 解析必须扫描 `-` 分隔符**：可选字段（shared:N/master:N）零或多，固定下标 `fields[6]=="-"` 会整行丢弃带传播标记的挂载——恰是同设备 bind 别名最密集的类（本容器 32 行中 5 行）。同设备双挂载的别名投影测试需要**真实存在的 deny 路径**（deny_aliases 先取 st_dev，不存在即早退）。
- **信任来源检查一律用 canonical 路径**：策略路径/exe 路径的 `..`/符号链接词法形态可绕过组件前缀比较；同模块先 canonicalize 再比较（exe 侧）与不 canonicalize（policy 侧）并存就是漏掉的证据。同一 env 变量读两次（一次 trim 一次不 trim）会让 deny 安装目录装配静默落空——单次读取返回 (值, 路径)。
- **「拒绝开始下一轮」类契约需要宿主侧门，不只是错误文案**：ProcessManager 落收尾失败闩锁（失败置位/成功清除），两宿主轮入口查询＋重试一次 reap；§1.7-4 的「成功切换」出口（032 commit）同样要调用回收。
- **fail-closed 分派的三分支形状**：`Some(Some(快照))/Some(None)＝毒丸显式错误/None＝非 031 原路径`——把毒丸折叠进 None 分支就是 H1（apply_patch 回退 033 全盘）的根因；启动期基线预检（baseline_snapshot）把拓扑型毒丸提前到启动失败。
- **测试有效性三连坑**：`sleep 30` 不读 stdin → write_stdin 断言恒真（须读循环 shell＋先注入存活标记）；`assert!(x || true)` 恒真断言；把「超限先弹窗后失败」钉成 `host.calls==1` 预期（§1.7-1 是确认前失败）。安全攻防脚手架修复回归的归属要在文件头与 git 状态一致（要么常驻回归集，要么迁走再删）。
- **bash 的 /dev/tcp 语法是 host/port（斜杠）不是 host:port**——冒号形态 ENOENT，曾被误当成沙箱/seccomp 缺陷排查了一整轮（对照变体矩阵才定位）。
- **授权模型单调时，挂载面必须先化简为规范形**：read 严格位于 write 之下＝冗余授权（write ⊇ read，access 只累加；「只读 carve-out」不存在，收窄原语只有 deny）。不化简时深度升序挂载使冗余 ro-bind 遮蔽祖先 rw 面——已批准写权限在子树内静默 EROFS，且 covers() 判「已覆盖」让再次申请无法自愈（审查 #15）。快照构建处一次 retain 即闭合，deny carving 不受影响。
- **性能计量的 face 必须断言载荷真实成功**：载荷 chdir 落在未挂载面时 bwrap「Can't chdir」快速失败 ≈6.5ms 恒定——无成功断言会把失败样本当性能基线（本轮补丁面首测 8–17ms 即此伪基线，修正后真实 ≈65–130ms）。另：debug 载荷二进制进程启动（≈60ms）远大于 bwrap＋/bin/true（≈5ms），faces 计量要区分载荷成本与沙箱成本并注明构建口径。
- **安装 deny 白名单内外分级（031 方案 A，2026-10-29 用户裁决）**：策略目录无论位置目录级 deny；应用/bwrap 安装对象位于固定运行时白名单树（/usr、/etc，usrmerge 下 bwrap 即在 /usr/bin）之内时单文件 deny，之外目录级 deny。依据：目录级 deny 共享运行时树会遮蔽 shell/coreutils 且 dpkg 合法硬链接（nlink=2）使目录递归检查必拒＝功能不可用；保护对象（二进制/bwrap 本体）遮蔽与 nlink/别名/逐启动复查不变。后端版本/摘要按 §1.6「可追溯」走部署记录（发行版包固定＋SHA-256 记录），不做逐次运行时校验。

## 035 多 Agent 实施（2026-10-29，已实施待验收）

- **注册表状态与事件必须同点发布**：子线程先提交 state_changed(running) 事件、未把注册表 Starting→Running，终文邮箱门按注册表判定把全部子任务误判为非运行（探针实证：终态全 interrupted）。事件是持久化证据、注册表是门判定事实，两者要在同一提交点一起翻转。
- **配对型预留/释放要逐出口核对**：全局 3 子槽在「预算拒绝」「保存失败」「线程启动失败」三类早期出口各漏一次释放（c15 槽位排空等待暴露）；预留型资源的每个 return 路径都要有对应 fetch_sub/回滚，与 018-B「多个错误出口收敛单一收尾」同根。
- **「调度泵」的锁链等价实现**：规格要求事件经有界通道交泵线程串行落盘；用 writer 互斥＋`writer→session` 单向嵌套同样满足「单写者、根阻塞期间持续落盘、seq=文件序」，且少一类线程与背压面。前提是锁纪律成文并全链遵守（core/writer 绝不同持）。
- **测试全局资源的套件要串行＋真实排空**：全应用子槽是进程级共享，cargo 默认并行会让两个用例互相挤成伪 CAPACITY_LIMIT（首轮 14 失败的根因）；套件级 Mutex 串行＋Drop 时轮询槽位归零（不 assert——unwind 中二次 panic 会 SIGABRT）后，单线程/并行四轮全稳。
- **007 装载规则与纯事件文件**：「头部非 user 记录丢弃至首个 user」会把只有 Agent 事件的测试文件合法裁空（load Ok(0)），坏链用例必须带 user 前缀才真正进入链校验——构造装载反例时先过一遍该规则的路径。
- **机械批量改测试的结构体字面量**：给 TurnExtras 加字段需改 50+ 处字面量；括号计数会误伤字符串内花括号，行尾 `\}` 系＋缩进分「嵌套字段闭后插/字面量闭前插」两段式可行但仍有单行 `};` 漏网——最终以编译错误清单驱动收尾最省。
- **「关闭事件」必须是该写者的最后一个事件**：组关闭（group_closed）先落盘、在途子事件后到，会让装载链校验把整个会话文件判死（/resume 不可用）——收尾标记的提交时机要绑定「全部相关线程排空」，由最后退出者经 CAS 恰一次补写，主出口不阻塞等待；配合迟到返回门（响应到达先查关闭/取消）把确定性路径也堵死（035 审查 B1）。
- **`let _ = admit();` 式预算旁路**：准入函数返回 Err 后继续执行工具＝零预算下真实外部动作（035 审查 B2）；准入失败要就地生成失败结果占位（保持调用配对完整）并跳过分派，注释承诺与实现行为必须一致。
