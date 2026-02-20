use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use chrono::Local;

use crate::app::{App, AppMode, Pane};
use crate::ingest::SourceStatus;
use crate::theme::Theme;
use crate::tui::source_menu::{SourceMenuScreen, MAIN_MENU_ITEMS};

const SPINNER_CHARS: &[char] = &['◐', '◓', '◑', '◒'];

/// Compute the header height based on terminal size and banner setting.
/// Returns: banner lines (0 or 3) + separator (1) + stats line (1)
fn header_height(app: &App, terminal_height: u16) -> u16 {
    if !app.show_banner || terminal_height < 22 {
        1 // Minimal single-line header
    } else {
        7 // 5 lines wordmark + 1 separator + 1 stats line
    }
}

pub fn render(f: &mut Frame, app: &mut App) {
    let theme = app.theme().clone();
    let h = header_height(app, f.size().height);

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(h),  // header (dynamic)
            Constraint::Min(3),     // body
            Constraint::Length(1),  // status bar
        ])
        .split(f.size());

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(18),
            Constraint::Percentage(52),
            Constraint::Percentage(30),
        ])
        .split(main_chunks[1]);

    render_header(f, main_chunks[0], app, &theme);

    if app.mode == AppMode::Help {
        render_sources(f, body_chunks[0], app, &theme);
        render_patterns(f, body_chunks[1], app, &theme);
        render_details(f, body_chunks[2], app, &theme);
        render_status_bar(f, main_chunks[2], app, &theme);
        let help_area = centered_rect(60, 80, f.size());
        render_help(f, help_area, &theme);
        return;
    }

    render_sources(f, body_chunks[0], app, &theme);

    match app.mode {
        AppMode::ProfilePicker => {
            render_profile_picker(f, body_chunks[1], app, &theme);
            render_details(f, body_chunks[2], app, &theme);
        }
        AppMode::Drilldown => {
            render_drilldown(f, body_chunks[1], app, &theme);
            render_drilldown_detail(f, body_chunks[2], app, &theme);
        }
        _ => {
            render_patterns(f, body_chunks[1], app, &theme);
            render_details(f, body_chunks[2], app, &theme);
        }
    }

    render_status_bar(f, main_chunks[2], app, &theme);

    if app.mode == AppMode::SourceMenu {
        let menu_area = centered_rect(60, 70, f.size());
        render_source_menu(f, menu_area, app, &theme);
    }
}

fn pane_block<'a>(title: &str, focused: bool, theme: &Theme) -> Block<'a> {
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border
    };
    let title_color = if focused {
        theme.accent
    } else {
        theme.text_dim
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ))
}

fn render_sources(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.active_pane == Pane::Sources && app.mode == AppMode::Normal;
    let block = pane_block("Sources", focused, theme);

    let rows = app.visible_source_rows();

    if rows.is_empty() {
        let msg = if app.sources.is_empty() {
            "No sources (a=add)"
        } else {
            "No sources"
        };
        let empty = Paragraph::new(Span::styled(
            msg,
            Style::default().fg(theme.text_dim),
        ))
        .block(block);
        f.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(row_idx, (is_header, kind, src_idx))| {
            if *is_header {
                let collapsed = app.collapsed_groups.contains(kind.as_str());
                let chevron = if collapsed { "▸" } else { "▾" };
                let count = app.sources.iter().filter(|s| s.kind == *kind).count();
                let rate = app.provider_rate_1m(kind);
                let label = provider_label(kind);
                let rate_str = if rate > 0.0 {
                    format!(" {:.0}/m", rate)
                } else {
                    String::new()
                };
                let is_selected = row_idx == app.selected_source && focused;
                let header_style = if is_selected {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", chevron), header_style),
                    Span::styled(format!("{} ({})", label, count), header_style),
                    Span::styled(
                        rate_str,
                        Style::default().fg(theme.success),
                    ),
                ]))
            } else {
                let src = &app.sources[src_idx.unwrap()];
                let is_filtered = app.active_source_filter.as_ref() == Some(&src.id);

                let (marker, marker_color) = if is_filtered {
                    ("▶".to_string(), theme.header_accent)
                } else {
                    match &src.status {
                        SourceStatus::Running => ("●".to_string(), theme.success),
                        SourceStatus::Starting => {
                            let ch = SPINNER_CHARS[(app.tick_count / 4) as usize % SPINNER_CHARS.len()];
                            (ch.to_string(), theme.warn)
                        }
                        SourceStatus::Error(_) => ("✖".to_string(), theme.error),
                        SourceStatus::Stopped => ("○".to_string(), theme.text_dim),
                    }
                };

                let is_selected = row_idx == app.selected_source && focused;
                let style = if is_selected {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                } else if is_filtered {
                    Style::default()
                        .fg(theme.header_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                };
                let rate = app.source_rate_1m(&src.id);
                let rate_str = if rate > 0.0 {
                    format!(" {:.0}/m", rate)
                } else {
                    match &src.status {
                        SourceStatus::Starting => " starting...".to_string(),
                        SourceStatus::Error(e) => {
                            let short = if e.len() > 20 { &e[..20] } else { e };
                            format!(" {}", short)
                        }
                        _ => String::new(),
                    }
                };
                let rate_color = match &src.status {
                    SourceStatus::Error(_) => theme.error,
                    SourceStatus::Starting => theme.warn,
                    _ => theme.text_dim,
                };
                // Show just the name part after the kind prefix
                let display_name = src.id.split('/').nth(1).unwrap_or(&src.id);
                ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{} ", marker), Style::default().fg(marker_color)),
                    Span::styled(display_name.to_string(), style),
                    Span::styled(
                        rate_str,
                        Style::default().fg(rate_color),
                    ),
                ]))
            }
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn provider_label(kind: &str) -> &str {
    match kind {
        "docker" => "Docker",
        "azure" => "Azure",
        "command" => "Command",
        "file" => "File",
        _ => kind,
    }
}

fn render_patterns(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.active_pane == Pane::Patterns
        && matches!(app.mode, AppMode::Normal | AppMode::Search);
    let source_tag = app
        .active_source_filter
        .as_ref()
        .map(|s| {
            let short = s.split('/').nth(1).unwrap_or(s);
            format!(" [{}]", short)
        })
        .unwrap_or_default();
    let title = if app.mode == AppMode::Search {
        format!("Patterns{} [/{}]", source_tag, app.search_query)
    } else if !app.search_query.is_empty() {
        format!(
            "Patterns{} ({}) [/{}] Esc=clear",
            source_tag,
            app.filtered_view.len(),
            app.search_query
        )
    } else {
        format!("Patterns{} ({})", source_tag, app.filtered_view.len())
    };
    let block = pane_block(&title, focused, theme);
    let inner = block.inner(area);

    if app.filtered_view.is_empty() {
        let msg = if app.store.len() == 0 {
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No active patterns",
                    Style::default().fg(theme.text_dim),
                )),
                Line::from(Span::styled(
                    "Waiting for log events...",
                    Style::default().fg(theme.text_dim),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Press 'a' to add a source",
                    Style::default().fg(theme.accent),
                )),
            ]
        } else {
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No matching patterns",
                    Style::default().fg(theme.text_dim),
                )),
            ]
        };
        let p = Paragraph::new(msg)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(p, area);
        return;
    }

    // Column layout: Lvl(5) Cnt(5) 1m(6) Tr(3) Signature(rest) Spark(24)
    // Overhead: highlight_symbol(2) + fixed_cols(5+5+6+3+24=43) + column_spacing(5 gaps=5) = 50
    let sig_width = (inner.width as usize).saturating_sub(50);
    let patterns = app.store.patterns();

    let header = Row::new(vec![
        Cell::from("Lvl"),
        Cell::from("Cnt"),
        Cell::from("1m"),
        Cell::from("Tr"),
        Cell::from("Signature"),
        Cell::from("Activity"),
    ])
    .style(
        Style::default()
            .fg(theme.text_dim)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .filtered_view
        .iter()
        .enumerate()
        .map(|(row_idx, sr)| {
            let p = &patterns[sr.index];
            let is_selected = row_idx == app.selected_pattern;

            // Severity badge: [ERR] [WRN] [INF] [DBG] [???]
            let badge_text = format!("[{}]", p.level.short());
            let badge_cell = Cell::from(Line::from(Span::styled(
                badge_text,
                Style::default()
                    .fg(theme.level_color(p.level))
                    .bg(theme.badge_bg(p.level))
                    .add_modifier(Modifier::BOLD),
            )));

            // Count with activity-based coloring
            let count_str = compact_count(p.count_total);
            let count_cell = Cell::from(Line::from(Span::styled(
                count_str,
                Style::default().fg(theme.count_color(p.rate_1m())),
            )));

            // 1m rate
            let rate = p.rate_1m();
            let rate_str = if rate >= 100.0 {
                format!("{:.0}", rate)
            } else if rate >= 10.0 {
                format!("{:.1}", rate)
            } else if rate > 0.0 {
                format!("{:.1}", rate)
            } else {
                "·".to_string()
            };
            let rate_cell = Cell::from(Line::from(Span::styled(
                rate_str,
                Style::default().fg(theme.count_color(rate)),
            )));

            // Trend arrow with color coding
            let (trend_str, trend_color) = if p.spike {
                ("↑↑".to_string(), theme.count_hot)
            } else {
                (p.trend.symbol().to_string(), theme.trend_color(p.trend))
            };
            let trend_cell = Cell::from(Line::from(Span::styled(
                trend_str,
                Style::default().fg(trend_color).add_modifier(
                    if p.spike { Modifier::BOLD } else { Modifier::empty() },
                ),
            )));

            // Signature with ellipsis truncation + keyword highlighting
            let sig = ellipsis_truncate(&p.canonical, sig_width);
            let sig_cell = if !sr.matched_indices.is_empty() {
                Cell::from(Line::from(highlight_matches(&sig, &sr.matched_indices, theme)))
            } else if is_selected {
                Cell::from(Line::from(highlight_sig_keywords(&sig, theme, true)))
            } else {
                Cell::from(Line::from(highlight_sig_keywords(&sig, theme, false)))
            };

            // Sparkline: accent color if spiking, muted otherwise (far-right column)
            let spark_spans = render_spark(
                &p.sparkline_buckets,
                p.current_bucket_count,
                if p.spike { theme.accent } else { theme.sparkline },
                theme,
            );
            let spark_cell = Cell::from(Line::from(spark_spans));

            Row::new(vec![
                badge_cell,
                count_cell,
                rate_cell,
                trend_cell,
                sig_cell,
                spark_cell,
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(5),  // [ERR]
        Constraint::Length(5),  // Count
        Constraint::Length(6),  // 1m rate
        Constraint::Length(3),  // Trend
        Constraint::Min(10),   // Signature
        Constraint::Length(24), // Sparkline (24 buckets)
    ];

    let highlight_style = Style::default()
        .fg(theme.selected_fg)
        .bg(theme.selected_bg)
        .add_modifier(Modifier::BOLD);

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("▸ ");

    let mut state = TableState::default();
    state.select(Some(app.selected_pattern));
    f.render_stateful_widget(table, area, &mut state);
}

fn render_details(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let focused = app.active_pane == Pane::Details && app.mode == AppMode::Normal;
    let block = pane_block("Details", focused, theme);
    let inner_width = block.inner(area).width as usize;

    if let Some(pattern) = app.selected_pattern_data() {
        let divider_str: String = "─".repeat(inner_width.saturating_sub(2));
        let divider_style = Style::default().fg(theme.divider);
        let label_style = Style::default().fg(theme.text_dim);
        let value_style = Style::default().fg(theme.text);

        let mut lines = Vec::new();

        // --- Metadata section ---
        // Level + Trend on one line
        lines.push(Line::from(vec![
            Span::styled(
                format!("[{}]", pattern.level.short()),
                Style::default()
                    .fg(theme.level_color(pattern.level))
                    .bg(theme.badge_bg(pattern.level))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                pattern.trend.symbol(),
                Style::default().fg(theme.trend_color(pattern.trend)),
            ),
            if pattern.spike {
                Span::styled(
                    " SPIKE",
                    Style::default()
                        .fg(theme.error)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("")
            },
            Span::raw("  "),
            Span::styled("count ", label_style),
            Span::styled(
                compact_count(pattern.count_total),
                Style::default().fg(theme.count_color(pattern.rate_1m())),
            ),
        ]));

        // Rate line
        lines.push(Line::from(vec![
            Span::styled("1m ", label_style),
            Span::styled(format!("{:.1}/m", pattern.rate_1m()), value_style),
            Span::raw("  "),
            Span::styled("5m ", label_style),
            Span::styled(format!("{:.1}/m", pattern.rate_5m()), value_style),
        ]));

        // Sources
        if !pattern.sources.is_empty() {
            let src_list: Vec<&String> = pattern.sources.iter().collect();
            let src_str = src_list
                .iter()
                .map(|s| s.split('/').nth(1).unwrap_or(s))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(vec![
                Span::styled("src ", label_style),
                Span::styled(src_str, Style::default().fg(theme.text_dim)),
            ]));
        }

        // --- Divider ---
        lines.push(Line::from(Span::styled(divider_str.clone(), divider_style)));

        // --- Signature section ---
        lines.push(Line::from(Span::styled(
            "SIGNATURE",
            Style::default()
                .fg(theme.text_dim)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            pattern.canonical.clone(),
            Style::default().fg(theme.accent),
        )));

        // --- Divider ---
        lines.push(Line::from(Span::styled(divider_str, divider_style)));

        // --- Latest Sample section ---
        lines.push(Line::from(Span::styled(
            if app.show_normalized { "NORMALIZED" } else { "LATEST SAMPLE" },
            Style::default()
                .fg(theme.text_dim)
                .add_modifier(Modifier::BOLD),
        )));

        if let Some(sample) = pattern.samples.back() {
            let display = if app.show_normalized {
                &pattern.canonical
            } else {
                sample
            };
            for line_str in display.lines() {
                lines.push(Line::from(highlight_sample_terms(line_str, theme)));
            }
        }

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll as u16, 0));
        f.render_widget(paragraph, area);
    } else {
        let msg = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Select a pattern",
                Style::default().fg(theme.text_dim),
            )),
            Line::from(Span::styled(
                "to view details",
                Style::default().fg(theme.text_dim),
            )),
        ];
        let paragraph = Paragraph::new(msg)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(paragraph, area);
    }
}

fn render_drilldown(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = pane_block("Drilldown - Samples", true, theme);

    if let Some(pattern) = app.selected_pattern_data() {
        let items: Vec<ListItem> = pattern
            .samples
            .iter()
            .enumerate()
            .map(|(i, sample)| {
                let display = if app.show_normalized {
                    pattern.canonical.clone()
                } else {
                    sample.clone()
                };
                let style = if i == app.detail_scroll {
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selected_bg)
                } else {
                    Style::default().fg(theme.text)
                };
                ListItem::new(Line::from(Span::styled(display, style)))
            })
            .collect();

        let list = List::new(items).block(block);
        f.render_widget(list, area);
    } else {
        let p = Paragraph::new("No pattern selected")
            .block(block)
            .style(Style::default().fg(theme.text_dim));
        f.render_widget(p, area);
    }
}

fn render_drilldown_detail(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = pane_block("Sample Detail", false, theme);

    if let Some(pattern) = app.selected_pattern_data() {
        if let Some(sample) = pattern.samples.get(app.detail_scroll) {
            let display = if app.show_normalized {
                pattern.canonical.clone()
            } else {
                sample.clone()
            };
            let paragraph = Paragraph::new(display)
                .block(block)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text));
            f.render_widget(paragraph, area);
            return;
        }
    }

    let p = Paragraph::new("No sample")
        .block(block)
        .style(Style::default().fg(theme.text_dim));
    f.render_widget(p, area);
}

fn render_status_bar(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let style = Style::default()
        .fg(theme.status_bar_fg)
        .bg(theme.status_bar_bg);

    if app.mode == AppMode::Search {
        let line = Line::from(vec![
            Span::styled(" Search: ", style.add_modifier(Modifier::BOLD)),
            Span::styled(
                app.search_query.clone(),
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.status_bar_bg),
            ),
            Span::styled(
                "█",
                Style::default()
                    .fg(theme.accent)
                    .bg(theme.status_bar_bg),
            ),
        ]);
        let bar = Paragraph::new(line).style(style);
        f.render_widget(bar, area);
        return;
    }

    let paused = if app.paused { " PAUSED " } else { "" };
    let profile_name = app.profile().name.clone();

    // Calculate total ingest rate
    let total_rate: f64 = app.source_rates.values().map(|ts| ts.len() as f64).sum();
    let rate_str = format!("{:.0} evt/m", total_rate);
    let rate_color = if total_rate > 10.0 {
        theme.success
    } else {
        theme.text_dim
    };

    let line = Line::from(vec![
        Span::styled(
            " logradar ",
            Style::default()
                .fg(theme.accent)
                .bg(theme.status_bar_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "│ {} patterns │ {} events │ ",
                app.store.len(),
                app.log_count,
            ),
            style,
        ),
        Span::styled(
            rate_str,
            Style::default()
                .fg(rate_color)
                .bg(theme.status_bar_bg),
        ),
        Span::styled(" │ ", style),
        Span::styled(
            theme.name.clone(),
            Style::default()
                .fg(theme.accent)
                .bg(theme.status_bar_bg),
        ),
        Span::styled(
            format!(" │ {} ", profile_name),
            style,
        ),
        Span::styled(
            paused.to_string(),
            Style::default()
                .fg(theme.warn)
                .bg(theme.status_bar_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("│ ", style),
        Span::styled("?", Style::default().fg(theme.accent).bg(theme.status_bar_bg).add_modifier(Modifier::BOLD)),
        Span::styled("=help ", Style::default().fg(theme.status_bar_fg).bg(theme.status_bar_bg)),
        Span::styled("a", Style::default().fg(theme.accent).bg(theme.status_bar_bg).add_modifier(Modifier::BOLD)),
        Span::styled("=add ", Style::default().fg(theme.status_bar_fg).bg(theme.status_bar_bg)),
        Span::styled("/", Style::default().fg(theme.accent).bg(theme.status_bar_bg).add_modifier(Modifier::BOLD)),
        Span::styled("=search ", Style::default().fg(theme.status_bar_fg).bg(theme.status_bar_bg)),
        Span::styled("q", Style::default().fg(theme.accent).bg(theme.status_bar_bg).add_modifier(Modifier::BOLD)),
        Span::styled("=quit ", Style::default().fg(theme.status_bar_fg).bg(theme.status_bar_bg)),
    ]);

    let bar = Paragraph::new(line).style(style);
    f.render_widget(bar, area);
}

fn render_source_menu(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    f.render_widget(Clear, area);

    let menu = &app.source_menu;
    match menu.screen {
        SourceMenuScreen::MainMenu => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.modal_border))
                .style(Style::default().bg(theme.modal_bg))
                .title(Span::styled(
                    " Add Source ",
                    Style::default()
                        .fg(theme.modal_title)
                        .add_modifier(Modifier::BOLD),
                ));

            let items: Vec<ListItem> = MAIN_MENU_ITEMS
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    let marker = if i == menu.main_cursor { "▸ " } else { "  " };
                    let style = if i == menu.main_cursor {
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                    } else {
                        Style::default().fg(theme.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, Style::default().fg(theme.accent)),
                        Span::styled(label.to_string(), style),
                    ]))
                })
                .collect();

            let list = List::new(items).block(block);
            f.render_widget(list, area);
        }
        SourceMenuScreen::DockerDiscovery => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.modal_border))
                .style(Style::default().bg(theme.modal_bg))
                .title(Span::styled(
                    " Docker Containers (Space=select, Enter=add, r=refresh) ",
                    Style::default()
                        .fg(theme.modal_title)
                        .add_modifier(Modifier::BOLD),
                ));

            if menu.docker_loading {
                let p = Paragraph::new(Span::styled(
                    "Discovering containers...",
                    Style::default().fg(theme.text_dim),
                ))
                .block(block);
                f.render_widget(p, area);
                return;
            }

            if let Some(ref err) = menu.docker_error {
                let p = Paragraph::new(Span::styled(
                    err.clone(),
                    Style::default().fg(theme.error),
                ))
                .block(block);
                f.render_widget(p, area);
                return;
            }

            if menu.docker_containers.is_empty() {
                let p = Paragraph::new(Span::styled(
                    "No containers found",
                    Style::default().fg(theme.text_dim),
                ))
                .block(block);
                f.render_widget(p, area);
                return;
            }

            let items: Vec<ListItem> = menu
                .docker_containers
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let checkbox = if menu.selected.contains(&i) {
                        "[x] "
                    } else {
                        "[ ] "
                    };
                    let cursor = if i == menu.discovery_cursor {
                        "▸ "
                    } else {
                        "  "
                    };
                    let style = if i == menu.discovery_cursor {
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                    } else {
                        Style::default().fg(theme.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(cursor, Style::default().fg(theme.accent)),
                        Span::styled(
                            checkbox.to_string(),
                            Style::default().fg(theme.success),
                        ),
                        Span::styled(c.name.clone(), style),
                        Span::styled(
                            format!("  {}", c.image),
                            Style::default().fg(theme.text_dim),
                        ),
                    ]))
                })
                .collect();

            let list = List::new(items).block(block);
            f.render_widget(list, area);
        }
        SourceMenuScreen::AzureDiscovery => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.modal_border))
                .style(Style::default().bg(theme.modal_bg))
                .title(Span::styled(
                    " Azure Container Apps (Space=select, Enter=add, r=refresh) ",
                    Style::default()
                        .fg(theme.modal_title)
                        .add_modifier(Modifier::BOLD),
                ));

            if menu.azure_loading {
                let p = Paragraph::new(Span::styled(
                    "Discovering Azure Container Apps...",
                    Style::default().fg(theme.text_dim),
                ))
                .block(block);
                f.render_widget(p, area);
                return;
            }

            if let Some(ref err) = menu.azure_error {
                let p = Paragraph::new(Span::styled(
                    err.clone(),
                    Style::default().fg(theme.error),
                ))
                .block(block);
                f.render_widget(p, area);
                return;
            }

            if menu.azure_apps.is_empty() {
                let p = Paragraph::new(Span::styled(
                    "No Azure Container Apps found",
                    Style::default().fg(theme.text_dim),
                ))
                .block(block);
                f.render_widget(p, area);
                return;
            }

            let items: Vec<ListItem> = menu
                .azure_apps
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let checkbox = if menu.selected.contains(&i) {
                        "[x] "
                    } else {
                        "[ ] "
                    };
                    let cursor = if i == menu.discovery_cursor {
                        "▸ "
                    } else {
                        "  "
                    };
                    let style = if i == menu.discovery_cursor {
                        Style::default()
                            .fg(theme.selected_fg)
                            .bg(theme.selected_bg)
                    } else {
                        Style::default().fg(theme.text)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(cursor, Style::default().fg(theme.accent)),
                        Span::styled(
                            checkbox.to_string(),
                            Style::default().fg(theme.success),
                        ),
                        Span::styled(a.name.clone(), style),
                        Span::styled(
                            format!("  ({})", a.resource_group),
                            Style::default().fg(theme.text_dim),
                        ),
                    ]))
                })
                .collect();

            let list = List::new(items).block(block);
            f.render_widget(list, area);
        }
        SourceMenuScreen::FileInput => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.modal_border))
                .style(Style::default().bg(theme.modal_bg))
                .title(Span::styled(
                    " File Path (Enter to add, Esc to cancel) ",
                    Style::default()
                        .fg(theme.modal_title)
                        .add_modifier(Modifier::BOLD),
                ));

            let text = Line::from(vec![
                Span::styled("Path: ", Style::default().fg(theme.text_dim)),
                Span::styled(
                    menu.text_input.clone(),
                    Style::default().fg(theme.text),
                ),
                Span::styled("█", Style::default().fg(theme.accent)),
            ]);

            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        }
        SourceMenuScreen::CommandInput => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.modal_border))
                .style(Style::default().bg(theme.modal_bg))
                .title(Span::styled(
                    " Command (Enter to add, Esc to cancel) ",
                    Style::default()
                        .fg(theme.modal_title)
                        .add_modifier(Modifier::BOLD),
                ));

            let text = Line::from(vec![
                Span::styled("$ ", Style::default().fg(theme.accent)),
                Span::styled(
                    menu.text_input.clone(),
                    Style::default().fg(theme.text),
                ),
                Span::styled("█", Style::default().fg(theme.accent)),
            ]);

            let p = Paragraph::new(text).block(block);
            f.render_widget(p, area);
        }
    }
}

// ASCII wordmark — 5 rows, figlet shadow style (42 chars wide)
const WORDMARK: [&str; 5] = [
    r" |                           |            ",
    r" |  _ \   _` |  __| _` |  _` |  _` |  __| ",
    r" | (   | (   | |   (   | (   | (   | |    ",
    r"_|\___/ \__, |_|  \__,_|\__,_|\__,_|_|    ",
    r"        |___/                             ",
];
// Character index where "radar" starts (column 15)
const WORDMARK_SPLIT: usize = 15;

fn render_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 1 {
        // Minimal single-line fallback
        render_header_minimal(f, area, app, theme);
        return;
    }

    // Full banner: 5 lines wordmark + 1 separator + 1 stats
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // wordmark
            Constraint::Length(1), // separator
            Constraint::Length(1), // stats line
        ])
        .split(area);

    // --- Wordmark rows ---
    let tagline = "real-time log intelligence";
    let w = area.width as usize;

    let mut wm_lines: Vec<Line> = Vec::with_capacity(3);
    for (i, row) in WORDMARK.iter().enumerate() {
        let row_chars: Vec<char> = row.chars().collect();
        // Truncate wordmark to fit available width (minus 1 for leading space)
        let max_wm = w.saturating_sub(1);
        let visible_len = row_chars.len().min(max_wm);
        let split = WORDMARK_SPLIT.min(visible_len);
        let log_part: String = row_chars[..split].iter().collect();
        let radar_part: String = row_chars[split..visible_len].iter().collect();
        let wm_width = 1 + visible_len; // leading space + visible chars

        let mut spans = vec![
            Span::styled(" ", Style::default().bg(theme.header_bg)),
            Span::styled(
                log_part,
                Style::default()
                    .fg(theme.banner_primary)
                    .bg(theme.header_bg),
            ),
            Span::styled(
                radar_part,
                Style::default()
                    .fg(theme.banner_accent)
                    .bg(theme.header_bg)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        // Show tagline next to the last wordmark row if space allows
        if i == 4 && w > wm_width + tagline.len() + 4 {
            let gap = 3;
            spans.push(Span::styled(
                " ".repeat(gap),
                Style::default().bg(theme.header_bg),
            ));
            spans.push(Span::styled(
                tagline.to_string(),
                Style::default()
                    .fg(theme.banner_tagline)
                    .bg(theme.header_bg),
            ));
            let used = wm_width + gap + tagline.len();
            let pad = w.saturating_sub(used);
            spans.push(Span::styled(
                " ".repeat(pad),
                Style::default().bg(theme.header_bg),
            ));
        } else {
            let pad = w.saturating_sub(wm_width);
            spans.push(Span::styled(
                " ".repeat(pad),
                Style::default().bg(theme.header_bg),
            ));
        }

        wm_lines.push(Line::from(spans));
    }

    let wm_para = Paragraph::new(wm_lines).style(Style::default().bg(theme.header_bg));
    f.render_widget(wm_para, chunks[0]);

    // --- Separator ---
    let sep: String = "─".repeat(w);
    let sep_line = Paragraph::new(Line::from(Span::styled(
        sep,
        Style::default().fg(theme.banner_separator).bg(theme.header_bg),
    )))
    .style(Style::default().bg(theme.header_bg));
    f.render_widget(sep_line, chunks[1]);

    // --- Stats line ---
    render_header_stats(f, chunks[2], app, theme);
}

fn render_header_minimal(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let clock = Local::now().format("%H:%M:%S").to_string();
    let total_rate: f64 = app.source_rates.values().map(|ts| ts.len() as f64).sum();

    let left = format!(
        " logradar  {} src  {} pat  {} evt  {:.0} evt/m",
        app.sources.len(),
        app.store.len(),
        app.log_count,
        total_rate,
    );
    let right = format!("{}  ", clock);
    let pad = (area.width as usize).saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(
            " log",
            Style::default()
                .fg(theme.banner_primary)
                .bg(theme.header_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "radar",
            Style::default()
                .fg(theme.banner_accent)
                .bg(theme.header_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {} src  {} pat  {} evt  {:.0} evt/m",
                app.sources.len(),
                app.store.len(),
                app.log_count,
                total_rate,
            ),
            Style::default().fg(theme.header_fg).bg(theme.header_bg),
        ),
        Span::styled(
            " ".repeat(pad),
            Style::default().bg(theme.header_bg),
        ),
        Span::styled(
            right,
            Style::default().fg(theme.header_accent).bg(theme.header_bg),
        ),
    ]);

    let bar = Paragraph::new(line).style(Style::default().bg(theme.header_bg));
    f.render_widget(bar, area);
}

fn render_header_stats(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let clock = Local::now().format("%H:%M:%S").to_string();
    let total_rate: f64 = app.source_rates.values().map(|ts| ts.len() as f64).sum();

    let left = format!(
        " logradar  {}  {} sources  {}  {} patterns  {}  {} events  {}  {:.0} evt/m",
        "▸", app.sources.len(), "▸", app.store.len(), "▸", app.log_count, "▸", total_rate,
    );
    let right = format!("{}  ", clock);
    let pad = (area.width as usize).saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(" ", Style::default().bg(theme.header_bg)),
        Span::styled(
            "log",
            Style::default()
                .fg(theme.banner_primary)
                .bg(theme.header_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "radar",
            Style::default()
                .fg(theme.banner_accent)
                .bg(theme.header_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " ▸ {} sources ▸ {} patterns ▸ {} events ▸ {:.0} evt/m",
                app.sources.len(),
                app.store.len(),
                app.log_count,
                total_rate,
            ),
            Style::default().fg(theme.header_fg).bg(theme.header_bg),
        ),
        Span::styled(
            " ".repeat(pad),
            Style::default().bg(theme.header_bg),
        ),
        Span::styled(
            right,
            Style::default().fg(theme.header_accent).bg(theme.header_bg),
        ),
    ]);

    let bar = Paragraph::new(line).style(Style::default().bg(theme.header_bg));
    f.render_widget(bar, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_help(f: &mut Frame, area: Rect, theme: &Theme) {
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.modal_border))
        .style(Style::default().bg(theme.modal_bg))
        .title(Span::styled(
            " Help ",
            Style::default()
                .fg(theme.modal_title)
                .add_modifier(Modifier::BOLD),
        ));
    let help_text = vec![
        Line::from(Span::styled(
            "Keybindings",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        help_line("Tab/Shift+Tab", "Switch panes", theme),
        help_line("j/k or Up/Down", "Navigate", theme),
        help_line("Enter", "Drilldown / lock search filter", theme),
        help_line("b", "Back from drilldown", theme),
        help_line("/", "Search patterns", theme),
        help_line("Esc", "Clear filter / exit overlay", theme),
        help_line("a", "Add source (interactive)", theme),
        help_line("n", "Toggle normalized / raw", theme),
        help_line("t", "Toggle color / mono theme", theme),
        help_line("p", "Pause / resume ingest", theme),
        help_line("r", "Reset all patterns", theme),
        help_line("c", "Clear counters", theme),
        help_line("P", "Profile picker", theme),
        help_line("q", "Quit", theme),
        help_line("?", "Toggle help", theme),
    ];

    let paragraph = Paragraph::new(help_text).block(block);
    f.render_widget(paragraph, area);
}

fn help_line<'a>(key: &'a str, desc: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {:<18}", key),
            Style::default().fg(theme.accent),
        ),
        Span::styled(desc, Style::default().fg(theme.text)),
    ])
}

fn render_profile_picker(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = pane_block("Profile Picker (Enter to select, Esc to cancel)", true, theme);

    let items: Vec<ListItem> = app
        .profiles
        .iter()
        .enumerate()
        .map(|(i, profile)| {
            let marker = if i == app.profile_index {
                "▸ "
            } else {
                "  "
            };
            let style = if i == app.profile_index {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
            } else {
                Style::default().fg(theme.text)
            };
            let detail = format!(
                "  (min: {}, highlights: {})",
                profile.min_level,
                profile.highlights.len()
            );
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.accent)),
                Span::styled(
                    profile.name.clone(),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(detail, Style::default().fg(theme.text_dim)),
            ]))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn highlight_matches<'a>(text: &str, indices: &[usize], theme: &Theme) -> Vec<Span<'a>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut normal_buf = String::new();

    let match_style = Style::default()
        .fg(theme.fuzzy_match)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default().fg(theme.text);

    for (i, &ch) in chars.iter().enumerate() {
        if indices.contains(&i) {
            if !normal_buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut normal_buf),
                    normal_style,
                ));
            }
            spans.push(Span::styled(ch.to_string(), match_style));
        } else {
            normal_buf.push(ch);
        }
    }
    if !normal_buf.is_empty() {
        spans.push(Span::styled(normal_buf, normal_style));
    }

    spans
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len || max_len == 0 {
        s
    } else {
        let end = s
            .char_indices()
            .take(max_len)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        &s[..end]
    }
}

fn ellipsis_truncate(s: &str, max_len: usize) -> String {
    if max_len < 4 {
        return truncate_str(s, max_len).to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        let mut result: String = chars[..max_len - 1].iter().collect();
        result.push('\u{2026}'); // …
        result
    }
}

fn compact_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

/// Render a sparkline from completed buckets + the in-progress bucket.
/// Uses soft-cap normalization: cap = max(mean_nonzero * 3, 1) to prevent
/// a single spike from flattening everything.
/// Returns 24 Span characters (oldest→newest), with the newest char bolded.
fn render_spark<'a>(
    buckets: &std::collections::VecDeque<u16>,
    current: u16,
    color: ratatui::style::Color,
    theme: &Theme,
) -> Vec<Span<'a>> {
    const CHARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    const WIDTH: usize = 24;

    // Build full array: completed buckets + current in-progress bucket
    let mut values: Vec<u16> = Vec::with_capacity(WIDTH);
    // Left-pad with zeros if needed
    let total = buckets.len() + 1; // +1 for current
    let pad = WIDTH.saturating_sub(total);
    for _ in 0..pad {
        values.push(0);
    }
    // Add completed buckets (skip oldest if more than WIDTH-1)
    let skip = if buckets.len() + 1 > WIDTH {
        buckets.len() + 1 - WIDTH
    } else {
        0
    };
    for &v in buckets.iter().skip(skip) {
        values.push(v);
    }
    // Add current bucket as newest
    values.push(current);
    // Ensure exactly WIDTH
    while values.len() < WIDTH {
        values.push(0);
    }

    // Soft-cap normalization: use mean of non-zero values × 3
    let nonzero: Vec<f64> = values.iter().filter(|&&v| v > 0).map(|&v| v as f64).collect();
    let cap = if nonzero.is_empty() {
        1.0
    } else {
        let mean = nonzero.iter().sum::<f64>() / nonzero.len() as f64;
        (mean * 3.0).max(1.0)
    };

    let base_style = Style::default().fg(color);
    let dim_style = Style::default().fg(theme.text_dim);
    let muted_style = Style::default().fg(theme.sparkline_dim);
    let bold_style = base_style.add_modifier(Modifier::BOLD);

    // Two-tone: last 4 buckets are bright, older ones are muted
    let bright_start = WIDTH.saturating_sub(4);

    let mut spans = Vec::with_capacity(WIDTH);
    for (i, &v) in values.iter().enumerate() {
        let is_newest = i == WIDTH - 1;
        if v == 0 {
            spans.push(Span::styled(" ", dim_style));
        } else {
            let fraction = (v as f64 / cap).clamp(0.0, 1.0);
            let idx = (fraction * 7.0).round() as usize;
            let ch = CHARS[idx.min(7)];
            let style = if is_newest {
                bold_style
            } else if i >= bright_start {
                base_style
            } else {
                muted_style
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
    }
    spans
}

/// Highlight ERROR/FATAL/WARN/PANIC keywords in signature text.
/// Non-selected rows use dimmed base color; selected rows use bright text.
fn highlight_sig_keywords<'a>(text: &str, theme: &Theme, selected: bool) -> Vec<Span<'a>> {
    let base_style = if selected {
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_dim)
    };
    let error_style = Style::default()
        .fg(theme.error)
        .add_modifier(Modifier::BOLD);
    let warn_style = Style::default()
        .fg(theme.warn)
        .add_modifier(Modifier::BOLD);

    let mut spans = Vec::new();
    let mut buf = String::new();

    for word in text.split_inclusive(|c: char| c.is_whitespace() || c == '=' || c == ':' || c == ',' || c == ';') {
        let trimmed = word.trim();
        let upper = trimmed.to_ascii_uppercase();
        if upper == "ERROR" || upper == "ERR" || upper == "FATAL" || upper == "PANIC" {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base_style));
            }
            spans.push(Span::styled(word.to_string(), error_style));
        } else if upper == "WARN" || upper == "WARNING" || upper == "WRN" {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base_style));
            }
            spans.push(Span::styled(word.to_string(), warn_style));
        } else {
            buf.push_str(word);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, base_style));
    }
    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), base_style));
    }
    spans
}

fn highlight_sample_terms<'a>(text: &str, theme: &Theme) -> Vec<Span<'a>> {
    // Highlight numbers, IPs, durations, and severity keywords in samples
    let mut spans = Vec::new();
    let mut buf = String::new();
    let text_style = Style::default().fg(theme.text);
    let number_style = Style::default().fg(theme.accent);
    let error_style = Style::default()
        .fg(theme.error)
        .add_modifier(Modifier::BOLD);
    let warn_style = Style::default()
        .fg(theme.warn)
        .add_modifier(Modifier::BOLD);

    // Simple word-based highlighting
    for word in text.split_inclusive(|c: char| c.is_whitespace() || c == '=' || c == ':' || c == ',' || c == ';') {
        let trimmed = word.trim();
        let upper = trimmed.to_ascii_uppercase();
        if upper == "ERROR" || upper == "ERR" || upper == "FATAL" || upper == "PANIC" {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), text_style));
            }
            spans.push(Span::styled(word.to_string(), error_style));
        } else if upper == "WARN" || upper == "WARNING" || upper == "WRN" {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), text_style));
            }
            spans.push(Span::styled(word.to_string(), warn_style));
        } else if trimmed.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == ':')
            && trimmed.chars().any(|c| c.is_ascii_digit())
            && trimmed.len() > 0
        {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), text_style));
            }
            spans.push(Span::styled(word.to_string(), number_style));
        } else {
            buf.push_str(word);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, text_style));
    }
    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), text_style));
    }
    spans
}
