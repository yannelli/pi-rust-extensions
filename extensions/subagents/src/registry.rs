//! Agent definitions: built-in defaults, then `<agentDir>/ext/subagents/agents/*.md`, then
//! `<cwd>/.pi/ext/subagents/agents/*.md`. Later layers override earlier ones per field.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::agent::{AgentDef, AgentPatch, PromptMode, Source};
use crate::defaults::builtin_agents;
use crate::frontmatter::Frontmatter;

pub fn agents_dir(base: &Path) -> PathBuf {
    base.join("ext").join("subagents").join("agents")
}

#[derive(Debug, Clone)]
pub struct Layers {
    pub global: PathBuf,
    pub project: Option<PathBuf>,
}

impl Layers {
    /// `project_base` is `<cwd>/.pi`; `None` skips project agents (untrusted project).
    pub fn new(agent_dir: &Path, project_base: Option<&Path>) -> Self {
        Self {
            global: agents_dir(agent_dir),
            project: project_base.map(agents_dir),
        }
    }

    pub fn dir(&self, source: Source) -> Option<&Path> {
        match source {
            Source::Global => Some(&self.global),
            Source::Project => self.project.as_deref(),
            Source::Builtin => None,
        }
    }
}

pub fn load(layers: &Layers) -> Vec<AgentDef> {
    let mut agents = builtin_agents();
    merge_dir(&mut agents, &layers.global, Source::Global);
    if let Some(project) = &layers.project {
        merge_dir(&mut agents, project, Source::Project);
    }
    agents
}

pub fn merge_dir(agents: &mut Vec<AgentDef>, dir: &Path, source: Source) {
    for path in md_files(dir) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        merge_file(agents, &path, &content, source);
    }
}

pub fn merge_file(agents: &mut Vec<AgentDef>, path: &Path, content: &str, source: Source) {
    let fm = Frontmatter::parse(content);
    let patch = AgentPatch::from_frontmatter(&fm);
    if let Some(raw) = &patch.bad_exposure {
        warn_bad_exposure(path, raw);
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let name = patch.name.clone().unwrap_or_else(|| stem.to_owned());
    let name = sanitize_name(&name);
    if name.is_empty() {
        return;
    }
    let index = agents.iter().position(|agent| agent.name == name);
    let agent = match index {
        Some(index) => &mut agents[index],
        None => {
            agents.push(AgentDef {
                name: name.clone(),
                description: String::new(),
                prompt: String::new(),
                tools: None,
                disallowed_tools: Vec::new(),
                model: None,
                thinking: None,
                max_turns: None,
                prompt_mode: PromptMode::Append,
                exposure: None,
                source,
                builtin: false,
                files: Vec::new(),
            });
            agents.last_mut().expect("pushed")
        }
    };
    patch.apply(agent);
    agent.source = source;
    agent.files.push(path.to_owned());
}

/// Warns once per file and value; agent files load again for each prompt.
fn warn_bad_exposure(path: &Path, raw: &str) {
    static WARNED: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
    let key = (path.to_owned(), raw.to_owned());
    let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    if !warned.contains(&key) {
        pi_ext_sdk::warn!(
            "subagents: {}: exposure must be direct or deferred, not {raw:?}; ignoring it",
            path.display()
        );
        warned.push(key);
    }
}

/// Lowercase letters, digits, `-` and `_`; other characters become `-`.
pub fn sanitize_name(name: &str) -> String {
    let name: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    name.trim_matches('-').to_owned()
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md") && path.is_file())
        .collect();
    files.sort();
    files
}

/// The file for agent `name` in `dir`: the existing file that defines it, else `<name>.md`.
pub fn file_for(dir: &Path, name: &str) -> PathBuf {
    let existing = md_files(dir).into_iter().find(|path| {
        let fm = std::fs::read_to_string(path)
            .map(|c| Frontmatter::parse(&c))
            .unwrap_or_default();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        sanitize_name(&fm.text("name").unwrap_or_else(|| stem.to_owned())) == name
    });
    existing.unwrap_or_else(|| dir.join(format!("{name}.md")))
}

/// Reads the agent file of `name` in `dir` (empty when absent), edits it, and writes it back.
pub fn edit_file(
    dir: &Path,
    name: &str,
    edit: impl FnOnce(&mut Frontmatter),
) -> std::io::Result<PathBuf> {
    let path = file_for(dir, name);
    let mut fm = std::fs::read_to_string(&path)
        .map(|c| Frontmatter::parse(&c))
        .unwrap_or_default();
    if fm.get("name").is_none() {
        fm.fields.insert(
            0,
            (
                "name".into(),
                crate::frontmatter::FmValue::Text(name.into()),
            ),
        );
    }
    edit(&mut fm);
    write_atomic(&path, &fm.render())?;
    Ok(path)
}

/// Writes `text` to a temp file next to `path` and renames it over `path`, so a reader never
/// sees a partial agent file. Creates the parent directory.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("agent");
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}
