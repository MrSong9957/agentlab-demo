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

// ---------- bwrap 探测（failIfUnavailable） ----------

/// 探测结果进程级缓存：`Err` 文案同样缓存（探测含真实 bwrap 干跑，代价不低；
/// 同一进程内环境不变，结果可复用）。测试经 [`resolve_launcher_with`] 注入。
static LAUNCHER: OnceLock<Result<PathBuf, String>> = OnceLock::new();

fn resolve_launcher() -> Result<&'static PathBuf, String> {
    let init: &Result<PathBuf, String> = LAUNCHER.get_or_init(probe_bwrap);
    init.as_ref().map_err(|e| e.clone())
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

    if !unreadable_root.exists() {
        // 上游：缺失路径用 `--ro-bind-data <fd>` 预创建空文件，阻止沙箱内重建
        // 层级。本项目适配（无 fd mount，见下）：deny 目标是已存在的凭据文件，
        // missing＝当前无凭据可保护，no-op；沙箱内若新建 .env 属写入面（027
        // 语义为读保护，写面与普通模式一致开放）。
        return Ok(());
    }

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

// ---------- 路径辅助（上游实现逐字保留，除标注外） ----------

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

/// 纯词法规范化（本项目补充：上游策略层在 codex-protocol 内完成等价处理）——
/// 消除 `.`/`..`，保证 starts_with 关系与 bwrap 目标一致。
fn absolute_lexical(path: &Path) -> PathBuf {
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
