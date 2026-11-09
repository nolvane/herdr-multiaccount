//! An account's own Claude Code folder. It holds only what belongs to the
//! login (the global config file with the signed-in account, the
//! organisation's policy files); everything else is a link to the same entry
//! in `~/.claude`, so settings, skills, plugins, memory and history stay one
//! setup. Claude Code keeps each folder's login apart by itself (a keychain
//! entry per folder on macOS).

use crate::paths;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub fn home_of(id: &str) -> PathBuf {
    paths::homes_dir().join(id)
}

/// Entries that belong to one login and are never shared.
pub fn is_own(name: &str) -> bool {
    const OWN: [&str; 7] = [
        ".credentials.json",
        "backups",
        ".DS_Store",
        // A background-session daemon runs with the login that started it.
        "daemon",
        "jobs",
        // claude.ai connectors differ per account.
        "mcp-needs-auth-cache.json",
        "statsig",
    ];
    OWN.contains(&name)
        || name.starts_with(".claude.json")
        || name.starts_with("remote-settings.json")
        || name.starts_with("policy-limits.json")
        || name.contains(".tmp")
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Links made this time.
    pub linked: usize,
    /// Files that had been replaced by a copy and were linked again.
    pub healed: Vec<String>,
    /// Entries that are folders of their own and stay so.
    pub separate: Vec<String>,
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// A name no other backup has: label, time to the nanosecond, process.
fn backup_name(label: &str) -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{label}.{}.{:09}.{}",
        t.as_secs(),
        t.subsec_nanos(),
        std::process::id()
    )
}

/// Copies `src` into the backup folder. A failed backup stops whatever was
/// about to replace the file.
fn backup(dir: &Path, src: &Path, label: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    paths::private_dir(dir)?;
    let data = std::fs::read(src)?;
    // The copy is never readable by more people than the original.
    let mode = paths::mode_of(&std::fs::metadata(src)?) & 0o666;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(dir.join(backup_name(label)))?;
    f.set_permissions(std::fs::Permissions::from_mode(mode))?;
    std::io::Write::write_all(&mut f, &data)
}

/// Points `link` at `target`, replacing whatever file or link is there in
/// one step (a new link under a temporary name, renamed over it).
fn relink(target: &Path, link: &Path) -> std::io::Result<()> {
    let name = link
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = link.with_file_name(format!(".{name}.link.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    std::fs::rename(&tmp, link)
}

/// Holds an account folder's lock while it lives, so two sessions never
/// change the folder (or heal the same file) at once.
pub struct Lock(#[allow(dead_code)] std::fs::File);

pub fn lock(home: &Path) -> std::io::Result<Lock> {
    let path = home.with_extension("lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    f.lock()?;
    Ok(Lock(f))
}

/// Links every shared entry of `main` into `home`. A file that Claude Code
/// replaced by a copy of its own (some programs save by writing a new file
/// over the link) is folded back: the newer of the two wins, the other goes
/// to `backups/`, and the link is made again. Takes the folder's lock.
pub fn sync_dirs(main: &Path, home: &Path, id: &str, backups: &Path) -> std::io::Result<Report> {
    std::fs::create_dir_all(home)?;
    let _lock = lock(home)?;
    let mut report = Report::default();
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(main)? {
        let name = entry?.file_name().to_string_lossy().to_string();
        if !is_own(&name) {
            names.push(name);
        }
    }
    names.sort();
    for name in &names {
        let target = main.join(name);
        let link = home.join(name);
        match std::fs::symlink_metadata(&link) {
            Err(_) => {
                relink(&target, &link)?;
                report.linked += 1;
            }
            Ok(m) if m.file_type().is_symlink() => {
                if std::fs::read_link(&link).ok().as_deref() != Some(target.as_path()) {
                    relink(&target, &link)?;
                    report.linked += 1;
                }
            }
            Ok(m) if m.is_file() && target.is_file() => {
                let copy = std::fs::read(&link)?;
                let shared = std::fs::read(&target)?;
                if copy != shared {
                    if modified(&link) > modified(&target) {
                        backup(backups, &target, &format!("main-{name}"))?;
                        // Into the real file, also when ~/.claude links it
                        // elsewhere; never by copying a file onto itself.
                        let real = target.canonicalize()?;
                        paths::write_atomic(&real, &copy)?;
                    } else {
                        backup(backups, &link, &format!("{id}-{name}"))?;
                    }
                }
                relink(&target, &link)?;
                report.healed.push(name.clone());
            }
            Ok(m) if m.is_dir() && std::fs::read_dir(&link)?.next().is_none() => {
                std::fs::remove_dir(&link)?;
                relink(&target, &link)?;
                report.linked += 1;
            }
            Ok(_) => report.separate.push(name.clone()),
        }
    }
    // Links whose entry in ~/.claude is gone.
    for entry in std::fs::read_dir(home)? {
        let entry = entry?;
        let path = entry.path();
        if let Ok(m) = std::fs::symlink_metadata(&path) {
            if m.file_type().is_symlink() && !path.exists() {
                if let Ok(t) = std::fs::read_link(&path) {
                    if t.starts_with(main) {
                        std::fs::remove_file(&path)?;
                    }
                }
            }
        }
    }
    Ok(report)
}

pub fn sync(id: &str) -> std::io::Result<Report> {
    let main = paths::claude_dir();
    std::fs::create_dir_all(&main)?;
    let home = home_of(id);
    let report = sync_dirs(&main, &home, id, &paths::backups_dir())?;
    let _lock = lock(&home)?;
    sync_claude_json(&paths::main_claude_json(), &home.join(".claude.json"))?;
    Ok(report)
}

/// Keys of `~/.claude.json` that a new account folder starts with, so Claude
/// Code skips its first-run questions. Nothing about the signed-in account.
const SEED_KEYS: [&str; 12] = [
    "hasCompletedOnboarding",
    "lastOnboardingVersion",
    "theme",
    "editorMode",
    "installMethod",
    "autoUpdates",
    "autoUpdatesProtectedForNative",
    "preferredNotifChannel",
    "lastReleaseNotesSeen",
    "migrationVersion",
    "officialMarketplaceAutoInstallAttempted",
    "officialMarketplaceAutoInstalled",
];

/// Brings the shared parts of `~/.claude.json` into an account's own copy:
/// your MCP servers and, per project, trust and tool settings. The account's
/// own per-project numbers (`last…`) stay.
pub fn merge_claude_json(main: &Value, own: Option<&Value>) -> Value {
    let mut out: Map<String, Value> = match own {
        Some(Value::Object(m)) => m.clone(),
        _ => {
            let mut m = Map::new();
            for k in SEED_KEYS {
                if let Some(v) = main.get(k) {
                    m.insert(k.to_string(), v.clone());
                }
            }
            m
        }
    };
    // Servers by name: the main ones, plus ones added in this account only.
    if let Some(Value::Object(servers)) = main.get("mcpServers") {
        let mut merged = match out.get("mcpServers") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        for (name, server) in servers {
            merged.insert(name.clone(), server.clone());
        }
        out.insert("mcpServers".into(), Value::Object(merged));
    }
    if let Some(Value::Object(projects)) = main.get("projects") {
        let mut merged = match out.get("projects") {
            Some(Value::Object(p)) => p.clone(),
            _ => Map::new(),
        };
        for (path, settings) in projects {
            let Value::Object(shared) = settings else {
                continue;
            };
            let mut p = shared.clone();
            if let Some(Value::Object(mine)) = merged.get(path) {
                for (k, v) in mine {
                    if k.starts_with("last") || !p.contains_key(k) {
                        p.insert(k.clone(), v.clone());
                    }
                }
            }
            merged.insert(path.clone(), Value::Object(p));
        }
        out.insert("projects".into(), Value::Object(merged));
    }
    Value::Object(out)
}

fn sync_claude_json(main: &Path, own: &Path) -> std::io::Result<()> {
    let Some(main_v) = std::fs::read_to_string(main)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Ok(());
    };
    let own_text = std::fs::read_to_string(own).ok();
    let own_v = own_text
        .as_deref()
        .and_then(|t| serde_json::from_str::<Value>(t).ok());
    // A copy Claude Code is halfway through writing is left alone.
    if own_text.is_some() && own_v.is_none() {
        return Ok(());
    }
    let merged = merge_claude_json(&main_v, own_v.as_ref());
    if Some(&merged) == own_v.as_ref() {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&merged).unwrap_or_default();
    // Private from the first byte: it holds MCP server settings, which can
    // carry tokens.
    paths::write_private(own, text.as_bytes())
}

/// Real entries in an account's folder that are neither links nor its own
/// login files: things made only there, which removing it keeps in backups.
pub fn own_data(id: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(home_of(id)) else {
        return vec![];
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !is_own(n))
        .collect();
    out.sort();
    out
}

/// Removes an account's folder: its links (never what they point to) and
/// its own login files. Anything else that is real (a folder Claude Code
/// made there before `~/.claude` had it) goes to `backups/` instead of being
/// deleted; their names are returned.
pub fn remove(id: &str) -> std::io::Result<Vec<String>> {
    if !crate::store::valid_id(id) || id == crate::store::MAIN_ID {
        return Ok(vec![]);
    }
    let home = home_of(id);
    match std::fs::symlink_metadata(&home) {
        Ok(m) if m.is_dir() => {}
        _ => return Ok(vec![]),
    }
    let _lock = lock(&home)?;
    let keep = paths::backups_dir().join(backup_name(&format!("{id}-removed")));
    let mut kept = Vec::new();
    for entry in std::fs::read_dir(&home)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let m = std::fs::symlink_metadata(&path)?;
        if m.file_type().is_symlink() {
            std::fs::remove_file(&path)?;
        } else if is_own(&name) {
            if m.is_dir() {
                // Does not follow links inside it either.
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        } else {
            std::fs::create_dir_all(&keep)?;
            std::fs::rename(&path, keep.join(&name))?;
            kept.push(name);
        }
    }
    std::fs::remove_dir(&home)?;
    let _ = std::fs::remove_file(home.with_extension("lock"));
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "herdr-multiaccount-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn own_entries() {
        assert!(is_own(".claude.json"));
        assert!(is_own(".claude.json.backup"));
        assert!(is_own("remote-settings.json.synced-at"));
        assert!(is_own("policy-limits.json.stamp.json.tmp.1"));
        assert!(is_own(".credentials.json"));
        assert!(!is_own("settings.json"));
        assert!(!is_own("projects"));
        assert!(!is_own("CLAUDE.md"));
    }

    #[test]
    fn links_and_heals() {
        let root = scratch("links");
        let main = root.join("main");
        let home = root.join("home");
        std::fs::create_dir_all(main.join("projects")).unwrap();
        std::fs::write(main.join("settings.json"), "{\"a\":1}").unwrap();
        std::fs::write(main.join("remote-settings.json"), "{}").unwrap();
        std::fs::write(main.join(".credentials.json"), "secret").unwrap();

        let r = sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        assert_eq!(r.linked, 2);
        assert!(std::fs::symlink_metadata(home.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!home.join("remote-settings.json").exists());
        assert!(!home.join(".credentials.json").exists());

        // Something saved settings.json as a new file over the link: the
        // newer copy goes back into main and the link returns.
        std::fs::remove_file(home.join("settings.json")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(home.join("settings.json"), "{\"a\":2}").unwrap();
        let r = sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        assert_eq!(r.healed, vec!["settings.json".to_string()]);
        assert_eq!(
            std::fs::read_to_string(main.join("settings.json")).unwrap(),
            "{\"a\":2}"
        );
        assert!(std::fs::symlink_metadata(home.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink());

        // A link whose entry is gone is removed.
        std::fs::remove_dir(main.join("projects")).unwrap();
        sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        assert!(std::fs::symlink_metadata(home.join("projects")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn heal_and_backups_keep_a_private_mode() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let root = scratch("modes");
        let main = root.join("main");
        let home = root.join("home");
        let backups = root.join("backups");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(main.join("settings.json"), "{\"env\":{\"DUMMY\":\"x\"}}").unwrap();
        std::fs::set_permissions(
            main.join("settings.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        sync_dirs(&main, &home, "x", &backups).unwrap();
        // Saved over the link as a newer file with the usual mode.
        std::fs::remove_file(home.join("settings.json")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(home.join("settings.json"), "{\"env\":{\"DUMMY\":\"y\"}}").unwrap();
        let r = sync_dirs(&main, &home, "x", &backups).unwrap();
        assert_eq!(r.healed, vec!["settings.json".to_string()]);
        assert_eq!(
            mode(&main.join("settings.json")),
            0o600,
            "the original stays private"
        );
        assert_eq!(mode(&backups), 0o700);
        let saved: Vec<PathBuf> = std::fs::read_dir(&backups)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            mode(&saved[0]),
            0o600,
            "the backup is no wider than its source"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn heal_writes_through_a_linked_settings_file() {
        let root = scratch("through");
        let main = root.join("main");
        let home = root.join("home");
        let repo = root.join("repo");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("settings.json"), "old").unwrap();
        std::os::unix::fs::symlink(repo.join("settings.json"), main.join("settings.json")).unwrap();
        sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        std::fs::remove_file(home.join("settings.json")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(home.join("settings.json"), "new").unwrap();
        sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        // The file in the repository got the content; ~/.claude's link stays.
        assert_eq!(
            std::fs::read_to_string(repo.join("settings.json")).unwrap(),
            "new"
        );
        assert!(std::fs::symlink_metadata(main.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        // Running it again changes nothing and empties nothing.
        sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        assert_eq!(
            std::fs::read_to_string(main.join("settings.json")).unwrap(),
            "new"
        );
        let backups = std::fs::read_dir(root.join("backups")).unwrap().count();
        assert_eq!(backups, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_real_folder_stays() {
        let root = scratch("folder");
        let main = root.join("main");
        let home = root.join("home");
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::create_dir_all(home.join("skills")).unwrap();
        std::fs::write(home.join("skills").join("x"), "x").unwrap();
        let r = sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        assert_eq!(r.separate, vec!["skills".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_json_merge() {
        let main = json!({
            "hasCompletedOnboarding": true,
            "theme": "dark",
            "oauthAccount": {"emailAddress": "alex@example.com"},
            "userID": "u1",
            "mcpServers": {"docs": {"command": "x"}},
            "projects": {"/p": {"hasTrustDialogAccepted": true, "lastCost": 5, "allowedTools": ["Bash"]}}
        });
        let fresh = merge_claude_json(&main, None);
        assert_eq!(fresh["theme"], "dark");
        assert!(fresh.get("oauthAccount").is_none(), "never the account");
        assert!(fresh.get("userID").is_none());
        assert_eq!(fresh["mcpServers"]["docs"]["command"], "x");

        let own = json!({
            "oauthAccount": {"emailAddress": "sam@example.com"},
            "projects": {"/p": {"lastCost": 1, "mine": true}, "/q": {"x": 1}}
        });
        let merged = merge_claude_json(&main, Some(&own));
        assert_eq!(merged["oauthAccount"]["emailAddress"], "sam@example.com");
        assert_eq!(merged["projects"]["/p"]["lastCost"], 1);
        assert_eq!(merged["projects"]["/p"]["hasTrustDialogAccepted"], true);
        assert_eq!(merged["projects"]["/p"]["mine"], true);
        assert_eq!(merged["projects"]["/q"]["x"], 1);

        let own = json!({"mcpServers": {"mine": {"command": "y"}, "docs": {"command": "old"}}});
        let merged = merge_claude_json(&main, Some(&own));
        assert_eq!(merged["mcpServers"]["mine"]["command"], "y");
        assert_eq!(merged["mcpServers"]["docs"]["command"], "x");
    }
}
