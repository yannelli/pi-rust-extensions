//! Under the real pi binary: startup discovery registers provider `ollama`, and pi chats with
//! the mock through its OpenAI-compatible `/v1` endpoint.

mod mock;

use mock::MockOllama;
use pi_ext_e2e::{Run, cargo_bin, json, pi_bin_or_skip};

#[test]
fn pi_uses_a_discovered_ollama_model() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let mock = MockOllama::start(&["qwen3:8b"]);
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["ollama"])
        .env("OLLAMA_HOST", mock.url())
        .settings(json!({"defaultProvider": "ollama", "defaultModel": "qwen3:8b"}))
        .prompt("Say hello")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(
        output.final_text(),
        "Hello from Ollama",
        "{}",
        output.stderr
    );
    let chat: Vec<_> = mock
        .requests()
        .into_iter()
        .filter(|(_, path, _)| path == "/v1/chat/completions")
        .collect();
    assert_eq!(chat.len(), 1);
    assert_eq!(chat[0].2["model"], "qwen3:8b");
}
