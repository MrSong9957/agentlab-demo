# 009 目录整理实现记录（2026-10-14）

规格：`docs/specs/009-project-layout.md`（V1–V8 已回填实测）。分支 `feat/008-codex-tui-port` @ `b0e8d48`，全部改动未提交。**008 保持"实现中"（A6 未闭合）。**

## 1. 执行顺序与关键决策落点

1. 规格 009（五节）→ 迁移前清单（`2026-10-14-009-pre-migration-inventory.txt`，1386 件含 sha256；008 未跟踪源码全含）→ 迁移前基线（check＋全量 2670/0/3）。
2. 迁移（J3 顺序）：`src/*.rs`→`src/app/src/`（跟踪件 git mv、`tui_host.rs` mv）→ `tests`/`assets`→`src/app/` → `tui`/`shims`/`vendor`→`src/` → `git mv logs docs/records`（目录级，中文/空格文件名随目录整体移动）。
3. Cargo：根 `Cargo.toml` 重写为 workspace 入口（members＋default-members=src/app＋resolver 2＋crossterm fork patch 留根）；`src/app/Cargo.toml` 新建（包名/依赖原样，`tchat-tui` path=`../tui`）。**`Cargo.lock` 迁移前后 sha256 一致（零新增变化）**；tui/shims/vendor 的 22 个 Cargo.toml 因同层平移**零改动**。
4. 引用扫描与修改：源码零改动（无 `CARGO_MANIFEST_DIR`；`include_str!` 全部 crate 内相对路径随整体平移保持）。文档改动＝登记的引用修改文件（见 §4 V6 列表）。
5. 验证 → comprehensive-testing → 独立 code-review（双轴）→ 处置 → verify:full → A6 v2。

## 2. 验证结果（命令 → 结果）

| 命令（容器内 /workspace） | 迁移前基线 | 迁移后 |
|---|---|---|
| `cargo check --workspace` | 通过 | 通过（32.4s 重建＋终验 12.7s） |
| `cargo test --workspace --no-fail-fast` | **2670/0/3**（39 目标） | **2670/0/3**（39 目标；剥离耗时逐目标序列一致） |
| `cargo test -p terminal-chat --test e2e_acceptance` | —（008 终验 21/21） | **21/21**（定向＋终验两轮） |
| `cargo test -p tchat-tui --test security_untrusted_text` | —（008 终验 8/8） | **8/8**（A7 面，两轮） |
| `cargo metadata --no-deps` | — | 成员 27、默认成员 src/app |
| 快照 `.snap` 总数 | 724（tui 717＋vendor 7） | 724，sha256 全量一致 |
| 清单完整性 | 1386 件生成 | 0 缺失；一致 1376＋例外（见 V6）＋4 个 `.pending-snap`；反向唯一新增＝`src/app/Cargo.toml` |

日志：迁移前/后全量测试原始输出存 `/tmp/009-baseline-test.log`、`/tmp/009-final-test.log`（会话产物；计数结论已录于本表）。

## 3. comprehensive-testing 裁定与变异证据

- 裁定：纯路径迁移无新运行时行为，**不新增 Rust 回归用例**（拒绝绑定目录布局的低价值框架测试；AGENTS.md「不为覆盖率增加低价值测试」）。保护面＝计数对照＋清单校验和＋既有 2670 套件。
- 敏感性证明（A10 同构）：临时移走 `src/app/assets/codex/prompt.md` → `cargo check -p terminal-chat` **exit 101**（`include_str!` 路径损坏即被编译期捕获）；恢复后通过且 sha256 与清单一致。无临时变异残留。
- A7 安全面复跑 ✅（见上表）。

## 4. 独立 code-review（Standards/Spec 双轴子代理）与处置

- **Standards**：无硬违规。发现①`docs/testing/critical-functional-test-cases.md:281` 活指针 `logs/2026-10-10-007-implementation.md` 悬空 → **已补改**为 `docs/records/…`；②V 状态未回填＋全量未跑 → 本轮已闭合；映射叙述四处重复属可接受交叉引用（显式指回规格）。`db/wiki.db` 未 ignore 属规格明示"本次不动"，非 009 责任。
- **Spec**：映射全落实（独立复核 Cargo.lock sha、724 快照逐一比对、占位文件/.pytest_cache 原样）。发现①handoff §11 未更新＋ V6 例外登记与事实不符 → **已更新**（§3/§11 现行引用＋§12 附录）；②快照总数口径 717 应为 724（漏算 vendor 7） → **规格已修正**（D2/坑6/V5）；③"6 例外"措辞未覆盖 4 个 `.pending-snap` → **V6/坑 6a 已补**。无 scope creep。

## 5. 008 遗留观察项（登记不处理，范围外）

- `src/tui/src` 两个错误文本文件名占位文件（`custom_terminal.rs:56:1: error… `／`wrapping.rs:1400:1: error… `，内容 `// 008-VERIFY: 占位`）——随目录平移保留，仅从 A6 v2 包排除并登记。
- 4 个 `.pending-snap`（JSONL 追加日志，**每次测试运行追加**，含迁移前基线运行；其中记录的运行时快照值与已接受值不一致但测试全绿——成因未深究，属 008 测试卫生问题）与 9 个 13:16/13:23 的 `.snap.new`——同上保留。
- `src/tui/src/snapshots/` 198 个中约 90+ 疑似对应已裁模块（008 收尾口径）。

## 6. A6 补测包 v2

- 位置：`/vault/008-windows-a6-v2/`（宿主 `D:\AgentLab\shared\Obsidian\008-windows-a6-v2\`）；staging `/tmp/009-a6-v2/pkg/008-windows-a6-v2/`（1371 文件）。
- `008-windows-a6-v2.tgz` SHA-256＝`1e53d6274e4c2d7c8fcaae3a6b0fb3cb22232cee7d34af606d4dcb4ab079ee99`（包外 `008-a6-v2-package.sha256`，不自引用；落位后复验一致）。
- 内容：新布局候选（src/app＋src/tui|shims|vendor＋docs＋根 Cargo/lock；config 取 Git 基线；排除 target/db/.env/.git/.pytest_cache/占位文件/insta 工作文件并登记）＋`conpty_probe`（依赖 `../src/tui`，经 `/tmp/009-a6-v2/probe-target` 隔离 `cargo check` 通过；probe Cargo.lock 与 v1 相同）＋`scripts/`（conpty_run.mjs 八场景，包根相对寻址无需改）＋`README-A6.md`/`WINDOWS-RUN.md`（v2 适配版）。
- 旧 v1 包 `/vault/008-windows-a6/`（sha 5363e32d…）原样保留，仅作历史定版；**A6 结论只采信 v2 在 Windows ConPTY 上的运行**。Node 脚本未本地执行（无 Node）——待验证。

## 7. 剩余风险

- Windows A6 未执行（既有阻断，008 状态不变）；v2 包的 Node 脚本未经 `node --check`。
- `/tmp/009-a6-v2`、`/tmp/009-*.log` 为会话产物，容器重建即失（vault 交付物不受影响）。
- 008 遗留观察项（§5）留待 008 收尾轮裁决；`db/wiki.db` 跟踪策略、`.env` 入库问题维持现状待讨论。
