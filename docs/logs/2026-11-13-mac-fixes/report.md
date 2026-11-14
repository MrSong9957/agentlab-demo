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


## 验证轮与轮换面扩大（2026-11-13，43381070533，3e5b095）

批 A 三项修复在后续轮次**持续有效**（s71/s72/s6 未再出现）。
门控修正轮（cfg 小写修正后）暴露 **runner 机器池轮换的又一批子集（15 项）**：
attack×3、bin_pipe、env_and_fd(fd5)、matrix_a、perf_tiers、pty_cancel、
s103_mcp_reload、s113、s114a、s18、s23、s73、tls——其中：
- **既有轮换族**（s113/s114a/s18/s23/s73/pty_cancel/tls/s103）：基线 26 项
  内已有登记（时序/PTY/连接面）；
- **新轮换面**（attack×3/bin_pipe/matrix_a/perf/fd5）：基线轮通过、本轮
  命中——attack×3 为批次六修复后的**残留路径**（产品侧 path_covers 之外
  仍有 /var 形态敏感点），需产品侧深挖非断言归一。

**止损判定**：mac runner 机器池轮换导致失败子集逐轮漂移（26→22→15→…），
逐批修复速度追不上轮换暴露；attack×3 残留需产品侧深挖。按任务止损线
（连续两轮同项红且无新证据／修复追不上）**停下回报，待裁决**：
①批准 mac 深挖专项（attack 残留路径＋轮换面逐项）；
②按 §5 大范围门控（mac 平台登记 e2e/security/perf 面全部已知不稳定项，
 linux/windows 全量不变）；
③合并现状（mac failure 维持台账，ubuntu 绿、Windows 仅 t18）。

## 提交索引（分支 fix/mac-ci-failures）

6a7d75a（批 A）→ b3f076f/007fbbf/31325c0/c3db79d（取证设施）→ bafeb3a
（mac-diag2）→ 45aa5fb（15 门控）→ 3e5b095（cfg 小写修正）→ fd50208/…
（台账）——PR #7 未合并，等验收。

## 用户裁决（2026-11-13）：mac 收尾口径

13 个 7/7 全红项＝确定性平台分叉，**不适用门控，逐项修**；仅轮换型走
「≥3 轮轮换实证→§5 门控」。attack×3 夹具 canonicalize 可行，但须保住
「越锚被拒」测试意图。修完销号、修不动单独举证停下问。

## PR #7 八轮失败集合轮次表（R1–R7 已完，R8=43394402954 进行中）

| 轮 | run | commit | 失败数 | 备注 |
|---|---|---|---|---|
| R1 | 43363077628 | 007fbbf | 32 | mac-diag 设施轮 |
| R2 | 43366154148 | 31325c0 | 32 | upload 条件修正 |
| R3 | 43369350915 | c3db79d | 32 | ubuntu 也红（插桩轮，已还原） |
| R4 | 43373035232 | bafeb3a | 31 | mac-diag2 四点插桩 |
| R5 | 43377431234 | 45aa5fb | 33 | 15 项门控（cfg 大写失效＝未生效）＋c04 新观测 |
| R6 | 43381070533 | 3e5b095 | 15 | cfg 小写修正，门控生效（剪贴板 12＋渲染 3 消失） |
| R7 | 43384471070 | b94695a | 16 | e12 新观测 |
| R8 | 43394402954 | 6b69a36 | ？ | 批 1（44e126c）＋批 2（7270172）＋取证插桩（6b69a36） |

**13 项 7/7 持续红（确定性，非轮换）**：attack01/02/05、bin_pipe、fd5、
matrix_a、perf_tiers、pty_cancel、s103_reload、s113、s114a、s18、s23、s73、
tls——跨轮报文逐字节稳定（除 pid/tempdir）。

**轮换型计数（R1–R7）**：p2 4/7（R1/R2/R3/R5）、s103_async 2/7（R2/R4）、
s106 2/7（R1/R4）、s69 1/7（R3）、c04 1/7（R5）、e12 1/7（R7）——均未达
「≥3 轮＋族属」门控证据线，继续观察（p2 已够轮次，族属待报文比对）。

## 批 1 修复（44e126c，Linux 定向绿）

1. **attack×3**＝产品「逐组件验证拒绝符号链接组件」×mac 临时目录词法形态
   （/var→/private/var 系统符号链接）：attack01/05 期望 denied/granted 实得
   error、attack02 期望 requestable 归因实得符号链接文案——根因均为越锚请求
   （reqA/outside 是 workspace 锚的兄弟/外部路径）从根起走先撞 /var。
   **夹具 canonicalize 归一**（root_dir 出口，macOS only；linux 恒等零漂移；
   windows 不动防 verbatim 分叉）：全部断言逐字不变，越锚拒绝仍按 scope
   归因（与 linux 同理由），步骤 5 显式 reqA/link 符号链接用例不受影响。
   安全语义（拒绝＋零弹窗＋零授权）不测自 product 行为变化。
2. **bin_pipe**＝环境块 cwd 断言词法 vs getcwd 物理形态：environment_block
   渲染 sources.cwd＝子进程 getcwd() 结果（内核恒返物理路径）——断言侧
   canonicalize 归一（s6 同法，macOS only）。

## 批 2 修复（7270172，Linux 定向绿）

1. **fd5（产品修复）**：js worker spawn 的 fd 收口块挂
   `#[cfg(all(linux, x86_64/aarch64/riscv64))]`——macOS 编译期整块剔除，
   planted 非 CLOEXEC fd 直落 worker（哨兵 7/7）。Darwin 无 close_range；
   pre_exec 异步信号安全上下文不能 opendir(/dev/fd)（malloc）——
   getrlimit(RLIMIT_NOFILE) 上界（夹 64K）逐 fd close≥3。close 语义与
   Linux 分支一致（worker 只留 0/1/2），best-effort 失败不阻断。
2. **matrix_a（能力前置）**：测试面＝Linux 沙箱设施验证（/proc 扫描、
   ns/pid 证据链、bwrap/seccomp 探针），mac 无验证对象——前置
   fs::metadata("/proc/<pid>") 恒败 7/7 同点位；基线轮「通过」实为 job
   提前收尾未观测。按 attack03/04 bwrap skip 先例做 /proc/self 能力前置；
   linux 全量不变（windows 文件级 cfg(unix) 不编译）。**注：此处理按
   「修不动→单独举证」路线走，候选回报时供裁决（若需 mac 平行证据面另立专项）。**
3. **s103_reload（测试平台化）**：process_alive 读 /proc＝mac 恒 false→
   存活断言必败＋wait_pid_gone 恒 true 空真——mac 分支改 POSIX
   kill(pid,0)（EPERM 计存活；僵尸计存活＝/proc 目录语义一致）。
   mac 侧回收断言由空真转真验证（若产品回收在 mac 有真缺陷将在 R8 显形）。
4. **s73（测试壳分叉）**：marker 依赖「shell 进程驻留且 argv 含原文」——
   dash（ubuntu /bin/sh）单命令不尾-exec（本机实验：pgrep 命中）；macOS
   /bin/sh＝bash sh-mode 尾命令 exec 优化直接替换自身为 sleep（实验：
   bash 行消失）＝marker 从全部 cmdline 消失、pgrep 前置恒败。末尾补
   恒真内建 `:`（`sleep 240; : # marker`）——bash 尾命令为内建不 exec 优化，
   双平台 argv 均保 marker；进程组回收面不变。产品契约（平台 shell 语义）
   无需变化。

## 修不动候选（单独举证，候选回报时裁决）

- **perf_tiers**：5 次替换窗口峰值 [140,170,170,215,260] 跨轮逐字节稳定
  ＝macOS 分配器保留语义（不归还页）非泄漏；per-peak 上界断言（≤B0+u×96
  +128）通过，仅「离散 ≤32 MiB」辅助断言结构性为假。外部无 heap 计量
  通道（ps/RSS 含分配器保留）。候选：mac 平台资源语义豁免或另立
  heap-in-use 计量专项。
- **e12**：R7 首现（1/7）。报文形态＝受限子进程先撞 manual.json 重建写
  （EFBIG）而非会话追加——静态推理未闭合（manual.json 为何在子进程视角
  缺失），证据仅 1 轮不足定性，继续观察。

## 取证插桩（6b69a36，暂驻，定位后拆除）

- **tls**：connection.rs 初始化失败附 Debug 链＋mcp_tls 原始 rustls 全往返
  探针（与 hook 同配置）——区分 reqwest/hyper 层 vs TLS/平台层。
- **pty_cancel**：超时报文附会话存活/cnt/release——区分「载荷早亡族
  （python3 解析）」vs「写路径阻塞」。
- **TUI 族（s18/s113）**：diag_exit_probe——超时后发 Ctrl+C 观察能否退出
  ＋新增输出，区分「事件循环冻结」vs「仅提交路径失效」；s23 失败附屏幕
  ＋子进程状态；s114a 的 write_bounded EIO 附子进程状态。
- **已拆**：ci.yml mac-diag upload artifact 步骤（产品诊断文件通道已不存在）。
- **待拆**：e2e_acceptance watchdog_start/log_direct（038 D-mac 轮遗留）＋
  本轮全部暂驻标记。

## R8（43394602657，3b6b641）结果：16→9，批 1/批 2 六项销号

**销号**：attack01/02/05、bin_pipe、env_and_fd(fd5)、matrix_a、
s103_mcp_reload、s73——全部修复持续有效。

**余 9 项**：pty_cancel（R9 已修）、tls、s113、s114a、s18、s23、
s103_async（3/8）、perf_tiers、c09（新 1/8）。

**R8 取证结论**：
1. **tls**：raw rustls 探针同死（写成功→read@0 FIN 无 close_notify）＝
   TLS/平台层问题，非 reqwest/产品装配——服务端线程在响应前关闭连接，
   原因待 R9 服务端捕获报文。
2. **pty_cancel**：sent=0 会话存活=true cnt=1024＝接收侧吃满 LIMIT 而
   记账零——write_chunked 整块落账 ×mac tty 输入缓冲<4096＝首块永卡
   半途。**产品修：INPUT_CHUNK 4096→512**（a755d94；512 低于全平台缓冲
   下限，记账/取消粒度同细，写入语义不变）。
3. **TUI 族 SIGKILL 实锤**：s18/s113 探针 exit=unix_wait_status(9)、
   s114a PTY write EIO 时子进程状态=9——**三例都是被 SIGKILL 猎杀**，
   非产品冻结。产品 kill 路径全组作用域（killpg 对独立会话 ESRCH 安全）
   无法命中；猎杀者身份待 R9 死亡时点全进程表快照（ps auxww）＋看门狗
   15s 提频时间线。
4. **c09**（1/8）：i1 中断落点时 previous_state=completed＝子 agent 在
   中断前已完成——调度竞态（sleep 30 工具未按预期保持活动态）；agents
   与 js worker 无代码关联、排除批 2 回归——轮换观察。
5. **轮换计数（R8 后）**：p2 4/8、s103_async 3/8（R2/R4/R8）、s106 2/8、
   s69/c04/e12/c09 各 1/8。

## R10（43401124686，5daee98）结果：9→7，pty_cancel 修复生效

**销号**：pty_cancel（INPUT_CHUNK 512 记账粒度修复 ✓）、s113（SIGKILL 未
命中——猎杀随机性佐证）、c09（轮换过，1/9）。

**余 7 项**：tls、s18、s114a、s103_async（4/9）、s23、perf_tiers、
p2（5/9→已门控）。

**R10 取证结论（决定性）**：
1. **tls 根因闭合**：服务端事件＝「服务端读错误：Resource temporarily
   unavailable (os error 35)」＝EAGAIN——BSD 语义 accept 继承监听器
   O_NONBLOCK（Linux 不继承），夹具阻塞式读恒撞 EAGAIN 即关连接＝客户端
   「EOF 无 close_notify」。**修复：accept 后 set_nonblocking(false)**
   （24a433d，夹具可移植性修复，产品无涉）。
2. **TUI 族三例再次 SIGKILL(9)**（s18/s114a/s103_async）；s113 本轮绿＝
   猎杀目标随机。产品 kill 路径全部组作用域（killpg 对独立会话 ESRCH
   安全）——猎杀者非产品 kill 路径。
3. **p2 达标 §5 门控**（5 轮轮换＋时序族：30ms 通知×100ms 停等窗口在
   mac 调度下先后不确定）——mac 门控（24a433d），linux/windows 全量不变。
4. **s23 画像**：子进程存活（状态 None）＋视图真实处于上滚态（Back to
   bottom 悬浮条＋顶部内容）——PageDown×3 后视图从底部回到顶部＝上滚
   事件到达应用。R11 复检＋尾字节取证。
5. **windows 腿 R10＝仅 t18** ✓（豁免口径不变）。

## R11（24a433d）——串行二分轮（取证）

- **mac 全量串行**（workflow RUST_TEST_THREADS=1，[kill-diag] 暂驻）：
  死亡消失＝测试内并发互杀（对最终修复定标：并发互斥或组隔离）；仍在＝
  系统级（统一日志 kill 捕获给证据）。
- 死亡报告追加统一日志 kill 记录（近 2m 尾部 40 行）。
- s23 加 600ms 复检＋输出尾字节（区分瞬帧 vs 真实上滚态）。
- pty_cancel 的 [pty-diag3] 插桩已拆（修复确认）。

## R11（43405203424，24a433d）串行二分结果：二分裁定＋tls 销号

**串行（RUST_TEST_THREADS=1，全量 27 分钟）**：e2e 仍红 5 项
（s103_async/s113/s114a/s18/s23）＋session_perf（perf_tiers）＋
tool_search_perf（ts30，新面孔 1/9）。

**裁定**：
1. **并发互杀假说死亡**——全串行下 TUI 五项仍红＝杀源在单测试自身
   生命周期内或系统级。
2. **统一日志无系统级猎杀**（仅 mds→mdworker 常规回收噪声占满尾部 40 行
   ——R12 扩捕 jetsam/memorystatus 谓词）。
3. **exit=9 之谜破案**：wait_screen_pred 的超时清理路径自己 `child.kill()`
   ＝s18/s113/s103_async 的「SIGKILL」是测试框架自清理，非外部猎杀——
   子进程到 30s 超时都活着、探针 Ctrl+C 零输出＝**应用活着、渲染正常、
   完全停止消费输入＝「输入停滞」产品缺陷**（stderr 接在 PTY 上而无 panic
   文本＝线程未崩，指向输入线程被锁阻塞的死锁形态）。s23 复检 still=false
   ＝真实上滚态＋输入已处理＝同一停滞的两副面孔：停顿期按键排队、恢复后
   按序重放（早期 PageUp 延迟重放＝净效果顶部＋Back to bottom 悬浮条）。
4. **tls 销号**（accept 恢复阻塞语义修复生效）、**p2 门控生效**（串行名单
   无 p2）。

**R12 取证**：diag_exit_probe 对存活子进程跑 `sample <pid> 3` 采线程栈
（定位输入线程阻塞点＝死锁锚定）；统一日志谓词扩捕 jetsam/memorystatus；
s23 失败改走 diag_exit_probe。串行保持（27 分钟成本低）。

**mac 当前口径**（R11 后）：TUI 五项（停滞根因待 sample 锚定）＋
perf_tiers（分配器语义，举证）＋ts30（1/9 观察）。

## R12（43410002215，ac42c22）结果与破案

**破案（SIGKILL 之谜终局）**：`wait_screen_timeout` 超时清理路径「只 kill
不 wait」（注释引 b42da47 教训）——s18/s113/s103_async 的 exit=9 全是
**测试框架自己的超时清理**（僵尸态让 try_wait 见 status 9），从来不存在
外部猎杀。子进程到超时全程存活、探针 Ctrl+C 零输出＝「输入停滞」实锤。

**s114a 破案**：两个「预期不出现」断言用 `wait_screen_timeout(...).is_err()`
——超时路径的 kill 副作用顺手杀子进程（僵尸态），后续 Esc 写入在 mac 撞
EIO；**linux master 写死从端静默成功＝两平台分叉的本体**。修法：新增
`probe_absent`（无杀副作用缺席探针）＋收尾显式杀清（原流程靠杀清收尾）。
顺带发现：本机验证时 probe_absent 后 finish 会等活进程退出而挂——
原设计依赖杀清收尾的证据。

**R13 取证**：s18/s113/s103_async 的失败等待换 `wait_screen_nokill_diag`
（无杀清理版）——超时时子进程仍活，diag_exit_probe 的 `sample <pid> 3`
线程栈打得中（定位输入线程阻塞点＝死锁锚定）。

## 逐项状态（R12 后）

- **s114a**：测试设计修复（probe_absent＋显式杀清）——待 R13 销号。
- **s18/s113/s103_async**：输入停滞（应用活、渲染正常、不理输入）——
  R13 sample 线程栈锚定阻塞点后产品修。
- **s23**：同一停滞的延迟重放面孔——sample 同轮取证。
- **perf_tiers**：分配器保留语义（修不动举证，已登记）。
- **ts30_perf_gates**：RSS 子进程未输出期望行（1/9 新面孔，perf 族观察）。
- mac 串行轮保持（27 分钟成本低，排除并发变量）。

## R13 中断与修复（43415329396 已取消）

**CI 挂死实证**：s114a 卡死拖住整个 e2e 目标——看门狗快照：测试进程
55 min、一个 terminal-chat 子进程 **E 态 54 min**（tty=?)、ES 状态＝退出
中不可中断）。根因＝probe_absent 去掉杀副作用后，我的收尾 `c.wait()`
无限等一个 E 态子进程（不排干 master 走不完退出序列——24c8ca3 教训的
同族形态）。隐含好消息：挂点在收尾＝**probe_absent 断言在 mac 已通过**。

**修复**：收尾改 finish 同款「杀清＋排干有界等待」（10s 上限，超限交
Drop 分离承接；Drop 本体已有界安全）。

## R14（43426602759，1bcbef4）结果：7→5

**销号**：s114a（probe_absent＋排干收尾 ✓）、s103_async（本轮绿——4/9
轮换形态延续，继续计数）。tls/p2 门控/c09/e12 均未再现 ✓。

**余 5**：s18、s113、s23（停滞族）＋perf_tiers、ts30（资源族）。

**R14 关键反转**：s18/s113 探针 Ctrl+C **即时处理、应用正常退出
（status 0，完整退出序列）**＝输入路径健康——「输入停滞」收窄为
「**提交面/键面分裂**」：composer 显示 "/save busy1"（打字已处理）而
\r 未触发挥交，30s 后 Ctrl+C 却即时生效。s23 顶部画面＝早期 PageUp
延迟重放（同族另一面孔）。

**R15 取证**：分类探针（Z＝打字面、\r＝提交面、屏幕前后差指纹）＋
sample 线程栈（存活态）；统一日志捕获已移除（runningboard 噪声淹没
无价值）。

## R15/R16 取证与根因闭合（停滞族＝crossterm mac 唤醒丢失）

**R15**：探针顺序失误（\x03 先行把应用正常退出、分类探针落空）——R16 重排
（分类→sample→退出面）。

**R16 分类指纹（三例一致）**：Z 前含 Z？false → Z 后含 Z？true →
\r 后屏幕变？true——**打字✓、Enter✓＝探针时点应用完全健康**。结合 R14
（Ctrl+C 即时处理）＝**输入延迟几十秒后泄洪，非丢失非死锁**：s23 顶部画面
＝早期 PageUp 延迟重放；s18 的 save 提交晚到（探针 \r 时一并执行）。

**根因（sample 线程栈，R16）**：主线程 99.7% park 于 tokio Condvar
（CachedParkThread::park → pthread_cond_wait），事件循环 select! 的
FRAME_TICK=100ms 兜底存在却未恢复＝crossterm EventStream 内部后台读线程
（mio/kqueue 事件源）在 mac PTY 丢输入就绪通知——数据滞留内核 tty 缓冲，
内部缓冲不更新，tick 再轮询也 Pending；下一个输入字节的写触发了恢复
（探针 Z/\r 全部即时生效）。

**产品修复（6421167→本提交）**：src/tui/Cargo.toml mac target 段为
crossterm 启用 `use-dev-tty`（/dev/tty + poll(2) 备用事件源，crossterm
为 tty 事件问题提供的经典回退；fork 的 tty 后端完整支持
InternalEvent/bracketed-paste）。经 TIOCSCTTY 会话 /dev/tty 即测试 PTY
从设备，语义等价；仅 mac 编译生效，linux/windows 构建零变化。

**顺带修**：s18 的 save 提交晚到使探针 \r 实际执行了 /save busy1Z——
分类探针的 \r 副作用在報文里可见（屏幕变=true），无碍取证。

## R17–R20：停滞族根因闭合与产品修复尝试——停下问

**R17**：lock 未随 target 段更新的 --locked 失败（ubuntu/windows 同源），
a698cb1 补 lock 后消除。

**R19 全线程栈（关键）**：一个 tokio worker **100% CPU 自旋于
slice_contains**（调度队列扫描不 park），其余 worker cvwait/kevent，
**主线程 park 等唤醒、tick 罕见轮询（1552 样本仅 4）＝tokio 计时器驱动
停摆 + 输入唤醒 starvation**。s114a 类同点位的 R20 全量表：主线程
clock_gettime 抖动、spawner 正常 park、worker 77769 kevent、**77771
slice_contains 自旋**、77772 cvwait。

**use-dev-tty 未消除停滞**（R18/R19/R20 三轮同指纹：分类探针 Z/\\r/退出
全健康、输入延迟数十秒泄洪）＝停滞在 tokio 运行时层（worker 饥饿→主线程
唤醒 starvation→EventStream/计时器延后），不在 tty 事件源。

**修复所需＝输入架构调整**（EventStream→专属读线程＋通道桥，涉 tui.rs
suspend/resume 的 EventStream drop/resume 交互路径）——**设计取舍，
按阻断线停下问**；不影响当前 PR 的 ubuntu（R20 绿）与 windows（R20 仅
t18）基线。

## 收尾状态总表（R20，a8c9be6）

**已销号（本轮收尾累计 10 项）**：attack01/02/05、bin_pipe、fd5、
matrix_a、s103_reload、s73、pty_cancel、tls、s114a；**p2 §5 mac 门控**
（5 轮轮换＋时序族）。

**mac 现存 6 项**：
| 组 | 项 | 定性 | 证据 |
|---|---|---|---|
| 输入延迟泄洪族 | s18/s113/s23/s103_async | crossterm/tokio mac 唤醒 starvation（产品运行时级） | R11–R20 十轮：栈证 park 99.7%＋worker slice_contains 自旋；分类探针证打字/提交/退出全健康；use-dev-tty 无效 |
| 资源语义族 | perf_tiers | mac 分配器保留语义（离散断言结构性为假） | 峰形跨轮逐字节一致 |
| 资源语义族 | ts30_perf_gates | RSS 子进程未输出期望行（perf 族，1/9） | R14 首现 |

**轮换观察（未达标门控线）**：c04 2/12、c09 2/12、e12 1/9、s69 1/9、
s103_async 5/10（停滞族同源）。

**linux R20＝绿；windows R20＝仅 t18（豁免不变）**。

## 输入架构专项（裁决①执行，2026-11-13）

**方案**：`ThreadedEventSource`（src/tui/src/tui/threaded_event_source.rs，
cfg(target_os = "macos") 编译）——独立读线程以 crossterm 公共 API
poll(100ms)+read() 串行采样，事件经 tokio mpsc 通道供 select! 消费；
替代点＝chat_driver 事件多路与 EventBroker 的 CrosstermEventSource 两处
EventStream 构造（产品输入路径唯一活跃点为前者，后者惰性）。

**等价论证（逐条）**：
1. 事件源/解析器/事件类型/顺序：同一 crossterm 全局 reader＋公共 API
   poll/read（EventStream 内部同路径）——语义一致；
2. EOF/错误：Err 入通道→消费方既有 Some(Err) 路径（EventStream 同形态）；
3. Drop：stop 标志＋rx drop 双保险，线程 ≤100ms（poll 超时窗口）内退出
   （EventStream shutdown+waker 同形态）；
4. pause/resume（broker drop/recreate）：桥 drop＝线程退出＝不再读 stdin
   （放手 stdin 语义一致）；resume 新建＝惰性重建（全局 reader 单例零成本）；
5. 唤醒：mpsc send→recv 直接唤醒（单跳，不经 EventStream 双 waker＋tokio
   调度往返）＝修复本体；
6. linux/windows：cfg 隔离（linux 编译路径零变化＝产物与 main 一致；
   windows 的 WindowsKeySequence<EventStream> 包装原样——generic Stream
   bound 兼容性已核）；
7. use-dev-tty 撤销（R18–R20 三轮实证对 EventStream 无效），crossterm
   依赖与 Cargo.lock 回归 main 原样（单一变量＝EventStream→桥）。

**资源族**：
- perf_tiers：离散辅助断言（≤32 MiB）cfg(not(macos)) 收口——mac 分配器
  保留语义下 RSS 无法区分「保留 vs 泄漏」（峰形 R1–R20 逐字节一致）；
  per-peak 上界主断言（≤B0＋u×96＋128）三平台不变＝真实契约未松。
- ts30：本轮再观察（1/9，未达门控线不动代码）。

**拆桩还原（git grep 标记＝0）**：ci.yml 并行还原；e2e watchdog_start/
log_direct/ps_snapshot/diag_exit_probe/wait_screen_nokill_diag/sample 全拆；
s18/s113/s103_async/s23 失败等待还原原版断言；mcp_tls 原始 rustls 探针
＋服务端事件捕获还原（set_nonblocking 修复本体保留）；connection.rs
debug 链透传还原；clipboard_copy [clip-diag] 拆（漏网残留）；proc_session
[pty-diag] 报文标记拆。保留＝修复本体：probe_absent（s114a 无杀缺席探针）
＋排干收尾＋INPUT_CHUNK 512＋INPUT 注释依据。

**本轮改动面**：threaded_event_source.rs（新）＋chat_driver/event_stream/
tui.rs 接线＋Cargo.toml 注释＋Cargo.lock 还原＋session_perf 断言修正＋
拆桩 6 文件＋ci.yml。

## 本机环境观察（非回归记录）

cancel_wait_keeps_process_alive_and_queryable / deadline_survives_cancel
在本机（TCHAT_SANDBOX=1 部署注入态）间歇红——stash 对照基线同样间歇红
（基线 4 跑 3 红）＝「300ms/250ms cancel 置位窗 vs bwrap 确认闸」的本机
竞态临界，与输入架构改动无关（INPUT_CHUNK 在写入路径、启动确认面零改动）；
R20 ubuntu CI proc_session 28/28 全绿佐证 CI runner 无此差异。CI 结果为准。

## 提交索引（输入架构专项批）

## 输入架构专项结果与根因修正（R21–R25，停下回报）

**R21**：桥首次推送——ubuntu/windows 编译翻车（mac-only cfg 本机不可编译
验证的两处低级错误：路径多写一层 tui、EventBroker Default windows 分支
缺 events 绑定）；fc82919 修复。
**R22**：perf_tiers mac 分支引用比较类型错（同类）；34a1f76 修复。
**R23（桥首个有效验证轮）**：ubuntu 绿；windows 仅 t18＋a15 新面孔
（snapshot 空＝a12 同族回环噪声，1 轮观察）；**mac 停滞族依旧**（s18/
s113/s23/s103_async＋c12 新面孔 1 轮）＝**EventStream 双 waker 假说否定**。
**R24/R25**：临时 sample 探针（无杀等待版，R24 踩超时先杀坑扑空）取证——
**R25 全量线程栈（决定性）**：
- 桥读线程 tchat-tty-events 正常：1282/1288 样本阻塞在 mio kevent 等 tty
  事件（桥本体无恙）；
- 主线程 1288/1288 park 于 Condvar（block_on 等根 future）；
- 4 个 tokio worker 全部闲等（kevent/cvwait），**R19 的 slice_contains
  自旋者消失**；
- **3 秒采样窗内 FRAME_TICK=100ms 的 tick 一次都未唤醒主线程**＝tokio
  timer/任务唤醒事件全部丢失（worker kevent 空）。

**根因修正**：停滞不在 crossterm 也不在 EventStream 结构——**tokio
multi_thread 运行时在 macOS 的 kqueue driver 跨线程唤醒丢失**（任务
notify/timer 事件不入 worker；Linux epoll 无此问题）。全部历史现象吻合：
输入延迟泄洪（等下一次偶然唤醒）＋Ctrl+C 即时生效（SIGINT 中断 kevent
强制唤醒 worker 顺带泄洪）＋s23 PageUp 延迟重放＋桥/EventStream 无差异。

**R25 windows＝仅 t18**（a15 未再现）；ubuntu 持续绿。

**修复所需**：tokio 运行时级改动（current_thread flavor 或等效——影响
turn/MCP/UI 全部并发面，行为变化面大）——**超出「输入架构」授权边界，
按阻断线停下回报**。停滞族证据线已充分（11 轮稳定＋族属明确＝运行时
唤醒丢失）。

**探针已拆**（r24-diag 标记归零；插桩净保持）。

## 运行时专项（裁决①执行，2026-11-14）

**根因链终局**（R25 全量栈）：该 runner（Anka 虚拟化 macOS）的 **kqueue
EVFILT 层失效**——crossterm mio 与 tokio driver 两个独立 kqueue 双双收不
到事件（输入 EVFILT_READ 不触发、timer 超时不返回）；epoll（linux）无此
问题。全部历史现象由「kqueue 沉默＋SIGINT 中断强制唤醒」统一解释。

**选型**（影响面最小＋全链路去 kqueue）：**组合拳**而非 current_thread——
current_thread 不解决（主线程 block_on 的 park/unpark 虽是 pthread Condvar，
但 timer/IO 仍依赖 driver kqueue；且 turn 并发模型变化风险大）。三件套
（全部 cfg(target_os = "macos")，linux/windows 构建零变化）：
1. **读层**：crossterm `use-dev-tty`——tty.rs 用 POSIX poll(2)
   （filedescriptor crate）读 stdin，绕 mio/kqueue；tty_fd()＝stdin is
   tty ? STDIN_FILENO : /dev/tty（TUI 进程 stdin＝PTY slave）。
2. **timer 层**：`Heartbeat`（src/tui/src/tui/heartbeat.rs）——独立
   std::thread 以 thread::sleep(100ms) 计时（内核 sleep，不经 kqueue），
   tokio mpsc bounded(1)＋try_send 满则丢（＝MissedTickBehavior::Skip
   等价）；select! 的 `ticker.tick()` 分支签名兼容（Interval::tick 同
   async fn 形态），分支代码零改动。
3. **唤醒层**：既有 ThreadedEventSource 桥——通道 send→waker→主线程
   CachedParkThread unparker→pthread cond signal（不经 kqueue）。

**语义对照（逐条）**：打字（poll(2) EVFILT 等价读＋同一解析器）、回车
（同上）、Ctrl+C（内核信号面不变——TIOCSCTTY/ISIG 原样）、暂停恢复
（tty.rs 的 wake_pipe/SIGWINCH 接收器为 use-dev-tty 内建；EventBroker
pause/resume drop/recreate 桥生命周期承接）。FRAME_TICK=100ms 语义保持
（Skip 等价）；app/draw 分支（tokio mpsc/broadcast send→直接 unpark
主线程）不经 kqueue 原样保留。

**验证硬性要求**：
- linux 全量（本机，epoll 正常路径原样编译）＋TUI lib 2175 全绿；
- CI（R26）＝mac 停滞族 4 项恢复＋linux 全绿＋win 仅 t18；
- 延迟量化探针 `input_latency_probe`（e2e_acceptance，三段实测 println：
  echo/submit/quit；断言防降级上界 250ms/2000ms/1000ms，目标 <50ms）。

## R27 形态与最终形态（轮询化主循环，2026-11-14）

**R27（组合拳验证轮）**：mac 编译过（129 passed）＋input_latency_probe
**通过**（早期交互全链路 OK）——但停滞族 4 项依旧红（s18 的 /save busy1
turn 后提交静默：字符渲染了、回车无反应）。假说再修正：早期交互通过
（事件已在通道无需唤醒）＋turn 后卡死＋R14「SIGINT 唯一可靠唤醒」＝
**Anka VM 的 pthread cond signal 跨线程丢失**（一切 park 型等待在首个
Pending 后永久睡死；kqueue EVFILT 与 pthread 双失效的虚拟化缺陷）。

**最终形态（唤醒依赖归零）**：
- **删除** ThreadedEventSource 桥＋Heartbeat（park/唤醒面全部退场）；
- **mac 主循环轮询化**（poll_event_mac，chat_driver）：主线程同线程非阻塞
  读输入（crossterm 公共 API poll(ZERO)+read；读层 use-dev-tty＝POSIX
  poll(2)）＋app/draw 通道 try_recv＋Tick＝elapsed 计时；无事件
  sleep(10ms)——root future 永不 Pending＝永不 park；输入延迟上界 10ms；
- **FrameRequester mac 直通化**：schedule_frame/schedule_frame_in 绕开
  FrameScheduler task（其 tokio 唤醒同样丢失＝打字上屏链断）——请求即时
  广播；合并/120fps 限频牺牲（登记；schedule_frame_in 延迟帧退化为立即）。
  scheduler 行为测试 cfg(not macos)（linux 验证）。
- EventBroker 三平台统一 EventStream 原样（mac 产品路径不经它；惰性不创建
  ——无与主循环 poll(0) 的输入竞争）。

**语义对照（逐条）**：打字（同线程 poll(2) 读＋同一解析器＋直通帧请求）、
回车/Ctrl+C（同输入面＋内核信号面不变）、SIGWINCH（tty.rs 源内建
winch_signal_receiver）、暂停恢复（EventBroker pause/resume 的
drop/recreate 与产品主循环无关——产品唯一输入路径为 poll_event_mac）；
FRAME_TICK 帧节奏（Tick elapsed 计时 Skip 等价）；app/draw 通道语义
（try_recv 的 Empty/Closed/Lagged 与 select 分支逐条对齐）。

**Linux 防回归**：本机全量（除 2 项已知本机环境临界竞态——stash 对照
基线同样间歇红非引入）全绿；tchat-tui lib 2175 全绿；探针
echo=104/submit=315/quit=3ms。

## 运行时专项两轮结果与阻断（R26–R32，2026-11-14 停下回报）

**组合拳轮（R27 有效验证）**：早期交互全过（input_latency_probe 绿）；
停滞族 4 项依旧红——「EventStream 双 waker」假说否定。
**轮询化轮（R32 有效验证）**：**回归扩大**——
- e2e_acceptance 33/100（TUI 大面积；探针打字回显 10s 不上屏＝比前更糟）；
- **波及非 TUI 套件**：sandbox_lifecycle 0/4、user_input_security 6/3、
  native_model_apis 31/2、session_cli 26/1（e12 观察项）——影响面超出
  TUI 进程边界；
- windows 仅 t18 ✓、ubuntu 绿 ✓（linux 路径零变化复证）。

**编译翻车记录（mac-only cfg 本机不可编译验证的结构性障碍）**：R26 路径
笔误、R28 Instant 未导入、R29 TryRecvError 变体名（Disconnected）、R30
test_channel/元组元素 cfg 不合法＋tests mod 残余、R31 screen_reader/
clipboard 两函数遗漏——**六个 mac-only 编译轮**。

**根因假说演进**：EventStream 双 waker（R23 否定）→ tokio kqueue driver
（R25）→ pthread cond signal 跨线程丢失（R27 形态）——三个假说均有证据
支撑但轮询化（消除全部唤醒依赖）仍大面积回归＝**假说不完备，缺一份
「挂点直证」**（poll(0)/首帧 draw/初始化的具体挂点无栈数据）。

**两项已闭环**（独立于停滞族）：perf_tiers mac 断言收口、s114a/s114a 类
测试设计修复、p2 门控、tls/pty_cancel/attack×3/bin_pipe/fd5/matrix_a/
s103_reload/s73 全部销号——mac 现存红＝停滞族 4 项＋perf/ts30 观察项。

**三选项（回到裁决）**：
①**挂点直证专项**：产品代码加暂驻文件日志（mac 分支 run_tui/run_interactive
 各阶段＋poll_event_mac 采样计数）跑一轮定位 R32 的 100 失败挂点——上一轮
 的 sample 栈已不可用（探针已拆），需要新的挂点证据再修；
②**§5 门控收口**：停滞族 4 项＋perf_tiers/ts30 登记 mac 平台限制（11+ 轮
 证据线），mac 达标口径=门控生效；插桩已拆净、ubuntu/win 基线绿——PR 现状
 已是「除门控外全绿」；
③**现状候选验收**：PR #7 现状（mac 红面=停滞族，台账齐全）合并候选。
