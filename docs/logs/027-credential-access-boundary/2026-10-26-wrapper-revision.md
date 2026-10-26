# 027 wrapper 方案旁路核查与 9p 实测（2026-10-26 修订二）

全部只读或临时可逆探针（假内容），真实 `.env` 仅做不输出内容的可读性布尔；未改
sudoers/系统账号/宿主 ACL/容器配置。

## 系统面（实测）

| 项 | 命令（要旨） | 结果 |
|---|---|---|
| wrapper/二进制落点可写性 | `stat /usr/local/bin /opt`＋`test -w` | root:root 0755，dev 不可写 ✓ |
| `sudo -u` 环境行为 | `sudo -n -u nobody env` | env_reset 生效：HOME→目标用户、PATH→secure_path、USER 重置 ✓ |
| Docker 控制权 | `ls /var/run/docker.sock`、`command -v docker` | 无 socket、无 CLI；仅经 sudo（随 sudoers 收紧移除） |
| 其他挂载旁路 | `grep 9p /proc/mounts`＋`stat /vault/.env` | /workspace 与 /vault 同源 D:\（mounts 显示一致）但内容不同：`/vault/.env` ENOENT——.env 无别名路径；`/mnt` 为空（无宿主符号根可达） |
| /workspace 顶层 | `stat /workspace` | **777 root:root**——无 sticky，任何本地 uid 可 unlink/替换他人文件 |
| 真实 .env 现状 | `[ -r /workspace/.env ]` | dev 可读＝true（边界未实施，仅布尔） |

## 9p 权限语义（metadata 挂载，实测强制）

| 探针 | 预期 | 实测 |
|---|---|---|
| chown nobody＋0400 临时文件，dev `cat` | 拒绝 | Permission denied（exit 1）✓ |
| 同文件 `sudo -u nobody cat` | 属主可读 | 成功 ✓ |
| nobody 属主 1777 目录（sticky），dev 删他人文件 | 拒绝 | Operation not permitted（exit 1）✓ |
| dev 属主 0644 文件，`sudo -u nobody` 追加 | 拒绝 | Permission denied（exit 2）✓ |

结论：本 9p 挂载下属主/权限位/sticky 均实际生效（不能只看 chmod 显示值——本处以
行为验证）。⇒ `chown tchat-app 0400 .env` 与 `chmod +t /workspace` 在本环境可用。
首测 sticky 时目录属主为 dev（sticky 允许目录属主删除），复测改 nobody 属主目录后
才成立——记录该测试陷阱。

## 代码面（只读核查，file:line）

- `.env` 来源＝cwd：`src/app/src/main.rs:59-60`（`load_dotenv_from(".env")`，已存在
  环境变量不覆盖）。
- 工具子进程全部继承应用 uid（若应用为 tchat-app 则工具即 tchat-app）：
  - exec_command：`src/app/src/proc_session.rs:1050,1072`（`Command::new(SHELL)`）；
  - MCP stdio 服务器：`src/app/src/mcp/transport.rs:204`；
  - 记忆 git 操作：`src/app/src/memory/workspace.rs:169,394`。
  （初稿曾列入 apply_patch.rs:1426 与 view_image.rs:850——两轴复查证实均在 `#[cfg(test)]`
  测试内，非生产 spawn 点，已纠正。）
  ⇒ 驾驶者让模型执行 `cat /workspace/.env`、或 mcp.json 命令，都能以 tchat-app 读凭据。
- mcp.json 路径＝`user_dir()/mcp.json`：`src/app/src/mcp/manager.rs:495,690`＋
  `src/app/src/mcp/config.rs:103`；user_dir 由 `TCHAT_USER_DIR` 或 HOME 决定
  （`src/app/src/catalog.rs:877-895`）——sudo env_reset 后 HOME=tchat-app，路径转
  管理员控制 ✓；若 env 泄漏 `TCHAT_USER_DIR` 则可注入。
- 模型端点配置＝cwd `config/`（`TCHAT_CONFIG_DIR` 或相对 `config`，
  `src/app/src/catalog.rs:874-878`）；`base_url` 可配置（catalog.rs:94,399）。
  /workspace/config 为 dev 属主 ⇒ **凭据导向面**：dev 改端点可让持真实 Key 的应用把
  Key 发到自选地址。关闭方式＝wrapper 固定 `TCHAT_CONFIG_DIR` 指向 root 属主目录
  （env 变量已受支持，无需产品改动）。
- CLI `-i/--image`（`src/app/src/main.rs:31-62`）＝以运行 uid 读任意文件的入口 ⇒
  sudoers 应用无参数形式（`""`）封掉参数注入。

## 由此修订的方案结论（写入规格附录）

- 原「固定 wrapper 转发参数」不能直接批准：R1 驾驶者经 exec_command/mcp.json 以
  tchat-app 读 .env（代码证）；方案 A 还叠加功能代价——工具以 tchat-app 运行时无法
  改写 dev 属主 0644 文件（写位实测拒绝）。
- 修订：方案 A（纯管理员层，含 sticky＋TCHAT_CONFIG_DIR 固定＋无参数 sudoers）明确
  其威胁模型与功能代价；方案 B（A＋工具子进程降权回 dev）为关闭 R1 的最小产品变更，
  仅描述不实施。详见规格附录修订版。
