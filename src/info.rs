//! Who an account is signed in as: Claude Code's own `claude auth status`,
//! run with the account's folder, and the plan from the account's
//! `.claude.json`. Read only, for the list.

use crate::launch;
use crate::mirror;
use crate::paths;
use crate::secret;
use crate::store::{Account, Kind};
use serde_json::Value;

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
