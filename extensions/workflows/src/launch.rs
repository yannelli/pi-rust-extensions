//! The `pi --mode json -p` command line and environment of one step agent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{ASK, Scope};

/// The JSON Schema of every step agent's final answer.
pub const REPORT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "status": { "type": "string", "enum": ["done", "continue", "blocked"] },
    "summary": { "type": "string" }
  },
  "required": ["status", "summary"],
  "additionalProperties": false
}
"#;

const REPORT_GUIDE: &str = "You run one step of an automated workflow. No user watches this run: \
work on your own and do not ask questions. Your final answer is the step report as JSON: \
`status` is `done` when the task (or its goal) is complete, `continue` when the goal is not met \
yet and the workflow should check again later, and `blocked` when you cannot go on. `summary` \
tells the next agent what you did, what changed (files, commands, links), and what is still open.";

/// Skills and MCP servers that a launch resolves names against.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Catalog {
    /// Skill name to `SKILL.md` path.
    pub skills: BTreeMap<String, String>,
    /// Configured or connected MCP server names.
    pub mcp_servers: Vec<String>,
}

/// One agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub model: Option<String>,
}

pub struct Request<'a> {
    pub scope: &'a Scope,
    pub parent_model: Option<&'a str>,
    pub parent_thinking: Option<&'a str>,
    pub catalog: &'a Catalog,
    pub session: &'a Path,
    pub schema: &'a Path,
    /// Skills that tasks run as `/skill:` commands; they load also when `skills` limits the list.
    pub task_skills: &'a [String],
    pub memory: Option<&'a Path>,
    pub env: Vec<(String, String)>,
    pub prompt: &'a str,
}

pub fn build(program: &Path, cwd: &Path, req: Request<'_>) -> Result<Launch, String> {
    let scope = req.scope;
    let mut args: Vec<String> = vec![
        "--mode".into(),
        "json".into(),
        "-p".into(),
        "--session".into(),
    ];
    args.push(req.session.display().to_string());
    let own_model = scope.model.clone().filter(|m| m != ASK);
    let (model, thinking) = match own_model {
        Some(model) => (Some(model), scope.thinking.clone()),
        None => (
            req.parent_model.map(str::to_owned),
            scope
                .thinking
                .clone()
                .or(req.parent_thinking.map(str::to_owned)),
        ),
    };
    if let Some(model) = &model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(thinking) = &thinking {
        args.extend(["--thinking".into(), thinking.clone()]);
    }
    tool_args(scope, req.catalog, req.memory.is_some(), &mut args);
    skill_args(scope, req.catalog, req.task_skills, &mut args)?;
    let mut system = REPORT_GUIDE.to_owned();
    if let Some(extra) = scope
        .system_prompt
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        system = format!("{system}\n\n{extra}");
    }
    args.extend(["--append-system-prompt".into(), system]);
    args.extend(["--output-schema".into(), req.schema.display().to_string()]);
    if let Some(turns) = scope.max_turns {
        args.extend(["--max-turns".into(), turns.to_string()]);
    }
    if let Some(cost) = scope.max_cost {
        args.extend(["--max-cost".into(), cost.to_string()]);
    }
    args.push("--".into());
    // pi reads a positional that starts with `@` as a file.
    args.push(if req.prompt.starts_with('@') {
        format!(" {}", req.prompt)
    } else {
        req.prompt.to_owned()
    });
    let mut env: Vec<(String, String)> = scope
        .env
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                expand_env(value, |name| std::env::var(name).ok()),
            )
        })
        .collect();
    env.extend(req.env);
    if let Some(rules) = scope.subagents.as_ref().filter(|r| !r.is_empty()) {
        let rules = serde_json::to_string(rules).map_err(|e| e.to_string())?;
        env.push((crate::policy::SUBAGENTS_ENV.into(), rules));
    }
    if let Some(memory) = req.memory {
        env.push((
            crate::policy::MEMORY_ENV.into(),
            memory.display().to_string(),
        ));
    }
    Ok(Launch {
        program: program.to_owned(),
        args,
        env,
        cwd: cwd.to_owned(),
        model,
    })
}

fn tool_args(scope: &Scope, catalog: &Catalog, memory: bool, args: &mut Vec<String>) {
    let mut excluded = vec!["workflow".to_owned()];
    if scope.subagents.as_ref().is_some_and(Vec::is_empty) {
        excluded.push("subagent".into());
    }
    let servers = scope.mcp.as_ref();
    match &scope.tools {
        Some(tools) => {
            let mut allowed: Vec<String> = tools
                .iter()
                .filter(|t| !excluded.contains(t))
                .cloned()
                .collect();
            let rules = scope.subagents.as_ref().is_some_and(|r| !r.is_empty());
            if rules && !allowed.iter().any(|t| t == "subagent") {
                allowed.push("subagent".into());
            }
            allowed.extend(
                servers
                    .into_iter()
                    .flatten()
                    .map(|s| format!("mcp__{}__*", mcp_id(s))),
            );
            if memory && !allowed.is_empty() {
                allowed.push(crate::policy::MEMORY_TOOL.into());
            }
            if allowed.is_empty() {
                args.push("--no-tools".into());
            } else {
                args.extend(["--tools".into(), allowed.join(",")]);
            }
        }
        None => {
            if let Some(servers) = servers.filter(|s| !s.is_empty()) {
                let keep: Vec<String> = servers.iter().map(|s| mcp_id(s)).collect();
                excluded.extend(
                    catalog
                        .mcp_servers
                        .iter()
                        .map(|s| mcp_id(s))
                        .filter(|s| !keep.contains(s))
                        .map(|s| format!("mcp__{s}__*")),
                );
            }
        }
    }
    if servers.is_some_and(Vec::is_empty) {
        args.push("--no-mcp".into());
    }
    args.extend(["--exclude-tools".into(), excluded.join(",")]);
}

fn skill_args(
    scope: &Scope,
    catalog: &Catalog,
    task_skills: &[String],
    args: &mut Vec<String>,
) -> Result<(), String> {
    let Some(skills) = &scope.skills else {
        return Ok(());
    };
    args.push("--no-skills".into());
    let mut names: Vec<&String> = skills.iter().collect();
    names.extend(task_skills.iter().filter(|s| !skills.contains(s)));
    for name in names {
        let path = catalog
            .skills
            .get(name)
            .ok_or_else(|| format!("unknown skill {name:?}"))?;
        args.extend(["--skill".into(), path.clone()]);
    }
    Ok(())
}

/// The MCP server part of a tool name: everything but `[A-Za-z0-9_]` becomes `_`.
pub fn mcp_id(server: &str) -> String {
    server
        .chars()
        .flat_map(|c| {
            let keep = c.is_ascii_alphanumeric() || c == '_';
            std::iter::repeat_n(
                if keep { c } else { '_' },
                if keep { 1 } else { c.len_utf16() },
            )
        })
        .collect()
}

/// Replaces `${NAME}` with the variable's value, or nothing when it is unset.
pub fn expand_env(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            rest = &rest[start..];
            break;
        };
        out.push_str(&lookup(&after[..end]).unwrap_or_default());
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `$PI_BIN`, else the parent process when it is pi, else `pi` on `PATH`.
pub fn pi_binary() -> PathBuf {
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
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    path.is_absolute().then_some(path)
}

#[cfg(not(unix))]
fn parent_exe() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SubagentRule;

    fn launch(
        scope: &Scope,
        catalog: &Catalog,
        task_skills: &[String],
        memory: bool,
    ) -> Result<Launch, String> {
        let memory_path = PathBuf::from("/run/memory.md");
        build(
            Path::new("pi"),
            Path::new("/work"),
            Request {
                scope,
                parent_model: Some("anthropic/opus"),
                parent_thinking: Some("high"),
                catalog,
                session: Path::new("/run/01-a.jsonl"),
                schema: Path::new("/run/report.schema.json"),
                task_skills,
                memory: memory.then_some(memory_path.as_path()),
                env: vec![("PI_WORKFLOW_RUN".into(), "wf-1".into())],
                prompt: "@do it",
            },
        )
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == flag)
            .map(|i| args[i + 1].as_str())
    }

    #[test]
    fn inherits_the_parent_model_and_thinking() {
        let out = launch(&Scope::default(), &Catalog::default(), &[], false).unwrap();
        assert_eq!(after(&out.args, "--model"), Some("anthropic/opus"));
        assert_eq!(after(&out.args, "--thinking"), Some("high"));
        assert_eq!(after(&out.args, "--exclude-tools"), Some("workflow"));
        assert_eq!(out.args.last().map(String::as_str), Some(" @do it"));
        assert!(!out.args.contains(&"--tools".to_owned()));
        let own = Scope {
            model: Some("openai/gpt".into()),
            ..Scope::default()
        };
        let out = launch(&own, &Catalog::default(), &[], false).unwrap();
        assert_eq!(after(&out.args, "--model"), Some("openai/gpt"));
        assert_eq!(after(&out.args, "--thinking"), None);
    }

    #[test]
    fn scopes_tools_mcp_and_skills() {
        let catalog = Catalog {
            skills: BTreeMap::from([
                ("review".into(), "/s/review/SKILL.md".into()),
                ("pr".into(), "/s/pr/SKILL.md".into()),
            ]),
            mcp_servers: vec!["github".into(), "my-db".into()],
        };
        let scope = Scope {
            tools: Some(vec!["read".into(), "bash".into()]),
            mcp: Some(vec!["github".into()]),
            skills: Some(vec!["review".into()]),
            subagents: Some(vec![]),
            ..Scope::default()
        };
        let out = launch(&scope, &catalog, &["pr".into()], true).unwrap();
        assert_eq!(
            after(&out.args, "--tools"),
            Some("read,bash,mcp__github__*,workflow_memory")
        );
        assert_eq!(
            after(&out.args, "--exclude-tools"),
            Some("workflow,subagent")
        );
        assert!(out.args.contains(&"--no-skills".to_owned()));
        let skills: Vec<&str> = out
            .args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--skill")
            .map(|(i, _)| out.args[i + 1].as_str())
            .collect();
        assert_eq!(skills, ["/s/review/SKILL.md", "/s/pr/SKILL.md"]);
        assert!(
            out.env
                .contains(&("PI_WORKFLOW_MEMORY".into(), "/run/memory.md".into()))
        );

        let only_mcp = Scope {
            mcp: Some(vec!["github".into()]),
            ..Scope::default()
        };
        let out = launch(&only_mcp, &catalog, &[], false).unwrap();
        assert_eq!(
            after(&out.args, "--exclude-tools"),
            Some("workflow,mcp__my_db__*")
        );
        let no_mcp = Scope {
            mcp: Some(vec![]),
            tools: Some(vec![]),
            ..Scope::default()
        };
        let out = launch(&no_mcp, &catalog, &[], false).unwrap();
        assert!(
            out.args.contains(&"--no-mcp".to_owned())
                && out.args.contains(&"--no-tools".to_owned())
        );
        let unknown = Scope {
            skills: Some(vec!["nope".into()]),
            ..Scope::default()
        };
        assert_eq!(
            launch(&unknown, &catalog, &[], false).unwrap_err(),
            "unknown skill \"nope\""
        );
    }

    #[test]
    fn env_expands_and_carries_subagent_rules() {
        let scope = Scope {
            env: BTreeMap::from([("DATABASE_URL".into(), "${WF_TEST_DB}/app".into())]),
            subagents: Some(vec![SubagentRule {
                agent: "explore".into(),
                ..SubagentRule::default()
            }]),
            max_turns: Some(5),
            ..Scope::default()
        };
        let out = launch(&scope, &Catalog::default(), &[], false).unwrap();
        assert_eq!(out.env[0], ("DATABASE_URL".into(), "/app".into()));
        assert!(
            out.env
                .iter()
                .any(|(k, v)| k == "PI_WORKFLOW_SUBAGENTS" && v.contains("explore"))
        );
        assert_eq!(after(&out.args, "--max-turns"), Some("5"));
        assert_eq!(
            expand_env("${A}-${B}-${", |n| (n == "A").then(|| "x".into())),
            "x--${"
        );
    }
}
