pub mod agent;
pub mod command;
pub mod http;
pub mod mcp;
pub mod prompt;
pub mod shell;

use std::path::PathBuf;

use pi_ext_sdk::Host;
use serde_json::Value;

use crate::model::HookDef;
use crate::output::RawRun;
use crate::paths::{env_file, project_root};
use command::{CommandSpec, run_command};
use shell::ShellKind;

pub struct Context {
    pub host: Host,
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub session_id: String,
    pub http: http::HttpPolicy,
    /// pi thinking level as a Claude effort level, for `CLAUDE_EFFORT`.
    pub effort: Option<&'static str>,
}

pub async fn run(cx: &Context, def: &HookDef, input: String) -> RawRun {
    let timeout = def.timeout();
    match def.kind() {
        "http" => http::run_http(&def.handler, input, timeout, &cx.http).await,
        "prompt" => prompt::run_prompt(&cx.host, &def.event, &def.handler, input, timeout).await,
        "mcp_tool" => mcp::run_mcp_tool(&cx.host, &def.handler, &input, timeout).await,
        "agent" => agent::run_agent(&def.handler, &def.event, input, &cx.cwd, timeout).await,
        _ => {
            let env = hook_env(cx, def);
            let args: Option<Vec<String>> =
                def.handler
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .map(|a| a.as_str().map_or_else(|| a.to_string(), str::to_owned))
                            .map(|a| expand_placeholders(&a, &env))
                            .collect()
                    });
            let mut command = command_line(def).to_owned();
            if args.is_some() {
                command = expand_placeholders(&command, &env);
            }
            let spec = CommandSpec {
                command,
                args,
                stdin: input,
                cwd: cx.cwd.clone(),
                env,
                timeout,
                shell: shell_kind(def),
                shell_path: shell_path(cx).await,
            };
            run_command(spec).await
        }
    }
}

/// Codex `commandWindows` (TOML `command_windows`) replaces `command` on Windows.
fn command_line(def: &HookDef) -> &str {
    let windows = (cfg!(windows) && def.source.is_codex())
        .then(|| {
            def.str_field("commandWindows")
                .or(def.str_field("command_windows"))
        })
        .flatten();
    windows.or(def.str_field("command")).unwrap_or_default()
}

fn shell_kind(def: &HookDef) -> ShellKind {
    if def.str_field("shell") == Some("powershell") {
        ShellKind::PowerShell
    } else if def.source.is_codex() {
        ShellKind::Login
    } else {
        ShellKind::Posix
    }
}

/// pi's `shellPath`, so hooks on Windows use the same bash as the bash tool.
async fn shell_path(cx: &Context) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    cx.host
        .settings::<String>("shellPath")
        .await
        .ok()
        .flatten()
        .filter(|path| !path.trim().is_empty())
}

const PLACEHOLDERS: &[&str] = &[
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PLUGIN_DATA",
];

/// Exec form has no shell, so path placeholders and plugin `${user_config.<key>}` values (from
/// `CLAUDE_PLUGIN_OPTION_<KEY>`) are replaced as plain strings.
pub fn expand_placeholders(text: &str, env: &[(String, String)]) -> String {
    let mut text = text.to_owned();
    for name in PLACEHOLDERS {
        if let Some((_, value)) = env.iter().find(|(key, _)| key == name) {
            text = text.replace(&format!("${{{name}}}"), value);
        }
    }
    for (key, value) in env {
        if let Some(option) = key.strip_prefix(crate::config::PLUGIN_OPTION_PREFIX) {
            let placeholder = format!("${{user_config.{}}}", option.to_lowercase());
            text = text.replace(&placeholder, value);
        }
    }
    text
}

pub fn hook_env(cx: &Context, def: &HookDef) -> Vec<(String, String)> {
    let project = project_root(&cx.cwd, ".claude");
    let mut env = vec![(
        "CLAUDE_PROJECT_DIR".to_owned(),
        project.display().to_string(),
    )];
    if let Some(level) = cx.effort.filter(|_| !def.source.is_codex()) {
        env.push(("CLAUDE_EFFORT".into(), level.into()));
    }
    if let Some(plugin) = &def.plugin {
        if let Some(root) = &plugin.root {
            env.push(("CLAUDE_PLUGIN_ROOT".into(), root.clone()));
            env.push(("PLUGIN_ROOT".into(), root.clone()));
        }
        if let Some(data) = &plugin.data {
            env.push(("CLAUDE_PLUGIN_DATA".into(), data.clone()));
            env.push(("PLUGIN_DATA".into(), data.clone()));
        }
        env.extend(plugin.env.iter().cloned());
    }
    if matches!(
        &*def.event,
        "SessionStart" | "Setup" | "CwdChanged" | "FileChanged"
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_expand_from_the_hook_env() {
        let env = vec![
            ("CLAUDE_PROJECT_DIR".to_owned(), "/repo".to_owned()),
            ("CLAUDE_PLUGIN_ROOT".to_owned(), "/plug".to_owned()),
        ];
        assert_eq!(
            expand_placeholders("${CLAUDE_PROJECT_DIR}/x ${CLAUDE_PLUGIN_ROOT} $HOME", &env),
            "/repo/x /plug $HOME"
        );
        assert_eq!(
            expand_placeholders("${CLAUDE_PLUGIN_DATA}", &env),
            "${CLAUDE_PLUGIN_DATA}"
        );
        let plugin = vec![(
            "CLAUDE_PLUGIN_OPTION_WEBHOOK_URL".to_owned(),
            "https://h.example".to_owned(),
        )];
        assert_eq!(
            expand_placeholders("--url=${user_config.webhook_url}", &plugin),
            "--url=https://h.example"
        );
    }
}
