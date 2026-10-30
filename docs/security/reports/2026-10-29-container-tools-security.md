# 容器工具配置安全测试报告

- 日期：2026-10-29。
- 结论：本次作用域未发现有证据支持的新增安全漏洞。针对性检查 **20 通过、0 失败**，其中包含静态检查、隔离假数据执行和两个失败对照。该结论不代表镜像已构建或运行验收通过。
- 约束：用户明确本轮只准备 Docker 配置与数据持久化，不构建镜像、不重建容器；环境也没有 Docker CLI/daemon。

## 范围与基线

仅审查下列配置相对 `/tmp/node-pi-config-BPvOtk8D/before/` 的变更，并检查必要的既有配置是否保留：

- [Dockerfile](../../../.devcontainer/Dockerfile)
- [compose.yaml](../../../.devcontainer/compose.yaml)
- [.gitignore](../../../.gitignore)
- [.dockerignore](../../../.dockerignore)
- [README.md](../../../README.md)

依据：[036 规格](../../specs/036-node-pi-container.md)、[037 规格](../../specs/037-cc-switch-container.md)、[审查规则](../../../rules/REVIEW.md)、[安全测试要求](../../../.agents/agents/Security-Testing-Gate.md)。未审查 Rust 源码及其他并行修改；没有读取真实 `.container-state` 内容、认证信息、用户会话，也没有操作真实 Agent 配置路径。真实数据迁移与内容一致性由实施日志另行证明，本报告不将其记为已测。

## 已核实问题

无。没有需要列出风险等级或修复复现的已确认新增漏洞。

## 检查与实测证据

| 检查目标 | 操作或输入 | 结果与限制 |
|---|---|---|
| 非 root 与系统路径 | 解析 Dockerfile 最终 `USER`、Compose `user` 与 `PATH` | 均声明 `dev`，PATH 包含 `/usr/local/bin`；没有把持久数据目录加入 PATH。仅静态成立，未执行镜像内命令。 |
| 固定安装来源 | 解析 `FROM`、`COPY`、安装命令 | Node 来源 `node:24-bookworm-slim`；最终基础镜像保持原值；COPY 仅来自 Node 阶段。Pi 是规格指定的 `@earendil-works/pi-coding-agent@latest`；CC 是固定的官方 GitHub Release HTTPS 地址。没有下载或执行上游内容。 |
| npm 安装约束 | 检查 `--prefix /usr/local --ignore-scripts --engine-strict`；将 `npm` 与 `ln` 替换为临时桩，令 install 返回 17 | 原 shell 命令返回 **17**；后续 cache-clean 未执行。参数静态检查通过；没有真实安装包或制造系统链接。 |
| CC 下载失败传播 | 在原 `RUN` shell 链中仅把 `/tmp/cc-switch-install.sh` 改为独立临时文件；用假 curl 写入假脚本后返回 22 | 返回 **22**，调用记录只有 `curl`，假安装器没有执行；即使存在下载残留也没有继续。 |
| CC 安装失败传播 | 假 curl 成功，假安装器返回 1 | 返回 **1**，调用记录 `curl → bash`，没有执行末尾 `rm`；失败没有被后续成功掩盖。 |
| CC 正常流程及安装参数 | 假 curl、假安装器均成功；假 bash 校验安装环境变量 | 返回 **0**，调用记录 `curl → bash → rm`；`CC_SWITCH_INSTALL_DIR=/usr/local/bin`、`CC_SWITCH_FORCE=1` 均传入。全程仅执行假脚本。 |
| 失败断言有效性 | 临时将 CC 链首个 `&&` 改为 `;`，仍令假 curl 返回 22 | 破坏版本返回 **0** 且执行假安装器，违背同一失败检查断言；验证检查能够识别此回归。正式文件未修改。 |
| 凭证进入 Git | 独立临时 Git 仓库复制本次 `.gitignore`，写入 5 种假 auth/session/db/env 路径，通过真实 `git check-ignore --no-index` 检查 | **5/5 忽略**，覆盖含空格和隐藏子目录的路径；只读写假数据。删除临时副本中的新规则后，假 auth 路径变为未忽略，退出 **1**，对照有效。 |
| 凭证进入构建上下文 | 静态检查 `.dockerignore` | `.container-state/` 与 `**/.env` 均存在，且无否定规则；Dockerfile 没有从宿主项目复制文件。没有运行 Docker 上下文打包，不能作为实际传输验证。 |
| 挂载与旧配置 | YAML 解析后与固定基线逐项比较 | Pi/CC 使用预定宿主目录，均 `create_host_path: false`；所有已有服务设置、原挂载和命名卷声明保持。缺失宿主目录的真实 Compose 失败行为未执行。 |
| 已有数据与目录权限 | 检查镜像目录初始化 `install -d -m 0700 -o dev -g dev`；在临时假数据目录实际运行同类 `install -d -m 0700` | 假配置内容保留、目录模式为 0700。实际 bind mount 会覆盖镜像目录；宿主 NTFS ACL、用户映射与实际可写性均未测。 |

Git 使用的假路径为 `.container-state/pi/agent/auth.json`、`.container-state/pi/a space/session.jsonl`、`.container-state/pi/.nested/config.json`、`.container-state/cc-switch/cc-switch.db`、`.env`；内容均为合成标记，不包含真实密钥。

执行命令与原始结果暂存位置：

```bash
# 工作目录：/workspace
PYTHONPATH=/tmp/agentlab-config-validation-036 python3 /tmp/container-tools-security-Q7T1Ur/probe.py
# TOTAL 20 PASS, 0 FAIL；进程退出 0
```

脚本与 `results.json` 均只保留在 `/tmp/container-tools-security-Q7T1Ur/`，未将临时扫描结果或原始日志写入报告目录。PyYAML 与 dockerfile_parse 仅复用已有临时工具，没有安装软件。

## 八类攻击项的适用性

本次变更提供镜像安装和持久化配置，没有新增后端 HTTP 接口、用户/订单模型、管理员角色或前端校验。因此数据越权、功能越权、token 认证、提交字段提权、SQL/命令/XSS 注入、绕过前端限制及接口高并发防刷均不适用于这次配置变更，未执行这些接口攻击，不能据此判断现有 Rust 程序的安全性。敏感信息泄露仅针对新持久目录的 Git/构建上下文排除及禁止项目数据 COPY 做了以上限定检查。

## 未执行与结论边界

以下 **8 组运行验收未执行**，均不得计入通过：

1. 真实镜像构建、上游安装脚本和 npm 下载执行结果。
2. 镜像内 Node/npm/Pi/CC-Switch 的版本、帮助与依赖兼容性。
3. `dev` 在交互和非登录 shell 中实际调用系统程序。
4. Docker 实际构建上下文打包与凭证排除。
5. 宿主路径缺失时 Compose 是否拒绝启动。
6. 真实挂载权限、NTFS ACL 与 dev 首次写入。
7. 容器停启、删除后重建的数据与数据库保留。
8. 旧 Pi 数据迁移的完整性与现存数据路径切换。

完整 Rust 回归不在本子任务作用域，未由本报告执行或作出通过结论。没有对真实服务发网络攻击、做并发压测或读取真实凭证。未发现配置层安全问题，不消除上述运行时验证缺口。

## 测试快照

测试结束时 5 个文件内容与测试开始时一致：

| 文件 | SHA-256 |
|---|---|
| `.devcontainer/Dockerfile` | `4a4b216c7d9fae90e94a845bc02da349e525183ed35f0576647d7cbf5d921143` |
| `.devcontainer/compose.yaml` | `d5229bea5fbb175c18e16c4cf3634b609275c224c948d814ad11373aaf158605` |
| `.gitignore` | `997cd19c7a56fbef1cee4a483bfbf07fa349faf34045e2b0caa08344cc69fe2c` |
| `.dockerignore` | `c044aa0e028e51743caf8a29b5f052dee52e144729c86451a683adabc08594be` |
| `README.md` | `a525d6fd0ddf00c24535d5babcc3aa5510d564582dd1127eb37570cf7a807dea` |
