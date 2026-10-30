pub mod command;
pub mod http;
pub mod prompt;

use std::path::PathBuf;

use pi_ext_sdk::Host;
use serde_json::Value;

use crate::model::HookDef;
use crate::output::RawRun;
use crate::paths::{env_file, project_root};
use command::{CommandSpec, run_command};

pub struct Context {
    pub host: Host,
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub session_id: String,
}

pub async fn run(cx: &Context, def: &HookDef, input: String) -> RawRun {
    let timeout = def.timeout();
    match def.kind() {
        "http" => http::run_http(&def.handler, input, timeout).await,
        "prompt" => prompt::run_prompt(&cx.host, &def.handler, input, timeout).await,
        _ => {
            let args = def
                .handler
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .map(|a| a.as_str().map_or_else(|| a.to_string(), str::to_owned))
                        .collect()
                });
            let spec = CommandSpec {
                command: def.str_field("command").unwrap_or_default().to_owned(),
                args,
                stdin: input,
                cwd: cx.cwd.clone(),
                env: hook_env(cx, def),
                timeout,
            };
            run_command(spec).await
        }
    }
}

pub fn hook_env(cx: &Context, def: &HookDef) -> Vec<(String, String)> {
    let project = project_root(&cx.cwd, ".claude");
    let mut env = vec![(
        "CLAUDE_PROJECT_DIR".to_owned(),
        project.display().to_string(),
    )];
    if let Some(plugin) = &def.plugin {
        if let Some(root) = &plugin.root {
            env.push(("CLAUDE_PLUGIN_ROOT".into(), root.clone()));
            env.push(("PLUGIN_ROOT".into(), root.clone()));
        }
        if let Some(data) = &plugin.data {
            env.push(("CLAUDE_PLUGIN_DATA".into(), data.clone()));
            env.push(("PLUGIN_DATA".into(), data.clone()));
        }
    }
    if def.event == "SessionStart" {
        let file = env_file(&cx.agent_dir, &cx.session_id);
        let created = file
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&file)
                    .map(drop)
            });
        match created {
            Ok(()) => env.push(("CLAUDE_ENV_FILE".into(), file.display().to_string())),
            Err(err) => pi_ext_sdk::warn!("hooks: cannot create {}: {err}", file.display()),
        }
    }
    env
}
