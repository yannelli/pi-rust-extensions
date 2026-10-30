//! Reads every active rule source.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::codex;
use crate::codex_config;
use crate::paths::{Paths, project_root, up_to_root};
use crate::rules::{ClaudeRule, Decision, Kind, Rule, Source, split_claude};
use crate::settings::{Options, PiFile, global_file, project_file};

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
}

pub fn load(env: &Env) -> Loaded {
    let mut out = Loaded {
        global_file: global_file(&env.agent_dir),
        project_file: project_file(&env.cwd),
        trusted: env.trusted,
        cwd: env.cwd.clone(),
        ..Loaded::default()
    };
    let global = pi_file(&out.global_file, &mut out.notes);
    let project = env
        .trusted
        .then(|| pi_file(&out.project_file, &mut out.notes));
    out.options = Options::merge(&global, project.as_ref());
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
            Source::CodexUser | Source::CodexProject => codex_source(&mut out, source, env),
            _ => claude_source(&mut out, source, env),
        }
    }
    if skipped_project {
        out.notes
            .push("project is not trusted; project sources are skipped".to_owned());
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

fn claude_source(out: &mut Loaded, source: Source, env: &Env) {
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
    for decision in Decision::ALL {
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
    }
    if let Some(dirs) = permissions["additionalDirectories"].as_array() {
        let dirs: Vec<&str> = dirs.iter().filter_map(Value::as_str).collect();
        info.push(format!("additionalDirectories={}", dirs.join(",")));
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

fn codex_file(out: &mut Loaded, source: Source, file: &Path) {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    match codex::parse_rules(&text) {
        Ok(parsed) => {
            for name in parsed.skipped {
                out.notes
                    .push(format!("{}: {name} is not applied", file.display()));
            }
            out.rules.extend(parsed.rules.into_iter().map(|r| Rule {
                source,
                decision: r.decision,
                text: r.text,
                file: file.to_path_buf(),
                justification: r.justification,
                kind: Kind::Codex(r.rule),
            }));
        }
        Err(err) => out.notes.push(format!("{}: {err}", file.display())),
    }
}
