//! A Codex rules file with an error, as Codex `load_exec_policy_with_warning` handles it: no Codex system, user,
//! or project rule and no `host_executable` applies, while managed, Claude, and pi rules still decide.

#[allow(dead_code)]
mod common;

use common::*;
use pi_ext_sdk::test_util::TestHost;

async fn decide(
    host: &mut TestHost,
    fx: &Fx,
    ui: &mut Ui,
    id: u64,
    command: &str,
) -> Option<String> {
    blocked(&bash(host, id, command, ctx(fx, false, true), ui).await)
}

#[tokio::test]
async fn a_bad_codex_rules_file_leaves_only_managed_claude_and_pi_rules_deciding() {
    let fx = fx();
    write(
        &fx.paths.codex_home.join("rules/default.rules"),
        "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\", justification=\"user rm\")\n\
         host_executable(name=\"git\", paths=[\"/opt/git\"])",
    );
    write(
        &fx.paths.codex_home.join("rules/zz-bad.rules"),
        r#"network_rule(host="evil.example", protocol="ftp", decision="deny")"#,
    );
    write(
        &fx.cwd.join(".codex/rules/project.rules"),
        r#"prefix_rule(pattern=["curl"], decision="forbidden", justification="project curl")"#,
    );
    write(
        &fx.paths.codex_requirements[0],
        "[rules]\nprefix_rules = [{ pattern = [{ token = \"git\" }, { token = \"push\" }], decision = \"forbidden\", justification = \"managed push\" }]\n",
    );
    claude_settings(
        &fx.paths.claude_dir.join("settings.json"),
        serde_json::json!({ "deny": ["Bash(echo *)"] }),
    );
    write(
        &fx.agent.join("ext/permissions.json"),
        r#"{"deny": ["Bash(touch *)"]}"#,
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    let rm = decide(&mut host, &fx, &mut ui, 1, "rm build").await;
    assert_eq!(rm, None, "the user rule still applies");
    let curl = decide(&mut host, &fx, &mut ui, 2, "curl https://x.example").await;
    assert_eq!(curl, None, "the project rule still applies");
    let push = decide(&mut host, &fx, &mut ui, 3, "/usr/bin/git push origin main").await;
    let push =
        push.expect("the managed rule, with the dropped host_executable no longer limiting git");
    assert!(
        push.contains("codex-managed") && push.ends_with("managed push"),
        "{push}"
    );
    let echo = decide(&mut host, &fx, &mut ui, 4, "echo hi")
        .await
        .expect("Claude rule");
    assert!(echo.contains("from claude-user"), "{echo}");
    let touch = decide(&mut host, &fx, &mut ui, 5, "touch made.txt")
        .await
        .expect("pi rule");
    assert!(touch.contains("from pi-global"), "{touch}");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn an_unreadable_codex_rules_file_blocks_a_call_that_a_pi_rule_allows_until_it_is_fixed() {
    let fx = fx();
    write(
        &fx.agent.join("ext/permissions.json"),
        r#"{"allow": ["Bash(ls *)"]}"#,
    );
    let bad = fx.paths.codex_home.join("rules/bad.rules");
    write(&bad, "");
    std::fs::write(&bad, [0xff, 0xfe]).unwrap();
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    let reason = decide(&mut host, &fx, &mut ui, 1, "ls")
        .await
        .expect("Codex rules that cannot be read block the call");
    assert!(
        reason.starts_with("Codex rules cannot be read")
            && reason.contains(&bad.display().to_string()),
        "{reason}"
    );
    write(&bad, r#"prefix_rule(pattern=["rm"], decision="forbidden")"#);
    assert_eq!(
        decide(&mut host, &fx, &mut ui, 2, "ls").await,
        None,
        "each call loads the fixed file"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn an_invalid_requirements_file_blocks_every_tool_until_it_is_fixed() {
    let fx = fx();
    write(
        &fx.agent.join("ext/permissions.json"),
        r#"{"allow": ["Bash(ls *)"]}"#,
    );
    let requirements = &fx.paths.codex_requirements[0];
    write(
        requirements,
        "[rules]\nprefix_rules = [{ pattern = [{ token = \" \" }], decision = \"forbidden\" }]\n",
    );
    let mut host = start(&fx).await;
    let mut ui = Ui::default();
    let reason = decide(&mut host, &fx, &mut ui, 1, "ls")
        .await
        .expect("a requirements rule error blocks the call that a pi rule allows");
    assert!(
        reason.starts_with("Codex rules cannot be read")
            && reason.contains(&requirements.display().to_string()),
        "{reason}"
    );
    let event = serde_json::json!({ "type": "tool_call", "toolCallId": "t2", "toolName": "read",
        "input": { "path": "README.md" } });
    let params =
        serde_json::json!({ "name": "tool_call", "event": event, "ctx": ctx(&fx, false, true) });
    let read = drive(&mut host, 2, "event", params, &mut ui).await;
    assert_eq!(
        blocked(&read["result"]["result"]).as_deref(),
        Some(reason.as_str()),
        "every tool gets the same reason"
    );
    write(
        requirements,
        "[rules]\nprefix_rules = [{ pattern = [{ token = \"rm\" }], decision = \"forbidden\" }]\n",
    );
    assert_eq!(
        decide(&mut host, &fx, &mut ui, 3, "ls").await,
        None,
        "each call loads the fixed file"
    );
    host.finish().await.unwrap();
}
