use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Unknown,
}

impl Level {
    pub fn severity(self) -> u8 {
        match self {
            Level::Trace => 0,
            Level::Debug => 1,
            Level::Info => 2,
            Level::Unknown => 2,
            Level::Warn => 3,
            Level::Error => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Unknown => "???",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Level::Trace => "TRC",
            Level::Debug => "DBG",
            Level::Info => "INF",
            Level::Warn => "WRN",
            Level::Error => "ERR",
            Level::Unknown => "???",
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[allow(dead_code)]
pub struct LogEvent {
    pub level: Level,
    pub source: String,
    pub raw: String,
    pub normalized: String,
}

static ISO_TS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2}| ?UTC)?")
        .unwrap()
});

static SYSLOG_TS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?:",
        // "20 Feb 2026 15:03:24.123" (Redis, RFC 2822)
        r"\b\d{1,2}\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{4}\s+\d{2}:\d{2}:\d{2}(?:\.\d+)?",
        r"|",
        // "Feb 20 15:03:24.123" (classic syslog)
        r"\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}\s+\d{2}:\d{2}:\d{2}(?:\.\d+)?",
        r"|",
        // "20/Feb/2026:15:03:24" (Common Log Format / Apache)
        r"\b\d{2}/(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)/\d{4}:\d{2}:\d{2}:\d{2}",
        r")",
    ))
    .unwrap()
});

static UUID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .unwrap()
});

static IP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap()
});

static HEX_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b0x[0-9a-fA-F]+\b").unwrap()
});

static DUR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b\d+(\.\d+)?\s?(ms|s|us|µs|ns)\b").unwrap()
});

static NUM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b\d+(\.\d+)?\b").unwrap()
});

pub fn detect_level(line: &str) -> Level {
    let upper = line.to_ascii_uppercase();
    if upper.contains("FATAL") || upper.contains("PANIC") || upper.contains("ERROR") {
        Level::Error
    } else if upper.contains("WARN") {
        Level::Warn
    } else if upper.contains("INFO")
        || upper.contains(" LOG:")
        || upper.contains("NOTICE:")
        || upper.contains("HINT:")
    {
        Level::Info
    } else if upper.contains("DEBUG")
        || upper.contains("STATEMENT:")
        || upper.contains("DETAIL:")
    {
        Level::Debug
    } else if upper.contains("TRACE") {
        Level::Trace
    } else {
        Level::Unknown
    }
}

pub fn normalize(line: &str) -> String {
    let a = ISO_TS.replace_all(line, "<TS>");
    let a2 = SYSLOG_TS.replace_all(&a, "<TS>");
    let b = UUID_RE.replace_all(&a2, "<UUID>");
    let c = IP_RE.replace_all(&b, "<IP>");
    let d = HEX_RE.replace_all(&c, "<HEX>");
    let e = DUR_RE.replace_all(&d, "<DUR>");
    let f = NUM_RE.replace_all(&e, "<NUM>");
    f.into_owned()
}

pub fn parse_line(source: &str, line: &str) -> LogEvent {
    let clean = crate::util::strip_ansi(line);
    let level = detect_level(&clean);
    let normalized = normalize(&clean);
    LogEvent {
        level,
        source: source.to_string(),
        raw: clean,
        normalized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_level_error() {
        assert_eq!(detect_level("[ERROR] something failed"), Level::Error);
        assert_eq!(detect_level("error: bad thing"), Level::Error);
    }

    #[test]
    fn detect_level_warn() {
        assert_eq!(detect_level("[WARN] disk almost full"), Level::Warn);
        assert_eq!(detect_level("Warning: low memory"), Level::Warn);
    }

    #[test]
    fn detect_level_info() {
        assert_eq!(detect_level("[INFO] server started"), Level::Info);
    }

    #[test]
    fn detect_level_debug() {
        assert_eq!(detect_level("[DEBUG] entering function"), Level::Debug);
    }

    #[test]
    fn detect_level_trace() {
        assert_eq!(detect_level("[TRACE] packet received"), Level::Trace);
    }

    #[test]
    fn detect_level_unknown() {
        assert_eq!(detect_level("just a random line"), Level::Unknown);
    }

    #[test]
    fn detect_level_case_insensitive() {
        assert_eq!(detect_level("ErRoR in module"), Level::Error);
        assert_eq!(detect_level("info: all good"), Level::Info);
    }

    #[test]
    fn detect_level_postgres_log() {
        assert_eq!(
            detect_level("2026-02-20 15:03:24 UTC [123] LOG:  checkpoint starting"),
            Level::Info
        );
    }

    #[test]
    fn detect_level_postgres_statement() {
        assert_eq!(
            detect_level("2026-02-20 15:03:24 UTC [123] STATEMENT:  SELECT * FROM users"),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_postgres_detail() {
        assert_eq!(
            detect_level("2026-02-20 15:03:24 UTC [123] DETAIL:  Key already exists"),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_postgres_notice() {
        assert_eq!(
            detect_level("2026-02-20 15:03:24 UTC [123] NOTICE:  table created"),
            Level::Info
        );
    }

    #[test]
    fn detect_level_postgres_hint() {
        assert_eq!(
            detect_level("HINT:  Consider using CREATE INDEX"),
            Level::Info
        );
    }

    #[test]
    fn normalize_timestamp() {
        let out = normalize("2025-01-15T10:30:00Z request ok");
        assert!(out.contains("<TS>"));
        assert!(!out.contains("2025"));
    }

    #[test]
    fn normalize_timestamp_with_offset() {
        let out = normalize("2025-01-15T10:30:00.123+05:30 hello");
        assert!(out.contains("<TS>"));
    }

    #[test]
    fn normalize_uuid() {
        let out = normalize("id=550e8400-e29b-41d4-a716-446655440000 done");
        assert!(out.contains("<UUID>"));
        assert!(!out.contains("550e8400"));
    }

    #[test]
    fn normalize_ip() {
        let out = normalize("from 192.168.1.100 port 8080");
        assert!(out.contains("<IP>"));
        assert!(!out.contains("192.168"));
    }

    #[test]
    fn normalize_hex() {
        let out = normalize("addr 0xDEADBEEF offset 0x1a2b");
        assert!(out.contains("<HEX>"));
        assert!(!out.contains("DEADBEEF"));
    }

    #[test]
    fn normalize_duration() {
        let out = normalize("took 350ms to respond");
        assert!(out.contains("<DUR>"));
        assert!(!out.contains("350ms"));
    }

    #[test]
    fn normalize_numbers() {
        let out = normalize("processed 42 items in batch 7");
        assert_eq!(out.matches("<NUM>").count(), 2);
        assert!(!out.contains("42"));
    }

    #[test]
    fn normalize_combined() {
        let line = "2025-01-15T10:00:00Z [INFO] 192.168.1.1 processed 100 requests in 50ms";
        let out = normalize(line);
        assert!(out.contains("<TS>"));
        assert!(out.contains("<IP>"));
        assert!(out.contains("<DUR>"));
        assert!(out.contains("<NUM>"));
    }

    #[test]
    fn parse_line_integrates() {
        let ev = parse_line("test/src", "2025-01-01T00:00:00Z [ERROR] failed at 192.168.0.1");
        assert_eq!(ev.level, Level::Error);
        assert_eq!(ev.source, "test/src");
        assert!(ev.normalized.contains("<TS>"));
        assert!(ev.normalized.contains("<IP>"));
    }

    #[test]
    fn normalize_syslog_timestamp() {
        let out = normalize("Feb 20 15:03:24 myhost sshd[12345]: Accepted");
        assert!(out.contains("<TS>"), "got: {}", out);
        assert!(!out.contains("Feb"));
        assert!(!out.contains("15:03"));
    }

    #[test]
    fn normalize_redis_timestamp() {
        let out = normalize("12345:M 20 Feb 2026 15:03:24.123 * Background saving");
        assert!(out.contains("<TS>"), "got: {}", out);
        assert!(!out.contains("Feb"));
        assert!(!out.contains("15:03"));
    }

    #[test]
    fn normalize_clf_timestamp() {
        let out = normalize("127.0.0.1 - - [20/Feb/2026:15:03:24 +0000] \"GET /\"");
        assert!(out.contains("<TS>"), "got: {}", out);
        assert!(!out.contains("Feb"));
    }

    #[test]
    fn normalize_iso_utc_suffix() {
        let out = normalize("2026-02-20 15:03:24 UTC [123] LOG: checkpoint starting");
        assert!(out.contains("<TS>"), "got: {}", out);
        assert!(!out.contains("2026-02-20"));
    }

    #[test]
    fn level_severity_ordering() {
        assert!(Level::Error.severity() > Level::Warn.severity());
        assert!(Level::Warn.severity() > Level::Info.severity());
        assert!(Level::Info.severity() > Level::Debug.severity());
        assert!(Level::Debug.severity() > Level::Trace.severity());
    }
}
