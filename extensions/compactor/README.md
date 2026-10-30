# compactor

Created 2026-10-30. Last updated 2026-10-30.

Rust port of `@pi-unipi/compactor` 3.0.0 for pi-rust (`pi-rust-ext compactor`). The extension replaces pi's compaction summary with one of two methods:

- `vcc`: a lossless, zero-LLM structured summary. The extension rebuilds it from the whole session branch on each compaction. It does not merge earlier summaries. The sections are Active Work, Your Requests, Latest State, Decisions & Constraints, Files, Lessons, Project Knowledge, Commits, Open Errors, and Recent Transcript. The extension removes secrets from the summary text only. `session_recall` still finds the original text.
- `llm`: a model-written summary through `llm.complete` with pi's summarization prompt and credentials, and the `[Active Work]` header on top.

## Tools

| Tool | Parameters | Result |
| --- | --- | --- |
| `session_recall` (Session Recall) | `query?` (keywords or regex; `#N:path`, `#N:path:full`, `#N:path:<offset>[:<limit>]` drill into file content of entry N), `expand?: number[]`, `page?: number >= 1`, `scope?: "lineage" \| "all"`, `mode?: "hybrid" \| "touched"` | Paged text (5 hits per page), `details: { query }` |
| `context_budget` (Context Budget) | none | Percent full, tokens left, and what triggers the next compaction |

## Commands

| Command | Does |
| --- | --- |
| `/unipi:compact-vcc [keep:N]` | Lossless compaction now. `keep:N` keeps the last N user turns. |
| `/unipi:compact-by-llm [focus]` | Model summary now. The text focuses the summary. |
| `/unipi:compact-then [prompt]` | Compacts, then sends the prompt. The `input` hook catches it, so it is not a registered command. When idle, it compacts at once. During a run it compacts at the next turn boundary (steer) or after the run settles (follow-up). |
| `/unipi:session-recall <query> [scope:all] [page:N]` | Shows recall results and sends them to the agent (`compactor-recall` message, `triggerTurn`). |
| `/unipi:compact-stats` | Compactions on this branch and tokens saved. |
| `/unipi:compact-doctor` | Checks the settings, pi's `compaction.enabled`, and the old `~/.unipi/db/compactor` folder. |
| `/unipi:compact-help` | Command list. |

## Method selection

1. A pending command (`/unipi:compact-vcc`, `/unipi:compact-by-llm`).
2. The `__compactor__` marker in `customInstructions` selects `vcc`.
3. For manual `/compact`: `piCompact`, when it is not `follow`.
4. `method`.

When the vcc plan cannot cut (no messages, or 2 or fewer), the extension cancels the compaction. An `overflow` compaction goes to pi's summarizer instead. When the `llm` call fails, or the preparation is a split turn, pi's own summarizer runs.

## Events

`session_before_compact`, `session_compact` (appends a `unipi-compaction` card entry when `notify` is on or a command ran), `context` (removes the old `compactor-auto-continue` and `unipi-compactor-resume` custom messages), `input` (origin marks for extension-sent user text, `/unipi:compact-then`), `turn_end` (the `percent` trigger and steered compact-then as boundary compaction drafts), `agent_end` and `agent_settled` (follow-up compact-then), `session_start` (state reset).

Other extensions can add `[Active Work]` blocks: emit `{ "id": "<id>", "text": "<text>" }` on the `pi.events` channel `compactor:active-work`. Empty text removes the block. After each compaction the extension emits `unipi:compactor:compacted` with `{ method, summarized, kept, tokensBefore, tokensAfter, tokensSaved }`.

## Config

Paths: global `<agentDir>/ext/compactor.json`, project `<cwd>/.pi/ext/compactor.json`. The extension deep-merges the two files. A project key replaces the same global key.

| Key | Default | Meaning |
| --- | --- | --- |
| `method` | `"vcc"` | `vcc` or `llm` for automatic compactions |
| `piCompact` | `"follow"` | What pi's `/compact` does: `follow`, `vcc`, or `llm` |
| `trigger` | `"pi"` | `pi` (pi's own limit) or `percent` |
| `thresholdPercent` | `80` | Context percent for `trigger: "percent"` (upstream range 30 to 95) |
| `notify` | `true` | Card entries and notices for automatic compactions |
| `smartKeepTail` | `true` | Keeps more turns while the kept tail is 5k tokens or less (up to 25k) |
| `summaryBudgetTokens` | `0` | Summary size; `0` scales from 1.5k to 4k with the history |
| `summarySections` | all 9 ids | `activeWork`, `requests`, `state`, `decisions`, `files`, `commits`, `errors`, `lessons`, `transcript` |
| `cooldownMs` | `60000` | Minimum time between `percent` triggers |
| `repeatMinGrowthTokens` | `4000` | Growth needed to trigger again while usage stays above the threshold |
| `llmInstructions` | `""` | Extra instructions for model summaries |
| `debug` | `false` | Writes the last plan to `compactor-debug.json` in the temp directory (also `UNIPI_COMPACTOR_DEBUG=1`) |

Legacy keys: `method: "jev"` reads as `vcc`. `overrideDefaultCompaction: false` sets `method: "llm"`. `autoCompaction.enabled: true` sets `trigger: "percent"`. `autoCompaction.{thresholdPercent, notify, cooldownMs, repeatMinGrowthTokens}` fill the same keys. A legacy `sections` object of booleans sets `summarySections` when the layer has no `summarySections` array. These keys fill only keys that the layer does not set.

## Differences from upstream

- Config files are at the paths above. The extension does not read `~/.unipi/config/compactor/config.json` and does not register with the unipi settings hub (`/unipi:settings`). It does not write migrated keys back to the files.
- `cooldownMs`, `repeatMinGrowthTokens`, and `debug` are config keys. Upstream 3.0.0 has fixed values for the first two.
- Origin marks (`compactor-origin`) use a 64-bit FNV-1a key. Marks that upstream wrote (SHA-256) do not match, so the known-pattern filter handles those sessions.
- Recall hits show the session entry index (`#N`), which `expand` and drill-down accept. Upstream shows the block index. `expand` returns all blocks of an entry. `scope: "all"` searches `session.getEntries` (all branches).
- Regex queries use the Rust `regex` crate: no lookaround or backreferences (such queries search as literal text), and no search time limit is necessary.
- Active work comes from the `compactor:active-work` channel. Upstream reads goal, ralph, and kanboard providers from `@pi-unipi/core`.
- Not available over the native protocol: the `unipi-compaction` entry renderer (the card entry is data only), the info-screen group, the unipi bus `MODULE_READY` event, the `compactor` skill directory, and the `tool_result` diff width clamp (the extension does not know the terminal width). The `/unipi:compact-then` autocomplete item shows only after `/unipi:compact-t` or `/compact-t`, because a native suggestion list replaces pi's list.
