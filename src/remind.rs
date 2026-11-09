//! A reminder that a session can go back: fallback started it on another
//! account because the one it should have had was full, and that one has
//! room again. A running session cannot change its login, so the reminder
//! asks you to restart it (`/exit`, then `claude --continue`).
//!
//! The session's status line notices it (it runs every 30 seconds, also
//! while the session is idle) and starts `herdr-multiaccount remind`, which
//! opens a dialog over whatever tab is in front in Herdr. It stays until it
//! is answered. Each session is reminded once.

use crate::config::Config;
use crate::herdr;
use crate::sessions::{self, Entry};
use crate::store::Store;
use crate::usage::Usage;
use std::process::{Command, Stdio};

/// The account to go back to, when the reminder is due.
pub fn due(
    note: &Entry,
    session_account: &str,
    store: &Store,
    wanted_usage: &Usage,
    threshold: f64,
    now: i64,
) -> Option<String> {
    let wanted = note.wanted.as_deref()?;
    if note.told || note.account != session_account || wanted == session_account {
        return None;
    }
    store.account(wanted)?;
    wanted_usage
        .at_limit(threshold, now)
        .is_none()
        .then(|| wanted.to_string())
}

/// From the status line: starts the reminder when it is due.
pub fn check(store: &Store, config: &Config, session_account: &str, now: i64) {
    if !config.remind || !herdr::inside() {
        return;
    }
    let Ok(pane) = std::env::var("HERDR_PANE_ID") else {
        return;
    };
    let Some(note) = sessions::get(&pane) else {
        return;
    };
    let Some(wanted) = note.wanted.as_deref() else {
        return;
    };
    // With its twins' readings: the same login may have filled up elsewhere.
    let usage = crate::launch::load_usage(store)
        .remove(wanted)
        .unwrap_or_default();
    let Some(back) = due(&note, session_account, store, &usage, config.threshold, now) else {
        return;
    };
    if !crate::launch::usable(store, &back) || !sessions::mark_told(&pane) {
        return;
    }
    // On its own, detached: the status line must not wait for an answer.
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    use std::os::unix::process::CommandExt;
    let _ = Command::new(exe)
        .args(["remind", &pane, &back, session_account])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

pub fn texts(session: &str, back: &str, now_on: &str) -> (String, String) {
    (
        format!("{back} has room again"),
        format!(
            "The session \"{session}\" still runs on {now_on}. To move it to {back}: type /exit in it, then claude --continue. The conversation carries on."
        ),
    )
}

/// Where the reminder pane finds what it is about: pane, account to go
/// back to and the account the session runs on, separated by tabs.
const ENV: &str = "HERDR_MULTIACCOUNT_REMIND";

/// Popup width in cells, border included.
const WIDTH: u16 = 64;

/// `herdr-multiaccount remind <pane> <back> <now on>`: opens the reminder
/// as a Herdr popup over whatever is in front; a Herdr notification when
/// that fails.
pub fn show(pane: &str, back: &str, now_on: &str) -> i32 {
    let store = Store::load();
    let label = herdr::pane_label(pane).unwrap_or_else(|| pane.to_string());
    let (title, body) = texts(&label, &store.name_of(back), &store.name_of(now_on));
    let value = format!("{pane}\t{back}\t{now_on}");
    // Border, title, a gap, the text, a gap, the buttons, border.
    let lines = crate::ui::wrap(&body, (WIDTH - 4) as usize).len() as u16;
    if herdr::open_popup("remind", WIDTH, lines + 6, &[(ENV, &value)]) {
        return 0;
    }
    herdr::notify(&title, &body);
    0
}

/// The reminder pane: a dialog that stays until it is answered. "Go to
/// session" brings the session's tab to the front.
pub fn pane() -> i32 {
    let value = std::env::var(ENV).unwrap_or_default();
    let parts: Vec<&str> = value.split('\t').collect();
    let [pane, back, now_on] = parts.as_slice() else {
        return 2;
    };
    let store = Store::load();
    let label = herdr::pane_label(pane).unwrap_or_else(|| pane.to_string());
    let (title, body) = texts(&label, &store.name_of(back), &store.name_of(now_on));
    let mut terminal = crate::enter_terminal();
    let go = dialog(&mut terminal, &title, &body).unwrap_or(false);
    crate::leave_terminal();
    if go {
        if let Some(tab) = herdr::pane_tab(pane) {
            herdr::focus_tab(&tab);
        }
    }
    0
}

/// Draws the dialog until a button is chosen: true for "Go to session".
fn dialog(
    terminal: &mut ratatui::DefaultTerminal,
    title: &str,
    body: &str,
) -> std::io::Result<bool> {
    use crate::theme::{self, ACCENT, BORDER, MUTED, TEXT};
    use ratatui::crossterm::event::{
        self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind,
    };
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span, Text};
    use ratatui::widgets::{Clear, Paragraph};

    let mut go_area = Rect::default();
    let mut later_area = Rect::default();
    loop {
        terminal.draw(|f| {
            // Herdr draws the popup's border; this fills its inside.
            let area = f.area();
            let inner = Rect::new(
                area.x + 1,
                area.y,
                area.width.saturating_sub(2),
                area.height,
            );
            let mut text = vec![
                Line::from(Span::styled(
                    title.to_string(),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for l in crate::ui::wrap(body, inner.width as usize) {
                text.push(Line::from(Span::styled(l, Style::new().fg(TEXT))));
            }
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(Text::from(text)),
                Rect::new(
                    inner.x,
                    inner.y,
                    inner.width,
                    inner.height.saturating_sub(2),
                ),
            );
            let y = inner.y + inner.height.saturating_sub(1);
            let go = " ⏎ Go to session ";
            let later = " Esc later ";
            go_area = Rect::new(inner.x, y, go.chars().count() as u16, 1);
            later_area = Rect::new(go_area.x + go_area.width + 1, y, later.len() as u16, 1);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        go,
                        Style::new()
                            .fg(theme::on(ACCENT))
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(later, Style::new().fg(MUTED).bg(BORDER)),
                ])),
                Rect::new(inner.x, y, inner.width, 1),
            );
        })?;
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter | KeyCode::Char('g') => return Ok(true),
                KeyCode::Esc | KeyCode::Char('l') | KeyCode::Char('q') => return Ok(false),
                _ => {}
            },
            Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                let inside = |r: Rect| {
                    m.column >= r.x
                        && m.column < r.x + r.width
                        && m.row >= r.y
                        && m.row < r.y + r.height
                };
                if inside(go_area) {
                    return Ok(true);
                }
                if inside(later_area) {
                    return Ok(false);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Account, Kind};
    use crate::usage::Window;

    const NOW: i64 = 1_794_225_600;

    fn store() -> Store {
        let mut s = Store::default();
        for id in ["max", "key"] {
            s.accounts.push(Account {
                id: id.into(),
                name: id.into(),
                kind: Kind::Subscription,
                sso: false,
                fallback: true,
            });
        }
        s
    }

    fn note(told: bool) -> Entry {
        Entry {
            account: "key".into(),
            pid: 1,
            wanted: Some("max".into()),
            told,
        }
    }

    #[test]
    fn reminds_once_when_room_is_back() {
        let s = store();
        let full = Usage {
            seven_day: Some(Window {
                used: 100.0,
                resets_at: NOW + 60,
            }),
            ..Usage::default()
        };
        assert_eq!(due(&note(false), "key", &s, &full, 95.0, NOW), None);
        // After the reset the window counts as fresh.
        assert_eq!(
            due(&note(false), "key", &s, &full, 95.0, NOW + 61).as_deref(),
            Some("max")
        );
        assert_eq!(due(&note(true), "key", &s, &full, 95.0, NOW + 61), None);
        // A note of another session in the same pane does not count.
        assert_eq!(due(&note(false), "main", &s, &full, 95.0, NOW + 61), None);
        // Not moved by fallback: nothing to go back to.
        let mut plain = note(false);
        plain.wanted = None;
        assert_eq!(due(&plain, "key", &s, &full, 95.0, NOW + 61), None);
    }

    #[test]
    fn reminder_text() {
        let (title, body) = texts("acme › claude", "Max", "Key");
        assert_eq!(title, "Max has room again");
        assert!(body.contains("/exit") && body.contains("claude --continue"));
    }
}
