//! The list's state and what the keys and the mouse do.

use crate::carry::Moved;
use crate::config::{self, Config};
use crate::herdr::{self, Pane, Workspace};
use crate::info::{self, Info};
use crate::launch;
use crate::mirror;
use crate::paths;
use crate::relocate;
use crate::secret;
use crate::setup;
use crate::signin::SignIn;
use crate::store::{Account, Kind, Store, MAIN_ID};
use crate::time;
use crate::ui;
use crate::usage::Usage;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::DefaultTerminal;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Accounts,
    Spaces,
    Sessions,
}

impl Section {
    pub const ORDER: [Section; 3] = [Section::Accounts, Section::Spaces, Section::Sessions];

    pub fn name(self) -> &'static str {
        match self {
            Section::Accounts => "Accounts",
            Section::Spaces => "Spaces",
            Section::Sessions => "Agents",
        }
    }

    fn next(self, by: i32) -> Section {
        let i = Section::ORDER.iter().position(|s| *s == self).unwrap_or(0) as i32;
        let n = Section::ORDER.len() as i32;
        Section::ORDER[(i + by).rem_euclid(n) as usize]
    }
}

/// A clickable area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Section(Section),
    Row(usize),
    PickRow(usize),
    Key(char),
    /// The repository link in the keys overlay's bottom border.
    Link,
}

/// Runs the system's opener (first argument) on a web link, true when it
/// could be started. Replaceable, so tests open nothing.
pub type Opener = fn(&str, &str) -> bool;

/// Reads Herdr's panes (`herdr::panes`), and closes one (`herdr::close_pane`);
/// fields of the app so tests never reach Herdr.
pub type PaneList = fn() -> Option<Vec<Pane>>;
pub type PaneCloser = fn(&str) -> bool;
/// Reads Herdr's spaces (`herdr::workspaces`), and closes one, with its
/// worktree group when asked (`herdr::close_workspace`).
pub type SpaceList = fn() -> Option<Vec<Workspace>>;
pub type SpaceCloser = fn(&str, bool) -> bool;
/// Changes `accounts.json` under its lock (`Store::update`); a field so
/// tests never write the real config directory.
pub type StoreUpdate = for<'a> fn(Box<dyn FnOnce(&mut Store) + 'a>) -> std::io::Result<Store>;
/// Signs an account's folders out (`sign_out`), removes its folders
/// (`mirror::remove`) and starts a sign-in (`SignIn::start`); fields so
/// tests run without Claude Code or the real config directory.
pub type SignOut = fn(&Account) -> Vec<PathBuf>;
pub type FolderRemover = fn(&str) -> std::io::Result<Vec<String>>;
pub type SignInStarter = fn(&Account, bool) -> Result<SignIn, String>;
/// The conversation a pane holds now and its Claude Code process
/// (`real_conversation`), and the start of the move (`relocate::perform`,
/// which notes it in the folder given and starts its process); fields so
/// tests never reach Herdr.
pub type ConversationOf = fn(&str) -> Option<Held>;

/// The conversation a pane holds, its Claude Code process, and what a move
/// carries over.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub session_id: String,
    pub pid: u32,
    pub carry: Moved,
}
/// The folder of the notes, this list's id and the plan.
pub type Mover = fn(&std::path::Path, u32, relocate::Plan) -> Result<(), String>;

/// The program that opens links: `open` on macOS, `xdg-open` elsewhere.
const OPENER: &str = if cfg!(target_os = "macos") {
    "open"
} else {
    "xdg-open"
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddKind {
    Subscription,
    SubscriptionSso,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PickFor {
    AddKind,
    Space {
        workspace: String,
        label: String,
    },
    /// The account to move a running session to.
    MoveSession(Moving),
}

/// A session to move, as the list saw it when `m` was pressed.
#[derive(Debug, Clone, PartialEq)]
pub struct Moving {
    pub pane: String,
    pub label: String,
    pub session_id: String,
    pub pid: u32,
    /// The account it runs on now (the main login when unknown).
    pub from: String,
    /// The start options and model the move carries over.
    pub carry: Moved,
}

/// A move that runs: the session's pane and name, and the account it goes
/// to, shown until its result arrives.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveRun {
    pub pane: String,
    pub label: String,
    pub to: String,
}

impl MoveRun {
    fn of(p: &relocate::Plan) -> MoveRun {
        MoveRun {
            pane: p.pane.clone(),
            label: p.label.clone(),
            to: p.account_name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickItem {
    pub label: String,
    pub detail: String,
    /// Account id for a space; `None` for "the active account".
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub title: String,
    pub items: Vec<PickItem>,
    pub sel: usize,
    pub purpose: PickFor,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputFor {
    NewAccount(AddKind),
    Rename(String),
    /// An API key for an account (`id`), or for a new one named `name`.
    ApiKey {
        id: Option<String>,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub title: String,
    pub hint: String,
    pub text: String,
    pub purpose: InputFor,
    /// Shown as dots (an API key).
    pub secret: bool,
}

impl Input {
    fn max_len(&self) -> usize {
        if self.secret {
            300
        } else {
            40
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmFor {
    Remove(String),
    Setup,
    /// Ends the Claude Code session in this pane by closing the pane.
    EndSession {
        pane: String,
        label: String,
    },
    /// Moves the session to the account `to`.
    MoveSession {
        session: Moving,
        to: String,
    },
    /// Closes a space (`spaces[0]`), with the worktree spaces the question
    /// named when it is their main checkout.
    CloseSpace {
        spaces: Vec<Workspace>,
        label: String,
        /// The sessions the question counted, and those it warned about
        /// (working or waiting for an answer).
        sessions: Vec<String>,
        warned: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub question: String,
    pub lines: Vec<String>,
    pub purpose: ConfirmFor,
}

impl Confirm {
    /// Whether Enter answers yes: only on the move question; on the others
    /// (end, remove, setup) a stray Enter must not lose work.
    pub fn enter_says_yes(&self) -> bool {
        matches!(self.purpose, ConfirmFor::MoveSession { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Normal,
    Help,
    Pick(Picker),
    Input(Input),
    Confirm(Confirm),
    /// A claude.ai sign-in runs (`App::signin`); `code` is what the page may
    /// ask to paste back.
    SignIn {
        code: String,
    },
    /// A note to read, closed with Enter or Esc.
    Message {
        title: String,
        lines: Vec<String>,
        good: bool,
    },
    AskQuit,
}

type HerdrState = (Option<Vec<Workspace>>, Option<Vec<Pane>>);

/// How the list works, shown on first open and under `?`.
pub const INTRO: [&str; 6] = [
    "Main login is the login Claude Code already has in ~/.claude, the one /login sets. This list never changes it; to switch it, run /login in Claude Code.",
    "Every other account sits next to it in a folder of its own with its own login, and shares your settings, skills, plugins and history.",
    "New sessions start on the active account; choose it with ⏎. Running sessions keep theirs, and a space can have an account of its own (Spaces).",
    "When an account is at its limit, new sessions go to the next account in the list that has room (fallback, F).",
    "Limits come from Claude Code: its /usage, read every minute while this list is open, and the status line of running sessions.",
    "Agents shows which account each Claude Code session runs on. ? shows all keys and this note again.",
];

/// The field an API key is pasted into.
fn key_input(id: Option<String>, name: String) -> Mode {
    Mode::Input(Input {
        title: format!("API key for {name}"),
        hint: "Paste the key from platform.claude.com (API keys). It is shown as dots and goes straight into the keychain.".into(),
        text: String::new(),
        purpose: InputFor::ApiKey { id, name },
        secret: true,
    })
}

pub struct App {
    pub config: Config,
    pub dir: PathBuf,
    pub store: Store,
    pub usage: HashMap<String, Usage>,
    /// For each account, the others signed in with the same login.
    pub twins: HashMap<String, Vec<String>>,
    pub info: HashMap<String, Info>,
    pub info_loading: bool,
    /// Results of the sign-in refresh: the account, what was found, and
    /// for a subscription the folder that was checked.
    info_rx: Option<Receiver<(String, Info, Option<PathBuf>)>>,
    info_left: usize,
    /// When the running refresh started (Unix seconds).
    info_started: i64,
    /// Accounts whose noted check was thrown away since the refresh
    /// started: its results for them are not noted.
    info_forgotten: HashSet<String>,
    /// Where the noted checks are, for the launcher (`<config>/signin`).
    pub checks_dir: PathBuf,
    pub sign_out: SignOut,
    pub folder_remover: FolderRemover,
    pub sign_in_starter: SignInStarter,
    herdr_rx: Option<Receiver<HerdrState>>,
    pub workspaces: Vec<Workspace>,
    pub panes: Vec<Pane>,
    pub section: Section,
    pub sel: usize,
    pub space_sel: usize,
    pub session_sel: usize,
    pub mode: Mode,
    /// How far the keys are scrolled, and how far they can be (set when
    /// drawn).
    pub help_scroll: u16,
    pub help_max: u16,
    /// The same for a question taller than the pane (set when drawn), and
    /// the question it belongs to, so a new one starts at the top.
    pub confirm_scroll: u16,
    pub confirm_max: u16,
    pub confirm_of: String,
    /// The dialog the quit bar (Ctrl+C) was asked over, back on "stay".
    quit_over: Option<Mode>,
    pub flash: Option<(String, Color, Instant)>,
    pub setup: setup::State,
    pub signin: Option<SignIn>,
    pub quit: bool,
    pub hits: Vec<(Rect, Hit)>,
    pub opener: Opener,
    pub pane_list: PaneList,
    pub pane_closer: PaneCloser,
    pub space_list: SpaceList,
    pub space_closer: SpaceCloser,
    pub store_update: StoreUpdate,
    /// The list's own pane in Herdr (`HERDR_PANE_ID`).
    pub own_pane: Option<String>,
    pub conversation_of: ConversationOf,
    pub mover: Mover,
    /// Whether the list runs inside Herdr (`herdr::inside`).
    pub in_herdr: bool,
    /// A move that runs, from the notes of moves (`relocate::Note`), also
    /// one another list started.
    pub moving: Option<MoveRun>,
    /// Where the notes of moves are (`<config>/moving`).
    pub moves_dir: PathBuf,
    next_moves: Instant,
    /// This list, in the notes of the moves it starts (its process id).
    pub me: u32,
    pub cwd: Option<String>,
    pub read_at: Instant,
    next_files: Instant,
    next_herdr: Instant,
    next_info: Instant,
    /// Limit checks through Claude Code's `/usage` (`probe.rs`).
    probe_rx: Option<Receiver<()>>,
    pub probing: bool,
    next_probe: Instant,
    last_click: Option<(Instant, Hit)>,
}

impl App {
    pub fn new(config: Config, dir: PathBuf) -> App {
        let store = Store::load();
        let twins = crate::login::twins(&store, &crate::login::all(&store));
        let usage = launch::load_usage_with(&store, &twins);
        let moves_dir = dir.join(relocate::MOVING);
        App {
            config,
            dir,
            store,
            usage,
            twins,
            info: HashMap::new(),
            info_loading: false,
            info_rx: None,
            info_left: 0,
            info_started: 0,
            info_forgotten: HashSet::new(),
            checks_dir: paths::checks_dir(),
            sign_out,
            folder_remover: mirror::remove,
            sign_in_starter: SignIn::start,
            herdr_rx: None,
            workspaces: vec![],
            panes: vec![],
            section: Section::Accounts,
            sel: 0,
            space_sel: 0,
            session_sel: 0,
            mode: Mode::Normal,
            help_scroll: 0,
            help_max: 0,
            confirm_scroll: 0,
            confirm_max: 0,
            confirm_of: String::new(),
            quit_over: None,
            flash: None,
            setup: setup::state(),
            signin: None,
            quit: false,
            hits: vec![],
            opener: open_in_browser,
            pane_list: herdr::panes,
            pane_closer: herdr::close_pane,
            space_list: herdr::workspaces,
            space_closer: herdr::close_workspace,
            store_update: |change| Store::update(change),
            own_pane: std::env::var("HERDR_PANE_ID").ok(),
            conversation_of: real_conversation,
            mover: relocate::perform,
            in_herdr: herdr::inside(),
            moving: None,
            moves_dir,
            next_moves: Instant::now(),
            me: std::process::id(),
            cwd: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            read_at: Instant::now(),
            next_files: Instant::now(),
            next_herdr: Instant::now(),
            next_info: Instant::now(),
            probe_rx: None,
            probing: false,
            next_probe: Instant::now(),
            last_click: None,
        }
    }

    // ---- data -------------------------------------------------------

    pub fn now(&self) -> i64 {
        time::now()
    }

    pub fn selected(&self) -> Option<&Account> {
        self.store.accounts.get(self.sel)
    }

    /// Accounts.json and the usage readings, which the status line writes.
    fn reload_files(&mut self) {
        let keep = self.selected().map(|a| a.id.clone());
        self.store = Store::load();
        self.twins = crate::login::twins(&self.store, &crate::login::all(&self.store));
        self.usage = launch::load_usage_with(&self.store, &self.twins);
        if let Some(id) = keep {
            if let Some(i) = self.store.accounts.iter().position(|a| a.id == id) {
                self.sel = i;
            }
        }
        self.sel = self.sel.min(self.store.accounts.len().saturating_sub(1));
        self.read_at = Instant::now();
        self.next_files = Instant::now() + Duration::from_secs(3);
    }

    fn start_herdr(&mut self) {
        self.next_herdr = Instant::now() + Duration::from_secs(5);
        if self.herdr_rx.is_some() || !herdr::inside() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send((herdr::workspaces(), herdr::panes()));
        });
        self.herdr_rx = Some(rx);
    }

    fn receive_herdr(&mut self) {
        let Some(rx) = &self.herdr_rx else {
            return;
        };
        if let Ok((ws, panes)) = rx.try_recv() {
            if let Some(ws) = ws {
                self.workspaces = ws;
            }
            if let Some(p) = panes {
                self.panes = p;
            }
            self.space_sel = self.space_sel.min(self.workspaces.len().saturating_sub(1));
            self.session_sel = self
                .session_sel
                .min(self.sessions().len().saturating_sub(1));
            self.herdr_rx = None;
        }
    }

    /// Who each account is signed in as. Slow (Claude Code's command line
    /// per account), so it runs in threads.
    pub fn start_info(&mut self) {
        self.next_info = Instant::now() + Duration::from_secs(600);
        let (tx, rx) = channel();
        self.begin_info();
        for account in self.store.accounts.clone() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                // Before `info::read`, which asks about this same folder: a
                // sign-in that moves the account to its new folder throws
                // the note away anyway (`forget_check`).
                let folder =
                    (account.kind == Kind::Subscription).then(|| mirror::folder(&account.id));
                let _ = tx.send((account.id.clone(), info::read(&account), folder));
            });
        }
        self.info_rx = Some(rx);
    }

    /// The bookkeeping of a new sign-in refresh, before its threads run.
    fn begin_info(&mut self) {
        self.info_left = self.store.accounts.len();
        self.info_started = time::now();
        self.info_forgotten.clear();
        self.info_loading = true;
    }

    fn receive_info(&mut self) {
        let Some(rx) = &self.info_rx else {
            return;
        };
        let mut received = vec![];
        while let Ok(r) = rx.try_recv() {
            received.push(r);
        }
        for (id, info, folder) in received {
            if let Some(folder) = folder {
                self.note_check(&id, &folder, info.own_login());
            }
            self.info.insert(id, info);
            self.info_left = self.info_left.saturating_sub(1);
        }
        if self.info_left == 0 {
            self.info_rx = None;
            self.info_loading = false;
        }
    }

    /// Notes a subscription's sign-in check for the launcher, unless its
    /// login may have changed since the check started: a sign-in runs for
    /// it, or the check was thrown away since (`forget_check`).
    fn note_check(&mut self, id: &str, folder: &std::path::Path, signed_in: bool) {
        if self.info_forgotten.contains(id) || self.signin.as_ref().is_some_and(|s| s.id == id) {
            return;
        }
        info::save_check(&self.checks_dir, id, folder, signed_in, self.info_started);
    }

    /// Throws away the noted sign-in check of `id` before anything that can
    /// change its login, and keeps a refresh that runs from noting it again.
    fn forget_check(&mut self, id: &str) {
        info::forget_check(&self.checks_dir, id);
        self.info_forgotten.insert(id.to_string());
    }

    /// The accounts whose limits `/usage` can show: signed-in
    /// subscriptions, and the main login when it is a claude.ai login.
    fn probe_targets(&self) -> Vec<(String, Option<PathBuf>)> {
        self.store
            .accounts
            .iter()
            .filter(|a| {
                let Some(i) = self.info.get(&a.id) else {
                    return false;
                };
                match a.kind {
                    Kind::Main => i.signed_in && i.claude_ai,
                    Kind::Subscription => i.own_login(),
                    Kind::ApiKey => false,
                }
            })
            .map(|a| {
                let home = a.kind.has_home().then(|| mirror::folder(&a.id));
                (a.id.clone(), home)
            })
            .collect()
    }

    /// Reads the limits of every account through Claude Code's `/usage`,
    /// one after another in the background.
    fn start_probe(&mut self) {
        if self.config.check_limits == 0 || self.probe_rx.is_some() || self.info_loading {
            return;
        }
        self.next_probe = Instant::now() + Duration::from_secs(self.config.check_limits);
        let targets = self.probe_targets();
        if targets.is_empty() {
            return;
        }
        let cwd = paths::config_dir().join("probe");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for (id, home) in targets {
                if let Ok(reading) = crate::probe::run(home.as_deref(), &cwd) {
                    let now = time::now();
                    let _ = Usage::update(&id, |u| u.record_probe(&reading, now));
                }
            }
            let _ = tx.send(());
        });
        self.probe_rx = Some(rx);
        self.probing = true;
    }

    fn receive_probe(&mut self) {
        let Some(rx) = &self.probe_rx else {
            return;
        };
        if rx.try_recv().is_ok() {
            self.probe_rx = None;
            self.probing = false;
            self.reload_files();
        }
    }

    /// Claude Code sessions in Herdr, by space and tab.
    pub fn sessions(&self) -> Vec<Pane> {
        let order = |ws: &str| {
            self.workspaces
                .iter()
                .find(|w| w.id == ws)
                .map(|w| w.number)
                .unwrap_or(u64::MAX)
        };
        let mut list: Vec<Pane> = self
            .panes
            .iter()
            .filter(|p| self.is_session(p))
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            order(&a.workspace)
                .cmp(&order(&b.workspace))
                .then_with(|| a.tab.cmp(&b.tab))
                .then_with(|| a.id.cmp(&b.id))
        });
        list
    }

    pub fn space_label(&self, ws: &str) -> String {
        self.workspaces
            .iter()
            .find(|w| w.id == ws)
            .map(|w| w.label.clone())
            .unwrap_or_default()
    }

    fn go_to_session(&mut self) {
        let Some(p) = self.sessions().get(self.session_sel).cloned() else {
            return;
        };
        if herdr::focus_tab(&p.tab) {
            self.flash(
                format!("Went to {}", self.space_label(&p.workspace)),
                crate::theme::ACCENT,
            );
        } else {
            self.flash("Herdr could not show that tab", crate::theme::RED);
        }
    }

    /// The account a session runs on, as the Agents tab names it.
    fn session_account(&self, p: &Pane) -> String {
        match (&p.account_id, &p.account) {
            (Some(id), _) if self.store.account(id).is_some() => self.store.name_of(id),
            (_, Some(name)) => name.clone(),
            _ => format!("{} (likely)", self.store.name_of(MAIN_ID)),
        }
    }

    /// Asks before ending the selected session (`d` in the Agents tab).
    fn ask_end_session(&mut self) {
        let Some(p) = self.sessions().get(self.session_sel).cloned() else {
            return;
        };
        let label = Self::pane_name(&p);
        let space = self.space_label(&p.workspace);
        let account = self.session_account(&p);
        let mut lines = vec![if space.is_empty() {
            format!("It runs on {account}.")
        } else {
            format!("It runs in {space} on {account}.")
        }];
        match p.status.as_deref() {
            Some("working") => {
                lines.push("It is working right now: its current work stops.".into())
            }
            Some("blocked") => {
                lines.push("It is waiting for your answer: its current work stops.".into())
            }
            _ => {}
        }
        lines.push("Its pane closes; claude --resume finds the conversation again.".into());
        self.mode = Mode::Confirm(Confirm {
            question: format!("End {label}?"),
            lines,
            purpose: ConfirmFor::EndSession { pane: p.id, label },
        });
    }

    /// Closes the session's pane, after checking with Herdr that the pane
    /// still runs that session: a row read seconds ago may be stale.
    fn end_session(&mut self, pane: &str, label: &str) {
        let Some(mut now) = (self.pane_list)() else {
            self.flash(
                "Herdr could not be asked; nothing was ended",
                crate::theme::RED,
            );
            return;
        };
        // Herdr's pane list carries no pane tokens: the account comes only
        // from the launcher's note, which goes with its process
        // (`sessions::live`), so a shell left in the pane does not count.
        let still = now.iter().any(|p| p.id == pane && self.is_session(p));
        if still {
            if !(self.pane_closer)(pane) {
                self.flash(format!("Herdr could not close {label}"), crate::theme::RED);
                return;
            }
            now.retain(|p| p.id != pane);
            self.flash(format!("Ended {label}"), crate::theme::ACCENT);
        } else {
            self.flash(format!("{label} had already ended"), crate::theme::MUTED);
        }
        self.panes = now;
        self.after_close();
    }

    /// Whether a session is working or waiting for an answer: closing it
    /// stops that.
    fn busy(p: &Pane) -> bool {
        matches!(p.status.as_deref(), Some("working") | Some("blocked"))
    }

    /// A space's name in the list: its label, else its number.
    fn space_name(ws: &Workspace) -> String {
        if ws.label.is_empty() {
            format!("space {}", ws.number)
        } else {
            ws.label.clone()
        }
    }

    /// The spaces closing `id` takes, by Herdr's own rule: the main checkout
    /// of a repository takes every open space of that repository with it
    /// (when there are any); every other space closes alone.
    fn space_group(spaces: &[Workspace], id: &str) -> Vec<Workspace> {
        let Some(ws) = spaces.iter().find(|w| w.id == id) else {
            return vec![];
        };
        let group: Vec<Workspace> = match ws.worktree.as_ref().filter(|t| !t.linked) {
            Some(main) => spaces
                .iter()
                .filter(|w| {
                    w.worktree
                        .as_ref()
                        .is_some_and(|t| t.repo_key == main.repo_key)
                })
                .cloned()
                .collect(),
            None => vec![],
        };
        if group.len() >= 2 {
            // The chosen space first, the others in Herdr's order.
            let mut list = vec![ws.clone()];
            list.extend(group.into_iter().filter(|w| w.id != id));
            list
        } else {
            vec![ws.clone()]
        }
    }

    /// Asks before closing the selected space (`d` in the Spaces tab).
    fn ask_close_space(&mut self) {
        let Some(ws) = self.workspaces.get(self.space_sel).cloned() else {
            return;
        };
        let label = Self::space_name(&ws);
        let spaces = Self::space_group(&self.workspaces, &ws.id);
        let group = spaces.len() > 1;
        let mut lines = vec![];
        if group {
            let others: Vec<String> = spaces[1..].iter().map(Self::space_name).collect();
            lines.push(format!(
                "It is the main checkout of its repository: the repository's other spaces {} close with it.",
                others.join(", ")
            ));
        }
        let sessions: Vec<&Pane> = spaces
            .iter()
            .flat_map(|w| self.sessions_in(&w.id))
            .collect();
        let place = if group { "in them" } else { "in it" };
        lines.push(match sessions.len() {
            0 => format!("No Claude Code session runs {place}."),
            1 => format!("1 Claude Code session runs {place}."),
            n => format!("{n} Claude Code sessions run {place}."),
        });
        let busy = sessions.iter().filter(|p| Self::busy(p)).count();
        if busy > 0 {
            lines.push(if sessions.len() == 1 {
                "It is working or waiting for your answer: its current work stops.".into()
            } else {
                format!(
                    "{busy} of them {} working or waiting for your answer: their current work stops.",
                    if busy == 1 { "is" } else { "are" }
                )
            });
        }
        lines.push(
            "Its tabs and everything running in them close; claude --resume finds the conversations again."
                .into(),
        );
        if spaces
            .iter()
            .any(|w| w.worktree.as_ref().is_some_and(|t| t.linked))
        {
            lines.push("Worktree checkouts and their branches stay on disk.".into());
        }
        let own = self
            .own_pane
            .as_deref()
            .and_then(|me| self.panes.iter().find(|p| p.id == me))
            .is_some_and(|me| spaces.iter().any(|w| w.id == me.workspace));
        if own {
            lines.push(format!(
                "This list runs in {}: it closes too.",
                if group { "one of them" } else { "it" }
            ));
        }
        let ids = sessions.iter().map(|p| p.id.clone()).collect();
        self.mode = Mode::Confirm(Confirm {
            question: format!("Close {label}?"),
            lines,
            purpose: ConfirmFor::CloseSpace {
                spaces,
                label,
                sessions: ids,
                warned: sessions
                    .iter()
                    .filter(|p| Self::busy(p))
                    .map(|p| p.id.clone())
                    .collect(),
            },
        });
    }

    /// Closes the spaces the question named, after checking with Herdr that
    /// the space is still there, and that the close takes no space and no
    /// session the question did not name, nor a busy session it did not
    /// warn about.
    fn close_space(
        &mut self,
        named: &[Workspace],
        label: &str,
        counted: &[String],
        warned: &[String],
    ) {
        let Some(first) = named.first() else {
            return;
        };
        if let Some(m) = self.running_move() {
            self.flash(
                format!("{} is being moved; nothing was closed", m.label),
                crate::theme::MUTED,
            );
            return;
        }
        let Some(now) = (self.space_list)() else {
            self.flash(
                "Herdr could not be asked; nothing was closed",
                crate::theme::RED,
            );
            return;
        };
        if !now.iter().any(|w| w.id == first.id) {
            self.flash(format!("{label} had already closed"), crate::theme::MUTED);
            self.workspaces = now;
            self.after_close();
            return;
        }
        let group = Self::space_group(&now, &first.id);
        if let Some(new) = group.iter().find(|w| !named.iter().any(|n| n.id == w.id)) {
            self.flash(
                format!(
                    "{} opened since; nothing was closed. Press d again",
                    Self::space_name(new)
                ),
                crate::theme::YELLOW,
            );
            self.workspaces = now;
            self.after_close();
            return;
        }
        let Some(panes) = (self.pane_list)() else {
            self.flash(
                "Herdr could not be asked; nothing was closed",
                crate::theme::RED,
            );
            return;
        };
        let changed = panes
            .iter()
            .filter(|p| group.iter().any(|w| w.id == p.workspace) && self.is_session(p))
            .any(|p| !counted.contains(&p.id) || (Self::busy(p) && !warned.contains(&p.id)));
        if changed {
            self.flash(
                format!("{label} changed since the question; nothing was closed. Press d again"),
                crate::theme::YELLOW,
            );
            self.workspaces = now;
            self.panes = panes;
            self.after_close();
            return;
        }
        // The account choices go first: when the list runs in one of these
        // spaces, Herdr ends it with the close. Read from the stored file
        // itself, which another list may have changed since.
        let spaces: Vec<(String, String)> = group
            .iter()
            .map(|w| (w.id.clone(), w.label.clone()))
            .collect();
        let mut forgot: Vec<(String, String, String)> = vec![];
        if !self.update(|s| {
            for (id, name) in &spaces {
                if let Some(account) = s.space_account(id).map(str::to_string) {
                    s.assign(id, name, None);
                    forgot.push((id.clone(), name.clone(), account));
                }
            }
        }) {
            return;
        }
        if !(self.space_closer)(&first.id, group.len() > 1) {
            // Back only for the spaces still open (Herdr may have closed
            // part of them before it failed).
            let open = (self.space_list)();
            forgot.retain(|(id, _, _)| open.as_ref().is_none_or(|l| l.iter().any(|w| &w.id == id)));
            let back = forgot.is_empty()
                || self.update(|s| {
                    for (id, name, account) in &forgot {
                        // A choice made meanwhile (another list) wins.
                        if s.space_account(id).is_some() {
                            continue;
                        }
                        s.assign(id, name, Some(account));
                    }
                });
            self.flash(
                if back {
                    format!("Herdr could not close {label}")
                } else {
                    format!("Herdr could not close {label}, and its account choice is lost: choose it again with ⏎")
                },
                crate::theme::RED,
            );
            return;
        }
        self.panes = panes;
        let gone: Vec<&str> = group.iter().map(|w| w.id.as_str()).collect();
        self.workspaces = now;
        self.workspaces.retain(|w| !gone.contains(&w.id.as_str()));
        self.panes.retain(|p| !gone.contains(&p.workspace.as_str()));
        self.flash(format!("Closed {label}"), crate::theme::ACCENT);
        self.after_close();
    }

    /// Keeps the selections on rows that exist, and reads Herdr again a
    /// moment after it is done closing (a read started before would bring
    /// the rows back).
    fn after_close(&mut self) {
        self.space_sel = self.space_sel.min(self.workspaces.len().saturating_sub(1));
        self.session_sel = self
            .session_sel
            .min(self.sessions().len().saturating_sub(1));
        self.herdr_rx = None;
        self.next_herdr = Instant::now() + Duration::from_secs(1);
    }

    /// The account id a session runs on; the main login when the list
    /// does not know it (started without the plugin).
    fn session_account_id(&self, p: &Pane) -> String {
        p.account_id
            .as_deref()
            .filter(|id| self.store.account(id).is_some())
            .unwrap_or(MAIN_ID)
            .to_string()
    }

    /// Whether `id` is `of` or has the same login.
    fn same_login(&self, of: &str, id: &str) -> bool {
        id == of
            || self
                .twins
                .get(of)
                .is_some_and(|t| t.iter().any(|x| x == id))
    }

    /// Whether a session can be moved to an account: it is checked and
    /// signed in with a login (or key) of its own, as the launcher demands.
    fn can_take(&self, id: &str) -> bool {
        self.store.account(id).is_some() && self.info.get(id).is_some_and(Info::own_login)
    }

    /// Why a session cannot be moved now, from Herdr's status.
    fn not_idle(label: &str, status: Option<&str>) -> Option<String> {
        match status {
            Some("idle") | Some("done") => None,
            Some("working") => Some(format!(
                "{label} can only be moved while it waits for you; it is working"
            )),
            Some("blocked") => Some(format!(
                "{label} can only be moved while it waits for you; it waits for your answer"
            )),
            _ => Some(format!(
                "Herdr cannot tell whether {label} is idle; try again in a moment"
            )),
        }
    }

    fn pane_name(p: &Pane) -> String {
        if p.label.is_empty() {
            p.id.clone()
        } else {
            p.label.clone()
        }
    }

    /// `m` in the Agents tab: the accounts to move the selected session to.
    fn ask_move_session(&mut self) {
        if !self.in_herdr {
            self.flash("Only inside Herdr", crate::theme::MUTED);
            return;
        }
        if let Some(m) = &self.moving {
            self.flash(
                format!("{} is being moved; wait a moment", m.label),
                crate::theme::MUTED,
            );
            return;
        }
        let Some(p) = self.sessions().get(self.session_sel).cloned() else {
            return;
        };
        let label = Self::pane_name(&p);
        if let Some(why) = Self::not_idle(&label, p.status.as_deref()) {
            self.flash(why, crate::theme::YELLOW);
            return;
        }
        let from = self.session_account_id(&p);
        let Some(held) = (self.conversation_of)(&p.id) else {
            self.mode = Mode::Message {
                title: format!("Which conversation {label} holds is not known"),
                lines: vec![
                    "Its status line notes it (every 30 seconds while it runs), and only once setup (S) is done. Try again in a moment.".into(),
                    format!(
                        "To move it by hand: type /exit in it, then {}=<account> claude --resume and pick the conversation.",
                        launch::ENV_USE
                    ),
                ],
                good: false,
            };
            return;
        };
        let now = self.now();
        let items: Vec<PickItem> = self
            .store
            .accounts
            .iter()
            .filter(|a| !self.same_login(&from, &a.id) && self.can_take(&a.id))
            .map(|a| {
                let limit = self
                    .usage
                    .get(&a.id)
                    .and_then(|u| u.at_limit(self.config.threshold, now));
                let detail = match limit {
                    Some(l) => format!(
                        "at its {} limit, resets in {}",
                        l.window,
                        time::span(l.resets_at - now)
                    ),
                    None => self
                        .info
                        .get(&a.id)
                        .and_then(|i| i.plan.clone())
                        .unwrap_or_default(),
                };
                PickItem {
                    label: a.name.clone(),
                    detail,
                    value: Some(a.id.clone()),
                }
            })
            .collect();
        if items.is_empty() {
            self.flash(
                if self.info_loading {
                    "The accounts are still being checked; try again in a moment".to_string()
                } else {
                    format!("No other signed-in account to move {label} to")
                },
                crate::theme::MUTED,
            );
            return;
        }
        self.mode = Mode::Pick(Picker {
            title: format!("Move {label} to"),
            items,
            sel: 0,
            purpose: PickFor::MoveSession(Moving {
                pane: p.id,
                label,
                session_id: held.session_id,
                pid: held.pid,
                from,
                carry: held.carry,
            }),
        });
    }

    /// Where a session picked for `wanted` goes: fallback as at any start,
    /// never back to its own login.
    pub fn move_target(&self, from: &str, wanted: &str) -> launch::Choice {
        let check = |id: &str| !self.same_login(from, id) && self.can_take(id);
        launch::choose(
            &self.store,
            &self.config,
            &self.usage,
            &check,
            wanted.to_string(),
            launch::Source::Active,
            self.now(),
        )
    }

    /// Asks before the move, saying where it goes and what it costs.
    fn ask_move_to(&mut self, session: Moving, wanted: &str) {
        let choice = self.move_target(&session.from, wanted);
        let to = self.store.name_of(&choice.id);
        let picked = self.store.name_of(wanted);
        let mut lines = vec![];
        let now = self.now();
        let full = self
            .usage
            .get(wanted)
            .and_then(|u| u.at_limit(self.config.threshold, now));
        match (&choice.moved, full) {
            (Some(l), _) => lines.push(format!(
                "{picked} is at its {} limit, so it goes to {to}.",
                l.window
            )),
            (None, Some(l)) if !self.config.fallback => lines.push(format!(
                "{picked} is at its {} limit; fallback is off.",
                l.window
            )),
            (None, Some(l)) => lines.push(format!(
                "{picked} is at its {} limit; no other account has room.",
                l.window
            )),
            _ => {}
        }
        lines.push(
            "It restarts there and its conversation carries on; unsent text in it is lost.".into(),
        );
        lines.extend(session.carry.lines());
        // The conversation's cache stays with the old login.
        let api_key = self
            .store
            .account(&choice.id)
            .is_some_and(|a| a.kind == Kind::ApiKey);
        lines.push(if api_key {
            "The first answer is slower and costs more on its key.".to_string()
        } else {
            format!("The first answer is slower and counts more against {to}'s limit.")
        });
        self.mode = Mode::Confirm(Confirm {
            question: format!("Move {} to {to}?", session.label),
            lines,
            purpose: ConfirmFor::MoveSession {
                session,
                to: choice.id,
            },
        });
    }

    /// The user answered yes and waits for an outcome: a refusal gets the
    /// dialog a failed move gets, not a note in the footer.
    fn move_refused(&mut self, label: &str, why: String) {
        self.mode = Mode::Message {
            title: format!("{label} was not moved"),
            lines: vec![why],
            good: false,
        };
    }

    /// Starts the move after checking with Herdr once more that the session
    /// is still there, idle, and holds the same conversation.
    fn move_session(&mut self, s: Moving, to: &str) {
        let label = s.label.clone();
        let Some(now) = (self.pane_list)() else {
            self.move_refused(
                &label,
                format!(
                    "Herdr could not be asked whether {label} is still there and waits for you. Try again in a moment."
                ),
            );
            return;
        };
        let Some(p) = now.iter().find(|p| p.id == s.pane && self.is_session(p)) else {
            self.move_refused(&label, format!("{label} has ended."));
            return;
        };
        if let Some(why) = Self::not_idle(&label, p.status.as_deref()) {
            self.move_refused(&label, format!("{why}."));
            return;
        }
        let Some(held) = (self.conversation_of)(&s.pane)
            .filter(|h| h.session_id == s.session_id && h.pid == s.pid)
        else {
            self.move_refused(
                &label,
                format!("{label} holds another conversation now. Press m again to move that one."),
            );
            return;
        };
        // The question named the model; a /model since then would move it
        // on another. A model noted only since is the one it runs on.
        if s.carry.model.is_some() && held.carry.model != s.carry.model {
            self.move_refused(
                &label,
                format!("{label}'s model changed since the question. Press m again to move it on the new one."),
            );
            return;
        }
        let plan = relocate::Plan {
            pane: s.pane,
            label: label.clone(),
            session_id: s.session_id,
            pid: s.pid,
            account_id: to.to_string(),
            account_name: self.store.name_of(to),
            carry: held.carry,
        };
        // Another list may have started a move since this one last looked.
        if let Some(m) = self.running_move() {
            self.move_refused(
                &label,
                format!(
                    "{} is being moved; one move runs at a time. Try again once it is done.",
                    m.label
                ),
            );
            return;
        }
        let run = MoveRun::of(&plan);
        if let Err(e) = (self.mover)(&self.moves_dir, self.me, plan) {
            self.move_refused(&label, e);
            return;
        }
        // The footer shows it until the result arrives (`ui::draw_footer`).
        self.moving = Some(run);
        self.next_moves = Instant::now();
    }

    /// The move that runs now, from its note.
    fn running_move(&self) -> Option<MoveRun> {
        let now = crate::time::now();
        relocate::notes(&self.moves_dir)
            .iter()
            .find(|n| relocate::state(n, now, &crate::sessions::alive) == relocate::State::Running)
            .map(|n| MoveRun::of(&n.plan))
    }

    /// Reads the notes of moves (every half second): which one runs, and
    /// the results this list tells (`relocate::tells`). A result is taken
    /// only while no other dialog is open, so it is never held by a list
    /// that may close before showing it; the move process tells it
    /// otherwise.
    pub fn receive_move(&mut self) {
        if Instant::now() < self.next_moves {
            return;
        }
        self.next_moves = Instant::now() + Duration::from_millis(500);
        let dir = self.moves_dir.clone();
        let now = crate::time::now();
        let alive = crate::sessions::alive;
        let mut running = None;
        let mut finished = false;
        for note in relocate::notes(&dir) {
            let p = &note.plan;
            let state = relocate::state(&note, now, &alive);
            if state == relocate::State::Running {
                running.get_or_insert_with(|| MoveRun::of(p));
                continue;
            }
            if self.mode != Mode::Normal
                || !relocate::tells(&note, &state, self.me, &alive)
                || !relocate::take(&dir, &p.pane)
            {
                continue;
            }
            finished = true;
            let (title, line) = match state {
                relocate::State::Done(Ok(msg)) => {
                    self.flash(msg, crate::theme::GREEN);
                    continue;
                }
                relocate::State::Done(Err(e)) => (format!("{} was not moved", p.label), e),
                _ => relocate::stopped_text(p),
            };
            self.mode = Mode::Message {
                title,
                lines: vec![line],
                good: false,
            };
        }
        self.moving = running;
        if finished {
            // The pane's new label and note show on the next read.
            self.herdr_rx = None;
            self.next_herdr = Instant::now() + Duration::from_secs(1);
        }
    }

    pub fn refresh_all(&mut self) {
        self.reload_files();
        self.setup = setup::state();
        self.start_herdr();
        self.start_info();
        // The limits follow once the accounts are checked.
        self.next_probe = Instant::now();
    }

    pub fn flash(&mut self, text: impl Into<String>, color: Color) {
        self.flash = Some((text.into(), color, Instant::now() + Duration::from_secs(5)));
    }

    /// Changes the stored accounts (locked, on a fresh read) and takes the
    /// result. False when it could not be saved.
    fn update(&mut self, change: impl FnOnce(&mut Store)) -> bool {
        let keep = self.selected().map(|a| a.id.clone());
        match (self.store_update)(Box::new(change)) {
            Ok(store) => {
                self.store = store;
                if let Some(id) = keep {
                    if let Some(i) = self.store.accounts.iter().position(|a| a.id == id) {
                        self.sel = i;
                    }
                }
                self.sel = self.sel.min(self.store.accounts.len().saturating_sub(1));
                true
            }
            Err(e) => {
                self.flash(format!("Not saved: {e}"), crate::theme::RED);
                false
            }
        }
    }

    /// The names of the other accounts with the same login as `id`.
    pub fn twin_names(&self, id: &str) -> Vec<String> {
        self.twins
            .get(id)
            .map(|t| t.iter().map(|i| self.store.name_of(i)).collect())
            .unwrap_or_default()
    }

    /// The account new sessions get, with fallback, for `wanted`.
    pub fn effective(&self, wanted: &str) -> launch::Choice {
        let store = &self.store;
        // Accounts whose login or key is missing take no sessions.
        let check =
            |id: &str| store.account(id).is_some() && self.info.get(id).is_none_or(|i| i.signed_in);
        launch::choose(
            store,
            &self.config,
            &self.usage,
            &check,
            wanted.to_string(),
            launch::Source::Active,
            self.now(),
        )
    }

    /// Whether a pane runs Claude Code. The list itself starts `claude`
    /// for a moment (to check logins and limits), which Herdr may take for
    /// a session: its own panes never count.
    pub fn is_session(&self, p: &Pane) -> bool {
        let ours = p.label == "Multiaccount" || self.own_pane.as_deref() == Some(p.id.as_str());
        !ours && (p.agent.as_deref() == Some("claude") || p.account_id.is_some())
    }

    /// The Claude Code sessions in a space, as the Spaces tab counts them.
    pub fn sessions_in(&self, ws: &str) -> Vec<&Pane> {
        self.panes
            .iter()
            .filter(|p| p.workspace == ws && self.is_session(p))
            .collect()
    }

    /// Live Claude Code sessions per account, from the pane labels.
    pub fn running(&self, id: &str) -> usize {
        self.panes
            .iter()
            .filter(|p| p.account_id.as_deref() == Some(id))
            .filter(|p| p.agent.as_deref().is_none_or(|a| a == "claude"))
            .count()
    }

    /// How many open spaces start new sessions on this account right now:
    /// the spaces set to it, for the active account every space without its
    /// own, and after a fallback the spaces that moved over to it.
    pub fn spaces_using(&self, id: &str) -> usize {
        self.workspaces
            .iter()
            .filter(|w| {
                let wanted = self
                    .store
                    .space_account(&w.id)
                    .unwrap_or(self.store.active.as_str());
                self.effective(wanted).id == id
            })
            .count()
    }

    // ---- actions ----------------------------------------------------

    fn make_active(&mut self) {
        let Some(a) = self.selected().cloned() else {
            return;
        };
        if self.store.active == a.id {
            self.flash(format!("{} is already active", a.name), crate::theme::MUTED);
            return;
        }
        if !self.update(|s| s.active = a.id.clone()) {
            return;
        }
        self.flash(
            format!("New sessions use {} (running ones keep theirs)", a.name),
            crate::theme::ACCENT,
        );
    }

    fn open_add(&mut self) {
        let items = vec![
            PickItem {
                label: "Claude subscription".into(),
                detail: "Pro, Max, Ultra, Team or Enterprise; signs in in the browser".into(),
                value: None,
            },
            PickItem {
                label: "Claude subscription with SSO".into(),
                detail: "Team or Enterprise through your organisation's single sign-on".into(),
                value: None,
            },
            PickItem {
                label: "API key (Claude Console)".into(),
                detail: "A key from platform.claude.com; kept in the keychain".into(),
                value: None,
            },
        ];
        self.mode = Mode::Pick(Picker {
            title: "Add an account".into(),
            items,
            sel: 0,
            purpose: PickFor::AddKind,
        });
    }

    fn open_space_picker(&mut self) {
        let Some(ws) = self.workspaces.get(self.space_sel).cloned() else {
            return;
        };
        let current = self.store.space_account(&ws.id).map(str::to_string);
        let mut items = vec![PickItem {
            label: "The active account".into(),
            detail: format!("follows it, now {}", self.store.name_of(&self.store.active)),
            value: None,
        }];
        for a in &self.store.accounts {
            let detail = self
                .info
                .get(&a.id)
                .and_then(|i| i.plan.clone())
                .unwrap_or_default();
            items.push(PickItem {
                label: a.name.clone(),
                detail,
                value: Some(a.id.clone()),
            });
        }
        let sel = items.iter().position(|i| i.value == current).unwrap_or(0);
        self.mode = Mode::Pick(Picker {
            title: format!("Account for {}", ws.label),
            items,
            sel,
            purpose: PickFor::Space {
                workspace: ws.id,
                label: ws.label,
            },
        });
    }

    fn picked(&mut self, p: Picker) {
        let Some(item) = p.items.get(p.sel).cloned() else {
            self.mode = Mode::Normal;
            return;
        };
        match p.purpose {
            PickFor::AddKind => {
                let kind = match p.sel {
                    0 => AddKind::Subscription,
                    1 => AddKind::SubscriptionSso,
                    _ => AddKind::ApiKey,
                };
                if kind != AddKind::ApiKey && !self.subscription_allowed() {
                    return;
                }
                let suggestion = match kind {
                    AddKind::ApiKey => "API key",
                    _ => "Personal",
                };
                self.mode = Mode::Input(Input {
                    title: "Name of the account".into(),
                    hint: match kind {
                        AddKind::ApiKey => {
                            "Next you paste the key; it goes straight into the keychain."
                        }
                        _ => "Next Claude Code opens the browser: sign in with this account.",
                    }
                    .into(),
                    text: suggestion.into(),
                    purpose: InputFor::NewAccount(kind),
                    secret: false,
                });
            }
            PickFor::MoveSession(session) => {
                if let Some(id) = item.value.as_deref() {
                    self.ask_move_to(session, id);
                }
            }
            PickFor::Space { workspace, label } => {
                self.mode = Mode::Normal;
                if !self.update(|s| s.assign(&workspace, &label, item.value.as_deref())) {
                    return;
                }
                let name = item.value.as_deref().map(|id| self.store.name_of(id));
                herdr::tag_workspace(&workspace, name.as_deref());
                self.mode = Mode::Normal;
                self.flash(
                    match &name {
                        Some(n) => format!("New sessions in {label} use {n}"),
                        None => format!("{label} follows the active account"),
                    },
                    crate::theme::ACCENT,
                );
            }
        }
    }

    fn entered(&mut self, input: Input) {
        if let InputFor::ApiKey { id, name } = input.purpose {
            self.store_key(id, name, input.text.trim().to_string());
            return;
        }
        let name = input.text.trim().to_string();
        if name.is_empty() {
            return;
        }
        if self.store.accounts.iter().any(|a| {
            a.name.eq_ignore_ascii_case(&name) && InputFor::Rename(a.id.clone()) != input.purpose
        }) {
            self.flash("An account with that name exists", crate::theme::RED);
            return;
        }
        self.mode = Mode::Normal;
        match input.purpose {
            InputFor::Rename(id) => {
                let new_name = name.clone();
                if !self.update(|s| {
                    if let Some(a) = s.accounts.iter_mut().find(|a| a.id == id) {
                        a.name = new_name;
                    }
                }) {
                    return;
                }
                for s in self.store.spaces.clone().iter().filter(|s| s.account == id) {
                    herdr::tag_workspace(&s.workspace_id, Some(&name));
                }
                self.flash(format!("Renamed to {name}"), crate::theme::ACCENT);
            }
            InputFor::NewAccount(AddKind::ApiKey) => {
                // The account is added once its key is stored.
                self.mode = key_input(None, name);
            }
            InputFor::NewAccount(kind) => {
                let mut account = Account {
                    id: String::new(),
                    name,
                    kind: Kind::Subscription,
                    sso: kind == AddKind::SubscriptionSso,
                    fallback: true,
                };
                let mut added = None;
                if !self.update(|s| {
                    account.id = s.new_id(&account.name);
                    added = Some(account.clone());
                    s.accounts.push(account);
                }) {
                    return;
                }
                if let Some(a) = added {
                    self.sel = self.store.accounts.len() - 1;
                    self.start_sign_in(&a, true);
                }
            }
            InputFor::ApiKey { .. } => {}
        }
    }

    /// Stores a pasted key, adding its account first when it is new. A key
    /// the keychain does not take leaves no account behind.
    fn store_key(&mut self, id: Option<String>, name: String, key: String) {
        if let Err(e) = secret::plausible(&key) {
            self.flash(format!("Not stored: {e}"), crate::theme::RED);
            return;
        }
        self.mode = Mode::Normal;
        let (id, added) = match id {
            Some(id) => (id, false),
            None => {
                let mut new_id = String::new();
                let account_name = name.clone();
                if !self.update(|s| {
                    new_id = s.new_id(&account_name);
                    s.accounts.push(Account {
                        id: new_id.clone(),
                        name: account_name,
                        kind: Kind::ApiKey,
                        sso: false,
                        fallback: true,
                    });
                }) {
                    return;
                }
                self.sel = self.store.accounts.len() - 1;
                (new_id, true)
            }
        };
        match secret::store(&id, &name, &key) {
            Ok(()) => {
                self.flash(
                    format!("The key for {name} is in the keychain"),
                    crate::theme::GREEN,
                );
                self.start_info();
            }
            Err(e) => {
                if added {
                    self.update(|s| s.accounts.retain(|a| a.id != id));
                }
                self.flash(format!("Not stored: {e}"), crate::theme::RED);
            }
        }
    }

    /// A claude.ai sign-in signs out the machine-wide Console profile, so
    /// none starts while the main login is one.
    fn subscription_allowed(&mut self) -> bool {
        if !info::main_uses_profile() {
            return true;
        }
        self.mode = Mode::Message {
            title: "First change the main login".into(),
            lines: vec![
                "Your main login is a Claude Console sign-in without an API key. Claude Code keeps that one for the whole computer, and signing in any claude.ai account (Pro, Max, Team, Enterprise) signs it out.".into(),
                "Sign the main login in again with an API key: in a terminal run claude auth login --console and choose \"Create an API key\". Then add the subscription here.".into(),
            ],
            good: false,
        };
        false
    }

    fn start_sign_in(&mut self, account: &Account, new_account: bool) {
        // Whatever the sign-in ends in, the next start checks live.
        self.forget_check(&account.id);
        match (self.sign_in_starter)(account, new_account) {
            Ok(s) => {
                self.signin = Some(s);
                self.mode = Mode::SignIn {
                    code: String::new(),
                };
            }
            Err(e) => {
                if new_account {
                    self.drop_new_account(&account.id);
                }
                self.mode = Mode::Normal;
                self.flash(
                    format!("Could not start the sign-in: {e}"),
                    crate::theme::RED,
                );
            }
        }
    }

    /// Removes an account that was added for a sign-in that did not finish.
    /// The sign-in may have stored a login before it was stopped, so the
    /// folder is signed out first, and its id is never used again.
    fn drop_new_account(&mut self, id: &str) {
        self.forget_check(id);
        let failed = match self.store.account(id).cloned() {
            Some(a) if a.kind == Kind::Subscription => (self.sign_out)(&a),
            _ => vec![],
        };
        let _ = (self.folder_remover)(id);
        self.update(|s| {
            s.accounts.retain(|a| a.id != id);
            if !s.retired.iter().any(|r| r == id) {
                s.retired.push(id.to_string());
            }
        });
        if !failed.is_empty() {
            self.mode = sign_out_failed(&failed);
        }
    }

    /// Follows a running sign-in and shows how it ended.
    fn poll_sign_in(&mut self) {
        let Some(s) = self.signin.as_mut() else {
            return;
        };
        s.poll();
        let Some(ok) = s.finished else {
            return;
        };
        let s = self.signin.take().expect("checked above");
        let account = self.store.account(&s.id).cloned();
        let was_old = self.info.get(&s.id).is_some_and(|i| i.old_folder);
        let info = account.as_ref().map(info::read).unwrap_or_default();
        if ok && info.own_login() {
            // From now on the account stays on its new folder.
            mirror::mark_moved(&mirror::home_of(&s.id));
            let who = info
                .email
                .clone()
                .or(info.organization.clone())
                .unwrap_or_else(|| "an account".into());
            let mut lines = vec![format!(
                "Signed in as {who}{}.",
                info.plan
                    .as_deref()
                    .map(|p| format!(" ({p})"))
                    .unwrap_or_default()
            )];
            // The browser often signs in with whatever account it already
            // has open.
            let same = account
                .as_ref()
                .map(|a| crate::login::same_as(&self.store, a))
                .unwrap_or_default();
            if !same.is_empty() {
                lines.push(crate::login::same_login_note(&same));
            }
            if was_old && !info.old_folder {
                lines.push(
                    "New sessions on it use its new folder, where Claude no longer asks before every edit in ~/.claude. Sessions already running keep their old folder until they end.".into(),
                );
            }
            self.info.insert(s.id.clone(), info);
            self.mode = Mode::Message {
                title: format!("{} is signed in", s.name),
                lines,
                good: same.is_empty(),
            };
        } else {
            if s.new_account {
                self.drop_new_account(&s.id);
            }
            let reason = if s.code_sent || !s.last_line().is_empty() {
                s.last_line()
            } else {
                String::new()
            };
            self.mode = Mode::Normal;
            self.flash(
                if reason.is_empty() || reason.to_lowercase().contains("paste code") {
                    format!("{}: sign-in cancelled", s.name)
                } else {
                    format!("{}: not signed in ({reason})", s.name)
                },
                crate::theme::RED,
            );
        }
        self.start_info();
    }

    fn sign_in_again(&mut self) {
        let Some(a) = self.selected().cloned() else {
            return;
        };
        match a.kind {
            Kind::Main => {
                self.mode = Mode::Message {
                    title: "The main login is Claude Code's own".into(),
                    lines: vec![
                        "Sign it in again inside Claude Code with /login, or in a terminal with claude auth login.".into(),
                        "For a Claude Console account choose \"Create an API key\": a Console sign-in without a key is kept for the whole computer, and it and the claude.ai accounts here sign each other out.".into(),
                    ],
                    good: true,
                };
            }
            Kind::ApiKey => self.mode = key_input(Some(a.id), a.name),
            Kind::Subscription => {
                if self.subscription_allowed() {
                    self.start_sign_in(&a, false);
                }
            }
        }
    }

    fn ask_remove(&mut self) {
        let Some(a) = self.selected().cloned() else {
            return;
        };
        if a.kind == Kind::Main {
            self.flash(
                "The main login stays; sign it out in Claude Code with /logout",
                crate::theme::MUTED,
            );
            return;
        }
        let what = match a.kind {
            Kind::ApiKey => "Its key is deleted from the keychain.",
            _ => "It is signed out and its folder of links is deleted.",
        };
        let mut lines = vec![
            what.to_string(),
            "Sessions, memory and settings in ~/.claude are not touched.".to_string(),
        ];
        let running = self.running(&a.id);
        if running > 0 {
            lines.push(format!(
                "{running} Claude Code session(s) run on it now and stop working once it is gone: end them first."
            ));
        }
        let own = mirror::own_data(&a.id);
        if !own.is_empty() {
            lines.push(format!(
                "Only in its folder, moved to backups: {}.",
                own.join(", ")
            ));
        }
        self.mode = Mode::Confirm(Confirm {
            question: format!("Remove {}?", a.name),
            lines,
            purpose: ConfirmFor::Remove(a.id),
        });
    }

    fn remove(&mut self, id: &str) {
        let Some(a) = self.store.account(id).cloned() else {
            return;
        };
        let mut still_signed_in = vec![];
        match a.kind {
            Kind::Main => return,
            Kind::ApiKey => {
                secret::delete(id);
                if secret::exists(id) {
                    self.mode = Mode::Message {
                        title: format!("{} was not removed", a.name),
                        lines: vec![
                            "The keychain did not delete its API key, so the account stays in the list.".into(),
                            format!(
                                "Delete the item \"{id}\" (service herdr-multiaccount) in Keychain Access, then remove the account again."
                            ),
                        ],
                        good: false,
                    };
                    return;
                }
            }
            Kind::Subscription => {
                self.forget_check(id);
                still_signed_in = (self.sign_out)(&a);
                match (self.folder_remover)(id) {
                    Ok(kept) if !kept.is_empty() => {
                        self.flash(
                            format!("Removed {}; kept in backups: {}", a.name, kept.join(", ")),
                            crate::theme::ACCENT,
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.flash(
                            format!("Could not delete its folder: {e}"),
                            crate::theme::RED,
                        );
                        return;
                    }
                }
            }
        }
        for s in self.store.spaces.clone().iter().filter(|s| s.account == id) {
            herdr::tag_workspace(&s.workspace_id, None);
        }
        if !self.update(|s| {
            s.accounts.retain(|x| x.id != id);
            s.spaces.retain(|sp| sp.account != id);
            if s.active == id {
                s.active = MAIN_ID.into();
            }
            if !s.retired.iter().any(|r| r == id) {
                s.retired.push(id.to_string());
            }
        }) {
            return;
        }
        let _ = std::fs::remove_file(paths::usage_dir().join(format!("{id}.json")));
        self.info.remove(id);
        if !still_signed_in.is_empty() {
            // Removed anyway: its id is never used again, so no new account
            // can find the login. Saying so keeps the login from being
            // forgotten.
            self.mode = sign_out_failed(&still_signed_in);
        } else if self.flash.is_none() {
            self.flash(format!("Removed {}", a.name), crate::theme::ACCENT);
        }
    }

    fn toggle_account_fallback(&mut self) {
        let Some(a) = self.selected().cloned() else {
            return;
        };
        if !self.update(|s| {
            if let Some(x) = s.accounts.iter_mut().find(|x| x.id == a.id) {
                x.fallback = !a.fallback;
            }
        }) {
            return;
        }
        let msg = if !a.fallback {
            format!("{} takes part in fallback", a.name)
        } else {
            format!("Fallback skips {}", a.name)
        };
        self.flash(msg, crate::theme::ACCENT);
    }

    fn toggle_fallback(&mut self) {
        self.config.fallback = !self.config.fallback;
        let _ = config::save_value(
            &self.dir,
            "fallback",
            if self.config.fallback {
                "true"
            } else {
                "false"
            },
        );
        self.flash(
            if self.config.fallback {
                format!(
                    "Fallback on: from {:.0}% new sessions move to the next account",
                    self.config.threshold
                )
            } else {
                "Fallback off".into()
            },
            crate::theme::ACCENT,
        );
    }

    fn move_selected(&mut self, by: i32) {
        let Some(id) = self.selected().map(|a| a.id.clone()) else {
            return;
        };
        // `update` keeps the selection on the moved account.
        self.update(|s| {
            s.move_account(&id, by);
        });
    }

    fn start_claude(&mut self) {
        let Some(a) = self.selected().cloned() else {
            return;
        };
        if !self.in_herdr {
            self.flash("Only inside Herdr", crate::theme::MUTED);
            return;
        }
        if !self.setup.shell {
            self.flash(
                "Set up first (S): the claude function picks the account",
                crate::theme::YELLOW,
            );
            return;
        }
        let ws = std::env::var("HERDR_WORKSPACE_ID").ok();
        let mut args: Vec<String> = vec!["tab".into(), "create".into()];
        if let Some(w) = &ws {
            args.extend(["--workspace".into(), w.clone()]);
        }
        if let Some(c) = &self.cwd {
            args.extend(["--cwd".into(), c.clone()]);
        }
        args.push("--focus".into());
        let out = std::process::Command::new(herdr::bin())
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let pane = out
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
            .and_then(|v| {
                v["result"]["root_pane"]["pane_id"]
                    .as_str()
                    .map(str::to_string)
            });
        match pane {
            Some(p) => {
                let _ = std::process::Command::new(herdr::bin())
                    // Only for this one start (ids are letters, digits and
                    // dashes), not for the tab: later sessions there follow
                    // the space and fallback again.
                    .args([
                        "pane",
                        "run",
                        &p,
                        &format!("{}={} claude", launch::ENV_USE, a.id),
                    ])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                self.flash(
                    format!("Started Claude Code with {} in a new tab", a.name),
                    crate::theme::ACCENT,
                );
            }
            None => self.flash("Herdr did not open a tab", crate::theme::RED),
        }
    }

    fn ask_setup(&mut self) {
        let lines = setup::plan();
        if lines.is_empty() {
            self.flash("Already set up", crate::theme::MUTED);
            return;
        }
        let mut lines = lines;
        lines.push(
            "Both are backed up first; `herdr-multiaccount setup --undo` reverts them.".into(),
        );
        self.mode = Mode::Confirm(Confirm {
            question: "Set up the accounts?".into(),
            lines,
            purpose: ConfirmFor::Setup,
        });
    }

    /// Says that a move the user turned back did not run.
    fn not_moved(&mut self, label: &str) {
        self.flash(format!("{label} was not moved"), crate::theme::MUTED);
    }

    fn confirmed(&mut self, c: Confirm) {
        self.mode = Mode::Normal;
        match c.purpose {
            ConfirmFor::Remove(id) => self.remove(&id),
            ConfirmFor::EndSession { pane, label } => self.end_session(&pane, &label),
            ConfirmFor::MoveSession { session, to } => self.move_session(session, &to),
            ConfirmFor::CloseSpace {
                spaces,
                label,
                sessions,
                warned,
            } => self.close_space(&spaces, &label, &sessions, &warned),
            ConfirmFor::Setup => {
                match setup::apply() {
                    Ok(msg) => self.flash(msg, crate::theme::GREEN),
                    Err(e) => self.flash(e, crate::theme::RED),
                }
                self.setup = setup::state();
                self.store = Store::load();
            }
        }
    }

    fn toggle_icons(&mut self) {
        self.config.nerd_font = !self.config.nerd_font;
        let _ = config::save_value(
            &self.dir,
            "icons",
            if self.config.nerd_font {
                "\"nerd\""
            } else {
                "\"plain\""
            },
        );
    }

    // ---- input ------------------------------------------------------

    fn rows_len(&self) -> usize {
        match self.section {
            Section::Accounts => self.store.accounts.len(),
            Section::Spaces => self.workspaces.len(),
            Section::Sessions => self.sessions().len(),
        }
    }

    fn cursor(&mut self) -> &mut usize {
        match self.section {
            Section::Accounts => &mut self.sel,
            Section::Spaces => &mut self.space_sel,
            Section::Sessions => &mut self.session_sel,
        }
    }

    fn step(&mut self, by: i32) {
        let len = self.rows_len();
        if len == 0 {
            return;
        }
        let c = self.cursor();
        *c = (*c as i32 + by).clamp(0, len as i32 - 1) as usize;
    }

    fn switch(&mut self, section: Section) {
        self.section = section;
        if section != Section::Accounts {
            self.start_herdr();
        }
    }

    fn enter(&mut self) {
        match self.section {
            Section::Accounts => self.make_active(),
            Section::Spaces => self.open_space_picker(),
            Section::Sessions => self.go_to_session(),
        }
    }

    pub fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            if self.mode != Mode::AskQuit {
                self.quit_over = (self.mode != Mode::Normal).then(|| self.mode.clone());
            }
            self.mode = Mode::AskQuit;
            return;
        }
        match self.mode.clone() {
            Mode::AskQuit => {
                // Staying goes back to the dialog it was asked over, so a
                // question or a move's account list never vanishes unsaid.
                self.mode = self.quit_over.take().unwrap_or(Mode::Normal);
                // A move goes on in its own process (the owner's decision
                // on #12).
                if k.code == KeyCode::Char('y') {
                    self.quit = true;
                }
            }
            Mode::Help => match k.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter => {
                    self.mode = Mode::Normal;
                }
                KeyCode::Up | KeyCode::Char('k') => self.scroll_help(-1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll_help(1),
                KeyCode::PageUp => self.scroll_help(-10),
                KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_help(10),
                KeyCode::Home | KeyCode::Char('g') => self.help_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.help_scroll = self.help_max,
                _ => {}
            },
            // `y` answers yes, `n` and Esc no; Enter is yes only on the
            // move question, which follows the Enter that picked the
            // account (the owner's decision on #10). Any other key leaves
            // the question open: a stray key never answers.
            // Ctrl+Y, Alt+N and the like are other keys too.
            Mode::Confirm(_)
                if k.modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {}
            Mode::Confirm(c) => match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.scroll_confirm(-1, c),
                KeyCode::Down | KeyCode::Char('j') => self.scroll_confirm(1, c),
                KeyCode::Char('y') => {
                    self.confirm_of.clear();
                    self.confirmed(c)
                }
                KeyCode::Enter if c.enter_says_yes() => {
                    self.confirm_of.clear();
                    self.confirmed(c)
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.confirm_of.clear();
                    self.mode = Mode::Normal;
                    if let ConfirmFor::MoveSession { session, .. } = &c.purpose {
                        self.not_moved(&session.label);
                    }
                }
                _ => {}
            },
            Mode::Pick(mut p) => match k.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                    if let PickFor::MoveSession(session) = &p.purpose {
                        self.not_moved(&session.label);
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    p.sel = p.sel.saturating_sub(1);
                    self.mode = Mode::Pick(p);
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    p.sel = (p.sel + 1).min(p.items.len().saturating_sub(1));
                    self.mode = Mode::Pick(p);
                }
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    let i = c as usize - '1' as usize;
                    if i < p.items.len() {
                        p.sel = i;
                        self.picked(p);
                    }
                }
                KeyCode::Enter | KeyCode::Char(' ') => self.picked(p),
                _ => {}
            },
            Mode::Input(mut input) => match k.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Enter => self.entered(input),
                KeyCode::Backspace => {
                    input.text.pop();
                    self.mode = Mode::Input(input);
                }
                KeyCode::Char('u') if ctrl => {
                    input.text.clear();
                    self.mode = Mode::Input(input);
                }
                KeyCode::Char(c) if !ctrl && input.text.chars().count() < input.max_len() => {
                    input.text.push(c);
                    self.mode = Mode::Input(input);
                }
                _ => {}
            },
            Mode::Message { .. } => {
                if matches!(
                    k.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char(' ')
                ) {
                    self.mode = Mode::Normal;
                }
            }
            Mode::SignIn { mut code } => match k.code {
                KeyCode::Esc => {
                    if let Some(s) = self.signin.as_mut() {
                        s.cancel();
                    }
                }
                KeyCode::Enter if !code.trim().is_empty() => {
                    if let Some(s) = self.signin.as_mut() {
                        s.send_code(&code);
                    }
                    self.mode = Mode::SignIn {
                        code: String::new(),
                    };
                }
                KeyCode::Backspace => {
                    code.pop();
                    self.mode = Mode::SignIn { code };
                }
                KeyCode::Char('o') if code.is_empty() => {
                    if let Some(url) = self.signin.as_ref().and_then(|s| s.url.clone()) {
                        if !(self.opener)(OPENER, &url) {
                            self.flash(format!("Could not run {OPENER}"), crate::theme::RED);
                        }
                    }
                }
                KeyCode::Char('y') if code.is_empty() => {
                    if let Some(url) = self.signin.as_ref().and_then(|s| s.url.clone()) {
                        if copy(&url) {
                            self.flash("Link copied", crate::theme::GREEN);
                        }
                    }
                }
                KeyCode::Char(c) if !ctrl && code.len() < 300 => {
                    code.push(c);
                    self.mode = Mode::SignIn { code };
                }
                _ => {}
            },
            Mode::Normal => self.normal_key(k),
        }
    }

    fn normal_key(&mut self, k: KeyEvent) {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('q') => {
                self.quit_over = None;
                self.mode = Mode::AskQuit
            }
            KeyCode::Esc => {}
            KeyCode::Char('?') => {
                self.help_scroll = 0;
                self.mode = Mode::Help;
            }
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Char('g') | KeyCode::Home => *self.cursor() = 0,
            KeyCode::Char('G') | KeyCode::End => {
                let last = self.rows_len().saturating_sub(1);
                *self.cursor() = last;
            }
            KeyCode::Tab | KeyCode::Right => self.switch(self.section.next(1)),
            KeyCode::BackTab | KeyCode::Left => self.switch(self.section.next(-1)),
            KeyCode::Char('1') => self.switch(Section::Accounts),
            KeyCode::Char('2') => self.switch(Section::Spaces),
            KeyCode::Char('3') => self.switch(Section::Sessions),
            KeyCode::Enter | KeyCode::Char(' ') => self.enter(),
            KeyCode::Char('r') => {
                self.refresh_all();
                self.flash("Refreshing", crate::theme::MUTED);
            }
            KeyCode::Char('F') => self.toggle_fallback(),
            KeyCode::Char('S') => self.ask_setup(),
            KeyCode::Char('i') => self.toggle_icons(),
            KeyCode::Char('a') => self.open_add(),
            _ if self.section == Section::Accounts => match k.code {
                KeyCode::Char('l') => self.sign_in_again(),
                KeyCode::Char('n') => {
                    if let Some(a) = self.selected().cloned() {
                        self.mode = Mode::Input(Input {
                            title: format!("New name for {}", a.name),
                            hint: "Only the name changes; its login stays.".into(),
                            text: a.name.clone(),
                            purpose: InputFor::Rename(a.id),
                            secret: false,
                        });
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => self.ask_remove(),
                KeyCode::Char('f') => self.toggle_account_fallback(),
                KeyCode::Char('K') => self.move_selected(-1),
                KeyCode::Char('J') => self.move_selected(1),
                KeyCode::Up if shift => self.move_selected(-1),
                KeyCode::Down if shift => self.move_selected(1),
                KeyCode::Char('c') => self.start_claude(),
                _ => {}
            },
            _ if self.section == Section::Sessions => match k.code {
                KeyCode::Char('d') | KeyCode::Delete => {
                    if let Some(m) = self.moving.clone() {
                        self.flash(
                            format!("{} is being moved; wait a moment", m.label),
                            crate::theme::MUTED,
                        );
                    } else {
                        self.ask_end_session();
                    }
                }
                KeyCode::Char('m') => self.ask_move_session(),
                _ => {}
            },
            _ if self.section == Section::Spaces => match k.code {
                // `d` only: Delete keeps "back to the active account" (the
                // owner's decision on #11).
                KeyCode::Char('d') => {
                    if let Some(m) = self.moving.clone() {
                        self.flash(
                            format!("{} is being moved; wait a moment", m.label),
                            crate::theme::MUTED,
                        );
                    } else {
                        self.ask_close_space();
                    }
                }
                KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete => {
                    if let Some(ws) = self.workspaces.get(self.space_sel).cloned() {
                        if !self.update(|s| s.assign(&ws.id, &ws.label, None)) {
                            return;
                        }
                        herdr::tag_workspace(&ws.id, None);
                        self.flash(
                            format!("{} follows the active account", ws.label),
                            crate::theme::ACCENT,
                        );
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn click(&mut self, x: u16, y: u16) {
        let Some(hit) = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|(_, h)| *h)
        else {
            return;
        };
        let double = self
            .last_click
            .is_some_and(|(at, h)| h == hit && at.elapsed() < Duration::from_millis(400));
        self.last_click = Some((Instant::now(), hit));
        match hit {
            Hit::Section(s) => {
                if matches!(self.mode, Mode::Normal) {
                    self.switch(s);
                }
            }
            Hit::Row(i) => {
                if matches!(self.mode, Mode::Normal) {
                    *self.cursor() = i;
                    if double {
                        self.enter();
                    }
                }
            }
            Hit::PickRow(i) => {
                if let Mode::Pick(mut p) = self.mode.clone() {
                    p.sel = i;
                    if double {
                        self.picked(p);
                    } else {
                        self.mode = Mode::Pick(p);
                    }
                }
            }
            Hit::Key(c) => {
                let code = match c {
                    '\n' => KeyCode::Enter,
                    '\u{1b}' => KeyCode::Esc,
                    c => KeyCode::Char(c),
                };
                self.key(KeyEvent::new(code, KeyModifiers::NONE));
            }
            Hit::Link => {
                self.mode = Mode::Normal;
                if (self.opener)(OPENER, env!("CARGO_PKG_REPOSITORY")) {
                    self.flash("Opened in the browser", crate::theme::GREEN);
                } else {
                    self.flash(format!("Could not run {OPENER}"), crate::theme::RED);
                }
            }
        }
    }

    pub fn event(&mut self, e: Event) {
        match e {
            Event::Key(k) => self.key(k),
            Event::Paste(text) => {
                let clean: String = text
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect::<String>()
                    .trim()
                    .to_string();
                match self.mode.clone() {
                    Mode::Input(mut input) => {
                        input.text.push_str(&clean);
                        input.text = input.text.chars().take(input.max_len()).collect();
                        self.mode = Mode::Input(input);
                    }
                    Mode::SignIn { mut code } => {
                        code.push_str(&clean);
                        code = code.chars().take(300).collect();
                        self.mode = Mode::SignIn { code };
                    }
                    _ => {}
                }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => self.click(m.column, m.row),
                MouseEventKind::ScrollDown if self.mode == Mode::Help => self.scroll_help(2),
                MouseEventKind::ScrollUp if self.mode == Mode::Help => self.scroll_help(-2),
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                    if matches!(self.mode, Mode::Confirm(_)) =>
                {
                    if let Mode::Confirm(c) = self.mode.clone() {
                        let by = if m.kind == MouseEventKind::ScrollDown {
                            1
                        } else {
                            -1
                        };
                        self.scroll_confirm(by, c);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Mode::Pick(mut p) = self.mode.clone() {
                        p.sel = (p.sel + 1).min(p.items.len().saturating_sub(1));
                        self.mode = Mode::Pick(p);
                    } else if matches!(self.mode, Mode::Normal) {
                        self.step(1);
                    }
                }
                MouseEventKind::ScrollUp => {
                    if let Mode::Pick(mut p) = self.mode.clone() {
                        p.sel = p.sel.saturating_sub(1);
                        self.mode = Mode::Pick(p);
                    } else if matches!(self.mode, Mode::Normal) {
                        self.step(-1);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Scrolls a question taller than the pane; it stays open.
    fn scroll_confirm(&mut self, by: i32, c: Confirm) {
        self.confirm_scroll =
            (self.confirm_scroll as i32 + by).clamp(0, self.confirm_max as i32) as u16;
        self.mode = Mode::Confirm(c);
    }

    fn scroll_help(&mut self, by: i32) {
        self.help_scroll = (self.help_scroll as i32 + by).clamp(0, self.help_max as i32) as u16;
    }

    /// The note on how the list works, once, on first open.
    fn show_intro(&mut self) {
        if self.store.intro_seen || !matches!(self.mode, Mode::Normal) {
            return;
        }
        if self.update(|s| s.intro_seen = true) {
            self.mode = Mode::Message {
                title: "How this list works".into(),
                lines: INTRO.iter().map(|l| l.to_string()).collect(),
                good: true,
            };
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        self.refresh_all();
        self.show_intro();
        while !self.quit {
            self.receive_herdr();
            self.receive_info();
            self.receive_probe();
            self.receive_move();
            if Instant::now() >= self.next_probe {
                self.start_probe();
            }
            if Instant::now() >= self.next_files {
                self.reload_files();
            }
            if Instant::now() >= self.next_herdr {
                self.start_herdr();
            }
            if Instant::now() >= self.next_info && self.info_rx.is_none() {
                self.start_info();
            }
            if self
                .flash
                .as_ref()
                .is_some_and(|(_, _, until)| Instant::now() >= *until)
            {
                self.flash = None;
            }
            self.poll_sign_in();
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
            if event::poll(Duration::from_millis(250))? {
                self.event(event::read()?);
            }
        }
        Ok(())
    }
}

/// The conversation the status line noted for a pane, while its Claude
/// Code still runs in front there: its session id and process.
pub fn real_conversation(pane: &str) -> Option<Held> {
    let note = crate::sessions::conversation(pane)?;
    let pid = crate::sessions::front_process(&note, &herdr::process_info(pane)?)?;
    Some(held_from(note, pid, crate::sessions::get(pane)))
}

/// What a move carries: the model from the status line's note, the start
/// options from the launcher's note, which counts only for the same Claude
/// Code process (the launcher execs, so the pid stays).
fn held_from(
    note: crate::sessions::Conversation,
    pid: u32,
    launcher: Option<crate::sessions::Entry>,
) -> Held {
    let launcher = launcher.filter(|e| e.pid == pid);
    Held {
        session_id: note.session_id,
        pid,
        carry: Moved {
            options: launcher.as_ref().and_then(|e| e.options.clone()),
            dropped: launcher.map(|e| e.dropped).unwrap_or_default(),
            model: note.model,
        },
    }
}

/// Starts `opener` (`open`, `xdg-open`) on `url`. It is waited on in the
/// background, so it leaves no ended process behind and the list never waits
/// for an opener that keeps running.
pub fn open_in_browser(opener: &str, url: &str) -> bool {
    let started = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match started {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Puts text on the clipboard (macOS, or Wayland / X11 tools when there).
fn copy(text: &str) -> bool {
    let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    };
    for (tool, args) in tools {
        if let Ok(mut child) = std::process::Command::new(tool)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().is_ok_and(|s| s.success());
        }
    }
    false
}

/// The folders of an account that may hold a login: its folder, and the
/// old folder an older version signed in (it has a config file of its own).
fn login_folders(home: &std::path::Path) -> Vec<PathBuf> {
    // A real folder only: an old link of that name leads into ~/.claude.
    let mut out: Vec<PathBuf> = mirror::real_new_dir(home).into_iter().collect();
    if home.join(".claude.json").exists() || home.join(".credentials.json").exists() {
        out.push(home.to_path_buf());
    }
    out
}

/// Signs every folder of an account out of its own claude.ai login, and
/// checks that it worked. A folder without a login of its own (or one that
/// only borrows the main login, which must not be signed out) needs
/// nothing. Returns the folders that still have a login afterwards.
fn sign_out(a: &Account) -> Vec<PathBuf> {
    login_folders(&mirror::home_of(&a.id))
        .into_iter()
        .filter(|dir| !sign_out_folder(dir))
        .collect()
}

fn sign_out_folder(dir: &std::path::Path) -> bool {
    let own_login = |dir: &std::path::Path| {
        info::parse_status(&info::status_text(Some(dir)).unwrap_or_default()).own_login()
    };
    if !own_login(dir) {
        return true;
    }
    let Some(claude) = launch::find_claude() else {
        return false;
    };
    let mut cmd = std::process::Command::new(claude);
    // Clean first: it may remove CLAUDE_CONFIG_DIR, and without it the main
    // login would be signed out.
    launch::clean_env(&mut cmd);
    cmd.args(["auth", "logout"])
        .env("CLAUDE_CONFIG_DIR", dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.status();
    !own_login(dir)
}

/// The notice when a removed account's login could not be signed out, with
/// the command that does it by hand for each folder still signed in.
fn sign_out_failed(dirs: &[PathBuf]) -> Mode {
    let mut lines = vec![
        "Claude Code may still keep this account's login. The plugin never reuses its folder, so no other account gets it.".into(),
        "To remove the login, run in a terminal:".into(),
    ];
    for dir in dirs {
        lines.push(format!(
            "CLAUDE_CONFIG_DIR=\"{}\" command claude auth logout",
            dir.display()
        ));
    }
    Mode::Message {
        title: "Sign-out did not work".into(),
        lines,
        good: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// A folder of its own for each test's notes of moves.
    fn moves_dir() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("hm-moving-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    thread_local! {
        /// What the test `accounts.json` holds, per test thread.
        static STORED: std::cell::RefCell<Store> = std::cell::RefCell::new(Store::default());
    }

    /// `Store::update` on the test's own store, never the real file.
    fn stores(change: Box<dyn FnOnce(&mut Store) + '_>) -> std::io::Result<Store> {
        STORED.with(|s| {
            let mut s = s.borrow_mut();
            change(&mut s);
            Ok(s.clone())
        })
    }

    fn cannot_store(_: Box<dyn FnOnce(&mut Store) + '_>) -> std::io::Result<Store> {
        Err(std::io::Error::other("disk full"))
    }

    thread_local! {
        static STORE_BROKEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// `stores`, until a test breaks the disk.
    fn stores_until_broken(change: Box<dyn FnOnce(&mut Store) + '_>) -> std::io::Result<Store> {
        if STORE_BROKEN.with(|b| b.get()) {
            cannot_store(change)
        } else {
            stores(change)
        }
    }

    fn stored() -> Store {
        STORED.with(|s| s.borrow().clone())
    }

    fn app() -> App {
        let mut a = App::new(Config::default(), std::env::temp_dir());
        a.moves_dir = moves_dir();
        a.own_pane = None;
        a.store_update = stores;
        a.space_closer = |_, _| panic!("a test closed a space without a fake closer");
        a.checks_dir = checks_dir();
        a.sign_out = |_| panic!("a test signed out without a fake");
        a.folder_remover = |_| panic!("a test removed a folder without a fake");
        a.sign_in_starter = |_, _| Err("no sign-in in tests".into());
        a.store = Store::default();
        a.store.accounts.push(Account {
            id: "personal".into(),
            name: "Personal".into(),
            kind: Kind::Subscription,
            sso: false,
            fallback: true,
        });
        a.usage = HashMap::new();
        a.twins = HashMap::new();
        a.workspaces = vec![Workspace {
            id: "w1".into(),
            label: "acme-app".into(),
            number: 1,
            focused: true,
            worktree: None,
        }];
        STORED.with(|s| *s.borrow_mut() = a.store.clone());
        a
    }

    #[test]
    fn keys_move_and_switch() {
        let mut a = app();
        a.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(a.sel, 1);
        a.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(a.sel, 1, "stays on the last row");
        a.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(a.section, Section::Spaces);
        a.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(a.section, Section::Sessions, "→ goes to the next tab");
        a.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        a.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(a.section, Section::Accounts, "← goes back");
        a.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        a.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!a.quit, "Esc never closes the list");
        a.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(a.mode, Mode::AskQuit);
        a.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(!a.quit);
        a.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        a.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(a.quit);
    }

    #[test]
    fn add_flow_reaches_name_input() {
        let mut a = app();
        a.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(a.mode, Mode::Pick(_)));
        a.key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        match &a.mode {
            Mode::Input(i) => assert_eq!(i.purpose, InputFor::NewAccount(AddKind::ApiKey)),
            m => panic!("unexpected {m:?}"),
        }
    }

    /// A folder of its own for each test's noted sign-in checks.
    fn checks_dir() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("hm-signin-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn signed_in_info() -> Info {
        Info {
            signed_in: true,
            claude_ai: true,
            ..Info::default()
        }
    }

    /// Hands `results` to the list as a finished sign-in refresh (without
    /// its threads, which would ask Claude Code about the real folders).
    fn refresh(a: &mut App, results: Vec<(&str, Info, Option<PathBuf>)>) {
        let (tx, rx) = channel();
        a.info_loading = true;
        a.info_left = results.len();
        for (id, info, folder) in results {
            tx.send((id.to_string(), info, folder)).unwrap();
        }
        a.info_rx = Some(rx);
        a.receive_info();
    }

    thread_local! {
        /// Whether the noted check was still there when the fake sign-out ran.
        static CHECK_AT_SIGN_OUT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
        static CHECKS_OF_TEST: std::cell::RefCell<PathBuf> = const { std::cell::RefCell::new(PathBuf::new()) };
    }

    fn sign_out_noting(a: &Account) -> Vec<PathBuf> {
        let there = CHECKS_OF_TEST.with(|d| d.borrow().join(format!("{}.json", a.id)).exists());
        CHECK_AT_SIGN_OUT.with(|c| c.set(Some(there)));
        vec![]
    }

    /// A list whose refresh found "personal" signed in in `folder`.
    fn with_noted_check() -> (App, PathBuf) {
        let mut a = app();
        let folder = a.checks_dir.join("homes/personal/.claude");
        a.begin_info();
        refresh(
            &mut a,
            vec![("personal", signed_in_info(), Some(folder.clone()))],
        );
        assert!(info::fresh_check(
            &a.checks_dir,
            "personal",
            &folder,
            time::now()
        ));
        CHECKS_OF_TEST.with(|d| *d.borrow_mut() = a.checks_dir.clone());
        CHECK_AT_SIGN_OUT.with(|c| c.set(None));
        (a, folder)
    }

    #[test]
    fn the_refresh_notes_subscriptions_for_the_launcher() {
        let mut a = app();
        a.store.accounts.push(Account {
            id: "key".into(),
            name: "Key".into(),
            kind: Kind::ApiKey,
            sso: false,
            fallback: true,
        });
        let folder = a.checks_dir.join("homes/personal/.claude");
        a.begin_info();
        refresh(
            &mut a,
            vec![
                ("personal", signed_in_info(), Some(folder.clone())),
                ("main", signed_in_info(), None),
                ("key", signed_in_info(), None),
            ],
        );
        assert!(info::fresh_check(
            &a.checks_dir,
            "personal",
            &folder,
            time::now()
        ));
        // The main login and API keys are not checked by the launcher.
        assert!(!a.checks_dir.join("main.json").exists());
        assert!(!a.checks_dir.join("key.json").exists());
        assert!(!a.info_loading);
        // A later refresh that finds it signed out replaces the old result.
        refresh(
            &mut a,
            vec![("personal", Info::default(), Some(folder.clone()))],
        );
        assert!(!info::fresh_check(
            &a.checks_dir,
            "personal",
            &folder,
            time::now()
        ));
        let _ = std::fs::remove_dir_all(&a.checks_dir);
    }

    #[test]
    fn signing_in_throws_the_noted_check_away() {
        // The sign-in cannot even start.
        let (mut a, folder) = with_noted_check();
        let account = a.store.account("personal").cloned().unwrap();
        a.start_sign_in(&account, false);
        assert_eq!(a.mode, Mode::Normal);
        assert!(!a.checks_dir.join("personal.json").exists());
        // A refresh that was running does not write it back.
        refresh(
            &mut a,
            vec![("personal", signed_in_info(), Some(folder.clone()))],
        );
        assert!(!info::fresh_check(
            &a.checks_dir,
            "personal",
            &folder,
            time::now()
        ));
        let _ = std::fs::remove_dir_all(&a.checks_dir);

        // A sign-in that runs: a refresh that starts meanwhile does not
        // note the account either.
        let (mut a, folder) = with_noted_check();
        a.sign_in_starter = |a, _| {
            let mut s = SignIn::showing("https://example.com/sign-in");
            s.id = a.id.clone();
            Ok(s)
        };
        a.start_sign_in(&account, false);
        assert!(matches!(a.mode, Mode::SignIn { .. }));
        assert!(!a.checks_dir.join("personal.json").exists());
        a.begin_info();
        refresh(
            &mut a,
            vec![("personal", signed_in_info(), Some(folder.clone()))],
        );
        assert!(!a.checks_dir.join("personal.json").exists());
        if let Some(s) = a.signin.as_mut() {
            s.cancel();
        }
        let _ = std::fs::remove_dir_all(&a.checks_dir);
    }

    #[test]
    fn removing_throws_the_noted_check_away_before_signing_out() {
        let (mut a, folder) = with_noted_check();
        a.sign_out = sign_out_noting;
        // Stops before anything else is removed, so the test touches no
        // real file.
        a.folder_remover = |_| Err(std::io::Error::other("test"));
        a.remove("personal");
        assert_eq!(CHECK_AT_SIGN_OUT.with(|c| c.get()), Some(false));
        assert!(!a.checks_dir.join("personal.json").exists());
        refresh(
            &mut a,
            vec![("personal", signed_in_info(), Some(folder.clone()))],
        );
        assert!(!a.checks_dir.join("personal.json").exists());
        let _ = std::fs::remove_dir_all(&a.checks_dir);
    }

    #[test]
    fn dropping_a_half_added_account_throws_its_check_away() {
        let (mut a, folder) = with_noted_check();
        a.sign_out = sign_out_noting;
        a.folder_remover = |_| Ok(vec![]);
        a.drop_new_account("personal");
        assert_eq!(CHECK_AT_SIGN_OUT.with(|c| c.get()), Some(false));
        assert!(a.store.account("personal").is_none());
        assert!(!a.checks_dir.join("personal.json").exists());
        refresh(
            &mut a,
            vec![("personal", signed_in_info(), Some(folder.clone()))],
        );
        assert!(!a.checks_dir.join("personal.json").exists());
        let _ = std::fs::remove_dir_all(&a.checks_dir);
    }

    #[test]
    fn main_cannot_be_removed() {
        let mut a = app();
        a.sel = 0;
        a.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn sign_out_failure_names_the_command() {
        for (w, h) in [(100, 30), (60, 20)] {
            let mut a = app();
            a.mode = sign_out_failed(&[mirror::new_dir_of("ultra"), mirror::home_of("ultra")]);
            // One command per folder still signed in.
            let Mode::Message { lines, .. } = &a.mode else {
                panic!("no message")
            };
            let commands: Vec<&String> =
                lines.iter().filter(|l| l.contains("auth logout")).collect();
            assert_eq!(commands.len(), 2);
            assert!(commands[0].contains("ultra/.claude\""), "{}", commands[0]);
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            let buf = t.backend().buffer();
            let rows: Vec<String> = (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            if std::env::var("SHOW_SCREEN").is_ok() {
                println!("{}", rows.join("\n"));
            }
            let screen = rows.join("");
            assert!(screen.contains("Sign-out did not work"), "{screen}");
            assert!(screen.contains("auth logout"), "{screen}");
        }
    }

    #[test]
    fn sign_out_never_runs_through_a_link_named_claude() {
        let root = std::env::temp_dir().join(format!("hm-login-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("homes").join("work");
        let elsewhere = root.join("dot-claude").join(".claude");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join(mirror::NEW)).unwrap();
        assert!(
            login_folders(&home).is_empty(),
            "the link is no folder of the account"
        );
        std::fs::write(home.join(".claude.json"), "{}").unwrap();
        assert_eq!(login_folders(&home), vec![home.clone()]);
        std::fs::remove_file(home.join(mirror::NEW)).unwrap();
        std::fs::create_dir_all(home.join(mirror::NEW)).unwrap();
        assert_eq!(
            login_folders(&home),
            vec![home.join(mirror::NEW), home.clone()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_account_on_its_old_folder_says_to_sign_in_again() {
        for w in [160, 100] {
            let mut a = app();
            a.info.insert(MAIN_ID.into(), signed_in("Max"));
            a.info.insert(
                "personal".into(),
                Info {
                    old_folder: true,
                    ..signed_in("Pro")
                },
            );
            a.store.accounts.push(Account {
                id: "work".into(),
                name: "Work".into(),
                kind: Kind::Subscription,
                sso: false,
                fallback: true,
            });
            a.info.insert("work".into(), signed_in("Team"));
            let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            let buf = t.backend().buffer();
            let rows: Vec<String> = (0..30)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            if std::env::var("SHOW_SCREEN").is_ok() {
                println!("{}", rows.join("\n"));
            }
            let with: Vec<&String> = rows
                .iter()
                .filter(|r| r.contains("l signs in again"))
                .collect();
            assert_eq!(with.len(), 1, "only Personal's row: {}", rows.join("\n"));
            assert!(
                with[0].contains(ui::OLD_FOLDER_NOTICE),
                "whole notice: {}",
                with[0]
            );
            let screen = rows.join("\n");
            assert!(screen.contains("Team"), "Work shows its plan: {screen}");
        }
    }

    #[test]
    fn a_limit_still_shows_on_an_account_on_its_old_folder() {
        let mut a = app();
        a.info.insert(
            "personal".into(),
            Info {
                old_folder: true,
                ..signed_in("Pro")
            },
        );
        let now = a.now();
        a.usage.insert("personal".into(), full_at(97.0, now));
        let mut t = Terminal::new(TestBackend::new(160, 30)).unwrap();
        t.draw(|f| ui::draw(f, &mut a)).unwrap();
        let screen: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("at its 5-hour limit"), "{screen}");
    }

    #[test]
    fn a_never_signed_in_account_shows_the_usual_text() {
        let mut a = app();
        // Not signed in anywhere: no sign-in-again notice.
        a.info.insert("personal".into(), Info::default());
        let mut t = Terminal::new(TestBackend::new(160, 30)).unwrap();
        t.draw(|f| ui::draw(f, &mut a)).unwrap();
        let screen: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("not signed in · l signs in"), "{screen}");
        assert!(!screen.contains("signs in again"), "{screen}");
    }

    #[test]
    fn twins_are_named_in_the_list() {
        let mut a = app();
        a.twins = [
            (MAIN_ID.to_string(), vec!["personal".to_string()]),
            ("personal".to_string(), vec![MAIN_ID.to_string()]),
        ]
        .into();
        let mut t = Terminal::new(TestBackend::new(160, 30)).unwrap();
        t.draw(|f| ui::draw(f, &mut a)).unwrap();
        let screen: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("same login as Personal"), "{screen}");
        assert!(screen.contains("same login as Main login"), "{screen}");
    }

    #[test]
    fn draws_at_many_sizes() {
        for (w, h) in [(160, 40), (100, 30), (60, 20), (40, 12)] {
            let mut a = app();
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            a.section = Section::Spaces;
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            a.section = Section::Sessions;
            a.panes = vec![Pane {
                id: "w1:p1".into(),
                workspace: "w1".into(),
                tab: "w1:t1".into(),
                label: "A long session name that does not fit everywhere".into(),
                agent: Some("claude".into()),
                status: Some("working".into()),
                account: Some("Main login".into()),
                account_id: Some(MAIN_ID.into()),
            }];
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            a.mode = Mode::Help;
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            if h < 40 {
                assert!(a.help_max > 0, "the keys scroll on a low screen ({w}x{h})");
                a.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
                t.draw(|f| ui::draw(f, &mut a)).unwrap();
                let buffer = format!("{:?}", t.backend().buffer());
                assert!(buffer.contains("scroll · end"), "{w}x{h}");
                a.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
                assert_eq!(a.help_scroll, a.help_max - 1);
            }
            a.section = Section::Accounts;
            a.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            a.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            a.mode = Mode::Message {
                title: "A title".into(),
                lines: vec![
                    "First line that is long enough to wrap around in a narrow pane.".into(),
                    "Second".into(),
                ],
                good: false,
            };
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            a.mode = key_input(None, "Sandbox".into());
            a.event(Event::Paste(
                "sk-ant-api03-abcdefghijklmnopqrstuvwxyz".into(),
            ));
            t.draw(|f| ui::draw(f, &mut a)).unwrap();
            let buffer = format!("{:?}", t.backend().buffer());
            assert!(!buffer.contains("sk-ant"), "a key is never shown");
        }
    }

    // ---- the repository link in the keys overlay ----------------------

    thread_local! {
        /// What the test opener was asked to open, per test thread.
        static OPENED: std::cell::RefCell<Vec<(String, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// An opener that starts nothing and only notes what it got.
    fn opens(opener: &str, url: &str) -> bool {
        OPENED.with(|o| o.borrow_mut().push((opener.into(), url.into())));
        true
    }

    /// An opener that cannot be started.
    fn cannot_open(opener: &str, url: &str) -> bool {
        opens(opener, url);
        false
    }

    fn opened() -> Vec<(String, String)> {
        OPENED.with(|o| o.borrow_mut().drain(..).collect())
    }

    const REPO: &str = "https://github.com/nolvane/herdr-multiaccount";

    /// The keys overlay drawn on a `w`×`h` screen: the app, the rows of the
    /// screen and the overlay's box.
    fn help_on(w: u16, h: u16, opener: Opener) -> (App, Vec<String>, Rect, Terminal<TestBackend>) {
        let mut a = app();
        a.opener = opener;
        a.mode = Mode::Help;
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui::draw(f, &mut a)).unwrap();
        let buf = t.backend().buffer().clone();
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        if std::env::var("SHOW_SCREEN").is_ok() {
            println!("{}", rows.join("\n"));
        }
        // The overlay's own target is the last `?` (the footer has one too).
        let area = a
            .hits
            .iter()
            .rev()
            .find_map(|(r, h)| (*h == Hit::Key('?')).then_some(*r))
            .expect("the overlay");
        (a, rows, area, t)
    }

    fn link_spot(a: &App) -> Option<Rect> {
        a.hits
            .iter()
            .find_map(|(r, h)| (*h == Hit::Link).then_some(*r))
    }

    fn row_text(rows: &[String], y: u16) -> String {
        rows[y as usize].clone()
    }

    #[test]
    fn the_overlay_names_version_and_repository_in_its_bottom_border() {
        let version = env!("CARGO_PKG_VERSION");
        let (a, rows, area, t) = help_on(160, 40, opens);
        let bottom = area.bottom() - 1;
        let full = format!("Herdr Multiaccount {version} · github.com/nolvane/herdr-multiaccount");
        let line = row_text(&rows, bottom);
        assert!(line.contains(&full), "{line}");
        // Starts right after the padding space, two cells past the corner.
        let start = line.chars().position(|c| c == 'H').unwrap() as u16;
        assert_eq!(start, area.x + 3, "{line}");
        let cell = &t.backend().buffer()[(area.x + 3, bottom)];
        assert!(cell.modifier.contains(ratatui::style::Modifier::UNDERLINED));
        assert_eq!(cell.fg, crate::theme::ACCENT);
        // In the border itself, so it takes no row of the content.
        assert!(line.contains('╰') && line.contains('╯'), "{line}");
        assert_eq!(link_spot(&a).unwrap().width as usize, full.chars().count());
    }

    #[test]
    fn a_narrow_overlay_shortens_the_text_and_keeps_its_corners() {
        let version = env!("CARGO_PKG_VERSION");
        let name = format!("Herdr Multiaccount {version}");
        for (w, want) in [(40, Some(name.as_str())), (20, Some(version)), (10, None)] {
            let (a, rows, area, _t) = help_on(w, 24, opens);
            let line = row_text(&rows, area.bottom() - 1);
            assert!(!line.contains("github.com"), "{w}: {line}");
            let chars: Vec<char> = line.chars().collect();
            assert_eq!(chars[area.x as usize], '╰', "{w}: {line}");
            assert_eq!(chars[(area.right() - 1) as usize], '╯', "{w}: {line}");
            match want {
                Some(text) => {
                    assert!(line.contains(text), "{w}: {line}");
                    if text == version {
                        assert!(!line.contains("Herdr"), "{w}: {line}");
                    }
                    assert_eq!(link_spot(&a).unwrap().width as usize, text.chars().count());
                }
                None => {
                    assert!(!line.contains(version), "{w}: {line}");
                    assert_eq!(link_spot(&a), None);
                }
            }
        }
    }

    #[test]
    fn the_version_shows_on_a_scrolling_and_a_two_column_overlay() {
        let version = env!("CARGO_PKG_VERSION");
        // Low: the content scrolls, the hint stays above the border.
        let (a, rows, area, _t) = help_on(100, 30, opens);
        assert!(a.help_max > 0);
        assert!(row_text(&rows, area.bottom() - 1).contains(&format!("Multiaccount {version}")));
        assert!(row_text(&rows, area.bottom() - 2).contains("↑ ↓ scroll"));
        // Wide and low: two columns.
        let (_a, rows, area, _t) = help_on(130, 30, opens);
        assert_eq!(area.width, 124, "two columns");
        assert!(
            row_text(&rows, area.bottom() - 1).contains("github.com/nolvane/herdr-multiaccount")
        );
    }

    #[test]
    fn clicking_the_version_opens_the_repository() {
        for end in [false, true] {
            let (mut a, _rows, _area, _t) = help_on(160, 40, opens);
            let spot = link_spot(&a).unwrap();
            let x = if end { spot.right() - 1 } else { spot.x };
            a.click(x, spot.y);
            assert_eq!(a.mode, Mode::Normal, "the overlay closes");
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            assert_eq!(opened(), vec![(opener.to_string(), REPO.to_string())]);
            assert_eq!(
                a.flash.as_ref().map(|f| f.0.as_str()),
                Some("Opened in the browser")
            );
        }
    }

    #[test]
    fn a_failing_opener_is_named_and_the_list_goes_on() {
        let (mut a, _rows, _area, mut t) = help_on(160, 40, cannot_open);
        let spot = link_spot(&a).unwrap();
        a.click(spot.x, spot.y);
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(opened().len(), 1);
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(
            a.flash.as_ref().map(|f| f.0.clone()),
            Some(format!("Could not run {opener}"))
        );
        a.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(a.sel, 1, "the list still takes keys");
        t.draw(|f| ui::draw(f, &mut a)).unwrap();
    }

    #[test]
    fn other_clicks_on_the_overlay_only_close_it() {
        let (mut a, _rows, area, _t) = help_on(160, 40, opens);
        let spot = link_spot(&a).unwrap();
        for (x, y) in [
            (spot.x - 1, spot.y),
            (spot.right(), spot.y),
            (area.x + area.width / 2, area.y + 2),
        ] {
            a.mode = Mode::Help;
            a.click(x, y);
            assert_eq!(a.mode, Mode::Normal, "({x}, {y})");
        }
        assert!(opened().is_empty());
        for code in [
            KeyCode::Esc,
            KeyCode::Char('?'),
            KeyCode::Char('q'),
            KeyCode::Enter,
        ] {
            a.mode = Mode::Help;
            a.key(KeyEvent::new(code, KeyModifiers::NONE));
            assert_eq!(a.mode, Mode::Normal, "{code:?}");
        }
        assert!(!a.quit);
        // Scrolling still works on a low screen, with keys and the wheel.
        let (mut a, _rows, _area, _t) = help_on(100, 30, opens);
        a.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(a.help_scroll, 1);
        a.event(Event::Mouse(ratatui::crossterm::event::MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(a.help_scroll, 3);
        assert!(opened().is_empty());
    }

    #[test]
    fn the_sign_in_link_goes_through_the_opener() {
        let mut a = app();
        a.opener = opens;
        a.signin = Some(SignIn::showing("https://example.com/sign-in"));
        a.mode = Mode::SignIn {
            code: String::new(),
        };
        a.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(
            opened(),
            vec![(
                opener.to_string(),
                "https://example.com/sign-in".to_string()
            )]
        );
        assert!(a.flash.is_none(), "a started opener shows no message");
        assert_eq!(
            a.mode,
            Mode::SignIn {
                code: String::new()
            }
        );
    }

    #[test]
    fn a_sign_in_link_that_cannot_open_is_named_and_the_dialog_goes_on() {
        let mut a = app();
        a.opener = cannot_open;
        a.signin = Some(SignIn::showing("https://example.com/sign-in"));
        a.mode = Mode::SignIn {
            code: String::new(),
        };
        a.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert_eq!(opened().len(), 1);
        let flash = a.flash.as_ref().map(|f| (f.0.clone(), f.1));
        assert_eq!(
            flash,
            Some((format!("Could not run {OPENER}"), crate::theme::RED))
        );
        // The dialog stays and still takes a code, sends it and cancels.
        for c in "abc".chars() {
            a.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(a.mode, Mode::SignIn { code: "abc".into() });
        a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(a.signin.as_ref().unwrap().code_sent, "the code was sent");
        assert_eq!(
            a.mode,
            Mode::SignIn {
                code: String::new()
            }
        );
        a.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(a.signin.as_ref().unwrap().finished, Some(false));
    }

    /// The test process's children that have ended but were not waited on.
    fn ended_children() -> usize {
        let me = std::process::id().to_string();
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,stat="])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                f.len() == 3 && f[1] == me && f[2].starts_with('Z')
            })
            .count()
    }

    #[test]
    fn opened_links_leave_no_ended_opener_behind() {
        for _ in 0..5 {
            assert!(open_in_browser("true", "https://example.com"));
        }
        let until = Instant::now() + Duration::from_secs(5);
        let mut left = ended_children();
        while left > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
            left = ended_children();
        }
        assert_eq!(left, 0, "ended openers are waited on");
    }

    #[test]
    fn opening_a_link_does_not_wait_for_the_opener() {
        let start = Instant::now();
        // Still running when it returns; ends soon after the test.
        assert!(open_in_browser("sleep", "2"));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!open_in_browser(
            "/nonexistent/opener",
            "https://example.com"
        ));
    }

    // ---- the Agents tab: ending a session ------------------------------

    thread_local! {
        /// The panes the test closer was asked to close, and what the test
        /// pane list hands back, per test thread.
        static CLOSED: std::cell::RefCell<Vec<String>> =
            const { std::cell::RefCell::new(Vec::new()) };
        static FRESH: std::cell::RefCell<Option<Vec<Pane>>> =
            const { std::cell::RefCell::new(None) };
    }

    fn closes(pane: &str) -> bool {
        CLOSED.with(|c| c.borrow_mut().push(pane.into()));
        true
    }

    fn cannot_close(pane: &str) -> bool {
        closes(pane);
        false
    }

    fn closed() -> Vec<String> {
        CLOSED.with(|c| c.borrow_mut().drain(..).collect())
    }

    fn fresh() -> Option<Vec<Pane>> {
        FRESH.with(|f| f.borrow().clone())
    }

    fn set_fresh(panes: Option<Vec<Pane>>) {
        FRESH.with(|f| *f.borrow_mut() = panes);
    }

    fn pane(id: &str, tab: &str, label: &str, status: Option<&str>, account: Option<&str>) -> Pane {
        Pane {
            id: id.into(),
            workspace: "w1".into(),
            tab: tab.into(),
            label: label.into(),
            agent: Some("claude".into()),
            status: status.map(str::to_string),
            account: account.map(str::to_string),
            account_id: account.map(str::to_string),
        }
    }

    fn shell(id: &str, tab: &str) -> Pane {
        Pane {
            agent: None,
            account: None,
            account_id: None,
            ..pane(id, tab, "zsh", None, None)
        }
    }

    /// The Agents tab with two sessions on Personal (one in another tab) and
    /// a plain shell next to the first; Herdr reports the same panes.
    fn agents() -> App {
        let mut a = app();
        a.pane_list = fresh;
        a.pane_closer = closes;
        a.panes = vec![
            pane(
                "w91:p1",
                "w91:t1",
                "Fix the login bug",
                Some("idle"),
                Some("personal"),
            ),
            shell("w91:p2", "w91:t1"),
            pane(
                "w91:p3",
                "w91:t2",
                "Write the docs",
                Some("idle"),
                Some("personal"),
            ),
        ];
        set_fresh(Some(a.panes.clone()));
        closed();
        a.section = Section::Sessions;
        a
    }

    fn press(a: &mut App, code: KeyCode) {
        a.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn screen_of(a: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui::draw(f, a)).unwrap();
        let buf = t.backend().buffer();
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        if std::env::var("SHOW_SCREEN").is_ok() {
            println!("{}", rows.join("\n"));
        }
        rows
    }

    fn confirm(a: &App) -> Confirm {
        match &a.mode {
            Mode::Confirm(c) => c.clone(),
            m => panic!("no question open: {m:?}"),
        }
    }

    #[test]
    fn the_third_tab_is_called_agents() {
        let mut a = agents();
        a.section = Section::Accounts;
        press(&mut a, KeyCode::Char('3'));
        assert_eq!(a.section, Section::Sessions, "3 reaches the tab");
        let rows = screen_of(&mut a, 100, 30);
        assert!(rows[0].contains("Agents (2)"), "{}", rows[0]);
        assert!(!rows[0].contains("Sessions"), "{}", rows[0]);
    }

    #[test]
    fn the_texts_that_name_the_tab_say_agents() {
        assert!(INTRO[5].starts_with("Agents shows"), "{}", INTRO[5]);
        let mut a = agents();
        a.mode = Mode::Help;
        let screen = screen_of(&mut a, 120, 60).join("\n");
        assert!(screen.contains("accounts / spaces / agents"), "{screen}");
        assert!(!screen.contains("Sessions"), "{screen}");
        if !herdr::inside() {
            let mut a = app();
            a.section = Section::Sessions;
            let screen = screen_of(&mut a, 100, 30).join("\n");
            assert!(screen.contains("Agents are Herdr's"), "{screen}");
        }
    }

    #[test]
    fn d_asks_before_ending_a_session_and_names_it() {
        for key in [KeyCode::Char('d'), KeyCode::Delete] {
            let mut a = agents();
            press(&mut a, key);
            let c = confirm(&a);
            assert_eq!(c.question, "End Fix the login bug?");
            let text = c.lines.join(" ");
            assert!(text.contains("acme-app"), "names the space: {text}");
            assert!(text.contains("Personal"), "names the account: {text}");
            assert!(text.contains("pane closes"), "{text}");
            assert!(!text.contains("current work stops"), "an idle one: {text}");
            assert!(closed().is_empty(), "nothing ended yet");
            assert_eq!(a.sessions().len(), 2);
        }
    }

    #[test]
    fn yes_closes_only_that_pane_and_the_row_goes() {
        let mut a = agents();
        assert_eq!(a.running("personal"), 2);
        press(&mut a, KeyCode::Char('j'));
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(confirm(&a).question, "End Write the docs?");
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(closed(), vec!["w91:p3".to_string()]);
        assert_eq!(a.mode, Mode::Normal);
        let left: Vec<String> = a.sessions().into_iter().map(|p| p.id).collect();
        assert_eq!(left, vec!["w91:p1".to_string()], "the row is gone");
        assert_eq!(a.session_sel, 0, "the selection moves to the neighbour");
        assert!(
            a.panes.iter().any(|p| p.id == "w91:p2"),
            "the shell in the same space stays"
        );
        assert_eq!(a.running("personal"), 1, "the account counts one fewer");
        assert_eq!(
            a.flash.as_ref().map(|f| f.0.as_str()),
            Some("Ended Write the docs")
        );
        let rows = screen_of(&mut a, 100, 30);
        assert!(rows[0].contains("Agents (1)"), "{}", rows[0]);
    }

    #[test]
    fn any_other_answer_keeps_the_session() {
        for key in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut a = agents();
            let before = a.panes.clone();
            press(&mut a, KeyCode::Char('d'));
            press(&mut a, key);
            assert_eq!(a.mode, Mode::Normal, "{key:?}");
            assert!(closed().is_empty(), "{key:?}");
            assert_eq!(a.panes, before, "{key:?}");
        }
    }

    #[test]
    fn enter_and_other_keys_leave_the_other_questions_open() {
        let end = || {
            let mut a = agents();
            press(&mut a, KeyCode::Char('d'));
            a
        };
        let remove = || {
            let mut a = agents();
            a.section = Section::Accounts;
            a.sel = 1;
            press(&mut a, KeyCode::Char('d'));
            a
        };
        // Never answered here: yes would change the real setup.
        let setup = || {
            let mut a = agents();
            a.mode = Mode::Confirm(Confirm {
                question: "Set up the accounts?".into(),
                lines: vec!["Adds the claude function.".into()],
                purpose: ConfirmFor::Setup,
            });
            a
        };
        let close = || {
            let mut a = spaces();
            press(&mut a, KeyCode::Char('d'));
            a
        };
        let asks: [fn() -> App; 4] = [end, remove, setup, close];
        for ask in asks {
            for key in [KeyCode::Enter, KeyCode::Char('x'), KeyCode::Char(' ')] {
                let mut a = ask();
                let before = confirm(&a);
                press(&mut a, key);
                assert_eq!(confirm(&a), before, "{}: {key:?}", before.question);
                assert!(closed().is_empty());
                assert!(spaces_closed().is_empty());
            }
            let mut a = ask();
            press(&mut a, KeyCode::Esc);
            assert_eq!(a.mode, Mode::Normal);
            assert_eq!(flash_text(&a), "", "only a move says it did not run");
        }
        // Their buttons name y and Esc n, not Enter.
        let mut a = end();
        let screen = screen_of(&mut a, 100, 30).join("\n");
        assert!(screen.contains(" y yes "), "{screen}");
        assert!(screen.contains(" Esc n no "), "{screen}");
        assert!(!screen.contains("⏎ y yes"), "{screen}");
    }

    #[test]
    fn a_busy_session_is_warned_about_and_still_ends() {
        for status in ["working", "blocked"] {
            let mut a = agents();
            a.panes[0].status = Some(status.into());
            set_fresh(Some(a.panes.clone()));
            press(&mut a, KeyCode::Char('d'));
            let text = confirm(&a).lines.join(" ");
            assert!(text.contains("current work stops"), "{status}: {text}");
            press(&mut a, KeyCode::Char('y'));
            assert_eq!(closed(), vec!["w91:p1".to_string()], "{status}");
        }
    }

    #[test]
    fn a_session_that_ended_meanwhile_closes_nothing_else() {
        // Gone from Herdr, or its pane now runs something else.
        for now in [
            vec![shell("w91:p2", "w91:t1")],
            vec![shell("w91:p1", "w91:t1")],
        ] {
            let mut a = agents();
            press(&mut a, KeyCode::Char('d'));
            set_fresh(Some(now));
            press(&mut a, KeyCode::Char('y'));
            assert!(closed().is_empty());
            assert_eq!(
                a.flash.as_ref().map(|f| f.0.as_str()),
                Some("Fix the login bug had already ended")
            );
            assert!(a.sessions().iter().all(|p| p.id != "w91:p1"));
        }
    }

    #[test]
    fn a_pane_herdr_cannot_close_stays_with_an_error() {
        let mut a = agents();
        a.pane_closer = cannot_close;
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(closed(), vec!["w91:p1".to_string()]);
        let flash = a.flash.clone().map(|f| (f.0, f.1));
        assert_eq!(
            flash,
            Some((
                "Herdr could not close Fix the login bug".to_string(),
                crate::theme::RED
            ))
        );
        assert_eq!(a.sessions().len(), 2, "the row stays");

        // Herdr cannot be asked at all: nothing is closed either.
        let mut a = agents();
        press(&mut a, KeyCode::Char('d'));
        set_fresh(None);
        press(&mut a, KeyCode::Char('y'));
        assert!(closed().is_empty());
        assert_eq!(a.flash.as_ref().map(|f| f.1), Some(crate::theme::RED));
        assert_eq!(a.sessions().len(), 2);
    }

    #[test]
    fn d_does_nothing_in_an_empty_tab_and_still_removes_in_accounts() {
        let mut a = agents();
        a.panes = vec![shell("w91:p2", "w91:t1")];
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(a.mode, Mode::Normal);
        a.section = Section::Accounts;
        a.sel = 1;
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(confirm(&a).purpose, ConfirmFor::Remove("personal".into()));
    }

    #[test]
    fn the_list_never_ends_itself() {
        let mut a = agents();
        a.panes = vec![pane("w91:p9", "w91:t1", "Multiaccount", Some("idle"), None)];
        assert!(a.sessions().is_empty());
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(a.mode, Mode::Normal);
        assert!(closed().is_empty());
    }

    #[test]
    fn the_footer_and_the_keys_overlay_show_the_new_key() {
        let mut a = agents();
        let rows = screen_of(&mut a, 100, 30);
        assert!(rows[29].contains("d end session"), "{}", rows[29]);
        a.mode = Mode::Help;
        let screen = screen_of(&mut a, 120, 60).join("\n");
        assert!(screen.contains("end the session"), "{screen}");
        assert!(screen.contains("remove (asks first)"), "{screen}");
        // A click on the footer key asks too.
        let mut a = agents();
        screen_of(&mut a, 100, 30);
        let spot = a
            .hits
            .iter()
            .find_map(|(r, h)| (*h == Hit::Key('d')).then_some(*r))
            .expect("d in the footer");
        a.click(spot.x, spot.y);
        assert_eq!(confirm(&a).question, "End Fix the login bug?");
    }

    #[test]
    fn the_question_draws_at_many_sizes() {
        for (w, h) in [(160, 40), (100, 30), (60, 20), (40, 12), (24, 20), (20, 30)] {
            let mut a = agents();
            a.panes[0].status = Some("working".into());
            press(&mut a, KeyCode::Char('d'));
            let screen = screen_of(&mut a, w, h).join("\n");
            assert!(screen.contains("End Fix"), "{w}x{h}: {screen}");
            assert!(screen.contains("y yes"), "{w}x{h}: {screen}");
            // Wrapped to the pane, not cut off at its edge: every word of
            // the question shows whole.
            let shown: Vec<&str> = screen.split_whitespace().collect();
            for word in confirm(&a).lines.iter().flat_map(|l| l.split_whitespace()) {
                assert!(shown.contains(&word), "{w}x{h}: {word} cut off\n{screen}");
            }
        }
    }

    // ---- closing a space (d in Spaces) --------------------------------

    thread_local! {
        /// The spaces the test closer was asked to close (with `--group` or
        /// not), and what the test space list hands back.
        static SPACES_CLOSED: std::cell::RefCell<Vec<(String, bool)>> =
            const { std::cell::RefCell::new(Vec::new()) };
        static FRESH_SPACES: std::cell::RefCell<Option<Vec<Workspace>>> =
            const { std::cell::RefCell::new(None) };
    }

    fn closes_space(ws: &str, group: bool) -> bool {
        SPACES_CLOSED.with(|c| c.borrow_mut().push((ws.into(), group)));
        true
    }

    fn cannot_close_space(ws: &str, group: bool) -> bool {
        closes_space(ws, group);
        false
    }

    fn spaces_closed() -> Vec<(String, bool)> {
        SPACES_CLOSED.with(|c| c.borrow_mut().drain(..).collect())
    }

    fn fresh_spaces() -> Option<Vec<Workspace>> {
        FRESH_SPACES.with(|f| f.borrow().clone())
    }

    fn set_fresh_spaces(list: Option<Vec<Workspace>>) {
        FRESH_SPACES.with(|f| *f.borrow_mut() = list);
    }

    fn space(id: &str, number: u64, label: &str, tree: Option<(&str, bool)>) -> Workspace {
        Workspace {
            id: id.into(),
            label: label.into(),
            number,
            focused: false,
            worktree: tree.map(|(key, linked)| herdr::Worktree {
                repo_key: key.into(),
                linked,
            }),
        }
    }

    fn in_space(ws: &str, p: Pane) -> Pane {
        Pane {
            workspace: ws.into(),
            ..p
        }
    }

    /// The Spaces tab: acme-app (main checkout, one idle session and a
    /// shell), its worktree space fix-login (one session), and notes (no
    /// session, on Work). Herdr reports the same.
    fn spaces() -> App {
        let mut a = app();
        a.store.accounts.push(Account {
            id: "work".into(),
            name: "Work".into(),
            kind: Kind::Subscription,
            sso: false,
            fallback: true,
        });
        a.store.assign("w3", "notes", Some("work"));
        a.store.assign("w2", "fix-login", Some("personal"));
        STORED.with(|s| *s.borrow_mut() = a.store.clone());
        a.workspaces = vec![
            space("w1", 1, "acme-app", Some(("/src/acme/.git", false))),
            space("w2", 2, "fix-login", Some(("/src/acme/.git", true))),
            space("w3", 3, "notes", None),
        ];
        a.panes = vec![
            in_space(
                "w1",
                pane(
                    "w1:p1",
                    "w1:t1",
                    "Fix the login bug",
                    Some("idle"),
                    Some("personal"),
                ),
            ),
            in_space("w1", shell("w1:p2", "w1:t1")),
            in_space(
                "w2",
                pane(
                    "w2:p1",
                    "w2:t1",
                    "Write the docs",
                    Some("idle"),
                    Some("personal"),
                ),
            ),
            in_space("w3", shell("w3:p1", "w3:t1")),
        ];
        a.space_list = fresh_spaces;
        a.space_closer = closes_space;
        a.pane_list = fresh;
        set_fresh_spaces(Some(a.workspaces.clone()));
        set_fresh(Some(a.panes.clone()));
        spaces_closed();
        a.section = Section::Spaces;
        a
    }

    fn ids(list: &[Workspace]) -> Vec<String> {
        list.iter().map(|w| w.id.clone()).collect()
    }

    #[test]
    fn d_asks_before_closing_a_space_and_names_its_sessions() {
        let mut a = spaces();
        a.space_sel = 1;
        press(&mut a, KeyCode::Char('d'));
        let c = confirm(&a);
        assert_eq!(c.question, "Close fix-login?");
        let text = c.lines.join(" ");
        assert!(text.contains("1 Claude Code session runs in it."), "{text}");
        assert!(
            text.contains("tabs and everything running in them close"),
            "{text}"
        );
        assert!(text.contains("claude --resume"), "{text}");
        assert!(!text.contains("current work stops"), "idle: {text}");
        assert!(!text.contains("closes too"), "{text}");
        assert!(spaces_closed().is_empty(), "nothing closed yet");
        assert_eq!(a.workspaces.len(), 3);
    }

    #[test]
    fn a_space_without_sessions_says_so_and_a_nameless_one_has_its_number() {
        let mut a = spaces();
        a.space_sel = 2;
        press(&mut a, KeyCode::Char('d'));
        let text = confirm(&a).lines.join(" ");
        assert!(
            text.contains("No Claude Code session runs in it."),
            "{text}"
        );
        assert!(!text.contains("stay on disk"), "no worktree: {text}");
        let mut a = spaces();
        a.workspaces[2].label.clear();
        a.space_sel = 2;
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(confirm(&a).question, "Close space 3?");
    }

    #[test]
    fn busy_sessions_are_warned_about() {
        for status in ["working", "blocked"] {
            let mut a = spaces();
            a.space_sel = 1;
            a.panes[2].status = Some(status.into());
            press(&mut a, KeyCode::Char('d'));
            let text = confirm(&a).lines.join(" ");
            assert!(text.contains("its current work stops"), "{status}: {text}");
        }
        // Several sessions, one of them busy.
        let mut a = spaces();
        a.panes[0].status = Some("working".into());
        press(&mut a, KeyCode::Char('d'));
        let text = confirm(&a).lines.join(" ");
        assert!(
            text.contains("2 Claude Code sessions run in them."),
            "{text}"
        );
        assert!(
            text.contains(
                "1 of them is working or waiting for your answer: their current work stops."
            ),
            "{text}"
        );
    }

    #[test]
    fn yes_closes_the_space_and_its_rows_go() {
        let mut a = spaces();
        a.space_sel = 2;
        a.section = Section::Sessions;
        a.session_sel = 1;
        a.section = Section::Spaces;
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w3".to_string(), false)]);
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(ids(&a.workspaces), vec!["w1", "w2"], "the row is gone");
        assert_eq!(a.space_sel, 1, "the selection moves to the row above");
        assert!(a.panes.iter().all(|p| p.workspace != "w3"));
        assert_eq!(flash_text(&a), "Closed notes");
        let rows = screen_of(&mut a, 100, 30);
        assert!(rows[0].contains("Spaces (2)"), "{}", rows[0]);

        // Its sessions leave the Agents tab too.
        let mut a = spaces();
        a.space_sel = 1;
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w2".to_string(), false)]);
        let left: Vec<String> = a.sessions().into_iter().map(|p| p.id).collect();
        assert_eq!(left, vec!["w1:p1".to_string()]);
        assert_eq!(a.space_sel, 1, "the next space moves up");
        assert_eq!(a.running("personal"), 1);
    }

    #[test]
    fn closing_the_only_space_leaves_the_empty_tab() {
        let mut a = spaces();
        a.workspaces.truncate(1);
        a.workspaces[0].worktree = None;
        set_fresh_spaces(Some(a.workspaces.clone()));
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w1".to_string(), false)]);
        assert!(a.workspaces.is_empty());
        assert_eq!(a.space_sel, 0);
        let rows = screen_of(&mut a, 100, 30).join("\n");
        assert!(rows.contains("Spaces (0)"), "{rows}");
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(a.mode, Mode::Normal, "nothing to ask about");
    }

    #[test]
    fn no_and_esc_keep_the_space() {
        for key in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut a = spaces();
            let before = (a.workspaces.clone(), a.panes.clone());
            press(&mut a, KeyCode::Char('d'));
            press(&mut a, key);
            assert_eq!(a.mode, Mode::Normal, "{key:?}");
            assert!(spaces_closed().is_empty(), "{key:?}");
            assert_eq!((a.workspaces.clone(), a.panes.clone()), before);
            assert_eq!(stored(), a.store, "{key:?}: account choices kept");
        }
    }

    #[test]
    fn a_worktree_space_closes_alone_and_its_checkout_stays() {
        let mut a = spaces();
        a.space_sel = 1;
        press(&mut a, KeyCode::Char('d'));
        let text = confirm(&a).lines.join(" ");
        assert!(
            text.contains("Worktree checkouts and their branches stay on disk."),
            "{text}"
        );
        assert!(!text.contains("main checkout"), "{text}");
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w2".to_string(), false)]);
        assert_eq!(ids(&a.workspaces), vec!["w1", "w3"]);
    }

    #[test]
    fn a_main_checkout_names_its_worktree_spaces_and_closes_them_together() {
        let mut a = spaces();
        a.workspaces
            .push(space("w4", 4, "docs-site", Some(("/src/acme/.git", true))));
        a.workspaces.push(space(
            "w5",
            5,
            "other-repo",
            Some(("/src/other/.git", false)),
        ));
        a.panes.push(in_space(
            "w4",
            pane("w4:p1", "w4:t1", "Docs", Some("working"), None),
        ));
        set_fresh_spaces(Some(a.workspaces.clone()));
        set_fresh(Some(a.panes.clone()));
        press(&mut a, KeyCode::Char('d'));
        let c = confirm(&a);
        assert_eq!(c.question, "Close acme-app?");
        let text = c.lines.join(" ");
        assert!(
            text.contains("main checkout of its repository: the repository's other spaces fix-login, docs-site close with it."),
            "{text}"
        );
        assert!(!text.contains("other-repo"), "{text}");
        assert!(
            text.contains("3 Claude Code sessions run in them."),
            "{text}"
        );
        assert!(text.contains("1 of them is working"), "{text}");
        assert!(text.contains("stay on disk"), "{text}");
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w1".to_string(), true)], "--group");
        assert_eq!(ids(&a.workspaces), vec!["w3", "w5"]);
        assert!(a.sessions().is_empty());
        assert_eq!(
            stored().space_account("w2"),
            None,
            "fix-login's choice goes"
        );
        assert_eq!(
            stored().space_account("w3"),
            Some("work"),
            "notes keeps its"
        );
    }

    #[test]
    fn a_main_checkout_without_open_worktree_spaces_closes_alone() {
        let mut a = spaces();
        a.workspaces.remove(1);
        set_fresh_spaces(Some(a.workspaces.clone()));
        press(&mut a, KeyCode::Char('d'));
        let text = confirm(&a).lines.join(" ");
        assert!(!text.contains("other spaces"), "{text}");
        assert!(text.contains("1 Claude Code session runs in it."), "{text}");
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w1".to_string(), false)]);
    }

    #[test]
    fn a_group_that_grew_since_the_question_closes_nothing() {
        let mut a = spaces();
        press(&mut a, KeyCode::Char('d'));
        let mut now = a.workspaces.clone();
        now.push(space("w4", 4, "docs-site", Some(("/src/acme/.git", true))));
        set_fresh_spaces(Some(now));
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert!(
            flash_text(&a).contains("docs-site opened since"),
            "{}",
            flash_text(&a)
        );
        assert_eq!(stored(), a.store, "account choices kept");
        assert_eq!(a.workspaces.len(), 4, "the new space shows");

        // A group that shrank closes what is left of it.
        let mut a = spaces();
        press(&mut a, KeyCode::Char('d'));
        set_fresh_spaces(Some(vec![a.workspaces[0].clone(), a.workspaces[2].clone()]));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w1".to_string(), false)]);
    }

    #[test]
    fn a_space_closed_meanwhile_changes_nothing_else() {
        let mut a = spaces();
        a.space_sel = 2;
        press(&mut a, KeyCode::Char('d'));
        set_fresh_spaces(Some(a.workspaces[..2].to_vec()));
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert_eq!(flash_text(&a), "notes had already closed");
        assert_eq!(stored().space_account("w3"), Some("work"), "kept");
        assert_eq!(a.space_sel, 1, "the selection stays on a row");
    }

    #[test]
    fn a_space_herdr_cannot_close_stays_with_its_account() {
        let mut a = spaces();
        a.space_sel = 2;
        a.space_closer = cannot_close_space;
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w3".to_string(), false)]);
        assert_eq!(
            a.flash.clone().map(|f| (f.0, f.1)),
            Some(("Herdr could not close notes".to_string(), crate::theme::RED))
        );
        assert_eq!(a.workspaces.len(), 3, "the row stays");
        assert_eq!(stored().space_account("w3"), Some("work"), "choice back");
        assert_eq!(a.store.space_account("w3"), Some("work"));

        // Herdr cannot be asked at all.
        let mut a = spaces();
        press(&mut a, KeyCode::Char('d'));
        set_fresh_spaces(None);
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert_eq!(a.flash.as_ref().map(|f| f.1), Some(crate::theme::RED));
        assert_eq!(a.workspaces.len(), 3);

        // Herdr refuses and the choice cannot be put back: both are said.
        let mut a = spaces();
        a.space_sel = 2;
        a.space_closer = |ws, group| {
            STORE_BROKEN.with(|b| b.set(true));
            cannot_close_space(ws, group)
        };
        a.store_update = stores_until_broken;
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        STORE_BROKEN.with(|b| b.set(false));
        assert_eq!(spaces_closed(), vec![("w3".to_string(), false)]);
        assert_eq!(
            flash_text(&a),
            "Herdr could not close notes, and its account choice is lost: choose it again with ⏎"
        );

        // The account choice cannot be forgotten: nothing closes either.
        let mut a = spaces();
        a.space_sel = 2;
        a.store_update = cannot_store;
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert!(
            flash_text(&a).starts_with("Not saved"),
            "{}",
            flash_text(&a)
        );
    }

    #[test]
    fn sessions_that_changed_since_the_question_close_nothing() {
        // One started working without the question warning about it.
        let mut a = spaces();
        a.space_sel = 1;
        press(&mut a, KeyCode::Char('d'));
        let mut now = a.panes.clone();
        now[2].status = Some("working".into());
        set_fresh(Some(now.clone()));
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert_eq!(
            flash_text(&a),
            "fix-login changed since the question; nothing was closed. Press d again"
        );
        assert_eq!(stored(), a.store, "account choices kept");
        assert_eq!(a.panes, now, "the list shows what Herdr says now");

        // A new session opened in it.
        let mut a = spaces();
        a.space_sel = 2;
        press(&mut a, KeyCode::Char('d'));
        let mut now = a.panes.clone();
        now.push(in_space(
            "w3",
            pane("w3:p2", "w3:t1", "New", Some("idle"), None),
        ));
        set_fresh(Some(now));
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert!(flash_text(&a).contains("changed since the question"));

        // The question warned about one; another started working since.
        let mut a = spaces();
        a.panes[0].status = Some("working".into());
        set_fresh(Some(a.panes.clone()));
        press(&mut a, KeyCode::Char('d'));
        let mut now = a.panes.clone();
        now[2].status = Some("working".into());
        set_fresh(Some(now));
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert!(flash_text(&a).contains("changed since the question"));

        // The question warned already, or a session ended: it closes.
        let mut a = spaces();
        a.space_sel = 1;
        a.panes[2].status = Some("blocked".into());
        let mut now = a.panes.clone();
        now[2].status = Some("working".into());
        press(&mut a, KeyCode::Char('d'));
        set_fresh(Some(now));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w2".to_string(), false)]);
        let mut a = spaces();
        press(&mut a, KeyCode::Char('d'));
        set_fresh(Some(vec![a.panes[1].clone()]));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w1".to_string(), true)]);

        // Herdr's panes cannot be read: nothing closes.
        let mut a = spaces();
        press(&mut a, KeyCode::Char('d'));
        set_fresh(None);
        press(&mut a, KeyCode::Char('y'));
        assert!(spaces_closed().is_empty());
        assert_eq!(a.flash.as_ref().map(|f| f.1), Some(crate::theme::RED));
    }

    #[test]
    fn the_stored_choices_are_forgotten_also_when_this_list_has_not_read_them() {
        let mut a = spaces();
        // Another list chose an account for acme-app since this one read.
        STORED.with(|s| s.borrow_mut().assign("w1", "acme-app", Some("work")));
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w1".to_string(), true)]);
        assert_eq!(stored().space_account("w1"), None);
        assert_eq!(stored().space_account("w2"), None);
        assert_eq!(stored().space_account("w3"), Some("work"));
    }

    #[test]
    fn a_close_that_failed_halfway_puts_back_only_the_open_spaces() {
        let mut a = spaces();
        a.store.assign("w1", "acme-app", Some("work"));
        STORED.with(|s| *s.borrow_mut() = a.store.clone());
        a.space_closer = |ws, group| {
            // fix-login went, acme-app stayed.
            set_fresh_spaces(
                fresh_spaces().map(|l| l.into_iter().filter(|w| w.id != "w2").collect()),
            );
            cannot_close_space(ws, group)
        };
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w1".to_string(), true)]);
        assert_eq!(stored().space_account("w1"), Some("work"), "still open");
        assert_eq!(
            stored().space_account("w2"),
            None,
            "closed: stays forgotten"
        );
        assert_eq!(flash_text(&a), "Herdr could not close acme-app");

        // Another list chose an account meanwhile: its choice stays.
        let mut a = spaces();
        a.space_sel = 2;
        a.space_closer = |ws, group| {
            STORED.with(|s| s.borrow_mut().assign("w3", "notes", Some("personal")));
            cannot_close_space(ws, group)
        };
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(stored().space_account("w3"), Some("personal"));
    }

    #[test]
    fn closing_forgets_the_space_account_choice() {
        let mut a = spaces();
        a.space_sel = 2;
        assert_eq!(stored().space_account("w3"), Some("work"));
        press(&mut a, KeyCode::Char('d'));
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(spaces_closed(), vec![("w3".to_string(), false)]);
        assert_eq!(stored().space_account("w3"), None);
        assert!(
            stored().spaces.iter().all(|s| s.label != "notes"),
            "a new space named notes starts on the active account"
        );
        assert_eq!(
            stored().space_account("w2"),
            Some("personal"),
            "others stay"
        );
        assert_eq!(a.store, stored());
    }

    #[test]
    fn d_in_spaces_waits_while_a_session_is_moved() {
        let mut a = spaces();
        a.moving = Some(MoveRun {
            pane: "w1:p1".into(),
            label: "Fix the login bug".into(),
            to: "Work".into(),
        });
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(a.mode, Mode::Normal, "asks nothing");
        assert_eq!(
            flash_text(&a),
            "Fix the login bug is being moved; wait a moment"
        );
        assert!(spaces_closed().is_empty());
    }

    #[test]
    fn the_list_says_when_it_closes_with_its_space() {
        let mut a = spaces();
        a.own_pane = Some("w3:p9".into());
        a.panes.push(in_space(
            "w3",
            pane("w3:p9", "w3:t1", "Multiaccount", None, None),
        ));
        a.space_sel = 2;
        press(&mut a, KeyCode::Char('d'));
        let text = confirm(&a).lines.join(" ");
        assert!(
            text.contains("This list runs in it: it closes too."),
            "{text}"
        );
        assert!(
            text.contains("No Claude Code session"),
            "the list is none: {text}"
        );
        a.mode = Mode::Normal;
        a.space_sel = 0;
        press(&mut a, KeyCode::Char('d'));
        let text = confirm(&a).lines.join(" ");
        assert!(!text.contains("closes too"), "{text}");
    }

    #[test]
    fn the_footer_and_the_keys_overlay_show_d_and_x_keeps_its_meaning() {
        let mut a = spaces();
        let rows = screen_of(&mut a, 100, 30);
        assert!(rows[29].contains("d close space"), "{}", rows[29]);
        assert!(rows[29].contains("x use active"), "{}", rows[29]);
        let spot = a
            .hits
            .iter()
            .find_map(|(r, h)| (*h == Hit::Key('d')).then_some(*r))
            .expect("d in the footer");
        a.click(spot.x, spot.y);
        assert_eq!(confirm(&a).question, "Close acme-app?");
        let mut a = spaces();
        a.mode = Mode::Help;
        let screen = screen_of(&mut a, 120, 60).join("\n");
        assert!(
            screen.contains("close the space with everything in it"),
            "{screen}"
        );
        assert!(screen.contains("back to the active account"), "{screen}");
        // x, Backspace and Delete still go back to the active account (the
        // failing store stops them before they reach Herdr) and ask nothing.
        for key in [KeyCode::Char('x'), KeyCode::Backspace, KeyCode::Delete] {
            let mut a = spaces();
            a.store_update = cannot_store;
            press(&mut a, key);
            assert_eq!(a.mode, Mode::Normal, "{key:?}");
            assert!(flash_text(&a).starts_with("Not saved"), "{key:?}");
        }
        // Enter still picks the space's account.
        let mut a = spaces();
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.mode, Mode::Pick(_)), "{:?}", a.mode);
    }

    #[test]
    fn the_close_question_draws_at_many_sizes() {
        for (w, h) in [(160, 40), (100, 30), (60, 20), (40, 12), (24, 20), (20, 30)] {
            let mut a = spaces();
            a.own_pane = Some("w1:p2".into());
            a.panes[0].status = Some("working".into());
            press(&mut a, KeyCode::Char('d'));
            let screen = screen_of(&mut a, w, h).join("\n");
            assert!(screen.contains("Close"), "{w}x{h}: {screen}");
            assert!(screen.contains("y yes"), "{w}x{h}: {screen}");
            assert!(!screen.contains("⏎ y yes"), "{w}x{h}: {screen}");
            // Smaller panes scroll the question (tested with the move).
            if w >= 60 {
                let shown: Vec<&str> = screen.split_whitespace().collect();
                for word in confirm(&a).lines.iter().flat_map(|l| l.split_whitespace()) {
                    assert!(shown.contains(&word), "{w}x{h}: {word} cut off\n{screen}");
                }
            }
        }
    }

    // ---- moving a session (m) ----------------------------------------

    const CONV: &str = "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f6";

    fn held(session_id: &str, pid: u32, carry: Moved) -> Option<Held> {
        Some(Held {
            session_id: session_id.into(),
            pid,
            carry,
        })
    }

    /// A session started with options, on a model changed since.
    fn started_with_options() -> Moved {
        Moved {
            options: Some(vec![
                "--model=sonnet".into(),
                "--add-dir=/work/lib".into(),
                "--permission-mode=plan".into(),
                format!("--append-system-prompt={}", "Be very careful. ".repeat(40)),
            ]),
            dropped: vec!["--worktree".into()],
            model: Some("claude-opus-4-5[1m]".into()),
        }
    }

    fn knows(_: &str) -> Option<Held> {
        held(CONV, 4242, started_with_options())
    }

    fn knows_nothing(_: &str) -> Option<Held> {
        None
    }

    fn knows_another(_: &str) -> Option<Held> {
        held(
            "11111111-2222-4333-8444-555555555555",
            4243,
            Moved::default(),
        )
    }

    /// The same conversation, after a /model since the question.
    fn knows_other_model(_: &str) -> Option<Held> {
        let mut c = started_with_options();
        c.model = Some("claude-haiku-4-5".into());
        held(CONV, 4242, c)
    }

    /// A session started before this version, whose status line noted no
    /// model either.
    fn knows_only_the_conversation(_: &str) -> Option<Held> {
        held(CONV, 4242, Moved::default())
    }

    /// Notes a move as its process would once done (this test process
    /// stands in for it), without Herdr.
    fn noted(
        dir: &std::path::Path,
        list: u32,
        p: relocate::Plan,
        result: Option<Result<String, String>>,
    ) {
        assert!(relocate::write_note(
            dir,
            &relocate::Note {
                plan: p,
                at: crate::time::now(),
                list: Some(list),
                mover: Some(std::process::id()),
                result,
            }
        ));
    }

    /// Hands back what it was asked to do.
    fn moves(dir: &std::path::Path, list: u32, p: relocate::Plan) -> Result<(), String> {
        let text = format!(
            "moved {} {} {} {} to {} with {}",
            p.pane,
            p.label,
            p.session_id,
            p.pid,
            p.account_id,
            p.carry.args().len()
        );
        noted(dir, list, p, Some(Ok(text)));
        Ok(())
    }

    const FAILED: &str = "Claude Code did not end in it, so nothing was started. By hand: type /exit in it, then HERDR_MULTIACCOUNT_USE=work claude --resume x.";

    fn fails(dir: &std::path::Path, list: u32, p: relocate::Plan) -> Result<(), String> {
        noted(dir, list, p, Some(Err(FAILED.into())));
        Ok(())
    }

    /// A move whose process runs (this test process stands in for it).
    fn runs(dir: &std::path::Path, list: u32, p: relocate::Plan) -> Result<(), String> {
        noted(dir, list, p, None);
        Ok(())
    }

    /// A move whose process could not be started.
    fn cannot_start(_: &std::path::Path, _: u32, _: relocate::Plan) -> Result<(), String> {
        Err("Nothing was changed: the move could not be started. By hand: type /exit in it.".into())
    }

    fn signed_in(plan: &str) -> Info {
        Info {
            signed_in: true,
            plan: Some(plan.into()),
            ..Info::default()
        }
    }

    /// The Agents tab inside Herdr, with Personal (where both sessions
    /// run), Work and the main login, all signed in.
    fn movable() -> App {
        let mut a = agents();
        a.in_herdr = true;
        a.conversation_of = knows;
        a.mover = moves;
        a.store.accounts.push(Account {
            id: "work".into(),
            name: "Work".into(),
            kind: Kind::Subscription,
            sso: false,
            fallback: true,
        });
        a.info_loading = false;
        a.info = HashMap::new();
        for (id, plan) in [("main", "Max 5x"), ("personal", "Pro"), ("work", "Team")] {
            a.info.insert(id.into(), signed_in(plan));
        }
        a
    }

    fn picker(a: &App) -> Picker {
        match &a.mode {
            Mode::Pick(p) => p.clone(),
            m => panic!("no picker open: {m:?}"),
        }
    }

    fn names(p: &Picker) -> Vec<String> {
        p.items.iter().map(|i| i.label.clone()).collect()
    }

    fn flash_text(a: &App) -> String {
        a.flash.as_ref().map(|f| f.0.clone()).unwrap_or_default()
    }

    /// Waits for the move started in the background.
    fn finish(a: &mut App) {
        for _ in 0..200 {
            a.next_moves = Instant::now();
            a.receive_move();
            if a.moving.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the move never finished");
    }

    fn full_at(pct: f64, now: i64) -> Usage {
        let mut u = Usage::default();
        u.record(
            &serde_json::json!({"rate_limits": {"five_hour": {"used_percentage": pct, "resets_at": now + 3600}}}),
            now,
        );
        u
    }

    #[test]
    fn m_offers_the_other_signed_in_accounts_but_not_its_own_login() {
        let mut a = movable();
        // "Old" has the same login as Personal; "Unchecked" is not checked
        // yet; "Borrowed" has no login of its own.
        for (id, name) in [
            ("old", "Old"),
            ("unchecked", "Unchecked"),
            ("borrowed", "Borrowed"),
        ] {
            a.store.accounts.push(Account {
                id: id.into(),
                name: name.into(),
                kind: Kind::Subscription,
                sso: false,
                fallback: true,
            });
        }
        a.info.insert("old".into(), signed_in("Pro"));
        a.info.insert(
            "borrowed".into(),
            Info {
                signed_in: true,
                borrows_main: true,
                ..Info::default()
            },
        );
        a.twins.insert("personal".into(), vec!["old".into()]);
        press(&mut a, KeyCode::Char('m'));
        let p = picker(&a);
        assert_eq!(
            names(&p),
            vec!["Main login".to_string(), "Work".to_string()]
        );
        assert_eq!(p.title, "Move Fix the login bug to");
        assert_eq!(p.items[1].detail, "Team");
    }

    #[test]
    fn a_session_without_known_account_counts_as_the_main_login() {
        let mut a = movable();
        a.panes[0].account = None;
        a.panes[0].account_id = None;
        press(&mut a, KeyCode::Char('m'));
        assert_eq!(
            names(&picker(&a)),
            vec!["Personal".to_string(), "Work".to_string()]
        );
    }

    #[test]
    fn picking_and_yes_moves_that_conversation_to_that_account() {
        let mut a = movable();
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Char('2'));
        let c = confirm(&a);
        assert_eq!(c.question, "Move Fix the login bug to Work?");
        let text = c.lines.join(" ");
        assert!(text.contains("slower"), "{text}");
        assert!(text.contains("counts more against Work"), "{text}");
        assert!(text.contains("unsent text in it is lost"), "{text}");
        assert!(!text.contains("limit,"), "no fallback note: {text}");
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(
            a.moving.as_ref().map(|m| m.label.as_str()),
            Some("Fix the login bug")
        );
        finish(&mut a);
        assert_eq!(
            flash_text(&a),
            format!("moved w91:p1 Fix the login bug {CONV} 4242 to work with 4")
        );
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn a_busy_session_is_not_moved() {
        for (status, says) in [
            (Some("working"), "it is working"),
            (Some("blocked"), "waits for your answer"),
            (None, "cannot tell"),
        ] {
            let mut a = movable();
            a.panes[0].status = status.map(str::to_string);
            press(&mut a, KeyCode::Char('m'));
            assert_eq!(a.mode, Mode::Normal, "{status:?}");
            assert!(
                flash_text(&a).contains(says),
                "{status:?}: {}",
                flash_text(&a)
            );
            assert!(a.moving.is_none());
        }
        // Done counts as waiting for you.
        let mut a = movable();
        a.panes[0].status = Some("done".into());
        press(&mut a, KeyCode::Char('m'));
        picker(&a);
    }

    #[test]
    fn a_session_that_got_busy_or_changed_before_yes_stays() {
        type Change = fn(&mut App);
        let cases: [(Change, &str); 4] = [
            (
                |a| {
                    let mut now = a.panes.clone();
                    now[0].status = Some("working".into());
                    set_fresh(Some(now));
                },
                "it is working",
            ),
            (
                |_| set_fresh(Some(vec![shell("w91:p1", "w91:t1")])),
                "has ended",
            ),
            (
                |a| a.conversation_of = knows_another,
                "holds another conversation now",
            ),
            (|_| set_fresh(None), "Herdr could not be asked"),
        ];
        for (change, says) in cases {
            let mut a = movable();
            press(&mut a, KeyCode::Char('m'));
            press(&mut a, KeyCode::Enter);
            change(&mut a);
            press(&mut a, KeyCode::Char('y'));
            assert!(a.moving.is_none(), "{says}");
            refused(&a, says);
        }
    }

    /// The dialog a move refused after yes opens, naming why.
    fn refused(a: &App, says: &str) {
        match &a.mode {
            Mode::Message { title, lines, good } => {
                assert_eq!(title, "Fix the login bug was not moved", "{says}");
                let text = lines.join(" ");
                assert!(text.contains(says), "{says}: {text}");
                assert!(!text.contains("nothing was moved"), "said once: {text}");
                assert!(!good);
            }
            m => panic!("{says}: no dialog: {m:?}"),
        }
    }

    /// Opens the move question for Fix the login bug to Work.
    fn asked_to_move() -> App {
        let mut a = movable();
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Char('2'));
        assert_eq!(confirm(&a).question, "Move Fix the login bug to Work?");
        a
    }

    #[test]
    fn enter_on_the_move_question_moves() {
        // The steps of #10: m, Enter on the account, Enter on the question.
        let mut a = movable();
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Enter);
        confirm(&a);
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.moving.is_some(), "the move runs");
        finish(&mut a);
        assert!(
            flash_text(&a).starts_with("moved w91:p1 Fix the login bug"),
            "{}",
            flash_text(&a)
        );
    }

    #[test]
    fn no_on_the_move_question_says_the_session_was_not_moved() {
        for key in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut a = asked_to_move();
            press(&mut a, key);
            assert_eq!(a.mode, Mode::Normal, "{key:?}");
            assert!(a.moving.is_none(), "{key:?}");
            assert_eq!(flash_text(&a), "Fix the login bug was not moved", "{key:?}");
            assert_eq!(a.flash.as_ref().map(|f| f.1), Some(crate::theme::MUTED));
        }
        // A click on the no button too.
        let mut a = asked_to_move();
        screen_of(&mut a, 100, 30);
        let spot = a
            .hits
            .iter()
            .find_map(|(r, h)| (*h == Hit::Key('n')).then_some(*r))
            .expect("the no button");
        a.click(spot.x, spot.y);
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.moving.is_none());
        assert_eq!(flash_text(&a), "Fix the login bug was not moved");
    }

    #[test]
    fn leaving_the_move_picker_says_the_session_was_not_moved() {
        for key in [KeyCode::Esc, KeyCode::Char('q')] {
            let mut a = movable();
            press(&mut a, KeyCode::Char('m'));
            picker(&a);
            press(&mut a, key);
            assert_eq!(a.mode, Mode::Normal, "{key:?}");
            assert!(a.moving.is_none(), "{key:?}");
            assert_eq!(flash_text(&a), "Fix the login bug was not moved", "{key:?}");
        }
    }

    #[test]
    fn other_keys_leave_the_move_question_open() {
        for key in [
            KeyCode::Char('x'),
            KeyCode::Char('Y'),
            KeyCode::Char(' '),
            KeyCode::Tab,
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('m'),
        ] {
            let mut a = asked_to_move();
            screen_of(&mut a, 160, 40);
            assert_eq!(a.confirm_max, 0, "the question fits");
            let before = confirm(&a);
            press(&mut a, key);
            assert_eq!(confirm(&a), before, "{key:?}");
            assert!(a.moving.is_none(), "{key:?}");
            assert_eq!(flash_text(&a), "", "{key:?}");
        }
    }

    #[test]
    fn staying_after_ctrl_c_keeps_the_question() {
        let mut a = asked_to_move();
        let before = confirm(&a);
        a.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(a.mode, Mode::AskQuit);
        press(&mut a, KeyCode::Char('n'));
        assert_eq!(confirm(&a), before);
        press(&mut a, KeyCode::Char('y'));
        assert!(a.moving.is_some(), "the question still answers");
        // The move's account list comes back too.
        let mut a = movable();
        press(&mut a, KeyCode::Char('m'));
        let before = picker(&a);
        a.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        press(&mut a, KeyCode::Char('n'));
        assert_eq!(picker(&a), before);
        // q from the list itself stays in the list.
        let mut a = movable();
        a.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        press(&mut a, KeyCode::Char('n'));
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Esc);
        press(&mut a, KeyCode::Char('q'));
        press(&mut a, KeyCode::Char('n'));
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn keys_with_ctrl_or_alt_never_answer() {
        for mods in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            for code in [KeyCode::Char('y'), KeyCode::Char('n'), KeyCode::Enter] {
                let mut a = asked_to_move();
                let before = confirm(&a);
                a.key(KeyEvent::new(code, mods));
                assert_eq!(confirm(&a), before, "{mods:?} {code:?}");
                assert!(a.moving.is_none());
                let mut a = agents();
                press(&mut a, KeyCode::Char('d'));
                a.key(KeyEvent::new(code, mods));
                confirm(&a);
                assert!(closed().is_empty(), "{mods:?} {code:?}");
            }
        }
    }

    #[test]
    fn with_no_other_account_m_says_so() {
        let mut a = movable();
        a.info.remove("main");
        a.info.remove("work");
        press(&mut a, KeyCode::Char('m'));
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(
            flash_text(&a),
            "No other signed-in account to move Fix the login bug to"
        );
    }

    #[test]
    fn an_unknown_conversation_names_the_way_by_hand() {
        let mut a = movable();
        a.conversation_of = knows_nothing;
        press(&mut a, KeyCode::Char('m'));
        match &a.mode {
            Mode::Message { title, lines, .. } => {
                assert!(title.contains("not known"), "{title}");
                let text = lines.join(" ");
                assert!(text.contains("/exit"), "{text}");
                assert!(text.contains("claude --resume"), "{text}");
                assert!(text.contains("setup (S)"), "{text}");
            }
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn a_failed_move_is_shown_with_the_way_by_hand() {
        let mut a = movable();
        a.mover = fails;
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Enter);
        press(&mut a, KeyCode::Char('y'));
        finish(&mut a);
        match &a.mode {
            Mode::Message { title, lines, good } => {
                assert_eq!(title, "Fix the login bug was not moved");
                assert!(lines.join(" ").contains("/exit"));
                assert!(!good);
            }
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn a_failed_move_waits_for_the_dialog_the_user_is_in() {
        let mut a = movable();
        a.mover = fails;
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Enter);
        press(&mut a, KeyCode::Char('y'));
        // The user opened the keys overlay meanwhile.
        a.mode = Mode::Help;
        for _ in 0..200 {
            a.receive_move();
            if a.moving.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(a.moving.is_none(), "the move finished");
        assert_eq!(a.mode, Mode::Help, "the overlay stays");
        a.mode = Mode::Normal;
        poll(&mut a);
        match &a.mode {
            Mode::Message { title, .. } => assert_eq!(title, "Fix the login bug was not moved"),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn outside_herdr_m_says_so() {
        let mut a = movable();
        a.in_herdr = false;
        press(&mut a, KeyCode::Char('m'));
        assert_eq!(a.mode, Mode::Normal);
        assert_eq!(flash_text(&a), "Only inside Herdr");
    }

    #[test]
    fn an_account_at_its_limit_is_offered_and_hands_over_to_the_next() {
        let mut a = movable();
        let now = a.now();
        a.store.accounts.push(Account {
            id: "spare".into(),
            name: "Spare".into(),
            kind: Kind::Subscription,
            sso: false,
            fallback: true,
        });
        a.info.insert("spare".into(), signed_in("Pro"));
        a.usage.insert("work".into(), full_at(99.0, now));
        press(&mut a, KeyCode::Char('m'));
        let p = picker(&a);
        let work = p.items.iter().position(|i| i.label == "Work").unwrap();
        assert!(
            p.items[work].detail.starts_with("at its 5-hour limit"),
            "{}",
            p.items[work].detail
        );
        press(&mut a, KeyCode::Char(char::from(b'1' + work as u8)));
        let c = confirm(&a);
        assert_eq!(c.question, "Move Fix the login bug to Spare?");
        assert!(
            c.lines[0].contains("Work is at its 5-hour limit, so it goes to Spare"),
            "{}",
            c.lines[0]
        );
        press(&mut a, KeyCode::Char('y'));
        finish(&mut a);
        assert!(
            flash_text(&a).ends_with("to spare with 4"),
            "{}",
            flash_text(&a)
        );
    }

    #[test]
    fn fallback_never_goes_back_to_its_own_login_and_otherwise_stays() {
        let mut a = movable();
        let now = a.now();
        // Work is full; next in line is Personal (its own) and then main,
        // which is full too: nothing has room.
        a.usage.insert("work".into(), full_at(99.0, now));
        a.usage.insert("main".into(), full_at(99.0, now));
        let c = a.move_target("personal", "work");
        assert_eq!(c.id, "work");
        press(&mut a, KeyCode::Char('m'));
        let p = picker(&a);
        let work = p.items.iter().position(|i| i.label == "Work").unwrap();
        press(&mut a, KeyCode::Char(char::from(b'1' + work as u8)));
        let c = confirm(&a);
        assert_eq!(c.question, "Move Fix the login bug to Work?");
        assert!(
            c.lines[0].contains("no other account has room"),
            "{}",
            c.lines[0]
        );
        // With fallback off it stays on the pick and says why.
        let mut a = movable();
        a.config.fallback = false;
        a.usage.insert("work".into(), full_at(99.0, now));
        assert_eq!(a.move_target("personal", "work").id, "work");
        press(&mut a, KeyCode::Char('m'));
        let p = picker(&a);
        let work = p.items.iter().position(|i| i.label == "Work").unwrap();
        press(&mut a, KeyCode::Char(char::from(b'1' + work as u8)));
        assert!(confirm(&a).lines[0].contains("fallback is off"));
    }

    #[test]
    fn a_move_names_its_account_so_no_reminder_follows() {
        // The command names the account (HERDR_MULTIACCOUNT_USE): such a
        // start never falls back, so the launcher notes no account to go
        // back to and the reminder of an earlier fallback ends.
        let a = movable();
        let c = launch::choose(
            &a.store,
            &a.config,
            &a.usage,
            &|_| true,
            "work".into(),
            launch::Source::Explicit,
            a.now(),
        );
        assert_eq!(c.id, c.wanted);
        let note = crate::sessions::Entry {
            account: c.id.clone(),
            pid: 1,
            wanted: (c.id != c.wanted).then(|| c.wanted.clone()),
            told: false,
            options: None,
            dropped: vec![],
        };
        assert_eq!(
            crate::remind::due(&note, "work", &a.store, &Usage::default(), 95.0, a.now()),
            None
        );
    }

    #[test]
    fn while_a_move_runs_the_session_is_kept_and_q_closes_the_list() {
        let mut a = movable();
        a.mover = runs;
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Enter);
        press(&mut a, KeyCode::Char('y'));
        a.receive_move();
        assert!(a.moving.is_some());
        press(&mut a, KeyCode::Char('d'));
        assert_eq!(a.mode, Mode::Normal);
        press(&mut a, KeyCode::Char('m'));
        assert_eq!(a.mode, Mode::Normal);
        assert!(flash_text(&a).contains("being moved"));
        // The move goes on in its own process (the owner's decision on
        // #12), so q closes the list, as Ctrl+C does.
        press(&mut a, KeyCode::Char('q'));
        press(&mut a, KeyCode::Char('y'));
        assert!(a.quit);
        assert!(
            relocate::read_note(&a.moves_dir, "w91:p1").is_some(),
            "the move is left to its process"
        );
        let mut b = movable();
        b.mover = runs;
        press(&mut b, KeyCode::Char('m'));
        press(&mut b, KeyCode::Enter);
        press(&mut b, KeyCode::Char('y'));
        b.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        press(&mut b, KeyCode::Char('y'));
        assert!(b.quit);
    }

    /// Reads the notes again now, as the list does every half second.
    fn poll(a: &mut App) {
        a.next_moves = Instant::now();
        a.receive_move();
    }

    #[test]
    fn a_move_started_by_a_closed_list_reports_in_the_next_one() {
        let mut a = movable();
        a.mover = runs;
        // Its process is gone once the list closed.
        a.me = dead_pid();
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Enter);
        press(&mut a, KeyCode::Char('y'));
        let dir = a.moves_dir.clone();
        let note = relocate::read_note(&dir, "w91:p1").unwrap();
        assert_eq!(note.list, Some(a.me));
        drop(a);
        // The move process finishes after the list closed.
        let ok = relocate::Note {
            result: Some(Ok("Moved Fix the login bug to Work".into())),
            ..note.clone()
        };
        assert!(relocate::write_note(&dir, &ok));
        let mut b = movable();
        b.moves_dir = dir.clone();
        poll(&mut b);
        assert_eq!(flash_text(&b), "Moved Fix the login bug to Work");
        assert!(b.moving.is_none());
        assert!(relocate::notes(&dir).is_empty(), "told once, then gone");
        // A failure while the next list is in a dialog stays in its note
        // (the move process's popup tells it if the list closes), and
        // shows once the dialog is closed.
        let failed = relocate::Note {
            result: Some(Err(FAILED.into())),
            ..note
        };
        assert!(relocate::write_note(&dir, &failed));
        let mut c = movable();
        c.moves_dir = dir.clone();
        c.mode = Mode::Help;
        poll(&mut c);
        assert_eq!(c.mode, Mode::Help);
        assert_eq!(relocate::notes(&dir).len(), 1, "left in its note");
        c.mode = Mode::Normal;
        poll(&mut c);
        match &c.mode {
            Mode::Message { title, lines, good } => {
                assert_eq!(title, "Fix the login bug was not moved");
                assert!(lines.join(" ").contains("/exit"));
                assert!(!good);
            }
            m => panic!("{m:?}"),
        }
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "gone once shown"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_leaves_the_result_of_another_open_list_to_it() {
        let mut a = asked_to_move();
        a.mover = fails;
        let mut b = movable();
        b.moves_dir = a.moves_dir.clone();
        // Another list, whose process (pid 1) runs.
        b.me = 1;
        press(&mut a, KeyCode::Char('y'));
        poll(&mut b);
        assert_eq!(b.mode, Mode::Normal, "not in a tab the user may not see");
        poll(&mut a);
        refused(&a, "did not end");
        let _ = std::fs::remove_dir_all(&a.moves_dir);
    }

    /// A pid no process has: one that just ended.
    fn dead_pid() -> u32 {
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    }

    #[test]
    fn a_list_opened_during_a_move_shows_it_and_refuses_m_and_d() {
        let mut a = asked_to_move();
        a.mover = runs;
        // The list that started it has closed.
        a.me = dead_pid();
        press(&mut a, KeyCode::Char('y'));
        let mut b = movable();
        b.moves_dir = a.moves_dir.clone();
        poll(&mut b);
        assert_eq!(
            b.moving.as_ref().map(|m| (m.label.as_str(), m.to.as_str())),
            Some(("Fix the login bug", "Work"))
        );
        let rows = screen_of(&mut b, 160, 40);
        assert!(
            rows[39].contains("Moving Fix the login bug to Work…"),
            "{}",
            rows[39]
        );
        for key in ['m', 'd'] {
            press(&mut b, KeyCode::Char(key));
            assert_eq!(b.mode, Mode::Normal, "{key}");
            assert!(
                flash_text(&b).contains("being moved; wait a moment"),
                "{key}"
            );
        }
        // The result shows there when the move ends.
        let n = relocate::read_note(&b.moves_dir, "w91:p1").unwrap();
        noted(
            &b.moves_dir,
            n.list.unwrap(),
            n.plan,
            Some(Ok("Moved Fix the login bug to Work".into())),
        );
        poll(&mut b);
        assert!(b.moving.is_none());
        assert_eq!(flash_text(&b), "Moved Fix the login bug to Work");
        let _ = std::fs::remove_dir_all(&b.moves_dir);
    }

    #[test]
    fn two_lists_run_one_move_at_a_time() {
        let mut a = movable();
        a.mover = runs;
        let mut b = movable();
        b.mover = runs;
        b.moves_dir = a.moves_dir.clone();
        // b asks before a starts, and answers after.
        press(&mut b, KeyCode::Char('j'));
        press(&mut b, KeyCode::Char('m'));
        press(&mut b, KeyCode::Enter);
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Enter);
        press(&mut a, KeyCode::Char('y'));
        press(&mut b, KeyCode::Char('y'));
        match &b.mode {
            Mode::Message { title, lines, .. } => {
                assert_eq!(title, "Write the docs was not moved");
                assert!(
                    lines[0].contains("Fix the login bug is being moved"),
                    "{lines:?}"
                );
            }
            m => panic!("{m:?}"),
        }
        assert_eq!(relocate::notes(&a.moves_dir).len(), 1);
        let _ = std::fs::remove_dir_all(&a.moves_dir);
    }

    #[test]
    fn a_move_whose_process_is_gone_is_reported_once() {
        let a = movable();
        let dir = a.moves_dir.clone();
        let plan = relocate::Plan {
            pane: "w91:p1".into(),
            label: "Fix the login bug".into(),
            session_id: CONV.into(),
            pid: 4242,
            account_id: "work".into(),
            account_name: "Work".into(),
            carry: started_with_options(),
        };
        // Killed halfway, and one that never noted itself.
        let now = crate::time::now();
        for (mover, at) in [(Some(dead_pid()), now), (None, now - 31)] {
            assert!(relocate::write_note(
                &dir,
                &relocate::Note {
                    plan: plan.clone(),
                    at,
                    list: Some(std::process::id()),
                    mover,
                    result: None,
                }
            ));
            let mut b = movable();
            b.moves_dir = dir.clone();
            poll(&mut b);
            assert!(b.moving.is_none());
            match &b.mode {
                Mode::Message { title, lines, good } => {
                    assert_eq!(
                        title,
                        "The move of Fix the login bug stopped before it finished"
                    );
                    let text = lines.join(" ");
                    assert!(text.contains("Look at the pane"), "{text}");
                    assert!(
                        text.contains(&format!(
                            "HERDR_MULTIACCOUNT_USE=work claude --resume {CONV} --model 'claude-opus-4-5[1m]'"
                        )),
                        "{text}"
                    );
                    assert!(!good);
                }
                m => panic!("{m:?}"),
            }
            b.mode = Mode::Normal;
            poll(&mut b);
            assert_eq!(b.mode, Mode::Normal, "said once");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_move_that_cannot_start_presses_nothing_and_says_so() {
        let mut a = movable();
        a.mover = cannot_start;
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Enter);
        press(&mut a, KeyCode::Char('y'));
        assert!(a.moving.is_none());
        refused(&a, "could not be started");
    }

    #[test]
    fn a_running_move_shows_until_its_result() {
        let mut a = asked_to_move();
        press(&mut a, KeyCode::Char('y'));
        assert!(a.moving.is_some());
        // Past the 5 seconds of a flash, and during a refresh as well.
        a.flash = None;
        for (loading, probing) in [(false, false), (true, false), (false, true)] {
            a.info_loading = loading;
            a.probing = probing;
            for (w, h) in [(160, 40), (100, 30), (60, 20)] {
                let rows = screen_of(&mut a, w, h);
                let footer = &rows[h as usize - 1];
                assert!(
                    footer.contains("Moving Fix the login bug to Work…"),
                    "{w}x{h} {loading} {probing}: {footer}"
                );
                let screen = rows.join("\n");
                assert!(screen.contains("moving to Work…"), "{w}x{h}: {screen}");
            }
        }
        // A newer note shows for its time, then the move again.
        a.flash(
            "Write the docs is being moved; wait a moment",
            crate::theme::MUTED,
        );
        let rows = screen_of(&mut a, 160, 40);
        assert!(rows[39].contains("wait a moment"), "{}", rows[39]);
        a.flash = None;
        a.info_loading = false;
        a.probing = false;
        finish(&mut a);
        let rows = screen_of(&mut a, 160, 40);
        assert!(rows[39].contains("moved w91:p1"), "{}", rows[39]);
        let screen = rows.join("\n");
        assert!(!screen.contains("oving to Work…"), "{screen}");
    }

    #[test]
    fn a_long_session_name_leaves_the_footer_its_keys() {
        let mut a = movable();
        a.moving = Some(MoveRun {
            pane: "w91:p1".into(),
            label: "Refactor the whole billing module and write the migration notes".into(),
            to: "Work".into(),
        });
        let rows = screen_of(&mut a, 100, 30);
        assert!(rows[29].contains("m move"), "{}", rows[29]);
        assert!(rows[29].contains("Moving Refactor"), "{}", rows[29]);
        assert!(rows[29].contains("… to Work…"), "{}", rows[29]);
    }

    #[test]
    fn the_buttons_name_the_keys_at_every_size() {
        for (w, h) in [(160, 40), (100, 30), (60, 20), (40, 12), (24, 20), (20, 30)] {
            let mut a = asked_to_move();
            let screen = screen_of(&mut a, w, h).join("\n");
            assert!(screen.contains("y yes "), "{w}x{h}: {screen}");
            assert!(
                screen.contains(" n no "),
                "{w}x{h}: the whole no button\n{screen}"
            );
            if w >= 40 {
                assert!(screen.contains(" ⏎ y yes "), "{w}x{h}: {screen}");
                assert!(screen.contains(" Esc n no "), "{w}x{h}: {screen}");
            }
            // The no button answers no where it is drawn.
            let spot = a
                .hits
                .iter()
                .find_map(|(r, h)| (*h == Hit::Key('n')).then_some(*r))
                .expect("the no button");
            assert!(spot.x + spot.width <= w, "{w}x{h}");
            a.click(spot.x, spot.y);
            assert_eq!(flash_text(&a), "Fix the login bug was not moved", "{w}x{h}");
        }
    }

    #[test]
    fn the_keys_overlay_says_how_questions_are_answered() {
        let mut a = movable();
        a.mode = Mode::Help;
        let mut seen = screen_of(&mut a, 120, 60).join("\n");
        for _ in 0..a.help_max {
            press(&mut a, KeyCode::Down);
            seen.push_str(&screen_of(&mut a, 120, 60).join("\n"));
        }
        assert!(seen.contains("Questions"), "{seen}");
        assert!(
            seen.contains("yes on the move question; nothing on the others"),
            "{seen}"
        );
        assert!(
            seen.contains("any other key leaves the question open"),
            "{seen}"
        );
    }

    #[test]
    fn the_footer_and_the_keys_overlay_show_m() {
        let mut a = movable();
        let rows = screen_of(&mut a, 100, 30);
        assert!(rows[29].contains("m move"), "{}", rows[29]);
        a.mode = Mode::Help;
        let screen = screen_of(&mut a, 120, 60).join("\n");
        assert!(screen.contains("move it to another account"), "{screen}");
        let mut a = movable();
        screen_of(&mut a, 100, 30);
        let spot = a
            .hits
            .iter()
            .find_map(|(r, h)| (*h == Hit::Key('m')).then_some(*r))
            .expect("m in the footer");
        a.click(spot.x, spot.y);
        picker(&a);
    }

    #[test]
    fn the_question_names_what_carries_over_and_what_not() {
        let mut a = movable();
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Char('2'));
        let c = confirm(&a);
        let text = c.lines.join("\n");
        assert!(
            text.contains("Carries over: model claude-opus-4-5[1m], --add-dir, --permission-mode plan, --append-system-prompt."),
            "{text}"
        );
        assert!(text.contains("Not carried: --worktree."), "{text}");
        assert!(!text.contains("Be very careful"), "no long values: {text}");
        // Old sessions: says the options are not known.
        let mut a = movable();
        a.conversation_of = knows_only_the_conversation;
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Char('2'));
        let text = confirm(&a).lines.join(" ");
        assert!(
            text.contains("start options and model are not known"),
            "{text}"
        );
        press(&mut a, KeyCode::Char('y'));
        finish(&mut a);
        assert!(flash_text(&a).ends_with("with 0"), "{}", flash_text(&a));
    }

    #[test]
    fn a_model_changed_after_the_question_stops_the_move() {
        let mut a = movable();
        press(&mut a, KeyCode::Char('m'));
        press(&mut a, KeyCode::Char('2'));
        a.conversation_of = knows_other_model;
        press(&mut a, KeyCode::Char('y'));
        assert!(a.moving.is_none(), "nothing started");
        refused(&a, "model changed");
    }

    #[test]
    fn start_options_count_only_for_the_process_that_was_started_with_them() {
        let note = crate::sessions::Conversation {
            session_id: CONV.into(),
            pids: vec![4242],
            model: Some("opus".into()),
        };
        let launcher = |pid| crate::sessions::Entry {
            account: "work".into(),
            pid,
            wanted: None,
            told: false,
            options: Some(vec!["--verbose".into()]),
            dropped: vec!["--worktree".into()],
        };
        let h = held_from(note.clone(), 4242, Some(launcher(4242)));
        assert_eq!(h.carry.options, Some(vec!["--verbose".to_string()]));
        assert_eq!(h.carry.dropped, vec!["--worktree".to_string()]);
        assert_eq!(h.carry.model.as_deref(), Some("opus"));
        // An older Claude Code's note in the same pane: options unknown,
        // the current model still kept.
        let h = held_from(note.clone(), 4242, Some(launcher(1111)));
        assert_eq!(h.carry.options, None);
        assert!(h.carry.dropped.is_empty());
        assert_eq!(h.carry.model.as_deref(), Some("opus"));
        let h = held_from(note, 4242, None);
        assert_eq!(h.carry.options, None);
    }

    #[test]
    fn the_picker_and_the_question_draw_at_many_sizes() {
        for (w, h, full) in [
            (160, 40, false),
            (100, 30, true),
            (60, 20, false),
            (40, 12, false),
            (40, 12, true),
            (24, 20, true),
            (20, 30, true),
        ] {
            let mut a = movable();
            if full {
                // Work is full and hands over to Main login: one line more.
                let now = a.now();
                a.usage.insert("work".into(), full_at(99.0, now));
            }
            press(&mut a, KeyCode::Char('m'));
            let screen = screen_of(&mut a, w, h).join("\n");
            assert!(screen.contains("Work"), "{w}x{h}: {screen}");
            press(&mut a, KeyCode::Char('2'));
            let screen = screen_of(&mut a, w, h).join("\n");
            assert!(screen.contains("y yes"), "{w}x{h}: {screen}");
            // A question taller than the pane scrolls: every word shows on
            // the way down.
            let mut seen = screen.clone();
            for _ in 0..a.confirm_max {
                assert!(seen.contains("scroll"), "{w}x{h}: says it scrolls");
                press(&mut a, KeyCode::Down);
                seen.push('\n');
                seen.push_str(&screen_of(&mut a, w, h).join("\n"));
            }
            let screen = seen;
            let shown: Vec<&str> = screen
                .split(|c: char| c.is_whitespace() || c == '│')
                .collect();
            // The width the question's text wraps to: its box less 6
            // (`ui::draw_confirm`).
            let width = screen
                .lines()
                .find(|l| l.contains('╭'))
                .map(|l| l.trim().chars().count().saturating_sub(6))
                .unwrap_or(0);
            // Only a word longer than the line is split, across lines in a
            // row; the text without borders and spaces holds it whole.
            let flat: String = screen
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '│')
                .collect();
            let c = confirm(&a);
            // The title may be cut on a narrow pane; the text names where
            // the session goes.
            let to = if full { "Main login's" } else { "Work's" };
            assert!(c.lines.join(" ").contains(to), "{w}x{h}");
            for word in c.lines.iter().flat_map(|l| l.split_whitespace()) {
                let whole = if word.chars().count() > width {
                    flat.contains(word)
                } else {
                    shown.contains(&word)
                };
                assert!(whole, "{w}x{h}: {word} cut off\n{screen}");
            }
        }
    }
}
