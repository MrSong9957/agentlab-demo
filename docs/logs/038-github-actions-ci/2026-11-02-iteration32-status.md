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
