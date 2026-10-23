# 020 CLI 用户附图：安全攻防测试报告

- 日期：2026-10-23。范围：`feat/020-cli-image-input` 未提交候选（用户附图五入口链路、`prepare_user_images`、`HistoryMessage.images/text_elements`、TUI 驱动队列与恢复、`-i` 启动参数）。
- 方法：按仓库安全门禁要求先审代码再以可运行用例实测（本功能为本地 CLI，无 HTTP 业务接口——Web 项"数据越权/管理员/订单/短信"不适用，如实标注；攻击验证以 Rust 测试为脚本，覆盖 020 §5.3 列出的重点）。

## 1. 威胁面与实测结果

| # | 攻击面 | 攻击方式（可运行用例） | 结果 |
|---|---|---|---|
| 1 | 能力/来源伪造：text-only 模型诱导附图 | `prepare_user_images_capability_gate_before_open`（缺失路径+能力未知） | 防住：能力错误先于文件打开，零请求（A11）；e2e `e2e_cli_image_flag_capability_rejected_zero_request` / `s98_tui_capability_reject_restores_draft` |
| 2 | 手写 `[Image #1]` 伪造附件标记 | `s16_handwritten_marker_without_payload_projects_plain_text`：无 `images` 载荷的标记按纯文字投影 | 防住：无 image_url/data URL；标记与图片的绑定以保存的 `images`/`text_elements` 为权威 |
| 3 | 路径与文件类型：目录/FIFO/设备/非图片 | 019 既有 `execute_directory_device_fifo_missing_rejected`/`device_path_rejected`＋`prepare_user_images_invalid_paths_fail_with_path`（复用同一 open_regular_file/read_bounded/prepare 核心） | 防住：`O_NONBLOCK` 打开＋句柄 fstat 复核；内容识别非扩展名；错误带路径 |
| 4 | 解析耗尽：超限图片/解码预算 | `e2e_cli_user_images_request_limit_zero_send`（两张 ≈3.86MiB→10.3MiB 超请求上限）；019 限额单测（20MiB/16384/64MiB/4MiB）沿用 | 防住：准备后单图 4MiB、最终 JSON 10MiB 统一计量零发送；输出无 base64 |
| 5 | 持久化投毒：坏 data URL/越界标记/远程 URL/assistant 带图/问询标记带图 | `user_image_rows_valid_roundtrip_and_bad_lines`（10 类坏行族） | 防住：按 007 坏行跳过＋配对修复；不为校验访问 URL 或源路径（A18） |
| 6 | 提炼/日志泄漏 base64 | `user_images_stripped_from_memory_input`（用户图剥除）；`e2e_cli_user_images_request_limit_zero_send` 断言 stdout 无 base64 | 防住：用户 images 与工具 image 同规则剥除；错误回显遮蔽沿用 019 `redact_data_urls` |
| 7 | 终端注入：路径/文件名控制字符 | 019 `s11c`（附件标注控制字符过滤）；CLI `say` 统一 `sanitize_for_terminal`（准备错误带路径输出） | 防住：显示副本过滤；模型请求载荷不受显示过滤改写 |
| 8 | 远程 URL/凭证边界 | 坏行族含 `https://` data URL 拒绝；不下载 http(s) 图片；origin/重定向限制沿用 002/003 | 防住：`file://` 只转路径；无新增出站地址 |
| 9 | 问询/MemoryMerge 越界 | session 校验：`user_input_event.is_some() && images` → 坏行（A21）；MemoryMerge 分派沿用 019（伪造 view_image 落未知工具） | 防住：问询回答记录不带附件；合并轮工具面不变 |
| 10 | 取消/迟到残留 | `SubmissionRejected` 语义：消息不入史、零请求、草稿恢复（`host_rejects_submission_on_bad_image`、变异 M6 证明测试可检出"拒绝后仍提交"） | 防住：一次拒绝不残留可重发载荷；排队项保留待用户处理 |
| 11 | 提前冻结/排队时序攻击（源图交换） | `s99_tui_queued_submission_reads_image_at_dequeue_time`：排队期间改写源图→出队采用新版本 | 行为符合规格（正式提交时读取；入史后内容固定由 A17 恢复测试证明） |

### 增补（2026-10-23 第二轮：A02 `@` 选图实施后）

| # | 攻击面 | 攻击方式 | 结果 |
|---|---|---|---|
| 12 | `@` 弹窗路径注入（选中项转附件/路径文本时注入控制字符/引号） | `insert_selected_path` 含空格路径加引号（上游语义）；占位符元素原子化（textarea 元素不可注入标记文字）；变异 M9 证明"坏图伪称已附图"被 `at_token_broken…` 检出 | 防住：图片走真实解码（image_dimensions）；非图片按带引号路径文本；标记伪造路径（A08/s16）无载荷不附图 |
| 13 | `@` 同步搜索资源耗尽（大目录扫描阻塞 UI） | `local_file_search`：深度≤3、文件数≤400、跳隐藏——有界扫描 | 防住：硬上限＋候选截断（MAX_POPUP_ROWS×4） |
| 14 | `@` 弹窗 Esc 后重开抖动/状态残留 | `DismissedToken`（范围＋文本匹配）——同 token 不重开、编辑后重开（`esc_dismisses_and_token_edit_reopens`） | 防住：上游 dismiss 语义恢复 |

## 2. 发现的问题与处置

- 无高危发现。审查中确认两处设计边界并已在规格登记：① `@` 选图入口未实施（无新增攻击面，纯缺失）；② Ctrl+V 在 Docker 内通常读不到 Windows 剪贴板——失败路径如实提示并保留草稿，不冒称成功（H01 真实宿主验证另列）。
- Web 专项（数据越权/功能越权/身份认证/字段提权/前端绕过/短信防刷）：不适用（本地 CLI，无多用户/HTTP 业务接口），未强行构造。

## 3. 结论

用户附图链路的安全边界（能力门、文件类型、限额、坏行、载荷隔离、终端过滤、恢复校验）均有可运行证据支撑；7/7 变异证明关键回归可检出对应退化。无阻断项。
