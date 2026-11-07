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

use crate::chat_driver::HostHistoryItem;
use crate::history_cell::HistoryCell;
use crate::test_backend::VT100Backend;

/// A7 唯一攻击标记：渲染器合法的光标／样式／超链接控制不含该标记——以此
/// 区分外来载荷与合法控制，不对全部 ESC 一律拒绝。
const ATK: &str = "ATK7QZ";

/// 原始字节检查：进入 VT100 解析器**之前**捕获的字节流不得含带攻击标记的
/// 外来序列（CSI／OSC 52／DCS／nF／C1 CSI／CR 六类）。两层判定：
/// 1. 直接组合——引入符紧跟标记（覆盖 nF/CR 等短序列与刻意构造形态）；
/// 2. 序列范围扫描——CSI/OSC/DCS/SOS/PM/APC 的完整序列内部出现标记（覆盖
///    真实形态如 OSC 52 `\x1b]52;c;…`，标记与引入符隔着参数）。
/// 这是比屏幕断言更强的证据：即使解析器吞掉序列、屏幕不含 ESC，也必须失败。
fn assert_no_foreign_sequences(raw: &[u8], where_: &str) {
    let atk = ATK.as_bytes();
    let mut foreign: Vec<String> = Vec::new();
    // 第一层：引入符＋标记直接相邻
    for intro in [
        b"\x1b[" as &[u8], b"\x1b]", b"\x1bP", b"\x1b(", b"\x1b)", b"\x9b", b"\r", b"\xc2\x9b",
    ] {
        let mut seq = intro.to_vec();
        seq.extend_from_slice(atk);
        if raw.windows(seq.len()).any(|w| w == seq.as_slice()) {
            foreign.push(format!("直接组合 {:?}", String::from_utf8_lossy(&seq)));
        }
    }
    // 第二层：完整序列范围内含标记
    let mut i = 0;
    while i < raw.len() {
        let start = i;
        let end;
        match raw[i] {
            0x1b if i + 1 < raw.len() && raw[i + 1] == b'[' => {
                // CSI：参数/中间字节后由 0x40..=0x7E 终止
                let mut j = i + 2;
                while j < raw.len() && !(0x40..=0x7E).contains(&raw[j]) {
                    j += 1;
                }
                end = j.min(raw.len() - 1);
                i = end + 1;
            }
            0x1b if i + 1 < raw.len() && matches!(raw[i + 1], b']' | b'P' | b'X' | b'^' | b'_') => {
                // OSC/DCS/SOS/PM/APC：BEL 或 ST（ESC \）终止
                let mut j = i + 2;
                while j < raw.len() {
                    if raw[j] == 0x07 {
                        break;
                    }
                    if raw[j] == 0x1b && j + 1 < raw.len() && raw[j + 1] == b'\\' {
                        break;
                    }
                    j += 1;
                }
                end = j.min(raw.len() - 1);
                i = end + 1;
            }
            0x9b => {
                let mut j = i + 1;
                while j < raw.len() && !(0x40..=0x7E).contains(&raw[j]) {
                    j += 1;
                }
                end = j.min(raw.len() - 1);
                i = end + 1;
            }
            _ => {
                i += 1;
                continue;
            }
        }
        let seq = &raw[start..=end];
        if seq.windows(atk.len()).any(|w| w == atk) {
            foreign.push(format!("序列含标记 {:?}", String::from_utf8_lossy(seq)));
        }
    }
    assert!(
        foreign.is_empty(),
        "{where_}：外来注入序列直通 writer：{foreign:?}（原始字节片段 {:?}）",
        String::from_utf8_lossy(&raw[..raw.len().min(400)])
    );
}

/// 捕获进入 VT100 解析器前的原始字节：先记录再转发（A7 原始输出证据的测试侧
/// 辅助，与 VT100Backend 的 parser 边界等价——不改生产渲染与过滤逻辑）。
struct CapturingParser {
    parser: vt100::Parser,
    raw: Vec<u8>,
}

impl std::io::Write for CapturingParser {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.raw.extend_from_slice(buf);
        self.parser.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.parser.flush()
    }
}

/// 六类攻击载荷（各带唯一标记）：CSI 清屏、OSC 52 写剪贴板、nF 字符集、
/// DCS、CR 行首覆盖、C1 单字节 CSI；字母 A–G 为正常文字探针。
const ATTACK_PAYLOAD: &str = concat!(
    "A", "\x1b[", "ATK7QZ", "2J", "B",           // CSI 清屏
    "\x1b]52;c;", "ATK7QZ", "\x07", "C",          // OSC 52 剪贴板
    "\x1b(", "ATK7QZ", "D",                       // nF 字符集
    "\x1bP", "ATK7QZ", ";2|x\x1b\\", "E",         // DCS
    "E2", "\r", "ATK7QZ", "F",                    // CR 行首覆盖
    "\u{9b}", "ATK7QZ", "2J", "G",                // C1 CSI
);

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
    let cell = crate::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
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
    let mut controller = crate::streaming::controller::StreamController::new_with_inline_visualizations(
        None,
        std::path::Path::new("/tmp"),
        crate::history_cell::HistoryRenderMode::Rich,
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
    let cell = crate::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
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
    // 原始字节＋屏幕双查（ASCII 载荷；CJK 见 cell_screen_text 注释）：
    // Paragraph 渲染 → Buffer（过滤边界）→ 逐 cell 写入**捕获 writer**
    //（进入 VT100 解析器之前）→ 原始字节断言＋解析后屏幕断言。
    // 屏幕不含 ESC 不能作为原始字节安全证据——解析器消费的序列必须先在此拦截。
    use ratatui::widgets::Widget as _;
    let cell = crate::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
        ATTACK_PAYLOAD.to_string(),
        std::path::Path::new("/tmp"),
        None,
    );
    let area = ratatui::layout::Rect::new(0, 0, 80, 24);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Paragraph::new(cell.display_lines(80)).render(area, &mut buf);
    let mut cap = CapturingParser { parser: vt100::Parser::new(24, 80, 0), raw: Vec::new() };
    for y in 0..area.height {
        for x in 0..area.width.saturating_sub(1) {
            cap.write(buf.cell((x, y)).unwrap().symbol().as_bytes())
                .expect("写入捕获 writer 成功");
        }
        if y + 1 < area.height {
            cap.write(b"\r\n").expect("行分隔写入成功");
        }
    }
    // 原始字节：无任何外来标记序列；正常文字确实直通（非空捕获）
    assert_no_foreign_sequences(&cap.raw, "full_pipeline");
    assert!(cap.raw.windows(1).any(|w| w[0] == b'A'), "正常文字进入 writer");
    // 解析后屏幕：文字保留、序列零动作
    let text = cap.parser.screen().contents().to_string();
    for ch in ['A', 'B', 'C', 'D', 'E', 'F', 'G'] {
        assert!(text.contains(ch), "文字 {ch} 保留：{text:?}");
    }
    assert!(!text.contains('\x1b'), "屏幕无 ESC：{text:?}");
    assert!(!text.contains('\u{9b}'), "屏幕无 C1 CSI：{text:?}");
    assert!(!text.contains('\r'), "屏幕无 CR：{text:?}");
    // 屏幕级：[2J 若被执行则 A-G 全部消失——上方存在性断言即防清屏
}

#[test]
fn a7_resume_projection_of_tool_output_with_payload_is_neutralized() {
    // /resume 投影路径：exec 输出含注入序列
    let cells = crate::chat_driver::project_history(vec![HostHistoryItem::Exec {
        command: "echo x".into(),
        output: "line1\x1b[2Jline2\x1b]52;c;evil\x07line3".into(),
        exit_code: Some(0),
        cancelled: false,
        error: None,
        duration_ms: None,
        running: None,
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
// 这里固定断言：外来序列若直通 writer（含被解析器消费、屏幕不可见的情形），
// a7_full_pipeline／a7_byte_level 的**原始字节检查**先行失败；屏幕断言（若有人
// 移除 ratatui 的控制字符剥离，ESC 会出现在屏幕文本）为第二道防线。

#[test]
fn a7_error_message_cell_with_payload_is_neutralized() {
    // 错误消息入口：驱动 Failed 路径的 PlainHistoryCell（错误文本来自服务端响应，
    // 属不可信来源——003 redact 后仍需渲染边界等价强度）
    let cell = crate::history_cell::PlainHistoryCell::new(vec![ratatui::text::Line::from(
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
    let cell = crate::history_cell::PlainHistoryCell::new(vec![ratatui::text::Line::from(
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
    // 真实 draw 路径的原始字节直查：CrosstermBackend 写出的字节先经捕获 writer
    // 记录、再进入 VT100 解析器——draw 产生的**合法**光标/样式控制序列（含 ESC）
    // 必须存在（正向对照，证明捕获边界看到了真实渲染输出、检查并非一刀切拒绝
    // ESC），而带唯一标记的外来载荷序列不得出现。
    let cell = crate::history_cell::AgentMarkdownCell::new_with_inline_visualizations(
        ATTACK_PAYLOAD.to_string(),
        std::path::Path::new("/tmp"),
        None,
    );
    let mut cap = CapturingParser { parser: vt100::Parser::new(10, 80, 0), raw: Vec::new() };
    {
        let backend = ratatui::prelude::CrosstermBackend::new(&mut cap);
        // 038-CI：原 with_options 会查询环境终端尺寸（CI 无 TTY＋tput 不可用时
        // EAGAIN 失败）——本测试要的是确定性 80×10 内存终端，改用显式尺寸的
        // 测试构造器，断言本体不变。
        let mut term = crate::custom_terminal::Terminal::with_screen_size_and_cursor_position_for_test(
            backend,
            ratatui::layout::Size { width: 80, height: 10 },
            ratatui::layout::Position::ORIGIN,
        );
        term.set_viewport_area(ratatui::layout::Rect::new(0, 0, 80, 10)); // 008-ADAPT：同上
        term.draw(|frame| {
            use ratatui::widgets::Widget as _;
            let area = frame.area();
            ratatui::widgets::Paragraph::new(cell.display_lines(80)).render(area, frame.buffer_mut());
        })
        .expect("draw 必须成功");
    }
    // 正向对照：渲染器合法控制序列（ESC 引导）确实流经 writer——检查不误报
    assert!(
        cap.raw.windows(2).any(|w| w == b"\x1b["),
        "渲染器合法 CSI 控制应存在于原始字节（否则捕获未生效）"
    );
    // 外来标记序列零直通；正常文字保留
    assert_no_foreign_sequences(&cap.raw, "draw 路径");
    let text = cap.parser.screen().contents().to_string();
    for ch in ['A', 'B', 'C', 'D', 'E', 'F', 'G'] {
        assert!(text.contains(ch), "文字 {ch} 保留：{text:?}");
    }
    assert!(!text.contains('\x1b') && !text.contains('\u{9b}') && !text.contains('\r'));
}

// ---------- 012 扩展：工具命令/diff 载体注入 ----------

/// 012 A7 扩展：工具命令字符串携带伪造转义（CSI 清屏/OSC 52 剪贴板/nF 字符集），
/// 经 ExecCell 渲染零动作——文本可见、无裸控制序列（剪贴板写入零动作由字节级
/// 断言在 e2e S33 钉住）。
#[test]
fn a7_exec_cell_command_payload_is_neutralized() {
    let cells = crate::chat_driver::project_history(vec![HostHistoryItem::Exec {
        command: "echo \u{1b}[2Jclear\u{1b}]52;c;evil\u{07}clip\u{1b}(Bn".into(),
        output: String::new(),
        exit_code: Some(0),
        cancelled: false,
        error: None,
        duration_ms: None,
        running: None,
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
    use crate::chat_driver::HostPatchChange;
    use crate::chat_driver::HostPatchKind;
    let cells = crate::chat_driver::project_history(vec![HostHistoryItem::Patch {
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
    let cells = crate::chat_driver::project_history(vec![HostHistoryItem::Exec {
        command: "run \u{1b}[2Jx".into(),
        output: "partial \u{1b}]52;c;evil\u{07}".into(),
        exit_code: None,
        cancelled: true,
        error: Some("取消原因 \u{1b}(B".into()),
        duration_ms: None,
        running: None,
    }]);
    let mut all = String::new();
    for cell in &cells {
        all.push_str(&cell_screen_text(cell.as_ref(), 80));
    }
    assert!(all.contains("run") && all.contains("partial"), "文本可见：{all:?}");
    assert!(!all.contains('\u{1b}'), "控制序列不得出现：{all:?}");
}

/// 016 A7 扩展：运行中会话 cell（running＋会话编号＋已收集输出）携带注入序列——
/// 经降级渲染（PlainHistoryCell 文本行）零动作，PTY 输出继续被过滤。
#[test]
fn a7_running_session_cell_payload_is_neutralized() {
    let cells = crate::chat_driver::project_history(vec![HostHistoryItem::Exec {
        command: "srv \u{1b}[2Jx".into(),
        output: "boot \u{1b}]52;c;evil\u{07} ready".into(),
        exit_code: None,
        cancelled: false,
        error: None,
        duration_ms: Some(12),
        running: Some("s7\u{1b}(B".into()),
    }]);
    let mut all = String::new();
    for cell in &cells {
        all.push_str(&cell_screen_text(cell.as_ref(), 80));
    }
    // 宽度渲染会插入空格：以去空白视图断言
    let compact: String = all.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("仍在运行"), "运行中独立呈现：{all:?}");
    assert!(compact.contains("write_stdin"), "提示可续接：{all:?}");
    assert!(compact.contains("boot") && compact.contains("ready"), "输出文本可见：{all:?}");
    assert!(!all.contains('\u{1b}'), "控制序列不得出现（含会话编号载荷）：{all:?}");
}

