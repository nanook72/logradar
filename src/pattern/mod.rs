use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::parse::{Level, LogEvent};

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
    pub canonical: String,
    pub level: Level,
    pub count_total: u64,
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub samples: VecDeque<String>,
    pub trend: Trend,
    pub spike: bool,
    pub sources: HashSet<String>,
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
            canonical,
            level,
            count_total: 1,
            first_seen: now,
            last_seen: now,
            samples,
            trend: Trend::Stable,
            spike: false,
            sources,
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
    /// When counting started (store creation or the last clear/reset).
    origin: Instant,
    max_patterns: usize,
    evicted: u64,
}

impl PatternStore {
    pub fn new() -> Self {
        Self::with_capacity_limit(MAX_PATTERNS)
    }

    pub fn with_capacity_limit(max_patterns: usize) -> Self {
        PatternStore {
            patterns: Vec::new(),
            index: HashMap::new(),
            origin: Instant::now(),
            max_patterns: max_patterns.max(1),
            evicted: 0,
        }
    }

    pub fn ingest(&mut self, event: &LogEvent) {
        let now = Instant::now();
        if let Some(&idx) = self.index.get(event.normalized.as_str()) {
            self.patterns[idx].record(event.raw.clone(), event.level, &event.source, now);
        } else {
            if self.patterns.len() >= self.max_patterns {
                self.evict_stalest();
            }
            let idx = self.patterns.len();
            self.patterns.push(Pattern::new(
                event.normalized.clone(),
                event.level,
                event.raw.clone(),
                event.source.clone(),
                now,
            ));
            self.patterns[idx].observed_5m = now.duration_since(self.origin).min(WINDOW_5M);
            self.index.insert(event.normalized.clone(), idx);
        }
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
        self.index.remove(&removed.canonical);
        if let Some(moved) = self.patterns.get(victim) {
            self.index.insert(moved.canonical.clone(), victim);
        }
        self.evicted += 1;
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
        let mut store = PatternStore::new();
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
}
