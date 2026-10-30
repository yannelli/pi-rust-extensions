use serde_json::json;

use crate::config::{self, Settings};
use crate::state::TaskStatus;
use crate::store::Store;
use crate::tests::{goal_with_tasks, temp_dir};

#[test]
fn config_layers_and_trust() {
    let agent = temp_dir("agent");
    let cwd = temp_dir("cwd");
    std::fs::create_dir_all(agent.join("ext")).unwrap();
    std::fs::create_dir_all(cwd.join(".pi/ext")).unwrap();
    std::fs::write(
        agent.join("ext/goal.json"),
        r#"{"disableTasks":true,"maxAutonomousRuns":5}"#,
    )
    .unwrap();
    std::fs::write(
        cwd.join(".pi/pi-goal-x-settings.json"),
        r#"{"maxAutonomousRuns":7}"#,
    )
    .unwrap();
    std::fs::write(
        cwd.join(".pi/ext/goal.json"),
        r#"{"model":"openai/gpt-5","disableAuditor":true}"#,
    )
    .unwrap();
    let ns = json!({ "maxAutonomousRuns": 6, "disableContracts": true });
    let loaded = config::load(&agent, &cwd, Some(&ns), false);
    assert!(loaded.settings.disable_tasks && loaded.settings.disable_contracts);
    assert_eq!(loaded.settings.max_autonomous_runs, Some(7));
    assert!(loaded.settings.disable_auditor);
    assert_eq!(loaded.settings.model, None);
    assert_eq!(loaded.ignored, vec!["model".to_owned()]);
    let trusted = config::load(&agent, &cwd, Some(&ns), true);
    assert_eq!(
        trusted.settings.auditor_model().as_deref(),
        Some("openai/gpt-5")
    );
    let default_dir = Settings::default().goals_dir(&agent, &cwd);
    assert!(default_dir.starts_with(agent.join("ext/goal/projects")));
    config::write_key(&agent.join("ext/goal.json"), "disableTasks", None).unwrap();
    assert!(
        !config::load(&agent, &cwd, None, false)
            .settings
            .disable_tasks
    );
}

#[test]
fn untrusted_namespace_preserves_global_auditor_settings() {
    let agent = temp_dir("namespace-agent");
    let cwd = temp_dir("namespace-cwd");
    std::fs::write(
        agent.join("settings.json"),
        json!({"goal":{"provider":"global-provider","model":"global-model"}}).to_string(),
    )
    .unwrap();
    let namespace =
        json!({"provider":"project-provider","model":"project-model","thinkingLevel":"high"});
    let untrusted = config::load(&agent, &cwd, Some(&namespace), false);
    assert_eq!(
        untrusted.settings.auditor_model().as_deref(),
        Some("global-provider/global-model")
    );
    assert_eq!(untrusted.settings.thinking_level, None);
    assert_eq!(untrusted.ignored, ["provider", "model", "thinkingLevel"]);
    let global_namespace = json!({"provider":"global-provider","model":"global-model"});
    let global_only = config::load(&agent, &cwd, Some(&global_namespace), false);
    assert!(global_only.ignored.is_empty());
    let trusted = config::load(&agent, &cwd, Some(&namespace), true);
    assert_eq!(
        trusted.settings.auditor_model().as_deref(),
        Some("project-provider/project-model")
    );
    std::fs::remove_dir_all(agent).unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn store_round_trip_and_archive() {
    let store = Store::new(temp_dir("store"));
    let goal = goal_with_tasks();
    store.save(&goal).unwrap();
    std::fs::write(store.root().join("broken.json"), "{").unwrap();
    let listing = store.list();
    assert_eq!(listing.goals, vec![goal.clone()]);
    assert_eq!(listing.unreadable.len(), 1);
    store.quarantine(&listing.unreadable[0]).unwrap();
    let archived = store.archive(&goal).unwrap();
    assert!(archived.is_file());
    let listing = store.list();
    assert!(listing.goals.is_empty() && listing.unreadable.is_empty());
}

#[test]
fn get_goal_pages() {
    let text = "x".repeat(crate::view::PAGE_CHARS + 10);
    let first = crate::view::page(&text, None).unwrap();
    assert!(first.ends_with(&format!("cursor=\"{}\"]", crate::view::PAGE_CHARS)));
    let rest = crate::view::page(&text, Some(&crate::view::PAGE_CHARS.to_string())).unwrap();
    assert_eq!(rest, "x".repeat(10));
    assert!(crate::view::page(&text, Some("abc")).is_err());
}

#[test]
fn widget_compact_and_expanded() {
    let goal = goal_with_tasks();
    let compact = crate::widget::lines(&goal, false, 80);
    assert!(compact[0].contains("goal: running"));
    assert!(compact.iter().any(|l| l.contains("[ ] Task a1")));
    let mut done = goal.clone();
    done.tasks[2].status = TaskStatus::Complete;
    let expanded = crate::widget::lines(&done, true, 80);
    assert!(expanded.iter().any(|l| l.contains("[x] Task b")));
    assert!(
        !crate::widget::lines(&done, false, 80)
            .iter()
            .any(|l| l.contains("Task b"))
    );
}

#[test]
fn prompt_sections() {
    let goal = goal_with_tasks();
    let text = crate::prompts::goal_prompt(&goal, &Settings::default());
    assert!(text.starts_with(&format!("[PI GOAL ACTIVE goalId={}]", goal.id)));
    assert!(text.contains("<untrusted_objective>\nShip it\n</untrusted_objective>"));
    assert!(text.contains("[TASK LIST — 0/3 tasks complete]"));
    assert!(crate::prompts::continuation_trigger("g1").len() <= 160);
    let draft = crate::draft::drafting_prompt("", crate::app::DraftMode::Sisyphus, None);
    assert!(draft.contains("focus=sisyphus") && draft.contains("no topic provided"));
}
