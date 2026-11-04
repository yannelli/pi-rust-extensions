//! Plugin settings from `ext/claude-marketplace.json`. Project installs read the project, then
//! the user layers; user installs are shared by all projects, so they read the user layers only.

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use crate::config::Config;
use crate::env::{Env, Scope};
use crate::state::split_id;

/// Component kinds that `paths` and `extraPaths` accept.
pub const PATH_KINDS: [&str; 8] = [
    "commands",
    "agents",
    "skills",
    "hooks",
    "mcpServers",
    "lspServers",
    "outputStyles",
    "bin",
];

/// How `$N` in a command counts arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArgIndex {
    /// `$0` is the first argument if the file uses `$0`, `$ARGUMENTS[N]`, or `arguments`.
    #[default]
    Auto,
    Zero,
    One,
}

impl ArgIndex {
    pub fn parse(text: &str) -> Result<ArgIndex> {
        match text {
            "auto" => Ok(ArgIndex::Auto),
            "zero" | "0" => Ok(ArgIndex::Zero),
            "one" | "1" => Ok(ArgIndex::One),
            other => bail!("argumentIndex \"{other}\": use auto, zero, or one"),
        }
    }
}

/// One `agentModels` rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rule {
    pub plugin: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Output: `provider/model`, `model`, or `inherit`.
    pub to_model: Option<String>,
    pub thinking: Option<String>,
    /// Where the rule comes from, for `info`.
    pub source: String,
}

/// `*` matches any run of characters, `?` one character. Case-insensitive.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

impl Rule {
    fn parse(value: &Value, source: String) -> Result<Rule> {
        let Value::Object(map) = value else {
            bail!("{source}: an agentModels rule is not an object");
        };
        let text =
            |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_str).map(str::to_owned);
        let empty = Map::new();
        let matcher = map
            .get("match")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let model = match (text(map, "provider"), text(map, "model")) {
            (Some(p), Some(m)) if !m.contains('/') && m != "inherit" => Some(format!("{p}/{m}")),
            (Some(_), None) => bail!("{source}: a rule with provider needs model"),
            (_, model) => model,
        };
        Ok(Rule {
            plugin: text(matcher, "plugin"),
            agent: text(matcher, "agent"),
            model: text(matcher, "model"),
            effort: text(matcher, "effort"),
            to_model: model,
            thinking: text(map, "thinking"),
            source,
        })
    }

    /// `agents` are the Claude and the staged agent names.
    pub fn matches(&self, plugin: &str, agents: &[&str], model: &str, effort: &str) -> bool {
        let ok = |pattern: &Option<String>, values: &[&str]| {
            pattern
                .as_deref()
                .is_none_or(|p| values.iter().any(|v| glob(p, v)))
        };
        ok(&self.plugin, &[plugin])
            && ok(&self.agent, agents)
            && ok(&self.model, &[model])
            && ok(&self.effort, &[effort])
    }
}

/// The merged settings of one plugin.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// The `strict` override and the layer that set it.
    pub strict: Option<(bool, String)>,
    /// `paths.<kind>`: replaces what the manifest declares for that kind.
    pub paths: Map<String, Value>,
    /// `extraPaths.<kind>`: added to the resolved paths.
    pub extra_paths: Map<String, Value>,
    /// Extra env: the most specific layer wins for each name.
    pub env: Vec<(String, String)>,
    pub arg_index: (ArgIndex, String),
    /// `agentModels` rules, in match order.
    pub rules: Vec<Rule>,
}

struct Layer {
    label: String,
    map: Map<String, Value>,
    plugin: bool,
}

fn layers(env: &Env, scope: Scope, id: &str) -> Result<Vec<Layer>> {
    let (_, mp) = split_id(id)?;
    let scopes = match scope {
        Scope::Project => vec![Scope::Project, Scope::User],
        Scope::User => vec![Scope::User],
    };
    let mut out = Vec::new();
    for scope in scopes {
        let config = Config::load(&env.dirs(scope))?;
        let section = |key: &str, name: &str| {
            let entry = config.get(key).and_then(|s| s.get(name));
            entry
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        let layer = |label: &str, map, plugin| Layer {
            label: format!("{scope} {label} config"),
            map,
            plugin,
        };
        out.push(layer("plugin", section("plugins", id), true));
        out.push(layer("marketplace", section("marketplaces", mp), false));
        out.push(layer("default", config.top().clone(), false));
    }
    Ok(out)
}

fn string_map(value: &Value, what: &str) -> Result<Vec<(String, String)>> {
    let Value::Object(map) = value else {
        bail!("{what} is not an object");
    };
    let mut out = Vec::new();
    for (key, value) in map {
        let Some(text) = value.as_str() else {
            bail!("{what}.{key} is not a string");
        };
        out.push((key.clone(), text.to_owned()));
    }
    Ok(out)
}

/// The settings of plugin `id` installed in `scope`.
pub fn load(env: &Env, scope: Scope, id: &str) -> Result<Settings> {
    let mut out = Settings {
        arg_index: (ArgIndex::Auto, "default".into()),
        ..Settings::default()
    };
    let layers = layers(env, scope, id)?;
    let mut arg_set = false;
    for layer in &layers {
        let map = &layer.map;
        if let (None, Some(value)) = (&out.strict, map.get("strict")) {
            let Some(strict) = value.as_bool() else {
                bail!("{}: strict is not true or false", layer.label);
            };
            out.strict = Some((strict, layer.label.clone()));
        }
        if let (false, Some(value)) = (arg_set, map.get("argumentIndex")) {
            let text = value.as_str().unwrap_or_default();
            out.arg_index = (ArgIndex::parse(text)?, layer.label.clone());
            arg_set = true;
        }
        if let Some(list) = map.get("agentModels") {
            let Value::Array(list) = list else {
                bail!("{}: agentModels is not a list", layer.label);
            };
            for (index, rule) in list.iter().enumerate() {
                let source = format!("rule {} of the {}", index + 1, layer.label);
                out.rules.push(Rule::parse(rule, source)?);
            }
        }
        if layer.plugin {
            for (key, target) in [
                ("paths", &mut out.paths),
                ("extraPaths", &mut out.extra_paths),
            ] {
                let Some(value) = map.get(key) else { continue };
                let Value::Object(kinds) = value else {
                    bail!("{}: {key} is not an object", layer.label);
                };
                for (kind, paths) in kinds {
                    if !PATH_KINDS.contains(&kind.as_str()) {
                        bail!("{}: {key}.{kind} is not a component kind", layer.label);
                    }
                    target.entry(kind.clone()).or_insert_with(|| paths.clone());
                }
            }
        }
    }
    for layer in layers.iter().rev() {
        if let Some(value) = layer.map.get("env") {
            for (key, text) in string_map(value, &format!("{}: env", layer.label))? {
                out.env.retain(|(k, _)| *k != key);
                out.env.push((key, text));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob("opus*", "Opus[1m]"));
        assert!(glob("*-audit", "kit-review-audit"));
        assert!(glob("k?t", "kit"));
        assert!(glob("*", ""));
        assert!(!glob("opus*", "sonnet"));
        assert!(!glob("a*b", "acd"));
    }

    #[test]
    fn rules_parse_and_match() {
        let value = serde_json::json!({ "match": { "agent": "*-audit", "model": "opus*" },
            "provider": "anthropic", "model": "claude-opus-4-1", "thinking": "high" });
        let rule = Rule::parse(&value, "r".into()).unwrap();
        assert_eq!(rule.to_model.as_deref(), Some("anthropic/claude-opus-4-1"));
        assert!(rule.matches("kit", &["audit", "kit-review-audit"], "opus", ""));
        assert!(!rule.matches("kit", &["audit"], "opus", ""));
        let bad = serde_json::json!({ "provider": "x" });
        assert!(Rule::parse(&bad, "r".into()).is_err());
    }
}
