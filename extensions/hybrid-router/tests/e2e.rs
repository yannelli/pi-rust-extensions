//! `hybrid/auto` under the real pi binary. The local model is not in pi's catalog, so the prompt
//! goes to the remote model, the mock.

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn hybrid_auto_routes_to_the_remote_model_without_the_local_model() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let config = json!({ "local": "box/qwen", "remote": "mock/mock-model" });
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["hybrid-router"])
        .file("agent/ext/hybrid-router.json", config.to_string())
        .pi_args(["--model", "hybrid/auto"])
        .turns([Turn::text("Routed to the remote model.")])
        .prompt("Say hello")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(
        output.final_text(),
        "Routed to the remote model.",
        "{}",
        output.stderr
    );
    assert_eq!(output.requests.len(), 1);
    assert_eq!(output.requests[0]["model"], "mock-model");
}
