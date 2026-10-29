# 031 权限申请 安全攻防测试报告

- 日期：2026-10-29
- 分支：`feat/031-request-permissions`（工作区未提交状态，未做任何修改；仅新增未跟踪的攻击脚手架）
- 被测面：`src/app/src/permissions.rs`、`src/app/src/sandbox.rs`（031 动态入口）、`src/app/src/proc_session.rs`、`src/app/src/turn.rs`、`src/app/src/main.rs` + `tui_host.rs`、`src/app/src/apply_patch.rs`、`src/app/src/view_image.rs`、`src/app/src/mcp/transport.rs`
- 方法：先只读审查代码定位可疑点，再写可运行攻击测试真实 bwrap 实测（攻击者＝模型视角）；攻击脚手架为未跟踪文件 `src/app/tests/security_attack_031.rs`（7 个用例，不入回归集合，建议阅后删除），其余证据复用既有测试并标注用例名。
- 环境约束遵守：全部 `env -u TCHAT_SANDBOX cargo test --locked`；隔离临时目录＋`FAKE-*` 假数据；未读真实 `.env`/凭证/用户配置。

## 结论总览

| 编号 | 风险 | 发现 | 状态 |
|---|---|---|---|
| H1 | 高 | 毒丸快照下 `apply_patch` 静默回退 033 全盘只读策略执行（fail-open） | 已实证复现 |
| L1 | 低 | 已授权祖先覆盖下的永久拒绝路径请求被静默 `granted`（不弹窗、误导性结果；执行层仍遮蔽） | 已实证复现 |
| L2 | 低 | seccomp 清单未拦 `unshare`(2)，沙箱内可成功 `unshare(CLONE_NEWUSER)`（未构成实际逃逸，纵深防御缺口） | 已实证复现 |
| — | — | 其余攻击类别：已实测未发现 | 见下文逐类证据 |

---

## H1（高）：毒丸快照下 apply_patch 静默回退 033 全盘只读策略

**位置**：`src/app/src/turn.rs:400-407`（execute_tool 的 `apply_patch` 分支）

```rust
let snapshot_ref = exec_snapshot.as_ref().and_then(|s| s.valid());
let (r, changes) = if let Some(snap) = snapshot_ref {
    crate::apply_patch::apply_patch_sandboxed(&v, Some(snap))
} else if crate::sandbox::sandbox_requested() {
    crate::apply_patch::apply_patch_sandboxed(&v, None)   // ← 031 毒丸也走这里
} else {
    crate::apply_patch::apply_patch_with_display(&v)
};
```

**问题**：031 模式下每个工具调用开始都会用当前授权构建执行快照；构建失败得到毒丸快照（`poison: Some`）。`exec_command` 对毒丸显式报错、MCP stdio 基线快照构建失败直接拒绝、`view_image` 对 `Some(Err)` 显式报错——唯独 `apply_patch` 把 `valid()` 返回的 `None` 与"未启用 031"混为一谈，落入 `else if sandbox_requested()` 分支，用 **033 固定策略（全盘只读＋工作区/系统临时目录可写）** 继续执行补丁。规格 §1.2-2/§1.6 明确要求"不退回 033 的固定全盘可读策略"且"其他不能表达的策略也不得静默降级"；`PolicySnapshot::valid` 的注释也写明毒丸"调用方走显式失败分支"。

**触发条件**：`build_snapshot` 失败即毒丸，现实可达途径至少有：
- 部署拓扑使自动敏感边界（`config_dir`/`user_dir`/`install_dirs`/`anchor/.env`）成为 base 或已授权条目的**祖先**（如 `TCHAT_USER_DIR`/`HOME` 误配进工作区、安装目录与写面重叠）。`install_from_env` 启动期**不做**基线快照预检，毒丸在每次工具调用时才被发现——此时 exec 全挂而 apply_patch 照常工作，模型会自然转移到补丁面继续以全盘读执行；
- `effective_deny_paths` 按当下存在性过滤：启动后新出现的自动 deny 路径同样会让后续快照中毒；
- 64 条累计授权接近上限时的 argv 容量超限。

**后果**：031 的受限读树失效——模型经 apply_patch 的 `--internal-apply` 子进程可读**全盘任意文件**（补丁上下文读取），并可在工作区/系统临时目录写入；等于绕开"基线＋授权"的整个挂载面。

**复现**（`security_attack_031.rs::attack04_poisoned_snapshot_apply_patch_falls_back_to_033`，真实 bwrap）：

```bash
env -u TCHAT_SANDBOX cargo test --locked --test security_attack_031 attack04 -- --nocapture
```

构造：base write＝`<tmp>/workspace`，`DenyContext.user_dir`＝工作区本身（deny 祖先＝内容路径 ⇒ `build_snapshot` 报"策略矛盾"毒丸）。同一毒丸下：
- `ProcessManager::start_with_policy`（exec_command 同路径）→ `ExecStatus::Error`"生效策略无法编码"，marker 文件未产生（零执行）；
- 按 `turn.rs` 原分支执行 `apply_patch_sandboxed_with_exe(..., None)` → **`ok: true`**，宿主系统临时目录 `attack031-patch-<pid>/probe.txt` 的内容被从 `FAKE-ATTACK-OLD` 成功改为 `FAKE-ATTACK-NEW`——该路径在 031 受限树之外，fail-closed 语义下本应整体拒绝。

**建议修复**：`turn.rs` apply_patch 分支区分"无 031 快照"与"毒丸快照"——`exec_snapshot` 存在但 `valid()` 为 None 时返回显式错误结果（与 `view_image` 的 `Some(Err)` 分支同构），不得落入 033 分支。另建议 `install_from_env` 启动期预检 `baseline_snapshot`＋空授权 `build_snapshot`，让拓扑型毒丸在启动时即明确失败（§1.2-2），而不是拖到工具调用时才暴露并被此分支回退。

---

## L1（低）：已授权祖先覆盖下的永久拒绝路径请求被静默 granted

**位置**：`src/app/src/permissions.rs` 的 `TurnGrants::covers`／`base_covers`（只按路径包含＋访问级别判定，不检查 `deny_paths`）

**问题**：管理员 requestable 含工作区写、模型先获批 `工作区 write`（永久拒绝 `.env` 作为嵌套 carve-out 合法存在），之后再请求 `read 工作区/.env`：覆盖判定命中 ⇒ **不弹确认窗、直接返回 `status="granted"`**。规格 §1.4/§1.5 要求"永久拒绝、管理员上限与现有 OS 权限优先"，对被拒路径的请求按理应整次 error 而非 granted。强制执行层不受影响（快照恒含有效 deny 集，挂载面照旧遮蔽），所以这是**结果误导**而非越权：模型得到"已授权读 .env"的假信号，实际读取仍被拒。

**复现**（`security_attack_031.rs::attack05_grant_limit_atomicity_and_deny_covered_request`）：

```bash
env -u TCHAT_SANDBOX cargo test --locked --test security_attack_031 attack05 -- --nocapture
```

输出：`deny 覆盖路径返回 granted（误导性结果，强制执行层仍安全）`；同用例随后用真实 bwrap 验证 granted 之后 `cat .env` 仍读不到 `FAKE-ATTACK-ENV` 正文。

**建议修复**：`execute_request_permissions` 在覆盖判定（及弹窗）之前检查每个规范化条目是否落在有效 deny 集内，命中即 `error`（与 requestable 超限预检同位置）。

---

## L2（低）：seccomp 清单未拦 unshare(2)，沙箱内可成功创建嵌套用户命名空间

**位置**：`src/app/src/sandbox.rs` `SECCOMP_DENY_EPERM`/`SECCOMP_DENY_ENOSYS`（`clone` 的 namespace 标志掩码只覆盖 clone(2)，未覆盖 unshare(2)=272）

**实测**（`security_attack_031.rs::attack06_seccomp_allows_toolchain_blocks_privileged_syscalls`，沙箱内 gcc 编译 syscall 探针）：

```
unshare_newuser=Success        ← CLONE_NEWUSER 在沙箱内成功
mount_after_newuser=Operation not permitted
unshare_newns_after_newuser=Operation not permitted
keyctl=Operation not permitted
perf_event_open=Operation not permitted
PROBE-EXIT=0
```

**分析**：这与代码注释"嵌套进程不得创建自己的命名空间视图"的意图相悖（clone 掩码拦住的形态经 unshare 可以做到一半）。实际逃逸未成立：新 userns（U2）不拥有外层 mount ns（属 bwrap 的 U1），嵌套后 mount/再次 unshare NEWNS 仍 EPERM；setns/ptrace/bpf 在过滤器内被拦；seccomp 过滤器跨 unshare 持续生效。**当前无可达的提权链**，故评低风险（纵深防御缺口＋与注释意图不一致，未来内核/配置变化下可能变成落点）。

**建议修复**：把 `unshare`(272)（或至少按 flags 掩码拦截）纳入过滤器；顺带评估 `add_key`/`keyctl`/`mknod`/`perf_event_open` 的取舍记录（keyctl/perf_event_open 当前靠内核能力检查挡住，实测 EPERM）。

---

## 已实测未发现的类别（攻击清单逐项证据）

| 类别 | 结论 | 证据 |
|---|---|---|
| 1 请求参数偷塞/伪造：未知字段、`environment_id`/`scope`/`token`/`deny`/`unrestricted`、旧 read/write 数组、null/数组/标量形态、33 条、>64KiB、4097 字节路径/reason、控制字符、各级重复 JSON 键 | 未发现：32 种恶意形态全部整次 `error`、**零确认弹窗**、授权集不变；边界正例（4096 字节 reason、32 条内）正常弹一次窗 | `attack01_forged_arguments_never_prompt_host`；既有 `request_rejects_*`、`request_entries_bounds_1_to_32`、`request_arguments_size_limit`、`request_path_string_validation`（request_permissions.rs，36 用例全过） |
| 2 路径攻击：`..` 穿越、符号链接组件、相似前缀（/a vs /a-sib）、缺失组件、字面 `%XX`/中文不解码、相对路径逃逸、请求非存在父目录 | 未发现：`..` 词法归一后展示实际路径；符号链接组件拒绝；缺失组件拒绝；`%41` 按字面处理（解码形态 `A` 不命中）；中文原样；相对逃逸目标被 requestable 门拦下（error 零弹窗）；相似前缀目录不因共享前缀免确认 | `attack02_path_attacks_parse_and_enforce`（含真实 bwrap 执行面：前缀兄弟目录与未批树 ENOENT、获批面写入正例、`.env` 不可得）；既有 `normalize_*` 用例 |
| 3 越权读取：deny 目录内预存硬链接（P06 变体 A）、deny 文件 nlink>1、同设备 bind 别名、未批树 ENOENT、自动敏感边界正文不可得 | 未发现：deny 树内任一非目录项 nlink>1 ⇒ 启动前拒绝＋工具零执行；运行期建链失败；mountinfo 同设备别名投影（M0 实证，`docs/logs/031-request-permissions/2026-10-28-m0-hardlink-alias.log`）；`.env`/config/user/install 目录正文不可得 | 既有 `deny_dir_internal_hardlink_zero_execution_real`、`runtime_hardlink_to_denied_unreachable`、`restricted_tree_read_write_and_deny_real`、`snapshot_refuses_content_under_auto_deny`（均过）；attack02/03 执行面再证 |
| 3b 获批树内符号链接逃逸（链接指向 / 或未批树） | 未发现：链接在沙箱命名空间内解析——指向 `/` 落到沙箱根（`.env` 仍遮蔽），指向未批树悬空，正文均不可得 | `attack03_symlink_escape_contained_in_sandbox`（真实 bwrap 双执行） |
| 4 边界绕过：伪造用户同意、无宿主/内部记忆轮调用、授予超 requestable、64 条上限、单次全部生效或失败 | 未发现：无 031 状态时伪造调用按未知工具拒绝；超 requestable／网络超限 error 零弹窗；65>64 条整次失败且**零部分提交**（63 条原状），64 条整次生效；已覆盖请求不重问；typed 回执链路见 s140/s141 | `attack05`；既有 `request_permissions_without_state_is_unknown_tool`（turn.rs）、`turn_grants_limit_64_after_dedup`、e2e `s140_request_permissions_cli_grant_and_enforce`、`s141_tui_permission_prompt_allow_and_deny`（均过）。注意 L1：覆盖判定不查 deny 集是本类唯一缺口 |
| 5 生命周期逃逸：旧 shell write_stdin 获新授权、额外权限进程 daemonize/setsid 跨轮存活、毒丸零执行 | 未发现：旧 shell 出生快照冻结读不到后授权文件；`setsid sh -c 'while…'` 守护进程在轮末收尾后**停写**（日志行数冻结）且会话终止；毒丸快照 exec 零执行（但 apply_patch 见 H1） | `attack07_setsid_daemon_reaped_at_turn_end`；既有 `old_shell_does_not_gain_later_permissions`、`extra_permission_processes_reaped_baseline_kept`、`poisoned_snapshot_zero_execution` |
| 6 部署前提：策略在 cwd/临时目录/自身写面、二进制在工作区产物（nlink=1 的 target 二进制同样）、缺 `TCHAT_SANDBOX` | 未发现：上述来源全部拒绝启动；`.env` 无法注入 `TCHAT_PERMISSION_POLICY`/`TCHAT_SANDBOX`（TRUST_CONTROL_VARS 跳过，catalog.rs:236）；真实 CLI 全链路在 nlink=1 副本＋固定安装目录下验收 | 既有 `policy_load_rejects_workspace_and_temp_locations`、`policy_load_rejects_self_write_reference`、`exe_deployment_prerequisite_refuses_workspace_and_temp`、`policy_env_assembly_gates`；e2e s140（真实进程装配成功路径） |
| 7 信息泄露与 seccomp 误伤：结果 JSON 四字段无凭证/令牌；错误消息只含调用方已知的路径；bash/cp/gcc 不被拦、清单内调用被拦 | 未发现：`granted_result`/`denied_result`/`error_result` 恰含 status/permissions/scope/error（attack01 逐条断言四键），无 token；`unshare_newuser` 之外清单全部 EPERM/ENOSYS，工具链（含 gcc 编译）正常；模型 API/MCP HTTP 控制面不经本轮网络开关 | `attack01`、`attack06_seccomp_allows_toolchain_blocks_privileged_syscalls`；既有 `network_switch_real`、`view_image_sandboxed_read_boundary`、`pid_namespace_isolated_real` |

另两条设计内边界如实记录（非漏洞）：mountinfo 别名投影只枚举主命名空间已存在的同设备挂载，deny 检查到挂载之间存在 TOCTOU 替换窗口——均为代码注释与 B1/M0 记录已声明的残余边界（P04"替换窗口需真实竞态证据"仍开放）；P01/P02 的"零模型命令执行"在启动失败路径由 main.rs 启动即 `exit(1)` 保证（代码路径审查，未单独脚本化）。

## 实际运行过的命令

```bash
# 攻击脚手架类型检查与执行（均为未跟踪文件 security_attack_031.rs）
env -u TCHAT_SANDBOX cargo check --locked --test security_attack_031 -p terminal-chat
env -u TCHAT_SANDBOX cargo test --locked --test security_attack_031 -- --test-threads=1 --nocapture
env -u TCHAT_SANDBOX cargo test --locked --test security_attack_031 attack04 -- --nocapture
env -u TCHAT_SANDBOX cargo test --locked --test security_attack_031 attack06_seccomp -- --nocapture

# 既有测试复跑取证
env -u TCHAT_SANDBOX cargo test --locked --test request_permissions --test request_permissions_sandbox
env -u TCHAT_SANDBOX cargo test --locked --test request_permissions
env -u TCHAT_SANDBOX cargo test --locked --test e2e_acceptance s140 -- --nocapture
env -u TCHAT_SANDBOX cargo test --locked --test e2e_acceptance s141 -- --nocapture
```

结果：security_attack_031 7/7 过（其中 attack04/attack05/attack06 是对当前行为缺口的实证记录）；request_permissions 36/36、request_permissions_sandbox 11/11、e2e s140/s141 各 1/1 过。

## 遗留与建议

1. H1 必修：apply_patch 毒丸分支 fail-closed（见上文建议）；修复后 `attack04` 的断言需反转为"毒丸下 apply_patch 显式失败、宿主文件不变"（该用例即可当回归用例）。
2. L1/L2 建议修：覆盖判定前置 deny 检查；seccomp 补 `unshare` 并在注释记录取舍。
3. 攻击脚手架 `src/app/tests/security_attack_031.rs` 为未跟踪文件，不提交；采纳第 1 条修复后可将其改造为回归用例，否则阅后删除。
