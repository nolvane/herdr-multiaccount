# Herdr Multiaccount

A [Herdr](https://herdr.dev) plugin, written in Rust, that keeps several
Claude accounts side by side: a company Enterprise seat, a personal Max
plan, an API key from the Claude Console. It shows how much of each
account's usage limits is used, lets you pick the account new sessions use,
gives each Herdr space its own account if you like, and moves new sessions
to the next account when one is at its limit.

```
  Claude accounts    Accounts (3)   Spaces (4)   Agents (5)                 fallback at 95%

    Account                            5-hour              7-day               Used by
  ● Work                               –                   –                   2 spaces
    active · Enterprise · Acme · alex@acme.test  no limit  no limit            ▶ 3 running
    Personal                           ━━━━━━━───   68%    ━━━──────   31%     1 space
    Max 5x · alex@example.com          resets in 2h 10m    resets in 4d 3h     ▶ 1 running
    Sandbox                            –                   –                   no spaces
    API key                            no limit            no limit
 ⏎ make active  a add  c start Claude  ? all keys  q close                  r refresh  ↻ read 4s ago
```

## What it does

- **Several accounts.** The main login (whatever Claude Code's own `/login`
  holds in `~/.claude`), any number of claude.ai subscriptions (Pro, Max,
  Ultra, Team, Enterprise, also with single sign-on) and API keys from the
  Claude Console.
- **One setup.** Every account uses the same settings, skills, plugins,
  memory, `CLAUDE.md` and session history in `~/.claude`. Only the login is
  separate (see "How it works").
- **Active account.** `⏎` on an account makes it the one new sessions use.
  Sessions that already run keep their account.
- **Limits.** The 5-hour and 7-day windows of each subscription with the time
  until they reset, a gateway's spend limit, and this month's cost per
  account as Claude Code prices it. While the list is open it reads each
  subscription's limits every minute, also for accounts no session uses.
- **Spaces.** In the Spaces tab, `⏎` on a space picks its account. New
  sessions in that space use it; the sidebar shows it next to the space.
- **Fallback.** When the account a session would get is used at least 95 %
  (in any window), the session goes to the next account in the list that has
  room. You get one Herdr notification when that starts. Once the full
  window resets, sessions go back by themselves. `f` keeps an account out of
  fallback, `K` and `J` change the order, `F` turns fallback off.
- **Reminder to go back.** A session that fallback started on another
  account cannot switch by itself once the full account has room again. Then
  a Herdr popup opens over whatever is in front and stays until you answer:
  restart that session with `/exit` and `claude --continue`, or "Go to
  session". Once per session.
- **Start with an account.** `c` opens a new tab with Claude Code on the
  selected account, whatever the space or the active account say.
- **Agents.** The Agents tab lists every Claude Code session in Herdr:
  what it does (working, waiting for you, idle), its space, the account it
  runs on and that account's limits. `⏎` goes to the session. `d` ends it
  after asking: its pane closes, and `claude --resume` finds the conversation
  again. A session that is working or waiting for you can be ended too; the
  question warns that its current work stops.
- **Move a running session.** `m` in the Agents tab moves an idle session to
  another signed-in account: Claude Code in its pane ends and starts again
  there on that account, with the same conversation, and the pane's label
  follows. You pick from every other account (not one with the same login);
  one at its limit hands over to the next account with room, as at any
  start, and the question says where the session goes. The first answer
  afterwards is slower and counts more against the new account's limit,
  because Claude Code's cache of the conversation stays with the old login;
  text typed but not sent is lost. A session that is working or waiting for
  an answer is not moved. Sessions started without the plugin count as the
  main login.
- **Pane labels.** Each Claude Code pane carries the account it runs on, for
  the sidebar (see "Install").
- **A note on first open.** The list explains the main login, the active
  account and fallback once; `?` shows it again with all keys.

### Watch out for

- **A Console main login without an API key.** Claude Code keeps that kind
  of Console sign-in for the whole computer, and it and claude.ai logins sign
  each other out. The list refuses to add a subscription while the main login
  is one; sign the main login in with `claude auth login --console` and
  choose "Create an API key" first.
- **The browser's account.** The sign-in page uses whatever Claude account
  the browser has open. Sign out there or use a private window; the list
  warns when two accounts end up with the same login.
- **Two entries with the same login.** This also happens when `/login` in
  Claude Code switches the main login to an account the list already has.
  Both entries then say "same login as …", share one set of limits (the
  newest reading counts for both) and fallback never moves a session from
  one to the other. Remove one of them, or sign one in again, to keep them
  apart.
- **Shells opened before setup.** They do not have the `claude` function;
  sessions started there run on the main login and show as "without
  account" in the Spaces tab and "Main login (likely)" in the Agents tab.
  Open a new tab after setup.
- **Removing an account with running sessions.** They stop working; the
  list warns before it removes one.
- **Uninstalling.** Run `herdr-multiaccount setup --undo` first, or Claude Code's
  status line stays empty.

### What it cannot do

- **Fresh limits while the list is closed.** Then only the status line of
  running sessions updates them; the last reading stays, and a window whose
  reset time has passed counts as fresh.
- **Limits of Enterprise seats and API keys.** Claude Code reports none for
  them; the list shows their monthly cost instead.
- **The desktop app.** It signs in with its own login; the plugin changes
  only `claude` started from a shell.

## How it works

Claude Code supports several accounts through
[`CLAUDE_CONFIG_DIR`](https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts):
a folder per account, each with its own login. Separate folders would mean
separate settings and history, so the plugin keeps each account's folder in
`~/.config/herdr/plugins/config/multiaccount/homes/<account>/` and fills it with
links to the entries of `~/.claude`. Only what belongs to the login stays in
the folder: Claude Code's global config with the signed-in account, the
organisation's policy files, the background-session daemon. Your MCP servers
and per-project trust are copied into it from `~/.claude.json` whenever a
session starts.

- **Subscriptions** sign in through Claude Code's own flow (`claude auth
  login`, run by the list in a dialog you can cancel with `Esc`), and Claude
  Code keeps the login itself (on macOS in the Keychain, one entry per
  folder). The plugin never sees a token.
- **API keys** go into the system keychain (macOS Keychain, or the Secret
  Service through `secret-tool` on Linux). You paste the key into a field
  that shows dots; the plugin reads it only to start Claude Code with
  `ANTHROPIC_API_KEY`.
- **The main login** is what `~/.claude` holds; the plugin does not touch it.
  Sign it in again with `/login` in Claude Code.

Two things outside Herdr make it work. Setup (`S` in the list) adds both,
backs up each file first, and `herdr-multiaccount setup --undo` takes them back:

1. A `claude` shell function in `~/.zshrc` or `~/.bashrc`. It runs
   `herdr-multiaccount run`, which picks the account (the one named with
   `HERDR_MULTIACCOUNT_USE`, else the space's, else the active one, else the next
   one with room), prepares its folder and then becomes the real `claude`.
   Claude Code started inside such a session (headless runs, subagents)
   stays on the same account.
2. Claude Code's status line command, wrapped: `herdr-multiaccount statusline`
   records the usage windows and cost Claude Code hands the status line, for
   the account of that session, and then runs the status line command you had
   with the same input, so it looks as before. Without one, it shows the
   account and its usage. Inside Herdr it also notes which conversation
   (Claude Code's session id) each pane holds, so `m` can move it.

While the list is open, it also runs Claude Code's own `/usage` command for
each signed-in subscription once a minute (`claude -p /usage` in the
account's folder, with `--safe-mode` and `--no-session-persistence`, so no
hooks, plugins or MCP servers start and no session is saved). That uses no
model and costs nothing; Claude Code asks with its own login, as when you
type `/usage`, and the list reads the percentages and reset times from what
it prints. `check_limits` sets how often, `0` turns it off.

The plugin makes no network requests of its own and never asks Anthropic for
anything; all numbers come from Claude Code.

## Install

```sh
herdr plugin install nolvane/herdr-multiaccount
```

Herdr builds it with `cargo` (install Rust from https://rustup.rs first).
Then add keys to Herdr's `config.toml`, and the account to the sidebar:

```toml
[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "multiaccount.open"
description = "Claude accounts"

[[keys.command]]
key = "prefix+shift+a"
type = "plugin_action"
command = "multiaccount.open-split"
description = "Claude accounts (split)"

# The space's account under its name, and each Claude Code pane's account.
[ui.sidebar.spaces]
rows = [["state_icon", "workspace"], ["$claude_account", "branch", "git_status"]]

[ui.sidebar.agents.rows_by_agent]
claude = [["state_icon", "machine", "workspace", "tab"], ["agent", "$claude_account"]]
```

and reload with `herdr server reload-config`. Open the list with
`prefix+a`, press `S` to set up, then `a` to add accounts. Open a new
terminal tab afterwards, so the shell has the `claude` function.

To work on the code, clone the repository, run `sh scripts/build.sh` and
`herdr plugin link "$PWD"`. `./herdr-multiaccount` shows the list in any
terminal.

## Keys

| Key | Does |
|---|---|
| `⏎` `Space` | Accounts: make active. Spaces: choose the space's account. Agents: go to the session |
| `a` | Add an account: subscription, subscription with SSO, or API key |
| `l` | Sign in again, or store the key again |
| `n` | Rename (the login stays) |
| `d` | Remove: signs it out or deletes its key and deletes its folder of links; anything made only in that folder goes to `backups/` (asks first). Agents: end the session; its pane closes (asks first) |
| `m` | Agents: move the session to another account, with its conversation (asks first) |
| `c` | Start Claude Code with this account in a new tab |
| `f` | Take part in fallback, or not |
| `K` `J`, `Shift+↑` `Shift+↓` | Move the account up or down; fallback goes down the list |
| `x` | Spaces: back to the active account |
| `F` | Fallback on or off |
| `S` | Set up the `claude` function and the status line |
| `←` `→` `Tab` `1` `2` `3` | Accounts, spaces or agents |
| `r` | Refresh now (sign-in state is checked every 10 minutes) |
| `i` | Nerd Font icons or plain symbols |
| `?` | All keys and how the list works (scrolls with `↑` `↓` when the window is low) |
| `q` | Close the list (asks first); `Esc` only closes dialogs |

## Command line

```sh
herdr-multiaccount status            # the accounts and their usage as text
herdr-multiaccount use Personal      # make an account active
herdr-multiaccount setup             # what S does in the list
herdr-multiaccount setup --undo      # take the shell function and status line back
HERDR_MULTIACCOUNT_USE=Personal claude   # one session on a given account
```

The binary for the command line is
`~/.config/herdr/plugins/config/multiaccount/bin/herdr-multiaccount`.

## Settings

Optional. Copy `config.toml.example` to the directory
`herdr plugin config-dir multiaccount` prints and name it `config.toml`.

| Setting | Default | Meaning |
|---|---|---|
| `fallback` | `true` | Move new sessions to the next account when one is at its limit |
| `threshold` | `95` | Used share (50 to 100 %) from which an account counts as at its limit |
| `notify` | `true` | A Herdr notification when fallback starts moving sessions |
| `remind` | `true` | A Herdr popup when a session can go back to the account fallback moved it away from |
| `check_limits` | `60` | Seconds between limit checks while the list is open (at least 30); `0` turns them off |
| `placement` | `overlay` | `overlay` over the tab, `split` next to the focused pane, or `tab` |
| `split_direction` | `down` | Where a split goes: `down` or `right` |
| `icons` | `nerd` | `plain` for terminals without a Nerd Font (or press `i`) |

## Files

Everything lives in the plugin's config directory
(`~/.config/herdr/plugins/config/multiaccount/`):

| Path | What |
|---|---|
| `accounts.json` | The accounts, the active one, the spaces |
| `usage/<account>.json` | Last usage reading and this month's sessions per account |
| `homes/<account>/` | A subscription's Claude Code folder (links to `~/.claude`) |
| `bin/herdr-multiaccount` | The copy the shell function and status line run |
| `backups/` | Files replaced by setup or when a link was restored; only you can open it, and each copy keeps the original's permissions |
| `probe/` | Where the limit checks run (empty) |
| `sessions/<pane>.json` | The account of each session started through the plugin, removed when it ends |
| `conversations/<pane>.json` | Which conversation each Claude Code pane holds, from its status line; dropped after a week |

## Development

```sh
sh scripts/check.sh
```

This runs a secret scan (needs `gitleaks`), `cargo fmt --check`,
`cargo clippy`, the tests and the release build. Design decisions are in
[docs/decisions.md](docs/decisions.md).

Claude Code is a product of Anthropic. This plugin is not made, endorsed or
supported by Anthropic.
