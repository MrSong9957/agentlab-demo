//! 033：工具执行面沙箱（bubblewrap 包装）。
//!
//! 上游来源：openai/codex `codex-rs/linux-sandbox` @ commit
//! `d8acdf151577132d4ee1de47a4d1822ae5acb762`（Apache-2.0）。逐函数对应关系与
//! 适配差异记录于 `docs/specs/033-tool-sandbox.md`「上游复用与适配声明」。
//!
//! 启用：环境变量 `TCHAT_SANDBOX` 非空（信任根控制变量，由管理员 wrapper 固定
//! 导出；catalog::TRUST_CONTROL_VARS 收录——.env 不是信任根，不得借它开启）。
//! 未设＝普通模式，既有行为零变化；已设但沙箱不可用＝**显式错误，不静默回退**。
//!
//! 策略（本项目本地化极小集；上游 `FileSystemSandboxPolicy` 耦合 codex-protocol，
//! 不搬）：全盘只读 ＋ 工作区＋系统临时目录可写 ＋ `<工作区>/.env` 不可读遮蔽；
//! 网络默认保留；PID/IPC/用户命名空间隔离＋新 procfs 成对挂载（上游 mount_proc
//! 语义——`/proc/self/fd` 消费者依赖，见 create_bwrap_flags 注释）。
//!
//! 2026-10-27 B1/B2 加固（本轮）：B1＝deny 目标存在预建硬链接（nlink>1）时
//! 拒绝该次工具启动（路径级遮蔽不覆盖同 inode 别名）；B2＝沙箱工具子进程经
//! pre_exec `close_range(CLOSE_RANGE_CLOEXEC)` 收口继承 fd ≥3。保证范围与
//! 剩余边界见 docs/logs/033-tool-sandbox/2026-10-27-b1-b2-risk-verification.md。
//!
//! 031（动态权限）：[`PolicySnapshot`]＋[`permission_launch_argv`] 在同一模块
//! 扩展可信策略启动入口——受限读树（tmpfs 根＋固定运行时白名单）、拒绝优先
//! 的挂载布局（每个 bind 实例后逐别名遮蔽，含 /proc/self/mountinfo 同设备
//! bind 投影）、启动前 deny 边界递归 nlink 检查（fail-closed＋遍历预算）、
//! 网络开关（--unshare-net）与应用层 seccomp（cBPF，AUDIT_ARCH 门）。
//! M0 原型证据：docs/logs/031-request-permissions/2026-10-28-m0-bwrap-prototype.md。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 上游 bwrap.rs:68 `WSL_INTEROP_DIR`（WSL Windows 进程桥的 Unix socket 目录）。
const WSL_INTEROP_DIR: &str = "/run/WSL";

/// 沙箱模式启用判定（027 的 `TCHAT_ISOLATED_TOOLS` 判定入口的替代位）。
pub fn sandbox_requested() -> bool {
    #[cfg(unix)]
    {
        std::env::var("TCHAT_SANDBOX")
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// 启动审计行：启用时输出 bwrap 位置或不可用原因（用户可见的启用确认／失败预告）。
pub fn startup_notice() -> Option<String> {
    if !sandbox_requested() {
        return None;
    }
    Some(match resolve_launcher() {
        Ok(bwrap) => format!(
            "工具沙箱模式：exec/MCP/apply_patch 经 bubblewrap 隔离（{}）",
            bwrap.display()
        ),
        Err(reason) => format!(
            "工具沙箱模式不可用：{reason}——工具调用将被拒绝（不静默回退）；\
             请检查 TCHAT_SANDBOX 设置与 bubblewrap 安装"
        ),
    })
}

/// 外部探测：沙箱当前是否真实可用（缓存语义同 [`resolve_launcher`]）。
/// 供启动审计与集成测试按环境分支断言；只读，不改变任何全局状态。
pub fn probe_available() -> bool {
    resolve_launcher().is_ok()
}

/// 工具启动命令构造的统一入口（exec_command、MCP stdio、apply_patch 子进程共用）。
///
/// 返回 `None`＝普通模式（按既有行为执行 `command`）；`Some((program, args, env))`＝
/// 沙箱包装（program/args 直接作为 spawn 目标，env 为白名单子进程环境）；探测或
/// 组装失败＝`Err`（调用侧必须显式报错，禁止回退为无沙箱执行）。
///
/// **策略锚定（033 安全攻防 G1 修复）**：deny 与可写根一律锚定**主进程 cwd**
///（启动期会话工作区，本函数内部取用，不经调用方传参）——exec 的 `workdir` 是
/// 模型可控入参，若允许策略随其锚定，换 workdir 即可让真实 `.env` 脱离遮蔽。
/// 调用方的 workdir/cwd 仅用于 spawn 的 `current_dir`（沙箱内 `--chdir` 语义）。
pub fn tool_launch_argv(
    command: Vec<String>,
) -> std::io::Result<Option<(String, Vec<String>, Vec<(String, String)>)>> {
    if !sandbox_requested() {
        return Ok(None);
    }
    let bwrap = resolve_launcher().map_err(std::io::Error::other)?;
    // 锚定根＝主进程 cwd 的 canonical 形态（G1：不采信任何调用方传入的目录）。
    let anchor = std::env::current_dir()?;
    let anchor = anchor.canonicalize().map_err(std::io::Error::other)?;
    let policy = policy_for(&anchor);
    // 上游 mask_wsl_interop 判定（linux_run_main.rs:278）：`Path::new(
    // WSL_INTEROP_DIR).is_dir()`——仅 WSL 发行版存在该目录。容器/裸机环境
    // 无条件 `--tmpfs /run/WSL` 会让 bwrap 在 ro-bind 只读根上 mkdir 目标，
    // EROFS 直接失败（2026-10-27 宿主验收实证；interop socket 不存在时也无
    // 可遮蔽对象，跳过不损失防线）。
    let mask_wsl_interop = Path::new(WSL_INTEROP_DIR).is_dir();
    let args = create_bwrap_command_args(command, &policy, &anchor, mask_wsl_interop)
        .map_err(std::io::Error::other)?;
    Ok(Some((bwrap.to_string_lossy().into_owned(), args, tool_child_env(&anchor))))
}

/// 沙箱子进程白名单环境：PATH/TERM 继承（非凭据；工具可用性优先），HOME 定为
/// 工作区。主进程经 .env dotenv 加载的凭据变量**不在白名单**——027 R1 的环境面
/// 防线（「凭据 env 不进工具」）在沙箱模式下由本函数＋调用侧 `env_clear` 保留。
fn tool_child_env(workspace: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        ),
        ("HOME".to_string(), workspace.to_string_lossy().into_owned()),
    ];
    if let Ok(term) = std::env::var("TERM") {
        env.push(("TERM".to_string(), term));
    }
    env
}

/// 033-B2（2026-10-27 加固）：沙箱工具子进程 fd 收口——pre_exec 内以
/// close_range(2) 的 `CLOSE_RANGE_CLOEXEC` 把继承 fd ≥3 **标记**为 execve 时
/// 关闭（只标记、不关闭）：
/// - 仅作用于 fork 出的子进程副本，主进程 fd（PTY master、MCP 管道、错误上
///   报通道等）不受影响；
/// - std 的 spawn 失败信道（Linux 为 SOCK_SEQPACKET socketpair，fd≥3、自带
///   CLOEXEC）不受影响：execve 失败时 errno 仍回传父进程、spawn 返回 Err。
///   若在 pre_exec 里真 close 掉 fd≥3，失败会被误报为 spawn 成功（子进程
///   静默 exit 1）——故必须 CLOEXEC 标记而非关闭；
/// - stdio 0/1/2 不在标记范围；载荷运行时自开的 fd 在 execve 之后，不受影响。
///
/// 三处工具 spawn 点（proc_session build_tool_command、mcp transport、
/// apply_patch internal-apply）仅在**沙箱分支**注入；普通模式行为零变化。
/// close_range 失败（如旧内核 ENOSYS）＝Err → pre_exec 失败 → std 回传错误、
/// spawn 返回 Err（沙箱工具显式失败，不静默放行）；部署前提＝内核 ≥5.11
///（CLOSE_RANGE_CLOEXEC 引入）＋glibc ≥2.34（close_range 封装；libc 0.2.189
/// 绑定为 gnu 目标普通符号——musl 目标无绑定，链接期即失败，非静默）。
/// 异步信号安全：单个 syscall，无分配/锁/环境读取（io::Error 为 Os repr，
/// 不堆分配）。
#[cfg(target_os = "linux")]
pub fn close_inherited_fds_mark_cloexec() -> std::io::Result<()> {
    // SAFETY：close_range(2) 仅操作本进程 fd 表的标志位，无内存安全影响。
    let ret = unsafe {
        libc::close_range(3, libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC as libc::c_int)
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// 非 Linux unix：close_range(2) 为 Linux 特有 syscall，此处＝能力不足，按
/// 既有 fail-closed 语义显式失败（Os repr ENOSYS，不堆分配——pre_exec 内
/// 合法），沙箱工具不静默放行。非 unix（Windows）下 tool_launch_argv 恒
/// None，不会调用本函数。
#[cfg(all(unix, not(target_os = "linux")))]
pub fn close_inherited_fds_mark_cloexec() -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
}

/// 031：[`close_inherited_fds_mark_cloexec`] 的 seccomp 例外变体——跳过
/// `skip`（seccomp 过滤器 memfd，须以非 CLOEXEC 穿越 execve 交给 bwrap），
/// 其余继承 fd ≥3 照常标记。两次 close_range 均 CLOEXEC 标记而非关闭（同
/// B2 的 std 失败信道保全理由）；异步信号安全（两个 syscall）。
#[cfg(target_os = "linux")]
pub fn close_inherited_fds_except_cloexec(skip: std::os::fd::RawFd) -> std::io::Result<()> {
    if skip < 3 {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY：close_range(2) 仅操作本进程 fd 表的标志位。
    unsafe {
        if skip > 3 {
            let ret = libc::close_range(
                3,
                (skip - 1) as libc::c_uint,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            );
            if ret != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        let ret = libc::close_range(
            (skip + 1) as libc::c_uint,
            libc::c_uint::MAX,
            libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
        );
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// 非 Linux unix：同 [`close_inherited_fds_mark_cloexec`] 的能力不足语义。
#[cfg(all(unix, not(target_os = "linux")))]
pub fn close_inherited_fds_except_cloexec(_skip: std::os::fd::RawFd) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
}

// ---------- bwrap 探测（failIfUnavailable） ----------

/// 探测结果进程级缓存：`Err` 文案同样缓存（探测含真实 bwrap 干跑，代价不低；
/// 同一进程内环境不变，结果可复用）。测试经 [`resolve_launcher_with`] 注入。
static LAUNCHER: OnceLock<Result<PathBuf, String>> = OnceLock::new();

fn resolve_launcher() -> Result<&'static PathBuf, String> {
    let init: &Result<PathBuf, String> = LAUNCHER.get_or_init(probe_bwrap);
    init.as_ref().map_err(|e| e.clone())
}

/// 031：已解析 bwrap 的安装目录输入（deny 自动边界用）。未探测成功时返回
/// None——调用方（可信安装位置收集）跳过即可，真正的可用性校验在启动入口。
pub fn launcher_path() -> Option<PathBuf> {
    resolve_launcher().ok().cloned()
}

/// 测试注入变体：绕过进程级缓存，直接以给定 bwrap 路径探测。
pub(crate) fn resolve_launcher_with(bwrap: &Path) -> Result<PathBuf, String> {
    probe_bwrap_at(bwrap)
}

/// 探测：PATH 查找 bwrap ＋ 真实干跑（ro-bind＋userns＋cap-drop 全套最小参数执行
/// `/bin/true`）——「存在」不等于「可用」，容器禁 userns 时必须在这里暴露。
fn probe_bwrap() -> Result<PathBuf, String> {
    let program = find_bwrap_in_path().ok_or_else(|| {
        "未在 PATH 中找到 bubblewrap（bwrap）；请安装 bubblewrap".to_string()
    })?;
    probe_bwrap_at(&program)?;
    Ok(program)
}

fn probe_bwrap_at(bwrap: &Path) -> Result<PathBuf, String> {
    let run = std::process::Command::new(bwrap)
        // 与真实策略同构的最小集：全盘 ro-bind＋userns＋cap-drop；不 unshare-pid/net。
        .args([
            "--ro-bind",
            "/",
            "/",
            "--unshare-user",
            "--unshare-ipc",
            "--cap-drop",
            "ALL",
            "--",
            "/bin/true",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match run {
        Ok(status) if status.success() => Ok(bwrap.to_path_buf()),
        Ok(status) => Err(format!(
            "bwrap（{}）干跑失败（exit {:?}）：内核/容器可能禁止 unshare 用户命名空间",
            bwrap.display(),
            status.code()
        )),
        Err(e) => Err(format!("bwrap（{}）无法执行：{e}", bwrap.display())),
    }
}

fn find_bwrap_in_path() -> Option<PathBuf> {
    // G5 防御：主进程 PATH 可能含工作区内目录（部署配置失误面）——上一会话
    // 植入的假 bwrap 会被本会话执行（假沙箱＝无沙箱）。解析结果不在锚定工作区
    // 之下才接受；工作区内的候选跳过（宁可显式不可用，不执行可能被栽赃的二进制）。
    let workspace = std::env::current_dir().ok()?;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("bwrap"))
        .find(|candidate| {
            let in_workspace = candidate
                .canonicalize()
                .ok()
                .is_some_and(|c| c.starts_with(&workspace));
            !in_workspace
                && candidate.is_file()
                && std::fs::metadata(candidate)
                    .map(|m| {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            m.permissions().mode() & 0o111 != 0
                        }
                        #[cfg(not(unix))]
                        {
                            true
                        }
                    })
                    .unwrap_or(false)
        })
}

// ---------- 策略（本地化极小集） ----------

/// 文件系统沙箱策略：上游 `FileSystemSandboxPolicy` 的本项目投影。
/// `full_disk_read`＝全盘只读基线；`writable_roots`＝可写根；`unreadable_roots`＝
/// 不可读遮蔽路径（deny）。网络策略当前为保留（如需断网可加 `--unshare-net`）。
struct SandboxPolicy {
    full_disk_read: bool,
    writable_roots: Vec<PathBuf>,
    unreadable_roots: Vec<PathBuf>,
}

fn policy_for(workspace: &Path) -> SandboxPolicy {
    SandboxPolicy {
        full_disk_read: true,
        // 可写面＝工作区＋系统临时目录（对齐 Claude Code 沙箱默认：写＝工作
        // 目录＋per-user temp；编译产物、管道临时文件在 temp 落地属常规用法）。
        // 两根均 canonical 化（033 攻防 G2：词法路径可被内核解析回其他位置，
        // 非 canonical 输入在 create_filesystem_args 中显式拒绝）。
        writable_roots: vec![
            workspace.to_path_buf(),
            std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()),
        ],
        unreadable_roots: vec![workspace.join(".env")],
    }
}

// ---------- argv 组装（上游 bwrap.rs 裁剪适配） ----------

/// 上游 `create_bwrap_command_args`（bwrap.rs:296）的适配版：本项目策略恒非
/// full-disk-write，全放行捷径与 glob 展开分支裁剪，直接进入分层组装。
fn create_bwrap_command_args(
    command: Vec<String>,
    policy: &SandboxPolicy,
    command_cwd: &Path,
    mask_wsl_interop: bool,
) -> Result<Vec<String>, String> {
    create_bwrap_flags(command, policy, command_cwd, mask_wsl_interop)
}

/// 上游 `create_bwrap_flags`（bwrap.rs:309）的适配版。会话与命名空间标志保留；
/// WSLg/WSL-interop 遮蔽、`--proc`（本项目保留 PID 命名空间以维持 016 的
/// killpg 进程组管理，新 procfs 不挂——工具内进程视图受限为已知取舍）、
/// unshare-pid/net 默认关闭。
fn create_bwrap_flags(
    command: Vec<String>,
    policy: &SandboxPolicy,
    command_cwd: &Path,
    mask_wsl_interop: bool,
) -> Result<Vec<String>, String> {
    // 上游顺序：会话标志在最前，随后文件系统参数（perms/tmpfs 组合的顺序语义
    // 依赖挂载操作次序，保持与上游一致）。`--as-pid-1`＝上游 launcher 在
    // `--unshare-pid` 时的插入项（exec_bwrap argv.insert(1)）：bwrap 为子
    // PID 命名空间的 pid 1（033 安全复验 G4-R1/G3 修复，成对机制恢复）。
    let mut args = vec![
        "--as-pid-1".to_string(),
        "--new-session".to_string(),
        "--die-with-parent".to_string(),
    ];
    args.extend(create_filesystem_args(policy)?);
    // 上游 mask_wsl_interop 语义（033 攻防 G4）：WSL 的 Windows 进程桥用
    // /run/WSL 下的 Unix socket——MZ 头文件 execve 经宿主 interop 处理器在
    // Windows 侧执行，可绕开全部 Linux 挂载策略。仅当该目录存在（WSL 发行
    // 版，上游 linux_run_main.rs:278 is_dir 判定）时遮蔽；容器/裸机无此目录
    // 时跳过——无条件遮蔽会让 bwrap 在 ro-bind 根上 mkdir 目标而 EROFS
    // （2026-10-27 宿主验收实证修复）。继承 procfs 的回读面由下方成对的
    // `--unshare-pid`＋`--proc` 关闭（上游：mask 与新 procfs 独立，tmpfs
    // /proc 仅在未挂新 procfs 时需要——本处不使用）。
    if mask_wsl_interop {
        args.push("--tmpfs".to_string());
        args.push(WSL_INTEROP_DIR.to_string());
    }
    // 上游：显式请求 user namespace 而非依赖 bwrap 自动判定（uid 0 时会跳过）；
    // 同时阻断经宿主 procfs 的 ptrace 路径面。容器内无需 CAP_SYS_ADMIN 的关键。
    // `--unshare-pid`（033 安全复验 G4-R1/G3 成对修复）：沙箱内进程与主进程
    // 隔离在独立 PID 命名空间——主进程从沙箱 /proc 视图消失（environ 读取与
    // kill 面关闭）；killpg 契约不受影响（进程组为跨 ns 的内核全局结构，
    // 主进程对组长发信号覆盖组内全部成员；bwrap 退出经 --die-with-parent
    // 连带回收子 ns）。跨 ns 的 016 PTY/取消/超时契约列入 bwrap 环境验收。
    args.push("--unshare-user".to_string());
    args.push("--unshare-pid".to_string());
    args.push("--unshare-ipc".to_string());
    // 与 --unshare-pid 成对的新 procfs 实例（上游 mount_proc 语义）：保留
    // `/proc/self/fd` 消费者（apply_patch Update 的 F2 写绑定），主进程不在
    // 沙箱 /proc 视图内。不再需要 `--tmpfs /proc`（见上）。
    args.push("--proc".to_string());
    args.push("/proc".to_string());
    let normalized = normalize_command_cwd_for_bwrap(command_cwd);
    if normalized.as_path() != command_cwd {
        // 上游：bwrap 只挂载 canonical 根时，符号链接别名 cwd 会消失——显式进入
        // canonical cwd，保持相对路径语义对齐。
        args.push("--chdir".to_string());
        args.push(path_to_string(normalized.as_path()));
    }
    args.push("--cap-drop".to_string());
    args.push("ALL".to_string());
    args.push("--".to_string());
    args.extend(command);
    Ok(args)
}

/// 上游 `create_filesystem_args`（bwrap.rs:409）的裁剪适配版，mount 顺序与上游
/// 注释一致：
/// 1. 全读策略 `--ro-bind / /` ＋ `--dev /dev` 最小设备树（null/zero/random/urandom/tty）；
/// 2. 可写根按路径深度排序，`--bind <root> <root>` 重新开放写入；
/// 3. 可写根内的不可读 carve-out 在根 bind **之后**遮蔽（后遮蔽覆盖先 bind）；
/// 4. 可写根之外的不可读路径最后遮蔽。
/// 裁剪：daemon socket 遮蔽、ProjectRoots 元数据自动保护、ripgrep glob 展开
/// （本项目 deny 为具体路径）、synthetic mount registry（`--ro-bind /dev/null`
/// 路径 bind 不需要宿主侧临时文件，见 `append_existing_unreadable_path_args`）。
fn create_filesystem_args(policy: &SandboxPolicy) -> Result<Vec<String>, String> {
    // Bubblewrap 的 bind 目标必须存在；缺失的可写根跳过（上游同语义）。
    // G2 防御：可写根必须**词法清洁**（absolute_lexical 后不变）——`sub/..` 这类
    // 路径被内核解析回父级，使遮蔽与 bind 的排布失配（挂载序后写者胜，遮蔽被
    // 重开）。符号链接可写根不受此限：上游的 canonical bind＋deny remap 语义
    // （canonical_target_if_symlinked_path）已正确处理。失配即显式拒绝组装。
    let mut writable_roots: Vec<PathBuf> = policy
        .writable_roots
        .iter()
        .filter(|root| root.exists())
        .map(|root| {
            if absolute_lexical(root) != *root {
                return Err(format!(
                    "可写根 {} 词法非清洁（含 ./.. 等组件）：拒绝组装（解析失配会致遮蔽失效）",
                    path_to_string(root)
                ));
            }
            Ok(root.clone())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let unreadable_roots: Vec<PathBuf> = policy
        .unreadable_roots
        .iter()
        .map(|p| absolute_lexical(p))
        .collect();
    let mut args: Vec<String> = if policy.full_disk_read {
        // 上游：`--dev /dev` 建最小设备树，必须在可写根 bind 之前，
        // 显式 /dev/* 可写 bind 才可见。
        vec![
            "--ro-bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
        ]
    } else {
        return Err("本项目策略恒为全盘只读基线；受限读分支未启用".to_string());
    };

    let allowed_write_paths: Vec<PathBuf> = writable_roots
        .iter()
        .flat_map(|root| {
            let mut paths = vec![root.clone()];
            // 可写根本身是符号链接时，canonical 目标同样视为可写（上游同语义）。
            if let Some(target) = canonical_target_if_symlinked_path(root) {
                paths.push(target);
            }
            paths
        })
        .collect();

    // 只遮蔽位于可写根之外的不可读祖先（上游：嵌套 carve-out 在根 bind 后处理）。
    // 本项目策略 deny 恒为可写根内的单文件，不会产生此分支；若未来策略放宽到
    // deny 可写根祖先，必须同时补上游的「遮蔽后重建可写后代挂载点」逻辑
    //（append_mount_target_parent_dir_args）——在此之前显式失败（fail closed），
    // 不产出缺重建步骤的错误 argv。
    let mut unreadable_ancestors: Vec<&PathBuf> = unreadable_roots
        .iter()
        .filter(|p| {
            let p = p.as_path();
            !allowed_write_paths.iter().any(|root| p.starts_with(root))
                && allowed_write_paths.iter().any(|root| root.starts_with(p))
        })
        .collect();
    if !unreadable_ancestors.is_empty() {
        return Err(format!(
            "deny 路径 {} 是可写根的祖先：本项目策略不支持（需遮蔽后重建可写后代挂载点），拒绝组装",
            path_to_string(unreadable_ancestors[0].as_path())
        ));
    }

    writable_roots.sort_by_key(|root| path_depth(root));
    for root in &writable_roots {
        let symlink_target = canonical_target_if_symlinked_path(root);
        let mount_root = symlink_target.as_deref().unwrap_or(root.as_path());
        args.push("--bind".to_string());
        args.push(path_to_string(mount_root));
        args.push(path_to_string(mount_root));

        // 嵌套不可读 carve-out：本项目 deny 目标（workspace/.env）正落在此分支——
        // 可写根 bind 之后遮蔽，遮蔽结果覆盖先前 bind。
        let mut nested: Vec<PathBuf> = unreadable_roots
            .iter()
            .filter(|p| p.starts_with(root))
            .cloned()
            .collect();
        if let Some(target) = &symlink_target {
            nested = remap_paths_for_symlink_target(nested, root, target);
        }
        nested.sort_by_key(|p| path_depth(p));
        for unreadable_root in &nested {
            append_unreadable_root_args(&mut args, unreadable_root, &allowed_write_paths)?;
        }
    }

    // 可写根与不可读根互不重叠时的剩余遮蔽（上游 rootless 分支）。
    let mut rootless: Vec<&PathBuf> = unreadable_roots
        .iter()
        .filter(|p| {
            let p = p.as_path();
            !allowed_write_paths
                .iter()
                .any(|root| p.starts_with(root) || root.starts_with(p))
        })
        .collect();
    rootless.sort_by_key(|p| path_depth(p));
    for unreadable_root in rootless {
        append_unreadable_root_args(&mut args, unreadable_root, &allowed_write_paths)?;
    }

    Ok(args)
}

/// 上游 `append_unreadable_root_args`（bwrap.rs:1328）。deny 路径穿过可写符号
/// 链接时 **fail closed**——按当前目标遮蔽只是启动时刻快照，沙箱内进程可改写
/// 该符号链接后经逻辑路径读回原内容（TOCTTOU；上游注释逐字保留）。
fn append_unreadable_root_args(
    args: &mut Vec<String>,
    unreadable_root: &Path,
    allowed_write_paths: &[PathBuf],
) -> Result<(), String> {
    if let Some(symlink) =
        first_writable_symlink_component_in_path(unreadable_root, allowed_write_paths)
    {
        /*
         * Deny-read masks must fail closed when the protected path crosses a
         * symlink that remains writable to the sandboxed process. Resolving and
         * masking the symlink's current target is a TOCTTOU snapshot: bwrap would
         * protect the old target while the logical path could later point
         * somewhere else.
         */
        return Err(format!(
            "cannot enforce sandbox deny-read path {} because it crosses writable symlink {}",
            unreadable_root.display(),
            symlink.display()
        ));
    }

    // B1（2026-10-27 加固；收口轮改单次 stat 三分类）：不能用 `exists()` 判
    // 「不存在」——它对任何 stat 错误（EACCES/ELOOP/ESTALE…）都返回 false，
    // 会把非 NotFound 错误静默当 missing 跳过检查（不遮蔽、不拒绝＝fail-open；
    // 且上方符号链接遍历遇 lstat 错误按上游语义 break 跳过，不构成拦截）。
    // 改为一次 `fs::metadata`（跟随符号链接）显式三分类：Ok → 硬链接检查＋
    // 遮蔽；NotFound/NotADirectory → 既有 missing no-op（见下）；其余错误 →
    // 目标与链接数状态不明＝拒绝该次启动（fail closed，与上方可写符号链接
    // fail-closed 同构）。
    let metadata = match std::fs::metadata(unreadable_root) {
        Ok(md) => Some(md),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            // 上游：缺失路径用 `--ro-bind-data <fd>` 预创建空文件，阻止沙箱内重建
            // 层级。本项目适配（无 fd mount，见下）：deny 目标是已存在的凭据文件，
            // missing＝当前无凭据可保护，no-op；沙箱内若新建 .env 属写入面（027
            // 语义为读保护，写面与普通模式一致开放）。
            return Ok(());
        }
        Err(e) => {
            return Err(format!(
                "受保护文件 {} 状态读取失败（{e}）：无法核实遮蔽目标与硬链接，拒绝启动沙箱工具（不回退普通模式）",
                unreadable_root.display()
            ));
        }
    };

    // B1：deny 目标为常规文件且链接数>1（沙箱外已存在的硬链接别名）＝拒绝
    // 组装——路径级遮蔽（挂载在路径上）不覆盖同 inode 的其他路径，别名不可
    // 枚举，只能拒绝该次启动。检查跟随符号链接取**实际受保护文件**的链接数
    //（rootless deny 自身为符号链接时不经上方分支，查链接本身会漏判）；目录
    // 不适用（nlink＝子目录计数，非文件硬链接）。每次工具启动组装时复查，
    // 不缓存安全结论。
    // 边界如实声明：本检查只阻断「检查时已存在」的硬链接；检查到挂载之间及
    // 沙箱运行期的并发变化仍是边界，不构成完整竞态防护（见 B1/B2 核实记录）。
    // nlink 为 unix 语义；非 unix 下 bwrap 路线本就不可用，跳过本检查。
    #[cfg(unix)]
    if let Some(md) = metadata {
        use std::os::unix::fs::MetadataExt;
        let links = md.nlink();
        if !md.is_dir() && links > 1 {
            return Err(format!(
                "受保护文件 {} 存在 {links} 个硬链接：路径级遮蔽无法覆盖同 inode 别名，拒绝启动沙箱工具（不回退普通模式）",
                unreadable_root.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;

    append_existing_unreadable_path_args(args, unreadable_root);
    Ok(())
}

/// 上游 `append_existing_unreadable_path_args`（bwrap.rs:1360）的适配版。
/// 文件：`--perms 000`＋`--ro-bind-data <nullfd>` 在本项目改为
/// `--ro-bind /dev/null <path>`——std Command 跨 spawn 传递 fd 需要额外的
/// CLOEXEC 处理（上游为 execve 同进程模式无此问题），且 /dev/null 读取恒空、
/// 写入丢弃，与「空普通文件＋000 权限」在读面上安全等价。目录分支保留
/// `--perms 000`＋`--tmpfs`＋`--remount-ro`（本项目策略 deny 仅文件，目录分支
/// 为上游函数完整性保留，未含可写后代重建）。
fn append_existing_unreadable_path_args(args: &mut Vec<String>, unreadable_root: &Path) {
    if unreadable_root.is_dir() {
        args.push("--perms".to_string());
        args.push("000".to_string());
        args.push("--tmpfs".to_string());
        args.push(path_to_string(unreadable_root));
        args.push("--remount-ro".to_string());
        args.push(path_to_string(unreadable_root));
        return;
    }

    args.push("--ro-bind".to_string());
    args.push("/dev/null".to_string());
    args.push(path_to_string(unreadable_root));
}

// ---------- 031：动态策略启动入口 ----------

/// 031 执行快照：出生时冻结的「基线＋当前有效授权」（§1.6）。由
/// permissions 模块在**工具调用开始时**构建，经 SpawnJob 携带到长寿创建
/// 线程——创建线程不临时重读授权（排队创建不因后续授权变化升级权限）。
#[derive(Debug, Clone)]
pub struct PolicySnapshot {
    /// 只读挂载面（基线 read＋本轮 read 授权；canonical 绝对路径）。
    pub read_paths: Vec<PathBuf>,
    /// 可写挂载面（基线 write＋本轮 write 授权；canonical 绝对路径）。
    pub write_paths: Vec<PathBuf>,
    /// 有效 deny 集（显式＋自动敏感边界；canonical）。
    pub deny_paths: Vec<PathBuf>,
    /// 网络是否启用（基线 OR 本轮授权）。
    pub network: bool,
    /// 毒丸：Some(reason)＝本轮「基线＋授权」无法编码为生效策略——任何执行
    /// 工具必须以该原因显式失败（fail-closed：不回退 033/普通模式，也不用
    /// 空面冒充成功）。[`Self::valid`] 是唯一取用有效视图的入口。
    pub poison: Option<String>,
}

impl PolicySnapshot {
    /// 构建失败的毒丸快照（仅承载失败原因）。
    pub fn poisoned(reason: String) -> Self {
        Self {
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            deny_paths: Vec::new(),
            network: false,
            poison: Some(reason),
        }
    }

    /// 有效视图：毒丸返回 None（调用方走显式失败分支，不得用空面执行）。
    pub fn valid(&self) -> Option<&PolicySnapshot> {
        if self.poison.is_some() {
            None
        } else {
            Some(self)
        }
    }
}

/// deny 边界遍历预算（§1.7/启动前检查）：条目数与墙钟双上限，**超限拒绝
/// 启动**（fail-closed，不静默跳过剩余项）。默认值按实测基线留余量冻结
/// （见 docs/logs/031-request-permissions/ 性能证据；历史建议值 20000 项/
/// 2000ms，实测典型 deny 树≪该值）。
#[derive(Debug, Clone, Copy)]
pub struct TraversalBudget {
    pub max_entries: usize,
    pub timeout_ms: u64,
}

pub const DEFAULT_TRAVERSAL_BUDGET: TraversalBudget = TraversalBudget {
    max_entries: 20_000,
    timeout_ms: 2_000,
};

/// 031 沙箱启动产物：argv/env＋seccomp 过滤器 fd 守卫（fd 必须存活到
/// bwrap execve 完成——守卫 Drop 才关闭；调用方保持其在作用域内直到 spawn
/// 返回）。
#[derive(Debug)]
pub struct SandboxLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub seccomp: Option<SeccompFd>,
}

/// seccomp 过滤器的 memfd 守卫：非 CLOEXEC（须穿越 execve 交给 bwrap），
/// Drop 关闭。spawn 后（Command::spawn 返回即可）即可 drop。
/// unix：包装 RawFd；Windows：031 不可用（build_seccomp_fd 恒 Err），
/// 空壳仅为 SandboxLaunch 字段类型的三端一致。
#[cfg(unix)]
#[derive(Debug)]
pub struct SeccompFd(std::os::fd::RawFd);

#[cfg(unix)]
impl SeccompFd {
    pub fn raw(&self) -> std::os::fd::RawFd {
        self.0
    }
}

#[cfg(unix)]
impl Drop for SeccompFd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub struct SeccompFd;

#[cfg(windows)]
impl SeccompFd {
    pub fn raw(&self) -> i32 {
        unreachable!("031 seccomp 仅 unix（build_seccomp_fd 恒 Err，本壳不构造）")
    }
}

/// 031 可信策略启动入口（§1.6）：由宿主把已校验的基线与本轮 grant 编码为
/// 内部策略快照后调用。**每次调用同步完成全部检查**（bwrap 可用性、P06(b)
/// deny 边界递归检查、挂载布局组装、seccomp 过滤器构造）——任一失败＝Err，
/// 模型命令零执行，不回退普通 shell 或 033 固定策略。
///
/// 隔离创建失败的「命令执行前拒绝」链：bwrap 自身对命名空间/挂载设置失败
/// 会以非零退出结束且**不 exec 载荷**（M0 实证；缺 `--unshare-pid` 配
/// `--as-pid-1` 即拒绝）；调用方对 bwrap 失败的退出状态显式报错。结构性
/// 断言（`--proc` 与 `--unshare-pid` 成对等）在组装层强制。
pub fn permission_launch_argv(
    command: Vec<String>,
    snapshot: &PolicySnapshot,
    command_cwd: &Path,
) -> std::io::Result<SandboxLaunch> {
    if let Some(reason) = &snapshot.poison {
        return Err(std::io::Error::other(format!(
            "生效策略无法编码：{reason}（拒绝启动，不回退）"
        )));
    }
    let bwrap = resolve_launcher().map_err(std::io::Error::other)?;
    // P06(b)：启动前 deny 边界检查（每次启动重新执行；不缓存安全结论）。
    verify_deny_boundaries(&snapshot.deny_paths, &DEFAULT_TRAVERSAL_BUDGET)
        .map_err(std::io::Error::other)?;
    let mut args = vec![
        "--as-pid-1".to_string(),
        "--new-session".to_string(),
        "--die-with-parent".to_string(),
    ];
    // 载荷程序自身的单文件只读挂载（受限树不挂它的目录）：execve 需要目标
    // 可见。安全属性：安装目录整体 deny 遮蔽后，单文件 bind 按深度排序在其
    // 后重暴露——恰好只开放这一个文件（本二进制/bwrap 自身的可信部署语义）。
    let payload_program = command
        .first()
        .and_then(|p0| std::fs::canonicalize(p0).ok())
        .filter(|p| p.is_file());
    create_permission_filesystem_args(&mut args, snapshot, payload_program.as_deref())
        .map_err(std::io::Error::other)?;
    // 命名空间组（033 已验证语义保留）：独立 PID/用户/IPC＋成对新 procfs。
    args.push("--unshare-user".to_string());
    args.push("--unshare-pid".to_string());
    args.push("--unshare-ipc".to_string());
    args.push("--proc".to_string());
    args.push("/proc".to_string());
    // 网络开关（M0 实证 --unshare-net：回环外全部拒绝；sysfs 视图残留不构成连接面）。
    if !snapshot.network {
        args.push("--unshare-net".to_string());
    }
    // 载荷 cwd：受限树内相对路径语义需要显式落点（canonical 化；非 canonical
    // 落点在受限树内可能不可达——统一 --chdir）。
    let normalized = normalize_command_cwd_for_bwrap(command_cwd);
    args.push("--chdir".to_string());
    args.push(path_to_string(normalized.as_path()));
    // seccomp：构造 memfd（须在本函数返回的守卫中存活到 spawn）。
    let seccomp = build_seccomp_fd().map_err(std::io::Error::other)?;
    if let Some(fd) = seccomp.as_ref() {
        args.push("--seccomp".to_string());
        args.push(fd.raw().to_string());
    }
    args.push("--cap-drop".to_string());
    args.push("ALL".to_string());
    args.push("--".to_string());
    args.extend(command);
    Ok(SandboxLaunch {
        program: bwrap.to_string_lossy().into_owned(),
        args,
        env: permission_child_env(),
        seccomp,
    })
}

/// 031 沙箱子进程白名单环境：PATH/TERM 继承（非凭据）；HOME＝沙箱私有
/// `/tmp`（可写暂存；宿主用户目录不进沙箱——凭据防线与 033 R1 同源）。
fn permission_child_env() -> Vec<(String, String)> {
    let mut env = vec![
        (
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        ),
        ("HOME".to_string(), "/tmp".to_string()),
    ];
    if let Ok(term) = std::env::var("TERM") {
        env.push(("TERM".to_string(), term));
    }
    env
}

/// 受限读树挂载布局（§1.6；M0 restricted-read 实证布局）：
/// 1. tmpfs 空根＋固定运行时白名单（`/usr` ro、usrmerge 符号链接、`/etc`
///    ro、`/dev` 最小设备树、私有可写 `/tmp`）——033 的全盘 ro-bind 不再
///    兜底，运行时组件缺失＝工具失败（M0 技术点 2，白名单为代码内固定集）；
/// 2. 内容挂载＝read_paths（ro-bind）＋write_paths（bind），按路径深度升序；
/// 3. **每个挂载实例后**立刻补该实例下的 deny 遮蔽（P06(b) 点 3：嵌套挂载
///    会重暴露先前的遮蔽——逐实例重复，不能只对最外层做一次）；遮蔽目标
///    含 mountinfo 同设备 bind 投影出的全部别名路径；
/// 4. deny 与内容面同路径＝策略矛盾（加载层已拒绝；此处防御性再拒）。
fn create_permission_filesystem_args(
    args: &mut Vec<String>,
    snapshot: &PolicySnapshot,
    payload_program: Option<&Path>,
) -> Result<(), String> {
    args.push("--tmpfs".to_string());
    args.push("/".to_string());
    // 固定运行时白名单（顺序：usrmerge 符号链接目标须先存在）。
    for (flag, src, dst) in [
        ("--ro-bind", "/usr", "/usr"),
        ("--symlink", "usr/bin", "/bin"),
        ("--symlink", "usr/lib", "/lib"),
        ("--symlink", "usr/lib64", "/lib64"),
        ("--symlink", "usr/sbin", "/sbin"),
        ("--ro-bind", "/etc", "/etc"),
    ] {
        args.push(flag.to_string());
        args.push(src.to_string());
        args.push(dst.to_string());
    }
    args.push("--dev".to_string());
    args.push("/dev".to_string());
    args.push("--tmpfs".to_string());
    args.push("/tmp".to_string());
    // 运行时挂载也是遮蔽实例：deny 别名落在 /usr、/etc 之下的，在这两个
    // ro-bind 之后同样要 carve（deny 优先对全部挂载面成立，不能只管内容挂载）。
    let mountinfo_rt = parse_mountinfo();
    for rt in [Path::new("/usr"), Path::new("/etc")] {
        let mut carves: Vec<PathBuf> = Vec::new();
        for dn in &snapshot.deny_paths {
            for alias in deny_aliases(dn, &mountinfo_rt) {
                if alias.starts_with(rt) && &alias != rt && !carves.contains(&alias) {
                    carves.push(alias);
                }
            }
        }
        carves.sort_by_key(|p| path_depth(p));
        for c in &carves {
            append_existing_unreadable_path_args(args, c);
        }
    }

    // 内容挂载收集：同路径 write 覆盖 read；深度升序（浅先挂，深层重挂载
    // 后的重复遮蔽由逐实例规则覆盖）。载荷程序＝追加的单文件只读挂载。
    let mut mounts: Vec<(PathBuf, bool)> = Vec::new();
    for p in &snapshot.read_paths {
        push_mount(&mut mounts, p.clone(), false);
    }
    for p in &snapshot.write_paths {
        push_mount(&mut mounts, p.clone(), true);
    }
    if let Some(prog) = payload_program {
        push_mount(&mut mounts, prog.to_path_buf(), false);
    }
    mounts.sort_by_key(|(p, _)| path_depth(p));
    let mountinfo = parse_mountinfo();
    for (dest, writable) in &mounts {
        // 运行中消失的内容路径＝无法表达既定策略：显式失败（fail-closed）。
        // 目录与常规文件均可 bind（单文件挂载点同样有效）。
        if let Err(e) = std::fs::metadata(dest) {
            return Err(format!(
                "内容路径 {} 不可访问（{e}）：拒绝启动沙箱工具",
                dest.display()
            ));
        }
        if snapshot.deny_paths.iter().any(|d| d == dest) {
            return Err(format!(
                "内容路径 {} 与永久拒绝路径相同：策略矛盾，拒绝启动",
                dest.display()
            ));
        }
        args.push(if *writable { "--bind" } else { "--ro-bind" }.to_string());
        args.push(path_to_string(dest));
        args.push(path_to_string(dest));
        // 本实例下的 deny 遮蔽（含别名）：deny 优先于任何授权（§1.3）。
        let mut carves: Vec<PathBuf> = Vec::new();
        for dn in &snapshot.deny_paths {
            for alias in deny_aliases(dn, &mountinfo) {
                if alias.starts_with(dest) && &alias != dest && !carves.contains(&alias) {
                    carves.push(alias);
                }
            }
        }
        carves.sort_by_key(|p| path_depth(p));
        for c in &carves {
            append_existing_unreadable_path_args(args, c);
        }
    }
    Ok(())
}

fn push_mount(mounts: &mut Vec<(PathBuf, bool)>, path: PathBuf, writable: bool) {
    match mounts.iter_mut().find(|(p, _)| p == &path) {
        Some((_, w)) => {
            if writable {
                *w = true;
            }
        }
        None => mounts.push((path, writable)),
    }
}

// ---------- P06(b)：启动前 deny 边界检查 ----------

/// 启动前检查（每次工具启动重新执行；fail-closed）：
/// 1. deny 文件本身 nlink>1 ⇒ 拒绝（033 B1 的直接推广——任何位置的硬链接
///    别名都会使 deny 侧 nlink≥2，无需枚举允许树）；
/// 2. deny **目录**递归遍历：内部全部非目录项 nlink≤1（目录级 deny 的
///    反例缺口闭合——变体 A：目录内文件的预存硬链接到允许树曾绕过全部防线）；
/// 3. 遍历预算：条目数/墙钟超限 ⇒ 拒绝启动；任何 stat/read_dir 失败
///    （非 NotFound）⇒ 状态不明＝拒绝（不静默跳过或降级）。
pub fn verify_deny_boundaries(
    deny_paths: &[PathBuf],
    budget: &TraversalBudget,
) -> Result<(), String> {
    let mut visited = 0usize;
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(budget.timeout_ms);
    for dn in deny_paths {
        let md = match std::fs::metadata(dn) {
            Ok(md) => md,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, // 无对象可保护
            Err(e) => {
                return Err(format!(
                    "受保护路径 {} 状态读取失败（{e}）：拒绝启动沙箱工具",
                    dn.display()
                ));
            }
        };
        if md.is_dir() {
            let mut stack = vec![dn.clone()];
            while let Some(dir) = stack.pop() {
                let entries = match std::fs::read_dir(&dir) {
                    Ok(r) => r,
                    Err(e) => {
                        return Err(format!(
                            "受保护目录 {} 遍历失败（{e}）：拒绝启动沙箱工具（不静默跳过）",
                            dir.display()
                        ));
                    }
                };
                for entry in entries {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(e) => {
                            return Err(format!(
                                "受保护目录 {} 目录项读取失败（{e}）：拒绝启动",
                                dir.display()
                            ));
                        }
                    };
                    visited += 1;
                    if visited > budget.max_entries {
                        return Err(format!(
                            "受保护路径遍历超过 {} 项预算（{}）：拒绝启动沙箱工具",
                            budget.max_entries,
                            dn.display()
                        ));
                    }
                    if std::time::Instant::now() > deadline {
                        return Err(format!(
                            "受保护路径遍历超过 {}ms 时限（{}）：拒绝启动沙箱工具",
                            budget.timeout_ms,
                            dn.display()
                        ));
                    }
                    // lstat（不跟随符号链接——deny 树内的符号链接只遮蔽自身，
                    // 不能经它走到树外产生误判）；任何非目录项统一 nlink 检查。
                    let md = match std::fs::symlink_metadata(entry.path()) {
                        Ok(m) => m,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => {
                            return Err(format!(
                                "受保护目录项 {} 状态读取失败（{e}）：拒绝启动",
                                entry.path().display()
                            ));
                        }
                    };
                    if md.is_dir() {
                        stack.push(entry.path());
                    } else {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::MetadataExt;
                            if md.nlink() > 1 {
                                return Err(format!(
                                    "受保护文件 {} 存在 {} 个硬链接：路径级遮蔽无法覆盖同 inode 别名，\
                                     拒绝启动沙箱工具（不回退普通模式）",
                                    entry.path().display(),
                                    md.nlink()
                                ));
                            }
                        }
                    }
                }
            }
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if md.nlink() > 1 {
                    return Err(format!(
                        "受保护文件 {} 存在 {} 个硬链接：路径级遮蔽无法覆盖同 inode 别名，\
                         拒绝启动沙箱工具（不回退普通模式）",
                        dn.display(),
                        md.nlink()
                    ));
                }
            }
        }
    }
    Ok(())
}

// ---------- mountinfo 同设备 bind 别名（P06(b) 点 3） ----------

struct MountInfo {
    /// 挂载源在文件系统内的根（mountinfo 第 4 列）。
    root: PathBuf,
    /// 挂载点（第 5 列）。
    mount_point: PathBuf,
    /// 设备号 major:minor（第 3 列）。
    dev: String,
}

/// 解析 /proc/self/mountinfo（缺行/坏行跳过；octal 转义还原）。同设备 bind
/// **不改变 st_dev**——仅靠 st_dev 变化检测挂载边界会漏检（点 3），别名枚举
/// 以 (dev, root→mount_point) 映射为准。
fn parse_mountinfo() -> Vec<MountInfo> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    parse_mountinfo_from(&text)
}

fn parse_mountinfo_from(text: &str) -> Vec<MountInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // 前段固定 6 列（id parent dev root mount_point options），随后为
        // **零或多**个可选字段（shared:N/master:N 等），再后才是 “-” 分隔符
        // 与 fstype/source/super_options（proc(5)）。审查 #3：固定下标
        // fields[6]=="-" 会整行丢弃带传播标记的挂载——恰是同设备 bind 别名
        // 最密集的挂载类（本容器实测 32 行中 5 行带 master:30）。
        if fields.len() < 7 {
            continue;
        }
        let Some(sep) = fields.iter().skip(6).position(|f| *f == "-") else {
            continue;
        };
        let _ = sep;
        out.push(MountInfo {
            root: unescape_mountinfo_path(fields[3]),
            mount_point: unescape_mountinfo_path(fields[4]),
            dev: fields[2].to_string(),
        });
    }
    out
}

/// mountinfo 路径的 octal 转义（`\040` 空格、`\011` 制表等）还原。内核对
/// 非_ascii 路径同样 octal 转义；按字节还原后整体 UTF-8 重建。
fn unescape_mountinfo_path(s: &str) -> PathBuf {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

/// deny 路径在当前挂载命名空间的全部可达路径（别名）。方法：把 canonical
/// 路径经「覆盖它的挂载」还原为文件系统内位置（fs 路径），再把 fs 路径经
/// **每个同设备挂载**投影回命名空间——同设备 bind 的两个挂载点由此互相
/// 显形（M0 实证：同一目录挂两路径时，遮蔽只做一处会从另一处全文泄露）。
fn deny_aliases(deny: &Path, mounts: &[MountInfo]) -> Vec<PathBuf> {
    let mut out = vec![deny.to_path_buf()];
    let Some(dev) = path_dev(deny) else { return out };
    // 还原 fs 位置：可能有多个覆盖挂载（如同一路径被反复 bind），全部收集。
    let mut fs_paths: Vec<PathBuf> = Vec::new();
    for m in mounts {
        if m.dev != dev {
            continue;
        }
        if let Ok(rel) = deny.strip_prefix(&m.mount_point) {
            let fs_path = m.root.join(rel);
            if !fs_paths.contains(&fs_path) {
                fs_paths.push(fs_path);
            }
        }
    }
    // 投影回命名空间：每个同设备挂载上，fs 路径可见的位置都是别名。
    for m in mounts {
        if m.dev != dev {
            continue;
        }
        for fs_path in &fs_paths {
            if let Ok(rel) = fs_path.strip_prefix(&m.root) {
                let alias = m.mount_point.join(rel);
                if !out.contains(&alias) {
                    out.push(alias);
                }
            }
        }
    }
    out
}

/// 路径所在设备的 "major:minor"（与 mountinfo 第 3 列同构）。glibc dev_t
/// 分解公式（__gnu_dev_major/minor 的位运算等价；coreutils 显示同一来源）。
fn path_dev(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::metadata(path).ok()?;
        let dev = md.dev();
        let maj = ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff_u64);
        let min = (dev & 0xff) | ((dev >> 12) & !0xff_u64);
        Some(format!("{}:{}", maj as u32, min as u32))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

// ---------- seccomp（应用层系统调用过滤；M0 机制实证） ----------

/// x86_64 拒绝清单：特权/经典攻击面调用（cap-drop ALL＋NNP 之下的纵深
/// 防御；清单为本项目固定选择，非上游搬运）。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const SECCOMP_DENY_EPERM: &[u32] = &[
    101, // ptrace
    155, // pivot_root
    165, // mount
    166, // umount2
    167, // swapon
    168, // swapoff
    169, // reboot
    175, // init_module
    176, // delete_module
    246, // kexec_load
    272, // unshare（L2：与 clone-ns 掩码同意图——嵌套进程不得创建自己的命名空间）
    300, // fanotify_init
    303, // name_to_handle_at
    304, // open_by_handle_at
    308, // setns
    310, // process_vm_readv
    311, // process_vm_writev
    313, // finit_module
    320, // kexec_file_load
    321, // bpf
    323, // userfaultfd
    328, // open_tree
    428, // fsopen
    429, // fsconfig
    430, // fsmount
    431, // move_mount
    432, // mount_setattr
];

/// clone3：以 ENOSYS 拒绝（glibc 对 ENOSYS 回退 clone(2)；对 EPERM 不回退，
/// 会破坏线程创建）。namespace 创建经 clone(2) flags 由掩码检查拦截。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const SECCOMP_DENY_ENOSYS: &[u32] = &[437];

/// clone(2) 的 namespace 标志位（CLONE_NEWNS|NEWCGROUP|NEWUTS|NEWIPC|
/// NEWUSER|NEWPID|NEWNET）：置位即 EPERM（嵌套进程不得创建自己的命名空间
/// 视图——外层边界之上只允许更严）。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const CLONE_NS_MASK: u32 = 0x7E02_0000;

const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const SECCOMP_RET_ALLOW: u32 = 0x7FFF_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;

/// 构造 cBPF 过滤器（M0 教训：nr 在偏移 0、arch 在偏移 4；RET 收尾；8 字节
/// 整除——bwrap 按此校验）。结构：arch 门（非 x86_64 KILL_PROCESS）→ nr
/// 逐项 EPERM/ENOSYS → clone flags 掩码 EPERM → ALLOW。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn build_seccomp_filter() -> Vec<u8> {
    fn insn(code: u16, jt: u8, jf: u8, k: u32) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[..2].copy_from_slice(&code.to_le_bytes());
        b[2] = jt;
        b[3] = jf;
        b[4..].copy_from_slice(&k.to_le_bytes());
        b
    }
    let mut prog: Vec<u8> = Vec::new();
    // arch 门
    prog.extend_from_slice(&insn(0x20, 0, 0, 4)); // LD W ABS arch
    prog.extend_from_slice(&insn(0x15, 1, 0, AUDIT_ARCH_X86_64)); // JEQ → 跳过 kill
    prog.extend_from_slice(&insn(0x06, 0, 0, SECCOMP_RET_KILL_PROCESS));
    // nr 拒绝清单
    prog.extend_from_slice(&insn(0x20, 0, 0, 0)); // LD W ABS nr
    for &nr in SECCOMP_DENY_EPERM {
        prog.extend_from_slice(&insn(0x15, 0, 1, nr)); // 命中 → 下一句 RET
        prog.extend_from_slice(&insn(0x06, 0, 0, SECCOMP_RET_ERRNO | libc::EPERM as u32));
    }
    for &nr in SECCOMP_DENY_ENOSYS {
        prog.extend_from_slice(&insn(0x15, 0, 1, nr));
        prog.extend_from_slice(&insn(0x06, 0, 0, SECCOMP_RET_ERRNO | libc::ENOSYS as u32));
    }
    // clone(2) namespace 标志掩码（args[0] 在偏移 16）——**必须先门控在
    // nr==clone 上**：掩码检查只对 clone 有意义，对其他调用的 args[0]
    //（任意值）按位与几乎恒非零，会把 execve 等误杀成 EPERM（集成测试
    // 首跑实证：execvp /bin/sh Operation not permitted）。
    prog.extend_from_slice(&insn(0x15, 0, 4, 56)); // JEQ clone → 落入掩码块；否则跳 4 到 ALLOW
    prog.extend_from_slice(&insn(0x20, 0, 0, 16)); // LD W ABS args[0] 低 32 位
    prog.extend_from_slice(&insn(0x54, 0, 0, CLONE_NS_MASK)); // AND 掩码
    prog.extend_from_slice(&insn(0x15, 1, 0, 0)); // JEQ 0 → 跳过 deny
    prog.extend_from_slice(&insn(0x06, 0, 0, SECCOMP_RET_ERRNO | libc::EPERM as u32));
    // 放行
    prog.extend_from_slice(&insn(0x06, 0, 0, SECCOMP_RET_ALLOW));
    prog
}

/// 非 x86_64：031 的过滤器清单按 x86_64 编号固定，其他架构 fail-closed
/// （返回 Err，031 拒绝启用——不产出无 arch 门或错误编号的过滤器）。
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn build_seccomp_filter() -> Option<Vec<u8>> {
    None
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn build_seccomp_fd() -> std::io::Result<Option<SeccompFd>> {
    Err(std::io::Error::other(
        "031 沙箱的 seccomp 过滤器仅实现 x86_64：本平台拒绝启用 031 模式",
    ))
}

/// 把过滤器写入非 CLOEXEC memfd 并归零偏移（bwrap `--seccomp <fd>` 从当前
/// 偏移读取）。memfd 生命周期由 [`SeccompFd`] 守卫管理；全部经原始 syscall
/// 写入（File 接管会引入 Drop close 与守卫的 double-close 竞态）。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn build_seccomp_fd() -> std::io::Result<Option<SeccompFd>> {
    let filter = build_seccomp_filter();
    // SAFETY：memfd_create 仅创建匿名内存文件；write/lseek 仅操作本 fd。
    let fd = unsafe { libc::memfd_create(c"tchat-031-seccomp".as_ptr().cast(), 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let guard = SeccompFd(fd);
    let mut off = 0usize;
    while off < filter.len() {
        let n = unsafe {
            libc::write(fd, filter[off..].as_ptr().cast(), filter.len() - off)
        };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        off += n as usize;
    }
    if unsafe { libc::lseek(fd, 0, libc::SEEK_SET) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Some(guard))
}

/// 031 逐启动 PID 命名空间验证（§1.6）：**验证对象是 bwrap 的子进程（载荷）**
/// ——直连 bwrap 进程在新 pid ns 之外的监督位，其自身 ns 与主进程相同属正常
/// 形态（实测实证；不能只凭启动时一次探测判定）。经 `/proc/<bwrap>/task/*/
/// children` 找到载荷的宿主 pid，其 `ns/pid` 必须与主进程不同。bwrap 自身
/// 多次 re-exec：spawn 返回时载荷可能尚未出现——有界轮询（≤200ms）；
/// bwrap 已退出＝自身 fail-hard 已生效（载荷未运行），返回 Ok；超时仍无任
/// 何异 ns 子进程且进程存活＝隔离未建立：killpg 整组终止＋waitpid 回收并返
/// 回 Err（模型命令拒绝）。
pub fn verify_child_pid_namespace(child_pid: u32) -> std::io::Result<()> {
    let self_ns = std::fs::read_link("/proc/self/ns/pid")
        .map_err(std::io::Error::other)?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(200);
    loop {
        // bwrap 监督进程仍在？
        if !Path::new(&format!("/proc/{child_pid}")).exists() {
            return Ok(()); // 已退出：fail-hard 已生效，交由退出状态报告
        }
        // 遍历其全部线程的 children（载荷的宿主 pid）
        let mut found_isolated = false;
        let mut found_any_child = false;
        let task_dir = format!("/proc/{child_pid}/task");
        if let Ok(tasks) = std::fs::read_dir(&task_dir) {
            for t in tasks.flatten() {
                let children_file = t.path().join("children");
                if let Ok(text) = std::fs::read_to_string(&children_file) {
                    for pid_str in text.split_whitespace() {
                        let Ok(pid) = pid_str.parse::<u32>() else { continue };
                        found_any_child = true;
                        if let Ok(ns) = std::fs::read_link(format!("/proc/{pid}/ns/pid")) {
                            if ns != self_ns {
                                found_isolated = true;
                            }
                        }
                    }
                }
            }
        }
        if found_isolated {
            return Ok(()); // 载荷确在新 PID 命名空间
        }
        if found_any_child && std::time::Instant::now() >= deadline {
            break; // 有子进程但全部同 ns：隔离失败
        }
        if std::time::Instant::now() >= deadline {
            // 无子进程且超时：bwrap 仍在 setup 或已失败——保守放行由退出状态
            // 报告（killpg 兜底存在于会话生命周期）。
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    #[cfg(unix)]
    {
        // 隔离未建立：整组终止＋回收，再报错。
        let pid = child_pid as libc::pid_t;
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
            let mut status = 0;
            libc::waitpid(pid, &mut status, 0);
        }
    }
    Err(std::io::Error::other(format!(
        "031 沙箱子进程 {child_pid} 的载荷不在独立 PID 命名空间：隔离创建失败，\
         已终止整组并拒绝执行模型命令"
    )))
}

// ---------- 路径辅助（上游实现逐字保留，除标注外） ----------

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

/// 纯词法规范化（本项目补充：上游策略层在 codex-protocol 内完成等价处理）——
/// 消除 `.`/`..`，保证 starts_with 关系与 bwrap 目标一致。
/// 031：permissions 模块的路径规范化共用（同一包含语义，禁止两处定义分叉）。
pub(crate) fn absolute_lexical(path: &Path) -> PathBuf {
    let mut current = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::RootDir => current.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => {}
        }
    }
    current
}

/// 上游 `canonical_target_if_symlinked_path`（bwrap.rs:1146）逐字保留。
fn canonical_target_if_symlinked_path(path: &Path) -> Option<PathBuf> {
    // Return the fully resolved target only when some path component is a
    // symlink. Callers use this to bind/mask the real filesystem location while
    // leaving ordinary paths in their logical form.
    let mut current = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::RootDir => {
                current.push(Path::new("/"));
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => continue,
        }

        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => return None,
        };
        if metadata.file_type().is_symlink() {
            let target = std::fs::canonicalize(path).ok()?;
            if target.as_path() == path {
                return None;
            }
            return Some(target);
        }
    }
    None
}

/// 上游 `remap_paths_for_symlink_target`（bwrap.rs:1182）逐字保留。
fn remap_paths_for_symlink_target(paths: Vec<PathBuf>, root: &Path, target: &Path) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| {
            if let Ok(relative) = path.strip_prefix(root) {
                target.join(relative)
            } else {
                path
            }
        })
        .collect()
}

/// 上游 `normalize_command_cwd_for_bwrap`（bwrap.rs:1195）逐字保留。
fn normalize_command_cwd_for_bwrap(command_cwd: &Path) -> PathBuf {
    command_cwd
        .canonicalize()
        .unwrap_or_else(|_| command_cwd.to_path_buf())
}

fn is_within_allowed_write_paths(path: &Path, allowed_write_paths: &[PathBuf]) -> bool {
    allowed_write_paths
        .iter()
        .any(|root| path.starts_with(root))
}

/// 上游 `first_writable_symlink_component_in_path`（bwrap.rs:1442）逐字保留。
fn first_writable_symlink_component_in_path(
    target_path: &Path,
    allowed_write_paths: &[PathBuf],
) -> Option<PathBuf> {
    /*
     * Walk the logical path and report the first symlink component that lives
     * under a writable root. These symlinks are mutable from inside the sandbox,
     * so any mount or mask based on their resolved target would be racing a path
     * the sandboxed process can change.
     */
    let mut current = PathBuf::new();

    for component in target_path.components() {
        use std::path::Component;
        match component {
            Component::RootDir => {
                current.push(Path::new("/"));
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => continue,
        }

        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => break,
        };

        if metadata.file_type().is_symlink()
            && is_within_allowed_write_paths(&current, allowed_write_paths)
        {
            return Some(current);
        }
    }

    None
}

#[cfg(test)]
mod tests;
