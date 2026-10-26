# 027 方案 B 权限边界补齐与假凭据验证（2026-10-26）

只读代码核查＋假凭据实验；未改产品代码、sudoers、账号、ACL、容器配置；真实 `.env`
未读取正文。复用 2026-10-26-wrapper-revision.md 的 9p/旁路证据，不重复扩大审查。

## 一、模型可影响的执行面与文件面全景（代码证）

| 面 | 代码入口 | 身份（现状＝A） | B 后 | 依据 |
|---|---|---|---|---|
| exec_command | proc_session.rs:1050/1072（`/bin/sh -c`，SHELL＝固定常量非 env；pgroup/setsid） | 应用 uid；**继承完整父环境（无 env_clear）** | dev（经 sudo） | R1 主路径；环境含 .env 载入变量＝现状泄露原语 |
| MCP stdio | transport.rs:204-212（env_clear＋配置 env 显式注入，pgroup(0)） | 应用 uid | dev | 配置/管理员命令面 |
| apply_patch 读+写 | apply_patch.rs:804（读入判型→/proc/self/fd 同 inode 写，**进程内**） | 应用 uid | dev（自再执行 helper） | 模型指定路径读写原语 |
| 记忆 git | workspace.rs:169/394（固定 argv 含 -c 覆盖、env_clear＋固定 PATH） | 应用 uid | **保持 tchat-app** | 依据＝.git/配置/钩子面三层不可达（见 §四 E6；固定 argv 仅辅助，不防已植入钩子）；记忆根在 user_dir（tchat-app 属主），降权反致 .git 属主分裂 |
| 记忆工具读写 | mem_tools.rs:286-295（resolve_scoped_path 限记忆根＋拒 symlink＋拒非常规文件） | 应用（进程内） | 保持 | 根域限制，够不到 .env |
| 图片读取（-i/粘贴/查看图片工具） | view_image.rs:142-143（open_regular_file＋read_bounded）、153（prepare_user_images，main.rs:647/tui_host.rs:283/turn.rs:789/1346 调用） | 应用（进程内） | 保持 | **格式门** view_image.rs:249-250：`image::guess_format` 失败即弃内容、错误仅含路径——.env 文本过不了解码，内容不外带 |
| 上下文注入 | context.rs:94/142（仅 AGENTS.md 链＋全局目录，来源参数化） | 应用（进程内） | 保持 | 文件名固定＋符号链接拒绝（probe_source lstat，见 §四 E5），非任意路径原语 |
| 会话读写 | session.rs:39-41（user_dir/sessions）＋进程内写 | 应用 | 保持 | 私有目录 |
| .env/catalog/user_dir 配置 | main.rs:59-60、catalog.rs:874-895 | 应用 | tchat-app（预期）/wrapper 固定 | 预期行为 |

**主进程内部文件操作核查结论**：存在且全部枚举如上——须降权的只有 apply_patch
（读+写原语）；其余进程内文件面或有格式门（图片）、或限根域（记忆）、或文件名固定
（AGENTS.md 链）、或属私有目录（会话/配置）。全库生产 `File::open/OpenOptions/fs::write`
grep 证据：apply_patch.rs:804、view_image.rs:142、context.rs:94、catalog.rs:950-1126
（配置写入）、session.rs:426-893（会话）、memory/*（根域）——无其他落点。
（apply_patch.rs:1426 与 view_image.rs:850 系 `#[cfg(test)]`，非生产。）

## 二、假凭据验证（本轮实测）

| # | 实验 | 结果 |
|---|---|---|
| E1 | 父进程设假哨兵 env，`sudo -u` 子进程 `env` 查哨兵 | 计数 0＝环境零继承（sudo env_reset；B 下主进程 env 内凭据不进工具）✓ |
| E2 | 父进程持有假凭据文件 fd9，`sudo -u` 子进程 `cat <&9` | Bad file descriptor＝fd 不透传 ✓ |
| E3 | nobody 进程运行中，dev 读 `/proc/<pid>/environ` | Permission denied＝主进程 env 跨 uid 不可读 ✓ |
| E4 | dev 建读追加删自己的开发文件 | 正常 ✓（降权工具保有开发能力） |
| 复用 | 9p 属主读拒/写位拒/sticky | 见 wrapper-revision.md（他人属主 0400 读拒＝假凭据读拒同构）✓ |

**dev 文件能力/假凭据不可读/环境不透传/fd 不透传**四项均实证；「不能重新获得
tchat-app 身份」依赖 sudoers 收紧（NOPASSWD: ALL 现存），列为管理员待验证项——机制
层面 dev 仅经固定 wrapper 可再启应用，且再启实例的工具同为 dev（无凭据读取面）。

## 三、管理员待执行验证项（需真实账号/sudoers，本批不做）

1. `sudo -u tchat-app` 除 `/usr/local/bin/tchat ""` 外全拒（sudoers 收紧后实测）。
2. PTY 模式经 sudo 的组合实测：pre_exec setsid＋TIOCSCTTY 作用于 sudo 进程的语义、
   取消/超时进程组信号对 sudo+命令的覆盖（016 回归集）。
3. MCP 配置 env 经 `/usr/bin/env` 注入的实测（保持 env_clear 语义）。
4. 记忆根属主核对（tchat-app HOME 下）与 wrapper `ulimit -c 0`（防 core 落凭据）。
5. requiretty 未启用复核（本轮 sudo 非 tty 可用已旁证）。

修订后的完整方案（含影响/验收/回滚/未关闭路径）见规格附录「方案 B（修订版）」。

## 四、定向补充验证（上下文读取符号链接＋记忆 Git 钩子，2026-10-26）

### E5：AGENTS.md→假凭据符号链接（双面结果）
- **terminal-chat 读取路径（B 的对象）：已阻断**。代码：context.rs:65-79 `probe_source`
  用 `symlink_metadata`（lstat 不跟随）判 `Symlink` → 跳过并记"符号链接"提示
  （context.rs:234），提示不含正文。实际测试证据：`t04_anomalous_sources_skipped_with_notice_no_abort`
  （context_injection.rs:306-336，断言提示不得含"符号链接目标正文G"）定向运行 1 passed。
  ⇒ 上下文注入维持「保持 tchat-app」不变，不纳入 B 降权范围；安全依据＝文件名固定
  **＋符号链接拒绝**（双层，非仅文件名）。
  - 收口更正（安全攻防 F1）：上述「符号链接拒绝」最初只护**末组件**——目录组件
    （global_dir/项目链目录本身为 symlink）可静态布置注入通道（实证穿透）。已修复：
    context.rs 逐前缀 symlink_metadata 检查＋末组件 O_NOFOLLOW（RED
    `symlinked_directory_component_source_is_rejected`→GREEN，既有 15 项集成回归过）。
- **mock 验收入口（嵌套 claude CLI）端到端：链接被跟随**。临时目录 CLAUDE.md→`@AGENTS.md`、
  AGENTS.md→symlink→假凭据文件（标记 FAKE-CRED-MARKER-7F3A），经 claude_mock_runner.py
  run --cwd --capture：**标记进入模型请求（计数 1）**。属 dev 侧工具链行为，非
  terminal-chat 面：A/B 实施（.env 0400 tchat-app）后该路径读不到真实凭据（dev 无权）；
  使用注意＝--capture 落盘内容可能含被跟随链接的目标正文（文件已 0600，仅本地）。

### E6：记忆 Git 假钩子（nobody 模拟 tchat-app）
- **固定 argv 不防已植入钩子**：nobody 属主临时仓库植入 pre-commit 钩子（仅 touch 标记），
  以与生产相同的调用形态 `env -i PATH=… git -c commit.gpgsign=false -c core.quotePath=false
  commit` 提交 → 钩子执行、标记文件属主＝nobody（＝git 进程 uid；B 下即 tchat-app）。
  ⇒ 「固定 argv」不能单独作为安全依据（证实）。
- **实际防线＝.git/配置/钩子面不可达，三层**：
  1. 模型记忆工具写不进 `.git`：`resolve_scoped_path`（mem_tools.rs:218-258）拒越界
     （../、绝对路径）、拒隐藏组件（`.git` 属之，mem_tools.rs:172-177）、拒符号链接组件、
     拒穿越非常规目录；既有测试 `hidden_and_excluded_component_checks`（mem_tools.rs:495
     断言 `.git` 为隐藏组件）定向运行 1 passed。
  2. dev/其他 uid 写不进：记忆根在 tchat-app HOME（属主隔离）；E6 同场实测 dev 追加
     nobody 属主 `.git/hooks/pre-commit` → Permission denied。
  3. 全局配置注入不可达：run_git `env_clear`（无 HOME→无 ~/.gitconfig；/etc/gitconfig
     属 root）＋固定 argv 的 `-c` 覆盖。
- **可选纵深**：run_git argv 增加 `-c core.hooksPath=/dev/null`（一行产品改动）——即使
  记忆根属主被误配置，钩子也不执行。列为 B 的可选项而非必需（现有三层已闭合）。
