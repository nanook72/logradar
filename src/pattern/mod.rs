use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::parse::{Level, LogEvent};

pub mod drain;

use drain::{ClusterId, Drain};

/// How log lines are grouped into patterns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clustering {
    /// Lines whose normalized text is identical form a pattern.
    Exact,
    /// Drain template mining: lines of the same shape that differ in a few
    /// words share a template with `<*>` wildcards. `similarity` is the share
    /// of tokens that must match (0.0-1.0).
    Drain { similarity: f64 },
}

impl Default for Clustering {
    fn default() -> Self {
        Clustering::Drain { similarity: drain::DEFAULT_SIMILARITY }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    Up,
    Down,
    Stable,
}

impl Trend {
    pub fn symbol(self) -> &'static str {
        match self {
            Trend::Up => "↑",
            Trend::Down => "↓",
            Trend::Stable => "→",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Pattern {
    /// Stable id, unique for the life of the store (templates can change).
    pub id: u64,
    pub canonical: String,
    pub level: Level,
    pub count_total: u64,
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub samples: VecDeque<String>,
    pub trend: Trend,
    pub spike: bool,
    pub sources: HashSet<String>,
    /// The Drain cluster behind this pattern, when clustering with Drain.
    pub cluster: Option<ClusterId>,
    /// Completed sparkline buckets (each = events in one SPARKLINE_BUCKET_SECS window).
    pub sparkline_buckets: VecDeque<u16>,
    /// In-progress bucket count (not yet committed to sparkline_buckets).
    pub current_bucket_count: u16,
    sparkline_last_advance: Instant,
    timestamps_1m: VecDeque<Instant>,
    timestamps_5m: VecDeque<Instant>,
    /// How much of the 5-minute window has actually been observed, so the
    /// 5m rate isn't diluted while logradar has been running for less than 5m.
    observed_5m: Duration,
}

const WINDOW_1M: Duration = Duration::from_secs(60);
const WINDOW_5M: Duration = Duration::from_secs(300);
const MAX_SAMPLES: usize = 10;
const SPARKLINE_BUCKET_SECS: u64 = 5;
const SPARKLINE_BUCKET_COUNT: usize = 24;
/// Most recent lines kept (across all patterns) for drill-down and context.
/// Each is at most `parse::MAX_STORED_BYTES`, so this is ~40 MB at worst.
pub const RECENT_LINES: usize = 10_000;

/// One line as it arrived, for drill-down.
#[derive(Debug, Clone)]
pub struct RecentLine {
    /// Arrival order; consecutive across the whole store.
    pub seq: u64,
    pub at: std::time::SystemTime,
    pub source: String,
    pub pattern_id: u64,
    pub level: Level,
    pub raw: String,
}

/// Most patterns kept at once; past this the least recently seen is dropped,
/// so high-cardinality logs can't grow memory without bound.
pub const MAX_PATTERNS: usize = 5000;

impl Pattern {
    fn new(canonical: String, level: Level, raw: String, source: String, now: Instant) -> Self {
        let mut samples = VecDeque::with_capacity(MAX_SAMPLES);
        samples.push_back(raw);
        let mut ts1 = VecDeque::new();
        ts1.push_back(now);
        let mut ts5 = VecDeque::new();
        ts5.push_back(now);
        let mut sources = HashSet::new();
        sources.insert(source);
        let sparkline_buckets = VecDeque::with_capacity(SPARKLINE_BUCKET_COUNT);
        Pattern {
            id: 0,
            canonical,
            level,
            count_total: 1,
            first_seen: now,
            last_seen: now,
            samples,
            trend: Trend::Stable,
            spike: false,
            sources,
            cluster: None,
            sparkline_buckets,
            current_bucket_count: 1,
            sparkline_last_advance: now,
            timestamps_1m: ts1,
            timestamps_5m: ts5,
            observed_5m: Duration::ZERO,
        }
    }

    fn record(&mut self, raw: String, level: Level, source: &str, now: Instant) {
        self.sources.insert(source.to_string());
        self.count_total += 1;
        self.last_seen = now;
        if level.severity() > self.level.severity() {
            self.level = level;
        }
        if self.samples.len() >= MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(raw);
        self.timestamps_1m.push_back(now);
        self.timestamps_5m.push_back(now);
        self.current_bucket_count = self.current_bucket_count.saturating_add(1);
    }

    pub fn rate_1m(&self) -> f64 {
        self.timestamps_1m.len() as f64
    }

    /// Events per minute averaged over the last 5 minutes, or over the time
    /// observed so far (at least 1 minute) when that is shorter.
    pub fn rate_5m(&self) -> f64 {
        let minutes = (self.observed_5m.as_secs_f64() / 60.0).clamp(1.0, 5.0);
        self.timestamps_5m.len() as f64 / minutes
    }

    fn prune_windows(&mut self, now: Instant, origin: Instant) {
        self.observed_5m = now.duration_since(origin).min(WINDOW_5M);
        while let Some(&front) = self.timestamps_1m.front() {
            if now.duration_since(front) > WINDOW_1M {
                self.timestamps_1m.pop_front();
            } else {
                break;
            }
        }
        while let Some(&front) = self.timestamps_5m.front() {
            if now.duration_since(front) > WINDOW_5M {
                self.timestamps_5m.pop_front();
            } else {
                break;
            }
        }
        // Advance sparkline buckets: commit current_bucket_count, then add empty buckets for elapsed intervals
        let bucket_dur = Duration::from_secs(SPARKLINE_BUCKET_SECS);
        let mut advanced = false;
        while now.duration_since(self.sparkline_last_advance) >= bucket_dur {
            self.sparkline_last_advance += bucket_dur;
            if !advanced {
                // First advance: commit the in-progress bucket
                self.sparkline_buckets.push_back(self.current_bucket_count);
                self.current_bucket_count = 0;
                advanced = true;
            } else {
                // Subsequent advances: empty buckets for time gaps
                self.sparkline_buckets.push_back(0);
            }
            if self.sparkline_buckets.len() > SPARKLINE_BUCKET_COUNT {
                self.sparkline_buckets.pop_front();
            }
        }
    }

    fn update_trend(&mut self) {
        let r1 = self.rate_1m();
        let r5 = self.rate_5m();
        if r5 < 0.1 {
            self.trend = if r1 > 0.0 { Trend::Up } else { Trend::Stable };
        } else if r1 > r5 * 1.5 {
            self.trend = Trend::Up;
        } else if r1 < r5 * 0.5 {
            self.trend = Trend::Down;
        } else {
            self.trend = Trend::Stable;
        }
        self.spike = r1 > r5 * 3.0 && self.count_total > 10;
    }
}

pub struct PatternStore {
    patterns: Vec<Pattern>,
    index: HashMap<String, usize>,
    drain: Option<Drain>,
    by_cluster: HashMap<ClusterId, usize>,
    /// When counting started (store creation or the last clear/reset).
    origin: Instant,
    max_patterns: usize,
    evicted: u64,
    next_id: u64,
    recent: VecDeque<RecentLine>,
    next_seq: u64,
}

impl PatternStore {
    pub fn new() -> Self {
        Self::with_options(Clustering::default(), MAX_PATTERNS)
    }

    #[cfg(test)]
    pub fn with_capacity_limit(max_patterns: usize) -> Self {
        Self::with_options(Clustering::Exact, max_patterns)
    }

    pub fn with_options(clustering: Clustering, max_patterns: usize) -> Self {
        PatternStore {
            patterns: Vec::new(),
            index: HashMap::new(),
            drain: match clustering {
                Clustering::Exact => None,
                Clustering::Drain { similarity } => Some(Drain::new(similarity)),
            },
            by_cluster: HashMap::new(),
            origin: Instant::now(),
            max_patterns: max_patterns.max(1),
            evicted: 0,
            next_id: 0,
            recent: VecDeque::new(),
            next_seq: 0,
        }
    }

    pub fn ingest(&mut self, event: &LogEvent) {
        let now = Instant::now();
        let existing = match self.drain.as_mut() {
            Some(drain) => {
                let m = drain.add(event.level, &event.normalized);
                match self.by_cluster.get(&m.id) {
                    Some(&idx) => {
                        if m.changed {
                            self.patterns[idx].canonical = m.template;
                        }
                        Ok(idx)
                    }
                    None => Err((m.template, Some(m.id))),
                }
            }
            None => match self.index.get(event.normalized.as_str()) {
                Some(&idx) => Ok(idx),
                None => Err((event.normalized.clone(), None)),
            },
        };

        let pattern_id = match existing {
            Ok(idx) => {
                self.patterns[idx].record(event.raw.clone(), event.level, &event.source, now);
                self.patterns[idx].id
            }
            Err((canonical, cluster)) => {
                if self.patterns.len() >= self.max_patterns {
                    self.evict_stalest();
                }
                let idx = self.patterns.len();
                let mut pattern = Pattern::new(
                    canonical,
                    event.level,
                    event.raw.clone(),
                    event.source.clone(),
                    now,
                );
                pattern.observed_5m = now.duration_since(self.origin).min(WINDOW_5M);
                pattern.cluster = cluster;
                pattern.id = self.next_id;
                self.next_id += 1;
                match cluster {
                    Some(id) => {
                        self.by_cluster.insert(id, idx);
                    }
                    None => {
                        self.index.insert(pattern.canonical.clone(), idx);
                    }
                }
                let id = pattern.id;
                self.patterns.push(pattern);
                id
            }
        };

        if self.recent.len() >= RECENT_LINES {
            self.recent.pop_front();
        }
        self.recent.push_back(RecentLine {
            seq: self.next_seq,
            at: std::time::SystemTime::now(),
            source: event.source.clone(),
            pattern_id,
            level: event.level,
            raw: event.raw.clone(),
        });
        self.next_seq += 1;
    }

    /// Recent lines of one pattern, oldest first.
    pub fn recent_lines(&self, pattern_id: u64) -> Vec<&RecentLine> {
        self.recent.iter().filter(|l| l.pattern_id == pattern_id).collect()
    }

    /// Up to `n` lines from the same source before and after line `seq`
    /// (any pattern), oldest first; the line itself is included.
    pub fn context(&self, seq: u64, n: usize) -> Vec<&RecentLine> {
        let Some(base) = self.recent.front().map(|l| l.seq) else {
            return Vec::new();
        };
        let Some(pos) = seq.checked_sub(base).map(|p| p as usize).filter(|&p| p < self.recent.len()) else {
            return Vec::new();
        };
        let source = &self.recent[pos].source;
        let before: Vec<&RecentLine> = self.recent.range(..pos).rev().filter(|l| &l.source == source).take(n).collect();
        let after = self.recent.range(pos + 1..).filter(|l| &l.source == source).take(n);
        before.into_iter().rev().chain(std::iter::once(&self.recent[pos])).chain(after).collect()
    }

    /// Drop the pattern seen least recently.
    fn evict_stalest(&mut self) {
        let Some(victim) = self
            .patterns
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| p.last_seen)
            .map(|(i, _)| i)
        else {
            return;
        };
        let removed = self.patterns.swap_remove(victim);
        match removed.cluster {
            Some(id) => {
                self.by_cluster.remove(&id);
                if let Some(drain) = self.drain.as_mut() {
                    drain.remove(id);
                }
            }
            None => {
                self.index.remove(&removed.canonical);
            }
        }
        if let Some(moved) = self.patterns.get(victim) {
            match moved.cluster {
                Some(id) => self.by_cluster.insert(id, victim),
                None => self.index.insert(moved.canonical.clone(), victim),
            };
        }
        self.evicted += 1;
    }

    /// The id the next new pattern will get; every existing id is lower.
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// How many patterns have been dropped to stay under the cap.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let origin = self.origin;
        for p in &mut self.patterns {
            p.prune_windows(now, origin);
            p.update_trend();
        }
    }

    pub fn patterns(&self) -> &[Pattern] {
        &self.patterns
    }

    pub fn sorted_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.patterns.len()).collect();
        indices.sort_by(|&a, &b| {
            let pa = &self.patterns[a];
            let pb = &self.patterns[b];
            pb.rate_1m()
                .partial_cmp(&pa.rate_1m())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| pb.last_seen.cmp(&pa.last_seen))
        });
        indices
    }

    pub fn clear_counters(&mut self) {
        let now = std::time::Instant::now();
        self.origin = now;
        for p in &mut self.patterns {
            p.count_total = 0;
            p.timestamps_1m.clear();
            p.timestamps_5m.clear();
            p.sparkline_buckets.clear();
            p.current_bucket_count = 0;
            p.sparkline_last_advance = now;
            p.trend = Trend::Stable;
            p.spike = false;
            p.observed_5m = Duration::ZERO;
        }
    }

    pub fn reset(&mut self) {
        self.patterns.clear();
        self.index.clear();
        self.by_cluster.clear();
        self.recent.clear();
        if let Some(drain) = self.drain.as_mut() {
            drain.clear();
        }
        self.origin = Instant::now();
        self.evicted = 0;
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn make_event(normalized: &str, raw: &str, level: Level) -> LogEvent {
        LogEvent {
            level,
            source: "test".into(),
            raw: raw.into(),
            normalized: normalized.into(),
        }
    }

    #[test]
    fn ingest_creates_pattern() {
        let mut store = PatternStore::new();
        let ev = make_event("GET /api/<NUM>", "GET /api/42", Level::Info);
        store.ingest(&ev);
        assert_eq!(store.len(), 1);
        assert_eq!(store.patterns()[0].count_total, 1);
        assert_eq!(store.patterns()[0].canonical, "GET /api/<NUM>");
    }

    #[test]
    fn duplicate_normalized_clusters() {
        let mut store = PatternStore::new();
        let ev1 = make_event("GET /api/<NUM>", "GET /api/1", Level::Info);
        let ev2 = make_event("GET /api/<NUM>", "GET /api/2", Level::Info);
        let ev3 = make_event("GET /api/<NUM>", "GET /api/3", Level::Info);
        store.ingest(&ev1);
        store.ingest(&ev2);
        store.ingest(&ev3);
        assert_eq!(store.len(), 1);
        assert_eq!(store.patterns()[0].count_total, 3);
    }

    #[test]
    fn different_normalized_separate_patterns() {
        let mut store = PatternStore::new();
        let ev1 = make_event("GET /api/<NUM>", "GET /api/1", Level::Info);
        let ev2 = make_event("POST /api/<NUM>", "POST /api/1", Level::Info);
        store.ingest(&ev1);
        store.ingest(&ev2);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn samples_capped_at_max() {
        let mut store = PatternStore::new();
        for i in 0..15 {
            let ev = make_event("pattern", &format!("raw {}", i), Level::Info);
            store.ingest(&ev);
        }
        assert_eq!(store.patterns()[0].samples.len(), 10);
        assert_eq!(store.patterns()[0].samples.back().unwrap(), "raw 14");
    }

    #[test]
    fn level_escalates() {
        let mut store = PatternStore::with_options(Clustering::Exact, MAX_PATTERNS);
        let ev1 = make_event("same", "info line", Level::Info);
        let ev2 = make_event("same", "error line", Level::Error);
        store.ingest(&ev1);
        store.ingest(&ev2);
        assert_eq!(store.patterns()[0].level, Level::Error);
    }

    #[test]
    fn rate_1m_counts_events() {
        let mut store = PatternStore::new();
        for _ in 0..5 {
            let ev = make_event("p", "r", Level::Info);
            store.ingest(&ev);
        }
        assert_eq!(store.patterns()[0].rate_1m(), 5.0);
    }

    #[test]
    fn clear_counters_resets() {
        let mut store = PatternStore::new();
        let ev = make_event("p", "r", Level::Info);
        store.ingest(&ev);
        store.ingest(&ev);
        store.clear_counters();
        assert_eq!(store.patterns()[0].count_total, 0);
        assert_eq!(store.patterns()[0].rate_1m(), 0.0);
    }

    #[test]
    fn reset_clears_all() {
        let mut store = PatternStore::new();
        let ev = make_event("p", "r", Level::Info);
        store.ingest(&ev);
        store.reset();
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn sorted_indices_by_rate() {
        let mut store = PatternStore::new();
        // Pattern A: 1 event
        store.ingest(&make_event("A", "a", Level::Info));
        // Pattern B: 3 events
        for _ in 0..3 {
            store.ingest(&make_event("B", "b", Level::Info));
        }
        let sorted = store.sorted_indices();
        // B should come first (higher rate)
        assert_eq!(store.patterns()[sorted[0]].canonical, "B");
        assert_eq!(store.patterns()[sorted[1]].canonical, "A");
    }

    #[test]
    fn sparkline_accumulates_in_current_bucket() {
        let mut store = PatternStore::new();
        for _ in 0..5 {
            store.ingest(&make_event("p", "r", Level::Info));
        }
        let p = &store.patterns()[0];
        // All 5 events should be in current_bucket_count (not yet committed)
        assert_eq!(p.current_bucket_count, 5);
        // No completed buckets yet (haven't ticked past a bucket interval)
        assert_eq!(p.sparkline_buckets.len(), 0);
    }

    #[test]
    fn integration_with_parse() {
        let mut store = PatternStore::new();
        let ev = parse::parse_line("src", "2025-01-01T00:00:00Z [ERROR] timeout from 10.0.0.1 after 500ms");
        store.ingest(&ev);
        assert_eq!(store.len(), 1);
        let p = &store.patterns()[0];
        assert_eq!(p.level, Level::Error);
        assert!(p.canonical.contains("<TS>"));
        assert!(p.canonical.contains("<IP>"));
        assert!(p.canonical.contains("<DUR>"));
    }

    #[test]
    fn no_false_spike_at_startup() {
        let mut store = PatternStore::new();
        for _ in 0..200 {
            store.ingest(&make_event("burst <NUM>", "burst 1", Level::Info));
        }
        store.tick();
        let p = &store.patterns()[0];
        assert_eq!(p.rate_1m(), 200.0);
        assert_eq!(p.rate_5m(), 200.0);
        assert!(!p.spike);
    }

    #[test]
    fn spike_detected_after_quiet_history() {
        let mut store = PatternStore::new();
        store.ingest(&make_event("x", "x", Level::Info));
        let p = &mut store.patterns[0];
        let now = Instant::now();
        // Pretend 5 minutes were observed with 12 events in the last minute and
        // 1 more spread over the earlier four.
        p.observed_5m = WINDOW_5M;
        for _ in 0..12 {
            p.record("x".into(), Level::Info, "s", now);
        }
        p.update_trend();
        assert!(p.spike, "1m={} 5m={}", p.rate_1m(), p.rate_5m());
    }

    #[test]
    fn evicts_least_recently_seen_at_cap() {
        let mut store = PatternStore::with_capacity_limit(3);
        for name in ["a", "b", "c"] {
            store.ingest(&make_event(name, name, Level::Info));
            std::thread::sleep(Duration::from_millis(2));
        }
        // Touch "a" so "b" becomes the stalest.
        store.ingest(&make_event("a", "a", Level::Info));
        std::thread::sleep(Duration::from_millis(2));
        store.ingest(&make_event("d", "d", Level::Info));

        assert_eq!(store.len(), 3);
        assert_eq!(store.evicted(), 1);
        let names: HashSet<&str> = store.patterns().iter().map(|p| p.canonical.as_str()).collect();
        assert_eq!(names, HashSet::from(["a", "c", "d"]));
        // Index still points at the right rows after the swap_remove.
        store.ingest(&make_event("c", "c", Level::Info));
        let c = store.patterns().iter().find(|p| p.canonical == "c").unwrap();
        assert_eq!(c.count_total, 2);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn drain_merges_lines_that_differ_in_a_word() {
        let mut store = PatternStore::new();
        for user in ["alice", "bob", "carol"] {
            let line = format!("user {} logged in", user);
            store.ingest(&make_event(&line, &line, Level::Info));
        }
        store.ingest(&make_event("cache warmed", "cache warmed", Level::Info));
        assert_eq!(store.len(), 2);
        let p = store.patterns().iter().find(|p| p.count_total == 3).unwrap();
        assert_eq!(p.canonical, "user <*> logged in");
        // Raw samples keep the real lines.
        assert_eq!(p.samples.back().unwrap(), "user carol logged in");
    }

    #[test]
    fn drain_keeps_levels_apart() {
        let mut store = PatternStore::new();
        store.ingest(&make_event("payment for order <NUM> ok", "a", Level::Info));
        store.ingest(&make_event("payment for order <NUM> failed", "b", Level::Error));
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn drain_eviction_keeps_index_consistent() {
        let mut store = PatternStore::with_options(Clustering::default(), 2);
        for (i, line) in ["alpha one", "beta two words here", "gamma"].iter().enumerate() {
            store.ingest(&make_event(line, line, Level::Info));
            if i < 2 {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(store.len(), 2);
        assert_eq!(store.evicted(), 1);
        // The survivors still receive their own lines.
        store.ingest(&make_event("gamma", "gamma", Level::Info));
        store.ingest(&make_event("beta two words here", "b", Level::Info));
        for p in store.patterns() {
            assert_eq!(p.count_total, 2, "{}", p.canonical);
        }
        // The evicted shape starts a fresh pattern.
        store.ingest(&make_event("alpha one", "alpha one", Level::Info));
        assert_eq!(store.len(), 2);
        assert!(store.patterns().iter().any(|p| p.canonical == "alpha one" && p.count_total == 1));
    }

    #[test]
    fn recent_lines_and_context() {
        let mut store = PatternStore::new();
        let feed = [
            ("a", "boot start"),
            ("b", "other source noise"),
            ("a", "[ERROR] db timeout"),
            ("a", "retrying now"),
            ("a", "[ERROR] db timeout"),
            ("a", "gave up"),
        ];
        for (src, line) in feed {
            let mut ev = make_event(line, line, Level::Info);
            ev.source = src.into();
            store.ingest(&ev);
        }
        let timeout_id = store.patterns().iter().find(|p| p.canonical.contains("timeout")).unwrap().id;
        let lines = store.recent_lines(timeout_id);
        assert_eq!(lines.len(), 2);
        // Context of the first timeout: same source only, 1 each side.
        let ctx: Vec<&str> = store.context(lines[0].seq, 1).iter().map(|l| l.raw.as_str()).collect();
        assert_eq!(ctx, ["boot start", "[ERROR] db timeout", "retrying now"]);
        let ctx: Vec<&str> = store.context(lines[1].seq, 5).iter().map(|l| l.raw.as_str()).collect();
        assert_eq!(ctx.len(), 5);
        assert!(!ctx.contains(&"other source noise"));
        assert!(store.context(9999, 1).is_empty());
    }

    #[test]
    fn recent_lines_are_bounded() {
        let mut store = PatternStore::new();
        for i in 0..(RECENT_LINES + 50) {
            let line = format!("line {}", i);
            store.ingest(&make_event("line <NUM>", &line, Level::Info));
        }
        let id = store.patterns()[0].id;
        let lines = store.recent_lines(id);
        assert_eq!(lines.len(), RECENT_LINES);
        assert_eq!(lines[0].raw, "line 50");
        // seq arithmetic still finds lines after the front was dropped
        assert_eq!(store.context(lines[10].seq, 0)[0].raw, "line 60");
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `cargo test --release -- --ignored throughput --nocapture`
    #[test]
    #[ignore]
    fn throughput() {
        let path = std::env::var("LOGRADAR_BENCH_LOG").expect("set LOGRADAR_BENCH_LOG");
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let events: Vec<LogEvent> = lines.iter().map(|l| crate::parse::parse_line("b", l)).collect();
        for (name, mode) in [("exact", Clustering::Exact), ("drain", Clustering::default())] {
            let mut store = PatternStore::with_options(mode, MAX_PATTERNS);
            let start = Instant::now();
            for _ in 0..100 {
                for ev in &events {
                    store.ingest(ev);
                }
            }
            let n = events.len() * 100;
            let secs = start.elapsed().as_secs_f64();
            eprintln!("{name}: {} lines in {:.3}s = {:.0} lines/s, {} patterns", n, secs, n as f64 / secs, store.len());
        }
        let start = Instant::now();
        for l in &lines {
            std::hint::black_box(crate::parse::parse_line("b", l));
        }
        let secs = start.elapsed().as_secs_f64();
        eprintln!("parse_line: {:.0} lines/s", lines.len() as f64 / secs);
    }
}
