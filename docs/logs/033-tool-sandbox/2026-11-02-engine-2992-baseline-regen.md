# 引擎 29.8.2→29.9.2 升级后的容器故障排查、基线再生成与 sweep 误杀修复（2026-11-02）

## 0. 现象与结论

用户报告：ZCode 无法连接容器；从终端进入容器不久即掉线退回宿主机。

结论（按证据链）：

1. **Docker Desktop 引擎自动升级 29.8.2 → 29.9.2**（版本守卫与 compose 注释的
   预期版本失配；事件历史因引擎重启丢失，容器 FinishedAt=17:41:52 / 重启
   17:47:41 与引擎重启窗口吻合）。
2. **seccomp 基线过期**：旧基线按 docker-v29.8.2 → `moby/profiles/seccomp`
   **v0.2.3** 生成；29.9.2 的 `vendor/modules.txt` 指向 **seccomp/v0.2.4**。
   两版默认基线唯一差异＝socket 规则：v0.2.3 一条 `SCMP_CMP_GT 40`（放行
   domain>40），v0.2.4 改为逐个枚举 `LT 38 / EQ 39 / EQ 41..45`；其余 32 条
   规则与 defaultAction（ERRNO）一致。已按部署记录「基线再生成」链条重做
   （37 条默认＋6 条 033 前插＝43 条），deploy 守卫更新 29.8.2→29.9.2，
   容器重建（镜像全 CACHED），SecurityOpt 实证新基线生效。
3. **ZCode 无法连接的直接原因**：zcode server 死亡后变僵尸（容器
   pid1=`sleep infinity` 不 reap），`server.lock`/`status.json`（state=ready，
   陈旧）滞留持久卷；新实例经 `kill(pid,0)` 探测锁持有者，**僵尸进程依然
   "存活"** → 永远拒绝启动（"Another ZCode Server instance is already
   running"）→ ZCode 重连循环失败（19:59 ZCode 重连报
   "Reusing running Source Server → did not respond → exited before ready
   code=1"＝同一形态的第二次发作）。清理 run/ 锁文件重启后恢复。
4. **server 反复死亡的根因＝038 D2 ns 补杀误圈调用者 ns**（§3/§4）：
   `sweep_namespace_members` 拿组长（bwrap）自身的 ns inode——容器实测等于
   调用者 ns——扫 /proc 杀「同 ns」进程＝SIGKILL 容器全体。沙箱会话终止
   （E2E 沙箱用例/用户任务 cancel）即触发；zcode server、任务进程树、
   E2E 测试树连坐（第一轮 17:49 server 死亡＋17 个任务进程僵尸；
   第二轮 18:04–18:27 server 死亡与 18:12 E2E 灭杀同窗，97 个
   terminal-chat 僵尸）。修复见 §4，修复后 E2E 全量 150/150（§3）。

## 1. 基线再生成操作记录

- `GET api.github.com/repos/moby/moby/contents/vendor/modules.txt?ref=docker-v29.9.2`
  → `github.com/moby/profiles/seccomp v0.2.4`（L1344）。
- 注意 tag 布局：`moby/profiles` 仓库 tag 带**子目录前缀**——`seccomp/v0.2.4`
  （直接用 `v0.2.4` 会 404 "No commit found for the ref"）。
- `GET .../repos/moby/profiles/contents/seccomp/default.json?ref=seccomp/v0.2.4`
  （raw media 通道）→ 14122 字节。
- `python3 scripts/acceptance/033-seccomp-patch.py default.json 033-seccomp.json`
  → `inserted 6 rule(s); total 37 -> 43`（旧链 33→39）。
- v0.2.3↔v0.2.4 结构化 diff：仅 socket 规则（上表），无其他差异。
- `.devcontainer/deploy-033-host.sh` 版本守卫 29.8.2→29.9.2；
  compose.yaml 注释同步。执行 deploy 脚本：守卫通过→回滚材料固化→
  build（CACHED）→ up -d（Recreated）→ SecurityOpt 实证新 socket 枚举规则。

## 2. 重建后验证（verify-033-deploy.sh）

- 第 1-5 步 PASS：gitleaks 恢复、bwrap 0.9.0、TCHAT_SANDBOX 注入、产品干跑
  横幅、三型沙箱集成（exec/mcp_stdio/apply_patch）、假凭据遮蔽。
- 第 6 步（普通模式回归 E2E）当时未完成——根因与闭合见 §3；最终全量回归
  结果见 §5。
- 独立验证：`cargo check` 通过（20.1s，44 条存量 warning，无错误）。
- zcode server：清锁重启后 state=ready，crashCount=0。

## 3. E2E 全量死亡：根因确认与闭合（原「验证缺口」，已解决）

**E2E 全量（150 用例）在容器内被无声 SIGKILL 灭杀**，修复前多次复现：

- 死亡形式：测试进程树（exec bash→cargo→e2e 二进制）无声 SIGKILL；无
  panic、无 FAILED 报告；并行（s57-s59 窗口，`[e2e-spawn]` 行终止、ok 未打，
  最后写入 18:12:12 / 18:34:25，启动后约 20-40s）与串行（s140，18:40）均死；
  收尸 bash 同批消失（log 无 serial-exit 行）；log 大小巧合（并行 193458 /
  串行 193546 字节，SIGKILL 丢失块缓冲，尾部≠死亡真实进度）；串行越过并行
  死亡窗口（s102、s130+ 正常），死亡点漂移。
- 已排除：OOM（cgroup memory.events oom_kill=0 累计不变，memory.peak
  4.1GB/16GB）；宿主/timeout 超时（死亡早于全部超时窗）；cgroup pids；
  测试侧组杀代码（tests/ 仅 `killpg(pgid,0)` 探测与单 pid kill）；E2E
  helper（pty.rs spawn CLI 时 pre_exec setsid，CLI 独立会话，其组杀打不到
  测试组）。
- 单独跑不触发：s59 单测 2.46s、s5* 家族 10 例 5.8s、exec_sandbox timeout
  用例（policy=None，sweep 不触发）均 PASS＋哨兵存活。

**根因＝§4 的 sweep 误杀**：E2E 沙箱用例（e2e_acceptance.rs 内 4 处
`env TCHAT_SANDBOX=1` 注入 CLI）经生产路径 `frozen().map(...)` 得
policy=Some → ns_inode 误圈调用者 ns → 沙箱会话终止（timeout/cancel/
close）触发 sweep → SIGKILL 容器 ns 全体（E2E 树＋verify 树＋zcode
server 连坐）。18:34 一次死亡中 setsid 哨兵幸存属另一触发路径（该次无
sweep；修复后不再复现，未再单独深挖）。

**修复后验证**：E2E 全量 **150/150 PASS**（30.6s，两轮；首轮唯一失败
s141＝ETXTBSY 链接瞬态，单跑 3.1s PASS 后全量复跑全绿）；setsid 哨兵
幸存；`cargo check` 通过。

## 4. sweep 误杀修复（038 D2 回归）

**缺陷**（proc_session.rs 管道模式与 PTY 模式两处 spawn 的 ns 捕获）：
沙箱模式（policy=Some）用 `read_ns_inode(child.id())` 读组长（bwrap）
自身——容器实测（完整 033 形态 argv）bwrap **自身**留调用者 ns
（`/proc/<bwrap>/ns/pid` == `/proc/self/ns/pid` == `pid:[4026532384]`），
只有**载荷**在新 ns（`pid:[4026532464]`）——bwrap raw_clone 一次建 ns 给
载荷。旧实现圈定调用者 ns，`sweep_namespace_members`（monitor_thread 终止
路径）扫 /proc 杀「同 ns」＝调用者 ns 全体进程。触发面：生产路径
（`start_with_owner`，policy=`frozen().map(...)` 为 Some 时）的任何沙箱
会话终止；测试路径 `ProcessManager::start` 传 policy=None 不触发（与
「038 D2（CI 实证）」注释的矛盾由此解释：CI 上未踩中该交错）。

**修复**（`read_payload_ns_inode`，替换两处调用点）：遍历组长
`/proc/<leader>/task/*/children`，取 `ns/pid != self` 的载荷 ns；载荷
瞬时退出（children 不可得）返回 None 降级为不补杀——killpg 兜底仍在，
不劣于无 D2 的旧行为。

**回归测试**（proc_session.rs `ns_sweep_tests`，模块内单测访问私有函数）：
①完整沙箱形态 bwrap 下捕获载荷 ns 且 ≠ 调用者 ns（500ms 轮询窗）；
②非沙箱进程（无新 ns 子进程）返回 None。bwrap 不可用环境跳过。
**有效性已证明**：临时改回旧实现形态（读组长自身 ns）→ 测试 FAILED
（断言「载荷 ns 不得等于调用者 ns」触发）→ 恢复正确实现 → PASS（1/1）。

**受影响测试**：回归测试 1/1；exec_sandbox 2/2、mcp_stdio_sandbox 1/1、
apply_patch_sandbox 7/7（sweep 触发路径）；E2E 全量 150/150。

## 5. 全量回归（AGENTS.md §8，env -u TCHAT_SANDBOX）

- 命令：`cargo check && cargo test --workspace --no-fail-fast`（容器内
  /workspace，后台落盘 /tmp/full-regression.log，173 个测试目标）。
- 结果：**3856 passed / 1 failed / 6 skipped**；cargo check 通过（44 条
  warning 均为存量，无新增；无编译错误）。
- 唯一失败 `pipe_ctrl_c_interrupts_and_terminate_kills_ignoring_process`
  （proc_session，SIGINT 后期望 Completed 实际 Running）：**单独重跑
  PASS（1.98s，--test-threads=1）**——判定为全量并发负载下的时序瞬态，
  非本次修复引入（修复仅触及 policy=Some 的 ns 捕获；该测试走 pipe
  Ctrl+C→kill_group_sigint 路径，与 ns 圈定无交集）。按规则不计为通过，
  如实列为「瞬态失败，单跑复验通过」；若复现加剧需另行排查其负载敏感度。
