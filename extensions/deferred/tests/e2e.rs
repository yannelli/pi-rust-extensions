#![cfg(unix)]

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

const SKILL: &str =
    "---\nname: demo\ndescription: Demo skill for the e2e test\n---\nDEMO-SKILL-BODY-41c2\n";

#[test]
fn deferred_tool_loads_on_the_next_call_and_skills_list_by_name() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let settings = json!({ "deferred": { "tools": { "bash": "names" }, "skills": { "demo": "names" }, "context": { "AGENTS.md": "names" } } });
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["deferred"])
        .settings(settings)
        .file("agent/skills/demo/SKILL.md", SKILL)
        .file("work/AGENTS.md", "# Rules\nCTX-MARKER-9d07\n")
        .turns([
            Turn::tool_call("deferred_search", json!({ "query": "select:bash,demo" })),
            Turn::text("done"),
        ])
        .prompt("hello")
        .run()
        .unwrap();
    let stderr = &output.stderr;
    let first = output.tool_names(0);
    assert!(
        first.contains(&"deferred_search".to_owned()),
        "{first:?}\n{stderr}"
    );
    assert!(first.contains(&"read".to_owned()), "{first:?}");
    assert!(!first.contains(&"bash".to_owned()), "{first:?}");
    let prompt = output.system_prompt(0);
    assert!(prompt.contains("Deferred tools: bash"), "{prompt}");
    assert!(prompt.contains("Deferred skills: demo"), "{prompt}");
    assert!(!prompt.contains("<name>demo</name>"), "{prompt}");
    assert!(
        prompt.contains("Deferred context files: ") && prompt.contains("AGENTS.md"),
        "{prompt}"
    );
    assert!(!prompt.contains("CTX-MARKER-9d07"), "{prompt}");

    let text = output
        .tool_result_text("deferred_search")
        .unwrap_or_default();
    assert!(text.starts_with("Loaded 1 tool(s)"), "{text}\n{stderr}");
    assert!(text.contains("DEMO-SKILL-BODY-41c2"), "{text}");
    assert!(
        output.tool_names(1).contains(&"bash".to_owned()),
        "{:?}",
        output.tool_names(1)
    );
}
