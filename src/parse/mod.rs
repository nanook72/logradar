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

// Bracketed PID: [12345] — common in Postgres, nginx, syslog
static PID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\[\d+\]").unwrap()
});

static NUM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b\d+(\.\d+)?\b").unwrap()
});

// Whole-word level keywords (input is upper-cased first), so "0 errors" or
// "error_count=0" don't count as errors.
static ERROR_WORD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b(?:FATAL|PANIC|ERROR|CRITICAL)\b").unwrap());
static WARN_WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bWARN(?:ING)?\b").unwrap());
static INFO_WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bINFO\b").unwrap());
static DEBUG_WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bDEBUG\b").unwrap());
static TRACE_WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bTRACE\b").unwrap());

// `level=warn`, `lvl="error"`, `severity=INFO` (logfmt and friends).
static LOGFMT_LEVEL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?:^|\s)(?:LEVEL|LVL|SEVERITY)=\"?([A-Z]+)\"?(?:\s|$)"#).unwrap()
});

// `[ERROR]`, `[warn]`, `<INFO>` near the start of the line.
static BRACKET_LEVEL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^.{0,64}?[\[<](TRACE|DEBUG|INFO|NOTICE|WARN|WARNING|ERROR|FATAL|CRIT|CRITICAL|PANIC)[\]>]").unwrap()
});

// logfmt message: msg="quoted text" or msg=word
static LOGFMT_MSG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?:^|\s)(?:msg|message)=(?:"((?:[^"\\]|\\.)*)"|(\S+))"#).unwrap()
});

// Redis log format: "PID:X DD Mon YYYY HH:MM:SS.mmm <level_char> message"
// Level chars: # = warning, * = notice/info, - = verbose/info, . = debug
static REDIS_LEVEL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\d+:[A-Z]\s+.+\s([#*.\-])\s").unwrap()
});

pub fn detect_level(line: &str) -> Level {
    // Check Redis format first (PID:ROLE ... <char> message)
    if let Some(caps) = REDIS_LEVEL.captures(line) {
        return match &caps[1] {
            "#" => Level::Warn,
            "*" => Level::Info,
            "-" => Level::Info,
            "." => Level::Debug,
            _ => Level::Unknown,
        };
    }

    let upper = line.to_ascii_uppercase();
    if let Some(level) = explicit_level(&upper) {
        return level;
    }
    if ERROR_WORD.is_match(&upper) {
        Level::Error
    } else if WARN_WORD.is_match(&upper) {
        Level::Warn
    } else if INFO_WORD.is_match(&upper)
        || upper.contains(" LOG:")
        || upper.contains("NOTICE:")
        || upper.contains("HINT:")
    {
        Level::Info
    } else if DEBUG_WORD.is_match(&upper)
        || upper.contains("STATEMENT:")
        || upper.contains("DETAIL:")
    {
        Level::Debug
    } else if TRACE_WORD.is_match(&upper) {
        Level::Trace
    } else if is_sql_fragment(&upper) {
        Level::Debug
    } else {
        Level::Unknown
    }
}

/// A level written where loggers put one: `[ERROR]`, `level=warn`, or a
/// leading `ERROR`/`E ` style token. Checked before the loose word search so
/// a message that merely mentions "error" keeps its real level.
fn explicit_level(upper: &str) -> Option<Level> {
    if let Some(caps) = LOGFMT_LEVEL.captures(upper) {
        if let Some(level) = level_from_str(&caps[1]) {
            return Some(level);
        }
    }
    if let Some(caps) = BRACKET_LEVEL.captures(upper) {
        return level_from_str(&caps[1]);
    }
    None
}

/// Map a level name or a numeric (pino/bunyan) level to `Level`.
pub fn level_from_str(s: &str) -> Option<Level> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u32>() {
        return Some(match n {
            0..=10 => Level::Trace,
            11..=20 => Level::Debug,
            21..=30 => Level::Info,
            31..=40 => Level::Warn,
            _ => Level::Error,
        });
    }
    match s.to_ascii_uppercase().as_str() {
        "TRACE" | "TRC" | "VERBOSE" | "FINEST" | "FINER" => Some(Level::Trace),
        "DEBUG" | "DBG" | "FINE" => Some(Level::Debug),
        "INFO" | "INF" | "INFORMATION" | "INFORMATIONAL" | "NOTICE" => Some(Level::Info),
        "WARN" | "WRN" | "WARNING" => Some(Level::Warn),
        "ERROR" | "ERR" | "FATAL" | "FTL" | "PANIC" | "CRIT" | "CRITICAL" | "ALERT"
        | "EMERG" | "EMERGENCY" | "SEVERE" => Some(Level::Error),
        _ => None,
    }
}

/// Detect SQL statement fragments (multi-line continuation from Postgres, etc.)
fn is_sql_fragment(upper: &str) -> bool {
    let trimmed = upper.trim_start();
    const SQL_KEYWORDS: &[&str] = &[
        "SELECT ", "INSERT ", "UPDATE ", "DELETE ", "VALUES ", "SET ",
        "WHERE ", "FROM ", "JOIN ", "LEFT ", "RIGHT ", "INNER ", "OUTER ",
        "ON CONFLICT", "DO UPDATE", "DO NOTHING", "RETURNING ",
        "ORDER BY", "GROUP BY", "HAVING ", "LIMIT ", "OFFSET ",
        "CREATE ", "ALTER ", "DROP ", "EXCLUDED.", "INTO ",
        ") VALUES", "$1", "CONFLICT (",
    ];
    SQL_KEYWORDS.iter().any(|kw| trimmed.contains(kw))
}

pub fn normalize(line: &str) -> String {
    let a = ISO_TS.replace_all(line, "<TS>");
    let a2 = SYSLOG_TS.replace_all(&a, "<TS>");
    let b = UUID_RE.replace_all(&a2, "<UUID>");
    let c = IP_RE.replace_all(&b, "<IP>");
    let d = HEX_RE.replace_all(&c, "<HEX>");
    let e = DUR_RE.replace_all(&d, "<DUR>");
    let f = PID_RE.replace_all(&e, "[PID]");
    let g = NUM_RE.replace_all(&f, "<NUM>");
    g.into_owned()
}

/// Fields pulled from a structured (JSON or logfmt) line.
struct Structured {
    level: Option<Level>,
    message: Option<String>,
}

const JSON_LEVEL_KEYS: &[&str] = &["level", "lvl", "severity", "log.level", "@l", "levelname", "loglevel"];
const JSON_MSG_KEYS: &[&str] = &["@mt", "msg", "message", "@m", "@message", "event", "log", "MESSAGE"];

fn json_str(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_json(line: &str) -> Option<Structured> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let obj = value.as_object()?;
    let mut level = JSON_LEVEL_KEYS
        .iter()
        .filter_map(|k| obj.get(*k).and_then(json_str))
        .find_map(|s| level_from_str(&s));
    // ECS nests it: {"log": {"level": "error"}}
    if level.is_none() {
        level = obj
            .get("log")
            .and_then(|l| l.get("level"))
            .and_then(json_str)
            .and_then(|s| level_from_str(&s));
    }
    let message = JSON_MSG_KEYS
        .iter()
        .filter_map(|k| obj.get(*k))
        .find_map(|v| v.as_str().map(|s| s.trim_end().to_string()));
    Some(Structured { level, message })
}

fn parse_logfmt(line: &str) -> Option<Structured> {
    let caps = LOGFMT_MSG.captures(line)?;
    let message = caps
        .get(1)
        .map(|m| m.as_str().replace("\\\"", "\""))
        .or_else(|| caps.get(2).map(|m| m.as_str().to_string()));
    let upper = line.to_ascii_uppercase();
    let level = LOGFMT_LEVEL
        .captures(&upper)
        .and_then(|c| level_from_str(&c[1]));
    Some(Structured { level, message })
}

pub fn parse_line(source: &str, line: &str) -> LogEvent {
    let clean = crate::util::strip_ansi(line);
    let structured = parse_json(&clean).or_else(|| parse_logfmt(&clean));
    let (level, normalized) = match structured {
        Some(st) => {
            let level = st
                .level
                .unwrap_or_else(|| detect_level(st.message.as_deref().unwrap_or(&clean)));
            // Cluster on the message alone, so request ids, timings and other
            // fields that vary per line don't split one pattern into many.
            let normalized = match st.message {
                Some(m) if !m.is_empty() => normalize(&m),
                _ => normalize(&clean),
            };
            (level, normalized)
        }
        None => (detect_level(&clean), normalize(&clean)),
    };
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
    fn detect_level_redis_info() {
        assert_eq!(
            detect_level("12345:M 20 Feb 2026 15:03:24.123 * Background saving terminated with success"),
            Level::Info
        );
    }

    #[test]
    fn detect_level_redis_warning() {
        assert_eq!(
            detect_level("12345:M 20 Feb 2026 15:03:24.123 # WARNING overcommit_memory is set to 0"),
            Level::Warn
        );
    }

    #[test]
    fn detect_level_redis_debug() {
        assert_eq!(
            detect_level("12345:M 20 Feb 2026 15:03:24.123 . some debug info"),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_redis_verbose() {
        assert_eq!(
            detect_level("12345:C 20 Feb 2026 15:03:24.123 - Connecting to MASTER"),
            Level::Info
        );
    }

    #[test]
    fn detect_level_sql_insert() {
        assert_eq!(
            detect_level("            INSERT INTO pending_contracts ("),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_sql_values() {
        assert_eq!(
            detect_level("            ) VALUES ($1, $2, $3, $4)"),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_sql_on_conflict() {
        assert_eq!(
            detect_level("            ON CONFLICT (exchange_product_code_id, maturity_year_month)"),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_sql_do_update() {
        assert_eq!(
            detect_level("            DO UPDATE SET"),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_sql_returning() {
        assert_eq!(
            detect_level("            RETURNING id"),
            Level::Debug
        );
    }

    #[test]
    fn detect_level_sql_select() {
        assert_eq!(
            detect_level("  SELECT os.id, u.symbol, pg.id AS product_group_id"),
            Level::Debug
        );
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
    fn normalize_bracketed_pid() {
        let out = normalize("2026-02-20 15:03:24 UTC [12345] LOG:  checkpoint starting");
        assert!(out.contains("[PID]"), "got: {}", out);
        assert!(!out.contains("[12345]"));
        assert!(!out.contains("[<NUM>]"));
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

    #[test]
    fn detect_level_ignores_partial_words() {
        assert_eq!(detect_level("0 errors found"), Level::Unknown);
        assert_eq!(detect_level("stats error_count=0 ok"), Level::Unknown);
        assert_eq!(detect_level("ErrorBoundary mounted"), Level::Unknown);
    }

    #[test]
    fn detect_level_prefers_explicit_level() {
        assert_eq!(detect_level("[INFO] retried after error"), Level::Info);
        assert_eq!(detect_level("ts=1 level=warn msg=\"error budget low\""), Level::Warn);
    }

    #[test]
    fn level_from_str_names_and_numbers() {
        assert_eq!(level_from_str("warning"), Some(Level::Warn));
        assert_eq!(level_from_str("Fatal"), Some(Level::Error));
        assert_eq!(level_from_str("30"), Some(Level::Info));
        assert_eq!(level_from_str("50"), Some(Level::Error));
        assert_eq!(level_from_str("banana"), None);
    }

    #[test]
    fn parse_json_line() {
        let ev = parse_line(
            "src",
            r#"{"time":"2026-01-01T00:00:00Z","level":"error","msg":"db timeout after 512ms","req":"abc-123"}"#,
        );
        assert_eq!(ev.level, Level::Error);
        assert_eq!(ev.normalized, "db timeout after <DUR>");
    }

    #[test]
    fn parse_json_clusters_on_message() {
        let a = parse_line("s", r#"{"level":"info","msg":"user login","user":"alice","took":12}"#);
        let b = parse_line("s", r#"{"level":"info","msg":"user login","user":"bob","took":40}"#);
        assert_eq!(a.normalized, b.normalized);
    }

    #[test]
    fn parse_json_numeric_and_nested_levels() {
        assert_eq!(parse_line("s", r#"{"level":40,"msg":"slow"}"#).level, Level::Warn);
        assert_eq!(
            parse_line("s", r#"{"log":{"level":"error"},"message":"boom"}"#).level,
            Level::Error
        );
    }

    #[test]
    fn parse_json_without_level_falls_back_to_message() {
        assert_eq!(parse_line("s", r#"{"msg":"ERROR disk full"}"#).level, Level::Error);
    }

    #[test]
    fn parse_invalid_json_is_plain_text() {
        let ev = parse_line("s", "{not json} ERROR here");
        assert_eq!(ev.level, Level::Error);
    }

    #[test]
    fn parse_logfmt_line() {
        let ev = parse_line(
            "s",
            r#"ts=2026-01-01T00:00:00Z level=error msg="payment failed for order 42" order=42"#,
        );
        assert_eq!(ev.level, Level::Error);
        assert_eq!(ev.normalized, "payment failed for order <NUM>");
    }
}
