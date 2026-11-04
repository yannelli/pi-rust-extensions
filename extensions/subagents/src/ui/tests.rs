//! Unit tests of the panel: list badges, the form, pickers, and the file flows.

use std::path::{Path, PathBuf};

use pi_ext_sdk::{Key, style};
use serde_json::json;

use super::draft::Field;
use super::paint::Paint;
use super::picker::Item;
use super::state::{Effect, Focus, Setup, State, Tab};
use super::view;
use crate::registry::Layers;

pub struct Fixture {
    _root: pi_ext_e2e::TempDir,
    pub global: PathBuf,
    pub project: PathBuf,
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A global reviewer, a project override of explore, and a plugin agent with its state file.
pub fn fixture() -> Fixture {
    let root = pi_ext_e2e::TempDir::new().unwrap();
    let base = root.path().join("agent");
    let global = base.join("ext/subagents/agents");
    let project = root.path().join("work/.pi/ext/subagents/agents");
    write(
        &global.join("reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews diffs.\ntools: read, grep\nmodel: openai/gpt-5\nexposure: deferred\n---\nReview the diff.\n",
    );
    write(
        &project.join("explore.md"),
        "---\nname: explore\nthinking: high\n---\n",
    );
    write(
        &global.join("fmt-rev.md"),
        "---\nname: fmt-rev\ndescription: Formats code.\n---\nFormat.\n",
    );
    let state = json!({ "plugins": { "fmt@market": { "plugin": "fmt", "files": [global.join("fmt-rev.md")] } } });
    write(
        &base.join("ext/claude-marketplace/state.json"),
        &state.to_string(),
    );
    Fixture {
        _root: root,
        global,
        project,
    }
}

pub fn state(f: &Fixture) -> State {
    let tools = ["read", "grep", "find", "ls", "bash", "edit", "write"];
    State::new(Setup {
        layers: Layers {
            global: f.global.clone(),
            project: Some(f.project.clone()),
        },
        models: [
            "anthropic/claude-sonnet-4-6",
            "anthropic/claude-haiku-4-5",
            "openai/gpt-5",
        ]
        .map(|m| Item::new(m, ""))
        .to_vec(),
        tools: tools.map(|t| Item::new(t, "")).to_vec(),
        advertise: true,
        context_entries: false,
    })
}

pub fn keys(state: &mut State, keys: &[Key]) -> Effect {
    let mut last = Effect::None;
    for key in keys {
        last = state.key(key, &[]);
    }
    last
}

pub fn typed(state: &mut State, text: &str) {
    for c in text.chars() {
        state.key(&Key::Char(c), &[]);
    }
}

pub fn screen(state: &State, width: usize) -> String {
    let lines = view::render(state, &Paint::default(), &[], width, 40);
    lines
        .iter()
        .map(|l| style::strip(l))
        .collect::<Vec<_>>()
        .join("\n")
}

fn select(state: &mut State, name: &str) {
    state.row = state
        .visible()
        .iter()
        .position(|&i| state.entries[i].def.name == name)
        .unwrap();
    state.sync();
}

fn field(state: &mut State, field: Field) {
    state.focus = Focus::Form;
    state.field = super::draft::FIELDS
        .iter()
        .position(|f| *f == field)
        .unwrap();
}

#[test]
fn list_shows_badges_overrides_model_and_exposure() {
    let f = fixture();
    let s = state(&f);
    let text = screen(&s, 140);
    let row = |name: &str| {
        text.lines()
            .find(|l| l.contains(&format!(" {name} ")))
            .unwrap()
            .to_owned()
    };
    assert!(row("explore").contains("built-in   ✎ project"), "{text}");
    assert!(
        row("build").contains("built-in") && !row("build").contains('✎'),
        "{text}"
    );
    assert!(
        row("reviewer").contains("global") && row("reviewer").contains("gpt-5 · deferred"),
        "{text}"
    );
    assert!(row("fmt-rev").contains("plugin fmt"), "{text}");
    assert!(
        text.contains("+ New agent") && text.contains("[Agents (5)]"),
        "{text}"
    );
    assert!(text.contains("Layers       built-in → project"), "{text}");
    assert!(text.contains("n new") && text.contains("q close"), "{text}");
    let narrow = screen(&s, 60);
    assert!(
        narrow.contains("Description") && narrow.contains("explore"),
        "{narrow}"
    );
}

#[test]
fn form_edits_fields_and_saves_only_changes() {
    let f = fixture();
    let mut s = state(&f);
    select(&mut s, "reviewer");
    field(&mut s, Field::Description);
    keys(&mut s, &[Key::Enter, Key::Ctrl('u')]);
    typed(&mut s, "Reviews diffs and lists bugs.");
    keys(&mut s, &[Key::Enter]);
    field(&mut s, Field::Thinking);
    keys(&mut s, &[Key::Right, Key::Right]);
    field(&mut s, Field::MaxTurns);
    keys(&mut s, &[Key::Enter]);
    typed(&mut s, "12");
    keys(&mut s, &[Key::Enter]);
    assert!(s.dirty());
    assert!(screen(&s, 140).contains("● unsaved"));
    keys(&mut s, &[Key::Char('s')]);
    assert!(!s.dirty(), "{:?}", s.message);
    let names: Vec<String> = std::fs::read_dir(&f.global)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| n.ends_with(".md")),
        "no temp file stays: {names:?}"
    );
    let text = std::fs::read_to_string(f.global.join("reviewer.md")).unwrap();
    assert_eq!(
        text,
        "---\nname: reviewer\ndescription: Reviews diffs and lists bugs.\ntools: read, grep\nmodel: openai/gpt-5\nexposure: deferred\nthinking: minimal\nmaxTurns: 12\n---\n\nReview the diff.\n"
    );
}

#[test]
fn validation_blocks_the_save() {
    let f = fixture();
    let mut s = state(&f);
    select(&mut s, "reviewer");
    field(&mut s, Field::MaxTurns);
    keys(&mut s, &[Key::Enter]);
    typed(&mut s, "lots");
    keys(&mut s, &[Key::Enter]);
    assert!(s.message.as_ref().unwrap().1.contains("not a whole number"));
    keys(&mut s, &[Key::Char('s')]);
    assert!(
        s.message
            .as_ref()
            .unwrap()
            .1
            .starts_with("Not saved: Max turns")
    );
    field(&mut s, Field::Description);
    keys(&mut s, &[Key::Enter, Key::Ctrl('u'), Key::Enter]);
    assert!(screen(&s, 140).contains("✗ Description is empty"));
    let text = std::fs::read_to_string(f.global.join("reviewer.md")).unwrap();
    assert!(
        text.contains("description: Reviews diffs."),
        "the file is unchanged"
    );
}

#[test]
fn model_search_and_tool_toggles() {
    let f = fixture();
    let mut s = state(&f);
    select(&mut s, "reviewer");
    field(&mut s, Field::Model);
    keys(&mut s, &[Key::Enter]);
    typed(&mut s, "haiku");
    assert!(screen(&s, 140).contains("❯ anthropic/claude-haiku-4-5"));
    keys(&mut s, &[Key::Enter]);
    assert_eq!(
        s.draft.as_ref().unwrap().values.model.as_deref(),
        Some("anthropic/claude-haiku-4-5")
    );
    field(&mut s, Field::Tools);
    keys(&mut s, &[Key::Enter]);
    typed(&mut s, "bash");
    keys(&mut s, &[Key::Char(' '), Key::Escape]);
    assert!(screen(&s, 140).contains("[✓] bash"));
    keys(&mut s, &[Key::Enter]);
    let tools = s.draft.as_ref().unwrap().values.tools.clone();
    assert_eq!(
        tools,
        Some(vec!["read".into(), "grep".into(), "bash".into()])
    );
    keys(&mut s, &[Key::Enter, Key::Char(' '), Key::Enter]);
    assert_eq!(
        s.draft.as_ref().unwrap().values.tools,
        None,
        "the all row inherits every tool"
    );
    keys(&mut s, &[Key::Char('s')]);
    let text = std::fs::read_to_string(f.global.join("reviewer.md")).unwrap();
    assert!(
        text.contains("tools: all\nmodel: anthropic/claude-haiku-4-5\n"),
        "{text}"
    );
}

#[test]
fn builtin_save_asks_the_scope_and_leave_asks_to_save() {
    let f = fixture();
    let mut s = state(&f);
    select(&mut s, "build");
    field(&mut s, Field::Exposure);
    keys(&mut s, &[Key::Right, Key::Escape]);
    assert!(screen(&s, 140).contains("The agent has unsaved changes"));
    keys(&mut s, &[Key::Enter]);
    assert!(screen(&s, 140).contains("Save the build override to"));
    keys(&mut s, &[Key::Char('1')]);
    assert_eq!(s.focus, Focus::List, "the save continues to the list");
    let text = std::fs::read_to_string(f.global.join("build.md")).unwrap();
    assert_eq!(text, "---\nname: build\nexposure: direct\n---\n");
    select(&mut s, "build");
    assert!(s.selected().unwrap().override_scope().is_some());
}

#[test]
fn creates_from_a_template_copies_and_renames() {
    let f = fixture();
    let mut s = state(&f);
    keys(&mut s, &[Key::Char('n'), Key::Char('3')]);
    typed(&mut s, "Critic");
    keys(&mut s, &[Key::Enter, Key::Char('2')]);
    let critic = f.project.join("critic.md");
    let text = std::fs::read_to_string(&critic).unwrap();
    assert!(
        text.starts_with("---\nname: critic\ndescription: Reviewer that"),
        "{text}"
    );
    assert!(
        text.contains("tools: read, grep, find, ls\nmodel: inherit\n"),
        "{text}"
    );
    assert_eq!(
        (s.focus, s.selected().unwrap().def.name.as_str()),
        (Focus::Form, "critic")
    );
    keys(&mut s, &[Key::Escape, Key::Char('n'), Key::Char('1')]);
    typed(&mut s, "critic");
    keys(&mut s, &[Key::Enter]);
    assert!(
        s.message.as_ref().unwrap().1.contains("exists"),
        "taken names are refused"
    );
    keys(&mut s, &[Key::Escape]);
    select(&mut s, "reviewer");
    keys(&mut s, &[Key::Char('c'), Key::Enter, Key::Char('1')]);
    let copy = std::fs::read_to_string(f.global.join("reviewer-copy.md")).unwrap();
    assert!(
        copy.contains("model: openai/gpt-5") && copy.contains("Review the diff."),
        "{copy}"
    );
    keys(&mut s, &[Key::Escape, Key::Char('r'), Key::Ctrl('u')]);
    typed(&mut s, "second-opinion");
    keys(&mut s, &[Key::Enter]);
    assert!(!f.global.join("reviewer-copy.md").exists());
    let renamed = std::fs::read_to_string(f.global.join("second-opinion.md")).unwrap();
    assert!(
        renamed.starts_with("---\nname: second-opinion\n"),
        "{renamed}"
    );
    select(&mut s, "build");
    keys(&mut s, &[Key::Char('r')]);
    assert!(s.message.as_ref().unwrap().1.contains("built-in"));
}

#[test]
fn delete_reset_and_scope_move() {
    let f = fixture();
    let mut s = state(&f);
    select(&mut s, "reviewer");
    keys(&mut s, &[Key::Char('m'), Key::Enter]);
    assert!(!f.global.join("reviewer.md").exists() && f.project.join("reviewer.md").exists());
    assert_eq!(
        s.selected().unwrap().def.source,
        crate::agent::Source::Project
    );
    keys(&mut s, &[Key::Char('d')]);
    assert!(screen(&s, 140).contains("Delete reviewer?"));
    keys(&mut s, &[Key::Char('2')]);
    assert!(
        f.project.join("reviewer.md").exists(),
        "cancel keeps the file"
    );
    keys(&mut s, &[Key::Char('d'), Key::Enter]);
    assert!(!f.project.join("reviewer.md").exists());
    assert!(s.entries.iter().all(|e| e.def.name != "reviewer"));
    select(&mut s, "explore");
    keys(&mut s, &[Key::Char('d'), Key::Enter]);
    assert!(!f.project.join("explore.md").exists());
    assert!(
        !s.selected().unwrap().override_scope().is_some(),
        "reset to the built-in"
    );
    select(&mut s, "fmt-rev");
    keys(&mut s, &[Key::Char('d')]);
    assert!(s.message.as_ref().unwrap().1.contains("/claude-plugins"));
}

#[test]
fn prompt_file_and_runs_tab() {
    let f = fixture();
    let mut s = state(&f);
    select(&mut s, "reviewer");
    let Effect::EditPrompt { text, .. } = keys(&mut s, &[Key::Char('p')]) else {
        panic!("p opens the editor");
    };
    assert_eq!(text, "Review the diff.");
    s.prompt_edited(Some("Review it well.\n".into()));
    assert!(s.dirty() && s.focus == Focus::Form);
    assert!(
        matches!(keys(&mut s, &[Key::Char('f')]), Effect::None),
        "a dirty form blocks f"
    );
    keys(&mut s, &[Key::Char('s'), Key::Escape]);
    let Effect::EditFile { path, text } = keys(&mut s, &[Key::Char('f')]) else {
        panic!("f opens the file");
    };
    assert!(text.ends_with("Review it well.\n"), "{text}");
    s.file_edited(&path, Some(text.replace("name: reviewer", "name: other")));
    assert!(s.message.as_ref().unwrap().1.contains("Rename with r"));
    keys(&mut s, &[Key::Char('u')]);
    assert_eq!(
        (s.tab, s.runs_agent.as_deref()),
        (Tab::Runs, Some("reviewer"))
    );
    assert!(screen(&s, 140).contains("Runs of reviewer"));
    keys(&mut s, &[Key::Char('a')]);
    assert!(screen(&s, 140).contains("Runs of all agents"));
    keys(&mut s, &[Key::Tab]);
    assert_eq!(s.tab, Tab::Agents);
    assert_eq!(keys(&mut s, &[Key::Char('q')]), Effect::Close);
}

#[test]
fn search_filters_the_list() {
    let f = fixture();
    let mut s = state(&f);
    keys(&mut s, &[Key::Char('/')]);
    typed(&mut s, "rev");
    keys(&mut s, &[Key::Enter]);
    let names: Vec<&str> = s
        .visible()
        .iter()
        .map(|&i| s.entries[i].def.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["reviewer", "fmt-rev", "advisor"],
        "name matches first, then descriptions"
    );
    keys(&mut s, &[Key::Escape]);
    assert_eq!(s.visible().len(), 5);
}
