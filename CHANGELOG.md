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

- A session moved with `m` keeps the model it ran on and the options it was
  started with that are safe to give again (such as `--permission-mode`,
  `--add-dir` and `--dangerously-skip-permissions`); the prompt and options
  that pick a conversation or could replace the login are never given
  again. The question names what carries over, and scrolls when it is
  taller than the pane.
- The Sessions tab is called Agents, the word Herdr's sidebar uses.

### Fixed

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
