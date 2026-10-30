//! The todo extension under the real pi binary: create, then list.

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn todo_create_then_list_in_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["todo"])
        .turns([
            Turn::tool_call(
                "todo",
                json!({ "action": "create", "subject": "Write the parser" }),
            ),
            Turn::tool_call("todo", json!({ "action": "list" })),
            Turn::text("Done."),
        ])
        .prompt("Plan the parser work")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let results: Vec<String> = output
        .tool_results("todo")
        .iter()
        .map(|r| {
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(
        results,
        [
            "Created #1: Write the parser (pending)",
            "[pending] #1 Write the parser"
        ]
    );
    assert!(output.tool_names(0).contains(&"todo".to_string()));
    assert_eq!(output.final_text(), "Done.");
}
