# OpenCode 全局规则、技能与 Agent 初始化

日期：2026-10-21。执行环境：Docker 容器 `/workspace`，OpenCode v2.1.2（后台服务 `127.0.0.1:49374`）。来源：本地 Obsidian 仓库 `/vault`。

## 安装结果

| 项目 | 来源 | 目标 |
| --- | --- | --- |
| 用户全局规则 | `/vault/wiki/AGENTS-md-base.md` 全局正文（与 `~/.codex/AGENTS.md` 相同） | `~/.config/opencode/AGENTS.md` |
| 项目规则 | `/workspace/AGENTS.md`（已存在，未改动） | `/workspace/AGENTS.md` |
| 9 个技能 | `https://github.com/mattpocock/skills` | `~/.config/opencode/skills/<skill>/` |
| 安全测试 Agent | `/vault/agents/security-testing-gate.md` | `~/.config/opencode/agents/security-testing-gate.md` |

- OpenCode V2 原生读取全局 `~/.config/opencode/AGENTS.md` 和项目 `AGENTS.md`；不读取 `CLAUDE.md` 作为回退，因此不需要全局 `CLAUDE.md`。
- `/workspace/AGENTS.md` 在本次执行前已按双层模板配置完成，含项目技术栈、入口、契约和验证命令；本次未修改。
- 技能按 v2 清单只安装 9 个：`grilling`、`grill-me`、`handoff`、`grill-with-docs`、`domain-modeling`、`to-spec`、`tdd`、`code-review`、`diagnosing-bugs`。使用 Git 克隆后目录复制，未安装清单外的技能。
- Agent 从 Claude 格式转换为 OpenCode Markdown Agent：保留 `description` 和全部正文，增加 `mode: subagent`。源元数据 `color: yellow` 不是 OpenCode 要求的六位十六进制颜色，`injectAgentsMd: false` 在 OpenCode 无对应项，均未转换；不能据此认为 OpenCode 会跳过 AGENTS.md。
- OpenCode 还会通过兼容路径扫描 `~/.claude/skills`；本次 9 个技能落在官方全局目录 `~/.config/opencode/skills`，不依赖兼容扫描。

## 验证证据

1. **新会话双层规则**：执行 `opencode run` 创建独立会话，要求只回答全局规则第 1 条准则标题和项目规则第 7 节技术栈。实际返回 `编码前思考` 与 `Rust CLI terminal-chat（edition 2021，rustup stable rustc/cargo 1.99.0，Cargo，Cargo.lock 锁版本）；纯 Rust 项目。`，证明全局与项目两层规则均进入新会话上下文。验证会话：`ses_ea1988747ffeX5cigFkme56wNW`，标题「验证：OpenCode 双层规则加载」。
2. **技能注册**：`GET /api/skill` 返回 9 个技能，路径均为 `~/.config/opencode/skills/<id>/SKILL.md`，其中 `grilling` 存在；另有 OpenCode 内置技能和兼容路径下的 `comprehensive-testing`，不影响本次清单。
3. **技能内容一致**：9 个目录与安装源逐目录 `diff -r` 全部一致，且每个目录均含 `SKILL.md`。
4. **技能调用**：新会话以 `@grill-me` 显式加载并附带清单测试文本，实际回复 `安装成功`；该结果仅作为调用链佐证，技能就绪仍以第 2、3 条的注册和文件一致性为准。验证会话：`ses_ea197b2ddffek372Yzo1UnIXsW`，标题「验证：grill-me 技能调用」。
5. **Agent 注册**：`opencode debug agents` 与 `GET /api/agent` 均返回 `security-testing-gate`，模式 `subagent`，描述和 system 正文与源文件一致。
6. **全局规则一致**：`~/.config/opencode/AGENTS.md` 与 `~/.codex/AGENTS.md` 的 SHA-256 均为 `98fd5ad2e6ffab608095b16ef98866af0699bc0473a1ebe80ba284dc5933ef39`。

## 文件校验和

| 文件 | SHA-256 |
| --- | --- |
| `~/.config/opencode/AGENTS.md` | `98fd5ad2e6ffab608095b16ef98866af0699bc0473a1ebe80ba284dc5933ef39` |
| `~/.config/opencode/agents/security-testing-gate.md` | `d6fb9e07e4a4495ad42e4e35a0455ef6fd93e1695455846ee24fd1562868969e` |
| `skills/grilling/SKILL.md` | `befdff1e27a1dfe2a294d5871169ac1afe16d983911daa4f32627514a48c1e15` |
| `skills/grill-me/SKILL.md` | `caaf8b8de1684f96e26b28f3c29189db5c89cce4b73e1c93d86164f66ef88637` |
| `skills/handoff/SKILL.md` | `4cbb043950dad2ffd3662ff1a5a0b51ae676004b5cae10e3787e8ee7a4ee69d3` |
| `skills/grill-with-docs/SKILL.md` | `7de372c13488f1ee96cc11cd8907b56b6809cc93eef776eeddd37de6b6cbe3fe` |
| `skills/domain-modeling/SKILL.md` | `7b925d7b1e341a2eeae33ad68a8a8c0ab889a38ddd22a7598e4c240e5a7556a3` |
| `skills/to-spec/SKILL.md` | `43ad9cf318e5e7d3d1fa360253a37021796dc87a0c2e595ad262661a10f85088` |
| `skills/tdd/SKILL.md` | `5505d25bbaa8fc14a79e4d20163936958992a2995b7f1dd1d6197edc9ed722e0` |
| `skills/code-review/SKILL.md` | `09eb147f793e4647949edd32dbad278f0554dfb56e0d46b699cd4c2c55bf0357` |
| `skills/diagnosing-bugs/SKILL.md` | `849198df88cdb65fd0d97236604c1fca7ff2f22d2f705f9724a800d0266b118d` |

## 约束与说明

- `grill-me`、`grill-with-docs`、`to-spec`、`handoff` 的 `SKILL.md` 带 `disable-model-invocation: true`，OpenCode 不会自动向模型推荐它们，但技能仍已注册，可用技能 ID 或 `@<id>` 显式加载；这是上游技能设计，不是安装失败。
- 本次只修改用户级 OpenCode 配置和 `docs/records/`，未修改项目源码、`Cargo.toml`、`AGENTS.md` 或既有未提交改动。配置变更不涉及编译与测试，`cargo check` / `cargo test` 不适用，未运行，也不宣称项目测试通过。
- 结果：规则两层加载、9 个技能注册与内容一致、Agent 注册三项验证全部通过，失败 0。
