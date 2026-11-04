# pi-ext-simple-goal

Created 2026-11-04. Last updated 2026-11-04.

One objective that keeps the agent working until an evaluator agrees it is done. Use it for small, well-defined tasks. The `goal` extension covers larger work with drafting, a task tree, and contracts; `simple-goal` has none of those.

After each run (`agent_settled`) with an active simple goal, the extension judges whether the objective is done. The judge gets:

- the objective and the optional `done_when` check,
- the text of the agent's last 3 assistant messages (each cut to 2000 characters),
- `git status --short` (cut to 2000 characters) and `git diff HEAD` of the session cwd (cut to 16000 characters; `git diff` in a repository without commits).

The judge is TypeSafe or an LLM, set by `judge`:

- **TypeSafe** (`judge` `auto` or `typesafe`): when the `typesafe` extension is loaded and enabled, simple-goal sends one `noul` question ("Is the objective fully met?") with the evidence above as `state` on the `pi.events` channel `typesafe:decision:request`, and waits up to 20 seconds for the reply on `typesafe:decision:reply`. The goal is done when the probability reaches `typesafeThreshold`. The request uses the TypeSafe backend and model the user picked with `/typesafe models` (for example an OpenRouter decision model), and counts toward the TypeSafe limits. With `auto`, a missing or disabled TypeSafe, an error, or a timeout falls back to the LLM; with `typesafe`, it pauses the goal.
- **LLM** (`judge` `llm`, or the `auto` fallback): one `llm.complete` call with the evidence and the configured `instructions`. The reply ends with `<done/>` or `<continue/>` on the last non-empty line, after a short reason. A reply without either marker counts as `<continue/>`.

TypeSafe sends the evidence, including the diff and the agent messages, to the TypeSafe backend. Set `judge` to `llm` to keep it on the session's model provider.

| Verdict | Result |
| --- | --- |
| `<done/>` or TypeSafe probability at or above the threshold | The goal is cleared. The user gets a notification, and a displayed `simple-goal` custom message gives the reason. No turn starts. |
| `<continue/>` or TypeSafe probability below the threshold | The round count goes up and a `simple-goal` custom message (`display: true`, `deliverAs: followUp`, `triggerTurn: true`) gives the agent the objective, the reason, the round `n/maxRounds`, and how to give up with `simple_goal` action `clear`. |
| `<continue/>` after `maxRounds` rounds | The goal pauses and the user gets a warning. |
| `<continue/>` twice in a row after runs with no tool calls | The goal pauses. Calls of `simple_goal` itself do not count. |
| Evaluator error | The goal pauses and the user gets a warning. |

An aborted run pauses the goal without an evaluation. `/simple-goal resume` sets a paused goal active again with a new round count and starts a turn.

The evaluation runs in its own task, so `agent_settled` returns at once. A goal that was cleared, replaced, or paused during the evaluation ignores the verdict.

## Tool

| Tool | Parameters | Result |
| --- | --- | --- |
| `simple_goal` | `action` (`set`, `status`, `clear`), `objective` (required for `set`), `done_when?` | `set` replaces the current simple goal. `status` reports objective, state (active with round, or paused with the reason), and the last evaluation. `clear` removes the goal. |

## Command

| Command | Action |
| --- | --- |
| `/simple-goal <objective>` | Sets the goal and starts a turn with the objective. |
| `/simple-goal` | Shows the status. |
| `/simple-goal clear` | Clears the goal. |
| `/simple-goal resume` | Resumes a paused goal and starts a turn. |

The footer status `simple-goal` shows `simple goal 2/8` while a goal is active and is empty otherwise.

## Config

`<agentDir>/ext/simple-goal.json`, then `<cwd>/.pi/ext/simple-goal.json` (project keys win). `model`, `thinking`, `systemPrompt`, and `judge` apply from the project file only in a trusted project; otherwise the extension ignores them and warns once.

| Key | Default | Use |
| --- | --- | --- |
| `model` | session model | Evaluator model as `provider/id` |
| `thinking` | unset | Reasoning level of the evaluator call, such as `low` or `high` |
| `systemPrompt` | built in | Replaces the evaluator system prompt |
| `instructions` | unset | Text added to the evaluator prompt |
| `maxRounds` | `8` | Continuations before the goal pauses (minimum 1) |
| `judge` | `auto` | `auto` (TypeSafe when enabled, else the LLM), `typesafe`, or `llm` |
| `typesafeThreshold` | `0.7` | Probability of done, from 0 to 1, at which a TypeSafe answer counts as done |

Global example:

```json
{
  "model": "anthropic/claude-haiku-4-5",
  "thinking": "low",
  "maxRounds": 6
}
```

Project example:

```json
{
  "instructions": "Done means `cargo test --workspace` passes and the change has a test.",
  "maxRounds": 10
}
```

## Files

State lives in the session as `custom` entries with `customType` `simple-goal` and data `{ "goal": { ... } }` or `{ "goal": null }` after a clear. The extension restores the latest entry of the current branch on `session_start` and `session_tree`, so the goal survives `/reload` and resume.

## Tests

`cargo test -p pi-ext-simple-goal` runs unit tests (verdict parsing, recent messages, the TypeSafe request and verdict, config layering with and without trust, settle decisions, prompt building) and protocol tests against an in-memory host with a mocked `llm.complete` reply and a mocked TypeSafe bus reply.
