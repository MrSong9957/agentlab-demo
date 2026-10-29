# 031 M0：核实环境与固定后端——bwrap 原型验证（2026-10-28）

- 性质：只收集证据，不实现功能、不改仓库源码（唯一写入＝本目录 `2026-10-28-m0-*` 证据文件）。
- 方法：以 bwrap 命令行原型模拟 031 动态策略将构造的 argv；全部夹具为 `/tmp/m0-031/fix`
  下假数据；无 cargo 运行；本地回环服务与 GitHub 查证是仅有的两类外部触点。
- 每份 log 含真实命令（set -x 全量追踪）、工作目录、退出码与脱敏原始输出；多次运行
  追加保留（含失败轮与修正轮——它们本身是证据：探针缺陷 vs 后端缺陷的区分记录）。
- 复现：全部探针脚本与夹具生成器在 `/tmp/m0-031/bin/`（ephemeral；log 内 set -x 行即
  完整命令）。

## 环境基线（2026-10-28T13:20Z 实测，[2026-10-28-m0-env.log](2026-10-28-m0-env.log)）

- 内核 `6.18.33.2-microsoft-standard-WSL2`（Ubuntu 24.04.5 容器，host b2f35f825d62）；
  宿主 UID `1000(dev)`；`TCHAT_SANDBOX=1`（compose 注入态在场）。
- bwrap `/usr/bin/bwrap` 0.9.0-1ubuntu0.3，SHA-256
  `e318903862396f96de3df57264e0158682b952fd3fb53ac23d876413e7b30f71`。
- 容器挂载要点：`/`＝overlayfs（containerd 快照），`/workspace` 与 `/vault`＝9p drvfs
  （硬链接跨设备天然不可行，同 fs 内可行——见硬链接节）；`/tmp`＝overlayfs。
- 内核开关：`user.max_user_namespaces=63379`；
  `apparmor_restrict_unprivileged_userns` sysctl 不存在；容器内 `CapEff=0`、
  Docker seccomp 过滤态（`Seccomp: 2`）——033 放行 profile 生效中。
- 真实 bwrap 干跑：033 最小集与产品全参集均 rc=0；沙箱内 `id -u`＝**1000（与主进程
  同 UID，坐实 031 §1.3 的同 UID 前提）**；沙箱内 `NoNewPrivs: 1`（bwrap 自动置位）。

## 十项验证结论

| # | 项 | 结论 | 证据 log |
|---|---|---|---|
| 1 | 环境记录 | **已证实**（见上表） | m0-env.log |
| 2 | 受限读取树 | **已证实**：白名单正例可读；白名单外假敏感文件 ENOENT（不可见）；白名单树只读（EROFS）；沙箱私有 tmpfs 可写 | m0-restricted-read.log |
| 3 | deny 优先 | **已证实**：宽祖先可读 ＋ 窄文件 `Permission denied` ＋ 窄目录 000 不可入；变异对照（去掉 carve-out 挂载）＝两条全部可读 ⇒ 拒绝确由挂载布局产生 | m0-restricted-read.log |
| 4 | 网络开关 | **已证实**：宿主 ns 可达（服务端 ACCEPTED）→ 033 布局（无 `--unshare-net`）可达 → `--unshare-net` 内 `Connection refused`；注意 sysfs 绑自宿主，`/sys/class/net` 显示 eth0 是**视图残留**，连接面已断 | m0-net-ns.log |
| 5 | no_new_privs / seccomp | **已证实**：`--seccomp <fd>` 生效——最小 BPF（含 AUDIT_ARCH_X86_64 检查）禁 rmdir ⇒ 沙箱内 `Operation not permitted`，其余调用正常；独立 C 装载器复证同一 48 字节 ⇒ EPERM | m0-seccomp.log |
| 6 | fresh /proc 与 PID ns | **已证实**：沙箱内 /proc 计数 4（宿主 2073）；pid 1＝载荷；真实宿主 pid 的 `/proc/<pid>` 与 cmdline 均 ABSENT；procfs 为新实例 | m0-proc-pid.log |
| 7 | 同 UID 硬链接/bind 别名 | **部分证实＋一项布局级缺口**（见下节详述） | m0-hardlink-alias.log |
| 8 | 父死亡清理与进程组 | **已证实**：杀 wrapper ⇒ 载荷与 bwrap 消失（先证在场后杀，非空证据）；杀 bwrap ⇒ 载荷消失；PTY smoke：killpg(SIGTERM) ⇒ `BWRAP-EXIT: killed-by-signal 15` ＋ master EOF ＋ 外层无残留 | m0-die-with-parent-pty.log |
| 9 | 上游来源固定 | **已证实**：GitHub API 确认 `d8acdf151577…`（2026-10-23，#56454）与 `d138398cef7d…`（2026-10-21，#55883）均存在；许可证＝仓库根 `LICENSE`（Apache-2.0），`codex-rs/linux-sandbox/` 目录内无独立 LICENSE 文件 | m0-upstream-verify.log |
| 10 | 性能初步样本 | **已出基线（M0 初测，非冻结阈值）**：033 型最小沙箱启动 p95 中位轮 **3.99 ms**；031 受限树型 **3.32 ms**；峰值 RSS 均 **12.8 MiB**（600 原始样本落盘；i5-13500H/WSL2，噪声未控） | m0-perf.log＋m0-perf-samples.txt |

## 第 7 项详述：同 UID 永久边界（031 §1.3 硬性要求）

实测矩阵（假数据，overlayfs 同设备 inode 实证 `dev=86`）：

1. **朴素受限树＋预存硬链接＝泄露**（反向证据）：获批只读树 ro-bind 进沙箱后，
   预存硬链接正文完全可读——路径级挂载布局**无法**中和同 inode 别名。
2. **B1 式 nlink 检查＝fail-closed 证实**：启动前检查 deny 目标 `nlink>1` ⇒
   拒绝该次启动（rc=42，工具未运行 ⇒ 正文不可得）。这是 033 已交付机制的直接推广，
   对 031 需覆盖**全部 deny_paths**且逐次启动复查。
3. **bind 别名＝泄露→闭合**：同一目录挂两个路径、carve-out 只挂一个 ⇒ 别名路径全文
   可读；**对每个 bind 实例重复 carve-out** ⇒ 两条路径均不可得、其余数据正常。
4. **运行期新建链接＝不可达**：沙箱内 `ln <已遮蔽目标> <新链接>` 失败（bind 到
   /dev/null 的 deny 路径解析到别的设备）；跨文件系统硬链接 EXDEV（overlay↔9p 实测）。

**布局级缺口（如实声明，需 M2 裁决）**：bwrap 的路径挂载不能「按 inode 拒绝」——
预存硬链接的防御只有两条路：a) nlink fail-closed（拒绝启动，非遮蔽）；b) 白名单内容
快照复制（破坏 inode 语义，代价大、有 TOCTOU）。且 nlink 检查只覆盖「检查时已存在」
的链接：检查→挂载窗口与运行期并发变化仍是边界（033 B1 已声明同款边界）。**按规格
§1.3 的口径，这不是遮蔽搪塞——是「检测即拒绝」路线的实证**；但「内容经任何别名都
不可得」在本后端**不可证明**，M2 必须把该边界写进启用条件与用户告知。

## 探针自身缺陷记录（区分后端缺陷与测试缺陷）

- `--as-pid-1` 必须配 `--unshare-pid`（首版布局缺命名空间组被 bwrap 拒绝——正确行为）。
- seccomp：`seccomp_data.nr` 在**偏移 0**（偏移 4 是 `arch`）；过滤器须以 `RET` 收尾且
  bwrap 按 8 字节整除校验；fd 传递用持久 `exec 5<` 成功，command-scoped 重定向上
  出现过一次 EBADF（Bash CLOEXEC 语义竞态）——M2 在 Rust 侧应显式 dup2 传 fd。
- `exec -a` 是 bash 特性，dash 内不存在 ⇒ 首版 die-with-parent/PTY smoke 载荷从未
  启动（空证据），修正后重跑全部通过。
- 占位符未替换导致过一次假 HOST-PROC-VISIBLE（`-d /proc/` 恒真）——修正为真实 pid
  烘焙后证伪。

## M2 实施最需注意的技术点

1. **seccomp fd 的传递管道**：bwrap 自身多次 re-exec，fd 必须活到最终 stage；bash
   语义不可依赖，Rust 侧用 pre_exec 显式继承，且过滤器文件生命周期要与 spawn 同步。
   过滤器要带 AUDIT_ARCH_X86_64 门（跨架构 nr 空间不同，上游用 seccompiler 解决）。
2. **受限读取树的运行时目录完备性**：tmpfs 空根 + usrmerge 符号链接
   （`--symlink usr/bin /bin` 等三条）是 shell 能跑的最小集；033 的「全盘 ro-bind」
   不再兜底，缺一个运行时组件＝工具全部失败——需要一层可配置的固定运行时白名单。
3. **硬链接边界的启用条件**：nlink fail-closed 对 031 的全部 deny_paths 生效＋
   carve-out 对每个 bind 实例重复；「不可证明的别名面」必须作为 031 启用告知与
   P06 验收的显式边界写进契约，不能写成已消除。

## 附：证据文件清单

| 文件 | 内容 |
|---|---|
| 2026-10-28-m0-env.log | 环境/内核/bwrap/UID/挂载/干跑 |
| 2026-10-28-m0-restricted-read.log | 受限树＋deny 优先＋变异对照（含首版缺命名空间的失败轮） |
| 2026-10-28-m0-net-ns.log | 网络：宿主→033 现状→unshare-net |
| 2026-10-28-m0-seccomp.log | seccomp：错误偏移轮→EBADF 诊断→修正轮→C 复证 |
| 2026-10-28-m0-proc-pid.log | fresh /proc/PID ns（含占位符缺陷轮与修正轮） |
| 2026-10-28-m0-hardlink-alias.log | 硬链接/别名/H4 运行期链接/跨设备 |
| 2026-10-28-m0-die-with-parent-pty.log | 父死亡两路径＋PTY killpg smoke（含 exec -a 空证据轮） |
| 2026-10-28-m0-upstream-verify.log | GitHub 两提交＋许可证 |
| 2026-10-28-m0-perf.log / -samples.txt | 性能摘要＋600 原始样本 |
