# Lens

Lens runs formatters, linters, type checkers and configured LSP servers after successful `write` and `edit` results.
The `context` event adds current findings to the next model request.
Each scan replaces the findings from the previous scan of that file.

Tools:

- `lens_diagnostics`: filters session findings by `paths`, `scope` and `severity`. `source: "lsp"` requests diagnostics for the given paths. With no paths, it checks files scanned in this session.
- `effective_config`: shows the merged config.

Commands:

- `/lens`: lists status and edits settings, languages, tools, commands and LSP servers. Choose project or global scope before each change.
- `/lens-allow-edit <path>`: permits one edit without a prior read.
- `/lens-toggle`: toggles Lens for this session.
- `/lens-context-toggle`: toggles context injection for this session.

The read guard blocks writes to existing files until the agent reads the file.
An edit can use a partial read when its unique `oldText` falls within that read.
External changes and successful writes invalidate the read.
New files do not need a prior read.
Guard state uses `lens-read-guard` session entries and follows the selected branch.

Config paths, in precedence order:

1. `PI_LENS_CONFIG_PATH`, when set.
2. `<agentDir>/ext/lens.json`.
3. The nearest `.pi-lens.json` above the current directory.
4. `<cwd>/.pi/ext/lens.json`.

Project fields override global fields. Objects merge by key. Arrays replace previous arrays.
Untrusted project config cannot override `commands` or `lsp`.
In an untrusted project, Lens does not auto-detect tools and does not add `node_modules/.bin` to `PATH`. Commands and LSP servers from global config still run. Lens shows one notice.

| Key | Default | Use |
| --- | --- | --- |
| `enabled` | `true` | Runs the pipeline and read guard. |
| `context` | `true` | Adds findings to model requests. |
| `readGuard` | `true` | Requires a read before an edit. |
| `autoDetect` | `true` | Detects tools from language and project files. |
| `timeoutMs` | `30000` | Sets each command and LSP timeout, from 1 to 300000 ms. |
| `pipelineTimeoutMs` | `30000` | Sets the total time for all commands and LSP servers after one write or edit, and for one `lens_diagnostics` LSP request, from 1 to 600000 ms. Lens skips the remaining steps with a warning finding. |
| `languages.<name>.enabled` | `true` | Enables `rust`, `javascript`, `python` or `go`. |
| `tools.<name>.enabled` | `true` | Enables a detected or configured tool. |
| `commands` | `[]` | Adds commands or overrides detected commands by name. |
| `lsp.servers` | `[]` | Configures stdio LSP servers. |

```json
{
  "tools": { "clippy": { "enabled": false } },
  "commands": [{
    "name": "custom-lint",
    "language": "python",
    "stage": "lint",
    "command": "custom-linter",
    "args": ["{path}"],
    "enabled": true
  }],
  "lsp": { "servers": [{
    "command": "rust-analyzer",
    "args": [],
    "languages": ["rust"],
    "enabled": true
  }] }
}
```

Stages run in this order: `format`, `lint`, `types`.
Arguments expand `{path}` and `{root}` without a shell.
Children use the nearest project root. In a trusted project, `PATH` starts with `<root>/node_modules/.bin`.
On Unix, each command and LSP server runs in its own process group. On timeout or cancel, Lens kills the group.
Lens skips files outside the current directory, after it resolves symlinks.
`rustfmt` uses the edition from the nearest `Cargo.toml`: `package.edition`, or the workspace root's `workspace.package.edition` when the package sets `edition.workspace = true`. The default is 2021. Lens reads one-line `key = value` entries only.
Detected tools: Rust `rustfmt`, `cargo check`, `cargo clippy`; JavaScript `prettier`, `eslint`, `tsc`; Python `ruff format`, `ruff check`; Go `gofmt`, `go vet`.
JavaScript tools need a matching dependency or config file. Rust and Go need their project manifests.
Missing tools, failed commands and timeouts produce findings.
Lens does not install tools.

CLI flags: `--no-lens`, `--no-lens-context`, `--no-read-guard`.
`PI_LENS_NO_CONTEXT_INJECTION=1` disables context injection.
Tool text and context injection are capped at 40 KiB.
LSP servers run for each diagnostic request and close after the response or timeout.

## Limits

Lens returns from every hook and tool within a fixed time. A failure becomes a warning finding or a log line, and the tool call continues.

| Limit | Value | Setting |
| --- | --- | --- |
| One command or LSP server | 30 s | `timeoutMs` |
| All steps after one `write` or `edit` | 30 s | `pipelineTimeoutMs` |
| `tool_result` hook | `pipelineTimeoutMs` + 6 s | `pipelineTimeoutMs` |
| `tool_call` read guard: file read and state lock | 2 s each | Fixed |
| Each call from a hook to pi (session entries, widget, notice, trust query) | 2 s | Fixed |
| LSP server after `shutdown` and `exit` | 0.5 s | Fixed |
| Output kept from each command stream | 1 MiB | Fixed |
| Wait for output pipes after a command exits | 0.5 s | Fixed |
| LSP header line | 8 KiB | Fixed |
| LSP message | 16 MiB | Fixed |
| File sent to an LSP server | 4 MiB, regular files only | Fixed |
| File checked by the read guard | 2 MiB, regular UTF-8 files only | Fixed |
| Findings from one command or server for one file | 200, plus one count of the rest | Fixed |
| Length of one finding message | 4 KiB | Fixed |

- Scans run one at a time. A scan that waits longer than `pipelineTimeoutMs` for its turn adds one skip finding.
- An LSP server that times out after at least 90% of the smaller of `timeoutMs` and `pipelineTimeoutMs` is skipped for the rest of the session. A session start, branch change, compaction or `/lens` change resets this.
- When a command exits but a child process keeps its output open, Lens kills the command's process group after 0.5 s.
- After a response, Lens kills the LSP server's process group, also when the server ignores `exit`.
- When pi stops the extension, Lens kills all process groups that are still running. When the extension process is killed with SIGKILL, LSP servers get `processId` in `initialize` and stdin EOF.
- Commands get stdin from `/dev/null` and `GIT_TERMINAL_PROMPT=0`. A command that reads `/dev/tty` runs in a background process group, so it stops until the timeout kills it.
- The read guard does not block FIFOs, devices, files larger than 2 MiB or non-UTF-8 files.
- An LSP message with a header line longer than 8 KiB, no `Content-Length`, or invalid JSON ends that request with a warning finding.

Checks use fake tool binaries, fake stdio LSP servers that stall, crash, flood or send malformed output, the in-memory native protocol, and pi with a local mock model.
