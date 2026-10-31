//! Decides one pi tool call against the loaded rules.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::file_rules::{bash_files, carved, inside_dirs, needs_exact_rule, path_rule_matches};
use crate::pattern::{PathContext, bash_matches, domain_matches, name_glob, normalize, url_host};
use crate::rules::{ClaudeRule, Decision, Kind, Rule, split_claude};
use crate::shell::{self, Word, raw_text, strip_wrappers};

const READ_TOOLS: &[&str] = &["read", "grep", "find", "ls"];
const EDIT_TOOLS: &[&str] = &["edit", "write"];
const FETCH_TOOLS: &[&str] = &["fetch", "web_fetch", "url_context"];

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Command(Vec<Word>),
    Path(PathBuf),
    /// A file that a Bash command or redirection reads or writes; `plain` is false for a
    /// path with `~`, a glob, or a variable.
    File {
        path: PathBuf,
        write: bool,
        plain: bool,
    },
    Url(String),
    /// A script Codex cannot split, as the single call `["bash", "-lc", script]`.
    Shell(Vec<String>),
    Call,
}

impl Target {
    pub fn describe(&self) -> String {
        match self {
            Target::Command(words) => raw_text(words),
            Target::Path(path) => path.display().to_string(),
            Target::File { path, write, .. } => {
                let verb = if *write { "write" } else { "read" };
                format!("{verb} {}", path.display())
            }
            Target::Url(host) => host.clone(),
            Target::Shell(words) => words.join(" "),
            Target::Call => "call".to_owned(),
        }
    }
}

pub struct Call<'a> {
    pub tool: &'a str,
    pub input: &'a Value,
    pub cwd: &'a Path,
    pub home: &'a Path,
    /// The cwd and the additional directories.
    pub dirs: &'a [PathBuf],
}

#[derive(Debug, Clone, Default)]
pub struct Verdict {
    pub decision: Option<Decision>,
    /// The rule behind `decision`.
    pub rule: Option<Rule>,
    /// Each target with its deciding rule.
    pub targets: Vec<(Target, Option<Rule>)>,
}

/// Claude tool names for a pi tool; the pi name itself also matches.
pub fn claude_names(tool: &str) -> Vec<&str> {
    let mapped: &[&str] = match tool {
        "bash" => &["Bash"],
        "read" => &["Read"],
        "edit" => &["Edit", "MultiEdit"],
        "write" => &["Write"],
        "grep" => &["Grep"],
        "find" => &["Glob"],
        "ls" => &["LS"],
        "powershell" => &["PowerShell"],
        "web_search" => &["WebSearch"],
        "subagent" => &["Agent", "Task"],
        t if FETCH_TOOLS.contains(&t) => &["WebFetch"],
        _ => &[],
    };
    let mut names = mapped.to_vec();
    names.push(tool);
    names
}

/// The parts of a call that rules test; `false` when allow rules cannot apply to the bash script.
pub fn targets(call: &Call) -> (Vec<Target>, bool, bool) {
    let input = call.input;
    let path = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(|p| normalize(&call.cwd.join(p)))
    };
    match call.tool {
        "bash" => {
            let script = shell::parse(input.get("command").and_then(Value::as_str).unwrap_or(""));
            let mut targets: Vec<Target> = script
                .commands
                .iter()
                .cloned()
                .map(Target::Command)
                .collect();
            targets.extend(bash_files(&script, call));
            if !script.plain {
                let command = input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                targets.push(Target::Shell(vec![
                    "bash".into(),
                    "-lc".into(),
                    command.into(),
                ]));
            }
            (targets, script.complete, script.plain)
        }
        t if READ_TOOLS.contains(&t) || EDIT_TOOLS.contains(&t) => {
            let p = path("path")
                .or_else(|| path("file_path"))
                .unwrap_or_else(|| call.cwd.to_path_buf());
            (vec![Target::Path(p)], true, true)
        }
        t if FETCH_TOOLS.contains(&t) => {
            let mut urls: Vec<&str> = input
                .get("url")
                .and_then(Value::as_str)
                .into_iter()
                .collect();
            if let Some(list) = input.get("urls").and_then(Value::as_array) {
                urls.extend(list.iter().filter_map(Value::as_str));
            }
            let hosts = urls
                .into_iter()
                .map(|u| Target::Url(url_host(u).unwrap_or_default()));
            (hosts.collect(), true, true)
        }
        _ => (vec![Target::Call], true, true),
    }
}

pub fn evaluate(rules: &[Rule], call: &Call) -> Verdict {
    let (targets, complete, plain) = targets(call);
    let decided: Vec<(Target, Option<Rule>)> = targets
        .into_iter()
        .map(|target| {
            let best = rules
                .iter()
                .filter(|rule| complete || rule.decision != Decision::Allow)
                .filter(|rule| applies(rule, call, &target, plain))
                .filter(|rule| !carved(rules, rule, call, &target))
                .max_by_key(|rule| rule.rank())
                .cloned();
            (target, best)
        })
        .collect();
    let pick = |d: Decision| {
        decided
            .iter()
            .find_map(|(_, rule)| rule.as_ref().filter(|r| r.decision == d))
            .cloned()
    };
    let all_allowed = !decided.is_empty()
        && decided.iter().all(|(target, rule)| {
            rule.is_some() || inside_dirs(target, call) || matches!(target, Target::Shell(_))
        });
    let rule = pick(Decision::Deny)
        .or_else(|| pick(Decision::Ask))
        .or_else(|| all_allowed.then(|| pick(Decision::Allow)).flatten());
    Verdict {
        decision: rule.as_ref().map(|r| r.decision),
        rule,
        targets: decided,
    }
}

fn applies(rule: &Rule, call: &Call, target: &Target, plain: bool) -> bool {
    match &rule.kind {
        Kind::Codex(prefix) => match target {
            Target::Command(words) => {
                (plain || rule.decision != Decision::Allow) && prefix.matches(&shell::values(words))
            }
            Target::Shell(words) => prefix.matches(words),
            _ => false,
        },
        Kind::Network(network) => match target {
            Target::Url(host) => {
                FETCH_TOOLS.contains(&call.tool) && domain_matches(&network.host, host)
            }
            _ => false,
        },
        Kind::Claude(claude) => claude_applies(claude, rule.decision, call, target),
    }
}

fn claude_applies(rule: &ClaudeRule, decision: Decision, call: &Call, target: &Target) -> bool {
    let restrict = decision != Decision::Allow;
    let Some(spec) = &rule.spec else {
        return name_matches(&rule.tool, decision, call.tool);
    };
    match (rule.tool.as_str(), target) {
        ("Bash", Target::Command(words)) if call.tool == "bash" => {
            let words = strip_wrappers(words, restrict);
            let exact_only = !restrict && spec.contains('*') && needs_exact_rule(words);
            !exact_only && bash_matches(spec, &raw_text(words))
        }
        ("Read" | "Edit", Target::Path(path)) => {
            let tool_ok = match rule.tool.as_str() {
                "Read" => {
                    READ_TOOLS.contains(&call.tool)
                        || (decision == Decision::Deny && EDIT_TOOLS.contains(&call.tool))
                }
                _ => EDIT_TOOLS.contains(&call.tool),
            };
            let cx = PathContext {
                cwd: call.cwd,
                home: call.home,
                anchor: &rule.anchor,
            };
            tool_ok && path_rule_matches(spec, &cx, path, decision)
        }
        ("Read" | "Edit", Target::File { path, write, .. }) => {
            let tool_ok = match rule.tool.as_str() {
                "Read" => !*write || decision == Decision::Deny,
                _ => *write,
            };
            let cx = PathContext {
                cwd: call.cwd,
                home: call.home,
                anchor: &rule.anchor,
            };
            tool_ok && path_rule_matches(spec, &cx, path, decision)
        }
        ("Agent" | "Task", Target::Call) if call.tool == "subagent" => agent_names(call.input)
            .iter()
            .any(|agent| name_glob(&spec.to_lowercase(), agent)),
        ("WebFetch", Target::Url(host)) => spec
            .strip_prefix("domain:")
            .is_some_and(|d| domain_matches(d, host)),
        (_, Target::Call) if restrict && name_matches(&rule.tool, decision, call.tool) => {
            param_matches(spec, call.input)
        }
        _ => false,
    }
}

fn name_matches(rule_tool: &str, decision: Decision, tool: &str) -> bool {
    let names = claude_names(tool);
    if rule_tool.contains('*') {
        let anchored = rule_tool
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
            .is_some_and(|(server, _)| !server.is_empty() && !server.contains('*'));
        return (decision != Decision::Allow || anchored)
            && names.iter().any(|n| name_glob(rule_tool, n));
    }
    if rule_tool.starts_with("mcp__") && !rule_tool["mcp__".len()..].contains("__") {
        return tool.starts_with(&format!("{rule_tool}__"));
    }
    names.contains(&rule_tool)
}

/// The lowercase agent names of a `subagent` call; `explore` when none is named.
fn agent_names(input: &Value) -> Vec<String> {
    let name = |v: &Value| {
        v.get("agent")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
    };
    let mut names: Vec<String> = input
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|task| {
            name(task)
                .or_else(|| name(input))
                .unwrap_or_else(|| "explore".into())
        })
        .collect();
    if names.is_empty() {
        names.push(name(input).unwrap_or_else(|| "explore".into()));
    }
    names
}

/// `Tool(param:value)` for deny and ask rules; `*` in the value matches any text.
fn param_matches(spec: &str, input: &Value) -> bool {
    let Some((key, want)) = spec.split_once(':') else {
        return false;
    };
    let got = match input.get(key.trim()) {
        Some(Value::String(s)) => s.clone(),
        Some(v @ (Value::Bool(_) | Value::Number(_))) => v.to_string(),
        _ => return false,
    };
    name_glob(want.trim(), &got)
}

/// Rules that "allow always" saves for the targets no allow rule covers.
pub fn suggestions(verdict: &Verdict, tool: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (target, rule) in &verdict.targets {
        if rule.as_ref().is_some_and(|r| r.decision == Decision::Allow) {
            continue;
        }
        let text = match target {
            Target::Command(words) => format!("Bash({})", raw_text(strip_wrappers(words, false))),
            Target::Path(path) => {
                let kind = if EDIT_TOOLS.contains(&tool) {
                    "Edit"
                } else {
                    "Read"
                };
                format!("{kind}(/{})", path.display())
            }
            Target::File { path, write, .. } => {
                let kind = if *write { "Edit" } else { "Read" };
                format!("{kind}(/{})", path.display())
            }
            Target::Url(host) => format!("WebFetch(domain:{host})"),
            Target::Shell(_) => continue,
            Target::Call => claude_names(tool)[0].to_owned(),
        };
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// `/permissions test` input: `Tool(arg)` or a bash command.
pub fn sample_call(sample: &str) -> (String, Value) {
    let sample = sample.trim();
    let tool_form = sample.ends_with(')')
        && sample.find('(').is_some_and(|i| {
            i > 0
                && sample[..i]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if let Some((tool, arg)) =
        split_claude(sample).filter(|_| tool_form || sample.starts_with("mcp__"))
    {
        let arg = arg.unwrap_or_default();
        return match tool.as_str() {
            "Bash" => ("bash".into(), json!({ "command": arg })),
            "Read" => ("read".into(), json!({ "path": arg })),
            "Edit" => ("edit".into(), json!({ "path": arg })),
            "Write" => ("write".into(), json!({ "path": arg })),
            "WebFetch" => ("fetch".into(), json!({ "url": arg })),
            _ => (tool, json!({})),
        };
    }
    ("bash".into(), json!({ "command": sample }))
}

#[cfg(test)]
#[path = "evaluate_tests.rs"]
mod tests;
