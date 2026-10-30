# pi-ext-sdk

Created 2026-10-29. Last updated 2026-10-29.

Rust SDK for pi-rust native extensions. pi starts the extension as a child process and talks JSON-RPC 2.0 with it, one JSON object per line on stdin and stdout. The protocol is in `docs/extensions/native-protocol.md` in the pi-rust repo (protocol version 1). The SDK runs the protocol loop. You register tools, commands, and event handlers, and you call pi through a `Host` handle.

## Minimal extension

Each extension is a library crate `pi-ext-<name>` with `pub fn main() -> ExitCode`. The `pi-rust-ext` binary calls it.

```rust,no_run
use std::process::ExitCode;

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct GreetArgs {
    name: String,
}

pub fn main() -> ExitCode {
    let greet = ToolSpec::new("greet", "Greet someone").parameters(json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"]
    }));
    Extension::new("hello")
        .tool(greet, |args: GreetArgs, ctx: ToolCtx| async move {
            ctx.update("working")?;
            Ok(format!("Hello, {}", args.name))
        })
        .command("hello", "Say hello", |_args, ctx: CommandCtx| async move {
            let host = ctx.host();
            if let Some(choice) = host.select("Pick", &["a", "b"]).await? {
                host.notify_info(format!("You picked {choice}")).await?;
            }
            Ok(())
        })
        .on("before_agent_start", |_ev: EventCtx| async move {
            Ok(EventReply::section("hello", "Greet people by name."))
        })
        .run()
}
```

`examples/hello.rs` also has a widget and a panel. Run it with a transcript: `cargo run -p pi-ext-sdk --example hello < transcript.jsonl`.

## Builder: `Extension`

| Method | Use |
| --- | --- |
| `Extension::new(name)` | Starts the builder. |
| `.tool(ToolSpec, \|args: A, ctx: ToolCtx\| async { Ok(R) })` | Adds a tool. `A` is a serde type or `Value`. `R` is `ToolResult`, `String`, or `&str`. Bad arguments answer `-32602`. |
| `.command(name, description, \|args: String, ctx: CommandCtx\| ...)` | Adds a slash command. `args` is the text after the name. |
| `.completions(name, \|prefix, host\| ...)` | Adds argument completions to a command that you added before. |
| `.shortcut(key, description, \|ctx: CommandCtx\| ...)` | Adds a shortcut such as `ctrl+shift+t`. |
| `.flag(FlagSpec::boolean("plan").default(false))` | Adds a CLI flag. |
| `.on(event, \|ev: EventCtx\| async { Ok(EventReply) })` | Handles a pi event. `Ok(())` also works and means no change. |
| `.provider(name, config)` | Declares a provider (`baseUrl`, `api`, `apiKey`, `headers`, `models`, `oauth`). |
| `.virtual_model(definition, \|req: RouteRequest, host\| ...)` | Declares a virtual model and its route handler. |
| `.autocomplete(&["@"], \|req: AutocompleteRequest, host\| ...)` | Editor suggestions. Return `None` to keep the suggestions of pi. |
| `.on_start(\|host\| ...)` | Runs in its own task at the first `session_start`, when `host.snapshot()` holds the session ctx. |
| `.on_shutdown(\|host\| ...)` | Runs one time on `shutdown`, `exit`, or EOF. The limit is 1.5 seconds. |
| `.on_surface(\|event: SurfaceEvent, host\| ...)` | Runs when pi resizes or removes a widget or the footer. |
| `.run()` | Builds a tokio runtime and serves stdin and stdout. Returns `ExitCode::FAILURE` on a protocol failure. |
| `.serve(reader, writer)` | Serves any `AsyncRead` and `AsyncWrite`. Use it in tests with `tokio::io::duplex`. |

All handlers are closures that return a future: `|args, ctx| async move { ... }`. The handler result type is `pi_ext_sdk::Result<T>`.

### Handler contexts

`ToolCtx`, `EventCtx`, and `CommandCtx` are owned and cheap to clone. Move them into the `async move` block.

- All three: `host()`, `snapshot()` (the ctx snapshot: `cwd`, `has_ui`, `model`, `flags`, `session_id`, and more), `cancel_token()`, `is_cancelled()`, `cancelled().await`.
- `ToolCtx`: `tool_call_id()`, `update(partial)` sends `tool.update`.
- `EventCtx`: `name()`, `event()` (JSON), `event_as::<T>()`, `get(key)`. Typed views of common events are in `pi_ext_sdk::event_types`.

### Event replies

`EventReply { result, patch }`. `patch` is an RFC 7386 merge patch that pi applies to the event object.

| Helper | Sends |
| --- | --- |
| `EventReply::none()` | `{}` |
| `EventReply::result(value)` | `{ result }` |
| `EventReply::patch(json!({ "input": { "command": "ls" } }))` | `{ patch }` |
| `EventReply::block(reason)` | `tool_call`: `{ block: true, reason }` |
| `EventReply::transform(text)`, `EventReply::handled()` | `input` results |
| `EventReply::cancel()` | `{ cancel: true }` |
| `EventReply::section(key, text)`, `.with_section(key, None)` | Sets or removes `systemPromptOptions.sections.<key>` |
| `.with_result(value)`, `.with_patch(value)` | Combines a result and patches |

When one extension has more than one handler for an event, the handlers run in order. Each handler sees the patches of the handlers before it. A later non-null result replaces an earlier one. The SDK sends one patch for all handlers. When all handlers fail, the SDK sends the first error.

### Cancellation

`$/cancelRequest` fires the cancel token of the request. When the handler then returns `Ok`, the SDK sends the result. When it returns `Err`, the SDK sends `-32800`. When the handler does not stop in 2 seconds, the SDK drops it and sends `-32800`. A handler panic sends `-32603`.

When you drop a `Host` request future, the SDK sends `$/cancelRequest` to pi. pi then closes the dialog. For a dialog with a time limit, use `tokio::time::timeout(d, host.select(..))`.

## `Host`

`Host` is `Clone + Send + Sync`. Use it from any task, also from the inside of a handler.

| Area | Methods |
| --- | --- |
| Raw | `call(method, params)`, `call_as::<T>(..)`, `notify(method, params)`, `pi_call(target, method, args)` |
| State | `init()`, `snapshot()` (last ctx snapshot), `has_ui()`, `cwd()`, `agent_dir()`, `project_dir()` |
| UI | `notify_info`, `notify_warning`, `notify_error`, `notify_as`, `select`, `confirm`, `input`, `editor`, `set_status`, `set_widget(key, lines, Placement)`, `clear_widget`, `set_footer(lines)`, `clear_footer`, `set_title`, `set_editor_text`, `editor_text`, `write_terminal`, `panel(lines)` |
| Surfaces | `surface_events()` (resize and close of `widget:<key>` and `footer`), `surface_width(surface)` |
| Session | `send_message(CustomMessage)`, `send_user_message`, `append_entry(custom_type, &T)`, `entries()`, `branch(from_id)`, `entries_of::<T>(custom_type)`, `set_session_name`, `session_name` |
| Model and tools | `model`, `set_model`, `thinking_level`, `set_thinking_level`, `active_tools`, `set_active_tools`, `all_tools`, `all_commands`, `flag::<T>(name)` |
| Settings | `settings::<T>(namespace)` returns `Option<T>`, `all_settings()` |
| Context | `compact`, `abort`, `is_idle`, `context_usage`, `system_prompt` |
| Other | `complete(LlmRequest)`, `provider_auth`, `usage(provider)` and `usage_with(provider, refresh)` (`usage.get`), `emit(channel, data)`, `subscribe(channel)`, `exec(cmd, &args)`, `exec_with(ExecRequest)`, `register_provider`, `unregister_provider` |
| Later registration | `register_tool`, `register_command`, `register_shortcut`, `register_flag`, `on(event, handler)`, `register_autocomplete`, `register_virtual_model`, `unregister_virtual_model` |

`subscribe` returns a `Subscription`. Call `recv().await` or `recv_as::<T>().await`. When you drop the last subscription of a channel, the SDK sends `events.unsubscribe`.

`entries_of::<T>(custom_type)` reads `session.getBranch` and returns the `data` of each `custom` entry with that `customType`. It skips entries whose data does not deserialize into `T`.

## Panels

```rust,no_run
use pi_ext_sdk::prelude::*;

async fn pick(host: Host) -> Result<Value> {
    let mut panel = host.panel(["Pick one", "q closes"]).overlay(true).open().await?;
    loop {
        match panel.next_event().await {
            PanelEvent::Key(key) if key.is_cancel() => return panel.close(Value::Null).await,
            PanelEvent::Key(key) if key.is("enter") => return panel.close("picked").await,
            PanelEvent::Key(_) => {}
            PanelEvent::Resize(width) => panel.set_lines([style::truncate("Pick one", width)])?,
            PanelEvent::Closed => return Ok(Value::Null),
        }
    }
}
```

- `open()` sends `ui.custom` with a new surface id and returns at once. It fails with `-32001` when pi has no UI.
- `set_lines` sends `ui.update`. `close(result)` sends `ui.close` and waits for the `ui.custom` answer. `result()` waits without a close.
- When you drop an open panel, the SDK sends `ui.close`.
- `Key::parse(raw)` reads raw terminal input: arrows, enter, escape, tab, backspace, ctrl and alt keys, and kitty `CSI u` keys. `key.is("ctrl+c")` compares with a name. `key.is_cancel()` is true for escape, ctrl+c, and `q`.

## Widgets and footer

```rust,no_run
use pi_ext_sdk::prelude::*;

fn ext() -> Extension {
    Extension::new("status").on_surface(|event: SurfaceEvent, host: Host| async move {
        match event {
            SurfaceEvent::Resize { width, .. } if event.widget_key() == Some("todo") => {
                let line = style::truncate("3 tasks open", width);
                host.set_widget("todo", [line], Placement::AboveEditor).await
            }
            _ => Ok(()),
        }
    })
}
```

## Helpers

- `style`: `bold`, `dim`, `italic`, `underline`, `inverse`, `fg(Color, s)`, `bg(Color, s)`, `strip`, `visible_width`, `truncate(s, width)`, `fit(s, width)`. Width counts wide CJK and emoji as 2 columns and skips ANSI codes.
- `paths`: `agent_dir()` (`$PI_CODING_AGENT_DIR` or `~/.pi/agent`), `project_dir(cwd)` (`<cwd>/.pi`), `extension_dir(base, name)` (`<base>/extensions-data/<name>`). Use `host.agent_dir()` after `initialize`.
- `json`: `merge_patch`, `merge_patch_compose`, `merge_patch_diff`.
- Logging: `pi_ext_sdk::info!`, `warn!`, `error!`, `debug!` write to stderr. Warnings and errors always print. Set `PI_EXT_LOG=info` or `PI_EXT_LOG=debug` for more. Do not write to stdout: stdout carries only protocol lines.

## Errors

`pi_ext_sdk::Error` is a JSON-RPC error (`code`, `message`, `data`). The `?` operator converts any `std::error::Error` and any `anyhow::Error` into an internal error (`-32603`). Use `Error::msg("text")`, `Error::new(code, "text")`, or `bail!("text {x}")` for your own errors. `err.is_cancelled()`, `err.is_no_ui()`, and `err.is_no_context()` test the codes of pi. `Error` does not implement `std::error::Error`. In `anyhow` code, use `err.into_anyhow()`.

## Testing

Run the extension against an in-memory host:

```rust,no_run
use pi_ext_sdk::prelude::*;

async fn test(ext: Extension) {
    let (host_side, ext_side) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(ext_side);
    let server = tokio::spawn(ext.serve(read, write));
    // Write requests to host_side, read responses from it, then shut it down.
    drop(host_side);
    server.await.unwrap().unwrap();
}
```

The `test-util` feature exports `pi_ext_sdk::test_util::TestHost`, which does this. `TestHost::start(ext).cwd(..).agent_dir(..)` sets the `initialize` dirs, and `drive` sends a request and answers the extension's host requests by method until the response arrives.

For an end-to-end test under the real pi binary, add `pi-ext-e2e` as a dev-dependency. `Run::new(binary).args([...]).turns([Turn::tool_call(..), Turn::text(..)]).prompt(..).run()` starts a mock OpenAI-compatible provider, installs your binary as a `pi.native` package in a temp agent dir, runs `$PI_BIN --mode json -p <prompt>`, and returns the JSON events, the provider requests, and stderr. The test skips when `PI_BIN` does not exist. See `crates/pi-ext-e2e/README.md`.
