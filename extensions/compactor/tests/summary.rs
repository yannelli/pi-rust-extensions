mod common;

use std::collections::HashSet;

use pi_ext_compactor::config::Config;
use pi_ext_compactor::cut::{build_own_cut, collect_live};
use pi_ext_compactor::plan::{PlanInput, format_tokens, plan_lossless};
use pi_ext_compactor::source::{
    COMPACTOR_INSTRUCTION, collect_summary_source, parse_compaction_instructions,
    parse_keep_and_prompt,
};
use pi_ext_compactor::summary::commits::{commit_lines, select_commits};
use pi_ext_compactor::summary::redact_secrets;
use pi_ext_compactor::summary::requests::{
    select_decisions, select_request_lines, split_sentences,
};
use pi_ext_compactor::text::{Block, Kind};
use serde_json::json;

fn plan_input(smart: bool) -> PlanInput {
    let config = Config {
        smart_keep_tail: smart,
        ..Config::default()
    };
    PlanInput {
        branch: common::branch(),
        config,
        cwd: Some("/work".into()),
        ..PlanInput::default()
    }
}

#[test]
fn plan_keeps_last_turn_and_builds_sections() {
    let plan = plan_lossless(&plan_input(false)).unwrap();
    assert_eq!(plan.first_kept_entry_id, "e11");
    assert_eq!(plan.stats.summarized, 10);
    assert_eq!(
        (plan.stats.kept_user_turns, plan.stats.total_user_turns),
        (1, 3)
    );
    let text = &plan.summary;
    assert!(
        text.contains("[Your Requests]\n- Build a CLI tool that parses CSV files."),
        "{text}"
    );
    assert!(text.contains("[Files]\n- Modified: src/main.rs"), "{text}");
    assert!(
        text.contains("[Commits]\n- abc1234 feat: add parser"),
        "{text}"
    );
    assert!(
        text.contains("[Open Errors]\n- [bash] error[E0425]"),
        "{text}"
    );
    assert!(
        text.contains("[Lessons]\n- The root cause is that parse was never defined"),
        "{text}"
    );
    assert!(text.contains("[Recent Transcript]"), "{text}");
    assert!(text.ends_with("Do not redo work that is already done."));
    let sections = plan.details["sections"].as_array().unwrap();
    assert!(
        sections.contains(&json!("Your Requests"))
            && sections.contains(&json!("Recent Transcript"))
    );
    assert_eq!(plan.details["compactor"], "@pi-unipi/compactor");
}

#[test]
fn smart_keep_grows_a_small_tail() {
    let plan = plan_lossless(&plan_input(true)).unwrap();
    assert_eq!(plan.first_kept_entry_id, "e8");
    assert!(plan.stats.smart_keep_adjusted);
}

#[test]
fn explicit_keep_zero_compacts_all() {
    let input = PlanInput {
        keep: Some(0),
        keep_explicit: true,
        ..plan_input(true)
    };
    let plan = plan_lossless(&input).unwrap();
    assert_eq!(plan.first_kept_entry_id, "");
    assert_eq!(plan.stats.summarized, 12);
}

#[test]
fn too_few_messages_cancel() {
    let branch = common::branch()[..2].to_vec();
    assert_eq!(
        build_own_cut(&branch, 1).unwrap_err(),
        "too_few_live_messages"
    );
    assert_eq!(build_own_cut(&[], 1).unwrap_err(), "no_live_messages");
}

#[test]
fn live_window_starts_at_kept_entry_after_compaction() {
    let mut branch = common::branch();
    branch.push(
        json!({ "type": "compaction", "id": "c1", "summary": "s", "firstKeptEntryId": "e8" }),
    );
    let live = collect_live(&branch);
    assert_eq!(live.first().unwrap().id, "e8");
    branch
        .push(json!({ "type": "compaction", "id": "c2", "summary": "s", "firstKeptEntryId": "" }));
    assert!(collect_live(&branch).is_empty());
}

#[test]
fn summary_redacts_secrets_and_source_keeps_them() {
    let branch = common::branch();
    let plan = plan_lossless(&plan_input(false)).unwrap();
    assert!(!plan.summary.contains("sk-abcdefghijklmnop1234"));
    assert!(
        plan.summary.contains("api_key = [redacted]"),
        "{}",
        plan.summary
    );
    let source = collect_summary_source(&branch, branch.len());
    assert!(source.requests[0].contains("sk-abcdefghijklmnop1234"));
}

#[test]
fn redaction_patterns() {
    let cases = [
        ("password: hunter22", "password: [redacted]"),
        ("use token 'abcd efgh'", "use token [redacted]"),
        ("secret=\"s3cr3t\" ok", "secret=[redacted] ok"),
        (
            "Authorization: Bearer abcdefghijklmnop",
            "Authorization: Bearer [redacted]",
        ),
        ("key ghp_abcdefghijklmnopqrstuvwxyz", "key [redacted]"),
        ("aws AKIAABCDEFGHIJKLMNOP done", "aws [redacted] done"),
        (
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----",
            "[redacted private key]",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(redact_secrets(input), expected, "{input}");
    }
    assert_eq!(redact_secrets("no secrets here"), "no secrets here");
}

#[test]
fn requests_keep_opening_and_newest() {
    let requests: Vec<String> = (1..=9)
        .map(|i| format!("request number {i} with some words"))
        .chain(["ok".into()])
        .collect();
    let selection = select_request_lines(&requests, 600);
    assert_eq!(selection.lines[0], "request number 1 with some words");
    assert!(selection.lines[1].starts_with("(3 more requests in between"));
    assert_eq!(
        selection.lines.last().unwrap(),
        "request number 9 with some words"
    );
    assert_eq!(selection.shown.len(), 6);
}

#[test]
fn decisions_pick_instructions_and_skip_questions() {
    let requests = vec![
        "Please fix the parser. Don't add new dependencies to the project at all.".to_owned(),
        "Should we use serde for this or not?".to_owned(),
        "Which database? → Postgres".to_owned(),
    ];
    let decisions = select_decisions(&requests, 2000, &HashSet::new());
    assert_eq!(
        decisions,
        [
            "Don't add new dependencies to the project at all.",
            "Which database? → Postgres"
        ]
    );
}

#[test]
fn sentence_split_needs_capital_after_period() {
    assert_eq!(
        split_sentences("One. Two. e.g. three"),
        ["One.", "Two. e.g. three"]
    );
}

#[test]
fn commit_parsing() {
    assert_eq!(
        commit_lines("[main abc1234] fix: x 2 files changed"),
        [("abc1234".to_owned(), "fix: x".to_owned())]
    );
    let blocks = vec![
        Block::call(
            "bash",
            json!({ "command": "git commit -qm 'quiet commit'" }),
            1,
        ),
        Block::result("bash", String::new(), false, 2),
    ];
    assert_eq!(select_commits(&blocks, 500), ["quiet commit"]);
    let _ = Kind::User;
}

#[test]
fn compaction_instruction_parsing() {
    let marker = parse_compaction_instructions(Some(COMPACTOR_INSTRUCTION));
    assert!(marker.is_compactor && marker.keep == 1 && !marker.keep_explicit);
    let keep = parse_compaction_instructions(Some("__compactor__ keep:3"));
    assert!(keep.is_compactor && keep.keep == 3 && keep.keep_explicit);
    let user = parse_compaction_instructions(Some("focus on tests keep:2"));
    assert!(!user.is_compactor);
    assert_eq!(
        (user.keep, user.follow_up.as_deref()),
        (2, Some("focus on tests"))
    );
    assert_eq!(
        parse_keep_and_prompt("keep:4 then go"),
        ("then go".to_owned(), Some(4))
    );
    assert!(!parse_compaction_instructions(Some("__compactor__x")).is_compactor);
}

#[test]
fn token_format() {
    assert_eq!(
        [
            format_tokens(999),
            format_tokens(1234),
            format_tokens(45_600),
            format_tokens(1_500_000)
        ],
        ["999", "1.2k", "46k", "1.5M"]
    );
}
