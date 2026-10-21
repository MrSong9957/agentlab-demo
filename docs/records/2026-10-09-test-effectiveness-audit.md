# 测试有效性审计与变异验证（2026-10-09）

方法：独立只读子代理逐项审计全部测试 → 隔离副本（/tmp/audit-ws，含未提交候选的完整拷贝）串行变异 → 幸存变异确认与测试强化（红→绿）→ 独立 Review → 业务源码字节核对 → 全量回归。
基线：`cargo test --workspace --no-fail-fast`（/workspace，退出码 0）：单元 69＋apply_patch 31＋exec 9＋llm_flow 40＝**149 通过 / 0 失败 / 0 跳过**。
实际测试函数数：149（子代理清点 142＋display.rs 内嵌 7 由主 Agent 补审）。

## 一、逐项审计结论

格式：编号｜测试名｜验证什么｜判定。完整 142 条明细见子代理交付（下方附全文）；display.rs 补审 7 条：

- 143 csi_sequences_removed｜CSI 序列剥离（含被换行打断）｜有效（字节级全等，真实 sanitize_for_terminal）
- 144 osc_and_string_sequences_removed｜OSC(BEL/ST)/DCS/APC/PM/SOS 剥离、未终止吞到串尾｜有效
- 145 two_char_escapes_removed｜两字符转义（ESC c 复位整体剥离、ESC M 后正文保留）｜有效
- 146 dangerous_controls_removed｜DEL/C1 CSI/退格响铃垂直制表｜有效
- 147 normal_text_preserved｜常规文本（中文/emoji/制表）无损｜有效
- 148 bare_escape_dropped｜裸 ESC 丢弃｜有效
- 149 truncate_is_char_safe｜300 字符上限按字符截断｜有效

### 汇总判定
- **无效（空跑/绕过被测逻辑/只测 mock）：0**。mock 均处系统边界（HTTP 应答、交互确认回调、RLIMIT_FSIZE 故障注入），请求侧与断言依赖真实业务函数/磁盘/请求记录。
- **断言薄弱：0**（就地强化前）。
- **覆盖缺口 3 处**：
  1. `stale_tmp_does_not_block_save`（catalog 单元）：只断言 `load_auto().is_ok()`，未证明新内容落盘 → **本轮已强化**（见二）。
  2. `key_leak_scan_across_failures`（llm_flow）：5 场景条件收集，某场景意外成功时泄露检查空跑 → **本轮已强化**（见二）。
  3. `refresh_does_not_change_in_flight_request`（llm_flow）："快照固定"由测试自行构造（自 clone 参数传入 chat），main 侧快照捕获时点未覆盖。**剩余缺口**：管道 E2E 按行顺序处理 stdin，无法在会话中注入并发 /refresh；待 CLI 接线多轮/并发场景时补 E2E。
- 记录在案不改：`fingerprint_rules` 一条恒真子断言（hex 输出 contains 明文，恒 false）；`stdout_stderr_merged` 不断言双流顺序（到达顺序非确定，合理放宽）。

## 二、变异验证（隔离副本中，一次一个错误；执行数=该过滤器命中并执行的数量）

| 编号 | 变异（源码差异） | 对应测试 | 执行/失败 | 失败断言 |
|---|---|---|---|---|
| V1 | apply_patch：Update 写入附加多余换行 | add_update_delete_full_cycle | 1/1 | 字节全等（mode=test\n\n ≠ 期望） |
| V2 | apply_patch：Delete 登记为 add | a2_delete_removes_link_only_target_kept | 1/1 | changed_files 全等（action） |
| V3 | apply_patch：校验型失败谎报 state_known=false | conflicting_targets_rejected_without_changes | 1/1 | assert!(r.state_known) |
| V4 | catalog：授权只按 origin 匹配忽略指纹 | origin_grant_store_semantics | 1/1 | "换 Key 必须重新授权" |
| V5 | exec：退出码恒报 0 | stdout_stderr_merged_exit_code_preserved | 1/1 | exit_code Some(7)≠Some(0) |
| V6 | api：chat 请求丢失 Authorization 头 | chat_sends_expected_json_and_parses_reply | 1/1 | auth 头全等 |
| V-S | catalog：write_atomic 静默不写盘 | （弱版）stale_tmp_does_not_block_save | 1/**0 通过** | —— 幸存 |

- 六类风险（文件内容/匹配、变更清单、失败状态、授权放行、命令执行结果、模型请求契约）全部由**真实结果断言**捕获，无编译失败或环境失败顶替。
- V-S 幸存确认（非等价、确在执行路径）：save_auto→write_atomic 被测试实际调用；变异后新内容不落盘（可观察结果改变）；弱版断言 `load_auto().is_ok()` 在文件缺失时经默认重建仍真。处置：强化测试（保存前插入标记模型 stale-tmp-marker，load 后断言 contains_key），随后同一变异下**红**（"新内容必须落盘：遗留临时文件不得使保存静默失效"），恢复业务后**绿**。
- `key_leak_scan_across_failures` 强化：新增 `assert_eq!(messages.len(), 5)`，封住条件收集空跑；正常实现下绿。
- 历史变异（前几轮，记录于各自日志）：M1 覆盖检查、M2 行尾、M3/M-A state_known、M-B 清单混入、M-C 去重、M-D 非交互放行、MA 链接跟随、MB 匹配轮次。本轮不重复执行。

## 三、Review 与恢复核对

- 独立子代理审查两处强化：均通过——marker 经 serde 全量 round-trip 不丢失；计数 5 结构上恒成立（五场景必 Err），无借测试改政策。
- 业务源码字节核对：`src/` 与轮初备份 diff，**唯一差异为 catalog.rs 内嵌测试函数 stale_tmp_does_not_block_save 的强化**（测试模块内），业务逻辑零改动；其余 src 文件字节一致。变异全部在隔离副本中执行并已恢复；/workspace 业务代码未被触碰（无 HEAD/reset/checkout）。
- 最终回归：见下。

## 四、证据分级与可检出错误类型（收窄表述，仅限实际验证过）

**分级口径**：
- **已逐项审计**（149/149）：142 项由独立子代理、7 项（display.rs）由另一独立子代理复核（见第八节），逐项核实入口/断言/mock 边界。
- **已做变异验证**（错误→断言失败→恢复，含本轮复验，见第八节）：Update 写入内容多写一个换行（V1）；Delete 动作登记错误（V2）；校验型失败 state_known 谎报（V3）；授权忽略凭证指纹误放行（V4）；退出码谎报 0（V5）；chat 请求丢失 Authorization 头（V6）；保存静默不写盘（V-S，强化后检出）。另有历史变异（各轮日志）：新增覆盖已有文件、插入行行尾统一 LF、写入失败仍称清单可信、失败目标混入清单、清单去重、非交互未授权放行、Update 拒绝链接、匹配改逐位置就近容差。
- **仅有正常用例覆盖**（审计判定有效、未做专属变异）：链接三动作语义（A1–A6）、匹配顺序细化（B1–B9 精确优先/最早命中/@@ 推进/EOF 锚定/哨兵重试——字节级对照断言）、同文件多块叠加（D1–D6）、EFBIG 半写前缀、JSON 边界、凭证不泄露、双流合并与进程组终止、分页/超限/重定向拒绝、目录身份绑定与名单替换等。这些错误类型**未逐个变异证明**，仅有审计+正常用例证据。
- **剩余缺口**：Q1–Q4 待决契约（链接/匹配 tie-break/清单路径规范化）；main 侧快照捕获时点（管道 stdin 顺序处理无法注入并发刷新）；真实 ENOSPC（EFBIG 已覆盖同一 write_all 中途错误路径）；`fingerprint_rules` 一条恒真子断言（记录在案未改）；双流合并顺序（到达顺序非确定，不断言）。
不声称能发现所有错误。

## 五、完整逐项清单（142＋7）

以下为子代理交付的 142 条（判定经主 Agent 抽查核实）＋ display.rs 补审 7 条已列于第一节。

| 编号 | 测试名 | 文件 | 验证什么 | 入口 | 检查 | mock | 判定 |
|---|---|---|---|---|---|---|---|
| 1 | sink_keeps_valid_text | src/exec.rs | TextSink 保留合法文本 | TextSink::push→finish | 输出与截断标志全等 | 无 | 有效 |
| 2 | sink_joins_multibyte_split_across_chunks | src/exec.rs | 跨块多字节拼接 | 同上 | "你🎉"全等 | 无 | 有效 |
| 3 | sink_replaces_invalid_bytes | src/exec.rs | 非法字节各替换一个 U+FFFD | 同上 | 逐字符全等 | 无 | 有效 |
| 4 | sink_cap_exact_not_truncated | src/exec.rs | 恰达上限不算截断 | 同上 | 全等+truncated=false | 无 | 有效 |
| 5 | sink_cap_truncates_and_stays_bounded | src/exec.rs | 超限截断且缓存有界 | 同上 | 内容+truncated+len 不变 | 无 | 有效 |
| 6 | sink_cap_cuts_inside_chunk | src/exec.rs | 块内按码点截断 | 同上 | "a🎉b"全等 | 无 | 有效 |
| 7 | sink_cap_reached_exactly_then_more | src/exec.rs | 填满后再来必标记 | 同上 | 内容+truncated | 无 | 有效 |
| 8 | sink_incomplete_tail_at_eof_replaced | src/exec.rs | EOF 未补全替换 U+FFFD | push→finish | 全等 | 无 | 有效 |
| 9 | sink_truncation_replacement_counts_one_char | src/exec.rs | 替换符占一码点额度 | 同上 | "a\uFFFD"全等 | 无 | 有效 |
| 10 | args_happy_path_with_all_fields | src/exec.rs | 四字段解析、null 缺省 | ExecArgs::from_value | 结构体全等 | 无 | 有效 |
| 11 | args_reject_non_object_and_unknown_field | src/exec.rs | 非对象/未知字段拒绝 | 同上 | Err 文案 | 无 | 有效 |
| 12 | args_reject_bad_cmd | src/exec.rs | cmd 缺失/空/非串拒绝 | 同上 | Err 含 cmd | 无 | 有效 |
| 13 | args_reject_bad_numbers_and_workdir | src/exec.rs | 数值与 workdir 校验 | from_value→positive_int | Err 含字段名 | 无 | 有效 |
| 14 | result_serializes_spec_shape | src/exec.rs | 五字段 JSON 契约 | serde 序列化 | to_value 全等 | 无 | 有效 |
| 15 | args_accept_patch_field | src/apply_patch.rs | patch 解析 | from_value | 全等 | 无 | 有效 |
| 16 | args_reject_bad_values | src/apply_patch.rs | 参数拒绝 | 同上 | 文案×5 | 无 | 有效 |
| 17 | result_serializes_spec_shape | src/apply_patch.rs | 四字段 JSON 契约 | serde | 两形全等 | 无 | 有效 |
| 18 | parse_full_patch_structure | src/apply_patch.rs | 三类块解析 | parse_patch 全链 | Hunk 全等 | 无 | 有效 |
| 19 | parse_update_without_at_at_starts_first_chunk | src/apply_patch.rs | 无 @@ 首块 | 同上 | chunks 全等 | 无 | 有效 |
| 20 | parse_context_and_empty_lines | src/apply_patch.rs | 上下文/空行入两侧 | 同上 | old/new 全等 | 无 | 有效 |
| 21 | parse_end_of_file_marker_sets_flag | src/apply_patch.rs | EOF 标志 | 同上 | 标志+old 空 | 无 | 有效 |
| 22 | parse_crlf_patch_text_matches_lf | src/apply_patch.rs | CRLF 补丁 | 同上 | content 全等 | 无 | 有效 |
| 23 | parse_marker_whitespace_is_lenient | src/apply_patch.rs | 块头宽容 | 同上 | Hunk 全等 | 无 | 有效 |
| 24 | parse_indented_update_marker_is_context | src/apply_patch.rs | 缩进块头是上下文 | 同上 | hunks==1 | 无 | 有效 |
| 25 | parse_empty_patch_yields_no_hunks | src/apply_patch.rs | 空补丁解析 | parse_patch | 空 Vec | 无 | 有效 |
| 26 | parse_rejects_invalid_patches | src/apply_patch.rs | 13 种格式错误 | 同上 | Err 文案 | 无 | 有效 |
| 27 | seek_exact_then_rstrip_then_trim | src/apply_patch.rs | 三级容差 | seek_sequence | 下标×4 | 无 | 有效 |
| 28 | seek_respects_start_index | src/apply_patch.rs | 起点约束 | 同上 | Some/None | 无 | 有效 |
| 29 | seek_eof_anchors_at_end | src/apply_patch.rs | EOF 锚定 | 同上 | Some(2)/Some(0) | 无 | 有效 |
| 30 | seek_pattern_longer_than_lines_is_none | src/apply_patch.rs | 模式过长/空模式 | 同上 | None/Some(5) | 无 | 有效 |
| 31 | replacements_retry_without_trailing_empty_sentinel | src/apply_patch.rs | 哨兵重试 | compute_replacements | reps 全等 | 无 | 有效 |
| 32 | replacements_split_around_context_lines | src/apply_patch.rs | 上下文切分 | 同上 | reps 全等 | 无 | 有效 |
| 33 | pure_insert_chunk_appends_at_end | src/apply_patch.rs | 纯插入追加 | 同上 | reps 全等 | 无 | 有效 |
| 34 | change_context_seek_advances_search_start | src/apply_patch.rs | @@ 推进起点 | 同上 | 双替换全等 | 无 | 有效 |
| 35 | source_file_preferred_ending_is_first_found | src/apply_patch.rs | 约定行尾 | SourceFile::parse | preferred×4 | 无 | 有效 |
| 36 | source_file_roundtrips_contents | src/apply_patch.rs | 拆行重组无损 | parse→into_contents | 6 例全等 | 无 | 有效 |
| 37 | apply_replacements_keep_endings_and_complete_trailing | src/apply_patch.rs | 行尾保持+补尾 | apply_replacements | 字节全等 | 无 | 有效 |
| 38 | file_credential_confined_to_credential_dir | src/catalog.rs | file: 凭证目录限制 | resolve_credential（真实软链/临时目录） | 拒绝文案+可读 | 无 | 有效 |
| 39 | private_fs_modes_enforced | src/catalog.rs | 0700/0600 模式 | ensure_private_dir 等 | 模式+内容 | 无 | 有效 |
| 40 | audit_tightens_and_reports_without_secrets | src/catalog.rs | 审计收紧不泄密 | audit_file_permissions | 模式/报告/告警 | 无 | 有效 |
| 41 | origin_grant_store_semantics | src/catalog.rs | 授权双匹配/fail-closed | is/grant/ensure_origin_authorized | 布尔×7+0600 | prompt 闭包（边界） | 有效 |
| 42 | origin_of_normalizes | src/catalog.rs | origin 归一化 | origin_of | 全等×4 | 无 | 有效 |
| 43 | route_checks_credential_and_disabled_model | src/catalog.rs | 路由检查 | merge_final→route | 快照/Err | 无 | 有效 |
| 44 | dotenv_loads_missing_vars_without_override | src/catalog.rs | .env 不覆盖 | load_dotenv_from | 计数/值 | 无 | 有效 |
| 45 | debug_output_redacts_key | src/catalog.rs | Debug 不泄密 | Debug impl | 不含秘密 | 无 | 有效 |
| 46 | route_rejects_remote_http_and_empty_base_url | src/catalog.rs | 强制 HTTPS | validate_base_url | Err | 无 | 有效 |
| 47 | unknown_manual_field_rejected | src/catalog.rs | 未知字段拒绝 | load_manual | Err+文件不变 | 无 | 有效 |
| 48 | manual_overrides_only_written_fields | src/catalog.rs | 字段级覆盖 | merge_final | 字段值 | 无 | 有效 |
| 49 | manual_model_overlay_add_and_restore | src/catalog.rs | 模型叠加 | merge_final→overlay | contains/值 | 无 | 有效 |
| 50 | fingerprint_rules | src/catalog.rs | 指纹规则 | fingerprint | 相等/不等/定长 | 无 | 有效（一条恒真子句记录在案） |
| 51 | credential_env_and_file | src/catalog.rs | env: 引用 | resolve_credential | 值/文案 | 无 | 有效 |
| 52 | validate_base_url_rules | src/catalog.rs | URL 规则 | validate_base_url | Ok/Err×10 | 无 | 有效 |
| 53 | credential_file_error_branches | src/catalog.rs | file: 错误分支 | 同上 | 文案×5 | 无 | 有效 |
| 54 | identity_fields_each_invalidate | src/catalog.rs | 身份四字段失效 | identity_of 过滤 | 控制组含/变体不含 | 无 | 有效 |
| 55 | explicit_false_and_zero_semantics | src/catalog.rs | 显式 false/0 | merge_final/route | 字段+Ok | 无 | 有效 |
| 56 | corrupt_files_reported_and_not_rewritten | src/catalog.rs | 损坏文件报错 | load_* | Err+字节不变 | 无 | 有效 |
| 57 | first_run_creates_default_files | src/catalog.rs | 首跑默认文件 | load/save | 默认项+存在 | 无 | 有效 |
| 58 | readonly_dir_write_fails_cleanly | src/catalog.rs | 只读目录失败干净 | save_auto→write_atomic | Err+无残留 | 无 | 有效 |
| 59 | stale_tmp_does_not_block_save | src/catalog.rs | 遗留 tmp 不阻塞 | 同上 | **本轮强化**：标记落盘+遗留保留 | 无 | 有效（强化后） |
| 60 | public_catalog_url_rules | src/api.rs | 公共目录 HTTPS | validate_public_catalog_url | Ok/Err×6 | 无 | 有效 |
| 61 | model_id_rules | src/api.rs | 模型 id 规则 | valid_model_id | 布尔×8 | 无 | 有效 |
| 62 | limited_reader_allows_exact_limit_and_stops_over | src/api.rs | 响应体上限 | LimitedReader::read | 长度/ErrKind | Cursor（读边界） | 有效 |
| 63 | invalid_patches_error_without_touching_files | tests/apply_patch.rs | 10 种非法补丁零副作用 | apply_patch 全链 | 四字段+字节不变 | 无 | 有效 |
| 64 | invalid_json_args_error_without_side_effects | tests/apply_patch.rs | JSON 边界 | apply_patch | 字段+文案+probe | 无 | 有效 |
| 65 | add_update_delete_full_cycle | tests/apply_patch.rs | 增改删全流程 | apply_patch→apply_hunk | 清单三元组+三文件 | 无 | 有效 |
| 66 | conflicting_targets_rejected_without_changes | tests/apply_patch.rs | 目标冲突拒绝 | 同上 | 字段+文案+字节 | 无 | 有效 |
| 67 | second_file_write_failure_reports_partial_state | tests/apply_patch.rs | 写入故障两阶段 | 全链+EFBIG 子进程 | JSON+字节+4096 前缀 | RLIMIT 子进程注入 | 有效 |
| 68 | line_endings_follow_spec_rules | tests/apply_patch.rs | 行尾六场景 | compute_new_content | 字节全等 | 无 | 有效 |
| 69 | relative_paths_resolve_against_session_cwd | tests/apply_patch.rs | 相对路径解析 | apply_patch（真实 cwd 切换） | 文件+清单 | 无 | 有效 |
| 70 | non_utf8_target_rejected_and_unicode_roundtrip | tests/apply_patch.rs | UTF-8 限制 | apply_hunk | Err+字节 | 无 | 有效 |
| 71 | multi_chunk_update_and_context_seek | tests/apply_patch.rs | 多 @@ 块 | compute_replacements | 字节全等 | 无 | 有效 |
| 72 | readonly_permissions_are_enforced_not_bypassed | tests/apply_patch.rs | 权限门 | apply_patch | 字段+存在性 | 无 | 有效 |
| 73 | a1_update_follows_symlink_writes_real_target | tests/apply_patch.rs | Update 跟随链接 | apply_hunk | 双侧字节+清单 | 无 | 有效 |
| 74 | a2_delete_removes_link_only_target_kept | tests/apply_patch.rs | Delete 删本体 | 同上 | 链接/目标/清单 | 无 | 有效 |
| 75 | a3_add_existing_symlink_rejected | tests/apply_patch.rs | Add 拒链接 | 同上 | Err+双侧+指向 | 无 | 有效 |
| 76 | a4_add_dangling_symlink_rejected_keeps_dangling | tests/apply_patch.rs | 悬空 Add 拒绝 | 同上 | Err+仍悬空 | 无 | 有效 |
| 77 | a5_update_dangling_symlink_target_missing | tests/apply_patch.rs | 悬空 Update 缺失 | 同上 | Err+未创建 | 无 | 有效 |
| 78 | a6_delete_dangling_symlink_cleans_link | tests/apply_patch.rs | 悬空 Delete 清理 | 同上 | 链接+清单 | 无 | 有效 |
| 79 | b1_exact_match_beats_earlier_trim_equivalent | tests/apply_patch.rs | 精确优先 | seek_sequence | 字节全等 | 无 | 有效 |
| 80 | b2_repeated_line_hits_first_occurrence | tests/apply_patch.rs | 最早命中 | 同上 | 字节 | 无 | 有效 |
| 81 | b3_change_context_disambiguates_duplicates | tests/apply_patch.rs | @@ 消歧 | 同上 | 字节 | 无 | 有效 |
| 82 | b4_trim_end_tolerance_whole_line_replacement | tests/apply_patch.rs | trim_end 级 | 同上 | 字节 | 无 | 有效 |
| 83 | b5_trim_both_sides_tolerance_only_match | tests/apply_patch.rs | trim 级 | 同上 | 字节 | 无 | 有效 |
| 84 | b6_end_of_file_anchor_pins_to_last_line | tests/apply_patch.rs | EOF 锚定对照 | 同上 | 双文件字节 | 无 | 有效 |
| 85 | b7_end_of_file_anchor_mismatch_fails | tests/apply_patch.rs | 锚定失败 | 同上 | Err+字节不变 | 无 | 有效 |
| 86 | b8_trailing_empty_sentinel_retry | tests/apply_patch.rs | 哨兵重试 | 同上 | 5 字节全等 | 无 | 有效 |
| 87 | b9_empty_file_pure_insert | tests/apply_patch.rs | 空文件插入 | 同上 | 字节+清单 | 无 | 有效 |
| 88 | d1_same_file_two_hunks_stack_sequentially | tests/apply_patch.rs | 顺序叠加 | run 循环 | 17 字节+清单两条 | 无 | 有效 |
| 89 | d2_same_file_second_hunk_validation_failure_keeps_first | tests/apply_patch.rs | 失败保留前块 | 同上 | Err+清单+字节 | 无 | 有效 |
| 90 | d3_same_file_three_hunk_chain | tests/apply_patch.rs | 三块链 | 同上 | v4+清单 3 | 无 | 有效 |
| 91 | d4_same_file_eof_anchor_after_append | tests/apply_patch.rs | 锚定×叠加复合 | 同上 | 字节+清单 2 | 无 | 有效 |
| 92 | d5_same_file_parse_error_first_hunk_not_executed | tests/apply_patch.rs | 解析先行 | parse_patch | Err+清单空+字节不变 | 无 | 有效 |
| 93 | d6_same_file_efbig_mid_write_keeps_prefix | tests/apply_patch.rs | 同文件半写 | 全链+子进程 | JSON+4096 前缀 | RLIMIT 子进程 | 有效 |
| 94 | invalid_inputs_error_without_side_effects | tests/exec_command.rs | 9 种非法输入 | exec_command | 五字段+probe | 无 | 有效 |
| 95 | workdir_resolves_relative_paths | tests/exec_command.rs | workdir 三态 | exec_command→spawn | 输出+退出码 | 真实进程 | 有效 |
| 96 | stdout_stderr_merged_exit_code_preserved | tests/exec_command.rs | 双流+退出码 | 同上 | status/code/两行 | 真实进程 | 有效（顺序不断言，合理放宽） |
| 97 | invalid_utf8_bytes_replaced_end_to_end | tests/exec_command.rs | 非法字节端到端 | TextSink | 全等 | 真实进程 | 有效 |
| 98 | timeout_kills_process_group_and_stops_writes | tests/exec_command.rs | 超时杀进程组 | run→kill_group | Timeout+s1==s2 | 真实孙进程 | 有效 |
| 99 | cancel_flag_stops_managed_processes | tests/exec_command.rs | 取消终止 | 同上 | Cancelled+s1==s2 | 真实线程/进程 | 有效 |
| 100 | oversized_output_truncated_to_char_cap | tests/exec_command.rs | 码点截断 | TextSink cap | 200 码点前缀+code0 | 真实进程 | 有效 |
| 101 | discard_after_cap_keeps_child_running_to_completion | tests/exec_command.rs | 超限继续读 | 同上 | Completed+200 | 真实进程 | 有效 |
| 102 | cd_inside_command_does_not_stick | tests/exec_command.rs | cd 不粘连 | exec_command×2 | 第二次 code0 | 真实进程 | 有效 |
| 103 | chat_sends_expected_json_and_parses_reply | tests/llm_flow.rs | 请求契约+解析 | api::chat | reply+1 次+头+体 | mock HTTP | 有效 |
| 104 | auth_failure_is_reported_clearly | tests/llm_flow.rs | 401 分类 | describe_status_error | Err 文案 | mock HTTP | 有效 |
| 105 | unsupported_protocol_rejected_before_request | tests/llm_flow.rs | 协议前置 | api::chat | Err+count0 | mock HTTP | 有效 |
| 106 | confirmed_unsupported_param_blocks_before_request | tests/llm_flow.rs | 参数能力门 | checked_params | 文案+count+body | mock HTTP | 有效 |
| 107 | list_models_reads_pagination_completely | tests/llm_flow.rs | 分页完整 | api::list_models | ids 顺序+字段+游标 | mock HTTP | 有效 |
| 108 | pagination_cursor_is_single_encoded_param | tests/llm_flow.rs | 游标编码 | Url 拼接 | 编码还原 | mock HTTP | 有效 |
| 109 | chat_body_over_limit_aborts | tests/llm_flow.rs | 11MiB 截断 | LimitedReader | Err 含 10 MiB | mock HTTP | 有效 |
| 110 | error_body_over_limit_reports_cap_with_status | tests/llm_flow.rs | 错误体上限+状态 | 同上 | Err 双要素 | mock HTTP | 有效 |
| 111 | json_error_message_capped_at_300_chars | tests/llm_flow.rs | 300 字符截断 | truncate_chars | X 计数 | mock HTTP | 有效 |
| 112 | refresh_success_replaces_old_list_and_keeps_selection | tests/llm_flow.rs | 刷新全流程 | catalog::refresh 全链 | report+identity+二次刷新 | mock×2 | 有效 |
| 113 | refresh_failure_after_key_change_excludes_old_list | tests/llm_flow.rs | 换 Key 排除旧名单 | 身份过滤 | failed+不含旧+含兜底 | mock HTTP | 有效 |
| 114 | refresh_with_valid_empty_list_clears_models | tests/llm_flow.rs | 空名单清空 | refresh | queried+空 | mock HTTP | 有效 |
| 115 | pagination_failure_keeps_old_list_intact | tests/llm_flow.rs | 分页中断保护 | list_models | count2+旧名单 | mock HTTP | 有效 |
| 116 | refresh_does_not_change_in_flight_request | tests/llm_flow.rs | 刷新不扰进行中请求 | chat∥refresh（真实线程） | reply+各 1 次 | mock×3 | 覆盖缺口（main 侧时点未覆盖，见第一节） |
| 117 | bin_pipe_session_chats_and_lists_models | tests/llm_flow.rs | bin E2E 全链（=C1） | 真实二进制 | stdout+请求+头+auth 不变 | mock+子进程 | 有效 |
| 118 | bin_pipe_unauthorized_origin_rejects_without_request | tests/llm_flow.rs | C2 未授权拒绝 | 真实二进制 | count0+文案+退出码 | mock+子进程 | 有效 |
| 119 | bin_pipe_rotated_key_invalidates_grant | tests/llm_flow.rs | C3 换 Key 失效 | 真实二进制 | count0+auth 不变 | mock+子进程 | 有效 |
| 120 | refresh_refuses_unauthorized_origin_without_request | tests/llm_flow.rs | F1 refresh 授权 | refresh 回调 Err | failed+count0 | mock+回调 | 有效 |
| 121 | redirect_targets_never_receive_credentials | tests/llm_flow.rs | 重定向拒绝 | api::chat send | Err+目标 0 请求 | mock×2 | 有效 |
| 122 | public_sync_requires_authorization_and_validates_fields | tests/llm_flow.rs | F4 目录授权+字段 | sync_public | public_error+入库过滤 | mock HTTP | 有效 |
| 123 | production_catalog_path_rejects_plaintext_http_even_with_stale_switch | tests/llm_flow.rs | 正式路径 HTTPS | sync_public 真实校验 | Err+不入库 | mock HTTP | 有效 |
| 124 | missing_id_fails_whole_list | tests/llm_flow.rs | 缺 id 整败 | list_models | failed+旧名单 | mock HTTP | 有效 |
| 125 | empty_page_with_has_more_fails | tests/llm_flow.rs | 空页异常 | 游标缺失 | failed+旧名单 | mock HTTP | 有效 |
| 126 | cursor_stall_fails_fast | tests/llm_flow.rs | 游标停滞 | 游标比对 | count2+旧名单 | mock HTTP | 有效 |
| 127 | key_leak_scan_across_failures | tests/llm_flow.rs | 泄漏扫描 | chat/resolve 错误分支 | **本轮强化**：len==5+不含明文/指纹 | mock×4 | 有效（强化后） |
| 128 | newline_key_rejected_without_panic | tests/llm_flow.rs | 换行 Key 拦截 | check_key_header_safe | Err+count0 | mock HTTP | 有效 |
| 129 | timeout_no_response_keeps_list | tests/llm_flow.rs | 超时保护 | send 短超时 | count1+旧名单 | mock（挂死） | 有效 |
| 130 | disconnect_after_request_no_retry | tests/llm_flow.rs | 无重试 | send | count==1 | mock（断开） | 有效 |
| 131 | multi_turn_config_change_next_round | tests/llm_flow.rs | 轮间配置生效 | build_final→chat×2 | 两 model 值 | mock HTTP | 有效 |
| 132 | reserved_params_rejected_before_request | tests/llm_flow.rs | 保留字段拒绝 | checked_params | Err+count0 | mock HTTP | 有效 |
| 133 | response_edge_branches | tests/llm_flow.rs | 空回复三分支 | chat 解析 | Err×3 | mock HTTP | 有效 |
| 134 | base_url_slash_and_v1_paths | tests/llm_flow.rs | URL 拼接 | chat | path×2 | mock HTTP | 有效 |
| 135 | map_key_authority_in_request | tests/llm_flow.rs | map 键权威 | merge→chat | model 值 | mock HTTP | 有效 |
| 136 | manual_vendor_full_custom_end_to_end | tests/llm_flow.rs | 自定义厂商 E2E | refresh→chat | queried+reply+body | mock HTTP | 有效 |
| 137 | public_missing_provider_keeps_base | tests/llm_flow.rs | 目录缺厂商 | sync_public | 三不变 | mock HTTP | 有效 |
| 138 | public_malformed_fields_no_panic | tests/llm_flow.rs | 字段容错 | 映射 | 三字段+极值 | mock HTTP | 有效 |
| 139 | public_unknown_providers_not_added | tests/llm_flow.rs | 未知厂商不入库 | sync_public | keys 全等 | mock HTTP | 有效 |
| 140 | public_cannot_touch_connection_fields | tests/llm_flow.rs | 目录不改连接 | sync_public | 三字段原样 | mock HTTP | 有效 |
| 141 | public_sync_failure_continues_query | tests/llm_flow.rs | 同步失败继续查询 | refresh | error+queried | mock HTTP | 有效 |
| 142 | concurrent_saves_last_writer_wins | tests/llm_flow.rs | 并发保存完整 | write_atomic×50 | 一方完整+无 tmp | 无（真实并发） | 有效 |
| 143–149 | display.rs 7 条 | src/display.rs | 转义剥离/文本保留/字符截断 | sanitize_for_terminal/truncate_chars | 字节全等 | 无 | 有效（见第一节明细） |

## 六、构建产物污染事故与处置（如实记录）

首次最终回归出现 3 处失败（lib 1：origin_grant_store_semantics「换 Key 必须重新授权」；llm_flow 2：chat/bin 请求 Authorization 头缺失），特征与 V4/V6 变异一致。排查：工作区源码经 diff 与模式检索确认**全程干净**（双匹配逻辑与 Authorization 头均在，与轮初仅差 catalog.rs 测试模块强化）。根因：本环境全局设置 `CARGO_TARGET_DIR=/workspace/target`，隔离副本 /tmp/audit-ws 的构建写入了**共享** target 目录，变异构建产物被工作区测试运行复用。处置：`cargo clean -p terminal-chat` 强制从干净源码全量重建 → 最终回归 **149 通过 / 0 失败 / 退出码 0**。教训：后续隔离副本必须显式设置副本本地 `CARGO_TARGET_DIR`。3 处失败为构建产物污染（环境/基建问题），非业务回归，未被掩盖：已根因分析、清理重建并以干净结果复核。

## 七、最终回归（干净重建后）

`cargo test --workspace --no-fail-fast`（/workspace）：单元 69＋apply_patch 31＋exec 9＋llm_flow 40＝**149 通过 / 0 失败 / 0 跳过 / 未运行 0**，退出码 **0**。

## 八、收尾轮（2026-10-09 第二次）：独立补审与变异证据重建

### 8.1 display.rs 独立补审（149/149 全部经独立审计）
独立子代理只读复核 display.rs 7 项：**维持全部有效**。要点：逐断言追踪到 skip_escape/is_dangerous_control/truncate_chars 真实分支（如"若只删 ESC 会得 a[2\nJb，断言能抓错"）；与安全报告 F3 宣称维度（序列逐类、载荷不残留、正文保留）相符。注记（不降级）：CSI 中间字节变体（ESC[!p）、SS3、串内 ESC 中断分支无直接用例，均为共享路径，记入"剩余缺口"观察项。至此 149 项＝142（首轮独立审计）＋7（本轮独立复核）全部有独立记录。

### 8.2 构建目录隔离核实（针对第三节污染事故的重建）
- 审计副本 /tmp/audit-ws2（含未提交候选拷贝），**全部 Cargo 调用显式 `--target-dir /tmp/audit-ws2-target`**。
- 产物落点核实：副本构建产物在 /tmp/audit-ws2-target/debug/deps；工作区 /workspace/target 各测试二进制 mtime 在整个复验前后**逐一比对无变化**。
- 根源备注：副本与工作区源内容相同导致测试二进制哈希相同（apply_patch-b1bda2c26f55e5de），共享 target 目录时相互覆盖——显式 --target-dir 后物理隔离。

### 8.3 七组变异复验（每轮三步：正常→单一变异→恢复；执行数=过滤器命中数）
| 组 | 目标测试 | 正常 | 变异（exit/断言） | 恢复 | 源码恢复核对 |
|---|---|---|---|---|---|
| V1 | add_update_delete_full_cycle | 1 过，exit=0 | 1 败，exit=101，字节全等断言（写入多换行） | 1 过，exit=0 | 字节一致 |
| V2 | a2_delete_removes_link_only_target_kept | 1 过，exit=0 | 1 败，exit=101，清单 action 全等 | 1 过，exit=0 | 字节一致 |
| V3 | conflicting_targets_rejected_without_changes | 1 过，exit=0 | 1 败，exit=101，assert!(r.state_known) | 1 过，exit=0 | 字节一致 |
| V4 | origin_grant_store_semantics | 1 过，exit=0 | 1 败，exit=101，「换 Key 必须重新授权」 | 1 过，exit=0 | 字节一致 |
| V5 | stdout_stderr_merged_exit_code_preserved | 1 过，exit=0 | 1 败，exit=101，exit_code Some(7)≠Some(0) | 1 过，exit=0 | 字节一致 |
| V6 | chat_sends_expected_json_and_parses_reply | 1 过，exit=0 | 1 败，exit=101，Authorization 头全等 | 1 过，exit=0 | 字节一致 |
| V-S | stale_tmp_does_not_block_save（强化版） | 1 过，exit=0 | 1 败，exit=101，「新内容必须落盘」 | 1 过，exit=0 | 字节一致 |

全部为业务断言失败（panic 于断言），无编译失败/环境失败/0 项执行；每轮恢复后副本源码与工作区 git diff --no-index 字节一致。

### 8.4 最终核对与回归
- 业务源码与本轮开始备份 diff：**字节一致**（003/004 既有改动保留）。
- 工作区最终回归：`cargo test --workspace --no-fail-fast`（/workspace）**退出码 0**：单元 69＋apply_patch 31＋exec 9＋llm_flow 40＝**149 通过 / 0 失败 / 0 跳过 / 未运行 0**。
- 验证规则更新：AGENTS.md「全量验证」行补入副本 Cargo 必须显式 --target-dir 的隔离要求（最小化，未新增变异框架）。
- 剩余缺口维持如实标注：Q1–Q4 待决、main 快照时点、真实 ENOSPC、fingerprint 恒真子句、双流顺序、display 未覆盖转义变体（8.1 注记）。

## 九、测试精简执行（2026-10-09，依据第八节后清单讨论决定）

| 动作 | 项 | 承重断言保留位置 | 证据 |
|---|---|---|---|
| 删除 | `d3_same_file_three_hunk_chain` | 并入 `d1`（三跳链：块3 匹配块2 产物 GAMMA→FINAL；最终 `alpha\nbeta\nFINAL\n` 逐字节＋三项 changed_files 精确） | 专用副本（显式 --target-dir）变异：`run()` 只执行前两块 → 合并后 D1 红（GAMMA≠FINAL 字节级）；恢复绿、源码字节一致 |
| 删除 | `pure_insert_chunk_appends_at_end`（单元，内部元组断言） | `d4_same_file_eof_anchor_after_append`：真实 Update 路径、**非空文件** `head\n` 纯插入 `tail` 追加到末尾（块2 在 EOF 锚点匹配到 tail＋最终字节 `head\nTAIL\n` 逐字节证明追加位置）；空文件纯插入由 `b9` 持有——守护依据非仅 B9 | D4 实跑通过；等价性核对记录 |
| 缩减 | `invalid_json_args_error_without_side_effects` 6 例→2 代表例 | 单元 `args_reject_bad_values` 持完整矩阵；集成保留非对象＋未知字段（**携带本可执行的有效补丁**，断言拒绝后 probe 未创建＝拒绝先于任何执行） | 实跑通过 |
| 保留 | `replacements_split_around_context_lines` | —（按决定：上下文两侧修改/保留的等价守护证据不足） | 不动 |
| 保留 | `fingerprint_rules` 的 contains 子断言 | 其余三条（相等/不等/len=16）**不能**发现"定长输出内嵌原文"类变体（如 8 字节原文+8 字节哈希），仅 contains 可抓——按"证据不足则保留"判定 | 推演记录 |

计数：152 → **150**（删 2 项；矩阵缩减不减测试数）。B 组候选与其余测试未动。
