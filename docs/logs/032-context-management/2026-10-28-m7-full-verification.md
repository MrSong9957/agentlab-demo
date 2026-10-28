# 032 M6/M7 最终验证记录

- 日期：2026-10-28；工作目录 `/workspace`；分支 `feat/032-context-management`（自 main `3fdc2e7e`，未提交）。
- 环境：容器内；`env -u TCHAT_SANDBOX`（解除沙箱环境泄漏——见 M0 基线记录与 lessons）；rustc/cargo 1.99.0（`--locked`）。

## 执行链与结果

| 环节 | 命令/方式 | 结果 |
|---|---|---|
| M0 定向基线 | 规格规定的 10 目标定向套件 | 210/210 通过（解除 TCHAT_SANDBOX 后） |
| M1–M5 实现＋TDD | `tests/context_management.rs` 逐里程碑红→绿 | 全绿 |
| comprehensive-testing | 缺口测试 6 项＋变异反证 4/4（红→还原绿） | 通过；证据 `2026-10-28-mutation-evidence.md` |
| Security Testing Gate | 8 类攻防矩阵真实路径用例＋报告 | 通过（2 项低危观察）；报告 `docs/security/reports/2026-10-28-032-context-management-security.md`（含审查轮勘误） |
| ce-code-review（独立审查） | 10 名未参与实现的审查者＋合并＋验证批（8/8 confirmed）＋报告 | **Ready with fixes**：25 项 actionable（5×P1/14×P2/6×P3） |
| 审查修复轮 | 25/25 全部修复（P1×5、P2×14、P3×6）＋新增回归 11 项 | 定向套件全绿 |
| C17 性能门禁 | `cargo test --locked --test context_perf` | 通过（59.5s，含装载时链校验；阈值不变） |
| e2e PTY | `cargo test --locked --test e2e_acceptance s32` | 通过（真实 CLI/PTY：查询/切换/检查点/恢复） |
| 全量验证 | `cargo check --locked && env -u TCHAT_SANDBOX cargo test --workspace --no-fail-fast` | check exit 0；测试 **3621 通过 / 1 失败 / 3 忽略**（73 目标） |

## 唯一失败的处理（不自动豁免，证据链判定）

`turn::tests::cancelled_batch_skips_per_tool_shapes_and_unknown_generic_error` 在并行满载下失败（turn.rs:2522 取消时序断言）。判定为基线已记录偶发而非本轮新增，依据：

1. 隔离复跑 3/3 通过（`cargo test --lib turn::tests::cancelled_batch_skips`）。
2. 同一测试在 **029 的最终全量**（`docs/logs/029-file-patch/2026-10-28-m6-final-full.log`，早于本轮全部改动）同样满载失败、同样隔离通过、同样判定（3569/1/3）。
3. 该用例依赖真实进程取消时序（apply_patch→exec 交替），与 032 改动面（上下文管理）无交集；032 未触碰其取消路径。

如实登记：本轮若在同等满载下重跑可能复现该偶发；其修复属既有用例的稳定性问题，不在 032 范围。

## 审查修复摘要（25 项全修复）

- P1：#3 混合批控制调用/summary 泄漏记忆提炼（按调用粒度过滤）＋回归；#4 V3 seed 内嵌图片绕过装载校验（019 §5.1 同规则）＋负例；#1 计划快照取最早（改取最新）＋多计划回归；#2/#10 工具描述补模型契约（批末位置/一次/未决阻塞/scheduled≠applied/快照边界/context_id）；#5 提交失败模型可见（宿主标识记录入史＋核对提示）。
- P2：#12 链级校验移至装载/提交点＋请求路径轻量本地校验（4 位审查者独立标记的线性恶化）；#13 迁移原子化（局部演练后替换，失败零副作用）；#9 超限旧窗口有界计量（LimitCountingWriter）；#11 编码失败与超限分支拆分（fail-closed）；#16 轮入口免整视图克隆；#15 候选校验单一入口；#24 关联须成功排定调用；#6 main.rs 回千行内；#7/#17/#18/#19 测试真断言与独立性；#14 部分拆分。
- P3：#8/#20/#21/#22/#25 死代码/常量/中位口径清理；#23 报错顺序固化断言。

## 残留（未测/接受项，验收人须知）

- 旧二进制实测拒绝 V3：由 meta 门（`matches!(1|2)`）与既有测试推断，无跨版本二进制实测。
- 断流批、MCP 连接活跃腿沿既有协议路径，未单列 032 用例。
- 真实模型对工具描述契约的遵循未实测（静态契约对比＋同仓先例）。
- C17 口径边界（Full 大窗口查询、切换路径计时）见 `2026-10-28-m0-perf-freeze.md` 修订段。
- 跨模型对抗审查未运行（acpx/node 基础设施缺失），对抗镜头由进程内审查者承担（报告 Coverage 已注明）。
