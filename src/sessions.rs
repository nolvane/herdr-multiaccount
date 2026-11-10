//! Which account each session started through the plugin runs on, per Herdr
//! pane: `<config>/sessions/<pane>.json`. Herdr shows the pane labels the
//! launcher sets in its sidebar but does not hand them back on its command
//! line, so the launcher also notes them here, with the pid of the `claude`
//! process (the launcher execs, so the pid stays). A note whose process has
//! ended counts for nothing and is removed.

use crate::carry::Carried;
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
    /// The start options a move carries over (`carry::carried`); `None` in
    /// notes written before they were kept.
    pub options: Option<Vec<String>>,
    /// The names of the start options a move does not carry over.
    pub dropped: Vec<String>,
}

fn strings(v: &serde_json::Value) -> Option<Vec<String>> {
    v.as_array()?
        .iter()
        .map(|s| s.as_str().map(str::to_string))
        .collect()
}

fn entry(v: &serde_json::Value) -> Option<Entry> {
    Some(Entry {
        account: v["account"].as_str()?.to_string(),
        pid: u32::try_from(v["pid"].as_u64()?).ok()?,
        wanted: v["wanted"].as_str().map(str::to_string),
        told: v["told"].as_bool().unwrap_or(false),
        options: strings(&v["options"]),
        dropped: strings(&v["dropped"]).unwrap_or_default(),
    })
}

fn dir() -> PathBuf {
    paths::config_dir().join("sessions")
}

/// A pane id as a file name stem (`w3:p1` → `w3_p1`): letters and digits
/// only, so no id can reach outside its folder.
pub fn safe_name(pane: &str) -> String {
    pane.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn file(pane: &str) -> PathBuf {
    dir().join(format!("{}.json", safe_name(pane)))
}

/// Whether a process runs; true when `ps` cannot tell, so nothing is
/// removed on a guess.
pub fn alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(true)
}

/// Notes the session the launcher starts in a pane. Only its owner can
/// read the note: start options can hold private text or tokens.
/// `carried` is `None` when the start options are not known (a move whose
/// hand-over was missing, or that did not know them either).
pub fn record(
    pane: &str,
    account: &str,
    wanted: Option<&str>,
    pid: u32,
    carried: Option<&Carried>,
) {
    let path = file(pane);
    let _ = record_at(&path, note(pane, account, wanted, pid, carried));
}

fn record_at(path: &std::path::Path, note: String) -> std::io::Result<()> {
    paths::write_private(path, note.as_bytes())
}

fn note(
    pane: &str,
    account: &str,
    wanted: Option<&str>,
    pid: u32,
    carried: Option<&Carried>,
) -> String {
    let mut v = json!({"pane": pane, "account": account, "pid": pid});
    if let Some(c) = carried {
        v["options"] = json!(c.options);
        v["dropped"] = json!(c.dropped);
    }
    if let Some(w) = wanted {
        v["wanted"] = json!(w);
    }
    format!("{v}\n")
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
    /// The model it runs on now (`model.id`); `None` when not noted.
    pub model: Option<String>,
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
    let model = v["model"]
        .as_str()
        .filter(|m| crate::carry::valid_model(m))
        .map(str::to_string);
    Some(Conversation {
        session_id,
        pids,
        model,
    })
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
    let mut v = json!({"pane": pane, "session_id": c.session_id, "pids": c.pids});
    if let Some(m) = c.model.as_deref().filter(|m| crate::carry::valid_model(m)) {
        v["model"] = json!(m);
    }
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

/// The one-time hand-over of a move (`relocate.rs`): the options for the
/// conversation the list starts again in a pane,
/// `<config>/moves/<pane>.json`, only its owner can read. The command typed
/// into the pane names no option, so none lands in the shell's history or is
/// read by the shell.
fn moves_dir() -> PathBuf {
    paths::config_dir().join("moves")
}

/// How long a hand-over counts: the move types its command seconds after
/// writing it.
const HAND_OVER_FOR: i64 = 120;

/// What the launcher gets from a hand-over: its arguments for Claude Code,
/// and what the moved session's note says of its start options: `None`
/// when the list did not know them (only the model was handed over), else
/// the names the first start dropped, so the next move names them again.
#[derive(Debug, Clone, PartialEq)]
pub struct HandedOver {
    pub args: Vec<String>,
    pub dropped: Option<Vec<String>>,
}

/// What a move hands over: the options for Claude Code, and the names of
/// the dropped ones, `None` when the start options are not known.
pub fn write_hand_over(
    pane: &str,
    session_id: &str,
    args: &[String],
    dropped: Option<&[String]>,
) -> bool {
    write_hand_over_in(
        &moves_dir(),
        pane,
        session_id,
        args,
        dropped,
        crate::time::now(),
    )
}

fn write_hand_over_in(
    dir: &std::path::Path,
    pane: &str,
    session_id: &str,
    args: &[String],
    dropped: Option<&[String]>,
    now: i64,
) -> bool {
    if paths::private_dir(dir).is_err() {
        return false;
    }
    prune_hand_overs(dir, std::time::SystemTime::now());
    let mut v = json!({"session_id": session_id, "args": args, "at": now});
    if let Some(d) = dropped {
        v["dropped"] = json!(d);
    }
    paths::write_private(&conversation_file(dir, pane), format!("{v}\n").as_bytes()).is_ok()
}

/// Removes hand-overs no launcher took in time (a list that stopped
/// between writing and typing): they may hold option values.
fn prune_hand_overs(dir: &std::path::Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let limit = std::time::Duration::from_secs(HAND_OVER_FOR as u64);
    for e in entries.flatten() {
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > limit);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub fn drop_hand_over(pane: &str) {
    let _ = std::fs::remove_file(conversation_file(&moves_dir(), pane));
}

/// The launcher's arguments for a moved session: the hand-over's options,
/// then `args`, which must be exactly `--resume <id>` of the same
/// conversation. The hand-over is removed in every case, and its options go
/// through the list of carried options again. Err says why it was not used.
pub fn take_hand_over(pane: &str, args: &[String]) -> Result<HandedOver, &'static str> {
    take_hand_over_in(&moves_dir(), pane, args, crate::time::now())
}

fn take_hand_over_in(
    dir: &std::path::Path,
    pane: &str,
    args: &[String],
    now: i64,
) -> Result<HandedOver, &'static str> {
    let path = conversation_file(dir, pane);
    let text = std::fs::read_to_string(&path).map_err(|_| "there was none");
    let _ = std::fs::remove_file(&path);
    prune_hand_overs(dir, std::time::SystemTime::now());
    let v: serde_json::Value = serde_json::from_str(&text?).map_err(|_| "it was unreadable")?;
    let [flag, id] = args else {
        return Err("it is only for --resume <id>");
    };
    if flag != "--resume" || v["session_id"].as_str() != Some(id.as_str()) {
        return Err("it was for another conversation");
    }
    let at = v["at"].as_i64().ok_or("it was unreadable")?;
    if !(now - HAND_OVER_FOR..=now + 5).contains(&at) {
        return Err("it was too old");
    }
    let wanted = strings(&v["args"]).ok_or("it was unreadable")?;
    let mut out = crate::carry::carried(&wanted, None).options;
    out.extend(args.iter().cloned());
    Ok(HandedOver {
        args: out,
        dropped: strings(&v["dropped"]),
    })
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
            model: None,
        };
        record_conversation_in(&dir, "w3:p1", &c, now);
        let path = conversation_file(&dir, "w3:p1");
        assert!(path.ends_with("w3_p1.json"));
        assert_eq!(read_conversation(&path), Some(c.clone()));
        // An id that is no UUID is never written, nor read.
        let bad = Conversation {
            session_id: "x; rm -rf ~".into(),
            pids: vec![1],
            model: None,
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
            model: None,
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

    #[test]
    fn the_model_is_noted_with_the_conversation_and_old_notes_have_none() {
        let dir = temp().join("model");
        let now = std::time::SystemTime::now();
        let mut c = Conversation {
            session_id: ID.into(),
            pids: vec![4242],
            model: Some("claude-opus-4-5[1m]".into()),
        };
        record_conversation_in(&dir, "w3:p1", &c, now);
        let path = conversation_file(&dir, "w3:p1");
        assert_eq!(read_conversation(&path), Some(c.clone()));
        // A /model change is noted again.
        c.model = Some("claude-sonnet-4-5".into());
        record_conversation_in(&dir, "w3:p1", &c, now);
        assert_eq!(read_conversation(&path).unwrap().model, c.model);
        // A note from before this version, or with an odd model, has none.
        std::fs::write(&path, format!(r#"{{"session_id": "{ID}", "pids": [1]}}"#)).unwrap();
        assert_eq!(read_conversation(&path).unwrap().model, None);
        std::fs::write(
            &path,
            format!(r#"{{"session_id": "{ID}", "pids": [1], "model": "$(x)"}}"#),
        )
        .unwrap();
        assert_eq!(read_conversation(&path).unwrap().model, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launcher_notes_keep_the_options_and_old_ones_read_as_unknown() {
        let carried = Carried {
            options: vec!["--model=sonnet".into(), "--add-dir=/a".into()],
            dropped: vec!["--worktree".into()],
        };
        let v: serde_json::Value =
            serde_json::from_str(&note("w1:p1", "work", None, 42, Some(&carried))).unwrap();
        let e = entry(&v).unwrap();
        assert_eq!(e.options, Some(carried.options.clone()));
        assert_eq!(e.dropped, carried.dropped);
        let old = entry(&json!({"account": "work", "pid": 42})).unwrap();
        assert_eq!(old.options, None);
        assert!(old.dropped.is_empty());
        // A moved session whose options were not known stays unknown.
        let v: serde_json::Value =
            serde_json::from_str(&note("w1:p1", "work", None, 42, None)).unwrap();
        assert_eq!(entry(&v).unwrap().options, None);
    }

    #[test]
    fn launcher_notes_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp().join("private");
        let path = dir.join("w1_p1.json");
        let carried = Carried {
            options: vec!["--mcp-config={\"token\":\"dummy\"}".into()],
            dropped: vec![],
        };
        // An older, wider note is replaced by a private one.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        record_at(&path, note("w1:p1", "work", None, 42, Some(&carried))).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_hand_over_arrives_unchanged_once_and_only_for_its_conversation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp().join("moves");
        let now = 1_794_225_600;
        let odd = [
            "--append-system-prompt=it's \"quoted\" $(touch /tmp/x) `id`; rm -rf ~\nline two",
            "--add-dir=/work/a b/[c]",
            "--model=claude-opus-4-5[1m]",
        ];
        let dropped = strs(&["--worktree"]);
        assert!(write_hand_over_in(
            &dir,
            "w1:p1",
            ID,
            &strs(&odd),
            Some(&dropped),
            now
        ));
        let path = conversation_file(&dir, "w1:p1");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        let resume = strs(&["--resume", ID]);
        let got = take_hand_over_in(&dir, "w1:p1", &resume, now + 3).unwrap();
        let mut want = strs(&odd);
        want.extend(resume.clone());
        assert_eq!(got.args, want);
        assert_eq!(got.dropped, Some(dropped), "named again on the next move");
        assert!(!path.exists(), "used once");
        assert!(take_hand_over_in(&dir, "w1:p1", &resume, now + 3).is_err());
        // Another conversation, other arguments, an old file: not used, and
        // gone all the same.
        let other = strs(&["--resume", "11111111-2222-4333-8444-555555555555"]);
        for (args, at) in [
            (other, now),
            (strs(&["--resume", ID, "--verbose"]), now),
            (strs(&["--continue"]), now),
            (resume.clone(), now - 600),
        ] {
            assert!(write_hand_over_in(
                &dir,
                "w1:p1",
                ID,
                &strs(&odd),
                Some(&[]),
                at
            ));
            assert!(
                take_hand_over_in(&dir, "w1:p1", &args, now).is_err(),
                "{args:?}"
            );
            assert!(!path.exists());
        }
        // A planted file cannot add options that are never carried.
        assert!(write_hand_over_in(
            &dir,
            "w1:p1",
            ID,
            &strs(&["--settings=/tmp/s.json", "--bare", "--verbose", "hello"]),
            None,
            now
        ));
        assert_eq!(
            take_hand_over_in(&dir, "w1:p1", &resume, now).unwrap().args,
            strs(&["--verbose", "--resume", ID])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_start_options_stay_unknown_through_a_hand_over() {
        let dir = temp().join("unknown");
        let now = 1_794_225_600;
        let resume = strs(&["--resume", ID]);
        assert!(write_hand_over_in(
            &dir,
            "w1:p1",
            ID,
            &strs(&["--model=opus"]),
            None,
            now
        ));
        let got = take_hand_over_in(&dir, "w1:p1", &resume, now).unwrap();
        assert_eq!(got.dropped, None);
        assert_eq!(got.args, strs(&["--model=opus", "--resume", ID]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hand_overs_nobody_took_are_removed() {
        let dir = temp().join("stale");
        paths::private_dir(&dir).unwrap();
        let old = conversation_file(&dir, "w9:p9");
        std::fs::write(&old, "{}").unwrap();
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(600))
            .unwrap();
        drop(f);
        assert!(write_hand_over_in(&dir, "w1:p1", ID, &[], Some(&[]), 0));
        assert!(!old.exists(), "a stale hand-over is gone");
        assert!(
            conversation_file(&dir, "w1:p1").exists(),
            "the new one stays"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
