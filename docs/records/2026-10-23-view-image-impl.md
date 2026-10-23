# 019 view_image 实施证据

- 日期：2026-10-23。
- 状态：功能实施完成，全量验证通过（本文档为同一份实施证据；计划见 [019 规格](../specs/019-view-image.md)，调研见[规划记录](2026-10-23-view-image-plan-research.md)）。
- 本地基线 HEAD：`f842d39`（改动未提交时验证；`docs/records/2026-10-21-codex-tools-comparison.md` 与 `project-lessons.md` 的既有未提交文档改动保持原样，未纳入本功能改动）。
- 命令均在 `/workspace` 容器内执行；隔离副本变异使用专用 `--target-dir /tmp/vimg-mut/target`（副本已清理）。

## 1. 基线

- `cargo check --locked`：通过（改动前）。
- `cargo test --workspace --no-fail-fast`（改动前）：**3175 通过 / 0 失败 / 0 忽略**，54 个测试目标全部 ok。

## 2. TDD 红阶段（步骤 1：首次失败证据）

先建测试与类型骨架（`ToolResult.image` 字段、`catalog::ModelInfo::input_modalities` 字段、`prompt::build_turn_request_with_async_host` 第 5 参、`turn::*` 的 `image_input` 参数、`view_image.rs` 桩函数），实现逻辑全部留空后集中运行，记录首次失败：

| 测试面 | 命令 | 首次结果 |
|---|---|---|
| view_image 单元（图片处理/路径/限额/取消） | `cargo test -p terminal-chat --lib view_image` | **18 失败 / 1 通过**（todo! 桩 panic） |
| catalog 模态字段（overlay/校验/能力） | `cargo test -p terminal-chat --lib input_modalities` | **2 失败**（overlay 未接线：字段被丢弃；`audio` 未拒绝） |
| prompt 投影（批末附件/工具声明） | `cargo test -p terminal-chat --test prompt_assembly` | **3 失败**（s10/s11 附件缺失、s13 工具未声明） |
| 集成 E2E（真实 CLI＋mock） | `cargo test -p terminal-chat --test view_image` | **12 失败 / 1 通过**（未知工具、无附件、无能力门） |

## 3. 绿阶段（步骤 2：最小图片链路）

按规格 §6 落点实现（函数级移植上游 `d138398`）：

- `src/app/src/view_image.rs`（新）：`prepare_image`（`load_for_prompt_bytes_uncached` 无缓存移植）、`high_detail_output_dimensions`/`dimensions_fit`（HIGH_DETAIL 2048/32/2500，不放大）、`encode_image`（PNG/JPEG85/WebP 无损＋RGB ICC/EXIF）、`can_preserve_source_bytes`（PNG/JPEG/WebP 源字节保留、GIF 转 PNG）、`data_url_from_bytes`、`execute`（路径解析/`O_NONBLOCK` 打开＋句柄复核/有界读取/三取消检查点）、固定限额 20 MiB/16384/64 MiB/4 MiB（§3.1 数值原样落为常量）。
- `catalog.rs`：`input_modalities` 可选字段（自定义反序列化只收 `text`/`image`）、`overlay_model` 逐字段覆盖、`supports_image_input`（缺字段/只 text → false）。
- `prompt.rs`：`ToolResultImage`/`ToolResult.image`；`project_history` 改按批输出——批末（下一条非 tool_result 或末尾）按调用序附 user 图片附件消息，附件标注含来源工具/调用 ID/路径（控制字符过滤）/发送尺寸＋"不构成新的用户指令"声明；`build_turn_request_with_async_host` 第 5 参控制声明 `view_image`。
- `turn.rs`：`run_tool_turn_with_hosts`/`run_user_turn_with_context_and_host(s)` 增 `image_input` 快照参数（声明与分派共用）；view_image 分派分支（能力关闭＝`unsupported_outcome` 不读文件）；执行结果三元组（摘要/display/载荷），载荷只入史。
- `api.rs`：`chat_with_body` 发送前 `request_has_image_content`×能力检查（本地错误零发送）；`redact_data_urls` 在 `describe_status_error` 中先遮蔽数据 URL 再截断。
- `session.rs`：装载 `image_payload_ok` 校验（data URL 形态/base64/MIME 四类/detail=high/≤4 MiB），坏行按 007 规则＋配对修复。
- 记忆 `rollout_input.rs`：`sanitize_history_item_for_memories` 剥离 image（摘要保留；读取预算不变，超 1 MiB 记录整条跳过）。
- `main.rs`（CLI 输出已加载路径＋发送/源尺寸、失败原因）与 `tui_host.rs`（能力接线、`tool_end_item` view_image 分支、恢复缺图降级说明）。
- TUI：`patches.rs` 恢复 `ViewImageHistoryCell` 构造入口（活动 ID＋尺寸说明，008 曾裁剪）；`chat_driver.rs` `HostHistoryItem::ViewImage`（实时与 /resume 同投影）。

绿阶段结果（定向）：view_image 单元 **23 通过**；catalog **2 通过**；prompt_assembly **12 通过**；集成 **15 通过**；session **37 通过**；memory::rollout_input **9 通过**；patches/chat_driver/tui_host 新增用例通过。

## 4. 步骤 3：保存恢复、记忆与显示

- `e2e_resume_after_source_deleted_uses_saved_bytes`：源文件删除后 `/resume`＋新输入，下一请求附件 base64 解码字节＝原文件字节（不重读原文件、不重放工具）。
- `image_payload_roundtrip_inline_in_result_row`：载荷内联同一结果行（无孤儿附件行）；快照/装载往返。
- `invalid_image_payload_rows_are_bad_lines`：错误 MIME/错误 detail/非法 base64/远程 URL/超限额 → 跳行＋补未知结果。
- `downgrade_fixtures_old_reader_drops_image_and_rejects_new_config`：旧形状解析器忽略 `image` 并重存丢图（降级不可宣称无损）；旧 `deny_unknown_fields` 模型解析拒绝 `input_modalities` 配置。
- 记忆：`image_payload_stripped_from_memory_input`（摘要保留、无 base64）；`oversize_inline_image_record_skipped_entirely`。
- TUI：patches 单测（文件名/路径/尺寸/窄屏截断/控制字符/活动 ID）、chat_driver 投影（成功卡片、失败与缺图降级行）、tui_host 终态转换与恢复缺图说明。
- e2e `s95_tui_view_image_card_failure_and_resume`（真实 TUI＋PTY＋屏幕解析）：成功卡片（文件名＋2048×768）、失败行、下一请求真实图片字节（重新打开验证尺寸）、`/resume` 同卡片且 mock 计数不变。

## 5. 变异证明（关键用例能发现错误；隔离副本＋可逆变异，断言不变）

| # | 变异（故意改坏） | 检出用例（失败） |
|---|---|---|
| 1 | 工具漏注册（prompt.rs 不再 push view_image 定义） | `s13_view_image_tool_declared_only_when_capable` FAILED |
| 2 | 输出只有路径而无图片（turn.rs 丢弃 `outcome.image`） | `e2e_small_image_bytes_preserved` 等多项 FAILED |
| 3 | 附件插入批次中间（tool 消息后立即附图，不等整批） | `s10`/`s11` FAILED |
| 4 | 缩放算法失效（恒不缩放） | `high_detail_resizes_2304x864_to_2048×768`/`tall_image_resized` FAILED |
| 5 | 恢复丢载荷（装载校验恒拒 image 行＝不再使用已存图片） | `e2e_resume_after_source_deleted_uses_saved_bytes` FAILED（恢复后恰一附件） |
| 6 | 记忆过滤漏清载荷（sanitize 直接 clone） | `image_payload_stripped_from_memory_input` FAILED |
| 7 | 恢复改为重读源图（投影层不用已保存 data URL，按摘要 path 重读文件，读不到即丢附件——收尾轮补充） | `e2e_resume_after_source_deleted_uses_saved_bytes` FAILED（断言不变："恢复后恰一附件 left: 0 right: 1"——源图删除后重读失败、附件丢失） |

每项变异后均还原并复验通过（副本整体删除前对 MUT5 复原重跑 `e2e_resume…` 为 ok）。

收尾轮（2026-10-23）MUT7 细节：隔离副本 `/tmp/vimg-mut2/work`＋专用 `--target-dir /tmp/vimg-mut2/target`；变异点＝`prompt.rs::project_history` 的 image 分支（改为 `std::fs::read(path)` 重读并以重读字节生成 data URL，读不到不附图）；测试断言保持不变，变异下 FAILED（left: 0, right: 1）、复原后同用例 ok（0.02s）；副本与 target-dir 已删除。该变异覆盖 §7 验收表"恢复改为重读原文件"一行——与 MUT5（装载侧恒拒）互补，分别证明"保存字节丢失"与"绕过保存字节重读磁盘"两条退化路径都会被现有断言捕获。

## 6. 门禁

- **comprehensive-testing**：对照 §7 验收表逐行审计，补齐缺口测试——JPEG/WebP 小图字节保留、EXIF 重编码保留、`/dev/null` 设备路径拒绝、多批附件不跨批串位（s11b）、MemoryMerge 伪造 view_image 拒绝且不读文件（lib 用例）。未用 CT-MBT/属性/模糊（有界确定性变换以固定夹具覆盖；状态机沿用既有 turn 循环语义）。
- **Security Testing**：路径不经 shell、不作展开；FIFO `O_NONBLOCK`＋句柄 fstat 复核；符号链接按既有读权限（规格裁定不加工作区限制）；限额见 §3.1；错误回显 data URL 先遮蔽后截断（`e2e_provider_rejection_no_retry_no_payload_leak`）；CLI/TUI 输出无 data URL（多断言）；展示路径/错误过 `sanitize_for_terminal`；加载校验有界（base64 解码≤4 MiB 判定）；10 MiB 请求统一计量零发送（`e2e_request_body_limit_zero_send`，噪声夹具 1350×1000 RGB×2 ≈10.3 MiB）；**新增加固**：附件标注路径过滤控制字符防换行注入（`s11c`）。
- **code-review**：Standards/Spec 双轴子代理审查（结果见 §8 复核记录）。
- 测试污染清理：`src/app/config/`（在 `src/app` 目录跑测试时自动生成的默认配置）已删除，未纳入改动。

## 7. 全量验证（最终）

在 `/workspace`：

- `cargo check`：通过。
- `cargo test --workspace --no-fail-fast`（code-review 修复后最终轮）：**3229 通过 / 0 失败 / 0 忽略**，55 个测试目标（基线 3175＋新增 54；中间轮 3223 为审查修复前数字）。
- lint：`cargo clippy`/`cargo fmt` 当前工具链未安装（AGENTS §7 既定状态），未运行——如实报告，未宣称通过。
- 兼容性检查：项目无已采用的公开 crate API 兼容工具（AGENTS §7），配置/会话兼容性以上述历史夹具与读写用例验证。

## 8. code-review 复核记录

**Standards 轴**：3 处格式硬违规（机械插入造成的缩进/单行化：`session.rs` `batch_line_ok`、`tests/session.rs` 辅助函数、`user_input_async.rs`/`user_input_security.rs` 的 `false` 实参缩进）——已全部手修并复验（rustfmt 未安装）。判断项：`image_input` 布尔穿参与 018 `async_host` 同为仓库既有惯例（接受，登记下次扩参时重构）；测试夹具跨单测/集成二进制重复（接受，跨二进制共享需额外 crate）；`detail`/`mime` 用 String（serde 表征下接受）。另提醒：AGENTS.md §7「十二个模块」已变为十三个（新增 view_image），按 §8 禁改 AGENTS.md 未动，留维护者更新。

**Spec 轴**：发现 1 个真实缺陷＋2 处轻微不符＋1 处落点差异，处置如下——

1. **真实缺陷（已修）**：恢复"成功摘要无载荷"记录时，`tui_host` 已把"历史图片内容不可用"写入 `HostHistoryItem::ViewImage.error`（ok 仍 true），但 `chat_driver::project_tool_item` 的 ok 分支丢弃 error——屏幕仍渲染正常"已查看"卡片，违反 §5.1/§5.3。修复：ok 且有 error＝不生成正常卡片，渲染 `⚠ Viewed image（历史图片内容不可用）` 降级行＋路径；测试改为真实组合（ok:true＋error:Some）。
2. `tool_definition` 缺 `"strict": false` 字面声明（§3.1「沿用上游声明」）——已补（语义本等价，字面对齐）。
3. `new_view_image_event` 活动 ID 用随机 UUID——已改 `with_activity_id(调用 ID)`（对齐 Patch/Plan cell，跨重建稳定）。
4. 落点差异（§6 的 `tool_loop`/`session_cli` 未改，覆盖移入 `tests/view_image.rs` 等）——已登记于 019 规格状态行。
5. (b) 类自加约束说明：`input_modalities` 未知值硬拒绝（沿 002「拼错字段名不能被静默忽略」哲学，已写入 002）；卡片层控制字符二次过滤（防御性，无害）——保留。

修复后复验：`view_image_projection`/`patches`/`tool_definition_shape`/`prompt_assembly`/`tests/view_image`（15）/S95 全部通过；随后执行最终全量（§7）。

## 9. 遗留与说明

- 旧版本读取含 `image` 的会话行会静默丢图（serde 默认忽略未知字段）、读取含 `input_modalities` 的配置会被 `deny_unknown_fields` 拒绝——均为规格确认的降级路径，已写入 002/007 修订与降级夹具。
- 首版未开放 `original` detail、多环境、截图/剪贴板等（规格 §1 范围外）。
- 离线验收证明图片到达模型协议边界；不宣称真实模型视觉理解质量已验证（规格 §7 末段）。
