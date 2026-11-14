# Herdr Multiaccount

A [Herdr](https://herdr.dev) plugin that keeps several Claude Code accounts
side by side: usage limits, an active account, one account per space,
fallback. What it does and how users install it: [README.md](README.md). Why
it is built this way: [docs/decisions.md](docs/decisions.md).

## Stack

- Rust (edition 2021), one binary built by `scripts/build.sh` into the plugin
  root, where `herdr-plugin.toml` starts it. The build also copies it to
  `<config dir>/bin/`, where the `claude` shell function and the status line
  run it.
- `ratatui` for the interface, `serde_json` (with `preserve_order`, so
  `settings.json` keeps its key order) for JSON, `unicode-width` for widths.
- Needs Herdr 0.9.0 or newer (`min_herdr_version` in `herdr-plugin.toml`).

## Modules

| File | What |
|---|---|
| `main.rs` | Commands: list, open, run, statusline, setup, status, use, startup, pane-event, remind |
| `app.rs`, `ui.rs`, `theme.rs` | The list: state and keys, drawing, colours |
| `store.rs` | `accounts.json`: accounts, active, spaces |
| `usage.rs` | Usage readings per account and the limit check |
| `probe.rs` | Limit readings from Claude Code's `/usage`, run headless |
| `launch.rs` | Picking the account (with fallback) and starting `claude` |
| `mirror.rs` | An account's folder of links to `~/.claude` |
| `statusline.rs` | Recording usage from Claude Code's status line input |
| `remind.rs` | The popup when a moved session can go back to its account |
| `setup.rs` | Shell function and status line setup, and undo |
| `secret.rs` | API keys in the system keychain |
| `sessions.rs` | The account and the conversation of each running session, per Herdr pane |
| `relocate.rs` | Moving a running session to another account (`m` in the Agents tab) |
| `carry.rs` | The start options and model a moved session keeps |
| `info.rs` | Who an account is signed in as (`claude auth status`), and the list's noted check the launcher reuses |
| `herdr.rs`, `pane.rs` | Herdr's command line: spaces, panes, labels, the open action |

## Conventions

- Code, comments and docs are in English.
- Notable technical decisions go into `docs/decisions.md` (decision, why,
  date).
- The repository is public: no personal data, no secrets, no private setup.
  Examples and tests use made-up people and companies (`alex@example.com`,
  Acme).
- Never handle a claude.ai token: logins happen in Claude Code's own flow and
  stay with Claude Code (see decisions). API keys only through the keychain.
- Never write outside the plugin's config directory except through setup
  (shell startup file, `~/.claude/settings.json`), and back up first.
- Removing an account must never follow a link into `~/.claude`.
- `Esc` never closes the list; `q` asks with `y`.

## Checks

```sh
./scripts/check.sh            # default stage: merge
./scripts/check.sh --stage quick
```

Every stage runs the same steps: gitleaks secret scan, `cargo fmt --check`,
`cargo clippy` with warnings as errors, the unit tests and the release build.
There are no integration tests: they would need a running Herdr server and
signed-in Claude Code accounts. Tests that touch files use temporary
directories, never the real config directory.

There is no CI workflow: the checks run locally, since GitHub's runners
would add nothing the local run does not cover.

## Releasing

The version lives in `herdr-plugin.toml` and `Cargo.toml` and must match;
after changing it run `cargo build --release` once so `Cargo.lock` follows,
then `./scripts/check.sh --stage release`. Users install from GitHub, and
`herdr plugin update` builds the newest commit of the installed ref, so a
merge to `main` is what reaches users. Try a release first with
`sh scripts/build.sh` and `herdr plugin link "$PWD"`.
`CHANGELOG.md` follows Keep a Changelog; a release ends with an annotated tag
`vX.Y.Z` and a GitHub release with that version's notes.
