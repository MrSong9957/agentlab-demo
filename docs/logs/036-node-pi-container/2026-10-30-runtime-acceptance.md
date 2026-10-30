# 容器重建后的运行验收（036/037 公共证据）

- 日期：2026-10-30；工作目录：容器内 `/workspace`（执行用户 `dev`）。
- 部署事件：部署者于 2026-10-30 01:25 在 Windows 宿主按 README「启动」执行 `New-Item` 目录准备＋`docker compose -f .devcontainer/compose.yaml up -d --build`，完成镜像首次构建与容器重建（构建时间标记：镜像内 `/usr/local/bin/cc-switch` 等文件 mtime 01:25；本次容器启动时刻 01:26:13，由内核 btime＋PID1 starttime 推算）。旧容器可写层随之丢弃——上一轮保留在旧可写层的 `~/.pi.before-container-state-*`、`~/.cc-switch.before-container-state-*` 备份按预期消失；数据本体已在此前迁入 `.container-state/`（[迁移与逐文件哈希校验记录](2026-10-29-implementation.md)）。

## 验收入口与结果

- 入口：新增 `scripts/acceptance/036-node-pi-container.sh`（036/037 共用），容器内执行 `bash scripts/acceptance/036-node-pi-container.sh`。
- 结果：退出码 0；**34 通过 / 0 失败**，其中 4 个对照用例全部按预期失败（证明断言能发现缺程序、缺挂载、PATH 阴影与时间方向错误）。全程 `env -i`＋临时 HOME 调用 CLI，真实数据库仅做只读副本检查，未读写真实凭证。

## 分项结果（对应规格 §5 场景表）

1. **版本与 shell 可达**（036 R1–R3、037 R1–R2）：`node v24.22.2`、`npm 11.19.1`、`pi 1.4.0`、`cc-switch 5.11.3`；`pi --help`、`cc-switch --help` 退出码 0；非登录、登录、交互三种 shell 中四工具全部解析到 `/usr/local/bin`。
2. **镜像入口优先**（036 §2 旧启动器风险）：`pi` 实体为 `/usr/local/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js`，`npm` 实体在镜像 npm 全局目录，`node`、`cc-switch` 实体为 `/usr/local/bin/*`；PATH 不含数据卷路径。信息项：卷内遗留旧手动安装启动器 `~/.pi/agent/bin/pi`（1.4KB shell 脚本，重建前 10-29 21:50），不在 PATH、不生效，实际解析断言证明镜像版本获胜——规格预警的风险场景实际存在且被正确排除。（该启动器已于验收后按用户指令删除，删除后 `pi` 仍解析到镜像入口；同目录 `fd` 为 pi 工具二进制，未动。）
3. **挂载与 dev 可写**（036 R4–R5、037 R3）：`/home/dev/.pi`、`/home/dev/.cc-switch` 均为独立挂载点且 dev 可写读回。挂载目标身份用跨路径标记证明（在 `/workspace/.container-state/<name>` 写唯一标记、于 `/home/dev/<name>` 读到同一标记）：容器内 9p 视角的 `/proc/mounts` 源只显示盘符 `D:\134`、不呈现宿主子路径，无法从挂载表直接确认目标，同目录内容一致是容器内可得的最强身份证据。
4. **CC-Switch 数据库**（037 §5 场景 4）：隔离 HOME 下真实 CLI（`cc-switch provider list`）全新初始化生成 `cc-switch.db`＋`settings.json`，`quick_check` ok；写入假标记后重新打开数据库仍可读、标记保留；真实库复制到临时目录后 `quick_check` ok（不对真实库写入）。
5. **Pi 首次写入与既有数据**（036 §5 场景 3、5）：隔离 HOME 首次运行 `pi list` 即创建 `~/.pi/agent` 结构；真实数据树 17,358 个文件在位（迁移时 14,951，差额为迁移后真实使用新增的会话）。
6. **数据跨容器重建保留**（036 §5 场景 5、计划 U2 生命周期场景的容器内证据）：`cc-switch.db` mtime（10-29 23:23）与 `.pi` 树内最早文件（08-26）均早于本次容器启动时刻 → 数据来自挂载而非本容器可写层。本次重建即"删除容器后重建"（可写层已丢弃），严格于停启场景；镜像 `command: sleep infinity` 无启动钩子，停启不存在额外数据清除路径，由删除重建覆盖。

## 对照用例（断言有效性证明）

| 对照 | 预期 | 实测 |
|---|---|---|
| 缺程序：`env PATH=/usr/bin:/bin` 后找 pi | 必须发现缺失 | 按预期消失并被检测到 |
| PATH 阴影：PATH 前置假 `pi` 启动器 | 必须被定位 | 被检测器定位到假文件 |
| 缺挂载：普通临时目录过挂载点断言 | 必须判非挂载 | 按预期判非挂载点 |
| 时钟方向：新建文件 vs 容器启动时刻 | 新文件必须更晚 | 按预期更晚 |

## 上游对齐（036 R8、计划 U3 场景 1）

本次为该 Dockerfile 在宿主的首次构建（无缓存层），安装即按刷新路径解析当前稳定版，且与查询时点上游 latest 逐一相符：

| 组件 | 安装版本 | 上游查询结果 |
|---|---|---|
| Node | v24.22.2 | nodejs.org/dist 当前 v24 最新 v24.22.2 |
| Pi | 1.4.0 | npm `dist-tags.latest` = 1.4.0 |
| CC-Switch | 5.11.3 | GitHub latest Release v5.11.3（2026-10-22 发布） |

README「主动更新」已写明 `docker pull node:24-bookworm-slim`＋`build --no-cache`＋`up -d --no-build` 流程；本轮未重复执行该演示（见下节）。

## 未执行与缺口（不计为通过）

- **宿主 docker 场景未执行**：临时容器/临时卷的停启与删除重建、`docker pull`＋`--no-cache` 刷新演示。原因：容器内无 docker CLI（`command -v docker` 无结果），引擎仅在宿主。影响评估：删除重建已由本次真实部署事件覆盖（数据保留、入口正确、挂载身份在真实重建后成立）；首次构建已按刷新路径解析并记录当前 latest，README 流程供下次例行刷新使用。宿主侧 Agent 指令已下达：[2026-10-30 宿主侧验收指令](../../records/2026-10-30-host-side-acceptance-brief.md)。（→ 同日宿主侧已执行完毕并全部通过，见 [宿主侧运行验收](2026-10-30-host-side-runtime.md)）
- 计划 U2/U4 的"全新空卷"用例在本容器内以隔离 HOME＋全新目录等价覆盖（dev 可初始化、可写入），真实新空卷挂载仍属宿主场景。

## 脚本断言修正记录（开发过程中的失败用例）

初版脚本 4 个误报，均为断言方法错误而非被测对象缺陷，已修正：

1. 卷内可执行文件一概判"阴影"→ 误报卷内遗留 `pi` 启动器。实际规格要求是"不能命中"，改为前提断言（PATH 不含卷路径）＋实际解析断言（第 1 节），遗留文件降为信息项。
2. `/proc/mounts` 源字段匹配 `.container-state` → 9p 只显示盘符，改为跨路径标记同目录证明。
3. `.pi/agent` 目录 mtime 随会话写入更新（01:28 晚于容器启动）→ 改用树内最早文件 mtime。
4. `cc-switch --version` 输出含程序名前缀，汇总行重复显示 → 取末字段。

## 全量回归

在新镜像的隔离项目副本中执行：`git clone /workspace /tmp/ws-reg-036` 后 `env -u TCHAT_SANDBOX CARGO_TARGET_DIR=/tmp/ws-reg-036/target bash -c "cargo check --locked && cargo test --workspace --no-fail-fast --locked"`（副本专用 target 目录，不污染 `/workspace/target`；按 AGENTS.md 注记以 `env -u TCHAT_SANDBOX` 关闭沙箱运行全量，另行标注）。

- `cargo check --locked` 通过；测试总计 **3856 用例：3854 通过 / 2 失败 / 6 忽略**（83 个套件），总数与 2026-10-29 基线（3855＋1＋6）一致，失败位不同。
- 失败 1（布局耦合，非本轮引入）：`request_permissions::exe_deployment_prerequisite_refuses_workspace_and_temp`。根因：该测试第 684 行写死以 `/workspace` 模拟系统临时目录来触发拒绝路径，前提是测试二进制位于 `/workspace/target`（源码注释明示）；副本在 `/tmp/ws-reg-036` 时二进制不在 `/workspace` 下，`unwrap_err` 收到 `Ok` 而 panic（`request_permissions.rs:684`）。定向证据：标准位置 `/workspace` 重跑同套件 **38 通过 / 0 失败**。036/037 未改任何 Rust 代码，属既有测试对标准检出位置的耦合，单独登记不在本任务修复。
- 失败 2（已知不稳定）：`multi_agent::c09_fourth_child_rejected_and_group_assignment_cap`，即 [036 实施日志](2026-10-29-implementation.md)已登记的不稳定用例（全量并行负载下偶发）。定向复跑 5 次全部通过。与本轮变更无关。
- 其余 82 个套件全部通过；原始完整输出（临时留存）：`/tmp/ws-reg-036-full.log`。
