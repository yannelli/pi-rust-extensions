mod common;

use common::TempDir;
use pi_ext_web_search::config::{Api, Choice, Legacy, load, read_legacy, update_file};
use pi_ext_web_search::keys::{KeySource, api_key, mask, set_key};
use serde_json::json;

#[test]
fn trusted_project_overrides_per_key() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    agent.write(
        "ext/web-search.json",
        &json!({
            "defaultProvider": "exa", "tools": { "url_context": "tavily" },
            "providers": { "exa": { "baseUrl": "https://global.test/", "maxResults": 50 } }
        }),
    );
    cwd.write(
        ".pi/ext/web-search.json",
        &json!({ "defaultProvider": "brave", "providers": { "exa": { "maxResults": 3 } } }),
    );
    let trusted = load(&agent.0, &cwd.0, true);
    assert_eq!(
        trusted.config.choice("web_search"),
        Ok(Choice::Api(Api::Brave))
    );
    assert_eq!(
        trusted.config.choice("url_context"),
        Ok(Choice::Api(Api::Tavily))
    );
    assert_eq!(trusted.config.base_url(Api::Exa), "https://global.test");
    assert_eq!(trusted.config.max_results(Api::Exa), 3);
    assert!(!trusted.project_ignored);

    let untrusted = load(&agent.0, &cwd.0, false);
    assert_eq!(
        untrusted.config.choice("web_search"),
        Ok(Choice::Api(Api::Exa))
    );
    assert_eq!(untrusted.config.max_results(Api::Exa), 20);
    assert!(untrusted.project_ignored);
    assert_eq!(
        untrusted.config.base_url(Api::Tavily),
        "https://api.tavily.com"
    );
}

#[test]
fn invalid_files_and_unknown_providers_are_reported() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    std::fs::create_dir_all(agent.0.join("ext")).unwrap();
    std::fs::write(agent.0.join("ext/web-search.json"), "{ nope").unwrap();
    let loaded = load(&agent.0, &cwd.0, true);
    assert!(loaded.invalid.unwrap().1.starts_with("Invalid JSON"));

    agent.write("ext/web-search.json", &json!({ "defaultProvider": "bing" }));
    let loaded = load(&agent.0, &cwd.0, true);
    assert_eq!(
        loaded.config.choice("web_search"),
        Err("Unknown web search provider \"bing\"".to_owned())
    );
}

#[test]
fn legacy_file_shapes() {
    let dir = TempDir::new();
    let path = dir.write(
        "web-search.json",
        &json!({ "provider": "openai", "modelId": " gpt-5.1 " }),
    );
    assert_eq!(
        read_legacy(path.clone()),
        Legacy::Model {
            path,
            provider: "openai".into(),
            model: "gpt-5.1".into()
        }
    );
    let path = dir.write("bad.json", &json!({ "provider": "openai" }));
    assert_eq!(
        read_legacy(path.clone()),
        Legacy::Invalid {
            path,
            error: "Missing required string field: model".into()
        }
    );
    let path = dir.write("array.json", &json!([1]));
    let Legacy::Invalid { error, .. } = read_legacy(path) else {
        panic!("expected invalid")
    };
    assert_eq!(
        error,
        "Expected a JSON object with provider and model fields"
    );
    assert_eq!(read_legacy(dir.0.join("missing.json")), Legacy::Missing);
}

#[test]
fn update_file_merges_and_removes_keys() {
    let dir = TempDir::new();
    let path = dir.0.join("ext/web-search.json");
    update_file(
        &path,
        &json!({ "defaultProvider": "exa", "tools": { "web_search": "brave" } }),
    )
    .unwrap();
    update_file(
        &path,
        &json!({ "tools": { "web_search": null }, "searchModel": "openai/gpt" }),
    )
    .unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        value,
        json!({ "defaultProvider": "exa", "tools": {}, "searchModel": "openai/gpt" })
    );
}

#[test]
fn keys_store_and_remove() {
    let agent = TempDir::new();
    set_key(&agent.0, Api::Firecrawl, Some(" fc-key ")).unwrap();
    assert_eq!(
        api_key(&agent.0, Api::Firecrawl),
        Some(("fc-key".to_owned(), KeySource::Stored))
    );
    set_key(&agent.0, Api::Firecrawl, None).unwrap();
    let stored = std::fs::read_to_string(agent.0.join("ext/web-search/keys.json")).unwrap();
    assert_eq!(stored.trim(), "{}");
    assert_eq!(mask("abcdefghijkl"), "abcd...ijkl");
    assert_eq!(mask("short"), "*****");
}
