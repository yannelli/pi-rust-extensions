//! File changes behind `/agents`: save, create, copy, rename, move, delete, and raw edits.
//! Each write goes through a temp file and a rename.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::{AgentDef, Source};
use crate::frontmatter::{FmValue, Frontmatter};
use crate::registry::{self, Layers, sanitize_name, write_atomic};
use crate::ui::draft::{Draft, Values};

/// An agent with the plugin that staged its top file, when `claude-marketplace` did.
#[derive(Debug, Clone)]
pub struct Entry {
    pub def: AgentDef,
    pub plugin: Option<String>,
    /// Where the agent comes from: built-in, or the scope of its lowest file.
    pub origin: Source,
}

impl Entry {
    /// The file that wins: the last layer.
    pub fn top(&self) -> Option<&Path> {
        self.def.files.last().map(PathBuf::as_path)
    }

    pub fn badge(&self) -> String {
        match &self.plugin {
            Some(plugin) => format!("plugin {plugin}"),
            None => self.origin.label().to_owned(),
        }
    }

    /// The scope of the top file when it overrides a built-in or a lower file.
    pub fn override_scope(&self) -> Option<Source> {
        let overridden = self.def.overridden() || self.def.files.len() > 1;
        overridden.then_some(self.def.source)
    }
}

/// The agents of all layers, with plugin owners from each scope's marketplace state.
pub fn entries(layers: &Layers) -> Vec<Entry> {
    let mut owners = plugin_files(&layers.global);
    if let Some(project) = &layers.project {
        owners.extend(plugin_files(project));
    }
    let agents = registry::load(layers);
    let entry = |def: AgentDef| {
        let top = def.files.last().cloned();
        let plugin = top.and_then(|f| owners.get(&f).cloned());
        let origin = match def.files.first() {
            _ if def.builtin => Source::Builtin,
            Some(first) => scope_of(layers, first),
            None => def.source,
        };
        Entry {
            def,
            plugin,
            origin,
        }
    };
    agents.into_iter().map(entry).collect()
}

/// `<root>/ext/claude-marketplace/state.json` next to `<root>/ext/subagents/agents`: file to plugin.
fn plugin_files(agents_dir: &Path) -> BTreeMap<PathBuf, String> {
    let Some(ext) = agents_dir.parent().and_then(Path::parent) else {
        return BTreeMap::new();
    };
    let state = ext.join("claude-marketplace").join("state.json");
    let Ok(text) = std::fs::read_to_string(state) else {
        return BTreeMap::new();
    };
    let state: Value = serde_json::from_str(&text).unwrap_or_default();
    let mut owners = BTreeMap::new();
    let plugins = state["plugins"].as_object().into_iter().flatten();
    for (key, record) in plugins {
        let name = record["plugin"].as_str().unwrap_or(key).to_owned();
        for file in record["files"].as_array().into_iter().flatten() {
            let Some(file) = file.as_str().map(PathBuf::from) else {
                continue;
            };
            if file.starts_with(agents_dir) {
                owners.insert(file, name.clone());
            }
        }
    }
    owners
}

/// The done line, and the agent to select after the change.
pub type Done = Result<(String, String), String>;

fn io(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |err| format!("Cannot write {}: {err}", path.display())
}

/// Writes the changed fields into the agent's file in `dir`.
pub fn save(dir: &Path, draft: &Draft) -> Done {
    let path = registry::file_for(dir, &draft.name);
    let path = registry::edit_file(dir, &draft.name, |fm| draft.apply(fm)).map_err(io(&path))?;
    Ok((format!("Saved {}", path.display()), draft.name.clone()))
}

/// Checks a new agent name; returns it sanitized.
pub fn check_name(raw: &str, taken: &[Entry]) -> Result<String, String> {
    let name = sanitize_name(raw);
    if name.is_empty() {
        return Err("Type a name: letters, digits, - or _.".into());
    }
    if taken.iter().any(|e| e.def.name == name) {
        return Err(format!("An agent named {name} exists. Pick another name."));
    }
    Ok(name)
}

/// Writes a new agent file `<dir>/<name>.md`; refuses to replace a file.
pub fn create(dir: &Path, name: &str, values: &Values) -> Done {
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        return Err(format!("{} exists.", path.display()));
    }
    write_atomic(&path, &values.file(name).render()).map_err(io(&path))?;
    Ok((format!("Created {}", path.display()), name.to_owned()))
}

/// Renames every file of a custom agent to `<dir>/<to>.md` with `name: <to>`.
pub fn rename(agent: &AgentDef, to: &str) -> Done {
    let mut moves = Vec::new();
    for file in &agent.files {
        let dir = file.parent().unwrap_or(Path::new("."));
        let target = dir.join(format!("{to}.md"));
        if target.exists() && target != *file {
            return Err(format!("{} exists.", target.display()));
        }
        moves.push((file.clone(), target));
    }
    for (from, to_path) in &moves {
        let text = std::fs::read_to_string(from)
            .map_err(|e| format!("Cannot read {}: {e}", from.display()))?;
        let mut fm = Frontmatter::parse(&text);
        let value = FmValue::Text(to.to_owned());
        if fm.get("name").is_some() {
            fm.set("name", Some(value));
        } else {
            fm.fields.insert(0, ("name".into(), value));
        }
        write_atomic(to_path, &fm.render()).map_err(io(to_path))?;
        if from != to_path {
            std::fs::remove_file(from)
                .map_err(|e| format!("Cannot remove {}: {e}", from.display()))?;
        }
    }
    Ok((format!("Renamed {} to {to}", agent.name), to.to_owned()))
}

/// Moves the top file of an agent into `to_dir`; refuses when that scope has the agent.
pub fn move_to(agent: &AgentDef, to_dir: &Path) -> Done {
    let Some(from) = agent.files.last() else {
        return Err(format!(
            "{} has no file yet. Save a change first.",
            agent.name
        ));
    };
    let existing = registry::file_for(to_dir, &agent.name);
    if existing.exists() {
        return Err(format!("{} exists. Delete it first.", existing.display()));
    }
    let name = from
        .file_name()
        .map_or_else(|| format!("{}.md", agent.name).into(), |n| n.to_owned());
    let target = to_dir.join(name);
    let text = std::fs::read_to_string(from)
        .map_err(|e| format!("Cannot read {}: {e}", from.display()))?;
    write_atomic(&target, &text).map_err(io(&target))?;
    std::fs::remove_file(from).map_err(|e| format!("Cannot remove {}: {e}", from.display()))?;
    Ok((
        format!("Moved {} to {}", agent.name, target.display()),
        agent.name.clone(),
    ))
}

pub fn delete(name: &str, files: &[PathBuf], reset: bool) -> Done {
    for file in files {
        std::fs::remove_file(file).map_err(|e| format!("Cannot remove {}: {e}", file.display()))?;
    }
    let what = if reset { "Reset" } else { "Deleted" };
    let list: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    Ok((
        format!("{what} {name}: removed {}", list.join(", ")),
        name.to_owned(),
    ))
}

/// Writes the raw text of an agent file after a check that it still names the agent.
pub fn write_raw(path: &Path, name: &str, text: &str) -> Done {
    let fm = Frontmatter::parse(text);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let named = sanitize_name(&fm.text("name").unwrap_or_else(|| stem.to_owned()));
    if named != name {
        return Err(format!(
            "The file now names {named:?}, not {name}. Rename with r instead."
        ));
    }
    write_atomic(path, text).map_err(io(path))?;
    Ok((format!("Saved {}", path.display()), name.to_owned()))
}

/// The directory of a scope, when it is open to this session.
pub fn scope_dir(layers: &Layers, source: Source) -> Option<PathBuf> {
    layers.dir(source).map(Path::to_path_buf)
}

/// The scope of a file: project when it is under the project agents dir.
pub fn scope_of(layers: &Layers, file: &Path) -> Source {
    match &layers.project {
        Some(project) if file.starts_with(project) => Source::Project,
        _ => Source::Global,
    }
}
