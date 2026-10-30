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

| Key | Default | Use |
| --- | --- | --- |
| `enabled` | `true` | Runs the pipeline and read guard. |
| `context` | `true` | Adds findings to model requests. |
| `readGuard` | `true` | Requires a read before an edit. |
| `autoDetect` | `true` | Detects tools from language and project files. |
| `timeoutMs` | `30000` | Sets each command and LSP timeout, from 1 to 300000 ms. |
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
Children use the nearest project root and include `node_modules/.bin` on `PATH`.
Detected tools: Rust `rustfmt`, `cargo check`, `cargo clippy`; JavaScript `prettier`, `eslint`, `tsc`; Python `ruff format`, `ruff check`; Go `gofmt`, `go vet`.
JavaScript tools need a matching dependency or config file. Rust and Go need their project manifests.
Missing tools, failed commands and timeouts produce findings.
Lens does not install tools.

CLI flags: `--no-lens`, `--no-lens-context`, `--no-read-guard`.
`PI_LENS_NO_CONTEXT_INJECTION=1` disables context injection.
Tool text and context injection are capped at 40 KiB.
LSP servers run for each diagnostic request and close after the response or timeout.

Checks use fake tool binaries, a fake stdio LSP server, the in-memory native protocol, and pi with a local mock model.
