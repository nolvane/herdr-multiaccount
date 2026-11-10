//! `herdr-multiaccount run -- <claude args>`: what the `claude` shell function
//! calls. Picks the account (the space's, else the active one; the next one
//! in the list when that one is at its limit), prepares its folder, and
//! replaces itself with the real `claude`, so signals and exit codes are
//! Claude Code's own.

use crate::config::Config;
use crate::herdr;
use crate::info;
use crate::login;
use crate::mirror;
use crate::paths;
use crate::secret;
use crate::store::{Kind, Store};
use crate::time;
use crate::usage::{Limit, Usage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// Set for Claude Code and everything it starts: the account it runs on.
pub const ENV_ID: &str = "HERDR_MULTIACCOUNT_ID";
/// Set by hand or by the list's "start with this account": use exactly this
/// account (id or name), without fallback.
pub const ENV_USE: &str = "HERDR_MULTIACCOUNT_USE";
/// The variables this plugin set, so a nested start can take them back.
const ENV_SET: &str = "HERDR_MULTIACCOUNT_SET";

/// Credentials that outrank a claude.ai login in Claude Code.
const OVERRIDING: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// Why an account was picked.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Named with `HERDR_MULTIACCOUNT_USE`.
    Explicit,
    /// The Claude Code this one runs inside uses it.
    Inherited,
    /// The space's account.
    Space(String),
    Active,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The account the space or the active setting asks for.
    pub wanted: String,
    /// The account the session gets.
    pub id: String,
    pub source: Source,
    /// Set when fallback moved the session: why.
    pub moved: Option<Limit>,
}

/// The account fallback moves to from `from`: the next one in the list
/// (wrapping around) that takes part in fallback and is not at its limit.
pub fn fallback_from(
    store: &Store,
    usage: &HashMap<String, Usage>,
    usable: &dyn Fn(&str) -> bool,
    from: &str,
    threshold: f64,
    now: i64,
) -> Option<String> {
    let n = store.accounts.len();
    let start = store.accounts.iter().position(|a| a.id == from)?;
    (1..n)
        .map(|k| &store.accounts[(start + k) % n])
        .find(|a| {
            a.fallback
                && usage
                    .get(&a.id)
                    .and_then(|u| u.at_limit(threshold, now))
                    .is_none()
                && usable(&a.id)
        })
        .map(|a| a.id.clone())
}

/// Picks the account for a new session.
pub fn choose(
    store: &Store,
    config: &Config,
    usage: &HashMap<String, Usage>,
    usable: &dyn Fn(&str) -> bool,
    wanted: String,
    source: Source,
    now: i64,
) -> Choice {
    let mut choice = Choice {
        id: wanted.clone(),
        wanted,
        source,
        moved: None,
    };
    if !config.fallback || choice.source == Source::Explicit {
        return choice;
    }
    let Some(limit) = usage
        .get(&choice.wanted)
        .and_then(|u| u.at_limit(config.threshold, now))
    else {
        return choice;
    };
    if let Some(next) = fallback_from(store, usage, usable, &choice.wanted, config.threshold, now) {
        choice.id = next;
        choice.moved = Some(limit);
    }
    choice
}

/// What a space or the active setting asks for, with the reason.
pub fn wanted(store: &Store) -> (String, Source) {
    if let Ok(key) = std::env::var(ENV_USE) {
        if let Some(a) = store.find(key.trim()) {
            return (a.id.clone(), Source::Explicit);
        }
    }
    if let Ok(id) = std::env::var(ENV_ID) {
        if store.account(&id).is_some() {
            return (id, Source::Inherited);
        }
    }
    if herdr::inside() {
        if let Ok(pane) = std::env::var("HERDR_PANE_ID") {
            if let Some((ws, label)) = herdr::pane_workspace(&pane) {
                if let Some(acc) = store.space_account(&ws) {
                    return (acc.to_string(), Source::Space(label));
                }
            }
        } else if let Ok(ws) = std::env::var("HERDR_WORKSPACE_ID") {
            if let Some(acc) = store.space_account(&ws) {
                return (acc.to_string(), Source::Space(ws));
            }
        }
    }
    (store.active.clone(), Source::Active)
}

/// Every account's readings, twins (accounts with the same login) sharing
/// the freshest limits.
pub fn load_usage(store: &Store) -> HashMap<String, Usage> {
    load_usage_with(store, &login::twins(store, &login::all(store)))
}

pub fn load_usage_with(
    store: &Store,
    twins: &HashMap<String, Vec<String>>,
) -> HashMap<String, Usage> {
    let mut usage = store
        .accounts
        .iter()
        .map(|a| (a.id.clone(), Usage::load(&a.id)))
        .collect();
    login::share_limits(&mut usage, twins);
    usage
}

/// Whether an account can take a session at all (an API key account needs
/// its key). Claude.ai logins are not checked here: that takes a second.
pub fn usable(store: &Store, id: &str) -> bool {
    match store.account(id).map(|a| a.kind) {
        Some(Kind::ApiKey) => secret::exists(id),
        // Signed in at least once: Claude Code wrote the account into the
        // folder's config. (Whether the login still works takes a second.)
        Some(Kind::Subscription) => {
            std::fs::read_to_string(mirror::home_of(id).join(".claude.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .is_some_and(|v| v["oauthAccount"].is_object())
        }
        Some(Kind::Main) => true,
        None => false,
    }
}

/// The real `claude` on `PATH` (the shell function is not on `PATH`, and
/// this binary never is called `claude`).
pub fn find_claude() -> Option<PathBuf> {
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("claude");
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        use std::os::unix::fs::PermissionsExt;
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
            continue;
        }
        if candidate.canonicalize().ok() == me {
            continue;
        }
        return Some(candidate);
    }
    // The installers' places, for shells whose PATH misses them.
    [".local/bin/claude", ".claude/local/claude"]
        .iter()
        .map(|p| paths::home().join(p))
        .find(|p| p.is_file())
}

/// Takes back what an outer start of this plugin set, so the account is
/// decided afresh.
pub fn clean_env(cmd: &mut Command) {
    if let Ok(set) = std::env::var(ENV_SET) {
        for var in set.split(',').filter(|v| !v.is_empty()) {
            cmd.env_remove(var);
        }
    }
    cmd.env_remove(ENV_SET);
}

/// Variables to set and variables to remove for a session.
pub type SessionEnv = (Vec<(String, String)>, Vec<&'static str>);

/// The environment for a session on `id`: (variable, value) to set, and
/// variables to remove.
pub fn session_env(store: &Store, id: &str) -> Result<SessionEnv, String> {
    let account = store.account(id).ok_or("unknown account")?;
    let mut set: Vec<(String, String)> = Vec::new();
    let mut remove: Vec<&'static str> = Vec::new();
    match account.kind {
        Kind::Main => {}
        Kind::Subscription => {
            let report =
                mirror::sync(id).map_err(|e| format!("could not prepare its folder: {e}"))?;
            for name in &report.separate {
                eprintln!(
                    "herdr-multiaccount: {name} in the {} folder is not linked to ~/.claude",
                    account.name
                );
            }
            // Without a login of its own, Claude Code would quietly use the
            // main login (or ask to sign in, where the browser may pick the
            // wrong account): stop with a clear message instead. When Claude
            // Code cannot be asked, start anyway.
            if let Some(text) = info::status_text(Some(&mirror::home_of(id))) {
                let i = info::parse_status(&text);
                if !i.signed_in || i.borrows_main {
                    return Err(format!(
                        "{} is not signed in, so this session would not run on it. Open the accounts list (Herdr: the Multiaccount action) and press l on {} to sign in.",
                        account.name, account.name
                    ));
                }
            }
            set.push((
                "CLAUDE_CONFIG_DIR".into(),
                mirror::home_of(id).to_string_lossy().to_string(),
            ));
            remove.extend(OVERRIDING);
        }
        Kind::ApiKey => {
            let key = secret::get(id).ok_or_else(|| {
                format!(
                    "no API key stored for {}. Open the accounts list and press l on it.",
                    account.name
                )
            })?;
            set.push(("ANTHROPIC_API_KEY".into(), key));
            remove.extend([
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CONFIG_DIR",
            ]);
        }
    }
    let names: Vec<&str> = set.iter().map(|(k, _)| k.as_str()).collect();
    set.push((ENV_SET.into(), names.join(",")));
    set.push((ENV_ID.into(), id.to_string()));
    Ok((set, remove))
}

/// The launcher's arguments for a session the list moved here (`run
/// --moved`): the start options and model from the move's hand-over, when
/// it is there for this conversation; otherwise as given, with a note.
fn moved_args(args: Vec<String>) -> Vec<String> {
    let pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|_| herdr::inside());
    let result = match &pane {
        Some(pane) => crate::sessions::take_hand_over(pane, &args),
        None => Err("it runs outside Herdr"),
    };
    match result {
        Ok(full) => full,
        Err(why) => {
            eprintln!(
                "herdr-multiaccount: the session's start options and model were not carried over ({why}); it starts without them."
            );
            args
        }
    }
}

pub fn run(args: Vec<String>, moved: bool) -> i32 {
    let args = if moved { moved_args(args) } else { args };
    let Some(claude) = find_claude() else {
        eprintln!("herdr-multiaccount: claude was not found on PATH");
        return 127;
    };
    let config = Config::load(&paths::config_dir());
    let store = Store::load();
    let usage = load_usage(&store);
    let (wanted, source) = wanted(&store);
    let check = |id: &str| usable(&store, id);
    let choice = choose(&store, &config, &usage, &check, wanted, source, time::now());

    if let Some(limit) = &choice.moved {
        eprintln!(
            "herdr-multiaccount: {} is at its {} limit ({:.0}%, resets in {}); this session uses {}.",
            store.name_of(&choice.wanted),
            limit.window,
            limit.used,
            time::span(limit.resets_at - time::now()),
            store.name_of(&choice.id)
        );
    }

    let mut cmd = Command::new(&claude);
    cmd.args(&args);
    clean_env(&mut cmd);
    // A named account is for this start only; sessions inside it inherit
    // the account through ENV_ID and may still fall back.
    cmd.env_remove(ENV_USE);
    match session_env(&store, &choice.id) {
        Ok((set, remove)) => {
            for v in remove {
                cmd.env_remove(v);
            }
            for (k, v) in set {
                cmd.env(k, v);
            }
        }
        Err(e) => {
            eprintln!("herdr-multiaccount: {e}");
            return 1;
        }
    }

    // Label the pane in Herdr's sidebar: only for the session the user
    // started there, not for ones Claude Code starts inside it.
    use std::io::IsTerminal;
    if herdr::inside() && choice.source != Source::Inherited && std::io::stdin().is_terminal() {
        if let Ok(pane) = std::env::var("HERDR_PANE_ID") {
            herdr::tag_pane(
                &pane,
                &store.name_of(&choice.id),
                &choice.id,
                std::process::id(),
            );
            // Herdr does not hand labels back, so the list reads this.
            let wanted = (choice.id != choice.wanted).then_some(choice.wanted.as_str());
            // What a move of this session carries over (`carry.rs`).
            let cwd = std::env::current_dir().ok();
            let carried = crate::carry::carried(&args, cwd.as_deref());
            crate::sessions::record(&pane, &choice.id, wanted, std::process::id(), &carried);
        }
    }

    use std::os::unix::process::CommandExt;
    let err = cmd.exec();
    eprintln!(
        "herdr-multiaccount: could not start {}: {err}",
        claude.display()
    );
    126
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Account;
    use serde_json::json;

    const NOW: i64 = 1_794_225_600;

    fn store() -> Store {
        let mut s = Store::default();
        for (id, fb) in [("a", true), ("b", false), ("c", true)] {
            s.accounts.push(Account {
                id: id.into(),
                name: id.to_uppercase(),
                kind: Kind::Subscription,
                sso: false,
                fallback: fb,
            });
        }
        s.active = "a".into();
        s
    }

    fn full(pct: f64) -> Usage {
        let mut u = Usage::default();
        u.record(
            &json!({"rate_limits": {"five_hour": {"used_percentage": pct, "resets_at": NOW + 600}}}),
            NOW,
        );
        u
    }

    #[test]
    fn stays_when_not_at_limit() {
        let s = store();
        let usage: HashMap<String, Usage> = [("a".to_string(), full(50.0))].into();
        let c = choose(
            &s,
            &Config::default(),
            &usage,
            &|_| true,
            "a".into(),
            Source::Active,
            NOW,
        );
        assert_eq!(c.id, "a");
        assert!(c.moved.is_none());
    }

    #[test]
    fn moves_to_next_skipping_excluded_and_full() {
        let s = store();
        // b is out of fallback, c is full: wraps around to main.
        let usage: HashMap<String, Usage> =
            [("a".to_string(), full(97.0)), ("c".to_string(), full(99.0))].into();
        let c = choose(
            &s,
            &Config::default(),
            &usage,
            &|_| true,
            "a".into(),
            Source::Active,
            NOW,
        );
        assert_eq!(c.id, "main");
        assert_eq!(c.moved.unwrap().window, "5-hour");
        // Once the window has reset, a is used again.
        let later = choose(
            &s,
            &Config::default(),
            &usage,
            &|_| true,
            "a".into(),
            Source::Active,
            NOW + 601,
        );
        assert_eq!(later.id, "a");
    }

    #[test]
    fn never_moves_to_the_same_login() {
        let s = store();
        // c has the same login as a and no reading of its own; a is full.
        let twins: HashMap<String, Vec<String>> = [
            ("a".to_string(), vec!["c".to_string()]),
            ("c".to_string(), vec!["a".to_string()]),
        ]
        .into();
        let mut usage: HashMap<String, Usage> = [("a".to_string(), full(97.0))].into();
        login::share_limits(&mut usage, &twins);
        let c = choose(
            &s,
            &Config::default(),
            &usage,
            &|_| true,
            "a".into(),
            Source::Active,
            NOW,
        );
        assert_eq!(c.id, "main", "c is the same full login, so it is skipped");
        // Wanting c itself also moves on: its twin's reading says full.
        let c = choose(
            &s,
            &Config::default(),
            &usage,
            &|_| true,
            "c".into(),
            Source::Active,
            NOW,
        );
        assert_eq!(c.id, "main");
    }

    #[test]
    fn explicit_and_off_never_move() {
        let s = store();
        let usage: HashMap<String, Usage> = [("a".to_string(), full(100.0))].into();
        let c = choose(
            &s,
            &Config::default(),
            &usage,
            &|_| true,
            "a".into(),
            Source::Explicit,
            NOW,
        );
        assert_eq!(c.id, "a");
        let off = Config {
            fallback: false,
            ..Config::default()
        };
        let c = choose(&s, &off, &usage, &|_| true, "a".into(), Source::Active, NOW);
        assert_eq!(c.id, "a");
    }

    #[test]
    fn stays_when_nothing_is_free() {
        let s = store();
        let usage: HashMap<String, Usage> = [("a".to_string(), full(100.0))].into();
        let c = choose(
            &s,
            &Config::default(),
            &usage,
            &|id| id == "a",
            "a".into(),
            Source::Active,
            NOW,
        );
        assert_eq!(c.id, "a");
        assert!(c.moved.is_none());
    }
}
