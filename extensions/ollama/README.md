# ollama

Created 2026-10-30. Last updated 2026-10-30.

Local Ollama models in pi-rust. The extension reads `/api/tags` and `/api/show` (context length and capabilities) and registers provider `ollama` with `provider.register`: OpenAI-compatible API (`openai-completions`) at `<host>/v1`, API key `ollama`. Embedding-only models are skipped; `thinking` sets `reasoning` and `vision` adds image input.

Discovery runs at process start (at most 1.5 seconds, so `--model ollama/<id>` resolves), on each `session_start`, and on `/ollama refresh`. pi's llama.cpp support is separate and unchanged.

## Commands

- `/ollama` or `/ollama list`: a panel with the models. Keys: arrows, Enter details, `p` pull (with progress from the `/api/pull` stream, Esc cancels), `d` delete, `r` refresh, `h` host, `q` close.
- `/ollama refresh`, `/ollama pull <model>` (progress in the status bar), `/ollama delete <model>`, `/ollama show <model>`, `/ollama host [url]`.

## Config

| Source | Key | Default |
| --- | --- | --- |
| `<cwd>/.pi/ext/ollama.json` (trusted projects only) | `host` | none |
| `~/.pi/agent/ext/ollama.json` (`/ollama host` writes it) | `host` | none |
| Environment | `OLLAMA_HOST` | `http://127.0.0.1:11434` |

The first one set wins. `host`, `host:port`, `:port`, and URLs work; `0.0.0.0` means `127.0.0.1`.
