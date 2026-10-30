//! The `pi --mode json -p` command line of one child run.

use std::path::{Path, PathBuf};

use crate::agent::{AgentDef, PromptMode};

/// The parent session's model (`provider/id`) and thinking level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inherit {
    pub model: Option<String>,
    pub thinking: Option<String>,
}

/// Per-call overrides from the `subagent` tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub model: Option<String>,
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub thinking: Option<String>,
}

pub struct ChildInput<'a> {
    pub agent: &'a AgentDef,
    pub task: &'a str,
    pub run_id: &'a str,
    pub parent: &'a Inherit,
    pub overrides: &'a Overrides,
    pub program: &'a Path,
    pub cwd: &'a Path,
    pub extra_args: &'a [String],
}

/// Model: call override, else agent model, else the parent model. Thinking follows the same
/// order; a model set by the agent or the call does not take the parent's thinking level.
pub fn resolve_model(
    agent: &AgentDef,
    parent: &Inherit,
    overrides: &Overrides,
) -> (Option<String>, Option<String>) {
    let own_model = overrides.model.clone().or_else(|| agent.model.clone());
    let own_thinking = overrides
        .thinking
        .clone()
        .or_else(|| agent.thinking.clone());
    match own_model {
        Some(model) => (Some(model), own_thinking),
        None => (
            parent.model.clone(),
            own_thinking.or_else(|| parent.thinking.clone()),
        ),
    }
}

pub fn build(input: ChildInput<'_>) -> ChildSpec {
    let agent = input.agent;
    let (model, thinking) = resolve_model(agent, input.parent, input.overrides);
    let mut args: Vec<String> = ["--mode", "json", "-p", "--no-session"]
        .map(String::from)
        .to_vec();
    if let Some(model) = &model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(thinking) = &thinking {
        args.extend(["--thinking".into(), thinking.clone()]);
    }
    let mut denied: Vec<&str> = vec!["subagent"];
    denied.extend(agent.disallowed_tools.iter().map(String::as_str));
    match &agent.tools {
        Some(tools) => {
            let tools: Vec<&str> = tools
                .iter()
                .map(String::as_str)
                .filter(|t| !denied.contains(t))
                .collect();
            if tools.is_empty() {
                args.push("--no-tools".into());
            } else {
                args.extend(["--tools".into(), tools.join(",")]);
            }
        }
        None => args.extend(["--exclude-tools".into(), denied.join(",")]),
    }
    if !agent.prompt.trim().is_empty() {
        let flag = match agent.prompt_mode {
            PromptMode::Append => "--append-system-prompt",
            PromptMode::Replace => "--system-prompt",
        };
        args.extend([flag.into(), agent.prompt.clone()]);
    }
    if let Some(turns) = agent.max_turns {
        args.extend(["--max-turns".into(), turns.to_string()]);
    }
    args.extend(input.extra_args.iter().cloned());
    args.push("--".into());
    args.push(task_arg(input.task));
    let env = vec![
        ("PI_TODO_SCOPE".into(), input.run_id.to_owned()),
        ("PI_SUBAGENT_RUN_ID".into(), input.run_id.to_owned()),
        ("PI_SUBAGENT_AGENT".into(), agent.name.clone()),
    ];
    ChildSpec {
        program: input.program.to_owned(),
        args,
        env,
        cwd: input.cwd.to_owned(),
        model,
        thinking,
    }
}

/// pi reads a positional that starts with `@` as a file; a leading space keeps it text.
fn task_arg(task: &str) -> String {
    if task.starts_with('@') {
        format!(" {task}")
    } else {
        task.to_owned()
    }
}

/// `configured` (config `piBin`), else `$PI_BIN`, else the parent process when it is pi,
/// else `pi` on `PATH`.
pub fn pi_binary(configured: Option<&str>) -> PathBuf {
    if let Some(path) = configured.filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("PI_BIN").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    parent_pi().unwrap_or_else(|| PathBuf::from("pi"))
}

fn parent_pi() -> Option<PathBuf> {
    let exe = parent_exe()?;
    let name = exe.file_name()?.to_str()?.to_ascii_lowercase();
    (name.starts_with("pi") && exe.is_file()).then_some(exe)
}

#[cfg(unix)]
fn parent_exe() -> Option<PathBuf> {
    let pid = std::os::unix::process::parent_id();
    if let Ok(path) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        return Some(path);
    }
    let output = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let comm = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let path = PathBuf::from(&comm);
    path.is_absolute().then_some(path)
}

#[cfg(not(unix))]
fn parent_exe() -> Option<PathBuf> {
    None
}
