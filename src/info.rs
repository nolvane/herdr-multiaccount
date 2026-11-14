//! Who an account is signed in as: Claude Code's own `claude auth status`,
//! run with the account's folder, and the plan from the account's
//! `.claude.json`. Read only, for the list; the list notes each
//! subscription's result (`save_check`) so the launcher can skip asking
//! Claude Code again for a few minutes (`fresh_check`).

use crate::launch;
use crate::mirror;
use crate::paths;
use crate::secret;
use crate::store::{Account, Kind};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    /// Signed in with a login of its own (or a key is stored).
    pub signed_in: bool,
    /// The folder has no login of its own and would run on the main login.
    pub borrows_main: bool,
    /// The credential in use is the machine-wide Anthropic profile (a
    /// Claude Console sign-in without an API key).
    pub profile: bool,
    /// A claude.ai login (subscription) is the credential in use.
    pub claude_ai: bool,
    pub organization: Option<String>,
    pub email: Option<String>,
    /// Pro, Max 5x, Team, Enterprise, Console, API key …
    pub plan: Option<String>,
    /// A subscription that still runs on the folder an older version made
    /// and should sign in again (`mirror::on_old_folder`).
    pub old_folder: bool,
}

impl Info {
    /// Signed in with a login of its own, not one borrowed from the main
    /// login: what a folder needs to take a session.
    pub fn own_login(&self) -> bool {
        self.signed_in && !self.borrows_main
    }
}

/// Whether a folder can take a session, from its `claude auth status` text.
/// No answer (Claude Code could not be asked) counts as yes, so a session is
/// never kept from starting only because the check could not run.
pub fn takes_sessions(status: Option<&str>) -> bool {
    status.is_none_or(|t| parse_status(t).own_login())
}

/// Parses `claude auth status --text`. A row ending in "not in use" names a
/// credential that is there but loses to another one.
pub fn parse_status(text: &str) -> Info {
    let mut info = Info::default();
    let mut api_key = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let in_use = !value.to_lowercase().contains("not in use");
        match key.trim() {
            "Organization" => {
                let name = value.split(" · ").next().unwrap_or(value).trim();
                if !name.is_empty() {
                    info.organization = Some(name.to_string());
                }
            }
            "Email" if !value.is_empty() => info.email = Some(value.to_string()),
            "Profile" if in_use => info.profile = true,
            "API key" if in_use => api_key = true,
            "Login method" if in_use => {
                info.claude_ai = true;
                // "Claude Max Account" → "Max".
                let v = value
                    .trim_start_matches("Claude ")
                    .trim_end_matches(" Account")
                    .trim_end_matches(" account")
                    .trim();
                if !v.is_empty() {
                    info.plan = Some(v.to_string());
                }
            }
            _ => {}
        }
    }
    if info.profile && !info.claude_ai {
        info.plan = Some("Console".into());
    } else if api_key && !info.claude_ai {
        info.plan = Some("Console API key".into());
    }
    // Only Claude Code's own words count, never an organisation name or an
    // email address that happens to contain "not logged in".
    let lower: String = text
        .lines()
        .filter(|l| {
            let key = l.split_once(':').map(|(k, _)| k.trim());
            !matches!(key, Some("Organization") | Some("Email"))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    info.signed_in = !lower.contains("not logged in")
        && !lower.contains("not signed in")
        && (info.claude_ai || info.profile || api_key || info.email.is_some());
    info.borrows_main = info.profile && !info.claude_ai && !api_key;
    info
}

/// `claude auth status --text` for the main login (`None`) or an account
/// folder. `None` when Claude Code could not be asked.
pub fn status_text(home: Option<&std::path::Path>) -> Option<String> {
    let claude = launch::find_claude()?;
    let mut cmd = std::process::Command::new(claude);
    // Clean first, so CLAUDE_CONFIG_DIR set below stays.
    launch::clean_env(&mut cmd);
    cmd.args(["auth", "status", "--text"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for v in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        cmd.env_remove(v);
    }
    match home {
        Some(h) => cmd.env("CLAUDE_CONFIG_DIR", h),
        None => cmd.env_remove("CLAUDE_CONFIG_DIR"),
    };
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    (!text.trim().is_empty()).then_some(text)
}

/// How long the list's sign-in check of a subscription stands in for the
/// launcher's own: the list's 10-minute refresh plus the time the refresh
/// itself takes, so a start right before the next result still uses it.
pub const CHECK_FRESH_SECS: i64 = 11 * 60;

/// The file with the list's last sign-in check of account `id` in `dir`.
/// `None` for an id that is not a plain account id, so nothing is ever read
/// or written outside `dir`.
fn check_file(dir: &Path, id: &str) -> Option<PathBuf> {
    crate::store::valid_id(id).then(|| dir.join(format!("{id}.json")))
}

/// Notes the list's sign-in check of a subscription: the folder it asked
/// Claude Code about, when the check started, and whether that folder had a
/// login of its own.
pub fn save_check(dir: &Path, id: &str, folder: &Path, signed_in: bool, at: i64) {
    let Some(file) = check_file(dir, id) else {
        return;
    };
    let text = serde_json::json!({
        "folder": folder.to_string_lossy(),
        "checked_at": at,
        "signed_in": signed_in,
    })
    .to_string();
    let _ = paths::write_atomic(&file, format!("{text}\n").as_bytes());
}

/// Throws away the list's sign-in check of `id`, before anything that can
/// change its login.
pub fn forget_check(dir: &Path, id: &str) {
    if let Some(file) = check_file(dir, id) {
        let _ = std::fs::remove_file(file);
    }
}

/// Whether the list found `folder` signed in with a login of its own at
/// most [`CHECK_FRESH_SECS`] before `now`. Anything else (no file, an older
/// or damaged one, another folder, "signed out", a time ahead of `now`) is
/// no: the caller asks Claude Code. A cached "signed out" is never used, so
/// a sign-in done outside the list is never hidden.
pub fn fresh_check(dir: &Path, id: &str, folder: &Path, now: i64) -> bool {
    let Some(file) = check_file(dir, id) else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let (Some(at), Some(dir_of_check), Some(true)) = (
        v["checked_at"].as_i64(),
        v["folder"].as_str(),
        v["signed_in"].as_bool(),
    ) else {
        return false;
    };
    let age = now - at;
    Path::new(dir_of_check) == folder && (0..=CHECK_FRESH_SECS).contains(&age)
}

/// Whether the main login is the machine-wide Anthropic profile. A claude.ai
/// sign-in anywhere on the machine signs that profile out (Claude Code does
/// this), so the list refuses to add a subscription while it is in use.
pub fn main_uses_profile() -> bool {
    status_text(None).is_some_and(|t| parse_status(&t).profile)
}

/// The plan from the account fields Claude Code keeps in `.claude.json`.
pub fn plan_from_account(oauth: &Value) -> Option<String> {
    let mut words = String::new();
    for k in [
        "seatTier",
        "organizationType",
        "billingType",
        "userRateLimitTier",
        "organizationRateLimitTier",
    ] {
        if let Some(s) = oauth[k].as_str() {
            words.push_str(&s.to_lowercase());
            words.push(' ');
        }
    }
    if words.trim().is_empty() {
        return None;
    }
    let plan = if words.contains("enterprise") {
        "Enterprise"
    } else if words.contains("team") {
        "Team"
    } else if words.contains("max_20x") || words.contains("max 20x") {
        "Max 20x"
    } else if words.contains("max_5x") || words.contains("max 5x") {
        "Max 5x"
    } else if words.contains("ultra") {
        "Ultra"
    } else if words.contains("max") {
        "Max"
    } else if words.contains("pro") {
        "Pro"
    } else {
        "Subscription"
    };
    Some(plan.to_string())
}

fn oauth_of(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let o = v.get("oauthAccount")?.clone();
    o.is_object().then_some(o)
}

/// Looks the account up. Takes up to a second (it starts Claude Code's
/// command line), so the list calls it off the main thread.
pub fn read(account: &Account) -> Info {
    match account.kind {
        Kind::ApiKey => Info {
            signed_in: secret::exists(&account.id),
            plan: Some("API key".into()),
            ..Info::default()
        },
        Kind::Main | Kind::Subscription => {
            let home = (account.kind == Kind::Subscription).then(|| mirror::folder(&account.id));
            // A new folder that is not there yet (or only an old link of
            // that name) has no login: Claude Code is not asked about it.
            if let Some(h) = &home {
                if *h != mirror::home_of(&account.id)
                    && mirror::real_new_dir(&mirror::home_of(&account.id)).is_none()
                {
                    return Info::default();
                }
            }
            let json = match &home {
                Some(h) => h.join(".claude.json"),
                None => paths::main_claude_json(),
            };
            let text = status_text(home.as_deref()).unwrap_or_default();
            let mut info = parse_status(&text);
            if account.kind == Kind::Main {
                // Whatever the main login is, it is its own.
                info.borrows_main = false;
            } else if info.borrows_main {
                info.signed_in = false;
            }
            info.old_folder =
                account.kind == Kind::Subscription && mirror::on_old_folder(&account.id);
            // The account fields in .claude.json describe the last claude.ai
            // login, which is not always the credential in use.
            if info.claude_ai {
                if let Some(o) = oauth_of(&json) {
                    if let Some(p) = plan_from_account(&o) {
                        info.plan = Some(p);
                    }
                    if info.organization.is_none() {
                        info.organization = o["organizationName"].as_str().map(str::to_string);
                    }
                    if info.email.is_none() {
                        info.email = o["emailAddress"].as_str().map(str::to_string);
                    }
                }
            }
            info
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn takes_sessions_only_with_a_login_of_its_own() {
        // Claude Code could not be asked: start as before.
        assert!(takes_sessions(None));
        assert!(takes_sessions(Some(
            "Login method: Claude Max Account\nEmail: alex@example.com\n"
        )));
        assert!(!takes_sessions(Some(
            "Not logged in. Run claude auth login.\n"
        )));
        assert!(!takes_sessions(Some("")));
        // Only the machine's Console profile, borrowed from the main login.
        assert!(!takes_sessions(Some(
            "Profile: credentials-file · user_oauth · profile default\nOrganization: Acme · 1\n"
        )));
    }

    #[test]
    fn status_text() {
        let i = parse_status(
            "Login method: Claude Max Account\nOrganization: Acme · 0000-1111\nEmail: alex@example.com\n",
        );
        assert!(i.signed_in);
        assert!(!i.borrows_main);
        assert_eq!(i.plan.as_deref(), Some("Max"));
        assert_eq!(i.organization.as_deref(), Some("Acme"));
        assert_eq!(i.email.as_deref(), Some("alex@example.com"));

        let p = parse_status(
            "Profile: credentials-file · user_oauth · profile default\nOrganization: Acme · 1\n",
        );
        assert!(p.borrows_main);
        assert!(!parse_status("Not logged in. Run claude auth login.\n").signed_in);
        // An organisation or email that contains the words does not count.
        let odd = parse_status(
            "Login method: Claude Max Account\nOrganization: Not Logged In Ltd · 1\nEmail: not.signed.in@example.com\n",
        );
        assert!(odd.signed_in);
        assert!(!parse_status("").signed_in);
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hm-info-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const NOW: i64 = 1_794_225_600;

    #[test]
    fn a_fresh_signed_in_check_is_reused() {
        let d = scratch("fresh");
        let folder = d.join("homes/work/.claude");
        save_check(&d, "work", &folder, true, NOW - 300);
        assert!(fresh_check(&d, "work", &folder, NOW));
        // Up to the refresh plus its run time.
        assert!(fresh_check(
            &d,
            "work",
            &folder,
            NOW - 300 + CHECK_FRESH_SECS
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_old_or_missing_check_is_not_reused() {
        let d = scratch("old");
        let folder = d.join("homes/work/.claude");
        assert!(!fresh_check(&d, "work", &folder, NOW), "never checked");
        save_check(&d, "work", &folder, true, NOW - CHECK_FRESH_SECS - 1);
        assert!(!fresh_check(&d, "work", &folder, NOW), "too old");
        // A time ahead of now (the clock was set back) is not trusted.
        save_check(&d, "work", &folder, true, NOW + 60);
        assert!(!fresh_check(&d, "work", &folder, NOW));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_signed_out_check_is_never_reused() {
        let d = scratch("out");
        let folder = d.join("homes/work/.claude");
        save_check(&d, "work", &folder, false, NOW - 10);
        assert!(!fresh_check(&d, "work", &folder, NOW));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_check_of_another_folder_is_not_reused() {
        let d = scratch("other");
        // Checked on the old folder; sessions now use the new one.
        save_check(&d, "work", &d.join("homes/work"), true, NOW - 10);
        assert!(!fresh_check(&d, "work", &d.join("homes/work/.claude"), NOW));
        // Nor one of another account.
        let folder = d.join("homes/work/.claude");
        save_check(&d, "home", &folder, true, NOW - 10);
        assert!(!fresh_check(&d, "personal", &folder, NOW));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_check_leads_to_the_live_one() {
        let d = scratch("damaged");
        let folder = d.join("homes/work/.claude");
        for text in [
            "",
            "{",
            "not json",
            r#"{"folder": 3, "checked_at": 1794225590, "signed_in": true}"#,
            r#"{"checked_at": 1794225590, "signed_in": true}"#,
            r#"{"folder": "x", "checked_at": "soon", "signed_in": true}"#,
            r#"{"folder": "x", "checked_at": 1794225590, "signed_in": "yes"}"#,
        ] {
            std::fs::write(d.join("work.json"), text).unwrap();
            assert!(!fresh_check(&d, "work", &folder, NOW), "{text}");
        }
        // A folder in place of the file.
        let _ = std::fs::remove_file(d.join("work.json"));
        std::fs::create_dir_all(d.join("work.json")).unwrap();
        assert!(!fresh_check(&d, "work", &folder, NOW));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn forgetting_a_check_removes_it() {
        let d = scratch("forget");
        let folder = d.join("homes/work/.claude");
        save_check(&d, "work", &folder, true, NOW - 10);
        forget_check(&d, "work");
        assert!(!d.join("work.json").exists());
        assert!(!fresh_check(&d, "work", &folder, NOW));
        // Nothing there: nothing happens.
        forget_check(&d, "work");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn checks_stay_inside_their_folder() {
        let d = scratch("inside");
        let folder = d.join("x");
        for id in ["../escape", "a/b", "", "Main"] {
            save_check(&d, id, &folder, true, NOW);
            assert!(!fresh_check(&d, id, &folder, NOW), "{id}");
            forget_check(&d, id);
        }
        assert!(!d.parent().unwrap().join("escape.json").exists());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn plans() {
        assert_eq!(
            plan_from_account(&json!({"billingType": "stripe_subscription_enterprise_self_serve"}))
                .as_deref(),
            Some("Enterprise")
        );
        assert_eq!(
            plan_from_account(&json!({"userRateLimitTier": "default_claude_max_20x"})).as_deref(),
            Some("Max 20x")
        );
        assert_eq!(
            plan_from_account(&json!({"organizationType": "claude_pro"})).as_deref(),
            Some("Pro")
        );
        assert_eq!(plan_from_account(&json!({})), None);
    }
}
