use chrono::{DateTime, Duration, Utc};
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::source::SourceStatus;
use super::diff_app::DiffTuiState;

// ---------------------------------------------------------------------------
// Palette — shares the same calm dark theme as the usage TUI so both commands
// feel like one app.
//
// Color semantics are used consistently across the whole page:
//   - prev window values  → MUTED  (grey = the past)
//   - curr window values  → TEXT   (white = the present)
//   - deltas (any column) → GREEN when positive, RED when negative, MUTED when zero
//   - ACCENT/ACCENT2      → titles, window labels, key hints only
// ---------------------------------------------------------------------------
const BG: Color = Color::Rgb(15, 17, 23);
const SURFACE: Color = Color::Rgb(20, 23, 33);
const HILITE: Color = Color::Rgb(38, 52, 74);
const BORDER: Color = Color::Rgb(42, 47, 58);
const TITLE: Color = Color::Rgb(139, 149, 168);
const TEXT: Color = Color::Rgb(201, 209, 217);
const MUTED: Color = Color::Rgb(110, 118, 129);
const ACCENT: Color = Color::Rgb(86, 212, 221);
const GREEN: Color = Color::Rgb(126, 231, 135);
const BLUE: Color = Color::Rgb(121, 192, 255);
const YELLOW: Color = Color::Rgb(227, 179, 65);
const RED: Color = Color::Rgb(255, 123, 114);

fn source_color(name: &str) -> Color {
    match name {
        "claude" => Color::Rgb(255, 138, 76),
        "opencode" => BLUE,
        "omp" => GREEN,
        "kilo" => YELLOW,
        "mixed" => MUTED,
        _ => MUTED,
    }
}

fn panel(block_title: &'static str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG))
        .title(Line::from(Span::styled(
            format!(" {} ", block_title),
            Style::default().fg(TITLE),
        )))
}

fn fmt_ts(t: &DateTime<Utc>) -> String {
    t.format("%m-%d %H:%M").to_string()
}

/// Human-readable window length: "1d", "12h", "45m" (whichever is whole, most
/// significant first).
fn fmt_dur(d: Duration) -> String {
    let s = d.num_seconds();
    let days = s / 86_400;
    let hours = (s % 86_400) / 3_600;
    let mins = (s % 3_600) / 60;
    if days > 0 && hours == 0 && mins == 0 {
        format!("{}d", days)
    } else if hours > 0 && mins == 0 {
        format!("{}h", hours)
    } else if days > 0 {
        format!("{}d {}h", days, hours)
    } else if mins > 0 {
        format!("{}m", mins)
    } else {
        format!("{}h", hours)
    }
}

pub fn render(frame: &mut Frame, state: &mut DiffTuiState) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::default().bg(BG)), area);

    let chunks = Layout::default()
        .constraints([
            Constraint::Length(8),  // header: title + window bounds
            Constraint::Length(3),  // sources
            Constraint::Min(8),     // table
            Constraint::Length(3),  // legend + key hints
        ])
        .split(area);

    frame.render_widget(render_header(state), chunks[0]);
    frame.render_widget(render_sources(&state.app.source_statuses), chunks[1]);
    frame.render_widget(render_table(state, chunks[2].width as usize), chunks[2]);
    frame.render_widget(render_footer(), chunks[3]);
}

fn render_header(state: &DiffTuiState) -> Paragraph<'_> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "llmhelper",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  diff", Style::default().fg(TITLE)),
        Span::styled(
            "   prev window vs current window  ·  Δ = curr − prev",
            Style::default().fg(MUTED),
        ),
    ])];

    if let Some((ps, pe, cs, ce, d_prev, d_curr)) = state.app.window_bounds() {
        lines.push(Line::from(vec![
            Span::styled("prev  ", Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} → {}  ({}  earlier)", fmt_ts(ps), fmt_ts(pe), fmt_dur(*d_prev)),
                Style::default().fg(MUTED),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("curr  ", Style::default().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} → {}  (last {})", fmt_ts(cs), fmt_ts(ce), fmt_dur(*d_curr)),
                Style::default().fg(TEXT),
            ),
        ]));
    }

    Paragraph::new(lines).block(panel("llmhelper diff"))
}

fn render_sources(statuses: &[SourceStatus]) -> Paragraph<'_> {
    let mut spans: Vec<Span> = Vec::new();
    if statuses.is_empty() {
        spans.push(Span::styled("no sources", Style::default().fg(MUTED)));
    }
    for s in statuses {
        let ok = s.error.is_none();
        let color = if ok { source_color(&s.name) } else { RED };
        let mark = if ok { "●" } else { "✗" };
        spans.push(Span::styled(
            format!(" {} ", mark),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(
            s.name.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        if let Some(err) = &s.error {
            spans.push(Span::styled(
                format!(" {}", err),
                Style::default().fg(RED),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} records loaded", s.record_count),
                Style::default().fg(MUTED),
            ));
        }
        spans.push(Span::raw("    "));
    }
    Paragraph::new(Line::from(spans)).block(panel("sources"))
}

fn render_footer() -> Paragraph<'static> {
    let keys: &[(&str, &str)] = &[
        ("Tab", "group"),
        ("↑↓", "select"),
        ("r", "refresh"),
        ("q", "quit"),
    ];
    let mut spans = vec![
        Span::styled("status  ", Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        Span::styled("✓ ", Style::default().fg(MUTED)),
        Span::styled("in both windows    ", Style::default().fg(MUTED)),
        Span::styled("+ ", Style::default().fg(GREEN)),
        Span::styled("new this window    ", Style::default().fg(GREEN)),
        Span::styled("− ", Style::default().fg(RED)),
        Span::styled("gone this window    ", Style::default().fg(RED)),
        Span::styled("Δ green = up, red = down    ", Style::default().fg(MUTED)),
        Span::styled("underlined key = selected    ", Style::default().fg(MUTED)),
    ];
    for (k, label) in keys {
        spans.push(Span::styled(
            format!("  {} ", k),
            Style::default()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{}   ", label),
            Style::default().fg(MUTED),
        ));
    }
    Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(BG)),
    )
}

fn fmt_tokens(v: u64) -> String {
    if v < 1_000 {
        return v.to_string();
    }
    let (value, suffix) = if v < 1_000_000 {
        (v as f64 / 1_000.0, "K")
    } else if v < 1_000_000_000 {
        (v as f64 / 1_000.0, "M")
    } else {
        (v as f64 / 1_000_000_000.0, "B")
    };
    let text = format!("{:.1}", value);
    let text = text.strip_suffix(".0").unwrap_or(&text);
    if text == "1000" {
        return match suffix {
            "K" => "1M".to_string(),
            "M" => "1B".to_string(),
            _ => "1000B".to_string(),
        };
    }
    format!("{}{}", text, suffix)
}

/// Signed display value; magnitude is scaled (K/M/B) for tokens, raw for counts.
fn fmt_delta(v: i64) -> String {
    if v >= 0 {
        format!("+{}", fmt_tokens(v as u64))
    } else {
        format!("−{}", fmt_tokens((-v) as u64))
    }
}

/// Color for a signed delta: up = green, down = red, zero = muted.
fn delta_color(v: i64) -> Color {
    if v > 0 {
        GREEN
    } else if v < 0 {
        RED
    } else {
        MUTED
    }
}

fn fmt_cell(v: Option<u64>) -> String {
    v.map(fmt_tokens).unwrap_or_else(|| "—".to_string())
}

/// Percentage display: signed, one decimal; `n/a` when the previous value was 0.
fn fmt_pct(pct: Option<f64>) -> String {
    match pct {
        Some(v) => format!("{:+.1}%", v),
        None => "n/a".to_string(),
    }
}

/// Cost delta display: always shows sign and 6 decimal places, or `n/a` when
/// not computable (mixed-source groups or missing cost in one window).
fn fmt_cost(v: Option<f64>) -> String {
    match v {
        Some(c) => format!("{:+.6}", c),
        None => "n/a".to_string(),
    }
}

/// Message count delta with sign prefix.
fn fmt_delta_msgs(v: i64) -> String {
    if v >= 0 {
        format!("+{}", v)
    } else {
        format!("−{}", -v)
    }
}

/// Per-column display metadata for the two-row header: the group band this
/// column belongs to (prev / curr / Δ) and its name (in / out / ...).
#[derive(Clone, Copy, PartialEq)]
enum ColKind {
    Key,
    Status,
    /// prev or curr window value
    Value,
    /// absolute delta
    Delta,
    /// percentage delta
    Pct,
    /// session count delta
    Sessions,
    /// message count delta
    Messages,
    /// cost delta
    Cost,
}

struct ColMeta {
    kind: ColKind,
    band: &'static str,
    band_color: Color,
    name: &'static str,
}

impl ColMeta {
    fn width(&self) -> Constraint {
        match self.kind {
            ColKind::Key => Constraint::Min(14),
            ColKind::Status => Constraint::Length(4),
            ColKind::Value => Constraint::Length(8),
            ColKind::Delta => Constraint::Length(8),
            ColKind::Pct => Constraint::Length(8),
            ColKind::Sessions => Constraint::Length(6),
            ColKind::Messages => Constraint::Length(7),
            ColKind::Cost => Constraint::Length(8),
        }
    }
}

/// Terminal width from which the Δ% columns are shown.
const SHOW_PCT_WIDTH: usize = 110;

fn build_columns(show_pct: bool) -> Vec<ColMeta> {
    let mut cols = vec![
        ColMeta { kind: ColKind::Key, band: "", band_color: TITLE, name: "Key" },
        ColMeta { kind: ColKind::Status, band: "", band_color: TITLE, name: "st" },
        ColMeta {
            kind: ColKind::Value,
            band: "prev",
            band_color: MUTED,
            name: "in",
        },
        ColMeta { kind: ColKind::Value, band: "", band_color: MUTED, name: "out" },
        ColMeta { kind: ColKind::Value, band: "curr", band_color: TEXT, name: "in" },
        ColMeta { kind: ColKind::Value, band: "", band_color: TEXT, name: "out" },
    ];
    cols.push(ColMeta {
        kind: ColKind::Delta,
        band: "Δ",
        band_color: ACCENT,
        name: "in",
    });
    cols.push(ColMeta {
        kind: ColKind::Delta,
        band: "Δ",
        band_color: ACCENT,
        name: "out",
    });
    if show_pct {
        cols.push(ColMeta {
            kind: ColKind::Pct,
            band: "Δ%",
            band_color: ACCENT,
            name: "in",
        });
        cols.push(ColMeta {
            kind: ColKind::Pct,
            band: "",
            band_color: ACCENT,
            name: "out",
        });
    }
    cols.push(ColMeta {
        kind: ColKind::Sessions,
        band: "",
        band_color: ACCENT,
        name: "sess",
    });
    cols.push(ColMeta {
        kind: ColKind::Messages,
        band: "",
        band_color: ACCENT,
        name: "msgs",
    });
    cols.push(ColMeta {
        kind: ColKind::Cost,
        band: "",
        band_color: ACCENT,
        name: "cost",
    });
    cols
}

fn render_table(state: &mut DiffTuiState, width: usize) -> Table<'static> {
    use crate::diff::Presence;

    let show_pct = width >= SHOW_PCT_WIDTH;
    let cols = build_columns(show_pct);

    let prev = state.app.prev_total_sessions();
    let curr = state.app.curr_total_sessions();
    let delta = curr as i64 - prev as i64;

    let title = format!(
        " {}   ·  {} rows   ·  sessions {} → {}  Δ {}",
        state.app.group_by.label(),
        state.app.rows.len(),
        prev,
        curr,
        fmt_delta(delta),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG))
        .title(Line::from(vec![
            Span::styled("groups", Style::default().fg(TITLE)),
            Span::styled(title, Style::default().fg(ACCENT)),
        ]));

    if state.app.rows.is_empty() {
        return Table::new(
            vec![Row::new(vec![Cell::from(" No groups to compare. ")]).style(
                Style::default().fg(MUTED).bg(BG),
            )],
            vec![Constraint::Min(20)],
        )
        .header(Row::new(vec![Cell::from("")]))
        .block(block);
    }

    let widths: Vec<Constraint> = cols.iter().map(|c| c.width()).collect();

    // Two-level header built from two-line cells: line 1 = the group band
    // label (prev / curr / Δ, shown in the band's first column), line 2 = the
    // per-column name. ratatui's Table takes a single header Row, so the band
    // is approximated by painting the label in its first column only.
    let mut painted_bands: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let header_cells: Vec<Cell> = cols
        .iter()
        .map(|c| {
            let show_label = !c.band.is_empty() && !painted_bands.contains(c.band);
            if show_label {
                painted_bands.insert(c.band);
            }
            let line1 = Line::from(Span::styled(
                format!(" {} ", c.band),
                Style::default()
                    .fg(if show_label { c.band_color } else { Color::Reset })
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right);
            let line2 = Line::from(Span::styled(
                c.name.to_string(),
                Style::default()
                    .fg(TITLE)
                    .add_modifier(Modifier::BOLD),
            ));
            let text = Text::from(vec![line1, line2])
                .alignment(if c.kind == ColKind::Key {
                    Alignment::Left
                } else {
                    Alignment::Right
                });
            Cell::from(text).style(Style::default().bg(SURFACE))
        })
        .collect();
    let header_row = Row::new(header_cells).height(2);

    let selected = state.table_state.selected();
    let rows: Vec<Row> = state
        .app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let is_sel = Some(i) == selected;
            let row_bg = if is_sel {
                HILITE
            } else if i % 2 == 1 {
                SURFACE
            } else {
                BG
            };

            let num = |s: String, fg: Color, right: bool, bold: bool| -> Cell {
                let mut style = Style::default().fg(fg).bg(row_bg);
                if bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if right {
                    Cell::from(Text::from(s).alignment(Alignment::Right)).style(style)
                } else {
                    Cell::new(s).style(style)
                }
            };

            // Status column: ✓ both / + new / − removed.
            let (status, status_color) = match row.presence {
                Presence::Both => ("✓", MUTED),
                Presence::New => ("+", GREEN),
                Presence::Removed => ("−", RED),
            };

            let prev_in = fmt_cell(row.prev.as_ref().map(|s| s.tokens.input));
            let prev_out = fmt_cell(row.prev.as_ref().map(|s| s.tokens.output));
            let curr_in = fmt_cell(row.curr.as_ref().map(|s| s.tokens.input));
            let curr_out = fmt_cell(row.curr.as_ref().map(|s| s.tokens.output));

            let d_in = fmt_delta(row.delta.tokens.input);
            let d_out = fmt_delta(row.delta.tokens.output);
            let d_sess = fmt_delta(row.delta.sessions);
            let p_in = fmt_pct(row.delta.pct.as_ref().and_then(|p| p.input));
            let p_out = fmt_pct(row.delta.pct.as_ref().and_then(|p| p.output));

            let mut cells: Vec<Cell> = vec![
                Cell::new(row.key.clone()).style(
                    Style::default()
                        .fg(TEXT)
                        .bg(row_bg)
                        .add_modifier(if is_sel {
                            Modifier::BOLD | Modifier::UNDERLINED
                        } else {
                            Modifier::empty()
                        }),
                ),
                num(status.to_string(), status_color, true, true),
                num(prev_in, MUTED, true, false),
                num(prev_out, MUTED, true, false),
                num(curr_in, TEXT, true, false),
                num(curr_out, TEXT, true, false),
                num(d_in, delta_color(row.delta.tokens.input), true, true),
                num(d_out, delta_color(row.delta.tokens.output), true, true),
            ];
            if show_pct {
                cells.push(num(p_in, delta_color(row.delta.tokens.input), true, false));
                cells.push(num(p_out, delta_color(row.delta.tokens.output), true, false));
            }
            cells.push(num(d_sess, delta_color(row.delta.sessions), true, false));
            cells.push(Cell::new(fmt_delta_msgs(row.delta.messages)).style(
                Style::default().fg(delta_color(row.delta.messages)).bg(row_bg),
            ));
            // Cost color: green if positive, red if negative, muted if zero or n/a.
            let cost_fg = match row.delta.cost {
                Some(c) if c > 0.0 => GREEN,
                Some(c) if c < 0.0 => RED,
                _ => MUTED,
            };
            cells.push(Cell::new(fmt_cost(row.delta.cost)).style(
                Style::default().fg(cost_fg).bg(row_bg),
            ));

            let row = Row::new(cells).height(1);
            row
        })
        .collect();

    Table::new(rows, widths)
        .header(header_row)
        .block(block)
        .style(Style::default().bg(BG))
}
