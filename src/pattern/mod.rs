use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
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
}

const WINDOW_1M: Duration = Duration::from_secs(60);
const WINDOW_5M: Duration = Duration::from_secs(300);
const MAX_SAMPLES: usize = 10;
const SPARKLINE_BUCKET_SECS: u64 = 5;
const SPARKLINE_BUCKET_COUNT: usize = 24;

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

    pub fn rate_5m(&self) -> f64 {
        self.timestamps_5m.len() as f64 / 5.0
    }

    fn prune_windows(&mut self, now: Instant) {
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

fn hash_str(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

pub struct PatternStore {
    patterns: Vec<Pattern>,
    index: HashMap<u64, usize>,
}

impl PatternStore {
    pub fn new() -> Self {
        PatternStore {
            patterns: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn ingest(&mut self, event: &LogEvent) {
        let now = Instant::now();
        let hash = hash_str(&event.normalized);
        if let Some(&idx) = self.index.get(&hash) {
            self.patterns[idx].record(event.raw.clone(), event.level, &event.source, now);
        } else {
            let idx = self.patterns.len();
            self.patterns.push(Pattern::new(
                event.normalized.clone(),
                event.level,
                event.raw.clone(),
                event.source.clone(),
                now,
            ));
            self.index.insert(hash, idx);
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        for p in &mut self.patterns {
            p.prune_windows(now);
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
        for p in &mut self.patterns {
            p.count_total = 0;
            p.timestamps_1m.clear();
            p.timestamps_5m.clear();
            p.sparkline_buckets.clear();
            p.current_bucket_count = 0;
            p.sparkline_last_advance = now;
            p.trend = Trend::Stable;
            p.spike = false;
        }
    }

    pub fn reset(&mut self) {
        self.patterns.clear();
        self.index.clear();
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
}
