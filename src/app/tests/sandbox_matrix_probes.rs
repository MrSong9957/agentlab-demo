//! 033 矩阵实测 C/D/E/F 补充探针（2026-10-27）：经**生产分派路径**（turn.rs 同款
//! 库入口：ProcessManager::start / apply_patch_sandboxed_with_exe / McpManager）的
//! 沙箱行为探针。
//!
//! **子进程隔离**（同 e2e_acceptance tool_child_mode 模式）：每项探针由父测试以
//! 显式构造的环境派生 `current_exe --exact probe_child`——TCHAT_SANDBOX 默认清除、
//! 按场景显式置 1，PATH 可替换（fail-closed 场景）。进程边界消除环境变量污染，
//! 父进程不 set/remove 任何进程级变量，全量回归下与普通用例并行安全。
//!
//! 覆盖：C MCP 面（服务器经 bwrap 启动后读锚定根 .env 被遮蔽＋显式 env 契约保留
//! ＋TERM/HOME 白名单）与 apply_patch 面（Update/Delete 触碰 .env 被拒、外部不变）；
//! D 无 CLOEXEC 假凭据 fd 的继承边界（bwrap 不关闭继承 fd——已知边界记录）；
//! E 取消/超时（管道＋PTY）与正常完成的整批回收（含后台子进程＋孙进程，沙箱内
//! 外双侧扫描＋时序敏感项重复）；F fail-closed 四场景（bwrap 缺失/不可执行/探测
//! 失败/启动失败）×三工具面，执行标记不得出现、无静默回退。全程假凭据。

mod common;

use common::TempDir;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use terminal_chat::exec::{ExecArgs, ExecResult};
use terminal_chat::mcp::McpManager;
use terminal_chat::proc_session::ProcessManager;

const WATCHDOG: Duration = Duration::from_secs(90);

// ---------- 子进程探针入口 ----------

#[test]
fn probe_child() {
    let Some(input) = std::env::var_os("TCHAT_033_PROBE_INPUT") else { return; };
    let output = std::env::var_os("TCHAT_033_PROBE_OUTPUT").expect("缺少 TCHAT_033_PROBE_OUTPUT");
    let spec: Value = serde_json::from_str(&fs::read_to_string(&input).unwrap()).unwrap();
    let outcome = match spec["kind"].as_str().expect("探针类型") {
        "exec" => exec_probe(&spec),
        "apply_patch" => apply_patch_probe(&spec),
        "mcp" => mcp_probe(&spec),
        other => panic!("未知探针类型 {other}"),
    };
    fs::write(&output, serde_json::to_string(&outcome).unwrap()).unwrap();
    std::process::exit(0);
}

/// /proc 下 cmdline 含 marker 的进程 pid 列表（父/子两侧共用；空＝无遗留）。
fn scan_marker(marker: &str) -> Vec<u32> {
    if marker.is_empty() {
        return Vec::new();
    }
    fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .filter(|pid| {
            fs::read_to_string(format!("/proc/{pid}/cmdline"))
                .map(|c| c.contains(marker))
                .unwrap_or(false)
        })
        .collect()
}

/// exec 探针（生产入口 ProcessManager::start）：支持取消延时（cancel_after_ms）、
/// 终止后双侧扫描（before_close＝mgr.close() 前、after_close＝后）与无 CLOEXEC
/// 假凭据 fd 注入（fd_path；fd 号回填 cmd 中的 __FD__ 占位符）。
fn exec_probe(spec: &Value) -> Value {
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    if let Some(ms) = spec["cancel_after_ms"].as_u64() {
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            flag.store(true, Ordering::Relaxed);
        });
    }
    let mut fd = Value::Null;
    let mut input = spec["input"].clone();
    // 038：假凭据 fd 注入为 unix 设施探针（fcntl CLOEXEC 语义），Windows
    // 不执行（fd_path 场景仅由 unix 门控的探针使用）。
    #[cfg(unix)]
    if let Some(path) = spec["fd_path"].as_str() {
        use std::os::fd::AsRawFd;
        let f = fs::File::open(path).expect("打开假凭据 fd");
        let n = f.as_raw_fd();
        // 清除 CLOEXEC 并跨 spawn 保持打开（mem::forget 防 Drop 关闭）——模拟
        // 父进程遗留的可继承 fd（本轮创建的假凭据文件）。
        unsafe { assert_eq!(libc::fcntl(n, libc::F_SETFD, 0), 0, "清除 CLOEXEC 失败"); }
        std::mem::forget(f);
        if let Some(cmd) = input["cmd"].as_str() {
            input["cmd"] = json!(cmd.replace("__FD__", &n.to_string()));
        }
        fd = json!(n);
    }
    #[cfg(windows)]
    let _ = &mut input;
    let marker = spec["scan_marker"].as_str().unwrap_or("");
    let mgr = ProcessManager::new();
    let result = match ExecArgs::from_value(&input) {
        Ok(args) => mgr.start(&args, Some(cancel.as_ref())).result,
        Err(msg) => ExecResult::error(msg),
    };
    std::thread::sleep(Duration::from_millis(600)); // 终止/回收异步落定
    // 038-CI：组杀→进程消亡在慢 CI 上超过固定等待——按项目时序约定改 poll＋
    // 截止（取消场景期望进程仍在运行，不适用 poll-until-empty）。
    let mut before = scan_marker(marker);
    if spec["cancel_after_ms"].is_null() {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !before.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            before = scan_marker(marker);
        }
    }
    mgr.close();
    let mut after = scan_marker(marker);
    let after_deadline = Instant::now() + Duration::from_secs(15);
    while !after.is_empty() && Instant::now() < after_deadline {
        std::thread::sleep(Duration::from_millis(200));
        after = scan_marker(marker);
    }
    json!({"result": result, "before_close": before, "after_close": after, "fd": fd})
}

/// apply_patch 探针（生产入口 turn.rs 同款 apply_patch_sandboxed；测试进程的
/// current_exe 是测试二进制，按既有测试缝传 CARGO_BIN_EXE）。
fn apply_patch_probe(spec: &Value) -> Value {
    let exe = Path::new(env!("CARGO_BIN_EXE_terminal-chat"));
    let (result, changes) =
        terminal_chat::apply_patch::apply_patch_sandboxed_with_exe(exe, &spec["input"], None);
    json!({"result": result, "changes": changes.len()})
}

/// MCP 探针（生产入口 McpManager prepare/call/close）：mcp.json 由父测试写入
/// TCHAT_USER_DIR；探针记录文本（rec）由服务器侧包装脚本落盘。可选 fd_path＝
/// launcher 角色无 CLOEXEC 假凭据 fd（B2 的 MCP 面专项）。
fn mcp_probe(spec: &Value) -> Value {
    // B2：launcher 角色 fd——清除 CLOEXEC 并跨 spawn 保持（子进程退出即关闭）。
    // 038：unix 设施探针（fcntl 语义），Windows 不执行。
    #[cfg(unix)]
    if let Some(path) = spec["fd_path"].as_str() {
        use std::os::fd::AsRawFd;
        let f = fs::File::open(path).expect("打开假凭据 fd");
        let n = f.as_raw_fd();
        unsafe { assert_eq!(libc::fcntl(n, libc::F_SETFD, 0), 0, "清除 CLOEXEC 失败"); }
        std::mem::forget(f);
    }
    let manager = McpManager::new();
    let prepared = manager.prepare_for_turn(&AtomicBool::new(false));
    let Some(snapshot) = prepared else {
        manager.close();
        return json!({"started": false, "tools": 0, "call": null, "probe_rec": ""});
    };
    let tools = snapshot.len();
    let call = snapshot.entry("mcp__fx__echo").map(|entry| {
        let (result, failed) = manager.call(entry, json!({"text": "iso"}), &AtomicBool::new(false));
        json!({"result": result, "failed": failed})
    });
    manager.close();
    let probe_rec =
        fs::read_to_string(spec["rec"].as_str().expect("rec 路径")).unwrap_or_default();
    json!({"started": true, "tools": tools, "call": call, "probe_rec": probe_rec})
}

// ---------- 父侧：子进程派生（显式环境构造，看门狗） ----------

fn run_probe(tag: &str, spec: &Value, setup: impl FnOnce(&mut Command)) -> Value {
    let io = TempDir::new(&format!("033p-{tag}"));
    let in_path = io.path().join("in.json");
    let out_path = io.path().join("out.json");
    fs::write(&in_path, spec.to_string()).unwrap();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.arg("probe_child")
        .arg("--exact")
        .env("TCHAT_033_PROBE_INPUT", &in_path)
        .env("TCHAT_033_PROBE_OUTPUT", &out_path)
        // 默认清除部署注入的开关：沙箱场景由 setup 显式置 1（隔离构造，不依赖
        // 本测试进程的运行环境，全量回归 env -u 下同样有效）。
        .env_remove("TCHAT_SANDBOX")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    setup(&mut cmd);
    let mut child = cmd.spawn().expect("探针子进程启动失败");
    let deadline = Instant::now() + WATCHDOG;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("探针看门狗超时（{tag}）");
            }
        }
    };
    assert!(status.success(), "探针子进程异常退出（{tag}）：{status}");
    serde_json::from_str(
        &fs::read_to_string(&out_path)
            .unwrap_or_else(|e| panic!("探针成功退出但未写结果（{tag}）：{e}")),
    )
    .unwrap()
}

/// 沙箱场景公共环境：cwd＝锚定根、开关显式置 1。
fn sandbox_env(work: &Path) -> Box<dyn FnOnce(&mut Command) + '_> {
    Box::new(move |cmd: &mut Command| {
        cmd.current_dir(work).env("TCHAT_SANDBOX", "1");
    })
}

// ---------- 矩阵 C：MCP 面 / apply_patch 面的假凭据边界 ----------

/// C-MCP：服务器经生产 transport（bwrap 包装）启动——沙箱内读锚定根 .env 被遮蔽
/// （外部先证可读）；mcp.json 显式 env 保留；HOME＝锚定根、TERM 传递、污染 env
/// 不进入（env_clear）；载荷＝新 PID ns 的 pid 1。普通模式对照：.env 可读、
/// HOME＝主进程值、pid≠1——差异证明来自沙箱。
#[cfg(target_os = "linux")]
// 038-CI：本探针要求 bwrap 真实执行（033 沙箱后端），macOS/Windows 不编译。
#[test]
fn c_mcp_face_masks_env_and_keeps_explicit_env() {
    for sandbox in [true, false] {
        let work = TempDir::new("033mx-mcp-work");
        let user = TempDir::new("033mx-mcp-user");
        let recs = TempDir::new("033mx-mcp-rec");
        let home = user.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let marker = format!("matrix-fake-033-mcp-{}", std::process::id());
        let env_file = work.path().join(".env");
        fs::write(&env_file, format!("TCHAT_033_FAKE_ENV_SECRET={marker}\n")).unwrap();
        assert!(
            fs::read_to_string(&env_file).unwrap().contains(&marker),
            "前置：假凭据在沙箱外存在、可读且含唯一标记"
        );

        let rec = recs.path().join("probe.txt");
        let rec2 = recs.path().join("fixture.jsonl");
        let fixture = common::mcp_fixtures::FIXTURE.replace('\\', "/");
        let anchor = work.path().display().to_string();
        // 服务器包装脚本：记录自身环境与 .env 读取尝试后，转 exec 真实测试夹具
        //（同一进程＝bwrap 直接载荷）。
        let wrapper = format!(
            r#"{{ echo "PID=$$"; echo "HOME=$HOME"; echo "TERM=$TERM"; echo "TOKEN=[$TCHAT_033_MCP_TOKEN]"; echo "POISON=[$TCHAT_033_POISON]"; echo "ENV_READ=$(cat '{anchor}/.env' 2>&1 | head -c 90)"; }} >> '{rec}'; exec python3 '{fixture}' --scenario echo --record '{rec2}'"#,
            anchor = anchor,
            rec = rec.display(),
            fixture = fixture,
            rec2 = rec2.display(),
        );
        let mcp = json!({"mcp_servers": {"fx": {
            "enabled": true, "command": "sh", "args": ["-c", wrapper],
            "cwd": anchor, "env_vars": ["TCHAT_033_MCP_TOKEN"]}}});
        fs::write(user.path().join("mcp.json"), mcp.to_string()).unwrap();

        let spec = json!({"kind": "mcp", "rec": rec.display().to_string()});
        let outcome = run_probe("mcp", &spec, |cmd| {
            cmd.env("TCHAT_USER_DIR", user.path())
                .env("TCHAT_033_MCP_TOKEN", "canary-033mx")
                .env("TCHAT_033_POISON", "poison-033mx")
                .env("TERM", "xterm-033mx")
                .env("HOME", &home)
                .env("LC_ALL", "C");
            if sandbox {
                cmd.env("TCHAT_SANDBOX", "1").current_dir(work.path());
            }
        });
        assert_eq!(outcome["started"], json!(true), "服务器应启动：{outcome}");
        assert_eq!(outcome["tools"], json!(1), "应发现 echo 工具：{outcome}");
        assert_eq!(outcome["call"]["failed"], json!(false), "调用往返成功：{outcome}");
        assert!(
            outcome["call"]["result"].as_str().is_some_and(|r| r.contains("echo:iso")),
            "echo 往返：{outcome}"
        );
        let rec_text = outcome["probe_rec"].as_str().unwrap();
        let val = |k: &str| {
            rec_text
                .lines()
                .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
                .unwrap_or("<missing>")
        };
        if sandbox {
            assert!(!rec_text.contains(&marker), "MCP 面不得读出 .env 正文：{rec_text}");
            assert!(
                val("ENV_READ").contains("denied"),
                "遮蔽证据（open 被拒文本）：{rec_text}"
            );
            assert_eq!(val("PID"), "1", "服务器＝新 ns pid 1：{rec_text}");
            assert_eq!(val("HOME"), anchor.as_str(), "HOME＝锚定根：{rec_text}");
        } else {
            assert!(rec_text.contains(&marker), "普通对照：.env 可读：{rec_text}");
            assert_ne!(val("PID"), "1", "普通模式 pid≠1：{rec_text}");
            assert_eq!(
                val("HOME"),
                home.display().to_string(),
                "普通模式 HOME＝主进程值：{rec_text}"
            );
        }
        assert_eq!(val("TOKEN"), "[canary-033mx]", "mcp.json 显式 env 契约保留：{rec_text}");
        assert_eq!(val("POISON"), "[]", "污染 env 不进入（env_clear）：{rec_text}");
        assert_eq!(val("TERM"), "xterm-033mx", "TERM 白名单传递：{rec_text}");
        assert!(
            fs::read_to_string(&env_file).unwrap().contains(&marker),
            "外部假凭据未被修改"
        );
        // B：正常退出（manager.close）后服务器进程应全部回收——fixture 的
        // record 路径唯一，含于服务器 cmdline，扫描即回收证据（close 有界回收
        // 后留 400ms 落定余量）。
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            scan_marker(&rec2.display().to_string()).is_empty(),
            "{mode}：close 后服务器进程应全部回收",
            mode = if sandbox { "沙箱" } else { "普通对照" },
        );
    }
}

/// C-apply_patch：补丁经 bwrap 包装的 internal-apply 子进程——Update/Delete
/// 触碰锚定根 .env 全部被拒（读取失败），外部文件字节不变。
#[test]
fn c_apply_patch_face_cannot_touch_env() {
    let work = TempDir::new("033mx-ap-work");
    let marker = format!("matrix-fake-033-ap-{}", std::process::id());
    let env_file = work.path().join(".env");
    let original = format!("TCHAT_033_FAKE_ENV_SECRET={marker}\n");
    fs::write(&env_file, &original).unwrap();

    let update = format!(
        "*** Begin Patch\n*** Update File: .env\n@@\n-TCHAT_033_FAKE_ENV_SECRET={marker}\n+EVIL-REPLACED\n*** End Patch\n"
    );
    let delete = "*** Begin Patch\n*** Delete File: .env\n*** End Patch\n".to_string();
    for (tag, patch) in [("update", update), ("delete", delete)] {
        let outcome = run_probe(
            "ap",
            &json!({"kind": "apply_patch", "input": {"patch": patch}}),
            sandbox_env(work.path()),
        );
        let result = &outcome["result"];
        assert_ne!(result["ok"], json!(true), "{tag}：触碰 .env 必须被拒：{result}");
        assert!(
            !result["error"].as_str().unwrap_or("").contains(&marker),
            "{tag}：错误信息不得回显 .env 正文：{result}"
        );
        assert_eq!(
            fs::read_to_string(&env_file).unwrap(),
            original,
            "{tag}：外部假凭据字节不变"
        );
        println!("033 C-apply_patch {tag} 结果：{result}");
    }
}

// ---------- 矩阵 B1（2026-10-27 加固）：预建硬链接的 deny 目标＝三面拒绝 ----------

/// B1：锚定根 `.env` 存在**预建硬链接**（nlink>1）时，三型工具面全部显式拒绝
///（错误指向硬链接）、无执行副作用、不回退普通模式；单链接控制组照常运行且
/// 遮蔽有效。只处理检查时已存在的硬链接——检查到挂载之间及运行期的并发变化
/// 仍是边界（如实声明，见 B1/B2 核实记录）。
#[cfg(target_os = "linux")]
// 038-CI：本探针要求 bwrap 真实执行（033 沙箱后端），macOS/Windows 不编译。
#[test]
fn b1_hardlinked_deny_refused_on_all_three_faces() {
    // ── 多链接：三面拒绝 ──
    let work = TempDir::new("033b1-hl");
    let env_file = work.path().join(".env");
    let marker = format!("matrix-fake-033-b1-{}", std::process::id());
    fs::write(&env_file, format!("TCHAT_033_FAKE_ENV_SECRET={marker}\n")).unwrap();
    assert!(fs::hard_link(&env_file, work.path().join("env-alias")).is_ok(), "夹具：预建硬链接");

    // exec 面：显式拒绝＋无执行副作用
    let exec_marker = work.path().join("exec-ran.txt");
    let outcome = run_probe(
        "b1-exec",
        &json!({"kind": "exec", "input": {
            "cmd": format!("echo ran > {}", exec_marker.display()),
            "workdir": work.path().display().to_string(),
            "timeout_ms": 10000, "yield_time_ms": 10000,
        }}),
        sandbox_env(work.path()),
    );
    let result = &outcome["result"];
    assert_eq!(result["status"], "error", "exec 面应显式拒绝：{result}");
    assert!(
        result["error"].as_str().unwrap_or("").contains("硬链接"),
        "exec 面错误应指向硬链接：{result}"
    );
    assert!(!exec_marker.exists(), "拒绝时不得有执行副作用");

    // apply_patch 面：拒绝＋文件不产生
    let pwned = work.path().join("pwned.txt");
    let outcome = run_probe(
        "b1-ap",
        &json!({"kind": "apply_patch", "input": {"patch": format!(
            "*** Begin Patch\n*** Add File: {}\n+must-not-exist\n*** End Patch\n", pwned.display())}}),
        sandbox_env(work.path()),
    );
    assert_ne!(outcome["result"]["ok"], json!(true), "apply_patch 面应拒绝：{}", outcome["result"]);
    assert!(
        outcome["result"]["error"].as_str().unwrap_or("").contains("硬链接"),
        "apply_patch 面错误应指向硬链接：{}", outcome["result"]
    );
    assert!(!pwned.exists(), "拒绝时不得产生文件");

    // MCP 面：服务器不启动（marker 不出现）、无工具（022 容错语义内）
    let user = TempDir::new("033b1-user");
    let recs = TempDir::new("033b1-rec");
    let srv_marker = recs.path().join("srv-started.txt");
    let mcp = json!({"mcp_servers": {"fx": {
        "enabled": true, "command": "sh",
        "args": ["-c", format!("echo started >> '{}'; exec sleep 30", srv_marker.display())],
        "cwd": work.path().display().to_string()}}});
    fs::write(user.path().join("mcp.json"), mcp.to_string()).unwrap();
    let outcome = run_probe(
        "b1-mcp",
        &json!({"kind": "mcp", "rec": recs.path().join("unused.txt").display().to_string()}),
        |cmd: &mut Command| {
            cmd.current_dir(work.path())
                .env("TCHAT_USER_DIR", user.path())
                .env("TCHAT_SANDBOX", "1");
        },
    );
    assert!(
        outcome["started"] == json!(false) || outcome["tools"] == json!(0),
        "MCP 面不得发现工具：{outcome}"
    );
    assert!(!srv_marker.exists(), "MCP 服务器不得启动（无回退）");

    // 外部假凭据未被改动
    assert!(
        fs::read_to_string(&env_file).unwrap().contains(&marker),
        "拒绝操作不得改动用户文件"
    );

    // ── 单链接控制组：照常运行且遮蔽有效 ──
    let work2 = TempDir::new("033b1-single");
    let env2 = work2.path().join(".env");
    fs::write(&env2, format!("TCHAT_033_FAKE_ENV_SECRET={marker}\n")).unwrap();
    let outcome = run_probe(
        "b1-ctl",
        &json!({"kind": "exec", "input": {
            "cmd": format!("echo ENV=$(cat .env 2>/dev/null | grep -c {marker} || true); echo OK-033B1"),
            "workdir": work2.path().display().to_string(),
            "timeout_ms": 10000, "yield_time_ms": 10000,
        }}),
        sandbox_env(work2.path()),
    );
    let r = &outcome["result"];
    assert_eq!(r["status"], "completed", "单链接控制组应照常运行：{r}");
    let out_text = r["output"].as_str().unwrap_or_default();
    assert!(out_text.contains("ENV=0"), "单链接 deny 照常遮蔽：{r}");
    assert!(out_text.contains("OK-033B1"), "命令确实执行（防空跑）：{r}");
}

/// B1 收口（2026-10-27）：deny 目标**无法核实**（稳定条件：`.env` 为自指符号
/// 链接，metadata 恒 ELOOP）＝端到端在工具启动前显式拒绝、无执行副作用、
/// 夹具未变。真实 CLI 固定策略下该夹具先命中可写符号链接 fail-closed 分支
///（错误含 "writable symlink"）；元数据错误三分类的单元证据见 sandbox::tests
///（ELOOP rootless／父目录 EACCES 两个稳定夹具＋ENOTDIR 控制组）。
#[cfg(target_os = "linux")]
// 038-CI：本探针要求 bwrap 真实执行（033 沙箱后端），macOS/Windows 不编译。
#[test]
fn b1_unverifiable_deny_target_refused_before_launch() {
    let work = TempDir::new("033b1-loop");
    let env_file = work.path().join(".env");
    std::os::unix::fs::symlink(&env_file, &env_file).expect("夹具：自指符号链接");
    let exec_marker = work.path().join("exec-ran.txt");
    let outcome = run_probe(
        "b1-loop",
        &json!({"kind": "exec", "input": {
            "cmd": format!("echo ran > {}; cat .env", exec_marker.display()),
            "workdir": work.path().display().to_string(),
            "timeout_ms": 10000, "yield_time_ms": 10000,
        }}),
        sandbox_env(work.path()),
    );
    let result = &outcome["result"];
    assert_eq!(result["status"], "error", "无法核实的 deny 目标应显式拒绝：{result}");
    let err = result["error"].as_str().unwrap_or("");
    assert!(
        err.contains("writable symlink"),
        "应命中 fail-closed 拒绝而非放行（普通模式下 cat 循环链接也会失败，但那不是沙箱拒绝）：{result}"
    );
    assert!(!exec_marker.exists(), "拒绝发生在工具启动前：无执行副作用");
    assert!(
        fs::symlink_metadata(&env_file)
            .expect("夹具仍在")
            .file_type()
            .is_symlink(),
        "夹具（循环符号链接）未被改动"
    );
}

// ---------- 矩阵 D/B2：无 CLOEXEC fd 的沙箱收口与普通对照 ----------

/// D-FD（B2 加固后）：launcher 遗留的无 CLOEXEC fd——沙箱分支经 pre_exec
/// `close_range(CLOSE_RANGE_CLOEXEC)` 于 execve 时关闭（`/proc/self/fd/N`
/// 不再可读）；普通对照分支继承行为不变（零变化承诺）。动态 fd 编号（避免与
/// 并发用例的 PTY/套接字 fd 互踩）。
#[cfg(target_os = "linux")]
// 038-CI：本探针要求 bwrap 真实执行（033 沙箱后端），macOS/Windows 不编译。
#[test]
fn d_fd_inheritance_sandbox_closed_normal_unchanged() {
    for sandbox in [true, false] {
        let work = TempDir::new("033mx-fd-work");
        let fd_file = work.path().join("fd-secret.txt");
        fs::write(&fd_file, "FAKE-FD-SECRET-033").unwrap();
        let spec = json!({"kind": "exec", "fd_path": fd_file.display().to_string(), "input": {
            "cmd": "echo \"PID=$$\"; echo \"FD_LIST=$(ls /proc/self/fd | tr '\\n' ',')\"; echo \"FD_CAT=$(cat /proc/self/fd/__FD__ 2>&1 | head -c 60)\"",
            "workdir": work.path().display().to_string(),
            "timeout_ms": 10000, "yield_time_ms": 10000
        }});
        let outcome = run_probe("fd", &spec, |cmd| {
            cmd.current_dir(work.path());
            if sandbox {
                cmd.env("TCHAT_SANDBOX", "1");
            }
        });
        let output = outcome["result"]["output"].as_str().unwrap_or_default();
        println!(
            "033 D-FD（{}）：{output}",
            if sandbox { "沙箱" } else { "普通对照" }
        );
        if sandbox {
            assert!(
                !output.contains("FAKE-FD-SECRET-033") && output.contains("No such file"),
                "沙箱分支：无 CLOEXEC fd 应于 execve 时关闭（B2 加固）：{output}"
            );
            assert!(
                output.lines().any(|l| l == "PID=1"),
                "链路证据：exec 经 bwrap --as-pid-1：{output}"
            );
        } else {
            assert!(
                output.contains("FAKE-FD-SECRET-033"),
                "普通对照：fd 继承行为不变：{output}"
            );
        }
    }
}

/// B2 关键机制探针（与产品实现同机制）：pre_exec 内以
/// `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` **标记**（非关闭）fd ≥3——
/// (a) execve 成功路径：fd 不进入载荷、stdio 0/1/2 保留；
/// (b) execve 失败路径（程序存在但不可执行）：spawn() 仍返回 Err——std 的
///     spawn 失败信道（Linux 为 SOCK_SEQPACKET socketpair，fd≥3、自带
///     CLOEXEC）不被破坏；若误用真 close，execve 失败会被误报为 spawn 成功
///     （子进程静默 exit 1）——本探针即该风险的回归防线。
// 038-CI：直接调用 libc::close_range（Linux 专属 syscall，apple libc 无绑定）
// ——macOS/Windows 不编译，排除原因见 docs/specs/038-github-actions-ci.md。
#[cfg(target_os = "linux")]
#[test]
fn b2_fix_feasibility_preexec_close_fds_keeps_stdio() {
    use std::os::unix::process::CommandExt;
    let io = TempDir::new("033mx-fdfix");
    let fake = io.path().join("fake-secret.txt");
    fs::write(&fake, "FAKE-FD-FIX-033").unwrap();
    use std::os::fd::AsRawFd;
    let f = fs::File::open(&fake).unwrap();
    let fd_no = f.as_raw_fd();
    unsafe { assert_eq!(libc::fcntl(fd_no, libc::F_SETFD, 0), 0, "清除 CLOEXEC 失败"); }
    std::mem::forget(f);

    // (a) 成功路径：stdio 先于 pre_exec 就位（dup2 顺序），标记后 fd 不进载荷。
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(format!(
            "echo SURVIVED; echo \"FD_LIST=$(ls /proc/self/fd | tr '\\n' ',')\"; echo \"FD_CAT=$(cat /proc/self/fd/{fd_no} 2>&1 | head -c 40)\""
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            let r = libc::close_range(
                3,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            );
            if r != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = cmd.output().expect("探针子进程启动失败");
    assert!(out.status.success(), "探针退出：{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    println!("033 B2 可行性（成功路径）：{stdout}");
    assert!(stdout.contains("SURVIVED"), "stdio 须先于 pre_exec 就位（管道仍可用）：{stdout}");
    assert!(
        !stdout.contains("FAKE-FD-FIX-033") && stdout.contains("No such file"),
        "被标记 fd 应于 execve 时关闭（不得读出正文）：{stdout}"
    );
    let list = stdout
        .lines()
        .find_map(|l| l.strip_prefix("FD_LIST="))
        .unwrap_or_default()
        .trim_end_matches(',');
    let fds: Vec<&str> = list.split(',').collect();
    for stdio in ["0", "1", "2"] {
        assert!(fds.contains(&stdio), "stdio {stdio} 须保留：{stdout}");
    }
    unsafe { libc::close(fd_no); }

    // (b) 失败路径：存在但不可执行的程序 → execve EACCES → spawn 必须返回 Err
    //（错误信道完好；真 close 方案在此会误报成功）。
    let non_exec = io.path().join("not-a-program");
    fs::write(&non_exec, "plain text\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&non_exec, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut cmd = Command::new(&non_exec);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            let r = libc::close_range(
                3,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            );
            if r != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.spawn() {
        Err(e) => {
            // Os repr 错误原样回传（非 panic、非挂起、非误报成功）
            assert!(
                e.raw_os_error().is_some(),
                "应回传 OS 错误码（execve 失败），实际：{e}"
            );
            println!("033 B2 可行性（失败路径 EACCES）：spawn Err = {e}（错误信道完好）");
        }
        Ok(mut child) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("execve 失败被误报为 spawn 成功（错误信道被破坏）");
        }
    }

    // (c) 程序不存在（ENOENT）→ 同一错误信道返回 Err，非误报成功。
    let mut cmd = Command::new("/nonexistent/prog-033b2");
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            let r = libc::close_range(
                3,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            );
            if r != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.spawn() {
        Err(e) => println!("033 B2 可行性（失败路径 ENOENT）：spawn Err = {e}"),
        Ok(mut child) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("程序不存在被误报为 spawn 成功");
        }
    }
}

// ---------- 矩阵 B2（2026-10-27 加固）：MCP 面的 fd 收口专项 ----------

/// B2 的 MCP 面专项：launcher（探针子进程）遗留无 CLOEXEC 假凭据 fd → 真实
/// McpManager spawn（tokio `pre_exec` 注入点，与 exec 面为不同代码位）→ bwrap
/// → 服务器包装脚本扫描自身 fd ≥3。沙箱分支不得读出正文；普通对照保持可读
///（零变化）。exec 面由 d_fd/b2_real 覆盖（管道＋真实 CLI 全链路）；PTY 与
/// exec 共用 build_tool_command 注入位；apply_patch 面共用同一助手，其子进程
///（--internal-apply）无 fd 观测面，由共用机制＋代码审查覆盖（记录注明）。
#[cfg(target_os = "linux")]
// 038-CI：本探针要求 bwrap 真实执行（033 沙箱后端），macOS/Windows 不编译。
#[test]
fn b2_mcp_face_fd_closed() {
    // 038 恢复真实检查（2026-10-31）：本地含 bwrap 0.11.1（runner 同版本）均通过。
    // GH runner 实测的沙箱内 MCP 服务器未应答差异（tools=0）根因待 runner 侧
    // 诊断（见 docs/logs/038-github-actions-ci/2026-10-31-runner-diagnosis-plan.md）；
    // 该用例不再提前 return——runner 上若复现即如实失败，为诊断提供信号。
    for sandbox in [true, false] {
        let work = TempDir::new("033b2m-work");
        let user = TempDir::new("033b2m-user");
        let recs = TempDir::new("033b2m-rec");
        let home = user.path().join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(work.path().join(".env"), "TCHAT_033_FAKE_ENV_SECRET=b2m-fake\n").unwrap();
        let fd_file = recs.path().join("fd-secret.txt");
        fs::write(&fd_file, "FAKE-FD-MCP-033").unwrap();

        let rec = recs.path().join("probe.txt");
        let rec2 = recs.path().join("fixture.jsonl");
        let fixture = common::mcp_fixtures::FIXTURE.replace('\\', "/");
        // 包装脚本：扫描 fd ≥3 内容（跳过 0/1/2＝stdio 管道，防阻塞）后转真实
        // 夹具——泄漏时假凭据正文会出现在 FDSCAN 行。
        let wrapper = format!(
            r#"{{ for f in /proc/self/fd/*; do case "$f" in */0|*/1|*/2) ;; *) echo "FDSCAN=$(cat "$f" 2>&1 | head -c 30)";; esac; done; }} >> '{rec}'; exec python3 '{fixture}' --scenario echo --record '{rec2}'"#,
            rec = rec.display(),
            fixture = fixture,
            rec2 = rec2.display(),
        );
        let mcp = json!({"mcp_servers": {"fx": {
            "enabled": true, "command": "sh", "args": ["-c", wrapper],
            "cwd": work.path().display().to_string(), "env_vars": []}}});
        fs::write(user.path().join("mcp.json"), mcp.to_string()).unwrap();

        let spec = json!({
            "kind": "mcp",
            "rec": rec.display().to_string(),
            "fd_path": fd_file.display().to_string(),
        });
        let outcome = run_probe("b2m", &spec, |cmd| {
            cmd.env("TCHAT_USER_DIR", user.path())
                .env("HOME", &home)
                .env("LC_ALL", "C")
                .env("TERM", "xterm-033mx")
                .current_dir(work.path());
            if sandbox {
                cmd.env("TCHAT_SANDBOX", "1");
            }
        });
        // 服务器应正常启动并完成往返（fd 收口不破坏 MCP 面）
        assert_eq!(outcome["started"], json!(true), "服务器应启动：{outcome}");
        assert_eq!(outcome["tools"], json!(1), "应发现工具：{outcome}");
        assert_eq!(outcome["call"]["failed"], json!(false), "调用往返成功：{outcome}");
        let rec_text = outcome["probe_rec"].as_str().unwrap_or_default();
        println!(
            "==== 033 B2 MCP 面（{}）====\n{rec_text}",
            if sandbox { "沙箱" } else { "普通对照" }
        );
        if sandbox {
            assert!(
                !rec_text.contains("FAKE-FD-MCP-033"),
                "MCP 面：沙箱分支不得读出 fd 正文（tokio 注入点回归）：{rec_text}"
            );
        } else {
            assert!(
                rec_text.contains("FAKE-FD-MCP-033"),
                "MCP 面：普通对照 fd 可读（零变化承诺）：{rec_text}"
            );
        }
    }
}

// ---------- 矩阵 E：取消/超时/正常完成的整批回收（管道＋PTY，重复时序） ----------

/// E：四组合（管道/PTY × 取消/超时）各重复 3 次＋正常完成两型＋纯短命令两型。
/// 回收证据＝双侧扫描（子进程 close 前后＋父进程在子进程退出后）皆无 marker
/// 进程；沙箱下正常完成的后台子/孙进程随 PID ns 回收（pid 1 退出→内核 SIGKILL）。
#[cfg(target_os = "linux")]
// 038-CI：本探针要求 bwrap 真实执行（033 沙箱后端），macOS/Windows 不编译。
#[test]
fn e_reaping_cancel_timeout_completion_pipe_and_pty() {
    // 038 恢复真实检查（2026-10-31）：本地含 bwrap 0.11.1（runner 同版本）均通过。
    // GH runner 实测的组杀后孙进程存活差异（close 后 3 进程超 15s poll）根因待
    // runner 侧诊断（见 docs/logs/038-github-actions-ci/
    // 2026-10-31-runner-diagnosis-plan.md）；该用例不再提前 return——runner 上
    // 若复现即如实失败，为诊断提供信号。
    let mk_cmd = |tag: &str, fg: &str| {
        // 两个后台子进程（其一再派生孙进程）＋前台长命令；marker 进各进程 cmdline。
        format!("sh -c 'sleep 299' {t} & sh -c 'exec sh -c \"sleep 299\" {t}' {t} & {fg}", t = tag)
    };
    for (tty, mode) in [(false, "cancel"), (true, "cancel"), (false, "timeout"), (true, "timeout")] {
        for rep in 0..3 {
            let work = TempDir::new("033mx-e-work");
            let tag = format!("033MXE-{}-{}-{}", mode, tty, std::process::id());
            let mut input = json!({
                "cmd": mk_cmd(&tag, "sleep 299"),
                "workdir": work.path().display().to_string(),
                "yield_time_ms": 20000,
                "tty": tty,
            });
            if mode == "timeout" {
                input["timeout_ms"] = json!(800);
            } else {
                input["timeout_ms"] = json!(30000);
            }
            let spec = json!({
                "kind": "exec",
                "input": input,
                "scan_marker": tag,
                "cancel_after_ms": if mode == "cancel" { json!(700) } else { Value::Null },
            });
            let outcome = run_probe("e", &spec, sandbox_env(work.path()));
            let result = &outcome["result"];
            let expect = if mode == "timeout" { "timeout" } else { "cancelled" };
            assert_eq!(
                result["status"], expect,
                "{mode}/tty={tty} 第 {rep} 次：{result}"
            );
            assert!(
                result["output"].as_str().is_some_and(|o| !o.contains("033MXE-")),
                "输出不应包含 marker"
            );
            if mode == "cancel" {
                // 016 既有契约：普通轮取消只停止当前交互，进程由可续接会话继续
                // 持有（result.session_id=s1、error「进程仍在运行」）——取消点
                // 不杀；整批回收由 close() 终止（after_close＋父侧扫描验证）。
                assert!(
                    outcome["result"]["session_id"].as_str().is_some_and(|s| !s.is_empty()),
                    "{mode}/tty={tty} 第 {rep} 次：取消后应保留会话句柄：{result}"
                );
                assert!(
                    !outcome["before_close"].as_array().unwrap().is_empty(),
                    "{mode}/tty={tty} 第 {rep} 次：取消点进程应仍在运行（016 契约）：{outcome}"
                );
            } else {
                // 016 既有契约：显式 timeout_ms 到期＝后台终止并回收（killpg 组杀，
                // 含后台子/孙进程）——close 前即应无遗留。
                assert!(
                    outcome["before_close"].as_array().is_some_and(Vec::is_empty),
                    "{mode}/tty={tty} 第 {rep} 次：超时应已组杀（close 前无遗留）：{outcome}"
                );
            }
            assert!(
                outcome["after_close"].as_array().is_some_and(Vec::is_empty),
                "{mode}/tty={tty} 第 {rep} 次：close 后应无遗留：{outcome}"
            );
            assert!(
                scan_marker(&tag).is_empty(),
                "{mode}/tty={tty} 第 {rep} 次：父侧扫描无遗留"
            );
        }
    }

    // 正常完成（沙箱）：前台立即退出，后台子/孙进程随 PID ns 回收（033 结构性
    // 改进；普通模式按 016 契约为可续接会话语义，不在本断言面）。
    for tty in [false, true] {
        let work = TempDir::new("033mx-e-done");
        let tag = format!("033MXE-done-{}-{}", tty, std::process::id());
        let spec = json!({
            "kind": "exec",
            "input": {"cmd": mk_cmd(&tag, "echo done-033"), "workdir": work.path().display().to_string(),
                      "timeout_ms": 20000, "yield_time_ms": 20000, "tty": tty},
            "scan_marker": tag,
        });
        let outcome = run_probe("e-done", &spec, sandbox_env(work.path()));
        assert_eq!(outcome["result"]["status"], "completed", "正常完成：{outcome}");
        assert!(
            outcome["result"]["output"].as_str().is_some_and(|o| o.contains("done-033")),
            "输出应含 done-033：{outcome}"
        );
        assert!(
            outcome["before_close"].as_array().is_some_and(Vec::is_empty)
                && outcome["after_close"].as_array().is_some_and(Vec::is_empty),
            "沙箱内正常完成后后台子/孙进程应随 PID ns 回收：{outcome}"
        );
        assert!(scan_marker(&tag).is_empty(), "父侧扫描无遗留");
    }

    // 纯短命令（管道/PTY 各一）：完成、输出正确、无任何遗留。
    for (tty, word) in [(false, "pipe-ok"), (true, "pty-ok")] {
        let work = TempDir::new("033mx-e-short");
        let spec = json!({
            "kind": "exec",
            "input": {"cmd": format!("echo {word}"), "workdir": work.path().display().to_string(),
                      "timeout_ms": 10000, "yield_time_ms": 10000, "tty": tty},
            "scan_marker": "",
        });
        let outcome = run_probe("e-short", &spec, sandbox_env(work.path()));
        assert_eq!(outcome["result"]["status"], "completed", "{word}：{outcome}");
        assert!(
            outcome["result"]["output"].as_str().is_some_and(|o| o.contains(word)),
            "{word} 输出：{outcome}"
        );
    }
}

// ---------- 矩阵 F：fail-closed（隔离子环境模拟 bwrap 各级不可用） ----------

/// 建造假 bwrap 目录：mode 0644＝不可执行文件；0755＋exit 1＝探测干跑失败；
/// 0755＋仅探测参数通过＝探测成功但真实启动失败。
// 038：假 bwrap 构造依赖 unix 权限位（chmod 语义）与 bwrap 探针——unix 门控。
#[cfg(unix)]
fn fake_bwrap_dir(kind: &str) -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new(&format!("033f-{kind}"));
    let path = dir.path().join("bwrap");
    let (content, mode) = match kind {
        "noexec" => ("#!/bin/sh\nexit 0\n".to_string(), 0o644),
        "dryrun-fail" => ("#!/bin/sh\nexit 1\n".to_string(), 0o755),
        "launch-fail" => (
            "#!/bin/sh\nif [ \"$*\" = \"--ro-bind / / --unshare-user --unshare-ipc --cap-drop ALL -- /bin/true\" ]; then exec /bin/true; fi\nexit 1\n".to_string(),
            0o755,
        ),
        _ => return dir, // "missing"：空目录
    };
    fs::write(&path, content).unwrap();
    fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    dir
}

/// F：四场景 × 三工具面——沙箱启用时必须显式失败，执行标记不得出现，绝不静默
/// 回退为无沙箱执行。全程不触碰 /usr/bin/bwrap（仅隔离子进程 PATH 替换）。
#[test]
fn f_fail_closed_without_fallback() {
    let scenarios: [(&str, &str); 4] = [
        ("missing", "bwrap 缺失"),
        ("noexec", "bwrap 不可执行"),
        ("dryrun-fail", "探测干跑失败"),
        ("launch-fail", "探测成功但启动失败"),
    ];
    for (kind, desc) in scenarios {
        let dir = fake_bwrap_dir(kind);
        let fake_path = dir.path().display().to_string();

        // exec 面：目标命令写 marker——失败时 marker 必须不存在。
        let work = TempDir::new("033mx-f-work");
        let marker_file = work.path().join("f-marker.txt");
        let spec = json!({"kind": "exec", "input": {
            "cmd": format!("echo ran > {}", marker_file.display()),
            "workdir": work.path().display().to_string(),
            "timeout_ms": 10000, "yield_time_ms": 10000,
        }});
        let outcome = run_probe("f-exec", &spec, |cmd| {
            cmd.current_dir(work.path())
                .env("TCHAT_SANDBOX", "1")
                .env("PATH", &fake_path);
        });
        let result = &outcome["result"];
        if kind == "launch-fail" {
            // 探测通过后 bwrap 启动即失败：载荷未运行（marker 不出现），结果为
            // 非零退出（bwrap 自身 exit 1）——非静默回退、非成功执行。
            let failed = result["status"] == "error"
                || result["exit_code"] != json!(0);
            assert!(failed, "{desc}：不得报告成功执行：{result}");
        } else {
            assert_eq!(result["status"], "error", "{desc}：必须显式报错：{result}");
            assert!(
                result["error"].as_str().unwrap_or("").contains("bwrap"),
                "{desc}：错误应指向 bwrap：{result}"
            );
            assert!(
                result["output"].as_str().unwrap_or("").is_empty(),
                "{desc}：不得有执行输出：{result}"
            );
        }
        assert!(
            !marker_file.exists(),
            "{desc}：执行标记不得出现（无静默回退）"
        );
        println!("033 F exec 面（{desc}）：{result}");

        // apply_patch 面：Add 被拒、文件不产生（launch-fail 场景同理由 marker 证明）。
        let pwned = work.path().join("pwned.txt");
        let outcome = run_probe(
            "f-ap",
            &json!({"kind": "apply_patch", "input": {"patch": format!(
                "*** Begin Patch\n*** Add File: {}\n+must-not-exist\n*** End Patch\n", pwned.display())}}),
            |cmd: &mut Command| {
                cmd.current_dir(work.path())
                    .env("TCHAT_SANDBOX", "1")
                    .env("PATH", &fake_path);
            },
        );
        assert_ne!(outcome["result"]["ok"], json!(true), "{desc}：apply_patch 必须被拒");
        assert!(!pwned.exists(), "{desc}：补丁文件不得产生");

        // MCP 面：服务器命令唯一动作＝写 marker——沙箱不可用时服务器不得启动。
        let user = TempDir::new("033mx-f-user");
        let recs = TempDir::new("033mx-f-rec");
        let srv_marker = recs.path().join("srv-started.txt");
        let mcp = json!({"mcp_servers": {"fx": {
            "enabled": true, "command": "sh",
            "args": ["-c", format!("echo started >> '{}'; exec sleep 30", srv_marker.display())],
            "cwd": work.path().display().to_string()}}});
        fs::write(user.path().join("mcp.json"), mcp.to_string()).unwrap();
        let outcome = run_probe(
            "f-mcp",
            &json!({"kind": "mcp", "rec": recs.path().join("unused.txt").display().to_string()}),
            |cmd: &mut Command| {
                cmd.current_dir(work.path())
                    .env("TCHAT_USER_DIR", user.path())
                    .env("TCHAT_SANDBOX", "1")
                    .env("PATH", &fake_path);
            },
        );
        assert!(
            outcome["started"] == json!(false) || outcome["tools"] == json!(0),
            "{desc}：MCP 不得发现工具：{outcome}"
        );
        assert!(!srv_marker.exists(), "{desc}：服务器不得启动（无静默回退）");
        println!("033 F mcp 面（{desc}）：started={} tools={}", outcome["started"], outcome["tools"]);
    }
}
