use std::path::Path;

use serde_json::{Map, Value};

pub const SETTINGS_NAMESPACE: &str = "observational-memory";
pub const PASSIVE_ENV: &str = "PI_OBSERVATIONAL_MEMORY_PASSIVE";
pub const OBSERVER_CHUNK_FALLBACK_MAX_TOKENS: u64 = 60_000;
pub const OBSERVER_CHUNK_MIN_TOKENS: u64 = 256;
pub const OBSERVER_CHUNK_CONTEXT_RATIO: f64 = 0.2;
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredModel {
    pub provider: String,
    pub id: String,
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub observe_after_tokens: u64,
    pub reflect_after_tokens: u64,
    pub observer_chunk_max_tokens: Option<u64>,
    pub compact_after_tokens: u64,
    /// `"calibrated"` or `"ratio"`.
    pub compact_after_tokens_mode: String,
    pub compact_after_tokens_ratio: f64,
    pub observations_pool_max_tokens: u64,
    pub observations_pool_target_tokens: u64,
    pub agent_max_turns: u64,
    pub agent_max_tokens: u64,
    pub model: Option<ConfiguredModel>,
    pub show_worker_notifications: bool,
    pub passive: bool,
    pub debug_log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            observe_after_tokens: 10_000,
            reflect_after_tokens: 20_000,
            observer_chunk_max_tokens: None,
            compact_after_tokens: 81_000,
            compact_after_tokens_mode: "calibrated".to_owned(),
            compact_after_tokens_ratio: 0.68,
            observations_pool_max_tokens: 20_000,
            observations_pool_target_tokens: 10_000,
            agent_max_turns: 16,
            agent_max_tokens: 32_000,
            model: None,
            show_worker_notifications: true,
            passive: false,
            debug_log: false,
        }
    }
}

impl Config {
    pub fn thinking(&self) -> &str {
        self.model
            .as_ref()
            .and_then(|m| m.thinking.as_deref())
            .unwrap_or("low")
    }
}

fn positive_int(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).filter(|v| *v > 0)
}

fn normalize_model(value: Option<&Value>) -> Option<ConfiguredModel> {
    let non_empty = |k: &str| {
        value?
            .get(k)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Some(ConfiguredModel {
        provider: non_empty("provider")?,
        id: non_empty("id")?,
        thinking: non_empty("thinking").filter(|t| THINKING_LEVELS.contains(&t.as_str())),
    })
}

/// Applies the valid keys of one source onto `config`; invalid values are ignored.
fn apply(config: &mut Config, target: &mut Option<u64>, source: &Map<String, Value>) {
    let int = |k: &str| positive_int(source.get(k));
    let bool_of = |k: &str| source.get(k).and_then(Value::as_bool);
    if let Some(v) = int("observeAfterTokens") {
        config.observe_after_tokens = v;
    }
    if let Some(v) = int("reflectAfterTokens") {
        config.reflect_after_tokens = v;
    }
    if let Some(v) = int("observerChunkMaxTokens") {
        config.observer_chunk_max_tokens = Some(v);
    }
    if let Some(v) = int("compactAfterTokens") {
        config.compact_after_tokens = v;
    }
    if let Some(v) = int("observationsPoolMaxTokens") {
        config.observations_pool_max_tokens = v;
    }
    if let Some(v) = int("observationsPoolTargetTokens") {
        *target = Some(v);
    }
    if let Some(v) = int("agentMaxTurns") {
        config.agent_max_turns = v;
    }
    if let Some(v) = int("agentMaxTokens") {
        config.agent_max_tokens = v;
    }
    if let Some(mode) = source
        .get("compactAfterTokensMode")
        .and_then(Value::as_str)
        .filter(|m| matches!(*m, "calibrated" | "ratio"))
    {
        config.compact_after_tokens_mode = mode.to_owned();
    }
    if let Some(ratio) = source
        .get("compactAfterTokensRatio")
        .and_then(Value::as_f64)
        .filter(|r| r.is_finite() && *r > 0.0 && *r < 1.0)
    {
        config.compact_after_tokens_ratio = ratio;
    }
    if let Some(v) = bool_of("showWorkerNotifications") {
        config.show_worker_notifications = v;
    }
    if let Some(v) = bool_of("passive") {
        config.passive = v;
    }
    if let Some(v) = bool_of("debugLog") {
        config.debug_log = v;
    }
    if let Some(model) = normalize_model(source.get("model")) {
        config.model = Some(model);
    }
}

pub fn env_passive(raw: Option<&str>) -> Option<bool> {
    match raw?.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Merges sources in order, later wins per key, then derives the pool target.
pub fn merge(sources: &[Value], passive_env: Option<bool>) -> Config {
    let mut config = Config::default();
    let mut target = None;
    for source in sources {
        if let Value::Object(map) = source {
            apply(&mut config, &mut target, map);
        }
    }
    if let Some(passive) = passive_env {
        config.passive = passive;
    }
    let max = config.observations_pool_max_tokens;
    config.observations_pool_target_tokens = target.filter(|t| *t < max).unwrap_or(max / 2);
    config
}

/// The JSON object in `path`, or `Null` when it is missing or unreadable.
pub fn read_json_file(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or(Value::Null)
}

pub fn resolve_compact_after_tokens(config: &Config, context_window: Option<u64>) -> u64 {
    match context_window.filter(|w| *w > 0) {
        Some(window) if config.compact_after_tokens_mode == "ratio" => {
            ((window as f64 * config.compact_after_tokens_ratio).floor() as u64).max(1)
        }
        _ => config.compact_after_tokens,
    }
}

pub fn resolve_observer_chunk_max_tokens(config: &Config, context_window: Option<u64>) -> u64 {
    if let Some(max) = config.observer_chunk_max_tokens.filter(|m| *m > 0) {
        return max.max(OBSERVER_CHUNK_MIN_TOKENS);
    }
    match context_window.filter(|w| *w > 0) {
        Some(window) => ((window as f64 * OBSERVER_CHUNK_CONTEXT_RATIO).floor() as u64)
            .max(OBSERVER_CHUNK_MIN_TOKENS),
        None => OBSERVER_CHUNK_FALLBACK_MAX_TOKENS,
    }
}

/// `min(model.maxTokens, agentMaxTokens)`, as upstream `boundedMaxTokens`.
pub fn bounded_max_tokens(model_max: Option<u64>, requested: u64) -> u64 {
    model_max
        .filter(|m| *m > 0)
        .map_or(requested, |m| m.min(requested))
}
