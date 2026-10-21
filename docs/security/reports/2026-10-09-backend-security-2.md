# 后端安全攻防测试报告（2026-10-09 第二轮）

> 范围：terminal-chat 后端全部真实接口——REPL 会话与内存历史（main.rs）、005 提示词组装（prompt.rs＋assets/codex/prompt.md）、006 工具调用循环（turn.rs::run_tool_turn）、出站客户端（api.rs::chat_with_body/list_models/sync_public）、授权门与配置凭证（catalog.rs）、exec_command（exec.rs）、apply_patch（apply_patch.rs）、显示边界（display.rs::sanitize_for_terminal）。
> 方法：先攻击者视角代码审查，再实弹攻击：真实 CLI 二进制（/workspace/target/debug/terminal-chat，cargo build --locked 产物）管道/PTY 模式＋本地 mock HTTP（记录全部请求与 Authorization 头）；库级直调经独立 harness（path 依赖 /workspace，隔离 --target-dir）。全程假 Key（sk-fake-attack-0001）、隔离临时目录、非 root（uid 1000）、不触网、不读写真实用户目录。
> 上一轮（2026-10-09-backend-security.md）F1/F2 已修复并回归；本轮为增量攻防，重点覆盖新增面 005/006 与八类映射，未修改任何业务代码。

## 〇、概念映射（八类 ↔ 本项目真实面）

本项目无订单/支付/多租户/Web 服务，映射不上的如实标 N/A，不编造：

| 类别 | 本工程对应面 | 本轮结论 |
|---|---|---|
| 1 数据越权 | 工具对任意路径读/写/删；file: 凭证路径逃逸 | 路径无限制是 003/004 规格明示设计，边界＝OS 权限（T6 实测：/etc/shadow 读写均 EACCES；root 外文件按 uid 判定）。file: 逃逸（绝对路径/软链接/`..`）全部拦截（T6c）。见 N8 信任模型声明 |
| 2 功能越权 | 凭证外发授权门（origin＋指纹双匹配）；公共目录来源授权 | 全部伪造失败：非交互 fail-closed（T1a/T2e）、预置正确授权才放行（T1b/T2f）、损坏记录放行失败（T2c）、并发竞争仅丢 grant 不增益（T2g） |
| 3 身份认证 | 出站凭证授权（无登录/token 体系） | userinfo/大小写/scheme/端口归一化后不存在混淆（T2d）；302 重定向拒绝且攻击端零请求（T2a）；换 Key 拒绝（T2b）；异常 IPv4 拼写归一到同一回环目的地（N7，无绕过） |
| 4 偷改字段提权 | 工具入参未知字段、manual.json 未知字段、request_params 保留字段、tool_calls 伪造结构 | 全部拦截：未知参数不执行（T3b）、manual deny_unknown_fields（既有回归）、messages/tools/model 保留字段发请求前拒绝（T3c1-3）、五种伪造 tool_calls 结构零副作用（T3d1-5）、未知工具名不执行（T3e） |
| 5 恶意输入与注入 | 命令注入（工具本体即执行命令）、补丁格式注入、终端转义注入、.env 信任根注入、模型恶意响应驱动工具 | 命令执行是功能本体，边界＝OS 权限与超时/输出上限（但上限本身可被恶意模型改写，见 N2/N3/N4）；Move to 拒绝（T4e）、NUL 拒绝（T4h）；OSC/CSI/DCS 滤净（T4b）；**CR(0x0D) 穿透显示边界（N1）**；.env 信任根劫持失败（T4d4）；rm -rf/工作区外写在隔离目录实测确认后果（T4f/T4g，按设计） |
| 6 绕过前端限制 | REPL 即"前端"，库函数/管道为直连通道 | 管道模式（无终端）授权门同样 fail-closed（T1a）；空白输入 REPL 与库层同拒（E2）；request_params 保留字段在库层同样前置拒绝（T3c）；协议结构校验在循环层完成（T3d）——REPL 与库层校验一致，无仅前端校验项 |
| 7 敏感信息泄露 | 错误消息/Debug/审计/终端输出的 Key 与指纹、授权记录内容 | 401 整串回显脱敏且跨 300 边界无片段（T4c1/c2）；Debug 输出 `***`（T8）；授权记录仅 origin＋16hex 指纹、0600/目录 0700（T8）；交互询问只露指纹前 8 hex（T2f）；**部分回显（非整串）不被脱敏（N9，低）**；程序自身无请求日志 |
| 8 资源滥用与防刷 | 每轮 8 次请求上限、exec 超时/输出上限、10MiB 响应上限、巨型补丁/文件、无限 tool_calls、并发授权 | 8 次上限成立（T5b）；10MiB 响应上限成立（T5f）；**但：单批调用数无上限（N3）、timeout_ms 无上限且循环不接取消标志→REPL 可被无限挂住（N2）、max_output_chars 无上限→内存/出站体放大（N4）、apply_patch 读入无大小上限→31 倍内存放大（N5）** |

另：工作区 `db/wiki.db`、`fpv.rs`、`port.txt` 与后端无关（源码零引用，系工具产物/上轮遗留），范围外。

## 一、发现清单

### N1【低】显示边界漏掉 CR(0x0D)：模型回复/工具通知/错误消息可将原始回车送入终端，可实施行首覆盖伪造
- **后果**：`sanitize_for_terminal` 剥离全部 ESC 序列与大部分控制字符，但 0x0D（CR）不在白名单也不在危险名单，原样放行。真实终端上 CR 使光标回到行首，后续文本覆盖该行开头——攻击者（恶意/被入侵的模型服务）可用"正常文字\r伪造文字"改写 `Agent：`/`错误：` 行的可见内容（如把失败提示伪造成成功转账确认）。仅限当前行覆盖，无转义序列执行能力（ESC 已滤净）；管道/日志场景残留原始 CR 字节。三条输出路径实测全中：最终回复、`[工具]` 通知行（工具 id 内嵌 CR，`trim()` 只去首尾）、401 错误消息回显。
- **根因**：src/display.rs `is_dangerous_control`（约 33-39 行）的 C0 范围漏 0x0D：`'\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}'`——放行了 \n \t 之外的 0x0D。与模块自述"其余 C0/C1 控制字符一律剥离"不符。
- **复现**（/tmp/attack-1010，实测）：
  1. mock 返回 `content:"金额确认：100 元\r已被替换-PWNED"` → CLI（管道模式）stdout 含原始 `0D` 字节 1 个、ESC 0 个（out/t4a.out，`cat -A` 显示 `…100 元^M已被替换-PWNED`）。
  2. mock 返回 tool_calls，`id:"AAAA\rBBBB"` → stdout `[工具] exec_command（AAAA^MBBB）`（out/t4a2.out）。
  3. mock 返回 401 `error.message` 含 `\r` → 错误行含原始 CR（out/t4c3.out）。
- **建议**：`is_dangerous_control` 补 `'\r'`（或在主循环白名单外整体剥离），补一条 CR 回归测试。

### N2【中】恶意模型响应可用 `timeout_ms=u64::MAX` 无限挂住 REPL，工具循环未接入取消标志
- **后果**：一个 tool_calls 即可把会话钉死（u64::MAX ms ≈ 584M 年）。REPL 无应用内取消路径；唯一逃生是 SIGINT/SIGKILL 杀掉整个 CLI（交互模式 Ctrl+C 有效，实测退出码 130），当前轮次与内存历史全部丢失。exec 内部的取消机制（cancel 标志）存在且可用（库级实测 0.31s 内取消），但 `turn.rs::execute_tool` 调 `exec_command(&v, None)` 未传；apply_patch 无取消概念。
- **根因**：exec.rs `positive_int` 只要求正整数、无上限；turn.rs 不传 cancel；main.rs 在 handle_input 期间无中断机制。
- **复现**（实测，out/t5c.out）：mock 返回 `exec_command {"cmd":"sleep 600","timeout_ms":18446744073709551615}` → 4 秒后 CLI 进程仍在（`ps` 可见 `/bin/sh -c sleep 600` 子进程），stdout 仅 1 条工具通知后静默；`kill -INT` 后 CLI 退出码 130。对照组（harness execcancel，cancel 300ms 置位）：0.31s 返回 `{"status":"cancelled",…}`。
- **建议**：给 timeout_ms 设上限（如 60s/300s）或由宿主注入固定超时；工具循环接入取消标志（REPL 在执行期间响应 Ctrl+C）。

### N3【中】单批 tool_calls 数量无上限：8 次请求上限不约束调用条数，恶意服务可一次推送任意工作量
- **后果**：每个模型响应可携带任意多条合法调用，全部串行真实执行（各自默认超时 10s、可写/删文件、可联网之外的任意本地副作用）。8 次"请求"上限语义上防的是对话轮数，不是执行量：一次响应即可推送成千上万条调用。实测 300 条全部执行（3.8s、300 个文件落盘、仅 2 次模型请求）；配合 N2（每条 timeout_ms 巨大）或大体量参数（N4/N5）可放大为长时间占用/存储耗尽。
- **根因**：turn.rs `parse_tool_calls`/`run_tool_turn` 对批大小无限制，串行 for 循环逐条执行。
- **复现**（实测，out/t5a.out、t5a.log）：mock 单响应含 300 条 `touch f{i}.txt` 调用 → work 目录 300/300 文件生成、mock 仅收到 2 个请求、回复 `Agent：300 done`。
- **建议**：对单批调用数设上限（如 32），超限按协议错误整批拒绝；或在执行间检查取消/时间预算。

### N4【中】`max_output_chars` 无上限：恶意模型可用命令输出放大客户端内存与出站请求体
- **后果**：模型可控该参数（工具 schema 里就是普通 integer）。实测 `max_output_chars=2e8` + 60MB 命令输出 → CLI 进程 VmHWM 峰值 293MB（约 5 倍放大：TextSink 60MB＋历史字符串＋第 2 次请求 JSON 序列化缓冲），且 60MB 工具结果整体进入出站请求体（无出站大小上限）；更大输出可直达 OOM。对照组（默认 10000 上限）峰值仅 7MB、truncated=true。
- **根因**：exec.rs `max_output_chars` 仅要求正整数、无上限；api.rs 无出站请求体大小限制；turn.rs 将完整工具结果原样入历史回传。
- **复现**（实测，out/t5d3.log 第 2 请求、/proc VmHWM 轮询）：恶意参数运行 `VmHWM=293MB status=completed output=60000000字符 truncated=False`；默认对照 `VmHWM=7MB output=10000 truncated=True`。
- **建议**：为 max_output_chars 设硬上限（如 1MB 码点）；出站请求体超限时截断工具结果并标注。

### N5【中】apply_patch Update 读入无大小上限：95MB 文件实测 3GB 峰值内存（约 31 倍放大）
- **后果**：模型可用一条两行补丁（纯追加块，无需上下文匹配）指向任意**当前用户可读**的大文件（大日志、数据集、备份），`read_to_end` 全量读入后按行重建（每行一个 String）＋整文件克隆＋重写缓冲，实测 95MB（2000 万行）→ VmHWM 3001MB。本机 14GB 内存下放大可控，但指向数 GB 文件或并发场景即 OOM；且目标文件被真实改写（追加 tail-line，实测落盘）。
- **根因**：apply_patch.rs Update 分支 `read_to_end` 无大小上限；`SourceFile::parse`/`line_texts`/`apply_replacements` 多次全量复制。
- **复现**（实测，out/t5e.log）：95.37MB big.txt + `*** Update File: big.txt @@ +tail-line` → `VmHWM=3001MB ok=True changed=['big.txt']`，文件尾行为 `tail-line`。
- **建议**：Update 前对目标文件大小设上限（如 16/64MB，超限拒绝并建议整文件重写策略），与 exec 输出上限对齐。

### N6【低/观察】并发授权写入存在读-改-写竞争，可能静默丢失一条授权（fail-safe 方向）
- **后果**：两个本地进程同时首次授权不同 origin 时，后写者覆盖前写者：实测 2 条并发 grant 后文件只剩 1 条（文件可解析、无混合/伪造条目）。丢失的是"已同意的授权"——后果是下次请求重新询问用户，不产生未授权放行；已知项（上轮 AUTH-B05 调查），本轮实测复现记录。利用前提是攻击者已能本地跑进程（此时已无边界可守）。
- **复现**（实测，env-t2g/user/authorized-origins.json）：两 harness 进程并发 grant 19101/fpA 与 19102/fpB → 仅存 `http://127.0.0.1:19102 fpB` 一条。
- **建议**：低优先；如修复可加文件锁或原子合并写。

### N7【信息】origin 归一化行为澄清：异常 IPv4 拼写归一到 127.0.0.1（同目的地，无绕过）
- 实测 `origin_of`：`http://0x7f000001:18110`、`http://127.000.000.1:18110` → `http://127.0.0.1:18110`（WHATWG IPv4 宽松解析归一）；`validate_base_url` 对二者放行明文 http（仍在本机回环语义内，且实际连接目的地就是归一后的 127.0.0.1，授权按归一后 origin 匹配——不可用于把凭证导向其他主机）。`http://localhost.:18110` 不在豁免名单（拒绝 http，需 https），`[::ffff:127.0.0.1]`、`0.0.0.0`、`127.0.0.2` 均拒绝明文 http。结论：归一化只会把拼写折叠到同一实际目的地或变得更严格，未发现跨目的地混淆。
- 证据：out/ 无独立文件（T2d 控制台输出，命令与输出已录入本节）。

### N8【信息/设计确认】模型驱动工具的信任模型：当前 OS 用户的全部权限
- 实测：恶意模型响应驱动 `rm -rf <隔离树>` 成功清空（T4f）；驱动 apply_patch 以绝对路径在 REPL 工作区外创建文件成功（T4g，/tmp/attack-1010/outside-pwned.txt）；exec/apply 可读写删工作区外同 uid 文件（T6）。003/004 规格明示"工作目录不构成权限隔离，权限由执行环境控制"，授权门只管凭证外发、不管工具本地作用域。**这不是缺陷而是声明的信任模型**：一旦接受了某个模型的 tool_calls，该模型即等价于本 shell 用户（含读 ~/.ssh、改启动脚本等）。缓解责任在使用者侧（容器/专用账号运行）。列入报告以明确边界。

### N9【低/信息】错误消息脱敏只做整串匹配：服务端回显"缺末字符"的部分 Key 可穿透
- **后果**：`redact_credential` 用整串替换。服务端若回显 `sk-fake-attack-000`（缺最后一位）→ 实测原样显示在错误行。该信息只流向用户自己的终端/日志；构造它的服务端本已持有完整 Key，故机密性影响有限（终端被旁观/录屏、日志外发场景才有意义）。300 字符截断同样不做部分匹配。
- **复现**（实测，out/t4c4.out）：401 `error.message="你的密钥 sk-fake-attack-000 无效"` → 错误行含 `sk-fake-attack-000`（grep 命中 1）。
- **建议**：如需收敛，可对 Key 的长前缀（如前 2/3 长度）做遮蔽；优先级低。

## 二、通过项（实弹攻击未突破）

| # | 攻击 | 输入/构造 | 实测结果 | 证据 |
|---|---|---|---|---|
| P1 | 非交互未授权外发凭证 | 管道模式无授权记录 | `非交互模式…已拒绝发送`，mock 0 请求，退出码 0 | t1a-nogrant.out |
| P2 | 授权后正常放行 | 预置 (origin, fp) 双匹配 grant | 1 请求带 `Bearer sk-fake-attack-0001`，`Agent：基线回复` | t1b.log |
| P3 | 302 重定向投递 | 授权 A 返回 302→攻击者 B | `拒绝跟随重定向（302）`，B 收 0 请求 | t2a-302.out、t2a-b.log |
| P4 | 换 Key 复用旧授权 | grant=fp(keyA)，env=keyB | 拒绝，0 请求 | t2b-rekey.out |
| P5 | 损坏授权记录放行 | `{broken json` | fail-closed 拒绝，0 请求 | t2c-corrupt.out |
| P6 | origin 伪造（userinfo/大小写/scheme/端口/路径垃圾） | origin_of 探测 11 种 URL | userinfo 剥离、大小写/scheme/端口各成独立 origin，无跨目的地混淆；异常 IPv4 拼写折叠到同一回环（N7） | T2d 输出 |
| P7 | /refresh 非交互越权 | 管道模式 /refresh | 公共目录未授权跳过＋厂商查询拒绝，0 网络 | t2e-refresh.out |
| P8 | 交互授权闭环 | PTY 回答 y / n | y→写 0600 记录＋发请求；n→无记录 0 请求 | t2f-y.txt、t2f2-n.txt |
| P9 | 工具入参未知字段 | `{"cmd":"touch…","extra":1}` | `未知参数：extra` 结果回传，文件未创建 | t3b.out、t3b.log |
| P10 | tool_calls 伪造结构×5 | 缺 id / type=custom / 批内重复 id / arguments=5 / content=数组 | 全部 `协议错误` 整批拒绝，零副作用文件 | t3d1-5.out |
| P11 | 未知工具名 | `web_search` | 不执行，`未知工具` 结果回传模型 | t3e.log |
| P12 | request_params 保留字段 | messages / tools / model | 发请求前拒绝，0 网络 | t3c1-3 |
| P13 | 未声明能力参数 | temperature 无 supported_params | `能力未知` 拒绝，0 网络；声明后正确进入请求体且 model 不被覆盖 | t3c4、t3c5.log |
| P14 | OSC52/CSI/DCS 终端注入 | 恶意回复内嵌全套序列 | stdout ESC 字节 0 个，前后文字保留 | t4b.out |
| P15 | 401 整串回显 Key＋跨 300 边界片段 | `X×290+Key` / `prefix Key suffix` | 均替换为 `***`，无整串无前缀 | t4c1/c2.out |
| P16 | .env 信任根劫持（回归 F1） | .env 注入 TCHAT_USER_DIR（含小写变体）/HOME/TCHAT_PUBLIC_CATALOG_URL→攻击者目录（含有效指纹授权） | 全部跳过：拒绝消息指向真实 HOME 路径，0 请求；对照组显式环境变量正常生效 | t4d4.out、t4d3-ctrl.out |
| P17 | 补丁 Move to 注入 | `*** Move to: …` | `MVP 不支持 *** Move to`，无文件移动 | t4e.log |
| P18 | cmd 含 NUL | `"touch ok.txt\0rm -rf /"` | `进程启动失败：nul byte found`，未执行 | t4h.log |
| P19 | apply Add 已存在目标（/dev/null） | Add File /dev/null | `新增目标已存在`，create_new 拦截 | T6 输出 |
| P20 | file: 凭证逃逸 | `../`、绝对路径、软链接指向目录外 | 全部 `必须位于专用凭证目录…已拦截`；目录内合法引用可用 | T6c 输出 |
| P21 | exec/apply 读 root 专属文件 | cat /etc/shadow、Update /etc/shadow | EACCES（OS 权限边界生效） | T6a/b 输出 |
| P22 | 8 次请求上限绕过 | 恒返 tool_calls | 恰 8 请求后 `已达本轮模型请求上限（8）`，第 8 批结果保存、不发第 9 次 | t5b.out、t5b.log |
| P23 | >10MiB 响应体 | mock 返回 10MiB+100B | `响应体超过 10 MiB 上限，已中止读取` | t5f.out |
| P24 | 空白输入（REPL 与库层） | 空格/Tab 行；run_tool_turn("   ") | REPL 不发请求；库层 `当前用户输入不能为空或全为空白`，sent=0 | e2-blank.out |
| P25 | Debug 输出泄 Key | `{:?}`/`{:#?}` 打印含假 Key 的 Credential | `value: "***"` | T8 输出 |
| P26 | 授权记录内容/权限 | 落盘文件 | 仅 origin＋16hex 指纹＋时间戳；0600 文件、0700 目录 | env-t2f/user/ |
| P27 | 凭证读取含控制字符 | check_key_header_safe 路径 | （既有回归覆盖；本轮 401/网络错误路径无 Key 泄露复核一致） | t4c* |

既有自动化回归（`cargo test`，151 项）覆盖且本轮未变化的点直接引用：授权门语义（llm_flow.rs）、apply_patch 权限/链接/EFBIG（apply_patch.rs 测试）、F2 非常规文件拒绝与 inode 绑定（f2_swap_barrier_*）、exec 参数校验与进程组终止（exec_command.rs）。

## 三、与上轮报告的差异（新增面 005/006 结论）

1. **005 提示词组装（prompt.rs）——未发现可利用面**。实弹捕获首个出站请求体（t7-request-baseline.log）：恰 1 条 system（16928 字符，为发送版：update_plan 节已剔除、apply_patch 示例已改写为 `{"patch":…}`、AGENTS.md 声明已改写），tools 恰为 exec_command＋apply_patch 且 `additionalProperties:false`；工具定义白名单在客户端侧只影响模型可见性，真正执行白名单在 turn.rs 分派层（P11）。母本 assets/codex/prompt.md 经 `include_str!` 构建期内置且 git 状态干净——运行时无文件读取，无提示词文件注入面；供应链面仅存在于仓库提交层面（范围外声明）。
2. **006 工具调用循环（turn.rs）——结构性校验稳固，但资源语义有四个洞**：协议结构伪造全部失败（P10/P11，五种变体零副作用、历史不污染）；第 2 次请求结构正确（system×1、user×1 不重复、assistant 原样、tool 结果回传，t3b.log）；但"每轮 8 次请求"上限不约束调用条数（N3）、不约束单调用资源（N2/N4），apply_patch 大文件读入放大叠加（N5）。上轮报告的 F3 边界观察（"timeout_ms=u64::MAX 无 panic 正常执行"）在本轮推进为完整攻击链：配合 sleep 即无限挂住（N2）。
3. **REPL 会话历史（内存态）**：多轮间完整保留并整体重发（含工具结果原文，最长为 exec 输出上限——但上限可被恶意模型改写，见 N4）；exit 即丢弃，无落盘泄露面。
4. **api::chat_with_body**：保留字段检查（messages/tools/model）在发送前生效（P12/P13）；重定向拒绝、错误脱敏、10MiB 上限与 chat 一致（P3/P15/P23）。未发现绕过。
5. 上轮 F1/F2 修复回归通过（P16、P19/T6），无回退。

## 四、修复优先级建议（本轮只测不修）

1. **N5**（一行文件触发 31 倍内存放大）＋ **N4**（exec 输出/出站体放大）：给工具参数与目标文件大小设硬上限。
2. **N2**（REPL 无限挂住）：timeout_ms 上限或接入取消标志。
3. **N3**：单批调用数上限。
4. **N1**：`is_dangerous_control` 补 `'\r'`（最小改动）。
5. N6/N9 低优先；N7/N8 为边界声明，建议写入 README/AGENTS 信任模型说明。

## 五、材料位置

- 攻击材料根目录：`/tmp/attack-1010/`（一次性，未入 docs）。
  - 脚手架：`env.sh`（隔离环境/授权预置/CLI 管道运行）、`mock.py`（记录型本地 mock）、`mkresp.py`（响应构造）、`pty_auth.py`（PTY 交互授权）、`resp-*.json`（响应脚本）。
  - 库级 harness：`harness/src/main.rs`（子命令 cred/origin/grant/ensure/validate_url/sanitize/dotenv/debugcred/chatbody/execfile/applyfile/execcancel/turn；构建于 /tmp/attack-1010/target，隔离 --target-dir）。
  - 全部原始输出：`/tmp/attack-1010/out/`（167 个文件：`*.out`＝CLI stdout、`*.log`＝mock 请求 JSONL、`t2f-y.txt`/`t2f2-n.txt`＝PTY 捕获、`t7-request-baseline.log`＝出站请求体基线）。
- 本报告引用的隔离环境目录：`/tmp/attack-1010/env-*/`（配置/授权/工作区副本）。
- 复现总入口：`source /tmp/attack-1010/env.sh` 后按本报告各条"复现"小节的构造重放；假 Key `sk-fake-attack-0001`（指纹 fe2a4d6d54375db0）。

## 修复记录（2026-10-10）

N1–N5 已全部修复并验证（每项真实 RED→最小修改→GREEN，攻击场景重放通过）：见
logs/2026-10-10-security-fix-n1-n5.md。修复轮 223/0/0；验收轮补强（混合批分形、n2a 恢复、生成字节边界、
N2b 合格 RED）后终态见修复日志文末，cargo check 0 警告。
固定边界已写入 003/004/005/006 与 AGENTS.md。N6/N9 未在修复范围。
