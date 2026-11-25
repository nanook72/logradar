# logradar

A modern, colorful, high-performance log analysis TUI built in Rust. Streams logs from multiple sources, clusters repeated patterns automatically, and includes an integrated fuzzy finder.

![Logradar](https://github.com/user-attachments/assets/e90ae3bf-e2b5-4e0a-8a0b-806fbf54fe62)

![Rust](https://img.shields.io/badge/Rust-stable-orange)
![License](https://img.shields.io/badge/License-MIT-blue)

## Features

- **Multi-source streaming** — Docker containers, Azure Container Apps, shell commands, file tailing
- **Interactive source menu** — Press `a` to discover and add sources at runtime
- **Template clustering (Drain)** — Lines that differ only in a few words share a template: `user alice logged in` + `user bob logged in` → `user <*> logged in`. Typically cuts the pattern count 5-10× on real logs. Use `--exact` to group only identical lines; keeps up to 5,000 patterns, dropping the least recently seen
- **Structured logs** — JSON (pino, bunyan, zap, Serilog, ECS…) and logfmt lines are clustered on their message field, with the level read from `level`/`severity`
- **Activity sparklines** — Per-pattern 2-minute history (24 buckets x 5s) using Unicode block characters
- **Rolling metrics** — 1-minute and 5-minute rate windows with color-coded trend indicators
- **Alerts** — Run a command or call a webhook (Slack, Discord, Teams…) when a pattern first appears, spikes, or crosses a rate
- **Spike detection** — Flags patterns whose 1-minute rate is over 3× their 5-minute average (sparkline turns accent on spike)
- **Live source status** — Per-source status icons: `●` running, `◐` starting (animated), `✖` error, `○` stopped
- **Fuzzy search** — Live pattern filtering with matched-character highlighting
- **ASCII banner** — Matrix-inspired wordmark header with responsive layout (disable with `--no-banner`)
- **Theme system** — nine themes (matrix, nebula, frostbyte, ember, deepwave, signal, obsidian, notredame, mono); cycle with `t` or pick with `--theme`
- **Switchable profiles** — default, ops, network (live switching with `P`)
- **Config file** — Custom profiles via `logradar.toml` or `~/.config/logradar/config.toml`
- **3-pane layout** — Sources (grouped by provider), Patterns (with sparklines), Details

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
# Binary at target/release/logradar
```

## Usage

```bash
# Tail a Docker container
logradar tui --docker my-container

# Stream from a shell command
logradar tui --cmd "kubectl logs -f deploy/api"

# Tail a log file
logradar tui --file /var/log/syslog

# Multiple sources at once
logradar tui --docker web --docker db --file /var/log/app.log

# Use a specific profile
logradar tui --profile ops --cmd "journalctl -f"

# Use a custom config file
logradar tui --config ./my-config.toml --cmd "my-app"

# Launch with a specific theme
logradar tui --theme ember --docker my-container
```

## Keybindings

| Key              | Action                       |
|------------------|------------------------------|
| `Tab`/`Shift+Tab`| Switch panes                |
| `j`/`k` or `↑`/`↓`| Navigate lists             |
| `Enter`          | Drilldown / filter by source |
| `b`              | Back from drilldown          |
| `/`              | Enter search mode            |
| `Esc`            | Exit search / help / picker  |
| `a`              | Add source (interactive menu)|
| `n`              | Toggle normalized / raw      |
| `t`              | Cycle theme                  |
| `p`              | Pause / resume ingest        |
| `P`              | Profile picker               |
| `r`              | Reset all patterns           |
| `c`              | Clear counters               |
| `?`              | Help overlay                 |
| `q`              | Quit                         |

## Profiles

| Profile   | Min Level | Highlights |
|-----------|-----------|------------|
| `default` | INFO      | —          |
| `ops`     | WARN      | panic, timeout, error, fail, refused, disconnect |
| `network` | WARN      | down, up, flap, reset, timeout, link, vpn, error |

### Custom Profiles

Create `logradar.toml` (or `~/.config/logradar/config.toml`):

```toml
default_profile = "myapp"

[profiles.myapp]
min_level = "DEBUG"
theme = "color"
highlights = ["panic", "timeout", "oom"]
```

Custom profiles are added alongside the built-ins. To override a built-in, use its name (e.g., `[profiles.default]`).

### Clustering

```toml
[clustering]
mode = "drain"      # or "exact": group only identical lines (same as --exact)
similarity = 0.6    # share of words that must match to join a template (0.0-1.0)
```

Raise `similarity` if unrelated lines are merged; lower it to merge more aggressively. Lines at different levels are never merged.

### Alerts

```toml
# A new kind of error appeared: desktop notification
[[alerts]]
name = "new error"
when = "first_seen"          # first_seen | spike | rate
level = "error"              # optional: lowest level that counts
command = ["notify-send", "logradar: {name}", "{sample}"]

# Timeouts above 20/min from any Docker source: post to Slack
[[alerts]]
name = "db timeouts"
when = "rate"
rate_per_min = 20
match = "*timeout*"          # optional: glob over the pattern
source = "docker/*"          # optional: glob over the source
cooldown = "10m"             # per pattern; default 5m
webhook = "https://hooks.slack.com/services/..."

[alert_settings]
max_per_minute = 10          # across all rules; extra alerts are dropped
startup_grace = "10s"        # patterns seen this early (e.g. file history) aren't "new"
```

Placeholders for `command`: `{name}` `{pattern}` `{level}` `{count}` `{rate}` `{source}` `{sample}` `{summary}`.

Log text is untrusted, so `command` is an argument list that is never run through a shell, and each placeholder fills exactly one argument. Don't wrap `{sample}` into a `sh -c` script string yourself; pass it as a separate argument (`["sh", "-c", 'echo "$1"', "sh", "{sample}"]`). Webhooks receive a JSON body (`text`, `alert`, `pattern`, `level`, `count`, `rate_per_min`, `source`, `sample`); the URL and body are handed to curl on stdin, so a secret webhook URL never shows up in `ps`.

## Azure Container Apps Setup

logradar can auto-discover and stream logs from Azure Container Apps. This requires the Azure CLI.

### Prerequisites

1. **Install the Azure CLI**

   ```bash
   # macOS
   brew install azure-cli

   # Linux
   curl -sL https://aka.ms/InstallAzureCLIDeb | sudo bash

   # Windows
   winget install Microsoft.AzureCLI
   ```

2. **Log in to Azure**

   ```bash
   az login
   ```

   For service principal / non-interactive auth:

   ```bash
   az login --service-principal -u <app-id> -p <secret> --tenant <tenant-id>
   ```

3. **Verify access** — you should see your Container Apps:

   ```bash
   az containerapp list -o table
   ```

### Usage

Once authenticated, logradar discovers Container Apps automatically:

- Press `a` in the TUI to open the source menu
- Select **Azure Container App** — available apps are listed from all subscriptions
- Select one or more apps and press `Enter` to start streaming

logradar pre-fetches a management token during discovery and uses the Azure REST API directly for fast log streaming. If that fails, it falls back to the `az` CLI.

### Scope

Azure Container Apps is the only Azure service currently supported. VMs, databases, and other Azure resources use different logging mechanisms (Azure Monitor, Log Analytics) that don't provide the same kind of real-time log stream.

## Architecture

```
src/
  main.rs        — CLI (clap) + terminal setup + event loop
  app.rs         — Central state, mode management, key dispatch
  config.rs      — TOML config file loading + profile merging
  theme.rs       — Theme struct with named color roles (8 themes)
  profile.rs     — Profile definitions (level filters + highlights)
  tui/ui.rs      — All ratatui rendering (3-pane layout, sparklines, modals)
  tui/source_menu.rs — Source menu state (Docker/Azure/File/Command discovery)
  ingest/        — Async source spawning (docker, azure, command, file) with status events
  discovery.rs   — Docker + Azure Container App auto-discovery
  parse/         — Level detection + log normalization (regex), ANSI stripping
  pattern/       — Clustering engine, rolling windows, spike detection, sparkline buckets
  search/        — Fuzzy matching via fuzzy-matcher/skim
  util/          — ANSI escape code stripping
```

**Data flow:** Sources → tokio tasks → `SourceEvent::Log`/`Status` → bounded mpsc → App event loop → PatternStore → TUI render

### Sparkline

Each pattern tracks a rolling history of event frequency. The sparkline column shows 24 buckets (5 seconds each, ~2 minutes total) rendered left-to-right as Unicode blocks `▁▂▃▄▅▆▇█`. Normalization uses a soft cap (3x mean of non-zero buckets) to prevent single spikes from flattening normal activity. The newest bucket (rightmost) is bolded to indicate "live". When a pattern is flagged as a spike, its sparkline color changes to the theme accent color.

### Source Status

Each source reports its lifecycle status via the event channel:
- **Starting** (`◐` animated spinner) — process spawn in progress
- **Running** (`●` green) — process spawned, streaming logs
- **Error** (`✖` red) — spawn failed or process exited with error
- **Stopped** (`○` dim) — stream ended normally

Azure Container App sources use `kill_on_drop(true)` so cancelling a source (aborting its tokio task) automatically kills the `az` child process.

### Banner

The ASCII wordmark header shows at the top when terminal height is 20+ rows. It uses a two-tone `log`/`radar` color split. A separator line and stats bar (`sources ▸ patterns ▸ events ▸ evt/m`) appear below.

- `--no-banner` — Skip the ASCII wordmark and use a minimal single-line header
- Small terminals (< 20 rows) automatically fall back to single-line mode

### Themes

Select via `--theme <name>` or cycle at runtime with `t`:

| Theme       | Mood |
|-------------|------|
| `matrix`    | Radar console — muted green primary, bright green accent (default) |
| `nebula`    | Deep space observability — soft purple, electric cyan |
| `frostbyte` | Clean icy enterprise — cool blue, bright ice accents |
| `ember`     | High-alert warm signal — burnt orange, gold |
| `deepwave`  | Ocean analytics — teal, aqua, deep navy |
| `signal`    | Minimalist professional — clean grays, subtle blue accent |
| `obsidian`  | Ultra minimal — grayscale with strategic green accents |
| `notredame` | Fighting Irish — navy, metallic gold, shamrock green |
| `mono`      | Pure grayscale — no color, maximum readability |

## License

MIT
