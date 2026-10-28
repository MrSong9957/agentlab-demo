//! 004 规格 apply_patch 集成测试：真实文件系统、隔离临时目录；
//! 核对文件字节、存在状态、实际变更清单与失败后的状态，不只检查返回值。
//! 覆盖规格第 5 节预测表，外加 JSON 边界、相对路径、UTF-8 限制、权限门，
//! 以及回归基线用例 A1–A6 / B1–B9 / D1–D6（编号↔测试名映射见各节注释）。
//! A/B/D 的断言依据为"当前实际行为"（Q1–Q4 契约未定，见
//! logs/2026-10-08-004-apply-patch.md）：Add 拒绝已有链接；Update 跟随链接；
//! Delete 删除链接本体；清单路径不 canonicalize；匹配按精确→trim_end→trim
//! 全文件扫描顺序、同级取最早、EOF 锚定末位；同文件各块顺序重读、清单不去重。

mod common;

use std::fs;
use std::os::unix::fs::{symlink, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::json;
use terminal_chat::apply_patch::{apply_patch, Action, ApplyPatchResult, ChangedFile};

/// 串行化切换进程当前目录的唯一用例，避免同进程并行竞态。
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// 每个用例独立临时目录，用例名保证并行唯一，结束清理。
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tchat-ap-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 包裹 Begin/End 标记生成完整补丁。
fn patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

fn call(body: &str) -> ApplyPatchResult {
    apply_patch(&json!({ "patch": patch(body) }))
}

fn read(p: &Path) -> Vec<u8> {
    fs::read(p).unwrap()
}

fn set_mode(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

// ---------- EFBIG 故障注入：子进程隔离 ----------

/// 子进程模式入口：由 `TCHAT_EFBIG_INPUT` 触发（父进程经
/// [`apply_patch_under_fsize_limit`] 重入本测试二进制）。读取输入（补丁、限额），
/// 收紧本进程 RLIMIT_FSIZE 并忽略 SIGXFSZ 后，经真实 `apply_patch` 入口执行补丁，
/// 把结果 JSON 写到 `TCHAT_EFBIG_OUTCOME` 后退出。限额与信号只存在于本子进程，
/// 与同二进制其他并行用例**结构隔离**——安全不依赖其他用例的写入量（坑003）。
fn efbig_child_mode() {
    let Some(input) = std::env::var_os("TCHAT_EFBIG_INPUT") else {
        return; // 父进程模式：无事发生
    };
    let outcome = std::env::var_os("TCHAT_EFBIG_OUTCOME").expect("缺少 TCHAT_EFBIG_OUTCOME");
    let spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&input).unwrap()).unwrap();
    let patch_text = spec["patch"].as_str().unwrap().to_string();
    let soft = spec["limit"].as_u64().unwrap();
    unsafe {
        let mut old: libc::rlimit = std::mem::zeroed();
        assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &mut old), 0, "读取 RLIMIT_FSIZE 失败");
        let lim = libc::rlimit { rlim_cur: soft, rlim_max: old.rlim_max };
        assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &lim), 0, "设置 RLIMIT_FSIZE 失败");
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
    let r = apply_patch(&json!({ "patch": patch_text }));
    std::fs::write(&outcome, serde_json::to_string(&r).unwrap()).unwrap();
    std::process::exit(0);
}

/// 父进程侧：重启本测试二进制（`--exact` 定位到 `entry` 用例，该用例开头调用
/// [`efbig_child_mode`]），让补丁在限额子进程内执行；返回结果 JSON 供字段断言，
/// 文件副作用留在共享临时目录中由调用方重读核对。
fn apply_patch_under_fsize_limit(entry: &str, patch_text: &str, soft: u64) -> serde_json::Value {
    assert!(
        std::env::var_os("TCHAT_EFBIG_INPUT").is_none(),
        "EFBIG 子进程模式到达了未设防的用例，防止进程链递归"
    );
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let io = temp_dir(&format!("efbig-io-{seq}"));
    let input = io.join("in.json");
    let outcome = io.join("out.json");
    std::fs::write(&input, json!({ "patch": patch_text, "limit": soft }).to_string()).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        // 子进程 CWD 固定为稳定目录：防继承 chdir 类用例的瞬时 CWD（目录被删后
        // 解析会话目录 ENOENT 的偶发竞态；行为断言不变）
        .current_dir(std::env::temp_dir())
        .arg(entry)
        .arg("--exact")
        .env("TCHAT_EFBIG_INPUT", &input)
        .env("TCHAT_EFBIG_OUTCOME", &outcome)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("EFBIG 子进程启动失败");
    if !status.status.success() {
        panic!(
            "EFBIG 子进程异常退出（entry={entry}）：{}，stderr：{}",
            status.status,
            String::from_utf8_lossy(&status.stderr)
        );
    }
    let outcome_text = std::fs::read_to_string(&outcome).unwrap_or_else(|e| {
        panic!(
            "EFBIG 子进程成功退出但未写结果（entry={entry} 是否与用例名漂移，\
             或该用例开头缺少 efbig_child_mode 调用？）：{e}"
        )
    });
    let v: serde_json::Value = serde_json::from_str(&outcome_text).unwrap();
    let _ = std::fs::remove_dir_all(&io);
    v
}

/// 场景：格式错误、空补丁、不支持的操作 → ok=false；清单为空；state_known=true；文件字节不变。
#[test]
fn invalid_patches_error_without_touching_files() {
    let dir = temp_dir("invalid");
    let a = dir.join("a.txt");
    fs::write(&a, b"keep-a\n").unwrap();
    let pa = a.display().to_string();

    let cases: Vec<(String, &str)> = vec![
        ("".into(), "为空"),
        ("not a patch at all".into(), "Begin Patch"),
        // 空补丁：只有 marker，无文件操作
        ("*** Begin Patch\n*** End Patch".into(), "空补丁"),
        // 不是有效的块头
        (patch("what is this line"), "块头"),
        // Update 块为空（无 @@ / 变更行）
        (patch(&format!("*** Update File: {pa}")), "为空"),
        // @@ 块没有任何变更行
        (patch(&format!("*** Update File: {pa}\n@@")), "变更行"),
        // 变更行后出现非 +/-/空格 开头的行
        (patch(&format!("*** Update File: {pa}\n@@\n-a\nbad line")), "@@"),
        // ---- 029：Move 非法语法变体（含合法前序块时也不执行任何文件操作）----
        // 重复 Move
        (
            patch(&format!(
                "*** Update File: {pa}\n*** Move to: {}\n*** Move to: {}\n@@\n-a\n+b",
                dir.join("b1.txt").display(),
                dir.join("b2.txt").display()
            )),
            "Move",
        ),
        // Move 位于修改行之后（错位）
        (
            patch(&format!(
                "*** Update File: {pa}\n@@\n-a\n*** Move to: {}",
                dir.join("b.txt").display()
            )),
            "@@",
        ),
        // 空 Move 目标
        (
            patch(&format!("*** Update File: {pa}\n*** Move to: \n@@\n-a\n+b")),
            "Move",
        ),
        // Add 块缺少 + 内容行（后续 Delete 不应执行）
        (
            patch(&format!(
                "*** Add File: {}\n*** Delete File: {pa}",
                dir.join("never.txt").display()
            )),
            "+",
        ),
        // End Patch 之后出现多余内容
        (format!("{}\nextra line", patch(&format!("*** Delete File: {pa}"))), "多余内容"),
    ];
    for (text, hint) in &cases {
        let r = apply_patch(&json!({ "patch": text }));
        assert!(!r.ok, "应失败：{text}");
        assert!(r.changed_files.is_empty(), "失败补丁不应产生变更记录：{r:?}");
        assert!(r.state_known, "未发生写入，清单为空且可信：{r:?}");
        let err = r.error.clone().unwrap_or_default();
        assert!(!err.is_empty(), "应给出错误说明：{text}");
        assert!(err.contains(hint), "错误应包含「{hint}」，实际：{err}");
    }
    // 全部失败补丁均不触碰目标文件，也不产生新文件
    assert_eq!(read(&a), b"keep-a\n");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "目录内不应出现新文件");
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：JSON 工具边界参数校验 → ok=false 且给出原因，不触发文件操作。
#[test]
fn invalid_json_args_error_without_side_effects() {
    let dir = temp_dir("args");
    let probe = dir.join("probe.txt");
    let add_probe = patch(&format!("*** Add File: {}\n+x", probe.display()));

    // 完整参数矩阵由单元 args_reject_bad_values 持有；集成层保留两个代表例，
    // 其中未知字段例携带本可执行的有效补丁：证明拒绝发生在任何执行之前
    let cases: Vec<(serde_json::Value, &str)> = vec![
        (json!(5), "对象"),
        (json!({ "patch": &add_probe, "evil": 1 }), "未知参数"),
    ];
    for (input, hint) in &cases {
        let r = apply_patch(input);
        assert!(!r.ok, "{input}");
        assert!(r.changed_files.is_empty());
        assert!(r.state_known);
        let err = r.error.clone().unwrap_or_default();
        assert!(err.contains(hint), "错误应包含「{hint}」，实际：{err}");
    }
    assert!(!probe.exists(), "非法参数不得触发文件操作");
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：新增 → 修改 → 删除；内容与存在状态正确，清单动作与绝对路径准确、按执行顺序。
#[test]
fn add_update_delete_full_cycle() {
    let dir = temp_dir("cycle");
    let cfg = dir.join("config.txt");
    fs::write(&cfg, "mode=dev\n").unwrap();
    let old = dir.join("old.txt");
    fs::write(&old, "obsolete\n").unwrap();
    let newf = dir.join("new.txt");
    let untouched = dir.join("unrelated.txt");
    fs::write(&untouched, "keep\n").unwrap();
    assert_eq!(read(&cfg).len(), 9, "mode=dev\\n 为 9 字节");

    let r = call(&format!(
        "*** Update File: {cfg}\n@@\n-mode=dev\n+mode=test\n\
         *** Add File: {newf}\n+hello\n\
         *** Delete File: {old}",
        cfg = cfg.display(),
        newf = newf.display(),
        old = old.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(r.error, None);
    assert!(r.state_known);
    assert_eq!(
        r.changed_files,
        vec![
            terminal_chat::apply_patch::ChangedFile {
                path: cfg.display().to_string(),
                action: Action::Update
            },
            terminal_chat::apply_patch::ChangedFile {
                path: newf.display().to_string(),
                action: Action::Add
            },
            terminal_chat::apply_patch::ChangedFile {
                path: old.display().to_string(),
                action: Action::Delete
            },
        ]
    );
    assert_eq!(read(&cfg), b"mode=test\n");
    assert_eq!(read(&cfg).len(), 10, "mode=test\\n 为 10 字节");
    assert_eq!(read(&newf), b"hello\n");
    assert_eq!(read(&newf).len(), 6, "新文件恰为 6 字节");
    assert!(!old.exists());
    assert_eq!(read(&untouched), b"keep\n", "无关文件不得改动");
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：新增目标已存在 / 修改或删除目标不存在 / 更新上下文不存在
/// → 不改变当前目标，返回包含路径的具体错误，state_known=true。
#[test]
fn conflicting_targets_rejected_without_changes() {
    let dir = temp_dir("conflict");
    let exists = dir.join("exists.txt");
    fs::write(&exists, b"original\n").unwrap();

    // 新增目标已存在（不得覆盖）
    let r = call(&format!("*** Add File: {}\n+new content", exists.display()));
    assert!(!r.ok);
    assert!(r.changed_files.is_empty());
    assert!(r.state_known);
    let err = r.error.clone().unwrap();
    assert!(err.contains("已存在"), "{err}");
    assert!(err.contains("exists.txt"), "{err}");
    assert_eq!(read(&exists), b"original\n");

    // 修改目标不存在
    let missing = dir.join("missing.txt");
    let r = call(&format!("*** Update File: {}\n@@\n-a\n+b", missing.display()));
    assert!(!r.ok);
    assert!(r.state_known);
    let err = r.error.clone().unwrap();
    assert!(err.contains("不存在"), "{err}");
    assert!(err.contains("missing.txt"), "{err}");
    assert!(!missing.exists());

    // 删除目标不存在
    let r = call(&format!("*** Delete File: {}", missing.display()));
    assert!(!r.ok);
    assert!(r.state_known);
    assert!(r.error.clone().unwrap().contains("不存在"));

    // 更新上下文不存在（文件存在、内容不匹配）
    let victim = dir.join("victim.txt");
    fs::write(&victim, b"alpha\nbeta\ngamma\n").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-delta\n+replaced", victim.display()));
    assert!(!r.ok);
    assert!(r.state_known);
    let err = r.error.clone().unwrap();
    assert!(err.contains("victim.txt"), "{err}");
    assert!(err.contains("delta"), "{err}");
    assert_eq!(read(&victim), b"alpha\nbeta\ngamma\n");

    // @@ 定位行不存在
    let r = call(&format!(
        "*** Update File: {}\n@@ class NotFound\n-x\n+y",
        victim.display()
    ));
    assert!(!r.ok);
    assert!(r.state_known);
    assert!(r.error.clone().unwrap().contains("定位上下文"));
    assert_eq!(read(&victim), b"alpha\nbeta\ngamma\n");
    let _ = fs::remove_dir_all(&dir);
}

/// 跨文件校验失败：前项真实改动保留，失败目标字节不变，后续新增不执行，清单只含成功前缀。
#[test]
fn second_file_context_mismatch_preserves_first_and_skips_rest() {
    let dir = common::TempDir::new("patch-context-prefix");
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    let third = dir.path().join("never.txt");
    fs::write(&first, b"before-first\n").unwrap();
    fs::write(&second, b"keep-second\n").unwrap();

    let r = call(&format!(
        "*** Update File: {first}\n@@\n-before-first\n+after-first\n\
         *** Update File: {second}\n@@\n-missing-context\n+changed-second\n\
         *** Add File: {third}\n+must-not-run",
        first = first.display(),
        second = second.display(),
        third = third.display(),
    ));

    assert!(!r.ok, "{r:?}");
    assert!(r.state_known, "上下文校验失败，已生效清单仍完整可信：{r:?}");
    assert_eq!(
        r.changed_files,
        vec![ChangedFile { path: first.display().to_string(), action: Action::Update }],
    );
    let error = r.error.unwrap();
    assert!(error.contains(second.to_str().unwrap()), "{error}");
    assert!(error.contains("missing-context"), "{error}");
    assert_eq!(read(&first), b"after-first\n", "第一项不回滚");
    assert_eq!(read(&second), b"keep-second\n", "失败目标未写入");
    assert!(!third.exists(), "失败后的文件操作不得执行");
}

/// 场景：两文件补丁，第一项成功、第二项写入故障
/// → ok=false；清单只含第一项；state_known=false；重读两文件核对实际状态。
/// 覆盖两种确定性写入故障：open 阶段拒绝（EACCES，未写入任何字节）与
/// 写入开始后的半写截断（RLIMIT_FSIZE→EFBIG，坑003；限额在隔离子进程内生效）。
#[test]
fn second_file_write_failure_reports_partial_state() {
    efbig_child_mode(); // 子进程模式：限额内执行注入场景后已在子进程内退出

    // —— 阶段一：只读文件，open 即失败，目标字节不变 ——
    let dir = temp_dir("partial");
    let good = dir.join("good.txt");
    fs::write(&good, "one\ntwo\n").unwrap();
    let locked = dir.join("locked.txt");
    fs::write(&locked, "keep\n").unwrap();
    set_mode(&locked, 0o444); // 只读文件：可读不可写，写入在 open 阶段被拒

    let r = call(&format!(
        "*** Update File: {good}\n@@\n-one\n+uno\n*** Update File: {locked}\n@@\n-keep\n+held",
        good = good.display(),
        locked = locked.display()
    ));
    assert!(!r.ok, "{r:?}");
    let err = r.error.clone().unwrap();
    assert!(err.contains("locked.txt"), "{err}");
    assert!(!r.state_known, "写入失败的目标可能被部分修改，清单不可信：{r:?}");
    assert_eq!(r.changed_files.len(), 1, "只应记录第一项：{r:?}");
    assert_eq!(r.changed_files[0].path, good.display().to_string());
    assert_eq!(r.changed_files[0].action, Action::Update);
    assert_eq!(read(&good), b"uno\ntwo\n", "第一项修改已生效");
    assert_eq!(read(&locked), b"keep\n", "open 阶段被拒，目标字节不变");
    let _ = fs::remove_dir_all(&dir);

    // —— 阶段二：RLIMIT_FSIZE 使第二项写入开始后失败——目标被真实截断为部分新内容 ——
    let dir = temp_dir("partial-efbig");
    let good = dir.join("good.txt");
    fs::write(&good, "one\ntwo\n").unwrap();
    let big = dir.join("big.txt");
    fs::write(&big, "keep\n").unwrap();
    // 新内容 82 行 ×(101 字符+LF)=8364 字节，远超 4096 软限制；第一项新内容仅 8 字节
    let new_lines: Vec<String> =
        (0..82).map(|i| format!("line-{:03}-{}", i, "x".repeat(92))).collect();
    let expected_full: String = format!("{}\n", new_lines.join("\n"));
    let added_body: String =
        new_lines.iter().map(|l| format!("+{l}\n")).collect::<String>();
    let patch_text = patch(&format!(
        "*** Update File: {good}\n@@\n-one\n+uno\n*** Update File: {big}\n@@\n-keep\n{added_body}",
        good = good.display(),
        big = big.display()
    ));
    let r = apply_patch_under_fsize_limit(
        "second_file_write_failure_reports_partial_state",
        &patch_text,
        4096,
    );
    assert_eq!(r["ok"], json!(false), "{r}");
    let err = r["error"].as_str().unwrap();
    assert!(err.contains("big.txt"), "错误应含可确定目标路径：{err}");
    assert!(err.contains("File too large"), "错误应含原因：{err}");
    assert_eq!(r["state_known"], json!(false), "半写截断后清单不可信：{r}");
    assert_eq!(
        r["changed_files"],
        json!([{ "path": good.display().to_string(), "action": "update" }]),
        "失败目标不得计入清单：{r}"
    );

    // 重读核对实际状态（state_known=false 时的要求；文件在共享临时目录，父子进程可见）
    assert_eq!(read(&good), b"uno\ntwo\n", "第一项修改已生效");
    let after = read(&big);
    assert!(
        after.starts_with(b"line-000-"),
        "第二项目标确实已被写入改变，实际开头：{:?}",
        String::from_utf8_lossy(after.get(..24).unwrap_or(&after))
    );
    assert_eq!(after.len(), 4096, "写入开始后失败，只落盘软限制内的前 4096 字节");
    assert_eq!(
        after.as_slice(),
        &expected_full.as_bytes()[..4096],
        "半写内容为新内容的确定性前缀（O_TRUNC 已发生，原内容 gone）"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：LF、CRLF、混合行尾、无末尾换行
/// → 未改行保持原换行；新行沿用原文件首个换行风格；更新补末尾换行；新增文件用 LF。
#[test]
fn line_endings_follow_spec_rules() {
    let dir = temp_dir("eol");

    // LF 文件：替换行使用 LF
    let lf = dir.join("lf.txt");
    fs::write(&lf, "a\nb\nc\n").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-b\n+B", lf.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&lf), b"a\nB\nc\n");

    // CRLF 文件：未改行保持 CRLF，插入行沿用 CRLF（原文件首个换行风格）
    let crlf = dir.join("crlf.txt");
    fs::write(&crlf, "a\r\nb\r\nc\r\n").unwrap();
    let r = call(&format!(
        "*** Update File: {}\n@@\n-b\n+B\n+inserted",
        crlf.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&crlf), b"a\r\nB\r\ninserted\r\nc\r\n");

    // 混合行尾：约定风格取首个换行（LF）；未改的 CRLF 行保持 CRLF
    let mixed = dir.join("mixed.txt");
    fs::write(&mixed, "first\nsecond\r\nthird\r\n").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-second\n+2nd", mixed.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&mixed), b"first\n2nd\nthird\r\n");

    // 上下文行保留原行尾：替换块中的上下文行不因替换而统一行尾
    let ctxf = dir.join("ctx.txt");
    fs::write(&ctxf, "top\r\nspacer\nbottom\n").unwrap(); // 首个换行 CRLF → 约定 CRLF
    let r = call(&format!(
        "*** Update File: {}\n@@\n top\n-spacer\n+repl\n bottom",
        ctxf.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&ctxf), b"top\r\nrepl\r\nbottom\n", "上下文行 top/bottom 保留原行尾");

    // 无末尾换行：更新后补齐末尾换行
    let noend = dir.join("noend.txt");
    fs::write(&noend, "x\ny").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-y\n+z", noend.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&noend), b"x\nz\n");

    // 新增文件一律 LF
    let added = dir.join("added.txt");
    let r = call(&format!("*** Add File: {}\n+hello", added.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&added), b"hello\n");
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：相对路径基于会话当前目录解析；清单路径为实际目标绝对路径。
#[test]
fn relative_paths_resolve_against_session_cwd() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir("relcwd");
    let subdir = dir.join("work");
    fs::create_dir(&subdir).unwrap();

    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(&subdir).unwrap();
    let r = call("*** Add File: rel.txt\n+content");
    std::env::set_current_dir(&prev).unwrap();

    assert!(r.ok, "{r:?}");
    let expect = subdir.join("rel.txt");
    assert_eq!(read(&expect), b"content\n");
    assert_eq!(r.changed_files[0].path, expect.display().to_string());
    assert_eq!(r.changed_files[0].action, Action::Add);
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：仅支持 UTF-8 文本——非法 UTF-8 修改目标拒绝且字节不变；补丁内容含中文/emoji 正常落盘。
#[test]
fn non_utf8_target_rejected_and_unicode_roundtrip() {
    let dir = temp_dir("utf8");
    let bin = dir.join("bin.dat");
    fs::write(&bin, [0x41, 0xFF, 0xFE, 0x42]).unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-a\n+b", bin.display()));
    assert!(!r.ok);
    assert!(r.state_known);
    assert!(r.error.clone().unwrap().contains("UTF-8"));
    assert_eq!(read(&bin), [0x41, 0xFF, 0xFE, 0x42]);

    let uni = dir.join("uni.txt");
    let r = call(&format!("*** Add File: {}\n+你好 🎉\n+emoji ✓", uni.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&uni), "你好 🎉\nemoji ✓\n".as_bytes());
    let _ = fs::remove_dir_all(&dir);
}

/// 场景：同一文件多 @@ 块定位修改 + 纯插入块（old 为空）追加到文件末尾。
#[test]
fn multi_chunk_update_and_context_seek() {
    let dir = temp_dir("multichunk");
    let f = dir.join("code.txt");
    fs::write(&f, "header\nfn a()\n    old_a\nfn b()\n    old_b\nfooter\n").unwrap();
    let r = call(&format!(
        "*** Update File: {f}\n@@ fn a()\n-    old_a\n+    new_a\n@@ fn b()\n-    old_b\n+    new_b\n@@\n+appended",
        f = f.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(
        read(&f),
        "header\nfn a()\n    new_a\nfn b()\n    new_b\nfooter\nappended\n".as_bytes()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// 权限门：只读目录 / 只读文件目标必须失败，不得绕过执行环境的既有权限。
#[test]
fn readonly_permissions_are_enforced_not_bypassed() {
    let dir = temp_dir("security");
    let ro_dir = dir.join("ro");
    fs::create_dir(&ro_dir).unwrap();

    // 新增：目标位于只读目录 → 拒绝
    set_mode(&ro_dir, 0o555);
    let target = ro_dir.join("new.txt");
    let r = call(&format!("*** Add File: {}\n+x", target.display()));
    assert!(!r.ok, "{r:?}");
    assert!(!r.state_known, "失败的写入不能假设无副作用");
    assert!(!target.exists());
    assert!(r.error.clone().unwrap().contains("new.txt"));

    // 删除：文件位于只读目录 → 拒绝，文件仍在
    set_mode(&ro_dir, 0o755);
    let victim = ro_dir.join("victim.txt");
    fs::write(&victim, "data\n").unwrap();
    set_mode(&ro_dir, 0o555);
    let r = call(&format!("*** Delete File: {}", victim.display()));
    assert!(!r.ok, "{r:?}");
    assert!(!r.state_known);
    assert!(victim.exists(), "删除不得成功");
    assert!(r.error.clone().unwrap().contains("victim.txt"));

    // 修改：只读文件（0444）可读不可写 → 拒绝，字节不变
    set_mode(&victim, 0o444);
    let r = call(&format!("*** Update File: {}\n@@\n-data\n+changed", victim.display()));
    assert!(!r.ok, "{r:?}");
    assert!(!r.state_known);
    assert!(r.error.clone().unwrap().contains("victim.txt"));
    assert_eq!(read(&victim), b"data\n");

    set_mode(&ro_dir, 0o755); // 恢复以便清理
    let _ = fs::remove_dir_all(&dir);
}

// ---------- A：符号链接语义（回归基线，编号↔用例映射：A1–A6）----------
// 断言依据为当前实际行为（Q1–Q4 待决契约，见文件头注释）：
// Add 拒绝已有链接（含悬空）；Update 跟随链接写真实目标；Delete 只删链接本体；
// changed_files 报告补丁解析路径，不 canonicalize。

/// A1：Update 跟随符号链接写穿到真实目标；链接本体保持；清单报告链接路径。
#[test]
fn a1_update_follows_symlink_writes_real_target() {
    let dir = temp_dir("a1");
    let real = dir.join("real.txt");
    fs::write(&real, "mode=dev\n").unwrap();
    let link = dir.join("link.txt");
    symlink(&real, &link).unwrap();

    let r = call(&format!(
        "*** Update File: {}\n@@\n-mode=dev\n+mode=test",
        link.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&real), b"mode=test\n", "真实目标被写穿修改（10 字节）");
    assert_eq!(read(&link).len(), 10, "经链接读取与目标一致");
    assert!(
        std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
        "链接本体必须保持为符号链接"
    );
    assert_eq!(
        r.changed_files,
        vec![ChangedFile { path: link.display().to_string(), action: Action::Update }],
        "清单报告补丁路径（不 canonicalize，待决 Q1）"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A2：Delete 只删除链接本体，真实目标保留。
#[test]
fn a2_delete_removes_link_only_target_kept() {
    let dir = temp_dir("a2");
    let real = dir.join("real2.txt");
    fs::write(&real, "data\n").unwrap();
    let link = dir.join("link2.txt");
    symlink(&real, &link).unwrap();

    let r = call(&format!("*** Delete File: {}", link.display()));
    assert!(r.ok, "{r:?}");
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "链接本体应被删除"
    );
    assert_eq!(read(&real), b"data\n", "真实目标必须保留且字节不变");
    assert_eq!(
        r.changed_files,
        vec![ChangedFile { path: link.display().to_string(), action: Action::Delete }]
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A3：Add 目标为已有符号链接 → 拒绝，不写穿目标、不改链接指向。
#[test]
fn a3_add_existing_symlink_rejected() {
    let dir = temp_dir("a3");
    let real = dir.join("real3.txt");
    fs::write(&real, "orig\n").unwrap();
    let link = dir.join("link3.txt");
    symlink(&real, &link).unwrap();

    let r = call(&format!("*** Add File: {}\n+new", link.display()));
    assert!(!r.ok, "{r:?}");
    assert!(r.changed_files.is_empty());
    assert!(r.state_known);
    let err = r.error.clone().unwrap();
    assert!(err.contains("已存在"), "{err}");
    assert!(err.contains("link3.txt"), "{err}");
    assert_eq!(read(&real), b"orig\n", "目标不得被写穿");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        real,
        "链接指向必须保持不变"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A4：Add 目标为悬空链接 → 按已存在拒绝；目标仍不存在、链接仍悬空。
#[test]
fn a4_add_dangling_symlink_rejected_keeps_dangling() {
    let dir = temp_dir("a4");
    let missing = dir.join("no-such-file.txt");
    let dang = dir.join("dang.txt");
    symlink(&missing, &dang).unwrap();

    let r = call(&format!("*** Add File: {}\n+x", dang.display()));
    assert!(!r.ok, "{r:?}");
    assert!(r.state_known);
    assert!(r.error.clone().unwrap().contains("已存在"));
    assert!(!missing.exists(), "不得借 Add 创建悬空目标");
    assert!(
        std::fs::symlink_metadata(&dang).is_ok(),
        "悬空链接必须原样保留（不得被“修复”）"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A5：Update 悬空链接 → 按目标不存在拒绝；不创建缺失目标。
#[test]
fn a5_update_dangling_symlink_target_missing() {
    let dir = temp_dir("a5");
    let missing = dir.join("missing.txt");
    let dang = dir.join("dang2.txt");
    symlink(&missing, &dang).unwrap();

    let r = call(&format!("*** Update File: {}\n@@\n-a\n+b", dang.display()));
    assert!(!r.ok, "{r:?}");
    assert!(r.state_known);
    assert!(r.changed_files.is_empty());
    let err = r.error.clone().unwrap();
    assert!(err.contains("不存在"), "{err}");
    assert!(err.contains("dang2.txt"), "{err}");
    assert!(!missing.exists(), "不得创建缺失目标");
    let _ = fs::remove_dir_all(&dir);
}

/// A6：Delete 悬空链接 → 成功清理链接本体（与目标存在性无关）。
#[test]
fn a6_delete_dangling_symlink_cleans_link() {
    let dir = temp_dir("a6");
    let never = dir.join("never.txt");
    let dang = dir.join("dang3.txt");
    symlink(&never, &dang).unwrap();

    let r = call(&format!("*** Delete File: {}", dang.display()));
    assert!(r.ok, "{r:?}");
    assert!(std::fs::symlink_metadata(&dang).is_err(), "悬空链接本体应被清理");
    assert!(!never.exists());
    assert_eq!(
        r.changed_files,
        vec![ChangedFile { path: dang.display().to_string(), action: Action::Delete }]
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------- B：匹配位置（回归基线，编号↔用例映射：B1–B9）----------
// 断言依据为当前实际行为：三级容差（精确→trim_end→trim）各自全文件扫描、
// 精确优先于更早的容差命中、同级取最早下标、EOF 锚定只试末位、哨兵裁剪重试。

/// B1：精确匹配优先于位置更靠前的容差等价候选。
#[test]
fn b1_exact_match_beats_earlier_trim_equivalent() {
    let dir = temp_dir("b1");
    let f = dir.join("f1.txt");
    fs::write(&f, "b \nc\nb\n").unwrap(); // 行1 带尾空格，行3 为精确 b
    let r = call(&format!("*** Update File: {}\n@@\n-b\n+HIT", f.display()));
    assert!(r.ok, "{r:?}");
    // 精确轮全文件扫描命中行3；若实现为“逐位置就近容差”会错误地改行1
    assert_eq!(read(&f), b"b \nc\nHIT\n");
    let _ = fs::remove_dir_all(&dir);
}

/// B2：同内容重复行命中最早出现。
#[test]
fn b2_repeated_line_hits_first_occurrence() {
    let dir = temp_dir("b2");
    let f = dir.join("f2.txt");
    fs::write(&f, "m\nx\nm\n").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-m\n+FIRST", f.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&f), b"FIRST\nx\nm\n");
    let _ = fs::remove_dir_all(&dir);
}

/// B3：@@ 定位行把搜索起点推进，消歧重复 old 行。
#[test]
fn b3_change_context_disambiguates_duplicates() {
    let dir = temp_dir("b3");
    let f = dir.join("f3.txt");
    fs::write(&f, "sec a\nold\nsec b\nold\n").unwrap();
    let r = call(&format!(
        "*** Update File: {}\n@@ sec b\n-old\n+new",
        f.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&f), b"sec a\nold\nsec b\nnew\n", "定位行之后的 old 被替换，行2 保留");
    let _ = fs::remove_dir_all(&dir);
}

/// B4：无精确命中时 trim_end 级容差取最早候选；替换为整行（原空白不保留）。
#[test]
fn b4_trim_end_tolerance_whole_line_replacement() {
    let dir = temp_dir("b4");
    let f = dir.join("f4.txt");
    fs::write(&f, "m \nx\n m\n").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-m\n+NEW", f.display()));
    assert!(r.ok, "{r:?}");
    // 精确轮无 "m"：行1 是 "m "、行3 是 " m"；trim_end 轮命中行1（最早）
    assert_eq!(read(&f), b"NEW\nx\n m\n", "行1 整行替换尾空格消失；行3 首空格保留");
    let _ = fs::remove_dir_all(&dir);
}

/// B5：仅第三级 trim（两端）容差能命中的行。
#[test]
fn b5_trim_both_sides_tolerance_only_match() {
    let dir = temp_dir("b5");
    let f = dir.join("f5.txt");
    fs::write(&f, "\tm\t\nend\n").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-m\n+CORE", f.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&f), b"CORE\nend\n");
    let _ = fs::remove_dir_all(&dir);
}

/// B6：End of File 锚定只试末位；同一补丁去掉锚定则从头匹配。
#[test]
fn b6_end_of_file_anchor_pins_to_last_line() {
    let dir = temp_dir("b6");
    let ga = dir.join("g1.txt");
    let gb = dir.join("g2.txt");
    fs::write(&ga, "x\nkeep\nx\n").unwrap();
    fs::write(&gb, "x\nkeep\nx\n").unwrap();

    let ra = call(&format!(
        "*** Update File: {}\n@@\n-x\n+LAST\n*** End of File",
        ga.display()
    ));
    assert!(ra.ok, "{ra:?}");
    assert_eq!(read(&ga), b"x\nkeep\nLAST\n", "EOF 锚定替换最后一次出现");

    let rb = call(&format!("*** Update File: {}\n@@\n-x\n+LAST", gb.display()));
    assert!(rb.ok, "{rb:?}");
    assert_eq!(read(&gb), b"LAST\nkeep\nx\n", "无锚定从头匹配首次出现");
    let _ = fs::remove_dir_all(&dir);
}

/// B7：EOF 锚定与实际位置不符 → 整体失败，字节不变。
#[test]
fn b7_end_of_file_anchor_mismatch_fails() {
    let dir = temp_dir("b7");
    let f = dir.join("f7.txt");
    fs::write(&f, "x\nend\n").unwrap();
    let r = call(&format!(
        "*** Update File: {}\n@@\n-x\n+NEW\n*** End of File",
        f.display()
    ));
    assert!(!r.ok, "{r:?}");
    assert!(r.state_known);
    assert!(r.changed_files.is_empty());
    let err = r.error.clone().unwrap();
    assert!(err.contains("f7.txt"), "{err}");
    assert!(err.contains("找不到要修改的行"), "{err}");
    assert_eq!(read(&f), b"x\nend\n", "锚定失败不得改动文件");
    let _ = fs::remove_dir_all(&dir);
}

/// B8：末尾空行哨兵裁剪重试——old 末尾空行代表“最终换行”。
#[test]
fn b8_trailing_empty_sentinel_retry() {
    let dir = temp_dir("b8");
    let f = dir.join("f8.txt");
    fs::write(&f, b"x\ny").unwrap(); // 3 字节，末行无换行
    assert_eq!(fs::metadata(&f).unwrap().len(), 3);
    let r = call(&format!(
        "*** Update File: {}\n@@\n-y\n-\n+y2\n+",
        f.display()
    ));
    assert!(r.ok, "{r:?}");
    let after = read(&f);
    assert_eq!(after.len(), 5, "输出 x\\ny2\\n 为 5 字节（补齐末尾换行）");
    assert_eq!(after, b"x\ny2\n");
    let _ = fs::remove_dir_all(&dir);
}

/// B9：空文件上的纯插入块（old 为空）追加到末尾，行尾 LF。
#[test]
fn b9_empty_file_pure_insert() {
    let dir = temp_dir("b9");
    let f = dir.join("f9.txt");
    fs::write(&f, b"").unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n+seed", f.display()));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&f), b"seed\n");
    assert_eq!(
        r.changed_files,
        vec![ChangedFile { path: f.display().to_string(), action: Action::Update }]
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------- D：同文件多个 Update File 块（编号↔用例映射：D1–D6）----------
// 断言依据为当前实际行为：逐块独立重读目标文件（后块基于前块结果）、
// 清单按成功执行事件列出不去重（待决 Q5）、解析先于全部执行。

/// D1：块2 匹配的行只存在于块1 生成的新内容中——顺序叠加的直接证据。
#[test]
fn d1_same_file_two_hunks_stack_sequentially() {
    let dir = temp_dir("d1");
    let f = dir.join("d1.txt");
    fs::write(&f, "alpha\nbeta\n").unwrap();
    // 三跳链（吸收原 D3）：块2 匹配块1 产物 gamma，块3 匹配块2 产物 GAMMA
    let r = call(&format!(
        "*** Update File: {f}\n@@\n-beta\n+beta\n+gamma\n*** Update File: {f}\n@@\n-gamma\n+GAMMA\n*** Update File: {f}\n@@\n-GAMMA\n+FINAL",
        f = f.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&f), b"alpha\nbeta\nFINAL\n", "最终 17 字节，三块修改同时落盘");
    assert_eq!(
        r.changed_files,
        vec![
            ChangedFile { path: f.display().to_string(), action: Action::Update },
            ChangedFile { path: f.display().to_string(), action: Action::Update },
            ChangedFile { path: f.display().to_string(), action: Action::Update },
        ],
        "清单按成功执行事件逐块列出，不去重（待决 Q5，按现有行为）"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// D2：块2 校验失败 → 块1 修改保留（中间态即终态），state_known=true。
#[test]
fn d2_same_file_second_hunk_validation_failure_keeps_first() {
    let dir = temp_dir("d2");
    let f = dir.join("d2.txt");
    fs::write(&f, "alpha\nbeta\n").unwrap();
    let r = call(&format!(
        "*** Update File: {f}\n@@\n-alpha\n+ALPHA\n*** Update File: {f}\n@@\n-missing\n+X",
        f = f.display()
    ));
    assert!(!r.ok, "{r:?}");
    let err = r.error.clone().unwrap();
    assert!(err.contains("d2.txt"), "{err}");
    assert!(err.contains("missing"), "{err}");
    assert!(r.state_known, "校验型失败未发生写入，清单可信");
    assert_eq!(r.changed_files.len(), 1);
    assert_eq!(r.changed_files[0].path, f.display().to_string());
    assert_eq!(read(&f), b"ALPHA\nbeta\n", "块1 修改保留，不被回滚");
    let _ = fs::remove_dir_all(&dir);
}


/// D4：块2 的 EOF 锚定作用于块1 追加生成的新末行——两种机制复合。
#[test]
fn d4_same_file_eof_anchor_after_append() {
    let dir = temp_dir("d4");
    let f = dir.join("d4.txt");
    fs::write(&f, "head\n").unwrap();
    let r = call(&format!(
        "*** Update File: {f}\n@@\n+tail\n*** Update File: {f}\n@@\n-tail\n+TAIL\n*** End of File",
        f = f.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(read(&f), b"head\nTAIL\n");
    assert_eq!(r.changed_files.len(), 2);
    let _ = fs::remove_dir_all(&dir);
}

/// D5：块2 解析错误 → 整个补丁不执行，块1 本可成功也不落盘。
#[test]
fn d5_same_file_parse_error_first_hunk_not_executed() {
    let dir = temp_dir("d5");
    let f = dir.join("d5.txt");
    fs::write(&f, "orig\n").unwrap();
    let r = call(&format!(
        "*** Update File: {f}\n@@\n-orig\n+changed\n*** Update File: {f}\n@@\n-x\nbad",
        f = f.display()
    ));
    assert!(!r.ok, "{r:?}");
    let err = r.error.clone().unwrap();
    assert!(err.contains("更新块须以 @@ 上下文行开始"), "{err}");
    assert!(err.contains("bad"), "{err}");
    assert!(err.contains("第 "), "{err}");
    assert!(r.state_known);
    assert!(r.changed_files.is_empty(), "解析失败不得有任何执行");
    assert_eq!(read(&f), b"orig\n", "可执行的块1 也不得落盘（先完整解析再执行）");
    let _ = fs::remove_dir_all(&dir);
}

/// D6：块1 成功后块2 写入中途故障（EFBIG）——块1 已落盘计入清单，块2 半写截断。
/// 限额与 SIGXFSZ 在隔离子进程内生效，不影响本二进制其他并行用例。
#[test]
fn d6_same_file_efbig_mid_write_keeps_prefix() {
    efbig_child_mode(); // 子进程模式：限额内执行注入场景后已在子进程内退出

    let dir = temp_dir("d6");
    let f = dir.join("d6.txt");
    fs::write(&f, "one\ntwo\n").unwrap();
    let new_lines: Vec<String> =
        (0..82).map(|i| format!("line-{:03}-{}", i, "x".repeat(92))).collect();
    let added: String = new_lines.iter().map(|l| format!("+{l}\n")).collect::<String>();
    let patch_text = patch(&format!(
        "*** Update File: {f}\n@@\n-one\n+uno\n*** Update File: {f}\n@@\n-two\n{added}",
        f = f.display()
    ));
    let r = apply_patch_under_fsize_limit("d6_same_file_efbig_mid_write_keeps_prefix", &patch_text, 4096);

    assert_eq!(r["ok"], json!(false), "{r}");
    let err = r["error"].as_str().unwrap();
    assert!(err.contains("d6.txt"), "{err}");
    assert!(err.contains("File too large"), "{err}");
    assert_eq!(r["state_known"], json!(false), "写入中途故障清单不可信：{r}");
    assert_eq!(
        r["changed_files"],
        json!([{ "path": f.display().to_string(), "action": "update" }]),
        "只有块1 计入清单：{r}"
    );

    // 块2 的完整新内容 = "uno\n" + 82×102 = 8368 字节 > 4096；截断前缀以块1 的 uno\n 开头
    let expected_full = format!("uno\n{}\n", new_lines.join("\n"));
    assert_eq!(expected_full.len(), 8368);
    let after = read(&f);
    assert_eq!(after.len(), 4096, "写入开始后失败，只落盘前 4096 字节");
    assert!(after.starts_with(b"uno\n"), "块1 修改已作为前缀落盘，须重读核实");
    assert_eq!(after.as_slice(), &expected_full.as_bytes()[..4096]);
    let _ = fs::remove_dir_all(&dir);
}

// ---------- F2：Update 只允许常规文件（看门狗子进程验证）----------

/// F2 子进程模式：由 `TCHAT_F2_INPUT` 触发，执行补丁并把结果 JSON 写到
/// `TCHAT_F2_OUTCOME` 后退出。看门狗父进程据此判定挂起，测试自身不会永久阻塞。
fn f2_child_mode() {
    let Some(input) = std::env::var_os("TCHAT_F2_INPUT") else {
        return; // 父进程模式
    };
    let outcome = std::env::var_os("TCHAT_F2_OUTCOME").expect("缺少 TCHAT_F2_OUTCOME");
    let patch_text = std::fs::read_to_string(&input).unwrap();
    let r = apply_patch(&json!({ "patch": patch_text }));
    std::fs::write(&outcome, serde_json::to_string(&r).unwrap()).unwrap();
    std::process::exit(0);
}

/// 看门狗内经真实 apply_patch 入口执行补丁：子进程在期限内未返回则杀死并报错
/// （阻塞 I/O 会让测试永久挂起，必须在测试内自保）。
fn apply_patch_with_watchdog(entry: &str, patch_text: &str) -> serde_json::Value {
    assert!(
        std::env::var_os("TCHAT_F2_INPUT").is_none(),
        "F2 子进程模式到达了未设防的用例"
    );
    let io = temp_dir("f2-io");
    let input = io.join("in.txt");
    let outcome = io.join("out.json");
    std::fs::write(&input, patch_text).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .current_dir(std::env::temp_dir()) // 同 EFBIG 夹具：稳定 CWD 防继承竞态
        .arg(entry)
        .arg("--exact")
        .env("TCHAT_F2_INPUT", &input)
        .env("TCHAT_F2_OUTCOME", &outcome)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("F2 子进程启动失败");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                assert!(st.success(), "F2 子进程异常退出：{st}");
                let text = std::fs::read_to_string(&outcome)
                    .unwrap_or_else(|e| panic!("子进程未写结果（entry 漂移或缺 f2_child_mode？）：{e}"));
                let _ = std::fs::remove_dir_all(&io);
                return serde_json::from_str(&text).unwrap();
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("看门狗超时：apply_patch 在非常规目标上挂起（F2）");
            }
        }
    }
}

/// F2：Update 对直接 FIFO、指向 FIFO 的链接、/dev/null 必须及时明确报错
/// （无假成功、无错误清单、FIFO 本体不被破坏）；常规文件对照不受影响。
#[test]
fn f2_update_rejects_non_regular_targets() {
    f2_child_mode();
    let dir = temp_dir("f2");
    let fifo = dir.join("pipe.txt");
    let c_path =
        std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes().to_vec()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) }, 0, "mkfifo 失败");
    let link_to_fifo = dir.join("link-pipe.txt");
    symlink(&fifo, &link_to_fifo).unwrap();

    let cases: [(&str, PathBuf); 3] = [
        ("直接FIFO", fifo.clone()),
        ("指向FIFO的链接", link_to_fifo),
        ("/dev/null", PathBuf::from("/dev/null")),
    ];
    for (name, path) in cases {
        let r = apply_patch_with_watchdog(
            "f2_update_rejects_non_regular_targets",
            &patch(&format!("*** Update File: {}\n@@\n-x\n+y", path.display())),
        );
        assert_eq!(r["ok"], json!(false), "{name} 不得假成功：{r}");
        assert_eq!(r["state_known"], json!(true), "{name} 为校验型失败：{r}");
        assert_eq!(r["changed_files"], json!([]), "{name} 清单必须为空：{r}");
        let err = r["error"].as_str().unwrap();
        assert!(err.contains("不是常规文件"), "{name} 错误应指明非常规文件：{err}");
        assert!(name == "/dev/null" || err.contains("pipe"), "{name} 错误应含目标路径：{err}");
    }
    // FIFO 本体原样保留（拒绝发生在任何破坏性 I/O 之前）
    assert!(std::fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo());

    // 对照：常规文件经同一子进程通道正常更新
    let reg = dir.join("reg.txt");
    fs::write(&reg, "x\n").unwrap();
    let r = apply_patch_with_watchdog(
        "f2_update_rejects_non_regular_targets",
        &patch(&format!("*** Update File: {}\n@@\n-x\n+y", reg.display())),
    );
    assert_eq!(r["ok"], json!(true), "常规文件不得被误伤：{r}");
    assert_eq!(read(&reg), b"y\n");
    let _ = fs::remove_dir_all(&dir);
}


// ---------- 高风险清单补充（2026-10-10）----------

/// PATCH-B05：两个并发 Add 同一路径、内容不同——create_new 互斥只允许一个成功；
/// 最终文件完整属于获胜者，不覆盖、不混合、不截断。
#[test]
fn patch_b05_concurrent_add_single_winner() {
    use std::sync::Arc;
    let dir = temp_dir("b05");
    let target = dir.join("race.txt");
    let t1 = Arc::new(target.clone());
    let t2 = Arc::new(target.clone());
    let h1 = std::thread::spawn(move || {
        call(&format!("*** Add File: {}\n+winner-one\n+AAAA", t1.display()))
    });
    let h2 = std::thread::spawn(move || {
        call(&format!("*** Add File: {}\n+winner-two\n+BBBB", t2.display()))
    });
    let r1 = h1.join().unwrap();
    let r2 = h2.join().unwrap();
    let wins = r1.ok as u8 + r2.ok as u8;
    assert_eq!(wins, 1, "create_new 互斥：恰一个成功：{r1:?}\n{r2:?}");
    let bytes = read(&target);
    // 完整属于获胜者，绝不混合或截断
    assert!(
        bytes == b"winner-one\nAAAA\n".to_vec() || bytes == b"winner-two\nBBBB\n".to_vec(),
        "内容必须完整属于获胜者：{bytes:?}"
    );
    let loser = if r1.ok { &r2 } else { &r1 };
    assert!(!loser.ok && loser.changed_files.is_empty(), "败者明确拒绝：{loser:?}");
    let _ = fs::remove_dir_all(&dir);
}

// ---------- N5：有限读入与生成内容限制（2026-10-10）----------

/// N5 参数层：补丁文本 ≤1 MiB（UTF-8 字节），解析前拒绝；恰 1 MiB 可用（Add 成功、内容精确）。
#[test]
fn n5_patch_text_byte_limit() {
    let dir = temp_dir("n5-patch");
    let target = dir.join("t.txt");
    let head = format!("*** Begin Patch\n*** Add File: {}\n+", target.display());
    let tail = "\n*** End Patch";
    let build = |total: usize| {
        let fill = total - head.len() - tail.len();
        format!("{head}{}{tail}", "x".repeat(fill))
    };
    let exact = 1024 * 1024;
    assert_eq!(build(exact).len(), exact, "恰 1 MiB 构造独立核对");
    assert_eq!(build(exact + 1).len(), exact + 1);
    let r = apply_patch(&json!({ "patch": build(exact) }));
    assert!(r.ok, "恰 1 MiB 应可用：{r:?}");
    let fill = exact - head.len() - tail.len();
    assert_eq!(fs::read(&target).unwrap(), format!("x{}\n", "x".repeat(fill - 1)).as_bytes(),
        "内容精确（单行＋LF 收尾）");
    let _ = fs::remove_file(&target);
    let r = apply_patch(&json!({ "patch": build(exact + 1) }));
    assert!(!r.ok, "超 1 字节必须拒绝：{r:?}");
    assert!(r.error.clone().unwrap().contains("1 MiB"), "{r:?}");
    assert!(!target.exists(), "超限补丁不得创建目标");
    let _ = fs::remove_dir_all(&dir);
}

/// N5 读入：目标文件 ≤4 MiB 且 ≤100000 逻辑行——恰上限成功改写；上限＋1（字节或行）拒绝且目标字节不变。
#[test]
fn n5_update_read_byte_and_line_limits() {
    let dir = temp_dir("n5-read");
    const MI_B: usize = 1024 * 1024;
    let needle = "\nneedle\n";
    // 恰 4 MiB（行数 2：超长行＋needle 行）→ 成功
    let f_ok = dir.join("ok.txt");
    let fill = 4 * MI_B - needle.len();
    fs::write(&f_ok, format!("{}{needle}", "x".repeat(fill))).unwrap();
    assert_eq!(fs::metadata(&f_ok).unwrap().len() as usize, 4 * MI_B);
    let r = call(&format!("*** Update File: {}\n@@\n-needle\n+NEEDLE", f_ok.display()));
    assert!(r.ok, "恰 4 MiB（同长度替换不增长）应可用：{r:?}");
    assert!(fs::read_to_string(&f_ok).unwrap().ends_with("NEEDLE\n"));

    // 4 MiB+1 字节 → 拒绝、字节不变
    let f_big = dir.join("big.txt");
    fs::write(&f_big, format!("{}{needle}", "x".repeat(fill + 1))).unwrap();
    let before = fs::read(&f_big).unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-needle\n+REPLACED", f_big.display()));
    assert!(!r.ok, "4 MiB+1 必须拒绝：{r:?}");
    assert!(r.error.clone().unwrap().contains("4 MiB"), "{r:?}");
    assert_eq!(fs::read(&f_big).unwrap(), before, "超限读入不得改变目标字节");
    assert!(r.state_known, "读入上限属校验型失败：{r:?}");

    // 100000 行恰上限 → 成功；100001 行 → 拒绝
    let f_l = dir.join("lines.txt");
    fs::write(&f_l, "l\n".repeat(100_000)).unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-l\n+L", f_l.display()));
    assert!(r.ok, "恰 100000 行应可用：{r:?}");
    let f_l2 = dir.join("lines2.txt");
    fs::write(&f_l2, "l\n".repeat(100_001)).unwrap();
    let before = fs::read(&f_l2).unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-l\n+L", f_l2.display()));
    assert!(!r.ok, "100001 行必须拒绝：{r:?}");
    assert!(r.error.clone().unwrap().contains("100000"), "{r:?}");
    assert_eq!(fs::read(&f_l2).unwrap(), before);
    // 非空无末尾换行的尾段算一行：100000 行（99999 个 "l\n"＋尾段 "l"）恰上限 → 成功
    let f_l3 = dir.join("lines3.txt");
    fs::write(&f_l3, format!("{}l", "l\n".repeat(99_999))).unwrap();
    let r = call(&format!("*** Update File: {}\n@@\n-l\n+L", f_l3.display()));
    assert!(r.ok, "尾段计行：100000 行应可用：{r:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// N5 生成内容：Add/Update 产物 ≤4 MiB 且 ≤100000 行——超限在写入前拒绝、目标不变；
/// 前序已成功操作遵守无回滚语义。
#[test]
fn n5_generated_content_limits() {
    let dir = temp_dir("n5-gen");
    // Add 生成 100001 行（补丁本身 ≤1 MiB）：拒绝且不创建目标
    let f_add = dir.join("gen-add.txt");
    let mut body = String::from("*** Begin Patch\n*** Add File: ");
    body.push_str(&f_add.display().to_string());
    body.push('\n');
    for _ in 0..100_001 { body.push_str("+\n"); } // 每行空内容 → 生成内容 "\n"×100001
    body.push_str("*** End Patch");
    assert!(body.len() <= 1024 * 1024, "补丁自身须 ≤1 MiB：{}", body.len());
    let r = apply_patch(&json!({ "patch": body }));
    assert!(!r.ok, "生成 100001 行必须拒绝：{r:?}");
    assert!(r.error.clone().unwrap().contains("100000"), "{r:?}");
    assert!(!f_add.exists(), "写入前拒绝不得创建目标");

    // 恰 100000 行 Add → 成功
    let f_add2 = dir.join("gen-add2.txt");
    let mut body2 = format!("*** Begin Patch\n*** Add File: {}\n", f_add2.display());
    for _ in 0..100_000 { body2.push_str("+\n"); }
    body2.push_str("*** End Patch");
    let r = apply_patch(&json!({ "patch": body2 }));
    assert!(r.ok, "恰 100000 行应可用：{r:?}");
    assert_eq!(fs::read_to_string(&f_add2).unwrap().lines().count(), 100_000);

    // 生成内容字节边界（验收轮）：合法读入＋远小于 1 MiB 的补丁——恰 4 MiB 成功、
    // 4 MiB+1 写前拒绝且目标原始字节不变、changed_files/state_known 符合 004。
    {
        const MI_B: usize = 1024 * 1024;
        let mk_case = |extra: usize| (PathBuf::from(dir.join(format!("genb{extra}.txt"))), extra);
        for extra in [0usize, 1] {
            let (fb, extra) = mk_case(extra);
            let b = 4 * MI_B - 8 + extra; // 生成内容 = b + 1 + 6 + 1 = 4MiB + extra
            fs::write(&fb, format!("{}\nold\n", "x".repeat(b))).unwrap();
            let patch_len = patch(&format!(
                "*** Update File: {}\n@@\n-old\n+NEWOLD", fb.display()
            )).len();
            assert!(patch_len < MI_B, "补丁远小于 1 MiB：{patch_len}");
            let before = fs::read(&fb).unwrap();
            let r = call(&format!("*** Update File: {}\n@@\n-old\n+NEWOLD", fb.display()));
            if extra == 0 {
                assert!(r.ok, "生成恰 4 MiB 应成功：{r:?}");
                assert_eq!(fs::metadata(&fb).unwrap().len() as usize, 4 * MI_B, "生成字节数独立核对");
            } else {
                assert!(!r.ok, "生成 4 MiB+1 必须拒绝：{r:?}");
                let err = r.error.clone().unwrap();
                assert!(err.contains("4 MiB"), "{err}");
                assert_eq!(fs::read(&fb).unwrap(), before, "写前拒绝：目标原始字节不变");
                assert!(r.changed_files.is_empty() && r.state_known, "004 字段如实：{r:?}");
            }
        }
    }

    // Update 生成超限：前项成功＋后项拒绝（无回滚，清单只含前项）
    let first = dir.join("first.txt");
    fs::write(&first, "a\n").unwrap();
    let victim = dir.join("victim.txt");
    fs::write(&victim, "v\n").unwrap();
    let mut huge = String::new();
    for _ in 0..100_001 { huge.push_str("+line\n"); } // 追加 100001 行 → 生成 100002 行
    let patch_text = patch(&format!(
        "*** Update File: {f}\n@@\n-a\n+A\n*** Update File: {v}\n@@\n-v\n{huge}",
        f = first.display(), v = victim.display()
    ));
    let r = apply_patch(&json!({ "patch": patch_text }));
    assert!(!r.ok, "生成超行必须拒绝：{r:?}");
    assert!(r.error.clone().unwrap().contains("100000"), "{r:?}");
    assert_eq!(fs::read(&first).unwrap(), b"A\n".to_vec(), "前项成功保留（无回滚）");
    assert_eq!(fs::read(&victim).unwrap(), b"v\n".to_vec(), "超限项写入前拒绝、字节不变");
    assert_eq!(r.changed_files.len(), 1, "清单只含已确认前项：{r:?}");
    assert!(r.state_known, "写入前校验失败：{r:?}");
    let _ = fs::remove_dir_all(&dir);
}

// ---------- 029 M2：Move 文件操作与部分成功（验收 A1/A2/A4/A5/A6/A7a/A8/A9＋展示）----------

/// A1 纯移动：常规/空文件/无末尾换行/混合行尾 → 目标逐字节等于原文；源消失；
/// 清单依次为目标 add、源 delete；中文与含空格路径可用。
#[test]
fn move_pure_moves_bytes_verbatim() {
    let dir = temp_dir("move-a1");
    for (tag, content) in [
        ("regular", b"a\nb\nc\n".to_vec()),
        ("empty", b"".to_vec()),
        ("no-final-nl", b"x\ny".to_vec()),
        ("mixed-eol", b"a\r\nb\rc\n".to_vec()),
    ] {
        let src = dir.join(format!("旧 文-{tag}.txt"));
        let dst = dir.join(format!("归 档 子目录/新 名-{tag}.txt")); // 父目录缺失→按 Add 创建
        fs::write(&src, &content).unwrap();
        let r = call(&format!(
            "*** Update File: {s}\n*** Move to: {d}",
            s = src.display(),
            d = dst.display()
        ));
        assert!(r.ok, "{tag}: {r:?}");
        assert!(r.state_known, "{tag}: {r:?}");
        assert_eq!(r.error, None, "{tag}: {r:?}");
        assert_eq!(
            r.changed_files,
            vec![
                ChangedFile { path: dst.display().to_string(), action: Action::Add },
                ChangedFile { path: src.display().to_string(), action: Action::Delete },
            ],
            "{tag}: 清单依次为目标 add、源 delete"
        );
        assert!(!src.exists(), "{tag}: 源应消失");
        assert_eq!(read(&dst), content, "{tag}: 纯移动逐字节保留（含行尾与缺失的末尾换行）");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A2 移动并修改：中文/空格路径、@@ 定位与替换；未改行行尾逐字节保留，替换行
/// 沿用源文件首个换行风格，清单两记录有序。
#[test]
fn move_with_modification_updates_content() {
    let dir = temp_dir("move-a2");
    let src = dir.join("旧 文 件.txt");
    let dst = dir.join("归档/新 文件.txt");
    fs::write(&src, "alpha\r\nbeta\r\n中文 行\r\ngamma\r\n").unwrap();
    let r = call(&format!(
        "*** Update File: {s}\n*** Move to: {d}\n@@ 中文 行\n-gamma\n+gamma2\n+added",
        s = src.display(),
        d = dst.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(
        r.changed_files,
        vec![
            ChangedFile { path: dst.display().to_string(), action: Action::Add },
            ChangedFile { path: src.display().to_string(), action: Action::Delete },
        ],
        "{r:?}"
    );
    assert!(!src.exists());
    assert_eq!(
        read(&dst),
        b"alpha\r\nbeta\r\n\xe4\xb8\xad\xe6\x96\x87 \xe8\xa1\x8c\r\ngamma2\r\nadded\r\n".to_vec(),
        "未改行保留原行尾（CRLF/中文上下文原样），替换行用源首个换行风格 CRLF"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A4 源问题：缺失/符号链接（含悬空）/FIFO/目录/非 UTF-8 → 不创建目标、不删源、
/// 清单为空、状态已知、错误含源路径。
#[test]
fn move_rejects_bad_sources_without_side_effects() {
    let dir = temp_dir("move-a4");
    let real = dir.join("real.txt");
    fs::write(&real, b"real\n").unwrap();
    let fifo = dir.join("fifo");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes().to_vec()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);

    struct Case {
        tag: &'static str,
        setup: Box<dyn Fn(&Path) -> PathBuf>,
        expect_err: &'static str,
    }
    let cases = vec![
        Case {
            tag: "missing",
            setup: Box::new(|dir: &Path| dir.join("no-such.txt")),
            expect_err: "不存在",
        },
        Case {
            tag: "symlink",
            setup: Box::new(|dir: &Path| {
                let target = dir.join("real.txt");
                let link = dir.join("link.txt");
                symlink(&target, &link).unwrap();
                link
            }),
            expect_err: "符号链接",
        },
        Case {
            tag: "dangling",
            setup: Box::new(|dir: &Path| {
                let link = dir.join("dangling.txt");
                symlink(dir.join("nowhere.txt"), &link).unwrap();
                link
            }),
            expect_err: "符号链接",
        },
        Case {
            tag: "fifo",
            // 顶部已创建 dir/fifo（供收尾复查），此处只复用同一对象
            setup: Box::new(|dir: &Path| dir.join("fifo")),
            expect_err: "常规文件",
        },
        Case {
            tag: "directory",
            setup: Box::new(|dir: &Path| {
                let d = dir.join("sub-dir");
                fs::create_dir_all(&d).unwrap();
                d
            }),
            expect_err: "常规文件",
        },
        Case {
            tag: "non-utf8",
            setup: Box::new(|dir: &Path| {
                let f = dir.join("bin.txt");
                fs::write(&f, [0xFF, 0xFE, 0x00]).unwrap();
                f
            }),
            expect_err: "UTF-8",
        },
    ];
    for (i, case) in cases.iter().enumerate() {
        let src = (case.setup)(&dir);
        let dst = dir.join(format!("dst-{i}.txt"));
        let r = call(&format!(
            "*** Update File: {s}\n*** Move to: {d}",
            s = src.display(),
            d = dst.display()
        ));
        assert!(!r.ok, "{}: {r:?}", case.tag);
        assert!(r.changed_files.is_empty(), "{}: 清单为空：{r:?}", case.tag);
        assert!(r.state_known, "{}: 未发生写入：{r:?}", case.tag);
        let err = r.error.clone().unwrap_or_default();
        assert!(err.contains(case.expect_err), "{}: 错误应含「{}」：{err}", case.tag, case.expect_err);
        assert!(err.contains(&src.display().to_string()), "{}: 错误含源路径：{err}", case.tag);
        assert!(!dst.exists(), "{}: 不得创建目标", case.tag);
        // 源对象原样保留
        match case.tag {
            "missing" => {
                assert!(!src.exists(), "缺失源不得被创建");
            }
            "dangling" => {
                assert!(
                    fs::symlink_metadata(&src).unwrap().file_type().is_symlink(),
                    "dangling: 源符号链接本体不动"
                );
            }
            "non-utf8" => assert_eq!(read(&src), [0xFF, 0xFE, 0x00].to_vec(), "{}: 源字节不变", case.tag),
            "directory" => assert!(src.is_dir(), "{}: 目录不动", case.tag),
            _ => {}
        }
    }
    assert!(real.exists(), "real 不受影响");
    assert!(fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo(), "FIFO 不动");
    let _ = fs::remove_dir_all(&dir);
}

/// A5 目标冲突：已存在的各类对象（常规/目录/符号链接/悬空/指向源的硬链接）、
/// 源=目标同串、`.`/`..` 等价路径 → 按目标冲突拒绝；既有对象与源均不变。
#[test]
fn move_rejects_existing_targets() {
    let dir = temp_dir("move-a5");
    // 指向源的硬链接：链接数 2，双方内容与存在状态不变
    {
        let src = dir.join("hl-src.txt");
        let dst = dir.join("hl-dst.txt");
        fs::write(&src, b"hard\n").unwrap();
        fs::hard_link(&src, &dst).unwrap();
        let r = call(&format!(
            "*** Update File: {s}\n*** Move to: {d}",
            s = src.display(),
            d = dst.display()
        ));
        assert!(!r.ok && r.state_known && r.changed_files.is_empty(), "{r:?}");
        assert!(r.error.clone().unwrap().contains("已存在"), "{r:?}");
        assert_eq!(read(&src), b"hard\n".to_vec(), "源不变");
        assert_eq!(read(&dst), b"hard\n".to_vec(), "硬链接目标不变");
        assert_eq!(
            fs::symlink_metadata(&dst).unwrap().nlink(),
            2,
            "链接数不变（未删任何一方）"
        );
    }
    // 常规文件 / 目录 / 符号链接 / 悬空链接
    for (tag, mk) in [
        ("file", Box::new(|p: &Path| fs::write(p, b"x\n").unwrap()) as Box<dyn Fn(&Path)>),
        ("dir", Box::new(|p: &Path| fs::create_dir_all(p).unwrap())),
        (
            "symlink",
            Box::new(|p: &Path| symlink("/etc/hostname", p).unwrap()),
        ),
        ("dangling", Box::new(|p: &Path| symlink("/no/such/target", p).unwrap())),
    ] {
        let src = dir.join(format!("src-{tag}.txt"));
        let dst = dir.join(format!("dst-{tag}"));
        fs::write(&src, b"src\n").unwrap();
        mk(&dst);
        let dst_before = fs::symlink_metadata(&dst).unwrap();
        let r = call(&format!(
            "*** Update File: {s}\n*** Move to: {d}",
            s = src.display(),
            d = dst.display()
        ));
        assert!(!r.ok && r.state_known && r.changed_files.is_empty(), "{tag}: {r:?}");
        let err = r.error.clone().unwrap_or_default();
        assert!(err.contains("已存在"), "{tag}: {err}");
        assert_eq!(read(&src), b"src\n".to_vec(), "{tag}: 源不变");
        let dst_after = fs::symlink_metadata(&dst).unwrap();
        assert_eq!(
            (dst_after.file_type(), dst_after.len()),
            (dst_before.file_type(), dst_before.len()),
            "{tag}: 目标原有对象不变"
        );
    }
    // 源=目标同串；`./`、`..` 等价路径
    {
        let src = dir.join("same.txt");
        fs::write(&src, b"same\n").unwrap();
        for dst_expr in [
            src.display().to_string(),
            format!("{}/./same.txt", dir.display()),
            format!("{}/sub/../same.txt", dir.display()),
        ] {
            let r = call(&format!(
                "*** Update File: {s}\n*** Move to: {d}",
                s = src.display(),
                d = dst_expr
            ));
            assert!(!r.ok && r.changed_files.is_empty(), "{dst_expr}: {r:?}");
            let err = r.error.clone().unwrap_or_default();
            assert!(
                err.contains("同一") || err.contains("已存在"),
                "{dst_expr}: 按目标冲突拒绝：{err}"
            );
            assert_eq!(read(&src), b"same\n".to_vec(), "{dst_expr}: 源不变");
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A6 半写：RLIMIT_FSIZE 子进程内移动 → 目标残片可观察、源完整、不虚报 add、
/// 状态未知；目标写入路径确实执行。
#[test]
fn move_half_write_preserves_source() {
    efbig_child_mode(); // 子进程模式：限额内执行注入场景后已在子进程内退出
    let dir = temp_dir("move-a6");
    let src = dir.join("src.txt");
    let dst = dir.join("dst.txt");
    let body = "line-0000\n".repeat(1024); // 10 KiB
    fs::write(&src, &body).unwrap();
    let patch_text = patch(&format!(
        "*** Update File: {s}\n*** Move to: {d}",
        s = src.display(),
        d = dst.display()
    ));
    let r = apply_patch_under_fsize_limit("move_half_write_preserves_source", &patch_text, 4096);
    assert_eq!(r["ok"], json!(false), "{r}");
    assert_eq!(r["state_known"], json!(false), "目标可能半写：{r}");
    assert_eq!(r["changed_files"], json!([]), "未完整写入不得记 add：{r}");
    let err = r["error"].as_str().unwrap();
    assert!(err.contains(&dst.display().to_string()), "错误含目标路径：{err}");
    assert_eq!(read(&src), body.as_bytes().to_vec(), "源内容完整保留");
    let partial = read(&dst);
    assert_eq!(partial.len(), 4096, "写入开始后失败，落盘软限制内前 4096 字节");
    assert_eq!(partial.as_slice(), &body.as_bytes()[..4096], "残片为确定性的内容前缀");
    let _ = fs::remove_dir_all(&dir);
}

/// A7a 删源失败：目标可创建但源父目录不可写（非 root 身份下 unlink EACCES）→
/// 清单保留目标 add、不记源 delete、状态未知、错误含两路径与失败阶段；
/// 已写好的目标保留，不自动清理。
#[test]
fn move_delete_failure_keeps_target_add() {
    if unsafe { libc::getuid() } == 0 {
        // root 无视目录权限位，EACCES 不可触发；换用只读 bind mount 之外的
        // 确定性替代：跳过并在报告中注明（本容器执行身份非 root，通常可达）。
        eprintln!("以 root 运行：权限位不构成删除失败，跳过 A7a 权限变体");
        return;
    }
    let dir = temp_dir("move-a7a");
    let srcdir = dir.join("srcdir");
    let dstdir = dir.join("dstdir");
    fs::create_dir_all(&srcdir).unwrap();
    fs::create_dir_all(&dstdir).unwrap();
    let src = srcdir.join("f.txt");
    let dst = dstdir.join("g.txt");
    fs::write(&src, b"content\n").unwrap();
    set_mode(&srcdir, 0o555);
    let r = call(&format!(
        "*** Update File: {s}\n*** Move to: {d}",
        s = src.display(),
        d = dst.display()
    ));
    set_mode(&srcdir, 0o755); // 先恢复再断言/清理
    assert!(!r.ok, "{r:?}");
    assert!(!r.state_known, "删除失败：{r:?}");
    assert_eq!(
        r.changed_files,
        vec![ChangedFile { path: dst.display().to_string(), action: Action::Add }],
        "清单保留已完整写入的目标 add：{r:?}"
    );
    let err = r.error.clone().unwrap_or_default();
    assert!(err.contains("删除"), "错误标明失败阶段：{err}");
    assert!(err.contains(&src.display().to_string()), "错误含源路径：{err}");
    assert!(err.contains(&dst.display().to_string()), "错误含目标路径：{err}");
    assert_eq!(read(&dst), b"content\n".to_vec(), "目标保留（不自动清理）");
    assert_eq!(read(&src), b"content\n".to_vec(), "源仍在");
    let _ = fs::remove_dir_all(&dir);
}

/// A8 有序块链 A→B、B→C（并修改），随后失败块 → 每次真实操作入清单且不去重、
/// 失败停止不回滚；重复同一补丁：源已缺失 → 失败且不改已成功目标。
#[test]
fn move_chain_and_repeat_patch() {
    let dir = temp_dir("move-a8");
    let a = dir.join("A.txt");
    let b = dir.join("B.txt");
    let c = dir.join("C.txt");
    let d = dir.join("D.txt");
    let missing = dir.join("missing.txt");
    fs::write(&a, b"A-content\n").unwrap();
    let patch_text = patch(&format!(
        "*** Update File: {a}\n*** Move to: {b}\n\
         *** Update File: {b}\n*** Move to: {c}\n@@\n-A-content\n+C-content\n\
         *** Update File: {m}\n*** Move to: {d}",
        a = a.display(),
        b = b.display(),
        c = c.display(),
        d = d.display(),
        m = missing.display()
    ));
    let r = apply_patch(&json!({ "patch": patch_text }));
    assert!(!r.ok, "第三块失败：{r:?}");
    assert!(r.state_known, "失败为校验型（源不存在）：{r:?}");
    assert_eq!(
        r.changed_files,
        vec![
            ChangedFile { path: b.display().to_string(), action: Action::Add },
            ChangedFile { path: a.display().to_string(), action: Action::Delete },
            ChangedFile { path: c.display().to_string(), action: Action::Add },
            ChangedFile { path: b.display().to_string(), action: Action::Delete },
        ],
        "清单按实际完成顺序追加（B 的 add 与 delete 均记录，不去重）：{r:?}"
    );
    assert!(!a.exists() && !b.exists(), "A、B 均已消失");
    assert_eq!(read(&c), b"C-content\n".to_vec(), "移动并修改的目标内容");
    assert!(!d.exists(), "失败块不创建目标");

    // 重复同一补丁：首块 A→B 因 A 缺失失败；C 不被改动
    let r2 = apply_patch(&json!({ "patch": patch_text }));
    assert!(!r2.ok && r2.changed_files.is_empty() && r2.state_known, "{r2:?}");
    assert_eq!(read(&c), b"C-content\n".to_vec(), "已成功目标内容不变");
    let _ = fs::remove_dir_all(&dir);
}

/// A9 资源边界：源超读入上限（字节/行数）→ 不创建目标；生成内容超限 →
/// 创建目标前拒绝，源与目标均不变。
#[test]
fn move_respects_size_limits() {
    let dir = temp_dir("move-a9");
    // 源 > 4 MiB：读入上限拒绝
    let big = dir.join("big-src.txt");
    fs::write(&big, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
    let dst = dir.join("big-dst.txt");
    let r = call(&format!(
        "*** Update File: {s}\n*** Move to: {d}",
        s = big.display(),
        d = dst.display()
    ));
    assert!(!r.ok && r.state_known && r.changed_files.is_empty(), "{r:?}");
    assert!(r.error.clone().unwrap().contains("4 MiB"), "{r:?}");
    assert!(!dst.exists(), "读入拒绝不创建目标");
    assert_eq!(fs::metadata(&big).unwrap().len(), 4 * 1024 * 1024 + 1, "源不变");

    // 源行数 > 100000：读入上限拒绝（字节远小于 4 MiB）
    let many = dir.join("many-lines.txt");
    fs::write(&many, format!("{}z", "z\n".repeat(100_000))).unwrap(); // 100001 逻辑行
    let dst2 = dir.join("many-dst.txt");
    let r = call(&format!(
        "*** Update File: {s}\n*** Move to: {d}",
        s = many.display(),
        d = dst2.display()
    ));
    assert!(!r.ok && r.state_known && r.changed_files.is_empty(), "{r:?}");
    assert!(r.error.clone().unwrap().contains("100000"), "{r:?}");
    assert!(!dst2.exists());

    // 移动并修改：生成内容 > 4 MiB（源 4 MiB-2＋替换更长行 → 4 MiB+1）→ 创建目标前拒绝
    let grown = dir.join("grown.txt");
    fs::write(&grown, format!("{}\nold\n", "x".repeat(4 * 1024 * 1024 - 7))).unwrap();
    let dst3 = dir.join("grown-dst.txt");
    let r = call(&format!(
        "*** Update File: {s}\n*** Move to: {d}\n@@\n-old\n+NEWOLD",
        s = grown.display(),
        d = dst3.display()
    ));
    assert!(!r.ok && r.state_known && r.changed_files.is_empty(), "{r:?}");
    assert!(r.error.clone().unwrap().contains("4 MiB"), "生成内容写前检查：{r:?}");
    assert!(!dst3.exists(), "超限不创建目标");
    assert_eq!(read(&grown)[..16], b"xxxxxxxxxxxxxxxx"[..], "源不变");
    let _ = fs::remove_dir_all(&dir);
}

/// 012/029 展示：Move 产出两条卡片——目标 Add（真实新内容）＋源 Delete（真实旧
/// 内容）；结果 JSON 与清单并行且不含展示字段；部分成功只展示已确认操作。
#[test]
fn move_display_shows_two_cards() {
    let dir = temp_dir("move-disp");
    let src = dir.join("s.txt");
    let dst = dir.join("d.txt");
    fs::write(&src, "one\ntwo\n").unwrap();
    let patch_text = patch(&format!(
        "*** Update File: {s}\n*** Move to: {d}\n@@\n-two\n+TWO",
        s = src.display(),
        d = dst.display()
    ));
    let (r, changes) =
        terminal_chat::apply_patch::apply_patch_with_display(&json!({ "patch": patch_text }));
    assert!(r.ok, "{r:?}");
    assert_eq!(changes.len(), 2, "两条展示卡片：{changes:?}");
    assert_eq!(changes[0].path, dst.display().to_string());
    match &changes[0].kind {
        terminal_chat::prompt::PatchChangeKind::Add { content } => {
            assert_eq!(content, "one\nTWO\n", "目标卡片＝真实新内容");
        }
        other => panic!("卡片 0 应为 Add：{other:?}"),
    }
    assert_eq!(changes[1].path, src.display().to_string());
    match &changes[1].kind {
        terminal_chat::prompt::PatchChangeKind::Delete { content } => {
            assert_eq!(content.as_deref(), Some("one\ntwo\n"), "源卡片＝真实旧内容");
        }
        other => panic!("卡片 1 应为 Delete：{other:?}"),
    }
    let _ = fs::remove_dir_all(&dir);
}

// ---------- 029 M3：第四级 Unicode 匹配容差（验收 B1/B3/B4/C1 的文件级行为）----------

/// B1 集成层：源文行内含全部四组 Unicode 字符，补丁上下文/旧行全用 ASCII 对应物
/// → @@ 定位与旧行匹配均成功；只按指定映射替换，未涉及行逐字节保留。
#[test]
fn update_level4_matches_all_groups_inline() {
    let dir = temp_dir("m3-b1");
    let f = dir.join("b1.txt");
    // 行内位置（非行首尾），保证不被 trim 先行消除
    let src_lines = [
        format!("前{}后", '\u{2014}'),          // em dash → '-'
        format!("{}quoted{}", '\u{201C}', '\u{201D}'), // 弯双引号 → '"'
        format!("it{}s", '\u{2019}'),           // 弯单引号 → '\''
        format!("keep{}mid", '\u{00A0}'),       // NBSP → ' '
        format!("w{}ide", '\u{2003}'),          // em space → ' '
        "plain".to_string(),
    ];
    let src_text = src_lines.join("\n") + "\n";
    fs::write(&f, &src_text).unwrap();
    // 补丁：旧行全部用 ASCII 对应物匹配（第一~三级必失败，走第四级）
    let r = call(&format!(
        "*** Update File: {f}\n@@\n\
         -前-后\n\
         -\"quoted\"\n\
         -it's\n\
         -keep mid\n\
         -w ide\n\
         +REPLACED\n+plain2",
        f = f.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(r.changed_files.len(), 1);
    // 未涉及行逐字节保留（plain 原样）；替换与新增按补丁文本写入
    let after = fs::read_to_string(&f).unwrap();
    assert_eq!(after, "REPLACED\nplain2\nplain\n", "目标字节与预期一致：{after:?}");
    let _ = fs::remove_dir_all(&dir);
}

/// B3：Unicode 上下文＋ASCII 补丁匹配；新增文本含另一组 Unicode；混合行尾——
/// 未修改上下文逐字节保留（含弯引号与 CRLF），新增行按补丁文字＋004 行尾规则。
#[test]
fn update_level4_preserves_unmodified_unicode_bytes() {
    let dir = temp_dir("m3-b3");
    let f = dir.join("b3.txt");
    // 首个换行 CRLF ⇒ preferred ending CRLF；未改行保留各自原行尾
    let src = "\u{201C}头部\u{201D}\r\nkeep\u{00A0}me\r\nold\u{2013}line\r\n";
    fs::write(&f, src).unwrap();
    let (r, changes) = terminal_chat::apply_patch::apply_patch_with_display(&json!({ "patch":
        patch(&format!(
            "*** Update File: {f}\n@@ \"头部\"\n-keep me\n+新‘增’行",
            f = f.display()
        ))
    }));
    assert!(r.ok, "{r:?}");
    // 头部上下文逐字节保留（弯引号＋CRLF）；旧行按第四级匹配删除；
    // 新增行文字按补丁写入、行尾用 preferred CRLF
    assert_eq!(
        read(&f),
        "\u{201C}头部\u{201D}\r\n新‘增’行\r\nold\u{2013}line\r\n".as_bytes().to_vec(),
        "未修改行逐字节保留，新增行用补丁文字＋CRLF"
    );
    // 展示 diff 来自真实前后内容：含原始弯引号（未被归一化）
    match &changes[0].kind {
        terminal_chat::prompt::PatchChangeKind::Update { diff } => {
            let d = diff.clone();
            assert!(d.contains("\u{201C}头部\u{201D}"), "diff 含原始弯引号上下文：{d}");
        }
        other => panic!("Update 形状：{other:?}"),
    }
    let _ = fs::remove_dir_all(&dir);
}

/// B4 集成层：未映射标点/全角/大小写/组合字符/连续空格差异 → 匹配失败、字节不变。
#[test]
fn update_level4_unmapped_differences_fail() {
    let dir = temp_dir("m3-b4");
    let f = dir.join("b4.txt");
    let before = "a〜b\nａｂ\nAb\ná b\nx  y\n";
    fs::write(&f, before).unwrap();
    for old_line in ["a~b", "ab", "ab", "a b", "x y"] {
        let r = call(&format!(
            "*** Update File: {f}\n@@\n-{old}\n+NEVER",
            f = f.display(),
            old = old_line
        ));
        assert!(!r.ok, "未映射差异不应匹配：{old_line} → {r:?}");
        assert!(r.state_known && r.changed_files.is_empty(), "{r:?}");
    }
    assert_eq!(fs::read_to_string(&f).unwrap(), before, "失败后字节不变");
    let _ = fs::remove_dir_all(&dir);
}

/// C1：源文含弯引号/特殊空格，Move 块用 ASCII 上下文并修改 → 移动＋第四级匹配
/// 共同生效；不匹配对照不创建目标、不删源。
#[test]
fn move_with_level4_matching_moves_and_modifies() {
    let dir = temp_dir("m3-c1");
    let src = dir.join("c1-src.txt");
    let dst = dir.join("c1-dst.txt");
    let bad_dst = dir.join("c1-bad-dst.txt");
    fs::write(&src, "\u{201C}标题\u{201D}\nmode=dev\nkeep\u{00A0}this\n").unwrap();

    // 正例：@@ 定位用 ASCII 引号，旧行 ASCII 空格
    let r = call(&format!(
        "*** Update File: {s}\n*** Move to: {d}\n@@ \"标题\"\n-mode=dev\n+mode=test",
        s = src.display(),
        d = dst.display()
    ));
    assert!(r.ok, "{r:?}");
    assert_eq!(
        r.changed_files,
        vec![
            ChangedFile { path: dst.display().to_string(), action: Action::Add },
            ChangedFile { path: src.display().to_string(), action: Action::Delete },
        ],
        "{r:?}"
    );
    assert!(!src.exists());
    // 未修改上下文保留原始 Unicode 字节；替换行按补丁写入
    assert_eq!(
        read(&dst),
        "\u{201C}标题\u{201D}\nmode=test\nkeep\u{00A0}this\n".as_bytes().to_vec(),
        "目标保留未归一化上下文"
    );

    // 对照：未映射差异（波浪线）匹配失败 → 不创建目标、不删源
    fs::write(&src, "a〜b\n").unwrap();
    let r = call(&format!(
        "*** Update File: {s}\n*** Move to: {d}\n@@\n-a~b\n+z",
        s = src.display(),
        d = bad_dst.display()
    ));
    assert!(!r.ok && r.state_known && r.changed_files.is_empty(), "{r:?}");
    assert!(!bad_dst.exists(), "不匹配不得创建目标");
    assert_eq!(fs::read_to_string(&src).unwrap(), "a〜b\n", "源不删");
    let _ = fs::remove_dir_all(&dir);
}

/// 029 补强（comprehensive-testing）：合法前序块＋后续非法 Move 语法 → 完整解析
/// 阶段失败，前序块也不执行（空清单、已知状态、目录无新文件）。
#[test]
fn invalid_move_syntax_after_legal_block_executes_nothing() {
    let dir = temp_dir("move-syn2");
    let never = dir.join("never.txt");
    let a = dir.join("a.txt");
    fs::write(&a, b"keep\n").unwrap();
    // 前序合法 Add＋后续重复 Move：整体解析失败
    let r = apply_patch(&json!({ "patch": patch(&format!(
        "*** Add File: {n}\n+content\n*** Update File: {a}\n*** Move to: {b}\n*** Move to: {c}\n@@\n-keep\n+K",
        n = never.display(), a = a.display(), b = dir.join("b.txt").display(), c = dir.join("c.txt").display()
    )) }));
    assert!(!r.ok, "{r:?}");
    assert!(r.changed_files.is_empty(), "前序合法块也不得执行：{r:?}");
    assert!(r.state_known, "{r:?}");
    let err = r.error.clone().unwrap_or_default();
    assert!(err.contains("Move"), "{err}");
    assert!(!never.exists(), "不创建前序目标");
    assert_eq!(read(&a), b"keep\n".to_vec(), "后续块目标不动");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "目录内无新文件");
    let _ = fs::remove_dir_all(&dir);
}

/// 029 补强（comprehensive-testing）：部分成功（目标写好、删源失败）时，展示
/// 卡片只含已确认完成的目标 Add——不能把计划中的源删除画成成功。
#[test]
fn move_partial_success_display_shows_only_completed_add() {
    if unsafe { libc::getuid() } == 0 {
        eprintln!("以 root 运行：权限位不构成删除失败，跳过");
        return;
    }
    let dir = temp_dir("move-pdisp");
    let srcdir = dir.join("srcdir");
    let dstdir = dir.join("dstdir");
    fs::create_dir_all(&srcdir).unwrap();
    fs::create_dir_all(&dstdir).unwrap();
    let src = srcdir.join("f.txt");
    let dst = dstdir.join("g.txt");
    fs::write(&src, "PARTIAL\n").unwrap();
    set_mode(&srcdir, 0o555);
    let patch_text = patch(&format!(
        "*** Update File: {s}\n*** Move to: {d}",
        s = src.display(),
        d = dst.display()
    ));
    let (r, changes) =
        terminal_chat::apply_patch::apply_patch_with_display(&json!({ "patch": patch_text }));
    set_mode(&srcdir, 0o755);
    assert!(!r.ok && !r.state_known, "{r:?}");
    assert_eq!(r.changed_files.len(), 1, "结果只保留目标 add：{r:?}");
    assert_eq!(changes.len(), 1, "展示只含已确认完成的目标卡片：{changes:?}");
    assert_eq!(changes[0].path, dst.display().to_string());
    assert!(matches!(
        &changes[0].kind,
        terminal_chat::prompt::PatchChangeKind::Add { content } if content == "PARTIAL\n"
    ));
    let _ = fs::remove_dir_all(&dir);
}
