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
