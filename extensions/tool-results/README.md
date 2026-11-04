# tool-results

Created 2026-11-04. Last updated 2026-11-04.

Shrinks tool results before the model reads them. It removes terminal noise, converts JSON results to TOON or other compact forms, and stores large results behind a retrieval tool. Each feature can be turned on or off globally and for each tool.

Run it as `pi-rust-ext tool-results`. It needs pi-rust 0.8.0 or later, which applies the `tool_result` patches of native extensions.

## How it works

The extension handles the `tool_result` event of pi. When a result gets smaller, it answers with a merge patch `{ "content": [...], "details": { "toolResults": {...} } }`, and pi replaces the content of the result and adds the record to its `details`. pi keeps the tool's own `details` keys and `structuredContent`. The transcript in the TUI shows the shortened content, and an expanded row can show the full text (see [Expanded rows](#expanded-rows)). Tools can cut their own output first: pi's `bash` keeps the last 2,000 lines or 50 KB and names a log file. The extension works on what the tool returns.

Each text block goes through these steps, in this order. Each step runs only when its key is on.

1. `ansi`: removes ANSI escape sequences (colors, cursor moves, OSC 8 links).
2. `progress`: applies `\r` and backspace as a terminal does, drops lines that hold only a spinner frame, and keeps the last line of each run of updates of one progress bar.
3. `control`: removes NUL and the other control characters. `\n` and `\t` stay.
4. `json`: when the whole block is one JSON object or array (also in a code fence), converts it to the selected format. The original stays when the new form is not shorter. A JSON block gets no other text steps.
5. `base64`: replaces data URI payloads and base64 or hex runs of `base64Min` characters or more with their size.
6. `stackTraces`: folds frames that repeat (recursion) and keeps the first and last frames of a trace longer than `stackMaxFrames`. It knows JavaScript, Java, and C# `at ...` frames, Python `File "...", line N` frames, and Rust backtraces.
7. `dedupe`: collapses a line, or a block of up to 4 lines, that repeats `dedupeMin` times or more in a row into one copy and `[previous line repeated N more times]`. It does this only when the result is shorter.
8. `longLines`: clips lines longer than `longLineChars`, such as minified code.
9. `collapseSpaces`: collapses runs of spaces inside lines. Indentation stays.
10. `whitespace`: removes trailing spaces and keeps at most one blank line in a row.

Then, when the text is longer than `threshold` characters, the extension stores the full text and replaces the result with the first `headChars` and last `tailChars` characters and this notice:

```text
Result truncated (121k characters, 3,210 lines). Use `tool_result_get` with result_id "tr-1a2b3c4d" to read a line range, search for a pattern and return matches with about 200 characters of context, or return it in full.
```

The stored text is the result after steps 1 to 3, so it has no lossy changes. When steps 5, 6, or 8 removed content from a result under the threshold, the extension also stores it and adds a short note with the id (`keepElided`).

Rules that always apply:

- The results of `tool_result_get` never change.
- Image blocks never change. When a result is truncated, its images follow the preview.
- Error results (`isError`) get only steps 1 to 3, 9, and 10 by default (`errors: "clean"`). Use `"all"` for every step or `"off"` to keep them as they are.
- Results that do not get shorter stay as they are.

## The `tool_result_get` tool

| Argument | Use |
| --- | --- |
| `result_id` | The id from the notice. Required. |
| `mode` | `lines` (default), `search`, or `full`. |
| `start`, `end` | `lines`: 1-based and inclusive. The default is 200 lines from `start`. |
| `pattern`, `regex`, `ignore_case` | `search`: literal text, or a regular expression with `regex: true`. |
| `context_chars`, `max_matches` | `search`: characters on each side of a match (default `contextChars`) and the most matches (default `maxMatches`). Matches with overlapping context merge. |

## Stored results

Each changed result is stored in `<agentDir>/ext/tool-results/store/` as `<id>.txt` (the full text after steps 1 to 3) and `<id>.json` (tool, tool call id, session id and file, size, SHA-256, time). The id comes from the tool call id and the hash of the result, so a lookup needs only the id. Results therefore keep working after a resume, a fork (which gets a new session id), `/tree` navigation, a pi restart, and `/reload`. With `expandOriginal` and `embedOriginal` both off, only truncated results and results with elided parts are stored.

The record `details.toolResults` of the result (`resultId`, `chars`, `lines`, `hash`) goes into the session file. When the store no longer has a result, `tool_result_get` finds the record and says that the retention policy or a purge removed it. Files are written to a temporary file, synced, and renamed, and a result counts only once its `<id>.json` exists, so a crash never leaves a half-written result.

At each session start, a background task applies the retention policy without delaying the session:

- `retentionDays` (default 30) removes older results. With `keepWhileSessionExists` (default on), results whose session file still exists skip this rule.
- `retentionMaxMb` (default 1024) removes the oldest results until the store fits. This rule applies to every result.
- Temporary files and texts without a record that are older than 1 hour are removed.

`embedOriginal` (default off) also writes the full text into `details.toolResults.original`. The session file then has everything it needs, and `tool_result_get` and the expanded view work even after the store is gone. The cost: each changed result adds its full size to the session file, so a session with many large outputs grows by megabytes, and pi reads that text again on each resume. The model never gets `details`.

## Expanded rows

The existing expand key of pi (`app.tools.expand`, `ctrl+o` by default) or a click opens a tool row. For a changed result, the open row shows the full text from the store (or from `details`) in place of the shortened text, with a header line. The text never goes to the model. The view shows at most `expandedMaxHeight` lines (default 30) with a position line such as `lines 1-29 of 300`, and the mouse wheel over it scrolls it in fullscreen mode. The same key collapses the row, and the view returns to the top.

This needs a pi-rust with the tool result expander (feature `toolResultExpander`, [yannelli/pi-rust#229](https://github.com/yannelli/pi-rust/pull/229)). Older hosts ignore it, and an expanded row shows the shortened content as before. Turn `expandOriginal` off to keep pi's own expanded rows.

## Commands

- `/tool-results` opens the panel. Without a UI it prints the status.
- `/tool-results status` shows every setting and the rules.
- `/tool-results preview` shows the last changed result, also in dry run.
- `/tool-results stats` shows the characters saved per tool, for this session and in total.
- `/tool-results list` shows the stored results, newest first, with their size and age.
- `/tool-results purge <result_id>`, `purge session` (the results of this session), and `purge all` (asks first) remove stored results.
- `/tool-results reset` clears the stats.

The panel has 3 tabs. **Settings** has a row for each key: `←` `→` or space change a switch or a choice, and enter edits a number. **Rules** lists the per-tool rules: enter opens a rule, `a` adds one, and `d` deletes one. In a rule, each field can be `inherit`, or a value for the tools that match. **Stats** shows the calls, changed results, stored results, and characters saved per tool, and the last change. `r` clears the stats. The panel writes to `<agentDir>/ext/tool-results.json`.

## Config

`<agentDir>/ext/tool-results.json`. A trusted project can add `<cwd>/.pi/ext/tool-results.json`, which is merged over the global file (RFC 7386). Changes apply at the next session start or `/reload`, and at once from the panel.

| Key | Default | Use |
| --- | --- | --- |
| `enabled` | `true` | All transforms. |
| `dryRun` | `false` | Measure the savings, but do not change results. Global only. |
| `defaultRules` | `true` | The built-in rule for `read` (below). Global only. |
| `errors` | `"clean"` | `clean`, `all`, or `off` for error results. |
| `ansi`, `progress`, `control`, `whitespace` | `true` | Steps 1, 2, 3, and 10. |
| `collapseSpaces` | `false` | Step 9. |
| `dedupe`, `dedupeMin` | `true`, `3` | Step 7. |
| `base64`, `base64Min` | `true`, `256` | Step 5. |
| `stackTraces`, `stackMaxFrames` | `true`, `30` | Step 6. |
| `longLines`, `longLineChars` | `true`, `2000` | Step 8. |
| `json` | `"toon"` | `toon`, `compact` (minified), `stripped` (minified, without null and empty fields), `table` (CSV for arrays of objects with one key set, else minified), or `off`. |
| `jsonStrip` | `false` | Drop null, `""`, `[]`, and `{}` fields before any format. |
| `jsonDropDefaults` | `false` | With stripping, also drop `false` and `0`. |
| `truncate` | `true` | Store large results. |
| `threshold` | `20000` | Characters before a result is stored. |
| `headChars`, `tailChars` | `4000`, `2000` | The preview. |
| `keepElided` | `true` | Store results that lost content in steps 5, 6, or 8. |
| `contextChars`, `maxMatches` | `200`, `20` | Defaults of `search`. Global only. |
| `expandOriginal` | `true` | Store every changed result and show it in an expanded row. Global only. |
| `expandedMaxHeight` | `30` | Lines of the expanded view, with the position line. Global only. |
| `embedOriginal` | `false` | Also keep the full text in the session file (see above). Global only. |
| `retentionDays` | `30` | Days to keep stored results; `0` keeps them. Global only. |
| `retentionMaxMb` | `1024` | Size limit of the store in MB; `0` sets none. Global only. |
| `keepWhileSessionExists` | `true` | Results of an existing session file skip `retentionDays`. Global only. |
| `tools` | `{}` | Per-tool rules: a tool name or glob (`*`, `?`) and the keys it sets. |

Rules apply in file order after the global keys, and later rules win. The built-in rule for `read` turns off every text step and `json`, because the model edits file text by exact match. Only the storage of large results applies to `read`.

```json
{
  "threshold": 30000,
  "json": "toon",
  "tools": {
    "mcp__*": { "json": "table", "threshold": 10000 },
    "bash": { "collapseSpaces": true },
    "web_fetch": { "enabled": false }
  }
}
```

Stats are in `<agentDir>/ext/tool-results/stats.json`.

## TOON

The TOON encoder follows [spec v4.4](https://github.com/toon-format/spec) (comma delimiter, 2-space indent). It writes objects as `key: value` lines, primitive arrays inline (`tags[3]: a,b,c`), uniform arrays of objects as tables (`items[2]{id,name}:` and one row per line), nested field groups (`customer{name,country}`), objects of uniform objects in the keyed form (`users[2:]{age,city}:`), and other arrays as `- ` lists. Strings are quoted only as section 7.2 requires. The unit tests use the examples of Appendix A.

## Prior art

Methods for smaller tool results, and what this extension takes from them:

- **TOON.** An indentation-based form of the JSON data model that declares array lengths and the fields of uniform arrays once. The project reports about 40% fewer tokens than JSON on its own benchmark. For deep, non-uniform data, minified JSON can be smaller, so the extension keeps the shorter form. [spec](https://github.com/toon-format/spec), [reference implementation](https://github.com/toon-format/toon)
- **Compact JSON and tables.** Minifying and dropping empty fields saves 20% to 60%, depending on the data and the tokenizer. CSV is the most compact form for flat tables, but cannot nest. [Optimizing JSON for LLMs](https://dev.to/mattlewandowski93/optimizing-json-for-llms-1dgf), [ImprovingAgents format tests](https://www.improvingagents.com/blog/best-nested-data-format)
- **Anthropic guidance.** Tools should return only what the agent needs, with pagination, filtering, and truncation that have sensible defaults, and a concise response format. Context editing (`clear_tool_uses`) replaces old tool results with a placeholder, and the memory tool keeps data out of the context. [Writing effective tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents), [context editing](https://platform.claude.com/docs/en/build-with-claude/context-editing), [memory tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool)
- **Claude Code.** A Bash result over about 30,000 characters (`BASH_MAX_OUTPUT_LENGTH`) becomes a saved file path and a preview. MCP output gets a warning at 10,000 tokens and a cap at 25,000 tokens (`MAX_MCP_OUTPUT_TOKENS`). [tools reference](https://code.claude.com/docs/en/tools-reference), [environment variables](https://code.claude.com/docs/en/env-vars), [MCP](https://code.claude.com/docs/en/mcp)
- **Codex CLI.** Exec output was limited to 10 KiB or 256 lines. A head and tail split was proposed to keep the error at the end. Current builds cut the middle of the output to `tool_output_token_limit` tokens. [PR 6476](https://github.com/openai/codex/pull/6476), [issue 7867](https://github.com/openai/codex/issues/7867)
- **Truncation with a retrieval handle.** OpenCode and Copilot CLI write large output to a file and point the agent to it. Masking old observations made SWE-agent runs cheaper at a similar solve rate. [OpenCode PR 8953](https://github.com/anomalyco/opencode/pull/8953), [Copilot CLI issue 1732](https://github.com/github/copilot-cli/issues/1732), [The Complexity Trap](https://arxiv.org/abs/2508.21433)
- **Text cleanup.** ANSI stripping ([strip-ansi](https://github.com/chalk/strip-ansi)), the last segment after `\r` for progress bars, collapsed repeats, elided base64 blobs, and folded stack frames ([Dart `foldFrames`](https://api.flutter.dev/flutter/package-stack_trace_stack_trace/Trace/foldFrames.html)).
- **Token-reduction tools.** [RTK](https://github.com/rtk-ai/rtk) filters and groups the output of common dev commands. [Headroom](https://github.com/chopratejas/headroom) compresses by content type and keeps originals for retrieval. [context-mode](https://github.com/mksglu/context-mode) runs commands in a sandbox and indexes their output. [LLMLingua](https://github.com/microsoft/LLMLingua) removes low-information tokens with a small model.
