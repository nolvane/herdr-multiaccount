//! Clock helpers: now, the month of a time, and short durations ("2h 10m").

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Year, month and day for days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `2026-11` for seconds since 1970, in UTC.
pub fn month(secs: i64) -> String {
    let (y, m, _) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}")
}

/// The month before `2026-11`: `2026-10`.
pub fn previous_month(month: &str) -> String {
    let (y, m) = month.split_once('-').unwrap_or(("1970", "01"));
    let (y, m) = (
        y.parse::<i64>().unwrap_or(1970),
        m.parse::<i64>().unwrap_or(1),
    );
    if m <= 1 {
        format!("{:04}-12", y - 1)
    } else {
        format!("{y:04}-{:02}", m - 1)
    }
}

/// A duration as one or two units: `45s`, `12m`, `2h 10m`, `3d 4h`.
pub fn span(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => {
            let m = (s % 3600) / 60;
            if m == 0 {
                format!("{}h", s / 3600)
            } else {
                format!("{}h {m}m", s / 3600)
            }
        }
        _ => {
            let h = (s % 86_400) / 3600;
            if h == 0 {
                format!("{}d", s / 86_400)
            } else {
                format!("{}d {h}h", s / 86_400)
            }
        }
    }
}

/// A duration as one unit, for narrow columns: `45s`, `12m`, `2h`, `3d`.
pub fn short(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        assert_eq!(month(0), "1970-01");
        // 2026-11-09T12:00:00Z
        assert_eq!(month(1_794_225_600), "2026-11");
        assert_eq!(previous_month("2026-11"), "2026-10");
        assert_eq!(previous_month("2027-01"), "2026-12");
    }

    #[test]
    fn spans() {
        assert_eq!(span(-5), "0s");
        assert_eq!(span(59), "59s");
        assert_eq!(span(600), "10m");
        assert_eq!(span(7800), "2h 10m");
        assert_eq!(span(7200), "2h");
        assert_eq!(span(3 * 86_400 + 4 * 3600 + 5), "3d 4h");
        assert_eq!(short(3 * 86_400 + 4 * 3600), "3d");
    }
}
