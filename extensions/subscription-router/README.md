# pi-ext-subscription-router

Created 2026-10-30. Last updated 2026-10-30.

Native pi-rust extension that combines `@nredd/pi-subscription-router` and `@henryqw/pi-multi-codex`:

- Several ChatGPT Codex and Anthropic subscription accounts as numbered provider slots. Slot 1 is pi's built-in provider (`openai-codex`, `anthropic`). Slots 2..n (`openai-codex-<n>`, `anthropic-<n>`) belong to the extension.
- Quota reads from the Codex and Anthropic usage endpoints, with a cache.
- `router/<profile>` virtual models that pick a chain model from cached quota, and move down the chain after a confirmed quota error.
- HTTP 429 failover between slots for models outside `router/*`.

## Tool

`subscription_usage` (label "Subscription Usage"). Parameters: `provider?: string`, `refresh?: boolean` (live read, at most once per minute per provider). Output: one line per provider (`<provider>: <windows>`) and one line per confirmed block (`blocked <provider> [families]: <window> until <ISO>`). `details: { snapshots, blocks }`.

## Commands

| Command | Does |
| --- | --- |
| `/router` or `/router status` | Cached usage, the pick of each profile with candidate scores and blocked entries, the last 5 decisions. |
| `/router refresh` | Reads usage now, then shows the status. |
| `/router reset` | Clears all confirmed blocks. |
| `/router manage` | Menu: status, refresh, reset, accounts, add account, switch slot, profiles. |
| `/router accounts` | Slot list with quota. Per slot: view quota, refresh quota, switch to the slot, log in again, remove. |
| `/router add` | Pick Codex or Anthropic, log in, and register the next free slot. |
| `/router profiles` | Create, edit (chain entries, order, thinking level, strategy, exhaustion policy, max wait, name), and delete profiles. Changes go to the config file and to pi at once. |
| `/codex-status` | One line per Codex slot: percent of the seven-day quota left, reset countdown, measured or stale. |
| `/codex-switch` | Moves the current Codex model to another slot. |

## Events

- `session_start`: shows a config error, sets the status, reads cold usage caches in the background.
- First `session_start`: registers the stored slots with `provider.register`, refreshes tokens that expire within 5 minutes (checked every minute), serves the RPC channels.
- `before_agent_start`: sets the `subscription_usage` system prompt section from cached data in 25% buckets. On the first prompt, or when the current slot hit its five-hour limit, moves to the fresh slot with the most quota (unless the user picked the model).
- `virtualModel.route` for `router/<profile>`: see "Routing".
- `after_provider_response` and `message_end`: a 429 on a slot model switches to the next eligible slot with `model.set` and returns `{ message }` with `errorMessage` `HTTP 429: ...`, so pi retries on the new slot. When no slot is left, the error says so.
- `agent_before_settle`: when a `router/*` run ends on a quota error that pi does not retry (for example `quota exceeded`, `billing`, `subscription_sharing_usage_limit_exceeded`), a fresh usage read confirms the block, a `context_edit` drops the failed message, and the run continues on the next chain model. Each chain model recovers at most once per prompt.
- `router:decision` session entries: the first pick of a profile per session and each change of the chosen model.
- `pi.events` RPC: `router:rpc:admit` `{ requestId, model }` answers `{ ok, resolvedModel?, resetAt?, wait? }`; `router:rpc:usage` `{ requestId, refresh? }` answers `{ snapshots, blocks }`. Replies go to `<channel>:reply:<requestId>` as `{ success, data | error }`.
- Status `subscription-router`: `Codex #2 · 63% · 7d 2d 4h`, `· 5h limit · 1h 2m`, `· stale`, `· unavailable`, or `router/<profile> → <model> · <slot status>`.

## Routing

- Eligible chain entry: pi has the model and credentials for it, no confirmed block covers it, and cached usage does not show its window exhausted.
- `ordered`: the first eligible entry. `balanced`: the highest `remaining / clamp(timeLeft, 0.05, 1)` over the windows that apply; a new prompt keeps the previous model unless the best candidate scores more than 20% higher.
- `retry` after a failed request: when the error matches the quota pattern and a usage read made after the failure shows the window exhausted, the window's (provider, family) is blocked until its reset (or 5 minutes, doubling to 60, when the reset is unknown) and the next entry is picked. Otherwise the same model retries. A failed message with partial output fails the route.
- All entries blocked: the request fails with the exhausted error. With policy `wait` and `maxWaitMinutes` set, `virtualModel.route` first sleeps once until the earliest reset when it is within `maxWaitMinutes`. `router:rpc:admit` reports `wait: true` under policy `wait` without a limit, as upstream.

## Config

`<agentDir>/ext/subscription-router.json`. When it does not exist, the upstream file `<agentDir>/subscription-router.json` is read. `PI_ROUTER_CONFIG` overrides the path. A missing file registers no profiles; an invalid file is ignored whole, with an error at session start.

| Key | Default | Meaning |
| --- | --- | --- |
| `profiles.<id>` | none | One profile per `router/<id>`. `id` matches `^[a-z0-9][a-z0-9._-]*$`. |
| `profiles.<id>.name` | `Router <id>` | Display name. |
| `profiles.<id>.strategy` | `ordered` | `ordered` or `balanced`. |
| `profiles.<id>.chain` | required | Non-empty list of `{ "model": "provider/id", "thinking"?: level }`. No `router/*`, no duplicates. `thinking` is one of `off minimal low medium high xhigh max`. |
| `profiles.<id>.exhaustionPolicy` | `wait` | `wait` or `fail`. |
| `profiles.<id>.maxWaitMinutes` | unset | Longest wait for a reset under `wait`. Unset: the route does not wait; `router:rpc:admit` has no limit. |
| `autoSwitchOn429` | `true` | Slot failover on HTTP 429 for models outside `router/*`. |

```json
{
  "profiles": {
    "default": {
      "strategy": "balanced",
      "chain": [
        { "model": "openai-codex-2/gpt-5.5" },
        { "model": "openai-codex/gpt-5.5" },
        { "model": "anthropic/claude-sonnet-5", "thinking": "high" }
      ],
      "exhaustionPolicy": "wait",
      "maxWaitMinutes": 360
    }
  }
}
```

## Paths and environment

| Path | Content |
| --- | --- |
| `<agentDir>/ext/subscription-router/accounts.json` | Credentials of slots 2..n (`access`, `refresh`, `expires`, `accountId`), mode 0600, atomic writes under an `accounts.json.lock` directory lock. Re-read before each refresh and save. A malformed file blocks saves until it is fixed. |
| `<agentDir>/ext/subscription-router/usage.json` | Usage snapshots per provider, mode 0600, atomic writes under an `accounts.json.lock` directory lock. Re-read before each refresh and save. A malformed file blocks saves until it is fixed. |

| Variable | Default |
| --- | --- |
| `PI_ROUTER_CONFIG` | the config path above |
| `PI_ROUTER_CODEX_USAGE_URL` | `https://chatgpt.com/backend-api/wham/usage` |
| `PI_ROUTER_ANTHROPIC_USAGE_URL` | `https://api.anthropic.com/api/oauth/usage` |
| `PI_ROUTER_CODEX_TOKEN_URL` | `https://auth.openai.com/oauth/token` |
| `PI_ROUTER_ANTHROPIC_TOKEN_URL` | `https://platform.claude.com/v1/oauth/token` |

Usage reads: Codex sends `Authorization: Bearer` and `ChatGPT-Account-Id` (from the token JWT); Anthropic sends `Authorization: Bearer` and `anthropic-beta: oauth-2025-04-20`. A good read is cached for 5 minutes. A failed read backs off 1, 2, 5, then 15 minutes, or longer when `retry-after` asks (at most 60 minutes).

## Logins and the OAuth gap

Slot 1 uses pi's `/login`. The extension reads its token with `auth.getProviderAuth` for quota reads.

For slots 2..n the extension runs the login itself, ported from pi-ai:

- Codex: PKCE, a callback listener on `localhost:1455/auth/callback`, and a pasted code or redirect URL in `ui.input` as the fallback.
- Anthropic: PKCE, then the user pastes the `code#state` that Anthropic shows.

The extension opens the browser with `open` / `xdg-open` and also shows the URL with `ui.notify`. The slot registers with `provider.register` and the access token as a literal `apiKey`; the extension refreshes it before expiry and registers again.

Why: `provider.register` sends `oauth` as plain JSON, and pi's `/login` calls `config.oauth[method]` as a JavaScript function. A declarative `oauth` therefore fails with "Provider X has no OAuth login". As a result, `/login` and `/logout` do not list slots 2..n, and pi does not refresh them.

Host change that removes the gap: accept `oauth: { builtin: "<provider id>" }` in `provider.register`. pi then reuses that built-in provider's login and refresh for the alias, stores its credentials in `auth.json` under the alias id, and lists it in `/login`. The extension then registers slots without `apiKey` and drops its own login code.
