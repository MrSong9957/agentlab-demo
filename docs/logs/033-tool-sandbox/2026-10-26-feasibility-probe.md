# 033 沙箱替换可行性探针（2026-10-26）

任务：评估用系统级沙箱替代 027 B 双账号（sudo + 降权工具）方案的可行性。
约束：仅临时目录＋FAKE 假凭据；不安装组件、不改容器/权限；所有探针结束后清理。

## 1. 环境探针（只读）

| 检查 | 命令 | 结果 | 判定 |
|---|---|---|---|
| bubblewrap | `which bwrap firejail` | 均不存在（rc=1） | 不可用（禁安装） |
| 用户命名空间 | `unshare --user --map-root-user --mount sh -c 'echo ok'` | `unshare failed: Operation not permitted` | 不可用 |
| 内核 | `uname -r` | 6.18.33.2-microsoft-standard-WSL2 | Landlock 可用版本 |
| seccomp | `grep Seccomp /proc/self/status` | `Seccomp: 2`（filter 模式） | 追加过滤器非特权允许 |
| 容器能力位 | `grep CapBnd /proc/self/status` | `00000000a80425fb`（无 bit21 CAP_SYS_ADMIN） | mount 类沙箱（bwrap/tmpfs 遮蔽）即使安装也不能工作；setuid bwrap 同样受 bounding set 限制 |
| Landlock ABI | `landlock_create_ruleset(NULL,0,VERSION)` | **ABI 7** | 非特权可调用 ✓ |

## 2. Landlock 行为探针（ctypes 直调 syscall；/workspace 9p 面＋/tmp overlay 对照）

脚本：Python ctypes（`landlock_create_ruleset/add_rule/restrict_self`＝444/445/446，
`PR_SET_NO_NEW_PRIVS`=38；`landlock_path_beneath_attr` 为 4 参 syscall，flags=0）。
探针文件内容全部为 `FAKE-*-TOKEN=…` 假值，目录用后即删。

| 场景 | 授予面 | 实测 | 结论 |
|---|---|---|---|
| 全拒绝基线 | 无 | 9p normal/secret、overlay、/etc/hostname 全 EACCES | Landlock 对 **9p 生效** ✓ |
| 子进程继承＋env | 无（含 EXECUTE 未授） | 子进程 `exec cat` 直接 PermissionError；env 从零构造则哨兵不出现 | 限制随 fork/exec 继承 ✓（包装器模型成立） |
| 目录授予 | 探针目录 READ_FILE\|READ_DIR | 目录内 normal **和同目录 secret 均 READ-OK**；未授予 /etc/hostname EACCES | **规则是白名单并集，无 deny：目录授权连带目录内全部文件** |
| 继承＋正常工作 | /usr/bin、动态库目录 EXECUTE＋探针目录 | `cat` 正常执行（动态库 EXECUTE 授权后）；secret（同目录）可读；env 哨兵未带入 | 执行面需覆盖动态链接器/库——bwrap 全盘 ro-bind 的原因 |
| 写后读（树级无 READ_FILE） | 工作区树 EXECUTE\|READ_DIR\|WRITE_FILE＋子目录 READ_FILE | 树根下 secret EACCES ✓；但 `mkdir` 被拒（缺 MAKE_DIR）——授权面枚举即碎裂 | 不授 READ_FILE 则读不了任何工作区文件 |
| **全功能授予（判定场）** | 工作区 FULL＋执行面 | mkdir/write/cat 正常；**secret.env 读取 rc=0 泄露=True** | **让工具正常干活的最小充分授予面必然连带 .env 可读** |

## 3. 判定

1. **当前环境（不安装、不改容器）无任何现成沙箱可完整实现「工具能干活但不能读 .env」**：
   bwrap 缺位且 bounding set 无 CAP_SYS_ADMIN（安装也不能工作）；Landlock 模型不匹配
   （并集无 deny，判定场泄露=True）；unshare 被禁。
2. **管理员解封后 bwrap 路线可行**（Claude Code sandbox-runtime / codex-linux-sandbox 同型）：
   需要 ①容器允许 userns（`kernel.apparmor_restrict_unprivileged_userns=0` 或 seccomp 放行
   unshare）或 bounding set 加 CAP_SYS_ADMIN＋setuid bwrap；②`apt-get install bubblewrap`。
   均为容器运行参数/系统变更，超出本仓库边界。
3. Landlock 可作**纵深层**（写面/网络面收窄，ABI 4+ 可禁 TCP），不能作 .env 读防线。
4. 附带实证：Landlock 限制下进程连清理自己建的临时目录都被拒（REMOVE 未授）——探针
   残留由限制消失后的后续 shell 删除，过程与结论一致。
