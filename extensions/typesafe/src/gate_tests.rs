use super::*;

fn config(value: Value) -> GateConfig {
    GateConfig::parse(Some(&value))
}

fn names(gate: &Gate) -> Vec<(&str, &str, Source)> {
    let rules = gate.rules.iter();
    rules
        .map(|r| (r.name.as_str(), r.text.as_str(), r.source))
        .collect()
}

fn global() -> GateConfig {
    config(json!({
        "enabled": true, "blockAt": 0.8, "askAt": 0.5, "onError": "ask",
        "rules": { "no_prod": "Changes prod", "no_push": "Force-pushes" }
    }))
}

#[test]
fn defaults_and_invalid_keys() {
    let gate = Gate::default();
    assert!(!gate.enabled);
    assert_eq!(gate.tools, ["bash"]);
    assert_eq!(
        (gate.block_at, gate.ask_at),
        (DEFAULT_BLOCK_AT, DEFAULT_ASK_AT)
    );
    assert_eq!(gate.on_error, OnError::Allow);
    let parsed = config(json!({
        "enabled": "yes", "blockAt": 2, "askAt": 0.9, "onError": "maybe",
        "rules": { "": "x", "blank": " ", "ok": "Text", "n": 3 }
    }));
    assert_eq!(parsed.enabled, None);
    assert_eq!(parsed.block_at, None);
    assert_eq!(parsed.rules, [("ok".to_owned(), Some("Text".to_owned()))]);
    // askAt above blockAt leaves no ask band.
    assert_eq!(merge(&parsed, &GateConfig::default(), true).ask_at, 0.8);
}

#[test]
fn trusted_project_wins() {
    let project = config(json!({
        "enabled": false, "tools": ["edit"], "blockAt": 0.95, "onError": "allow",
        "rules": { "no_prod": "Touches production", "no_push": null, "no_rm": "Deletes files" }
    }));
    let gate = merge(&global(), &project, true);
    assert!(!gate.enabled);
    assert_eq!(gate.tools, ["edit"]);
    assert_eq!(gate.block_at, 0.95);
    assert_eq!(gate.on_error, OnError::Allow);
    assert_eq!(
        names(&gate),
        [
            ("no_prod", "Touches production", Source::Project),
            ("no_rm", "Deletes files", Source::Project)
        ]
    );
    assert!(gate.ignored.is_empty());
}

#[test]
fn untrusted_project_only_tightens() {
    let project = config(json!({
        "enabled": true, "tools": ["edit", "bash"], "blockAt": 0.95, "askAt": 0.3,
        "onError": "allow",
        "rules": { "no_prod": "Nothing", "no_push": null, "no_rm": "Deletes files" }
    }));
    let gate = merge(&global(), &project, false);
    assert!(gate.enabled);
    assert_eq!(gate.tools, ["bash", "edit"]);
    assert_eq!((gate.block_at, gate.ask_at), (0.8, 0.3));
    assert_eq!(gate.on_error, OnError::Ask);
    assert_eq!(
        names(&gate),
        [
            ("no_prod", "Changes prod", Source::Global),
            ("no_push", "Force-pushes", Source::Global),
            ("no_rm", "Deletes files", Source::Project)
        ]
    );
    assert_eq!(
        gate.ignored,
        [
            "blockAt",
            "onError",
            "enabled",
            "rules.no_prod",
            "rules.no_push"
        ]
    );
    let strict = config(json!({ "onError": "block" }));
    assert_eq!(merge(&global(), &strict, false).on_error, OnError::Block);
    let off = merge(
        &GateConfig::default(),
        &config(json!({ "enabled": true })),
        false,
    );
    assert!(!off.enabled, "an untrusted project cannot turn the gate on");
}

#[test]
fn reviews_listed_tools_with_rules() {
    let mut gate = merge(&global(), &GateConfig::default(), true);
    assert!(gate.reviews("bash") && !gate.reviews("read"));
    gate.tools = vec!["*".to_owned()];
    assert!(gate.reviews("read"));
    gate.rules.clear();
    assert!(!gate.reviews("bash"));
}

fn rules(n: usize) -> Vec<Rule> {
    (0..n)
        .map(|i| Rule {
            name: format!("r{i}"),
            text: format!("Rule {i}"),
            source: Source::Global,
        })
        .collect()
}

#[test]
fn requests_ask_one_noul_question_per_rule() {
    let input = json!({ "command": "git push -f", "timeout": 5 });
    let built = requests(&rules(2), "bash", &input, "/work", 32);
    assert_eq!(built.len(), 1);
    assert_eq!(
        built[0],
        json!({
            "state": { "tool": "bash", "input": "git push -f", "cwd": "/work" },
            "questions": {
                "r0": { "type": "noul", "instructions": "Does this tool call violate the rule \"Not allowed: Rule 0\"?" },
                "r1": { "type": "noul", "instructions": "Does this tool call violate the rule \"Not allowed: Rule 1\"?" }
            }
        })
    );
    crate::validate::prepare(built[0].clone(), None).unwrap();
    let other = requests(&rules(1), "write", &json!({ "path": "a" }), "/w", 32);
    assert_eq!(other[0]["state"]["input"], json!({ "path": "a" }));
}

#[test]
fn requests_split_at_the_question_limit() {
    let built = requests(&rules(40), "bash", &json!({ "command": "ls" }), "/w", 16);
    let sizes: Vec<usize> = built
        .iter()
        .map(|r| r["questions"].as_object().unwrap().len())
        .collect();
    assert_eq!(sizes, [16, 16, 8]);
    assert_eq!(requests(&rules(40), "bash", &json!({}), "/w", 99).len(), 2);
}

#[test]
fn large_inputs_are_cut_under_the_byte_limit() {
    let command = format!("echo {}", "\"é".repeat(40_000));
    let built = requests(&rules(3), "bash", &json!({ "command": command }), "/w", 32);
    let text = built[0].to_string();
    assert!(text.len() <= BYTE_BUDGET, "{}", text.len());
    let input = built[0]["state"]["input"].as_str().unwrap();
    assert!(input.starts_with("echo \"é") && input.contains("… [truncated "));
    crate::validate::prepare(built[0].clone(), None).unwrap();
    let big = json!({ "content": "x".repeat(70_000) });
    let other = requests(&rules(1), "write", &big, "/w", 32);
    assert!(other[0].to_string().len() <= BYTE_BUDGET);
    assert!(
        other[0]["state"]["input"]
            .as_str()
            .unwrap()
            .starts_with("{\"content\":\"xxx")
    );
}

fn review(ps: &[f64]) -> Review {
    let rules = rules(ps.len());
    let answers: Map<String, Value> = ps
        .iter()
        .enumerate()
        .map(|(i, p)| (format!("r{i}"), json!({ "type": "noul", "noul": p })))
        .collect();
    let result = json!({ "model": "typesafe/jev-1.14", "answers": answers });
    Review::from_results(&rules, &[result], "fallback").unwrap()
}

#[test]
fn verdict_thresholds() {
    let gate = merge(&global(), &GateConfig::default(), true);
    assert_eq!(review(&[0.1, 0.49]).verdict(&gate), Verdict::Allow);
    assert_eq!(review(&[0.5, 0.1]).verdict(&gate), Verdict::Ask);
    assert_eq!(review(&[0.1, 0.8]).verdict(&gate), Verdict::Block);
    let blocked = review(&[0.93, 0.85, 0.2]);
    assert_eq!(blocked.top(), 0.93);
    assert_eq!(
        blocked.block_reason(&gate),
        "TypeSafe gate blocked this call: rule `r0` (Rule 0) p=0.93; rule `r1` (Rule 1) p=0.85 (blockAt 0.8, typesafe/jev-1.14)."
    );
    let missing = Review::from_results(&rules(2), &[json!({ "answers": {} })], "m");
    assert_eq!(missing.unwrap_err().code, Code::Response);
}

#[test]
fn cache_is_bounded() {
    let state = GateState::default();
    for i in 0..CACHE_SIZE + 5 {
        state.remember(format!("k{i}"), review(&[0.1]));
    }
    assert!(state.cached("k0").is_none());
    assert!(state.cached(&format!("k{}", CACHE_SIZE + 4)).is_some());
    assert_eq!(state.lock().cache.reviews.len(), CACHE_SIZE);
}
