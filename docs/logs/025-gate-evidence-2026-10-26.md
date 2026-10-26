# 025 批次门禁证据（补齐版，2026-10-26）

## Security Testing Gate
检查项与结果（详见 docs/logs/025-security-gate-2026-10-26.md 及各规格日志）：
- TLS 信任隔离：注入根全链/不受信拒绝/默认无注入三场景＋回环回归（022-mcp-tls/）。
- 提交扫描：暂存区语义、值锚定豁免（豁免值与另一假 Key 同文件仍拦截且不回显）、
  gitleaks 缺失 fail closed（026-pre-commit-secret-scan/）。
- 脱敏：假凭据演练四项、真实替换、全目录零明文、.env 零接触（025-credential-redaction/）。
- 访问边界：提权/同 UID/快照范围实测（027-credential-access-boundary/）。
重点复核（本轮）：
- 测试根接缝生产外部不可选用：set_extra_root_certificate_for_tests 调用点仅
  src/app/tests/mcp_tls.rs（测试二进制）；mcp.json/生产配置无任何路径触达
  EXTRA_ROOT（config.rs 零引用）；生产 build_http_transport 默认 None。
- 脱敏无新明文备份：脚本 read→replace→write_text 同路径覆盖，无中间文件；artifact
  目录无 .bak/backup 文件（本轮复核 0 个）。
- 扫描三项（暂存区/豁免/fail closed）沿用六场景证据，未重复执行。

## code-review
方式：主审查两轴自查（仓库标准轴＋规格符合轴），对本批全部差异逐文件
（Cargo.toml/transport.rs 接缝/mcp_tls/native_model_apis 新增测试/fixtures 证书/
gitleaks 配置/hook/README/AGENTS.md/六份规格）。
发现与处理：0 个阻断项；2 个过程缺陷（mcp_tls 测试基建 id 回显与 close_notify、
024 补测对 020 语义误解）已在实现中定位修复；1 个样式实验（裸 @）验证无收益后还原。
027 的“流程约定非强制”结论经实测复核成立。

## Clippy
命令：/tmp/024-rust-components/.../cargo-clippy clippy --locked --offline
-p terminal-chat --lib --test mcp_tls --test native_model_apis（及 --all-targets 全量目视）。
退出状态：0。本批文件零警告（mcp_tls 3 条机械警告经 --fix 清零后复验 3 项全绿）；
既有警告基线（catalog/memory.store/tui_host/turn.rs 两处/common/pty.rs 等）与本批
hunks 无交集，未触碰。

## 顺序说明（如实）
全量#1（3480/0/3）→ Clippy 发现 mcp_tls 3 条 → --fix 修改 mcp_tls.rs → 定向复验
mcp_tls 3 项绿 → **全量#2（补跑，覆盖 --fix 后的最终候选，结果以 #2 为准）**。
依赖审计（exit 0、漏洞 0）与性能复测（30 组零超限）晚于 --fix 的代码状态？
——否：--fix 仅改 mcp_tls.rs（测试文件），不影响依赖与模型 API 性能路径；
审计/性能二者的对象（Cargo.lock/模型 API mock 路径）在 --fix 前后一致，证据继续有效。

## RED 证据（如实分层）
- MCP HTTPS：事前证据＝2026-10-25 运行时连接失败记录（022 §5.4 引用）＋本轮实现前
  测试编译期缺失（022-mcp-tls/2026-10-26-red.log）。测试基建的 id/close_notify 错误
  不作为产品 RED（已在前报告声明，属实现过程缺陷）。
- 脱敏/扫描：对照全部使用假凭据；真实明文未恢复、不重演。

## 收口二（2026-10-26 夜）：验收保护（claude_mock_runner.py）门禁

### TDD（RED/GREEN）
- RED＝demo-red：旧流程（不隔离配置，假 HOME settings env→canary、ambient→mock）实测
  canary 收到 1 次模型调用、mock 0 次、claude exit 0 且输出＝canary 标记——错误端点被
  静默选中且回复被当作模型输出（与真实事故同构；单一变量差证明 settings env > ambient env）。
- GREEN＝demo-green/run：mock 模型调用≥1、canary 0、输出含 mock 标记（marker 并入判定）。
- negative：生效 BASE_URL 非批准 mock → 预检在子进程启动前拦截（未启动、双零请求；
  run 路径退出码 2，demo 语义 exit 0）。
- 证据：2026-10-26-mock-runner-evidence.log（含加固后复验与预检定向用例）。

### Security Testing Gate（子代理攻防，2026-10-26-mock-runner-security.md）
结论：通过——三条硬目标（仅批准 mock 可生效／非 mock 绝不真正出网／零真实凭据）
13 组攻防实验均未攻破。发现 7 项 Low/Info，处置：
- F-1 预检键盲区（Bedrock 等端点键放行）→ 改拒绝清单语义（ANTHROPIC_/CLAUDE_CODE_/
  AWS_/BEDROCK_ 前缀默认拦截，只 BASE_URL==mock 豁免）——定向用例验证拦截。
- F-2 PATH 劫持空转绿灯 → 定性为验收完整性（控 PATH 者在威胁模型外，子进程无真实
  凭据）；summary 打印 shutil.which("claude") 供审计。
- F-3 退出码语义/超时不可区分 → run 路径预检拦截 exit 2；超时哨兵独立显示
  timeout(240s)；文档措辞已对齐。
- F-4 marker 仅打印 → 并入 ok 判定。
- F-5 --capture 0644 → 0600（含已存在旧文件的显式 chmod；复验 600）。
- F-6 类型混淆栈退出＋临时目录遗留 → 顶层/env 类型守卫（空列表亦拒，定向用例验证）
  ＋atexit 清理。
- F-7 mock 计数过宽 → 断言改用 model_calls（仅 /v1/messages 且非 count_tokens）。
- 全部处置后四模式复跑一致（evidence log 末段）。
- 攻防过程偏差（子代理如实记录）：超时实验前两次因自身脚本失误走了真实 claude 基线
  路径（端点仍为本地 mock，无真实服务影响）；清理阶段 pkill 自匹配误杀一次后重做。

### code-review
见下节补记（收口二对新增验收脚本与文档修订执行仓库两轴复查）。

### code-review（收口二，两轴并行子代理）
- Standards 轴：1 项硬违规——make_listener docstring 与 last_body 保留行为矛盾 → 已改
  注释如实说明；判断题 2 项——断言形状重复 → 提取 proc_ok()；finish() 参数结伴 → 脚本
  规模接受不改。卫生 2 项——scripts/acceptance/__pycache__ 未被忽略 → 删目录＋.gitignore
  加规则（check-ignore 验证）；evidence 复验段缺命令行/日期不一致 → 补遗说明（F-5 权限
  RED→GREEN 显式化、UTC/批次日期约定）。
- Spec 轴：2 项文档滞后——安全报告锚定加固前版本 → 追加主会话处置附录（结论维持）；
  批次总结第 6 条「CLI 自动加载需下一会话确认」残留 → 改为终局结论。1 项错引——027 文档
  把 apply_patch.rs:1426/view_image.rs:850 列入生产 spawn 点（实测均在 #[cfg(test)]）→
  规格 R1 行本就只引生产点，日志清单已纠正；R1 结论方向不变。
- 范围蔓延：无。处置后终态三模式复跑一致（evidence log 末段）。
