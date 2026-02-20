mod app;
mod config;
mod discovery;
mod ingest;
mod parse;
mod pattern;
mod profile;
mod search;
mod theme;
mod tui;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};
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

#[derive(Subcommand)]
enum Commands {
    /// Launch the TUI dashboard
    Tui {
        /// Profile name (default, ops, network, or custom)
        #[arg(long)]
        profile: Option<String>,

        /// Docker container to tail
        #[arg(long)]
        docker: Vec<String>,

        /// Shell command to stream
        #[arg(long)]
        cmd: Vec<String>,

        /// File path to tail
        #[arg(long)]
        file: Vec<String>,

        /// Path to config file (default: ./logradar.toml or ~/.config/logradar/config.toml)
        #[arg(long)]
        config: Option<String>,

        /// Theme name (matrix, nebula, frostbyte, ember, deepwave, signal, obsidian, mono)
        #[arg(long)]
        theme: Option<String>,

        /// Disable ASCII banner header
        #[arg(long)]
        no_banner: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tui {
            profile,
            docker,
            cmd,
            file,
            config: config_path,
            theme: theme_name,
            no_banner,
        } => {
            run_tui(profile, docker, cmd, file, config_path, theme_name, no_banner).await?;
        }
    }

    Ok(())
}

async fn run_tui(
    profile: Option<String>,
    dockers: Vec<String>,
    cmds: Vec<String>,
    files: Vec<String>,
    config_path: Option<String>,
    theme_name: Option<String>,
    no_banner: bool,
) -> Result<()> {
    let cfg = config::Config::load(config_path.as_deref())?;
    let default_profile = cfg.default_profile.clone();
    let profiles = cfg.into_profiles();

    let profile_name = profile.or(default_profile);
    let mut app = app::App::with_profiles(profiles, profile_name.as_deref());
    app.show_banner = !no_banner;

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
    let has_cli_sources =
        !dockers.is_empty() || !cmds.is_empty() || !files.is_empty();
    for container in dockers {
        app.add_docker_source(container);
    }
    for cmd_str in cmds {
        app.add_command_source(cmd_str);
    }
    for path in files {
        app.add_file_source(path);
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
            KeyCode::Up | KeyCode::Char('k') => {
                if app.detail_scroll > 0 {
                    app.detail_scroll -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(p) = app.selected_pattern_data() {
                    if app.detail_scroll + 1 < p.samples.len() {
                        app.detail_scroll += 1;
                    }
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
        KeyCode::Char('t') => app.toggle_theme(),
        KeyCode::Tab => app.next_pane(),
        KeyCode::BackTab => app.prev_pane(),
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Enter => {
            if app.active_pane == Pane::Patterns && app.selected_pattern_data().is_some() {
                app.mode = AppMode::Drilldown;
                app.detail_scroll = 0;
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
                        // File (tail)
                        app.source_menu.screen = SourceMenuScreen::FileInput;
                        app.source_menu.text_input.clear();
                    }
                    2 => {
                        // Azure Container App — results already pre-fetched
                        app.source_menu.screen = SourceMenuScreen::AzureDiscovery;
                        app.source_menu.discovery_cursor = 0;
                        app.source_menu.selected.clear();
                    }
                    3 => {
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
