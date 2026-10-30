mod fixtures;

use fixtures::*;
use pi_ext_observational_memory::ledger::*;
use pi_ext_observational_memory::serialize::serialize_source_chunk;
use pi_ext_observational_memory::tokens::{estimate_entry_tokens, hash_id};
use serde_json::json;

#[test]
fn hash_id_is_twelve_hex_of_sha256() {
    assert_eq!(hash_id("hello"), "2cf24dba5fb0");
    assert!(is_memory_id(&hash_id("anything")));
    assert!(!is_memory_id("2CF24DBA5FB0"));
    assert!(!is_memory_id("2cf24dba5fb"));
}

#[test]
fn entry_tokens_follow_pi_estimate() {
    assert_eq!(estimate_entry_tokens(&user("u", "abcde")), 2);
    let call = json!({ "type": "message", "id": "a", "message": { "role": "assistant",
        "content": [{ "type": "toolCall", "name": "ls", "arguments": {} }] } });
    assert_eq!(estimate_entry_tokens(&call), 1);
    let custom = json!({ "type": "custom_message", "id": "c", "content": "12345678" });
    assert_eq!(estimate_entry_tokens(&custom), 2);
}

#[test]
fn projection_and_summary_from_branch_entries() {
    let o1 = observation("User stated they use Postgres.", "critical", &["u0"]);
    let o2 = observation("Agent ran tests and they passed.", "low", &["u1"]);
    let r1 = reflection("User uses Postgres for the project database.", &[&o1.id]);
    let entries = vec![
        user("u0", "I use Postgres"),
        user("u1", "run tests"),
        observations_entry("m1", &[o1.clone(), o2.clone()], "u1"),
        reflections_entry("m2", std::slice::from_ref(&r1), "u1"),
        dropped_entry("m3", &[&o2.id], "u1"),
        user("u2", "next"),
    ];

    let folded = fold_ledger(&entries, None);
    assert_eq!(folded.observations.len(), 2);
    assert_eq!(folded.active_observations, vec![o1.clone()]);

    let compact = build_compaction_projection(&entries, "u2", 20_000);
    assert!(!compact.full_fold);
    assert_eq!(
        compact.projection.observations,
        vec![o1.clone(), o2.clone()]
    );
    assert!(compact.projection.reflections.is_empty());
    assert_eq!(compact.details.kind, "om.folded");

    let full = build_compaction_projection(&entries, "u2", 1);
    assert!(full.full_fold);
    assert_eq!(full.projection.observations, vec![o1.clone()]);
    assert_eq!(full.projection.reflections, vec![r1.clone()]);

    let summary = render_summary(&full.projection.reflections, &full.projection.observations);
    assert!(summary.starts_with("These are condensed memories from earlier in this session."));
    assert!(summary.contains(&format!("## Reflections\n[{}] {}", r1.id, r1.content)));
    assert!(summary.contains(&format!(
        "## Observations\n[{}] 2026-10-29 10:00 [critical] User stated they use Postgres.",
        o1.id
    )));
    assert_eq!(render_summary(&[], &[]), "");
}

#[test]
fn full_fold_boundary_unlocks_reflections_and_drops() {
    let o1 = observation("first", "low", &["u0"]);
    let r1 = reflection("durable", &[&o1.id]);
    let details = json!({ "type": "om.folded", "version": 1, "fullFold": true,
        "observations": [], "reflections": [] });
    let entries = vec![
        user("u0", "a"),
        observations_entry("m1", std::slice::from_ref(&o1), "u0"),
        reflections_entry("m2", std::slice::from_ref(&r1), "u0"),
        dropped_entry("m3", &[&o1.id], "u0"),
        user("u1", "b"),
        compaction("c1", "u1", details),
        user("u2", "c"),
    ];
    assert_eq!(
        latest_full_fold_boundary_id(&entries).as_deref(),
        Some("u1")
    );
    let projection = build_compaction_projection(&entries, "u2", 20_000).projection;
    assert!(projection.observations.is_empty());
    assert_eq!(projection.reflections, vec![r1]);
    assert!(visible_projection(&entries, None).observations.is_empty());
}

#[test]
fn coverage_and_token_progress() {
    let o1 = observation("seen", "low", &["u0"]);
    let mut entries = conversation(2, 400);
    entries.push(observations_entry("m1", &[o1], "u0"));
    assert_eq!(latest_coverage_index(&entries, OM_OBSERVATIONS_RECORDED), 0);
    assert_eq!(raw_tokens_since_observation_coverage(&entries), 100);
    assert_eq!(raw_tokens_since_reflection_coverage(&entries), 200);
    assert_eq!(
        earlier_coverage_marker_id(&entries, Some("u1"), Some("u0")).as_deref(),
        Some("u0")
    );

    let entries = vec![
        assistant("a0", "x", Some(1_000)),
        observations_entry("m1", &[observation("o", "low", &["a0"])], "a0"),
    ];
    assert_eq!(
        real_tokens_since_anchor(&entries, Some(OM_OBSERVATIONS_RECORDED), 4_000),
        Some(3_000)
    );
    assert_eq!(
        real_tokens_since_anchor(&entries, Some(OM_OBSERVATIONS_RECORDED), 500),
        None
    );
    assert_eq!(
        real_tokens_since_anchor(&[], Some(OM_OBSERVATIONS_RECORDED), 700),
        Some(700)
    );

    let entries = vec![
        user("u0", &"y".repeat(800)),
        compaction("c1", "u1", json!({})),
        user("u1", &"z".repeat(40)),
    ];
    assert_eq!(raw_tokens_since_last_compaction(&entries), 10);
}

#[test]
fn source_chunk_caps_and_truncates() {
    let entries = conversation(3, 400);
    let all = serialize_source_chunk(&entries, None);
    assert_eq!(all.source_entry_ids, ["u0", "u1", "u2"]);
    assert!(all.text.starts_with("[Source entry id: u0]\n[User @ "));

    let capped = serialize_source_chunk(&entries, Some(250));
    assert_eq!(capped.source_entry_ids, ["u0", "u1"]);
    assert!(capped.truncated_source_entry_ids.is_empty());

    let big = vec![user("u0", &"q".repeat(10_000))];
    let excerpt = serialize_source_chunk(&big, Some(300));
    assert_eq!(excerpt.truncated_source_entry_ids, ["u0"]);
    assert!(excerpt.text.contains("middle omitted"));
    assert!(excerpt.estimated_tokens <= 300);
}

#[test]
fn recall_finds_sources_and_collisions() {
    let o1 = observation("User stated they use Postgres.", "high", &["u0", "gone"]);
    let r1 = reflection("Postgres is the database.", &[&o1.id, "aaaaaaaaaaaa"]);
    let entries = vec![
        user("u0", "I use Postgres"),
        observations_entry("m1", std::slice::from_ref(&o1), "u0"),
        reflections_entry("m2", std::slice::from_ref(&r1), "u0"),
    ];
    let found = recall_memory_sources(&entries, &o1.id).unwrap();
    assert_eq!(found.kind, "observation");
    assert_eq!(found.missing_source_entry_ids, ["gone"]);
    assert!(found.partial);
    let found = recall_memory_sources(&entries, &r1.id).unwrap();
    assert_eq!(found.kind, "reflection");
    assert_eq!(found.missing_supporting_observation_ids, ["aaaaaaaaaaaa"]);
    assert_eq!(found.source_entries.len(), 1);
    assert!(recall_memory_sources(&entries, "bbbbbbbbbbbb").is_none());

    let twice = vec![
        observations_entry("m1", std::slice::from_ref(&o1), "u0"),
        observations_entry("m2", std::slice::from_ref(&o1), "u0"),
    ];
    assert!(recall_memory_sources(&twice, &o1.id).unwrap().collision);
}
