# pi-ext-todo

Created 2026-10-30. Last updated 2026-10-30.

A task list for pi, ported from `@juicesharp/rpiv-todo`, with ordered phases, subagent scopes, a widget above the editor, and a `/todos` panel. Run it with `pi-rust-ext todo`.

## Tool `todo`

The six rpiv-todo actions keep their parameters, status machine, dependency checks, result text, and `details` snapshot:

| Action | Params |
| --- | --- |
| `create` | `subject` or `tasks[]`, `description`, `activeForm`, `blockedBy`, `owner`, `metadata`, `phase` |
| `update` | `id`, `subject`, `description`, `activeForm`, `status`, `owner`, `metadata`, `addBlockedBy`, `removeBlockedBy`, `phase` (`0` removes the phase) |
| `list` | `status`, `includeDeleted` |
| `get`, `delete` | `id` |
| `clear` | none; also removes phases |

Added actions:

| Action | Params | Result text |
| --- | --- | --- |
| `move` | `id`, `position` (1-based) | `Moved #3 to position 1` |
| `add_phase` | `title`, `goal`, `position` | `Added phase P1: Design (pending)` |
| `update_phase` | `phase`, `title`, `goal`, `status` (`pending`, `in_progress`, `completed`), `position` | `Updated phase P1 (pending → in_progress)` |
| `set_goal` | `phase`, `goal` (empty clears) | `Set goal of phase P1: ...` |
| `delete_phase` | `phase` | `Deleted phase P1: Design (2 tasks moved to no phase)` |

Tasks without `phase` belong to no phase, so calls written for rpiv-todo work unchanged. A task that goes `in_progress` starts its pending phase. The current phase is the first phase in progress, else the first pending phase, else the last phase. `details` adds `phases` and `nextPhaseId` when phases exist, and `scope` in a subagent child.

## Command `/todos`

Opens a panel: `↑`/`↓` (or `j`/`k`) move, `enter`/`space` advance the status of a todo or phase, `a` adds a todo to the selected phase, `p` adds a phase, `e` edits the subject or title, `g` sets the phase goal, `d` deletes (asks `y`/`n`), `K`/`J` reorder, `v` (or `enter`) cycles the visibility of a subagent scope, `c` collapses the widget, `q`/`esc` closes. Each change appends a state entry. `/todos list`, or `/todos` without UI, shows the rpiv-todo grouped text through a notification.

Shortcut: `collapseKey` (default `ctrl+shift+t`) collapses or expands the widget.

## Widget

Key `rpiv-todos`, above the editor. Expanded: a `Todos (done/total)` heading with the current phase, the phase goal, and the todos of the current phase plus todos without a phase (`○` pending, `◐` in progress, `✓` completed, `⛓` dependencies). Completed todos shown in one turn hide at the next `agent_start`. Collapsed: the heading and the expand hint. The widget clears when nothing is left to show.

## Scopes

- A subagent child runs with `PI_TODO_SCOPE=<run id>`. Its todo extension tags `details` with `scope` and draws no widget.
- The parent subscribes to the `pi.events` channel `todo:scope`. Payload: `{ "scope": "<run id>", "label": "<agent>: <task>", "status": "running" | "completed" | "failed" | "cancelled", "state": <child todo details> | null }`. The `subagents` extension sends it.
- Per-scope visibility: `all` shows the scope's todos in the widget, `collapsed` shows one summary line, `main-only` hides the scope from the widget. Finished scopes hide from the widget at the next `agent_start`.

## Session entries

| `customType` | Data |
| --- | --- |
| `rpiv-todo-state` | the `details` snapshot after each change |
| `pi-todo-scopes` | `{ "scopes": [{ scope, label, status, state, visibility? }] }` after each scope change |

On `session_start`, `session_tree`, and `session_compact` the extension replays the branch: the last valid `rpiv-todo-state` entry, else the last `todo` tool result `details`, and the last `pi-todo-scopes` entry.

## Config

Global `<agentDir>/ext/todo.json`, project `<cwd>/.pi/ext/todo.json`. The project file wins per key (`guidance` per field).

| Key | Default | Use |
| --- | --- | --- |
| `maxWidgetLines` | `12` (min 3) | Widget rows for the heading, goal, and todos |
| `collapseKey` | `"ctrl+shift+t"` | Collapse shortcut; `"off"` disables it. Read at load. |
| `defaultVisibility` | `"collapsed"` | Visibility of scopes without their own setting |
| `guidance.promptSnippet` | rpiv-todo snippet | Tool prompt snippet. Read at load. |
| `guidance.promptGuidelines` | rpiv-todo guidelines plus one for phases | Tool prompt guidelines. Read at load. |

## Not ported

rpiv-todo's custom `renderCall` and `renderResult` and its i18n locales have no native protocol equivalent.
