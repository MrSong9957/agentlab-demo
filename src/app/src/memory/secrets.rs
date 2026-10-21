//! 013-M3：秘密信息替换（D7）。
//!
//! 上游来源：openai/codex @ `ce0323f2f2d3b25e82806cd555237da3db4e1897`
//! （Apache-2.0）——`secrets/src/sanitizer.rs::redact_secrets` 逐字移植
//! （四条正则与替换文本不变；ASCII、Unicode 变体故意排除）。适配唯一点：
//! `regex`→`regex-lite`（工作区已锁版本，tui 同款）——语法兼容性（内联
//! `(?i:…)`、`\b`、量词、`$N` 捕获引用替换）由下方移植测试证明（D7 条件）。

use regex_lite::Regex;
use std::sync::LazyLock;

static OPENAI_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| compile_regex(r"sk-[A-Za-z0-9]{20,}"));
static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAKIA[0-9A-Z]{16}\b"));
static BEARER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?i:\bBearer)[ \t]+[A-Za-z0-9._~+/-]{16,}=*"));
static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r#"(?i)\b(api[_-]?key|token|secret|password)\b(\s*[:=]\s*)(["']?)[^\s"']{8,}"#)
});

/// 尽力而为地按已知正则移除文本中的密钥与令牌（上游语义：best-effort）。
pub fn redact_secrets(input: String) -> String {
    let redacted = BEARER_TOKEN_REGEX.replace_all(&input, "Bearer [REDACTED_SECRET]");
    let redacted = OPENAI_KEY_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = AWS_ACCESS_KEY_ID_REGEX.replace_all(&redacted, "[REDACTED_SECRET]");
    let redacted = SECRET_ASSIGNMENT_REGEX.replace_all(&redacted, "$1$2$3[REDACTED_SECRET]");

    redacted.to_string()
}

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(err) => panic!("invalid regex pattern `{pattern}`: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // —— 上游 secrets/src/sanitizer.rs 测试移植（语义锚）——

    #[test]
    fn redacts_supported_bearer_tokens() {
        let cases = [
            (
                "Bearer abcde+fghijklmnopqrstuvwxyz012345",
                "Bearer [REDACTED_SECRET]",
            ),
            (
                "Bearer abcdefghijklmnop+secret_suffix",
                "Bearer [REDACTED_SECRET]",
            ),
            (
                "Bearer sk-abcdefghijklmnopqrst+secret_suffix",
                "Bearer [REDACTED_SECRET]",
            ),
            (
                "Bearer AKIAABCDEFGHIJKLMNOP/~secret_suffix",
                "Bearer [REDACTED_SECRET]",
            ),
            (
                "Bearer AbcdefghijklMN09._~+/-==; echo done",
                "Bearer [REDACTED_SECRET]; echo done",
            ),
            (
                "authorization: bEaReR\tabcdefghijklmnop",
                "authorization: Bearer [REDACTED_SECRET]",
            ),
            ("Bearer   abcdefghijklmnop", "Bearer [REDACTED_SECRET]"),
        ];

        for (input, expected) in cases {
            assert_eq!(redact_secrets(input.to_string()), expected);
        }
    }

    #[test]
    fn avoids_bearer_false_positives() {
        let cases = [
            "Bearer of good news",
            "Bearer abcdefghijklmno",
            "NotABearer abcdefghijklmnop",
            "Bearerabcdefghijklmnop",
            "Bearer\nabcdefghijklmnop",
            "Bearer\u{a0}abcdefghijklmnop",
            "Bearer abcdefghijklmno\u{212a}",
        ];

        for input in cases {
            assert_eq!(redact_secrets(input.to_string()), input);
        }
    }

    // —— 其余三条正则的行为锚（上游测试未直接覆盖，按模式语义钉住）——

    #[test]
    fn redacts_openai_style_keys() {
        assert_eq!(
            redact_secrets("key sk-ABCDEFGHIJKLMNOPQRSTUVwx123 end".into()),
            "key [REDACTED_SECRET] end"
        );
        // 少于 20 个尾随字母数字不匹配。
        let short = "sk-abc123";
        assert_eq!(redact_secrets(short.into()), short);
    }

    #[test]
    fn redacts_aws_access_key_ids() {
        assert_eq!(
            redact_secrets("AKIAABCDEFGHIJKLMNOP".into()),
            "[REDACTED_SECRET]"
        );
        // 不足 16 位或小写不匹配。
        assert_eq!(redact_secrets("AKIAABC".into()), "AKIAABC");
        assert_eq!(
            redact_secrets("AKIAabcdefghijklmnop".into()),
            "AKIAabcdefghijklmnop"
        );
    }

    #[test]
    fn redacts_secret_assignments_and_keeps_prefix() {
        assert_eq!(
            redact_secrets("api_key = supersecretvalue123".into()),
            "api_key = [REDACTED_SECRET]"
        );
        assert_eq!(
            redact_secrets(r#"token: "anotherlongsecret99""#.into()),
            r#"token: "[REDACTED_SECRET]""#
        );
        assert_eq!(
            redact_secrets("PASSWORD: hunter2hunter2".into()),
            "PASSWORD: [REDACTED_SECRET]"
        );
        // 值不足 8 字符不替换。
        assert_eq!(
            redact_secrets("password: short".into()),
            "password: short"
        );
        // 非赋值语境的普通词不替换。
        assert_eq!(
            redact_secrets("the token machine".into()),
            "the token machine"
        );
    }
}
