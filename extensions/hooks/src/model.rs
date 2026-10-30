use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const MAX_TIMEOUT_SECS: f64 = 86_400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Claude,
    Codex,
    Pi,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Claude, Source::Codex, Source::Pi];

    pub fn name(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
            Source::Pi => "pi",
        }
    }

    /// pi-native hooks use Claude semantics.
    pub fn is_codex(self) -> bool {
        self == Source::Codex
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub id: Option<String>,
    pub root: Option<String>,
    pub data: Option<String>,
}

/// One handler of one matcher group of one config file.
#[derive(Debug, Clone, PartialEq)]
pub struct HookDef {
    pub source: Source,
    pub scope: Scope,
    pub file: PathBuf,
    pub event: String,
    pub matcher: Option<String>,
    pub handler: Value,
    pub plugin: Option<Plugin>,
    pub hash: String,
}

impl HookDef {
    pub fn new(
        source: Source,
        scope: Scope,
        file: PathBuf,
        event: &str,
        matcher: Option<String>,
        handler: Value,
    ) -> Self {
        let hash = trust_hash(&file.to_string_lossy(), event, matcher.as_deref(), &handler);
        Self {
            source,
            scope,
            file,
            event: event.to_owned(),
            matcher,
            handler,
            plugin: None,
            hash,
        }
    }

    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.handler.get(key).and_then(Value::as_str)
    }

    pub fn bool_field(&self, key: &str) -> bool {
        self.handler
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn kind(&self) -> &str {
        self.str_field("type").unwrap_or("command")
    }

    pub fn is_async(&self) -> bool {
        self.bool_field("async") || self.bool_field("asyncRewake")
    }

    pub fn timeout(&self) -> Duration {
        let codex_short =
            self.source.is_codex() && matches!(&*self.event, "SessionEnd" | "Interrupt");
        let default = match self.kind() {
            "prompt" => 30.0,
            _ if codex_short => 1.0,
            _ => 600.0,
        };
        let mut secs = self
            .handler
            .get("timeout")
            .and_then(Value::as_f64)
            .filter(|s| *s > 0.0)
            .map(|s| s.min(MAX_TIMEOUT_SECS));
        if codex_short {
            secs = secs.map(|s| s.min(3.0));
        }
        Duration::from_secs_f64(secs.unwrap_or(default))
    }

    /// Command, URL, or prompt text for listings and trust prompts.
    pub fn summary(&self) -> String {
        let text = match self.kind() {
            "http" => self.str_field("url").unwrap_or_default().to_owned(),
            "prompt" => format!("prompt: {}", self.str_field("prompt").unwrap_or_default()),
            "command" => {
                let mut text = self.str_field("command").unwrap_or_default().to_owned();
                if let Some(args) = self.handler.get("args").and_then(Value::as_array) {
                    for arg in args.iter().filter_map(Value::as_str) {
                        text.push(' ');
                        text.push_str(arg);
                    }
                }
                text
            }
            other => format!("{other} (unsupported)"),
        };
        text.replace('\n', " ")
    }

    /// The listing line plus what an http hook sends: header templates and allowed env var names.
    pub fn trust_text(&self) -> String {
        let mut text = self.to_string();
        if self.kind() != "http" {
            return text;
        }
        if let Some(headers) = self.handler.get("headers").and_then(Value::as_object) {
            for (name, value) in headers {
                let value = value.as_str().unwrap_or_default().replace('\n', " ");
                text.push_str(&format!("\n  header {name}: {value}"));
            }
        }
        let vars: Vec<&str> = self
            .handler
            .get("allowedEnvVars")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !vars.is_empty() {
            text.push_str(&format!("\n  sends env vars: {}", vars.join(", ")));
        }
        text
    }

    pub fn label(&self) -> String {
        let mut label = format!("{}/{}", self.source.name(), self.scope.name());
        if let Some(id) = self.plugin.as_ref().and_then(|p| p.id.as_deref()) {
            label.push_str(&format!(" plugin {id}"));
        }
        label
    }
}

impl fmt::Display for HookDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let matcher = self
            .matcher
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or("*");
        write!(
            f,
            "[{}] {} {}: {}",
            self.label(),
            self.event,
            matcher,
            self.summary()
        )
    }
}

/// sha256 hex of the canonical JSON `{ file, event, matcher, handler }`.
pub fn trust_hash(file: &str, event: &str, matcher: Option<&str>, handler: &Value) -> String {
    let doc = json!({ "file": file, "event": event, "matcher": matcher, "handler": handler });
    let digest = Sha256::digest(canonical(&doc).to_string().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Rebuilds objects with sorted keys; `serde_json` keeps insertion order in this workspace.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let sorted: Map<String, Value> = keys
                .into_iter()
                .map(|k| (k.clone(), canonical(&map[k])))
                .collect();
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_order_independent() {
        let a: Value =
            serde_json::from_str(r#"{"type":"command","command":"x","timeout":3}"#).unwrap();
        let b: Value =
            serde_json::from_str(r#"{"timeout":3,"command":"x","type":"command"}"#).unwrap();
        let h = trust_hash("/f.json", "PreToolUse", Some("Bash"), &a);
        assert_eq!(h, trust_hash("/f.json", "PreToolUse", Some("Bash"), &b));
        assert_eq!(h.len(), 64);
        assert_ne!(h, trust_hash("/g.json", "PreToolUse", Some("Bash"), &a));
        assert_ne!(h, trust_hash("/f.json", "PreToolUse", None, &a));
        let c = json!({ "type": "command", "command": "y", "timeout": 3 });
        assert_ne!(h, trust_hash("/f.json", "PreToolUse", Some("Bash"), &c));
    }

    #[test]
    fn http_trust_covers_headers_and_env_vars() {
        let handler = json!({ "type": "http", "url": "https://h.example/x",
            "headers": { "Authorization": "Bearer $TOKEN" }, "allowedEnvVars": ["TOKEN"] });
        let def = HookDef::new(
            Source::Claude,
            Scope::Global,
            "/f".into(),
            "Stop",
            None,
            handler.clone(),
        );
        let text = def.trust_text();
        assert!(
            text.contains("header Authorization: Bearer $TOKEN"),
            "{text}"
        );
        assert!(text.contains("sends env vars: TOKEN"), "{text}");
        let mut other = handler.clone();
        other["allowedEnvVars"] = json!(["TOKEN", "AWS_SECRET_ACCESS_KEY"]);
        assert_ne!(def.hash, trust_hash("/f", "Stop", None, &other));
        let mut other = handler;
        other["headers"]["X-Key"] = json!("$AWS_SECRET_ACCESS_KEY");
        assert_ne!(def.hash, trust_hash("/f", "Stop", None, &other));
    }

    #[test]
    fn huge_timeouts_are_clamped() {
        for timeout in [json!(1e300), json!(f64::MAX)] {
            let handler = json!({ "command": "x", "timeout": timeout });
            let def = HookDef::new(
                Source::Claude,
                Scope::Global,
                "/f".into(),
                "Stop",
                None,
                handler,
            );
            assert_eq!(def.timeout().as_secs(), 86_400);
        }
    }

    #[test]
    fn timeouts_follow_source_defaults() {
        let def = |source, event: &str, handler| {
            HookDef::new(source, Scope::Global, "/f".into(), event, None, handler)
        };
        let cmd = json!({ "command": "x" });
        assert_eq!(
            def(Source::Claude, "Stop", cmd.clone()).timeout().as_secs(),
            600
        );
        assert_eq!(def(Source::Codex, "SessionEnd", cmd).timeout().as_secs(), 1);
        let long = json!({ "command": "x", "timeout": 10 });
        assert_eq!(
            def(Source::Codex, "Interrupt", long.clone())
                .timeout()
                .as_secs(),
            3
        );
        assert_eq!(
            def(Source::Claude, "SessionEnd", long).timeout().as_secs(),
            10
        );
        let prompt = json!({ "type": "prompt", "prompt": "p" });
        assert_eq!(def(Source::Pi, "Stop", prompt).timeout().as_secs(), 30);
    }
}
