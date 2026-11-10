//! Moving a running Claude Code session to another account, from the list's
//! Agents tab (`m`). A running session cannot change its login, so the move
//! ends Claude Code in its pane and starts the same conversation again
//! there, through the launcher, on the other account:
//!
//! 1. Ctrl+C until Claude Code has ended: the first one clears text typed
//!    but not sent, then two in a row end it (the first of them only shows
//!    "Press Ctrl-C again to exit"). Nothing is typed into Claude Code, so
//!    unsent text can never go out as a message. Before each press Herdr is
//!    asked again whether the session started working; then it stops.
//! 2. Wait until it has left the pane and the pane's shell is back in front.
//! 3. Type `HERDR_MULTIACCOUNT_USE=<account> "<binary>" run -- --resume <id>`
//!    into the shell. The launcher labels the pane and notes the account.
//!    When the session has start options or a model to carry over
//!    (`carry.rs`), the list writes them to a one-time hand-over file before
//!    step 1, and types `run --moved -- --resume <id>`: the launcher adds
//!    them, so no option value is ever typed into the shell.
//! 4. Wait until Herdr sees Claude Code in the pane again.
//!
//! The list checks before that the session is idle and which conversation
//! it holds (`sessions::Conversation`, noted by the status line).

use crate::carry::Moved;
use crate::herdr::{self, Processes};
use crate::launch;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One move, as the list decided it.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub pane: String,
    /// The session's name in the list.
    pub label: String,
    pub session_id: String,
    /// The Claude Code process that holds the conversation now.
    pub pid: u32,
    pub account_id: String,
    pub account_name: String,
    /// The start options and model it carries over.
    pub carry: Moved,
}

/// What the move needs from Herdr; a fake in tests.
pub trait Io {
    fn processes(&self, pane: &str) -> Option<Processes>;
    /// Herdr's agent status for the pane (`idle`, `working`, ...).
    fn status(&self, pane: &str) -> Option<String>;
    fn send_keys(&self, pane: &str, keys: &[&str]) -> bool;
    fn run(&self, pane: &str, command: &str) -> bool;
    fn agent(&self, pane: &str) -> Option<String>;
    fn sleep(&self, d: Duration);
    /// Writes the hand-over for the launcher (`sessions::write_hand_over`).
    fn hand_over(&self, pane: &str, session_id: &str, carry: &Moved) -> bool;
    fn drop_hand_over(&self, pane: &str);
    /// Opens the popup of a failed move (`move-pane`) over whatever is in
    /// front; false when Herdr could not.
    fn popup(&self, pane: &str, body: &str) -> bool;
    fn notify(&self, title: &str, body: &str);
}

struct Herdr;

impl Io for Herdr {
    fn processes(&self, pane: &str) -> Option<Processes> {
        herdr::process_info(pane)
    }
    fn status(&self, pane: &str) -> Option<String> {
        herdr::pane_status(pane)
    }
    fn send_keys(&self, pane: &str, keys: &[&str]) -> bool {
        herdr::send_keys(pane, keys)
    }
    fn run(&self, pane: &str, command: &str) -> bool {
        herdr::run_in_pane(pane, command)
    }
    fn agent(&self, pane: &str) -> Option<String> {
        herdr::pane_agent(pane)
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn hand_over(&self, pane: &str, session_id: &str, carry: &Moved) -> bool {
        let dropped = carry.options.as_ref().map(|_| carry.dropped.as_slice());
        crate::sessions::write_hand_over(pane, session_id, &carry.args(), dropped)
    }
    fn drop_hand_over(&self, pane: &str) {
        crate::sessions::drop_hand_over(pane);
    }
    fn popup(&self, pane: &str, body: &str) -> bool {
        let width = crate::remind::WIDTH;
        let lines = crate::ui::wrap(body, (width - 4) as usize).len() as u16;
        herdr::open_popup("moved", width, lines + 6, &[(ENV_MOVED, pane)])
    }
    fn notify(&self, title: &str, body: &str) {
        herdr::notify(title, body);
    }
}

/// Ctrl+C presses at most (clear, the "again" hint, the end, one spare),
/// and the pause between them.
const PRESSES: usize = 4;
const PRESS_GAP: Duration = Duration::from_millis(300);
/// How long Claude Code may take to end, and to start again.
const END_WAIT: Duration = Duration::from_secs(8);
const START_WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(250);

/// Account ids are letters, digits, `-` and `_` (`Store::new_id`); checked
/// again, as the id goes into a shell command.
fn safe_account_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The command that starts the conversation again on the account. `bin` is
/// the plugin's binary as the `claude` shell function names it (inside
/// double quotes, `$HOME` allowed), so the move also works in a shell
/// started before setup. `moved`: the launcher takes the start options from
/// the hand-over.
pub fn command(
    account_id: &str,
    session_id: &str,
    bin: &str,
    moved: bool,
) -> Result<String, String> {
    if !safe_account_id(account_id) {
        return Err("the account id has characters a shell would misread".into());
    }
    if !crate::sessions::valid_session_id(session_id) {
        return Err("the conversation id is not one of Claude Code's".into());
    }
    let rest = bin.strip_prefix("$HOME/").unwrap_or(bin);
    if bin.is_empty() || crate::setup::quotable(std::path::Path::new(rest)).is_err() {
        return Err("the plugin's path has characters a shell would misread".into());
    }
    let flag = if moved { " --moved" } else { "" };
    Ok(format!(
        "{}={account_id} \"{bin}\" run{flag} -- --resume {session_id}",
        launch::ENV_USE
    ))
}

/// Starting the conversation by hand, once Claude Code has ended: with the
/// model, and naming the start options to add.
fn resume_by_hand(account_id: &str, session_id: &str, carry: &Moved) -> String {
    let (model, names) = carry.by_hand();
    let mut text = format!(
        "{}={account_id} claude --resume {session_id}{model}",
        launch::ENV_USE
    );
    if !names.is_empty() {
        text.push_str(&format!(
            ", adding the options it was started with ({})",
            names.join(", ")
        ));
    }
    text
}

/// The same move by hand, for the error messages.
pub fn manual(account_id: &str, session_id: &str, carry: &Moved) -> String {
    format!(
        "type /exit in it, then {}",
        resume_by_hand(account_id, session_id, carry)
    )
}

/// What the pane shows of the old Claude Code (`pid`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Front {
    /// Still in front.
    Running,
    /// Gone, and the pane's own shell is in front.
    Ended,
    /// Gone, but something else is in front (a nested shell, a program).
    Other,
    /// Herdr could not tell.
    Unknown,
}

fn front(io: &dyn Io, pane: &str, pid: u32) -> Front {
    match io.processes(pane) {
        None => Front::Unknown,
        Some(p) if p.front.iter().any(|(f, _)| *f == pid) => Front::Running,
        Some(p) if p.shell_in_front() => Front::Ended,
        Some(_) => Front::Other,
    }
}

/// Whether a process name is the plugin's own launcher (`run`), which is in
/// front for a moment before it hands over to Claude Code. macOS cuts
/// process names to 16 characters.
fn is_launcher(name: &str) -> bool {
    name.rsplit('/')
        .next()
        .is_some_and(|b| b.starts_with("herdr-multiacc"))
}

/// Whether a Claude Code other than the old one runs in the pane: not the
/// shell, not the launcher on its way, and Herdr sees an agent.
fn started(io: &dyn Io, pane: &str, old: u32) -> bool {
    let Some(p) = io.processes(pane) else {
        return false;
    };
    let fresh = !p.shell_in_front()
        && !p.front.is_empty()
        && p.front
            .iter()
            .all(|(f, name)| *f != old && !is_launcher(name));
    fresh && io.agent(pane).as_deref() == Some("claude")
}

/// Waits up to `limit` for `done`, counting the pauses and the clock, so
/// slow Herdr calls cannot stretch the wait.
fn wait(io: &dyn Io, limit: Duration, done: &dyn Fn() -> bool) -> bool {
    let start = Instant::now();
    let mut waited = Duration::ZERO;
    loop {
        if done() {
            return true;
        }
        if waited >= limit || start.elapsed() >= limit {
            return false;
        }
        io.sleep(POLL);
        waited += POLL;
    }
}

/// Runs a move: Ok with what to tell, or Err naming the way by hand.
pub fn run_move(io: &dyn Io, plan: &Plan, bin: &str) -> Result<String, String> {
    let by_hand = manual(&plan.account_id, &plan.session_id, &plan.carry);
    let resume = resume_by_hand(&plan.account_id, &plan.session_id, &plan.carry);
    // Unknown start options go through a hand-over too, so the moved
    // session's note keeps them unknown.
    let moved = plan.carry.options.is_none() || !plan.carry.is_empty();
    let cmd = command(&plan.account_id, &plan.session_id, bin, moved)
        .map_err(|e| format!("Nothing was changed: {e}. By hand: {by_hand}."))?;
    // Written before anything happens to the session, so a failure here
    // changes nothing.
    if moved && !io.hand_over(&plan.pane, &plan.session_id, &plan.carry) {
        return Err(format!(
            "Nothing was changed: its start options could not be saved for the move. By hand: {by_hand}."
        ));
    }
    // Until the command is typed, a stop takes the hand-over back; after
    // that the launcher may still read it, and it expires by itself.
    let ended = end(io, plan, &by_hand, &resume);
    let typed = ended.and_then(|()| {
        if io.run(&plan.pane, &cmd) {
            Ok(())
        } else {
            Err(format!(
                "Claude Code ended, but Herdr could not start it again. By hand: {resume}."
            ))
        }
    });
    if let Err(e) = typed {
        if moved {
            io.drop_hand_over(&plan.pane);
        }
        return Err(e);
    }
    if !wait(io, START_WAIT, &|| started(io, &plan.pane, plan.pid)) {
        return Err(format!(
            "Claude Code did not start again in it; look at the pane for why. By hand: {resume}."
        ));
    }
    Ok(format!("Moved {} to {}", plan.label, plan.account_name))
}

/// Ends Claude Code in the pane (steps 1 and 2), or says why not.
fn end(io: &dyn Io, plan: &Plan, by_hand: &str, resume: &str) -> Result<(), String> {
    for n in 0..PRESSES {
        match front(io, &plan.pane, plan.pid) {
            Front::Ended => break,
            Front::Running => {
                // A turn that started since the list asked is never cut off.
                if matches!(
                    io.status(&plan.pane).as_deref(),
                    Some("working" | "blocked")
                ) {
                    return Err(if n == 0 {
                        "It started working, so nothing was changed.".to_string()
                    } else {
                        format!(
                            "It started working while the list pressed Ctrl+C, so it stopped there and started nothing. By hand: {by_hand}."
                        )
                    });
                }
            }
            Front::Other | Front::Unknown => {}
        }
        if !io.send_keys(&plan.pane, &["ctrl+c"]) {
            return Err(format!(
                "Herdr could not press Ctrl+C in it, so nothing was started. By hand: {by_hand}."
            ));
        }
        io.sleep(PRESS_GAP);
    }
    if !wait(io, END_WAIT, &|| {
        front(io, &plan.pane, plan.pid) == Front::Ended
    }) {
        return Err(match front(io, &plan.pane, plan.pid) {
            Front::Running | Front::Ended => format!(
                "Claude Code did not end in it, so nothing was started. By hand: {by_hand}."
            ),
            Front::Other => format!(
                "Claude Code ended, but the pane's own shell is not in front (a nested shell or another program is), so nothing was started. Start it again by hand: {resume}."
            ),
            Front::Unknown => format!(
                "Herdr could not tell whether Claude Code ended, so nothing was started. Look at the pane; by hand: {by_hand}."
            ),
        });
    }
    Ok(())
}

// ---- the move in a process of its own --------------------------------
//
// Herdr ends every process of a plugin pane's session when the pane closes
// (also a process group of its own), so a move run by the list would stop
// halfway when the list closes. The list writes the plan to a private note
// and starts `herdr-multiaccount move <pane>` in a session of its own; that
// process runs the move and writes the result into the note. A list takes
// the result from there (any list, also one opened later); when none does,
// the move process reports a failure itself (popup, else a notification).

/// The notes of moves, `<config>/moving/<pane>.json`, only their owner can
/// read: the plan holds the start option values. Not #9's hand-over
/// (`moves/`), which the launcher takes.
pub fn moving_dir() -> PathBuf {
    crate::paths::config_dir().join("moving")
}

/// A move as noted for the move process and the lists.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub plan: Plan,
    /// When the list wrote it (unix seconds).
    pub at: i64,
    /// The move process, once it runs.
    pub mover: Option<u32>,
    pub result: Option<Result<String, String>>,
}

/// Where a move stands, for a list.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Running,
    Done(Result<String, String>),
    /// Its process is gone without a result (killed, a restart).
    Stopped,
}

/// How long the move process may take to note itself, and how long a move
/// runs at most (presses, both waits and the hand-over to a list or popup
/// are well under it), so a reused pid never holds a move open.
const START_GRACE: i64 = 10;
const MOVE_AT_MOST: i64 = 120;
/// How long the move process leaves a result to a list, and a failure to
/// its popup.
const LIST_WAIT: Duration = Duration::from_secs(3);
const POPUP_WAIT: Duration = Duration::from_secs(10);

fn note_file(dir: &Path, pane: &str) -> PathBuf {
    dir.join(format!("{}.json", crate::sessions::safe_name(pane)))
}

/// The failure the move process hands to its popup: title and text only.
fn report_file(dir: &Path, pane: &str) -> PathBuf {
    dir.join(format!("{}.report", crate::sessions::safe_name(pane)))
}

fn note_json(n: &Note) -> String {
    let p = &n.plan;
    let mut v = json!({
        "pane": p.pane,
        "label": p.label,
        "session_id": p.session_id,
        "pid": p.pid,
        "account_id": p.account_id,
        "account_name": p.account_name,
        "options": p.carry.options,
        "dropped": p.carry.dropped,
        "model": p.carry.model,
        "at": n.at,
    });
    if let Some(m) = n.mover {
        v["mover"] = json!(m);
    }
    match &n.result {
        Some(Ok(t)) => v["ok"] = json!(t),
        Some(Err(t)) => v["err"] = json!(t),
        None => {}
    }
    format!("{v}\n")
}

fn note_from(v: &Value) -> Option<Note> {
    let s = |k: &str| v[k].as_str().map(str::to_string);
    let strings = |k: &str| -> Option<Vec<String>> {
        v[k].as_array()?
            .iter()
            .map(|x| x.as_str().map(str::to_string))
            .collect()
    };
    let pid = |k: &str| v[k].as_u64().and_then(|p| u32::try_from(p).ok());
    Some(Note {
        plan: Plan {
            pane: s("pane")?,
            label: s("label")?,
            session_id: s("session_id")?,
            pid: pid("pid")?,
            account_id: s("account_id")?,
            account_name: s("account_name")?,
            carry: Moved {
                options: strings("options"),
                dropped: strings("dropped").unwrap_or_default(),
                model: s("model"),
            },
        },
        at: v["at"].as_i64()?,
        mover: pid("mover"),
        result: s("ok").map(Ok).or_else(|| s("err").map(Err)),
    })
}

fn read_note_at(path: &Path) -> Option<Note> {
    note_from(&serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?)
}

pub fn read_note(dir: &Path, pane: &str) -> Option<Note> {
    read_note_at(&note_file(dir, pane))
}

/// Writes (or replaces) the note of a move.
pub fn write_note(dir: &Path, n: &Note) -> bool {
    crate::paths::private_dir(dir).is_ok()
        && crate::paths::write_private(&note_file(dir, &n.plan.pane), note_json(n).as_bytes())
            .is_ok()
}

/// Writes a new note, never over one that is there: one move per pane.
fn create_note(dir: &Path, n: &Note) -> bool {
    if crate::paths::private_dir(dir).is_err() {
        return false;
    }
    let safe = crate::sessions::safe_name(&n.plan.pane);
    let tmp = dir.join(format!("{safe}.new.{}", std::process::id()));
    if crate::paths::write_private(&tmp, note_json(n).as_bytes()).is_err() {
        return false;
    }
    // A link fails when the name is taken, unlike a rename.
    let made = std::fs::hard_link(&tmp, note_file(dir, &n.plan.pane)).is_ok();
    let _ = std::fs::remove_file(&tmp);
    made
}

/// Every move noted now.
pub fn notes(dir: &Path) -> Vec<Note> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| read_note_at(&p))
        .collect()
}

pub fn state(n: &Note, now: i64, alive: &dyn Fn(u32) -> bool) -> State {
    if let Some(r) = &n.result {
        return State::Done(r.clone());
    }
    let age = now - n.at;
    let running = age <= MOVE_AT_MOST
        && match n.mover {
            Some(pid) => alive(pid),
            None => age <= START_GRACE,
        };
    if running {
        State::Running
    } else {
        State::Stopped
    }
}

/// Takes a finished note away: true only for the one that removed it, so a
/// result is told once.
pub fn take(dir: &Path, pane: &str) -> bool {
    std::fs::remove_file(note_file(dir, pane)).is_ok()
}

/// Takes a note a list will show in a dialog, keeping it (as
/// `<pane>.held.<list pid>`) until the dialog is on screen, so a list closed
/// in between leaves it to the next one. `None` when another took it.
pub fn hold(dir: &Path, pane: &str, holder: u32) -> Option<PathBuf> {
    let held = dir.join(format!(
        "{}.held.{holder}",
        crate::sessions::safe_name(pane)
    ));
    std::fs::rename(note_file(dir, pane), &held).ok()?;
    Some(held)
}

/// Gives the notes held by lists that are gone back to the next list.
pub fn recover_held(dir: &Path, alive: &dyn Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((stem, holder)) = name.split_once(".held.") else {
            continue;
        };
        let Ok(holder) = holder.parse::<u32>() else {
            continue;
        };
        if alive(holder) {
            continue;
        }
        let back = dir.join(format!("{stem}.json"));
        if back.exists() {
            // A newer move of that pane: the old result is stale.
            let _ = std::fs::remove_file(&path);
        } else {
            let _ = std::fs::rename(&path, back);
        }
    }
}

/// What a list says of a move whose process is gone without a result.
pub fn stopped_text(plan: &Plan) -> (String, String) {
    (
        format!("The move of {} stopped before it finished", plan.label),
        format!(
            "Look at the pane: if Claude Code has ended there, start it again by hand: {}.",
            resume_by_hand(&plan.account_id, &plan.session_id, &plan.carry)
        ),
    )
}

/// The move process's command line: the pane only, never the plan, since
/// other users can read command lines and options can hold private text.
pub fn move_args(pane: &str) -> Vec<String> {
    vec!["move".into(), pane.into()]
}

/// Notes the move and starts its process (`spawn`). Err (nothing pressed)
/// when either fails.
pub fn start(
    dir: &Path,
    plan: Plan,
    now: i64,
    spawn: &dyn Fn(&[String]) -> bool,
) -> Result<(), String> {
    let by_hand = manual(&plan.account_id, &plan.session_id, &plan.carry);
    let pane = plan.pane.clone();
    let note = Note {
        plan,
        at: now,
        mover: None,
        result: None,
    };
    if !create_note(dir, &note) {
        return Err(format!(
            "Nothing was changed: the move could not be noted, or another move of it still runs. By hand: {by_hand}."
        ));
    }
    if !spawn(&move_args(&pane)) {
        let _ = std::fs::remove_file(note_file(dir, &pane));
        return Err(format!(
            "Nothing was changed: the move could not be started. By hand: {by_hand}."
        ));
    }
    Ok(())
}

/// Starts `program` with `args` in a session of its own, without input or
/// output, so closing the list's pane does not end it. A thread waits for
/// it, so a move process that was killed leaves no zombie that would look
/// alive to the list.
pub fn spawn_detached(program: &Path, args: &[String]) -> bool {
    use std::os::unix::process::CommandExt;
    extern "C" {
        fn setsid() -> i32;
    }
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: runs in the child between fork and exec; `setsid` is
    // async-signal-safe and touches no memory. `CommandExt::setsid` is not
    // stable yet, and a process group alone does not survive Herdr's close.
    unsafe {
        cmd.pre_exec(|| {
            if setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// The list's `Mover`: notes the move and starts its process.
pub fn perform(dir: &Path, plan: Plan) -> Result<(), String> {
    let exe = std::env::current_exe().ok();
    start(dir, plan, crate::time::now(), &|args| {
        exe.as_deref().is_some_and(|e| spawn_detached(e, args))
    })
}

/// The move process (`move <pane>`): runs the noted move once, writes the
/// result, leaves it to a list for a moment, and reports a failure itself
/// when no list took it. A success with no list says nothing (the owner's
/// decision on #12): the pane's label shows the new account.
pub fn run_noted(io: &dyn Io, dir: &Path, pane: &str, bin: &str, me: u32) {
    let Some(mut note) = read_note(dir, pane) else {
        return;
    };
    if note.plan.pane != pane || note.mover.is_some() || note.result.is_some() {
        return;
    }
    note.mover = Some(me);
    if !write_note(dir, &note) {
        return;
    }
    let result = run_move(io, &note.plan, bin);
    note.result = Some(result.clone());
    let file = note_file(dir, pane);
    let written = write_note(dir, &note);
    if written && wait(io, LIST_WAIT, &|| !file.exists()) {
        return;
    }
    // Taken by a list just now: it tells.
    if !take(dir, pane) && written {
        return;
    }
    let Err(body) = result else {
        return;
    };
    let title = format!("{} was not moved", note.plan.label);
    let report = report_file(dir, pane);
    let handed = crate::paths::write_private(
        &report,
        format!("{}\n", json!({"title": title, "body": body})).as_bytes(),
    )
    .is_ok();
    if handed && io.popup(pane, &body) && wait(io, POPUP_WAIT, &|| !report.exists()) {
        return;
    }
    // The popup did not take it: a notification, unless it did just now.
    if !handed || std::fs::remove_file(&report).is_ok() {
        io.notify(&title, &body);
    }
}

/// The popup's text, taken once: `None` when it is gone (the move process
/// sent a notification instead).
pub fn take_report(dir: &Path, pane: &str) -> Option<(String, String)> {
    let path = report_file(dir, pane);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    std::fs::remove_file(&path).ok()?;
    Some((
        v["title"].as_str()?.to_string(),
        v["body"].as_str()?.to_string(),
    ))
}

/// `herdr-multiaccount move <pane>`.
pub fn run_from_note(pane: &str) -> i32 {
    run_noted(
        &Herdr,
        &moving_dir(),
        pane,
        &crate::setup::bin_for_shell(),
        std::process::id(),
    );
    0
}

/// Where the popup finds the pane whose move failed.
const ENV_MOVED: &str = "HERDR_MULTIACCOUNT_MOVED";

/// The popup of a failed move (`move-pane`): stays until it is answered;
/// "Go to session" brings the session's tab to the front.
pub fn moved_pane() -> i32 {
    let pane = std::env::var(ENV_MOVED).unwrap_or_default();
    let Some((title, body)) = take_report(&moving_dir(), &pane) else {
        return 0;
    };
    let mut terminal = crate::enter_terminal();
    let go = crate::remind::dialog(&mut terminal, &title, &body, " Esc close ").unwrap_or(false);
    crate::leave_terminal();
    if go {
        if let Some(tab) = herdr::pane_tab(&pane) {
            herdr::focus_tab(&tab);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const ID: &str = "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f6";
    const BIN: &str = "$HOME/.config/herdr/plugins/config/multiaccount/bin/herdr-multiaccount";

    /// A pane whose Claude Code (pid 400) ends after `ends_after` Ctrl+C
    /// presses (never when `None`), and whose new one (pid 500) shows up
    /// when `starts` after the command.
    struct Fake {
        ends_after: Option<usize>,
        starts: bool,
        /// What is in front once the old one ended (the pane's shell).
        after_end: Vec<(u32, String)>,
        /// Herdr cannot be asked once the old one ended.
        blind_after_end: bool,
        /// After the command only the launcher is in front, and Herdr still
        /// says `claude` from before.
        launcher_stays: bool,
        /// Herdr says `working` from this many presses on.
        busy_after: Option<usize>,
        presses: RefCell<usize>,
        /// The hand-over as written, and whether it was taken back.
        handed: RefCell<Option<(String, Vec<String>)>>,
        dropped: RefCell<bool>,
        /// Writing the hand-over fails.
        no_hand_over: bool,
        ran: RefCell<Vec<String>>,
        log: RefCell<Vec<String>>,
        slept: RefCell<Duration>,
        /// The notes' folder, for a list that takes the result while the
        /// move process waits (`list_takes`) and for the popup.
        dir: Option<PathBuf>,
        list_takes: bool,
        popup_opens: bool,
        /// The popup takes the report (it started).
        popup_takes: bool,
        /// Each popup: its pane, and the title and text it took.
        popups: RefCell<Vec<(String, Option<Report>)>>,
        notified: RefCell<Vec<(String, String)>>,
    }

    impl Fake {
        fn new(ends_after: Option<usize>, starts: bool) -> Fake {
            Fake {
                ends_after,
                starts,
                after_end: vec![(300, "-zsh".to_string())],
                blind_after_end: false,
                launcher_stays: false,
                busy_after: None,
                presses: RefCell::new(0),
                handed: RefCell::new(None),
                dropped: RefCell::new(false),
                no_hand_over: false,
                ran: RefCell::new(vec![]),
                log: RefCell::new(vec![]),
                slept: RefCell::new(Duration::ZERO),
                dir: None,
                list_takes: false,
                popup_opens: true,
                popup_takes: true,
                popups: RefCell::new(vec![]),
                notified: RefCell::new(vec![]),
            }
        }
        fn old_running(&self) -> bool {
            self.ends_after.is_none_or(|n| *self.presses.borrow() < n)
        }
    }

    impl Io for Fake {
        fn processes(&self, _: &str) -> Option<Processes> {
            let ran = !self.ran.borrow().is_empty();
            let front = if self.old_running() {
                vec![(400, "2.1.30".to_string())]
            } else if self.blind_after_end {
                return None;
            } else if ran && self.launcher_stays {
                vec![(500, "herdr-multiaccou".to_string())]
            } else if ran && self.starts {
                vec![(500, "2.1.30".to_string())]
            } else {
                self.after_end.clone()
            };
            Some(Processes {
                shell: Some(300),
                group: None,
                front,
            })
        }
        fn status(&self, _: &str) -> Option<String> {
            let busy = self.busy_after.is_some_and(|n| *self.presses.borrow() >= n);
            Some(if busy { "working" } else { "idle" }.to_string())
        }
        fn send_keys(&self, _: &str, keys: &[&str]) -> bool {
            self.log
                .borrow_mut()
                .push(format!("keys {}", keys.join(" ")));
            *self.presses.borrow_mut() += 1;
            true
        }
        fn run(&self, _: &str, command: &str) -> bool {
            assert!(!self.old_running(), "typed while Claude Code still ran");
            self.log.borrow_mut().push(format!("run {command}"));
            self.ran.borrow_mut().push(command.to_string());
            true
        }
        fn agent(&self, _: &str) -> Option<String> {
            let ran = !self.ran.borrow().is_empty();
            ((self.starts && ran) || self.launcher_stays).then(|| "claude".to_string())
        }
        fn sleep(&self, d: Duration) {
            *self.slept.borrow_mut() += d;
            if let (true, Some(dir)) = (self.list_takes, &self.dir) {
                if read_note(dir, "w91:p1").is_some_and(|n| n.result.is_some()) {
                    assert!(take(dir, "w91:p1"));
                }
            }
        }
        fn hand_over(&self, _: &str, session_id: &str, carry: &Moved) -> bool {
            assert_eq!(*self.presses.borrow(), 0, "written before any Ctrl+C");
            self.log.borrow_mut().push("hand over".into());
            *self.handed.borrow_mut() = Some((session_id.to_string(), carry.args()));
            !self.no_hand_over
        }
        fn drop_hand_over(&self, _: &str) {
            *self.dropped.borrow_mut() = true;
        }
        fn popup(&self, pane: &str, _: &str) -> bool {
            let taken = (self.popup_opens && self.popup_takes)
                .then(|| take_report(self.dir.as_ref().unwrap(), pane))
                .flatten();
            self.popups.borrow_mut().push((pane.to_string(), taken));
            self.popup_opens
        }
        fn notify(&self, title: &str, body: &str) {
            self.notified
                .borrow_mut()
                .push((title.to_string(), body.to_string()));
        }
    }

    type Report = (String, String);

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hm-relocate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A list noted this move and started the process.
    fn listed(dir: &Path, p: Plan) {
        assert!(start(dir, p, crate::time::now(), &|_| true).is_ok());
    }

    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_note_is_private_and_round_trips() {
        let dir = scratch("note");
        listed(&dir, carrying());
        let file = note_file(&dir, "w91:p1");
        assert_eq!(file, dir.join("w91_p1.json"));
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&dir), 0o700);
        let n = read_note(&dir, "w91:p1").unwrap();
        assert_eq!(n.plan, carrying());
        assert_eq!((n.mover, n.result.clone()), (None, None));
        let done = Note {
            mover: Some(77),
            result: Some(Err("x".into())),
            ..n.clone()
        };
        assert!(write_note(&dir, &done));
        assert_eq!(read_note(&dir, "w91:p1"), Some(done));
        assert_eq!(mode(&file), 0o600);
        // Unknown start options stay unknown.
        let mut p = plan();
        p.pane = "w91:p2".into();
        p.carry = Moved::default();
        listed(&dir, p.clone());
        assert_eq!(read_note(&dir, "w91:p2").unwrap().plan, p);
        // One move per pane.
        assert!(start(&dir, plan(), 0, &|_| true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_pane_goes_on_the_command_line() {
        let dir = scratch("args");
        let seen = RefCell::new(vec![]);
        let p = carrying();
        assert!(start(&dir, p.clone(), 0, &|a| {
            *seen.borrow_mut() = a.to_vec();
            true
        })
        .is_ok());
        assert_eq!(
            *seen.borrow(),
            vec!["move".to_string(), "w91:p1".to_string()]
        );
        let line = seen.borrow().join(" ");
        for o in p.carry.args() {
            assert!(!line.contains(&o), "{o}");
        }
        assert!(!line.contains(ID) && !line.contains("work"), "{line}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_spawn_leaves_no_note_and_presses_nothing() {
        let dir = scratch("spawn");
        let e = start(&dir, plan(), 0, &|_| false).unwrap_err();
        assert!(e.starts_with("Nothing was changed"), "{e}");
        assert!(e.contains("/exit"), "{e}");
        assert!(notes(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_move_process_finishes_without_a_list_and_a_success_says_nothing() {
        let dir = scratch("alone");
        listed(&dir, plan());
        let mut io = Fake::new(Some(2), true);
        io.dir = Some(dir.clone());
        run_noted(&io, &dir, "w91:p1", BIN, 4711);
        assert_eq!(io.ran.borrow().len(), 1, "the session started again");
        assert!(io.popups.borrow().is_empty());
        assert!(io.notified.borrow().is_empty());
        assert!(notes(&dir).is_empty(), "nothing left");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        // Runs once: a second start finds no note.
        run_noted(&io, &dir, "w91:p1", BIN, 4712);
        assert_eq!(io.ran.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_running_move_is_never_started_twice() {
        let dir = scratch("twice");
        listed(&dir, plan());
        let mut n = read_note(&dir, "w91:p1").unwrap();
        n.mover = Some(1);
        assert!(write_note(&dir, &n));
        let io = Fake::new(Some(2), true);
        run_noted(&io, &dir, "w91:p1", BIN, 4711);
        assert!(io.log.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_with_no_list_opens_the_popup() {
        let dir = scratch("popup");
        listed(&dir, plan());
        let mut io = Fake::new(None, true);
        io.dir = Some(dir.clone());
        run_noted(&io, &dir, "w91:p1", BIN, 4711);
        let popups = io.popups.borrow().clone();
        assert_eq!(popups.len(), 1);
        let (pane, report) = &popups[0];
        assert_eq!(pane, "w91:p1");
        let (title, body) = report.clone().expect("the popup got the text");
        assert_eq!(title, "Fix the login bug was not moved");
        assert!(body.contains("did not end"), "{body}");
        assert!(
            body.contains(&format!("HERDR_MULTIACCOUNT_USE=work claude --resume {ID}")),
            "{body}"
        );
        assert!(io.notified.borrow().is_empty(), "the popup only");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_whose_popup_does_not_come_is_notified() {
        // Herdr cannot open it, or it opened but never took the text.
        for (opens, takes) in [(false, false), (true, false)] {
            let dir = scratch(&format!("notify-{opens}"));
            listed(&dir, plan());
            let mut io = Fake::new(None, true);
            io.dir = Some(dir.clone());
            io.popup_opens = opens;
            io.popup_takes = takes;
            run_noted(&io, &dir, "w91:p1", BIN, 4711);
            let told = io.notified.borrow().clone();
            assert_eq!(told.len(), 1, "{opens}");
            assert_eq!(told[0].0, "Fix the login bug was not moved");
            assert!(told[0].1.contains("claude --resume"), "{}", told[0].1);
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing left");
            // A popup that comes too late finds nothing to show.
            assert_eq!(take_report(&dir, "w91:p1"), None);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_list_that_takes_the_result_gets_no_popup() {
        for ends in [Some(2), None] {
            let dir = scratch(&format!("list-{}", ends.is_some()));
            listed(&dir, plan());
            let mut io = Fake::new(ends, true);
            io.dir = Some(dir.clone());
            io.list_takes = true;
            run_noted(&io, &dir, "w91:p1", BIN, 4711);
            assert!(io.popups.borrow().is_empty(), "{ends:?}");
            assert!(io.notified.borrow().is_empty(), "{ends:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn where_a_move_stands() {
        let n = Note {
            plan: plan(),
            at: 1000,
            mover: None,
            result: None,
        };
        let yes = |_| true;
        let no = |_| false;
        assert_eq!(state(&n, 1005, &no), State::Running, "starting");
        assert_eq!(state(&n, 1011, &yes), State::Stopped, "never started");
        let n = Note {
            mover: Some(5),
            ..n
        };
        assert_eq!(state(&n, 1030, &yes), State::Running);
        assert_eq!(state(&n, 1030, &no), State::Stopped);
        assert_eq!(state(&n, 1000 + 121, &yes), State::Stopped, "a reused pid");
        let n = Note {
            result: Some(Ok("Moved".into())),
            ..n
        };
        assert_eq!(state(&n, 5000, &no), State::Done(Ok("Moved".into())));
    }

    #[test]
    fn a_held_result_goes_back_once_its_list_is_gone() {
        let dir = scratch("held");
        listed(&dir, plan());
        let held = hold(&dir, "w91:p1", 9).unwrap();
        assert!(hold(&dir, "w91:p1", 10).is_none(), "taken once");
        assert!(notes(&dir).is_empty());
        recover_held(&dir, &|_| true);
        assert!(notes(&dir).is_empty(), "its list still runs");
        recover_held(&dir, &|_| false);
        assert_eq!(notes(&dir).len(), 1);
        assert!(!held.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_move_process_runs_in_a_session_of_its_own() {
        // What survived Herdr closing a plugin pane in a test (see the
        // decisions); a harmless program in place of the binary.
        extern "C" {
            fn getsid(pid: i32) -> i32;
        }
        let dir = scratch("sid");
        let mark = dir.join("pid");
        std::fs::create_dir_all(&dir).unwrap();
        let script = format!("echo $$ > '{}'; sleep 2", mark.display());
        assert!(spawn_detached(
            Path::new("/bin/sh"),
            &["-c".to_string(), script]
        ));
        let mut pid = None;
        for _ in 0..200 {
            pid = std::fs::read_to_string(&mark)
                .ok()
                .and_then(|t| t.trim().parse::<i32>().ok());
            if pid.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = pid.expect("it started");
        // SAFETY: getsid only reads the process table.
        let (child, mine) = unsafe { (getsid(pid), getsid(0)) };
        assert_eq!(child, pid, "leads a session of its own");
        assert_ne!(child, mine);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn plan() -> Plan {
        Plan {
            pane: "w91:p1".into(),
            label: "Fix the login bug".into(),
            session_id: ID.into(),
            pid: 400,
            account_id: "work".into(),
            account_name: "Work".into(),
            // Started without options, model not noted: as before #9.
            carry: Moved {
                options: Some(vec![]),
                ..Moved::default()
            },
        }
    }

    /// A session started with options, now on another model.
    fn carrying() -> Plan {
        Plan {
            carry: Moved {
                options: Some(vec![
                    "--model=sonnet".into(),
                    "--add-dir=/work/lib".into(),
                    "--append-system-prompt=it's $(x) `y`; z\n[1m]".into(),
                ]),
                dropped: vec!["--worktree".into()],
                model: Some("claude-opus-4-5[1m]".into()),
            },
            ..plan()
        }
    }

    #[test]
    fn options_go_through_the_hand_over_never_through_the_shell() {
        let io = Fake::new(Some(2), true);
        let p = carrying();
        assert!(run_move(&io, &p, BIN).is_ok());
        let typed = io.ran.borrow()[0].clone();
        assert_eq!(
            typed,
            format!("HERDR_MULTIACCOUNT_USE=work \"{BIN}\" run --moved -- --resume {ID}")
        );
        let (id, args) = io.handed.borrow().clone().unwrap();
        assert_eq!(id, ID);
        assert_eq!(args, p.carry.args());
        for a in &args {
            let value = a.split_once('=').map(|(_, v)| v).unwrap_or("");
            assert!(value.is_empty() || !typed.contains(value), "{a} typed");
        }
        assert_eq!(io.log.borrow()[0], "hand over", "before the first Ctrl+C");
        assert!(!*io.dropped.borrow(), "left for the launcher");
    }

    #[test]
    fn a_move_that_stops_before_typing_takes_the_hand_over_back() {
        let mut p = carrying();
        // Does not end.
        let io = Fake::new(None, true);
        assert!(run_move(&io, &p, BIN).is_err());
        assert!(*io.dropped.borrow());
        // Starts working.
        let mut io = Fake::new(Some(3), true);
        io.busy_after = Some(1);
        assert!(run_move(&io, &p, BIN).is_err());
        assert!(*io.dropped.borrow());
        // The hand-over cannot be written: nothing is pressed.
        let mut io = Fake::new(Some(2), true);
        io.no_hand_over = true;
        let e = run_move(&io, &p, BIN).unwrap_err();
        assert!(e.starts_with("Nothing was changed"), "{e}");
        assert_eq!(*io.presses.borrow(), 0);
        // Nothing to carry: no hand-over, the command as before.
        p.carry = Moved {
            options: Some(vec![]),
            ..Moved::default()
        };
        let io = Fake::new(Some(2), true);
        assert!(run_move(&io, &p, BIN).is_ok());
        assert!(io.handed.borrow().is_none());
        assert!(!io.ran.borrow()[0].contains("--moved"));
        // Options not known: handed over as such, so they stay unknown.
        p.carry = Moved::default();
        let io = Fake::new(Some(2), true);
        assert!(run_move(&io, &p, BIN).is_ok());
        assert!(io.ran.borrow()[0].contains("run --moved --"));
        assert_eq!(io.handed.borrow().as_ref().map(|h| h.1.len()), Some(0));
    }

    #[test]
    fn the_way_by_hand_names_the_model_and_the_options() {
        let io = Fake::new(None, true);
        let e = run_move(&io, &carrying(), BIN).unwrap_err();
        assert!(
            e.contains(&format!(
                "HERDR_MULTIACCOUNT_USE=work claude --resume {ID} --model 'claude-opus-4-5[1m]', adding the options it was started with (--add-dir, --append-system-prompt)"
            )),
            "{e}"
        );
        assert!(!e.contains("$(x)"), "no values: {e}");
        // A started --model is named when the current one is not known.
        let mut p = carrying();
        p.carry.model = None;
        let io = Fake::new(None, true);
        let e = run_move(&io, &p, BIN).unwrap_err();
        assert!(
            e.contains(&format!("--resume {ID} --model 'sonnet'")),
            "{e}"
        );
    }

    #[test]
    fn the_command_resumes_that_conversation_on_that_account() {
        assert_eq!(
            command("work", ID, BIN, false).unwrap(),
            format!("HERDR_MULTIACCOUNT_USE=work \"{BIN}\" run -- --resume {ID}")
        );
        assert!(command("main", ID, "/opt/hm/bin/herdr-multiaccount", false).is_ok());
        for bad in ["", "work; rm -rf ~", "a b", "$(x)", "wörk"] {
            assert!(command(bad, ID, BIN, false).is_err(), "{bad}");
        }
        assert!(command("work", "not-a-uuid", BIN, false).is_err());
        assert!(command("work", &format!("{ID};ls"), BIN, false).is_err());
        for bad in ["", "$HOME/a\"b", "/a/$(x)", "/a/`x`", "$HOME/$HOME/x"] {
            assert!(command("work", ID, bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn ctrl_c_ends_it_then_the_same_conversation_starts_on_the_account() {
        // With unsent text: the first press clears it, the second shows
        // "Press Ctrl-C again to exit", the third ends it.
        let io = Fake::new(Some(3), true);
        let r = run_move(&io, &plan(), BIN);
        assert_eq!(r, Ok("Moved Fix the login bug to Work".to_string()));
        assert_eq!(
            *io.log.borrow(),
            vec![
                "keys ctrl+c".to_string(),
                "keys ctrl+c".to_string(),
                "keys ctrl+c".to_string(),
                format!("run HERDR_MULTIACCOUNT_USE=work \"{BIN}\" run -- --resume {ID}"),
            ],
            "only Ctrl+C before the end, no text, one start"
        );
    }

    #[test]
    fn no_more_presses_once_it_has_ended() {
        let io = Fake::new(Some(1), true);
        assert!(run_move(&io, &plan(), BIN).is_ok());
        assert_eq!(*io.presses.borrow(), 1);
    }

    #[test]
    fn a_session_that_does_not_end_starts_nothing() {
        let io = Fake::new(None, true);
        let e = run_move(&io, &plan(), BIN).unwrap_err();
        assert!(e.contains("did not end"), "{e}");
        assert!(e.contains("/exit"), "{e}");
        assert!(
            e.contains(&format!("HERDR_MULTIACCOUNT_USE=work claude --resume {ID}")),
            "{e}"
        );
        assert!(io.ran.borrow().is_empty(), "no second claude");
        assert_eq!(*io.presses.borrow(), PRESSES, "never more than 4 presses");
        assert!(*io.slept.borrow() >= END_WAIT, "waited before giving up");
    }

    #[test]
    fn a_start_that_fails_is_named_with_the_way_by_hand() {
        let io = Fake::new(Some(2), false);
        let e = run_move(&io, &plan(), BIN).unwrap_err();
        assert!(e.contains("did not start again"), "{e}");
        assert!(e.contains(&format!("claude --resume {ID}")), "{e}");
        assert_eq!(io.ran.borrow().len(), 1, "started once, not again");
    }

    #[test]
    fn a_bad_plan_touches_nothing() {
        let io = Fake::new(Some(2), true);
        let mut p = plan();
        p.account_id = "work\"; rm -rf ~; \"".into();
        assert!(run_move(&io, &p, BIN).is_err());
        assert!(io.log.borrow().is_empty());
    }

    #[test]
    fn a_session_that_starts_working_is_never_cut_off() {
        // Busy before the first press: nothing is pressed at all.
        let mut io = Fake::new(Some(3), true);
        io.busy_after = Some(0);
        let e = run_move(&io, &plan(), BIN).unwrap_err();
        assert!(e.contains("nothing was changed"), "{e}");
        assert!(io.log.borrow().is_empty());
        // Busy after the first press: no further press, nothing started.
        let mut io = Fake::new(Some(3), true);
        io.busy_after = Some(1);
        let e = run_move(&io, &plan(), BIN).unwrap_err();
        assert!(e.contains("started working"), "{e}");
        assert!(e.contains("/exit"), "{e}");
        assert_eq!(*io.presses.borrow(), 1);
        assert!(io.ran.borrow().is_empty());
    }

    #[test]
    fn an_end_into_a_nested_shell_starts_nothing_and_says_it_ended() {
        let mut io = Fake::new(Some(2), true);
        io.after_end = vec![(600, "bash".to_string())];
        let e = run_move(&io, &plan(), BIN).unwrap_err();
        assert!(e.starts_with("Claude Code ended"), "{e}");
        assert!(
            e.contains(&format!("HERDR_MULTIACCOUNT_USE=work claude --resume {ID}")),
            "{e}"
        );
        assert!(!e.contains("/exit"), "{e}");
        assert!(
            io.ran.borrow().is_empty(),
            "nothing typed into the nested shell"
        );
    }

    #[test]
    fn when_herdr_cannot_tell_nothing_starts_and_it_says_so() {
        let mut io = Fake::new(Some(2), true);
        io.blind_after_end = true;
        let e = run_move(&io, &plan(), BIN).unwrap_err();
        assert!(e.starts_with("Herdr could not tell"), "{e}");
        assert!(io.ran.borrow().is_empty());
    }

    #[test]
    fn the_launcher_alone_in_front_is_no_start() {
        // The launcher stopped before Claude Code (a missing key, say)
        // while Herdr still names the old agent: no success.
        let mut io = Fake::new(Some(2), false);
        io.launcher_stays = true;
        let e = run_move(&io, &plan(), BIN).unwrap_err();
        assert!(e.contains("did not start again"), "{e}");
        assert!(is_launcher("herdr-multiaccount"));
        assert!(is_launcher("/x/bin/herdr-multiaccou"));
        assert!(!is_launcher("2.1.30"));
    }
}
