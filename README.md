# agentlab-demo

agentlab 容器开发方案的验收项目。

## 启动

```powershell
docker compose -f .devcontainer/compose.yaml config --quiet
docker compose -f .devcontainer/compose.yaml up -d --build
```

## 容器内测试

```powershell
docker compose -f .devcontainer/compose.yaml exec -u dev -w /workspace dev cargo test
```

在容器内的仓库根目录 `/workspace`，完整回归使用同一条命令：

```bash
cargo check --locked --offline && cargo test --workspace --locked --offline --no-fail-fast
```

这条命令先做类型检查，再运行整个 workspace 的单元、集成和真实 CLI／PTY 测试，包含后端七项关键功能的正常、边界与异常回归。测试使用临时数据、假 Key 和本地服务，无需真实供应商密钥；`--locked --offline` 使用已缓存的锁定依赖，不联网下载。首次使用新环境时须先准备 `Cargo.lock` 对应依赖。

查看结果：

- **通过**：整条命令退出码为 `0`，所有套件均为 `test result: ok`，`failed` 均为 `0`。
- **失败**：退出码非 `0`；查看编译错误或 `FAILED` 用例名称及其断言差异。`--no-fail-fast` 会在某个测试目标失败后继续运行其他目标，不自动重试掩盖失败。
- **忽略**：`ignored` 单独统计，不能算通过；类型检查或编译失败时，尚未运行的测试也不能算通过。

每次新增或修改功能后，都重新运行这条完整回归命令，并向用户报告通过、失败、忽略及未运行情况；单个名称过滤只用于排查，不能代替交付验证。用例与行为对应关系见 [后端回归覆盖清单](docs/testing/backend-regression.md)。

## Rust CLI（terminal-chat）

```powershell
# 交互式进入：compose 已配置 TTY 与 TERM=xterm-256color，无需手动设置；
# 需与本机终端严格一致时加 -e TERM="$TERM"（PowerShell 用 $env:TERM）
docker compose -f .devcontainer/compose.yaml exec -u dev -w /workspace dev bash
# 容器内：
cargo run
```

管道输入（脚本/CI）用 `-T` 关闭 TTY，程序自动降级为非交互模式：

```powershell
echo "hello" | docker compose -f .devcontainer/compose.yaml exec -T -u dev -w /workspace dev cargo run --quiet
```

> 注意： rustyline 需要 TERM 正常（非 `dumb`/空），否则方向键、Ctrl+C 等交互退化为字面文本。

## 目录结构（009 目录整理）

- `src/app/`＝主程序 crate `terminal-chat`（源码/测试/资产）；`src/tui`、`src/shims`、`src/vendor`＝008 TUI 渲染层移植；根 `Cargo.toml` 为 workspace 入口（默认成员 `src/app`，从仓库根 `cargo run` 不变）。
- `docs/specs/`＝规格；`docs/records/`＝过程证据（原 `logs/`）；`config/`、`db/`、`.env`＝运行配置与数据。详见 `docs/specs/009-project-layout.md`。

## 约定

| 项目     | 值                          |
| -------- | --------------------------- |
| 容器名   | `agentlab-demo-dev`         |
| 用户     | `dev`                       |
| 工作目录 | `/workspace`（本项目源码）  |
| 知识库   | `/vault`（共享 Obsidian）   |
| 缓存     | `/cache`（可清理）          |
| Agent 配置 | `/home/dev/.zcode` `.claude` `.codex`（持久卷） |
