//! The SDK `hello` example under the pi binary: tool update and result, and a prompt section.

use pi_ext_e2e::{Run, Turn, cargo_example, json, pi_bin_or_skip};

#[test]
fn hello_example_greets_through_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let hello = cargo_example("pi-ext-sdk", "hello");
    let output = Run::new(hello)
        .args(["hello"])
        .turns([
            Turn::tool_call("greet", json!({"name": "Ada"})),
            Turn::text("The tool said hello."),
        ])
        .prompt("Greet Ada")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(output.tool_updates("greet"), ["working"]);
    assert_eq!(
        output.tool_result_text("greet").as_deref(),
        Some("Hello, Ada")
    );
    assert_eq!(output.requests.len(), 2);
    assert!(output.tool_names(0).contains(&"greet".to_string()));
    assert!(
        output.system_prompt(0).contains("Greet people by name."),
        "{}",
        output.system_prompt(0)
    );
    assert_eq!(output.final_text(), "The tool said hello.");
}
