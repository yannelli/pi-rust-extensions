use serde_json::json;

use super::*;
use crate::model::Scope;

fn hook(source: Source, layer: Layer, matcher: Option<&str>, command: &str) -> HookDef {
    let file = format!("/{}/{}.json", source.name(), layer.name());
    let handler = json!({ "type": "command", "command": command });
    let matcher = matcher.map(str::to_owned);
    let mut def = HookDef::new(
        source,
        Scope::Global,
        file.into(),
        "PreToolUse",
        matcher,
        handler,
    );
    def.layer = layer;
    def
}

#[test]
fn conflicts_need_different_handlers_from_two_layers_of_one_family() {
    let global = hook(Source::Claude, Layer::Global, Some("Bash"), "a");
    let project = hook(Source::Pi, Layer::Project(0), Some("Bash"), "b");
    let same = hook(Source::Claude, Layer::Local, Some("Bash"), "a");
    let codex = hook(Source::Codex, Layer::Project(0), Some("Bash"), "c");
    let all = hook(Source::Claude, Layer::Local, None, "d");
    let star = hook(Source::Claude, Layer::Global, Some("*"), "e");
    let found = find_conflicts(&[global.clone(), project, codex.clone(), all, star]);
    let texts: Vec<String> = found.iter().map(Conflict::text).collect();
    assert_eq!(
        texts,
        [
            "PreToolUse * (Claude/pi): global, local",
            "PreToolUse Bash (Claude/pi): global, project",
        ]
    );
    assert!(find_conflicts(&[global, same]).is_empty());
    let codex_global = hook(Source::Codex, Layer::Global, Some("Bash"), "c");
    assert!(find_conflicts(&[codex, codex_global]).is_empty());
}

#[test]
fn duplicates_across_claude_and_codex_are_listed() {
    let claude = hook(Source::Claude, Layer::Global, Some("Bash"), "fmt.sh");
    let codex = hook(Source::Codex, Layer::Global, Some("^Bash$"), "fmt.sh");
    let other = hook(Source::Codex, Layer::Global, Some("Bash"), "lint.sh");
    assert_eq!(
        cross_source_duplicates(&[claude, codex, other]),
        ["PreToolUse: fmt.sh (Claude/pi and Codex)"]
    );
}

#[test]
fn override_shadows_less_specific_layers_but_not_managed_hooks() {
    let global = hook(Source::Claude, Layer::Global, Some("Bash"), "a");
    let project = hook(Source::Claude, Layer::Project(0), Some("Bash"), "b");
    let deeper = hook(Source::Codex, Layer::Project(2), Some("Bash"), "c");
    let codex_root = hook(Source::Codex, Layer::Project(0), Some("Bash"), "d");
    let managed = hook(Source::Claude, Layer::Managed, Some("Bash"), "m");
    let hooks = [
        global.clone(),
        project.clone(),
        deeper,
        codex_root.clone(),
        managed.clone(),
    ];
    let shadowed = shadowed(&hooks, |_| true);
    let expected: HashSet<String> = [global.hash.clone(), codex_root.hash].into();
    assert_eq!(shadowed, expected);
    let inactive = super::shadowed(&hooks, |def| def.hash != project.hash);
    assert!(!inactive.contains(&global.hash));
    assert!(!inactive.contains(&managed.hash));
}

#[test]
fn disagreement_names_decisions_and_rewrites() {
    let allow = Outcome {
        permission: Some((Permission::Allow, None)),
        updated_input: Some(json!({ "command": "a" })),
        ..Outcome::default()
    };
    let deny = Outcome {
        block: Some("no".into()),
        ..Outcome::default()
    };
    let rewrite = Outcome {
        updated_input: Some(json!({ "command": "b" })),
        ..Outcome::default()
    };
    assert_eq!(
        disagreement("PreToolUse", "Bash", &[allow.clone(), deny, rewrite]).unwrap(),
        "hooks: PreToolUse hooks for Bash: the decisions are different (allow, deny), and deny \
         applies; 2 hooks changed the input, and the most specific change applies."
    );
    assert_eq!(disagreement("PreToolUse", "Bash", &[allow]), None);
}

#[test]
fn modes_parse_by_name() {
    for mode in ConflictMode::ALL {
        assert_eq!(ConflictMode::parse(mode.name()), Some(mode));
    }
    assert_eq!(ConflictMode::parse("graceful"), None);
}
