//! Drawing: section tabs, the accounts, spaces or sessions table, dialogs
//! and a footer. Records clickable areas for the mouse.

use crate::app::{App, Hit, Mode, Section};
use crate::herdr::Workspace;
use crate::store::{Account, Kind};
use crate::theme::{
    self, Icons, ACCENT, BAR_BG, BORDER, FAINT, GREEN, MUTED, RED, SELECTED_BG, TEXT, TRACK, YELLOW,
};
use crate::time;
use crate::usage::{Usage, Window};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    let icons = Icons {
        nerd: app.config.nerd_font,
    };
    let height = frame.area().height;
    let spacer = if height >= 14 { 1 } else { 0 };
    let [header, _, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(spacer),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, &icons, header);
    let body = if !app.setup.done() && body.height > 6 {
        let [notice, _, rest] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .areas(body);
        draw_setup_notice(frame, app, &icons, notice);
        rest
    } else {
        body
    };
    match app.section {
        Section::Accounts => draw_accounts(frame, app, &icons, body),
        Section::Spaces => draw_spaces(frame, app, &icons, body),
        Section::Sessions => draw_sessions(frame, app, &icons, body),
    }
    draw_footer(frame, app, &icons, footer);
    match app.mode.clone() {
        Mode::Help => draw_help(frame, app),
        Mode::Pick(p) => draw_picker(frame, app, &p),
        Mode::Input(i) => draw_input(frame, &i),
        Mode::Confirm(c) => draw_confirm(frame, app, &c),
        Mode::SignIn { code } => draw_sign_in(frame, app, &code),
        Mode::Message { title, lines, good } => draw_message(frame, app, &title, &lines, good),
        _ => {}
    }
}

fn draw_header(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    let line = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1);
    let title = format!("{} Claude accounts   ", icons.title());
    let mut spans = vec![Span::styled(
        title.clone(),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    let mut x = line.x + title.width() as u16;
    for section in Section::ORDER {
        let count = match section {
            Section::Accounts => app.store.accounts.len(),
            Section::Spaces => app.workspaces.len(),
            Section::Sessions => app.sessions().len(),
        };
        let label = format!(" {} ({count}) ", section.name());
        let style = if app.section == section {
            Style::new()
                .fg(TEXT)
                .bg(SELECTED_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(MUTED)
        };
        let w = label.width() as u16;
        app.hits
            .push((Rect::new(x, line.y, w, 1), Hit::Section(section)));
        x += w + 1;
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    let left = Line::from(spans);
    let used = left.width();
    frame.render_widget(Paragraph::new(left), line);

    let chip = if app.config.fallback {
        (
            format!(
                "{} fallback at {:.0}%",
                icons.fallback(),
                app.config.threshold
            ),
            Style::new().fg(MUTED),
        )
    } else {
        (
            format!("{} fallback off", icons.fallback()),
            Style::new().fg(FAINT),
        )
    };
    let w = chip.0.width();
    if used + w + 3 <= line.width as usize {
        let x = line.x + line.width - w as u16;
        app.hits
            .push((Rect::new(x, line.y, w as u16, 1), Hit::Key('F')));
        frame.render_widget(
            Paragraph::new(Span::styled(chip.0, chip.1)).alignment(Alignment::Right),
            line,
        );
    }
}

fn draw_setup_notice(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    let missing = match (app.setup.shell, app.setup.statusline) {
        (false, false) => "claude does not pick accounts yet and limits are not recorded",
        (false, true) => "claude does not pick accounts yet",
        _ => "limits are not recorded yet",
    };
    let line = Line::from(vec![
        Span::styled(format!(" {} ", icons.warning()), Style::new().fg(YELLOW)),
        Span::styled(format!("Not set up: {missing}.  "), Style::new().fg(TEXT)),
        Span::styled("S", Style::new().fg(YELLOW).add_modifier(Modifier::BOLD)),
        Span::styled(" set up", Style::new().fg(MUTED)),
    ]);
    app.hits.push((area, Hit::Key('S')));
    frame.render_widget(Paragraph::new(line), area);
}

// ---- accounts --------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Mark,
    Account,
    FiveHour,
    SevenDay,
    Month,
    Spaces,
    Seen,
}

/// Room between columns: like the Git Panel, wider on wide panes.
fn spacing(width: u16) -> u16 {
    if width >= 80 {
        3
    } else {
        1
    }
}

/// The column headings on a bar of their own, like the header of a table in
/// an app (same look as the Git Panel).
fn heading_row<'a>(names: impl IntoIterator<Item = &'a str>) -> Row<'a> {
    Row::new(names.into_iter().map(Cell::from)).style(
        Style::new()
            .fg(MUTED)
            .bg(BAR_BG)
            .add_modifier(Modifier::BOLD),
    )
}

fn account_columns(width: u16) -> Vec<(Col, Constraint)> {
    let mut cols = vec![
        (Col::Mark, Constraint::Length(2)),
        (Col::Account, Constraint::Fill(1)),
        (Col::FiveHour, Constraint::Length(17)),
        (Col::SevenDay, Constraint::Length(17)),
        (Col::Month, Constraint::Length(10)),
        (Col::Spaces, Constraint::Length(11)),
        (Col::Seen, Constraint::Length(6)),
    ];
    // Narrow panes drop columns before the account gets shorter than 26.
    let gap = spacing(width);
    for drop in [
        Col::Seen,
        Col::Spaces,
        Col::Month,
        Col::SevenDay,
        Col::FiveHour,
    ] {
        let fixed: u16 = cols
            .iter()
            .map(|(_, c)| match c {
                Constraint::Length(n) => n + gap,
                _ => gap,
            })
            .sum();
        if width >= fixed + 26 {
            break;
        }
        cols.retain(|(c, _)| *c != drop);
    }
    cols
}

fn bar(used: f64, width: usize) -> Vec<Span<'static>> {
    let filled = ((used.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let filled = if used > 0.0 { filled.max(1) } else { 0 }.min(width);
    vec![
        Span::styled("━".repeat(filled), Style::new().fg(theme::level(used))),
        Span::styled("━".repeat(width - filled), Style::new().fg(TRACK)),
        Span::styled(
            format!(" {:>3.0}%", used),
            Style::new().fg(theme::level(used)),
        ),
    ]
}

fn window_cell(w: Option<Window>, usage: &Usage, kind: Kind, now: i64) -> Text<'static> {
    match w {
        Some(w) => match w.current(now) {
            Some(used) => Text::from(vec![
                Line::from(bar(used, 10)),
                Line::from(Span::styled(
                    format!("resets in {}", time::span(w.resets_at - now)),
                    Style::new().fg(FAINT),
                )),
            ]),
            None => Text::from(vec![
                Line::from(bar(0.0, 10)),
                Line::from(Span::styled("new window", Style::new().fg(FAINT))),
            ]),
        },
        None => {
            let note = if kind == Kind::ApiKey
                || ((usage.updated_at.is_some() || usage.probed_at.is_some())
                    && !usage.has_windows())
            {
                "no limit"
            } else {
                "no reading yet"
            };
            Text::from(vec![
                Line::from(Span::styled("–", Style::new().fg(FAINT))),
                Line::from(Span::styled(note, Style::new().fg(FAINT))),
            ])
        }
    }
}

/// The second line under an account's name: where it stands.
fn status_line(app: &App, a: &Account) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if app.store.active == a.id {
        spans.push(Span::styled(
            "active",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::new().fg(FAINT)));
    }
    let info = app.info.get(&a.id);
    let now = app.now();
    let limit = app
        .usage
        .get(&a.id)
        .and_then(|u| u.at_limit(app.config.threshold, now));
    match info {
        None if app.info_loading => {
            spans.push(Span::styled("checking…", Style::new().fg(FAINT)));
            return Line::from(spans);
        }
        Some(i) if !i.signed_in => {
            let text = match a.kind {
                Kind::ApiKey => "no key stored · l adds it",
                _ => "not signed in · l signs in",
            };
            spans.push(Span::styled(text, Style::new().fg(RED)));
            return Line::from(spans);
        }
        _ => {}
    }
    // Two entries with one login share one set of limits: say so, so the
    // list never looks like it had two separate ones.
    let twins = app.twin_names(&a.id);
    if !twins.is_empty() {
        spans.push(Span::styled(
            format!("same login as {}", twins.join(", ")),
            Style::new().fg(YELLOW),
        ));
        spans.push(Span::styled(" · ", Style::new().fg(FAINT)));
    }
    if let Some(l) = limit {
        let choice = app.effective(&a.id);
        let text = if choice.id != a.id && app.config.fallback {
            format!(
                "at its {} limit · new sessions use {}",
                l.window,
                app.store.name_of(&choice.id)
            )
        } else {
            format!("at its {} limit", l.window)
        };
        spans.push(Span::styled(text, Style::new().fg(YELLOW)));
        return Line::from(spans);
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(i) = info {
        if let Some(p) = &i.plan {
            parts.push(p.clone());
        }
        match (&i.organization, &i.email) {
            (Some(o), Some(e)) if a.kind != Kind::ApiKey => parts.push(format!("{o} · {e}")),
            (Some(o), None) => parts.push(o.clone()),
            (None, Some(e)) => parts.push(e.clone()),
            _ => {}
        }
    } else if a.kind == Kind::ApiKey {
        parts.push("API key".into());
    }
    if a.sso {
        parts.push("SSO".into());
    }
    if !a.fallback {
        parts.push("no fallback".into());
    }
    if parts.is_empty() {
        // Nothing after the separator.
        if spans.last().is_some_and(|s| s.content == " · ") {
            spans.pop();
        }
    } else {
        spans.push(Span::styled(parts.join(" · "), Style::new().fg(MUTED)));
    }
    Line::from(spans)
}

fn account_cell(app: &App, c: Col, a: &Account, icons: &Icons) -> Text<'static> {
    let now = app.now();
    let empty = Usage::default();
    let usage = app.usage.get(&a.id).unwrap_or(&empty);
    match c {
        Col::Mark => {
            if app.store.active == a.id {
                Text::from(Span::styled(icons.active(), Style::new().fg(ACCENT)))
            } else {
                Text::from("")
            }
        }
        Col::Account => {
            let mut name = vec![Span::styled(
                a.name.clone(),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            )];
            if a.kind == Kind::ApiKey {
                name.push(Span::styled(
                    format!(" {}", icons.key()),
                    Style::new().fg(FAINT),
                ));
            }
            Text::from(vec![Line::from(name), status_line(app, a)])
        }
        Col::FiveHour => window_cell(usage.five_hour, usage, a.kind, now),
        Col::SevenDay => match &usage.spend {
            // Behind a gateway the spend limit takes the 7-day place.
            Some(s) if usage.seven_day.is_none() => {
                let mut t = window_cell(Some(s.window), usage, a.kind, now);
                if let (Some(u), Some(l)) = (s.used_usd, s.limit_usd) {
                    t.lines[1] = Line::from(Span::styled(
                        format!("${u:.0} of ${l:.0}"),
                        Style::new().fg(FAINT),
                    ));
                }
                t
            }
            _ => window_cell(usage.seven_day, usage, a.kind, now),
        },
        Col::Month => {
            let usd = usage.month_usd(now);
            if usd > 0.0 {
                Text::from(vec![
                    Line::from(Span::styled(format!("${usd:.2}"), Style::new().fg(TEXT))),
                    Line::from(Span::styled("this month", Style::new().fg(FAINT))),
                ])
            } else {
                Text::from(Span::styled("–", Style::new().fg(FAINT)))
            }
        }
        Col::Spaces => {
            let spaces = app.spaces_using(&a.id);
            let running = app.running(&a.id);
            let (text, color) = match spaces {
                0 => ("no spaces".to_string(), FAINT),
                1 => ("1 space".to_string(), TEXT),
                n => (format!("{n} spaces"), TEXT),
            };
            let mut lines = vec![Line::from(Span::styled(text, Style::new().fg(color)))];
            if running > 0 {
                lines.push(Line::from(Span::styled(
                    format!("{} {running} running", icons.running()),
                    Style::new().fg(GREEN),
                )));
            }
            Text::from(lines)
        }
        Col::Seen => match usage.updated_at {
            Some(t) => Text::from(Span::styled(time::short(now - t), Style::new().fg(FAINT))),
            None => Text::from(Span::styled("–", Style::new().fg(FAINT))),
        },
    }
}

fn heading(c: Col) -> &'static str {
    match c {
        Col::Mark => "",
        Col::Account => "Account",
        Col::FiveHour => "5-hour",
        Col::SevenDay => "7-day",
        Col::Month => "Month",
        Col::Spaces => "Used by",
        Col::Seen => "Seen",
    }
}

/// The first row to show so the selected one is visible.
fn offset(selected: usize, rows: usize, visible: usize) -> usize {
    if visible == 0 || rows <= visible {
        return 0;
    }
    selected.saturating_sub(visible - 1).min(rows - visible)
}

fn draw_accounts(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    let cols = account_columns(area.width.saturating_sub(2));
    let row_h = 3u16; // two lines and a gap
    let visible = (area.height.saturating_sub(1) / row_h).max(1) as usize;
    let accounts = app.store.accounts.clone();
    let first = offset(app.sel, accounts.len(), visible);
    let header = heading_row(cols.iter().map(|(c, _)| heading(*c)));
    let mut rows = Vec::new();
    for (i, a) in accounts.iter().enumerate().skip(first).take(visible) {
        let cells: Vec<Cell> = cols
            .iter()
            .map(|(c, _)| Cell::from(account_cell(app, *c, a, icons)))
            .collect();
        let mut row = Row::new(cells).height(2).bottom_margin(1);
        if i == app.sel {
            row = row.style(Style::new().bg(SELECTED_BG));
        }
        rows.push(row);
        let y = area.y + 1 + ((i - first) as u16) * row_h;
        if y + 2 <= area.y + area.height {
            app.hits
                .push((Rect::new(area.x, y, area.width, 2), Hit::Row(i)));
        }
    }
    let widths: Vec<Constraint> = cols.iter().map(|(_, w)| *w).collect();
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(spacing(area.width.saturating_sub(2)));
    frame.render_widget(
        table,
        Rect::new(
            area.x + 1,
            area.y,
            area.width.saturating_sub(2),
            area.height,
        ),
    );
}

// ---- spaces ----------------------------------------------------------

fn draw_spaces(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    if app.workspaces.is_empty() {
        let text = if crate::herdr::inside() {
            "Reading the spaces…"
        } else {
            "Spaces are Herdr's: open this list inside Herdr."
        };
        frame.render_widget(
            Paragraph::new(Span::styled(text, Style::new().fg(FAINT))),
            Rect::new(area.x + 2, area.y, area.width.saturating_sub(4), 1),
        );
        return;
    }
    let wide = area.width >= 70;
    let mut widths = vec![
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ];
    if wide {
        widths.push(Constraint::Length(30));
    }
    let mut head = vec!["", "Space", "Account"];
    if wide {
        head.push("Sessions");
    }
    let header = heading_row(head);
    let row_h = 3u16;
    let visible = (area.height.saturating_sub(1) / row_h).max(1) as usize;
    let spaces: Vec<Workspace> = app.workspaces.clone();
    let first = offset(app.space_sel, spaces.len(), visible);
    let mut rows = Vec::new();
    for (i, ws) in spaces.iter().enumerate().skip(first).take(visible) {
        let assigned = app.store.space_account(&ws.id).map(str::to_string);
        let wanted = assigned.clone().unwrap_or_else(|| app.store.active.clone());
        let choice = app.effective(&wanted);
        let mark = if ws.focused {
            Span::styled(icons.space(), Style::new().fg(ACCENT))
        } else {
            Span::raw("")
        };
        let space = Text::from(vec![
            Line::from(Span::styled(
                ws.label.clone(),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!("space {}", ws.number),
                Style::new().fg(FAINT),
            )),
        ]);
        let first_line = match &assigned {
            Some(id) => Line::from(Span::styled(
                app.store.name_of(id),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            )),
            None => Line::from(vec![
                Span::styled("active account", Style::new().fg(MUTED)),
                Span::styled(
                    format!(" ({})", app.store.name_of(&app.store.active)),
                    Style::new().fg(FAINT),
                ),
            ]),
        };
        let second = if choice.id != choice.wanted {
            Line::from(Span::styled(
                format!(
                    "at its limit · new sessions use {}",
                    app.store.name_of(&choice.id)
                ),
                Style::new().fg(YELLOW),
            ))
        } else {
            let u = app.usage.get(&choice.id);
            let now = app.now();
            let mut parts = Vec::new();
            if let Some(u) = u {
                if let Some(p) = u.five_hour.and_then(|w| w.current(now)) {
                    parts.push(format!("5-hour {p:.0}%"));
                }
                if let Some(p) = u.seven_day.and_then(|w| w.current(now)) {
                    parts.push(format!("7-day {p:.0}%"));
                }
            }
            Line::from(Span::styled(parts.join(" · "), Style::new().fg(FAINT)))
        };
        let mut cells = vec![
            Cell::from(mark),
            Cell::from(space),
            Cell::from(Text::from(vec![first_line, second])),
        ];
        if wide {
            let claude: Vec<&crate::herdr::Pane> = app
                .panes
                .iter()
                .filter(|p| p.workspace == ws.id)
                .filter(|p| app.is_session(p))
                .collect();
            let mut names: Vec<String> = claude
                .iter()
                .filter_map(|p| p.account_id.as_deref())
                .map(|id| app.store.name_of(id))
                .collect();
            names.sort();
            names.dedup();
            // Started before setup, or not through the claude function: the
            // plugin cannot tell which account these use.
            let unknown = claude.iter().filter(|p| p.account_id.is_none()).count();
            let mut second = vec![Span::styled(names.join(", "), Style::new().fg(FAINT))];
            if unknown > 0 {
                if !names.is_empty() {
                    second.push(Span::styled(" · ", Style::new().fg(FAINT)));
                }
                second.push(Span::styled(
                    format!("{unknown} without account"),
                    Style::new().fg(YELLOW),
                ));
            }
            cells.push(Cell::from(if claude.is_empty() {
                Text::from(Span::styled("–", Style::new().fg(FAINT)))
            } else {
                Text::from(vec![
                    Line::from(Span::styled(
                        format!("{} {} running", icons.running(), claude.len()),
                        Style::new().fg(GREEN),
                    )),
                    Line::from(second),
                ])
            }));
        }
        let mut row = Row::new(cells).height(2).bottom_margin(1);
        if i == app.space_sel {
            row = row.style(Style::new().bg(SELECTED_BG));
        }
        rows.push(row);
        let y = area.y + 1 + ((i - first) as u16) * row_h;
        if y + 2 <= area.y + area.height {
            app.hits
                .push((Rect::new(area.x, y, area.width, 2), Hit::Row(i)));
        }
    }
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(spacing(area.width.saturating_sub(2)));
    frame.render_widget(
        table,
        Rect::new(
            area.x + 1,
            area.y,
            area.width.saturating_sub(2),
            area.height,
        ),
    );
}

// ---- sessions --------------------------------------------------------

/// What an agent status looks like: colour and word.
fn agent_status(status: Option<&str>) -> (ratatui::style::Color, &'static str) {
    match status {
        Some("working") => (GREEN, "working"),
        Some("blocked") => (YELLOW, "waiting for you"),
        Some("done") => (ACCENT, "done"),
        Some("idle") => (MUTED, "idle"),
        _ => (FAINT, "–"),
    }
}

fn draw_sessions(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    let sessions = app.sessions();
    if sessions.is_empty() {
        let text = if !crate::herdr::inside() {
            "Sessions are Herdr's: open this list inside Herdr."
        } else if app.panes.is_empty() {
            "Reading the sessions…"
        } else {
            "No Claude Code session runs in Herdr."
        };
        frame.render_widget(
            Paragraph::new(Span::styled(text, Style::new().fg(FAINT))),
            Rect::new(area.x + 2, area.y, area.width.saturating_sub(4), 1),
        );
        return;
    }
    let wide = area.width >= 90;
    let mut widths = vec![
        Constraint::Length(2),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ];
    let mut head = vec!["", "Session", "Account"];
    if wide {
        widths.push(Constraint::Length(26));
        head.push("Limits of the account");
    }
    let header = heading_row(head);
    let row_h = 3u16;
    let visible = (area.height.saturating_sub(1) / row_h).max(1) as usize;
    let first = offset(app.session_sel, sessions.len(), visible);
    let now = app.now();
    let mut rows = Vec::new();
    for (i, p) in sessions.iter().enumerate().skip(first).take(visible) {
        let (color, word) = agent_status(p.status.as_deref());
        let label = if p.label.is_empty() {
            p.id.clone()
        } else {
            p.label.clone()
        };
        let session = Text::from(vec![
            Line::from(Span::styled(
                label,
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(word, Style::new().fg(color)),
                Span::styled(
                    format!(" · {}", app.space_label(&p.workspace)),
                    Style::new().fg(FAINT),
                ),
            ]),
        ]);
        let known = p
            .account_id
            .as_deref()
            .filter(|id| app.store.account(id).is_some());
        let account = match (known, &p.account) {
            (Some(id), _) => {
                let a = app.store.account(id).cloned();
                let plan = app.info.get(id).and_then(|i| i.plan.clone());
                let detail = match a.as_ref().map(|a| a.kind) {
                    Some(Kind::ApiKey) => "API key".to_string(),
                    Some(Kind::Main) => plan.unwrap_or_else(|| "~/.claude".into()),
                    _ => plan.unwrap_or_else(|| "subscription".into()),
                };
                Text::from(vec![
                    Line::from(Span::styled(
                        app.store.name_of(id),
                        Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(detail, Style::new().fg(FAINT))),
                ])
            }
            (None, Some(name)) => Text::from(vec![
                Line::from(Span::styled(
                    name.clone(),
                    Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled("account removed", Style::new().fg(YELLOW))),
            ]),
            // Started before setup, or not through the claude function:
            // such a session uses ~/.claude, so the main login, unless its
            // environment says otherwise, which the plugin cannot see.
            (None, None) => Text::from(vec![
                Line::from(Span::styled(
                    format!("{} (likely)", app.store.name_of(crate::store::MAIN_ID)),
                    Style::new().fg(MUTED),
                )),
                Line::from(Span::styled(
                    "started without this plugin",
                    Style::new().fg(YELLOW),
                )),
            ]),
        };
        let mut cells = vec![
            Cell::from(Span::styled(icons.running(), Style::new().fg(color))),
            Cell::from(session),
            Cell::from(account),
        ];
        if wide {
            let mut parts: Vec<Line> = Vec::new();
            if let Some(u) = known.and_then(|id| app.usage.get(id)) {
                for (name, w) in [("5-hour", u.five_hour), ("7-day", u.seven_day)] {
                    if let Some(used) = w.and_then(|w| w.current(now)) {
                        let mut spans =
                            vec![Span::styled(format!("{name:<7}"), Style::new().fg(FAINT))];
                        spans.extend(bar(used, 10));
                        parts.push(Line::from(spans));
                    }
                }
            }
            if parts.is_empty() {
                parts.push(Line::from(Span::styled("–", Style::new().fg(FAINT))));
            }
            cells.push(Cell::from(Text::from(parts)));
        }
        let mut row = Row::new(cells).height(2).bottom_margin(1);
        if i == app.session_sel {
            row = row.style(Style::new().bg(SELECTED_BG));
        }
        rows.push(row);
        let y = area.y + 1 + ((i - first) as u16) * row_h;
        if y + 2 <= area.y + area.height {
            app.hits
                .push((Rect::new(area.x, y, area.width, 2), Hit::Row(i)));
        }
    }
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(spacing(area.width.saturating_sub(2)));
    frame.render_widget(
        table,
        Rect::new(
            area.x + 1,
            area.y,
            area.width.saturating_sub(2),
            area.height,
        ),
    );
}

// ---- footer ----------------------------------------------------------

fn draw_footer(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    frame.render_widget(Block::new().style(Style::new().bg(BAR_BG)), area);
    if app.mode == Mode::AskQuit {
        let question = " Close the accounts list? ";
        let qw = question.width() as u16;
        let line = Line::from(vec![
            Span::styled(question, Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(
                " y close ",
                Style::new()
                    .fg(theme::on(RED))
                    .bg(RED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(" any other key stays ", Style::new().fg(TEXT).bg(BORDER)),
        ]);
        app.hits
            .push((Rect::new(area.x + qw, area.y, 9, 1), Hit::Key('y')));
        app.hits
            .push((Rect::new(area.x + qw + 10, area.y, 21, 1), Hit::Key('n')));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let right = if let Some((text, color, _)) = &app.flash {
        Span::styled(format!("{text} "), Style::new().fg(*color))
    } else if app.info_loading {
        Span::styled(
            format!("{} checking accounts… ", icons.refresh()),
            Style::new().fg(YELLOW),
        )
    } else if app.probing {
        Span::styled(
            format!("{} reading limits… ", icons.refresh()),
            Style::new().fg(FAINT),
        )
    } else {
        Span::styled(
            format!(
                "{} read {} ago ",
                icons.refresh(),
                time::short(app.read_at.elapsed().as_secs() as i64)
            ),
            Style::new().fg(FAINT),
        )
    };
    let refresh = if app.flash.is_none() && !app.info_loading {
        vec![
            Span::styled("r", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(" refresh  ", Style::new().fg(FAINT)),
        ]
    } else {
        vec![]
    };
    let key_w: u16 = refresh.iter().map(|s| s.width() as u16).sum();
    let right_w = (right.width() as u16 + key_w).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    if key_w > 0 {
        app.hits.push((
            Rect::new(right_area.x, area.y, key_w.saturating_sub(2), 1),
            Hit::Key('r'),
        ));
    }
    let mut rs = refresh;
    rs.push(right);
    frame.render_widget(
        Paragraph::new(Line::from(rs)).alignment(Alignment::Right),
        right_area,
    );

    let keys: Vec<(&str, &str, char)> = match app.section {
        Section::Accounts => vec![
            ("⏎", "make active", '\n'),
            ("a", "add", 'a'),
            ("c", "start Claude", 'c'),
            ("?", "all keys", '?'),
            ("q", "close", 'q'),
        ],
        Section::Spaces => vec![
            ("⏎", "choose account", '\n'),
            ("x", "use active", 'x'),
            ("?", "all keys", '?'),
            ("q", "close", 'q'),
        ],
        Section::Sessions => vec![
            ("⏎", "go to session", '\n'),
            ("?", "all keys", '?'),
            ("q", "close", 'q'),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    let mut x = left_area.x + 1;
    for (key, what, click) in keys {
        let text = format!(" {what}  ");
        let width = (key.width() + text.width()) as u16;
        if x + width > left_area.x + left_area.width {
            break;
        }
        app.hits
            .push((Rect::new(x, area.y, width - 2, 1), Hit::Key(click)));
        x += width;
        spans.push(Span::styled(
            key,
            Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(text, Style::new().fg(FAINT)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), left_area);
}

// ---- dialogs ---------------------------------------------------------

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width
        .min(area.width.saturating_sub(2))
        .max(10.min(area.width));
    let h = height
        .min(area.height.saturating_sub(2))
        .max(3.min(area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn dialog(frame: &mut Frame, area: Rect, title: &str) -> Rect {
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    )
}

fn draw_picker(frame: &mut Frame, app: &mut App, p: &crate::app::Picker) {
    let want_h = p.items.len() as u16 * 2 + 4;
    let area = centered(frame.area(), 72, want_h);
    let inner = dialog(frame, area, &p.title);
    let list_h = inner.height.saturating_sub(2);
    let per = 2u16;
    let visible = (list_h / per).max(1) as usize;
    let first = offset(p.sel, p.items.len(), visible);
    for (i, item) in p.items.iter().enumerate().skip(first).take(visible) {
        let y = inner.y + ((i - first) as u16) * per;
        let r = Rect::new(inner.x, y, inner.width, per.min(inner.y + list_h - y));
        let style = if i == p.sel {
            Style::new().bg(SELECTED_BG)
        } else {
            Style::new()
        };
        let text = Text::from(vec![
            Line::from(vec![
                Span::styled(format!("{} ", i + 1), Style::new().fg(FAINT)),
                Span::styled(
                    item.label.clone(),
                    Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                format!("  {}", item.detail),
                Style::new().fg(MUTED),
            )),
        ]);
        frame.render_widget(Paragraph::new(text).style(style), r);
        app.hits.push((r, Hit::PickRow(i)));
    }
    let foot = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("⏎", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(" choose  ", Style::new().fg(FAINT)),
            Span::styled("Esc", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(" back", Style::new().fg(FAINT)),
        ])),
        foot,
    );
}

fn draw_input(frame: &mut Frame, i: &crate::app::Input) {
    let area = centered(frame.area(), 72, 9);
    let inner = dialog(frame, area, &i.title);
    let field = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    frame.render_widget(Block::new().style(Style::new().bg(SELECTED_BG)), field);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if i.secret {
                    // Dots, at most as many as fit; the length shows a paste
                    // arrived.
                    format!(" {}", "•".repeat(i.text.chars().count().min(40)))
                } else {
                    format!(" {}", i.text)
                },
                Style::new().fg(TEXT),
            ),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ])),
        field,
    );
    if inner.height > 3 {
        frame.render_widget(
            Paragraph::new(Text::from(
                wrap(&i.hint, inner.width as usize)
                    .into_iter()
                    .map(|l| Line::from(Span::styled(l, Style::new().fg(MUTED))))
                    .collect::<Vec<_>>(),
            )),
            Rect::new(
                inner.x,
                inner.y + 3,
                inner.width,
                inner.height.saturating_sub(4),
            ),
        );
    }
    let foot = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(1),
        inner.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("⏎", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(" ok  ", Style::new().fg(FAINT)),
            Span::styled("Esc", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(" cancel", Style::new().fg(FAINT)),
        ])),
        foot,
    );
}

fn draw_confirm(frame: &mut Frame, app: &mut App, c: &crate::app::Confirm) {
    let width = 76u16;
    let wrapped: Vec<Line> = c
        .lines
        .iter()
        .flat_map(|l| wrap(l, width.saturating_sub(6) as usize))
        .map(|l| Line::from(Span::styled(l, Style::new().fg(MUTED))))
        .collect();
    let area = centered(frame.area(), width, wrapped.len() as u16 + 5);
    let inner = dialog(frame, area, &c.question);
    frame.render_widget(
        Paragraph::new(Text::from(wrapped)),
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(3),
        ),
    );
    let y = inner.y + inner.height.saturating_sub(1);
    let yes = " y yes ";
    let no = " n no ";
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                yes,
                Style::new()
                    .fg(theme::on(GREEN))
                    .bg(GREEN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(no, Style::new().fg(TEXT).bg(BORDER)),
        ])),
        Rect::new(inner.x, y, inner.width, 1),
    );
    app.hits.push((Rect::new(inner.x, y, 7, 1), Hit::Key('y')));
    app.hits
        .push((Rect::new(inner.x + 8, y, 6, 1), Hit::Key('n')));
}

fn key_hint(key: &'static str, what: &'static str) -> [Span<'static>; 2] {
    [
        Span::styled(key, Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
        Span::styled(what, Style::new().fg(FAINT)),
    ]
}

fn draw_sign_in(frame: &mut Frame, app: &mut App, code: &str) {
    let Some(s) = app.signin.as_ref() else {
        return;
    };
    let width = 78u16;
    let text_w = width.saturating_sub(6) as usize;
    let mut lines: Vec<Line> = Vec::new();
    let para = |t: &str, style: Style, lines: &mut Vec<Line>| {
        for l in wrap(t, text_w) {
            lines.push(Line::from(Span::styled(l, style)));
        }
    };
    para(
        "Claude Code opened its sign-in page in your browser. Sign in there with the account you want for this one.",
        Style::new().fg(TEXT),
        &mut lines,
    );
    lines.push(Line::from(""));
    para(
        "Is the browser already signed in to another Claude account? Then sign out there first, or open the link in a private window, or this account ends up with that login.",
        Style::new().fg(MUTED),
        &mut lines,
    );
    lines.push(Line::from(""));
    let asks = s.output.to_lowercase().contains("paste code");
    if asks {
        lines.push(Line::from(Span::styled(
            "If the page shows a code, paste it here and press Enter:",
            Style::new().fg(MUTED),
        )));
    }
    let spin = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let tick = (s.started.elapsed().as_millis() / 120) as usize % spin.len();
    let status = if s.code_sent {
        format!("{} Checking the code…", spin[tick])
    } else {
        format!(
            "{} Waiting for the sign-in  {}",
            spin[tick],
            time::short(s.started.elapsed().as_secs() as i64)
        )
    };
    let field_lines = if asks { 2 } else { 0 };
    let height = lines.len() as u16 + field_lines + 6;
    let area = centered(frame.area(), width, height);
    let inner = dialog(frame, area, &format!("Sign in {}", s.name));
    let body_h = lines.len() as u16;
    frame.render_widget(
        Paragraph::new(Text::from(lines)),
        Rect::new(inner.x, inner.y + 1, inner.width, body_h.min(inner.height)),
    );
    let mut y = inner.y + 1 + body_h;
    if asks && y < inner.y + inner.height {
        let field = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(Block::new().style(Style::new().bg(SELECTED_BG)), field);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {code}"), Style::new().fg(TEXT)),
                Span::styled("▏", Style::new().fg(ACCENT)),
            ])),
            field,
        );
        y += 2;
    }
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Span::styled(status, Style::new().fg(YELLOW))),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    let foot_y = inner.y + inner.height.saturating_sub(1);
    let mut foot: Vec<Span> = Vec::new();
    let mut x = inner.x;
    let mut hint =
        |key: &'static str, what: &'static str, hit: char, foot: &mut Vec<Span<'static>>| {
            let [k, w] = key_hint(key, what);
            let width = (k.width() + w.width()) as u16;
            app.hits.push((
                Rect::new(x, foot_y, width.saturating_sub(2), 1),
                Hit::Key(hit),
            ));
            x += width;
            foot.push(k);
            foot.push(w);
        };
    if s.url.is_some() && code.is_empty() {
        hint("o", " open again  ", 'o', &mut foot);
        hint("y", " copy link  ", 'y', &mut foot);
    }
    if asks && !code.is_empty() {
        hint("⏎", " send code  ", '\n', &mut foot);
    }
    hint("Esc", " cancel", '\u{1b}', &mut foot);
    frame.render_widget(
        Paragraph::new(Line::from(foot)),
        Rect::new(inner.x, foot_y, inner.width, 1),
    );
}

fn draw_message(frame: &mut Frame, app: &mut App, title: &str, text: &[String], good: bool) {
    let width = 78u16.min(frame.area().width.saturating_sub(2)).max(24);
    let mut lines: Vec<Line> = Vec::new();
    for (i, t) in text.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        let style = if i == 0 {
            Style::new().fg(TEXT)
        } else {
            Style::new().fg(if good { MUTED } else { YELLOW })
        };
        for l in wrap(t, width.saturating_sub(6) as usize) {
            lines.push(Line::from(Span::styled(l, style)));
        }
    }
    let area = centered(frame.area(), width, lines.len() as u16 + 5);
    let inner = dialog(frame, area, title);
    frame.render_widget(
        Paragraph::new(Text::from(lines)),
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(3),
        ),
    );
    let y = inner.y + inner.height.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " ⏎ ok ",
            Style::new()
                .fg(theme::on(ACCENT))
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        )])),
        Rect::new(inner.x, y, inner.width, 1),
    );
    app.hits.push((Rect::new(inner.x, y, 6, 1), Hit::Key('\n')));
}

/// Word wrap for plain text.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        // A word longer than the line (a path) is split, never cut off.
        for c in word.chars() {
            if line.width() + c.to_string().width() > width {
                out.push(std::mem::take(&mut line));
            }
            line.push(c);
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn draw_help(frame: &mut Frame, app: &mut App) {
    let groups: [(&str, &[(&str, &str)]); 4] = [
        (
            "Accounts",
            &[
                ("⏎  Space", "make active: new sessions use it"),
                ("a", "add an account (subscription, SSO or API key)"),
                ("l", "sign in again, or store the key again"),
                ("n", "rename"),
                ("d", "remove (asks first)"),
                ("c", "start Claude Code with it in a new tab"),
                ("f", "take part in fallback or not"),
                (
                    "K J  Shift+↑ ↓",
                    "move up / down (fallback goes down the list)",
                ),
            ],
        ),
        (
            "Spaces",
            &[
                ("⏎", "choose the account for the space"),
                ("x", "back to the active account"),
            ],
        ),
        ("Sessions", &[("⏎", "go to the session's tab")]),
        (
            "List",
            &[
                ("↑ ↓  j k", "move"),
                ("← →  Tab  1 2 3", "accounts / spaces / sessions"),
                ("F", "fallback on / off"),
                ("S", "set up the claude command and the limits"),
                ("r", "refresh now"),
                ("i", "Nerd Font icons / plain symbols"),
                ("Esc", "close a dialog (never the list)"),
                ("q", "close the list (asks first)"),
            ],
        ),
    ];
    let mut lines: Vec<Line> = Vec::new();
    for (gi, (title, keys)) in groups.iter().enumerate() {
        if gi > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *title,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        for (k, what) in keys.iter() {
            if k.is_empty() {
                lines.push(Line::from(Span::styled(*what, Style::new().fg(MUTED))));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{k:<18}"),
                        Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*what, Style::new().fg(MUTED)),
                ]));
            }
        }
    }
    let how = |width: usize| -> Vec<Line<'static>> {
        let mut out = vec![Line::from(Span::styled(
            "How it works",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))];
        for (i, p) in crate::app::INTRO.iter().enumerate() {
            if i > 0 {
                out.push(Line::from(""));
            }
            for l in wrap(p, width) {
                out.push(Line::from(Span::styled(l, Style::new().fg(MUTED))));
            }
        }
        out
    };
    let screen = frame.area();
    let width = 78u16.min(screen.width.saturating_sub(2)).max(24);
    let one_column = how(width.saturating_sub(6) as usize);
    let two_columns =
        screen.width >= 124 && (screen.height as usize) < lines.len() + 1 + one_column.len() + 3;
    let (area, columns): (Rect, Vec<Vec<Line<'static>>>) = if two_columns {
        // Keys on the left, the note on the right.
        let right = how(52);
        let height = lines.len().max(right.len()) as u16 + 3;
        (centered(screen, 124, height), vec![lines, right])
    } else {
        lines.push(Line::from(""));
        lines.extend(one_column);
        let height = lines.len() as u16 + 3;
        (centered(screen, width, height), vec![lines])
    };
    let inner = dialog(frame, area, "Keys");
    app.hits.push((area, Hit::Key('?')));
    // Taller than the screen: scroll, with a line that says so.
    let longest = columns.iter().map(Vec::len).max().unwrap_or(0) as u16;
    let overflow = longest > inner.height;
    let visible = if overflow {
        inner.height.saturating_sub(1)
    } else {
        inner.height
    };
    app.help_max = longest.saturating_sub(visible);
    app.help_scroll = app.help_scroll.min(app.help_max);
    let body = Rect::new(inner.x, inner.y, inner.width, visible);
    let areas: Vec<Rect> = if columns.len() == 2 {
        let [l, _, r] = Layout::horizontal([
            Constraint::Length(64),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(body);
        vec![l, r]
    } else {
        vec![body]
    };
    for (col, a) in columns.into_iter().zip(areas) {
        frame.render_widget(
            Paragraph::new(Text::from(col)).scroll((app.help_scroll, 0)),
            a,
        );
    }
    if overflow {
        let at_end = app.help_scroll >= app.help_max;
        let hint = if at_end {
            "↑ ↓ scroll · end".to_string()
        } else {
            format!("↑ ↓ scroll · {} more lines", app.help_max - app.help_scroll)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, Style::new().fg(FAINT))),
            Rect::new(inner.x, inner.y + visible, inner.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_when_narrow() {
        assert_eq!(account_columns(160).len(), 7);
        let narrow = account_columns(60);
        assert!(narrow.iter().any(|(c, _)| *c == Col::Account));
        assert!(narrow.iter().all(|(c, _)| *c != Col::Seen));
    }

    #[test]
    fn offsets_and_wrap() {
        assert_eq!(offset(0, 10, 3), 0);
        assert_eq!(offset(5, 10, 3), 3);
        assert_eq!(offset(9, 10, 3), 7);
        assert_eq!(offset(2, 2, 3), 0);
        assert_eq!(wrap("a bb ccc dddd", 10), vec!["a bb ccc", "dddd"]);
        assert_eq!(
            wrap("run /a/very/long/path/name now", 10),
            vec!["run", "/a/very/lo", "ng/path/na", "me now"]
        );
    }

    #[test]
    fn bars() {
        let b = bar(50.0, 10);
        assert_eq!(b[0].content.chars().count(), 5);
        assert_eq!(b[1].content.chars().count(), 5);
        let tiny = bar(0.4, 10);
        assert_eq!(tiny[0].content.chars().count(), 1, "any use shows");
    }
}
