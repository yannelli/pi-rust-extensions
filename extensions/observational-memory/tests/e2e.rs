//! `recall` under the real pi binary with a scripted tool call.

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn recall_unknown_id_through_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["observational-memory"])
        .turns([
            Turn::tool_call("recall", json!({ "id": "abcdefabcdef" })),
            Turn::text("Nothing to recall."),
        ])
        .prompt("Recall memory abcdefabcdef")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(
        output.tool_result_text("recall").as_deref(),
        Some("No observation or reflection with id abcdefabcdef was found on the current branch.")
    );
    assert!(output.tool_names(0).contains(&"recall".to_string()));
    assert!(
        output.system_prompt(0).contains("Use recall(<id>)"),
        "{}",
        output.system_prompt(0)
    );
    assert_eq!(output.final_text(), "Nothing to recall.");
}
