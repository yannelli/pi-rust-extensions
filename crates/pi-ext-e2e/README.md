# pi-ext-e2e

Created 2026-10-29. Last updated 2026-10-29.

A dev-only harness that runs a native extension under the real pi binary. `MockProvider` is a local OpenAI-compatible chat-completions server that streams scripted turns and records each request. `Run` writes a temp agent dir (`models.json` points at the mock), a temp package whose `package.json` `pi.native` names your binary, and runs `$PI_BIN --mode json -p <prompt>` with a timeout. It returns the JSON events, the requests, and stderr (extension log lines included).

```rust,ignore
// Cargo.toml: [dev-dependencies] pi-ext-e2e = { path = "../../crates/pi-ext-e2e" }
use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn todo_tool_runs_in_pi() {
    if pi_bin_or_skip().is_none() { return; }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["todo"])
        .turns([Turn::tool_call("todo", json!({"action": "list"})), Turn::text("done")])
        .prompt("List my todos")
        .run()
        .unwrap();
    assert_eq!(output.tool_result_text("todo").as_deref(), Some("No todos"));
}
```

- `PI_BIN` selects pi (default `/Users/ryanyannelli/Development/pi-upstream-ref/bin/pi-dev`); tests skip with a message when it does not exist.
- `PI_E2E_KEEP=1` keeps the temp dir. pi runs with a cleared environment (`PATH`, `HOME`, `TMPDIR`, `LANG` only) and `PI_OFFLINE=1`.
- `Output` helpers: `events_of`, `tool_results`, `tool_result_text`, `tool_updates`, `tool_names`, `system_prompt`, `final_text`.
- `Run` options: `.env(k, v)` for the extension, `.settings(json)`, `.pi_args([...])` (for example extension flags), `.file("work/AGENTS.md", text)` (a file under the cwd `work/` or the agent dir `agent/`, written before pi starts), `.timeout(..)`.
