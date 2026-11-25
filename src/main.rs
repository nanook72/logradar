mod alert;
mod app;
mod config;
mod discovery;
mod export;
mod headless;
mod ingest;
mod parse;
mod pattern;
mod profile;
mod search;
mod theme;
mod tui;
mod util;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use std::time::Duration;

use app::{AppMode, Pane};
use tui::source_menu::SourceMenuScreen;

#[derive(Parser)]
#[command(name = "logradar", version, about = "Modern log analysis TUI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Where logs come from. Shared by every subcommand that reads logs.
#[derive(Args, Debug, Default)]
struct SourceArgs {
    /// Docker container to tail
    #[arg(long)]
    docker: Vec<String>,

    /// Shell command to stream
    #[arg(long)]
    cmd: Vec<String>,

    /// File path to tail ('-' reads standard input)
    #[arg(long)]
    file: Vec<String>,

    /// Kubernetes logs: pod, kind/name (deploy/api) or label selector
    /// (app=api), optionally prefixed with "namespace:"
    #[arg(long, value_name = "TARGET")]
    kube: Vec<String>,
}

impl SourceArgs {
    fn is_empty(&self) -> bool {
        self.docker.is_empty() && self.cmd.is_empty() && self.file.is_empty() && self.kube.is_empty()
    }
}

/// How logs are parsed and grouped. Shared by every subcommand that reads logs.
#[derive(Args, Debug, Default)]
struct AnalysisArgs {
    /// Path to config file (default: ./logradar.toml or ~/.config/logradar/config.toml)
    #[arg(long)]
    config: Option<String>,

    /// Group only identical lines (turn off Drain template clustering)
    #[arg(long)]
    exact: bool,
}

#[derive(Args, Debug)]
struct TuiArgs {
    #[command(flatten)]
    sources: SourceArgs,

    #[command(flatten)]
    analysis: AnalysisArgs,

    /// Profile name (default, ops, network, or custom)
    #[arg(long)]
    profile: Option<String>,

    /// Theme name (matrix, nebula, frostbyte, ember, deepwave, signal, obsidian, notredame, mono)
    #[arg(long)]
    theme: Option<String>,

    /// Disable ASCII banner header
    #[arg(long)]
    no_banner: bool,
}

#[derive(Args, Debug)]
struct SummaryArgs {
    #[command(flatten)]
    sources: SourceArgs,

    #[command(flatten)]
    analysis: AnalysisArgs,

    /// Lowest level to include (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    level: String,

    /// How many patterns to print
    #[arg(long, default_value_t = 20)]
    top: usize,

    /// Print JSON instead of a table
    #[arg(long)]
    json: bool,

    /// Exit with status 1 if any pattern is at this level or above (for CI)
    #[arg(long, value_name = "LEVEL")]
    fail_on: Option<String>,
}

#[derive(Args, Debug)]
struct WatchArgs {
    #[command(flatten)]
    sources: SourceArgs,

    #[command(flatten)]
    analysis: AnalysisArgs,

    /// Lowest level to include (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    level: String,

    /// Print JSON lines instead of text
    #[arg(long)]
    json: bool,

    /// Don't print new patterns; only alerts from the config
    #[arg(long)]
    no_new: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the TUI dashboard
    Tui(TuiArgs),
    /// Read logs once and print the most common patterns
    Summary(SummaryArgs),
    /// Follow logs without the TUI, printing new patterns and alerts
    Watch(WatchArgs),
}

fn parse_kube_targets(specs: &[String]) -> Result<Vec<ingest::KubeTarget>> {
    specs.iter().map(|s| ingest::KubeTarget::parse(s)).collect()
}

fn parse_level_arg(flag: &str, value: &str) -> Result<parse::Level> {
    parse::level_from_str(value)
        .ok_or_else(|| anyhow::anyhow!("{} '{}': use trace, debug, info, warn or error", flag, value))
}

/// Config, clustering and sources shared by the headless subcommands.
fn headless_options(
    sources: SourceArgs,
    analysis: &AnalysisArgs,
    level: &str,
) -> Result<(config::Config, headless::Options)> {
    let cfg = config::Config::load(analysis.config.as_deref())?;
    let clustering = if analysis.exact {
        pattern::Clustering::Exact
    } else {
        cfg.clustering.to_clustering()?
    };
    let opts = headless::Options {
        docker: sources.docker,
        cmd: sources.cmd,
        file: sources.file,
        kube: parse_kube_targets(&sources.kube)?,
        clustering,
        min_level: parse_level_arg("--level", level)?,
    };
    Ok((cfg, opts))
}

#[tokio::main]
async fn main() {
    // Errors are for people: one line, no backtrace.
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("logradar: {:#}", e);
        std::process::exit(2);
    }
    // Exit now rather than when the runtime shuts down: a stdin reader is a
    // blocking thread that would otherwise keep the process alive until the
    // pipe closes. Sources have already been stopped.
    std::process::exit(0);
}

async fn run(cli: Cli) -> Result<()> {

    match cli.command {
        Commands::Tui(args) => {
            run_tui(args).await?;
        }
        Commands::Summary(args) => {
            let (_, opts) = headless_options(args.sources, &args.analysis, &args.level)?;
            let fail_on = match &args.fail_on {
                Some(l) => Some(parse_level_arg("--fail-on", l)?),
                None => None,
            };
            let code = headless::summary(
                opts,
                headless::SummaryOptions {
                    top: args.top,
                    json: args.json,
                    fail_on,
                },
            )
            .await?;
            std::process::exit(code);
        }
        Commands::Watch(args) => {
            let (cfg, opts) = headless_options(args.sources, &args.analysis, &args.level)?;
            let alerts = alert::AlertEngine::from_config(&cfg.alerts, &cfg.alert_settings)?;
            headless::watch(
                opts,
                headless::WatchOptions {
                    json: args.json,
                    show_new: !args.no_new,
                },
                alerts,
            )
            .await?;
        }
    }

    Ok(())
}

async fn run_tui(args: TuiArgs) -> Result<()> {
    let TuiArgs {
        sources,
        analysis,
        profile,
        theme: theme_name,
        no_banner,
    } = args;
    let cfg = config::Config::load(analysis.config.as_deref())?;
    let clustering = if analysis.exact {
        pattern::Clustering::Exact
    } else {
        cfg.clustering.to_clustering()?
    };
    let mut alerts = alert::AlertEngine::from_config(&cfg.alerts, &cfg.alert_settings)?;
    let default_profile = cfg.default_profile.clone();
    let profiles = cfg.into_profiles();

    let profile_name = profile.or(default_profile);
    let mut app = app::App::with_profiles(profiles, profile_name.as_deref());
    app.show_banner = !no_banner;
    app.store = pattern::PatternStore::with_options(clustering, pattern::MAX_PATTERNS);

    // Apply --theme override
    if let Some(ref name) = theme_name {
        if let Some(t) = theme::Theme::by_name(name) {
            app.theme_override = Some(t);
        } else {
            eprintln!(
                "Unknown theme '{}'. Available: {}",
                name,
                theme::Theme::all_names().join(", ")
            );
            std::process::exit(1);
        }
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    app.set_tx(tx.clone());

    // Discovery channel
    let (discovery_tx, mut discovery_rx) =
        tokio::sync::mpsc::channel::<discovery::DiscoveryResult>(16);
    app.discovery_tx = Some(discovery_tx);

    // Spawn ingest sources from CLI
    let has_cli_sources = !sources.is_empty();
    for container in sources.docker {
        app.add_docker_source(container);
    }
    for cmd_str in sources.cmd {
        app.add_command_source(cmd_str);
    }
    for path in sources.file {
        app.add_file_source(path);
    }
    for target in parse_kube_targets(&sources.kube)? {
        app.add_kube_source(target);
    }

    // Keep tx alive for dynamic source additions (drop our local clone)
    drop(tx);

    // Auto-open source menu if no CLI sources given
    if !has_cli_sources {
        app.open_source_menu();
    }

    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let tick_rate = Duration::from_millis(50);

    let mut last_alert_check = std::time::Instant::now();
    loop {
        if app.needs_clear {
            terminal.clear()?;
            app.needs_clear = false;
        }
        app.update_filtered_view();
        terminal.draw(|f| tui::ui::render(f, &mut app))?;

        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(&mut app, key);
            }
        }

        // Drain log events (non-blocking)
        loop {
            match rx.try_recv() {
                Ok(ev) => app.process_event(ev),
                Err(_) => break,
            }
        }

        // Drain discovery results
        loop {
            match discovery_rx.try_recv() {
                Ok(result) => app.handle_discovery_result(result),
                Err(_) => break,
            }
        }

        app.store.tick();
        if !alerts.is_empty() && last_alert_check.elapsed() >= Duration::from_secs(1) {
            last_alert_check = std::time::Instant::now();
            for firing in alerts.evaluate(app.store.patterns(), last_alert_check) {
                app.alerts_fired += 1;
                app.last_alert = Some((last_alert_check, firing.summary()));
                alert::deliver(firing);
            }
        }
        app.tick_source_rates();
        app.tick_count += 1;

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    terminal::disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Stop sources and give the aborted tasks a moment to kill their children.
    app.stop_all_sources();
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok(())
}

fn handle_key_event(app: &mut app::App, key: event::KeyEvent) {
    // Search mode: capture all input
    if app.mode == AppMode::Search {
        match key.code {
            KeyCode::Esc => app.exit_search(false),
            KeyCode::Enter => app.exit_search(true),
            KeyCode::Backspace => {
                app.search_query.pop();
            }
            KeyCode::Char(c) => app.search_query.push(c),
            _ => {}
        }
        return;
    }

    // Source menu
    if app.mode == AppMode::SourceMenu {
        handle_source_menu_key(app, key);
        return;
    }

    // Profile picker
    if app.mode == AppMode::ProfilePicker {
        match key.code {
            KeyCode::Esc => app.mode = AppMode::Normal,
            KeyCode::Up | KeyCode::Char('k') => app.prev_profile(),
            KeyCode::Down | KeyCode::Char('j') => app.next_profile(),
            KeyCode::Enter => app.mode = AppMode::Normal,
            _ => {}
        }
        return;
    }

    // Help overlay
    if app.mode == AppMode::Help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                app.mode = AppMode::Normal;
                app.needs_clear = true;
            }
            _ => {}
        }
        return;
    }

    // Drilldown
    if app.mode == AppMode::Drilldown {
        match key.code {
            KeyCode::Esc | KeyCode::Char('b') => {
                app.mode = AppMode::Normal;
                app.detail_scroll = 0;
                app.needs_clear = true;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp | KeyCode::Char('g') => {
                let len = app.drill_lines().len();
                let cur = app.drill_index(len);
                app.detail_scroll = match key.code {
                    KeyCode::PageUp => cur.saturating_sub(10),
                    KeyCode::Char('g') => 0,
                    _ => cur.saturating_sub(1),
                };
                app.drill_follow = false;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                let len = app.drill_lines().len();
                let cur = app.drill_index(len);
                let step = if key.code == KeyCode::PageDown { 10 } else { 1 };
                app.detail_scroll = (cur + step).min(len.saturating_sub(1));
                // Moving onto the newest line resumes following.
                app.drill_follow = app.detail_scroll + 1 >= len;
            }
            KeyCode::Char('G') | KeyCode::Char('f') => app.drill_follow = true,
            KeyCode::Char('y') => {
                let lines = app.drill_lines();
                let line = lines.get(app.drill_index(lines.len())).map(|l| l.raw.clone());
                match line {
                    Some(text) => match export::copy_to_clipboard(&text) {
                        Ok(()) => app.say("Copied line to clipboard (via terminal)"),
                        Err(e) => app.say(format!("Copy failed: {}", e)),
                    },
                    None => app.say("Nothing to copy"),
                }
            }
            KeyCode::Char('n') => app.show_normalized = !app.show_normalized,
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        }
        return;
    }

    // Normal mode
    match key.code {
        KeyCode::Esc => {
            // Clear active search filter or source filter
            if !app.search_query.is_empty() {
                app.search_query.clear();
                app.selected_pattern = 0;
                app.needs_clear = true;
            } else if app.active_source_filter.is_some() {
                app.active_source_filter = None;
                app.selected_pattern = 0;
                app.needs_clear = true;
            }
        }
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('?') => app.mode = AppMode::Help,
        KeyCode::Char('/') => app.enter_search(),
        KeyCode::Char('a') => app.open_source_menu(),
        KeyCode::Char('p') => app.paused = !app.paused,
        KeyCode::Char('P') => app.mode = AppMode::ProfilePicker,
        KeyCode::Char('r') => {
            app.store.reset();
            app.needs_clear = true;
        }
        KeyCode::Char('c') => {
            app.store.clear_counters();
            app.needs_clear = true;
        }
        KeyCode::Char('n') => app.show_normalized = !app.show_normalized,
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('L') => app.cycle_level_filter(),
        KeyCode::Char('x') => app.export("md"),
        KeyCode::Char('X') => app.export("json"),
        KeyCode::Char('t') => app.toggle_theme(),
        KeyCode::Tab => app.next_pane(),
        KeyCode::BackTab => app.prev_pane(),
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Enter => {
            if app.active_pane == Pane::Patterns && app.selected_pattern_data().is_some() {
                app.mode = AppMode::Drilldown;
                app.detail_scroll = 0;
                app.drill_follow = true;
                app.needs_clear = true;
            } else if app.active_pane == Pane::Sources {
                app.activate_selected_source();
            }
        }
        _ => {}
    }
}

fn handle_source_menu_key(app: &mut app::App, key: event::KeyEvent) {
    let screen = app.source_menu.screen;

    match screen {
        SourceMenuScreen::MainMenu => match key.code {
            KeyCode::Esc => {
                app.mode = AppMode::Normal;
                app.needs_clear = true;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.source_menu.main_cursor > 0 {
                    app.source_menu.main_cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if app.source_menu.main_cursor < tui::source_menu::MAIN_MENU_ITEMS.len() - 1 {
                    app.source_menu.main_cursor += 1;
                }
            }
            KeyCode::Enter => {
                match app.source_menu.main_cursor {
                    0 => {
                        // Docker Container — results already pre-fetched
                        app.source_menu.screen = SourceMenuScreen::DockerDiscovery;
                        app.source_menu.discovery_cursor = 0;
                        app.source_menu.selected.clear();
                    }
                    1 => {
                        // Kubernetes Pod — results already pre-fetched
                        app.source_menu.screen = SourceMenuScreen::KubeDiscovery;
                        app.source_menu.discovery_cursor = 0;
                        app.source_menu.selected.clear();
                    }
                    2 => {
                        // File (tail)
                        app.source_menu.screen = SourceMenuScreen::FileInput;
                        app.source_menu.text_input.clear();
                    }
                    3 => {
                        // Azure Container App — results already pre-fetched
                        app.source_menu.screen = SourceMenuScreen::AzureDiscovery;
                        app.source_menu.discovery_cursor = 0;
                        app.source_menu.selected.clear();
                    }
                    4 => {
                        // Custom Command
                        app.source_menu.screen = SourceMenuScreen::CommandInput;
                        app.source_menu.text_input.clear();
                    }
                    _ => {}
                }
            }
            _ => {}
        },
        SourceMenuScreen::DockerDiscovery => match key.code {
            KeyCode::Esc => {
                app.source_menu.screen = SourceMenuScreen::MainMenu;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.source_menu.discovery_cursor > 0 {
                    app.source_menu.discovery_cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = app.source_menu.docker_containers.len();
                if count > 0 && app.source_menu.discovery_cursor < count - 1 {
                    app.source_menu.discovery_cursor += 1;
                }
            }
            KeyCode::Char(' ') => {
                app.source_menu.toggle_selection();
            }
            KeyCode::Char('r') => {
                app.source_menu.docker_loading = true;
                app.source_menu.docker_error = None;
                app.source_menu.docker_containers.clear();
                app.source_menu.selected.clear();
                app.source_menu.discovery_cursor = 0;
                if let Some(dtx) = app.discovery_tx.clone() {
                    discovery::discover_docker(dtx);
                }
            }
            KeyCode::Enter => {
                app.spawn_selected_docker_sources();
                app.mode = AppMode::Normal;
                app.needs_clear = true;
            }
            _ => {}
        },
        SourceMenuScreen::KubeDiscovery => match key.code {
            KeyCode::Esc => {
                app.source_menu.screen = SourceMenuScreen::MainMenu;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.source_menu.discovery_cursor > 0 {
                    app.source_menu.discovery_cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = app.source_menu.kube_pods.len();
                if count > 0 && app.source_menu.discovery_cursor < count - 1 {
                    app.source_menu.discovery_cursor += 1;
                }
            }
            KeyCode::Char(' ') => {
                app.source_menu.toggle_selection();
            }
            KeyCode::Char('r') => {
                app.source_menu.kube_loading = true;
                app.source_menu.kube_error = None;
                app.source_menu.kube_pods.clear();
                app.source_menu.selected.clear();
                app.source_menu.discovery_cursor = 0;
                if let Some(dtx) = app.discovery_tx.clone() {
                    discovery::discover_kube(dtx);
                }
            }
            KeyCode::Enter => {
                app.spawn_selected_kube_sources();
                app.mode = AppMode::Normal;
                app.needs_clear = true;
            }
            _ => {}
        },
        SourceMenuScreen::AzureDiscovery => match key.code {
            KeyCode::Esc => {
                app.source_menu.screen = SourceMenuScreen::MainMenu;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if app.source_menu.discovery_cursor > 0 {
                    app.source_menu.discovery_cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = app.source_menu.azure_apps.len();
                if count > 0 && app.source_menu.discovery_cursor < count - 1 {
                    app.source_menu.discovery_cursor += 1;
                }
            }
            KeyCode::Char(' ') => {
                app.source_menu.toggle_selection();
            }
            KeyCode::Char('r') => {
                app.source_menu.azure_loading = true;
                app.source_menu.azure_error = None;
                app.source_menu.azure_apps.clear();
                app.source_menu.selected.clear();
                app.source_menu.discovery_cursor = 0;
                if let Some(dtx) = app.discovery_tx.clone() {
                    discovery::discover_azure(dtx);
                }
            }
            KeyCode::Enter => {
                app.spawn_selected_azure_sources();
                app.mode = AppMode::Normal;
                app.needs_clear = true;
            }
            _ => {}
        },
        SourceMenuScreen::FileInput => match key.code {
            KeyCode::Esc => {
                app.source_menu.screen = SourceMenuScreen::MainMenu;
            }
            KeyCode::Backspace => {
                app.source_menu.text_input.pop();
            }
            KeyCode::Enter => {
                let path = app.source_menu.text_input.trim().to_string();
                if !path.is_empty() {
                    app.add_file_source(path);
                }
                app.mode = AppMode::Normal;
                app.needs_clear = true;
            }
            KeyCode::Char(c) => {
                app.source_menu.text_input.push(c);
            }
            _ => {}
        },
        SourceMenuScreen::CommandInput => match key.code {
            KeyCode::Esc => {
                app.source_menu.screen = SourceMenuScreen::MainMenu;
            }
            KeyCode::Backspace => {
                app.source_menu.text_input.pop();
            }
            KeyCode::Enter => {
                let cmd = app.source_menu.text_input.trim().to_string();
                if !cmd.is_empty() {
                    app.add_command_source(cmd);
                }
                app.mode = AppMode::Normal;
                app.needs_clear = true;
            }
            KeyCode::Char(c) => {
                app.source_menu.text_input.push(c);
            }
            _ => {}
        },
    }
}
