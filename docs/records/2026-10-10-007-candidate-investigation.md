# 007 候选调查：会话保存与恢复（2026-10-10，仅调查不实现）

## 推荐结论

**确认"会话保存与恢复"为 007**，候选规格：`docs/specs/007-session-persistence.md`（§6 六项契约待用户确认）。

依据（均为仓库现有证据）：
1. **是既有路线的显式留白**，非临时起意：001 规格（`docs/specs/001-cli-repl-loop.md` §1"历史仅内存内……不持久化"）与 006 规格（状态行"持久化不在范围"）都明确推迟本能力。
2. **母本有成熟方案可复用**（见下节），且本地 `HistoryItem`（`src/prompt.rs`）已是唯一历史真源，持久化是薄层。
3. **竞争候选优先级更低**：N6/N9 为安全报告自评低优先级；002 重试机制未实现但 006 坑003 已声明重试与已执行工具副作用冲突、需独立设计，不宜插队；并行/流式/审批/压缩均无近期需求信号。

## Codex 母本证据（固定提交 2fdf047c9631c9ed01a31b62efb7891718a931a8）

经 GitHub API/raw 实查（非记忆引用）：

- crate 位置：`codex-rs/rollout/src/`（`core/src/rollout.rs` 为薄再导出层；workspace 根目录列表确认 crate 名）。
- **写入**（`recorder.rs`）：`JsonlWriter::write_line`＝`serde_json::to_string`＋`\n`＋`write_all`＋每行 `flush`，文件 `.append(true)`（O_APPEND），**无 fsync**；后台写者任务（channel 缓冲 256）＋写失败恢复模式（重开重试一次、未写项暂存）。
- **行 schema**：`RolloutLine{timestamp(RFC-3339), ordinal?, #[serde(flatten)] item: RolloutItem}`；首行 `RolloutItem::SessionMeta`。
- **加载**（`recorder.rs::load_rollout_items`）：跳过坏行并计 `parse_errors`，剔除 legacy ghost_snapshot——**本规格不采纳该容差**（fail-closed，理由见规格 §3）。
- **命名/布局**（`rollout_file_name.rs`）：`rollout-<YYYY-MM-DDTHH-MM-SS>-<thread_id>.jsonl`，位于 `SESSIONS_SUBDIR/YYYY/MM/DD/`。
- **Resume**：`RolloutRecorderParams::Resume{path}`、`list_threads`/`find_latest_thread_path`/`find_thread_path_by_id_str`。

## 本地复用面（实查）

- `src/prompt.rs`：`HistoryItem`/`ToolCallRecord`/`HistoryMessage`（无 serde 派生，投影 `project_history` 手写）；请求重建入口 `build_turn_request` 单一。
- `src/catalog.rs`：`user_dir()`（`TCHAT_USER_DIR` 可覆盖＝测试隔离入口）、`ensure_private_dir`（0700）、`write_private_file`（**整文件 0600 写＋父目录收紧＋写后复读确认**，`authorized-origins.json` 先例）——快照式 `/save` 直接复用，无需追加写。
- `src/main.rs`：`handle_input` 斜杠命令分派（现 `/models`、`/refresh`，未知命令帮助行）、`prompt_y_n` 交互确认（读取错误即拒绝）、授权门非交互拒绝先例。
- 安全边界：凭证值不在 `HistoryItem`（现状，需测试锚定）；工具输出可含秘密→0600/0700 私有目录强制。

## 关键设计判断（摘要，详见规格 §3）

- 快照式（显式 `/save` 整写）而非 Codex 自动追加：复用面最大（`write_private_file` 现成）、无后台写者/恢复模式/时钟依赖（文件不含时间戳，符合 AGENTS.md §3 测试规则）。
- 加载 fail-closed＋配对完整性校验（tool_calls↔tool_result id 集合一致）：截断最易切断配对，静默装载会产生非法请求。
- 恢复零重放：工具仅由新模型 tool_calls 触发（006 现状），规格以验收钉死。

## 未决事项

规格 §6 A–F：加载失败语义、命令面/命名、覆盖与丢弃确认、会话不绑定模型、大小上限（天然有界 vs 显式）、首条 user 约束。确认前不实现。

## 定稿记录（2026-10-10 第二批）

- 用户指令确认复用优先级约束并定稿：以规格为行为与验收契约，取材顺序＝本地已有实现 → Codex 母本 → 新写；完全符合直接复用、部分符合最小适配、无法复用须说明原因只补缺失；复用代码同口径过验收。
- §6 A–F 全部按候选稿建议裁决（B 经用户指令再次确认：显式 `/save`，不采纳 Codex 自动记录/后台写者/O_APPEND/恢复模式）。
- 规格新增：§3 复用决策规则、映射表"不采纳"行（自动逐轮记录）、缺失部分清单（serde 派生、会话文件层、加载校验、命令接线四处，逐项注明不可复用原因）、§5 复用代码同口径验证句。
- 建立实现分支 `dev/007-session-persistence`（基线 `0d1bb80`）。规格与本文档暂未提交，随实现 /goal 首个提交入库。

## 第二批裁决修订（2026-10-10 第三批记录）

用户审查否决候选定稿的多处表述，第二批裁决已全部同步进规格：

1. **E 大小上限**：由"10 MiB 请求限天然有界"改为 **32 MiB（33,554,432 字节）显式上限，保存与加载均执行**；加载超限不得先整体读入（复用 `api.rs` `LimitedReader` 有界读模式，最小适配换 32 MiB 界）。
2. **F 空历史例外**：允许仅含合法 meta 的空历史快照；非空历史首条须为 user 文本。
3. **凭证边界**：删除"凭证值不在历史中"的错误保证——完整保存用户输入与工具输出**可能含秘密**；契约改为"应用不主动保存鉴权配置＋不新增脱敏"，防护依赖 0700/0600。
4. **完整末行无换行可接受**：EOF 即行尾，仅 JSON 截断/非法才拒绝（原"去掉末行换行→拒绝"有误）。
5. **保存失败保护旧快照（写入函数核实，只读）**：
   - `write_private_file`（`catalog.rs:877-899`）：`truncate(true)` 原地打开后 `write_all`（`catalog.rs:885-893`）——写中途失败（EFBIG/盘满）旧文件已被截断，**不满足**"保存失败不得损坏旧快照"；
   - `write_atomic`（`catalog.rs:1017-1049`）：独立 tmp（pid/纳秒/序号命名、0600）完整写盘后 `rename` 替换、失败仅清理本次 tmp（`catalog.rs:1046-1048`）、旧文件成功替换前不动——**满足**；但为私有 fn（无 `pub`），规格记为**最小适配**（开放或同模式新写薄层）；继承 002"遗留 tmp 不阻断后续保存"语义（`stale_tmp_does_not_block_save` 锚定）。
   - 候选稿曾以"加载侧 fail-closed 兜底半写"替代旧快照保护——该表述已删除（用户否决：不得以加载拒绝坏文件代替旧快照保护）。
6. **配对校验四拒绝**：明确重复、缺失、多余、交错四类一律整文件拒绝（加载侧独立校验，不信任文件内容）。
7. **目标措辞**：改为"退出前保存、后续启动恢复"；serde 派生明确标注**最小适配**（需改源码），与"直接复用（不动源码）"区分。
8. 附注：用户消息附带的上传稿 `sandbox:/workspace/scratch/08b7c74411e5/upload/007-session-persistence(1).md` 在文件系统中不存在（`/workspace/scratch/` 目录缺失）；其描述状态（"补齐复用规则、关键修改未落实"）与仓库规格修订前状态一致，故按消息内契约清单直接修订仓库规格，无信息缺口。
