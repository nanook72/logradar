//! Online log template mining, after Drain (He et al., ICWS 2017).
//!
//! Each event's normalized text is split into tokens. Lines with the same
//! level, token count and first token land in the same leaf; within a leaf the
//! line joins the most similar template if at least `similarity` of its tokens
//! match, and every position where they differ becomes the wildcard `<*>`.
//! So `user alice logged in` and `user bob logged in` become one template,
//! `user <*> logged in`, while lines of other shapes stay apart.

use std::collections::HashMap;

use crate::parse::Level;

/// Placeholder for a token that varies between lines of one template.
pub const WILDCARD: &str = "<*>";

/// Default share of tokens that must match for a line to join a template.
pub const DEFAULT_SIMILARITY: f64 = 0.6;

pub type ClusterId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LeafKey {
    level: Level,
    len: usize,
    first: String,
}

#[derive(Debug, Clone)]
struct Cluster {
    tokens: Vec<String>,
    key: LeafKey,
}

/// What `Drain::add` did with a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub id: ClusterId,
    /// The cluster's template after this line, tokens joined by single spaces.
    pub template: String,
    /// True when this line created the cluster.
    pub created: bool,
    /// True when this line widened an existing template with a new wildcard.
    pub changed: bool,
}

pub struct Drain {
    similarity: f64,
    clusters: HashMap<ClusterId, Cluster>,
    leaves: HashMap<LeafKey, Vec<ClusterId>>,
    next_id: ClusterId,
}

impl Default for Drain {
    fn default() -> Self {
        Self::new(DEFAULT_SIMILARITY)
    }
}

/// A token that is already a value placeholder (`<NUM>`, `<IP>`…) or holds a
/// digit. Such tokens never decide which leaf a line goes to, so lines that
/// differ only in values still meet.
fn is_variable(token: &str) -> bool {
    token == WILDCARD
        || (token.starts_with('<') && token.ends_with('>'))
        || token.bytes().any(|b| b.is_ascii_digit())
}

impl Drain {
    pub fn new(similarity: f64) -> Self {
        Drain {
            similarity: similarity.clamp(0.0, 1.0),
            clusters: HashMap::new(),
            leaves: HashMap::new(),
            next_id: 0,
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.clusters.len()
    }

    /// Add one line (already normalized) and return the cluster it joined.
    pub fn add(&mut self, level: Level, text: &str) -> Match {
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let key = LeafKey {
            level,
            len: tokens.len(),
            first: match tokens.first() {
                Some(t) if !is_variable(t) => (*t).to_string(),
                Some(_) => WILDCARD.to_string(),
                None => String::new(),
            },
        };

        if let Some(id) = self.best_match(&key, &tokens) {
            let cluster = self.clusters.get_mut(&id).expect("leaf ids are live");
            let mut changed = false;
            for (slot, token) in cluster.tokens.iter_mut().zip(&tokens) {
                if slot != token && slot != WILDCARD {
                    *slot = WILDCARD.to_string();
                    changed = true;
                }
            }
            return Match {
                id,
                template: cluster.tokens.join(" "),
                created: false,
                changed,
            };
        }

        let id = self.next_id;
        self.next_id += 1;
        let cluster = Cluster {
            tokens: tokens.iter().map(|t| (*t).to_string()).collect(),
            key: key.clone(),
        };
        let template = cluster.tokens.join(" ");
        self.clusters.insert(id, cluster);
        self.leaves.entry(key).or_default().push(id);
        Match {
            id,
            template,
            created: true,
            changed: false,
        }
    }

    /// The most similar cluster in the line's leaf, if similar enough. Ties go
    /// to the template with more wildcards (the more general one).
    fn best_match(&self, key: &LeafKey, tokens: &[&str]) -> Option<ClusterId> {
        let ids = self.leaves.get(key)?;
        if tokens.is_empty() {
            return ids.first().copied();
        }
        let mut best: Option<(ClusterId, usize, usize)> = None;
        for &id in ids {
            let cluster = &self.clusters[&id];
            let mut same = 0;
            let mut wild = 0;
            for (slot, token) in cluster.tokens.iter().zip(tokens) {
                if slot == WILDCARD {
                    wild += 1;
                } else if slot == token {
                    same += 1;
                }
            }
            let better = match best {
                None => true,
                Some((_, s, w)) => same > s || (same == s && wild > w),
            };
            if better {
                best = Some((id, same, wild));
            }
        }
        let (id, same, _) = best?;
        // Wildcards don't count as matches, so a template that is mostly
        // wildcards can't swallow unrelated lines.
        (same as f64 / tokens.len() as f64 >= self.similarity).then_some(id)
    }

    /// Forget a cluster (when its pattern is evicted).
    pub fn remove(&mut self, id: ClusterId) {
        if let Some(cluster) = self.clusters.remove(&id) {
            if let Some(ids) = self.leaves.get_mut(&cluster.key) {
                ids.retain(|&other| other != id);
                if ids.is_empty() {
                    self.leaves.remove(&cluster.key);
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.clusters.clear();
        self.leaves.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(d: &mut Drain, line: &str) -> Match {
        d.add(Level::Info, line)
    }

    #[test]
    fn varying_word_becomes_wildcard() {
        let mut d = Drain::default();
        let a = add(&mut d, "user alice logged in");
        let b = add(&mut d, "user bob logged in");
        assert_eq!(a.id, b.id);
        assert!(b.changed);
        assert_eq!(b.template, "user <*> logged in");
        let c = add(&mut d, "user carol logged in");
        assert_eq!(c.id, a.id);
        assert!(!c.changed);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn different_shapes_stay_apart() {
        let mut d = Drain::default();
        let a = add(&mut d, "user alice logged in");
        let b = add(&mut d, "user alice logged out of every session"); // other length
        let c = add(&mut d, "cache warmed for region east"); // other first token
        let e = add(&mut d, "user alice was deleted"); // too different
        assert_eq!(d.len(), 4, "{:?}", [a, b, c, e]);
    }

    #[test]
    fn levels_never_merge() {
        let mut d = Drain::default();
        let a = d.add(Level::Info, "payment for order <NUM> ok");
        let b = d.add(Level::Error, "payment for order <NUM> failed");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn variable_first_token_still_clusters() {
        let mut d = Drain::default();
        let a = add(&mut d, "<IP> GET /health <NUM>");
        let b = add(&mut d, "<IP> GET /metrics <NUM>");
        assert_eq!(a.id, b.id);
        assert_eq!(b.template, "<IP> GET <*> <NUM>");
    }

    #[test]
    fn similarity_threshold_is_respected() {
        let mut strict = Drain::new(0.9);
        let a = add(&mut strict, "user alice logged in");
        let b = add(&mut strict, "user bob logged in");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn wildcard_heavy_template_cannot_swallow_unrelated_lines() {
        let mut d = Drain::default();
        add(&mut d, "job a b c done");
        add(&mut d, "job d e f done"); // 2/5 = 0.4 < 0.6: separate
        assert_eq!(d.len(), 2);
        let mut d = Drain::new(0.4);
        add(&mut d, "job a b c done");
        let m = add(&mut d, "job d e f done");
        assert_eq!(m.template, "job <*> <*> <*> done");
        // 'job' and 'done' match: still 2/5, joins
        assert_eq!(add(&mut d, "job x y z done").id, m.id);
        // only 'job' matches: 1/5, does not join
        let other = add(&mut d, "job x y z failed");
        assert_ne!(other.id, m.id);
    }

    #[test]
    fn remove_forgets_cluster() {
        let mut d = Drain::default();
        let a = add(&mut d, "user alice logged in");
        d.remove(a.id);
        assert_eq!(d.len(), 0);
        let b = add(&mut d, "user bob logged in");
        assert!(b.created);
        assert_eq!(b.template, "user bob logged in");
    }

    #[test]
    fn empty_line() {
        let mut d = Drain::default();
        let a = add(&mut d, "");
        let b = add(&mut d, "   ");
        assert_eq!(a.id, b.id);
    }
}
