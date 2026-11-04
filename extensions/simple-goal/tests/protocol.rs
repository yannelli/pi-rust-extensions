mod common;

use common::{Host, TestDir};
use serde_json::{Value, json};

const OBJECTIVE: &str = "Make the parser accept trailing commas";

async fn started(label: &str) -> (Host, TestDir, TestDir) {
    let work = TestDir::new(&format!("{label}-work"));
    let agent = TestDir::new(&format!("{label}-agent"));
    let host = Host::start(work.path(), agent.path(), Vec::new()).await;
    (host, work, agent)
}

async fn set(host: &mut Host) {
    let args = json!({ "action": "set", "objective": OBJECTIVE, "done_when": "cargo test passes" });
    let (text, failed) = host.tool("simple_goal", args).await;
    assert!(!failed && text.starts_with("Simple goal set"), "{text}");
}

fn last_status(host: &Host) -> Value {
    host.called("ui.setStatus")
        .last()
        .map_or(Value::Null, |p| p["text"].clone())
}

#[tokio::test]
async fn registers_tool_and_command() {
    let (host, _work, _agent) = started("reg").await;
    let init = host.init_result();
    let tool = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "simple_goal")
        .expect("simple_goal tool");
    assert_eq!(tool["parameters"]["required"], json!(["action"]));
    let commands = init["commands"].as_array().unwrap();
    assert!(commands.iter().any(|c| c["name"] == "simple-goal"));
}

#[tokio::test]
async fn continue_verdict_sends_a_follow_up() {
    let (mut host, _work, _agent) = started("continue").await;
    set(&mut host).await;
    assert_eq!(last_status(&host), json!("simple goal 0/8"));
    host.verdict = Some("The parser still rejects `[1,]`.\n<continue/>".into());
    host.run(2, false).await;

    let request = host.wait_for("llm.complete").await;
    let prompt = request["prompt"].as_str().unwrap();
    assert!(prompt.contains(OBJECTIVE) && prompt.contains("cargo test passes"));
    assert!(
        request["systemPrompt"]
            .as_str()
            .unwrap()
            .contains("<done/>")
    );
    assert!(request.get("model").is_none());

    let message = host.wait_for("session.sendMessage").await;
    assert_eq!(message["customType"], "simple-goal");
    assert_eq!(message["display"], true);
    assert_eq!(message["deliverAs"], "followUp");
    assert_eq!(message["triggerTurn"], true);
    let text = message["content"].as_str().unwrap();
    assert!(
        text.contains("1/8") && text.contains("rejects `[1,]`"),
        "{text}"
    );
    assert!(text.contains("action `clear`"), "{text}");
    assert_eq!(last_status(&host), json!("simple goal 1/8"));
}

#[tokio::test]
async fn done_verdict_clears_the_goal() {
    let (mut host, _work, _agent) = started("done").await;
    set(&mut host).await;
    host.run(1, false).await;
    let message = host.wait_for("session.sendMessage").await;
    assert_eq!(message["customType"], "simple-goal");
    assert!(message.get("triggerTurn").is_none());
    assert!(
        message["content"]
            .as_str()
            .unwrap()
            .contains("Looks finished.")
    );
    let (status, _) = host
        .tool("simple_goal", json!({ "action": "status" }))
        .await;
    assert_eq!(status, "No simple goal.");
    assert_eq!(last_status(&host), Value::Null);
}

#[tokio::test]
async fn aborted_run_pauses_and_resume_restarts() {
    let (mut host, _work, _agent) = started("abort").await;
    set(&mut host).await;
    host.run(1, true).await;
    assert!(host.called("llm.complete").is_empty());
    let (status, _) = host
        .tool("simple_goal", json!({ "action": "status" }))
        .await;
    assert!(status.contains("paused (the run was aborted)"), "{status}");

    host.command("simple-goal", "resume").await;
    let message = host
        .called("session.sendMessage")
        .last()
        .cloned()
        .cloned()
        .unwrap();
    assert_eq!(message["triggerTurn"], true);
    let (status, _) = host
        .tool("simple_goal", json!({ "action": "status" }))
        .await;
    assert!(status.starts_with("Simple goal active"), "{status}");
}

#[tokio::test]
async fn evaluator_error_pauses() {
    let (mut host, _work, _agent) = started("error").await;
    set(&mut host).await;
    host.verdict = None;
    host.run(1, false).await;
    let notice = host.wait_for("ui.notify").await;
    assert!(
        notice["message"]
            .as_str()
            .unwrap()
            .contains("evaluator failed"),
        "{notice}"
    );
    let (status, _) = host
        .tool("simple_goal", json!({ "action": "status" }))
        .await;
    assert!(status.contains("paused"), "{status}");
}

#[tokio::test]
async fn idle_runs_pause_after_two_continue_verdicts() {
    let (mut host, _work, _agent) = started("idle").await;
    set(&mut host).await;
    host.verdict = Some("Nothing changed.\n<continue/>".into());
    host.run(0, false).await;
    host.wait_for("session.sendMessage").await;
    host.run(0, false).await;
    let notice = host.wait_for("ui.notify").await;
    assert!(
        notice["message"]
            .as_str()
            .unwrap()
            .contains("no tool calls"),
        "{notice}"
    );
    assert_eq!(host.called("session.sendMessage").len(), 1);
}

#[tokio::test]
async fn command_sets_and_state_survives_a_restart() {
    let (mut host, work, agent) = started("restore").await;
    host.command("simple-goal", OBJECTIVE).await;
    let message = host.called("session.sendMessage")[0].clone();
    assert_eq!(message["triggerTurn"], true);
    assert!(message["content"].as_str().unwrap().contains(OBJECTIVE));

    let mut restarted = Host::start(work.path(), agent.path(), host.branch.clone()).await;
    let (status, _) = restarted
        .tool("simple_goal", json!({ "action": "status" }))
        .await;
    assert!(status.contains(OBJECTIVE), "{status}");
    assert_eq!(last_status(&restarted), json!("simple goal 0/8"));
    let (cleared, _) = restarted
        .tool("simple_goal", json!({ "action": "clear" }))
        .await;
    assert_eq!(cleared, "Simple goal cleared.");
}

#[tokio::test]
async fn untrusted_project_ignores_model_keys_once() {
    let work = TestDir::new("trust-work");
    let agent = TestDir::new("trust-agent");
    std::fs::create_dir_all(work.path().join(".pi/ext")).unwrap();
    let config = json!({ "model": "evil/model", "maxRounds": 3 });
    std::fs::write(
        work.path().join(".pi/ext/simple-goal.json"),
        config.to_string(),
    )
    .unwrap();
    let mut host = Host::start(work.path(), agent.path(), Vec::new()).await;
    host.ctx["isProjectTrusted"] = json!(false);
    host.event(
        "session_start",
        json!({ "type": "session_start", "reason": "reload" }),
    )
    .await;
    set(&mut host).await;
    host.verdict = Some("Not yet.\n<continue/>".into());
    host.run(1, false).await;
    let request = host.wait_for("llm.complete").await;
    assert!(request.get("model").is_none(), "{request}");
    host.wait_for("session.sendMessage").await;
    let warnings: Vec<_> = host
        .called("ui.notify")
        .into_iter()
        .filter(|n| n["message"].as_str().unwrap_or("").contains("not trusted"))
        .collect();
    assert_eq!(warnings.len(), 1);
    assert_eq!(last_status(&host), json!("simple goal 1/3"));
}

#[tokio::test]
async fn an_enabled_typesafe_judges_instead_of_the_llm() {
    let (mut host, _work, _agent) = started("typesafe").await;
    host.typesafe = Some(0.9);
    set(&mut host).await;
    host.run(1, false).await;
    let emitted = host.wait_for("events.emit").await;
    assert_eq!(emitted["channel"], "typesafe:decision:request");
    let request = &emitted["data"]["request"];
    assert_eq!(request["state"]["objective"], OBJECTIVE);
    assert_eq!(request["questions"]["done"]["type"], "noul");
    let message = host.wait_for("session.sendMessage").await;
    let text = message["content"].as_str().unwrap();
    assert!(text.starts_with("Simple goal done"), "{text}");
    assert!(text.contains("probability 0.90"), "{text}");
    assert!(host.called("llm.complete").is_empty());
}
