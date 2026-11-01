//! Writes to Claude protected paths through the extension protocol: without a UI the ask blocks with the
//! reason, with one it offers no "Allow always", an allow rule does not pre-approve, and a deny rule decides.

#[allow(dead_code)]
mod common;

use common::*;
use pi_ext_sdk::test_util::TestHost;
use serde_json::json;

async fn write_file(
    host: &mut TestHost,
    fx: &Fx,
    ui: &mut Ui,
    id: u64,
    path: &str,
    has_ui: bool,
) -> Option<String> {
    let event = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "write",
        "input": { "path": path, "content": "x" } });
    let params = json!({ "name": "tool_call", "event": event, "ctx": ctx(fx, has_ui, true) });
    blocked(&drive(host, id, "event", params, ui).await["result"]["result"])
}

#[tokio::test]
async fn an_allowed_protected_write_still_asks_and_an_ordinary_write_runs() {
    let fx = fx();
    claude_settings(
        &fx.paths.claude_dir.join("settings.json"),
        json!({ "defaultMode": "acceptEdits", "allow": ["Edit(./.claude/**)"], "deny": ["Edit(./.git/**)"] }),
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    let settings = write_file(&mut host, &fx, &mut ui, 1, ".claude/settings.json", false).await;
    let settings = settings.expect("a protected write asks, and without a UI it is blocked");
    assert!(
        settings.contains("Claude protected path") && settings.contains("no UI"),
        "{settings}"
    );
    let git = write_file(&mut host, &fx, &mut ui, 2, ".git/config", false)
        .await
        .expect("the deny rule");
    assert!(
        git.starts_with("Blocked by permission rule Edit(./.git/**)"),
        "{git}"
    );
    assert_eq!(
        write_file(&mut host, &fx, &mut ui, 3, "src/a.rs", false).await,
        None,
        "acceptEdits runs it"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn with_a_ui_a_protected_write_asks_without_allow_always_and_the_answer_decides() {
    let fx = fx();
    claude_settings(
        &fx.paths.claude_dir.join("settings.json"),
        json!({ "defaultMode": "acceptEdits", "allow": ["Edit(./.claude/**)"] }),
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    ui.selects.push_back(json!("Allow once"));
    let allowed = write_file(&mut host, &fx, &mut ui, 1, ".claude/settings.json", true).await;
    assert_eq!(allowed, None, "Allow once runs the call");
    {
        let asked = ui.methods("ui.select");
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(
            asked[0]["options"],
            json!(["Allow once", "Deny", "Deny with a reason"])
        );
        let title = asked[0]["title"].as_str().unwrap_or_default();
        assert!(title.contains("Claude protected path"), "{title}");
    }
    ui.selects.push_back(json!("Deny"));
    let denied = write_file(&mut host, &fx, &mut ui, 2, ".claude/settings.json", true).await;
    assert_eq!(denied.as_deref(), Some("The user denied this tool call."));
    assert_eq!(
        ui.methods("ui.select").len(),
        2,
        "each protected write asks again"
    );
    host.finish().await.unwrap();
}
