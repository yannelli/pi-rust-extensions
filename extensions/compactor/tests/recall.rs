mod common;

use pi_ext_compactor::recall::drill::parse_drill;
use pi_ext_compactor::recall::search::search;
use pi_ext_compactor::recall::{RecallInput, parse_command_args, vcc_recall};
use pi_ext_compactor::text::{Block, Kind, recall_blocks};
use serde_json::json;

fn recall(input: RecallInput) -> String {
    vcc_recall(&recall_blocks(&common::branch()), &input, "/work")
}

fn query(q: &str) -> RecallInput {
    RecallInput {
        query: Some(q.into()),
        ..RecallInput::default()
    }
}

#[test]
fn keyword_search_ranks_and_indexes_by_entry() {
    let text = recall(query("dependencies standard library"));
    assert!(
        text.starts_with("1 matches for \"dependencies standard library\":"),
        "{text}"
    );
    assert!(
        text.contains("#7 [user] Don't add new dependencies"),
        "{text}"
    );
}

#[test]
fn regex_search_and_no_match() {
    let text = recall(query("E04\\d+"));
    assert!(text.contains("#6 [tool_result]"), "{text}");
    assert_eq!(recall(query("zebra")), "No matches for \"zebra\".");
}

#[test]
fn paging_splits_five_per_page() {
    let blocks: Vec<Block> = (0..12)
        .map(|i| Block::text(Kind::User, format!("deploy step {i}"), i))
        .collect();
    let page1 = vcc_recall(&blocks, &query("deploy"), "/work");
    assert!(page1.starts_with("Page 1/3 (12 total matches)"), "{page1}");
    assert!(
        page1.ends_with("--- Use page:2 for more results ---"),
        "{page1}"
    );
    let page3 = vcc_recall(
        &blocks,
        &RecallInput {
            page: Some(3.0),
            ..query("deploy")
        },
        "/work",
    );
    assert!(page3.starts_with("Page 3/3"), "{page3}");
    assert!(!page3.contains("Use page:"));
    let page9 = vcc_recall(
        &blocks,
        &RecallInput {
            page: Some(9.0),
            ..query("deploy")
        },
        "/work",
    );
    assert_eq!(page9, "No matches for \"deploy\" on page 9.");
    assert_eq!(search(&blocks, None).len(), 12);
}

#[test]
fn expand_returns_full_entries_and_rejects_unknown() {
    let text = recall(RecallInput {
        expand: Some(vec![3.0]),
        ..RecallInput::default()
    });
    assert!(text.starts_with("#3 [tool_call] edit {"), "{text}");
    let bad = recall(RecallInput {
        expand: Some(vec![99.0]),
        ..RecallInput::default()
    });
    assert_eq!(bad, "Cannot expand indices outside active lineage: 99");
}

#[test]
fn touched_mode_lists_files() {
    let text = recall(RecallInput {
        mode: Some("touched".into()),
        ..RecallInput::default()
    });
    assert_eq!(
        text,
        "1 files touched:\n\n  ./src/main.rs    #1 (read), #3 (edit)"
    );
}

#[test]
fn drill_down_into_edit() {
    let drill = parse_drill("#3:main.rs:full").unwrap();
    assert!(drill.full);
    let text = recall(query("#3:main.rs"));
    assert_eq!(
        text,
        "File: /work/src/main.rs\nTool: edit\n\n--- old ---\nfn main() {}\n--- new ---\nfn main() { parse(); }"
    );
    assert_eq!(
        recall(query("#50:x")),
        "Entry #50 not found in session history."
    );
    assert!(parse_drill("see #3:main.rs").is_none());
}

#[test]
fn recent_entries_without_query() {
    let text = recall(RecallInput::default());
    assert!(text.starts_with("Session history ("), "{text}");
}

#[test]
fn command_args_parse_scope_and_page() {
    assert_eq!(
        parse_command_args("redis cache scope:all page:2"),
        ("redis cache".to_owned(), true, 2)
    );
    assert_eq!(parse_command_args("plain"), ("plain".to_owned(), false, 1));
}

#[test]
fn noise_filter_drops_wrappers() {
    let entries = vec![
        json!({ "type": "message", "id": "a", "message": { "role": "user", "content": "<system-reminder>x</system-reminder>" } }),
    ];
    assert!(recall_blocks(&entries).is_empty());
}
