pub mod codex;
mod codex_bundled;
mod codex_cloud;
mod codex_managed;
pub(crate) mod codex_plugin_bundle;
mod codex_plugins;
pub mod managed;
pub mod options;

use std::path::{Path, PathBuf};

use pi_ext_sdk::paths::project_path;
use serde_json::Value;

use crate::exec::http::HttpPolicy;
use crate::model::{HookDef, Layer, Plugin, Scope, Source};
use crate::paths::{Paths, project_root};
use crate::policy::{Conflict, cross_source_duplicates, find_conflicts};
pub use options::{Options, ScopeOption, read_json, write_option};

/// Options a project file cannot set, because the file can come from the repository.
const GLOBAL_ONLY: [&str; 2] = ["trustAll", "conflicts"];

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
    pub http: HttpPolicy,
    /// Event and matcher keys that different hooks from more than one layer define.
    pub conflicts: Vec<Conflict>,
    /// Commands that Claude or pi config and Codex config both register for one event.
    pub duplicates: Vec<String>,
}

pub fn options_files(agent_dir: &Path, cwd: &Path) -> (PathBuf, PathBuf) {
    let global = agent_dir.join("ext").join("hooks.json");
    let project = pi_ext_sdk::paths::project_ext_config(cwd, "hooks");
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
    if let Some(Value::Object(doc)) = project_doc.as_mut() {
        for key in GLOBAL_ONLY {
            if doc.remove(key).is_some() {
                pi_ext_sdk::info!("hooks: ignoring {key} in {}", project_file.display());
            }
        }
    }
    let layers: Vec<&Value> = [Some(&global_doc), project_doc.as_ref()]
        .into_iter()
        .flatten()
        .collect();
    loaded.options = Options::merge(&layers);
    loaded.options.switches = options::read_switches(&global_doc, options::SWITCHES);
    let options = loaded.options.clone();
    let claude_disabled = options.claude && load_claude(input, &options, &mut loaded);
    if options.codex {
        load_codex(input, &options, &mut loaded);
    }
    if options.pi {
        let docs = [
            (Scope::Global, &global_file, Some(&global_doc)),
            (Scope::Project, &project_file, project_doc.as_ref()),
        ];
        for (scope, path, doc) in docs {
            let doc = doc.filter(|_| options.scope.allows(scope));
            if let Some(hooks) = doc.and_then(|d| d.get("hooks")) {
                let defs = parse_hooks(Source::Pi, scope, path, hooks);
                // Plugin hooks come from Claude Code plugins, so Claude `disableAllHooks` covers them.
                loaded.hooks.extend(
                    defs.into_iter()
                        .filter(|def| !(claude_disabled && def.plugin.is_some())),
                );
            }
        }
    }
    loaded.conflicts = find_conflicts(&loaded.hooks);
    loaded.duplicates = cross_source_duplicates(&loaded.hooks);
    loaded
}

fn last_bool<'a>(docs: impl Iterator<Item = &'a Value>, key: &str) -> Option<bool> {
    docs.filter_map(|doc| doc.get(key).and_then(Value::as_bool))
        .last()
}

fn with_layer(mut defs: Vec<HookDef>, layer: Layer) -> Vec<HookDef> {
    for def in &mut defs {
        def.layer = layer;
    }
    defs
}

/// Claude user, project, local, and managed settings. Returns `true` when the settings turn off
/// the non-managed Claude hooks.
fn load_claude(input: &LoadInput, options: &Options, loaded: &mut Loaded) -> bool {
    let scoped = |scope: Scope| options.scope.allows(scope);
    let root = project_root(input.cwd, ".claude");
    let mut files = vec![(Layer::Global, input.paths.claude_dir.join("settings.json"))];
    if scoped(Scope::Project) {
        let names = [
            ("settings.json", Layer::Project(0)),
            ("settings.local.json", Layer::Local),
        ];
        for (name, layer) in names {
            let path = project_path(&root, Path::new(".claude").join(name));
            if same_path(&path, &files[0].1) {
                continue;
            }
            if let Some(path) = project_layer(input, &path, loaded) {
                files.push((layer, path));
            }
        }
    }
    let docs: Vec<(Layer, PathBuf, Value)> = files
        .into_iter()
        .filter(|(layer, _)| *layer != Layer::Global || scoped(Scope::Global))
        .filter_map(|(layer, path)| read_json(&path).map(|doc| (layer, path, doc)))
        .collect();
    let policy = input
        .paths
        .claude_policy
        .as_deref()
        .map(managed::claude_policy_docs)
        .unwrap_or_default();
    let policy_docs = || policy.iter().map(|(_, doc)| doc);
    let user_docs = || docs.iter().map(|(_, _, doc)| doc);
    loaded.http = HttpPolicy::merge(policy_docs().chain(user_docs()));
    let policy_disable = last_bool(policy_docs(), "disableAllHooks");
    let managed_only = last_bool(policy_docs(), "allowManagedHooksOnly") == Some(true);
    let all_off = policy_disable == Some(true);
    let user_off =
        policy_disable.is_none() && last_bool(user_docs(), "disableAllHooks") == Some(true);
    let others_off = all_off || managed_only || user_off;
    if all_off {
        pi_ext_sdk::info!("hooks: managed disableAllHooks is set; Claude hooks are off");
    } else if others_off {
        pi_ext_sdk::info!("hooks: Claude settings allow only managed Claude hooks");
    }
    if !all_off {
        for (path, doc) in &policy {
            if let Some(hooks) = doc.get("hooks") {
                let defs = parse_hooks(Source::Claude, Scope::Global, path, hooks);
                loaded.hooks.extend(with_layer(defs, Layer::Managed));
            }
        }
    }
    if !others_off {
        for (layer, path, doc) in &docs {
            if let Some(hooks) = doc.get("hooks") {
                let scope = if *layer == Layer::Global {
                    Scope::Global
                } else {
                    Scope::Project
                };
                let defs = parse_hooks(Source::Claude, scope, path, hooks);
                loaded.hooks.extend(with_layer(defs, *layer));
            }
        }
    }
    others_off
}

/// Codex managed layers (system dir, legacy file, MDM, requirements), cloud, user, project,
/// and plugin layers. Each managed method, the cloud cache, and plugin loading have a switch.
fn load_codex(input: &LoadInput, options: &Options, loaded: &mut Loaded) {
    let scoped = |scope: Scope| options.scope.allows(scope);
    let mut sources = codex_managed::ManagedSources::load(input.paths, options);
    let mut layers: Vec<(Scope, Layer, codex::CodexLayer)> = Vec::new();
    if let Some(system) = input.paths.codex_system.as_deref()
        && options.switch_on(options::CODEX_SYSTEM)
    {
        layers.push((Scope::Global, Layer::Managed, codex::load_layer(system)));
    }
    if scoped(Scope::Global) {
        let cloud = std::mem::take(&mut sources.cloud_configs);
        layers.extend(cloud.into_iter().map(|l| (Scope::Global, Layer::Global, l)));
        let user = codex::load_layer(&input.paths.codex_home);
        layers.push((Scope::Global, Layer::Global, user));
    }
    if scoped(Scope::Project) {
        let markers = managed::codex_root_markers(&input.paths.codex_home);
        for (depth, dir) in codex_project_dirs(input.cwd, &markers)
            .into_iter()
            .enumerate()
        {
            if same_path(&dir, &input.paths.codex_home) {
                continue;
            }
            let any = ["hooks.json", "config.toml"]
                .iter()
                .filter_map(|name| project_layer(input, &dir.join(name), loaded))
                .count();
            if any > 0 {
                let layer = codex::load_layer(&dir);
                layers.push((Scope::Project, Layer::Project(depth), layer));
            }
        }
    }
    for layer in std::mem::take(&mut sources.top_configs) {
        layers.push((Scope::Global, Layer::Managed, layer));
    }
    let pinned = sources.pin(|r| r.feature);
    let feature = pinned.or_else(|| layers.iter().rev().find_map(|(_, _, l)| l.feature));
    if feature == Some(false) {
        pi_ext_sdk::info!("hooks: Codex [features] hooks = false; Codex hooks are off");
        return;
    }
    let managed_only = sources.pin(|r| r.managed_only) == Some(true);
    for req in sources.requirements.iter().rev() {
        if let Some(hooks) = &req.hooks {
            let defs = parse_hooks(Source::Codex, Scope::Global, &req.file, hooks);
            loaded.hooks.extend(with_layer(defs, Layer::Managed));
        }
    }
    let states = codex::hook_states(&input.paths.codex_home);
    let configs: Vec<&Value> = layers
        .iter()
        .filter_map(|(_, _, l)| l.config.as_ref())
        .collect();
    let plugins = (!managed_only
        && scoped(Scope::Global)
        && options.switch_on(options::CODEX_PLUGINS))
    .then(|| {
        let pinned = sources.pin(|r| r.plugins);
        codex_plugins::load(&input.paths.codex_home, &configs, pinned, &states)
    });
    for (scope, layer, codex_layer) in layers {
        if managed_only && layer != Layer::Managed {
            continue;
        }
        for (path, hooks) in codex_layer.hooks {
            let defs = with_layer(parse_hooks(Source::Codex, scope, &path, &hooks), layer);
            loaded.hooks.extend(defs.into_iter().filter(|def| {
                layer == Layer::Managed || states.get(&def.codex_state_key()) != Some(&false)
            }));
        }
    }
    loaded.hooks.extend(plugins.into_iter().flatten());
}

/// `.codex` dirs from the project root down to `cwd`; Codex loads each one. The root is the
/// nearest dir with one of `markers` (Codex `project_root_markers`); no markers means `cwd`.
fn codex_project_dirs(cwd: &Path, markers: &[String]) -> Vec<PathBuf> {
    let root = cwd
        .ancestors()
        .find(|dir| markers.iter().any(|marker| dir.join(marker).exists()))
        .unwrap_or(cwd);
    let mut dirs: Vec<PathBuf> = cwd
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .map(|dir| project_path(dir, ".codex"))
        .filter(|dir| dir.is_dir())
        .collect();
    dirs.reverse();
    dirs
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

/// Prefix of the only variables a group's `pluginEnv` can set.
pub const PLUGIN_OPTION_PREFIX: &str = "CLAUDE_PLUGIN_OPTION_";

/// The string values of `pluginEnv` whose names start with [`PLUGIN_OPTION_PREFIX`].
fn plugin_env(group: &Value) -> Vec<(String, String)> {
    let map = group.get("pluginEnv").and_then(Value::as_object);
    map.into_iter()
        .flatten()
        .filter(|(key, _)| key.starts_with(PLUGIN_OPTION_PREFIX))
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect()
}

/// Reads the Claude settings shape: event -> matcher groups -> handlers.
pub fn parse_hooks(source: Source, scope: Scope, file: &Path, hooks: &Value) -> Vec<HookDef> {
    let Some(events) = hooks.as_object() else {
        return Vec::new();
    };
    let mut defs = Vec::new();
    for (event, groups) in events {
        for (group_index, group) in groups.as_array().into_iter().flatten().enumerate() {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let field = |key: &str| group.get(key).and_then(Value::as_str).map(str::to_owned);
            let plugin = Plugin {
                id: field("plugin"),
                root: field("pluginRoot"),
                data: field("pluginData"),
                env: plugin_env(group),
            };
            let has_plugin = plugin.id.is_some() || plugin.root.is_some() || plugin.data.is_some();
            for (handler_index, handler) in group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
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
                def.position = (group_index, handler_index);
                defs.push(def);
            }
        }
    }
    defs
}

#[cfg(test)]
mod tests;
