# 038 迭代轮 32：安全测试＋Windows 第二轮修复（2026-11-02）

基线：3c96aff（迭代 31 收口）→ 本轮 HEAD 见 git log。安全测试由
Security-Testing-Gate 代理执行（范围 5f98508..HEAD 产品代码，焦点锁/路径别名/
FD 继承/树回收），报告结论：**高风险 0 项；中风险 1 项（D1，已修）；低风险 7 项
（D2 已修，A1/A4/B2/B3/B4/C3 登记下轮加固）**。报告全文见代理输出
（docs/security/reports 未落盘——按代理建议下轮归档）。

## 本轮已交付修复（全部先本地复现/回归再推 CI）

| 提交 | 内容 |
|---|---|
| 5f5ad80 | D-mac 取证插桩（e2e spawn 直写 fd2 标记＋60s ps/线程采样，定位后移除） |
| 67f29af | canonical_long 产品统一（策略/边界/lex 三侧形态一致——verbatim/8.3 形态差异致边界判定系统性失配）＋Start-Sleep 延时＋exec_p01 平台分支＋cd/workdir long_norm 断言＋symlink_dir 分支＋env 正向段 unix 门控 |
| 667ae85 | Windows flock 锁偏移移出数据区→批次 9 改命名互斥体（字节 0 强制锁致外部读者 error 33）＋memory reset 对在途连接持有的 DB 降级 DROP 全表（外部契约不变）＋phase1 存储错误注入 Windows 全文件字节锁分支＋retention 大小写孪生平台分支 |
| 2f285d8 | MCP 夹具 python -X utf8（locale 编码读 UTF-8 流致 mojibake） |
| a410a6b | 安全 D1/D2：mem_write 统一共享锁临界区（None＝拒绝）＋reset DROP 事务/转义/失败计残留 |

## 验证结果

- L3 全量（2e60960 本地）：3850/6/6——6 失败全为已登记环境敏感/存量族
  （c18×3 冻结阈值、p16 RSS、session_perf 预算、proc_session Ctrl-C），
  无新回归；安全修复后 memory_runner/phase1/request_permissions/session 全绿。
- Windows CI：批次 7-9 首轮因 windows-sys 0.52 OVERLAPPED 字段路径（OffsetHigh
  非直接字段）编译失败——已改命名互斥体模式（已验证可编译）＋Send impl；
  最新 CI 轮（d1e01ce/a410a6b）结果见 Actions。
- macOS：e2e_acceptance 挂点取证插桩已就位；本轮 macOS job 多为并发组取消，
  插桩证据待完整一轮（后续单独触发）。

## 登记的后续项（下轮）

1. ConPTY（008 A6）：windows-sys 实锁 0.52，CreatePseudoConsole/
   InitializeProcThreadAttributeList/UpdateProcThreadAttribute/
   PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE 均在已启用 feature 内——实现为
   独立提交（不可本地编译验证，须单独 CI 轮迭代）。
2. Ubuntu D1：产品启动链 argv 对证（诊断 job 工件已证明 seccomp 机制正常）。
3. 安全加固：A1 互斥体名掺随机成分、A4 lock None 契约全临界区统一、
   B2 UNC 剥离特判、B3 junction 逐组件检测、B4 canonical_long Result 变体。
4. macOS e2e 挂点：以插桩单独触发 workflow_dispatch 取证。
5. c18_barrier：按项目规则选用可复现验证环境（/tmp 原生 fs）复验；9p 环境
   相关性已有交叉实验记录，机制解释不作已证结论。

## 迭代 32 续：D-mac 挂点收敛（插桩实证）＋Windows 219→55（be32bc9..c216494）

### D-mac 挂点收敛（插桩证据，run 41341822162 macos job）
- 插桩证据：e2e 二进制 2h21m 总 CPU 20ms（纯睡眠）；3 个 terminal-chat 子进程
  卡 **E 态（退出中）2h20m**；整目标零输出。
- 挂死链＝死锁闭环：子进程退出序列写满 PTY 缓冲→阻塞在退出中；测试线程
  Drop→无界 child.wait()→无人排空 master→互卡；Drop 无看门狗＝零输出形态。
- 修复（be32bc9）：PtySession/PtyCli Drop＝先有界（10s）排空 master（让子进程
  写完退出序列）→超时强杀→分离线程收尸（Drop 永不无限阻塞）；child 字段
  Option 化；存活断言改 kill(0) 探测。本地 e2e 150/150 绿。
- 修复验证：D-mac 修复后的 macos 轮被后续推送并发取消——待单独完整轮验证
  （下一轮 macos 即可直接验证挂死是否解除）。

### Windows 55 失败逐项归因（windows11.log，run 41359234973，c216494）
- proc_session 13：6＝pty_*/ConPTY fail-closed（待 ConPTY 实现）；2＝Start-Sleep
  引号在 cmd→powershell 传参中被回显进输出（`-Command "..."` 双引号经 Rust
  转义后 powershell 回显命令行）→改无引号形态 `powershell ... -Command
  Start-Sleep -Milliseconds 800`；2＝双流行 for/l echo 尾空格（`echo X &` 的
  空格进输出）→改 `echo X&echo Y` 紧凑形态；read_does_not_reset 2.5s 容差
  边界（2.504s>2.5s）→微调；pipe_ctrl_c＝存量；turn_exec/incremental 同
  Start-Sleep 回显根因。
- memory 族 11：DROP 降级保留 memories.sqlite 文件（内容已清）与「文件不存在」
  断言冲突（unix 删除语义）→Windows 断言改「新库 0 行」；phase1 注入的全文件
  字节锁连读查询也锁（database is locked）→注入机制需只锁写面（下轮设计）。
- exec/cd_inside：cmp_norm 在 remove_dir_all 之后执行→目录已删 canonicalize
  失败退回短名→归一取值移到清理之前。
- attack02 展示路径断言/attack05/2×request_permissions：形态断言测试侧同步
  （产品已统一 canonical_long，断言侧补 ln）。
- mcp 13+tls 1、tool_search_perf 1、multi_agent 1、terminal_chat 3：错误面
  取证后逐项（下轮）。

### 本轮结论
- Windows：219→72→55（编译清零保持）；全部 55 项有根因记录（上表）。
- L3（e8dd11b 本地）：3852/4/6——bwrap_backend 环境竞态（TMPDIR 并行改写）
  已修（c216494 env_lock），其余 3＝已登记环境敏感/存量。
- 安全低风险项：B2/B4 已修；A1（互斥体名随机成分）/A4（其余临界区 None 契约
  统一）/B3（junction 检测）/C3（理论项）登记下轮（A4 的 mem_write 处已随 D1
  统一）。

## 迭代 32 终：D-mac 二轮取证（41362078838 取消轮日志）——修正挂点判断

Drop 排空/强杀修复**未解除挂死**（同 3 子进程 E 态 1h25m＝出生即卡）。二轮证据：
- e2e 7 次 spawn 后冻结；s100×2＋s101 的 3 个 terminal-chat 子进程**从 spawn 起即
  E 态**（ELAPSED＝job 全长）＝CLI 启动/退出早期即阻塞（macOS pty session-leader
  teardown 形态——master 持有时 leader 退出死锁的已知类别）。
- 测试线程真实挂点＝**timeout 分支的 kill→无界 wait()**：wait_screen 等看门狗到
  期→kill（E 态子进程不可中断）→child.wait() 永不返回→panic 永未打印→零输出
  ＋无失败报告。此前判断「Drop 无界 wait」只是次级挂点（本轮 Drop 修复后主挂点
  上移到 timeout 分支——同款无界 wait 还存于 e2e_acceptance 1235/1288/1366 与
  common/pty.rs 同位）。
- 下轮修复序列：①全部 kill→wait 对改有界等待＋分离收尸（与 Drop 同款）；②
  master 提前丢弃（先关 master 让 slave 侧写返回 EIO、退出序列不再阻塞——排空
  不是解法，EOF 才是）；③若①②后仍 E 态＝macOS tty 驱动级死锁，转为产品侧
  （CLI 退出序列不阻塞写：超时放弃）＋MACOSX pty 复用调研。
- Windows 并行证据（windows12/windows11）：55 失败族中 powershell 回显与 b04
  尾空格已修（81f236e EncodedCommand＋紧凑重定向），待下轮 CI 回报其余。

## 迭代 32 终补：b42da47 macos 关键进展＋三轮采证（f39e616）
- b42da47 macos（kill_and_reap 版）：e2e spawn 7→**130**（大批用例解锁通过），
  仅剩 s100×2＋s101 三场景的 E 态子进程（6-7 分钟龄）；其余 terminal-chat 进程
  活跃（S 态）——timeout 分支解除阻塞的修复方向得到验证。
- 三轮采证（f39e616）：看门狗对 E 态子进程执行 macOS `sample` 堆栈采样（2 秒
  采样写入工件日志）——直接给出产品退出路径的精确阻塞调用栈。
- 已知：其余 17 个 spawn 未发生＝3 个卡死测试线程占满 3 线程池，约 20 用例
  排队其后——修复 3 场景即可全量解锁。

## 迭代 32 终定：Windows 55→14（windows13.log，b42da47 轮＝首个 EncodedCommand 全量验证）

EncodedCommand（-Command 回显根因）＋关联修复消除 41 项。剩余 14 项分类归因：
1. **sandbox fail-closed 文本族×4**（non_canonical_writable_root、sandbox_mode_pty_
   timeout、sandbox_mode_wraps_tool、js sandbox_wrapper）：Windows 无 bwrap 的
   报错文本/哨兵行为与 unix 断言预期不同——按平台报错契约逐个对齐（下轮）。
2. **RUNNER~1 短名族×4**（result_json_display_capture、view_image 目录 Access
   denied、internal_apply os error 87、install_dir）：lib 单测夹具比较含短名——
   比较侧 long_norm 化＋view_image 的 `dir\.` 形态开档行为下轮处理。
3. **exec_command×2**：b04 `echo X 1>&2` 的无空格形态把 `1` 绑进文本（打印
   err-eeeeeeeeee1）→行成员判定改 trim_end（内容契约不含尾随空白）；cancel
   3→3＝PowerShell 写入者节流仍慢于 400ms 窗→窗口 2.5s 已有但取点仍早（下轮
   用 wait_until_true 有界轮询替代固定睡眠）。
4. **mcp_resources×3**（m1_directed_list、m2_read_preserves、m2_templates）：
   scenario 级细分——错误面取证后修（下轮）。
5. **min_output_cap 码点边界**（"p" vs "中"）：Windows 输出截断的字节级边界行
   为差异（下轮对证）。
6. **c18_host_rss_bounded**：/proc RSS 读取在 Windows 不存在→主进程 RSS 采样
   需 Windows 机制或平台登记（下轮）。

## 迭代 32 末批：Windows 14 项中 4 项修复（本地验证，待 CI 回报）
1. b04 行成员判定 trim_end（`text1>&2` 空格绑定两形态均归一）。
2. cancel 观察窗改有界轮询（6s Windows／1.5s unix——固定 400ms/2.5s 窗在
   runner 两次实测零增长）。
3. c18_host_rss：Windows 分支 GetProcessMemoryInfo WorkingSetSize（/proc 不
   存在——/proc 路径恒 0＝"主进程 RSS 读取失败"），新增
   Win32_System_ProcessStatus feature。
4. f39e616 教训登记：文档推送同样触发并发组取消采证轮——采证期间禁止任何
   推送（含 docs）。
剩余 10 项：sandbox 文本族×4、RUNNER~1 短名族×4（apply_patch display capture
与 view_image 属 0.02s 家族——套件内确定性失败、单跑待验）、mcp_resources×3
（报文未捕获，下轮取证）。macOS sample 堆栈：活跑 f1fbb53（41372513512）
macos job 收束后提取（预计 ~03:56 上限或提前完成）。

## 迭代 33：41373539854（08f1d75）三平台完整归因——Windows 真实全集 54 项

**口径修正：此前「14」是 windows job 未完整跑完全部目标时的部分计数**。本轮
三平台 job 均完整结束（29m16s，failure），Windows 实际失败 54 项/19 个目标
（exec_b04、cancel、min_output_cap、c18 等 12 项与既有归因一致；c18_host_rss
经 GetProcessMemoryInfo 修复已通过；其余 42 项为此前从未被观测的目标）。

- **macOS 主根因（级联 47 项）＝夹具 finish() 等待循环只 sleep 不排干 master**
  （e2e_acceptance.rs:1434）：产品退出序列向 slave 回写大块渲染（图片卡/
  问题恢复/计划快照）写满 PTY 缓冲后阻塞在退出中（看门狗两次采到 `?<Es`
  E 态子进程 11–21s 不死＝SIGKILL 无法中断内核写等待）→「PTY 子进程未在
  期限内退出」级联；泄漏进程累积后 runner 在 23m 处截断（连失败汇总都未打
  完，无超时无取消——资源耗尽形态）。修复＝等待期 read_once(20) 持续排干。
  s82 挂 421s 后自愈通过（曾疑 s100×2+s101 实为同根因的不同表现）。
- **mcp 家族统一根因＝setup() 模板漏 `-X utf8`**（mcp_fixtures.rs:72，其余
  模板均有）：Windows 管道 stdout 默认 locale 编码（cp1252）——夹具含中文
  的场景（content_types/is_error/resources 等）UnicodeEncodeError 崩溃＝
  「传输已关闭，响应未到达」；a03 mojibake（record 记录 ä½ å¥½＝cp1252 解码
  UTF-8 字节）与 ubuntu/macOS（locale UTF-8）幸免完全互证。
- **Windows 授权形态族×4（canonical_long 残留缺口）**：install_dir deny 逃逸
  （策略尾组件不存在→canonical_long 回退原形态→RUNNER~1 短名 vs deny 长名
  文本失配）、nested_read verbatim 授权路径被误判「超出 requestable」、
  attack02/policy_load 期望侧绑定 verbatim 形态。修复＝path_covers（最长已
  存在祖先 canonicalize＋词法余部）应用于 requestable 子集/deny 冲突/base
  覆盖三处比较；测试期望侧归一。
- **Windows 锁语义审计（ReleaseMutex 线程所有权契约）**：SessionLock/
  RootLock 的 Drop 此前无条件 ReleaseMutex——TUI 下 turn 线程经 on_history
  取锁、UI 线程经 flush_final/Drop 释放＝非拥有线程释放（ERROR_NOT_OWNER
  静默失败，互斥体滞留）。修复＝守卫记录 owner ThreadId，Drop 判定后释放；
  同线程重入（WaitForSingleObject 对自有互斥体递归成功）经 resume 的
  same_file 复用分支规避，canonicalize 失败形态下残余差异已登记（递归深度
  语义与 unix flock 二次加锁 Busy 不同，单写者外部可观测语义不变）。
- **f2_swap_barrier 平台分科**：macOS 无 /proc 且 fdescfs 拒写重开——FIFO
  占位路径 O_NONBLOCK 重开 ENXIO＝fail-closed（拒写）即设计语义；Linux
  /proc/self/fd＝真 inode 绑定写成功。测试按平台断言（共同不变量：FIFO 未
  被读写、不写错对象）。
- **D1–D3 诊断工件（workflow_dispatch 41378437334）**：独立探针 bwrap＋
  产品同款 cBPF＝clone3 ENOSYS ✓；产品链 strace 显示 bwrap **未调用
  seccomp(SET_MODE_FILTER)**，且 runner `unshare --user` 失败（AppArmor 限
  制 userns；bwrap 自带 profile 故独立探针可用）——产品链根因继续收敛方向
  ＝seccomp fd 传递/加载侧；诊断 job 下轮补 apparmor sysctl＋bwrap stderr。
  D2 证据：close 后 marker sleep 被 init 收养（marker_pid=1）＝ns 成员未补杀。

本轮修复（提交批次一）：mcp -X utf8、mac finish() 排干、SessionLock/RootLock
owner 线程守卫、path_covers 三处＋测试期望归一、f2 平台分科、apply_patch
relative_paths 期望归一。本地（Linux）request_permissions 38/38、attack02、
f2_swap、relative_paths 全绿。

## 迭代 34：批次一/二回报（41382750135、41387309164）＋批次三

**Windows 54→32**（41387309164）。修复确认：b04、min_output_cap、mcp_stdio×2、
mcp_resources×3、mcp_turn a09×3、m1/m2×2、memory m3×2、reset_between/
reset_during、install_dir、nested_read、policy_load、attack02/05、c18、
sandbox_mode_wraps_tool、js sandbox_wrapper（fail-open 闭合）。剩余 32 项
新归因：
1. **cancel（3→3）**＝cmd for 体吞噬：writer_cmd 经 seq 追加的 `& sleep` 被
   cmd 绑进 for 循环体（每迭代多跑一次 30s 睡眠 powershell）＝写入停滞——
   两轮"零增长"同一根因；Windows 分支去掉保活追加（for /l (1,0,2) 自身无限）。
2. **CLIXML 进度噪声**：powershell EncodedCommand 在 stdout 为管道时把进度
   流序列化进 stdout（"Preparing modules for first use"）＝
   incremental_output/running_returns 前缀污染——ps_encoded 统一
   $ProgressPreference='SilentlyContinue' 前缀。
3. **result_json**：替换在 serde_json 转义后文本上进行（单反斜杠目标恒
   miss）；Add 记录词法路径（短名原样）＋canonical 两种形态都替换。
4. **view_image**：目录开档在内核即被拒（CreateFile 无 BACKUP_SEMANTICS
   打开目录＝error 5），词法消 dot 后仍如此——打开失败时按路径元数据判型，
   目录复用统一「非普通文件」错误面。
5. **G2 verbatim**：tmp_ws 经 canonicalize＝verbatim 形态，`sub\..` 的分量
   语义平台差异仍未被②命中——升级四道判据（点分段无条件拒绝；③内核解析
   ≠词法形态仅在非符号链接路径拒绝）＋测试失败时打印 raw/segments/lex 诊断
   （若下轮仍失败可直接归因）。
6. **m6/memory_lifecycle rg**：路径探测带 .exe 已生效（越过探测）；新失败
   ＝cmd /C 多引号剥离规则吞掉内层引号对＋夹具路径含空格——Windows 分支
   免引号＋无空格夹具路径（空格语义由 unix 分支覆盖）。
7. proc_session 管道族 13 项：CLIXML＋超时容忍属夹具层；pty_* 5 项待 ConPTY。

**macOS 全集首次可见**（job 不再 23 分钟截断）：92 项/19 目标——js worker
spawn EINVAL（os error 22）×41 最大家族、e2e TUI s 系×25、normalize 逐组件
检查拒 /var 符号链接×7、attack/sandbox_lifecycle×6、其余单点。**此前的
"47→25 改善"实为截断口径（两轮 job 分别只完成 7/5 个目标）**。request_
permissions 的 /var 拒绝非回归——该目标此前从未在 mac job 中跑到。
**ubuntu**：两轮均只剩 D1–D3 四项（c09 为偶发，pipe_ctrl_c 失败为本地容器
特性，CI 全绿）。

**批次三（本轮提交）**：ConPTY 实现（ChildHandle 抽象＋CreatePseudoConsole
＋PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE＋CreateProcessW；作业挂接/整树终止
与管道模式同机制；进程退出→ClosePseudoConsole 拆除顺序）；mingw 交叉 check
（x86_64-pc-windows-gnu target）本地验证 cfg(windows) 代码 0 error——
不依赖 CI 即可验证 Windows 编译面；批次三夹具/产品修复如上 1–6。

## 迭代 35：批次三回报（41392289704）＋批次四

**Windows 32→21**。ConPTY 生效实证：pty_child/pty_interactive/pty_cancel/
pty_write_window 在修复后通过（pty_write_window 的 sh 命令仍需 Windows 分支
——本轮修）；**ConPTY 管线（CreatePseudoConsole＋属性清单＋CreateProcessW
＋作业挂接＋退出后拆伪终端）在 CI 上端到端工作**。其余确认：cancel（for 体
吞噬修复）、incremental_output/running_returns（CLIXML 静默）、blocked_input
升到 Running 后差 terminate 语义、result_json/view_image/m6×2/exec_sandbox、
install_dir、attack05、same_chain、resources_filename、c18、reset 两例。

剩余 21 项新归因：
1. **G2 破案（诊断输出）**：Windows `PathBuf::join("..")` 在 push 时折叠——
   `ws.join("sub").join("..")` 产物就是干净路径（raw 无 sub/.. 可言）＝测试
   前提在 Windows 上不可经 join 构造。下轮改字符串拼接构造＋失败诊断保留。
2. **pty_interactive/pty_child/pty_write_window**：命令为 sh 设施（printf/
   read/echo、`[ -t 0 ]`、stty）——Windows 等价分支（PowerShell In.ReadLine
   /IsInputRedirected/sleep）。
3. **input_limit_exact -1073741502（0xC0000142 STATUS_DLL_INIT_FAILED）**＝
   并行 spawn 下的 runner 级瞬态（Windows CI 已知形态），非产品缺陷。
4. **pipe_ctrl_c**：Windows 中断档＝整树终止（产品文档化的残差登记）——
   「忽略中断仍运行」断言需平台分科。
5. **read_does_not_reset_deadline 2.53s**：Windows spawn/监控粒度下超时容差
   待对齐（下轮对证）。
6. **m7_reset_during_inflight**：Windows DROP 降级后旧连接查询＝no such
   table（预期形态）——断言平台分科（本轮已修）。
7. **c15/t18/ts30**：依赖真实注入/时序，下轮按消息对证。

批次四（本轮提交）：**D2 ns 成员补杀**（monitor 终态后按 NSpid 链分离线程
SIGKILL 补杀，unix 门控）；macOS RSS 采样 ps 分支（session_perf/
tool_search_perf）；pty.rs finish() 等待期排干（同 e2e finish 根因）；
retention 大小写分支扩展到 macOS（APFS 默认不敏感）；m4 大小写折叠碰撞
诊断；m7 平台分科；pty 三测试 Windows 命令分支。ubuntu 侧 D2 修复后 c13/
e_reaping 待 CI 复测。

## 迭代 35 末批（环境重置事故与恢复）

**D2 v1 生产事故（如实记录）**：批次五（0764134）的 sweep 读组长（bwrap）的
ns/pid inode——bwrap raw_clone 为载荷建新 ns 后自身留在调用者 ns，target 实
为容器 ns inode；会话终止时 sweep 扫 /proc 对「同 ns」进程 SIGKILL＝容器进
程树连坐死亡。两处实证：①本地容器 zcode server 被杀（会话环境重置）；②
CI 41398734625 ubuntu job 49 分钟后「hosted runner lost communication」。
修正（遍历组长 children 取 ns≠self 的载荷 ns；children 不可得降级不补杀）
在容器内由并发协作完成并已合入本地提交。

**环境重置损失与恢复**：gh CLI、GitHub 推送凭据、rustup mingw target、
/tmp 采证目录全部丢失。批次六（e7fae16：mac /proc 族等价取证＋path_covers
mac 扩展）、批次七（perf 门禁分科＋m4 折叠分科）、D2 修正三个提交滞留本地。
匿名 API 复核（仓库公开）：0764134 三平台 failure（ubuntu＝事故轮）。

**待凭据恢复后的动作**：推送三个本地提交→CI 验证（重点：windows G2 字符
串构造/proc_session 容差族、mac /proc 族＋retention 大小写＋perf 门禁、
ubuntu D2 修正后 c13/e_reaping）；恢复 rustup mingw 交叉 check。

**本地验证记录（批次七 HEAD）**：lib 290/0、request_permissions 38/0、
attack031 7/0、sandbox_lifecycle 4/0、js_security 18/0、memory_storage_files
22/0、session_perf 1/0、proc_session 27/1（pipe_ctrl_c＝本地容器 dash 特
性，CI ubuntu 绿）、multi_agent c13 2/0、sandbox_matrix_probes e_reaping
1/0＋b2_mcp 1/0＋attack06 1/0（放宽 seccomp profile 容器）。

## 迭代 36：本地 L3 全量（批次七后 HEAD）与并行抖动复跑

**L3 全量第一轮**：`cargo check --locked`（通过）＋`cargo test --workspace
--no-fail-fast --locked`＝**3856 通过/1 失败/6 忽略**——1 失败＝pipe_ctrl_c
（本地容器 dash 对 trap'' INT 的组信号行为特性；CI ubuntu 同用例绿）。
**并行抖动复跑捕获**：c18_barrier（串行对照 wall-clock 在容器多核争用下四
连倒挂 217/250ms——对照降级采样，重叠验收由屏障结构化断言完整承载，五连
复验绿）＋s141（定向复跑绿）。**定向绿全集**：lib 290、request_permissions
38、attack031 7、sandbox_lifecycle 4、js_security 18、js_perf 7、
memory_storage_files 22、session_perf 1、multi_agent c13 2、e_reaping 1、
b2_mcp 1、attack06 1（本容器为放宽 seccomp profile——D1/D2/D3 复测通过，
CI 严格 profile 的回报待推送后）。

**推送阻塞（返回决策事项）**：会话环境重置丢失 GitHub 推送凭据（gh CLI、
credential helper、ssh key 三路均无）。本地滞留 4 个提交待推送：
批次六 e7fae16（mac /proc 族＋path_covers mac 扩展）、批次七（perf 门禁
分科＋m4 折叠分科）、D2 载荷 ns 修正（安全修正——已推送的 0764134 含容器
连坐缺陷，**应尽快推送替代**）、c18_barrier 对照降级。

**安全测试记录（本轮范围）**：D2 v1 事故＝本会话最高优先安全发现，根因
（bwrap ns 拓扑误解）与修正已入库；D1/D2/D3 复测命令与结果如上（放宽容器
通过）；Windows 锁 ReleaseMutex owner 契约审计结论（迭代 33）＝同线程
重入经 resume same_file 复用规避、跨线程释放经 owner 判定跳过。

## 迭代 37（方案 A 核验与补修，工作区改动未提交）

用户确认方案 A（沙箱启动确认闸）。条款→实现→测试映射见规格 §0-A。核心：
载荷自停前缀（STOP）＋宿主确认（隔离＋载荷 ns 标识同点确认）＋SIGCONT 放行；
超时/失败/取消统一整组终止（016 有界清理）＋Err。普通模式零前缀零闸。

**实现要点**：确认闸在 start_with_owner 调用线程同步执行（monitor 启动前）
——cancel 引用仅调用线程可用，故 spawner 线程只负责 spawn；确认成功前句柄
不发布。verify_child_pid_namespace 超时语义修订（原「无子进程超时保守放行」
→超时即 Err＋清理）惠及 MCP stdio/apply_patch/js worker 的失败场景。

**测试**：lib 单测 3（cancel 真实 bwrap＋哨兵边界／超时确定性（无载荷监督
进程）／LeaderGone）＋集成 1（gate03 放行→016 终止）；三连稳定；lib 293/0；
exec_sandbox 3/0；沙箱族回归（c13 2/0、e_reaping、b2_mcp、attack031 7/0）。

**c18 归因（规格归属核对）**：034 规格 120 行明文「并发有效性以屏障证明 4
个独立调用实际重叠，**不靠模糊延迟断言**」——wall-clock 串行对照属规格明文
排除的延迟断言，降级为采样不是验收弱化；验收本体＝结构化屏障断言（4 全到
达才放行，未重叠必超时＝不可伪造）＋C04 集成。降级合规。

**pipe_ctrl_c 归因（部分闭合）**：①段（可中断）失败＝spawn→trap 生效窗口
竞态（并行负载下 250ms 内 trap 未执行→SIGINT 按继承处置杀 sh）——已修
（READY 文件标记确定性时序，trap 语句后写出，测试等标记再发 \x03），单跑
绿。②段（忽略中断）本地仍失败：诊断链——READY2 出现（trap 已执行）→前置
查询确认 sh 存活→\x03（唯一信号＝killpg(SIGINT)，代码审查）→sh 死
（Completed/exit None）。同形态三方独立复现（bash setsid/python
DEVNULL+setsid/rust setpgid+null+pipe）全部 SURVIVES——与 harness 内行为矛
盾，归因未闭合；CI ubuntu 同用例绿。**如实保留失败**，不称全量通过。

**s141**：目标内并行 150/150 两轮绿（含 s141）——workspace 级并行失败与
c18_barrier 同机制（跨目标多进程 CPU 争用放大时延），待 L3 全量复现后按
证据处置窗口容差。

## 迭代 37 末批：L3 全量（方案 A 候选）与安全审查

**L3 全量**（工作区改动在库，含方案 A）：`cargo check --locked` 通过；
`cargo test --workspace --no-fail-fast --locked`＝**3860 通过/1 失败/6 忽略**
（较迭代 36 ＋4＝方案 A 测试 4 个）。复跑失败名单＝仅 pipe_ctrl_c②（归因未
闭合，见上；CI ubuntu 绿）。**s141 与 c18_barrier 本轮全量通过**——s141
证据累计：目标内并行两轮 150/150＋本轮全量＋定向四连＝5 绿/1 失败（并行
时延偶发，与 c18 同机制；无确定性复现，不加猜测性窗口改动）。

**安全审查自查（方案 A diff，450+ 行）**：①STOP 前缀注入点唯一（tool_argv
沙箱两分支，Cow 统一；普通模式零注入），拼接层＝sh 源码（cmd 本就是 sh 源
码——exec 契约，无新解析面），STOP 失败即自杀＝fail-closed；②CONT 放行对
象精确到载荷 pid（children 定位，非 killpg），且仅 ns≠caller 确认后；
③清理边界：killpg 限沙箱组＋D2 sweep 按载荷 ns 圈定，gate01 哨兵存活断言
＋payload_ns 回归双向锁定；④TCHAT_SANDBOX_CONFIRM_TIMEOUT_MS 接缝任何取值
均不产生放行旁路（更长→超时清理；更短→更快不放行）——fail-closed 方向；
⑤收尸边界：confirm 的 kill_group 内联 waitpid 在 monitor 启动前执行，无
竞争；LeaderGone 无僵尸。**安全测试命令与结果**：gate_confirm_*（lib，
真实 bwrap）三连 3/3；沙箱族回归（exec_sandbox 3、multi_agent 22、
sandbox_matrix_probes 10、e2e_sandbox 7、request_permissions_sandbox 15、
mcp_stdio_sandbox 1、attack031 7、lib 293）全部通过。SHA＝工作区未提交改
动（基线 57d5408）。

**ce-code-review 自查结论**：confirm 函数与接线逐段复查（放行前提/清理
路径/竞态窗口）无缺陷；两处 Cow 借用问题在编译期发现并修复；测试断言的
ECHILD 语义（confirm 已收尸）已按实际行为校正并注释。

## 迭代 38（2026-11-03）：方案 A 启动路径全覆盖＋环境阻塞记录

**四路启动闸全接**（契约条款→实现→接线测试映射见规格 §0-A 更新段）：
①exec/proc_session（shell 前缀，迭代 37 已接）；②apply_patch internal-apply
（INTERNAL_GATE_ENV 自停入口；env 注入点在 env_clear 后——初版在先被清除，
编译期未暴露、接线测试暴露后修正）；③js worker（同自停入口，读 init 帧前
STOP；标记仅沙箱请求设置——无条件设置会让普通模式 worker 永久 STOP）；
④MCP stdio（sh 自停＋execve 原位替换包装；首版漏传 program——bwrap `--`
后首参为 `-c`＝启动失败，/dev/shm argv 转储定位修复）。
verify_child_pid_namespace 调用方缩减为 confirm 闸替代（proc_session/
apply_patch/MCP/transport）；闸核心迁 sandbox.rs pub（跨模块复用）。

**接线测试证据（真实 bwrap）**：
- apply_patch_sandbox 9/0（含 gate_applies_patch_after_release＝放行真实
  落盘；gate_leader_gone_rejects＝坏 exe→确认失败零执行）；
- exec_sandbox 3/0（gate03 放行→016 Terminated→写入停止）；
- js_orchestration_security 19 用例（gate_timeout_worker_startup_fails_closed
  ＝0ms 窗＋即退假 worker→启动失败；env_and_fd＝放行后执行）；
- mcp_stdio_sandbox 1/0＋mcp_stdio 35/0＋mcp_resources 37/0＋mcp_turn 26/0
  （包装放行→协议面无回归）；
- lib gate_confirm 3/0 ×3 连跑（取消/超时/LeaderGone 确定性）＋剥 trace 后
  复跑 3/0；lib 全量 293/0。

**pipe_ctrl_c② 归因（ce-debug 方法，按失败路径取证）**：①段（可中断）已修
＝READY 文件标记确定性时序（spawn→trap 生效窗口竞态，并行负载实证）；②段
（忽略中断）失败路径取证：READY2 在 trap 语句后写出且存在（trap 已生效）→
前置查询确认 sh 存活→interrupt 唯一动作＝killpg(SIGINT)（代码审查）→
Completed/exit None（sh 死于信号）。同形态三方独立对照（bash setsid、python
DEVNULL+setsid、rust setpgid+DEVNULL+PIPE、std process_group posix_spawn/
fork 双形态）全部 SURVIVES 且 SigIgn bit2 置位（trap 生效）——失败路径与
对照路径存在未识别差异，归因未闭合。临时 interrupt 取证插桩已移除（结案
前提未满足）。CI ubuntu 同用例绿＝环境相关。**如实保留失败**。

**环境阻塞（返回门禁事项）**：容器累积僵尸 23,632（terminal-chat 11162/
bwrap 8140/sh 3300 等，全部父进程＝PID1 `sleep infinity`——容器 init 不收
尸，结构性不可回收）触发隐藏 pids 上限（≈23.9k，cgroup 内不可见）：仅剩
27 线程余量，rustc codegen/cc 链接间歇性 EAGAIN/ICE。**最终全量 L3 被阻断**
（最后一次完整 L3＝3860/1/6，在方案 A 接线前；接线后的全量仅能以目标级
回归代替——如上清单）。清理需宿主侧操作（超出本轮授权）。

**验证降级复核**：c18 wall-clock 对照降级＝034 规格 120 行明文排除模糊
延迟断言，结构化屏障断言完整承载（合规，保留）；s141 失败与通过记录保留，
CPU 争用为假设（未直接证明，无猜测性修复）。**6 忽略项核对**：源级
`#[ignore]`＝4（multi_agent A1/A2/A3＋基线既有用例），与基线 e6e3662 的
4 处位置一致、零新增；L3 聚合「6 忽略」为 cargo 按测试目标分别计数的口径
差（multi_agent 目标报 3＋其余目标报 3），非新增跳过。

## 迭代 38 末批：最终 L3 阻断终态

`cargo test --workspace --no-fail-fast --locked` 在 lib 链接期 ICE（exit 101
＝codegen 线程 EAGAIN）——环境 pids 上限阻断成立（非代码编译错误：同命令
在方案 A 接线前完整通过 3860/1/6）。接线后的完整全量未能执行；目标级回归
清单（上文）为接线后的最新证据。**待批准候选＝工作区差异（HEAD 57d5408
未变）**：方案 A 四路闸＋接线测试＋verify 修订＋规格 §0-A 更新＋证据日志。
云端验证（推送＋CI 三平台）单列待授权；宿主侧僵尸清理单列待授权。

## 迭代 39（2026-11-03）：宿主清理未生效后的环境终态与②段取证补充

宿主侧清理未发生：僵尸 23,656（略增）、pids.current 23,877、线程余量实测
仅 7-8（长生命周期线程）——上界仍由僵尸槽位占满。降载构建（codegen-units
=1＋jobs=1，本地 .cargo/config.toml 辅助——已撤除）首个依赖 crate 即 ICE；
最终全量 L3 复跑同样在 lib 链接 ICE。**环境终态＝无法执行任何 rustc 链接。**

**②段取证补充（免重建路径）**：旧测试二进制（Nov2 23:09 构建，含②旧版）
＋/proc 1-3ms 外部采样器。两次尝试均被 EAGAIN 干扰（测试线程 spawn 失败／
采样器误锁跨运行同标记的遗留 sh——该遗留 sh SigIgn bit2 置位（trap 生效）
且存活远超其测试生命周期＝**至少一次运行中②sh 确实存活过 SIGINT** 的反证
样本）；其最终死亡原因未证。②段在-container 归因仍不闭合；CI ubuntu 绿。
残留问题清单更新：需要干净环境（或宿主清理）复跑以获得②的在位证据链。

**迭代 38 记录勘误**：MCP 包装 program 缺失缺陷的发现路径——更正为
「/dev/shm argv 转储（调试插桩）定位」，插桩已全部剥除（grep 0 残留）。

## 迭代 40（2026-11-03）：环境临界终态

降载三旋钮（jobs=1＋codegen-units=1＋llvm-args=-threads=1）后 once_cell 仍
ICE；线程余量实测降至 2，bash 自身 fork 重试失败——容器进入「无法编译」终
态。每次 rustc 尝试新增少量僵尸（23,878→23,883），继续尝试适得其反；本轮
已停止一切编译尝试。**降载配置（.cargo/config.toml）已撤除**，内容存档：
`[build] jobs=1; rustflags=["-C","codegen-units=1","-C","llvm-args=-threads=1"]; [profile.dev] codegen-units=1`（宿主清理后如需受限变体可原样恢复；
规范 L3 命令运行时不应存在该文件）。

**②段取证插桩保留在源中**（proc_session.rs request_group_interrupt 的
PRE/POST150ms 状态快照——迭代 39 登记）——环境恢复后的首轮 proc_session
pipe_ctrl 运行即产出在位证据（/dev/shm/int-trace），结案后移除。

**需宿主侧执行（精确清单，均超出本轮授权）**：
A. 清理僵尸：容器 PID1 为 `sleep infinity` 不收尸——需重启容器（僵尸随
   PID1 消亡）或将 PID1 换成会 wait 的 init（宿主 compose 改动）；
B. 或提升该容器 pids 上限（宿主侧 cgroup pids.max，容器内不可见不可改）；
C. 完成后于 /workspace 运行规范 L3：`cargo check --locked &&
   cargo test --workspace --no-fail-fast --locked`（工作区差异在库——方案 A
   四路闸＋接线测试＋②插桩），预期 3864± 用例、唯一候选失败仍为
   pipe_ctrl_c②（/dev/shm/int-trace 即产出②在位证据链）。

## 迭代 41（2026-11-03）：fork 耗尽终态与条件执行器结局

容器 fork 完全耗尽（`spawn bash EAGAIN`）——会话仅余 Read/Edit 等无 fork
通道。**条件执行器（l3_runner.sh）结局＝超时退出**：220 轮轮询（12:08–
16:43Z，约 4.6 小时）内宿主清理未发生；自 poll#2 起 runner 自身的 pids 读
取亦失败（日志 pids= 空）；日志**无 CHECK-EXIT/TEST-EXIT/SUMMARY 行**＝
L3 未被执行器运行（恢复后无双跑风险）。保全核验（无 fork 通道完成）：
/workspace 为宿主 bind mount——git refs 实读核验（HEAD＝57d5408、远端＝
0764134）＋spec §0-A／证据日志迭代 36–40 全文实读——重启不丢任何提交、
修改或文档。**容器 ephemeral 项（重启丢失，已评估）**：/tmp/l3_runner.sh
（内容在会话历史可重建）、/tmp/l3-conditional.log（本文引用已存）、
/dev/shm/*（int-trace 尚未产生；②插桩在源码随 /workspace 存活，恢复后
首轮 pipe_ctrl 运行即重新产出）。执行器不再重建（恢复后手动按序执行）。
**等待用户批准原地重启容器**——重启即解除：僵尸消亡→fork 恢复→规范 L3
补齐→②在位证据（插桩在源）→插桩移除→同一候选最终验证闭环。
