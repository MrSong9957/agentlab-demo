# 008 TUI 渲染层移植——仓库调查证据（2026-10-14）

上游母本：openai/codex 固定提交 `97771b1d78cf6366bc3946fe9e4bbca4ebee31d1`（fetch 自 github，浅取，`git rev-parse` 复核一致）。对照目录 `/tmp/codex-upstream`（只读，未构建）。

## 1. 本地仓库状态（调查时点）

- 根目录 `git rev-parse --show-toplevel` = `/workspace`；分支 `main` @ `b0e8d48`；无远端（纯本地仓库）。
- 工作区差异：`M config/auto_catalog.json`（仅 `/refresh` 产生的 `updated_at` 时间戳刷新，运行时产物）；`?? db/`（wiki.db 索引，运行时产物）。两者非人工代码改动，本轮保持原样未动。
- 基线命令（容器内 `/workspace`）：`cargo check` 通过（12.26s）；`cargo test --workspace --no-fail-fast` 退出码 0，明细：
  92 单元 + 35 apply_patch + 11 e2e_acceptance + 16 exec_command + 51 llm_flow + 6 prompt_assembly + 32 session + 24 session_cli + 1 session_perf + 14 tool_loop + 0 doc-tests = **282 通过 / 0 失败 / 0 忽略**。

## 2. 现有"数据 → 终端输出"调用链（实测画定）

```
rustyline readline("你> ")                          src/main.rs REPL 循环
  → handle_input                                    src/main.rs
  → cmd_chat → catalog::build_final/route + origin 授权门
  → turn::run_tool_turn(history, input, sender, notify, cancel, on_record)   src/turn.rs
      sender  = api::chat_with_body（ureq 同步 POST，非流式，整段 JSON 响应）  src/api.rs:282
      notify  = |line| say(line)     ← 唯一工具活动显示挂点  src/turn.rs:227
      on_record = session::Session 自动追加落盘（007）
  → 最终回复 say(&format!("Agent：{reply}"))          src/main.rs
  → say = println!(display::sanitize_for_terminal(text))   显示边界，剥离全部 ECMA-48 序列  src/display.rs
```

- 事件模型：`prompt::HistoryItem` = Text(HistoryMessage{role,content}) / AssistantToolCalls / ToolResult（serde tagged，007 序列化表征）。
- 无 tokio、无 ratatui；依赖：libc、rustyline(buffer-redux)、serde/serde_json、sha2、ureq、url。

## 3. 上游结构实测（固定提交）

任务点名模块规模（`codex-rs/tui/src/`）：

| 路径 | 规模 |
|---|---|
| custom_terminal.rs（+cursor.rs、tests） | 1324 行 + 73 行 |
| tui.rs + tui/（31 文件） | 1583 + 6116 行 |
| insert_history.rs | 1184 行 |
| render/（9 文件） | 2700 行 |
| markdown_render.rs | 3243 行 |
| markdown_stream.rs | 903 行 |
| streaming/（16 文件） | 5970 行 |
| history_cell/（38 文件） | 12523 行 |
| bottom_pane/（156 文件） | 55608 行 |

内部依赖闭包（脚本计算，`use crate::x` 传递闭包，排除 `*_tests.rs`/tests 目录/snapshots）：

- 种子＝上述点名模块（含 bottom_pane）：**119 模块 / 256,921 行**。
- 种子去掉 bottom_pane：**45 模块 / 137 文件 / 53,662 行**。
- 污染路径（谁把 app 层拉进闭包）：`bottom_pane → app`（直连）；`bottom_pane → app_event → app_server_session / chatwidget → worktree_browser`；`bottom_pane → app → file_search、transcript_view`；`bottom_pane → multi_agents`。渲染核心模块（custom_terminal、tui、insert_history、render、markdown*、streaming、history_cell）不经过 bottom_pane 时到不了 app 层。
- `HistoryCell` 是 `pub(crate) trait`（mod.rs），具体 cell 各自成文件——按事件源裁剪是自然操作。

## 4. 依赖与供应链核实

- 上游 `codex-rs/Cargo.toml` `[patch.crates-io]`：crossterm → `openai-oss-forks/crossterm` rev `ed1cdab335221515706178d68495bba2aed1924f`；tokio-tungstenite/tungstenite 亦为 fork（与渲染无关）。ratatui patch 行已注释（crates.io 版本）。
- fork 已实际 fetch：版本号 0.29.0，71 个 .rs；顶端提交 `[crossterm] Preserve Escape from Windows Terminal input mappings (#10)`、`Discard secondary device attribute replies`——即 fork 动机是 Windows 输入映射修复与设备属性回复处理，**行为等价必须复刻该 patch**。
- ratatui：workspace `0.30.2`（features: crossterm/layout-cache/underline-color）＋ tui 侧再加 scrolling-regions/unstable-backend-writer/unstable-rendered-line-info/unstable-widget-ref。0.30 是 facade crate：lockfile 实证解析为 registry `ratatui 0.30.2` + `ratatui-crossterm 0.1.2`（sparse index 单次抓取的特征表不完整，以 Cargo.lock 为准）。crates.io 可用性：ratatui 0.30.2、crossterm 0.29.0 均在 index 中。
- syntect 5（features parsing/plist-load/**regex-onig**，含 C 库 onig）+ two-face 0.5：语法高亮（render/highlight*）。
- 其余渲染闭包直接依赖：pulldown-cmark 0.10、textwrap 0.16.2、unicode-width 0.2、unicode-segmentation 1.12、tokio（tui/ 事件流与 composer 计时）、tokio-stream、itertools、chrono、base64、rand、url、serde、derive_more、regex-lite；dev：insta 1.46.3、vt100 0.16.2、pretty_assertions、serial_test。
- codex 外部类型 crate（闭包内 use 位点计数）：codex_protocol 30、codex_terminal_detection 22、codex_app_server_protocol 21、codex_config 18、codex_ansi_escape 5、codex_utils_absolute_path 6、codex_utils_path_uri 3，另有 codex_mcp/rmcp/codex_otel/codex_mermaid/image 各 1–2（集中在 history_cell 的 mcp/computer_activity 等文件，裁剪即消除）。tui.rs 对 codex_config 仅 Notification{Condition,Method} 两类型。
- 小叶 crate 可整体 vendor：ansi-escape 142 行（+76 行测试；依赖 ansi-to-tui）、terminal-detection 416 行（+864 行测试）。
- 许可：Apache-2.0 + NOTICE（仓库根）。

## 5. 上游测试设施（可复用）

- `tests/all.rs` 聚合 `tests/suite/`（34 千行，含 vt100_history、vt100_live_commit、resize_reflow、clipboard 等）。
- `src/test_backend.rs` 的 **VT100Backend**（vt100 crate 模拟屏）直接驱动 `codex_tui::Terminal`/`RowBuilder` 做屏幕级断言；`tests/test_backend.rs` 以 `#[path]` 暴露。
- insta 快照目录：`src/{render,streaming,history_cell,bottom_pane,tui}/snapshots/*.snap`（如 streaming controller 数学/表格、markdown 增量渲染、dynamic 预览截断等），黄金主文件可直接移植。

## 6. 本轮结论要点

- 渲染核心（45 模块 5.4 万行）可高比例原样 vendor；bottom_pane 必须裁剪（否则等于移植整个 tui crate + app 层）；chatwidget（33,820 行，含 realtime/pets/plugins/usage/review 与 app/app-server 耦合）不移植，驱动层新写最小实现。
- 重大待决：交互模式从 rustyline 行式 REPL 整体替换为 ratatui+tokio 全屏 TUI，与 001 已验收交互行为冲突，须用户决策后方可建实现分支。
- 详细复用映射、裁剪清单、验收项见 `docs/specs/008-tui-rendering-port.md`。
