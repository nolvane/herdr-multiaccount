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
    /// The git checkout the space is for; `None` outside a repository.
    pub worktree: Option<Worktree>,
}

/// A space's checkout, as Herdr groups them: spaces with the same
/// `repo_key` belong to one repository.
#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub repo_key: String,
    /// A linked worktree, not the repository's main checkout.
    pub linked: bool,
}

pub fn workspaces() -> Option<Vec<Workspace>> {
    workspaces_from(&call(&["workspace", "list"])?)
}

/// Reads the answer of `herdr workspace list`.
pub fn workspaces_from(v: &Value) -> Option<Vec<Workspace>> {
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
                    worktree: w["worktree"]["repo_key"].as_str().map(|key| Worktree {
                        repo_key: key.to_string(),
                        linked: w["worktree"]["is_linked_worktree"]
                            .as_bool()
                            .unwrap_or(false),
                    }),
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

/// Closes a pane, and with it what runs in it.
pub fn close_pane(pane: &str) -> bool {
    run_quiet(&["pane", "close", pane])
}

/// Closes a space with all its tabs and panes; `group` also closes the
/// worktree spaces of a main checkout, which Herdr refuses to close alone.
pub fn close_workspace(workspace: &str, group: bool) -> bool {
    let mut args = vec!["workspace", "close", workspace];
    if group {
        args.push("--group");
    }
    run_quiet(&args)
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

/// What runs in the front of a pane: the shell's pid and the processes of
/// the foreground process group, with their names.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Processes {
    pub shell: Option<u32>,
    /// The foreground process group: the shell's own pid when it is in
    /// front.
    pub group: Option<u32>,
    pub front: Vec<(u32, String)>,
}

impl Processes {
    pub fn from_json(v: &Value) -> Option<Processes> {
        let info = &v["result"]["process_info"];
        if !info.is_object() {
            return None;
        }
        let pid = |v: &Value| v.as_u64().and_then(|p| u32::try_from(p).ok());
        Some(Processes {
            shell: pid(&info["shell_pid"]),
            group: pid(&info["foreground_process_group_id"]),
            front: info["foreground_processes"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|p| {
                            Some((
                                pid(&p["pid"])?,
                                p["name"].as_str().unwrap_or("").to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Whether the pane's own shell is in front (waiting at its prompt).
    pub fn shell_in_front(&self) -> bool {
        self.shell
            .is_some_and(|s| self.group == Some(s) || self.front.iter().any(|(p, _)| *p == s))
    }
}

pub fn process_info(pane: &str) -> Option<Processes> {
    Processes::from_json(&call(&["pane", "process-info", "--pane", pane])?)
}

/// Presses keys in a pane (`ctrl+c`, `esc`, ...).
pub fn send_keys(pane: &str, keys: &[&str]) -> bool {
    let mut args = vec!["pane", "send-keys", pane];
    args.extend_from_slice(keys);
    run_quiet(&args)
}

/// Types a command and Enter into a pane's shell.
pub fn run_in_pane(pane: &str, command: &str) -> bool {
    run_quiet(&["pane", "run", pane, command])
}

/// The agent Herdr sees in a pane now (`claude`), if any.
pub fn pane_agent(pane: &str) -> Option<String> {
    pane_field(pane, "agent")
}

/// What the agent in a pane does now (`idle`, `working`, ...), if known.
pub fn pane_status(pane: &str) -> Option<String> {
    pane_field(pane, "agent_status").filter(|s| s != "unknown")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn process_info_reads_shell_group_and_front() {
        // The shape of `herdr pane process-info` (Herdr's API schema).
        let v = json!({"id": "cli:pane:process_info", "result": {"type": "pane_process_info",
            "process_info": {"pane_id": "w1:p6", "shell_pid": 300,
                "foreground_process_group_id": 400,
                "foreground_processes": [{"pid": 400, "name": "2.1.30", "argv0": "claude"},
                    {"pid": 401, "name": "node"}]}}});
        let p = Processes::from_json(&v).unwrap();
        assert_eq!(p.shell, Some(300));
        assert_eq!(p.group, Some(400));
        assert_eq!(p.front, vec![(400, "2.1.30".into()), (401, "node".into())]);
        assert!(!p.shell_in_front());
        let back = Processes {
            shell: Some(300),
            group: Some(300),
            front: vec![],
        };
        assert!(back.shell_in_front());
        assert_eq!(Processes::from_json(&json!({"result": {}})), None);
    }

    #[test]
    fn workspace_list_reads_the_checkout_of_each_space() {
        // The shape of `herdr workspace list` (Herdr's API schema, 0.9.0).
        let v = json!({"id": "cli:workspace:list", "result": {"type": "workspace_list",
        "workspaces": [
            {"workspace_id": "w1", "number": 1, "label": "acme-app", "focused": true,
                "worktree": {"repo_key": "/src/acme-app/.git", "repo_name": "acme-app",
                    "repo_root": "/src/acme-app", "checkout_path": "/src/acme-app",
                    "is_linked_worktree": false}},
            {"workspace_id": "w2", "number": 2, "label": "fix-login", "focused": false,
                "worktree": {"repo_key": "/src/acme-app/.git", "repo_name": "acme-app",
                    "repo_root": "/src/acme-app", "checkout_path": "/src/wt/fix-login",
                    "is_linked_worktree": true}},
            {"workspace_id": "w3", "number": 3, "label": "notes", "focused": false}
        ]}});
        let list = workspaces_from(&v).unwrap();
        let key = "/src/acme-app/.git".to_string();
        assert_eq!(
            list.iter().map(|w| w.worktree.clone()).collect::<Vec<_>>(),
            vec![
                Some(Worktree {
                    repo_key: key.clone(),
                    linked: false
                }),
                Some(Worktree {
                    repo_key: key,
                    linked: true
                }),
                None,
            ]
        );
        assert_eq!(list[0].label, "acme-app");
        assert!(list[0].focused);
    }
}
