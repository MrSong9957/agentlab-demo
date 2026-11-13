# macOS CI 修复台账（2026-11-13，分支 fix/mac-ci-failures）

基线：main `289577c`（含 Windows 专项的 monitor cfg 根因修复——对 mac 同样生效）。

## 基线重建（取证轮 43351426432，空分支 dispatch）

**mac 失败 61→26**（调研报告 61 项中 35 项已随 monitor cfg 修复连带销号）：
proc_session 终止/时限族 15、seam 注入族 6、exec_command 4、attack×3、
memory×2、n2b/e18、env_and_fd(fd5)、matrix_a、perf_tiers、bin_pipe、
resources_filename、a17、tls、p2——**全部不再出现**（终止/回收链路修复的
连带效应＋Windows 批次共享修复）。

**新基线 26 项全部集中在 e2e_acceptance 的 TUI 交互面**（本轮 job 于 e2e
后段被 runner 收尾，为不完整观测；修复轮会补齐）：

| 族 | 项 | 形态 |
|---|---|---|
| 剪贴板右键复制族 | 12 | 「等待屏幕 Copy sent/Copy failed 超时（30s）」，屏幕渲染正常、选区可见 |
| 渲染停滞族 | 3 | s35/s37/s91「等待输出再见! 超时」，捕获仅初始化转义序列 |
| PTY 写/时序 | 3 | s114a（PTY write EIO）、s73（前置未就绪）、s18（/save 恢复超时） |
| 线程计数 | 2 | s71/s72「线程数 0 基线 0」 |
| 路径词法 | 1 | s6 `/var` vs `/private/var` |
| 连接/清理 | 5 | s103×2、s113、s23、s91 归渲染族后余 s23 等（含 s20 本轮新观测） |

## 批 A 已修复（提交 6a7d75a，验证轮 43358807346 销号 3 项→余 23）

1. **thread_count 平台分科（s71/s72）**：`/proc/<pid>/task` 为 linux 专属，
   macOS 无 /proc→恒 0（断言必败实证「线程数 0 基线 0」）——mac 分支改
   `ps -M` 采样（行数-表头）；linux 语义不变。
2. **s6 路径归一**：`/var→/private/var` 符号链接——pwd 输出物理形态 vs
   夹具词法形态，断言侧 canonicalize 归一（linux 恒等零漂移）。
3. （同轮）s20 新观测＝runner 集合轮换（非本轮引入——行数缩放轮该测试
   未到执行位）。

## 剪贴板假设否定（重要取证，clip-diag 零输出）

调研报告批 C 假设「无 GUI 会话使 Clipboard::new 挂起→OSC52 永不触达」。
插桩（[clip-diag] 于 Clipboard::new 入口/错误/成功三处）在本轮 **零输出**
——**native_copy 闭包从未被调用**＝Copy sent 超时的根因**不在剪贴板 API**，
在更早的「右键事件→复制指令→UI 反馈」链路。结合渲染停滞族（仅初始化序
列）——**e2e TUI 交互面（输入处理/渲染/指令分发）在 mac PTY 环境存在产品
级系统性缺陷**，需专项深取证（mac PTY 输入回放＋渲染帧 dump，多轮迭代）。

## 当前台账（26→23）

| 组 | 项数 | 状态 |
|---|---|---|
| 已销号（monitor 连带） | 35 | 终止/回收/注入/路径族 |
| 已销号（批 A） | 3 | s71/s72（thread_count 分科）、s6（路径归一） |
| **TUI 交互面产品缺陷** | 23 | 剪贴板反馈链 12＋渲染停滞 3＋时序/PTY 8——**需 mac PTY 专项深取证**（非测试侧可修；clip-diag 已否定剪贴板假设） |
| 决策② | 0 | t18 为 Windows 专属（mac 历轮通过） |

Linux 防回归：全量 3924/2（turn::tests＋multi_agent 各 1——单跑绿＝已登记
并行瞬态）；批 A 改动文件直接验证 e2e s71/s6/s72、mcp_http、llm_flow 全绿。

## 止损与待裁决

剩余 23 项按调研报告属「产品级 mac 缺陷需专项深取证」（调研报告批次 D/深
水区）——超出本轮「测试侧＋已实证修复」范围。**候选 PR 已开**（mac 现状：
23 项 TUI 交互面＋口径说明），三项裁决口径供选择：
①批准 mac PTY 专项取证（多轮插桩迭代）；②按 §5 门控 23 项（e2e_acceptance
的 TUI 交互面在 mac 平台登记，linux/windows 全量不变）；③合并现状（mac
failure 维持、ubuntu 绿、Windows 仅 t18）。
