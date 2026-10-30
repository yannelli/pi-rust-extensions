use super::*;
use crate::tests::Temp;
use std::collections::VecDeque;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn toggle_writes_only_enabled_in_the_selected_scope() {
    for scope in ["Global", "Project"] {
        let temp = Temp::new();
        let agent = temp.0.join("agent");
        let global = agent.join("ext/ssh.json");
        let project = temp.0.join(".pi/ext/ssh.json");
        let original_global = json!({"profiles":{"dev":{"host":"global","user":"alice"}}});
        let original_project = json!({"profiles":{"dev":{"host":"project"}}});
        config::save(&global, &original_global).unwrap();
        config::save(&project, &original_project).unwrap();
        let (client, server) = tokio::io::duplex(65536);
        let (reader, writer) = tokio::io::split(server);
        let server = tokio::spawn(extension().serve(reader, writer));
        let (reader, mut writer) = tokio::io::split(client);
        let mut lines = BufReader::new(reader).lines();
        let mut choices = VecDeque::from([
            json!("dev (project)"),
            json!("Enable or disable"),
            json!(scope),
            Value::Null,
        ]);
        for (id, method, params) in [
            (
                1,
                "initialize",
                json!({"protocolVersion":1,"cwd":temp.0,"agentDir":agent}),
            ),
            (
                2,
                "command.run",
                json!({"name":"ssh","args":"","ctx":{"cwd":temp.0,"hasUI":true}}),
            ),
        ] {
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            loop {
                let line =
                    tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                let message: Value = serde_json::from_str(&line).unwrap();
                if message.get("method").is_none() {
                    assert_eq!(message["id"], id);
                    assert!(message.get("error").is_none(), "{message}");
                    break;
                }
                let answer = match message["method"].as_str().unwrap() {
                    "pi.call" => json!(true),
                    "ui.select" => choices.pop_front().unwrap(),
                    "ui.setStatus" => Value::Null,
                    method => panic!("unexpected request {method}"),
                };
                writer
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"jsonrpc":"2.0","id":message["id"],"result":answer})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
        let mut expected_global = original_global;
        let mut expected_project = original_project;
        if scope == "Global" {
            expected_global["profiles"]["dev"]["enabled"] = json!(false);
        } else {
            expected_project["profiles"]["dev"]["enabled"] = json!(false);
        }
        assert_eq!(config::read(&global).unwrap(), expected_global);
        assert_eq!(config::read(&project).unwrap(), expected_project);
        writer.shutdown().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn menu_saves_edits_toggles_and_deletes_profiles() {
    let temp = Temp::new();
    let agent = temp.0.join("agent");
    let (client, server) = tokio::io::duplex(65536);
    let (reader, writer) = tokio::io::split(server);
    let server = tokio::spawn(extension().serve(reader, writer));
    let (reader, mut writer) = tokio::io::split(client);
    let mut lines = BufReader::new(reader).lines();
    let mut answers: VecDeque<(&str, Value)> = VecDeque::from([
        ("ui.select", json!("Add profile")),
        ("ui.select", json!("Global")),
        ("ui.input", json!("dev")),
        ("ui.editor", json!("{\"host\":\"first\"}")),
        ("ui.notify", Value::Null),
        ("ui.select", json!("dev (first)")),
        ("ui.select", json!("Edit")),
        ("ui.select", json!("Global")),
        ("ui.editor", json!("{\"host\":\"second\",\"port\":2222}")),
        ("ui.notify", Value::Null),
        ("ui.select", json!("dev (second)")),
        ("ui.select", json!("Enable or disable")),
        ("ui.select", json!("Global")),
        ("ui.setStatus", Value::Null),
        ("ui.select", json!("dev (second) [disabled]")),
        ("ui.select", json!("Delete")),
        ("ui.select", json!("Global")),
        ("ui.confirm", json!(true)),
        ("ui.setStatus", Value::Null),
        ("ui.select", Value::Null),
    ]);
    for (id, method, params) in [
        (
            1,
            "initialize",
            json!({"protocolVersion":1,"cwd":temp.0,"agentDir":agent}),
        ),
        (
            2,
            "command.run",
            json!({"name":"ssh","args":"","ctx":{"cwd":temp.0,"hasUI":true}}),
        ),
    ] {
        writer
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let message: Value = serde_json::from_str(&line).unwrap();
            if message.get("method").is_none() {
                assert_eq!(message["id"], id);
                assert!(message.get("error").is_none(), "{message}");
                break;
            }
            let (expected, answer) = answers.pop_front().expect("unexpected host request");
            assert_eq!(message["method"], expected, "{message}");
            if expected == "ui.select" && answer == "dev (second) [disabled]" {
                let config = config::read(&agent.join("ext/ssh.json")).unwrap();
                assert_eq!(config["profiles"]["dev"]["host"], "second");
                assert_eq!(config["profiles"]["dev"]["port"], 2222);
                assert_eq!(config["profiles"]["dev"]["enabled"], false);
            }
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":message["id"],"result":answer})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    }
    assert!(answers.is_empty());
    assert_eq!(
        config::read(&agent.join("ext/ssh.json")).unwrap()["profiles"],
        json!({})
    );
    writer.shutdown().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
