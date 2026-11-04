//! `<agentDir>/ext/simple-goal.json` and `<cwd>/.pi/ext/simple-goal.json`; project keys win.

use std::path::Path;

use serde_json::{Map, Value};

pub const DEFAULT_MAX_ROUNDS: u32 = 8;
pub const DEFAULT_TYPESAFE_THRESHOLD: f64 = 0.7;

/// Who judges the goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judge {
    /// TypeSafe when the `typesafe` extension is loaded and enabled, else the LLM.
    Auto,
    Llm,
    Typesafe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// `provider/id` of the evaluator; the session model when unset.
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub system_prompt: Option<String>,
    pub instructions: Option<String>,
    pub max_rounds: u32,
    pub judge: Judge,
    /// The probability of "done" from which a TypeSafe answer counts as done.
    pub typesafe_threshold: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: None,
            thinking: None,
            system_prompt: None,
            instructions: None,
            max_rounds: DEFAULT_MAX_ROUNDS,
            judge: Judge::Auto,
            typesafe_threshold: DEFAULT_TYPESAFE_THRESHOLD,
        }
    }
}

/// Keys that pick the evaluator, its model, or its prompt; the project file sets them only in a
/// trusted project.
pub const TRUSTED_KEYS: [&str; 4] = ["model", "thinking", "systemPrompt", "judge"];

pub struct Loaded {
    pub config: Config,
    /// Project keys skipped because the project is not trusted.
    pub ignored: Vec<String>,
}

/// `project_dir` is `<cwd>/.pi`.
pub fn load(agent_dir: &Path, project_dir: &Path, trusted: bool) -> Loaded {
    let mut merged = read(&agent_dir.join("ext").join("simple-goal.json"));
    let mut ignored = Vec::new();
    for (key, value) in read(&project_dir.join("ext").join("simple-goal.json")) {
        if !trusted && TRUSTED_KEYS.contains(&key.as_str()) {
            ignored.push(key);
            continue;
        }
        merged.insert(key, value);
    }
    Loaded {
        config: from_map(&merged),
        ignored,
    }
}

fn read(path: &Path) -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => {
            pi_ext_sdk::warn!("ignoring {}: not a JSON object", path.display());
            Map::new()
        }
    }
}

pub fn from_map(map: &Map<String, Value>) -> Config {
    let text = |key: &str| {
        map.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Config {
        model: text("model"),
        thinking: text("thinking"),
        system_prompt: text("systemPrompt"),
        instructions: text("instructions"),
        max_rounds: map
            .get("maxRounds")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_ROUNDS, |n| {
                n.clamp(1, u64::from(u32::MAX)) as u32
            }),
        judge: match text("judge").as_deref() {
            Some("llm") => Judge::Llm,
            Some("typesafe") => Judge::Typesafe,
            _ => Judge::Auto,
        },
        typesafe_threshold: map
            .get("typesafeThreshold")
            .and_then(Value::as_f64)
            .filter(|t| (0.0..=1.0).contains(t))
            .unwrap_or(DEFAULT_TYPESAFE_THRESHOLD),
    }
}
