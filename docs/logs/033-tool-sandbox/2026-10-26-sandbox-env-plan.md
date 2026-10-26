# 033 真实沙箱验收——最小环境变更方案（草案，待批准；2026-10-26）

**本轮状态**：只读核查完成，方案未实施。批准后按 §3 顺序执行。
**基线证据**：`2026-10-26-feasibility-probe.md`（探针）、`2026-10-26-sandbox-security.md`（攻防）、
`2026-10-26-implementation.md`（实现与全量）。

> **收口轮精确化（同日）**：本文 §2 变更 B 的「clone/unshare 规则放宽」描述已被
> **精确补丁**取代——实际产物为 `scripts/acceptance/033-seccomp-patch.py`（结构化
> 修改器，含幂等与位级等值规则：unshare 恰等于 NEWUSER/NEWNS/NEWPID/NEWIPC 各
> 一条＋mount 族放行，不改默认条目）＋`scripts/acceptance/033-sandbox-accept.sh`
> （宿主一键验收：默认对照→seccomp→probe 断言→真实分支套件→PID 视图→假凭据
> 探针→全量→清理）＋`scripts/acceptance/033-accept-bwrap.Dockerfile`（验收镜像）。
> **成品已预生成**：`scripts/acceptance/generated-033-seccomp.json`
> （moby **v27.5.1** 默认 profile 31 规则 → 36 规则，结构化差异断言通过：
> 仅前插 5 条、默认条目逐字节保留；脚本运行时按实际引擎版本重取重生成，
> 幂等）。生成器逻辑已在合成 profile 上验证（插入/幂等/位级等值断言全过）。
>
> **执行入口阻断（二次修正方法复核后维持）**：初版 tcp 探针经 `sh` 的
> `/dev/tcp`（dash 不支持，方法无效）；修正为 curl 重测——`host.docker.internal`
> 可解析（fdc4:f303:9324::254）但 2375/2376 连接失败（000ERR），容器内无
> docker socket/CLI/DOCKER_HOST——**无宿主执行入口成立**。本轮未执行验收，
> 由用户在宿主机运行 `scripts/acceptance/033-sandbox-accept.sh`；脚本输出即
> 真实验收证据，本报告不代称验收完成。
>
> **宿主入口终检修正（同日第三轮）**：①bwrap 0.9.0 源码核对——ns 创建为
> **合并标志 clone**（bubblewrap.c:2900，无 unshare() 调用）；seccomp 补丁改为
> **单一精确等值 clone 规则**（939655185＝SIGCHLD|NEWNS|NEWUSER|NEWPID|NEWIPC，
> excludes s390 系）＋pivot_root/mount/umount2（默认规则按 init_user_ns 判
> CAP_SYS_ADMIN，userns 内不满足；内核调用点能力检查仍在），删除全部 unshare/
> setns 规则。②默认基线无法取得/无法确认对应时**非零退出**（不回退旧版继续、
> 不启动验收容器）；seccomp 参数统一 **CLI 本地可读的 HOST_JSON**
> （`--security-opt "seccomp=$HOST_JSON"`），删除 daemon 路径推导；启动前回归
> 检查文件存在/可读/合法 JSON。成品：`scripts/acceptance/generated-033-seccomp.json`
> （v27.5.1 基线 31→35 规则，结构化差异断言全过，确定性与幂等已验）。
> **一条执行命令见脚本头部注释。**

## 1. 拒绝原因定位（只读核查结论，全部实证）

| 候选原因 | 核查方法 | 结论 |
|---|---|---|
| 缺少 bubblewrap | `which bwrap`（rc=1）；`apt-cache policy bubblewrap` | **确认**：未安装；Ubuntu 24.04 源候选 **0.9.0-1ubuntu0.3** |
| 容器策略拒绝 userns | 纯 `unshare --user --map-root-user`（无 mount）EPERM；对照纯 `unshare --mount` 亦 EPERM；`CapBnd=00000000a80425fb`（无 CAP_SYS_ADMIN）；`Seccomp: 2` | **确认**：Docker 默认 seccomp 对 `clone/unshare(CLONE_NEWUSER)` 要求 CAP_SYS_ADMIN，容器 bounding set 无此能力——**唯一需要变更的策略层** |
| 宿主策略拒绝 | `/proc/self/attr/current`＝`kernel`（**无 AppArmor profile**）；`/proc/sys/user/max_user_namespaces`＝63379（正常）；`unprivileged_userns_clone` 项不存在（微软 WSL 内核非 Debian 补丁）；`apparmor_restrict_unprivileged_userns` 不可见（未启用） | **排除**：宿主侧无 userns 全局限制 |

补充事实：dev 具 `sudo (ALL) NOPASSWD: ALL`（027 实测沿用，本轮未改）——安装通道可用；
compose 无 `security_opt/cap_add/privileged/sysctls`（默认安全面）；compose.yaml 与 Dockerfile 均
git tracked（可回滚）；rustup 工具链在**镜像层**（Dockerfile RUN rustup，/usr/local/{rustup,cargo}），
volumes 五个（workspace/vault bind＋zcode/claude/codex/cargo/cache 卷）——**重建容器不丢工具链与卷数据**。

## 2. 推荐方案（最小局部调整；不采用 privileged / 宽 CAP_SYS_ADMIN / 宿主全局策略变更）

### 变更 A：容器内安装 bubblewrap（唯一安装项）

- 执行：`sudo apt-get update && sudo apt-get install -y bubblewrap`（Ubuntu 源 0.9.0）。
- **推荐同时固化进 Dockerfile**（`.devcontainer/Dockerfile` 的 USER root 段追加一行
  `RUN apt-get update && apt-get install -y bubblewrap && rm -rf /var/lib/apt/lists/*`）：
  避免容器重建后丢失（否则每次 up --build 后需重装）。
- 扩大的权限：**无**（用户态工具，无 setuid 位，能力依赖运行时策略）。

### 变更 B：放行非特权用户命名空间（仅此一项策略；局部 seccomp 定制）

- 执行：新增 `.devcontainer/seccomp-allow-userns.json`——**以 Docker Engine 对应版本的
  官方默认 profile 为底**（moby `profiles/seccomp/default.json`，执行前按 `docker version`
  取对应 tag 下载），仅修改三处默认规则：
  1. `clone`：默认对含 `CLONE_NEWUSER` 的调用要求 CAP_SYS_ADMIN → 放宽为始终允许；
  2. `unshare`：同上放宽；
  3. `setns`：保持默认不变（无需求）。
  compose `dev` 服务增加：`security_opt: ["seccomp:<宿主可达路径>/seccomp-allow-userns.json"]`。
- **为什么需要**：bwrap 的 `--unshare-user`（033 argv 的第一道命名空间，后续 pid/ipc/procfs
  成对机制都构建其上）在非特权路径下必须能创建 user namespace——这是**沙箱的构成性能力**，
  不是附加权限。
- **扩大的权限与面**：容器内任意进程获得「创建用户命名空间」能力。ns 内的 uid 映射不
  提升宿主侧权限（能力集受 ns 约束）；社区共识为低风险（Chrome/Firefox/flatpak 常规依赖）。
  实际受影响面＝容器内进程；宿主与其他容器不受影响。
- **明确不采用**（及理由）：
  - `cap_add: SYS_ADMIN`：把近 root 能力授予容器**全部进程**，远大于「单点放行 userns」；
  - `seccomp=unconfined`：关闭整个 syscall 过滤，远宽于三条规则修改；
  - 宿主 `apparmor_restrict_unprivileged_userns` 类调整：已核实宿主无此策略，无对象；
  - `apparmor=unconfined`：已核实容器无 AppArmor 约束，无对象——**这两项无需任何变更**。
- 若执行后 bwrap 干跑仍失败（默认 profile 之外的第二层限制，当前证据不指向它）：
  下一级局部选项＝dev 服务 `security_opt` 追加 `apparmor=unconfined`（仅该服务）——
  **属扩大项，先停，等用户单独批准**。

### 变更 C：无宿主侧、无账号/sudoers/ACL 变更（核实结论：无需）

## 3. 执行步骤（批准后按序；预计 20–40 分钟）

1. **快照**：`docker compose -f .devcontainer/compose.yaml config > /tmp/compose-before.yaml`
   （宿主机执行）；记录 `docker version`；确认 /workspace 与全部卷已持久化（现状即满足）。
2. **改 Dockerfile**（变更 A 固化）与**新增 seccomp json**、**改 compose**（变更 B）——
   三处 diff 保留在 git 工作区（本轮仓库状态干净，回滚即 git checkout）。
3. **重建**：宿主机 `docker compose -f .devcontainer/compose.yaml up -d --build`
   （工具链在镜像层保留；五卷持久；容器内容器层安装过的其他东西会丢——执行前
   `dpkg --get-selections` 快照比对一次，确认无非镜像层的必要自装项）。
4. **容器内验证链**（逐条，失败即停在该层）：
   a. `bwrap --version`（≥0.9）；
   b. `unshare --user --map-root-user true; echo $?`（应 0——策略放行生效）；
   c. worktree 内 `CARGO_TARGET_DIR=/tmp/wt033-target cargo test --locked --offline --lib sandbox::`
      ——此时 `probe_available()` 转为 true，全部用例自动走**真实沙箱分支**；
   d. 按下节验收清单逐项执行。
5. **回滚**（任一步不满意）：
   - `git checkout -- .devcontainer/compose.yaml .devcontainer/Dockerfile`＋删除
     `seccomp-allow-userns.json`；
   - 宿主机重新 `up -d --build`（恢复默认 seccomp 与无 bwrap 镜像）；
   - 可选 `sudo apt-get remove -y bubblewrap`；
   - **独立恢复入口**（dev 失能时）：宿主机 `docker exec -it agentlab-demo-dev sh`（容器 root）
     或宿主机重建容器（027 附录同款通道）。
   - 回滚后本分支测试自动回到 failIfUnavailable 分支（产品代码不受影响）。

## 4. 真实验收清单（变更完成后逐项；假凭据夹具，全部可离线复跑）

### A. 套件层（夹具内建于测试，自动清理；有 bwrap 后自动走成功分支）

| # | 命令（worktree 内） | 验收点（真实沙箱分支的新覆盖） |
|---|---|---|
| A1 | `CARGO_TARGET_DIR=/tmp/wt033-target cargo test --lib sandbox::` | 15/15；**新增生效断言**：锚定根 .env 被遮蔽（`test ! -s` 通过）＝**沙箱内真实拒读** |
| A2 | `... --test exec_sandbox` | 2/2：同 uid `id -un`＋哨兵 env 拒绝＋锚定根 .env 拒读（workdir 转移不影响，G1）＋**跨 PID ns 的 PTY 回显与超时组杀**（016 契约重验） |
| A3 | `... --test mcp_stdio_sandbox` | 1/1：MCP 往返＋HOME=锚定根（白名单）＋污染 env 拒绝 |
| A4 | `... --test apply_patch_sandbox` | 3/3：Add/**Update（G4-R1 修复验证：/proc/self/fd 写绑定在沙箱内恢复）**/Delete |
| A5 | `... --test exec_command --test mcp_stdio` | 18＋35：普通模式零变化（对照面） |

### B. 手工探针层（主程序链路最小集；不依赖模型服务）

夹具：worktree 根临时写 `FAKE-SECRET-LINE=1` 的 `.env`（验收后删除；**不触真实凭据**）。
```bash
# B1 主程序读 .env 正常（dotenv 契约）＋沙箱开关生效：
TCHAT_SANDBOX=1 cargo run --quiet <<< '/quit'     # 启动横幅应含「工具沙箱模式：… bwrap」
# B2 沙箱内 exec 对锚定根 .env 拒读（经集成测试 A2 已自动化；手工等价命令）：
#    （沙箱模式下 exec_command 执行）`cat /workspace/.worktrees/033-tool-sandbox/.env` → 输出为空
# B3 apply_patch 沙箱三型（不经模型的内部链路直证）：
TCHAT_SANDBOX=1 cargo test --test apply_patch_sandbox -- --nocapture
# B4 不可用时拒绝执行（对照面，去掉 bwrap 的环境不重跑——已由 failIfUnavailable 分支覆盖）
```

### C. 全量（验收收尾）

- `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 cargo check --locked --offline &&
  cargo test --workspace --no-fail-fast --locked --offline`——预期 68 目标、
  **3505+ passed / 0 failed / 3 ignored**（与收口轮一致；沙箱用例真实分支数不改变断言数）。
- 证据落盘：`docs/logs/033-tool-sandbox/` 追加验收记录（命令＋退出码＋关键输出脱敏）。

## 5. 外部残留进程清理建议（单列；本轮未执行）

执行前**必须**再次核实（防 PID 复用）：`ps -p <pid> -o pid,ppid,user,etime,args --no-headers`
输出与下表完全一致（尤其 args 逐字与 PPID=1）才可执行；用精确 pid，不用模式匹配。

| PID | 当前身份（2026-10-26 核实） | 所属 | 清理建议 |
|---|---|---|---|
| 474850 | nobody；PPID=1；`/bin/sh -c while true; do echo x >> /tmp/tchat-exec-474826-iso-timeout/w.log; sleep 0.05; done & sleep 30`（14h39m；w.log≈2MB） | **027 B** exec_command 隔离 PTY 超时用例组杀失效的实证残留（该用例已随 033 替换为同 uid 沙箱版） | `kill 474850`（SIGTERM）；随后确认 w.log 不再增长 |
| 935465 | dev；PPID=1；`/bin/sh -c while [ ! -e /tmp/tchat-test-934929-s103-user-357117770/work/gate ]; do sleep 0.05; done`（8h45m；gate 不存在） | **024** s103 用例 gate 等待残留 | `kill 935465` |

风险说明：两者均为孤儿 shell 循环（无数据、无网络、仅 /tmp 写/空转）；不清理仅浪费少量
CPU。027 的组杀失效面在 033 已由同 uid 沙箱＋`--unshare-pid` 成对机制结构性关闭。

## 6. 约束确认

本轮零实施（未安装、未改 compose/Dockerfile/seccomp、未动进程）；`.env` 未读取正文；
未调用真实服务；worktree 产品代码冻结未动；不提交、不合并、不推送。
