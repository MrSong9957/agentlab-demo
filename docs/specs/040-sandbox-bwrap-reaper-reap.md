# 规格：沙箱 bwrap ns-PID-1 reaper 僵尸根治（产品内收割）

- 状态：**已实施待验收（2026-11-08）**
- 创建：2026-11-08。来源：038 U4 500 门禁环境调查——权限沙箱每次真实启动泄漏 1 个不可回收僵尸，用户立项根治（候选与基线该路径逐字节一致，属存量；此前的容器重启与外部 subreaper 包装均为缓解，不算修复）。
- 关联：`docs/specs/031-request-permissions.md`（权限沙箱启动面）、`docs/specs/033-tool-sandbox.md`（工具沙箱启动面）、`docs/specs/038-github-actions-ci.md`（§0-A 启动闸拓扑：bwrap 内建 reaper 为 ns-PID 1）。

## 1. 根因（2026-11-08 实证，证据见验证记录）

- bwrap 0.9.0 监督进程（宿主直接子进程）在载荷 stdio 管道 EOF 后即退出，**不等自己的 namespace PID 1 reaper**（`--new-session` 后 reaper 自成会话；实测进程树：监督进程先死并被调用方收割，reaper 随后退出）。
- reaper 孤儿化后被最近祖先 subreaper 或容器 init 收养；本容器 PID 1 为 `sleep infinity`，不收割 → **每次真实 bwrap 启动累积 1 个永久僵尸**（NSpid 双级 `host 1`、comm=bwrap）。
- 影响面：031 权限沙箱、033 工具沙箱、MCP stdio、JS worker、view_image、bwrap 探测干跑——全部共用 bwrap 启动拓扑。request_permissions 全套件真实启动数十次＋p16 计量场景 → cgroup pids.current 只增不减 → 500 门禁不可满足（U4 曾因此阻断，另叠加容器历史僵尸存量）。
- 危害边界：泄漏只发生在 init 不收割的环境（本容器、嵌入式 init）；systemd 类 init 环境只表现为 init 代收，泄漏本身仍在。

## 2. 修复设计（用户候选 A：改启动路径的子进程收割逻辑）

- 产品进程启动首个沙箱面时登记 `PR_SET_CHILD_SUBREAPER`（`probe_bwrap_at` 入口，进程级一次；失败静默退回现状）。只改变「无父进程者」的收养归属，不影响任何在世子进程的 wait 语义。
- 新增 `sandbox::reap_adopted_bwrap_reapers()`：扫描 /proc 中签名进程（PPid=本进程＋comm=bwrap＋NSpid 末级=1），定向 `waitpid(WNOHANG)` 回收。签名三重精确——bwrap 监督进程 NSpid 单级、载荷及后代 NSpid 末级≠1，均不匹配，绝不触碰其他线程在管子进程；并行收割点竞争同一目标时后到者 ECHILD 按已处理；S 态 reaper（退出在即）10ms 步进有界等待约 1s，超限放弃留给下一收割点。
- 收割点＝每个 bwrap 监督进程被收割之处：探测干跑后、016 monitor 线程终态回收后、MCP close 终态、apply_patch 沙箱执行等待后、view_image 沙箱准备等待后、JS worker 五处 try_wait 收割后、request_permissions 测试辅助（run_payload/run_payload_pty）。
- 全部收割代码 `cfg(target_os = "linux")`；Windows（作业对象树终止）/macOS 不适用也无此泄漏形态。

## 3. 边界

- **不改变沙箱安全语义**：seccomp 清单、clone 掩码、bwrap argv、namespace 拓扑、D2 sweep 与启动闸全部不动；subreaper 只影响孤儿收养归属。
- **不改变测试可观测性契约**：marker 扫描基于 /proc cmdline（僵尸无 cmdline，天然不受影响）；e_reaping 等遗留断言面已核对不受收养归属影响。
- **不是外部包装**：不用 run.py 层/辅助进程 subreaper 包装掩盖泄漏（038 曾用，用户裁定不算修复）；收割逻辑在产品启动路径内，对一切调用方生效。
- **不抬门禁、不删测试**：500 门禁口径原样执行。
- 不顺修：其他测试文件自身的直接 bwrap 启动（e2e_sandbox、sandbox_matrix_probes 等）不在本轮收割点覆盖内（其泄漏形态与修复前等价，未恶化）；登记为后续可增量接入点。

## 4. 验收（用户口径）

1. request_permissions 全套件（含 p16）在 run.py 500 门禁（原口径，容器 pids 计线程）下 exit 0，GATE peak <500。
2. 跑完后 pids 基线不累积增长（跑前/跑后 pids.current 与全容器僵尸数对照）。
3. 回归测试 `sandbox_launch_leaks_no_bwrap_reaper_zombie`：隔离辅助进程 3 次真实启动，reaper 僵尸基线零新增；变异证明——禁用 subreaper 登记后同一测试失败（before=13 after=16），还原后通过。
4. 负向对照：故意注入断言失败后，同一门禁命令必须非零退出（门禁不洗绿真失败）。
5. 不回归：lib 全量、attack06、seccomp 族测试通过。

## 5. 预测 → 实测（2026-11-08 已回填）

- 回归测试（修复态）→ 预期零新增 → 实测：**符合**——`REAPER-BASELINE-CLEAN 21->21`（隔离辅助进程 3 次真实启动，reaper 僵尸基线零新增，多次复跑一致）。
- 回归测试（变异态）→ 预期 +N 失败 → 实测：**符合**——禁用 subreaper 登记后同测试 FAILED：「沙箱启动新增了 bwrap reaper 僵尸：before=13 after=16」（3 次启动泄漏 3 个，PPid=1）；还原后 ok。
- 全套件 500 门禁 → 预期 exit 0、peak <500、基线不累积 → 实测：**符合**——`GATE start tasks=196 processes=89` → 16/16 通过（14.20s）→ `GATE peak tasks=223 processes=101; exit=0`；跑后全容器僵尸 45→45、S 态卡死 bwrap 29→29、pids.current 196→196——**零累积**。修复前同套件实测：16/16 通过但泄漏 +9 reaper 僵尸 +5 host sleep 僵尸（且收割循环被卡死候选拖慢至 241s，见验证记录）。
- 负向对照 → 预期非零退出 → 实测：**符合**——故意反转 `pid_namespace_isolated_real` 断言后同门禁命令 `test result: FAILED`、`GATE peak tasks=225 processes=96; exit=101`；还原后通过。

## 6. 调查补充：三类残余泄漏形态（实测发现与处置）

调查中发现并处置三类「每启动泄漏 1 僵尸」之外的残余形态（全部有 /proc 实证，详见验证记录）：

1. **设置期孤儿（S 态卡死，不可收割）**：主进程在 clone 后、child_wait_fd 写放行前死亡（强杀竞态或 bwrap setup 瞬态错误），其 ns-PID-1 子进程永远阻塞在 bwrap 内部同步原语上（实测 wchan/syscall：read(eventfd)），无子进程、永不退出。处置：收割放弃窗口内对**无子进程**候选 SIGKILL（空命名空间销毁，无附带进程）并随即收割；有子进程者＝合法等待中的 reaper，仅登记跳过重复等待。
2. **合法慢 reaper**：载荷树尚有成员时 reaper 保持 S 态（正常）。处置：登记后由后续收割点单次 WNOHANG 巡检，转 Z 即回收，不重复消耗等待窗口。
3. **测试进程退出级联（PDEATHSIG＋--die-with-parent）**：进程退出瞬间在飞 bwrap 主进程被杀，其设置期子进程即落入第 1 类。绿跑（无中途失败/杀进程）实测不产生此类新增；非绿跑的辅助进程强杀可复现，属退出时序固有形态，登记为已知边界。

容器存量（调查期间累积的卡死/僵尸，PPid=1）只能经容器重启清零，与产品无关；本规格验收只计跑前/跑后增量。
