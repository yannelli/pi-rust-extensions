#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

use common::fixture::Fixture;
use pi_ext_claude_marketplace::{Scope, install, market};
use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin, pi_bin_or_skip};

#[test]
fn staged_skill_and_list_tool_reach_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let fx = Fixture::new();
    let env = fx.env(true);
    market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap();
    install::install(&env, "fmt@acme", Scope::User).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let wrapper = dir.path().join("pi");
    let script = format!(
        "#!/bin/sh\nmkdir -p \"$PI_CODING_AGENT_DIR\"\ncp -R '{}' \"$PI_CODING_AGENT_DIR/\"\nexec '{}' \"$@\"\n",
        fx.agent.join("ext").display(),
        pi_bin().display()
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["claude-marketplace"])
        .pi(&wrapper)
        .turns([
            Turn::tool_call("pi_claude_marketplace_list", json!({})),
            Turn::text("done"),
        ])
        .prompt("List the marketplaces")
        .run()
        .unwrap();
    let prompt = output.system_prompt(0);
    assert!(
        prompt.contains("fmt-check"),
        "staged skill missing from the system prompt\n{prompt}\nstderr:\n{}",
        output.stderr
    );
    let text = output.tool_result_text("pi_claude_marketplace_list");
    let text = text.unwrap_or_else(|| panic!("no tool result; stderr:\n{}", output.stderr));
    assert!(text.starts_with("[user] acme -- 4 plugin(s) -- "), "{text}");
}
