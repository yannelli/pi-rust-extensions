# pi-ext-subscription-router

Created 2026-10-30. Last updated 2026-11-05.

Native pi-rust extension that combines `@nredd/pi-subscription-router` and `@henryqw/pi-multi-codex`:

- Several ChatGPT Codex and Anthropic subscription accounts. Account 1 is pi's own login of the built-in provider (`openai-codex`, `anthropic`). Accounts 2..n log in under `openai-codex-<n>` and `anthropic-<n>`.
- With pi that has the `providerAuth` feature, the accounts are logins of one provider: models show once (`anthropic/claude-opus-5-5`), and a policy picks the account of each request (see [Accounts](#accounts)). On older pi each account is a provider slot with its own copy of the models.
- Quota reads from the Codex and Anthropic usage endpoints, with a cache.
- When an account is full: the next account, a banked Codex usage reset, or a fallback model of another provider.
- `router/<profile>` virtual models that pick a chain model from cached quota, and move down the chain after a confirmed quota error.

## Tool

`subscription_usage` (label "Subscription Usage"). Parameters: `provider?: string`, `refresh?: boolean` (live read, at most once per minute per provider). Output: one line per provider (`<provider>: <windows>`) and one line per confirmed block (`blocked <provider> [families]: <window> until <ISO>`). `details: { snapshots, blocks }`.

## Commands

| Command | Does |
| --- | --- |
| `/router` or `/router status` | Cached usage, the pick of each profile with candidate scores and blocked entries, the last 5 decisions. |
| `/router refresh` | Reads usage now, then shows the status. |
| `/router reset` | Clears all confirmed blocks. |
| `/router manage` | Menu: status, refresh, reset, accounts, add account, switch slot, profiles, account policy, usage resets. |
| `/router accounts` | Account list with quota. Per account: view quota, refresh quota, use it (switch to the slot on older pi), log in again, remove. |
| `/router use [claude\|codex] [<n>\|auto]` | The account of this session for a provider; `auto` returns to the policy. Without arguments, a menu. |
| `/router policy [claude\|codex]` | Menu of the provider's [account policy](#accounts); each change is saved to the config file. |
| `/router resets [<n>]` | Banked usage resets of each Codex account; with a number, spends one on that account after a confirmation. |
| `/router add` | Pick Codex or Anthropic, log in, and register the next free slot. |
| `/router profiles` | Create, edit (chain entries, order, thinking level, strategy, exhaustion policy, max wait, name), and delete profiles. Changes go to the config file and to pi at once. |
| `/codex-status` | One line per Codex slot: percent of the seven-day quota left, reset countdown, measured or stale. |
| `/codex-switch` | Picks the Codex account of this session (moves the model to another slot on older pi). |

## Events

- `session_start`: shows a config error, sets the status, reads cold usage caches in the background.
- `initialize`: declares stored slots whose family has cached base models, so `--model` and saved default models resolve before the first session starts. Both Anthropic and ChatGPT Codex use the cache; accounts without cached models wait for the first `session_start`.
- First `session_start`: re-registers the stored slots with fresh base models through `provider.register`, refreshes tokens that expire within 5 minutes (checked every minute), serves the RPC channels. Each successful registration updates the family's model cache; a cache write failure is logged without undoing registration.
- `before_agent_start`: sets the `subscription_usage` system prompt section from cached data in 25% buckets. With accounts: moves a session on a retired account model (`anthropic-2/x`) to `anthropic/x` on that account, returns from a fallback when the provider has room, and falls back before sending when every account is full. On older pi: on the first prompt, or when the current slot hit its five-hour limit, moves to the fresh slot with the most quota (unless the user picked the model).
- `before_provider_auth` (accounts): names the account of each request for an `anthropic` or `openai-codex` model, as [Accounts](#accounts) describes.
- `virtualModel.route` for `router/<profile>`: see "Routing".
- `after_provider_response` and `message_end`: a limit error (HTTP 429, or a rate, usage, or quota limit message) marks the account full until its reset, then picks what [Accounts](#accounts) describes and returns `{ message }` with `errorMessage` `HTTP 429: rate limited; ...`, so pi retries. On older pi, a 429 on a slot model switches to the next eligible slot with `model.set`. When nothing is left, the error says so and pi stops.
- `agent_before_settle`: when a `router/*` run ends on a quota error that pi does not retry (for example `quota exceeded`, `billing`, `subscription_sharing_usage_limit_exceeded`), a fresh usage read confirms the block, a `context_edit` drops the failed message, and the run continues on the next chain model. Each chain model recovers at most once per prompt.
- `router:decision` session entries: the first pick of a profile per session and each change of the chosen model.
- `pi.events` RPC: `router:rpc:admit` `{ requestId, model }` answers `{ ok, resolvedModel?, resetAt?, wait? }`; `router:rpc:usage` `{ requestId, refresh? }` answers `{ snapshots, blocks }`. Replies go to `<channel>:reply:<requestId>` as `{ success, data | error }`.
- Status `subscription-router`: the account in use, `Codex #2 · 63% · 7d 2d 4h`, `· 5h limit · 1h 2m`, `· stale`, `· unavailable`, `· Claude full, fallback`, or `router/<profile> → <model> · <account status>`.

## Accounts

pi-rust with the `providerAuth` feature (it lists it in `initialize`) sends `before_provider_auth` before each request. The router registers accounts 2..n as logins of the built-in provider with no models (`provider.register` with `oauth: { builtin }` and `models: []`), and answers each request for an `anthropic` or `openai-codex` model with the account to use. The model stays the same; only the credential changes. Model lists show each model once.

An account is full when a usage window that applies to the model (the five-hour or weekly window, or a model window such as weekly Opus) is at 100% (or `switchAt` under `threshold`) and resets later, when a confirmed block covers it, or when a request on it failed with a limit error, until the reset the next usage read reports (5 minutes when unknown).

Which account a request uses, first match wins:

1. The account a `router/<profile>` chain entry named for this model.
2. The account `/router use` pinned for this session.
3. The provider's `strategy`:
   - `manual`: always `account`.
   - `fill`: the account in use until it is full, then the next one in order.
   - `threshold`: like `fill`, with an account full at `switchAt` percent.
   - `balance` (default): among accounts that are not full, the best `remaining / clamp(timeLeft, 0.05, 1)`; the account in use stays unless another scores more than 20% higher.

When a request fails with a limit error:

1. With `useResets: "when-full"` (Codex), the router spends a banked usage reset on that account and retries it.
2. Otherwise the policy picks the next account that is not full, and pi retries there.
3. When every account is full: with `useResets: "when-all-full"`, a reset on the account that stays full longest; else the first `fallback` model that can run (`model.set`, with its `thinking` level). With `returnWhenAvailable`, the next prompt after an account has room switches back to the original model and thinking level. Choosing another model ends the fallback.
4. Nothing left: the error names the time the first account frees up, and pi stops.

The router also falls back before sending a prompt when cached usage shows every account full.

Usage resets: the Codex CLI's `rate-limit-reset-credits` endpoints, next to the usage endpoint. The router lists the account's banked resets and spends one (`consume` with a new `redeem_request_id`) only when the full window frees up later than `resetMinWaitMinutes`, at most once per account in 10 minutes, then reads usage again. Anthropic's saved resets have no API; apply them in claude.ai Settings > Usage.

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
| `profiles.<id>.chain` | required | Non-empty list of `{ "model": "provider/id", "account"?: n, "thinking"?: level }`. `account` (anthropic and openai-codex models) runs the model on that account; `anthropic-2/id` is read as `anthropic/id` on account 2. No `router/*`, no duplicates. `thinking` is one of `off minimal low medium high xhigh max`. |
| `profiles.<id>.exhaustionPolicy` | `wait` | `wait` or `fail`. |
| `profiles.<id>.maxWaitMinutes` | unset | Longest wait for a reset under `wait`. Unset: the route does not wait; `router:rpc:admit` has no limit. |
| `autoSwitchOn429` | `true` | Account failover on limit errors for models outside `router/*`. |
| `accounts.<provider>` | defaults below | Account policy of `anthropic` or `openai-codex`. |
| `accounts.<provider>.strategy` | `balance` | `manual`, `fill`, `threshold`, or `balance`. |
| `accounts.<provider>.account` | `1` | The account of `manual`. |
| `accounts.<provider>.switchAt` | `90` | Percent used at which `threshold` treats an account as full. |
| `accounts.<provider>.fallback` | `[]` | Models of other providers for when every account is full, in order: `{ "model", "account"?, "thinking"? }`. |
| `accounts.<provider>.returnWhenAvailable` | `true` | Switch back from a fallback when an account has room. |
| `accounts.<provider>.useResets` | `off` | Codex usage resets: `off`, `when-full`, or `when-all-full`. |
| `accounts.<provider>.resetMinWaitMinutes` | `60` | A full window that frees up sooner is waited out instead of reset. |

```json
{
  "accounts": {
    "anthropic": {
      "strategy": "threshold",
      "switchAt": 90,
      "fallback": [{ "model": "openai-codex/gpt-5.5", "thinking": "high" }]
    },
    "openai-codex": { "strategy": "fill", "useResets": "when-all-full" }
  },
  "profiles": {
    "default": {
      "strategy": "balanced",
      "chain": [
        { "model": "openai-codex/gpt-5.5", "account": 2 },
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
| `<agentDir>/ext/subscription-router/models.json` | Base model lists by built-in provider: `{ "version": 1, "families": { "anthropic": [<models>], "openai-codex": [<models>] } }`. Mode 0600, atomic writes under a `models.json.lock` directory lock, with new directories mode 0700. Re-read under the lock before updating a family. Missing, malformed, or unsupported-version caches declare no startup slots and are replaced after successful registration. Contains model definitions, without account credentials. |
| `<agentDir>/ext/subscription-router/accounts.json` | Slots 2..n: `{ "type": "pi" }` when pi stores the login, else the router's own `access`, `refresh`, `expires`, `accountId`. Mode 0600, atomic writes under an `accounts.json.lock` directory lock. Re-read before each refresh and save. A malformed file blocks saves until it is fixed. |
| `<agentDir>/ext/subscription-router/usage.json` | Usage snapshots per provider, mode 0600, atomic writes under an `accounts.json.lock` directory lock. Re-read before each refresh and save. A malformed file blocks saves until it is fixed. |

| Variable | Default |
| --- | --- |
| `PI_ROUTER_CONFIG` | the config path above |
| `PI_ROUTER_CODEX_USAGE_URL` | `https://chatgpt.com/backend-api/wham/usage` |
| `PI_ROUTER_ANTHROPIC_USAGE_URL` | `https://api.anthropic.com/api/oauth/usage` |
| `PI_ROUTER_CODEX_TOKEN_URL` | `https://auth.openai.com/oauth/token` |
| `PI_ROUTER_ANTHROPIC_TOKEN_URL` | `https://platform.claude.com/v1/oauth/token` |
| `PI_ROUTER_CODEX_RESETS_URL` | the Codex usage URL with `/usage` replaced by `/rate-limit-reset-credits` |

Usage reads: Codex sends `Authorization: Bearer` and `ChatGPT-Account-Id` (from the token JWT); Anthropic sends `Authorization: Bearer` and `anthropic-beta: oauth-2025-04-20`. A good read is cached for 5 minutes. A failed read backs off 1, 2, 5, then 15 minutes, or longer when `retry-after` asks (at most 60 minutes).

## Logins

Slot 1 uses pi's `/login`. Slots 2..n (pi-rust with `auth.login`):

- Each slot registers with `provider.register` and `oauth: { builtin: "openai-codex" | "anthropic", name: "<family> #<n>" }`, without `apiKey`. `/login` and `/logout` list the slot under that name.
- `/router add` and "log in again" in `/router accounts` open pi's login dialog for the slot (`auth.login`). pi runs the login, stores the token in `auth.json` under the slot id, and refreshes it. When the user cancels, the slot stays as it was.
- The account file keeps `{ "type": "pi" }` for such a slot. Quota reads take the token from `auth.getProviderAuth`.
- Removing a slot unregisters it. pi keeps the token until `/logout <slot id>`.

Slots of older versions keep their own tokens in the account file. The router refreshes them and logs their ids at start. "Log in again" moves a slot to pi's login.

With pi that has no `auth.login` (it answers `-32601`), the router runs its own login, ported from pi-ai, and keeps the token in the account file:

- Codex: PKCE, a callback listener on `localhost:1455/auth/callback`, and a pasted code or redirect URL in `ui.input` as the fallback.
- Anthropic: PKCE, then the user pastes the `code#state` that Anthropic shows.
