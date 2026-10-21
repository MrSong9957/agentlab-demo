# 规格：项目目录整理（纯路径迁移）

> 后续整理（2026-10-19）：本规格保留 2026-10-14 的迁移事实。原 `docs/records/` 四个附件目录现归类为测试／安全核心证据及历史日志摘要，完整原件仍在 `9e32a63`；当前路径与旧路径映射见[统一证据索引](../records/evidence-index.md)。

> 状态：**待批准（V1–V8 已实测通过，2026-10-14 实现轮回填 §5）**　分支 `feat/008-codex-tui-port`（基线 main @ `b0e8d48`），未提交；不提交、不合并、不删分支、不推送。
> 前置事实：008 候选（TUI 渲染层移植）全部改动在工作区未提交，Linux 终验 2670/0/3、e2e 21/21、A7 8/8 已实测，最终独立审查已闭环；**Windows A6 暂停且未闭合**——本规格不改变 008 的"实现中"状态，不宣称整体候选完成验收。
> 方案来源：用户已确认的目录整理方案（2026-10-14 会话指令）；调查证据＝同会话目录调查（含 `tui/`/`shims/`/`vendor/` 边界、`logs/` 重复情况、根目录归属核对）。
> 本规格为**纯路径迁移**：不新增功能、不改变任何运行行为；所有"待验证"项见 §5。

## 1. 目标 & I/O（含迁移映射）

- **目标**：把根包与 008 新增目录整理为 `src/` 分层布局、叙事证据归入 `docs/records/`，同时保持：从仓库根运行主程序与全量验证的方式不变、全部包边界与内部布局不变、全部测试与黄金快照原样被发现。
- **输入**：当前工作区（008 未提交候选＝已跟踪修改＋未跟踪 `tui/`、`shims/`、`vendor/`、`src/tui_host.rs`、`docs/specs/008-*`、`logs/2026-10-14-008-*`；未跟踪源码属候选，**不得当作临时文件**）。
- **输出**：迁移后工作区（仍全部未提交）＋同步后的路径引用与文档＋A6 补测包 v2（§5-V7）。

**迁移映射总表**（旧路径 → 新路径；"根包"指 `terminal-chat`）：

| 旧路径 | 新路径 | 说明 |
|---|---|---|
| `src/`（11 个 .rs） | `src/app/src/` | 根包源码整体平移；先建 `src/app/` 再移其余，避免新建子目录误入 app |
| 根 `Cargo.toml` 的 `[package]`/`[dependencies]` | `src/app/Cargo.toml` | 包名 `terminal-chat`、lib/bin 名（默认 `terminal_chat`/`terminal-chat`）不变；`tchat-tui` 依赖改 `path = "../tui"` |
| 根 `Cargo.toml` | 工作区入口（重写） | `members = ["src/app", "src/tui", "src/shims/*", "src/vendor/*"]`；`default-members = ["src/app"]`；`resolver = "2"`；`[patch.crates-io]` crossterm fork rev 原样留根 |
| `tests/` | `src/app/tests/` | 集成测试整体平移 |
| `assets/` | `src/app/assets/` | `src/prompt.rs` 的 `include_str!("../assets/codex/prompt.md")` 相对关系保持 |
| `tui/` | `src/tui/` | 与 `shims/`/`vendor/` 同批平移 → tui 对 `../shims/*`、`../vendor/*` 的相对依赖**零改动** |
| `shims/`（21 crate） | `src/shims/` | 同上；shim 间 `../<crate>` 互引保持 |
| `vendor/`（4 crate） | `src/vendor/` | 同上 |
| `logs/` | `docs/records/` | 全部叙事日志与证据（含 `security-20261005/`） |
| `Cargo.lock` | 原位（根） | 内容不含路径，预期零变化（以 git diff 为证） |
| `config/`、`db/`、`.env`、`.devcontainer/`、`.git*`、`README.md`、`AGENTS.md`、`docs/specs/`、`docs/security/`、`docs/testing/` | 原位 | 位置与 Git 跟踪策略**本次一律不动** |

**范围外（明确不做）**：孤儿快照清理（`tui/src/snapshots/` 中疑似对应已裁模块的约 90+ 个 .snap 留待 008 收尾轮裁决）、`.pytest_cache/` 等缓存清理、包拆分或合并、任何功能/行为重构、`tui/src/` 下两个错误文本文件名占位文件的删除（仅从 A6 打包排除并登记，仓库内原样保留并随目录平移）。

## 2. 边界 & 风险（行为约束）

- [x] **C1 行为零变化**：业务逻辑、命令语义、终端交互（001/008 Ctrl+C 契约）、配置与数据读写位置（`config/`、`db/`、`.env`、用户私有目录）不变；`config/auto_catalog.json`、`db/`、`.env` 的位置与 Git 跟踪策略不动，其现有本地修改原样保护。
- [x] **C2 包边界不变**：`terminal-chat`（edition 2021）、`tchat-tui` 与 21 个 shim（edition 2024）、4 个 vendor crate 的包名/库名/成员与内部目录布局不变；依赖版本、crossterm fork 固定 rev、许可证文件与黄金快照（.snap）内容不变。
- [x] **C3 根入口不变**：仓库根 `cargo run`/`cargo build`/`cargo check`/`cargo test` 语义经 `default-members = ["src/app"]` 保持（裸 `cargo test`＝app 测试面；全量用 `--workspace`，与 AGENTS.md 第 7 节命令一致）。
- [ ] **D1 Cargo.lock 漂移**：lock 不含路径，预期不变；迁移后 `git diff Cargo.lock` 必须无新增变化，有则查明（成员重排/解析差异）再处置。
- [ ] **D2 快照寻址**：insta 快照按测试文件相对位置寻址；`tui/` 整体平移保持相对结构。验收：迁移前后 .snap 总数一致（全仓 724＝`src/tui` 717＋`src/vendor` 7）且 tui 测试全绿。
- [ ] **D3 嵌入资源与 manifest 相对引用**：`include_str!`/`include_bytes!`/`CARGO_MANIFEST_DIR`/fixture 相对路径逐点核对（已知 `src/app/src/prompt.rs → ../assets/codex/prompt.md` 保持；shims 的 `.ps1` fixture 随 crate 平移保持）。
- [ ] **D4 未提交重命名的可审性**：全程不提交；跟踪文件用 `git mv`（重命名入索引便于审阅），未跟踪文件用 `mv`；以迁移前清单＋sha256（§3）证明无丢失、无意外改写。
- [ ] **D5 历史证据路径**：只改**当前有效引用**（导航、模块地图、008 规格中的现状路径、handoff §11 执行指令、A6 打包脚本）；**历史记录（当时的命令、输出、日期叙事）保留原意**，以映射说明衔接，禁止无差别全局替换。
- [ ] **D6 隔离构建**：全局 `CARGO_TARGET_DIR=/workspace/target`；实验/包核验显式 `--target-dir`；最终全量验证才用默认目录（AGENTS.md 既有纪律）。
- [ ] **D7 A6 包独立性**：不得以旧包或旧验证结果代表迁移后候选；v2 包在新版本位置重建（含未跟踪源码、排除规则同旧包＋新增排除项登记），旧定版包原样保留。

## 3. 关键判断（为什么）

- **J1 tui/shims/vendor 同批平移**：三者互为 `../` 相对 path 依赖，同层移入 `src/` 后相对关系不变 → 22 个 Cargo.toml 中除根（重写）与 app（新建）外**零改动**，把迁移面压到最小。
- **J2 default-members 而非维持根包**：根包下沉后，从根运行主程序的方式（README/AGENTS.md 的 `cargo run`/`cargo test`）靠 `default-members = ["src/app"]` 等价保持；`--workspace` 全量语义不变。
- **J3 顺序**：先 `src/` 源码入 `src/app/src/`、再 `tests/`+`assets/` 入 `src/app/`、后 `tui/`/`shims/`/`vendor/` 入 `src/`——避免把新建的 `src/app/` 或后续目录误搬进 app（任务指令的显式约束）。
- **J4 完整性证明**：迁移前生成全量清单（git 跟踪 ∪ 未跟踪非忽略，排除 `.git/`、`target/`、`.pytest_cache/`）＋逐文件 sha256；迁移后按映射逐一复验——除**登记的引用修改文件**（§5-V6 列表）外全部一致。
- **J5 重构级验证**：纯路径迁移按"迁移前后检查都通过"验证：迁移前基线（check＋全量 test 计数）→ 迁移后同套命令＋named targets；计数与 008 终验（2670/0/3、e2e 21/21、A7 8/8）对照，不允许无说明的减少。
- **J6 A6 v2 只更新交接材料**：Windows A6 保持暂停；v2 包＝候选新布局快照＋probe（依赖路径改 `../src/tui`）＋脚本与说明适配；probe 在隔离 target-dir 对新候选 `cargo check` 通过即可信（接口一致），Node/Windows 执行面如实标待验证。

## 4. 坑记录

1. 全局 `CARGO_TARGET_DIR=/workspace/target`：副本/probe 核验必须显式 `--target-dir`（D6）。
2. `logs/` 内含中文与空格文件名（`security-20261005/v1-prefix-evidence/`）：目录级 `git mv` 整体移动，避免逐文件转义。
3. `.gitattributes`＝`* text=auto`：纯移动不改内容，无行尾重写风险（sha256 复验兜底）。
4. `logs/2026-10-14-008-handoff.md` 对 Read 工具误报"不支持的编码"（实为合法 UTF-8）：用 shell 读取/编辑。
5. `db/`、`config/auto_catalog.json`、`.env` 是受保护本地数据：不进 A6 v2 包（沿旧包排除规则），仓库内不动。
6. 快照计数口径（审查轮修正）：全仓 .snap 共 724 个（`src/tui` 717＋`src/vendor` 7）；`tui/src/snapshots/` 198 个中约 90+ 疑似对应已裁模块（008 收尾口径问题，范围外）；`tui/README.md` 登记的 519 为模块内目录口径——本次以 724 为对照总数，不修该登记。
6a. 008 遗留 insta 工作文件（登记，范围外）：4 个 `.pending-snap`（JSONL 追加日志，**每次测试运行都会追加**、哈希必然漂移）与 9 个 13:16/13:23 的 `.snap.new`——均属 008 时代产物，本次原样随目录平移，不做清理。
7. 迁移会改变 cargo 构建指纹：`/workspace/target` 增量失效属预期，非行为差异。

## 5. 预测 → 实测（验收清单）

已实测（2026-10-14 实现轮回填；证据＝`docs/records/2026-10-14-009-layout-migration.md`）：

- **V1 根入口 ✅**：仓库根 `cargo check --workspace` 通过；`cargo metadata` 成员 27＝app＋tui＋21 shim＋4 vendor，默认成员＝src/app（virtual root 解析正常）；根裸 `cargo check`/`cargo test --no-run`/bin 产出均验证。
- **V2 e2e ✅**：`cargo test -p terminal-chat --test e2e_acceptance`＝21/21（定向＋终验两轮）。
- **V3 A7 面 ✅**：`cargo test -p tchat-tui --test security_untrusted_text`＝8/8（定向＋终验两轮）。
- **V4 全量 ✅**：`cargo check --workspace && cargo test --workspace --no-fail-fast`＝**2670 通过 / 0 失败 / 3 忽略**（迁移前基线实测同值；39 个测试目标剥离耗时后逐目标序列一致）。
- **V5 快照 ✅**：全仓 `.snap` 总数迁移前后均＝724（`src/tui` 717＋`src/vendor` 7），sha256 与迁移前清单全量比对一致（独立审查子代理复验同结论）。
- **V6 完整性 ✅**：清单 1386 件复验：一致 1376＋登记例外（含审查轮补改的 `docs/testing/critical-functional-test-cases.md` 活指针）＋4 个 `.pending-snap`（测试运行固有追加，见坑 6a）；0 缺失；反向核对唯一新增文件＝`src/app/Cargo.toml`。
- **V7 A6 v2 包 ✅（Node/Windows 执行面待验证）**：staging 按新布局组装（1371 文件；排除 `target/`、`db/`、`.env`、`config/` 用户本地修改（manual.json 与 auto_catalog.json 均取 Git 基线版本）、`.git/`、`.pytest_cache/`、两个错误文本名占位文件、4 个 `.pending-snap`、9 个 `.snap.new`——后两类为已确认非构建临时产物，登记排除、仓库内保留）；`conpty_probe` 依赖改 `../src/tui` 且经隔离 `--target-dir`（`/tmp/009-a6-v2/probe-target`）`cargo check` 通过；MANIFEST/CANDIDATE-ID/包外 SHA-256（不自引用）；落位 `/vault/008-windows-a6-v2/`（宿主 `D:\AgentLab\shared\Obsidian\008-windows-a6-v2\`）复验一致；旧定版包 `/vault/008-windows-a6/` 原样保留；Node 脚本与 Windows 执行标**待验证**（环境无 Node/Windows）。
- **V8 审查 ✅**：独立 code-review（Standards/Spec 双轴子代理）完成：无硬违规；处置——补改 `docs/testing` 活指针、规格口径修正（快照 724、V6 措辞涵盖 pending-snap）；comprehensive-testing 按纯路径迁移裁定不新增 Rust 用例（拒绝低价值框架测试），以 include_str 资产定向变异证明检查敏感性（损坏→`cargo check` exit 101，恢复后绿且哈希一致）。
