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
//! 4. Wait until Herdr sees Claude Code in the pane again.
//!
//! The list checks before that the session is idle and which conversation
//! it holds (`sessions::Conversation`, noted by the status line).

use crate::herdr::{self, Processes};
use crate::launch;
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
/// started before setup.
pub fn command(account_id: &str, session_id: &str, bin: &str) -> Result<String, String> {
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
    Ok(format!(
        "{}={account_id} \"{bin}\" run -- --resume {session_id}",
        launch::ENV_USE
    ))
}

/// Starting the conversation by hand, once Claude Code has ended.
fn resume_by_hand(account_id: &str, session_id: &str) -> String {
    format!(
        "{}={account_id} claude --resume {session_id}",
        launch::ENV_USE
    )
}

/// The same move by hand, for the error messages.
pub fn manual(account_id: &str, session_id: &str) -> String {
    format!(
        "type /exit in it, then {}",
        resume_by_hand(account_id, session_id)
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
    let by_hand = manual(&plan.account_id, &plan.session_id);
    let resume = resume_by_hand(&plan.account_id, &plan.session_id);
    let cmd = command(&plan.account_id, &plan.session_id, bin)
        .map_err(|e| format!("Nothing was changed: {e}. By hand: {by_hand}."))?;
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
    if !io.run(&plan.pane, &cmd) {
        return Err(format!(
            "Claude Code ended, but Herdr could not start it again. By hand: {resume}."
        ));
    }
    if !wait(io, START_WAIT, &|| started(io, &plan.pane, plan.pid)) {
        return Err(format!(
            "Claude Code did not start again in it; look at the pane for why. By hand: {resume}."
        ));
    }
    Ok(format!("Moved {} to {}", plan.label, plan.account_name))
}

/// The move against the real Herdr (on the list's background thread).
pub fn perform(plan: Plan) -> Result<String, String> {
    run_move(&Herdr, &plan, &crate::setup::bin_for_shell())
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
        ran: RefCell<Vec<String>>,
        log: RefCell<Vec<String>>,
        slept: RefCell<Duration>,
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
                ran: RefCell::new(vec![]),
                log: RefCell::new(vec![]),
                slept: RefCell::new(Duration::ZERO),
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
        }
    }

    fn plan() -> Plan {
        Plan {
            pane: "w91:p1".into(),
            label: "Fix the login bug".into(),
            session_id: ID.into(),
            pid: 400,
            account_id: "work".into(),
            account_name: "Work".into(),
        }
    }

    #[test]
    fn the_command_resumes_that_conversation_on_that_account() {
        assert_eq!(
            command("work", ID, BIN).unwrap(),
            format!("HERDR_MULTIACCOUNT_USE=work \"{BIN}\" run -- --resume {ID}")
        );
        assert!(command("main", ID, "/opt/hm/bin/herdr-multiaccount").is_ok());
        for bad in ["", "work; rm -rf ~", "a b", "$(x)", "wörk"] {
            assert!(command(bad, ID, BIN).is_err(), "{bad}");
        }
        assert!(command("work", "not-a-uuid", BIN).is_err());
        assert!(command("work", &format!("{ID};ls"), BIN).is_err());
        for bad in ["", "$HOME/a\"b", "/a/$(x)", "/a/`x`", "$HOME/$HOME/x"] {
            assert!(command("work", ID, bad).is_err(), "{bad}");
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
