use pi_ext_compactor::config::{Config, Method, Sections, load, merge_layers, translate_legacy};
use pi_ext_compactor::trigger::{AutoConfig, AutoState, Reason, Usage, decide, mark_complete};
use serde_json::{Map, Value, json};

fn layer(value: Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}

#[test]
fn defaults_match_upstream() {
    let config = Config::default();
    assert_eq!(config.method, Method::Vcc);
    assert_eq!(config.pi_compact, None);
    assert!(!config.trigger_percent);
    assert_eq!(config.threshold_percent, 80.0);
    assert!(config.notify && config.smart_keep_tail);
    assert_eq!(
        (
            config.summary_budget_tokens,
            config.cooldown_ms,
            config.repeat_min_growth_tokens
        ),
        (0, 60_000, 4_000)
    );
    assert_eq!(config.sections, Sections::default());
    assert_eq!(config.summary_sections.len(), 9);
}

#[test]
fn project_wins_per_key() {
    let config = merge_layers(
        layer(json!({ "method": "llm", "thresholdPercent": 70, "notify": false })),
        layer(json!({ "thresholdPercent": 60, "piCompact": "vcc" })),
    );
    assert_eq!(config.method, Method::Llm);
    assert_eq!(config.threshold_percent, 60.0);
    assert!(!config.notify);
    assert_eq!(config.pi_compact, Some(Method::Vcc));
}

#[test]
fn legacy_keys_translate() {
    let raw = json!({ "overrideDefaultCompaction": false, "autoCompaction": { "enabled": true, "thresholdPercent": 75, "notify": false } });
    let translated = translate_legacy(raw.as_object().unwrap());
    assert_eq!(
        Value::Object(translated),
        json!({ "method": "llm", "trigger": "percent", "thresholdPercent": 75, "notify": false })
    );
    let jev = merge_layers(layer(json!({ "method": "jev", "piCompact": "jev" })), None);
    assert_eq!(
        (jev.method, jev.pi_compact),
        (Method::Vcc, Some(Method::Vcc))
    );
    let explicit = translate_legacy(
        json!({ "method": "vcc", "overrideDefaultCompaction": false })
            .as_object()
            .unwrap(),
    );
    assert_eq!(explicit["method"], "vcc");
}

#[test]
fn section_selection_precedence() {
    let legacy = merge_layers(
        layer(json!({ "sections": { "transcript": false, "commits": false } })),
        None,
    );
    assert!(!legacy.sections.transcript && !legacy.sections.commits && legacy.sections.files);
    let project_array = merge_layers(
        layer(json!({ "summarySections": ["files"] })),
        layer(json!({ "summarySections": ["requests", "state"] })),
    );
    assert!(
        project_array.sections.requests
            && project_array.sections.state
            && !project_array.sections.files
    );
    let project_legacy = merge_layers(
        layer(json!({ "summarySections": ["files"] })),
        layer(json!({ "sections": { "lessons": false } })),
    );
    assert!(
        project_legacy.sections.files
            && project_legacy.sections.requests
            && !project_legacy.sections.lessons
    );
}

#[test]
fn loads_files_from_agent_and_project_dirs() {
    let root = std::env::temp_dir().join(format!("compactor-config-{}", std::process::id()));
    let (agent, cwd) = (root.join("agent"), root.join("work"));
    std::fs::create_dir_all(agent.join("ext")).unwrap();
    std::fs::create_dir_all(cwd.join(".pi/ext")).unwrap();
    std::fs::write(
        agent.join("ext/compactor.json"),
        r#"{"trigger":"percent","thresholdPercent":50}"#,
    )
    .unwrap();
    std::fs::write(
        cwd.join(".pi/ext/compactor.json"),
        r#"{"thresholdPercent":40}"#,
    )
    .unwrap();
    let config = load(&agent, &cwd);
    assert!(config.trigger_percent);
    assert_eq!(config.threshold_percent, 40.0);
    std::fs::remove_dir_all(root).ok();
}

fn auto() -> AutoConfig {
    AutoConfig {
        enabled: true,
        threshold_percent: 80.0,
        cooldown_ms: 60_000,
        repeat_min_growth_tokens: 4_000,
    }
}

fn usage(tokens: u64, percent: f64) -> Option<Usage> {
    Some(Usage { tokens, percent })
}

#[test]
fn trigger_crosses_threshold_once_then_waits() {
    let start = AutoState::default();
    let below = decide(auto(), usage(50_000, 50.0), &start, 0);
    assert_eq!(below.reason, Reason::BelowThreshold);
    let crossed = decide(auto(), usage(85_000, 85.0), &below.state, 1_000);
    assert!(crossed.trigger);
    assert_eq!(crossed.reason, Reason::ThresholdCrossed);
    assert_eq!(
        decide(auto(), usage(86_000, 86.0), &crossed.state, 2_000).reason,
        Reason::InFlight
    );
    let done = mark_complete(&crossed.state);
    let baseline = decide(auto(), usage(82_000, 82.0), &done, 3_000);
    assert_eq!(baseline.reason, Reason::PostCompactionBaseline);
    assert_eq!(
        decide(auto(), usage(84_000, 84.0), &baseline.state, 4_000).reason,
        Reason::CooldownActive
    );
    let grown = decide(auto(), usage(84_000, 84.0), &baseline.state, 70_000);
    assert_eq!(grown.reason, Reason::RepeatGrowthNeeded);
    let repeat = decide(auto(), usage(87_000, 87.0), &grown.state, 80_000);
    assert_eq!(repeat.reason, Reason::RepeatGrowthReached);
    assert!(repeat.trigger);
}

#[test]
fn trigger_ignores_unknown_usage_and_disabled() {
    let state = AutoState::default();
    assert_eq!(decide(auto(), None, &state, 0).reason, Reason::UnknownUsage);
    let off = AutoConfig {
        enabled: false,
        ..auto()
    };
    assert_eq!(
        decide(off, usage(90_000, 90.0), &state, 0).reason,
        Reason::Disabled
    );
    assert_eq!(
        Usage::from_value(Some(&json!({ "tokens": null, "percent": null }))),
        None
    );
}
