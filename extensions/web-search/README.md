# web-search

Created 2026-10-30. Last updated 2026-10-30.

This extension adds web search and URL reading to pi. It is a Rust port of [pi-web-search](https://github.com/ttttmr/pi-web-search) 1.7.0. The tool names, parameters, and `details` fields are the same as upstream. The extension also adds four search APIs: Exa, Firecrawl, Brave, and Tavily.

## Tools

| Tool | Parameters | Use |
| --- | --- | --- |
| `web_search` | `query` (string), `urls` (optional, a maximum of 20) | Searches the web. Also reads the URLs that you give. |
| `url_context` | `query` (string), `urls` (1 to 20) | Reads the URLs and answers the query. |

An error result has `isError: true` and `details.error`: `unsupported_model`, `missing_config`, `invalid_config`, `configured_model_not_found`, `unsupported_provider`, `missing_api_key`, or `true` for a failed request.

pi removes `url_context` from the active tools when no provider can read URLs. pi adds it again when a provider becomes available. The extension does this at `session_start`, `session_tree`, `model_select`, and when `/web-search` closes. Tools that you turn on or off yourself stay that way.

## Providers

| Id | Provider | `web_search` | `url_context` |
| --- | --- | --- | --- |
| `native` | The pi model, with its own credentials | Yes | Gemini and Ollama Cloud only |
| `exa` | Exa | Yes | Yes |
| `firecrawl` | Firecrawl, also a self-hosted Firecrawl | Yes | Yes |
| `brave` | Brave Search | Yes | No |
| `tavily` | Tavily | Yes | Yes |
| `auto` | Selection by rule (the default) | See below | See below |

`auto` for `web_search`:

1. If `searchModel` or the legacy file sets a model, the extension uses that model. It does not try a different provider.
2. If the current model has native search, the extension uses it.
3. Else, the extension uses the first API provider that has a key, in this order: Exa, Tavily, Brave, Firecrawl.

`auto` for `url_context` uses the current model if it is a Gemini or Ollama Cloud model. Else, it uses the first API provider with a key that can read pages: Exa, Tavily, Firecrawl.

### Native search

The extension gets the full model from pi with `pi.call` (`ctx.model` and `ctx.modelRegistry.find`). It gets the credentials with `ctx.modelRegistry.getApiKeyAndHeaders`. If pi gives no key, the extension reads the provider environment variable, for example `OPENAI_API_KEY`.

| Model | Request |
| --- | --- |
| Gemini (`google-generative-ai`) | `streamGenerateContent` with `google_search` and `url_context`. YouTube URLs go to `url_context` as video parts. |
| Antigravity, Gemini CLI | The same request, in the Cloud Code `v1internal` envelope. The key is JSON `{ "token", "projectId" }`. |
| OpenAI Responses, Azure, Codex, GitHub Copilot, OpenCode Zen and Go | `POST /responses` with the `web_search` tool. The request uses the session thinking level as `reasoning.effort`. |
| xAI Grok, and `grok-*` model ids | The Responses request in the Grok shape. Set `compat.webSearchDialect` to `"grok"` or `"openai"` to override. |
| Anthropic, DeepSeek | Messages API with the `web_search_20260209` tool. DeepSeek uses `<baseUrl>/anthropic/v1/messages`. |
| Ollama Cloud | `POST /api/web_search` and `/api/web_fetch`. |

## Command

`/web-search` opens a menu. Use it to set these items:

- The default provider, and the provider for each tool.
- The search model.
- The API keys. The extension writes them to the keys file with mode 0600.
- The base URL of each API provider.

Provider settings can use the global or project scope. API keys use the global keys file.

## Config

| Path | Contents |
| --- | --- |
| `<agentDir>/ext/web-search.json` | Global config |
| `<cwd>/.pi/ext/web-search.json` | Project config. The extension reads it only when pi trusts the project. The project value of a key replaces the global value. |
| `$PI_WEB_SEARCH_CONFIG`, else `<agentDir>/web-search.json` | Upstream file `{ "provider", "model" }` (`modelId` is also correct). It sets the search model. `searchModel` has priority over this file. |
| `<agentDir>/ext/web-search/keys.json` | API keys, mode 0600 |

`<agentDir>` is `~/.pi/agent`, or `$PI_CODING_AGENT_DIR`.

```json
{
  "defaultProvider": "auto",
  "tools": { "web_search": "exa", "url_context": "firecrawl" },
  "searchModel": "openai/gpt-5.1",
  "providers": {
    "exa": { "baseUrl": "https://api.exa.ai", "maxResults": 10 },
    "firecrawl": { "baseUrl": "https://api.firecrawl.dev", "maxResults": 10 },
    "brave": { "baseUrl": "https://api.search.brave.com", "maxResults": 10 },
    "tavily": { "baseUrl": "https://api.tavily.com", "maxResults": 10 }
  }
}
```

| Key | Default | Values |
| --- | --- | --- |
| `defaultProvider` | `auto` | `auto`, `native`, `exa`, `firecrawl`, `brave`, `tavily` |
| `tools.web_search`, `tools.url_context` | `defaultProvider` | The same values |
| `searchModel` | The current model | `provider/id` |
| `providers.<id>.baseUrl` | The URLs above | A URL |
| `providers.<id>.maxResults` | `10` | 1 to 20 |

A project config can send requests and keys to a different host. Thus the extension ignores the project file in a project that pi does not trust. It writes one log line when it ignores the file.

### API keys

The extension uses the key in `keys.json`. If there is no key in the file, it uses the environment variable:

| Provider | Variable |
| --- | --- |
| Exa | `EXA_API_KEY` |
| Firecrawl | `FIRECRAWL_API_KEY` |
| Brave | `BRAVE_API_KEY` |
| Tavily | `TAVILY_API_KEY` |

## Provider API documents

Read 2026-10-29.

- Exa: <https://exa.ai/docs/reference/search>, <https://exa.ai/docs/reference/get-contents>. Header `x-api-key`.
- Firecrawl: <https://docs.firecrawl.dev/api-reference/endpoint/search>, <https://docs.firecrawl.dev/api-reference/endpoint/scrape>. `Authorization: Bearer`.
- Brave: <https://api-dashboard.search.brave.com/app/documentation/web-search/get-started>. Header `X-Subscription-Token`. `count` has a maximum of 20.
- Tavily: <https://docs.tavily.com/documentation/api-reference/endpoint/search>, <https://docs.tavily.com/documentation/api-reference/endpoint/extract>. `Authorization: Bearer`.
- OpenAI: <https://platform.openai.com/docs/guides/tools-web-search>.
- Anthropic: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool>.
- Gemini: <https://ai.google.dev/gemini-api/docs/google-search>, <https://ai.google.dev/gemini-api/docs/url-context>.
- Ollama: <https://docs.ollama.com/capabilities/web-search>.

## Tests

```sh
cargo test -p pi-ext-web-search
```

The tests use a mock HTTP server on localhost for each provider and an in-memory pi host for the protocol. They use fake keys and make no requests outside localhost.
