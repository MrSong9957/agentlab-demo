# 环境恢复统一清单（2026-11-03）

由容器内 Agent 以无 fork 通道（/proc 与文件直读）核实。**本 Agent 无宿主执行能力
（Bash 通道 EAGAIN，一次探测确认后未再重试）**，无法自行执行 docker 命令或字节级
复制——重启与 tar 保全须由宿主执行者按本目录 `recover.sh` / `verify.sh` 完成。

## 1. 容器核实（容器内视角）

| 项 | 值 | 来源 |
|---|---|---|
| 容器完整 ID | `b944a159ec468720c68dc9daf0824160c8ce6880d4498f932bafe81068e55d89`（短 ID `b944a159ec46`） | `/etc/hostname` ＋ `/proc/self/mountinfo`（/data/docker/containers/<ID>/）交叉一致 |
| 名称 `agentlab-demo-dev` | **未证实**（容器内无法读取名称映射）——宿主执行 `docker ps --filter id=$CID` 确认后以名称与 ID 双锚点操作 | 未知项→recover.sh 第 0 步 |
| 根文件系统 | overlay 可写层（desktop-containerd snapshotter） | `/proc/self/mountinfo` |
| `/workspace` | **bind mount（9p/drvfs）**：容器内 `/workspace` ← Windows 宿主 `D:\AgentLab\projects\demo`（WSL2 侧 `/mnt/d/AgentLab/projects/demo`）→ **文件天然持久** | mountinfo 第 469 行 |
| `/tmp` | **普通可写层**（无独立 tmpfs/volume 挂载）→ `docker restart` 保留，`docker rm`/重建才丢失 | mountinfo 无 /tmp 独立条目 |
| 其他持久挂载 | `/cache`、`/home/dev/.cargo`、`/home/dev/.claude`、`/home/dev/.codex`、`/home/dev/.zcode`（volume）；`/vault`、`/home/dev/.cc-switch`、`/home/dev/.pi`（9p） | mountinfo |
| cgroup | v2；容器内 `pids.max=max`（无限制）、`pids.current=23884`、命名空间进程总数 24268（`/proc/loadavg` 第 4 字段 3/24268） | 实测 |
| EAGAIN 来源 | **未知**：容器内可见三层配额均未触顶（cgroup pids=max、`RLIMIT_NPROC=unlimited`、`kernel.pid_max=4194304`、threads-max=126759）→ 疑在宿主侧父层级限制或资源分配，容器内不可见，需宿主 `docker inspect .HostConfig.PidsLimit` 与宿主 cgroup 树核实 | 未知项→recover.sh 第 0 步 |

## 2. 任务 A：`/workspace`（038 GitHub Actions CI）

- 工作目录 `/workspace`；HEAD＝分支 `feat/038-github-actions-ci` @ `57d5408cf2c6e80689d1adbc9f9ce8aa1a5796c0`（分支 tip 直接读取，非 detached；此前"Current branch: HEAD"快照与实际不符）。
- 对象库位置：`/workspace/.git/objects`，**自足**（`objects/info/alternates` 不存在、`.git/shallow` 不存在）。两个工作树共用此对象库。
- 未推送提交：`origin/feat/038-github-actions-ci` = `07641341`（迭代批次五），本地 tip `57d5408`（迭代 36）→ **约 6 个未推送提交**（批次六 `e7fae164`、批次七 `874c1c2d`、D2 补杀修正 `0c30fddd`、迭代 35 收口 `ce775c02`、c18 降级 `2a95f1b8`、迭代 36 `57d5408`）。`main` = `origin/main` = `4b85ce8`（已推送）。无 stash（`refs/stash` 不存在）。
- 未提交修改 / 未跟踪文件：**未知**（无 fork 无法运行 git status）→ recover.sh 前置快照采集，以此为准。
- 远端 `origin = https://github.com/MrSong9957/agentlab-demo.git`（`.git/config` 无内嵌凭据）。reflog 记载推送凭据曾随环境重置丢失。
- 验收结论（保留，不因环境恢复改变）：规格 `docs/specs/038-github-actions-ci.md` 状态＝**038 未完成**（迭代 36 收口记录于 HEAD 提交），六项未决保留；推送阻塞待凭据恢复。续接点：按规格"更新时间"行与六项未决清单继续，旧验证结果留档有效。

## 3. 任务 B：`/tmp/wt-038-f1`（迭代 37 D2 v2 补修候选）

- 工作目录 `/tmp/wt-038-f1`＝`/workspace` 仓库的**链接 worktree**（`.git` 文件 → `/workspace/.git/worktrees/wt-038-f1`，元数据在持久 bind mount 上；工作树文件本体在 overlay 可写层）。
- HEAD＝detached `57d5408`（与任务 A 当前 tip 同提交；worktree reflog 仅出生一条，无新提交）。创建时间约在迭代 36 提交后 46 分钟。
- 候选数据＝**未提交差异**（规格状态行原文："候选形态：隔离 worktree /tmp/wt-038-f1（基线 57d5408）未提交差异，等待用户审阅后并入任务分支；不提交、不推送"）。已实证至少 `docs/specs/038-github-actions-ci.md` 为修改态（worktree 版含迭代 37 段、HEAD 版无）。
- 参考差异清单（源自安全报告 `git diff --stat` 快照，最终以宿主 git status 为准）：
  `src/app/src/apply_patch.rs`、`src/app/src/mcp/transport.rs`、`src/app/src/proc_session.rs`（+327 行级）、`src/app/src/sandbox.rs`、`src/app/src/view_image.rs`、`src/app/tests/memory_storage_files.rs`、`src/app/tests/request_permissions_sandbox.rs`、`src/app/tests/sandbox_lifecycle.rs`、`docs/specs/016-*.md`（文件名未核实）、`docs/specs/031-request-permissions.md`、`docs/specs/033-tool-sandbox.md`（后两者已核实存在）。
  新增文件（疑未跟踪，已核实存在）：`docs/security/reports/2026-11-02-038-f1-isolation-security.md`。
- 临时取证脚本：`/tmp/sec-test-038/`（非仓库文件，安全报告引用）→ 保全范围必须含它（recover.sh 对未取得按阻断处理，不降级为提示）；/tmp 其余零散文件未枚举（无目录列举能力），宿主无 fork 枚举通道：`docker cp <CID>:/tmp - | tar -t`（守护进程侧归档读取，不在容器内建进程）。
- 清单权威性（2026-11-03 修订）：上文参考清单与迭代 37 报告仅作补充交叉核对；任务 A/B 的权威文件清单＝recover.sh 在备份副本上实测的 `status-ws-copy.txt` / `status-wt-copy.txt`（副本自足化后 `git status --porcelain -uall`），任何快照或报告不得替代。
- 验收结论（保留）：迭代 37 本地全量 3863 通过/1 失败（存量 pipe_ctrl_c）/6 忽略、e2e 150/150；候选**待用户审阅**，不提交不推送。续接点：审阅候选差异 → 决定并入 `feat/038-github-actions-ci`。

## 4. 此前报告矛盾纠正

| 此前说法 | 核实结论 |
|---|---|
| `/workspace` 是否 bind mount 存疑 | **是** bind mount（9p/drvfs，源 `D:\AgentLab\projects\demo`） |
| `/tmp` 重启是否保留存疑 | /tmp 属普通可写层 → `restart` 保留；删除/重建才丢失 |
| 读到 Git 引用即认为安全 | 引用≠对象库完整——对象完整性须 `git fsck`（recover.sh 对备份副本执行；已排除 alternates/shallow 干扰） |
| EAGAIN 推断进程冻结/配额耗尽 | EAGAIN 既不证明写入停止，也未定位具体配额（容器内可见配额均未触顶，见 §1） |

## 5. 保全与重启执行（宿主执行者）

```bash
cd /d/AgentLab/projects/demo/env-recovery-20261103   # Windows Git Bash 实测；WSL2 则 /mnt/d/…
bash validate_recovery.sh .              # 模拟验证（mktemp 临时目录＋docker shim，不触碰真实目标）；全部通过才生成 validate-result.txt
bash recover.sh                          # 强制门：未通过模拟验证拒绝启动；保全＋内容双读核验 → 全部通过才原地重启一次
bash verify.sh <recover 输出的确切备份目录>  # 显式传入、不用占位符；任一必要检查失败/缺失 → 退出码非 0，并归档证据
```

- 备份位置：`~/agentlab-recovery-<时间戳>/`（WSL2 ext4，容器外）；manifest.txt 记录命令、时间、sha256 与核验结果。
- 排除范围（2026-11-03 修订；tar 流上排除，绝不先落盘完整包再删除凭据）：`./target`（含全局 `CARGO_TARGET_DIR=/workspace/target` 副本）、`./.devcontainer/033-rollback`（大体积二进制，可由 deploy 脚本再生）、`.env`/`.env.*`（任意深度；宿主侧已核实根目录 `.env` 实存且含 1 个真实 API 密钥——`.gitignore` 使其不入 git status，「status 无 .env」≠「.env 不存在」，值不记录）、`./.container-state`、`./.claude`、`./.compound-engineering`、`./.context`（凭证/会话类目录）、`./src/app/config`（运行时生成的本地模型配置）、`*.pem`/`*.key`/`*.p12`/`*.pfx`（扩展名只是凭据信号之一，非唯一判据）。`.git/config` 无内嵌凭据（已核）随全量保留。已跟踪的非敏感测试夹具（证书/密钥等）**不回填副本**：其当前内容（`fixtures-current.tgz`，逐字节复核）与删除状态（`fixtures-status.txt`）独立保全；副本 status 如实显示排除结果（' D'），不宣称副本＝原工作目录的准确备份。`.env`*/凭证目录类的已跟踪文件仅登记 `tracked-review.txt` 交人工判断，不自动保全。排除在各备份通道落实：workspace 主备份 tar 流上排除；/tmp 侧三通道（docker cp／exec tar／export）均在提取侧排除；tar 后另有泄漏断言，清单再现任何排除项即失败。`.agents/`（项目技能与代理）非敏感，随全量保留。
- 失败语义：保全核验任一项失败 → recover.sh 退出且**不重启**；重启失败 → 保留现场，不自动重试、不升级为删除/重建。
- 保全通道层级（R2 修订）：/workspace 恒由宿主直读 bind 源；挂载源对应关系须**证实**方可继续——金丝雀文件经容器视图回读比对，或归一化路径相等，二者取一；已删除「目录尾部相同即放行」分支，无法确认即停止重启。/tmp 侧按 docker cp（守护进程侧读取，**不在容器内建进程**）→ docker exec tar（需容器内可 fork）→ docker export（兜底，按命名目录提取）取用。**docker export 只含容器可写层，不含任何 bind 挂载/volume——/workspace、/vault 等均不在导出内**；export 成功仅代表 /tmp 侧保全，两处备份是否完成分别以第 2、3 步各自结果为准。tar 提取参数已简化为「命名目录＋排除项」单一形式；recover.sh 第 0.5 步以深层小归档自检验证：递归提取完整、排除生效、缺失成员退出码非 0。
- 一致性（R2 修订）：git status 只作变更登记，**不作为保全条件**；保全条件＝workspace／wt-038-f1／sec-test-038 三者内容双读逐字节比对（同一源两次独立读取＋文件项计数下限防空跑），任一差异即阻断重启。保全前控制写入：可选人工 `docker pause`（脚本识别 paused 并在重启前 unpause，冻结期间一致性最强），默认依赖双读实证；pids 采样仅旁证。重启后 verify.sh 仅作回归检测，**不补充重启前条件**。
- 模拟验证强制门（R2）：recover.sh 启动前检查同目录 `validate-result.txt`（含 VALIDATION PASS，由 `bash validate_recovery.sh .` 在临时目录＋docker shim 内生成，不触碰真实目标）；未验证或验证失败则拒绝启动。
- 状态账本：本目录 `STATUS.md`（准备/备份/重启/核验四里程碑＋证据槽位）；verify.sh 末尾将全部证据归档至 `docs/logs/2026-11-03-env-recovery/`，归档完成后方可清理本目录。
- 本目录 `env-recovery-20261103/` 为本次恢复新增（未跟踪，5 个文件：README.md、recover.sh、verify.sh、validate_recovery.sh、STATUS.md；验证运行后另生成 validate-result.txt 与 validate-summary.txt）。

## 6. 能力边界声明

容器内 Agent：无 fork ⇒ 不能运行 git/docker/tar；无目录列举能力 ⇒ 未跟踪文件全集、/tmp 全目录内容标记未知；无宿主通道 ⇒ **重启未由本 Agent 执行**，仅交付已核实的 ID、清单与脚本。环境恢复成功≠任何代码任务验收通过。
