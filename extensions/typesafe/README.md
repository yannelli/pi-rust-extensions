# pi-ext-typesafe

Created 2026-10-30. Last updated 2026-10-30.

A native pi extension that sends structured questions to TypeSafe AI Jev and returns probabilities. It is a Rust port of the npm package `pi-typesafe` 0.13.1. The tool name, parameters, command, environment variables, and key file are the same as the package.

## Tool

`typesafe_evaluate` (label `TypeSafe`) sends one request with 1 to 32 questions about a `state` value:

| Type | Question | Answer |
| --- | --- | --- |
| `choice` | Which label fits? `criteria` maps 1 to 64 labels to descriptions. | `choice`, `confidence`, `probabilities` |
| `score` | Where on an ordered rubric? `criteria` is a list of 2 to 32 levels. | `score`, `confidence`, `probabilities`, `legend` |
| `noul` | Is the statement true? `criteria` is optional `{ true, false }`. | `noul` (probability of yes) |

- `state` is a string, an array, or an object. It cannot be null. `model` is optional (1 to 100 characters).
- Tool calls must use the schema form of `criteria`. pi checks the arguments against the schema before the extension gets them, so pi refuses an alias.
- The client and the playground change these aliases before they check a request: `options`, `levels`, or `choices` in place of `criteria`; a list of labels for a `choice`; a string for a `noul`.
- Limits: 64 KiB of JSON, 20 attempts in each session, a 15 second timeout, and no retries. SubQ accepts at most 16 questions, 2 or more choice options, and 10 or fewer score levels.
- The result text is the JSON answer. `details` holds the same answer and `elapsedMs`.
- The tool stays registered when it is disabled. A call then fails and tells the agent to ask the operator.

## Command

`/typesafe <action> [backend]`. The default action is `status`.

| Action | Result |
| --- | --- |
| `status` | Shows the gate, backend, key state, session and daily counters, caps, and the consent text. |
| `login [backend]` | Asks for the key in a hidden input panel. Verifies the key with the model list, or with one judge request on backends whose model list is public. Then saves the key. |
| `logout [backend]` | Removes the stored key. For the active backend, when no key variable is set, it also disables the tool for this session. |
| `setup` | Starts `login` when no key is set. Otherwise it shows the key source. |
| `enable` | Asks for consent, then enables the tool for this session. |
| `disable` | Disables the tool for this session. Requests that started continue. |
| `test` | Sends the sample request after you confirm. |
| `playground` | Opens the sample request in the editor, then sends your edited JSON after you confirm. |

Completions give the action names, and the backend names after `login ` and `logout `. Without a UI, the command writes a `typesafe-status` message. `login`, `setup`, `enable`, `test`, and `playground` need a UI. `test` and `playground` save each result as a `typesafe-result` session entry and show it in a notification.

## Session gate

The tool is disabled until the operator enables it. At each `session_start` the extension:

1. Reads `PI_TYPESAFE_ENABLED` (`1` enables). When the variable is not set, it reads `enabled` from the trusted project file, then from the global file.
2. Removes the client. This sets the session counters to zero.
3. Shows one warning when the backend name is not valid, or when the tool is enabled and the key is missing, unusable, or rejected.

A 401 or 403 response or a configuration error during a tool call shows one warning for each different failure.

## Configuration

Environment variables take precedence over the config files.

| Variable | Config key | Default | Use |
| --- | --- | --- | --- |
| `PI_TYPESAFE_ENABLED` | `enabled` | off | `1` enables the tool for each session. |
| `PI_TYPESAFE_BACKEND` | `backend` | `typesafe` | `typesafe`, `openrouter`, `commandcode`, `liquid`, or `subq`. |
| `PI_TYPESAFE_MODEL` | `model` | backend default | Model id. OpenRouter changes `jev-latest` and `jev-1.13` to its own ids. |
| `PI_TYPESAFE_MAX_REQUESTS_PER_DAY` | `maxRequestsPerDay` | none | Daily request cap. |
| `PI_TYPESAFE_MAX_INPUT_TOKENS_PER_DAY` | `maxInputTokensPerDay` | none | Daily input token cap. |
| `PI_TYPESAFE_MAX_USD_PER_DAY` | `maxUsdPerDay` | none | Daily cost cap in USD. |
| `PI_TYPESAFE_BASE_URL` | none | registry host | Replaces the backend host. Use an https origin, or http on localhost, 127.0.0.0/8, or [::1]. Tests and local gateways use it. |

- Global config: `<agentDir>/ext/typesafe.json`. Project config: `<cwd>/.pi/ext/typesafe.json`.
- The extension reads `backend` and `model` from the global file one time, when it starts. These values set the request host and the tool description.
- The project file sets `enabled` only when pi trusts the project. When pi does not trust the project, the extension ignores that key and shows one notice. Caps from the environment, the global file, and the project file all apply. The lowest cap wins.
- API keys: `TYPESAFE_API_KEY`, `OPENROUTER_API_KEY`, `COMMANDCODE_API_KEY`, `LIQUID_API_KEY`, `SUBQ_API_KEY`. A key variable takes precedence over a stored key. `login` does not run when the variable is set.

## Files

All files are in `<agentDir>/pi-typesafe/`, the same directory that the npm package uses. Each file has mode 0600 and the directory has mode 0700. The extension writes each file to a temporary name, then renames it.

| File | Contents |
| --- | --- |
| `auth.json` | `{ "version": 1, "keys": { "<backend>": "<key>" } }`. The extension reads the old `{ "apiKey" }` shape as the `typesafe` key. When group or other users can read the file, the extension does not read it and reports the key as unusable. |
| `auth-state.json` | The time of the last verification and the last failure (code, message, time, status). |
| `usage.json` | Daily counters: requests started, succeeded, and failed; input and output tokens; cost. The day changes at local midnight. The extension keeps 31 days. |
| `usage.json.lock` | The lock for `usage.json`. An update waits at most 200 ms for it, and takes over a lock that is older than 5 seconds. When the lock is busy, the update stays in memory until the next update. |

Cost is input tokens multiplied by the backend price for the model sent: Liquid `d1` $0.04 and SubQ $0.05 for each million tokens. Other models use $0.042 for each million tokens.

Error messages do not include keys, response bodies, or the state that you send. A 429 message includes the `Retry-After` value only when it is a number of seconds.

## Differences from the npm package

- The native protocol does not support `prepareArguments`. Thus pi refuses a tool call that uses an alias. When the protocol adds `prepareArguments`, tool calls can use the aliases again.
- The native protocol does not support `renderCall`, `renderResult`, or `registerEntryRenderer`. The tool sends one `tool.update` line (`TypeSafe · N questions · external request`). `test` and `playground` show the formatted result in a notification.
- The hidden key input is a `ui.custom` panel. When pi cannot open the panel, the extension uses `ui.input`, which shows the key while you type.
- The config files and `PI_TYPESAFE_BASE_URL` are new in this port.

## Tests

`cargo test -p pi-ext-typesafe` runs unit tests, protocol tests against an in-memory host, and tests against a mock API server on 127.0.0.1 (`mock_api`). No test uses the network outside localhost or a real key.
