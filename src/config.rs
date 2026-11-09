//! Settings, read from `config.toml` in the plugin's config directory. Only
//! flat `key = value` lines are understood; anything else is ignored, so a typo
//! never stops the plugin.

use std::path::Path;

/// Where the list opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Overlay,
    Split,
    Tab,
}

impl Placement {
    pub fn as_arg(self) -> &'static str {
        match self {
            Placement::Overlay => "overlay",
            Placement::Split => "split",
            Placement::Tab => "tab",
        }
    }
}

/// Which side of the focused pane a split list goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down,
    Right,
}

impl Direction {
    pub fn as_arg(self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Right => "right",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub placement: Placement,
    pub split_direction: Direction,
    /// Nerd Font icons; plain Unicode symbols when false.
    pub nerd_font: bool,
    /// Move new sessions to the next account when one is at its limit.
    pub fallback: bool,
    /// Used share of a window (percent) from which an account counts as at
    /// its limit.
    pub threshold: f64,
    /// Show a Herdr notification when fallback moves new sessions.
    pub notify: bool,
    /// A dialog when the account a session was moved away from has room
    /// again.
    pub remind: bool,
    /// Seconds between limit checks (Claude Code's `/usage`) while the list
    /// is open; 0 turns them off.
    pub check_limits: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            placement: Placement::Overlay,
            split_direction: Direction::Down,
            nerd_font: true,
            fallback: true,
            threshold: 95.0,
            notify: true,
            check_limits: 60,
            remind: true,
        }
    }
}

impl Config {
    pub fn load(dir: &Path) -> Config {
        match std::fs::read_to_string(dir.join("config.toml")) {
            Ok(text) => Config::parse(&text),
            Err(_) => Config::default(),
        }
    }

    pub fn parse(text: &str) -> Config {
        let mut config = Config::default();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').trim();
            let flag = match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            match key.trim() {
                "placement" => match value {
                    "overlay" => config.placement = Placement::Overlay,
                    "split" => config.placement = Placement::Split,
                    "tab" => config.placement = Placement::Tab,
                    _ => {}
                },
                "split_direction" => match value {
                    "down" => config.split_direction = Direction::Down,
                    "right" => config.split_direction = Direction::Right,
                    _ => {}
                },
                "icons" => match value {
                    "nerd" => config.nerd_font = true,
                    "plain" => config.nerd_font = false,
                    _ => {}
                },
                "fallback" => {
                    if let Some(f) = flag {
                        config.fallback = f;
                    }
                }
                "remind" => {
                    if let Some(f) = flag {
                        config.remind = f;
                    }
                }
                "notify" => {
                    if let Some(f) = flag {
                        config.notify = f;
                    }
                }
                "check_limits" => {
                    if let Ok(n) = value.trim_end_matches('s').parse::<u64>() {
                        // Never more often than every 30 seconds.
                        config.check_limits = if n == 0 { 0 } else { n.max(30) };
                    }
                }
                "threshold" => {
                    if let Ok(n) = value.trim_end_matches('%').parse::<f64>() {
                        if (50.0..=100.0).contains(&n) {
                            config.threshold = n;
                        }
                    }
                }
                _ => {}
            }
        }
        config
    }
}

/// Sets `key = value` in `config.toml`, keeping every other line.
pub fn save_value(dir: &Path, key: &str, value: &str) -> std::io::Result<()> {
    let file = dir.join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    crate::paths::write_atomic(&file, with_value(&text, key, value).as_bytes())
}

fn with_value(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let k = l
                .split('#')
                .next()
                .unwrap_or("")
                .split('=')
                .next()
                .unwrap_or("");
            if l.contains('=') && k.trim() == key {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(line);
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::parse("");
        assert_eq!(c.placement, Placement::Overlay);
        assert!(c.fallback);
        assert_eq!(c.threshold, 95.0);
        assert!(c.notify);
    }

    #[test]
    fn values_and_bad_lines() {
        let c = Config::parse(
            "placement = split\nicons = \"plain\"\nfallback = false # off\nthreshold = 80%\nnotify = maybe\n[x]\n",
        );
        assert_eq!(c.placement, Placement::Split);
        assert!(!c.nerd_font);
        assert!(!c.fallback);
        assert_eq!(c.threshold, 80.0);
        assert!(c.notify, "an unknown value keeps the default");
        assert_eq!(Config::parse("threshold = 10").threshold, 95.0);
        assert_eq!(c.check_limits, 60);
        assert_eq!(Config::parse("check_limits = 0").check_limits, 0);
        assert_eq!(Config::parse("check_limits = 5").check_limits, 30);
        assert_eq!(Config::parse("check_limits = 120s").check_limits, 120);
    }

    #[test]
    fn value_is_written() {
        assert_eq!(with_value("", "fallback", "false"), "fallback = false\n");
        assert_eq!(
            with_value(
                "icons = \"nerd\"\nfallback = true # x\n",
                "fallback",
                "false"
            ),
            "icons = \"nerd\"\nfallback = false\n"
        );
    }
}
