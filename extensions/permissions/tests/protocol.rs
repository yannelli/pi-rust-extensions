mod common;

use common::*;
use serde_json::{Value, json};

#[tokio::test]
async fn claude_deny_rule_blocks_with_reason() {
    let fx = fx();
    claude_settings(
        &fx.paths.claude_dir.join("settings.json"),
        json!({ "deny": ["Bash(git push *)"] }),
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    let result = bash(
        &mut host,
        1,
        "git status && git push origin main",
        ctx(&fx, true, false),
        &mut ui,
    )
    .await;
    let reason = blocked(&result).expect("blocked");
    assert!(
        reason.contains("Bash(git push *) from claude-user"),
        "{reason}"
    );
    assert_eq!(
        blocked(&bash(&mut host, 2, "git status", ctx(&fx, true, false), &mut ui).await),
        None
    );
    assert!(ui.methods("ui.select").is_empty());
    host.finish().await.unwrap();
}

#[tokio::test]
async fn codex_forbidden_rule_blocks_with_justification() {
    let fx = fx();
    write(
        &fx.paths.codex_home.join("rules/default.rules"),
        r#"prefix_rule(pattern=["rm", "-rf"], decision="forbidden", justification="Use trash instead.")"#,
    );
    let mut host = start(&fx).await;
    let result = bash(
        &mut host,
        1,
        "ls | rm -rf build",
        ctx(&fx, false, false),
        &mut Ui::default(),
    )
    .await;
    let reason = blocked(&result).expect("blocked");
    assert!(
        reason.contains("codex-user") && reason.ends_with("Use trash instead."),
        "{reason}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn ask_allow_always_saves_to_the_chosen_scope() {
    let fx = fx();
    claude_settings(
        &fx.paths.claude_dir.join("settings.json"),
        json!({ "ask": ["Bash(npm publish *)"] }),
    );
    write(
        &fx.agent.join("ext/permissions.json"),
        r#"{"scope": "project"}"#,
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    ui.selects.push_back(json!("Allow always"));
    let project_file = fx.cwd.join(".pi/ext/permissions.json");
    ui.selects.push_back(json!(format!(
        "Save to project ({})",
        project_file.display()
    )));
    let result = bash(
        &mut host,
        1,
        "npm publish --dry-run",
        ctx(&fx, true, true),
        &mut ui,
    )
    .await;
    assert_eq!(blocked(&result), None);
    let selects = ui.methods("ui.select");
    assert_eq!(
        selects[0]["options"],
        json!(["Allow once", "Allow always", "Deny", "Deny with a reason"])
    );
    assert!(
        selects[0]["title"]
            .as_str()
            .unwrap()
            .contains("npm publish --dry-run")
    );
    assert_eq!(
        selects[1]["options"][0],
        format!("Save to project ({})", project_file.display())
    );
    assert_eq!(selects[1]["options"].as_array().unwrap().len(), 2);
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&project_file).unwrap()).unwrap();
    assert_eq!(saved["allow"], json!(["Bash(npm publish --dry-run)"]));
    let mut again = Ui::default();
    let result = bash(
        &mut host,
        2,
        "npm publish --dry-run",
        ctx(&fx, true, true),
        &mut again,
    )
    .await;
    assert_eq!(blocked(&result), None);
    assert!(again.methods("ui.select").is_empty());
    host.finish().await.unwrap();
}

#[tokio::test]
async fn ask_deny_paths() {
    let fx = fx();
    write(
        &fx.agent.join("ext/permissions.json"),
        r#"{"ask": ["Bash(curl *)"]}"#,
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    ui.selects.push_back(json!("Deny with a reason"));
    ui.inputs.push_back(json!("use the cached file"));
    let result = bash(&mut host, 1, "curl x", ctx(&fx, true, false), &mut ui).await;
    assert_eq!(
        blocked(&result).unwrap(),
        "The user denied this tool call: use the cached file"
    );
    let mut ui = Ui::default();
    let result = bash(&mut host, 2, "curl x", ctx(&fx, true, false), &mut ui).await;
    assert_eq!(blocked(&result).unwrap(), "The user denied this tool call.");
    let mut ui = Ui::default();
    ui.selects.push_back(json!("Allow once"));
    assert_eq!(
        blocked(&bash(&mut host, 3, "curl x", ctx(&fx, true, false), &mut ui).await),
        None
    );
    let result = bash(
        &mut host,
        4,
        "curl x",
        ctx(&fx, false, false),
        &mut Ui::default(),
    )
    .await;
    assert!(blocked(&result).unwrap().contains("no UI"));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn project_sources_need_trust_and_sources_can_be_off() {
    let fx = fx();
    claude_settings(
        &fx.cwd.join(".claude/settings.json"),
        json!({ "deny": ["Bash(make *)"] }),
    );
    write(
        &fx.cwd.join(".codex/rules/p.rules"),
        r#"prefix_rule(pattern=["cargo"], decision="forbidden")"#,
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    assert_eq!(
        blocked(&bash(&mut host, 1, "make", ctx(&fx, false, false), &mut ui).await),
        None
    );
    assert_eq!(
        blocked(&bash(&mut host, 2, "cargo b", ctx(&fx, false, false), &mut ui).await),
        None
    );
    assert!(blocked(&bash(&mut host, 3, "make", ctx(&fx, false, true), &mut ui).await).is_some());
    assert!(
        blocked(&bash(&mut host, 4, "cargo b", ctx(&fx, false, true), &mut ui).await).is_some()
    );
    write(
        &fx.agent.join("ext/permissions.json"),
        r#"{"sources": {"claude-project": false}}"#,
    );
    assert_eq!(
        blocked(&bash(&mut host, 5, "make", ctx(&fx, false, true), &mut ui).await),
        None
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn panel_adds_rules_and_switches_scope() {
    let fx = fx();
    claude_settings(
        &fx.paths.claude_dir.join("settings.json"),
        json!({ "deny": ["Bash(git push *)"] }),
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    ui.keys.push_back(vec!["a"]);
    ui.inputs.push_back(json!("Bash(rm *)"));
    ui.selects.push_back(json!("ask"));
    ui.keys.push_back(vec!["c"]);
    ui.keys.push_back(vec!["\u{1b}"]);
    let run = json!({ "name": "permissions", "args": "", "ctx": ctx(&fx, true, true) });
    drive(&mut host, 1, "command.run", run, &mut ui).await;
    let global: Value = serde_json::from_str(
        &std::fs::read_to_string(fx.agent.join("ext/permissions.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(global["ask"], json!(["Bash(rm *)"]));
    assert_eq!(global["scope"], "project");
    let panels = ui.methods("ui.custom");
    assert_eq!(panels.len(), 3);
    let last = panels[2]["lines"].to_string();
    assert!(
        last.contains("new rules: project") && last.contains("Bash(rm *)"),
        "{last}"
    );
    assert!(last.contains("[claude-user]"), "{last}");

    let mut ui = Ui::default();
    let run = json!({ "name": "permissions", "args": "test git push origin", "ctx": ctx(&fx, true, true) });
    drive(&mut host, 2, "command.run", run, &mut ui).await;
    let text = ui.methods("ui.notify")[0]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.starts_with("bash: deny") && text.contains("Bash(git push *)"),
        "{text}"
    );
    host.finish().await.unwrap();
}
