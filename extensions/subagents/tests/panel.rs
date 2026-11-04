//! `/agents` over the protocol: the panel, keys, the prompt editor, saving, and the fallback.

#![cfg(unix)]

mod common;

use common::{TestHost, ctx, dirs, scripted_events};
use serde_json::{Value, json};

fn lines_of(params: &Value) -> String {
    let lines = params["lines"].as_array().cloned().unwrap_or_default();
    let text: Vec<String> = lines.iter().filter_map(Value::as_str).map(strip).collect();
    text.join("\n")
}

/// Drops ANSI escapes.
fn strip(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for e in chars.by_ref() {
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

async fn press(host: &mut TestHost, surface: &Value, data: &str) {
    host.notify("ui.key", json!({ "surface": surface, "data": data }))
        .await;
}

/// Waits for a `ui.update` whose text contains `needle`.
async fn until_shown(host: &mut TestHost, needle: &str) -> String {
    for _ in 0..20 {
        let text = lines_of(&host.until_method("ui.update").await);
        if text.contains(needle) {
            return text;
        }
    }
    panic!("never showed {needle:?}");
}

#[tokio::test]
async fn panel_edits_the_prompt_and_saves() {
    let d = dirs(&scripted_events("done"), 0);
    let global = d.agent.join("ext/subagents/agents");
    std::fs::create_dir_all(&global).unwrap();
    let file = global.join("reviewer.md");
    std::fs::write(
        &file,
        "---\nname: reviewer\ndescription: Reviews.\n---\nOld.\n",
    )
    .unwrap();
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    host.answer(
        "pi.call",
        json!([{ "provider": "openai", "id": "gpt-5", "name": "GPT-5" }]),
    );
    host.answer(
        "tools.getAll",
        json!([{ "name": "read" }, { "name": "bash" }]),
    );
    host.request(
        1,
        "command.run",
        json!({ "name": "agents", "args": "reviewer", "ctx": ctx(&d.cwd) }),
    )
    .await;
    let custom = host.until_method("ui.custom").await;
    let text = lines_of(&custom);
    assert!(text.contains("reviewer global"), "{text}");
    assert!(text.contains("explore  built-in"), "{text}");
    let surface = custom["surface"].clone();
    host.answer("ui.editor", json!("New prompt."));
    press(&mut host, &surface, "p").await;
    let editor = host.until_method("ui.editor").await;
    assert_eq!(editor["prefill"], "Old.");
    until_shown(&mut host, "unsaved").await;
    press(&mut host, &surface, "s").await;
    let text = until_shown(&mut host, "Saved").await;
    assert!(text.contains("✓ Saved"), "{text}");
    let saved = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        saved,
        "---\nname: reviewer\ndescription: Reviews.\n---\n\nNew prompt.\n"
    );
    press(&mut host, &surface, "q").await;
    let close = host.until_method("ui.close").await;
    assert_eq!(close["surface"], surface);
    host.until_response(1).await;
    host.finish().await;
}

#[tokio::test]
async fn no_ui_lists_agents_and_runs() {
    let d = dirs(&scripted_events("done"), 0);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let mut no_ui = ctx(&d.cwd);
    no_ui["hasUI"] = json!(false);
    for (id, args) in [(1, ""), (2, "runs"), (3, "list")] {
        let params = json!({ "name": "agents", "args": args, "ctx": no_ui.clone() });
        host.request(id, "command.run", params).await;
        host.until_response(id).await;
    }
    let notes = host.seen_of("ui.notify");
    let texts: Vec<&str> = notes.iter().filter_map(|n| n["message"].as_str()).collect();
    assert!(texts[0].contains("- explore (built-in):"), "{texts:?}");
    assert_eq!(texts[1], "No subagent runs.");
    assert!(texts[2].contains("- advisor (built-in):"), "{texts:?}");
    assert!(host.seen_of("ui.custom").is_empty());
    host.finish().await;
}
