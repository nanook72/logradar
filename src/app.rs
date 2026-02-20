use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::discovery::DiscoveryResult;
use crate::ingest::{self, SourceEvent, SourceInfo, SourceStatus};
use crate::parse;
use crate::pattern::PatternStore;
use crate::profile::Profile;
use crate::search::{self, SearchResult};
use crate::theme::Theme;
use crate::tui::source_menu::SourceMenuState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    Search,
    Drilldown,
    Help,
    ProfilePicker,
    SourceMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sources,
    Patterns,
    Details,
}

pub struct App {
    pub mode: AppMode,
    pub active_pane: Pane,
    pub sources: Vec<SourceInfo>,
    pub store: PatternStore,
    pub selected_source: usize,
    pub selected_pattern: usize,
    pub search_query: String,
    pub filtered_view: Vec<SearchResult>,
    pub paused: bool,
    pub profiles: Vec<Profile>,
    pub profile_index: usize,
    pub theme_override: Option<Theme>,
    pub should_quit: bool,
    pub log_count: u64,
    pub show_normalized: bool,
    pub detail_scroll: usize,
    // Dynamic source support
    pub tx: Option<mpsc::Sender<SourceEvent>>,
    pub handles: HashMap<String, JoinHandle<()>>,
    pub tick_count: u64,
    // Source menu
    pub source_menu: SourceMenuState,
    pub discovery_tx: Option<mpsc::Sender<DiscoveryResult>>,
    // Set to true when closing a modal overlay so the terminal forces a full repaint
    pub needs_clear: bool,
    // Filter patterns to a specific source (None = show all)
    pub active_source_filter: Option<String>,
    // Per-source event rates (source_id → rolling 1m timestamps)
    pub source_rates: HashMap<String, VecDeque<Instant>>,
    // Collapsed provider groups in Sources pane
    pub collapsed_groups: HashSet<String>,
    // Cached Azure management access token (pre-fetched during discovery)
    pub azure_token: Option<String>,
    // Whether to show the ASCII banner header
    pub show_banner: bool,
}

impl App {
    #[allow(dead_code)]
    pub fn new(profile_name: Option<&str>) -> Self {
        Self::with_profiles(Profile::all_profiles(), profile_name)
    }

    pub fn with_profiles(profiles: Vec<Profile>, profile_name: Option<&str>) -> Self {
        let profile_index = profile_name
            .and_then(|name| profiles.iter().position(|p| p.name == name))
            .unwrap_or(0);

        App {
            mode: AppMode::Normal,
            active_pane: Pane::Patterns,
            sources: Vec::new(),
            store: PatternStore::new(),
            selected_source: 0,
            selected_pattern: 0,
            search_query: String::new(),
            filtered_view: Vec::new(),
            paused: false,
            profiles,
            profile_index,
            theme_override: None,
            should_quit: false,
            log_count: 0,
            show_normalized: false,
            detail_scroll: 0,
            tx: None,
            handles: HashMap::new(),
            tick_count: 0,
            source_menu: SourceMenuState::new(),
            discovery_tx: None,
            needs_clear: false,
            active_source_filter: None,
            source_rates: HashMap::new(),
            collapsed_groups: HashSet::new(),
            azure_token: None,
            show_banner: true,
        }
    }

    pub fn profile(&self) -> &Profile {
        &self.profiles[self.profile_index]
    }

    pub fn theme(&self) -> &Theme {
        if let Some(ref t) = self.theme_override {
            t
        } else {
            &self.profile().theme
        }
    }

    pub fn toggle_theme(&mut self) {
        self.theme_override = Some(self.theme().next());
    }

    pub fn process_event(&mut self, event: SourceEvent) {
        match event {
            SourceEvent::Log { source, line } => self.process_log(source, line),
            SourceEvent::Status { source, status } => self.update_source_status(&source, status),
        }
    }

    fn process_log(&mut self, source: String, line: String) {
        if self.paused {
            return;
        }
        // Track per-source rate
        self.source_rates
            .entry(source.clone())
            .or_default()
            .push_back(Instant::now());

        let log_event = parse::parse_line(&source, &line);
        if log_event.level.severity() >= self.profile().min_level.severity() {
            self.store.ingest(&log_event);
            self.log_count += 1;
        }
    }

    fn update_source_status(&mut self, source_id: &str, status: SourceStatus) {
        if let Some(src) = self.sources.iter_mut().find(|s| s.id == source_id) {
            src.status = status;
        }
    }

    /// Abort and remove a source by id (kills child process via kill_on_drop).
    #[allow(dead_code)]
    pub fn stop_source(&mut self, source_id: &str) {
        if let Some(handle) = self.handles.remove(source_id) {
            handle.abort();
        }
        if let Some(src) = self.sources.iter_mut().find(|s| s.id == source_id) {
            src.status = SourceStatus::Stopped;
        }
    }

    /// Prune old timestamps from per-source rate windows. Called each tick.
    pub fn tick_source_rates(&mut self) {
        let cutoff = Instant::now() - std::time::Duration::from_secs(60);
        for timestamps in self.source_rates.values_mut() {
            while timestamps.front().map_or(false, |t| *t < cutoff) {
                timestamps.pop_front();
            }
        }
    }

    /// Get 1-minute event rate for a specific source.
    pub fn source_rate_1m(&self, source_id: &str) -> f64 {
        self.source_rates
            .get(source_id)
            .map_or(0.0, |ts| ts.len() as f64)
    }

    /// Get total 1-minute rate for a provider kind (docker, azure, command, file).
    pub fn provider_rate_1m(&self, kind: &str) -> f64 {
        self.sources
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| self.source_rate_1m(&s.id))
            .sum()
    }

    pub fn toggle_source_group(&mut self, kind: &str) {
        let key = kind.to_string();
        if self.collapsed_groups.contains(&key) {
            self.collapsed_groups.remove(&key);
        } else {
            self.collapsed_groups.insert(key);
        }
    }

    /// Provider ordering for the Sources pane.
    pub fn provider_order() -> &'static [&'static str] {
        &["docker", "azure", "command", "file"]
    }

    /// Build the visible rows in the sources pane: headers + items.
    /// Returns vec of (is_header, kind, Option<source_index>).
    pub fn visible_source_rows(&self) -> Vec<(bool, String, Option<usize>)> {
        let mut rows = Vec::new();
        for &kind in Self::provider_order() {
            let sources_in_kind: Vec<usize> = self
                .sources
                .iter()
                .enumerate()
                .filter(|(_, s)| s.kind == kind)
                .map(|(i, _)| i)
                .collect();
            if sources_in_kind.is_empty() {
                continue;
            }
            // Group header
            rows.push((true, kind.to_string(), None));
            if !self.collapsed_groups.contains(kind) {
                for idx in sources_in_kind {
                    rows.push((false, kind.to_string(), Some(idx)));
                }
            }
        }
        rows
    }

    pub fn update_filtered_view(&mut self) {
        let sorted = self.store.sorted_indices();
        let mut results = if !self.search_query.is_empty() {
            search::fuzzy_search(&self.search_query, self.store.patterns(), &sorted)
        } else {
            sorted
                .iter()
                .map(|&i| SearchResult {
                    index: i,
                    score: 0,
                    matched_indices: vec![],
                })
                .collect()
        };

        // Apply source filter if active
        if let Some(ref source_id) = self.active_source_filter {
            let patterns = self.store.patterns();
            results.retain(|sr| patterns[sr.index].sources.contains(source_id));
        }

        self.filtered_view = results;
        if !self.filtered_view.is_empty() {
            if self.selected_pattern >= self.filtered_view.len() {
                self.selected_pattern = self.filtered_view.len() - 1;
            }
        } else {
            self.selected_pattern = 0;
        }
    }

    pub fn next_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Sources => Pane::Patterns,
            Pane::Patterns => Pane::Details,
            Pane::Details => Pane::Sources,
        };
    }

    pub fn prev_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Sources => Pane::Details,
            Pane::Patterns => Pane::Sources,
            Pane::Details => Pane::Patterns,
        };
    }

    pub fn move_up(&mut self) {
        match self.active_pane {
            Pane::Sources => {
                if self.selected_source > 0 {
                    self.selected_source -= 1;
                }
            }
            Pane::Patterns => {
                if self.selected_pattern > 0 {
                    self.selected_pattern -= 1;
                }
            }
            Pane::Details => {
                if self.detail_scroll > 0 {
                    self.detail_scroll -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.active_pane {
            Pane::Sources => {
                let rows = self.visible_source_rows();
                if !rows.is_empty() && self.selected_source < rows.len() - 1 {
                    self.selected_source += 1;
                }
            }
            Pane::Patterns => {
                let max = if self.filtered_view.is_empty() {
                    0
                } else {
                    self.filtered_view.len() - 1
                };
                if self.selected_pattern < max {
                    self.selected_pattern += 1;
                }
            }
            Pane::Details => {
                self.detail_scroll += 1;
            }
        }
    }

    /// Handle Enter on sources pane — toggle collapse on group headers,
    /// or set/clear source filter on individual sources.
    pub fn activate_selected_source(&mut self) {
        let rows = self.visible_source_rows();
        if let Some((is_header, kind, src_idx)) = rows.get(self.selected_source) {
            if *is_header {
                self.toggle_source_group(kind);
                self.needs_clear = true;
            } else if let Some(idx) = src_idx {
                let source_id = self.sources[*idx].id.clone();
                if self.active_source_filter.as_ref() == Some(&source_id) {
                    self.active_source_filter = None;
                } else {
                    self.active_source_filter = Some(source_id);
                }
                self.selected_pattern = 0;
                self.needs_clear = true;
            }
        }
    }

    pub fn selected_pattern_data(&self) -> Option<&crate::pattern::Pattern> {
        self.filtered_view
            .get(self.selected_pattern)
            .map(|sr| &self.store.patterns()[sr.index])
    }

    pub fn enter_search(&mut self) {
        self.mode = AppMode::Search;
        self.search_query.clear();
        self.active_pane = Pane::Patterns;
    }

    pub fn exit_search(&mut self, keep_filter: bool) {
        self.mode = AppMode::Normal;
        if !keep_filter {
            self.search_query.clear();
        }
        self.selected_pattern = 0;
        self.needs_clear = true;
    }

    pub fn next_profile(&mut self) {
        if self.profile_index < self.profiles.len() - 1 {
            self.profile_index += 1;
        }
    }

    pub fn prev_profile(&mut self) {
        if self.profile_index > 0 {
            self.profile_index -= 1;
        }
    }

    pub fn set_tx(&mut self, tx: mpsc::Sender<SourceEvent>) {
        self.tx = Some(tx);
    }

    pub fn add_docker_source(&mut self, container: String) {
        if let Some(tx) = self.tx.clone() {
            let (info, handle) = ingest::spawn_docker(container, tx);
            let id = info.id.clone();
            self.sources.push(info);
            self.handles.insert(id, handle);
        }
    }

    pub fn add_file_source(&mut self, path: String) {
        if let Some(tx) = self.tx.clone() {
            let (info, handle) = ingest::spawn_file(path, tx);
            let id = info.id.clone();
            self.sources.push(info);
            self.handles.insert(id, handle);
        }
    }

    pub fn add_command_source(&mut self, cmd: String) {
        if let Some(tx) = self.tx.clone() {
            let name = cmd
                .split_whitespace()
                .next()
                .unwrap_or("cmd")
                .to_string();
            let (info, handle) = ingest::spawn_command(name, cmd, tx);
            let id = info.id.clone();
            self.sources.push(info);
            self.handles.insert(id, handle);
        }
    }

    pub fn add_azure_source(
        &mut self,
        app_name: String,
        resource_group: String,
        subscription_id: String,
    ) {
        if let Some(tx) = self.tx.clone() {
            let (info, handle) = ingest::spawn_azure_containerapp(
                app_name,
                resource_group,
                subscription_id,
                self.azure_token.clone(),
                tx,
            );
            let id = info.id.clone();
            self.sources.push(info);
            self.handles.insert(id, handle);
        }
    }

    pub fn open_source_menu(&mut self) {
        self.source_menu.reset();
        self.source_menu.docker_loading = true;
        self.source_menu.azure_loading = true;
        self.mode = AppMode::SourceMenu;
        // Pre-fetch both Docker and Azure discovery in parallel
        if let Some(dtx) = self.discovery_tx.clone() {
            crate::discovery::discover_docker(dtx.clone());
            crate::discovery::discover_azure(dtx);
        }
    }

    pub fn handle_discovery_result(&mut self, result: DiscoveryResult) {
        match result {
            DiscoveryResult::Docker(Ok(containers)) => {
                self.source_menu.docker_containers = containers;
                self.source_menu.docker_loading = false;
                self.source_menu.docker_error = None;
            }
            DiscoveryResult::Docker(Err(e)) => {
                self.source_menu.docker_loading = false;
                self.source_menu.docker_error = Some(e);
            }
            DiscoveryResult::Azure(Ok(apps)) => {
                self.source_menu.azure_apps = apps;
                self.source_menu.azure_loading = false;
                self.source_menu.azure_error = None;
            }
            DiscoveryResult::Azure(Err(e)) => {
                self.source_menu.azure_loading = false;
                self.source_menu.azure_error = Some(e);
            }
            DiscoveryResult::AzureToken(Ok(token)) => {
                self.azure_token = Some(token);
            }
            DiscoveryResult::AzureToken(Err(_)) => {
                // Token pre-fetch failed; will fall back to az CLI for log streaming
            }
        }
    }

    pub fn spawn_selected_docker_sources(&mut self) {
        let selected: Vec<usize> = self.source_menu.selected.iter().copied().collect();
        for idx in selected {
            if let Some(c) = self.source_menu.docker_containers.get(idx) {
                self.add_docker_source(c.name.clone());
            }
        }
    }

    pub fn spawn_selected_azure_sources(&mut self) {
        let selected: Vec<usize> = self.source_menu.selected.iter().copied().collect();
        for idx in selected {
            if let Some(a) = self.source_menu.azure_apps.get(idx) {
                self.add_azure_source(
                    a.name.clone(),
                    a.resource_group.clone(),
                    a.subscription_id.clone(),
                );
            }
        }
    }
}
