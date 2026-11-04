# pi-ext-timers

Created 2026-11-04. Last updated 2026-11-04.

Timers for the agent: a `sleep` that ends early on notifications, a heartbeat that wakes an idle agent during long unattended work, and prompts scheduled for a time, after a delay, every interval, or on a cron expression. Heartbeats and schedules belong to one session, are saved to disk, and are armed again after `/reload` or a restart.

## Tools

| Tool | Parameters | Result |
| --- | --- | --- |
| `sleep` | `seconds` (1 to `maxSleepSeconds`), `reason?` | `Slept Ns`, or `Woke after Ns: <source>` when it ends early |
| `heartbeat` | `action` (`start`, `ack`, `list`), `every?` (default `15m`), `reason` (required for `start`), `name?`, `max_pulses?`, `id` (for `ack`) | The new heartbeat with its id, the stopped heartbeat, or the list |
| `schedule` | `action` (`create`, `list`, `cancel`), `prompt` (required for `create`), one of `at`, `in`, `every`, `cron`, `max_runs?`, `name?`, `id` (for `cancel`) | The new schedule with its next run time, the cancelled schedule, or the list |

Durations are strings such as `30s`, `15m`, `1h30m`, `2d`, or a number of minutes.

`sleep` ends early when the tool call is cancelled, when an `agent:wake` message arrives on the `pi.events` bus (payload `{source, reason}`; background tasks, subagents, and agent messages send it), when a schedule of this extension fires, or when the user sends a steering message while the agent runs (Enter while the agent works). A queued follow-up (Alt+Enter or Ctrl+Q) does not end the sleep: it waits until the agent stops, as follow-ups do. The input itself is not changed.

A heartbeat is a watchdog for the session. At each interval, when the agent is idle, it sends a `heartbeat` custom message (`display: true`, `deliverAs: followUp`, `triggerTurn: true`) with the id, name, pulse number, interval, the reason it was started, and how to stop it (`heartbeat` with action `ack` and the id). When the agent is busy, the pulse is skipped. After `max_pulses` pulses it stops. The minimum interval is `minHeartbeatSeconds`.

A schedule sends a `schedule` custom message (`display: true`, `deliverAs: followUp`, `triggerTurn: true`) with the id, name, and prompt, then emits `agent:wake` with `{source: "schedule", reason: <name or id>}`. It fires also while the agent is busy; pi queues the message. A repeating schedule says how to cancel it.

| Trigger | Meaning |
| --- | --- |
| `at` | Once, at a local datetime (`2026-05-01 09:00`, `2026-05-01T09:00:00`), a local time (`14:30`, today or tomorrow), or RFC 3339 |
| `in` | Once, after a delay |
| `every` | Repeats at an interval of at least 1 minute |
| `cron` | Repeats on a 5-field expression in local time: `*`, `*/n`, `a-b`, `a-b/n`, `a/n`, lists, numbers; day of week 0-7 (0 and 7 are Sunday). When day of month and day of week are both restricted, either one matches. |

One-shot schedules are removed after they fire. Repeating ones stop after `max_runs` runs.

`before_agent_start` adds the system prompt section `pi_timers_policy`, which says when to use each tool.

## Command

| Command | Action |
| --- | --- |
| `/timers` | Lists heartbeats and schedules with ids and next fire time |
| `/timers cancel <id>` | Removes a heartbeat or schedule. Completes ids. |
| `/timers clear` | Removes all heartbeats and schedules of the session |

The footer status `timers` shows the counts (`timers: 1 heartbeat, 2 schedules`) and is cleared when there are none.

## Sessions and reload

The timers of a session are armed at `session_start` and disarmed at `session_shutdown`. A session switch disarms the old session's timers and arms the new session's. After a load, a schedule whose time passed while pi was not running fires once; a heartbeat waits one full interval.

## Config

`<agentDir>/ext/timers.json` and `<cwd>/.pi/ext/timers.json`; project keys win.

| Key | Default | Meaning |
| --- | --- | --- |
| `maxSleepSeconds` | 3600 | Longest `sleep` |
| `minHeartbeatSeconds` | 30 | Shortest heartbeat interval |

## Paths

- Timers of a session: `<agentDir>/ext/timers/sessions/<sessionId>.json`, written with a temp-file rename and removed when the session has no timers. Without a session id the file is `no-session.json`.

## Tests

`cargo test -p pi-ext-timers` runs unit tests (cron parsing and next match, durations and datetimes, heartbeat and schedule transitions, the session file, `/timers`) and protocol tests against an in-memory host (`sleep` woken by `agent:wake` and by input, a schedule firing, session switches).
