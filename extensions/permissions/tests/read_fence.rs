//! Claude `blockReadsOutsideWorkingDirectories` through the extension protocol: a read outside the working
//! directories is refused even with an allow rule, and a pi skill file stays readable.

#[allow(dead_code)]
mod common;

use common::*;
use pi_ext_sdk::test_util::TestHost;
use serde_json::{Value, json};

async fn call(
    host: &mut TestHost,
    fx: &Fx,
    ui: &mut Ui,
    id: u64,
    tool: &str,
    input: Value,
) -> Option<String> {
    let event =
        json!({ "type": "tool_call", "toolCallId": "t1", "toolName": tool, "input": input });
    let params = json!({ "name": "tool_call", "event": event, "ctx": ctx(fx, false, true) });
    blocked(&drive(host, id, "event", params, ui).await["result"]["result"])
}

#[tokio::test]
async fn reads_outside_are_refused_and_skill_files_and_the_cwd_stay_readable() {
    let fx = fx();
    let outside = fx.paths.home.parent().unwrap().join("outside/a.txt");
    write(&outside, "secret");
    let skill = fx.agent.join("skills/x/SKILL.md");
    write(&skill, "skill");
    let allow = format!("Read(/{})", outside.display());
    claude_settings(
        &fx.paths.claude_dir.join("settings.json"),
        json!({ "blockReadsOutsideWorkingDirectories": true, "defaultMode": "bypassPermissions", "allow": [allow] }),
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    let refused = call(
        &mut host,
        &fx,
        &mut ui,
        1,
        "read",
        json!({ "path": outside }),
    )
    .await;
    let refused =
        refused.expect("a read outside the working directories is refused in bypassPermissions");
    assert!(
        refused.contains("additionalDirectories in user settings"),
        "{refused}"
    );
    assert_eq!(
        call(&mut host, &fx, &mut ui, 2, "read", json!({ "path": skill })).await,
        None
    );
    assert_eq!(
        call(
            &mut host,
            &fx,
            &mut ui,
            3,
            "read",
            json!({ "path": "notes.txt" })
        )
        .await,
        None
    );
    let cat = call(
        &mut host,
        &fx,
        &mut ui,
        4,
        "bash",
        json!({ "command": format!("cat {}", outside.display()) }),
    )
    .await;
    assert!(
        cat.is_some_and(|reason| reason.contains("no UI")),
        "Bash asks, and without a UI it is blocked"
    );
    host.finish().await.unwrap();
}
