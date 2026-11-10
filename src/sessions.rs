//! Which account each session started through the plugin runs on, per Herdr
//! pane: `<config>/sessions/<pane>.json`. Herdr shows the pane labels the
//! launcher sets in its sidebar but does not hand them back on its command
//! line, so the launcher also notes them here, with the pid of the `claude`
//! process (the launcher execs, so the pid stays). A note whose process has
//! ended counts for nothing and is removed.

use crate::paths;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub account: String,
    pub pid: u32,
    /// The account the session would have had when fallback moved it.
    pub wanted: Option<String>,
    /// The reminder that `wanted` has room again was shown.
    pub told: bool,
}

fn entry(v: &serde_json::Value) -> Option<Entry> {
    Some(Entry {
        account: v["account"].as_str()?.to_string(),
        pid: u32::try_from(v["pid"].as_u64()?).ok()?,
        wanted: v["wanted"].as_str().map(str::to_string),
        told: v["told"].as_bool().unwrap_or(false),
    })
}

fn dir() -> PathBuf {
    paths::config_dir().join("sessions")
}

/// A file name for a pane id (`w3:p1` → `w3_p1.json`).
fn file(pane: &str) -> PathBuf {
    let safe: String = pane
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir().join(format!("{safe}.json"))
}

pub fn record(pane: &str, account: &str, wanted: Option<&str>, pid: u32) {
    let mut v = json!({"pane": pane, "account": account, "pid": pid});
    if let Some(w) = wanted {
        v["wanted"] = json!(w);
    }
    let _ = paths::write_atomic(&file(pane), format!("{v}\n").as_bytes());
}

pub fn get(pane: &str) -> Option<Entry> {
    let text = std::fs::read_to_string(file(pane)).ok()?;
    entry(&serde_json::from_str(&text).ok()?)
}

/// Marks the reminder as shown; false when it already was.
pub fn mark_told(pane: &str) -> bool {
    let path = file(pane);
    let Some(mut v) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return false;
    };
    if v["told"].as_bool() == Some(true) {
        return false;
    }
    v["told"] = json!(true);
    paths::write_atomic(&path, format!("{v}\n").as_bytes()).is_ok()
}

pub fn remove(pane: &str) {
    let _ = std::fs::remove_file(file(pane));
}

/// Pids of the processes running now, from one `ps` call; `None` when `ps`
/// failed (then nothing is removed).
fn running_pids() -> Option<HashSet<u32>> {
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect(),
    )
}

/// The sessions whose process still runs, by pane id. Notes of ended ones
/// are removed.
pub fn live() -> HashMap<String, Entry> {
    let mut map = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return map;
    };
    let pids = running_pids();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            continue;
        };
        let (Some(pane), Some(e)) = (v["pane"].as_str(), entry(&v)) else {
            continue;
        };
        match &pids {
            Some(p) if !p.contains(&e.pid) => {
                let _ = std::fs::remove_file(&path);
            }
            _ => {
                map.insert(pane.to_string(), e);
            }
        }
    }
    map
}

/// Which conversation a pane holds, from its status line: Claude Code's
/// session id and the processes above the status line command (Claude Code
/// among them, shells left out). `<config>/conversations/<pane>.json`, kept
/// apart from the launcher's notes, so sessions started without the plugin
/// have one too.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub session_id: String,
    pub pids: Vec<u32>,
}

/// Whether `id` looks exactly like Claude Code's session ids (a UUID): only
/// such ids may go into the command that resumes a session in a shell.
pub fn valid_session_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn conversations_dir() -> PathBuf {
    paths::config_dir().join("conversations")
}

fn conversation_file(dir: &std::path::Path, pane: &str) -> PathBuf {
    let name = file(pane);
    dir.join(name.file_name().unwrap_or_default())
}

fn read_conversation(path: &std::path::Path) -> Option<Conversation> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let session_id = v["session_id"].as_str()?.to_string();
    if !valid_session_id(&session_id) {
        return None;
    }
    let pids = v["pids"]
        .as_array()?
        .iter()
        .filter_map(|p| u32::try_from(p.as_u64()?).ok())
        .collect();
    Some(Conversation { session_id, pids })
}

/// The conversation the status line last noted for a pane. Whether its
/// Claude Code still runs there is for the caller to check
/// (`front_process`).
pub fn conversation(pane: &str) -> Option<Conversation> {
    read_conversation(&conversation_file(&conversations_dir(), pane))
}

/// Notes a pane's conversation; writes only when it changed. Notes older
/// than a week are dropped when a pane gets its first one.
pub fn record_conversation(pane: &str, c: &Conversation) {
    record_conversation_in(&conversations_dir(), pane, c, std::time::SystemTime::now());
}

fn record_conversation_in(
    dir: &std::path::Path,
    pane: &str,
    c: &Conversation,
    now: std::time::SystemTime,
) {
    if !valid_session_id(&c.session_id) {
        return;
    }
    let path = conversation_file(dir, pane);
    match read_conversation(&path) {
        Some(old) if old == *c => return,
        Some(_) => {}
        None => prune(dir, now),
    }
    let v = json!({"pane": pane, "session_id": c.session_id, "pids": c.pids});
    let _ = std::fs::create_dir_all(dir);
    let _ = paths::write_atomic(&path, format!("{v}\n").as_bytes());
}

fn prune(dir: &std::path::Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > week);
        if old && e.path().extension().is_some_and(|x| x == "json") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Whether a process name is a shell's (`-zsh` for a login shell).
pub fn is_shell(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    let base = base.trim_start_matches('-');
    matches!(
        base,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh"
    )
}

/// The processes above `start` (itself included), from a `ps` table of
/// (pid, parent pid, name), leaving out shells, at most `max`.
pub fn ancestors_from(table: &[(u32, u32, String)], start: u32, max: usize) -> Vec<u32> {
    let mut out = Vec::new();
    let mut pid = start;
    for _ in 0..16 {
        let Some((_, parent, name)) = table.iter().find(|(p, _, _)| *p == pid) else {
            break;
        };
        if !is_shell(name) {
            out.push(pid);
            if out.len() >= max {
                break;
            }
        }
        if *parent <= 1 || *parent == pid {
            break;
        }
        pid = *parent;
    }
    out
}

/// The processes above this one (Claude Code, when the status line runs),
/// from one `ps` call.
pub fn my_ancestors() -> Vec<u32> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,comm="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return vec![];
    };
    let table: Vec<(u32, u32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let name = it.collect::<Vec<_>>().join(" ");
            Some((pid, ppid, name))
        })
        .collect();
    ancestors_from(&table, std::os::unix::process::parent_id(), 6)
}

/// The noted Claude Code process when it is in the front of the pane now:
/// then the note is the pane's conversation. `None` once it has ended, or
/// when only a shell is there.
pub fn front_process(c: &Conversation, p: &crate::herdr::Processes) -> Option<u32> {
    c.pids.iter().copied().find(|pid| {
        Some(*pid) != p.shell && p.front.iter().any(|(f, name)| f == pid && !is_shell(name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_ids_become_file_names() {
        assert!(file("w3:p1").ends_with("sessions/w3_p1.json"));
        assert!(file("../x").ends_with("sessions/___x.json"));
    }

    const ID: &str = "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f6";

    #[test]
    fn only_uuid_session_ids_count() {
        assert!(valid_session_id(ID));
        for bad in [
            "",
            "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f",
            "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f6; rm -rf ~",
            "0b7f3c2e 1d4a-4c6b-9e8f-a1b2c3d4e5f6",
            "$(touch x)-1d4a-4c6b-9e8f-a1b2c3d4e5f6",
            "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5fg",
            "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f6-",
        ] {
            assert!(!valid_session_id(bad), "{bad}");
        }
    }

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "hm-conv-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn conversations_are_noted_per_pane_and_only_valid_ones() {
        let dir = temp();
        let now = std::time::SystemTime::now();
        let c = Conversation {
            session_id: ID.into(),
            pids: vec![4242, 17],
        };
        record_conversation_in(&dir, "w3:p1", &c, now);
        let path = conversation_file(&dir, "w3:p1");
        assert!(path.ends_with("w3_p1.json"));
        assert_eq!(read_conversation(&path), Some(c.clone()));
        // An id that is no UUID is never written, nor read.
        let bad = Conversation {
            session_id: "x; rm -rf ~".into(),
            pids: vec![1],
        };
        record_conversation_in(&dir, "w3:p2", &bad, now);
        assert!(!conversation_file(&dir, "w3:p2").exists());
        std::fs::write(
            conversation_file(&dir, "w3:p3"),
            r#"{"session_id": "$(boom)", "pids": [1]}"#,
        )
        .unwrap();
        assert_eq!(read_conversation(&conversation_file(&dir, "w3:p3")), None);
        // A week later a new pane's first note drops the old ones.
        let later = now + std::time::Duration::from_secs(8 * 24 * 3600);
        record_conversation_in(&dir, "w3:p4", &c, later);
        assert!(!path.exists(), "old note pruned");
        assert!(conversation_file(&dir, "w3:p4").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ancestors_leave_out_shells() {
        let table = vec![
            (500, 400, "sh".to_string()),
            (400, 300, "/Users/alex/.local/bin/claude".to_string()),
            (300, 200, "-zsh".to_string()),
            (200, 1, "herdr".to_string()),
        ];
        assert_eq!(ancestors_from(&table, 500, 6), vec![400, 200]);
        assert_eq!(ancestors_from(&table, 500, 1), vec![400]);
        assert!(ancestors_from(&table, 999, 6).is_empty());
    }

    #[test]
    fn a_note_counts_only_while_its_claude_code_is_in_front() {
        use crate::herdr::Processes;
        let c = Conversation {
            session_id: ID.into(),
            pids: vec![400, 200],
        };
        let running = Processes {
            shell: Some(300),
            group: None,
            front: vec![(400, "2.1.30".into())],
        };
        assert_eq!(front_process(&c, &running), Some(400));
        // Claude Code ended: the shell is back in front.
        let ended = Processes {
            shell: Some(300),
            group: None,
            front: vec![(300, "zsh".into())],
        };
        assert_eq!(front_process(&c, &ended), None);
        // Another Claude Code started since, whose status line has not run.
        let other = Processes {
            shell: Some(300),
            group: None,
            front: vec![(777, "claude".into())],
        };
        assert_eq!(front_process(&c, &other), None);
        // A nested shell in front never counts as Claude Code.
        let nested = Processes {
            shell: Some(300),
            group: None,
            front: vec![(400, "bash".into())],
        };
        assert_eq!(front_process(&c, &nested), None);
    }
}
