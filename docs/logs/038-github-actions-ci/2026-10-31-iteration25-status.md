# 038 迭代轮 25+ 三平台状态（2026-10-31，commits 787fb24..5b66e6d+）

## ubuntu（全绿→真实检查恢复后 4 失败）
3852 通过/4 失败——4 失败＝恢复的真实检查在 GH runner 上复现的 D1-D3 异常
（c13/b2_mcp/attack06/sandbox_matrix_probes）＋已登记存量；本地全量 3854/2/6 绿。

## windows（编译清零、全量测试首跑）
31 个测试目标失败，~60 用例：
- SHELL=/bin/sh 不存在 → Exec spawn Error（proc_session/turn/exec_command 族，~29）
- sandbox::tests deny/bwrap//proc 族（~19，需 unix 门控或等价验证设计）
- proc_session seam_clock_shift 族（5，时钟注入）
- tui paste/golden（2，路径分隔符/文本差异）
- 零编译错误——产品层平台实现（Job/锁族/ReOpenFile）经首跑编译验证

## macos（编译清零、运行至 PTY 卡死截断）
2812 通过/28 失败后运行至 e2e_acceptance 卡死：
- apply_patch /dev/fd→EACCES（fdescfs 写检查）→ 已改 O_NONBLOCK 路径安全序（本轮 7772d60）
- sandbox policy argv canonical 断言 → 已修（本轮）
- 卡死转移：s100/s101 TUI 交互、c17（读路径/子进程侧——写侧有界化已排除写因）
- native_model_apis 两指名用例本轮未到达（在卡死目标之前）

## 下一轮清单
1. windows：SHELL 平台决策（cmd /C）＋sandbox 族 unix 门控/等价设计＋seam 族处理
2. macos：PTY 读路径卡死定位（D-mac 方案——bounded read 超时＋线程转储）
3. ubuntu：D1-D3 runner 诊断（获授权的 CI 诊断 job）
