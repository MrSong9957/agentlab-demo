# /vault 仓库 Git token 保存与输出修复（2026-10-05）

## 调查结果

- `/vault` origin URL 早期含明文 token（`ghp_` 串，曾随 `git remote -v` 进入会话输出）；本轮开始时 URL 已是干净形式（无凭据），`.git` 目录内无 `ghp_` 模式残留。
- `credential.helper` 未配置——URL 清理后认证来源缺失，push 会失败。
- `/vault/sources/tools/2026-05-15-token-management.md` 含 3 个 `ghp_` 串（1 个 40 位真实格式 + 2 个短占位），该文件 2026-10-05 更正注已声明旧做法废弃。
- 会话记录文件（`~/.zcode/cli/rollout/model-io-sess_*.jsonl`）含当初配置在 remote URL 的 token——既有存储位置，未删改。

## 修复内容

1. **凭据存储**：安装 GCM v2.6.1（`/usr/local/lib/gcm`，wrapper `/usr/local/bin/git-credential-manager` 补 `GPG_TTY`）与 gnupg 2.4.4、libicu74；`credential.helper=manager`、`credential.credentialStore=gpg`、`credential.interactive=never`、`credential.github.com.provider=generic`。条目落盘 `~/.password-store/MrSong9957.gpg`（GPG 加密，公钥指纹 92ED0473…，明文扫描 0 命中）。
2. **token 迁移**：从既有会话记录经管道提取用户当初配置的串（全程 shell 变量，不进命令参数/新文件），`git credential approve` 写入加密存储；笔记中 40 位串实测被 GitHub 判无效（"Invalid username or token"，已从存储清除）。
3. **URL**：`remote.origin.url` 设为 `https://MrSong9957@github.com/MrSong9957/Obsidian.git`——只含用户名（非敏感），密码由 helper 提供；同时修复 `/vault/.git` 目录属主（root→dev，否则 config.lock 无法创建）。
4. **输出防护**：新增 `/vault/scripts/git-token-check.sh`（remote/config 脱敏显示 + 凭据嵌入形态检测，正常与错误输出均过脱敏管道）；`/vault/AGENTS.md` 与 `sources/security/feedback_git-token-security.md` 更新为实际存储实现与脚本入口。

## 实测验收

- 生效 fetch/push 地址与 insteadOf（0 条）均不含 token（ghp_ 计数 0）✓
- `git push --dry-run` 退出码 0（"Everything up-to-date"，真实认证，无远端改动）✓
- `git credential fill` 取回值与迁移值一致：YES ✓
- 假 token 实测：含假 token 的临时仓库经检查脚本输出，假串 0 泄漏、掩蔽标记 4 处、诊断信息保留；错误输出（不存在路径）同样过管道 ✓
- 残留终检：remote/pushurl/insteadOf 中 ghp_ 为 0；`/vault/.git` 内含 ghp_ 文件 0；`/workspace/.env` 无 GITHUB 项；password-store 明文 0 ✓

## 防护范围与未解决项

- 覆盖入口：`scripts/git-token-check.sh`（新增）、两份文档规则；不声称所有输出都已受保护——git 内部命令裸跑、其他未接入脚本的工具仍需遵守 AGENTS 规则。
- 未解决项（如实列出）：
  1. 笔记文件第 38 行的 40 位 token 已随提交历史推送，用户边界明确不轮换——残留保持原状，轮换后按文档说明即归零。
  2. 会话记录 jsonl 含历史 token 文本（ZCode 内部数据），未删改。
  3. GPG 私钥无口令——本方案的取舍（换取无人值守解密；代价是保护强度等同 0600 私钥文件本身，并非自动化所必需；如需更强保护需引入口令与每会话解锁流程，属用户操作）。
  4. `~/.password-store` 为容器内文件，容器卷丢失需重新迁移 token（来源与步骤已记录于本文档与笔记）。
- 代理：原未配置代理，保持未配置，连接可用（push 实测通过）。

## 收尾轮（同日）：明文清理、权限、脱敏入口实测、持久化核查

### 1. 笔记明文清理

- `sources/tools/2026-05-15-token-management.md`：36 位以上真实格式 `ghp_` 串替换 1 处为明确占位说明（含"原串曾随提交历史推送、按既定边界不轮换"指引）；替换后 36+ 位串 0 残留，短占位示例 5 处按原样保留，教程结构未动。
- 本轮产物明文扫描（位置与计数）：`/workspace/logs/2026-10-05-git-token-remediation.md` 0、`/tmp` 全目录 0、GCM tar 包/probe/测试备份 0。既有会话记录 jsonl 含历史 token 文本——按边界未删改。

### 2. 权限收紧（只动直接相关项）

- `/vault/.git/config`：777 → 644；`/vault/.git/COMMIT_EDITMSG`、`FETCH_HEAD`：777 → 775（去全员写，`.git` 顶层 o+w 残留 0）。
- `~/.password-store`：目录 700、全部文件 600（凭据实际路径 `git/https/github.com/MrSong9957.gpg`）。
- 发现并修复 helper 安装缺陷：`/usr/local/bin/git-credential-manager` 曾是悬空 symlink（symlink 权限恒显示 777、chmod 无效），已改为实体 wrapper 文件 755；`/usr/local/lib/gcm` 内误产生的 wrapper 副本已删除，真二进制为 `gcm-real`。
- `~/.gnupg` 700、`private-keys-v1.d` 700 不变。

### 3. 脱敏入口实测（git-token-check.sh 重写）

- 结构：所有输出（正常与错误）统一经 mask 管道；`set -o pipefail` 传播被封装命令的失败；sed 缺失时抑制全部原文输出（退出码 3）。退出码语义：0=干净且命令成功；1=发现凭据嵌入形态或命令失败；3=脱敏不可用。
- 场景 A（正常+错误输出都含假 token，假 host）：`user:FAKEPASS@` 与 ls-remote 失败信息均被掩蔽，假串原文 0 泄漏，host/路径诊断信息保留，退出码 1 传播。
- 场景 B（/vault 真实）：配置干净、ls-remote 正常显示远端引用、退出码 0。
- 场景 C（损坏 config，被封装命令失败）：错误输出经管道未被吞、退出码 1 传播。
- 场景 D（PATH 无 sed）：提示"输出已抑制"、退出码 3、无原文退回。

### 4. 持久化与无人值守

- `env -i`（清空环境、无 TTY、全新进程）下 `git push --dry-run` 通过——无人值守可用。
- 容器重建保留性核对（对照 compose 卷）：**保留**：/vault（.git 配置、检查脚本、文档）、`/home/dev/.zcode`（会话记录，token 再迁移来源）。**容器层、重建丢失**：GCM 二进制与 wrapper（/usr/local/lib/gcm、/usr/local/bin）、`~/.gnupg`（GPG 私钥）、`~/.password-store`（凭据）、`~/.gitconfig`（helper 配置）。凭据与私钥均未写入镜像或 /vault 仓库（git 跟踪文件扫描 0 命中）。重建后恢复步骤：重装 GCM+gnupg+libicu → 重建 GPG key 并初始化 `~/.password-store/.gpg-id` → 按本文档从会话记录（卷内，持久）重新迁移 token。未重建正在使用的容器，重建流程本身列为待验。

### 5. 终态扫描与边界

- 当前配置与工作区明文残留：`/vault` git 跟踪文件 36+ 位串 0 命中；remote/pushurl/insteadOf 无 token；`.git` 内 0。本次扫描未命中不等于全部历史无残留——token 曾随提交历史推送（已知例外，按既定边界不轮换、不重写历史）。
- `git push --dry-run` 收尾复验：通过。
- 工作区改动区分：本轮修改 AGENTS.md、sources/security/feedback_git-token-security.md、sources/tools/2026-05-15-token-management.md、scripts/git-token-check.sh（新增）；工作区中 MEMORY.md、README.md、sources/_index.md、sources/agent/agent-arch-p1.md 等为仓库既有未提交改动，未混入本轮。
