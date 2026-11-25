use once_cell::sync::Lazy;
use regex::Regex;

/// Regex matching ANSI escape sequences (CSI sequences, OSC, simple escapes).
static ANSI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        // CSI: ESC [ or 8-bit CSI, parameter bytes, intermediates, final byte
        r"(?:\x1b\[|\x{9b})[0-?]*[ -/]*[@-~]",
        // OSC / DCS / SOS / PM / APC strings, terminated by BEL or ST, or
        // unterminated through the end of the line
        r"|(?:\x1b[\]PX^_]|[\x{9d}\x{90}\x{98}\x{9e}\x{9f}])[^\x07\x1b\x{9c}]*(?:\x07|\x1b\\|\x{9c})?",
        // any other two-byte escape
        r"|\x1b[^\[\]PX^_]?",
    ))
    .unwrap()
});

/// Regex matching JSON-encoded ANSI escapes: literal text \u001b[...m or \u001B[...m
/// These appear in Docker JSON log output where ESC is encoded as \u001b.
static JSON_ANSI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\\u001[bB]\[[0-9;]*[A-Za-z]").unwrap()
});

/// Make untrusted text safe to draw in the terminal: strip ANSI escape
/// sequences, turn tabs into spaces, and drop every other control character
/// (C0, DEL, C1) plus bidi overrides that could reorder what is shown.
/// Log content is attacker-influenced, so everything shown from a source goes
/// through this.
pub fn sanitize(s: &str) -> String {
    strip_ansi(s)
        .chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() => None,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' => None,
            c => Some(c),
        })
        .collect()
}

/// Strip ANSI escape codes from a string.
/// Handles both real ESC bytes (\x1b) and JSON-encoded (\u001b) forms.
pub fn strip_ansi(s: &str) -> String {
    let pass1 = ANSI_RE.replace_all(s, "");
    JSON_ANSI_RE.replace_all(&pass1, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_sgr_codes() {
        assert_eq!(strip_ansi("\x1b[31mERROR\x1b[0m"), "ERROR");
        assert_eq!(strip_ansi("\x1b[1;32mOK\x1b[0m"), "OK");
    }

    #[test]
    fn strip_dim_and_reset() {
        assert_eq!(
            strip_ansi("\x1b[2m2025-01-15T10:00:00Z\x1b[0m [INFO] hello"),
            "2025-01-15T10:00:00Z [INFO] hello"
        );
    }

    #[test]
    fn no_ansi_passthrough() {
        let s = "plain log line with no escapes";
        assert_eq!(strip_ansi(s), s);
    }

    #[test]
    fn strip_multiple_sequences() {
        assert_eq!(
            strip_ansi("\x1b[1m\x1b[31mred bold\x1b[0m normal"),
            "red bold normal"
        );
    }

    #[test]
    fn strip_cursor_movement() {
        assert_eq!(strip_ansi("\x1b[2Jcleared"), "cleared");
    }

    #[test]
    fn empty_string() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_json_encoded_ansi() {
        assert_eq!(
            strip_ansi(r#"\u001b[31mERROR\u001b[0m"#),
            "ERROR"
        );
    }

    #[test]
    fn strip_json_encoded_mixed() {
        assert_eq!(
            strip_ansi(r#"\u001b[2m2025-01-15\u001b[0m [INFO] hello"#),
            "2025-01-15 [INFO] hello"
        );
    }

    #[test]
    fn strip_json_encoded_uppercase_b() {
        assert_eq!(
            strip_ansi(r#"\u001B[1mBOLD\u001B[0m"#),
            "BOLD"
        );
    }

    #[test]
    fn sanitize_removes_every_control_and_bidi_override() {
        let nasty = "a\x1b[?1049lb\x1b]0;t\x07c\x1b]52;c;eA==\x1b\\d\u{9b}31me\u{9d}0;t\u{9c}f\r\x08\x07\x7f\u{202E}g\u{2066}h\x1bPq\x1b\\i\x1b]unterminated";
        let out = sanitize(nasty);
        assert!(!out.chars().any(|c| c.is_control()), "{:?}", out);
        assert_eq!(out, "abcdefghi");
    }

    #[test]
    fn sanitize_keeps_ordinary_text() {
        assert_eq!(sanitize("café ✓ 日本 [x] a\tb"), "café ✓ 日本 [x] a b");
    }
}
