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

/// The names of the other accounts that hold the same login as `account`,
/// in list order.
pub fn same_as(store: &Store, account: &Account) -> Vec<String> {
    let Some(mine) = of(account) else {
        return Vec::new();
    };
    store
        .accounts
        .iter()
        .filter(|a| a.id != account.id && of(a).as_ref() == Some(&mine))
        .map(|a| a.name.clone())
        .collect()
}

/// The warning after a sign-in that ended up with another account's login:
/// the browser often signs in with whatever account it already has open.
/// Its key `l` is marked for the list (`theme::keys`); a terminal gets it
/// through `theme::plain`.
pub fn same_login_note(names: &[String]) -> String {
    crate::theme::named(
        &format!("That is the same login as {}", names.join(", ")),
        ". If you meant another account, sign out in the browser (or use a private window) and press `l` to sign in again.",
    )
}

/// The start line for an account whose folder holds the same login as the
/// accounts `names`, such as after a `/login` inside one of its sessions.
pub fn shared_login_note(name: &str, names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| format!(
        "{name} holds the same login as {}, so this session runs on that login (a /login inside a session, or the browser at sign-in, can do that). If {name} should have its own, open the accounts list (Herdr: the Multiaccount action) and press l on {name} to sign it in again.",
        names.join(", ")
    ))
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

/// How fresh the limits an account shows are, for picking among twins;
/// `None` when it has none to share. A known limits time
/// (`Usage::limits_read_at`: the newest `/usage` check or status line answer
/// that brought limits) beats a reading without one. Files written before
/// that time was stored have only `updated_at`, which every status line call
/// moves forward, also one that brings no limits; among such readings alone
/// it still decides.
fn read_at(u: &Usage) -> Option<(bool, i64)> {
    if !u.has_windows() {
        return None;
    }
    match u.limits_read_at() {
        Some(t) => Some((true, t)),
        None => u.updated_at.map(|t| (false, t)),
    }
}

/// Gives every account the limits of the freshest reading among its twins;
/// a window that reading lacks (`/usage` shows no spend limit) comes from
/// the next freshest that has it. Costs stay each account's own.
pub fn share_limits(usage: &mut HashMap<String, Usage>, twins: &HashMap<String, Vec<String>>) {
    let before = usage.clone();
    for (id, others) in twins {
        let mut readings: Vec<(Option<(bool, i64)>, &Usage)> = std::iter::once(id)
            .chain(others)
            .filter_map(|i| before.get(i))
            .map(|u| (read_at(u), u))
            .filter(|(at, _)| at.is_some())
            .collect();
        if readings.is_empty() {
            continue;
        }
        readings.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let u = usage.entry(id.clone()).or_default();
        u.five_hour = readings.iter().find_map(|(_, r)| r.five_hour);
        u.seven_day = readings.iter().find_map(|(_, r)| r.seven_day);
        u.spend = readings.iter().find_map(|(_, r)| r.spend.clone());
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
    fn a_login_shared_with_another_entry_is_named_at_start() {
        assert_eq!(shared_login_note("Work", &[]), None);
        let line = shared_login_note("Work", &["Main login".to_string()]).unwrap();
        assert!(
            line.contains("Work holds the same login as Main login"),
            "{line}"
        );
        assert!(line.contains("/login"), "{line}");
        assert!(line.contains("press l on Work"), "{line}");
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

    const T: i64 = 1_794_225_600;

    /// A status line call of session `s1` with limits and a session cost.
    fn answer(five: f64, usd: f64) -> serde_json::Value {
        json!({
            "session_id": "s1",
            "cost": {"total_cost_usd": usd},
            "rate_limits": {
                "five_hour": {"used_percentage": five, "resets_at": T + 3600},
                "seven_day": {"used_percentage": five, "resets_at": T + 86_400}
            }
        })
    }

    /// A `/usage` check of B at `T` that reads 90 percent.
    fn checked() -> Usage {
        use crate::probe::{Probed, Reading};
        let mut b = Usage::default();
        let p = Probed {
            used: 90.0,
            resets_at: Some(T + 3600),
        };
        b.record_probe(
            &Reading {
                five_hour: Some(p),
                seven_day: Some(p),
            },
            T,
        );
        b
    }

    fn shared(a: Usage, b: Usage) -> HashMap<String, Usage> {
        let mut usage: HashMap<String, Usage> = [("a".to_string(), a), ("b".to_string(), b)].into();
        let t: HashMap<String, Vec<String>> = [
            ("a".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["a".to_string()]),
        ]
        .into();
        share_limits(&mut usage, &t);
        usage
    }

    #[test]
    fn a_call_without_limits_does_not_hide_a_twins_check() {
        // A's session answered before B's check, then made a call without
        // limits after it (a new session before its first answer).
        let mut a = Usage::default();
        a.record(&answer(10.0, 1.0), T - 100);
        a.record(&json!({"session_id": "s2"}), T + 10);
        let usage = shared(a, checked());
        assert_eq!(usage["a"].five_hour.unwrap().used, 90.0);
        assert_eq!(usage["b"].five_hour.unwrap().used, 90.0);
        // A brand new account whose first call has no limits yet.
        let mut fresh = Usage::default();
        fresh.record(&json!({"session_id": "s1"}), T + 10);
        let usage = shared(fresh, checked());
        assert_eq!(usage["a"].five_hour.unwrap().used, 90.0);
        assert_eq!(usage["b"].five_hour.unwrap().used, 90.0);
    }

    #[test]
    fn an_idle_refresh_does_not_hide_a_twins_check() {
        let mut a = Usage::default();
        a.record(&answer(10.0, 1.0), T - 100);
        // Idle: the same answer again, same cost.
        a.record(&answer(10.0, 1.0), T + 30);
        let usage = shared(a, checked());
        assert_eq!(usage["a"].five_hour.unwrap().used, 90.0);
        assert_eq!(usage["b"].seven_day.unwrap().used, 90.0);
    }

    #[test]
    fn a_newer_answer_beats_a_twins_check() {
        let mut a = Usage::default();
        a.record(&answer(10.0, 1.0), T - 100);
        a.record(&answer(95.0, 1.5), T + 30);
        let usage = shared(a, checked());
        assert_eq!(usage["a"].five_hour.unwrap().used, 95.0);
        assert_eq!(usage["b"].five_hour.unwrap().used, 95.0);
    }

    #[test]
    fn a_window_the_freshest_reading_lacks_comes_from_the_next() {
        use crate::probe::Reading;
        // A's answer brought a spend limit; B's newer check reads only the
        // 5-hour and 7-day windows, which `/usage` shows.
        let mut a = Usage::default();
        a.record(
            &json!({"session_id": "s1", "cost": {"total_cost_usd": 1.0},
                "rate_limits": {"spend_limit": {"used_percentage": 100.0, "resets_at": T + 3600}}}),
            T - 100,
        );
        let usage = shared(a.clone(), checked());
        assert_eq!(usage["a"].five_hour.unwrap().used, 90.0);
        assert_eq!(
            usage["b"].spend, a.spend,
            "B's check has no spend: A's stays"
        );
        assert_eq!(usage["a"].spend, a.spend);
        // A check that read no window at all shares nothing.
        let mut a = Usage::default();
        a.record(&answer(95.0, 1.0), T - 100);
        let mut empty = Usage::default();
        empty.record_probe(
            &Reading {
                five_hour: None,
                seven_day: None,
            },
            T,
        );
        let usage = shared(a, empty);
        assert_eq!(usage["a"].five_hour.unwrap().used, 95.0);
        assert_eq!(usage["b"].five_hour.unwrap().used, 95.0);
    }

    #[test]
    fn a_reading_without_a_limits_time_loses_to_one_with_it() {
        // Written before limits times were stored: windows and `updated_at`
        // only, which a later call without limits moved past B's check.
        let mut old = Usage::default();
        old.record(
            &json!({"rate_limits": {"five_hour": {"used_percentage": 10.0, "resets_at": T + 3600}}}),
            T + 10,
        );
        assert_eq!(old.limits_read_at(), None);
        let usage = shared(old, checked());
        assert_eq!(usage["a"].five_hour.unwrap().used, 90.0);
        assert_eq!(usage["b"].five_hour.unwrap().used, 90.0);
    }
}
