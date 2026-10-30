# ask-user-question

Created 2026-10-30. Last updated 2026-10-30.

Rust port of `@juicesharp/rpiv-ask-user-question`. The `ask_user_question` tool shows a tabbed questionnaire in a `ui.custom` panel and returns the answers to the model. Tool name, parameters, validation codes, result text, `details`, and `pi.events` channels match upstream, so prompts written for it keep working.

## Tool `ask_user_question`

Parameters: `questions` (1 to 4), each `{ question, header (max 16), options (2 to 4) of { label (max 60), description, preview? }, multiSelect? }`. The panel appends a `Type something.` row to every question and a `Next` row to multi-select questions. The labels `Other`, `Type something.`, and `Next` are reserved.

- Before validation the tool decodes `questions`, its items, and `options` when a model sent them as JSON strings, and normalizes `\r\n` to `\n` (a lone `\r` is removed).
- Validation errors return `details: { answers: [], cancelled: true, error }` with `error` one of `no_questions`, `too_many_questions`, `duplicate_question`, `empty_options`, `reserved_label`, `duplicate_option_label`. Without a UI the error is `no_ui`. When neither a panel nor dialogs work, the error is `no_custom_ui`.
- Result text: `User has answered your questions: "<q>"="<a>". ... You can now continue with the user's answers in mind.`, or `User declined to answer questions`. `details.answers[]` holds `{ questionIndex, question, kind: option|custom|multi, answer, selected?, notes?, preview? }`, plus `cancelled`, `globalNote?`, `unanswered?`.
- In `rpc` mode, and when pi cannot open a panel, the tool asks each question with `ui.select` and `ui.input` (multi-select takes `1,3; custom text`).
- `before_agent_start` removes the tool from the active tools when pi has no UI and adds it back when the UI returns.
- Before it waits, the tool writes one BEL (`\x07`) with `ui.writeTerminal`.

## Keys

| Key | Action |
| --- | --- |
| Up / Down | Move between rows (wraps) |
| Enter | Answer with the focused option, commit typed text, close notes, or activate Submit / Cancel. On a multi-select option it toggles the box; `Next` commits. |
| Shift+Enter | New line in the text row and in notes (legacy `ESC CR` and `CSI 27;2;13~`) |
| Esc, Ctrl+C | Cancel the questionnaire (in notes: close notes) |
| Tab / Shift+Tab, Right / Left | Next / previous tab (more than one question) |
| 1 to 9 | Answer with (single-select) or toggle (multi-select) that row; the `Type something.` digit focuses the text row |
| Space | Toggle the focused multi-select option |
| `n` (`notesKey`) | Notes for the question; on the Submit tab, the global note |
| Ctrl+G | Edit the text row or the note in pi's editor (`ui.editor`) |
| Ctrl+U | Clear the text row or the note |
| Ctrl+] (`collapseKey`) | Shrink the panel to one line and back; while collapsed only Esc works |

Options with a `preview` show the preview of the focused option in a box: beside the options when the panel is at least 100 columns wide, below them otherwise.

## Events

| Channel | Direction | Payload |
| --- | --- | --- |
| `rpiv:ask-user:prompt` | out | `{ toolCallId, questions[{ question, header, multiSelect, options[{ label, description, hasPreview }] }] }` after validation |
| `rpiv:ask-user:blocked` | out | `{ active: true, toolCallId }` before the wait, `{ active: false, toolCallId }` after |
| `rpiv:ask-user:answer` | in | `{ toolCallId, requestId?, answers?[{ questionIndex, optionIndexes?, text?, notes? }], globalNote?, cancel? }` |
| `rpiv:ask-user:answer-result` | out | `{ ok, code?: not_found\|invalid_payload\|invalid_answer\|unsupported_host, reason?, requestId?, toolCallId? }`, one per inbound answer |

An inbound answer is all or nothing. The first answer wins, from the panel or from the event; the panel closes when an event answers it.

## Config

Files, later ones win per key:

1. `$XDG_CONFIG_HOME/rpiv-ask-user-question/config.json` when it exists, else `~/.config/rpiv-ask-user-question/config.json` (upstream path)
2. `<agentDir>/ext/ask-user-question.json`
3. `<cwd>/.pi/ext/ask-user-question.json`

| Key | Default | Notes |
| --- | --- | --- |
| `collapseKey` | `"ctrl+]"` | pi key spec (`ctrl+]`, `alt+o`, `f9`); `"off"` disables; an invalid spec uses the default |
| `notesKey` | `"n"` | one character as typed, or a key spec; navigation keys are rejected |
| `guidance.description` | built-in | replaces the tool description; read at startup |
| `guidance.promptSnippet` | built-in | replaces the system prompt snippet; read at startup |
| `guidance.promptGuidelines` | 4 built-in guidelines | list of strings; read at startup |

`guidance` is read when the extension starts, from `$PI_CODING_AGENT_DIR` (or `~/.pi/agent`) and the process working directory. The keys are read each time the tool runs.

## Not ported

- `renderCall` and `renderResult`: the native protocol has no custom transcript renderers.
- `prepareArguments` before pi validates the arguments: the decoding runs in the tool, after pi's schema check.
- Overlay placement (inline editor slot versus stacked overlay) and hiding an overlay through `OverlayHandle`: collapse shrinks the panel to one line instead.
- Kitty `CSI 13;2u` Shift+Enter: the SDK hands panel keys over parsed, and that sequence arrives as Enter.
- Localization (`rpiv-i18n`): English strings only.
