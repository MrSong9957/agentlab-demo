//! 015 规格验收：用户轮上下文注入（项目规则 AGENTS.md 链＋环境上下文）。
//! 全部使用独立临时目录＋固定时钟/来源，不依赖真实用户目录或真实时间；
//! 预期上下文文本一律由测试侧按规格 §1 字节级形态**独立重建**，不调用生产组装函数。

use serde_json::{json, Value};
use terminal_chat::proc_session::ProcessManager;
use std::path::{Path, PathBuf};
use terminal_chat::context::{build_context_message, context_body, ContextSources};
use terminal_chat::memory::{Clock, MemoriesConfig};
use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord};
use terminal_chat::turn::{run_tool_turn_with_mode, run_user_turn_with_context, ToolMode, TurnNotify};

/// 固定时钟锚点：UTC 2025-10-09。
const T0: i64 = 1_760_000_000;
const T0_DATE: &str = "2025-10-09";
/// 015 §1 固定替换声明（逐字节）。
const DECL: &str = "The following context replaces the previously provided AGENTS.md instructions and environment context.";

fn clock_at(t: i64) -> Clock {
    std::sync::Arc::new(move || t)
}

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "tchat-015-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sources(global_dir: &Path, cwd: &Path, shell: Option<&str>, t: i64) -> ContextSources {
    ContextSources {
        global_dir: global_dir.to_path_buf(),
        cwd: cwd.to_path_buf(),
        shell: shell.map(str::to_string),
        clock: clock_at(t),
    }
}

/// 测试侧 XML 文本转义（独立实现，勿引用生产代码）。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// 测试侧独立重建 015 §1 字节级形态：`global`/`project` 为原始文件文本
/// （重建内部做与规格一致的首尾 trim 与段间分隔）。
fn rebuild_ctx(
    cwd: &str,
    shell: Option<&str>,
    date: &str,
    global: Option<&str>,
    project: &[&str],
) -> String {
    let mut segs: Vec<&str> = Vec::new();
    if let Some(g) = global {
        if !g.trim().is_empty() {
            segs.push(g.trim());
        }
    }
    let mut has_project = false;
    for p in project {
        if !p.trim().is_empty() {
            segs.push(p.trim());
            has_project = true;
        }
    }
    let mut out = String::new();
    if !segs.is_empty() {
        if has_project {
            out.push_str(&format!("# AGENTS.md instructions for {cwd}\n\n"));
        } else {
            out.push_str("# AGENTS.md instructions\n\n");
        }
        out.push_str("<INSTRUCTIONS>\n");
        out.push_str(&segs.join("\n\n--- project-doc ---\n\n"));
        out.push_str("\n</INSTRUCTIONS>\n\n");
    }
    out.push_str("<environment_context>\n");
    out.push_str(&format!("  <cwd>{}</cwd>\n", xml_escape(cwd)));
    if let Some(sh) = shell {
        if !sh.trim().is_empty() {
            out.push_str(&format!("  <shell>{}</shell>\n", xml_escape(sh)));
        }
    }
    out.push_str(&format!("  <current_date>{date}</current_date>\n"));
    out.push_str("</environment_context>");
    out
}

fn answer(text: &str) -> Value {
    json!({ "choices": [{ "message": { "role": "assistant", "content": text },
        "finish_reason": "stop" }] })
}

fn tool_reply(id: &str, name: &str, arguments: &Value) -> Value {
    json!({ "choices": [{ "message": { "role": "assistant", "content": null,
        "tool_calls": [{ "id": id, "type": "function",
            "function": { "name": name, "arguments": arguments.to_string() } }] },
        "finish_reason": "tool_calls" }] })
}

fn user(content: &str) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: content.to_string(),
    })
}

fn assistant(content: &str) -> HistoryItem {
    HistoryItem::Text(HistoryMessage {
        role: HistoryRole::Assistant,
        content: content.to_string(),
    })
}

/// 请求/历史中上下文消息计数（按块标记独立计数，不依赖生产识别器）。
fn ctx_marker_count(texts: &[&str]) -> usize {
    texts
        .iter()
        .filter(|t| t.contains("<environment_context>") && t.contains("</environment_context>"))
        .count()
}

/// c1：首轮注入——三层来源（全局＋根＋cwd）字节级一致，ctx 先于用户输入，
/// 持久化点逐项可见。
#[test]
fn t01_first_turn_full_context_byte_exact() {
    let cfg = Tmp::new("c1-cfg");
    let proj = Tmp::new("c1-proj");
    std::fs::write(cfg.path().join("AGENTS.md"), "全局规则A\n").unwrap();
    let root = proj.path().join("repo");
    let sub = root.join("pkg");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(root.join("AGENTS.md"), "根规则R\n").unwrap();
    std::fs::write(sub.join("AGENTS.md"), "项目规则C\n").unwrap();
    let src = sources(cfg.path(), &sub, Some("/bin/testsh"), T0);

    let mut history = Vec::new();
    let mut bodies = Vec::new();
    let mut recorded = Vec::new();
    let reply = run_user_turn_with_context(
        &mut history,
        "你好",
        &mut |b| {
            bodies.push(b.clone());
            Ok(answer("完成"))
        },
        &mut |_| {},
        None,
        &mut |h| recorded.push(h.len()),
        ToolMode::User,
        &src,
    &ProcessManager::new(),
    )
    .unwrap();
    assert_eq!(reply, "完成");
    assert_eq!(bodies.len(), 1);

    let cwd = sub.display().to_string();
    let expected = rebuild_ctx(
        &cwd,
        Some("/bin/testsh"),
        T0_DATE,
        Some("全局规则A\n"),
        &["根规则R\n", "项目规则C\n"],
    );
    let msgs = bodies[0]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3, "system → ctx → 本轮输入");
    assert_eq!(msgs[0]["role"], json!("system"));
    assert_eq!(msgs[1]["role"], json!("user"));
    assert_eq!(msgs[1]["content"], json!(expected), "ctx 与独立重建逐字节一致");
    assert_eq!(msgs[2]["role"], json!("user"));
    assert_eq!(msgs[2]["content"], json!("你好"));

    // 历史与持久化点：ctx 先于 user 入史；每个入史点各回调一次
    assert_eq!(history.len(), 3, "ctx、user、assistant");
    match (&history[0], &history[1]) {
        (
            HistoryItem::Text(HistoryMessage {
                role: r1,
                content: c1,
            }),
            HistoryItem::Text(HistoryMessage {
                role: r2,
                content: c2,
            }),
        ) => {
            assert_eq!(*r1, HistoryRole::User);
            assert_eq!(c1, &expected);
            assert_eq!(*r2, HistoryRole::User);
            assert_eq!(c2, "你好");
        }
        other => panic!("历史前两项应为 ctx 与本轮输入：{other:?}"),
    }
    assert_eq!(recorded, vec![1, 2, 3], "三个入史点：首快照恰仅含 ctx");
}

/// c2：无规则→仅环境块；仅全局→无 for 后缀；SHELL 缺失/纯空白→无 shell 行；
/// 纯空白来源静默跳过。
#[test]
fn t02_env_only_global_only_blank_shell() {
    let cfg = Tmp::new("c2-cfg");
    let proj = Tmp::new("c2-proj");
    let cwd = proj.path().display().to_string();

    // 无任何规则文件：仅环境块、无标题行，无提示
    let built = build_context_message(&sources(cfg.path(), proj.path(), Some("/bin/sh"), T0));
    assert_eq!(
        built.message,
        rebuild_ctx(&cwd, Some("/bin/sh"), T0_DATE, None, &[])
    );
    assert!(!built.message.contains("AGENTS.md instructions"));
    assert!(built.notices.is_empty());

    // 仅全局：无 ` for {cwd}` 后缀
    std::fs::write(cfg.path().join("AGENTS.md"), "仅全局G\n").unwrap();
    let built = build_context_message(&sources(cfg.path(), proj.path(), Some("/bin/sh"), T0));
    assert_eq!(
        built.message,
        rebuild_ctx(&cwd, Some("/bin/sh"), T0_DATE, Some("仅全局G\n"), &[])
    );
    assert!(!built.message.contains(&format!("instructions for {cwd}")));

    // SHELL 缺失与纯空白：整行省略
    for shell in [None, Some("  \t ")] {
        let built = build_context_message(&sources(cfg.path(), proj.path(), shell, T0));
        assert_eq!(
            built.message,
            rebuild_ctx(&cwd, shell, T0_DATE, Some("仅全局G\n"), &[])
        );
        assert!(!built.message.contains("<shell>"));
    }

    // 纯空白规则文件＝跳过（无提示）
    std::fs::write(cfg.path().join("AGENTS.md"), " \n\t\n").unwrap();
    let built = build_context_message(&sources(cfg.path(), proj.path(), Some("/bin/sh"), T0));
    assert_eq!(
        built.message,
        rebuild_ctx(&cwd, Some("/bin/sh"), T0_DATE, None, &[])
    );
    assert!(built.notices.is_empty(), "空白来源静默跳过：{:?}", built.notices);
}

/// c2：项目链取最近 `.git` 根、按根→cwd 排序、无文件目录跳过。
#[test]
fn t03_chain_nearest_git_root_skips_gap_dirs() {
    let proj = Tmp::new("c2-chain");
    let outer = proj.path().join("outer");
    let mid = outer.join("mid");
    let deep = mid.join("gap").join("deep");
    std::fs::create_dir_all(outer.join(".git")).unwrap();
    std::fs::create_dir_all(mid.join(".git")).unwrap();
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(outer.join("AGENTS.md"), "外层O\n").unwrap();
    std::fs::write(mid.join("AGENTS.md"), "中层M\n").unwrap();
    std::fs::write(deep.join("AGENTS.md"), "深层D\n").unwrap();

    // 最近根＝mid（outer 的更远根不得入选）；gap 目录无文件被跳过
    let built = build_context_message(&sources(proj.path(), &deep, Some("/bin/sh"), T0));
    let cwd = deep.display().to_string();
    assert_eq!(
        built.message,
        rebuild_ctx(&cwd, Some("/bin/sh"), T0_DATE, None, &["中层M\n", "深层D\n"])
    );

    // 无 .git 祖先：只查 cwd 一个目录
    let loose = Tmp::new("c2-loose");
    std::fs::write(loose.path().join("AGENTS.md"), "散目录L\n").unwrap();
    let built = build_context_message(&sources(proj.path(), loose.path(), Some("/bin/sh"), T0));
    let cwd = loose.path().display().to_string();
    assert_eq!(
        built.message,
        rebuild_ctx(&cwd, Some("/bin/sh"), T0_DATE, None, &["散目录L\n"])
    );
}

/// c2：异常来源（符号链接/目录/非法 UTF-8/读取失败）跳过＋原因提示、不中止轮；
/// 提示不含规则正文。
#[test]
fn t04_anomalous_sources_skipped_with_notice_no_abort() {
    let cfg = Tmp::new("c2-anom-cfg");
    let proj = Tmp::new("c2-anom-proj");
    // 全局＝符号链接
    let real = cfg.path().join("real.md");
    std::fs::write(&real, "符号链接目标正文G\n").unwrap();
    std::os::unix::fs::symlink(&real, cfg.path().join("AGENTS.md")).unwrap();
    // 项目根＝目录（非普通文件）
    let root = proj.path().join("r");
    let sub = root.join("s");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(root.join("AGENTS.md")).unwrap();
    // cwd＝非法 UTF-8 内容
    std::fs::write(sub.join("AGENTS.md"), b"R\xff\xfeULE\n").unwrap();

    let built = build_context_message(&sources(cfg.path(), &sub, Some("/bin/sh"), T0));
    assert!(
        !built.message.contains("INSTRUCTIONS"),
        "全部来源异常→无 AGENTS 块：{}",
        built.message
    );
    assert_eq!(built.notices.len(), 3, "三个异常来源各一行提示：{:?}", built.notices);
    for n in &built.notices {
        assert!(!n.contains("符号链接目标正文G"), "提示不得含规则正文：{n}");
        assert!(!n.contains('R'), "提示不得含规则正文：{n}");
    }
    assert!(built.notices.iter().any(|n| n.contains("符号链接")));
    assert!(built.notices.iter().any(|n| n.contains("非普通文件")));
    assert!(built.notices.iter().any(|n| n.contains("UTF-8")));

    // 读取失败（chmod 000；测试进程非 root）：来源跳过且用户轮正常完成
    let cfg2 = Tmp::new("c2-anom-cfg2");
    let proj2 = Tmp::new("c2-anom-proj2");
    let unreadable = cfg2.path().join("AGENTS.md");
    std::fs::write(&unreadable, "不可读正文G\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    let built2 = build_context_message(&sources(cfg2.path(), proj2.path(), Some("/bin/sh"), T0));
    assert!(!built2.message.contains("INSTRUCTIONS"));
    assert!(
        built2.notices.iter().any(|n| n.contains("读取失败")),
        "读取失败有提示：{:?}",
        built2.notices
    );
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();

    let mut history = Vec::new();
    let src2 = sources(cfg2.path(), proj2.path(), Some("/bin/sh"), T0);
    let reply = run_user_turn_with_context(
        &mut history,
        "继续",
        &mut |_| Ok(answer("轮未中止")),
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &src2,
    &ProcessManager::new(),
    )
    .unwrap();
    assert_eq!(reply, "轮未中止");
    assert_eq!(history.len(), 3, "ctx（仅环境块）、user、assistant");
}

/// c3：同内容跨轮与轮内续请求都不重复注入。
#[test]
fn t05_same_content_not_reinjected() {
    let cfg = Tmp::new("c3-cfg");
    let proj = Tmp::new("c3-proj");
    std::fs::write(cfg.path().join("AGENTS.md"), "稳定规则S\n").unwrap();
    std::fs::create_dir_all(proj.path().join(".git")).unwrap();
    let src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);

    let mut history = Vec::new();
    let mut all_bodies = Vec::new();
    for input in ["第一问", "第二问"] {
        run_user_turn_with_context(
            &mut history,
            input,
            &mut |b| {
                all_bodies.push(b.clone());
                Ok(answer("答"))
            },
            &mut |_| {},
            None,
            &mut |_| {},
            ToolMode::User,
            &src,
        &ProcessManager::new(),
        )
        .unwrap();
    }
    let texts: Vec<&str> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m) if m.role == HistoryRole::User => Some(m.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ctx_marker_count(&texts), 1, "两轮后历史恰 1 条 ctx");
    for (i, body) in all_bodies.iter().enumerate() {
        let msgs = body["messages"].as_array().unwrap();
        let texts: Vec<&str> = msgs
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect();
        assert_eq!(ctx_marker_count(&texts), 1, "第 {} 次请求 ctx 恰一次", i + 1);
    }

    // 轮内工具续请求：ctx 仍恰一次（历史携带，不重注入）
    let mut bodies3 = Vec::new();
    run_user_turn_with_context(
        &mut history,
        "第三问",
        &mut |b| {
            bodies3.push(b.clone());
            if bodies3.len() == 1 {
                Ok(tool_reply("c3-1", "exec_command", &json!({ "cmd": "true" })))
            } else {
                Ok(answer("三轮答"))
            }
        },
        &mut |_| {},
        None,
        &mut |_| {},
        ToolMode::User,
        &src,
    &ProcessManager::new(),
    )
    .unwrap();
    assert_eq!(bodies3.len(), 2);
    let texts: Vec<&str> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m) if m.role == HistoryRole::User => Some(m.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ctx_marker_count(&texts), 1, "工具轮后仍恰 1 条 ctx");
    for body in &bodies3 {
        let msgs = body["messages"].as_array().unwrap();
        let texts: Vec<&str> = msgs.iter().filter_map(|m| m["content"].as_str()).collect();
        assert_eq!(ctx_marker_count(&texts), 1);
    }
}

/// c4：A→B→B——变化带替换声明、旧消息保留；再同内容不再注入。
#[test]
fn t06_content_change_replaces_with_declaration_abb() {
    let cfg = Tmp::new("c4-cfg");
    let proj = Tmp::new("c4-proj");
    std::fs::create_dir_all(proj.path().join(".git")).unwrap();
    let mut src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);

    let expected = |rules: &str| {
        rebuild_ctx(
            &proj.path().display().to_string(),
            Some("/bin/sh"),
            T0_DATE,
            Some(rules),
            &[],
        )
    };

    // 轮 1：A（裸消息，无声明）
    std::fs::write(cfg.path().join("AGENTS.md"), "版本A\n").unwrap();
    let mut history = Vec::new();
    run_user_turn_with_context(
        &mut history, "问一",
        &mut |_| Ok(answer("答一")), &mut |_| {}, None, &mut |_| {},
        ToolMode::User, &src,
    &ProcessManager::new(),
    ).unwrap();
    let ctx_a = expected("版本A\n");
    match &history[0] {
        HistoryItem::Text(m) => {
            assert_eq!(m.content, ctx_a);
            assert!(!m.content.contains(DECL), "首次注入无替换声明");
        }
        other => panic!("{other:?}"),
    }

    // 轮 2：文件改为 B → 带声明的新消息，旧 A 保留
    std::fs::write(cfg.path().join("AGENTS.md"), "版本B\n").unwrap();
    let mut bodies2 = Vec::new();
    run_user_turn_with_context(
        &mut history, "问二",
        &mut |b| { bodies2.push(b.clone()); Ok(answer("答二")) },
        &mut |_| {}, None, &mut |_| {},
        ToolMode::User, &src,
    &ProcessManager::new(),
    ).unwrap();
    let ctx_b = expected("版本B\n");
    let texts: Vec<&str> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m) if m.role == HistoryRole::User => Some(m.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ctx_marker_count(&texts), 2, "A 与 B(带声明) 两条");
    let msgs = bodies2[0]["messages"].as_array().unwrap();
    let texts: Vec<&str> = msgs.iter().filter_map(|m| m["content"].as_str()).collect();
    assert_eq!(ctx_marker_count(&texts), 2, "请求携带 A＋B");
    let declared_new = format!("{DECL}\n\n{ctx_b}");
    assert!(texts.contains(&declared_new.as_str()), "新消息＝声明＋空行＋B");

    // 轮 3：B 未变 → 不再注入
    src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);
    run_user_turn_with_context(
        &mut history, "问三",
        &mut |_| Ok(answer("答三")), &mut |_| {}, None, &mut |_| {},
        ToolMode::User, &src,
    &ProcessManager::new(),
    ).unwrap();
    let texts: Vec<&str> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m) if m.role == HistoryRole::User => Some(m.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ctx_marker_count(&texts), 2, "A→B→B 后仍恰 2 条 ctx");
}

/// c4：仿冒文本不认定为上下文（其后真实注入）；真实形态不同正文→带声明；
/// 带声明等正文→不注入。
#[test]
fn t07_lookalike_not_recognized_real_forms_recognized() {
    let cfg = Tmp::new("c4b-cfg");
    let proj = Tmp::new("c4b-proj");
    std::fs::write(cfg.path().join("AGENTS.md"), "现行规则N\n").unwrap();
    std::fs::create_dir_all(proj.path().join(".git")).unwrap();
    let src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);
    let ctx_now = rebuild_ctx(
        &proj.path().display().to_string(),
        Some("/bin/sh"),
        T0_DATE,
        Some("现行规则N\n"),
        &[],
    );

    // 仿冒 1：以标题开头、以 </INSTRUCTIONS> 结尾（无环境块）→ 不认定为上下文；
    // 其后注入裸消息（基线不存在）
    for lookalike in [
        format!(
            "# AGENTS.md instructions for {}\n\n<INSTRUCTIONS>\n用户粘贴的说明\n</INSTRUCTIONS>",
            proj.path().display()
        ),
        "<environment_context>\n  <cwd>普通用户输入开头</cwd>".to_string(),
    ] {
        let mut history = vec![user(&lookalike), assistant("收到")];
        run_user_turn_with_context(
            &mut history, "新问题",
            &mut |_| Ok(answer("答")), &mut |_| {}, None, &mut |_| {},
            ToolMode::User, &src,
        &ProcessManager::new(),
        )
        .unwrap();
        assert_eq!(history[2], HistoryItem::Text(HistoryMessage {
            role: HistoryRole::User,
            content: ctx_now.clone(),
        }), "仿冒不被认作基线：注入裸消息（无声明）：{lookalike}");
    }

    // 真实形态、正文不同（此前存在上下文）→ 新消息带声明
    let ctx_old = rebuild_ctx(
        &proj.path().display().to_string(),
        Some("/bin/sh"),
        T0_DATE,
        Some("旧正文O\n"),
        &[],
    );
    let mut history = vec![user(&ctx_old), assistant("旧答")];
    run_user_turn_with_context(
        &mut history, "新问题",
        &mut |_| Ok(answer("答")), &mut |_| {}, None, &mut |_| {},
        ToolMode::User, &src,
    &ProcessManager::new(),
    ).unwrap();
    assert_eq!(history[2], HistoryItem::Text(HistoryMessage {
        role: HistoryRole::User,
        content: format!("{DECL}\n\n{ctx_now}"),
    }), "真实旧上下文被识别：新消息带替换声明");

    // 带声明的等正文（恢复形态）→ 不注入
    let mut history = vec![
        user(&format!("{DECL}\n\n{ctx_now}")),
        user("上一问"),
        assistant("上一答"),
    ];
    run_user_turn_with_context(
        &mut history, "继续",
        &mut |_| Ok(answer("答")), &mut |_| {}, None, &mut |_| {},
        ToolMode::User, &src,
    &ProcessManager::new(),
    ).unwrap();
    let texts: Vec<&str> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m) if m.role == HistoryRole::User => Some(m.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ctx_marker_count(&texts), 1, "声明剥除后比较：等正文不注入");
}

/// c5：ctx 项经 serde 往返（007 行格式）恢复基线——不再注入；序列化形状零新增字段。
#[test]
fn t08_serde_roundtrip_restores_dedup_baseline() {
    let cfg = Tmp::new("c5-cfg");
    let proj = Tmp::new("c5-proj");
    std::fs::write(cfg.path().join("AGENTS.md"), "版本B\n").unwrap();
    std::fs::create_dir_all(proj.path().join(".git")).unwrap();
    let src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);
    let ctx_b = rebuild_ctx(
        &proj.path().display().to_string(),
        Some("/bin/sh"),
        T0_DATE,
        Some("版本B\n"),
        &[],
    );

    // 007 行格式＝serde tagged JSON：逐条序列化→解析回（/resume 装载路径）
    let original = vec![
        user(&format!("{DECL}\n\n{ctx_b}")),
        user("上一问"),
        assistant("上一答"),
    ];
    let mut history: Vec<HistoryItem> = original
        .iter()
        .map(|it| serde_json::from_str(&serde_json::to_string(it).unwrap()).unwrap())
        .collect();

    // ctx 项序列化形状：恰 type/role/content 三字段（无新增持久化状态）
    let ctx_json: Value =
        serde_json::from_str(&serde_json::to_string(&history[0]).unwrap()).unwrap();
    let mut keys: Vec<&str> = ctx_json.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["content", "role", "type"], "ctx＝普通 Text 项");

    let mut bodies = Vec::new();
    run_user_turn_with_context(
        &mut history, "恢复后首问",
        &mut |b| { bodies.push(b.clone()); Ok(answer("答")) },
        &mut |_| {}, None, &mut |_| {},
        ToolMode::User, &src,
    &ProcessManager::new(),
    ).unwrap();
    let texts: Vec<&str> = history
        .iter()
        .filter_map(|h| match h {
            HistoryItem::Text(m) if m.role == HistoryRole::User => Some(m.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ctx_marker_count(&texts), 1, "恢复基线生效：不重复注入");
    let msgs = bodies[0]["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["system", "user", "user", "assistant", "user"]);
    assert_eq!(msgs[2]["content"], json!("上一问"));
    assert_eq!(msgs[4]["content"], json!("恢复后首问"));
}

/// c6：预算截断——UTF-8 边界、包装完整、分隔计入、恰不超预算、截断提示、
/// 恰好占满后停止加载。
#[test]
fn t09_budget_truncation_utf8_boundary_and_exhaustion() {
    let cfg = Tmp::new("c6-cfg");
    let proj = Tmp::new("c6-proj");
    let root = proj.path().join("repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();

    // 独立 UTF-8 边界前缀
    let byte_prefix = |s: &str, max: usize| -> String {
        if s.len() <= max {
            s.to_string()
        } else {
            let mut end = max;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s[..end].to_string()
        }
    };
    let env_block = |cwd: &str| -> String {
        format!(
            "<environment_context>\n  <cwd>{}</cwd>\n  <shell>/bin/sh</shell>\n  <current_date>{T0_DATE}</current_date>\n</environment_context>",
            xml_escape(cwd)
        )
    };

    // a) 两 ASCII 文件合计超预算：分隔行计入，截为恰 32768 字节前缀
    std::fs::write(cfg.path().join("AGENTS.md"), "G".repeat(20_000)).unwrap();
    std::fs::write(root.join("AGENTS.md"), "P".repeat(20_000)).unwrap();
    let built = build_context_message(&sources(cfg.path(), &root, Some("/bin/sh"), T0));
    let cwd = root.display().to_string();
    let full = format!("{}\n\n--- project-doc ---\n\n{}", "G".repeat(20_000), "P".repeat(20_000));
    let expected_content = byte_prefix(&full, 32_768);
    assert_eq!(expected_content.len(), 32_768);
    assert_eq!(
        built.message,
        format!(
            "# AGENTS.md instructions for {cwd}\n\n<INSTRUCTIONS>\n{expected_content}\n</INSTRUCTIONS>\n\n{}",
            env_block(&cwd)
        )
    );
    assert!(
        built.notices.iter().any(|n| n.contains("32768")),
        "截断提示：{:?}",
        built.notices
    );

    // a2) 仅超 1 字节：保留恰 32768，末 1 字节丢弃，包装标签完整
    std::fs::write(cfg.path().join("AGENTS.md"), "H".repeat(32_769)).unwrap();
    let built = build_context_message(&sources(cfg.path(), &root, Some("/bin/sh"), T0));
    let cwd = root.display().to_string();
    let expected_content = "H".repeat(32_768);
    // 全局来源在项目来源之前占满预算并按 UTF-8 边界截断：无项目链内容进入，
    // 标题不带 ` for {cwd}` 后缀（015 §1）。
    assert_eq!(
        built.message,
        format!(
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>\n{expected_content}\n</INSTRUCTIONS>\n\n{}",
            env_block(&cwd)
        ),
        "超出上限 1 字节也必须完整截断且标签闭合"
    );
    assert!(
        !built.message.starts_with("# AGENTS.md instructions for "),
        "项目来源未进入正文时标题不得带项目后缀"
    );
    assert!(!built.message.contains('P'), "项目来源不得在预算耗尽后进入");
    assert!(
        built.notices.iter().any(|n| n.contains("32768")),
        "超 1 字节提示：{:?}",
        built.notices
    );
    // b) 截点落在多字节字符中间：按字符边界回退，无 U+FFFD
    std::fs::write(cfg.path().join("AGENTS.md"), format!("{}{}", "a".repeat(32_700), "汉".repeat(200))).unwrap();
    let built = build_context_message(&sources(cfg.path(), &root, Some("/bin/sh"), T0));
    let expected_content = format!("{}{}", "a".repeat(32_700), "汉".repeat(22));
    assert_eq!(expected_content.len(), 32_766, "边界回退到完整字符");
    assert!(built.message.contains(&format!("<INSTRUCTIONS>\n{expected_content}\n</INSTRUCTIONS>")));
    assert!(!built.message.contains('\u{fffd}'), "不得产生替换字符");

    // c) 全局恰好占满：content＝全局全文、项目文件不再进入、预算耗尽有提示
    std::fs::write(cfg.path().join("AGENTS.md"), "E".repeat(32_768)).unwrap();
    let built = build_context_message(&sources(cfg.path(), &root, Some("/bin/sh"), T0));
    assert!(built.message.contains(&format!("<INSTRUCTIONS>\n{}\n</INSTRUCTIONS>", "E".repeat(32_768))));
    assert!(!built.message.contains('P'), "预算耗尽：项目文件不再加载");
    assert!(
        built.notices.iter().any(|n| n.contains("32768")),
        "预算耗尽提示：{:?}",
        built.notices
    );

    // d) 耗尽后首个后续路径缺失、更后链上目录有普通来源：仍记耗尽提示
    //（终态审查发现补钉：缺失不得掩蔽更后来源的存在性）
    let cfg2 = Tmp::new("c6-cfg2");
    let proj2 = Tmp::new("c6-proj2");
    let root2 = proj2.path().join("repo2");
    let sub2 = root2.join("sub2");
    std::fs::create_dir_all(root2.join(".git")).unwrap();
    std::fs::create_dir_all(&sub2).unwrap();
    std::fs::write(cfg2.path().join("AGENTS.md"), "F".repeat(32_768)).unwrap();
    std::fs::write(sub2.join("AGENTS.md"), "后来者L\n").unwrap();
    let built = build_context_message(&sources(cfg2.path(), &sub2, Some("/bin/sh"), T0));
    assert!(
        built.message.contains(&format!("<INSTRUCTIONS>\n{}\n</INSTRUCTIONS>", "F".repeat(32_768))),
        "content＝恰好占满的全局"
    );
    assert!(!built.message.contains("后来者L"), "更后来源不加载");
    assert!(
        built.notices.iter().any(|n| n.contains("32768")),
        "首后续缺失不掩蔽更后来源：{:?}",
        built.notices
    );
}

/// c7：MemoryMerge 内部轮经新入口与直调逐字节一致——不读取不注入。
#[test]
fn t10_memory_merge_mode_injects_nothing() {
    let cfg = Tmp::new("c7-cfg");
    let proj = Tmp::new("c7-proj");
    std::fs::write(cfg.path().join("AGENTS.md"), "规则M\n").unwrap();
    std::fs::create_dir_all(proj.path().join(".git")).unwrap();
    let src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);
    let mem_root = Tmp::new("c7-mem");
    let mode = ToolMode::MemoryMerge {
        root: mem_root.path(),
        system: "merge-system",
        expected_epoch: 0,
    };

    let mut h1 = Vec::new();
    let mut b1 = Vec::new();
    run_tool_turn_with_mode(
        &mut h1, "合并",
        &mut |b| { b1.push(b.clone()); Ok(answer("直调答")) },
        &mut |_| {}, None, &mut |_| {}, mode,
    &ProcessManager::new(),
    ).unwrap();
    let mut h2 = Vec::new();
    let mut b2 = Vec::new();
    run_user_turn_with_context(
        &mut h2, "合并",
        &mut |b| { b2.push(b.clone()); Ok(answer("直调答")) },
        &mut |_| {}, None, &mut |_| {}, mode, &src,
    &ProcessManager::new(),
    ).unwrap();
    assert_eq!(
        serde_json::to_string(&b1[0]).unwrap(),
        serde_json::to_string(&b2[0]).unwrap(),
        "合并轮请求经新入口逐字节不变"
    );
    assert_eq!(h1, h2, "合并轮历史经新入口不变");
    assert!(!serde_json::to_string(&b2[0]).unwrap().contains("<INSTRUCTIONS>"));
}

/// c10：空白输入校验先于上下文写入——拒绝时历史与 on_record 零新增。
#[test]
fn t11_blank_input_rejected_before_context_write() {
    let cfg = Tmp::new("c10-cfg");
    let proj = Tmp::new("c10-proj");
    std::fs::write(cfg.path().join("AGENTS.md"), "规则W\n").unwrap();
    let src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);
    for blank in ["", "  \n\t", "\u{3000}"] {
        let mut history = Vec::new();
        let mut recorded = 0usize;
        let err = run_user_turn_with_context(
            &mut history, blank,
            &mut |_| Ok(answer("不应到达")), &mut |_| {}, None,
            &mut |_| recorded += 1,
            ToolMode::User, &src,
        &ProcessManager::new(),
        )
        .unwrap_err();
        assert!(err.contains("空白"), "{blank:?} 应拒绝：{err}");
        assert!(history.is_empty(), "拒绝路径不得写入任何历史项");
        assert_eq!(recorded, 0, "拒绝路径不得触发 on_record");
    }
}

/// c10：与工具历史共存（顺序/原文不变）；UserWithMemory 同样注入（D4 尾段不受影响）。
#[test]
fn t12_coexists_with_tool_history_and_user_with_memory() {
    let cfg = Tmp::new("c10b-cfg");
    let proj = Tmp::new("c10b-proj");
    std::fs::write(cfg.path().join("AGENTS.md"), "规则X\n").unwrap();
    let src = sources(cfg.path(), proj.path(), Some("/bin/sh"), T0);

    let seeded = vec![
        user("旧问"),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: "call-1".into(),
                name: "exec_command".into(),
                arguments: r#"{"cmd":"true"}"#.into(),
            }],
            content: None,
        },
        HistoryItem::ToolResult {
            tool_call_id: "call-1".into(),
            content: r#"{"status":"completed"}"#.into(),
            display: None,
        },
    ];
    let mut history = seeded.clone();
    let mut bodies = Vec::new();
    run_user_turn_with_context(
        &mut history, "新问",
        &mut |b| { bodies.push(b.clone()); Ok(answer("答")) },
        &mut |_| {}, None, &mut |_| {},
        ToolMode::User, &src,
    &ProcessManager::new(),
    ).unwrap();
    let msgs = bodies[0]["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(
        roles,
        vec!["system", "user", "assistant", "tool", "user", "user"],
        "种子历史原样投影，ctx 恰在本轮输入之前"
    );
    assert_eq!(msgs[1]["content"], json!("旧问"));
    assert_eq!(msgs[2]["tool_calls"][0]["id"], json!("call-1"));
    assert_eq!(msgs[3]["tool_call_id"], json!("call-1"));
    assert!(
        msgs[4]["content"].as_str().unwrap().contains("<INSTRUCTIONS>"),
        "ctx 紧邻本轮输入之前"
    );
    assert_eq!(msgs[5]["content"], json!("新问"));

    // UserWithMemory（空记忆根→无 D4 尾段）：ctx 照常注入，system＝发送版提示词
    let mem_root = Tmp::new("c10b-mem");
    let config = MemoriesConfig::default();
    let mut history2 = Vec::new();
    let mut bodies2 = Vec::new();
    run_user_turn_with_context(
        &mut history2, "带记忆模式的问",
        &mut |b| { bodies2.push(b.clone()); Ok(answer("答")) },
        &mut |_| {}, None, &mut |_| {},
        ToolMode::UserWithMemory { root: mem_root.path(), config: &config },
        &src,
    &ProcessManager::new(),
    ).unwrap();
    let msgs = bodies2[0]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3, "system → ctx → 输入");
    assert!(
        msgs[1]["content"].as_str().unwrap().contains("<INSTRUCTIONS>"),
        "UserWithMemory 同样注入 ctx"
    );
    assert_eq!(
        msgs[0]["content"],
        json!(terminal_chat::prompt::sent_instructions()),
        "无记忆摘要时 system 保持发送版（D4 契约不变）"
    );
}

/// c1 附：cwd/SHELL 的 XML 文本转义（环境块内转义；标题行保持原文）。
#[test]
fn t13_xml_escaping_cwd_and_shell() {
    let proj = Tmp::new("c13");
    let odd = proj.path().join("d&anger<l>");
    std::fs::create_dir_all(&odd).unwrap();
    // 无规则：仅环境块，值转义
    let built = build_context_message(&sources(proj.path(), &odd, Some("sh<&>tag"), T0));
    let raw = odd.display().to_string();
    assert!(built.message.contains(&format!("  <cwd>{}</cwd>\n", xml_escape(&raw))));
    assert!(built.message.contains("  <shell>sh&lt;&amp;&gt;tag</shell>\n"));
    assert!(!built.message.contains(&format!("<cwd>{raw}</cwd>")));
    // 有项目规则：标题行用原始路径（非 XML 上下文）
    std::fs::write(odd.join("AGENTS.md"), "规则T\n").unwrap();
    let built = build_context_message(&sources(proj.path(), &odd, Some("/bin/sh"), T0));
    assert!(built.message.contains(&format!("# AGENTS.md instructions for {raw}\n\n")));
}

/// 识别器边界：`context_body` 剥至多一条声明；空白与残缺文本不识别。
#[test]
fn t14_context_body_recognition_edges() {
    let ctx = rebuild_ctx("/w", Some("/bin/sh"), T0_DATE, Some("R\n"), &[]);
    assert_eq!(context_body(&ctx), Some(ctx.as_str()));
    let declared = format!("{DECL}\n\n{ctx}");
    assert_eq!(context_body(&declared), Some(ctx.as_str()), "剥除一条声明");
    // 两条声明只剥一条：剩余声明使其不再匹配结构
    let double = format!("{DECL}\n\n{declared}");
    assert_eq!(context_body(&double), None, "至多剥一条");
    assert_eq!(context_body("普通用户文本"), None);
    assert_eq!(context_body(""), None);
    assert_eq!(context_body("# AGENTS.md instructions for /w\n用户粘贴"), None);
    assert_eq!(context_body("<environment_context>\n开头但无闭合"), None);
}

/// 异常来源与有效规则混合时，实际请求只发送有效规则；记忆摘要仍可共存且不入史。
#[test]
fn t15_skipped_sources_never_enter_requests_and_valid_rules_coexist_with_memory() {
    let cfg = Tmp::new("c15-cfg");
    let proj = Tmp::new("c15-proj");
    let outside = Tmp::new("c15-outside");
    let mem = Tmp::new("c15-mem");
    let root = proj.path().join("repo");
    let cwd = root.join("sub");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let linked = outside.path().join("rules.md");
    std::fs::write(&linked, "OUTSIDE-RULES-MUST-NOT-SEND").unwrap();
    std::os::unix::fs::symlink(&linked, cfg.path().join("AGENTS.md")).unwrap();
    std::fs::write(root.join("AGENTS.md"), "VALID-PROJECT-RULE\n").unwrap();
    std::fs::write(cwd.join("AGENTS.md"), b"INVALID-UTF8-RULE\xff").unwrap();
    std::fs::write(mem.path().join("memory_summary.md"), "v1\nMEMORY-REFERENCE").unwrap();
    let config = MemoriesConfig { enabled: true, ..Default::default() };
    let mut history = Vec::new();
    let mut requests = Vec::new();
    let mut notices = Vec::new();
    let reply = run_user_turn_with_context(
        &mut history,
        "继续当前任务",
        &mut |body| { requests.push(body.clone()); Ok(answer("完成")) },
        &mut |event| {
            if let TurnNotify::Info(notice) = event {
                notices.push(notice);
            }
        },
        None,
        &mut |_| {},
        ToolMode::UserWithMemory { root: mem.path(), config: &config },
        &sources(cfg.path(), &cwd, Some("/bin/sh"), T0),
    &ProcessManager::new(),
    ).unwrap();

    assert_eq!(reply, "完成");
    assert_eq!(requests.len(), 1, "异常来源不阻止当前请求");
    let expected = rebuild_ctx(
        &cwd.display().to_string(), Some("/bin/sh"), T0_DATE, None, &["VALID-PROJECT-RULE\n"],
    );
    let messages = requests[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "system、ctx、真实用户输入");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1], json!({"role":"user", "content":expected}));
    assert_eq!(messages[2], json!({"role":"user", "content":"继续当前任务"}));
    let sent = serde_json::to_string(&requests[0]).unwrap();
    assert_eq!(sent.matches("VALID-PROJECT-RULE").count(), 1);
    assert_eq!(sent.matches("MEMORY-REFERENCE").count(), 1);
    assert!(!sent.contains("OUTSIDE-RULES-MUST-NOT-SEND"));
    assert!(!sent.contains("INVALID-UTF8-RULE"));
    assert_eq!(history, vec![user(&expected), user("继续当前任务"), assistant("完成")]);
    assert_eq!(notices.len(), 2);
    assert!(notices.iter().any(|notice| notice.contains("符号链接")));
    assert!(notices.iter().any(|notice| notice.contains("UTF-8")));
    for notice in notices {
        assert!(!notice.contains("OUTSIDE-RULES-MUST-NOT-SEND"));
        assert!(!notice.contains("INVALID-UTF8-RULE"));
        assert!(!notice.contains("VALID-PROJECT-RULE"));
        assert!(!notice.contains("MEMORY-REFERENCE"));
    }
}
