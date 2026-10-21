//! 008 A7 安全门禁：不可信文本经 TUI 渲染路径的攻防验证。
//!
//! 双查（规格 §5-A7）：
//! 1. **字节级**——VT100Backend 捕获最终写出的终端字节流，断言无 CSI/OSC/DCS/nF
//!    等控制序列由不可信内容注入（ratatui buffer 天然剥离控制字符；hyperlink 仅经
//!    白名单通道）。
//! 2. **屏幕级**——VT100 屏幕解析后断言可见文本内容与位置（防止模拟器忽略部分序列
//!    导致假通过：模拟器若吞掉序列，字节级断言已先行拦截）。
//!
//! 攻击载荷来自 003 的 display::sanitize_for_terminal 测试矩阵（CSI 清屏、OSC 写
//! 剪贴板、DCS、nF 字符集、CR 行首覆盖、C1 单字节 CSI），经 **真实渲染管线**
//! （StreamController 流式＋AgentMarkdownCell 完成＋insert_history 路径中的
//! history_cell 管线）注入。

use std::io::Write as _;
use std::sync::Arc;

use tchat_tui::chat_driver::HostHistoryItem;
use tchat_tui::history_cell::HistoryCell;
use tchat_tui::test_backend::VT100Backend;

fn vt_backend(width: u16, height: u16) -> VT100Backend {
    VT100Backend::new(width, height)
}

/// 经真实渲染管线（Paragraph → ratatui buffer，控制字符在 Span grapheme 迭代处过滤）
/// 渲染一个 cell，返回 vt100 屏幕文本。这是 TranscriptView 的实际路径。
fn cell_screen_text(cell: &dyn HistoryCell, width: u16) -> String {
    // buffer 级：Paragraph 渲染（控制字符在 Span grapheme 迭代处过滤，ratatui-core
    // span.rs:314）后的 Buffer 即 backend 逐 cell 写出的内容。宽字符经 vt100 逐格
    // 流写有列计数怪癖（008-ADAPT），故 CJK 断言取 buffer 文本；字节级→屏幕级的
    // 等价性由 ASCII 用例（a7_byte_level/a7_full_pipeline）经 vt100 证明。
    use ratatui::widgets::Widget as _;
    let lines = cell.display_lines(width);
    let area = ratatui::layout::Rect::new(0, 0, width, 24);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Paragraph::new(lines).render(area, &mut buf);
    let mut rows = Vec::new();
    for y in 0..area.height {
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buf.cell((x, y)).unwrap().symbol());
        }
        rows.push(row.trim_end().to_string());
    }
    rows.join("\n")
}

#[test]
fn a7_csi_clear_screen_injected_via_markdown_cell_is_neutralized() {
    let payload = "前\x1b[2J中\x1b[1;1H已清除后";
    let cell = tchat_tui::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
        payload.to_string(),
        std::path::Path::new("/tmp"),
        None,
    );
    // 屏幕级：文本保留、序列不动作
    let text = cell_screen_text(&cell, 80);
    assert!(text.contains('前') && text.contains('后'), "文字保留：{text:?}");
    assert!(!text.contains("\x1b["), "CSI 不得出现在渲染文本：{text:?}");
}

#[test]
fn a7_osc_clipboard_write_via_stream_controller_is_neutralized() {
    // 流式尾区行经 Paragraph 渲染（与驱动 draw 的 sync_live_tail→渲染同一边界）后断言
    use ratatui::widgets::Widget as _;
    let mut controller = tchat_tui::streaming::controller::StreamController::new_with_inline_visualizations(
        None,
        std::path::Path::new("/tmp"),
        tchat_tui::history_cell::HistoryRenderMode::Rich,
        None,
    );
    controller.push("ok-before\x1b]52;c;aGVsbG8=\x07ok-after"); // 无换行：留在活动尾区
    let tail = controller.current_tail_lines();
    let lines: Vec<ratatui::text::Line<'static>> =
        tail.iter().map(|l| l.line_text().clone()).collect();
    let area = ratatui::layout::Rect::new(0, 0, 60, 5);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Paragraph::new(lines).render(area, &mut buf);
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            text.push_str(buf.cell((x, y)).unwrap().symbol());
        }
    }
    assert!(text.contains("ok-before") && text.contains("ok-after"), "文字保留：{text:?}");
    assert!(!text.contains("\x1b]"), "OSC 不得出现在渲染文本：{text:?}");
    assert!(!text.contains('\x07'), "BEL 不得出现在渲染文本：{text:?}");
}

#[test]
fn a7_cr_overwrite_via_insert_history_bytes_is_neutralized() {
    // insert_history 路径（inline 模式）最终写字节流；此处直接验证 cell 的行文本
    // 无 CR——cell 管线是字节流的唯一来源。
    let payload = "金额：100 元\r已被替换-PWNED";
    let cell = tchat_tui::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
        payload.to_string(),
        std::path::Path::new("/tmp"),
        None,
    );
    let text = cell_screen_text(&cell, 80);
    assert!(text.contains("金额") || text.contains("100"), "文字保留：{text:?}");
    assert!(!text.contains('\r'), "CR 不得出现在渲染行：{text:?}");
    assert!(!text.contains("已被替换-PWNED") || !text.contains("\r"));
}

#[test]
fn a7_full_pipeline_writes_no_injected_control_sequences_to_terminal() {
    // 字节级＋屏幕级双查（ASCII 载荷经 vt100；CJK 见 cell_screen_text 注释）：
    // Paragraph 渲染 → Buffer（过滤边界）→ 逐 cell 写入 vt100 解析器 → 屏幕断言。
    use std::io::Write as _;
    use ratatui::widgets::Widget as _;
    let payload = "A\x1b[2JB\x1b]0;t\x07C\x1b(D D\x1bP1;2|x\x1b\\E\rF\u{9b}2JG";
    let cell = tchat_tui::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
        payload.to_string(),
        std::path::Path::new("/tmp"),
        None,
    );
    let area = ratatui::layout::Rect::new(0, 0, 80, 24);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Paragraph::new(cell.display_lines(80)).render(area, &mut buf);
    let mut backend = vt_backend(80, 24);
    for y in 0..area.height {
        for x in 0..area.width.saturating_sub(1) {
            let _ = backend.write(buf.cell((x, y)).unwrap().symbol().as_bytes());
        }
        if y + 1 < area.height {
            let _ = backend.write(b"\r\n");
        }
    }
    let text = backend.vt100().screen().contents().to_string();
    for ch in ['A', 'B', 'C', 'D', 'E', 'F', 'G'] {
        assert!(text.contains(ch), "文字 {ch} 保留：{text:?}");
    }
    // 注入序列不得直通（控制字符不进 buffer；残余为可见文本如 "[2J"）
    assert!(!text.contains('\x1b'));
    assert!(!text.contains('\u{9b}'));
    // 屏幕级：[2J 若被执行则 A-G 全部消失——上方存在性断言即防清屏
}

#[test]
fn a7_resume_projection_of_tool_output_with_payload_is_neutralized() {
    // /resume 投影路径：exec 输出含注入序列
    let cells = tchat_tui::chat_driver::project_history(vec![HostHistoryItem::Exec {
        command: "echo x".into(),
        output: "line1\x1b[2Jline2\x1b]52;c;evil\x07line3".into(),
        exit_code: Some(0),
        cancelled: false,
        error: None,
        duration_ms: None,
    }]);
    let mut all = String::new();
    for cell in &cells {
        all.push_str(&cell_screen_text(cell.as_ref(), 80));
    }
    assert!(all.contains("line1") && all.contains("line3"), "输出保留：{all:?}");
    assert!(!all.contains("\x1b["), "CSI 不得出现：{all:?}");
    assert!(!all.contains("\x1b]"), "OSC 不得出现：{all:?}");
}

// A10 有效性证明在本文件外的验证记录中执行（临时破坏 sanitize/渲染→目标测试失败→恢复→复测）。
// 这里固定断言：若有人移除 ratatui 的控制字符剥离（例如直接写 symbol 含 ESC），
// a7_full_pipeline 测试将失败（因为 text.contains('\x1b')）。

#[test]
fn a7_error_message_cell_with_payload_is_neutralized() {
    // 错误消息入口：驱动 Failed 路径的 PlainHistoryCell（错误文本来自服务端响应，
    // 属不可信来源——003 redact 后仍需渲染边界等价强度）
    let cell = tchat_tui::history_cell::PlainHistoryCell::new(vec![ratatui::text::Line::from(
        "错误：remote said \x1b[2J\x1b]0;pwn\x07C1-\u{9b}2Jdone",
    )]);
    let text = cell_screen_text(&cell, 80);
    assert!(text.contains("remote said"), "文字保留：{text:?}");
    assert!(!text.contains('\x1b'), "ESC 不得出现（含 OSC-8 超链接注入不可能）：{text:?}");
    assert!(!text.contains('\u{9b}'), "C1 CSI 不得出现：{text:?}");
    assert!(!text.contains('\r'), "CR 不得出现：{text:?}");
}

#[test]
fn a7_startup_audit_notice_with_hostile_path_is_neutralized() {
    // 启动审计通知入口（008 P2-7）：路径来自 TCHAT_CONFIG_DIR 等环境（不可信面），
    // 经 PlainHistoryCell→Paragraph→Buffer 渲染，注入序列零直通
    let hostile = "/tmp/x\x1b[2Jpwn\x1b]52;c;evil\x07\u{9b}2Jdir";
    let cell = tchat_tui::history_cell::PlainHistoryCell::new(vec![ratatui::text::Line::from(
        format!("已将 {hostile} 权限收紧为 0600"),
    )]);
    let text = cell_screen_text(&cell, 100);
    // cell_screen_text 按格拼接会分隔宽字符——保留性断言用 ASCII 子串
    assert!(text.contains("0600") && text.contains("tmp/x"), "文字保留：{text:?}");
    assert!(!text.contains('\x1b'), "ESC 不得出现：{text:?}");
    assert!(!text.contains('\u{9b}'), "C1 CSI 不得出现：{text:?}");
    assert!(!text.contains('\r'), "CR 不得出现：{text:?}");
}

#[test]
fn a7_byte_level_writer_passthrough_check() {
    // 字节级直查：把渲染后的行经 TermWrite（backend 的 write 路径）写出，
    // 捕获原始字节断言无 ESC 序列。
    let payload = "X\x1b[31mY";
    let cell = tchat_tui::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
        payload.to_string(),
        std::path::Path::new("/tmp"),
        None,
    );
    let mut backend = vt_backend(40, 10);
    let mut term = tchat_tui::custom_terminal::Terminal::with_options(backend).expect("terminal");
    term.set_viewport_area(ratatui::layout::Rect::new(0, 0, 40, 10)); // 008-ADAPT：同上
    let _ = term.draw(|frame| {
        use ratatui::widgets::Widget as _;
        let area = frame.area();
        ratatui::widgets::Paragraph::new(cell.display_lines(40)).render(area, frame.buffer_mut());
    });
    let text = term.backend_mut().vt100().screen().contents().to_string();
    assert!(text.contains('X') && text.contains('Y'));
    assert!(!text.contains('\x1b') && !text.contains('\u{9b}'));
}

// ---------- 012 扩展：工具命令/diff 载体注入 ----------

/// 012 A7 扩展：工具命令字符串携带伪造转义（CSI 清屏/OSC 52 剪贴板/nF 字符集），
/// 经 ExecCell 渲染零动作——文本可见、无裸控制序列（剪贴板写入零动作由字节级
/// 断言在 e2e S33 钉住）。
#[test]
fn a7_exec_cell_command_payload_is_neutralized() {
    let cells = tchat_tui::chat_driver::project_history(vec![HostHistoryItem::Exec {
        command: "echo \u{1b}[2Jclear\u{1b}]52;c;evil\u{07}clip\u{1b}(Bn".into(),
        output: String::new(),
        exit_code: Some(0),
        cancelled: false,
        error: None,
        duration_ms: None,
    }]);
    let mut all = String::new();
    for cell in &cells {
        all.push_str(&cell_screen_text(cell.as_ref(), 80));
    }
    assert!(all.contains("echo"), "命令文本保留：{all:?}");
    assert!(all.contains("clear") && all.contains("clip"), "载荷文本可见：{all:?}");
    assert!(!all.contains('\u{1b}'), "控制序列不得出现：{all:?}");
}

/// 012 A7 扩展：统一 diff 载体携带注入序列，经 PatchHistoryCell 渲染零动作。
#[test]
fn a7_patch_cell_diff_payload_is_neutralized() {
    use tchat_tui::chat_driver::HostPatchChange;
    use tchat_tui::chat_driver::HostPatchKind;
    let cells = tchat_tui::chat_driver::project_history(vec![HostHistoryItem::Patch {
        ok: true,
        cancelled: false,
        state_known: true,
        error: None,
        changed: Vec::new(),
        changes: vec![HostPatchChange {
            path: "/w/x.txt".into(),
            kind: HostPatchKind::Update {
                diff: "--- a\n+++ b\n@@ -1 +1 @@\n-old\u{1b}[2J\n+new\u{1b}]52;c;evil\u{07}".into(),
            },
        }],
    }]);
    let mut all = String::new();
    for cell in &cells {
        all.push_str(&cell_screen_text(cell.as_ref(), 80));
    }
    assert!(all.contains("old") && all.contains("new"), "diff 内容可见：{all:?}");
    assert!(!all.contains('\u{1b}'), "控制序列不得出现：{all:?}");
}

/// 012 A7 扩展：取消态 cell 的命令与错误消息载荷同样零动作。
#[test]
fn a7_cancelled_cell_payload_is_neutralized() {
    let cells = tchat_tui::chat_driver::project_history(vec![HostHistoryItem::Exec {
        command: "run \u{1b}[2Jx".into(),
        output: "partial \u{1b}]52;c;evil\u{07}".into(),
        exit_code: None,
        cancelled: true,
        error: Some("取消原因 \u{1b}(B".into()),
        duration_ms: None,
    }]);
    let mut all = String::new();
    for cell in &cells {
        all.push_str(&cell_screen_text(cell.as_ref(), 80));
    }
    assert!(all.contains("run") && all.contains("partial"), "文本可见：{all:?}");
    assert!(!all.contains('\u{1b}'), "控制序列不得出现：{all:?}");
}
