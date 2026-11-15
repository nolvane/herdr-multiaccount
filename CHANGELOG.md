# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `d` in the Agents tab ends a Claude Code session after asking: its pane
  closes and the row goes. The question warns when the session is working
  or waiting for an answer, and nothing is closed when the pane ended or
  runs something else by then.
- `d` in the Spaces tab closes a space after asking, with its tabs and
  everything running in them. The question counts its Claude Code sessions,
  warns when one is working or waiting for an answer, names the worktree
  spaces that close with a repository's main checkout, and says when the
  list closes too. The space's account choice is forgotten.
- An account that still runs on its 1.0.0 folder is hard to miss: inside
  Herdr every session started on it shows a notification before Claude Code
  starts, naming the account, what Claude asks about (skills, `CLAUDE.md`,
  memory) and `l` to stop it. Its row keeps the notice also while the
  account is at its limit.

### Changed

- `claude` starts about two seconds sooner on a claude.ai account while the
  accounts list is open (or was in the last 11 minutes): it reuses the
  list's sign-in check instead of asking Claude Code again. A check that
  found the account signed out is never reused, and signing an account in
  or removing it in the list throws its check away at once.
- Setup writes the status line command in `~/.claude/settings.json` with
  `$HOME` in place of your home folder, so the file holds no user name.
  A command from an earlier setup keeps working; running setup again moves
  it to the `$HOME` form, after a backup.
- A session moved with `m` keeps the model it ran on and the options it was
  started with that are safe to give again (such as `--permission-mode`,
  `--add-dir` and `--dangerously-skip-permissions`); the prompt and options
  that pick a conversation or could replace the login are never given
  again. The question names what carries over, and scrolls when it is
  taller than the pane.
- The Sessions tab is called Agents, the word Herdr's sidebar uses.
- The open list starts far fewer Claude Code processes for its limit
  checks: it checks an account only when its last reading is older than
  `check_limits`, now 300 seconds by default (was 60). A session that
  answered in that time (its status line brings the limits), a check from
  another open list and a check of an
  account with the same login count as a reading. No checks start while
  the list is not in front in Herdr; `r` still checks every account.
- Every key the list names is bold and in the accent colour, as in the
  other Herdr plugins: in the footer, the dialogs' footers, the `?` help,
  on grey buttons and inside a sentence ("not signed in · `l` signs in").
  What a key does stays in a quieter colour; the word "active" and the
  headings in the help turn white, so the accent colour in a text means a
  key (#18).

### Fixed

- Signing an account in with `l` no longer depends on the folder the list
  was opened in: Claude Code's sign-in, its login check and its sign-out
  now run in the plugin's own `probe/` folder, as the limit checks do, so a
  folder that was removed or that macOS blocks no longer makes them fail
  until the list is restarted (#25).
- A sign-in that fails opens a dialog with everything Claude Code printed,
  line by line (it scrolls when it is taller than the pane), instead of a
  cut-off footer note, and says what became of the account: an existing
  one stays in the list, one added for the sign-in is removed again.
  Cancelling with `Esc` stays a footer note (#25).
- While `accounts.json` cannot be read, the list keeps the accounts it
  shows and says so in the footer, instead of showing only the main login
  (#25).
- `claude` no longer leaves you without a way in when the account it would
  use is signed out: at a terminal it offers Claude Code's own sign-in for
  that account; otherwise, or after a no, the session moves to the next
  account that is signed in, as fallback does. When nothing is left, the
  message names `l` in the accounts list and
  `HERDR_MULTIACCOUNT_USE=main claude`.
- In a session on a subscription account, Claude no longer asks before every
  edit in `~/.claude` (skills, `CLAUDE.md`, settings): each account's folder
  is now `homes/<account>/.claude/`, so auto mode checks these edits as on
  the main login. Accounts added with 1.0.0 have to sign in once more (`l`);
  their row and their sessions say so, and they keep working on their old
  folder until then. Nothing in `~/.claude` changes.
- Closing the list while a session is being moved no longer stops the move
  halfway and leaves the session ended without a word: the move runs in a
  process of its own and finishes; a failure shows in the next list, or as
  a Herdr popup (or notification) when no list is open. `q` now closes the
  list during a move.
- Questions wrap to narrow panes instead of being cut off at the edge.
- Moving a session never ends without a word: `⏎` on the move question
  moves it, turning it back says it was not moved, a refusal after yes
  opens a dialog with the reason, and the footer and the session's row show
  the move until it is done. On every question only `y`, `n` and `Esc`
  answer (and `⏎` on the move question); other keys leave it open.
- `o` in the sign-in dialog says when the browser opener could not be
  started, and opened links no longer leave an ended opener process behind
  while the list is open.
- A `paste-cache` (or `image-cache`) folder that Claude Code made in an
  account folder no longer makes every start print that it is not linked:
  the next start moves its files into `~/.claude` (a clashing file goes to
  `backups/`) and links it again. Other folders that are not linked still
  get a start line, which now says how to share them.
- After a `/login` inside a session put another entry's login into an
  account's folder, sessions on that account say so at start and name `l`
  to sign it in again; they still start.
- Removing a subscription account no longer freezes the list while it is
  signed out: the sign-out runs in the background, the account's row says
  "signing out…" until it is gone, and closing the list meanwhile keeps the
  account for another try. Each `claude` call that does not answer
  within 30 seconds is stopped (an account with two login folders can take
  a few minutes in all), and the list shows the command that signs it out
  by hand. The same holds when an account added for a sign-in that did not
  finish is dropped again.
- Adding a subscription (`a`), signing an account in again (`l`) and the
  end of a sign-in no longer freeze the list while Claude Code is asked
  about the login: the list shows "Checking the main login…" or "Checking
  the sign-in…" and keeps redrawing, `q` closes it without waiting, and
  `Esc` cancels the check before a sign-in. Every `claude auth status` that
  does not answer within 30 seconds is stopped. When it gave no answer, no
  claude.ai sign-in starts (a note says so, since it could sign out a
  Console main login), and an account that was just signed in stays, with
  a note that its sign-in could not be checked. A check still running when
  the list closes is stopped then.

## [1.0.0] - 2026-11-09

### Added

- The list: accounts with their 5-hour and 7-day windows, spend limit,
  monthly cost, spaces and running sessions; the spaces with their account.
- Accounts: the main login, claude.ai subscriptions (also with SSO) in
  folders of their own that link to `~/.claude`, and API keys in the system
  keychain, pasted into a masked field.
- Sign-in inside the list, with the link, a field for a code, and `Esc` to
  cancel; an account added for a cancelled sign-in is removed again. A
  warning when a new login is the same as another account's, and "same
  login as …" on two entries that share one.
- Active account, one account per space, fallback to the next account with
  room, a notification when fallback starts.
- Limits are read every minute while the list is open, through Claude
  Code's own `/usage` (headless, no model, nothing saved), also for accounts
  no session uses. `check_limits` sets how often.
- A reminder when a session that fallback moved can go back: a Herdr popup
  over whatever is in front that stays until answered, with "Go to
  session". `remind` turns it off.
- A Sessions tab: every Claude Code session in Herdr with what it does, its
  space, its account and that account's limits; `⏎` goes to it.
- A note on how the list works (main login, active account, fallback,
  limits) on first open and under `?`; the keys panel scrolls when the
  window is too low for it.
- Setup of the `claude` shell function and the status line wrapper, and its
  undo.
- Account labels for spaces and Claude Code panes in Herdr's sidebar.
- The keys overlay (`?`) shows the version and links to the repository
  (#4).
