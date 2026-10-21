//! 显示边界：不可信文本（模型回复、远端错误消息、目录元数据）渲染到终端前的唯一过滤点。
//! 完整解析并剥离 ECMA-48 转义序列（CSI/OSC/DCS/SOS/PM/APC、两字符转义）与危险控制字符；
//! 只删除 ESC 不够——参数残留会被终端当文字显示，且序列可能被后续输出补全。

/// 终端显示白名单：保留常规文字、换行与 Tab；其余 C0/C1 控制字符与全部转义序列一律剥离。
pub fn sanitize_for_terminal(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' | '\t' => {
                out.push(c);
                i += 1;
            }
            '\x1b' => i = skip_escape(&chars, i),
            c if is_dangerous_control(c) => i += 1,
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// 按字符数截断：多字节文字不会在字节边界被切断。
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn is_dangerous_control(c: char) -> bool {
    // C0（\n \t 已放行；\r 剥离——回车可实现终端行首覆盖伪造，N1）、DEL、C1（含单字节形式的 CSI/OSC 引导）
    matches!(
        c,
        '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\r' | '\u{e}'..='\u{1f}' | '\u{7f}' | '\u{80}'..='\u{9f}'
    )
}

/// 跳过从 ESC 开始的完整转义序列，返回下一个处理索引。
fn skip_escape(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    let Some(&c) = chars.get(i) else { return i };
    match c {
        '[' => {
            // CSI：参数 0x30-0x3F、中间 0x20-0x2F，终止字节 0x40-0x7E
            i += 1;
            while i < chars.len() {
                let ch = chars[i] as u32;
                if (0x40..=0x7e).contains(&ch) {
                    return i + 1;
                }
                if (0x20..=0x3f).contains(&ch) {
                    i += 1;
                } else {
                    return i; // 序列被打断：后续字符按普通文本处理
                }
            }
            i
        }
        ']' | 'P' | 'X' | '^' | '_' => {
            // OSC/DCS/SOS/PM/APC：字符串序列，以 BEL 或 ST（ESC \）终止
            i += 1;
            while i < chars.len() {
                if chars[i] == '\x07' {
                    return i + 1;
                }
                if chars[i] == '\x1b' {
                    return if chars.get(i + 1) == Some(&'\\') { i + 2 } else { i };
                }
                i += 1;
            }
            i
        }
        // nF 序列（如字符集选择 ESC ( B）：中间字节 0x20-0x2F 可重复，终字节 0x30-0x7E 一个；
        // 打断（终字节缺位）时停在打断处，后续字符按普通文本处理
        c if (c as u32) >= 0x20 && (c as u32) <= 0x2f => {
            i += 1;
            while matches!(chars.get(i), Some(&n) if (n as u32) >= 0x20 && (n as u32) <= 0x2f) {
                i += 1;
            }
            match chars.get(i) {
                Some(&f) if (f as u32) >= 0x30 && (f as u32) <= 0x7e => i + 1,
                _ => i,
            }
        }
        // 两字符转义（终字节直接跟随 ESC，如 ESC c 复位终端、ESC M 反序滚动）
        c if (c as u32) >= 0x30 && (c as u32) <= 0x7e => i + 1,
        // 未知形式：只删 ESC，后续字符按普通文本处理
        _ => i,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csi_sequences_removed() {
        assert_eq!(sanitize_for_terminal("\x1b[2J\x1b[1;1H已清除"), "已清除");
        assert_eq!(sanitize_for_terminal("a\x1b[38;5;196m红\x1b[0mb"), "a红b");
        // 序列被换行打断：换行保留，序列参数剥离
        assert_eq!(sanitize_for_terminal("a\x1b[2\nJb"), "a\nJb");
    }

    #[test]
    fn osc_and_string_sequences_removed() {
        // OSC 带 BEL 终止：改标题 / 写剪贴板
        assert_eq!(sanitize_for_terminal("前\x1b]0;标题\x07后"), "前后");
        assert_eq!(sanitize_for_terminal("\x1b]52;c;aGVsbG8=\x07x"), "x");
        // OSC 带 ST 终止：超链接
        assert_eq!(sanitize_for_terminal("\x1b]8;;http://e\x1b\\点我\x1b]8;;\x07尾"), "点我尾");
        // OSC 未终止：吞到串尾，防止恶意内容延后生效
        assert_eq!(sanitize_for_terminal("a\x1b]0;pwned"), "a");
        // DCS / APC / PM / SOS
        assert_eq!(sanitize_for_terminal("a\x1bP1;2|data\x1b\\b"), "ab");
        assert_eq!(sanitize_for_terminal("a\x1b_X;Y\x07b"), "ab");
        // 悬挂裸 ESC（原 bare_escape 场景并入；注意与上面“OSC 未闭合”不同，
        // 这里是孤立 ESC 而非未闭合字符串序列）：串尾悬挂直接丢弃，ESC 后跟换行不吞后续行
        assert_eq!(sanitize_for_terminal("abc\x1b"), "abc");
        assert_eq!(sanitize_for_terminal("a\x1b\nb"), "a\nb");
    }

    #[test]
    fn two_char_escapes_removed() {
        // ESC c = 终端复位，必须整体剥离
        assert_eq!(sanitize_for_terminal("a\x1bc退出"), "a退出");
        // ESC M 剥离后，"滚动"是正常文字，应保留
        assert_eq!(sanitize_for_terminal("a\x1bM滚动b"), "a滚动b");
    }

    #[test]
    fn dangerous_controls_removed() {
        assert_eq!(sanitize_for_terminal("a\u{7f}b"), "ab");
        // C1 单字节 CSI（U+009B）：引导字符被剥离后，"2J"退化为普通文字
        assert_eq!(sanitize_for_terminal("a\u{9b}2Jb"), "a2Jb");
        // 退格、响铃、垂直制表等
        assert_eq!(sanitize_for_terminal("a\u{8}\u{7}\u{b}b"), "ab");
    }

    #[test]
    fn normal_text_preserved() {
        let text = "你好，world！\n\t缩进一行 🎉 常规~!@#$%^&*()_+<>？";
        assert_eq!(sanitize_for_terminal(text), text);
        assert_eq!(sanitize_for_terminal("纯文本"), "纯文本");
    }

    #[test]
    fn carriage_return_stripped_all_paths() {
        // N1：CR 不在显示白名单（\n/\t），必须剥离——否则终端行首覆盖可伪造 Agent：/错误： 行
        assert_eq!(sanitize_for_terminal("金额确认：100 元\r已被替换-PWNED"), "金额确认：100 元已被替换-PWNED");
        assert_eq!(sanitize_for_terminal("a\r\nb"), "a\nb", "CRLF 只保留 LF");
        assert_eq!(sanitize_for_terminal("\r"), "");
        // 正常文字、换行、Tab 保留
        assert_eq!(sanitize_for_terminal("正常\n\t文字"), "正常\n\t文字");
    }

    // ---------- 高风险清单补充（2026-10-10）----------

    #[test]
    fn charset_selection_escapes_removed() {
        // 字符集选择 ESC ( B / ESC ) 0（三字节序列）整段剥离，正常文字保留
        assert_eq!(sanitize_for_terminal("a\x1b(Bb"), "ab");
        assert_eq!(sanitize_for_terminal("a\x1b)0b"), "ab");
        // TERM-N02：正常前缀＋恶意序列＋正常后缀——前后缀保留，动作零
        assert_eq!(sanitize_for_terminal("前\x1b[2J中\x1b]0;t\x07后"), "前中后");
    }

    #[test]
    fn truncate_boundary_299_300_301() {
        // SECRET-B01：正文截断边界按字符精确（多字节不切坏）
        let base: String = "汉x".repeat(160); // 320 字符
        for n in [299usize, 300, 301] {
            let out = truncate_chars(&base, n);
            assert_eq!(out.chars().count(), n, "n={n}");
            assert_eq!(out, base.chars().take(n).collect::<String>(), "n={n} 前缀精确");
        }
        // 原 truncate_is_char_safe 场景并入：纯中文按字符截断不切半字节；短串原样通过
        assert_eq!(truncate_chars(&"汉".repeat(320), 300).chars().count(), 300);
        assert_eq!(truncate_chars("abc", 10), "abc");
    }
}
