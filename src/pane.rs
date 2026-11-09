//! The `open` plugin action: opens the list as a Herdr plugin pane in the
//! directory of the focused pane, or closes it when the current tab already
//! has one.

use crate::config::{Direction, Placement};
use crate::herdr;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// The panes this plugin opened, one file per Herdr server.
fn state_file() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HERDR_PLUGIN_STATE_DIR")?);
    let socket = std::env::var("HERDR_SOCKET_PATH").unwrap_or_default();
    // FNV-1a: stable across builds, unlike the standard library's hasher.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in socket.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Some(dir.join(format!("open-panes-{hash:016x}")))
}

fn read_ids() -> Vec<String> {
    state_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|t| {
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn write_ids(ids: &[String]) {
    if let Some(f) = state_file() {
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, ids.join("\n"));
    }
}

struct Context {
    pane: Option<String>,
    tab: Option<String>,
    cwd: Option<String>,
}

fn context() -> Context {
    let ctx: Value = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let get = |k: &str| ctx.get(k).and_then(Value::as_str).map(str::to_string);
    Context {
        pane: get("focused_pane_id").or_else(|| std::env::var("HERDR_PANE_ID").ok()),
        tab: get("tab_id").or_else(|| std::env::var("HERDR_TAB_ID").ok()),
        cwd: get("focused_pane_cwd").or_else(|| get("workspace_cwd")),
    }
}

/// Every live pane and the tab it is in.
fn live_panes() -> Option<HashMap<String, String>> {
    let out = Command::new(herdr::bin())
        .args(["pane", "list"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(
        v["result"]["panes"]
            .as_array()?
            .iter()
            .filter_map(|p| {
                Some((
                    p["pane_id"].as_str()?.to_string(),
                    p["tab_id"].as_str().unwrap_or("").to_string(),
                ))
            })
            .collect(),
    )
}

pub fn toggle(placement: Placement, direction: Direction) -> Result<String, String> {
    let ctx = context();
    let mut ids = read_ids();
    if let Some(live) = live_panes() {
        ids.retain(|id| live.contains_key(id));
        let here: Vec<String> = ids
            .iter()
            .filter(|id| Some(*id) == ctx.pane.as_ref() || live.get(*id) == ctx.tab.as_ref())
            .cloned()
            .collect();
        if !here.is_empty() {
            for id in &here {
                let _ = Command::new(herdr::bin())
                    .args(["plugin", "pane", "close", id])
                    .output();
            }
            ids.retain(|id| !here.contains(id));
            write_ids(&ids);
            return Ok(format!("closed {}", here.join(", ")));
        }
    }

    let mut cmd = Command::new(herdr::bin());
    cmd.args([
        "plugin",
        "pane",
        "open",
        "--plugin",
        "multiaccount",
        "--entrypoint",
        "list",
        "--placement",
        placement.as_arg(),
        "--focus",
    ]);
    if let Some(cwd) = &ctx.cwd {
        cmd.args(["--cwd", cwd]);
    }
    if placement == Placement::Split {
        cmd.args(["--direction", direction.as_arg()]);
        if let Some(pane) = &ctx.pane {
            cmd.args(["--target-pane", pane]);
        }
    }
    let output = cmd.output().map_err(|e| format!("herdr: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not open the list: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    let id = answer["result"]["plugin_pane"]["pane"]["pane_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if !id.is_empty() {
        ids.push(id.clone());
        write_ids(&ids);
    }
    Ok(format!("opened {id}"))
}

/// Called when the list closes itself (`q`): forgets its pane.
pub fn forget_self() {
    let Ok(me) = std::env::var("HERDR_PANE_ID") else {
        return;
    };
    let mut ids = read_ids();
    let before = ids.len();
    ids.retain(|id| *id != me);
    if ids.len() != before {
        write_ids(&ids);
    }
}
