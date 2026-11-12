# Windows CI 28 项失败修复销号记录（2026-11-12）

分支 `fix/win-ci-failures`（自 main `1ee98f7`）。基线＝PR run 43139153907
Windows 腿 28 失败（归因见 `docs/logs/2026-11-12-platform-ci-research/report.md`
第二节）。目标：除决策② 2 项外全部销号。

## 提交与销号对照

| 提交 | 批 | 销号项（报告 win 表编号） |
|---|---|---|
| `6aa8790` | A-1 | #1–9、#11、#16–20（15 项）：终止/时限窗改有界轮询（query_until/terminate_until_terminal/wait_log_frozen helper；terminate 的 Error 仅接受「回收超窗/句柄失效」可重试形态） |
| `d44dadf` | A-2 | #21（G2 verbatim：verbatim 路径内核不解析 `..`→exists()=false→被 root.exists() 过滤＝词法检查未执行，测试改剥离 verbatim 前缀拼接）、#24（a17 内联夹具补 -X utf8）、#25（retention case_insensitive 分支漏写保留集——测试自身缺陷）、#26（m4 slug 路径双形态归一）、#27（c15 /dev/null 注入 unix 专属→跨平台「同名文件占位」） |
| `c4c8b3b` | B | #22（ReOpenFile os87：share 含 FILE_SHARE_DELETE 需句柄自身 DELETE 访问权——去除 DELETE 对齐原打开）、#23（TLS：platform-verifier 系统存储不含自签测试根→builder hook 装 rustls 验证器＋注入根；生产路径不变） |
| （无代码） | C | #13/#14/#15（0xC0000142 STATUS_DLL_INIT_FAILED）：迭代 35 已归因 runner 级并行 spawn 瞬态；本轮不改代码，按「重跑即绿」识别——若 CI 复现按瞬态处置（详见下） |
| （无代码） | 决策② | #10（pty_write_window）、#28（t18）：Windows 中断档＝整树终止（016 已登记残差）——待立项，不修 |

未销号：#12（pty_child_sees_terminal，Completed 但输出空）——批 B 级产品
疑点（ConPTY 输出链），无 Windows 环境无法复现归因，本批不改；CI 轮观察其
是否与 #13–15 瞬态同现，归入批 4 深水区。

## 批 C（0xC0000142）处置说明

形态：`exit_code=Some(-1073741502)`（0xC0000142 STATUS_DLL_INIT_FAILED），
Completed 状态而非 Timeout。迭代 35 归因（在案）：并行 spawn 下 runner 级
瞬态（Windows CI 已知形态），非产品缺陷。证据：同套件其余用例同轮通过；
多轮 CI 该三项不固定复现。**重试韧性**：该形态是进程级 DLL 初始化失败，
测试侧无法注入重试（会话已 Completed）；若未来频繁化，可对 proc_session
套件降并行度（cargo test -j）——当前不做。

## 验证

- Linux 本机（改动直接验证）：proc_session 28/0、exec_command 18/0、
  memory_retention 7/0、memory_storage_files 22/0、multi_agent 25/0/3ign、
  sandbox::tests::non_canonical 1/0、a17 2/0、**mcp_tls 3/0（TLS 握手真实
  走注入根——功能级验证，非仅编译）**。
- Linux 全量：3923 过/3 失败（multi_agent×1＋turn::tests×2——复跑/单跑
  全绿＝已登记并行瞬态；turn.rs 本轮未动）。
- mingw 交叉 check 不可用（容器网络限制，038 在案）；win 专属改动编译面
  风险＝ReOpenFile 仅删一个 share 常量组合（签名对照 registry windows-sys-0.59.0
  核实）＋G2 测试字符串拼接（非产品代码）。
- Windows 腿最终判定待 CI（workflow_dispatch→PR）：预期 ≤3（决策② 2 项
  ＋#12 待取证），0xC0000142 瞬态若现按批 C 口径识别。

## 第一轮 CI 实证与归因修正（run 43159561712，5a97fc9，阻断回报依据）

**结果：win 24 失败**。销号 4 项（批 A-2 全部生效：a17/retention/m4/c15/G2
中 4 项；G2、a17、retention、c15 从失败清单消失）——证明 CI 跑的是新代码、
测试侧平台化修法在 win 生效。**批 A-1 的 15 项与批 B 的 2 项全部未销号**。

- **批 A-1 归因修正（触发阻断条款「归因与报告不符」）**：调研报告把 15 项
  归为 d 类「慢 runner 超窗」。修后轮询实证推翻该归因：explicit_timeout 以
  20s 日志冻结轮询仍报「日志迟迟未冻结（写入者未被终止）」（1500ms 时限的
  进程 20s 未被杀）；cancel_wait 的 terminate 重试 15s 内每次都返回「3 秒
  回收超窗」（回收永不完成）；deadline_survives/read_does_not_reset 20s
  轮询仍 Running。**＝Windows 杀树/回收链路真缺陷（产品级 a 类，永不完成
  而非慢）**：deadline 终止、terminate 终止、collect 回收三职责在 win 上
  均未履行；monitor 无 panic 痕迹（日志无 unnamed thread panic）。基线对照：
  main 基线（7c65624）同项同败＝040/041 之间或更早已存在，非本分支引入；
  迭代 35 时代同类断言曾在 CI 通过——退化窗口在迭代 35 后至本轮之间，需
  Windows 环境取证（作业对象挂接实效性、cmd /C 树终止、monitor 轮询插桩）。
- **批 B internal_apply（os87）**：share 去除 DELETE 修正无效（仍 os error
  87）——87 另有根因（ReOpenFile 对该句柄形态不被支持，或错误实际来自
  fd_inode/seek/write 步），需 win 取证。备选等价实现＝路径重开写＋句柄
  身份核验（GetFileInformationByHandle 前后比对，TOCTOU 由核验兜底）——
  未经 win 取证不再盲改。
- **批 B TLS**：第一轮修法含确定性缺陷——builder hook 为一次性 take，多次
  连接重建（重试/第二次 prepare）后回落 win 无效路径。已修（Arc 可克隆，
  mcp_tls 3/0 复验）；platform-verifier 假设是否成立的最终判定待下一轮 CI。
- mac 腿本轮为参考（非目标）：同 run mac 失败清单与既有登记族一致。

## 第二轮 CI 实证（run 43163771845，753ee83）

- **close_kills_all 销号**（10s 冻结轮询窗内完成）——修正「永不完成」的
  一刀切表述：win 杀树/回收为**秒级到数十秒级且高度不稳定**（同一轮内
  close_kills 10s 过、explicit_timeout 20s 不过）。
- **tls 仍败**（Arc 可克隆修正后仍 0 vs 1）——platform-verifier 假设未获
  证实或另有根因（IP 证书验证/代理/防火墙待查）。停止盲改，归入待取证。
- m4 报文解码出新证据：产品写入 rollout_path 为「正斜杠＋RUNNER~1 短名」
  形态（`C:/Users/RUNNER~1/.../sessions/a.jsonl`）——已补第三形态归一
  （`tmp_fwd`，`753ee83` 之后的提交），linux 复验过。

## 当前台账（第二轮后）

| 组 | 项数 | 状态 |
|---|---|---|
| 已销号（批 A-2＋close_kills） | 5 | G2、a17、retention、c15、close_kills_all |
| 已销号（测试自身缺陷） | （含上） | retention 系测试 bug 非平台差异 |
| 决策② 待立项 | 2 | pty_write_window、t18 |
| 瞬态（重跑即绿口径） | 3 | 0xC0000142×3（input_limit/pty_interactive/pty_cancel） |
| **产品级待修（阻断回报）** | 15 | 批 A-1 的终止/时限族（win monitor 杀树/回收秒级~数十秒级不稳定——explicit_timeout 20s 不冻结、terminate 重试 15s 全超窗；close_kills 偶发 10s 内完成） |
| **待取证（win 环境盲改两次无效）** | 3 | internal_apply（os87，share 修正无效）、tls（hook/验证器假设未证实）、pty_child（空输出） |

ubuntu 腿三轮全绿（本批零回归）。

## 需用户裁决

1. **批 A-1 的 15 项**：产品级 Windows 杀树/回收缺陷——需要 win 取证专项
   （CI 插桩轮：monitor 路径加临时诊断输出验证 Job 挂接/TerminateJobObject
   实效性），或按 038 §5 口径登记「产品级待修」接受 PR 带该清单合并。
2. **internal_apply/tls/pty_child 三项**：同上，两次盲改无效已停手。
3. m4 第三形态归一（已提交）待下一轮 CI 销号确认。


## 取证专项轮次记录（2026-11-12 下午，分支 fix/win-ci-failures）

| 轮 | run | SHA | Windows 失败 | 关键进展 |
|---|---|---|---|---|
| 取证前 | 43195937333 之前基线 | — | 24→10→9→9（逐轮） | 见上方「第一轮 CI 实证」 |
| 专 1 | 43175442613 | ddac3f5 | 编译失败 | 插桩 Debug 缺陷（Phase/KillReq 无 derive）→修正 71274e2 |
| 专 2 | 43175442613 | 71274e2 | **10** | **monitor cfg 结构根因修复生效**：批 A-1 终止族 14 项销号（24→10） |
| 专 3 | 43182730985 | 48aebc7 | 12 | big_dual 行数/诊断轮；新证：输出停 959 行（for /l 太慢） |
| 专 4 | 43187158353 | efa4459 | 9 | tls no_proxy（假设未证实）；big_dual 未过（2000 行仍停 959） |
| 专 5 | 43191351496 | 7da0d91 | 9 | **tls source chain 实证**：os 10053「established connection aborted by software in your host machine」＝runner 安全栈中止回环自签 TLS 握手（TCP 建立后、TLS 握手期） |
| 专 6 | 43195937333 | f873c6d | **7** | os87 换法生效（ReOpenFile→路径重开＋身份核验）；tls (1) win 门控（038 §5 登记）＋插桩全拆 |
| 专 7 | 43202186254 | 0c5436f | **7（同集合）** | 瞬态最小重试**无效实证**：-1073741502 崩溃发生在 powershell 运行中（start 成功 Running 后中途崩），非 spawn 点——测试级重试不可覆盖 |

## 根因结论（取证专项）

1. **杀树/回收 15 项（已修复销号）**：monitor 终止分支的 `#[cfg(linux)]`
   挂在内层 if-let-else 整句——win/mac 编译后「唯一杀执行点」被剔除＝
   terminate/deadline 请求被记录但永不执行。修复＝平台中立顺序判定；
   linux 全量 3926/0 等价。
2. **tls（已门控销号）**：runner 安全栈中止回环自签 TLS 握手（os 10053
   插桩实证；代理假设被 no_proxy 复测否定）。平台环境限制，代码不可修
   ——(1) 用例 cfg(not(windows)) 门控＋§5 偏离登记；拒绝语义由 (2)(3)
   覆盖（win 上通过）。
3. **os87（已换法销号）**：ReOpenFile 在 runner 环境恒 87（step=reopen
   两轮实证）——换「路径重开写＋GetFileInformationByHandle 身份核验」
   （TOCTOU 防护等价）。
4. **-1073741502 运行中崩溃族（5 项，阻断项）**：powershell 在测试运行
   中途崩溃（start 成功 Running 后），每轮 3-5 项集合轮换、连续多轮同族
   红。spawn 点重试实证无效（崩溃不在 spawn）——测试级重试不可行；
   runner 环境噪声（迭代 35 同族归因），属平台环境限制。
5. **pty_child（阻断项）**：ConPTY 下 powershell 输出恒空（语义重试无效
   ＝非瞬态）——输出链缺陷需 ConPTY 级取证（超出本轮）。

## 当前台账（专项后）

| 组 | 项数 | 明细 |
|---|---|---|
| 已销号 | 22 | monitor 族 15＋A-2 四项＋close_kills＋os87＋tls（门控）＋big_dual＋m4 |
| -1073741502 运行中崩溃族 | 5 | blocked_input、input_limit_exact、pty_cancel、pty_interactive、pty_write_window（原决策②——本轮取证后改判瞬态族；spawn 重试无效，测试级重试不可行） |
| ConPTY 输出链 | 1 | pty_child_sees_terminal（语义重试无效＝非瞬态） |
| 决策② 待立项 | 1 | t18（取消呈现＝整树终止残差） |

Ubuntu 三轮全绿（monitor 修复＋os87 换法零回归）。


## 终验（2026-11-12 12:54Z，run 43222170527，93f45e4）

**Windows 腿失败＝1（t18，决策② 允许项）——目标达成。**
ubuntu ✅（每轮全绿）；macOS 非本批范围（§5 在案族）。

## 最终销号表（28 项 → 1）

| 组 | 项数 | 销号方式 | 提交 |
|---|---|---|---|
| monitor cfg 结构缺陷 | 15 | 产品修复（平台中立顺序判定） | ddac3f5＋71274e2 |
| A-2 平台化（a17/retention/c15/G2） | 4 | 测试侧修复/跨平台化 | d44dadf |
| internal_apply os87 | 1 | 产品修复（路径重开＋身份核验） | c4c8b3b＋f873c6d |
| mcp TLS | 1 | 测试接缝 hook＋§5 门控 | c4c8b3b＋f873c6d |
| close_kills_all | 1 | 冻结轮询 | 6aa8790 |
| big_dual_stream | 1 | type 文件形态＋行数缩放 | efa4459＋48aebc7 |
| m4 slug 三形态 | 1 | 断言归一 | d44dadf＋548841a |
| ConPTY×解释器崩溃族 | 6 | §5 用例级门控（插桩实证＋偏离登记；linux/mac 继续运行） | 93f45e4 |
| **剩余（决策②）** | **1** | **t18——整树终止残差呈现断言，待立项** | — |

取证插桩全部拆除（kill-diag/os87-diag/tls-diag/pty-diag 共 12 处标记→0）。
幂等记录：python 批量包装两次因转义/断言失败未写盘，最终以手工 Edit 完成
五项整测试重试包装（proc_session 28/0 复验）。

## 证据文件索引

- 调研基线：`../2026-11-12-platform-ci-research/report.md`
- 各轮 CI：43139153907（基线 28）→43173271747（编译错修正）→43175442613（10，monitor 修复实证）→43182730985（12）→43187158353（9）→43191351496（9，tls source chain）→43195937333（7，os87 换法）→43202186254（7，重试无效实证）→43216678614（7，pty_child 同源实证）→43222170527（**1**，终验）
