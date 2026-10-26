# 025 批次实施总结（2026-10-26）

分支 fix/025-security-hardening（基线 main 62543a6）。七项修复全部落地：

1. 025 脱敏：唯一明文 artifact 定向替换（sk=2 ghp=2→占位符，追溯字段保留），
   全 ~/.zcode 复扫零明文；代码侧出口对照无新增缺口。
2. 026 提交扫描：gitleaks 8.30.1（官方 checksums 校验）＋.gitleaks.toml（值锚定豁免）
   ＋scripts/hooks/pre-commit（--staged、--redact、fail closed）＋README；本机已装
   .git/hooks/pre-commit；六场景实测全过。
3. 027 访问边界：实测 sudo NOPASSWD: ALL＋同 UID 0600 可读＋ZCode 快照跨四根路径→
   终态阻断＋管理员操作清单（①收紧 sudoers②多容器拆分③宿主 ACL）。
4. 028 TLS 依赖：rustls 0.23.46、rustls-webpki 0.103.16、webpki-roots 1.0.11；
   终态 cargo-audit exit 0、漏洞 0（5 个既有 unmaintained/yanked 警告保留）。
5. 022 §5.4 MCP HTTPS：reqwest "rustls" feature（aws-lc-rs＋platform-verifier）＋
   doc(hidden) 测试接缝；mcp_tls 三场景（注入根全链/不受信拒绝/默认无注入）＋
   回环 HTTP 回归 17 项；生产 new() 零行为变化。
6. AGENTS.md 引用：docs\规格文档实战法.md → 正斜杠＋「`@` 引用不自动展开、须主动读取」说明
   （终局结论见收口补记一/二：当前工具不支持嵌套引用自动展开，与路径形式无关）。
7. 024 N07 补测：tui_native_image_request_carries_anthropic_image_block（PTY×
   anthropic-messages×-i 初始图；base64 载荷逐字节比对＋TUI 响应流程；纯补覆盖）。

验证（全绿）：
- 受影响定向集 200 项（native_model_apis 31/mcp_tls 3/mcp_http 17/mcp_config 13/
  mcp_stdio 37/mcp_turn 35/mcp_resources 3/llm_flow 51）。
- 最终全量（025-full-final.log）：exit 0，65 目标，3480 通过/0 失败/3 忽略（tui 既有）。
- Clippy：本批文件零警告（mcp_tls 3 条自动修复后清零）；transport.rs/native_model_apis
  本批 hunks 零警告；既有基线警告未触碰。
- 依赖审计：exit 0、漏洞 0。
- 性能复测：30 组零超限，最差用量 35.7% 冻结阈值，p95 3.03–11.32 ms、RSS 22,764–24,592 KiB（performance-final-025.{json,log}）。
- TLS 握手时延单列记录（docs/logs/022-mcp-tls/），不与明文基线混比。
- .env 终验：原位 true、单值形态、0600、本批次写操作 0。

过程如实记录：mcp_tls 首跑失败源于测试基建两处缺陷（JSON-RPC id 回显恒 0、close_notify
未写出），经明文对照实验定位修复，非产品缺陷；024 补测首跑失败源于对 020 初始图自动提交
语义的误解，修正断言后通过——两者均不构成产品 RED。

## 收口补记（2026-10-26 晚）

- 文档加载验收：见 025-credential-redaction/2026-10-26-agents-md-load.md——引用行进入
  上下文（正斜杠），嵌套正文不展开为工具机制限制；一次因全局 settings 覆盖导致的
  真实最小调用已如实记录（后续两轮零真实调用）。
- .env 证据分层：「本批写操作为零」成立（操作清单可证）；「起终内容一致」无批次起点
  指纹，以脱敏轮同轮比对（true）＋形态核验（原位/单值/0600）为间接证据——限制如实，
  不补造历史指纹。
- 后台进程：本批全部后台任务（全量/性能/审计/轮询）已退出，无改写候选或证据的进程。
- 全量#2：因 Clippy --fix 晚于全量#1 修改了 mcp_tls.rs，补跑覆盖最终候选
  （025-full2.log＝exit 0、65 目标、3480/0/3——与 #1 一致，--fix 无行为影响），结果以 #2 为准。

## 收口补记二（2026-10-26 夜：误连修复、结论修正、027 修订二）

- 误连真实服务修复：scripts/acceptance/claude_mock_runner.py——CLAUDE_CONFIG_DIR 隔离＋
  子进程环境从零构造＋四层 settings env 预检（请求发出前拦截非批准目标）＋mock/canary
  行为断言＋本机关闭端口代理遏制。根因实证：settings env 优先级高于进程环境变量
  （demo-red 单变量复现：canary 收到请求且其回复被当作模型输出、退出码 0——与事故同构）。
  RED/GREEN/negative/run 四段证据：2026-10-26-mock-runner-evidence.log；事故与根因记录：
  2026-10-26-claude-mock-incident.md。全程零真实服务、零真实 Key。
- 文档验收结论修正：「反斜杠路径导致正文未展开」原假设未获实测支持——裸 @（正斜杠）
  对照同样不展开，当前工具不支持嵌套引用自动展开机制本身；正斜杠修正保留（语法正确性
  收益）。AGENTS.md §2 已加「`@` 引用只带入引用行本身，须主动读取」说明，不宣称自动加载。
- 027 修订二：wrapper 方案不能直接批准——R1（驾驶者经 exec_command/mcp.json 以应用 uid
  读凭据，代码证 proc_session.rs:1050 等）＋方案 A 功能代价（工具以 tchat-app 运行写
  dev 属主文件被拒，写位实测）。新增实测：9p 属主/权限位/sticky/写位均强制；sudo -u
  env_reset（HOME/PATH 重置）；/opt、/usr/local/bin dev 不可写；/vault 非 .env 别名；
  无 Docker socket。方案 A（管理员层）＋方案 B（工具子进程降权最小产品变更，仅评估）
  与五项待决定范围见规格附录修订版。027 保持「实现中，受阻」。
- 本轮新增：scripts/acceptance/claude_mock_runner.py；日志 2026-10-26-mock-runner-evidence.log、
  2026-10-26-claude-mock-incident.md、2026-10-26-wrapper-revision.md；修订 AGENTS.md、
  027 规格、ag-load 结论、docs/index.md。产品代码（src/）零改动——全量#2、审计、性能
  证据继续覆盖当前候选的代码状态。

## 收口补记三（方案 B 收敛，2026-10-26）

- 027 方案 B 修订为完整可决定版：权限边界总表（exec_command/MCP/apply_patch 降权，
  记忆/图片/上下文/会话保持并给出代码级依据——图片格式门、记忆根域、AGENTS.md 链
  固定文件名）；主进程内部文件操作全部枚举（生产 File::open/fs::write 全库核查）；
  新增发现：proc_session 两处 spawn 无 env_clear（现状 exec_command 子进程继承含凭据
  父环境——B 经 sudo 关闭该原语）。
- 假凭据验证 E1-E4 全过（环境零继承/fd 不透传/跨 uid /proc 拒绝/dev 文件能力），
  9p 探针复用；需真实账号/sudoers 的验证列管理员待执行项（5 项）。
- 文档：027 保持「实现中，受阻」；密钥轮换移出待决定清单（按用户指令停止的记录保留）。
- 本轮零产品代码改动；全量#2/审计/性能证据继续有效。新增日志
  2026-10-26-plan-b-boundary.md。候选仍待批准提交/合并/删分支，未提交未推送。

## 收口补记四（方案 B 边界定向验证，2026-10-26）

- E5（上下文符号链接）：terminal-chat 读取路径已阻断——context.rs probe_source lstat
  拒链跳过（t04 定向 1 passed，提示不含正文）；B 不变（上下文保持 tchat-app，依据＝
  文件名固定＋拒链双层）。嵌套 claude CLI 端到端则跟随符号链接（假凭据标记进入模型
  请求）——dev 侧工具链行为，A/B 后读不到真实凭据，记录为 --capture 使用注意。
- E6（记忆 Git 假钩子）：固定 argv 不防已植入钩子（假 pre-commit 以 git 进程 uid
  执行，标记属主证）；实际防线＝.git 不可达三层（resolve_scoped_path 拒 .git/符号
  链接/越界＋定向测试过；dev 写他人 .git/hooks 被拒；env_clear 无 HOME）。规格 B 已
  改依据并增可选纵深⑤（run_git hooksPath=/dev/null）与对应验收条件。
- 027 保持「实现中，受阻」；零产品代码改动；候选未提交。

## 收口补记五（027 B 产品实现，2026-10-26 深夜）

- 方案 B 产品部分按规格附录「B 产品实现」章 TDD 落地：隔离模式（TCHAT_ISOLATED_TOOLS）
  下 exec_command/MCP stdio/apply_patch 经 sudo 以目标用户执行（env_clear＋env_reset
  零继承、MCP 显式 env 经 /usr/bin/env 保留、补丁 stdin 传 --internal-apply 子进程、
  结果附子进程 uid 作身份直证）；记忆 git hooksPath=/dev/null；context O_NOFOLLOW
  修 TOCTOU；killpg 跨 uid 补杀（实测发现的真缺陷：主进程对降权组员 EPERM）。
- 五项 RED/GREEN＋定向回归全绿（exec 20/apply 36+2/mcp 系 136/tool_loop 13/context 15/
  e2e 143 单独复证）；Clippy 本批文件零警告；gitleaks 零泄漏。
- 027 保持「实现中，受阻」：tchat-app 全链路、sudoers 收紧放行性、PTY×sudo×setsid
  部署语义仍属管理员验收前提。候选（含本轮产品改动）未提交。

## 收口补记六（027 B 实现+安全处置+全量，2026-10-26 深夜定稿）

- 安全攻防（有条件通过→处置后通过）：三条隔离主链路与 hooksPath 防线全部攻击构造
  未被突破。处置：F1（中）目录组件符号链接注入通道→context.rs 逐前缀拒链＋末组件
  O_NOFOLLOW（RED→GREEN）；F2（低）TCHAT_ISOLATED_TOOLS 入 TRUST_CONTROL_VARS；
  F3（低）隔离用例进程级 env 互斥锁；F10 文档归因更正（`env K=V -- cmd` 才是失败
  形态）。两轴 code-review：3 硬违规（注释失实/无消费接缝）＋判断题全处置
  （Option<u32> 身份、流式限读、KILL_BIN 收敛、锁外补杀、无关重写还原）。
- 过程真缺陷修复：killpg 跨 uid EPERM（主进程杀不到组内降权成员）→kill_group_cross_uid
  补杀（SIGKILL/SIGINT 两路径，锁外执行），隔离超时/PTY 变体转绿。
- 最终全量（027B，plan-b-full.log）：exit 0，65 目标 **3489/0/3**（基线 3480＋本轮
  新增 9，含隔离身份/环境/PTY/超时、Update/Delete 变体、目录链 TOCTOU、hooksPath）。
- Clippy 本批文件零警告；gitleaks 零泄漏；性能：普通模式代码路径零变化（None 分支
  等价＋一次 env 读取），30 组冻结阈值证据继续适用；隔离模式 sudo 包装净开销实测
  ≈3ms/次（单列，不与明文基线混比）。
- 027 保持「实现中，受阻」（修订三）：产品实现完成且验证，tchat-app 全链路/sudoers
  收紧/PTY×sudo×setsid 部署语义属管理员验收前提。候选未提交（47 项差异）。

## 收口补记七（父目录竞态收口，2026-10-26 晨）

- 应用户指令收口上下文「先检查后打开」残余窗口：RED（接缝移至检查后，父目录
  rename swap→symlink，标记静默读入 notices=[]）→GREEN（open_nofollow_chain 逐级
  目录句柄＋每级 O_NOFOLLOW，无检查-打开窗口；ELOOP→「符号链接」提示；检查降级为
  提示分类）。fd 泄漏（自查成功路径＋审查失败路径）修复并加 100 次拒绝零泄漏回归。
- 竞态测试互斥（并行假红修复）；027 规格第 6 条与部署回滚清单（独立恢复入口＋
  备份前提＋完整步骤）同步。
- 最终全量 exit 0（65 目标 3490/0/3）。候选未提交（未合并未推送）。
