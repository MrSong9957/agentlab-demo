# 安全攻防测试报告：032 上下文管理（get_context_remaining / new_context / V3 检查点）

- 日期：2026-10-28；被测对象：`src/app/src/context_window.rs`、`turn.rs`（批末屏障/分派）、`session.rs`（V3/提交回执）、`api.rs`/`api/native*`（M1 纯定型与只读计量）、`memory/rollout_input.rs`、宿主接线（`main.rs`/`tui_host.rs`）
- 规格：`docs/specs/032-context-management.md`（§1.2/§1.4/§1.5/§1.6/§1.7）；关联 025/027 凭证边界、013 记忆、018/021 输入通道
- 方法：先代码审查定位攻击面，再以真实代码路径实测（Rust 攻防用例 `tests/context_management.rs` 的 `sec_*` 4 项＋边界用例 C06/C07/C10/C12＋e2e s32 真实 CLI/PTY）；`env -u TCHAT_SANDBOX`；独立临时目录与假密钥 `SECRET-KEY-CM32*`
- 执行身份：uid 1000(dev)，容器 Linux

## 结论概要

8 类攻击矩阵（按门禁清单对本地 CLI 形态适配）全部实测。**勘误（2026-10-28 独立审查轮）**：本报告的攻防矩阵未覆盖「记忆提炼混合批」路径——独立代码审查确认混合批（真实调用＋批末 new_context）中的控制调用与 summary 曾泄漏进 013 提炼输入（P1，已按调用粒度过滤修复并补混合批回归）；V3 seed 内嵌图片载荷校验缺口（P1，已在装载层复用 019 §5.1 同规则修复并补负例）。两处修复后本报告结论恢复成立，但「核心防线全部有效」不应外推到本轮矩阵未覆盖的路径（013 边界已由审查轮补齐）。**核心防线（本矩阵覆盖范围内）全部有效**：参数严格白名单（未知字段/权限/路径/身份/额度字段全拒）、摘要恒为 assistant 资料＋固定标识（角色不可提升）、检查点只能由真实提交回执进入历史（伪造/坏关联/批中位置/V1V2 走私全拒）、失败候选不复活、内部轮双层隔离、密钥不进入计量/检查点/查询结果、8/32/一次切换额度保持、体积边界（64 KiB/128 KiB/10 MiB/64 MiB 行）全部生效。**未发现高危或中危问题**；2 项低危观察（见 O-1/O-2）。

## 攻击矩阵与实测

| # | 类别（门禁） | 攻击手法（032 适配） | 实测防线 | 结果 |
|---|---|---|---|---|
| 1 | 数据越权 | 外部会话文件伪造检查点（V1/V2 走私、坏代数、坏关联、批中位置、截断行） | `m3_v1_v2…`、`ct_checkpoint_bad_link…`、`m2_context_id…`：V1/V2 整拒；V3 语义校验拒绝并明确报错；截断按坏行计数 | 拦截 ✓ |
| 2 | 功能越权 | 内部记忆合并轮误传上下文宿主/伪造调用 | `sec_memory_merge_cannot_use_context_tools`：不声明＋不执行（双层隔离） | 拦截 ✓ |
| 3 | 身份认证（适配：宿主资格） | 无检查点宿主/未声明模型调 new_context；无连接快照调查询 | `m2_unregistered…`、dispatch 逐项"不可用" | 拦截 ✓ |
| 4 | 偷改字段提权 | summary 附带 `granted`/`path`/`reset_quota`/`role:"system"`；摘要正文自称系统指令索权 | `sec_summary_privilege_escalation_attempts`：未知字段全拒；seed 层恒 assistant＋`[模型生成的交接摘要…]` 前缀 | 拦截 ✓ |
| 5 | 恶意输入注入 | 64 KiB+1 summary、128 KiB+ 参数、空/null/非对象、批内多处/非批末 new_context、图片与 opaque 冒充可计量 | `switch_arguments_shape_and_limits`（边界两口径恰好允许/超过拒绝）、`m4_batch_shape…`（整批零副作用）、C04 unknown 分支不伪造数字 | 拦截 ✓ |
| 6 | 绕过协议层 | 未闭合前缀直接发送；scheduled 冒 applied；检查点失败后经补写/退出 flush/手动保存复活 | M1 未闭合前缀仅计量不发送（mock 零请求）；applied 仅由提交回执产生；`m3_commit_over_limit…`＋`m4_commit_failure…`：失败候选不出现在任何后续写路径 | 拦截 ✓ |
| 7 | 敏感信息泄露 | 密钥进入计量 body/查询结果/检查点 seed；Base64 图片载荷冒充 token；摘要泄露进程越权信息 | `sec_no_credential_leakage…`（密钥零命中）；C04 图片→unknown；运行态说明只列真实句柄与状态（已退出不称运行中） | 拦截 ✓ |
| 8 | 资源滥用 | 同轮重复切换、第 9 次请求、切换重置额度、检查点行超限、大批量刷查询 | 每轮一次成功切换；`m4_eighth…`（8 请求封顶，变异反证 3）；64 MiB 行限拒绝；查询零额外请求且不随归档线性恶化（C17 冻结门禁） | 拦截 ✓ |

## 变异反证（C18，与综合测试共用证据）

4 项定向突变全部被关键用例捕获后还原复绿：视图选择（回退全历史→"归档前缀不得重发"红）、提交回执（失败也切换→"未应用通知"红）、请求计数（排定即重置额度→"不发第 9 次请求 9≠8"红）、记忆版本门（拒 V3→"V3 可读"红）。

## 低危观察（不构成漏洞，登记备查）

- **O-1【低】摘要与运行态说明进入会话文件属预期数据面**：检查点文件含模型摘要与进程命令行（与既有历史记录同一暴露面——016 起命令行已在 transcript 中；会话文件 0600 权限、独立用户目录）。未发现跨用户放大；维持既有凭证边界（密钥从不落盘）。
- **O-2【低】`{ }`（带空白空对象）通过查询参数校验**：serde 归一化后与 `{}` 等价、零字段差异，无注入面；与"恰为空对象"的字面差异不构成攻击路径。已以用例固定该行为（`query_arguments_must_be_exactly_empty_object`）。

## 报告与证据

- 本报告：`docs/security/reports/2026-10-28-032-context-management-security.md`
- 攻防与边界用例：`src/app/tests/context_management.rs`（`sec_*`、`ct_*`、`m2–m5` 系列）
- 变异反证记录：`docs/logs/032-context-management/2026-10-28-mutation-evidence.md`
