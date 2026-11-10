//! Setup: the two places outside Herdr the plugin needs.
//!
//! 1. A `claude` shell function in `~/.zshrc` (or `~/.bashrc`), so every
//!    `claude` you type starts with the right account.
//! 2. Claude Code's status line command wrapped by `herdr-multiaccount
//!    statusline`, the only way Claude Code shares the usage windows. The
//!    command you had keeps running after it, with the same input.
//!
//! Both are undone by `herdr-multiaccount setup --undo`. Every file is backed up
//! to `backups/` before it changes.

use crate::paths;
use crate::store::Store;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const BEGIN: &str = "# >>> herdr-multiaccount >>>";
const END: &str = "# <<< herdr-multiaccount <<<";
/// The block's markers before the plugin was renamed; setup replaces such a
/// block with the current one.
const OLD_BEGIN: &str = "# >>> herdr-accounts >>>";
const OLD_END: &str = "# <<< herdr-accounts <<<";

fn is_begin(line: &str) -> bool {
    line == BEGIN || line == OLD_BEGIN
}

fn is_end(line: &str) -> bool {
    line == END || line == OLD_END
}

fn has_block(text: &str) -> bool {
    text.lines().any(|l| is_begin(l.trim_end()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct State {
    pub shell: bool,
    pub statusline: bool,
}

impl State {
    pub fn done(&self) -> bool {
        self.shell && self.statusline
    }
}

/// The shell's startup file: `~/.zshrc` for zsh, `~/.bashrc` for bash.
pub fn rc_file() -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let name = Path::new(&shell).file_name()?.to_string_lossy().to_string();
    match name.as_str() {
        "zsh" => Some(paths::home().join(".zshrc")),
        "bash" => Some(paths::home().join(".bashrc")),
        _ => None,
    }
}

/// The binary path as the shell should write it (`$HOME/...`).
pub(crate) fn bin_for_shell() -> String {
    let bin = paths::installed_bin();
    match bin.strip_prefix(paths::home()) {
        Ok(rest) => format!("$HOME/{}", rest.to_string_lossy()),
        Err(_) => bin.to_string_lossy().to_string(),
    }
}

/// Paths go into a shell file and a shell command inside double quotes:
/// refuse the characters that would break out of them.
fn quotable(path: &Path) -> Result<(), String> {
    let p = path.to_string_lossy();
    if p.contains(['"', '$', '`', '\\', '\n']) {
        Err(format!(
            "the path {p} has characters a shell would misread; not changed"
        ))
    } else {
        Ok(())
    }
}

/// A `claude` alias in the startup file wins over the function in an
/// interactive shell, so setup would do nothing.
fn has_alias(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("alias claude=") || l.starts_with("alias -g claude=")
    })
}

pub fn shell_block() -> String {
    format!(
        "{BEGIN}\n\
         # Starts Claude Code with the account of this Herdr space (Herdr Multiaccount plugin).\n\
         function claude {{\n  \
           local b=\"{}\"\n  \
           if [ -x \"$b\" ]; then \"$b\" run -- \"$@\"; else command claude \"$@\"; fi\n\
         }}\n\
         {END}\n",
        bin_for_shell()
    )
}

/// `text` with the block added (or replaced), or removed when `block` is
/// `None`. Refuses a file whose block has lost its end line, rather than
/// guess where it ends.
pub fn with_block(text: &str, block: Option<&str>) -> Result<String, String> {
    let mut open = false;
    for line in text.lines() {
        let line = line.trim_end();
        if is_begin(line) {
            if open {
                return Err("the herdr-multiaccount block in the startup file is broken (two start lines); fix it by hand".into());
            }
            open = true;
        } else if is_end(line) {
            if !open {
                return Err("the herdr-multiaccount block in the startup file is broken (an end line without start); fix it by hand".into());
            }
            open = false;
        }
    }
    if open {
        return Err(
            "the herdr-multiaccount block in the startup file has no end line; fix it by hand"
                .into(),
        );
    }
    let mut out = String::new();
    let mut inside = false;
    let mut placed = false;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end();
        if is_begin(bare) {
            inside = true;
            if let (Some(b), false) = (block, placed) {
                out.push_str(b);
                placed = true;
            }
            continue;
        }
        if inside {
            if is_end(bare) {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
    }
    // Removing the block also removes the blank line added before it.
    if block.is_none() && has_block(text) && out.ends_with("\n\n") {
        out.pop();
    }
    if let (Some(b), false) = (block, placed) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(b);
    }
    Ok(out)
}

fn settings_file() -> PathBuf {
    paths::claude_dir().join("settings.json")
}

/// The status line command that records usage.
fn statusline_command() -> String {
    format!(
        "\"{}\" statusline",
        paths::installed_bin().to_string_lossy()
    )
}

/// A status line command this plugin set, also under its old name.
fn is_ours(command: &str) -> bool {
    (command.contains("herdr-multiaccount") || command.contains("herdr-accounts"))
        && command.trim_end().ends_with(" statusline")
}

/// The status line command of this version.
fn is_current(command: &str) -> bool {
    command == statusline_command()
}

pub fn state() -> State {
    let shell = rc_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .is_some_and(|t| t.lines().any(|l| l.trim_end() == BEGIN));
    let statusline = read_settings()
        .and_then(|v| v["statusLine"]["command"].as_str().map(is_current))
        .unwrap_or(false);
    State { shell, statusline }
}

fn read_settings() -> Option<Value> {
    let text = std::fs::read_to_string(settings_file()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Copies a file to `backups/` before it changes. A failed backup stops the
/// change.
fn backup(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let dir = paths::backups_dir();
    let target = dir.join(format!(
        "{name}.{}.{}",
        crate::time::now(),
        std::process::id()
    ));
    paths::private_dir(&dir)
        .and_then(|_| std::fs::copy(path, &target))
        .map(|_| ())
        .map_err(|e| format!("could not back up {}: {e}; not changed", path.display()))
}

/// A file's text; `None` when it does not exist. Any other problem (no
/// access, not UTF-8) is an error, never an empty file.
fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && std::fs::symlink_metadata(path).is_err() =>
        {
            Ok(None)
        }
        Err(e) => Err(format!(
            "could not read {}: {e}; not changed",
            path.display()
        )),
    }
}

/// Writes a file in place, through a link if it is one (a settings file kept
/// in a repository and linked into `~/.claude` stays linked).
fn write_through(path: &Path, text: &str) -> Result<(), String> {
    let real = match path.canonicalize() {
        Ok(p) => p,
        Err(_) if std::fs::symlink_metadata(path).is_err() => path.to_path_buf(),
        Err(e) => {
            return Err(format!(
                "{} points nowhere ({e}); not changed",
                path.display()
            ))
        }
    };
    paths::write_atomic(&real, text.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// The settings with the status line wrapped (or unwrapped), and the
/// command that ran before.
pub fn wrap_statusline(
    settings: &Value,
    original: Option<&str>,
    on: bool,
) -> (Value, Option<String>) {
    let mut v = settings.clone();
    if !v.is_object() {
        v = json!({});
    }
    let current = v["statusLine"]["command"].as_str().map(str::to_string);
    if on {
        if current.as_deref().is_some_and(is_current) {
            return (v, original.map(str::to_string));
        }
        // An older wrapper of ours: point it at this version, keep yours.
        if current.as_deref().is_some_and(is_ours) {
            v["statusLine"]["command"] = json!(statusline_command());
            return (v, original.map(str::to_string));
        }
        let mut line = match v.get("statusLine") {
            Some(Value::Object(m)) => m.clone(),
            _ => serde_json::Map::new(),
        };
        line.insert("type".into(), json!("command"));
        line.insert("command".into(), json!(statusline_command()));
        // Re-reads at least every 30 s, so limits stay fresh while idle.
        line.entry("refreshInterval").or_insert(json!(30));
        v["statusLine"] = Value::Object(line);
        (v, current)
    } else {
        if !current.as_deref().is_some_and(is_ours) {
            return (v, None);
        }
        match original {
            Some(cmd) => v["statusLine"]["command"] = json!(cmd),
            None => {
                if let Some(m) = v.as_object_mut() {
                    m.remove("statusLine");
                }
            }
        }
        (v, None)
    }
}

/// Installs the copy of the binary the shell function and status line run.
pub fn install_bin() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let target = paths::installed_bin();
    if exe.canonicalize().ok() == target.canonicalize().ok() {
        return Ok(());
    }
    let new = std::fs::read(&exe)?;
    if std::fs::read(&target).ok().as_deref() == Some(new.as_slice()) {
        return Ok(());
    }
    std::fs::create_dir_all(paths::bin_dir())?;
    // accounts.json marks the directory for the copy (see paths::config_dir).
    if !paths::accounts_file().exists() {
        Store::update(|_| {})?;
    }
    let tmp = target.with_extension("new");
    std::fs::write(&tmp, &new)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, &target)
}

/// What setup changes, for the question before it does.
pub fn plan() -> Vec<String> {
    let s = state();
    let mut out = Vec::new();
    if !s.shell {
        match rc_file() {
            Some(f) => {
                out.push(format!("adds a claude shell function to {}", f.display()));
                if std::fs::read_to_string(&f).is_ok_and(|t| has_alias(&t)) {
                    out.push(format!(
                        "but {} has an alias for claude, which wins over the function: remove that line first",
                        f.display()
                    ));
                }
            }
            None => {
                out.push("your shell is not zsh or bash: add the function by hand (README)".into())
            }
        }
    }
    if !s.statusline {
        out.push(format!(
            "wraps the status line in {} (yours keeps running)",
            settings_file().display()
        ));
    }
    out
}

pub fn apply() -> Result<String, String> {
    quotable(&paths::installed_bin())?;
    install_bin().map_err(|e| format!("could not install the binary: {e}"))?;
    let s = state();
    let mut done = Vec::new();
    if !s.shell {
        let rc = rc_file().ok_or("your shell is not zsh or bash; see the README")?;
        let text = read_optional(&rc)?;
        if let Some(t) = &text {
            if has_alias(t) {
                return Err(format!(
                    "{} has an alias for claude, which would win over the function; remove that line, then set up again",
                    rc.display()
                ));
            }
            backup(&rc)?;
        }
        let new = with_block(text.as_deref().unwrap_or(""), Some(&shell_block()))?;
        write_through(&rc, &new)?;
        done.push("shell");
    }
    if !s.statusline {
        let file = settings_file();
        let text = read_optional(&file)?;
        let settings: Value = serde_json::from_str(text.as_deref().unwrap_or("{}"))
            .map_err(|_| format!("{} is not valid JSON; not changed", file.display()))?;
        let mut new = Value::Null;
        Store::update(|store| {
            let (wrapped, original) =
                wrap_statusline(&settings, store.statusline_original.as_deref(), true);
            store.statusline_original = original;
            new = wrapped;
        })
        .map_err(|e| e.to_string())?;
        if text.is_some() {
            backup(&file)?;
        }
        let out = serde_json::to_string_pretty(&new).unwrap_or_default();
        write_through(&file, &format!("{out}\n"))?;
        done.push("status line");
    }
    Ok(if done.is_empty() {
        "already set up".into()
    } else {
        format!(
            "set up: {}; open a new terminal tab for claude to use it",
            done.join(" and ")
        )
    })
}

pub fn undo() -> Result<String, String> {
    if let Some(rc) = rc_file() {
        if let Some(text) = read_optional(&rc)? {
            if has_block(&text) {
                let new = with_block(&text, None)?;
                backup(&rc)?;
                write_through(&rc, &new)?;
            }
        }
    }
    let file = settings_file();
    if let Some(text) = read_optional(&file)? {
        let settings: Value = serde_json::from_str(&text)
            .map_err(|_| format!("{} is not valid JSON; not changed", file.display()))?;
        let store = Store::try_load().map_err(|e| e.to_string())?;
        let (new, _) = wrap_statusline(&settings, store.statusline_original.as_deref(), false);
        if new != settings {
            backup(&file)?;
            let out = serde_json::to_string_pretty(&new).unwrap_or_default();
            write_through(&file, &format!("{out}\n"))?;
        }
        Store::update(|s| s.statusline_original = None).map_err(|e| e.to_string())?;
    }
    Ok("setup undone; accounts and their folders are kept".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_added_replaced_removed() {
        let b = "# >>> herdr-multiaccount >>>\nX\n# <<< herdr-multiaccount <<<\n";
        let added = with_block("export A=1", Some(b)).unwrap();
        assert_eq!(added, format!("export A=1\n\n{b}"));
        let replaced = with_block(&added, Some(&b.replace('X', "Y"))).unwrap();
        assert_eq!(replaced.matches(">>> herdr-multiaccount").count(), 1);
        assert!(replaced.contains("\nY\n"));
        let removed = with_block(&replaced, None).unwrap();
        assert_eq!(removed, "export A=1\n");
        assert_eq!(with_block("", Some(b)).unwrap(), b);
        // A block under the old name is replaced, not doubled.
        let old = "x\n\n# >>> herdr-accounts >>>\nOLD\n# <<< herdr-accounts <<<\n";
        let new = with_block(old, Some(b)).unwrap();
        assert_eq!(new, format!("x\n\n{b}"));
        // A block that lost its end line is never guessed at.
        let broken = "a\n# >>> herdr-multiaccount >>>\nX\nexport KEEP=1\n";
        assert!(with_block(broken, None).is_err());
        assert!(with_block(broken, Some(b)).is_err());
    }

    #[test]
    fn aliases_and_paths() {
        assert!(has_alias(
            "x\n  alias claude=\"$HOME/.claude/local/claude\"\n"
        ));
        assert!(!has_alias("alias cl=claude\n"));
        assert!(quotable(Path::new("/Users/alex/.config/x")).is_ok());
        assert!(quotable(Path::new("/Users/a$b/x")).is_err());
    }

    #[test]
    fn shell_block_is_valid_looking() {
        let b = shell_block();
        assert!(b.starts_with(BEGIN));
        assert!(b.contains("function claude {"));
        assert!(b.contains("command claude \"$@\""));
        assert!(b.trim_end().ends_with(END));
    }

    #[test]
    fn statusline_wrap_and_unwrap() {
        let s = json!({"model": "opus", "statusLine": {"type": "command", "command": "~/.claude/sl.sh", "refreshInterval": 10}});
        let (on, original) = wrap_statusline(&s, None, true);
        assert_eq!(original.as_deref(), Some("~/.claude/sl.sh"));
        assert!(is_ours(on["statusLine"]["command"].as_str().unwrap()));
        assert_eq!(on["statusLine"]["refreshInterval"], 10);
        assert_eq!(on["model"], "opus");
        // Wrapping twice keeps the first original.
        let (again, kept) = wrap_statusline(&on, original.as_deref(), true);
        assert_eq!(again, on);
        assert_eq!(kept.as_deref(), Some("~/.claude/sl.sh"));
        let (off, _) = wrap_statusline(&on, original.as_deref(), false);
        assert_eq!(off, s);

        let none = json!({"model": "opus"});
        let (on, original) = wrap_statusline(&none, None, true);
        assert_eq!(original, None);
        assert_eq!(on["statusLine"]["refreshInterval"], 30);
        let (off, _) = wrap_statusline(&on, None, false);
        assert_eq!(off, none);

        // A wrapper set under the plugin's old name points at this version
        // and keeps the original.
        let old = json!({"statusLine": {"type": "command", "command": "\"/x/accounts/bin/herdr-accounts\" statusline", "refreshInterval": 30}});
        let (on, kept) = wrap_statusline(&old, Some("~/.claude/sl.sh"), true);
        assert!(is_current(on["statusLine"]["command"].as_str().unwrap()));
        assert_eq!(kept.as_deref(), Some("~/.claude/sl.sh"));
    }
}
