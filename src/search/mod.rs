use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

use crate::pattern::Pattern;

pub struct SearchResult {
    pub index: usize,
    pub score: i64,
    pub matched_indices: Vec<usize>,
}

pub fn fuzzy_search(query: &str, patterns: &[Pattern], sorted_indices: &[usize]) -> Vec<SearchResult> {
    if query.is_empty() {
        return sorted_indices
            .iter()
            .map(|&i| SearchResult {
                index: i,
                score: 0,
                matched_indices: vec![],
            })
            .collect();
    }

    let matcher = SkimMatcherV2::default();
    let mut results: Vec<SearchResult> = Vec::new();

    for &idx in sorted_indices {
        let pattern = &patterns[idx];
        if let Some((score, indices)) = matcher.fuzzy_indices(&pattern.canonical, query) {
            results.push(SearchResult {
                index: idx,
                score,
                matched_indices: indices,
            });
        }
    }

    results.sort_by(|a, b| b.score.cmp(&a.score));
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Level;
    use crate::pattern::PatternStore;

    fn build_store(canonicals: &[&str]) -> PatternStore {
        let mut store = PatternStore::new();
        for &c in canonicals {
            let ev = crate::parse::LogEvent {
                level: Level::Info,
                source: "test".into(),
                raw: c.into(),
                normalized: c.into(),
            };
            store.ingest(&ev);
        }
        store
    }

    #[test]
    fn empty_query_returns_all() {
        let store = build_store(&["foo", "bar", "baz"]);
        let indices = store.sorted_indices();
        let results = fuzzy_search("", store.patterns(), &indices);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn search_filters_non_matching() {
        let store = build_store(&["GET /api/users", "POST /api/orders", "DELETE /api/users"]);
        let indices = store.sorted_indices();
        let results = fuzzy_search("users", store.patterns(), &indices);
        assert_eq!(results.len(), 2);
        for r in &results {
            assert!(store.patterns()[r.index].canonical.contains("users"));
        }
    }

    #[test]
    fn search_returns_matched_indices() {
        let store = build_store(&["hello world"]);
        let indices = store.sorted_indices();
        let results = fuzzy_search("hlo", store.patterns(), &indices);
        assert_eq!(results.len(), 1);
        assert!(!results[0].matched_indices.is_empty());
    }

    #[test]
    fn search_scores_better_match_higher() {
        let store = build_store(&["ab_cd_ef", "abcdef"]);
        let indices = store.sorted_indices();
        let results = fuzzy_search("abcdef", store.patterns(), &indices);
        assert!(results.len() >= 1);
        // Exact or near-exact match should score highest
        let top = &results[0];
        assert!(store.patterns()[top.index].canonical.contains("abcdef"));
    }

    #[test]
    fn no_match_returns_empty() {
        let store = build_store(&["GET /api/users"]);
        let indices = store.sorted_indices();
        let results = fuzzy_search("zzzzzzz", store.patterns(), &indices);
        assert!(results.is_empty());
    }
}
