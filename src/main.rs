//! Herdr Multiaccount: several Claude accounts side by side, their usage
//! limits, one account per space, and fallback when one is full.
//!
//! Commands:
//!
//! - `list` (default): the list, in the current terminal.
//! - `open [overlay|split|tab]`: the plugin action; opens or closes the list.
//! - `run -- <args>`: what the `claude` shell function calls.
//! - `statusline`: Claude Code's status line command after setup.
//! - `setup [--undo]`, `status`, `use <account>`: for the command line.
//! - `startup`, `pane-event`, `install-bin`: Herdr hooks and the build.

mod app;
mod config;
mod herdr;
mod info;
mod launch;
mod login;
mod mirror;
mod pane;
mod paths;
mod probe;
mod remind;
mod secret;
mod sessions;
mod setup;
mod signin;
mod statusline;
mod store;
mod theme;
mod time;
mod ui;
mod usage;

use config::Config;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use store::Store;

pub fn enter_terminal() -> ratatui::DefaultTerminal {
    // ratatui's own panic hook restores raw mode and the screen; this one
    // also turns mouse reporting and bracketed paste off again.
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = ratatui::crossterm::execute!(
                std::io::stdout(),
                DisableBracketedPaste,
                DisableMouseCapture
            );
            previous(info);
        }));
    });
    let terminal = ratatui::init();
    let _ =
        ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    terminal
}

pub fn leave_terminal() {
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture
    );
    ratatui::restore();
}

fn list() -> std::io::Result<()> {
    let dir = paths::config_dir();
    let _ = setup::install_bin();
    let config = Config::load(&dir);
    let app = app::App::new(config, dir);
    let mut terminal = enter_terminal();
    let result = app.run(&mut terminal);
    leave_terminal();
    pane::forget_self();
    result
}

/// After Herdr starts: the copy for the shell, and the space labels in the
/// sidebar (spaces found again by name when their id changed).
fn startup() -> i32 {
    let _ = setup::install_bin();
    // Backups made by older versions sit in a folder others could open.
    let backups = paths::backups_dir();
    if backups.is_dir() {
        let _ = paths::private_dir(&backups);
    }
    let Some(live) = herdr::workspaces() else {
        return 0;
    };
    // Spaces whose id is gone are found again by name, but only among live
    // spaces nobody assigned yet (two spaces can share a name).
    let store = Store::update(|store| {
        let assigned: Vec<String> = store
            .spaces
            .iter()
            .filter(|s| live.iter().any(|w| w.id == s.workspace_id))
            .map(|s| s.workspace_id.clone())
            .collect();
        let mut taken = assigned.clone();
        for space in store.spaces.iter_mut() {
            if let Some(w) = live.iter().find(|w| w.id == space.workspace_id) {
                space.label = w.label.clone();
                continue;
            }
            if space.label.is_empty() {
                continue;
            }
            if let Some(w) = live
                .iter()
                .find(|w| w.label == space.label && !taken.contains(&w.id))
            {
                space.workspace_id = w.id.clone();
                taken.push(w.id.clone());
            }
        }
    });
    let store = match store {
        Ok(s) => s,
        Err(_) => Store::load(),
    };
    for s in &store.spaces {
        if live.iter().any(|w| w.id == s.workspace_id) {
            herdr::tag_workspace(&s.workspace_id, Some(&store.name_of(&s.account)));
        }
    }
    0
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(true)
}

/// On agent events: clears a pane's account label once the Claude Code it
/// was made for has ended.
fn pane_event() -> i32 {
    let pane = std::env::var("HERDR_PANE_ID").ok().or_else(|| {
        let v: serde_json::Value =
            serde_json::from_str(&std::env::var("HERDR_PLUGIN_EVENT_JSON").ok()?).ok()?;
        find_pane_id(&v)
    });
    let Some(pane) = pane else {
        return 0;
    };
    if let Some(entry) = sessions::get(&pane) {
        if !alive(entry.pid) {
            herdr::untag_pane(&pane);
            sessions::remove(&pane);
        }
    }
    0
}

fn find_pane_id(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(m) => {
            if let Some(id) = m.get("pane_id").and_then(|p| p.as_str()) {
                return Some(id.to_string());
            }
            m.values().find_map(find_pane_id)
        }
        serde_json::Value::Array(a) => a.iter().find_map(find_pane_id),
        _ => None,
    }
}

/// `status`: the accounts as text.
fn status() -> i32 {
    let store = Store::load();
    let config = Config::load(&paths::config_dir());
    let twins = login::twins(&store, &login::all(&store));
    let usage = launch::load_usage_with(&store, &twins);
    let now = time::now();
    for a in &store.accounts {
        let u = usage.get(&a.id).cloned().unwrap_or_default();
        let w = |w: Option<usage::Window>, name: &str| match w.and_then(|w| w.current(now)) {
            Some(p) => format!("{name} {p:.0}%"),
            None => format!("{name} –"),
        };
        let mark = if store.active == a.id { "*" } else { " " };
        let limit = u
            .at_limit(config.threshold, now)
            .map(|l| format!("  at {} limit", l.window))
            .unwrap_or_default();
        let same = twins
            .get(&a.id)
            .map(|t| {
                let names: Vec<String> = t.iter().map(|i| store.name_of(i)).collect();
                format!("  same login as {}", names.join(", "))
            })
            .unwrap_or_default();
        println!(
            "{mark} {:<20} {:<13} {:<10} {:<10} ${:.2}{limit}{same}",
            a.name,
            a.kind.as_str(),
            w(u.five_hour, "5h"),
            w(u.seven_day, "7d"),
            u.month_usd(now)
        );
    }
    let s = setup::state();
    if !s.done() {
        println!("\nnot set up: run `herdr-multiaccount setup`");
    }
    0
}

fn use_account(name: Option<String>) -> i32 {
    let store = Store::load();
    let Some(id) = name
        .as_deref()
        .and_then(|n| store.find(n))
        .map(|a| a.id.clone())
    else {
        eprintln!("usage: herdr-multiaccount use <account name or id>");
        return 2;
    };
    match Store::update(|s| s.active = id.clone()) {
        Ok(s) => {
            println!("new sessions use {}", s.name_of(&id));
            0
        }
        Err(e) => {
            eprintln!("herdr-multiaccount: {e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().cloned().unwrap_or_else(|| "list".into());
    let code = match command.as_str() {
        "list" => match list() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("herdr-multiaccount: {e}");
                1
            }
        },
        "open" => {
            let config = Config::load(&paths::config_dir());
            let placement = match args.get(1).map(String::as_str) {
                Some("split") => config::Placement::Split,
                Some("tab") => config::Placement::Tab,
                Some("overlay") => config::Placement::Overlay,
                _ => config.placement,
            };
            match pane::toggle(placement, config.split_direction) {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        "run" => {
            // Only the first `--` separates; later ones are Claude Code's.
            let skip = if args.get(1).map(String::as_str) == Some("--") {
                2
            } else {
                1
            };
            launch::run(args[skip.min(args.len())..].to_vec())
        }
        "statusline" => statusline::run(),
        "startup" => startup(),
        "pane-event" => pane_event(),
        "remind-pane" => remind::pane(),
        "remind" => match (args.get(1), args.get(2), args.get(3)) {
            (Some(pane), Some(back), Some(now_on)) => remind::show(pane, back, now_on),
            _ => 2,
        },
        "install-bin" => match setup::install_bin() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("herdr-multiaccount: {e}");
                1
            }
        },
        "setup" => {
            let result = if args.get(1).map(String::as_str) == Some("--undo") {
                setup::undo()
            } else {
                setup::apply()
            };
            match result {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(e) => {
                    eprintln!("herdr-multiaccount: {e}");
                    1
                }
            }
        }
        "status" => status(),
        "use" => use_account(args.get(1).cloned()),
        "--version" | "-V" => {
            println!("herdr-multiaccount {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!(
                "usage: herdr-multiaccount [list|open [overlay|split|tab]|run -- <claude args>|statusline|setup [--undo]|status|use <account>|--version]"
            );
            2
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_id_in_event() {
        let v = serde_json::json!({"event": "pane.agent_status_changed", "data": {"pane": {"pane_id": "w1:p2"}}});
        assert_eq!(find_pane_id(&v).as_deref(), Some("w1:p2"));
        assert_eq!(find_pane_id(&serde_json::json!({"x": 1})), None);
    }
}
