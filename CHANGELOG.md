# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

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
