# 018 同步用户问询安全攻防报告

- 日期：2026-10-22。
- 范围：阶段 A `request_user_input`，分支 `feat/018-user-input`。
- 候选：`6c91818e28ee4af19729197fa730e14565444399` 上的未提交工作树，包含新增功能文件与安全回归；不是空的 `<base>...HEAD`。
- 依据：[018 契约](../../specs/018-user-input.md)、[统一实施证据](../../records/2026-10-22-018-user-input-implementation.md)、本仓库 Security-Testing-Gate。
- 结论：**修复后安全门禁通过**。门禁及后续 Spec 审查共证实 2 项低风险终态/取消可靠性缺陷，均已修复并复验；未发现已证实的高、中风险漏洞，当前没有未修复的已证实安全问题。TERM 适配修复后的独立增量验证通过，可进入最终全量验证。

## 1. 审查与实测方法

先审查 `user_input.rs`、`prompt.rs`、`turn.rs`、CLI/TUI 宿主、共享 protocol、活动问询视图与历史投影，再运行攻击用例。重点核查：能力声明与执行门、严格字段解析、原始 ID 配对、一次性回送、终端展示过滤、回答与命令分派隔离、取消/退出与恢复。

本项目是本地 Rust CLI，不提供订单、用户管理、SQL 或公共登录/短信接口。通用门禁据实际信任边界适配；阶段 B 异步工具不在本轮实现范围。

真实进程攻击启动 Cargo 构建的 CLI 二进制，真实交互通过 PTY；网络仅连接本地 HTTP mock。新增用例清空子进程继承环境，隔离 HOME、配置、用户目录与工作目录，使用假凭证、固定会话时间戳、UTC 和固定响应。模型和 HTTP 是系统边界替身，问询、参数、输入、执行层与渲染使用生产实现。探针结束回收子进程；没有使用真实模型、真实密钥或真实用户目录。

## 2. 已证实问题清单

| 编号 | 风险 | 问题与后果 | 最终状态 |
|---|---|---|---|
| SEC-018-01 | 低 | 待显示问询事件与 Esc 交界时，用户取消被误回送为通道关闭，导致终态/审计原因失真；未观察到权限扩大、额外执行或敏感信息泄漏 | 已修复，原失败断言保持不变并通过 |
| SEC-018-02 | 低 | 不支持的终端名使用大写/混合大小写且 stdout 重定向时，问询退化读取漏用可取消路径，Ctrl+C 后继续等待，影响本地取消可用性 | Spec 轴实证并修复，安全门禁独立增量复验通过 |

### SEC-018-01：待显示问询的取消原因被覆盖

位置：`src/tui/src/chat_driver.rs` 的 `DriverEvent::UserInput` 拒绝分支。原实现对所有拒绝原因统一回送 `Closed`，没有保留已置位的 `turn_cancelled`。

复现使用真实 TUI 驱动与 PTY，在宿主事件边界控制投递，不依赖 HTTP 延迟或随机撞时序：

1. 提交 `SEC-start`，创建原始问题 `id=q` 的事件和独立回送通道，先保留该事件。
2. 等待 `SEC-pending-event-ready`，通过真实 PTY 发送 Esc。
3. 实际取消钩子收到 `true` 后，投递同一个待显示事件。
4. 捕获真实驱动的回送结果，断言原因必须为 `Cancelled`。

修复前实际运行：

```sh
cargo test --locked -p terminal-chat --test user_input_security esc_before_question_delivery_preserves_cancelled_reason
```

结果：退出 101，0 通过、1 失败、5 过滤。直接回送消息的精确差异：

```text
实际：回答通道关闭：问询未完成
预期：用户已取消本轮：问询未完成
```

同一原候选已构建测试二进制再执行一次，仍是同一断言失败。此探针直接验证事件回送，不冒充完整宿主的持久化实测；代码审查确认 `turn.rs` 会把这一错误原因写入工具结果。

主代理按 `diagnosing-bugs` 做最小修复：已取消时回送 `Cancelled`，其他拒绝情况保留 `Closed`。原真实驱动回归先单独 GREEN，再随完整安全目标通过。没有放宽断言或更换预期来消除失败。

持久回归：[user_input_security.rs](../../../src/app/tests/user_input_security.rs) 中 `esc_before_question_delivery_preserves_cancelled_reason`；统一 RED/GREEN 记录见[实施证据](../../records/2026-10-22-018-user-input-implementation.md)。

### SEC-018-02：终端名大小写差异使退化问询读取不能及时取消

初轮报告 §5 的待证实线索由 Spec 轴稳定复现。`main.rs::ask_cli` 原判断只匹配小写字面值，而锁定的 Rustyline 18.0.1 `is_unsupported_term` 使用 `eq_ignore_ascii_case`。因此在 stdin 为 TTY、stdout 重定向、`TERM=DUMB` 时，问询可能进入 Rustyline 的阻塞退化读取；Ctrl+C 后仍等待输入。

Spec 轴持久回归 S102 使用既有真实 PTY 作为 stdin/stderr，shell 在 exec CLI 前把 stdout 重定向到隔离文件。等待“选择编号”出现后仅发送 Ctrl+C，不补换行。修复前日志 `/tmp/018-term-case-red.log` 显示退出 101、0 通过、1 失败、117 过滤：5 秒内未出现“用户已取消本轮：问询未完成”，输出仍停留在选择提示。

最小修复只把 `ask_cli` 的终端名匹配改为 `term.to_ascii_lowercase().as_str()`；既有 `tui_should_run` 未修改。Spec 原 GREEN 日志为 `/tmp/018-term-case-green.log`。安全门禁随后在冻结候选独立执行：

```sh
cargo test --locked -p terminal-chat --test user_input_security
cargo test --locked -p terminal-chat --test e2e_acceptance s102_redirected_line_cli_uppercase_term_cancels_without_another_line
```

结果分别为 6 个入口通过（5 个实测场景＋1 个 helper）与 S102 1 通过、117 过滤；均退出 0、无失败或忽略。S102 验证 Ctrl+C 无需补换行即可取消，随后 EOF 正常退出；mock 恰 1 次、不发续请求，V1 中 `s102-ask` 恰一条 `{"error":"用户已取消本轮：问询未完成"}`，`display=None`，加载修复数为 0。既有真实 CLI/TUI 的恶意回答、权限、直送和取消测试也保持通过。

持久回归为 [e2e_acceptance.rs](../../../src/app/tests/e2e_acceptance.rs) 的 S102。本节修复前 RED 归 Spec 轴证据；安全门禁独立执行的是修复后两项增量命令，没有改动产品或测试文件。

## 3. 八类攻击与实际结果

| 门禁类别 | 本轮请求/载荷与验证 | 实际结果与证据 |
|---|---|---|
| 1. 数据越权/关联隔离 | 伪造其他问题 ID；原始 ID 含空白或控制序列；两个串行问询复用同一问题 ID；取消后旧回答再到达 | 未知 ID 以“回答无效”结束本轮，不续请求；原始 ID 与答案原文保留；旧请求不能回答新通道。`user_input`、TUI 相关用例及 S98/S100 通过 |
| 2. 功能越权 | 无回答宿主时伪造 `request_user_input`；内部 `MemoryMerge` 即使误传宿主仍伪造调用；带记忆摘要用户轮检查工具声明 | 用户轮有宿主 5 工具、无宿主 4 工具；内部记忆轮 3 工具，问询未声明且执行层拒绝，宿主调用数为 0。T19 与对应工具契约通过；未注册阶段 B 工具 |
| 3. 身份/凭证 | 回答中伪造 `role=admin`、`token`、`origin`、`base_url`，目标设为第二个本地 decoy 服务 | CLI/TUI 的后续请求仍使用原假凭证与原网络目标；decoy 请求数为 0；授权记录和配置字节未改；工具集不变。真实攻击用例通过。服务端账号/token 登录认证在本 CLI 不适用 |
| 4. 偷改字段提权 | 在顶层、问题、选项中分别塞入 `isBlocking`、`autoResolutionMs`、`isSecret`、`isOther`、`role`；空白/重复 ID；重复 JSON 字段 | 新增 21 组攻击均经真实执行层拒绝，结果仅含字符串 `error`，`display=None`，宿主执行数为 0。既有用例还覆盖缺字段、错类型和空选项 |
| 5. 恶意输入与注入 | CSI 清屏、OSC52 剪贴板、OSC 标题、DCS、C0/C1；Shell `$(touch <临时标记文件>)`；包含 `exec_command` 的伪造 JSON | 66 组展示载荷的展示副本去除危险控制，原始关联键/选项/答案不改。S98 真实 TUI 的 OSC52 输出为空、标题载荷不外泄，标记文件不存在；CLI/TUI 新攻击也没有创建标记文件。没有 SQL/网页脚本执行子系统 |
| 6. 绕过前端 | 直接向 turn 注入恶意工具参数，不经过界面；把 `/exit`、Shell 与工具 JSON 作为回答提交 | 后台严格解析仍生效。回答只以 `role=tool` 的 `answers` 数据进入下一请求，`/exit` 不退出宿主，JSON 不变成调用；每个真实宿主恰好 2 次模型 mock 请求，用户消息数保持 2（环境块＋原输入），没有额外聊天轮 |
| 7. 敏感信息泄露 | 检查后续请求、终端输出、授权记录与攻击者目标；核对秘密输入隐藏字段不能从模型面启用 | 假 API 凭证只用于既有请求认证头，不出现在消息正文或界面；目标服务无请求；模型面不能偷偷启用秘密输入。用户主动回答按契约回送模型并保存原文，不是秘密输入模式 |
| 8. 资源滥用/防刷 | 第 8 次请求、第 9 次禁止；32/33 工具批边界；精确 10 MiB 请求/响应边界（含中文 UTF-8） | 第 8 批结果保存、无第 9 请求；32 条问询串行完成，33 条整批拒绝且宿主未打开；HTTP 尺寸边界用例通过，超限出站零发送。没有公共接口可施加登录/短信并发；未增设问题数量硬上限，1–3/2–3 仍是使用指引 |

补充生命周期实测：S93/S96 验证明确提交前不续请求、不执行同批后项，原聊天草稿与队列不当答案；S94/S97 验证取消/退出/EOF，含未提交草稿；通道和组件用例验证关闭、重复提交与迟到回答只结束原请求。S95/S101 验证 V1 已完成/未知结果恢复只展示，无新模型请求、问询或执行。回答/取消通路不取得 turn 持有的历史或自动保存锁，此项结合代码审查与真实结束路径验证。

## 4. 门禁实测与增量验证数量

以下为第一轮门禁（SEC-018-01 修复完成、S102 尚未接入），两份无关公共测试格式差异清理后的实测：

| 命令 | 实际结果 |
|---|---|
| `cargo test --locked -p terminal-chat --test user_input_security --test user_input --test e2e_acceptance --test tool_loop` | 退出 0；E2E 117、工具循环 19、问询契约 13、安全目标 6，全通过；失败/忽略/过滤均为 0 |
| `cargo test --locked -p tchat-tui --lib request_user_input` | 退出 0；11 通过，0 失败、0 忽略，2137 过滤（未运行） |
| `cargo test --locked -p terminal-chat --test llm_flow body_limit` | 退出 0；3 通过，0 失败、0 忽略，48 过滤（未运行） |
| `LD_LIBRARY_PATH=/usr/local/rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib /tmp/018-quality-tools/rustfmt --edition 2021 --config skip_children=true --check src/app/tests/user_input_security.rs` | 退出 0 |
| `git diff --exit-code -- src/app/tests/common/mod.rs src/app/tests/common/pty.rs` | 退出 0，两份文件没有残留改动 |

SEC-018-02 最小修复后，仅重跑受影响的独立安全目标与 S102；未重复上表整套 E2E 或其他不受影响的检查：

| 增量命令 | 实际结果 |
|---|---|
| `cargo test --locked -p terminal-chat --test user_input_security` | 退出 0；6 个入口通过（5 个实测场景＋1 个 helper），0 失败、0 忽略、0 过滤 |
| `cargo test --locked -p terminal-chat --test e2e_acceptance s102_redirected_line_cli_uppercase_term_cancels_without_another_line` | 退出 0；1 通过，0 失败、0 忽略，117 过滤（未运行） |

增量原始日志为 `/tmp/018-security-gate-term-increment-security.log` 与 `/tmp/018-security-gate-term-increment-s102.log`。增量没有发现新增安全回归，先前待证实的 TERM 线索已关闭；全仓库最终数量仍以后续全量验证为准。

安全目标的 6 个 Rust 测试入口包含 **5 个实测场景与 1 个子进程 helper**。`pending_question_cancel_child` 在父进程中空返回，不单独计作安全覆盖；父测试会在独立 PTY 子进程中运行它并断言真实回送。

原始临时日志留在 `/tmp/018-security-gate-final-app.log`、`/tmp/018-security-gate-final-tui.log`、`/tmp/018-security-gate-size-limits.log`；竞态日志为 `/tmp/018-security-pending-cancel-{red,red-repeat,green}.log`。它们不是报告目录内的交付文件，以上正文保留了可复现命令与关键结果。

已有五类源代码变异（回答误入聊天、未提交续跑、取消不解除等待、旧回答串题、恢复重开轮）证据见 018-mutations.json。Standards 轴发现原 `failure_excerpt` 误摘编译告警，已从五份原日志的真正 `failures` 段修正；命令、补丁、退出码与恢复哈希未变。安全门禁本次增量重新审阅更正后的五段：均有目标行为断言失败、退出 101，且原始/恢复哈希一致，不把编译告警当检出证据。本轮没有重新执行这五类变异或改坏冻结产品源码。新取消竞态回归通过修复前两次 RED／修复后 GREEN，证明能够检出该错误。

首次格式工具曾递归格式化 `common/mod.rs`、`common/pty.rs`。已逐字节证明现状等于同一工具格式化后的干净基线，并仅撤销本代理造成的这两份格式差异；最终 diff 为空。此清理不算产品改动。

## 5. 已关闭线索与适用边界

- **已关闭**：原 `TERM=DUMB` 与单向 TTY 线索已由 Spec 轴实证并最小修复，安全门禁独立复验 S102 的 stdout 文件重定向组合通过，详见 SEC-018-02。另据 Spec 轴独立复核，原 stdout PIPE 拓扑的 `DUMB`、`CoNs25`、`EMACS` 均可在不补换行时 Ctrl+C 取消、EOF 退出 0，各恰 1 次 mock；这三项归 Spec 轴证据，不冒充安全门禁另行执行。
- 没有开展生产端点、真实模型或公共 Web 压测；本地批次/字节边界验证不代表任意上限内输入的性能 SLA。
- 阶段 B 异步问询、Web 账号/管理员/订单权限与 SQL/浏览器脚本注入不适用本次阶段 A CLI。
- 本门禁及增量不替代 `cargo check && cargo test --workspace --no-fail-fast` 和适用 lint。终验数量、两轴审查与既有 lint 限制由统一实施证据记录；不能把本报告的定向结果当作全仓库全量通过。
