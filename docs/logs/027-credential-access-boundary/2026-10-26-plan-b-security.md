# 027 方案 B 产品实现——安全攻防测试报告（2026-10-26）

- 测试角色：Security Testing Gate
- 被测：分支 `fix/025-security-hardening`，`git diff 62543a6` 下隔离模式（`TCHAT_ISOLATED_TOOLS`）生产改动（proc_session / mcp transport / apply_patch / catalog / context / main / turn / memory workspace）＋新增测试。
- 代码快照说明：测试期间主会话在 01:31–01:33 并发修改了 apply_patch.rs / proc_session.rs / catalog.rs / turn.rs；本报告全部二进制实测基于 01:39:23 构建的 `target/debug/terminal-chat`（晚于最后源码改动 01:33:57，快照一致）。
- 硬性约束遵守：未读取 `.env` 正文、未用真实凭据、未调用真实模型服务、未改真实 sudoers/账号/产品代码/测试；实验全部在 `/tmp`（假内容假凭据），sudo 仅 `-u nobody/dev`；实验区 `/tmp/sec027` 已删除。工具版本：git 2.43.0、procps-ng kill 4.0.4、GNU env 9.4、/bin/sh=dash。

## 一、攻防检查总表

| # | 检查项 | 攻击构造 | 结果 | 证据（请求→返回） | 结论 | 建议动作 |
|---|---|---|---|---|---|---|
| S1/S2 | env 值注入面（transport.rs 隔离分支 `format!("{k}={v}")`） | 值含 `=`（`A=x=y`）、换行、空串、空格、横杠开头、嵌套 `A=B=C=D`、shell 元字符 `'; rm…; $(id)` | 全部按字面单值语义；无解析歧义、无命令执行 | `/usr/bin/env A=x=y sh -c…`→`[x=y]`；换行值 od 逐字节复核；Canary 未触发 | 通过 | 无 |
| S3 | 隔离链路身份/环境 | 父进程带哨兵 env `TCHAT_SENTINEL_SECRET`＋完整产品 argv `sudo -n -u nobody -- env TERM=… /bin/sh -c…` 与 MCP 双 env 形态 | 身份=nobody(65534)、HOME 重置 /nonexistent、哨兵零继承；显式 env 值含 `=`/换行完整保留 | `id -un`=nobody；`SENTINEL=[ABSENT]` | 通过（与实现记录 E1 一致） | 无 |
| S4 | 跨 uid 补杀 kill_group_cross_uid | ①dev 直接 killpg 打 nobody 组；②产品形态 `sudo -n -u nobody /bin/kill -9 -- -PGID` 与 `-2` SIGINT 形态；③不带 `--` 对照；④正数 PID 对照（以 /proc cmdline＋非 Z 状态判活，规避 WSL2 init 不收割僵尸的判定陷阱） | ①EPERM 复现（前提成立）；②SIGKILL/SIGINT 均击杀成功 rc=0；③**不带 `--` 时 procps kill 静默无操作（rc=0、无 stderr、目标存活）**；④对照有效 | `kill: (-459085) - Operation not permitted`；产品形态后目标 DEAD；nosep 后 ALIVE | 通过（产品带了 `--`，正确且必要）；形态正确性对 procps 版本敏感 | 部署验收按实现记录复核跨 uid 组杀；锁定 procps 行为用例 |
| S5 | 记忆 git hooksPath（B⑤） | ①植入可执行 pre-commit（touch 标记＋exit 1）；②`-c core.hooksPath=/dev/null` 提交真实暂存变更；③`GIT_CONFIG_COUNT` env 注入恶意 hooksPath 与 `-c` 争优；④`commit -a` 变体；⑤`stat /dev/null/pre-commit` | 提交成功且钩子未执行；env 注入在无 `-c` 时确实生效（有效性对照），有 `-c` 时被压制；stat=ENOTDIR | S5c-redo：rc=0、1 file changed、EVIL-HOOK-RAN 不存在；S5d-redo：无 `-c` 时钩子触发 | 通过（git 2.43.0） | 极老 git(<2.9) 无 hooksPath 会忽略未知 `-c` 键→钩子照跑；部署锁定 git 版本 |
| S6 | sudo 失败面泄漏（apply_patch_isolated_with_exe stderr tail 200） | ①unknown user；②目标不可执行二进制（chmod 000）；③子进程内部失败（超限/非 UTF-8）；补丁内埋 `TOPSECRET-PATCH-MARKER-9Q` 后 grep 输出 | 所有失败路径 stderr 均无补丁内容（grep 0 命中）；tail(200) 只携带 sudo/internal-apply 通用错误 | `sudo: unknown user nosuchuser9x8`；`sudo: …/noexec: command not found` | 通过 | 无 |
| S7 | `--internal-apply` 本地滥用 | ①dev 直跑＋有效 Add 补丁；②相对路径补丁；③解析错误回显检查；④1MiB+1 超限 stdin；⑤非 UTF-8 | ①任意本地调用者可无鉴权使用，以**自身身份**应用（JSON 第三字段 uid=1000 直证），相对路径按 cwd 解析；③解析错误经 stdout JSON 回显补丁行（模型自身补丁，无跨边界泄漏）；④流式拒绝实测 38ms；⑤拒绝 rc=2 | `[{"ok":true,…},…,1000]`；`internal-apply：补丁超过 1048576 字节上限` | 定性：**无提权**（等价"以本人身份写文件"，dev 本可直跑应用/写盘），不构成新本地面；但入口无任何"须经父进程 sudo 调起"守卫，绕过 REPL 审计/授权入口 | 文档明示该入口语义；可选加守卫（如要求 TCHAT_ISOLATED_TOOLS 已设） |
| S6a+ | 真实隔离链路端到端 | 手工驱动 `sudo -n -u nobody -- terminal-chat --internal-apply`，补丁写 dev 目录 vs 777 目录 | dev 目录：`Permission denied`（隔离生效）；777 目录：写入成功、文件属主 nobody、uid=65534 直证 | JSON `…,"error":"写入新增文件失败…os error 13"…,65534]` / 成功 JSON＋`stat -c %U`=nobody | 通过 | 无 |
| S8 | TOCTOU 修复残余面（目录组件符号链接） | ①末组件符号链接（应拒）；②global_dir 本身为符号链接；③多组件中间符号链接（`m/link/sub/AGENTS.md`）。以零依赖 standalone 精确复刻 context.rs 真实读取路径（`symlink_metadata` 判型＋`O_NOFOLLOW` 打开），代码事实经通读 context.rs 双确认（probe 对 `global_dir.join("AGENTS.md")` 做 lstat，O_NOFOLLOW 只拒末组件） | ①拒绝（probe=Symlink＋open ELOOP 双层，产品 race 测试 1 passed）；②③**穿透成功：is_regular=true、正文被读取**（DIRSYMLINK-/MIDSYMLINK-MARKER） | standalone 输出 `probe is_regular=true / read: Ok("DIRSYMLINK-MARKER")`；`cargo test --lib race_…` passed | **残余面成立**：config_dir 默认＝cwd 相对 `config`，恶意仓库可投放 `config→/anywhere` 符号链接，主进程（应用 uid）读取任意 `<路径>/AGENTS.md` 正文进模型上下文——隔离模式下绕过"工具降权读不到 dev 文件"的同仓边界（受限：文件名固定 AGENTS.md、1MiB 预算）；普通模式下无增益 | 中优先：对目录链逐组件 O_NOFOLLOW（openat 逐级）或至少如实收窄边界文档＋部署约束项目来源 |
| S-extra | `.env` 是否可篡改隔离开关 | 项目 `.env` 写 `TCHAT_ISOLATED_TOOLS=nobody`，真实二进制启动（stdin EOF，无模型调用） | **生效**：输出「工具隔离模式：exec/MCP/apply_patch 以 nobody 身份执行」；无 .env 对照零输出 | envproj-out.log 第 6 行 | 低风险：`TCHAT_ISOLATED_TOOLS` 不在 TRUST_CONTROL_VARS（catalog.rs:236-246），违背代码注释"由管理员 wrapper 固定导出"的信任模型；只能收紧不能放宽（.env 不覆盖已设值），无提权 | 加入 TRUST_CONTROL_VARS |
| — | 恶意隔离用户值 | `TCHAT_ISOLATED_TOOLS`＝`-x`/`dev root`/`x=y`（数组直传给 sudo `-u`） | sudo `unknown user` 干净拒绝，无参数注入 | `sudo: unknown user -x` | 通过（仅致工具报错的 DoS，且改动者本就控制该面） | 无 |
| — | 普通模式零影响 | 代码比对：build_tool_command/transport spawn/turn 的 None 分支与改前逐行等价；kill_group_cross_uid 由 `isolated_tool_user()` 门控 | 普通模式不产生额外进程/argv | diff 结构＋门控阅读 | 通过 | 无 |

## 二、既有测试定向复跑（验证实现记录 GREEN 声明）

| 测试 | 结果 |
|---|---|
| `--lib commit_does_not_execute_planted_hooks` | 1 passed |
| `--lib race_symlink_swap_between_probe_and_open_is_not_read` | 1 passed |
| `--test apply_patch_isolated`（3 用例） | **并行 2 passed/1 FAILED（3/3 次复现）；单跑 passed**——`isolated_apply_update_and_delete` 在 139 行 Update 断言失败：同进程并行测试无锁操纵 `TCHAT_ISOLATED_TOOLS`（set_var/remove_var 竞态，测试 2 的 remove_var 落在测试 3 的 set 与 Update 之间→"隔离模式未启用"） |
| `--test exec_command isolated_mode`（2 用例） | 身份/env 用例 passed；`isolated_mode_pty_timeout_cancel_semantics_preserved` 并行同跑 1 次失败、单跑 3/3 通过——同族 env 竞态/并行时序（exec_command.rs:676-684/716-722 无锁） |
| `--test mcp_stdio a027_isolated` | 1 passed |

## 三、发现清单（按严重度）

| 编号 | 严重度 | 发现 |
|---|---|---|
| F1 | 中 | 目录组件符号链接残余面（S8）：`config_dir`/项目链目录为符号链接时 AGENTS.md 正文以应用 uid 被读入模型上下文；隔离模式下构成绕过工具降权边界的上下文注入/外传通道（恶意仓库投放 `config→symlink`）。边界文档 E5 的「符号链接拒绝」只对末组件成立，表述过宽 |
| F2 | 低 | 项目 `.env` 可启用隔离模式（TCHAT_ISOLATED_TOOLS 不在 TRUST_CONTROL_VARS，端到端实测生效）；无提权、无法关闭管理员设置，但违背"管理员 wrapper 固定导出"声明，建议加入信任根名单 |
| F3 | 低 | 隔离相关测试并行竞态：apply_patch_isolated 全套必败（单跑全过）、exec_command PTY 隔离用例偶败——测试无锁操纵进程级 env，违反"测试彼此独立"，会造成 CI 假红/掩盖回归（SEAM_LOCK 先例可套用） |
| F4 | 信息 | `--internal-apply` 对任意本地调用者无鉴权可用（以自身身份应用补丁，无提权）；1MiB 流式拒绝（38ms）与非 UTF-8 拒绝实测有效；建议文档明示入口语义 |
| F5 | 信息（正面） | sudo 失败面无补丁内容泄漏；stderr tail(200) 仅携带通用错误 |
| F6 | 通过 | env 注入面全部攻击构造未突破（值含 `=`/换行/元字符按字面单值；哨兵零继承；身份直证 nobody） |
| F7 | 通过（附部署注意） | 跨 uid 补杀有效；**不带 `--` 的形态是静默无操作**（rc=0 无 stderr），现实现正确但对 procps 版本敏感；pgid 复用误杀窗口毫秒级且需同 uid 新组，定性接受；dash 内建不支持负 pgid，`/bin/kill` 绝对路径为必要设计 |
| F8 | 通过 | hooksPath=/dev/null 对 git 2.43.0 完全有效（真实变更提交成功、植入钩子被抑制、env 注入被 `-c` 压制） |
| F9 | 通过 | TOCTOU 修复本体（末组件＋probe 双层）有效且产品级 race 测试通过 |
| F10 | 低 | 文档一致性：①code 注释与 implementation.md T5 称"本机 coreutils env 不接受 `--`"——实测 `env -- K=V cmd` 可用（GNU 9.4），真实失败形态是 `env K=V -- cmd`（rc=127），属归因错误（所选无 `--` 设计碰巧更保守，无害）；②「定向回归全部绿」与本快照矛盾（apply_patch_isolated 并行红，且第三个用例为文档落笔后新增）；③E5 表述过宽（同 F1）；④历史接缝 TCHAT_INTERNAL_APPLY_MARK 已从现行代码移除（改进，文档仅历史记录，无需改） |

## 四、总结论

**有条件通过。**

隔离模式三条主链路（exec/MCP 经 sudo+env 降权、apply_patch 经 sudo+--internal-apply、kill_group_cross_uid 跨 uid 补杀）与记忆 git hooksPath 防线，在本环境全部攻击构造下未被突破；身份直证（uid=65534）、环境零继承、1MiB 流式限量、sudo 失败面零泄漏均实测成立。

收口前置条件（建议）：F1 需修复（目录链逐组件 O_NOFOLLOW）或在文档与部署层面如实收窄「上下文加载可经目录符号链接以应用 uid 读取任意 AGENTS.md」的边界并接受该残余面；F2/F3 为低成本修复（TRUST_CONTROL_VARS 补一个键；测试加锁或改子进程注入 env）；F10 文档两处表述修正。以上完成后可达「通过」。
