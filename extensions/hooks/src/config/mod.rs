pub mod codex;
pub mod options;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{HookDef, Plugin, Scope, Source};
use crate::paths::{Paths, project_root};
pub use options::{Options, ScopeOption, read_json, write_option};

pub struct LoadInput<'a> {
    pub paths: &'a Paths,
    pub agent_dir: &'a Path,
    pub cwd: &'a Path,
    pub project_trusted: bool,
}

#[derive(Debug, Default)]
pub struct Loaded {
    pub options: Options,
    pub hooks: Vec<HookDef>,
    /// Project files skipped because the project is not trusted.
    pub untrusted: Vec<PathBuf>,
    pub global_file: PathBuf,
    pub project_file: PathBuf,
}

pub fn options_files(agent_dir: &Path, cwd: &Path) -> (PathBuf, PathBuf) {
    let global = agent_dir.join("ext").join("hooks.json");
    let project = cwd.join(".pi").join("ext").join("hooks.json");
    (global, project)
}

pub fn load(input: &LoadInput) -> Loaded {
    let (global_file, project_file) = options_files(input.agent_dir, input.cwd);
    let mut loaded = Loaded {
        global_file: global_file.clone(),
        project_file: project_file.clone(),
        ..Loaded::default()
    };
    let global_doc = read_json(&global_file).unwrap_or(Value::Null);
    let mut project_doc =
        project_layer(input, &project_file, &mut loaded).and_then(|p| read_json(&p));
    if let Some(Value::Object(doc)) = project_doc.as_mut()
        && doc.remove("trustAll").is_some()
    {
        pi_ext_sdk::info!("hooks: ignoring trustAll in {}", project_file.display());
    }
    let layers: Vec<&Value> = [Some(&global_doc), project_doc.as_ref()]
        .into_iter()
        .flatten()
        .collect();
    loaded.options = Options::merge(&layers);
    let options = loaded.options.clone();
    let scoped = |scope: Scope| options.scope.allows(scope);

    if options.claude {
        let root = project_root(input.cwd, ".claude");
        let mut files = vec![(Scope::Global, input.paths.claude_dir.join("settings.json"))];
        if scoped(Scope::Project) {
            for name in ["settings.json", "settings.local.json"] {
                let path = root.join(".claude").join(name);
                if same_path(&path, &files[0].1) {
                    continue;
                }
                if let Some(path) = project_layer(input, &path, &mut loaded) {
                    files.push((Scope::Project, path));
                }
            }
        }
        let docs: Vec<(Scope, PathBuf, Value)> = files
            .into_iter()
            .filter(|(scope, _)| scoped(*scope))
            .filter_map(|(scope, path)| read_json(&path).map(|doc| (scope, path, doc)))
            .collect();
        let disabled = docs
            .iter()
            .any(|(_, _, doc)| doc.get("disableAllHooks") == Some(&Value::Bool(true)));
        if disabled {
            pi_ext_sdk::info!("hooks: disableAllHooks is set; Claude hooks are off");
        } else {
            for (scope, path, doc) in docs {
                if let Some(hooks) = doc.get("hooks") {
                    loaded
                        .hooks
                        .extend(parse_hooks(Source::Claude, scope, &path, hooks));
                }
            }
        }
    }

    if options.codex {
        let global = scoped(Scope::Global).then(|| codex::load_layer(&input.paths.codex_home));
        let root = project_root(input.cwd, ".codex").join(".codex");
        let project = (scoped(Scope::Project) && !same_path(&root, &input.paths.codex_home))
            .then(|| {
                let any = ["hooks.json", "config.toml"]
                    .iter()
                    .filter_map(|name| project_layer(input, &root.join(name), &mut loaded))
                    .count();
                (any > 0).then(|| codex::load_layer(&root))
            })
            .flatten();
        let feature = project
            .as_ref()
            .and_then(|l| l.feature)
            .or(global.as_ref().and_then(|l| l.feature));
        if feature == Some(false) {
            pi_ext_sdk::info!("hooks: Codex [features] hooks = false; Codex hooks are off");
        } else {
            let layers = [(Scope::Global, global), (Scope::Project, project)];
            for (scope, layer) in layers {
                for (path, hooks) in layer.map(|l| l.hooks).unwrap_or_default() {
                    loaded
                        .hooks
                        .extend(parse_hooks(Source::Codex, scope, &path, &hooks));
                }
            }
        }
    }

    if options.pi {
        let docs = [
            (Scope::Global, &global_file, Some(&global_doc)),
            (Scope::Project, &project_file, project_doc.as_ref()),
        ];
        for (scope, path, doc) in docs {
            if let Some(hooks) = doc.filter(|_| scoped(scope)).and_then(|d| d.get("hooks")) {
                loaded
                    .hooks
                    .extend(parse_hooks(Source::Pi, scope, path, hooks));
            }
        }
    }
    loaded
}

/// `Some(path)` when the project file exists and the project is trusted; records skipped files.
fn project_layer(input: &LoadInput, path: &Path, loaded: &mut Loaded) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    if input.project_trusted {
        Some(path.to_path_buf())
    } else {
        loaded.untrusted.push(path.to_path_buf());
        None
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Reads the Claude settings shape: event -> matcher groups -> handlers.
pub fn parse_hooks(source: Source, scope: Scope, file: &Path, hooks: &Value) -> Vec<HookDef> {
    let Some(events) = hooks.as_object() else {
        return Vec::new();
    };
    let mut defs = Vec::new();
    for (event, groups) in events {
        for group in groups.as_array().into_iter().flatten() {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let field = |key: &str| group.get(key).and_then(Value::as_str).map(str::to_owned);
            let plugin = Plugin {
                id: field("plugin"),
                root: field("pluginRoot"),
                data: field("pluginData"),
            };
            let has_plugin = plugin.id.is_some() || plugin.root.is_some() || plugin.data.is_some();
            for handler in group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if !handler.is_object() {
                    continue;
                }
                let mut def = HookDef::new(
                    source,
                    scope,
                    file.to_path_buf(),
                    event,
                    matcher.clone(),
                    handler.clone(),
                );
                def.plugin = has_plugin.then(|| plugin.clone());
                defs.push(def);
            }
        }
    }
    defs
}

#[cfg(test)]
mod tests;
