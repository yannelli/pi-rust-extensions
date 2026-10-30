use pi_ext_sdk::prelude::*;

use crate::test_host::Host;
use crate::tests::Temp;

fn task_id(text: &str) -> String {
    let start = text.find('(').unwrap() + 1;
    text[start..start + 8].to_owned()
}

#[tokio::test]
async fn tools_notify_and_shutdown_over_protocol() {
    let temp = Temp::new();
    let agent = temp.0.join("agent");
    let ctx = json!({"cwd":temp.0,"hasUI":false,"sessionId":"sess-1"});
    let mut host = Host::start();
    let init = json!({"protocolVersion":1,"cwd":temp.0,"agentDir":agent,"hasUI":false});
    let init = host.call(0, "initialize", init).await;
    let tools: Vec<&str> = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["bg_run", "bg_status", "bg_logs", "bg_kill"]);
    host.call(
        1,
        "event",
        json!({"name":"session_start","event":{"reason":"startup"},"ctx":ctx}),
    )
    .await;

    let started = host
        .tool(
            2,
            &ctx,
            "bg_run",
            json!({"name":"Echo","command":"echo hi-there","isAgent":false}),
        )
        .await;
    assert!(
        started.starts_with("Started background task Echo"),
        "{started}"
    );
    let echo = task_id(&started);
    assert!(
        agent
            .join("ext/background-tasks/sessions/sess-1")
            .join(&echo)
            .join("task.json")
            .exists()
    );

    let message = host.wait_request("session.sendMessage").await;
    assert_eq!(message["customType"], "background-task-notification");
    assert_eq!(message["deliverAs"], "followUp");
    assert_eq!(message["triggerTurn"], true);
    assert_eq!(message["details"]["status"], "completed");
    assert!(message["content"].as_str().unwrap().contains("hi-there"));

    let logs = host
        .tool(3, &ctx, "bg_logs", json!({"taskId":&echo[..4]}))
        .await;
    assert!(
        logs.contains("[completed]") && logs.contains("hi-there"),
        "{logs}"
    );

    let sleeper = host
        .tool(
            4,
            &ctx,
            "bg_run",
            json!({"name":"Sleep","command":"sleep 30","isAgent":false}),
        )
        .await;
    let sleeper = task_id(&sleeper);
    let killed = host
        .tool(5, &ctx, "bg_kill", json!({"taskId":sleeper}))
        .await;
    assert!(
        killed.starts_with("Killed background task Sleep"),
        "{killed}"
    );
    let status = host.tool(6, &ctx, "bg_status", json!({})).await;
    assert!(
        status.contains("[completed]") && status.contains("[killed]"),
        "{status}"
    );
    let missing = host
        .tool(7, &ctx, "bg_kill", json!({"taskId":"zzzz"}))
        .await;
    assert!(missing.contains("no background task"), "{missing}");

    let reply = host
        .call(
            8,
            "event",
            json!({"name":"before_agent_start","event":{},"ctx":ctx}),
        )
        .await;
    let section = &reply["patch"]["systemPromptOptions"]["sections"]["pi_background_shell_policy"];
    assert!(section.as_str().unwrap().contains("bg_run"));

    let survivor = host
        .tool(
            9,
            &ctx,
            "bg_run",
            json!({"name":"Keep","command":"sleep 30","isAgent":false}),
        )
        .await;
    let survivor = task_id(&survivor);
    host.call(
        10,
        "event",
        json!({"name":"session_shutdown","event":{"reason":"reload"},"ctx":ctx}),
    )
    .await;
    let status = host
        .tool(11, &ctx, "bg_status", json!({"taskId":survivor}))
        .await;
    assert!(status.contains("[running]"), "{status}");
    host.call(
        12,
        "event",
        json!({"name":"session_shutdown","event":{"reason":"quit"},"ctx":ctx}),
    )
    .await;
    let status = host
        .tool(13, &ctx, "bg_status", json!({"taskId":survivor}))
        .await;
    assert!(status.contains("[killed]"), "{status}");
    let notices = host
        .requests
        .iter()
        .filter(|r| r["method"] == "session.sendMessage")
        .count();
    assert_eq!(notices, 1, "killed tasks do not send notifications");
}

#[tokio::test]
async fn bg_command_and_tasks_panel_over_protocol() {
    let temp = Temp::new();
    let ctx = json!({"cwd":temp.0,"hasUI":true,"sessionId":"ui"});
    let mut host = Host::start();
    let init =
        json!({"protocolVersion":1,"cwd":temp.0,"agentDir":temp.0.join("agent"),"hasUI":true});
    host.call(0, "initialize", init).await;
    host.call(
        1,
        "command.run",
        json!({"name":"bg","args":"--name \"Long sleep\" sleep 30","ctx":ctx}),
    )
    .await;
    let notify = host
        .requests
        .iter()
        .find(|r| r["method"] == "ui.notify")
        .unwrap();
    assert!(
        notify["params"]["message"]
            .as_str()
            .unwrap()
            .contains("Started background task Long sleep")
    );
    let status = host
        .requests
        .iter()
        .rev()
        .find(|r| r["method"] == "ui.setStatus")
        .unwrap();
    assert_eq!(status["params"]["text"], "bg: 1 running");
    assert!(host.requests.iter().any(|r| r["method"] == "ui.setWidget"));

    host.send(json!({"jsonrpc":"2.0","id":2,"method":"command.run","params":{"name":"tasks","args":"","ctx":ctx}})).await;
    let custom = loop {
        let message = host.recv().await;
        if message["method"] == "ui.custom" {
            break message;
        }
        if let (Some(_), Some(id)) = (message.get("method"), message.get("id")) {
            let id = id.clone();
            host.send(json!({"jsonrpc":"2.0","id":id,"result":{}}))
                .await;
        }
    };
    let surface = custom["params"]["surface"].clone();
    let text = custom["params"]["lines"].to_string();
    assert!(text.contains("Long sleep"), "{text}");
    for key in ["x", "x"] {
        host.send(
            json!({"jsonrpc":"2.0","method":"ui.key","params":{"surface":surface,"data":key}}),
        )
        .await;
    }
    let stopped = loop {
        let message = host.recv().await;
        if message["method"] == "ui.update"
            && message["params"]["lines"]
                .to_string()
                .contains("Stopped Long sleep")
        {
            break true;
        }
        if let (Some(_), Some(id)) = (message.get("method"), message.get("id")) {
            let id = id.clone();
            host.send(json!({"jsonrpc":"2.0","id":id,"result":{}}))
                .await;
        }
    };
    assert!(stopped);
    host.send(json!({"jsonrpc":"2.0","method":"ui.key","params":{"surface":surface,"data":"q"}}))
        .await;
    loop {
        let message = host.recv().await;
        if message["method"] == "ui.close" {
            break;
        }
    }
    host.send(json!({"jsonrpc":"2.0","id":custom["id"],"result":{"result":null}}))
        .await;
    loop {
        let message = host.recv().await;
        if message["id"] == 2 && message.get("method").is_none() {
            assert!(message.get("error").is_none(), "{message}");
            break;
        }
        if let (Some(_), Some(id)) = (message.get("method"), message.get("id")) {
            let id = id.clone();
            host.send(json!({"jsonrpc":"2.0","id":id,"result":{}}))
                .await;
        }
    }
}
