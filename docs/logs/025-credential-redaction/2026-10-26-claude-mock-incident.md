# 验收误连真实服务：根因、修复与防回归（2026-10-26）

## 事故（如实）

文档加载验收（见同目录 2026-10-26-agents-md-load.md）第一次运行时，嵌套 `claude -p`
未隔离配置目录：全局 `~/.claude/settings.json` 的 env 块把 `ANTHROPIC_BASE_URL` 覆盖为
`https://open.bigmodel.cn/api/anthropic`，覆盖了外部导出的 mock 地址，完成一次真实
最小调用（"回复 ok"，费用极小），违反本任务"不调用真实付费服务"边界。后续改用隔离
`CLAUDE_CONFIG_DIR` 的两次运行零真实调用。

## 根因（实测锚定）

- 加载顺序：settings env 块的优先级**高于进程环境变量**。验收方只检查了"启动参数/
  导出变量"，没有检查各层 settings 合并后的最终生效值——全局配置里的旧端点静默胜出。
- 键级核实（本轮，未读取任何 token 值）：`~/.claude/settings.json` env 键含
  `ANTHROPIC_BASE_URL`（值＝上述真实端点）与 `ANTHROPIC_AUTH_TOKEN` 等模型变量。

## 修复（限本项目验收脚本，未改外部产品）

`scripts/acceptance/claude_mock_runner.py`：

1. 隔离入口＝`CLAUDE_CONFIG_DIR`（工具支持的独立配置目录，settings 与状态全部重定向，
   不再读取真实 `~/.claude`）；子进程环境**从零构造**（仅 PATH 继承），不携带任何真实
   凭据或生产端点变量。
2. 预检（请求发出前）：按 user→project→local→managed 合并各层 settings.json 的 env
   块——BASE_URL 覆盖值 ≠ 本次批准的 mock 地址即中止；settings 内出现任何凭据键即中止。
3. 行为断言（不只看启动参数）：mock 监听器必须实际收到请求，canary 监听器必须零请求，
   输出标记必须来自 mock。
4. 兜底遏制：HTTP(S)_PROXY 指向本机关闭端口（NO_PROXY 放行 localhost），意外指向真实
   端点的请求在本机即失败，不出网。

## 防回归证据（RED/GREEN，全部假凭据＋本地监听器，零真实服务）

见同目录 2026-10-26-mock-runner-evidence.log（四段连续运行）：

- **demo-red（旧流程缺陷复现）**：假 HOME 的 settings env 指向 canary 本地监听器、
  ambient 仍指向 mock——canary 收到 1 请求、mock 0 请求、`claude_exit=0` 且输出即
  canary 的标记回复：错误端点被静默选中，其回复被当作模型输出（与真实事故同构）。
  单一变量差（settings env 有无）证明覆盖方向＝settings env > ambient env。
- **demo-green（新流程）**：隔离＋预检后 mock 1 请求、canary 0、输出＝mock 标记。
- **demo-negative（请求发出前失败）**：隔离目录被写入指向 canary 的 env 覆盖——预检
  拦截、子进程未启动、mock/canary 均 0 请求；demo 语义＝「按预期拦截」计通过（exit 0），
  run 路径同型拦截为退出码 2。
- **run（安全入口端到端）**：cwd=/workspace、请求体 56,882 字节经 --capture 落盘，
  AGENTS.md 引用行仍在注入上下文中——原文档加载验收可全程经此入口复跑。

RED/GREEN 过程未使用任何真实 Key、未访问任何真实服务；真实 `~/.claude/settings.json`
未被读取值、未修改。
