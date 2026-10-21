# 002 LLM 配置与模型目录实现（2026-10-04）

## 设计依据

- 按规格 `docs/specs/docs/specs/002-llm-api-key.md` 实现职责范围内的能力；`1.md` 仅用于核对职责边界（配置/统一数据/适配器分层），流式、取消、重试、会话未实现。
- 依赖最小化：serde/serde_json（配置文件 JSON）、ureq 2（阻塞式 HTTP，rustls）、sha2（凭证指纹）。无插件框架、无数据库、无后台服务。
- 厂商与协议：先接 DeepSeek（`.env` 已有真实 Key），协议 `openai-chat`（官方文档确认：基础地址 `https://api.deepseek.com`，`POST /chat/completions`，`GET /models`，Bearer 鉴权）。未支持协议明确报错。
- 文件拆分最小化：`src/lib.rs`（导出）+ `src/catalog.rs`（结构、内置资料、三步合并、指纹、原子保存、刷新、路由检查）+ `src/api.rs`（协议适配、分页列表、对话、公共目录同步）+ `src/main.rs`（终端循环与命令）。路由检查放库内以便测试。
- 目录文件：`config/manual.json`（手动，程序首次运行生成起步配置）与 `config/auto_catalog.json`（自动，base=内置/公共资料、query=查询名单并绑定连接身份）分别保存；最终字典每次调用在内存合成。
- 能力字段只映射厂商响应明确返回的内容（context_window/max_output_tokens/name），models.dev 只取 name 与 limit；未知留空。
- 配置目录可用 `TCHAT_CONFIG_DIR` 覆盖（测试隔离）；公共目录 URL 可用 `TCHAT_PUBLIC_CATALOG_URL` 覆盖。

## 修改文件

- 新增：`src/lib.rs`、`src/catalog.rs`、`src/api.rs`、`tests/common/mod.rs`、`tests/llm_flow.rs`、`config/`（运行生成）。
- 修改：`src/main.rs`（对话/命令接入，001 循环行为保留）、`Cargo.toml`/`Cargo.lock`（新依赖）、`docs/specs/docs/specs/002-llm-api-key.md`（实测列）、`AGENTS.md`（项目信息状态行）。

## 验证结果（证据）

- `cargo check`：0 警告。
- `cargo test`：10 个单元测试 + 12 个集成测试全部通过（mock HTTP 驱动真实业务代码，检查发送 JSON、请求次数、目录文件变化）。
- 联网验证（真实 DeepSeek Key）：
  - `/refresh`：公共目录更新 deepseek/openai；deepseek 查询到 2 个模型名单替换；openai 无凭证跳过；`config/auto_catalog.json` 中名单绑定地址/协议/凭证名/指纹（`cred_fp=3e9cb1fe...`），无 Key 泄露。
  - 真实对话：`Agent：收到`（pipe）与 PTY 交互回复；地址、Bearer 鉴权、模型 ID、回复解析均正确。
  - 错 Key：401 → 「鉴权失败（401）：Key 无效或未授权」，循环继续。
  - PTY（TERM=xterm-256color）：动画、`你>` 提示符、回复、`再见!` 全部正确。
- Python 侧 `uv run pytest -q`：2 通过（未受影响）。

## 待办 / 已知边界

- 「查询超时」场景未真实制造（本地 401/500 已覆盖失败保留逻辑），验收表标注待测。
- 重试、流式、取消、会话、多协议适配按 `1.md` 属后续阶段，本轮未实现。
- `openai` 内置条目仅连接信息，模型名单依赖公共目录同步与真实 Key。

## 修复：.env 自动读取（同日）

- 现象：用户直接 `cargo run` 报「环境变量 DEEPSEEK_API_KEY 未设置」——`.env` 只被 source/compose 类工具加载，进程环境里没有该变量。
- 修复：启动时 `catalog::load_dotenv_from(".env")` 解析 KEY=VALUE 写入进程环境；已存在的变量不覆盖，支持引号值与行内注释，文件缺失静默跳过。凭证解析与指纹逻辑不变，身份口径不受影响。
- 验证：新增单元测试（不覆盖已有变量、引号、行内注释、非法行跳过）；不 source 直接运行真实对话 `Agent：收到`，复现用户场景修复。23 个测试全过（11 单元 + 12 集成）。

## 四轮：高风险测试与最小修复（同日）

### 两项语义决策

1. **查询条目缺 ID → 整次查询失败**（报"第 N 页第 N 条缺少 id"）。理由：无法确认名单完整性就不能当成功刷新（与"断流算失败"同一哲学），保守保护旧名单；错误文案指明位置便于排查。
2. **固定配置字段拼错 → 明确报错**（全部配置结构加 `deny_unknown_fields`）。理由：拼错字段被静默忽略是"错误配置被静默采用"的入口；serde 报错会指出字段名。`request_params` 是具名字段，其内的扩展参数键不受影响，符合"不把合法扩展当错字"。

### ureq 2.12.1 核查结论（初版仅查了 docs.rs，结论"无自动重试"不严谨，已按锁定源码更正，见第五轮定点核查）

### 暴露用例（先红后绿）

6 个用例先按正确业务预期写好并确认当前实现失败，再最小修复后通过：

| 用例 | 复现的失败 | 修复 |
|---|---|---|
| debug_output_redacts_key | `Credential`/`CredOutcome` 的 `{:?}` 输出含明文 Key | 手动实现 Debug，值显示 `***` |
| route_rejects_remote_http_and_empty_base_url | 远程 http 地址被接受；空地址静默沿用 | `validate_base_url`：远程必须 HTTPS，本机地址允许 HTTP；接入 route 与 refresh |
| unknown_manual_field_rejected | `base_ur` 被静默忽略 | 全部配置结构 `deny_unknown_fields` |
| missing_id_fails_whole_list | 缺 ID 条目被静默跳过并替换名单 | 缺 ID 整次查询失败 |
| empty_page_with_has_more_fails | 空页+has_more 被当部分成功 | 报分页异常，名单不替换 |
| cursor_stall_fails_fast | 游标不前进空转 100 次请求 | 检测游标不前进立即失败（第 2 次请求后） |

### 过程中发现并修复的额外缺陷

- **非法请求头回显 Key**：含换行的 Key 会让 ureq 报错且回显 header 值。修复：`check_key_header_safe` 在构造请求头前拦截，报"密钥包含换行或控制字符"，0 请求、不回显（chat 与 list_models 均接入）。
- 临时文件名由固定 `{name}.tmp` 改为 `{name}.tmp.{pid}.{纳秒}.{序号}`，失败时清理本次文件；并发按"最后成功替换者为准"，不加跨进程锁（并发测试验证终态完整、无残留）。

### 测试总量与证据

- 63 个测试全过（单元 25 + 集成 38），`cargo check` 0 警告。六类清单覆盖：凭证四分支/空值/8KB Key/.env 首个生效/泄露扫描（401、500、断连、坏 JSON、凭证缺失全部不含 Key 与指纹）、刷新超时（读超时与连接超时分别验证）/分页上限/大名单 1 万条/身份四元组逐一失效、显式 false/0 值语义/整厂商手动新增 E2E/地址拼接/typo 报错、保留字段拦截/参数原值透传/2MB 输入/响应三分支、公共目录缺失与畸形与污染边界（连接字段不可触碰）/同步失败继续、首次运行默认文件/并发保存/遗留 tmp/只读目录/auto 损坏不自动改写。
- 真实联网冒烟：/refresh 名单替换 + 真实对话"收到"，产物含指纹不含 Key。
- 口径记录：报错不等于未计费——计费以厂商规则和实际用量为准，测试只断言客户端请求数与数据变化。
- 测试自身曾出现 ENV_LOCK 同线程重复加锁导致挂起，属测试代码缺陷，已修复（非业务代码问题）。

## 五轮：客户端重试行为定点核查（同日，纠正"ureq 无自动重试"结论）

**核查对象**：Cargo.lock 锁定的 ureq 2.12.1 实际源码（`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-2.12.1/src/`），而非文档。

### 源码依据（分三层）

1. **业务重试**：应用代码（`src/api.rs`/`src/catalog.rs`）无任何重试循环——业务层"重试只有一个负责人"的要求由我们自己保证，不存在。
2. **连接复用失败后的重发**（ureq 传输层，仅池中复用连接）：
   - 池取出时失效检测：`unit.rs:346-368` `connect_socket` 循环调用 `stream.server_closed()`，已关闭的池连接直接丢弃换新建——不产生重发。
   - 请求头发送失败：`unit.rs:253-269` `connect_inner`——复用连接上 `send_prelude` 失败时用非池连接重发一次（递归最多一次）。此时 body 尚未发送，服务器未收到请求，不构成重复处理。此路径**无幂等性检查**（含 POST）。
   - 响应阶段连接断开：`unit.rs:295-303`——条件 `err.connection_closed() && retryable && is_recycled`。`connection_closed` 定义在 `error.rs:306-325`（仅 ConnectionAborted/ConnectionReset）。`is_retryable` 定义在 `unit.rs:132-151`：幂等方法白名单 DELETE/GET/HEAD/OPTIONS/PUT/TRACE（**不含 POST**），且 body 仅 Empty/零长度可重发。
   - 判定：**对话 POST**（非幂等 + 非零 JSON body）双重不满足，响应阶段断开不会重发；**目录 GET**（幂等 + 无 body）在复用连接响应阶段断开时会自动重发一次（幂等，服务器重复处理无害）。
3. **重定向**：`unit.rs:165-242`——默认最多 5 次；301/302/303 将 POST 改写为 GET；307/308 仅无 body 的 GET/HEAD/OPTIONS/TRACE 保留原方法；Authorization/Cookie 头默认剥离（`redirect_auth_headers` 默认 Never，`unit.rs:231-236`）。

### 测试验证

- 复用现有断连测试：`disconnect_after_request_no_retry`——非池连接收到请求后断开，恰 1 次请求。
- 新增定点场景 `pooled_connection_failure_post_not_resent`（此前未覆盖失效复用连接）：mock 改造支持 keep-alive（响应不带 Connection: close，单连接处理多请求）。请求 1 正常响应进池；请求 2 复用该连接，服务器读完请求后断开。断言：第二次对话报网络错误，mock 恰好收到 2 条记录——若 ureq 重发 POST 会出现第 3 条。
- 全量回归 64 个测试通过（单元 25 + 集成 39），cargo check 0 警告。

### 是否修改业务代码

**否。** POST 不存在违反现有要求的重复发送（响应阶段被幂等门禁排除；请求头阶段的重发发生在 body 发出之前，服务器未处理原请求）。GET 的幂等重发无害。仅更正日志与报告：之前"ureq 2.12 无自动重试"的说法不成立，准确表述为"无业务级自动重试；传输层在池连接复用失败时有条件重发一次——POST 被幂等门禁排除，GET 会重发"。口径不变：报错不等于未计费，GET 重发场景服务器可能两次收到查询，但查询类请求无资金副作用。

## 六轮：变异测试——验证测试不是假的（同日）

方法：备份源码 → 每轮对一个业务判定点故意改错（27 个变异，含 M9b）→ 跑全量测试记录哪些红 → 在 finally 中还原并做 md5 校验，不一致立即停止。改错后测试仍绿的即假测试。

### 变异与捕获结果（26/26 捕获，0 个假测试）

| 变异（故意改错的内容） | 被捕获的失败测试 |
|---|---|
| M1 指纹恒为常量 | fingerprint_rules、same_cred_name_new_key_invalidates_query_list、identity_fields_each_invalidate |
| M2 连接合并忽略手动配置 | manual_overrides_only_written_fields、route_rejects_remote_http、full_custom_vendor、vendor E2E |
| M3 overlay 丢弃 display_name/context_length | manual_model_overlay_add_and_restore、explicit_false_and_zero_semantics |
| M4 route 不拦截禁用模型 | route_checks_credential_and_disabled_model |
| M5 参数能力检查恒放行 | param_capability_rules、confirmed_unsupported_param_blocks |
| M6 删除 request_params 保留字段拦截 | reserved_params_rejected_before_request |
| M7 地址校验对任意 scheme 放行 | validate_base_url_rules、route_rejects_remote_http |
| M8 合并不再按身份筛选名单 | same_cred_name、identity_fields、refresh_failure_after_key_change |
| M9 移除全部 deny_unknown_fields | unknown_manual_field_rejected |
| M10 .env 覆盖已有环境变量 | dotenv_loads_missing_vars、dotenv_duplicate_first_wins |
| M11 空环境变量视为有值 | empty_env_value_reported |
| M12 route 对不存在模型返回默认值 | route_checks_credential_and_disabled_model |
| M13 请求体 model 写死错值 | chat_sends_expected_json、bin E2E、map_key、vendor E2E、multi_turn |
| M14 解析取 reasoning_content 而非 content | 13 个集成测试全红（所有断言回复内容的用例） |
| M15 401 错误分类错位 | auth_failure_is_reported_clearly |
| M16 Key 控制字符校验恒通过 | newline_key_rejected_without_panic |
| M17 分页游标不推进（URL 固定） | list_models_reads_pagination_completely |
| M18 缺 ID 条目跳过 | missing_id_fails_whole_list |
| M19 空页+has_more 当部分成功 | empty_page_with_has_more_fails |
| M20 删除游标不前进检测 | cursor_stall_fails_fast（请求数 2→100） |
| M21 刷新失败也替换名单 | 8 个测试（含失败保留、分页中断、超时保留） |
| M22 公共目录可覆盖连接字段 | public_cannot_touch_connection_fields |
| M23 保存失败被吞成成功 | readonly_dir_write_fails_cleanly |
| M24 分页上限 100 改 10 | page_cap_stops_at_100 |
| M25 错误消息携带 Key | key_leak_scan_across_failures |
| M26 保存后误删目录内所有 tmp | stale_tmp_does_not_block_save |

### 说明与边界

- M9 初版只移除 ManualRoot 的 deny，测试仍绿——原因是拦截 `base_ur` 的是 ManualProvider 的 deny（变异点选错，非测试无效）；改为移除全部 8 处 deny 后被捕获。
- M22 初版引用了作用域外变量导致编译失败，测试未运行（无效变异），修正后捕获。
- 未做变异的测试及理由：bin 管道 E2E 与快照隔离由 M13/M21 覆盖杀伤力；大名单容量、首次运行默认内容、并发保存原子性属"实现正确性+数据形态"断言，单点变异难以构造且部分依赖时序（并发窗口），靠全字段断言与评审保障；超时类测试若变异超时参数会使用例挂起 180s，成本不成比例。
- 过程记录：第一版驱动脚本曾在异常路径上未还原源码（靠备份+md5 校验发现并恢复），随后脚本加了 try/finally 保护。
### 测试审计表（65 项，逐项）

统计口径：定点变异=专门针对该测试业务断言设计的变异；『M14 公共路径失败佐证』指该测试在 M14 下变红，但红因是对话解析公共路径被破坏，不作为该测试的定点证据。

- **catalog::tests::root_unknown_field_rejected**
  - 业务结果：仅根层未知字段的配置被明确拒绝；Err 含字段名；不改写用户文件
  - 对应变异：M9
  - 实际失败断言：unwrap_err() on Ok: ManualRoot{…}（解析成功未拒绝）
- **catalog::tests::unknown_manual_field_rejected**
  - 业务结果：厂商层未知字段（base_ur）被拒绝
  - 对应变异：M9b
  - 实际失败断言：unwrap_err() on Ok: ManualRoot{…}
- **catalog::tests::fingerprint_rules**
  - 业务结果：同 Key 同指纹/异 Key 异指纹/16 位/不含原文
  - 对应变异：M1
  - 实际失败断言：assert_ne left==right "0000000000000000"
- **catalog::tests::same_cred_name_new_key_invalidates_query_list**
  - 业务结果：同名换 Key 名单失效；换回恢复；基础连接保留
  - 对应变异：M1
  - 实际失败断言：assert !p.models.contains_key("only-from-query")
- **catalog::tests::identity_fields_each_invalidate**
  - 业务结果：地址/协议/凭证名/Key 四字段逐一失效（含控制组）
  - 对应变异：M8
  - 实际失败断言：assert contains_key("only-from-query") 未被排除
- **catalog::tests::manual_overrides_only_written_fields**
  - 业务结果：手动地址生效；协议/凭证沿用基础
  - 对应变异：M2
  - 实际失败断言：left==right（base_url 退回基础值）
- **catalog::tests::manual_model_overlay_add_and_restore**
  - 业务结果：手动新增模型/字段覆盖/删除恢复/地址恢复
  - 对应变异：M3
  - 实际失败断言：display_name 覆盖断言失败
- **catalog::tests::explicit_false_and_zero_semantics**
  - 业务结果：显式 disabled:false 恢复可选；context_length:0 保留 Some(0)
  - 对应变异：M3
  - 实际失败断言：Some(0) 断言失败（变 None）
- **catalog::tests::full_custom_vendor_from_manual_only**
  - 业务结果：全手动厂商连接字段全部来自手动
  - 对应变异：M2
  - 实际失败断言：left==right（base_url 断言）
- **catalog::tests::valid_empty_query_list_clears_models**
  - 业务结果：合法空名单记为空，不退回基础资料
  - 对应变异：未做变异验证（M2 间接红：is_empty 断言）
  - 实际失败断言：—
- **catalog::tests::disabled_model_stays_in_dict_but_marked**
  - 业务结果：禁用模型留在字典且带标记
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **catalog::tests::builtin_contains_deepseek_connection**
  - 业务结果：内置资料地址/协议/凭证引用/模型正确
  - 对应变异：未做变异验证（字面量数据）
  - 实际失败断言：—
- **catalog::tests::route_checks_credential_and_disabled_model**
  - 业务结果：禁用拦截/缺模型报错/缺 Key 报错/正常放行
  - 对应变异：M4、M12
  - 实际失败断言：实际错误：环境变量 … 未设置（禁用分支被放行后落到凭证检查）
- **catalog::tests::route_rejects_remote_http_and_empty_base_url**
  - 业务结果：远程 http 拒绝；空地址拒绝且不退回基础值
  - 对应变异：M7
  - 实际失败断言：unwrap_err() on Ok（远程 http 被放行）
- **catalog::tests::validate_base_url_rules**
  - 业务结果：HTTPS/本机 http/远程 http/空/无前缀/ftp 六分支
  - 对应变异：M7
  - 实际失败断言：unwrap_err() on Ok: ()
- **catalog::tests::param_capability_rules**
  - 业务结果：能力未知拦显式参数；已确认支持放行；已确认不支持拦
  - 对应变异：M5
  - 实际失败断言：部分放行断言失败
- **catalog::tests::credential_env_and_file**
  - 业务结果：env 正常读取、变量缺失报错、非法前缀报错
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **catalog::tests::credential_file_error_branches**
  - 业务结果：file 引用五种错误各自明确报错
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **catalog::tests::empty_env_value_reported**
  - 业务结果：空值变量报『存在但为空』
  - 对应变异：M11
  - 实际失败断言：error 为 None（空值被当有值）
- **catalog::tests::dotenv_loads_missing_vars_without_override**
  - 业务结果：.env 加载生效且不覆盖已有变量；引号/注释/非法行
  - 对应变异：M10
  - 实际失败断言：覆盖已存在变量断言失败
- **catalog::tests::dotenv_duplicate_first_wins**
  - 业务结果：重复定义首个生效
  - 对应变异：M10
  - 实际失败断言：首个生效断言失败
- **catalog::tests::debug_output_redacts_key**
  - 业务结果：{:?}/{:#?} 调试输出不含凭证明文
  - 对应变异：初版暴露用例（移除脱敏即红，修复后绿）
  - 实际失败断言：初版：assert !contains("sk-victim-secret") 失败
- **catalog::tests::corrupt_files_reported_and_not_rewritten**
  - 业务结果：manual/auto 损坏报错含路径且不自动改写
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **catalog::tests::first_run_creates_default_files**
  - 业务结果：首次运行生成两份默认文件且内容正确
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **catalog::tests::readonly_dir_write_fails_cleanly**
  - 业务结果：只读目录写失败报错/无半写/无残留/数据不坏
  - 对应变异：M23
  - 实际失败断言：unwrap_err() on Ok: ()（错误被吞）
- **catalog::tests::stale_tmp_does_not_block_save**
  - 业务结果：遗留 tmp 不阻塞保存且不误删
  - 对应变异：M26
  - 实际失败断言：unwrap() on Err NotFound（leftover 被误删）
- **auth_failure_is_reported_clearly**
  - 业务结果：401 分类为『鉴权失败』并附服务返回
  - 对应变异：M15
  - 实际失败断言：contains("鉴权失败") 失败
- **base_url_slash_and_v1_paths**
  - 业务结果：末尾斜杠不双拼；/v1 基础地址拼接正确
  - 对应变异：未做定点（M14 公共路径失败佐证）
  - 实际失败断言：—
- **bin_pipe_session_chats_and_lists_models**
  - 业务结果：管道 E2E：对话回复/模型列表/再见!/退出码 0/自动目录生成
  - 对应变异：M13
  - 实际失败断言：left==right（model 错值传导到输出）
- **chat_sends_expected_json_and_parses_reply**
  - 业务结果：请求体恰 model+messages；Bearer 正确；恰 1 次请求；回复解析
  - 对应变异：M13、M14
  - 实际失败断言：left==right（model 断言/回复断言）
- **confirmed_unsupported_param_blocks_before_request**
  - 业务结果：已确认不支持报错 0 请求；未知拦；支持则随请求发送
  - 对应变异：M5
  - 实际失败断言：0 请求断言失败（参数进入 body）
- **connect_timeout_rejected_cleanly**
  - 业务结果：连接阶段超时干净报错
  - 对应变异：未做变异验证（时序依赖）
  - 实际失败断言：—
- **cursor_stall_fails_fast**
  - 业务结果：游标不前进 2 次请求即停，名单不替换
  - 对应变异：M20
  - 实际失败断言：请求数 2→100、名单被替换
- **disconnect_after_request_no_retry**
  - 业务结果：非池连接断开报网络错误、恰 1 次请求
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **eight_kb_key_roundtrip**
  - 业务结果：8KB Key 完整进请求头不截断
  - 对应变异：未做定点（M14 公共路径失败佐证）
  - 实际失败断言：—
- **empty_page_with_has_more_fails**
  - 业务结果：空页+has_more 按失败处理，名单不替换
  - 对应变异：M19
  - 实际失败断言：failed 为空（被当部分成功）
- **key_leak_scan_across_failures**
  - 业务结果：401/500/拒连/坏 JSON/凭证缺失五场景输出无 Key 无指纹
  - 对应变异：M25
  - 实际失败断言：泄露 Key：…[key=sk-victim-SECRET-42]
- **large_input_sent_completely**
  - 业务结果：2MB 输入完整进入请求体不截断
  - 对应变异：未做定点（M14 公共路径失败佐证）
  - 实际失败断言：—
- **large_list_roundtrip**
  - 业务结果：1 万条名单数量/顺序/落盘正确
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **list_models_reads_pagination_completely**
  - 业务结果：两页收齐、after 游标正确、能力字段映射
  - 对应变异：M17
  - 实际失败断言：第二页路径/条目断言失败
- **manual_vendor_full_custom_end_to_end**
  - 业务结果：全手动厂商刷新+对话端到端
  - 对应变异：M2、M13、M14
  - 实际失败断言：model 断言失败
- **map_key_authority_in_request**
  - 业务结果：模型 map 键为权威：实际请求用该 ID
  - 对应变异：M13
  - 实际失败断言：left==right（model 断言）
- **missing_id_fails_whole_list**
  - 业务结果：缺 ID 整次失败，名单不替换
  - 对应变异：M18
  - 实际失败断言：报告无失败记录且名单被替换
- **multi_turn_config_change_next_round**
  - 业务结果：轮间换配置下一轮生效；每轮独立请求
  - 对应变异：M13（公共路径失败为主，定点未做）
  - 实际失败断言：left==right（model 断言）
- **newline_key_rejected_without_panic**
  - 业务结果：含换行 Key：前置拦截、0 请求、错误不回显
  - 对应变异：M16
  - 实际失败断言：实际错误：网络错误：Bad Header: invalid header 'Authorization: Bearer bad…'（回显）
- **no_scheme_base_url_rejected_before_request**
  - 业务结果：无 scheme 地址请求前报错 0 请求
  - 对应变异：未做定点
  - 实际失败断言：—
- **page_cap_stops_at_100**
  - 业务结果：第 100 页后中止、不发第 101 次、名单不替换
  - 对应变异：M24
  - 实际失败断言：请求数 100→10 断言失败
- **pagination_failure_keeps_old_list_intact**
  - 业务结果：分页中途失败不写半份名单
  - 对应变异：M21
  - 实际失败断言：名单内容断言失败（被清空）
- **param_values_passthrough_exact**
  - 业务结果：0/负/超大参数原值透传不裁剪
  - 对应变异：未做定点
  - 实际失败断言：—
- **pooled_connection_failure_post_not_resent**
  - 业务结果：复用连接响应阶段失效，POST 不自动重发
  - 对应变异：未做变异验证（被测行为在 ureq 源码内，见第五轮源码核查）
  - 实际失败断言：—
- **public_cannot_touch_connection_fields**
  - 业务结果：公共数据只能改能力字段，连接字段不可触碰
  - 对应变异：M22
  - 实际失败断言：base_url 被覆盖断言失败
- **public_malformed_fields_no_panic**
  - 业务结果：畸形公共数据不 panic；类型错跳过映射
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **public_missing_provider_keeps_base**
  - 业务结果：公共目录缺该厂商则基础数据不动
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **public_sync_failure_continues_query**
  - 业务结果：同步失败不中断厂商查询
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **public_unknown_providers_not_added**
  - 业务结果：未知厂商不自动新增
  - 对应变异：未做变异验证
  - 实际失败断言：—
- **refresh_does_not_change_in_flight_request**
  - 业务结果：刷新不影响进行中请求（快照所有权）
  - 对应变异：未做变异验证（需重构级变异）
  - 实际失败断言：—
- **refresh_failure_after_key_change_excludes_old_list**
  - 业务结果：换 Key 后刷新失败：旧名单排除、基础兜底、不套用
  - 对应变异：M8、M21（M15 文案断言亦红）
  - 实际失败断言：assert contains_key("deepseek-flash") 失败
- **refresh_failure_keeps_identity_matched_old_list**
  - 业务结果：失败保留同身份旧名单并报告原因
  - 对应变异：M21
  - 实际失败断言：q.models any only-from-query 断言失败
- **refresh_success_replaces_old_list_and_keeps_selection**
  - 业务结果：成功替换名单+单页 1 请求+默认模型不变
  - 对应变异：未做定点
  - 实际失败断言：—
- **refresh_with_valid_empty_list_clears_models**
  - 业务结果：合法空名单清空最终字典
  - 对应变异：未做定点（M2 间接红）
  - 实际失败断言：—
- **reserved_params_rejected_before_request**
  - 业务结果：保留字段 model/messages 报错 0 请求
  - 对应变异：M6
  - 实际失败断言：0 请求断言失败
- **response_edge_branches**
  - 业务结果：思考截断/空回复/坏 JSON 三分支文案
  - 对应变异：未做定点（M14 公共路径失败佐证）
  - 实际失败断言：—
- **concurrent_saves_last_writer_wins**
  - 业务结果：两进程并发保存，终态=某一方完整写入（不混合、不损坏、无残留）
  - 对应变异：未做变异验证（并发窗口时序依赖；原子性由 tmp+rename 实现，单点变异难以稳定复现）
  - 实际失败断言：—
- **unsupported_protocol_rejected_before_request**
  - 业务结果：未支持协议报错 0 请求
  - 对应变异：未做定点
  - 实际失败断言：—
- **timeout_no_response_keeps_list**
  - 业务结果：读超时按失败处理，名单保留
  - 对应变异：M21
  - 实际失败断言：queried 断言失败

### 本轮审计统计与修订记录

- 变异执行 27 个（M1–M26 + M9b）：捕获 27、存活 0、编译失败 0、超时 0、等价 0。
- 等价变异判断依据：等价=变异改变实现但无可观察行为差异。本次 27 个变异均至少改变一个被测试断言的可观察事实（文件内容/请求体/请求次数/错误文案/退出码），故无等价变异。
- 修订记录（更正前一轮过度结论）：
  - 原结论“没有假测试、每个测试均有杀伤力”修正为：65 项测试中，37 项有本轮业务断言失败证据（定点变异下其业务断言实际失败），1 项（debug_output_redacts_key）使用历史“红→绿”证据，27 项未做定点变异验证（理由见审计表各项）。
  - 捕获口径修正为：本轮最终执行的 27 个有效变异均被至少一个测试捕获。这不代表所有可能错误都能被发现，也不代表每个测试的全部分支都经过变异验证。
  - M9 原始变体（仅移除 ManualRoot 的 deny）在旧测试集下存活——不是等价变异（根层 typo 确实从拒绝变为放行），而是测试盲区；本轮补 root_unknown_field_rejected 后转为捕获。原始存活结果保留于前一轮记录，M9b（移除全部 8 处）单独记录且两用例均红。
  - M22 初版为编译失败（引用作用域外变量），属无效变异不计入捕获/存活。
  - “公共准备阶段失败”与“业务断言失败”已区分：如 M14 下 13 个测试变红，但其中 eight_kb_key_roundtrip、param_values_passthrough_exact、base_url_slash_and_v1_paths、large_input_sent_completely、refresh_does_not_change_in_flight_request 的红因是公共对话解析路径被破坏（准备阶段），其自身业务断言未获定点变异证据，已标注“未做变异验证”。
- 源码状态：已恢复到“新增根层测试后的基线”——即包含本轮新增的 root_unknown_field_rejected 用例，并非撤销新增测试。
- 覆盖数量核对：单元 26 + 集成 39 = 65 个测试；审计表 65 行（37 定点 + 1 历史红绿 + 27 未做 = 65）。
