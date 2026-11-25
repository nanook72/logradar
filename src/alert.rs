//! Alert rules: watch patterns and run a command or call a webhook when one
//! appears, spikes or crosses a rate.
//!
//! Log content is attacker-controlled, so it never reaches a shell: a
//! command is an argv list and each `{placeholder}` fills exactly one
//! argument; webhook URLs and bodies go to curl on stdin, never its command
//! line (webhook URLs are often secrets). Firing is rate-limited per pattern
//! (cooldown) and overall (`max_per_minute`), and patterns first seen during
//! the startup grace period (e.g. while a file's history is read) don't count
//! as new.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::parse::{level_from_str, Level};
use crate::pattern::Pattern;

/// What makes a rule fire for a matching pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trigger {
    /// The pattern appeared for the first time.
    FirstSeen,
    /// The pattern's 1-minute rate jumped over 3x its 5-minute average.
    Spike,
    /// The pattern's 1-minute rate reached this many events per minute.
    Rate(f64),
}

/// `[[alerts]]` as written in the config file.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AlertConfig {
    pub name: String,
    /// Glob over the pattern's template (`*` and `?`, case-insensitive).
    #[serde(default, rename = "match")]
    pub pattern: Option<String>,
    /// Glob over the source id, e.g. `docker/*`.
    #[serde(default)]
    pub source: Option<String>,
    /// Lowest level that counts, e.g. "warn".
    #[serde(default)]
    pub level: Option<String>,
    /// "first_seen", "spike" or "rate".
    pub when: String,
    /// For `when = "rate"`: events per minute.
    #[serde(default)]
    pub rate_per_min: Option<f64>,
    /// Quiet time per pattern after firing, e.g. "30s", "5m", "1h".
    #[serde(default)]
    pub cooldown: Option<String>,
    /// Program and arguments; never run through a shell.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// URL that receives a JSON POST.
    #[serde(default)]
    pub webhook: Option<String>,
}

/// `[alert_settings]`: limits that apply to all rules.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct AlertSettings {
    /// Most alerts (commands + webhooks) per minute across all rules.
    #[serde(default = "default_max_per_minute")]
    pub max_per_minute: u32,
    /// Patterns first seen in this long after start are not "new".
    #[serde(default = "default_startup_grace")]
    pub startup_grace: String,
}

impl Default for AlertSettings {
    fn default() -> Self {
        AlertSettings {
            max_per_minute: default_max_per_minute(),
            startup_grace: default_startup_grace(),
        }
    }
}

fn default_max_per_minute() -> u32 {
    10
}

fn default_startup_grace() -> String {
    "10s".into()
}

/// A validated rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pattern: Option<String>,
    source: Option<String>,
    min_level: Option<Level>,
    pub trigger: Trigger,
    cooldown: Duration,
    command: Option<Vec<String>>,
    webhook: Option<String>,
}

/// Parse "30s", "5m", "1h" or a bare number of seconds.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, "s"),
    };
    let n: u64 = num.parse().with_context(|| format!("bad duration '{}'", s))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => bail!("bad duration '{}' (use s, m or h)", s),
    };
    Ok(Duration::from_secs(secs))
}

impl Rule {
    pub fn from_config(c: &AlertConfig) -> Result<Rule> {
        let trigger = match c.when.as_str() {
            "first_seen" => Trigger::FirstSeen,
            "spike" => Trigger::Spike,
            "rate" => match c.rate_per_min {
                Some(r) if r > 0.0 => Trigger::Rate(r),
                _ => bail!("alert '{}': when = \"rate\" needs rate_per_min > 0", c.name),
            },
            other => bail!(
                "alert '{}': unknown when = \"{}\" (use first_seen, spike or rate)",
                c.name,
                other
            ),
        };
        let min_level = match &c.level {
            Some(l) => Some(
                level_from_str(l).with_context(|| format!("alert '{}': unknown level '{}'", c.name, l))?,
            ),
            None => None,
        };
        if c.command.is_none() && c.webhook.is_none() {
            bail!("alert '{}': needs a command or a webhook", c.name);
        }
        if let Some(cmd) = &c.command {
            if cmd.is_empty() || cmd[0].is_empty() {
                bail!("alert '{}': command must name a program", c.name);
            }
        }
        if let Some(url) = &c.webhook {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                bail!("alert '{}': webhook must be an http(s) URL", c.name);
            }
        }
        let cooldown = match &c.cooldown {
            Some(d) => parse_duration(d).with_context(|| format!("alert '{}'", c.name))?,
            None => Duration::from_secs(300),
        };
        Ok(Rule {
            name: c.name.clone(),
            pattern: c.pattern.clone(),
            source: c.source.clone(),
            min_level,
            trigger,
            cooldown,
            command: c.command.clone(),
            webhook: c.webhook.clone(),
        })
    }

    fn matches(&self, p: &Pattern) -> bool {
        if let Some(min) = self.min_level {
            if p.level.severity() < min.severity() {
                return false;
            }
        }
        if let Some(glob) = &self.pattern {
            if !glob_match(glob, &p.canonical) {
                return false;
            }
        }
        if let Some(glob) = &self.source {
            if !p.sources.iter().any(|s| glob_match(glob, s)) {
                return false;
            }
        }
        true
    }
}

/// Case-insensitive glob with `*` (any run) and `?` (one char).
pub fn glob_match(glob: &str, text: &str) -> bool {
    let g: Vec<char> = glob.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut gi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if gi < g.len() && (g[gi] == '?' || g[gi] == t[ti]) {
            gi += 1;
            ti += 1;
        } else if gi < g.len() && g[gi] == '*' {
            star = Some((gi, ti));
            gi += 1;
        } else if let Some((sg, st)) = star {
            gi = sg + 1;
            ti = st + 1;
            star = Some((sg, st + 1));
        } else {
            return false;
        }
    }
    g[gi..].iter().all(|&c| c == '*')
}

/// One alert to deliver.
#[derive(Debug, Clone, PartialEq)]
pub struct Firing {
    pub rule: String,
    pub trigger: Trigger,
    pub pattern: String,
    pub level: Level,
    pub count: u64,
    pub rate_1m: f64,
    pub source: String,
    pub sample: String,
    command: Option<Vec<String>>,
    webhook: Option<String>,
}

impl Firing {
    /// Short line for the status bar / stderr.
    pub fn summary(&self) -> String {
        let why = match self.trigger {
            Trigger::FirstSeen => "new pattern".to_string(),
            Trigger::Spike => "spike".to_string(),
            Trigger::Rate(r) => format!("rate ≥ {}/m", r),
        };
        format!("{} ({}): {} [{:.0}/m]", self.rule, why, self.pattern, self.rate_1m)
    }

    fn placeholder(&self, key: &str) -> Option<String> {
        Some(match key {
            "name" => self.rule.clone(),
            "pattern" => self.pattern.clone(),
            "level" => self.level.as_str().to_string(),
            "count" => self.count.to_string(),
            "rate" => format!("{:.1}", self.rate_1m),
            "source" => self.source.clone(),
            "sample" => self.sample.clone(),
            "summary" => self.summary(),
            _ => return None,
        })
    }

    /// Fill `{placeholders}` in one argument. The result is always a single
    /// argument, whatever the values contain. Unknown `{x}` stay as written.
    fn fill(&self, arg: &str) -> String {
        let mut out = String::with_capacity(arg.len());
        let mut rest = arg;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            match after.find('}') {
                Some(close) => match self.placeholder(&after[..close]) {
                    Some(v) => {
                        out.push_str(&v);
                        rest = &after[close + 1..];
                    }
                    None => {
                        out.push('{');
                        rest = after;
                    }
                },
                None => {
                    out.push_str(&rest[open..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// The command's argv with placeholders filled.
    pub fn argv(&self) -> Option<Vec<String>> {
        self.command.as_ref().map(|c| c.iter().map(|a| self.fill(a)).collect())
    }

    /// JSON body for the webhook. `text` makes it readable in Slack, Teams
    /// (via workflow) and Discord-compatible receivers.
    pub fn webhook_body(&self) -> String {
        serde_json::json!({
            "text": format!("logradar: {}", self.summary()),
            "content": format!("logradar: {}", self.summary()),
            "alert": self.rule,
            "trigger": match self.trigger {
                Trigger::FirstSeen => "first_seen",
                Trigger::Spike => "spike",
                Trigger::Rate(_) => "rate",
            },
            "pattern": self.pattern,
            "level": self.level.as_str(),
            "count": self.count,
            "rate_per_min": self.rate_1m,
            "source": self.source,
            "sample": self.sample,
        })
        .to_string()
    }
}

pub struct AlertEngine {
    rules: Vec<Rule>,
    started: Instant,
    grace: Duration,
    max_per_minute: u32,
    /// Next pattern id not yet checked for `first_seen`.
    next_unseen_id: u64,
    last_fired: HashMap<(usize, u64), Instant>,
    recent: VecDeque<Instant>,
    /// Alerts dropped by `max_per_minute`.
    pub suppressed: u64,
}

impl AlertEngine {
    pub fn new(rules: Vec<Rule>, settings: &AlertSettings) -> Result<Self> {
        Ok(AlertEngine {
            rules,
            started: Instant::now(),
            grace: parse_duration(&settings.startup_grace).context("alert_settings.startup_grace")?,
            max_per_minute: settings.max_per_minute,
            next_unseen_id: 0,
            last_fired: HashMap::new(),
            recent: VecDeque::new(),
            suppressed: 0,
        })
    }

    pub fn from_config(configs: &[AlertConfig], settings: &AlertSettings) -> Result<Self> {
        let rules = configs.iter().map(Rule::from_config).collect::<Result<Vec<_>>>()?;
        Self::new(rules, settings)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Check every pattern against every rule and return what should fire now.
    pub fn evaluate(&mut self, patterns: &[Pattern], now: Instant) -> Vec<Firing> {
        let mut out = Vec::new();
        let in_grace = now.duration_since(self.started) < self.grace;
        let mut max_id = self.next_unseen_id;

        while self.recent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.recent.pop_front();
        }

        for p in patterns {
            let is_new = p.id >= self.next_unseen_id;
            max_id = max_id.max(p.id + 1);
            for (ri, rule) in self.rules.iter().enumerate() {
                let triggered = match rule.trigger {
                    Trigger::FirstSeen => is_new && !in_grace,
                    Trigger::Spike => p.spike,
                    Trigger::Rate(r) => p.rate_1m() >= r,
                };
                if !triggered || !rule.matches(p) {
                    continue;
                }
                let key = (ri, p.id);
                if let Some(last) = self.last_fired.get(&key) {
                    if now.duration_since(*last) < rule.cooldown {
                        continue;
                    }
                }
                if self.recent.len() >= self.max_per_minute as usize {
                    self.suppressed += 1;
                    continue;
                }
                self.last_fired.insert(key, now);
                self.recent.push_back(now);
                out.push(Firing {
                    rule: rule.name.clone(),
                    trigger: rule.trigger,
                    pattern: p.canonical.clone(),
                    level: p.level,
                    count: p.count_total,
                    rate_1m: p.rate_1m(),
                    source: p.sources.iter().next().cloned().unwrap_or_default(),
                    sample: p.samples.back().cloned().unwrap_or_default(),
                    command: rule.command.clone(),
                    webhook: rule.webhook.clone(),
                });
            }
        }
        self.next_unseen_id = max_id;
        // Forget cooldowns for patterns that no longer exist.
        if self.last_fired.len() > 4 * patterns.len().max(64) {
            let live: std::collections::HashSet<u64> = patterns.iter().map(|p| p.id).collect();
            self.last_fired.retain(|(_, id), _| live.contains(id));
        }
        out
    }
}

/// Longest an alert command or webhook call may run.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Run the firing's command and webhook in the background. Output is
/// discarded so nothing writes over the TUI.
pub fn deliver(firing: Firing) {
    if let Some(argv) = firing.argv() {
        tokio::spawn(async move {
            let child = tokio::process::Command::new(&argv[0])
                .args(&argv[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn();
            if let Ok(mut child) = child {
                let _ = tokio::time::timeout(DELIVERY_TIMEOUT, child.wait()).await;
            }
        });
    }
    if let Some(url) = firing.webhook.clone() {
        let body = firing.webhook_body();
        tokio::spawn(async move {
            let _ = post_webhook(&url, &body).await;
        });
    }
}

/// curl config that POSTs `body` as JSON to `url`. Both go on stdin so the
/// URL (often a secret) and the log text never appear in the process list.
pub fn webhook_curl_config(url: &str, body: &str) -> String {
    format!(
        "url = \"{}\"\nheader = \"Content-Type: application/json\"\ndata-binary = \"{}\"\n",
        crate::ingest::curl_quote(url),
        crate::ingest::curl_quote(body)
    )
}

async fn post_webhook(url: &str, body: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("curl")
        .args(["-sf", "-o", "/dev/null", "--max-time", "10", "-K", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(webhook_curl_config(url, body).as_bytes()).await?;
    }
    let _ = tokio::time::timeout(DELIVERY_TIMEOUT, child.wait()).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_line, Level};
    use crate::pattern::{Clustering, PatternStore, MAX_PATTERNS};

    fn rule(toml_src: &str) -> Rule {
        let c: AlertConfig = toml::from_str(toml_src).unwrap();
        Rule::from_config(&c).unwrap()
    }

    fn engine(rules: Vec<Rule>, grace: &str, max: u32) -> AlertEngine {
        AlertEngine::new(
            rules,
            &AlertSettings {
                max_per_minute: max,
                startup_grace: grace.into(),
            },
        )
        .unwrap()
    }

    fn store_with(lines: &[&str]) -> PatternStore {
        let mut s = PatternStore::with_options(Clustering::default(), MAX_PATTERNS);
        for l in lines {
            s.ingest(&parse_line("docker/api", l));
        }
        s
    }

    const CMD: &str = r#"command = ["true"]"#;

    #[test]
    fn glob() {
        assert!(glob_match("db timeout*", "DB Timeout after <DUR>"));
        assert!(glob_match("*timeout*", "[ERROR] db timeout"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("docker/*", "docker/api"));
        assert!(!glob_match("docker/*", "file/x"));
        assert!(glob_match("*a*b*c*", "xxaxxbxxcxx"));
        assert!(!glob_match("*a*b*c", "xxaxxbxxcxx"));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
        assert!(parse_duration("5 days").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn config_validation() {
        let bad = [
            r#"name="a"
               when="sometimes"
               command=["true"]"#,
            r#"name="a"
               when="rate"
               command=["true"]"#,
            r#"name="a"
               when="spike""#,
            r#"name="a"
               when="spike"
               command=[]"#,
            r#"name="a"
               when="spike"
               level="loud"
               command=["true"]"#,
            r#"name="a"
               when="spike"
               webhook="file:///etc/passwd""#,
            r#"name="a"
               when="spike"
               cooldown="soon"
               command=["true"]"#,
        ];
        for src in bad {
            let c: AlertConfig = toml::from_str(src).unwrap();
            assert!(Rule::from_config(&c).is_err(), "{}", src);
        }
        assert!(toml::from_str::<AlertConfig>("name=\"a\"\nwhen=\"spike\"\ncomand=[\"x\"]").is_err());
    }

    #[test]
    fn first_seen_fires_once_and_respects_grace() {
        let r = rule(&format!("name=\"new\"\nwhen=\"first_seen\"\n{}", CMD));
        let mut store = store_with(&["[ERROR] disk full on /var"]);

        // During grace: history being read doesn't count as new.
        let mut e = engine(vec![r.clone()], "1h", 10);
        assert!(e.evaluate(store.patterns(), Instant::now()).is_empty());
        store.ingest(&parse_line("s", "[ERROR] brand new failure"));
        assert!(e.evaluate(store.patterns(), Instant::now()).is_empty());

        // No grace: fires once per new pattern.
        let mut e = engine(vec![r], "0s", 10);
        let first = e.evaluate(store.patterns(), Instant::now());
        assert_eq!(first.len(), 2);
        assert!(e.evaluate(store.patterns(), Instant::now()).is_empty());
        store.ingest(&parse_line("s", "[WARN] something else entirely happened here"));
        let next = e.evaluate(store.patterns(), Instant::now());
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].trigger, Trigger::FirstSeen);
    }

    #[test]
    fn filters_by_level_pattern_and_source() {
        let store = store_with(&["[ERROR] db timeout after 30ms", "[INFO] request ok", "[ERROR] other thing broke"]);
        let r = rule(&format!(
            "name=\"db\"\nwhen=\"rate\"\nrate_per_min=1\nlevel=\"error\"\nmatch=\"*timeout*\"\nsource=\"docker/*\"\n{}",
            CMD
        ));
        let mut e = engine(vec![r], "0s", 10);
        let fired = e.evaluate(store.patterns(), Instant::now());
        assert_eq!(fired.len(), 1, "{:?}", fired);
        assert!(fired[0].pattern.contains("timeout"));

        let r = rule(&format!("name=\"x\"\nwhen=\"rate\"\nrate_per_min=1\nsource=\"file/*\"\n{}", CMD));
        let mut e = engine(vec![r], "0s", 10);
        assert!(e.evaluate(store.patterns(), Instant::now()).is_empty());
    }

    #[test]
    fn rate_threshold_and_cooldown() {
        let mut store = store_with(&[]);
        for _ in 0..30 {
            store.ingest(&parse_line("s", "[ERROR] db timeout"));
        }
        let r = rule(&format!("name=\"r\"\nwhen=\"rate\"\nrate_per_min=20\ncooldown=\"5m\"\n{}", CMD));
        let mut e = engine(vec![r], "0s", 100);
        let t0 = Instant::now();
        assert_eq!(e.evaluate(store.patterns(), t0).len(), 1);
        assert!(e.evaluate(store.patterns(), t0 + Duration::from_secs(60)).is_empty());
        assert_eq!(e.evaluate(store.patterns(), t0 + Duration::from_secs(301)).len(), 1);

        let r = rule(&format!("name=\"r\"\nwhen=\"rate\"\nrate_per_min=31\n{}", CMD));
        let mut e = engine(vec![r], "0s", 100);
        assert!(e.evaluate(store.patterns(), t0).is_empty());
    }

    /// An attacker who can write logs can mint new patterns at will; alerts
    /// must not turn that into unbounded process spawning.
    #[test]
    fn global_cap_limits_alert_storms() {
        let mut store = store_with(&[]);
        let r = rule(&format!("name=\"new\"\nwhen=\"first_seen\"\n{}", CMD));
        let mut e = engine(vec![r], "0s", 5);
        e.evaluate(store.patterns(), Instant::now());
        for i in 0..50 {
            let words: String = (0..i + 1).map(|_| "w ").collect();
            store.ingest(&parse_line("s", &format!("[ERROR] {}", words)));
        }
        let t0 = Instant::now();
        assert_eq!(e.evaluate(store.patterns(), t0).len(), 5);
        assert_eq!(e.suppressed, 45);
        // The window slides: a minute later there is room again.
        store.ingest(&parse_line("s", "[ERROR] one more new kind of failure"));
        assert_eq!(e.evaluate(store.patterns(), t0 + Duration::from_secs(61)).len(), 1);
    }

    /// Log text filled into a command stays one argument: no word splitting,
    /// no shell, no extra options.
    #[test]
    fn placeholders_cannot_inject_arguments() {
        let evil = "[ERROR] x; rm -rf ~ $(touch /tmp/pwned) `id` --exec=evil \"quoted\" {name}";
        let store = store_with(&[evil]);
        let r = rule(r#"name="n"
            when="rate"
            rate_per_min=1
            command=["notify", "--title={name}", "{sample}", "{pattern}", "{nope}", "{unclosed"]"#);
        let mut e = engine(vec![r], "0s", 10);
        let fired = e.evaluate(store.patterns(), Instant::now());
        let argv = fired[0].argv().unwrap();
        assert_eq!(argv.len(), 6, "{:?}", argv);
        assert_eq!(argv[0], "notify");
        assert_eq!(argv[1], "--title=n");
        assert_eq!(argv[2], evil, "sample is passed through as one argument, and its own {{name}} is not expanded");
        assert_eq!(argv[4], "{nope}");
        assert_eq!(argv[5], "{unclosed");
    }

    #[test]
    fn command_really_runs_without_a_shell() {
        let dir = std::env::temp_dir().join(format!("logradar-alert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.txt");
        let canary = dir.join("pwned");
        let evil = format!("[ERROR] boom $(touch {}) ; touch {}", canary.display(), canary.display());
        let store = store_with(&[&evil]);
        // A program that writes its single argument to a file.
        let r = rule(&format!(
            "name=\"n\"\nwhen=\"rate\"\nrate_per_min=1\ncommand=[\"sh\", \"-c\", 'printf \"%s\" \"$1\" > \"{}\"', \"sh\", \"{{sample}}\"]",
            out.display()
        ));
        let mut e = engine(vec![r], "0s", 10);
        let argv = e.evaluate(store.patterns(), Instant::now())[0].argv().unwrap();
        let status = std::process::Command::new(&argv[0]).args(&argv[1..]).status().unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), evil);
        assert!(!canary.exists(), "log text was executed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn webhook_payload_is_json_and_config_is_one_value_per_line() {
        let evil = "[ERROR] \"quote\" \\ back\nslash url = \"http://evil\"";
        let store = store_with(&[evil]);
        let r = rule(r#"name="hook"
            when="rate"
            rate_per_min=1
            webhook="https://hooks.example.invalid/T000/secret""#);
        let mut e = engine(vec![r], "0s", 10);
        let f = &e.evaluate(store.patterns(), Instant::now())[0];
        let body: serde_json::Value = serde_json::from_str(&f.webhook_body()).unwrap();
        assert_eq!(body["alert"], "hook");
        assert_eq!(body["sample"], f.sample);
        let cfg = webhook_curl_config("https://hooks.example.invalid/T000/secret", &f.webhook_body());
        assert_eq!(cfg.lines().count(), 3, "{}", cfg);
        assert!(cfg.lines().all(|l| l.starts_with("url = ") || l.starts_with("header = ") || l.starts_with("data-binary = ")));
    }

    #[test]
    fn summary_reads_well() {
        let store = store_with(&["[ERROR] db timeout"]);
        let r = rule(&format!("name=\"db\"\nwhen=\"rate\"\nrate_per_min=1\n{}", CMD));
        let mut e = engine(vec![r], "0s", 10);
        let f = &e.evaluate(store.patterns(), Instant::now())[0];
        assert_eq!(f.summary(), "db (rate ≥ 1/m): [ERROR] db timeout [1/m]");
        assert_eq!(Level::Error, f.level);
    }
}
