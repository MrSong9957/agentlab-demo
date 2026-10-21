# 012 工具详情展示——实现记录（2026-10-15）

规格：`docs/specs/012-tui-tool-details.md`（含用户 8 条需求基线）。分支 `feat/008-codex-tui-port`，未提交。

## 1. 根因与复用入口

根因（调查轮结论，实施轮证实）：数据面断链——turn 只回推单行 `[工具] name（id8）`（原 turn.rs:227-231），`/resume` 投影丢弃 arguments（`tui_host.rs` 旧 :424 `command: String::new()`）且 patch 无 diff（`chat_driver.rs` 旧 :751 空串）；vendored 富 cell（ExecCell/PatchHistoryCell/disclosure）全部在编译面内但从未被喂数据。另修两处旧投影缺陷：exec 成功判定 `status=="success"` 恒 false、patch changed_files 按字符串提取对象数组恒空。

复用入口（零 vendored 文件改动）：`exec_cell::new_active_exec_command`/`complete_call`、`history_cell::{new_patch_event, new_patch_apply_failure, PlainHistoryCell}`、`diff_model::FileChange`＋`diff_render`（diffy）、`transcript_view` disclosure/detailed 呈现、`thread_items_to_transcript_cells`（文本运行）。

## 2. 必要适配（全部落点）

- **prompt.rs**：`ToolDisplay::{Exec{duration_ms}, Patch{changes}}`＋`PatchChange{path, kind}` 展示类型；`ToolResult` 增可选 `display`（serde 缺省兼容；`skip_serializing_if`——模型面 content 与请求组装零改动）。
- **apply_patch.rs**：`apply_patch_with_display`——Add=新全文、Delete=旧全文（先读后删；有界，超限/非 UTF-8→None 明示降级）、Update=`diffy::create_patch(真实前后缓冲)`；失败返回已生效前缀；模型面 `apply_patch` 结果逐字节不变（单测钉住）。
- **turn.rs**：`TurnNotify::{Info, ToolBegin, ToolEnd}`（End 携 arguments/result/display/cancelled）；`tool_activity_line` 保持行式输出逐字节；`SKIPPED_BY_CANCEL_MESSAGE` 导出常量；执行计时 Instant。
- **tui_host.rs**：notify→DriverEvent 映射（exec 参数不可解析/未知工具回退活动行）；`tool_end_item`（实时与 /resume 共用；exec 取消=status JSON、patch 取消=事件旗标或常量精确匹配）；`project_history_items` id→(name,arguments) 配对。
- **chat_driver.rs**（本地新写模块）：`DriverEvent::{ToolBegin,ToolEnd}`；`tool_cells` 记账（End 按下标替换，1→N splice 修正后续下标，Final/Failed 清空；不入 stream_indices——需求⑧）；`project_history`/`project_tool_item`（实时终态与恢复同一路径，需求⑤）；取消＝独立"已取消" cell（需求④）；缺 cmd/缺 diff 明确降级（需求⑥）；键路由：F4（focus_activity）＋Ctrl+T（open/close_transcript 同视图 set_presentation 切换）＋`has_active_interaction` 门＋视图放弃键落回 composer（母本 Ok(false) 语义）＋裸 exit 判定随迁至 composer 分支（聚焦态 Enter 不退出，S30b 钉住）。
- **main.rs**：行式 notify 映射，输出逐字节不变（tool_loop 14/14 既有断言不改动通过）。
- **Cargo.toml**：app crate 增 `diffy 0.4`（与 tchat-tui 同锁版本，无新版本引入）。

## 3. 测试（新增 24 项）

> 2026-10-15 审计修订：原稿分项「tui_host 2＋session 1＋chat_driver 6」与总数差不闭合
> （合计 26≠24）。按基线快照（`/tmp/008-a6-v4` 包，2688 时点）与两次运行日志逐二进制
> 核对（`/tmp/followup-final3.log` 2688 vs `/tmp/full-run.log` 2712）修正：terminal-chat
> lib 93→100（apply_patch 4＋turn 1＋tui_host 2）、tchat-tui lib 2111→2116（chat_driver
> **5**）、A7 8→11（3）、e2e 37→46（9）、session 集成 32→32（**0**——见 §7 死测试事故）。
> 合计 4+1+2+5+3+9＝24 ✓。A7（security_untrusted_text）是 tchat-tui 集成二进制、e2e 是
> terminal-chat 集成二进制，各计一次，与单元测试无重复统计。无删除、无替换。

- 单元：apply_patch 差异捕获 4（含模型契约不变）；turn 事件配对 1；tui_host 转换/配对 2（含两旧缺陷回归）；chat_driver 投影 5（取消 exec/取消 patch/空 cmd 保留输出/成功失败/降级＋state_known 标注）。
- e2e S27–S34＋S30b（46/46 全套）：命令/状态（S27、S16 增 Running、S14 升级）、增删改 diff＋计数锚点（S28）、取消（S29、N2b2 增跳过补丁断言）、折叠/展开/detailed/草稿（S30）、聚焦路由契约（S30b）、恢复不重放（S31）、旧会话降级（S32）、注入（S33）、多 cell 替换（S34）。
- A7 扩展：命令/diff/取消 cell 注入 3 单测（security_untrusted_text 11/11）。
- 变异 RED×3（均已回退复绿）：取消文案改 Declined→单测红；Update diff 置空→捕获测试红；F4 断路→S30 红。
- comprehensive-testing 审计补强：S30b（聚焦 Enter≠裸 exit＋字符落回草稿——发现并避开 paste-burst 时序坑）、S34（1→N splice 下标修正）。

**TDD 证据分类（2026-10-15 审计）**：上述新增测试未保存实施前的事前 RED 运行留档
（部分用例先于实现编写并预期失败，但无日志文件可引用）——登记为流程偏差，不补造。
可引用证据为事后变异 RED×3 与终验全绿；012 轮引用的 S26「修复前 RED」属 A3 轮资产。

## 4. 独立双轴审查与整改

Standards：无硬违规；3 处小型清理已做（arg_cmd 提取、回退分支合并、去多余 clone）；`popup_active` 参数名与 `path_label` 单行包装经评估保留（登记）。
Spec：2 处实现与规格抵触已修（跳过补丁失败化→"已取消"；空 cmd 丢输出→保留状态/输出）；state_known 呈现、计数锚点、Running 断言补齐；§5 已回填。
审查另确认 Delete 非 UTF-8 处理与坑 7 原措辞（lossy）不符——按实现定稿改为"明确降级 None"（比替换符内容更诚实），坑 7 措辞已同步。

## 5. 验证

- `cargo check --workspace` 通过。
- `cargo test --workspace --no-fail-fast`＝**2712 通过 / 0 失败 / 3 忽略**（基线 2688＋24）。
- 已知抖动登记：`session_perf::perf_tiers_baseline_driven`（/sessions 50ms 预算）全量并行下偶发超限（两次 63ms；隔离复跑×2 与终跑全量均通过）——本轮零改动路径，既有负载敏感项。
- 未覆盖（如实）：真实 Windows 终端矩阵（A6 暂停不变）；多显示器/极端尺寸下的 detailed 呈现未逐项截图比对（S30 断言文本级）。

## 6. 遗留与后续

- 010/011 未实施（010 的 Ctrl+C 规则待用户确认后另行任务）。
- 运行中补丁的"正在应用补丁…"标记为瞬态（无屏幕级断言；补丁应用通常亚秒级）。
- exec 输出无流式（本地 exec 同步整段返回——运行态无输出、终态一次性给全量，规格范围外声明）。

## 7. 2026-10-15 报告口径审计（后续轮）

**主数字复核无误**：终验日志 `/tmp/full-run.log` 逐行重算＝2712 通过/0 失败/3 忽略
（39 目标、2715 个测试函数＝2712＋3 忽略）；基线 `/tmp/followup-final3.log`＝2688/0/3
（2691 函数）；Δ＝24 与分项修正后闭合（§3）。口径统一为「通过/失败/忽略」三元组。

**审计发现并处置的真实缺陷（两项）**：

1. **session 往返测试从未运行（嵌套事故）**：`tool_result_display_roundtrip_and_
   legacy_default_none` 被嵌进上一个测试函数体内（缺右花括号），`#[test]` 从未生效——
   文件 33 个 `#[test]`、二进制只编译 32 个；原稿「session 1」计入了这条死测试。
   激活后暴露三处夹具错误（逐一修复）：孤立 tool_result 会被 007 配对修复跳过（须配
   tool_calls 批次）、缺 user 开篇记录触发 §6-F 头部丢弃规则、旧格式夹具同样两处。
   修复后按真实装载契约通过（历史长度 3、repairs=0；旧格式→display None）。
   **影响评估**：012 的 display 序列化往返在原终验中无执行证据（A6 声明当时不成立）；
   生产代码本身正确（激活即绿），属测试完整性缺陷而非行为缺陷。
2. **既有测试竞态（fork fd 继承）**：`new_auto_writer_lock_excludes_second_writer`
   全二进制并行下约 75% 失败（`new_auto × half_write 探针`配对 10/15 复现）。根因：
   `run_probe` 的 `Command::spawn()` fork 瞬时复制全进程 fd 表（含其他测试持有的会话
   flock），O_CLOEXEC 到 exec 才关闭——fork→exec 窗口内 `drop(a)` 无法释放锁，C 的
   `try_flock` 误判 Busy；panic 时子进程已 exec 完故现场「无锁无 fd」。tmpfs 上同样
   复现（排除 overlayfs 因素）；单线程/隔离运行不现。012 终验全绿与 2688/2678 基线
   均为该竞态下的幸运采样。**修复（测试设施）**：`PROBE_FORK_GUARD` 互斥闸——
   `run_probe` fork→子进程退出全程持闸，`new_auto_writer` 测试体持闸；修复后配对
   0/20、全二进制 0/15。**结论收敛（已验证范围）**：仅证实「测试间互扰」——已复现
   并修复；生产侧对等问题**未证实发生**（无直接证据；CLI 不并发断言锁转移、窗口为
   fork+exec 固有语义、CLOEXEC 已设仅为机理上不支持的分析，非生产无对等的证明），
   本轮不扩展生产修复范围。`flock_conflict`（自带探针）与 `same_name_poacher`
   （线程非进程）未现失败，未加闸——登记残余风险。


## 8. 实测反馈诊断（2026-10-16 诊断轮，无生产改动）

用户实测（真实会话）：①「你能做什么？」触发 `pwd && ls -la`；②演示两工具＋清理临时文件；③清理命令附带 `git status --short` 输出大量既有修改；④前两项有 Added/Ran 标题、清理项呈「裸命令＋大段输出」且随后**单独**出现「✓ • 1.03s」；⑤长列表末尾疑似中途截断无提示。

### 8.1 复现与定位（隔离夹具：3 调用批次＋150 行确定性输出，未触碰真实仓库）

- **默认（紧凑）形态无缺陷**：三 cell 均为 `• Ran '<cmd>'`＋尾部 3 行预览＋`+ 149 lines (^t to expand)`——标题/预览/截断提示齐全（收敛目标已达标）。
- **用户所见形态与第三 cell 的单 cell 展开态逐元素一致**（形态来源判定，**不据此断定用户当时主动展开**）：点击披露行（010 鼠标接线后可点）或 F4+Enter 展开即得同形 vendored `detailed_hyperlink_lines`（母本固有形态）：`$ <cmd>`（无 Ran 标题）＋全量输出＋**尾部独立 `✓ • 时长` 行**。端到端复现与用户截图逐元素一致。
- **「疑似截断」**＝视口末尾而非数据截断：展开形全量渲染无截断；紧凑形截断有 `+ N lines` 标记；滚动条沟位＋「↓ Back to bottom · esc」在位——**可发现性弱**（用户未识别为延续信号）。
- **实时/恢复一致** ✓（同一 `project_tool_item` 投影，S31）；无 Info/富 cell/正文重复 ✓（复现中每元素单次）；无 println/stderr 绕过统一渲染 ✓。
- **次要发现（展示）**：展开形命令显示为 `$ 'echo … && …'`（整条单引号包裹）——根因：003 契约 `cmd` 为单字符串，投影以单元素 argv 存储，`escape_command` 对含空格单元素 shlex 加引号；母本以 `["bash","-lc",script]` argv 存储经 `extract_shell_command` 剥壳无引号（`exec_command.rs:12-16`）。

### 8.2 最小修复方案（候选，待批准）

1. **展开态头部保留语义**（可选 UX）：展开形在 `$ cmd` 上方保留一行 `• Ran/Failed ＋时长`（复用 compact 头构造），尾部状态行维持母本不变——长输出下状态不再「凭空单现」。改动点：`exec_cell/transcript.rs::detailed_hyperlink_lines`（vendored——需登记偏离）。（**实施轮演化注（2026-10-16）**：用户实施指令明确「命令、状态和时长不因拼接重复展示」→ 尾部行不再保留（K-3：移除而非并存）；且头部收敛为**并入 `$ cmd` 首行**（K-1：与紧凑头同款单行构造＋复制文本 source 前缀记账），见 §9。）
2. **argv 形态对齐**（次要）：投影侧按母本 `["bash","-lc",<cmd>]` 存储（或单元素整串不加引号），消除 `$ '…'` 包裹。改动点：`chat_driver.rs::project_tool_item`＋ToolBegin 运行态（本地模块，零 vendored）。
3. **「继续内容」可发现性**：不在本轮扩范围——滚动条已提供位置信号；如需更强提示另行立项。
4. 不针对 `git status` 硬编码删行/丢弃结果（目标红线遵守）。

**验收场景**：①默认批量 3 调用→三 cell 均紧凑＋标记（既有 S28/S34 回归）；②点击披露行→展开形首行含状态语义（如实施 1）、`$` 行无整条包裹引号（如实施 2）、尾部 `✓ • 时长` 仍在；③150 行输出滚动可见全量（无数据丢失断言：恢复会话后同投影）；④副作用：Ctrl+T 全局 detailed 不回退。

### 8.3 模型行为 vs 工具契约（附件其他发现，不借 UI 修复顺带改）

- **能力问答触发 `pwd && ls -la`**：模型行为，与所载 codex agent 提示一致（`assets/codex/prompt.md:125`「coding agent…using the tools available」＋探索文化）；提示未约束能力问答免探测。是否收紧属提示政策决策——依据：能力答复通常无需仓库状态，但探测无害且有时相关；**不改**，留待用户裁决。
- **「文件修改只用 apply_patch」自述 vs `exec` rm 清理**：契约原文 scope 为「**编辑**文件用 apply_patch」（prompt.md:132），临时文件删除经 shell 属命令操作语境（:143「making folders, deleting folders」同段）——**无规则不一致**；系模型自述过宽。可在提示中补一句澄清（「删除/移动非编辑类文件操作可用 shell」），属可选提示修订，未实施。
- **大段能力介绍/主动建议**＝模型正文正常投影（无重复投影证据）。
## 9. 展开态修复轮（2026-10-16，规格 §6 方案 ①② 实施）

### 9.1 事前 RED（旧实现）

- 单测 `exec_command_displays_verbatim_without_shell_wrapping`（chat_driver）：四类命令（空格/嵌套引号/中文/运算符）断言 `Ran <cmd>` 逐字可见——旧实现 `Ran 'echo t012-ok'` 整条包裹，**RED**（「命令原文紧随标题逐字可见」失败）。
- 单测 `detailed_form_leads_with_status_header_without_trailing_result`（exec_cell）：断言展开态含 Ran 标题＋时长且无尾部 `✓`——旧实现首行即 `$ <cmd>` 无头、尾部独立状态行在，**RED**（「头部状态标题」失败）。
- 命令：`cargo test -p tchat-tui --lib exec_command_displays_verbatim`／`detailed_form_leads`（实现前运行各 1 FAILED）。

### 9.2 实现（最小差异）

- **argv 对齐（方案②，本地模块零 vendored）**：`chat_driver.rs` 新增 `shell_argv(cmd)`＝`["/bin/sh","-c",cmd]`——**已核对 003 真实执行入口**（`exec.rs:31/188`：`Command::new("/bin/sh").arg("-c").arg(cmd)`，cmd 为单参数），与真实 argv 一致才走母本剥壳（`extract_bash_command` 认 `-c`＋Sh 型）；不伪造 bash、不拆词、不增删引号；ToolBegin（运行态）与 `project_tool_item`（实时终态＝/resume 恢复同一路径）两处构造点替换。exec.rs 零改动；`HostHistoryItem`/`ToolDisplay` 序列化未动（无新增持久化字段）。
- **展开态头部（方案①，vendored 两处登记）**：`compact.rs` 提取 `status_marker_span`/`status_title` 共享构造（紧凑形态字节不变——快照佐证）；`transcript.rs::detailed_hyperlink_lines` 状态头（marker＋标题＋dim `• 时长`，时长仅 `duration.is_some()` 时附加）**并入 `$ cmd` 首行**（spans.splice＋`source.prefix_bytes` 记账——display 路径 render.rs 同法，**复制文本仍＝命令＋输出**，vendored 复制契约测试原样通过）；尾部独立 `✓/✗ • 时长` 行移除（K-3：不因拼接重复展示）。
- 净化边界：头部仅受控文案；命令仍经 `highlight_bash_to_lines`/`ansi_escape_line`/Paragraph 管线（A7＋S33 经新路径复验）。

### 9.3 快照变更（5 份，逐项）

见规格 §6.6（exec_cell×2、detailed_command_copy、tool_output、find_selective_expansion；紧凑段均不变，detailed_command_copy 的复制文本相等断言原样通过）。

### 9.4 验证

- GREEN：两单测＋新 e2e S56（批次 3 调用归属/展开收起/resize/草稿/滚动条/选区）＋C4 补强单测（失败 `Failed (exit 9) • 7ms`/运行 `Running` 具名断言）；e2e **70/70**；A7 **11/11**。
- 变异 RED（已还原）：`shell_argv` 临时回退单元素 → S56（「紧凑头命令原文」）与单测 A 双红——引号回归可被发现；Standards 审查者独立变异（注释 prefix_bytes 记账）→ 复制契约测试红——记账可被发现。
- 独立双轴审查处置：Spec 轴——§8.1 措辞软化（不反推用户操作）、规格措辞同步（并入首行）、快照 5 份补全说明、C4 补单测；Standards 轴——记录 §8.2 方案演化注（上）、hyperlink 平移隐患注释（当前无链接注记，非活 bug）、二次重排依赖注释。
- 终验：`cargo check --workspace` 0 错误；`cargo test --workspace --no-fail-fast`＝**2751 通过 / 0 失败 / 3 忽略**（基线 2747＋本轮 4）。
- 提醒（不处理，与任务无关的既有内容）：`src/tui/src/thread_transcript/snapshots/…completed_tool_presentations.snap` 为孤儿快照（对应测试 008-TRIM 轮已删，内容仍为旧形 `$ cmd`＋尾部 `✓`），易误导后人，留待 008 后续清理决策。

## 10. 展开后 Enter 无法提交修复轮（2026-10-16 第二轮，规格 §7）

### 10.1 根因（经验定论，未预设输入方式）

- **复现④机制**：`toggle_activity`（vendored disclosure.rs:361）在每次点击展开/收起后 `disclosure.focused = Some(ids)` → `has_active_interaction()` 吞下后续全部按键；此时 Enter＝`ListAction::Accept`＝再切换该工具，请求不提交。
- **两路径分野（s57/s58 双跑实证）**：逐字键入本就交还（vendored `handle_disclosure_key` 对普通文本清焦返回 None——"Ordinary text resumes composing"，修复前 s58 即绿）；**粘贴（bracketed paste）绕过键路径**，本地驱动 `CrosstermEvent::Paste` 只 `pane.handle_paste`，聚焦保留 → Enter 被吞（s57 事前 RED：mock 计数停 2＋工具被收起）。**用户输入方式未明——两路径分别验证后定论，非先设粘贴为根因**。独立审查者以移除粘贴清焦变异复证（s57 红/s58 绿）。
- **次要缺口**：composer 处理鼠标（非滚轮）后母本清三态（end_selection/cancel_search/clear_activity_focus，owned_transcript.rs:475-483），本地只清选区——点击输入区不交还聚焦。

### 10.2 修复（最小差异，零 vendored 改动）

`chat_driver.rs` 两处母本同法接线：①Paste 分支＝`end_selection`＋非搜索态 `clear_activity_focus`（母本 :640-649 逐字对齐，含 `paste_search` 调用——本地搜索不可达恒 false，注释登记）＋`handle_paste`；②composer 鼠标分支（非滚轮）＝既有 end_selection 后加 `clear_activity_focus`（cancel_search 不可达不调，登记）。不新建焦点状态机；非「草稿非空就提交」修补（S60 钉死：预存含 exit 字样草稿＋F4＋Enter 仍操作工具、零误发、无裸退出）。

### 10.3 验证

- 事前 RED：s57（粘贴路径）——真红（计数 2、工具被收起）；s58 首跑假红（视口保持阅读态使回复在视口外——**测试设计错误更正**：mock 计数先导＋双 Esc 回底后断言，提取 `wait_submitted_then_show_reply` 助手）。
- 用例：s57/s58（点击展开→粘贴/逐字→Enter）、s59（F4→Enter 展开证明→点输入区→同一键 Enter 提交，变异验证清焦可发现）、s60（D4 契约）、s61（F4 后键入/粘贴两会话）、s62（用户 03：exit 4 不重试＋失败状态＋03 阻断步骤解除）。
- 终验：`cargo check --workspace` 0 错误；`cargo test --workspace --no-fail-fast`＝**2757/0/3**（基线 2751＋6；连续三次全量复跑均 2757/0）＋e2e **76/76**；A7 **11/11**。探针/变异零残留。
- 抖动如实登记：终验首轮全量曾现 1 例失败（2756/1），当时管道未捕获用例名；随后三次连续全量均 2757/0＋`session_perf` 隔离复跑通过——模式与 §5 已登记的 `perf_tiers_baseline_driven` 负载敏感抖动一致但**未能证实**（无失败名），不据此断言原因。
- 独立双轴审查：Standards 通过（2 判断性建议：重复尾部已提取助手；s59 点击坐标硬编码＝响亮失败可接受）；Spec 全项可证（审查者独立变异复证根因分野）。观察项登记：横向滚轮（ScrollLeft/Right）在 composer 区会清焦（母本本地不可核对、终端罕见发射）。

### 10.4 用户复验指引

「展开旧工具后粘贴新请求并直接 Enter」：`cargo run` → 触发长输出工具 → 点击 `+ N lines (… to expand)` → 粘贴下一句 → Enter——预期：请求发出（Working 指示/回显出现；视口保持阅读态属 S48 语义，Esc 回底见回复）、旧工具保持展开；逐字键入路径同效。03（exit 4 场景）解除阻断。

## 11. 独立详情页轮（2026-10-16 第三轮，规格 §8——用户已选方案 B）

### 11.1 复用入口与实现

- **复用**：`pager_overlay::Overlay::Transcript`（TranscriptOverlay——事件/剪贴板 poll/notice/draw 骨架）＋内嵌 `TranscriptView`（滚动/选区/右键复制/折行＝与主对话同一视图，非第二套滚动模型）＋`TranscriptBookmark`（进入前快照/关闭按内容锚点恢复）＋既有富 cell §6 展开态渲染即页内容。不新建框架/并行事件循环/快捷键/配置。
- **本地（chat_driver.rs）**：`ToolDetail{overlay,bookmark,activity}` 槽位（母本 app.overlay 同构）；页开期间终端事件全路由 overlay（Ctrl+C/Ctrl+D J10 先行且消费——不继承 pager close 含 ctrl+c 的关页默认）；三入口（披露行点击＝`disclosure_hit` 纯命中、F4 聚焦→Enter、Ctrl+T＝聚焦/最近工具）；开页即清主视图活动聚焦（K-D——焦点残留根因结构性消除）；Tick 继续入账但剪贴板 poll 让位页内；ToolEnd 同 id `replace_cells` 刷新页 cell；尾绘制页开时路由 Draw 给页。
- **vendored（012-ADAPT 登记）**：transcript.rs 页模式（标题/Esc 一次关闭/沟位滚动条渲染＋按下归属拖动/固定提示/禁入搜索/开页置顶/绕过 footer 保提示区）；pager_overlay.rs 透传；disclosure.rs 命中提取＋焦点索引；layout.rs＋vendored 测试字面量（to expand→for details）。

### 11.2 TDD 与验证

- 事前 RED：s63–s66 对旧实现全红（无页标记/新文案 needle 不存在）；GREEN 后按新契约改写 s56–s62（原地展开断言→页开/Esc，逐项登记；S30/S30b 语义兼容自然通过）。
- 快照 3 份文案变更逐项核对接受（transcript_view×2 字面量＋e2e needle）。
- 变异 RED×2（页内 J10 先行→s67 红；Esc 直关→s63 红）＋双轴审查整改 6 项：沟位拖动归属（拖出沟位列不再漏成选区/滞留）、Ctrl+C 消费（取消态不被 pager close 关页踢出）、patch 页 id 修正（`patch:{id}`）、页禁入搜索（避免搜索编辑态 Esc 冲突）、页模式不走 keymap.close（q）、`page_drag` if-let＋回退 cell 移除。
- 终验：**2762/0/3**（基线 2757＋5）、e2e **81/81**、A7 **11/11**、check 0 错误；探针/变异零残留。
- 残余（如实）：保持阅读态＋resize 组合恢复的 e2e 未建（内容长度边界不稳定）——bookmark 单测＋s63 跟随态恢复承担机制证明；patch 运行 cell（PlainHistoryCell 无 activity）按构造不可开页——浏览中终态刷新对 patch 不可达（exec 路径在位）；页标题 "tool details" 为登记的适配文案。

### 11.3 用户复验指引

`cargo run`→长输出工具→点击 `+ N lines (⌃t for details)` 或 F4＋Enter 或 ⌃T→独立详情页（顶部命令/状态/时长＋全文＋右列滚动条＋底部 `esc back to conversation`）→滚轮/翻页/拖滑块→一次 Esc 返回原位（草稿/光标/输入归属不变）→粘贴或键入下一句＋Enter 正常提交。03（exit 4）场景同法可用。
