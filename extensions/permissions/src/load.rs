//! Reads every active rule source.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::codex;
use crate::codex_config;
use crate::mode::{self, Mode};
use crate::paths::{Paths, project_root, up_to_root};
use crate::rules::{ClaudeRule, Decision, Kind, Rule, Source, split_claude};
use crate::settings::{Options, PiFile, global_file, project_file};
use std::collections::HashMap;

pub struct Env<'a> {
    pub paths: &'a Paths,
    pub agent_dir: PathBuf,
    pub cwd: PathBuf,
    pub trusted: bool,
}

#[derive(Debug, Default)]
pub struct Loaded {
    pub rules: Vec<Rule>,
    pub options: Options,
    /// Load errors and skipped files or calls.
    pub notes: Vec<String>,
    /// Settings shown in `/permissions` but not applied, such as `defaultMode`.
    pub info: Vec<String>,
    pub global_file: PathBuf,
    pub project_file: PathBuf,
    pub trusted: bool,
    pub cwd: PathBuf,
    /// The cwd and the Claude `additionalDirectories`.
    pub dirs: Vec<PathBuf>,
    /// The Claude `defaultMode` with the highest settings precedence, and its source.
    pub claude_mode: Option<(String, Source)>,
    /// The mode for calls that no rule decides, with its label.
    pub mode: Option<(Mode, String)>,
    hosts: HashMap<String, Vec<String>>,
    /// Codex user and project rules files that failed to parse.
    codex_failed: Vec<PathBuf>,
}

pub fn load(env: &Env) -> Loaded {
    let mut out = Loaded {
        global_file: global_file(&env.agent_dir),
        project_file: project_file(&env.cwd),
        trusted: env.trusted,
        cwd: env.cwd.clone(),
        dirs: vec![env.cwd.clone()],
        ..Loaded::default()
    };
    let global = pi_file(&out.global_file, &mut out.notes);
    let project = env
        .trusted
        .then(|| pi_file(&out.project_file, &mut out.notes));
    out.options = Options::merge(&global, project.as_ref());
    let managed_only = out.options.on(Source::ClaudeManaged)
        && read_json(&env.paths.claude_managed).is_some_and(|json| {
            json["allowManagedPermissionRulesOnly"] == true
                || json["permissions"]["allowManagedPermissionRulesOnly"] == true
        });
    if managed_only {
        out.notes.push(
            "allowManagedPermissionRulesOnly: Claude user, project, and local rules are ignored"
                .to_owned(),
        );
    }
    let mut skipped_project = false;
    for source in Source::ALL {
        if !out.options.on(source) {
            continue;
        }
        if source.is_project() && !env.trusted {
            skipped_project = true;
            continue;
        }
        match source {
            Source::PiGlobal => pi_rules(&mut out, source, &global, &env.cwd),
            Source::PiProject => {
                if let Some(project) = &project {
                    pi_rules(&mut out, source, project, &env.cwd);
                }
            }
            Source::CodexManaged => codex_requirements(&mut out, env),
            Source::CodexUser | Source::CodexProject => codex_source(&mut out, source, env),
            _ => claude_source(&mut out, source, env, managed_only),
        }
    }
    if skipped_project {
        out.notes
            .push("project is not trusted; project sources are skipped".to_owned());
    }
    drop_codex_rules_after_a_failure(&mut out);
    let hosts = std::mem::take(&mut out.hosts);
    attach_hosts(&mut out.rules, &hosts);
    out.mode = mode::effective(out.options.mode.as_deref(), out.claude_mode.as_ref());
    match (&out.mode, &out.claude_mode) {
        (Some((_, label)), _) => out.info.push(format!("mode for unmatched calls: {label}")),
        (None, Some((name, _))) => out.info.push(format!(
            "Claude defaultMode {name} is not applied: a pi file sets \"defaultMode\": \"pi\""
        )),
        _ => {}
    }
    out
}

fn pi_file(path: &Path, notes: &mut Vec<String>) -> PiFile {
    PiFile::load(path).unwrap_or_else(|err| {
        notes.push(err);
        PiFile::default()
    })
}

fn pi_rules(out: &mut Loaded, source: Source, file: &PiFile, cwd: &Path) {
    let path = match source {
        Source::PiGlobal => out.global_file.clone(),
        _ => out.project_file.clone(),
    };
    for decision in Decision::ALL {
        for text in file.list(decision) {
            push_claude(out, source, decision, text, &path, cwd);
        }
    }
}

fn push_claude(
    out: &mut Loaded,
    source: Source,
    decision: Decision,
    text: &str,
    file: &Path,
    anchor: &Path,
) {
    let Some((tool, spec)) = split_claude(text) else {
        out.notes
            .push(format!("{}: cannot read rule {text:?}", file.display()));
        return;
    };
    out.rules.push(Rule {
        source,
        decision,
        text: text.to_owned(),
        file: file.to_path_buf(),
        justification: None,
        kind: Kind::Claude(ClaudeRule {
            tool,
            spec,
            anchor: anchor.to_path_buf(),
        }),
    });
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Settings precedence of the Claude sources: managed, local, project, user.
fn claude_precedence(source: Source) -> u8 {
    match source {
        Source::ClaudeManaged => 3,
        Source::ClaudeLocal => 2,
        Source::ClaudeProject => 1,
        _ => 0,
    }
}

fn claude_source(out: &mut Loaded, source: Source, env: &Env, managed_only: bool) {
    let root = project_root(&env.cwd, ".claude");
    let (file, anchor) = match source {
        Source::ClaudeManaged => {
            let file = env.paths.claude_managed.clone();
            let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
            (file, dir)
        }
        Source::ClaudeUser => (
            env.paths.claude_dir.join("settings.json"),
            env.paths.claude_dir.clone(),
        ),
        Source::ClaudeProject => (root.join(".claude/settings.json"), env.cwd.clone()),
        _ => (root.join(".claude/settings.local.json"), env.cwd.clone()),
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let json: Value = match serde_json::from_str(&text) {
        Ok(json) => json,
        Err(err) => {
            out.notes.push(format!("{}: {err}", file.display()));
            return;
        }
    };
    let permissions = &json["permissions"];
    let rules_ignored = managed_only && source != Source::ClaudeManaged;
    for decision in Decision::ALL.into_iter().filter(|_| !rules_ignored) {
        let list = permissions[decision.name()]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for text in list.iter().filter_map(Value::as_str) {
            push_claude(out, source, decision, text, &file, &anchor);
        }
    }
    let mut info = Vec::new();
    if let Some(mode) = permissions["defaultMode"].as_str() {
        info.push(format!("defaultMode={mode}"));
        if matches!(source, Source::ClaudeProject | Source::ClaudeLocal)
            && matches!(mode, "auto" | "bypassPermissions")
        {
            out.notes.push(format!(
                "{}: defaultMode {mode} is ignored; set it in user or managed settings",
                file.display()
            ));
        } else {
            let current = out.claude_mode.as_ref().map(|(_, s)| claude_precedence(*s));
            if current.is_none_or(|rank| claude_precedence(source) > rank) {
                out.claude_mode = Some((mode.to_owned(), source));
            }
        }
    }
    if let Some(dirs) = permissions["additionalDirectories"].as_array() {
        let dirs: Vec<&str> = dirs.iter().filter_map(Value::as_str).collect();
        info.push(format!("additionalDirectories={}", dirs.join(",")));
        for dir in dirs {
            let path = match dir.strip_prefix("~/") {
                Some(rest) => env.paths.home.join(rest),
                None => env.cwd.join(dir),
            };
            let path = crate::pattern::normalize(&path);
            if !out.dirs.contains(&path) {
                out.dirs.push(path);
            }
        }
    }
    if !info.is_empty() {
        out.info
            .push(format!("{}: {}", source.label(), info.join("  ")));
    }
}

fn codex_source(out: &mut Loaded, source: Source, env: &Env) {
    let dirs: Vec<PathBuf> = if source == Source::CodexUser {
        let root = project_root(&env.cwd, ".git");
        let config = codex_config::read_config(&env.paths.codex_home.join("config.toml"), &root);
        let fields = [
            ("approval_policy", config.approval_policy),
            ("sandbox_mode", config.sandbox_mode),
            ("trust_level", config.trust_level),
        ];
        let shown: Vec<String> = fields
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| format!("{k}={v}")))
            .collect();
        if !shown.is_empty() {
            out.info.push(format!("codex: {}", shown.join("  ")));
        }
        vec![env.paths.codex_home.join("rules")]
    } else {
        let mut dirs = up_to_root(&env.cwd);
        dirs.reverse();
        dirs.into_iter().map(|d| d.join(".codex/rules")).collect()
    };
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "rules"))
            .collect();
        files.sort();
        for file in files {
            codex_file(out, source, &file);
        }
    }
}

fn attach_hosts(rules: &mut [Rule], hosts: &HashMap<String, Vec<String>>) {
    for rule in rules {
        if let Kind::Codex(prefix) = &mut rule.kind {
            let name = prefix.pattern.first().and_then(|alts| alts.first());
            prefix.hosts = name.and_then(|n| hosts.get(n)).cloned();
        }
    }
}

fn codex_requirements(out: &mut Loaded, env: &Env) {
    for file in &env.paths.codex_requirements {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        match crate::requirements::parse_requirements(&text) {
            Ok(rules) => out.rules.extend(rules.into_iter().map(|r| Rule {
                source: Source::CodexManaged,
                decision: r.decision,
                text: r.text,
                file: file.clone(),
                justification: r.justification,
                kind: Kind::Codex(r.rule),
            })),
            Err(err) => out.notes.push(format!("{}: {err}", file.display())),
        }
        return;
    }
}

fn codex_file(out: &mut Loaded, source: Source, file: &Path) {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    match codex::parse_rules(&text) {
        Ok(parsed) => {
            for name in parsed.skipped {
                out.notes
                    .push(format!("{}: {name} is not applied", file.display()));
            }
            for (name, paths) in parsed.hosts {
                out.hosts.entry(name).or_default().extend(paths);
            }
            out.rules.extend(parsed.network.into_iter().map(
                |(rule, decision, justification, text)| Rule {
                    source,
                    decision,
                    text,
                    file: file.to_path_buf(),
                    justification,
                    kind: Kind::Network(rule),
                },
            ));
            out.rules.extend(parsed.rules.into_iter().map(|r| Rule {
                source,
                decision: r.decision,
                text: r.text,
                file: file.to_path_buf(),
                justification: r.justification,
                kind: Kind::Codex(r.rule),
            }));
        }
        Err(err) => {
            out.notes.push(format!("{}: {err}", file.display()));
            out.codex_failed.push(file.to_path_buf());
        }
    }
}

/// Like Codex `load_exec_policy_with_warning` (openai/codex 67f0994b `core/src/exec_policy.rs`): one bad
/// user or project rules file drops every user and project rule and `host_executable`, and keeps managed rules.
fn drop_codex_rules_after_a_failure(out: &mut Loaded) {
    if out.codex_failed.is_empty() {
        return;
    }
    out.rules
        .retain(|rule| !matches!(rule.source, Source::CodexUser | Source::CodexProject));
    out.hosts.clear();
    let files: Vec<String> = out
        .codex_failed
        .iter()
        .map(|f| f.display().to_string())
        .collect();
    out.notes.push(format!(
        "Codex user and project rules are not applied because these files have errors: {}",
        files.join(", ")
    ));
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "load_mode_tests.rs"]
mod mode_settings_tests;
