//! The compactor under the real pi binary: a scripted `context_budget` call.

use pi_ext_e2e::{Run, Turn, cargo_example, json, pi_bin_or_skip};

#[test]
fn context_budget_runs_in_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let binary = cargo_example("pi-ext-compactor", "compactor");
    let output = Run::new(binary)
        .args(["compactor"])
        .turns([
            Turn::tool_call("context_budget", json!({})),
            Turn::text("Budget checked."),
        ])
        .prompt("How full is the context?")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert!(output.tool_names(0).contains(&"session_recall".to_string()));
    let text = output
        .tool_result_text("context_budget")
        .unwrap_or_default();
    eprintln!("context_budget result: {text}");
    assert!(text.starts_with("Context"), "{text}\n{}", output.stderr);
    assert_eq!(output.final_text(), "Budget checked.");
}
