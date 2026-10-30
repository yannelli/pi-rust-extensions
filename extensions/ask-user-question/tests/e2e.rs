//! `ask_user_question` under the real pi binary in JSON mode, where pi has no UI.

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn json_mode_strips_the_tool() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["ask-user-question"])
        .env("PI_EXT_LOG", "debug")
        .turns([
            Turn::tool_call(
                "ask_user_question",
                json!({ "questions": [{ "question": "Which db?", "header": "DB", "options": [
                    { "label": "Postgres", "description": "server" },
                    { "label": "SQLite", "description": "file" }
                ] }] }),
            ),
            Turn::text("done"),
        ])
        .prompt("Ask me which database to use")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert!(
        output
            .stderr
            .contains("extension ask-user-question started"),
        "{}",
        output.stderr
    );
    assert!(
        !output
            .tool_names(0)
            .contains(&"ask_user_question".to_string()),
        "{:?}",
        output.tool_names(0)
    );
    assert_eq!(
        output.tool_result_text("ask_user_question").as_deref(),
        Some("Tool ask_user_question not found")
    );
}
