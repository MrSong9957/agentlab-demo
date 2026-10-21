# Agent 开发核心宗旨符合性核查（2026-10-08）

对照 `docs/Agent开发核心宗旨.md` 两条宗旨逐项核查，结论：结构性符合，三处差距。

## 宗旨一：前后端分离 —— 符合

- 核心自成一体：`src/lib.rs`（api/catalog/display）全部数据进出风格，返回 `Result<_, String>`；无 stdout 打印、无 stdin 读取（唯一 `eprintln!` 在 `catalog.rs` `#[test]` 内）。
- 交互确认经回调注入（`ensure_origin_authorized(..., interactive, &mut prompt)`），`interactive` 由前端传入；`rustyline` 仅出现在 `main.rs`，核心零终端依赖。
- `main.rs` 是薄前端：REPL 循环、开场动画、命令分发、`Agent：` 回显格式化均在前端侧。

差距：
- 目前仅 CLI 一种前端，"所有前端共用核心"未经第二个前端（Web/桌面）实际验证。
- 核心为同步阻塞（ureq blocking），CLI 够用；将来 Web 异步栈需 spawn_blocking 包装或改造，属已知适配成本。

## 宗旨二：Rust + 复用成熟框架 —— 基本符合

- 纯 Rust（edition 2021），`cargo check` 通过（本次实测）。
- HTTP/JSON/行编辑/SHA-256 全部用成熟 crate：ureq、serde/serde_json、rustyline、sha2、url。

自研两处（均有依据）：
- `catalog::load_dotenv_from`（28 行 .env 解析，未用 dotenvy）：2026-10-04 修 bug 的最小实现，带单元测试，日志有记录；严格对标可换 dotenvy。
- `display::sanitize_for_terminal`：安全关键路径（003 修复）需白名单语义（未终止序列、C1 引导），自研带完整测试，合理。

差距：
- 宗旨示例"参考/复用 Codex 开源仓库"完全未做：全仓库无任何 codex 参考、对照或复用记录；`api.rs` 协议适配从零编写。示例非硬性要求，但如后续扩展（重试、流式、多协议），应先对照 codex 等成熟实现再决定是否自研。
