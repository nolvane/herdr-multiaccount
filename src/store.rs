//! `accounts.json`: the accounts, which one is active, the order fallback
//! follows and which space uses which account. Written by the list, read by
//! the `claude` shell function and the status line.

use crate::paths;
use serde_json::{json, Map, Value};

/// How an account signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Whatever Claude Code's own `/login` holds in `~/.claude`.
    Main,
    /// A claude.ai login (Pro, Max, Ultra, Team or Enterprise) that Claude
    /// Code keeps in the account's own folder.
    Subscription,
    /// An API key (Claude Console), kept in the system keychain.
    ApiKey,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Main => "main",
            Kind::Subscription => "subscription",
            Kind::ApiKey => "api-key",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        match s {
            "main" => Some(Kind::Main),
            "subscription" => Some(Kind::Subscription),
            "api-key" => Some(Kind::ApiKey),
            _ => None,
        }
    }

    /// Whether the account has a folder of its own under `homes/`.
    pub fn has_home(self) -> bool {
        self == Kind::Subscription
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// Never changes; names the account's folder and keychain entry.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Sign in through the organisation's single sign-on.
    pub sso: bool,
    /// Whether fallback may move new sessions onto this account.
    pub fallback: bool,
}

/// A space (Herdr workspace) that uses a given account.
#[derive(Debug, Clone, PartialEq)]
pub struct Space {
    pub workspace_id: String,
    /// The space's name when it was assigned; finds it again when Herdr
    /// gave it a new id.
    pub label: String,
    pub account: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Store {
    /// The account new sessions use, unless their space says otherwise.
    pub active: String,
    /// In list order, which is also the fallback order.
    pub accounts: Vec<Account>,
    pub spaces: Vec<Space>,
    /// The status line command that ran before setup wrapped it.
    pub statusline_original: Option<String>,
    /// Fallback notices already shown: account id and the reset time of the
    /// window that was full, so each one is announced once.
    pub notified: Vec<(String, i64)>,
    /// The note on how the list works was shown on first open.
    pub intro_seen: bool,
    /// Ids of removed accounts. Never given out again: Claude Code keys a
    /// login to its folder's path, so a new account in an old folder could
    /// find a login that was meant to be gone.
    pub retired: Vec<String>,
}

pub const MAIN_ID: &str = "main";

impl Default for Store {
    fn default() -> Self {
        Store {
            active: MAIN_ID.into(),
            accounts: vec![Account {
                id: MAIN_ID.into(),
                name: "Main login".into(),
                kind: Kind::Main,
                sso: false,
                fallback: true,
            }],
            spaces: vec![],
            statusline_original: None,
            notified: vec![],
            intro_seen: false,
            retired: vec![],
        }
    }
}

impl Store {
    /// The stored accounts; the defaults when the file is missing or broken.
    /// For reading only: changes go through [`Store::update`].
    pub fn load() -> Store {
        Store::try_load().unwrap_or_default()
    }

    /// The stored accounts, or why the file cannot be read. Only a missing
    /// file means "no accounts yet".
    pub fn try_load() -> std::io::Result<Store> {
        match std::fs::read_to_string(paths::accounts_file()) {
            Ok(text) => serde_json::from_str::<Value>(&text)
                .map(|v| Store::from_json(&v))
                .map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("accounts.json is not valid JSON ({e}); not changed"),
                    )
                }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(e),
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
        paths::write_atomic(&paths::accounts_file(), format!("{text}\n").as_bytes())
    }

    /// Changes the stored accounts: under a lock, on a fresh read, so
    /// changes from the list, the status line and the command line never
    /// undo each other. A file that cannot be read is never overwritten.
    pub fn update(change: impl FnOnce(&mut Store)) -> std::io::Result<Store> {
        let lock_path = paths::config_dir().join("accounts.lock");
        std::fs::create_dir_all(paths::config_dir())?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)?;
        lock.lock()?;
        let mut store = Store::try_load()?;
        change(&mut store);
        store.save()?;
        Ok(store)
    }

    pub fn from_json(v: &Value) -> Store {
        let mut store = Store {
            active: v["active"].as_str().unwrap_or(MAIN_ID).to_string(),
            accounts: vec![],
            spaces: vec![],
            statusline_original: v["statusline_original"].as_str().map(str::to_string),
            notified: vec![],
            intro_seen: v["intro_seen"].as_bool().unwrap_or(false),
            retired: v["retired"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str())
                .filter(|x| valid_id(x))
                .map(str::to_string)
                .collect(),
        };
        for a in v["accounts"].as_array().into_iter().flatten() {
            let (Some(id), Some(kind)) =
                (a["id"].as_str(), a["kind"].as_str().and_then(Kind::parse))
            else {
                continue;
            };
            if !valid_id(id) || store.accounts.iter().any(|x| x.id == id) {
                continue;
            }
            store.accounts.push(Account {
                id: id.to_string(),
                name: a["name"].as_str().unwrap_or(id).to_string(),
                kind,
                sso: a["sso"].as_bool().unwrap_or(false),
                fallback: a["fallback"].as_bool().unwrap_or(true),
            });
        }
        // The main login always exists, and only once.
        store
            .accounts
            .retain(|a| a.kind != Kind::Main || a.id == MAIN_ID);
        if !store.accounts.iter().any(|a| a.id == MAIN_ID) {
            store
                .accounts
                .insert(0, Store::default().accounts.remove(0));
        }
        for s in v["spaces"].as_array().into_iter().flatten() {
            if let (Some(w), Some(acc)) = (s["workspace_id"].as_str(), s["account"].as_str()) {
                store.spaces.push(Space {
                    workspace_id: w.to_string(),
                    label: s["label"].as_str().unwrap_or("").to_string(),
                    account: acc.to_string(),
                });
            }
        }
        for n in v["notified"].as_array().into_iter().flatten() {
            if let (Some(id), Some(at)) = (n[0].as_str(), n[1].as_i64()) {
                store.notified.push((id.to_string(), at));
            }
        }
        if store.account(&store.active.clone()).is_none() {
            store.active = MAIN_ID.into();
        }
        // Spaces of removed accounts fall back to the active account.
        let ids: Vec<String> = store.accounts.iter().map(|a| a.id.clone()).collect();
        store.spaces.retain(|s| ids.contains(&s.account));
        store
    }

    pub fn to_json(&self) -> Value {
        let accounts: Vec<Value> = self
            .accounts
            .iter()
            .map(|a| {
                let mut m = Map::new();
                m.insert("id".into(), json!(a.id));
                m.insert("name".into(), json!(a.name));
                m.insert("kind".into(), json!(a.kind.as_str()));
                if a.sso {
                    m.insert("sso".into(), json!(true));
                }
                m.insert("fallback".into(), json!(a.fallback));
                Value::Object(m)
            })
            .collect();
        let spaces: Vec<Value> = self
            .spaces
            .iter()
            .map(
                |s| json!({"workspace_id": s.workspace_id, "label": s.label, "account": s.account}),
            )
            .collect();
        let mut m = Map::new();
        m.insert("active".into(), json!(self.active));
        m.insert("accounts".into(), Value::Array(accounts));
        m.insert("spaces".into(), Value::Array(spaces));
        if let Some(s) = &self.statusline_original {
            m.insert("statusline_original".into(), json!(s));
        }
        if self.intro_seen {
            m.insert("intro_seen".into(), json!(true));
        }
        if !self.retired.is_empty() {
            m.insert("retired".into(), json!(self.retired));
        }
        if !self.notified.is_empty() {
            let n: Vec<Value> = self
                .notified
                .iter()
                .map(|(id, at)| json!([id, at]))
                .collect();
            m.insert("notified".into(), Value::Array(n));
        }
        Value::Object(m)
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// An account by id, or by name (any case), for the command line.
    pub fn find(&self, key: &str) -> Option<&Account> {
        self.account(key).or_else(|| {
            self.accounts
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(key))
        })
    }

    pub fn name_of(&self, id: &str) -> String {
        self.account(id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// The account a space is assigned to. Only by id: two spaces can share
    /// a name. After a Herdr restart, `startup` finds moved ids by name.
    pub fn space_account(&self, workspace_id: &str) -> Option<&str> {
        self.spaces
            .iter()
            .find(|s| s.workspace_id == workspace_id)
            .map(|s| s.account.as_str())
    }

    pub fn assign(&mut self, workspace_id: &str, label: &str, account: Option<&str>) {
        self.spaces.retain(|s| s.workspace_id != workspace_id);
        if let Some(acc) = account {
            self.spaces.push(Space {
                workspace_id: workspace_id.to_string(),
                label: label.to_string(),
                account: acc.to_string(),
            });
        }
    }

    /// A new, unused id made from a name: `Personal Max` gives `personal-max`.
    pub fn new_id(&self, name: &str) -> String {
        let mut base: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        while base.contains("--") {
            base = base.replace("--", "-");
        }
        let base = base.trim_matches('-');
        let base = if base.is_empty() || base == MAIN_ID {
            "account".to_string()
        } else {
            base.chars().take(24).collect::<String>()
        };
        let base = base.trim_matches('-').to_string();
        let mut id = base.clone();
        let mut n = 2;
        while self.account(&id).is_some()
            || self.retired.contains(&id)
            || std::path::Path::new(&paths::homes_dir().join(&id)).exists()
        {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Moves an account one place up (`-1`) or down (`1`) in the list.
    pub fn move_account(&mut self, id: &str, by: i32) -> bool {
        let Some(i) = self.accounts.iter().position(|a| a.id == id) else {
            return false;
        };
        let j = i as i32 + by;
        if j < 0 || j >= self.accounts.len() as i32 {
            return false;
        }
        self.accounts.swap(i, j as usize);
        true
    }
}

/// Ids name folders and keychain entries: letters, digits and dashes only.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Store {
        let mut s = Store::default();
        s.accounts.push(Account {
            id: "personal".into(),
            name: "Personal".into(),
            kind: Kind::Subscription,
            sso: false,
            fallback: true,
        });
        s.accounts.push(Account {
            id: "sandbox".into(),
            name: "Sandbox".into(),
            kind: Kind::ApiKey,
            sso: false,
            fallback: false,
        });
        s.active = "personal".into();
        s.assign("w3", "acme-app", Some("sandbox"));
        s
    }

    #[test]
    fn round_trip() {
        let s = sample();
        let back = Store::from_json(&s.to_json());
        assert_eq!(back, s);
    }

    #[test]
    fn main_always_there_and_bad_entries_dropped() {
        let v = serde_json::json!({
            "active": "gone",
            "accounts": [
                {"id": "x", "kind": "subscription", "name": "X"},
                {"id": "Bad Id", "kind": "subscription"},
                {"id": "y", "kind": "nonsense"},
                {"id": "x", "kind": "api-key"}
            ],
            "spaces": [{"workspace_id": "w1", "account": "removed"}]
        });
        let s = Store::from_json(&v);
        let ids: Vec<&str> = s.accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["main", "x"]);
        assert_eq!(s.active, "main");
        assert!(s.spaces.is_empty());
    }

    #[test]
    fn spaces_by_id_only() {
        let mut s = sample();
        assert_eq!(s.space_account("w3"), Some("sandbox"));
        assert_eq!(
            s.space_account("w9"),
            None,
            "a space with the same name is another space"
        );
        s.assign("w9", "acme-app", Some("personal"));
        assert_eq!(s.space_account("w3"), Some("sandbox"));
        s.assign("w3", "acme-app", None);
        assert_eq!(s.space_account("w3"), None);
        assert_eq!(s.space_account("w9"), Some("personal"));
    }

    #[test]
    fn ids_from_names() {
        let s = sample();
        assert_eq!(s.new_id("Work Max!"), "work-max");
        assert_eq!(s.new_id("Personal"), "personal-2");
        assert_eq!(s.new_id("main"), "account");
        assert_eq!(s.new_id("***"), "account");
        assert!(valid_id(&s.new_id("Ünïcode Näme")));
    }

    #[test]
    fn removed_ids_are_never_given_out_again() {
        let mut s = sample();
        s.retired = vec!["work".into(), "work-2".into()];
        assert_eq!(s.new_id("Work"), "work-3");
        // Kept across a save and a load; bad entries are dropped.
        let mut v = s.to_json();
        v["retired"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("Bad Id"));
        let back = Store::from_json(&v);
        assert_eq!(back.retired, ["work", "work-2"]);
        assert_eq!(back.new_id("Work"), "work-3");
    }

    #[test]
    fn find_by_name_and_move() {
        let mut s = sample();
        assert_eq!(s.find("personal").map(|a| a.id.as_str()), Some("personal"));
        assert_eq!(s.find("SANDBOX").map(|a| a.id.as_str()), Some("sandbox"));
        assert!(s.move_account("sandbox", -1));
        assert_eq!(s.accounts[1].id, "sandbox");
        assert!(!s.move_account("main", -1));
    }
}
