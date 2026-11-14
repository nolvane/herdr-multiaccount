//! `herdr-multiaccount run -- <claude args>`: what the `claude` shell function
//! calls. Picks the account (the space's, else the active one; the next one
//! in the list when that one is at its limit), prepares its folder, and
//! replaces itself with the real `claude`, so signals and exit codes are
//! Claude Code's own.

use crate::carry::Carried;
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
    /// The session left the wanted account, which is not signed in (also
    /// when a limit moved it): nothing to go back to.
    pub signed_out: bool,
}

/// What to do for a new session, once sign-ins are known.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Start on `choice.id`.
    Start(Choice),
    /// The wanted account is not signed in: ask whether to sign it in.
    Ask(Choice),
    /// The wanted account is not signed in and no other account may take
    /// the session.
    Stop(Choice),
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
        signed_out: false,
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

/// Picks the account for a new session, also when accounts are signed out.
/// `usable` is the cheap check (`usable`), `signed_in` the one that asks
/// Claude Code. A limit moves the session as `choose` does, never onto an
/// account that is not signed in. When the wanted account itself is not
/// signed in, the start asks to sign it in where `may_ask` (a terminal, not
/// a nested start); otherwise a named start or one with fallback off stops,
/// and every other start goes down the list to the next account that is
/// signed in, has room and takes part in fallback.
#[allow(clippy::too_many_arguments)]
pub fn pick(
    store: &Store,
    config: &Config,
    usage: &HashMap<String, Usage>,
    usable: &dyn Fn(&str) -> bool,
    signed_in: &dyn Fn(&str) -> bool,
    wanted: String,
    source: Source,
    now: i64,
    may_ask: bool,
) -> Pick {
    let takes = |id: &str| usable(id) && signed_in(id);
    let mut choice = choose(store, config, usage, &takes, wanted, source, now);
    if choice.id != choice.wanted {
        // Moved for a limit. When the wanted account is signed out as well,
        // the session gets no reminder to go back to it.
        choice.signed_out = !signed_in(&choice.wanted);
        return Pick::Start(choice);
    }
    if signed_in(&choice.wanted) {
        return Pick::Start(choice);
    }
    if may_ask {
        return Pick::Ask(choice);
    }
    if choice.source == Source::Explicit || !config.fallback {
        return Pick::Stop(choice);
    }
    match fallback_from(store, usage, &takes, &choice.wanted, config.threshold, now) {
        Some(next) => {
            choice.id = next;
            choice.moved = None;
            choice.signed_out = true;
            Pick::Start(choice)
        }
        None => Pick::Stop(choice),
    }
}

/// Whether a start may ask to sign in: only at a terminal, and never in a
/// start inside another Claude Code (headless runs, subagents), which nobody
/// watches.
pub fn may_ask(terminal: bool, source: &Source) -> bool {
    terminal && *source != Source::Inherited
}

/// The line when a session stops because `name` is not signed in: both ways
/// out, the list and the shell. While the main login is a Console profile
/// (`console`), the list cannot sign it in either, so only the shell way out.
pub fn signed_out_stop(name: &str, console: bool) -> String {
    if console {
        return format!(
            "{name} is not signed in, so this session cannot start on it. Start on the main login with: HERDR_MULTIACCOUNT_USE=main claude"
        );
    }
    format!(
        "{name} is not signed in, so this session cannot start on it. Sign it in with l in the accounts list (Herdr: the Multiaccount action), or start on the main login with: HERDR_MULTIACCOUNT_USE=main claude"
    )
}

/// The line when a session moved away from `from` because it is not signed
/// in.
pub fn signed_out_moved(from: &str, to: &str) -> String {
    format!(
        "{from} is not signed in; this session uses {to}. Sign {from} in with l in the accounts list (Herdr: the Multiaccount action)."
    )
}

/// Why `name` cannot be signed in from here: a claude.ai sign-in would sign
/// out a main login that is a Console profile.
pub fn console_note(name: &str) -> String {
    format!(
        "{name} can't be signed in here: the main login is a Claude Console sign-in without an API key, which signing in a claude.ai account would sign out. Sign the main login in with an API key first (claude auth login --console, then \"Create an API key\")."
    )
}

/// The account a session's note names as the one it should go back to: only
/// after a move for a limit, since a signed-out account has no room to go
/// back to.
pub fn note_wanted(choice: &Choice) -> Option<&str> {
    (choice.id != choice.wanted && !choice.signed_out).then_some(choice.wanted.as_str())
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
        Some(Kind::Subscription) => mirror::has_login(&mirror::folder(id)),
        Some(Kind::Main) => true,
        None => false,
    }
}

/// Whether an account's folder is signed in with a login of its own, so a
/// session runs on it. Without a login of its own, Claude Code would quietly
/// use the main login, or ask to sign in where the browser may pick the
/// wrong account. Asks Claude Code (up to a second or two) unless the list
/// found the folder signed in a few minutes ago. API key accounts and the
/// main login need no check here.
pub fn signed_in(store: &Store, id: &str) -> bool {
    signed_in_with(store, id, true)
}

/// [`signed_in`], with the list's check reused only when `reuse` is set.
fn signed_in_with(store: &Store, id: &str, reuse: bool) -> bool {
    match store.account(id).map(|a| a.kind) {
        Some(Kind::Subscription) => {
            let dir = mirror::folder(id);
            folder_signed_in(
                mirror::has_login(&dir),
                || reuse && info::fresh_check(&paths::checks_dir(), id, &dir, time::now()),
                || info::status_text(Some(&dir)),
            )
        }
        Some(_) => true,
        None => false,
    }
}

/// The sign-in check of a subscription's folder: it needs a login in its
/// config (`has_login`), then either the list's fresh "signed in" (`fresh`)
/// or Claude Code's own answer (`live`, asked only without a fresh one).
fn folder_signed_in(
    has_login: bool,
    fresh: impl FnOnce() -> bool,
    live: impl FnOnce() -> Option<String>,
) -> bool {
    has_login && (fresh() || info::takes_sessions(live().as_deref()))
}

/// Asks a yes/no question on the terminal; anything but yes is no. Reads
/// standard input byte by byte, unbuffered, so nothing typed after the
/// answer is held back from the Claude Code that starts next.
fn ask_yes(question: &str) -> bool {
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    eprint!("herdr-multiaccount: {question} [y/N] ");
    let _ = std::io::stderr().flush();
    // SAFETY: borrows file descriptor 0 without taking it over; ManuallyDrop
    // keeps it open afterwards.
    let mut input = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < 64 {
        match input.read(&mut byte) {
            Ok(1) if byte[0] != b'\n' => line.push(byte[0]),
            _ => break,
        }
    }
    is_yes(&String::from_utf8_lossy(&line))
}

/// Whether an answer to a [y/N] question is yes.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Claude Code's own sign-in for `id`, in the terminal (Claude Code says
/// who signed in). True when the folder is signed in afterwards; warns when
/// that is the same login as another account.
fn sign_in_here(store: &Store, id: &str) -> bool {
    let Some(account) = store.account(id) else {
        return false;
    };
    // No result from before this sign-in may stand in for the check after it.
    info::forget_check(&paths::checks_dir(), id);
    match crate::signin::in_terminal(account) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("herdr-multiaccount: {} was not signed in.", account.name);
            return false;
        }
        Err(e) => {
            eprintln!(
                "herdr-multiaccount: could not sign {} in: {e}",
                account.name
            );
            return false;
        }
    }
    // The same check as for any start, but always live: a list that is open
    // may have noted a result from before this sign-in meanwhile. No answer
    // from Claude Code does not undo a sign-in it just reported as done.
    if !signed_in_with(store, id, false) {
        eprintln!("herdr-multiaccount: {} was not signed in.", account.name);
        return false;
    }
    // From now on the account stays on its new folder.
    mirror::mark_moved(&mirror::home_of(id));
    let same = login::same_as(store, account);
    if !same.is_empty() {
        eprintln!("herdr-multiaccount: {}", login::same_login_note(&same));
    }
    true
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

/// The line a session on `dir` prints when that is the old folder `home`
/// an older version made: Claude asks before every edit in ~/.claude there.
pub fn start_note(dir: &std::path::Path, home: &std::path::Path, name: &str) -> Option<String> {
    (dir == home).then(|| format!(
        "{name} still uses the folder an older version made, so Claude asks before every edit in ~/.claude. Open the accounts list (Herdr: the Multiaccount action) and press l on {name} to sign in again."
    ))
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
            let (dir, report) =
                mirror::sync(id).map_err(|e| format!("could not prepare its folder: {e}"))?;
            for name in &report.separate {
                eprintln!(
                    "herdr-multiaccount: {name} in the {} folder is not linked to ~/.claude",
                    account.name
                );
            }
            if let Some(note) = start_note(&dir, &mirror::home_of(id), &account.name) {
                eprintln!("herdr-multiaccount: {note}");
            }
            set.push((
                "CLAUDE_CONFIG_DIR".into(),
                dir.to_string_lossy().to_string(),
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
/// Also what the session's note keeps of its start options: `None` when
/// they are not known (the hand-over was missing, or did not know them).
fn moved_args(args: Vec<String>) -> (Vec<String>, Option<Carried>) {
    let pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|_| herdr::inside());
    let result = match &pane {
        Some(pane) => crate::sessions::take_hand_over(pane, &args),
        None => Err("it runs outside Herdr"),
    };
    match result {
        Ok(h) => {
            // Its own `--resume` is no start option the user gave; what the
            // first start dropped is named again instead.
            let carried = h.dropped.map(|dropped| Carried {
                options: crate::carry::carried(&h.args, None).options,
                dropped,
            });
            (h.args, carried)
        }
        Err(why) => {
            eprintln!(
                "herdr-multiaccount: the session's start options and model were not carried over ({why}); it starts without them."
            );
            (args, None)
        }
    }
}

pub fn run(args: Vec<String>, moved: bool) -> i32 {
    let (args, carried) = if moved {
        moved_args(args)
    } else {
        // What a move of this session carries over (`carry.rs`).
        let cwd = std::env::current_dir().ok();
        let c = crate::carry::carried(&args, cwd.as_deref());
        (args, Some(c))
    };
    let Some(claude) = find_claude() else {
        eprintln!("herdr-multiaccount: claude was not found on PATH");
        return 127;
    };
    let config = Config::load(&paths::config_dir());
    let store = Store::load();
    let usage = load_usage(&store);
    let (wanted, source) = wanted(&store);
    let check = |id: &str| usable(&store, id);
    // Each account is asked about at most once per start.
    let known = std::cell::RefCell::new(HashMap::<String, bool>::new());
    let is_signed_in = |id: &str| {
        if let Some(&v) = known.borrow().get(id) {
            return v;
        }
        let v = signed_in(&store, id);
        known.borrow_mut().insert(id.to_string(), v);
        v
    };
    use std::io::IsTerminal;
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let may_ask = may_ask(terminal, &source);
    let now = time::now();
    let mut picked = pick(
        &store,
        &config,
        &usage,
        &check,
        &is_signed_in,
        wanted.clone(),
        source.clone(),
        now,
        may_ask,
    );
    // A claude.ai sign-in would sign out a main login that is a Console
    // profile; that check takes a second, so only after a yes.
    let mut console = false;
    if let Pick::Ask(c) = &picked {
        let name = store.name_of(&c.wanted);
        if ask_yes(&format!("{name} is not signed in. Sign in now?")) {
            if info::main_uses_profile() {
                console = true;
                eprintln!("herdr-multiaccount: {}", console_note(&name));
            } else if sign_in_here(&store, &c.wanted) {
                known.borrow_mut().insert(c.wanted.clone(), true);
            }
        }
        picked = pick(
            &store,
            &config,
            &usage,
            &check,
            &is_signed_in,
            wanted,
            source,
            now,
            false,
        );
    }
    let choice = match picked {
        Pick::Start(c) => c,
        Pick::Ask(c) | Pick::Stop(c) => {
            eprintln!(
                "herdr-multiaccount: {}",
                signed_out_stop(&store.name_of(&c.wanted), console)
            );
            return 1;
        }
    };

    if choice.signed_out && choice.moved.is_none() {
        eprintln!(
            "herdr-multiaccount: {}",
            signed_out_moved(&store.name_of(&choice.wanted), &store.name_of(&choice.id))
        );
    }
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
    if herdr::inside() && choice.source != Source::Inherited && std::io::stdin().is_terminal() {
        if let Ok(pane) = std::env::var("HERDR_PANE_ID") {
            herdr::tag_pane(
                &pane,
                &store.name_of(&choice.id),
                &choice.id,
                std::process::id(),
            );
            // Herdr does not hand labels back, so the list reads this.
            crate::sessions::record(
                &pane,
                &choice.id,
                note_wanted(&choice),
                std::process::id(),
                carried.as_ref(),
            );
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
    use crate::store::{Account, MAIN_ID};
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

    /// `pick` with fallback on, readings `usage`, and the accounts in
    /// `out` signed out.
    fn pick_with(
        config: &Config,
        usage: &HashMap<String, Usage>,
        out: &[&str],
        wanted: &str,
        source: Source,
        may_ask: bool,
    ) -> Pick {
        let s = store();
        pick(
            &s,
            config,
            usage,
            &|_| true,
            &|id| !out.contains(&id),
            wanted.into(),
            source,
            NOW,
            may_ask,
        )
    }

    fn started(p: Pick) -> Choice {
        match p {
            Pick::Start(c) => c,
            other => panic!("expected a start, got {other:?}"),
        }
    }

    #[test]
    fn signed_in_wanted_account_starts_as_before() {
        let c = started(pick_with(
            &Config::default(),
            &HashMap::new(),
            &[],
            "a",
            Source::Active,
            true,
        ));
        assert_eq!(c.id, "a");
        assert!(c.moved.is_none() && !c.signed_out);
    }

    #[test]
    fn signed_out_wanted_account_asks_where_it_may() {
        for source in [
            Source::Active,
            Source::Space("Work".into()),
            Source::Explicit,
        ] {
            let p = pick_with(
                &Config::default(),
                &HashMap::new(),
                &["a"],
                "a",
                source,
                true,
            );
            assert!(matches!(p, Pick::Ask(ref c) if c.wanted == "a"), "{p:?}");
        }
    }

    #[test]
    fn only_a_watched_terminal_asks() {
        assert!(may_ask(true, &Source::Active));
        assert!(may_ask(true, &Source::Explicit));
        assert!(may_ask(true, &Source::Space("Work".into())));
        // Nested starts and starts without a terminal never wait for an answer.
        assert!(!may_ask(true, &Source::Inherited));
        assert!(!may_ask(false, &Source::Active));
        assert!(!may_ask(false, &Source::Explicit));
    }

    #[test]
    fn signed_out_falls_back_to_the_next_signed_in_account() {
        let c = started(pick_with(
            &Config::default(),
            &HashMap::new(),
            &["a"],
            "a",
            Source::Active,
            false,
        ));
        // b is kept out of fallback; c is the next one and signed in.
        assert_eq!(c.id, "c");
        assert!(c.signed_out);
        assert!(c.moved.is_none());
        assert_eq!(
            note_wanted(&c),
            None,
            "no reminder to go back to a signed-out account"
        );
    }

    #[test]
    fn fallback_skips_signed_out_full_and_kept_out_accounts() {
        // c still has a login in its folder (usable) but is signed out.
        let c = started(pick_with(
            &Config::default(),
            &HashMap::new(),
            &["a", "c"],
            "a",
            Source::Active,
            false,
        ));
        assert_eq!(c.id, "main", "everything else is signed out or kept out");
        // c signed in but full: skipped too.
        let usage: HashMap<String, Usage> = [("c".to_string(), full(99.0))].into();
        let c = started(pick_with(
            &Config::default(),
            &usage,
            &["a"],
            "a",
            Source::Active,
            false,
        ));
        assert_eq!(c.id, "main");
        // Nested starts fall back the same way.
        let c = started(pick_with(
            &Config::default(),
            &HashMap::new(),
            &["a"],
            "a",
            Source::Inherited,
            false,
        ));
        assert_eq!(c.id, "c");
    }

    #[test]
    fn stops_when_no_account_may_take_it() {
        // Named starts never move, also after a "no".
        let p = pick_with(
            &Config::default(),
            &HashMap::new(),
            &["a"],
            "a",
            Source::Explicit,
            false,
        );
        assert!(matches!(p, Pick::Stop(ref c) if c.id == "a"), "{p:?}");
        // Fallback off.
        let off = Config {
            fallback: false,
            ..Config::default()
        };
        let p = pick_with(&off, &HashMap::new(), &["a"], "a", Source::Active, false);
        assert!(matches!(p, Pick::Stop(_)), "{p:?}");
        // Everyone else signed out (b is out of fallback anyway).
        let p = pick_with(
            &Config::default(),
            &HashMap::new(),
            &["a", "c", "main"],
            "a",
            Source::Active,
            false,
        );
        assert!(matches!(p, Pick::Stop(_)), "{p:?}");
    }

    #[test]
    fn a_limit_move_never_lands_on_a_signed_out_account() {
        let usage: HashMap<String, Usage> = [("a".to_string(), full(97.0))].into();
        // c is next but signed out: main takes it, for the limit.
        let c = started(pick_with(
            &Config::default(),
            &usage,
            &["c"],
            "a",
            Source::Active,
            true,
        ));
        assert_eq!(c.id, "main");
        assert_eq!(c.moved.as_ref().unwrap().window, "5-hour");
        assert!(!c.signed_out);
        assert_eq!(note_wanted(&c), Some("a"), "the reminder to go back stays");
        // A full account that is also signed out still moves for the limit,
        // without asking.
        let c = started(pick_with(
            &Config::default(),
            &usage,
            &["a"],
            "a",
            Source::Active,
            true,
        ));
        assert_eq!(c.id, "c");
        assert!(c.moved.is_some());
        assert_eq!(
            note_wanted(&c),
            None,
            "no reminder to go back to an account that is signed out too"
        );
    }

    #[test]
    fn a_fresh_check_by_the_list_spares_claude_codes_own() {
        let asked = std::cell::Cell::new(0);
        let live = |answer: &'static str| {
            let asked = &asked;
            move || {
                asked.set(asked.get() + 1);
                Some(answer.to_string())
            }
        };
        let signed_out = "Not logged in. Run claude auth login.\n";
        let signed_in = "Login method: Claude Max Account\nEmail: alex@example.com\n";
        // Fresh "signed in" from the list: Claude Code is not asked.
        assert!(folder_signed_in(true, || true, live(signed_out)));
        assert_eq!(asked.get(), 0);
        // None (old, missing, other folder, signed out): asked live.
        assert!(folder_signed_in(true, || false, live(signed_in)));
        assert!(!folder_signed_in(true, || false, live(signed_out)));
        assert_eq!(asked.get(), 2);
        // The folder's config lost its login (`/logout` in a session): no
        // reuse and no live check, it is signed out.
        assert!(!folder_signed_in(
            false,
            || panic!("no need to read the list's check"),
            live(signed_in)
        ));
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn main_and_api_key_accounts_are_not_checked() {
        let mut s = store();
        s.accounts.push(Account {
            id: "key".into(),
            name: "Key".into(),
            kind: Kind::ApiKey,
            sso: false,
            fallback: true,
        });
        assert!(signed_in(&s, MAIN_ID));
        assert!(signed_in(&s, "key"));
        assert!(!signed_in(&s, "nobody"));
    }

    #[test]
    fn only_yes_is_yes() {
        for yes in ["y", "Y", "yes", " YES \r"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "n", "no", "ja", "yy", "\u{4}"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn messages_name_the_ways_out() {
        let stop = signed_out_stop("Work", false);
        assert!(stop.contains("Work is not signed in"), "{stop}");
        assert!(stop.contains("l in the accounts list"), "{stop}");
        assert!(
            stop.contains("HERDR_MULTIACCOUNT_USE=main claude"),
            "{stop}"
        );
        // With a Console main login, the list cannot sign Work in either.
        let console_stop = signed_out_stop("Work", true);
        assert!(!console_stop.contains("accounts list"), "{console_stop}");
        assert!(
            console_stop.contains("HERDR_MULTIACCOUNT_USE=main claude"),
            "{console_stop}"
        );
        let moved = signed_out_moved("Work", "Personal");
        assert!(
            moved.starts_with("Work is not signed in; this session uses Personal."),
            "{moved}"
        );
        assert!(moved.contains("Sign Work in with l"), "{moved}");
        let console = console_note("Work");
        assert!(
            console.contains("Console sign-in without an API key"),
            "{console}"
        );
        assert!(console.contains("claude auth login --console"), "{console}");
    }

    #[test]
    fn a_session_on_the_old_folder_says_to_sign_in_again() {
        let home = std::path::Path::new("/x/homes/work");
        let note = start_note(home, home, "Work").unwrap();
        assert!(note.contains("press l on Work to sign in again"), "{note}");
        assert!(
            note.contains("asks before every edit in ~/.claude"),
            "{note}"
        );
        assert_eq!(start_note(&home.join(".claude"), home, "Work"), None);
    }
}
