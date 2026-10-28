# 规格：用户轮上下文注入（项目规则 AGENTS.md ＋ 环境信息）

> 状态：**已实现并验证**（2026-10-20 实施轮完成：RED→GREEN→综合测试→安全门禁→独立审查→L3 全量；待用户批准提交）　更新：2026-10-20
> 需求来源：2026-10-20 用户指令——依据已验收调查提交 `2d7be44f0a403d1656aac89ecce1c959366b86ea`（[Codex 上下文组装调查报告](../records/2026-10-20-codex-context-assembly.md)＋[实测样本](../records/2026-10-20-codex-context-assembly-prompt-sample.json)）准备本项目上下文组装的下一个最小交付。
> 上游锚点：openai/codex 固定提交 `014489466a1739c2c563cdfe57880c2c7c8680e3`（调查报告 §2 清单 #5/#6、§5.2、§8.2/§8.8；本规格不引用未在该报告核实过的上游行为）。
> 与既有规格的关系：005 交付了最小组装主链（system＋两工具＋历史＋本轮输入）并**明确排除** AGENTS.md 自动注入（§1 范围）；本规格补上该缺口并**反转 005 适配 3**（见 §3-5）。006（工具循环/历史投影）、007（会话持久化）、013/014（记忆系统，含 D2 轮级触发、D3 合并轮受限工具、D4 记忆 system 尾段注入）契约全部沿用，不改。

## 1. 目标 & I/O

- **目标**：每个用户轮开始时，加载「项目规则（AGENTS.md 链）＋环境上下文」，构造**恰一条 user 角色上下文消息**注入模型请求（位于本轮用户输入之前）；内容与上次注入相同则不重复注入，不同则注入一条带替换声明的新消息；该消息作为普通历史项进入历史并按 007 契约持久化。
- **注入入口（实施轮定案）**：`turn.rs` 新增用户轮入口 `run_user_turn_with_context`（包转 `run_tool_turn_with_mode`）——上下文消息在压入本轮用户输入**之前**压入并 `on_record`；`run_tool_turn_with_mode` 自身签名与行为保持逐字节不变（006/012/013 全部既有调用与测试零改动），两宿主（CLI/TUI）改调新入口。仅 `ToolMode::User` 与 `UserWithMemory` 两模式注入；`MemoryMerge` 内部轮不读取、不注入。
- **输入**（全部为本项目既有机制，无新增配置项）：

| 来源 | 加载条件 | 缺失/为空时行为 | 限制 |
|---|---|---|---|
| 全局规则 `config_dir()/AGENTS.md`（`TCHAT_CONFIG_DIR` 可覆盖，`catalog.rs::config_dir`，002 机制） | 每用户轮读取一次（与 013 记忆上下文同位：轮开始、轮内续请求不重读） | 文件不存在或纯空白→跳过该来源 | 与项目链共用一条正文预算（见下） |
| 项目规则链：从 cwd 向上找**最近**含 `.git` 的祖先（含 cwd 自身）作为根，根→cwd 每目录至多读 `AGENTS.md` 一个文件（仅此文件名），按根→cwd 顺序拼接；无 `.git` 祖先时只查 cwd 一个目录 | 同上 | 单目录缺失/空白→跳过该目录；全链为空→省略整个 AGENTS 块 | 同上 |
| 环境上下文：cwd（进程工作目录）、shell（环境变量 `SHELL`）、UTC 日期（注入时钟，见 §3-6） | 恒定加载 | `SHELL` 缺失或纯空白→省略 `<shell>` 行；cwd/日期恒存在 | 文本恒定有界，不占正文预算 |

- **文件异常边界（本项目适配，纳入验收）**：仅自动加载「`AGENTS.md` 本身是普通、非符号链接文件」的来源——先 `symlink_metadata` 判定：符号链接、目录/FIFO/设备等非普通文件→跳过该来源；缺失→静默跳过（正常路径）；读取失败或内容非法 UTF-8→跳过该来源。跳过均不中止正常用户轮，经宿主提示通道记一行原因（含路径与原因类别，**不含规则正文**）。读入本身有界（见下），不做整文件读入后再截断。
- **正文预算与拼接（唯一截断策略）**：规则正文按「全局→项目根→cwd」加载顺序拼接为单一 `content`：每个来源文本先做首尾空白修剪（trim），来源间以固定分隔 `\n\n--- project-doc ---\n\n` 连接（分隔计入预算）。`content` 逐字节预算 **32768**（UTF-8）：统一保留预算内拼接结果的 UTF-8 前缀，尾部按 UTF-8 字符边界回退（不产生无效文本；`</INSTRUCTIONS>` 包装行与环境块不计入预算、恒完整）。单文件读取上限＝当前剩余预算（＋至多 3 字节判定边界）；预算耗尽（含恰好占满）后停止加载：**不再打开任何后续来源文件**，仅以元数据判定存在后续普通文件来源时记一行提示（纯空白来源可能被多报——罕见角形，换取「不再读取」的严格性）。发生截断时记一行提示（不再有「整段丢弃」二选一）。
- **输出**（一条 `HistoryItem::Text(HistoryMessage{role: User})`；块内布局对齐调查样本 item5——标题后空行、内容行包装、段间分隔行；AGENTS 块在前、环境块在后，块间恰一个空行）：

  ```
  # AGENTS.md instructions for {cwd}          ← 仅当存在项目链内容时带 ` for {cwd}` 后缀；仅全局规则时无后缀；无任何规则时整块（本行至 </INSTRUCTIONS>）省略
  （空行）
  <INSTRUCTIONS>
  {规则正文 content，段间 \n\n--- project-doc ---\n\n}
  </INSTRUCTIONS>
  （空行）
  <environment_context>
    <cwd>{cwd}</cwd>
    <shell>{SHELL}</shell>                    ← 缺失或纯空白时整行省略
    <current_date>{YYYY-MM-DD}</current_date> ← UTC
  </environment_context>
  ```

  字节级形态：有规则时 `# AGENTS.md instructions[ for {cwd}]\n\n<INSTRUCTIONS>\n{content}\n</INSTRUCTIONS>\n\n<environment_context>\n  <cwd>…</cwd>\n[  <shell>…</shell>\n]  <current_date>…</current_date>\n</environment_context>`；无规则时消息即环境块本身（以 `<environment_context>` 开头、`</environment_context>` 结尾）。`<cwd>`/`<shell>` 值按 XML 文本规则转义（`&`→`&amp;`、`<`→`&lt;`、`>`→`&gt;`）；标题行中的 cwd 不转义（非 XML 上下文）。日期由注入时钟 epoch 秒换算 UTC `YYYY-MM-DD`（chrono 既有依赖，无 clock feature，013-M4 模式）。
- **注入与去重（识别顺序固定）**：构造当前上下文消息后，与历史比较决定是否注入：
  1. 向后扫描历史 user 文本项，对每项**先剥除至多一条完整替换声明及其约定分隔**（声明行＋空行，仅当逐字节前缀匹配时剥除）；
  2. 再**识别完整上下文结构**（剥除后文本须以 `# AGENTS.md instructions` 或 `<environment_context>` 开头、以 `</environment_context>` 结尾，且含完整 INSTRUCTIONS 包装/块间结构——仅相似标题或标记开头、结构不完整的普通用户文本不认定为上下文）；
  3. 最后**比较规范化正文**（＝剥除声明后的全文）与当前构造文本：相同→不注入；不同→注入，且此前存在过上下文消息时新消息在最前加替换声明行 `The following context replaces the previously provided AGENTS.md instructions and environment context.`（后随空行）；不存在→注入裸消息。
  识别与比较不得先按原始文本前缀筛选（否则替换消息会漏识别）。比较基线完全由历史承载，resume 后自动恢复。
- **约束**：同一轮内续请求（工具循环）不再读取或注入；同一输入产出相同结果；本轮输入仍不得全为空白——**空白输入的既有校验先于上下文写入**（拒绝输入时，历史与 `on_record` 不得新增上下文或用户项）；来源（目录、cwd、SHELL、时钟）全部参数化注入，测试不得依赖真实用户目录或真实时间（AGENTS.md 测试规范）。
- **范围外（明确不做）**：`AGENTS.override.md` 与可配置 fallback 文件名；CLAUDE.md 等其他规则文件名；线程级指令层；世界状态逐 section 差量与独立的替换/移除声明语义（本地简化为整消息比较＋单一替换声明）；timezone/shell_version/platform/filesystem/network 环境块；按采样步骤刷新（仅按用户轮）；上下文文件监视与动态通知；压缩、WS 增量、MCP/插件/Skills 发现（调查边界参考，非本目标依赖）；新增任何配置项。

## 2. 边界 & 风险

- [ ] 上下文消息进入历史引发的耦合面：①07 会话保存/恢复重放（ctx 即普通 Text 项，无格式变更，恢复后去重基线必须来自重放历史）；②013 记忆提炼输入过滤——**实施轮核实发现**：既有过滤器按「以 `</INSTRUCTIONS>` 结尾」整条匹配，而 015 合并块以 `</environment_context>` 结尾，真实 ctx 消息**不会**被既有过滤命中；需最小适配——过滤识别复用 015 的同一识别器（剥声明→整结构），既有 `</INSTRUCTIONS>` 结尾与 `<skill>` 两类过滤行为保持不变（既有用例零改动验证）；③e2e 精确位置断言（`e2e_acceptance.rs:483-492`、`:577-579`、`:1190-1191`、`:1280-1281` 等硬编码消息序/角色序）随实施**更新断言而非放宽**；管道模式测试的 cwd 必须落到临时目录（继承 `/workspace` 会读入仓库真实 AGENTS.md，测试不得依赖仓库内容）。
- [ ] TUI `/resume` 投影：`tui_host::project_history_items` 把 user 文本项直映为用户气泡——ctx 消息不得被当成新的真实用户发言展示，投影层按同一识别器跳过 ctx 项（仅展示层过滤，历史与请求面不变）；CLI `/resume` 仅打印摘要、不回放条目，无需改动。运行中注入本身不产生任何展示事件（无 DriverEvent/say），CLI/TUI 交互不变。
- [ ] 重复注入：去重比较基线丢失（忘写 on_record、resume 后基线不可恢复）、替换声明未剥除导致恒判不同、日期跨天触发非必要替换（已知简化，接受并记录）。
- [ ] 005 适配 3 反转改变 `sent_instructions()`：prompt_assembly `s2` 的独立重建与"exec_command tool"断言按新声明更新；嵌入母本（`assets/codex/prompt.md`）与发送版仍是两个可分别校验的文本（005 契约不变）。
- [ ] 预算截断切在 UTF-8 多字节字符中间产生无效文本；截断必须发生在 `<INSTRUCTIONS>` 内容文本上（保留闭合标签，消息结构完整）；有界读取在字符边界收尾（＋3 字素判定，非文件本身损坏）。
- [ ] AGENTS.md 内容随请求外发到模型服务（与 Codex 同面）；本注入不经过 `display` 层（非终端输出路径）；文件内容由使用者在本地控制，不做密钥扫描（边界记录，安全审查复核）。跳过/截断提示行不包含规则正文。
- [ ] 时钟/环境来源在测试中不可控：必须沿用 013 `Clock` 注入模式与可覆盖的 cwd/config_dir 来源，禁止测试依赖真实时间或真实用户目录（AGENTS.md 测试规范）。

## 3. 关键判断（为什么）

1. **选「项目规则＋环境注入」为最小缺口**：005 主链与 006 工具循环已交付（调查确认本项目已具备"一次模型请求的上下文组装"最小主链：system＋工具声明＋历史＋工具结果＋本轮输入，样本对照见调查报告 §7.2 与本项目 `build_turn_request` 族）；调查 §2 清单中本项目仍缺失的一等上下文来源只有项目规则与环境上下文两项，且 005 适配 3（"AGENTS.md 未注入、请用 exec_command 自行读取"）正是该缺口的显式占位。压缩/世界状态差量/WS 增量按任务边界仅作参考，不纳入。
2. **user 角色消息进历史，而非仿照 D4 走 system 尾段**：证据三重——①Codex 将 AGENTS＋环境合并为一条 user 消息、置于真实输入之前、写入持久历史（调查 §2 #5/#6、样本 item5/6）；②本地 013 记忆输入过滤器已按 `# AGENTS.md instructions…</INSTRUCTIONS>` 用户消息标记格式预设（rollout_input.rs:180），历史位是既有设计预期；③规则是要求模型遵循的指令且体积可达预算上限，入历史一次注入、续请求与后续轮次经历史携带，避免每轮重复拼入 system（调查 §8.1 的位置策略权衡）。与 D4（记忆＝小体积参考摘要、system 尾段、按轮重读）并存：两类内容体积与语义不同，分置有据。
3. **去重＝整消息文本比较，不引入哈希或 section 差量**：Codex 的世界状态 section 快照/diff 是支撑「多来源独立差量＋替换/移除声明」的完整机制；本地最小等价是「构造当前消息→与历史最后一条上下文消息全文比较」。等价跳过、不同注入替换声明消息；比较基线完全由历史承载，resume 后自动恢复，零新增持久化状态。跨天日期变化触发一次替换注入——已知简化，影响一次消息体积，接受。
4. **预算 32 KiB 固定常量、全局＋项目共用**：32 KiB 是 Codex `project_doc_max_bytes` 默认值（调查 §2 #5）；本地沿用 003/004/005 的固定边界风格，不加配置项。Codex 预算只约束项目条目（调查 §7.4 未确认项 5），本地简化为全局＋项目合计一条预算——更保守（总量必不超），作为已批准适配记录。
5. **反转 005 适配 3**：提示词中的"宿主已做某事"声明是契约（005 坑004）；注入落地后声明必须改为与事实一致——新声明固定为：`The applicable AGENTS.md instructions and environment context are provided in a user message before the current input and don't need to be re-read.`（替换 005 现行"需要时用 exec_command 读取"声明句，其余适配不变）。
6. **UTC 日期复用 013 `Clock` 注入模式**（`memory/store.rs:58` `Arc<dyn Fn() -> i64>`＋`system_clock()`）：chrono 未启用 clock feature（013-M4 已锁定该模式），epoch 秒换算 UTC 日期无需新依赖；不引入 timezone（Codex 有 IANA 时区，本地从简，记录为差异）。
7. **`MemoryMerge` 不注入**：013 D3 合并轮使用受限工具集与专用 system，其历史是记忆内部合并语义；注入项目规则会污染合并输入且无用户轮语义。合并轮管线继续直调 `run_tool_turn_with_mode`（不经新入口），请求组装与 013 D3 逐字节不变。
8. **注入入口用包转函数而非改 `run_tool_turn_with_mode` 签名**（实施轮定案）：保持 006 契约函数与全部既有调用/测试零改动；新入口只增加「空白输入预检＋上下文注入」两步，随后完全复用既有循环。

## 4. 坑记录（预测，实测后回填）

- 坑001（已验证未发生）：ctx 经 `on_record` 落盘（t01 快照 1/2/3；session_cli e1/e3 落盘断言）；c5 通过。
- 坑002（实测命中）：llm_flow 2 用例、session_cli 8 用例、tool_loop 8 用例、e2e 5 用例因消息序/计数位移失败——全部按新序更新断言（未放宽）；llm_flow 两管道用例与 e2e `pty_cli` 默认 cwd 落临时目录（`/tmp`，无 `.git` 祖先），消除对仓库真实 AGENTS.md 的依赖。
- 坑003（已验证防住）：t09 多字节边界反例（截点落汉字中→回退 32766 完整字符）；变异 B（去掉边界回退）被 t09 击杀。
- 坑004（已验证未发生）：t02 缺失与纯空白均整行省略。
- 坑005（已验证防住）：t06/t08 通过；变异 A（不剥声明）被 t06/t07/t08/t14＋过滤用例击杀。
- 坑006（实测命中并已适配）：RED 阶段证实裸/带声明合并块消息均不被既有过滤器剔除；适配＝`is_memory_excluded_fragment` 增用 `context::context_body`（单一识别器），既有两类过滤用例零改动仍绿（c9）。
- 坑007（已验证防住）：`project_history_items` 跳过 ctx（tui_host 新用例，裸/带声明两形态）；CLI `/resume` 只打印摘要、无需改动（核实）。

## 5. 预测 → 实测（实施轮 RED→GREEN 验收面；实测列回填中）

| # | 场景 | 预测 / 验收 | 实测 |
|---|---|---|---|
| c1 | 首轮注入（临时目录：全局＋根＋cwd 子目录三层 AGENTS.md） | 首请求消息序恰为 `system → ctx → user输入`；ctx 文本与测试侧按 §1 字节级形态独立重建逐字节一致（含标题后缀、段间分隔行、块序、空行、环境块各行、XML 转义、UTC 日期） | ✅ t01：序正确、逐字节一致、on_record 快照 1/2/3 |
| c2 | 空值/缺失/异常文件边界 | 无任何规则文件→ctx 仅含环境块且无标题行；仅全局→无 ` for {cwd}` 后缀；`SHELL` 缺失或纯空白→无 shell 行；纯空白来源跳过；符号链接/目录/读取失败/非法 UTF-8 来源→跳过＋原因提示（不含正文）、轮正常完成；cwd/日期恒在 | ✅ t02/t03/t04（含最近 `.git` 根、跳空目录、chmod 000 读失败轮不中止） |
| c3 | 内容不变去重 | 同内容第二轮：历史中 ctx 消息恰 1 条（不新增），请求中 ctx 恰出现一次；轮内工具续请求不重读不重注入；A→B→B（B 注入后再次同内容不再注入，历史恰 2 条 ctx：A 裸＋B 带声明） | ✅ t05/t06（A→B→B 历史恰 2 条、请求恰 2 次出现后不再增） |
| c4 | 内容变化替换与识别 | 修改项目 AGENTS.md 后下一轮：新 ctx 带替换声明行、旧消息保留历史；仅相似标题/标记开头但结构不完整的普通用户文本不被认作上下文（其后真实变化的 ctx 仍正确注入）；保存恢复含 B 的历史后再次输入→不再注入（基线来自重放历史）；移除全部规则→新消息为环境块＋替换声明 | ✅ t06（替换声明）/t07（两类仿冒不认作基线、其后裸注入；真实旧上下文→带声明；声明等正文→不注入）/t08（恢复基线） |
| c5 | resume 基线（007 兼容） | 含 ctx 消息的历史经序列化/恢复（serde 往返＝007 行格式）后首轮：不重复注入；会话文件中 ctx 为普通 Text 项，无格式变更 | ✅ t08（恰 type/role/content 三字段）；e2e session_cli e1/e7/e8 全链恢复不重注入 |
| c6 | 预算截断 | 项目链合计 >32 KiB：截断发生在 `content`、按 UTF-8 字符边界、`</INSTRUCTIONS>` 保留、消息结构完整；总量恰不超 32768（分隔行计入）；截断有提示行；全局先占预算、后续来源不再打开（仅元数据判定提示）；多字节字符边界反例（截点落在汉字中间）不产生 U+FFFD | ✅ t09 三态（两文件/多字节边界 32766 回退/恰好占满）；运行探针：200 MiB 单来源 content 恰 32768、峰值 RSS 3628 KiB（未整读） |
| c7 | MemoryMerge 不注入 | 经新入口以 MemoryMerge 模式运行：历史/请求与直调 `run_tool_turn_with_mode` 逐字节一致（无 ctx、工具集与 system 与 013 D3 不变）；来源文件存在也不读取 | ✅ t10（请求与历史逐字节一致）；memory_phase2 43 用例不变 |
| c8 | 005 适配 3 反转 | `sent_instructions()` 新声明恰出现一次、旧声明（含 005 现行句）零残留；嵌入母本 `assets/codex/prompt.md` 字节不变；其余三项适配不变 | ✅ prompt_assembly s2（新句恰一、两句旧声明零残留）；s1 母本 SHA 锚不变；e2e n2b 实测请求含新句 |
| c9 | 记忆输入过滤兼容（013） | 含真实 ctx 消息的会话作提炼输入→首轮裸 ctx 与带替换声明 ctx 两种形态均被剔除（适配后）；既有 `</INSTRUCTIONS>` 结尾与 `<skill>` 过滤用例行为不变；环境块随整条剔除 | ✅ rollout_input 新用例（两种形态剔除＋既有两类不变＋仿冒保留）；memory_phase1 m3 扩带声明样本实测剔除 |
| c10 | 历史/工具信息兼容（006）与副作用顺序 | 带工具调用与结果的历史＋ctx 注入共存：消息投影顺序、角色、原文逐字保留；空白本轮输入仍明确报错且历史与 `on_record` 零新增（上下文写入先于用户项、后于空白校验）；UserWithMemory 模式 ctx 与记忆尾段并存（D4 不变） | ✅ t11（拒绝零写入）/t12（工具历史共存、UserWithMemory 注入且无尾段时 system 不变）；e2e n2b/n2b2/s1/s2a 全链 |

**验收方法与验证层级**（命令与工作目录：容器内 `/workspace`）：

- **L1（聚焦）**：新增 `src/app/tests/context_injection.rs`（c1–c8、c10，独立临时目录＋注入时钟/来源，遵守测试规范）＋ `memory` 侧 rollout 输入用例扩展（c9）——`cargo test --test context_injection`。
- **L2（受影响模块回归）**：`cargo test --test prompt_assembly --test llm_flow --test tool_loop --test session --test session_cli --test memory_inject --test memory_phase1`（套件名已按 `src/app/tests/` 实际核实；prompt_assembly 断言随 c8 更新，e2e 位置断言随实施更新后纳入 L3）。
- **L3（全量，AGENTS.md §7）**：`cargo check && cargo test --workspace --no-fail-fast`（记录通过/失败/跳过数）＋ `cargo test --test e2e_acceptance`（REPL 交互行为变化面：请求消息序变化，按 001 规格以伪终端验收更新）。
- **流程**：tdd（RED 先行，c1–c10 为 RED 素材）→ comprehensive-testing（变异：去重未剥声明的反例、预算边界字符反例）→ Security Testing Gate **进入**（新增自动文件读取面：固定文件名、只读、固定预算、无用户可控路径拼接；重点核查非普通文件/符号链接、读取预算、环境转义、提示行内容、记忆提炼输入兼容边界）→ code-review → 全量回归（L3）。
- **本轮基线（2026-10-20，main @ 2d7be44，分支建立前实测）**：`cargo check` exit 0（仅 tchat-tui 既有 warning，无 terminal-chat 新告警）。`cargo test --workspace --no-fail-fast`：全部套件通过，唯一例外为 e2e_acceptance 的既有 PTY 抖动——准备轮首轮 104/2、复跑 105/1（总数 106）；具名者仅 `s44`（滚轮选区，单独运行通过，与 014 规格记录一致），另一失败用例全名未留存于任何存活输出→**未确认**（不豁免、不归类）。实施轮改生产代码前复验（2026-10-20，两次连续全量）：`cargo test --test e2e_acceptance` **106/106×2 全绿**（20.9s/20.8s；证据 `/tmp/015-e2e-baseline-r1.txt`、`r2.txt`）。terminal-chat 侧各套件计数（准备轮实测）：lib 142、apply_patch 35、exec_command 16、llm_flow 51、memory_* 九套件 182、prompt_assembly 6、session 33、session_cli 24、session_perf 1、tool_loop 14——全部 0 失败。

**实测记录**（2026-10-20 实施轮，容器内 `/workspace`，分支 `feat/015-turn-context-injection` @ 基线 `2d7be44`）：

- **基线补全（改生产代码前）**：准备轮两例 e2e 失败的完整名称未留存于任何存活输出→第二例按规则记**未确认**（仅 s44 具名，与 014 记录一致的既有 PTY 抖动）；实施轮复验 e2e 两次连续全量 **106/106×2 全绿**（20.9s/20.8s，证据 `/tmp/015-e2e-baseline-r1/r2.txt`）。
- **RED**：`context_injection` 11/14 失败（t08/t10/t11 为与桩行为兼容的回归守卫）＋prompt_assembly s2＋memory_phase1 m3＋rollout_input/tui_host 新用例失败——六处 RED 实测。
- **GREEN 与 L1/L2**：`context_injection` 14/14；lib 144/144（含 2 新用例）；L2 七套件 prompt_assembly 6、llm_flow 51、tool_loop 14、session 33、session_cli 24、memory_inject 18、memory_phase1 19——全部 0 失败。
- **e2e**：106/106（更新 s1/s2a/n2b/n2b2/s15 断言＋`pty_cli` 默认 cwd 落 `/tmp`、llm_flow 两管道用例 cwd 落临时目录——均为断言随新消息序更新，未放宽）。
- **综合测试（变异）**：A 不剥声明→t06/t07/t08/t14＋rollout 过滤击杀；B 无 UTF-8 边界回退→t09 击杀；C 仅前缀识别→t07/t14＋rollout 过滤击杀；三变异恢复后全绿。
- **安全门禁**：轻量报告 `docs/security/reports/2026-10-20-015-context-injection-security.md`（200 MiB 来源有界读取探针：content 恰 32768、峰值 RSS 3628 KiB）。
- **独立审查**：双轴（Standards/Spec）并行审查；处理 5 项发现（`ContextSources::from_env()` 收拢重复、UTF-8 重建不可达分支扁平化、预算耗尽后不再打开文件仅元数据判定〔本表 c6 行同步〕、空白输入文案常量化、误建 `devnull` 清理），修复后 L1/lib 复验绿。
- **L3 最终**：`cargo check` exit 0（terminal-chat 0 告警；期间新增 1 处多余 `mut` 告警已修）；`cargo test --workspace --no-fail-fast` 共五轮：四轮全绿（捕获轮 **3035 通过 / 0 失败 / 3 忽略**，含 e2e 106），一轮出现**单例未具名失败**（该轮仅聚合统计未留存输出；前后三次全量复跑均全绿）——统一记**未确认失败**（证据不足，不作任何归因），不据此豁免也不视为交付阻断。
- **证据收口轮（2026-10-20 追加）**：终态补充独立审查覆盖首轮审查后全部生产增量（5 处：`from_env`、UTF-8 重建扁平化、`has_regular_source`、空白文案常量、`mut`）——增量无缺陷，但发现**预算耗尽分支只检查首个后续路径**（首路径缺失会掩蔽更后来源的存在性，与 §1「存在后续来源时提示」不符）→ 修复为扫描全部剩余路径的元数据存在性＋t09 补钉角形 (d)，变异 D（退回单路径）被 t09(d) 击杀后恢复。终验两轮：第一轮 workspace 全绿但 standalone e2e 出现 `s81_m7_enabled_turn_injects_and_spawns_pipeline` 播种断言失败——根因实证为**既有测试结构竞态**（管线建库先于播种、断言无等待窗；runner.rs 顺序＋单跑通过＋本任务 diff 不涉 runner），范围内修复为与建库同款的有界等待（断言不放宽）；第二轮（最终候选）：`cargo check` exit 0、`cargo test --workspace --no-fail-fast` exit 0（内含 e2e 106/106）、`cargo test --test e2e_acceptance` exit 0（106/106），合计 **3141 通过 / 0 失败 / 3 忽略**（日志 `docs/records/2026-10-20-015-final-verification.log`，含完整命令、cwd、退出码与两轮完整输出）。

---

## 附：实施轮修改范围与禁止范围（供 Supervisor 下发）

**允许修改**：`src/app/src/context.rs`（新建：来源加载＋消息构造＋去重识别）；`src/app/src/lib.rs`（注册 `context` 模块，最小改动）；`src/app/src/prompt.rs`（适配 3 反转＋替换声明常量）；`src/app/src/turn.rs`（新入口 `run_user_turn_with_context`：空白预检＋User/UserWithMemory 注入、MemoryMerge 不注入；来源参数化，时钟/目录可注入）；`src/app/src/main.rs`、`src/app/src/tui_host.rs`（改调新入口传入来源；TUI `/resume` 投影跳过 ctx 项）；`src/app/src/memory/rollout_input.rs`（过滤识别复用 015 识别器的最小适配＋用例扩展 c9）；`src/app/tests/context_injection.rs`（新建）；`src/app/tests/prompt_assembly.rs`（c8 断言更新）；`src/app/tests/e2e_acceptance.rs`（消息位置断言更新＋受影响用例 cwd 落临时目录）；`docs/specs/005-minimal-prompt-assembly.md`（适配 3 加一行指向本规格的修订注记，已完成）。

**禁止修改**：003/004 工具契约与两工具白名单；007 存储格式与恢复语义（ctx＝普通 Text 历史项，零 schema 变更）；013/014 的 D2/D3/D4 及全部已验收行为（c9 适配不得改变既有两类过滤的已验收行为）；`assets/codex/prompt.md` 嵌入母本字节；生产代码之外的任何用户文件；不新增配置项/依赖；不实现压缩、WS 增量、世界状态差量、Skills/MCP/插件发现。

## 032 关联（2026-10-28）

032 检查点后：去重基线取完整历史最后一条上下文消息（与活动视图中的最近消息恒等），同内容不重复注入、规则变化仍按本规格注入带替换声明的新消息；每用户轮读取一次的语义不变。见 [032 规格](032-context-management.md) §1.4/§1.7。
