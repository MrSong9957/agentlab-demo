# 最终候选未闭环项处理总报告（2026-11-12）

分支 `fix/agents-billing-deadlock`；起始 HEAD `f17ff48`（批 5 最终候选）。

## 处置清单

| # | 事项 | 结果 | 提交 |
|---|---|---|---|
| 1 | CI 触发方式查明 | 见 ci-windows.md：push 仅 main，分支触发＝workflow_dispatch（本轮采用）或 PR | — |
| 2 | 推分支＋跑 CI 盯 Windows 腿 | run 1（f17ff48）Windows 腿编译期 5 错→归因修复 `14a32ea`→run 2（43130590026）验证 | 14a32ea |
| 3a | A7 写线程端到端行为锚 | 红→绿实证（裸 JSON 突变→坏帧红；还原→绿），testing_gaps 登记项闭环 | 404198b |
| 3b | manager 失效行缩进 | 7 处对齐作用域缩进；`git diff -w` 为空＝仅空白；`cargo check` 0 错误 | b4ce034 |
| 4 | C18 阈值 ×20 口径确认 | **无需动作**（见下） | — |
| 4 | TCHAT_SANDBOX 依赖确认 | **无需动作**（见下） | — |

## 步骤 4：两项确认（各一句）

- **C18 阈值 ×20 口径：无需动作。**2026-11-11 重冻结（提交 `2f9a093`）已按
  当前环境实测 p95×20 更新断言阈值并在 docs/logs/2026-11-11-c18-refreeze/
  留存新旧对照，CI 侧由 `JS34_PERF_SAMPLE` 等采样开关按既有机制跳过冻结
  断言（ci.yml:45-50），口径已闭环。
- **TCHAT_SANDBOX 依赖：无需动作。**既有约定（AGENTS.md/038 §3）：CI 不
  预置该变量、容器部署注入 =1，测试命令按 `env -u TCHAT_SANDBOX` 消除
  部署注入态（038 §7 已登记双分支口径），本批全部测试命令照此执行。

## 提交清单（本轮新增，按序）

1. `b4ce034` style(mcp): manager 失效行缩进对齐作用域（diff 仅空白）
2. `14a32ea` fix(windows): CI 首验暴露的批 3 cfg(windows) 测试面编译错 5 处
3. `404198b` test(a7): 写线程端到端行为锚（红→绿实证）
4. （待 CI 结论后）证据目录提交

推送：f17ff48 → origin（run 1 触发）；14a32ea → origin（run 2 触发）。
均按授权「仅触发 CI」目的；无合并、无删分支、rules/ 两文件与 AGENTS.md
外部改动全程未入任何提交（git status 实证）。

## 阻断条件核对

- CI 触发方式：已查明（workflow_dispatch）——未阻断。
- 推送：两次均成功——未阻断。
- 「连续两次红在同一非本分支原因」：run 1 红为本分支问题（批 3 测试面
  编译错），已修；run 2 结论见 ci-windows.md 追加段——未触发本阻断。

## 剩余未闭环项

（CI 结论后更新）
