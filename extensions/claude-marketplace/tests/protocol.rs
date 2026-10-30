mod common;

use common::TestHost;
use common::fixture::Fixture;
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::{Scope, extension, install, market};
use serde_json::{Value, json};

fn ctx(fx: &Fixture, trusted: bool) -> Value {
    json!({ "cwd": fx.project, "hasUI": false, "flags": {}, "isProjectTrusted": trusted })
}

#[tokio::test]
async fn registers_and_answers_resources_discover() {
    let fx = Fixture::new();
    let env = fx.env(true);
    market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap();
    install::install(&env, "fmt@acme", Scope::User).unwrap();
    install::install(&env, "lint@acme", Scope::Project).unwrap();

    let mut host = TestHost::start(extension(fx.paths()));
    let init = host
        .initialize_in(fx.project.to_str().unwrap(), fx.agent.to_str().unwrap())
        .await;
    let tools: Vec<&str> = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        [
            "pi_claude_marketplace_list",
            "pi_claude_marketplace_plugin_list"
        ]
    );
    let commands = init["commands"].as_array().unwrap();
    assert_eq!(commands[0]["name"], "claude-plugins");
    assert_eq!(commands[1]["name"], "claude:plugin");
    assert_eq!(commands[1]["completions"], true);
    assert!(
        init["events"]
            .as_array()
            .unwrap()
            .contains(&json!("resources_discover"))
    );

    let user = Dirs::new(&fx.agent);
    let project = Dirs::new(&fx.project.join(".pi"));
    let event = json!({ "type": "resources_discover", "cwd": fx.project, "reason": "startup" });
    host.request(
        1,
        "event",
        json!({ "name": "resources_discover", "event": event, "ctx": ctx(&fx, true) }),
    )
    .await;
    let result = host.expect_response(1).await["result"]["result"].clone();
    assert_eq!(
        result["skillPaths"],
        json!([user.skills(), project.skills()])
    );
    assert_eq!(
        result["promptPaths"],
        json!([user.prompts(), project.prompts()])
    );

    host.request(
        2,
        "event",
        json!({ "name": "resources_discover", "event": event, "ctx": ctx(&fx, false) }),
    )
    .await;
    let mut result = Value::Null;
    for _ in 0..2 {
        let message = host.recv().await;
        if message["method"] == "ui.notify" {
            let text = message["params"]["message"].as_str().unwrap();
            assert!(text.contains("not trusted"), "{message}");
            host.answer(&message["id"], Value::Null).await;
        } else {
            assert_eq!(message["id"], 2, "{message}");
            result = message["result"]["result"].clone();
        }
    }
    assert_eq!(result["skillPaths"], json!([user.skills()]));

    let call = json!({ "toolCallId": "t1", "name": "pi_claude_marketplace_list", "params": {}, "ctx": ctx(&fx, true) });
    host.request(3, "tool.execute", call).await;
    let result = host.expect_response(3).await["result"].clone();
    let manifest = fx.repo.join(".claude-plugin/marketplace.json");
    let line = format!("[user] acme -- 4 plugin(s) -- {}", manifest.display());
    assert_eq!(result["content"][0]["text"], line);
    assert_eq!(result["details"]["marketplaces"][0]["pluginCount"], 4);

    let params = json!({ "installed": true });
    let call = json!({ "toolCallId": "t2", "name": "pi_claude_marketplace_plugin_list", "params": params, "ctx": ctx(&fx, true) });
    host.request(4, "tool.execute", call).await;
    let result = host.expect_response(4).await["result"].clone();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        text,
        "Marketplace acme (user)\n  [installed] fmt  1.0.0\n  [installed] lint  0.1.0  (unsupported: lspServers)"
    );
    assert_eq!(result["details"]["plugins"][1]["scope"], "project");

    let run =
        json!({ "name": "claude:plugin", "args": "uninstall lint@acme", "ctx": ctx(&fx, true) });
    host.request(5, "command.run", run).await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(
        params["message"],
        "Uninstalled lint@acme [project]. Run /reload to apply the changes in pi."
    );
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(5).await["result"], Value::Null);
    assert!(!project.prompts().join("lint:run.md").exists());

    host.request(
        6,
        "command.complete",
        json!({ "name": "claude-plugins", "prefix": "uni" }),
    )
    .await;
    let result = host.expect_response(6).await["result"].clone();
    assert_eq!(result, json!([{ "value": "uninstall" }]));
    host.finish().await.unwrap();
}
