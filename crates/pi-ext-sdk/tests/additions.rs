//! Protocol additions of SDK 1.1: raw panel keys, `footer.changed`, `model.find`, `ui.input`
//! options, `exec` stdin, `llm.complete` headers, and the `TestHost` helpers.

use std::sync::{Arc, Mutex};

use pi_ext_sdk::test_util::{TestHost, ctx};
use pi_ext_sdk::{ExecRequest, InputOptions, LlmRequest, prelude::*};

#[tokio::test]
async fn panel_keys_keep_raw_data_and_key_id() {
    let ext = Extension::new("keys").command("p", "", |_, ctx: CommandCtx| async move {
        let mut panel = ctx.host().panel(["keys"]).open().await?;
        while let PanelEvent::Key(key) = panel.next_event().await {
            let input = panel.last_input().cloned().expect("key input");
            assert_eq!(input.key, key);
            panel.set_lines([format!("{:?} {:?} {:?}", input.key, input.data, input.id)])?;
        }
        Ok(())
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "p", "args": "", "ctx": ctx() }),
    )
    .await;
    let (custom_id, params) = host.expect_request("ui.custom").await;
    let surface = params["surface"].clone();
    host.notify(
        "ui.key",
        json!({ "surface": surface, "data": "\r", "key": "enter" }),
    )
    .await;
    let update = host.expect_notification("ui.update").await;
    assert_eq!(update["lines"], json!(["Enter \"\\r\" Some(\"enter\")"]));
    host.notify("ui.key", json!({ "surface": surface, "data": "x" }))
        .await;
    let update = host.expect_notification("ui.update").await;
    assert_eq!(update["lines"], json!(["Char('x') \"x\" None"]));
    host.answer(&custom_id, json!({ "result": null })).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn footer_changes_reach_on_footer_change() {
    let ext = Extension::new("footer").on_footer_change(|footer, host| async move {
        let branch = footer["gitBranch"].as_str().unwrap_or_default().to_owned();
        host.notify_info(format!("branch {branch}")).await
    });
    let mut host = TestHost::start(ext);
    let result = host.initialize().await;
    assert_eq!(result["events"], json!(["footer_changed"]));
    host.notify(
        "footer.changed",
        json!({ "footer": { "gitBranch": "main" } }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["message"], "branch main");
    host.answer(&id, Value::Null).await;
    host.finish().await.unwrap();
}

fn requests_extension() -> Extension {
    Extension::new("requests").command("go", "", |_, ctx: CommandCtx| async move {
        let host = ctx.host();
        let model = host.find_model("local", "tiny").await?.expect("model");
        let missing = host.find_model("local", "none").await?;
        let options = InputOptions {
            prefill: Some("ab".into()),
            secret: true,
            ..InputOptions::default()
        };
        let token = host.input_with("Token", options).await?;
        let exec = host
            .exec_with(ExecRequest::new("cat", &[] as &[&str]).stdin("piped"))
            .await?;
        let llm = host
            .complete(LlmRequest::prompt("hi").header("x-trace", "7"))
            .await?;
        let init = host.init().expect("init");
        let summary = [
            format!("{} {:?} {}", model.id, model.base_url, missing.is_none()),
            format!("{token:?} {} {}", exec.stdout, llm.text),
            format!("{} {}", init.cwd, host.agent_dir().display()),
        ];
        host.notify_info(summary.join(" | ")).await
    })
}

#[tokio::test]
async fn drive_answers_host_requests_by_method() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut host = TestHost::start(requests_extension())
        .cwd("/repo")
        .agent_dir("/agent");
    host.initialize().await;
    let record = seen.clone();
    let params = json!({ "name": "go", "args": "", "ctx": { "cwd": "/repo", "hasUI": true } });
    let response = host
        .drive(4, "command.run", params, move |method, params| {
            record.lock().unwrap().push((method.to_owned(), params.clone()));
            match method {
                "model.find" if params["id"] == "tiny" => {
                    json!({ "provider": "local", "id": "tiny", "baseUrl": "http://m", "compat": null })
                }
                "ui.input" => json!(format!("{}c", params["prefill"].as_str().unwrap_or_default())),
                "exec" => json!({ "stdout": params["stdin"], "stderr": "", "code": 0, "killed": false }),
                "llm.complete" => json!({ "message": {}, "text": "answer" }),
                _ => Value::Null,
            }
        })
        .await;
    assert_eq!(response["result"], Value::Null, "{response}");
    let seen = seen.lock().unwrap().clone();
    let params = |name: &str| {
        seen.iter()
            .find(|(method, _)| method == name)
            .map(|(_, p)| p.clone())
    };
    assert_eq!(
        params("ui.input"),
        Some(json!({ "title": "Token", "prefill": "ab", "secret": true }))
    );
    assert_eq!(params("exec").unwrap()["stdin"], "piped");
    assert_eq!(
        params("llm.complete").unwrap()["headers"],
        json!({ "x-trace": "7" })
    );
    let notify = params("ui.notify").unwrap();
    assert_eq!(
        notify["message"],
        "tiny Some(\"http://m\") true | Some(\"abc\") piped answer | /repo /agent"
    );
    host.finish().await.unwrap();
}
