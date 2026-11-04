//! Claude plugin `userConfig`: the option schema, the stored values, and their substitution.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::jsonfile;

/// Prefix of the hook env variable of each option.
pub const ENV_PREFIX: &str = "CLAUDE_PLUGIN_OPTION_";

/// One `userConfig` option.
#[derive(Debug, Clone, PartialEq)]
pub struct Opt {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub description: String,
    pub required: bool,
    pub sensitive: bool,
    pub list: bool,
    pub default: Option<Value>,
    pub choices: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The options of a `userConfig` object; invalid keys and entries are left out.
pub fn schema(config: &Map<String, Value>) -> Vec<Opt> {
    let mut out = Vec::new();
    for (key, spec) in config {
        let Value::Object(spec) = spec else { continue };
        if !valid_key(key) {
            pi_ext_sdk::warn!("userConfig key \"{key}\" is not valid; skipped");
            continue;
        }
        let text = |k: &str| spec.get(k).and_then(Value::as_str).map(str::to_owned);
        let flag = |k: &str| spec.get(k).and_then(Value::as_bool).unwrap_or(false);
        let choices = spec.get("options").and_then(Value::as_array);
        out.push(Opt {
            key: key.clone(),
            kind: text("type").unwrap_or_else(|| "string".into()),
            title: text("title").unwrap_or_else(|| key.clone()),
            description: text("description").unwrap_or_default(),
            required: flag("required"),
            sensitive: flag("sensitive"),
            list: flag("multiple"),
            default: spec.get("default").cloned().filter(|v| !v.is_null()),
            choices: choices
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            min: spec.get("min").and_then(Value::as_f64),
            max: spec.get("max").and_then(Value::as_f64),
        });
    }
    out
}

/// The stored values; a missing file is empty.
pub fn load(file: &Path) -> Result<Map<String, Value>> {
    jsonfile::read_object(file)
}

/// Writes the values with owner-only permissions: they can hold sensitive values.
pub fn save(file: &Path, values: &Map<String, Value>) -> Result<()> {
    jsonfile::write(file, &Value::Object(values.clone()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(file, perms)
            .with_context(|| format!("cannot set permissions of {}", file.display()))?;
    }
    Ok(())
}

/// The stored value, else the default.
pub fn value<'a>(opt: &'a Opt, stored: &'a Map<String, Value>) -> Option<&'a Value> {
    stored.get(&opt.key).or(opt.default.as_ref())
}

/// Required options without a stored value or a default.
pub fn missing(opts: &[Opt], stored: &Map<String, Value>) -> Vec<String> {
    opts.iter()
        .filter(|opt| opt.required && value(opt, stored).is_none())
        .map(|opt| opt.key.clone())
        .collect()
}

pub fn to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(to_text).collect::<Vec<_>>().join(","),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `user_config.<KEY>` substitutions. `with_sensitive: false` leaves sensitive tokens as is.
pub fn vars(
    opts: &[Opt],
    stored: &Map<String, Value>,
    with_sensitive: bool,
) -> Vec<(String, String)> {
    opts.iter()
        .filter(|opt| with_sensitive || !opt.sensitive)
        .filter_map(|opt| {
            let value = value(opt, stored)?;
            Some((format!("user_config.{}", opt.key), to_text(value)))
        })
        .collect()
}

/// `CLAUDE_PLUGIN_OPTION_<KEY>` variables for hook processes.
pub fn env(opts: &[Opt], stored: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for opt in opts {
        if let Some(value) = value(opt, stored) {
            let name = format!("{ENV_PREFIX}{}", opt.key.to_ascii_uppercase());
            out.insert(name, json!(to_text(value)));
        }
    }
    out
}

/// Parses user text for `opt`. List values are comma separated.
pub fn parse(opt: &Opt, text: &str) -> Result<Value> {
    let text = text.trim();
    if opt.list {
        let items: Vec<&str> = text
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        return Ok(json!(items));
    }
    match opt.kind.as_str() {
        "number" => {
            let Ok(number) = text.parse::<f64>() else {
                bail!("{} must be a number", opt.key);
            };
            if opt.min.is_some_and(|min| number < min) || opt.max.is_some_and(|max| number > max) {
                bail!("{} is out of range", opt.key);
            }
            match text.parse::<i64>() {
                Ok(whole) => Ok(json!(whole)),
                Err(_) => Ok(json!(number)),
            }
        }
        "boolean" => match text.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(json!(true)),
            "false" | "no" | "off" | "0" => Ok(json!(false)),
            _ => bail!("{} must be true or false", opt.key),
        },
        _ if !opt.choices.is_empty() && !opt.choices.iter().any(|c| c == text) => {
            bail!("{} must be one of: {}", opt.key, opt.choices.join(", "))
        }
        _ => Ok(json!(text)),
    }
}

/// One line per option for `config` and `info`; sensitive values are masked.
pub fn lines(opts: &[Opt], stored: &Map<String, Value>) -> Vec<String> {
    opts.iter()
        .map(|opt| {
            let shown = match (stored.get(&opt.key), &opt.default) {
                (Some(_), _) if opt.sensitive => "(set)".to_owned(),
                (Some(value), _) => to_text(value),
                (None, Some(default)) => format!("{} (default)", to_text(default)),
                (None, None) if opt.required => "(missing, required)".to_owned(),
                (None, None) => "(not set)".to_owned(),
            };
            let mut line = format!("  {} = {shown}", opt.key);
            if !opt.title.is_empty() && opt.title != opt.key {
                line.push_str(&format!("  -- {}", opt.title));
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<Opt> {
        let config = json!({
            "api_key": { "type": "string", "title": "API key", "description": "Key", "sensitive": true, "required": true },
            "depth": { "type": "number", "title": "Depth", "description": "D", "default": 2, "min": 1, "max": 5 },
            "tags": { "type": "string", "title": "Tags", "description": "T", "multiple": true },
            "9bad": { "type": "string", "title": "x", "description": "x" }
        });
        schema(config.as_object().unwrap())
    }

    #[test]
    fn schema_values_and_vars() {
        let opts = opts();
        assert_eq!(opts.len(), 3);
        let mut stored = Map::new();
        assert_eq!(missing(&opts, &stored), ["api_key"]);
        stored.insert("api_key".into(), json!("s3cret"));
        stored.insert("tags".into(), parse(&opts[2], "a, b").unwrap());
        assert!(missing(&opts, &stored).is_empty());
        let content = vars(&opts, &stored, false);
        assert_eq!(
            content,
            [
                ("user_config.depth".to_owned(), "2".to_owned()),
                ("user_config.tags".to_owned(), "a,b".to_owned())
            ]
        );
        assert_eq!(vars(&opts, &stored, true).len(), 3);
        let env = env(&opts, &stored);
        assert_eq!(env["CLAUDE_PLUGIN_OPTION_API_KEY"], "s3cret");
        assert!(lines(&opts, &stored)[0].contains("api_key = (set)"));
    }

    #[test]
    fn parses_by_type() {
        let opts = opts();
        assert_eq!(parse(&opts[1], "3").unwrap(), json!(3));
        assert_eq!(parse(&opts[1], "2.5").unwrap(), json!(2.5));
        assert!(parse(&opts[1], "9").is_err());
        assert!(parse(&opts[1], "x").is_err());
    }
}
