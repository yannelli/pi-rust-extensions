use super::*;
use crate::tests::Temp;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn profiles_trust_remote_mode_and_session_replay() {
    let temp = Temp::new();
    config::save(
        &temp.0.join("agent/ext/ssh.json"),
        &json!({"profiles":{"dev":{"host":"global-host"}}}),
    )
    .unwrap();
    config::save(
        &temp.0.join(".pi/ext/ssh.json"),
        &json!({"profiles":{"dev":{"host":"project-host"}}}),
    )
    .unwrap();
    let (client, server) = tokio::io::duplex(65536);
    let (read, write) = tokio::io::split(server);
    let serve = tokio::spawn(extension().serve(read, write));
    let (read, mut writer) = tokio::io::split(client);
    let mut lines = BufReader::new(read).lines();
    let ctx = json!({"cwd":temp.0,"hasUI":true});
    let requests = [
        (
            1,
            "initialize",
            json!({"protocolVersion":1,"cwd":temp.0,"agentDir":temp.0.join("agent")}),
        ),
        (
            2,
            "tool.execute",
            json!({"name":"ssh_profiles","params":{},"toolCallId":"list","ctx":ctx}),
        ),
        (
            3,
            "command.run",
            json!({"name":"ssh","args":"remote dev","ctx":ctx}),
        ),
        (
            4,
            "event",
            json!({"name":"tool_call","event":{"toolName":"bash","input":{"command":"echo hi"}},"ctx":ctx}),
        ),
        (
            5,
            "event",
            json!({"name":"session_tree","event":{},"ctx":ctx}),
        ),
        (
            6,
            "event",
            json!({"name":"tool_call","event":{"toolName":"bash","input":{"command":"local"}},"ctx":ctx}),
        ),
    ];
    let mut notices = 0;
    let mut entries = Vec::<Value>::new();
    for (id, method, params) in requests {
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
                let result = &message["result"];
                match id {
                    1 => {
                        assert_eq!(result["tools"].as_array().unwrap().len(), 4);
                        assert_eq!(result["commands"][0]["name"], "ssh");
                    }
                    2 => assert_eq!(result["details"]["profiles"][0]["host"], "global-host"),
                    4 => assert!(
                        result["patch"]["input"]["command"]
                            .as_str()
                            .unwrap()
                            .contains("global-host")
                    ),
                    6 => assert!(result.get("patch").is_none()),
                    _ => {}
                }
                break;
            }
            let result = match message["method"].as_str().unwrap() {
                "pi.call" => {
                    assert_eq!(message["params"]["method"], "isProjectTrusted");
                    json!(false)
                }
                "ui.notify" => {
                    notices += 1;
                    Value::Null
                }
                "session.appendEntry" => {
                    entries.push(message["params"].clone());
                    Value::Null
                }
                "ui.setStatus" => Value::Null,
                "session.getBranch" => json!([]),
                other => panic!("unexpected request {other}"),
            };
            if let Some(id) = message.get("id") {
                writer
                    .write_all(
                        format!("{}\n", json!({"jsonrpc":"2.0","id":id,"result":result}))
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
    }
    assert_eq!(notices, 1);
    assert_eq!(entries[0], json!({"customType":"ssh-remote","data":"dev"}));
    writer.shutdown().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), serve)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
