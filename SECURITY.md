# Security

Please report a vulnerability privately through GitHub:
**Security → Report a vulnerability** on this repository. Do not open a public
issue for it.

What the plugin handles:

- **Claude.ai logins**: never. Claude Code signs each account in through its
  own flow and keeps the login itself, one per account folder. The plugin
  only sets `CLAUDE_CONFIG_DIR` and runs `claude auth login`, `claude auth
  status` and `claude auth logout`; it shows the sign-in link Claude Code
  prints and passes on a code the sign-in page may show.
- **API keys**: pasted into a field in the list that shows dots, handed to
  the system keychain (`security -i` on macOS, `secret-tool` on Linux) on
  standard input, never on a command line, and then forgotten. The plugin
  reads a key only to start Claude Code with it in `ANTHROPIC_API_KEY`. It
  never writes a key to a file or a log. The keychain tool is taken from
  `/usr/bin`, not from `PATH`. Two limits to know: the key is in the
  session's environment, so every program the session starts (tools, hooks,
  MCP servers) can read it, as with any `ANTHROPIC_API_KEY`; and on macOS
  any program running as you can read the keychain item with `security`, as
  with Claude Code's own login.
- **Removing an account**: signs its folder out and checks that it worked,
  or deletes its key and checks that it is gone; it says so when either did
  not work. A removed account's id is never given out again, because Claude
  Code ties a login to the folder's path and a new account in the old folder
  could otherwise find it.
- **Files it writes**: its own config directory, the `claude` function block
  in your shell's startup file and the status line command in
  `~/.claude/settings.json` (both only through setup, each backed up first).
  A file it rewrites keeps its permissions; backups are no more readable
  than the original, in a `backups/` folder only you can open. Account
  folders contain links to `~/.claude`; removing an account deletes the
  links, never what they point to.

The most likely issues are in the links between an account's folder and
`~/.claude`, and in the shell function.
