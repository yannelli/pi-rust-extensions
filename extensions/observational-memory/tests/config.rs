use pi_ext_observational_memory::config::*;
use serde_json::{Value, json};

#[test]
fn defaults_match_upstream() {
    let config = merge(&[], None);
    assert_eq!(config.observe_after_tokens, 10_000);
    assert_eq!(config.reflect_after_tokens, 20_000);
    assert_eq!(config.observer_chunk_max_tokens, None);
    assert_eq!(config.compact_after_tokens, 81_000);
    assert_eq!(config.compact_after_tokens_mode, "calibrated");
    assert_eq!(config.compact_after_tokens_ratio, 0.68);
    assert_eq!(config.observations_pool_max_tokens, 20_000);
    assert_eq!(config.observations_pool_target_tokens, 10_000);
    assert_eq!(config.agent_max_turns, 16);
    assert_eq!(config.agent_max_tokens, 32_000);
    assert!(config.show_worker_notifications && !config.passive && !config.debug_log);
    assert_eq!(config.thinking(), "low");
}

#[test]
fn later_sources_win_per_key_and_invalid_values_are_ignored() {
    let global = json!({ "observeAfterTokens": 5000, "reflectAfterTokens": 7000, "passive": true });
    let project = json!({ "observeAfterTokens": 6000, "compactAfterTokensRatio": 1.5,
        "model": { "provider": "openai", "id": "gpt-5-mini", "thinking": "minimal" } });
    let settings = json!({ "observeAfterTokens": -3, "passive": false, "compactAfterTokensMode": "ratio",
        "observationsPoolMaxTokens": 8000, "observationsPoolTargetTokens": 9000 });
    let config = merge(&[global, Value::Null, project, settings], None);
    assert_eq!(config.observe_after_tokens, 6000);
    assert_eq!(config.reflect_after_tokens, 7000);
    assert!(!config.passive);
    assert_eq!(config.compact_after_tokens_mode, "ratio");
    assert_eq!(config.compact_after_tokens_ratio, 0.68);
    assert_eq!(config.observations_pool_target_tokens, 4000);
    let model = config.model.clone().unwrap();
    assert_eq!(
        (model.provider.as_str(), model.id.as_str()),
        ("openai", "gpt-5-mini")
    );
    assert_eq!(config.thinking(), "minimal");

    let valid_target = merge(
        &[json!({ "observationsPoolTargetTokens": 3000 })],
        Some(true),
    );
    assert_eq!(valid_target.observations_pool_target_tokens, 3000);
    assert!(valid_target.passive);
    assert_eq!(env_passive(Some(" Yes ")), Some(true));
    assert_eq!(env_passive(Some("maybe")), None);
}

#[test]
fn thresholds_and_chunk_caps() {
    let mut config = merge(&[], None);
    assert_eq!(resolve_compact_after_tokens(&config, Some(200_000)), 81_000);
    config.compact_after_tokens_mode = "ratio".into();
    assert_eq!(
        resolve_compact_after_tokens(&config, Some(200_000)),
        136_000
    );
    assert_eq!(resolve_compact_after_tokens(&config, None), 81_000);

    assert_eq!(resolve_observer_chunk_max_tokens(&config, None), 60_000);
    assert_eq!(
        resolve_observer_chunk_max_tokens(&config, Some(128_000)),
        25_600
    );
    assert_eq!(resolve_observer_chunk_max_tokens(&config, Some(100)), 256);
    config.observer_chunk_max_tokens = Some(10);
    assert_eq!(
        resolve_observer_chunk_max_tokens(&config, Some(128_000)),
        256
    );

    assert_eq!(bounded_max_tokens(Some(8_192), 32_000), 8_192);
    assert_eq!(bounded_max_tokens(None, 32_000), 32_000);
}

#[test]
fn reads_config_files() {
    let dir = std::env::temp_dir().join(format!("om-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("observational-memory.json");
    std::fs::write(&path, r#"{ "agentMaxTurns": 4 }"#).unwrap();
    assert_eq!(read_json_file(&path), json!({ "agentMaxTurns": 4 }));
    assert_eq!(read_json_file(&dir.join("missing.json")), Value::Null);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn project_config_needs_trust() {
    use pi_ext_observational_memory::host_glue::{project_config_path, project_trusted};
    let snapshot = |trusted: Value| {
        serde_json::from_value(json!({ "cwd": "/w", "isProjectTrusted": trusted })).unwrap()
    };
    assert!(project_trusted(&snapshot(json!(true))));
    assert!(!project_trusted(&snapshot(json!(false))));
    assert!(!project_trusted(
        &serde_json::from_value(json!({ "cwd": "/w" })).unwrap()
    ));
    assert_eq!(
        project_config_path(std::path::Path::new("/w")),
        std::path::Path::new("/w/.pi/ext/observational-memory.json")
    );
}

#[test]
fn integer_valued_floats_are_accepted() {
    let config = merge(
        &[
            json!({ "observeAfterTokens": 10000.0, "reflectAfterTokens": 2.5, "agentMaxTokens": 0.0 }),
        ],
        None,
    );
    assert_eq!(config.observe_after_tokens, 10_000);
    assert_eq!(config.reflect_after_tokens, 20_000);
    assert_eq!(config.agent_max_tokens, 32_000);
}
