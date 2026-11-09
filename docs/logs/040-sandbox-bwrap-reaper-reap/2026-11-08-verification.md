# 040 验证记录（2026-11-08）

环境：devcontainer（bwrap 0.9.0、内核 6.18 WSL2、rustc stable、libc 0.2.189）；分支 `feat/040-sandbox-bwrap-reaper-reap`（基线＝合并后 main `b85c4ee`）。构建与测试均在隔离 target `/tmp/038-attack06-target`（显式 `CARGO_TARGET_DIR`）。门禁工具＝原口径 `/cache/recovery-038-20261108/tools/run.py`（500 上限、pids.current 计线程、GATE start/peak 行、exit 透传）。

## 根因实证链（全部本容器 /proc 实测）

1. **复现**：单次 `pid_namespace_isolated_real`（1 次真实启动）→ 全容器新增 1 僵尸，`NSpid: 9978 1`（ns-PID-1 reaper）、comm=bwrap、PPid=1（容器 PID 1 为 `sleep infinity`，不收割）。
2. **裸 bwrap 复现**（产品旗标组合）:存活期观测到监督进程（宿主直接子进程）与 reaper（`--new-session` 自成会话）；载荷退出后**监督进程先被收割，reaper 随后退出→孤儿→永久僵尸**。每次真实启动（含 execvp 失败的启动）各泄漏 1 个。
3. **源码对证**（bubblewrap v0.9.0 bubblewrap.c）：`monitor_child` 经 event_fd 取得载荷退出态即返回（主进程不等 reaper）；`do_init`（reaper）wait 循环至 ECHILD 才退出——两进程退出序天然错开，孤儿化是 bwrap 0.9.0 的固有形态，在收割型 init 环境不可见。
4. **多线程收养验证**：PR_SET_CHILD_SUBREAPER 后并行 3 启动，reaper 全部收养至进程（PPid=TGID），定向 waitpid 全部回收、零残留。

## 修复实现

- `sandbox.rs`：`ensure_child_subreaper()`（probe_bwrap_at 入口、进程级一次）＋ `reap_adopted_bwrap_reapers()`（签名三重精确：PPid=本进程＋comm=bwrap＋NSpid 末级=1；Z 态直接收割；S 态 10ms 步进有界等待约 1s；放弃时分两路——无子进程＝设置期孤儿 SIGKILL＋即收，有子进程＝登记跳过＋后续收割点单次巡检）。
- 收割点：探测干跑、016 monitor 线程终态、MCP close、apply_patch 沙箱等待、view_image 沙箱准备、JS worker 五处 try_wait、request_permissions 测试辅助（run_payload/run_payload_pty）。全部 `cfg(target_os = "linux")`。
- 回归测试：`sandbox_launch_leaks_no_bwrap_reaper_zombie`（隔离辅助进程模式，同 js 测试先例；父进程校验退出码＋标记行，防 --exact 零匹配伪绿）。

## 命令与结果（串行）

| 检查 | 命令 | 结果 |
|---|---|---|
| 回归（修复态） | `cargo test --test request_permissions_sandbox sandbox_launch_leaks_no_bwrap_reaper_zombie -- --nocapture` | ok，`REAPER-BASELINE-CLEAN 21->21`（多次复跑一致） |
| 回归（变异态） | 注释掉 prctl 登记后同命令 | FAILED：「沙箱启动新增了 bwrap reaper 僵尸：before=13 after=16」；还原后 ok |
| **验收门禁（修复后）** | `run.py 900 cargo test --test request_permissions_sandbox -- --test-threads=4` | `GATE start tasks=196 processes=89` → 16/16 通过（14.20s）→ `GATE peak tasks=223 processes=101; exit=0`；跑后僵尸 45→45、S 态卡死 bwrap 29→29、pids.current 196→196：**零累积** |
| 验收门禁（修复前对照） | 同命令（467c9aa 等价内容＋本修复前） | 16/16 通过（10.40s）、`GATE peak tasks=176; exit=0`，但跑后 +9 reaper 僵尸 +5 host sleep 僵尸（净 +14） |
| 负向对照 | 故意反转 `pid_namespace_isolated_real` 断言后同门禁 | FAILED＋`GATE peak tasks=225 processes=96; exit=101`（真失败不被洗绿）；还原后通过 |
| lib 全量 | `cargo test --lib` | 304 passed / 0 failed / 1 ignored（本分支无 039 锚测试，数目与基线一致） |
| 静态检查 | `cargo clippy --all-targets`（组件下载失败）→ 兜底 `cargo check --all-targets` | Finished，无新增错误（警告为存量基线） |
| 受影响路径套件 | `cargo test --test mcp_stdio --test mcp_stdio_sandbox --test apply_patch_sandbox` | mcp_stdio 35/0、mcp_stdio_sandbox 2/0、apply_patch_sandbox 10/0（真实沙箱启动路径收割点全 exercised） |
| view_image 路径 | `request_permissions_sandbox::view_image_sandboxed_read_boundary`（验收套件内） | ok |

## 追记（2026-11-09）：完整路径各阶段真实结果

- **comprehensive-testing**：审计发现签名所有权负向保护缺口 → 新增 `reap_adopted_bwrap_reapers_leaves_non_bwrap_children_alone`（提交 4994684）；敏感性证明：comm 过滤单变异被 NSpid 第二重防线挡住（防御深度正常）、comm+NSpid 双变异 FAILED「收割误收了非 bwrap 子进程（wait 责任被偷走）」→ 还原回绿。拒绝项：并行竞争单测（并行套件真实覆盖）、S 态路径单测（构造高成本、真实绿跑已 exercise）。lib 305/0/1 连续三轮。
- **Security Testing（进入，依据：改动触碰沙箱进程收割/权限边界）**：独立 Security-Testing-Gate 攻防全套——五条不变式实弹验证通过（107 伪造 comm 样本零误触、SIGKILL 只及空 ns、攻击者面封死、subreaper 语义零改动、无新失败面）；既有安全回归 attack 7/7＋seccomp 族全绿；1 建议（F1 pidfd TOCTOU 加固，与 D2 sweep 同款模式可闭合）＋3 记录；结论「通过（可合并）」。报告：docs/security/reports/2026-11-09-040-reaper-reap-security.md。
- **ce-simplify-code**（5 reviewer 并行）：DRAIN-TLM 遗留遥测删除、scan 复用既有 helper＋Vec 返回、stuck 分类锁收窄、JS 冗余外层收割删除（后被 code-review 推翻恢复）、wait_worker_supervisor 补缺失 cfg（顺带修复非 Linux 编译）、6 处复述注释删除；skipped：stat 单次读（效率最优）、reaper_zombie_baseline 独立 oracle（变异证明依赖）。提交 6796612。
- **ce-code-review**（6 reviewer：correctness/standards/testing/security/reliability/adversarial；cross-model peer 因宿主 family 无法 attest＋acpx 缺席不可启动，按规则 in-process adversarial 兜底）：三项本轮引入缺陷修复（提交 85afe37）——①probe_bwrap_at 补 cfg(linux)（P1：macOS 编译断裂）；②回归测试 bwrap 缺失环境父分支先行跳过（P2：macOS CI 腿保护）；③恢复 JS 两处错误路径收割（P2/75 三 reviewer 一致：helper 内部收割不覆盖其超时/轮询错误返回，外层 try_wait 是首次收割点；spec「五处」与代码重新一致）。残留风险记录：F1 pidfd 加固方向、进程退出无最终收割点（泄漏从每启动 1 收敛为每带在飞沙箱退出至多 1）、MCP close 同步 drain 有界延迟、oracle 并行盲区（变异证据为单测模式）。run 目录：/tmp/compound-engineering-1000/ce-code-review/20261109-051723-8466e93c/（6 份 reviewer 工件＋review.json 结构）。
- **ce-compound**：跳过——040 全部非显然经验（bwrap 0.9.0 退出序与源码证据、subreaper 收养语义、收割签名设计、libtest --exact 零匹配伪绿陷阱）已承载于本规格 §1/§2、验证记录根因实证链、代码注释；无「代码和测试无法保留」的剩余经验。
- **全量验证（500 门禁复跑）：环境阻断，未完成。** 时间线：workspace 构建未限流触顶 512 → `-j 4` 单命令仍触顶 517（rustc 内部线程不受 -j 控制）→ 按 U4 配方两阶段分离：构建 `cargo test --workspace --jobs 2 --no-run` **过门禁**（GATE peak tasks=458; exit=0，全缓存命中）→ 执行阶段触顶 501（差 1 任务）→ 分目标 proc_session 触顶（起点已涨至 481）。根因：前两次触顶后 run.py 组杀构建进程树，残留 ~250 个 rustc/cargo 僵尸（PPid=1，容器 init 不回收，容器内不可回收）推高 pids 基线至 456-481，500 门禁物理不可满足。此为组杀次生污染，非 040 泄漏面（040 验收跑后僵尸/卡死零增量的证据见上文门禁表）。解除条件：宿主侧 `docker compose -f .devcontainer/compose.yaml restart`（僵尸清零、基线回落）后按本记录配方重跑。替代证据链：85afe37 全部受影响路径定向验证绿（lib 304×2、回归 16->16、负向签名、seccomp、js sandbox_worker、mcp_stdio 35/0、apply_patch_sandbox 10/0、attack06）；合并后 main push 触发 CI ubuntu 腿全量（cargo test --workspace）作为远端全量证据。

## 范围外发现（非本分支引入，登记待归属）

- `js_orchestration::worker_killed_ipc_dead_fails_cell` 在当前容器确定性失败（「WORKER_REAP_FAILED：bwrap 被外力终止」覆盖预期的「IPC 断开」）。**在基线 `b85c4ee`（无本分支任何改动）上逐字节相同地失败**；同日早间的 U4 记录（gate-b4jsperf/c9_js/u4-js-rerun/u4-batchB.log）同代码全绿。判定：存量时序敏感问题（kill_worker 对外力击杀主进程返回 Err→cleanup_error 覆盖终态错误），受容器当日运行历史影响翻转，与 040 无因果。归属 038/js 路径，不在本规格处理。

## 调查过程中的关键中间观测（修复演化依据）

- 修复 v1（仅 subreaper＋1s 有界轮询）：绿跑泄漏 +9 reaper 僵尸 +5 sleep 僵尸；遥测版套件 241s（卡死候选使每个收割点耗尽等待窗口，194 次 GIVE-UP）。
- 28 个 S 态卡死 reaper 的 /proc 解剖：无子进程、阻塞于 read(eventfd)（bwrap child_wait_fd/privsep 同步）、持有 tchat-031-seccomp memfd——主进程在 clone 与放行写之间死亡（强杀竞态/设置期瞬态错误）的孤儿，**收割不可达**，只能击杀（v3）。
- 容器存量僵尸/卡死（本轮调查累积，PPid=1）只能经容器重启清零；验收只计增量。

## 边界声明

- 沙箱安全语义未变：seccomp 清单、clone 掩码、bwrap argv、namespace 拓扑、D2 sweep（仍跳过 reaper PID 1）、016 终止升级路径全部未动；subreaper 只改变孤儿收养归属，收割只作用于「主进程已死的 reaper」。
- 测试可观测性未变：marker 扫描基于 /proc cmdline（僵尸无 cmdline）；e_reaping 族断言面核对不受影响；测试断言无删减（负向对照用后即还原）。
- 无外部 subreaper 包装：收割逻辑在产品启动路径内。
- 未顺修：其他测试文件自身的直接 bwrap 启动（e2e_sandbox、sandbox_matrix_probes 等）未加收割点（泄漏形态与修复前等价）；普通模式会话在宿主进程退出时的在飞子进程孤儿化（016 既有退出清理语义范围内）——均登记为后续增量点，不在本规格范围。
