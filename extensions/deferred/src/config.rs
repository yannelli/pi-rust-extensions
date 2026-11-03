//! Rules that pick how each tool, skill, context file, and prompt section reaches the model.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

pub const NAMESPACE: &str = "deferred";
pub const FILE_NAME: &str = "deferred.json";
/// The source name of rules from the `deferred` setting.
pub const SETTING_SOURCE: &str = "setting `deferred`";
/// The key of repo-scoped rule blocks.
pub const REPOS: &str = "repos";
pub const DEFAULT_AUTO_PERCENT: f64 = 10.0;
pub const DEFAULT_MAX_RESULTS: usize = 5;

/// How one item reaches the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    /// Loaded as pi would load it.
    #[default]
    On,
    /// Only the name is listed; `deferred_search` loads it.
    Names,
    /// Not listed; `deferred_search` finds and loads it.
    Deferred,
    /// Skills only: out of the prompt and search, but the user can still run `/skill:name`.
    User,
    /// Removed, and not searchable.
    Off,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "on" | "load" | "eager" => Self::On,
            "names" | "name" | "name-only" => Self::Names,
            "deferred" | "defer" | "search" => Self::Deferred,
            "user" | "manual" | "user-only" => Self::User,
            "off" | "disable" | "disabled" => Self::Off,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Names => "names",
            Self::Deferred => "deferred",
            Self::User => "user",
            Self::Off => "off",
        }
    }

    /// The item is searchable: `names` or `deferred`.
    pub fn searchable(self) -> bool {
        matches!(self, Self::Names | Self::Deferred)
    }

    /// `user` is only for skills.
    pub fn fits(self, kind: Kind) -> bool {
        self != Self::User || kind == Kind::Skill
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Tool,
    Skill,
    Context,
    Section,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Tool, Kind::Skill, Kind::Context, Kind::Section];

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "tool" | "tools" => Self::Tool,
            "skill" | "skills" => Self::Skill,
            "context" | "contexts" | "file" | "files" => Self::Context,
            "section" | "sections" => Self::Section,
            _ => return None,
        })
    }

    /// The config key of the kind.
    pub fn key(self) -> &'static str {
        match self {
            Self::Tool => "tools",
            Self::Skill => "skills",
            Self::Context => "context",
            Self::Section => "sections",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Skill => "skill",
            Self::Context => "context file",
            Self::Section => "prompt section",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub kind: Kind,
    pub pattern: String,
    pub mode: Mode,
    /// The config source the rule came from.
    pub source: String,
    /// The `repos` key of a repo-scoped rule.
    pub repo: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Rules of every layer in order; the last match wins.
    pub rules: Vec<Rule>,
    /// Defer extension and MCP tools as names when their schemas pass this share of the
    /// context window; `None` turns it off.
    pub auto_percent: Option<f64>,
    pub max_results: usize,
    /// Problems found while parsing, for `/deferred`.
    pub warnings: Vec<String>,
    /// `owner/name` of the cwd's `origin` remote.
    pub repo: Option<String>,
}

/// The config sources in the order they apply.
#[derive(Default)]
pub struct Layers<'a> {
    pub global_file: Option<(&'a str, &'a Value)>,
    /// The `deferred` setting as pi merges it.
    pub setting: Option<&'a Value>,
    /// The `deferred` block of the project `settings.json`, when pi applies it.
    pub project_setting: Option<&'a Value>,
    pub project_file: Option<(&'a str, &'a Value)>,
    pub repo: Option<&'a str>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            auto_percent: Some(DEFAULT_AUTO_PERCENT),
            max_results: DEFAULT_MAX_RESULTS,
            warnings: Vec::new(),
            repo: None,
        }
    }
}

impl Config {
    /// Layers in order of rising priority; non-objects are skipped.
    pub fn merge<'a>(layers: impl IntoIterator<Item = (&'a str, Option<&'a Value>)>) -> Self {
        let mut config = Self::default();
        for (source, layer) in layers {
            if let Some(obj) = layer.and_then(Value::as_object) {
                config.apply(source, obj);
            }
        }
        config
    }

    /// Global file, global setting, matching `repos` blocks, project setting, project file.
    pub fn load(layers: &Layers) -> Self {
        let mut config = Self {
            repo: layers.repo.map(str::to_owned),
            ..Self::default()
        };
        let (global, project) = split_setting(layers.setting, layers.project_setting);
        let global_file = layers
            .global_file
            .and_then(|(n, v)| Some((n, v.as_object()?)));
        if let Some((name, obj)) = global_file {
            config.apply(name, obj);
        }
        config.apply(SETTING_SOURCE, &global);
        if let Some(repo) = layers.repo {
            if let Some((name, obj)) = global_file {
                config.apply_repos(name, obj, repo);
            }
            if let Some(obj) = layers.setting.and_then(Value::as_object) {
                config.apply_repos(SETTING_SOURCE, obj, repo);
            }
        }
        config.apply(SETTING_SOURCE, &project);
        if let Some(obj) = layers.project_file.and_then(|(_, v)| v.as_object()) {
            config.apply(layers.project_file.map_or("", |(n, _)| n), obj);
        }
        config
    }

    /// The blocks of `repos` whose key matches `repo`, ignoring case, in order.
    fn apply_repos(&mut self, source: &str, obj: &Map<String, Value>, repo: &str) {
        let Some(repos) = obj.get(REPOS) else {
            return;
        };
        let Some(repos) = repos.as_object() else {
            return self.warn(source, format!("`{REPOS}` is not an object"));
        };
        for (key, block) in repos {
            if !glob(&key.to_lowercase(), &repo.to_lowercase()) {
                continue;
            }
            match block.as_object() {
                Some(block) => {
                    let from = self.rules.len();
                    self.apply(source, block);
                    for rule in &mut self.rules[from..] {
                        rule.repo = Some(key.clone());
                    }
                }
                None => self.warn(source, format!("`{REPOS}.{key}` is not an object")),
            }
        }
    }

    fn apply(&mut self, source: &str, obj: &Map<String, Value>) {
        for kind in Kind::ALL {
            let Some(value) = obj.get(kind.key()) else {
                continue;
            };
            let Some(map) = value.as_object() else {
                self.warn(source, format!("`{}` is not an object", kind.key()));
                continue;
            };
            for (pattern, mode) in map {
                match mode.as_str().and_then(Mode::parse) {
                    Some(mode) if !mode.fits(kind) => self.warn(
                        source,
                        format!("`user` is for skills only, not `{pattern}`"),
                    ),
                    Some(mode) => self.rules.push(Rule {
                        kind,
                        pattern: pattern.clone(),
                        mode,
                        source: source.to_owned(),
                        repo: None,
                    }),
                    None => self.warn(source, format!("bad mode {mode} for `{pattern}`")),
                }
            }
        }
        match obj.get("auto") {
            None => {}
            Some(Value::Bool(false)) => self.auto_percent = None,
            Some(Value::Bool(true)) => self.auto_percent = Some(DEFAULT_AUTO_PERCENT),
            Some(value) => match value.as_f64().filter(|n| n.is_finite() && *n >= 0.0) {
                Some(n) => self.auto_percent = (n > 0.0).then_some(n),
                None => self.warn(source, format!("bad `auto` {value}")),
            },
        }
        if let Some(value) = obj.get("maxResults") {
            match value.as_u64().filter(|n| (1..=50).contains(n)) {
                Some(n) => self.max_results = n as usize,
                None => self.warn(source, format!("bad `maxResults` {value}")),
            }
        }
    }

    fn warn(&mut self, source: &str, message: String) {
        self.warnings.push(format!("{source}: {message}"));
    }

    /// The mode of the last rule that matches; `None` when no rule does.
    pub fn mode(&self, kind: Kind, name: &str) -> Option<Mode> {
        self.rule(kind, name).map(|rule| rule.mode)
    }

    /// The last rule that matches.
    pub fn rule(&self, kind: Kind, name: &str) -> Option<&Rule> {
        let matches = |rule: &&Rule| {
            rule.kind == kind
                && match kind {
                    Kind::Context => path_matches(&rule.pattern, name),
                    _ => glob(&rule.pattern, name),
                }
        };
        self.rules.iter().rev().find(matches)
    }
}

/// The merged setting split into the part from the global file and the part the project
/// `settings.json` sets, so repo rules can apply between them.
fn split_setting(
    setting: Option<&Value>,
    project: Option<&Value>,
) -> (Map<String, Value>, Map<String, Value>) {
    let (mut global, mut local) = (Map::new(), Map::new());
    let Some(setting) = setting.and_then(Value::as_object) else {
        return (global, local);
    };
    let in_project = |path: &[&str], value: &Value| {
        let found = path.iter().try_fold(project?, |v, key| v.get(key));
        Some(found? == value)
    };
    for (key, value) in setting {
        let rules = Kind::ALL.iter().any(|k| k.key() == key);
        match value.as_object() {
            Some(map) if rules => {
                for (pattern, mode) in map {
                    let target = match in_project(&[key, pattern], mode) {
                        Some(true) => &mut local,
                        _ => &mut global,
                    };
                    let entry = target.entry(key.clone()).or_insert_with(|| json!({}));
                    if let Some(entry) = entry.as_object_mut() {
                        entry.insert(pattern.clone(), mode.clone());
                    }
                }
            }
            _ if key == REPOS => {}
            _ => {
                let target = match in_project(&[key], value) {
                    Some(true) => &mut local,
                    _ => &mut global,
                };
                target.insert(key.clone(), value.clone());
            }
        }
    }
    (global, local)
}

/// `*` matches any run of characters (`/` too) and `?` one character.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            (pi, ti) = (pi + 1, ti + 1);
        } else if pi < p.len() && p[pi] == '*' {
            (star, mark, pi) = (Some(pi), ti, pi + 1);
        } else if let Some(s) = star {
            (pi, mark) = (s + 1, mark + 1);
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// An absolute or `~` pattern matches the whole path; others match a trailing part of it.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    let pattern = expand_home(pattern);
    if pattern.starts_with('/') {
        return glob(&pattern, path);
    }
    glob(&pattern, path) || glob(&format!("*/{pattern}"), path)
}

fn expand_home(pattern: &str) -> String {
    match (pattern.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if !home.is_empty() => {
            format!("{}/{rest}", home.trim_end_matches('/'))
        }
        _ => pattern.to_owned(),
    }
}

/// File contents as JSON; `None` when the file is missing or does not parse.
pub fn read_file(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn global_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(FILE_NAME)
}

pub fn project_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join(FILE_NAME)
}

/// `block` with `kind.pattern` set to `mode`, or removed when `mode` is `None`.
pub fn edit_rule(block: Option<Value>, kind: Kind, pattern: &str, mode: Option<Mode>) -> Value {
    let mut block = block
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let obj = block.as_object_mut().expect("object");
    let rules = obj
        .entry(kind.key())
        .or_insert_with(|| Value::Object(Map::new()));
    if !rules.is_object() {
        *rules = Value::Object(Map::new());
    }
    let rules = rules.as_object_mut().expect("object");
    rules.shift_remove(pattern);
    if let Some(mode) = mode {
        rules.insert(pattern.to_owned(), Value::String(mode.as_str().to_owned()));
    }
    if rules.is_empty() {
        obj.shift_remove(kind.key());
    }
    block
}

/// `edit_rule` inside `repos.<repo>`; an existing key that differs only in case is reused.
pub fn edit_repo_rule(
    block: Option<Value>,
    repo: &str,
    kind: Kind,
    pattern: &str,
    mode: Option<Mode>,
) -> Value {
    let mut block = block
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let obj = block.as_object_mut().expect("object");
    let repos = obj.entry(REPOS).or_insert_with(|| json!({}));
    if !repos.is_object() {
        *repos = json!({});
    }
    let repos = repos.as_object_mut().expect("object");
    let key = repos
        .keys()
        .find(|k| k.eq_ignore_ascii_case(repo))
        .cloned()
        .unwrap_or_else(|| repo.to_owned());
    let inner = edit_rule(repos.get(&key).cloned(), kind, pattern, mode);
    match inner.as_object().is_some_and(Map::is_empty) {
        true => {
            repos.shift_remove(&key);
        }
        false => {
            repos.insert(key, inner);
        }
    }
    if repos.is_empty() {
        obj.shift_remove(REPOS);
    }
    block
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
