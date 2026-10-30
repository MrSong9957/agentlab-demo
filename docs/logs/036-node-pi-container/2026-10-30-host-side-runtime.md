# 宿主侧运行验收（036/037 公共证据：临时资源生命周期＋主动更新）

- 日期：2026-10-30；执行位置：Windows 宿主（Docker Desktop，引擎/CLI 29.8.2，WSL2 后端），Git Bash＋docker.exe。
- 指令来源：[2026-10-30 宿主侧验收指令](../../records/2026-10-30-host-side-acceptance-brief.md)；依据 [036 规格](../../specs/036-node-pi-container.md) §5、[037 规格](../../specs/037-cc-switch-container.md) §5 与[计划 U2/U4 生命周期场景](../../plans/2026-10-29-2202-feat-node-pi-container-plan.md)。
- 容器内可验证部分见 [2026-10-30 运行验收](2026-10-30-runtime-acceptance.md)（34 通过 / 0 失败）；本日志只覆盖其「未执行与缺口」节列出的宿主侧项目。
- 约束遵守：未对固定名容器 `agentlab-demo-dev` 做实验性生命周期操作（唯一一次重建为任务 A 正式流程）；全程只写假数据、未读写真实凭证；未执行 `git commit`／`push`；037 专属结论在 [037 日志目录](../037-cc-switch-container/2026-10-30-host-side-runtime.md)引用，不重复抄写。
- 宿主执行要点：docker 命令一律前置 `MSYS_NO_PATHCONV=1`（防止 MSYS 路径改写，2026-10-27 已验证的配方）；compose `-f` 传 `D:/` 盘符正斜杠绝对路径。

## 任务 B：临时容器／临时卷生命周期（B1–B7）

执行镜像：`agentlab-demo-dev:latest`＝`130916a5ace9`（任务 A 重建前的 01:25 构建版）。容器名／卷名统一 `acc036-*` 前缀；临时容器默认用户 dev（uid 1000），只跑 sh／CLI，未挂 seccomp profile。

| # | 用例 | 关键命令（要点） | 实测 | 退出码 | 结论 |
|---|---|---|---|---|---|
| B1 | 新空卷首次写入 | `docker volume create acc036-vol1`；`docker run --rm -v acc036-vol1:/home/dev/.pi <镜像> sh -c 'mountpoint … && touch …/fake-config'` | `/home/dev/.pi is a mountpoint`；`fake-config` 由 dev 写入成功；卷根归属 `dev:dev`、权限 0700（镜像层所有权随首次挂载复制） | 0 | 通过 |
| B2 | 停启一致 | `acc036-ctl` 挂 `acc036-vol1`→`/home/dev/.pi`、`acc036-vol-cc`→`/home/dev/.cc-switch`，`sleep infinity`；写假标记＋假设置 → `docker stop` → `docker start` | md5 停前/停后完全一致：`b2-marker`＝`101983d1…`、`settings.json`＝`f2d2f357…`，逐字节一致 | 0 | 通过 |
| B3 | 删除重建＋数据库 | `rm -f acc036-ctl` → 同镜像同卷新建 `acc036-ctl2` → 隔离 env 下真实 CLI 初始化数据库 → 写假标记 → 再次 `rm -f`＋同名重建 → 复验 | 真实 CLI 全新初始化 `cc-switch.db`（237,568 字节）＋`settings.json`；重建后 `b3-marker` md5 `a5b682d0…` 一致、`cc-switch.db` md5 `18f40f36…` 一致、`quick_check`＝ok（容器内 python3 sqlite3）；B2 的 pi 标记同样保留 | 0 | 通过 |
| B4 | 模拟旧目录迁移 | `acc036-vol-old` 预灌假旧树（`agent/bin/pi` 假启动器、`agent/config.json`、`agent/sessions/old-session.log` 共 3 文件）→ 新容器挂载复验 | 挂载点成立；3 个迁移文件 md5 逐一相等（`diff` 通过）；dev 可写（`written-by-b4` 成功）；`command -v pi`＝`/usr/local/bin/pi`；对照：直呼卷内假启动器输出 `FAKE-OLD-PI-LAUNCHER`，而真实 `pi --version`＝1.4.0——镜像入口不被卷内旧程序命中 | 0 | 通过 |
| B5 | 缺挂载对照 | 不挂任何卷的临时容器内 `mountpoint /home/dev/.pi` | `/home/dev/.pi is not a mountpoint`（目录为镜像层普通目录，空、mtime 为镜像构建时刻） | 32 | 通过（按预期失败，证明断言能发现丢失挂载） |
| B6 | 安装失败传播 | `git archive HEAD` 干净上下文（19MB）复制到临时目录，Dockerfile 中 install.sh URL 改为 `https://invalid.acc036.example.invalid`，`docker build -f <临时Dockerfile> -t acc036-broken .` | 前置层全部 CACHED；cc-switch 下载层 `curl: (6) Could not resolve host` → 退出码 6 → `&&` 链中止 → `docker build` 退出 1；`docker images acc036-broken` 无输出（不产出可用镜像）；`docker rmi acc036-broken` 报 `No such image`（无产物可清） | 1 | 通过 |
| B7 | 已有目标程序非交互 | 临时容器预置同名假文件（`echo FAKE-OLD-CC-SWITCH`），官方 `install.sh` 从 GitHub latest 实下载，仅设 `CC_SWITCH_INSTALL_DIR=/tmp/b7target CC_SWITCH_FORCE=1`；无 TTY、stdin 关闭 | 脚本打印 `Existing installation detected … continuing because CC_SWITCH_FORCE=1`（无人工确认）；安装退出 0；目标被替换为 22,202,400 字节 ELF（官方 musl 静态包）；`/tmp/b7target/cc-switch --version`＝`cc-switch 5.11.3` | 0 | 通过 |

过程记录（两处执行层修正，不影响被测对象结论）：

1. B3 简报字面命令 `env -i HOME=/home/dev cc-switch provider list` 退出 127：`env -i` 清空 PATH 后 execvp 回退默认路径（`/bin:/usr/bin`，不含 `/usr/local/bin`），属命令执行语义而非产品缺陷；按[验收脚本](../../../scripts/acceptance/036-node-pi-container.sh)自身同款模式改为显式 `PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/bin` 后执行成功。
2. B6 首次构建因临时上下文路径少写 `/ctx` 段在上下文解析期失败（未触达被测层，操作笔误）；修正后重跑即上表结果。另：BuildKit 解析 `FROM node:24-bookworm-slim` 元数据当日曾一次瞬时失败（`auth.docker.io` IPv6 连接超时），随后 `docker pull` 成功、后续构建均正常，为瞬时网络抖动；README 已写明需要时可传构建代理 `--build-arg`，本轮未需要。

清理清单（结束即核验，全部为本轮自建资源）：容器 `acc036-ctl`（B3 中删除）、`acc036-ctl2`（清理步删除）、`acc036-b4`／`acc036-b5`／`acc036-b7`（`--rm` 自清理）；卷 `acc036-vol1`、`acc036-vol-cc`、`acc036-vol-old`；镜像 `acc036-broken`（无产物，rmi 确认不存在）；临时构建上下文与 md5 比对文件已删除；按 `acc036-*` 过滤的容器／卷／镜像列表均无残留。`alpine:latest` 为宿主原有镜像（创建于 6 周前），本轮未拉取未改动。

## 任务 A：主动更新验证（docker pull＋--no-cache 构建＋up -d）

| 步骤 | 命令 | 实测 |
|---|---|---|
| 刷新基础镜像 | `docker pull node:24-bookworm-slim` | 退出 0；digest `sha256:c86ecead9e55a5210ac1727de38a4ca840bf03dcaf34dc9749addbf383b1920c`（ID `c86ecead9e55`，上游镜像构建于 29 小时前，333MB） |
| 无缓存重建 | `docker compose -f D:/AgentLab/projects/demo/.devcontainer/compose.yaml build --no-cache dev` | 退出 0；耗时 **392 秒**（apt＋rustup 层 280.1s＋50.2s、bubblewrap 层 0.1s、npm 层 26.2s「added 121 packages」、cc-switch 层 8.3s「Installed cc-switch」）；未传 build-arg，直连成功 |
| 重建容器 | `docker compose -f … up -d --no-build dev` | 退出 0；`Recreated → Started`；新容器启动时刻 2026-10-30T02:31:18Z；`docker inspect` 的 SecurityOpt 含完整 033 seccomp JSON（profile 随重建继续生效） |

镜像标识：`agentlab-demo-dev:latest` → `sha256:937185b469187a71ced26961f1ce96bc7e5909a677b8dedac2ccdb3fb7cf4ed6`（ID `937185b46918`，创建 2026-10-30 10:30:14 +0800，2.68GB）；旧镜像 `130916a5ace9` 让出 latest 标签后已不在本地镜像列表（`docker images -a` 无悬空残留）。

重建后复核：新容器内 `bash /workspace/scripts/acceptance/036-node-pi-container.sh` → **34 通过 / 0 失败**（含 4 个对照用例），退出 0；汇总行 `node v24.22.2 / npm 11.19.1 / pi 1.4.0 / cc-switch 5.11.3`；数据保留断言照常成立（`cc-switch.db` mtime 2026-10-29T23:23Z、`.pi` 树最早文件 2026-08-26 均早于本次容器启动）。

上游对齐（构建窗口内查询）：

| 组件 | 安装版本 | 上游当时 latest | 查询途径 |
|---|---|---|---|
| Node | v24.22.2 | v24.22.2 | `nodejs.org/dist/latest-v24.x/` |
| npm | 11.19.1 | Node 官方镜像自带（npm 另提示自身可升 12.2.1，不属 R2「随 Node LTS 最新补丁」契约范围） | 随 Node 镜像 |
| Pi | 1.4.0 | 1.4.0 | 容器内 `npm view @earendil-works/pi-coding-agent dist-tags.latest` |
| CC-Switch | 5.11.3 | v5.11.3（2026-10-22 发布） | `api.github.com …/releases/latest` |

判定（简报口径）：安装版本与本次构建解析到的上游稳定版一致 → 通过。上游在 01:25 首次构建与本轮刷新之间未发新版，故安装版本与前次相同属预期行为。

信息项：`.pi` 树现 17,357 个文件，与上轮日志的 17,358 相差 1，恰为上轮验收后按用户指令删除的卷内旧启动器 `agent/bin/pi`，数目自洽。

## 统计与结论建议

- 统计：宿主侧用例 **11 通过 / 0 失败 / 0 未执行**（任务 B 七用例＋任务 A 四步：pull、构建、重建、复验；其中 B5 为按预期失败的对照用例，证明断言有效）；容器内复验 34 通过 / 0 失败。无新增失败，无环境阻塞项。
- 执行方只记录证据与结论建议，不宣称验收通过：宿主侧全部实测与规格预测一致，两份规格 §5 的宿主侧「未执行」注记已更新为实测结果，最终状态由验收方判定。
