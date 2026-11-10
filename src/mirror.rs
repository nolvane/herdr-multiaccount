//! An account's own Claude Code folder. It holds only what belongs to the
//! login (the global config file with the signed-in account, the
//! organisation's policy files); everything else is a link to the same entry
//! in `~/.claude`, so settings, skills, plugins, memory and history stay one
//! setup. Claude Code keeps each folder's login apart by itself (a keychain
//! entry per folder on macOS, keyed by the folder's path as given).
//!
//! The folder is `homes/<id>/.claude`, so paths through its links name
//! `.claude` as on the main login and Claude Code's permission checks treat
//! them the same way. Version 1.0.0 used `homes/<id>` itself (the "old
//! folder"); an account signed in there keeps using it until it signs in
//! again in the new folder.

use crate::paths;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The name of an account's Claude Code folder inside `homes/<id>`.
pub const NEW: &str = ".claude";

/// The account's folder, `homes/<id>`: the old folder, and the place of the
/// new one.
pub fn home_of(id: &str) -> PathBuf {
    paths::homes_dir().join(id)
}

/// The account's Claude Code folder, `homes/<id>/.claude`.
pub fn new_dir_of(id: &str) -> PathBuf {
    home_of(id).join(NEW)
}

/// Whether Claude Code signed in in this folder at least once: it wrote the
/// account into the folder's config. (Whether the login still works takes a
/// second: `claude auth status`.)
pub fn has_login(dir: &Path) -> bool {
    read_json(&dir.join(".claude.json")).is_some_and(|v| v["oauthAccount"].is_object())
}

/// The note that an account signed in in its new folder, `homes/<id>.new`:
/// from then on it never goes back to its old folder, also when the new
/// folder is signed out or its config is being rewritten.
fn moved_file(home: &Path) -> PathBuf {
    home.with_extension("new")
}

/// Notes that the account in `home` signed in in its new folder.
pub fn mark_moved(home: &Path) {
    if home.is_dir() {
        let _ = std::fs::write(moved_file(home), "");
    }
}

/// The folder sessions of the account in `home` run on: the new folder once
/// it had a login; the old folder while only that one has a login (an
/// account from version 1.0.0 that has not signed in again); otherwise the
/// new folder, where every sign-in happens.
pub fn folder_in(home: &Path) -> PathBuf {
    let new = home.join(NEW);
    if moved_file(home).exists() {
        return new;
    }
    // An old link of that name leads into `~/.claude`: no login of its own.
    if real_new_dir(home).is_some_and(|d| has_login(&d)) {
        mark_moved(home);
        return new;
    }
    if has_login(home) {
        home.to_path_buf()
    } else {
        new
    }
}

pub fn folder(id: &str) -> PathBuf {
    folder_in(&home_of(id))
}

/// Whether the account still runs on its old folder and should sign in
/// again.
pub fn on_old_folder(id: &str) -> bool {
    folder(id) == home_of(id)
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

/// One lock per account, `homes/<id>.lock`, for its old and its new folder.
fn lock_file(dir: &Path) -> PathBuf {
    match dir.parent() {
        Some(home) if dir.file_name().is_some_and(|n| n == NEW) => home.with_extension("lock"),
        _ => dir.with_extension("lock"),
    }
}

pub fn lock(home: &Path) -> std::io::Result<Lock> {
    let path = lock_file(home);
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
    // An account folder is never a link: one (such as an old link named
    // `.claude`) would lead the links into `~/.claude` itself.
    if std::fs::symlink_metadata(home).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::other("the account folder is a link"));
    }
    std::fs::create_dir_all(home)?;
    let _lock = lock(home)?;
    let mut report = Report::default();
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(main)? {
        let name = entry?.file_name().to_string_lossy().to_string();
        // The new folder lives under this name inside the old one, so it is
        // never made a link.
        if !is_own(&name) && name != NEW {
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

/// Brings the folder sessions of the account run on in step; returns it.
pub fn sync(id: &str) -> std::io::Result<(PathBuf, Report)> {
    let dir = folder(id);
    let report = sync_at(id, &dir)?;
    Ok((dir, report))
}

/// Brings one of the account's folders in step: links, and the shared parts
/// of `~/.claude.json` in its own copy.
pub fn sync_at(id: &str, dir: &Path) -> std::io::Result<Report> {
    let main = paths::claude_dir();
    std::fs::create_dir_all(&main)?;
    sync_folder(
        &main,
        &paths::main_claude_json(),
        &home_of(id),
        dir,
        id,
        &paths::backups_dir(),
    )
}

fn sync_folder(
    main: &Path,
    main_json: &Path,
    home: &Path,
    dir: &Path,
    id: &str,
    backups: &Path,
) -> std::io::Result<Report> {
    if dir != home {
        // Version 1.0.0 linked an entry `.claude` of `~/.claude` here, if
        // there was one: the new folder must never be made through it.
        let _lock = lock(home)?;
        if std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
            std::fs::remove_file(dir)?;
        }
    }
    let report = sync_dirs(main, dir, id, backups)?;
    let _lock = lock(dir)?;
    // A new folder starts with what the old one had of the shared parts.
    let seed = (dir != home).then(|| home.join(".claude.json"));
    sync_claude_json(main_json, &dir.join(".claude.json"), seed.as_deref())?;
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

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
}

/// `seed`: where a missing own copy takes its first shared parts from (the
/// old folder's copy; `merge_claude_json` takes only its MCP servers and
/// per-project settings, never the account fields).
fn sync_claude_json(main: &Path, own: &Path, seed: Option<&Path>) -> std::io::Result<()> {
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
    let seeded = match (&own_v, seed.and_then(read_json)) {
        // The first-run keys come from `~/.claude.json`; the old copy adds
        // only its MCP servers and per-project settings.
        (None, Some(old)) => Some(merge_claude_json(
            &old,
            Some(&merge_claude_json(&main_v, None)),
        )),
        _ => None,
    };
    let merged = merge_claude_json(&main_v, own_v.as_ref().or(seeded.as_ref()));
    if Some(&merged) == own_v.as_ref() {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&merged).unwrap_or_default();
    // Private from the first byte: it holds MCP server settings, which can
    // carry tokens.
    paths::write_private(own, text.as_bytes())
}

/// The new folder inside `home`, when it is a real folder (never a link,
/// which would lead somewhere else).
pub fn real_new_dir(home: &Path) -> Option<PathBuf> {
    let new = home.join(NEW);
    std::fs::symlink_metadata(&new)
        .is_ok_and(|m| m.is_dir())
        .then_some(new)
}

/// Real entries of one folder that are neither links nor login files.
fn data_in(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_symlink()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !is_own(n) && !(prefix.is_empty() && n == NEW))
        .map(|n| format!("{prefix}{n}"))
        .collect()
}

/// Real entries in an account's folders that are neither links nor its own
/// login files: things made only there, which removing it keeps in backups.
/// Entries of the new folder are named `.claude/<name>`.
pub fn own_data(id: &str) -> Vec<String> {
    own_data_in(&home_of(id))
}

fn own_data_in(home: &Path) -> Vec<String> {
    let mut out = data_in(home, "");
    if let Some(new) = real_new_dir(home) {
        out.extend(data_in(&new, &format!("{NEW}/")));
    }
    out.sort();
    out
}

/// Empties one folder: links are removed (never followed), login files
/// deleted, anything else real moved to `keep`. Returns the names kept,
/// with `prefix`. The new folder inside the old one is left to the caller.
fn clear(dir: &Path, keep: &Path, prefix: &str) -> std::io::Result<Vec<String>> {
    let mut kept = Vec::new();
    for entry in std::fs::read_dir(dir)? {
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
            std::fs::create_dir_all(keep)?;
            std::fs::rename(&path, keep.join(&name))?;
            kept.push(format!("{prefix}{name}"));
        }
    }
    Ok(kept)
}

/// Removes an account's folders, the new one and the old one around it:
/// their links (never what they point to) and their own login files.
/// Anything else that is real (a folder Claude Code made there before
/// `~/.claude` had it) goes to `backups/` instead of being deleted; their
/// names are returned.
pub fn remove(id: &str) -> std::io::Result<Vec<String>> {
    if !crate::store::valid_id(id) || id == crate::store::MAIN_ID {
        return Ok(vec![]);
    }
    remove_in(&home_of(id), &paths::backups_dir(), id)
}

fn remove_in(home: &Path, backups: &Path, id: &str) -> std::io::Result<Vec<String>> {
    match std::fs::symlink_metadata(home) {
        Ok(m) if m.is_dir() => {}
        _ => return Ok(vec![]),
    }
    let _lock = lock(home)?;
    let keep = backups.join(backup_name(&format!("{id}-removed")));
    let mut kept = Vec::new();
    if let Some(new) = real_new_dir(home) {
        kept.extend(clear(&new, &keep.join(NEW), &format!("{NEW}/"))?);
        std::fs::remove_dir(&new)?;
    }
    kept.extend(clear(home, &keep, "")?);
    std::fs::remove_dir(home)?;
    let _ = std::fs::remove_file(lock_file(home));
    let _ = std::fs::remove_file(moved_file(home));
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

    /// Every entry under `dir`, links not followed: path, and the link
    /// target or the file's content.
    fn snapshot(dir: &Path) -> Vec<(String, String)> {
        let mut out = vec![];
        let mut todo = vec![dir.to_path_buf()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let m = std::fs::symlink_metadata(&p).unwrap();
                let what = if m.file_type().is_symlink() {
                    format!("-> {}", std::fs::read_link(&p).unwrap().display())
                } else if m.is_dir() {
                    todo.push(p.clone());
                    "dir".into()
                } else {
                    std::fs::read_to_string(&p).unwrap_or_default()
                };
                out.push((p.strip_prefix(dir).unwrap().display().to_string(), what));
            }
        }
        out.sort();
        out
    }

    fn is_link(p: &Path) -> bool {
        std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
    }

    const SIGNED_IN: &str = r#"{"oauthAccount":{"emailAddress":"alex@example.com"},"userID":"u1"}"#;

    #[test]
    fn which_folder_an_account_uses() {
        let home = scratch("which").join("homes").join("work");
        let new = home.join(NEW);
        // Never signed in, or new: the new folder (no notice).
        assert_eq!(folder_in(&home), new);
        std::fs::create_dir_all(&home).unwrap();
        // An old folder that never signed in does not count either.
        std::fs::write(home.join(".claude.json"), r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(folder_in(&home), new);
        // Signed in on the old folder by an older version: stays there.
        std::fs::write(home.join(".claude.json"), SIGNED_IN).unwrap();
        assert_eq!(folder_in(&home), home);
        // The new folder exists but its sign-in was cancelled: still old.
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join(".claude.json"), r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(folder_in(&home), home);
        // Signed in again: the new folder from now on.
        std::fs::write(new.join(".claude.json"), SIGNED_IN).unwrap();
        assert_eq!(folder_in(&home), new);
        assert!(new.ends_with(".claude"), "the path names .claude");
        // Signed out there later, or its config half written: it never goes
        // back to the old login.
        std::fs::write(new.join(".claude.json"), "{\"oauthAcc").unwrap();
        assert_eq!(folder_in(&home), new);
        std::fs::remove_file(moved_file(&home)).unwrap();
        // An old link of that name never counts as the account's login.
        let elsewhere = home.parent().unwrap().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join(".claude.json"), SIGNED_IN).unwrap();
        std::fs::remove_dir_all(&new).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &new).unwrap();
        assert_eq!(folder_in(&home), home);
        let _ = std::fs::remove_dir_all(home.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn both_folders_share_one_lock() {
        let home = Path::new("/x/homes/work");
        assert_eq!(lock_file(home), Path::new("/x/homes/work.lock"));
        assert_eq!(lock_file(&home.join(NEW)), Path::new("/x/homes/work.lock"));
    }

    /// A fake `~/.claude`, `~/.claude.json` and an account signed in on its
    /// old folder by an older version.
    fn old_layout(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let main = root.join("dot-claude");
        let main_json = root.join("dot-claude.json");
        let home = root.join("homes").join("work");
        std::fs::create_dir_all(main.join("skills").join("notes")).unwrap();
        std::fs::write(main.join("skills").join("notes").join("SKILL.md"), "skill").unwrap();
        std::fs::write(main.join("settings.json"), "{\"a\":1}").unwrap();
        std::fs::write(main.join("CLAUDE.md"), "rules").unwrap();
        std::fs::write(main.join(".credentials.json"), "main secret").unwrap();
        std::fs::write(
            &main_json,
            r#"{"theme":"dark","oauthAccount":{"emailAddress":"main@example.com"},"mcpServers":{"docs":{"command":"x"}},"projects":{"/p":{"hasTrustDialogAccepted":true}}}"#,
        )
        .unwrap();
        sync_dirs(&main, &home, "work", &root.join("backups")).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"alex@example.com"},"userID":"u1","mcpServers":{"mine":{"command":"y"}},"projects":{"/q":{"hasTrustDialogAccepted":true}}}"#,
        )
        .unwrap();
        std::fs::write(home.join(".credentials.json"), "old login").unwrap();
        (main, main_json, home)
    }

    #[test]
    fn the_new_folder_links_the_same_entries_and_starts_signed_out() {
        let root = scratch("newfolder");
        let (main, main_json, home) = old_layout(&root);
        let before = snapshot(&main);
        let new = home.join(NEW);
        sync_folder(
            &main,
            &main_json,
            &home,
            &new,
            "work",
            &root.join("backups"),
        )
        .unwrap();
        for name in ["skills", "settings.json", "CLAUDE.md"] {
            assert!(is_link(&new.join(name)), "{name} is shared");
            assert_eq!(std::fs::read_link(new.join(name)).unwrap(), main.join(name));
        }
        // A write through the new folder lands in ~/.claude.
        assert_eq!(
            std::fs::read_to_string(new.join("skills/notes/SKILL.md")).unwrap(),
            "skill"
        );
        assert!(
            !new.join(".credentials.json").exists(),
            "no login is copied"
        );
        let json: Value =
            serde_json::from_str(&std::fs::read_to_string(new.join(".claude.json")).unwrap())
                .unwrap();
        assert!(json.get("oauthAccount").is_none(), "{json}");
        assert!(json.get("userID").is_none(), "{json}");
        assert_eq!(json["theme"], "dark");
        assert_eq!(json["mcpServers"]["docs"]["command"], "x");
        assert_eq!(
            json["mcpServers"]["mine"]["command"], "y",
            "the old folder's server"
        );
        assert_eq!(json["projects"]["/p"]["hasTrustDialogAccepted"], true);
        assert_eq!(json["projects"]["/q"]["hasTrustDialogAccepted"], true);
        // The new folder has no login yet, so the account stays on the old
        // folder, which still has everything.
        assert!(!has_login(&new));
        assert_eq!(folder_in(&home), home);
        assert!(is_link(&home.join("skills")));
        assert_eq!(
            std::fs::read_to_string(home.join(".credentials.json")).unwrap(),
            "old login"
        );
        // Syncing the old folder again leaves the new one alone.
        sync_folder(
            &main,
            &main_json,
            &home,
            &home,
            "work",
            &root.join("backups"),
        )
        .unwrap();
        assert!(std::fs::symlink_metadata(&new).unwrap().is_dir());
        assert!(!is_link(&new));
        assert_eq!(snapshot(&main), before, "nothing in ~/.claude changed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_claude_entry_in_dot_claude_never_replaces_the_new_folder() {
        let root = scratch("nested");
        let main = root.join("main");
        let home = root.join("home");
        std::fs::create_dir_all(main.join(NEW)).unwrap();
        std::fs::create_dir_all(home.join(NEW)).unwrap();
        // Empty, as right after a cancelled sign-in.
        let r = sync_dirs(&main, &home, "x", &root.join("backups")).unwrap();
        assert_eq!(r.linked, 0);
        assert!(!is_link(&home.join(NEW)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn removing_takes_both_folders_and_never_follows_a_link() {
        let root = scratch("remove-both");
        let (main, main_json, home) = old_layout(&root);
        let new = home.join(NEW);
        sync_folder(
            &main,
            &main_json,
            &home,
            &new,
            "work",
            &root.join("backups"),
        )
        .unwrap();
        std::fs::write(new.join(".claude.json"), SIGNED_IN).unwrap();
        std::fs::write(new.join(".credentials.json"), "new login").unwrap();
        std::fs::create_dir_all(new.join("plans")).unwrap();
        std::fs::write(new.join("plans").join("a.md"), "mine").unwrap();
        std::fs::create_dir_all(home.join("notes")).unwrap();
        std::fs::write(lock_file(&home), "").unwrap();
        mark_moved(&home);
        assert!(moved_file(&home).exists());
        let before = snapshot(&main);
        let mut data = own_data_in(&home);
        data.sort();
        assert_eq!(data, vec![".claude/plans".to_string(), "notes".to_string()]);

        let backups = root.join("backups");
        let mut kept = remove_in(&home, &backups, "work").unwrap();
        kept.sort();
        assert_eq!(kept, vec![".claude/plans".to_string(), "notes".to_string()]);
        assert!(
            std::fs::symlink_metadata(&home).is_err(),
            "both folders are gone"
        );
        assert!(!lock_file(&home).exists());
        assert!(!moved_file(&home).exists());
        assert_eq!(snapshot(&main), before, "nothing in ~/.claude changed");
        let saved = snapshot(&backups);
        assert!(
            saved
                .iter()
                .any(|(p, c)| p.ends_with(".claude/plans/a.md") && c == "mine"),
            "{saved:?}"
        );
        assert!(
            !saved.iter().any(|(_, c)| c.contains("login")),
            "login files are deleted, not kept"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn removing_an_account_whose_claude_entry_is_a_link_only_unlinks_it() {
        let root = scratch("remove-link");
        let elsewhere = root.join("elsewhere");
        let home = root.join("homes").join("work");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("keep.txt"), "keep").unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join(NEW)).unwrap();
        assert!(own_data_in(&home).is_empty());
        remove_in(&home, &root.join("backups"), "work").unwrap();
        assert!(std::fs::symlink_metadata(&home).is_err());
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("keep.txt")).unwrap(),
            "keep"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn removing_a_new_account_takes_its_folder() {
        let root = scratch("remove-new");
        let main = root.join("main");
        let main_json = root.join("main.json");
        let home = root.join("homes").join("fresh");
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::write(&main_json, "{}").unwrap();
        let before = snapshot(&main);
        sync_folder(
            &main,
            &main_json,
            &home,
            &home.join(NEW),
            "fresh",
            &root.join("backups"),
        )
        .unwrap();
        assert!(
            !home.join(".claude.json").exists(),
            "the old place stays empty"
        );
        assert!(remove_in(&home, &root.join("backups"), "fresh")
            .unwrap()
            .is_empty());
        assert!(std::fs::symlink_metadata(&home).is_err());
        assert_eq!(snapshot(&main), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_old_link_named_claude_is_never_followed_by_the_new_folder() {
        let root = scratch("oldlink");
        let main = root.join("main");
        let main_json = root.join("main.json");
        let home = root.join("homes").join("work");
        std::fs::create_dir_all(main.join(NEW)).unwrap();
        std::fs::write(&main_json, "{}").unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(main.join(NEW), home.join(NEW)).unwrap();
        let before = snapshot(&main);
        sync_folder(
            &main,
            &main_json,
            &home,
            &home.join(NEW),
            "work",
            &root.join("backups"),
        )
        .unwrap();
        assert!(!is_link(&home.join(NEW)), "a real folder now");
        assert_eq!(
            snapshot(&main),
            before,
            "nothing was written into ~/.claude"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn links_are_never_made_through_a_folder_that_is_a_link() {
        let root = scratch("linkhome");
        let main = root.join("main");
        let home = root.join("home");
        std::fs::create_dir_all(main.join(NEW)).unwrap();
        std::fs::write(main.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(main.join(NEW), home.join(NEW)).unwrap();
        let before = snapshot(&main);
        assert!(sync_dirs(&main, &home.join(NEW), "x", &root.join("backups")).is_err());
        assert_eq!(snapshot(&main), before);
        let _ = std::fs::remove_dir_all(&root);
    }
}
