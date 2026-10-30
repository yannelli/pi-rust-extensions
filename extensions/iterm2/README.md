# iterm2

Created 2026-10-30. Last updated 2026-10-30.

iTerm2 integration for pi-rust. It replaces the built-in `terminal.iterm2` feature. It writes iTerm2 escape sequences through `ui.writeTerminal` and asks pi to wrap them for tmux when `TMUX` is set.

- The tab color follows the footer `runState`: working (blue), blocked (amber), done (green), error (red). Idle uses the profile color.
- A badge names the session (`π <session name>`, or the project folder).
- User variables `user.pi_state`, `user.pi_session`, `user.pi_project`, and `user.pi_model` feed iTerm2 status bar components.
- A run that takes at least `terminal.notifyMinSeconds` (default 10) requests attention.
- iTerm2 learns the cwd (`CurrentDir`).
- On exit, the tab color and the badge are reset.

The extension does nothing outside iTerm2 (`TERM_PROGRAM=iTerm.app`, `LC_TERMINAL=iTerm2`, or `ITERM_SESSION_ID`) or without the TUI.

## Commands

`/terminal-setup`: turns the integration on, lets programs write the clipboard (`defaults write com.googlecode.iterm2 AllowClipboardAccess`), shows the manual steps, and with [it2](https://github.com/tmc/it2) offers to set Option-as-Alt and escape-sequence alerts for this pane.

## Config

`~/.pi/agent/ext/iterm2.json` (global) and `<cwd>/.pi/ext/iterm2.json` (project, wins per key). The value is `true`, `false`, or an object:

| Key | Default | Use |
| --- | --- | --- |
| `enabled` | `true` | `false` turns every feature off |
| `tabColor`, `badge`, `userVars`, `attention`, `cwd` | `true` | One feature each |

Without either file, the old `terminal.iterm2` setting applies (through `settings.get`). Without that too, all features are on. `terminal.notifyMinSeconds` comes from pi's settings.
