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
