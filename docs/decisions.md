# Decisions

## 2026-11-09: Logins stay with Claude Code; no token handling

Anthropic's terms for Claude Code say that developers "may not collect,
store, or intermediate Claude.ai credentials or session tokens — sign-in to a
Claude account must complete through Anthropic's own flow"
(https://code.claude.com/docs/en/legal-and-compliance). Tools that copy
OAuth tokens around (swapping the Keychain entry, storing `claude
setup-token` tokens) do exactly that, so this plugin does not. Each
subscription signs in with `claude auth login` in a folder of its own, and
Claude Code keeps that login itself (one Keychain entry per folder on macOS).
This is the way Anthropic documents for several accounts
(https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts).

The same terms leave the handling of one's own API keys to the customer, so
API keys (Claude Console) are stored, but only in the system keychain and
typed into the keychain tool's own prompt.

## 2026-11-09: One setup through linked folders

A plain `CLAUDE_CONFIG_DIR` per account would split settings, skills,
plugins, memory and history per account. Instead each account folder links
every entry of `~/.claude` except the ones that belong to a login
(`mirror::is_own`): the global config with the signed-in account, the
organisation's server-managed policy files (they must not apply to another
organisation's account), the background-session daemon and its jobs (they run
with the login that started them), and the claude.ai connector cache. MCP
servers and per-project trust live in the global config, so they are merged
into each account's copy when a session starts; account fields are never
copied.

Some programs save a file by writing a new one over the path, which turns a
link into a copy. The shell function and the status line put such files
back: the newer content wins, the other goes to `backups/`, the link is made
again. Folders that turned into real folders are left alone and reported.
All changes to an account folder hold a lock on it (`homes/<id>.lock`), the
healed file is written into the real file behind `~/.claude` (never copied
onto itself), and nothing is replaced unless its backup succeeded. Removing
an account deletes links and login files only; anything else real in the
folder goes to `backups/`.

## 2026-11-09: Usage only from Claude Code's status line

Extended the same day: the open list also reads Claude Code's `/usage` (see
"Limits from Claude Code's `/usage`, not from Anthropic directly"). The
numbers still come only from Claude Code.

Claude Code hands its status line the claude.ai usage windows
(`rate_limits.five_hour`, `seven_day`, and `spend_limit` behind a gateway)
and the session cost; this is documented
(https://code.claude.com/docs/en/statusline). Asking Anthropic's usage
endpoint directly would mean using a claude.ai token outside Claude Code,
which the terms above rule out. The cost of this choice: an account shows
numbers only after a session ran on it, and the reading ages until the next
session. Windows can only fill up until they reset, so an old reading is a
lower bound, and a window past its reset time counts as fresh. That is enough
for fallback. Every running session repeats the reading of its own last
answer, so an idle session would report an old, lower share: within one
window (same reset time) the higher share is kept, and a reading with an
earlier reset time is ignored.

The status line command is wrapped rather than replaced: the wrapper records
the reading and then runs the command that was there before, with the same
input.

## 2026-11-09: Fallback decides at session start, from the list order

A running Claude Code session cannot change its login, so fallback acts when
a session starts: the account the session would get (named, the space's, or
the active one) is checked against the last reading, and from the threshold
on (95 % by default) the next account down the list (wrapping around) that
takes part in fallback, has room and is signed in gets the session. Nothing
is switched for good, so sessions return to their account once the window
resets. A session started inside another (headless runs, subagents) inherits
its account but still falls back, since unattended runs are the ones that
hit limits. An account named with `HERDR_MULTIACCOUNT_USE` never falls back.

## 2026-11-09: A shell function, not a binary called `claude`

Claude Code reads its login at start, so the account must be chosen before
`claude` runs. A `claude` function in the shell's startup file does that
without putting anything named `claude` on `PATH`, falls back to the real
`claude` when the plugin is gone, and survives Claude Code updates. The
function calls a copy of the binary in the config directory, because Herdr
keeps installed plugins in folders that change with each update. The binary
replaces itself with `claude` (`exec`), so signals, job control and exit
codes are Claude Code's own.

## 2026-11-09: Console accounts as API keys; no claude.ai sign-in next to a Console profile

Claude Code can sign in to the Console without an API key. That login is
stored outside the configuration folder, as the machine's Anthropic profile
(`~/.config/anthropic`), and it and claude.ai logins sign each other out:
adding a claude.ai subscription in its own folder deleted the profile's
credentials and so signed out a main login of that kind (seen on
2026-11-09). Therefore:

- extra Console accounts come in as an API key created in the Console;
- the list refuses to sign in a claude.ai subscription while the main login
  is such a profile, and says how to switch the main login to a Console
  login with an API key (`claude auth login --console`, "Create an API
  key"), which Claude Code keeps per folder;
- the main login is never signed in from the list: that stays with Claude
  Code (`/login`), with a note to choose the API key route for Console.

## 2026-11-09: Sign-in and keys inside the list

`claude auth login` runs as a child of the list with its output read, not
with the terminal handed over: the list shows a dialog with the state, the
link (open again, copy), a field for a code the page may show, and `Esc`
cancels by ending the process (Claude Code writes nothing before success).
An account added for a sign-in that did not finish is removed again. After
a sign-in the list says who signed in and warns when that is the same login
as another account, since the browser often reuses whatever account it has
open.

API keys are pasted into a field that shows dots and handed to `security
-i` (or `secret-tool`) on standard input, never on a command line, which
other programs can read. The key is checked for the characters a key has.

## 2026-11-09: Sessions start only on a signed-in folder

Before a session starts on a subscription, the launcher asks `claude auth
status` for its folder (about 0.8 s). Without a login of its own the folder
would quietly use the main login, so the start stops with a message instead.
When Claude Code cannot be asked, the session starts.

## 2026-11-09: Labels through Herdr metadata

Spaces get a `claude_account` workspace token and Claude Code panes a
`claude_account` pane token, which Herdr's sidebar shows through
`$claude_account` in its row settings. A second pane token carries the
account id, so a rename does not lose track of running sessions; Claude Code
panes without it (started before setup, or not through the function) are
shown as "without account". The pane token also carries the pid of the
`claude` process (the launcher execs, so the pid stays); on agent status and
pane exit events the plugin clears a label whose process has ended.

## 2026-11-09: Name

The plugin is called "Multiaccount" (`herdr-multiaccount`, plugin id
`multiaccount`), not after Claude: the Claude and Claude Code names belong to
Anthropic, whose terms do not allow them in the name of another product. The
description says what it works with.

It was first called "Accounts" (`herdr-accounts`, id `accounts`). "Accounts"
was too generic for a public plugin list, so it was renamed before the first
release. Setup still recognises what the old name left behind: the shell
block between the old markers is replaced, and a status line command of the
old binary is pointed at the new one while the user's own status line stays
remembered. API keys stored under the old keychain service are still read;
storing a key again moves it to the new service. Herdr gives each plugin id
its own config directory, so data under the old id's directory has to be
moved by hand once; a subscription's login is tied to its folder's path, so
it has to sign in again after such a move.

## 2026-11-09: Rust and the Git Panel's design

Same stack, palette, layout and key habits as the Herdr Git Panel, so both
plugins feel like one: two-line rows, section tabs in the header, a footer
with the most used keys, `?` for all keys, `q` asks before closing and `Esc`
never closes. Dependencies: `ratatui`, `serde_json` (with `preserve_order`
for `settings.json`), `unicode-width`; all MIT or MIT/Apache-2.0, which allow
this use including redistribution.

## 2026-11-09: Spaces by id; one lock for accounts.json

Spaces are matched by Herdr's workspace id only, since two spaces can share
a name. After a Herdr restart the startup hook finds a space whose id is
gone by its name, among live spaces nobody has assigned. Every change to
`accounts.json` (list, status line, command line) takes a lock and works on
a fresh read, and a file that cannot be read is never overwritten.

## 2026-11-09: Limits from Claude Code's `/usage`, not from Anthropic directly

The status line only reports limits after a session's first answer, so an
unused account showed nothing. Reading the claude.ai login from the keychain
and calling Anthropic's internal usage endpoint directly, posing as Claude
Code, would fill that gap. That means handling a claude.ai
token and using an unofficial interface, which this plugin never does (see
"Logins stay with Claude Code; no token handling").

Instead the open list runs Claude Code itself: `claude -p /usage` in the
account's folder. Claude Code asks with its own login, exactly as when a
user types `/usage`; it uses no model and costs nothing (checked: zero
tokens, zero cost). `--safe-mode` keeps hooks, plugins and MCP servers from
starting, and `--no-session-persistence` keeps the run out of the session
history. The list reads "Current session: N% used · resets …" and "Current
week (all models): …" from the text. That text is not a stable interface:
when it changes, the check finds nothing and the status line readings stay
as before. Reset times are printed rounded and in a named time zone; the
system's `date` turns them into a point in time (it knows the zone's
daylight saving rules), and a reset time from the status line within the
hour is kept because it is exact.

Checks run only while the list is open, one account after the other, once a
minute by default (`check_limits`, at least 30 seconds): enough to see a
window fill up, little load, and nothing runs in the background when nobody
looks. API keys and logins without limits are skipped.

## 2026-11-09: Sessions tab

It should be visible which account each running session uses. The list
already reads Herdr's panes for the Spaces tab, and each session started
through the plugin carries its account as a pane label, so a third tab lists
the Claude Code panes with Herdr's agent status, their space and account.
Sessions without the label were started without the plugin; they use
`~/.claude` and so most likely the main login, which the tab says with
"(likely)" because the plugin cannot see their environment. The list starts
`claude` for a moment itself (login and limit checks), which Herdr can take
for a session in the list's pane; the list's own panes are left out.

## 2026-11-09: Session notes next to the pane labels

The Spaces and Sessions tabs read each pane's account from the labels the
launcher reports to Herdr. Herdr shows those labels in its sidebar, but its
command line (`pane list`, `pane get`) does not return them, so the list saw
every session as "without account" and stale labels were never cleared. The
launcher now also writes `sessions/<pane>.json` (account id and the pid of
`claude`) before it starts Claude Code. The list and the pane events read
those notes; one whose process has ended is removed, and its pane label is
cleared. The labels stay for the sidebar.

## 2026-11-09: A popup when a moved session can go back

A session keeps the login it started with, so one that fallback moved to an
API key keeps costing money after the full subscription has room again. So
the notice must be hard to miss and need an answer, and it belongs in Herdr
(a popup, not a system-wide dialog). The reminder is a plugin pane opened with
Herdr's `popup` placement (`herdr plugin pane open --placement popup`, sized
to its text): it floats over whatever pane is in front, takes the focus and
stays until "Go to session" or "Later". When Herdr cannot open it, a Herdr
notification goes out instead.

The session's own status line checks it, because it runs every 30 seconds
even while the session is idle and knows which pane and account it belongs
to: the launcher notes in `sessions/<pane>.json` which account fallback
moved the session away from, and once that account is not at its limit any
more, the status line marks the note as told and starts `herdr-multiaccount
remind` detached (its own process group, no shared output), so the status
line never waits. Restarting the session is left to the user: doing it for
them would cut off work in progress.

## 2026-11-09: Git Panel table look, "Used by" column

The list looks like the Herdr Git Panel: column headings in bold on a bar of
their own, and three cells between columns on panes 80 or more wide. Both
plugins sit side by side in Herdr, so they should read the same, colours
included: the Git Panel's violet palette. A Claude orange accent on warm
greys was tried and dropped; the violet look reads better. The tab counts
keep their brackets ("Accounts (3)").

The old "Spaces" column showed a bare number over a green number, with "–"
for none, which read as a fraction. It is now "Used by": "2 spaces" (or "no
spaces") over "▶ 3 running". The space count is where new sessions start
right now, after fallback, so an account at its limit shows no spaces and the
account that took them over shows them.

## 2026-11-09: Twins: two entries with the same login

`/login` in Claude Code can switch the main login to an account the list
already has, and a browser can sign a new entry in with the account it has
open. Both entries then draw on one set of limits, but the readings were kept
per entry, so fallback could move a session from a full account to the same
full account under its other name.

A login is identified by `oauthAccount.accountUuid` and
`organizationUuid` in the entry's `.claude.json`, which Claude Code writes on
sign-in. Both ids, because the same person in two organisations (a personal
plan and a company seat) has two sets of limits. The file is read, not
`claude auth status`, because the launcher has no second to spare and the
file changes as soon as `/login` finishes. Entries with the same login
("twins") share the limits of the freshest reading (`login::share_limits`,
applied in `launch::load_usage`, so the launcher, the status line notice, the
reminder and the list all see the same numbers); costs stay per entry. The
list says "same login as …" on both. API key accounts have no login and are
never twins.

## 2026-11-09: Hardening after a source-only security audit

A source-only security audit (quick profile) found no confirmed vulnerability and three leads. These changes close them
or the cheap hardening next to them:

- **Removed ids are never reused** (`retired` in `accounts.json`). Claude
  Code ties a login to the folder's path, so an old `homes/<id>` that still
  held a login would hand it to a new account with the same name. Removing
  and the cleanup after a cancelled sign-in now sign the folder out and check
  that it worked (`claude auth status` again); when it did not, the list
  shows the command to do it by hand. A key the keychain did not delete keeps
  the account in the list.
- **File permissions are kept.** `write_atomic` creates its temporary file
  with the replaced file's permissions, so a private `~/.zshrc` or
  `settings.json` stays private after setup, undo or a link heal. Backups
  copy the source's permissions, `backups/` is 0700, and the account
  `.claude.json` is private from its first byte. Only a reader with another
  OS account gains from this, and only where the home folder lets them in.
- **Small things**: the keychain tool comes from `/usr/bin`; API-key
  sessions drop `CLAUDE_CODE_OAUTH_TOKEN`; "signed in" ignores organisation
  and email values; the `/usage` time zone must be a zone name, not a path;
  the `date` calls give up after two seconds; long words wrap instead of
  being cut off.

Not changed: updates follow the installed branch, which is how Herdr plugins
work; the defence is that only the owner can push to `main` (a GitHub
setting). Account names starting with `-` were checked against Herdr's
notification command and pass, so nothing changes there. Passing the API key
through an `apiKeyHelper` instead of the environment is a larger change for
later.

