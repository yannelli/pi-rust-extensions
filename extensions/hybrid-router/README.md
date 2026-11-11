# hybrid-router

Created 2026-11-10. Last updated 2026-11-10.

The virtual model `hybrid/auto`. Each prompt that you write goes to a local model or to a remote model:

- The classifier of the local model rates the prompt `simple` or `complex`. Simple prompts go to the local model. Complex prompts go to the remote model.
- When the 1-minute load average per core is more than `busyLoadPerCore` (0.8 by default), prompts go to the remote model. The extension does not run the classifier.
- When the local model is not in pi's catalog, or its classifier fails, prompts go to the remote model.
- Tool results and the other requests of a turn stay on the model that answered. When a request to the local model fails, pi retries it on the remote model.

The pi footer shows the model that answered, for example `(hybrid) auto → unsloth/Qwen3.8-27B-GGUF:Q4_K_M`.

## Requirements

- pi-rust 0.12.12 or later. Older releases do not give extensions the classifier models.
- A local chat model that has a classifier with the same provider and ID. Each model that a llama.cpp router loads is also a classifier (API `llama-cpp-classify`).

## Set up

1. Start a llama.cpp router, for example `llama serve --no-models-autoload -ngl all -c 32768`.
2. In pi, run `/login llama.cpp`, then load a model with `/llama`.
3. Log in to the provider of the remote model with `/login`, or set its API key.
4. Run `/hybrid`. Select the local model, then the provider and the model for the remote model.

pi selects `hybrid/auto`. To select it later, use `/model` or `pi --model hybrid/auto`.

## Commands

| Command | Action |
| --- | --- |
| `/hybrid` | Select the local and the remote model, save them, and select `hybrid/auto`. |
| `/hybrid status` | Show the models, the busy limit, and the load per core now. |
| `/hybrid off` | Remove the config and `hybrid/auto`. A session on `hybrid/auto` moves to the remote model. |

## Config

`/hybrid` writes `~/.pi/agent/ext/hybrid-router.json` (in `$PI_CODING_AGENT_DIR` when it is set):

```json
{
  "local": "llama.cpp/unsloth/Qwen3.8-27B-GGUF:Q4_K_M",
  "remote": "zai/glm-5.3",
  "busyLoadPerCore": 0.8
}
```

| Key | Value |
| --- | --- |
| `local` | `provider/id` of the local chat model. |
| `remote` | `provider/id` of the model for complex prompts, and for all prompts while the machine is busy. |
| `busyLoadPerCore` | Optional. The default is 0.8. Windows has no load average, so the extension always runs the classifier there. |

The extension reads the file when pi starts and when `/hybrid` saves it. Restart pi after you edit the file.
