# 后端自动回归

从容器内仓库根目录 `/workspace` 运行：

```bash
cargo check --locked --offline && cargo test --workspace --locked --offline --no-fail-fast
```

默认运行整个 workspace，不传名称过滤。Cargo 自动发现库单测和 `tests/` 集成目标，包含真实 CLI／PTY 验收；新增回归放入相应目标即可随全量运行，不维护第二份执行名单。依赖须已缓存并与 Cargo.lock 一致；模型服务使用本地固定响应，不需要真实 Key。

退出码 0 表示类型检查及全部已执行测试通过；非 0 表示失败。失败输出包含目标、用例名称、断言实际值与期望值。每个目标的 `test result` 分别给出 passed、failed、ignored、filtered out；ignored 不算通过，不能将编译失败后未运行的目标记作通过。`--no-fail-fast` 不会自动重试失败目标。

下表对应已确认的七项功能；一条测试可保护多个场景，已有充分覆盖直接复用。表中列的是关键入口，整条命令还会运行其余既有回归。

| 功能 | 正常结果与数据变化 | 边界／并发 | 异常／非法输入 |
|---|---|---|---|
| 凭证读取、授权与外发 | [e2e_acceptance](../../src/app/tests/e2e_acceptance.rs) 的 `s2a_pty_first_use_authorization_agree_and_persist` 走首次真实同意、实际请求及单条私有授权记录；[llm_flow](../../src/app/tests/llm_flow.rs) 检查实际 URL、Bearer、模型、回复与授权记录保留 | llm_flow 的 `bin_pipe_rotated_key_invalidates_grant`；[catalog 单测](../../src/app/src/catalog.rs) 检查 origin 规范化、信任根和凭证真实路径包含性；[session_cli](../../src/app/tests/session_cli.rs) 的 `e26_concurrent_grant_integrity` 检查并发授权快照无混配 | e2e 的首次拒绝零请求；llm_flow 的未授权／损坏授权零请求、`net_b01_all_redirect_statuses_rejected`、Key 控制字符和错误回显脱敏；session_cli 的 `e24_grant_placeholder_rename_failure`、`e25_grant_partial_write_two_states` 验证落盘失败不外发 |
| 工具执行权限与停止 | [exec_command](../../src/app/tests/exec_command.rs)：`command_computes_product_and_writes_exact_file` 真实计算 7×3，stdout 与文件均为 `21\n`；`min_timeout_accepted_and_expires_as_one_millisecond` 证明最小合法 `timeout_ms=1` 被接受并真实超时；[memory_phase2](../../src/app/tests/memory_phase2.rs) 检查合法记忆工具真实读写 | exec_command 的超时上限、`min_output_cap_truncates_at_codepoint_boundaries` 的 1/2/3 码点边界与超限排空、两路排空、独立进程组；memory_phase2 的 `dispatch_rejects_unregistered_tools_in_merge_mode` 核对同一个绝对目标零副作用 | exec_command 的非法参数／目录与非零退出码；取消后子孙不再写盘；[e2e_acceptance](../../src/app/tests/e2e_acceptance.rs) 的 `s72_overlapping_cancels_reclaim_and_late_results_stay_inert` 验证迟到结果不执行 |
| 文本补丁 | [apply_patch](../../src/app/tests/apply_patch.rs)：`add_update_delete_full_cycle` 比较真实文件、精确字节（9→10、新建 6 字节）与顺序清单，并核对无关文件不变 | 同文件的 1 MiB 补丁、4 MiB 文件／100000 行边界、`patch_b05_concurrent_add_single_winner`、已有目标与行尾处理 | `invalid_patches_error_without_touching_files`、`second_file_context_mismatch_preserves_first_and_skips_rest`、`second_file_write_failure_reports_partial_state` 分别保护解析零写入、已知失败保留前序且后续不执行、写入失败的不确定状态 |
| 会话保存与恢复 | [session](../../src/app/tests/session.rs)：`save_snapshot_writes_meta_and_records_atomically`、`snapshot_resume_then_new_auto_starts_with_full_context`；[session_cli](../../src/app/tests/session_cli.rs) 的 `e7_snapshot_to_auto_full_context_roundtrip` 比较完整历史与原快照 | session 的空会话、`line_limit_accepts_exact_and_skips_one_byte_over` 的 64 MiB 恰好／超 1 字节、文件锁；session_cli 的 `e9_writer_halfline_pause_degraded_readonly_zero_modification` 检查竞争方不改原文件 | session 的坏行修复、`half_write_truncated_back_then_catchup_exactly_once`、`save_failure_keeps_old_snapshot_intact`；session_cli 的 `e10_interrupted_side_effect_unknown_zero_replay` 检查恢复不重放 |
| 记忆更新、清理、重置与所有权 | [memory_lifecycle](../../src/app/tests/memory_lifecycle.rs) 的同源整链保护源版本、产物与下轮注入；[memory_phase1](../../src/app/tests/memory_phase1.rs) 的 `m3_a12_valid_output_persists_and_replaces` 检查结果替换；[memory_runner](../../src/app/tests/memory_runner.rs) 的 `m7_reset_requires_confirm_and_preserves_sessions` 检查数据字节和代际 | [memory_retention](../../src/app/tests/memory_retention.rs) 检查 30 天严格小于、14 天包含等号、selected／last_usage、201 条只删 200；[memory_store](../../src/app/tests/memory_store.rs) 与 memory_runner 检查租约及重置交错；memory_phase2 的 `a16_lease_blocks_and_allows_takeover` 核对租期内零模型请求；`memory::store::tests::migration_rechecks_after_concurrent_apply` 保护新库并发打开只迁移一次 | memory_runner 检查缺确认、链接根、旧任务回写；memory_retention 检查根外链接与删除失败；memory_phase2 的 `failed_merge_does_not_advance_success_watermark` 与 `partial_changes_not_rolled_back_on_validation_failure` 检查真实失败状态、不推进成功水位 |
| 上下文外发与记忆脱敏 | [context_injection](../../src/app/tests/context_injection.rs)：首轮精确请求、来源顺序、去重、`t15_skipped_sources_never_enter_requests_and_valid_rules_coexist_with_memory` 检查有效规则／摘要同请求和准确历史；[memory_phase1](../../src/app/tests/memory_phase1.rs) 检查过滤后真实用户文本保留 | context_injection 的 `t09_budget_truncation_utf8_boundary_and_exhaustion` 保护 32768 字节、UTF-8、恰好／超 1 字节与耗尽停止读取；[memory_inject](../../src/app/tests/memory_inject.rs) 检查总开关／use 关闭时不读取 | t15 检查链接和非法 UTF-8 正文不进入实际模型请求，异常提示不泄露正文；memory_phase1 的秘密输入、输出、错误持久化脱敏以及上下文剔除 |
| 模型调用限次、失败不重放、后台去重 | [tool_loop](../../src/app/tests/tool_loop.rs)：T2/T3/T4 检查真实文件、工具结果、串行顺序；T8 检查 500／断线两轮 3 请求、1 次非幂等追加且下一轮恢复 | T7 为 8 请求／8 次追加后停止；T14 为 8 请求／7 次真实追加后终文；[turn 单测](../../src/app/src/turn.rs) 检查 32／33 整批边界；[memory_candidates](../../src/app/tests/memory_candidates.rs) 的 `m2_config_claim_limits_bound_actual_jobs` 检查配置 0→1、2→2、999→128 的真实任务数及水位 | T8 覆盖 500 和请求后断线、不重发且可继续；T12 保护坏批次零执行；memory_candidates 检查非法配置、同版本去重、活动租约、失败退避和重试耗尽；memory_phase2 检查合并冷却 |

验收以这些真实结果为准：请求目的地与次数、模型请求内容、文件字节、进程停止、历史顺序、任务状态及水位。单独断言返回成功不足以证明功能正确。关键新增和加强断言的隔离错误注入结果、完整回归计数见 [本轮证据](../records/2026-10-21-backend-regression.md)。最终全量 exit 0：50 个目标、3045 通过、0 失败、3 既有忽略；`e2e_acceptance` 106/0/0。期间发现并修复记忆库并发打开时重复执行迁移的竞态（`src/app/src/memory/store.rs`），回归用例与 S81 多轮复跑记录见同一证据。

已批准边界保持：用户命令的 cwd 不是沙箱；补丁和文件／Git／SQLite 合并不承诺跨操作回滚；原始会话保留用户输入和工具输出；AGENTS 及同用户全局记忆可随请求外发；8 次是请求次数上限，不是金额预算。摘要注入入口的链接／整读信任边界仍为待核项，不编造拒绝要求或算作通过。真实供应商语义、Windows 宿主专项不由 Linux 离线回归证明。

后续每次新增或修改功能：先补能检查真实结果的回归，再运行上面的完整命令；交付时主动报告通过、失败、忽略与未运行情况，并指出失败用例和原因。名称过滤只用于定位，例如 `cargo test --locked --offline --test tool_loop t8_request_failure_no_retry_history_kept -- --exact`，不能作为交付结果。
