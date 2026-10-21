# E2E 验收记录（2026-10-09 交付候选）

> 对象：当前未提交交付候选（F1/F2 安全修复、003 exec_command、004 apply_patch 之后）。
> 方法：REPL 走 cargo 本次构建的真实 CLI（`CARGO_BIN_EXE_terminal-chat`），PTY 由测试内
> libc 直接创建（posix_openpt/grantpt/unlockpt，不依赖 tmux/script）；exec_command/
> apply_patch 尚未接入 CLI，经最薄 JSON 宿主子进程调用真实库入口（harness，不构成
> Agent 工具调用接线）；网络边界一律本地 mock＋临时目录＋假凭证，配置读取、授权判断、
> 请求构造与文件操作全走生产实现。所有等待为 poll＋截止时间（看门狗），无固定延迟
> 猜测时序；子进程异常杀死并回收。
> 新增测试：`tests/e2e_acceptance.rs`（7 项，S1–S4、S6）；S5 与 F2 复用既有用例单独列示。

## 一、场景结果表

| 场景 | 测试 | 输入→预期 | 实际观测 | 结果 |
|---|---|---|---|---|
| S1 REPL 正常使用 | `s1_pty_normal_session_full_roundtrip` | PTY 会话：横幅→提示符→"你好"→回复→/models→exit；1 次请求、Bearer、请求体、退出码 0 | 请求 `POST /chat/completions`、`Authorization: Bearer s1-key-value`、body `model=deepseek-flash`/`content="你好"`；/models 展示手动配置的 mock 基础地址与 `上下文 1048576`；`Agent：S1-REPLY-收到`、`再见!`、退出码 0；授权记录未改写、密钥明文不出现在输出 | PASS |
| S2a 首次授权·同意 | `s2a_pty_first_use_authorization_agree_and_persist` | 交互询问→y→请求携带凭证；授权记录落盘且 (origin,指纹) 双匹配、0600；第二条消息不再询问 | 询问含凭证名/指纹前缀/origin；两次请求均 `Bearer s2a-key-value`；`authorized-origins.json` 新增 1 条 grant（origin＋fingerprint(s2a-key-value)、added_at>0、mode 0600）；首条回复之后无再次询问 | PASS |
| S2b 首次授权·拒绝 | `s2b_pty_first_use_authorization_deny_zero_requests` | 询问→n→零请求、零外发、不落盘授权、错误明确、干净退出 | `mock.count()==0`；授权文件不存在；`错误：已拒绝将凭证 TCHAT_S2B_KEY 发送到 <origin>`；`再见!`、退出码 0；无 `Agent：` 回显 | PASS |
| S2c 管道未授权（C2） | `bin_pipe_unauthorized_origin_rejects_without_request`（既有） | 管道会话未授权：零请求、fail-closed 提示、退出码 0 | 零请求、错误含 origin 与授权记录路径、`再见!`、退出码 0（本轮显式重跑通过） | PASS |
| S2d 换 Key 拒绝（C3） | `bin_pipe_rotated_key_invalidates_grant`（既有） | 旧 Key 授权＋新 Key 运行：零请求、授权记录不被改写 | 零请求、记录字节不变、退出码 0（本轮显式重跑通过） | PASS |
| S3a F1 攻击 | `s3a_f1_env_trust_root_hijack_blocked` | 恶意 .env（TCHAT_USER_DIR/HOME/TCHAT_CONFIG_DIR 大小写变体）＋攻击者目录预置受害者指纹授权；受害者凭证来自进程环境且攻击变量未预设 | 两个端点（受害者配置指向的 mock＋TCHAT_CONFIG_DIR 劫持目标）均 0 请求；错误指向受害者家目录 `…/victim-home/.config/terminal-chat/authorized-origins.json`（授权根未重定位）且含实际配置 origin；攻击者 grant 文件字节不变；受害者根未产生新授权；密钥明文不在输出；退出码 0 | PASS |
| S3b F1 正例 | `s3b_f1_positive_dotenv_credential_and_explicit_preauth` | 普通 .env 凭证变量照常加载；显式 TCHAT_USER_DIR＋合法预授权正常发送 | `.env` 的 `TCHAT_S3B_DOTENV_KEY` 生效：`Bearer dotenv-key-88`；预授权路径：`Bearer preauth-key-99`；各 1 次请求、回复展示、退出码 0 | PASS |
| S4 apply_patch 数据闭环 | `s4_json_host_apply_patch_data_loop` | JSON 宿主三阶段：新增→连续修改（补丁内级联两块＋跨调用依赖）→删除；逐阶段核对字节/存在性/changed_files；常规链接写穿；FIFO 与链接到 FIFO 拒绝 | 字节 `v1\nalpha\n`→`v2\nbeta\n`→`v2\nfinal\n`→不存在；changed_files 依次 `[add]`/`[update,update]`/`[delete]`；经链接更新 real.txt 成功且链接本体保持；FIFO 两例 `ok=false`、`不是常规文件`、`state_known=true`、空清单、FIFO 原样 | PASS |
| S5 真实半写失败 | `second_file_write_failure_reports_partial_state`（既有，EFBIG 隔离子进程） | 第二目标写入开始后失败：第一目标完整生效、第二目标截断前缀、清单只含第一项、state_known=false | `good.txt=b"uno\ntwo\n"`；`big.txt` 恰为新内容前 4096 字节前缀；changed_files 仅 good.txt；`File too large`；`state_known=false`（本轮显式重跑通过） | PASS |
| S6 exec_command | `s6_json_host_exec_command_streams_exit_and_pgroup_timeout` | 双流＋真实非零退出码；workdir 生效；超时后进程组真实终止（OS 级证据，非仅 timeout 结果） | `status=completed`、`exit_code=7`、out/err 两行均在；`pwd` 返回指定 workdir；`status=timeout`、`exit_code=null`、错误含"超时"；组长 pgid 与后台孙进程经 `killpg(ESRCH)`/`/proc/<pid>` 轮询在期限内从进程表消失（看门狗 5s，实测 <1s） | PASS |
| F2 同步屏障（单元级补充证据，不混计 E2E） | `apply_patch::tests::f2_swap_barrier_binds_write_to_validated_inode`（既有） | 写前确定性屏障：窗口内把目标换成 FIFO 后放行写入，须绑定已判型 inode | 本轮显式重跑通过（0.05s）；历史变异证明见安全报告 | PASS |

## 二、变异证明（测试有效性，AGENTS.md §4）

隔离副本 `/tmp/f1mut`（工作区 tar 副本，排除 target/.git；`--target-dir /tmp/f1mut-target`
专用），将 `src/catalog.rs` 的 `TRUST_CONTROL_VARS` 跳过改为 `|| false`（其余与工作区一致）：

- RED：`cargo test --target-dir /tmp/f1mut-target --test e2e_acceptance s3a` → FAILED，
  退出码 101。失败输出含 `Agent：S3A-EVIL2`（劫持端点对话成功＝凭证静默外发）与
  `已将 …/attacker-cfg…/manual.json 权限收紧`（配置根确被 .env 重定位），断言
  `TCHAT_CONFIG_DIR 劫持目标收到请求：left: 1, right: 0`。
- GREEN：恢复修复的工作区同一用例通过（见上表）。
- 结论：s3a 能确定性发现 F1 漏洞，非空跑。

## 三、命令与真实退出码

| 命令（容器内 /workspace） | 结果（退出码） |
|---|---|
| `cargo test --test e2e_acceptance` | 7 通过/0 失败（0），另连跑 5 次全绿（每次退出码 0） |
| `cargo test --test apply_patch second_file_write_failure_reports_partial_state -- --exact` | 1 通过（0） |
| `cargo test --lib apply_patch::tests::f2_swap_barrier_binds_write_to_validated_inode -- --exact` | 1 通过（0） |
| `cargo test --test llm_flow bin_pipe` | 3 通过（C1/C2/C3，0） |
| `cargo check` | 通过、0 警告（0） |
| `cargo test --workspace --no-fail-fast` | **157 通过/0 失败/0 跳过**：lib 70＋apply_patch 31＋e2e_acceptance 7＋exec_command 9＋llm_flow 40（0） |
| 副本变异 `cargo test --target-dir /tmp/f1mut-target --test e2e_acceptance s3a` | FAILED（101，预期 RED） |

基线对照：150（70+31+9+40）→ 157（+7 项新 E2E），既有用例无回归。

## 四、过程记录

- 首轮 s3b 失败：`run_pipe` 先 `env()` 后 `env_remove()` 把已设置的 `TCHAT_USER_DIR`
  又移除，正例读了真实 HOME——脚本错误，非产品缺陷；已改为先移除后设置，并给所有
  管道子进程显式测试 HOME。
- 首轮 s4 失败：对"同一 Update File 块内两个 @@"的清单预期写错（单块＝单执行事件）；
  按 D1 契约改为两个独立 `*** Update File:` 块，断言两事件。产品行为与规格一致。
- 变异首轮 RED 未出现真实外发：attacker 的 manual.json 误放 `config/` 子目录，
  `TCHAT_CONFIG_DIR` 值即目录本身导致回退内置地址；修正布局后 RED 完整
  （`Agent：S3A-EVIL2`＋劫持端点 1 请求）。
- PTY 会话宽窗口 200 列：授权询问含 origin，避免换行拆散子串断言。

## 五、验证边界（如实声明）

- exec_command/apply_patch 的宿主为测试 harness（测试二进制子进程模式，JSON 进/JSON
  出），验证宿主→库→OS 闭环；不宣称 Agent 工具调用已接入 CLI。
- 网络边界全部为本机回环 mock；未触真实外网与真实密钥。
- 001 规格交互行为（Ctrl+C/Ctrl+D 等）历史已验收，本轮未重复；exec_command 取消
  标志路径由既有 `cancel_flag_stops_managed_processes` 回归覆盖，未列入 E2E 场景表。
- PTY 平台语义为 Linux（posix_openpt/TIOCSWINSZ），与 AGENTS.md 平台约定一致。

## 六、用户手动实测（REPL，2026-10-09 追加）

> 来源：用户提供的真实终端输出（2026-10-09，Linux 容器、/workspace）；Agent 本轮未重新
> 执行，仅转录核对。手测触达真实端点（api.deepseek.com、models.dev），与上文自动 E2E 的
> 本地 mock 口径不同，分开列示；手测结果不计入自动测试数量（自动回归仍为
> **157 通过 / 0 失败 / 0 跳过**，见第三节）。`/refresh` 使工作区
> `config/auto_catalog.json` 产生真实更新，属手测副作用、如实保留。

### 手测记录（操作/输入｜关键实际输出｜判定）

| # | 操作/输入 | 关键实际输出 | 判定 |
|---|---|---|---|
| 1 | `cargo run` 启动，`/models` | 正常启动；deepseek-flash 为当前模型，并列展示 deepseek-v4-pro；终端显示两者上下文 1048576、输出上限 393216 | 通过 |
| 2 | 输入“你好，请回复‘手测通过’”，首次授权询问输入 `y` | 询问向 `https://api.deepseek.com` 发送 `DEEPSEEK_API_KEY`；确认后返回 `Agent：手测通过` | 通过 |
| 3 | 输入“请再回复一句话” | 正常回复；同一会话未再次询问授权 | 通过 |
| 4 | `/refresh`，同意从 `https://models.dev` 同步 | 基础资料更新 deepseek、openai；DeepSeek 查询到 2 个模型，OpenAI 因未设置凭证跳过；随后 `/models` 仍显示两个模型，所选模型不变 | 通过 |
| 5 | `exit`，随后 `echo $?` | 显示 `再见!`；退出码 0 | 通过 |
| 6 | `cargo run --bin terminal-chat` 重启后输入“你好” | 正常回复；未再次询问授权（跨进程授权复用通过） | 通过 |
| 7 | 空输入行按 Ctrl+D | 显示 `再见!` 并返回 Shell，退出码 0 | 通过 |
| 8 | `mktemp` 创建临时目录，经 `TCHAT_USER_DIR` 指定后启动；授权询问输入 `n` | 明确拒绝、无模型回复；`exit` 后退出码 0；`authorized-origins.json` 未生成 | 通过 |
| 9 | 同一临时目录：`printf '你好\nexit\n' \| TCHAT_USER_DIR="$manual_auth_dir" cargo run --quiet --bin terminal-chat` | 提示“非交互模式不做确认，已拒绝发送”；无授权询问与模型回复；`再见!`、退出码 0；授权记录未生成 | 通过 |

### 证据口径与范围

- 结论：上述 REPL 手动场景通过。
- 模型参数（上下文 1048576、输出上限 393216）仅为终端观测记录，不表述为已核实的官方参数。
- 场景 8/9 的手测证明终端行为与授权记录未落盘；未做抓包或请求计数，不据此声称零网络
  请求（零外发结论以上文自动 E2E S2b/S3a 的 mock 请求计数为准）。
- exec_command/apply_patch 未做手动验收，其验证仍以上文自动 E2E（S4/S5/S6）为准。
- 未记录密钥明文，不复制冗长回复，仅保留关键输出与复现命令。

## 七、基础工具 E2E 复验（同日第三轮：JSON 宿主定向实测）

> 对象与口径同第一节：复用既有最薄 JSON 宿主（测试二进制子进程模式），本轮为定向复验，
> 未新增手测入口、未做正式 CLI 接线、未改业务逻辑。为留档真实结果，另以同一宿主契约
> （`TCHAT_E2E_TOOL_INPUT/OUTCOME` 环境变量＋同一测试二进制）直接调用取回实际 JSON，
> 属证据采集，不构成新入口。临时产物 `/tmp/e2e-json-evidence` 已清理。

### 定向命令（各实际执行 1 项）

| 命令 | 结果（退出码） |
|---|---|
| `cargo test --test e2e_acceptance s4_json_host_apply_patch_data_loop -- --exact --nocapture` | 1 通过/0 失败（0） |
| `cargo test --test e2e_acceptance s6_json_host_exec_command_streams_exit_and_pgroup_timeout -- --exact --nocapture` | 1 通过/0 失败（0） |

### apply_patch 真实业务结果（直调宿主取回的实际 JSON 与文件字节）

- 新增：`{"ok":true,"state_known":true,"changed_files":[{"action":"add","path":"…/demo.txt"}]}`；
  落盘字节 `v1\nalpha\n`（od 核对 `v 1 \n a l p h a \n`）。
- 连续修改（第二块匹配第一块产物）：两条 update 事件入清单；字节变为 `v2\nbeta\n`。
- 删除：`{"action":"delete",…}` 单事件；目标不存在。
- 常规链接写穿：经 `link.txt` 更新后 `real.txt` 内容为 `mode=e2e\n`，链接本体仍为符号链接。
- FIFO 与链接到 FIFO：均 `{"ok":false,"state_known":true,"changed_files":[]}`，错误
  "修改目标不是常规文件（拒绝 FIFO/设备/目录等非常规目标）"；FIFO 本体原样保留。

### exec_command 真实业务结果（同上取回）

- 双流＋退出码：`{"status":"completed","exit_code":7,"output":"out-S6\nerr-S6\n","truncated":false,"error":null}`。
- 工作目录：`pwd` 输出即指定 workdir。
- 超时（400 ms，组长 `$$`＋后台孙进程 `$!` 写日志）：`{"status":"timeout","exit_code":null,"error":"执行超时（400 ms），已终止进程组"}`；
  宿主外部独立核验：日志已写入（运行过），孙进程进入 **Z 终态**（SIGKILL 已送达、
  进程已终止）。`kill -0 -PGID` 仍报存在仅因僵尸未回收——容器 init 为 `sleep`，不回收
  孤儿（环境特性，非产品缺陷；测试内 `wait_group_terminated` 以 ESRCH 或 Z/X 终态为
  终止判据，与此一致）。僵尸无法从用户态清理，如实保留。

### 补充重跑（与 E2E 分开统计，不混计）

| 用例 | 结果（退出码） |
|---|---|
| `second_file_write_failure_reports_partial_state`（EFBIG 半写） | 1 通过（0）：第二目标真实半写（前 4096 字节前缀）、第一目标完成、清单仅含第一项、`state_known=false` |
| `f2_swap_barrier_binds_write_to_validated_inode`（单元级，F2 屏障） | 1 通过（0）：对象替换窗口守护仍生效 |

### 数量与退出码汇总

- JSON 宿主 E2E：2 项定向，2 通过 / 0 失败 / 0 跳过 / 0 未运行（各退出码 0）。
- 补充用例：2 通过（不计入 E2E）。
- `cargo check`：通过、0 警告（0）。
- `cargo test --workspace --no-fail-fast`：**157 通过 / 0 失败 / 0 跳过**（lib 70＋
  apply_patch 31＋e2e 7＋exec 9＋llm_flow 40），退出码 0；与基线 157 一致，
  重复运行未累计为新增测试。失败：无（未出现需区分产品/脚本/环境原因的失败；
  僵尸滞留为环境观察项，非失败）。
