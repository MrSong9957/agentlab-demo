# 宿主侧运行验收指令（036/037 收尾，2026-10-30 下达）

> 执行者：在 Windows 宿主（有 docker CLI 与引擎）工作的 Agent；工作目录 `D:\AgentLab\projects\demo`。
> 依据：[036 规格](../specs/036-node-pi-container.md) §5、[037 规格](../specs/037-cc-switch-container.md) §5、[计划 U2/U4 生命周期场景](../plans/2026-10-29-2202-feat-node-pi-container-plan.md)。
> 容器内可验证部分已于 2026-10-30 通过（34 通过 / 0 失败，见 [036 运行验收日志](../logs/036-node-pi-container/2026-10-30-runtime-acceptance.md)）；本指令只覆盖其「未执行与缺口」节列出的宿主侧项目。

## 前提与硬约束

- 本地已有 `agentlab-base:latest`；compose 的 seccomp 使用宿主绝对路径 `D:/AgentLab/projects/demo/.devcontainer/033-seccomp.json`，本就在该宿主生效，无需改动。
- **不得对固定名容器 `agentlab-demo-dev` 做 stop／rm／重建以外的生命周期实验**（计划 U3 禁止；任务 A 的正式重建除外）。
- 全程只写假数据，不读写真实凭证；临时资源只清理本轮自己创建的。
- 不执行 `git commit`／`push`；证据写入 `docs/logs/036-node-pi-container/2026-10-30-host-side-runtime.md`（037 专属结论在该目录下引用，不重复抄写）。

## 任务 A：主动更新验证（docker pull ＋ --no-cache 构建）

> ⚠️ `up -d` 会**重建并替换 `agentlab-demo-dev` 容器**，容器内运行中的会话（含 ZCode）将中断。`/workspace` 与 `.container-state/` 都是宿主绑定，数据不受影响。请在用户方便中断会话时执行，且放在任务 B 之后。

```powershell
cd D:\AgentLab\projects\demo
docker pull node:24-bookworm-slim
docker images --digests node:24-bookworm-slim          # 记录 digest
docker compose -f .devcontainer/compose.yaml build --no-cache dev   # 记录耗时；如需代理按 README 传 --build-arg
docker compose -f .devcontainer/compose.yaml up -d --no-build dev
docker images --digests agentlab-demo-dev              # 记录镜像 ID／digest／创建时间
```

重建后复核（新容器内）：

```bash
docker exec agentlab-demo-dev bash /workspace/scripts/acceptance/036-node-pi-container.sh
```

记录项：Node／npm／Pi／CC-Switch 实际版本（脚本汇总行有）、上游当时 latest（容器内 `npm view @earendil-works/pi-coding-agent dist-tags.latest`、`curl -fsSL https://api.github.com/repos/SaladDay/cc-switch-cli/releases/latest`、nodejs.org/dist 的 v24 最新）、镜像 digest 与构建耗时。**判定口径**：安装版本与本次构建解析到的上游稳定版一致即通过；若上游在两次构建之间发了新版，安装版随之更新是预期行为，不是失败。

## 任务 B：临时容器／临时卷生命周期验收

临时容器直接 `docker run` 即可（只跑 sh／CLI，不需要 seccomp profile）；镜像末段 `USER dev`，默认即 dev。镜像统一用 `agentlab-demo-dev:latest`，容器名／卷名统一 `acc036-*` 前缀。

| # | 用例（对应场景） | 步骤要点 | 通过依据 |
|---|---|---|---|
| B1 | 新空卷首次写入（U2 场景 1） | `docker volume create acc036-vol1`；`docker run --rm -v acc036-vol1:/home/dev/.pi agentlab-demo-dev:latest sh -c 'mountpoint /home/dev/.pi && touch /home/dev/.pi/fake-config'` | 挂载点成立、dev 写入成功 |
| B2 | 停启一致（U2 场景 2 前半） | 临时容器 `acc036-ctl` 挂 `acc036-vol1` 到 `/home/dev/.pi` 与另一临时卷到 `/home/dev/.cc-switch`，`sleep infinity`；写假标记与假设置 → `docker stop` → `docker start` | 标记内容逐字节一致 |
| B3 | 删除重建（U2 场景 2 后半、U4 场景 3） | `docker rm -f acc036-ctl` 后用同一镜像同一卷新建 `acc036-ctl2`；`docker exec acc036-ctl2 env -i HOME=/home/dev cc-switch provider list` 初始化数据库，写假标记，再次 `docker rm -f`＋重建复验 | 假标记保留、`cc-switch.db` 可打开（容器内无 sqlite3 CLI，用 `python3 -c "import sqlite3;print(sqlite3.connect('/home/dev/.cc-switch/cc-switch.db').execute('PRAGMA quick_check').fetchone()[0])"` 得 `ok`） |
| B4 | 模拟旧目录迁移（U2 场景 3） | 临时卷先灌入假「旧目录」树（含一个假 `bin/pi` 启动器脚本）再挂载；校验逐文件一致、dev 可写、`command -v pi` 仍为 `/usr/local/bin/pi` | 内容一致；镜像入口不被卷内旧程序命中 |
| B5 | 缺挂载对照（U2 场景 3 对照） | 不挂任何卷的临时容器内执行 `mountpoint /home/dev/.pi` | 必须失败（证明断言能发现丢失挂载） |
| B6 | 安装失败传播（037 场景 2） | 临时目录复制仓库为构建上下文，把 Dockerfile 中 install.sh 的 URL 改为不可达地址，`docker build -f <临时Dockerfile> -t acc036-broken .` | 构建非零退出、不产出可用镜像；随后 `docker rmi acc036-broken` |
| B7 | 已有目标程序非交互（037 场景 3） | 临时目录预置同名假文件，执行官方 `install.sh` 并仅设 `CC_SWITCH_INSTALL_DIR=<临时目录> CC_SWITCH_FORCE=1` | 无人工确认、退出码 0、目标被替换 |

结束清理：`docker rm -f acc036-ctl acc036-ctl2`、`docker volume rm acc036-vol1 …`、`docker rmi acc036-broken`、删除临时构建上下文；在证据中列出清理清单。

## 证据与判定

- 证据文件记录：每条的实际命令、工作目录、退出码、关键输出、通过／失败／未执行数量、清理清单；格式参照 [036 运行验收日志](../logs/036-node-pi-container/2026-10-30-runtime-acceptance.md)。
- 执行方只记录证据与结论建议，不宣称验收通过；依据证据把两份规格的宿主侧「未执行」注记更新为实测结果，最终状态由验收方判定。
- 任务 A 完成后，如容器内验收脚本结果有变（如上游发版导致版本变化），把新结果追加到证据文件并同步规格 §5 实测列。
