//! Which Claude login each account holds, read from the `.claude.json` that
//! Claude Code writes on sign-in. Two entries can end up with the same login
//! (`/login` in Claude Code changes the main login; a browser may sign in
//! with the account it already has open). Such twins share one set of
//! limits: the freshest reading counts for all of them, so fallback never
//! moves a session from a full account to the same full account.

use crate::mirror;
use crate::paths;
use crate::store::{Account, Kind, Store};
use crate::usage::Usage;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// A claude.ai or Console login: the person's account and the organisation
/// the limits belong to. The same person in two organisations (a personal
/// plan and a company seat) are two logins.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Login {
    account: String,
    organization: String,
}

/// The login in a `.claude.json` text, if it holds one.
pub fn parse(text: &str) -> Option<Login> {
    let v: Value = serde_json::from_str(text).ok()?;
    let o = v.get("oauthAccount")?;
    let field = |k: &str| {
        o.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(Login {
        account: field("accountUuid")?,
        organization: field("organizationUuid")?,
    })
}

fn read(path: &Path) -> Option<Login> {
    parse(&std::fs::read_to_string(path).ok()?)
}

/// The login an account holds. API key accounts have none.
pub fn of(account: &Account) -> Option<Login> {
    match account.kind {
        Kind::ApiKey => None,
        Kind::Main => read(&paths::main_claude_json()),
        Kind::Subscription => read(&mirror::folder(&account.id).join(".claude.json")),
    }
}

/// Each account's login, where it has one.
pub fn all(store: &Store) -> HashMap<String, Login> {
    store
        .accounts
        .iter()
        .filter_map(|a| of(a).map(|l| (a.id.clone(), l)))
        .collect()
}

/// For each account, the other accounts with the same login, in list order.
/// Accounts without a twin are left out.
pub fn twins(store: &Store, logins: &HashMap<String, Login>) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    for a in &store.accounts {
        let Some(mine) = logins.get(&a.id) else {
            continue;
        };
        let others: Vec<String> = store
            .accounts
            .iter()
            .filter(|b| b.id != a.id && logins.get(&b.id) == Some(mine))
            .map(|b| b.id.clone())
            .collect();
        if !others.is_empty() {
            out.insert(a.id.clone(), others);
        }
    }
    out
}

/// When a reading was last taken for an account.
fn read_at(u: &Usage) -> i64 {
    u.updated_at.max(u.probed_at).unwrap_or(i64::MIN)
}

/// Gives every account the limits of the freshest reading among its twins.
/// Costs stay each account's own.
pub fn share_limits(usage: &mut HashMap<String, Usage>, twins: &HashMap<String, Vec<String>>) {
    let before = usage.clone();
    let empty = Usage::default();
    for (id, others) in twins {
        let freshest = std::iter::once(id)
            .chain(others)
            .map(|i| before.get(i).unwrap_or(&empty))
            .max_by_key(|u| read_at(u))
            .unwrap_or(&empty);
        if read_at(freshest) == i64::MIN {
            continue;
        }
        let u = usage.entry(id.clone()).or_default();
        u.five_hour = freshest.five_hour;
        u.seven_day = freshest.seven_day;
        u.spend = freshest.spend.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn login(a: &str, o: &str) -> String {
        json!({"oauthAccount": {"accountUuid": a, "organizationUuid": o, "emailAddress": "alex@example.com"}})
            .to_string()
    }

    fn store() -> Store {
        let mut s = Store::default();
        for id in ["a", "b", "c"] {
            s.accounts.push(Account {
                id: id.into(),
                name: id.to_uppercase(),
                kind: Kind::Subscription,
                sso: false,
                fallback: true,
            });
        }
        s
    }

    #[test]
    fn parses_the_login() {
        assert_eq!(parse(&login("p1", "o1")), parse(&login("p1", "o1")));
        assert_ne!(parse(&login("p1", "o1")), parse(&login("p1", "o2")));
        assert_eq!(parse("{}"), None);
        assert_eq!(parse(r#"{"oauthAccount": {"accountUuid": "p1"}}"#), None);
        assert_eq!(parse("not json"), None);
    }

    #[test]
    fn finds_twins() {
        let s = store();
        let logins: HashMap<String, Login> = [
            ("main".to_string(), parse(&login("p1", "o1")).unwrap()),
            ("a".to_string(), parse(&login("p1", "o1")).unwrap()),
            ("b".to_string(), parse(&login("p1", "o2")).unwrap()),
        ]
        .into();
        let t = twins(&s, &logins);
        assert_eq!(t.get("main"), Some(&vec!["a".to_string()]));
        assert_eq!(t.get("a"), Some(&vec!["main".to_string()]));
        assert!(!t.contains_key("b"), "same person, other organisation");
        assert!(!t.contains_key("c"), "no login");
    }

    #[test]
    fn twins_share_the_freshest_limits() {
        let mut fresh = Usage::default();
        fresh.record(
            &json!({"rate_limits": {"seven_day": {"used_percentage": 100.0, "resets_at": 2_000}}}),
            1_000,
        );
        let mut old = Usage::default();
        old.record(
            &json!({"rate_limits": {"seven_day": {"used_percentage": 20.0, "resets_at": 2_000}}}),
            500,
        );
        let mut usage: HashMap<String, Usage> =
            [("a".to_string(), fresh.clone()), ("main".to_string(), old)].into();
        let t: HashMap<String, Vec<String>> = [
            ("a".to_string(), vec!["main".to_string()]),
            ("main".to_string(), vec!["a".to_string()]),
            ("c".to_string(), vec!["b".to_string()]),
        ]
        .into();
        share_limits(&mut usage, &t);
        assert_eq!(usage["main"].seven_day, fresh.seven_day);
        assert_eq!(usage["a"].seven_day, fresh.seven_day);
        assert!(
            !usage.contains_key("c"),
            "no reading anywhere: nothing to share"
        );
    }
}
