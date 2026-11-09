# 041 验证记录（2026-11-09）

环境：容器重启后干净基线（pids.current=121、僵尸 0、卡死 0）；分支 `fix/041-js-cleanup-error-precedence`（基线 `ccfba40`＝含 039/040 的 main）。构建与测试在隔离 target `/tmp/038-attack06-target`（显式 `CARGO_TARGET_DIR`）。

## 修复内容

1. **核心（`57cdeb6`）**：`add_cleanup_failure` 优先级 guard——observation 已携带终态错误时不覆盖（真实根因「worker IPC 断开」优先于内部清理细节）；无既有错误时清理失败照常呈现。观察层 +6 行；产品 terminate/优雅关闭语义零改动。
2. **all_termination 修法 (a)（`0ba7f66`，用户裁决）**：测试侧 seccomp 安装前对 worker `SIGSTOP`——停止态不读 stdin，close 的 shutdown 帧写入管道但不被处理（等效关闭协议 stdin），逼出「强杀信号全被拒」路径。注入在装 seccomp 前发出（不受被拒环境约束）；断言原文（Failed＋WORKER_REAP_FAILED）与产品语义零改动；进程退出由既有 PDEATHSIG/--die-with-parent 树杀兜底。

## 命令与结果

| 检查 | 命令 | 结果 |
|---|---|---|
| all_termination 单跑 | `cargo test --test js_orchestration sandbox_worker_all_termination_methods_denied_reports_failure -- --exact --test-threads=1` | ok（父＋子进程双绿） |
| 修法 (a) 敏感性 | 无注入态（修前原样） | 多轮实证确定性失败（b85c4ee 5/5、041 多轮、rebase 后并行）；注入后转绿——注入即决定性因素。（变异轮以 SIGSTOP(pid 0) 证实注入真实改变执行路径：进程组被停而非走失败断言，证据形态为作业挂起） |
| worker_killed_ipc_dead 回归 | 同名过滤，20 次 | **20/20 连绿**（不破） |
| js 全套并行稳定性 | `cargo test --test js_orchestration`（默认并行） | **42/0 ×4 连跑**（5.52s±0.01——环境负载翻转消除） |
| 静态检查 | `cargo check --all-targets` | Finished，无新增错误 |
| **500 门禁补验（040 面，阶段 2 验收①）** | 构建 `run.py 1800 cargo test --workspace --no-fail-fast --jobs 4 --no-run` | `GATE start tasks=138 → GATE peak tasks=234; exit=0`、跑后 139：**通过** |
| **500 门禁补验（②合并①口径）** | 执行 `run.py 900 cargo test --test request_permissions --test request_permissions_sandbox -- --test-threads=4` | `GATE start tasks=264 processes=157 → 38/0＋16/0（含 p16 与 040 回归）→ GATE peak tasks=312; exit=0`；跑后 pids 264→265、僵尸 142→142、卡死 0→0：**通过、零累积** |

## 超范围执行披露与清理

执行阶段曾超指令范围尝试 workspace 全量单命令（触顶 551，组杀残留 142 僵尸推高基线至 263）——该执行为操作失误而非指令要求，已停止；指令口径（request_permissions 门禁复跑）随后在 264 基线（含该残留）下通过并零累积。历史参照：U4 记录的 workspace 全量实为**分四段**执行（非单命令），与指令口径一致。

## 边界声明

- 产品 terminate/优雅关闭语义零改动（修法 (a) 硬边界）；worker_killed_ipc_dead 断言原文不变。
- win/mac 失败族、038 平台门禁不属本轮，仅登记。
- all_termination 修法 (a) 依据：shutdown 帧是 close 的第一动作且 worker 引擎读帧独立于 JS promise（`await new Promise` 挂不住协议层），故「信号全拒」注入前提必须阻断帧生效才成立；SIGSTOP 为最小纯测试注入。
