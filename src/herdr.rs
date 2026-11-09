//! Talking to Herdr through its command line tool, which Herdr hands plugins
//! in `HERDR_BIN_PATH`.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Pane and space metadata tokens this plugin sets.
pub const SOURCE: &str = "multiaccount";
pub const TOKEN: &str = "claude_account";
pub const PID_TOKEN: &str = "claude_account_pid";
pub const ID_TOKEN: &str = "claude_account_id";

pub fn bin() -> PathBuf {
    std::env::var_os("HERDR_BIN_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("herdr"))
}

/// Whether this process runs inside a Herdr pane.
pub fn inside() -> bool {
    std::env::var("HERDR_ENV").as_deref() == Ok("1")
}

fn call(args: &[&str]) -> Option<Value> {
    let out = Command::new(bin())
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn run_quiet(args: &[&str]) -> bool {
    Command::new(bin())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub label: String,
    pub number: u64,
    pub focused: bool,
}

pub fn workspaces() -> Option<Vec<Workspace>> {
    let v = call(&["workspace", "list"])?;
    Some(
        v["result"]["workspaces"]
            .as_array()?
            .iter()
            .filter_map(|w| {
                Some(Workspace {
                    id: w["workspace_id"].as_str()?.to_string(),
                    label: w["label"].as_str().unwrap_or("").to_string(),
                    number: w["number"].as_u64().unwrap_or(0),
                    focused: w["focused"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
    )
}

/// The space a pane is in now (a pane moved to another space keeps its old
/// id in its environment, which Herdr still resolves).
pub fn pane_workspace(pane: &str) -> Option<(String, String)> {
    let v = call(&["pane", "get", pane])?;
    let ws = v["result"]["pane"]["workspace_id"].as_str()?.to_string();
    let label = workspaces()
        .and_then(|list| list.into_iter().find(|w| w.id == ws))
        .map(|w| w.label)
        .unwrap_or_default();
    Some((ws, label))
}

/// A live pane and the account its Claude Code runs on.
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub id: String,
    pub workspace: String,
    pub tab: String,
    /// The pane's name in Herdr (what the tab shows).
    pub label: String,
    pub agent: Option<String>,
    /// What the agent does: working, blocked, idle, done.
    pub status: Option<String>,
    /// The account's name and id: from the launcher's note
    /// (`sessions.rs`), as Herdr does not hand back the pane labels.
    pub account: Option<String>,
    pub account_id: Option<String>,
}

pub fn panes() -> Option<Vec<Pane>> {
    let v = call(&["pane", "list"])?;
    let notes = crate::sessions::live();
    let mut list: Vec<Pane> = v["result"]["panes"]
        .as_array()?
        .iter()
        .filter_map(|p| {
            Some(Pane {
                id: p["pane_id"].as_str()?.to_string(),
                workspace: p["workspace_id"].as_str().unwrap_or("").to_string(),
                tab: p["tab_id"].as_str().unwrap_or("").to_string(),
                label: p["label"]
                    .as_str()
                    .filter(|l| !l.is_empty())
                    .or(p["terminal_title_stripped"].as_str())
                    .unwrap_or("")
                    .to_string(),
                agent: p["agent"].as_str().map(str::to_string),
                status: p["agent_status"]
                    .as_str()
                    .filter(|s| *s != "unknown")
                    .map(str::to_string),
                account: p["tokens"][TOKEN].as_str().map(str::to_string),
                account_id: p["tokens"][ID_TOKEN].as_str().map(str::to_string),
            })
        })
        .collect();
    for p in &mut list {
        if let Some(e) = notes.get(&p.id) {
            p.account_id = Some(e.account.clone());
            p.account = Some(e.account.clone());
        }
    }
    Some(list)
}

/// A pane's name and the tab it is in.
fn pane_field(pane: &str, field: &str) -> Option<String> {
    let v = call(&["pane", "get", pane])?;
    v["result"]["pane"][field]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn pane_label(pane: &str) -> Option<String> {
    pane_field(pane, "label")
}

pub fn pane_tab(pane: &str) -> Option<String> {
    pane_field(pane, "tab_id")
}

/// Opens one of this plugin's panes as a Herdr popup (`width` × `height`
/// cells, border included) over whatever is in front, with the focus.
pub fn open_popup(entrypoint: &str, width: u16, height: u16, env: &[(&str, &str)]) -> bool {
    let mut args: Vec<String> = [
        "plugin",
        "pane",
        "open",
        "--plugin",
        SOURCE,
        "--entrypoint",
        entrypoint,
        "--placement",
        "popup",
        "--width",
        &width.to_string(),
        "--height",
        &height.to_string(),
        "--focus",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (k, v) in env {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_quiet(&refs)
}

/// Brings a tab (and its space) to the front.
pub fn focus_tab(tab: &str) -> bool {
    run_quiet(&["tab", "focus", tab])
}

/// Labels a pane with the account its Claude Code runs on.
pub fn tag_pane(pane: &str, account: &str, id: &str, pid: u32) {
    let a = format!("{TOKEN}={account}");
    let i = format!("{ID_TOKEN}={id}");
    let p = format!("{PID_TOKEN}={pid}");
    run_quiet(&[
        "pane",
        "report-metadata",
        pane,
        "--source",
        SOURCE,
        "--token",
        &a,
        "--token",
        &i,
        "--token",
        &p,
    ]);
}

pub fn untag_pane(pane: &str) {
    run_quiet(&[
        "pane",
        "report-metadata",
        pane,
        "--source",
        SOURCE,
        "--clear-token",
        TOKEN,
        "--clear-token",
        ID_TOKEN,
        "--clear-token",
        PID_TOKEN,
    ]);
}

/// Shows the account a space uses next to it in the sidebar (`None` clears).
pub fn tag_workspace(workspace: &str, account: Option<&str>) {
    match account {
        Some(name) => {
            let t = format!("{TOKEN}={name}");
            run_quiet(&[
                "workspace",
                "report-metadata",
                workspace,
                "--source",
                SOURCE,
                "--token",
                &t,
            ])
        }
        None => run_quiet(&[
            "workspace",
            "report-metadata",
            workspace,
            "--source",
            SOURCE,
            "--clear-token",
            TOKEN,
        ]),
    };
}

pub fn notify(title: &str, body: &str) {
    run_quiet(&[
        "notification",
        "show",
        title,
        "--body",
        body,
        "--sound",
        "request",
    ]);
}
