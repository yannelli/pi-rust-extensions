# pi-ext-observational-memory

Created 2026-10-30. Last updated 2026-10-30.

A Rust port of the pi package `pi-observational-memory` 3.1.4 for pi-rust native extensions. Run it as `pi-rust-ext observational-memory`.

During a session, three background workers keep a memory ledger in custom session entries:

1. The observer turns new conversation into timestamped, rated observations.
2. The reflector turns active observations into durable reflections.
3. The dropper removes observations from the active pool when the pool is over its target.

When pi compacts, the extension replaces pi's summary with the memory. The `recall` tool returns the source entries behind one memory id.

## Triggers

- `agent_start`, `turn_end`: when the session grew by `observeAfterTokens` since the last observation, or by `reflectAfterTokens` since the last reflection, the pipeline runs in a background task. Growth comes from the provider-reported context tokens when pi can measure it, else from a 4 characters per token estimate. One pipeline runs at a time. The dropper runs only after the reflector recorded reflections in the same run.
- `agent_settled`: when the estimated source tokens since the last compaction reach the threshold, the extension calls `ctx.compact` (when pi is idle).
- `session_before_compact`: returns `{ compaction: { summary, firstKeptEntryId, tokensBefore, details } }`. The summary starts with "These are condensed memories from earlier in this session." and has `## Reflections` and `## Observations` sections with `[id] timestamp [relevance] text` lines. With no memory, it returns nothing and pi's summarizer runs. A second compaction during a running one gets `{ cancel: true }`.

## Tool

`recall` (label "Recall memory evidence"). Parameter `id`: a 12-character lowercase hex memory id (`^[a-f0-9]{12}$`). It returns the observation or reflection, its supporting observations, and the raw source entries on the current branch. Other results: `No observation or reflection with id <id> was found on the current branch.` and `Memory id must be 12 lowercase hex characters. Received: <id>`. The prompt snippet and the 6 prompt guidelines are the upstream text.

## Commands

| Command | Does |
| --- | --- |
| `/om:status` | Shows recorded, dropped, active and visible counts, the token progress to each threshold, the pool sizes, running work and the last worker errors. |
| `/om:view [full]` | Shows the visible memory (the memory in the last compaction) or, with `full`, all recorded memory. It also copies the text to the clipboard. |

## Configuration

Sources, in order; a later source wins per key:

1. Defaults.
2. `<agentDir>/ext/observational-memory.json` (global).
3. `<cwd>/.pi/ext/observational-memory.json` (project). The extension reads it only when pi reports the project as trusted, because `model` selects the provider that receives the transcript. It shows one warning when it ignores the file.
4. The `observational-memory` key of pi settings (`~/.pi/agent/settings.json` and `.pi/settings.json`, merged by pi). This is where upstream reads its settings.
5. `PI_OBSERVATIONAL_MEMORY_PASSIVE` (`1`/`true`/`yes`/`on` or `0`/`false`/`no`/`off`) sets `passive`.

The extension ignores a value of the wrong type or range and keeps the earlier value.

| Key | Default | Meaning |
| --- | --- | --- |
| `observeAfterTokens` | `10000` | Session growth that starts the observer. |
| `reflectAfterTokens` | `20000` | Session growth that starts the reflector. |
| `observerChunkMaxTokens` | unset | Maximum estimated tokens per observer chunk. Unset: 0.2 x the memory model context window (minimum 256), or 60000 when the window is unknown. |
| `compactAfterTokens` | `81000` | Estimated source tokens since the last compaction that start a compaction. |
| `compactAfterTokensMode` | `"calibrated"` | `"calibrated"` uses `compactAfterTokens`. `"ratio"` uses `floor(contextWindow x compactAfterTokensRatio)`, and `compactAfterTokens` when the window is unknown. |
| `compactAfterTokensRatio` | `0.68` | Ratio for `"ratio"` mode, between 0 and 1. |
| `observationsPoolMaxTokens` | `20000` | When the observations in a compaction reach this size, the compaction folds all memory (`fullFold`). |
| `observationsPoolTargetTokens` | `10000` | Active pool target for the dropper. It must be less than the maximum; else it is half of the maximum. |
| `agentMaxTurns` | `16` | Accepted for compatibility. Each worker makes one completion (see the differences). |
| `agentMaxTokens` | `32000` | Maximum output tokens per worker completion, limited to the model `maxTokens`. |
| `model` | unset | `{ provider, id, thinking? }`: the model for the workers. Unset or not found: the session model. `thinking` (default `"low"`) goes to the model only when the model supports reasoning. |
| `showWorkerNotifications` | `true` | Shows worker progress notifications. Worker errors always show. |
| `passive` | `false` | Stops the workers and the automatic compaction. Compaction summaries, commands and `recall` stay on. |
| `debugLog` | `false` | Writes an NDJSON debug log. |

## Paths

- Debug log: `<agentDir>/ext/observational-memory/debug/<sessionId>.ndjson`, or `<agentDir>/ext/observational-memory/debug.ndjson` when there is no session id. At 10 MB the file moves to `.ndjson.1`.
- Clipboard: a temporary file in `$TMPDIR/pi-observational-memory/`, removed after the copy.

## Session entries

Custom entries (`session.appendEntry`), with the upstream data shapes:

| `customType` | `data` |
| --- | --- |
| `om.observations.recorded` | `{ observations: [{ id, content, timestamp, relevance, sourceEntryIds, tokenCount }], coversUpToId }` |
| `om.reflections.recorded` | `{ reflections: [{ id, content, supportingObservationIds, tokenCount }], coversUpToId }` |
| `om.observations.dropped` | `{ observationIds, coversUpToId }` |

Compaction `details`: `{ type: "om.folded", version: 1, fullFold, observations, reflections }`. A memory id is the first 12 hex characters of the SHA-256 of the content. `relevance` is `low`, `medium`, `high` or `critical`.

## Differences from upstream

- Upstream runs each worker as an agent loop with the tools `record_observations`, `record_reflections` and `drop_observations`. `llm.complete` has no tool calls, so each worker makes one completion that returns one JSON object (`{"observations": [...]}`, `{"reflections": [...]}`, `{"ids": [...]}`). The system prompts are the upstream text; only the lines about tool calls changed to describe the JSON reply. A reply that is not valid JSON gets one retry with a correction note. A second bad reply stops the run with a warning, and the coverage does not change.
- `agentMaxTurns` has no effect, because each worker makes one completion.
- Model and credential problems come from `llm.complete` as a worker error. Upstream checks the credentials before the call.
- pi has no custom tool renderers over the native protocol. `recall` returns plain text; the TUI shows no aligned rows and no collapsed view.
- The native `exec` call has no stdin. The clipboard copy writes the text to a temporary file and runs `sh -c '<command> < <file>'` (`cmd /C clip < <file>` on Windows).
- Upstream adds `x-opencode-session` headers for opencode providers. `llm.complete` takes no headers, so the extension does not send them.
- Config also comes from the two `ext/observational-memory.json` files. The project file needs a trusted project.
