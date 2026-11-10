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

Renamed to the Agents tab on 2026-11-10 (see "Ending a session from the
Agents tab").

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

## 2026-11-10: Ending a session from the Agents tab

The third tab is called "Agents", the word Herdr's sidebar uses for what it
lists. Only the tab and the texts that name it changed: each row is still a
"session" ("go to session", "end session"), as in the README, the reminder
and fallback.

`d` in the tab ends a session after asking, also one that is working or
waiting for an answer (the question warns that its current work stops).
Herdr has no command to end an agent (`herdr agent` only lists and reads),
and typing `/exit` into another pane is neither offered to plugins nor read
by a busy session, so ending closes the session's pane with `herdr pane
close`. Claude Code has saved the conversation by then, so `claude --resume`
finds it again. Before closing, the list asks Herdr for its panes once more
and closes only when that pane still runs a Claude Code session: a row read
seconds ago may belong to a pane that ended, or now runs something else. This
does not contradict "Restarting is left to the user" in the reminder entry:
the user asks for it, and the list asks back.

`herdr pane close` is in Herdr 0.9.0 (its CLI reference and
`src/cli/pane.rs` at tag `v0.9.0`), so `min_herdr_version` stays 0.9.0.

## 2026-11-10: Moving a running session to another account

`m` in the Agents tab moves an idle Claude Code session to another account:
the list ends Claude Code in its pane and starts the same conversation there
again with `HERDR_MULTIACCOUNT_USE=<account> "<binary>" run -- --resume <id>`.
This changes the stance of "A popup when a moved session can go back"
("restarting is left to the user"): the user asks for the move, the list
asks back, and only an idle session (Herdr's `idle` or `done`) is moved, so
no work in progress is cut off.

- **Which conversation.** The status line notes Claude Code's `session_id`
  per pane in `conversations/<pane>.json`, with the processes above it
  (shells left out, so Claude Code is among them). The note counts only
  while one of those processes is in the front of the pane now (`herdr pane
  process-info`, in Herdr 0.9.0's `src/cli/pane.rs`), so an old note or a
  new Claude Code whose status line has not run yet never resumes the wrong
  conversation. `claude --continue` is never used: it could pick another
  conversation in the same folder. Herdr's own `agent_session` field is
  filled only with Herdr's Claude Code integration installed, so it is not
  relied on. Only ids with the exact UUID shape are written or read, because
  the id goes into a command typed into a shell; account ids are checked to
  be letters, digits, `-` and `_` for the same reason.
- **Ending it.** The list presses Ctrl+C (Herdr `pane send-keys`), at most
  four times: the first clears text typed but not sent, then two in a row
  end Claude Code (the first of them only shows "Press Ctrl-C again to
  exit"), and one is spare. Nothing is typed into Claude Code; typing
  `/exit` would send unsent text along with it as a message. Before each
  press it asks Herdr again, and stops when the session started working in
  the meantime, so no turn is cut off. The command is typed only once the
  process has left the pane and the pane's own shell is in front again (up
  to 8 s, by the clock as well, so slow Herdr calls cannot stretch it). If
  it does not end, or a nested shell or another program is in front
  instead, nothing starts and the list says which and names the way by
  hand.
- **Starting it again.** The command calls the plugin's binary by the path
  the `claude` function uses, so it also works in a shell opened before
  setup. It goes through the launcher, which labels the pane and writes its
  note without a `wanted` account, so the reminder of an earlier fallback no
  longer fires. The list waits up to 20 s until Herdr sees Claude Code again;
  the launcher itself in front does not count, as it may still stop before
  Claude Code starts (a missing key). Its start options and current model
  are carried over since #9 (see "A move keeps the start options and the
  model").
- **Which accounts.** Every account signed in with a login or key of its
  own, except the session's own and those with the same login. Sessions
  started without the plugin count as the main login. An account at its
  limit can be picked; then fallback applies as at any start (the owner's
  decision on #6), never back to the session's own login, and the question
  says where the session goes. The list resolves this itself and names the
  final account in the command, as a named account never falls back.
- **The cost.** Claude Code's prompt cache belongs to the login, so the
  first answer on the new account reads the whole conversation again: slower
  and more against its limit. The question says so.
- While a move runs (up to about half a minute), quitting the list, `d` and
  another `m` wait, since quitting halfway would leave the session ended.
  (2026-11-10: replaced for quitting by "A move runs in a process of its
  own"; `d` and another `m` still wait.)

## 2026-11-10: A move keeps the start options and the model

A session moved with `m` (#9) starts again with the options it was started
with that are safe to give again, and on the model it ran on right before
the move.

- **Which options.** `carry.rs` reads `claude`'s arguments the way Claude
  Code's parser (commander) does, from a table of every option in `claude
  --help` (2.1.311) with the kind of value it takes: one value (also one
  starting with `-`), an optional value (not one starting with `-`), lists
  (`--add-dir a b`; `--add-dir=a b` takes only `a`), combined short flags
  (`-cp`), options after the prompt, and `--`. Kept: `--model`,
  `--permission-mode`, `--dangerously-skip-permissions`,
  `--allow-dangerously-skip-permissions`, `--add-dir`, `--mcp-config`,
  `--strict-mcp-config`, `--agent`, `--agents`, `--allowedTools`,
  `--disallowedTools`, `--tools`, `--effort`, `--fallback-model`,
  `--plugin-dir`, `--append-system-prompt`, `--system-prompt`, `--chrome`,
  `--no-chrome`, `--ide`, `--verbose`, `--name`, and `--restricted`,
  `--safe-mode` and `--disable-slash-commands`, because dropping those three
  would let the moved session do more than before. Never: the prompt,
  options that pick, fork or end a conversation or start it elsewhere
  (`--continue`, `--resume`, `--session-id`, `--fork-session`, `--print` and
  its options, `--from-pr`, `--teleport`, `--cloud`, `--desktop`, `--bg`,
  `--worktree`, `--tmux`, `--remote-control`), options that can set or
  force a credential and so undo the account switch (`--settings` can set
  `apiKeyHelper`, `--setting-sources`, `--bare` reads only
  `ANTHROPIC_API_KEY`), and anything not in the table (an option of a newer
  Claude Code is dropped until the table knows it). Kept options are written
  back as `--name=value`, one per value, so a value can never be read as an
  option; folders (`--add-dir`, `--plugin-dir`, `--mcp-config` and `--agents`
  files) are made absolute, so they point to the same place after the move.
  The system prompt options are passed again, but Claude Code keeps the
  system prompt it recorded for the conversation anyway
  (`--system-prompt-snapshot`), so nothing is promised about them.
- **Where they are kept.** The launcher stores the kept options and the
  names of the dropped ones in its pane note, which is now readable only by
  its owner (values such as `--mcp-config` JSON may hold tokens). They count
  only while that note's process is the conversation's Claude Code. The
  status line notes `model.id` with the conversation; model ids are checked
  against letters, digits and `._:/@[]-`.
- **How they get there.** Option values are never typed into the pane:
  they would land in the shell's history, zsh reads a model id like
  `…[1m]` as a pattern, and a multi-line value breaks the typed line.
  Before the first Ctrl+C the list writes a one-time hand-over,
  `moves/<pane>.json` (file 0600, folder 0700): the conversation id, the
  options with the current model in place of a started `--model`, and the
  time. It types `… run --moved -- --resume <id>`. The launcher uses the
  hand-over only when its arguments are exactly `--resume <id>` of the same
  conversation and the hand-over is under 2 minutes old, deletes it in every
  case, filters it through the same table again (so a file put there by
  something else cannot add other options), and records the options again
  for the next move, with the names the first start dropped. Without a
  usable hand-over it starts as before, says so in one line, and notes its
  start options as not known, as it does when the list did not know them
  either, so a later move never claims options it lost. The list deletes
  the hand-over when the move stops before the command is typed; after that
  the launcher may still read it, and every write or read of a hand-over
  removes the ones older than 2 minutes, so none with option values stays
  behind.
- **The question.** It names the model and the carried options (values only
  for short single words, so a long `--append-system-prompt` never stretches
  it) and the dropped ones, or says the start options are not known.
  `--resume` and `--continue` are not named as dropped: the move resumes the
  conversation itself. If the model the question named changed before `y`,
  nothing is moved; a model noted only after the question is used. A question
  taller than the pane scrolls with ↑ ↓, as the keys overlay does. The way
  by hand adds `--model '<id>'` and names the options to add.

## 2026-11-10: Questions answer only to their keys, and a move is never silent

On every question in the list, any key but `y` used to close it without a
word, so `Enter` after picking the move's account cancelled the move
silently (#10). Now `y` answers yes and `n` or `Esc` no; any other key
leaves the question open. `Enter` answers yes only on the move question,
because it follows the `Enter` that picked the account; on the questions
that end a session, remove an account or change setup it does nothing, so a
stray `Enter` never loses work (the owner's decision on #10). The buttons
name these keys, and fall back to `y yes` / `n no` where the pane is too
narrow for the longer ones. Keys held with Ctrl or Alt never answer. Ctrl+C
over a dialog (a question, the move's account list) shows the quit bar, and
staying brings the dialog back.

A move says how it ended in every case: turning it back (`n`/`Esc` on the
question, `Esc`/`q` in the account list) says "<session> was not moved" in
the footer; a refusal after yes (Herdr not reachable, the session ended,
got busy, holds another conversation, its model changed) opens the same
dialog as a failed move, since the user just answered and waits for an
outcome; refusals at `m` stay footer notes, as nothing was decided yet.
While the move runs (up to about half a minute) the footer keeps "Moving
<session> to <account>…" (a newer note covers it only for its 5 seconds),
and the session's row in the Agents tab says "moving to <account>…".
Closing the list while a move runs is #12.

## 2026-11-10: A move runs in a process of its own

A move ran on a thread of the list, so closing the list (the open-or-close
action, Herdr's close key, closing its tab or space) stopped it halfway and
could leave the session ended with no word (#12). Now the list writes the
move to a note, `moving/<pane>.json` (owner-only: it holds the start option
values from #9), and starts `herdr-multiaccount move <pane>`, which runs the
same steps (`relocate::run_move`) and writes the result into the note.

- **Its own session, not only its own process group.** Checked in a
  throwaway Herdr 0.9.5 server with a stand-in process: after `herdr plugin
  pane close` and after closing the workspace, children in the pane's
  process group, in their own process group, and in their own group with
  SIGHUP ignored all ended at once; only a child that called `setsid` kept
  running. Rust's `CommandExt::setsid` is not stable yet, so `pre_exec`
  calls the C `setsid` in one small `unsafe` block, the project's first;
  a test checks the child leads a session of its own. The list keeps a
  thread that waits for the child, so a killed move process is no zombie
  that would look alive.
- **Only the pane on the command line.** Other users can read command
  lines, so the plan stays in the note; `move` takes the pane id only.
- **Who tells.** Every list reads the notes (twice a second): a running
  move shows as #10's moving state in any list, also one opened later or in
  another tab, and `m` and `d` wait for it. A result is told once: whoever
  removes the note tells it. The note names the list that started the move
  (its pid); only that list takes the result while it runs, so a list in a
  tab the user does not see never swallows it; once that list is gone, any
  list may. A list takes a result only while no dialog of its own is open
  and shows it at once, so a list that closes in between never holds one.
  The move process leaves the result to a list for 3 s; then a success says
  nothing (the owner's decision on #12) and a failure opens a popup
  (entrypoint `moved`, reusing the reminder's dialog) that takes its text
  from `<pane>.report`. If the popup cannot open or has not taken the text
  within 10 s, a notification goes out instead; whoever removes the report
  shows it, so never both. The text names the model and the option names to
  add, never option values, so the notification's command line holds
  nothing private.
- **A move that stopped.** A note without result whose process is gone, or
  that never noted its process within 30 s, or is older than 2 minutes (a
  reused pid; a move takes about half a minute at most), is told as "The
  move of X stopped before it finished" with the way to resume by hand. A
  move process whose note was told that way writes no result.
- **Known gaps, accepted.** Checking "a move runs" and writing the note are
  two steps, so two lists answering yes in the same instant for different
  sessions could start two moves; each still runs safely on its own pane.
  The move process starts from the copy in `bin/` when there is one, since
  on Linux the list's own file is gone after a rebuild.
- **Quitting.** `q` / Ctrl+C then `y` close the list while a move runs (the
  owner's decision on #12), since the move goes on.

## 2026-11-10: Closing a space from the Spaces tab

`d` in the Spaces tab closes a space after asking, with
`herdr workspace close`: its tabs and panes close, and everything running in
them stops. Only `d`: `Delete` and `Backspace` keep "back to the active
account" there, and `Enter` does nothing on this question (the owner's
decisions on #11 and #10). The question counts the space's Claude Code
sessions as the "Sessions" column does (`App::sessions_in`) and warns when
one of them is working or waiting for an answer. The checkout of a worktree
space and its branch stay: closing a space only closes Herdr's state.

- **Worktree groups.** Herdr refuses to close the main checkout of a
  repository alone while worktree spaces of that repository are open; with
  `--group` it closes all of them. The list reads each space's checkout from
  `herdr workspace list` (`worktree.repo_key`, `is_linked_worktree`) and
  works out the group by Herdr's own rule (`workspace_close_indices` at tag
  `v0.9.0`), so the question names those spaces and counts their sessions
  (the owner's decision on #11). Before closing, the list reads the spaces
  again: a space that is gone is "had already closed", and a group that grew
  since the question closes nothing, so `y` never takes a space it did not
  name. `--group` is passed only when the fresh group has two or more
  spaces. A space whose answer lacks `is_linked_worktree` counts as a
  worktree, so it closes alone and never takes a group by mistake. The
  panes are read again too: a Claude Code session that opened in those
  spaces since the question, or one that started working without the
  question warning about it, closes nothing ("changed since the question").
- **The list's own space.** Closing it is allowed (the owner's decision on
  #11); the question says that the list closes too. The list's pane is
  `HERDR_PANE_ID`, kept in `App::own_pane`, which `is_session` uses as well.
- **Forgetting the account choice first.** A closed space's account choice
  is removed from `accounts.json`, as `x` does: after a Herdr restart the
  startup hook would otherwise hand it to a new space with the same name
  (see "Spaces by id"). It is removed before the close, because Herdr ends
  the list itself when the list's space closes, and put back when Herdr
  refuses, only for the spaces still open and still without a choice (one
  another list made meanwhile wins). The choices are taken from the
  file under its lock, not from what the list read earlier, so a choice
  another list made meanwhile goes too.
- **Tests.** The space reader, the closer and the write of `accounts.json`
  (`App::store_update`) are fields, so tests reach neither Herdr nor the
  real config directory.

`herdr workspace close [--group]` and the `worktree` field are in Herdr
0.9.0 (`src/cli/workspace.rs` and `src/api/schema/workspaces.rs` at tag
`v0.9.0`), so `min_herdr_version` stays 0.9.0.

## 2026-11-10: Account folders named `.claude`; accounts from 1.0.0 sign in again

Claude Code always asks before a write that reaches a protected path such as
`.claude` through a link when the requested path does not name it, also in
auto mode (https://code.claude.com/docs/en/permissions, "Symlinks";
https://code.claude.com/docs/en/permission-modes, "Protected paths"). An
account folder at `homes/<id>/` linked into `~/.claude`, so every edit of a
skill or `CLAUDE.md` from an account asked. Each account's
`CLAUDE_CONFIG_DIR` is now `homes/<id>/.claude/` with the same links, so the
requested path names `.claude` as on the main login. Nothing in `~/.claude`
changes, and which entries are shared stays the same; only an entry named
`.claude` inside `~/.claude` is never linked, since the new folder has that
name inside the old one.

Claude Code keys a folder's keychain entry by the `CLAUDE_CONFIG_DIR` string
as given (checked in Claude Code 2.1.311; the docs only say the entry is keyed
to the directory), so a new path has no login, and a link cannot keep it.
An undocumented variable would keep the old entry, but it may change with any
Claude Code update and sign accounts out at an unpredictable time; it is not
used. Accounts from 1.0.0 sign in again once instead. On Linux the login file
in the old folder is not moved either: moving credential files around is
what "Logins stay with Claude Code" rules out.

Until it signs in again, an account keeps running on its old folder: the
launcher picks the new folder once its config holds a login, else the old one
when that holds one, else the new one (every sign-in goes there). The list
row and each session started on the old folder say to press `l`. The new
folder's `.claude.json` starts with the shared parts of the old one (MCP
servers, per-project settings), never its account fields. Both folders share
the lock `homes/<id>.lock`. Once the new folder had a login (a sign-in in the
list, or its config seen with one), the note `homes/<id>.new` keeps the
account on it for good, so signing out there, or a config file caught
halfway through a write, never sends sessions back to the old login.

The old folder is not signed out or cleared automatically after the new
sign-in: a session an older version started outside Herdr cannot be seen,
and clearing would take its skills and login while it runs. Once the new
folder has a login the old one is never picked again, so it stays until the
account is removed, which signs out every folder that holds a login and
deletes both, links removed and never followed.
