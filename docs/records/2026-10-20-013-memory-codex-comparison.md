# 013 记忆系统 V1 与 Codex 开源实现对照报告

调查轮：2026-10-20。只调查与报告，未修改产品实现、未提交、未推送。

## 1. 对照版本、范围与证据口径

| 项 | 值 |
|---|---|
| 本地参照 | `main @ cab85bcf383498ae18b890b05f087fc2a8b558fe`（V1 最终收口提交，本轮起点与终点一致，无新提交） |
| 上游固定提交 | `openai/codex @ ce0323f2f2d3b25e82806cd555237da3db4e1897`（与 M1–M9 各轮固定上游相同；本轮重新下载的文件与 M9 准备轮缓存逐字节一致已核验） |
| 比较范围 | 本项目 V1（§18.1 七项）对上游 V1 路径（`memories/write`＋`memories/read`＋state 运行时＋core 注入/反馈接线）；上游 V2（`ext/memories` 专用工具、分层输入、dual write）与其他额外能力单列于 §4 |
| 上游快照 | `/tmp/013-cmp-up/tree/`（74 个文件按仓库原路径存放；本轮实际下载清单 `/tmp/013-cmp-up/filelist.txt`＋后续补取的接线/配置/git/sqlite/features 文件） |

证据等级标记：**[S]**＝源码确认（本轮实际读取两侧源码）；**[T]**＝既有测试/实测验证（引用已归档测试，未重跑）；**[R]**＝本轮实际运行（本轮命令，见 §5）；**[U]**＝尚待验证的推断。上游行号链接基底 `https://github.com/openai/codex/blob/ce0323f2f2d3b25e82806cd555237da3db4e1897/`。

差异类型标记：`一致`（行为相同）、`已批准差异`（G/D 系列既有裁决）、`实现替换`（等价目的不同手段）、`范围外`（V1 明确排除）、`未验证`（未证明等价）。上游触发/接线调用方的个别 core 侧行号未在本轮快照内定位到（快照是记忆相关子集），凡此情况注明「快照外」。

本轮核验方式：两侧各一次深度源码测绘（本地 19 个 memory 文件＋驱动侧；上游 74 文件），随后对全部承重结论做二次人工抽验——模板 5 份逐一 `diff`、SQL 三段逐字比对、驱动接线点/过滤层/隔离层/代数边界逐处 `sed` 复核（本报告引用的本地行号均经本轮直接读取确认）。AGENTS.md 既有并发修改原样保留，与本报告无关。

## 2. 功能对照表（15 项链路）

| # | 功能 | 上游 V1 行为 | 本地行为 | 差异类型 | 实际影响 | 源码依据（上／本） |
|---|---|---|---|---|---|---|
| 1 | 配置与开关 | feature flag `MemoryTool`（key "memories"，Stable，**默认关**，features/src/lib.rs:1291-1295 [S]）；开启后 `generate_memories`/`use_memories` 默认 true；`extract_model`/`consolidation_model` 可指定，缺省回退 provider 偏好（config/src/types.rs:315-375；write/phase1.rs:162-167、phase2.rs:344-350） | `MemoriesConfig` 七字段，`enabled` **默认 false**、`use_memories` 默认 true（memory/config.rs:38-78）；无专用记忆模型——用触发轮当前 `selected_model`（D6，runner.rs:159-166）；来源＝manual.json `memories` 段，无环境变量 | 一致（默认关）＋已批准差异（模型选择 D6、无独立生成开关） | 本地 `enabled` 同时门控生成＋登记＋反馈；上游生成/使用两开关独立。记忆用会话主模型，质量/成本与上游专用模型路径可能不同 | 上 features/src/lib.rs:1291 [S]；config/src/types.rs:358-375 [S]／本 memory/config.rs:38-94、runner.rs:147-166 [S] |
| 2 | 触发与后台 | **根会话启动时触发一次**（README.md:31-38；write/start.rs:24-59：ephemeral/非根 agent 跳过，每 version 一个 `tokio::spawn`，无取消句柄）；管线序 store→layout→seed→prune→配额→phase1→phase2（start.rs:60-92）；phase2 代理结束 10s 超时关闭（runtime.rs:446-460） | **每个成功启动的新用户轮触发一次**（D2，runner.rs:7-8；main.rs:340-356、tui_host.rs:165-180）；`std::thread::spawn`，进程退出不等待；管线序与上游同（runner.rs:228-318）＋阶段间代数复核 | 已批准差异（D2：轮级触发 vs 启动级）＋实现替换 | 本地更及时（闲置 6h 窗口更早满足）但每轮一次空跑开销；互斥均靠 DB 任务表。上游会在会话开始即提取上一次会话；本地在下一轮开始时 | 上 memories/README.md:31、write/src/start.rs:24-92 [S]／本 runner.rs:174-222、228-318；main.rs:340-356 [S] |
| 3 | 会话登记与候选 | 候选来自共享 state 库 `threads` 表（跨产品共享线程目录）；`push_thread_filters` 支持 `allowed_sources` 白名单与归档过滤（state/runtime/memories.rs:227-242，threads.rs 快照外）；窗口 `updated_at_ms ∈ [now−10d, now−6h]`，scan_limit 5000 | 候选来自**本地自身 sessions 目录枚举 ∩ 自建 threads 表**（G6 最小表：thread_id/memory_mode/updated_at_ms，store.rs:876-888、946-989；candidates.rs:74-124）；窗口同 10 天/6h/5000；排除当前会话；无来源白名单/归档概念 | 已批准差异（G6）＋一致（窗口常量） | 本地只管理自己的会话，无跨产品线程；`allowed_sources` 过滤在本地无对应物（本地无其他来源） | 上 state/src/runtime/memories.rs:153-299 [S]／本 memory/candidates.rs:74-124、store.rs:876-989 [S] |
| 4 | 输入装载与脱敏 | 尾部反向扫 JSONL，预算 8MB/单条 1MB（write/rollout_input.rs:28-29,35-87）；清洗剔除 AGENTS.md/`<skill>` 注入块（:327-359，**读取侧**）；V1 序列化后整体 `redact_secrets`（phase1.rs:420-423） | 同常量 8MiB/1MiB、尾部反向分块（rollout_input.rs:22-92）；AGENTS/skill 块在**序列化侧**剔除（:149-177，登记差异）；同样整体 `redact_secrets`，四条正则（secrets.rs:12-29） | 一致（预算/脱敏）＋实现替换（过滤位点） | 行为等价成立条件：被剔除块的字节同样不计入预算——本地在序列化后过滤，预算按已过滤文本计（与上游读取侧过滤后再计预算等价）[T]（memory_phase1/rollout 用例） | 上 write/src/rollout_input.rs:35-87,327-359 [S]／本 memory/rollout_input.rs:35-196、secrets.rs:12-29 [S] |
| 5 | 提炼请求与三态 | system 提示 `stage_one_system.md` 作 base_instructions；**`output_schema`＋strict 经 Responses API `text` 参数下发**（phase1.rs:311-312；client.rs:1055-1059,1091；schema 定义 phase1_output.rs:61-83）；流式聚合；严格 `deny_unknown_fields` 解析＋全字段脱敏（phase1_output.rs:34-58）；三态 WithOutput/NoOutput/Failed（phase1.rs:36-40,229-240） | 同模板（**逐字节一致**，本轮 diff 证实）；system＋user 两消息、无 tools（phase1.rs:114-133）；**不发任何 response_format/json_schema**——机器级保证移到响应侧严格解析（`deny_unknown_fields`，phase1_output.rs:22-42）；三态同构（phase1.rs:139-186）；失败原因先脱敏（本地加严） | 已批准差异（结构化输出下沉为解析侧约束）＋一致（模板/解析严格度/三态） | 本地依赖模型自觉返回 JSON；非 JSON → Failed＋1h 退避重试 3 次。供应商模型遵循度低时提炼成功率低于上游强约束路径；用户可经 `request_params`+`supported_params` 门自行透传结构化参数（api.rs:142-160） | 上 write/src/phase1.rs:258-321、client.rs:1055-1091 [S]／本 memory/phase1.rs:9-22,114-186、phase1_output.rs:22-42；api.rs:142-160 [S] |
| 6 | 任务领取与并发 | `begin_immediate`＋条件 INSERT..SELECT..ON CONFLICT；所有权 `Uuid::new_v4` token＋worker_id；租约 3600s 可抢占；`retry_remaining` 3 次、新水位重置；stage1 并发 `buffer_unordered(8)`（phase1.rs:179-190）；phase2 全局单例＋90s 心跳＋成功 6h 冷却（memories.rs:684-856,1092-1253；write/lib.rs:84,109） | 同构 SQL（**逐字一致**，仅 rusqlite 位置参数 `?N` 标记差异，本轮 diff 证实）；token＝纳秒-pid-计数器（非 UUID）；租约/心跳/重试/冷却常量相同（3600s/90s/3 次/6h）；stage1 串行逐任务、phase2 心跳独立线程 5ms 轮询（phase2.rs:33-41）；水位语义同（成功＝input_watermark / max） | 一致（SQL/语义）＋实现替换（token 格式、并发模型） | 行为等价边界：单进程内并发由互斥连接保证；上游 8 路 tokio 并发本地为串行——吞吐低但无交叉风险；`max_claimed` 本地只作领取数、上游同时充当全局 running 上限（上游自身耦合，见 §6-F4） | 上 state/src/runtime/memories.rs:684-856,1092-1253 [S]／本 memory/store.rs:206-360,551-720,1290-1321 [S]（SQL diff 本轮实证） |
| 7 | 来源选择与同步 | 非空行＋`last_usage`/`source_updated_at` ≥ cutoff；排序 usage_count→COALESCE→source→thread（**thread DESC**）＋终选 thread ASC 稳定；分页 512；同步 rollout_summaries＋raw_memories.md（V1）；keep 集外摘要删除（memories.rs:455-550；write/storage.rs:13-136） | 同排序/分页/终选（store.rs:1140-1233）；同步序与文件面一致（storage_files.rs:59-211）；文件名 stem 规则一致（UUID 时间戳/短哈希/slug≤60，storage_files.rs:229-306） | 一致 [S]＋[T]（a19 选择序黄金） | 无用户可见差异；同链已证文件集合逐字黄金 | 上 memories.rs:455-550、write/src/storage.rs:23-136 [S]／本 memory/store.rs:1140-1233、storage_files.rs:59-306 [S] |
| 8 | 差异判断与基线 | gix **库内**树对比：HEAD 树 vs 当前 fs 递归收集（仅排除 `.git`），BTreeMap diff＋库内统一 diff 渲染（git-utils/src/baseline.rs:60-141,169-330）；基线损坏→删 `.git` 重建（reset_git_repository，:98-108）；无变化＋产物校验通过→短路（phase2.rs:131-152）；写后校验 symlink 清零/MEMORY.md/summary 首行 v1（workspace.rs:76-115） | **外部 git 进程**：固定 argv＋`--git-dir/--work-tree` 锚定、`env_clear` 隔离（PATH/LC_ALL/GIT_CONFIG_* 屏蔽）、进程组终止＋60s 超时（workspace.rs:151-221）；`add -A -f`＋pathspec 排除 sqlite；基线 `commit --amend` 保单提交（:678-712）；短路/校验同（phase2.rs:224-230；workspace.rs:717-740） | 实现替换（G8 批准）＋一致（语义） | 等价条件：git≥可用且行为标准——同链以真实 git 单提交/diff clean 互证 [T]。本地多两处适配：DB 在根内需 pathspec 排除、每次 diff 先 `add -A`（索引副作用，自愈，见 §6-C6） | 上 git-utils/src/baseline.rs:60-330、write/src/workspace.rs:16-49 [S]／本 memory/workspace.rs:151-221,322-336,372-430,484-740 [S] |
| 9 | 摘要注入 | `MemoriesExtension` ContextContributor 读 memory_summary.md，截 2500 token，`developer` 角色 `PromptFragment`（ext/memories/prompts.rs:35-64；core/context/memory.rs:21-27；extension.rs:94-102）；记忆根加 readable roots（config/mod.rs:4319-4321）；每次贡献时重读（未见轮内缓存） | 轮开始读一次（含 None 快照固定，轮内不重读、不入史，turn.rs:302-309）；截 2500 token（inject.rs:16-27）；**追加到唯一 system 消息尾段** `<memory-context>`＋中文界定语（D4 批准适配，prompt.rs:290-305） | 已批准差异（developer→system，D4）＋实现替换（轮内快照 vs 每次重读） | 角色语义等价**未验证**（规格既有登记）；本地轮内快照意味着合并中途摘要更新不影响本轮——与上游每次重读相比是更保守的快照语义 [T]（M9 同链唯一 system/轮内相同断言） | 上 ext/memories/src/prompts.rs:35-64、core/src/context/memory.rs:21-27 [S]／本 memory/inject.rs:19-42、prompt.rs:290-305、turn.rs:302-309 [S] |
| 10 | 检索与工具边界 | V1 无专用工具：read_path.md 指导主会话模型用 shell 检索记忆根（read_path.md:33-41）；V2 另有专用工具（§4）。工具面＝Codex 全量工具＋沙箱 | 用户轮工具面＝exec_command＋apply_patch（prompt.rs:298-300），read_path 指引随 memory-context 注入；**合并轮**另注册恰三项 `mem_read/mem_write/mem_list`（mem_tools.rs:29-100；turn.rs:321-325）；mem 工具硬边界：拒绝对路径/`..`/隐藏/排除名/symlink 组件（mem_tools.rs:218-260） | 已批准差异（D3 合并轮受限工具）＋一致（V1 通用检索路径） | 用户可 rg 检索记忆根（M9 同链真实 exec-rg 实证 [T]）；合并轮无 exec——比上游合并代理的工具面更窄（上游为全量工具＋WorkspaceWrite 沙箱，phase2.rs:296-344） | 上 ext/memories/templates/memories/read_path.md:33-41、write/src/phase2.rs:296-344 [S]／本 memory/mem_tools.rs:29-100,218-260、prompt.rs:293-305 [S] |
| 11 | 引用反馈 | `<oai-mem-citation>` 解析（read/src/citations.rs:6-81）；**每条模型响应完成时**经 stream_events_utils 解析并 `record_stage1_output_usage`（core/src/stream_events_utils.rs:78-207，本轮补验快照）；SQL 单事务逐 ID `usage_count+1`/`last_usage`（memories.rs:60-91）；shell 读取只发遥测不计数（usage.rs:30-71；core/memory_usage.rs:9-51） | 同解析语义（feedback.rs:37-118，含跨块去重/旧标签兼容）；**用户轮完成位点**一次性 best-effort（main.rs:413-417、tui_host.rs:294）；同 SQL（**逐字一致**，本轮 diff 证实，store.rs:522-542）；打开不计数＝分类仅观测（feedback.rs:175-193） | 一致（解析/SQL/不计数语义）＋实现替换（接线位点：轮级 vs 每响应）＋范围外（遥测未移植） | 等价条件：最终回复含引用即计数——同链重复引用 B 恰 +1 实证 [T]；差异：上游每条响应都解析（多响应轮可累积），本地只解析最终回复文本；上游有遥测，本地无消费方故未移植 | 上 memories/read/src/citations.rs:6-81、core/src/stream_events_utils.rs:78-207、state mem memories.rs:60-91 [S]／本 memory/feedback.rs:37-118,198-226、store.rs:522-542 [S] |
| 12 | 用户便条 | V1：模型经通用工具写文件，仅 read_path.md 指引约束（:117-123），**无技术校验**；V2：专用 `memories.add_ad_hoc_note`（正则/长度 24-128/正文非空/create_new 不覆盖/拒 symlink，ext/memories tools+local/ad_hoc_note.rs） | V1 同路径（指引＋通用工具写入，exec/apply_patch 无路径边界）；**另加 D12-A 差异层过滤**：非法便条（目录外/子目录/文件名正则不合/正文空白）不入差异状态列表与正文、盘上保留、`mem_read/mem_list` 旁路照常（workspace.rs:514-560,600-666） | 已批准差异（D12-A＝本地在 V1 之上的增强）＋一致（V1 写入路径） | D12-A 使非法便条不污染合并输入（上游 V1 会原样进 diff）；旁路保留＝过滤不是隔离（代码注释自认「提示词不构成技术隔离」，workspace.rs:516-518）；V2 专用工具校验为范围外 | 上 ext/memories/src/tools/ad_hoc_note.rs:22-57、local/ad_hoc_note.rs:84-147、read_path.md:117-123 [S]／本 memory/workspace.rs:514-660、templates/read_path.md:117-123 [S] |
| 13 | 30 天/14 天清理 | prune SQL：`selected_for_phase2=0 AND COALESCE(last_usage,source_updated_at)<cutoff` 三键排序 LIMIT（memories.rs:405-438，调用 phase1.rs:96-117/start.rs:77）；资源 14 天：extensions/*/resources 直属 .md、UTC 前缀、批 200（write/extensions/prune.rs:9-96，调用 phase2.rs:207） | **SQL 逐字一致**（本轮 diff 证实，store.rs:1099-1129）；资源清理规则一致（retention.rs:18-95）；调用点同位（runner seed 后＋phase2 同步后） | 一致 [S]＋[T]（retention 7 例边界含 ±1s/等号） | 无差异；等号侧两侧同为 DB `<`/资源 `<=`（M9 变异 m2 检出证明） | 上 state mem memories.rs:405-438、write/src/extensions/prune.rs:9-96 [S]／本 memory/store.rs:1099-1129、retention.rs:18-95 [S] |
| 14 | 状态命令与重置 | TUI 设置面板（use/generate/reset 三项＋确认弹窗，tui/bottom_pane/memories_settings_view.rs:81-99,254-343）；重置＝`clear_memory_data`（DB 事务删 stage1_outputs＋jobs＋进度归零，memories.rs:1439-1466）＋`clear_memory_roots_contents`（清三根目录，write/control.rs:3-45）；在途保护＝心跳所有权确认＋无主失败恢复（phase2.rs:420-445；memories.rs:1368-1402） | `/memory-status`（进程内触发数/库计数/摘要存在性，runner.rs:429-479）＋`/memory-reset confirm`（**排他 flock 内清根＋`.epoch` 代数自增**，runner.rs:487-544）；旧任务失效＝各阶段边界代数比对＋任务快照（runner.rs:249-303；phase2.rs:160-167；mem_tools.rs:378-398） | 实现替换（代数/文件锁 vs 行删除/心跳确认）＋一致（目标语义） | 两侧均保证：重置后旧任务不重置新基线、不污染新库。本地机制更强（文件锁＋代数跨阶段复核）；上游靠心跳在成功前复核所有权。Windows 无 flock 时本地退化仅检查（既有暂缓登记） | 上 tui settings_view:81-343、state mem memories.rs:1439-1466、write/src/control.rs:3-45 [S]／本 memory/runner.rs:429-544、phase2.rs:160-167 [S] |
| 15 | 可观察性 | 完整指标系（write/read/ext 三 crate metrics：E2E_MS/JOBS/TOKEN_USAGE/STORAGE_BYTES，自动带 memory_version tag）；遥测会话；配额门 `guard::rate_limits_ok`（guard.rs:9-66，backend 限流窗口剩余百分比）；`tracing` 日志 | 无指标/无遥测/无配额（D8/A14 登记）；仅 `/memory-status` 进程内计数；storage/retention 有可注入告警回调但生产全部丢弃（`&mut |_| {}`）；管线完全静默 | 范围外（A14 配额、遥测）＋已批准差异（观测面） | 后台失败本地不可见（无日志文件）；上游可观测。配额门缺失＝配额紧张时本地照跑记忆任务（本地也无 backend 配额接口可查） | 上 write/src/guard.rs:9-66、runtime.rs:90-135、phase1.rs:505-570 [S]／本 memory/runner.rs:127-138,279；phase2.rs:210-213 [S] |

## 3. 实现差异表及关键调用链

| 维度 | 上游 | 本地 | 用户影响与取舍 |
|---|---|---|---|
| 数据库与事务 | sqlx 异步连接池＋busy 重试＋quick_check 校验＋BackupAndRebuild 恢复（state/sqlite.rs:37-129）；记忆库在 `CODEX_HOME/memories_1.sqlite`（**根外**，sqlite.rs:43,73）；threads 元数据与记忆库**双池**（memories.rs:33-41） | rusqlite 单连接 `Mutex`＋busy_timeout 5s；`journal_mode=DELETE`＋`synchronous=FULL`（store.rs:129-159）；库在 `<记忆根>/memories.sqlite`（**根内**，store.rs:1263-1266）；threads 与产物同库 | 根内 DB 迫使两处适配：git 基线 pathspec 排除 sqlite（workspace.rs:322-346）＋reset 前 `rm --cached`（:683-695）；DELETE+FULL 每 commit fsync、吞吐低但崩溃安全强；单库无双池往返（上游疑点 13 的 2N+1 查询在本地自然消解） |
| 布局与迁移 | `CODEX_HOME/memories/`（文件产物）＋`memories_1.sqlite`（库）分离；迁移由 state 运行时统一管理（runtime_memories_migrator） | `<user_dir>/memories/` 单根含一切（sqlite/Markdown/.git/.epoch）；迁移＝`schema_migrations` 登记表按文件名序 DDL＋登记同事务（store.rs:166-193） | 本地根自包含、可整体重置/打包；代价是 git/清理必须显式排除非产物文件 |
| 异步与进程 | tokio 全异步（spawn 管线、buffer_unordered(8)、spawn_blocking 文件 IO、独立代理线程）（start.rs:59、phase1.rs:179-190、runtime.rs:376-444） | `std::thread` 管线＋`thread::scope` 心跳＋双流读取线程；无 tokio 于记忆面（runner.rs:208；phase2.rs:249；workspace.rs:75-81） | 单机 CLI 场景线程模型足够；串行 stage1 吞吐低于上游 8 路并发（配置 max_rollouts_per_startup=2 时无实际差距） |
| Git 实现 | gix 库内：内存树对比＋库内 diff 渲染，身份 `Codex <noreply@openai.com>`（baseline.rs:162-171）；`read-tree --reset` 借外部 git（git_process.rs:65-97） | 外部 `git` 进程全家：argv 固定、env 隔离（GIT_CONFIG_COUNT=0 等）、身份 `terminal-chat <memory@terminal-chat.invalid>`、gpgsign 关（workspace.rs:86-87,151-179）；reset＝amend 保单提交（:678-712） vs 上游删 `.git` 重建（baseline.rs:98-108） | 等价条件＝系统 git 行为标准 [T] 同链单提交/diff clean；本地 amend 保留对象历史更少、上游重建更彻底（都能把基线推到当前态） |
| 模型协议 | Responses API：`output_schema`→`text` 参数（client.rs:1055-1091）；stage1 走 detached ModelClient 流式（runtime.rs:314-397）；phase2 走完整内部代理线程＋沙箱（WorkspaceWrite 无网络，phase2.rs:296-344） | Chat Completions 非流式：`{messages,tools}`＋request_params 透传（api.rs:289-319）；stage1 无 schema；phase2 进程内工具循环恰三项 mem 工具、无沙箱概念 | 协议面是**根本分叉**：本地适配 OpenAI 兼容 chat 端点生态。强 schema 缺失影响见 §2-5；合并代理工具面上游更宽（全量＋沙箱写根）vs 本地更窄（mem 工具无 exec）——本地以更窄面换取无需沙箱 |
| token 预算与截断 | `rollout_token_limit`＝窗口×effective_percent×70%，回退 150k（write/prompts.rs:121-129）；注入 2500 token（ext lib.rs:16） | 同公式（无 effective_percent 中间因子——上游默认 100% 等价，注释明示）；同回退 150k（phase1.rs:56-85）；同 2500（inject.rs:16）；近似换算 tokens×4 字节（truncate.rs:51-53 shim） | 等价 [S]；两侧同为近似换算非 tokenizer 精确计数 |
| Agent 循环与权限 | 合并代理＝完整 Codex 线程：cwd=记忆根、approval Never、MCP 清空、多项 feature 关闭、沙箱 WorkspaceWrite{writable_roots:[root], network:false}（phase2.rs:296-344）；phase1 detached JwtOnly | 合并代理＝本地工具循环：仅 mem_* 三工具、无 approval/MCP/沙箱概念（turn.rs:186-196,321-325；phase2.rs:282-294）；用户轮 exec/apply_patch 无沙箱（产品既有限制，非记忆面新增） | 本地工具面窄＝能力上限低（不能在合并轮跑 shell）但无需沙箱即安全；上游能跑 shell 但须沙箱兜底 |
| 时钟/ID/水位 | `Utc::now` 直取（无注入面，测试用 `*_with_now`）；token＝UUIDv4；水位 enqueue 时 max 或 +1（memories.rs:1528-1532） | `Clock` trait 注入贯穿（store.rs:40-50；runner.rs:219 生产恒 system_clock）；token＝纳秒-pid-计数器；水位同 max/+1（store.rs:1316-1320） | 本地可测性更强（确定性同链依赖它 [T]）；ID 格式差异不影响 SQL 相等语义 |

**关键调用链对照**（同链两侧）：
- 提炼：上游 `start_memories_startup_task → phase1::run → claim_stage1_jobs_for_startup → job::sample（rollout_input＋prompts＋stream_stage_one_prompt）→ mark_stage1_job_succeeded`；本地 `spawn_memory_pipeline → run_memory_pipeline_once → claim_stage1_jobs_for_startup → run_stage1_job（read_recent_rollout_items＋build_stage_one_request＋chat_with_body）→ mark_*`。环节一一对应 [S]。
- 合并：上游 `phase2::run → try_claim_global_phase2_job → prepare_memory_workspace（ensure_git_baseline_repository）→ get_phase2_input_selection → sync_phase2_workspace_inputs → memory_workspace_diff → spawn_consolidation_agent（agent::handle 心跳循环）→ validate → heartbeat 确认 → reset_git_repository → mark_succeeded`；本地 `run_phase2_consolidation → 同名领取 → ensure_baseline → get_phase2_input_selection → sync_stage1_snapshot → memory_workspace_diff → 工具循环 → validate_consolidation_artifacts → epoch 复核 → reset_memory_workspace_baseline → mark_succeeded`。阶段序一致，差异在代理载体与失效机制 [S]。

## 4. 上游额外能力与本地未实现项（范围外，非 V1 缺陷）

| 能力 | 上游位置 | 说明 |
|---|---|---|
| V2 记忆管线 | write/（stage_one_*_v2 模板、分层输入 serialize_tiered_input、无 raw_memory、摘要 9000 截断、四标题校验 is_valid_v2_summary） | 版本切换＋`dual_write` 同跑两套（start.rs:40-47）；013 规格明确排除 |
| 专用记忆工具 | ext/memories/src/tools/{search,read,list,ad_hoc_note} | search 多模式（any/all_on_same_line/all_within_lines）、read/list 同本地 mem 工具能力、add_ad_hoc_note 强校验；`dedicated_tools` 开关门控 |
| 配额门 | write/src/guard.rs:9-66 | backend 限流窗口剩余百分比检查，不足则跳过本轮启动 |
| 污染标记 | state memories.rs:632-666＋core session 接线 | MCP/web search 外部上下文→`memory_mode='polluted'` 不再提炼；本地无外部上下文源，概念自然不适用 |
| 指标与遥测 | write/read/ext 三处 metrics＋core/memory_usage.rs | E2E/任务/token/存储字节指标＋读取分类遥测 |
| 专用模型配置 | extract_model/consolidation_model＋provider 偏好回退 | D6 已裁决用会话主模型 |
| 外部 agent 记忆导入 | external-agent-migration crate | 范围外 |
| 服务端 trace 摘要端点 | codex-api/src/endpoint/memories.rs:32-59 | 与本地两阶段管线平行的服务端路径，未纳入对照结论 |

## 5. 验证覆盖差异

| 面 | 本地已有证据 | 上游 |
|---|---|---|
| 确定性同链 | M9 `memory_lifecycle` 同链 [T]：登记→提炼→同步→合并→注入/检索（真实 exec-rg）→引用→过期再合并→物理删除→新版本再提炼，12 份黄金＋SQL/请求断言；mock 模型、真实 SQLite/Git/Session/工具 | 上游有自身测试套件（storage/workspace/phase2_sandbox 等，快照内见 mod 声明），本轮未运行、不引用其结论 |
| 真实供应商 | M7 三项独立真实样本 [T]（非空提炼/合并工具链/注入用户轮，各自通过）；**真实同链未验证**（既有登记）；补验 harness 的请求大小/超时/额度限制是 harness 参数，**非产品默认行为**（本轮明示区分） | 上游生产环境隐含验证，无公开对照数据 |
| 错误检测 | 18 项变异 17 检出＋1 等价排除 [T]（M9 记录 §4）；D12-A/引用计数/接线断开均有变异证明 | — |
| 整链回归 | workspace 3010/0/3、e2e 106/0/0 [T]（V1 收口引用值，本轮未重跑——本轮仅调查未改代码，引用成立） | — |
| 本轮运行 [R] | 仅只读检查：模板/SQL diff、行号抽验、上游树下载校验（全部命令与结果见 §1 口径）；无测试重跑、无真实模型调用、未触碰真实用户库/会话/记忆目录 | — |

**mock 与真实样本各证明什么**：mock 同链证明控制流与数据传播的确定性正确（含 30/14 天边界、selected 保护、水位）；M7 真实样本证明真实供应商路径上提示词/解析/注入可用。**没有任何证据证明**：真实供应商同链端到端、developer/system 角色等价、其他供应商行为、真实模型遗忘判断质量——保持既有「未验证」登记。

## 6. 重要发现

### A. 可能影响当前正确性的缺口（登记，不修复）

- **A-1 stage1 失败记账 `retry_remaining-1` 无下限钳制**（store.rs:499；上游同病 memories.rs:1046 [S]——**上游继承而非本地回归**）。领取条件理论上挡住 0 值行，出现负值需先有异常路径；phase2 两侧都有 `max(...,0)`。
- **A-2 反馈路径可能建库**：`after_user_turn` 引用非空时 `MemoryStore::open` 会建根/建库/跑迁移（feedback.rs:218-220），与 `/memory-status` 的「无库不建库」守卫（runner.rs:463-467）不对称。触发条件窄（enabled 且回复恰含合法引用块且此前无库）。
- **A-3 diff 前置 `add -A` 的索引副作用**（workspace.rs:489）：读路径改写索引，崩溃夹缝留已暂存态（后续操作重暂存自愈）；上游 gix 路径纯只读。
- **A-4 注释与代码漂移**（文档-代码不一致，单列不静默改史）：runner.rs:40-47 头注释仍述「`.epoch`＋`.pipeline-epoch` 两文件」，实现已是 `.epoch`＋内存任务快照（phase2.rs:163、workspace.rs:365-367），`.pipeline-epoch` 不存在；store.rs:10-11 称「避免新增 uuid 依赖」而 storage_files.rs:17 已 `use uuid`。
- **A-5 心跳 5ms 轮询**（phase2.rs:39）：模型等待期 ~200Hz 空转（有界、请求返回即止）；上游 1s 状态轮询。

### B. 已接受且有用户影响的差异

- **B-1 无结构化输出强约束**（§2-5）：提炼成功率依赖模型 JSON 遵循度，失败→1h 退避×3 后该版本放弃（水位不重复）。最值得关注的单一差异。
- **B-2 developer→system 注入适配**（D4）：角色语义等价未验证；个别供应商对两角色权重不同时注入效力可能不同。
- **B-3 触发频率**（D2）：每轮 vs 每会话启动——本地闲置窗更早满足、空跑更多（`PIPELINE_SPAWNS` 如实计数）。
- **B-4 观测静默**：后台失败无日志/指标，用户只能从 `/memory-status` 间接察觉。
- **B-5 无配额门**（A14）：配额紧张时照跑。
- **B-6 记忆用会话主模型**（D6）：无便宜专用模型路径。

### C. 纯实现替换（行为等价或目的等价，已证或有界）

gix→外部 git（同链互证 [T]）；sqlx 池→Mutex 连接；tokio→线程；UUID token→纳秒-pid 串；Responses 流式→Chat 非流式；上游删 `.git` 重建→amend 保单提交；DB 根外→根内（带 pathspec/rm --cached 适配）；时钟注入面（本地增强可测性）。

### D. 未来可选能力（按价值排序的个人评估，非裁决）

结构化输出透传预置档（在 `supported_params` 门内为记忆请求自动附 response_format）；`extract_model` 类专用模型配置；指标最小集（任务计数落库）；V2 专用工具与 add_ad_hoc_note 强校验；配额门（出现 backend 配额接口后）。

## 7. 面向使用者的结论

**现在能做什么**：开启 `manual.json` 的 `memories` 段后，每个会话轮结束后台提炼闲置会话（10 天窗/6h 闲置），30 天未用结果与 14 天扩展资源自动清理；全局合并产出 `MEMORY.md`/`memory_summary.md`（单提交 git 基线托管变更），后续用户轮自动注入 2500 token 摘要并可经 exec/rg 检索原文；回复引用计数保鲜（被引来源不老化）；用户可让模型写 `extensions/ad_hoc/notes/` 便条（非法便条不进合并输入但旁路可读）；`/memory-status` 看状态、`/memory-reset confirm` 带代数保护重置。以上同链有确定性实证 [T]。

**与 Codex 哪里不同**：核心数据流（候选→提炼→合并→清理 SQL、阈值、租约/水位语义）与固定上游逐字或语义一致（SQL/模板 diff 实证）；分叉在载体与协议——外部 git、单连接 SQLite（库在记忆根内）、Chat Completions 无强 schema、system 尾段注入、线程模型、无指标/遥测/配额/沙箱、D12-A 便条差异过滤为本地独有增强。

**最值得关注**：① B-1 结构化输出缺失对提炼成功率的影响（可先在真实供应商上观测 Failed 率再决定是否补预置档）；② B-2 角色语义等价未验证（换供应商时留意注入效力）；③ A-2/A-4 两处小的文档-行为不一致与反馈建库守卫（低风险，修复属后续任务，本轮未动）。

---

*本轮产物：仅本报告一个新文件。未暂存、未提交、未合并、未推送、未删除分支；V1 已交付状态不变；未调用真实供应商；未访问真实用户数据库/会话/记忆目录。AGENTS.md 既有并发修改原样保留。*

## 8. 建议复核与处置判断（2026-10-20 复核轮）

复核轮：仅调查、讨论与记录；未改产品代码/测试/配置，未实施任何建议。方法＝源码追踪→可达性分析→临时目录探针实证→影响判断；「Codex 如此实现」不作为本项目必须跟随的理由。上游快照复用 `/tmp/013-cmp-up/tree/`（补取 app-server/cli 的 Cargo 与若干源文件）。

### 8.0 本轮实际证据

| 探针/检查 | 命令与位置 | 结果 |
|---|---|---|
| Rust 隔离探针（项 1/7） | `/tmp/013-probe/`（path 依赖＋专属 `--target-dir /tmp/013-probe-target`，`--offline`），纯临时目录＋合成数据＋固定时钟 1_700_000_000 | 构建成功、运行 **exit 0**；S1–S5 输出见各条目；探针临时根在 `/tmp/013-probe-*` |
| Git 对象保留探针（§8.1-E3） | `/tmp/013-git-a.*`（amend 变体）与 `/tmp/013-git-b.*`（重建变体），复刻生产 argv/env/GIT_*_DATE 序列（workspace.rs:151-179,449-450） | 见 §8.1-E3 |
| 官方文档核对（项 3） | WebFetch DeepSeek JSON mode 与 OpenAI Structured Outputs 指南 | 仅查文档，未发送任何供应商请求 |
| 上游触发调用方追查 | core/Cargo.toml 只依赖 `codex-memories-read`（无 write）；app-server 与 cli 依赖 write，已 grep 其 lib/message_processor/thread_processor/extensions/in_process 等 | **调用点未定位**——见 §8.1-E1 降级说明 |

### 8.1 勘误（原表述→修正结论→依据；不静默改写上文）

> **2026-10-20 开关收口轮追加 E5/E6**（详见 §9）：E1 的「上游调用点未定位」已由本轮 turn_processor.rs:706-715 补正；§8.2-4 中「清理随生成暂停＝上游同构」经源码证伪（E6）。§8.5 留给用户的两点语义决定已被固定上游源码确定，由 §9.5 接替——§8.5 原文保留作历史。

- **E1 触发时机**。原 §7（:111）：「每个会话轮结束**后**台提炼闲置会话」。修正：本地管线在**新用户轮成功启动时**触发——`main.rs:340-356`/`tui_host.rs:165-180` 在 `run_tool_turn` **之前**调用 `spawn_memory_pipeline`（已批准 D2 契约原文即「成功启动后触发」；§2-2 的表述正确，§7 概括失准）。依据：本轮 sed 复核两驱动调用点。上游「每会话启动一次」的证据等级降为「README＋start.rs 函数契约」（README.md:31-38、start.rs:24-59 [S]）；core 不依赖 memories-write、app-server/cli 依赖但已查文件未见调用点，**不足以支持任何改变本地触发频率的建议**。
- **E2 D12-A 范围**（原 §2-12「不污染合并输入」、§7「不进合并输入」）。修正：非法便条不进入的是**自动提供的差异面**（差异状态列表＋统一 diff 正文，workspace.rs:514-560——合并代理的自动输入面）；合并代理仍可经 `mem_list`/`mem_read` 主动发现并读取这些文件（旁路照常，D12-A 批准原文）。不能概括为「完全不进入合并模型」。上游 V1 无此过滤、非法便条原样进 diff——该对比本身成立。依据：本轮复核过滤层实现与 D12-A 登记；边界不重开。
- **E3 Git 单提交与对象保留**（原 §3：「本地 amend 保留对象历史更少、上游重建更彻底」）。修正：两者 HEAD 可达提交数同为 1（「单提交」合同两侧都满足）；但磁盘上 amend **保留更多**——生产 env 隔离不关 reflog（core.logAllRefUpdates 默认开），每次 amend 的旧提交/树/对象经 reflog 留存，直到 reflog 过期＋gc 才回收；上游删 `.git` 重建则物理清除全部旧对象。探针实证（同一生产命令序列、3 次基线更新后）：amend 变体 reflog 4 条、松散对象 28 个（112KB）、.git 368KB；重建变体 reflog 1 条、对象 4 个（16KB）、.git 184KB；两变体 `fsck --unreachable` 均 0（amend 侧对象是 reflog 可达而非不可达）。结论：原句方向写反；长期运行的记忆根会温和累积松散对象（每次基线更新约 8 对象/十余 KB 级），非正确性问题。
- **E4 request_params 透传适用性**（原 §2-5「用户可经 request_params＋supported_params 门自行透传」）。修正：机制为真但适用面有误导——`conn.request_params` 经 `chat_with_body`（api.rs:289-319）注入**该连接的每一个请求**（用户轮、phase1、phase2 工具循环同走此函数），无法只作用于 phase1。若用户为帮助提炼配置 `response_format=json_object`，用户轮与合并轮同样被迫 JSON 模式（正常聊天被破坏）。故「自行透传」不构成 phase1 格式约束的可用通路；见 §8.2-3。

### 8.2 九项逐项复核

**1. 引用反馈建库（已复现；建议低优先修复——评估仅）**。代码事实：`after_user_turn_with_clock`（feedback.rs:208-226）在 `enabled` 后**先** `MemoryStore::open`（建根＋建库＋迁移，store.rs:141-160）**再**解析引用——与原报告「引用非空时 open」相比更宽：每个完成轮都开库。探针：S1（enabled、无根、无引用回复）→ `root_exists=true db_exists=true`（**建库复现**）；S3（`/memory-reset confirm` 后带引用回复）→ 库被重建（`db_before=false db_after=true`，新库 outputs=0、`.epoch` 仍在）；S4（`/memory-status` 全新目录）→ 零创建（守卫有效，runner.rs:463-467）；S2（引用未知 ID）→ 缺行忽略、无误行。影响评估：无数据错误（缺行忽略语义保持、重置代数不受影响、SQL 落新空库无污染）；实际影响＝①与 `/memory-status` 的「读路径不建库」原则不对称；②重置后文件自行重现（用户刚清空又见 memories.sqlite）；③每轮一次开库（pragma＋迁移检查）轻微开销。是否违反契约：不违反 enabled/缺行忽略/重置代数三条明示契约；违反的是 status 已体现的「读不建库」隐含原则。最小修复范围（不实现）：把开库移到「引用解析非空之后」＋改用「仅打开既有库」变体（不存在→返回 None，不建目录/库/迁移）；并发边界＝无需新锁——重置竞态窗口内至多少计一次使用（引用指向已删行，SQL 缺行忽略天然覆盖）。

**2. 后台失败可见性（可小幅补强；建议中低优先）**。现状 `/memory-status`（runner.rs:429-479）能区分：未启用／根或库未初始化／摘要存在性／本进程触发数／产物条数／任务三态计数（running+pending 合并、done、error）。**不能**区分：冷却等待（phase2 done 后 6h 冷却与「无事可做」显示相同——冷却可由 jobs.finished_at 推导但未展示）；最近失败的原因/阶段/重试时间（`jobs.last_error`＋`retry_at` **已持久化**但未展示——stage1 原因为脱敏后的真实错误文本 phase1.rs:149，phase2 为阶段名 failed_prepare_workspace/failed_sync_workspace_inputs/failed_agent 等 phase2.rs:166-299）；成功有结果 vs 合法空结果（outputs 条数与 done 计数差值可近似但未呈现）；stage1/phase2 运行中不分列（kind 未分列）。告警回调（storage/retention 的 `warnings`）生产侧全部丢弃（phase2.rs:210-213 等）。最小补充评估：展示「最近一条 error 任务：kind、last_error 截断（如 200 字符）、retry_at 剩余分钟」＋「phase2 冷却剩余」——全部复用既有持久字段，**无需新增 schema**；保持先查文件存在再开库（不建库）、输出有界、原因已脱敏（写入前 redact_secrets＋api 层 redact_credential 双重）、显式命令不构成 TUI 污染。不引入日志/指标/遥测。

**3. 提炼请求格式约束（证据不足，暂缓）**。三层区分：①提示词要求 JSON——本地已有（stage_one_system.md 交付物节）[S]；②合法 JSON 模式（`response_format={"type":"json_object"}`）——DeepSeek 官方支持：仅保证语法合法 JSON，**不保证 schema**、可能返回空内容、要求提示词含 "json" 字样（本地满足）与合理 max_tokens（DeepSeek JSON mode 指南）；OpenAI 亦有此层；③严格 JSON Schema（structured outputs `json_schema`＋strict）——OpenAI 独有保证（schema 遵从＋可检测 refusal；Responses `text.format` 与 Chat Completions 均可）；**DeepSeek 按官方文档无 JSON Schema 层**。当前所选供应商 deepseek/deepseek-flash（manual.json）最高只能到第②层，而第②层**不能替代**本地响应侧严格解析（deny_unknown_fields 的字段校验仍必要）也不能强迫非空记忆（空内容→本地 SucceededNoOutput 语义保持，反而是正确归宿）。通路：`request_params` 连接级全局面（E4）→ 当前架构无法只对 phase1 注入；要做得改按请求注入（新面）。失败率证据：**没有**——记忆默认关闭、无失败计数持久化（项 2 所述）、真实提炼样本仅 M7 一例且成功。判断：无数据支持优先补；若未来补，属「按请求注入＋supported_params 确认＋不支持时优雅省略（回落提示词＋解析）」的最小改动，并需先有失败率观测（依赖项 2）。

**4. 独立生成开关（能力缺口，值得补充；语义需用户决定）**。门控事实表（源码核实）：

| 动作 | enabled | use_memories |
|---|---|---|
| 后台管线（seed/prune/stage1/phase2＝生成＋清理） | 必须 true | 不参与 |
| threads 登记 `note_persisted` | 必须 true | 不参与 |
| 引用反馈 `after_user_turn` | 必须 true | 不参与 |
| 摘要注入 `read_memory_context` | 必须 true | 必须 true |

结论：**当前做不到「继续使用已有记忆但暂停后台生成」**——注入要求 enabled=true，而 enabled=true 即全面启动生成（上游 generate_memories=false＋use_memories=true 恰为此形态）。若加 `generate` 开关，建议控制：spawn 管线（生成＋seed＋**清理**随之暂停——上游同构，prune 在 startup 管线内）；继续运行：注入、引用反馈、`/memory-status`、`/memory-reset`。threads 表已有 `memory_mode` 列（migrations/0003，现恒写 'enabled'）——上游 generate=false 即写 'disabled'，本地列已预留该语义。需用户决定：①清理是否随生成暂停（暂停＝30 天老化停摆、记忆暂不失效；两者皆合理）；②登记继续但标记 disabled（上游式）还是停止登记。

**5. 专用记忆模型（暂缓——仅未来可能性）**。现状：`ModelSend{conn,key,model,agent}` 携带触发轮的 `selected_model`（runner.rs:159-166），phase1 预算取该模型 `context_length`×70%（phase1.rs:74-85）；phase2 同模型。上游 extract/consolidation 专用模型＋provider 偏好回退解决的是「用便宜模型跑后台」的成本/质量选择——本地当前**无任何**成本或质量问题的证据（无失败率、无用量数据；记忆默认关）。最小改动面（若未来做）：`MemoriesConfig` 增可选模型 id＋spawn 时按 id 从同一 catalog 解析 `ModelInfo`（预算随之切换）；凭据/origin 授权不变（同连接），失败行为不变；不新增路由框架、不改主会话模型。前置条件：真实使用中观测到成本或质量问题。

**6. 注释漂移（文档勘误，应修；本轮不改）**。两处现行注释仍在误导：①runner.rs:36-49「承载＝记忆根内两个代际文件…`.pipeline-epoch`：管线 prepare 时刻的代数快照」——实现已是 `.epoch`＋flock＋**内存任务快照 task_epoch**（phase2.rs:163、mem_tools.rs:378-398、workspace.rs:365-367），`.pipeline-epoch` 全库仅此注释提及（grep 证实）。建议表述：「承载＝`.epoch` 代数文件＋根目录 flock＋各任务领取时捕获的内存代数快照；写入入口在共享锁内检查＋写入，阶段边界比对快照失配即中止」。②store.rs:10-11「避免新增 uuid 依赖」——uuid 已是依赖（src/app/Cargo.toml，storage_files.rs:17 用于文件名解析）。建议表述：「生成侧无需随机 UUID，复用纳秒-pid-计数器即满足 SQL 相等语义（uuid 依赖仅用于既有文件名解析）」或删去动机句。

**7. stage1 重试计数下溢（正常流程不可达；无需修复）**。完整守卫链（探针 S5 实证）：领取时 INSERT..SELECT 条件含 `retry_remaining>0 OR 水位更新`（后者同时重置 3）→ 运行行恒 rr≥1；`mark_stage1_job_failed` 的 UPDATE 带 `status='running' AND ownership_token=?`（store.rs:503-505）→ 同令牌二次回调 0 行命中（探针：fail#2 `applied=false`，rr 保持 2）→ **重复回调/并发回调均不可下溢**；正常重试路径完整（同水位再领→SkippedRetryBackoff；新水位→可领且 rr 重置 3，探针实证）。唯一可达路径＝库外直接写损坏（探针将运行行 rr 置 0 后 fail → rr=-1，仅证明算术无钳制）。判断：非正常流程缺陷；上游同构（state memories.rs:1046 亦无钳制、phase2 两侧均有 max(...,0)）。可选的一行对称钳制属美化，不构成必要修复；不做。

**8. 心跳轮询（保留；成本未测量但代码事实不支持「忙等」定性）**。代码事实（phase2.rs:264-279）：独立线程 `std::thread::sleep(5ms)` 真睡眠（非自旋）；每次唤醒仅读受控时钟＋比较；仅当距上次 ≥90s 才执行心跳 SQL（`heartbeat_global_phase2_job`，租 3600s）；停止位由正常返回显式置位＋panic 时 Drop 兜底展开，请求结束 join 回收（≤一个轮询周期退出）。原报告「~200Hz 空转」表述保留但补正定性：这是每秒 200 次纳秒级唤醒（OS 定时器仲裁），非忙等；未做隔离测量，**成本标记为未测**（预计可忽略）。调整评估：不值得——简单延长 sleep 会①增大请求结束时 join 的最坏等待（5ms→1s 即每请求多等至 1s）②测试受控时钟下可能跳过 90s 采样点（注释自述该竞态曾被测试捕获）。四条保障（全程续租、及时停止、panic 回收、不挂死）现状均满足，保留。

**9. 应继续保留的实现（无实际问题即不替换）**。rusqlite 单连接＋DELETE/FULL（崩溃安全强、单进程场景无争用证据；V1 收口全量绿）；std::thread 模型（配置默认每轮领取 2，无并发吞吐需求）；外部 Git 进程（同链黄金互证；对象累积见 E3，量级温和、非正确性问题）；system 尾段注入（D4 批准；角色等价未证＝换供应商时留意，非替换理由）；受限 mem_* 工具（D3；工具面窄免沙箱）；可注入 Clock（确定性同链的根基）；epoch＋flock 重置（探针 S3 复证失效有效）。**不需要**：V2/双写/专用记忆工具/完整遥测——单用户 CLI、记忆默认关、无遥测消费方、无配额接口；仅当真实使用出现对应需求再议。

### 8.3 判断表

| 项目 | 代码与复现依据 | 实际影响 | 建议处理或保留 | 优先级 | 最小范围与前置条件 |
|---|---|---|---|---|---|
| 反馈建库 | feedback.rs:208-226＋探针 S1/S3/S4 | 原则不对称；重置后文件重现；每轮开库开销 | 应修复（低） | 低 | 解析先行＋「仅打开既有库」变体；无需新锁 |
| 失败可见性 | runner.rs:429-479；jobs.last_error/retry_at 已持久化（store.rs:489-514 等） | 冷却/失败原因/重试时间不可见 | 值得补充 | 中低 | status 增「最近失败＋冷却剩余」；零新 schema |
| 格式约束 | api.rs:289-319（连接级）；DeepSeek/OpenAI 官方文档；E4 | 提炼成功率依赖模型自觉；无失败率数据 | 证据不足，暂缓 | — | 前置＝先有失败观测（依赖上行）；做则按请求注入 |
| 生成开关 | 门控表（§8.2-4）；threads.memory_mode 列已预留 | 无法「只用不生成」 | 值得补充 | 中 | generate 门控 spawn＋登记标记；清理归属需用户定 |
| 专用模型 | ModelSend/phase1.rs:74-85 | 无实际问题的未来可能性 | 暂缓 | 低 | 前置＝真实成本/质量证据 |
| 注释漂移 | runner.rs:36-49；store.rs:10-11 | 误导维护者 | 文档勘误，应修 | 中 | 两处注释改写（§8.2-6 已给表述）；随任一后续代码轮顺带 |
| 重试下溢 | store.rs:206-360,489-514＋探针 S5 | 正常流程不可达 | 保留（不修） | — | — |
| 心跳轮询 | phase2.rs:264-279 | 未测、预计可忽略 | 保留 | — | — |
| 七项实现保留 | §8.2-9 | 无问题证据 | 全部保留 | — | — |

### 8.4 分类汇总

- **已复现、应修复的缺陷**：反馈路径建库（含重置后重建；探针复现；数据无错误、原则违反）。
- **值得补充的能力**：后台失败可见性最小展示（复用既有字段）；独立生成开关（语义两点待用户定）。
- **可继续保留的实现差异**：全部 §8.2-9 所列；重试下溢不修；心跳不调。
- **文档勘误**：E1–E4（本节登记）＋两处源码注释漂移表述（§8.2-6）。
- **证据不足或暂缓**：格式约束（无失败率数据；当前供应商最高仅 json_object 层）；专用模型；上游触发调用点未定位（不影响本地任何决策）。

### 8.5 推荐最小后续范围（讨论稿，非批准计划）

一笔小改动轮即可覆盖全部「应修＋值得」项：①反馈开库后移＋仅打开既有库；②`/memory-status` 增最近失败（kind/脱敏原因截断/重试剩余）与 phase2 冷却剩余；③两处注释勘误；（可选，需先决定语义）④generate 开关。其中 ④ 的「清理是否随生成暂停」「登记标记 or 停止」两点是**真正需要用户决定的事项**；①②③ 无语义争议。格式约束与专用模型建议先取得真实使用数据（依赖②的失败观测）再议。

---

## 9. 开关语义收口（2026-10-20 第二复核轮：固定上游源码核定，暂不实施）

方法：仅离线核查固定提交源码（本轮补取 `app-server/src/request_processors/turn_processor.rs`、`state/src/runtime/threads.rs`、`core/src/session/handlers.rs`、`tui/src/app_event.rs`）；文档与 README 只作线索不作证据。每项按「固定上游事实／源码位置／本地采用规则／影响与拟验收项」登记。不重开 D2/D3/D4/D12-A/G8。

### 9.1 勘误 E5–E8

- **E5 触发调用点（补正 E1 的「未定位」）**。固定上游事实：`start_memories_startup_task` 的调用点在 `codex-rs/app-server/src/request_processors/turn_processor.rs:706-715`——轮输入成功提交后，**`turn_has_input && started && is_primary_environment_configured` 三条件**成立即触发。即上游是**每个带输入、成功启动（非 steer、非拒提交）的主环境用户轮触发一次**，不是「每会话启动一次」（README.md:31-38 的说法相对该调用点已过期）。started 排除 steering（在途轮转向不重复触发）；本地无 steer 概念。影响：本地 D2（每带输入新用户轮过 route/origin 门触发一次，runner.rs:7-8 注释即引「上游 turn_processor 三条件」）与上游**结构同构**，此前「触发频率差异＝每轮 vs 每会话」的对比不成立，撤销该差异条目；不据此对本地触发做任何改动（D2 已批准）。
- **E6 清理边界（证伪 §8.2-4 的「上游同构」推断）**。固定上游事实：`generate_memories` **不参与** `start_memories_startup_task` 的任何门控（调用点三条件无它；start.rs:33-38 的跳过条件只有 ephemeral／非根 agent／`!Feature::MemoryTool`）。因此 generate=false 时后台管线照常启动，其内的 30 天结果清理（start.rs:76-77→phase1::prune）与 14 天资源清理（phase2 同步内 phase2.rs:207）照常执行。§8.2-4「spawn 管线（含清理）随之暂停——上游同构」为错误推断，予以证伪；§8.5 的「清理是否随生成暂停」不再是要用户决定的事项（源码已定：不暂停）。
- **E7 「读取根全停」的适用面（2026-10-20 修订规格轮追加）**。§9.2 开关表与 §9.3 中 Feature 关＝「不开放读取根（生成与使用全停）」的表述须收窄：所停的是**记忆系统自身的自动入口**——上游指记忆扩展的 readable-roots 授权与注入/专用工具面（config/mod.rs:4319-4321、extension.rs:46-54）；**不代表通用 exec/shell 工具无法读取记忆文件**（上游合并代理与用户轮通用工具本就无记忆专用路径边界）。本地对应事实一致：enabled 关＝不注入、不注册 mem 工具、不启动管线；用户轮 `exec_command`/`apply_patch` 无路径边界（exec.rs:149-196、apply_patch.rs:740-741），仍可读写记忆目录下文件——与上游同为「通用工具不设记忆边界」，非本项目缺口。
- **E8 「重新启用无补处理」的准确表述（2026-10-20 修订规格轮追加）**。§9.2 项 3④ 的「无代码证据支持自动补处理，不得承诺」细化为：**全局重新开启生成，不会批量恢复旧 disabled 会话的资格**（config 只影响此后新建/新插入线程，threads.rs:1054 插入路径；无任何批量 UPDATE memory_mode 的调用方——写者仅插入、per-thread setter、SessionMeta 重放与 polluted 标记）；**逐会话显式恢复资格后，能否与何时被处理仍由既有条件决定**——候选时间窗口（10 天年龄/6h 闲置）、成功水位、运行上限、退避与冷却，不因恢复资格而绕过，也不是对关闭期间历史的回溯补跑。

### 9.2 四项核定

**项 1 开关的职责**。固定上游事实（读写位置全列）：

| 开关 | 读取位置 | 写入位置 | 实际职责 |
|---|---|---|---|
| `Feature::MemoryTool` | start.rs:34（管线启动门）；ext/memories extension.rs:46-54（注入门）；core config/mod.rs:4319-4321（记忆根 readable roots） | features 配置（features/src/lib.rs:1291-1295，key "memories"，Stable，**默认关**） | 总闸：关＝不启动管线＋不注入＋不开放读取根（生成与使用全停） |
| `generate_memories` | session.rs:1084-1088（**新建**线程 `ThreadPersistenceMetadata.memory_mode`＝generate?Enabled:Disabled）、:1119-1126（**恢复**参数同源，仅对尚无行的新插入生效，见项 3）；phase2.rs:307（合并代理配置写 false 防递归，非读取） | config.toml（TUI `UpdateMemorySettings` 保存，app_event.rs:1480-1484） | **仅决定会话的提炼来源资格**（新会话登记时的 memory_mode 初值）；不门控管线启动、不门控注入、不门控清理 |
| `use_memories` | extension.rs:46-54（注入：Feature&&use）；config/mod.rs:4319-4321（readable roots 同门）；phase2.rs:306（代理防递归写 false） | config.toml（同上） | **仅决定是否注入已有记忆**；不影响生成与清理 |
| 线程 `memory_mode` | memories.rs:243（stage1 候选过滤 `='enabled'`）；memories.rs:566-628 `enabled_thread_metadata`（phase2 来源选择过滤）；polluted 标记 memories.rs:632-666 | 新建/恢复插入（threads.rs:1054 `creation_memory_mode.unwrap_or("enabled")`，仅 INSERT）；`set_thread_memory_mode`（threads.rs:742-748，per-thread UPDATE）；rollout SessionMeta 重放（threads.rs:1100-1102＋:1372-1375） | 逐会话的来源资格权威载体；disabled/polluted 行不入候选也不入合并选择 |

结论（对「generate=false 是否只影响新会话来源资格」）：**是**。管线启动门控只有 Feature；generate=false 时管线每输入轮照常跑，只是新建会话登记为 disabled、不再成为候选；既有 enabled 会话的提炼、phase2 合并、两类清理全部继续。本地采用规则：如实现 generate 开关，语义照此——门控 `register_thread` 写入的 memory_mode（本地列已在 migrations/0003 预留，store.rs:876-888 现恒 'enabled'）；**不**门控 `spawn_memory_pipeline`、不门控清理、不门控注入。拟验收项：generate=false 时新建会话登记行 memory_mode='disabled'；`stage1_startup_scan`（store.rs:946-989 已按 ='enabled' 过滤）不再纳入该会话；既有 enabled 会话在下一轮管线照常被领取；prune SQL（store.rs:1099-1129，无 memory_mode 过滤，与上游一致）照常删除过期行；注入路径（inject.rs:19-27）不受影响。

**项 2 清理边界**。固定上游事实：调用链 `turn_processor 触发 → start.rs 管线（无 generate 门）→ phase1::prune（30 天，start.rs:76-77）→ phase1 → phase2（同步内 14 天资源清理 phase2.rs:207）`。generate=false 时：已有候选提炼继续（候选过滤只看 memory_mode，既有 enabled 行合格）；phase2 合并继续（全局任务与水位独立于 generate，且选择过滤只排除 disabled 线程的产物）；两类清理继续。**明确区分**：清理是否执行是管线行为；数据年龄（last_usage/source_updated_at 对 now）无论清理是否执行都在自然增长——「停止执行清理」只是不删除，**不是冻结年龄**；若 Feature 整体关闭（管线停），行龄照涨，重新开启后按当时年龄一次性按 30 天规则处理（这是正常水位/窗口语义，非专门补处理）。本地采用规则：与上游一致——generate 门不含清理；仅 Feature 语义位（本地 `enabled`）关闭时管线与清理同停。拟验收项同项 1。

**项 3 登记与重新启用**。固定上游事实：①关闭 generate 后会话**照常保存与登记**，只是 memory_mode='disabled'（session.rs:1084-1088；threads.rs 插入路径）。②恢复（resume）：`apply_rollout_items` 对**既有行**用普通 upsert（不动 memory_mode，threads.rs:1094；测试 :1622「keeps_creation_memory_mode_for_existing_rows」），随后按 rollout 的 SessionMeta 记载重放 `set_thread_memory_mode`（:1100-1102、:1372-1375）——即**恢复按会话文件里持久化的模式，不按当前全局配置**；仅当库中尚无该线程行时才用当前配置作插入初值（ResumeThreadParams，session.rs:1119-1126→threads.rs:1054）。（细节保留：`LiveThread::resume` 是否在重放前改写 SessionMeta 未逐行核实——主链「既有行不被当前配置覆盖」已由 upsert 语义＋重放来源证实。）③切换**当前会话**设置：per-thread 显式 API `set_thread_memory_mode`（session/mod.rs:1510-1515→handlers.rs:257-288：persist＋`update_memory_mode`＋flush，同时落 rollout 与库；注释明言「只影响该线程未来的生成资格」）；app-server 暴露为 v2 线程设置（tests/suite/v2/thread_memory_mode_set.rs）。TUI 设置面板保存的是**全局 config**（app_event.rs:1480-1484「persist them to config.toml」），不是逐线程切换。④重新启用：**无批量恢复**——config 改回只影响此后新建/新插入线程；旧 disabled 会话须逐线程显式改（或其 rollout 本就记载 enabled）；**无代码证据支持自动补处理**，不得承诺。重新合格的会话按候选窗（10 天）与水位正常参与，不是回溯补跑。本地采用规则：如实现——登记继续、写 disabled；本地无 resume-重放概念（threads 行持续存在），等价语义＝「既有行不被后续登记改写」；如提供切换面，等价 per-thread API（改行＋不承诺补处理）。拟验收项：generate=false 期间登记行为 disabled 且 updated_at_ms 正常推进；重新启用后旧 disabled 行保持 disabled（无批量改写）；显式切换后该行恢复 enabled。

**项 4 触发调用点**。已在 E5 完整登记：turn_processor.rs:706-715，三条件，每带输入成功启动轮一次；此前「每会话启动一次」标记为 README 过期说法并撤销；本地 D2 同构确认、不改。

### 9.3 上游能力边界登记（明确声明）

固定提交的 Codex **没有**「保留来源资格但暂停全部后台处理」的开关组合：`Feature::MemoryTool` 关＝生成＋注入＋读取根全停；`generate_memories` 关＝仅新会话来源资格；`use_memories` 关＝仅注入。不存在「管线停但候选仍合格」的形态。若本地未来需要该能力，属**新增**，不得称为上游同构；本轮不提议新增。

### 9.4 与既有建议的隔离

反馈建库修复、`/memory-status` 失败展示、注释勘误三项保持独立（§8.2-1/2/6），不并入开关语义；D2/D3/D4/D12-A/G8 已批准适配维持原判。§8.2-4 的门控表与「当前做不到只用不生成」的结论仍成立（本地现状），其「建议控制范围」中被 E6 证伪的清理/管线部分以 §9.2 项 1 为准。

### 9.5 最终推荐（源码可定细节不再交用户选择）

若实现 generate 开关，采用规则即 §9.2 项 1-3（登记写 disabled；管线/清理/合并/注入全部不动；无批量重启用、无补处理承诺）。此前 §8.5 交用户的两点已由源码确定：清理**不**随生成暂停；登记**继续并标记 disabled**。仍留给用户的只剩实施优先级与时机（是否本轮做、与反馈建库/失败展示/注释勘误如何排序），以及「是否需要 per-thread 切换命令面」这一上游有而本地未提议的能力取舍——语义本身无需再议。

---

*轮次与产物：第一轮＝§1–§7 对照；复核轮＝§8（探针在 /tmp/013-probe*、/tmp/013-git-a/b）；开关收口轮＝§9（补取 turn_processor.rs/threads.rs/handlers.rs/app_event.rs，仅离线读取）。三轮均：仅本报告一个文件变更，未暂存、未提交、未合并、未推送、未删除分支；未改产品代码/测试/配置；V1 已交付状态与已交付规格、历史实施记录、manifest、封批账本未动；未调用真实模型/未探测 /models/未用历史封存额度；未访问真实用户数据库/会话/记忆目录、未读真实凭据；AGENTS.md 既有并发修改原样保留。§9 为源码核定与采用规则登记，暂不实施；停在讨论阶段。*
