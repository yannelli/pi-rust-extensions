mod command;
mod config;
mod process;
mod remote;
mod ui;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod remote_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_tests;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pi_ext_sdk::prelude::*;
use serde::Deserialize;
use tokio::sync::Mutex;

#[derive(Default)]
pub(crate) struct State {
    remote: Mutex<Option<String>>,
    warned: AtomicBool,
    edits: Mutex<()>,
}

#[derive(Deserialize)]
struct ExecArgs {
    profile: String,
    command: String,
    cwd: Option<String>,
    #[serde(default = "timeout")]
    timeout: u64,
}
fn timeout() -> u64 {
    60
}

#[derive(Deserialize)]
struct FileArgs {
    profile: String,
    path: String,
    content: Option<String>,
    #[serde(default = "timeout")]
    timeout: u64,
}

async fn execute(
    state: &State,
    args: ExecArgs,
    ctx: ToolCtx,
    input: Option<String>,
) -> Result<ToolResult> {
    let config = config::load(ctx.host(), &state.warned).await?;
    let profile = command::profile(&config, &args.profile)?;
    let remote = command::remote_command(profile, &args.command, args.cwd.as_deref());
    let remote = remote::supervised(&remote);
    let process = command::build(&ctx.host().agent_dir(), &config, profile, &remote)?;
    Ok(process::run_remote(
        process,
        input,
        args.timeout,
        ctx.cancel_token().clone(),
        Some(ctx),
    )
    .await?
    .result())
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec::new(name, description)
        .parameters(json!({"type":"object", "properties":properties, "required":required}))
}

pub fn extension() -> Extension {
    let state = Arc::new(State::default());
    let exec = state.clone();
    let read = state.clone();
    let write = state.clone();
    let list = state.clone();
    let menu = state.clone();
    let routing = state.clone();
    let mut extension = Extension::new("ssh")
        .tool(tool("ssh_exec", "Run a command using an SSH profile", json!({
            "profile":{"type":"string"}, "command":{"type":"string"}, "cwd":{"type":"string"},
            "timeout":{"type":"integer", "minimum":1,"default":60}
        }), &["profile", "command"]), move |args: ExecArgs, ctx: ToolCtx| {
            let state = exec.clone();
            async move { execute(&state, args, ctx, None).await }
        })
        .tool(tool("ssh_read", "Read a UTF-8 remote file", json!({
            "profile":{"type":"string"}, "path":{"type":"string"}, "timeout":{"type":"integer","minimum":1}
        }), &["profile", "path"]), move |args: FileArgs, ctx: ToolCtx| {
            let state = read.clone();
            async move {
                execute(&state, ExecArgs { profile:args.profile, command:format!("cat < {}", command::quote(&args.path)),
                    cwd:None, timeout:args.timeout }, ctx, None).await
            }
        })
        .tool(tool("ssh_write", "Write UTF-8 content to a remote file", json!({
            "profile":{"type":"string"}, "path":{"type":"string"}, "content":{"type":"string"},
            "timeout":{"type":"integer","minimum":1}
        }), &["profile", "path", "content"]), move |args: FileArgs, ctx: ToolCtx| {
            let state = write.clone();
            async move {
                let content = args.content.ok_or_else(|| Error::msg("content is required"))?;
                execute(&state, ExecArgs { profile:args.profile, command:format!("cat > {}", command::quote(&args.path)),
                    cwd:None, timeout:args.timeout }, ctx, Some(content)).await
            }
        })
        .tool(tool("ssh_profiles", "List SSH profiles and the active remote mode", json!({}), &[]), move |_args: Value, ctx: ToolCtx| {
            let state = list.clone();
            async move {
                let config = config::load(ctx.host(), &state.warned).await?;
                let profiles: Vec<Value> = config.profiles.iter().map(|(name,p)| json!({"name":name,
                    "host":p.host,"user":p.user,"port":p.port,"cwd":p.cwd,"enabled":p.enabled})).collect();
                Ok(ToolResult::text(serde_json::to_string_pretty(&profiles)?).details(json!({
                    "profiles":profiles,"remoteProfile":state.remote.lock().await.clone()})))
            }
        })
        .command("ssh", "Manage SSH profiles and remote bash mode", move |args, ctx| {
            let state = menu.clone();
            async move { ui::manage(&state, &args, ctx).await }
        })
        .on("tool_call", move |ctx: EventCtx| {
            let state = routing.clone();
            async move {
                if ctx.get("toolName").and_then(Value::as_str) != Some("bash") { return Ok(EventReply::none()); }
                let Some(name) = state.remote.lock().await.clone() else { return Ok(EventReply::none()); };
                let result = async {
                    let config = config::load(ctx.host(), &state.warned).await?;
                    let profile = command::profile(&config, &name)?;
                    let input = ctx.get("input").ok_or_else(|| Error::msg("Missing bash input"))?;
                    let cmd = input.get("command").and_then(Value::as_str).ok_or_else(|| Error::msg("Missing bash command"))?;
                    let remote = remote::supervised(&command::remote_command(profile, cmd, input.get("cwd").and_then(Value::as_str)));
                    let shell = command::shell_line(&ctx.host().agent_dir(), &config, profile, &remote)?;
                    let shell = remote::heartbeat_pipe(&shell);
                    Ok::<_, Error>(EventReply::patch(json!({"input":{"command":shell,"cwd":null}})))
                }.await;
                Ok(result.unwrap_or_else(|error| EventReply::block(format!("Remote SSH mode: {error}"))))
            }
        });
    for event in [
        "session_start",
        "session_switch",
        "session_tree",
        "session_compact",
    ] {
        let state = state.clone();
        extension = extension.on(event, move |ctx: EventCtx| {
            let state = state.clone();
            async move {
                let entries = ctx
                    .host()
                    .entries_of::<Option<String>>("ssh-remote")
                    .await?;
                *state.remote.lock().await = entries.last().cloned().flatten();
                ui::status(&state, ctx.host()).await?;
                Ok(())
            }
        });
    }
    extension
}

pub fn main() -> ExitCode {
    extension().run()
}
