//! Config: defaults, global and project layers deep-merged (project wins), legacy keys translated.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

pub const SECTION_IDS: [&str; 9] = [
    "activeWork",
    "requests",
    "state",
    "decisions",
    "files",
    "commits",
    "errors",
    "lessons",
    "transcript",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sections {
    pub active_work: bool,
    pub requests: bool,
    pub state: bool,
    pub decisions: bool,
    pub files: bool,
    pub commits: bool,
    pub errors: bool,
    pub lessons: bool,
    pub transcript: bool,
}

impl Default for Sections {
    fn default() -> Self {
        Self::from_ids(&SECTION_IDS.map(str::to_owned))
    }
}

impl Sections {
    pub fn from_ids(ids: &[String]) -> Self {
        let on = |id: &str| ids.iter().any(|i| i == id);
        Self {
            active_work: on("activeWork"),
            requests: on("requests"),
            state: on("state"),
            decisions: on("decisions"),
            files: on("files"),
            commits: on("commits"),
            errors: on("errors"),
            lessons: on("lessons"),
            transcript: on("transcript"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Vcc,
    Llm,
}

impl Method {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "vcc" | "jev" => Some(Method::Vcc),
            "llm" => Some(Method::Llm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Vcc => "vcc",
            Method::Llm => "llm",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub method: Method,
    /// `None` follows `method`.
    pub pi_compact: Option<Method>,
    pub trigger_percent: bool,
    pub threshold_percent: f64,
    pub notify: bool,
    pub smart_keep_tail: bool,
    pub summary_budget_tokens: usize,
    pub sections: Sections,
    pub summary_sections: Vec<String>,
    pub cooldown_ms: u64,
    pub repeat_min_growth_tokens: u64,
    pub llm_instructions: String,
    pub debug: bool,
}

impl Default for Config {
    fn default() -> Self {
        from_value(&defaults())
    }
}

pub fn defaults() -> Value {
    json!({
        "method": "vcc", "piCompact": "follow", "trigger": "pi", "thresholdPercent": 80, "notify": true,
        "smartKeepTail": true, "summaryBudgetTokens": 0, "summarySections": SECTION_IDS,
        "cooldownMs": 60000, "repeatMinGrowthTokens": 4000, "llmInstructions": "", "debug": false
    })
}

const KNOWN_KEYS: [&str; 13] = [
    "method",
    "piCompact",
    "trigger",
    "thresholdPercent",
    "notify",
    "smartKeepTail",
    "summaryBudgetTokens",
    "sections",
    "summarySections",
    "cooldownMs",
    "repeatMinGrowthTokens",
    "llmInstructions",
    "debug",
];

pub fn global_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("compactor.json")
}

pub fn project_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join("compactor.json")
}

/// Current keys of one layer; pre-rework keys fill only keys the layer does not set.
pub fn translate_legacy(raw: &Map<String, Value>) -> Map<String, Value> {
    let mut out: Map<String, Value> = raw
        .iter()
        .filter(|(k, _)| KNOWN_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for key in ["method", "piCompact"] {
        if out.get(key).and_then(Value::as_str) == Some("jev") {
            out.insert(key.into(), json!("vcc"));
        }
    }
    if !raw.contains_key("method") && raw.get("overrideDefaultCompaction") == Some(&json!(false)) {
        out.insert("method".into(), json!("llm"));
    }
    if let Some(auto) = raw.get("autoCompaction").and_then(Value::as_object) {
        if !raw.contains_key("trigger") && auto.get("enabled") == Some(&json!(true)) {
            out.insert("trigger".into(), json!("percent"));
        }
        for key in [
            "thresholdPercent",
            "notify",
            "cooldownMs",
            "repeatMinGrowthTokens",
        ] {
            if let (false, Some(value)) = (raw.contains_key(key), auto.get(key)) {
                out.insert(key.into(), value.clone());
            }
        }
    }
    out
}

pub fn deep_merge(base: &mut Value, over: &Map<String, Value>) {
    let Some(target) = base.as_object_mut() else {
        return;
    };
    for (key, value) in over {
        match (target.get_mut(key), value) {
            (Some(existing @ Value::Object(_)), Value::Object(inner)) => {
                deep_merge(existing, inner)
            }
            (_, Value::Null) => {}
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Section selection: the first layer (project, then global) with `summarySections`, else legacy `sections`.
fn stored_selection(layers: &[Option<Map<String, Value>>]) -> Option<Vec<String>> {
    for layer in layers.iter().rev().flatten() {
        if let Some(list) = layer.get("summarySections").and_then(Value::as_array) {
            return Some(
                list.iter()
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                    .collect(),
            );
        }
        if let Some(legacy) = layer.get("sections").and_then(Value::as_object) {
            return Some(
                SECTION_IDS
                    .iter()
                    .filter(|id| legacy.get(**id) != Some(&json!(false)))
                    .map(|id| (*id).to_owned())
                    .collect(),
            );
        }
    }
    None
}

/// Effective config from the global and project layers (each `None` when absent or unreadable).
pub fn merge_layers(
    global: Option<Map<String, Value>>,
    project: Option<Map<String, Value>>,
) -> Config {
    let layers = [global, project];
    let mut merged = defaults();
    for layer in layers.iter().flatten() {
        deep_merge(&mut merged, &translate_legacy(layer));
    }
    if let Some(selection) = stored_selection(&layers) {
        merged["summarySections"] = json!(selection);
    }
    from_value(&merged)
}

fn read_layer(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Some(map),
        _ => {
            pi_ext_sdk::warn!("ignoring unreadable config {}", path.display());
            None
        }
    }
}

pub fn load(agent_dir: &Path, cwd: &Path) -> Config {
    merge_layers(
        read_layer(&global_path(agent_dir)),
        read_layer(&project_path(cwd)),
    )
}

fn from_value(value: &Value) -> Config {
    let d = |key: &str| value.get(key);
    let bool_of = |key: &str, fallback: bool| d(key).and_then(Value::as_bool).unwrap_or(fallback);
    let num_of = |key: &str, fallback: f64| {
        d(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
            .unwrap_or(fallback)
    };
    let summary_sections: Vec<String> = match d("summarySections").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        None => SECTION_IDS.map(str::to_owned).to_vec(),
    };
    Config {
        method: d("method")
            .and_then(Value::as_str)
            .and_then(Method::parse)
            .unwrap_or(Method::Vcc),
        pi_compact: d("piCompact")
            .and_then(Value::as_str)
            .and_then(Method::parse),
        trigger_percent: d("trigger").and_then(Value::as_str) == Some("percent"),
        threshold_percent: num_of("thresholdPercent", 80.0),
        notify: bool_of("notify", true),
        smart_keep_tail: bool_of("smartKeepTail", true),
        summary_budget_tokens: num_of("summaryBudgetTokens", 0.0).max(0.0) as usize,
        sections: Sections::from_ids(&summary_sections),
        summary_sections,
        cooldown_ms: num_of("cooldownMs", 60000.0).max(0.0) as u64,
        repeat_min_growth_tokens: num_of("repeatMinGrowthTokens", 4000.0).max(0.0) as u64,
        llm_instructions: d("llmInstructions")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        debug: bool_of("debug", false)
            || std::env::var("UNIPI_COMPACTOR_DEBUG").is_ok_and(|v| v == "1"),
    }
}
