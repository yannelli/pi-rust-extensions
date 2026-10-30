# context-include

Created 2026-10-29. Last updated 2026-10-29.

Rust port of `@d3ara1n/pi-context-include` v3.0.0. Lines such as `@docs/api.md` in loaded context files (`AGENTS.md`) add the referenced Markdown files to the system prompt, recursively.

Run it as `pi-rust-ext context-include`.

## Behavior

- On each `before_agent_start`, the extension scans `systemPromptOptions.contextFiles` and sets `systemPromptOptions.sections.context_include`. When no file resolves, it removes the section.
- A reference is a whole line `@<path>.md`, after trim and after an optional list marker (`-`, `*`, `+`, `1.`) and whitespace. The path matches `^@([~.]?[\w./-]+\.md)$`. Lines inside ```` ``` ```` fences are ignored. Duplicate references in one file are dropped.
- `~` expands to `$HOME`. Relative paths resolve against the realpath directory of the file that contains the reference.
- Files are deduplicated by realpath, so cycles and symlinks to the same file load one time. Context files count as visited.
- Fence: a target under a denied root is skipped, then a target outside every allowed root is skipped. The checks run before the file is read.
- Size: `maxBytes` is a running total of UTF-8 bytes. A file that goes over it is skipped; later smaller files still load.
- Depth: context files are depth 0. Files at depth `maxDepth + 1` load, but their references are not scanned.
- Skip reasons: `already included`, `denied by deniedRoots`, `outside allowed roots`, `file not found`, `read error: ...`, `file is empty`, `size limit exceeded (would reach N bytes)`, `max depth (N) exceeded`.
- Output: `Additional files included via @-syntax references in the project instructions:` then one `<project_instructions path="...">` block per file. `path` is the resolved path before realpath.

## Commands

- `/context-include:status`: an info notification with `maxDepth`, `maxBytes`, the effective allowed and denied roots, the context files (`ok`, `not found`, `ERROR`), the included files with sizes, and the skipped references with reasons. Before the first scan it shows `No scan data yet`.

No tools.

## Config

| Key | Default | Use |
| --- | --- | --- |
| `maxDepth` | `10` | Recursion limit. |
| `maxBytes` | `500000` | Total size limit in UTF-8 bytes. |
| `allowedRoots` | unset | Replaces the default allow roots. `[]` blocks all includes. |
| `deniedRoots` | `[]` | Roots that are always blocked. |

Invalid numbers (negative, non-finite, not a number) use the defaults. Arrays that hold a non-string are ignored. Roots expand `~` and resolve relative paths against the session cwd. Default allow roots: `<agentDir>/includes` and the directory of each context file.

The extension reads config on `session_start` only (`/reload` reads it again). It uses the first source that exists:

1. `<cwd>/.pi/ext/context-include.json` (fields at the top level).
2. The `contextInclude` setting (pi merges `<agentDir>/settings.json` and `<cwd>/.pi/settings.json`).
3. `<agentDir>/ext/context-include.json`.

A file that does not parse counts as `{}`. The chosen source replaces the others whole; fields do not merge across sources.

## Differences from upstream

- Upstream reads the `contextInclude` block from the two `settings.json` files itself, and a project block replaces the global block. This port gets the setting through `settings.get`, so the merge rule of pi applies. The two `ext/context-include.json` files are additions.
- `read error: ...` text comes from the Rust I/O error, for example `read error: Is a directory (os error 21)`.
- Relative context-file paths and the `max depth` display path use the session cwd in place of the process cwd.
- Invalid UTF-8 is replaced with U+FFFD before the size count, as Node `readFile(..., "utf-8")` does.
