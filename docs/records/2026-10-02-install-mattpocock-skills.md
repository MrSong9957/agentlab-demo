# 安装 mattpocock/skills 技能（2026-10-02）

## 设计依据

- 环境内无 node/npx（`command -v node nodejs deno bun` 均无，无 nvm），主方案 `npx skills@latest add` 不可执行，改走任务清单指定的降级方案：浅克隆 `mattpocock/skills` 后手动复制。
- 落盘位置选 ZCode 用户级技能目录 `~/.zcode/skills/`（优先全局；依据 ZCode 配置指南：用户级目录为 `~/.zcode/skills/`，discovery 顺序中先于 `~/.agents/skills/` 与插件）。
- 仓库技能按分类存放：`skills/productivity/{grilling,grill-me,handoff}`、`skills/engineering/{grill-with-docs,domain-modeling,to-spec,tdd,code-review,diagnosing-bugs}`，复制时拍平为 `~/.zcode/skills/<name>/`。

## 结果

首轮 8/8 复制成功，`diff -rq` 与源逐字节一致，frontmatter `name` 与目录名全部匹配，临时克隆目录已清理。

二轮补装（同日，用户确认后执行）：

- 补装 `domain-modeling`：`skills/engineering/domain-modeling/` → `~/.zcode/skills/domain-modeling/`，`diff -rq` 一致，自包含（无跨技能引用），自带 `ADR-FORMAT.md`、`GLOSSARY-FORMAT.md`。
- `codebase-design` 有意不装（见下）。

最终清单 **8 + 1 = 9 个技能**：grilling、grill-me、grill-with-docs、domain-modeling、to-spec、tdd、code-review、diagnosing-bugs、handoff。

## 依赖缺口处置

- 缺口 1（`domain-modeling`）→ **已补齐**。grill-with-docs 引用的两个技能（grilling、domain-modeling）均已就位，"边聊边沉淀文档"（术语表进 CONTEXT.md、决策落 ADR）的产出链路完整。
- 缺口 2（`codebase-design`）→ **有意不装**。tdd 主流程（红→绿→重构）不依赖它，影响仅限接口设计术语查阅。触发补装条件：开始做接口/模块设计、需要 depth/seam/adapter 等术语参考时，从仓库 `skills/engineering/codebase-design/` 复制即可。

## 待验证

- `/grill-me` 功能验证需新会话（当前会话技能列表在启动时已固定，新装技能本会话不可见）。

## 2026-10-18：Claude Code 侧安装（9 个，方式二 → 约束 5 降级）

### 过程与依据

- 检查：Claude Code 侧 9 个均不存在（无 `~/.claude/skills/`、无 mattpocock 插件；`~/.zcode/skills/` 是上方 v1/v2 的 ZCode 安装，与本次无关）。
- 方式二执行成功：`claude plugin install mattpocock-skills@claude-plugins-official --scope user`（v1.3.1，含 27 个技能，pin `49dd158d`）。
- 「仅启用 9 个」受阻：settings 的 `skillOverrides` 对**插件技能不生效**——裸名键与 `mattpocock-skills:<name>` 限定名键均实测无效，`/prototype` 直调仍真实执行（还被其发起了写文件动作，未落盘）；CLI 二进制内嵌文档明确：`skillOverrides` 只作用独立技能（项目级/`~/.claude/skills/`），插件技能只能 `enabledPlugins` 整插件开关。
- 按清单约束 5 降级：卸载插件 → 克隆 `mattpocock/skills` 并 checkout 插件所 pin 同一 commit `49dd158d`（与插件内容同源）→ 复制 9 个技能目录到 `~/.claude/skills/`（用户级全局，自动以 `<name>@skills-dir` 装载）。
- 清理：settings.json 移除试验性 `skillOverrides`/`enabledPlugins` 条目（备份 `settings.json.bak-skills-20261018-132147`）；临时克隆已删。

### 结果（9/9）

- `diff -rq` 与源逐字节一致；frontmatter `name` 与目录名全部匹配。
- grill-with-docs 的依赖（grilling、domain-modeling）均在装；codebase-design 仍有意不装（约束 6）。

### 验证（均为新起 `claude -p` 会话）

- `/grill-me` 测试指令 → 回复「安装成功」并确认 grilling 底座加载（清单验收步骤通过）。
- `/grill-with-docs`、`/to-spec`、`/handoff` 直调 → 各自真实响应。
- code-review、diagnosing-bugs、domain-modeling、grilling、tdd 出现在新会话技能枚举。
- `/prototype` → 「不可用…没有找到它的定义」；其余 18 个随插件卸载一并消失。
- 教训：模型自报的技能清单不可靠（grill-me 实测可用却未出现在枚举中），验证以直调响应/CLI 固定消息为准。

### 落盘路径

- 技能：`~/.claude/skills/{grilling, grill-me, grill-with-docs, domain-modeling, to-spec, tdd, code-review, diagnosing-bugs, handoff}/`
- 配置：`~/.claude/settings.json`（无残留开关；statusLine 等其余配置未动）

### 注意

- 已开的交互会话需新会话才可见这 9 个技能。
- 升级方式：重克隆仓库 → 覆盖复制 → `diff -rq` 校验；若日后 CLI 支持插件内技能级开关，可回到方式二。

## 2026-10-20：Codex 用户级初始化（v2，9 个）

### 依据与范围

- 执行 `/vault/wiki/Agent工具初始化配置.md` 及其引用的双层规则模板、v2 技能安装清单。
- 当前工具为 **Codex CLI 0.163.0**；`HOME=/home/dev`，`CODEX_HOME=/home/dev/.codex`。实际二进制位于该目录下的 standalone release，程序安装目录不是规则或技能入口。
- 用户级技能目录确认依据：安装前真实 `skills/list` 返回既有技能 `/home/dev/.codex/skills/comprehensive-testing/SKILL.md`，`scope=user`。因此设置 `AGENT_SKILLS_DIR=/home/dev/.codex/skills`，不在项目内安装。
- 使用 OpenAI Docs 核对 [规则加载顺序](https://learn.chatgpt.com/docs/agent-configuration/agents-md) 和 [技能加载说明](https://learn.chatgpt.com/docs/build-skills)。当前官方技能文档列出 `~/.agents/skills`；本机安装位置以本版本运行时返回的用户级目录及 skill-installer 说明为依据，没有再建立另一份安装。
- Git 浅克隆 `https://github.com/mattpocock/skills.git` 至独立临时目录，仅复制指定 9 个目录。来源固定为 [`5beb35b040e3de97e54fa9e386d7e42d15c295c2`](https://github.com/mattpocock/skills/tree/5beb35b040e3de97e54fa9e386d7e42d15c295c2)。未使用 npx 或插件安装。
- 沿用 **v2 新增 domain-modeling、共 9 个技能**的清单；本轮开始时 9 个技能均不存在，未覆盖既有技能。安装记录继续追加到本文件，依项目 009 迁移约定使用 `docs/records/`，未重新创建清单旧路径 `logs/`。

### 规则配置

- 新建 `/home/dev/.codex/AGENTS.md`，正文逐字采用 `/vault/wiki/AGENTS-md-base.md` 的全局规则代码块，包含 Karpathy 四条准则。
- 全局正文 SHA-256：`98fd5ad2e6ffab608095b16ef98866af0699bc0473a1ebe80ba284dc5933ef39`；未发现全局 `AGENTS.override.md`。
- `/workspace/AGENTS.md` 已有完整项目规范，沿用已有文件，本次没有写入该文件。执行期间外部更新了其 Obsidian 参考路径（补充容器 `/vault`），保留该更新，没有以安装前哈希恢复旧文件。
- 真实新会话 `thread/start` 返回的 `instructionSources` 顺序为：`/home/dev/.codex/AGENTS.md`、`/workspace/AGENTS.md`，两层加载通过。当前会话也已收到两层合并后的指令更新。

### 实际落盘路径

| 技能 | 用户级目录 | 与 Git 源一致的文件数 |
|---|---|---:|
| grilling | `/home/dev/.codex/skills/grilling/` | 2 |
| grill-me | `/home/dev/.codex/skills/grill-me/` | 2 |
| handoff | `/home/dev/.codex/skills/handoff/` | 2 |
| grill-with-docs | `/home/dev/.codex/skills/grill-with-docs/` | 2 |
| domain-modeling | `/home/dev/.codex/skills/domain-modeling/` | 4 |
| to-spec | `/home/dev/.codex/skills/to-spec/` | 2 |
| tdd | `/home/dev/.codex/skills/tdd/` | 4 |
| code-review | `/home/dev/.codex/skills/code-review/` | 2 |
| diagnosing-bugs | `/home/dev/.codex/skills/diagnosing-bugs/` | 3 |

共 **9 个目录、23 个文件**，全部包含 `SKILL.md`；frontmatter 名称与目录名一致。逐文件 SHA-256 与固定提交的克隆源一致，保留原版 `agents/openai.yaml` 和引用资料。`grilling`、`domain-modeling` 两项清单依赖均齐全。

### 安装后真实验证

- 重启独立 Codex app-server，以 `skills/list` 的 `forceReload=true` 扫描 `/workspace` 和独立临时目录：两处均识别 **9/9**，路径均指向上述用户级目录，`scope=user`、`enabled=true`，无技能解析错误。
- 相同的路径、作用域、启用状态检查作用于安装前枚举时仅得到 **0/9**，安装后为 **9/9**，验证能够区分缺失与安装完成。
- 在新的临时会话中，使用原生 `turn/start.input` 的 `type=skill` 显式调用 `grill-me`，附带清单原文：`这是一条测试指令，回复"安装成功"即可。`
- 首次仅选择 `grill-me`：模型尝试加载其底座，但当前环境的命令沙箱报 `bwrap: No permissions to create a new namespace`，且当前 Codex 无 Claude 的 `Skill` 工具。会话如实返回无法完成调用，**此项未通过**。
- 保持只读沙箱和原版技能文件，再通过 Codex 原生技能输入同时选择 `grill-me` 与 `grilling`：真实会话完成，精确回复 **「安装成功」**。此路径通过；没有关闭沙箱或修改宿主内核设置。当前环境单独调用、依靠 shell 按需加载底座的路径仍受沙箱故障限制。
- 功能冒烟使用真实新会话及真实模型响应，不作为离线项目回归测试。没有以成功回复替代 9 个技能的磁盘一致性和运行时枚举验证。

### 文档与当前上游的差异

- 安装清单称 `grill-with-docs` 将术语写入 `CONTEXT.md`；本次固定提交的 `domain-modeling/SKILL.md` 实际规定写入 `GLOSSARY.md`，多上下文时使用 `GLOSSARY-MAP.md`。
- 清单称 `to-spec`「只综合，不提问」；本次上游实际要求确认测试接口，并要求预先提供 issue tracker 与分流标签。`code-review` 也要求 issue tracker 和比较基点。未执行这些技能的业务流程，不能以本次安装验证宣称它们已完成项目定制。
- 本次按约束仅安装指定 9 个技能，没有因这些前提自动补装 `setup-matt-pocock-skills`、`codebase-design` 或其他技能，也没有修改知识库原文。

### 项目全量验证与收尾

- 在 `/workspace` 执行一次 `cargo check && cargo test --workspace --no-fail-fast`，设置 `CARGO_NET_OFFLINE=true`。本次未修改 Rust 代码；这次结果反映包含原有未提交改动的工作树状态。
- `cargo check` **通过**，保留原有编译警告；未执行 clippy/rustfmt，也未为初始化额外安装 Rust 组件。未改动应用接口、Schema 或持久化格式，兼容性专项检查不适用。
- 全量测试执行 **50 套件**：**3034 通过、1 失败、3 忽略、0 过滤、0 套件未运行**，命令退出码 **101**，不能报告全量通过。E2E 在其中执行，结果 **105 通过、1 失败**。
- 唯一失败：`s44_tui_wheel_keeps_selection_gutter_unchallenged`，`src/app/tests/e2e_acceptance.rs:3943`，resize 后 OSC 52 复制事件计数实际 **2**、预期 **3**。`project-lessons.md` §8 已记录同名用例的 PTY 时序抖动；本轮没有为此修改业务代码或断言，也没有通过重跑掩盖首次失败。
- 终检：9 个技能的 23 个文件及全局规则仍与安装时的固定源内容一致。项目规则的哈希变化已定位为外部补充 `/vault` 参考路径，本次保留其最新内容。
- 临时克隆、协议 Schema、验证脚本产物及会话日志统一放在本次独占的 `/tmp/codex-init-o1mr7sx6/`，关键结果收录本文后清理该目录；未清理用户或其他 Agent 的工作产物。
- 新装技能下轮即可使用；若界面未刷新，重开会话。当前环境单独调用 `grill-me` 的沙箱限制，以及上游 `to-spec` 等业务前提，以前述边界为准。

## 2026-10-20：按 wiki 初始化清单复验（Claude Code）

背景：执行 [[wiki/Agent工具初始化配置]] 两步清单的检查。第 1 步双层规则已就位（`~/.claude/CLAUDE.md` 与 `/workspace/CLAUDE.md` 均为单行 `@AGENTS.md`，本会话实际加载两层正文），无改动。

### 结果（9/9，无重装）

- `~/.claude/skills/` 下 9 个清单技能目录自 10-18 安装后未变动，`SKILL.md` 全部存在，frontmatter 有效；`claude plugin list` 无插件，无整包残留需要卸载。本次为纯检查，未重新克隆覆盖。
- 另有 `comprehensive-testing`（10-18 来自 `.agents/` 的独立安装）不在 9 清单内，未动。

### 验证

- 当前主会话技能枚举含 grilling、tdd、code-review、diagnosing-bugs、domain-modeling（含 grill 底座）。
- 新起 `claude -p '/grill-me 这是一条测试指令，回复"安装成功"即可。'` → 斜杠命令被识别并回复「安装成功」（清单验收步骤通过）。
- **枚举缺 4 个的根因确认**：grill-me、grill-with-docs、to-spec、handoff 的 frontmatter 均带 `disable-model-invocation: true`，是用户专用斜杠命令，按设计不进模型枚举——即 10-18「模型自报清单不可靠」教训的具体机制，非安装缺陷。
- 新观察：`claude -p` 子会话带 `[claude-code:unrecognized_model] glm-5.3-flash[1m]` 警告时，其模型自报「无 Skill 工具可用」；与主会话枚举矛盾，再次印证以直调响应为准，且 unrecognized 模型的子会话自报不作为技能装载证据。to-spec 直调会发布 issue、handoff 会写交接文档，有副作用，本次未重测（沿用 10-18 验证记录）。
