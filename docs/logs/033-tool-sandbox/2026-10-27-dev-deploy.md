# 033 沙箱 dev 容器部署记录（2026-10-27）

状态：**已部署并默认启用**（2026-10-27 宿主执行部署退出码 0、容器内验证 8 PASS/0 FAIL
退出码 0；本记录为部署材料的单一索引，全部改动为可审查差异，不提交）。

## 1. 执行位置与拓扑（实证）

- 容器内会话（host=d2944f0a103b，dev 用户）完成：配置修改、seccomp 生成、预构建、
  部署/验证脚本；容器内无 docker CLI（`sudo docker` 不存在）——**重建必须由宿主执行**。
- `/proc/self/mountinfo` 实证：`/workspace`＝`D:\AgentLab\projects\demo`（9p drvfs），
  `/vault`＝`D:\AgentLab\shared\Obsidian`。compose 的 seccomp 绝对路径据此写死。
- compose：项目 `agentlab-demo`，服务 `dev`，镜像 `agentlab-demo-dev:latest`，
  容器 `agentlab-demo-dev`；卷 `zcode-home/claude-home/codex-home/cargo-home/cache`
  （会话记录、记忆、cargo 缓存均在卷内，重建保留）。

## 2. 改动清单（可审查差异，全部未提交）

| 文件 | 类型 | 内容 |
|---|---|---|
| `.devcontainer/Dockerfile` | 修改（已跟踪） | dev 镜像持久安装 bubblewrap（USER root 段独立一层，同 033 验收镜像做法） |
| `.devcontainer/compose.yaml` | 修改（已跟踪） | ① `environment.TCHAT_SANDBOX: "1"`；② `security_opt: seccomp=D:/AgentLab/projects/demo/.devcontainer/033-seccomp.json` |
| `.devcontainer/033-seccomp.json` | 新增 | seccomp 放宽 profile（由执行 docker 的 CLI 在宿主侧读取文件内容、经 API 下发给引擎；引擎不读宿主路径，见 §3） |
| `.devcontainer/deploy-033-host.sh` | 新增 | 宿主执行：版本守卫→回滚材料固化→镜像回滚点→build→`up -d dev` |
| `.devcontainer/033-rollback/` | 部署时生成 | 回滚材料：本轮修改前快照（compose.yaml.pre-033 / Dockerfile.pre-033）＋正向补丁 033-host-changes.patch（快照取自 HEAD——部署前实证两文件相对 HEAD 的 diff 仅含 033 改动） |
| `.devcontainer/verify-033-deploy.sh` | 新增 | 重建后容器内执行的全部验证 |
| `.dockerignore` | 新增 | 构建上下文裁剪：排除 `**/.env`（凭据不进构建上下文）、`.git`/`target`/`.worktrees`（传输裁剪）；Dockerfile 无 COPY，对镜像内容零影响 |
| `docs/logs/033-tool-sandbox/2026-10-27-dev-deploy.md` | 新增 | 本记录 |

保护措施说明：build context 为项目根（既有行为）——新增 `.dockerignore` 排除 `.env`
与超大目录后，凭据不再进入构建上下文（见上表）；镜像内容不含任何项目文件。

## 3. seccomp 生成（沿用 033 最终验收链条）

- 锁定关系：引擎 `docker-v29.8.2` → `vendor/modules.txt` → `moby/profiles/seccomp`
  **v0.2.3** → `default.json`（api.github.com raw media 通道，与 2026-10-27 宿主验收
  同链）；本轮容器内重跑实证同版本。
- 补丁：`scripts/acceptance/033-seccomp-patch.py` 前插 6 条 033 规则，**33 → 39**
  （`syscalls` 数组实证；与宿主验收数字一致）。
- 引擎版本变化时（deploy 脚本守卫会拒绝）：按本节链条以新 `docker-v<版本>` 重新生成，
  不得沿用旧基线（fail closed）。

## 4. 预构建（重建前完成，产物随 bind 挂载保留）

- 源码口径：main `28b7999`；`git status -- src/ Cargo.toml Cargo.lock` 仅
  `?? src/app/config/`（运行时副产物，不参与编译）——**产品代码与 HEAD 一致**。
- `cargo build --locked`（CARGO_TARGET_DIR=/workspace/target）通过，1m03s；
  warning 2 条均为 `src/shims/codex-config` 存量（非 033 引入）。
- 3506/0/3 全量与宿主真实沙箱验收证据按覆盖范围复用，不重跑。

## 5. 宿主执行步骤

```
# Git Bash（宿主）：
cd /d/AgentLab/projects/demo
bash .devcontainer/deploy-033-host.sh
```

脚本内含：引擎版本守卫（≠29.8.2 即拒绝）→ seccomp 就绪检查（CLI 宿主侧读取）→
**回滚材料固化**（本轮修改前快照＋正向补丁，见 §7）→ 活跃容器展示（仅展示，
不处理）→ `docker tag …:pre-033-rollback` 回滚点（已存在则不覆盖，防重复执行污染）
→ `docker compose build dev` → `docker compose up -d dev`（保留全部卷；仅 dev 服务；
无 down -v）→ SecurityOpt 确认。

## 6. 重建后验证（容器内）

```
bash /workspace/.devcontainer/verify-033-deploy.sh
```

覆盖：gitleaks 恢复（v8.30.1，checksum 校验）→ bwrap 0.9.0＋`TCHAT_SANDBOX=1` 注入
断言 → 产品干跑横幅（bwrap 可用）→ 三型沙箱集成真实分支（exec/mcp_stdio/apply_patch，
含管道 failIfUnavailable、PTY 超时组杀、PID ns）→ 独立假凭据遮蔽实验（沙箱内 0 字节＋
cat 拒绝＋外部标记仍在＋pid1=载荷＋普通文件读写）→ e2e_acceptance（本地 mock）→
`.env` 元数据前后一致。

**验证顺序与开关口径**（2026-10-27 修正）：脚本先验证持久开关注入与默认启动
行为（compose 注入态原样运行，不清除、不补设开关）；三型沙箱集成同样在默认
注入态运行——用例内部 `set_var`/`remove_var` 自控开关并以互斥锁串行，不受外部值
影响（源码实证：`exec_sandbox.rs`/`mcp_stdio_sandbox.rs`/`apply_patch_sandbox.rs`）；
**仅普通模式回归**（e2e_acceptance，3506/0/3 基线在无此变量下取得）单独
`env -u TCHAT_SANDBOX` 清除开关后运行。日常跑全量回归同理须 `env -u` 起手。

## 7. 回滚（可执行命令）

回滚材料由部署脚本第 2 步固化到 `.devcontainer/033-rollback/`（本轮修改前快照
＋正向补丁）。**不用整文件 `git checkout`**——它会覆盖工作区内其他任务的未提交
改动；精确反向补丁可检测到此类冲突并拒绝执行，冲突时人工核对后从快照恢复。

```sh
# 宿主 Git Bash：
cd /d/AgentLab/projects/demo
# 1) 配置还原：优先精确反向补丁（仅撤销 033 的 Dockerfile/compose.yaml 改动；
#    与其他任务后续改动冲突时 apply 会失败报错，此时人工核对后改用快照整文件恢复：
#    cp .devcontainer/033-rollback/compose.yaml.pre-033 .devcontainer/compose.yaml
#    cp .devcontainer/033-rollback/Dockerfile.pre-033   .devcontainer/Dockerfile）
git apply -R .devcontainer/033-rollback/033-host-changes.patch
# 2) 删除本轮创建且未被其他任务修改的文件（.dockerignore/033-seccomp.json/
#    deploy/verify 脚本均为 033 本轮新增；033-rollback/ 与本日志保留为审计材料）
rm -f .dockerignore .devcontainer/033-seccomp.json \
      .devcontainer/deploy-033-host.sh .devcontainer/verify-033-deploy.sh
# 3) 镜像还原（pre-033-rollback 为部署前原镜像 tag，部署脚本第 4 步备好且不覆盖）
docker tag agentlab-demo-dev:pre-033-rollback agentlab-demo-dev:latest
# 4) 容器还原（保留全部卷，仅 dev 服务）
MSYS_NO_PATHCONV=1 docker compose -f .devcontainer/compose.yaml up -d dev
```

容器内软开关（不重建临时关闭）：删除/置空 compose 的 `TCHAT_SANDBOX` 并重建，
或临时会话 `unset TCHAT_SANDBOX`（仅当次会话）。

## 8. 边界与如实记录

- **Windows 原生沙箱：未实现**——`sandbox_requested()` 在 `#[cfg(not(unix))]` 下恒
  `false`（`src/app/src/sandbox.rs`），非 Linux 平台该开关静默无效、回普通模式；
  本轮未启动、未实测任何 Windows 原生隔离。
- **共享 dev 容器部署完成后即为本记录所称"已部署"**；生产/其他环境不在本轮范围。
- `.env`：原位、内容不变；全程仅允许 size/mtime 元数据核对（验证脚本第 0/7 步）。
- 活跃任务（2026-10-27 10:02 UTC 容器内 `ps` 全量实测，不依赖 docker ps）：**无活跃
  计算任务**——cargo 进程 4 例均为 defunct 僵尸；zcode server（pid 45/56/154+）与
  codex app-server daemon（408/470/1079）为常驻服务，数据在卷、重连自愈；codex
  交互会话 875406 挂起 26.7h 仅 10m48s CPU 且无子进程（空闲）；bash 304/316 为
  2 天空闲 shell。重建影响＝断开上述服务与会话的连接（在已批准的必要重建范围内），
  持久数据零丢失（5 卷挂载实测正常，/workspace bind 正常）。泄漏进程
  （pid 3701291 等）与僵尸随重建自然清除，未单独强杀。
- 此前批次泄漏进程（pid 474850/935465 等）本轮不清理（维持 033 实现记录决定）。
- gitleaks 装于 `/home/dev/.local/bin`（不在任何卷内）——重建必然丢失，验证脚本第 1 步
  按 `scripts/hooks/README.md` 恢复（checksum 校验，fail closed）。
- 密钥轮换、真实模型服务调用：未做、不做；E2E 与验证全部本地 mock。

## 9. 实际执行记录（2026-10-27，宿主侧接续）

- **部署**：`bash .devcontainer/deploy-033-host.sh`（宿主 Git Bash，项目根
  `D:\AgentLab\projects\demo`）——**退出码 0**。执行中发现并修复脚本两处自身问题
  （非部署配置问题）：① `docker compose` 缺显式 `-f`（compose.yaml 在
  `.devcontainer/` 下，默认搜索当前目录找不到）；② `-f` 传 Git Bash 的
  `/d/...` 路径在 `MSYS_NO_PATHCONV=1` 下不转换、docker.exe 解析失败，改用
  `cygpath -m` 转 `D:/...` 盘符正斜杠（与宿主验收配方一致）。
- **部署确认**：Engine 29.8.2 守卫通过；镜像重建（bubblewrap 0.9.0-1ubuntu0.3
  安装层 66s，rustup 层缓存命中）；容器 Recreated+Started；`docker inspect`
  SecurityOpt 显示 **seccomp= 后为完整 JSON 内容而非路径**（实证：CLI 在宿主侧
  读取文件、经 API 下发引擎），六条 033 规则（clone 双形态 939655185/402784273、
  unshare CLONE_NEWUSER、pivot_root、mount、umount2）全部在场。回滚点
  `agentlab-demo-dev:pre-033-rollback`（旧镜像，含防覆盖保护）。
- **验证**：`bash /workspace/.devcontainer/verify-033-deploy.sh`（docker exec
  容器内）——首次 7 PASS/1 FAIL：第 5 步遮蔽断言口径错误（期望 `size=0`，实测
  `wc -c < .env`＝open EACCES——userns 内非特权 bind 被内核强制 nodev，
  与产品 A2「正文不可得＋stat 级」口径不一致），修正断言（接受 OPEN-DENIED 或
  size=0）后重跑——**8 PASS / 0 FAIL，退出码 0**：
  ① gitleaks 8.30.1 重装（checksum 校验）；② bwrap 0.9.0；③ TCHAT_SANDBOX=1
  注入（无手工 export）；④ 干跑横幅「工具沙箱模式：exec/MCP/apply_patch 经
  bubblewrap 隔离（/usr/bin/bwrap）」；⑤ 三型沙箱集成 3 目标全绿（默认注入态，
  真实 bwrap 分支：exec 管道 failIfUnavailable/PTY 超时组杀、MCP stdio 白名单
  env、apply_patch 三类操作）；⑥ 遮蔽实验（.env 正文不可得＋cat 拒＋外部标记
  在＋pid1=载荷＋普通文件读写）；⑦ 普通模式回归 e2e_acceptance（单独
  `env -u TCHAT_SANDBOX`，本地 mock）；⑧ .env 元数据前后一致（64B，未读正文）。
- **验证日志（容器内）**：`/tmp/033-verify-sandbox.log`（三型）、
  `/tmp/033-verify-e2e.log`（E2E）；临时目录 `/tmp/033-gl` 与遮蔽夹具已清理。
- **默认启动**：`docker exec -it agentlab-demo-dev bash` → `cargo run`（或管道
  `echo hello | cargo run --quiet`）——TCHAT_SANDBOX=1 由 compose 注入，
  启动即沙箱模式，无需手工 export。
- **产品代码未变**（main 28b7999，src/ 无改动）：全量与性能证据按覆盖范围复用
  （§4），未重复执行。
