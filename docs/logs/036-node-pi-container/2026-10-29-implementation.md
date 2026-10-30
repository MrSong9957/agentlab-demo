# Node/Pi/CC-Switch 配置与数据持久化实施

- 日期：2026-10-29；工作目录：`/workspace`。
- 本次授权：配置好 Dockerfile/Compose，并保存当前已有数据；不构建镜像、不重建容器。
- 代码范围：`.devcontainer/Dockerfile`、`.devcontainer/compose.yaml`、`.gitignore`、`.dockerignore`、`README.md`。相关规格和计划只同步用户后续指令，不记录执行进度。
- 原分支：`feat/035-multi-agent`。修改前已有大量 035 相关改动，全部保留；本任务不提交或发布。固定原始文件快照和原工作区状态保存在 `/tmp/node-pi-config-BPvOtk8D/`。

## 配置结果

- Node/npm 从官方 `node:24-bookworm-slim` 阶段复制必要文件；最终基础镜像仍为 `agentlab-base:latest`。
- 补齐 Node 所需 `libstdc++6`、`libatomic1`，npm 安装官方 Pi `latest`，使用 `--ignore-scripts` 与 `--engine-strict`。
- CC-Switch 使用用户给定官方脚本，下载与 Bash 执行通过 `&&` 串联；仅安装进程指定系统目录和非交互覆盖参数。
- 最终 USER 仍为 dev，系统程序路径优先，Rust/bubblewrap 与原配置保持。
- 数据选择宿主项目 `.container-state/` 绑定挂载，使当前无 Docker 引擎时也能直接保存实际数据。已有 7 个挂载、原环境变量与命名卷声明未变。
- 数据目录同时排除在 Git 与 Docker 构建上下文之外，不把内容或凭证写入日志。

## 修改前基线与静态检查

静态解析工具仅安装到 `/tmp/agentlab-config-validation-036`，固定 PyYAML 6.0.3、dockerfile-parse 2.0.1；没有安装 Node/Pi/CC-Switch 或修改当前运行软件。

实际命令为带 `PYTHONPATH=/tmp/agentlab-config-validation-036` 的内联 `python3 - <<'PY'`：

- 基线：原 Compose 能解析，USER dev、7 个挂载、最终基础镜像 agentlab-base，尚无 Node/Pi/CC-Switch 安装。
- 修改后：YAML 能解析；原 7 个挂载和环境变量保持；新增 2 个绑定数据目录且 `create_host_path: false`。
- Dockerfile 能解析；最终基础镜像与 USER 正确；系统 PATH 优先；所有 RUN 段通过 `bash -n -c` 语法检查。
- 检查 Pi 的兼容性严格参数、CC 安装的失败传播链、数据目录的 Git/Docker 排除项。
- `git diff --check -- .devcontainer/Dockerfile .devcontainer/compose.yaml .gitignore .dockerignore README.md`：通过。

`.dockerignore` 初次写入曾误覆盖已有内容，立即从修改前版本恢复，最终 diff 仅追加新数据目录的排除规则；没有执行构建，原排除规则与数据未受影响。

## 真实数据迁移

元数据核实：`/workspace` 是宿主 9p 挂载，原 `/home/dev/.pi` 未独立挂载，包含 14,951 个普通文件、114,758,089 字节及 8 个符号链接；当前未发现 `/home/dev/.cc-switch`，相关配置目录覆盖变量均未设置。

先用 `git check-ignore` 确认敏感路径被忽略，再准备宿主持久目录，权限实测 0700，符号链接支持正常。源数据内容不输出。

首次逐文件 `shutil.copytree` 在 9p 上耗时偏高，主动中断，原目录未变。随后复用私有暂存目录，以真实 `tar` 流批量复制，保留符号链接，并对源与目标每个普通文件计算 SHA-256、比较目录与符号链接目标。未用“复制命令退出成功”代替内容一致性验证。

### 最终迁移与路径切换结果（接续被中断的会话完成）

- Pi：`tar` 流复制后源/目标逐文件 SHA-256 全部一致（14,951 个普通文件、114,758,089 字节，8 个符号链接目标一致）。22:41 将旧 `/home/dev/.pi` 改名为 `.pi.before-container-state-20261029T224105Z` 保留，建立指向 `/workspace/.container-state/pi` 的符号链接；经旧路径写入的标记出现在持久目录后删除，写入路径切换完成。
- CC-Switch：实施期间 `/home/dev/.cc-switch` 出现旧数据（22:31 起含活跃 SQLite 数据库；另有用户手动安装的 `/home/dev/.local/bin/cc-switch`）。22:41 先做在线一致性备份（`PRAGMA quick_check` ok，非数据库文件校验一致），因备份时检测到写入进程未做实时切换。23:23 确认无写入进程后完成最终迁移：对源库执行 `PRAGMA wal_checkpoint(TRUNCATE)` 稳定，全量复制 9 个文件共 7,130,486 字节，源/目标逐文件 SHA-256 零差异，`cc-switch.db` 与 `session-scan-cache.db` 的 `quick_check` 均 ok；被取代的在线备份移至 `/tmp/node-pi-config-BPvOtk8D/` 留存，旧目录改名为 `.cc-switch.before-container-state-20261029T232333Z` 保留，建立符号链接并验证写穿。源数据内容不输出。

## 质量流程

- comprehensive-testing：核对配置、程序与数据分离、错误传播、兼容性、挂载和迁移边界。此次未写构建验收脚本；用静态解析、真实数据一致性和假数据失败实验提供当前范围的证据，镜像运行用例按用户要求不执行。
- Security Testing：[安全报告](../../security/reports/2026-10-29-container-tools-security.md)，20 项针对性检查通过、0 失败，含两个故意破坏的对照；不把未运行的镜像验收计为通过。
- ce-simplify-code：当前仅小范围配置与必要说明，安装错误传播和数据排除规则保持明确，没有引入通用安装框架或额外脚本，无需精简修改。
- ce-code-review：对固定基线的 5 个文件创建独立临时审查副本，避免带入他人的 035 工作；审查已完成（运行目录 `/tmp/compound-engineering-1000/ce-code-review/20261029-223433-6470ccda`）。1 个 P2：README「启动」的 `up -d --build` 先于持久目录准备，全新克隆按文档顺序会因 `create_host_path: false` 且 `.container-state/` 不入 Git 而启动失败。已修复：`New-Item` 准备命令移入「启动」代码块并置于 `up` 之前，后文改为引用该节不再重复。无既有问题新发现；镜像运行验收按用户缩小范围记录为验证缺口，不计为通过。
- ce-compound：此次安装选择、数据目录和更新原因已由代码注释、README、规格和本日志承载；没有额外符合沉淀条件的已验证结论，不新增重复学习文档。

## 验证范围与限制

未构建镜像、未重建容器，尚未验证新镜像实际安装版本、动态库兼容、容器内 dev 调用和删除容器后的挂载行为。这些是用户明确不要求本次执行的检查，均不计作通过。

项目全量回归（`/workspace`，`env -u TCHAT_SANDBOX cargo check && cargo test --workspace --no-fail-fast`）：`cargo check` 通过；83 个套件 3855 通过、1 失败、6 忽略。唯一失败 `multi_agent::c09_fourth_child_rejected_and_group_assignment_cap` 属工作区 035 多智能体任务的未提交改动（本任务未改 Rust 代码），单跑复现为 FAILED/ok/FAILED 的不稳定用例，按规则单独报告、不由本任务处理；完整输出存 `/tmp/node-pi-config-BPvOtk8D/full-regression.log`。
