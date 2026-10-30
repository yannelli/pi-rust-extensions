use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};
use pi_ext_typesafe::mock_api::{MockApi, Reply};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typesafe_evaluates_through_pi_with_a_local_api() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let answer = json!({
        "model":"jev-1.13", "usage":{"input_tokens":100,"output_tokens":1},
        "answers":{"urgent":{"type":"noul","noul":0.75}}
    });
    let api = MockApi::start(vec![Reply::json(200, &answer)]).await;
    let url = api.url.clone();
    let output = tokio::task::spawn_blocking(move || {
        Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
            .args(["typesafe"])
            .env("PI_TYPESAFE_ENABLED", "1")
            .env("TYPESAFE_API_KEY", "ts_test_0123456789abcdef")
            .env("PI_TYPESAFE_BASE_URL", url)
            .turns([
                Turn::tool_call("typesafe_evaluate", json!({
                    "state":"Please respond today.",
                    "questions":{"urgent":{"type":"noul","instructions":"Does the sender request help today?"}}
                })),
                Turn::text("done"),
            ])
            .timeout(std::time::Duration::from_secs(30))
            .prompt("Evaluate whether the supplied request is urgent.")
            .run()
            .unwrap()
    }).await.unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let text = output.tool_result_text("typesafe_evaluate").unwrap();
    let result: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{error}: {text}\n{}", output.stderr));
    assert_eq!(result["answers"]["urgent"]["noul"], 0.75);
    assert!(
        output
            .tool_names(0)
            .iter()
            .any(|name| name == "typesafe_evaluate")
    );
    assert_eq!(output.final_text(), "done");
    let requests = api.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/systemone");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer ts_test_0123456789abcdef")
    );
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["model"], "jev-latest");
}
