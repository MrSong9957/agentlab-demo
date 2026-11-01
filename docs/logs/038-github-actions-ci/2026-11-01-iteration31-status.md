# 038 迭代轮 31：Windows 失败根因逐族修复实录（2026-11-01）

本轮以 Windows CI 219 失败的逐条报文为证据，按根因族修复；全部修复先本地
（unix 侧）回归再推 CI 实测。提交序列：915acb2 → 7705010 → 998ed0d → c59fe0b → 4de41a3。

## 实测轨迹（windows job 失败数）

| 轮次 | 提交 | 失败数 | 主要修复 |
|---|---|---|---|
| 迭代 30 | 9a1e65c | 219 | （基线） |
| 轮 1 | 915acb2＋7705010 | 152 | 会话锁句柄访问权、memory 根锁命名互斥体、mcp 夹具 python3→PY、mcp_config 平台绝对锚点、gitattributes eol=lf、cmd_builder＋sh 语法夹具迁移 |
| 轮 2 | 998ed0d | 80 | MCP 夹具 cwd 反斜杠 JSON 转义（invalid escape→服务器全不启动＝MCP 族 83 个的真根因）、normalize_request 盘符前缀、D-mac 有界写 |
| 轮 3 | c59fe0b | 77 | absolute_lexical 保留盘符（request_permissions 组件 NotFound 根因）、回环 ping 延时、PS emoji 字面量、cd/workdir 规范化断言 |
| 轮 4 | 4de41a3 | （待 run 41274163407） | cmd 路径保留反斜杠（重定向正斜杠误解析）、亚秒 sleep 1 秒档 |

## 本轮确认的产品级根因（全部有 CI/诊断证据）

1. **session.rs create 路径 append-only 句柄调 LockFileEx** → ERROR_ACCESS_DENIED
   → 自动保存停用 → multi_agent/context_*/js_orchestration 级联。修复：补 read(true)。
2. **memory flock_root 对目录句柄 LockFileEx** → 恒失败 → 共享锁 None → 管线全部
   临界区静默中止（memory 族零产出）。修复：Windows 命名互斥体（Local\<路径哈希>）。
3. **mcp.json 中 cwd 原生反斜杠＝非法 JSON 转义** → "invalid escape" → 服务器全不
   启动（83 个失败）。修复：路径统一正斜杠写入。
4. **normalize_request/absolute_lexical 丢弃 Windows 盘符前缀** → 组件按进程当前盘
   解析（runner 仓库 D:、临时目录 C:）→ NotFound（permissions/attack 族）。
5. **checkout CRLF**：runner 全局 autocrlf 影响 include_str!/黄金文件（字节数＝
   行数漂移，prompt_assembly 21178 vs 20903）→ .gitattributes eol=lf。
6. **D-mac 卡死真位置**：读路径内 answer_terminal_queries 的无界 write_all
   （子进程停读→永久阻塞→等待循环截止永远到不了）＋ send_keys 无界写 → 全部改
   有界写（超时转储证据后 panic，不把卡死当测试通过）。macOS job 本轮已运行
   47+ 分钟未复现卡死（此前 ~2 小时内卡死）。
7. **cmd 无 sh 设施**：`;` 序列/printf/seq/$(( ))/while/`` ` ``/pwd（内置）——
   经 cmd_builder（tests/common/cmd_builder.rs）平台等价构造。

## D1–D3 诊断结论（runner-diagnosis job 工件，run 41272093310）

- **D1 seccomp**：产品同款 cBPF 最小复现在 runner（bwrap 0.11.1，内核
  7.0.0-1017-azure）上 **clone3 正确返回 ENOSYS(38)**——过滤器机制正常；
  无过滤器对照为 E2BIG(7)（非本地 WSL2 的 EINVAL 22）。产品路径报 EINVAL
  ⇒ 差异在**产品启动链**（argv/fd 传递/031 动态过滤器分支），待下一轮按
  bwrap argv＋strace 逐项对证。
- **D2 进程树回收**：close() 后 bwrap 死、沙箱内 sleep 存活；ps/NSpid 采样
  证实 **PID 命名空间 init（会话子 Agent，ns-pid 1）存活 → 内核级联销毁未
  触发**。修复方向：016 组杀契约需覆盖 pid-ns init（产品设计项）。
- **D3 MCP fd**：沙箱内服务器 FDSCAN 证实 fd 10/3 **在 runner 上不存在**
  （本地存在）→ 与 D1 同族：fd 传递链在 runner 内核 7.0 下丢失。
- 注：unshare --user --map-root-user 在 runner 上 FAIL（与 bwrap 自身成功
  并存——诊断脚本侧现象，不影响主判读）。

## 剩余失败族（77 个，均有根因记录，修复待下一轮）

- proc_session 12：pty_*/input_limit/blocked_input＝ConPTY 未实现（008 A6，
  产品显式 fail-closed 登记）；read_does_not_reset 时序测量、cancel_keeps 增长
  窗口＝回环 ping 节流在 runner 上的实际耗时不稳，需取证再调。
- request_permissions 7＋attack 3：策略路径的 \\?\ verbatim／8.3 短名形态统一
  （产品 canonicalize 需 dunce 式简化——与 normalize_request 同域）。
- memory_runner 6＋memory_phase1 2：reset 对被打开 DB 文件（共享违例）的
  残留计数、符号链接根在 runner 上的创建语义、IO 注入机制的平台等价设计。
- mcp 残留 13＋mcp_tls 1：tools=0 场景细分（错误被 manager 吞掉，需错误面取证）。
- exec_command/exec_sandbox/双流截断等零星：cmd 行为差异逐个对证。
- exec_p01（$$/$! 组杀验证）＝unix 专属验证结构需平台重设计。
