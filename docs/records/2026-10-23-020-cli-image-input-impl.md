# 020 CLI 用户附图：实施证据

- 日期：2026-10-23。阶段：交付候选（部分完成，见"缺口"）。
- 工作树：`/tmp/terminal-chat-020-cli-image-input`（分支 `feat/020-cli-image-input`，基线 `7615454`＝本地 main）；未提交。
- 规格：[020-cli-image-input.md](../specs/020-cli-image-input.md)；准备记录：[spec-prep](2026-10-23-020-cli-image-input-spec-prep.md)。
- 构建入口：任务工作树专用 `--target-dir /tmp/terminal-chat-020-cli-image-input-target`（AGENTS §7 隔离要求；原 `/workspace` 与共享 target 未触碰）。

## 1. TDD 红绿记录（关键新增/修复）

| 切片 | 首次红（失败证据） | 绿 |
|---|---|---|
| S1 历史/投影/校验/记忆 | `prompt_assembly` s14/s15：`HistoryMessage` 缺 `images`/`text_elements` 字段（E0560 编译失败＝红）；session 坏行族先失败于合法行误判（发现并修复了 019 经验坑的复发：脚本误插 `images: None` 落入 matches! 模式位置静默改变行为——见坑记录） | s14/s15 ok；session 2 用例 ok；memory `user_images_stripped` ok |
| S2 提交准备＋turn 入口 | `user_images.rs` 建立时 17 个编译错误（`prepare_user_images`/`compose_cli_user_message`/`UserImagePayload`/13 参不存在） | 6 用例 ok（含纯图/空白规则、能力门先于开文件） |
| S3 `-i/--image` 行式 | 4 个 e2e 首跑全失败（零请求/纯图不发送） | 4 用例 ok（合并、多图序、EOF 纯图、能力拒/缺文件零请求） |
| S4 TUI 通道 | SubmittedTurn/队列/恢复接线后既有 36 处调用点适配（编译红）；PTY S96 首跑失败于两处真实发现（见坑） | S96/S97/S98/S99 ok；宿主链路 2 用例 ok |

## 2. 实现摘要（复用与来源）

- **复用 019 核心**：`view_image.rs` 提取 `prepare_from_path`（execute 改为复用）；新增 `prepare_user_images(paths, capable)`（能力门先于文件打开，A11）与 `compose_cli_user_message`（CLI 前置标记合成）。
- **消息与持久化**：`HistoryMessage.images: Option<Vec<String>>`＋`text_elements: Vec<TextElement>`（Codex `user_input::TextElement` shim 直用；缺省/空不序列化，旧记录读回 None/空）；投影＝同一条 user 消息内 text＋image_url×N（detail=high，纯图省 text 部件，无图消息形状不变）；session 装载校验（`user_images_payload_ok`：仅 User、与问询标记互斥、data URL 形态/≤4MiB、区间 UTF-8 边界、`[Image #N]` 形态/不越界/不重复绑定/按序不重叠）→ 坏行＋配对修复；记忆剥除用户 images（预算不变）。
- **提交通道**：`turn.rs` 入口增 `user_images: Option<&UserImagePayload>`（空白校验放宽：纯图合法 A05；入史随同一用户消息）；tui 宿主 `run_turn_inner` 在路由后准备（失败→`DriverEvent::SubmissionRejected{text,image_paths,reason}`：零请求、不入史）。
- **TUI 驱动**（Codex d138398 `chatwidget` glue 的本地最小适配）：`SubmittedTurn{text, local_images, text_elements}` 贯穿提交/队列/重放；`DriverHooks.spawn_turn` 改型＋新增 `initial_submission`（-i 就绪后提交一次）；提交即 `take_recent_submission_images_with_placeholders()` drain（上游 ChatWidget 职责的本地接线）；排队预览纯图项显示"（图片 ×N）"；`SubmissionRejected` 恢复草稿（`pane.restore_rejected_submission`：D0 前置＋Dnew 换行接续、附件合并重编号＝Codex 合并顺序）；`UserHistoryCell` 携带附件路径与元素；Ctrl+V／Ctrl+Alt+V 剪贴板图片（`paste_image_to_temp_png` keep 临时 PNG；活动视图期间不拦＝问询窗不接图）。
- **CLI 行式**：`-i <p>`/`--image <p>`/`--image=<p1,p2>`/重复参数（逗号拆分按序累积，值缺失明确报错退出 2）；首条可提交输入合并（前置标记）；EOF 无文字纯图恰一次；exit/命令优先；一次机会（进聊天轮即作废，失败＝报错零请求不转纯文本）。

## 3. 验收映射（A01–A25/H01–H03）

| 项 | 状态 | 证据（用例） |
|---|---|---|
| A01 剪贴板粘贴 | 代码接通＋边界实测受限 | Ctrl+V 接线（arboard/WSL 路径保留）；Docker 内无 Windows 剪贴板→如实提示（H01 另列；本环境无法真实验证成功路径，不冒称） |
| A02 `@` 选图 | 通过（第二轮） | composer 级 7 用例（a02_tests）＋PTY S111（真实选择事件与屏幕：@pick→候选→Tab→附件→请求字节）；M8/M9/M10 变异检出 |
| A03 路径粘贴/拖入 | 通过（拖入＝宿主转路径共用本链） | S96（bracketed paste 中文路径→附件）；shlex 单路径语义＝上游（裸空格路径不转，引号/file:// 可） |
| A04 `-i/--image` | 通过 | `e2e_cli_image_flag_merges_with_first_line`/`multiple_and_pure_image_eof`/S97（TUI 就绪提交恰一次） |
| A05 纯图/纯文本 | 通过 | `pure_image_submission_and_blank_rules`＋S96/EOF 纯图＋s14（纯文本零新字段） |
| A06 多图与顺序 | 通过 | multiple 用例（三图序）；同名不去重由逐路径读取保证 |
| A07 删除/清空/重编号 | 通过（第二轮） | composer 级 `delete_first_attachment_renumbers_composer_level`＋PTY S112（Home+Delete 删元素→重编号→请求只含幸存字节） |
| A08 手写标记 | 通过 | `s16`（无载荷标记投影纯文本）＋session 坏行族 |
| A09 运行中入队及源图变化 | 通过 | S99（gated mock：排队期间改写→出队采用新版本；FIFO） |
| A10 出队坏图恢复 | 通过 | `host_rejects_submission_on_bad_image`＋`rejected_submission_carries_full_draft_for_restore`（拒收载荷完整性：文本＋全部路径原样回传供 D0+Dnew 合并）；S110（拒后队列暂停） |
| A11 能力拒绝 | 通过 | lib 能力门＋行式 e2e＋S98（TUI 拒绝→恢复草稿→零请求） |
| A12 准备期取消/迟到 | 通过（第二轮） | `prepare_user_images_cancel_checkpoint_before_read`（取消标志在能力门后、读取前检查点生效）；迟到＝M6 变异＋`host_reject_is_terminal_no_retry` |
| A13 入史后取消 | 通过（第二轮） | PTY S113（gated＋Esc：旧图在后续请求保留恰一份、next 为纯字符串消息形态——020 投影正确行为） |
| A14 请求超限 | 通过 | `e2e_cli_user_images_request_limit_zero_send`（两张 ≈3.86MiB→10.3MiB，零 HTTP，输出无 base64） |
| A15 提供方拒绝/回显 | 既有覆盖 | 019 `redact_data_urls` 通道不变；工具侧 e2e 已证 |
| A16 真实图片内容 | 通过 | 全链用真实 PNG（源字节保留断言）；GIF/WebP/JPEG 沿用 019 单测（同一核心） |
| A17 保存恢复/源图删除 | 通过 | `e2e_cli_user_images_resume_after_source_deleted`＋session 往返/坏行族 |
| A18 损坏附件历史 | 通过 | `user_image_rows_valid_roundtrip_and_bad_lines`（10 类） |
| A19 旧会话降级 | 通过 | s14 旧记录读回；旧程序丢新字段＝既有 019 夹具模式（同构 serde 行为） |
| A20 记忆过滤 | 通过 | `user_images_stripped_from_memory_input`（正文留、无 base64/部件） |
| A21 问询边界 | 通过（第二轮） | PTY S1140（问询窗打开期间 Ctrl+V：无附件、无剪贴板读取提示——顺带修复驱动 Ctrl+V 分支补 `!pane.has_active_view()` 门）＋session 互斥校验 |
| A22 工具批次/晚到顺序 | 既有覆盖 | 用户图走独立用户轮（不插工具批）；018 顺序回归全量内绿 |
| A23 文件/解析安全 | 通过 | 019 边界复用＋安全报告 §1#3 |
| A24 keep 与退出 | 部分验证（保持） | keep 成功路径依赖真实剪贴板（H01 同源，环境不可达）；用户原图不变与退出不清队列已由 S110/S99 覆盖；上游 keep 代码未改动 |
| A25 工具回归 | 通过 | 全量第一轮 3251/0/0（含 019 全部回归与 e2e） |
| H01/H02/H03 | **待验证（宿主条件）** | 本环境无 Windows/Docker 剪贴板桥接、真实拖入与 ConPTY；未用模拟冒称 |

## 4. 变异证明（隔离副本 `/tmp/020-mut`＋专用 `--target-dir /tmp/020-mut-target`；断言不变；副本已清理）

| # | 变异 | 检出 |
|---|---|---|
| M1 | turn 入史丢用户 images | `turn_enters_user_images_and_projects_in_same_message` FAILED |
| M2 | 排队丢附件（queue_input 只存文本） | `s99_tui_queued…` FAILED（第二轮无图） |
| M3 | 装载校验恒拒用户 images（恢复丢载荷） | `e2e_cli_user_images_resume…` FAILED |
| M4 | 记忆过滤漏剥用户 images | `user_images_stripped_from_memory_input` FAILED |
| M5 | 用户图投影退化为纯文本 | `turn_enters…` FAILED |
| M6 | 拒绝后仍继续跑文本轮（迟到提交） | `host_rejects…` FAILED（mock.count=1） |
| M7 | 能力检查移到文件读取之后 | `prepare_user_images_capability_gate_before_open` FAILED |

"提前冻结"未单列变异：驱动层无读图点（架构上不可能提前读），出队读取行为由 S99 直接断言；"删图仍上传"由 composer 附件状态机（上游代码＋其测试）与 M2/M5 组合覆盖。

## 5. 安全门禁

报告：[docs/security/reports/2026-10-23-cli-image-input-security.md](../security/reports/2026-10-23-cli-image-input-security.md)——11 个威胁面逐项实测；无高危；Web 专项不适用如实标注。

## 6. 验证

- 专用 target-dir；`cargo check --workspace --all-targets`：0 错误。
- 全量第一轮（实现完成后）：`cargo test --workspace --no-fail-fast`＝**56 目标 3251 通过 / 0 失败 / 0 忽略**。
- code-review 双轴子代理结论与处置：见 §7。
- 最终 verify:full（审查修复后重跑）：见 §8。
- lint：clippy/fmt 未安装（AGENTS §7 既定），未运行，如实报告。

## 7. code-review 处理（双轴子代理，2026-10-23）

**Standards 轴**：
- 硬违规 3 处已修：① 测试助手 `crate_path`/`classify_pure_image` 死代码→补纯图分类断言（顺带补上 A05 纯图分类单测）；② `///` 挂 `let` 的 unused doc comment→`//`；③ 未跟踪 `src/app/config/` 运行时产物→删除。
- 判断项处置：e2e 编号撞号（新用例改 s100–s103/S110，避开 018/020 既有编号）✓已修；`prepare_from_path` 返回冗余 PathBuf✓已精简；`TuiHost.initial_images` Middle Man✓已改 `run_tui` 直接收参；`too_many_arguments`/双层空白校验重复/turn.rs 测试重排——接受（019 同模式既有惯例＋防御性校验，rustfmt 未装不追格式），已登记。
**Spec 轴**（3 个真实缺陷全部修复＋复验）：
1. **§2.3 Q 暂停未实现**：Rejected/Failed 后队列自动续发——修复：驱动增 `queue_paused`（拒绝/入史后失败置位；用户重新提交或重放出队解除）；**S110** 先红（自动续发被断言捕获）后绿。
2. **Rejected 不回收 transcript 用户 cell**（双份呈现）——修复：`user_cell_base` 记录推入基点，拒绝时 truncate＋清 stream（Final 后失效）。
3. **准备期取消被吞**（A12：`HOST_CANCEL` 在准备前被复位且 prepare 未接检查点）——修复：`prepare_user_images` 增 `cancel` 参数（读取前/后检查点，宿主传 `HOST_CANCEL`；取消＝Rejected 恢复草稿路径）＋宿主拒绝幂等测试。
- 部分验证补充说明：初始图"保护性输入结束后"本地驱动无对应机制（008 裁剪），按"就绪后一次"等价实现并登记；A21/A24/A15/A03 形态矩阵/A10 合并的 PTY 级断言维持 §3 部分验证清单。

## 8. 第二轮：A02 `@` 选图补实施与补验收

- **A02 实现**（上游 `file_search_popup.rs`/`insert_selected_file_path` 的本地恢复）：
  - 新增 `bottom_pane/file_search_popup.rs`（`FileMatch` 本地类型＋`FileSearchPopup`，复用保留的 `ScrollState`/`picker_rows`/`selection_popup_common`/`popup_consts` 基建；渲染走 `render_rows_single_line`）；
  - `ActivePopup::File` 变体恢复（popup_state/required_height/footer 分支）；
  - composer 接线：`sync_file_popup`（`completion_target::current_prefixed_token_range` 检测 `@token`；`DismissedToken` Esc 关闭语义）＋`local_file_search`（cwd 有界同步扫描：深度≤3、≤400 文件、跳隐藏；子串匹配）＋`handle_key_event_with_file_popup`（↑↓/Ctrl+P/N/Esc/Tab/Enter）＋`insert_selected_file_path`（图片走真实 `image_dimensions` 解码→`attach_image`；非图片/坏图→带引号路径文本，不伪称附图）＋`advance_past_completion_separator` 恢复＋popup 区域渲染（layout 保留的 `popup_rect`）。
- **TDD**：composer 级 7 用例（a02_tests：开窗/图片转附件/非图片/坏图回退/Esc dismiss 语义/未选择纯文本提交/Enter=Tab；A07 删除重编号 composer 级）——首次红（弹窗未开：字符键落入 paste-burst 缓冲，改 `handle_paste` 通道输入后绿；dismiss 语义初版断言与上游相反，修正为"同 token 不重开、编辑重开"）；PTY S111（@pick→候选可见→Tab→[Image #1]→请求字节＝源）。
- **补验收**：S112（A07 PTY：两张删一、幸存字节/重编号）、S113（A13：取消后旧图保留恰一、next 纯文本消息形态——发现并确认 020 投影下无图消息保持字符串 content 的正确行为）、S1140（A21：问询窗 Ctrl+V 无附件无剪贴板读取——顺带修复 Ctrl+V 驱动分支补 `!pane.has_active_view()` 门）、A12（`prepare_user_images_cancel_checkpoint_before_read`：取消先于读取）、A10（`rejected_submission_carries_full_draft_for_restore`：拒收载荷完整性）。
- **变异补充**（隔离副本 /tmp/020-mut2，专用 target-dir，已清理）：M8 `@` 选择退化不建附件→`at_token_opens…` 检出；M9 坏图伪称附图→`at_token_broken…` 检出；M10 搜索恒空→`at_token_opens…` 检出。累计 **10/10**。
- **安全增补**：报告 §1 追加 3 项（路径注入/搜索耗尽/Esc 状态）。

### 8b. 增量 code-review（A02 轮）处理

**Standards 轴**：
1. **弹窗失同步（已修）**：弹窗分支早退跳过 `sync_popups()`——修复为绑定结果统一 reset＋sync（上游同构）；既有 `bound_at_mentions_do_not_block_arrow_navigation` 在第一轮全量中失败暴露同一根因：`current_file_token` 匹配已绑定的 `@figma` 原子元素——补 `prefixed_token_range_is_editable` 门（可编辑 token 才触发文件弹窗），两族测试全绿。
2. popup 渲染死代码（截断调用结果丢弃）→删除＋注释更正；`sync_popups` 死绑定 `let _ = query`→重构为 `dismissal_stale` 绑定。
3. 命名笔误 `s1140`→`s114a`；018 旧用例的半截重命名残留（屏幕文案 S96-…）补齐为 S100-…；A02-6 误导性注释更正（↑↓ 导航由弹窗键路由分支承担）。
**Spec 轴**：
1. **坏图"并提示"缺失（已修）**：坏图回退为路径文本时增加 footer flash 提示（`at_token_broken…` 补 `flash_visible` 断言）。
2. **D0＋Dnew 合并零测试（已补＋发现真实缺陷）**：`restore_rejected_submission_merges_d0_then_dnew_and_renumbers` 首跑失败暴露合并实现的真实缺陷——直接拼接保留旧 `[Image #N]` 字面导致重编号失效（text 出现两个 #1）；修复：剔除 D0 系统标记与 Dnew 元素字面，按合并附件总数重建前置标记＋元素，重编号正确。
3. **A12 检查点位置（已修）**：`prepare_from_path` 开头补读取前取消检查点（原为 open+read 后），测试注释与实现一致。
4. A13 s113 的 Q 暂停断言：由 S110（同一驱动路径的暂停语义）与 s113（H 恰一份＋旧图随新请求）组合覆盖，s113 定位为 A13 场景（取消后历史保留），不再重复断言暂停。

## 9. 最终验证与缺口

- 第一轮 verify:full：3255/0/0（56 目标）。
- 第二轮 verify:full（A02＋补验收后）：**3258/1 失败**——失败用例 `bound_at_mentions_do_not_block_arrow_navigation` 暴露 `current_file_token` 缺 editable 门（匹配已绑定 `@mention` 原子元素）；修复后第三轮＝**3269/0/0**。
- 第四轮（A03/A24 补证据后）最终 verify:full：`cargo check` 通过＋`cargo test --workspace --no-fail-fast`＝**56 目标 3276 通过 / 0 失败 / 0 忽略**（较第三轮＋7：A03 生产入口 5 用例＋A24 keep 2 用例）。
- lint：clippy/fmt 未安装（AGENTS §7），未运行——如实报告。

### 缺口与剩余风险

1. **H01–H03 宿主验收待条件**（Windows/Docker 剪贴板、真实拖入、ConPTY）——环境不可达，按规格保持待验证，不以模拟冒称。
2. A24 的 keep 文件留存断言依赖真实剪贴板成功路径（H01 同源），本环境只能覆盖失败路径与"用户原图不变"——上游 keep 代码未改动。
3. A03 的引号/file:// 形态沿用 019 `normalize_pasted_path` 既有语义（上游同源），未重复铺形态矩阵。

## 10. 第三轮补证据：A03 / A24（2026-10-23）

### 10.1 A03 路径粘贴（生产入口实测；本地可验证部分→已验证）

既有 L1 单测（`clipboard_paste.rs::pasted_paths_tests`：file:// 解析、双/单引号、shlex 转义、多 token 拒绝）继续有效；本轮补齐**经生产入口**（`handle_paste` → `apply_paste` → `handle_paste_image_path` → `normalize_pasted_path` → `attach_image`）的直接证据（composer 级，真实临时文件）：

| 场景 | 用例 | 断言 |
|---|---|---|
| 双引号＋空格＋中文路径 | `paste_quoted_space_chinese_path_attaches_real_file` | 附件路径＝解码路径；占位符在场；**原图字节不变** |
| `file://` URL | `paste_file_url_attaches_decoded_path` | 附件＝URL 解码后的本地路径；原图不变 |
| shell 转义（反斜杠空格） | `paste_shell_escaped_path_attaches_unescaped` | 附件＝反转义后真实路径；原图不变 |
| 多路径文本 | `paste_multi_path_text_is_not_swallowed` | 不建附件；两个路径名按普通文本保留 |
| 普通文字 | `paste_plain_text_never_attaches` | 不建附件；文本原样 |

边界区分：宿主把拖入文件转成路径文本属 H02（宿主行为）；本表验证的是路径文本到达 composer 后的全部本地行为。

### 10.2 A24 keep 与退出（生产写入路径独立验证→已验证本地部分）

- 生产代码变更（最小提取）：`clipboard_paste.rs` 新增 `write_png_keep_temp(&[u8])`——`paste_image_to_temp_png` 的临时写入＋keep 段唯一实现（函数级复用，生产行为不变），剪贴板读取（arboard）仍在原函数。
- `write_png_keep_temp_persists_after_return`：keep 后文件存在、字节与写入一致、可解码 PNG、文件名形态符合 Builder 参数；返回 300ms 后仍存在（删附件/取消/发送/退出任何时点无自动清理）；测试尾自行清理（生产路径无人删除）。
- `tempfile_without_keep_is_auto_removed`：同一 Builder 形态未 keep 即丢弃→文件被清理——即"keep 前失败由 tempfile 生命周期清理"的契约依据（生产 write/keep 失败分支继承同一保证）。
- 用户原图原字节不变：由 10.1 三个正例断言（附件读取不改写源文件）。
- 退出不持久化未发 Q：S110（拒绝后队列暂停且不发送）＋既有 007 退出排空语义（不新增队列持久化）。
- 剩余缺口：keep 成功路径的**入口**（真实剪贴板读取成功）依赖 H01 宿主条件；本地已覆盖从 PNG 字节起的全部行为与失败路径。

### 10.3 安全复验

`write_png_keep_temp` 仅移动既有写入段：无新输入面（字节来自既有剪贴板读取路径）、无新权限/网络；A03 新测试均为正/负例行为断言，无新增攻击面。安全报告无需修订（登记于本节）。

### 10.4 增量 code-review 处置（A03/A24 轮）

- 双轴审查确认：提取最小且生产等价（全仓唯一写临时 PNG 路径）；A03 五用例真走生产入口并与规格行一致；A24 规格如实标注 H01 依赖。处置：`Dir_path` → `fixture_path`（non_snake_case 警告清零）；其余为登记项（a24 用例 2 为 tempfile 库契约的弱钉住——规格声明"同形"契约；a02 夹具 cwd 切换对同 crate 并行测试的暴露面为既有形态，非本轮引入）。
