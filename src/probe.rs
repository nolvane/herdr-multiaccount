//! Limit readings without a running session. Claude Code's own `/usage`
//! command, run headless in the account's folder, prints the subscription's
//! windows ("Current session: 12% used · resets 7pm (Europe/Zurich)"). The
//! plugin reads that text: Claude Code does the asking, with its own login,
//! as it does when you type `/usage`; the plugin never sees a token.
//!
//! The run uses no model and costs nothing. `--safe-mode` skips hooks,
//! plugins and MCP servers, and `--no-session-persistence` keeps it out of
//! the session history.

use crate::launch;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// One window as `/usage` printed it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probed {
    pub used: f64,
    /// When it resets, seconds since 1970; `None` when no window runs (a
    /// session window before the first message) or the time was not read.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Reading {
    pub five_hour: Option<Probed>,
    pub seven_day: Option<Probed>,
}

/// A reset time as printed, before it becomes a point in time.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetText {
    /// Month (1 to 12) and day, when the text names a date.
    pub date: Option<(u32, u32)>,
    pub hour: u32,
    pub minute: u32,
    /// The time zone in brackets, such as `Europe/Zurich`.
    pub zone: Option<String>,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// `2am`, `2:30pm`, `14:00`.
fn clock(token: &str) -> Option<(u32, u32)> {
    let t = token.trim().to_lowercase();
    let (body, pm) = if let Some(b) = t.strip_suffix("am") {
        (b.trim(), Some(false))
    } else if let Some(b) = t.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else {
        (t.as_str(), None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    if m > 59 {
        return None;
    }
    let h = match pm {
        Some(pm) if (1..=12).contains(&h) => (h % 12) + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if h < 24 => h,
        None => return None,
    };
    Some((h, m))
}

/// Reads `Nov 10 at 2am (Europe/Zurich)`, `7:30pm (Europe/Zurich)` or
/// `Nov 10, 2am`.
pub fn reset_text(text: &str) -> Option<ResetText> {
    let text = text.trim();
    let (rest, zone) = match text.rfind('(') {
        Some(i) if text.ends_with(')') => (
            text[..i].trim(),
            Some(text[i + 1..text.len() - 1].trim().to_string()),
        ),
        _ => (text, None),
    };
    let words: Vec<&str> = rest
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty() && *w != "at")
        .collect();
    let (date, time) = match words.as_slice() {
        [t] => (None, *t),
        [mon, day, t] => {
            let m = MONTHS
                .iter()
                .position(|x| mon.to_lowercase().starts_with(x))? as u32
                + 1;
            let d = day.parse::<u32>().ok().filter(|d| (1..=31).contains(d))?;
            (Some((m, d)), *t)
        }
        _ => return None,
    };
    let (hour, minute) = clock(time)?;
    Some(ResetText {
        date,
        hour,
        minute,
        zone: zone.filter(|z| {
            // A zone name like Europe/Zurich, never a file path: TZ with a
            // leading '/' or '..' makes date open that file.
            !z.is_empty()
                && !z.starts_with('/')
                && !z.contains("..")
                && z.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
        }),
    })
}

/// `Current session: 12% used · resets 7pm (Europe/Zurich)` → 12 and the
/// reset text.
fn window_line(line: &str) -> Option<(f64, Option<&str>)> {
    let (_, rest) = line.split_once(':')?;
    let pct = rest.find('%')?;
    let used = rest[..pct].trim().parse::<f64>().ok()?;
    let reset = rest
        .find("resets ")
        .map(|i| rest[i + "resets ".len()..].trim());
    Some((used, reset))
}

/// The windows in `/usage` text. `None` when the text is not a usage report
/// at all; a report without windows (an organisation without limits) gives
/// an empty reading.
pub fn parse(text: &str, resolve: &dyn Fn(&ResetText) -> Option<i64>) -> Option<Reading> {
    let mut reading = Reading::default();
    let mut understood = false;
    for line in text.lines().map(str::trim) {
        let lower = line.to_lowercase();
        let slot = if lower.starts_with("current session") {
            &mut reading.five_hour
        } else if lower.starts_with("current week (all models)")
            || lower.starts_with("current week:")
        {
            &mut reading.seven_day
        } else {
            if lower.contains("usage") || lower.contains("subscription") {
                understood = true;
            }
            continue;
        };
        if let Some((used, reset)) = window_line(line) {
            understood = true;
            *slot = Some(Probed {
                used,
                resets_at: reset.and_then(reset_text).and_then(|r| resolve(&r)),
            });
        }
    }
    understood.then_some(reading)
}

/// Runs a short command and takes its output, giving up after two seconds:
/// a `date` that hangs must not stop the limit readings.
fn output_within(cmd: &mut Command) -> Option<std::process::Output> {
    let mut child = cmd.stdout(Stdio::piped()).spawn().ok()?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Seconds since 1970 for a wall clock time in a zone, through the system's
/// `date` tool, which knows the zone's daylight saving rules.
fn epoch(zone: Option<&str>, y: i64, m: u32, d: u32, h: u32, min: u32) -> Option<i64> {
    let stamp = format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:00");
    let mut cmd = Command::new("date");
    if cfg!(target_os = "macos") {
        cmd.args(["-j", "-f", "%Y-%m-%d %H:%M:%S", &stamp, "+%s"]);
    } else {
        cmd.args(["-d", &stamp, "+%s"]);
    }
    if let Some(z) = zone {
        cmd.env("TZ", z);
    }
    let out = output_within(cmd.stdin(Stdio::null()).stderr(Stdio::null()))?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Today's date in a zone.
fn today(zone: Option<&str>) -> Option<(i64, u32, u32)> {
    let mut cmd = Command::new("date");
    cmd.arg("+%Y %m %d");
    if let Some(z) = zone {
        cmd.env("TZ", z);
    }
    let out = output_within(cmd.stdin(Stdio::null()).stderr(Stdio::null()))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut parts = text.split_whitespace();
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

/// The next point in time the text can mean, from `now` on.
pub fn resolve(r: &ResetText, now: i64) -> Option<i64> {
    let zone = r.zone.as_deref();
    let (y, m, d) = today(zone)?;
    match r.date {
        Some((month, day)) => {
            let t = epoch(zone, y, month, day, r.hour, r.minute)?;
            // "Jan 2" read on 30 December is next year.
            if t < now - 86_400 {
                epoch(zone, y + 1, month, day, r.hour, r.minute)
            } else {
                Some(t)
            }
        }
        None => {
            let t = epoch(zone, y, m, d, r.hour, r.minute)?;
            Some(if t <= now { t + 86_400 } else { t })
        }
    }
}

/// Runs `/usage` for an account: its folder, or the main login for `None`.
pub fn run(home: Option<&Path>, cwd: &Path) -> Result<Reading, String> {
    let claude = launch::find_claude().ok_or("claude was not found")?;
    let mut cmd = Command::new(claude);
    launch::clean_env(&mut cmd);
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
    std::fs::create_dir_all(cwd).map_err(|e| e.to_string())?;
    cmd.args([
        "-p",
        "/usage",
        "--safe-mode",
        "--no-session-persistence",
        "--output-format",
        "json",
    ])
    .current_dir(cwd)
    .stdin(Stdio::null())
    .stderr(Stdio::null());
    let out = launch::run_within(&mut cmd, Duration::from_secs(45))
        .map_err(|e| e.to_string())?
        .ok_or("Claude Code took too long")?;
    let v: serde_json::Value =
        serde_json::from_str(out.trim()).map_err(|_| "unexpected answer from Claude Code")?;
    if v["is_error"].as_bool() == Some(true) {
        return Err("Claude Code could not read the usage".into());
    }
    let text = v["result"].as_str().ok_or("no usage in the answer")?;
    let now = crate::time::now();
    parse(text, &|r| resolve(r, now)).ok_or_else(|| "the usage report was not understood".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_texts() {
        assert_eq!(
            reset_text("Nov 10 at 2am (Europe/Zurich)"),
            Some(ResetText {
                date: Some((11, 10)),
                hour: 2,
                minute: 0,
                zone: Some("Europe/Zurich".into())
            })
        );
        let r = reset_text("7:30pm (America/New_York)").unwrap();
        assert_eq!((r.date, r.hour, r.minute), (None, 19, 30));
        assert_eq!(reset_text("Dec 1, 12am").unwrap().hour, 0);
        assert_eq!(reset_text("12pm").unwrap().hour, 12);
        assert_eq!(reset_text("14:05").unwrap().minute, 5);
        assert_eq!(reset_text("soon"), None);
        assert_eq!(reset_text("Foo 10 at 2am"), None);
        // A zone that is not a plain name is dropped, never passed on.
        assert_eq!(reset_text("2am (x; rm y)").unwrap().zone, None);
        // A zone name, never a file path.
        assert_eq!(reset_text("2am (/dev/tty)").unwrap().zone, None);
        assert_eq!(reset_text("2am (../../x)").unwrap().zone, None);
        assert_eq!(
            reset_text("2am (Europe/Zurich)").unwrap().zone.as_deref(),
            Some("Europe/Zurich")
        );
    }

    #[test]
    fn usage_report() {
        let text = "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 0% used\nCurrent week (all models): 100% used · resets Nov 10 at 2am (Europe/Zurich)\nCurrent week (Fable): 3% used · resets Nov 10 at 2am (Europe/Zurich)\n\nWhat's contributing to your usage?\nLast 24h · 7605 requests · 69 sessions\n  88%  Give subagents a cheaper model\n";
        let r = parse(text, &|r| Some(r.hour as i64 * 1000)).unwrap();
        assert_eq!(
            r.five_hour,
            Some(Probed {
                used: 0.0,
                resets_at: None
            })
        );
        assert_eq!(
            r.seven_day,
            Some(Probed {
                used: 100.0,
                resets_at: Some(2000)
            })
        );

        // An organisation without limits: understood, no windows.
        let r = parse(
            "You are currently using your subscription to power your Claude Code usage\n\nWhat's contributing to your usage?\n",
            &|_| None,
        )
        .unwrap();
        assert_eq!(r, Reading::default());

        assert_eq!(parse("Not logged in · Please run /login", &|_| None), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn wall_clock_to_seconds() {
        // 2026-11-10 02:00 in Zurich is 01:00 UTC.
        assert_eq!(
            epoch(Some("Europe/Zurich"), 2026, 11, 10, 2, 0),
            Some(1_794_272_400)
        );
    }
}
