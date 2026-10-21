# .agents 安装到 Codex 全局目录

日期：2026-10-18。来源：`/workspace/.agents/`。目标：`/home/dev/.codex/`。本机版本：`codex-cli 0.163.0`。

## 安装结果

| 来源 | 安装位置 |
| --- | --- |
| `agents/Security-Testing-Gate.md` | `/home/dev/.codex/agents/Security-Testing-Gate.toml` |
| `skills/comprehensive-testing/SKILL.md` | `/home/dev/.codex/skills/comprehensive-testing/SKILL.md` |
| `skills/comprehensive-testing/reference.md` | `/home/dev/.codex/skills/comprehensive-testing/reference.md` |

- Agent 按 [Codex 自定义 Agent 格式](https://learn.chatgpt.com/docs/agent-configuration/subagents)转换为独立 TOML，保留原名称、描述和全部正文；模型和权限由父会话继承。
- 原 Agent 元数据 `color: yellow` 和 `injectAgentsMd: false` 没有对应的 Codex 支持项，未转换为生效设置，已在目标 TOML 注释中保留说明。不能据此认为 Codex 会跳过 AGENTS.md。
- Skill 两个文件按字节复制。skill-installer 的下载脚本用于 GitHub 来源，本次使用本地已有文件，无需下载。
- 安装前目标均不存在；没有覆盖已有项，没有修改全局 `config.toml` 和项目源文件。

## 验证证据

1. 使用 Python `tomllib` 解析生成的 Agent TOML，对比名称、描述和正文，全部一致。
2. 使用真实 `codex app-server --stdio --strict-config -c analytics.enabled=false`，调用 `initialize`、`config/read`、`skills/list`。检查运行于独立临时 HOME、CODEX_HOME、XDG 路径和空项目目录，环境不包含真实密钥，代理指向本地关闭端口，未发起模型请求。`skills/list` 返回 `comprehensive-testing` 的 `scope=user`、`enabled=true`，路径为临时用户的 `.codex/skills/comprehensive-testing/SKILL.md`，错误列表为空。临时用户目录结束后自动清理。
3. 安装后逐字节核对三个目标文件与准备文件，全部一致；再次解析目标 Agent TOML通过。Skill 哈希与源文件一致。

真实 Codex 的 Skill 全局发现已验证；Agent 验证限于官方格式、TOML 解析和内容一致性，没有启动安全测试 Agent 执行任务。临时路径下 helper 别名创建警告，以及 bubblewrap 的 PATH 警告，不影响上述配置读取和 Skill 发现结果。

| 已安装文件 | SHA-256 |
| --- | --- |
| `agents/Security-Testing-Gate.toml` | `5abc093e609e683807ed7738832d816e26a677eb5b9bac1a2b03e46ddbc2be58` |
| `skills/comprehensive-testing/SKILL.md` | `9ac39237aa7873a6e91b1357ce2437524ae0f023aaf451d5374e0b635f37630a` |
| `skills/comprehensive-testing/reference.md` | `a155b0eebc6f87377021a1cc04a9cbb5e32e57b4e29d5bc8743034ec9160ab37` |

结果：上述三项验证全部通过，失败 0；未修改项目功能，项目 cargo 全量回归、typecheck/lint 和 E2E 不适用，未运行，未宣称项目测试通过。

Skill 下一轮可用；Agent 在新会话中按名称 `Security-Testing-Gate` 调用。若 Skill 未出现在选择器中，重启 Codex 后检查。
