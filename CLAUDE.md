# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build, Test & Run

```bash
cargo build                       # dev build
cargo build --release             # optimized build
cargo test                        # run all 49 unit tests
cargo test parse::tests           # run tests for a single module
cargo run -- tui --help           # show CLI help
cargo run -- tui --cmd "command"  # launch TUI with a command source
cargo run -- tui --docker name    # launch TUI tailing a Docker container
cargo run -- tui --file path      # launch TUI tailing a file
cargo run -- tui --config path    # use a custom config file
cargo run -- tui --theme nebula   # launch with a specific theme
```

## Architecture

**Data flow:** Ingest sources (tokio tasks) → `SourceEvent::Log`/`Status` via bounded mpsc → main event loop → PatternStore → TUI render

The main event loop in `main.rs::run_tui` is synchronous on the main thread: it polls crossterm events with 50ms timeout, drains the mpsc channel via `try_recv`, ticks the pattern store, and renders. Tokio ingest tasks run on worker threads.

### Module Responsibilities

- **`main.rs`** — Clap CLI definition, terminal setup/teardown, event loop, key handling
- **`app.rs`** — Central `App` struct holding all state. Modes: Normal, Search, Drilldown, Help, ProfilePicker, SourceMenu. Panes: Sources, Patterns, Details. Source management with per-source handles (HashMap keyed by id).
- **`config.rs`** — TOML config loading from `--config`, `./logradar.toml`, or `~/.config/logradar/config.toml`. Merges custom profiles with built-ins via `into_profiles()`.
- **`parse/`** — `LogEvent` struct, level detection (case-insensitive string search), normalization (regex replacement chain: timestamps→UUIDs→IPs→hex→durations→numbers). Applies ANSI stripping before all processing.
- **`pattern/`** — `PatternStore` groups events by normalized string hash. `Pattern` tracks rolling window timestamps (VecDeque<Instant>), computes 1m/5m rates, trend direction, spike detection, sparkline history (24 buckets × 5s).
- **`search/`** — Fuzzy search over pattern canonical strings using `fuzzy-matcher` SkimMatcherV2. Returns scored results with matched character indices for highlighting
- **`ingest/`** — Spawns tokio tasks for docker, azure, command, file. Uses `SourceEvent` enum (`Log`/`Status` variants). Each source sends status updates (Starting→Running→Stopped/Error). All commands use `kill_on_drop(true)`.
- **`discovery.rs`** — Async Docker (`docker ps`) and Azure (`az containerapp list`) auto-discovery.
- **`tui/ui.rs`** — All ratatui rendering. Uses `Theme` for every color. 3-pane layout with header bar, sparkline column, severity badges, status icons with spinner animation.
- **`tui/source_menu.rs`** — Source menu state: screens for Docker/Azure discovery (multi-select), file/command text input.
- **`theme.rs`** — `Theme` struct with 40+ named color roles. Eight themes: matrix (default), nebula, frostbyte, ember, deepwave, signal, obsidian, mono. Cycle with `t`, select with `--theme <name>`. `Theme::by_name()` for lookup, `Theme::next()` for cycling. Banner colors: `banner_primary`, `banner_accent`, `banner_tagline`, `banner_separator`.
- **`profile.rs`** — Profile definitions (default/ops/network) with min_level filter and highlight keywords
- **`util/`** — ANSI escape code stripping via regex

### Key Design Decisions

- Pattern clustering uses hash of normalized string (not edit distance) for O(1) lookup
- Rolling windows use `VecDeque<Instant>` pruned each tick — simple and allocation-free in steady state
- Sparkline uses separate `current_bucket_count` (in-progress) committed to `sparkline_buckets` ring on 5s tick
- No async in the TUI loop — crossterm polling + try_recv keeps it simple
- `Theme` is cloned per render frame to avoid borrow conflicts with `&mut App`
- `SearchResult.matched_indices` feeds directly into span-based highlighting in the TUI
- Docker source streams both stdout and stderr concurrently via separate tokio tasks
- `SourceEvent` is an enum — `Log` for data, `Status` for lifecycle updates (avoids shared mutable state)
- All child processes use `kill_on_drop(true)` — aborting a task handle kills the process cleanly
- Config merges custom profiles into built-ins; using a built-in name overrides it
- ANSI escape codes are stripped before normalization/clustering to avoid pattern pollution
