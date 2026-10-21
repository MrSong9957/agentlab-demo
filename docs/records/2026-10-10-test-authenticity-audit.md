# 测试真实性与敏感性审计（2026-10-10）

> 方法：先取真实基线并枚举全部测试名；按“承诺/输入/生产路径/mock 边界/核对/预期来源/变异验证”逐项审计；再在隔离副本（含全部未提交与未跟踪文件、独立构建目录、无硬链接）施加**单一业务变异**，用准确测试名＋`--exact` 实跑确认检出。全程假凭证、本地 mock、隔离目录；未提交未 push。

## 一、真实基线与枚举

- `cargo check` → Finished，0 警告，退出码 0。
- `cargo test --workspace --no-fail-fast` → **214 通过 / 0 失败 / 0 跳过**，退出码 0
  （lib 85＋apply_patch 32＋e2e_acceptance 9＋exec_command 14＋llm_flow 51＋prompt_assembly 9＋tool_loop 14）。
- `cargo test --workspace --no-fail-fast -- --list` → 枚举 **214 个唯一全名**
  （`/tmp/all-tests.txt`，本记录第三节逐条覆盖；分组：lib 单元 85＝catalog 27＋apply_patch 23＋exec 14＋display 9＋turn 5＋prompt 4＋api 3）。

## 二、变异验证总表（13 个有效变异，按唯一独立改动编号计；每行注明轮次与原记录）

每个变异＝独立副本＋单一改动＋`--exact` 实跑（执行数非零），失败均源于业务断言；
首轮/收尾轮已证变异按指令复用，不重复制造等价修改。

| 编号 | 功能类 | 变异（副本内 diff 位置与内容） | 实跑命令（均带 `-- --exact`、`timeout 300` 看门狗） | 结果（退出码）与失败断言 | 轮次 |
|---|---|---|---|---|---|
| M1 授权绕过 | 授权门 | `catalog.rs::ensure_origin_authorized` 非交互分支 `return Err(...)`→`return Ok(())` | `--test llm_flow bin_pipe_unauthorized_origin_rejects_without_request` | FAILED(101)：`未授权必须零请求，凭证零外发`（mock 计数≠0，凭证真实外发） | 本轮 |
| M2 .env 控制变量 | 信任根 | `catalog.rs` `TRUST_CONTROL_VARS` 名单命中改为 `false` | `--test e2e_acceptance s3a_f1_env_trust_root_hijack_blocked` | FAILED(101)：`TCHAT_CONFIG_DIR 劫持目标收到请求：left 1 right 0`＋输出 `Agent：S3A-EVIL2`（静默外发） ｜原记录：logs/2026-10-09-e2e-acceptance.md §二变异证明 | 复用（2026-10-09 首轮） |
| M-3xx 3xx 错误分类 | 重定向/HTTPS | `api.rs::post_chat_text` 3xx 拒绝分支整体移除（注：该变异**并未造成跟随**，仅取消拒绝分类，故名改此） | `--test llm_flow redirect_targets_never_receive_credentials` | FAILED(101)：`实际错误：响应解析失败：EOF…`——`err.contains("重定向")` 不成立 | 本轮 |
| **M-follow 重定向跟随**（新） | 重定向/HTTPS | `api.rs::agent_with_timeouts` `.redirects(0)`→`.redirects(5)`（真实客户端允许跟随） | `--test llm_flow redirect_targets_never_receive_credentials`（＋副本内探针 `m_follow_probe_second_hop` 取独立第二跳证据） | 测试 FAILED(101)：`unwrap_err() on Ok("收到")`（跟随成功返回）；**探针证据：source_count=1、target_count=1（第二跳真实发生）、第二跳为 GET 且 `Authorization` 未被 ureq 透传（auth=None）**——目标零请求契约依然必要：请求本身到达目标，且凭证面依赖客户端实现细节 | 本轮（M-follow） |
| M4 退出码错报 | exec_command | `exec.rs` `child.wait().ok().and_then(\|s\| s.code())`→`.map(\|_\| 0)` | `--test exec_command stdout_stderr_merged_exit_code_preserved` | FAILED(101)：`left: Some(0) right: Some(7)` | 本轮 |
| M5 Add 覆盖 | apply_patch | `apply_patch.rs` `.create_new(true)`→`.create(true).truncate(true)` | `--test apply_patch patch_b05_concurrent_add_single_winner`（对照 a1 通过） | FAILED(101)：并发 Add 出现覆盖/混合，恰一胜者断言破｜原记录：logs/2026-10-10-critical-functional-coverage.md §二变异 A（对照 a1 通过） | 复用（ctmut2 变异 A） |
| M6a 无效批部分执行 | 工具循环 | `turn.rs::parse_tool_calls` 缺 id `return Err`→`continue`（跳过无效条目） | `--test tool_loop t12_invalid_batch_no_side_effects_history_stays_valid` | FAILED(101)：`整批不执行：首项合法也不得有副作用`（flag 文件被创建） | 本轮 |
| M6b 突破 8 次上限 | 工具循环 | `turn.rs` `MAX_MODEL_REQUESTS_PER_TURN` 8→9 | `--test tool_loop t7_request_cap_reports_exhaustion` | FAILED(101)：`恰 8 次请求，无第 9 次：left 9 right 8` | 本轮 |
| M7 脱敏移回截断后 | 凭证泄露面 | `api.rs::describe_status_error` 先脱敏后截断→对调回先截断 | `--test llm_flow secret_e02_straddle_boundary_no_fragment` | FAILED(101)：`截断造成的 Key 片段残留`（错误尾部可见 `test-key-A`）｜原记录：logs/2026-10-10-critical-functional-coverage.md §一（收尾轮缺陷表，含旧顺序还原复现） | 复用（收尾轮，真实行为 RED） |
| M8 字符集终字节残留 | 终端过滤 | `display.rs::skip_escape` nF 中间字节分支回退为两字符转义 | `--lib display::tests::charset_selection_escapes_removed` | FAILED：`left "aBb" right "ab"`（终字节 B 残留）｜原记录：logs/2026-10-10-critical-functional-coverage.md §一（TERM-B03 缺陷表） | 复用（高风险轮 RED） |

附加复用（历史变异，编号不与表内重复——核对：`grant_origin` 写失败忽略≠M1 的 `ensure_origin_authorized` 非交互分支；回复分支提前上限≠M6b 的常量改动；F2 写回按路径≠M5 的 create_new）：
| 编号 | 变异 | 检出测试 | 原记录与失败原因 |
|---|---|---|---|
| 变异B（ctmut2） | `catalog.rs::grant_origin` 忽略 `write_private_file` 失败 | `auth_e02_persistence_failure_blocks_request` | logs/2026-10-10-critical-functional-coverage.md §二：同意后仍外发（对照 s2a 通过） |
| 变异A-006（加固轮） | `turn.rs` 回复分支内提前报上限 | `t14_eighth_request_final_reply_ends_normally` | docs/specs/006-tool-call-loop.md §五加固记录：第 8 次最终回复被吞（t7/t13/t1 对照通过） |
| 变异F2（SR09 轮） | `apply_patch.rs` 写回从 /proc/fd 绑定改为按路径重开 | `f2_swap_barrier_binds_write_to_validated_inode` | docs/security/reports/2026-10-09-backend-security.md §F2 最终验收：看门狗红 15.01s（写入阻塞在 FIFO），副本源码与工作区一致复证 |

**漏检：0/13**——所有变异均被对应测试以业务断言检出（其中 redirect_targets 被 M-3xx 与 M-follow 两个不同变异点各自检出）；无“变异改变应保护结果而测试仍通过”的情况，故无补强项。
**诚实声明**：13 个变异动态证明的是上表及附加复用表列出的具体测试，**不等于 214 项全部动态证明**；其余测试的真实性依据第三节静态审计（实际输出/字节/请求/进程核对方式）。

## 三、逐项审计表（214 行；键＝完整测试名，与 logs/2026-10-10-test-inventory.txt 一一对应）

替身边界四分类：**无替身**（纯函数/真实 fs/真实进程/真实并发）、**HTTP mock**（仅替代远端服务器，
业务逻辑零替换）、**注入 sender/宿主**（替代模型端或作为 harness，循环与工具真实执行）、
**测试屏障**（f2_seam 同步接缝、EFBIG 限额子进程、看门狗）。多边界组合如实标注（如“真实 CLI＋HTTP mock”）。
动态标注仅授予在对应变异下**实际执行并报红**的测试；其余为“静态关联（…，未运行）”或“未动态验证”。

### 3.1 lib 单元（85）
#### catalog::tests（27）
- `catalog::tests::audit_tightens_and_reports_without_secrets`｜承诺：启动审计只收权限位、不泄秘密（SR05 F7）｜输入：0777 .env 含秘密＋0666 配置＋0777 上级｜路径：audit_file_permissions｜替身：无替身（真实 fs）｜核对：权限位实变 0600、报告含路径无秘密值、上级仅告警｜来源：SR05｜动态：未动态验证
- `catalog::tests::auth_b05_concurrent_grants_investigation`｜承诺：〔调查〕并发授权安全不变量（CT 待项）｜输入：双线程 grant 不同 origin｜路径：grant_origin｜替身：无替身（真实 fs 线程并发）｜核对：文件可解析、grant 均属同意集合；实测保留 1 条｜来源：CT 待项｜动态：静态关联（AUTH-B05 决策点，未动态验证）
- `catalog::tests::corrupt_files_reported_and_not_rewritten`｜承诺：损坏文件明确报错不覆写（S002）｜输入：坏 JSON manual/auto｜路径：load_manual/load_auto｜替身：无替身｜核对：错误文案＋原文件字节不变｜来源：S002｜动态：未动态验证
- `catalog::tests::credential_env_and_file`｜承诺：env:/file: 取值与缺失语义（S002）｜输入：各形态凭证引用｜路径：resolve_credential｜替身：无替身｜核对：值/错误文案（未设置/为空/不支持前缀）｜来源：S002｜动态：未动态验证
- `catalog::tests::credential_file_error_branches`｜承诺：file: 五类失败分支（S002）｜输入：缺文件/坏JSON/缺字段/空值/缺#｜路径：resolve_credential｜替身：无替身｜核对：错误分类逐一命中｜来源：S002｜动态：未动态验证
- `catalog::tests::debug_output_redacts_key`｜承诺：Debug 脱敏（SR05）｜输入：含秘密 Credential/CredOutcome｜路径：Debug impl｜替身：无替身（纯格式化）｜核对：不含明文、保留 *** 状态｜来源：SR05｜动态：未动态验证
- `catalog::tests::dotenv_loads_missing_vars_without_override`｜承诺：.env 只补缺不覆盖＋引号/注释/重复键（S002）｜输入：预置 A＋多行 .env｜路径：load_dotenv_from｜替身：无替身（真实临时 .env）｜核对：加载计数与最终值独立预期｜来源：S002｜动态：未动态验证
- `catalog::tests::dotenv_skips_trust_control_vars`｜承诺：信任根名单跳过含小写变体（SR09 F1）｜输入：名单全量＋小写＋普通变量｜路径：load_dotenv_from｜替身：无替身｜核对：名单全未注入、普通变量加载（计数=1）｜来源：SR09 F1｜动态：静态关联（M2 面，未运行）
- `catalog::tests::explicit_false_and_zero_semantics`｜承诺：显式 false/0 不当缺省（S002）｜输入：disabled:false/context:0｜路径：merge_final/route｜替身：无替身｜核对：Some(false) 可路由、Some(0) 保留｜来源：S002｜动态：未动态验证
- `catalog::tests::file_credential_confined_to_credential_dir`｜承诺：file: 围栏（SR09）｜输入：目录外/内/相对/穿越/软链逃逸｜路径：resolve_credential（canonicalize 包含性）｜替身：无替身（真实软链）｜核对：逃逸三类拒绝、根内取值正确｜来源：SR09｜动态：静态关联（M2 链，未运行）
- `catalog::tests::fingerprint_rules`｜承诺：指纹规则（S002）｜输入：同/异 Key｜路径：fingerprint｜替身：无替身（纯函数）｜核对：精确串值＝独立 SHA-256 前 16 hex｜来源：S002 独立向量｜动态：未动态验证
- `catalog::tests::first_run_creates_default_files`｜承诺：首启默认文件（S002）｜输入：空目录｜路径：load_manual/load_auto｜替身：无替身｜核对：默认选中与两文件存在｜来源：S002｜动态：未动态验证
- `catalog::tests::identity_fields_each_invalidate`｜承诺：身份四字段任一变化失效旧名单（S002）｜输入：地址/协议/凭证名/换 Key 矩阵｜路径：merge_final｜替身：无替身｜核对：only-from-query 出现/消失逐项｜来源：S002｜动态：未动态验证
- `catalog::tests::manual_model_overlay_add_and_restore`｜承诺：字段级叠加与恢复（S002）｜输入：覆盖/新增/重建 manual｜路径：merge_final｜替身：无替身｜核对：display_name/上下文字段精确｜来源：S002｜动态：未动态验证
- `catalog::tests::manual_overrides_only_written_fields`｜承诺：只覆盖写出字段（S002）｜输入：仅 base_url 覆盖｜路径：effective_connection｜替身：无替身｜核对：协议/key_ref 沿用基础值｜来源：S002｜动态：未动态验证
- `catalog::tests::net_b03_loopback_list_precise`｜承诺：回环豁免名单精确（CT NET-B03）｜输入：三种回环写法＋127.0.0.2｜路径：validate_base_url｜替身：无替身（纯函数）｜核对：前三 Ok、.2 报 HTTPS｜来源：CT＋IANA 对照｜动态：未动态验证
- `catalog::tests::origin_grant_store_semantics`｜承诺：授权双匹配/损坏 fail-closed/交互同意拒绝（SR09 F1）｜输入：多形态 store＋回调｜路径：is/grant/ensure_origin_authorized｜替身：无替身（真实 fs）｜核对：布尔判定、0600、{broken}→无授权、非交互 Err｜来源：SR09｜动态：静态关联（M1 面，未运行）
- `catalog::tests::origin_of_normalizes`｜承诺：origin 归一化（S002）｜输入：路径/默认端口/坏 URL｜路径：origin_of｜替身：无替身（纯函数）｜核对：精确 origin 串｜来源：S002＋URL 标准｜动态：未动态验证
- `catalog::tests::private_fs_modes_enforced`｜承诺：0700/0600 且复读确认（SR05 F7）｜输入：真实目录与文件｜路径：ensure_private_dir/write_private_file｜替身：无替身｜核对：权限位实测值｜来源：SR05｜动态：未动态验证
- `catalog::tests::readonly_dir_write_fails_cleanly`｜承诺：只读目录保存失败不损坏（S002）｜输入：0555 目录＋probe 探权限｜路径：save_auto/write_atomic｜替身：无替身（非 root 实测）｜核对：旧数据字节不变、无 .tmp 残留｜来源：S002｜动态：未动态验证
- `catalog::tests::route_checks_credential_and_disabled_model`｜承诺：路由检查四形态（S002）｜输入：禁用/缺模型/缺 Key/正常｜路径：route｜替身：无替身（凭证文件真实）｜核对：错误文案＋快照字段精确｜来源：S002｜动态：未动态验证
- `catalog::tests::route_rejects_remote_http_and_empty_base_url`｜承诺：远程 http/空地址拒绝（SR05）｜输入：http 远程/空串｜路径：route→validate_base_url｜替身：无替身（纯函数链）｜核对：错误含 HTTPS/地址｜来源：SR05｜动态：未动态验证
- `catalog::tests::stale_tmp_does_not_block_save`｜承诺：遗留 tmp 不阻断保存（S002 坑）｜输入：预置旧 tmp＋标记模型｜路径：save_auto/load_auto｜替身：无替身｜核对：stale-tmp-marker 落盘、旧 tmp 原样｜来源：S002 坑记录｜动态：未动态验证
- `catalog::tests::trust_b05_credential_root_symlink`｜承诺：链接根下围栏仍成立（CT TRUST-B05）｜输入：根为符号链接＋根内/逃逸引用｜路径：resolve_credential｜替身：无替身（真实软链）｜核对：根内取值正确、逃逸拒绝不泄内容｜来源：CT｜动态：静态关联（M2 链，未运行）
- `catalog::tests::trust_b06_dotenv_bom_and_crlf`｜承诺：BOM 跳过/CRLF 解析/控制变量不生效（CT TRUST-B06）｜输入：BOM+CRLF .env｜路径：load_dotenv_from｜替身：无替身｜核对：计数=1、值精确、名单未注入｜来源：CT＋既有语法｜动态：未动态验证
- `catalog::tests::unknown_manual_field_rejected`｜承诺：拼错字段拒绝且不改写（S002）｜输入：base_ur/根层未知字段｜路径：load_manual｜替身：无替身｜核对：错误含字段名＋原文件字节不变｜来源：S002｜动态：未动态验证
- `catalog::tests::validate_base_url_rules`｜承诺：URL 准入规则表（SR05）｜输入：https/回环/远程http/空/无前缀/ftp/带空格｜路径：validate_base_url｜替身：无替身（纯函数）｜核对：Ok/Err 分类与文案｜来源：SR05 规则表｜动态：未动态验证
#### apply_patch::tests（23）
- `apply_patch::tests::apply_replacements_keep_endings_and_complete_trailing`｜承诺：行尾保持与末尾换行补齐（S004）｜输入：混合行尾样本｜路径：替换引擎｜替身：无替身（纯函数）｜核对：输出字节黄金向量｜来源：S004｜动态：未动态验证
- `apply_patch::tests::args_accept_patch_field`｜承诺：入参仅接受 patch（S004）｜输入：合法/Null 字段｜路径：ApplyPatchArgs::from_value｜替身：无替身（纯函数）｜核对：解析值精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::args_reject_bad_values`｜承诺：非对象/未知/非字符串拒绝（S004）｜输入：矩阵｜路径：ApplyPatchArgs::from_value｜替身：无替身（纯函数）｜核对：错误文案（校验先于执行）｜来源：S004｜动态：未动态验证
- `apply_patch::tests::change_context_seek_advances_search_start`｜承诺：@@ 定位推进搜索起点（S004）｜输入：重复 old 行｜路径：匹配引擎｜替身：无替身（纯函数）｜核对：命中位置字节精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::f2_swap_barrier_binds_write_to_validated_inode`｜承诺：F2 写前窗口对象替换绑定原 inode（SR09 F2 收口）｜输入：屏障＋rename 换 FIFO 两变体｜路径：apply_patch（含 f2_seam）｜替身：测试屏障（f2_seam 同步接缝＋15s 看门狗子进程）｜核对：原文件字节 R1..R5 精确、FIFO 未被读写、清单正确｜来源：SR09 F2｜动态：动态检出（SR09 历史变异：写回按路径重开→看门狗红 15.01s，docs/security/reports/2026-10-09-backend-security.md §F2 最终验收）
- `apply_patch::tests::parse_full_patch_structure`｜承诺：完整补丁解析结构（S004）｜输入：典型补丁｜路径：parse_patch｜替身：无替身（纯函数）｜核对：Hunk 结构精确｜来源：S004 母本｜动态：未动态验证
- `apply_patch::tests::parse_rejects_invalid_patches`｜承诺：非法补丁逐类拒绝（S004）｜输入：坏标记/坏行｜路径：parse_patch｜替身：无替身（纯函数）｜核对：错误分类逐项｜来源：S004｜动态：未动态验证
- `apply_patch::tests::parse_empty_patch_yields_no_hunks`｜承诺：空补丁零块（S004）｜输入：仅 marker｜路径：parse_patch｜替身：无替身（纯函数）｜核对：块数=0＋错误｜来源：S004｜动态：未动态验证
- `apply_patch::tests::parse_crlf_patch_text_matches_lf`｜承诺：CRLF 行匹配 LF 上下文（S004）｜输入：CRLF 文件｜路径：parse_patch｜替身：无替身（纯函数）｜核对：匹配成功｜来源：S004｜动态：未动态验证
- `apply_patch::tests::parse_end_of_file_marker_sets_flag`｜承诺：EOF 标记置位（S004）｜输入：带 End of File 补丁｜路径：parse_patch｜替身：无替身（纯函数）｜核对：锚定标志精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::parse_context_and_empty_lines`｜承诺：上下文/空行解析（S004）｜输入：混合行｜路径：parse_patch｜替身：无替身（纯函数）｜核对：行分类精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::parse_indented_update_marker_is_context`｜承诺：缩进 Update 标记按上下文（S004）｜输入：缩进行｜路径：parse_patch｜替身：无替身（纯函数）｜核对：分类正确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::parse_marker_whitespace_is_lenient`｜承诺：标记空白宽容（S004）｜输入：变体空白｜路径：parse_patch｜替身：无替身（纯函数）｜核对：解析成功｜来源：S004｜动态：未动态验证
- `apply_patch::tests::parse_update_without_at_at_starts_first_chunk`｜承诺：无 @@ 首块起点（S004）｜输入：直接变更行｜路径：parse_patch｜替身：无替身（纯函数）｜核对：首块语义正确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::replacements_retry_without_trailing_empty_sentinel`｜承诺：哨兵重试（S004 B8）｜输入：末尾空行 old｜路径：替换引擎｜替身：无替身（纯函数）｜核对：输出字节精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::replacements_split_around_context_lines`｜承诺：上下文分块（S004）｜输入：含上下文块｜路径：替换引擎｜替身：无替身（纯函数）｜核对：输出字节精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::result_serializes_spec_shape`｜承诺：结果四字段形状（S004）｜输入：构造结果｜路径：Serialize｜替身：无替身（纯函数）｜核对：json! 精确对照｜来源：S004 契约｜动态：未动态验证
- `apply_patch::tests::seek_eof_anchors_at_end`｜承诺：EOF 锚定末位（S004 B6）｜输入：重复行文件｜路径：seek 引擎｜替身：无替身（纯函数）｜核对：命中索引精确｜来源：S004 规则推导｜动态：未动态验证
- `apply_patch::tests::seek_exact_then_rstrip_then_trim`｜承诺：三级容差优先（S004 B 区）｜输入：空白变体行｜路径：seek 引擎｜替身：无替身（纯函数）｜核对：命中索引序精确｜来源：S004 规则推导｜动态：未动态验证
- `apply_patch::tests::seek_pattern_longer_than_lines_is_none`｜承诺：超长 pattern 无命中（S004）｜输入：长 pattern｜路径：seek 引擎｜替身：无替身（纯函数）｜核对：None｜来源：S004｜动态：未动态验证
- `apply_patch::tests::seek_respects_start_index`｜承诺：起始索引约束（S004）｜输入：@@ 后搜索｜路径：seek 引擎｜替身：无替身（纯函数）｜核对：命中位置精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::source_file_preferred_ending_is_first_found`｜承诺：风格取首个换行（S004）｜输入：混合行尾｜路径：行尾引擎｜替身：无替身（纯函数）｜核对：输出字节精确｜来源：S004｜动态：未动态验证
- `apply_patch::tests::source_file_roundtrips_contents`｜承诺：内容往返（S004）｜输入：多行样本｜路径：行尾引擎｜替身：无替身（纯函数）｜核对：逐字节一致｜来源：S004｜动态：未动态验证
#### exec::tests（14）
- `exec::tests::sink_keeps_valid_text`｜承诺：有效文本保留（S003）｜输入：hello 块｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：输出+truncated 精确｜来源：S003｜动态：未动态验证
- `exec::tests::sink_joins_multibyte_split_across_chunks`｜承诺：跨块多字节拼接（S003）｜输入：你/🎉拆字节块｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：精确串｜来源：S003＋UTF-8 标准｜动态：未动态验证
- `exec::tests::sink_replaces_invalid_bytes`｜承诺：非法字节替换（S003）｜输入：0xFF/0xC3(｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：U+FFFD 计数精确｜来源：S003＋lossy 标准｜动态：未动态验证
- `exec::tests::sink_cap_exact_not_truncated`｜承诺：恰上限不截断（S003）｜输入：héllo=5 码点｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：计数与布尔精确｜来源：S003｜动态：未动态验证
- `exec::tests::sink_cap_truncates_and_stays_bounded`｜承诺：超限截断且有界（S003）｜输入：超限块序列｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：缓存不再增长｜来源：S003｜动态：未动态验证
- `exec::tests::sink_cap_cuts_inside_chunk`｜承诺：块内截断（S003）｜输入：a🎉bcd 上限 3｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：a🎉b｜来源：S003｜动态：未动态验证
- `exec::tests::sink_cap_reached_exactly_then_more`｜承诺：恰满后再来必标截断（S003）｜输入：ab+0x01｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：truncated=true｜来源：S003｜动态：未动态验证
- `exec::tests::sink_incomplete_tail_at_eof_replaced`｜承诺：EOF 尾残缺替换（S003）｜输入：0xE4｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：单个 U+FFFD｜来源：S003｜动态：未动态验证
- `exec::tests::sink_truncation_replacement_counts_one_char`｜承诺：替换符计一码点（S003）｜输入：a\xffb 上限 2｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：a\u{FFFD}｜来源：S003｜动态：未动态验证
- `exec::tests::args_happy_path_with_all_fields`｜承诺：全字段解析＋Null 缺省（S003）｜输入：四字段/Null workdir｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：结构体精确｜来源：S003｜动态：未动态验证
- `exec::tests::args_reject_non_object_and_unknown_field`｜承诺：非对象/未知字段（S003）｜输入：5/"cmd"/evil｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：文案含 对象/未知参数｜来源：S003｜动态：未动态验证
- `exec::tests::args_reject_bad_cmd`｜承诺：cmd 非法（S003）｜输入：缺/空/空白/数字｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：文案含 cmd｜来源：S003｜动态：未动态验证
- `exec::tests::args_reject_bad_numbers_and_workdir`｜承诺：数值与 workdir 非法（S003）｜输入：0/-5/"500"/1.5/3｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：字段名逐一｜来源：S003｜动态：未动态验证
- `exec::tests::result_serializes_spec_shape`｜承诺：五字段小写形状（S003）｜输入：构造结果｜路径：ExecArgs::from_value / TextSink / Serialize（按项）｜替身：无替身（纯函数）｜核对：json! 精确｜来源：S003 契约｜动态：未动态验证
#### display::tests（9）
- `display::tests::csi_sequences_removed`｜承诺：CSI 剥离（SR05 电池D）｜输入：\x1b[2J 等｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：过滤串逐字精确｜来源：SR05/CT/ECMA-48｜动态：未动态验证
- `display::tests::osc_and_string_sequences_removed`｜承诺：OSC/DCS/APC/PM/SOS 剥离（SR05）｜输入：OSC52/标题/ST/未闭合｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：载荷整段移除｜来源：SR05/CT/ECMA-48｜动态：未动态验证
- `display::tests::two_char_escapes_removed`｜承诺：两字符转义剥离（SR05）｜输入：ESC c/ESC M｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：整段剥离＋正常字保留｜来源：SR05/CT/ECMA-48｜动态：未动态验证
- `display::tests::dangerous_controls_removed`｜承诺：危险控制符剥离（SR05）｜输入：DEL/C1 0x9B/BS/BEL｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：引导符剥离｜来源：SR05/CT/ECMA-48｜动态：未动态验证
- `display::tests::normal_text_preserved`｜承诺：正常文字无损（SR05）｜输入：中文 emoji 标点｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：逐字相等｜来源：SR05/CT/ECMA-48｜动态：未动态验证
- `display::tests::bare_escape_dropped`｜承诺：悬挂 ESC 丢弃（SR05）｜输入：abc\x1b｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：无 ESC 残留｜来源：SR05/CT/ECMA-48｜动态：未动态验证
- `display::tests::charset_selection_escapes_removed`｜承诺：nF 三字节整段剥离（模块 ECMA-48 契约）｜输入：ESC ( B/ESC ) 0/混合｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：无终字节残留｜来源：SR05/CT/ECMA-48｜动态：动态检出（M8·历史：修复前 left "aBb"≠"ab"，logs/2026-10-10-critical-functional-coverage.md §一）
- `display::tests::truncate_is_char_safe`｜承诺：按字符截断（SR05）｜输入：320 汉→300｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：计数精确｜来源：SR05/CT/ECMA-48｜动态：未动态验证
- `display::tests::truncate_boundary_299_300_301`｜承诺：截断三点边界（CT SECRET-B01）｜输入：299/300/301｜路径：sanitize_for_terminal / truncate_chars｜替身：无替身（纯函数）｜核对：计数＋前缀精确｜来源：SR05/CT/ECMA-48｜动态：未动态验证
#### turn::tests（5）
- `turn::tests::execute_unknown_tool_and_bad_json_arguments`｜承诺：未知工具/参数 JSON 无效按失败结构回传（S006）｜输入：两类调用｜路径：execute_tool→生产工具入口错误形态｜替身：无替身（分派真实工具校验）｜核对：error/status/ok 字段精确｜来源：S006｜动态：未动态验证
- `turn::tests::loop_b05_ten_call_batch_serial_with_real_side_effects`｜承诺：十调用批串行＋id 对应＋条数≠请求预算（CT LOOP-B05）｜输入：十条 exec 调用｜路径：run_tool_turn→生产 exec_command 真实落盘｜替身：注入 sender（模型端替身；工具真实执行）｜核对：十文件字节逐一＋十条结果 id/exit_code＋sent==2｜来源：CT｜动态：未动态验证
- `turn::tests::non_string_content_is_protocol_error_and_history_clean`｜承诺：content 非字符串=协议错误不污染历史（S006）｜输入：content 数组响应｜路径：run_tool_turn｜替身：注入 sender（模型端替身）｜核对：错误文案＋history 仅 1 条 user｜来源：S006｜动态：未动态验证
- `turn::tests::parse_accepts_valid_batch_and_rejects_bad_structure`｜承诺：整批结构校验规则（S006）｜输入：合法/缺id/type错/缺名/args非串/重复id/非数组｜路径：parse_tool_calls｜替身：无替身（纯函数）｜核对：批内容精确或协议错误文案｜来源：S006｜动态：静态关联（M6a 面，未运行）
- `turn::tests::turn_cap_and_batch8_semantics`｜承诺：恒返调用恰 8 请求/第 8 批入史/user 恰一次（S006）｜输入：恒 tc sender｜路径：run_tool_turn｜替身：注入 sender（模型端替身）｜核对：sent==8/结果==8/user==1｜来源：S006｜动态：静态关联（M6b 面，未运行）
#### prompt::tests（4）
- `prompt::tests::omits_checklist_sections_and_preserves_other_planning`｜承诺：过滤器移植保真（S005 母本）｜输入：Codex 原测试样本｜路径：without_update_plan_instructions｜替身：无替身（纯函数）｜核对：剔除 update_plan 节、保留无关 Planning 节｜来源：Codex-Pin 原测试三例｜动态：未动态验证
- `prompt::tests::omits_legacy_tool_and_plan_mode_cross_reference_sections`｜承诺：过滤器移植保真（S005 母本）｜输入：Codex 原测试样本｜路径：without_update_plan_instructions｜替身：无替身（纯函数）｜核对：剔除旧工具交叉引用节｜来源：Codex-Pin 原测试三例｜动态：未动态验证
- `prompt::tests::omits_only_checklist_list_items_and_goal_guidance`｜承诺：过滤器移植保真（S005 母本）｜输入：Codex 原测试样本｜路径：without_update_plan_instructions｜替身：无替身（纯函数）｜核对：只剔清单列表项｜来源：Codex-Pin 原测试三例｜动态：未动态验证
- `prompt::tests::plain_text_unchanged`｜承诺：过滤器移植保真（S005 母本）｜输入：Codex 原测试样本｜路径：without_update_plan_instructions｜替身：无替身（纯函数）｜核对：无规划节原文不变｜来源：Codex-Pin 原测试三例｜动态：未动态验证
#### api::tests（3）
- `api::tests::limited_reader_allows_exact_limit_and_stops_over`｜承诺：10MiB 读限精确（SR05 F6）｜输入：恰限/超限流｜路径：校验函数/LimitedReader｜替身：无替身（纯函数/纯 IO 包装）｜核对：恰限通过/超限 Err｜来源：SR05 常量｜动态：未动态验证
- `api::tests::model_id_rules`｜承诺：模型 id 校验（SR09 F4）｜输入：合法/非法 id｜路径：校验函数/LimitedReader｜替身：无替身（纯函数/纯 IO 包装）｜核对：分类精确｜来源：SR09｜动态：未动态验证
- `api::tests::public_catalog_url_rules`｜承诺：公共目录 URL 校验（SR09 F4）｜输入：各形态 URL｜路径：校验函数/LimitedReader｜替身：无替身（纯函数/纯 IO 包装）｜核对：分类精确｜来源：SR09｜动态：未动态验证
### 3.2 tests/apply_patch.rs（32）
- `invalid_patches_error_without_touching_files`｜承诺：十类坏补丁零写入（S004 E 区）｜输入：格式/空/Move to/End 后多余等｜路径：apply_patch（生产）｜替身：无替身（真实 fs 隔离目录）｜核对：ok=false＋文案＋目标字节不变＋目录无新文件｜来源：S004｜动态：未动态验证
- `invalid_json_args_error_without_side_effects`｜承诺：入参拒绝先于执行（S004）｜输入：5/带补丁的未知字段｜路径：apply_patch（生产）｜替身：无替身｜核对：文案＋probe 文件不存在｜来源：S004｜动态：未动态验证
- `add_update_delete_full_cycle`｜承诺：新增→改→删全链（S004）｜输入：真实三块补丁｜路径：apply_patch（生产）｜替身：无替身｜核对：三文件字节/存在性/changed_files 三元组精确｜来源：S004｜动态：未动态验证
- `conflicting_targets_rejected_without_changes`｜承诺：已存在/缺失/上下文不匹配置零改动（S004）｜输入：四形态｜路径：apply_patch（生产）｜替身：无替身｜核对：错误含路径＋目标字节不变＋state_known=true｜来源：S004｜动态：未动态验证
- `second_file_write_failure_reports_partial_state`｜承诺：第二目标写失败=前项生效＋截断前缀＋清单可信性（S004 坑003）｜输入：两文件补丁＋RLIMIT_FSIZE 4096｜路径：apply_patch（生产，子进程重入）｜替身：测试屏障（EFBIG 限额＋SIGXFSZ 忽略，隔离子进程）｜核对：good.txt 字节精确、big.txt 恰 4096 确定性前缀、清单只含前项、state_known=false｜来源：S004＋OS 语义｜动态：未动态验证
- `line_endings_follow_spec_rules`｜承诺：行尾规则五场景（S004）｜输入：LF/CRLF/混合/无末尾换行｜路径：apply_patch（生产）｜替身：无替身｜核对：字节黄金向量｜来源：S004｜动态：未动态验证
- `relative_paths_resolve_against_session_cwd`｜承诺：相对路径基于会话 cwd（S004）｜输入：chdir 后 Add rel.txt｜路径：apply_patch（生产）｜替身：无替身｜核对：绝对落点＋清单绝对路径｜来源：S004｜动态：未动态验证
- `non_utf8_target_rejected_and_unicode_roundtrip`｜承诺：仅 UTF-8＋中文 emoji 落盘（S004）｜输入：0xFF 目标＋uni 补丁｜路径：apply_patch（生产）｜替身：无替身｜核对：字节拒绝原文＋UTF-8 精确｜来源：S004｜动态：未动态验证
- `multi_chunk_update_and_context_seek`｜承诺：多 @@ 块＋纯插入追加（S004）｜输入：多块补丁｜路径：apply_patch（生产）｜替身：无替身｜核对：单文件字节精确｜来源：S004｜动态：未动态验证
- `readonly_permissions_are_enforced_not_bypassed`｜承诺：OS 权限不被绕过（S004 权限门）｜输入：0555 目录/0444 文件｜路径：apply_patch（生产）｜替身：无替身（非 root 实测）｜核对：三类失败＋state_known 区分＋victim 字节不变｜来源：S004｜动态：未动态验证
- `a1_update_follows_symlink_writes_real_target`｜承诺：Update 经链接写穿真实目标（S004 Q1 基线）｜输入：实链+Update｜路径：apply_patch（生产）｜替身：无替身（真实软链）｜核对：real.txt 字节=mode=test\n、链接本体保持、清单报链接路径｜来源：S004 契约记录｜动态：未动态验证（曾作 M5 对照通过）
- `a2_delete_removes_link_only_target_kept`｜承诺：Delete 只删链接本体（S004 Q2）｜输入：实链+Delete｜路径：apply_patch（生产）｜替身：无替身｜核对：链接消失、目标字节不变｜来源：S004 契约｜动态：未动态验证
- `a3_add_existing_symlink_rejected`｜承诺：Add 已有链接拒绝（S004 Q3）｜输入：实链+Add｜路径：apply_patch（生产）｜替身：无替身｜核对：不写穿、指向不变、目标字节不变｜来源：S004 契约｜动态：未动态验证
- `a4_add_dangling_symlink_rejected_keeps_dangling`｜承诺：Add 悬空链接拒绝且不修复（S004 Q4）｜输入：悬空+Add｜路径：apply_patch（生产）｜替身：无替身｜核对：目标仍不存在、悬空原样｜来源：S004 契约｜动态：未动态验证
- `a5_update_dangling_symlink_target_missing`｜承诺：Update 悬空按缺失拒（S004）｜输入：悬空+Update｜路径：apply_patch（生产）｜替身：无替身｜核对：不创建目标｜来源：S004 契约｜动态：未动态验证
- `a6_delete_dangling_symlink_cleans_link`｜承诺：Delete 悬空清理本体（S004）｜输入：悬空+Delete｜路径：apply_patch（生产）｜替身：无替身｜核对：本体删除、目标不存在｜来源：S004 契约｜动态：未动态验证
- `b1_exact_match_beats_earlier_trim_equivalent`｜承诺：精确优先于更早容差（S004 B1）｜输入：b␣/b 重复行｜路径：apply_patch（生产）｜替身：无替身｜核对：输出字节证明改行 3｜来源：S004 规则推导｜动态：未动态验证
- `b2_repeated_line_hits_first_occurrence`｜承诺：同内容取最早（S004 B2）｜输入：m/x/m｜路径：apply_patch（生产）｜替身：无替身｜核对：FIRST 在行 1｜来源：S004｜动态：未动态验证
- `b3_change_context_disambiguates_duplicates`｜承诺：@@ 消歧重复行（S004 B3）｜输入：sec a/sec b old｜路径：apply_patch（生产）｜替身：无替身｜核对：行 4 被改｜来源：S004｜动态：未动态验证
- `b4_trim_end_tolerance_whole_line_replacement`｜承诺：尾空白容差整行替换（S004 B4）｜输入：m␣/␣m｜路径：apply_patch（生产）｜替身：无替身｜核对：行 1 改、行 3 保留｜来源：S004｜动态：未动态验证
- `b5_trim_both_sides_tolerance_only_match`｜承诺：两端容差命中（S004 B5）｜输入：\tm\t｜路径：apply_patch（生产）｜替身：无替身｜核对：CORE 替换｜来源：S004｜动态：未动态验证
- `b6_end_of_file_anchor_pins_to_last_line`｜承诺：EOF 锚定末位（S004 B6）｜输入：x/keep/x 两份｜路径：apply_patch（生产）｜替身：无替身｜核对：锚定改末行/无锚改首行对照｜来源：S004｜动态：未动态验证
- `b7_end_of_file_anchor_mismatch_fails`｜承诺：EOF 锚定不符整体失败（S004 B7）｜输入：x/end｜路径：apply_patch（生产）｜替身：无替身｜核对：字节不变＋文案｜来源：S004｜动态：未动态验证
- `b8_trailing_empty_sentinel_retry`｜承诺：末尾空行哨兵重试（S004 B8）｜输入：x\ny 无末换行｜路径：apply_patch（生产）｜替身：无替身｜核对：5 字节 x\ny2\n｜来源：S004｜动态：未动态验证
- `b9_empty_file_pure_insert`｜承诺：空文件纯插入（S004 B9）｜输入：空文件｜路径：apply_patch（生产）｜替身：无替身｜核对：seed\n＋清单｜来源：S004｜动态：未动态验证
- `d1_same_file_two_hunks_stack_sequentially`｜承诺：同文件三块级联＋清单不去重（S004 D1/Q5）｜输入：三跳链｜路径：apply_patch（生产）｜替身：无替身｜核对：终态 17 字节＋三条 update 事件｜来源：S004 契约｜动态：未动态验证
- `d2_same_file_second_hunk_validation_failure_keeps_first`｜承诺：块 2 校验失败保留块 1（S004 D2）｜输入：两块｜路径：apply_patch（生产）｜替身：无替身｜核对：ALPHA 保留＋清单 1 条＋state_known=true｜来源：S004｜动态：未动态验证
- `d4_same_file_eof_anchor_after_append`｜承诺：EOF×追加复合（S004 D4）｜输入：head+tail｜路径：apply_patch（生产）｜替身：无替身｜核对：head\nTAIL\n＋2 事件｜来源：S004｜动态：未动态验证
- `d5_same_file_parse_error_first_hunk_not_executed`｜承诺：解析错误整补丁不执行（S004 D5）｜输入：块 2 语法错｜路径：apply_patch（生产）｜替身：无替身｜核对：orig 不变＋空清单｜来源：S004｜动态：未动态验证
- `d6_same_file_efbig_mid_write_keeps_prefix`｜承诺：同文件 EFBIG 半写保留前缀（S004 D6）｜输入：块 1 小改+块 2 大写｜路径：apply_patch（生产，子进程重入）｜替身：测试屏障（EFBIG 限额子进程）｜核对：前 4096 字节=期望前缀、清单仅块 1、state_known=false｜来源：S004＋OS 语义｜动态：未动态验证
- `f2_update_rejects_non_regular_targets`｜承诺：FIFO/链接到 FIFO//dev/null 有界拒绝（SR09 F2）｜输入：mkfifo 真实构造｜路径：apply_patch（生产）｜替身：测试屏障（15s 看门狗子进程）｜核对：三例 ok=false/文案/空清单/FIFO 原样＋常规对照可改｜来源：SR09 F2｜动态：未动态验证
- `patch_b05_concurrent_add_single_winner`｜承诺：并发 Add create_new 互斥（CT PATCH-B05）｜输入：双线程同路径异内容｜路径：apply_patch（生产）｜替身：无替身（真实并发）｜核对：恰一 ok、字节完整属胜者两种合法全文之一、败者拒绝｜来源：CT｜动态：动态检出（M5·历史：create_new→truncate 后覆盖/混合，logs/2026-10-10-critical-functional-coverage.md §二变异 A；对照 a1 通过）
### 3.3 tests/exec_command.rs（14）
- `invalid_inputs_error_without_side_effects`｜承诺：九类非法参数不执行（S003）｜输入：参数矩阵＋touch 探针｜路径：exec_command（生产）｜替身：无替身（真实进程探针）｜核对：Error/无退出码/无输出＋探针不存在｜来源：S003｜动态：未动态验证
- `workdir_resolves_relative_paths`｜承诺：绝对/相对/缺省 workdir（S003）｜输入：真实文件 cat/test｜路径：exec_command（生产）｜替身：无替身｜核对：输出内容＋cwd 判定｜来源：S003｜动态：未动态验证
- `stdout_stderr_merged_exit_code_preserved`｜承诺：双流合并＋真实退出码 7（S003）｜输入：echo 双流+exit 7｜路径：exec_command→/bin/sh -c（生产）｜替身：无替身｜核对：两行俱在＋exit_code==7｜来源：S003｜动态：动态检出（M4：`.map(|_| 0)` 后 Some(0)≠Some(7)）
- `invalid_utf8_bytes_replaced_end_to_end`｜承诺：非法字节 U+FFFD 端到端（S003）｜输入：printf 0xFF/0xFE｜路径：exec_command（生产）｜替身：无替身｜核对：精确串 a\u{FFFD}b\u{FFFD}c｜来源：S003＋lossy 标准｜动态：未动态验证
- `timeout_kills_process_group_and_stops_writes`｜承诺：超时终止进程组含孙进程（S003）｜输入：后台写循环+sleep 30｜路径：exec_command（生产）｜替身：无替身（断言窗口即看门狗）｜核对：Timeout/无退出码＋日志尺寸窗口前后相等｜来源：S003｜动态：未动态验证
- `cancel_flag_stops_managed_processes`｜承诺：取消标志终止组（S003）｜输入：250ms 后置位｜路径：exec_command（生产）｜替身：无替身｜核对：Cancelled＋日志停增｜来源：S003｜动态：未动态验证
- `oversized_output_truncated_to_char_cap`｜承诺：码点上限精确前缀（S003）｜输入：中文 emoji 500 码点/上限 200｜路径：exec_command（生产）｜替身：无替身｜核对：chars==200＋与全文前 200 码点逐字比｜来源：S003｜动态：未动态验证
- `discard_after_cap_keeps_child_running_to_completion`｜承诺：超限后继续排空（S003 坑）｜输入：200KB/上限 200｜路径：exec_command（生产）｜替身：无替身｜核对：completed+exit 0（未排空必超时）｜来源：S003｜动态：未动态验证
- `cd_inside_command_does_not_stick`｜承诺：cd 不跨调用（S003 范围）｜输入：cd 后再 test｜路径：exec_command（生产）｜替身：无替身｜核对：下条 Cargo.toml 判定成立｜来源：S003｜动态：未动态验证
- `exec_b04_dual_stream_mb_drain_after_cap`｜承诺：双流 MB 级小上限排空收尾（CT EXEC-B04）｜输入：40000×双行/上限 200｜路径：exec_command（生产）｜替身：无替身｜核对：completed/exit0/truncated/200 码点＋完整行属两合法流、尾段合法前缀（跨流时序不钉）｜来源：CT｜动态：未动态验证
- `exec_b05_utf8_multibyte_across_read_boundary`｜承诺：合法 UTF-8 跨读取块无损（CT EXEC-B05）｜输入：8190A+中🙂+99b｜路径：exec_command（生产）｜替身：无替身｜核对：与独立构造期望串逐字相等、无 U+FFFD｜来源：CT｜动态：未动态验证
- `exec_b07_concurrent_groups_isolated`｜承诺：并发进程组互不误杀（CT EXEC-B07）｜输入：A 超时写循环线程+B 五连发｜路径：exec_command（生产）｜替身：无替身（线程并发）｜核对：B 全部 completed 输出正确、A Timeout 后日志停增｜来源：CT｜动态：未动态验证
- `exec_p01_huge_timeout_investigation`｜承诺：〔调查〕u64::MAX 现状＋取消回收（CT 待项，不固化）｜输入：echo$$+sleep 30＋500ms 观察窗｜路径：exec_command（生产）｜替身：无替身（观察窗+取消即看门狗）｜核对：窗口内未返回（事实）、取消后 Cancelled＋组长/孙进程 ESRCH/Z 终态｜来源：CT 待项语义｜动态：未动态验证
- `exec_e03_nul_in_command_fails_to_start`｜承诺：NUL 启动失败（CT EXEC-E03）｜输入：cmd 含 \0＋touch 探针｜路径：exec_command（生产）｜替身：无替身｜核对：Error/无退出码输出＋文案＋探针不存在｜来源：CT＋OS 语义｜动态：未动态验证
### 3.4 tests/e2e_acceptance.rs（9）
- `s1_pty_normal_session_full_roundtrip`｜承诺：REPL 正常链路（S001/S002/S005）｜输入：PTY 交互序列｜路径：真实 CLI（CARGO_BIN_EXE）→route→授权门→工具循环｜替身：真实 CLI＋HTTP mock（仅远端模型）｜核对：捕获请求 method/path/Bearer/请求体（system 首位+两 tools）＋终端回复＋退出码 0＋授权记录字节不变＋密钥不回显｜来源：S001/S005｜动态：未动态验证
- `s2a_pty_first_use_authorization_agree_and_persist`｜承诺：首次同意全链（SR09 F1）｜输入：PTY y 确认｜路径：真实 CLI→ensure_origin_authorized→api｜替身：真实 CLI（PTY）＋HTTP mock｜核对：两次请求 Bearer、授权文件从无到有字段/0600/added_at 精确、首答后无再询问、无 Key 回显｜来源：SR09｜动态：静态关联（M1 同门，本测试未在该变异下运行）
- `s2b_pty_first_use_authorization_deny_zero_requests`｜承诺：拒绝零外发（SR09）｜输入：PTY n｜路径：真实 CLI→授权门｜替身：真实 CLI（PTY）＋HTTP mock｜核对：mock 0 请求、无授权文件、错误含 origin、无回复｜来源：SR09｜动态：静态关联（M1 同门，未运行）
- `s3a_f1_env_trust_root_hijack_blocked`｜承诺：F1 完整攻击链阻断（SR09 F1）｜输入：恶意 .env（未预设控制变量）＋预置指纹授权＋进程环境真 Key｜路径：真实 CLI 管道（cwd=项目）｜替身：真实 CLI＋HTTP mock×2（受害者端点+劫持端点）｜核对：两端点 0 请求、错误指向受害者家目录、攻击者 grant 字节不变、受害者根无新记录、无 Key 泄露｜来源：SR09｜动态：动态检出（M2·历史：名单禁用后劫持端点 1 请求＋Agent：S3A-EVIL2，logs/2026-10-09-e2e-acceptance.md §二）
- `s3b_f1_positive_dotenv_credential_and_explicit_preauth`｜承诺：F1 正例不误伤（SR09）｜输入：.env 普通凭证＋显式根预授权｜路径：真实 CLI 管道｜替身：真实 CLI＋HTTP mock×2｜核对：两路径请求头 Bearer 精确＋回复展示＋退出码 0｜来源：SR09｜动态：未动态验证
- `s4_json_host_apply_patch_data_loop`｜承诺：apply_patch 宿主→库→OS 闭环（S004 契约面）｜输入：JSON 宿主子进程三阶段+链接/FIFO｜路径：apply_patch（生产，测试二进制子进程模式）｜替身：注入 JSON 宿主（harness；库入口生产）｜核对：三阶段字节/存在性/changed_files＋链接写穿＋FIFO 两例拒绝｜来源：S004/SR09 F2｜动态：未动态验证
- `s6_json_host_exec_command_streams_exit_and_pgroup_timeout`｜承诺：exec_command 宿主→OS 闭环（S003 契约面）｜输入：JSON 宿主子进程双流/退出码/超时｜路径：exec_command（生产，子进程模式）｜替身：注入 JSON 宿主＋无替身进程判定｜核对：双流+exit 7+pwd＋超时后组长/孙进程 killpg ESRCH 或 /proc Z 终态｜来源：S003｜动态：未动态验证
- `auth_e02_persistence_failure_blocks_request`｜承诺：落盘失败不外发（CT AUTH-E02）｜输入：PTY y＋授权路径目录占位｜路径：真实 CLI→grant_origin 写失败路径｜替身：真实 CLI（PTY）＋HTTP mock｜核对：mock 0 请求、错误展示、夹具原样、无 Key 回显｜来源：CT｜动态：动态检出（变异 B·历史：忽略 grant 写失败→同意后仍外发，logs/2026-10-10-critical-functional-coverage.md §二；对照 s2a 通过）
- `auth_n04_refresh_first_use_authorization_via_repl`｜承诺：refresh 真实首次授权（CT AUTH-N04）｜输入：PTY /refresh 双 y＋本地不可达 HTTPS 目录 URL｜路径：真实 CLI→cmd_refresh→双授权门→list/chat｜替身：真实 CLI（PTY）＋HTTP mock＋注入目录 URL（本地不可达）｜核对：列表请求 Bearer、两询问各恰一次、汇总文案、双 grant（fp＋catalog 标记）｜来源：CT｜动态：未动态验证
### 3.5 tests/llm_flow.rs（51）
- `chat_sends_expected_json_and_parses_reply`｜承诺：请求体契约（S002）｜输入：mock 200 回复｜路径：api::chat（生产）｜替身：HTTP mock（仅远端）｜核对：body 字段数=2/model/messages 精确＋Bearer＋恰 1 请求｜来源：S002｜动态：未动态验证
- `auth_failure_is_reported_clearly`｜承诺：401 分类清晰（S002）｜输入：401+文案｜路径：api::chat｜替身：HTTP mock｜核对：分类文案＋恰 1 请求｜来源：S002｜动态：未动态验证
- `unsupported_protocol_rejected_before_request`｜承诺：未支持协议拒发（S002）｜输入：anthropic-messages｜路径：api::chat｜替身：HTTP mock（空）｜核对：0 请求（发送前拦截证据）｜来源：S002｜动态：未动态验证
- `confirmed_unsupported_param_blocks_before_request`｜承诺：参数能力门（S002）｜输入：temperature 三形态｜路径：api::chat｜替身：HTTP mock｜核对：三形态 0 请求＋支持时参数入 body｜来源：S002｜动态：未动态验证
- `list_models_reads_pagination_completely`｜承诺：分页读全（S002）｜输入：两页游标｜路径：api::list_models｜替身：HTTP mock｜核对：三模型按序＋两请求路径精确｜来源：S002｜动态：未动态验证
- `pagination_cursor_is_single_encoded_param`｜承诺：游标单参数编码（SR05 F5）｜输入：a&b#c模式 游标｜路径：api::list_models｜替身：HTTP mock｜核对：查询串单参数、百分解码还原｜来源：SR05 F5｜动态：未动态验证
- `chat_body_over_limit_aborts`｜承诺：响应体超限（SR05 F6）｜输入：11MiB JSON｜路径：api::chat｜替身：HTTP mock｜核对：文案含 10 MiB｜来源：SR05｜动态：未动态验证
- `error_body_over_limit_reports_cap_with_status`｜承诺：错误体超限含状态（SR05 F6）｜输入：11MiB 401｜路径：api::chat｜替身：HTTP mock｜核对：10 MiB＋401 保留｜来源：SR05｜动态：未动态验证
- `json_error_message_capped_at_300_chars`｜承诺：错误消息 300 上限（SR05 F3）｜输入：320 字符 message｜路径：api::chat｜替身：HTTP mock｜核对：X 计数≤300＋总长有界｜来源：SR05｜动态：未动态验证
- `refresh_success_replaces_old_list_and_keeps_selection`｜承诺：刷新成功契约（S002）｜输入：mock 列表两轮｜路径：catalog::refresh（生产）｜替身：HTTP mock（注入目录 URL 参数）｜核对：auto 文件独立解析：名单/身份/base source 精确＋默认模型不变＋第二轮替换｜来源：S002｜动态：未动态验证
- `refresh_failure_after_key_change_excludes_old_list`｜承诺：换 Key 失效旧名单（S002）｜输入：401+旧身份 auto｜路径：catalog::refresh｜替身：HTTP mock｜核对：failed 归类＋合并排除 only-from-query＋基础兜底｜来源：S002｜动态：未动态验证
- `refresh_with_valid_empty_list_clears_models`｜承诺：合法空名单清空（S002）｜输入：data=[]｜路径：catalog::refresh｜替身：HTTP mock｜核对：queried=(0)＋models 空｜来源：S002｜动态：未动态验证
- `pagination_failure_keeps_old_list_intact`｜承诺：分页失败保旧（S002）｜输入：页 2 失败｜路径：catalog::refresh｜替身：HTTP mock｜核对：恰 2 请求＋auto 名单保持旧值｜来源：S002｜动态：未动态验证
- `refresh_refuses_unauthorized_origin_without_request`｜承诺：未授权拒查（SR09 F1）｜输入：授权回调 Err｜路径：catalog::refresh｜替身：HTTP mock｜核对：queried 空＋failed 含未授权＋0 请求｜来源：SR09｜动态：静态关联（M1 面，未运行）
- `refresh_does_not_change_in_flight_request`｜承诺：快照隔离（S002）｜输入：慢响应线程+轮间换址｜路径：api::chat/catalog::refresh｜替身：HTTP mock×2＋线程｜核对：慢请求回复完整＋两 mock 路径各 1｜来源：S002｜动态：未动态验证
- `bin_pipe_session_chats_and_lists_models`｜承诺：C1 管道会话＋预授权（S001/S002/S005）｜输入：管道 你好/models/exit｜路径：真实 CLI 管道｜替身：真实 CLI＋HTTP mock｜核对：请求 Bearer/请求体（system+tools）＋回复＋auto 生成＋授权记录字节不变＋退出码 0｜来源：S001/S005｜动态：未动态验证
- `bin_pipe_unauthorized_origin_rejects_without_request`｜承诺：C2 非交互未授权零外发（SR09 F1）｜输入：管道 你好/exit＋无授权｜路径：真实 CLI 管道→授权门｜替身：真实 CLI＋HTTP mock｜核对：mock 0 请求＋文案（origin/路径/非交互）＋无授权文件＋无 Key 回显＋退出码 0｜来源：SR09｜动态：动态检出（M1：非交互放行后凭证真实外发，mock 计数≠0）
- `bin_pipe_rotated_key_invalidates_grant`｜承诺：C3 换 Key 失效（SR09）｜输入：旧 Key 授权+新 Key 运行｜路径：真实 CLI 管道｜替身：真实 CLI＋HTTP mock｜核对：0 请求＋记录字节不变＋退出码 0｜来源：SR09｜动态：未动态验证
- `redirect_targets_never_receive_credentials`｜承诺：重定向不跟随、目标零凭证（SR05）｜输入：302→B｜路径：api::chat（生产客户端）｜替身：HTTP mock×2｜核对：目标 0 请求＋源 1 次＋错误含重定向｜来源：SR05｜动态：动态检出（M-3xx：取消拒绝后错误变解析失败；M-follow：客户端 redirects(0)→(5) 后跟随，api 返回 Ok 使 unwrap_err 失败——探针独立证据：target_count=1、第二跳为 GET 且 Authorization 未透传）
- `public_sync_requires_authorization_and_validates_fields`｜承诺：F4 来源授权+字段校验（SR09）｜输入：未授权→授权两轮｜路径：catalog::refresh→sync_public｜替身：HTTP mock（注入 URL）｜核对：未授权保留旧目录＋非法 id 不入库＋OSC 展示名清理｜来源：SR09 F4｜动态：未动态验证
- `production_catalog_path_rejects_plaintext_http_even_with_stale_switch`｜承诺：正式路径强制 HTTPS（SR09）｜输入：http 目录+旧开关 env｜路径：catalog::refresh（None 注入）｜替身：HTTP mock｜核对：public_error 含 HTTPS＋0 请求＋旧目录保留｜来源：SR09｜动态：未动态验证
- `public_malformed_fields_no_panic`｜承诺：畸形字段不 panic（SR09 F4）｜输入：类型错字段｜路径：sync_public｜替身：HTTP mock｜核对：合法字段照常映射｜来源：SR09｜动态：未动态验证
- `public_missing_provider_keeps_base`｜承诺：目录缺厂商保底（SR09）｜输入：目录无 deepseek｜路径：sync_public｜替身：HTTP mock｜核对：base 全不变｜来源：SR09｜动态：未动态验证
- `public_unknown_providers_not_added`｜承诺：未知厂商不入库（SR09）｜输入：100 未知厂商｜路径：sync_public｜替身：HTTP mock｜核对：auto 键恰两内置｜来源：SR09｜动态：未动态验证
- `public_cannot_touch_connection_fields`｜承诺：公共数据不碰连接字段（SR09）｜输入：evil base_url/protocol/key_ref｜路径：sync_public｜替身：HTTP mock｜核对：连接三字段原样｜来源：SR09｜动态：未动态验证
- `public_sync_failure_continues_query`｜承诺：同步失败继续查询（SR09）｜输入：500 目录｜路径：sync_public+list｜替身：HTTP mock｜核对：public_error＋查询照常｜来源：SR09｜动态：未动态验证
- `missing_id_fails_whole_list`｜承诺：缺 id 整单失败（S002 六类）｜输入：无 id 条目｜路径：refresh→list_models｜替身：HTTP mock｜核对：failed 含缺少 id＋auto 保旧｜来源：S002｜动态：未动态验证
- `empty_page_with_has_more_fails`｜承诺：空页 has_more 异常（S002）｜输入：data=[]&has_more｜路径：refresh｜替身：HTTP mock｜核对：failed 分页异常＋保旧｜来源：S002｜动态：未动态验证
- `cursor_stall_fails_fast`｜承诺：游标停滞快败（S002）｜输入：同 id 两页｜路径：refresh｜替身：HTTP mock｜核对：恰 2 请求＋failed 游标未前进｜来源：S002｜动态：未动态验证
- `timeout_no_response_keeps_list`｜承诺：总超时保旧（S002）｜输入：silent mock+短超时｜路径：refresh（agent_with_timeouts）｜替身：HTTP mock｜核对：failed 网络错误＋名单不变｜来源：S002｜动态：未动态验证
- `disconnect_after_request_no_retry`｜承诺：断连不重试（S002）｜输入：连接即断｜路径：api::chat｜替身：HTTP mock｜核对：恰 1 请求＋网络错误｜来源：S002｜动态：未动态验证
- `key_leak_scan_across_failures`｜承诺：五类失败不泄 Key/指纹（SR05）｜输入：401/500/坏JSON/拒绝连接/缺文件｜路径：api::chat/resolve_credential｜替身：HTTP mock×3＋不可达地址｜核对：消息集合非空（防空转）＋逐条不含明文/指纹｜来源：SR05｜动态：未动态验证
- `newline_key_rejected_without_panic`｜承诺：换行 Key 拦截（SR05）｜输入：bad\nkey｜路径：api::chat｜替身：HTTP mock｜核对：Err 含控制字符＋不含 Key 片段＋0 请求｜来源：SR05｜动态：未动态验证
- `multi_turn_config_change_next_round`｜承诺：轮间换模型（S002）｜输入：两轮换 selected｜路径：route+api::chat｜替身：HTTP mock｜核对：两请求 body.model 精确｜来源：S002｜动态：未动态验证
- `reserved_params_rejected_before_request`｜承诺：保留字段拒发（S002）｜输入：request_params.model｜路径：api::chat｜替身：HTTP mock｜核对：文案＋0 请求｜来源：S002｜动态：未动态验证
- `response_edge_branches`｜承诺：回复三分类（S002）｜输入：思考截断/空/坏JSON｜路径：api::chat｜替身：HTTP mock｜核对：三类文案逐项｜来源：S002｜动态：未动态验证
- `base_url_slash_and_v1_paths`｜承诺：URL 拼接（S002）｜输入：尾斜杠//v1｜路径：api::chat｜替身：HTTP mock｜核对：/chat/completions 与 /v1/chat/completions｜来源：S002｜动态：未动态验证
- `map_key_authority_in_request`｜承诺：map 键权威（S002）｜输入：manual-id vs 记录 id｜路径：route+chat｜替身：HTTP mock｜核对：body.model==manual-id｜来源：S002｜动态：未动态验证
- `manual_vendor_full_custom_end_to_end`｜承诺：全新厂商全链（S002）｜输入：localmock 手动｜路径：refresh+chat｜替身：HTTP mock｜核对：查询+对话 body.model 精确｜来源：S002｜动态：未动态验证
- `concurrent_saves_last_writer_wins`｜承诺：并发保存完整（S002）｜输入：双线程×25 保存｜路径：load/save_auto｜替身：无替身（真实并发 fs）｜核对：终态可解析完整属一方＋无 .tmp 残留｜来源：S002｜动态：未动态验证
- `net_b01_all_redirect_statuses_rejected`｜承诺：全 3xx 拒绝（CT NET-B01）｜输入：300..399 七码｜路径：api::chat｜替身：HTTP mock×2（每码）｜核对：各 1 源请求/0 目标/文案含重定向｜来源：CT｜动态：静态关联（M-3xx/M-follow 面，未在变异下运行本测试）
- `net_b02_redirect_edge_forms_no_second_hop`｜承诺：3xx 边缘形态无二跳（CT NET-B02）｜输入：同origin/无Location/非法Location｜路径：api::chat｜替身：HTTP mock｜核对：各 1 请求＋文案｜来源：CT｜动态：静态关联（同上）
- `net_b06_body_limit_exact_three_points`｜承诺：L−1/L/L+1 精确（CT NET-B06）｜输入：独立字节核对构造｜路径：api::chat｜替身：HTTP mock｜核对：构造断言 target 字节＋L±成功内容完整（长度精确）＋L+1 含 10 MiB｜来源：CT＋SR05 常量｜动态：未动态验证
- `secret_e02_server_echo_of_key_is_redacted`｜承诺：回显三位置脱敏（CT SECRET-E02）｜输入：Key 在开头/300内/300外｜路径：api::chat｜替身：HTTP mock｜核对：请求确实发出＋err 不含 Key＋分类保留｜来源：CT/SR05｜动态：静态关联（M7 同面，未在变异下运行本测试）
- `secret_e02_straddle_boundary_no_fragment`｜承诺：横跨截断边界无片段（CT）｜输入：Key 起点 290＋10 字符片段｜路径：api::chat｜替身：HTTP mock｜核对：不含完整 Key 与片段＋分类保留｜来源：CT｜动态：动态检出（M7·历史：脱敏移回截断后→错误尾部残留 test-key-A，logs/2026-10-10-critical-functional-coverage.md §一收尾轮）
- `secret_e02_real_cli_output_never_echoes_key`｜承诺：真实 CLI 输出面（CT）｜输入：预授权+401 回显｜路径：真实 CLI 管道｜替身：真实 CLI＋HTTP mock｜核对：stdout+stderr 双扫描无 Key＋错误展示＋退出码｜来源：CT｜动态：静态关联（M7 面，未运行）
- `secret_b03_cr_and_nul_keys_rejected_without_echo`｜承诺：CR/NUL 拦截（CT SECRET-B03）｜输入：\r/\u{0} Key｜路径：api::chat｜替身：HTTP mock｜核对：0 请求＋文案不含原值｜来源：CT/SR05｜动态：未动态验证
- `term_e01_real_cli_filters_injected_sequences`｜承诺：真实 CLI 转义过滤（CT TERM-E01）｜输入：回复注入 OSC52/清屏/标题｜路径：真实 CLI 管道→say｜替身：真实 CLI＋HTTP mock｜核对：输出零 ESC 字节＋正常文字保留＋载荷不出现｜来源：CT/SR05｜动态：静态关联（M8 面：单元级已动态检出，本 E2E 未运行变异）
- `auth_e01_malformed_store_variants_fail_closed`｜承诺：四类损坏 fail-closed（CT AUTH-E01）｜输入：空/数组/根数组/字段类型错｜路径：真实 CLI 管道→load_origin_store｜替身：真实 CLI＋无 mock（0 请求即证）｜核对：零外发文案＋记录内容原样不被改写｜来源：CT｜动态：未动态验证
- `chat_with_body_rejects_params_overriding_core_fields`｜承诺：核心字段防覆盖（S006 收尾）｜输入：request_params 三字段合法声明｜路径：api::chat_with_body｜替身：HTTP mock（0 请求即证）｜核对：文案含保留字段＋mock 0 请求｜来源：S006｜动态：动态检出（历史 RED：tools 可覆盖复现失败后修复，logs/2026-10-10-critical-functional-coverage.md 缺陷表前置轮）
- `chat_with_body_sends_normal_params_and_assembled_core`｜承诺：普通参数传递+核心来源（S006）｜输入：temperature+组装请求｜路径：api::chat_with_body｜替身：HTTP mock｜核对：temperature 入 body＋model/messages/tools 来自组装｜来源：S006｜动态：未动态验证
### 3.6 tests/prompt_assembly.rs（9）
- `s1_embedded_source_matches_pinned_commit`｜承诺：嵌入母本锚点（S005）｜输入：RAW 常量｜路径：include_str 资产｜替身：无替身｜核对：len==20903＋SHA-256==独立锚点（非实现自比）｜来源：S005＋CT 锚点｜动态：未动态验证
- `s2_sent_prompt_only_listed_adaptations`｜承诺：发送文本仅含四项适配（S005）｜输入：母本 vs 发送版｜路径：sent_instructions（生产）｜替身：无替身（测试侧独立重建差分）｜核对：与独立重建逐字节一致＋残留/保留项逐项断言｜来源：S005｜动态：未动态验证
- `s3_empty_history_two_messages_two_tools`｜承诺：空历史形态（S005）｜输入：[]+你好｜路径：build_request（生产）｜替身：无替身｜核对：序列化再解析后 2 消息/2 工具/内容精确｜来源：S005｜动态：未动态验证
- `s4_history_preserved_no_dedup`｜承诺：历史保真不去重（S005）｜输入：两轮历史+同文当前｜路径：build_request（生产）｜替身：无替身｜核对：角色序/原文逐字/恰 2 次/历史快照不变｜来源：S005｜动态：未动态验证
- `s5_unicode_quotes_newlines_roundtrip`｜承诺：特殊字符往返（S005）｜输入：中文引号换行 emoji 反斜杠｜路径：build_request（生产）｜替身：无替身｜核对：\n 转义存在＋再解析逐字一致｜来源：S005｜动态：未动态验证
- `s6_tool_whitelist_matches_local_contracts`｜承诺：白名单与本地契约差分（S005/003/004）｜输入：schema 构实参/越界字段｜路径：build_request+ExecArgs::from_value+apply_patch（生产）｜替身：无替身（契约差分锚定）｜核对：schema 键集/required/additionalProperties＋实参接受/越界拒绝＋apply_patch 仅 patch 字段｜来源：S003/S004/S005｜动态：未动态验证
- `s7_protocol_mapping_shape`｜承诺：协议映射形态（S005）｜输入：两轮历史｜路径：build_request（生产）｜替身：无替身｜核对：顶层恰 {messages,tools}＋system 首/user 末＋function 数组｜来源：S005｜动态：未动态验证
- `s8_deterministic_reassembly`｜承诺：重复组装确定（S005）｜输入：同输入两次｜路径：build_request（生产）｜替身：无替身｜核对：两次 to_string 相等｜来源：S005｜动态：未动态验证
- `s9_blank_input_rejected`｜承诺：空白拒绝（S005）｜输入：空/空白/全角空格｜路径：build_request（生产）｜替身：无替身｜核对：三形态 Err＋历史不变｜来源：S005｜动态：未动态验证
### 3.7 tests/tool_loop.rs（14）
- `t1_plain_reply_ends_immediately`｜承诺：普通回复即结束（S006）｜输入：无 tool_calls 响应｜路径：真实 CLI→run_tool_turn｜替身：真实 CLI 管道＋HTTP mock｜核对：恰 1 请求+角色序+两工具+model+回复展示｜来源：S006｜动态：未动态验证
- `t2_apply_patch_creates_file_and_echoes_result`｜承诺：apply_patch 数据闭环（S006）｜输入：tc(apply_patch)+最终回复｜路径：真实 CLI→循环→apply_patch 真实落盘｜替身：真实 CLI＋HTTP mock｜核对：文件字节+第 2 请求 assistant 原样+tool_call_id 匹配+ApplyPatchResult JSON+[工具]行+user/system 恰一｜来源：S006｜动态：未动态验证
- `t3_exec_command_returns_real_output_and_exit_code`｜承诺：exec 真实输出/退出码回传（S006）｜输入：tc(exec exit 7)｜路径：真实 CLI→循环→exec_command｜替身：真实 CLI＋HTTP mock｜核对：回传 JSON status/exit_code=7/output 含 out-T3｜来源：S006｜动态：静态关联（M4 面：单元级已动态检出，本测试未在变异下运行）
- `t4_multiple_calls_run_serially_in_order`｜承诺：多调用串行顺序（S006）｜输入：tc 两调用（后者 cat 前者产物）｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：文件字节+第二条结果含 SERIAL-X+两条 id/顺序对应+活动行顺序｜来源：S006｜动态：未动态验证
- `t5_param_error_and_unknown_tool_echoed_as_tool_result`｜承诺：错误按调用回传（S006）｜输入：tc 三调用（参数错/未知/合法）｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：三条结果结构精确+合法调用真实落盘+循环继续至回复｜来源：S006｜动态：未动态验证
- `t6_history_carried_across_user_turns`｜承诺：跨轮历史保留（S006）｜输入：两用户轮｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：第 3 请求完整历史序+最终回复入史+system 恰一｜来源：S006｜动态：未动态验证
- `t7_request_cap_reports_exhaustion`｜承诺：上限耗尽（S006）｜输入：恒返调用 mock｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：恰 8 请求/副作用恰 8 次/第 8 请求含 7 批结果/无假回复｜来源：S006｜动态：动态检出（M6b：上限 8→9 后 9≠8 请求）
- `t8_request_failure_no_retry_history_kept`｜承诺：请求失败不重发（S006）｜输入：tc+500+再一轮｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：两轮共 3 请求+[工具]恰 1 次+下轮携带已执行记录｜来源：S006｜动态：未动态验证
- `t9_protocol_error_aborts_without_execution`｜承诺：协议错误中止（S006）｜输入：无 choices 响应｜路径：真实 CLI→run_tool_turn｜替身：真实 CLI＋HTTP mock｜核对：报错+无 [工具]+恰 1 请求｜来源：S006｜动态：未动态验证
- `t10_non_null_content_preserved_but_not_shown`｜承诺：非空 content 保真不显示（S006）｜输入：tc+content 字符串｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：终端无该内容+第 2 请求原样字符串｜来源：S006｜动态：未动态验证
- `t11_current_user_not_duplicated_in_loop`｜承诺：当前 user 不重复（S006）｜输入：tc,tc,ok｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：第 3 请求 user=1/system=1/两批结果就位｜来源：S006｜动态：未动态验证
- `t12_invalid_batch_no_side_effects_history_stays_valid`｜承诺：无效批整批拒绝（S006）｜输入：首项合法+后项缺 id｜路径：真实 CLI→run_tool_turn｜替身：真实 CLI＋HTTP mock｜核对：无副作用+无 [工具]+第 2 请求 [system,user,user]+下轮正常｜来源：S006｜动态：动态检出（M6a：缺 id 改跳过后 flag 文件被创建）
- `t13_cap_resets_next_turn_and_carries_batch8`｜承诺：上限重置+第 8 批保留（S006）｜输入：恒返调用+第二轮｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：共 16 请求/两次上限错误/第 9 请求含 8 批结果/副作用 16 次｜来源：S006｜动态：静态关联（M6b 面：t7 已检出，本测试未运行）
- `t14_eighth_request_final_reply_ends_normally`｜承诺：第 8 次恰为最终回复正常结束（S006）｜输入：7×tc+第 8 次 ok｜路径：真实 CLI→循环｜替身：真实 CLI＋HTTP mock｜核对：恰 8 请求+回复展示+不报上限+第 8 请求 7 批结果｜来源：S006｜动态：动态检出（变异 A-006·历史：回复分支内提前报上限后回复被吞，docs/specs/006-tool-call-loop.md §五加固记录）

## 四、漏检处理

9 个有效变异全部被对应测试以业务断言检出（第二节），**无漏检**，故无测试补强项。
过程中出现过两类**不计入检出**的失败，如实记录：首轮 M1 变异脚本漏闭合大括号（编译错误，修正后重跑）；
历史轮一次夹具算术错误 RED（已按严格顺序重做为真实行为 RED）。

## 五、尚未动态验证的范围（诚实边界）

- 9 个变异动态证明的是第二节列出的具体测试及其同断言面；其余约 200 项的真实性依据为第三节静态审计
  （每项均核对实际输出/磁盘字节/捕获请求/进程状态，而非 Ok/成功标志）。
- 未做专属变异的高价值候选（后续可做）：`write_atomic` 临时文件替换路径、`LimitedReader` 边界、
  快照隔离线程时序、EFBIG 前缀链（历史多轮复验但本轮未变异）。
- 环境受限：NET-N01 真实 TLS；三项待决策（并发授权保证、timeout 上限、REPL 取消）维持不动。

## 六、恢复与最终验证（本轮收尾）

- **源码完整性（哈希对照）**：M-follow 前后对 `src/*.rs` 逐文件 SHA-256 对比**完全一致**
  （变异前 `/tmp/src-hash-before.txt` 与清理后 `/tmp/src-hash-after.txt` diff 为空）；grep 无 MUTATION 仅作辅助。
  诚实注记：**历史各变异轮（M2/M5/M7/M8/auth_e02/t14/F2）当时仅做 grep 与副本删除确认，无哈希前后对照**，
  本轮不补造历史证据；本轮（M-follow 及此前同日 M1/M-3xx/M4/M6a/M6b）副本均现场清理并核对无残留进程。
- 副本清理：`/tmp/mut-follow*` 已删；pgrep 无 mut 相关进程；`/tmp/audit-part*.json` 为本表生成中间物（临时）。
- 终态命令与数量：`cargo check` 0 警告（退出码 0）；`cargo test --workspace --no-fail-fast`
  → 见下（以本轮实际输出为准）。

## 七、四项统计（修正口径）

- **逐项静态审计**：214 行（第三节，与 logs/2026-10-10-test-inventory.txt 程序校验集合相等、
  无重复/遗漏/额外、13 个分组计数全部一致）。
- **实际动态检出测试**：13 个（在对应变异或已知坏实现下实际执行并报红，其余标注一律为静态关联/未动态验证）：
  `bin_pipe_unauthorized_origin_rejects_without_request`（M1）、`redirect_targets_never_receive_credentials`
  （M-3xx、M-follow 两变异各自实跑）、`stdout_stderr_merged_exit_code_preserved`（M4）、
  `t12_invalid_batch_no_side_effects_history_stays_valid`（M6a）、`t7_request_cap_reports_exhaustion`（M6b）、
  `s3a_f1_env_trust_root_hijack_blocked`（M2）、`patch_b05_concurrent_add_single_winner`（M5）、
  `secret_e02_straddle_boundary_no_fragment`（M7）、`charset_selection_escapes_removed`（M8）、
  `auth_e02_persistence_failure_blocks_request`（历史变异 B）、`t14_eighth_request_final_reply_ends_normally`
  （历史变异 A-006）、`f2_swap_barrier_binds_write_to_validated_inode`（SR09 历史变异）、
  `chat_with_body_rejects_params_overriding_core_fields`（tools 可覆盖缺陷 RED）。
- **有效变异**：**13 个**（按唯一独立改动编号，不按功能类别）：表内十个＝M1、M2、M-3xx、M-follow、M4、
  M5、M6a、M6b、M7、M8（其中 M2/M5/M7/M8 为历史复用并注明原记录）；历史三个＝变异B（auth_e02 忽略落盘）、
  变异A-006（t14 提前上限）、变异F2（写回按路径）——经核对与表内编号互不重复。注：`chat_with_body` 的
  tools 可覆盖为**缺陷 RED**（当时真实实现即错，未施加变异），计入动态检出测试数、不计入变异数。
- **未验证范围**：其余 201 项测试为静态审计（逐项核对实际输出/字节/请求/进程，未逐一造变异）；
  环境受限（真实 TLS）与三项待决策（并发授权、timeout 上限、REPL 取消）维持不动。

## 八、修复轮增量（2026-10-10，N1–N5 安全修复）

主树枚举 214→223（`-- --list` 程序比对无遗漏）。新增 10 项（全部逐条口径，动态列＝对应修复的真实 RED→GREEN）：
- `display::tests::carriage_return_stripped_all_paths`｜N1 CR 剥离｜无替身（纯函数）｜动态检出（N1 修复 RED）
- `llm_flow::n1_real_cli_three_paths_no_raw_cr`｜N1 三路径｜真实 CLI＋HTTP mock｜stdout 零 0x0D＋历史 id 保真｜动态检出（N1）
- `exec_command::n2a_timeout_upper_bound_rejected_before_start`｜N2a 超时上限｜无替身（真实进程探针）｜零启动零副作用｜动态检出（N2a 修复 RED）
- `e2e_acceptance::n2b_repl_ctrl_c_cancels_tool_group_cli_alive`｜N2b REPL 取消｜真实 CLI（PTY，新增 setsid+TIOCSCTTY）＋HTTP mock｜进程组终止/剩余跳过/历史完整/下轮正常｜动态检出（N2b 修复 RED）
- `turn::tests::n3_batch_size_limit`｜N3 批≤32｜注入 sender（工具真实执行）｜33/300 拒零副作用、32 全执行｜动态检出（隔离副本移除上限 RED）
- `exec_command::n4a_max_output_chars_upper_bound`｜N4a 输出上限｜无替身｜拒绝零启动＋恰上限截断语义｜动态检出（N4a 修复 RED）
- `llm_flow::n4b_request_body_limit_exact_three_points`｜N4b 请求体上限｜HTTP mock｜L±发送/内容完整、L+1 零发送｜动态检出（N4b 修复 RED）
- `apply_patch::n5_patch_text_byte_limit`／`n5_update_read_byte_and_line_limits`／`n5_generated_content_limits`｜N5 三层上限｜无替身（真实 fs）｜边界三点＋目标字节不变＋无回滚｜动态检出（N5 修复 RED）

改名 1：`exec_p01_huge_timeout_investigation`→`exec_p01_huge_timeout_rejected_and_cancel_path`（新契约：超限拒绝＋取消回收，关键断言保留）。
- `e2e_acceptance::n2b2_mixed_batch_cancel_per_tool_result_shapes`｜验收轮混合批取消｜真实 CLI（PTY）＋HTTP mock｜按工具契约分形的结果结构＋目标无变化＋下轮完整历史｜动态检出（修复前 apply_patch 拿到 exec 形状的真实 RED）

**验收轮数量修正**：枚举程序比对发现 n2a 曾被 p01 重写区间误删（此前 223 缺它；重放 grep 假阳性），
已恢复；最终口径＝新增 11＋改名 1，214→225。n5_generated_content_limits 扩展生成内容字节边界
（恰 4 MiB/＋1），其定向变异（仅移除字节检查）检出。N2b 补做合格 RED（修正夹具＋断开接线）。
证据详见 logs/2026-10-10-security-fix-n1-n5.md（验收轮补强节）。

## 九、测试精简轮（2026-10-10，过度测试治理：合并去重，不删业务断言）

用户确认执行 B 档合并＋C 档并入业务测试（A 档 4 项 apply_patch 单元测试全部保留）。
**只修改测试函数与验收映射，零生产代码改动**。前后枚举程序比对：
`logs/2026-10-10-test-inventory-before-trim-225.txt` → `logs/2026-10-10-test-inventory-after-trim-215.txt`，
差集恰为下列 10 个被合并函数、无意外删除、无新增函数（225→215）。

### 断言迁移对应表

| 被合并函数 | 去向 | 场景与断言迁移说明 |
|---|---|---|
| `newline_key_rejected_without_panic` | `secret_b03_cr_and_nul_keys_rejected_without_echo` | \n 场景并入三字符循环（\n/\r/\0）；逐场景保留：发送前拒绝（panic 文案含 key）、零请求（循环内逐场景断言）、不回显原值；原"不含 bad 片段"断言保留并适用于全部场景 |
| `refresh_with_valid_empty_list_clears_models` | `refresh_success_replaces_old_list_and_keeps_selection` 第三轮 | 保留：queried=0、auto 名单空、merge 后模型空（清空而非退回基础资料）；新增"failed 为空"把原注释语义"不算失败"变为断言（加强） |
| `missing_id_fails_whole_list` | `pagination_failure_keeps_old_list_intact` 矩阵场景 missing-id | 保留：错误分类"缺少 id"、旧名单磁盘状态断言；新增请求数=1 断言（请求行为，加强） |
| `empty_page_with_has_more_fails` | 同上矩阵场景 empty-page-has-more | 保留：错误分类"分页异常"、旧名单磁盘状态断言；新增请求数=1 断言（加强） |
| （矩阵宿主）原单场景 | 同上矩阵场景 page2-fails | 原第二页失败场景保留：恰 2 请求证明读到第二页、"page 2 broke" 分类、旧名单保留 |
| `display::bare_escape_dropped` | `osc_and_string_sequences_removed` 尾段 | 悬挂裸 ESC 两断言原样保留，注释标明与"OSC 未闭合"非同义输入 |
| `display::truncate_is_char_safe` | `truncate_boundary_299_300_301` 尾段 | 纯中文 320→300 按字符计数＋短串 "abc" 原样通过两断言原样保留 |
| `s5_unicode_quotes_newlines_roundtrip` | `s4_history_preserved_no_dedup` | 特殊字符样本改为 s4 的历史首条与本轮输入（同文保不去重语义）；保留：逐字投影、`\n` JSON 转义、序列化再解析逐字往返 |
| `s7_protocol_mapping_shape` | `s3_empty_history_two_messages_two_tools` | 顶层恰 `{messages, tools}`＋tools 全 function 断言并入 s3；system 首位/user 末位与角色白名单由 s4 精确角色序断言承载（`system→user→assistant→user`＋内容等值即蕴含 content 为字符串） |
| `s8_deterministic_reassembly` | `s4_history_preserved_no_dedup` 重复组装段 | 库行为断言（两次序列化相等）移除；改为业务断言：两次组装对每次结果分别核对消息序列/逐字内容/两工具定义，输入历史快照在两次组装后不变——不以"两份相等"单独作证 |
| `exec::tests::sink_cap_exact_not_truncated` | `sink_cap_reached_exactly_then_more` 首段 | 恰满（héllo=5 码点）内容正确＋未截断断言保留；与"恰满后再来必标截断"同规则两侧合为一处 |

### 范围与保护

- 保留：A 档 4 项（`parse_empty_patch_yields_no_hunks`、`seek_pattern_longer_than_lines_is_none`、
  `seek_respects_start_index`、`change_context_seek_advances_search_start`）、`cursor_stall_fails_fast`
  与 `timeout_no_response_keeps_list`（独立契约：快败请求计数／超时路径）、N1–N5 新增 11 项全部、
  其余非目标测试。
- 文档同步：005 规格验收表 s3/s4/s5/s7/s8 行映射更新＋精简注记（验收要求未变）；本审计记录为追加，
  历史各节（214 行表、修复轮、验收轮）不重写。
- 本轮无生产代码改动，无需缺陷 RED；合并均保留并（三处）加强原有断言，无放宽。
