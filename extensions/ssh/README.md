# SSH

Reuse SSH profiles and OpenSSH ControlMaster connections across pi sessions.
The remote host needs a POSIX shell and `cat`. Authentication uses the system
SSH client and agent. Batch mode prevents password prompts.

## Tools and commands

- `ssh_exec`: `profile`, `command`, optional `cwd` and `timeout` (seconds, 60).
  Streams stdout and stderr. Each output stream has a 1 MiB capture limit.
- `ssh_read`: `profile`, `path`, optional `timeout`. Reads a UTF-8 file.
- `ssh_write`: `profile`, `path`, `content`, optional `timeout`. Writes a UTF-8 file.
- `ssh_profiles`: lists profiles, enabled state, and the session's remote profile.
- `/ssh`: list, add, edit, enable, disable, delete, test, connect, and disconnect
  profiles. The editor exposes all profile fields. Changes select global or
  project scope.
- `/ssh remote <profile>` routes the built-in `bash` tool through that profile.
- `/ssh off` restores local bash execution.
- `/ssh connect <profile>`, `/ssh disconnect <profile>`, `/ssh test <profile>`.

Remote mode displays the active host in a footer status. It persists in
`ssh-remote` session entries and replays on session changes and compaction.
A missing or disabled remote profile blocks bash until the user changes modes.
The built-in bash timeout controls commands in remote mode. Other built-in file
tools remain local; use `ssh_read` and `ssh_write` for remote files.

## Configuration

Global: `<agentDir>/ext/ssh.json`. Project: `<cwd>/.pi/ext/ssh.json`.
Project keys override global keys recursively. The extension ignores project
config until `ctx.isProjectTrusted` returns true and reports this once.

```json
{
  "controlPersist": 600,
  "profiles": {
    "dev": {
      "host": "dev.example.com",
      "user": "developer",
      "port": 22,
      "identityFile": null,
      "jumpHost": null,
      "cwd": "/srv/app",
      "env": { "APP_ENV": "development" },
      "options": { "ServerAliveInterval": "30" },
      "enabled": true
    }
  }
}
```

`host` is required. Defaults: port 22, enabled true, empty env and options,
no user, identity file, jump host, or cwd; ControlPersist is 600 seconds.
`options` maps OpenSSH option names to string values. Connection multiplexing,
stdin, session type, remote command, and batch settings are extension-managed.
Config files use mode 0600 on Unix. Socket directories use mode 0700.
Sockets live under `<agentDir>/ext/ssh/sockets/` and use a connection hash;
changing host, user, port, identity, jump host, or options selects a new socket.
SSH runs from the socket directory with a relative ControlPath, so long agent
paths fit the Unix socket path limit. Use absolute paths for local identity
files and paths in SSH options. Disconnect closes the shared master connection,
including other sessions that use that connection.
