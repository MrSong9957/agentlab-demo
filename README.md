# agentlab-demo

agentlab 容器开发方案的验收项目。

## 启动

首次克隆后先准备持久数据目录（`.container-state/` 不入 Git，挂载配置也禁止 Compose 自动创建缺失的源目录），再启动容器：

```powershell
New-Item -ItemType Directory -Force .container-state/pi, .container-state/cc-switch | Out-Null
docker compose -f .devcontainer/compose.yaml config --quiet
docker compose -f .devcontainer/compose.yaml up -d --build
```

## Node.js、Pi 与 CC-Switch CLI

Dockerfile 使用官方 Node.js 24 LTS 镜像提供 Node/npm，构建时安装 `@earendil-works/pi-coding-agent@latest`；CC-Switch 使用官方 Release 的 latest 安装脚本。程序安装到镜像系统目录，最终仍以 `dev` 运行。

Pi 数据保存在宿主项目的 `.container-state/pi/`，CC-Switch 数据保存在 `.container-state/cc-switch/`，分别挂载到 `/home/dev/.pi` 和 `/home/dev/.cc-switch`。该目录已被 Git 和 Docker 构建上下文排除，可能含密钥、数据库和会话，不要提交或分享其内容。Windows 宿主上的访问权限由目录的 NTFS ACL 管理。

首次克隆项目时的目录准备命令见「启动」一节，须在启动容器之前执行。已有数据必须先复制到这两个目录并核对，再启用新挂载；直接挂载空目录不会自动迁移旧容器的数据。若 CC-Switch 已有数据库，先关闭相关写入进程，再完整复制其数据目录。旧目录应保留备份。当前容器也可用指向 `/workspace/.container-state/` 对应子目录的符号链接继续读写，无需为数据持久化重建容器。

“latest”在重新执行安装步骤时获取；普通启动或命中构建缓存不会自动升级。以后需要主动更新时，在宿主执行：

```powershell
docker pull node:24-bookworm-slim
docker compose -f .devcontainer/compose.yaml build --no-cache dev
docker compose -f .devcontainer/compose.yaml up -d --no-build dev
```

保留本地 `agentlab-base:latest`，不要给整个构建追加全局 `--pull`。如构建需要代理，在 `build` 命令中传入 `--build-arg HTTP_PROXY=...` 和 `--build-arg HTTPS_PROXY=...`；Compose 的运行时代理不自动成为构建代理。拉取 Node 镜像所需代理由 Docker Desktop 配置。

构建后可在容器内检查 `node --version`、`npm --version`、`pi --version` 和 `cc-switch --version`。修改配置文件本身不会升级当前容器里的程序。

## API Key 配置

在容器内的仓库根目录执行以下命令；已有 `.env` 时保留其内容：

```bash
if [ ! -e .env ]; then
  (umask 077; cp .env.example .env)
fi
chmod 600 .env
```

编辑本地 `.env`，按需填写对应厂商的真实密钥；默认 DeepSeek 使用 `DEEPSEEK_API_KEY`。`.env.example` 按国内、国外及聚合平台分组，列有各厂商的凭证变量与官方文档，其余值保持为空即可。程序启动时从当前工作目录加载 `.env`，跳过空值，已设置的环境变量优先。`.env` 已被 Git 忽略，真实 Key 只保存在本地；可提交的 `.env.example` 保持空值。

项目内置 DeepSeek 和 OpenAI。其他厂商需在 `config/manual.json` 中配置地址、模型及凭证引用，例如 `"key_ref": "env:DASHSCOPE_API_KEY"`。当前仅支持 `openai-chat` 协议，接入时须核对厂商提供的 Chat Completions 兼容入口。

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
| Pi / CC-Switch 数据 | `/home/dev/.pi`、`/home/dev/.cc-switch`（宿主项目 `.container-state/` 绑定挂载） |
