use pi_ext_sdk::prelude::*;

use crate::test_host::Host;
use crate::tests::Temp;

fn execute(id: u64, ctx: &Value, name: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tool.execute","params":{"name":name,"params":params,"toolCallId":format!("c{id}"),"ctx":ctx}})
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn last_status(host: &Host) -> Value {
    host.requests
        .iter()
        .rev()
        .find(|r| r["method"] == "ui.setStatus")
        .map(|r| r["params"]["text"].clone())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn sleep_wakes_on_agent_wake_and_input() {
    let temp = Temp::new();
    let ctx = json!({"cwd":temp.0,"hasUI":false,"sessionId":"sess-1"});
    let mut host = Host::start();
    let init =
        json!({"protocolVersion":1,"cwd":temp.0,"agentDir":temp.0.join("agent"),"hasUI":false});
    let init = host.call(0, "initialize", init).await;
    let tools: Vec<&str> = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tools, ["sleep", "heartbeat", "schedule"]);
    let start = json!({"name":"session_start","event":{"reason":"startup"},"ctx":ctx});
    host.call(1, "event", start).await;

    host.send(execute(2, &ctx, "sleep", json!({"seconds":30})))
        .await;
    let subscribe = host.wait_request("events.subscribe").await;
    assert_eq!(subscribe["channel"], "agent:wake");
    let data = json!({"source":"subagents","reason":"worker finished"});
    host.send(json!({"jsonrpc":"2.0","method":"events.message","params":{"channel":"agent:wake","data":data}}))
        .await;
    let woke = text(&host.response(2).await);
    assert_eq!(woke, "Woke after 0s: subagents (worker finished)");

    host.send(execute(3, &ctx, "sleep", json!({"seconds":30})))
        .await;
    host.wait_request("events.subscribe").await;
    let input = json!({"name":"input","event":{"text":"stop","source":"interactive","streamingBehavior":"steer"},"ctx":ctx});
    host.send(json!({"jsonrpc":"2.0","id":4,"method":"event","params":input}))
        .await;
    let woke = text(&host.response(3).await);
    assert_eq!(woke, "Woke after 0s: user input");

    let short = host.tool(5, &ctx, "sleep", json!({"seconds":0.2})).await;
    assert!(short.contains("must be between 1 and 3600"), "{short}");
}

#[tokio::test]
async fn timers_persist_fire_and_follow_the_session() {
    let temp = Temp::new();
    let agent = temp.0.join("agent");
    let ctx = json!({"cwd":temp.0,"hasUI":true,"sessionId":"sess-1"});
    let mut host = Host::start();
    let init = json!({"protocolVersion":1,"cwd":temp.0,"agentDir":agent,"hasUI":true});
    host.call(0, "initialize", init).await;
    let start = json!({"name":"session_start","event":{"reason":"startup"},"ctx":ctx});
    host.call(1, "event", start).await;

    let started = host
        .tool(
            2,
            &ctx,
            "heartbeat",
            json!({"action":"start","every":"30m","reason":"watch the build"}),
        )
        .await;
    assert!(started.starts_with("Started heartbeat"), "{started}");
    assert!(agent.join("ext/timers/sessions/sess-1.json").exists());
    let created = host
        .tool(
            3,
            &ctx,
            "schedule",
            json!({"action":"create","in":"1s","prompt":"check the deploy","name":"deploy"}),
        )
        .await;
    assert!(created.starts_with("Created schedule"), "{created}");
    assert_eq!(last_status(&host), "timers: 1 heartbeat, 1 schedule");

    let message = host.wait_request("session.sendMessage").await;
    assert_eq!(message["customType"], "schedule");
    assert_eq!(message["deliverAs"], "followUp");
    assert_eq!(message["triggerTurn"], true);
    assert_eq!(message["display"], true);
    assert!(
        message["content"]
            .as_str()
            .unwrap()
            .contains("check the deploy")
    );
    let wake = host.wait_request("events.emit").await;
    assert_eq!(wake["channel"], "agent:wake");
    assert_eq!(wake["data"], json!({"source":"schedule","reason":"deploy"}));
    let listed = host
        .tool(4, &ctx, "schedule", json!({"action":"list"}))
        .await;
    assert_eq!(listed, "No active schedules.");

    let other = json!({"cwd":temp.0,"hasUI":true,"sessionId":"sess-2"});
    let switch = json!({"name":"session_start","event":{"reason":"new"},"ctx":other});
    host.call(5, "event", switch).await;
    let listed = host
        .tool(6, &other, "heartbeat", json!({"action":"list"}))
        .await;
    assert_eq!(listed, "No active heartbeats.");
    host.call(
        7,
        "command.run",
        json!({"name":"timers","args":"","ctx":other}),
    )
    .await;
    assert_eq!(last_status(&host), Value::Null);

    let back = json!({"name":"session_start","event":{"reason":"resume"},"ctx":ctx});
    host.call(8, "event", back).await;
    let listed = host
        .tool(9, &ctx, "heartbeat", json!({"action":"list"}))
        .await;
    assert!(listed.contains("watch the build"), "{listed}");
    let id = listed.split_whitespace().nth(1).unwrap().to_owned();
    let acked = host
        .tool(10, &ctx, "heartbeat", json!({"action":"ack","id":id}))
        .await;
    assert!(acked.starts_with("Stopped heartbeat"), "{acked}");
    assert!(!agent.join("ext/timers/sessions/sess-1.json").exists());
    let missing = host
        .tool(11, &ctx, "heartbeat", json!({"action":"ack","id":"zz"}))
        .await;
    assert!(missing.contains("active heartbeats: none"), "{missing}");

    let reply = host
        .call(
            12,
            "event",
            json!({"name":"before_agent_start","event":{},"ctx":ctx}),
        )
        .await;
    let section = &reply["patch"]["systemPromptOptions"]["sections"]["pi_timers_policy"];
    assert!(section.as_str().unwrap().contains("heartbeat"));
}
