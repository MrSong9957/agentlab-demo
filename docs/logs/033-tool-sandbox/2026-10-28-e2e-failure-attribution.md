# 033 部署态 e2e_acceptance 三失败归因（2026-10-28，只归因与记录）

对象：默认沙箱注入态（ambient `TCHAT_SANDBOX=1`）下 `n2b_esc_cancel_keeps_process_
cli_alive_cleans_on_exit`、`n2b2_mixed_batch_cancel_per_tool_result_shapes`、
`s103_mcp_reload_disables_and_exit_reaps_process` 三例失败。env -u 验收口径 143/143
全绿（同代码），deploy 记录已述。本轮：读测试与生产链路 → 真实 CLI＋本地 mock
行为级复现（`2026-10-28-e2e-attr-drive.py`，输出 `…-attr-a.log`/`…-attr-b.log`）→
pid/信号级取证 → 新旧二进制四变体对照（新=正式路径 `4ab2dcc1…`；旧=回滚点
`6430d84c…` 直接执行，未替换正式路径）。**不做修复、不改断言、不提交。**

## 0. 根因机制（三项中前两项的共同根因，证据见 §2）

**`--die-with-parent` 的 PDEATHSIG 锚定的是「创建者内核线程」。** 033 沙箱 argv 含
`--die-with-parent`；TUI 每个用户轮在**独立的 turn 线程**内执行工具（tui_host.rs:249
克隆管理器后 spawn 轮线程 → :286 传 `&HOST_CANCEL` → turn.rs `execute_tool` →
`procs.start` 在**该轮线程**内 spawn bwrap）→ 轮结束＝turn 线程退出 → 内核按
PDEATHSIG 给 bwrap 发 SIGKILL → bwrap 死亡又触发 payload（ns 内 pid1，其 PDEATHSIG
锚定 bwrap）被 SIGKILL → 整树清空。取证事实：①ESC 后 ~60ms 内**仅 bwrap 消失**、
payload sh 变 **Z（zombie，PPid=1）**——容器 PID1 不收割孤儿（既有记载）；②trap
INT/TERM/HUP 全部无记录＝不可捕获信号（SIGKILL）＝组杀/PDEATHSIG 特征；③库级对照
探针（`ProcessManager::start` 直连，沙箱成立性由载荷 `MYPID=1` 证实）：**cancel 返回
不杀**（e_reaping 既有）、**yield 返回不杀**（写盘 98→120 继续）——库无辜；④普通模式
（无 bwrap，无 PDEATHSIG）全部存活 ✓。四个既有测试盲区由此解释：探针线程长寿、
e2e 016 用例从未在沙箱注入态运行。

**判定：033 引入的既有缺陷**——旧二进制（pre-b1b2，同条件）复现完全一致（§2），
与 B1/B2（pre_exec fd 标记＋nlink/stat 检查）无关。

## 1. n2b_esc_cancel_keeps_process_cli_alive_cleans_on_exit

- **失败断言**：e2e_acceptance.rs:2316 `assert!(s2 > s1, "016：取消当前轮不终止已
  启动进程，写盘继续")`（:2346「退出后清理」未到达）。
- **复现（真实 CLI＋mock，行为级）**：沙箱=1：取消反馈「错误：」「仍由会话管理」
  正常渲染、批内第二调用不执行、无自动续请求、下一轮正常、退出码 0——**但取消后
  0.4s 写盘停止**（时间线：ESC 前 6B → +100ms 10B → 此后恒 10B；bwrap +60ms 消失、
  sh 成僵尸）。env -u：写盘 12→26 继续、写入进程存活、退出后停止且无遗留 ✓。
- **契约判断**：**（b）沙箱模式违反用户可见契约**——016「取消当前轮不终止已启动
  进程」被违反；且取消 UI 宣称「仍由会话管理」而句柄指向的进程已死（续接语义悬空，
  构成二次误导）。取消反馈、批内跳过、不续请求、下一轮、退出清理等其余契约面在
  沙箱模式全部满足。
- **最小修改建议（供决策，非本轮实施）**：机制修复需让「spawn bwrap 的线程」与
  「轮线程」解耦——①工具 spawn 移交长寿执行器线程（架构改动）；②评估移除/条件化
  `--die-with-parent`（弱化宿主异常退出时的清树保证；现行 `close_process_sessions`
  只覆盖正常退出路径——需规格权衡）；③引入长寿沙箱监督进程（上游 helper 同型）。
  属 016×033 规格冲突，需规格轮决策。测试侧在修复前可双分支化（沙箱分支按实际
  行为断言），但 (b) 类不应仅以双分支掩盖。

## 2. 新旧二进制四变体对照（场景 A，n2b 同款命令）

| 变体 | 取消后写盘继续 | 取消后树存活 | 其余契约面 |
|---|---|---|---|
| 新＋沙箱 | ✗（6→6） | ✗（bwrap+sh 亡） | 全部满足 |
| 新＋普通 | ✓（12→26） | ✓ | 全部满足 |
| **旧＋沙箱** | **✗（6→6）** | **✗（同款）** | 全部满足 |
| 旧＋普通 | ✓ | ✓ | 全部满足 |

「既有问题」证据成立：旧版（033 沙箱、无 B1/B2）同条件同款失败。完整矩阵
`2026-10-28-e2e-attr-a.log`。

## 3. n2b2_mixed_batch_cancel_per_tool_result_shapes

- **失败断言**：:2418（同款「写盘继续」）。混合批的其余断言（exec=cancelled＋句柄、
  apply_patch=ok=false＋未执行原因、跳过不呈现失败、历史结构）在失败点之后未到达。
- **复现与契约判断**：与 n2b 同机制同分类 **（b）**；混合批特有的断言面（apply_patch
  跳过呈现、per-tool 结果形状）在驱动观察中未见异常（取消反馈正常、目标文件未产生、
  无续请求）。归为 n2b 同因。

## 4. s103_mcp_reload_disables_and_exit_reaps_process

- **失败断言**：:9860 `wait_pid_gone(pid, 5s)`（「停用后旧 MCP 进程被回收」）。
- **复现（真实 CLI＋mock＋同一 fixture 服务器）**：沙箱=1：MCP 服务器经生产
  transport 启动正常（记录 startup）、reload 完成提示正常、停用后对话照常、退出
  成功——**且 reload 后外层实际进程确已回收**（cmdline 含 fixture 路径的进程扫描
  为空，含外层 bwrap）。断言失败的原因：服务器在沙箱内记录的 pid 是 **ns 内 pid=1**，
  测试用 `process_alive(1)`/`wait_pid_gone(1)` 探测——`/proc/1`（容器 init）恒存在，
  回收判定机制在沙箱下必然失效。env -u：记录宿主侧 pid，回收判定正常 ✓。旧＋沙箱
  同款失败 → 既有。
- **契约判断**：**（a）测试依赖普通模式的内部假设，用户契约仍满足**——「停用后旧
  MCP 进程被回收」实际发生（外层进程消失），失败纯属测试探测语义在沙箱下失效。
- **最小修改建议**：测试双分支化——沙箱分支改用**外层 argv 扫描**（cmdline 含
  fixture 路径的进程集合由非空变空，复用既有 `scan_procs`/`scan_leftovers` 模式）
  替代记录 pid 的存活检查；普通分支保持原断言。

## 5. 汇总分类

| 用例 | 分类 | 依据 |
|---|---|---|
| n2b | **（b）沙箱违反用户可见契约**（016 取消/跨轮保留语义）＋既有（033 引入） | §0 机制＋§2 矩阵 |
| n2b2 | 同 n2b（同断言同机制） | 同上 |
| s103 | **（a）测试内部假设依赖，用户契约满足**＋既有 | §4 外层回收证据 |

边界与证据：全程假凭据＋本地 mock；真实 .env 未读未动；未改部署配置、未停用户
实例；旧二进制仅从回滚路径直接执行（正式路径未替换，部署产物哈希未复核变动——
本轮零写操作触及）；临时归因探针文件已删除，工作树保持干净；未提交/未推送。
证据：`2026-10-28-e2e-attr-drive.py`、`…-attr-a.log`、`…-attr-b.log`（本目录）；
失败原貌：`2026-10-28-post-merge-suites.log`；口径对照：`…-e2e-acceptance-env-u.log`。
