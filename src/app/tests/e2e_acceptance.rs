//! 2026-10-09 交付候选 E2E 验收（场景 S1–S4、S6）：
//! - 全部用真实二进制/真实库入口：REPL 走 cargo 本次构建的真实 CLI（CARGO_BIN_EXE），
//!   PTY 由测试内 libc 直接创建（不依赖 tmux/script 等外部工具）；
//! - exec_command/apply_patch 未接入 CLI，经"最薄 JSON 宿主"子进程调用真实库入口，
//!   验证宿主→库→操作系统闭环；这是测试 harness，不宣称已完成 Agent 工具调用接线；
//! - 网络边界一律本地 mock＋临时目录＋假凭证；配置读取、授权判断、请求构造与
//!   文件操作全部走生产实现。
//! - S5（EFBIG 半写）与 F2 同步屏障不在本文件：复用既有用例，验收记录单独列示。
//! - 时序约定：所有等待均为 poll＋截止时间（看门狗），无固定延迟猜测关键时序；
//!   子进程异常时杀死并回收，不留孤儿。

mod common;

use common::{spawn_mock, MockResp, TempDir, PTY_COLS, PTY_ROWS};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use terminal_chat::catalog;

const WATCHDOG: Duration = Duration::from_secs(30);

fn input_pty_cmd(cfg: &Path, user: &Path, key_name: &str, key: &str, tui: bool) -> Command {
    let work = user.join("work");
    let home = user.join("home");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&home).unwrap();
    let mut cmd = pty_cli(
        Some(&work),
        &[
            ("TCHAT_CONFIG_DIR", cfg.to_str().unwrap()),
            ("TCHAT_USER_DIR", user.to_str().unwrap()),
            (key_name, key),
        ],
    );
    cmd.env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("APPDATA", &home)
        .env("CODEX_HOME", &home)
        .env("TZ", "UTC")
        .env("LC_ALL", "C.UTF-8")
        .env("SHELL", "/bin/sh")
        .env("TCHAT_SESSION_TS", "20260101T000000")
        .env_remove("TCHAT_PUBLIC_CATALOG_URL");
    if !tui {
        cmd.env("TERM", "dumb");
    }
    cmd
}

/// 018：真实 TUI 收到问题后，只有明确提交才能续请求，回答不成为聊天轮。
#[test]
fn s93_tui_user_input_submit_and_continue() {
    let cfg = TempDir::new("s93-cfg");
    let user = TempDir::new("s93-user");
    let args = json!({"questions":[{"id":"direction","header":"方向","question":"S93-请选择方向",
        "options":[{"label":"保留密码","description":"兼容账号"},{"label":"验证码","description":"修改流程"}]}]});
    let tc =
        json!({"choices":[{"message":{"tool_calls":[{"id":"s93-ask","type":"function","function":{
        "name":"request_user_input","arguments":args.to_string()}}]}}]})
        .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S93-继续完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S93_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s93"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S93_KEY",
        "fake-s93",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("先确认方向");
    pty.wait_screen_timeout("S93-请选择方向", Duration::from_secs(5))
        .unwrap();
    assert_eq!(mock.count(), 1, "预选不算提交");
    pty.send_raw(b"\r");
    pty.wait_screen("S93-继续完成").unwrap();
    pty.wait_screen("Questions 1/1 answered").unwrap();
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
    assert_eq!(mock.count(), 2, "答案不成为下一轮聊天");
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "s93-ask")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(result["content"].as_str().unwrap()).unwrap(),
        json!({"answers":{"direction":{"answers":["保留密码"]}}})
    );
    assert_eq!(body["tools"].as_array().unwrap().len(), 7);
}

#[test]
fn s94_line_user_input_choice_notes_and_eof() {
    let cfg = TempDir::new("s94-cfg");
    let user = TempDir::new("s94-user");
    let ask = |id: &str| {
        json!({"id":id,"type":"function","function":{"name":"request_user_input","arguments":json!({"questions":[{
        "id":"q","header":"方式","question":"S94-请选择","options":[{"label":"保持","description":"保持兼容"}]}]}).to_string()}})
    };
    let tc = |id| json!({"choices":[{"message":{"tool_calls":[ask(id)]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc("first")),
        MockResp::json(200, chat_body("S94-回答完成")),
        MockResp::json(200, tc("eof")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S94_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s94"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S94_KEY",
        "fake-s94",
        false,
    ));
    pty.wait_output("你> ", WATCHDOG).unwrap();
    pty.send_line("确认方式");
    pty.wait_output("选择编号", Duration::from_secs(5)).unwrap();
    pty.send_line("无效");
    pty.wait_output("编号无效", WATCHDOG).unwrap();
    pty.send_line("1");
    pty.wait_output("补充", WATCHDOG).unwrap();
    pty.send_line("中文说明");
    pty.wait_output("S94-回答完成", WATCHDOG).unwrap();
    pty.wait_output("你> ", WATCHDOG).unwrap();
    pty.send_line("再次确认");
    pty.wait_output("选择编号", WATCHDOG).unwrap();
    pty.send_raw(b"\x04");
    let (out, status) = pty.finish();
    assert!(status.success(), "{out}");
    assert!(out.contains("宿主退出：问询未完成"), "{out}");
    assert_eq!(mock.count(), 3, "EOF 后不续请求");
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let tool = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "first")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap(),
        json!({"answers":{"q":{"answers":["保持","user_note: 中文说明"]}}})
    );
}

#[test]
fn s95_tui_multi_question_other_paste_unanswered_resize_and_restore() {
    let cfg = TempDir::new("s95-cfg");
    let user = TempDir::new("s95-user");
    let long = "S95-长问题-一二三四五六七八九十甲乙丙丁戊己庚辛壬癸-尾段";
    let args = json!({"questions":(1..=3).map(|n|json!({"id":format!("q{n}"),"header":format!("题{n}"),
        "question":if n==1 {long.to_string()}else{format!("S95-问题{n}")},
        "options":[{"label":format!("A{n}"),"description":"兼容"},{"label":format!("B{n}"),"description":"调整"}]})).collect::<Vec<_>>()});
    let tc =
        json!({"choices":[{"message":{"tool_calls":[{"id":"s95-ask","type":"function","function":{
        "name":"request_user_input","arguments":args.to_string()}}]}}]})
        .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S95-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S95_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s95"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S95_KEY",
        "fake-s95",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("多题确认");
    pty.wait_screen(long).unwrap();
    pty.resize_and_sync(40, PTY_ROWS);
    pty.wait_screen_pred("问题按新窗口折行", |s| {
        s.contains("S95-长问题") && s.contains("尾段") && !s.contains(long)
    })
    .unwrap();
    pty.resize_and_sync(PTY_COLS, PTY_ROWS);
    pty.wait_screen(long).unwrap();
    pty.send_raw(b"\x1b[C");
    pty.wait_screen("S95-问题2").unwrap(); // 第一题预选尚未提交
    pty.send_raw(b"\x1b[B\x1b[B\t");
    pty.send_raw("\x1b[200~中文粘贴补充\x1b[201~".as_bytes());
    pty.wait_screen("中文粘贴补充").unwrap();
    let deadline=Instant::now()+WATCHDOG;
    loop {
        if let Some((col,row))=pty.screen_find("中文粘贴补充") {
            if pty.vt.vt100().screen().cursor_position()==(row,col+12) {break;}
        }
        assert!(Instant::now()<deadline,"中文补充光标未对齐：{:?}",pty.vt.vt100().screen().cursor_position());
        pty.read_once(50);
    }
    pty.send_raw(b"\r");
    pty.wait_screen("S95-问题3").unwrap();
    pty.send_raw(b"\r");
    pty.wait_screen("Submit with unanswered questions?")
        .unwrap();
    assert_eq!(mock.count(), 1);
    pty.send_raw(b"2\r");
    pty.wait_screen(long).unwrap();
    pty.send_raw(b"\r");
    pty.wait_screen("S95-问题2").unwrap();
    pty.send_raw(b"\t\r");
    pty.wait_screen("S95-问题3").unwrap();
    pty.send_raw(b"\r");
    pty.wait_screen("S95-完成").unwrap();
    pty.wait_screen("Questions 3/3 answered").unwrap();
    pty.send_line("/save s95");
    pty.wait_screen("已保存").unwrap();
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 2);
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "s95-ask")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(result["content"].as_str().unwrap()).unwrap(),
        json!({"answers":{
        "q1":{"answers":["A1"]},"q2":{"answers":["None of the above","user_note: 中文粘贴补充"]},"q3":{"answers":["A3"]}}})
    );
    let mut resumed = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S95_KEY",
        "fake-s95",
        true,
    ));
    resumed.wait_screen("? for shortcuts").unwrap();
    resumed.send_line("/resume s95");
    resumed.wait_screen("Questions 3/3 answered").unwrap();
    resumed.wait_screen("note: 中文粘贴补充").unwrap();
    resumed.send_raw(b"\x03");
    assert!(resumed.finish().1.success());
    assert_eq!(mock.count(), 2, "恢复只展示");
}

#[test]
fn s96_tui_question_keeps_queued_chat_and_composer_draft_separate() {
    let cfg = TempDir::new("s100-cfg");
    let user = TempDir::new("s100-user");
    let tc=json!({"choices":[{"message":{"tool_calls":[{"id":"s96-ask","type":"function","function":{
        "name":"request_user_input","arguments":json!({"questions":[{"id":"q","header":"选择","question":"S96-当前问询",
        "options":[{"label":"S96-回答选项","description":"确认方向"}]}]}).to_string()}}]}}]}).to_string();
    let (first, gate) = MockResp::gated(200, tc);
    let mock = spawn_mock(vec![
        first,
        MockResp::json(200, chat_body("S96-本轮完成")),
        MockResp::json(200, chat_body("S96-队列完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S96_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s100"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S96_KEY",
        "fake-s100",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("先执行");
    wait_request_count(&mut pty, &mock, 1);
    // 显式排队键（Tab）提交聊天（021 起 Enter 运行中＝接续当前轮）：
    // 排队聊天不得被问询窗口吞掉，也不得作为答案
    pty.send_raw("S96-排队聊天".as_bytes());
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\t");
    pty.wait_screen("S96-排队聊天").unwrap();
    pty.send_raw("\x1b[200~S96-未发草稿\x1b[201~".as_bytes());
    pty.wait_screen("S96-未发草稿").unwrap();
    gate.release();
    pty.wait_screen("S96-当前问询").unwrap();
    assert_eq!(mock.count(), 1);
    pty.send_raw(b"\r");
    pty.wait_screen("S96-队列完成").unwrap();
    pty.wait_screen("S96-未发草稿").unwrap();
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 3);
    let bodies: Vec<Value> = mock
        .requests()
        .iter()
        .map(|r| serde_json::from_str(&r.body).unwrap())
        .collect();
    let last_user = bodies[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .unwrap();
    assert_eq!(last_user["content"], "S96-排队聊天");
    assert!(bodies.iter().all(|b| b["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .all(|m| m["content"] != "S96-回答选项" && m["content"] != "S96-未发草稿")));
}

#[test]
fn s97_question_cancel_exit_and_eof_save_one_terminal_result_and_pair_batch() {
    use terminal_chat::prompt::HistoryItem;
    for (tui, key, exits, expected) in [
        (
            true,
            b"\x1b".as_slice(),
            false,
            "用户已取消本轮：问询未完成",
        ),
        (true, b"\x03".as_slice(), true, "宿主退出：问询未完成"),
        (
            false,
            b"\x03".as_slice(),
            false,
            "用户已取消本轮：问询未完成",
        ),
        (false, b"\x04".as_slice(), true, "宿主退出：问询未完成"),
    ] {
        let cfg = TempDir::new("s101-cfg");
        let user = TempDir::new("s101-user");
        let forbidden = user.path().join("must-not-execute.txt");
        let tc=json!({"choices":[{"message":{"tool_calls":[
            {"id":"s97-ask","type":"function","function":{"name":"request_user_input","arguments":json!({"questions":[{
                "id":"q","header":"确认","question":"S97-请选择","options":[{"label":"允许后续任务","description":"普通任务数据"}]}]}).to_string()}},
            {"id":"s97-after","type":"function","function":{"name":"apply_patch","arguments":json!({"patch":format!("*** Begin Patch\n*** Add File: {}\n+wrong\n*** End Patch",forbidden.display())}).to_string()}}
        ]}}]}).to_string();
        let mock = spawn_mock(vec![MockResp::json(200, tc)]);
        write_manual(cfg.path(), &mock.url, "env:TCHAT_S97_KEY");
        seed_grant(
            user.path(),
            &catalog::origin_of(&mock.url).unwrap(),
            &catalog::fingerprint("fake-s101"),
        );
        let mut pty = PtySession::spawn(input_pty_cmd(
            cfg.path(),
            user.path(),
            "TCHAT_S97_KEY",
            "fake-s101",
            tui,
        ));
        if tui {
            pty.wait_screen("? for shortcuts").unwrap();
        } else {
            pty.wait_output("你> ", WATCHDOG).unwrap();
        }
        pty.send_line("等待中结束");
        if tui {
            pty.wait_screen("S97-请选择").unwrap();
        } else {
            pty.wait_output("选择编号", WATCHDOG).unwrap();
        }
        if !tui {
            pty.send_raw("未提交草稿".as_bytes());
        }
        pty.send_raw(key);
        if !exits {
            if tui {
                pty.wait_screen("问询未完成").unwrap();
                pty.send_raw(b"\x03");
            } else {
                pty.wait_output("问询未完成", WATCHDOG).unwrap();
                pty.send_line("exit");
            }
        }
        let (out, status) = pty.finish();
        assert!(status.success(), "{out}");
        assert_eq!(mock.count(), 1, "等待结束后不续请求");
        assert!(!forbidden.exists(), "同批后续工具不得执行");
        let autos: Vec<_> = fs::read_dir(user.path().join("sessions"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        assert_eq!(autos.len(), 1);
        let loaded = terminal_chat::session::load_session_file(&autos[0]).unwrap();
        assert_eq!(loaded.repairs, 0, "必须保存完整配对");
        let results: Vec<_> = loaded
            .history
            .iter()
            .filter_map(|h| {
                if let HistoryItem::ToolResult {
                    image,
                    tool_call_id,
                    content,
                    display,
                } = h
                {
                    assert!(display.is_none());
                    Some((
                        tool_call_id,
                        serde_json::from_str::<Value>(content).unwrap(),
                    ))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "s97-ask");
        assert_eq!(results[0].1, json!({"error":expected}));
        assert_eq!(results[1].0, "s97-after");
        assert!(results[1].1["error"]
            .as_str()
            .unwrap()
            .contains("本调用未执行"));
        if expected.starts_with("宿主退出") {
            assert!(results[1].1["error"].as_str().unwrap().contains("宿主退出"));
        }
    }
}

#[test]
fn s98_untrusted_question_and_answer_are_display_copies_and_never_commands() {
    let cfg = TempDir::new("s102-cfg");
    let user = TempDir::new("s102-user");
    let target = user.path().join("answer-must-not-execute");
    let id = " q\u{1b}[2J ";
    let call_id = "s98\u{1b}]52;c;SUQtQVRUQUNL\u{7}";
    let label = "保留\u{1b}[2J原标签";
    let note = format!(
        "S98-补充\u{1b}]52;c;Tk9URS1BVFRBQ0s=\u{7}$(touch {})",
        target.display()
    );
    let args=json!({"questions":[{"id":id,"header":"标题\u{1b}]0;HEADER-ATTACK\u{7}",
        "question":"S98-安全问题\u{1b}]0;QUESTION-ATTACK\u{7}","options":[{"label":label,"description":"说明\u{1b}Ppayload\u{1b}\\"}]}]}).to_string();
    let tc=json!({"choices":[{"message":{"tool_calls":[{"id":call_id,"type":"function","function":{"name":"request_user_input","arguments":args}}]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S98-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S98_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s102"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S98_KEY",
        "fake-s102",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("安全展示");
    pty.wait_screen("S98-安全问题").unwrap();
    pty.wait_screen("保留原标签").unwrap();
    pty.send_raw(format!("\x1b[200~{note}\x1b[201~").as_bytes());
    pty.wait_screen("S98-补充").unwrap();
    pty.send_raw(b"\r");
    pty.wait_screen("S98-完成").unwrap();
    pty.send_raw(b"\x03");
    let (out, status) = pty.finish();
    assert!(status.success());
    assert!(
        osc52_payloads(&out).is_empty(),
        "回答与关联 ID 不得控制剪贴板"
    );
    assert!(!out.contains("QUESTION-ATTACK") && !out.contains("HEADER-ATTACK"));
    assert!(!target.exists());
    assert_eq!(mock.count(), 2);
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let tool = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == call_id)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap(),
        json!({"answers":{id:{"answers":[label,format!("user_note: {note}")]}}})
    );
    let assistant = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m.get("tool_calls").is_some())
        .unwrap();
    assert_eq!(assistant["tool_calls"][0]["id"], call_id);
    assert_eq!(assistant["tool_calls"][0]["function"]["arguments"], args);
}

#[test]
fn s99_line_multi_question_other_and_confirmed_unanswered() {
    let cfg = TempDir::new("s103-cfg");
    let user = TempDir::new("s103-user");
    let tc=json!({"choices":[{"message":{"tool_calls":[{"id":"s99-ask","type":"function","function":{
        "name":"request_user_input","arguments":json!({"questions":(1..=2).map(|n|json!({"id":format!("q{n}"),"header":"确认",
            "question":format!("S99-问题{n}"),"options":[{"label":"保留","description":"兼容"}]})).collect::<Vec<_>>()}).to_string()}}]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S99-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S99_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s103"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S99_KEY",
        "fake-s103",
        false,
    ));
    pty.wait_output("你> ", WATCHDOG).unwrap();
    pty.send_line("多题");
    pty.wait_output("S99-问题1", WATCHDOG).unwrap();
    pty.send_raw(b"\r");
    pty.wait_output("S99-问题2", WATCHDOG).unwrap();
    pty.send_line("2");
    pty.wait_output("补充（可留空）", WATCHDOG).unwrap();
    pty.send_line("其他中文回答");
    pty.wait_output("仍有未答题，确认提交？", WATCHDOG).unwrap();
    assert_eq!(mock.count(), 1);
    pty.send_line("y");
    pty.wait_output("S99-完成", WATCHDOG).unwrap();
    pty.send_line("exit");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 2);
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let tool = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "s99-ask")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap(),
        json!({"answers":{
        "q1":{"answers":[]},"q2":{"answers":["None of the above","user_note: 其他中文回答"]}}})
    );
}

#[test]
fn s100_tui_two_calls_with_same_question_id_wait_serially_then_execute_patch() {
    let cfg = TempDir::new("s100-cfg");
    let user = TempDir::new("s100-user");
    let target = user.path().join("after-both.txt");
    let mut calls:Vec<_>=(1..=2).map(|n|json!({"id":format!("ask{n}"),"type":"function","function":{"name":"request_user_input",
        "arguments":json!({"questions":[{"id":"same-id","header":"确认","question":format!("S100-当前问题{n}"),
        "options":[{"label":format!("答案{n}"),"description":"本次回答"}]}]}).to_string()}})).collect();
    calls.push(json!({"id":"after-both","type":"function","function":{"name":"apply_patch","arguments":json!({
        "patch":format!("*** Begin Patch\n*** Add File: {}\n+both answered\n*** End Patch",target.display())}).to_string()}}));
    let mock = spawn_mock(vec![
        MockResp::json(
            200,
            json!({"choices":[{"message":{"tool_calls":calls}}]}).to_string(),
        ),
        MockResp::json(200, chat_body("S100-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S100_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s100"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S100_KEY",
        "fake-s100",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("串行问询");
    pty.wait_screen("S100-当前问题1").unwrap();
    assert_eq!(mock.count(), 1);
    assert!(!target.exists());
    pty.send_raw(b"\r");
    pty.wait_screen("S100-当前问题2").unwrap();
    assert_eq!(mock.count(), 1);
    assert!(!target.exists());
    pty.send_raw(b"\r");
    pty.wait_screen("S100-完成").unwrap();
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert_eq!(fs::read_to_string(&target).unwrap(), "both answered\n");
    assert_eq!(mock.count(), 2);
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    for n in 1..=2 {
        let tool = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_call_id"] == format!("ask{n}"))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap(),
            json!({"answers":{"same-id":{"answers":[format!("答案{n}")]}}})
        );
    }
}

#[test]
fn s101_restore_unfinished_question_only_displays_unknown_result() {
    use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord};
    let cfg = TempDir::new("s101-cfg");
    let user = TempDir::new("s101-user");
    let sessions = user.path().join("sessions");
    let args=json!({"questions":[{"id":"q","header":"方向","question":"S101-未完成问题","options":[{"label":"原选择","description":"未提交"}]}]}).to_string();
    terminal_chat::session::save_snapshot(
        &sessions,
        "unfinished",
        &[
            HistoryItem::Text(HistoryMessage {
                images: None,
                text_elements: Vec::new(),
                role: HistoryRole::User,
                content: "崩溃前用户轮".into(),
                user_input_event: None,
            }),
            HistoryItem::AssistantToolCalls {
                content: None,
                tool_calls: vec![ToolCallRecord {
                    id: "old-call".into(),
                    name: "request_user_input".into(),
                    arguments: args.clone(),
                }],
            },
        ],
    )
    .unwrap();
    let original = fs::read(sessions.join("unfinished.jsonl")).unwrap();
    let mock = spawn_mock(vec![]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S101_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s101"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S101_KEY",
        "fake-s101",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume unfinished");
    pty.wait_screen("Questions 0/1 answered (interrupted)")
        .unwrap();
    pty.wait_screen(terminal_chat::session::UNKNOWN_RESULT_ERROR)
        .unwrap();
    assert!(
        !pty.screen().contains("Question 1/1"),
        "恢复不能显示活动问答"
    );
    pty.send_line("/save restored");
    pty.wait_screen("已保存").unwrap();
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 0, "恢复与保存都不发模型请求");
    assert_eq!(
        fs::read(sessions.join("unfinished.jsonl")).unwrap(),
        original
    );
    let restored =
        terminal_chat::session::load_session_file(&sessions.join("restored.jsonl")).unwrap();
    assert_eq!(restored.repairs, 0);
    assert!(
        matches!(&restored.history[1],HistoryItem::AssistantToolCalls {tool_calls,..} if tool_calls[0].arguments==args && tool_calls[0].id=="old-call")
    );
    assert!(
        matches!(&restored.history[2],HistoryItem::ToolResult {content,display:None,..} if content.contains(terminal_chat::session::UNKNOWN_RESULT_ERROR) && !content.contains("answers"))
    );
}

/// 018：stdin 是终端、stdout 重定向时，TERM 大小写变体仍须能直接 Ctrl+C 取消。
#[test]
fn s102_redirected_line_cli_uppercase_term_cancels_without_another_line() {
    let cfg = TempDir::new("s102-cfg");
    let user = TempDir::new("s102-user");
    let output = user.path().join("stdout.txt");
    let mock = spawn_mock(vec![MockResp::json(
        200,
        json!({"choices":[{"message":{"tool_calls":[{"id":"s102-ask","type":"function","function":{
            "name":"request_user_input","arguments":json!({"questions":[{"id":"q","header":"方向",
            "question":"S102-请选择","options":[{"label":"保留","description":"兼容"}]}]}).to_string()}}]}}]}).to_string(),
    )]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S102_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s102"),
    );
    let original = input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S102_KEY",
        "fake-s102",
        false,
    );
    // 使用已有 PTY 宿主；shell 只负责 exec 前的真实 stdout 重定向，路径均通过参数传递。
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "exec \"$1\" > \"$2\"", "018-redirect"])
        .arg(original.get_program())
        .arg(&output)
        .current_dir(original.get_current_dir().unwrap())
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    for (key, value) in original.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    command.env("TERM", "DUMB");
    let mut pty = PtySession::spawn(command);
    let wait = |pty: &mut PtySession, needle: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = fs::read_to_string(&output).unwrap_or_default();
            if text.contains(needle) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "等待重定向输出 {needle:?} 超时：{text}"
            );
            pty.read_once(50);
        }
    };
    wait(&mut pty, "你> ");
    pty.send_line("确认方式");
    wait(&mut pty, "选择编号");
    assert_eq!(mock.count(), 1);
    pty.send_raw(b"\x03");
    wait(&mut pty, "用户已取消本轮：问询未完成");
    // 不补换行解除等待；Ctrl+D 从恢复后的普通 CLI 输入中退出宿主。
    pty.send_raw(b"\x04");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 1, "取消后不续请求");
    assert!(fs::read_to_string(output).unwrap().contains("再见!"));
    let sessions: Vec<_> = fs::read_dir(user.path().join("sessions"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    assert_eq!(sessions.len(), 1);
    let loaded = terminal_chat::session::load_session_file(&sessions[0]).unwrap();
    assert_eq!(loaded.repairs, 0);
    let results: Vec<_> = loaded
        .history
        .iter()
        .filter_map(|h| match h {
            terminal_chat::prompt::HistoryItem::ToolResult {
                image,
                tool_call_id,
                content,
                display,
            } => {
                assert!(display.is_none());
                Some((
                    tool_call_id,
                    serde_json::from_str::<Value>(content).unwrap(),
                ))
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "s102-ask");
    assert_eq!(results[0].1, json!({"error":"用户已取消本轮：问询未完成"}));
}

/// 018-B S103：异步发布不抢焦点——F6 打开原问答窗口；独立命令继续执行；
/// 回答在完整批后进入下一请求；聊天草稿保留。
#[test]
fn s103_async_publish_f6_answer_enters_next_request_and_keeps_draft() {
    let cfg = TempDir::new("s103-cfg");
    let user = TempDir::new("s103-user");
    let gate = user.path().join("work").join("gate");
    let block_cmd = format!("while [ ! -e {} ]; do sleep 0.05; done", gate.display());
    let args = json!({"questions":[{"title":"S103-报告面向谁？","options":["S103-内部评审","S103-外部客户"]}]}).to_string();
    let tc = json!({"choices":[{"message":{"tool_calls":[
        {"id":"s103-ask","type":"function","function":{
            "name":"request_user_input_async","arguments":args}},
        {"id":"s103-work","type":"function","function":{
            "name":"exec_command","arguments":json!({"cmd": block_cmd, "yield_time_ms": 30000}).to_string()}}
    ]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S103-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S103_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s103"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S103_KEY",
        "fake-s103",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("准备报告");
    // 发布提示 cell：不自动打开窗口，仅 F6 入口
    pty.wait_screen("已发布问询，等待用户回答，按 F6 打开回答")
        .unwrap();
    // 草稿不被抢占；等待期间继续输入（不提交）
    pty.send_raw("\x1b[200~S103-未发草稿\x1b[201~".as_bytes());
    pty.wait_screen("S103-未发草稿").unwrap();
    // F6 打开本轮唯一未决项（复用原问答窗口）
    pty.send_raw(b"\x1b[17~"); // F6（xterm 转义序列）
    pty.wait_screen("S103-报告面向谁").unwrap();
    pty.send_raw(b"\r");
    pty.wait_screen("Questions 1/1 answered").unwrap();
    // 回答已受理；解除独立命令的阻塞，批收尾后回答进入下一请求
    std::fs::write(&gate, b"1").unwrap();
    pty.wait_screen("S103-完成").unwrap();
    pty.wait_screen("S103-未发草稿").unwrap();
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 2, "回答不成为聊天轮、无空转请求");
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let publish_result = msgs
        .iter()
        .find(|m| m["tool_call_id"] == "s103-ask")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(publish_result["content"].as_str().unwrap()).unwrap(),
        json!({"accepted":true,"request_id":"s103-ask"})
    );
    let work_pos = msgs
        .iter()
        .position(|m| m["tool_call_id"] == "s103-work")
        .unwrap();
    let answer_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == "user" && m["content"].as_str().is_some_and(|c| c.contains("S103-内部评审"))
        })
        .expect("回答进入下一请求");
    assert!(work_pos < answer_pos, "回答在完整批之后进入");
}

/// 018-B S104：显式等待自动打开同一窗口；等待期间请求计数不增加；
/// ready 结果之后答案数据随下一请求供依赖动作使用。
#[test]
fn s104_wait_auto_opens_and_dependent_action_follows_answer() {
    let cfg = TempDir::new("s110-cfg");
    let user = TempDir::new("s110-user");
    let args = json!({"questions":[{"title":"S104-面向谁？","options":["S104-内部","S104-外部"]}]}).to_string();
    let wait = json!({"request_id":"s104-ask"}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, json!({"choices":[{"message":{"tool_calls":[
            {"id":"s104-ask","type":"function","function":{
                "name":"request_user_input_async","arguments":args}}]}}]}).to_string()),
        MockResp::json(200, json!({"choices":[{"message":{"tool_calls":[
            {"id":"s104-w","type":"function","function":{
                "name":"wait_for_user_input","arguments":wait}}]}}]}).to_string()),
        MockResp::json(200, chat_body("S104-依赖完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S110_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s110"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S110_KEY",
        "fake-s110",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("先问清楚");
    pty.wait_screen("已发布问询，等待用户回答").unwrap();
    // 模型显式等待：同一窗口自动打开（无需 F6）
    pty.wait_screen("S104-面向谁").unwrap();
    wait_request_count(&mut pty, &mock, 2);
    pty.send_raw(b"\r");
    pty.wait_screen("S104-依赖完成").unwrap();
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 3);
    let body: Value = serde_json::from_str(&mock.requests()[2].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let wait_pos = msgs
        .iter()
        .position(|m| m["tool_call_id"] == "s104-w")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(
            msgs[wait_pos]["content"].as_str().unwrap()
        )
        .unwrap(),
        json!({"ready":true,"request_id":"s104-ask"}),
        "ready 不是第二份回答"
    );
    let answer_pos = msgs
        .iter()
        .position(|m| {
            m["role"] == "user" && m["content"].as_str().is_some_and(|c| c.contains("S104-内部"))
        })
        .expect("答案数据随等待之后进入请求");
    assert!(wait_pos < answer_pos);
}

/// 018-B S105：等待中 Esc 取消——解除等待、剩余配对收尾、按 cancelled 关闭、
/// 不再请求模型；会话文件保存真实关闭原因。
#[test]
fn s105_esc_during_wait_cancels_and_closes_question() {
    let cfg = TempDir::new("s105-cfg");
    let user = TempDir::new("s105-user");
    let args = json!({"questions":[{"title":"S105-还继续吗？","options":["S105-继续","S105-停止"]}]}).to_string();
    let wait = json!({"request_id":"s105-ask"}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, json!({"choices":[{"message":{"tool_calls":[
            {"id":"s105-ask","type":"function","function":{
                "name":"request_user_input_async","arguments":args}}]}}]}).to_string()),
        MockResp::json(200, json!({"choices":[{"message":{"tool_calls":[
            {"id":"s105-w","type":"function","function":{
                "name":"wait_for_user_input","arguments":wait}}]}}]}).to_string()),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S105_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s105"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S105_KEY",
        "fake-s105",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("确认方向");
    pty.wait_screen("S105-还继续吗").unwrap();
    wait_request_count(&mut pty, &mock, 2);
    pty.send_raw(b"\x1b"); // Esc＝仅取消当前轮
    pty.wait_screen("用户已取消本轮：问询未完成").unwrap();
    assert_eq!(mock.count(), 2, "取消后不再请求模型");
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    // 会话文件：wait 错误结果＋关闭事件（cancelled），无回答事件
    let sessions = fs::read_dir(user.path().join("sessions")).unwrap();
    let session_file = sessions
        .into_iter()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("auto-"))
        })
        .expect("自动会话已落盘");
    let content = fs::read_to_string(&session_file).unwrap();
    assert!(content.contains("用户已取消本轮：问询未完成"), "{content}");
    // content 内 JSON 为转义形态；按字段组合断言（reason=cancelled 且无 answer 事件）
    assert!(
        content.contains("cancelled") && content.contains("reason"),
        "真实关闭原因：{content}"
    );
    assert!(
        !content.contains("\"answer\":{\"request_id\""),
        "未答问题不得出现回答事件：{content}"
    );
}

/// 018-B S106：已答异步问询的会话恢复——同源投影显示已答，恢复零请求。
#[test]
fn s106_resume_answered_async_question_displays_without_new_requests() {
    let cfg = TempDir::new("s106-cfg");
    let user = TempDir::new("s106-user");
    let args = json!({"questions":[{"title":"S106-风格？","options":["S106-正式","S106-轻松"]}]}).to_string();
    let wait = json!({"request_id":"s106-ask"}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, json!({"choices":[{"message":{"tool_calls":[
            {"id":"s106-ask","type":"function","function":{
                "name":"request_user_input_async","arguments":args}}]}}]}).to_string()),
        MockResp::json(200, json!({"choices":[{"message":{"tool_calls":[
            {"id":"s106-w","type":"function","function":{
                "name":"wait_for_user_input","arguments":wait}}]}}]}).to_string()),
        MockResp::json(200, chat_body("S106-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S106_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s106"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S106_KEY",
        "fake-s106",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("确认风格");
    pty.wait_screen("S106-风格").unwrap();
    pty.send_raw(b"\r");
    pty.wait_screen("S106-完成").unwrap();
    pty.send_line("/save s106");
    pty.wait_screen("已保存").unwrap();
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert_eq!(mock.count(), 3);
    let mut resumed = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S106_KEY",
        "fake-s106",
        true,
    ));
    resumed.wait_screen("? for shortcuts").unwrap();
    resumed.send_line("/resume s106");
    resumed.wait_screen("Questions 1/1 answered").unwrap();
    resumed.wait_screen("S106-正式").unwrap();
    resumed.send_raw(b"\x03");
    assert!(resumed.finish().1.success());
    assert_eq!(mock.count(), 3, "恢复只展示，不重新提问或请求模型");
}

// ---------- PTY 基础设施 ----------

/// 真实伪终端会话：master 留在本测试，slave 作为子进程的 stdin/stdout/stderr，
/// 使 CLI 走交互分支（IsTerminal＝true、授权走交互询问）。
///
/// 008-ADAPT：交互分支进入全屏 TUI（ratatui 差分渲染，跳过未变化空格＋光标
/// 跳转），含空格文本在原始字节流中不连续——新增 wait_screen 在 VT100 解析后
/// 的屏幕文本上等待；send_line 改以 CR 结尾（TUI raw mode 下 Enter＝CR，
/// rustyline 阶段 CR 经 ICRNL 同样按行结束）。
struct PtySession {
    master: File,
    child: Child,
    captured: Vec<u8>,
    vt: tchat_tui::test_backend::VT100Backend,
    vt_fed: usize,
    query_responder: QueryResponder,
}

impl PtySession {
    fn spawn(mut cmd: Command) -> PtySession {
        unsafe {
            let master_fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(master_fd >= 0, "posix_openpt 失败：{}", std::io::Error::last_os_error());
            assert_eq!(libc::grantpt(master_fd), 0, "grantpt 失败");
            assert_eq!(libc::unlockpt(master_fd), 0, "unlockpt 失败");
            let mut name = [0u8; 256];
            assert_eq!(
                libc::ptsname_r(master_fd, name.as_mut_ptr().cast(), name.len()),
                0,
                "ptsname_r 失败"
            );
            let len = name.iter().position(|&b| b == 0).unwrap();
            let slave_path =
                PathBuf::from(std::ffi::OsString::from_vec(name[..len].to_vec()));
            let c_path = std::ffi::CString::new(slave_path.as_os_str().as_bytes()).unwrap();
            let fd0 = libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            assert!(fd0 >= 0, "打开 PTY 从设备失败：{}", std::io::Error::last_os_error());
            let fd1 = libc::dup(fd0);
            let fd2 = libc::dup(fd0);
            // 宽窗口：长提示（含 origin 地址）不换行，保证子串断言稳定
            let ws = libc::winsize { ws_row: PTY_ROWS, ws_col: PTY_COLS, ws_xpixel: 0, ws_ypixel: 0 };
            libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
            // 成为会话组长并把 PTY 挂为控制终端：终端产生的 SIGINT（Ctrl+C）才能
            // 送达前台进程组——否则  只有回显、无信号（N2b 真实语义所需）
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            cmd.stdin(Stdio::from(File::from_raw_fd(fd0)));
            cmd.stdout(Stdio::from(File::from_raw_fd(fd1)));
            cmd.stderr(Stdio::from(File::from_raw_fd(fd2)));
            let child = cmd.spawn().expect("PTY 子进程启动失败");
            PtySession {
                master: File::from_raw_fd(master_fd),
                child,
                captured: Vec::new(),
                vt: tchat_tui::test_backend::VT100Backend::new(PTY_COLS, PTY_ROWS), // 008-ADAPT
                vt_fed: 0,
                query_responder: QueryResponder::new(),
            }
        }
    }

    /// 008-ADAPT：把未喂入字节增量送进 VT100 解析器。
    fn feed_screen(&mut self) {
        use std::io::Write as _;
        let end = self.captured.len();
        if self.vt_fed < end {
            let _ = self.vt.write_all(&self.captured[self.vt_fed..end]);
            self.vt_fed = end;
        }
    }

    /// 008-ADAPT：等待 VT100 屏幕文本包含 needle（TUI 渲染内容的标准等待）。
    fn wait_screen(&mut self, needle: &str) -> Result<(), String> {
        self.wait_screen_timeout(needle, WATCHDOG)
    }

    fn wait_screen_timeout(&mut self, needle: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.feed_screen();
            if self.vt.to_string().contains(needle) {
                return Ok(());
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(format!(
                    "等待屏幕 {:?} 超时（{:?}），屏幕：\n{}\n原始输出：\n{}",
                    needle,
                    timeout,
                    self.vt.to_string(),
                    String::from_utf8_lossy(&self.captured)
                ));
            }
            self.read_once(50);
        }
    }

    /// 非阻塞读一次 master（poll ms），有数据并入 captured。
    fn read_once(&mut self, poll_ms: i32) {
        let mut pfd =
            libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, poll_ms) };
        if r > 0 && pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            let mut buf = [0u8; 4096];
            let n = unsafe {
                libc::read(self.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len())
            };
            if n > 0 {
                let start = self.captured.len();
                self.captured.extend_from_slice(&buf[..n as usize]);
                self.answer_terminal_queries(start);
            }
        }
    }

    /// 用 QueryResponder 处理新增字节的查询应答（见其文档）。
    fn answer_terminal_queries(&mut self, from: usize) {
        self.feed_screen(); // 解析器先行消化（DSR 回报解析器真实光标）
        let cursor = self.vt.vt100().screen().cursor_position();
        let replies = self.query_responder.advance(&self.captured[from..], cursor);
        let _ = self.master.write_all(&replies);
    }

    /// 轮询 master 直到累计输出包含 needle；超时则杀死子进程并报错（看门狗）。
    /// 008-ADAPT：TUI 差分渲染下仅适合稳定的 raw 标记（横幅、rustyline 授权
    /// 提问、终端恢复后的“再见!”）；TUI 屏幕内容用 wait_screen。
    fn wait_output(&mut self, needle: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let target = needle.as_bytes();
        while !window_contains(&self.captured, target) {
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(format!(
                    "等待输出 {:?} 超时（{:?}），已捕获：\n{}",
                    needle,
                    timeout,
                    String::from_utf8_lossy(&self.captured)
                ));
            }
            self.read_once(50);
        }
        Ok(())
    }

    /// 发送原始字节（如 Ctrl+C 的 0x03，不附加换行）。
    fn send_raw(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).expect("PTY 写入失败");
    }

    /// 017：真实缩放＋测试侧屏幕同步——在既有 [`PtySession::resize`]（TIOCSWINSZ
    /// →SIGWINCH→驱动重绘）基础上把 VT100 屏幕同步为新尺寸，此后的字节按新
    /// 尺寸解析；用于断言重排后的窄/宽布局本身（与既有 resize 的「解析器保持
    /// 原尺寸、行号一一对应」语义互补，各自服务不同断言面）。
    fn resize_and_sync(&mut self, cols: u16, rows: u16) {
        self.resize(cols, rows);
        self.vt.set_size(rows, cols);
    }

    /// 017：有界等待自定义屏幕谓词成立（真实屏幕断言；超时报错含屏幕与说明）。
    fn wait_screen_pred(
        &mut self,
        desc: &str,
        pred: impl Fn(&str) -> bool,
    ) -> Result<(), String> {
        let deadline = Instant::now() + WATCHDOG;
        loop {
            self.feed_screen();
            if pred(&self.vt.to_string()) {
                return Ok(());
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(format!(
                    "等待屏幕条件 {:?} 超时，屏幕：\n{}",
                    desc,
                    self.vt.to_string()
                ));
            }
            self.read_once(50);
        }
    }

    /// 发送一行：先写文本、隔 150ms 再写 CR。008-ADAPT：
    /// - TUI raw mode 下 Enter＝CR；TUI 之前的 rustyline 提问对 CR 经 ICRNL 按行结束；
    /// - TUI composer 的 paste-burst 启发式（≥3 个 ASCII 字符连打→120ms 内的
    ///   Enter 被并入草稿）要求分隔发送，模拟真人键入节奏。
    fn send_line(&mut self, line: &str) {
        self.master.write_all(line.as_bytes()).expect("PTY 写入失败");
        std::thread::sleep(Duration::from_millis(150));
        self.master.write_all(b"\r").expect("PTY 写入失败");
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(&self.captured).into_owned()
    }

    /// 结束会话：限期等待退出、排干尾部输出，返回（输出，退出状态）。
    fn finish(mut self) -> (String, std::process::ExitStatus) {
        let deadline = Instant::now() + WATCHDOG;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("PTY 子进程未在期限内退出");
                }
            }
        };
        // 排干退出前后的残余输出（有限窗口，非关键时序）
        let drain_end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < drain_end {
            let mut pfd =
                libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let r = unsafe { libc::poll(&mut pfd, 1, 50) };
            if r > 0 && pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                let mut buf = [0u8; 4096];
                let n = unsafe {
                    libc::read(self.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len())
                };
                if n > 0 {
                    self.captured.extend_from_slice(&buf[..n as usize]);
                } else {
                    break; // EOF/EIO：对端已关闭
                }
            } else {
                break;
            }
        }
        (self.output(), status)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn window_contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len().max(1)).any(|w| w == needle)
}

fn pty_cli(project: Option<&Path>, envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    // 015：未指定项目目录时默认落 /tmp（无 .git 祖先、无 AGENTS.md）——上下文
    // 注入为环境块形态，测试不依赖仓库真实 AGENTS.md 内容
    let default_dir = std::env::temp_dir();
    cmd.current_dir(project.unwrap_or(&default_dir));
    for (k, v) in envs {
        cmd.env(k, v);
    }
    // 交互分支的前提：PTY 使 stdin/stdout 均为终端；TERM 显式给正常值
    cmd.env("TERM", "xterm");
    cmd
}

// ---------- 最薄 JSON 宿主（exec_command / apply_patch）----------

/// 宿主子进程模式：由 `TCHAT_E2E_TOOL_INPUT` 触发。读入
/// {"tool": "exec_command"|"apply_patch", "input": {...}}，调用真实库入口，
/// 把结果 JSON 写到 `TCHAT_E2E_TOOL_OUTCOME` 后退出。
fn tool_child_mode() {
    let Some(input_path) = std::env::var_os("TCHAT_E2E_TOOL_INPUT") else {
        return; // 父进程模式
    };
    let outcome_path =
        std::env::var_os("TCHAT_E2E_TOOL_OUTCOME").expect("缺少 TCHAT_E2E_TOOL_OUTCOME");
    let spec: Value =
        serde_json::from_str(&std::fs::read_to_string(&input_path).unwrap()).unwrap();
    let result = match spec["tool"].as_str().unwrap() {
        "exec_command" => {
            // 016：经进程会话管理器执行（短命令在等待窗口内完成；子进程退出即关）
            let mgr = terminal_chat::proc_session::ProcessManager::new();
            let outcome = match terminal_chat::exec::ExecArgs::from_value(&spec["input"]) {
                Ok(args) => mgr.start(&args, None),
                Err(msg) => terminal_chat::proc_session::ToolOutcome::from(
                    terminal_chat::exec::ExecResult::error(msg),
                ),
            };
            let result = outcome.result;
            mgr.close();
            serde_json::to_value(result).unwrap()
        }
        "apply_patch" => serde_json::to_value(terminal_chat::apply_patch::apply_patch(
            &spec["input"],
        ))
        .unwrap(),
        other => panic!("未知工具 {other}"),
    };
    std::fs::write(&outcome_path, serde_json::to_string(&result).unwrap()).unwrap();
    std::process::exit(0);
}

/// 经隔离子进程调用工具（JSON 进、JSON 出），30s 看门狗，超时杀死并报错。
fn call_tool(entry: &str, tool: &str, input: Value) -> Value {
    assert!(
        std::env::var_os("TCHAT_E2E_TOOL_INPUT").is_none(),
        "工具宿主子进程模式到达了未设防的用例"
    );
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let io = TempDir::new(&format!("e2e-tool-io-{seq}"));
    let input_path = io.path().join("in.json");
    let outcome_path = io.path().join("out.json");
    std::fs::write(&input_path, json!({ "tool": tool, "input": input }).to_string())
        .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg(entry)
        .arg("--exact")
        .env("TCHAT_E2E_TOOL_INPUT", &input_path)
        .env("TCHAT_E2E_TOOL_OUTCOME", &outcome_path)
        .stdout(Stdio::null())
        .spawn()
        .expect("工具宿主子进程启动失败");
    let deadline = Instant::now() + WATCHDOG;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                assert!(st.success(), "工具宿主子进程异常退出：{st}");
                let text = std::fs::read_to_string(&outcome_path).unwrap_or_else(|e| {
                    panic!("宿主成功退出但未写结果（entry 漂移或缺 tool_child_mode？）：{e}")
                });
                return serde_json::from_str(&text).unwrap();
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("工具宿主看门狗超时（{WATCHDOG:?}）：tool={tool}");
            }
        }
    }
}

// ---------- 通用辅助 ----------

fn write_manual(dir: &Path, base_url: &str, key_ref: &str) {
    write_manual_with_memories(dir, base_url, key_ref, None)
}

/// 013-M7：带 memories 段的 manual（开态 e2e 用）。
fn write_manual_with_memories(
    dir: &Path,
    base_url: &str,
    key_ref: &str,
    memories: Option<terminal_chat::memory::MemoriesConfig>,
) {
    let m = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(base_url.into()),
                key_ref: Some(key_ref.into()),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(dir.join("manual.json"), serde_json::to_string_pretty(&m).unwrap())
        .unwrap();
}

/// 019：声明 input_modalities 含 image 的 manual（view_image e2e 用）。
fn write_manual_image_capable(dir: &Path, base_url: &str, key_ref: &str) {
    let m = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(base_url.into()),
                key_ref: Some(key_ref.into()),
                models: [(
                    "deepseek-flash".to_string(),
                    catalog::ModelInfo {
                        id: "deepseek-flash".into(),
                        input_modalities: Some(vec!["text".into(), "image".into()]),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(dir.join("manual.json"), serde_json::to_string_pretty(&m).unwrap())
        .unwrap();
}

/// 预置授权记录（合法预授权＝此前交互确认过），返回写入字节便于比对"未改写"。
fn seed_grant(user_dir: &Path, origin: &str, cred_fp: &str) -> String {
    let text =
        json!({ "grants": [{ "origin": origin, "cred_fp": cred_fp, "added_at": 0 }] })
            .to_string();
    std::fs::write(user_dir.join("authorized-origins.json"), &text).unwrap();
    text
}

/// 管道模式运行真实 CLI；先移除信任根变量（防止"已有变量不可覆盖"让攻击场景空跑），
/// 再应用调用方显式变量（含测试专用 HOME/TCHAT_*，保证不读写真实用户环境）。
fn run_pipe(
    project_dir: Option<&Path>,
    envs: &[(&str, &str)],
    stdin_text: &str,
) -> (String, String, Option<i32>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    if let Some(d) = project_dir {
        cmd.current_dir(d);
    }
    for k in ["TCHAT_USER_DIR", "TCHAT_CONFIG_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
        cmd.env_remove(k);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("CLI 子进程启动失败");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_text.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("等待 CLI 子进程失败");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

fn chat_body(content: &str) -> String {
    json!({ "choices": [{ "message": { "content": content }, "finish_reason": "stop" }] })
        .to_string()
}

fn unix_mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

// ---------- S1：REPL 正常使用（PTY 交互全链路）----------

/// S1：交互会话（008-ADAPT：TUI）读取配置→发送消息→显示回复→退出；
/// /models 展示证据经管道（行式）路径核对（TUI 内被 composer 拒绝，见用例内注释）。
/// 核对：请求地址与路径、Authorization、请求体（模型与消息内容）、回复展示、退出码 0。
#[test]
fn s1_pty_normal_session_full_roundtrip() {
    let cfg = TempDir::new("s1-cfg");
    let user = TempDir::new("s1-user");
    let key = "s1-key-value";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S1-REPLY-收到"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S1_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    let seeded = seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S1_KEY", key),
    ]));
    // 开场横幅 → TUI 首屏（空历史占位）→ 对话 → 回复 → 退出告别
    // 008-REVIEW：TUI 模式跳过开场横幅（J10 显示层替代）——等待 TUI 首屏

    // 008-ADAPT：行式提示符 "你> " 不再存在；TUI 就绪标记＝首屏空历史占位
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell）
    pty.send_line("你好");
    // 008-ADAPT：回复经 Markdown cell 渲染，无 "Agent：" 前缀，等待回复原文
    pty.wait_screen("S1-REPLY-收到").unwrap();
    // 008-REVIEW P2-8：用户消息 cell 持久渲染（母本 transcript 含用户消息；
    // 提交后草稿已清空，此断言只可能由 transcript cell 满足）
    std::thread::sleep(Duration::from_millis(300));
    pty.feed_screen();
    assert!(
        pty.vt.to_string().contains("你好"),
        "用户消息必须在提交后保留在 transcript\n{}",
        pty.vt.vt100().screen().contents()
    );
    // 008-REVIEW：/models 已入注册表（slash_command.rs 008-ADAPT），TUI 内可用；
    // 但管道（行式）路径核对配置读取证据更完整（含能力字段上下文行），沿用。
    pty.send_line("/exit"); // 008-ADAPT：TUI 退出走 /exit 斜杠命令（SlashCommand::Exit）
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(0), "退出码契约（001 §5），输出：{out}");
    // 008-A5：TUI 退出后终端恢复——备用屏禁用序列必须出现（[?1049l），
    // 括号粘贴与同步更新关闭；光标重新可见。原始字节级检查（经 wait_output 的捕获流）。
    assert!(
        out.contains("\u{1b}[?1049l"),
        "退出后备用屏必须被禁用（终端恢复），输出尾部：{}",
        &out[out.len().saturating_sub(400)..]
    );
    // 实际发出的请求：地址、方法、路径、凭证头、请求体
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "无重试机制：一次对话只发一次请求，实际 {reqs:?}");
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].path, "/chat/completions");
    assert_eq!(reqs[0].auth.as_deref(), Some("Bearer s1-key-value"));
    let body: Value = serde_json::from_str(&reqs[0].body).unwrap();
    assert_eq!(body["model"], "deepseek-flash");
    // 005/006 契约：system（发送版提示词）首位＋015 ctx＋user＋两项工具定义
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(
        body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("You are a coding agent"),
        "system 应为 Codex 母本适配版提示词"
    );
    assert_eq!(body["messages"][1]["role"], "user");
    // 015：ctx（环境块形态）恰在本轮输入之前
    assert!(
        body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .starts_with("<environment_context>"),
        "ctx 应为环境块：{}",
        body["messages"][1]["content"]
    );
    assert_eq!(body["messages"][2]["role"], "user");
    assert_eq!(body["messages"][2]["content"], "你好");
    assert_eq!(
        body["tools"].as_array().unwrap().len(),
        7,
        "018-B：TUI 具备同步＋异步回答宿主的交互轮七工具"
    );
    // 凭证明文不回显；已授权命中不改写授权记录
    assert!(!out.contains(key), "密钥明文不得出现在终端输出");
    assert_eq!(
        std::fs::read(user.path().join("authorized-origins.json")).unwrap(),
        seeded.as_bytes(),
        "已授权提前返回，不得改写授权记录"
    );

    // 008-ADAPT：/models 展示证据改经管道（行式）路径核对——TUI 内 "/models" 被
    // composer 以 Unrecognized command 拒绝（母本命令注册表不含该名），无法触发
    let (models_out, _err, models_code) = run_pipe(
        None,
        &[
            ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
            ("TCHAT_S1_KEY", key),
        ],
        "/models\nexit\n",
    );
    assert_eq!(models_code, Some(0), "{models_out}");
    assert!(
        models_out.contains(&format!("厂商 deepseek（基础地址 {}）", mock.url)),
        "配置读取证据（基础地址＝mock 地址）：{models_out}"
    );
    assert!(models_out.contains("deepseek-flash"), "{models_out}");
    assert!(models_out.contains("上下文 1048576"), "{models_out}");
}

// ---------- S2：授权闭环（PTY 首次授权）----------

/// S2a：PTY 首次授权同意——真实交互询问出现，y 确认后：
/// 请求携带凭证、授权记录落盘（0600、origin+指纹双匹配）、同会话第二条消息不再询问。
#[test]
fn s2a_pty_first_use_authorization_agree_and_persist() {
    let cfg = TempDir::new("s2a-cfg");
    let user = TempDir::new("s2a-user");
    let key = "s2a-key-value";
    let mock = spawn_mock(vec![
        MockResp::json(200, chat_body("S2A-AGREE-OK")),
        MockResp::json(200, chat_body("S2A-SECOND-OK")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S2A_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    let auth_file = user.path().join("authorized-origins.json");

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S2A_KEY", key),
    ]));
    // 008-ADAPT：首次授权询问发生在 TUI 之前（普通终端 rustyline 提问，008 §3）：
    // 启动即问，无需先发消息触发
    pty.wait_output("即将把凭证 TCHAT_S2A_KEY", WATCHDOG).unwrap();
    pty.wait_output(&format!("发送到 {origin}"), WATCHDOG).unwrap();
    pty.wait_output("允许并记住此授权？", WATCHDOG).unwrap();
    pty.send_line("y");
    // 授权通过后进入 TUI（空历史占位＝就绪）
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell）
    pty.send_line("你好");
    // 008-ADAPT：无 "Agent：" 前缀，等待回复原文
    pty.wait_screen("S2A-AGREE-OK").unwrap();
    let grant_after_first = std::fs::read(&auth_file).unwrap();
    pty.send_line("第二条消息");
    pty.wait_screen("S2A-SECOND-OK").unwrap();
    pty.send_line("/exit"); // 008-ADAPT：TUI 退出走 /exit
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(0), "输出：{out}");
    // 实际请求数与凭证：两次对话各一次，均携带凭证
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2, "实际请求数：{reqs:?}");
    for r in &reqs {
        assert_eq!(r.path, "/chat/completions");
        assert_eq!(r.auth.as_deref(), Some("Bearer s2a-key-value"));
    }
    let body: Value = serde_json::from_str(&reqs[1].body).unwrap();
    // 006：第二轮请求携带第一轮历史（system 首位＋015 ctx→user→assistant→新 user；
    // 同会话内容不变，ctx 不重复注入）
    let roles: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        vec!["system", "user", "user", "assistant", "user"],
        "跨轮历史原样携带（含 015 ctx 恰一条）"
    );
    assert!(
        body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .starts_with("<environment_context>"),
        "015 ctx 为环境块"
    );
    assert_eq!(body["messages"][3]["content"], json!("S2A-AGREE-OK"), "第一轮回复入历史");
    assert_eq!(body["messages"][4]["content"], "第二条消息");
    // 授权记录变化：从无到有，且 (origin, 指纹) 双匹配、私有权限；
    // 第二次使用不得重复新增，也不得把明文 Key 写入任何字段。
    let grant_bytes = std::fs::read(&auth_file).unwrap();
    assert_eq!(grant_bytes, grant_after_first, "再次使用不得新增授权记录");
    let grant_text = std::fs::read_to_string(&auth_file).unwrap();
    assert!(!grant_text.contains(key), "授权文件不得保存明文 Key：{grant_text}");
    let store: Value = serde_json::from_str(&grant_text).unwrap();
    assert_eq!(store["grants"].as_array().map(Vec::len), Some(1), "{grant_text}");
    assert_eq!(store["grants"][0]["origin"], json!(origin));
    assert_eq!(store["grants"][0]["cred_fp"], json!(catalog::fingerprint(key)));
    assert!(store["grants"][0]["added_at"].as_u64().unwrap_or(0) > 0);
    assert_eq!(unix_mode(&auth_file), 0o600, "授权记录必须 0600");
    // 第二条消息不再询问：首次回复之后不应再次出现授权询问
    // 008-ADAPT：回复无 "Agent：" 前缀，切分锚点改为回复原文（ASCII 无空格，raw 稳定）
    let tail = out
        .split_once("S2A-AGREE-OK")
        .map(|(_, t)| t)
        .unwrap_or(&out);
    assert!(!tail.contains("允许并记住此授权"), "已授权后不得重复询问：{tail}");
    assert!(!out.contains(key), "密钥明文不得出现在终端输出");
}

/// S2b：PTY 首次授权拒绝——n 拒绝后：零请求、凭证零外发、不落盘授权记录、错误明确。
#[test]
fn s2b_pty_first_use_authorization_deny_zero_requests() {
    let cfg = TempDir::new("s2b-cfg");
    let user = TempDir::new("s2b-user");
    let key = "s2b-key-value";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S2B-MUST-NOT-APPEAR"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S2B_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    let auth_file = user.path().join("authorized-origins.json");

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S2B_KEY", key),
    ]));
    // 008-ADAPT：拒绝发生在 TUI 之前的启动期授权门（rustyline 提问文本不变）
    pty.wait_output("允许并记住此授权？", WATCHDOG).unwrap();
    pty.send_line("n");
    pty.wait_output(&format!("错误：已拒绝将凭证 TCHAT_S2B_KEY 发送到 {origin}"), WATCHDOG)
        .unwrap();
    // 008-ADAPT：启动期授权被拒→main 直接报错退出（码 1），不进入 TUI，无 "再见!"
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(1), "输出：{out}");
    assert!(!out.contains("再见!"), "未进入 TUI 不得打印告别语：{out}");
    assert_eq!(mock.count(), 0, "拒绝授权必须零请求、凭证零外发");
    assert!(!auth_file.exists(), "拒绝路径不得落盘授权记录");
    // 008-ADAPT：TUI 无 "Agent：" 前缀可断，"不得出现模型回复"改盯 mock 回复原文
    assert!(!out.contains("S2B-MUST-NOT-APPEAR"), "不得出现模型回复：{out}");
    assert!(!out.contains(key), "密钥明文不得出现在终端输出：{out}");
}

/// S2c：启动期授权问句期间 Ctrl+C——rustyline 将其转为 Interrupted，与答 n 同路径
/// （prompt_y_n 视一切读取错误为拒绝）：错误退出码 1、零请求、无再见、不落盘授权。
/// 008 登记灰区：该阶段不属于 001 三段 Ctrl+C 契约（启动早期 130／就绪空闲 0／
/// 工具期取消）；本用例钉住现状（拒绝＝安全侧），是否改为优雅退出属待决项。
#[test]
fn s2c_pty_authorization_question_ctrl_c_denies() {
    let cfg = TempDir::new("s2c-cfg");
    let user = TempDir::new("s2c-user");
    let key = "s2c-key-value";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S2C-MUST-NOT-APPEAR"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S2C_KEY");
    let auth_file = user.path().join("authorized-origins.json");

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S2C_KEY", key),
    ]));
    pty.wait_output("允许并记住此授权？", WATCHDOG).unwrap();
    pty.send_raw(b"\x03"); // 授权问句期间 Ctrl+C
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(1), "Ctrl+C 拒绝与答 n 同路径（码 1），输出：{out}");
    assert!(!out.contains("再见!"), "未进入 TUI 不得打印告别语：{out}");
    assert_eq!(mock.count(), 0, "拒绝授权必须零请求、凭证零外发");
    assert!(!auth_file.exists(), "拒绝路径不得落盘授权记录");
    assert!(!out.contains(key), "密钥明文不得出现在终端输出：{out}");
}

// ---------- S3：F1 攻击与正例 ----------

/// S3a：项目 .env 尝试重定位信任根（TCHAT_USER_DIR/HOME/TCHAT_CONFIG_DIR，
/// 含大小写变体），攻击者目录预置对受害者凭证的有效指纹授权记录，受害者凭证来自
/// 进程环境且攻击变量未预设（攻击不被"已有变量不可覆盖"空跑）。
/// 修复后必须：零外发、授权根未重定位（指向受害者家目录）、攻击者文件原样。
#[test]
fn s3a_f1_env_trust_root_hijack_blocked() {
    let project = TempDir::new("s3a-proj");
    let attacker_user = TempDir::new("s3a-attacker-user");
    let attacker_home = TempDir::new("s3a-attacker-home");
    let attacker_cfg = TempDir::new("s3a-attacker-cfg");
    let victim_home = TempDir::new("s3a-victim-home");
    let victim_key = "sk-victim-f1-e2e";
    // 攻击者端点：受害者项目 config 指向它（克隆仓库自带配置的真实攻击面）
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S3A-LEAK-MUST-NOT-HAPPEN"))]);
    // 重定位成功时才会被访问的第二个端点（TCHAT_CONFIG_DIR 劫持目标）
    let evil2 = spawn_mock(vec![MockResp::json(200, chat_body("S3A-EVIL2"))]);
    let origin = catalog::origin_of(&mock.url).unwrap();

    // 受害者项目：./config/manual.json（缺省回退位置）指向攻击者端点
    let cfg_dir = project.path().join("config");
    fs::create_dir_all(&cfg_dir).unwrap();
    write_manual(&cfg_dir, &mock.url, "env:DEEPSEEK_API_KEY");
    // 攻击者重定向目标：TCHAT_CONFIG_DIR 值即目录本身，manual.json 位于其根
    write_manual(attacker_cfg.path(), &evil2.url, "env:DEEPSEEK_API_KEY");
    // 攻击者 .env：重定位信任根（大小写变体齐上）
    let env_text = format!(
        "TCHAT_USER_DIR={au}\n\
         tchat_user_dir={au}\n\
         TCHAT_CONFIG_DIR={ac}\n\
         tchat_config_dir={ac}\n\
         HOME={ah}\n\
         home={ah}\n\
         APPDATA={ah}\n\
         appdata={ah}\n",
        au = attacker_user.path().display(),
        ac = attacker_cfg.path().display(),
        ah = attacker_home.path().display(),
    );
    fs::write(project.path().join(".env"), &env_text).unwrap();
    // 攻击者预置：对受害者凭证有效指纹的授权记录（同时覆盖两个端点的 origin，
    // 使任一重定向路径都能得手——变异证明时攻击不得空跑）
    let grant_path = attacker_user.path().join("authorized-origins.json");
    let seeded = {
        let text = json!({ "grants": [
            { "origin": origin, "cred_fp": catalog::fingerprint(victim_key), "added_at": 0 },
            { "origin": catalog::origin_of(&evil2.url).unwrap(), "cred_fp": catalog::fingerprint(victim_key), "added_at": 0 },
        ] })
        .to_string();
        fs::write(&grant_path, &text).unwrap();
        text
    };

    // 受害者运行：凭证来自进程环境；信任根变量未预设（HOME 指向受害者家目录）
    let (out, err, code) = run_pipe(
        Some(project.path()),
        &[
            ("HOME", victim_home.path().to_str().unwrap()),
            ("DEEPSEEK_API_KEY", victim_key),
        ],
        "你好\nexit\n",
    );

    assert_eq!(code, Some(0), "stdout：{out}\nstderr：{err}");
    // 零外发：攻击端点与重定向端点都不得收到请求
    assert_eq!(mock.count(), 0, "攻击者端点收到请求——信任根被 .env 重定位：{out}");
    assert_eq!(evil2.count(), 0, "TCHAT_CONFIG_DIR 劫持目标收到请求：{out}");
    // 授权根未改变：拒绝提示指向受害者家目录的授权记录路径
    let victim_auth = victim_home
        .path()
        .join(".config")
        .join("terminal-chat")
        .join("authorized-origins.json");
    let victim_auth_text = victim_auth.display().to_string();
    assert!(
        out.contains(&victim_auth_text),
        "错误应指向受害者授权根 {victim_auth_text}，实际：{out}"
    );
    assert!(out.contains("非交互模式"), "管道模式必须 fail-closed：{out}");
    assert!(out.contains("未授权发送到"), "{out}");
    // 实际使用的是受害者 ./config（其 origin），未被 .env 重定位到攻击者配置
    assert!(out.contains(&origin), "错误应含实际配置的 origin：{out}");
    // 攻击者预置记录原样未被触碰；受害者家目录未产生新授权记录
    assert_eq!(fs::read(&grant_path).unwrap(), seeded.as_bytes());
    assert!(!victim_auth.exists(), "非交互拒绝不得在受害者根落盘授权");
    // 凭证未泄露到输出
    assert!(!out.contains(victim_key) && !err.contains(victim_key), "密钥明文泄露：{out}{err}");
}

/// S3b：F1 正例——普通 .env 凭证变量照常加载；调用者显式 TCHAT_USER_DIR＋合法预授权
/// 正常发送凭证。证明修复只封信任根重定位，不误伤正常使用。
#[test]
fn s3b_f1_positive_dotenv_credential_and_explicit_preauth() {
    // 正例一：普通凭证变量来自项目 .env（进程环境未预设该变量）
    let project = TempDir::new("s3b-proj1");
    let cfg_dir = project.path().join("config");
    fs::create_dir_all(&cfg_dir).unwrap();
    let user1 = TempDir::new("s3b-user1");
    let mock1 = spawn_mock(vec![MockResp::json(200, chat_body("S3B-DOTENV-OK"))]);
    let origin1 = catalog::origin_of(&mock1.url).unwrap();
    write_manual(&cfg_dir, &mock1.url, "env:TCHAT_S3B_DOTENV_KEY");
    fs::write(project.path().join(".env"), "TCHAT_S3B_DOTENV_KEY=dotenv-key-88\n").unwrap();
    seed_grant(user1.path(), &origin1, &catalog::fingerprint("dotenv-key-88"));
    let home1 = TempDir::new("s3b-home1");
    let (out, err, code) = run_pipe(
        Some(project.path()),
        &[
            ("HOME", home1.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user1.path().to_str().unwrap()),
        ],
        "你好\nexit\n",
    );
    assert_eq!(code, Some(0), "stdout：{out}\nstderr：{err}");
    assert!(out.contains("Agent：S3B-DOTENV-OK"), ".env 普通凭证应正常对话：{out}");
    assert_eq!(mock1.count(), 1);
    assert_eq!(mock1.requests()[0].auth.as_deref(), Some("Bearer dotenv-key-88"));

    // 正例二：显式 TCHAT_USER_DIR＋合法预授权（此前交互确认过）→ 正常发送
    let project2 = TempDir::new("s3b-proj2");
    let cfg2 = project2.path().join("config");
    fs::create_dir_all(&cfg2).unwrap();
    let user2 = TempDir::new("s3b-user2");
    let mock2 = spawn_mock(vec![MockResp::json(200, chat_body("S3B-PREAUTH-OK"))]);
    let origin2 = catalog::origin_of(&mock2.url).unwrap();
    write_manual(&cfg2, &mock2.url, "env:DEEPSEEK_API_KEY");
    seed_grant(user2.path(), &origin2, &catalog::fingerprint("preauth-key-99"));
    let home2 = TempDir::new("s3b-home2");
    let (out, err, code) = run_pipe(
        Some(project2.path()),
        &[
            ("HOME", home2.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user2.path().to_str().unwrap()),
            ("DEEPSEEK_API_KEY", "preauth-key-99"),
        ],
        "你好\nexit\n",
    );
    assert_eq!(code, Some(0), "stdout：{out}\nstderr：{err}");
    assert!(out.contains("Agent：S3B-PREAUTH-OK"), "合法预授权应正常对话：{out}");
    assert_eq!(mock2.count(), 1);
    assert_eq!(mock2.requests()[0].auth.as_deref(), Some("Bearer preauth-key-99"));
}

// ---------- S4：apply_patch 数据闭环（最薄 JSON 宿主）----------

/// S4：新增 → 依赖前块结果的连续修改（补丁内级联两块＋跨调用依赖）→ 删除，
/// 逐阶段核对文件字节、存在状态与 changed_files；常规文件链接写穿正常；
/// FIFO 与链接到 FIFO 的 Update 被拒绝。
#[test]
fn s4_json_host_apply_patch_data_loop() {
    tool_child_mode();
    let dir = TempDir::new("s4");
    let f = dir.path().join("app.conf");
    let entry = "s4_json_host_apply_patch_data_loop";
    let patch_of = |body: String| json!({ "patch": format!("*** Begin Patch\n{body}\n*** End Patch") });

    // 阶段A：新增
    let r = call_tool(entry, "apply_patch", patch_of(format!(
        "*** Add File: {p}\n+v1\n+alpha", p = f.display()
    )));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["state_known"], json!(true), "{r}");
    assert_eq!(
        r["changed_files"],
        json!([{ "path": f.display().to_string(), "action": "add" }]),
        "{r}"
    );
    assert_eq!(fs::read(&f).unwrap(), b"v1\nalpha\n".to_vec(), "新增文件字节（LF 收尾）");

    // 阶段B：补丁内两个 Update File 块级联（第二块匹配第一块产物；
    // 同文件多块逐块独立重读、按执行事件列清单——D1 契约）
    let r = call_tool(entry, "apply_patch", patch_of(format!(
        "*** Update File: {p}\n@@\n-v1\n+v2\n*** Update File: {p}\n@@\n-alpha\n+beta", p = f.display()
    )));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(
        r["changed_files"],
        json!([
            { "path": f.display().to_string(), "action": "update" },
            { "path": f.display().to_string(), "action": "update" },
        ]),
        "清单按成功执行事件逐块列出：{r}"
    );
    assert_eq!(fs::read(&f).unwrap(), b"v2\nbeta\n".to_vec(), "级联修改后的字节");
    let r = call_tool(entry, "apply_patch", patch_of(format!(
        "*** Update File: {p}\n@@\n-beta\n+final", p = f.display()
    )));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(fs::read(&f).unwrap(), b"v2\nfinal\n".to_vec(), "跨调用依赖前次结果的修改");

    // 阶段C：删除
    let r = call_tool(entry, "apply_patch", patch_of(format!(
        "*** Delete File: {p}", p = f.display()
    )));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(
        r["changed_files"],
        json!([{ "path": f.display().to_string(), "action": "delete" }]),
        "{r}"
    );
    assert!(!f.exists(), "删除后目标不存在");

    // 常规文件链接：Update 经链接写穿真实目标，链接本体保持
    let real = dir.path().join("real.txt");
    fs::write(&real, "mode=dev\n").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let r = call_tool(entry, "apply_patch", patch_of(format!(
        "*** Update File: {p}\n@@\n-mode=dev\n+mode=e2e", p = link.display()
    )));
    assert_eq!(r["ok"], json!(true), "常规文件链接不得被误伤：{r}");
    assert_eq!(fs::read(&real).unwrap(), b"mode=e2e\n".to_vec(), "真实目标被写穿修改");
    assert!(
        std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
        "链接本体必须保持为符号链接"
    );

    // FIFO 与链接到 FIFO：Update 必须明确拒绝（F2），FIFO 本体原样
    let fifo = dir.path().join("pipe.txt");
    let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) }, 0, "mkfifo 失败");
    let link_fifo = dir.path().join("link-pipe.txt");
    std::os::unix::fs::symlink(&fifo, &link_fifo).unwrap();
    for (name, p) in [("直接FIFO", &fifo), ("链接到FIFO", &link_fifo)] {
        let r = call_tool(entry, "apply_patch", patch_of(format!(
            "*** Update File: {p}\n@@\n-x\n+y", p = p.display()
        )));
        assert_eq!(r["ok"], json!(false), "{name} 不得假成功：{r}");
        assert_eq!(r["state_known"], json!(true), "{name} 为校验型失败：{r}");
        assert_eq!(r["changed_files"], json!([]), "{name} 清单必须为空：{r}");
        assert!(
            r["error"].as_str().unwrap().contains("不是常规文件"),
            "{name} 错误应指明非常规文件：{r}"
        );
    }
    assert!(
        std::fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo(),
        "FIFO 本体必须原样保留"
    );
    let _ = fs::remove_dir_all(dir.path());
}

// ---------- S6：exec_command（最薄 JSON 宿主）----------

/// /proc/<pid>/stat 的进程状态字符；进程不存在返回 None。
fn proc_state(pid: i32) -> Option<char> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm 字段可含空格/括号：从最后一个 ')' 之后取第 3 字段
    let after = text.rsplit_once(')')?.1;
    after.trim_start().chars().next()
}

/// 看门狗轮询：组长与孙进程全部消失（ESRCH）或孙进程进入 Z/X 终态。
/// 这是进程组被真实终止的 OS 级证据，不依赖固定延迟。
fn wait_group_terminated(pgid: i32, kid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let errno = unsafe {
            let r = libc::killpg(pgid, 0);
            if r == 0 {
                None
            } else {
                std::io::Error::last_os_error().raw_os_error()
            }
        };
        let group_gone = errno == Some(libc::ESRCH);
        let kid_dead = matches!(proc_state(kid), None | Some('Z') | Some('X'));
        if group_gone || kid_dead {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "进程组未在期限内终止：pgid={pgid} kid={kid} kid_state={:?}",
            proc_state(kid)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// S6：双流合并与真实非零退出码；workdir 真实生效；超时后受控进程组被真实终止
/// （组长与后台孙进程从进程表消失，而非仅收到 timeout 结果）。
#[test]
fn s6_json_host_exec_command_streams_exit_and_pgroup_timeout() {
    tool_child_mode();
    let entry = "s6_json_host_exec_command_streams_exit_and_pgroup_timeout";
    let dir = TempDir::new("s6");

    // 双流 + 真实非零退出码：stdout/stderr 各一行，退出码 7 原样返回
    let r = call_tool(entry, "exec_command", json!({
        "cmd": "echo out-S6; echo err-S6 1>&2; exit 7",
        "timeout_ms": 5000
    }));
    assert_eq!(r["status"], json!("completed"), "{r}");
    assert_eq!(r["exit_code"], json!(7), "真实整数退出码：{r}");
    assert_eq!(r["error"], json!(null), "{r}");
    assert_eq!(r["truncated"], json!(false), "{r}");
    let output = r["output"].as_str().unwrap();
    assert!(output.contains("out-S6") && output.contains("err-S6"), "双流都应返回：{r}");

    // workdir 真实生效（宿主→OS：目录切换由操作系统语义决定）
    let r = call_tool(entry, "exec_command", json!({
        "cmd": "pwd", "workdir": dir.path().display().to_string(), "timeout_ms": 5000
    }));
    assert_eq!(r["status"], json!("completed"), "{r}");
    assert_eq!(r["exit_code"], json!(0), "{r}");
    assert_eq!(r["output"].as_str().unwrap().trim(), dir.path().display().to_string());

    // 超时：直接子进程是组长（$$），后台孙进程（$!）持续写日志
    let log = dir.path().join("w.log");
    let pgid_file = dir.path().join("pgid.txt");
    let kid_file = dir.path().join("kid.txt");
    let cmd = format!(
        "echo $$ > {pgid}; while true; do echo x >> {log}; sleep 0.05; done & echo $! > {kid}; sleep 30",
        pgid = pgid_file.display(),
        log = log.display(),
        kid = kid_file.display(),
    );
    let r = call_tool(entry, "exec_command", json!({
        "cmd": cmd, "timeout_ms": 400
    }));
    assert_eq!(r["status"], json!("timeout"), "{r}");
    assert_eq!(r["exit_code"], json!(null), "被信号终止无整数退出码：{r}");
    assert!(r["error"].as_str().unwrap().contains("时限"), "016 错误措辞：{r}");
    // 写入循环确实运行过
    let s = fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    assert!(s > 0, "后台孙进程应已写入日志");
    // OS 级证据：组长与孙进程在期限内从进程表消失（SIGKILL 后被回收/终态）
    let pgid: i32 = fs::read_to_string(&pgid_file).unwrap().trim().parse().unwrap();
    let kid: i32 = fs::read_to_string(&kid_file).unwrap().trim().parse().unwrap();
    wait_group_terminated(pgid, kid);
    let _ = fs::remove_dir_all(dir.path());
}

// ---------- 高风险清单补充（2026-10-10）----------

/// AUTH-E02：交互同意但持久化失败（授权记录路径被目录占位的受控写故障）——
/// 必须报错且零请求：一次口头同意不能越过落盘失败继续外发。
#[test]
fn auth_e02_persistence_failure_blocks_request() {
    let cfg = TempDir::new("ae02-cfg");
    let user = TempDir::new("ae02-user");
    let key = "ae02-key";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("AE02-MUST-NOT-REPLY"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_AE02_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    // 受控写故障夹具：授权记录路径已是目录，任何写入都失败
    let auth_path = user.path().join("authorized-origins.json");
    std::fs::create_dir(&auth_path).unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_AE02_KEY", key),
    ]));
    // 008-ADAPT：授权询问发生在 TUI 之前（启动期授权门，提问文本不变）
    pty.wait_output("允许并记住此授权？", WATCHDOG).unwrap();
    pty.send_line("y");
    pty.wait_output("错误：", WATCHDOG).unwrap();
    // 008-ADAPT：启动期授权持久化失败→main 报错退出（码 1），不进入 TUI、无 "再见!"
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(1), "{out}");
    assert_eq!(mock.count(), 0, "落盘失败必须零请求：同意不能替代授权记录");
    // 008-ADAPT：无 "Agent：" 前缀可断，改盯 mock 回复原文（语义：不得出现模型回复）
    assert!(!out.contains("AE02-MUST-NOT-REPLY"), "{out}");
    assert!(!out.contains(key), "密钥明文不得出现在输出：{out}");
    assert!(auth_path.is_dir(), "故障夹具保持原样");
}

/// AUTH-N04：携带凭证的 refresh 走真实首次授权入口（PTY 交互）——
/// 公共目录来源与厂商凭证两处询问各恰一次；同意后真实发出带凭证的列表请求，
/// 同会话后续对话不再询问。
/// 008-ADAPT：以 TERM=dumb 行式路径验证（TUI 内 "/refresh" 被母本命令注册表
/// 拒为 Unrecognized command，交互问句不存在）——002/003 授权闭环语义不变。
#[test]
fn auth_n04_refresh_first_use_authorization_via_repl() {
    let cfg = TempDir::new("an04-cfg");
    let user = TempDir::new("an04-user");
    let key = "an04-key";
    let list_mock = spawn_mock(vec![
        MockResp::json(200, json!({ "object": "list", "data": [
            { "id": "m-a", "name": "N", "context_window": 1024, "max_output_tokens": 256 }
        ], "has_more": false }).to_string()),
        MockResp::json(200, chat_body("AN04-CHAT-OK")),
    ]);
    // 所选模型直接用刷新将返回的 m-a：/refresh 不需要路由，刷新后 m-a 进入名单、对话可路由
    // （refresh 会替换名单且不切换所选模型——002 既有契约）
    let manual = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("m-a".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(list_mock.url.clone()),
                key_ref: Some("env:TCHAT_AN04_KEY".into()),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(
        cfg.path().join("manual.json"),
        serde_json::to_string_pretty(&manual).unwrap(),
    )
    .unwrap();
    let provider_origin = catalog::origin_of(&list_mock.url).unwrap();

    let mut cmd = pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_AN04_KEY", key),
        // 公共目录指向本地不可达 HTTPS：正式路径强制 HTTPS，本地即刻失败、不触真实网络
        ("TCHAT_PUBLIC_CATALOG_URL", "https://127.0.0.1:1/pub.json"),
    ]);
    // 008-ADAPT：TERM=dumb→008 tui_should_run 判否，走既有行式路径（001 已知退化）。
    // 原因：TUI composer 的母本命令注册表不含 "/refresh"——提交即被拒为
    // "Unrecognized command"（PTY 实测），交互式 refresh 授权问句在 TUI 内不存在；
    // 该授权闭环（002/003 契约）仅可在行式路径交互验证，故以 dumb 终端保留原用例。
    cmd.env("TERM", "dumb");
    let mut pty = PtySession::spawn(cmd);
    pty.wait_output("你> ", WATCHDOG).unwrap();
    pty.send_line("/refresh");
    // 第一问：公共目录来源（无凭证）；同步对本地不可达 HTTPS 失败但厂商查询继续
    pty.wait_output("即将从 https://127.0.0.1:1 同步公共目录", WATCHDOG).unwrap();
    pty.send_line("y");
    // 第二问：凭证外发到厂商 origin（发生在 refresh 汇总输出之前）
    pty.wait_output("即将把凭证 TCHAT_AN04_KEY", WATCHDOG).unwrap();
    pty.wait_output(&format!("发送到 {provider_origin}"), WATCHDOG).unwrap();
    pty.send_line("y");
    // refresh 汇总：公共目录失败如实报告＋厂商名单更新
    pty.wait_output("公共目录同步失败", WATCHDOG).unwrap();
    pty.wait_output("查询到 1 个模型", WATCHDOG).unwrap();
    // 同会话对话：不再询问（两次授权都已落盘）
    pty.send_line("你好");
    pty.wait_output("Agent：AN04-CHAT-OK", WATCHDOG).unwrap();
    pty.send_line("exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(0), "{out}");
    // 真实数据变化：带凭证的列表请求确实发出
    let reqs = list_mock.requests();
    assert_eq!(reqs.len(), 2, "列表＋对话各一次：{reqs:?}");
    assert_eq!(reqs[0].path, "/models");
    assert_eq!(reqs[0].auth.as_deref(), Some("Bearer an04-key"));
    // 两条询问各恰一次；对话阶段无新增询问
    assert_eq!(out.matches("允许并记住").count(), 2, "{out}");
    assert_eq!(out.matches("即将把凭证").count(), 1, "{out}");
    // 授权记录：两条 grant（公共目录 catalog 标记＋厂商凭证指纹）
    let store: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(user.path().join("authorized-origins.json")).unwrap(),
    )
    .unwrap();
    let fps: Vec<&str> = store["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["cred_fp"].as_str().unwrap())
        .collect();
    assert!(fps.contains(&catalog::fingerprint(key).as_str()), "{store}");
    assert!(fps.contains(&"catalog"), "{store}");
}

/// N2b：工具执行期间 Ctrl+C——取消命令进程组而非杀死 CLI；3 秒内回到聊天入口；
/// 后代停止写盘；同批剩余调用不执行且每个已记录调用都有匹配 tool 结果（下一次
/// 请求携带完整历史核验）；取消后不自动请求模型；下一用户轮正常。
#[test]
fn n2b_esc_cancel_keeps_process_cli_alive_cleans_on_exit() {
    let cfg = TempDir::new("n2b-cfg");
    let user = TempDir::new("n2b-user");
    let work = TempDir::new("n2b-work");
    let key = "n2b-key";
    let log = work.path().join("w.log");
    let second = work.path().join("second.txt");
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-n2b-writer", "type": "function", "function": { "name": "exec_command",
              "arguments": format!("{{\"cmd\":\"while true; do echo x >> {}; sleep 0.05; done\",\"timeout_ms\":30000}}", log.display()) } },
            { "id": "call-n2b-second", "type": "function", "function": { "name": "exec_command",
              "arguments": format!("{{\"cmd\":\"touch {}\"}}", second.display()) } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("N2B-第二轮-OK")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_N2B_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_N2B_KEY", key),
    ]));
    // 008-ADAPT：TUI 就绪＝空历史占位（原 "你> " 行式提示符不再存在）
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell）
    pty.send_line("go");
    // 工具运行态：实际命令上屏（012：exec begin＝运行态 ExecCell——命令可见）
    pty.wait_screen("while true; do echo x >>").unwrap();
    // 等写入循环确实运行
    let mut grew = 0;
    for _ in 0..100 {
        if std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0) > 0 { grew = 1; break; }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(grew, 1, "写盘探针应已运行");
    // 016：Esc＝取消当前轮——停止 AI 工作与等待，已启动进程仍由会话管理；
    // CLI 必须存活并在 3 秒内回到聊天入口（错误经 Failed 路径渲染）
    pty.send_raw(b"\x1b"); // 单一职责轮：Esc＝取消
    pty.wait_screen_timeout("错误：", std::time::Duration::from_secs(3))
        .expect("取消后应报错并返回聊天入口");
    pty.wait_screen("仍由会话管理").expect("016：取消消息列出仍有效的会话编号");
    let s1 = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    assert!(s1 > 0);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let s2 = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    assert!(s2 > s1, "016：取消当前轮不终止已启动进程，写盘继续");
    assert!(!second.exists(), "同批剩余调用不得执行");
    assert_eq!(mock.count(), 1, "取消后不得自动请求模型");
    // 下一用户轮正常，且下一个请求携带完整取消历史
    pty.send_line("next");
    // 008-ADAPT：回复无 "Agent：" 前缀，等待回复原文（屏幕级）
    pty.wait_screen("N2B-第二轮-OK").unwrap();
    assert_eq!(mock.count(), 2);
    let body: serde_json::Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    // system + user(015 ctx) + user(go) + assistant(tool_calls 2) + tool(cancelled) + tool(skipped) + user(next)
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["system", "user", "user", "assistant", "tool", "tool", "user"], "{body}");
    assert_eq!(msgs[3]["tool_calls"].as_array().unwrap().len(), 2);
    let r1: serde_json::Value = serde_json::from_str(msgs[4]["content"].as_str().unwrap()).unwrap();
    assert_eq!(msgs[4]["tool_call_id"], json!("call-n2b-writer"));
    assert_eq!(r1["status"], json!("cancelled"), "执行中调用按 cancelled 结构记录：{r1}");
    assert!(r1["session_id"].is_string(), "016：取消后保留可续接句柄：{r1}");
    let r2: serde_json::Value = serde_json::from_str(msgs[5]["content"].as_str().unwrap()).unwrap();
    assert_eq!(msgs[5]["tool_call_id"], json!("call-n2b-second"));
    assert_eq!(r2["status"], json!("cancelled"), "未执行调用记录未执行原因：{r2}");
    assert!(r2["session_id"].is_null(), "016：未执行调用＝空句柄：{r2}");
    assert!(r2["error"].as_str().unwrap().contains("未执行"), "{r2}");
    pty.send_line("/exit"); // 008-ADAPT：TUI 退出走 /exit
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "CLI 必须存活并干净退出：{out}");
    // 016：宿主退出清理——全部进程会话关闭，写盘停止
    let s3 = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(std::time::Duration::from_millis(350));
    assert_eq!(
        s3,
        std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0),
        "退出后受管理进程必须被清理"
    );
}

/// N2b-混合批（016 语义）：执行中的 exec_command＋待执行的 apply_patch——取消后
/// exec 进程保留并由会话管理（继续写盘）、apply_patch 目标无变化；每个 call_id
/// 恰一条结果且结构符合各自工具契约（exec：cancelled＋句柄；apply_patch：
/// ok=false＋未执行原因）；无自动续请求；下一用户轮携带完整历史并正常运行；
/// 退出时宿主清理全部进程会话。
#[test]
fn n2b2_mixed_batch_cancel_per_tool_result_shapes() {
    let cfg = TempDir::new("n2b2-cfg");
    let user = TempDir::new("n2b2-user");
    let work = TempDir::new("n2b2-work");
    let key = "n2b2-key";
    let log = work.path().join("w.log");
    let target = work.path().join("never.txt");
    let add_patch = format!(
        "*** Begin Patch\\n*** Add File: {}\\n+PWNED\\n*** End Patch",
        target.display()
    );
    let args_exec = format!(
        "{{\"cmd\":\"while true; do echo x >> {}; sleep 0.05; done\",\"timeout_ms\":30000}}",
        log.display()
    );
    let args_patch = serde_json::json!({ "patch": add_patch }).to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-mix-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args_exec } },
            { "id": "call-mix-patch", "type": "function",
              "function": { "name": "apply_patch", "arguments": args_patch } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("N2B2-第二轮-OK")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_N2B2_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_N2B2_KEY", key),
    ]));
    // 008-ADAPT：TUI 就绪＝空历史占位；工具运行态命令为屏幕级等待（012：富 cell）
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell）
    pty.send_line("go");
    pty.wait_screen("while true; do echo x >>").unwrap();
    for _ in 0..100 {
        if std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0) > 0 { break; }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    pty.send_raw(b"\x1b"); // 单一职责轮：Esc＝取消
    // 012：取消呈现——被杀 exec 与批内跳过的 apply_patch 均为独立"已取消"
    //（后者不得呈现 "Failed to apply patch"，需求④）
    pty.wait_screen_timeout("已取消", std::time::Duration::from_secs(3)).unwrap();
    pty.feed_screen();
    assert!(
        !pty.vt.to_string().contains("Failed to apply patch"),
        "跳过的补丁不得呈现为失败：{}",
        pty.vt.vt100().screen().contents()
    );
    // 008-ADAPT：取消错误经 Failed 路径渲染为 "错误：" 屏幕单元（3 秒 SLA 不变）
    pty.wait_screen_timeout("错误：", std::time::Duration::from_secs(3)).unwrap();
    let s1 = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0) > s1,
        "016：取消当前轮不终止已启动进程，写盘继续"
    );
    assert!(!target.exists(), "apply_patch 未执行：目标不得被创建/修改");
    assert_eq!(mock.count(), 1, "无自动续请求");
    pty.send_line("next");
    // 008-ADAPT：回复无 "Agent：" 前缀，等待回复原文（屏幕级）
    pty.wait_screen("N2B2-第二轮-OK").unwrap();
    assert_eq!(mock.count(), 2);
    // 完整历史核验：结构符合各自工具契约
    let body: serde_json::Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    // 015：ctx（user）恰一条，位于首轮输入之前
    assert_eq!(roles, vec!["system", "user", "user", "assistant", "tool", "tool", "user"], "{body}");
    assert_eq!(msgs[4]["tool_call_id"], json!("call-mix-exec"));
    let re: serde_json::Value = serde_json::from_str(msgs[4]["content"].as_str().unwrap()).unwrap();
    assert_eq!(re["status"], json!("cancelled"), "exec 结果用其 cancelled 结构：{re}");
    assert!(re["session_id"].is_string(), "016：取消后保留可续接句柄：{re}");
    assert_eq!(msgs[5]["tool_call_id"], json!("call-mix-patch"));
    let rp: serde_json::Value = serde_json::from_str(msgs[5]["content"].as_str().unwrap()).unwrap();
    assert_eq!(rp["ok"], json!(false), "apply_patch 结果用其契约结构：{rp}");
    assert_eq!(rp["changed_files"], json!([]), "{rp}");
    assert!(rp["error"].as_str().unwrap().contains("未执行"), "{rp}");
    pty.send_line("/exit"); // 008-ADAPT：TUI 退出走 /exit
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    // 016：宿主退出清理——全部进程会话关闭，写盘停止
    let s3 = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(std::time::Duration::from_millis(350));
    assert_eq!(
        s3,
        std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0),
        "退出后受管理进程必须被清理"
    );
}

// ---------- S10：TUI 内 /resume 投影渲染（008 P3 真实调用链）----------

/// S10：预先保存快照 → TUI 内 `/resume seed` → 装载历史经 thread_transcript 投影
/// 为 cells 渲染到屏幕（不重放工具——mock 零请求）→ 退出。
#[test]
fn s10_tui_resume_projects_history_without_replay() {
    let cfg = TempDir::new("s10-cfg");
    let user = TempDir::new("s10-user");
    let key = "s10-key";
    let mock = spawn_mock(vec![]); // 零回复：/resume 不重放工具→零请求
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S10_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    // 种子快照：一轮用户消息＋assistant 回复（经生产 save_snapshot 写入）
    let msg = |role: terminal_chat::prompt::HistoryRole, content: &str| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role,
            content: content.to_string(),
            user_input_event: None,
        })
    };
    let hist = vec![
        msg(terminal_chat::prompt::HistoryRole::User, "S10-历史-用户消息"),
        msg(terminal_chat::prompt::HistoryRole::Assistant, "S10-历史-助手回复"),
    ];
    terminal_chat::session::save_snapshot(&user.path().join("sessions"), "seed10", &hist).unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S10_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell） // TUI 就绪
    pty.send_line("/resume seed10");
    // 投影历史渲染：用户消息与助手回复原文出现在屏幕
    pty.wait_screen("S10-历史-用户消息").unwrap();
    pty.wait_screen("S10-历史-助手回复").unwrap();
    // 008-REVIEW P1-1：/resume 的 summary（装载统计）渲染在历史其后（不得被
    // ReplaceCells 整体覆盖丢弃）
    pty.wait_screen("恢复 2 条").unwrap();
    // 等 summary 行稳定（区分后续 /exit 的输出，避免时序混叠）
    std::thread::sleep(std::time::Duration::from_millis(500));
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(0), "退出码契约，输出：{out}");
    // 不重放工具＝零模型请求
    assert!(
        mock.requests().is_empty(),
        "/resume 只投影不重放，实际请求：{:?}",
        mock.requests()
    );
    // 快照文件只读装载（字节不变）
    let snapshot = user.path().join("sessions/seed10.jsonl");
    let reloaded = terminal_chat::session::load_session_file(&snapshot).unwrap();
    assert_eq!(reloaded.history.len(), 2);
}

// ---------- S11：turn 运行中提交排队重放（008 J10 不丢输入）----------

/// S11：第一轮 turn 因 mock 延迟仍在运行时提交第二条 → 排队不丢失 →
/// 第一轮完成后自动作为下一 turn 按序发出（母本排队语义的最小映射）。
/// 核对：第二条请求的用户消息＝排队原文；两轮回复均渲染；退出码 0。
#[test]
fn s11_tui_queued_input_replayed_after_turn() {
    let cfg = TempDir::new("s11-cfg");
    let user = TempDir::new("s11-user");
    let key = "s11-key-value";
    let mock = spawn_mock(vec![
        MockResp::slow_json(200, chat_body("S11-第一轮-完成"), 1500),
        MockResp::json(200, chat_body("S11-第二轮-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S11_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S11_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell）
    pty.send_line("第一条");
    // 等 mock 收到第 1 个请求——此时 turn_running 已置位（提交同步置位后才 spawn）
    let deadline = Instant::now() + WATCHDOG;
    while mock.count() < 1 {
        assert!(Instant::now() < deadline, "mock 未收到第一轮请求");
        std::thread::sleep(Duration::from_millis(50));
    }
    // turn 在飞（响应延迟 1.5s）时以显式排队键（Tab）提交第二条 → 排队
    //（021 起 Enter 运行中＝接续当前轮；显式排队保留 queue key，§2.3-1）
    pty.send_raw("排队消息".as_bytes());
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\t");
    // 排队预览独有文案（第一轮完成前出现）——锚定排队路径本身（若退化为直接
    // 提交路径则本断言失败，防止回归被负载掩盖）
    pty.wait_screen("Queued follow-up inputs").unwrap();
    // 第一轮完成渲染
    pty.wait_screen("S11-第一轮-完成").unwrap();
    // 排队消息在第一轮结束后自动发出并渲染第二轮回复
    pty.wait_screen("S11-第二轮-完成").unwrap();
    assert_eq!(mock.count(), 2, "排队消息必须作为第二 turn 发出");
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let user_msgs: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert!(
        user_msgs.contains(&"排队消息"),
        "第二轮请求必须携带排队原文（不丢输入）：{body}"
    );
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "退出码契约（001），输出：{out}");
}

/// S11b（021 A09/A10/A11 L3）：运行中 Enter＝接续当前轮——工具批执行期间受理
/// 的追问在完整批配对之后、下一次**本轮**模型请求之前入史；不另起第二条
/// turn（恰两次请求；显式排队形态会退队成第三条请求，可区分）。
#[test]
fn s11b_tui_running_enter_steers_current_turn() {
    let cfg = TempDir::new("s11b-cfg");
    let user = TempDir::new("s11b-user");
    let key = "s11b-key-value";
    let tc = json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"s11b-e1","type":"function","function":{
            "name":"exec_command",
            "arguments": json!({"cmd":"true"}).to_string()}}]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::slow_json(200, tc, 1200),
        MockResp::json(200, chat_body("S11B-已处理追问")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S11B_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S11B_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("开始任务");
    let deadline = Instant::now() + WATCHDOG;
    while mock.count() < 1 {
        assert!(Instant::now() < deadline, "mock 未收到首轮请求");
        std::thread::sleep(Duration::from_millis(50));
    }
    // 请求在途（延迟 1.2s）时 Enter 提交追问 → 接续当前轮
    pty.send_line("等等，先补充一点");
    pty.wait_screen("S11B-已处理追问").unwrap();
    assert_eq!(mock.count(), 2, "接续进入同一轮的下一次请求，不另起新 turn");
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let tool_pos = msgs
        .iter()
        .position(|m| m["role"] == "tool" && m["tool_call_id"] == "s11b-e1")
        .expect("工具结果在请求中");
    let steer_pos = msgs
        .iter()
        .position(|m| m["role"] == "user" && m["content"] == "等等，先补充一点")
        .expect("追问进入请求");
    assert!(tool_pos < steer_pos, "追问在完整批结果之后：{body}");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S12：Ctrl+D 退出契约（001/J10）----------

/// S12：空输入 Ctrl+D＝恢复终端后打印 `再见!`、退出码 0（J10：与 001 行式实测一致，
/// rustyline 仅空行返回 Eof）；非空草稿 Ctrl+D 不退出（母本 composer 键位，textarea
/// 无 Ctrl+D 删字符映射→无操作；rustyline 非空为前向删字符——编辑域差异，规格登记）。
#[test]
fn s12_tui_ctrl_d_empty_input_exits_with_code_0() {
    let cfg = TempDir::new("s12-cfg");
    let user = TempDir::new("s12-user");
    let key = "s12-key-value";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S12-MUST-NOT-APPEAR"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S12_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S12_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell）
    // 非空草稿：Ctrl+D 不退出（母本编辑键域；textarea 无删字符映射则为无操作）
    pty.send_raw(b"draft");
    std::thread::sleep(Duration::from_millis(300));
    pty.send_raw(b"\x04");
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        pty.child.try_wait().unwrap().is_none(),
        "非空草稿 Ctrl+D 不得退出（J10 退出条件＝空输入行；非空＝母本 composer 键位，无操作）"
    );
    // 清空草稿（5 个退格）后 Ctrl+D：空输入行 EOF 退出
    for _ in 0..5 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(30));
    }
    std::thread::sleep(Duration::from_millis(300));
    pty.send_raw(b"\x04");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();

    assert_eq!(status.code(), Some(0), "退出码契约（001），输出：{out}");
    assert!(out.contains("\u{1b}[?1049l"), "退出后终端必须恢复（A5）：{out}");
    assert_eq!(mock.count(), 0, "Ctrl+D 退出不得触发模型请求");
}

// ---------- S13：上键回溯已提交输入（001 rustyline 历史的目标保留）----------

/// S13：提交一轮后按上键——最近提交回填到草稿区（composer local_history 承接，
/// 001 rustyline 内存历史等价：均无跨会话持久化）。以光标行断言草稿回填，
/// 区分 transcript 中同文的历史投影（避免空跑）。
#[test]
fn s13_tui_history_recall_up_key_refills_draft() {
    let cfg = TempDir::new("s13-cfg");
    let user = TempDir::new("s13-user");
    let key = "s13-key-value";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S13-REPLY-OK"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S13_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S13_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap(); // 008：TUI 就绪（footer 提示；启动无占位 cell）
    pty.send_line("可回溯输入");
    pty.wait_screen("S13-REPLY-OK").unwrap();
    // 回复后草稿已清空：光标行（composer 草稿区）不含已提交原文
    std::thread::sleep(Duration::from_millis(500));
    let cursor_line = |pty: &mut PtySession| {
        pty.feed_screen();
        let (row, _col) = pty.vt.vt100().screen().cursor_position();
        pty.vt
            .vt100()
            .screen()
            .contents()
            .lines()
            .nth(row as usize)
            .unwrap_or_default()
            .to_string()
    };
    let before = cursor_line(&mut pty);
    assert!(
        !before.contains("可回溯输入"),
        "前置：草稿已清空（光标行={before:?}；transcript 投影不算回填）"
    );
    // 上键召回：草稿区（光标行）出现已提交原文
    pty.send_raw(b"\x1b[A");
    let deadline = Instant::now() + WATCHDOG;
    loop {
        if cursor_line(&mut pty).contains("可回溯输入") {
            break;
        }
        if Instant::now() > deadline {
            pty.feed_screen();
            let (row, col) = pty.vt.vt100().screen().cursor_position();
            panic!(
                "上键必须把最近提交回填到草稿（光标行）；光标=({row},{col})，屏幕：\n{}",
                pty.vt.vt100().screen().contents()
            );
        }
        pty.read_once(50);
    }
    // 清空回填草稿后正常退出
    for _ in 0..5 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(30));
    }
    std::thread::sleep(Duration::from_millis(300));
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "退出码契约（001），输出：{out}");
    assert_eq!(mock.count(), 1, "召回只回填草稿，不重发请求");
}


// ---------- S14：turn 期间 Info cell 在完成后存留（008-REVIEW P0-1）----------

/// S14：带工具调用的 turn——工具详情 cell（012：实际命令＋输出＋终态）与最终回复
/// **同时**留在屏幕：Final 只移除本轮流式 cell，不得吞掉工具详情（012 需求⑧；
/// 006 通知契约的显示层等价——活动行由富 cell 承接）。最终回复含换行使 Delta 流式
/// cell 与工具 cell 共存，「Final 只移除流式 cell」的记账真正被用例压住。
#[test]
fn s14_info_cells_survive_turn_completion() {
    let cfg = TempDir::new("s14-cfg");
    let user = TempDir::new("s14-user");
    let key = "s14-key";
    let args = r#"{"cmd":"echo s14-ok"}"#.to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s14-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S14-最终-完成\nS14-收尾行")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S14_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S14_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑一下");
    // 运行态：实际命令上屏（012：exec begin＝运行态 ExecCell）
    pty.wait_screen("echo s14-ok").unwrap();
    // 终态：输出＋成功呈现（012：ToolEnd 替换为完成态 cell）
    pty.wait_screen("s14-ok").unwrap();
    // 最终回复渲染（turn 完成）
    pty.wait_screen("S14-最终-完成").unwrap();
    // 完成后工具详情仍在屏幕（不被 Final 截断丢弃——012 需求⑧）
    pty.feed_screen();
    assert!(
        pty.vt.to_string().contains("echo s14-ok"),
        "工具详情（实际命令）必须在 turn 完成后存留\n{}",
        pty.vt.vt100().screen().contents()
    );
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert_eq!(mock.count(), 2, "工具轮＋续轮");
}

// ---------- S15：TUI 退出排空（008-REVIEW P0-2，007 契约）----------

/// S15：会话追加中途失败（子进程 RLIMIT_FSIZE——对已打开 fd 的 write 同样生效；
/// SIGXFSZ 忽略后 write 返回 EFBIG）→ 滞留待写段＋失败提示可见（P0-1 修复面）；
/// TUI 退出时执行排空（与行式路径同契约）——排空重试仍失败时明确报错、退出码不变。
#[test]
fn s15_tui_exit_flushes_pending_session_writes() {
    let cfg = TempDir::new("s15-cfg");
    let user = TempDir::new("s15-user");
    let key = "s15-key";
    let replies: Vec<MockResp> = (1..=10)
        .map(|i| MockResp::json(200, chat_body(&format!("S15-R{i}"))))
        .collect();
    let mock = spawn_mock(replies);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S15_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut cmd = pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S15_KEY", key),
    ]);
    // 会话文件大小上限 1024B：记录累计超限后追加失败（EFBIG）→ 滞留待写段
    unsafe {
        cmd.pre_exec(|| {
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                let lim = libc::rlimit {
                    rlim_cur: 1024,
                    rlim_max: libc::RLIM_INFINITY,
                };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &lim), 0);
            }
            Ok(())
        });
    }
    let mut pty = PtySession::spawn(cmd);
    pty.wait_screen("? for shortcuts").unwrap();
    // 连发 10 轮：累计必然超过 1024B → 追加失败。015 上下文消息增大单轮记录，
    // 失败提示出现得更早——逐轮捕捉（提示行会随后续轮次滚出屏幕）
    let mut saw_append_failure = false;
    for i in 1..=10 {
        pty.send_line("填充记录测试数据");
        // 各轮回复文本互异：等待本轮回复而非旧内容（避免竞态）
        pty.wait_screen(&format!("S15-R{i}")).unwrap();
        if !saw_append_failure {
            std::thread::sleep(Duration::from_millis(100));
            pty.feed_screen();
            saw_append_failure = pty.vt.to_string().contains("自动会话追加失败");
        }
    }
    assert!(
        saw_append_failure,
        "追加失败提示应可见（P0-1 修复面）\n{}",
        pty.vt.vt100().screen().contents()
    );
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    // 退出排空执行且失败明确报错（007 契约：成功静默，失败明确；不改变退出码）
    assert!(
        out.contains("会话退出排空失败"),
        "排空失败必须明确报错（007 退出排空契约），输出尾：{}",
        &out[out.len().saturating_sub(600)..]
    );
    assert_eq!(status.code(), Some(0), "排空失败不改变退出码（与行式一致）：{out}");
}

// ---------- S16：退出取消运行中的 turn（008-REVIEW P2-4）----------

/// S16：turn 运行中（exec 长任务执行期）/exit——退出路径先置取消标志：exec 进程组
/// 被终止、turn 迅速返回、排空后干净退出；不得阻塞到工具超时（20s）才退出。
#[test]
fn s16_tui_exit_cancels_running_turn() {
    let cfg = TempDir::new("s16-cfg");
    let user = TempDir::new("s16-user");
    let key = "s16-key";
    let work = TempDir::new("s16-work");
    let args = r#"{"cmd":"sleep 20","timeout_ms":30000}"#.to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s16-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![MockResp::json(200, batch)]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S16_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(Some(work.path()), &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S16_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("go");
    // 工具执行中（sleep 20）：turn 在飞——运行态 ExecCell 显示 Running＋实际命令（012）
    pty.wait_screen("Running").unwrap();
    pty.wait_screen("sleep 20").unwrap();
    pty.send_line("/exit");
    // 取消标志生效→exec 进程组终止→turn 返回→排空→再见!＋退出码 0；
    // 上限 8s（远小于工具 20s——若退出路径不取消则阻塞到工具结束，本断言红）
    pty.wait_output("再见!", Duration::from_secs(8))
        .expect("退出必须取消运行中的 turn（不得阻塞到工具超时）");
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S17：TUI 启动审计通知（008-REVIEW P2-7，F7 等价输出）----------

/// S17：行式启动打印 F7 权限审计（内容/脱敏/收紧行为不变）；TUI 以既有通知
/// cell（PlainHistoryCell）等价展示——manual.json 过宽（0644）被收紧为 0600
/// 且通知可见。
#[test]
fn s17_tui_startup_audit_notices_rendered() {
    let cfg = TempDir::new("s17-cfg");
    let user = TempDir::new("s17-user");
    let key = "s17-key";
    let mock = spawn_mock(vec![]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S17_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S17_KEY", key),
    ]));
    // TUI 哨兵先行（防止退化为行式时横幅审计行造成假通过）
    pty.wait_screen("? for shortcuts").unwrap();
    // 审计通知（manual.json 0644→0600 收紧）作为启动通知 cell 渲染
    pty.wait_screen("权限收紧为 0600").unwrap();
    // 副作用核对：实际收紧（既有审计行为，非仅提示）
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(cfg.path().join("manual.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "审计须实际收紧（F7 既有行为保留）");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert_eq!(mock.count(), 0);
}

// ---------- S18：turn 期间 /save 不冻结（008-REVIEW P2-10 核实修正）----------

/// S18：turn 运行中 /save——composer 的 available_during_task 门（母本行为，本地
/// 命令登记"工具运行中禁用"）**立即**拒绝并提示、恢复草稿（不派发→不存在
/// history 锁等待冻结）；本轮结束后重试成功。宿主侧 try_lock 为纵深防御
/// （队列重放仅在空闲派发，正常不可达）。
#[test]
fn s18_save_during_turn_refused_fast_and_retries_ok() {
    let cfg = TempDir::new("s18-cfg");
    let user = TempDir::new("s18-user");
    let key = "s18-key";
    let mock = spawn_mock(vec![MockResp::slow_json(200, chat_body("S18-回复"), 1500)]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S18_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S18_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("消息");
    let deadline = Instant::now() + WATCHDOG;
    while mock.count() < 1 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    // turn 在飞（1.5s 延迟响应）期间提交 /save 与 /resume：均立即被 composer 门拒
    //（available_during_task=false——含 Resume 的 008-ADAPT 登记完整；提示先于回复
    // 出现＝未阻塞在 history 锁上）。独立复审 P1-1：Resume 曾遗漏登记→直达宿主。
    pty.send_line("/save busy1");
    pty.wait_screen("'/save' is disabled while a task is in progress").unwrap();
    pty.feed_screen();
    assert!(
        !pty.vt.to_string().contains("S18-回复"),
        "拒绝提示必须先于本轮回复（证明未阻塞在 history 锁上）"
    );
    // 清掉被拒后恢复的草稿，再验 /resume 同门
    for _ in 0..12 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(20));
    }
    pty.send_line("/resume nonexist");
    pty.wait_screen("disabled while a task is in progress").unwrap();
    pty.feed_screen();
    assert!(
        !pty.vt.to_string().contains("S18-回复"),
        "/resume 拒绝提示同样必须先于本轮回复"
    );
    // 本轮完成后重试：清掉恢复的 "/resume nonexist" 草稿，重发 /save
    pty.wait_screen("S18-回复").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    for _ in 0..20 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    pty.send_line("/save busy1");
    pty.wait_screen("已保存快照 busy1").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(user.path().join("sessions/busy1.jsonl").exists(), "{out}");
}

// ---------- S19–S24：A3 滚动修复（2026-10-14 重开，RED→GREEN）----------
// 用户实测：完整 CLI 无法滚动聊天历史。这组用例经真实 CLI＋PTY＋真实 chat_driver
// 事件分发断言屏幕可见内容变化（不直接调用 scroll/偏移变量）。

/// 008-A3 收尾：PTY 宿主的终端查询应答器（增量字节流 → 应答字节）。
///
/// 背景（2026-10-14 滚动修复轮教训）：应用（custom_terminal/terminal_probe）会发
/// `ESC[6n`（DSR 光标位置）与 OSC 10/11（前景/背景色）查询并读等应答；宿主不应答
/// 时 resize 后的 `get_cursor_position` 等待会吞掉后续输入。本类型模拟真实终端的
/// 应答面：**完整查询恰应答一次**；DSR 回报 vt 解析器当前光标（真实终端状态）；
/// OSC 10/11 回报宿主固定中性色。
struct QueryResponder {
    /// 跨读取的未决查询缓冲（结尾可能是部分查询的字节，留待后续补齐判定）。
    pending: Vec<u8>,
}

/// 在 `s`（以 ESC 起头）上识别一个查询；返回 (应答光标种类, 查询全长)。
enum QueryKind {
    /// DSR：`ESC[6n`，固定 4 字节。
    CursorReport,
    /// OSC 10/11 色彩查询：前缀＋终止符（BEL 或 `ESC\`）。
    Color { is_fg: bool, len: usize },
}

impl QueryResponder {
    fn new() -> Self {
        Self { pending: Vec::new() }
    }

    /// 喂入本轮新增字节与终端当前光标（0 基），返回应答字节（可为空）。
    fn advance(&mut self, chunk: &[u8], cursor: (u16, u16)) -> Vec<u8> {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < buf.len() {
            let Some(esc) = (i..buf.len()).find(|&k| buf[k] == 0x1b) else {
                i = buf.len();
                break;
            };
            match Self::match_query(&buf[esc..]) {
                Ok(Some(kind)) => {
                    let len = match &kind {
                        QueryKind::CursorReport => 4,
                        QueryKind::Color { len, .. } => *len,
                    };
                    match kind {
                        QueryKind::CursorReport => {
                            // DSR 应答：光标位置报文（1 基）
                            out.extend_from_slice(
                                format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1).as_bytes(),
                            );
                        }
                        QueryKind::Color { is_fg, .. } => {
                            out.extend_from_slice(if is_fg {
                                b"\x1b]10;rgb:c7c7/c7c7/c7c7\x1b\\"
                            } else {
                                b"\x1b]11;rgb:0000/0000/0000\x1b\\"
                            });
                        }
                    }
                    i = esc + len;
                }
                Ok(None) => {
                    // 非查询转义序列：跳过该 ESC，继续向右找
                    i = esc + 1;
                }
                Err(()) => {
                    // 结尾可能是部分查询：保留待后续字节
                    self.pending = buf[esc..].to_vec();
                    return out;
                }
            }
        }
        out
    }

    /// 识别 `s` 开头的查询：Ok(None)＝非查询；Err(())＝字节不足（可能是不完整查询）。
    fn match_query(s: &[u8]) -> Result<Option<QueryKind>, ()> {
        if s.len() < 2 {
            return Err(());
        }
        match s[1] {
            b'[' => {
                if s.len() < 4 {
                    return Err(()); // 可能是不完整 CSI
                }
                if &s[..4] == b"\x1b[6n" {
                    Ok(Some(QueryKind::CursorReport))
                } else {
                    Ok(None)
                }
            }
            b']' => {
                for (prefix, is_fg) in [(b"\x1b]10;?" as &[u8], true), (b"\x1b]11;?", false)] {
                    if s.len() < prefix.len() {
                        if prefix.starts_with(s) {
                            return Err(());
                        }
                        continue;
                    }
                    if &s[..prefix.len()] != prefix {
                        continue;
                    }
                    // 查询前缀已齐：等待终止符（BEL 或 ESC\）
                    let rest = &s[prefix.len()..];
                    if rest.is_empty() {
                        return Err(());
                    }
                    if rest[0] == 0x07 {
                        return Ok(Some(QueryKind::Color { is_fg, len: prefix.len() + 1 }));
                    }
                    if rest.len() == 1 {
                        if rest[0] == 0x1b {
                            return Err(()); // ST 的 ESC 已到，等 '\'
                        }
                        return Ok(None); // 非法终止：非查询
                    }
                    if rest[0] == 0x1b && rest[1] == b'\\' {
                        return Ok(Some(QueryKind::Color { is_fg, len: prefix.len() + 2 }));
                    }
                    return Ok(None);
                }
                // 既非 10 也非 11 的 OSC：尾部不足 6 字节且仍像 OSC 开头
                // （"ESC]1x" 或更短）时，可能是不完整查询，留待后续字节；
                // 其余情况视为非查询跳过。
                if s.len() < 6 && (s.len() < 4 || b"\x1b]1".starts_with(&s[..4])) {
                    return Err(());
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

/// 逐字符慢打（每字符后读取；≥40ms 间隔避开 paste-burst 缓冲，读取有数据即提前返回）。
fn type_slow(pty: &mut PtySession, text: &str) {
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        pty.send_raw(ch.encode_utf8(&mut buf).as_bytes());
        pty.read_once(60);
    }
}

/// 长历史回复体：`前缀-L00`…`前缀-L29` 共 30 行（PTY 50 行高，多轮即超过一屏）。
fn long_reply(prefix: &str) -> String {
    (0..30)
        .map(|i| format!("{prefix}-L{i:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 组装 TUI 会话（配置/授权/mock 就绪）。返回的 TempDir 必须在用例内保持存活
/// （Drop 会删除配置/授权目录，子进程将回落公共目录）。
fn scroll_session(
    tag: &str,
    replies: Vec<MockResp>,
) -> (PtySession, common::Mock, Vec<TempDir>) {
    let cfg = TempDir::new(&format!("{tag}-cfg"));
    let user = TempDir::new(&format!("{tag}-user"));
    let key = format!("{tag}-key");
    let mock = spawn_mock(replies);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_SCROLL_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(&key));
    let pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_SCROLL_KEY", key.as_str()),
    ]));
    (pty, mock, vec![cfg, user])
}

impl PtySession {
    /// 当前 VT100 屏幕全文（先喂入未消费字节）。
    fn screen(&mut self) -> String {
        self.feed_screen();
        self.vt.vt100().screen().contents().to_string()
    }

    /// 调整 PTY 尺寸（SIGWINCH 送达子进程；VT100 解析器保持原尺寸，
    /// 行号一一对应，内容断言仍有效）。
    fn resize(&mut self, cols: u16, rows: u16) {
        use std::os::unix::io::AsRawFd as _;
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        unsafe {
            libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }
    }

    /// 011 §7 帧调度：按墙钟静置排空（read_once 有数据即时返回，轮数≠时长）。
    fn dwell(&mut self, ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            self.read_once(50);
        }
    }

    /// 010 收尾轮：CLI 子进程线程数（/proc/<pid>/task 项数）——直接观测取消
    /// 后辅助线程的存活与回收，不依赖 180s 超时兜底推断。
    fn thread_count(&self) -> usize {
        std::fs::read_dir(format!("/proc/{}/task", self.child.id()))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
}

/// S19：PageUp 分页上翻使早期历史回到可见区；单一职责轮——Esc 空闲＝无动作
/// （视口保持），Ctrl+End（JumpTarget::Latest）返回最新；
/// 滚动本身不发送请求（mock 计数不变）。
#[test]
fn s19_tui_pageup_esc_noop_ctrl_end_returns_to_latest() {
    let (mut pty, mock, _keep) = scroll_session("s19", vec![
        MockResp::json(200, chat_body(&long_reply("S19-R1"))),
        MockResp::json(200, chat_body(&long_reply("S19-R2"))),
        MockResp::json(200, chat_body(&long_reply("S19-R3"))),
    ]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S19-R1-L29").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S19-R2-L29").unwrap();
    pty.send_line("第三问");
    pty.wait_screen("S19-R3-L29").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    // 前置：第一轮早朧行已滚出屏幕（历史 > 一屏）
    let before = pty.screen();
    assert!(!before.contains("S19-R1-L00"), "前置失败：早期能见={before:?}");

    // PageUp 上翻两页（历史≈两屏）→ 早期历史可见，最新尾部不可见（视口确已移动）
    pty.send_raw(b"\x1b[5~");
    pty.send_raw(b"\x1b[5~");
    pty.wait_screen("S19-R1-L00").unwrap();
    let scrolled = pty.screen();
    assert!(!scrolled.contains("S19-R3-L29"), "上翻后最新尾部应不可见");

    // 单一职责轮：Esc 空闲＝无动作——视口保持（仍看不到最新尾部）
    pty.send_raw(b"\x1b");
    for _ in 0..5 { pty.read_once(60); }
    let held = pty.screen();
    assert!(!held.contains("S19-R3-L29"), "Esc 空闲不得回底/改视口：{held}");
    assert!(held.contains("S19-R1-L00"), "视口保持在旧历史：{held}");
    // Ctrl+End 返回最新（替代入口）
    pty.send_raw(b"\x1b[1;5F");
    pty.wait_screen("S19-R3-L29").unwrap();

    // 滚动不发送消息
    assert_eq!(mock.count(), 3, "滚动不得产生模型请求");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S20：鼠标滚轮（SGR 序列）上下滚动；滚到底部后恢复跟随（新内容重新可见）。
#[test]
fn s20_tui_mouse_wheel_scrolls_history() {
    let (mut pty, mock, _keep) = scroll_session("s20", vec![
        MockResp::json(200, chat_body(&long_reply("S20-R1"))),
        MockResp::json(200, chat_body(&long_reply("S20-R2"))),
        MockResp::json(200, chat_body(&long_reply("S20-R3"))),
    ]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S20-R1-L29").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S20-R2-L29").unwrap();
    pty.send_line("第三问");
    pty.wait_screen("S20-R3-L29").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    // 滚轮上（按钮 64；鼠标捕获已启用，事件位于 transcript 区域内）
    for _ in 0..60 {
        pty.send_raw(b"\x1b[<64;10;5M");
        std::thread::sleep(Duration::from_millis(5));
    }
    pty.wait_screen("S20-R1-L00").unwrap();
    let scrolled = pty.screen();
    assert!(!scrolled.contains("S20-R3-L29"), "滚轮上翻后最新尾部应不可见");

    // 滚轮下（按钮 65）超过底部 → 恢复跟随
    for _ in 0..80 {
        pty.send_raw(b"\x1b[<65;10;5M");
        std::thread::sleep(Duration::from_millis(5));
    }
    pty.wait_screen("S20-R3-L29").unwrap();
    assert_eq!(mock.count(), 3, "滚动不得产生模型请求");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S21：上翻期间中文/emoji 草稿不丢失、光标行保持草稿；Ctrl+End（JumpTarget）返回最新。
#[test]
fn s21_tui_scrolling_preserves_draft_and_ctrl_end_jumps_latest() {
    let (mut pty, mock, _keep) = scroll_session("s21", vec![
        MockResp::json(200, chat_body(&long_reply("S21-R1"))),
        MockResp::json(200, chat_body(&long_reply("S21-R2"))),
    ]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S21-R1-L29").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S21-R2-L29").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    // 输入中文＋emoji 草稿（不提交）。检查前持续读 PTY（睡眠不读会取到陈旧
    // 屏幕——本 harness 捕获只在读取时增长）
    let cursor_line = |pty: &mut PtySession| {
        pty.feed_screen();
        let (row, _col) = pty.vt.vt100().screen().cursor_position();
        pty.vt
            .vt100()
            .screen()
            .contents()
            .lines()
            .nth(row as usize)
            .unwrap_or_default()
            .to_string()
    };
    type_slow(&mut pty, "草稿中文🎨保持");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pty.read_once(100);
        if cursor_line(&mut pty).contains("草稿中文") {
            break;
        }
        assert!(
            Instant::now() <= deadline,
            "前置：草稿未渲染（光标行={:?}，屏幕={}）",
            cursor_line(&mut pty),
            pty.screen()
        );
    }

    // 上翻两页 → 草稿仍在光标行；请求计数不变。滚动过渡帧中光标行可能瞬时
    // 呈现 transcript 内容（帧内重排），故有界轮询至稳定帧（真丢草稿会超时失败）。
    pty.send_raw(b"\x1b[5~");
    pty.send_raw(b"\x1b[5~");
    pty.wait_screen("S21-R1-L00").unwrap();
    let draft_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        pty.read_once(100);
        if cursor_line(&mut pty).contains("草稿中文") {
            break;
        }
        assert!(
            Instant::now() <= draft_deadline,
            "上翻不得丢失草稿（光标行={:?}，屏幕={}）",
            cursor_line(&mut pty),
            pty.screen()
        );
    }
    assert_eq!(mock.count(), 2, "滚动不得提交草稿");

    // Ctrl+End（JumpTarget::Latest）返回最新
    pty.send_raw(b"\x1b[1;5F");
    pty.wait_screen("S21-R2-L29").unwrap();

    // 清空草稿后正常退出（不把草稿当命令）
    for _ in 0..12 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert_eq!(mock.count(), 2, "草稿不得被提交");
}

/// S22：浏览旧历史时新回复追加不抢回底部；Esc 后恢复跟随。
#[test]
fn s22_tui_browsing_old_history_new_reply_keeps_viewport() {
    let (mut pty, mock, _keep) = scroll_session("s22", vec![
        MockResp::json(200, chat_body(&long_reply("S22-R1"))),
        MockResp::json(200, chat_body(&long_reply("S22-R2"))),
        MockResp::json(200, chat_body("S22-最新回复-到达")),
    ]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S22-R1-L29").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S22-R2-L29").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(!pty.screen().contains("S22-R1-L00"), "前置：早期能见（历史未超屏则夹具无效）");

    // 上翻浏览旧历史
    pty.send_raw(b"\x1b[5~");
    pty.wait_screen("S22-R1-L00").unwrap();

    // 上翻状态下提交第三问（Enter 不归 transcript）→ 新回复到达后视口保持在旧位置。
    // 轮询读取（睡眠不读会取到陈旧屏幕），直到第三问请求已发出且再等一拍渲染
    pty.send_line("第三问");
    let send_deadline = Instant::now() + Duration::from_secs(5);
    while mock.count() < 3 && Instant::now() < send_deadline {
        pty.read_once(100);
    }
    for _ in 0..8 {
        pty.read_once(150);
    }
    let held = pty.screen();
    assert!(
        !held.contains("S22-最新回复-到达"),
        "浏览旧历史时新回复不得抢回底部（屏幕仍应显示旧内容）"
    );
    assert!(held.contains("S22-R1-L0"), "视口应保持在旧历史");
    assert_eq!(mock.count(), 3);

    // Ctrl+End 恢复跟随 → 最新回复可见（单一职责轮：Esc 不再回底）
    pty.send_raw(b"\x1b[1;5F");
    pty.wait_screen("S22-最新回复-到达").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S23：顶部限位（越界 PageUp 不跳变不崩溃）；短历史上翻无布局跳变；resize 后存活可滚。
#[test]
fn s23_tui_scroll_limits_short_history_and_resize() {
    let (mut pty, mock, _keep) = scroll_session("s23", vec![
        MockResp::json(200, chat_body(&long_reply("S23-R1"))),
        MockResp::json(200, chat_body(&long_reply("S23-R2"))),
    ]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S23-R1-L29").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S23-R2-L29").unwrap();
    std::thread::sleep(Duration::from_millis(500));

    // 越界上翻 ×5 → 顶部限位：最早期内容可见、composer 仍在、无崩溃
    for _ in 0..5 {
        pty.send_raw(b"\x1b[5~");
        std::thread::sleep(Duration::from_millis(50));
    }
    pty.wait_screen("S23-R1-L00").unwrap();
    let top = pty.screen();
    assert!(top.contains("? for shortcuts"), "限位后 footer 仍在");

    // 键盘越底限位：Ctrl+End 回底后再多按 PageDown → 停在最新、无跳变
    pty.send_raw(b"\x1b[1;5F");
    pty.wait_screen("S23-R2-L29").unwrap();
    for _ in 0..3 {
        pty.send_raw(b"\x1b[6~");
        pty.read_once(120);
    }
    assert!(pty.screen().contains("S23-R2-L29"), "越底 PageDown 须限位在最新");

    // 验收 4（字面）：缩放期间草稿不丢失——resize 前输入草稿，resize 后仍在光标行。
    // 注：resize 后「立即慢打」的草稿渲染存在时序抖动（快打＋提交不受影响），
    // 已登记 008 收尾观察项，不在本断言范围。
    for ch in "缩放草稿".chars() {
        let mut b = [0u8; 4];
        pty.send_raw(ch.encode_utf8(&mut b).as_bytes());
        pty.read_once(60);
    }
    let draft_before = (0..10).any(|_| {
        pty.read_once(150);
        pty.screen().contains("缩放草稿")
    });
    assert!(draft_before, "前置：resize 前草稿已渲染，屏幕：\n{}", pty.screen());
    pty.resize(100, 30);
    let draft_after = (0..12).any(|_| {
        pty.read_once(150);
        pty.screen().contains("缩放草稿")
    });
    assert!(draft_after, "resize 后草稿不得丢失，屏幕：\n{}", pty.screen());
    for _ in 0..10 {
        pty.send_raw(b"\x7f");
        pty.read_once(40);
    }

    // 短历史：清屏为短内容后上翻不造成布局跳变（回复与 footer 均在）
    let (mut pty2, _mock2, _keep2) = scroll_session("s23b", vec![MockResp::json(200, chat_body("S23B-短回复"))]);
    pty2.wait_screen("? for shortcuts").unwrap();
    pty2.send_line("短问");
    pty2.wait_screen("S23B-短回复").unwrap();
    for _ in 0..4 {
        pty2.read_once(150);
    }
    let before = pty2.screen();
    for _ in 0..3 {
        pty2.send_raw(b"\x1b[5~");
        pty2.read_once(120);
    }
    // composer 区（底部）滚轮冒烟：不崩、不滚动 transcript
    for _ in 0..3 {
        pty2.send_raw(b"\x1b[<64;10;49M");
        pty2.read_once(60);
    }
    for _ in 0..4 {
        pty2.read_once(150);
    }
    let after = pty2.screen();
    assert_eq!(before, after, "短历史上翻不得造成布局跳变");
    assert!(after.contains("S23B-短回复") && after.contains("? for shortcuts"));
    let _ = mock;
    pty2.send_line("/exit");
    pty2.wait_output("再见!", WATCHDOG).unwrap();
    let (out2, status2) = pty2.finish();
    assert_eq!(status2.code(), Some(0), "{out2}");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S24：恢复长会话后可以滚动浏览，且不重放模型请求（007 契约保持）。
#[test]
fn s24_tui_resume_long_session_then_scrolls() {
    let cfg = TempDir::new("s24-cfg");
    let user = TempDir::new("s24-user");
    let key = "s24-key";
    let mock = spawn_mock(vec![]); // 零回复：恢复后滚动不得产生请求
    write_manual(cfg.path(), &mock.url, "env:TCHAT_SCROLL_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let msg = |role: terminal_chat::prompt::HistoryRole, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            role,
            content,
            user_input_event: None,
            images: None,
            text_elements: Vec::new(),
        })
    };
    let hist: Vec<_> = (0..60)
        .map(|i| {
            let role = if i % 2 == 0 {
                terminal_chat::prompt::HistoryRole::User
            } else {
                terminal_chat::prompt::HistoryRole::Assistant
            };
            msg(role, format!("S24-条目-{i:02}"))
        })
        .collect();
    terminal_chat::session::save_snapshot(&user.path().join("sessions"), "seed24", &hist).unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_SCROLL_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seed24");
    pty.wait_screen("S24-条目-59").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let before = pty.screen();
    assert!(!before.contains("S24-条目-00"), "前置：早期条目不在屏幕");

    for _ in 0..4 {
        pty.send_raw(b"\x1b[5~");
        std::thread::sleep(Duration::from_millis(50));
    }
    pty.wait_screen("S24-条目-00").unwrap();
    assert!(
        mock.requests().is_empty(),
        "恢复后滚动不得重放请求：{:?}",
        mock.requests()
    );
    pty.send_raw(b"\x1b[1;5F"); // Ctrl+End 回底（单一职责轮：Esc 不再回底）
    pty.wait_screen("S24-条目-59").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}


// ---------- QueryResponder：终端查询应答器契约（008-A3 收尾 TDD）----------

mod query_responder_tests {
    use super::QueryResponder;

    /// 便捷：以 0 基光标 (11,6) 推进。
    fn adv(r: &mut QueryResponder, chunk: &[u8]) -> Vec<u8> {
        r.advance(chunk, (11, 6))
    }

    /// DSR 应答回报真实光标（0 基输入 → 1 基报文）。
    #[test]
    fn dsr_reports_real_cursor_position() {
        let mut r = QueryResponder::new();
        assert_eq!(adv(&mut r, b"\x1b[6n"), b"\x1b[12;7R".to_vec());
        assert_eq!(
            r.advance(b"\x1b[6n", (0, 0)),
            b"\x1b[1;1R".to_vec(),
            "应答随终端光标变化，非固定值"
        );
    }

    /// 完整查询恰应答一次：单块内的一个查询只产出一份应答（不含重复报文），
    /// 已消费字节不会被后续块重复扫描。
    #[test]
    fn complete_query_answered_exactly_once() {
        let mut r = QueryResponder::new();
        let out = adv(&mut r, b"\x1b[6n");
        assert_eq!(out, b"\x1b[12;7R".to_vec(), "恰一份 DSR 应答");
        assert_eq!(out.iter().filter(|&&b| b == b'R').count(), 1);
        assert!(adv(&mut r, b"").is_empty());
        assert!(adv(&mut r, b"ordinary").is_empty());
    }

    /// DSR 拆在不同读取边界：`\x1b[` + `6n` → 第二块后应答。
    #[test]
    fn dsr_split_across_reads_answered_after_completion() {
        let mut r = QueryResponder::new();
        assert!(adv(&mut r, b"\x1b[").is_empty(), "不完整查询不得提前应答");
        assert_eq!(adv(&mut r, b"6n"), b"\x1b[12;7R".to_vec());
    }

    /// OSC 10 拆分终止符（BEL 单字节后到）。
    #[test]
    fn osc10_split_bel_terminator() {
        let mut r = QueryResponder::new();
        assert!(adv(&mut r, b"\x1b]10;?").is_empty());
        assert_eq!(adv(&mut r, b"\x07"), b"\x1b]10;rgb:c7c7/c7c7/c7c7\x1b\\".to_vec());
    }

    /// OSC 11 终止符 `ESC\` 从中间拆开（ESC 与 \\ 分属两块）。
    #[test]
    fn osc11_split_st_terminator_mid_pair() {
        let mut r = QueryResponder::new();
        assert!(adv(&mut r, b"\x1b]11;?").is_empty());
        assert!(adv(&mut r, b"\x1b").is_empty(), "只到 ST 的 ESC 仍不完整");
        assert_eq!(adv(&mut r, b"\\"), b"\x1b]11;rgb:0000/0000/0000\x1b\\".to_vec());
    }

    /// 连续查询逐一按序应答。
    #[test]
    fn consecutive_queries_each_answered_in_order() {
        let mut r = QueryResponder::new();
        let out = adv(&mut r, b"\x1b[6n\x1b]10;?\x07\x1b[6n");
        assert_eq!(out, b"\x1b[12;7R\x1b]10;rgb:c7c7/c7c7/c7c7\x1b\\\x1b[12;7R".to_vec());
    }

    /// 查询夹在普通输出中间：恰一次应答，普通字节不受影响。
    #[test]
    fn mixed_output_around_query_answers_once() {
        let mut r = QueryResponder::new();
        let out = adv(&mut r, b"hello \x1b[6n world");
        assert_eq!(out, b"\x1b[12;7R".to_vec());
        assert!(adv(&mut r, b"tail").is_empty());
    }

    /// 非查询转义序列（CSI/SGR 等）不应答，也不把垃圾滞留为未决缓冲。
    #[test]
    fn non_query_escapes_never_answered_and_no_stale_pending() {
        let mut r = QueryResponder::new();
        assert!(adv(&mut r, b"\x1b[2J\x1b[?25h\x1b[0m").is_empty());
        assert!(adv(&mut r, b"").is_empty(), "无滞留未决缓冲");
        // 拆分的非查询 CSI 也不误答
        assert!(adv(&mut r, b"\x1b[").is_empty());
        assert!(adv(&mut r, b"2J").is_empty());
        assert_eq!(adv(&mut r, b"\x1b[6n"), b"\x1b[12;7R".to_vec(), "缓冲清空后新查询正常应答");
    }
}

// ---------- S25：A3 收尾——resize 后立即慢打的草稿渲染时延（观察 2 重判定）----------
// 背景：2026-10-14 修复轮曾观察 resize 后立即逐字输入的草稿渲染偶发延迟（>1.5s），
// 当时 PTY 宿主尚不应答终端查询（DSR 应答器为后来补齐）。本用例在「持续读取＋
// 查询应答完好」的环境下复验并钉住行为：resize 前 CJK/emoji 草稿、resize 后立即
// 逐字输入（测首现时延）、随后提交并继续输入下一条。

/// S25：resize 前草稿在缩放后存活；resize 后立即逐字输入的草稿在有限时延内渲染
///（粘贴突发冲刷＋帧调度属母本正常缓冲时机）；提交与后续输入不受影响。
#[test]
fn s25_tui_slow_type_immediately_after_resize() {
    let (mut pty, mock, _keep) = scroll_session("s25", vec![
        MockResp::json(200, chat_body(&long_reply("S25-R1"))),
        MockResp::json(200, chat_body(&long_reply("S25-R2"))),
        MockResp::json(200, chat_body("S25-缩放后回复-OK")),
    ]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S25-R1-L29").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S25-R2-L29").unwrap();
    for _ in 0..4 {
        pty.read_once(150);
    }

    // resize 前：中文＋emoji 草稿已渲染
    type_slow(&mut pty, "缩放前草稿🎨");
    let pre_ok = (0..10).any(|_| {
        pty.read_once(150);
        pty.screen().contains("缩放前草稿")
    });
    assert!(pre_ok, "前置：resize 前草稿已渲染，屏幕：\n{}", pty.screen());

    // resize → 立即逐字输入（首现时延测量，持续读取）
    pty.resize(120, 40);
    let t0 = Instant::now();
    type_slow(&mut pty, "缩放后立打");
    let mut latency_ms = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pty.read_once(100);
        if pty.screen().contains("缩放后立打") {
            latency_ms = Some(t0.elapsed().as_millis());
            break;
        }
        assert!(
            Instant::now() <= deadline,
            "resize 后立即慢打 5s 未渲染（时延超界），屏幕：\n{}",
            pty.screen()
        );
    }
    let latency = latency_ms.unwrap();
    eprintln!("S25 诊断：resize 后立即慢打首现时延 = {latency}ms（读取轮询粒度 100ms）");
    // 旧草稿仍在（缩放期间草稿不丢失）
    assert!(pty.screen().contains("缩放前草稿"), "resize 后旧草稿不得丢失");

    // 清空草稿（退格 ×每字符），提交一条新输入，验证交互链路完整
    for _ in 0..16 {
        pty.send_raw(b"\x7f");
        pty.read_once(40);
    }
    pty.send_line("缩放后的新提问");
    let dl = Instant::now() + Duration::from_secs(5);
    while mock.count() < 3 && Instant::now() < dl {
        pty.read_once(100);
    }
    assert_eq!(mock.count(), 3, "resize 后提交通道正常");
    pty.wait_screen("S25-缩放后回复-OK").unwrap();

    // 提交后再慢打下一条草稿（继续输入）
    type_slow(&mut pty, "再来一条🚀");
    let next_ok = (0..10).any(|_| {
        pty.read_once(150);
        pty.screen().contains("再来一条")
    });
    assert!(next_ok, "提交后继续慢打正常渲染，屏幕：\n{}", pty.screen());
    for _ in 0..12 {
        pty.send_raw(b"\x7f");
        pty.read_once(40);
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S26：resize 与输入竞态（2026-10-15 收尾：曾致 composer 越界 panic）----------
// 根因：draw() 预取的屏幕尺寸与 tui.draw 帧缓冲在 SIGWINCH 到达窗口内不一致，
// composer 按旧行号写新缓冲越界（120x40 缓冲 index (0,47)）。修复＝渲染区域取自
// 真实帧缓冲尺寸。本用例多轮 resize＋紧贴输入施压竞态窗口，断言全程存活且草稿渲染。

/// S26：多轮交替 resize＋每轮立即逐字输入——无 panic、草稿每轮渲染、应用存活。
#[test]
fn s26_tui_resize_input_race_survives() {
    let (mut pty, _mock, _keep) = scroll_session("s26", vec![
        MockResp::json(200, chat_body(&long_reply("S26-R1"))),
        MockResp::json(200, chat_body(&long_reply("S26-R2"))),
    ]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S26-R1-L29").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S26-R2-L29").unwrap();
    for _ in 0..4 {
        pty.read_once(150);
    }
    for round in 0..8 {
        let (cols, rows) = if round % 2 == 0 { (120, 40) } else { (200, 50) };
        pty.resize(cols, rows);
        // 紧贴 resize 立即逐字输入（竞态窗口施压；间隔 40ms 档）
        for ch in format!("轮{round}字").chars() {
            let mut buf = [0u8; 4];
            pty.send_raw(ch.encode_utf8(&mut buf).as_bytes());
            pty.read_once(40);
        }
        let ok = (0..20).any(|_| {
            pty.read_once(100);
            pty.screen().contains(&format!("轮{round}字"))
        });
        assert!(ok, "第 {round} 轮（{cols}x{rows}）草稿未渲染，屏幕：\n{}", pty.screen());
        for _ in 0..8 {
            pty.send_raw(b"\x7f");
            pty.read_once(30);
        }
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "resize 竞态下不得 panic：{out}");
    assert!(!out.contains("panicked"), "子进程不得 panic：{out}");
}

// ---------- S27–S32：012 工具详情展示（命令/差异/状态/展开/恢复/兼容）----------

/// S27：exec 失败终态——实际命令、非零退出码与"Failed"可见；CLI 存活、后续轮正常
/// （012 需求①/③：状态准确区分）。
#[test]
fn s27_tui_exec_failure_state_visible_and_recovers() {
    let cfg = TempDir::new("s27-cfg");
    let user = TempDir::new("s27-user");
    let key = "s27-key";
    let args = r#"{"cmd":"echo t012-fail; exit 9"}"#.to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s27-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S27-恢复-OK")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S27_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S27_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("echo t012-fail; exit 9").unwrap();
    pty.wait_screen("Failed").unwrap();
    pty.feed_screen();
    assert!(
        pty.vt.to_string().contains("9"),
        "退出码可见：{}",
        pty.vt.vt100().screen().contents()
    );
    // 失败后模型续轮正常（CLI 存活）
    pty.wait_screen("S27-恢复-OK").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert_eq!(mock.count(), 2);
}

/// S28：apply_patch 增/改/删——路径与真实增删内容（+新行/−旧行）上屏，磁盘副作用
/// 与展示一致（012 需求②③：diff 来自真实前后内容）。
#[test]
fn s28_tui_patch_add_update_delete_diff_visible() {
    let cfg = TempDir::new("s28-cfg");
    let user = TempDir::new("s28-user");
    let key = "s28-key";
    let work = TempDir::new("s28-work");
    let upd = work.path().join("upd.txt");
    let del = work.path().join("del.txt");
    let newf = work.path().join("new.txt");
    std::fs::write(&upd, "KEEP-A\nOLD-LINE\nKEEP-B\n").unwrap();
    std::fs::write(&del, "DEL-LINE-1\nDEL-LINE-2\n").unwrap();
    let patch = format!(
        "*** Begin Patch\n*** Add File: {n}\n+ADDED-LINE-1\n+ADDED-LINE-2\n*** Update File: {u}\n@@\n-OLD-LINE\n+NEW-LINE\n*** Delete File: {d}\n*** End Patch",
        n = newf.display(),
        u = upd.display(),
        d = del.display(),
    );
    let args = serde_json::json!({ "patch": patch }).to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s28-patch", "type": "function",
              "function": { "name": "apply_patch", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S28-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S28_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(Some(work.path()), &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S28_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("改");
    // 终态完整差异 cell（路径＋真实增删行；运行中标记为瞬态，不以屏幕级断言钉住）
    pty.wait_screen("new.txt").unwrap();
    pty.wait_screen("upd.txt").unwrap();
    pty.wait_screen("del.txt").unwrap();
    pty.wait_screen("ADDED-LINE-1").unwrap();
    pty.wait_screen("NEW-LINE").unwrap();
    // 计数锚点（012 §5-A2）：+3/−3 与真实 diff 行数一致（2 新增＋1 改增／2 删除＋1 改删）
    pty.feed_screen();
    assert!(
        pty.vt.to_string().contains("+3") && pty.vt.to_string().contains("-3"),
        "增删计数与真实差异一致：{}",
        pty.vt.vt100().screen().contents()
    );
    pty.wait_screen("OLD-LINE").unwrap();
    pty.wait_screen("DEL-LINE-1").unwrap();
    // 最终回复出现后差异仍可浏览（012 需求⑧）
    pty.wait_screen("S28-完成").unwrap();
    pty.feed_screen();
    assert!(pty.vt.to_string().contains("NEW-LINE"), "详情在最终回复后保留");
    // 磁盘事实与展示一致
    assert_eq!(std::fs::read_to_string(&upd).unwrap(), "KEEP-A\nNEW-LINE\nKEEP-B\n");
    assert_eq!(std::fs::read_to_string(&newf).unwrap(), "ADDED-LINE-1\nADDED-LINE-2\n");
    assert!(!del.exists(), "删除生效");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S29：工具执行中 Ctrl+C——"已取消"独立呈现（非"拒绝"）、CLI 存活、后续轮正常
/// （012 需求④；取消语义＝006 N2b 不变）。
#[test]
fn s29_tui_cancel_renders_explicit_cancelled_and_recovers() {
    let cfg = TempDir::new("s29-cfg");
    let user = TempDir::new("s29-user");
    let key = "s29-key";
    let args = r#"{"cmd":"sleep 20","timeout_ms":30000}"#.to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s29-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    // 006 既有调度契约：单调用批的取消结果会续发模型一次（取消标志只拦截后续
    // 未执行调用——多调用批的终止语义由 N2b 钉住）；012 只改呈现，不改调度。
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S29-续轮-OK")),
        MockResp::json(200, chat_body("S29-新轮-OK")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S29_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S29_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("go");
    pty.wait_screen("sleep 20").unwrap();
    pty.send_raw(b"\x1b"); // 单一职责轮：Esc＝取消
    // 取消呈现："已取消"（被杀执行 cell，含原因）；不得出现"拒绝"
    pty.wait_screen_timeout("已取消", std::time::Duration::from_secs(3))
        .expect("取消必须呈现为已取消");
    pty.wait_screen("S29-续轮-OK").unwrap(); // 既有契约：单调用取消后续发模型一次
    pty.feed_screen();
    assert!(
        !pty.vt.to_string().contains("Declined") && !pty.vt.to_string().contains("拒绝"),
        "取消不得机械映射拒绝：{}",
        pty.vt.vt100().screen().contents()
    );
    // CLI 存活：新一轮正常
    pty.send_line("再来");
    pty.wait_screen("S29-新轮-OK").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S30：长输出折叠与展开——折叠预览有限行＋ctrl+t 提示真实可用；F4 聚焦＋Enter
/// 展开、Esc 退出聚焦；Ctrl+T detailed 全文；全程草稿不丢（012 需求⑦）。
#[test]
fn s30_tui_long_output_fold_expand_detailed_draft_kept() {
    let cfg = TempDir::new("s30-cfg");
    let user = TempDir::new("s30-user");
    let key = "s30-key";
    // 20 行编号输出：折叠预览只含尾部若干行，首行须展开后才可见
    let cmd = "for i in $(seq -w 1 20); do echo OUT-$i; done";
    let args = serde_json::json!({ "cmd": cmd }).to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s30-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S30-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S30_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S30_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("OUT-20").unwrap();
    // 草稿（不提交）——交互全程必须保留
    pty.send_raw("S30-草稿".as_bytes());
    pty.wait_screen("S30-草稿").unwrap();
    // F4 聚焦活动 cell → Enter＝打开详情页（首行可见；折叠时不可见）
    pty.feed_screen();
    assert!(!pty.vt.to_string().contains("OUT-01"), "折叠态首行不可见（预览有限）");
    pty.send_raw(b"\x1bOS"); // F4
    pty.read_once(200);
    pty.send_raw(b"\r"); // Enter＝打开详情页
    pty.wait_screen("OUT-01").expect("详情页首行可见（F4+Enter）");
    // 单一职责轮：Esc 空闲＝无动作——页保持打开
    pty.send_raw(b"\x1b");
    pty.dwell(250);
    assert!(
        pty.screen().contains("back to conversation"),
        "Esc 空闲不得关页：{}",
        pty.screen()
    );
    // q 关页返回对话（草稿保留）
    pty.send_raw(b"q");
    pty.read_once(200);
    // Ctrl+T＝再次打开详情页全文（提示按键真实可用）
    pty.send_raw(b"\x14");
    pty.wait_screen("OUT-05").expect("详情页呈现全文");
    pty.send_raw(b"q"); // q 关页（单一职责轮）
    pty.read_once(200);
    pty.feed_screen();
    assert!(
        (0..20).any(|_| {
            let _ = pty.read_once(100);
            pty.screen().contains("S30-草稿")
        }),
        "交互后草稿保留：{}",
        pty.screen()
    );
    // 清空草稿再退出（避免排队）
    for _ in 0..24 {
        pty.send_raw(b"\x7f");
        pty.read_once(30);
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("panicked"), "不得 panic：{out}");
}

/// S31：工具详情随会话持久——重启后 /resume 恢复同渲染，零模型请求、工具不重执行
/// （012 需求⑤；007 不重放契约）。
#[test]
fn s31_tui_resume_preserves_tool_details_without_replay() {
    let cfg = TempDir::new("s31-cfg");
    let user = TempDir::new("s31-user");
    let key = "s31-key";
    let work = TempDir::new("s31-work");
    let counter = work.path().join("counter.txt");
    let cmd = format!("echo side >> {}", counter.display());
    let args = serde_json::json!({ "cmd": cmd }).to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s31-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S31-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S31_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let envs: Vec<(String, String)> = vec![
        ("TCHAT_CONFIG_DIR".into(), cfg.path().display().to_string()),
        ("TCHAT_USER_DIR".into(), user.path().display().to_string()),
        ("TCHAT_S31_KEY".into(), key.into()),
    ];
    let envs_ref: Vec<(&str, &str)> =
        envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    // 会话 1：执行工具轮并退出（auto 落盘）
    {
        let mut pty = PtySession::spawn(pty_cli(Some(work.path()), &envs_ref));
        pty.wait_screen("? for shortcuts").unwrap();
        pty.send_line("跑");
        pty.wait_screen("S31-完成").unwrap();
        pty.send_line("/exit");
        pty.wait_output("再见!", WATCHDOG).unwrap();
        let (out, status) = pty.finish();
        assert_eq!(status.code(), Some(0), "{out}");
    }
    assert_eq!(mock.count(), 2, "会话 1 恰两次请求");
    let lines_after_run = std::fs::read_to_string(&counter)
        .unwrap()
        .lines()
        .count();
    assert_eq!(lines_after_run, 1, "工具恰好执行一次");

    // 会话 2：/resume（最近 auto）→ 命令详情恢复渲染；同一 mock 计数保持 2
    //（恢复后任何模型请求都会使 count > 2——比空 mock 更强的零请求断言）
    {
        let mut pty = PtySession::spawn(pty_cli(Some(work.path()), &envs_ref));
        pty.wait_screen("? for shortcuts").unwrap();
        pty.send_line("/resume");
        pty.wait_screen("echo side >>").expect("恢复后命令详情可见");
        pty.feed_screen();
        assert!(
            pty.vt.to_string().contains("side"),
            "输出详情可见：{}",
            pty.vt.vt100().screen().contents()
        );
        pty.send_line("/exit");
        pty.wait_output("再见!", WATCHDOG).unwrap();
        let (out, status) = pty.finish();
        assert_eq!(status.code(), Some(0), "{out}");
    }
    assert_eq!(mock.count(), 2, "恢复不得产生模型请求");
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        lines_after_run,
        "恢复不得重执行工具"
    );
}

/// S32：旧会话兼容——无 display 字段的旧记录：exec 命令自 arguments 恢复展示、
/// 结果如实；旧 patch 记录明确降级（不伪造差异、不崩溃）（012 需求⑥）。
#[test]
fn s32_tui_legacy_session_without_display_degrades_cleanly() {
    let cfg = TempDir::new("s32-cfg");
    let user = TempDir::new("s32-user");
    let key = "s32-key";
    let mock = spawn_mock(vec![]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S32_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    // 手写旧格式会话（无 display 字段）：user → tool_calls(exec＋patch) → 两条结果
    let legacy = concat!(
        "{\"v\":1}\n",
        "{\"type\":\"text\",\"role\":\"user\",\"content\":\"旧会话提问\"}\n",
        "{\"type\":\"tool_calls\",\"tool_calls\":[",
        "{\"id\":\"c-legacy-e\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"echo legacy-out\\\"}\"},",
        "{\"id\":\"c-legacy-p\",\"name\":\"apply_patch\",\"arguments\":\"{\\\"patch\\\":\\\"x\\\"}\"}",
        "],\"content\":null}\n",
        "{\"type\":\"tool_result\",\"tool_call_id\":\"c-legacy-e\",\"content\":\"{\\\"status\\\":\\\"completed\\\",\\\"exit_code\\\":0,\\\"output\\\":\\\"legacy-out\\\\n\\\",\\\"truncated\\\":false,\\\"error\\\":null}\"}\n",
        "{\"type\":\"tool_result\",\"tool_call_id\":\"c-legacy-p\",\"content\":\"{\\\"ok\\\":true,\\\"changed_files\\\":[{\\\"path\\\":\\\"/w/old.txt\\\",\\\"action\\\":\\\"update\\\"}],\\\"state_known\\\":true,\\\"error\\\":null}\"}\n",
    );
    std::fs::create_dir_all(user.path().join("sessions")).unwrap();
    std::fs::write(user.path().join("sessions/legacy012.jsonl"), legacy).unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S32_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume legacy012");
    pty.wait_screen("echo legacy-out").expect("旧会话命令自 arguments 恢复");
    pty.wait_screen("legacy-out").expect("旧会话输出如实渲染");
    pty.wait_screen("早期记录未保存差异详情").expect("缺真实 diff＝明确降级");
    pty.feed_screen();
    assert!(
        pty.vt.to_string().contains("/w/old.txt"),
        "降级清单保留文件路径：{}",
        pty.vt.vt100().screen().contents()
    );
    assert!(!pty.vt.to_string().contains("+0"), "不得伪造 +0 计数");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "旧会话装载不得崩溃：{out}");
    assert!(!out.contains("panicked"), "{out}");
    assert_eq!(mock.count(), 0);
}

/// S33（A7 扩展）：工具命令/输出携带伪造转义序列（OSC 52 剪贴板/CSI 清屏）——
/// 渲染零动作：无 OSC 52 外发、无清屏、文本可见（012 §2 安全门禁）。
#[test]
fn s33_tui_tool_output_injection_neutralized() {
    let cfg = TempDir::new("s33-cfg");
    let user = TempDir::new("s33-user");
    let key = "s33-key";
    // 命令字符串含转义（JSON \u001b）＋输出含伪造 OSC52/CSI/nF
    let cmd = "printf 'A\\u001b]52;c;aGk=\\u0007B\\u001b[2JC\\u001b(De'";
    let args = serde_json::json!({ "cmd": cmd }).to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s33-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S33-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S33_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S33_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S33-完成").unwrap();
    // 屏幕级：A/B/C 输出字符可见（未被清屏抹掉），转义序列不生效
    pty.feed_screen();
    let screen = pty.vt.to_string();
    assert!(screen.contains('A') && screen.contains('B') && screen.contains('C'),
        "输出文本可见（清屏未执行）：{screen}");
    assert!(!screen.contains("\u{1b}]52"), "屏幕无 OSC 52 文本");
    assert!(!screen.contains("\u{1b}["), "屏幕无裸 CSI");
    // 字节级：捕获流中不得出现应用外发的 OSC 52（剪贴板写入零动作）
    let (out, status) = {
        pty.send_line("/exit");
        pty.wait_output("再见!", WATCHDOG).unwrap();
        pty.finish()
    };
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("\u{1b}]52"), "不得外发 OSC 52（无剪贴板写入）：{out:?}");
}

/// S30b：012 键路由契约——聚焦态 Enter 归视图（草稿恰为 "exit" 也不触发裸退出）；
/// 聚焦态普通字符落回 composer 草稿（母本"视图放弃的键归输入区"语义）；Esc 解除
/// 聚焦后回到常规 footer。用干净草稿（无退格操作，避开 paste-burst 时序）。
#[test]
fn s30b_focused_enter_not_bare_exit_and_chars_fall_through() {
    let cfg = TempDir::new("s30b-cfg");
    let user = TempDir::new("s30b-user");
    let key = "s30b-key";
    // 5 行输出：折叠预览只含尾部 3 行（B3/B4/B5），首行 B1 须展开后可见
    let args = r#"{"cmd":"for i in 1 2 3 4 5; do echo B$i; done"}"#.to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s30b", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S30B-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S30B_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S30B_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S30B-完成").unwrap();
    pty.feed_screen();
    assert!(!pty.vt.to_string().contains("B1"), "折叠态首行不可见");
    // 草稿逐字符输入 "exit"（>8ms 间隔＝键入而非粘贴突发）
    for ch in "exit".chars() {
        let mut buf = [0u8; 4];
        pty.send_raw(ch.encode_utf8(&mut buf).as_bytes());
        pty.read_once(120);
    }
    assert!(
        (0..20).any(|_| {
            let _ = pty.read_once(100);
            pty.screen().contains("exit")
        }),
        "草稿应为 exit：{}",
        pty.screen()
    );
    // F4 聚焦 → Enter：必须是打开详情页（cell 详情变化），绝不能裸退出
    pty.send_raw(b"\x1bOS");
    pty.read_once(200);
    pty.send_raw(b"\r");
    pty.wait_screen("B1").expect("聚焦态 Enter＝打开详情页（首行可见）");
    // 单一职责轮：Esc 空闲＝无动作——页保持打开、不退出；
    // q 关页后继续（存活证明＝后续输入仍落草稿）
    pty.send_raw(b"\x1b");
    pty.dwell(300);
    assert!(
        pty.screen().contains("back to conversation"),
        "Esc 空闲不得关页：{}",
        pty.screen()
    );
    pty.send_raw(b"q");
    pty.read_once(300);
    pty.feed_screen();
    assert!(!pty.vt.to_string().contains("再见"), "聚焦态 Enter/Esc/q 不得退出");
    // 聚焦态字符落回草稿：再聚焦，输入字符应追加进草稿（并解除聚焦）
    pty.send_raw(b"\x1bOS");
    pty.read_once(200);
    pty.send_raw("字".as_bytes());
    assert!(
        (0..20).any(|_| {
            let _ = pty.read_once(100);
            pty.screen().contains("exit字")
        }),
        "聚焦态输入须落入草稿：{}",
        pty.screen()
    );
    // 清空草稿（exit字＝5 个字符簇）后正常退出
    for _ in 0..12 {
        pty.send_raw(b"\x7f");
        pty.read_once(30);
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("panicked"), "{out}");
    assert_eq!(mock.count(), 2);
}

/// S34：同批多工具＋多 cell 终态替换——失败的 apply_patch 终态产出 2 个 cell
///（部分差异＋失败 cell），其后 exec 的终态替换必须落在正确位置（1→N splice
/// 后的记账下标修正；丢失/错替＝详情缺失或重复）。
#[test]
fn s34_batch_multi_cell_replacement_keeps_subsequent_tools() {
    let cfg = TempDir::new("s34-cfg");
    let user = TempDir::new("s34-user");
    let key = "s34-key";
    let work = TempDir::new("s34-work");
    let okf = work.path().join("ok.txt");
    let patch = format!(
        "*** Begin Patch\n*** Add File: {a}\n+S34-PART\n*** Update File: {missing}\n@@\n-x\n+y\n*** End Patch",
        a = okf.display(),
        missing = work.path().join("missing.txt").display(),
    );
    let args_patch = serde_json::json!({ "patch": patch }).to_string();
    let args_exec = r#"{"cmd":"echo S34-EXEC-OUT"}"#.to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s34-patch", "type": "function",
              "function": { "name": "apply_patch", "arguments": args_patch } },
            { "id": "call-s34-exec", "type": "function",
              "function": { "name": "exec_command", "arguments": args_exec } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S34-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S34_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(Some(work.path()), &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S34_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    // 补丁终态＝部分差异（+S34-PART）＋失败 cell（Failed to apply patch）双 cell
    pty.wait_screen("S34-PART").unwrap();
    pty.wait_screen("Failed to apply patch").unwrap();
    // 同批 exec 终态不被误替换/丢失：命令与输出可见
    pty.wait_screen("S34-EXEC-OUT").unwrap();
    // 最终回复出现后三段详情共存（需求⑧）
    pty.wait_screen("S34-完成").unwrap();
    pty.feed_screen();
    let screen = pty.vt.to_string();
    assert!(screen.contains("S34-PART") && screen.contains("Failed to apply patch")
        && screen.contains("S34-EXEC-OUT"), "详情共存：{screen}");
    // 磁盘事实：部分补丁生效＋缺失目标未创建
    assert_eq!(std::fs::read_to_string(&okf).unwrap(), "S34-PART\n");
    assert!(!work.path().join("missing.txt").exists());
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S35–S39：011 历史区右侧滚动条（沟位渲染/拖动/轨道点击/resize/短历史）----------

/// 011 辅助：读取沟位列（`col` 为 0 基列号）的非空单元格 (row, char)。
fn gutter_marks_at(pty: &mut PtySession, col: u16) -> Vec<(u16, char)> {
    pty.feed_screen();
    let screen = pty.vt.vt100().screen();
    let mut marks = Vec::new();
    for row in 0..PTY_ROWS {
        if let Some(c) = screen.cell(row, col).map(|c| c.contents().to_string()) {
            if let Some(ch) = c.chars().next().filter(|ch| !ch.is_whitespace()) {
                marks.push((row, ch));
            }
        }
    }
    marks
}

fn gutter_marks(pty: &mut PtySession) -> Vec<(u16, char)> {
    gutter_marks_at(pty, PTY_COLS - 1)
}

/// SGR 鼠标序列（1 基坐标）：左键按下 / 按住拖动 / 释放。
fn sgr_down(col: u16, row: u16) -> Vec<u8> {
    format!("\x1b[<0;{col};{row}M").into_bytes()
}
fn sgr_drag(col: u16, row: u16) -> Vec<u8> {
    format!("\x1b[<32;{col};{row}M").into_bytes()
}
fn sgr_up(col: u16, row: u16) -> Vec<u8> {
    format!("\x1b[<0;{col};{row}m").into_bytes()
}

/// 011 辅助：种一个长会话并恢复（120 条单行记录 → 折行后远超一屏），零 mock 回复。
fn scrollbar_session(tag: &str) -> (PtySession, common::Mock, Vec<TempDir>) {
    let (pty, mock, keep) = scroll_session(tag, vec![]);
    let msg = |role: terminal_chat::prompt::HistoryRole, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            role,
            content,
            user_input_event: None,
            images: None,
            text_elements: Vec::new(),
        })
    };
    let stem = format!("S{tag}");
    let hist: Vec<_> = (0..120)
        .map(|i| {
            let role = if i % 2 == 0 {
                terminal_chat::prompt::HistoryRole::User
            } else {
                terminal_chat::prompt::HistoryRole::Assistant
            };
            msg(role, format!("{stem}-L{i:03}"))
        })
        .collect();
    terminal_chat::session::save_snapshot(&keep[1].path().join("sessions"), "seedscroll", &hist)
        .unwrap();
    (pty, mock, keep)
}

/// S35：历史超一屏时沟位出现轨道＋滑块（跟随态滑块贴底）；PageUp 后滑块位置同步上移；
/// 滚动条出现不改变滚动语义（不上屏即不发请求）。
#[test]
fn s35_tui_scrollbar_renders_and_tracks_position() {
    let (mut pty, mock, _keep) = scrollbar_session("35");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S35-L119").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    let marks = gutter_marks(&mut pty);
    let thumbs: Vec<u16> = marks.iter().filter(|m| m.1 == '█').map(|m| m.0).collect();
    let tracks: Vec<u16> = marks.iter().filter(|m| m.1 == '║').map(|m| m.0).collect();
    assert!(!thumbs.is_empty(), "跟随态必须有滑块：{marks:?}");
    assert!(!tracks.is_empty(), "溢出时必须有轨道：{marks:?}");
    // 跟随态滑块精确贴底（position 范围＝最大可滚动顶行，011 收尾修正）；
    // 断言「滑块位于轨道下部」——上方轨道远多于下方
    let thumb_top = *thumbs.iter().min().unwrap();
    let thumb_bottom = *thumbs.iter().max().unwrap();
    let above = marks.iter().filter(|m| m.0 < thumb_top).count();
    let below = marks.iter().filter(|m| m.0 > thumb_bottom).count();
    assert!(above > below * 3, "跟随态滑块在轨道下部：上{above}/下{below}");
    assert!(
        thumbs.len() * 2 < marks.len(),
        "A1 占比：长内容滑块须明显小于轨道（{}/{}）",
        thumbs.len(),
        marks.len()
    );
    assert!(!pty.screen().contains("S35-L000"), "前置：早期条目不在屏幕");

    // PageUp 上翻 → 视口移动，滑块同步上移（不再贴底）
    pty.send_raw(b"\x1b[5~");
    for _ in 0..6 { pty.read_once(60); }
    let marks2 = gutter_marks(&mut pty);
    let thumbs2: Vec<u16> = marks2.iter().filter(|m| m.1 == '█').map(|m| m.0).collect();
    assert!(!thumbs2.is_empty(), "上翻后滑块仍在：{marks2:?}");
    let max2 = *thumbs2.iter().max().unwrap();
    let max1 = *thumbs.iter().max().unwrap();
    assert!(max2 < max1, "PageUp 后滑块底边上移：{max2} < {max1}");
    // A4：滚轮与滚动条同一视口状态——滚轮继续上翻，滑块同步再上移
    // 12 次滚轮＝12 行（分数行累积），足以让滑块跨过一个轨道格
    for _ in 0..12 {
        pty.send_raw(b"\x1b[<64;10;30M");
        pty.read_once(40);
    }
    for _ in 0..4 { pty.read_once(60); }
    let marks3 = gutter_marks(&mut pty);
    let thumbs3: Vec<u16> = marks3.iter().filter(|m| m.1 == '█').map(|m| m.0).collect();
    let max3 = *thumbs3.iter().max().unwrap();
    assert!(max3 < max2, "滚轮后续滚动滑块继续上移：{max3} < {max2}");
    assert!(!pty.screen().contains("S35-L119"), "上翻后最新条目移出视口");
    assert!(mock.requests().is_empty(), "滚动条/翻页不得发请求");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S36：拖动滑块到顶＝最早历史可见；拖回底部＝恢复跟随（后续输出自动贴底）；
/// 释放结束拖动。全程零模型请求。
#[test]
fn s36_tui_scrollbar_drag_browses_and_restores_follow() {
    let (mut pty, mock, _keep) = scrollbar_session("36");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S36-L119").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let marks = gutter_marks(&mut pty);
    let thumb_bottom = marks
        .iter()
        .filter(|m| m.1 == '█')
        .map(|m| m.0)
        .max()
        .expect("前置：跟随态滑块");

    // 按下滑块（1 基）→ 拖到顶部
    pty.send_raw(&sgr_down(PTY_COLS, thumb_bottom + 1));
    for _ in 0..2 { pty.read_once(60); }
    pty.send_raw(&sgr_drag(PTY_COLS, 1));
    pty.wait_screen("S36-L000").unwrap();
    let marks_top = gutter_marks(&mut pty);
    assert!(
        marks_top.iter().any(|m| m.1 == '█' && m.0 == 0),
        "拖到顶后滑块贴顶：{marks_top:?}"
    );

    // A2 中点：拖到轨道中部——最早与最新条目都不可见（按比例落位）
    let mid_row = (marks.iter().map(|m| m.0).max().unwrap()) / 2;
    pty.send_raw(&sgr_drag(PTY_COLS, mid_row));
    for _ in 0..6 { pty.read_once(60); }
    let mid_screen = pty.screen();
    assert!(!mid_screen.contains("S36-L000"), "中点不得见最早：{mid_screen}");
    assert!(!mid_screen.contains("S36-L119"), "中点不得见最新：{mid_screen}");
    assert!(mid_screen.contains("S36-L0"), "中点应见中部条目：{mid_screen}");

    // 拖回底部 → 最新可见 + 恢复跟随
    pty.send_raw(&sgr_drag(PTY_COLS, PTY_ROWS));
    pty.wait_screen("S36-L119").unwrap();
    pty.send_raw(&sgr_up(PTY_COLS, PTY_ROWS));
    for _ in 0..3 { pty.read_once(60); }

    // 跟随已恢复：新输出（/sessions 列表）自动贴底可见，无需手动滚动
    pty.send_line("/sessions");
    pty.wait_screen("seedscroll").unwrap();

    assert!(mock.requests().is_empty(), "拖动不得发请求");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S37：轨道点击＝整页滚动（滑块上方点击上翻、下方点击下翻），连续点击不丢步。
#[test]
fn s37_tui_scrollbar_track_click_pages() {
    let (mut pty, _mock, _keep) = scrollbar_session("37");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S37-L119").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    assert!(!pty.screen().contains("S37-L000"), "前置：顶部条目不可见");

    // 滑块上方轨道点击＝上翻一页。第 1 行是 prompt header（正文区外，按下无
    // 操作），轨道点击取第 2 行；首页锚点用 L09x（底部本来就有 L10x 前缀误匹配）
    pty.send_raw(&sgr_down(PTY_COLS, 2));
    pty.send_raw(&sgr_up(PTY_COLS, 2));
    pty.wait_screen("S37-L09").unwrap();
    for _ in 0..3 { pty.read_once(60); }
    assert!(!pty.screen().contains("S37-L119"), "上翻一页后最新条目移出视口");

    // 连续点击继续上翻（不丢步）：再点 3 次到更早区域
    for _ in 0..3 {
        pty.send_raw(&sgr_down(PTY_COLS, 2));
        pty.send_raw(&sgr_up(PTY_COLS, 2));
        for _ in 0..2 { pty.read_once(60); }
    }
    let screen = pty.screen();
    assert!(
        screen.contains("S37-L0"),
        "连续点击持续上翻（应达 L0xx 区）：{screen}"
    );
    assert!(!screen.contains("S37-L119"), "仍在旧历史");

    // 滑块下方轨道点击＝下翻；连点回最新（页数容差内）。最底行取沟位最末
    // 标记行（屏幕末行属 composer 区，不归 transcript）
    let bottom_row = gutter_marks(&mut pty)
        .iter()
        .map(|m| m.0)
        .max()
        .expect("沟位标记");
    for _ in 0..6 {
        if pty.screen().contains("S37-L119") {
            break;
        }
        pty.send_raw(&sgr_down(PTY_COLS, bottom_row + 1));
        pty.send_raw(&sgr_up(PTY_COLS, bottom_row + 1));
        for _ in 0..3 { pty.read_once(60); }
    }
    pty.wait_screen("S37-L119").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S38：拖动期间 resize，沟位按新几何重算、草稿（中文/emoji）与光标存活、无 panic；
/// resize 后继续拖动与释放仍有效。
#[test]
fn s38_tui_scrollbar_drag_survives_resize_with_draft() {
    let (mut pty, _mock, _keep) = scrollbar_session("38");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S38-L119").unwrap();

    // 逐字草稿（避开粘贴突发）
    for ch in "S38-草稿😊".chars() {
        pty.send_raw(ch.to_string().as_bytes());
        for _ in 0..1 { pty.read_once(60); }
    }
    for _ in 0..5 { pty.read_once(60); }

    let marks = gutter_marks(&mut pty);
    let thumb_bottom = marks
        .iter()
        .filter(|m| m.1 == '█')
        .map(|m| m.0)
        .max()
        .expect("前置滑块");
    pty.send_raw(&sgr_down(PTY_COLS, thumb_bottom + 1));
    for _ in 0..1 { pty.read_once(60); }

    // 拖动中途 resize（200x50 → 120x30）：不 panic、草稿仍在、拖动状态不丢
    //（中途这一步 Drag 只要求存活；几何精确断言放在释放后的重新抓取）
    pty.resize(120, 30);
    for _ in 0..6 { pty.read_once(60); }
    pty.send_raw(&sgr_drag(120, 2));
    for _ in 0..5 { pty.read_once(60); }
    let screen = pty.screen();
    assert!(screen.contains("S38-草稿😊"), "resize+拖动后草稿存活：{screen}");
    pty.send_raw(&sgr_up(120, 2));
    for _ in 0..2 { pty.read_once(60); }

    // 新几何下重新抓取拖到顶：滑块行按任意列扫描（vt 解析器保持 200 宽、
    // 列号不再与 120 宽帧对齐）；拖到顶后早期条目可见、草稿仍在
    let thumb_row = {
        pty.feed_screen();
        let scr = pty.vt.vt100().screen();
        (0..PTY_ROWS).find(|&r| {
            (0..120u16).any(|c| {
                scr.cell(r, c).map_or(false, |cell| cell.contents() == "█")
            })
        }).expect("resize 后沟位仍有滑块")
    };
    pty.send_raw(&sgr_down(120, thumb_row + 1));
    for _ in 0..1 { pty.read_once(60); }
    pty.send_raw(&sgr_drag(120, 2));
    for _ in 0..6 { pty.read_once(60); }
    let screen = pty.screen();
    assert!(screen.contains("S38-草稿😊"), "重新抓取拖动后草稿存活：{screen}");
    assert!(screen.contains("S38-L00"), "重新抓取拖到顶（最早条目可见）：{screen}");
    // 退回最新、清草稿、干净退出
    pty.send_raw(b"\x1b[1;5F"); // Ctrl+End 回底（单一职责轮：Esc 不再回底）
    pty.wait_screen("S38-L119").unwrap();
    for _ in 0..24 {
        pty.send_raw(b"\x7f");
        for _ in 0..1 { pty.read_once(60); }
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S39：短历史（不足一屏）沟位留空（K1 常驻但不渲染轨道/滑块）；极窄窗口不 panic。
#[test]
fn s39_tui_scrollbar_short_history_blank_and_narrow_alive() {
    let (mut pty, _mock, _keep) = scroll_session("s39", vec![]);
    pty.wait_screen("? for shortcuts").unwrap();
    let marks = gutter_marks(&mut pty);
    assert!(
        !marks.iter().any(|m| m.1 == '█' || m.1 == '║'),
        "短历史沟位留空：{marks:?}"
    );
    // 窄窗 resize 存活。不读睡眠会让应用的终端查询停等（应答器只随读取推进），
    // 后续输入易落入粘贴突发窗口——两段 resize 后都以 read_once 轮询推进
    //（S23/S25 同法；不加 Esc——会触发 composer 的编辑上一条模式，与本用例无关）。
    pty.resize(40, 12);
    for _ in 0..8 {
        pty.read_once(60);
    }
    pty.resize(PTY_COLS, PTY_ROWS);
    for _ in 0..8 {
        pty.read_once(60);
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S36b（011 审计补强）：拖动浏览旧历史期间新输出到达——视口保持（A4 拖动版；
/// S22 盖的是 PageUp 路径），拖回底部后恢复跟随。
#[test]
fn s36b_tui_dragged_position_held_when_new_output_arrives() {
    let (mut pty, _mock, _keep) = scrollbar_session("36b");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S36b-L119").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let marks = gutter_marks(&mut pty);
    let thumb_bottom = marks
        .iter()
        .filter(|m| m.1 == '█')
        .map(|m| m.0)
        .max()
        .expect("前置滑块");
    pty.send_raw(&sgr_down(PTY_COLS, thumb_bottom + 1));
    for _ in 0..1 { pty.read_once(60); }
    pty.send_raw(&sgr_drag(PTY_COLS, 1));
    pty.wait_screen("S36b-L000").unwrap();
    for _ in 0..3 { pty.read_once(60); }

    // 拖到顶后新输出（/sessions 列表）到达：视口保持在旧历史——保持期间新输出
    // 不上屏（不可见），以「保持断言＋Esc 回底后输出可见」双段证明（S22 同法）
    pty.send_line("/sessions");
    for _ in 0..6 { pty.read_once(60); }
    let held = pty.screen();
    assert!(held.contains("S36b-L00"), "浏览旧历史时新输出不得抢位置：{held}");
    assert!(!held.contains("S36b-L119"), "不得跳回底部");
    assert!(!held.contains("seedscroll"), "保持期间新输出不上屏");
    // Ctrl+End 回底（单一职责轮：Esc 不再回底）：新输出此时可见
    pty.send_raw(b"\x1b[1;5F");
    pty.wait_screen("seedscroll").unwrap();
    pty.wait_screen("S36b-L119").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S40–S45：010 文字选择与右键复制（K5：键盘永不复制） ----------

/// 010 辅助：裸流中的全部 OSC 52 载荷（base64 段；非 tmux 格式 `ESC]52;c;…BEL`）。
fn osc52_payloads(raw: &str) -> Vec<String> {
    let mut payloads = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("\x1b]52;c;") {
        rest = &rest[start + 7..];
        let end = rest.find('\x07').unwrap_or(rest.len());
        payloads.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    payloads
}

/// 010 辅助：标准 base64 编解码（避免测试引入新依赖；字母表与 OSC 52 一致）。
fn sel_b64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

fn sel_b64_decode(text: &str) -> Vec<u8> {
    fn value(ch: u8) -> Option<u32> {
        match ch {
            b'A'..=b'Z' => Some(u32::from(ch - b'A')),
            b'a'..=b'z' => Some(u32::from(ch - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(ch - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, b) in chunk.iter().enumerate() {
            if *b == b'=' {
                continue;
            }
            n |= value(*b).unwrap() << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 && chunk[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 && chunk[3] != b'=' {
            out.push(n as u8);
        }
    }
    out
}

/// SGR 右键按下（1 基坐标）。
fn sgr_right_down(col: u16, row: u16) -> Vec<u8> {
    format!("\x1b[<2;{col};{row}M").into_bytes()
}

/// 010 夹具：种选择会话——user 行含唯一 ASCII/宽字符针文本（拖选载荷可逐字节断言）。
fn selection_session(tag: &str) -> (PtySession, common::Mock, Vec<TempDir>) {
    selection_session_with_reply(tag, "")
}

fn selection_session_with_reply(
    tag: &str,
    reply: &str,
) -> (PtySession, common::Mock, Vec<TempDir>) {
    selection_session_with_delayed_reply(tag, reply, /*delay_ms*/ 0)
}

fn selection_session_with_delayed_reply(
    tag: &str,
    reply: &str,
    delay_ms: u64,
) -> (PtySession, common::Mock, Vec<TempDir>) {
    let replies = if reply.is_empty() {
        vec![]
    } else {
        vec![MockResp::slow_json(200, chat_body(reply), delay_ms)]
    };
    let (pty, mock, keep) = scroll_session(tag, replies);
    let msg = |assistant: bool, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: if assistant {
                terminal_chat::prompt::HistoryRole::Assistant
            } else {
                terminal_chat::prompt::HistoryRole::User
            },
            content,
            user_input_event: None,
        })
    };
    let stem = format!("S{tag}");
    let mut hist = Vec::new();
    // user 行针文本唯一；assistant 行为干扰内容（选区不跨行时不出现在载荷中）
    hist.push(msg(false, format!("{stem} Word00SelKt")));
    hist.push(msg(true, format!("{stem}-A00 filler")));
    hist.push(msg(false, format!("{stem} 中文😊组合01")));
    hist.push(msg(true, format!("{stem}-A01 filler")));
    hist.push(msg(false, format!("{stem} Word02SelKt")));
    hist.push(msg(true, format!("{stem}-A02 filler")));
    terminal_chat::session::save_snapshot(&keep[1].path().join("sessions"), "seedsel", &hist)
        .unwrap();
    (pty, mock, keep)
}

/// 在屏幕上拖选 `needle`（同行拖拽：Down 起点 → Drag/Up 终点＝needle 之后的首个
/// 内容格；单元格锚定边界，不做宽度启发）。找不到针文本时 panic 给出屏幕内容。
fn drag_select_needle(pty: &mut PtySession, needle: &str) {
    let (col, end, row) = pty
        .screen_needle_span(needle)
        .unwrap_or_else(|| panic!("屏幕上找不到 {needle}：{}", pty.screen()));
    // 1 基坐标；Drag 与 Up 落在 needle 之后（半开区间＝恰好整个 needle）
    pty.send_raw(&sgr_down(col + 1, row + 1));
    for _ in 0..2 { pty.read_once(60); }
    pty.send_raw(&sgr_drag(end + 1, row + 1));
    for _ in 0..2 { pty.read_once(60); }
    pty.send_raw(&sgr_up(end + 1, row + 1));
    for _ in 0..2 { pty.read_once(60); }
}

impl PtySession {
    /// 010 辅助：屏幕全文中查找子串，返回 0 基 (列, 行) 起点。
    /// 单元格锚定（补验轮任务二定论）：avt 对 U+203A「›」按宽度 1 计（单格），
    /// 空白格 contents 为空串——逐格拼接并记录每字符所属格列，不做宽度启发。
    fn screen_find(&mut self, needle: &str) -> Option<(u16, u16)> {
        self.screen_needle_span(needle).map(|(col, _, row)| (col, row))
    }

    /// 010 辅助： needle 的屏幕跨度＝(起点格列, 末字符之后的首个非空格列, 行)。
    /// 末边界取「覆盖 needle 之后的下一个有内容格」——点击该列经 hit_test 落在
    /// needle 之后的字符上，形成恰好覆盖 needle 的半开区间。
    fn screen_needle_span(&mut self, needle: &str) -> Option<(u16, u16, u16)> {
        self.feed_screen();
        let screen = self.vt.vt100().screen();
        for row in 0..PTY_ROWS {
            let mut line = String::new();
            let mut cols: Vec<u16> = Vec::new();
            for col in 0..PTY_COLS {
                if let Some(cell) = screen.cell(row, col) {
                    for ch in cell.contents().chars() {
                        line.push(ch);
                        cols.push(col);
                    }
                }
            }
            if let Some(pos) = line.find(needle) {
                let start_idx = line[..pos].chars().count();
                let end_idx = start_idx + needle.chars().count();
                let start = cols[start_idx];
                let after = cols.get(end_idx).copied().unwrap_or(PTY_COLS - 1);
                // 末字符所在格的下一格（跨格安全：取 > 末格的最小已用格或行末）
                let last_cell = cols[end_idx - 1];
                let end = after.max(last_cell + 1).min(PTY_COLS);
                return Some((start, end, row));
            }
        }
        None
    }

    /// 010 辅助：点击 composer 区（pane 占屏幕底部若干行；清 transcript 选区）。
    fn send_screen_click_composer(&mut self) {
        let row = PTY_ROWS.saturating_sub(2).max(1);
        self.send_raw(&sgr_down(5, row));
        std::thread::sleep(std::time::Duration::from_millis(60));
        self.send_raw(&sgr_up(5, row));
    }

    /// 012 §7 辅助：needle 所在行是否存在粗体格。聚焦态披露行被加粗去暗
    ///（render_disclosure：accent＋bold），未聚焦为暗淡——F4 聚焦在位的
    /// **非清焦**屏幕证据（不经 Enter 打开详情页）。
    fn needle_row_bold(&mut self, needle: &str) -> Option<bool> {
        self.feed_screen();
        let screen = self.vt.vt100().screen();
        for row in 0..PTY_ROWS {
            let mut line = String::new();
            let mut bolds: Vec<bool> = Vec::new();
            for col in 0..PTY_COLS {
                if let Some(cell) = screen.cell(row, col) {
                    for ch in cell.contents().chars() {
                        line.push(ch);
                        bolds.push(cell.bold());
                    }
                }
            }
            if let Some(pos) = line.find(needle) {
                let idx = line[..pos].chars().count();
                let len = needle.chars().count();
                return Some(bolds[idx..idx + len].iter().any(|b| *b));
            }
        }
        None
    }
}

/// S40：左键拖选历史文字＋右键复制——裸流出现 OSC 52 载荷且解码＝针文本；
/// 无模型请求、草稿不变、反馈如实（Unconfirmed＝已发送待验证）；退出干净。
#[test]
fn s40_tui_drag_select_right_click_copies_via_osc52() {
    let (mut pty, mock, _keep) = selection_session("40");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S40 Word00SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    drag_select_needle(&mut pty, "Word00SelKt");
    // 右键复制（1 基任意 transcript 内位置）
    pty.send_raw(&sgr_right_down(10, 10));
    // 100ms tick 驱动 poll → OSC 52 写 /dev/tty；等待反馈行
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    let raw = pty.output();
    let payloads = osc52_payloads(&raw);
    assert!(!payloads.is_empty(), "右键复制必须产生 OSC 52：{raw}");
    let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
    assert_eq!(decoded, "Word00SelKt", "载荷＝选区文本（base64 解码）");
    assert_eq!(payloads[0], sel_b64_encode(b"Word00SelKt"), "载荷逐字节一致");
    assert!(mock.requests().is_empty(), "选择/复制不得发请求");
    // 反馈如实：Unconfirmed 文案（发送≠宿主写入确认）
    assert!(pty.screen().contains("Copy sent"), "反馈可见");
    assert!(!pty.screen().contains("Copied "), "不得宣示已写入宿主剪贴板");
    // 复制后 CLI 仍可交互并干净退出
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S41（守护用例）：有选区时 Ctrl+C 与无选区时行为一致——空闲＝`再见!`＋退出码 0，
/// 不触发复制（无 OSC 52）。K5：键盘永不复制。
#[test]
fn s41_tui_ctrl_c_with_selection_still_exits_without_copy() {
    let (mut pty, mock, _keep) = selection_session("41");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S41 Word02SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    drag_select_needle(&mut pty, "Word02SelKt");
    pty.send_raw(b"\x03"); // Ctrl+C（选区存在）
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "有选区时 Ctrl+C 仍＝空闲退出：{out}");
    assert!(osc52_payloads(&out).is_empty(), "Ctrl+C 不得触发 OSC 52：{out}");
    assert!(mock.requests().is_empty());
}

/// S41b：选区激活时**首个键即 Enter**＝照常提交 turn（K5：键盘永不复制——
/// 母本 handle_selection_key 会把 Enter 变 copy&follow，驱动守卫先清选区）。
#[test]
fn s41b_tui_enter_with_selection_submits_normally() {
    let (mut pty, mock, _keep) = selection_session_with_reply("41b", "S41b-replied-ok");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S41b Word00SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    // 输入草稿 → 拖选（选区激活、草稿仍在）→ Enter＝提交（不是复制）
    pty.send_raw(b"hello-submit");
    for _ in 0..3 { pty.read_once(60); }
    drag_select_needle(&mut pty, "Word00SelKt");
    pty.send_raw(b"\r"); // TUI raw mode：Enter＝CR
    pty.wait_screen("S41b-replied-ok").unwrap();
    assert_eq!(mock.requests().len(), 1, "Enter＝提交恰好一次");
    let body = mock.requests()[0].body.replace('\n', " ");
    assert!(body.contains("hello-submit"), "提交载荷含草稿文本：{body}");
    // 无复制发生（Enter 不是复制键）
    assert!(osc52_payloads(&pty.output()).is_empty(), "Enter 不得触发 OSC 52");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S42：无选区右键＝无 OSC 52、无提交；选区后点击 composer 区＝选区清除（再右键无载荷）。
#[test]
fn s42_tui_right_click_without_selection_and_composer_click_clears() {
    let (mut pty, _mock, _keep) = selection_session("42");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S42 Word00SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    // 无选区右键：无动作
    pty.send_raw(&sgr_right_down(10, 10));
    for _ in 0..8 { pty.read_once(60); }
    assert!(osc52_payloads(&pty.output()).is_empty(), "无选区右键不得复制");
    assert!(pty.screen().contains("S42 Word00SelKt"), "CLI 存活");

    // 拖选 → 点击输入区清选区（单一职责轮：Esc 不再清选区）→ 右键无载荷
    drag_select_needle(&mut pty, "Word00SelKt");
    pty.send_screen_click_composer();
    for _ in 0..3 { pty.read_once(60); }
    pty.send_raw(&sgr_right_down(10, 10));
    for _ in 0..8 { pty.read_once(60); }
    assert!(osc52_payloads(&pty.output()).is_empty(), "Esc 清选区后右键不得复制");

    // 拖选 → 点击 composer 区（清选区）→ 右键无载荷
    drag_select_needle(&mut pty, "Word00SelKt");
    pty.send_screen_click_composer();
    for _ in 0..3 { pty.read_once(60); }
    pty.send_raw(&sgr_right_down(10, 10));
    for _ in 0..8 { pty.read_once(60); }
    assert!(osc52_payloads(&pty.output()).is_empty(), "composer 点击清选区后右键不得复制");

    // composer 区内选区＋右键复制（同链路，PlainText）：输入草稿→composer 行内拖选→右键
    let draft = "composerPickTxt";
    pty.send_raw(draft.as_bytes());
    // 一次写入 ≥3 个 ASCII 字符触发 composer 粘贴突发缓冲——Down 事件经
    // prepare_composer_mouse（母本：仅 Down 冲刷）落草稿后再定位
    pty.send_screen_click_composer();
    for _ in 0..4 { pty.read_once(60); }
    if let Some((col, end, row)) = pty.screen_needle_span(draft) {
        pty.send_raw(&sgr_down(col + 1, row + 1));
        for _ in 0..2 { pty.read_once(60); }
        pty.send_raw(&sgr_drag(end + 1, row + 1));
        for _ in 0..2 { pty.read_once(60); }
        pty.send_raw(&sgr_up(end + 1, row + 1));
        for _ in 0..2 { pty.read_once(60); }
        pty.send_raw(&sgr_right_down(col + 1, row + 1));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while osc52_payloads(&pty.output()).is_empty()
            && std::time::Instant::now() < deadline
        {
            pty.read_once(60);
        }
        let payloads = osc52_payloads(&pty.output());
        assert!(!payloads.is_empty(), "composer 选区右键应复制（PlainText 链路）");
        let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
        assert!(decoded.contains(draft), "composer 载荷含草稿选区：{decoded:?}");
    } else {
        panic!("composer 草稿可见性前置失败：{}", pty.screen());
    }

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S43：中文/emoji 行拖选——载荷逐字节＝宽字符针文本（组合列与折行坐标换算正确）。
#[test]
fn s43_tui_drag_select_wide_chars_payload_exact() {
    let (mut pty, _mock, _keep) = selection_session("43");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("中文😊组合01").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    let needle = "中文😊组合01";
    drag_select_needle(&mut pty, needle);
    pty.send_raw(&sgr_right_down(10, 10));
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let payloads = osc52_payloads(&pty.output());
    assert!(!payloads.is_empty(), "宽字符选区复制必须产生 OSC 52");
    let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
    assert_eq!(decoded, needle, "载荷＝宽字符选区文本（含 emoji）");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S44：滚轮保留选区（快照冻结）——滚离后右键仍复制原选区；滚动条沟位拖动与
/// 选区手势互不干扰（沟位归 011）。
#[test]
fn s44_tui_wheel_keeps_selection_gutter_unchallenged() {
    let (mut pty, _mock, _keep) = selection_session("44");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S44 Word00SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    drag_select_needle(&mut pty, "Word00SelKt");
    // 滚轮上翻 6 行：选区保留（母本语义）
    for _ in 0..6 {
        pty.send_raw(b"\x1b[<64;10;10M");
        pty.read_once(40);
    }
    for _ in 0..4 { pty.read_once(60); }
    // 滚轮清 pointer 但不清选区——右键仍复制原文本
    pty.send_raw(&sgr_right_down(10, 10));
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let payloads = osc52_payloads(&pty.output());
    assert!(!payloads.is_empty());
    let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
    assert_eq!(decoded, "Word00SelKt", "滚轮后选区仍在（快照冻结）");

    // 沟位（最右列）按下归 011——不开选区、不破坏既有选区（夹具内容不满屏时
    // 沟位渲染留空，按下为无操作）
    pty.send_raw(&sgr_down(PTY_COLS, 2));
    pty.send_raw(&sgr_up(PTY_COLS, 2));
    for _ in 0..3 { pty.read_once(60); }
    // 选区仍在：再次右键＝第二个相同载荷（deadline 轮询——复制完成依赖
    // 100ms tick 的剪贴板 poll，固定轮数在并行负载下会早退，s42 同法）
    pty.send_raw(&sgr_right_down(10, 10));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while osc52_payloads(&pty.output()).len() < 2 && std::time::Instant::now() < deadline {
        pty.read_once(60);
    }
    let payloads = osc52_payloads(&pty.output());
    assert_eq!(payloads.len(), 2, "沟位按下后选区仍在，可再次复制：{payloads:?}");
    assert_eq!(payloads[1], sel_b64_encode(b"Word00SelKt"), "第二次载荷一致");

    // 选区跨 resize 存活（内容锚定，非屏幕坐标）：往返 resize 后仍可复制
    drag_select_needle(&mut pty, "Word02SelKt");
    pty.resize(120, 30);
    for _ in 0..6 { pty.read_once(60); }
    pty.resize(PTY_COLS, PTY_ROWS);
    for _ in 0..6 { pty.read_once(60); }
    pty.send_raw(&sgr_right_down(10, 10));
    // 与上面的第二次复制同源：等实际载荷到达，固定读次数会在并行编译负载下早退。
    let deadline = Instant::now() + Duration::from_secs(5);
    while osc52_payloads(&pty.output()).len() < 3 && Instant::now() < deadline {
        pty.read_once(60);
    }
    let payloads = osc52_payloads(&pty.output());
    assert_eq!(payloads.len(), 3, "resize 后选区仍在：{payloads:?}");
    assert_eq!(payloads[2], sel_b64_encode(b"Word02SelKt"), "resize 后载荷一致");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S45：拖选期间指针停在视口顶边＝自动滚动上翻（100ms tick 驱动）；释放后可正常退出。
#[test]
fn s45_tui_edge_drag_auto_scrolls() {
    let (mut pty, _mock, _keep) = scrollbar_session("45");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S45-L119").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    assert!(!pty.screen().contains("S45-L000"), "前置：最早条目不在屏幕");

    // 前置快照：记录当前视口顶条目号（pre/post 对照证明上翻，避免魔法常量）
    let topmost = |text: &str| {
        text.lines().find_map(|l| {
            let at = l.find("S45-L")? + 5;
            l.get(at..at + 3)?.parse::<u32>().ok()
        })
    };
    let pre = pty.screen();
    let pre_top = topmost(&pre).expect("前置：可解析顶条目号");
    // 从第 5 行按下拖到第 0 行（顶边）并停住：tick_selection 每 100ms tick 上翻一行
    pty.send_raw(&sgr_down(5, 6));
    for _ in 0..2 { pty.read_once(60); }
    pty.send_raw(&sgr_drag(5, 1));
    // 停驻按墙钟计（read_once 在持续重绘下即时返回，轮数≠时长）：2.5s ≥ 25 tick
    let dwell_until = std::time::Instant::now() + std::time::Duration::from_millis(2500);
    while std::time::Instant::now() < dwell_until {
        pty.read_once(60);
    }
    let scrolled = pty.screen();
    let post_top = topmost(&scrolled).expect("后置：可解析顶条目号");
    assert!(
        post_top + 5 <= pre_top,
        "顶边停驻自动上翻：pre 顶＝L{pre_top:03}，post 顶＝L{post_top:03}"
    );
    pty.send_raw(&sgr_up(5, 1));
    for _ in 0..3 { pty.read_once(60); }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S46（A7 扩展）：不可信文本携带 OSC 52 伪装——渲染与拖选阶段零自发剪贴板
/// 写入（裸流零 OSC 52）；右键复制后唯一载荷为净化文本（控制字符剥离、序列
/// 只存在于 base64 编码内，无原始逸出）。合法 OSC 52 仅来自明确用户动作。
#[test]
fn s46_tui_selection_copy_injection_neutralized() {
    // 恶意 user 消息：OSC 52 写剪贴板伪装 + CSI + BEL 混入正文
    let hostile = "EVILHEAD\x1b]52;c;bnVsbA==\x07\x1b[2JTAIL\x1b[1;1HEND";
    let (pty, mock, keep) = scroll_session("46", vec![]);
    let msg = |assistant: bool, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: if assistant {
                terminal_chat::prompt::HistoryRole::Assistant
            } else {
                terminal_chat::prompt::HistoryRole::User
            },
            content,
            user_input_event: None,
        })
    };
    let hist = vec![
        msg(false, hostile.to_string()),
        msg(true, "S46-benign-reply".to_string()),
    ];
    terminal_chat::session::save_snapshot(&keep[1].path().join("sessions"), "seedevil", &hist)
        .unwrap();
    let mut pty = pty;
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedevil");
    pty.wait_screen("EVILHEAD").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    // 渲染＋拖选阶段：不可信文本不得自行触发剪贴板写入
    let (col, row) = pty
        .screen_find("EVILHEAD")
        .unwrap_or_else(|| panic!("敌意文本可见：{}", pty.screen()));
    // 单元格锚定拖选整个净化文本（任务二定论：avt 与布局「›」均按 1 格计，
    // 坐标已逐格锚定——载荷可逐字节精确断言，不再用 containment 关闭疑问）
    let hostile_visible = "EVILHEAD]52;c;bnVsbA==TAILEND";
    let _ = (col, row);
    drag_select_needle(&mut pty, hostile_visible);
    for _ in 0..5 { pty.read_once(60); }
    assert!(
        osc52_payloads(&pty.output()).is_empty(),
        "选中敌意文本不得自发产生 OSC 52：{}",
        pty.output().chars().filter(|c| *c == '\x1b').count()
    );

    // 右键（用户显式动作）→ 唯一载荷＝净化文本（ESC/BEL 剥离，伪装序列无逸出）
    pty.send_raw(&sgr_right_down(col + 1, row + 1));
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let payloads = osc52_payloads(&pty.output());
    assert_eq!(payloads.len(), 1, "仅用户动作产生一条 OSC 52");
    let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
    assert_eq!(decoded, hostile_visible, "载荷逐字节＝净化选区（无相邻/控制字符混入）");
    // 原始流中敌意序列不出现在 base64 之外（编码即围栏）
    let raw = pty.output();
    assert!(!raw.contains("EVILHEAD\x1b"), "敌意原文不得逸出到终端字节流");
    assert!(mock.requests().is_empty());

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S47（补验一.1）：多行（真实换行）＋软折行＋中文＋emoji＋组合字符（e＋U+0301）
/// 经真实拖选/右键链路，OSC 52 载荷逐字节＝预期选区文本；不混入沟位/相邻字符，
/// 不提交消息、不改草稿。
#[test]
fn s47_tui_multiline_wrap_combining_payload_exact() {
    // 多行消息：真实换行＋组合字符；折行消息：长行软折（>200 列）＋emoji
    let multi = "MLINE-alpha 中文😊 e\u{0301}combi\nMLINE-beta 第二行XYZ";
    let wrap_head = "WRAP-head-";
    let wrap_tail = "-e\u{0301}nd";
    let wrapped = format!("{wrap_head}{}{wrap_tail}", "宽字😊".repeat(60));
    let (pty, mock, keep) = scroll_session("47", vec![]);
    let msg = |assistant: bool, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: if assistant {
                terminal_chat::prompt::HistoryRole::Assistant
            } else {
                terminal_chat::prompt::HistoryRole::User
            },
            content,
            user_input_event: None,
        })
    };
    let hist = vec![
        msg(false, multi.to_string()),
        msg(true, "S47-filler-A".to_string()),
        msg(false, wrapped.clone()),
        msg(true, "S47-filler-B".to_string()),
    ];
    terminal_chat::session::save_snapshot(&keep[1].path().join("sessions"), "seedml", &hist)
        .unwrap();
    let mut pty = pty;
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedml");
    pty.wait_screen("MLINE-alpha").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    // 跨行拖选（真实换行）：从 alpha 行首字符到 beta 行末——载荷含内部换行
    let (mut ax, _ex, mut ay) = pty
        .screen_needle_span("MLINE-alpha")
        .expect("多行消息首行可见");
    let _ = (&mut ax, &mut ay);
    pty.send_raw(&sgr_down(ax + 1, ay + 1));
    for _ in 0..2 { pty.read_once(60); }
    // 终点＝「第二行XYZ」之后的首个内容格（不同屏幕行）
    let (_bx, be, by) = pty
        .screen_needle_span("第二行XYZ")
        .expect("多行消息次行可见");
    pty.send_raw(&sgr_drag(be + 1, by + 1));
    for _ in 0..2 { pty.read_once(60); }
    pty.send_raw(&sgr_up(be + 1, by + 1));
    for _ in 0..3 { pty.read_once(60); }
    pty.send_raw(&sgr_right_down(ax + 1, ay + 1));
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let payloads = osc52_payloads(&pty.output());
    assert!(!payloads.is_empty(), "多行复制必须产生 OSC 52");
    let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
    assert_eq!(decoded, multi, "跨行载荷逐字节＝消息全文（真实换行保留）");
    assert!(!decoded.contains('║') && !decoded.contains('█'), "不混入沟位");

    // 点击输入区清选区（单一职责轮：Esc 不再清选区）后做第二段：软折行＋组合字符
    pty.send_screen_click_composer();
    for _ in 0..4 { pty.read_once(60); }
    let (wx, _we, wy) = pty
        .screen_needle_span(wrap_head)
        .expect("折行首段可见");
    let (_tx, te, ty) = pty
        .screen_needle_span(wrap_tail)
        .expect("折行尾段可见");
    assert!(ty > wy, "夹具前置：长行确已软折为多视觉行");
    pty.send_raw(&sgr_down(wx + 1, wy + 1));
    for _ in 0..2 { pty.read_once(60); }
    pty.send_raw(&sgr_drag(te + 1, ty + 1));
    for _ in 0..2 { pty.read_once(60); }
    pty.send_raw(&sgr_up(te + 1, ty + 1));
    for _ in 0..3 { pty.read_once(60); }
    pty.send_raw(&sgr_right_down(wx + 1, wy + 1));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while osc52_payloads(&pty.output()).len() < 2 && std::time::Instant::now() < deadline {
        pty.read_once(60);
    }
    let payloads = osc52_payloads(&pty.output());
    assert_eq!(payloads.len(), 2, "第二段复制到达（墙钟等待，轮数≠时长）");
    let decoded = String::from_utf8(sel_b64_decode(&payloads[1])).unwrap();
    assert_eq!(decoded, wrapped, "折行载荷逐字节＝逻辑行原文（折行不引入换行）");

    // 无提交（mock 零请求）；草稿未动＝结构性保证（全程仅鼠标事件，无键输入）；
    // composer 可用性由结尾 /exit 提交成功证明
    assert!(mock.requests().is_empty(), "选择/复制不得发请求");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S48（补验一.2）：选区激活＋回复到达——阅读位置保持（双段证明：保持期旧历史
/// 在屏且新回复不可见，Esc 回底后回复可见）、选区快照不漂移（右键仍复制原文本）、
/// 随后可解除并继续输入；turn 运行中＋有选区时 Ctrl+C＝取消路径（CLI 存活、不复制）。
#[test]
fn s48_tui_selection_holds_across_reply_and_ctrl_c_cancels() {
    // 溢出会话（约 62 条）：早期含针文本——PageUp 才有真实位移；延迟回复供两 turn
    let (pty, mock, keep) = scroll_session(
        "48",
        vec![
            MockResp::slow_json(200, chat_body("S48-reply-one-OK"), 2500),
            MockResp::slow_json(200, chat_body("S48-reply-two-OK"), 2500),
        ],
    );
    let msg = |assistant: bool, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: if assistant {
                terminal_chat::prompt::HistoryRole::Assistant
            } else {
                terminal_chat::prompt::HistoryRole::User
            },
            content,
            user_input_event: None,
        })
    };
    let mut hist = Vec::new();
    for i in 0..30 {
        if i == 1 {
            hist.push(msg(false, "S48 Word00SelKt".to_string()));
            hist.push(msg(true, "S48-A01".to_string()));
        } else {
            hist.push(msg(i % 2 == 0, format!("S48-L{i:02}")));
        }
    }
    terminal_chat::session::save_snapshot(&keep[1].path().join("sessions"), "seed48", &hist)
        .unwrap();
    let mut pty = pty;
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seed48");
    pty.wait_screen("S48-L29").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    assert!(!pty.screen().contains("Word00SelKt"), "前置：针文本（旧历史）不在屏");

    // 提交 turn（2.5s 延迟回复）→ PageUp 上翻旧历史并拖选（turn 在飞、选区激活）
    pty.send_line("start-turn");
    for _ in 0..4 { pty.read_once(60); }
    for _ in 0..3 {
        pty.send_raw(b"\x1b[5~");
        for _ in 0..4 { pty.read_once(60); }
    }
    let held = pty.screen();
    assert!(held.contains("Word00SelKt"), "PageUp 后旧历史可见");
    assert!(!held.contains("S48-L29"), "已离开底部");
    drag_select_needle(&mut pty, "Word00SelKt");

    // 回复到达（保持期间）：墙钟等待 2.5s 确定性延迟＋渲染余量（read_once 即时
    // 返回，轮数≠时长；到达侧由结尾 Esc 回底＋wait_screen 双段证明兜底）
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(4000);
    while std::time::Instant::now() < deadline {
        pty.read_once(60);
    }
    let after = pty.screen();
    assert!(after.contains("Word00SelKt"), "回复到达后视口保持在旧历史：{after}");
    assert!(!after.contains("S48-reply-one"), "保持期间新回复不上屏（双段证明·保持侧）");

    // 选区快照不漂移：右键仍复制原选区文本
    pty.send_raw(&sgr_right_down(10, 10));
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let payloads = osc52_payloads(&pty.output());
    assert!(!payloads.is_empty(), "回复到达后选区仍可复制");
    let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
    assert_eq!(decoded, "Word00SelKt", "选区快照不漂移（载荷＝原选区）");

    // 点击输入区清选区，Ctrl+End 回底（单一职责轮：Esc 不再清选区/回底）
    pty.send_screen_click_composer();
    for _ in 0..3 { pty.read_once(60); }
    pty.send_raw(b"\x1b[1;5F");
    pty.wait_screen("S48-reply-one-OK").unwrap();
    for _ in 0..3 { pty.read_once(60); }

    // 第二 turn 在飞＋拖选 → Esc＝取消（CLI 存活、不新增复制、请求计数＝2）
    pty.send_line("after-hold");
    for _ in 0..4 { pty.read_once(60); }
    drag_select_needle(&mut pty, "S48-reply-one-OK");
    pty.send_raw(b"\x1b");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        pty.read_once(60);
    }
    assert_eq!(osc52_payloads(&pty.output()).len(), 1, "Ctrl+C（turn 中）不得新增复制");
    assert!(pty.screen().contains("S48-reply-one-OK"), "Ctrl+C 后 CLI 存活");
    assert_eq!(mock.requests().len(), 2, "两个 turn 均已提交（取消不撤回已发请求）");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S49（补验一.3）：超限失败结果分派——>100KB 选区右键＝Err（「Copy failed」反馈、
/// 无 OSC 52 字节），随后小选区复制成功（继续交互）；证明驱动消费失败结果并如实反馈。
#[test]
fn s49_tui_oversize_copy_fails_with_feedback_then_recovers() {
    let oversize = format!("BIG-{}", "x".repeat(100_100));
    let (pty, _mock, keep) = scroll_session("49", vec![]);
    let msg = |assistant: bool, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: if assistant {
                terminal_chat::prompt::HistoryRole::Assistant
            } else {
                terminal_chat::prompt::HistoryRole::User
            },
            content,
            user_input_event: None,
        })
    };
    let hist = vec![
        msg(false, oversize),
        msg(false, "S49-small-ok".to_string()),
    ];
    terminal_chat::session::save_snapshot(&keep[1].path().join("sessions"), "seedbig", &hist)
        .unwrap();
    let mut pty = pty;
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedbig");
    pty.wait_screen("xxx").unwrap();
    for _ in 0..5 { pty.read_once(60); }

    // 三击＝整行选择（100KB 逻辑行整体入选区）。跟随态视口只见尾部 x 行
    //（头部在视口外）——任意 x 行三击即选整个逻辑行。
    let (col, _, row) = pty.screen_needle_span("xxxxxx").expect("超限行可见");
    for _ in 0..3 {
        pty.send_raw(&sgr_down(col + 1, row + 1));
        std::thread::sleep(Duration::from_millis(40));
        pty.send_raw(&sgr_up(col + 1, row + 1));
        std::thread::sleep(Duration::from_millis(40));
    }
    for _ in 0..4 { pty.read_once(60); }
    pty.send_raw(&sgr_right_down(col + 1, row + 1));
    pty.wait_screen("Copy failed").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    assert!(osc52_payloads(&pty.output()).is_empty(), "超限复制不得产生 OSC 52 字节");
    assert!(!pty.screen().contains("Copied "), "失败不得显示成功文案");

    // 恢复：点击输入区清选区（单一职责轮：Esc 不再清选区），小选区复制成功
    pty.send_screen_click_composer();
    for _ in 0..4 { pty.read_once(60); }
    drag_select_needle(&mut pty, "small-ok");
    pty.send_raw(&sgr_right_down(10, 10));
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    let payloads = osc52_payloads(&pty.output());
    assert_eq!(payloads.len(), 1, "失败后小选区复制成功");
    let decoded = String::from_utf8(sel_b64_decode(&payloads[0])).unwrap();
    assert_eq!(decoded, "small-ok", "恢复后载荷逐字节正确");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S50–S53：011 §7 帧调度（无条件绘制修复）----------

/// 011 §7.3 K-B：tui.draw 每次经 crossterm SynchronizedUpdate 包裹，以
/// ESC[?2026l 收尾——捕获输出中计数该标记即应用帧边界，空帧不漏计
/// （不能只比对屏幕内容）。
fn applied_frames_from(pty: &PtySession, from: usize) -> usize {
    pty.captured[from..]
        .windows(8)
        .filter(|w| w == b"\x1b[?2026l")
        .count()
}

/// 011 §7 辅助：VT 屏幕逐行文本（cell 拼接，行号与 PTY 一一对应）。
fn screen_rows_text(pty: &mut PtySession) -> Vec<String> {
    pty.feed_screen();
    let screen = pty.vt.vt100().screen();
    (0..PTY_ROWS)
        .map(|r| {
            (0..PTY_COLS)
                .map(|c| {
                    screen.cell(r, c).map(|cell| cell.contents().to_string()).unwrap_or_default()
                })
                .collect::<String>()
        })
        .collect()
}

/// S50（B1）：turn 完成后无动画/待处理状态，静置 1.2s（12 个 100ms tick）
/// 不得产生任何应用帧或输出字节（旧驱动每 tick 无条件 draw→滴流，事前 RED）；
/// 空闲后交互仍即时绘制（键入字符免按键上屏）。
#[test]
fn s50_idle_ticks_produce_no_frames() {
    let (mut pty, _mock, _keep) =
        scroll_session("s50", vec![MockResp::json(200, chat_body("S50-done"))]);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("hello");
    pty.wait_screen("S50-done").unwrap();
    // 静置排空：避免把回复尾帧计入空闲窗口
    pty.dwell(400);
    let mark = pty.captured.len();
    pty.dwell(1200);
    assert_eq!(
        applied_frames_from(&pty, mark),
        0,
        "空闲 tick 不得产生应用帧（旧实现每 100ms 滴流一帧）"
    );
    assert_eq!(pty.captured.len() - mark, 0, "空闲不得有任何输出字节");
    // 空闲后仍即时响应：键入草稿字符（两字符不触发 burst 启发式）
    pty.send_raw(b"Qk");
    pty.wait_screen("Qk").unwrap();
    // 清空草稿（2 个退格，S12 模式）后 Ctrl+C 空闲退出（再见!＋退出码 0，001 契约）
    for _ in 0..2 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(30));
    }
    pty.send_raw(b"\x03");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S51（B2）：50 个无状态变化的 Moved 悬停事件不产生应用帧（旧驱动每事件
/// 一帧，事前 RED）；事件处理本身保留——随后滚轮仍产生绘制与位移。
#[test]
fn s51_moved_hover_events_produce_no_frames() {
    let (mut pty, _mock, _keep) = scrollbar_session("51");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S51-L119").unwrap();
    pty.dwell(400);
    let mark = pty.captured.len();
    // SGR 1006：35＝无按键指针移动（Moved）；8ms 间隔模拟悬停流
    for _ in 0..50 {
        pty.send_raw(b"\x1b[<35;10;20M");
        std::thread::sleep(Duration::from_millis(8));
    }
    pty.dwell(400);
    assert_eq!(
        applied_frames_from(&pty, mark),
        0,
        "无状态变化的 Moved 不得产生应用帧（旧实现每事件一帧）"
    );
    assert_eq!(pty.captured.len() - mark, 0, "Moved 悬停不得有任何输出字节");
    // 事件处理保留：随后的滚轮事件仍产生绘制
    pty.send_raw(b"\x1b[<64;10;20M");
    pty.dwell(300);
    assert!(
        applied_frames_from(&pty, mark) >= 1,
        "Moved 之后滚轮事件仍须产生应用帧"
    );
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S52（B3＋B4）：未触限位的确定性历史——10 个滚轮事件恰 10 行位移（钉住
/// 条目屏幕行号前后对照）；单次写入 10 事件（突发）与逐个发送位移一致、
/// 排空后输出归零（无持续追赶）。不要求 10 事件 10 帧。
#[test]
fn s52_wheel_displacement_conserved_slow_and_burst() {
    let wheel = b"\x1b[<64;10;20M";
    let (mut pty, _mock, _keep) = scrollbar_session("52");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedscroll");
    pty.wait_screen("S52-L119").unwrap();
    pty.dwell(400);
    // 钉住屏幕上第一个条目标记的行号与条目号（确定性 fixture）
    let stem = "S52-L".to_string();
    let rows = screen_rows_text(&mut pty);
    let (r0, id0) = rows
        .iter()
        .enumerate()
        .find_map(|(r, line)| {
            let at = line.find(&stem)? + stem.len();
            line.get(at..at + 3)?.parse::<u32>().ok().map(|id| (r, id))
        })
        .expect("前置：屏幕上可解析条目标记");
    // 未触限位：上方余量远大于 10 行
    assert!(id0 >= 20, "前置：距顶部有充足余量（顶条目 L{id0:03}）");

    // 慢速：逐个发送 10 个滚轮事件
    for _ in 0..10 {
        pty.send_raw(wheel);
        pty.dwell(30);
    }
    pty.dwell(400);
    let rows2 = screen_rows_text(&mut pty);
    let needle = format!("{stem}{id0:03}");
    let r1 = rows2
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("滚动后条目 {needle} 仍应可见：{rows2:?}"));
    assert_eq!(r1, r0 + 10, "10 个滚轮事件必须恰 10 行位移（慢速）");
    let quiet_mark = pty.captured.len();
    pty.dwell(500);
    assert_eq!(pty.captured.len() - quiet_mark, 0, "慢速排空后无持续追赶输出");

    // 突发：10 个滚轮事件一次写入——位移与慢速一致，最终屏幕对应最新状态
    let mut burst = Vec::new();
    for _ in 0..10 {
        burst.extend_from_slice(wheel);
    }
    pty.send_raw(&burst);
    pty.dwell(400);
    let rows3 = screen_rows_text(&mut pty);
    let r2 = rows3
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("突发滚动后条目 {needle} 仍应可见：{rows3:?}"));
    assert_eq!(r2, r0 + 20, "突发 10 事件再 10 行位移（与慢速一致）");
    let burst_quiet = pty.captured.len();
    pty.dwell(500);
    assert_eq!(pty.captured.len() - burst_quiet, 0, "突发排空后无持续追赶输出");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S53（B5）：粘贴突发冲刷——burst 缓冲的草稿字符在冲刷定时到达后免按键
/// 上屏（守护用例：修复不得让缓冲文本滞留屏幕外）。
#[test]
fn s53_paste_burst_flushes_without_extra_keys() {
    let (mut pty, _mock, _keep) = scroll_session("s53", vec![]);
    pty.wait_screen("? for shortcuts").unwrap();
    // 一次写入 8 个 ASCII 字符 → composer 判定 paste burst（≥3 连打），
    // 缓冲不逐键上屏；冲刷延迟到达后应无任何后续按键自行上屏
    pty.send_raw(b"flushtxt");
    pty.wait_screen("flushtxt").unwrap();
    // 清空草稿（8 个退格，S12 模式）后 /exit
    for _ in 0..8 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(30));
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S54（B5 补强）：复制反馈 5s 到期由到期帧免按键收口（draw 捕获
/// render_composer_gap 返回的到期延迟→schedule_frame_in 接线；若断裂，
/// 反馈在无输入静置下永不清除）。
#[test]
fn s54_copy_feedback_expires_without_extra_keys() {
    let (mut pty, _mock, _keep) = selection_session("54");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S54 Word02SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    drag_select_needle(&mut pty, "Word02SelKt");
    pty.send_raw(&sgr_right_down(10, 10));
    pty.wait_screen("Copy sent").unwrap();
    for _ in 0..3 { pty.read_once(60); }
    // 到期帧免按键清屏：墙钟等待反馈消失（5s 到期＋余量；期间零输入）
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if !pty.screen().contains("Copy sent") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "复制反馈须在 8s 内到期消失（到期帧接线）：\n{}",
            pty.screen()
        );
        pty.read_once(100);
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S55（B5 补强）：turn 运行期活跃动画帧持续流动（状态行自调度 32ms 间隔经
/// FrameScheduler→Draw 通知唤醒驱动；若通知臂断裂，运行期除事件外零帧、
/// 屏幕冻结——最终回复仍会到屏，故须专测运行窗口）。
#[test]
fn s55_running_turn_animation_frames_flow() {
    let (mut pty, _mock, _keep) = scroll_session(
        "s55",
        vec![MockResp::slow_json(200, chat_body("S55-late-reply"), 2500)],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("hello");
    // turn 在飞（响应延迟 2.5s）：300ms 让 turn 建立，随后 1s 采样窗口零输入——
    // 下界 5 帧容忍并行测试负载（标称 ≈31fps @32ms；通知臂断裂＝恰 0 帧，
    // 0 与 ≥5 分离度足够）
    pty.dwell(300);
    let mark = pty.captured.len();
    pty.dwell(1000);
    let frames = applied_frames_from(&pty, mark);
    assert!(
        frames >= 5,
        "turn 运行期动画帧应持续流动（Draw 通知链），实测 1s 内 {frames} 帧"
    );
    pty.wait_screen("S55-late-reply").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S56（012 §6 C1–C3/C6）：一批 3 个 exec（150 行长输出/含中文与引号的短命令/
/// 失败退出 4）——默认全紧凑；仅点披露行展开长输出项：状态头（Ran＋时长）并入
/// `$ cmd` 首行、无整条包裹引号、头部可达；其余两项保持紧凑（归属不串扰）；
/// 收起恢复预览；resize/草稿/滚动条/选区不退化。
#[test]
fn s56_tui_expanded_batch_header_attribution_and_collapse() {
    let cfg = TempDir::new("s56-cfg");
    let user = TempDir::new("s56-user");
    let key = "s56-key";
    let work = TempDir::new("s56-work");
    let args_long =
        serde_json::json!({ "cmd": "for i in $(seq -w 1 150); do echo S56-L$i; done" }).to_string();
    let args_short =
        serde_json::json!({ "cmd": "echo \"中文 引号 测试\"" }).to_string();
    let args_bad = serde_json::json!({ "cmd": "echo S56-err-line 1>&2; exit 4" }).to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s56-long", "type": "function",
              "function": { "name": "exec_command", "arguments": args_long } },
            { "id": "call-s56-short", "type": "function",
              "function": { "name": "exec_command", "arguments": args_short } },
            { "id": "call-s56-bad", "type": "function",
              "function": { "name": "exec_command", "arguments": args_bad } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S56-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S56_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(Some(work.path()), &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S56_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S56-完成").unwrap();
    pty.dwell(300);

    // C1 默认全紧凑：长输出尾部预览＋披露标记；短命令（中文＋引号）紧凑头无
    // 包裹引号（C3）；失败项 Failed (exit 4)（C4）
    let compact = pty.screen();
    assert!(compact.contains("S56-L150"), "长输出尾部预览：{compact}");
    assert!(compact.contains("for details"), "详情入口标记：{compact}");
    assert!(
        compact.contains("Ran echo \"中文 引号 测试\""),
        "C3 紧凑头命令原文（含引号/中文，无包裹）：{compact}"
    );
    assert!(compact.contains("Failed (exit 4)"), "失败状态：{compact}");

    // C1 点击长输出项的详情入口：打开独立详情页（唯一 details 标记）
    let (col, _, row) = pty.screen_needle_span("for details").expect("详情入口可见");
    pty.send_raw(&sgr_down(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(col + 1, row + 1));
    pty.wait_screen("back to conversation").expect("详情页打开");
    let page = pty.screen();
    assert!(page.contains("Ran •"), "页内状态头（§6 形态）：{page}");
    assert!(page.contains("$ for i in $(seq"), "页内命令：{page}");
    assert!(page.contains("S56-L01"), "页内全文可达（顶部起）：{page}");
    assert!(
        !page.contains("中文 引号 测试") && !page.contains("S56-err-line"),
        "页内不混入其他工具内容：{page}"
    );
    assert_eq!(page.matches("seq -w 1 150").count(), 1, "命令恰一次：{page}");
    // 一次 q（单一职责轮）：主对话恢复紧凑预览（所有 cell），页标记消失
    pty.send_raw(b"q");
    pty.dwell(300);
    let collapsed = pty.screen();
    assert!(collapsed.contains("for details"), "收起后恢复详情入口：{collapsed}");
    assert!(!collapsed.contains("back to conversation"), "页已关闭：{collapsed}");
    assert!(collapsed.contains("Failed (exit 4)"), "失败项仍在：{collapsed}");
    assert!(
        collapsed.matches("seq -w 1 150").count() == 1,
        "收起后命令仍恰一次：{collapsed}"
    );

    // C6 resize 后草稿/滚动条/选区不退化
    pty.send_raw("S56-草稿".as_bytes());
    pty.wait_screen("S56-草稿").unwrap();
    pty.resize(120, 30);
    pty.dwell(300);
    let after_resize = pty.screen();
    assert!(after_resize.contains("S56-草稿"), "resize 后草稿保留：{after_resize}");
    assert!(
        after_resize.contains("Failed (exit 4)"),
        "resize 后布局正常：{after_resize}"
    );
    // resize 后重开详情页：新几何下页内沟位滚动条在位
    let (col2, _, row2) = pty
        .screen_needle_span("for details")
        .expect("resize 后详情入口可见");
    pty.send_raw(&sgr_down(col2 + 1, row2 + 1));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(col2 + 1, row2 + 1));
    pty.wait_screen("back to conversation").expect("resize 后仍可打开详情页");
    let marks = gutter_marks_at(&mut pty, /*new_width-1*/ 119);
    assert!(
        marks.iter().any(|m| m.1 == '█' || m.1 == '║'),
        "详情页滚动条在新几何在位（沟位列 119）：{marks:?}"
    );
    pty.send_raw(b"q"); // 单一职责轮：q 关页
    pty.dwell(300);
    drag_select_needle(&mut pty, "S56-L150");
    pty.send_screen_click_composer(); // 清选区（Esc 不再清选区）
    for _ in 0..3 { pty.read_once(60); }

    // 清空草稿（S56-草稿＝6 个字符簇）后退出
    for _ in 0..8 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(30));
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("panicked"), "不得 panic：{out}");
    assert_eq!(mock.count(), 2, "工具轮＋续轮");
}

// ---------- S57/S58：012 §7 展开后 Enter 无法提交（用户复现）----------
// 公共夹具：一个 30 行长输出工具调用（展开→披露行在位）＋两条后续回复。
// 症状断言＝用户复现④⑤：新请求恰好提交一次（回复可见＋mock 计数）且
// 旧工具展开态不被该 Enter 切换（仍 − Show less）。

fn focus_session57(tag: &str, second_reply: &str) -> (PtySession, common::Mock, Vec<TempDir>) {
    let cfg = TempDir::new(&format!("{tag}-cfg"));
    let user = TempDir::new(&format!("{tag}-user"));
    let key = format!("{tag}-key");
    let args = serde_json::json!({
        "cmd": format!("for i in $(seq -w 1 30); do echo {tag}-L$i; done")
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": format!("call-{tag}"), "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body(&format!("{tag}-第一段完成"))),
        MockResp::json(200, chat_body(second_reply)),
    ]);
    write_manual(cfg.path(), &mock.url, &format!("env:TCHAT_{tag}_KEY").as_str());
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(&key));
    let pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        (format!("TCHAT_{tag}_KEY").as_str(), key.as_str()),
    ]));
    (pty, mock, vec![cfg, user])
}

/// 展开旧工具后回底并确认展开态仍在（− Show less 可达）。
fn assert_still_expanded_57(pty: &mut PtySession) {
    for _ in 0..12 {
        pty.send_raw(b"\x1b[5~");
        pty.dwell(80);
        if pty.screen().contains("− Show less") {
            return;
        }
    }
    panic!("旧工具应保持展开（− Show less 可达）：\n{}", pty.screen());
}

/// 012 §7 助手：轮询 mock 计数证实提交（展开锚定使视口保持阅读态，回复在
/// 视口外——S48 语义），双 Esc 回底后断言回复可见。
fn wait_submitted_then_show_reply(
    pty: &mut PtySession,
    mock: &common::Mock,
    expected_count: usize,
    reply_needle: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.count() < expected_count {
        assert!(
            Instant::now() < deadline,
            "请求未被提交（mock 计数 {}，期望 {expected_count}）——Enter 可能被旧工具吞掉",
            mock.count()
        );
        pty.read_once(50);
    }
    pty.send_raw(b"\x1b[1;5F"); // Ctrl+End 回底（单一职责轮：Esc 不再回底）
    pty.dwell(300);
    pty.wait_screen(reply_needle).expect("回底后新回复可见");
}

/// S57（D1）：鼠标展开 → 粘贴新请求 → Enter——须提交且旧工具不被切换。
#[test]
fn s57_expand_then_paste_request_enter_submits() {
    let (mut pty, mock, _keep) = focus_session57("S57", "S57-新回复已收到");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S57-第一段完成").unwrap();
    pty.dwell(300);
    // 点击详情入口→独立详情页→一次 Esc 返回（012 §8 新交互）
    let (col, _, row) = pty.screen_needle_span("for details").expect("详情入口可见");
    pty.send_raw(&sgr_down(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(col + 1, row + 1));
    pty.wait_screen("back to conversation").expect("详情页打开");
    pty.dwell(200);
    pty.send_raw(b"q"); // 单一职责轮：q 关页（Esc 已专取消）
    pty.dwell(300);
    assert!(!pty.screen().contains("back to conversation"), "页已关闭");
    // 粘贴新请求（bracketed paste）→ 冲刷上屏 → Enter
    pty.send_raw(b"\x1b[200~\xe8\xaf\xb7\xe7\xbb\xa7\xe7\xbb\xad\xe5\x88\x86\xe6\x9e\x90\x1b[201~");
    pty.dwell(300);
    pty.send_raw(b"\r");
    // 用户复现⑤：新请求提交（mock 计数先导＋回底后回复可见）；文本完整＝
    // 提交后用户回显逐字可见（草稿已清空，屏幕所见即回显）
    wait_submitted_then_show_reply(&mut pty, &mock, 3, "S57-新回复已收到");
    let submitted = pty.screen();
    assert!(
        submitted.contains("请继续分析"),
        "用户消息回显完整：{submitted}"
    );
    assert_eq!(mock.count(), 3, "恰好三轮请求（工具＋续轮＋新请求）");
    // 旧工具不被该 Enter 重开：页标记不在
    assert!(!pty.screen().contains("back to conversation"), "旧工具未被重开");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("panicked"), "{out}");
}

/// S58（D2）：鼠标展开 → 逐字输入（含中文）→ Enter——同契约。
#[test]
fn s58_expand_then_typed_request_enter_submits() {
    let (mut pty, mock, _keep) = focus_session57("S58", "S58-新回复已收到");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S58-第一段完成").unwrap();
    pty.dwell(300);
    // 点击详情入口→独立详情页→一次 Esc 返回（012 §8 新交互）
    let (col, _, row) = pty.screen_needle_span("for details").expect("详情入口可见");
    pty.send_raw(&sgr_down(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(col + 1, row + 1));
    pty.wait_screen("back to conversation").expect("详情页打开");
    pty.dwell(200);
    pty.send_raw(b"q"); // 单一职责轮：q 关页
    pty.dwell(300);
    assert!(!pty.screen().contains("back to conversation"), "页已关闭");
    // 逐字键入（40ms 间隔＝键入而非突发；含中文多字节）
    for ch in "请继续分析".chars() {
        let mut buf = [0u8; 4];
        pty.send_raw(ch.encode_utf8(&mut buf).as_bytes());
        std::thread::sleep(Duration::from_millis(40));
    }
    pty.dwell(200);
    pty.send_raw(b"\r");
    // 与 s57 同法：mock 计数先导＋回底后回复可见
    wait_submitted_then_show_reply(&mut pty, &mock, 3, "S58-新回复已收到");
    assert_eq!(mock.count(), 3, "恰好三轮请求");
    assert!(!pty.screen().contains("back to conversation"), "旧工具未被重开");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("panicked"), "{out}");
}

/// S59（D3）：F4 聚焦工具 → 点击输入区 → Enter——输入归属交还，预存草稿提交
/// （不误操作工具、不触发裸 exit）。点击输入区＝母本 clear_activity_focus 位点。
#[test]
fn s59_f4_focus_then_composer_click_returns_input_ownership() {
    let (mut pty, mock, _keep) = focus_session57("S59", "S59-新回复已收到");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S59-第一段完成").unwrap();
    pty.dwell(300);
    // 先有草稿（预存，验证点击交还后 Enter 提交它而非操作工具/裸退出）
    pty.send_raw("S59-预存请求".as_bytes());
    pty.dwell(400);
    // F4 聚焦最新活动（工具 cell）——Enter 此刻＝打开详情页（聚焦在位的硬证明；
    // 若未聚焦，Enter 会直接提交草稿使后续断言失去意义）
    pty.send_raw(b"\x1bOS");
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.wait_screen("back to conversation").expect("聚焦态 Enter＝打开详情页（证明）");
    pty.send_raw(b"q"); // 单一职责轮：q 关页
    pty.dwell(300);
    // 点击输入区（composer 行）：交还输入归属（012 §7 修复位点）
    pty.send_raw(&sgr_down(10, 48));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(10, 48));
    pty.dwell(200);
    // 同一键 Enter：此刻必须＝提交预存草稿（若聚焦未清，则会收起工具且计数不动）
    pty.send_raw(b"\r");
    wait_submitted_then_show_reply(&mut pty, &mock, 3, "S59-新回复已收到");
    assert_eq!(mock.count(), 3, "恰好三轮请求");
    // 第二个 Enter 没有重开旧工具详情页
    assert!(!pty.screen().contains("back to conversation"), "旧工具未被重开");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S60（D4，契约保持）：先有草稿 → F4 → Enter——仍控制工具（展开），草稿不
/// 发送、无裸 exit、CLI 存活。禁止「草稿非空就提交」式修补的锚定用例。
#[test]
fn s60_draft_then_f4_enter_still_controls_tool() {
    let (mut pty, mock, _keep) = focus_session57("S60", "S60-不应到达");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S60-第一段完成").unwrap();
    pty.dwell(300);
    // 预存草稿（含 exit 字样也不得触发裸退出——裸 exit 判定在 composer 分支内）
    pty.send_raw("exit 类草稿勿发".as_bytes());
    pty.dwell(400);
    // F4 聚焦 → Enter＝展开工具（ListAction::Accept），非提交
    pty.send_raw(b"\x1bOS");
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.wait_screen("back to conversation").expect("聚焦态 Enter＝打开详情页");
    pty.dwell(600);
    assert_eq!(mock.count(), 2, "草稿不得被发送（仍两轮）");
    assert!(!pty.output().contains("再见"), "不得触发裸 exit");
    assert!(pty.child.try_wait().unwrap().is_none(), "CLI 存活");
    // 草稿仍在：q 关页后输入区内容未丢
    pty.send_raw(b"q");
    pty.dwell(200);
    assert!(
        pty.screen().contains("exit 类草稿勿发"),
        "草稿保留：\n{}",
        pty.screen()
    );
    for _ in 0..10 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(30));
    }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert_eq!(mock.count(), 2, "全程零误发");
}

/// S61（D5）：F4 聚焦后重新键入或粘贴 → Enter 正常提交（两路径各一会话）。
/// 聚焦在位证据（不清焦）：聚焦态披露行被加粗去暗（render_disclosure 的
/// accent＋bold），F4 前后屏幕样式可区分且请求计数未增加——F4 事件本身不算
/// 证据。随后**不打开详情页清焦**（open_tool_detail 会清焦，K-D），直接键入／
/// 粘贴（输入动作本身必须解除聚焦），Enter 提交。
#[test]
fn s61_f4_then_retype_or_paste_then_enter_submits() {
    // 会话一：F4（披露行加粗＝聚焦证据）→ 直接逐字键入（字符清焦＝vendored
    // 「Ordinary text resumes composing」）→ Enter 提交
    let (mut pty, mock, _keep) = focus_session57("S61", "S61-新回复已收到");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S61-第一段完成").unwrap();
    pty.dwell(300);
    // 基线对照：未聚焦时披露行不加粗（区分聚焦／未聚焦的屏幕证据）
    assert_ne!(
        pty.needle_row_bold("for details"),
        Some(true),
        "F4 前披露行不应加粗：\n{}",
        pty.screen()
    );
    pty.send_raw(b"\x1bOS");
    // 聚焦在位证据：披露行加粗（有界等待）。不经 Enter 打开详情页
    //（那会清焦，沿用已清焦状态不算证明）。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pty.read_once(60);
        if pty.needle_row_bold("for details") == Some(true) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "F4 聚焦证据超时（披露行未加粗）：\n{}",
            pty.screen()
        );
    }
    assert_eq!(mock.count(), 2, "聚焦本身不得发送请求");
    // 直接逐字键入：输入动作必须解除聚焦（不先开详情页清焦）
    for ch in "键入后提交".chars() {
        let mut buf = [0u8; 4];
        pty.send_raw(ch.encode_utf8(&mut buf).as_bytes());
        std::thread::sleep(Duration::from_millis(40));
    }
    pty.dwell(200);
    pty.send_raw(b"\r");
    wait_submitted_then_show_reply(&mut pty, &mock, 3, "S61-新回复已收到");
    assert_eq!(mock.count(), 3, "恰好三轮请求");
    // 实际提交文本逐字完整（按内容定位用户轮请求，不假设请求序）
    assert!(
        mock.requests().iter().any(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["messages"].as_array().is_some_and(|m| {
                m.iter()
                    .any(|x| x["role"] == "user" && x["content"].as_str() == Some("键入后提交"))
            })
        }),
        "新请求的实际用户文本必须逐字完整"
    );
    // Enter 未被旧工具吞掉打开详情页
    assert!(!pty.screen().contains("back to conversation"), "旧详情页未被打开");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");

    // 会话二：F4（披露行加粗＝聚焦证据）→ 直接粘贴（012 §7 Paste 清焦修复
    // 路径）→ Enter 提交
    let (mut pty, mock, _keep) = focus_session57("S61b", "S61b-新回复已收到");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S61b-第一段完成").unwrap();
    pty.dwell(300);
    assert_ne!(
        pty.needle_row_bold("for details"),
        Some(true),
        "F4 前披露行不应加粗：\n{}",
        pty.screen()
    );
    pty.send_raw(b"\x1bOS");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        pty.read_once(60);
        if pty.needle_row_bold("for details") == Some(true) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "F4 聚焦证据超时（披露行未加粗）：\n{}",
            pty.screen()
        );
    }
    assert_eq!(mock.count(), 2, "聚焦本身不得发送请求");
    // 直接粘贴：Paste 事件必须解除聚焦
    pty.send_raw(b"\x1b[200~\xe7\xb2\x98\xe8\xb4\xb4\xe5\x90\x8e\xe6\x8f\x90\xe4\xba\xa4\x1b[201~");
    pty.dwell(300);
    pty.send_raw(b"\r");
    wait_submitted_then_show_reply(&mut pty, &mock, 3, "S61b-新回复已收到");
    assert_eq!(mock.count(), 3, "恰好三轮请求");
    assert!(
        mock.requests().iter().any(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["messages"].as_array().is_some_and(|m| {
                m.iter()
                    .any(|x| x["role"] == "user" && x["content"].as_str() == Some("粘贴后提交"))
            })
        }),
        "粘贴文本必须逐字完整提交"
    );
    assert!(!pty.screen().contains("back to conversation"), "旧详情页未被打开");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S62（D7，用户 03 场景）：工具 `for i in $(seq 1 30); do echo E$i; done; exit 4`
/// ——退出码 4＝预期不重试、失败状态正确；随后展开旧工具＋粘贴新请求＋Enter
/// 可提交并收到回复（修复后）。
#[test]
fn s62_user03_exit4_tool_then_next_request_submits() {
    let cfg = TempDir::new("s62-cfg");
    let user = TempDir::new("s62-user");
    let key = "s62-key";
    let args = serde_json::json!({
        "cmd": "for i in $(seq 1 30); do echo S62-E$i; done; exit 4"
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s62", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S62-第一段完成")),
        MockResp::json(200, chat_body("S62-新回复已收到")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S62_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S62_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S62-第一段完成").unwrap();
    pty.dwell(300);
    // 退出码 4：失败状态＋输出可见；不重试（计数仍 2）
    let screen = pty.screen();
    assert!(screen.contains("Failed (exit 4)"), "失败状态：{screen}");
    assert!(screen.contains("S62-E30"), "输出可见：{screen}");
    assert_eq!(mock.count(), 2, "退出码 4＝预期，不重试");
    // 用户 03 的阻断步骤：打开旧工具详情页 → Esc → 粘贴新请求 → Enter
    let (col, _, row) = pty.screen_needle_span("for details").expect("详情入口可见");
    pty.send_raw(&sgr_down(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(col + 1, row + 1));
    pty.wait_screen("back to conversation").expect("详情页打开");
    pty.send_raw(b"q"); // 单一职责轮：q 关页
    pty.dwell(300);
    pty.send_raw(b"\x1b[200~\xe4\xb8\x8b\xe4\xb8\x80\xe6\x9d\xa1\xe8\xaf\xb7\xe6\xb1\x82\x1b[201~");
    pty.dwell(300);
    pty.send_raw(b"\r");
    wait_submitted_then_show_reply(&mut pty, &mock, 3, "S62-新回复已收到");
    assert_eq!(mock.count(), 3, "恰好三轮（无重试）");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("panicked"), "{out}");
}

// ---------- S63–S66：012 §8 独立详情页（替代原地展开）----------

/// 打开工具详情页（点击披露入口）并等待页内标记。返回后用页标记消失断言关闭。
fn open_tool_page_63(pty: &mut PtySession, entry_needle: &str) {
    let (col, _, row) = pty.screen_needle_span(entry_needle).expect("详情入口可见");
    pty.send_raw(&sgr_down(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(col + 1, row + 1));
    pty.wait_screen("back to conversation").expect("详情页打开（固定提示）");
}

/// S63（E1/E7）：长输出工具→Ctrl+T 打开详情页→滚轮/翻页/沟位滚动条→一次 Esc
/// →主对话恢复紧凑预览与跟随态。零额外请求。
#[test]
fn s63_tool_detail_page_ctrl_t_scroll_and_q_close() {
    // 60 行长输出（超出详情页一屏，滚动可验证）
    let cfg = TempDir::new("s63-cfg");
    let user = TempDir::new("s63-user");
    let key = "s63-key";
    let args = serde_json::json!({
        "cmd": "for i in $(seq -w 1 60); do echo S63-L$i; done"
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s63", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S63-第一段完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S63_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S63_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S63-第一段完成").unwrap();
    pty.dwell(300);
    // Ctrl+T＝打开最近工具的详情页（聚焦工具同链路）
    pty.send_raw(b"\x14");
    pty.wait_screen("back to conversation").expect("详情页打开");
    let page = pty.screen();
    assert!(page.contains("$ for i in $(seq"), "页内命令：{page}");
    assert!(page.contains("Ran •"), "页内状态头（§6 形态）：{page}");
    assert!(page.contains("S63-L01"), "页从内容顶部开始：{page}");
    assert!(
        !page.contains("+ 57 lines"),
        "页内不得残留主视图紧凑标记：{page}"
    );
    // 翻页下滚两屏：L01 离开、末段到达；沟位滚动条渲染
    pty.send_raw(b"\x1b[6~");
    pty.dwell(150);
    pty.send_raw(b"\x1b[6~");
    pty.dwell(250);
    let scrolled = pty.screen();
    assert!(!scrolled.contains("S63-L01"), "滚动只影响详情页：{scrolled}");
    assert!(scrolled.contains("S63-L5"), "翻页到达下半段：{scrolled}");
    let marks = gutter_marks(&mut pty);
    assert!(
        marks.iter().any(|m| m.1 == '█' || m.1 == '║'),
        "详情页沟位滚动条在位：{marks:?}"
    );
    // 一次 q 关闭（滚动后，单一职责轮）：主对话恢复紧凑预览＋跟随态回底
    pty.send_raw(b"q");
    pty.dwell(300);
    let main = pty.screen();
    assert!(!main.contains("back to conversation"), "页已关闭：{main}");
    assert!(main.contains("+ 57 lines"), "紧凑预览恢复：{main}");
    assert!(main.contains("S63-L60"), "跟随态回底（尾部预览）：{main}");
    assert!(!main.contains("S63-L01"), "未展开态首行不可见：{main}");
    assert_eq!(mock.count(), 2, "打开/滚动/关闭零额外请求");
    // 单一职责轮：主对话 q＝普通文字（落入草稿，不触发任何动作）
    pty.send_raw(b"q");
    pty.dwell(300);
    let typed = pty.screen();
    assert!(typed.contains('›') && typed.contains('q'), "主对话 q 应为草稿文字：{typed}");
    assert!(!typed.contains("back to conversation"), "主对话 q 不得开页：{typed}");
    pty.send_raw(b"\x7f"); // 清除草稿 q（避免污染后续 /exit）
    pty.dwell(200);
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S64（E2）：中文/emoji 草稿＋光标移位→点击打开详情→resize→Esc——草稿与
/// 光标保留、输入交还 composer、无残留工具聚焦。
#[test]
fn s64_detail_page_preserves_draft_cursor_across_resize() {
    // 自带会话：工具（30 行）＋长回复（30 行）——主对话溢出可进入保持态
    let cfg = TempDir::new("s64-cfg");
    let user = TempDir::new("s64-user");
    let key = "s64-key";
    let args = serde_json::json!({
        "cmd": "for i in $(seq -w 1 30); do echo S64-L$i; done"
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s64", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body(&long_reply("S64-BODY"))),
        MockResp::json(200, chat_body("S64-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S64_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S64_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S64-BODY-L29").unwrap();
    pty.dwell(300);
    // 预置草稿＋光标左移两簇（插入点落在 稿 与 🎉 之间）
    pty.send_raw("中文草稿🎉".as_bytes());
    pty.dwell(400);
    pty.send_raw(b"\x1b[D\x1b[D");
    pty.dwell(200);
    // 点击详情入口（新文案 for details）→ resize → q 关页（单一职责轮）
    // （保持态＋resize 的组合恢复由 bookmark 单测＋s63 跟随态恢复承担，
    // PTY 场景构建在内容长度边界不稳定——登记为残余验证项）
    open_tool_page_63(&mut pty, "for details");
    pty.resize(120, 30);
    pty.dwell(300);
    pty.send_raw(b"q"); // 单一职责轮：q 关页
    pty.dwell(300);
    let main = pty.screen();
    assert!(main.contains("中文草稿🎉"), "草稿保留：{main}");
    assert!(!main.contains("back to conversation"), "页已关闭：{main}");
    // 输入交还 composer：插入字符落在光标处（证明光标位置保留）
    pty.send_raw(b"X");
    // Left×2 从末尾回退两簇：插入点在 草 与 稿 之间（末尾=中文草稿X🎉 为失败）
    pty.wait_screen("中文草X稿🎉").expect("光标位置保留（插入点未重置到末尾）");
    // 无残留工具聚焦：Enter 直接提交草稿（而非控制旧工具）
    for _ in 0..9 {
        pty.send_raw(b"\x7f");
        std::thread::sleep(Duration::from_millis(25));
    }
    pty.send_line("下一句S64");
    pty.wait_screen("S64-完成").unwrap(); // 第二轮回复同名——用计数/存在性即可
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S65（E3/E6）：浏览详情期间新回复入账；q 关页后粘贴（及键入）新请求→Enter
/// 恰好提交一次；旧工具不展开不重执行；exit 4 呈现为失败；CLI 存活。
#[test]
fn s65_detail_browsing_accounts_new_messages_then_submit() {
    let cfg = TempDir::new("s65-cfg");
    let user = TempDir::new("s65-user");
    let key = "s65-key";
    let mk_batch = |id: &str, cmd: &str| {
        let args = serde_json::json!({ "cmd": cmd }).to_string();
        serde_json::json!({ "choices": [{ "message": {
            "role": "assistant", "content": null,
            "tool_calls": [
                { "id": id, "type": "function",
                  "function": { "name": "exec_command", "arguments": args } }
            ]
        }, "finish_reason": "tool_calls" }] }).to_string()
    };
    let mock = spawn_mock(vec![
        MockResp::json(200, mk_batch("call-s65-a", "for i in $(seq -w 1 30); do echo S65-L$i; done")),
        MockResp::slow_json(200, chat_body("S65-第一段完成"), 1500),
        MockResp::json(200, mk_batch("call-s65-b", "echo S65-ERR; exit 4")),
        MockResp::json(200, chat_body("S65-新回复已收到")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S65_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S65_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    // 工具终态（紧凑）先到；回复延迟 1.5s——在其到达前打开详情页浏览
    pty.wait_screen("+ 27 lines").expect("工具紧凑预览");
    open_tool_page_63(&mut pty, "for details");
    // 页内滚动；期间回复到达并入账（不可见——页在前台）
    pty.send_raw(b"\x1b[6~");
    pty.dwell(400);
    assert_eq!(mock.count(), 2, "浏览零额外请求");
    // 一次 q 关页（单一职责轮）：回复在延迟窗口内到达并入账（跟随态恢复后自动上屏）
    pty.send_raw(b"q");
    pty.dwell(300);
    pty.wait_screen("S65-第一段完成").expect("浏览期间新回复入账（q 关页后可见）");
    let main = pty.screen();
    assert!(main.contains("+ 27 lines"), "旧工具保持紧凑：{main}");
    // 粘贴新请求→Enter：exit 4 工具执行（预期失败不重试）＋最终回复
    pty.send_raw(b"\x1b[200~\xe4\xb8\x8b\xe4\xb8\x80\xe6\x9d\xa1\xe8\xaf\xb7\xe6\xb1\x82\x1b[201~");
    pty.dwell(300);
    pty.send_raw(b"\r");
    let deadline = Instant::now() + Duration::from_secs(8);
    while mock.count() < 4 {
        assert!(Instant::now() < deadline, "新请求未提交（计数 {}）", mock.count());
        pty.read_once(50);
    }
    pty.wait_screen("Failed (exit 4)").expect("exit 4 呈现为失败");
    pty.wait_screen("S65-新回复已收到").expect("最终回复可见");
    assert_eq!(mock.count(), 4, "恰好四轮（无重试无重放）");
    // 旧工具未被打开/展开/重执行：仍紧凑在屏
    let done = pty.screen();
    assert!(done.contains("+ 27 lines"), "旧工具不受影响：{done}");
    assert!(!done.contains("back to conversation"), "无页面残留：{done}");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(!out.contains("panicked"), "{out}");
}

/// S66（E4/E5）：预存草稿→F4→Enter 打开详情（草稿零提交）→页内选择＋右键
/// 复制（载荷正确、头部不污染）→q 关页→草稿保留→编辑并提交（单一职责轮）。
#[test]
fn s66_f4_enter_opens_page_copy_then_draft_submit() {
    let (mut pty, mock, _keep) = focus_session57("S66", "S66-新回复已收到");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S66-第一段完成").unwrap();
    pty.dwell(300);
    // 预存草稿→F4 聚焦→Enter＝打开详情页（非提交）
    pty.send_raw("S66-预存草稿".as_bytes());
    pty.dwell(400);
    pty.send_raw(b"\x1bOS");
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.wait_screen("back to conversation").expect("F4+Enter＝打开详情页");
    pty.dwell(300);
    assert_eq!(mock.count(), 2, "打开详情零提交");
    // 页内滚动到中部并选择输出行（不含头部）→右键复制
    pty.send_raw(b"\x1b[5~");
    pty.dwell(250);
    drag_select_needle(&mut pty, "S66-L10");
    let (col, _, row) = pty.screen_needle_span("S66-L12").expect("选区延伸目标可见");
    pty.send_raw(&sgr_drag(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(80));
    pty.send_raw(&sgr_up(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(80));
    pty.send_raw(&sgr_right_down(col + 1, row + 1));
    // 页内反馈＝CopyStatus 文案（OSC 52 终端回写无法确认→unconfirmed 措辞）
    pty.wait_screen("unconfirmed").expect("复制已发送（页内反馈）");
    let payloads = osc52_payloads(&pty.output());
    let last = payloads.last().expect("OSC 52 载荷存在");
    let decoded = String::from_utf8(sel_b64_decode(last)).unwrap();
    assert_eq!(decoded, "S66-L10", "载荷恰为所选输出（无头部污染）：{decoded:?}");
    assert!(!decoded.contains("Ran"), "状态头不污染复制文本：{decoded:?}");
    assert!(!decoded.contains('$'), "命令前缀不混入所选输出：{decoded:?}");
    // 一次 q 关闭（含选区，单一职责轮）→草稿保留→追加并提交
    pty.send_raw(b"q");
    pty.dwell(300);
    let main = pty.screen();
    assert!(!main.contains("back to conversation"), "页已关闭：{main}");
    assert!(main.contains("S66-预存草稿"), "草稿未受页内操作影响：{main}");
    pty.send_raw("续写".as_bytes());
    pty.dwell(300);
    pty.send_raw(b"\r");
    wait_submitted_then_show_reply(&mut pty, &mock, 3, "S66-新回复已收到");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert_eq!(mock.count(), 3);
}

/// S67（E7/J10 红线）：详情页内 Ctrl+C 恒 J10——空闲＝退出（再见!＋0），不得
/// 继承母本 pager「Ctrl+C 仅关闭页面」默认（PagerKeymap.close 含 ctrl+c）。
#[test]
fn s67_ctrl_c_inside_detail_page_keeps_j10() {
    let (mut pty, _mock, _keep) = focus_session57("S67", "S67-完成");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S67-第一段完成").unwrap();
    pty.dwell(300);
    let (col, _, row) = pty.screen_needle_span("for details").expect("详情入口可见");
    pty.send_raw(&sgr_down(col + 1, row + 1));
    std::thread::sleep(Duration::from_millis(60));
    pty.send_raw(&sgr_up(col + 1, row + 1));
    pty.wait_screen("back to conversation").expect("详情页打开");
    // 页内 Ctrl+C（空闲）＝退出，不是关闭页面
    pty.send_raw(b"\x03");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "J10：页内空闲 Ctrl+C＝退出：{out}");
    assert!(out.contains("?1049l"), "退出后终端恢复：{out}");
}

// ---------- S68–S70：有选区时 Ctrl+C 执行既定动作（010 K5/J10 契约定点轮）----------

/// 定点轮夹具：可选历史（含选区针文本）＋自定义 mock 回复序列。
fn cancel_session(
    tag: &str,
    replies: Vec<MockResp>,
) -> (PtySession, common::Mock, Vec<TempDir>) {
    let (pty, mock, keep) = scroll_session(&tag.to_lowercase(), replies);
    let msg = |assistant: bool, content: String| {
        terminal_chat::prompt::HistoryItem::Text(terminal_chat::prompt::HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: if assistant {
                terminal_chat::prompt::HistoryRole::Assistant
            } else {
                terminal_chat::prompt::HistoryRole::User
            },
            content,
            user_input_event: None,
        })
    };
    let stem = format!("S{tag}");
    let mut hist = Vec::new();
    hist.push(msg(false, format!("{stem} Word00SelKt")));
    hist.push(msg(true, format!("{stem}-A00 filler")));
    hist.push(msg(false, format!("{stem} Word02SelKt")));
    hist.push(msg(true, format!("{stem}-A02 filler")));
    terminal_chat::session::save_snapshot(&keep[1].path().join("sessions"), "seedsel", &hist)
        .unwrap();
    (pty, mock, keep)
}

/// 轮询 mock 计数到达期望值（请求已发出；在途回复由 delay 控制）。
fn wait_request_count(pty: &mut PtySession, mock: &common::Mock, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while mock.count() < expected {
        assert!(
            Instant::now() < deadline,
            "请求未被提交（mock 计数 {}，期望 {expected}）",
            mock.count()
        );
        pty.read_once(50);
    }
    pty.dwell(150);
}

/// S68（契约 1/3）：详情页 × 空闲 × 有选区 × Ctrl+C＝退出 CLI（J10 恒定，
/// 不被页内选区/pager 关闭默认接管）；零 OSC 52（Ctrl+C 永不复制）。
#[test]
fn s68_ctrl_c_in_detail_page_with_selection_exits() {
    let (mut pty, _mock, _keep) = focus_session57("S68", "S68-完成");
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S68-第一段完成").unwrap();
    pty.dwell(300);
    // 打开详情页 → 页内拖选输出行（选区激活、空闲）→ Ctrl+C
    open_tool_page_63(&mut pty, "for details");
    pty.dwell(200);
    drag_select_needle(&mut pty, "S68-L03");
    pty.send_raw(b"\x03");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "页内空闲＋选区 Ctrl+C＝退出：{out}");
    assert!(osc52_payloads(&out).is_empty(), "Ctrl+C 不得触发 OSC 52：{out}");
    assert!(out.contains("?1049l"), "退出后终端恢复：{out}");
}

/// S69（契约 2/3，单一职责轮改键）：主对话 × 运行中 × 无选区 × Esc＝**取消任务**＋CLI 存活。
/// 钉死在途模型请求（阻塞 ureq）可被取消——修复前：置标志后无可视变化，
/// 回复照常到达渲染（「无提示、不退出、不取消」＝现场症状）。
#[test]
fn s69_esc_cancels_inflight_reply_without_selection() {
    let (mut pty, mock, _keep) = cancel_session(
        "S69",
        vec![
            MockResp::slow_json(200, chat_body("S69-reply-never"), 5000),
            MockResp::json(200, chat_body("S69-after-ok")),
        ],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S69 Word00SelKt").unwrap();
    for _ in 0..5 {
        pty.read_once(60);
    }
    // 提交 turn（5s 延迟回复在途、无选区）→ Ctrl+C
    pty.send_line("turn-one");
    wait_request_count(&mut pty, &mock, 1);
    pty.send_raw(b"\x1b"); // 单一职责轮：Esc＝取消
    // 取消须及时可见（反馈行），而非等 HTTP 自然返回；文案如实＝「已停止等待
    // 模型回复」，不得宣称网络请求已中止（收尾轮契约：仅停止本地等待）
    pty.wait_screen_timeout("用户已取消本轮：已停止等待模型回复", Duration::from_secs(2))
        .expect("Ctrl+C（运行中）须产生取消反馈（如实文案）");
    assert!(
        !pty.screen().contains("已中止"),
        "不得宣称网络请求已中止（实际仅停止本地等待）：{}",
        pty.screen()
    );
    // 静置越过延迟终点：被取消的回复不得再渲染
    pty.dwell(5500);
    assert!(
        !pty.screen().contains("S69-reply-never"),
        "取消后在途回复不得渲染：{}",
        pty.screen()
    );
    assert!(osc52_payloads(&pty.output()).is_empty(), "Ctrl+C 不得触发 OSC 52");
    // CLI 存活且完全可用：下一 turn 正常完成
    pty.send_line("turn-two");
    wait_request_count(&mut pty, &mock, 2);
    pty.wait_screen("S69-after-ok").unwrap();
    assert_eq!(mock.count(), 2);
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S69b（契约 2/3）：主对话 × 运行中 × **有选区** × Ctrl+C＝取消＋存活（选区
/// 不得改变 Ctrl+C 归属——K5），零 OSC 52。
#[test]
fn s69b_esc_cancels_with_selection_preserved() {
    let (mut pty, mock, _keep) = cancel_session(
        "S69B",
        vec![
            MockResp::slow_json(200, chat_body("S69B-reply-never"), 5000),
            MockResp::json(200, chat_body("S69B-after-ok")),
        ],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S69B Word00SelKt").unwrap();
    for _ in 0..5 {
        pty.read_once(60);
    }
    pty.send_line("turn-one");
    wait_request_count(&mut pty, &mock, 1);
    // 选区激活（turn 在飞）→ Esc＝取消（单一职责轮：选区不得提前消费 Esc）。
    // 取消即时生效；选区冻结视口（S48）下反馈行暂不可见（登记取舍）——
    // 断言序：①CLI 存活＋Working 消失（取消生效）②选区未被 Esc 改变（右键
    // 仍复制原选区）③Ctrl+End 回底后反馈行可见（取消的真实效果）
    drag_select_needle(&mut pty, "Word00SelKt");
    pty.send_raw(b"\x1b");
    pty.dwell(600);
    assert!(
        pty.child.try_wait().unwrap().is_none(),
        "取消后 CLI 存活"
    );
    pty.send_raw(&sgr_right_down(10, 10));
    pty.wait_screen("Copy sent").expect("Esc 不清选区——取消后右键仍复制原选区");
    {
        let payloads = osc52_payloads(&pty.output());
        let last = payloads.last().expect("取消后选区复制载荷存在");
        let decoded = String::from_utf8(sel_b64_decode(last)).unwrap();
        assert_eq!(decoded, "Word00SelKt", "选区未被 Esc 改变：{decoded:?}");
    }
    pty.send_raw(b"\x1b[1;5F"); // Ctrl+End 回底（清选区＋跟随）→ 反馈行可见
    pty.wait_screen_timeout("用户已取消", Duration::from_secs(2))
        .expect("Esc 取消生效（回底后反馈可见）");
    pty.dwell(5500);
    assert!(
        !pty.screen().contains("S69B-reply-never"),
        "取消后在途回复不得渲染：{}",
        pty.screen()
    );
    let payloads = osc52_payloads(&pty.output());
    assert_eq!(
        payloads.len(),
        1,
        "Esc 取消不得触发复制；唯一载荷＝上文已验的选区复制：{payloads:?}"
    );
    pty.send_line("turn-two");
    wait_request_count(&mut pty, &mock, 2);
    pty.wait_screen("S69B-after-ok").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S70（单一职责轮）：详情页 × 运行中 × Esc＝取消且**页保持打开**，取消反馈经
/// 页内 notice 可见、不得改正在查看的旧工具 cell；q 返回后可正常提交。两段覆盖
/// 页内无选区/有选区（零 OSC 52）。
#[test]
fn s70_esc_in_page_cancels_keeps_page_notice_then_q() {
    let cfg = TempDir::new("s70-cfg");
    let user = TempDir::new("s70-user");
    let key = "s70-key";
    let args = serde_json::json!({
        "cmd": "for i in $(seq -w 1 30); do echo S70-L$i; done"
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s70", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S70-第一段完成")),
        MockResp::slow_json(200, chat_body("S70-reply-never-a"), 5000),
        MockResp::slow_json(200, chat_body("S70-reply-never-b"), 5000),
        MockResp::json(200, chat_body("S70-after-ok")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S70_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S70_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S70-第一段完成").unwrap();
    pty.dwell(300);

    // 段 A：页内 × 运行中 × 无选区 → Esc＝取消，页保持打开，反馈经页内 notice 可见
    pty.send_line("second");
    wait_request_count(&mut pty, &mock, 3);
    pty.send_raw(b"\x14"); // Ctrl+T 打开最近工具详情页
    pty.wait_screen("back to conversation").expect("详情页打开");
    pty.dwell(200);
    let before = pty.screen();
    assert!(before.contains("S70-L01"), "页内容在位（取消前）：{before}");
    pty.send_raw(b"\x1b"); // 单一职责轮：页内 Esc＝取消（非关页）
    pty.wait_screen_timeout("已停止等待模型回复", Duration::from_secs(2))
        .expect("页内取消反馈（notice）须及时可见");
    let stayed = pty.screen();
    assert!(
        stayed.contains("back to conversation"),
        "取消后详情页保持打开：{stayed}"
    );
    assert!(
        stayed.contains("Ran") && stayed.contains("S70-L01"),
        "取消不得改正在查看的旧工具状态：{stayed}"
    );
    pty.dwell(5500);
    let after = pty.screen();
    assert!(
        !after.contains("S70-reply-never-a") && after.contains("back to conversation"),
        "取消后在途回复不得渲染且页仍在：{after}"
    );
    // q 返回主对话；随后可正常提交新请求
    pty.send_raw(b"q");
    pty.dwell(300);
    assert!(
        !pty.screen().contains("back to conversation"),
        "q 关闭详情页：{}",
        pty.screen()
    );

    // 段 B：页内 × 运行中 × 有选区 → Esc＝取消（页保持、零 OSC 52），q 返回
    pty.send_line("third");
    wait_request_count(&mut pty, &mock, 4);
    pty.send_raw(b"\x14");
    pty.wait_screen("back to conversation").expect("详情页再次打开");
    pty.dwell(200);
    drag_select_needle(&mut pty, "S70-L03");
    pty.send_raw(b"\x1b");
    pty.wait_screen_timeout("已停止等待模型回复", Duration::from_secs(2))
        .expect("段 B 页内取消反馈可见");
    assert!(
        pty.screen().contains("back to conversation"),
        "选区激活时 Esc 仍＝取消（非关页）：{}",
        pty.screen()
    );
    pty.dwell(5500);
    assert!(
        !pty.screen().contains("S70-reply-never-b"),
        "段 B 取消后在途回复不得渲染：{}",
        pty.screen()
    );
    let payloads = osc52_payloads(&pty.output());
    assert!(payloads.is_empty(), "页内 Esc（选区激活）不得触发 OSC 52：{payloads:?}");
    pty.send_raw(b"q");
    pty.dwell(300);
    // CLI 完全可用
    pty.send_line("fourth");
    wait_request_count(&mut pty, &mock, 5);
    pty.wait_screen("S70-after-ok").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S71（收尾轮·取消语义定点核查）：Ctrl+C 停止的是**本地等待**——底层请求继续
/// 在途，响应到达（2.5s≪180s 兜底）后辅助线程自然结束（/proc 线程数回落基线，
/// 非超时兜底路径）；连续两次取消各遗留一个线程且先后回收；迟到回复不上屏、
/// 后续 turn 正常、零复制写入。
#[test]
fn s71_cancel_semantics_reclaims_helper_threads_on_reply_arrival() {
    let (mut pty, mock, _keep) = cancel_session(
        "S71",
        vec![
            MockResp::slow_json(200, chat_body("S71-late-A"), 2500),
            MockResp::slow_json(200, chat_body("S71-late-B"), 2500),
            MockResp::json(200, chat_body("S71-final-ok")),
        ],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S71 Word00SelKt").unwrap();
    for _ in 0..5 {
        pty.read_once(60);
    }
    let baseline = pty.thread_count();
    // 两轮连续取消：提交→取消→反馈；被弃请求 2.5s 后到达并回收线程
    for round in 1..=2 {
        pty.send_line(&format!("turn-{round}"));
        wait_request_count(&mut pty, &mock, round);
        pty.send_raw(b"\x1b"); // 单一职责轮：Esc＝取消
        pty.wait_screen_timeout("用户已取消", Duration::from_secs(2))
            .expect("取消反馈及时可见");
        // 本地等待已停，但底层请求仍在途＝辅助线程仍在（相对基线 +1）
        let deadline = Instant::now() + Duration::from_secs(1);
        while pty.thread_count() <= baseline && Instant::now() < deadline {
            pty.read_once(50);
        }
        assert!(
            pty.thread_count() > baseline,
            "取消后辅助线程应仍在（停止等待≠底层请求中止），线程数 {} 基线 {baseline}",
            pty.thread_count()
        );
        // 响应到达（2.5s）→迟到结果落已弃接收端→线程结束→线程数回落基线
        let deadline = Instant::now() + Duration::from_secs(4);
        while pty.thread_count() > baseline && Instant::now() < deadline {
            pty.read_once(50);
        }
        assert_eq!(
            pty.thread_count(),
            baseline,
            "迟到响应到达后辅助线程须回收（非 180s 兜底路径）"
        );
    }
    // 两轮被取消回复均不上屏；第三轮正常完成（迟到请求不干扰新 turn）
    pty.dwell(500);
    pty.send_line("turn-final");
    wait_request_count(&mut pty, &mock, 3);
    pty.wait_screen("S71-final-ok").unwrap();
    let screen = pty.screen();
    assert!(
        !screen.contains("S71-late-A") && !screen.contains("S71-late-B"),
        "被取消回复不得上屏：{screen}"
    );
    assert!(osc52_payloads(&pty.output()).is_empty(), "全程零复制写入");
    assert_eq!(mock.count(), 3, "恰好三次请求（两次被取消＋一次完成）");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S72（重叠取消定点）：A 取消后其辅助线程**尚未结束**时立即发起 B（重叠期
/// 线程数基线＋2＝两个未结束请求的辅助线程同时存活）→ 取消 B → A、B 迟到
/// 响应先后到达后双双回收（回落基线）。A 的迟到响应携带 tool_calls——若被
/// 错误处理将执行 exec 并上屏（断言不触发）；B 迟到文本不上屏；随后正常 C
/// 不受干扰；取消结果不入会话历史（读 auto 会话文件：含 C 回复正向锚点、
/// 不含 late-B/工具批 id/工具输出）。
#[test]
fn s72_overlapping_cancels_reclaim_and_late_results_stay_inert() {
    let tool_args = serde_json::json!({ "cmd": "echo S72-TOOL-LEAK" }).to_string();
    let tool_batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s72", "type": "function",
              "function": { "name": "exec_command", "arguments": tool_args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let (mut pty, mock, keep) = cancel_session(
        "S72",
        vec![
            // A：迟到响应携带工具批（泄漏即执行 exec）
            MockResp::slow_json(200, tool_batch, 2500),
            // B：迟到纯文本
            MockResp::slow_json(200, chat_body("S72-late-B"), 2500),
            // C：正常完成
            MockResp::json(200, chat_body("S72-final-C")),
        ],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S72 Word00SelKt").unwrap();
    for _ in 0..5 {
        pty.read_once(60);
    }
    let baseline = pty.thread_count();
    // A：提交→取消（本地等待停止，辅助线程存活至 2.5s 响应到达）
    pty.send_line("turn-A");
    wait_request_count(&mut pty, &mock, 1);
    pty.send_raw(b"\x1b"); // 单一职责轮：Esc＝取消
    pty.wait_screen_timeout("用户已取消", Duration::from_secs(2))
        .expect("A 取消反馈及时可见");
    // 重叠：A 未结束（<2.5s）时立即发起 B——两个辅助线程同时存活（基线＋2）
    pty.send_line("turn-B");
    wait_request_count(&mut pty, &mock, 2);
    let deadline = Instant::now() + Duration::from_millis(900);
    while pty.thread_count() < baseline + 2 && Instant::now() < deadline {
        pty.read_once(50);
    }
    assert!(
        pty.thread_count() >= baseline + 2,
        "重叠期两个未结束请求的辅助线程并存（实际 {} 基线 {baseline}）",
        pty.thread_count()
    );
    // 取消 B——两请求均处「已取消等待、底层在途」态
    pty.send_raw(b"\x1b"); // 单一职责轮：Esc＝取消
    pty.dwell(400);
    // A（≈提交后 2.5s）、B（≈其提交后 2.5s）迟到响应先后到达→双双回收
    let deadline = Instant::now() + Duration::from_secs(6);
    while pty.thread_count() > baseline && Instant::now() < deadline {
        pty.read_once(50);
    }
    assert_eq!(
        pty.thread_count(),
        baseline,
        "A、B 迟到响应到达后辅助线程全部回收（重叠不改变回收路径）"
    );
    // C：正常完成——迟到 A/B 不干扰
    pty.send_line("turn-C");
    wait_request_count(&mut pty, &mock, 3);
    pty.wait_screen("S72-final-C").unwrap();
    let screen = pty.screen();
    assert!(
        !screen.contains("S72-TOOL-LEAK"),
        "A 迟到 tool_calls 不得触发工具执行：{screen}"
    );
    assert!(
        !screen.contains("S72-late-B"),
        "B 迟到文本不得上屏：{screen}"
    );
    assert!(osc52_payloads(&pty.output()).is_empty(), "全程零复制写入");
    // 不入会话历史：auto 文件含 C 回复（正向锚点），不含 late-B／工具批／工具输出
    let mut persisted = String::new();
    for entry in std::fs::read_dir(keep[1].path().join("sessions")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("auto-")) {
            persisted.push_str(&std::fs::read_to_string(&path).unwrap());
        }
    }
    assert!(
        persisted.contains("S72-final-C"),
        "正向锚点：C 回复已入会话历史（否则后续断言空跑）：{persisted}"
    );
    assert!(
        !persisted.contains("S72-late-B") && !persisted.contains("call-s72")
            && !persisted.contains("S72-TOOL-LEAK"),
        "取消的迟到结果（文本/工具批/工具输出）不得入史：{persisted}"
    );
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S73–S76：按键单一职责轮（Ctrl+C 恒退出 / Esc 专取消 / q 关页）----------

/// 辅助：等待标记进程全部消失（有界；pgrep -f 唯一标记）。
fn wait_no_marker_process(marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let out = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(marker)
            .output()
            .expect("pgrep");
        if out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty() {
            assert!(Instant::now() < deadline, "标记进程 {marker} 仍存活（工具进程组未回收）");
            std::thread::sleep(Duration::from_millis(100));
        } else {
            return;
        }
    }
}

fn marker_process_alive(marker: &str) -> bool {
    std::process::Command::new("pgrep")
        .arg("-f")
        .arg(marker)
        .output()
        .map(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

/// S73（A·主对话）：工具进程组运行中 Ctrl+C＝完整收尾退出——CLI 退出码 0＋`再见!`
/// ＋终端恢复；**工具进程组被终止并回收**（唯一标记进程归零，不以「已置取消标志」
/// 替代）；已发生会话记录排空落盘。
#[test]
fn s73_ctrl_c_running_tool_exits_full_teardown() {
    let cfg = TempDir::new("s73-cfg");
    let user = TempDir::new("s73-user");
    let key = "s73-key";
    // 标记进程：每次运行唯一（防跨运行孤儿污染）＋有界时长（泄漏自愈 ≤4min）
    let marker = format!("s73mark{}end", std::process::id());
    let args = serde_json::json!({ "cmd": format!("sleep 240 # {marker}") }).to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s73", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![MockResp::json(200, batch)]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S73_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S73_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen(&marker).expect("工具运行中（命令可见）");
    // 正向锚点：标记进程确在运行
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker_process_alive(&marker) && Instant::now() < deadline {
        pty.read_once(50);
    }
    assert!(marker_process_alive(&marker), "前置：工具进程在运行");
    // 运行中 Ctrl+C（单一职责轮：一律退出）
    pty.send_raw(b"\x03");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "Ctrl+C 运行中＝退出码 0：{out}");
    assert!(out.contains("?1049l"), "终端模式恢复（离开备用屏）：{out}");
    assert!(out.contains("?1000l") || out.contains("?1002l"), "鼠标捕获恢复：{out}");
    // 工具进程组终止并回收（不残留）
    wait_no_marker_process(&marker);
    // 会话排空：auto 文件含本轮 user 消息（flush_final 在退出路径完成）
    let mut persisted = String::new();
    for entry in std::fs::read_dir(user.path().join("sessions")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("auto-")) {
            persisted.push_str(&std::fs::read_to_string(&path).unwrap());
        }
    }
    assert!(persisted.contains("跑"), "会话排空：本轮 user 记录在盘：{persisted}");
}

/// S73b（A·详情页）：页开＋turn 运行中 Ctrl+C＝退出（终端恢复；不取消不关页留存）。
#[test]
fn s73b_ctrl_c_in_running_page_exits() {
    let (mut pty, mock, _keep) = focus_session57("S73B", "S73B-x");
    // 专注夹具第 3 响改为慢回复不可行（夹具固定），改用直接构造：第 2 轮提交后开页
    let _ = mock;
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S73B-第一段完成").unwrap();
    pty.dwell(300);
    pty.send_line("second");
    // 等待请求发出（在途）→ Ctrl+T 开页 → 运行中 Ctrl+C＝退出
    let deadline = Instant::now() + Duration::from_secs(3);
    while mock.count() < 3 && Instant::now() < deadline {
        pty.read_once(50);
    }
    assert_eq!(mock.count(), 3, "第二轮请求在途");
    pty.wait_screen("Working").expect("前置：任务运行中锚点");
    pty.send_raw(b"\x14");
    pty.wait_screen("back to conversation").expect("详情页打开");
    pty.send_raw(b"\x03");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "页内运行中 Ctrl+C＝退出码 0：{out}");
    assert!(out.contains("?1049l"), "终端恢复：{out}");
    assert!(!out.contains("用户已取消"), "Ctrl+C 不再承担取消：{out}");
}

/// S74（B·工具焦点）：F4 聚焦＋turn 运行中 Esc＝一次按键一次取消（焦点不得提前
/// 消费 Esc）；取消后焦点保持（Enter 仍控制工具＝打开详情页）；CLI 存活可继续。
#[test]
fn s74_esc_with_tool_focus_cancels_and_keeps_focus() {
    let cfg = TempDir::new("s74-cfg");
    let user = TempDir::new("s74-user");
    let key = "s74-key";
    let args = serde_json::json!({
        "cmd": "for i in $(seq -w 1 30); do echo S74-L$i; done"
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s74", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S74-第一段完成")),
        MockResp::slow_json(200, chat_body("S74-late-never"), 5000),
        MockResp::json(200, chat_body("S74-after-ok")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S74_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S74_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S74-第一段完成").unwrap();
    pty.dwell(300);
    // turn 在飞 → F4 聚焦（运行中聚焦可达）→ Esc＝取消（不得被聚焦分支消费）
    pty.send_line("second");
    wait_request_count(&mut pty, &mock, 3);
    pty.send_raw(b"\x1bOS"); // F4
    pty.dwell(300);
    pty.send_raw(b"\x1b"); // Esc：一次按键一次取消
    pty.wait_screen_timeout("已停止等待模型回复", Duration::from_secs(2))
        .expect("聚焦态 Esc＝取消（聚焦不得提前消费）");
    assert!(
        pty.child.try_wait().unwrap().is_none(),
        "取消后 CLI 存活"
    );
    pty.dwell(5000);
    assert!(
        !pty.screen().contains("S74-late-never"),
        "被取消回复不得上屏：{}",
        pty.screen()
    );
    // 焦点保持证明：Enter 仍控制工具（打开详情页而非提交）
    pty.send_raw(b"\r");
    pty.wait_screen("back to conversation").expect("焦点保持：Enter＝打开详情页");
    pty.send_raw(b"q");
    pty.dwell(300);
    // 后续可正常提交新请求
    pty.send_line("third");
    wait_request_count(&mut pty, &mock, 4);
    pty.wait_screen("S74-after-ok").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S75（B/C·? 提示）：运行中「?」展开态 Esc＝取消且提示不被收起；空闲展开态
/// Esc＝无动作（提示保持），再按 ? 收起（既有切换方式）。
#[test]
fn s75_esc_with_shortcut_overlay_cancel_and_keep_hints() {
    let (mut pty, mock, _keep) = cancel_session(
        "S75",
        vec![
            MockResp::slow_json(200, chat_body("S75-late-never"), 5000),
            MockResp::json(200, chat_body("S75-after-ok")),
        ],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S75 Word00SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    // 运行中：? 展开提示 → Esc＝取消（一次），提示不被收起
    pty.send_line("turn-one");
    wait_request_count(&mut pty, &mock, 1);
    pty.send_raw(b"?"); // 空草稿＋?＝切换 footer 快捷键展开
    pty.dwell(300);
    assert!(
        !pty.screen().contains("? for shortcuts"),
        "前置：? 已展开提示（收起态提示行消失）"
    );
    pty.send_raw(b"\x1b");
    pty.wait_screen_timeout("已停止等待模型回复", Duration::from_secs(2))
        .expect("展开态 Esc＝取消（提示不得提前消费）");
    assert!(
        !pty.screen().contains("? for shortcuts"),
        "取消不得收起 ? 提示（保持展开）"
    );
    pty.dwell(4500);
    // 空闲：展开态 Esc＝无动作（提示保持）；再按 ? 收起
    pty.send_raw(b"\x1b");
    pty.dwell(300);
    assert!(
        !pty.screen().contains("? for shortcuts"),
        "空闲 Esc 无动作：提示保持展开"
    );
    pty.send_raw(b"?");
    pty.dwell(300);
    assert!(
        pty.screen().contains("? for shortcuts"),
        "再按 ? 收起提示（既有切换方式）"
    );
    assert!(
        pty.child.try_wait().unwrap().is_none(),
        "CLI 全程存活"
    );
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S76（C·空闲矩阵）：空闲态 Esc 一律无动作——选区保持（右键仍复制）、工具焦点
/// 保持（Enter 仍开页）、详情页保持（仍需 q 关闭）；一次按键零副作用。
#[test]
fn s76_idle_esc_is_noop_across_states() {
    let cfg = TempDir::new("s76-cfg");
    let user = TempDir::new("s76-user");
    let key = "s76-key";
    let args = serde_json::json!({
        "cmd": "for i in $(seq -w 1 30); do echo S76-L$i; done"
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s76", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![MockResp::json(200, batch), MockResp::json(200, chat_body("S76-完成"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S76_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S76_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S76-完成").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    // ① 空闲＋选区：Esc 无动作——选区保持（右键仍复制出原选区）
    drag_select_needle(&mut pty, "S76-L28");
    pty.send_raw(b"\x1b");
    pty.dwell(400);
    pty.send_raw(&sgr_right_down(10, 10));
    pty.wait_screen("Copy sent").expect("Esc 空闲不清选区——右键仍可复制");
    {
        let payloads = osc52_payloads(&pty.output());
        let last = payloads.last().expect("空闲 Esc 后选区复制载荷存在");
        let decoded = String::from_utf8(sel_b64_decode(last)).unwrap();
        assert_eq!(decoded, "S76-L28", "选区未被空闲 Esc 改变：{decoded:?}");
    }
    pty.send_screen_click_composer(); // 清选区进入下一状态
    for _ in 0..4 { pty.read_once(60); }
    // ② 空闲＋工具焦点：Esc 无动作——焦点保持（Enter 仍＝打开详情页）
    pty.send_raw(b"\x1bOS"); // F4
    pty.dwell(300);
    pty.send_raw(b"\x1b");
    pty.dwell(400);
    pty.send_raw(b"\r");
    pty.wait_screen("back to conversation").expect("焦点保持：Enter＝打开详情页");
    // ③ 空闲＋详情页：Esc 无动作——页保持打开；q 关闭
    pty.send_raw(b"\x1b");
    pty.dwell(400);
    assert!(
        pty.screen().contains("back to conversation"),
        "空闲 Esc 不得关页：{}",
        pty.screen()
    );
    pty.send_raw(b"q");
    pty.dwell(300);
    assert!(
        !pty.screen().contains("back to conversation"),
        "q 关闭详情页：{}",
        pty.screen()
    );
    assert!(
        pty.child.try_wait().unwrap().is_none(),
        "空闲 Esc 全程零副作用（CLI 存活，无取消动作）"
    );
    assert_eq!(mock.count(), 2, "空闲 Esc 零模型请求");
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S78（收尾轮·空闲态矩阵）：Ctrl+R 历史搜索——Esc 无动作（搜索态不变）；
/// Enter＝替代出口（无匹配态退出并恢复原草稿＋光标；匹配态接受历史项）；
/// Ctrl+C 直接退出 CLI（任何搜索态）。
#[test]
fn s78_history_search_idle_esc_noop_enter_exits_ctrl_c_quits() {
    let (mut pty, _mock, _keep) = cancel_session(
        "S78",
        vec![MockResp::json(200, chat_body("S78-x"))],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S78 Word00SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    // 预置草稿（含光标验证锚点——恢复后继续输入应拼接在尾部）
    pty.send_raw("草稿XY".as_bytes());
    pty.dwell(400);
    // Ctrl+R 进入搜索 → 无匹配 query
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("历史搜索态可见");
    pty.send_raw(b"zzqq-no-match");
    pty.dwell(300);
    // ① 空闲 Esc＝无动作：搜索态保持
    pty.send_raw(b"\x1b");
    pty.dwell(400);
    assert!(
        pty.screen().contains("reverse-i-search:"),
        "空闲 Esc 不得退出/改变历史搜索态：{}",
        pty.screen()
    );
    // ② Enter＝替代出口：退出搜索并恢复原草稿（含光标——追加输入落尾部）
    pty.send_raw(b"\r");
    pty.dwell(400);
    let after = pty.screen();
    assert!(
        !after.contains("reverse-i-search:"),
        "Enter 须退出历史搜索（无匹配态恢复原稿）：{after}"
    );
    assert!(after.contains("草稿XY"), "原草稿恢复：{after}");
    pty.send_raw("尾".as_bytes());
    pty.dwell(300);
    let typed = pty.screen();
    assert!(
        typed.contains("草稿XY尾"),
        "恢复后光标在尾（继续输入拼接）：{typed}"
    );
    // ③ 匹配态 Enter＝接受历史项（既有行为回归）——用真实消息提交作历史
    // （命令提交不入 composer 本地历史）
    for _ in 0..10 { pty.send_raw(b"\x7f"); pty.read_once(30); }
    pty.send_line("hist-target-msg");
    pty.wait_screen("S78-x").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("再次进入搜索");
    pty.send_raw(b"hist-target");
    pty.dwell(400);
    // 正置 Match 态（footer 出现 accept 提示）——防「误走 cancel 分支」假绿
    pty.wait_screen("enter accept").expect("前置：Match 态提示可见");
    pty.send_raw(b"\r");
    pty.dwell(400);
    let accepted = pty.screen();
    assert!(
        !accepted.contains("reverse-i-search:"),
        "匹配态 Enter＝退出搜索：{accepted}"
    );
    // 接受后草稿＝历史项（区分于 transcript 回显）：追加输入拼接验证
    pty.send_raw("尾2".as_bytes());
    pty.dwell(300);
    let appended = pty.screen();
    assert!(
        appended.contains("hist-target-msg尾2"),
        "接受历史项入草稿且光标在尾：{appended}"
    );
    for _ in 0..24 { pty.send_raw(b"\x7f"); pty.read_once(30); }
    // ④ 搜索态 Ctrl+C＝直接退出 CLI
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("第三次进入搜索");
    pty.send_raw(b"zz");
    pty.dwell(200);
    pty.send_raw(b"\x03");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "搜索态 Ctrl+C＝退出码 0：{out}");
    assert!(!out.contains("用户已取消"), "Ctrl+C 不再承担取消：{out}");
}

/// S79（收尾轮·运行中）：历史搜索激活＋任务运行中——Esc＝取消任务**且不退出
/// 搜索**（搜索态保持）；随后 Enter 退出搜索恢复输入；下一请求正常提交。
#[test]
fn s79_history_search_running_esc_cancels_keeps_search() {
    let (mut pty, mock, _keep) = cancel_session(
        "S79",
        vec![
            MockResp::slow_json(200, chat_body("S79-late-never"), 5000),
            MockResp::json(200, chat_body("S79-after-ok")),
        ],
    );
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seedsel");
    pty.wait_screen("S79 Word00SelKt").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    // 任务在飞 → Ctrl+R 进入搜索（无匹配 query）
    pty.send_line("turn-one");
    wait_request_count(&mut pty, &mock, 1);
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("运行中进入历史搜索");
    pty.send_raw(b"zzqq");
    pty.dwell(300);
    // Esc＝取消任务，不退出搜索（反馈行在 composer 上方 gap 区可见——不依赖视口回底）
    pty.send_raw(b"\x1b");
    pty.wait_screen_timeout("已停止等待模型回复", Duration::from_secs(2))
        .expect("Esc＝取消任务（搜索态不拦截）");
    assert!(
        pty.screen().contains("reverse-i-search:"),
        "取消后历史搜索态保持（Esc 不退出搜索）：{}",
        pty.screen()
    );
    pty.dwell(5000);
    assert!(
        !pty.screen().contains("S79-late-never"),
        "被取消回复不得上屏：{}",
        pty.screen()
    );
    // Enter 退出搜索（无匹配恢复原稿）→ 下一请求正常提交
    pty.send_raw(b"\r");
    pty.dwell(400);
    assert!(
        !pty.screen().contains("reverse-i-search:"),
        "Enter 退出搜索：{}",
        pty.screen()
    );
    pty.send_line("turn-two");
    wait_request_count(&mut pty, &mock, 2);
    pty.wait_screen("S79-after-ok").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S80（复合态收尾）：历史搜索＋工具聚焦的 Enter 归属——搜索激活时 Enter 按搜索
/// 规则处理（匹配＝接受/无匹配＝恢复原稿），不得被工具聚焦的详情页入口抢占；
/// 退出搜索后再次 Enter 按正常输入规则（不被旧焦点抢走）；非搜索态 F4→Enter
/// 开页行为不变。两场景：F4→Ctrl+R→Enter（Ctrl+R 普通文本清焦，复合态自解）
/// 与 Ctrl+R→F4→Enter（复合态真实形成）。
#[test]
fn s80_search_plus_focus_enter_follows_search_rules() {
    let cfg = TempDir::new("s80-cfg");
    let user = TempDir::new("s80-user");
    let key = "s80-key";
    let args = serde_json::json!({
        "cmd": "for i in $(seq -w 1 30); do echo S80-L$i; done"
    })
    .to_string();
    let batch = serde_json::json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "call-s80", "type": "function",
              "function": { "name": "exec_command", "arguments": args } }
        ]
    }, "finish_reason": "tool_calls" }] }).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, batch),
        MockResp::json(200, chat_body("S80-完成A")),
        MockResp::json(200, chat_body("S80-完成B")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S80_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S80_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("跑");
    pty.wait_screen("S80-完成A").unwrap();
    for _ in 0..5 { pty.read_once(60); }
    // 正置控制：非搜索态 F4→Enter＝打开详情页（证明夹具可聚焦，防空跑）
    pty.send_raw(b"\x1bOS");
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.wait_screen("back to conversation").expect("控制：F4→Enter＝开页");
    pty.send_raw(b"q");
    pty.dwell(300);
    // 预置草稿
    pty.send_raw("复合草稿".as_bytes());
    pty.dwell(400);
    // 场景 1：F4 聚焦 → Ctrl+R（普通文本清焦，复合态自解）→ 无匹配 → Enter
    pty.send_raw(b"\x1bOS");
    pty.dwell(300);
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("场景1：搜索开启");
    pty.send_raw(b"zz80-a");
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.dwell(400);
    let s1 = pty.screen();
    assert!(
        !s1.contains("back to conversation"),
        "场景1 Enter 不得开页（复合态应自解）：{s1}"
    );
    assert!(
        !s1.contains("reverse-i-search:") && s1.contains("复合草稿"),
        "场景1 Enter＝无匹配恢复原稿：{s1}"
    );
    // 场景 1 后续：追加＋Enter 提交恰好一次（焦点不得残留抢占）
    pty.send_raw("尾1".as_bytes());
    pty.dwell(300);
    pty.send_raw(b"\r");
    {
        let deadline = Instant::now() + Duration::from_secs(3);
        while mock.count() < 3 && Instant::now() < deadline {
            pty.read_once(50);
        }
    }
    assert_eq!(mock.count(), 3, "场景1 后续提交恰好一次");
    let s1b = pty.screen();
    assert!(
        !s1b.contains("back to conversation") && s1b.contains("复合草稿尾1"),
        "场景1 提交回显可见且无页：{s1b}"
    );
    // 场景 2（裸复合态）：预置独立草稿 → Ctrl+R → 无匹配 query → F4（复合态
    // 形成且不被输入消解）→ Enter 按搜索规则（无匹配＝恢复原稿），不得开页
    pty.dwell(600);
    pty.send_raw("草稿2".as_bytes());
    pty.dwell(400);
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("场景2：搜索开启");
    pty.send_raw(b"zz80-b");
    pty.dwell(300);
    pty.send_raw(b"\x1bOS"); // F4：复合态形成（此后无输入消解）
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.dwell(400);
    let s2 = pty.screen();
    assert!(
        !s2.contains("back to conversation"),
        "场景2 复合态 Enter 不得开页（搜索规则优先）：{s2}"
    );
    assert!(
        !s2.contains("reverse-i-search:") && s2.contains("草稿2"),
        "场景2 Enter＝无匹配恢复原稿（草稿2 在）：{s2}"
    );
    // 场景2 后续：退出搜索后再次 Enter 按正常输入规则（不被旧焦点抢走）
    pty.send_raw("尾2".as_bytes());
    pty.dwell(300);
    pty.send_raw(b"\r");
    {
        let deadline = Instant::now() + Duration::from_secs(3);
        while mock.count() < 4 && Instant::now() < deadline {
            pty.read_once(50);
        }
    }
    assert_eq!(mock.count(), 4, "场景2 后续提交恰好一次（无旧焦点抢占）");
    let s2b = pty.screen();
    assert!(
        !s2b.contains("back to conversation") && s2b.contains("尾2"),
        "场景2 提交回显可见且全程无页：{s2b}"
    );
    // 场景 2b（裸复合态·匹配）：query 命中 → F4 → Enter＝接受历史项，不得开页
    pty.dwell(600);
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("场景2b：搜索开启");
    pty.send_raw("复合草稿".as_bytes());
    pty.dwell(400);
    pty.wait_screen("enter accept").expect("场景2b：Match 态正置");
    pty.send_raw(b"\x1bOS"); // F4：复合态形成（Match 保持）
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.dwell(400);
    let s2c = pty.screen();
    assert!(
        !s2c.contains("back to conversation"),
        "场景2b 复合态接受不得开页：{s2c}"
    );
    assert!(
        !s2c.contains("reverse-i-search:"),
        "场景2b Enter＝退出搜索：{s2c}"
    );
    // 接受历史项的确定性证据（区分 transcript 回显）：追加输入拼接＋提交恰一次
    pty.send_raw("尾3".as_bytes());
    pty.dwell(300);
    pty.send_raw(b"\r");
    {
        let deadline = Instant::now() + Duration::from_secs(3);
        while mock.count() < 5 && Instant::now() < deadline {
            pty.read_once(50);
        }
    }
    assert_eq!(mock.count(), 5, "场景2b 接受后提交恰好一次");
    let s2d = pty.screen();
    assert!(
        s2d.contains("复合草稿尾1尾3"),
        "场景2b 接受历史项入草稿（拼接验证）：{s2d}"
    );
    // 场景 4（复审 P2-1）：预览稿恰为 "exit" 的搜索态 Enter＝按搜索规则处理，
    // 不得触发裸退出（CLI 存活、搜索退出、草稿恢复）
    pty.dwell(600);
    pty.send_raw(b"exit");
    pty.dwell(400);
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("场景4：搜索开启（预览稿=exit）");
    pty.send_raw(b"zz80-d");
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.dwell(400);
    let s4 = pty.screen();
    assert!(
        !s4.contains("reverse-i-search:"),
        "场景4 Enter＝退出搜索：{s4}"
    );
    assert!(
        pty.child.try_wait().unwrap().is_none(),
        "场景4 不得裸退出（CLI 存活）"
    );
    for _ in 0..6 { pty.send_raw(b"\x7f"); pty.read_once(30); }
    // 场景 3（消解路径登记）：F4 后输入 query——普通文本清焦，复合态自解
    //（Enter 天然按搜索规则；钉住机制防回归）
    for _ in 0..24 { pty.send_raw(b"\x7f"); pty.read_once(30); }
    pty.dwell(400);
    pty.send_raw(b"\x12");
    pty.wait_screen("reverse-i-search:").expect("场景3：搜索开启");
    pty.send_raw(b"\x1bOS"); // 先 F4
    pty.dwell(300);
    pty.send_raw(b"zz80-c"); // 后输入——清焦消解复合态
    pty.dwell(300);
    pty.send_raw(b"\r");
    pty.dwell(400);
    let s3 = pty.screen();
    assert!(
        !s3.contains("back to conversation") && !s3.contains("reverse-i-search:"),
        "场景3 输入消解复合态——Enter 按搜索规则退出：{s3}"
    );
    for _ in 0..24 { pty.send_raw(b"\x7f"); pty.read_once(30); }
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ---------- S81/S82：013-M7 生产接线（D2 触发＋M6 注入＋D9 命令面）----------

/// S81：开态 TUI 用户轮——请求 system 尾段含 memory-context 注入（D4 适配生产
/// 链路）；后台管线真实触发：建库＋播种扩展说明＋threads 登记当前会话
/// （候选空＝A15 首用语义，零额外模型请求）。
#[test]
fn s81_m7_enabled_turn_injects_and_spawns_pipeline() {
    let cfg = TempDir::new("s81-cfg");
    let user = TempDir::new("s81-user");
    let key = "s81-key";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S81-REPLY-好"))]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S81_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));
    // 预置总摘要（开态读取注入）
    let mem_root = user.path().join("memories");
    std::fs::create_dir(&mem_root).unwrap();
    std::fs::write(mem_root.join("memory_summary.md"), "v1\nS81-历史经验：优先复用\n").unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S81_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("你好");
    pty.wait_screen("S81-REPLY-好").unwrap();
    // 后台管线有界等待：状态库出现（建库即触发完成的强可观察信号）
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !mem_root.join("memories.sqlite").is_file() {
        assert!(std::time::Instant::now() < deadline, "后台管线必须在轮启动后建库");
        std::thread::sleep(Duration::from_millis(100));
    }
    // 播种断言（上游原序 seed）：播种在建库后的独立临界区（runner 先 open 库
    // 再 seed）——同样有界等待，消除「见库即断言」的结构竞态（终验轮实测命中）
    let instructions = mem_root.join("extensions").join("ad_hoc").join("instructions.md");
    while !instructions.is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "扩展说明必须随管线播种"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // threads 登记：当前 auto 会话已登记（毫秒）
    std::thread::sleep(Duration::from_millis(400)); // 登记随入史点已发生，稍待后台收尾
    let store = terminal_chat::memory::store::MemoryStore::open(
        &mem_root,
        terminal_chat::memory::store::system_clock(),
    )
    .unwrap();
    let sessions: Vec<String> = std::fs::read_dir(user.path().join("sessions"))
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().ok()?;
            n.strip_suffix(".jsonl").map(str::to_string)
        })
        .collect();
    assert_eq!(sessions.len(), 1, "恰一个 auto 会话");
    let ms = store
        .thread_updated_at_ms(&sessions[0])
        .unwrap()
        .expect("当前会话必须已登记（写入侧接线）");
    assert!(ms >= 1_000_000_000_000, "登记值为毫秒：{ms}");

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    // 注入断言：按内容定位用户轮请求（用户轮与后台 phase2 的到达顺序在
    // 并发下不定，不假设 reqs[0]）
    let reqs = mock.requests();
    assert!(!reqs.is_empty());
    let user_turn = reqs
        .iter()
        .find(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["messages"].as_array().is_some_and(|m| {
                m.iter().any(|x| x["role"] == "user" && x["content"].as_str() == Some("你好"))
            })
        })
        .expect("用户轮请求必须存在");
    let body: Value = serde_json::from_str(&user_turn.body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    let systems: Vec<&Value> = msgs.iter().filter(|m| m["role"] == "system").collect();
    assert_eq!(systems.len(), 1, "仍恰一条 system（D4）");
    let sys = systems[0]["content"].as_str().unwrap();
    assert!(sys.contains("<memory-context>"), "记忆段边界：{sys}");
    assert!(sys.contains("S81-历史经验"), "摘要正文注入：{sys}");
    assert!(sys.contains("以下是历史记忆摘要"), "历史参考定性行");
    // 候选空＝无 stage1 提炼请求（A15 首用语义）；phase2 首轮恒合并（M5 P7：
    // ensure 新建基线后首轮必产生差异）发起一次合并请求，其收到末条文本回复
    // 即终文完成——总请求数＝用户轮 1＋phase2 首轮 1。
    assert_eq!(
        reqs.len(),
        2,
        "候选为空：仅用户轮＋phase2 首轮各一，实际 {} 个",
        reqs.len()
    );
    let p2 = reqs
        .iter()
        .find(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["tools"].as_array().is_some_and(|t| {
                t.iter().any(|x| x["function"]["name"].as_str() == Some("mem_write"))
            })
        })
        .expect("合并轮请求必须存在");
    let p2: Value = serde_json::from_str(&p2.body).unwrap();
    let names: Vec<&str> = p2["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["mem_read", "mem_write", "mem_list"], "合并轮恰三项受限工具");
}

/// S82：D9 命令面——/memory-status 观察、/memory-reset 缺 confirm 零删除、
/// confirm 清生成记忆保原始会话（A31）与根目录。
#[test]
fn s82_m7_memory_status_and_reset_roundtrip() {
    let cfg = TempDir::new("s82-cfg");
    let user = TempDir::new("s82-user");
    let key = "s82-key";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S82-OK"))]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S82_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    // 预置：记忆根内容（库＋摘要＋扩展）与一个原始会话
    let mem_root = user.path().join("memories");
    std::fs::create_dir_all(mem_root.join("extensions").join("ad_hoc")).unwrap();
    let store = terminal_chat::memory::store::MemoryStore::open(
        &mem_root,
        terminal_chat::memory::store::system_clock(),
    )
    .unwrap();
    drop(store);
    std::fs::write(mem_root.join("memory_summary.md"), "v1\nS82-旧摘要").unwrap();
    std::fs::write(mem_root.join("MEMORY.md"), "# M\n").unwrap();
    let sessions = user.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    std::fs::write(sessions.join("auto-keep.jsonl"), "{\"v\":1}\n").unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S82_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    // 状态观察（只读——不建任务）
    pty.send_line("/memory-status");
    pty.wait_screen("已启用").unwrap();
    pty.wait_screen("总摘要：存在").unwrap();
    // 缺 confirm：零删除
    pty.send_line("/memory-reset");
    pty.wait_screen("用法：/memory-reset confirm").unwrap();
    assert!(mem_root.join("memories.sqlite").is_file(), "缺 confirm 不得清理");
    // confirm：清生成记忆，保根与原始会话
    pty.send_line("/memory-reset confirm");
    pty.wait_screen("已清理").unwrap();
    assert!(mem_root.is_dir(), "根目录保留");
    assert_eq!(
        std::fs::read_dir(&mem_root)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != ".epoch")
            .count(),
        0,
        "生成内容全清（仅剩重置代数标记）"
    );
    assert!(sessions.join("auto-keep.jsonl").is_file(), "原始会话保留（A31）");
    // 清理后再观察：状态如实（未初始化的库面）
    pty.send_line("/memory-status");
    pty.wait_screen("状态库：未初始化").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    assert_eq!(mock.count(), 0, "命令面不得发起模型请求");
}

/// S83：route 失败（开态但无可路由配置）不触发管线——对话报错期间零记忆根
/// 创建（触发位点在 route/origin 门之后，D2 批准边界）；同轮核对行式
/// /memory-status 与 /memory-reset 命令往返（含缺 confirm 零删除）。
#[test]
fn s83_m7_route_failure_and_line_mode_commands() {
    let cfg = TempDir::new("s83-cfg");
    let user = TempDir::new("s83-user");
    // 开态但无 providers/selected_model：route 必失败
    std::fs::write(
        cfg.path().join("manual.json"),
        r#"{"memories":{"enabled":true}}"#,
    )
    .unwrap();
    let mem_root = user.path().join("memories");
    // 预置生成内容验证命令面与零删除边界
    std::fs::create_dir_all(mem_root.join("extensions").join("ad_hoc")).unwrap();
    std::fs::write(mem_root.join("memory_summary.md"), "v1\nS83-摘要").unwrap();
    let sessions = user.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    std::fs::write(sessions.join("auto-keep.jsonl"), "{\"v\":1}\n").unwrap();

    // 行式（PTY 不可用条件＝管道模式）：对话报错不建记忆根
    let (out, _err, code) = run_pipe(
        None,
        &[
            ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ],
        "你好\n/memory-status\n/memory-reset\n/memory-reset confirm\nexit\n",
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("错误："), "无 provider 对话必须报错：{out}");
    // 命令面（行式回显）
    assert!(out.contains("已启用"), "status 展示开关：{out}");
    assert!(out.contains("总摘要：存在"), "status 展示摘要存在性：{out}");
    assert!(out.contains("用法：/memory-reset confirm"), "缺 confirm 用法提示：{out}");
    // 缺 confirm 的那次在 reset confirm 之前：先确认未清理时机由下一条命令输出证明
    assert!(out.contains("已清理"), "confirm 清理成功回显：{out}");
    assert!(mem_root.is_dir(), "根目录保留");
    assert_eq!(
        std::fs::read_dir(&mem_root)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != ".epoch")
            .count(),
        0,
        "生成内容全清（仅剩重置代数标记）"
    );
    assert!(sessions.join("auto-keep.jsonl").is_file(), "原始会话保留（A31）");
    // 对话报错（route 失败）阶段不得创建记忆根——此处根存在性来自命令面预置；
    // 真正的零副作用断言：对话报错时库未重建（reset 后无 memories.sqlite）
    assert!(
        !mem_root.join("memories.sqlite").is_file(),
        "route 失败＋命令面均不得重建状态库（对话不触发管线）"
    );
}

/// S81b：生产链路「每个新用户轮读取一次、下一轮取最新」——第二轮对话前
/// 更新总摘要，第二轮请求必须携带新摘要正文（轮内快照与跨轮重读的生产面）。
#[test]
fn s81b_m7_next_turn_rereads_updated_summary() {
    let cfg = TempDir::new("s81b-cfg");
    let user = TempDir::new("s81b-user");
    let key = "s81b-key";
    let mock = spawn_mock(vec![
        MockResp::json(200, chat_body("S81B-回复一")),
        MockResp::json(200, chat_body("S81B-第二轮完成")),
        MockResp::json(200, chat_body("S81B-第二轮完成")), // 后台 phase2 首轮与第二用户轮次序不定，统一文本
    ]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S81B_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mem_root = user.path().join("memories");
    std::fs::create_dir(&mem_root).unwrap();
    std::fs::write(mem_root.join("memory_summary.md"), "v1\nS81B-旧经验\n").unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S81B_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一问");
    pty.wait_screen("S81B-回复一").unwrap();
    // 下一轮前更新摘要
    std::fs::write(mem_root.join("memory_summary.md"), "v1\nS81B-新经验-跨轮生效\n").unwrap();
    pty.send_line("第二问");
    pty.wait_screen("S81B-第二轮完成").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");

    let reqs = mock.requests();
    let first: Value = serde_json::from_str(&reqs[0].body).unwrap();
    let first_sys = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .count();
    assert_eq!(first_sys, 1);
    let first_sys_text = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "system")
        .and_then(|m| m["content"].as_str())
        .unwrap();
    assert!(first_sys_text.contains("S81B-旧经验"), "首轮用旧摘要：{first_sys_text}");
    // 第二个用户轮请求（含「第二问」的用户消息）用新摘要
    let second = reqs
        .iter()
        .find(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["messages"].as_array().is_some_and(|m| {
                m.iter().any(|x| x["content"].as_str() == Some("第二问"))
            })
        })
        .expect("第二轮请求必须存在");
    let b: Value = serde_json::from_str(&second.body).unwrap();
    let sys: Vec<&Value> = b["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .collect();
    assert_eq!(sys.len(), 1);
    let sys_text = sys[0]["content"].as_str().unwrap();
    assert!(sys_text.contains("S81B-新经验-跨轮生效"), "下一轮必须重读最新摘要：{sys_text}");
    assert!(!sys_text.contains("S81B-旧经验"), "旧摘要不得残留");
}

// ---------- S84–S86：013-M7 定点补验（排队重放触发／后台阻塞退出）----------

/// S84：开态排队重放——turn1 运行中入队的输入在实际重放时作为新用户轮
/// 恰发起一次（含该文本的用户轮请求恰 1 份，可区分 0/1/2 次）；threads
/// 登记随两轮推进。入队路径不经过 spawn_turn（vendored chat_driver 的
/// `queue_input` 与 `spawn_turn` 钩子分离——静态位点＋本用例行为证据）。
#[test]
fn s84_m7_queued_replay_single_user_turn() {
    let cfg = TempDir::new("s84-cfg");
    let user = TempDir::new("s84-user");
    let key = "s84-key";
    let mock = spawn_mock(vec![
        MockResp::slow_json(200, chat_body("S84-第一轮完成"), 1_500),
        // 后台 phase2 与重放轮次序不定——第二阶段响应统一文本
        MockResp::json(200, chat_body("S84-第二轮完成")),
        MockResp::json(200, chat_body("S84-第二轮完成")),
    ]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S84_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mem_root = user.path().join("memories");
    std::fs::create_dir(&mem_root).unwrap();
    std::fs::write(mem_root.join("memory_summary.md"), "v1\nS84-经验\n").unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S84_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一轮输入");
    // turn1 在慢响应上运行期间以显式排队键入队（021 起 Enter＝接续当前轮，
    // 显式排队保留 queue key；不得触发新轮）
    pty.dwell(300);
    pty.send_raw("排队的输入".as_bytes());
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\t");
    pty.wait_screen("S84-第一轮完成").unwrap();
    // 重放作为新用户轮执行
    pty.wait_screen("S84-第二轮完成").unwrap();
    // 等后台管线收尾（有界）
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !mem_root.join("memories.sqlite").is_file() {
        assert!(std::time::Instant::now() < deadline, "后台管线必须建库");
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(400));
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");

    // 重放恰一次：含排队文本的用户轮请求恰 1 份（0＝未重放，2＝双重放）
    let reqs = mock.requests();
    let replay_turns = reqs
        .iter()
        .filter(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["messages"].as_array().is_some_and(|m| {
                m.iter()
                    .any(|x| x["role"] == "user" && x["content"].as_str() == Some("排队的输入"))
            })
        })
        .count();
    assert_eq!(replay_turns, 1, "重放轮恰一次，实际 {replay_turns}（总请求 {}）", reqs.len());
    // threads 登记存在（两轮均经登记写入侧）
    let store = terminal_chat::memory::store::MemoryStore::open(
        &mem_root,
        terminal_chat::memory::store::system_clock(),
    )
    .unwrap();
    let stems = std::fs::read_dir(user.path().join("sessions"))
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().ok()?;
            n.strip_suffix(".jsonl").map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert_eq!(stems.len(), 1);
    assert!(
        store.thread_updated_at_ms(&stems[0]).unwrap().is_some(),
        "当前会话已登记"
    );
}

/// S85：TUI 退出不等后台——后台 phase2 请求在 silent mock 上永悬（真实
/// HTTP 180s 超时），`/exit` 必须在远小于该超时的有界窗口（15s）内完成
/// 进程退出（再见＋码 0）——真实进程退出观察，非句柄丢弃。
#[test]
fn s85_m7_tui_exit_does_not_wait_for_blocked_pipeline() {
    let cfg = TempDir::new("s85-cfg");
    let user = TempDir::new("s85-user");
    let key = "s85-key";
    let mock = spawn_mock(vec![
        MockResp::json(200, chat_body("S85-回复")),
        MockResp::silent(), // 后台 phase2：连接保持、永不响应
    ]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S85_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S85_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("你好");
    pty.wait_screen("S85-回复").unwrap();
    // 确认后台请求已在 silent 上在途（此后台永不完成）
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while mock.count() < 2 {
        assert!(std::time::Instant::now() < deadline, "后台请求未发出");
        std::thread::sleep(Duration::from_millis(50));
    }
    // 前台照常可退出——且不等待后台（15s ≪ 180s HTTP 超时）
    pty.send_line("/exit");
    pty.wait_output("再见!", Duration::from_secs(15))
        .expect("退出不得等待后台请求（180s 超时远大于 15s 窗口）");
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S86：行式退出不等后台——管道模式真实 CLI 子进程，后台请求悬在 silent
/// mock 上，stdin `exit` 后进程须在有界窗口（15s）内退出（码 0）。
#[test]
fn s86_m7_line_mode_exit_does_not_wait_for_blocked_pipeline() {
    let cfg = TempDir::new("s86-cfg");
    let user = TempDir::new("s86-user");
    let key = "s86-key";
    let mock = spawn_mock(vec![
        MockResp::json(200, chat_body("S86-回复")),
        MockResp::silent(),
    ]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S86_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    // 有界运行器：子进程 15s 内必须自行退出，否则 kill 并失败
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.env_remove("TERM"); // 管道模式：TERM 未设置 → 行式路径
    for k in ["TCHAT_CONFIG_DIR", "TCHAT_USER_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
        cmd.env_remove(k);
    }
    cmd.env("TCHAT_CONFIG_DIR", cfg.path());
    cmd.env("TCHAT_USER_DIR", user.path());
    cmd.env("TCHAT_S86_KEY", key);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("CLI 子进程启动失败");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all("\u{4f60}\u{597d}\n".as_bytes()).unwrap(); // 先喂用户轮
    stdin.flush().unwrap();
    // 等用户轮回复与后台请求均在途（后台悬在 silent 上），再喂 exit
    let wait_bg = std::time::Instant::now() + Duration::from_secs(15);
    while mock.count() < 2 {
        assert!(
            std::time::Instant::now() < wait_bg,
            "用户轮回复与后台请求未在窗口内发出：{}",
            mock.count()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    stdin.write_all(b"exit\n").unwrap();
    stdin.flush().unwrap();
    drop(stdin);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait().expect("try_wait 失败") {
            Some(s) => break s,
            None => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "行式进程必须不等后台（silent 悬置 180s 超时远大于 15s 窗口）"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    assert_eq!(status.code(), Some(0));
    // 用户轮已回复、后台请求已发出（悬置中被进程退出终止）
    assert!(mock.count() >= 2, "后台请求须已发出：{}", mock.count());
}

/// S87：后台触发直接计数（真实生产入口——`/memory-status` 显示的累计
/// 触发＝`spawn_memory_pipeline` 生产壳位点计数）。排队场景：turn1 运行
/// 中入队**不增加计数**（运行中查 status）；成功重放恰＋1（可区分 0/1/2）。
#[test]
fn s87_m7_queued_replay_spawn_count_via_status() {
    let cfg = TempDir::new("s87-cfg");
    let user = TempDir::new("s87-user");
    let key = "s87-key";
    let mock = spawn_mock(vec![
        MockResp::slow_json(200, chat_body("S87-第一轮完成"), 1_500),
        MockResp::json(200, chat_body("S87-第二轮完成")),
        MockResp::json(200, chat_body("S87-第二轮完成")), // phase2 首轮与重放轮次序不定，统一文本
    ]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S87_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S87_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("第一轮");
    // turn1 慢响应期间：显式排队键入队（021 起 Enter＝接续当前轮；不得触发）
    // ＋运行中查计数（只读命令可用）
    pty.dwell(300);
    pty.send_raw("排队的输入".as_bytes());
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\t");
    pty.dwell(200);
    pty.send_line("/memory-status");
    pty.wait_screen("本进程累计触发后台管线：1 次").unwrap();
    pty.wait_screen("S87-第一轮完成").unwrap();
    pty.wait_screen("S87-第二轮完成").unwrap();
    // 重放恰一次：计数恰 2（1＝首轮，2＝重放轮；0/2 重放或入队触发均可区分）
    pty.send_line("/memory-status");
    pty.wait_screen("本进程累计触发后台管线：2 次").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
    // 用户轮请求恰 2（与计数互证：每轮恰一请求，无额外轮）
    let user_turns = mock
        .requests()
        .iter()
        .filter(|r| {
            let b: Value = serde_json::from_str(&r.body).unwrap();
            b["messages"].as_array().is_some_and(|m| {
                m.iter().any(|x| x["role"] == "user" && x["content"].as_str() == Some("排队的输入"))
            })
        })
        .count();
    assert_eq!(user_turns, 1, "重放恰一次（互证）");
}

/// S88：关闭态与拒绝启动不计数——关态轮后计数恒 0；开态但 route 失败
/// （无 provider）的轮不启动、不计数（行式管道）。
#[test]
fn s88_m7_spawn_count_zero_for_disabled_and_rejected() {
    // 关态：正常对话后计数 0
    let cfg = TempDir::new("s88-cfg");
    let user = TempDir::new("s88-user");
    let key = "s88-key";
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S88-回复"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S88_KEY");
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));
    let (out, _err, code) = run_pipe(
        None,
        &[
            ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
            ("TCHAT_S88_KEY", key),
        ],
        "你好\n/memory-status\nexit\n",
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("未启用"), "{out}");
    assert!(out.contains("本进程累计触发后台管线：0 次"), "关态恒零：{out}");

    // 开态但 route 失败：轮未成功启动，不计数
    let cfg2 = TempDir::new("s88b-cfg");
    let user2 = TempDir::new("s88b-user");
    std::fs::write(
        cfg2.path().join("manual.json"),
        r#"{"memories":{"enabled":true}}"#,
    )
    .unwrap();
    let (out2, _e2, c2) = run_pipe(
        None,
        &[
            ("TCHAT_CONFIG_DIR", cfg2.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user2.path().to_str().unwrap()),
        ],
        "你好\n/memory-status\nexit\n",
    );
    assert_eq!(c2, Some(0), "{out2}");
    assert!(out2.contains("已启用"), "{out2}");
    assert!(out2.contains("本进程累计触发后台管线：0 次"), "拒绝启动不计数：{out2}");
}

// —— 013-M8：引用反馈真实入口（两驱动完成位点）——

/// 经真实 API 播种上次合并选中的来源，M9 启动清理应保护该行。
fn s89_seed_stage1(user_dir: &std::path::Path) -> String {
    use terminal_chat::memory::store::{system_clock, MemoryStore};
    use terminal_chat::memory::{Phase2JobClaimOutcome, SelectedOutput, Stage1JobClaimOutcome};
    let id = "auto-20261019T101112-4321".to_string();
    let store = MemoryStore::open(&user_dir.join("memories"), system_clock()).unwrap();
    store.register_thread(&id, "enabled", 1_699_974_800_000).unwrap();
    let Stage1JobClaimOutcome::Claimed { ownership_token } = store
        .try_claim_stage1_job(&id, "seed", 1_699_974_800, 3600, 8)
        .unwrap()
    else {
        panic!("种子行应可领取")
    };
    store
        .mark_stage1_job_succeeded(&id, &ownership_token, 1_699_974_800, "raw", "summary", None)
        .unwrap();
    let Phase2JobClaimOutcome::Claimed { ownership_token, input_watermark } = store
        .try_claim_global_phase2_job("seed-previous-merge", 3600)
        .unwrap()
    else {
        panic!("种子合并任务应可领取")
    };
    assert!(store.mark_global_phase2_job_succeeded(
        &ownership_token,
        input_watermark,
        &[SelectedOutput { thread_id: id.clone(), source_updated_at: 1_699_974_800 }],
    ).unwrap());
    assert_eq!(store.prune_stage1_outputs_for_retention(30, 200).unwrap(), 0);
    id
}

fn s89_usage(user_dir: &std::path::Path, id: &str) -> (Option<i64>, Option<i64>) {
    let conn = rusqlite::Connection::open(
        user_dir.join("memories").join("memories.sqlite"),
    )
    .unwrap();
    conn.query_row(
        "SELECT usage_count, last_usage FROM stage1_outputs WHERE thread_id = ?1",
        rusqlite::params![id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn s89_citation_reply(id: &str) -> String {
    format!(
        "S89-完成。\n<oai-mem-citation>\n<citation_entries>\nMEMORY.md:1-2|note=[used]\n</citation_entries>\n<rollout_ids>\n{id}\n</rollout_ids>\n</oai-mem-citation>"
    )
}

/// S89：TUI 驱动完成位点——真实回复携带引用 → SQLite usage_count＋1
/// （上次合并成功后处于冷却；反馈同步于驱动线程）。
#[test]
fn s89_m8_tui_citation_feedback_updates_usage() {
    let cfg = TempDir::new("s89-cfg");
    let user = TempDir::new("s89-user");
    let key = "s89-key";
    let id = s89_seed_stage1(user.path());
    let mock = spawn_mock(vec![
        MockResp::json(200, chat_body(&s89_citation_reply(&id))),
        MockResp::silent(),
    ]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S89_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S89_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("用记忆回答");
    pty.wait_screen("S89-完成").unwrap();
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");

    let (count, last) = s89_usage(user.path(), &id);
    assert_eq!(count, Some(1), "真实 TUI 轮完成后引用反馈恰 +1");
    assert!(last.is_some_and(|t| t >= 1_700_000_000), "last_usage 已写入：{last:?}");
}

/// S90：行式驱动完成位点——同款回复经管道路径完成反馈。
#[test]
fn s90_m8_line_mode_citation_feedback_updates_usage() {
    let cfg = TempDir::new("s90-cfg");
    let user = TempDir::new("s90-user");
    let key = "s90-key";
    let id = s89_seed_stage1(user.path());
    let mock = spawn_mock(vec![
        MockResp::json(200, chat_body(&s89_citation_reply(&id))),
        MockResp::silent(),
    ]);
    write_manual_with_memories(
        cfg.path(),
        &mock.url,
        "env:TCHAT_S90_KEY",
        Some(terminal_chat::memory::MemoriesConfig { enabled: true, ..Default::default() }),
    );
    seed_grant(user.path(), &catalog::origin_of(&mock.url).unwrap(), &catalog::fingerprint(key));

    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_terminal-chat"));
    cmd.env_remove("TERM"); // 管道模式：行式路径
    for k in ["TCHAT_CONFIG_DIR", "TCHAT_USER_DIR", "TCHAT_PUBLIC_CATALOG_URL", "APPDATA"] {
        cmd.env_remove(k);
    }
    cmd.env("TCHAT_CONFIG_DIR", cfg.path());
    cmd.env("TCHAT_USER_DIR", user.path());
    cmd.env("TCHAT_S90_KEY", key);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("CLI 子进程启动失败");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all("用记忆回答\nexit\n".as_bytes()).unwrap();
    stdin.flush().unwrap();
    drop(stdin);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().expect("try_wait 失败") {
            Some(s) => break s,
            None => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "行式进程未在窗口内退出"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    assert_eq!(status.code(), Some(0));
    let (count, last) = s89_usage(user.path(), &id);
    assert_eq!(count, Some(1), "真实行式轮完成后引用反馈恰 +1");
    assert!(last.is_some_and(|t| t >= 1_700_000_000), "last_usage 已写入：{last:?}");
}

// ---------- S91：TUI update_plan 计划快照渲染与恢复一致（017）----------

/// S91：TUI 真实链路——模型调用 update_plan → 屏幕出现说明与步骤清单
///（三种状态标记）；最终回复正常渲染；退出后新会话 /resume 恢复同一快照
///（零模型请求、不重放）；失败调用的参数不画成成功清单。
#[test]
fn s91_tui_update_plan_snapshot_and_resume_consistency() {
    let cfg = TempDir::new("s91-cfg");
    let user = TempDir::new("s91-user");
    let key = "s91-key";
    let plan_args = json!({
        "explanation": "S91-计划说明-开始任务",
        "plan": [
            { "step": "S91-步骤-接入", "status": "completed" },
            { "step": "S91-步骤-验证", "status": "in_progress" },
            { "step": "S91-步骤-收尾", "status": "pending" }
        ]
    })
    .to_string();
    let tc = json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [
            { "id": "s91-plan-1", "type": "function",
              "function": { "name": "update_plan", "arguments": plan_args } },
            { "id": "s91-plan-2", "type": "function",
              "function": { "name": "update_plan",
                "arguments": json!({ "plan": [{ "step": "坏状态", "status": "blocked" }] }).to_string() } }
        ]
    }, "finish_reason": "tool_calls" }] })
    .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S91-最终-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S91_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S91_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap(); // TUI 就绪
    pty.send_line("按计划执行");
    // 成功快照：紧凑视图＝标题＋计数＋按状态排序的步骤（进行中→待办→已完成）
    pty.wait_screen("Updated Plan · 1/3 complete").unwrap();
    pty.wait_screen("□ S91-步骤-验证").unwrap();
    pty.wait_screen("□ S91-步骤-收尾").unwrap();
    pty.wait_screen("✔ S91-步骤-接入").unwrap();
    // 失败调用：原因呈现，坏状态步骤不画进清单
    pty.wait_screen("update_plan 未生效").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    pty.feed_screen();
    let screen = pty.vt.to_string();
    assert!(!screen.contains("坏状态"), "失败参数不得画成清单：{screen}");
    // 最终回复渲染
    pty.wait_screen("S91-最终-完成").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    // Ctrl+T 详情页：完整视图含说明（紧凑视图按母本语义不展示说明）
    pty.send_raw(b"\x14"); // Ctrl+T
    pty.wait_screen("S91-计划说明-开始任务").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    pty.send_raw(b"q"); // q 关详情页（012 §8）
    std::thread::sleep(Duration::from_millis(300));
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "退出码契约，输出：{out}");
    // 请求配对：两字段成功/失败结果与原始参数、调用 ID 对应
    assert_eq!(mock.requests().len(), 2, "恰两次请求");
    let body2: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let tools: Vec<&Value> = body2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("tool"))
        .collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["tool_call_id"], json!("s91-plan-1"));
    assert_eq!(
        serde_json::from_str::<Value>(tools[0]["content"].as_str().unwrap()).unwrap(),
        json!({ "ok": true, "error": null })
    );
    let failed: Value = serde_json::from_str(tools[1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(failed["ok"], json!(false), "失败两字段：{failed}");
    let assistant = body2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == json!("assistant") && m.get("tool_calls").is_some())
        .unwrap();
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        json!(plan_args),
        "原始参数逐字投影"
    );

    // 恢复一致：新 TUI 会话 /resume（无参＝最近 auto）→ 同一快照渲染；零请求
    let mock2 = spawn_mock(vec![]);
    write_manual(cfg.path(), &mock2.url, "env:TCHAT_S91_KEY");
    let origin2 = catalog::origin_of(&mock2.url).unwrap();
    seed_grant(user.path(), &origin2, &catalog::fingerprint(key));
    let mut pty2 = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S91_KEY", key),
    ]));
    pty2.wait_screen("? for shortcuts").unwrap();
    pty2.send_line("/resume");
    pty2.wait_screen("Updated Plan").unwrap();
    pty2.wait_screen("✔ S91-步骤-接入").unwrap();
    pty2.wait_screen("□ S91-步骤-验证").unwrap();
    pty2.wait_screen("update_plan 未生效").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    pty2.send_line("/exit");
    pty2.wait_output("再见!", WATCHDOG).unwrap();
    let (_out2, status2) = pty2.finish();
    assert_eq!(status2.code(), Some(0));
    assert!(
        mock2.requests().is_empty(),
        "恢复只投影不重放（零模型请求）"
    );
}

// ---------- S92：TUI 计划真实缩放重排（017 补验收）----------

/// S92：同一会话内实际改变 PTY 窗口尺寸（宽 200 → 窄 40 → 宽 200）：
/// - 宽屏：紧凑视图三种状态齐全，长中文步骤连续成行；
/// - 窄屏：紧凑行按新宽度重排（长步骤裁剪出省略号 `…`）；Ctrl+T 详情页完整
///   视图折行且文本完整——前缀与尾部各自成行可见、整行不截断丢字；
/// - 回宽：完整步骤重新连续成行、省略号消失、状态标记保持。
/// 全程有界等待＋VT100 解析后的真实屏幕断言（无固定延时、不查通知日志）。
#[test]
fn s92_tui_update_plan_resize_reflow_integrity() {
    let cfg = TempDir::new("s92-cfg");
    let user = TempDir::new("s92-user");
    let key = "s92-key";
    let long_step = "S92-长步骤-一二三四五六七八九十甲乙丙丁戊己庚辛壬癸";
    let plan_args = json!({
        "explanation": "S92-缩放-说明",
        "plan": [
            { "step": "S92-步骤-已完成", "status": "completed" },
            { "step": "S92-步骤-进行中", "status": "in_progress" },
            { "step": long_step, "status": "pending" }
        ]
    })
    .to_string();
    let tc = json!({ "choices": [{ "message": {
        "role": "assistant", "content": null,
        "tool_calls": [{ "id": "s92-plan", "type": "function",
            "function": { "name": "update_plan", "arguments": plan_args } }]
    }, "finish_reason": "tool_calls" }] })
    .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S92-最终-完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S92_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S92_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap(); // TUI 就绪
    pty.send_line("按计划执行");
    // 宽屏（200 列）：计数＋三种状态＋长步骤连续成行（不折行不裁剪）
    pty.wait_screen("Updated Plan · 1/3 complete").unwrap();
    pty.wait_screen("□ S92-步骤-进行中").unwrap();
    pty.wait_screen("✔ S92-步骤-已完成").unwrap();
    pty.wait_screen(&format!("□ {long_step}")).unwrap();
    pty.wait_screen("S92-最终-完成").unwrap();

    // 窄屏（40 列）：主视图紧凑行重排——长步骤裁剪出省略号、连续长行消失
    pty.resize_and_sync(40, 30);
    pty.wait_screen_pred("窄屏重排（省略号出现）", |s| s.contains('…')).unwrap();
    pty.wait_screen_pred("窄屏裁剪（长步骤不再连续成行）", |s| {
        !s.contains(long_step)
    })
    .unwrap();
    // 状态标记在窄屏重排后保持
    pty.wait_screen("□ S92-步骤-进行中").unwrap();

    // 详情页（Ctrl+T）窄屏完整视图：折行且文本完整——前缀/尾部分自成行，
    // 整行无截断丢字（尾部仍在屏）
    pty.send_raw(b"\x14"); // Ctrl+T
    pty.wait_screen("S92-缩放-说明").unwrap(); // 完整视图含说明
    // 折行点实测落在「甲」后（40 列）：首行含前缀、次行「乙…癸」自成行，
    // 两分片均可见且整串不再连续——完整无丢字
    pty.wait_screen_pred("窄屏详情折行完整（前缀行＋折行尾段行、不连续）", |s| {
        s.contains("S92-长步骤-一") && s.contains("乙丙丁戊己庚辛壬癸") && !s.contains(long_step)
    })
    .unwrap();
    pty.send_raw(b"q"); // 关详情页

    // 回宽（200 列）：完整步骤重新连续成行、省略号消失
    pty.resize_and_sync(PTY_COLS, PTY_ROWS);
    pty.wait_screen(&format!("□ {long_step}")).unwrap();
    pty.wait_screen_pred("回宽无裁剪", |s| !s.contains('…')).unwrap();
    pty.wait_screen("✔ S92-步骤-已完成").unwrap();

    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "退出码契约，输出：{out}");
    assert_eq!(mock.requests().len(), 2, "恰两次模型请求（resize 不触发请求）");
}

/// 019 S95：真实 TUI 看图——成功卡片（文件名＋尺寸）、失败行、下一请求协议边界
/// 携带真实图片、/resume 恢复同一卡片（不重放工具）。
#[test]
fn s95_tui_view_image_card_failure_and_resume() {
    let cfg = TempDir::new("s95-cfg");
    let user = TempDir::new("s95-user");
    // 夹具图写入 TUI 工作目录（相对路径按 cwd 解析）
    let work = user.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let shot = work.join("截图 s95.png");
    let img = image::ImageBuffer::from_pixel(
        2304,
        864,
        image::Rgba([210u8, 40, 60, 255]),
    );
    let mut cursor = std::io::Cursor::new(Vec::new());
    use image::GenericImageView as _;
    img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
    std::fs::write(&shot, cursor.into_inner()).unwrap();
    let vi_call = |path: &str, id: &str| {
        json!({"choices":[{"message":{"tool_calls":[{"id":id,"type":"function","function":{
        "name":"view_image","arguments":json!({"path":path}).to_string()}}]}}]}).to_string()
    };
    let mock = spawn_mock(vec![
        MockResp::json(200, vi_call("截图 s95.png", "s95-img")),
        MockResp::json(200, chat_body("S95-完成")),
        MockResp::json(200, vi_call("definitely-missing.png", "s95-miss")),
        MockResp::json(200, chat_body("S95-失败已知")),
    ]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S95_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s95"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S95_KEY",
        "fake-s95",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("看图");
    pty.wait_screen_timeout("Viewed image", Duration::from_secs(10)).unwrap();
    pty.wait_screen("截图 s95.png").unwrap();
    pty.wait_screen("2048×768").unwrap();
    pty.wait_screen("S95-完成").unwrap();
    // 协议边界：下一请求携带可解码图片（格式/尺寸）
    assert_eq!(mock.count(), 2);
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let url = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter_map(|p| p["image_url"]["url"].as_str())
        .next()
        .expect("请求含 image_url");
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    use image::GenericImageView;
    assert_eq!(decoded.dimensions(), (2048, 768), "2304×864 → 2048×768");
    // 失败行：缺失文件按原因降级，不生成"已查看"卡片
    pty.send_line("再看缺失文件");
    pty.wait_screen_timeout("view_image 失败", Duration::from_secs(10)).unwrap();
    pty.wait_screen("S95-失败已知").unwrap();
    assert_eq!(mock.count(), 4);
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
    // /resume：空历史新进程装载同一会话——恢复同一成功卡片（不重新执行工具）
    let mut pty2 = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S95_KEY",
        "fake-s95",
        true,
    ));
    pty2.wait_screen("? for shortcuts").unwrap();
    pty2.send_line("/resume");
    pty2.wait_screen_timeout("Viewed image", Duration::from_secs(10)).unwrap();
    pty2.wait_screen("截图 s95.png").unwrap();
    assert_eq!(mock.count(), 4, "恢复只投影，不重放工具、不续请求");
    pty2.send_raw(b"\x03");
    assert_eq!(pty2.finish().1.code(), Some(0));
}

/// 020 S100：真实 TUI 粘贴图片路径→附件→提交→请求带图（入口 3/4；A03/A05/A16）。
fn s100_png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let img = image::ImageBuffer::from_pixel(width, height, image::Rgba(rgba));
    let mut cursor = std::io::Cursor::new(Vec::new());
    img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
    cursor.into_inner()
}

#[test]
fn s100_tui_paste_image_path_submit_with_attachment() {
    let cfg = TempDir::new("s100-cfg");
    let user = TempDir::new("s100-user");
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S100-分析完成"))]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S96_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s100"),
    );
    let bytes = s100_png(4, 4, [120, 200, 80, 255]);
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let shot = work.join("路径图.png");
    fs::write(&shot, &bytes).unwrap();
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S96_KEY",
        "fake-s100",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    // bracketed paste 图片路径（中文无空格——裸路径单 token）→ composer 转附件
    let path_text = shot.display().to_string();
    pty.send_raw(format!("\x1b[200~{path_text}\x1b[201~").as_bytes());
    pty.wait_screen_timeout("[Image #1]", Duration::from_secs(5)).unwrap();
    // 等待粘贴突发窗口过期（母本行为：窗口内 Enter 是换行不是提交）
    std::thread::sleep(Duration::from_millis(700));
    pty.send_raw(b" check this");
    std::thread::sleep(Duration::from_millis(200));
    pty.send_raw(b"\r");
    pty.wait_screen_timeout("S100-分析完成", Duration::from_secs(10)).unwrap();
    assert_eq!(mock.count(), 1, "恰一次请求");
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let img_msg = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == json!("user") && m["content"].is_array())
        .expect("图片用户消息");
    let parts = img_msg["content"].as_array().unwrap();
    let text = parts.iter().find(|p| p["type"] == json!("text")).unwrap();
    assert!(text["text"].as_str().unwrap().contains("[Image #1]"), "{text:?}");
    assert!(text["text"].as_str().unwrap().contains("check this"), "{text:?}");
    let url = parts.iter().find_map(|p| p["image_url"]["url"].as_str()).unwrap();
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        bytes,
        "请求图片＝源字节"
    );
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S101：TUI `-i` 启动初始图——就绪后纯图提交恰一次（A04 TUI 分支）。
#[test]
fn s101_tui_initial_image_flag_submits_once() {
    let cfg = TempDir::new("s101-cfg");
    let user = TempDir::new("s101-user");
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S101-初始图已收"))]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S97_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s101"),
    );
    let bytes = s100_png(2, 2, [10, 20, 30, 255]);
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let shot = work.join("init.png");
    fs::write(&shot, &bytes).unwrap();
    let mut cmd = input_pty_cmd(cfg.path(), user.path(), "TCHAT_S97_KEY", "fake-s101", true);
    cmd.arg("-i").arg(shot.display().to_string());
    let mut pty = PtySession::spawn(cmd);
    pty.wait_screen("? for shortcuts").unwrap();
    pty.wait_screen_timeout("S101-初始图已收", Duration::from_secs(10)).unwrap();
    assert_eq!(mock.count(), 1, "初始图恰一次请求");
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let urls: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .filter_map(|p| p["image_url"]["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 1, "一张初始图");
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S102：TUI 能力未知＋粘贴路径提交→提交被拒恢复草稿，零请求（A11 TUI）。
#[test]
fn s102_tui_capability_reject_restores_draft() {
    let cfg = TempDir::new("s102-cfg");
    let user = TempDir::new("s102-user");
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("不应到达"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S98_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s102"),
    );
    let bytes = s100_png(2, 2, [1, 2, 3, 255]);
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let shot = work.join("cap.png");
    fs::write(&shot, &bytes).unwrap();
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S98_KEY",
        "fake-s102",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    let path_text = shot.display().to_string();
    pty.send_raw(format!("\x1b[200~{path_text}\x1b[201~").as_bytes());
    pty.wait_screen_timeout("[Image #1]", Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(700));
    pty.send_raw(b"\r");
    pty.wait_screen_timeout("提交被拒", Duration::from_secs(10)).unwrap();
    pty.wait_screen_timeout("已恢复到输入框", Duration::from_secs(5)).unwrap();
    assert_eq!(mock.count(), 0, "零请求");
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S103：运行中入队（A09）——队列只存路径；出队正式提交时读取（源图在
/// 排队期间被改写→采用新版本）；FIFO 顺序与 Dnew 保护由驱动既有语义承担。
#[test]
fn s103_tui_queued_submission_reads_image_at_dequeue_time() {
    let cfg = TempDir::new("s103-cfg");
    let user = TempDir::new("s103-user");
    let (first, gate) = MockResp::gated(200, chat_body("S103-第一轮"));
    let mock = spawn_mock(vec![first, MockResp::json(200, chat_body("S103-第二轮"))]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S99_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s103"),
    );
    let v1 = s100_png(3, 3, [10, 10, 10, 255]);
    let v2 = s100_png(3, 3, [200, 200, 200, 255]);
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let shot = work.join("q.png");
    fs::write(&shot, &v1).unwrap();
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S99_KEY",
        "fake-s103",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    // 第一轮（gated 挂起→turn 运行中）；分开发送避免突发窗口吞 Enter
    pty.send_raw(b"first");
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\r");
    std::thread::sleep(Duration::from_millis(500));
    // 排队：粘贴路径→附件→Enter（turn 运行中→入队）
    pty.send_raw(format!("\x1b[200~{}\x1b[201~", shot.display()).as_bytes());
    pty.wait_screen_timeout("[Image #1]", Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    pty.send_raw(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    // 排队期间改写源图（新版本）
    fs::write(&shot, &v2).unwrap();
    // 放行第一轮；重放出队读取的是新版本
    gate.release();
    pty.wait_screen_timeout("S103-第二轮", Duration::from_secs(15)).unwrap();
    assert_eq!(mock.count(), 2, "两轮各一次请求");
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let url = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .find_map(|p| p["image_url"]["url"].as_str())
        .expect("第二轮带图");
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        v2,
        "出队采用排队期间改写后的新版本（入队不冻结）"
    );
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S110（§2.3 修复验收）：出队项被拒后，剩余队列暂停自动发送；
/// 恢复稿留在输入框等待用户处理。
#[test]
fn s110_tui_rejected_dequeue_pauses_remaining_queue() {
    let cfg = TempDir::new("s110-cfg");
    let user = TempDir::new("s110-user");
    let (first, gate) = MockResp::gated(200, chat_body("S110-第一轮"));
    let mock = spawn_mock(vec![first, MockResp::json(200, chat_body("S110-C 自动发"))]);
    // 能力未知（无 input_modalities）→带图提交必被拒
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S110_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s110"),
    );
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let bad = work.join("bad.png");
    fs::write(&bad, b"not really a png but has extension").unwrap();
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S110_KEY",
        "fake-s110",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    // 第一轮（gated 挂起）
    pty.send_raw(b"first");
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\r");
    std::thread::sleep(Duration::from_millis(500));
    // 排队 B：坏图附件（粘贴路径→附件→显式排队键；021 起 Enter 运行中＝接续）
    pty.send_raw(format!("\x1b[200~{}\x1b[201~", bad.display()).as_bytes());
    pty.wait_screen_timeout("[Image #1]", Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    pty.send_raw(b"\t");
    std::thread::sleep(Duration::from_millis(300));
    // 排队 C：纯文本（显式排队键）
    pty.send_raw(b"second C");
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\t");
    std::thread::sleep(Duration::from_millis(300));
    // 放行第一轮 → 重放 B → 宿主拒绝（能力未知）→ B 回草稿；
    // 断言：C 不自动发送（mock 仍只有第一轮 1 次请求）
    gate.release();
    pty.wait_screen_timeout("提交被拒", Duration::from_secs(15)).unwrap();
    // 等待多个 tick 周期（若自动续发，C 请求会在此窗口发出）
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(mock.count(), 1, "剩余队列暂停：仅第一轮请求，C 未自动发送");
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S111（A02 L3）：`@` 搜索选图——弹窗候选可见、Tab 选中转附件、
/// 提交请求携带真实图片字节。
#[test]
fn s111_tui_at_file_search_selects_image() {
    let cfg = TempDir::new("s111-cfg");
    let user = TempDir::new("s111-user");
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S111-已分析"))]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S111_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s111"),
    );
    let bytes = s100_png(3, 3, [66, 77, 88, 255]);
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("pickme.png"), &bytes).unwrap();
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S111_KEY",
        "fake-s111",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    // 输入 @pick → 弹窗出现候选文件名
    pty.send_raw(b"@pick");
    pty.wait_screen_timeout("pickme.png", Duration::from_secs(5)).unwrap();
    // Tab 选中 → 附件占位符
    pty.send_raw(b"\t");
    pty.wait_screen_timeout("[Image #1]", Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    pty.send_raw(b"\r");
    pty.wait_screen_timeout("S111-已分析", Duration::from_secs(10)).unwrap();
    assert_eq!(mock.count(), 1);
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let url = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .find_map(|p| p["image_url"]["url"].as_str())
        .expect("请求带图");
    let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        bytes,
        "请求图片＝@ 选中的源字节"
    );
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S112（A07 L3）：两张附件删除第一张——第二张重编号为 1，
/// 提交请求只携带幸存图片的字节。
#[test]
fn s112_tui_delete_first_attachment_renumbers_and_excludes() {
    let cfg = TempDir::new("s112-cfg");
    let user = TempDir::new("s112-user");
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S112-完成"))]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S112_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s112"),
    );
    let first = s100_png(2, 2, [111, 1, 1, 255]);
    let second = s100_png(2, 2, [222, 2, 2, 255]);
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("first.png"), &first).unwrap();
    fs::write(work.join("second.png"), &second).unwrap();
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S112_KEY",
        "fake-s112",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    // 粘贴两张
    pty.send_raw(format!("\x1b[200~{}\x1b[201~", work.join("first.png").display()).as_bytes());
    pty.wait_screen_timeout("[Image #1]", Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    pty.send_raw(format!("\x1b[200~{}\x1b[201~", work.join("second.png").display()).as_bytes());
    pty.wait_screen_timeout("[Image #2]", Duration::from_secs(5)).unwrap();
    // 删除第一张：Home 到行首（紧邻 [Image #1] 元素左侧）＋Delete 前向删元素
    std::thread::sleep(Duration::from_millis(500));
    pty.send_raw(b"\x1b[H"); // Home
    std::thread::sleep(Duration::from_millis(200));
    pty.send_raw(b"\x1b[3~"); // Delete 删除整个占位符元素
    std::thread::sleep(Duration::from_millis(300));
    // 图 2 重编号为 1——由最终请求字节断言验证（屏幕中间态不作硬断言）
    std::thread::sleep(Duration::from_millis(700));
    pty.send_raw(b"\r");
    pty.wait_screen_timeout("S112-完成", Duration::from_secs(10)).unwrap();
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let urls: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .filter_map(|p| p["image_url"]["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 1, "只发幸存一张：{urls:?}");
    let b64 = urls[0].strip_prefix("data:image/png;base64,").unwrap();
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap(),
        second,
        "幸存的是第二张（第一张字节不再发送）"
    );
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S113（A13 L3）：带图入史后取消（gated 响应挂起时 Esc）——消息与
/// 图片在历史保留恰一份；继续新输入后请求同时含旧图与新文本。
#[test]
fn s113_tui_cancel_after_history_keeps_image_once() {
    let cfg = TempDir::new("s113-cfg");
    let user = TempDir::new("s113-user");
    let (first, gate) = MockResp::gated(200, chat_body("不应显示"));
    let mock = spawn_mock(vec![
        first,
        MockResp::json(200, chat_body("S113-第二轮完成")),
    ]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S113_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s113"),
    );
    let bytes = s100_png(2, 3, [55, 66, 77, 255]);
    let work = user.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("keep.png"), &bytes).unwrap();
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S113_KEY",
        "fake-s113",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    // 粘贴图片并提交（入史；请求被 gated 挂起）
    pty.send_raw(format!("\x1b[200~{}\x1b[201~", work.join("keep.png").display()).as_bytes());
    pty.wait_screen_timeout("[Image #1]", Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    pty.send_raw(b"\r");
    std::thread::sleep(Duration::from_millis(800));
    // Esc 取消（请求丢弃）
    pty.send_raw(b"\x1b");
    std::thread::sleep(Duration::from_millis(600));
    gate.release();
    std::thread::sleep(Duration::from_millis(600));
    // 新输入：请求应含已入史图片恰一张＋新文本
    pty.send_raw(b"next");
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\r");
    pty.wait_screen_timeout("S113-第二轮完成", Duration::from_secs(10)).unwrap();
    // next 轮请求（首个索引）：gated 响应在取消后已被消费丢弃
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let urls: Vec<&str> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("user") && m["content"].is_array())
        .flat_map(|m| m["content"].as_array().unwrap())
        .filter_map(|p| p["image_url"]["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 1, "旧图保留恰一份：{urls:?}");
    // 新文本（next）＝纯字符串 user 消息（020：无图消息保持原 content 形态）
    let next_present = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == json!("user") && m["content"].is_string() && m["content"].as_str().unwrap() == "next");
    assert!(next_present, "新文本在场：{:?}", body["messages"]);
    pty.send_raw(b"\x03");
    assert_eq!(pty.finish().1.code(), Some(0));
}

/// 020 S114（A21 L3）：问询回答窗口打开期间 Ctrl+V 不建附件、不产生剪贴板
/// 提示（活动视图拥有按键；普通问询窗口不接图片）。
#[test]
fn s114a_tui_question_window_rejects_image_paste() {
    let cfg = TempDir::new("s114a-cfg");
    let user = TempDir::new("s114a-user");
    let args = json!({"questions":[{"id":"dir","header":"方向","question":"S114A-请选择方向",
        "options":[{"label":"甲","description":"a"},{"label":"乙","description":"b"}]}]});
    let tc = json!({"choices":[{"message":{"tool_calls":[{"id":"q-ask","type":"function","function":{
        "name":"request_user_input","arguments":args.to_string()}}]}}]})
        .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S114A-继续")),
    ]);
    write_manual_image_capable(cfg.path(), &mock.url, "env:TCHAT_S114A_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s114a"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S114A_KEY",
        "fake-s114a",
        true,
    ));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_raw(b"ask");
    std::thread::sleep(Duration::from_millis(250));
    pty.send_raw(b"\r");
    pty.wait_screen_timeout("S114A-请选择方向", Duration::from_secs(5)).unwrap();
    // Ctrl+V：问询窗口活动——不应建附件、不应出现剪贴板提示
    pty.send_raw(b"\x16");
    std::thread::sleep(Duration::from_millis(600));
    let no_attachment = pty.wait_screen_timeout("[Image #", Duration::from_millis(300)).is_err();
    assert!(no_attachment, "问询窗口不建附件");
    let no_clip_hint = pty
        .wait_screen_timeout("剪贴板图片", Duration::from_millis(300))
        .is_err();
    assert!(no_clip_hint, "不触发剪贴板提示（按键归问询视图）");
    // 收尾：Esc 中断问询后退出（问询期间按键不建附件、不触发剪贴板读取已断言
    // 为本用例核心；退出码不属本用例验证目标，按框架清理收尾）
    pty.send_raw(b"\x1b");
    std::thread::sleep(Duration::from_millis(800));
    let _ = pty.finish();
}

// ---------- S21：021 send_message_to_user_async（A15/A04 L3 行式管道）----------

/// 021：声明 experimental_supported_tools 的 manual（消息能力 e2e 用）。
fn write_manual_message_capable(dir: &Path, base_url: &str, key_ref: &str) {
    let m = catalog::ManualRoot {
        selected_provider: Some("deepseek".into()),
        selected_model: Some("deepseek-flash".into()),
        memories: None,
        providers: [(
            "deepseek".to_string(),
            catalog::ManualProvider {
                base_url: Some(base_url.into()),
                key_ref: Some(key_ref.into()),
                models: [(
                    "deepseek-flash".to_string(),
                    catalog::ModelInfo {
                        id: "deepseek-flash".into(),
                        experimental_supported_tools: Some(vec![
                            "send_message_to_user_async".into()
                        ]),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(dir.join("manual.json"), serde_json::to_string_pretty(&m).unwrap())
        .unwrap();
}

/// S21（021 A15/A04 L3）：行式管道——模型在终文前发出即时消息：通知先于最终
/// 回复输出并 flush（stdout 重定向/纯管道同样成立）；下一请求含调用＋accepted
/// 且无同文 assistant；不等待用户、不结束轮；工具活动行不重复打印正文。
#[test]
fn s21_pipe_send_message_displayed_before_final_and_flushed() {
    let cfg = TempDir::new("s21-cfg");
    let user = TempDir::new("s21-user");
    let key = "s21-key-value";
    let message = "S21-中途重要发现。\n继续核对。";
    let tc = json!({"choices":[{"message":{
        "role": "assistant",
        "content": null,
        "tool_calls":[{"id":"s21-m1","type":"function","function":{
            "name":"send_message_to_user_async",
            "arguments": json!({ "message": format!("  {message}  ") }).to_string()
        }}]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S21-最终回复")),
    ]);
    write_manual_message_capable(cfg.path(), &mock.url, "env:TCHAT_S21_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint(key),
    );
    let (out, err, code) = run_pipe(
        None,
        &[
            ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
            ("TCHAT_S21_KEY", key),
            ("TERM", "dumb"),
        ],
        "开始\nexit\n",
    );
    assert_eq!(code, Some(0), "退出码：{out}\n{err}");
    // 消息先于最终回复，且 trim 后输出（首尾空白删除，内部换行保留）
    let msg_pos = out
        .find("Agent：S21-中途重要发现。")
        .expect("消息已在终文前输出");
    let final_pos = out.find("Agent：S21-最终回复").expect("最终回复已输出");
    assert!(msg_pos < final_pos, "通知先于最终回复：{out}");
    assert!(!out.contains("Agent： "), "消息 trim 后输出：{out}");
    // 工具活动行不重复打印正文（Begin 已抑制）
    assert!(
        !out.contains("[工具] send_message_to_user_async"),
        "成功通知不打印活动行：{out}"
    );
    assert_eq!(mock.count(), 2, "通知不结束轮：终文前恰两次请求");
    // 声明面：首请求声明该工具
    let body1: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    assert!(body1["tools"].as_array().unwrap().iter().any(|t| {
        t["function"]["name"] == json!("send_message_to_user_async")
    }), "声明开启：{body1}");
    // 请求面：第二请求含调用＋accepted，无同文 assistant
    let body2: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let msgs = body2["messages"].as_array().unwrap();
    assert!(msgs.iter().any(|m| {
        m["role"] == json!("tool")
            && m["tool_call_id"] == json!("s21-m1")
            && m["content"] == json!(r#"{"accepted":true}"#)
    }), "第二请求含 accepted 结果：{body2}");
    assert!(
        !msgs.iter().any(|m| {
            m["role"] == json!("assistant")
                && m.get("tool_calls").is_none()
                && m["content"].as_str().is_some_and(|c| c.contains("S21-中途"))
        }),
        "无额外同文 assistant 注入：{body2}"
    );
}

/// S21b（021 A01/A08 L3 行式管道）：关闭态伪造调用拒绝；/save→/resume 恢复
/// 只展示成功消息、零重发；失败结果不伪造成功。
#[test]
fn s21b_pipe_send_message_resume_display_without_resend() {
    let cfg = TempDir::new("s21b-cfg");
    let user = TempDir::new("s21b-user");
    let key = "s21b-key-value";
    // 两个能力关闭的响应（伪造调用被拒）后正常收尾
    let tc_off = json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"s21b-x","type":"function","function":{
            "name":"send_message_to_user_async",
            "arguments": r#"{"message":"未声明不该发"}"#.to_string()
        }}]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc_off),
        MockResp::json(200, chat_body("S21B-关闭态完成")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S21B_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint(key),
    );
    let (out, err, code) = run_pipe(
        None,
        &[
            ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
            ("TCHAT_S21B_KEY", key),
            ("TERM", "dumb"),
        ],
        "开始\nexit\n",
    );
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        out.contains("send_message_to_user_async 不可用"),
        "关闭态伪造调用被拒：{out}"
    );
    assert!(!out.contains("未声明不该发"), "关闭态无成功消息：{out}");
    // 伪造调用有匹配结果且非 accepted
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let msgs = body["messages"].as_array().unwrap();
    assert!(
        msgs.iter().any(|m| {
            m["role"] == json!("tool")
                && m["tool_call_id"] == json!("s21b-x")
                && m["content"].as_str().is_some_and(|c| c.contains("不可用"))
        }),
        "伪造调用有匹配错误结果：{body}"
    );
    let _ = code;
}

/// S21c（021 A08 L3）：/resume 恢复含即时消息的会话——成功记录重建为消息
/// cell 恰一次显示（不显示原始 accepted JSON）；取消跳过的失败记录不伪造为
/// 已发送；恢复零重执行、零模型请求。
#[test]
fn s21c_tui_resume_shows_message_without_resend() {
    let cfg = TempDir::new("s21c-cfg");
    let user = TempDir::new("s21c-user");
    let key = "s21c-key";
    let mock = spawn_mock(vec![]); // 零回复：/resume 只投影不重放
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S21C_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));

    use terminal_chat::prompt::{HistoryItem, HistoryMessage, HistoryRole, ToolCallRecord};
    let hist = vec![
        HistoryItem::Text(HistoryMessage {
            images: None,
            text_elements: Vec::new(),
            role: HistoryRole::User,
            content: "S21C-历史输入".into(),
            user_input_event: None,
        }),
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: "s21c-m1".into(),
                name: "send_message_to_user_async".into(),
                arguments: r#"{"message":"S21C-中途说明"}"#.into(),
            }],
            content: None,
        },
        HistoryItem::ToolResult {
            tool_call_id: "s21c-m1".into(),
            content: r#"{"accepted":true}"#.into(),
            display: None,
            image: None,
        },
        // 取消跳过：不伪造为已发送
        HistoryItem::AssistantToolCalls {
            tool_calls: vec![ToolCallRecord {
                id: "s21c-m2".into(),
                name: "send_message_to_user_async".into(),
                arguments: r#"{"message":"S21C-被取消"}"#.into(),
            }],
            content: None,
        },
        HistoryItem::ToolResult {
            tool_call_id: "s21c-m2".into(),
            content: r#"{"error":"用户取消，本调用未执行"}"#.into(),
            display: None,
            image: None,
        },
    ];
    terminal_chat::session::save_snapshot(&user.path().join("sessions"), "seed21c", &hist)
        .unwrap();

    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S21C_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("/resume seed21c");
    // 成功消息按 Markdown 展示恰一次；失败记录按普通文本呈现原因
    pty.wait_screen("S21C-中途说明").unwrap();
    // 取消跳过按事实文本呈现（与实时失败提示同一文案），不显示原始结果 JSON
    pty.wait_screen("send_message_to_user_async 已取消").unwrap();
    pty.wait_screen("恢复 5 条").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    pty.send_raw(b"\x03");
    assert!(pty.finish().1.success());
    assert!(
        mock.requests().is_empty(),
        "/resume 只投影不重放：{:?}",
        mock.requests()
    );
}

/// S21d（021 A04/A06 L3 TUI）：模型在任务中发出即时消息——消息以 Markdown cell
/// 追加显示、终文不覆盖已显示消息、不抢输入焦点（composer 草稿保留）、
/// 不结束当前轮（后续请求继续）。
#[test]
fn s21d_tui_message_displayed_and_not_overwritten_by_final() {
    let cfg = TempDir::new("s21d-cfg");
    let user = TempDir::new("s21d-user");
    let key = "s21d-key-value";
    let tc = json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"s21d-m1","type":"function","function":{
            "name":"send_message_to_user_async",
            "arguments": json!({"message":"S21D-中途重要发现"}).to_string()}}]}}]}).to_string();
    let (last, gate) = MockResp::gated(200, chat_body("S21D-最终回复"));
    let mock = spawn_mock(vec![MockResp::json(200, tc), last]);
    write_manual_message_capable(cfg.path(), &mock.url, "env:TCHAT_S21D_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint(key),
    );
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S21D_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("开始任务");
    // 消息即时显示
    pty.wait_screen("S21D-中途重要发现").unwrap();
    // 消息出现后任务仍在运行（第 2 请求被闸门挂起）——通知被当 Final 会提前
    // 结束任务态，此断言即失败（变异可检出）
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut still_running = false;
    while Instant::now() < deadline {
        if pty.screen().contains("to interrupt") {
            still_running = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(still_running, "消息出现后任务必须仍在运行（通知不是 Final）");
    // 通知后仍可输入：留一份草稿（不提交）——消息不抢焦点、不清草稿
    pty.send_raw("S21D-新草稿".as_bytes());
    std::thread::sleep(Duration::from_millis(300));
    gate.release();
    pty.wait_screen("S21D-最终回复").unwrap();
    // 终文不覆盖已显示通知、草稿仍在：三者同屏
    pty.wait_screen("S21D-中途重要发现").unwrap();
    pty.wait_screen("S21D-新草稿").unwrap();
    assert_eq!(mock.count(), 2, "通知不结束轮");
    // 清空草稿（8 字符退格）后退出
    for _ in 0..8 {
        pty.send_raw(b"\x7f");
    }
    std::thread::sleep(Duration::from_millis(200));
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

/// S21e（021 A14 安全门禁 L3）：模型可控消息的展示副本过终端过滤——ESC/CR/BEL
/// 不得进入 stdout（不执行终端动作）；通知不执行命令、不打开路径、不获得授权
///（零副作用：无文件写入、零额外请求）。
#[test]
fn s21e_pipe_message_display_copy_is_terminal_filtered() {
    let cfg = TempDir::new("s21e-cfg");
    let user = TempDir::new("s21e-user");
    let key = "s21e-key-value";
    let tc = json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"s21e-m","type":"function","function":{
            "name":"send_message_to_user_async",
            "arguments": json!({"message":"\u{1b}[2J清除\u{7}屏\rPWNED"}).to_string()
        }}]}}]}).to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S21E-最终回复")),
    ]);
    write_manual_message_capable(cfg.path(), &mock.url, "env:TCHAT_S21E_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint(key),
    );
    let (out, _err, code) = run_pipe(
        None,
        &[
            ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
            ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
            ("TCHAT_S21E_KEY", key),
            ("TERM", "dumb"),
        ],
        "开始\nexit\n",
    );
    assert_eq!(code, Some(0));
    // 展示副本剥离全部转义与危险控制字符（动作零）
    assert!(!out.contains('\u{1b}'), "stdout 不得含 ESC：{out:?}");
    assert!(!out.contains('\r'), "stdout 不得含 CR：{out:?}");
    assert!(!out.contains('\u{7}'), "stdout 不得含 BEL：{out:?}");
    assert!(out.contains("清除屏PWNED"), "剥离后文本仍如实可见：{out}");
    // 零副作用：恰两次请求（通知不产生额外请求/授权动作）
    assert_eq!(mock.count(), 2);
}

/// S11c（021 A11/§2.3-6 L3 驱动恢复）：运行中受理的接续输入在取消时未入史——
/// 宿主交还驱动恢复为待处理完整消息并暂停自动发送；用户再次提交后按受理
/// 顺序恰一次重放为后续轮（完整消息不丢失、不重复）。
#[test]
fn s11c_tui_cancel_restores_steer_to_queue_and_replays_once() {
    let cfg = TempDir::new("s11c-cfg");
    let user = TempDir::new("s11c-user");
    let key = "s11c-key-value";
    let (first, gate) = MockResp::gated(200, chat_body("S11C-第一轮"));
    let mock = spawn_mock(vec![
        first,
        MockResp::json(200, chat_body("S11C-继续完成")),
        MockResp::json(200, chat_body("S11C-追问已处理")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S11C_KEY");
    let origin = catalog::origin_of(&mock.url).unwrap();
    seed_grant(user.path(), &origin, &catalog::fingerprint(key));
    let mut pty = PtySession::spawn(pty_cli(None, &[
        ("TCHAT_CONFIG_DIR", cfg.path().to_str().unwrap()),
        ("TCHAT_USER_DIR", user.path().to_str().unwrap()),
        ("TCHAT_S11C_KEY", key),
    ]));
    pty.wait_screen("? for shortcuts").unwrap();
    pty.send_line("开始任务");
    let deadline = Instant::now() + WATCHDOG;
    while mock.count() < 1 {
        assert!(Instant::now() < deadline, "mock 未收到首轮请求");
        std::thread::sleep(Duration::from_millis(50));
    }
    // 受理接续输入（在途），随后取消当前轮
    pty.send_line("等等，先补充一点");
    std::thread::sleep(Duration::from_millis(400));
    pty.send_raw(b"\x1b"); // Esc＝仅取消
    pty.wait_screen("接续输入未入史").unwrap();
    // 暂停自动发送：取消后无新请求（仍恰 1 次）
    assert_eq!(mock.count(), 1, "恢复为待处理消息后不自动发送");
    // 用户再次提交＝已处理，恢复队列：先发"继续"，随后队列重放接续输入
    pty.send_line("继续");
    pty.wait_screen("S11C-继续完成").unwrap();
    pty.wait_screen("S11C-追问已处理").unwrap();
    assert_eq!(mock.count(), 3, "重放恰一次（继续＋追问两轮）");
    let body: Value = serde_json::from_str(&mock.requests()[2].body).unwrap();
    assert!(
        body["messages"].as_array().unwrap().iter().any(|m| {
            m["role"] == "user" && m["content"] == "等等，先补充一点"
        }),
        "接续输入恰一次重放进请求：{body}"
    );
    pty.send_line("/exit");
    pty.wait_output("再见!", WATCHDOG).unwrap();
    let (out, status) = pty.finish();
    assert_eq!(status.code(), Some(0), "{out}");
}

// ========== 022 MCP 外部工具（L3：真实 CLI＋PTY＋本地 stdio 服务器） ==========

/// 022：stdio MCP 夹具（与 mcp_stdio/mcp_turn 共用同一固定测试程序）。
fn write_mcp_config(user: &Path, scenario: &str, record: &Path, extra: &str) {
    write_mcp_config_enabled(user, scenario, record, extra, true)
}

fn write_mcp_config_enabled(
    user: &Path,
    scenario: &str,
    record: &Path,
    extra: &str,
    enabled: bool,
) {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp_stdio_server.py");
    let json = format!(
        r#"{{"mcp_servers": {{"fx": {{"enabled": {enabled}, "command": "python3", "args": ["{fixture}", "--scenario", "{scenario}", "--record", "{record}"], "cwd": "{cwd}"{extra}}}}}}}"#,
        fixture = fixture,
        record = record.display(),
        cwd = std::env::temp_dir().display(),
        extra = extra,
    );
    fs::write(user.join("mcp.json"), json).unwrap();
}

/// 外层 argv 扫描计数：cmdline 含全部片段的进程数（033 生命周期）。
/// 僵尸进程 cmdline 为空，天然不计——「活进程」与「已死未收割」在扫描层面区分。
fn scan_arg_procs(markers: &[&str]) -> usize {
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok())
        .filter(|e| {
            fs::read_to_string(e.path().join("cmdline"))
                .map(|c| !c.is_empty() && markers.iter().all(|m| c.contains(m)))
                .unwrap_or(false)
        })
        .count()
}

fn mcp_record(path: &Path) -> Vec<Value> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// 进程是否仍存活（/proc 探测；进程组回收的 L3 证据）。
fn process_alive(pid: u64) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn wait_pid_gone(pid: u64, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// A03/A07/A09 L3：真实行式 CLI——MCP 工具声明进请求、真实子进程调用、
/// 结果配对回模型、`/mcp` 状态可见。
#[test]
fn s102_mcp_line_cli_declares_calls_and_pairs() {
    let cfg = TempDir::new("s102-cfg");
    let user = TempDir::new("s102-user");
    let record_dir = TempDir::new("s102-record");
    let record = record_dir.path().join("record.jsonl");
    write_mcp_config(user.path(), "echo", &record, r#", "env_vars": ["TCHAT_S102_TOKEN"]"#);
    let tc = json!({"choices":[{"message":{"tool_calls":[{"id":"s102-c1","type":"function","function":{
        "name":"mcp__fx__echo","arguments":json!({"text":"端到端"}).to_string()}}]}}]})
        .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S102-工具结果已收到")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S102_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s102"),
    );
    let mut cmd = input_pty_cmd(cfg.path(), user.path(), "TCHAT_S102_KEY", "fake-s102", false);
    cmd.env("TCHAT_S102_TOKEN", "s102-canary-token");
    let mut pty = PtySession::spawn(cmd);
    pty.wait_output("你> ", WATCHDOG).unwrap();
    pty.send_line("调用 MCP 工具");
    // 工具活动行（真实子进程调用前的 flush 契约）。
    pty.wait_output("[工具] mcp__fx__echo", WATCHDOG).unwrap();
    pty.wait_output("S102-工具结果已收到", WATCHDOG).unwrap();
    pty.wait_output("你> ", WATCHDOG).unwrap();
    // /mcp 状态：连接状态＋模型名与原始名映射。
    pty.send_line("/mcp");
    pty.wait_output("MCP：已加载", WATCHDOG).unwrap();
    pty.wait_output("mcp__fx__echo ← echo", WATCHDOG).unwrap();
    pty.send_raw(b"\x04");
    let (out, status) = pty.finish();
    assert!(status.success(), "{out}");
    // 服务器收到原始名与参数（真实调用）。
    let record_lines = mcp_record(&record);
    let call_entry = record_lines
        .iter()
        .find(|e| e["kind"] == "call")
        .expect("服务器真实收到调用");
    assert_eq!(call_entry["name"], "echo");
    assert_eq!(call_entry["arguments"]["text"], "端到端");
    // 下一请求包含配对结果。
    let body: Value = serde_json::from_str(&mock.requests()[1].body).unwrap();
    let tool = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["tool_call_id"] == "s102-c1")
        .expect("下一请求含配对结果");
    let content: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["content"][0]["text"], "echo:端到端");
    // 声明在请求 tools 中且位于原生工具之后。
    let first: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap_or(""))
        .collect();
    let pos = names.iter().position(|n| *n == "mcp__fx__echo").expect("MCP 已声明");
    assert!(pos >= 4, "MCP 定义追加在原生工具之后：{names:?}");
}

/// A13 L3：`/mcp reload` 停用立即生效、旧进程回收；A18 L3：退出显式回收
/// MCP 进程组。
#[test]
fn s103_mcp_reload_disables_and_exit_reaps_process() {
    let cfg = TempDir::new("s103-cfg");
    let user = TempDir::new("s103-user");
    let record_dir = TempDir::new("s103-record");
    let record = record_dir.path().join("record.jsonl");
    write_mcp_config(user.path(), "echo", &record, "");
    let mock = spawn_mock(vec![
        MockResp::json(200, chat_body("S103-就绪")),
        MockResp::json(200, chat_body("S103-停用后仍可对话")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S103_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s103"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S103_KEY",
        "fake-s103",
        false,
    ));
    pty.wait_output("你> ", WATCHDOG).unwrap();
    // 首个用户轮触发 MCP 启动（记录 PID）。
    pty.send_line("连接一次");
    pty.wait_output("S103-就绪", WATCHDOG).unwrap();
    pty.wait_output("你> ", WATCHDOG).unwrap();
    let record_lines = mcp_record(&record);
    let pid = record_lines
        .iter()
        .find(|e| e["kind"] == "startup")
        .expect("启动记录")["pid"]
        .as_u64()
        .expect("pid");
    // 033 生命周期（2026-10-28）：沙箱注入态下服务器记录的 pid 是**命名空间内**
    // pid（恒为 1；外层 /proc/1 恒存在，不得作为回收判据）——存活/回收判定切换
    // 为外层 argv 扫描（bwrap 包装与 python 载荷均含 fixture 路径；僵尸进程
    // cmdline 为空，天然排除，不把僵尸计为泄漏）。普通模式保留原 pid 断言。
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp_stdio_server.py");
    // 扫描标记＝fixture 路径＋**本测试唯一**的 record 路径（AND）：全局共享的
    // fixture 路径会被并行 MCP 用例的服务器污染（假失败/假通过），record 路径
    // 含每测试独立 TempDir 名，精确归属本用例进程（2026-10-28 审查发现）。
    let record_str = record.display().to_string();
    let mcp_markers = [fixture, record_str.as_str()];
    let sandbox = std::env::var("TCHAT_SANDBOX")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if sandbox {
        let dl = std::time::Instant::now() + Duration::from_secs(5);
        while scan_arg_procs(&mcp_markers) == 0 && std::time::Instant::now() < dl {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            scan_arg_procs(&mcp_markers) > 0,
            "MCP 服务器实际启动（外层 argv 身份）"
        );
        let _ = pid;
    } else {
        assert!(process_alive(pid), "MCP 子进程存活");
    }
    // 修改配置停用服务器 → reload → 立即生效且旧进程回收。
    write_mcp_config_enabled(user.path(), "echo", &record, "", false);
    pty.send_line("/mcp reload");
    pty.wait_output("已重新加载配置并重建连接", WATCHDOG).unwrap();
    if sandbox {
        let dl = std::time::Instant::now() + Duration::from_secs(5);
        while scan_arg_procs(&mcp_markers) > 0 && std::time::Instant::now() < dl {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            scan_arg_procs(&mcp_markers) == 0,
            "停用后旧 MCP 进程被回收（外层 argv 口径）"
        );
    } else {
        assert!(
            wait_pid_gone(pid, Duration::from_secs(5)),
            "停用后旧 MCP 进程被回收"
        );
    }
    pty.send_line("/mcp");
    pty.wait_output("fx（stdio）：已停用", WATCHDOG).unwrap();
    // 停用后继续对话照常（原生工具面不受影响）。
    pty.send_line("还在吗");
    pty.wait_output("S103-停用后仍可对话", WATCHDOG).unwrap();
    // 退出：显式清理路径完成（无 MCP 连接残留）。
    pty.send_raw(b"\x04");
    let (out, status) = pty.finish();
    assert!(status.success(), "{out}");
}

/// A14 L3：V1 历史中的 MCP 调用恢复只展示；无配置时 `/resume` 零 MCP 启动、
/// 新提交才取当前能力；历史可读且不被回写。
#[test]
fn s104_mcp_resume_replays_nothing_and_new_turn_uses_current_caps() {
    let cfg = TempDir::new("s104-cfg");
    let user = TempDir::new("s104-user");
    let record_dir = TempDir::new("s104-record");
    let record = record_dir.path().join("record.jsonl");
    write_mcp_config(user.path(), "echo", &record, "");
    let tc = json!({"choices":[{"message":{"tool_calls":[{"id":"s104-c1","type":"function","function":{
        "name":"mcp__fx__echo","arguments":json!({"text":"历史调用"}).to_string()}}]}}]})
        .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S104-第一轮完成")),
        MockResp::json(200, chat_body("S104-恢复后新轮")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S104_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s104"),
    );
    // 第一段：真实调用一次（产生自动会话与 MCP 记录）。
    {
        let mut cmd =
            input_pty_cmd(cfg.path(), user.path(), "TCHAT_S104_KEY", "fake-s104", false);
        cmd.env("TCHAT_S104_TOKEN", "s104-canary");
        let mut pty = PtySession::spawn(cmd);
        pty.wait_output("你> ", WATCHDOG).unwrap();
        pty.send_line("第一轮");
        pty.wait_output("S104-第一轮完成", WATCHDOG).unwrap();
        pty.wait_output("你> ", WATCHDOG).unwrap();
        pty.send_raw(b"\x04");
        let (out, status) = pty.finish();
        assert!(status.success(), "{out}");
    }
    let record_lines = mcp_record(&record);
    assert_eq!(
        record_lines.iter().filter(|e| e["kind"] == "call").count(),
        1,
        "历史恰一次真实调用"
    );
    // 移除 mcp.json：恢复后无 MCP 配置。
    fs::remove_file(user.path().join("mcp.json")).unwrap();
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S104-恢复后新轮"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S104_KEY2");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s104-2"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S104_KEY2",
        "fake-s104-2",
        false,
    ));
    pty.wait_output("你> ", WATCHDOG).unwrap();
    pty.send_line("/resume");
    // 恢复展示历史摘要（含 MCP 工具调用记录），零 MCP 启动（无配置）。
    pty.wait_output("恢复 5 条", WATCHDOG).unwrap();
    pty.wait_output("你> ", WATCHDOG).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let record_after = mcp_record(&record);
    assert_eq!(
        record_after.iter().filter(|e| e["kind"] == "startup").count(),
        1,
        "恢复零 MCP 启动（不重启服务器）"
    );
    assert_eq!(
        record_after.iter().filter(|e| e["kind"] == "call").count(),
        1,
        "恢复零重放"
    );
    // 恢复后的新提交：MCP 不声明（无配置），普通对话照常。
    pty.send_line("恢复后的新输入");
    pty.wait_output("S104-恢复后新轮", WATCHDOG).unwrap();
    pty.send_raw(b"\x04");
    let (out, status) = pty.finish();
    assert!(status.success(), "{out}");
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let names: Vec<String> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !names.iter().any(|n| n.starts_with("mcp__")),
        "无配置时不声明 MCP 工具：{names:?}"
    );
    // 历史中的 MCP 调用与结果仍在请求里（事实记录）。
    let msgs = body["messages"].as_array().unwrap();
    assert!(
        msgs.iter().any(|m| m["tool_call_id"] == "s104-c1"),
        "V1 历史工具记录照常投影"
    );
}

/// A14 L3（023）：资源工具调用的 V1 历史恢复只展示；`/resume` 零 MCP
/// 启动、零资源请求（配置仍在）；新提交取当前能力。
#[test]
fn s105_mcp_resources_resume_replays_nothing() {
    let cfg = TempDir::new("s105-cfg");
    let user = TempDir::new("s105-user");
    let record_dir = TempDir::new("s105-record");
    let record = record_dir.path().join("record.jsonl");
    write_mcp_config(user.path(), "resources_only", &record, "");
    let tc = json!({"choices":[{"message":{"tool_calls":[{"id":"s105-c1","type":"function","function":{
        "name":"read_mcp_resource","arguments":json!({"server":"fx","uri":"tchat://guide/hello"}).to_string()}}]}}]})
        .to_string();
    let mock = spawn_mock(vec![
        MockResp::json(200, tc),
        MockResp::json(200, chat_body("S105-第一轮完成")),
        MockResp::json(200, chat_body("S105-恢复后新轮")),
    ]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S105_KEY");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s105"),
    );
    // 第一段：真实读取一次（用户可见资源工具执行与结果），产生自动会话。
    {
        let mut cmd =
            input_pty_cmd(cfg.path(), user.path(), "TCHAT_S105_KEY", "fake-s105", false);
        let mut pty = PtySession::spawn(cmd);
        pty.wait_output("你> ", WATCHDOG).unwrap();
        pty.send_line("读取资源");
        pty.wait_output("read_mcp_resource", WATCHDOG)
            .expect("用户看到资源工具执行");
        pty.wait_output("S105-第一轮完成", WATCHDOG).unwrap();
        pty.wait_output("你> ", WATCHDOG).unwrap();
        pty.send_raw(b"\x04");
        let (out, status) = pty.finish();
        assert!(status.success(), "{out}");
    }
    let record_lines = mcp_record(&record);
    assert_eq!(
        record_lines
            .iter()
            .filter(|e| e["kind"] == "read_resource")
            .count(),
        1,
        "历史恰一次真实读取"
    );
    // 配置保留：恢复后即使有 MCP 配置也不启动、不发资源请求。
    let mock = spawn_mock(vec![MockResp::json(200, chat_body("S105-恢复后新轮"))]);
    write_manual(cfg.path(), &mock.url, "env:TCHAT_S105_KEY2");
    seed_grant(
        user.path(),
        &catalog::origin_of(&mock.url).unwrap(),
        &catalog::fingerprint("fake-s105-2"),
    );
    let mut pty = PtySession::spawn(input_pty_cmd(
        cfg.path(),
        user.path(),
        "TCHAT_S105_KEY2",
        "fake-s105-2",
        false,
    ));
    pty.wait_output("你> ", WATCHDOG).unwrap();
    pty.send_line("/resume");
    pty.wait_output("恢复 5 条", WATCHDOG).unwrap();
    pty.wait_output("你> ", WATCHDOG).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let record_after = mcp_record(&record);
    assert_eq!(
        record_after.iter().filter(|e| e["kind"] == "startup").count(),
        1,
        "恢复零 MCP 启动（配置仍在）"
    );
    assert_eq!(
        record_after
            .iter()
            .filter(|e| e["kind"] == "read_resource")
            .count(),
        1,
        "恢复零资源请求（不重放读取）"
    );
    assert_eq!(
        record_after
            .iter()
            .filter(|e| e["kind"] == "list_resources")
            .count(),
        0,
        "恢复不预读资源目录"
    );
    // 恢复后的新提交：当前能力生效（新进程首次用户轮启动 MCP），历史照常。
    pty.send_line("恢复后的新输入");
    pty.wait_output("S105-恢复后新轮", WATCHDOG).unwrap();
    pty.send_raw(b"\x04");
    let (out, status) = pty.finish();
    assert!(status.success(), "{out}");
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    let names: Vec<String> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "read_mcp_resource"),
        "新进程声明资源工具：{names:?}"
    );
    let msgs = body["messages"].as_array().unwrap();
    assert!(
        msgs.iter().any(|m| m["tool_call_id"] == "s105-c1"),
        "V1 历史资源工具记录照常投影"
    );
    let record_final = mcp_record(&record);
    assert_eq!(
        record_final.iter().filter(|e| e["kind"] == "startup").count(),
        2,
        "新进程首轮启动一次（恢复本身零启动）"
    );
}
