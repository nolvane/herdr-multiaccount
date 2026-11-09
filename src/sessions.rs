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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_ids_become_file_names() {
        assert!(file("w3:p1").ends_with("sessions/w3_p1.json"));
        assert!(file("../x").ends_with("sessions/___x.json"));
    }
}
