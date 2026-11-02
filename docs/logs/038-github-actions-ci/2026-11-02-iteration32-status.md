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
