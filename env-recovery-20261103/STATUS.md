# 环境恢复状态账本（四里程碑）— 2026-11-03

方案与脚本见同目录 README.md / recover.sh / verify.sh。按用户要求分别记录
「准备完成、备份完成、重启完成、核验完成」，每项附实际证据。授权状态：
**保全核验通过后原地重启一次的授权保持有效，本账本不重复申请**。
备份/重启/核验三节的证据槽由宿主执行者运行脚本后落位；脚本会把里程碑行写入
备份目录 manifest.txt 与 post-restart-verify.txt；verify.sh 末尾把证据归档到
`docs/logs/2026-11-03-env-recovery/`，归档后方可清理本目录。

## 里程碑 1：准备完成 ✔（2026-11-03，本会话）

核对通道：无 fork 文件直读（Read/Edit/Write 工具）；本会话未运行任何 Bash、
未在容器内创建任何进程、未向 /workspace 除本目录外写入任何文件。

### 1.1 脚本核对问题清单（11 项，均已在原授权范围内修正）

| # | 级别 | 问题 | 处置 |
|---|---|---|---|
| 1 | 阻断 | exec 不可用分支声明「请人工确认后手动执行第 5 步」，但脚本继续自动重启——言行矛盾且无一致性证据即重启 | 无一致性证据（cp 双读＋pids 采样）即阻断；有证据才按既有授权重启 |
| 2 | 阻断 | docker export 提取用 `--wildcards` 但缺 `--wildcards-match-slash`：GNU tar 提取默认不跨 `/`，`tmp/wt-038-f1/*` 只匹配一层，深层文件被静默跳过且退出码 0（残缺备份假成功，仅靠关键项 grep 兜底） | 显式 `--wildcards-match-slash`＋双形态模式＋目录存在性断言 |
| 3 | 高危 | 根目录 `.env` 实存（本会话直读证实，含 1 个真实 API 密钥，值不记录）且因 .gitignore 不入 git status；原排除仅精确根名 `.env` | 扩为任意深度 `.env`/`.env.*`；新增 `.container-state`/`.claude`/`.compound-engineering`/`.context`/`src/app/config` 排除（依 .gitignore 语义）；tar 后泄漏断言防通配符失效 |
| 4 | 高危 | `*.pem`/`*.key` 通配可能漏掉非敏感测试夹具证书/密钥（`src/app/tests/fixtures` 目录经存在性探测证实存在） | 2c 步 `git ls-files` 枚举被排除清单中的**已跟踪**文件并从对象库回填备份副本；verify.sh §5 活环境抽查；未跟踪清单（git status -uall）中无任何 .pem/.key/.env 条目，无未跟踪夹具可漏 |
| 5 | 高危 | 备份源 SRC 硬编码但从未与 docker inspect 实测 bind 源比对——可能备份错误目录而不自知 | 第 0 步归一化比对（Windows 形式↔/mnt 形式），不符即阻断 |
| 6 | 中 | wt 备份缺失仅提示不阻断；sec-test-038 缺失「可接受」与 README §3「必须保全」矛盾 | wt 缺失显式 die；sec-test-038 未取得即阻断并附 /tmp 无 fork 枚举清单（docker cp 流）供人工裁决 |
| 7 | 中 | exec 失败时 pre 文件只含 docker 报错文本，verify.sh 直接 diff 出误导性 DIFF/FAIL | pre/post 有效性门（40 位哈希格式／报错文本检测）；无效转 SKIP 或改用备份副本比对；重启后 exec 仍不可用如实报核验缺口 |
| 8 | 中 | verify.sh 在 pre 缺失时仅提示「以备份副本对照」却不实际比对 | 实际执行 diff post vs `status-*-copy.txt` |
| 9 | 低 | docker cp 未纳入通道（用户指示优先检查；它不依赖容器内进程，正是 EAGAIN 场景正解） | 通道层级改为 docker cp → exec tar → docker export，所用通道登记 manifest；export「不含 bind 挂载（/workspace）数据、成功≠两处备份完成」语义写入脚本与 README §5 |
| 10 | 低 | exec 不可用时一致性完全依赖「会话已暂停」假设 | pids 采样×2＋备份窗口首末双读逐字节比对＋重启后 post vs pre/副本比对，全部落盘 |
| 11 | 低 | 证据只在 `~/agentlab-recovery-*`（容器外 ext4），不符 AGENTS.md「证据入 docs/logs」且本目录拟清理 | verify.sh 末尾归档全部证据至 `docs/logs/2026-11-03-env-recovery/` |

### 1.2 关键事实无 fork 实证（本会话宿主侧直读）

- `.git/config`：`filemode = false`、无内嵌凭据、origin 无 token（佐证 README §2）——备份副本 status 不会因 drvfs 权限位产生模式噪声；
- `.git/worktrees/wt-038-f1/HEAD` 存在且＝`57d5408cf2c6…`：链接 worktree 元数据确在 bind 源上，workspace tar（含 `./.git`）必涵盖，recover.sh 关键项断言二次校验；
- `docs/specs/038-github-actions-ci.md` 状态行在案（038 未完成、六项未决、更新时间 2026-11-02）——038 验收结论不因环境恢复改变；
- `docs/plans/2026-11-02-0044-feat-web-access-plan.md` 实存（任务 A 未跟踪清单抽样核验）；
- 根目录 `.env` 实存、`src/app/tests/fixtures` 目录实存（见 1.1 #3/#4）。

### 1.3 任务 A（/workspace）文件清单——以当前磁盘内容为准

采集机制：本会话启动时宿主 git status 快照（`-uall` 语义，未跟踪文件单列，含中文名八进制转写）；git status 不含被 .gitignore 忽略的文件，其处置见下方忽略类条目。**快照时点后本目录新增 STATUS.md，其余无写入**；最终权威清单＝recover.sh 实测 `status-ws-copy.txt`（备份副本 status），两者应可逐条对上。

已修改（16）：
`.devcontainer/033-seccomp.json`、`.devcontainer/compose.yaml`、`.devcontainer/deploy-033-host.sh`、`AGENTS.md`、`docs/logs/038-github-actions-ci/2026-11-02-iteration32-status.md`、`docs/specs/038-github-actions-ci.md`、`src/app/src/apply_patch.rs`、`src/app/src/js_orchestration/runtime.rs`、`src/app/src/js_orchestration/worker.rs`、`src/app/src/mcp/transport.rs`、`src/app/src/proc_session.rs`、`src/app/src/sandbox.rs`、`src/app/tests/apply_patch_sandbox.rs`、`src/app/tests/exec_sandbox.rs`、`src/app/tests/js_orchestration_security.rs`、`src/app/tests/proc_session.rs`

未跟踪（11 个文件＋本目录）：
`docs/logs/033-tool-sandbox/2026-11-02-engine-2992-baseline-regen.md`、`docs/plans/2026-11-01-next-three-features-plan.md`、`docs/plans/2026-11-02-0044-feat-web-access-plan.md`、`docs/records/2026-11-01-039-web-access-codex-comparison.md`、`docs/think-tank/2026-11-01-039-web-access.md`、`docs/think-tank/2026-11-02-039-stream-api-host.md`、`docs/think-tank/2026-11-02-039-stream-sse-event-types.md`、`docs/think-tank/raw/`（6 个：`2026-11-01-039-web-access-参考实现考古.md`、`-http与ssrf.md`、`-内容提取.md`、`-搜索后端.md`、`-项目集成.md`、`-侦察兵.md`，中文名已由 think-tank 汇总文档索引交叉证实）、`env-recovery-20261103/`（4 个文件）

被 .gitignore 忽略、不进 git status、按排除策略不入备份（1.1 #3）：
`.env`（含 1 个 API 密钥）、`target/`、`src/app/config/`、`.container-state/`、`.claude/`、`.context/`、`.compound-engineering/`、`.devcontainer/033-rollback/`（均以实存为准，由泄漏断言兜底）；`.agents/`（项目技能与代理，非敏感）**随全量保留**。其中 `.env` 密钥若备份外无副本，属排除策略固有损失，用户知悉即可。

### 1.4 任务 B（/tmp/wt-038-f1）文件清单口径

`/tmp` 在容器 overlay 可写层，宿主不可直读——**任何宿主侧清单（含迭代 37 报告、README §3 参考清单）只能作补充**。权威清单＝recover.sh 3b 在备份副本上实测的 `status-wt-copy.txt`（副本自足化后 `git status --porcelain -uall`，脚本会把全文回显进 manifest）。参考清单（README §3，11 项）已在脚本中逐项交叉核对登记。迭代 37 报告（worktree 规格段＋安全报告）仅补充说明，不替代实测清单。

### 1.5 一致性措施（不依赖「会话已暂停」假设）

1. `docker stats` pids 采样×2（备份前/后，守护进程侧读取，不需容器内进程）——旁证；
2. 备份窗口首末双读逐字节比对：exec 可用时 pre/post `git status` diff；exec 不可用时 docker cp 双读 `diff -r`——主判据，任一显示写入活动即阻断重启；
3. workspace 侧：宿主 drvfs 直读＋重启后 verify.sh §3（post vs 保全前/备份副本 status）兜底；
4. 本会话自执行者启动 recover.sh 起暂停，不再写 /workspace 任何文件（归档由 verify.sh 在重启后完成）。

## 里程碑 2：备份完成 ⏳ 待执行者

证据槽（R2）：`~/agentlab-recovery-<TS>/manifest.txt`（含「【里程碑：备份完成】」行与全部 sha256）；保全条件证据＝`ws-double-read-diff.txt`／`wt-double-read-diff.txt`／`sec-double-read-diff.txt`（空差异＝逐字节一致）＋文件项计数（防空跑）、`fsck-workspace-backup.txt`、HEAD 双核验、泄漏断言（leak-scan/wt-leak/fixtures-leak 均空）、`fixtures-current.tgz`＋`fixtures-current.list`＋`fixtures-status.txt`；登记类＝`status-ws-copy.txt`／`status-wt-copy.txt`、`tracked-excluded.txt`／`tracked-review.txt`、`pids-pre/post.txt`。前置＝`validate-result.txt`（VALIDATION PASS）。
判定标准：recover.sh 退出码 0 且里程碑行在案；任一 die 即不重启。

## 里程碑 3：重启完成 ⏳ 待执行者

证据槽：manifest.txt 末段「重启前/重启后 ID 与 Status」（ID 未变、Status=running）＋「【里程碑：重启完成】」行。仅 `docker restart` 一次，不删除/不重建；重启失败保留现场不自动重试。

## 里程碑 4：核验完成 ⏳ 待执行者

证据槽（R2）：`post-restart-verify.txt` 全部必要检查 PASS（容器 ID 未变且 running、shell 恢复、两工作树 HEAD＝基线、status 与保全前/备份副本一致——成功执行且输出为空的 status 亦有效、候选抽查、`git fsck --full` 通过、夹具保全证据与活环境一致、备份产物齐全）＋「【里程碑：核验完成】」行；任一必要检查失败/缺失 → 判「核验未完成」且退出码非 0。证据归档 `docs/logs/2026-11-03-env-recovery/`。

## 交接注意

- 推送凭据仍缺失（README §2），环境恢复与推送解耦；038 六项未决与验收结论不变。
- 执行顺序：`bash recover.sh`（内含备份核验，通过才自动重启一次）→ `bash verify.sh`（核验＋归档）。
- sec-test-038 若确不存在，recover.sh 会阻断并给出 /tmp 清单；人工核实后按脚本提示调整判定再续。

---

## R2 第二轮修正（2026-11-03，用户六项指示）

本节修订并部分取代上文 1.1 表与 1.5 节的对应条目。

| # | 指示 | 修正 |
|---|---|---|
| R2-1 | 简化 tar 提取参数 | 全部改为「命名目录＋排除项」单一形式（无 ./ 双形态、无必须同时匹配的通配符清单）；recover.sh 第 0.5 步内置深层小归档自检：命名目录递归提取完整、排除生效、缺失成员退出码非 0，任一不成立即阻断 |
| R2-2 | 修正一致性判断 | git status 降级为登记，不作保全条件；保全条件＝workspace／wt-038-f1／sec-test-038 内容双读逐字节比对（同一源两次独立读取＋文件项计数下限防空跑）；写入控制＝可选人工 `docker pause`（脚本识别 paused、重启前 unpause）；重启后 verify 仅作回归检测，不补充重启前条件 |
| R2-3 | 夹具当前内容与删除状态 | 废弃 git checkout 回填：已跟踪非敏感夹具经 ls-files 枚举，当前内容独立保全于 `fixtures-current.tgz`（逐字节 cmp 复核），删除状态登记于 `fixtures-status.txt`；副本 status 如实显示 ' D'（排除结果），**不宣称副本＝原工作目录准确备份**；`.env`*/凭证目录类已跟踪文件仅入 `tracked-review.txt` 人工判断；排除在各通道（workspace tar／cp／exec／export 提取侧）统一落实并有泄漏断言 |
| R2-4 | 删除尾部相似放行 | 挂载源对应须证实：金丝雀经容器视图回读，或归一化路径相等，二者取一；均不成立即阻断 |
| R2-5 | verify 判定与退出码 | 检查命令失败＝FAIL、必要检查缺失＝FAIL（不判核验完成）；成功执行且输出为空的 status 有效（.ok 标记机制）；FAILS>0 → exit 1 |
| R2-6 | verify 显式目录／重启完成确认 | verify.sh 必传备份目录参数（缺失/无效 exit 2），不自动选最新目录；recover.sh 重启后仅当 ID 未变且 Status=running 才登记重启完成 |

**模拟验证**：`validate_recovery.sh` 在 mktemp 临时目录内以真实 git＋模拟 docker（shim）运行 16 场景：正常路径／空 status 有效性／写入者内容变化检出／exec 全程不可用（cp 通道）／仅 export 通道／状态读取失败／fsck 失败／sec-test 缺失／关键文件缺失／基线不符／挂载源无法确认／挂载源形式不同经金丝雀确认／重启后 ID 变化／人工 pause 流程／重启后漂移检出／未验证拒跑（另含 verify 参数误用）。全部通过才写 `validate-result.txt`（VALIDATION PASS），recover.sh 以此为**强制前置门**——未验证即对真实目标运行会被脚本自身拒绝，这也落实「不得对真实目标试运行整套恢复流程」。运行证据＝`validate-summary.txt`（逐项 PASS/FAIL）；各场景明细在临时目录内随收尾清理，结论折叠进 summary。

**执行顺序**：validate_recovery.sh → recover.sh → verify.sh（verify 显式传入 recover 输出的备份目录路径）。本会话无 Bash 执行能力，模拟验证由宿主执行者运行；「修正并回传验证证据」＝validate-summary.txt 落盘后回传，其后方可执行真实保全。

---

## R3 实际执行记录（2026-11-04，宿主侧接管执行）

### 模拟验证（最终有效轮）

`bash validate_recovery.sh .` **exit=0**；`validate-result.txt`＝`VALIDATION PASS 2026-11-04 00:57:33 scenarios=18 checks=81`；`validate-summary.txt` 0 失败。pause 降级语义修改后按规则重验，仍全绿。调试期修复并逐项经模拟回归：recover.sh `}` 参数位置语法错误；0.5 自检 `tar -C` 误指；**dk 包装**（`MSYS_NO_PATHCONV` 全局 export 会打断 git.exe 的 `-C /tmp/…` POSIX 路径解析——只允许对 docker 调用前缀屏蔽）；3b worktree 指针需 `cygpath -m` 原生形式（POSIX 形式实测 `gitfile does not point to a valid repository`）；harness shim inspect 未跳 `-f`、mapargs 双重映射（workspace 替换产物再撞 /tmp 前缀）、S08 只读对象 dd 静默失败需 chmod、S11/S12 断言目标 stderr→stdout、S15/S16 过期 needle。

### 实际执行（宿主 Git Bash＋docker 29.9.2 直连锚点容器）

- 第 1 次（`~/agentlab-recovery-20261104-005756`）：`docker pause` 失败——`runc did not terminate successfully: pthread_create failed: Resource temporarily unavailable`（与容器内 fork EAGAIN 同族）→ 旧逻辑阻断；容器未被误冻。随后 pause 改为降级语义（可选强化，失败不阻断，登记后继续）。
- 第 2 次（`~/agentlab-recovery-20261104-010320`，workspace-full.tgz sha256=`96419025bcc3f68d3a113f1bf6037012d9cadbffb3bd40030e182496ec30a4f1`）：**workspace 侧保全完成且核验全过**——备份副本 HEAD＝57d5408 ✓；`git fsck --full` 通过（61s）✓；泄漏断言通过 ✓；workspace 双读逐字节一致（ws-double-read-diff.txt 空）✓；夹具 26 项 present／0 deleted，fixtures-current.tgz 独立保全并逐字节 cmp 复核 ✓；tracked-review 仅 `.env.example`（已跟踪模板，非真实凭据，副本如实显示 ' D'）✓；未提交/未跟踪 39 条登记（status-ws-copy.txt）✓。
- **/tmp 侧（wt-038-f1＋sec-test-038）未取得**：`docker cp` 连单文件也失败，daemon 报 **`Docker Desktop is unable to start`**；export 流同样损坏；exec 不可用（EAGAIN 家族）→ 按失败语义**阻断，未重启**（两次均正确阻断，未误冻、未升级为删除/重建）。**重启授权保留**，待 /tmp 侧备份达成后执行。
- verify.sh 未运行——重启未发生，此刻运行必然全 FAIL，无信息量；属如实缺口而非遗漏。

### 尚存缺口与下一步（需人工介入）

1. **Docker Desktop 后端已退化到流通道全灭**（exec/pause/cp/export 全部 EAGAIN 或 unable to start；workspace 的宿主直读不受影响）。需用户人工重启 Docker Desktop（GUI/服务级操作——超出本 Agent 授权，且与「容器原地重启一次」授权不同范畴）。
2. Docker Desktop 恢复后：重跑 `bash recover.sh`（幂等，重新完整保全 workspace＋/tmp 两侧；workspace 侧预期再次通过）→ 全过则按既有授权自动原地重启一次 → `bash verify.sh <该次输出的确切备份目录>`。
3. wt-038-f1 候选与 sec-test-038 在 overlay 可写层、容器未删除——重启 Docker Desktop 不销毁它们；风险仅在容器被删除/重建，维持「不删除不重建」纪律即可。

---

## R4 实际执行完成（2026-11-04，四里程碑全部达成）

**【里程碑：备份完成】✔** —— 最终备份目录 **`C:\Users\sry27\agentlab-recovery-20261104-052734\`**（recover-exit=0，05:31:41 登记）：
- workspace 侧：HEAD＝57d5408 ✓、fsck --full 通过 ✓、泄漏断言通过 ✓、双读逐字节一致 ✓、夹具 26 present／0 deleted ✓、未提交/未跟踪 39 条 ✓（sha256 见 manifest）；
- /tmp 侧：wt-038-f1 双读逐字节一致（1913 文件项×2，exec-tar 源侧排除 24GB 本地 target）、迭代 37 标记 2 处 ✓、安全报告在案 ✓；sec-test-038 7 项 ✓；
- 另存独立抢救副本 `C:\Users\sry27\agentlab-rescue-wtsec-20261104-033240\`（wt-038-f1.tgz sha256=`d2ce1b1f…`、sec-test-038.tgz sha256=`3ef8560e…`）与既有备份 `010320`（按指示保留）。

**【里程碑：核验完成】✔** —— `verify.sh`（显式传入 052734）**exit=0**：容器 ID 未变且 running、shell 恢复、两工作树 HEAD＝基线、status 与保全前一致（workspace 37 条／wt 17 条）、**内容回归 workspace live vs 备份逐哈希一致 1751 项／wt 1720 项**、迭代 37 段 2 处、安全报告与 sec-test-038 在位、`git fsck --full` 通过、夹具活环境在位、备份产物齐全（证据＝`post-restart-verify.txt` 全文，已归档 `docs/logs/2026-11-03-env-recovery/` 39 个文件）。

**容器：原地重启授权未消耗**——Docker Desktop 重启后环境即恢复（exec 正常、pids 23884→35），按指示 4 走 SKIP_RESTART 保全路径，容器保持运行、配置/挂载/凭据未动。

### 过程中的关键实证（已固化脚本与记忆）

1. **docker pause 使本容器后续 exec/cp 全部失效**（第 5/6 轮对照：pause 后即刻全灭；无 pause 的抢救轮 60 秒全通）→ 恢复流程默认不用 pause（RECOVERY_PAUSE=1 显式启用才尝试），写入控制＝协调停止＋双读辅助＋pids 采样；
2. **wt 内存在 24GB 本地 target**（全局 CARGO_TARGET_DIR 对 worktree 构建未生效的旁证）——docker cp 无排除能力会全量流读直至挂起 → 两步法（容器内 tar 源侧排除落盘 → 单文件 cp 取出，cygpath -m 目标路径）；
3. **docker.exe 客户端在容器侧进程退出后间歇性不退出**且被杀后 daemon 短暂无响应 → 所有 docker 调用 `timeout -k 15` 强杀＋以本地产物判定成功；dk 实现为真实脚本（timeout 不能包装 bash 函数）；
4. Docker Desktop 后端每次重启后约 10–13 分钟再次退化（本轮共重启 7 次，其中第 3–7 次为同一恢复授权范畴的解卡操作，逐次记录于 dd-restart-output.txt）；
5. MSYS 全局 `MSYS_NO_PATHCONV=1` 会打断 git.exe 的 `-C` POSIX 路径解析（只对 docker 调用屏蔽）；`git` 在本机实为 ugrep 同源的别名环境——提取脚本避免 grep 依赖。

### 尚存缺口（不阻塞环境恢复结论）

- verify §8 证据归档为部分成功（39 个文件入 docs/logs；部分 glob 未命中）——完整证据集在备份目录 `052734` 本体；
- 多个中间态备份目录（005756/020303/025825/035125/041857/045208/050022/rescue-012144）为失败/部分尝试残留，保留待用户处置（删除需用户决定）；
- 首次 verify 尝试曾因提取脚本 sed 贪婪匹配失败（exit 2），修正后重跑通过——verify 幂等；
- 容器内 `/tmp/.probe.tgz`、`.rescue.tgz` 残留（诊断产物，约 4–8MB），可在后续任务中随手清理；
- 容器进程数 35（恢复前 24268）——原 EAGAIN 病灶清除；若复发，按 R3 脚本既有降级/阻断语义重跑即可。
