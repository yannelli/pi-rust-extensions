use std::path::PathBuf;

use serde_json::{Value, json};

use crate::config::{self, Config, DEFAULT_MAX_ROUNDS, Judge, from_map};
use crate::evaluator::{
    Evidence, RECENT_MESSAGES, Verdict, build_prompt, parse_verdict, recent_messages,
    typesafe_request, typesafe_verdict,
};
use crate::settle::{Decision, Settle, apply, continuation_text, decide, on_settle};
use crate::state::{Goal, Status};

fn goal() -> Goal {
    Goal::new(
        "Fix the failing test".into(),
        Some("cargo test passes".into()),
    )
}

fn verdict(done: bool) -> Result<Verdict, String> {
    Ok(Verdict {
        done,
        reason: "reason".into(),
    })
}

#[test]
fn verdict_marker_must_end_the_output() {
    let done = parse_verdict("All tests pass.\n<done/>\n\n");
    assert_eq!(
        done,
        Verdict {
            done: true,
            reason: "All tests pass.".into()
        }
    );
    let more = parse_verdict("One test still fails. <continue/>");
    assert!(!more.done);
    assert_eq!(more.reason, "One test still fails.");
    let quoted = parse_verdict("<done/>\nBut the build is broken.");
    assert!(!quoted.done);
    assert_eq!(quoted.reason, "<done/>\nBut the build is broken.");
    assert_eq!(parse_verdict("<done/>").reason, "no reason given");
    let long = parse_verdict(&format!("{}\n<continue/>", "x".repeat(700)));
    assert_eq!(long.reason.chars().count(), 601);
}

struct Dirs(PathBuf);

impl Dirs {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("pi-simple-goal-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("agent/ext")).unwrap();
        std::fs::create_dir_all(path.join("project/ext")).unwrap();
        Self(path)
    }

    fn write(&self, which: &str, value: serde_json::Value) {
        let path = self.0.join(which).join("ext/simple-goal.json");
        std::fs::write(path, value.to_string()).unwrap();
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn project_config_wins_and_model_keys_need_trust() {
    let dirs = Dirs::new("config");
    dirs.write(
        "agent",
        json!({ "model": "a/global", "thinking": "low", "maxRounds": 3, "instructions": "global" }),
    );
    dirs.write(
        "project",
        json!({ "model": "b/project", "systemPrompt": "custom", "maxRounds": 5, "instructions": "project" }),
    );
    let (agent, project) = (dirs.0.join("agent"), dirs.0.join("project"));

    let trusted = config::load(&agent, &project, true);
    assert!(trusted.ignored.is_empty());
    assert_eq!(
        trusted.config,
        Config {
            model: Some("b/project".into()),
            thinking: Some("low".into()),
            system_prompt: Some("custom".into()),
            instructions: Some("project".into()),
            max_rounds: 5,
            ..Config::default()
        }
    );

    let untrusted = config::load(&agent, &project, false);
    assert_eq!(untrusted.ignored, ["model", "systemPrompt"]);
    assert_eq!(untrusted.config.model.as_deref(), Some("a/global"));
    assert_eq!(untrusted.config.system_prompt, None);
    assert_eq!(untrusted.config.instructions.as_deref(), Some("project"));
    assert_eq!(untrusted.config.max_rounds, 5);
}

#[test]
fn config_defaults_without_files() {
    let dirs = Dirs::new("empty");
    let loaded = config::load(&dirs.0.join("agent"), &dirs.0.join("project"), false);
    assert_eq!(loaded.config, Config::default());
    assert_eq!(loaded.config.max_rounds, DEFAULT_MAX_ROUNDS);
    let zero = config::from_map(json!({ "maxRounds": 0 }).as_object().unwrap());
    assert_eq!(zero.max_rounds, 1);
}

#[test]
fn settle_evaluates_only_an_active_goal() {
    let active = goal();
    let mut paused = goal();
    paused.pause("x");
    assert_eq!(on_settle(None, false, false), Settle::Nothing);
    assert_eq!(on_settle(Some(&paused), false, false), Settle::Nothing);
    assert_eq!(on_settle(Some(&paused), true, false), Settle::Nothing);
    assert_eq!(on_settle(Some(&active), true, false), Settle::PauseAborted);
    assert_eq!(on_settle(Some(&active), false, true), Settle::Nothing);
    assert_eq!(on_settle(Some(&active), false, false), Settle::Evaluate);
}

#[test]
fn verdict_decides_done_continue_or_pause() {
    let goal = goal();
    assert_eq!(decide(&goal, &verdict(true), 0, 8), Decision::Done);
    assert_eq!(decide(&goal, &verdict(false), 3, 8), Decision::Continue);
    assert_eq!(
        decide(&goal, &Err("boom".into()), 3, 8),
        Decision::PauseError("boom".into())
    );
    let mut last = goal.clone();
    last.rounds = 8;
    assert_eq!(
        decide(&last, &verdict(false), 3, 8),
        Decision::PauseMaxRounds
    );
    assert_eq!(decide(&last, &verdict(true), 3, 8), Decision::Done);

    let first = verdict(false);
    let decision = decide(&goal, &first, 0, 8);
    assert_eq!(decision, Decision::Continue);
    let after = apply(goal.clone(), &decision, &first, 0).unwrap();
    assert_eq!((after.rounds, after.idle_streak), (1, 1));
    assert_eq!(after.last_reason.as_deref(), Some("reason"));
    assert_eq!(decide(&after, &first, 0, 8), Decision::PauseIdle);
    assert_eq!(decide(&after, &first, 2, 8), Decision::Continue);
    let reset = apply(after, &Decision::Continue, &first, 2).unwrap();
    assert_eq!((reset.rounds, reset.idle_streak), (2, 0));
}

#[test]
fn apply_finishes_or_pauses() {
    assert_eq!(apply(goal(), &Decision::Done, &verdict(true), 1), None);
    for decision in [
        Decision::PauseIdle,
        Decision::PauseMaxRounds,
        Decision::PauseError("x".into()),
    ] {
        let paused = apply(goal(), &decision, &verdict(false), 0).unwrap();
        assert_eq!(paused.status, Status::Paused);
        assert!(paused.paused_reason.is_some());
    }
}

#[test]
fn prompt_holds_objective_evidence_and_instructions() {
    let config = Config {
        instructions: Some("Run nothing.".into()),
        ..Config::default()
    };
    let evidence = Evidence {
        messages: vec!["first".into(), "Tests pass now.".into()],
        git_status: " M a.rs".into(),
        git_diff: "+fixed".into(),
    };
    let prompt = build_prompt(&goal(), &config, &evidence);
    for part in [
        "<objective>\nFix the failing test\n</objective>",
        "<done_when>\ncargo test passes\n</done_when>",
        "<last_agent_messages>\nfirst\n\n---\n\nTests pass now.\n</last_agent_messages>",
        "<git_status>\nM a.rs\n</git_status>",
        "<git_diff>\n+fixed\n</git_diff>",
        "Run nothing.",
        "<done/>",
    ] {
        assert!(prompt.contains(part), "{part}: {prompt}");
    }
    let bare = build_prompt(
        &Goal::new("x".into(), Some("  ".into())),
        &Config::default(),
        &Evidence::default(),
    );
    assert!(!bare.contains("<done_when>") && !bare.contains("<git_diff>"));
    assert!(bare.contains("<last_agent_messages>\n(none)"));
}

#[test]
fn continuation_names_round_reason_and_clear() {
    let mut goal = goal();
    goal.rounds = 2;
    let text = continuation_text(&goal, "a test fails", 8);
    for part in [
        "2/8",
        "Fix the failing test",
        "cargo test passes",
        "a test fails",
        "`simple_goal` with action `clear`",
    ] {
        assert!(text.contains(part), "{part}: {text}");
    }
}

#[test]
fn recent_messages_keep_the_last_assistant_texts() {
    let assistant = |text: &str| {
        json!({ "type": "message", "message": { "role": "assistant", "content": [
            { "type": "thinking", "thinking": "hidden" }, { "type": "text", "text": text }
        ] } })
    };
    let entries = vec![
        assistant("one"),
        json!({ "type": "message", "message": { "role": "user", "content": "do it" } }),
        assistant("two"),
        json!({ "type": "custom" }),
        assistant("three"),
        assistant(&"y".repeat(2_500)),
    ];
    let messages = recent_messages(&entries);
    assert_eq!(messages.len(), RECENT_MESSAGES);
    assert_eq!(messages[..2], ["two", "three"]);
    assert!(
        messages[2].ends_with("… (500 more characters)"),
        "{}",
        messages[2]
    );
}

#[test]
fn typesafe_request_and_verdict() {
    let evidence = Evidence {
        messages: vec!["done".into()],
        git_status: String::new(),
        git_diff: "+x".into(),
    };
    let request = typesafe_request(&goal(), &evidence);
    assert_eq!(request["state"]["done_when"], "cargo test passes");
    assert_eq!(request["state"]["git_diff"], "+x");
    assert_eq!(request["questions"]["done"]["type"], "noul");
    let result = |p: f64| json!({ "model": "typesafe/jev-1.14", "answers": { "done": { "type": "noul", "noul": p } } });
    let yes = typesafe_verdict(&result(0.82), 0.7).unwrap();
    assert!(yes.done);
    assert_eq!(
        yes.reason,
        "typesafe/jev-1.14 judged the objective done with probability 0.82 (threshold 0.70)."
    );
    assert!(!typesafe_verdict(&result(0.4), 0.7).unwrap().done);
    assert!(typesafe_verdict(&json!({ "answers": {} }), 0.7).is_err());
}

#[test]
fn judge_and_threshold_config() {
    let map = |value: Value| value.as_object().cloned().unwrap();
    let config = from_map(&map(
        json!({ "judge": "typesafe", "typesafeThreshold": 0.9 }),
    ));
    assert_eq!(config.judge, Judge::Typesafe);
    assert_eq!(config.typesafe_threshold, 0.9);
    let config = from_map(&map(json!({ "judge": "llm", "typesafeThreshold": 2 })));
    assert_eq!(config.judge, Judge::Llm);
    assert_eq!(config.typesafe_threshold, 0.7);
    assert_eq!(from_map(&map(json!({}))).judge, Judge::Auto);
}
