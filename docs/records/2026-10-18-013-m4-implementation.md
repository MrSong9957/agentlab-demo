# 013-M4 全量 stage1 快照 Markdown 同步实施记录（2026-10-18）

任务分支 `feat/013-memory-m4`；基线 main @
`0b429aefa19772bd00fe251a51e6c74c4671b3f0`。规格：
`docs/specs/013-memory-system-v1.md` §6 M4。§1–§8 保留实施轮形成待批准候选的
历史证据，该轮不提交/合并/推送/删除分支；2026-10-18 用户批准后的交付见 §9。
M5–M9 未实施，全程不调用真实模型。

## 1. 准备与门禁方法

- 起始分支/HEAD 与报告一致，暂存区为空；已有未暂存 compose 和 013 修改、
  未跟踪 `db/` 全部保留。compose SHA-256：
  `b71aa84f47469f67cdee59af89d3eaa9a298b09273f67d62e793558b9cc4cf87`。
  不打开、不写入 `db/wiki.db`，不将其用于检查、测试或副本。
- 本地 Skill 目录与会话清单均无 tdd、comprehensive-testing、Security Testing
  Gate、code-review、verify:full；本轮按用户要求及 AGENTS.md 手动依次执行，
  不声称加载或运行缺失 Skill，不使用子代理。
- 基线实跑：`cargo check` exit 0；
  `cargo test --workspace --test memory_phase1 --test memory_candidates --test memory_store`
  = **19＋18＋21 / 0**；`cargo test -p terminal-chat --lib memory::`
  = **22 / 0**（100 项无关单测被名称过滤）。此前全量 2856/0 是 M3 交接证据，
  不当作本轮完整验证。
- Cargo 离线运行；新用例只用显式临时根、临时 SQLite 和合成会话。业务时钟固定；
  复用 `common::TempDir` 的 RAII 清理，目录名仅 pid＋计数器，不使用时钟/随机源。
  所有黄金主文件事先手写；无自动接受快照流程。

## 2. 来源、复用与适配

固定上游 `openai/codex @ ce0323f2f2d3b25e82806cd555237da3db4e1897`
（Apache-2.0，来源与许可已在移植文件头注明）：

- `codex-rs/memories/write/src/storage.rs`：正文格式、canonical 文件名、
  keep 集合、直属小写 `.md` 清理、空占位及错误语义。
- `storage_tests.rs`：UUID 时间与短 hash、slug、旧命名清理和正文来源锚。
- `lib.rs::ensure_layout`、`workspace.rs::remove_memory_symlinks`：
  根链接拒绝、递归移除链接本身、不跟随目标。
- `phase2.rs::sync_phase2_workspace_inputs`：先同步摘要，再重建 raw。

本地变化：

- `memory/storage_files.rs`：`Stage1FileInput::from_row` 显式补齐元数据，
  `sync_stage1_snapshot` 处理完整已排序快照；不加入 M5 选择上限或保留策略。
  异步 I/O 改同步，tracing 告警改调用方诊断出口，诊断经既有显示净化函数；
  写入复用 `catalog::write_private_file`（0700/0600，既存权限亦收紧）。
  这是本地私有文件策略适配，仍直接覆盖、不提供单文件或跨文件原子性。
- `MemoryStore::list_stage1_outputs`：单 SELECT、连接锁内读取、thread_id 升序；
  与单行读取共用原行映射，不写任务/水位/selected、不迁移 Schema。
- `chrono`/`uuid` 直接复用工作区已锁版本；关闭默认特性，前者仅需 std，
  后者仅解析现有 UUID，不生成 UUID。锁文件只新增 app 对两项既有包的依赖边。
- `rollout_path`/`cwd`/可选 git_branch 由调用方提供；不伪造原会话 cwd。
  M3 的显式 cwd 差异继续保留，生产接线属 M7。

## 3. TDD 与补强

- RED：先建立 `memory_storage_files` 及黄金主文件，再运行
  `cargo test --workspace --test memory_storage_files`；exit 101，缺少
  `sync_stage1_snapshot`/`Stage1FileInput` 与 `list_stage1_outputs`。
- 初次 GREEN 6/15：9 个失败来自测试辅助误排除上游文件名
  `memories_1.sqlite`；实际本地库名为 `memories.sqlite`。根因定位后复用
  `memories_db_path` 排除并单独断言 DB 字节，未改任何黄金主文件。
  修正后 **15/15**。
- comprehensive-testing：按契约检查缺口，增加 260 来源无上限/年龄过滤、
  快照读取后数据库改变仍导出原快照、布局失败与根常规文件、负时间戳、
  权限收紧、canonical 链接与终端安全告警；合计 **22/22**。
- A20 用真实 store 写入/无内容成功删除，检查完整产物文件集合和黄金字节；
  幂等不依赖 mtime。共享常规文件/子目录/大写后缀保留。
- A13 经 `run_stage1_job` →严格解析脱敏→真实 SQLite→真实导出；模型边界仅固定
  闭包响应并断言调用 1 次；正文 marker、准确脱敏后 slug 文件名与黄金字节均检查，
  另断言原合成会话和 DB 字节未变。
- 故障为真实文件系统结构：canonical 文件位置建目录→摘要写失败；raw 路径建目录
  →raw 写失败；旧 `.md` 建非空目录→删除告警并继续。不依赖 chmod 在 root 下失效，
  分别检查已发生的删除/写入、原 raw 保持或摘要已写、数据库未变。

## 4. 错误检测证明

仅复制 Cargo 元数据和源码/测试到 `/tmp/tchat-m4-mutants`，不复制
`.git`、compose、真实配置或 `db/`。所有 Cargo 命令显式
`--target-dir /tmp/tchat-m4-mutation-target`，与全局共享 target 隔离。
变异只发生在副本；断言不变，最终恢复副本正确行为并全目标复跑。

| 故意破坏 | 对应观测 | 结果 |
|---|---|---|
| 跳过旧摘要清理 | 目录完整文件集合 | 运行时断言失败 |
| `.md` 错为 `.MD` | 小写/大写与直属边界 | 运行时断言失败 |
| 改坏空占位正文 | A20 归零黄金字节 | 运行时断言失败 |
| raw 不 trim | A20 新增黄金字节 | 运行时断言失败 |
| 跳过根内链接清理 | 链接目标保持与移除结果 | 运行时断言失败 |
| 文件名忽略已脱敏 slug | A13 精确文件名/来源引用 | 运行时断言失败 |
| SQL 快照改降序 | 全量快照返回顺序 | 运行时断言失败 |
| 吞掉摘要写入错误 | 返回值＋raw 原状＋已发生副作用 | 运行时断言失败 |
| 丢弃诊断 | 旧 `.md` 删除失败告警 | 运行时断言失败 |
| 绕过私有写入函数 | 已存目录/文件权限收紧 | 运行时断言失败 |

上述 **10/10** 检出（各为 1 条执行/1 条失败，均非编译失败或空转）；副本
基线与恢复后均 **22/22**。安全轮再增加独立新根用例，移除根初始化的第 11 项
变异被该用例检出；更新副本测试设施后恢复为最终 **23/23**，移植源码与工作区
候选字节一致。未改变原断言、未放宽黄金文件；本地工作区生产文件未做变异。
验证结束后已清理副本、独立 target 及临时上游参考副本，仅保留关键日志/结果。

## 5. Security Testing Gate

覆盖、删除、布局链接处理属于新增文件写入面，进入并通过详细检查。
`cargo test --workspace --test memory_storage_files` = **23 通过 / 0 失败 / 0 忽略**，
非零执行；包含此前 22 项与独立无数据库新根用例。

| 安全面 | 证据 |
|---|---|
| 管理范围 | 直属任意/旧 ID/隐藏小写 `.md` 清理；keep 同名覆盖；大写后缀、备份、子目录及共享普通文件保留 |
| 文件名 | 固定 UUID v7 前缀/hash、非 UUID fallback、ASCII/Unicode/路径分隔符 slug、60 字符上限的准确结果；完整函数正文与固定上游对照（仅类型/借用/可见性适配，忽略文件尾空白） |
| 链接 | 根本身链接拒绝且保留；canonical 文件、摘要目录、raw、skills、`.git`、dangling link 均移除链接本身，外部目标完整字节不变 |
| 权限 | 新根 0700、新 raw 0600；有产物目录 0700、产物 0600；已有 0777/0666 收紧，DB 保持原 0600 |
| 脱敏 | A13 三字段真实 M3→DB→文件链；正文 marker 与准确 slug 投影，原会话/DB 不变 |
| 错误与诊断 | 布局/摘要/raw 真实故障、副作用和 DB 状态；删除失败继续且有告警；恶意 ESC 文件名诊断经 display 净化 |

全量文件层静态检查无模型、子进程、任务提交、`remove_dir_all` 入口。
新根用例证明文件层不要求数据库或 Git 基线。非终端系统边界均真实执行，
无核心文件系统或 SQLite 替身。

## 6. code-review

安全检查通过后，由主 Agent 手工做两遍审查（未冒充 Skill 或独立代理审查）：

- 规格轴：逐项核对全量/顺序/格式/空集合/keep 清理/部分失败/告警/链接/A13，
  不添加来源选择、合并代理、后台触发或新的配置面。检查出入库状态边界，
  M4 只读 DB、只投影显式快照，M5/M7 责任未提前实现。
- 复用与质量轴：命名函数保留上游；私有写入与显示净化复用既有函数；
  单行/全量读取共用行映射；无新增包版本或未使用代码；M4/L2 相对基线的编译
  warning 标题增量为空。全量另有既有 memory_store/memory_phase1/session_cli/
  e2e/catalog 测试警告，均定位到未改动源码；新 M4 源码/测试无 warning。
- 已处理的范围内问题：TDD 测试辅助库名误认；新行映射插入位置的文档注释归属；
  临时目录设施改复用 RAII＋pid/计数器（不依赖真实时钟/随机源）。审查无其他
  必须修复发现，未通过删除用例或放宽断言消除失败。
- 受影响 L2 实跑：`cargo test --workspace --test memory_storage_files --test memory_phase1
  --test memory_candidates --test memory_store` = **22＋19＋18＋21 / 0**；
  `cargo test -p terminal-chat --lib memory::` = **22/0**。之后仅补新根用例与测试
  目录分配设施，安全轮已以完整 M4 目标 **23/23** 复验，最终全量还将覆盖全部。
- 持久化 Schema/已有接口未改变，仅新增内部 Rust 文件投影 API 和只读查询；
  本项目非发布库，无适用的 semver/IDL 兼容性检查命令。

## 7. verify:full 与最终状态

审查后对当前候选执行，Cargo 离线（`CARGO_NET_OFFLINE=true`），显式 UTC 与
`LC_ALL=C.UTF-8`；HOME/USERPROFILE/APPDATA/XDG 配置、数据、缓存及 TCHAT 用户与
配置目录均指向本次私有临时目录，结束已清理。仅保留实际工具链/依赖缓存路径供 Cargo。

| 层级 | 实际命令 | 结果 |
|---|---|---|
| L1 | `cargo test --workspace --test memory_storage_files` | 23 通过 / 0 失败 / 0 忽略 / 0 过滤 |
| L2 | `cargo test --workspace --test memory_storage_files --test memory_phase1 --test memory_candidates --test memory_store` | 联动轮 22＋19＋18＋21 通过，0 失败；新增独立新根用例后 M4 23/23，最终全量覆盖 |
| L2 | `cargo test -p terminal-chat --lib memory::` | 22 通过 / 0 失败 / 100 无关用例按名过滤（仅定向检查，不作全量依据） |
| L3 | `cargo check` | exit 0 |
| L3 | `cargo test --workspace --no-fail-fast` | exit 0；**2879 通过 / 0 失败 / 3 忽略 / 0 过滤；除忽略项外未运行 0** |

最终全量中 M4 **23/23**、`e2e_acceptance` **95/95**；M3 2856＋M4 23＝2879，
通过数对账一致。3 个忽略项均属未改动的原有源码（已与 HEAD 逐字核对）：

- `bottom_pane::textarea::tests::altgr_ctrl_alt_char_inserts_literal`：非 Windows
  `cfg_attr(ignore)`，Linux 不执行此 AltGr 平台用例。
- `tui::owned_screen_tests::owned_screen_lifecycle_child`：标记 ignore，实际由正常
  父测试用 `--exact --ignored` 派生执行；父测试通过。
- `tui::panic_tests::caught_panic_pty_child`：同为父测试显式启动的 PTY 子入口，
  `src/tui/src/tui_panic_tests.rs`，父测试通过。

未新增忽略项。clippy/rustfmt 未安装，**两项工具检查未运行**，不声称通过；
完整回归的忽略/未运行计数与工具检查分开报告。原有 shim/TUI 及既有 app 测试
警告已区分，不把它们写成新增问题或零警告。

验证开始/结束比对 Cargo 清单/lock、新旧生产文件、新测试与全部黄金文件 SHA-256，
完全一致；之后只回填文档，不改影响验证结果的代码。完整日志与指纹在本轮
`/tmp/tchat-m4-evidence/`，本记录保留关键证据而不重复提交大体积日志。

实施轮收尾 Git（批准前历史）：分支为 `feat/013-memory-m4`，HEAD 与 main 均为完整基线
`0b429aefa19772bd00fe251a51e6c74c4671b3f0`，领先/落后 0/0，暂存区为空。
本轮候选文件均未提交；既有 compose 修改指纹与 §1 一致，`db/` 仍未跟踪，
`db/wiki.db` 未打开/修改/纳入测试或副本。013 的准备登记已保留并回填 M4 待批准；
未提交、合并、推送或删除任务分支。

## 8. 限制与后续交接

- 保留上游非原子语义：写失败可能已有删除、覆盖或截断；单个旧件删除失败
  仅诊断后继续。调用方必须显示/记录诊断，不能据返回 Ok 宣称所有旧件已清理。
  不为此添加回滚、清单或独立重试机制。
- `Stage1FileInput` 的元数据由调用方提供；必须交入完整、thread_id 升序快照。
  本阶段不推断原会话 cwd，不引入 M5 来源选择、合并/Git 或 M7 生产触发。
- 本轮平台为 Linux；Windows 权限/目录链接分支未实跑，不宣称 ACL 已验证。
  无 REPL/按键行为改动；完整回归仍包含既有 CLI/PTY e2e。
- clippy/rustfmt 缺失，两个工具检查未运行；Rust 编译检查及手工复用/未使用代码
  审查已执行。未安装新工具或改写真实用户配置来凑检查通过。

## 9. 批准交付与合并核验（2026-10-18）

用户本轮明确批准提交 M4、合并 main、核验后以 `git branch -d` 安全删除本次
本地任务分支，并授权必要的交付文档提交；不推送、不发布、不删除远程分支。

- 复核 AGENTS.md、完整 Git 状态、暂存区、工作树与验证记录：分支为
  `feat/013-memory-m4`，任务 HEAD 与实际 main 均为
  `0b429aefa19772bd00fe251a51e6c74c4671b3f0`，暂存区为空。
  已有 compose 指纹与 §1 相同，未跟踪 `db/` 保留，不打开 `db/wiki.db`。
- 核对 15 项源码/Cargo/测试/黄金文件 SHA-256 均与最终验证时一致。
  用精确文件清单 `git add -- <17 个逐项文件路径>` 暂存上述 15 项及现有 013/
  实施记录；索引文件集合与批准清单完全一致，逐项索引字节亦与工作树一致。
  源码/文档 `git diff --cached --check -- <8 个非黄金文件路径>` 通过；
  黄金文本保留规定末尾空行，9 份全部以既有指纹逐字节核验，未为样式检查改快照。
- M4 实现提交：`4cd9f19e10393efa97c0fc8d9cb826fd9bdac611`，
  `feat: 013-M4 全量 stage1 快照同步 Markdown`，17 文件。
  `git diff-tree --no-commit-id --name-only -r <实现提交>` 与精确清单一致；
  `git show <实现提交>:<路径>` 逐项核对批准字节，compose 与 db 均未入提交。
- main 实际状态仍为基线，执行 `git switch main`、
  `git merge --ff-only feat/013-memory-m4` 成功（Fast-forward）。
  合并后 main 精确等于上述实现提交；
  `git merge-base --is-ancestor 4cd9f19e10393efa97c0fc8d9cb826fd9bdac611 main`
  exit 0，`git diff --exit-code 4cd9f19e10393efa97c0fc8d9cb826fd9bdac611 main`
  exit 0（交付文档提交前）。再次核对 main 中与工作树的 15 项候选指纹，全部一致。
- 合并未引入代码差异，依据本轮授权及 AGENTS.md 不重复无新增信息的检查，
  复用 §7 的完整验证：`cargo check` exit 0；
  `cargo test --workspace --no-fail-fast` exit 0，**2879 通过/0 失败/3 既有忽略**，
  M4 23/23、CLI/PTY e2e 95/95。交付轮仅执行 Git/字节核验，未调用真实模型，
  未在真实记忆目录运行同步或清理。Windows 权限未实测、clippy/rustfmt 未运行
  以及三项既有忽略的说明全部保留。
- 合并核验成功后，013 的 M4 更新为**已交付**，M5–M9 保持未实施。
  交付文档提交仅限 013 与本记录两文件，不引入影响验证结果的代码变化。
  任务分支的安全删除及 main 最终 SHA 由本轮最终 Git 核验报告给出；不涉及远端。
