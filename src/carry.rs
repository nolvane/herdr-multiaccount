//! Which of Claude Code's start options a moved session gets again
//! (`relocate.rs`). The launcher hands every `claude` argument on; this reads
//! them the way Claude Code's command line parser (commander) does, keeps the
//! options on a fixed list and drops the rest: the prompt, options that pick
//! or end a conversation, and options that could replace the account's login.
//!
//! Kept options are written back as `--name=value`, one per value, so a value
//! can never be read as an option, and folders are made absolute, so they
//! point to the same place wherever the session starts again.

use std::collections::VecDeque;
use std::path::Path;

/// What an option takes after it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Takes {
    Nothing,
    /// One value: the next word, even one starting with `-`.
    One,
    /// The next word unless it starts with `-`.
    Maybe,
    /// The next word, then every word up to the next one starting with `-`
    /// (`--add-dir a b`); `--add-dir=a` takes only `a`.
    List,
}

/// Whether an option goes along on a move.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Carry {
    Keep,
    /// A folder or file: made absolute.
    Path,
    /// JSON or a file: a file is made absolute.
    JsonOrPath,
    Never,
}

use Carry::{JsonOrPath, Keep, Never, Path as KeepPath};
use Takes::{List, Maybe, Nothing, One};

/// Every option of `claude --help` (Claude Code 2.1.311), without `--`.
const OPTIONS: &[(&str, Takes, Carry)] = &[
    // Carried: how the session works.
    ("model", One, Keep),
    ("permission-mode", One, Keep),
    ("dangerously-skip-permissions", Nothing, Keep),
    ("allow-dangerously-skip-permissions", Nothing, Keep),
    ("add-dir", List, KeepPath),
    ("append-system-prompt", One, Keep),
    ("system-prompt", One, Keep),
    ("mcp-config", List, JsonOrPath),
    ("strict-mcp-config", Nothing, Keep),
    ("agent", One, Keep),
    ("agents", One, JsonOrPath),
    ("allowedTools", List, Keep),
    ("allowed-tools", List, Keep),
    ("disallowedTools", List, Keep),
    ("disallowed-tools", List, Keep),
    ("tools", List, Keep),
    ("effort", One, Keep),
    ("fallback-model", One, Keep),
    ("plugin-dir", One, KeepPath),
    ("chrome", Nothing, Keep),
    ("no-chrome", Nothing, Keep),
    ("ide", Nothing, Keep),
    ("verbose", Nothing, Keep),
    ("name", One, Keep),
    // Carried because dropping them would let the session do more.
    ("restricted", Nothing, Keep),
    ("safe-mode", Nothing, Keep),
    ("disable-slash-commands", Nothing, Keep),
    // Never: they pick, fork or end a conversation, start it elsewhere, or
    // could replace the login (`--settings` can set `apiKeyHelper`).
    ("continue", Nothing, Never),
    ("resume", Maybe, Never),
    ("session-id", One, Never),
    ("fork-session", Nothing, Never),
    ("print", Nothing, Never),
    ("from-pr", Maybe, Never),
    ("teleport", Maybe, Never),
    ("cloud", Maybe, Never),
    ("desktop", Nothing, Never),
    ("bg", Nothing, Never),
    ("background", Nothing, Never),
    ("worktree", Maybe, Never),
    ("tmux", Maybe, Never),
    ("remote-control", Maybe, Never),
    ("remote-control-session-name-prefix", One, Never),
    ("settings", One, Never),
    ("setting-sources", One, Never),
    ("bare", Nothing, Never),
    ("environment", One, Never),
    // Never: only for `--print`, or of no use again.
    ("output-format", One, Never),
    ("input-format", One, Never),
    ("json-schema", One, Never),
    ("max-budget-usd", One, Never),
    ("no-session-persistence", Nothing, Never),
    ("include-hook-events", Nothing, Never),
    ("include-partial-messages", Nothing, Never),
    ("replay-user-messages", Nothing, Never),
    ("forward-subagent-text", Nothing, Never),
    ("permission-prompts", One, Never),
    ("prompt-suggestions", Maybe, Never),
    ("autocompact", One, Never),
    ("ax-screen-reader", Nothing, Never),
    ("betas", List, Never),
    ("brief", Nothing, Never),
    ("debug", Maybe, Never),
    ("debug-file", One, Never),
    ("exclude-dynamic-system-prompt-sections", Nothing, Never),
    ("file", List, Never),
    ("plugin-url", One, Never),
    ("system-prompt-snapshot", One, Never),
    ("help", Nothing, Never),
    ("version", Nothing, Never),
];

/// Short options and their long names.
const SHORT: &[(char, &str)] = &[
    ('n', "name"),
    ('c', "continue"),
    ('p', "print"),
    ('r', "resume"),
    ('w', "worktree"),
    ('d', "debug"),
    ('h', "help"),
    ('v', "version"),
];

fn spec(name: &str) -> Option<(Takes, Carry)> {
    OPTIONS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, t, c)| (*t, *c))
}

/// Whether commander reads a word as an option.
fn optionish(word: &str) -> bool {
    word.len() > 1 && word.starts_with('-')
}

/// The start options of a session: those kept for a move, and the names of
/// those dropped (the prompt is dropped without a name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Carried {
    pub options: Vec<String>,
    pub dropped: Vec<String>,
}

/// Reads `claude`'s arguments; `cwd` is where it starts, for folders given
/// relative to it.
pub fn carried(args: &[String], cwd: Option<&Path>) -> Carried {
    let mut out = Carried::default();
    let mut words: VecDeque<String> = args.iter().cloned().collect();
    while let Some(word) = words.pop_front() {
        if word == "--" {
            // Everything after it is the prompt.
            break;
        }
        let (name, inline) = if let Some(long) = word.strip_prefix("--") {
            match long.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (long.to_string(), None),
            }
        } else if optionish(&word) {
            let mut chars = word[1..].chars();
            let c = chars.next().unwrap_or('-');
            let rest: String = chars.collect();
            let Some((_, long)) = SHORT.iter().find(|(s, _)| *s == c) else {
                drop_name(&mut out, &format!("-{c}"));
                continue;
            };
            let takes = spec(long).map(|(t, _)| t).unwrap_or(Nothing);
            if rest.is_empty() {
                (long.to_string(), None)
            } else if takes == Nothing {
                // `-cp`: `-c`, then `-p`.
                words.push_front(format!("-{rest}"));
                (long.to_string(), None)
            } else {
                (long.to_string(), Some(rest))
            }
        } else {
            // A word of the prompt.
            continue;
        };
        let Some((takes, carry)) = spec(&name) else {
            drop_name(&mut out, &format!("--{name}"));
            continue;
        };
        let values: Vec<String> = match (takes, inline) {
            (Nothing, _) => vec![],
            (_, Some(v)) => vec![v],
            (One, None) => words.pop_front().into_iter().collect(),
            (Maybe, None) => match words.front() {
                Some(w) if !optionish(w) => words.pop_front().into_iter().collect(),
                _ => vec![],
            },
            (List, None) => {
                let mut v: Vec<String> = words.pop_front().into_iter().collect();
                while words.front().is_some_and(|w| !optionish(w)) {
                    v.extend(words.pop_front());
                }
                v
            }
        };
        if carry == Never {
            drop_name(&mut out, &format!("--{name}"));
            continue;
        }
        match takes {
            Nothing => out.options.push(format!("--{name}")),
            _ => {
                for v in values {
                    let v = match carry {
                        KeepPath => absolute(&v, cwd),
                        JsonOrPath if !looks_like_json(&v) => absolute(&v, cwd),
                        _ => v,
                    };
                    out.options.push(format!("--{name}={v}"));
                }
            }
        }
    }
    out
}

fn drop_name(out: &mut Carried, name: &str) {
    if !out.dropped.iter().any(|d| d == name) {
        out.dropped.push(name.to_string());
    }
}

fn looks_like_json(v: &str) -> bool {
    matches!(v.trim_start().chars().next(), Some('{' | '['))
}

/// A folder relative to `cwd` made absolute; `~` and empty values stay.
fn absolute(v: &str, cwd: Option<&Path>) -> String {
    match cwd {
        Some(cwd) if !v.is_empty() && !v.starts_with('~') && !Path::new(v).is_absolute() => {
            cwd.join(v).to_string_lossy().to_string()
        }
        _ => v.to_string(),
    }
}

/// Whether a model id is safe to pass on and to show in a command typed by
/// hand: letters, digits and `._:/@[]-`, not starting with `-`.
pub fn valid_model(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/@[]-".contains(c))
}

fn is_model(option: &str) -> bool {
    option.starts_with("--model=")
}

/// What a move carries over, as the list knows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Moved {
    /// The kept start options; `None` when they are not known (started
    /// before this version, or without the plugin's `claude` function).
    pub options: Option<Vec<String>>,
    /// The names of the start options that are not carried.
    pub dropped: Vec<String>,
    /// The model the session runs on now, from its status line.
    pub model: Option<String>,
}

impl Moved {
    /// Nothing to hand over: the move starts as before.
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.options.as_ref().is_none_or(|o| o.is_empty())
    }

    /// The model it will run on: the current one, else the one it was
    /// started with.
    pub fn model(&self) -> Option<String> {
        self.model.clone().or_else(|| {
            self.options
                .iter()
                .flatten()
                .rev()
                .find_map(|o| o.strip_prefix("--model=").map(str::to_string))
        })
    }

    /// The options for the new session: the start options with the current
    /// model in place of a started `--model`.
    pub fn args(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .options
            .iter()
            .flatten()
            .filter(|o| self.model.is_none() || !is_model(o))
            .cloned()
            .collect();
        if let Some(m) = &self.model {
            out.push(format!("--model={m}"));
        }
        out
    }

    /// The start options (not the model) by name, with short single-word
    /// values (`--permission-mode plan`), each name once.
    fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for o in self.options.iter().flatten().filter(|o| !is_model(o)) {
            let (name, value) = match o.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (o.as_str(), None),
            };
            if out
                .iter()
                .any(|x| x == name || x.starts_with(&format!("{name} ")))
            {
                continue;
            }
            let short = value.filter(|v| {
                !v.is_empty()
                    && v.len() <= 24
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            });
            out.push(match short {
                Some(v) => format!("{name} {v}"),
                None => name.to_string(),
            });
        }
        out
    }

    /// The lines of the move's question.
    pub fn lines(&self) -> Vec<String> {
        let model = self.model().filter(|m| valid_model(m));
        let mut lines = vec![];
        let mut carries: Vec<String> = model.iter().map(|m| format!("model {m}")).collect();
        carries.extend(self.names());
        match &self.options {
            Some(_) => {
                if !carries.is_empty() {
                    lines.push(format!("Carries over: {}.", carries.join(", ")));
                }
                let dropped: Vec<String> = self.dropped.iter().map(|d| shown(d)).collect();
                if !dropped.is_empty() {
                    lines.push(format!("Not carried: {}.", dropped.join(", ")));
                }
                if model.is_none() {
                    lines.push("Its model is not known; it starts on the account's default.".into());
                }
            }
            None => lines.push(match model {
                Some(m) => format!(
                    "Carries over: model {m}. Its start options are not known, so it starts without them."
                ),
                None => "Its start options and model are not known; it starts with the account's defaults."
                    .into(),
            }),
        }
        lines
    }

    /// What the command by hand adds: `--model '<id>'`, then the other
    /// start options by name.
    pub fn by_hand(&self) -> (String, Vec<String>) {
        let model = self
            .model()
            .filter(|m| valid_model(m))
            .map(|m| format!(" --model '{m}'"))
            .unwrap_or_default();
        let names = self
            .names()
            .into_iter()
            .map(|n| n.split(' ').next().unwrap_or_default().to_string())
            .collect();
        (model, names)
    }
}

/// An option name as the question shows it: printable, at most 40
/// characters.
fn shown(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() > 40 {
        format!("{}…", clean.chars().take(39).collect::<String>())
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn keep(list: &[&str]) -> Vec<String> {
        carried(&args(list), Some(Path::new("/work/app"))).options
    }

    #[test]
    fn the_issues_options_are_kept_and_folders_made_absolute() {
        assert_eq!(
            keep(&[
                "--model",
                "sonnet",
                "--add-dir",
                "../lib",
                "--permission-mode",
                "plan"
            ]),
            args(&[
                "--model=sonnet",
                "--add-dir=/work/app/../lib",
                "--permission-mode=plan"
            ])
        );
        assert_eq!(
            keep(&["--dangerously-skip-permissions"]),
            args(&["--dangerously-skip-permissions"])
        );
        assert_eq!(
            keep(&[
                "--mcp-config",
                "./mcp.json",
                r#"{"mcpServers":{}}"#,
                "--plugin-dir",
                "p"
            ]),
            args(&[
                "--mcp-config=/work/app/./mcp.json",
                r#"--mcp-config={"mcpServers":{}}"#,
                "--plugin-dir=/work/app/p"
            ])
        );
        // Absolute, home-relative and missing folders stay as given.
        assert_eq!(
            keep(&["--add-dir", "/abs", "~/x"]),
            args(&["--add-dir=/abs", "--add-dir=~/x"])
        );
        assert_eq!(
            carried(&args(&["--add-dir", "rel"]), None).options,
            args(&["--add-dir=rel"])
        );
        assert_eq!(
            keep(&[
                "--append-system-prompt",
                "Be brief",
                "--agent",
                "reviewer",
                "--allowedTools",
                "Bash(git *)",
                "Edit",
                "--effort",
                "high",
                "-n",
                "fix",
                "--verbose",
                "--restricted",
                "--no-chrome",
            ]),
            args(&[
                "--append-system-prompt=Be brief",
                "--agent=reviewer",
                "--allowedTools=Bash(git *)",
                "--allowedTools=Edit",
                "--effort=high",
                "--name=fix",
                "--verbose",
                "--restricted",
                "--no-chrome",
            ])
        );
    }

    #[test]
    fn the_prompt_is_never_kept() {
        assert!(keep(&["fix the bug"]).is_empty());
        assert_eq!(
            keep(&["fix the bug", "--model", "opus"]),
            args(&["--model=opus"]),
            "options after the prompt still count"
        );
        assert_eq!(
            keep(&["--model", "opus", "--", "--verbose", "fix"]),
            args(&["--model=opus"]),
            "after -- everything is the prompt"
        );
        // `--add-dir=a b`: only `a` is the folder, `b` is the prompt.
        assert_eq!(
            keep(&["--add-dir=a", "b"]),
            args(&["--add-dir=/work/app/a"])
        );
        // A flag takes no value, so the next word is the prompt.
        assert_eq!(keep(&["--verbose", "fix it"]), args(&["--verbose"]));
        // A value that looks like an option stays a value.
        assert_eq!(
            keep(&["--append-system-prompt", "--model evil"]),
            args(&["--append-system-prompt=--model evil"])
        );
    }

    #[test]
    fn options_that_pick_or_end_a_conversation_or_the_login_are_dropped() {
        let id = "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f6";
        let c = carried(
            &args(&[
                "--continue",
                "-c",
                "--resume",
                id,
                "-r",
                "--session-id",
                id,
                "--fork-session",
                "-p",
                "--print",
                "--output-format",
                "json",
                "--from-pr",
                "12",
                "--teleport",
                "--cloud",
                "--desktop",
                "--bg",
                "--worktree",
                "feature",
                "-w",
                "--tmux",
                "--remote-control",
                "--settings",
                "/tmp/s.json",
                "--setting-sources",
                "user",
                "--bare",
                "--frobnicate",
                "-x",
                "--model",
                "opus",
            ]),
            Some(Path::new("/w")),
        );
        assert_eq!(c.options, args(&["--model=opus"]), "{c:?}");
        for name in [
            "--continue",
            "--resume",
            "--session-id",
            "--fork-session",
            "--print",
            "--output-format",
            "--from-pr",
            "--teleport",
            "--cloud",
            "--desktop",
            "--bg",
            "--worktree",
            "--tmux",
            "--remote-control",
            "--settings",
            "--setting-sources",
            "--bare",
            "--frobnicate",
            "-x",
        ] {
            assert!(c.dropped.iter().any(|d| d == name), "{name}: {c:?}");
        }
        assert_eq!(
            c.dropped.iter().filter(|d| *d == "--continue").count(),
            1,
            "each name once"
        );
        // No dropped value leaks into the kept options.
        assert!(!c
            .options
            .iter()
            .any(|o| o.contains(id) || o.contains("s.json")));
    }

    #[test]
    fn short_options_combine_like_claude_codes_parser() {
        // `-cp`: two flags; `-nfoo`: a name; `-rabc`: resume abc.
        let c = carried(&args(&["-cp", "-nfoo", "-rabc", "-d"]), None);
        assert_eq!(c.options, args(&["--name=foo"]));
        assert_eq!(
            c.dropped,
            args(&["--continue", "--print", "--resume", "--debug"])
        );
        // `-r` takes the next word unless it is an option.
        let c = carried(&args(&["-r", "--verbose"]), None);
        assert_eq!(c.options, args(&["--verbose"]));
    }

    #[test]
    fn the_current_model_replaces_the_started_one() {
        let m = Moved {
            options: Some(args(&["--model=sonnet", "--add-dir=/a"])),
            dropped: vec![],
            model: Some("claude-opus-4-5[1m]".into()),
        };
        assert_eq!(
            m.args(),
            args(&["--add-dir=/a", "--model=claude-opus-4-5[1m]"])
        );
        // Without a known current model the started one stays.
        let m = Moved { model: None, ..m };
        assert_eq!(m.args(), args(&["--model=sonnet", "--add-dir=/a"]));
        assert_eq!(m.model().as_deref(), Some("sonnet"));
        // Started without options: the current model only.
        let m = Moved {
            options: Some(vec![]),
            dropped: vec![],
            model: Some("claude-sonnet-4-5".into()),
        };
        assert_eq!(m.args(), args(&["--model=claude-sonnet-4-5"]));
        assert!(!m.is_empty());
        assert!(Moved::default().is_empty());
        assert!(Moved {
            options: Some(vec![]),
            ..Moved::default()
        }
        .is_empty());
    }

    #[test]
    fn a_second_move_carries_the_same() {
        let first = carried(
            &args(&["fix it", "--model", "sonnet", "--add-dir", "../lib"]),
            Some(Path::new("/work/app")),
        );
        let moved = Moved {
            options: Some(first.options),
            dropped: first.dropped,
            model: Some("opus".into()),
        };
        let mut started = moved.args();
        started.extend(args(&["--resume", "0b7f3c2e-1d4a-4c6b-9e8f-a1b2c3d4e5f6"]));
        let again = carried(&started, Some(Path::new("/elsewhere")));
        assert_eq!(
            again.options,
            args(&["--add-dir=/work/app/../lib", "--model=opus"])
        );
    }

    #[test]
    fn model_ids_are_checked() {
        for good in [
            "claude-opus-4-5[1m]",
            "sonnet",
            "us.anthropic.claude-sonnet-4-5-v1:0",
            "arn:aws:bedrock:us-east-1:123:inference-profile/x",
        ] {
            assert!(valid_model(good), "{good}");
        }
        for bad in [
            "", "-x", "a b", "a'b", "$(x)", "a;b", "a`b`", "a*", "a?", "x\ny",
        ] {
            assert!(!valid_model(bad), "{bad}");
        }
    }

    #[test]
    fn the_question_names_options_not_long_values() {
        let long = "x".repeat(400);
        let m = Moved {
            options: Some(vec![
                "--model=sonnet".into(),
                "--add-dir=/a".into(),
                "--add-dir=/b".into(),
                "--permission-mode=plan".into(),
                format!("--append-system-prompt={long}"),
                "--dangerously-skip-permissions".into(),
            ]),
            dropped: args(&["--worktree", "--settings"]),
            model: Some("claude-opus-4-5[1m]".into()),
        };
        let lines = m.lines();
        assert_eq!(
            lines,
            args(&[
                "Carries over: model claude-opus-4-5[1m], --add-dir, --permission-mode plan, --append-system-prompt, --dangerously-skip-permissions.",
                "Not carried: --worktree, --settings.",
            ])
        );
        let unknown = Moved::default().lines().join(" ");
        assert!(unknown.contains("not known"), "{unknown}");
        let model_only = Moved {
            model: Some("opus".into()),
            ..Moved::default()
        };
        assert!(model_only.lines()[0].starts_with("Carries over: model opus."));
        let (model, names) = m.by_hand();
        assert_eq!(model, " --model 'claude-opus-4-5[1m]'");
        assert_eq!(
            names,
            args(&[
                "--add-dir",
                "--permission-mode",
                "--append-system-prompt",
                "--dangerously-skip-permissions"
            ])
        );
        // A model id a shell would misread is left out of the way by hand.
        let odd = Moved {
            model: Some("x';rm -rf ~'".into()),
            ..Moved::default()
        };
        assert_eq!(odd.by_hand().0, "");
    }
}
