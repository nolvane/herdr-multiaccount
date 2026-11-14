//! A claude.ai sign-in, run inside the list: Claude Code's own `claude auth
//! login` in the account's folder, with its output read and shown in a
//! dialog. Claude Code opens the browser and keeps the login; the list only
//! shows the link, passes on a code the page may show, and can cancel.

use crate::launch;
use crate::mirror;
use crate::store::Account;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

pub struct SignIn {
    pub id: String,
    pub name: String,
    /// The account was added for this sign-in; cancelling removes it again.
    pub new_account: bool,
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    /// Everything Claude Code printed, without terminal escape codes.
    pub output: String,
    pub url: Option<String>,
    /// `Some(true)` once the sign-in succeeded, `Some(false)` when it ended
    /// without.
    pub finished: Option<bool>,
    pub started: Instant,
    pub code_sent: bool,
    /// Ended by `cancel` (`Esc`), not by Claude Code.
    pub cancelled: bool,
}

/// Text without terminal escape codes (colours, and the link markers that
/// wrap the sign-in URL).
pub fn strip_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c == '\u{7}' {
                continue;
            }
            out.push(c);
            continue;
        }
        match chars.peek() {
            // OSC: up to BEL or ESC \
            Some(']') => {
                chars.next();
                while let Some(d) = chars.next() {
                    if d == '\u{7}' {
                        break;
                    }
                    if d == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // CSI: up to a final letter
            Some('[') => {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() || d == '~' {
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

/// The first https link in the text.
pub fn find_url(text: &str) -> Option<String> {
    let start = text.find("https://")?;
    let url: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '\u{7}' && *c != '\u{1b}')
        .collect();
    Some(url)
}

/// Claude Code's own `claude auth login` for a claude.ai account, in the
/// account's (new) folder, where every sign-in goes. Prepares the folder
/// first.
fn login_command(account: &Account) -> Result<Command, String> {
    let dir = mirror::new_dir_of(&account.id);
    mirror::sync_at(&account.id, &dir).map_err(|e| format!("could not prepare its folder: {e}"))?;
    let claude = launch::find_claude().ok_or("claude was not found on PATH")?;
    Ok(login_command_at(&claude, &dir, account.sso))
}

/// `claude auth login` with `claude`, signing in the folder `dir`.
fn login_command_at(claude: &Path, dir: &Path, sso: bool) -> Command {
    let mut cmd = Command::new(claude);
    // Clean first, so CLAUDE_CONFIG_DIR set below stays.
    launch::clean_env(&mut cmd);
    for v in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        cmd.env_remove(v);
    }
    cmd.args(["auth", "login", "--claudeai"]);
    if sso {
        cmd.arg("--sso");
    }
    cmd.env("CLAUDE_CONFIG_DIR", dir);
    launch::in_work_dir(&mut cmd);
    cmd
}

/// SIGINT, and the dispositions `signal` takes.
const SIGINT: i32 = 2;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

/// The sign-in in a terminal, for the launcher: Claude Code gets the
/// terminal and asks and prints what it needs. Ctrl+C cancels the sign-in
/// only: the launcher ignores it meanwhile and takes it back afterwards, so
/// it can go on (and the Claude Code it starts next gets Ctrl+C as usual).
/// Whether Claude Code reported success; the caller checks the login itself.
pub fn in_terminal(account: &Account) -> Result<bool, String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = login_command(account)?;
    // SAFETY: runs in the child between fork and exec; `signal` is
    // async-signal-safe. An ignored signal would stay ignored across exec.
    unsafe {
        cmd.pre_exec(|| {
            signal(SIGINT, SIG_DFL);
            Ok(())
        });
    }
    // SAFETY: plain libc calls on this process's own disposition.
    let before = unsafe { signal(SIGINT, SIG_IGN) };
    let status = cmd.status();
    unsafe { signal(SIGINT, before) };
    status.map(|s| s.success()).map_err(|e| e.to_string())
}

impl SignIn {
    pub fn start(account: &Account, new_account: bool) -> Result<SignIn, String> {
        SignIn::spawn(
            login_command(account)?,
            &account.id,
            &account.name,
            new_account,
        )
    }

    /// Runs `cmd` as the sign-in of the account `id`.
    fn spawn(mut cmd: Command, id: &str, name: &str, new_account: bool) -> Result<SignIn, String> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        for pipe in [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut pipe = pipe;
                let mut buf = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0
                        || tx
                            .send(String::from_utf8_lossy(&buf[..n]).to_string())
                            .is_err()
                    {
                        break;
                    }
                }
            });
        }
        Ok(SignIn {
            id: id.to_string(),
            name: name.to_string(),
            new_account,
            stdin: child.stdin.take(),
            child,
            rx,
            output: String::new(),
            url: None,
            finished: None,
            started: Instant::now(),
            code_sent: false,
            cancelled: false,
        })
    }

    /// A sign-in whose Claude Code is the shell script `script`, for tests.
    #[cfg(test)]
    pub fn scripted(script: &str, id: &str, name: &str, new_account: bool) -> SignIn {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        SignIn::spawn(cmd, id, name, new_account).unwrap()
    }

    /// Polls until Claude Code ended, for tests.
    #[cfg(test)]
    pub fn until_finished(&mut self) -> bool {
        let until = Instant::now() + std::time::Duration::from_secs(10);
        while self.finished.is_none() {
            assert!(Instant::now() < until, "the sign-in never ended");
            self.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.finished == Some(true)
    }

    /// A sign-in that shows `url`, for tests: a harmless child process
    /// stands in for Claude Code.
    #[cfg(test)]
    pub fn showing(url: &str) -> SignIn {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let (_tx, rx) = channel();
        SignIn {
            id: "personal".into(),
            name: "Personal".into(),
            new_account: false,
            stdin: child.stdin.take(),
            child,
            rx,
            output: String::new(),
            url: Some(url.into()),
            finished: None,
            started: Instant::now(),
            code_sent: false,
            cancelled: false,
        }
    }

    /// Takes in new output and notices when Claude Code is done.
    pub fn poll(&mut self) {
        let _ = self.take_output();
        if self.finished.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                // Late output still in the pipes: until both are closed,
                // at most half a second (something Claude Code started may
                // keep them open).
                let until = Instant::now() + std::time::Duration::from_millis(500);
                while !self.take_output() && Instant::now() < until {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                self.finished = Some(
                    status.success() && self.output.to_lowercase().contains("login successful"),
                );
            }
        }
    }

    /// Takes the output that arrived since the last call, and the sign-in
    /// link once it shows. True once both pipes are closed.
    fn take_output(&mut self) -> bool {
        let mut raw = String::new();
        let closed = loop {
            match self.rx.try_recv() {
                Ok(chunk) => raw.push_str(&chunk),
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        if !raw.is_empty() {
            if self.url.is_none() {
                self.url = find_url(&raw);
            }
            self.output.push_str(&strip_escapes(&raw));
        }
        closed
    }

    /// Passes a code from the sign-in page on to Claude Code.
    pub fn send_code(&mut self, code: &str) -> bool {
        let Some(stdin) = self.stdin.as_mut() else {
            return false;
        };
        let ok = stdin
            .write_all(format!("{}\n", code.trim()).as_bytes())
            .and_then(|_| stdin.flush())
            .is_ok();
        self.code_sent = ok;
        ok
    }

    pub fn cancel(&mut self) {
        if self.finished.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.finished = Some(false);
            self.cancelled = true;
        }
    }

    /// What Claude Code printed after the sign-in link (all of it when it
    /// showed none), line by line, for a sign-in that failed.
    pub fn said(&self) -> String {
        let after = self
            .url
            .as_deref()
            .and_then(|u| self.output.rfind(u).map(|i| i + u.len()))
            .unwrap_or(0);
        let lines: Vec<&str> = self.output[after..].lines().map(str::trim_end).collect();
        lines.join("\n").trim_matches('\n').to_string()
    }
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_link() {
        let raw = "Opening browser to sign in…\nIf the browser didn't open, visit: \u{1b}]8;;https://claude.example/oauth?x=1&y=2\u{7}https://claude.example/oauth?x=1&y=2\u{1b}]8;;\u{7}\nPaste code here if prompted > \u{1b}[32mok\u{1b}[0m";
        assert_eq!(
            find_url(raw).as_deref(),
            Some("https://claude.example/oauth?x=1&y=2")
        );
        let clean = strip_escapes(raw);
        assert!(clean.contains("visit: https://claude.example/oauth?x=1&y=2\n"));
        assert!(clean.ends_with("prompted > ok"));
        assert!(!clean.contains('\u{1b}'));
    }

    /// #25: Claude Code reads its working folder when it starts, so the
    /// sign-in runs in the plugin's own folder, never where the list was
    /// opened (which may be gone or blocked by then).
    #[test]
    fn the_sign_in_runs_in_the_work_folder() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("hm-signin-cwd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("account");
        std::fs::create_dir_all(&dir).unwrap();
        let claude = root.join("claude");
        std::fs::write(
            &claude,
            "#!/bin/sh\npwd -P > \"$CLAUDE_CONFIG_DIR/cwd\"\necho \"$*\" > \"$CLAUDE_CONFIG_DIR/args\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Another test thread may still hold the file open while it starts
        // a process ("text file busy"): try again for a moment.
        let mut status = None;
        for _ in 0..100 {
            match login_command_at(&claude, &dir, true).status() {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                other => {
                    status = Some(other.unwrap());
                    break;
                }
            }
        }
        assert!(status.unwrap().success());
        let cwd = std::fs::read_to_string(dir.join("cwd")).unwrap();
        let work = launch::work_dir().canonicalize().unwrap();
        assert_eq!(cwd.trim(), work.to_string_lossy());
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert_eq!(args.trim(), "auth login --claudeai --sso");
        let _ = std::fs::remove_dir_all(&root);
    }
}
