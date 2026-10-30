use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pi_ext_statusline::editor::{StatusLineEditor, edit_value};
use serde_json::{Value, json};

fn home() -> PathBuf {
    PathBuf::from("/nonexistent-home")
}

fn recording_editor(value: Option<Value>) -> (StatusLineEditor, Arc<Mutex<Vec<Value>>>) {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let writes = saved.clone();
    let save = Box::new(move |value: Value| {
        writes.lock().unwrap().push(value);
        Ok(())
    });
    (StatusLineEditor::new(value, home(), save), saved)
}

#[test]
fn context_choice_keeps_position_and_custom_pill_attributes() {
    let value = json!({"command": "saved.sh", "unknown": true, "segments": [
        ["dir", "fill", {"segment": "contextBar", "icon": "C", "color": "accent"}, "cost"],
        ["model", "contextUsed"]]});
    let edited = edit_value(&value, "context", "contextHeadroom", &home()).unwrap();
    assert_eq!(
        edited["segments"][0][2],
        json!({"segment": "contextHeadroom", "icon": "C", "color": "accent"})
    );
    assert_eq!(edited["segments"][1].as_array().unwrap().len(), 1);
    assert_eq!(edited["command"], "saved.sh");
    assert_eq!(edited["unknown"], true);
    let hidden = edit_value(&edited, "context", "hidden", &home()).unwrap();
    assert_eq!(hidden["segments"][0].as_array().unwrap().len(), 3);
    let reset = edit_value(
        &json!({"chip": "block", "style": "rounded"}),
        "chip",
        "preset",
        &home(),
    )
    .unwrap();
    assert!(reset.get("chip").is_none() && reset.get("style").is_none());
}

#[test]
fn keyboard_selection_saves_and_escape_closes() {
    let (mut editor, saved) = recording_editor(None);
    editor.select_item("preset");
    editor.handle_input("\r");
    editor.handle_input("compact");
    editor.handle_input("\r");
    assert_eq!(saved.lock().unwrap().len(), 1);
    assert_eq!(saved.lock().unwrap()[0]["preset"], "compact");
    assert!(!editor.closed);
    editor.handle_input("\x1b");
    assert!(editor.closed);
}

#[test]
fn module_editor_applies_changes_only_when_requested() {
    let (mut editor, saved) = recording_editor(Some(json!({"segments": ["model"]})));
    editor.select_item("segments");
    for key in ["\r", "\r", "remove", "\r", "\x1b"] {
        editor.handle_input(key);
    }
    assert!(saved.lock().unwrap().is_empty());
    assert!(!editor.closed);
    let keys = [
        "\r",
        "\r",
        "remove",
        "\r",
        "\r",
        "contextRemaining",
        "\r",
        "\x1b[B",
        "\x1b[B",
        "\r",
    ];
    for key in keys {
        editor.handle_input(key);
    }
    assert_eq!(saved.lock().unwrap().len(), 1);
    assert_eq!(
        saved.lock().unwrap()[0]["segments"],
        json!([["contextRemaining"]])
    );
}

#[test]
fn rejected_save_keeps_original_settings_and_reports_error() {
    let original = json!({"preset": "minimal"});
    let save = Box::new(|_| Err("read only".to_string()));
    let mut editor = StatusLineEditor::new(Some(original.clone()), home(), save);
    editor.apply_change("preset", "compact");
    assert_eq!(editor.value, original);
    assert_eq!(editor.error.as_deref(), Some("read only"));
    let lines = editor.render(60, &["preview".to_string()]);
    assert!(lines.iter().any(|line| line.contains("read only")));
    assert!(lines.iter().any(|line| line.contains("preview")));
}
