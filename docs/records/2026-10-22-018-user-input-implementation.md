# 018 同步用户问询实施与验证

实施契约：[018-user-input](../specs/018-user-input.md)，只实施阶段 A。阶段 B 未注册工具、未建设运行机制；无新增第三方依赖和版本升级。

当前状态：**阶段 A 已获用户批准并交付本地 main（2026-10-23），合并核验及本地任务分支清理均已完成**。全量 3142 通过、0 失败、3 原有忽略；全目标 lint 的两处旧测试错误及未运行环境见 §6，不报告全部检查无例外通过。

- Git 基线：本地 `main`＝`6c91818e28ee4af19729197fa730e14565444399`（017 已交付）。
- 原任务分支：`feat/018-user-input`（已删除）；本地 `main` 已快进包含任务提交。实际交付顺序与核验见 §7；未推送、未发布、未删除远程分支。
- 初始现场：两份已修改文档与五份未跟踪文档已逐文件备份并记录 SHA-256（`/tmp/018-starting-state`）；不 stash/reset/clean。三个无关未跟踪记录保留原样。
- Codex 固定来源：`d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`。从本地源码缓存按 `git show <固定提交>:<路径>` 读取，未使用缓存 HEAD 代替固定基线。

## 1. 实际复用与适配

上游路径均相对于 `codex-rs/`，链接锚点见 018 §2。

| 上游来源 | 本地落点 | 实际适配 |
|---|---|---|
| `protocol/src/request_user_input.rs` | `src/shims/codex-protocol/src/request_user_input.rs` | 共用 Question/Option/Answer/Response；去掉 Schema/TS 派生，不恢复事件包装。宿主结束原因是本地临时类型，不持久化 |
| `core/src/tools/handlers/request_user_input_spec.rs` | `src/app/src/user_input.rs`、`prompt.rs` | 同步 Schema 转 Chat Completions function；保留数量指引、每题非空选项、启用 Other。补严格字段与非空/唯一原始 ID 校验；描述适配交互宿主 |
| 同步 handler、`core/src/session/mod.rs::request_user_input` | `turn.rs`、`user_input.rs`、`tui_host.rs` | 保留等待回答后再回传；用宿主回调和每次独立标准库通道，未移入上游会话运行框架。无接口仍四工具，内部记忆轮仍受限且丢弃误传问询接口 |
| `tui/src/bottom_pane/request_user_input/{mod,layout,render}.rs` | 同名本地目录 | 复用选项、逐题草稿、Other、前后切题、未答确认与布局；本地一次性回送替代 App Server。去掉异步队列、自动空答、倒计时和图片；Esc 遵守本地取消职责 |
| `tui/src/history_cell/request_user_input.rs` | 同名本地文件、`tui_host.rs` 投影 | 共用 protocol 类型的局部别名；实时/恢复同一转换与 cell，结果 `display=None`；恢复只展示已完成/中断/未知，不重问 |
| `request_user_input/render_tests.rs` 与其原快照 | 同目录本地测试/快照 | 两条字符簇截断渲染回归原样复用；快照文件仅适配 crate 名前缀，黄金内容来自固定源码，不因测试失败重录 |

CLI 沿用主线程行式输入。Rustyline 18.0.1 的 unsupported TERM 分支使用阻塞 `Stdin::read_line`，SIGINT 后会继续等待；仅此退化路径用主线程 poll＋临时非阻塞读取，退出恢复原 fd 标志，未增加 stdin 读取线程。正常行编辑仍使用已有 readline。

TUI 回答/取消不获取历史与自动保存锁；未显示事件和活动视图均有释放终态。每次回复包装只消费一次 sender，旧事件的 live 标志失效，迟到回答只会落到已关闭的原通道。成功答案保留原始 ID/选项/文字；所有展示走现有过滤函数的副本。

## 2. TDD 与已定位问题

先确认受影响基线：应用库 162 项通过；`prompt_assembly`、`tool_loop`、`session_cli` 通过；TUI 库 2136 通过、3 个原有忽略。

| 反馈环 | 失败证据与根因 | 修复/复验 |
|---|---|---|
| 问题类型与真实工具循环 | 初始新入口不存在；宿主闭环测试要求五工具、按原始 ID 回传并等待明确提交 | 最小类型/归一化及可选宿主接入后通过 |
| S93 真实 TUI | 接入前问题不显示，直接续跑；新屏幕断言失败 | 独立回送与必要 overlay 接入后通过 |
| S94 真实 CLI | 接入前没有选择入口 | readline 问答、非法选择重输、中文补充、EOF 收尾通过 |
| S97 Ctrl+C | `TERM=dumb` 仅回显 `^C`，未解除阻塞读取 | 按 `diagnosing-bugs` 核对固定依赖的 `readline_direct`；退化读取适配后四条真实结束路径通过 |
| S98 原文保留 | 普通聊天粘贴过滤删掉答案中的 ESC/BEL，实际模型请求与原答案不等 | 问询使用原文插入＋展示副本过滤；危险终端序列不执行、原文配对保留，通过 |
| 已提交答案与取消交界 | 取消后仍可能发续请求 | 保存已受理答案后停止续请求。全 E2E 发现普通 exec 契约受影响后，将此规则收窄到含成功问询的批次；原 S29 不改断言并复验通过 |

S95 首次回看 Other 后的测试少了一步进入 notes，按固定上游交互补齐实际 Tab 操作；答案、模型请求与恢复断言不放宽。恢复渲染器的原测试文件引用缺失已补齐上游原测试和黄金快照。上述是实测失败和修复，不作为未执行阶段的通过证据。

计划内验收：`cargo test --locked -p terminal-chat --test e2e_acceptance --test user_input`＝116＋10 通过；`cargo test --locked -p tchat-tui --lib request_user_input`＝10 通过。后续加固增加的结果及最终全量以下方终验为准。

## 3. comprehensive-testing 状态审计

已在计划测试全绿后进入加固，读取技能 `reference.md` 的 CT-MBT 条款。

| 状态/事件 | 允许转换及结果 | 禁止行为/对应证据 |
|---|---|---|
| 无回答宿主、内部合并轮 | 明确拒绝伪造调用；无界面 | 不能靠模型工具名获得能力；工具契约、T19 |
| 参数无效 | 失败工具结果，界面不打开 | 不接收未声明字段；解析/循环测试 |
| 等待中，预选/时间推进 | 仍等待 | 不续请求、不执行后项；组件可控 tick、S93/S96/S100 |
| 明确提交 | 一条成功结果，原任务继续 | 不入普通聊天队列；S93/S96，原文 S98 |
| 有未答题 | 用户确认才提交或返回补答 | 不因预选替用户回答；S95/S99 |
| Esc/CLI Ctrl+C | 当前轮取消，未完成错误，同批补配对 | 不遗留等待、不执行剩余工具；通道测试、S97 |
| TUI Ctrl+C/CLI EOF | 宿主退出，保存真实原因，排空 | 不发续请求、不遗留悬空调用；S94/S97 |
| 通道关闭、旧/迟到事件 | 只结束原请求；新请求仍待明确提交 | 不串题、不重复终态；组件与通道测试 |
| 第 8 请求/32 与 33 调用 | 第 8 批结果仍保存；32 合法/33 整批拒绝 | 不发第 9 请求；边界测试 |
| V1 完成/中断/未知恢复 | 同源投影，只展示 | 不活动提问、不执行或联网；S95/S101 与结果黄金文本 |

加固只补不同风险：32 条问询串行上界、已送达终态与取消优先级、输入含未提交草稿时 EOF/取消、未知结果真实恢复、关键用例的失败检测证明。不引入通用状态机框架。

加固中按失败测试返回实现环节处理了三项：

- 非空草稿 Ctrl+D：canonical tty 先交付非换行片段，旧分支继续等待。新增真实 PTY 失败后，明确将此 VEOF 片段作为宿主退出处理，不当答案。
- 中文补充的可见光标：组件 RED 显示 `(12,7)`、期望 `(10,6)`；根因是显示改用过滤文本而游标仍使用原 composer 的边距。现复用 TextArea 的同一过滤副本计算渲染、换行/滚动和游标，并增加 S95 实际 PTY 光标断言；原回答不变。
- 非取消结束的跳过结果：新增工具循环 RED 显示 exec `status=null`，未保持 006 既定结构。现扩展已有未执行结果生产者，保留 exec/patch/plan 各自结构，只替换真实结束原因。

完整 E2E 与 clippy 并行期间 S44 第三次复制曾只捕获两条载荷；独立复验通过。其原有第二次复制已注明固定读次数会在负载下早退，第三次仍使用固定次数。仅将第三次等待改为同样的“实际载荷就绪＋截止时间”，保留数量与内容断言；问询 pre_draw_tick 也限定在问询视图内，避免改变普通 composer 行为。

## 4. 五类关键回归的有效性

五类源代码可逆破坏均实际运行，均为目标行为断言失败（退出 101），不是编译失败或空跑；每次立即按原始字节恢复并核对 SHA-256。补丁、完整目标命令、原/变异/恢复哈希、失败摘录及正确候选复验统一保留在 [018-mutations.json](../testing/evidence/018-mutations.json)。

| 故意破坏 | 实际检出路径 | 失败信号 |
|---|---|---|
| 成功答案额外排入普通聊天队列 | 真实 TUI S93 | 模型请求变成 3 次，期望 2 次，答案不应成为下一轮聊天 |
| tick 自动提交预选项 | 问询组件可控 tick | 未明确提交，回送通道却收到答案 |
| 忽略等待取消标志 | 真实等待通道 | 就绪后取消不能解除等待；看门狗先关闭原通道、回收线程再报错 |
| 旧回答路由到最新通道 | 迟到回答组件回归 | 新问询收到旧答案，原本应仍为空 |
| /resume 时启动新轮 | 真实 TUI 未完成 V1 恢复 S101 | 恢复/保存出现模型请求，期望零请求 |

正确候选复验已通过：E2E 117、工具契约 12、相关 TUI 10；之后增加的光标及收尾结构用例以最终验证为准，状态流保护断言未放宽。

## 5. 安全门禁与双轴审查

Security Testing Gate 已通过，[最终安全报告](../security/reports/2026-10-22-018-user-input-security.md)记录实际攻击与适用边界。修复后应用复验为 E2E 117、工具循环 19、问询契约 13、安全目标 6（5 个实测场景＋1 个子进程 helper），TUI 问询 11、HTTP 大小边界 3，均通过；定向过滤不算全量通过。

候选阶段已按固定基线对工作树进行 Standards/Spec 两轴独立审查。当时用户禁止提交，因此 `git log 6c91818..HEAD` 无新提交；实际比较用 `git diff 6c91818` 加逐个新增功能文件，不用空的 `<base>...HEAD` 代替实际候选。两份初始已有文档改动保留，三个无关未跟踪记录排除在审查范围之外。

安全门禁实证发现低风险终态问题：问询事件已创建但未显示时，真实 Esc 先触发取消，再投递原事件，驱动拒绝分支一律回送 `Closed`，导致历史原因错记。`user_input_security::esc_before_question_delivery_preserves_cancelled_reason` 保留实际驱动＋PTY 受控回归，修复前退出 101（实际“回答通道关闭”，期望“用户已取消”）；按 `diagnosing-bugs` 排除按键未处理与等待函数覆盖原因，只修正已取消事件的回送枚举，不增加管理器或新状态。原断言单独 GREEN，随后随完整安全目标通过；无未修复的已证实安全问题。

### Standards

独立报告：原发现 1 项 P2，当前无未解决发现。`018-mutations.json` 五项 `failure_excerpt` 错截了相同编译警告，不符合 AGENTS.md §2 的关键失败原因记录要求，削弱 §4 的有效性证据。现已从五份原始日志的 `failures:` 区段替换摘录，包含目标测试、实际 panic/断言和失败汇总；独立审查逐项核对原日志一致。命令、变异补丁、退出码、原/变异/恢复哈希及正向控制未改变，不重跑已完成的有效实验。

增量检查 CLI 一行修复和完整 S102，确认范围最小、复用既有 PTY、隔离配置和输出、按就绪等待，验证取消、EOF、请求数及 V1 终态；未发现可行动的 smell。保留无宿主兼容入口与上游必要界面形状不作为无依据抽象。

### Spec

独立报告：原发现 1 项 P2，已修复并独立复核，当前未关闭 0 项。018 §4 要求行式 CLI Ctrl+C 及时取消，不得无限阻塞。原 `ask_cli` 的 TERM 判断区分大小写，锁定 Rustyline 却按 ASCII 大小写无关匹配；stdin=PTY、stdout 重定向、`TERM=DUMB` 时会漏用可取消读取。S102 真实进程回归先 RED（5 秒看门狗到期，仍停选择提示），再仅对齐一行匹配规则，原断言 GREEN（0.51 秒完成），不改旧 TUI 探测或公共 PTY 辅助。

Spec 代理另用 stdin=PTY/stdout=PIPE 的原始探针独立复核 `DUMB`、`CoNs25`、`EMACS`：不补换行就产生正确取消终态，随后 EOF 退出码 0，每次恰一条 mock 请求。持久 S102 使用 stdout 重定向文件，并检查 V1 一次正确配对、`display=None`；两个拓扑均覆盖无终端 stdout。其余未发现需求遗漏、异步阶段越界或实现语义偏差。安全代理在最小修复后独立增量复验安全目标 6 项与 S102 1 项，均通过；[安全报告](../security/reports/2026-10-22-018-user-input-security.md)已关闭原 TERM 线索。

## 6. 最终验证与边界

最终冻结候选已在 `/workspace` 完成：

| 检查 | 实际结果 |
|---|---|
| `cargo check && cargo test --workspace --no-fail-fast` | 退出 0；53 个测试目标汇总 **3142 通过、0 失败、3 忽略、0 过滤**。环境 `CARGO_NET_OFFLINE=true`，无真实模型或密钥；无失败导致未执行的目标 |
| `git diff --check` | 通过；最终文档同步后再次复核 |
| V1 与旧入口兼容 | 全量中 `session_cli` 27、`prompt_assembly` 6、`tool_loop` 19、`memory_inject` 18 通过；E2E 全部 118 通过，含 S95 完成问答恢复、S101 未完成记录修复后只展示、S102 正确取消配对。`user_input` 13 与安全目标 6 通过 |
| 新增源码格式 | 10 个新 Rust 文件通过固定 rustfmt `--check --config skip_children=true`，app 用 edition 2021、shim/TUI 用 edition 2024；新增 S102 完整函数单独格式化，未格式化旧公共模块 |

全量中的 3 个忽略均为原有 TUI 条目：`bottom_pane::textarea::tests::altgr_ctrl_alt_char_inserts_literal`（Windows 专属）、`tui::owned_screen_tests::owned_screen_lifecycle_child`、`tui::panic_tests::caught_panic_pty_child`（子进程入口）。没有新增忽略或名称过滤；不把这些框架忽略项计作通过。全量临时日志 `/tmp/018-verify-full.log`，SHA-256 `b6206d719b204ed79635082837e724b6317723ef88c28d8b6c61ebd917c48778`。

兼容性以真实 V1 读写、锁/配对修复、恢复投影和旧入口请求/执行用例验证；`session.rs`、原持久化枚举形状、Cargo 清单与锁文件未改。新增 Schema 是独立新工具，无宿主入口保持四工具，protocol shim 为 `publish=false`；本轮无 Protobuf 或外部 SDK 协议变更，未安装/运行额外的 `buf breaking` 或 `cargo semver-checks`，仓库没有这类独立检查命令。

全局 rustup 安装 rustfmt/clippy 失败（下载错误：清理下载缓存时文件不存在，自动回滚）。随后按现有工具链发行清单的 URL/SHA-256 下载固定 1.99.0 组件到 `/tmp/018-quality-tools`，使用该工具链已有共享库；版本为 rustfmt 1.10.0-stable / clippy 0.1.99（同一 `b940084d7e`），没有升级工具链或项目依赖。最终 lint 在冻结候选串行补跑，固定组件直接调用：

- `LD_LIBRARY_PATH=/usr/local/rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib /tmp/018-quality-tools/cargo-clippy clippy --workspace --lib --bins --locked` 通过（原有 warning 与沿用 turn 入口参数方式的参数数量提示保留）。
- 同一固定组件执行 `clippy -p terminal-chat --all-targets --locked` 通过，覆盖新增安全测试和 S102。
- 同命令改为 `--workspace --all-targets --locked` 失败：`src/tui/tests/security_untrusted_text.rs:231`、`:235` 的 `clippy::unused_io_amount`。该文件本轮未修改，已与 `git show 6c91818:<路径>` 核对两段原文一致；不把此结果报告成全部 lint 通过，不顺手改无关旧测试。

三个 lint 日志为 `/tmp/018-lint-final-{production,app-all-targets,workspace-all-targets}.log`；退出码分别为 0、0、101，不把全目标 lint 失败写成通过。全仓 `cargo fmt --check` 未运行：存量文件格式并不统一，本轮只核对新增文件和完整新增函数，不顺手制造旧格式差异。

区段格式化脚本曾错误拼接重复缩进块，影响 TUI 宿主和 UserWithMemory 分支；发现后停用脚本，从 `6c91818` 对宿主重新应用三处最小功能补丁，恢复记忆分支，并审计其余原有删除行。之后完整 E2E 117 项、问询工具 12 项及相关 TUI 10 项重新通过；不把格式工具的成功退出当作代码正确证据。

验收范围：018 §7 的参数、宿主能力、真正等待、CLI/TUI 实际操作、多题/中文粘贴/未答确认、真实 PTY `TIOCSWINSZ` 宽→窄→宽后重排、取消/退出/EOF、聊天/迟到事件隔离、历史与额度、显示与副作用均有持久自动用例，并已纳入本次全量。五类关键错误的有效性证据见 §4；两处后来发现的取消边界也保留原断言 RED→GREEN。

未运行与剩余边界：当前环境为 Linux，未运行 Windows ConPTY 和实际目标宿主终端窗口验收；真实 PTY resize 与 VT100 屏幕断言不冒充这些平台/宿主证据，008 原有 A6 边界保持。没有对真实模型或公共生产端点发请求；阶段 B 及本地 CLI 不适用的 Web 权限/登录压测未实施。当前无已知未修复的 018 行为或安全发现。

2026-10-22 候选收尾核对已通过：终验期间及之后 1330 份源码/测试/依赖文件的冻结哈希一致，三个初始无关记录字节未变，公共测试 helper 无残留差异。14 份相关 Markdown 的 79 条本地链接可解析，9 处固定 Codex 源码引用存在，UTF-8/末尾换行/新增文件空白及五段实际失败证据复核通过。候选包含 22 个已跟踪文件差异与 18 个相关未跟踪文件，另有三个原有无关未跟踪记录排除；当时未暂存、未提交，`HEAD` 与本地 `main` 均为 `6c91818e28ee4af19729197fa730e14565444399`，当前分支 `feat/018-user-input`。当时未合并、未推送、未发布，未删除分支。

## 7. 已批准的本地交付（2026-10-23）

用户已明确批准：提交阶段 A 及必要文档，合并到本地 `main`，核验后删除本地 `feat/018-user-input`；不推送、不发布、不删除远程分支。

交付开始时的核验：任务分支与 `main` 均在报告基线 `6c91818`，暂存区为空；22 个已跟踪文件差异与 18 个相关新增文件均属于阶段 A 或必要文档。开工时两份已修改文档的既有 hunk 均为 018 问询计划/复核，不含额外的 016 修改；三个无关未跟踪记录排除。1330 份源码/测试/依赖文件与终验冻结哈希一致，三份无关记录与开工哈希一致。

本轮仅同步批准与交付事实，不改程序、测试或依赖，沿用 §6 的完整验证和既有 lint/未运行限制，不重复无关测试。以下记录已经发生的提交、合并、核验与清理。

首次暂存检查补抓到新增安全报告末尾的一个空白行；仅删除该空白行，报告结论、证据与程序均未改，重新核对暂存范围、内容及空白。

- 任务提交：`96682d0a9b873f3c797c0f4d685ea16507b15241`，`feat: 018 同步用户问询最小接入`；父提交为 `6c91818e28ee4af19729197fa730e14565444399`，范围恰为 40 个核定文件（22 个修改、18 个新增）。程序、测试与依赖逐字节沿用已验证候选，仅六份必要文档同步批准/交付准备事实或去除报告末尾空行。
- 暂存采用明确文件白名单，逐文件 `git add -- <路径>`；索引 blob 与各工作树文件逐字节一致，`git diff --cached --check`、`git diff --check`、文档编码/末尾换行及 14 份 Markdown 的 79 条本地链接检查通过。
- 执行 `git switch main`、`git merge --ff-only feat/018-user-input`，**Fast-forward** 成功；合并完成时 `main` 与任务提交 SHA 均为 `96682d0a9b873f3c797c0f4d685ea16507b15241`，树均为 `0751d2641adddc62e76abc739771cb29c84fb326`。
- 在补记交付结果之前执行 `git merge-base --is-ancestor 96682d0 main`、`git diff --exit-code 96682d0 main`、`git diff --check 6c91818 main`，均退出 0；`6c91818..main` 文件范围与 40 项白名单一致。合并后 1330 份源码/测试/依赖冻结哈希全部吻合，工作树及暂存区无已跟踪差异，三份无关记录哈希及未跟踪状态保留。
- 仅在上述核验成功后执行 `git branch -d feat/018-user-input`，实际输出 `Deleted branch feat/018-user-input (was 96682d0).`；`git show-ref --verify --quiet refs/heads/feat/018-user-input` 返回 1，确认本地分支已不存在。远程引用与交付前相同，未调用 pull/push 或发布操作。

后续提交只保存含本节在内的五份状态/结果文档，不改程序或验证结论。三份开工时的无关未跟踪记录继续保留：`docs/records/2026-10-21-opencode-global-agents-skills-install.md`、`docs/records/2026-10-21-vault-credential-recovery.md`、`docs/records/2026-10-22-codex-command-execution-and-process-interaction.md`，未展示或改写其正文、未纳入任务提交。
