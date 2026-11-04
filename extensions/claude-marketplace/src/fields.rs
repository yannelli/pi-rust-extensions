//! Every plugin.json and marketplace entry field, and the reason for each one pi does not
//! apply. `install` and `info` show these, so nothing is dropped silently.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A component or field that is not applied, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dropped {
    pub what: String,
    pub reason: String,
}

impl Dropped {
    pub fn new(what: impl Into<String>, reason: impl Into<String>) -> Dropped {
        Dropped {
            what: what.into(),
            reason: reason.into(),
        }
    }
}

/// Fields that the extension applies.
pub const APPLIED: [&str; 14] = [
    "name",
    "version",
    "description",
    "commands",
    "agents",
    "skills",
    "hooks",
    "mcpServers",
    "outputStyles",
    "userConfig",
    "dependencies",
    "experimental",
    "source",
    "strict",
];

pub const METADATA: &str = "catalog metadata; pi does not use it";

/// The reason pi does not apply `field`, or `None` when it is applied.
pub fn reason(field: &str) -> Option<&'static str> {
    if APPLIED.contains(&field) {
        return None;
    }
    Some(match field {
        "lspServers" => "pi runs no plugin LSP servers; add them to lens.json by hand",
        "monitors" => "pi has no background monitors",
        "themes" => "pi themes use another format",
        "channels" => "pi has no message channels",
        "workflows" => "pi has no workflow runner",
        "evals" => "pi has no eval runner",
        "types" => "pi has no mods",
        "settings" => "pi has no plugin default agent or subagent status line",
        "defaultEnabled" => "pi enables a plugin when you install it",
        "headers" | "headersHelper" => "only for archive sources, which pi does not support",
        "$schema" | "displayName" | "author" | "homepage" | "repository" | "license"
        | "keywords" | "metadata" | "icon" | "documentationUrl" | "supportUrl"
        | "privacyPolicyUrl" | "termsOfServiceUrl" | "category" | "tags" | "relevance" => METADATA,
        _ => "unknown field",
    })
}

/// The fields of one manifest object that are not applied. `origin` names the object.
pub fn manifest(value: &Value, origin: &str, out: &mut Vec<Dropped>) {
    let Some(map) = value.as_object() else { return };
    for (key, inner) in map {
        if key == "experimental" {
            for nested in inner.as_object().into_iter().flatten().map(|(k, _)| k) {
                let reason = match nested.as_str() {
                    "themes" | "monitors" | "evals" => self::reason(nested),
                    _ => Some("unknown field"),
                };
                if let Some(reason) = reason {
                    out.push(Dropped::new(
                        format!("{origin} experimental.{nested}"),
                        reason,
                    ));
                }
            }
        } else if let Some(reason) = reason(key) {
            out.push(Dropped::new(format!("{origin} {key}"), reason));
        }
    }
}

/// Frontmatter keys that pi or the subagents extension applies, per staged kind.
fn applied_front(kind: &str) -> &'static [&'static str] {
    match kind {
        "command" => &["description", "argument-hint", "arguments", "name"],
        "skill" => &[
            "name",
            "description",
            "disable-model-invocation",
            "license",
            "compatibility",
            "metadata",
            "allowed-tools",
        ],
        "agent" => &[
            "name",
            "description",
            "tools",
            "disallowedTools",
            "model",
            "effort",
            "thinking",
            "maxTurns",
            "systemPromptMode",
        ],
        _ => &["name", "description", "force-for-plugin"],
    }
}

fn front_reason(kind: &str, key: &str) -> &'static str {
    match (kind, key) {
        ("skill", "model" | "effort") => "pi skills have no model or thinking setting",
        ("command", "model" | "effort") => "pi prompt templates run on the session model",
        ("command", "allowed-tools" | "disallowed-tools") => {
            "pi prompt templates do not change tool permissions"
        }
        ("agent", "permissionMode" | "hooks" | "mcpServers" | "initialPrompt") => {
            "Claude Code ignores it for plugin agents too"
        }
        ("style", "keep-coding-instructions") => "pi keeps its own instructions in all cases",
        ("command", _) => "pi prompt templates do not use it",
        ("skill", _) => "pi skills do not use it",
        ("agent", _) => "the subagents extension does not use it",
        _ => "pi output styles do not use it",
    }
}

/// Collects the frontmatter keys of one staged file that are not applied.
pub struct FrontDrops {
    items: Vec<(String, String, String)>,
}

impl FrontDrops {
    pub fn new() -> FrontDrops {
        FrontDrops { items: Vec::new() }
    }

    /// `kind` is `command`, `skill`, `agent`, or `style`; `keys` are the frontmatter keys.
    pub fn add(&mut self, kind: &str, name: &str, keys: &[String]) {
        for key in keys {
            if !applied_front(kind).contains(&key.as_str()) {
                self.items.push((kind.into(), key.clone(), name.into()));
            }
        }
    }

    /// A body feature that is not applied, such as `!`cmd`` in a command.
    pub fn feature(&mut self, kind: &str, feature: &str, name: &str) {
        self.items
            .push((kind.into(), format!("@{feature}"), name.into()));
    }

    /// One entry per kind and key, with the file names.
    pub fn finish(self, out: &mut Vec<Dropped>) {
        let mut seen: Vec<(String, String, Vec<String>)> = Vec::new();
        for (kind, key, name) in self.items {
            match seen.iter_mut().find(|(k, n, _)| *k == kind && *n == key) {
                Some((_, _, names)) if !names.contains(&name) => names.push(name),
                Some(_) => {}
                None => seen.push((kind, key, vec![name])),
            }
        }
        for (kind, key, names) in seen {
            let (what, reason) = match key.strip_prefix('@') {
                Some("shell") => (
                    "!`cmd` shell injection".to_owned(),
                    "pi prompt templates do not run shell commands; the text stays as is",
                ),
                Some(_) => (
                    "@file references".to_owned(),
                    "pi prompt templates do not attach files; the text stays as is",
                ),
                None => (
                    format!("{kind} frontmatter {key}"),
                    front_reason(&kind, &key),
                ),
            };
            out.push(Dropped::new(
                format!("{what} ({})", names.join(", ")),
                reason,
            ));
        }
    }
}

impl Default for FrontDrops {
    fn default() -> Self {
        FrontDrops::new()
    }
}

/// `install` and `info` text: the items grouped by reason.
pub fn summary(dropped: &[Dropped]) -> Vec<String> {
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for item in dropped {
        match groups.iter_mut().find(|(r, _)| *r == item.reason) {
            Some((_, whats)) if !whats.contains(&item.what.as_str()) => whats.push(&item.what),
            Some(_) => {}
            None => groups.push((&item.reason, vec![&item.what])),
        }
    }
    groups
        .into_iter()
        .map(|(reason, whats)| format!("{} ({reason})", whats.join(", ")))
        .collect()
}

/// The marketplace.json fields that are not applied, as ` Not applied: ...` text.
pub fn marketplace_note(value: &Value) -> String {
    let mut out = Vec::new();
    for (key, inner) in value.as_object().into_iter().flatten() {
        let reason = match key.as_str() {
            "name" | "plugins" => continue,
            "metadata" => {
                for nested in inner.as_object().into_iter().flatten().map(|(k, _)| k) {
                    if nested != "pluginRoot" {
                        out.push(Dropped::new(format!("metadata.{nested}"), METADATA));
                    }
                }
                continue;
            }
            "$schema" | "owner" | "description" | "version" => METADATA,
            "forceRemoveDeletedPlugins" => "pi keeps installed plugins that leave the catalog",
            "renames" => "pi does not rename installed plugins",
            "allowCrossMarketplaceDependenciesOn" => {
                "pi does not check it; a name@marketplace dependency installs when that marketplace is added"
            }
            _ => "unknown field",
        };
        out.push(Dropped::new(key.clone(), reason));
    }
    match summary(&out).as_slice() {
        [] => String::new(),
        lines => format!(" Not applied: {}.", lines.join("; ")),
    }
}

/// `!`cmd`` and ```` ```! ```` blocks in a command body.
pub fn has_shell(body: &str) -> bool {
    body.contains("!`") || body.contains("```!")
}

/// `@path` references at the start of a word.
pub fn has_file_ref(body: &str) -> bool {
    body.split_whitespace().any(|word| {
        word.strip_prefix('@')
            .is_some_and(|rest| rest.contains(['/', '.']) && !rest.contains('@'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reports_fields_and_frontmatter() {
        let mut out = Vec::new();
        let value = json!({ "name": "x", "author": {}, "lspServers": {}, "wat": 1,
            "experimental": { "themes": "./t", "evals": "./e", "new": 1 } });
        manifest(&value, "plugin.json", &mut out);
        let whats: Vec<&str> = out.iter().map(|d| d.what.as_str()).collect();
        assert_eq!(
            whats,
            [
                "plugin.json author",
                "plugin.json lspServers",
                "plugin.json wat",
                "plugin.json experimental.themes",
                "plugin.json experimental.evals",
                "plugin.json experimental.new"
            ]
        );
        let mut front = FrontDrops::new();
        front.add("agent", "a", &["name".into(), "color".into()]);
        front.add("agent", "b", &["color".into()]);
        front.feature("command", "shell", "c");
        let mut out = Vec::new();
        front.finish(&mut out);
        assert_eq!(out[0].what, "agent frontmatter color (a, b)");
        assert_eq!(out[1].what, "!`cmd` shell injection (c)");
        assert_eq!(summary(&out).len(), 2);
        assert!(has_file_ref("see @docs/a.md now"));
        assert!(!has_file_ref("mail me@x.org"));
    }
}
