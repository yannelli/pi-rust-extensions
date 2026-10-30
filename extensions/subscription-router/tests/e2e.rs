//! `router/<profile>` under the real pi binary: the profile chain names the mock model.

use pi_ext_e2e::{Run, Turn, cargo_example, json, pi_bin_or_skip};

#[test]
fn router_profile_routes_to_the_mock_model() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("sr-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("subscription-router.json");
    let profile = json!({ "profiles": { "e2e": { "name": "Router e2e", "chain": [{ "model": "mock/mock-model" }] } } });
    std::fs::write(&config, profile.to_string()).unwrap();
    let output = Run::new(cargo_example("pi-ext-subscription-router", "router"))
        .env("PI_ROUTER_CONFIG", config.display().to_string())
        .pi_args(["--model", "router/e2e"])
        .turns([Turn::text("Routed through the chain.")])
        .prompt("Say hello")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(output.final_text(), "Routed through the chain.");
    assert_eq!(output.requests.len(), 1);
    assert_eq!(output.requests[0]["model"], "mock-model");
    let prompt = output.system_prompt(0);
    assert!(
        !prompt.contains("Treat usage as decision support"),
        "{prompt}"
    );
    std::fs::remove_dir_all(dir).ok();
}
