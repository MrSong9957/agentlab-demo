# 031 实施验证记录（M1–M5 交付候选，2026-10-29）

- 分支：feat/031-request-permissions（基线 main@523eb30，工作区含计划期未提交编辑）。
- 环境：与 M0 同容器（内核 6.18.33.2-microsoft-standard-WSL2，bwrap 0.9.0，宿主 UID 1000）。
- 本记录为实施方证据；最终验收由验收人依规格判定（不自行写「已通过」）。

## 交付内容（逐路径）

| 路径 | 内容 |
|---|---|
| src/app/src/permissions.rs（新） | 策略严格解析（递归重复键拒绝/未知字段/矛盾规则/信任来源）、请求解析（§1.4 全边界）、路径规范化、四字段结果、TurnGrants、PermissionHost trait、快照构建（含 argv 容量校验与预演原子提交）、冻结装配 |
| src/app/src/sandbox.rs | 031 动态入口 permission_launch_argv：受限读树（tmpfs 根＋/usr、/etc ro＋usrmerge＋私有 /tmp）、逐挂载实例 deny 遮蔽（含 mountinfo 同设备 bind 别名投影）、P06(b) 递归 nlink 检查（条目/墙钟预算，fail-closed）、--unshare-net、seccomp cBPF（arch 门＋clone-ns 门控）、载荷程序单文件挂载、逐启动 PID ns 子进程验证、close_range 跳 fd 变体 |
| src/app/src/proc_session.rs | SpawnJob 携出生快照＋extra 标记；start_with_policy；毒丸零执行；reap_extra_permission_sessions（统一 SIGTERM→共享 3 秒窗→SIGKILL 升级→失败如实报告） |
| src/app/src/apply_patch.rs / view_image.rs / mcp/transport.rs | apply_patch 策略变体；view_image --internal-view-image 沙箱内部入口（主进程不代读）；MCP stdio 基线快照（不继承临时授权） |
| src/app/src/turn.rs | TurnExtras.permissions、request_permissions 分派（含已覆盖直回 granted）、出生快照 per-call、轮出口统一收尾门（薄包装）、view_image 沙箱分派 |
| src/app/src/main.rs / catalog.rs / prompt.rs | CLI/TUI 分流前 install_from_env 装配（失败拒启）；CliPermissionHost（默认拒绝/Esc 语义/EOF error）；TRUST_CONTROL_VARS 收录 TCHAT_PERMISSION_POLICY；工具声明门控与定义 |
| 测试 | request_permissions.rs（33 用例）、request_permissions_sandbox.rs（11 用例，真实 bwrap）、sandbox/tests.rs ＋12、e2e S140（真实 CLI＋PTY 全链路） |

## 关键实测发现（实施期）

1. **seccomp clone-flags 门控必须先判 nr==clone**：掩码块对全部调用执行会把 execve 的 args[0]（指针）误判为 namespace 标志 → 全载荷 EPERM（集成测试首跑抓到）。
2. **直连 bwrap 进程是 pid ns 之外的监督者**：其自身 ns/pid_for_children 与主进程相同属正常形态；逐启动验证对象应为 /proc/<bwrap>/task/*/children 里的载荷进程（ns/pid）。
3. **P06(b) 反例闭合实证**：deny 目录内部文件预存硬链接（变体 A）→ 启动前递归检查拒绝、工具零执行（request_permissions_sandbox::deny_dir_internal_hardlink_zero_execution_real）。
4. **安装目录整体 deny 与运行时白名单冲突**：Debian dpkg 合法硬链接（/usr/bin/perl nlink=2）使目录级递归检查每次误拒（S140 实证）；修正为共享系统目录**单文件 deny**（应用二进制/bwrap 文件），策略专用目录仍整体 deny——保护语义等同（对象是二进制内容）。
5. **cargo target/debug/terminal-chat 为硬链接对（nlink=2）**：按已确认契约拒启成立；031 部署需 nlink=1 固定副本（S140 以拷贝部署验证；真实部署装专用位置）。
6. **工作区位于用户私有根内＝拓扑矛盾**：build_snapshot 的「内容位于 deny 之下」检查正确拒绝（S140 夹具初版踩中）。
7. **mountinfo 运行时挂载也要遮蔽**：deny 别名落在 /usr、/etc 之下的 carve 不能只对内容挂载做。

## 验证结果（2026-10-29 实测）

| 检查 | 命令 | 结果 |
|---|---|---|
| 031 单元/集成（M1） | env -u TCHAT_SANDBOX cargo test --locked --test request_permissions | 33 通过/0 失败 |
| 031 沙箱（M2，真实 bwrap） | env -u TCHAT_SANDBOX cargo test --locked --test request_permissions_sandbox | 11 通过/0 失败 |
| sandbox 模块单测 | env -u TCHAT_SANDBOX cargo test --locked --lib sandbox:: | 34 通过/0 失败 |
| e2e S140（真实 CLI+PTY+bwrap） | env -u TCHAT_SANDBOX cargo test --locked --test e2e_acceptance s140 | 1 通过/0 失败 |
| 全量（中途快照） | env -u TCHAT_SANDBOX cargo test --locked --workspace --no-fail-fast | 3721 通过/1 失败（cancelled_batch_skips——干净树复跑同样失败，既有并行环境抖动，与本次无关） |
| 最终全量 | cargo check --locked && cargo test --locked --workspace --no-fail-fast | 见 2026-10-29-final-verify.log（本目录） |

## 已知边界与未完成项（如实登记）

- **TUI 确认宿主未接线**（tui_host.rs permissions: None，已注释登记）：TUI 声明面不开放 request_permissions；行式 CLI 宿主为当前唯一确认界面。规格 M3 的 chat_driver 权限视图/事件为剩余工作。
- **性能验收（P16）未完成**：递归检查成本未做正式 p95/RSS 测量与阈值冻结（当前预算沿用 20000 项/2000ms 建议值；实测典型 deny 树≪该值）。M0 数字未外推。
- **comprehensive-testing / Security Testing / ce-code-review / ce-compound 门禁未运行**（本轮上下文耗尽前未及执行）——交付候选不含这些门禁的通过声明。
- P01–P17 大部分经上述用例覆盖，但 §5 实测栏未逐项回填（见规格）。
- 五协议适配走普通函数声明（024 模式），无原生托管审批类型——符合规格；native_model_apis 回归全绿。

## 第二轮补充（2026-10-29，门禁收口轮）

### 边界核查（任务 1）
- **安装目录→单文件 deny**：§1.3 文义为目录级。最终规则：位于固定运行时白名单树（/usr、/etc）之内的安装位置退为单文件 deny（usrmerge 下 bwrap 在 /usr/bin，目录级会遮蔽 shell/coreutils，与 §1.6 载荷可用性冲突；dpkg 合法硬链接 nlink=2 会使目录级检查每次误拒）；白名单之外保持**目录级** deny（契约文义完整保留，单测 install_dir_outside_runtime_keeps_directory_deny 锁定）。保护对象不变（应用/bwrap 二进制与策略本身）。**该项为对 §1.3 文义的必要调和，待用户确认**。
- **二进制部署前提**：§1.3「不从可被模型覆盖的工作区产物自再执行」为既有契约；新增显式检查 verify_exe_deployment（二进制位于 anchor/temp 之下→拒启 031），单测锁定。
- **拓扑限制**：内容位于自动敏感边界之下→快照拒绝（既有 deny 优先的推论，非新限制）；显式单测 snapshot_refuses_content_under_auto_deny；正例＝S140/S141 分离布局。

### TUI 宿主（任务 2，M3 补齐）
- 专用事件 DriverEvent::PermissionPrompt（chat_driver.rs）＋TuiPermissionHost（tui_host.rs）：详情经 Info 全量入转录（cwd/路径/网络含义/reason 过滤/永久拒绝提示），决定经问答视图构件收集（默认首项＝拒绝；Esc/取消＝拒绝；EOF/断开＝error）。typed 回执独立通道，一般问询答案不转授权（§1.8）。
- e2e S141（真实 TUI+PTY+bwrap）：声明→申请→j+Enter 允许→granted→批准面真实写入→第二次 Enter 默认拒绝→denied，全部屏幕与请求体断言通过。S140 同步适配专用安装目录布局，两测 2/2。

### P06(b)/P16 冻结（任务 3）
- 测量（p16_launch_check_perf_and_budget_freeze，样本落盘 2026-10-29-p16-launch-check-samples.txt）：生产形态 permission_launch_argv（含递归 nlink＋seccomp memfd）p95 中位轮 **531µs**（三轮 523/531/727）；1000 文件 deny 树递归遍历中位 **1080µs（≈1.08µs/项）**。
- 冻结：遍历预算维持 **20000 项/2000ms**（20000 项预计 ≈22ms，超时余量 ≈93×≥2–3× 要求；条目预算为部署 deny 树规模上限）；启动检查回归阈值冻结 **50ms**（覆盖预算内最大合法 deny 树＋数倍余量）。单并发口径（§1.8）。
- 预算覆盖性：verify_deny_boundaries 对每个 readdir 目录项计数与检查（含 mountinfo 别名枚举在挂载层），预算超限/枚举失败/超时均拒启且零执行（单测＋集成锁定）。

### 安全攻防（任务 4 门禁，报告：docs/security/reports/2026-10-29-031-request-permissions-security.md）
- **H1（高）已修复**：毒丸快照下 apply_patch 曾回退 033 全盘策略（turn.rs valid() 折叠「未启用/毒丸」）——修复为三分支（快照/毒丸显式错误/非 031 原路径）＋ install_from_env 启动期基线预检（拓扑型毒丸启动即失败）；回归＝attack04 断言反转（探针文件保持旧内容＋显式错误）。
- **L1（低）已修复**：deny 命中的申请（已授权祖先之下）曾静默 granted——execute_request_permissions 增加永久拒绝优先检查（命中＝整次 error 不弹窗）；回归＝attack05 断言反转。
- **L2（低）已修复**：seccomp 清单补 unshare(272)（与 clone-ns 掩码同意图）；回归＝attack06 unshare_newuser 断言 EPERM。
- 攻防脚手架 src/app/tests/security_attack_031.rs（7 用例）保留为安全回归集（--test-threads=1 全绿）。
- 其余类别（参数伪造 32 形态/路径攻击/越权读取/嵌套逃逸/生命周期/部署前提/信息泄露）实测未发现，逐项证据见报告。

### 第三轮：ce-code-review 及修复（任务 4 收口）
- 8 审查者（correctness/security/adversarial[进程内兜底：容器缺 node/acpx]/testing/maintainability/reliability/agent-native/project-standards）→ 37 条输入 → 合并 20 primary（P0×1/P1×4/P2×9/P3×6）→ 验证批 8：confirmed 7 / rejected 1（#6 单文件拆分——规格 M1 点名单文件，契约优先）。报告 verdict：Not ready → 修复后复验。
- 修复 17/19（Fix order 执行）：#1 基线键改 frozen()（P0）、#5 信任路径 canonicalize、#12 路径单读、#3 mountinfo 分隔符扫描（含跨挂载别名投影回归）、#2 收尾失败闩锁＋CLI/TUI 双宿主门、#16 切换提交点回收、#4 旧 shell 载荷读循环＋存活标记、#8 删恒真、#11 确认前 64 上限预检＋断言翻转、#9 P16 样本落临时目录、#26 统一规范化错误文案、#27 三面逐启动 ns 验证、#10 攻防回归集头注释＋gcc 守卫、#20 反向注释、#21 规格状态行、#18/#19 死代码删除。
- 显式延后（非阻塞，待后续）：#7 sandbox.rs 031 段拆 permission_launch.rs（机械重构）、#15 嵌套授权遮蔽（需确认 base 显式 ro carve-out 语义取舍）。
- 新增回归：mountinfo 可选字段解析、跨挂载别名投影、无宿主基线强制、reap 门语义、旧 shell 有效性正例。
- 受影响面复验：request_permissions 36/0、request_permissions_sandbox 13/0、security_attack 7/0、sandbox lib 37/0、turn lib 13/0、e2e s140/s141 2/0；最终全量见 2026-10-29-final-verify2.log。

### 最终全量验证（第二轮，修复后）
- `cargo check --locked` 通过；`env -u TCHAT_SANDBOX cargo test --locked --workspace --no-fail-fast`＝**3740 通过/1 失败/3 忽略**（日志 2026-10-29-final-verify2.log）。
- 唯一失败＝turn::tests::cancelled_batch_skips…（既有并行抖动：同名单测隔离复跑通过；第一轮已在 git stash 干净树复现同失败，与本轮改动无关；失败形态与历轮一致，无新失败形态）。
- 收口材料：2026-10-29-candidate-manifest.txt（路径白名单＋核心文件 sha256 前 16 位）。

## 第三轮（2026-10-29）：剩余语义决策与验收缺口收口

### 1. 安装 deny 调和——决策材料

> **裁决（2026-10-29 第四轮）：采用方案 A**——白名单内安装对象单文件 deny／白名单外目录级；策略目录、显式 deny 及其他敏感边界原规则不变；全部点名保护对象不遗漏；nlink/bind 别名/逐启动复查保护不变。§1.3 已按裁决更新；实现自第三轮起即按此形态，无追加代码改动或重复测试。以下为裁决所依据的材料原文。

**原规则（§1.3 文义）**：「应用还必须自动合并……**策略与应用/bwrap 安装目录**……等安全边界」——自动敏感边界按**安装目录**整体 deny（目录级：目录及全部内部条目不可读、不可写，递归 nlink 检查覆盖内部文件）。§1.3 同段定义的保护对象为「包含内部执行入口的应用二进制、bwrap、必要的后端依赖和构建清单……来自固定的不可写安装位置」。

**拟议/已实现规则**：安装位置位于固定运行时白名单树（/usr、/etc——usrmerge 部署下 bwrap 即在 /usr/bin）之内时，应用二进制与 bwrap 退为**单文件 deny**；白名单之外的安装目录（如 /opt/tchat）保持**目录级 deny**（契约文义完整保留，回归 `install_dir_outside_runtime_keeps_directory_deny`）。策略文件所在目录**无论位置一律目录级**（策略目录是专属目录，非共享运行时树）。

**代表性路径对照**：

| 路径 | 原规则 | 已实现规则 | 实际权限变化 |
|---|---|---|---|
| /usr/bin/bwrap（bwrap 本体） | 目录级 deny /usr/bin → 不可读 | 单文件 deny /usr/bin/bwrap → 不可读 | **无变化**（保护对象本身仍全遮蔽） |
| /opt/tchat/terminal-chat（应用二进制，白名单外） | 目录级 deny /opt/tchat | 目录级 deny /opt/tchat（不变） | 无变化 |
| 策略目录（任意位置） | 目录级 | 目录级（不变） | 无变化 |
| /usr/bin/perl 等 dpkg 共享文件（非保护对象） | （字面上）随 /usr/bin 整体不可读 | 保持只读可读（ro-bind 白名单的一部分） | 可读性变化，但**原规则在该形态下从未可运行**（见下） |

**为什么判定为既有契约内的澄清而非保护范围缩小**：
1. **§1.3 点名的保护对象全部保留**：应用二进制、bwrap（单文件 deny＋策略目录目录级）的可读性遮蔽不变；沙箱内 /usr、/etc 本就 ro-bind——**可写性恒为零**，deny 的增量只在读面。
2. **原规则在 usrmerge 下从未可运行**：目录级 deny /usr/bin 会使受限读树失去 shell/coreutils（§1.6「载荷必须能运行」的受限读树设计要求 /usr 只读可读）；且 dpkg 合法硬链接（/usr/bin/perl↔perl5.x，nlink=2）使 P06(b) 目录递归 nlink 检查**每次启动必拒**（e2e S140 实证）——字面执行＝031 在 Debian/Ubuntu usrmerge 上完全不可用，与 §1.6「隔离创建失败必须在模型命令执行前拒绝」的可运行验收矛盾。
3. **别名/链接保护对单文件 deny 完整生效**：deny 文件本体 nlink>1 拒启（B1 推广）、mountinfo 同设备 bind 别名逐挂载实例 carve（运行时白名单与内容挂载两处）、每次启动复查。目录递归 nlink 检查不适用于单文件形态（无内部条目），其保护的「目录内文件硬链接进允许树」变体在单文件形态下由本体 nlink 检查等价覆盖。

**残余差异（如实列出）**：/usr、/etc 内与二进制同目录的其他文件（如 dpkg 共享库/工具）在原规则字面下应被 deny 而现已可读——但这些对象 (a) 不是 §1.3 点名的保护对象；(b) 是任何非特权进程本就可读的发行版公共文件；(c) 沙箱内不可写。管理员若要求整个私有安装树不可读，部署到白名单外目录即恢复目录级语义（正例：S140 布局）。

**备选方案（供选择）**：
- **A（推荐，现状）**：白名单内单文件、白名单外目录级。保护对象不变，usrmerge 可用。
- **B（严格文义）**：一律目录级。后果：Debian/Ubuntu usrmerge 上 031 无法启动（dpkg 硬链接必拒）＝功能在主流部署上不可用；需自行将 bwrap 拷出 /usr（破坏「固定安装位置」与来源可追溯）。
- **C（目录级＋包管理硬链接豁免）**：目录级 deny 但跳过 nlink>1 的包管理文件。后果：P06(b) 变体 A（deny 目录内文件预存硬链接到允许树）在 /usr 场景重新打开——削弱已闭合的反例，**不推荐**。

### 2. 审查 #15——判定为规格内缺陷，已修复

**发现原文**（ce-code-review #15，correctness，P2）：「只读授权嵌套于可写授权之下时，挂载遮蔽使已批准写面静默只读」——同一轮先授予子目录 read、再授予父目录 write（或 base 含子目录 read 且获授父目录 write）时，快照按深度升序挂载：父目录 rw 先挂、子目录 ro-bind 后挂并遮蔽父面，用户明确批准的父目录写入在子树内实际 EROFS；`grants.covers` 按逻辑授权集判「已覆盖」，再次申请无法修复。

**父/子路径授权与实际权限对照（修复前）**：

| 路径 | 逻辑授权（grants） | 实际挂载（修复前） | 偏差 |
|---|---|---|---|
| /a（父，write） | 读＋写 | rw-bind | 一致 |
| /a/sub（子，read） | 读（被父 write 蕴含） | ro-bind 后挂遮蔽父面 → /a/sub 内**只读** | 已批准的父写权限在子树内静默降级 |

**规格判定依据（明确要求，非未定义）**：§1.4「write 包含读取及该路径内文件变更能力；目录授权递归」＋「用实际路径包含关系判断覆盖」＋「同一路径 write 覆盖 read」——授权模型单调累加（write ⊇ read），access 不互斥；§1.5「已被……当前有效授权完全覆盖的合法请求直接返回 granted」——write /a 之后的 read /a/b 请求判「已覆盖」；§1.6「路径批准对象与实际挂载/读写对象须绑定」「不能表达的策略不得静默降级」。read 在本模型中不构成「只读 carve-out」语义（收窄原语只有 deny：§1.3「用户批准祖先目录也不能冲掉更窄的拒绝」，deny 祖先/可写后代组合由 §1.6 点名要求可表达）。故按缺陷修复。

**修复**：`build_snapshot` 在收集 read/write（base＋grants）后化简——read 路径等于或严格位于任一 write 路径之下时不进快照（挂载面与逻辑授权集同构）。永久 deny 优先不受影响（deny carving 独立于 read/write 面进行）。

**回归与证据**：单元 `nested_read_under_write_reduced_in_snapshot`（授予顺序、base-read 形态、同路径覆盖、兄弟 read/write-under-read 对照不化简）；真实 bwrap `nested_read_grant_does_not_mask_approved_write_face`（子目录内写入成功＝EROFS 反例闭合）；变异证据＝注释化简行后两用例均失败（行为测试输出修复前挂载面 read=["…/export/sub"]），恢复后通过。

### 3. 待测项处理

- **P02**：补 `verify_bwrap_backend`（§1.3「不通过工作目录或模型 PATH 查找」的运行时执行）：bwrap 缺失或位于工作区/临时目录之下→**启动期**明确失败拒绝启用 031（此前缺后端要到首个工具调用才逐次失败）。单测覆盖四分支（缺失/工作区/临时目录/固定位置通过）。既有证据沿用：.env 不能注入信任根（TRUST_CONTROL_VARS＋用例）、策略位于工作区/临时目录/自身可写面之下拒绝（load_verified）、应用二进制工作区产物拒绝（verify_exe_deployment＋e2e S140 布局）、缺 TCHAT_SANDBOX 拒绝（用例在案）。**仍待测（环境受限，未降级）**：真实环境部署矩阵（内核不支持 userns 的裸机/发行版差异、bwrap 版本矩阵、PATH 被重排为非固定二进制）；bwrap 版本/SHA-256 运行时校验未实现——按 §1.6「可追溯」以 033 部署记录（固定 /usr/bin/bwrap＋SHA-256）为部署材料，属验收人判定项。
- **P15**：补 e2e S142（真实 TUI＋PTY＋bwrap 全链）：会话 1 允许授权→exec 落盘→/save；会话 2 /resume——历史以结果 JSON 呈现路径/access/scope=turn（§1.8「仅保存普通 ToolResult」，实时 Info 卡不入会话）；恢复后同路径申请**重新弹确认**（默认拒绝）；exec 写 proof2 落回基线失败（proof2 不产生）；mock 配对断言 s142-r2=denied、s142-e2 输出无标记。变异证据＝`grants.covers()→true` 时用例失败（首次申请即静默 granted、无弹窗）。/resume 不执行工具、不重建授权由 024/007 既有机制保证。
- **P16**：补完整启动（faces 分别：exec pipes／exec PTY／apply_patch／view_image；生产形态组装＋spawn＋bwrap execve＋载荷退出）与峰值 RSS（RUSAGE_CHILDREN 跨面上界）测量与冻结阈值（§1.8 补冻结行；样本 2026-10-29-p16-full-launch-rss-samples.txt）。**计量有效性教训**：载荷 chdir 落在未挂载面时 bwrap 快速失败 ≈6.5ms 恒定，无成功断言会把失败样本当基线（补丁面已加 r.ok 断言＋305 文件计数防空跑）。**三个验收对象互不替代**：启动前检查（50ms，p95 531µs/重跑 521µs）、完整启动（faces 30ms/250ms）、峰值 RSS（128MiB）各自有独立证据与门禁。
- 531µs/50ms 口径核对：50ms 阈值的测量对象＝启动前检查（permission_launch_argv 组装含 P06(b) 递归检查与 seccomp memfd 构造，**不含 spawn**），与 §1.8 首条冻结行一致；归档样本文件（2026-10-29-p16-launch-check-samples.txt）为终验重跑输出（521µs），与冻结时测量（531µs）同形态波动 ≈2%，已在证据文件与规格中注明，阈值判定不受影响。

### 4. 变更清单（第三轮）

- 代码：src/app/src/permissions.rs（#15 化简＋`verify_bwrap_backend`＋install_from_env 接线）。
- 测试：request_permissions.rs（nested_read…、bwrap_backend…）、request_permissions_sandbox.rs（nested_read…真实 bwrap、p16_full_launch_faces_and_peak_rss）、e2e_acceptance.rs（S142）。
- 文档：规格 §1.8/§3.5/§5、本记录、P16 完整启动样本、rules/project-lessons.md（2 条新增）、manifest 追记。
- 受影响面复验与最终全量：见下方 final-verify3。

### 5. 最终全量验证（第三轮，本轮改动后）

- `cargo check --locked` 通过；`env -u TCHAT_SANDBOX cargo test --locked --workspace --no-fail-fast`＝**3745 通过/1 失败/3 忽略**（日志 2026-10-29-final-verify3.log）。
- 与第二轮（3740/1/3）之差＝本轮新增 5 用例全部通过；唯一失败仍为 turn::tests::cancelled_batch_skips…（既有并行抖动：隔离复跑通过，失败形态〔同用例、同 panic 位置〕与历轮归因一致，无新失败形态）。
- 受影响面定向复验（全量前）：request_permissions 38/0、request_permissions_sandbox 15/0、security_attack_7/0、sandbox lib 37/0、e2e s140/s141/s142 4/0。

## 第四轮（2026-10-29）：方案 A 落定＋P02 最终契约核对（仅文档，无代码改动）

### 1. 方案 A 落定

- 规格 §1.3 安装边界句按裁决更新（白名单内点名安装对象单文件 deny／白名单外目录级／策略目录一律目录级；点名对象当前形态＝应用二进制＋bwrap，未来同部署后端依赖/构建清单一并列为安装对象；nlink/别名/逐启动复查两种形态同等生效）；§3.5 状态由「待用户决策」改为「已裁决」。第三轮 §1 材料顶部加裁决注记。
- 实现自第三轮起即按此形态（deny_context_for 白名单内外分级＋策略目录恒目录级），**无追加代码改动或重复测试**；final-verify3（3745/1 既有抖动/3）证据沿用。

### 2. P02 契约核对（按 a/b/c 三类区分）

**原条款**：§1.3「包含内部执行入口的应用二进制、bwrap、必要的后端依赖和构建清单同样来自固定的不可写安装位置，不通过工作目录或模型 PATH 查找；031 模式不从可被模型覆盖的工作区产物自再执行」；§1.6「应用/bwrap、必要依赖的版本、SHA-256、目标平台、适配补丁和许可证须**可追溯**，不采用未经固定的 PATH/最新版二进制」；§5-P02「.env/项目配置试图控制策略，策略/应用/bwrap可写、未知版本、缺TCHAT_SANDBOX或扩展后端、内核不支持 → 启动031 → 明确失败且零模型命令执行；不退回…不改变管理员配置」。

**a. 部署时记录/核验（部署材料）**——规格要求的是**可追溯**（记录），非逐次运行时摘要校验：
- 已有：033 部署链以发行版包固定后端（镜像构建装 Ubuntu `bubblewrap 0.9.0-1ubuntu0.3`；`verify-033-deploy.sh` 断言 `bwrap --version` 存在；宿主脚本含引擎版本守卫）；部署记录（docs/logs/033-tool-sandbox/2026-10-27-dev-deploy.md）记版本 0.9.0 与注入断言。
- **缺项（本轮补记，属记录补全而非新功能）**：记录中无 bwrap 二进制 SHA-256。当前实际后端核验：`/usr/bin/bwrap`，bubblewrap 0.9.0（dpkg 0.9.0-1ubuntu0.3），sha256＝`e318903862396f96de3df57264e0158682b952fd3fb53ac23d876413e7b30f71`，nlink=1，root:root 0755（uid 1000 不可写），linux/amd64。此为 031 验收材料的后端身份记录；033 历史记录不改写。
- 应用二进制：验收环境（dev 容器）内为工作区构建产物，031 部署前提按 §1.3 以「不从工作区产物自再执行」的运行时检查＋e2e 专用安装目录形态（S140/S142）执行；正式部署材料沿 033 镜像链。

**b. 应用必须自动执行的运行时校验**——按条款逐项核对实现状态：
- 策略可信来源：`load_verified`（绝对路径/常规文件/1MiB/不在工作区、临时目录、自身可写面之下；canonical 比较）；.env 不能注入信任根（TRUST_CONTROL_VARS）；缺 `TCHAT_SANDBOX` 拒绝启用——均已实现＋用例。
- 应用二进制：`verify_exe_deployment`（工作区/临时目录产物拒启）；bwrap：`verify_bwrap_backend`（缺失或位于工作区/临时目录之下拒启，第三轮补）——「策略/应用/bwrap 可写→明确失败」以模型可写面（工作区、临时目录、base/requestable 可写面）为判定集合：同 UID 操作者自有的其他可写位置不属模型威胁面（模型写面＝base＋grants，自引用检查已覆盖）。
- 「缺扩展后端/内核不支持」：`probe_bwrap` 非仅查存在——以 ro-bind＋unshare-user＋cap-drop 最小集**真实执行** bwrap（「存在不等于可用」），userns 不可用内核/损坏后端→探测失败→启动期拒绝启用 031（第三轮接线）。本容器实证通过；跨环境行为属 c 类。
- 「未知版本→明确失败」：冻结策略格式 v1（§1.3）**无后端版本/摘要固定字段**，§1.6 要求为可追溯（a 类）；故不构成逐次运行时摘要校验需求，本轮不新增（按指示「不得额外新增逐次运行时校验要求」）。运行时防线为：来源位置检查（上列）＋探测真实可用＋逐启动 P06(b)/seccomp/PID-ns 复查。
- **判定：无必需运行时功能缺项**；缺项仅为 a 类记录（bwrap SHA-256），已补记。

**c. 跨内核、发行版、bwrap 版本的环境验证矩阵**——未测，保持待测（不降级）：
- 已验证环境（验收结论实际覆盖范围）：WSL2 容器（内核 6.18.33.2-microsoft-standard-WSL2）、Ubuntu bubblewrap 0.9.0-1ubuntu0.3、同宿主 UID 1000、userns 可用、debug 构建载荷（性能/RSS 口径）。
- 未覆盖：userns 受限内核/SELinux/AppArmor 约束主机、其他发行版与 musl、bwrap 其他版本（0.10+ 等）、release 构建载荷性能。

### 3. 本轮变更清单（全部为文档）

- docs/specs/031-request-permissions.md（§1.3 方案 A 落定、§3.5 状态）、本记录（第三轮 §1 裁决注记＋第四轮节）、rules/project-lessons.md（+1 条裁决记录）、manifest 追记。
- 仅文档调整：内容与链接已检查；复用 final-verify3 证据，未重跑代码检查/全量测试（无代码变更）。

### 4. P02「未知版本」对象核对（2026-10-29 第五轮，微收口；仅记录，无代码/测试变更）

- **版本对象判定**：§1.3（deny_paths 段后策略规则条）是规格中唯一把「未知版本」定义为报错条件的条款——「同一路径互相矛盾的重复规则、重复 JSON 键、未知字段、**未知版本**、不可执行的规则、可写策略来源均报错，不默默忽略」，其对象为**策略格式版本**（首版固定 version=1）。§5-P02 的「未知版本→启动失败」按该定义对应策略格式版本；后端（应用/bwrap）版本按 §1.6 属部署可追溯记录（第四轮 a 类），不构成逐次运行时判定，亦不新增后端固定字段。
- **对应关系（版本对象→拒绝入口→负例证据）**：策略格式版本 → `Policy::parse` 版本门（permissions.rs:391-396，非 1 即「策略版本不支持」）→ `load_verified`（permissions.rs:427 传播）→ `install_from_env`（permissions.rs:900）→ `main.rs:75-81` Err 即 exit(1)，发生在 CLI/TUI 分流与任何模型/工具执行之前（零执行为结构性保证）。负例：`policy_rejects_wrong_version`（version=2 拒绝，final-verify3 通过）；同链 Err→exit 语义另证：`policy_env_assembly_gates`（缺 TCHAT_SANDBOX 装配失败）、`policy_load_rejects_*`（信任来源逐类拒绝）；工具面 fail-closed：`poisoned_snapshot_zero_execution` 等。未知版本×e2e 直连组合无独立用例——同一 Err→exit 代码链路，无新代码路径，不补重复测试。
- **三类区分**（维持第四轮结论）：版本判定＝策略格式版本（运行时，已实现）；部署可追溯＝应用/bwrap 版本与摘要（记录，bwrap SHA-256 已补记）；逐次 SHA 校验＝规格未要求，不新增。
- **功能缺项：无**；final-verify3（3745/1 既有抖动/3）仍有效（本轮零代码变更）。
