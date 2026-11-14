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

/// The size of a popup with [`dialog`] for `body`: border, title, a gap,
/// the text, a gap, the buttons, border.
pub fn popup_size(body: &str) -> (u16, u16) {
    let lines = crate::ui::wrap(body, (WIDTH - 4) as usize).len() as u16;
    (WIDTH, lines + 6)
}

/// `herdr-multiaccount remind <pane> <back> <now on>`: opens the reminder
/// as a Herdr popup over whatever is in front; a Herdr notification when
/// that fails.
pub fn show(pane: &str, back: &str, now_on: &str) -> i32 {
    let store = Store::load();
    let label = herdr::pane_label(pane).unwrap_or_else(|| pane.to_string());
    let (title, body) = texts(&label, &store.name_of(back), &store.name_of(now_on));
    let value = format!("{pane}\t{back}\t{now_on}");
    let (width, height) = popup_size(&body);
    if herdr::open_popup("remind", width, height, &[(ENV, &value)]) {
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
    let go = dialog(&mut terminal, &title, &body, " `Esc` later ").unwrap_or(false);
    crate::leave_terminal();
    if go {
        if let Some(tab) = herdr::pane_tab(pane) {
            herdr::focus_tab(&tab);
        }
    }
    0
}

/// Draws the dialog until a button is chosen: true for "Go to session",
/// false for the other button (`other`, its text, with its key between
/// backticks as for `theme::keys`). Also the popup of a failed move
/// (`relocate::moved_pane`).
pub fn dialog(
    terminal: &mut ratatui::DefaultTerminal,
    title: &str,
    body: &str,
    other: &str,
) -> std::io::Result<bool> {
    use ratatui::crossterm::event::{
        self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind,
    };
    use ratatui::layout::Rect;

    let mut go_area = Rect::default();
    let mut later_area = Rect::default();
    loop {
        terminal.draw(|f| (go_area, later_area) = draw_dialog(f, title, body, other))?;
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

/// One frame of [`dialog`]; gives the places of its two buttons.
fn draw_dialog(
    f: &mut ratatui::Frame,
    title: &str,
    body: &str,
    other: &str,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    use crate::theme::{self, ACCENT, BORDER, MUTED, TEXT};
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span, Text};
    use ratatui::widgets::{Clear, Paragraph};
    use unicode_width::UnicodeWidthStr;

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
    let later = theme::keys(other);
    let go_area = Rect::new(inner.x, y, go.width() as u16, 1);
    let later_area = Rect::new(go_area.x + go_area.width + 1, y, later.width() as u16, 1);
    // The accent button stays one block; the grey one shows its key.
    let mut buttons = vec![
        Span::styled(
            go,
            Style::new()
                .fg(theme::on(ACCENT))
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    buttons.extend(crate::ui::with_keys(
        &later,
        Style::new().fg(MUTED).bg(BORDER),
    ));
    f.render_widget(
        Paragraph::new(Line::from(buttons)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    (go_area, later_area)
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
            options: None,
            dropped: vec![],
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

    #[test]
    fn the_grey_button_shows_its_key_and_the_notification_holds_no_mark() {
        use crate::theme::{ACCENT, BORDER, KEY_MARK};
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;
        let (title, body) = texts("acme › claude", "`d`", "Key");
        // A Herdr notification shows text as it is: no marks in it.
        assert!(!title.contains(KEY_MARK) && !body.contains(KEY_MARK));
        for other in [" `Esc` later ", " `Esc` close "] {
            let mut t = ratatui::Terminal::new(TestBackend::new(64, 12)).unwrap();
            let mut areas = Default::default();
            t.draw(|f| areas = draw_dialog(f, &title, &body, other))
                .unwrap();
            let (go, later) = areas;
            let buf = t.backend().buffer();
            let key = |x: u16| {
                let c = &buf[(x, later.y)];
                c.fg == ACCENT && c.modifier.contains(Modifier::BOLD)
            };
            // " Esc later ": the key on the grey, the word not.
            assert_eq!(later.width as usize, other.len() - 2, "{other}");
            for x in later.x..later.x + later.width {
                assert_eq!(buf[(x, later.y)].bg, BORDER, "{other}");
                let in_key = (later.x + 1..later.x + 4).contains(&x);
                assert_eq!(key(x), in_key, "{other} at {x}");
            }
            // The accent button stays one block.
            for x in go.x..go.x + go.width {
                assert_eq!(buf[(x, go.y)].bg, ACCENT);
                assert_ne!(buf[(x, go.y)].fg, ACCENT);
            }
            // The title keeps its bold accent (the owner's decision).
            assert!(key_at(buf, 1, 0));
        }
    }

    fn key_at(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> bool {
        let c = &buf[(x, y)];
        c.fg == crate::theme::ACCENT && c.modifier.contains(ratatui::style::Modifier::BOLD)
    }
}
