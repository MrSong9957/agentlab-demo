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
  风险＝ReOpenFile 仅删一个常量组合（签名对照 registry windows-sys-0.59.0
  核实）＋G2 测试字符串拼接（非产品代码）。
- Windows 腿最终判定待 CI（workflow_dispatch→PR）：预期 ≤3（决策② 2 项
  ＋#12 待取证），0xC0000142 瞬态若现按批 C 口径识别。
