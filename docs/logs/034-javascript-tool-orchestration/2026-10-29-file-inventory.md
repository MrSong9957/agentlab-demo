# 034 交付文件清单（2026-10-29 收口三版，逐文件、互斥分类、Git 可复核）

生成方式：git status --short 逐文件展开目录，每文件唯一归类。复核命令：`git status --short`（26 M＋19 ?? 与本清单一一对应）。

## A. 任务相关·已跟踪修改（22）
| 分类 | 文件 |
|---|---|
| 依赖 | Cargo.lock |
| 规格/经验 | docs/records/2026-10-21-codex-tools-comparison.md |
| 规格/经验 | docs/specs/034-javascript-tool-orchestration.md |
| 规格/经验 | rules/project-lessons.md |
| 依赖 | src/app/Cargo.toml |
| 实现 | src/app/src/api/native.rs |
| 实现 | src/app/src/api/native/anthropic.rs |
| 实现 | src/app/src/api/native/gemini.rs |
| 实现 | src/app/src/catalog.rs |
| 实现 | src/app/src/lib.rs |
| 实现 | src/app/src/main.rs |
| 实现 | src/app/src/memory/rollout_input.rs |
| 实现 | src/app/src/prompt.rs |
| 实现 | src/app/src/session.rs |
| 实现 | src/app/src/tui_host.rs |
| 实现 | src/app/src/turn.rs |
| 测试 | src/app/tests/context_management.rs |
| 测试 | src/app/tests/mcp_http.rs |
| 测试 | src/app/tests/mcp_resources.rs |
| 测试 | src/app/tests/mcp_turn.rs |
| 测试 | src/app/tests/memory_phase2.rs |
| 测试 | src/app/tests/message_to_user.rs |

## B. 任务相关·新增（17 文件，11 顶层路径展开）
| 分类 | 文件 |
|---|---|
| 实现 | src/app/src/js_orchestration/args.rs |
| 实现 | src/app/src/js_orchestration/mod.rs |
| 实现 | src/app/src/js_orchestration/protocol.rs |
| 实现 | src/app/src/js_orchestration/runtime.rs |
| 实现 | src/app/src/js_orchestration/worker.rs |
| 测试 | src/app/tests/js_orchestration.rs |
| 测试 | src/app/tests/js_orchestration_perf.rs |
| 测试 | src/app/tests/js_orchestration_security.rs |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-26-doc-check-2.log |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-26-doc-check.log |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-26-spec-research.md |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-29-code-review.md |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-29-file-inventory.md |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-29-final-hashes.md |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-29-m0-prototype.md |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-29-perf-freeze.md |
| 证据 | docs/logs/034-javascript-tool-orchestration/2026-10-29-perf-samples.txt |
| 证据 | docs/logs/034-javascript-tool-orchestration/m0-prototype/proto-Cargo.lock |
| 证据 | docs/logs/034-javascript-tool-orchestration/m0-prototype/proto-Cargo.toml |
| 证据 | docs/logs/034-javascript-tool-orchestration/m0-prototype/proto-main.rs |
| 证据 | docs/security/reports/2026-10-29-034-js-orchestration-security.md |

## C. 范围外·用户文件（4 修改＋8 未跟踪项，未触碰）
修改：AGENTS.md、docs/records/2026-10-27-sandbox-spec-sync.md、docs/specs/029-file-patch.md、docs/specs/035-multi-agent.md
未跟踪：.claude/ .compound-engineering/ docs/logs/plan-preresearch/ docs/logs/project-rules/ docs/think-tank/ docs/大脑模型*.md docs/项目规则.md docs/项目规则v2.md rules/计划前调研.md

## D. 034 规格的交付关系
docs/specs/034-javascript-tool-orchestration.md：用户 2026-10-28 调研编辑为实施输入（原 M），本次叠加实施状态/验收表/口径更新——同文件两层编辑，交付为合并态。

## E. 算术
任务修改=22（实现 11＋测试 6＋依赖 2＋规格/经验 3——分类明细见 A 表，以 A 表行数为准）; 任务新增文件=21（源码/测试 8＋证据 9）; 范围外=12。总计 26 M＋19 ??（?? 含目录项，展开后文件数以 B 表为准）。
