# 015 用户轮上下文注入——实施记录

> 日期：2026-10-20　分支：`feat/015-turn-context-injection`（基线 `2d7be44`，合并目标 `main`）
> 规格：`docs/specs/015-turn-context-injection.md`（含实施轮同步的五条收口条款与证据收口轮追加）
> 流程：规格同步 → RED → GREEN → 综合测试（变异）→ 安全门禁 → code-review（双轴＋终态补充）→ L3 全量 ×2 → 证据收口；全程未提交。

## 1. 逐文件清单与分类统计（以 Git 实际状态核定）

**分类统计：生产 7＋测试 7＋文档 4＝18 份**（暂存区为空；全部未提交）。

| # | 文件 | 类别 | 用途 |
|---|---|---|---|
| 1 | `src/app/src/context.rs`（新） | 生产 | 来源加载（全局＋最近 `.git` 根→cwd 链）、有界读取与 UTF-8 边界、32768 字节唯一截断、异常来源跳过＋提示、字节级消息构造、`context_body` 完整结构识别（剥至多一条声明）、去重基线、`ContextSources::from_env()` |
| 2 | `src/app/src/lib.rs` | 生产 | 注册 `context` 模块（一行） |
| 3 | `src/app/src/turn.rs` | 生产 | 新入口 `run_user_turn_with_context`：空白预检→User/UserWithMemory 注入→复用既有循环；MemoryMerge 不注入；空白文案常量 |
| 4 | `src/app/src/prompt.rs` | 生产 | 适配 3 反转（母本句→015 新句）；`CONTEXT_REPLACEMENT_DECLARATION` 常量 |
| 5 | `src/app/src/main.rs` | 生产 | CLI 宿主改调新入口（`from_env()`） |
| 6 | `src/app/src/tui_host.rs` | 生产 | TUI 宿主改调新入口；`/resume` 投影跳过 ctx 项（仅展示层） |
| 7 | `src/app/src/memory/rollout_input.rs` | 生产 | 过滤最小适配：`context_body` 识别器加入剔除（既有两类过滤不变） |
| 8 | `src/app/tests/context_injection.rs`（新） | 测试 | c1–c8/c10 共 14 用例（独立临时目录＋固定时钟；预期文本测试侧独立重建；t09 含四态预算角形） |
| 9 | `src/app/tests/prompt_assembly.rs` | 测试 | s2 断言随 c8 更新 |
| 10 | `src/app/tests/llm_flow.rs` | 测试 | 两管道用例 cwd 落临时目录＋消息序断言更新 |
| 11 | `src/app/tests/session_cli.rs` | 测试 | 8 用例按新记录数/消息序更新（e1/e7 同 cwd 复跑；e22「＋1 条 ctx」结构对齐） |
| 12 | `src/app/tests/tool_loop.rs` | 测试 | 8 用例角色序/索引更新 |
| 13 | `src/app/tests/e2e_acceptance.rs` | 测试 | `pty_cli` 默认 cwd 落 `/tmp`；s1/s2a/n2b/n2b2 断言更新；s15 逐轮捕捉提示；`s81_m7_enabled_turn_injects_and_spawns_pipeline` 播种断言改有界等待（既有竞态，证据收口轮修复） |
| 14 | `src/app/tests/memory_phase1.rs` | 测试 | m3 扩带替换声明的合并块样本 |
| 15 | `docs/specs/015-turn-context-injection.md`（新） | 文档 | 规格＋收口条款＋实测回填＋证据收口轮追加 |
| 16 | `docs/specs/005-minimal-prompt-assembly.md` | 文档 | 适配 3 修订注记（已实施） |
| 17 | `docs/records/2026-10-20-015-implementation.md`（新，本文件） | 文档 | 实施记录与证据 |
| 18 | `docs/security/reports/2026-10-20-015-context-injection-security.md`（新） | 文档 | 轻量安全报告 |

**保护排除（他人工作，未触碰、不入提交）**：`AGENTS.md`、`project-lessons.md`、`docs/records/2026-10-20-013-memory-codex-comparison.md`、`docs/records/2026-10-02-install-mattpocock-skills.md`。

## 2. 关键证据

- **RED（六处）**：context_injection 11/14 失败；prompt_assembly s2、memory_phase1 m3、rollout_input/tui_host 新用例失败（t08/t10/t11 为与桩行为兼容的回归守卫，实现后仍绿）。
- **变异**：A 不剥声明→t06/t07/t08/t14＋rollout 过滤击杀；B 无边界回退→t09；C 仅前缀识别→t07/t14＋rollout 过滤；D 耗尽只查首路径→t09(d)（证据收口轮新增）。全部恢复后绿。
- **有界读取运行探针**（临时测试，证据跑毕即删）：200 MiB 全 `Z` 来源→content 恰 32768、截断提示存在、VmHWM 3628 KiB（整读将 ≥204800 KiB）。
- **安全门禁**：轻量报告见文件 18；200 MiB 探针、符号链接/非普通/非法 UTF-8/读取失败跳过、提示不含正文、XML 转义、记忆输入剔除均实测通过。

## 3. 独立审查覆盖、发现与处理

- **首轮双轴（实施轮）**：两代理均被显式指派「未跟踪新文件用 Read 读全文」，输出引用 `context.rs:100-105`、`context_injection.rs:22-46`、`e2e_acceptance.rs:255`、`rollout_input.rs:214` 等具体行号证明确读。Standards 无硬违规（3 判断题）；Spec 核对 §1 全条款。处理 5 项：`from_env()` 收拢重复、UTF-8 重建不可达分支扁平化、预算耗尽改不打开文件仅元数据判定、空白文案常量化、误建 `devnull` 清理。
- **终态补充审查（证据收口轮）**：首轮审查后生产代码有上述 5 处修复，故对**最终状态**补做独立审查（已跟踪差异＋两个未跟踪新文件全文＋规格）。结论：5 处增量无缺陷；发现 1 项低危——**预算耗尽分支只检查首个后续路径**，首路径缺失/异常会掩蔽更后链上目录的普通来源存在性，与规格 §1「存在后续普通文件来源时记一行提示」不符。处理：修复为扫描全部剩余路径的元数据存在性（仍不打开任何文件）＋t09 补钉角形 (d)＋变异 D 证伪。
- **终验轮实测失败 1 例**：第一轮终验 standalone e2e `s81_m7_enabled_turn_injects_and_spawns_pipeline` 播种断言失败。根因实证：管线建库（`MemoryStore::open`）与播种（`runner.rs:262` 独立临界区）存在先后间隙，测试「见库即断言」无等待窗——**既有测试结构竞态**（单跑通过；本任务 diff 不涉 runner/播种）；范围内修复为与建库同款 10 秒有界等待（断言语义不放宽）。实施轮 workspace 那例未具名失败仍为**未确认失败**（证据不足，不作归因）。

## 4. 最终验证证据（证据收口轮第二轮，覆盖最终候选）

| 命令（cwd=/workspace） | 退出码 | 结果 |
|---|---|---|
| `cargo check` | 0 | terminal-chat 0 告警 |
| `cargo test --workspace --no-fail-fast` | 0 | 全套件绿（内含 e2e 106/106） |
| `cargo test --test e2e_acceptance` | 0 | 106/106 |

合计 **3141 通过 / 0 失败 / 3 忽略**；日志（含完整命令、cwd、两轮全部输出与退出码）：`docs/records/2026-10-20-015-final-verification.log`（原始留存 `/tmp/015-final-verification.log`）。**时间线**：最后一次生产代码修改＝预算耗尽分支扫描修复（先于第一轮终验）；第一轮终验发现 `s81_m7_enabled_turn_injects_and_spawns_pipeline` 既有竞态后**仅改测试**（播种断言改 10 秒有界等待，无生产变更）；第二轮终验（上表，覆盖最终候选）；此后仅改文档。

既有运行记录保留：实施前 e2e 106/106×2（`/tmp/015-e2e-baseline-r1/r2.txt`）；实施轮捕获轮 workspace 3035/0/3（`/tmp/015-l3-full-run.txt`）；e2e 单套件 106/106（多轮）。

## 5. 候选文件 SHA-256 指纹（终版）

终版指纹清单（候选 18＋证据件 3，含各文件完整 SHA-256）：`docs/records/2026-10-20-015-sha256-manifest.txt`。本文件自身的指纹只出现在该清单与最终返回中，不写入自身正文（自引不变式）。

完整可审阅差异（含 5 个未跟踪新文件全文）：`docs/records/2026-10-20-015-final-diff.patch`；终验完整日志（命令、cwd、退出码、两轮全部输出）：`docs/records/2026-10-20-015-final-verification.log`。以上三份均为**交付证据件，不属候选提交**。

## 6. 剩余风险与已知简化

- 跨天日期变化触发一次替换注入（规格 §3-3 已接受）；session_cli e1/e7 同 cwd 复跑存在 UTC 午夜切分秒级竞态（未观察到）。
- 预算耗尽提示以元数据判定：以空白开头的后续普通文件会多报一行（安全报告已记录）。
- AGENTS.md 规则正文随请求外发模型服务——与 Codex 同面，规格 §2 边界记录。
- 实施轮 workspace 单例未具名失败＝**未确认失败**（证据不足，不作归因；前后多轮全量全绿）。
