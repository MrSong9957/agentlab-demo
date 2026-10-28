# Codex 五步初始化执行记录

- 日期：2026-10-28（UTC）
- 目标：当前 Codex 用户环境与 `/workspace` 项目。
- 状态：五步已执行；配置、加载和调研引擎验证通过，待人工复核。来源限制与可选跳过项见下文，不代表所有研究来源均可用。
- 依据：`/vault/wiki/Agent工具初始化配置.md`、其引用的双层规则模板与 `ce + last30days 技能组合.md`。已通过 `read_thread` 读取引用会话 `01a1588d-dc7c-7622-bcf6-bdb8d46b2ab8`；返回内容仅包含旧任务的截断最终答复，本次结论均重新验证。
- 环境：Linux，用户 `dev`，`CODEX_HOME=/home/dev/.codex`，`codex-cli 0.164.0`，Python 3.12.3。

## 1. 五步结果

| 步骤 | 结果 | 实际检查与处理 |
|---|---|---|
| 定位来源 | 通过 | 本地 `/vault` 可读，无需下载 Obsidian 副本。 |
| 双层规则 | 通过，保留已有配置 | `/home/dev/.codex/AGENTS.md` 与模板全局正文一致；项目 `/workspace/AGENTS.md` 已包含项目规则及两份 `rules/` 的读取要求。无生效中的 `AGENTS.override.md`。新建临时会话返回的 `instructionSources` 实际包含这两份文件。 |
| 技能 | 通过，已补齐 | 安装并启用 CE 原生插件 3.33.1（36 个技能）和用户级 last30days 3.31.0。TDD、comprehensive-testing、diagnosing-bugs、code-review 已存在且启用。已执行 CE 项目设置和 last30days 真实调研。 |
| 用户级代理 | 通过，已存在 | `/home/dev/.codex/agents/Security-Testing-Gate.toml` 的 `name`、`description`、`developer_instructions` 可解析；正文与 `/vault/agents/security-testing-gate.md` 一致。本会话可选角色目录已包含 `Security-Testing-Gate`，无需重新转换或另启动安全扫描。 |
| 项目 rules | 通过，保留项目定制 | `/vault/rules/` 的两份文件均已存在于 `/workspace/rules/`；`REVIEW.md` 一致。规格文档中的经验路径已适配为 `rules/project-lessons.md`，并去除了本项目已删除的 `docs/index.md` 约定，未用通用副本覆盖。两份规则均已实际读取。 |

## 2. 安装与项目改动

| 项目 | 路径 / 来源 |
|---|---|
| CE marketplace | `/home/dev/.codex/.tmp/marketplaces/compound-engineering-plugin`；官方仓库 `EveryInc/compound-engineering-plugin`，固定提交 `00d0a5bbf1300b2138b65ce724b325dd4dff77ba`。 |
| CE 插件缓存 | `/home/dev/.codex/plugins/cache/compound-engineering-plugin/compound-engineering/3.33.1`。原生配置中插件 `enabled=true`。 |
| last30days | `/home/dev/.codex/skills/last30days`；通过系统 skill-installer 从 `mvanhorn/last30days-skill` 的 `skills/last30days` 安装，固定提交 `e06af25a9a78487b75e8509e0e1687fec137176e`。 |
| CE 项目配置 | 新增 `.compound-engineering/config.example.yaml` 和 `.compound-engineering/config.yaml`；均逐字复制官方模板，所有选项保持注释，使用默认 `docs/` 产物目录与本机执行引擎。 |
| 项目经验 | 仅在 `rules/project-lessons.md` §9 新增一条插件验收经验，指向本记录。 |

开始前已查看未提交改动；原有 `.devcontainer/`、`AGENTS.md`、规格、日志和其他工作均未覆盖或回滚。全局规则、项目规则、`.gitignore`、两份项目 `rules/`、安全代理的前后哈希一致。未改 Rust 源码。

新技能下轮可用；若当前客户端菜单未刷新，重新打开会话。原生插件详情返回的技能全名为 `compound-engineering:ce-plan` 等，明确调用可用 `$compound-engineering:ce-plan`；独立技能可用 `$last30days`。

## 3. 真实验证与证据

所有 `.log` 均由真实进程输出生成，记录工作目录、实际命令、退出码和脱敏输出；不是手工拟造的终端结果。

| 验证 | 结论 | 原始证据 |
|---|---|---|
| 两个官方来源的安装 | 两步安装与 marketplace 注册均退出 0 | [安装日志](2026-10-28-install.log) |
| 原生插件状态 | 已安装且启用，版本 3.33.1 | [配置检查](2026-10-28-config-check.log) |
| 技能和新会话加载 | `skills/list` 无解析错误；last30days 为 `scope=user` 且启用；`plugin/read` 返回 CE 36 个启用技能；新建临时会话实际加载两层规则 | [运行时发现日志](2026-10-28-runtime-discovery.log) |
| CE 设置前后 | 设置前缺配置示例；设置后 `Project config healthy`，退出 0 | [设置前](2026-10-28-ce-health-before.log)、[设置后](2026-10-28-ce-health-after.log) |
| last30days 配置解析 | Python 版本符合要求；隔离配置中没有付费供应商密钥，浏览器读取关闭 | [来源诊断](2026-10-28-last30days-diagnose.log) |
| last30days 调研 | 39.8 秒，退出 0，取得 Reddit 1 条和 YouTube 1 个结果，并输出来源链接 | [真实调研日志](2026-10-28-last30days-research.log)、[原始报告留存副本](2026-10-28-last30days-raw.md)、[查询计划](2026-10-28-query-plan.json) |
| 文件、格式、链接与内容核对 | 24 项通过、0 项失败；数量仅指本次初始化检查 | [最终检查](2026-10-28-final-check.log) |
| 任务相关文件状态 | 已有修改与新增文件分别保留 | [文件状态](2026-10-28-final-files.log) |

加载检查只使用 app-server 元数据接口及 `thread/start(ephemeral=true)`，没有向模型发送 `turn/start`。调研所用查询、参数和命令均可在日志中查看；核验脚本原文以证据快照保存：[命令记录器](2026-10-28-agent-init-run.py.txt)、[加载核验](2026-10-28-agent-init-rpc.py.txt)、[调研隔离启动器](2026-10-28-agent-init-last30days.py.txt)、[最终核验](2026-10-28-agent-init-verify.py.txt)及其[前置哈希](2026-10-28-before-hashes.json)。

首次 app-server 检查受全局目录只读限制，转为已授权的用户级运行后启动成功。随后发现核验脚本误把独立技能与插件技能放在同一接口检查，并使用未加命名空间的 CE 名称。按实际协议改为 `plugin/read` 和完整名称后，最终严格断言全部通过；未用重复安装或修改插件绕过问题。失败输出和最终通过输出保留于同一日志。

## 4. 调研限制与可选跳过项

本次固定话题为 `Compound Engineering plugin Codex`，日期窗为 **2026-09-28 至 2026-10-28**，使用 `--quick`、外部查询计划和 `--no-browser-cookies`。配置、缓存及调研输出在独立临时目录中；既有用户研究配置和密钥未用于本次验证。原始结果和必要状态已复制到本目录，临时目录随后清理。

- Reddit 主站多次 HTTP 403；免费补充路径仍取得 1 条结果。缓存同时记录了 `state=ok` 与受阻详情，不能概括为所有 Reddit 请求正常。
- YouTube 返回 1 个视频；日志明确指出 quick 深度的字幕抓取上限为 0，因此不能据通用提示断言 yt-dlp 已过期，也没有进行无依据升级。
- GitHub 项目模式提示没有可用 token，未取得条目。隔离验证没有借用真实 `gh` 凭证；不能把缓存中的 `no-results` 解释为该仓库没有讨论。
- 本轮 quick 运行未覆盖 Hacker News、Polymarket 和 dev.to；`--diagnose` 的可用来源清单不等于逐个来源的实时通过证明。
- X/Twitter、TikTok、Instagram 等未纳入本次无付费密钥、无浏览器读取的验证，不影响基础安装。
- `doctor --postmortem --json` 退出 0，但返回 `present=false`，未读出此次缓存。本次来源判断直接依据真实终端日志与 `last30days-report-cache/v2` 中的状态，见[诊断输出](2026-10-28-last30days-postmortem.log)与[状态摘录](2026-10-28-last30days-source-status.json)；未修补上游引擎。
- CE 的 `agent-browser`、`ast-grep`、`ffmpeg`、Node/npx 属可选能力，本机健康检查为 **2/6 可用**（gh、jq）。未批量安装可选依赖。
- 未启用可选的跨模型缓存预热、自动经验沉淀和文风指令；本项目继续使用既有 `rules/project-lessons.md`、`docs/specs/` 与 `docs/logs/`。未创建本机覆盖配置或写入 `.context/compound-engineering/`，因此没有为可选临时目录修改 `.gitignore`。

本次是工具安装、注释配置与文档处理；Rust 构建、全量回归、安全攻防测试不适用于这些变更，未运行，也不沿用旧会话的测试数量作为本次结论。

## 5. 文档与官方来源核对

初始化文档在当前 Codex 环境可执行，无需更改 `/vault`。其中 Claude Code 的插件命令是示例；本次使用上游明确支持的 Codex 原生插件命令与系统 skill-installer。规则目录、TOML 必需字段和技能加载方式同时核对了 [OpenAI 官方代理说明](https://learn.chatgpt.com/docs/agent-configuration/subagents)、[双层规则说明](https://learn.chatgpt.com/docs/agent-configuration/agents-md)和[技能说明](https://learn.chatgpt.com/docs/build-skills)。

CE 采用[官方 README](https://github.com/EveryInc/compound-engineering-plugin)的 Codex 安装路线；[3.33.1 发布记录](https://github.com/EveryInc/compound-engineering-plugin/releases)对应此次固定来源。后续升级须显式更新固定 ref，并按[官方升级文档](https://github.com/EveryInc/compound-engineering-plugin/blob/main/docs/install/upgrading.md)刷新 marketplace 后安装。last30days 的目录安装方式与[官方 README](https://github.com/mvanhorn/last30days-skill)的 Agent Skills 支持一致；未因本机缺少 npm 就安装额外运行时。
