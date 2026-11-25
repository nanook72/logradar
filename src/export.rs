//! Export the current pattern view as a Markdown report or JSON, and copy
//! text to the clipboard through the terminal (OSC 52).

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::pattern::Pattern;

/// What the export describes, for the report header.
pub struct Meta {
    pub sources: Vec<String>,
    pub events: u64,
    pub sort: String,
    pub filter: String,
}

/// A Markdown inline code span that shows `s` literally: the fence is longer
/// than any run of backticks inside it.
fn code_span(s: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{s}{pad}{fence}")
}

/// A table cell: code span with pipes escaped (GFM splits cells on `|` even
/// inside code spans).
fn cell(s: &str) -> String {
    code_span(s).replace('|', "\\|")
}

pub fn markdown(patterns: &[&Pattern], meta: &Meta) -> String {
    let now = jiff::ZonedDateTime::now().strftime("%Y-%m-%d %H:%M:%S %Z").to_string();
    let mut out = String::new();
    out.push_str("# logradar report\n\n");
    out.push_str(&format!("- Generated: {}\n", now));
    let sources: Vec<String> = meta.sources.iter().map(|s| code_span(s)).collect();
    out.push_str(&format!("- Sources: {}\n", if sources.is_empty() { "none".into() } else { sources.join(", ") }));
    out.push_str(&format!("- Events: {}\n", meta.events));
    out.push_str(&format!("- Patterns: {} (sorted by {}, {})\n\n", patterns.len(), meta.sort, meta.filter));
    out.push_str("| Count | Level | /min | Pattern | Latest sample |\n");
    out.push_str("|------:|:------|-----:|:--------|:--------------|\n");
    for p in patterns {
        out.push_str(&format!(
            "| {} | {} | {:.1} | {} | {} |\n",
            p.count_total,
            p.level.as_str(),
            p.rate_1m(),
            cell(&p.canonical),
            p.samples.back().map(|s| cell(s)).unwrap_or_default(),
        ));
    }
    out
}

pub fn json(patterns: &[&Pattern], meta: &Meta) -> String {
    let doc = serde_json::json!({
        "generated": jiff::ZonedDateTime::now().to_string(),
        "sources": meta.sources,
        "events": meta.events,
        "sort": meta.sort,
        "filter": meta.filter,
        "patterns": patterns.iter().map(|p| {
            let mut sources: Vec<&String> = p.sources.iter().collect();
            sources.sort();
            serde_json::json!({
                "pattern": p.canonical,
                "level": p.level.as_str(),
                "count": p.count_total,
                "rate_per_min": p.rate_1m(),
                "sources": sources,
                "samples": p.samples,
            })
        }).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// Write `content` to a new `logradar-<time>.<ext>` in the current directory,
/// never overwriting an existing file.
pub fn write_new(content: &str, ext: &str) -> Result<PathBuf> {
    let stamp = jiff::ZonedDateTime::now().strftime("%Y%m%d-%H%M%S").to_string();
    for n in 0..100 {
        let name = if n == 0 {
            format!("logradar-{}.{}", stamp, ext)
        } else {
            format!("logradar-{}-{}.{}", stamp, n, ext)
        };
        let path = PathBuf::from(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(content.as_bytes()).with_context(|| format!("writing {}", name))?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", name)),
        }
    }
    anyhow::bail!("could not find a free file name for the export")
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The OSC 52 sequence that asks the terminal to put `text` on the
/// clipboard. The text is base64-encoded, so nothing in it can end the
/// sequence early or smuggle in other escapes.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Copy `text` to the clipboard via the terminal.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(osc52(text).as_bytes())?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_spans_survive_backticks() {
        assert_eq!(code_span("plain"), "`plain`");
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("a``b`"), "``` a``b` ```");
    }

    #[test]
    fn table_cells_escape_pipes() {
        assert_eq!(cell("a | b"), "`a \\| b`");
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn osc52_payload_cannot_break_out() {
        let seq = osc52("evil \x07\x1b]0;title\x07 \x1b\\ done");
        let payload = &seq["\x1b]52;c;".len()..seq.len() - 1];
        assert!(payload.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='));
        assert!(seq.ends_with('\x07'));
    }

    #[test]
    fn markdown_report_shape() {
        let mut store = crate::pattern::PatternStore::new();
        for l in ["[ERROR] db | timeout `x`", "[INFO] ok"] {
            store.ingest(&crate::parse::parse_line("src", l));
        }
        let ps: Vec<&Pattern> = store.patterns().iter().collect();
        let md = markdown(
            &ps,
            &Meta { sources: vec!["file/app.log".into()], events: 2, sort: "count".into(), filter: "all levels".into() },
        );
        let rows: Vec<&str> = md.lines().filter(|l| l.starts_with("| ") && !l.starts_with("| Count")).collect();
        assert_eq!(rows.len(), 2, "{}", md);
        assert!(rows[0].contains("``[ERROR] db \\| timeout `x` ``") || rows[0].contains("\\|"), "{}", rows[0]);
        let v: serde_json::Value = serde_json::from_str(&json(&ps, &Meta { sources: vec![], events: 2, sort: "rate".into(), filter: "all".into() })).unwrap();
        assert_eq!(v["patterns"].as_array().unwrap().len(), 2);
    }
}
