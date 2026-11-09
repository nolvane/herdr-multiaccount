//! Usage readings, per account in `usage/<id>.json`, from two sources, both
//! Claude Code itself: its status line hands the plugin the subscription's
//! usage windows (`rate_limits`) and the session's cost, and the open list
//! runs Claude Code's `/usage` now and then (`probe.rs`). The plugin never
//! asks Anthropic itself.

use crate::{paths, time};
use serde_json::{json, Map, Value};

/// One usage window as Claude Code reported it last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Percent used, 0 to 100 (a spend limit can go above).
    pub used: f64,
    /// When the window starts afresh, seconds since 1970.
    pub resets_at: i64,
}

impl Window {
    /// The share used now: `None` once the window has reset.
    pub fn current(&self, now: i64) -> Option<f64> {
        (self.resets_at > now).then_some(self.used)
    }
}

/// A spend limit behind a Claude apps gateway.
#[derive(Debug, Clone, PartialEq)]
pub struct Spend {
    pub window: Window,
    pub used_usd: Option<f64>,
    pub limit_usd: Option<f64>,
    pub period: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    /// When Claude Code last reported anything for this account.
    pub updated_at: Option<i64>,
    /// When `/usage` was last read for this account.
    pub probed_at: Option<i64>,
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub spend: Option<Spend>,
    /// Cost per session (US dollars, as Claude Code prices it) and the month
    /// the session started in.
    pub sessions: Vec<(String, f64, String)>,
}

/// Why an account counts as at its limit.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub window: &'static str,
    pub used: f64,
    pub resets_at: i64,
}

fn window(v: &Value) -> Option<Window> {
    let used = v.get("used_percentage")?.as_f64()?;
    let resets_at = v.get("resets_at").and_then(|r| {
        r.as_i64()
            .or_else(|| r.as_f64().map(|f| f as i64))
            .or_else(|| r.as_str().and_then(|s| s.parse::<i64>().ok()))
    })?;
    Some(Window { used, resets_at })
}

fn window_json(w: &Window) -> Value {
    json!({"used": w.used, "resets_at": w.resets_at})
}

fn stored_window(v: &Value) -> Option<Window> {
    Some(Window {
        used: v.get("used")?.as_f64()?,
        resets_at: v.get("resets_at")?.as_i64()?,
    })
}

/// Every session of an account reports the window as of its own last answer,
/// so an idle one repeats an old, lower share. Within one window usage only
/// grows: keep the higher share. A later reset time means a new window; an
/// earlier one is an old reading and is ignored.
fn newer(old: Option<Window>, new: Window) -> Window {
    const SAME: i64 = 120;
    match old {
        Some(o) if (o.resets_at - new.resets_at).abs() <= SAME => Window {
            used: o.used.max(new.used),
            resets_at: o.resets_at.max(new.resets_at),
        },
        Some(o) if o.resets_at > new.resets_at => o,
        _ => new,
    }
}

impl Usage {
    pub fn load(id: &str) -> Usage {
        std::fs::read_to_string(paths::usage_dir().join(format!("{id}.json")))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| Usage::from_json(&v))
            .unwrap_or_default()
    }

    pub fn save(&self, id: &str) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
        paths::write_atomic(
            &paths::usage_dir().join(format!("{id}.json")),
            format!("{text}\n").as_bytes(),
        )
    }

    pub fn from_json(v: &Value) -> Usage {
        let mut u = Usage {
            updated_at: v["updated_at"].as_i64(),
            probed_at: v["probed_at"].as_i64(),
            five_hour: stored_window(&v["five_hour"]),
            seven_day: stored_window(&v["seven_day"]),
            spend: stored_window(&v["spend_limit"]).map(|w| Spend {
                window: w,
                used_usd: v["spend_limit"]["used_usd"].as_f64(),
                limit_usd: v["spend_limit"]["limit_usd"].as_f64(),
                period: v["spend_limit"]["period"].as_str().map(str::to_string),
            }),
            sessions: vec![],
        };
        if let Some(m) = v["sessions"].as_object() {
            for (id, s) in m {
                if let (Some(usd), Some(month)) = (s["usd"].as_f64(), s["month"].as_str()) {
                    u.sessions.push((id.clone(), usd, month.to_string()));
                }
            }
        }
        u
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        if let Some(t) = self.updated_at {
            m.insert("updated_at".into(), json!(t));
        }
        if let Some(t) = self.probed_at {
            m.insert("probed_at".into(), json!(t));
        }
        if let Some(w) = &self.five_hour {
            m.insert("five_hour".into(), window_json(w));
        }
        if let Some(w) = &self.seven_day {
            m.insert("seven_day".into(), window_json(w));
        }
        if let Some(s) = &self.spend {
            let mut o = window_json(&s.window);
            if let Some(x) = s.used_usd {
                o["used_usd"] = json!(x);
            }
            if let Some(x) = s.limit_usd {
                o["limit_usd"] = json!(x);
            }
            if let Some(p) = &s.period {
                o["period"] = json!(p);
            }
            m.insert("spend_limit".into(), o);
        }
        let mut sessions = Map::new();
        for (id, usd, month) in &self.sessions {
            sessions.insert(id.clone(), json!({"usd": usd, "month": month}));
        }
        m.insert("sessions".into(), Value::Object(sessions));
        Value::Object(m)
    }

    /// Takes in what Claude Code handed the status line. Windows it did not
    /// send keep their last reading.
    pub fn record(&mut self, input: &Value, now: i64) {
        self.updated_at = Some(now);
        let rl = &input["rate_limits"];
        if let Some(w) = window(&rl["five_hour"]) {
            self.five_hour = Some(newer(self.five_hour, w));
        }
        if let Some(w) = window(&rl["seven_day"]) {
            self.seven_day = Some(newer(self.seven_day, w));
        }
        if let Some(w) = window(&rl["spend_limit"]) {
            let w = newer(self.spend.as_ref().map(|s| s.window), w);
            let old = self.spend.take();
            let s = &rl["spend_limit"];
            self.spend = Some(Spend {
                window: w,
                used_usd: s["used_usd"]
                    .as_f64()
                    .or(old.as_ref().and_then(|o| o.used_usd)),
                limit_usd: s["limit_usd"]
                    .as_f64()
                    .or(old.as_ref().and_then(|o| o.limit_usd)),
                period: s["period"]
                    .as_str()
                    .map(str::to_string)
                    .or(old.and_then(|o| o.period)),
            });
        }
        let session = input["session_id"].as_str().unwrap_or("");
        let cost = input["cost"]["total_cost_usd"].as_f64();
        if let (false, Some(usd)) = (session.is_empty(), cost) {
            match self.sessions.iter_mut().find(|s| s.0 == session) {
                Some(s) => s.1 = usd,
                None => self
                    .sessions
                    .push((session.to_string(), usd, time::month(now))),
            }
        }
        // Keep this month and the one before.
        let this = time::month(now);
        let before = time::previous_month(&this);
        self.sessions.retain(|s| s.2 == this || s.2 == before);
    }

    /// Takes in what `/usage` printed. It is a fresh reading, so it replaces
    /// the share. The printed reset time is rounded, so a stored one within
    /// the hour (from the status line, to the second) stays.
    pub fn record_probe(&mut self, r: &crate::probe::Reading, now: i64) {
        fn take(old: Option<Window>, p: Option<crate::probe::Probed>, now: i64) -> Option<Window> {
            let Some(p) = p else {
                return old;
            };
            let resets_at = match (p.resets_at, old) {
                (Some(t), Some(o)) if (o.resets_at - t).abs() <= 3600 => o.resets_at,
                (Some(t), _) => t,
                // No window runs (nothing used yet): it counts as reset.
                (None, _) if p.used <= 0.0 => now,
                // A share without a readable reset time: keep the last one
                // while it still runs.
                (None, Some(o)) if o.resets_at > now => o.resets_at,
                (None, _) => return old,
            };
            Some(Window {
                used: p.used,
                resets_at,
            })
        }
        self.probed_at = Some(now);
        self.five_hour = take(self.five_hour, r.five_hour, now);
        self.seven_day = take(self.seven_day, r.seven_day, now);
    }

    /// Changes an account's readings under a lock, so the status line of
    /// several sessions and the list never write over each other.
    pub fn update(id: &str, change: impl FnOnce(&mut Usage)) -> std::io::Result<()> {
        let dir = paths::usage_dir();
        std::fs::create_dir_all(&dir)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(format!("{id}.lock")))?;
        lock.lock()?;
        let mut u = Usage::load(id);
        change(&mut u);
        u.save(id)
    }

    /// Cost of the sessions that started this month.
    pub fn month_usd(&self, now: i64) -> f64 {
        let this = time::month(now);
        self.sessions
            .iter()
            .filter(|s| s.2 == this)
            .fold(0.0, |sum, s| sum + s.1)
    }

    /// Whether any window still running is used at least `threshold` percent.
    pub fn at_limit(&self, threshold: f64, now: i64) -> Option<Limit> {
        let mut candidates: Vec<Limit> = Vec::new();
        for (name, w) in [("5-hour", self.five_hour), ("7-day", self.seven_day)] {
            if let Some(w) = w {
                if let Some(used) = w.current(now) {
                    if used >= threshold {
                        candidates.push(Limit {
                            window: name,
                            used,
                            resets_at: w.resets_at,
                        });
                    }
                }
            }
        }
        if let Some(s) = &self.spend {
            if let Some(used) = s.window.current(now) {
                if used >= threshold {
                    candidates.push(Limit {
                        window: "spend",
                        used,
                        resets_at: s.window.resets_at,
                    });
                }
            }
        }
        // The one that stays full longest decides.
        candidates.into_iter().max_by_key(|l| l.resets_at)
    }

    /// Whether Claude Code ever reported a usage window for this account.
    pub fn has_windows(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some() || self.spend.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_225_600;

    fn input(five: f64, week: f64) -> Value {
        json!({
            "session_id": "s1",
            "cost": {"total_cost_usd": 1.25},
            "rate_limits": {
                "five_hour": {"used_percentage": five, "resets_at": NOW + 3600},
                "seven_day": {"used_percentage": week, "resets_at": NOW + 86_400}
            }
        })
    }

    #[test]
    fn probe_readings() {
        use crate::probe::{Probed, Reading};
        let mut u = Usage::default();
        u.record(&input(23.5, 41.2), NOW);
        // A fresh share replaces the old one; the exact reset time stays
        // when the printed one is within the hour.
        u.record_probe(
            &Reading {
                five_hour: Some(Probed {
                    used: 30.0,
                    resets_at: Some(NOW + 3600 + 600),
                }),
                seven_day: Some(Probed {
                    used: 10.0,
                    resets_at: Some(NOW + 3 * 86_400),
                }),
            },
            NOW,
        );
        assert_eq!(
            u.five_hour,
            Some(Window {
                used: 30.0,
                resets_at: NOW + 3600
            })
        );
        assert_eq!(
            u.seven_day,
            Some(Window {
                used: 10.0,
                resets_at: NOW + 3 * 86_400
            })
        );
        assert_eq!(u.probed_at, Some(NOW));
        // Nothing used and no window: counts as reset.
        u.record_probe(
            &Reading {
                five_hour: Some(Probed {
                    used: 0.0,
                    resets_at: None,
                }),
                seven_day: None,
            },
            NOW + 10,
        );
        assert_eq!(u.five_hour.unwrap().current(NOW + 10), None);
        assert_eq!(u.seven_day.unwrap().used, 10.0);
        let back = Usage::from_json(&u.to_json());
        assert_eq!(back.probed_at, Some(NOW + 10));
    }

    #[test]
    fn records_windows_and_cost() {
        let mut u = Usage::default();
        u.record(&input(23.5, 41.2), NOW);
        assert_eq!(u.five_hour.unwrap().used, 23.5);
        assert_eq!(u.seven_day.unwrap().resets_at, NOW + 86_400);
        assert_eq!(u.month_usd(NOW), 1.25);
        // The same session again replaces its cost; a new one adds.
        let mut next = input(30.0, 42.0);
        next["cost"]["total_cost_usd"] = json!(2.0);
        u.record(&next, NOW + 60);
        next["session_id"] = json!("s2");
        next["cost"]["total_cost_usd"] = json!(0.5);
        u.record(&next, NOW + 120);
        assert_eq!(u.month_usd(NOW), 2.5);
        let back = Usage::from_json(&u.to_json());
        assert_eq!(back, u);
    }

    #[test]
    fn idle_sessions_do_not_lower_the_reading() {
        let mut u = Usage::default();
        u.record(&input(97.0, 60.0), NOW);
        // An idle session repeats its old, lower reading of the same window.
        let mut old = input(40.0, 55.0);
        old["rate_limits"]["five_hour"]["resets_at"] = json!(NOW + 3600 - 30);
        u.record(&old, NOW + 30);
        assert_eq!(u.five_hour.unwrap().used, 97.0);
        assert_eq!(u.seven_day.unwrap().used, 60.0);
        // A reading from an earlier window is ignored.
        let mut stale = input(10.0, 10.0);
        stale["rate_limits"]["five_hour"]["resets_at"] = json!(NOW - 9000);
        u.record(&stale, NOW + 40);
        assert_eq!(u.five_hour.unwrap().used, 97.0);
        // A new window starts low again.
        let mut fresh = input(3.0, 61.0);
        fresh["rate_limits"]["five_hour"]["resets_at"] = json!(NOW + 3600 + 18_000);
        u.record(&fresh, NOW + 3700);
        assert_eq!(u.five_hour.unwrap().used, 3.0);
    }

    #[test]
    fn missing_windows_keep_last_reading() {
        let mut u = Usage::default();
        u.record(&input(50.0, 60.0), NOW);
        u.record(&json!({"session_id": "s1"}), NOW + 10);
        assert_eq!(u.five_hour.unwrap().used, 50.0);
        assert!(u.has_windows());
    }

    #[test]
    fn limit_and_reset() {
        let mut u = Usage::default();
        u.record(&input(96.0, 99.0), NOW);
        let l = u.at_limit(95.0, NOW).unwrap();
        assert_eq!(l.window, "7-day", "the window that stays full longest");
        assert!(u.at_limit(99.5, NOW).is_none());
        // After both resets the account is free again.
        assert!(u.at_limit(95.0, NOW + 86_401).is_none());
        assert_eq!(u.five_hour.unwrap().current(NOW + 3601), None);
    }

    #[test]
    fn string_reset_times_and_spend() {
        let mut u = Usage::default();
        u.record(
            &json!({"rate_limits": {"spend_limit": {"used_percentage": 62.8, "resets_at": "1794312000", "used_usd": 314.12, "limit_usd": 500, "period": "monthly"}}}),
            NOW,
        );
        let s = u.spend.clone().unwrap();
        assert_eq!(s.window.resets_at, 1_794_312_000);
        assert_eq!(s.limit_usd, Some(500.0));
        // A later reading without the dollar fields keeps them.
        u.record(
            &json!({"rate_limits": {"spend_limit": {"used_percentage": 63.0, "resets_at": 1_794_312_000}}}),
            NOW + 5,
        );
        assert_eq!(u.spend.unwrap().used_usd, Some(314.12));
    }

    #[test]
    fn old_months_are_dropped() {
        let mut u = Usage {
            sessions: vec![("old".into(), 9.0, "2026-08".into())],
            ..Default::default()
        };
        u.record(
            &json!({"session_id": "s", "cost": {"total_cost_usd": 1.0}}),
            NOW,
        );
        assert_eq!(u.sessions.len(), 1);
        assert_eq!(u.sessions[0].0, "s");
    }
}
