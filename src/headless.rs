//! Modes without the TUI: `summary` (read once, print the top patterns) and
//! `watch` (follow sources, print new patterns and alerts as they happen).

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc;

use crate::alert::{self, AlertEngine};
use crate::ingest::{self, Follow, SourceEvent, SourceStatus};
use crate::parse::{self, Level};
use crate::pattern::{Clustering, Pattern, PatternStore, MAX_PATTERNS};

/// What to read and how to analyse it.
pub struct Options {
    pub docker: Vec<String>,
    pub cmd: Vec<String>,
    pub file: Vec<String>,
    pub clustering: Clustering,
    pub min_level: Level,
}

impl Options {
    fn has_sources(&self) -> bool {
        !(self.docker.is_empty() && self.cmd.is_empty() && self.file.is_empty())
    }

    /// Start every source; the returned receiver closes once all have ended.
    fn spawn(&self, follow: Follow) -> mpsc::Receiver<SourceEvent> {
        let (tx, rx) = mpsc::channel(4096);
        for c in &self.docker {
            ingest::spawn_docker(c.clone(), follow, tx.clone());
        }
        for c in &self.cmd {
            ingest::spawn_command(command_name(c), c.clone(), tx.clone());
        }
        for f in &self.file {
            ingest::spawn_file(f.clone(), follow, tx.clone());
        }
        rx
    }
}

fn command_name(cmd: &str) -> String {
    cmd.split_whitespace().next().unwrap_or("cmd").to_string()
}

/// Counts of lines read and kept, and sources that failed.
#[derive(Default)]
struct Counts {
    lines: u64,
    kept: u64,
    source_errors: u64,
}

/// Apply one source event to the store. Source errors go to stderr.
fn apply(event: SourceEvent, store: &mut PatternStore, min_level: Level, counts: &mut Counts) {
    match event {
        SourceEvent::Log { source, line } => {
            counts.lines += 1;
            let ev = parse::parse_line(&source, &line);
            if ev.level.severity() >= min_level.severity() {
                counts.kept += 1;
                store.ingest(&ev);
            }
        }
        SourceEvent::Status {
            source,
            status: SourceStatus::Error(e),
        } => {
            counts.source_errors += 1;
            eprintln!("logradar: {}: {}", source, crate::util::sanitize(&e));
        }
        SourceEvent::Status { .. } => {}
    }
}

// --- summary ---

pub struct SummaryOptions {
    pub top: usize,
    pub json: bool,
    /// Exit with status 1 if any pattern is at this level or above.
    pub fail_on: Option<Level>,
}

/// Read every source to its end, then print the top patterns. Returns the
/// process exit code: 0, 1 if `fail_on` matched, 2 if a source failed.
pub async fn summary(opts: Options, out: SummaryOptions) -> Result<i32> {
    if !opts.has_sources() {
        anyhow::bail!("no sources: pass --file (use '-' for stdin), --cmd or --docker");
    }
    let mut store = PatternStore::with_options(opts.clustering, MAX_PATTERNS);
    let mut counts = Counts::default();
    let mut rx = opts.spawn(Follow::No);
    while let Some(ev) = rx.recv().await {
        apply(ev, &mut store, opts.min_level, &mut counts);
    }

    let mut ranked: Vec<&Pattern> = store.patterns().iter().collect();
    ranked.sort_by(|a, b| {
        b.count_total
            .cmp(&a.count_total)
            .then(b.level.severity().cmp(&a.level.severity()))
            .then(a.canonical.cmp(&b.canonical))
    });

    let stdout = std::io::stdout();
    let mut w = stdout.lock();
    if out.json {
        let patterns: Vec<serde_json::Value> = ranked
            .iter()
            .take(out.top)
            .map(|p| {
                let mut sources: Vec<&String> = p.sources.iter().collect();
                sources.sort();
                serde_json::json!({
                    "pattern": p.canonical,
                    "level": p.level.as_str(),
                    "count": p.count_total,
                    "sources": sources,
                    "first_sample": p.samples.front(),
                    "last_sample": p.samples.back(),
                })
            })
            .collect();
        let doc = serde_json::json!({
            "lines": counts.lines,
            "matched": counts.kept,
            "min_level": opts.min_level.as_str(),
            "total_patterns": store.len(),
            "patterns": patterns,
        });
        writeln!(w, "{}", serde_json::to_string_pretty(&doc)?)?;
    } else {
        let width = ranked.iter().take(out.top).map(|p| p.count_total.to_string().len()).max().unwrap_or(5).max(5);
        writeln!(w, "{:>width$}  {:<5}  PATTERN", "COUNT", "LEVEL", width = width)?;
        for p in ranked.iter().take(out.top) {
            writeln!(w, "{:>width$}  {:<5}  {}", p.count_total, p.level.as_str(), p.canonical, width = width)?;
        }
        writeln!(
            w,
            "\n{} patterns from {} lines ({} at {} or above){}",
            store.len(),
            counts.lines,
            counts.kept,
            opts.min_level.as_str(),
            if store.len() > out.top {
                format!("; showing top {}", out.top)
            } else {
                String::new()
            }
        )?;
    }
    w.flush()?;

    if counts.source_errors > 0 {
        return Ok(2);
    }
    if let Some(threshold) = out.fail_on {
        let failing = ranked.iter().filter(|p| p.level.severity() >= threshold.severity()).count();
        if failing > 0 {
            eprintln!(
                "logradar: {} pattern(s) at {} or above",
                failing,
                threshold.as_str()
            );
            return Ok(1);
        }
    }
    Ok(0)
}

// --- watch ---

pub struct WatchOptions {
    pub json: bool,
    /// Print each pattern the first time it appears.
    pub show_new: bool,
}

fn clock() -> String {
    jiff::ZonedDateTime::now().strftime("%H:%M:%S").to_string()
}

/// Follow sources with no UI: print new patterns and fired alerts as lines
/// (or JSON lines) until the sources end or Ctrl-C.
pub async fn watch(opts: Options, out: WatchOptions, mut alerts: AlertEngine) -> Result<()> {
    if !opts.has_sources() {
        anyhow::bail!("no sources: pass --file (use '-' for stdin), --cmd or --docker");
    }
    if alerts.is_empty() && !out.show_new {
        anyhow::bail!("nothing to report: add [[alerts]] to the config or drop --no-new");
    }
    let mut store = PatternStore::with_options(opts.clustering, MAX_PATTERNS);
    let mut counts = Counts::default();
    let mut rx = opts.spawn(Follow::Yes);
    let mut next_new_id = 0u64;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let (open, ticked) = tokio::select! {
            ev = rx.recv() => match ev {
                Some(ev) => { apply(ev, &mut store, opts.min_level, &mut counts); (true, false) }
                None => (false, true),
            },
            _ = tick.tick() => (true, true),
            _ = tokio::signal::ctrl_c() => (false, true),
        };

        // New patterns print as soon as they appear.
        if out.show_new && store.next_id() > next_new_id {
            for p in store.patterns().iter().filter(|p| p.id >= next_new_id) {
                print_new(p, out.json)?;
            }
            next_new_id = store.next_id();
        }
        // Rates and alerts are evaluated once a second, and once more at the end.
        if ticked {
            store.tick();
            for firing in alerts.evaluate(store.patterns(), Instant::now()) {
                print_alert(&firing, out.json)?;
                alert::deliver(firing);
            }
        }
        if !open {
            break;
        }
    }
    // Let in-flight alert deliveries finish (they are capped at 15s).
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}

fn print_new(p: &Pattern, json: bool) -> Result<()> {
    let mut w = std::io::stdout().lock();
    if json {
        let line = serde_json::json!({
            "event": "new_pattern",
            "time": clock(),
            "pattern": p.canonical,
            "level": p.level.as_str(),
            "source": p.sources.iter().next(),
            "sample": p.samples.back(),
        });
        writeln!(w, "{}", line)?;
    } else {
        writeln!(w, "{} NEW   {:<5} {}", clock(), p.level.as_str(), p.canonical)?;
    }
    w.flush()?;
    Ok(())
}

fn print_alert(f: &alert::Firing, json: bool) -> Result<()> {
    let mut w = std::io::stdout().lock();
    if json {
        let mut v: serde_json::Value = serde_json::from_str(&f.webhook_body())?;
        v["event"] = "alert".into();
        v["time"] = clock().into();
        writeln!(w, "{}", v)?;
    } else {
        writeln!(w, "{} ALERT {}", clock(), f.summary())?;
    }
    w.flush()?;
    Ok(())
}
