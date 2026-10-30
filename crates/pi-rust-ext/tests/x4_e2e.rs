use pi_ext_e2e::{Run, Turn, json, pi_bin_or_skip};

#[cfg(unix)]
mod support;

fn run(extension: &str, tool: &str, params: pi_ext_e2e::Value) -> pi_ext_e2e::Output {
    Run::new(env!("CARGO_BIN_EXE_pi-rust-ext"))
        .args([extension])
        .turns([
            Turn::tool_call(tool, params),
            Turn::text("The extension returned its result."),
        ])
        .prompt("Run the requested extension tool once.")
        .run()
        .expect("pi completes against the local mock provider")
}

#[test]
#[cfg(unix)]
fn native_web_search_rejects_an_unsupported_model() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let (_config, wrapper) = support::configured_search(json!({"defaultProvider":"native"}));
    let output = Run::new(wrapper)
        .args(["web-search"])
        .turns([
            Turn::tool_call("web_search", json!({"query":"local test"})),
            Turn::text("The extension returned its result."),
        ])
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert!(output.tool_names(0).contains(&"web_search".to_string()));
    let results = output.tool_results("web_search");
    assert_eq!(results.len(), 1, "{}", output.stdout);
    assert_eq!(results[0]["isError"], true, "{}", output.stdout);
    assert_eq!(
        results[0]["result"]["details"]["error"], "unsupported_model",
        "{}",
        output.stdout
    );
    assert_eq!(output.requests.len(), 2);
}

#[test]
#[cfg(unix)]
fn exa_search_returns_local_provider_results_through_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let api = support::LocalApi::start(json!({"results":[{
        "title":"Local search result", "url":"https://example.test/result",
        "highlights":["Provider returned a local fixture."]
    }]}));
    let (_config, wrapper) = support::configured_search(json!({
        "defaultProvider":"exa", "providers":{"exa":{"baseUrl":api.url}}
    }));
    let output = Run::new(wrapper)
        .args(["web-search"])
        .env("EXA_API_KEY", "local-test-key")
        .turns([
            Turn::tool_call("web_search", json!({"query":"fixture query"})),
            Turn::text("The search returned the local fixture."),
        ])
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let text = output
        .tool_result_text("web_search")
        .expect("search result");
    assert!(
        text.contains("Provider returned a local fixture."),
        "{text}"
    );
    assert!(text.contains("https://example.test/result"), "{text}");
    assert_eq!(api.requests.lock().unwrap()[0]["query"], "fixture query");
    assert_eq!(output.requests.len(), 2);
}

#[test]
fn desktop_cached_query_rejects_an_unknown_state() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = run(
        "computer-use",
        "search_ui",
        json!({"stateId":"missing-state", "text":"example"}),
    );
    assert!(output.status.success(), "{}", output.stderr);
    assert!(output.tool_names(0).contains(&"search_ui".to_string()));
    let results = output.tool_results("search_ui");
    assert_eq!(results.len(), 1, "{}", output.stdout);
    assert_eq!(results[0]["isError"], true, "{}", output.stdout);
    let text = output.tool_result_text("search_ui").unwrap();
    assert!(text.to_lowercase().contains("state"), "{text}");
    assert_eq!(output.requests.len(), 2);
}

#[test]
fn chrome_tools_stay_inactive_without_session_authorization() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(env!("CARGO_BIN_EXE_pi-rust-ext"))
        .args(["chrome"])
        .turns([Turn::text("Chrome requires session authorization.")])
        .prompt("Describe the available tools.")
        .run()
        .expect("pi completes against the local mock provider");
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(output.requests.len(), 1);
    assert!(
        output
            .tool_names(0)
            .iter()
            .all(|name| !name.starts_with("chrome_")),
        "{}",
        output.stdout
    );
    assert!(
        !output.stderr.contains("failed to load"),
        "{}",
        output.stderr
    );
}
