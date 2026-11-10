//! `herdr-multiaccount statusline`: set as Claude Code's status line command.
//! Records the usage windows and cost Claude Code hands it for the account
//! the session runs on, tells you once when fallback starts moving new
//! sessions, then runs the status line command you had before with the same
//! input, so your status line looks as before.

use crate::config::Config;
use crate::herdr;
use crate::launch;
use crate::mirror;
use crate::paths;
use crate::store::{Store, MAIN_ID};
use crate::time;
use crate::usage::Usage;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// The account of the session whose status line this is.
pub fn account_of_session(store: &Store) -> Option<String> {
    if let Ok(id) = std::env::var(launch::ENV_ID) {
        if store.account(&id).is_some() {
            return Some(id);
        }
    }
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => {
            let dir = Path::new(&dir);
            if dir.parent() == Some(paths::homes_dir().as_path()) {
                let id = dir.file_name()?.to_string_lossy().to_string();
                return store.account(&id).map(|a| a.id.clone());
            }
            let same = |a: &Path, b: &Path| a.canonicalize().ok() == b.canonicalize().ok();
            same(dir, &paths::claude_dir()).then(|| MAIN_ID.to_string())
        }
        // A session started without the shell function, with a key of its
        // own, is not one of the accounts.
        None if std::env::var_os("ANTHROPIC_API_KEY").is_some() => None,
        None => Some(MAIN_ID.to_string()),
    }
}

pub fn run() -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    let parsed: Value = serde_json::from_slice(&input).unwrap_or(Value::Null);
    let store = Store::load();
    let config = Config::load(&paths::config_dir());

    if parsed.is_object() {
        if let Some(id) = account_of_session(&store) {
            let now = time::now();
            let _ = Usage::update(&id, |usage| usage.record(&parsed, now));
            announce(&store, &config, &id, now);
            crate::remind::check(&store, &config, &id, now);
        }
        note_conversation(&parsed);
    }

    // Keeps mirror folders in step when settings.json was saved over a link.
    if let Ok(id) = std::env::var(launch::ENV_ID) {
        if store.account(&id).is_some_and(|a| a.kind.has_home()) {
            let _ = mirror::sync_dirs(
                &paths::claude_dir(),
                &mirror::home_of(&id),
                &id,
                &paths::backups_dir(),
            );
        }
    }

    match store
        .statusline_original
        .as_deref()
        .filter(|c| !c.trim().is_empty())
    {
        Some(original) => run_original(original, &input),
        None => {
            print!("{}", default_line(&store, &parsed));
            0
        }
    }
}

/// Notes which conversation this pane holds, so the list can move it to
/// another account (`relocate.rs`). Only ids shaped like Claude Code's
/// session ids are kept: the id goes into a command typed into a shell.
fn note_conversation(input: &Value) {
    if !herdr::inside() {
        return;
    }
    let (Ok(pane), Some(id)) = (std::env::var("HERDR_PANE_ID"), input["session_id"].as_str())
    else {
        return;
    };
    if !crate::sessions::valid_session_id(id) {
        return;
    }
    let pids = crate::sessions::my_ancestors();
    if pids.is_empty() {
        return;
    }
    crate::sessions::record_conversation(
        &pane,
        &crate::sessions::Conversation {
            session_id: id.to_string(),
            pids,
        },
    );
}

/// Tells once per full window when new sessions move to another account.
fn announce(store: &Store, config: &Config, id: &str, now: i64) {
    if !config.fallback || !config.notify || !herdr::inside() {
        return;
    }
    let usage = launch::load_usage(store);
    let Some(limit) = usage
        .get(id)
        .and_then(|u| u.at_limit(config.threshold, now))
    else {
        return;
    };
    // Only accounts that new sessions would otherwise get.
    let wanted_somewhere = store.active == id || store.spaces.iter().any(|s| s.account == id);
    let seen = |list: &[(String, i64)]| {
        list.iter()
            .any(|(n, at)| n == id && (*at - limit.resets_at).abs() <= 300)
    };
    if !wanted_somewhere || seen(&store.notified) {
        return;
    }
    // Claim the notice under the lock, so two sessions never both show it.
    let mut claimed = false;
    let saved = Store::update(|s| {
        if !seen(&s.notified) {
            s.notified.retain(|(_, at)| *at > now);
            s.notified.push((id.to_string(), limit.resets_at));
            claimed = true;
        }
    });
    if saved.is_err() || !claimed {
        return;
    }
    let check = |a: &str| launch::usable(store, a);
    let next = launch::fallback_from(store, &usage, &check, id, config.threshold, now);
    let name = store.name_of(id);
    let title = format!("{name} is at its {} limit", limit.window);
    let body = match &next {
        Some(n) => format!(
            "New Claude Code sessions use {} until it resets in {}. To move a running one: /exit, then claude --continue.",
            store.name_of(n),
            time::span(limit.resets_at - now)
        ),
        None => format!(
            "No other account has room; it resets in {}.",
            time::span(limit.resets_at - now)
        ),
    };
    herdr::notify(&title, &body);
}

fn run_original(command: &str, input: &[u8]) -> i32 {
    let child = Command::new("/bin/sh")
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    let Ok(mut child) = child else {
        return 1;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    child.wait().ok().and_then(|s| s.code()).unwrap_or(1)
}

/// A status line of its own when there was none before: the account and
/// its usage.
fn default_line(store: &Store, input: &Value) -> String {
    let id = account_of_session(store).unwrap_or_default();
    let mut parts = vec![store.name_of(&id)];
    let rl = &input["rate_limits"];
    for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
        if let Some(p) = rl[key]["used_percentage"].as_f64() {
            parts.push(format!("{label} {p:.0}%"));
        }
    }
    if let Some(model) = input["model"]["display_name"].as_str() {
        parts.push(model.to_string());
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_line_names_account_and_windows() {
        let store = Store::default();
        let line = default_line(
            &store,
            &json!({"rate_limits": {"five_hour": {"used_percentage": 23.4}}, "model": {"display_name": "Opus"}}),
        );
        assert!(line.contains("5h 23%"));
        assert!(line.ends_with("Opus"));
    }
}
