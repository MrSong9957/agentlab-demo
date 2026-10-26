# claude_mock_runner.py 安全攻防测试报告（Security Testing Gate）

- 测试日期：2026-10-25（UTC）
- 被测对象：`scripts/acceptance/claude_mock_runner.py`（293 行，14282 字节，可执行；分支 fix/025-security-hardening）
- 关联证据：同目录 `2026-10-26-mock-runner-evidence.log`、`2026-10-26-claude-mock-incident.md`
- 测试者：Security Testing Gate（子代理）

## 0. 测试约束遵守声明

- 零真实模型服务/付费 API：全部 13 组实验的目标端点仅为脚本自建本地监听器、黑洞端口 127.0.0.1:9、测试方自建监听器 127.0.0.1:39127。
- 零真实凭据：所有注入/抓取实验仅使用假凭据 `FAKE-TOKEN-LOCAL-MOCK-ONLY` 与假端点；未读取 `/workspace/.env`；`~/.claude/settings.json` 仅做键名级核查（`env_keys` 清单与 `env_has_base_url` 布尔值），未输出任何值。
- 未修改被测脚本、`src/`、系统配置；未提交/合并/推送。实验现场全部位于 `/tmp/secgate`（假 claude 家族、攻击者监听器、实验 settings 树），测试结束即清理（见 §5）。
- 过程偏差如实记录：实验 E 前两次因实验脚本自身失误（h_hang 目录未建导致 cp 静默失败）跑了真实 claude 基线路径（结果与 A1 一致，mock 本地、无额外影响），第三次经 `command -v` 解析验证后完成。

## 1. 方法

静态审查：293 行源码逐行审读（预检分层 L129-162、子进程环境构造 L165-187、子进程启动 L190-198、判定与收尾 L201-219、main 判定与 capture L222-293）＋配置入口面核对（本机 `/workspace/.claude/` 不存在、`/etc/claude-code/` 不存在、`~/.claude.json` 存在）。

动态攻防：13 组实验（A1-A4 预检拦截面、B1 项目根解析盲区、C1 非 WATCHED 端点键、D1-D4 PATH 劫持与环境转储、E3 超时、F --capture 落盘、G1-G3 demo 三连），全部经真实 runner 入口发起。

## 2. 总体结论：通过

安全目标逐条核验：

| # | 设计目标 | 结论 |
|---|---|---|
| a | 最终生效端点只能是本次批准的本地 mock | 成立。用户层隔离＋预检＋行为断言三层全部按预期工作（P-1/P-3/P-4） |
| b | 生效目标非批准 mock 时请求发出前失败、绝不回退真实服务 | 基本成立。端点键（ANTHROPIC_BASE_URL）在四层 settings 的覆盖均在子进程启动前拦截（A2/A3/G3）；替代端点开关类键存在预检盲区（F-1），但被代理黑洞＋计数断言双重兜底，实测终态仍为失败、零出网（C1） |
| c | 全程零真实凭据 | 成立。子进程环境从零构造经环境转储实证（D4），凭据仅假 token 且只存在于请求头（F：体中无凭据形态） |

发现 7 项（F-1～F-7），最高严重度 Low，均不破坏上述三条硬目标；其中 F-1（预检键覆盖盲区）建议优先加固。

## 3. 正面确认（攻击未得手项）

| 检查项 | 攻击构造 | 结果 | 证据 | 结论 |
|---|---|---|---|---|
| P-1 用户层配置隔离 | 复跑 demo-green：真实 claude＋隔离 CLAUDE_CONFIG_DIR；本机全局 `~/.claude/settings.json` env 确实含 ANTHROPIC_BASE_URL 键（键名级核查） | 事故未复现：mock=1 canary=0 exit 0，全局配置未生效 | `[summary] mode=demo-green claude_exit=0 mock_requests=1 canary_requests=0 marker=MOCK-REPLY-MARKER`（G2） | 有效 |
| P-2 子进程环境从零构造 | PATH 前置假 claude 转储子进程 env | 键名清单恰为 runner 构造集（18 键，仅 PATH 继承自外部）；ANTHROPIC_AUTH_TOKEN 值=FAKE-TOKEN-LOCAL-MOCK-ONLY（唯一凭据）；`=sk-` 形态值 0 个 | D4 转储（仅打印键名） | 有效 |
| P-3 预检拦截-项目层 | `--cwd` 指向含 `.claude/settings.json {"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9"}}` 的目录 | 子进程启动前拦截：`[preflight-blocked] ANTHROPIC_BASE_URL 被 [project ...] 覆盖为 http://127.0.0.1:9`，exit 1，mock=canary=0 | A2 | 有效 |
| P-4 预检拦截-坏 JSON | 项目层 settings 半截 JSON | 启动前拦截：`不是合法 JSON（Expecting ',' delimiter...）`，exit 1 | A3 | 有效 |
| P-5 预检拦截-凭据键 | settings env 含 ANTHROPIC_AUTH_TOKEN（demo-negative 路径） | 启动前拦截，两监听器均 0 请求 | G3（复跑与 evidence log 一致） | 有效 |
| P-6 失败面 fail-closed：非零退出 | PATH 前置假 claude（echo boom; exit 3） | exit 1，`claude_exit=3` 如实上报 | D3 | 有效 |
| P-7 失败面 fail-closed：纯输出伪造 | PATH 前置假 claude（只打印 MOCK-REPLY-MARKER、不打 mock） | exit 1（mock_requests=0 不满足 mock>=1），计数断言挡住伪造 | D2 | 有效 |
| P-8 失败面 fail-closed：超时 | PATH 前置假 claude（sleep 300，已先经 command -v 验证解析）触发 CLAUDE_TIMEOUT=240 | 约 240s 后 exit 1，mock=canary=0；summary 将超时报为 `claude_exit=n/a(未启动)`（超时与未启动不可区分，标注瑕疵） | E3 | 有效（标注瑕疵记入 F-3 相关项） |
| P-9 错误端点必判失败 | run 判定＝`returncode==0 ∧ mock>=1 ∧ canary==0`（L270-271，main 内强制） | demo-red（canary=1）判 fail 语义成立；run 模式下任何 canary 流量必 exit≠0 | G1＋代码审读 | 有效（唯一理论旁路见 F-7） |
| P-10 代理遏制 | HTTP(S)_PROXY=127.0.0.1:9＋NO_PROXY=localhost,127.0.0.1；C1 注入替代端点开关实测 | 非 localhost 出网路径在本机即终止（两监听器 0 请求、claude_exit=1）；NO_PROXY 同时覆盖 localhost 与 127.0.0.1 两种写法 | C1＋L181-183 | 有效。定性：遏制前提是客户端遵循 env proxy（Claude Code 官方支持）；即便被无视，子进程环境亦无任何真实凭据可携带，付费调用被凭据层二次阻断 |
| P-11 git 根项目层盲区假设 | cwd=仓库子目录（无 .claude），git 根 `.claude/settings.json` 指向攻击者监听器 127.0.0.1:39127 | 预检放行（子目录无配置），但真实 claude 未读 git 根 settings：攻击者监听器 0 请求，mock=1 正常 | B1：attacker.log 为空、`mock_requests=1`、exit 0 | 盲区未复现（当前 claude 版本项目层按 cwd 解析，与预检路径构造一致；若未来版本改为 git 根解析将引入盲区，见 §4 建议） |
| P-12 demo 证据可复现 | demo-red / demo-green / demo-negative 复跑 | 三者 exit 0；计数与 marker 与 evidence log 逐项一致（canary=1/mock=0；mock=1/canary=0；预检拦截且双 0） | G1-G3 | 一致 |
| P-13 --capture 请求体无凭据 | 对 57038 字节 capture 体扫描：FAKE-TOKEN / Authorization / Bearer / api_key / apiKey 均不存在；`sk-` 唯一命中＝"task-notification" 子串误报；`authorization` 唯一命中＝仓库安全文档正文（授权语境）；64 位十六进制串＝metadata.user_id.device_id，UUID＝session_id，account_uuid 为空串 | 请求体只含仓库上下文与本地会话标识，无任何凭据 | F＋溯源 | 无凭据泄露（device_id/session_id 属本地标识，非敏感凭据，见 F-5 权限项） |

## 4. 发现清单（按严重度降序）

| 编号 | 检查项 | 攻击构造 | 结果 | 证据 | 结论 | 建议动作 |
|---|---|---|---|---|---|---|
| F-1 | Low：预检端点键覆盖盲区 | 项目层 settings env 注入 `CLAUDE_CODE_USE_BEDROCK=1`＋`ANTHROPIC_BEDROCK_BASE_URL=攻击者监听器`（能改变生效端点/协议的键不在 WATCHED 四键内） | 预检静默放行（无 [preflight-blocked]），子进程照常启动——「请求发出前拦截」承诺对这类键不成立；终态仍 fail closed：claude_exit=1、mock=canary=0、攻击者监听器 0 请求（代理黑洞＋计数断言兜底），零出网 | C1：exit 1、`[summary] mode=run claude_exit=1 mock_requests=0 canary_requests=0`、attacker.log 空 | 盲区确认，但无真实流量后果。另：ANTHROPIC_MODEL 在 WATCHED 中却从不产生 problem（L159-161 未检查），属死盯 | 预检改为拒绝清单：settings env 中出现 `^(ANTHROPIC|CLAUDE_CODE)_.*(BASE_URL|USE_BEDROCK|USE_VERTEX|API_KEY_HELPER|AUTH_TOKEN|API_KEY)` 等端点/凭据类键即拒绝；或白名单仅放行显式允许的键 |
| F-2 | Low（定性 Info）：PATH 劫持可造「空转绿灯」 | PATH 前置假 claude：自打 mock（读 `$ANTHROPIC_BASE_URL`）＋打印 MOCK-REPLY-MARKER＋exit 0 | run 判定 exit 0＝通过——验收可被完全伪造为绿 | D1：`A1_EXIT` 式判定通过，`mock_requests=1 canary_requests=0 marker=MOCK-REPLY-MARKER` | 定性：威胁模型是「误连真实服务」；能控制 PATH 者已能执行任意代码，不在本门防线上；且子进程环境无真实凭据可窃（D4）。残余风险＝意外 shadow（如 venv 残留脚本）造成假阴性验收，而非凭据泄露 | summary 行输出 `shutil.which("claude")` 解析路径供审计；或按固定可信路径解析二进制 |
| F-3 | Low：退出码语义与文档不符 | A2 实测 run＋预检拦截 exit=1；docstring L30 承诺「2＝预检拦截」；事故文档称 demo-negative「退出码 2」，实测 G3 exit=0（demo 模式把「按预期拦截」判为通过，0 合理） | 退出码 2 从不产生；上游编排无法用退出码区分「预检拦截」与「行为断言失败」 | A2 `A2_EXIT=1`；G3 `G3_EXIT=0`；L213-219 `ok=problems_ok` 只有 0/1 | 文档/实现不一致（fail closed 本身成立） | 统一：要么预检拦截在 finish 前显式 sys.exit(2)，要么修订 docstring 与事故文档为 0/1 语义 |
| F-4 | Low：marker 断言未强制 | 防护设计（L25-26）声称「输出标记必须来自 mock」，但 run 判定（L270-271）只查 returncode＋mock>=1＋canary==0；marker 仅打印（L204-208） | 断言强度低于文档声明；与 F-2 叠加构成空转绿灯条件 | 代码审读 L204-208、L270-271 | 声明与实现不一致（对真实 claude 无实际影响） | 将 `expect_marker in proc.stdout` 并入 ok |
| F-5 | Low：--capture 落盘默认权限 | run --capture 落盘 57038 字节请求体（含仓库上下文系统提示词与 device_id/session_id 元数据），`open(path,"wb")`（L273）受 umask 影响 | 实测权限 0644，本机其他用户可读；体内容无凭据（P-13） | F：`PERMS=644 OWNER=dev SIZE=57038` | 若仓库文档含敏感内容或捕获文件被共享，随之扩散；凭据本身不在体中 | `os.open(path, os.O_WRONLY\|os.O_CREAT\|os.O_TRUNC, 0o600)` 或落盘后 os.chmod 0600 |
| F-6 | Info：settings env 类型混淆未优雅处理；崩溃路径遗留临时目录 | 项目层 settings 写 `{"env":[]}`（非 dict） | 未捕获 AttributeError：`'list' object has no attribute 'items'`，栈退出 exit 1（仍 fail closed）；异常发生在 finish() 之前，mkdtemp 根目录未清理（/tmp/claude-mock-* 遗留，内容为空 settings/空目录，无敏感物） | A4 栈尾；遗留目录核查：`find` 仅 cfg/settings.json（内容 `{}`）等 | fail closed 成立，健壮性与清洁度问题 | `env.items()` 包 try/except(AttributeError, TypeError) 转为预检 problem；收尾用 try/finally |
| F-7 | Info：mock 计数语义过宽 | 监听器对任意路径/方法都计数（L73-74），404 亦计；GET / 也使 mock>=1 | 「mock>=1」实际语义是「被连过」而非「/v1/messages 业务请求成功」 | 代码审读 L66-79 | 对真实 claude 无影响；与 F-2 组合时可降低伪造门槛 | 仅对 `path.startswith("/v1/messages")` 计数 |

「连错端点但判定通过」专项核查结论：run 判定的三个条件在 main 内强制（非仅打印），未发现现实可复现的通过路径。唯一理论组合是「mock 被连过（F-7 宽计数）∧ 第三端点也收到流量 ∧ exit 0」——第三端点需绕过 DEAD_PROXY（仅 localhost 可达＝本机进程）且只能拿到假凭据，超出威胁模型。

## 5. 实验现场清理声明

- 测试方基础设施 `/tmp/secgate`（假 claude 家族、攻击者监听器 39127、实验 settings 树、capture 副本、实验输出）于测试结束删除。
- 被测脚本自身临时目录（claude-mock-*）在正常路径均自清理；F-6 所述崩溃路径遗留目录已由本次测试顺手清理并记入 F-6。
- 攻击者监听器进程随清理终止。

## 6. 证据索引

- 本报告引用的全部 `[summary]`/`[preflight-blocked]`/exit 原始行已归档至同目录 `2026-10-25-mock-runner-security-probe.log`（实验输出关键行摘录，非完整转储）。
- 复现命令：见 `2026-10-25-mock-runner-security-probe.log` 各段落头。

## 7. 加固建议优先级

1. F-1 预检拒绝清单（唯一触及「请求发出前失败」承诺的盲区）。
2. F-4 marker 并入判定、F-7 收紧计数语义（同属断言强度）。
3. F-5 capture 0600。
4. F-3 退出码文档对齐。
5. F-2 打印 claude 解析路径（审计性）。
6. F-6 类型混淆优雅化＋try/finally 清理。


## 主会话处置附录（2026-10-26）

本报告锚定加固前版本（293 行）。其后主会话按建议处置 F-1..F-7（拒绝清单预检、marker 入判、
model_calls 收紧、capture 0600、超时哨兵、类型守卫＋atexit 清理、claude 解析路径审计打印），
并复跑四模式一致（2026-10-26-mock-runner-evidence.log「加固后复验」段）。F-1 类键
（CLAUDE_CODE_USE_BEDROCK/ANTHROPIC_BEDROCK_BASE_URL/ANTHROPIC_MODEL）经定向用例验证现被
预检拦截；结论维持：通过。两轴 code-review 另纠正本批 027 文档两处非生产 spawn 点引用
（apply_patch.rs:1426/view_image.rs:850 属 #[cfg(test)]），已改。
