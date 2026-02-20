use once_cell::sync::Lazy;
use regex::Regex;

/// Regex matching ANSI escape sequences (CSI sequences, OSC, simple escapes).
static ANSI_RE: Lazy<Regex> = Lazy::new(|| {
    // Matches:
    //   \x1b\[...m   (SGR / CSI sequences)
    //   \x1b\[...    (other CSI)
    //   \x1b]...ST   (OSC terminated by BEL or ST)
    //   \x1b[^[]     (two-char escape like \x1b=)
    Regex::new(r"\x1b\[[0-9;]*[A-Za-z]|\x1b\][^\x07]*(?:\x07|\x1b\\)|\x1b[^\x5b\x5d]").unwrap()
});

/// Regex matching JSON-encoded ANSI escapes: literal text \u001b[...m or \u001B[...m
/// These appear in Docker JSON log output where ESC is encoded as \u001b.
static JSON_ANSI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\\u001[bB]\[[0-9;]*[A-Za-z]").unwrap()
});

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
}
