# 008 快照副产物核验、清理立项与实施（2026-10-18）

范围契约见 `docs/specs/008-tui-rendering-port.md` §5 维护验收（根因＝§4 坑 19；本记录只存证据，不重复验收清单）。基线：main @ `59c7e18`（父 `b0e8d48`），`feat/008-codex-tui-port` 已合并删除；实施分支 `fix/008-snapshot-hygiene`（自 59c7e18），截至本轮**未提交**，未改生产代码。

## 1. 结论

4 个 `.pending-snap`＋1 个 `.snap.new` 是 **2026-10-14（008 移植日）开发期中间态遗留**，经 `59c7e18` 的 `git add -A` 批量暂存误入库。当前代码**没有**被放行的快照失败——严格模式下受影响测试全绿。`.pending-snap` 为 insta 只写记录文件、不参与比对，入库无测试语义影响；实际危害＝每次测试运行追加 PASS 标记弄脏工作区（曾阻塞一次分支切换）＋ `cargo insta review` 可按陈旧行号误改写。

## 2. 证据

### 2.1 入库与陈旧性判定

- 5 路径经 `git ls-files --error-unmatch` 逐一确认被跟踪；`git log --all --oneline -- '*.pending-snap'` 唯一命中 `59c7e18`。
- pending 记录 JSON 首字段 `run_id` 前段为 Unix 时间戳：全部差异记录落在 2026-10-14 13:12–17:39 UTC（PASS 标记延续至 10-17 v14 轮，`1792208006`）。
- 当前测试二进制 `target/debug/deps/tchat_tui-7d5d3302a0686e59`（10-17 构建，10-18 全量运行即此件）`strings` 检索 `navigate_copy_resize_and_exit` 零命中——被裁测试不可能写出入库记录。
- insta 1.49.0 源码（`~/.cargo/registry/src/…/insta-1.49.0/src/runtime.rs:699-738` `finalize`）：inline 不匹配在 `update_result != InPlace && !force_pass()` 时 panic；inline 的 InPlace 被强制转 NewFile（runtime.rs:581-587）。仓库无放行开关：`env | grep -i insta` 为空、无 `.cargo/config.toml`、无 `insta.yaml`/`.config/insta.yaml`、`src/` 全树无 `set_var`/`with_settings`/`force_pass`。`cargo test` 退出码 0 因此只能是真实全绿。
- `.pending-snap`（源文件旁点文件）为 inline 断言专属写入路径（runtime.rs:402-411）；PASS 标记追加逻辑＝`cleanup_passing`（runtime.rs:511-517，仅当文件已存在）。

### 2.2 差异明细（4 组，全部开发期中间态或已裁剪，非本次回归）

| 测试（snapshot_name） | 位置（当时行号） | 期望（当时金快照） | 实际（当时渲染） | 判定 |
|---|---|---|---|---|
| `file_links_preserve_foreign_windows_paths_and_anchors_snapshot` | markdown_render_tests.rs:1115（测试现存 :1101） | `report (Quarterly Report.xlsx:4:2)` | `report (file:///C:/repo/Quarterly%20Report.xlsx#L4C2)` | 中间实现输出原始 URL；当天收敛为美化路径后持续通过（后续 118 条 PASS 标记） |
| `file_url_citation_preserves_literal_query_delimiters_snapshot` | file_citations_tests.rs:45（现存） | `report?final.xlsx` | `file:///tmp/report%3Ffinal.xlsx` | 同上（582 条 PASS 标记） |
| `followup_labels_render_like_ordinary_markdown_snapshot` | followups_tests.rs:37（现存） | `• Add revenue charts`（无前导空格） | ` • Add …`（前导空格变体） | 17:39 缩进中间态；当前金快照与实现一致 |
| `navigate_copy_resize_and_exit` | copy_mode_tests.rs:79（**测试已裁**） | plan cell `-` 缩进头 | `• Latest plan`（AgentMarkdownCell 圆点头） | 按 J8 不重接受快照，整测裁剪（源码 :27-30 008-TRIM 注释），覆盖由其余 17 测承接 |

第 5 个文件 `.snap.new`（`chat_composer__sparkle__…a_fresh_astra…`）属主测试同为 008-TRIM 裁剪对象（sparkle_tests.rs:330，Command 弹窗裁剪）；其与已接受 `.snap` 的 diff（`/status`、`/statusline` 两行差＋行号偏移）为裁剪前旧版渲染。该 `.snap` 与 `.snap.new` 均成孤儿，按立项决定保留 `.snap`、仅登记。

### 2.3 严格校验（清理前基线，已执行）

```
INSTA_UPDATE=no INSTA_FORCE_PASS=0 INSTA_OUTPUT=diff \
  cargo test -p tchat-tui --lib -- \
  markdown_render transcript_view::copy_mode a_fresh_astra_response_waits
→ 233 passed; 0 failed（第三过滤词属主测试已裁、命中 0，命令按执行原样记录）
```

### 2.4 副作用观察

严格运行后：4 个 pending 文件合计新增 9 行、全部为 `new:null/old:null` PASS 标记，零新增差异；`.snap.new` 无变化（属主测试不存在，无清理触发）。diff 已存证后 `git restore` 还原，工作区仅剩 `db/`。

### 2.5 原始证据文件（/tmp 非持久，要点已收录上文；如需全文可按下述命令再生）

- 备份 5 文件原样拷贝：`/tmp/pending-evidence-59c7e18/src/…`（或 `git show HEAD:<路径>` 取提交版本）。
- 差异全文：`mismatch-details.txt`；严格运行副作用：`strict-run-side-effects.diff`；再生命令：`git show 'HEAD:src/tui/src/transcript_view/.copy_mode_tests.rs.pending-snap'` 等逐文件。

## 3. 实施轮（2026-10-18）：变更与验证

**变更**（分支 `fix/008-snapshot-hygiene` @ `59c7e18`）：`git rm` 精确路径×5（staged D×5，未用 `git add -A`）；`.gitignore` 追加 `*.pending-snap`、`*.snap.new`（全文＝`/target/`＋两规则）；规格更新（§4 坑 19＋§5 维护验收回填＋头部/修订记录）＋本记录。

**验证**（全部通过，命令原样）：

1. 跟踪移除：5 个目标路径逐一 `git ls-files --error-unmatch` 均不再跟踪；`git ls-files | grep -E '\.(pending-snap|snap\.new)$'` 仅剩 §5 登记的 3 个残留（本轮范围外）。
2. 忽略规则：`git check-ignore --no-index` 对 5 个副产物路径全命中；抽验 6 个正常 `.snap`（随机 5＋孤儿 `…a_fresh_astra….snap`）均不命中。
3. `.snap` 一致性：基线 `git ls-tree -r 59c7e18 --name-only | grep -c '\.snap$'`＝724；当前 `git ls-files | grep -c '\.snap$'`＝724；`git diff 59c7e18 --stat -- '*.snap'` 为空；工作区 `.snap` 零修改。
4. 严格测试（`INSTA_UPDATE=no INSTA_FORCE_PASS=0 INSTA_OUTPUT=diff`）×2：
   - 3 个现存差异测试精确过滤 → **3 passed / 0 failed**，逐条 `test markdown_render::markdown_render_tests::file_links_preserve_foreign_windows_paths_and_anchors_snapshot ... ok`、`test markdown_render::inline_directives::tests::file_url_citation_preserves_literal_query_delimiters_snapshot ... ok`、`test markdown_render::inline_directives::followups_tests::followup_labels_render_like_ordinary_markdown_snapshot ... ok`；已裁剪 `navigate_copy_resize_and_exit`、`a_fresh_astra_response_waits…` 过滤零命中，**不计为通过项**。
   - 受影响子集（`markdown_render`＋`transcript_view::copy_mode`）→ **233 passed / 0 failed**，与清理前基线（§2.3）一致。
5. `cargo check` → `Finished dev profile … in 33.24s`。
6. 两次测试运行后文件系统扫描：`find /workspace/src/tui /workspace/src/app -name '*.pending-snap' -o -name '*.snap.new'` → 仅 §5 登记的 3 个残留，**零新增**（不以 git status 代替——新忽略规则会隐藏此类文件）。
7. `git diff --check`＋`git diff --cached --check` → 通过；差异仅授权范围（5 删除＋`.gitignore`＋规格/记录）。

**门禁适用性裁决**：tdd **不适用**（无产品行为修改，删除对象非测试期望，不新增单测）；comprehensive-testing＝上述实测项；Security Testing Gate **跳过**——仅删非测试文件＋忽略规则＋文档，无新增安全敏感面（该裁决**不补齐** 010 剪贴板安全证据缺口）；verify:full＝L3 合理范围（`cargo check`＋受影响测试＋一致性检查），**未重跑整个 workspace**（无代码路径变化，2026-10-18 合并门禁全量退出码 0 仍为有效全量基线）；Windows A6 暂缓不变。

## 4. code-review（Skill 双轴）处置

审查范围＝`git diff 59c7e18`（暂存＋未暂存）＋未跟踪的本记录；固定点解析通过、diff 非空（+20/−1331）。`docs/agents/issue-tracker.md` 不存在（如实说明：规格源为任务指定路径，非 issue 引用，不影响流程）。Standards/Spec 双轴并行子代理独立执行，结论如下：

- **Spec 轴发现 1 项（实质，已处置）**：审查时规格 §5 已勾「[x] code-review 完成」而本节尚为占位＝提前填通过。处置＝本节回填（审查即本节内容），回填后该项成立；其余 8 项 [x] 经子代理独立复核与仓库实际一致（删除清单与授权 5 路径逐一比对无多删、无 scope creep、严格测试 3/0 与 233/0 复现、check-ignore 正反例复现、`git diff --check` 两段 exit 0、残留 3 个清单一致、TRIM 行号引用属实）。
- **Standards 轴：硬违规 0；判断性意见 2 项（均已处置）**：
  1. 证据双写——规格 §5 维护验收与记录 §3 重复抄写逐测试名/计数，违反工作区 AGENTS.md §2「最终报告引用同一份证据，不重复抄写」。处置：规格 §5 严格模式项压缩为判定＋指针，明细以记录 §3 为准（已改）。
  2. `db/wiki.db` 未跟踪且不被本轮两条忽略规则覆盖＝坑 19 根因（`git add -A` 扫入滞留文件）的同类活体。处置：本轮授权仅两条规则，不扩权加规则；登记于 §5，留后续决定。
- 两轴均未发现 scope creep；基线坏味道其余各项对纯删除＋文档 diff 不适用。

**补跑受影响检查**（上述文档修复后）：`git diff --check`＋`git diff --cached --check` 通过；`git status --short --untracked-files=all` 仅授权变更。文档修复不触测试对象，按 AGENTS.md §3 不重复运行已通过的测试。

## 5. 残留登记（本轮范围外，如实登记不扩权）

3 个同类 `.snap.new` 仍被跟踪（均在 `59c7e18` 基线内；属主测试均为 008-TRIM 裁剪对象——shortcut overlay 批次，裁剪注释 `sparkle_tests.rs:256/258`、`status_surface_tests.rs:250`）：

- `src/tui/src/bottom_pane/chat_composer/snapshots/tchat_tui__bottom_pane__chat_composer__sparkle__tests__shortcut_help_hides_visible_sparkles_without_restarting_their_deadline.snap.new`
- `src/tui/src/bottom_pane/chat_composer/snapshots/tchat_tui__bottom_pane__chat_composer__sparkle__tests__shortcut_help_preserves_an_unused_sparkle_but_literal_question_marks_do_not.snap.new`
- `src/tui/src/bottom_pane/chat_composer/snapshots/tchat_tui__bottom_pane__chat_composer__status_surface__tests__activity_shortcut_help_follows_runtime_binding.snap.new`

新忽略规则已覆盖其模式（`check-ignore --no-index` 实测命中）；是否删除属后续新决定（含对应孤儿 `.snap` 一并评估）。

**复发向量登记（code-review Standards 轴意见）**：`db/wiki.db`（wiki MCP 运行时索引，未跟踪）不被本轮两条忽略规则覆盖——坑 19 根因（批量暂存扫入滞留文件）的同类活体；是否补 `db/` 忽略规则属后续决定（本轮授权仅两条规则，不扩权）。

## 6. 承上轮缺口登记（本轮未消解，如实保留）

- 010 剪贴板（OSC 52 净化）专项安全覆盖**尚未证实**，不得写成已通过；`docs/security/reports/` 止于 2026-10-10（001-007 时代）。
- "Security Testing Gate"/"verify:full" 非已装 skill，008 记录已注明按语义手工执行，维持该认定。
- Windows A6 ConPTY 继续暂缓（008 §5-A6/D8），本轮未恢复。

## 7. 过程失误记录（根因链与教训）

- 合并轮（同日早些）曾把 4 个 pending 文件当作本次运行副产物 `rm` 后又因合并恢复提交版——当时误判「运行产生」，实际是「运行向开发期遗留文件追加标记」。该误判已由 §2.1 证据链纠正；教训：对工作区已存在的 insta 点文件，先比对 mtime/内容与运行日志再定性。
- 实施轮盘点补正：立项轮按 `git ls-files | grep -i pending` 盘点，漏掉 3 个名字不含 pending 的 `.snap.new`（§5）——初版「1 个 .snap.new」结论不完整；教训：同类盘点必须按扩展名模式（`grep -E '\.(pending-snap|snap\.new)$'`），已并入规格坑 19。
