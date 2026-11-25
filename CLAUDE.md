# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build, Test & Run

```bash
cargo build                       # dev build
cargo build --release             # optimized build
cargo test                        # run all unit tests
cargo test parse::tests           # run tests for a single module
cargo run -- tui --help           # show CLI help
cargo run -- tui --cmd "command"  # launch TUI with a command source
cargo run -- tui --docker name    # launch TUI tailing a Docker container
cargo run -- tui --file path      # launch TUI tailing a file
cargo run -- tui --config path    # use a custom config file
cargo run -- tui --theme nebula   # launch with a specific theme
cargo run -- summary --file path  # headless: top patterns (--json, --fail-on error)
cargo run -- watch --file -       # headless: follow stdin, print new patterns + alerts
cargo test --test cli             # end-to-end tests of the binary (tests/cli.rs)
```

## Architecture

**Data flow:** Ingest sources (tokio tasks) → `SourceEvent::Log`/`Status` via bounded mpsc → main event loop → PatternStore → TUI render

The main event loop in `main.rs::run_tui` is synchronous on the main thread: it polls crossterm events with 50ms timeout, drains the mpsc channel via `try_recv`, ticks the pattern store, and renders. Tokio ingest tasks run on worker threads.

### Module Responsibilities

- **`main.rs`** — Clap CLI (`tui`, `summary`, `watch`; shared `SourceArgs`/`AnalysisArgs`), terminal setup/teardown, event loop, key handling. Errors print as one line and exit 2; after a clean run the process exits explicitly (a blocking stdin reader would otherwise hold the runtime open)
- **`headless.rs`** — `summary` (sources read once via `ingest::Follow::No`, top patterns as table/JSON, exit 1 on `--fail-on`, 2 on source errors) and `watch` (follow, print new patterns immediately, evaluate alerts each second)
- **`app.rs`** — Central `App` struct holding all state. Modes: Normal, Search, Drilldown, Help, ProfilePicker, SourceMenu. Panes: Sources, Patterns, Details. Source management with per-source handles (HashMap keyed by id).
- **`config.rs`** — TOML config loading from `--config`, `./logradar.toml`, or `~/.config/logradar/config.toml`. Merges custom profiles with built-ins via `into_profiles()`.
- **`parse/`** — `LogEvent` struct, structured JSON/logfmt parsing (clusters on the message field), level detection (explicit `[LEVEL]`/`level=` first, then case-insensitive whole-word search), normalization (regex replacement chain: timestamps→UUIDs→IPs→hex→durations→numbers). Applies ANSI stripping before all processing.
- **`pattern/`** — `PatternStore` groups events into patterns, by Drain template (`pattern/drain.rs`, default) or by exact normalized string (`Clustering::Exact`, `--exact`). `Pattern` tracks rolling window timestamps (VecDeque<Instant>), computes 1m/5m rates, trend direction, spike detection, sparkline history (24 buckets × 5s).
- **`export.rs`** — Markdown report (code spans sized past any backtick run, `|` escaped) / JSON export of the current view into a new `logradar-<time>.<ext>` (never overwrites); OSC 52 clipboard copy with base64 payload
- **`search/`** — Fuzzy search over pattern canonical strings using `fuzzy-matcher` SkimMatcherV2. Returns scored results with matched character indices for highlighting
- **`ingest/`** — Spawns tokio tasks for docker, azure, command, file. Uses `SourceEvent` enum (`Log`/`Status` variants). Each source sends status updates (Starting→Running→Stopped/Error). All commands use `kill_on_drop(true)`.
- **`alert.rs`** — `[[alerts]]` rules (first_seen / spike / rate, with level/pattern/source globs) evaluated once a second over the pattern store; delivers via argv-only commands and curl webhooks. Per-pattern cooldown, global `max_per_minute`, `startup_grace` so file history isn't "new". `Pattern.id` is the stable key.
- **`discovery.rs`** — Async Docker (`docker ps`) and Azure (`az containerapp list`) auto-discovery.
- **`tui/ui.rs`** — All ratatui rendering. Uses `Theme` for every color. 3-pane layout with header bar, sparkline column, severity badges, status icons with spinner animation.
- **`tui/source_menu.rs`** — Source menu state: screens for Docker/Azure discovery (multi-select), file/command text input.
- **`theme.rs`** — `Theme` struct with 40+ named color roles. Nine themes: matrix (default), nebula, frostbyte, ember, deepwave, signal, obsidian, notredame, mono. Cycle with `t`, select with `--theme <name>`. `Theme::by_name()` for lookup, `Theme::next()` for cycling. Banner colors: `banner_primary`, `banner_accent`, `banner_tagline`, `banner_separator`.
- **`profile.rs`** — Profile definitions (default/ops/network) with min_level filter and highlight keywords
- **`util/`** — ANSI escape code stripping via regex

### Key Design Decisions

- Drain clustering: leaf = (level, token count, first token unless it's a value); the line joins the most similar template if ≥ `similarity` (default 0.6) of tokens match exactly (wildcards don't count), and differing positions become `<*>`. Levels never merge. Exact mode keys a `HashMap<String, usize>` on the normalized string. Both are capped at `MAX_PATTERNS`, evicting the least recently seen (and removing its Drain cluster)
- CLI args shared between subcommands live in `SourceArgs` / `AnalysisArgs` (clap `flatten`)
- The 5m rate divides by the time actually observed (min 1m), so new sessions don't show false spikes
- `PatternStore` keeps the last `RECENT_LINES` (10k) lines with time/source/pattern id for drill-down; `context(seq, n)` finds same-source neighbours by seq arithmetic
- App-level view: `SortMode` (`s`), `level_filter` (`L`), `flash` status messages; patterns first seen within `NEW_FOR` (60s) show NEW in the trend column
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
- Log content is attacker-controlled: everything drawn from a source goes through `util::sanitize` (all ANSI sequences, control chars and invisible/bidi format chars removed). JSON messages are sanitized again after decoding, since `\u001b` only becomes ESC then
- `ingest::forward_lines` is the one line reader for every source: lines capped at `MAX_LINE_BYTES` (64 KiB), lossy UTF-8 so a bad byte can't end a stream; stored samples/signatures are capped again at `parse::MAX_STORED_BYTES` (4 KiB)
- Alerts never use a shell: `command` is argv and each `{placeholder}` fills one argument; webhook URL + body go to curl via `-K -` on stdin (`ingest::curl_quote`)
- Bearer tokens never go on a command line: curl gets them as a config file on stdin (`-K -`); URL parts from Azure are validated before use
